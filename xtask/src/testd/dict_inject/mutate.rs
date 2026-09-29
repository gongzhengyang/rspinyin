//! The byte-level work: what each mutation does to an image, and what a store's damage is
//! made of.
//!
//! Responsibility: turn one [`DictMutation`] and a pristine container into the image the
//! fixture writes, report the byte range a mutation names, and produce the byte pattern a
//! malformed user store is built from.
//!
//! Boundaries: this module works on bytes and nothing else. It opens no file, keeps no path
//! and decides no policy -- which change is inert, which container a mutation applies to,
//! and where a refusal has to be caught are the parent module's questions. Everything here
//! reports a [`FixtureError`] rather than indexing out of range, and every offset it
//! computes is a checked one: a fixture that overflowed would panic in the harness instead
//! of producing the malformed file it claims to.
//!
//! # Two ways of producing an image, and why both are here
//!
//! Most mutations patch bytes in place, and each of them writes exactly one field: the byte
//! diff between the image it produces and the pristine one lies inside the field the
//! variant names, plus the checksum fields the container's own arithmetic forces it to
//! repair. [`named_span`] is that claim as a value, so a case can check it against the diff
//! instead of taking this comment's word for it.
//!
//! A record's checksums are recomputed after it is patched, so that the record is the only
//! thing left wrong -- a file whose checksum no longer matched would be refused for the
//! checksum, and the record would never be read. The mutations that break a *checksum
//! field's* subject are the exception and say so: they are the ones that answer "would a
//! corrupted payload be caught", and repairing what they broke would delete the case.
//!
//! Two mutations cannot be patched at all. An FST packs its keys into shared states, so a
//! single key's value is not addressable on its own; the word-list mutation therefore
//! rebuilds the index with the real `fst` writer. A string pool is a byte run with no
//! internal structure, so cutting it is a re-encode rather than a patch. Both re-encode the
//! whole container with the real [`DictWriter`], which is also what leaves every checksum
//! correct -- the point of both being that the *only* thing wrong with the file is the one
//! thing they changed.

use std::ops::Range;

use fst::Streamer;
use ime_dict::format::writer::DictWriter;
use ime_dict::format::{
    self, DictEntry, ENTRY_SIZE, HEADER_SIZE, MAX_WORD_LEN, SECTION_COUNT, SECTION_ENTRY_SIZE,
    SectionEntry, SectionKind, crc32, pack_fst_value, parse_section_table,
};
use ime_types::DictError;

use super::FixtureError;
use super::mutation::DictMutation;

/// Width of one `WORDLIST` element: a `word_id` is a little-endian `u32`.
const WORD_ID_SIZE: usize = 4;

/// Width of the container's magic.
const MAGIC_LEN: usize = 4;

/// File offset of the header's `format_version` field.
const FORMAT_VERSION_OFFSET: usize = 4;

/// File offset of the header's `total_len` field.
const TOTAL_LEN_OFFSET: usize = 16;

/// File offset of the header's `file_crc32` field.
pub(super) const FILE_CRC_OFFSET: usize = 24;

/// File offset of the header's `entry_count` field.
const ENTRY_COUNT_OFFSET: usize = 28;

/// Offset of a section table row's `crc32` field.
pub(super) const SECTION_CRC_FIELD: usize = 4;

/// Offset of a section table row's `offset` field.
const SECTION_FIELD_OFFSET: usize = 8;

/// Offset of a section table row's `len` field.
const SECTION_FIELD_LEN: usize = 16;

/// Offset of `word_off` inside an encoded entry record.
const ENTRY_FIELD_WORD_OFF: usize = 0;

/// Offset of `word_len` inside an encoded entry record.
const ENTRY_FIELD_WORD_LEN: usize = 4;

/// Width of a section table row's `offset` and `len` fields.
const SECTION_NUMBER_WIDTH: usize = 8;

/// Bytes a malformed user store is made of.
pub(super) const GARBAGE_BYTES: usize = 4096;

/// The state the byte generator starts from.
///
/// A constant rather than a random source: a case compares the quarantine file against the
/// bytes it placed, so the same bytes have to come out on every run, and a real random
/// source would make the case assert nothing the second time.
const GARBAGE_SEED: u64 = 0x2545_f491_4f6c_dd1d;

/// Returns `len` bytes that are not a store.
///
/// The pattern is a xorshift sequence: arbitrary-looking, reproducible, and never all zero,
/// which matters because an all-zero file is the one shape a key-value store might mistake
/// for a fresh one.
pub(super) fn garbage(len: usize) -> Vec<u8> {
    let mut state = GARBAGE_SEED;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push(state as u8);
    }
    out
}

/// Applies `mutation` to a copy of `pristine` and returns the image to write.
///
/// # Errors
/// Returns [`FixtureError::Inapplicable`] when the container lacks the section, the entry or
/// the key the mutation names, [`FixtureError::InertMutation`] when the value the mutation
/// writes is already in contract, and [`FixtureError::ImageUnreadable`] when the image
/// cannot be read where the mutation needs to read it.
pub(super) fn apply(mutation: &DictMutation, pristine: &[u8]) -> Result<Vec<u8>, FixtureError> {
    let mut image = pristine.to_vec();
    match mutation {
        DictMutation::Magic => {
            // One byte of the four is enough for the magic to stop matching.
            put(&mut image, 0, b"X")?;
        }
        DictMutation::FormatVersion { version } => {
            if *version == format::FORMAT_VERSION {
                return Err(inert(mutation));
            }
            put(&mut image, FORMAT_VERSION_OFFSET, &version.to_le_bytes())?;
        }
        DictMutation::Truncate { at } => {
            if *at >= image.len() {
                return Err(inert(mutation));
            }
            image.truncate(*at);
        }
        DictMutation::SectionCrc { kind } => flip_section_byte(&mut image, *kind, mutation.name())?,
        DictMutation::EntryLength { index, len } => {
            if *len != 0 && *len <= MAX_WORD_LEN {
                return Err(inert(mutation));
            }
            let at = entry_field(&image, *index, ENTRY_FIELD_WORD_LEN, mutation.name())?;
            put(&mut image, at, &len.to_le_bytes())?;
            recompute_checksums(&mut image)?;
        }
        DictMutation::EntryOffset { index, off } => {
            // The mutation is defined as an offset at or past the end of the pool. An offset
            // *inside* the pool can still break a record, but only together with a length,
            // which is the other mutation's job.
            if u64::from(*off) < pool_len(pristine)? {
                return Err(inert(mutation));
            }
            let at = entry_field(&image, *index, ENTRY_FIELD_WORD_OFF, mutation.name())?;
            put(&mut image, at, &off.to_le_bytes())?;
            recompute_checksums(&mut image)?;
        }
        DictMutation::WordlistRange { key } => return rewrite_key_range(pristine, key),
        DictMutation::EmptyFile => image.clear(),
        DictMutation::DeclaredLength { len } => {
            if *len == image.len() as u64 {
                return Err(inert(mutation));
            }
            put(&mut image, TOTAL_LEN_OFFSET, &len.to_le_bytes())?;
        }
        DictMutation::SectionLength { kind, len } => {
            let at = section_field(&image, *kind, SECTION_FIELD_LEN, mutation.name())?;
            if read_field(&image, at, SECTION_NUMBER_WIDTH, "section_len")? == *len {
                return Err(inert(mutation));
            }
            put(&mut image, at, &len.to_le_bytes())?;
        }
        DictMutation::SectionOffset { kind, offset } => {
            let at = section_field(&image, *kind, SECTION_FIELD_OFFSET, mutation.name())?;
            if read_field(&image, at, SECTION_NUMBER_WIDTH, "section_offset")? == *offset {
                return Err(inert(mutation));
            }
            put(&mut image, at, &offset.to_le_bytes())?;
        }
        DictMutation::EntryCount { count } => {
            if read_field(&image, ENTRY_COUNT_OFFSET, 4, "entry_count")? == u64::from(*count) {
                return Err(inert(mutation));
            }
            put(&mut image, ENTRY_COUNT_OFFSET, &count.to_le_bytes())?;
        }
        DictMutation::StrPoolTruncate { keep } => {
            let pool = section_bytes(pristine, SectionKind::StrPool)?;
            // A cut that leaves every record inside the pool changes the file without
            // breaking it, which is the one outcome this fixture must never report as a
            // case.
            if *keep >= pool.len() || *keep as u64 >= max_record_end(pristine)? {
                return Err(inert(mutation));
            }
            return encode_with(pristine, SectionKind::StrPool, pool[..*keep].to_vec());
        }
        DictMutation::Append { bytes } => {
            if *bytes == 0 {
                return Err(inert(mutation));
            }
            let len = image
                .len()
                .checked_add(*bytes)
                .ok_or_else(|| unfittable("append_len", *bytes as u64))?;
            image.resize(len, 0);
        }
    }
    Ok(image)
}

/// The byte range `mutation` names inside `image`.
///
/// Only the mutations that patch bytes in place name a range, and the range is the field
/// the variant is named after. A case diffs the image against the pristine one and checks
/// that every differing byte lies inside this range or inside one of the container's
/// checksum fields: a mutation that wrote somewhere else would be a fixture whose refusal
/// came from a fault it never declared, and the case would report a pass for the wrong
/// reason.
///
/// The mutations that rebuild the container, and the ones that change its length, name no
/// range and answer `None`: there is no single field to compare a byte diff against.
///
/// # Errors
/// Returns [`FixtureError::Inapplicable`] when the section or the record the mutation names
/// is absent, [`FixtureError::ImageUnreadable`] when the table cannot be read, and
/// [`FixtureError::Unfittable`] when an offset does not fit a `usize`.
pub(super) fn named_span(
    mutation: &DictMutation,
    image: &[u8],
) -> Result<Option<Range<usize>>, FixtureError> {
    let span = match mutation {
        DictMutation::Magic => field_span(0, MAGIC_LEN, "magic")?,
        DictMutation::FormatVersion { .. } => {
            field_span(FORMAT_VERSION_OFFSET, 2, "format_version")?
        }
        DictMutation::DeclaredLength { .. } => field_span(TOTAL_LEN_OFFSET, 8, "total_len")?,
        DictMutation::EntryCount { .. } => field_span(ENTRY_COUNT_OFFSET, 4, "entry_count")?,
        DictMutation::SectionOffset { kind, .. } => {
            let at = section_field(image, *kind, SECTION_FIELD_OFFSET, mutation.name())?;
            field_span(at, SECTION_NUMBER_WIDTH, "section_offset")?
        }
        DictMutation::SectionLength { kind, .. } => {
            let at = section_field(image, *kind, SECTION_FIELD_LEN, mutation.name())?;
            field_span(at, SECTION_NUMBER_WIDTH, "section_len")?
        }
        DictMutation::EntryLength { index, .. } => {
            let at = entry_field(image, *index, ENTRY_FIELD_WORD_LEN, mutation.name())?;
            field_span(at, 2, "word_len")?
        }
        DictMutation::EntryOffset { index, .. } => {
            let at = entry_field(image, *index, ENTRY_FIELD_WORD_OFF, mutation.name())?;
            field_span(at, 4, "word_off")?
        }
        DictMutation::SectionCrc { kind } => {
            let at = section_last_byte(image, *kind, mutation.name())?;
            field_span(at, 1, "section_byte")?
        }
        DictMutation::Truncate { .. }
        | DictMutation::EmptyFile
        | DictMutation::Append { .. }
        | DictMutation::WordlistRange { .. }
        | DictMutation::StrPoolTruncate { .. } => return Ok(None),
    };
    Ok(Some(span))
}

/// The range `at..at + width`.
///
/// # Errors
/// Returns [`FixtureError::Unfittable`] when the end does not fit a `usize`.
fn field_span(at: usize, width: usize, field: &'static str) -> Result<Range<usize>, FixtureError> {
    let Some(end) = at.checked_add(width) else {
        return Err(unfittable(field, at as u64));
    };
    Ok(at..end)
}

/// The refusal a change that would leave the container valid is reported with.
fn inert(mutation: &DictMutation) -> FixtureError {
    FixtureError::InertMutation {
        mutation: mutation.name(),
    }
}

/// The refusal a mutation the container cannot carry is reported with.
fn inapplicable(mutation: &'static str, reason: String) -> FixtureError {
    FixtureError::Inapplicable { mutation, reason }
}

/// The refusal a value that does not fit the host's address space is reported with.
fn unfittable(field: &'static str, value: u64) -> FixtureError {
    FixtureError::Unfittable { field, value }
}

/// The refusal a range that leaves the image is reported with.
fn out_of_image(at: usize, end: usize, len: usize) -> FixtureError {
    FixtureError::OutOfImage { at, end, len }
}

/// The refusal an image the reader cannot parse is reported with.
fn image_unreadable(cause: DictError) -> FixtureError {
    FixtureError::ImageUnreadable { cause }
}

/// Writes `bytes` into `image` at `at`.
///
/// # Errors
/// Returns [`FixtureError::Unfittable`] when the range does not fit a `usize`, and
/// [`FixtureError::OutOfImage`] when it leaves the image.
fn put(image: &mut [u8], at: usize, bytes: &[u8]) -> Result<(), FixtureError> {
    let end = at
        .checked_add(bytes.len())
        .ok_or_else(|| unfittable("image_offset", at as u64))?;
    let len = image.len();
    let slot = image
        .get_mut(at..end)
        .ok_or_else(|| out_of_image(at, end, len))?;
    slot.copy_from_slice(bytes);
    Ok(())
}

/// Reads the little-endian integer of `width` bytes at `at`.
///
/// # Errors
/// Returns [`FixtureError::Unfittable`] when the range does not fit a `usize`, and
/// [`FixtureError::OutOfImage`] when it leaves the image.
fn read_field(
    image: &[u8],
    at: usize,
    width: usize,
    field: &'static str,
) -> Result<u64, FixtureError> {
    let Some(end) = at.checked_add(width) else {
        return Err(unfittable(field, at as u64));
    };
    let Some(bytes) = image.get(at..end) else {
        return Err(out_of_image(at, end, image.len()));
    };
    // Widened one byte at a time rather than through a fixed-width array: the width is a
    // parameter, and the loop is exact for every width up to eight.
    let mut value = 0u64;
    for byte in bytes.iter().rev() {
        value = (value << 8) | u64::from(*byte);
    }
    Ok(value)
}

/// Flips the last byte of the named section, leaving its recorded checksum alone.
///
/// # Errors
/// Returns [`FixtureError::Inapplicable`] when the section is absent,
/// [`FixtureError::ImageUnreadable`] when the table cannot be read, and
/// [`FixtureError::OutOfImage`] when the section's last byte leaves the image.
fn flip_section_byte(
    image: &mut [u8],
    kind: SectionKind,
    mutation: &'static str,
) -> Result<(), FixtureError> {
    let at = section_last_byte(image, kind, mutation)?;
    let len = image.len();
    let slot = image
        .get_mut(at)
        .ok_or_else(|| out_of_image(at, at.saturating_add(1), len))?;
    *slot ^= 0xff;
    Ok(())
}

/// The offset of the last byte of the named section.
///
/// # Errors
/// Returns [`FixtureError::Inapplicable`] when the section is absent,
/// [`FixtureError::ImageUnreadable`] when the table cannot be read, and
/// [`FixtureError::Unfittable`] when the offset does not fit a `usize`.
fn section_last_byte(
    image: &[u8],
    kind: SectionKind,
    mutation: &'static str,
) -> Result<usize, FixtureError> {
    let section = section_entry(image, kind)?;
    if section.is_absent() {
        return Err(inapplicable(
            mutation,
            format!("the container has no {} section", kind.name()),
        ));
    }
    let last = section
        .offset
        .checked_add(section.len)
        .and_then(|end| end.checked_sub(1))
        .ok_or_else(|| unfittable("section_len", section.len))?;
    usize::try_from(last).map_err(|_| unfittable("section_offset", last))
}

/// The absolute offset of `field` inside the section-table row for `kind`.
///
/// # Errors
/// Returns [`FixtureError::Inapplicable`] when the container has no such section,
/// [`FixtureError::ImageUnreadable`] when the table cannot be read, and
/// [`FixtureError::Unfittable`] when the offset does not fit a `usize`.
fn section_field(
    image: &[u8],
    kind: SectionKind,
    field: usize,
    mutation: &'static str,
) -> Result<usize, FixtureError> {
    if section_entry(image, kind)?.is_absent() {
        return Err(inapplicable(
            mutation,
            format!("the container has no {} section", kind.name()),
        ));
    }
    let index = kind.as_raw() as usize - 1;
    HEADER_SIZE
        .checked_add(index.saturating_mul(SECTION_ENTRY_SIZE))
        .and_then(|at| at.checked_add(field))
        .ok_or_else(|| unfittable("section_field", index as u64))
}

/// The largest `word_off + word_len` any record of `image` names.
///
/// A pool cut is inert exactly when it is at or past this end: a container whose records all
/// fit a shortened pool is still usable, so a case built on it would prove nothing about the
/// pool.
///
/// # Errors
/// Returns [`FixtureError::ImageUnreadable`] when the container has no `ENTRIES` section or
/// a record of it does not decode, and [`FixtureError::OutOfImage`] when the section's bytes
/// leave the image.
fn max_record_end(image: &[u8]) -> Result<u64, FixtureError> {
    let entries = section_bytes(image, SectionKind::Entries)?;
    let mut end = 0u64;
    // `chunks_exact` walks whole records only; the reader has already established that the
    // section is exactly `entry_count` records long, so nothing is left over.
    for record in entries.chunks_exact(ENTRY_SIZE) {
        let entry = DictEntry::decode(record).map_err(image_unreadable)?;
        end = end.max(u64::from(entry.word_off) + u64::from(entry.word_len));
    }
    Ok(end)
}

/// The absolute offset of `field` inside the entry record at `index`.
///
/// # Errors
/// Returns [`FixtureError::Inapplicable`] when the container has no `ENTRIES` section or no
/// record at `index`, and [`FixtureError::Unfittable`] when the offset does not fit a
/// `usize`.
fn entry_field(
    image: &[u8],
    index: u32,
    field: usize,
    mutation: &'static str,
) -> Result<usize, FixtureError> {
    let section = section_entry(image, SectionKind::Entries)?;
    if section.is_absent() {
        return Err(inapplicable(
            mutation,
            String::from("the container has no ENTRIES section"),
        ));
    }
    let count = section.len / ENTRY_SIZE as u64;
    if u64::from(index) >= count {
        return Err(inapplicable(
            mutation,
            format!("the container has {count} entries, so there is no entry {index}"),
        ));
    }
    let within = u64::from(index)
        .checked_mul(ENTRY_SIZE as u64)
        .and_then(|offset| offset.checked_add(field as u64))
        .ok_or_else(|| unfittable("entry_offset", u64::from(index)))?;
    let at = section
        .offset
        .checked_add(within)
        .ok_or_else(|| unfittable("entry_offset", within))?;
    usize::try_from(at).map_err(|_| unfittable("entry_offset", at))
}

/// The byte length of the container's string pool.
///
/// # Errors
/// Returns [`FixtureError::ImageUnreadable`] when the table cannot be read.
fn pool_len(image: &[u8]) -> Result<u64, FixtureError> {
    Ok(section_bytes(image, SectionKind::StrPool)?.len() as u64)
}

/// The number of `word_id` pairs in the container's word list.
///
/// # Errors
/// Returns [`FixtureError::ImageUnreadable`] when the table cannot be read.
fn word_list_len(image: &[u8]) -> Result<u64, FixtureError> {
    Ok(section_bytes(image, SectionKind::WordList)?.len() as u64 / WORD_ID_SIZE as u64)
}

/// The section-table entry for `kind`.
///
/// # Errors
/// Returns [`FixtureError::ImageUnreadable`] when the table cannot be read.
fn section_entry(image: &[u8], kind: SectionKind) -> Result<SectionEntry, FixtureError> {
    let sections = parse_section_table(image).map_err(image_unreadable)?;
    sections
        .get(kind.as_raw() as usize - 1)
        .copied()
        .ok_or_else(|| {
            image_unreadable(DictError::LengthOutOfRange {
                field: "section_kind",
                value: u64::from(kind.as_raw()),
            })
        })
}

/// The bytes of `kind`, or an empty slice when the section is absent.
///
/// # Errors
/// Returns [`FixtureError::ImageUnreadable`] when the table cannot be read and
/// [`FixtureError::OutOfImage`] when the recorded range leaves the image.
fn section_bytes(image: &[u8], kind: SectionKind) -> Result<&[u8], FixtureError> {
    let section = section_entry(image, kind)?;
    if section.is_absent() {
        return Ok(&[]);
    }
    let start = usize::try_from(section.offset)
        .map_err(|_| unfittable("section_offset", section.offset))?;
    let len = usize::try_from(section.len).map_err(|_| unfittable("section_len", section.len))?;
    let end = start
        .checked_add(len)
        .ok_or_else(|| unfittable("section_len", section.len))?;
    image
        .get(start..end)
        .ok_or_else(|| out_of_image(start, end, image.len()))
}

/// Recomputes every present section's checksum and the file checksum of `image`.
///
/// The table is read first and written afterwards: a section's bytes and the table row that
/// describes them are different parts of one buffer, and a single borrow cannot hold both.
///
/// # Errors
/// Returns [`FixtureError::ImageUnreadable`] when the table cannot be read, and the write
/// errors of [`put`] when a computed offset leaves the image.
fn recompute_checksums(image: &mut [u8]) -> Result<(), FixtureError> {
    let sections = parse_section_table(image).map_err(image_unreadable)?;
    let mut rows = Vec::with_capacity(SECTION_COUNT);
    for (index, section) in sections.iter().enumerate() {
        if section.is_absent() {
            continue;
        }
        let bytes = section_bytes(image, section.kind)?;
        let at = HEADER_SIZE
            .checked_add(index.saturating_mul(SECTION_ENTRY_SIZE))
            .and_then(|at| at.checked_add(SECTION_CRC_FIELD))
            .ok_or_else(|| unfittable("section_crc_offset", index as u64))?;
        rows.push((at, crc32(bytes)));
    }
    for (at, crc) in rows {
        put(image, at, &crc.to_le_bytes())?;
    }
    let body = image
        .get(HEADER_SIZE..)
        .ok_or_else(|| out_of_image(HEADER_SIZE, image.len(), image.len()))?;
    let file_crc = crc32(body);
    put(image, FILE_CRC_OFFSET, &file_crc.to_le_bytes())
}

/// Rebuilds the container with `key`'s packed value pointing past the end of the word list.
///
/// # Errors
/// Returns [`FixtureError::Inapplicable`] when the index holds no such key and
/// [`FixtureError::ImageUnreadable`] when the index or the container cannot be rebuilt.
fn rewrite_key_range(image: &[u8], key: &str) -> Result<Vec<u8>, FixtureError> {
    let map = fst::Map::new(section_bytes(image, SectionKind::Fst)?)
        .map_err(|error| image_unreadable(DictError::Fst(error.to_string())))?;
    if map.get(key.as_bytes()).is_none() {
        return Err(inapplicable(
            DictMutation::WordlistRange {
                key: key.to_owned(),
            }
            .name(),
            format!("the index holds no key {key:?}"),
        ));
    }
    // One element past the end: the lookup has to refuse the range rather than read it.
    let start = word_list_len(image)?.saturating_add(1);
    let value = pack_fst_value(start, 1).map_err(image_unreadable)?;
    let rebuilt = rebuild_index(&map, key, value)?;
    encode_with(image, SectionKind::Fst, rebuilt)
}

/// Copies `map` into a fresh index with `key`'s value replaced by `value`.
///
/// The stream yields the keys in the lexicographic order an FST builder requires, so the
/// rebuilt index holds every key of the original.
///
/// # Errors
/// Returns [`FixtureError::ImageUnreadable`] when the builder refuses a key or cannot
/// finish the index.
fn rebuild_index(map: &fst::Map<&[u8]>, key: &str, value: u64) -> Result<Vec<u8>, FixtureError> {
    let mut builder = fst::MapBuilder::memory();
    let mut stream = map.stream();
    while let Some((found, existing)) = stream.next() {
        let packed = if found == key.as_bytes() {
            value
        } else {
            existing
        };
        builder
            .insert(found, packed)
            .map_err(|error| image_unreadable(DictError::Fst(error.to_string())))?;
    }
    builder
        .into_inner()
        .map_err(|error| image_unreadable(DictError::Fst(error.to_string())))
}

/// Re-encodes the container with one section's payload replaced.
///
/// The real writer is used rather than a hand-rolled layout, so the result is a container
/// the reader accepts for exactly the reason the original was: every offset, alignment and
/// checksum is the writer's own.
///
/// # Errors
/// Returns [`FixtureError::ImageUnreadable`] when a section cannot be read or the writer
/// refuses a payload.
fn encode_with(
    image: &[u8],
    kind: SectionKind,
    mut payload: Vec<u8>,
) -> Result<Vec<u8>, FixtureError> {
    let mut writer = DictWriter::new();
    for section in SectionKind::ALL {
        // The replacement is moved out on the one pass over the section it belongs to; the
        // loop visits each kind once, so the empty vector it leaves behind is never read.
        let bytes = if section == kind {
            std::mem::take(&mut payload)
        } else {
            section_bytes(image, section)?.to_vec()
        };
        writer
            .add_section(section, bytes)
            .map_err(image_unreadable)?;
    }
    writer.encode().map_err(image_unreadable)
}
