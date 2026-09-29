//! FFI infrastructure between this crate and the Fcitx5 C++ host.
//!
//! This module is the crate's only `unsafe` boundary and the only place allowed to
//! cross it: the C ABI contract in [`abi`], the panic guard every entry point runs
//! under, and the crash channel those entry points report through.
//!
//! # Layout
//!
//! * [`abi`] — the frozen `#[repr(C)]` contract (`docs/dev/features.md` §2.2.3) and
//!   the callbacks the host calls into. This is the *engine* addon's table; the
//!   user-interface addon has its own, in `crates/ime-ui-addon`.
//! * `guard_ffi` / `catch_ffi` — the panic guard. Unwinding into C++ is undefined
//!   behaviour, so a panic has to become a value at the boundary; the workspace
//!   profile deliberately keeps unwinding enabled, without which the guard could not
//!   catch anything.
//! * `write_stderr_line` / `emit_diagnostic` — the crash channel. It bypasses the
//!   diagnostics layer on purpose: the addon can be rejected or panic before that layer
//!   is initialised. A code that repeats is written once per window, with the repeats it
//!   cost carried on the next line that code writes.
//!
//! # Diagnostic rate
//!
//! The callers of `emit_diagnostic` include the "the candidate window is not ready" path,
//! which a session without a window reaches on every keystroke, and every malformed
//! pointer the host hands across the ABI. Each of those writes is an unbuffered syscall
//! on the fcitx5 main loop, which is not work that loop should be paying per frame, so a
//! code reports itself once and then stays quiet until its window has passed. The first
//! occurrence, the one an operator needs, is always written; the repeats behind it are
//! counted and reported on that code's next line.
//!
//! # Threading
//!
//! Everything here runs on the Fcitx5 host thread, inside a host callback. No entry
//! point blocks, allocates without bound, or holds a lock across IO: the throttle's table
//! is a leaf lock taken for one comparison and released before the line is written, and
//! no path takes another lock while it is held.
//!
//! The table is process-wide, so two tests driving the real `emit_diagnostic` would
//! suppress each other. The tests drive `emit_through` with a table and a clock of their
//! own instead; the injected instant is also what makes the window boundary reachable
//! without sleeping.
// The FFI boundary is the one place in this crate allowed to use `unsafe`.
// The callbacks take raw pointers from the host and read through them. They cannot be
// `unsafe fn`: the C ABI contract (and the C++ glue) stores plain function pointers, and
// the pointer validity is the host's guarantee, documented in each `# Safety` section.
// The `unsafe_code` allowance for this module lives on the `pub mod ffi;` declaration in
// `lib.rs`, deliberately not here: a crate-level `#![allow]` would also permit raw
// pointers anywhere else in the crate, which the unsafe audit rejects.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::any::Any;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub mod abi;

pub use abi::{
    FcitxKeyEvent, RSPINYIN_ABI_VERSION, RSPINYIN_VTABLE, RspinyinHandshake, RspinyinVtable,
    rspinyin_plugin_init,
};

/// A panic caught at the FFI boundary, ready for the crash channel.
#[derive(Debug)]
pub(crate) struct PanicReport {
    /// Message taken from the panic payload, or a placeholder when the payload was
    /// not a string.
    message: String,
}

impl PanicReport {
    /// Extracts the message a `catch_unwind` payload carries.
    fn from_payload(payload: &(dyn Any + Send)) -> Self {
        if let Some(message) = payload.downcast_ref::<&'static str>() {
            return Self {
                message: (*message).to_owned(),
            };
        }
        if let Some(message) = payload.downcast_ref::<String>() {
            return Self {
                message: message.clone(),
            };
        }
        Self {
            message: String::from("<non-string panic payload>"),
        }
    }

    /// The line this report contributes to the crash channel.
    ///
    /// The `ffi/panic` code follows the `domain/action/reason` shape the project uses
    /// for every cross-boundary condition, so a crash is greppable next to the other
    /// FFI diagnostics.
    fn crash_line(&self) -> String {
        format!("rspinyin: ffi/panic: {}", self.message)
    }
}

/// Runs `body` and turns a panic into a [`PanicReport`].
///
/// The FFI entry points use `guard_ffi`, which maps the same case onto their
/// documented fallback value instead of surfacing the report.
pub(crate) fn catch_ffi<R>(body: impl FnOnce() -> R) -> Result<R, PanicReport> {
    catch_unwind(AssertUnwindSafe(body)).map_err(|payload| PanicReport::from_payload(&*payload))
}

/// Runs `body` under the FFI panic guard, returning `fallback` if it panics.
///
/// Every `extern "C"` body must run inside this: a panic that unwinds into C++ is
/// undefined behaviour, so the boundary turns it into an ordinary return value and
/// records it on the crash channel.
pub(crate) fn guard_ffi<R>(fallback: R, body: impl FnOnce() -> R) -> R {
    guard_ffi_with(fallback, write_stderr_line, body)
}

/// The body of [`guard_ffi`], over a caller-supplied crash channel.
///
/// The sink is a parameter for the same reason [`emit_through`]'s is: a test can then
/// read back the line a contained panic writes, which is the half of the guard's contract
/// that "it answers with the fallback" does not cover. The production path passes
/// [`write_stderr_line`] and pays nothing for the indirection — the argument is
/// monomorphised in place.
fn guard_ffi_with<R>(fallback: R, write_line: impl FnOnce(&str), body: impl FnOnce() -> R) -> R {
    match catch_ffi(body) {
        Ok(value) => value,
        Err(report) => {
            write_line(&report.crash_line());
            fallback
        }
    }
}

/// How long one diagnostic code stays quiet after it has been written.
///
/// The window is the unit a storm is measured in: a session whose candidate window never
/// becomes ready reports `ui/not-ready` on every keystroke, and what an operator needs
/// from that is the first line plus the rate behind it, not one line per frame.
const THROTTLE_WINDOW: Duration = Duration::from_secs(1);

/// How many distinct codes the throttle can track at once.
///
/// The codes this layer emits are a closed and small set — the literals in `ffi::abi` and
/// `engine`, plus the lifecycle lines that carry a step name — so a fixed table has
/// headroom, and a linear scan over a few dozen entries is cheaper than hashing into a
/// map, which would need an allocator of its own.
const THROTTLE_SLOTS: usize = 32;

/// The 64-bit FNV-1a offset basis.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// The 64-bit FNV-1a prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// What the throttle decided about one call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Emission {
    /// The code may be written.
    Emit {
        /// Repeats of this code that the window swallowed since it last wrote, reported
        /// so the count reaches the log instead of disappearing with the lines.
        suppressed: u32,
    },
    /// The code already wrote inside the current window: this call is counted and
    /// dropped.
    Suppressed,
}

/// One tracked code.
#[derive(Clone, Copy, Debug)]
struct Slot {
    /// Hash of the code, standing in for the code itself; `code_hash` records why a hash
    /// and not the code.
    code_hash: u64,
    /// When the code last wrote a line.
    last_emitted: Instant,
    /// Repeats dropped since then, reported with the next line the code writes.
    suppressed: u32,
}

/// The fixed table that decides which codes may write.
#[derive(Debug)]
struct Throttle {
    /// One slot per tracked code, filled from the front; `None` is a free slot.
    slots: [Option<Slot>; THROTTLE_SLOTS],
}

impl Throttle {
    /// Creates an empty table.
    const fn new() -> Self {
        Self {
            slots: [None; THROTTLE_SLOTS],
        }
    }

    /// Decides whether `code` may write at `now`, and records the answer.
    ///
    /// A code inside its window is counted and refused; a code whose window has passed
    /// writes again and hands back what it swallowed in the meantime.
    fn admit(&mut self, code: &str, now: Instant) -> Emission {
        let code_hash = code_hash(code);
        if let Some(slot) = self.find(code_hash) {
            if now.saturating_duration_since(slot.last_emitted) < THROTTLE_WINDOW {
                slot.suppressed = slot.suppressed.saturating_add(1);
                return Emission::Suppressed;
            }
            let suppressed = slot.suppressed;
            slot.last_emitted = now;
            slot.suppressed = 0;
            return Emission::Emit { suppressed };
        }
        self.track(code_hash, now);
        Emission::Emit { suppressed: 0 }
    }

    /// The slot tracking `code_hash`, if any slot does.
    fn find(&mut self, code_hash: u64) -> Option<&mut Slot> {
        self.slots
            .iter_mut()
            .flatten()
            .find(|slot| slot.code_hash == code_hash)
    }

    /// Claims a free slot for a code the table has not seen.
    ///
    /// A full table leaves the code untracked, which means it writes on every call rather
    /// than once per window. That is the direction to fail in: the alternative — reusing
    /// a slot — would discard the suppressed count of the code it displaced, and could
    /// displace a code that is being throttled precisely because it repeats. The table is
    /// sized above the set of codes this layer can emit, so the branch is not reachable
    /// with today's callers.
    fn track(&mut self, code_hash: u64, now: Instant) {
        if let Some(free) = self.slots.iter_mut().find(|slot| slot.is_none()) {
            *free = Some(Slot {
                code_hash,
                last_emitted: now,
                suppressed: 0,
            });
        }
    }

    /// Number of codes the table is tracking.
    ///
    /// Only the tests ask. A suppressed call that grew the table would mean it allocated,
    /// which is the one thing that path must not do.
    #[cfg(test)]
    fn tracked(&self) -> usize {
        self.slots.iter().flatten().count()
    }
}

/// The 64-bit FNV-1a hash of `code`.
///
/// The table stores a hash rather than the code itself because not every code is a
/// literal: the lifecycle lines carry a step name and the FFI lines carry an error, so
/// keeping the code would mean an allocation for each distinct one — exactly what the
/// throttle exists to remove. A collision makes two codes share one window, which costs
/// at most a line that was already written a moment earlier: the table only decides
/// *whether* a line is written, never what it says.
fn code_hash(code: &str) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in code.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// The process-wide throttle. A leaf lock: no path takes another lock while it is held.
static THROTTLE: Mutex<Throttle> = Mutex::new(Throttle::new());

/// Reports a `domain/action/reason` condition on the crash channel.
///
/// At most one line per code per `THROTTLE_WINDOW`. The callers include the "the
/// candidate window is not ready" path, which a session without a window reaches on every
/// keystroke, and an unbuffered write to a stderr whose reader is slow blocks the fcitx5
/// main loop. Repeats inside the window are counted rather than discarded — the next line
/// the code writes carries the count — so a storm shows up as a rate instead of as a line
/// per frame.
///
/// Throttling decides how often a line is written, never what it says: a code keeps its
/// exact spelling, because the codes are stable identifiers that diagnostics and tests
/// match on.
///
/// # Panics
///
/// This function does not panic. It is reached from panic paths, so it must not.
pub(crate) fn emit_diagnostic(code: &str) {
    emit_through(&THROTTLE, code, Instant::now(), write_stderr_line);
}

/// The body of `emit_diagnostic`, over a caller-supplied table, clock and sink.
///
/// All three are parameters so the tests can drive the throttle with a table of their own
/// — the process-wide one would make two tests suppress each other — a clock they
/// control, and a sink they can read back. The production path passes the static table,
/// the real clock and stderr, and pays nothing for the indirection: the arguments are
/// monomorphised in place.
fn emit_through(
    throttle_table: &Mutex<Throttle>,
    code: &str,
    now: Instant,
    // `mut` because the sink is an `FnMut`: calling it is a mutation of the closure's
    // captured state, so the binding has to be mutable even though it is only called once
    // per emit.
    mut write_line: impl FnMut(&str),
) {
    let decision = {
        // The lock is released before the write: a blocking write to stderr must never
        // happen while the table is held.
        let mut table = lock_throttle(throttle_table);
        table.admit(code, now)
    };
    match decision {
        Emission::Suppressed => {}
        Emission::Emit { suppressed } => write_line(&diagnostic_line(code, suppressed)),
    }
}

/// Borrows a throttle table, recovering the contents of a poisoned lock.
///
/// Poisoning means a holder panicked. The table holds timestamps and counts, so the worst
/// a recovered one can do is suppress a line that would have been written; treating the
/// crash channel as permanently unusable would be far worse than that.
fn lock_throttle(throttle: &Mutex<Throttle>) -> MutexGuard<'_, Throttle> {
    match throttle.lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The line one emission writes, with the count of what the window swallowed.
///
/// The code stays the line's first field and keeps its spelling, so a grep for a code
/// still matches every line that code produced. The count is a suffix rather than a line
/// of its own: a second line would double the writes the throttle exists to remove.
fn diagnostic_line(code: &str, suppressed: u32) -> String {
    if suppressed == 0 {
        return format!("rspinyin: {code}");
    }
    let window_ms = THROTTLE_WINDOW.as_millis();
    format!("rspinyin: {code} (suppressed {suppressed} repeats in {window_ms}ms)")
}

/// Writes one line to stderr.
///
/// Deliberately not `tracing`: a crash channel must not depend on another subsystem
/// being healthy, and this one is reached before the diagnostics layer is
/// initialised or while a panic is being handled. Fcitx5 redirects the process's
/// stderr into its own log, which is where an operator looks after a failed load.
fn write_stderr_line(line: &str) {
    // A failure to write is not actionable: there is no second channel to report it
    // on, and every caller is already on an error path.
    let _ = writeln!(std::io::stderr(), "{line}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A code a caller can reach once per frame, used to pin the throttle's shape.
    const REPEATED_CODE: &str = "ffi/null-key-event";

    /// A second code, so a test can show that one code's window does not silence another.
    const OTHER_CODE: &str = "ffi/invalid-commit";

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
}
