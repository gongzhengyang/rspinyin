//! Tests for the packager.
//!
//! Every test works in a scratch tree and drives one function at a time. Nothing here
//! runs `strip` or `tar`: a synthetic ELF image is not something a real `strip` is obliged
//! to leave intact, and a test that depended on binutils being installed would fail on the
//! machines least able to explain why. What is asserted here is the resolution, the
//! staging, the measuring and the document; the end-to-end path -- a real build, a real
//! strip, a real archive -- is what `just package` does in CI.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::install::layout::Sources;
use crate::install::{Budget, FACTORY_SYMBOL, SizeBudgets, elf};

use super::manifest::write_checksums;
use super::*;

/// The Crockford base32 alphabet, written out independently of the one under test.
const ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The SHA-256 of the three bytes `abc`, the standard test vector.
const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

/// A scratch directory unique to this test process and tag.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rspinyin-package-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

/// Writes one file into the scratch tree, creating its parent directories.
fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("a file has a parent")).expect("creating the parent");
    fs::write(&path, content).expect("writing the fixture");
}

/// A scratch tree holding every artifact the payload table requires.
fn fixture(tag: &str) -> PathBuf {
    let root = scratch(tag);
    write(&root, "target/release/librspinyin.so", "library bytes");
    write(&root, "target/release/librspinyin_ui.so", "library bytes");
    write(&root, "packaging/fcitx5/rspinyin.conf", "addon descriptor");
    write(
        &root,
        "packaging/fcitx5/rspinyin-ui.conf",
        "addon descriptor",
    );
    write(&root, "packaging/fcitx5/rspinyin-im.conf", "input method");
    write(&root, "data/compiled/base.dict", "dictionary");
    write(&root, "assets/icon-48.png", "png bytes");
    write(&root, "assets/icon.svg", "svg bytes");
    write(&root, "docs/dev/NOTICE", "notice");
    write(
        &root,
        "LICENSES/LicenseRef-Slint-Royalty-free-2.0.md",
        "licence text",
    );
    write(
        &root,
        "packaging/metainfo/org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml",
        "metainfo",
    );
    root
}

/// One planned payload, with everything the staging and the measuring read.
fn planned(name: &'static str, path: PathBuf, budget: Option<Budget>) -> Planned {
    Planned {
        name,
        role: "test",
        source: path.clone(),
        path,
        budget,
        required_exports: &[],
        exports: Vec::new(),
    }
}

/// The payload with `name`, which the table is expected to carry.
fn payload(name: &str) -> &'static Payload {
    PAYLOADS
        .iter()
        .find(|payload| payload.name == name)
        .expect("the payload table carries this file")
}

#[test]
fn test_payload_table_ships_the_libraries_the_size_gate_measures() {
    let libraries: Vec<&Payload> = PAYLOADS
        .iter()
        .filter(|payload| payload.budget == Some(Budget::StrippedLibrary))
        .collect();

    assert_eq!(libraries.len(), ADDON_LIBRARIES.len());
    for (entry, name) in libraries.iter().zip(ADDON_LIBRARIES) {
        assert_eq!(entry.name, name, "the two are written in the same order");
        assert_eq!(entry.exports.to_vec(), vec![FACTORY_SYMBOL]);
        assert!(!entry.optional, "a missing library is not a release");
        let built = format!("target/release/{name}");
        assert_eq!(entry.candidates.to_vec(), vec![built.as_str()]);
    }
}

#[test]
fn test_payload_table_marks_the_attribution_notice_as_mandatory() {
    // A release that ships Slint without the notice violates the licence the renderer is
    // used under, so a missing notice is an error rather than a line in the report.
    assert!(!payload("NOTICE").optional);
    assert_eq!(
        payload("NOTICE").candidates.to_vec(),
        vec!["docs/dev/NOTICE"]
    );
}

#[test]
fn test_payload_table_budgets_the_dictionary() {
    assert_eq!(
        payload(DICTIONARY_FILE).budget,
        Some(Budget::BaseDictionary)
    );
    assert_eq!(
        payload(DICTIONARY_FILE).candidates,
        crate::install::DICTIONARY
    );
}

#[test]
fn test_payload_table_ships_the_icons_a_release_must_carry() {
    // A release is unpacked on a machine that has no checkout, so a file the archive does
    // not carry is one the user can never install: `assets/` reaching the tarball is the
    // whole reason the icons are payloads. Both are committed to the repository rather
    // than produced by the build, which is why neither is optional -- an archive that
    // quietly left one out would install a plugin Fcitx5 shows with a placeholder.
    for (name, candidate) in [
        ("fcitx-rspinyin.png", "assets/icon-48.png"),
        ("fcitx-rspinyin.svg", "assets/icon.svg"),
    ] {
        let icon = payload(name);
        assert_eq!(icon.role, "icon", "{name}");
        assert_eq!(icon.candidates.to_vec(), vec![candidate], "{name}");
        assert!(
            !icon.optional,
            "{name} is committed, so a release carries it"
        );
        assert_eq!(icon.budget, None, "{name} carries no size budget");
    }
}

#[test]
fn test_plan_release_resolves_every_required_payload_and_reports_the_optional_ones() {
    let root = fixture("plan");
    let tree = root.join("target/package/tree");
    let plan = plan_release(&Sources::new(root.clone()), &tree).expect("every required payload");

    let names: Vec<&str> = plan.payloads.iter().map(|entry| entry.name).collect();
    let expected = vec![
        "librspinyin.so",
        "librspinyin_ui.so",
        "rspinyin.conf",
        "rspinyin-ui.conf",
        "rspinyin-im.conf",
        "base.dict",
        "fcitx-rspinyin.png",
        "fcitx-rspinyin.svg",
        "NOTICE",
        "LICENSES/LicenseRef-Slint-Royalty-free-2.0.md",
        "org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml",
    ];
    assert_eq!(
        names, expected,
        "the icons, the licence the fixture writes and the metainfo ship with the release; \
         only the two licence texts the fixture leaves unwritten are absent"
    );

    let absent: Vec<&str> = plan.absent.iter().map(|(name, _)| *name).collect();
    let missing = vec!["LICENSE-APACHE", "LICENSE-MIT"];
    assert_eq!(
        absent, missing,
        "a missing payload is reported, never skipped"
    );
    for (_, reason) in &plan.absent {
        assert!(
            reason.contains("none of the build outputs exists"),
            "{reason}"
        );
    }
    for entry in &plan.payloads {
        assert!(
            entry.path.starts_with(&tree),
            "{} is staged in the tree",
            entry.name
        );
    }
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_plan_release_refuses_a_missing_required_payload() {
    let root = fixture("plan-required");
    fs::remove_file(root.join("data/compiled/base.dict")).expect("removing the fixture");
    let tree = root.join("target/package/tree");
    let failure = plan_release(&Sources::new(root.clone()), &tree);
    let failure = failure.expect_err("a release without a dictionary is not a release");

    assert!(failure.to_string().contains("base.dict"), "{failure}");
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_plan_artifacts_records_the_thresholds_the_budget_document_states() {
    let root = fixture("budgets");
    let sources = Sources::new(root.clone());
    let plan = plan_release(&sources, &root.join("target/package/tree")).expect("the fixture");
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = repo.parent().expect("xtask lives in the repository");
    let budgets = SizeBudgets::load(&Sources::new(repo.to_path_buf())).expect("the document");

    let artifacts = plan.artifacts(&budgets);
    let library = |name: &str| {
        artifacts
            .iter()
            .find(|artifact| artifact.name == name)
            .expect("the payload is planned")
            .budget_mb
    };
    assert_eq!(library("librspinyin.so"), Some(budgets.library_mb));
    assert_eq!(library("librspinyin_ui.so"), Some(budgets.library_mb));
    assert_eq!(library(DICTIONARY_FILE), Some(budgets.dictionary_mb));
    assert_eq!(
        library("rspinyin.conf"),
        None,
        "a payload with no budget is recorded as unbudgeted rather than as zero"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_collect_stages_a_payload_that_is_not_a_library_verbatim() {
    let root = fixture("collect");
    let tree = root.join("target/package/tree");
    let source = root.join("docs/dev/NOTICE");
    let mut plan = Plan {
        payloads: vec![planned("NOTICE", tree.join("NOTICE"), None)],
        absent: Vec::new(),
    };
    plan.payloads[0].source = source.clone();

    collect(&mut plan, StripPolicy::Require).expect("a copy needs no external tool");
    let staged = fs::read_to_string(tree.join("NOTICE")).expect("reading the staged file");
    assert_eq!(
        staged,
        fs::read_to_string(&source).expect("reading the source")
    );
    assert!(
        plan.payloads[0].exports.is_empty(),
        "a copy exports nothing"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_verified_exports_reads_the_symbol_back_out_of_the_staged_library() {
    let root = scratch("exports");
    let library = root.join("librspinyin.so");
    let image = elf::synthetic_image(&[(FACTORY_SYMBOL, true), ("other", true)]);
    fs::write(&library, image).expect("writing the fixture");

    let exported = verified_exports(&library, &[FACTORY_SYMBOL]).expect("the symbol is there");
    assert_eq!(exported, vec![String::from(FACTORY_SYMBOL)]);
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_verified_exports_refuses_a_library_that_lost_the_symbol() {
    let root = scratch("exports-missing");
    let library = root.join("librspinyin.so");
    // What `strip --strip-all` leaves behind: the name is in the string table, and
    // nothing defines it.
    fs::write(&library, elf::synthetic_image(&[(FACTORY_SYMBOL, false)])).expect("the fixture");

    let failure = verified_exports(&library, &[FACTORY_SYMBOL]);
    let failure = failure.expect_err("a library that exports nothing is not shippable");
    let message = failure.to_string();
    assert!(
        message.contains("dist/verify/factory-symbol-missing"),
        "{message}"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_measure_fails_a_payload_past_its_budget_with_the_delivery_code() {
    let root = scratch("measure");
    let library = root.join("librspinyin.so");
    fs::write(&library, vec![0u8; 2 * 1024 * 1024]).expect("writing the fixture");
    let budget = Some(Budget::StrippedLibrary);
    let plan = Plan {
        payloads: vec![planned("librspinyin.so", library, budget)],
        absent: Vec::new(),
    };

    let strict = SizeBudgets {
        library_mb: 1.0,
        dictionary_mb: 20.0,
    };
    let failure = measure(&plan, &strict).expect_err("2MiB is past a 1MiB budget");
    let message = failure.to_string();
    assert!(
        message.contains("dist/verify/size-budget-exceeded"),
        "{message}"
    );
    assert!(message.contains("BUDGET-SIZE-01"), "{message}");

    let generous = SizeBudgets {
        library_mb: 12.0,
        dictionary_mb: 20.0,
    };
    measure(&plan, &generous).expect("2MiB is inside a 12MiB budget");
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_measure_ignores_a_payload_that_carries_no_budget() {
    let root = scratch("measure-unbudgeted");
    let descriptor = root.join("rspinyin.conf");
    fs::write(&descriptor, vec![0u8; 2 * 1024 * 1024]).expect("writing the fixture");
    let plan = Plan {
        payloads: vec![planned("rspinyin.conf", descriptor, None)],
        absent: Vec::new(),
    };
    let tiny = SizeBudgets {
        library_mb: 0.001,
        dictionary_mb: 0.001,
    };

    measure(&plan, &tiny).expect("a descriptor is not measured against a library budget");
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_artifact_record_reads_the_digest_and_the_size_from_the_file() {
    let root = scratch("record");
    let file = root.join("base.dict");
    fs::write(&file, "abc").expect("writing the fixture");
    let artifact = Artifact {
        name: String::from("base.dict"),
        role: "dictionary",
        path: file,
        budget_mb: Some(20.0),
        exports: Vec::new(),
    };

    let record = artifact.record().expect("the record is readable");
    assert_eq!(record["name"], serde_json::json!("base.dict"));
    assert_eq!(record["role"], serde_json::json!("dictionary"));
    assert_eq!(record["sha256"], serde_json::json!(ABC_SHA256));
    assert_eq!(record["size_bytes"], serde_json::json!(3));
    assert_eq!(record["size_budget_mb"], serde_json::json!(20.0));
    assert_eq!(record["exports"], serde_json::json!([]));
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_write_checksums_lists_every_file_in_the_format_sha256sum_reads() {
    let root = scratch("checksums");
    fs::write(root.join("base.dict"), "abc").expect("writing the fixture");
    fs::write(root.join("NOTICE"), "notice").expect("writing the fixture");
    let names = vec![String::from("base.dict"), String::from("NOTICE")];

    let path = write_checksums(&root, &names).expect("the list is writable");
    let list = fs::read_to_string(&path).expect("reading the list back");
    let lines: Vec<&str> = list.lines().collect();
    assert_eq!(lines.len(), 2, "one line per file: {list}");
    assert_eq!(lines[0], format!("{ABC_SHA256}  base.dict"));
    let notice = Sha256::digest(b"notice");
    let notice: String = notice.iter().map(|byte| format!("{byte:02x}")).collect();
    assert!(
        lines[1].starts_with(&format!("{notice}  NOTICE")),
        "{}",
        lines[1]
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

/// A release document with one artifact, for the assertions about its shape.
fn release(root: &Path) -> Release {
    Release {
        version: String::from("0.1.0"),
        arch: String::from("x86_64"),
        request_id: String::from("01J8ZQ4K7N3M2P8R5T6V9W0X1Y"),
        built_at: String::from("2026-09-29T00:00:00Z"),
        source_date_epoch: 1_790_899_200,
        commit: None,
        rustc: Some(String::from("1.98.0")),
        channel: Some(String::from("1.98.0")),
        fcitx5_floor: String::from("5.1.7"),
        artifacts: vec![Artifact {
            name: String::from("base.dict"),
            role: "dictionary",
            path: root.join("base.dict"),
            budget_mb: Some(20.0),
            exports: Vec::new(),
        }],
    }
}

#[test]
fn test_release_json_carries_the_documented_fields() {
    let root = scratch("document");
    fs::write(root.join("base.dict"), "abc").expect("writing the fixture");
    let document = release(&root).to_json().expect("the document renders");
    let document: serde_json::Value = serde_json::from_str(&document).expect("it is JSON");

    assert_eq!(document["manifest_version"], serde_json::json!(1));
    assert_eq!(
        document["request_id"],
        serde_json::json!("01J8ZQ4K7N3M2P8R5T6V9W0X1Y")
    );
    assert_eq!(document["release"]["version"], serde_json::json!("0.1.0"));
    assert_eq!(
        document["release"]["built_at"],
        serde_json::json!("2026-09-29T00:00:00Z")
    );
    assert_eq!(
        document["release"]["source_date_epoch"],
        serde_json::json!(1_790_899_200)
    );
    assert_eq!(document["toolchain"]["rustc"], serde_json::json!("1.98.0"));
    assert_eq!(
        document["toolchain"]["channel"],
        serde_json::json!("1.98.0")
    );
    assert_eq!(
        document["toolchain"]["glibc_min"],
        serde_json::json!("2.35")
    );
    assert_eq!(
        document["compatibility"]["fcitx5"],
        serde_json::json!(">=5.1.7")
    );
    assert_eq!(
        document["compatibility"]["architectures"],
        serde_json::json!(["x86_64"])
    );
    assert_eq!(
        document["compatibility"]["session_tiers"][0],
        serde_json::json!("x11")
    );
    assert_eq!(
        document["artifacts"][0]["sha256"],
        serde_json::json!(ABC_SHA256)
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_release_json_records_an_unknown_commit_and_signature_as_null() {
    // A source tarball has no commit to name and an unsigned release has no key id: both
    // are recorded as absent rather than guessed at.
    let root = scratch("document-null");
    fs::write(root.join("base.dict"), "abc").expect("writing the fixture");
    let document = release(&root).to_json().expect("the document renders");
    let document: serde_json::Value = serde_json::from_str(&document).expect("it is JSON");

    assert_eq!(document["release"]["commit"], serde_json::Value::Null);
    assert_eq!(document["signature"], serde_json::Value::Null);
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_release_write_puts_the_manifest_in_the_directory() {
    let root = scratch("document-write");
    fs::write(root.join("base.dict"), "abc").expect("writing the fixture");

    let path = release(&root)
        .write(&root)
        .expect("the manifest is writable");
    assert!(path.ends_with(manifest::MANIFEST_FILE));
    let written = fs::read_to_string(&path).expect("reading the manifest back");
    assert!(written.contains(ABC_SHA256), "{written}");
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_archive_stem_names_the_release_and_its_architecture() {
    assert_eq!(
        stamp::archive_stem("0.1.0", "x86_64"),
        "rspinyin-0.1.0-x86_64"
    );
    assert_eq!(
        stamp::archive_stem("1.2.3", "aarch64"),
        "rspinyin-1.2.3-aarch64"
    );
}

#[test]
fn test_rfc3339_formats_the_epoch_and_a_known_instant() {
    assert_eq!(stamp::rfc3339(0), "1970-01-01T00:00:00Z");
    assert_eq!(stamp::rfc3339(1_000_000_000), "2001-09-09T01:46:40Z");
    // 2000 is a leap year: a century rule read as 1900 would put this a day out.
    assert_eq!(stamp::rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    // A negative epoch is a valid instant and must not wrap.
    assert_eq!(stamp::rfc3339(-1), "1969-12-31T23:59:59Z");
}

#[test]
fn test_ulid_encodes_the_timestamp_in_its_first_ten_characters() {
    let id = stamp::ulid(1_790_899_200_000, [0; 10]);
    assert_eq!(id.len(), 26, "{id}");
    assert!(id.chars().all(|symbol| ALPHABET.contains(symbol)), "{id}");
    assert_eq!(decode(&id[..10]), 1_790_899_200_000);
    assert_eq!(decode(&id[10..]), 0, "no entropy means no random suffix");

    let random = stamp::ulid(1_790_899_200_000, [0xFF; 10]);
    assert_eq!(&random[..10], &id[..10], "the timestamp is not affected");
    assert_ne!(random, id, "the entropy reaches the identifier");
}

#[test]
fn test_ulid_sorts_by_the_millisecond_it_was_made_in() {
    // The property the format exists for: the alphabet is in code-point order, so the
    // lexical order of two identifiers is the order of their timestamps.
    assert!(stamp::ulid(1, [0; 10]) < stamp::ulid(2, [0; 10]));
    let later = stamp::ulid(1_790_899_200_001, [0; 10]);
    assert!(stamp::ulid(1_790_899_200_000, [0xFF; 10]) < later);
}

/// Decodes a Crockford base32 string, independently of the encoder under test.
fn decode(text: &str) -> u128 {
    text.bytes().fold(0u128, |value, byte| {
        let symbol = ALPHABET.bytes().position(|candidate| candidate == byte);
        value * 32 + u128::from(symbol.expect("a Crockford symbol") as u8)
    })
}

#[test]
fn test_fcitx5_floor_reads_the_descriptor() {
    let root = scratch("floor");
    write(
        &root,
        "packaging/fcitx5/rspinyin.conf",
        "[Addon]\nName=Rust Pinyin\n\n[Addon/Dependencies]\n0=core:5.1.7\n\n[Dependencies]\n",
    );
    let floor = stamp::fcitx5_floor(&root).expect("the descriptor declares a floor");
    assert_eq!(floor, "5.1.7");

    write(
        &root,
        "packaging/fcitx5/rspinyin.conf",
        "[Addon]\nName=Rust Pinyin\n",
    );
    assert!(
        stamp::fcitx5_floor(&root).is_err(),
        "a descriptor with no core dependency cannot state a floor"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_toolchain_channel_reads_the_pin() {
    let root = scratch("channel");
    assert_eq!(
        stamp::toolchain_channel(&root),
        None,
        "an unpinned tree has no channel to record"
    );

    write(
        &root,
        "rust-toolchain.toml",
        "[toolchain]\nchannel = \"1.98.0\"\n",
    );
    let channel = stamp::toolchain_channel(&root);
    assert_eq!(channel, Some(String::from("1.98.0")));
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_human_size_reports_mebibytes_and_kibibytes() {
    assert_eq!(human_size(Some(3 * 1024 * 1024)), "3.00MiB");
    assert_eq!(human_size(Some(1536)), "1.5KiB");
    assert_eq!(human_size(None), "unreadable");
}
