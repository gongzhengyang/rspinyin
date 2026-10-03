//! Tests for the FFI infrastructure: the panic guard, the crash channel and the
//! signal registrar.
//!
//! The throttle is process-wide, so two tests driving the real `emit_diagnostic`
//! would suppress each other; the tests drive `emit_through` with a table and a clock
//! of their own instead, which also makes the window boundary reachable without
//! sleeping. The registrar tests arm real handlers, and `nextest` gives every test a
//! process of its own, so the arming and the process-wide slots each test touches are
//! isolated.
//!
//! This file duplicates `ime-fcitx5/src/ffi/tests.rs` in shape, for the reason the
//! module it tests is duplicated: the two addons share no code.

use std::os::unix::fs::PermissionsExt as _;
use std::panic::panic_any;
use std::path::PathBuf;

use super::*;

/// A code a caller can reach once per frame, used to pin the throttle's shape.
const REPEATED_CODE: &str = "ui/not-ready";

/// A second code, so a test can show that one code's window does not silence another.
const OTHER_CODE: &str = "ffi/invalid-panel-snapshot";

/// A table, a clock and a sink the test owns.
///
/// The process-wide throttle would make two tests suppress each other, and the real
/// clock would put the window boundary out of reach without sleeping.
struct Harness {
    /// The table under test.
    throttle: Mutex<Throttle>,
    /// Every line the sink was handed.
    lines: Vec<String>,
}

impl Harness {
    /// An empty table and an empty log.
    fn new() -> Self {
        Self {
            throttle: Mutex::new(Throttle::new()),
            lines: Vec::new(),
        }
    }

    /// Drives one call at `now`, recording what it wrote.
    fn emit(&mut self, code: &str, now: Instant) {
        let Self { throttle, lines } = self;
        emit_through(throttle, code, now, |line: &str| {
            lines.push(line.to_owned());
        });
    }

    /// The lines written so far.
    fn lines(&self) -> Vec<String> {
        self.lines.clone()
    }

    /// How many codes the table is tracking.
    fn tracked(&self) -> usize {
        match self.throttle.lock() {
            Ok(table) => table.tracked(),
            Err(poisoned) => poisoned.into_inner().tracked(),
        }
    }
}

#[test]
fn test_catch_ffi_passes_a_successful_body_through() {
    assert!(matches!(catch_ffi(|| 7), Ok(7)));
}

#[test]
fn test_emit_diagnostic_does_not_panic_on_an_empty_code() {
    // The crash channel is reached from panic paths, so it must never be the
    // thing that fails; an empty code is the degenerate input.
    let outcome = catch_unwind(|| emit_diagnostic(""));
    assert!(outcome.is_ok(), "the crash channel must not panic");
}

#[test]
fn test_emit_through_writes_once_for_a_hundred_calls_inside_the_window() {
    let mut harness = Harness::new();
    let start = Instant::now();
    for call in 0..100 {
        harness.emit(REPEATED_CODE, start + Duration::from_millis(call));
    }
    assert_eq!(
        harness.lines(),
        [format!("rspinyin: {REPEATED_CODE}")],
        "a hundred calls inside one window must cost one write, not a hundred"
    );
    assert_eq!(
        harness.tracked(),
        1,
        "the suppressed path must not add a slot: a table that grew per call would \
         mean it allocated"
    );
}

#[test]
fn test_emit_through_reports_the_repeats_its_window_swallowed() {
    let mut harness = Harness::new();
    let start = Instant::now();
    harness.emit(REPEATED_CODE, start);
    for call in 1..100 {
        harness.emit(REPEATED_CODE, start + Duration::from_millis(call));
    }
    harness.emit(REPEATED_CODE, start + THROTTLE_WINDOW);
    assert_eq!(
        harness.lines(),
        [
            format!("rspinyin: {REPEATED_CODE}"),
            format!(
                "rspinyin: {REPEATED_CODE} (suppressed 99 repeats in {}ms)",
                THROTTLE_WINDOW.as_millis()
            ),
        ],
        "the line after the window must carry what the window swallowed"
    );
}

#[test]
fn test_emit_through_does_not_suppress_a_different_code() {
    let mut harness = Harness::new();
    let start = Instant::now();
    harness.emit(REPEATED_CODE, start);
    harness.emit(OTHER_CODE, start + Duration::from_millis(1));
    assert_eq!(
        harness.lines(),
        [
            format!("rspinyin: {REPEATED_CODE}"),
            format!("rspinyin: {OTHER_CODE}"),
        ],
        "one code's window must not silence another"
    );
    assert_eq!(harness.tracked(), 2, "each code needs a slot of its own");
}

#[test]
fn test_emit_through_writes_again_once_the_window_has_passed() {
    let mut harness = Harness::new();
    let start = Instant::now();
    harness.emit(REPEATED_CODE, start);
    harness.emit(REPEATED_CODE, start + THROTTLE_WINDOW);
    assert_eq!(
        harness.lines(),
        [
            format!("rspinyin: {REPEATED_CODE}"),
            format!("rspinyin: {REPEATED_CODE}"),
        ],
        "a code with nothing suppressed writes its line unchanged"
    );
}

#[test]
fn test_emit_through_writes_a_code_the_table_cannot_track() {
    // Fixtures, not codes the plugin emits: the table is filled past its capacity so
    // that the untracked branch is reached. What has to hold there is that it fails
    // towards writing -- a diagnostic the table cannot count is still a diagnostic.
    let codes: Vec<String> = (0..THROTTLE_SLOTS + 8)
        .map(|index| format!("test/fixture/{index}"))
        .collect();
    let mut harness = Harness::new();
    let start = Instant::now();
    for (index, code) in codes.iter().enumerate() {
        harness.emit(code, start + Duration::from_micros(index as u64));
    }
    let expected: Vec<String> = codes
        .iter()
        .map(|code| format!("rspinyin: {code}"))
        .collect();
    assert_eq!(
        harness.lines(),
        expected,
        "every distinct code must reach the sink, tracked or not"
    );
    assert_eq!(
        harness.tracked(),
        THROTTLE_SLOTS,
        "the table must stop growing at its capacity"
    );
}

#[test]
fn test_code_hash_matches_the_published_fnv1a_vector() {
    // The hash is only a throttle key, but changing the constants would move every
    // window boundary at once, so the algorithm is pinned against its published
    // vector rather than against itself.
    assert_eq!(code_hash(""), FNV_OFFSET_BASIS);
    assert_eq!(code_hash("a"), 0xaf63_dc4c_8601_ec8c);
    assert_ne!(
        code_hash(REPEATED_CODE),
        code_hash(OTHER_CODE),
        "two codes a session can hit together must not share a window"
    );
}

#[test]
fn test_guard_ffi_writes_exactly_one_line_for_an_injected_panic() {
    // The reverse of the defect the throttle exists for: the guard reports a contained
    // panic through the throttled channel alone, so an entry point that panics on every
    // key costs one line per window, never one line per channel it also touches. The
    // table is process-wide, so the test starts from an empty one, and the first panic
    // is the line the window allows: the slot for the code exists, and nothing is
    // behind it yet.
    THROTTLE
        .lock()
        .expect("the throttle is never poisoned")
        .slots = [const { None }; THROTTLE_SLOTS];
    let result = guard_ffi(false, || panic_any("the candidate window blew up"));
    assert!(!result, "a panicking body must yield the fallback value");
    let suppressed = THROTTLE
        .lock()
        .map(|mut table| {
            table
                .slots
                .iter_mut()
                .flatten()
                .find(|slot| slot.code_hash == code_hash("ffi/panic: the candidate window blew up"))
                .map(|slot| slot.suppressed)
        })
        .expect("the throttle is never poisoned");
    assert_eq!(
        suppressed,
        Some(0),
        "the first injected panic is the one line the window allows"
    );
}

#[test]
fn test_guard_ffi_throttles_the_failure_line_of_repeated_panics() {
    // The guard's failure line goes through the process-wide throttle. The sink here
    // is stderr, which a test cannot read back, so what is asserted is the state the
    // throttle is left in: one slot for the code, carrying the count of the lines it
    // swallowed. The table is process-wide, so the test starts from an empty one --
    // whatever another test emitted before must not change the count below.
    THROTTLE
        .lock()
        .expect("the throttle is never poisoned")
        .slots = [const { None }; THROTTLE_SLOTS];
    for _ in 0..100 {
        let _ = guard_ffi(false, || panic_any("the candidate window blew up"));
    }
    let suppressed = THROTTLE
        .lock()
        .map(|mut table| {
            table
                .slots
                .iter_mut()
                .flatten()
                .find(|slot| slot.code_hash == code_hash("ffi/panic: the candidate window blew up"))
                .map(|slot| slot.suppressed)
        })
        .expect("the throttle is never poisoned");
    assert_eq!(
        suppressed,
        Some(99),
        "a hundred injected panics write one line and count the other ninety-nine"
    );
}

#[test]
fn test_guard_ffi_records_the_contained_panic_in_the_crash_directory() {
    // The forensics half of the guard: a contained panic leaves a record file in the
    // crash directory, naming the boundary that held the payload, with the mode the
    // privacy baseline fixes. The directory is process-wide and first-wins, which is
    // why the runner gives each test a process of its own.
    let root = PathBuf::from("/tmp").join(format!("rspinyin-ui-guard-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    crash::set_crash_directory(root.clone());

    let result = guard_ffi(false, || panic_any("the candidate window blew up"));
    assert!(!result, "a panicking body must yield the fallback value");

    let names: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("listing the crash directory")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert_eq!(names.len(), 1, "one panic leaves one record: {names:?}");
    let mode = std::fs::metadata(&names[0])
        .expect("the record's metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the record is owner-only");
    let text = std::fs::read_to_string(&names[0]).expect("reading the record");
    assert!(
        text.contains("payload=ffi::guard: the candidate window blew up"),
        "the record names the boundary and the message:\n{text}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn test_register_crash_signal_arms_both_signals_and_refuses_a_foreign_one() {
    // The registrar's answer to the crash channel's contract: both handled signals
    // install, and a signal the channel does not handle is refused with the channel's
    // read-only code rather than registered for a body that would never run.
    fn body(_: SignalInfo) -> ! {
        unreachable!("the test arms no signal that is ever delivered")
    }
    for (signal, name) in signal::HANDLED_SIGNALS {
        assert!(
            register_crash_signal(signal, body).is_ok(),
            "{name} can be armed"
        );
    }
    let refused = register_crash_signal(42, body);
    assert!(
        matches!(refused, Err(ImeError::DataReadonly { .. })),
        "a foreign signal is refused, not registered: {refused:?}"
    );
}
