//! Reading a shared library's dynamic symbol table.
//!
//! Responsibility: answer one question -- does this image export this symbol to
//! `dlsym`? -- by reading the ELF header, the section header table, `.dynsym` and
//! `.dynstr` directly. Nothing outside this module needs to know how.
//!
//! # Why the check exists
//!
//! Fcitx5 resolves `fcitx_addon_factory_instance` with `dlsym` after `dlopen`. A
//! library built with `--strip-all` in the linker invocation, or stripped with
//! `strip --strip-all` afterwards, loses that name from its dynamic symbol table and
//! becomes a file that loads and contains no addon at all -- Fcitx5 reports nothing
//! and the input method simply never appears. The installer strips the library it
//! installs, so it has to confirm the symbol survived, and it has to do so on the file
//! it is about to copy rather than on a build tree it trusts.
//!
//! # Why this is not an `nm` invocation
//!
//! Shelling out to `nm -D` would make the check depend on binutils being installed on
//! the machine doing the install, and parsing its human-readable output would be
//! guessing at a format that is free to change. The table it reads is a few hundred
//! bytes of fixed layout, so it is read here instead.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};

/// Leading bytes of every ELF image.
const MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];

/// `EI_CLASS` value for a 64-bit image.
const CLASS_64: u8 = 2;

/// `EI_DATA` value for a little-endian image.
const DATA_LSB: u8 = 1;

/// `SHT_DYNSYM`: the dynamic symbol table.
const SHT_DYNSYM: u32 = 11;

/// `SHN_UNDEF`: a symbol this image imports rather than defines.
const SHN_UNDEF: u16 = 0;

/// `STB_LOCAL`: a binding `dlsym` cannot resolve, because it is not exported.
const STB_LOCAL: u8 = 0;

/// Size of one `Elf64_Shdr`.
const SHDR_BYTES: usize = 64;

/// Size of one `Elf64_Sym`.
const SYM_BYTES: usize = 24;

/// Whether `image` exports `symbol` to `dlsym`.
///
/// A symbol counts as exported when the dynamic symbol table holds it with a defined
/// section and a binding `dlsym` can see; a name that is only present in `.dynstr`
/// because some other image's symbol was imported there is not an export.
///
/// # Errors
///
/// Returns an error when the image is not a 64-bit little-endian ELF, when its header
/// or section header table is truncated, or when it has no dynamic symbol table -- the
/// last of which is itself the signature of an over-stripped library, so the caller
/// refuses the file rather than assuming it is fine.
pub fn exports(image: &[u8], symbol: &str) -> Result<bool> {
    let sections = sections(image)?;
    let dynsym = sections
        .iter()
        .find(|section| section.kind == SHT_DYNSYM)
        .context("the image has no .dynsym section, so its exports cannot be checked")?;
    let dynstr = sections
        .get(dynsym.link as usize)
        .context(".dynsym points at a section that is not in the image")?;
    let names = slice(image, dynstr.offset, dynstr.size)?;
    let table = slice(image, dynsym.offset, dynsym.size)?;
    let entry = if dynsym.entry_size == 0 {
        SYM_BYTES
    } else {
        dynsym.entry_size
    };
    for bytes in table.chunks_exact(entry) {
        let name = u32_at(bytes, 0)? as usize;
        let info = *bytes.get(4).context("a symbol entry is truncated")?;
        let section = u16_at(bytes, 6)?;
        if info >> 4 == STB_LOCAL || section == SHN_UNDEF {
            continue;
        }
        if cstring(names, name)? == symbol {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether the file at `path` exports `symbol` to `dlsym`.
///
/// # Errors
///
/// Returns an error when the file cannot be read, and as [`exports`].
pub fn exports_path(path: &Path, symbol: &str) -> Result<bool> {
    let image = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    exports(&image, symbol)
        .with_context(|| format!("{}: not an inspectable library", path.display()))
}

/// One `Elf64_Shdr`, reduced to the fields the symbol lookup needs.
#[derive(Debug, Clone, Copy)]
struct Section {
    /// `sh_type`.
    kind: u32,
    /// `sh_offset`, as an index into the image.
    offset: usize,
    /// `sh_size`.
    size: usize,
    /// `sh_link`: for `.dynsym`, the index of the string table its names live in.
    link: u32,
    /// `sh_entsize`.
    entry_size: usize,
}

/// Reads the section header table of a 64-bit little-endian ELF image.
///
/// # Errors
///
/// Returns an error when the image is not a 64-bit little-endian ELF, when its header
/// is truncated, or when the section header table reaches past the end of the image.
fn sections(image: &[u8]) -> Result<Vec<Section>> {
    ensure!(
        image.len() >= 64,
        "the file is {} bytes long, too short to be an ELF image",
        image.len()
    );
    ensure!(
        image.get(..4) == Some(&MAGIC),
        "the file does not start with the ELF magic"
    );
    let class = *image.get(4).context("the image has no EI_CLASS byte")?;
    ensure!(class == CLASS_64, "only 64-bit ELF images are supported");
    let data = *image.get(5).context("the image has no EI_DATA byte")?;
    ensure!(
        data == DATA_LSB,
        "only little-endian ELF images are supported"
    );
    let offset = u64_at(image, 0x28)? as usize;
    let entry_size = u16_at(image, 0x3A)? as usize;
    let count = u16_at(image, 0x3C)? as usize;
    ensure!(
        offset != 0 && count != 0,
        "the image has no section header table"
    );
    ensure!(
        entry_size >= SHDR_BYTES,
        "the section header table declares {entry_size}-byte entries"
    );
    let table = slice(image, offset, entry_size.saturating_mul(count))?;
    table
        .chunks_exact(entry_size)
        .map(|header| {
            Ok(Section {
                kind: u32_at(header, 4)?,
                offset: u64_at(header, 0x18)? as usize,
                size: u64_at(header, 0x20)? as usize,
                link: u32_at(header, 0x28)?,
                entry_size: u64_at(header, 0x38)? as usize,
            })
        })
        .collect()
}

/// The bytes at `offset` for `size`, or an error naming what is missing.
fn slice(image: &[u8], offset: usize, size: usize) -> Result<&[u8]> {
    let end = offset
        .checked_add(size)
        .context("a section header declares an offset past the end of the image")?;
    image.get(offset..end).with_context(|| {
        format!(
            "a section runs from byte {offset} to byte {end}, past the {} bytes of the image",
            image.len()
        )
    })
}

/// Reads a little-endian `u16` at `offset`.
fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    let field = slice(bytes, offset, 2)?;
    Ok(u16::from_le_bytes([field[0], field[1]]))
}

/// Reads a little-endian `u32` at `offset`.
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    let field = slice(bytes, offset, 4)?;
    Ok(u32::from_le_bytes([field[0], field[1], field[2], field[3]]))
}

/// Reads a little-endian `u64` at `offset`.
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64> {
    let field = slice(bytes, offset, 8)?;
    let mut value = [0u8; 8];
    value.copy_from_slice(field);
    Ok(u64::from_le_bytes(value))
}

/// The NUL-terminated string at `offset` in a string table.
///
/// # Errors
///
/// Returns an error when the offset is past the end of the table or the entry is not
/// terminated, either of which means the image is malformed rather than merely
/// missing a symbol.
fn cstring(table: &[u8], offset: usize) -> Result<&str> {
    let rest = table
        .get(offset..)
        .context("a symbol name starts past the end of the string table")?;
    let end = rest
        .iter()
        .position(|byte| *byte == 0)
        .context("a symbol name in the string table is not terminated")?;
    std::str::from_utf8(&rest[..end]).context("a symbol name is not UTF-8")
}

/// A minimal but well-formed 64-bit little-endian ELF image whose dynamic symbol table
/// holds `symbols`, each paired with whether it is defined.
///
/// A real shared library is the wrong fixture for the symbol check: what matters is the
/// difference between a name this image defines and one that is merely present in its
/// string table, and only a fixture that says which is which can assert that.
#[cfg(test)]
pub(super) fn synthetic_image(symbols: &[(&str, bool)]) -> Vec<u8> {
    let (shstrtab, names) = string_table(&[".shstrtab", ".dynstr", ".dynsym"]);
    let symbol_names: Vec<&str> = symbols.iter().map(|(name, _)| *name).collect();
    let (dynstr, offsets) = string_table(&symbol_names);
    let contents = [shstrtab, dynstr, symbol_table(symbols, &offsets)];
    let kinds = [3u32, 3, SHT_DYNSYM];

    let mut body = Vec::new();
    let mut sections = Vec::new();
    for (index, (content, kind)) in contents.iter().zip(kinds).enumerate() {
        while body.len() % 8 != 0 {
            body.push(0);
        }
        let offset = 64 + body.len();
        sections.push((names[index], kind, offset, content.len()));
        body.extend_from_slice(content);
    }
    write_image(&body, &sections)
}

/// A NUL-terminated string table, and the offset of each entry in it.
#[cfg(test)]
fn string_table(entries: &[&str]) -> (Vec<u8>, Vec<u32>) {
    let mut table = vec![0u8];
    let mut offsets = Vec::new();
    for entry in entries {
        offsets.push(table.len() as u32);
        table.extend_from_slice(entry.as_bytes());
        table.push(0);
    }
    (table, offsets)
}

/// A `.dynsym` section: the null entry, then one global entry per symbol.
#[cfg(test)]
fn symbol_table(symbols: &[(&str, bool)], name_offsets: &[u32]) -> Vec<u8> {
    let mut table = vec![0u8; SYM_BYTES];
    for ((_, defined), name) in symbols.iter().zip(name_offsets) {
        let mut entry = vec![0u8; SYM_BYTES];
        entry[0..4].copy_from_slice(&name.to_le_bytes());
        entry[4] = 1 << 4; // STB_GLOBAL, STT_NOTYPE
        entry[6..8].copy_from_slice(&u16::from(*defined).to_le_bytes());
        table.extend_from_slice(&entry);
    }
    table
}

/// Wraps section contents in an ELF header and a section header table.
///
/// `sections` holds each section's name offset in `.shstrtab`, its type, the offset of
/// its contents in `body`, and its length.
#[cfg(test)]
fn write_image(body: &[u8], sections: &[(u32, u32, usize, usize)]) -> Vec<u8> {
    let mut out = vec![0u8; 64];
    out[..4].copy_from_slice(&MAGIC);
    out[4] = CLASS_64;
    out[5] = DATA_LSB;
    out[6] = 1;
    out[0x10..0x12].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
    out[0x28..0x30].copy_from_slice(&((64 + body.len()) as u64).to_le_bytes());
    out[0x3A..0x3C].copy_from_slice(&(SHDR_BYTES as u16).to_le_bytes());
    out[0x3C..0x3E].copy_from_slice(&(sections.len() as u16 + 1).to_le_bytes());
    out[0x3E..0x40].copy_from_slice(&1u16.to_le_bytes()); // .shstrtab is section 1
    out.extend_from_slice(body);
    out.extend_from_slice(&[0u8; SHDR_BYTES]); // the null section
    for (name, kind, offset, size) in sections {
        out.extend_from_slice(&section_header(*name, *kind, *offset, *size));
    }
    out
}

/// One `Elf64_Shdr`, with `sh_link` pointing at `.dynstr`.
#[cfg(test)]
fn section_header(name: u32, kind: u32, offset: usize, size: usize) -> [u8; SHDR_BYTES] {
    let mut header = [0u8; SHDR_BYTES];
    header[0..4].copy_from_slice(&name.to_le_bytes());
    header[4..8].copy_from_slice(&kind.to_le_bytes());
    header[0x18..0x20].copy_from_slice(&(offset as u64).to_le_bytes());
    header[0x20..0x28].copy_from_slice(&(size as u64).to_le_bytes());
    header[0x28..0x2C].copy_from_slice(&2u32.to_le_bytes()); // sh_link -> .dynstr
    header[0x38..0x40].copy_from_slice(&(SYM_BYTES as u64).to_le_bytes());
    header
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exports_finds_a_defined_symbol() {
        let image = synthetic_image(&[("other_symbol", true), ("wanted", true)]);
        assert!(exports(&image, "wanted").expect("a well-formed image"));
    }

    #[test]
    fn test_exports_ignores_a_symbol_that_is_only_imported() {
        // The name is in `.dynstr` either way; only a defined section makes it an
        // export, and an imported name is exactly what a name-only scan would accept.
        let image = synthetic_image(&[("wanted", false)]);
        assert!(!exports(&image, "wanted").expect("a well-formed image"));
    }

    #[test]
    fn test_exports_ignores_a_symbol_that_is_not_there() {
        let image = synthetic_image(&[("something_else", true)]);
        assert!(!exports(&image, "wanted").expect("a well-formed image"));
    }

    #[test]
    fn test_exports_refuses_an_image_without_a_dynamic_symbol_table() {
        let image = synthetic_image(&[]);
        let stripped = &image[..64];
        assert!(exports(stripped, "wanted").is_err());
    }

    #[test]
    fn test_exports_refuses_a_truncated_or_foreign_file() {
        assert!(exports(b"not an elf at all", "wanted").is_err());
        let mut wrong_class = synthetic_image(&[("wanted", true)]);
        wrong_class[4] = 1; // 32-bit
        let failure = exports(&wrong_class, "wanted").expect_err("a 32-bit image");
        assert!(failure.to_string().contains("64-bit"), "{failure}");

        let mut wrong_endianness = synthetic_image(&[("wanted", true)]);
        wrong_endianness[5] = 2;
        assert!(exports(&wrong_endianness, "wanted").is_err());
    }

    #[test]
    fn test_exports_path_reports_the_file_it_could_not_read() {
        let missing = std::env::temp_dir().join("rspinyin-elf-does-not-exist.so");
        let failure = exports_path(&missing, "wanted").expect_err("a missing file");
        assert!(failure.to_string().contains("rspinyin-elf"), "{failure}");
    }
}
