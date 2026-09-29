//! The sequence machine, stroke by stroke.
//!
//! Two levels of testing, and they answer different questions. The sweeps drive every
//! keysym and modifier state in a corpus through the machine, to show that no combination
//! is left without an answer and that nothing but the bound strokes is ever claimed. The
//! named cases drive one contract at a time: the four answers a stroke can get, the
//! deadline, the `Escape` rule, the state a context change clears, and the invariant that a
//! stroke which leads nowhere is handed back rather than swallowed.
//!
//! Every timestamp comes from the test rather than from a clock, so the deadline is
//! checked at the millisecond that matters and no test ever sleeps.

use super::{
    KeySequence, MAX_SEQUENCE_STROKES, SEQUENCE_CONFLICT_CODE, SEQUENCE_TIMEOUT_MS,
    SEQUENCE_TOO_LONG_CODE, SequenceDecision, SequencePrefix, SequenceState, SequenceTable,
};
use crate::engine::{Consumed, KEY_ESCAPE, KeyEvent, arbitrate_sequence};

/// `fcitx::KeyState::Shift`.
const SHIFT: u32 = 1 << 0;
/// `fcitx::KeyState::Ctrl`.
const CTRL: u32 = 1 << 2;
/// `fcitx::KeyState::Alt`.
const ALT: u32 = 1 << 3;

/// `FcitxKey_k`, the first stroke of every sequence these tests bind.
const KEY_K: u32 = 0x6b;
/// `FcitxKey_s`, the second stroke of the first bound sequence.
const KEY_S: u32 = 0x73;
/// `FcitxKey_w`, the second stroke of the second bound sequence.
const KEY_W: u32 = 0x77;
/// `FcitxKey_F35`, a keysym no table in these tests names.
const KEY_F35: u32 = 0xffbe;

/// The modifier states the sweeps run over.
const STATES: [u32; 5] = [0, SHIFT, CTRL, CTRL | SHIFT, CTRL | SHIFT | ALT];

/// One stroke.
fn stroke(sym: u32, state: u32) -> SequencePrefix {
    SequencePrefix { sym, state }
}

/// A key press.
fn press(sym: u32, state: u32, time_ms: u32) -> KeyEvent {
    KeyEvent {
        sym,
        state,
        is_release: false,
        time_ms,
    }
}

/// A key release.
fn release(sym: u32, state: u32, time_ms: u32) -> KeyEvent {
    KeyEvent {
        sym,
        state,
        is_release: true,
        time_ms,
    }
}

/// The decision a completed sequence produces.
fn completed(binding: &'static str) -> SequenceDecision<&'static str> {
    SequenceDecision::Completed { binding }
}

/// The decision a dropped sequence produces.
fn abandoned(sym: u32, state: u32, depth: u8) -> SequenceDecision<&'static str> {
    SequenceDecision::Abandoned {
        prefix: stroke(sym, state),
        buffered: depth,
    }
}

/// The code and the reason a `bind` refusal carries.
///
/// The frozen error model has no variant for a key-binding table, so a refusal arrives as
/// [`ime_types::ImeError::ConfigInvalid`] with the stable code in its `key` field. Pulling
/// the pair back out lets a test name both without matching the whole rendering.
fn refused(result: Result<(), ime_types::ImeError>) -> (String, String) {
    match result {
        Ok(()) => (String::from("<accepted>"), String::new()),
        Err(ime_types::ImeError::ConfigInvalid { key, reason }) => (key, reason),
        Err(_) => (String::new(), String::new()),
    }
}

/// A table with two sequences that share their first stroke: `Ctrl+K Ctrl+S` and
/// `Ctrl+K Ctrl+W`.
fn two_stroke_table() -> SequenceTable<&'static str> {
    let mut table = SequenceTable::new();
    let save = [stroke(KEY_K, CTRL), stroke(KEY_S, CTRL)];
    let close = [stroke(KEY_K, CTRL), stroke(KEY_W, CTRL)];
    assert!(table.bind(&save, "save").is_ok());
    assert!(table.bind(&close, "close").is_ok());
    table
}

/// A table with one single-stroke sequence: `Ctrl+K`.
fn one_stroke_table() -> SequenceTable<&'static str> {
    let mut table = SequenceTable::new();
    assert!(table.bind(&[stroke(KEY_K, CTRL)], "palette").is_ok());
    table
}

/// Every keysym the sweeps run over: the two ranges the engine's tables read, plus a
/// deterministic pseudo-random tail for the keysyms nothing may claim.
fn corpus() -> Vec<u32> {
    let mut syms: Vec<u32> = (0x20..=0x100).collect();
    syms.extend(0xff00..=0xffff);
    syms.extend(pseudo_random_syms(200));
    // The tail may repeat a keysym from the ranges above. The sweeps count distinct
    // keysyms, so the repeats are dropped rather than counted twice.
    syms.sort_unstable();
    syms.dedup();
    syms
}

/// A deterministic keysym stream from a 32-bit LCG: reproducible, and needs no dependency
/// (`rand` is not one of this workspace's crates).
fn pseudo_random_syms(count: usize) -> Vec<u32> {
    let mut seed: u32 = 0x1234_5678;
    (0..count)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            seed
        })
        .collect()
}

#[test]
fn test_sequence_constants_match_the_contract() {
    // The two numbers are the design's, and the two codes are what a diagnostic or a test
    // matches on. Pinning them here makes a change to either one a decision.
    assert_eq!(SEQUENCE_TIMEOUT_MS, 1_500);
    assert_eq!(MAX_SEQUENCE_STROKES, 4);
    assert_eq!(SEQUENCE_CONFLICT_CODE, "keys/sequence-conflict");
    assert_eq!(SEQUENCE_TOO_LONG_CODE, "keys/sequence-too-long");
}

#[test]
fn test_sequence_table_new_starts_empty() {
    let table: SequenceTable<&'static str> = SequenceTable::new();
    assert!(table.is_empty());
    assert_eq!(table.len(), 0);

    let default: SequenceTable<&'static str> = SequenceTable::default();
    assert!(default.is_empty());
    assert_eq!(default.len(), 0);
}

#[test]
fn test_sequence_table_bind_registers_sequences_of_every_shape() {
    let mut table = two_stroke_table();
    assert!(!table.is_empty());
    assert_eq!(table.len(), 2, "two sequences that share a first stroke");

    // A stroke nobody else uses opens a branch of its own.
    assert!(table.bind(&[stroke(KEY_F35, 0)], "help").is_ok());
    assert_eq!(table.len(), 3);

    // And a longer sequence hangs off a stroke that is not a prefix of anything yet.
    let nested = [
        stroke(KEY_W, CTRL),
        stroke(KEY_K, CTRL),
        stroke(KEY_S, CTRL),
    ];
    assert!(table.bind(&nested, "nested").is_ok());
    assert_eq!(table.len(), 4);
}

#[test]
fn test_sequence_table_bind_refuses_a_sequence_already_bound() {
    let mut table = one_stroke_table();
    let (code, reason) = refused(table.bind(&[stroke(KEY_K, CTRL)], "again"));
    assert_eq!(code, SEQUENCE_CONFLICT_CODE);
    assert_eq!(reason, "a sequence that is already bound");
    assert_eq!(table.len(), 1, "a refused binding changes nothing");
}

#[test]
fn test_sequence_table_bind_refuses_a_sequence_that_prefixes_a_bound_one() {
    let mut table = two_stroke_table();
    let (code, reason) = refused(table.bind(&[stroke(KEY_K, CTRL)], "palette"));
    assert_eq!(code, SEQUENCE_CONFLICT_CODE);
    assert_eq!(reason, "a sequence that is a prefix of one already bound");
    assert_eq!(table.len(), 2);
}

#[test]
fn test_sequence_table_bind_refuses_a_sequence_that_extends_a_bound_one() {
    let mut table = one_stroke_table();
    let save = [stroke(KEY_K, CTRL), stroke(KEY_S, CTRL)];
    let (code, reason) = refused(table.bind(&save, "save"));
    assert_eq!(code, SEQUENCE_CONFLICT_CODE);
    assert_eq!(reason, "a sequence that extends one already bound");
    assert_eq!(table.len(), 1);
}

#[test]
fn test_sequence_table_bind_refuses_an_empty_sequence() {
    let mut table = SequenceTable::<&'static str>::new();
    let (code, reason) = refused(table.bind(&[], "nothing"));
    assert_eq!(code, SEQUENCE_CONFLICT_CODE);
    assert_eq!(reason, "a sequence needs at least one stroke");
    assert_eq!(table.len(), 0);
}

#[test]
fn test_sequence_table_bind_refuses_a_sequence_one_stroke_too_long() {
    let mut table = SequenceTable::<&'static str>::new();
    let longest = [stroke(KEY_K, CTRL); MAX_SEQUENCE_STROKES];
    assert!(table.bind(&longest, "longest").is_ok());

    let mut longer: Vec<SequencePrefix> = longest.to_vec();
    longer.push(stroke(KEY_S, CTRL));
    let (code, reason) = refused(table.bind(&longer, "longer"));
    assert_eq!(code, SEQUENCE_TOO_LONG_CODE);

    let expected = format!("len={} max={MAX_SEQUENCE_STROKES}", longer.len());
    assert_eq!(reason, expected);
    assert_eq!(table.len(), 1, "a refused binding changes nothing");
}

#[test]
fn test_sequence_table_bind_leaves_the_table_usable_when_it_refuses() {
    // A refusal that had inserted part of the path would show up here: both sequences that
    // were already bound must still complete.
    let mut table = two_stroke_table();
    assert!(table.bind(&[stroke(KEY_K, CTRL)], "palette").is_err());
    let nested = [
        stroke(KEY_K, CTRL),
        stroke(KEY_S, CTRL),
        stroke(KEY_S, CTRL),
    ];
    assert!(table.bind(&nested, "nested").is_err());

    let mut sequence = KeySequence::new();
    let first = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(first, SequenceDecision::Opened);
    let second = sequence.offer(&press(KEY_S, CTRL, 10), &table);
    assert_eq!(second, completed("save"));
    let third = sequence.offer(&press(KEY_K, CTRL, 20), &table);
    assert_eq!(third, SequenceDecision::Opened);
    let fourth = sequence.offer(&press(KEY_W, CTRL, 30), &table);
    assert_eq!(fourth, completed("close"));
}

#[test]
fn test_key_sequence_new_is_idle() {
    let sequence = KeySequence::new();
    assert_eq!(sequence.state(), SequenceState::Idle);

    let default = KeySequence::default();
    assert_eq!(default.state(), SequenceState::Idle);
}

#[test]
fn test_key_sequence_offer_with_an_empty_table_always_passes() {
    // The property the whole design rests on: a plugin with no sequences registered routes
    // every key exactly as it did before the machine existed.
    let table = SequenceTable::<&'static str>::new();
    let mut sequence = KeySequence::new();
    let keys = corpus();
    let mut passed = 0usize;
    for sym in &keys {
        for state in STATES {
            for is_release in [false, true] {
                let event = KeyEvent {
                    sym: *sym,
                    state,
                    is_release,
                    time_ms: 0,
                };
                let decision = sequence.offer(&event, &table);
                assert!(
                    matches!(decision, SequenceDecision::Pass),
                    "sym {sym:#06x} state {state:#x} release {is_release}"
                );
                assert!(!decision.keeps_key());
                assert_eq!(sequence.state(), SequenceState::Idle);
                passed += 1;
            }
        }
    }
    assert_eq!(passed, keys.len() * STATES.len() * 2);
    assert!(passed > 0, "the sweep must not be empty");
}

#[test]
fn test_key_sequence_offer_completes_only_the_strokes_the_table_names() {
    // The exhaustive statement of what a one-stroke table does: exactly one combination
    // out of the whole corpus is claimed, and every other key passes.
    let table = one_stroke_table();
    let mut sequence = KeySequence::new();
    let keys = corpus();
    let mut completed_count = 0usize;
    let mut passed = 0usize;
    for sym in &keys {
        for state in STATES {
            let decision = sequence.offer(&press(*sym, state, 0), &table);
            let expected = if *sym == KEY_K && state == CTRL {
                completed_count += 1;
                completed("palette")
            } else {
                passed += 1;
                SequenceDecision::Pass
            };
            assert_eq!(decision, expected, "sym {sym:#06x} state {state:#x}");
            assert_eq!(sequence.state(), SequenceState::Idle);
        }
    }
    assert_eq!(completed_count, 1, "one combination completes the sequence");
    assert_eq!(passed, keys.len() * STATES.len() - 1);
}

#[test]
fn test_key_sequence_offer_opens_on_the_first_stroke_of_a_sequence() {
    let table = two_stroke_table();
    let mut sequence = KeySequence::new();
    let decision = sequence.offer(&press(KEY_K, CTRL, 7), &table);
    assert_eq!(decision, SequenceDecision::Opened);
    assert!(decision.keeps_key(), "a prefix is already the plugin's key");
    assert_eq!(sequence.state(), SequenceState::Pending);
}

#[test]
fn test_key_sequence_offer_completes_on_the_second_stroke() {
    let table = two_stroke_table();
    let mut sequence = KeySequence::new();
    let opened = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(opened, SequenceDecision::Opened);

    let decision = sequence.offer(&press(KEY_S, CTRL, 30), &table);
    assert_eq!(decision, completed("save"));
    assert!(decision.keeps_key());
    assert_eq!(sequence.state(), SequenceState::Idle);
}

#[test]
fn test_key_sequence_offer_opens_each_stroke_of_a_longer_sequence() {
    let mut table = SequenceTable::new();
    let save_all = [
        stroke(KEY_K, CTRL),
        stroke(KEY_S, CTRL),
        stroke(KEY_W, CTRL),
    ];
    assert!(table.bind(&save_all, "save-all").is_ok());

    let mut sequence = KeySequence::new();
    let first = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(first, SequenceDecision::Opened);
    let second = sequence.offer(&press(KEY_S, CTRL, 10), &table);
    assert_eq!(second, SequenceDecision::Opened, "one stroke still to come");
    assert_eq!(sequence.state(), SequenceState::Pending);
    let third = sequence.offer(&press(KEY_W, CTRL, 20), &table);
    assert_eq!(third, completed("save-all"));
    assert_eq!(sequence.state(), SequenceState::Idle);
}

#[test]
fn test_key_sequence_offer_abandons_and_hands_the_key_back() {
    // When a sequence is dropped, the stroke that dropped it belongs to the application.
    let table = two_stroke_table();
    let mut sequence = KeySequence::new();
    let opened = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(opened, SequenceDecision::Opened);

    let decision = sequence.offer(&press(KEY_S, 0, 20), &table);
    assert_eq!(decision, abandoned(KEY_K, CTRL, 1));
    assert!(!decision.keeps_key(), "the stroke is handed back");
    assert_eq!(sequence.state(), SequenceState::Idle);
}

#[test]
fn test_key_sequence_offer_reports_every_stroke_the_dropped_sequence_took() {
    let mut table = SequenceTable::new();
    let save_all = [
        stroke(KEY_K, CTRL),
        stroke(KEY_S, CTRL),
        stroke(KEY_W, CTRL),
    ];
    assert!(table.bind(&save_all, "save-all").is_ok());

    let mut sequence = KeySequence::new();
    let first = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(first, SequenceDecision::Opened);
    let second = sequence.offer(&press(KEY_S, CTRL, 10), &table);
    assert_eq!(second, SequenceDecision::Opened);
    let dropped = sequence.offer(&press(KEY_F35, 0, 20), &table);
    assert_eq!(dropped, abandoned(KEY_K, CTRL, 2));
}

#[test]
fn test_key_sequence_offer_does_not_reopen_on_the_stroke_that_abandoned_it() {
    // `Ctrl+K` is a prefix, so it could open a sequence again. It does not: the stroke that
    // drops a sequence travels on like any other key, which is what keeps a sequence from
    // hiding a keystroke inside a re-entry rule.
    let table = two_stroke_table();
    let mut sequence = KeySequence::new();
    let opened = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(opened, SequenceDecision::Opened);
    let again = sequence.offer(&press(KEY_K, CTRL, 20), &table);
    assert_eq!(again, abandoned(KEY_K, CTRL, 1));
    assert_eq!(sequence.state(), SequenceState::Idle);
}

#[test]
fn test_key_sequence_offer_cancels_the_sequence_on_escape() {
    let table = two_stroke_table();
    let mut sequence = KeySequence::new();
    let opened = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(opened, SequenceDecision::Opened);

    let decision = sequence.offer(&press(KEY_ESCAPE, 0, 20), &table);
    assert_eq!(decision, SequenceDecision::Cancelled);
    assert!(
        decision.keeps_key(),
        "the cancelling Escape is the plugin's"
    );
    assert_eq!(sequence.state(), SequenceState::Idle);

    // The sequence is gone: the stroke that would have completed it now passes.
    let after = sequence.offer(&press(KEY_S, CTRL, 30), &table);
    assert_eq!(after, SequenceDecision::Pass);
}

#[test]
fn test_key_sequence_offer_lets_a_bound_escape_complete_a_sequence() {
    // The table is consulted before the `Escape` rule, so a sequence that binds Escape
    // keeps it reachable instead of having it cancelled out from under it.
    let mut table = SequenceTable::new();
    let quit = [stroke(KEY_K, CTRL), stroke(KEY_ESCAPE, 0)];
    assert!(table.bind(&quit, "quit").is_ok());

    let mut sequence = KeySequence::new();
    let opened = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(opened, SequenceDecision::Opened);
    let decision = sequence.offer(&press(KEY_ESCAPE, 0, 10), &table);
    assert_eq!(decision, completed("quit"));
}

#[test]
fn test_key_sequence_offer_keeps_the_sequence_open_at_the_deadline() {
    let table = two_stroke_table();
    let mut sequence = KeySequence::new();
    let opened = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(opened, SequenceDecision::Opened);

    let at_deadline = press(KEY_S, CTRL, SEQUENCE_TIMEOUT_MS);
    let decision = sequence.offer(&at_deadline, &table);
    assert_eq!(decision, completed("save"));
}

#[test]
fn test_key_sequence_offer_expires_the_sequence_one_millisecond_later() {
    let table = two_stroke_table();
    let mut sequence = KeySequence::new();
    let opened = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(opened, SequenceDecision::Opened);

    let too_late = press(KEY_S, CTRL, SEQUENCE_TIMEOUT_MS + 1);
    let decision = sequence.offer(&too_late, &table);
    assert_eq!(decision, SequenceDecision::Pass);
    assert_eq!(sequence.state(), SequenceState::Idle);
}

#[test]
fn test_key_sequence_offer_opens_a_new_sequence_after_a_timeout() {
    let table = two_stroke_table();
    let mut sequence = KeySequence::new();
    let opened = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(opened, SequenceDecision::Opened);

    // The stale prefix is dropped, and the stroke that arrives is offered as if the
    // sequence had never been opened, so the same prefix opens a fresh one.
    let stale = press(KEY_K, CTRL, SEQUENCE_TIMEOUT_MS + 1);
    let decision = sequence.offer(&stale, &table);
    assert_eq!(decision, SequenceDecision::Opened);
    assert_eq!(sequence.state(), SequenceState::Pending);
}

#[test]
fn test_key_sequence_offer_measures_the_deadline_from_the_opening_stroke() {
    // Two strokes in, the clock keeps running from the first one rather than restarting: a
    // sequence the user keeps poking at does not live forever.
    let mut table = SequenceTable::new();
    let save_all = [
        stroke(KEY_K, CTRL),
        stroke(KEY_S, CTRL),
        stroke(KEY_W, CTRL),
    ];
    assert!(table.bind(&save_all, "save-all").is_ok());

    let mut sequence = KeySequence::new();
    let first = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(first, SequenceDecision::Opened);
    let second = sequence.offer(&press(KEY_S, CTRL, 1_000), &table);
    assert_eq!(second, SequenceDecision::Opened, "1000ms in, still open");

    // 1501ms after the *first* stroke. A clock restarted by the second stroke would have
    // 501ms on it and would complete the sequence.
    let third = sequence.offer(&press(KEY_W, CTRL, 1_501), &table);
    assert_eq!(third, SequenceDecision::Pass);
    assert_eq!(sequence.state(), SequenceState::Idle);
}

#[test]
fn test_key_sequence_offer_treats_a_backwards_clock_as_expired() {
    // The host's timestamp is a `u32`. A clock that moves backwards wraps to an enormous
    // elapsed time and the sequence is dropped -- the safe direction, because the other one
    // is a sequence that never ends.
    let table = two_stroke_table();
    let mut sequence = KeySequence::new();
    let opened = sequence.offer(&press(KEY_K, CTRL, 1_000), &table);
    assert_eq!(opened, SequenceDecision::Opened);

    let backwards = sequence.offer(&press(KEY_S, CTRL, 0), &table);
    assert_eq!(backwards, SequenceDecision::Pass);
    assert_eq!(sequence.state(), SequenceState::Idle);
}

#[test]
fn test_key_sequence_offer_follows_the_clock_across_a_wrap() {
    // The same `u32`, wrapping the other way: a sequence opened just before the wrap is
    // still open just after it, because the elapsed time is the wrapping difference.
    let table = two_stroke_table();
    let mut sequence = KeySequence::new();
    let before_wrap = u32::MAX - 10;
    let opened = sequence.offer(&press(KEY_K, CTRL, before_wrap), &table);
    assert_eq!(opened, SequenceDecision::Opened);

    let after = sequence.offer(&press(KEY_S, CTRL, 5), &table);
    assert_eq!(after, completed("save"));
}

#[test]
fn test_key_sequence_offer_never_takes_a_release() {
    let table = two_stroke_table();
    let keys = corpus();
    let mut passed = 0usize;
    for sym in &keys {
        for state in STATES {
            let mut sequence = KeySequence::new();
            let decision = sequence.offer(&release(*sym, state, 0), &table);
            assert!(
                matches!(decision, SequenceDecision::Pass),
                "sym {sym:#06x} state {state:#x}"
            );
            assert_eq!(sequence.state(), SequenceState::Idle);
            passed += 1;
        }
    }
    assert_eq!(passed, keys.len() * STATES.len());
    assert!(passed > 0);
}

#[test]
fn test_key_sequence_offer_keeps_a_pending_sequence_across_a_release() {
    // Releasing the modifier the prefix was typed with is not a stroke, and it must not
    // cancel what the user is in the middle of.
    let table = two_stroke_table();
    let mut sequence = KeySequence::new();
    let opened = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(opened, SequenceDecision::Opened);

    let up = sequence.offer(&release(KEY_K, CTRL, 5), &table);
    assert_eq!(up, SequenceDecision::Pass);
    assert_eq!(sequence.state(), SequenceState::Pending);

    let finished = sequence.offer(&press(KEY_S, CTRL, 10), &table);
    assert_eq!(finished, completed("save"));
}

#[test]
fn test_key_sequence_offer_requires_the_exact_modifier_mask() {
    // A stroke is matched exactly, so one extra modifier is a different key -- and a key
    // that leads nowhere, which is handed back.
    let table = two_stroke_table();
    let mut sequence = KeySequence::new();
    let opened = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(opened, SequenceDecision::Opened);

    let extra = sequence.offer(&press(KEY_S, CTRL | SHIFT, 10), &table);
    assert_eq!(extra, abandoned(KEY_K, CTRL, 1));
}

#[test]
fn test_key_sequence_offer_abandons_when_the_table_lost_the_sequence() {
    // A table rebuilt under a sequence in flight -- a reload, say -- leaves the machine
    // holding strokes the new table knows nothing about. The stroke is handed back and the
    // drop is reported, rather than the key disappearing.
    let mut sequence = KeySequence::new();
    let opened = sequence.offer(&press(KEY_K, CTRL, 0), &two_stroke_table());
    assert_eq!(opened, SequenceDecision::Opened);

    let empty = SequenceTable::<&'static str>::new();
    let decision = sequence.offer(&press(KEY_S, CTRL, 10), &empty);
    assert_eq!(decision, abandoned(KEY_K, CTRL, 1));
    assert!(!decision.keeps_key());
    assert_eq!(sequence.state(), SequenceState::Idle);
}

#[test]
fn test_key_sequence_offer_is_a_pure_function_of_the_events() {
    // Nothing here reads a clock, a file or the environment, so the same events produce the
    // same decisions every time.
    let table = two_stroke_table();
    let events = [
        press(KEY_K, CTRL, 0),
        press(KEY_S, CTRL, 10),
        press(KEY_K, CTRL, 20),
        press(KEY_F35, 0, 30),
        press(KEY_K, CTRL, 40),
        press(KEY_ESCAPE, 0, 50),
    ];
    let drive = || {
        let mut sequence = KeySequence::new();
        events
            .iter()
            .map(|event| sequence.offer(event, &table))
            .collect::<Vec<_>>()
    };
    assert_eq!(drive(), drive());
    assert_eq!(drive().len(), events.len());
}

#[test]
fn test_sequence_decision_keeps_key_matches_what_the_caller_must_do() {
    let cases = [
        (SequenceDecision::<&'static str>::Pass, false),
        (SequenceDecision::Opened, true),
        (completed("save"), true),
        (SequenceDecision::Cancelled, true),
        (abandoned(KEY_K, CTRL, 2), false),
    ];
    let mut kept = 0usize;
    for (decision, expected) in cases {
        assert_eq!(decision.keeps_key(), expected, "decision {decision:?}");
        if decision.keeps_key() {
            kept += 1;
        }
    }
    assert_eq!(kept, 3, "three of the five answers take the key");
}

#[test]
fn test_key_sequence_reset_drops_the_sequence_in_flight() {
    let table = two_stroke_table();
    let mut sequence = KeySequence::new();
    let opened = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(opened, SequenceDecision::Opened);

    // Focus left the input context: the half-typed sequence belongs to keystrokes that are
    // being typed somewhere else now, so it must not survive the move.
    sequence.reset();
    assert_eq!(sequence.state(), SequenceState::Idle);
    assert_eq!(sequence.leader(), None);

    // The stroke that would have completed it is judged from scratch, so it travels on
    // rather than completing a sequence the user has left behind.
    let after = sequence.offer(&press(KEY_S, CTRL, 10), &table);
    assert_eq!(after, SequenceDecision::Pass);
}

#[test]
fn test_key_sequence_reset_with_nothing_in_flight_leaves_the_machine_usable() {
    // A host resets an input context whether or not anything was typed in it, so the
    // answer has to be "nothing happened" rather than a state the machine cannot leave.
    let table = two_stroke_table();
    let mut sequence = KeySequence::new();
    sequence.reset();
    assert_eq!(sequence.state(), SequenceState::Idle);

    let opened = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(opened, SequenceDecision::Opened);
    sequence.reset();
    let opened_again = sequence.offer(&press(KEY_K, CTRL, 1), &table);
    assert_eq!(opened_again, SequenceDecision::Opened);

    let finished = sequence.offer(&press(KEY_S, CTRL, 2), &table);
    assert_eq!(finished, completed("save"));
    sequence.reset();
    assert_eq!(sequence.state(), SequenceState::Idle);
}

#[test]
fn test_key_sequence_leader_names_the_stroke_that_opened_the_sequence() {
    let mut table = SequenceTable::new();
    let save_all = [
        stroke(KEY_K, CTRL),
        stroke(KEY_S, CTRL),
        stroke(KEY_W, CTRL),
    ];
    assert!(table.bind(&save_all, "save-all").is_ok());

    let mut sequence = KeySequence::new();
    assert_eq!(sequence.leader(), None, "nothing is in flight");

    let first = sequence.offer(&press(KEY_K, CTRL, 0), &table);
    assert_eq!(first, SequenceDecision::Opened);
    assert_eq!(sequence.leader(), Some(stroke(KEY_K, CTRL)));

    // A longer sequence keeps naming its first stroke: a hint names the key the user
    // pressed to enter the sequence, not the stroke they typed last.
    let second = sequence.offer(&press(KEY_S, CTRL, 10), &table);
    assert_eq!(second, SequenceDecision::Opened);
    assert_eq!(sequence.leader(), Some(stroke(KEY_K, CTRL)));

    let third = sequence.offer(&press(KEY_W, CTRL, 20), &table);
    assert_eq!(third, completed("save-all"));
    assert_eq!(sequence.leader(), None, "a finished sequence has no leader");

    let reopened = sequence.offer(&press(KEY_K, CTRL, 30), &table);
    assert_eq!(reopened, SequenceDecision::Opened);
    sequence.reset();
    assert_eq!(sequence.leader(), None, "a dropped sequence has no leader");
}

#[test]
fn test_key_sequence_offer_reads_the_time_from_the_event_and_no_clock() {
    // The same timeline at two absolute offsets. Only the differences between the
    // timestamps may matter, which is what makes the deadline a function of the events
    // rather than of a clock somewhere in the process.
    let table = two_stroke_table();
    let drive = |offset: u32| {
        let mut sequence = KeySequence::new();
        let first = sequence.offer(&press(KEY_K, CTRL, offset), &table);
        let second = sequence.offer(&press(KEY_S, CTRL, offset + 40), &table);
        let late = press(KEY_K, CTRL, offset + 40 + SEQUENCE_TIMEOUT_MS + 1);
        let third = sequence.offer(&late, &table);
        (first, second, third)
    };
    assert_eq!(drive(0), drive(1_000_000));
    let (first, second, third) = drive(0);
    assert_eq!(first, SequenceDecision::<&'static str>::Opened);
    assert_eq!(second, completed("save"));
    assert_eq!(third, SequenceDecision::Opened);
}

#[test]
fn test_sequence_decision_agrees_with_the_walks_vocabulary() {
    // What travels from this machine to the host's `filterAndAccept` is the bus's
    // `Consumed`, and the two vocabularies have to agree on which strokes stop at the
    // plugin: one that opened a sequence is ours while nothing has been executed, one that
    // completed or cancelled a sequence is ours outright, and one that led nowhere is the
    // application's.
    let pass: SequenceDecision<&'static str> = SequenceDecision::Pass;
    assert_eq!(arbitrate_sequence(&pass), Consumed::Ignored);

    let opened: SequenceDecision<&'static str> = SequenceDecision::Opened;
    assert_eq!(arbitrate_sequence(&opened), Consumed::ChainPending);

    let finished = completed("save");
    assert_eq!(arbitrate_sequence(&finished), Consumed::Consumed);

    let cancelled: SequenceDecision<&'static str> = SequenceDecision::Cancelled;
    assert_eq!(arbitrate_sequence(&cancelled), Consumed::Consumed);

    let dropped = abandoned(KEY_K, CTRL, 2);
    assert_eq!(arbitrate_sequence(&dropped), Consumed::Ignored);

    // The two answers are one answer, and this is where they are compared: the machine's
    // `keeps_key` says whether the caller must stop the stroke here, and `Ignored` is the
    // walk's only answer that lets it travel on. A stroke the machine took must never come
    // back out of the walk as the application's, and a stroke it handed back must never be
    // stopped by the walk.
    for decision in [pass, opened, finished, cancelled, dropped] {
        let walk = arbitrate_sequence(&decision);
        assert_eq!(
            decision.keeps_key(),
            !matches!(walk, Consumed::Ignored),
            "decision {decision:?} walks as {walk:?}"
        );
    }
}
