//! The modifier mask check, the hold machine, and the one release edge the walk answers.
//!
//! The mask tests pin the run-time check against the constant the build was compiled with.
//! The hold tests drive the whole gesture — a press, whatever happens while the key is
//! down, the release — with every timestamp supplied by the test, so nothing here sleeps
//! and nothing reads a clock. The last group drives the same gesture through `Dispatcher`,
//! which is where the release that ends a hold is separated from the releases the
//! application owns.
//!
//! The walk's own tests live beside the bus they exercise; these are the ones whose subject
//! is the hold, and the dispatcher is only what carries it.

use ime_core::state::{Session, SessionState};
use ime_types::KeyAction;

use super::{
    HOLD_THRESHOLD_MS, HoldOutcome, MODIFIER_MASK_MISMATCH_CODE, ModifierHold, ModifierKey,
    check_modifier_mask_with,
};
use crate::engine::*;

/// `FcitxKey_F35`, a keysym no row of the routing table names.
const KEY_F35: u32 = 0xffbe;

/// `FcitxKey_Control_L`, the modifier next to `Shift_L` that has no hold of its own.
const KEY_CONTROL_L: u32 = 0xffe3;

/// The letters of `nihao`, as the keysyms the host delivers.
const NIHAO: [u32; 5] = [0x6e, 0x69, 0x68, 0x61, 0x6f];

/// A key press of `sym` with `state` held, at `time_ms`.
fn press_at(sym: u32, state: u32, time_ms: u32) -> KeyEvent {
    KeyEvent {
        sym,
        state,
        is_release: false,
        time_ms,
    }
}

/// A key release of `sym` with `state` held, at `time_ms`.
fn release_at(sym: u32, state: u32, time_ms: u32) -> KeyEvent {
    KeyEvent {
        sym,
        state,
        is_release: true,
        time_ms,
    }
}

/// A session in `state`, for the walk to read.
fn session_in(state: SessionState) -> Session {
    let mut session = Session::new();
    session.state = state;
    session
}

// ── The mask check ───────────────────────────────────────────────────────────────

#[test]
fn test_check_modifier_mask_accepts_the_compiled_mask_silently() {
    let mut reported: Vec<String> = Vec::new();
    let result = check_modifier_mask_with(MODIFIER_MASK, |code| {
        reported.push(String::from(code));
    });
    assert!(result.is_ok(), "the compiled mask is the one the host has");
    assert!(
        reported.is_empty(),
        "a matching host must record nothing: {reported:?}"
    );
}

#[test]
fn test_check_modifier_mask_reports_a_host_that_added_a_bit() {
    let mut reported: Vec<String> = Vec::new();
    let result = check_modifier_mask_with(MODIFIER_MASK | (1 << 1), |code| {
        reported.push(String::from(code));
    });
    assert_eq!(reported, vec![String::from(MODIFIER_MASK_MISMATCH_CODE)]);
    let error = result.expect_err("a renumbered host is an error");
    assert!(
        error
            .to_string()
            .starts_with("platform/fcitx5/version-mismatch"),
        "the error renders as a frozen code: {error}"
    );
}

#[test]
fn test_check_modifier_mask_reports_a_host_that_dropped_a_bit() {
    // Bit 0 is Shift, the bit of the mask the composing keymap reads most often.
    let mut reported: Vec<String> = Vec::new();
    let result = check_modifier_mask_with(MODIFIER_MASK & !(1 << 0), |code| {
        reported.push(String::from(code));
    });
    assert_eq!(reported, vec![String::from(MODIFIER_MASK_MISMATCH_CODE)]);
    assert!(result.is_err(), "a mask without Shift is not this build's");
}

#[test]
fn test_check_modifier_mask_reports_an_empty_host_mask() {
    // The boundary: a host that reports nothing at all is the furthest a host can be from
    // this build, and it must be reported rather than read as "no modifiers".
    let mut reported: Vec<String> = Vec::new();
    let result = check_modifier_mask_with(0, |code| {
        reported.push(String::from(code));
    });
    assert_eq!(reported, vec![String::from(MODIFIER_MASK_MISMATCH_CODE)]);
    assert!(result.is_err());
}

// ── The modifier key ─────────────────────────────────────────────────────────────

#[test]
fn test_modifier_key_from_sym_names_both_shift_keys_and_nothing_else() {
    let shift = Some(ModifierKey::Shift);
    assert_eq!(ModifierKey::from_sym(KEY_SHIFT_L), shift);
    assert_eq!(ModifierKey::from_sym(KEY_SHIFT_R), shift);
    // The boundaries: the modifier next to Shift, a letter, the keysym the walk's sweeps
    // use as "names no row", and the lowest keysym there is.
    for sym in [KEY_CONTROL_L, KEY_A, KEY_F35, 0] {
        assert_eq!(ModifierKey::from_sym(sym), None, "sym {sym:#06x}");
    }
}

#[test]
fn test_modifier_key_from_sym_agrees_with_is_shift_press() {
    // The same predicate written twice: once over a keysym, once over a whole event. The
    // two must never drift, or the hold would track a key the walk hands to the
    // application.
    let syms = [
        0,
        KEY_A,
        KEY_SPACE,
        KEY_ESCAPE,
        KEY_SHIFT_L,
        KEY_SHIFT_R,
        KEY_CONTROL_L,
        0xffe9,
        0xffff,
    ];
    for sym in syms {
        for state in [0, SHIFT, CTRL, CTRL | SHIFT] {
            for is_release in [false, true] {
                let event = KeyEvent {
                    sym,
                    state,
                    is_release,
                    time_ms: 0,
                };
                assert_eq!(
                    ModifierKey::from_sym(sym).is_some(),
                    is_shift_press(&event.as_host_event()),
                    "sym {sym:#06x} state {state:#x} release {is_release}"
                );
            }
        }
    }
}

// ── The hold machine ─────────────────────────────────────────────────────────────

#[test]
fn test_modifier_hold_new_and_default_are_the_same_empty_hold() {
    assert_eq!(ModifierHold::new(), ModifierHold::default());
}

#[test]
fn test_modifier_hold_without_a_hold_answers_every_release_with_nothing() {
    // The boundary: the host delivers a release for every key, so this is asked of keys
    // that never armed anything.
    let mut hold = ModifierHold::new();
    let shift = Some(ModifierKey::Shift);
    assert_eq!(hold.release(shift, 0), HoldOutcome::Nothing);
    assert_eq!(hold.release(None, 1_000), HoldOutcome::Nothing);
    // And still nothing at the far end of the clock.
    assert_eq!(hold.release(None, u32::MAX), HoldOutcome::Nothing);
}

#[test]
fn test_modifier_hold_release_within_the_threshold_restores_the_interrupted_state() {
    let mut hold = ModifierHold::new();
    hold.arm(ModifierKey::Shift, 1_000, true);
    let shift = Some(ModifierKey::Shift);
    let restored = HoldOutcome::Restore { was_enabled: true };
    assert_eq!(hold.release(shift, 1_000 + HOLD_THRESHOLD_MS - 1), restored);
    // The hold is over: a second release of the same key has nothing to answer.
    assert_eq!(hold.release(shift, 2_000), HoldOutcome::Nothing);
}

#[test]
fn test_modifier_hold_release_at_the_threshold_is_a_long_press() {
    // The boundary from both sides: one millisecond short of the threshold is a mode
    // switch, and the threshold itself is a long press.
    let shift = Some(ModifierKey::Shift);

    let mut short = ModifierHold::new();
    short.arm(ModifierKey::Shift, 0, false);
    let restored = HoldOutcome::Restore { was_enabled: false };
    assert_eq!(short.release(shift, HOLD_THRESHOLD_MS - 1), restored);

    let mut exact = ModifierHold::new();
    exact.arm(ModifierKey::Shift, 0, false);
    let long = HoldOutcome::LongPress {
        held_ms: HOLD_THRESHOLD_MS,
    };
    assert_eq!(exact.release(shift, HOLD_THRESHOLD_MS), long);
}

#[test]
fn test_modifier_hold_release_of_a_key_that_is_not_the_held_modifier_keeps_the_hold() {
    let mut hold = ModifierHold::new();
    hold.arm(ModifierKey::Shift, 100, true);
    // A letter typed with Shift held: the release is not the modifier's, so the hold is
    // still waiting for its own.
    assert_eq!(hold.release(None, 120), HoldOutcome::Nothing);
    let restored = HoldOutcome::Restore { was_enabled: true };
    assert_eq!(hold.release(Some(ModifierKey::Shift), 150), restored);
}

#[test]
fn test_modifier_hold_arm_is_idempotent_while_the_key_repeats() {
    // The frontend repeats a press while a key is held down. A repeat must not restart the
    // hold, or the long press would be measured from the last repeat and would never be
    // reached.
    let mut hold = ModifierHold::new();
    hold.arm(ModifierKey::Shift, 0, true);
    hold.arm(ModifierKey::Shift, 90, false);
    hold.arm(ModifierKey::Shift, 180, false);
    let long = HoldOutcome::LongPress {
        held_ms: HOLD_THRESHOLD_MS,
    };
    assert_eq!(
        hold.release(Some(ModifierKey::Shift), HOLD_THRESHOLD_MS),
        long
    );
}

#[test]
fn test_modifier_hold_mark_used_turns_the_release_into_a_no_op() {
    let mut hold = ModifierHold::new();
    hold.arm(ModifierKey::Shift, 0, true);
    hold.mark_used();
    assert_eq!(
        hold.release(Some(ModifierKey::Shift), HOLD_THRESHOLD_MS * 10),
        HoldOutcome::Nothing,
        "a modifier the user typed with must not also switch the mode"
    );
}

#[test]
fn test_modifier_hold_mark_used_without_a_hold_changes_nothing() {
    // The boundary: the mark is about a hold, so with none there is nothing to mark and
    // the next hold is unaffected by the stray call.
    let mut hold = ModifierHold::new();
    hold.mark_used();
    assert_eq!(hold.release(None, 10), HoldOutcome::Nothing);

    hold.arm(ModifierKey::Shift, 20, true);
    let restored = HoldOutcome::Restore { was_enabled: true };
    assert_eq!(
        hold.release(Some(ModifierKey::Shift), 21),
        restored,
        "the stray mark did not reach the new hold"
    );
}

#[test]
fn test_modifier_hold_release_before_the_press_does_not_panic() {
    // A machine suspended and resumed between the press and the release hands back
    // timestamps that are not ordered. The difference saturates rather than underflowing,
    // because the caller is a host callback that must not unwind.
    let mut hold = ModifierHold::new();
    hold.arm(ModifierKey::Shift, 5_000, true);
    let restored = HoldOutcome::Restore { was_enabled: true };
    assert_eq!(hold.release(Some(ModifierKey::Shift), 4_000), restored);
}

#[test]
fn test_modifier_hold_clear_forgets_the_hold_and_what_was_typed_with_it() {
    let mut hold = ModifierHold::new();
    hold.arm(ModifierKey::Shift, 0, true);
    hold.mark_used();
    hold.clear();
    assert_eq!(
        hold.release(Some(ModifierKey::Shift), HOLD_THRESHOLD_MS * 2),
        HoldOutcome::Nothing,
        "a cleared hold has no release left to answer"
    );
    // And the next press starts a fresh hold, with none of the old one's marks.
    hold.arm(ModifierKey::Shift, 10_000, false);
    let restored = HoldOutcome::Restore { was_enabled: false };
    assert_eq!(hold.release(Some(ModifierKey::Shift), 10_001), restored);
}

// ── The release edge the walk answers ────────────────────────────────────────────

#[test]
fn test_dispatch_never_claims_a_release_of_a_modifier_it_never_armed() {
    let session = session_in(SessionState::Composing);
    let view = SessionView::new(&session);
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    for sym in [KEY_SHIFT_L, KEY_SHIFT_R, KEY_A, KEY_SPACE, KEY_F35] {
        let up = release_at(sym, 0, 10);
        assert_eq!(
            dispatcher.dispatch(&up, &view),
            Consumed::Ignored,
            "sym {sym:#06x} is the application's key-up"
        );
    }
    assert_eq!(
        dispatcher.take_hold_outcome(),
        None,
        "a release that ended no hold leaves nothing behind"
    );
}

#[test]
fn test_dispatch_consumes_the_release_of_the_modifier_it_armed() {
    let session = session_in(SessionState::Idle);
    let view = SessionView::new(&session);
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    let down = press_at(KEY_SHIFT_L, SHIFT, 1_000);
    let up = release_at(KEY_SHIFT_L, 0, 1_100);

    assert!(
        dispatcher.arm_hold(&down, true),
        "a modifier press is what the hold tracks"
    );
    assert_eq!(dispatcher.dispatch(&up, &view), Consumed::Consumed);
    let restored = HoldOutcome::Restore { was_enabled: true };
    assert_eq!(dispatcher.take_hold_outcome(), Some(restored));
    assert_eq!(
        dispatcher.take_hold_outcome(),
        None,
        "one release is acted on once"
    );
}

#[test]
fn test_dispatch_arms_no_hold_for_an_event_that_is_not_a_modifier_press() {
    let session = session_in(SessionState::Idle);
    let view = SessionView::new(&session);
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    let letter = press_at(KEY_A, 0, 0);
    let chord = press_at(KEY_SPACE, SHIFT, 0);
    let up = release_at(KEY_SHIFT_L, SHIFT, 0);

    assert!(!dispatcher.arm_hold(&letter, true));
    assert!(!dispatcher.arm_hold(&chord, true));
    assert!(!dispatcher.arm_hold(&up, true), "a release is not a press");
    // Nothing was armed, so the matching release is still the application's.
    let matching = release_at(KEY_SHIFT_L, 0, 5);
    assert_eq!(
        dispatcher.dispatch(&matching, &view),
        Consumed::Ignored,
        "nothing was armed, so the release is the application's"
    );
    assert_eq!(dispatcher.take_hold_outcome(), None);
}

#[test]
fn test_dispatch_does_not_mark_the_hold_used_for_a_key_the_walk_declines() {
    // The judgement the machine rests on: only a key the plugin acted on turns the
    // modifier into a typed key. A key that fell through every layer changed nothing, so
    // the hold is still a hold and the release is still a long press.
    let session = session_in(SessionState::Idle);
    let view = SessionView::new(&session);
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    dispatcher.arm_hold(&press_at(KEY_SHIFT_L, SHIFT, 0), true);

    // No layer names this key, so it changed nothing.
    let unnamed = press_at(KEY_F35, 0, 10);
    assert_eq!(dispatcher.dispatch(&unnamed, &view), Consumed::Ignored);

    let up = release_at(KEY_SHIFT_L, 0, HOLD_THRESHOLD_MS + 10);
    assert_eq!(dispatcher.dispatch(&up, &view), Consumed::Consumed);
    let long = HoldOutcome::LongPress {
        held_ms: HOLD_THRESHOLD_MS + 10,
    };
    assert_eq!(dispatcher.take_hold_outcome(), Some(long));
}

#[test]
fn test_dispatch_marks_the_hold_used_for_a_key_the_walk_claims() {
    // The other half, and the assertion behind "hold Shift, type a word, and the mode does
    // not move": every letter the composition takes is a key typed with the modifier, so
    // the release that follows is a no-op rather than a mode switch.
    let session = session_in(SessionState::Composing);
    let view = SessionView::new(&session);
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    dispatcher.arm_hold(&press_at(KEY_SHIFT_L, SHIFT, 0), true);

    let mut at_ms = 10;
    for sym in NIHAO {
        let typed = press_at(sym, SHIFT, at_ms);
        assert_eq!(
            dispatcher.dispatch(&typed, &view),
            Consumed::Consumed,
            "sym {sym:#06x} is a letter of the composition"
        );
        assert!(matches!(
            dispatcher.action_for(&typed),
            KeyAction::InputChar(_)
        ));
        at_ms += 10;
    }

    let up = release_at(KEY_SHIFT_L, 0, at_ms);
    assert_eq!(
        dispatcher.dispatch(&up, &view),
        Consumed::Ignored,
        "the modifier was part of a typed word, so its release changes nothing"
    );
    assert_eq!(dispatcher.take_hold_outcome(), None);
}

#[test]
fn test_dispatch_shift_space_switches_full_width_and_never_the_language() {
    // `Shift` held with the space bar is the full-width chord. The modifier's own press is
    // never a mode switch — the walk answers it with `Ignored` — so the chord produces one
    // action and not two.
    let session = session_in(SessionState::Composing);
    let view = SessionView::new(&session);
    let mut dispatcher = Dispatcher::new(KeyBindings::default());
    let modifier = press_at(KEY_SHIFT_L, SHIFT, 0);
    let space = press_at(KEY_SPACE, SHIFT, 5);

    assert_eq!(
        dispatcher.dispatch(&modifier, &view),
        Consumed::Ignored,
        "the modifier press belongs to the application"
    );
    assert_eq!(dispatcher.dispatch(&space, &view), Consumed::Consumed);
    assert_eq!(
        dispatcher.action_for(&space),
        KeyAction::ToggleFullWidth,
        "the space bar's action, and not the language switch"
    );
}

#[test]
fn test_clear_hold_forgets_the_hold_and_anything_waiting_on_it() {
    let session = session_in(SessionState::Idle);
    let view = SessionView::new(&session);
    let mut dispatcher = Dispatcher::new(KeyBindings::default());

    // A hold whose release was answered leaves its outcome waiting for the caller; losing
    // the context means nobody will ever act on it.
    dispatcher.arm_hold(&press_at(KEY_SHIFT_L, SHIFT, 0), true);
    let up = release_at(KEY_SHIFT_L, 0, 10);
    assert_eq!(dispatcher.dispatch(&up, &view), Consumed::Consumed);
    dispatcher.clear_hold();
    assert_eq!(
        dispatcher.take_hold_outcome(),
        None,
        "the wait ends with the hold"
    );

    // And a hold cleared before its release has no release left to answer.
    dispatcher.arm_hold(&press_at(KEY_SHIFT_L, SHIFT, 20), true);
    dispatcher.clear_hold();
    let late = release_at(KEY_SHIFT_L, 0, 30);
    assert_eq!(
        dispatcher.dispatch(&late, &view),
        Consumed::Ignored,
        "the hold was cleared, so there is no release left to answer"
    );
    assert_eq!(dispatcher.take_hold_outcome(), None);
}
