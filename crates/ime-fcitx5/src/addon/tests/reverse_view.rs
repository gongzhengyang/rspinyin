//! The reverse-lookup view the assembly hands the router: a working view for a container
//! that carries the index, none for one that does not.
//!
//! The view hangs off the process's own slots, so these tests lean on the mandated
//! one-process-per-test runner the way the aux-column tests in `aux_view` do: each maps
//! one container through `sources_for` and reads the view the assembly would hand the
//! router at the session-host step. The container each test writes is the format
//! writer's own output, so the bytes are what a real compile produces and nothing
//! here needs a committed dictionary or a display server.

use std::collections::BTreeMap;
use std::fs;

use ime_core::reverse::ReverseHit;
use ime_dict::format::writer::DictWriter;
use ime_dict::format::{DictEntry, PROB_Q12_MAX, SectionKind, hash_word, pack_fst_value};
use ime_dict::reverse::{IndexEntry, build_index};

use super::super::session::{reverse_source, sources_for};
use super::scratch_dir;

/// The unigram table row of one word, at the fixture's own probability.
fn unigram_row(text: &str) -> Vec<u8> {
    let mut row = Vec::new();
    row.extend_from_slice(&hash_word(text).to_le_bytes());
    row.extend_from_slice(&PROB_Q12_MAX.to_le_bytes());
    row.extend_from_slice(&0u16.to_le_bytes());
    row
}

/// The decomposition entries the fixture compiles: three characters whose codes
/// share the letter `h`, so one query answers all of them in codepoint order.
fn decomposition_entries() -> Vec<IndexEntry<'static>> {
    vec![
        IndexEntry {
            character: '中',
            code: "zh",
        },
        IndexEntry {
            character: '房',
            code: "hf",
        },
        IndexEntry {
            character: '放',
            code: "fh",
        },
    ]
}

/// Encodes a container image holding `words`, one key per reading, with no tail
/// payload yet.
///
/// The sections follow the format's own encoders, and the rows are laid out in the
/// hash order the records are stored in -- each word's code is resolved by text, so
/// the pairing cannot drift with the sort.
fn word_container(words: &[(&str, &str)]) -> DictWriter {
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
    writer
}

/// Encodes a container image whose tail carries the reverse index.
fn reverse_container() -> Vec<u8> {
    let mut writer = word_container(&[("ni", "你"), ("hao", "好")]);
    writer
        .add_reverse_index(
            build_index(&decomposition_entries())
                .expect("building the index")
                .bytes,
        )
        .expect("adding the reverse index");
    writer.encode().expect("encoding the container")
}

/// Encodes a one-word container without the index.
fn plain_container() -> Vec<u8> {
    word_container(&[("ni", "你")])
        .encode()
        .expect("encoding the container")
}

#[test]
fn test_reverse_source_follows_a_container_that_carries_the_index() {
    // The assembly produces a working view exactly when the start mapped a container
    // that carries the index: the flag answers true and a query resolves through the
    // trait object the router will read, borrowed from the mapping.
    let dir = scratch_dir("reverse-index");
    let live = dir.join("base.dict");
    fs::write(&live, reverse_container()).expect("writing the container");

    sources_for(Some(live.as_path()));

    let source = reverse_source().expect("an index-carrying container assembled a view");
    assert!(source.has_reverse_index());
    // An exact code answers its own character, with the full code as the annotation.
    assert_eq!(
        source.reverse_lookup("hf").expect("lookup"),
        vec![ReverseHit {
            character: '房',
            code: "hf"
        }]
    );
    // A shared letter answers every carrier in container order, which is
    // ascending codepoint order here.
    let hits = source.reverse_lookup("h").expect("lookup");
    let characters: Vec<char> = hits.iter().map(|hit| hit.character).collect();
    assert_eq!(characters, ['中', '房', '放']);
}

#[test]
fn test_reverse_source_stays_absent_for_a_plain_container() {
    // A container without the index assembles no view at all, which is the
    // degradation the router's once-per-process report names.
    let dir = scratch_dir("reverse-absent");
    let live = dir.join("base.dict");
    fs::write(&live, plain_container()).expect("writing the container");

    sources_for(Some(live.as_path()));

    assert!(reverse_source().is_none(), "no index, no view");
}
