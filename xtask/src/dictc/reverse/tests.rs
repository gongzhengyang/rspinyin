//! Tests for the reverse-index pass: block encoding, the header alignment the
//! container expects, and the per-mille budget ceiling.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use super::{Block, build_block, build_from, check_alignment, enforce_budget};

/// A temporary file that is gone with the returned directory.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rspinyin-dictc-reverse-{tag}-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

/// A two-character table whose two codes share exactly the fragment `h`.
///
/// The seven distinct fragments of one code plus the seven of the other, with the
/// shared `h` deduplicated to one, give the hand-added counts the first test pins;
/// one posting per (fragment, character) pair, of which the shared fragment has
/// two, gives the posting count.
fn table() -> BTreeMap<char, String> {
    [('房', "fhg"), ('户', "hdp")]
        .into_iter()
        .map(|(character, code)| (character, code.to_owned()))
        .collect()
}

#[test]
fn test_build_block_encodes_the_table_into_the_index_shape() {
    let block = build_block(&table()).expect("a two-character table");
    assert_eq!(&block.bytes[0..4], b"RVX1", "the block magic leads");
    assert_eq!(block.char_count, 2, "both characters carried");
    // Every non-empty subsequence of each code is a key: seven per code, with the
    // shared `h` deduplicated to one.
    assert_eq!(block.key_count, 13);
    // Twelve fragments name one character, and the shared `h` names both.
    assert_eq!(block.posting_count, 14);
}

#[test]
fn test_build_block_rejects_a_code_the_block_cannot_hold() {
    // The table reader refuses a broken code already; the encoder is the second
    // line, and an error here names the shape rather than encoding it.
    let mut broken = table();
    broken.insert('明', String::from("BS"));
    let error = build_block(&broken).expect_err("an uppercase code");
    assert!(
        error.to_string().contains("reverse_code"),
        "the error names the failing field: {error}"
    );
}

#[test]
fn test_check_alignment_accepts_the_built_block_and_a_missing_one() {
    let block = build_block(&table()).expect("a two-character table");
    check_alignment(Some(&block)).expect("the block the build produced");
    check_alignment(None).expect("no block is trivially aligned");
}

#[test]
fn test_check_alignment_refuses_a_block_that_disagrees_with_itself() {
    let block = build_block(&table()).expect("a two-character table");

    // A payload one byte past what the header describes: the shape the reader
    // refuses on open, caught here at build time.
    let mut padded = block.clone();
    padded.bytes.push(0);
    let error = check_alignment(Some(&padded)).expect_err("a padded block");
    assert!(
        error.to_string().contains("does not fill the shape"),
        "{error}"
    );

    // A header count inflated past the build's own counters: the bytes and the
    // build disagree, which is the defect the counters exist to catch.
    let mut inflated = block.clone();
    inflated.bytes[8..12].copy_from_slice(&99_u32.to_le_bytes());
    let error = check_alignment(Some(&inflated)).expect_err("an inflated count");
    assert!(
        error.to_string().contains("bytes and the build disagree"),
        "{error}"
    );

    // A block that is not a block at all.
    let mut headerless = block;
    headerless.bytes[0] = b'X';
    let error = check_alignment(Some(&headerless)).expect_err("a broken magic");
    assert!(error.to_string().contains("magic"), "{error}");
}

#[test]
fn test_build_from_skips_the_allowlist_check_for_a_missing_table() {
    let root = PathBuf::from(std::env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the repository root")
        .to_path_buf();
    let dir = scratch("build-from");
    let checked = std::cell::Cell::new(0);
    let check = |_: &std::path::Path| {
        checked.set(checked.get() + 1);
        Ok(())
    };

    // A missing table builds no block and must not gate-check it: the check runs
    // only once a file is there to compile.
    let block = build_from(&root, &dir.join("absent.tsv"), &check).expect("no error");
    assert!(block.is_none(), "an absent table builds no block");
    assert_eq!(checked.get(), 0);
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_build_from_gates_and_builds_a_present_table() {
    let root = PathBuf::from(std::env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the repository root")
        .to_path_buf();
    let dir = scratch("build-present");
    let checked = std::cell::Cell::new(0);
    let check = |_: &std::path::Path| {
        checked.set(checked.get() + 1);
        Ok(())
    };

    fs::write(dir.join("chaizi.tsv"), "房\tfhg\n户\thdp\n").expect("writing the table");
    let block = build_from(&root, &dir.join("chaizi.tsv"), &check).expect("a present table");
    let block = block.expect("the table compiled into a block");
    assert_eq!(block.char_count, 2);
    assert_eq!(checked.get(), 1, "a present table is gate-checked first");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_build_from_refuses_a_table_with_a_malformed_row() {
    // A table that is there but wrong stops the build: a wrong code compiled into
    // a shipped container is indistinguishable from a right one to the reader.
    let root = PathBuf::from(std::env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the repository root")
        .to_path_buf();
    let dir = scratch("build-malformed");
    let check = |_: &std::path::Path| Ok(());
    fs::write(dir.join("chaizi.tsv"), "房\tFHG\n").expect("writing the table");
    let error = build_from(&root, &dir.join("chaizi.tsv"), &check).expect_err("an uppercase code");
    assert!(
        error
            .to_string()
            .contains(&dir.join("chaizi.tsv").display().to_string()),
        "the error names the file: {error}"
    );
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_enforce_budget_passes_inside_and_fails_past_the_ceiling() {
    let root = PathBuf::from(std::env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the repository root")
        .to_path_buf();
    // A full-scale block: the planned coverage at its posting density, inside the
    // share with the headroom the constant's comment accounts for.
    enforce_budget(&root, 1_200_000).expect("inside the ceiling");
    let error = enforce_budget(&root, u64::from(u32::MAX)).expect_err("a 4 GiB block");
    assert!(
        error.to_string().contains("per-mille"),
        "the error names the budget discipline ({error})"
    );
}

#[test]
fn test_check_alignment_refuses_counters_the_header_contradicts() {
    // The build's own counters are the third party in the check: bytes whose
    // header describes one thing while the build recorded another cannot pass,
    // even when the bytes alone would fill a consistent shape.
    let block = build_block(&table()).expect("a two-character table");
    let mut lying = Block {
        bytes: block.bytes.clone(),
        char_count: block.char_count + 1,
        key_count: block.key_count,
        posting_count: block.posting_count,
    };
    lying.bytes[8..12].copy_from_slice(&(block.char_count as u32 + 1).to_le_bytes());
    let error = check_alignment(Some(&lying)).expect_err("a counter the header contradicts");
    assert!(
        error
            .to_string()
            .contains("does not fill the shape it announces"),
        "{error}"
    );
}
