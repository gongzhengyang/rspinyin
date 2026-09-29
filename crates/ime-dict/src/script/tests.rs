//! Tests for the dictionary layer's variant table.
//!
//! Every test runs against an index built inside the test: no dictionary file, no clock and
//! no environment is read, so the suite is deterministic by construction and gives the same
//! answer on every machine.

use std::sync::Arc;
use std::thread;

use ime_core::script::{VariantSource, convert, to_simplified, to_traditional};
use ime_types::ui::Script;

use super::*;

#[test]
fn test_script_index_seed_converts_a_committed_word_into_traditional() {
    let index = ScriptIndex::seed();
    // A word-level entry is what disambiguates a character with several traditional forms,
    // and the characters around the word are copied through.
    assert_eq!(to_traditional("去银行取钱", &index), "去銀行取钱");
    assert_eq!(to_simplified("頭髮乾淨", &index), "头发干净");
}

#[test]
fn test_script_index_seed_answers_none_for_a_word_it_does_not_hold() {
    let index = ScriptIndex::seed();
    // A word the table holds nothing for is an answer rather than a failure, and so is the
    // empty word, which no window the rewrite builds ever asks about.
    assert_eq!(index.lookup("火星文", Script::Traditional), None);
    assert_eq!(index.lookup("", Script::Traditional), None);
    assert_eq!(index.lookup("", Script::Simplified), None);
}

#[test]
fn test_script_index_from_pairs_serves_both_directions_of_a_pair() {
    let index = ScriptIndex::from_pairs(&[("干净", "乾淨"), ("银行", "銀行")]);
    assert_eq!(index.lookup("干净", Script::Traditional), Some("乾淨"));
    assert_eq!(index.lookup("銀行", Script::Simplified), Some("银行"));
    assert_eq!(to_traditional("干净", &index), "乾淨");
    assert_eq!(to_simplified("銀行", &index), "银行");
}

#[test]
fn test_script_index_from_pairs_of_no_pairs_answers_nothing() {
    let index = ScriptIndex::from_pairs(&[]);
    assert_eq!(index.lookup("干净", Script::Traditional), None);
    assert_eq!(to_traditional("干净", &index), "干净");
    // An index built from no pairs is the degradation value, however it was reached.
    assert_eq!(index, ScriptIndex::unavailable());
}

#[test]
fn test_script_index_lookup_does_not_answer_the_direction_a_pair_does_not_name() {
    // A pair is indexed by the key each direction reads, so the simplified side answers the
    // traditional direction and nothing else: asking for simplified with a simplified key
    // finds no entry and the text is left alone.
    let index = ScriptIndex::from_pairs(&[("干净", "乾淨")]);
    assert_eq!(index.lookup("干净", Script::Simplified), None);
    assert_eq!(index.lookup("乾淨", Script::Traditional), None);
    assert_eq!(convert("干净", &index, Script::Simplified), "干净");
}

#[test]
fn test_script_index_answers_with_a_pair_whose_sides_are_equal() {
    // A word spelled the same in both scripts is stored as a pair with equal sides: it is
    // the record of a disambiguation decision rather than a gap, so the lookup finds it and
    // the rewrite treats the answer as a match that happens to change nothing.
    let index = ScriptIndex::from_pairs(&[("干涉", "干涉")]);
    assert_eq!(index.lookup("干涉", Script::Traditional), Some("干涉"));
    assert_eq!(to_traditional("干涉", &index), "干涉");
}

#[test]
fn test_script_index_unavailable_leaves_a_commit_unchanged() {
    // What a build without a table does: the feature answers nothing and the text the user
    // is about to send is committed exactly as it was typed.
    let index = ScriptIndex::unavailable();
    let text = "去银行 (ICBC) 取钱 頭髮";
    assert_eq!(to_traditional(text, &index), text);
    assert_eq!(to_simplified(text, &index), text);
    assert_eq!(index.lookup("干净", Script::Traditional), None);
}

#[test]
fn test_script_index_serves_a_caller_that_holds_a_trait_object() {
    // The engine holds one source whatever the table behind it is, so the index has to
    // answer behind a `dyn` pointer as well as by value.
    let index = ScriptIndex::from_pairs(&[("干净", "乾淨")]);
    let source: &dyn VariantSource = &index;
    assert_eq!(convert("干净", source, Script::Traditional), "乾淨");
    // A word the table does not hold is copied through on the same path.
    assert_eq!(convert("干", source, Script::Traditional), "干");
}

#[test]
fn test_script_index_is_shared_across_threads() {
    // The index is shared rather than moved, which is how the decoding thread and the
    // drawing thread both reach it. Naming the `Send + Sync` the trait requires in the
    // handle's type is also what asserts it: a table that could not be shared would not
    // coerce to this handle.
    let index: Arc<dyn VariantSource + Send + Sync> = Arc::new(ScriptIndex::seed());
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let index = Arc::clone(&index);
            thread::spawn(move || to_traditional("去银行", &*index))
        })
        .collect();
    for worker in workers {
        // The joined value asserts both that the thread finished and what it converted.
        assert_eq!(worker.join().ok().as_deref(), Some("去銀行"));
    }
}
