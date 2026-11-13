//! The aux-column view the assembly hands the router: a working view for a container
//! that carries the column, none for one that does not.
//!
//! The view hangs off the process's own slots, so these tests lean on the mandated
//! one-process-per-test runner the way the staged-swap tests in `steps` do: each maps
//! one container through `sources_for` and reads the view the assembly would hand the
//! router at the session-host step. The container each test writes is the format
//! writer's own output, so the bytes are what a real compile produces and nothing
//! here needs a committed dictionary or a display server.

use std::collections::BTreeMap;
use std::fs;

use ime_dict::format::writer::DictWriter;
use ime_dict::format::{
    AUX_ROW_SIZE, DictEntry, PROB_Q12_MAX, SectionKind, hash_word, pack_fst_value,
};

use super::super::session::{aux_source, sources_for};
use super::scratch_dir;

/// The unigram table row of one word, at the fixture's own probability.
fn unigram_row(text: &str) -> Vec<u8> {
    let mut row = Vec::new();
    row.extend_from_slice(&hash_word(text).to_le_bytes());
    row.extend_from_slice(&PROB_Q12_MAX.to_le_bytes());
    row.extend_from_slice(&0u16.to_le_bytes());
    row
}

/// Encodes a container image whose two words carry the aux column.
///
/// The sections follow the format's own encoders, and the rows are laid out in the
/// hash order the records are stored in -- each word's code is resolved by text, so
/// the pairing cannot drift with the sort.
fn aux_container() -> Vec<u8> {
    let words = [("ni", "你"), ("hao", "好")];
    let codes = [("你", "nv"), ("好", "hk")];

    let mut hashed: Vec<(u32, &str)> = words
        .iter()
        .map(|(_, text)| (hash_word(text), *text))
        .collect();
    hashed.sort_by_key(|(hash, text)| (*hash, *text));
    let mut strpool = Vec::new();
    let mut entries = Vec::new();
    let mut unigram = Vec::new();
    for text in hashed.iter().map(|(_, text)| *text) {
        let offset = u32::try_from(strpool.len()).expect("small fixture");
        let len = u16::try_from(text.len()).expect("small fixture");
        strpool.extend_from_slice(text.as_bytes());
        let entry = DictEntry::new(offset, len, 1, 0, 1);
        entries.extend_from_slice(&entry.encode());
        unigram.extend_from_slice(&unigram_row(text));
    }
    // One key per word, ascending byte order, each naming the single word it reads.
    let id_of: BTreeMap<&str, u32> = hashed
        .iter()
        .enumerate()
        .map(|(id, (_, text))| (*text, u32::try_from(id).expect("small fixture")))
        .collect();
    let mut keys: Vec<(&str, u32)> = words
        .iter()
        .map(|(key, text)| (*key, id_of[text]))
        .collect();
    keys.sort_by_key(|(key, _)| key.as_bytes().to_vec());
    let mut fst_builder = fst::MapBuilder::memory();
    for (key, id) in keys {
        let packed = pack_fst_value(u64::from(id), 1).expect("packing a word");
        fst_builder.insert(key.as_bytes(), packed).expect("one key");
    }
    let mut wordlist = Vec::new();
    for id in 0..u32::try_from(hashed.len()).expect("small fixture") {
        wordlist.extend_from_slice(&id.to_le_bytes());
    }

    let mut writer = DictWriter::new();
    for (kind, payload) in [
        (
            SectionKind::Fst,
            fst_builder.into_inner().expect("finishing the FST"),
        ),
        (SectionKind::Entries, entries),
        (SectionKind::StrPool, strpool),
        (SectionKind::Unigram, unigram),
        (SectionKind::WordList, wordlist),
    ] {
        writer.add_section(kind, payload).expect("section added");
    }
    let by_text: BTreeMap<&str, &str> = codes.iter().copied().collect();
    let mut rows = Vec::with_capacity(hashed.len() * AUX_ROW_SIZE);
    for (_, text) in &hashed {
        let mut row = [0u8; AUX_ROW_SIZE];
        let code = by_text[text].as_bytes();
        row[..code.len()].copy_from_slice(code);
        rows.extend_from_slice(&row);
    }
    writer.add_aux_column(rows).expect("adding the aux column");
    writer.encode().expect("encoding the container")
}

/// Encodes a one-word container without the column.
fn plain_container() -> Vec<u8> {
    let mut fst_builder = fst::MapBuilder::memory();
    fst_builder
        .insert(
            "ni".as_bytes(),
            pack_fst_value(0, 1).expect("packing a word"),
        )
        .expect("one key");
    let mut writer = DictWriter::new();
    for (kind, payload) in [
        (
            SectionKind::Fst,
            fst_builder.into_inner().expect("finishing the FST"),
        ),
        (
            SectionKind::Entries,
            DictEntry::new(0, 3, 1, 0, 1).encode().to_vec(),
        ),
        (SectionKind::StrPool, "你".as_bytes().to_vec()),
        (SectionKind::Unigram, unigram_row("你")),
        (SectionKind::WordList, 0u32.to_le_bytes().to_vec()),
    ] {
        writer.add_section(kind, payload).expect("section added");
    }
    writer.encode().expect("encoding the container")
}

#[test]
fn test_aux_source_follows_a_container_that_carries_the_column() {
    // The assembly produces a working view exactly when the start mapped a container
    // that carries the column: the flag answers true and a recorded word resolves
    // through the trait object the router will read, borrowed from the mapping.
    let dir = scratch_dir("aux-column");
    let live = dir.join("base.dict");
    fs::write(&live, aux_container()).expect("writing the container");

    sources_for(Some(live.as_path()));

    let source = aux_source().expect("a column-carrying container assembled a view");
    assert!(source.has_aux_column());
    assert_eq!(source.aux_code("你").expect("lookup"), Some("nv"));
    assert_eq!(source.aux_code("好").expect("lookup"), Some("hk"));
}

#[test]
fn test_aux_source_stays_absent_for_a_plain_container() {
    // A container without the column assembles no view at all, which is the
    // degradation the router's once-per-process report names.
    let dir = scratch_dir("aux-absent");
    let live = dir.join("base.dict");
    fs::write(&live, plain_container()).expect("writing the container");

    sources_for(Some(live.as_path()));

    assert!(aux_source().is_none(), "no column, no view");
}
