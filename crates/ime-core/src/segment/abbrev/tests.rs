//! Unit tests for the abbreviation layer.
//!
//! They live beside the module rather than inside it because the suite is longer
//! than the code it drives; see `abbrev.rs` for the responsibility and the
//! boundaries of what is under test.

use ime_types::{DecodeFlags, SyllableId};

use super::*;
use crate::segment::{MAX_RAW_LEN, SYLLABLES, lookup, syllable_at};

/// The initials pinyin writes, which is what a spelling begins with when it begins
/// with a consonant.
const ONSETS: [&str; 23] = [
    "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x", "zh", "ch", "sh", "r",
    "z", "c", "s", "y", "w",
];

/// The spelling of one unit: a full syllable as the table writes it, an initial as
/// the one letter it is.
fn unit_text(unit: AbbrevSyllable) -> String {
    match unit {
        AbbrevSyllable::Full(id) => syllable_at(id)
            .expect("a full unit names a syllable of the table")
            .to_owned(),
        AbbrevSyllable::Initial(letter) => char::from(letter).to_string(),
    }
}

/// The units of one reading, in order.
fn units(reading: &AbbrevReading) -> Vec<String> {
    reading
        .syllables
        .iter()
        .map(|unit| unit_text(*unit))
        .collect()
}

/// The `(consumed, units)` pairs of a reading list.
///
/// A fingerprint: two lists that answer with the same pairs are the same enumeration,
/// byte for byte, which is what the determinism test compares.
fn shape(found: &[AbbrevReading]) -> Vec<(u16, Vec<String>)> {
    found
        .iter()
        .map(|reading| (reading.consumed, units(reading)))
        .collect()
}

/// Asserts that `found` is exactly the `(consumed, units)` pairs given, in order.
fn assert_shape(found: &[AbbrevReading], expected: &[(u16, Vec<&str>)]) {
    let seen = shape(found);
    let expected: Vec<(u16, Vec<String>)> = expected
        .iter()
        .map(|(consumed, units)| {
            let texts = units.iter().map(|text| (*text).to_owned()).collect();
            (*consumed, texts)
        })
        .collect();
    assert_eq!(seen, expected);
}

/// Builds a reading out of `(spelling, is_initial)` pairs.
///
/// A full unit names a syllable of the table; an initial is one letter, so a
/// one-letter syllable has to be written as a full unit on purpose.
fn reading(units: &[(&str, bool)], consumed: u16) -> AbbrevReading {
    AbbrevReading {
        syllables: units
            .iter()
            .map(|(text, is_initial)| {
                if *is_initial {
                    AbbrevSyllable::Initial(text.as_bytes()[0])
                } else {
                    AbbrevSyllable::Full(lookup(text).expect("a syllable of the table"))
                }
            })
            .collect(),
        consumed,
    }
}

/// Asserts that `reading` covers exactly the bytes of `raw` it claims to.
///
/// The invariant behind every offset: each unit spells the bytes it covers, the
/// marker after a unit is consumed with it, and the reading ends where it says. A
/// unit that crossed a marker, or a reading that ended inside a syllable, breaks it.
fn assert_reading_matches_input(raw: &str, reading: &AbbrevReading) {
    let mut at = 0usize;
    for unit in &reading.syllables {
        let text = unit_text(*unit);
        let end = at + text.len();
        assert_eq!(
            raw.get(at..end),
            Some(text.as_str()),
            "the unit {text:?} of {raw:?} does not start at {at}"
        );
        at = end;
        if raw.as_bytes().get(at) == Some(&b'\'') {
            at += 1;
        }
    }
    assert_eq!(
        Some(reading.consumed),
        u16::try_from(at).ok(),
        "{raw:?} is not consumed up to where the reading says"
    );
    assert!(at > 0, "a reading consumes at least one byte");
}

#[test]
fn test_readings_of_a_full_abbreviation_read_every_letter_as_an_initial() {
    // Every letter of `bjdx` is an initial and none of them spells a syllable, so the
    // input has exactly one reading per prefix of itself.
    let found = readings("bjdx");
    let expected: &[(u16, Vec<&str>)] = &[
        (4, vec!["b", "j", "d", "x"]),
        (3, vec!["b", "j", "d"]),
        (2, vec!["b", "j"]),
        (1, vec!["b"]),
    ];
    assert_shape(&found, expected);
    assert!(found[0].has_initial());
    assert_eq!(found[0].syllables.len(), 4);
}

#[test]
fn test_readings_of_a_mixed_input_keep_the_full_syllable_and_the_initial() {
    // `nih` spells `ni` out and leaves `hao` as its initial.
    let found = readings("nih");
    assert_eq!(found[0].consumed, 3);
    assert_eq!(units(&found[0]), vec!["ni", "h"]);
    assert!(found[0].has_initial());
    // `ni` has a reading of its own, and the leading `n` is read both as the syllable
    // it is and as the initial it can also stand for.
    assert!(
        found
            .iter()
            .any(|reading| units(reading) == ["ni"] && !reading.has_initial()),
        "the fully spelled reading is missing"
    );
    let spelled = lookup("n").expect("n is a syllable of the table");
    assert!(
        found
            .iter()
            .any(|reading| reading.syllables == [AbbrevSyllable::Full(spelled)])
    );
    assert!(
        found
            .iter()
            .any(|reading| reading.syllables == [AbbrevSyllable::Initial(b'n')])
    );
}

#[test]
fn test_readings_resolve_a_letter_that_is_both_a_syllable_and_an_initial() {
    // `n` is the interjection syllable and the initial of `ni`, `na` and the rest, so
    // `nh` reads two ways at once and neither of them is dropped. The reading that
    // spells the syllable out is the more specific of the two and comes first, which
    // is what makes the order the caller sees a rule rather than a coin flip.
    let found = readings("nh");
    let spelled = lookup("n").expect("n is a syllable of the table");
    assert_eq!(found.len(), 4);
    assert_eq!(
        found[0].syllables,
        vec![AbbrevSyllable::Full(spelled), AbbrevSyllable::Initial(b'h')],
        "the syllable reading is the more specific one and comes first"
    );
    assert_eq!(found[0].consumed, 2);
    // The all-initial reading survives beside it, and it is not the first.
    let all_initials = vec![AbbrevSyllable::Initial(b'n'), AbbrevSyllable::Initial(b'h')];
    assert!(
        found
            .iter()
            .any(|reading| reading.syllables == all_initials),
        "the all-initial reading must survive beside the syllable one"
    );
    assert_ne!(found[0].syllables, all_initials);
    // Both one-letter readings of `n` itself are there as well -- the syllable and
    // the initial -- again with the syllable first.
    assert!(
        found
            .iter()
            .any(|reading| reading.syllables == [AbbrevSyllable::Full(spelled)])
    );
    assert!(
        found
            .iter()
            .any(|reading| reading.syllables == [AbbrevSyllable::Initial(b'n')])
    );
}

#[test]
fn test_readings_prefer_the_longest_syllable_first() {
    // `nihao` cuts as `ni|hao`, and that reading is the most specific one the input
    // has, so it comes first; the reading that spells every letter as an initial is
    // the vaguest, so it comes last.
    let found = readings("nihao");
    assert_eq!(units(&found[0]), vec!["ni", "hao"]);
    assert_eq!(found[0].consumed, 5);
    assert!(!found[0].has_initial());

    let last = found.last().expect("the input has readings");
    assert_eq!(units(last), vec!["n"]);
    assert!(last.has_initial());
    assert_eq!(last.consumed, 1);
}

#[test]
fn test_readings_hold_a_reading_for_every_prefix_of_the_input() {
    // A reading consumes a prefix of the input rather than all of it, so a caller
    // walking the input node by node finds the reading that reaches the node it is
    // looking at.
    let found = readings("bjdx");
    let consumed: Vec<u16> = found.iter().map(|reading| reading.consumed).collect();
    assert_eq!(consumed, vec![4, 3, 2, 1]);
}

#[test]
fn test_readings_of_a_single_letter_are_empty() {
    // A one-letter input stands for a whole first-letter block of the dictionary, so
    // abbreviation does not apply to it at all.
    for raw in ["n", "a", "b", "z", "ê"] {
        assert!(readings(raw).is_empty(), "{raw:?} must have no reading");
        let mut buffer = Readings::new();
        assert!(!readings_into(&mut buffer, raw), "{raw:?}");
        assert!(buffer.is_empty());
    }
    assert_eq!(MIN_LETTERS, 2);
}

#[test]
fn test_readings_of_an_input_no_unit_can_start_are_empty() {
    // Nothing here starts a unit: the empty input, letters no syllable and no initial
    // begins with, and input that is nothing but boundary markers.
    for raw in ["", "ii", "uu", "vv", "''", "'i'"] {
        assert!(readings(raw).is_empty(), "{raw:?} must have no reading");
    }
}

#[test]
fn test_readings_of_an_input_that_is_not_normalized_are_empty() {
    // The alphabet is the normalized one, so an upper-case letter starts no unit: an
    // input that was not normalized has fewer readings rather than being misread.
    assert!(readings("Ni").is_empty());
    assert!(readings("Nihao").is_empty());
    assert!(!readings("nihao").is_empty());
}

#[test]
fn test_readings_stop_at_the_reading_limit() {
    // Sixty-four `a`s read 2^64 ways, and the cap is what keeps the work bounded. The
    // answer says the enumeration was cut short, which is the signal behind the
    // truncation diagnostic.
    let whole = u16::try_from(MAX_RAW_LEN).expect("the raw limit fits a u16");
    let long = "a".repeat(MAX_RAW_LEN);
    let mut buffer = Readings::new();
    assert!(
        readings_into(&mut buffer, &long),
        "the enumeration must report the cut"
    );
    assert_eq!(buffer.len(), MAX_READINGS);
    // The walk is depth-first, so the first reading out is the deepest one: the whole
    // input read as full syllables. The cap then keeps whatever the walk reached next,
    // which is a mix of depths -- what it must never do is drop that first reading,
    // which is the one a caller asking for the whole word needs.
    assert_eq!(
        buffer.as_slice()[0].consumed,
        whole,
        "the deepest reading comes out first"
    );
    assert!(
        buffer
            .as_slice()
            .iter()
            .any(|reading| reading.consumed == whole),
        "the whole-input reading survives the cap"
    );
    // An input that fits under the cap says so.
    let mut buffer = Readings::new();
    assert!(!readings_into(&mut buffer, "nihao"));
}

#[test]
fn test_readings_report_truncation_only_when_a_reading_is_dropped() {
    // `a` repeated n times reads 2^(n+1) - 2 ways: `a` is both a syllable and an
    // initial, no longer spelling of it (`aa`, `aaa`, ...) is a syllable either, and
    // a reading covers every prefix of the input, so the enumeration is a binary
    // tree. Five letters fit under the cap and are enumerated whole, which the
    // answer reports as "nothing was dropped".
    let mut buffer = Readings::new();
    assert!(!readings_into(&mut buffer, "aaaaa"));
    assert_eq!(buffer.len(), (1 << 6) - 2);
    assert!(buffer.len() < MAX_READINGS);
    assert_eq!(buffer.as_slice()[0].consumed, 5);
    assert!(
        !buffer.as_slice()[0].has_initial(),
        "the whole input read as syllables comes out first"
    );

    // Six letters read 126 ways, past the cap. The answer is the signal behind
    // `decode/abbrev-truncated`, and the reading it keeps in front of the rest is
    // still the most specific one.
    assert!(readings_into(&mut buffer, "aaaaaa"));
    assert_eq!(buffer.len(), MAX_READINGS);
    assert!(buffer.len() < (1 << 7) - 2);
    assert_eq!(buffer.as_slice()[0].consumed, 6);
    assert!(!buffer.as_slice()[0].has_initial());
}

#[test]
fn test_readings_are_deterministic_across_100_runs() {
    for raw in ["bjdx", "nihao", "zhongguo", "n'h", "beijingdaxue"] {
        let expected = shape(&readings(raw));
        assert!(!expected.is_empty(), "{raw:?} must have readings");
        for run in 0..100 {
            assert_eq!(shape(&readings(raw)), expected, "{raw:?}, run {run}");
        }
    }
}

#[test]
fn test_readings_consume_a_boundary_marker_with_the_unit_before_it() {
    // The marker belongs to no syllable, which is the rule the graph's forced edge
    // follows, so the unit before it covers it and the next unit starts past it.
    let found = readings("n'h");
    // Four readings, not two: the enumeration answers for every prefix, so the input up
    // to the marker and the whole input both appear, and each of those is reachable two
    // ways because `n` is a syllable of its own as well as the initial of `ni` and `na`.
    let mut consumed: Vec<u16> = found.iter().map(|reading| reading.consumed).collect();
    consumed.sort_unstable();
    assert_eq!(consumed, vec![2, 2, 3, 3]);

    for reading in found.iter().filter(|reading| reading.consumed == 3) {
        assert_eq!(
            units(reading),
            vec!["n", "h"],
            "the marker is swallowed, not read"
        );
        assert!(
            reading.has_initial(),
            "`h` is an initial and nothing else, so no reading ending in it is all syllables"
        );
    }
}

#[test]
fn test_readings_spell_the_input_they_cover() {
    for raw in [
        "bjdx",
        "nihao",
        "ni'hao",
        "n'h",
        "zhongguo",
        "beijingdaxue",
        "xian",
        "zhuang",
    ] {
        let found = readings(raw);
        assert!(!found.is_empty(), "{raw:?} must have readings");
        for reading in &found {
            assert_reading_matches_input(raw, reading);
        }
    }
}

#[test]
fn test_readings_of_every_table_syllable_hold_the_syllable_itself() {
    for &entry in SYLLABLES {
        let found = readings(entry);
        if entry.chars().count() < MIN_LETTERS {
            assert!(found.is_empty(), "{entry:?} is a single letter");
            continue;
        }
        let spelled = lookup(entry).expect("a table entry is a syllable");
        let whole = [AbbrevSyllable::Full(spelled)];
        assert!(
            found.iter().any(|reading| reading.syllables == whole),
            "{entry:?} must have a reading of its own spelling"
        );
        assert!(
            found.iter().any(|reading| !reading.has_initial()),
            "{entry:?} must have a reading that is no abbreviation"
        );
    }
}

/// Every capacity the enumeration can grow, so that a repeated call can be shown not
/// to have grown any of them.
fn capacities(buffer: &Readings) -> Vec<usize> {
    let mut seen = vec![
        buffer.entries.capacity(),
        buffer.spare.capacity(),
        buffer.path.capacity(),
    ];
    seen.extend(
        buffer
            .entries
            .iter()
            .map(|reading| reading.syllables.capacity()),
    );
    seen.extend(buffer.spare.iter().map(|syllables| syllables.capacity()));
    seen
}

#[test]
fn test_readings_into_reuses_the_callers_buffers() {
    // A reading list that fits under the cap, so that the enumeration is the whole
    // one rather than a truncated prefix of it.
    let raw = "nihao";
    let mut buffer = Readings::new();
    readings_into(&mut buffer, raw);
    readings_into(&mut buffer, raw);
    let expected = shape(buffer.as_slice());
    assert!(!expected.is_empty(), "the input must have readings");
    let before = capacities(&buffer);
    // The third call is the steady state: everything it needs is already there, so a
    // capacity that changed is an allocation.
    assert!(!readings_into(&mut buffer, raw));
    assert_eq!(shape(buffer.as_slice()), expected);
    assert_eq!(
        capacities(&buffer),
        before,
        "a steady-state enumeration grows no buffer"
    );
}

#[test]
fn test_readings_into_replaces_the_readings_of_the_previous_input() {
    let mut buffer = Readings::new();
    readings_into(&mut buffer, "beijingdaxue");
    assert!(buffer.len() > 1);
    // A second input replaces the readings rather than being appended to them.
    assert!(!readings_into(&mut buffer, "nih"));
    assert_eq!(shape(buffer.as_slice()), shape(&readings("nih")));
    // An input with no reading at all empties the buffer.
    assert!(!readings_into(&mut buffer, "ii"));
    assert!(buffer.is_empty());
}

#[test]
fn test_reading_has_initial_reports_a_mixed_reading() {
    assert!(!reading(&[("ni", false)], 2).has_initial());
    assert!(!reading(&[("ni", false), ("hao", false)], 5).has_initial());
    assert!(reading(&[("ni", false), ("h", true)], 3).has_initial());
    assert!(reading(&[("h", true)], 1).has_initial());
    // A reading with no unit at all is what a caller holds before the first
    // enumeration, and it is no abbreviation.
    let empty = AbbrevReading {
        syllables: Vec::new(),
        consumed: 0,
    };
    assert!(!empty.has_initial());
}

#[test]
fn test_initial_table_matches_the_syllable_table() {
    for letter in b'a'..=b'z' {
        assert_eq!(
            initials_of(letter),
            derived_initials(letter).as_slice(),
            "letter {}",
            char::from(letter)
        );
    }
}

/// The initials the syllable table actually begins its syllables with, derived from
/// the table rather than from the shipped table of this module.
fn derived_initials(letter: u8) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for &syllable in SYLLABLES {
        let Some(first) = syllable.chars().next() else {
            continue;
        };
        if !first.is_ascii() || u8::try_from(first).ok() != Some(letter) {
            continue;
        }
        // A syllable with no initial of its own -- `a`, `e`, `o` and the interjections
        // -- is reached by its own first letter.
        let onset = ONSETS
            .iter()
            .copied()
            .filter(|onset| syllable.starts_with(onset))
            .max_by_key(|onset| onset.len())
            .unwrap_or_else(|| syllable.get(..1).unwrap_or(syllable));
        if !out.contains(&onset) {
            out.push(onset);
        }
    }
    out
}

#[test]
fn test_initials_of_rejects_a_letter_no_syllable_starts_with() {
    // No syllable begins with `i`, `u` or `v` (normalization folds the last into
    // `ü`), so none of them can ever be a bare initial.
    for letter in *b"iuv" {
        assert!(initials_of(letter).is_empty(), "{}", char::from(letter));
    }
    // A byte outside `a` through `z` is not a letter of the input alphabet at all.
    for byte in [0u8, b'A', b'0', b'z' + 1, 0xC3, u8::MAX] {
        assert!(initials_of(byte).is_empty(), "{byte}");
    }
    // The three ambiguous letters are the only ones that reach more than one
    // spelling, and they are the reason a reading can spell several queries.
    assert_eq!(initials_of(b'z'), &["z", "zh"]);
    assert_eq!(initials_of(b'c'), &["c", "ch"]);
    assert_eq!(initials_of(b's'), &["s", "sh"]);
    assert_eq!(initials_of(b'h'), &["h"]);
    assert_eq!(initials_of(b'a'), &["a"]);
}

#[test]
fn test_spelling_count_multiplies_the_choices_of_every_initial() {
    assert_eq!(spelling_count(&reading(&[("ni", false)], 2)), 1);
    assert_eq!(
        spelling_count(&reading(&[("ni", false), ("h", true)], 3)),
        1
    );
    let four = [("z", true), ("g", true), ("r", true), ("m", true)];
    assert_eq!(spelling_count(&reading(&four, 4)), 2);
    let three = [("z", true), ("c", true), ("s", true)];
    assert_eq!(spelling_count(&reading(&three, 3)), 8);
    // A reading with no unit at all spells the empty query, once.
    let empty = AbbrevReading {
        syllables: Vec::new(),
        consumed: 0,
    };
    assert_eq!(spelling_count(&empty), 1);
}

#[test]
fn test_spell_into_writes_the_query_the_dictionary_is_keyed_by() {
    // A reading that spells its first syllable out spells a literal prefix of the
    // word's own key, which is the form a full-pinyin index already answers.
    let mixed = reading(&[("ni", false), ("h", true)], 3);
    let mut query = String::new();
    assert!(spell_into(&mut query, &mixed, 0));
    assert_eq!(query, "ni'h");
    assert!("ni'hao".starts_with(query.as_str()));

    // A reading made of initials spells the abbreviation key itself, and `z` stands
    // for both `z` and `zh`.
    let initials = reading(&[("z", true), ("g", true), ("r", true), ("m", true)], 4);
    assert_eq!(spelling_count(&initials), 2);
    assert!(spell_into(&mut query, &initials, 0));
    assert_eq!(query, "z'g'r'm");
    assert!(spell_into(&mut query, &initials, 1));
    assert_eq!(query, "zh'g'r'm");

    // The query of a reading taken from the input is the one the input spells.
    let found = readings("nih");
    assert!(spell_into(&mut query, &found[0], 0));
    assert_eq!(query, "ni'h");
}

#[test]
fn test_spell_into_refuses_a_combination_past_the_count() {
    let spelled = reading(&[("z", true), ("g", true)], 2);
    let mut query = String::from("stale");
    assert!(spell_into(&mut query, &spelled, 1));
    assert_eq!(query, "zh'g");
    assert!(!spell_into(&mut query, &spelled, 2));
    assert!(
        query.is_empty(),
        "a refused combination leaves the buffer empty"
    );
    assert!(!spell_into(&mut query, &spelled, usize::MAX));
    assert!(query.is_empty());
}

#[test]
fn test_spell_into_refuses_a_syllable_the_table_does_not_hold() {
    let unknown = AbbrevReading {
        syllables: vec![AbbrevSyllable::Full(SyllableId::new(u16::MAX))],
        consumed: 1,
    };
    let mut query = String::from("stale");
    assert!(!spell_into(&mut query, &unknown, 0));
    assert!(query.is_empty());
}

#[test]
fn test_spell_into_refuses_a_reading_whose_initial_starts_no_syllable() {
    // No syllable begins with `i`, so a reading built out of it as a bare initial
    // spells no query at all. The count is zero and every combination is refused
    // before the mixed-radix walk would divide by the empty list of choices, which
    // is why the walk has no panic path even for a reading the enumeration itself
    // would never produce.
    let impossible = reading(&[("z", true), ("i", true)], 2);
    assert!(initials_of(b'i').is_empty());
    assert_eq!(spelling_count(&impossible), 0);
    let mut query = String::from("stale");
    assert!(!spell_into(&mut query, &impossible, 0));
    assert!(query.is_empty());
    assert!(!spell_into(&mut query, &impossible, usize::MAX));
    assert!(query.is_empty());
}

#[test]
fn test_spell_into_refuses_a_reading_past_the_spelling_cap() {
    // Seven ambiguous initials spell 128 queries, past the cap the lattice ever
    // needs, since a word covers at most six syllables. The count answers with the
    // cap and every query is refused rather than silently cut to the first ones.
    let long = reading(&[("z", true); 7], 7);
    assert_eq!(spelling_count(&long), MAX_SPELLINGS);
    let mut query = String::new();
    for combination in 0..spelling_count(&long) {
        assert!(!spell_into(&mut query, &long, combination));
        assert!(query.is_empty());
    }
}

#[test]
fn test_spellings_of_one_reading_are_all_distinct() {
    let spelled = reading(&[("z", true), ("c", true), ("s", true)], 3);
    let mut seen: Vec<String> = Vec::new();
    let mut query = String::new();
    for combination in 0..spelling_count(&spelled) {
        assert!(spell_into(&mut query, &spelled, combination));
        assert!(!seen.contains(&query), "{query} was spelled twice");
        seen.push(query.clone());
    }
    assert_eq!(seen.len(), 8);
    // The choices of the first initial vary slowest, so the enumeration is fixed.
    assert_eq!(seen[0], "z'c's");
    assert_eq!(seen[1], "z'c'sh");
    assert_eq!(seen[4], "zh'c's");
    assert_eq!(seen[7], "zh'ch'sh");
}

#[test]
fn test_penalize_makes_an_exact_edge_outrank_an_abbreviated_one() {
    // Two edges of equal dictionary weight: the fully spelled one has to win, which
    // is what the penalty buys.
    let spelled_out = -3 << 16;
    let abbreviated = penalize(spelled_out);
    assert!(abbreviated < spelled_out);
    assert_eq!(spelled_out - abbreviated, ABBREV_PENALTY_Q8);
    // The two relations that fix the order of a shortening against a misspelling are
    // checked at build time beside the constants; see `abbrev`.
}

#[test]
fn test_penalize_saturates_at_the_floor() {
    assert_eq!(penalize(i32::MIN), i32::MIN);
    // A score just above the floor is still closer to it than the penalty is wide, so
    // the subtraction saturates rather than wrapping to a huge positive score -- which
    // would rank an abbreviation of the worst word above every real candidate.
    assert_eq!(penalize(i32::MIN + 1), i32::MIN);
    assert_eq!(penalize(i32::MIN + ABBREV_PENALTY_Q8), i32::MIN);
    assert_eq!(
        penalize(i32::MIN + ABBREV_PENALTY_Q8 + 1),
        i32::MIN + 1,
        "the first score the penalty fits under comes out exactly"
    );
}

#[test]
fn test_is_enabled_follows_the_frozen_flag() {
    assert!(is_enabled(DecodeFlags::ABBREV));
    assert!(is_enabled(DecodeFlags::ABBREV | DecodeFlags::USER_DICT));
    assert!(!is_enabled(DecodeFlags::empty()));
    assert!(!is_enabled(DecodeFlags::FUZZY | DecodeFlags::FUZZY_ZH_Z));
}

#[test]
fn test_is_enabled_over_every_flag_combination_follows_the_abbrev_bit() {
    // The gate is the frozen bit and nothing beside it: unlike fuzzy matching there
    // is no class namespace to consult, so a caller that asks before enumerating
    // pays for abbreviation exactly when the user switched it on and nothing at all
    // when they did not. Exhaustive over the namespace rather than sampled, because
    // "free when off" is the whole point of the gate.
    let mut enabled = 0usize;
    for raw in 0..=DecodeFlags::all().bits() {
        let Some(flags) = DecodeFlags::from_bits(raw) else {
            continue;
        };
        let expected = flags.contains(DecodeFlags::ABBREV);
        assert_eq!(is_enabled(flags), expected, "{flags:?}");
        enabled += usize::from(expected);
    }
    // Exactly half the namespace carries the bit, so a gate that always answered
    // `true` or always `false` could not have got through the loop above.
    assert_eq!(enabled, 1 << 13);
}

#[test]
fn test_abbreviation_budgets_are_the_shipped_ones() {
    assert_eq!(MAX_READINGS, 64);
    assert_eq!(PREFIX_LIMIT, 64);
    assert_eq!(MAX_SPELLINGS, 64);
    assert_eq!(MIN_LETTERS, 2);
    assert_eq!(ABBREV_PENALTY_Q8, 6 << 8);
}

#[test]
fn test_abbrev_truncated_code_is_the_registered_one() {
    // The code is what a diagnostic is matched on, so it is part of the contract and
    // is never reworded. It reports an informational outcome rather than a failure:
    // the decode still answers, with the readings the cap left room for.
    assert_eq!(ABBREV_TRUNCATED_CODE, "decode/abbrev-truncated");
    assert!(ABBREV_TRUNCATED_CODE.starts_with("decode/"));
}
