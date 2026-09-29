//! Tests for the user-interface role.
//!
//! The tests that read or write the process-wide slots take a lock of their own, so they
//! stay independent of the order the suite runs in and of nextest's one-process-per-test
//! scheduling. The rest drive the pure policy functions and the mirror directly.

use std::sync::{Mutex, MutexGuard};

use crate::addon::candidate_window_ready;
use crate::ffi::FcitxCursorRect;

use super::availability::plan_availability;
use super::panel::count_fields;
use super::takeover::{TakeoverPlan, apply_takeover, plan_takeover};
use super::*;

/// An input-context id no other test uses, so the process-wide slots stay
/// independent of the order the tests run in.
const TEST_IC: u64 = 0x5100_0000_0000_0001;

/// Serialises the tests that read or write the process-wide slots.
///
/// Those slots exist because the host has one panel and one caret, so two tests
/// that drive them would otherwise race whenever the suite runs in threads instead
/// of in nextest's one process per test.
static SLOTS: Mutex<()> = Mutex::new(());

/// Takes the slot lock, ignoring a lock poisoned by a failed test: the slots hold
/// plain values, and every test that takes the lock sets up what it asserts on.
fn lock_slots() -> MutexGuard<'static, ()> {
    match SLOTS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// A panel with two candidates, consistent by construction.
fn sample_panel<'a>(preedit: &'a str, candidates: &'a str) -> PanelUpdate<'a> {
    PanelUpdate {
        ic: TEST_IC,
        preedit,
        caret: preedit.len() as u32,
        candidates,
        candidate_count: count_fields(candidates) as u32,
        cursor_index: 0,
        page: 0,
        total_pages: 1,
        page_size: 2,
    }
}

/// A caret rectangle as the host reports one.
fn caret(x: i32, y: i32) -> FcitxCursorRect {
    FcitxCursorRect {
        x,
        y,
        w: 2,
        h: 20,
        scale: 1.0,
    }
}

#[test]
fn test_plan_availability_requires_the_window_and_the_backend() {
    assert!(plan_availability(true, true));
    assert!(!plan_availability(false, true), "no window");
    assert!(!plan_availability(true, false), "no backend");
    assert!(!plan_availability(false, false));
}

#[test]
fn test_plan_takeover_reports_the_missing_backend_before_the_missing_window() {
    assert_eq!(
        plan_takeover(false, false, false),
        TakeoverPlan::Unsupported,
        "a session without a backend can never host the window"
    );
    assert_eq!(plan_takeover(false, true, false), TakeoverPlan::NotReady);
}

#[test]
fn test_plan_takeover_asks_the_host_only_when_everything_is_ready() {
    assert_eq!(plan_takeover(true, true, false), TakeoverPlan::Ask);
    assert_eq!(
        plan_takeover(true, true, true),
        TakeoverPlan::Declined,
        "a refusal is not asked again"
    );
}

#[test]
fn test_apply_takeover_maps_every_plan_to_its_outcome() {
    assert_eq!(
        apply_takeover(TakeoverPlan::NotReady),
        TakeoverOutcome::NotReady
    );
    assert_eq!(
        apply_takeover(TakeoverPlan::Unsupported),
        TakeoverOutcome::Unsupported
    );
    let declined = apply_takeover(TakeoverPlan::Declined);
    assert!(
        matches!(declined, TakeoverOutcome::Declined { .. }),
        "a refusal is reported as one: {declined:?}"
    );
}

#[test]
fn test_register_takeover_never_asks_without_a_candidate_window() {
    let _slots = lock_slots();
    // No window is up in this process — the background start-up has not reported
    // ready — so whatever the platform probe has answered, the host must not be
    // asked to switch to a user interface that cannot draw.
    assert!(!candidate_window_ready());
    let outcome = register_takeover();
    assert!(
        matches!(
            outcome,
            TakeoverOutcome::NotReady | TakeoverOutcome::Unsupported
        ),
        "expected a skip, got {outcome:?}"
    );
}

#[test]
fn test_window_backend_availability_round_trips_and_gates_availability() {
    let _slots = lock_slots();
    set_window_backend_available(true);
    assert!(window_backend_available());
    // The window is never ready in this process, so the answer stays `false` even
    // with a backend: a backend without a window is still no window, and the host
    // must keep drawing its own candidates.
    assert!(!is_available());
    set_window_backend_available(false);
    assert!(
        !window_backend_available(),
        "the probe's answer is not sticky"
    );
    assert!(!is_available());
}

#[test]
fn test_on_host_suspend_and_resume_follow_the_host_state() {
    let _slots = lock_slots();
    on_host_resume();
    assert!(!is_host_ui_suspended());
    on_host_suspend();
    assert!(is_host_ui_suspended(), "suspend must be observable");
    assert!(
        plan_availability(true, true),
        "suspension is not part of the availability answer: the host re-evaluates \
         availability before it resumes, so a suspended user interface that answered \
         false could never be chosen again"
    );
    on_host_resume();
    assert!(!is_host_ui_suspended(), "resume must clear the suspension");
}

#[test]
fn test_on_input_panel_update_stores_the_host_panel_and_draws_nothing() {
    let _slots = lock_slots();
    let panel = sample_panel("ni hao", "ni\nhao");
    assert!(
        !on_input_panel_update(panel),
        "a panel update is not a drawing instruction"
    );
    let mirror = panel_mirror();
    assert!(mirror.is_some(), "the panel must be captured");
    if let Some(mirror) = mirror {
        assert_eq!(mirror.ic, TEST_IC);
        assert_eq!(mirror.preedit, "ni hao");
        assert_eq!(mirror.caret, 6);
        assert_eq!(mirror.candidates, "ni\nhao");
        assert_eq!(mirror.candidate_count, 2);
        assert_eq!(mirror.cursor_index, 0);
        assert_eq!(mirror.page_size, 2);
    }
}

#[test]
fn test_on_input_panel_update_rejects_a_candidate_count_that_disagrees_with_the_buffer() {
    let _slots = lock_slots();
    let good = sample_panel("ni", "ni\nhao");
    assert!(!on_input_panel_update(good));
    let mut broken = sample_panel("ni", "ni\nhao");
    broken.candidate_count = 3;
    assert!(
        !on_input_panel_update(broken),
        "a count that disagrees with the buffer is a broken host"
    );
    let kept = panel_mirror();
    assert!(kept.is_some());
    if let Some(kept) = kept {
        assert_eq!(
            kept.candidate_count, 2,
            "a rejected snapshot must not replace the mirror"
        );
    }
    let mut stray_highlight = sample_panel("ni", "ni\nhao");
    stray_highlight.cursor_index = 2;
    assert!(
        !on_input_panel_update(stray_highlight),
        "a highlight outside the list is a broken host"
    );
}

#[test]
fn test_on_input_panel_update_accepts_a_panel_with_no_candidates() {
    let _slots = lock_slots();
    let empty = PanelUpdate {
        ic: TEST_IC,
        preedit: "",
        caret: 0,
        candidates: "",
        candidate_count: 0,
        cursor_index: -1,
        page: 0,
        total_pages: 0,
        page_size: 0,
    };
    assert!(!on_input_panel_update(empty));
    let mirror = panel_mirror();
    assert!(mirror.is_some());
    if let Some(mirror) = mirror {
        assert!(mirror.preedit.is_empty());
        assert!(mirror.candidates.is_empty());
        assert_eq!(mirror.candidate_count, 0);
        assert_eq!(mirror.cursor_index, -1);
    }
}

#[test]
fn test_panel_mirror_refill_reuses_the_buffers_it_already_has() {
    let mut mirror = PanelMirror::default();
    mirror.refill(sample_panel("ni hao ma", "ni\nhao\nma"));
    let preedit_buffer = mirror.preedit.as_ptr();
    let candidates_buffer = mirror.candidates.as_ptr();
    mirror.refill(sample_panel("ni", "ni\nhao"));
    assert_eq!(mirror.preedit, "ni", "the contents must be replaced");
    assert_eq!(mirror.candidates, "ni\nhao");
    assert_eq!(
        mirror.preedit.as_ptr(),
        preedit_buffer,
        "a shorter panel must keep the buffer instead of allocating a new one"
    );
    assert_eq!(mirror.candidates.as_ptr(), candidates_buffer);
    assert_eq!(mirror.caret, 2, "and the numbers must follow the panel");
}

#[test]
fn test_on_cursor_rect_keeps_the_rect_of_its_own_context() {
    let _slots = lock_slots();
    let other_ic = TEST_IC + 1;
    assert_eq!(latest_cursor_rect(other_ic), None, "nothing reported yet");
    on_cursor_rect(TEST_IC, caret(300, 400));
    assert_eq!(latest_cursor_rect(TEST_IC), Some(caret(300, 400)));
    assert_eq!(
        latest_cursor_rect(other_ic),
        None,
        "another context must never inherit this caret"
    );
    on_cursor_rect(other_ic, caret(10, 20));
    assert_eq!(latest_cursor_rect(other_ic), Some(caret(10, 20)));
    assert_eq!(
        latest_cursor_rect(TEST_IC),
        Some(caret(300, 400)),
        "and must not overwrite it either"
    );
}

#[test]
fn test_count_fields_counts_an_empty_buffer_as_none() {
    assert_eq!(count_fields(""), 0);
    assert_eq!(count_fields("a"), 1);
    assert_eq!(count_fields("a\nb"), 2);
    assert_eq!(count_fields("a\n\nb"), 3);
}

#[test]
fn test_takeover_outcome_diagnostics_carry_the_documented_codes() {
    let active = TakeoverOutcome::Active {
        previous_ui: Some(String::from("classicui")),
    };
    assert!(active.is_active());
    let line = active.diagnostic();
    assert!(line.starts_with("ui/takeover/active: previous=classicui"));
    let no_previous = TakeoverOutcome::Active { previous_ui: None };
    assert!(no_previous.diagnostic().ends_with("previous=none"));
    let not_ready = TakeoverOutcome::NotReady.diagnostic();
    assert_eq!(
        not_ready,
        "ui/not-ready: the takeover waits for the candidate window"
    );
    let unsupported = TakeoverOutcome::Unsupported.diagnostic();
    assert!(unsupported.starts_with("platform/compositor/unsupported"));
    let not_registered = TakeoverOutcome::NotRegistered.diagnostic();
    assert!(not_registered.starts_with("ui/takeover/not-registered"));
    let unavailable = TakeoverOutcome::Unavailable.diagnostic();
    assert!(unavailable.starts_with("ffi/host-not-linked"));
    let declined = TakeoverOutcome::Declined {
        active_ui: Some(String::from("classicui")),
    };
    assert!(!declined.is_active());
    let line = declined.diagnostic();
    assert!(line.starts_with("ui/takeover/declined: classicui"));
}

#[cfg(not(fcitx5_host))]
#[test]
fn test_apply_takeover_reports_unavailable_without_the_host_abi() {
    // Without the host ABI there is nothing to ask, so the branch that talks to the
    // host has to answer with the honest "no host" outcome rather than a refusal
    // the host never gave.
    assert_eq!(
        apply_takeover(TakeoverPlan::Ask),
        TakeoverOutcome::Unavailable
    );
}
