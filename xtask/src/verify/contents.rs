//! What the artifacts contain, beyond their bytes.
//!
//! Responsibility: read each library's dynamic symbol table and confirm it exports what the
//! manifest lists, and read the dictionary as the untrusted container it is. Both checks
//! exist because a file can be byte-for-byte the one that was signed and still be useless:
//! a library stripped with `--strip-all` loads and contains no addon at all, and a dictionary
//! whose checksums disagree with its sections is a file the engine refuses at load time --
//! which, in a process that has already started, is a user with no input method.
//!
//! # Why the two checks are not re-implemented here
//!
//! The symbol reader is the one the installer already uses before it copies a library into
//! place, and the dictionary reader is the one the engine uses at load time. A second
//! implementation of either could disagree with the first, and the disagreement would be
//! found by a user rather than by a test.

use std::path::Path;

use ime_dict::format::reader::{Reader, Verify};

use super::error::{Code, VerifyError};
use super::manifest::{DICTIONARY_ROLE, Manifest};
use crate::install::elf;

/// Reads the contents of every artifact the manifest records contents for.
///
/// # Errors
///
/// Returns `dist/verify/factory-symbol-missing` when a library does not export a symbol the
/// manifest lists, or is not an image whose exports can be read at all, and
/// `dist/verify/dictionary-invalid` when the dictionary is not a container this build can
/// read.
pub fn verify(dir: &Path, manifest: &Manifest) -> Result<(), VerifyError> {
    for artifact in &manifest.artifacts {
        let path = dir.join(&artifact.name);
        check_exports(&path, &artifact.exports)?;
        if artifact.role == DICTIONARY_ROLE {
            check_dictionary(&path)?;
        }
    }
    Ok(())
}

/// Fails when `path` does not export every symbol in `exports`.
///
/// The list is what a consumer can resolve with `dlsym`, so it is checked rather than
/// assumed: a manifest that promised a symbol the file does not carry would make the release
/// look loadable right up to the moment Fcitx5 found no addon in it.
///
/// # Errors
///
/// Returns `dist/verify/factory-symbol-missing` naming the symbol and the file.
fn check_exports(path: &Path, exports: &[String]) -> Result<(), VerifyError> {
    for symbol in exports {
        let exported = elf::exports_path(path, symbol).map_err(|error| {
            VerifyError::rejected(
                Code::FactorySymbolMissing,
                format!("{}: {error}", path.display()),
            )
        })?;
        if !exported {
            return Err(VerifyError::rejected(
                Code::FactorySymbolMissing,
                format!("{} does not export `{symbol}`", path.display()),
            ));
        }
    }
    Ok(())
}

/// Fails when `path` is not a dictionary container this build can read.
///
/// The reader validates the magic, the format version, the section table and every length,
/// offset and checksum before anything is read from the file, which is the same treatment
/// the engine gives a dictionary at load time. Full verification is asked for rather than
/// the header-only mode: a release artifact is read once, so the checksum pass costs a user
/// nothing and is the only thing that catches a section that was damaged after signing.
///
/// # Errors
///
/// Returns `dist/verify/dictionary-invalid` naming the file, with the reader's own
/// diagnostic as the detail.
fn check_dictionary(path: &Path) -> Result<(), VerifyError> {
    Reader::open_with(path, Verify::Full).map_err(|error| {
        VerifyError::rejected(
            Code::DictionaryInvalid,
            format!(
                "{} is not a dictionary this build can read: {error}",
                path.display()
            ),
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::install::elf::synthetic_image;
    use crate::verify::manifest::parse_str;

    /// A scratch directory unique to this test process and tag.
    fn scratch(tag: &str) -> std::path::PathBuf {
        let name = format!("rspinyin-verify-contents-{tag}-{}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        dir
    }

    /// A manifest describing the artifacts a test writes, with `exports` on the first one.
    fn manifest_for(artifacts: &str) -> Manifest {
        let document = format!(
            r#"{{
              "manifest_version": 1,
              "request_id": "01J8ZQ4K7N3M2P8R5T6V9W0X1Y",
              "release": {{
                "version": "0.1.0",
                "commit": null,
                "built_at": "2026-09-29T00:00:00Z",
                "source_date_epoch": 1790899200
              }},
              "toolchain": {{ "rustc": null, "channel": null, "glibc_min": "2.35" }},
              "compatibility": {{
                "fcitx5": ">=5.1.7",
                "architectures": ["x86_64"],
                "session_tiers": ["x11"]
              }},
              "artifacts": [{artifacts}],
              "signature": null
            }}"#
        );
        parse_str(&document).expect("the fixture manifest is well formed")
    }

    /// One artifact record naming `name` with `role` and `exports`.
    fn artifact(name: &str, role: &str, exports: &[&str]) -> String {
        let exports: Vec<String> = exports.iter().map(|s| format!("\"{s}\"")).collect();
        format!(
            r#"{{"name": "{name}", "role": "{role}", "sha256": "{}",
                "size_bytes": 0, "size_budget_mb": null, "exports": [{}]}}"#,
            "a".repeat(64),
            exports.join(", ")
        )
    }

    /// Writes a minimal but real dictionary container into the scratch directory.
    fn dictionary(dir: &Path, name: &str) {
        use fst::MapBuilder;
        use ime_dict::format::SectionKind;
        use ime_dict::format::writer::DictWriter;

        let fst = MapBuilder::new(Vec::new())
            .expect("an empty builder")
            .into_inner()
            .expect("an empty map encodes");
        let mut writer = DictWriter::new();
        writer
            .add_section(SectionKind::Fst, fst)
            .expect("the section is added");
        let image = writer.encode().expect("the container encodes");
        fs::write(dir.join(name), image).expect("writing the dictionary");
    }

    #[test]
    fn test_verify_accepts_a_library_that_exports_what_the_manifest_lists() {
        let dir = scratch("exports");
        let image = synthetic_image(&[("fcitx_addon_factory_instance", true)]);
        fs::write(dir.join("librspinyin.so"), image).expect("writing the library");
        let manifest = manifest_for(&artifact(
            "librspinyin.so",
            "addon-inputmethod",
            &["fcitx_addon_factory_instance"],
        ));

        verify(&dir, &manifest).expect("the symbol is exported");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_verify_refuses_a_library_that_imports_the_symbol_instead_of_exporting_it() {
        // The name is in the string table either way; only a defined section makes it an
        // export, and an over-stripped library is exactly the file that keeps the name and
        // loses the export.
        let dir = scratch("not-exported");
        let image = synthetic_image(&[("fcitx_addon_factory_instance", false)]);
        fs::write(dir.join("librspinyin.so"), image).expect("writing the library");
        let manifest = manifest_for(&artifact(
            "librspinyin.so",
            "addon-inputmethod",
            &["fcitx_addon_factory_instance"],
        ));

        let failure = verify(&dir, &manifest).expect_err("the symbol is not exported");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::FactorySymbolMissing,
                ..
            }
        ));
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_verify_refuses_a_library_whose_exports_cannot_be_read() {
        let dir = scratch("not-an-elf");
        fs::write(dir.join("librspinyin.so"), b"not an ELF image").expect("writing the file");
        let manifest = manifest_for(&artifact(
            "librspinyin.so",
            "addon-inputmethod",
            &["fcitx_addon_factory_instance"],
        ));

        let failure = verify(&dir, &manifest).expect_err("there is no symbol table to read");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::FactorySymbolMissing,
                ..
            }
        ));
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_verify_accepts_a_dictionary_this_build_can_read() {
        let dir = scratch("dictionary");
        dictionary(&dir, "base.dict");
        let manifest = manifest_for(&artifact("base.dict", DICTIONARY_ROLE, &[]));

        verify(&dir, &manifest).expect("the container reads");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_verify_refuses_a_dictionary_that_is_not_a_container() {
        let dir = scratch("not-a-container");
        fs::write(dir.join("base.dict"), b"the word dictionary").expect("writing the file");
        let manifest = manifest_for(&artifact("base.dict", DICTIONARY_ROLE, &[]));

        let failure = verify(&dir, &manifest).expect_err("the magic is wrong");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::DictionaryInvalid,
                ..
            }
        ));
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_verify_leaves_a_file_with_no_recorded_contents_alone() {
        // A descriptor or an icon is not read as a container: the manifest says what a file
        // is for, and only the two roles that have a format are read as one.
        let dir = scratch("opaque");
        fs::write(dir.join("rspinyin.conf"), b"[Addon]\n").expect("writing the descriptor");
        let manifest = manifest_for(&artifact("rspinyin.conf", "addon-descriptor", &[]));

        verify(&dir, &manifest).expect("an opaque payload has nothing to read");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }
}
