//! Tests for the compilation manifest.
//!
//! Every fixture is written to a scratch directory of its own under the system temp
//! directory and removed afterwards, so nothing here depends on the fetched sources
//! under `data/raw/` or on the state another test left behind.

use std::fs;
use std::path::PathBuf;

use ime_dict::format::FORMAT_VERSION;
use serde_json::Value;

use crate::dictc::budget::SegmentBudgets;

use super::*;

/// The TSV fixture the measurement test writes and digests.
const FIXTURE: &str = "中国\t100\n# comment\n银行\t50\n";

/// The bytes the build tests write as a stand-in container.
const CONTAINER: &[u8] = b"RSPD-not-really-a-container";

/// A scratch directory unique to this test process and tag.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rspinyin-manifest-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

/// One source identity over the file at `path`.
fn identity(id: &str, path: PathBuf) -> SourceIdentity {
    SourceIdentity {
        id: id.to_owned(),
        kind: "derived".to_owned(),
        layer: "L2".to_owned(),
        spdx: "MIT OR Apache-2.0".to_owned(),
        path,
    }
}

/// A manifest for a container holding `entries` entries.
fn manifest_with(entries: u64) -> Manifest {
    Manifest {
        schema: MANIFEST_SCHEMA,
        format_version: FORMAT_VERSION,
        sources: Vec::new(),
        expansion: ExpansionRecord {
            band_weight: 2_856,
            band_rank: 50_000,
            cap: 4,
            band_words: 1_000,
            expanded_words: 250,
            added_keys: 300,
        },
        sections: Vec::new(),
        dictionary: DictionaryRecord {
            path: "data/compiled/base.dict".to_owned(),
            bytes: 248_484,
            sha256: "0".repeat(64),
            entries,
            keys: 6_000,
        },
    }
}

/// A ledger holding the two sections the tests register.
fn ledger_with_entries(entries: u64) -> Ledger {
    let mut ledger = Ledger::new(SegmentBudgets::from_container_ceiling(20 * 1024 * 1024));
    ledger
        .record(SectionKind::Fst, None, 1_000)
        .expect("recording the fst");
    ledger
        .record(SectionKind::Entries, Some(entries), 16 * entries)
        .expect("recording the entries");
    ledger
}

/// The build facts the tests record, over the sources they name.
fn facts(sources: Vec<SourceIdentity>) -> BuildFacts {
    BuildFacts {
        sources,
        expansion: ExpansionRecord {
            band_weight: 2_856,
            band_rank: 50_000,
            cap: 4,
            band_words: 1_000,
            expanded_words: 250,
            added_keys: 300,
        },
        keys: 6_000,
    }
}

#[test]
fn test_manifest_path_appends_the_suffix_beside_the_container() {
    let path = manifest_path(Path::new("/srv/build/data/compiled/base.dict"));
    assert_eq!(
        path,
        PathBuf::from("/srv/build/data/compiled/base.dict.manifest.json")
    );
    assert_eq!(
        manifest_path(Path::new("base.dict")),
        PathBuf::from("base.dict.manifest.json")
    );
    assert_eq!(
        manifest_path(Path::new("/")),
        PathBuf::from("/.manifest.json"),
        "a path with no file name of its own still gets a manifest name"
    );
}

#[test]
fn test_measure_records_size_line_count_and_digest() {
    let dir = scratch("measure");
    let path = dir.join("base.tsv");
    fs::write(&path, FIXTURE).expect("writing the fixture");

    let record = SourceRecord::measure(identity("base", path.clone())).expect("measuring");
    assert_eq!(record.id, "base");
    assert_eq!(record.kind, "derived");
    assert_eq!(record.layer, "L2");
    assert_eq!(record.spdx, "MIT OR Apache-2.0");
    assert_eq!(
        record.bytes, 31,
        "the size is in bytes, not characters: two Han characters weigh six bytes each"
    );
    assert_eq!(record.lines, 3, "comments and blanks count as lines");
    assert_eq!(record.sha256.len(), 64);
    assert!(
        record.sha256.chars().all(|digit| digit.is_ascii_hexdigit()),
        "the digest is hexadecimal: {}",
        record.sha256
    );
    assert_eq!(
        record.sha256,
        hex(&Sha256::digest(FIXTURE.as_bytes())),
        "the digest is over the bytes on disk"
    );

    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_measure_reports_a_source_that_was_never_fetched() {
    let dir = scratch("unfetched");
    let failure = SourceRecord::measure(identity("jieba-dict", dir.join("jieba-dict.tsv")))
        .expect_err("a missing source must not be silently dropped");
    assert!(
        failure.to_string().contains("jieba-dict.tsv"),
        "the failure names the file: {failure}"
    );
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_scale_classifies_the_entry_count() {
    assert_eq!(manifest_with(350_000).scale(), Scale::Product);
    assert_eq!(
        manifest_with(PRODUCT_ENTRY_FLOOR).scale(),
        Scale::Product,
        "the floor itself is product scale"
    );
    assert_eq!(
        manifest_with(PRODUCT_ENTRY_CEILING).scale(),
        Scale::Product,
        "the ceiling itself is product scale"
    );
    assert_eq!(
        manifest_with(PRODUCT_ENTRY_FLOOR - 1).scale(),
        Scale::Development { shortfall: 1 }
    );
    assert_eq!(
        manifest_with(5_441).scale(),
        Scale::Development { shortfall: 314_559 },
        "the committed development word list is far under the floor"
    );
    assert_eq!(
        manifest_with(PRODUCT_ENTRY_CEILING + 1).scale(),
        Scale::Oversized { excess: 1 }
    );
    assert_eq!(
        manifest_with(0).scale(),
        Scale::Development {
            shortfall: PRODUCT_ENTRY_FLOOR
        },
        "an empty dictionary is the degenerate development case"
    );
}

#[test]
fn test_render_is_a_stable_document_with_the_declared_schema() {
    let manifest = manifest_with(350_000);
    let first = manifest.render().expect("rendering");
    let second = manifest.render().expect("rendering again");
    assert_eq!(first, second, "rendering is a pure function of the record");
    assert!(first.ends_with('\n'), "the document is a text file");

    let parsed: Value = serde_json::from_str(&first).expect("the document is valid JSON");
    assert_eq!(parsed["schema"], MANIFEST_SCHEMA);
    assert_eq!(parsed["format_version"], FORMAT_VERSION);
    assert_eq!(parsed["dictionary"]["entries"], 350_000_u64);
    assert_eq!(parsed["dictionary"]["path"], "data/compiled/base.dict");
    assert_eq!(parsed["expansion"]["cap"], 4_u64);
    assert_eq!(manifest.entries(), 350_000);
}

#[test]
fn test_render_carries_the_section_names_the_container_uses() {
    let mut manifest = manifest_with(1_000);
    manifest.sections = vec![
        SectionUsage {
            kind: SectionKind::Fst,
            entries: None,
            bytes: 2_310_144,
        },
        SectionUsage {
            kind: SectionKind::StrPool,
            entries: None,
            bytes: 3_142_016,
        },
    ];
    let parsed: Value =
        serde_json::from_str(&manifest.render().expect("rendering")).expect("valid JSON");
    let sections = parsed["sections"].as_array().expect("an array");
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0]["kind"], "FST");
    assert_eq!(sections[0]["entries"], Value::Null);
    assert_eq!(sections[1]["kind"], "STRPOOL");
    assert_eq!(sections[1]["bytes"], 3_142_016_u64);
}

#[test]
fn test_write_returns_the_digest_of_the_bytes_it_wrote() {
    let dir = scratch("write");
    let path = dir.join("base.dict.manifest.json");
    let manifest = manifest_with(350_000);
    let digest = manifest.write(&path).expect("writing");

    let written = fs::read_to_string(&path).expect("reading back");
    assert_eq!(written, manifest.render().expect("rendering"));
    assert_eq!(
        digest,
        hex(&Sha256::digest(written.as_bytes())),
        "the returned digest is over the bytes on disk"
    );
    assert_eq!(digest.len(), 64);
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_write_reports_a_directory_that_is_not_there() {
    let dir = scratch("nowhere");
    let failure = manifest_with(1).write(&dir.join("missing").join("m.json"));
    let failure = failure.expect_err("writing into a missing directory must fail");
    assert!(failure.to_string().contains("m.json"), "{failure}");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_measured_sources_are_ordered_by_allowlist_id() {
    let dir = scratch("sources");
    let raw = dir.join("data").join("raw");
    fs::create_dir_all(&raw).expect("creating the raw directory");
    for id in ["polyphone", "base", "jieba-dict"] {
        fs::write(raw.join(format!("{id}.tsv")), format!("{id}\n")).expect("writing the fixture");
    }

    let records = measured_sources(vec![
        identity("polyphone", raw.join("polyphone.tsv")),
        identity("base", raw.join("base.tsv")),
        identity("jieba-dict", raw.join("jieba-dict.tsv")),
    ])
    .expect("measuring");
    let ids: Vec<&str> = records.iter().map(|record| record.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["base", "jieba-dict", "polyphone"],
        "the order is a function of the ids, not of the caller's list"
    );

    let failure = measured_sources(vec![identity("unihan", raw.join("unihan.tsv"))])
        .expect_err("a registered source that is absent must fail the record");
    assert!(failure.to_string().contains("unihan.tsv"), "{failure}");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_ordered_sections_follow_the_container_layout() {
    let mut ledger = Ledger::new(SegmentBudgets::from_container_ceiling(20 * 1024 * 1024));
    // Recorded in the table's order, which is not the container's.
    ledger
        .record(SectionKind::Unigram, Some(2), 30)
        .expect("unigram");
    ledger
        .record(SectionKind::StrPool, None, 10)
        .expect("strpool");
    ledger.record(SectionKind::Fst, None, 40).expect("fst");
    ledger
        .record(SectionKind::WordList, Some(1), 20)
        .expect("wordlist");

    let kinds: Vec<SectionKind> = ordered_sections(&ledger)
        .iter()
        .map(|row| row.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            SectionKind::Fst,
            SectionKind::StrPool,
            SectionKind::Unigram,
            SectionKind::WordList,
        ]
    );
}

#[test]
fn test_record_build_writes_the_manifest_beside_the_container() {
    let dir = scratch("build");
    let raw = dir.join("data").join("raw");
    let compiled = dir.join("data").join("compiled");
    fs::create_dir_all(&raw).expect("creating the raw directory");
    fs::create_dir_all(&compiled).expect("creating the compiled directory");
    let source = raw.join("base.tsv");
    fs::write(&source, "中国\t100\n").expect("writing the source");
    let container = compiled.join("base.dict");
    fs::write(&container, CONTAINER).expect("writing the container");

    let ledger = ledger_with_entries(1_234);
    let manifest = record_build(
        &dir,
        &container,
        &ledger,
        facts(vec![identity("base", source)]),
    )
    .expect("recording the build");

    assert_eq!(manifest.schema, MANIFEST_SCHEMA);
    assert_eq!(manifest.sources.len(), 1);
    assert_eq!(manifest.sources[0].id, "base");
    assert_eq!(manifest.dictionary.entries, 1_234);
    assert_eq!(manifest.dictionary.keys, 6_000);
    assert_eq!(manifest.dictionary.bytes, CONTAINER.len() as u64);
    assert_eq!(
        manifest.dictionary.path, "data/compiled/base.dict",
        "the path is relative to the repository root"
    );
    assert_eq!(
        manifest
            .sections
            .iter()
            .map(|row| row.kind)
            .collect::<Vec<SectionKind>>(),
        vec![SectionKind::Fst, SectionKind::Entries],
        "the sections follow the container's layout"
    );
    assert_eq!(manifest.scale(), Scale::Development { shortfall: 318_766 });

    let path = manifest_path(&container);
    assert!(path.is_file(), "the manifest landed beside the container");
    let written = fs::read_to_string(&path).expect("reading the manifest back");
    assert!(written.contains("\"base\""), "{written}");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_record_build_reports_a_container_that_is_not_there() {
    let dir = scratch("nobuild");
    let raw = dir.join("data").join("raw");
    fs::create_dir_all(&raw).expect("creating the raw directory");
    let source = raw.join("base.tsv");
    fs::write(&source, "中国\t100\n").expect("writing the source");

    let ledger = ledger_with_entries(10);
    let failure = record_build(
        &dir,
        &dir.join("data").join("compiled").join("base.dict"),
        &ledger,
        facts(vec![identity("base", source)]),
    )
    .expect_err("a container that was never written cannot be recorded");
    assert!(failure.to_string().contains("base.dict"), "{failure}");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_hex_renders_lower_case_and_pads_each_byte() {
    assert_eq!(hex(b""), "");
    assert_eq!(hex(b"\x00"), "00");
    assert_eq!(hex(b"\x0f\xa0\xff"), "0fa0ff");
}
