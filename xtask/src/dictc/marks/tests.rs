use super::*;

/// The scratch directory one list-file test uses, unique per test process.
fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("rspinyin-marks-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

#[test]
fn test_collision_cut_marks_the_spellings_a_pinyin_typist_leaves_mid_syllable() {
    // Every head is a syllable of the real table and every tail an initial,
    // which is the judgment the reduce list is built on.
    assert_eq!(collision_cut("ranch"), Some(3), "ran|ch");
    assert_eq!(collision_cut("bail"), Some(3), "bai|l");
    assert_eq!(collision_cut("push"), Some(2), "pu|sh");
    assert_eq!(collision_cut("sad"), Some(2), "sa|d");
    assert_eq!(collision_cut("but"), Some(2), "bu|t");
    assert_eq!(collision_cut("dash"), Some(2), "da|sh");
    assert_eq!(collision_cut("lunch"), Some(3), "lun|ch");
    assert_eq!(collision_cut("way"), Some(2), "wa|y");
    // The scan takes the first valid cut: `xia` is a syllable, so the cut at
    // three wins over the longer head.
    assert_eq!(collision_cut("xiax"), Some(3), "xia|x");
    // The casing the English keys never carry is folded before the scan.
    assert_eq!(collision_cut("RANCH"), Some(3));
}

#[test]
fn test_collision_cut_rejects_complete_pinyin_and_unpinnable_spellings() {
    // Both halves are syllables: the input is fully typed, not half-typed.
    assert_eq!(collision_cut("panda"), None, "pan|da has no initial tail");
    assert_eq!(collision_cut("mama"), None);
    assert_eq!(
        collision_cut("sushi"),
        None,
        "shi is a syllable, not an initial"
    );
    // No cut names a syllable head at all.
    assert_eq!(collision_cut("hello"), None, "he|llo leaves no initial tail");
    assert_eq!(collision_cut("github"), None);
    assert_eq!(collision_cut("wtf"), None);
    assert_eq!(collision_cut("the"), None);
    // A longer word whose tail runs past any initial.
    assert_eq!(collision_cut("butter"), None, "bu|tter: tter is no initial");
    assert_eq!(collision_cut("saddle"), None);
    // Degenerate inputs.
    assert_eq!(collision_cut(""), None);
    assert_eq!(collision_cut("café"), None, "non-ASCII keys cannot collide");
}

#[test]
fn test_collision_cut_requires_a_two_letter_syllable_head() {
    // The single-vowel heads would drag the one- and two-letter words in; the
    // rule starts at a two-letter syllable, so these stay out.
    assert_eq!(collision_cut("an"), None, "no cut: the range is empty");
    assert_eq!(collision_cut("at"), None);
    assert_eq!(collision_cut("as"), None);
    assert_eq!(
        collision_cut("ai"),
        None,
        "ai is a syllable, and the tail would be empty"
    );
    // The two-letter head accepted: `ai` plus an initial tail collides.
    assert_eq!(collision_cut("aih"), Some(2), "ai|h");
}

#[test]
fn test_load_reduce_list_reads_words_and_skips_comments_and_blanks() {
    let dir = scratch("list");
    let path = dir.join("reduce-eng.tsv");
    std::fs::write(
        &path,
        "# a comment line\n\nsad\tsa+d\nbut\nPUSH\tthe second column is not read\n",
    )
    .expect("writing the list");
    let words = load_reduce_list(&path).expect("loading");
    assert_eq!(words.len(), 3, "one entry per distinct row, case-folded");
    assert!(words.contains("sad") && words.contains("but") && words.contains("push"));
    std::fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_load_reduce_list_rejects_a_malformed_or_repeated_row() {
    let dir = scratch("list-bad");
    let path = dir.join("reduce-eng.tsv");
    std::fs::write(&path, "sad\n3d\n").expect("writing the list");
    let error = load_reduce_list(&path).expect_err("a non-alphabetic row fails");
    assert!(
        error.to_string().contains("2"),
        "the error names the offending line: {error}"
    );

    std::fs::write(&path, "sad\nsad\n").expect("rewriting the list");
    let error = load_reduce_list(&path).expect_err("a repeated row fails");
    assert!(
        error.to_string().contains("twice"),
        "the error names the repetition: {error}"
    );
    std::fs::remove_dir_all(&dir).expect("cleaning up");
}

/// Builds one key's candidate list from `(weight, id)` rows.
fn key_map(rows: &[&[(u32, u32)]]) -> BTreeMap<String, Vec<(u32, u32)>> {
    rows.iter()
        .enumerate()
        .map(|(key, rows)| (format!("key{key}"), rows.to_vec()))
        .collect()
}

#[test]
fn test_apply_long_word_gradient_promotes_longer_candidates_below_the_anchor() {
    // One key: a two-character head of 5000, two more two-character
    // candidates, then a three-character and a four-character candidate the
    // pre-mark order buries at the tail, and a two-character tail the mark
    // must not touch.
    let rows: Vec<(u32, u32)> = vec![(5_000, 0), (300, 1), (40, 2), (5, 3), (3, 4), (2, 5)];
    let mut keys = key_map([&rows[..]]);
    let char_counts = [2, 2, 2, 3, 4, 2];
    let promoted = apply_long_word_gradient(&mut keys, &char_counts);
    assert_eq!(promoted, 2, "only the two longer-than-head pairs move");
    let ranked = &keys["key0"];
    assert_eq!(ranked[0], (5_000, 0), "the head keeps its weight");
    assert_eq!(ranked[1], (300, 1), "a shorter candidate keeps its weight");
    assert_eq!(ranked[2], (40, 2), "the anchor keeps its weight");
    // The four-character candidate rides factor 4, the three-character one
    // factor 2; both rank below the anchor and above the untouched tail,
    // which is the insert-at-position-4 shape -- and the gradient, the longer
    // word ahead despite the lighter corpus weight.
    assert_eq!(ranked[3], (12, 4), "four characters: 3 x 4");
    assert_eq!(ranked[4], (10, 3), "three characters: 5 x 2");
    assert_eq!(ranked[5], (2, 5), "no longer than the head: untouched");
}

#[test]
fn test_apply_long_word_gradient_pulls_a_long_word_back_behind_the_anchor() {
    // A three-character candidate already ahead of the anchor is relocated
    // behind it, the way the benchmark's filter re-inserts at the position
    // rather than only lifting.
    let mut keys = key_map([&[(1_000, 0), (900, 1), (500, 2), (10, 3)]]);
    let char_counts = [2, 3, 2, 2];
    let promoted = apply_long_word_gradient(&mut keys, &char_counts);
    assert_eq!(promoted, 1);
    let ranked = &keys["key0"];
    assert_eq!(ranked[0], (1_000, 0));
    assert_eq!(ranked[1], (500, 2), "the anchor's neighborhood keeps its order");
    assert_eq!(ranked[2], (499, 1), "capped just below the anchor weight");
    assert_eq!(ranked[3], (10, 3));
}

#[test]
fn test_apply_long_word_gradient_appends_when_the_key_has_no_anchor() {
    // Three candidates: the anchor degrades to the last of them, so the
    // promotion appends, which is the same insertion at the end.
    let mut keys = key_map([&[(1_000, 0), (300, 1), (100, 2)]]);
    let char_counts = [2, 2, 3];
    let promoted = apply_long_word_gradient(&mut keys, &char_counts);
    assert_eq!(promoted, 1);
    let ranked = &keys["key0"];
    assert_eq!(ranked[0], (1_000, 0));
    assert_eq!(ranked[1], (300, 1));
    assert_eq!(ranked[2], (99, 2), "appended below the weakest candidate");
}

#[test]
fn test_apply_long_word_gradient_is_a_function_of_the_pre_mark_weights() {
    // The mark runs once per compilation over a map the compiler assembles
    // from the word list, so two runs over fresh maps have to agree -- that
    // is the reproducibility a repeated compilation exercises.
    let rows: Vec<(u32, u32)> = vec![(5_000, 0), (300, 1), (40, 2), (3, 3), (2, 4)];
    let char_counts = [2, 2, 2, 3, 2];
    let mut first = key_map([&rows[..]]);
    let mut second = key_map([&rows[..]]);
    let a = apply_long_word_gradient(&mut first, &char_counts);
    let b = apply_long_word_gradient(&mut second, &char_counts);
    assert_eq!(a, b);
    assert_eq!(first, second, "two runs over one input agree");
    assert_eq!(first["key0"][3], (6, 3), "three characters: 3 x 2");
}

#[test]
fn test_apply_long_word_gradient_skips_an_id_outside_the_counts() {
    // A defensive fallback, never produced by the compiler: an id past the
    // counts reads no length, and a pair with no length is never promoted.
    let mut keys = key_map([&[(5_000, 9), (300, 7)]]);
    let char_counts = [2];
    let promoted = apply_long_word_gradient(&mut keys, &char_counts);
    assert_eq!(promoted, 0);
    assert_eq!(keys["key0"][0], (5_000, 9));
    assert_eq!(keys["key0"][1], (300, 7));
}
