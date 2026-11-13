//! Tests for the aux column pass: table parsing, the `ENTRIES` alignment, and
//! the per-mille budget ceiling.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use ime_dict::format::{DictEntry, ENTRY_SIZE};

use super::{Column, build_column, build_from, check_alignment, enforce_budget, load_table};
use crate::dictc::build::{Band, compile};
use crate::dictc::normalize::NormalizeRules;
use crate::dictc::source::Word;

/// A temporary file that is gone with the returned directory.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rspinyin-dictc-aux-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

/// A word list small enough to compile in microseconds.
fn words() -> Vec<Word> {
    [
        ("中", 500),
        ("中国", 900),
        ("人民", 700),
        ("明天", 300),
        ("的理想", 200),
    ]
    .into_iter()
    .map(|(text, weight)| Word {
        text: text.to_owned(),
        key: "ni".to_owned(),
        readings: Vec::new(),
        weight,
        flags: 0,
        polyphone: false,
    })
    .collect()
}

/// A two-character table covering three of the fixture's first characters.
fn table() -> BTreeMap<char, String> {
    [('中', "kg"), ('人', "rr"), ('的', "prs")]
        .into_iter()
        .map(|(character, code)| (character, code.to_owned()))
        .collect()
}

#[test]
fn test_load_table_parses_comments_blank_lines_and_rows() {
    let dir = scratch("parse");
    let path = dir.join("chaizi.tsv");
    fs::write(&path, "# header comment\n\n明\tbs\n好\tnz\n\r\n赢\twkyb\n")
        .expect("writing the table");
    let table = load_table(&path).expect("loading").expect("a table");
    assert_eq!(table.len(), 3);
    assert_eq!(table[&'明'], "bs");
    assert_eq!(table[&'好'], "nz");
    assert_eq!(table[&'赢'], "wkyb");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_load_table_rejects_malformed_rows() {
    let cases = [
        ("明bs", "no separator"),
        ("明\tbs\textra", "a third column"),
        ("明明\tbs", "two characters"),
        ("A\tbs", "not a Han character"),
        ("明\tBS", "uppercase code"),
        ("明\tb2", "a digit in the code"),
        ("明\t", "an empty code"),
        ("明\tabcde", "a code past four letters"),
    ];
    for (row, why) in cases {
        let dir = scratch("reject");
        let path = dir.join("chaizi.tsv");
        fs::write(&path, row).expect("writing the table");
        let error = load_table(&path).expect_err(why);
        assert!(
            error
                .to_string()
                .contains(&dir.join("chaizi.tsv").display().to_string()),
            "{why}: the error names the file ({error})"
        );
        fs::remove_dir_all(&dir).expect("cleaning up");
    }
}

#[test]
fn test_load_table_rejects_a_repeated_character() {
    let dir = scratch("duplicate");
    let path = dir.join("chaizi.tsv");
    fs::write(&path, "明\tbs\n明\trm").expect("writing the table");
    let error = load_table(&path).expect_err("a second code for one character");
    assert!(error.to_string().contains("second code"), "{error}");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_load_table_reports_a_missing_file_as_none() {
    let dir = scratch("absent");
    let table = load_table(&dir.join("not-there.tsv")).expect("loading");
    assert!(
        table.is_none(),
        "an absent table builds no column, not a failure"
    );
    fs::remove_dir_all(&dir).expect("cleaning up");
}

/// Compiles the fixture words and returns the `ENTRIES` word texts in order.
fn compiled_word_texts() -> Vec<String> {
    let compiled = compile(
        &words(),
        Band {
            threshold: 400,
            requested: 2,
        },
        4,
        &[],
        NormalizeRules::default(),
    )
    .expect("compiling the fixture");
    let pool = &compiled.strpool;
    compiled
        .entries
        .chunks_exact(ENTRY_SIZE)
        .map(|row| {
            let entry = DictEntry::decode(row).expect("a record");
            let start = entry.word_off as usize;
            let end = start + entry.word_len as usize;
            String::from_utf8(pool[start..end].to_vec()).expect("utf-8 word text")
        })
        .collect()
}

#[test]
fn test_build_column_rows_align_with_the_compiled_entries_order() {
    let texts = compiled_word_texts();
    let column = build_column(&words(), &table());
    assert_eq!(
        texts.len(),
        column.rows.len() / 4,
        "one row per compiled entry"
    );
    for (index, text) in texts.iter().enumerate() {
        let row = &column.rows[index * 4..index * 4 + 4];
        let end = row.iter().position(|byte| *byte == 0).unwrap_or(row.len());
        let code = String::from_utf8(row[..end].to_vec()).expect("utf-8 code");
        let expected = table()
            .get(&text.chars().next().expect("a first character"))
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            code, expected,
            "entry {index} ({text}) carries the code of its first character"
        );
    }
}

#[test]
fn test_build_column_leaves_uncovered_first_characters_empty() {
    let column = build_column(&words(), &table());
    // First characters 中 中 人 明 的: the table covers all but 明, whose row
    // stays zero-filled while the coded counter counts the other four.
    let coded = column
        .rows
        .chunks_exact(4)
        .filter(|row| row.iter().any(|byte| *byte != 0))
        .count();
    assert_eq!(column.coded, coded, "the counter matches the rows");
    assert!(
        column.coded < words().len(),
        "uncovered characters stay empty"
    );
}

#[test]
fn test_check_alignment_accepts_a_column_built_for_the_compile() {
    let compiled = compile(
        &words(),
        Band {
            threshold: 400,
            requested: 2,
        },
        4,
        &[],
        NormalizeRules::default(),
    )
    .expect("compiling the fixture");
    let column = build_column(&words(), &table());
    check_alignment(&compiled, Some(&column)).expect("aligned");
    check_alignment(&compiled, None).expect("no column is trivially aligned");
    let short = Column {
        rows: column.rows[..column.rows.len() - 4].to_vec(),
        coded: column.coded,
        table_chars: column.table_chars,
    };
    let error = check_alignment(&compiled, Some(&short)).expect_err("a misaligned column");
    assert!(error.to_string().contains("different word list"), "{error}");
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

    // A missing table builds no column and must not gate-check it: the check
    // runs only once a file is there to compile.
    let column = build_from(&words(), &root, &dir.join("absent.tsv"), &check)
        .expect("a missing table is no error");
    assert!(column.is_none());
    assert_eq!(checked.get(), 0);

    // A present table is gate-checked before it is read.
    fs::write(dir.join("chaizi.tsv"), "中\tkg\n").expect("writing the table");
    let column = build_from(&words(), &root, &dir.join("chaizi.tsv"), &check)
        .expect("a present table compiles");
    assert!(column.is_some());
    assert_eq!(checked.get(), 1);
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_enforce_budget_passes_inside_and_fails_past_the_ceiling() {
    let root = PathBuf::from(std::env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the repository root")
        .to_path_buf();
    // A full-scale column: 350k words at four bytes, inside the share.
    enforce_budget(&root, 350_000 * 4).expect("inside the ceiling");
    let error = enforce_budget(&root, u64::from(u32::MAX)).expect_err("a 4 GiB column");
    assert!(
        error.to_string().contains("per-mille"),
        "the error names the budget discipline ({error})"
    );
}
