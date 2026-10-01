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
//!   catch anything. The guard also records the panic in the crash directory through
//!   `crash::record_ffi_panic` and reports it on the throttled channel.
//! * `register_crash_signal` — the registrar the crash channel's fault-signal arming
//!   passes: the one call here that asks the kernel to deliver a signal.
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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ime_diag::crash;
use ime_diag::crash::signal::{self, SignalCause, SignalHandler, SignalInfo};
use ime_types::ImeError;

pub mod abi;

/// Probes for the user-interface addon and triggers the transport handshake (ADR-0011).
///
/// Answers the probe mechanism that fired, or 0 when the UI addon is not loaded. The
/// engine's init sequence calls this once; the UI addon runs the mirrored probe at its
/// own registration step, which is what makes the handshake work in either load order.
pub fn engine_transport_probe() -> u32 {
    #[cfg(fcitx5_host)]
    {
        guard_ffi(0, || {
            // SAFETY: the glue reads no memory this side owns, blocks on nothing, and
            // answers with a code naming the probe mechanism that fired.
            unsafe { rspinyin_engine_transport_probe() }
        })
    }
    #[cfg(not(fcitx5_host))]
    {
        // No glue is linked, so there is no other addon to find and no log an operator
        // of this build would read.
        0
    }
}

#[cfg(fcitx5_host)]
unsafe extern "C" {
    /// The C++ half of the transport probe. Defined in `src/ffi/cpp/addon_glue.cpp`.
    ///
    /// # Safety
    ///
    /// The callee blocks on nothing and owns nothing the caller has to release.
    fn rspinyin_engine_transport_probe() -> u32;
}

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

/// The entry-point name a guard's record and line carry.
///
/// The guard wraps every `extern "C"` body and cannot know which one it wrapped -- the
/// entry points pass only their fallback value and their work -- so the record names the
/// boundary itself rather than an entry point. The message is a static template by
/// project rule, which is what keeps the pair from being a second spelling of user input.
const GUARD_ENTRY_POINT: &str = "ffi::guard";

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

    /// The code this report is emitted under.
    ///
    /// The message rides in the code because it is what makes one contained panic
    /// distinguishable from another on the crash channel, and the throttle keys on this
    /// string: a panic whose message repeats -- as the static templates do -- shares one
    /// window with its own repeats, which is the deduplication the diagnostic channel
    /// promises.
    fn code(&self) -> String {
        format!("ffi/panic: {}", self.message)
    }

    /// The line this report contributes to the crash channel.
    ///
    /// The `ffi/panic` code follows the `domain/action/reason` shape the project uses
    /// for every cross-boundary condition, so a crash is greppable next to the other
    /// FFI diagnostics.
    fn crash_line(&self) -> String {
        format!("rspinyin: {}", self.code())
    }

    /// Records the panic in the crash directory.
    ///
    /// The forensics half of the guard: the record file, the `tracing` event and the
    /// crash channel's own stderr line are `ime-diag`'s to produce, and the payload the
    /// guard already holds is all they need. Recording happens whether or not the
    /// throttled line below is written, because a record is evidence and a line is a
    /// convenience.
    fn record_crash(&self) {
        crash::record_ffi_panic_message(GUARD_ENTRY_POINT, &self.message);
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
/// undefined behaviour, so the boundary turns it into an ordinary return value, records
/// it in the crash directory, and reports it on the crash channel at most once per
/// throttle window.
pub(crate) fn guard_ffi<R>(fallback: R, body: impl FnOnce() -> R) -> R {
    match catch_ffi(body) {
        Ok(value) => value,
        Err(report) => {
            report.record_crash();
            emit_diagnostic(&report.code());
            fallback
        }
    }
}

/// The body of [`guard_ffi`], over a caller-supplied crash channel.
///
/// The sink is a parameter for the same reason [`emit_through`]'s is: a test can then
/// read back the line a contained panic writes, which is the half of the guard's contract
/// that "it answers with the fallback" does not cover. The throttle decision runs against
/// a table of the call's own rather than the process-wide one, so a test cannot be
/// suppressed by a line another test wrote into the shared table inside the same window;
/// the production path above is the one that shares the table.
fn guard_ffi_with<R>(fallback: R, write_line: impl FnMut(&str), body: impl FnOnce() -> R) -> R {
    match catch_ffi(body) {
        Ok(value) => value,
        Err(report) => {
            report.record_crash();
            emit_through(
                &Mutex::new(Throttle::new()),
                &report.code(),
                Instant::now(),
                write_line,
            );
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

// ── the crash signal registrar ─────────────────────────────────────────────────────

/// The bodies the crash channel armed, indexed by the position of the signal in
/// [`signal::HANDLED_SIGNALS`].
///
/// A body is stored before the handler that reads it is installed, and read from inside
/// the handler. A plain atomic is the whole of what that lookup may afford: a fault
/// interrupts whatever the process was doing, possibly a lock holder or an allocator
/// mid-update, so the handler path must not take a lock and must not allocate.
static ARMED_BODIES: [AtomicUsize; signal::HANDLED_SIGNALS.len()] =
    [const { AtomicUsize::new(0) }; signal::HANDLED_SIGNALS.len()];

/// Installs `handler` for `signal`: the registrar the crash channel's arming sequence
/// passes, and the one call in this crate that asks the kernel to deliver a signal.
///
/// The kernel call is `sigaction(2)` with `SA_SIGINFO`, so the handler receives the
/// `siginfo_t` whose `si_code` the crash channel classifies and whose `si_addr` names
/// the faulting address; see [`signal::RegisterSignal`] for the contract this
/// implementation answers to.
///
/// # Parameters
///
/// - `signal_number`: the signal number to handle, one of [`signal::HANDLED_SIGNALS`].
/// - `handler`: the body to run when it arrives. It never returns.
///
/// # Errors
///
/// Returns [`ImeError::DataReadonly`] when the signal is not one the crash channel
/// handles or the kernel refuses the registration. The code is the channel's own
/// "this process has stopped writing crash data" condition: faults still terminate the
/// process, they just leave no record behind.
///
/// # Panics
///
/// Never.
pub(crate) fn register_crash_signal(
    signal_number: i32,
    handler: SignalHandler,
) -> Result<(), ImeError> {
    let Some(slot) = signal::HANDLED_SIGNALS
        .iter()
        .position(|(number, _)| *number == signal_number)
    else {
        return Err(ImeError::DataReadonly {
            reason: format!("signal {signal_number} is not one of the crash signals"),
        });
    };
    // Stored before the registration lands, so that a fault arriving the instant the
    // kernel starts delivering finds the body already in place. The release pairs with
    // the acquire load in `on_host_signal`.
    ARMED_BODIES[slot].store(handler as usize, Ordering::Release);

    // Zeroed is a valid `sigaction`: an empty signal mask, no restorer, and every flag
    // the lines below set or leave. The body is the trampoline, which reads the two
    // facts the record needs out of the `siginfo_t` and hands them to `handler`.
    // SAFETY: `sigaction` is plain old data, so zeroed bytes are a valid empty state for
    // every field of it, and the assignments that follow finish the preparation.
    let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
    action.sa_sigaction = on_host_signal as usize;
    action.sa_flags = libc::SA_SIGINFO;
    // `sigemptyset(3)` and `sigaction(2)` are the documented interfaces for what this
    // does. Neither retains a pointer into `action`, and the old-action out-parameter is
    // not requested.
    let armed = libc::sigemptyset(&mut action.sa_mask) == 0
        && libc::sigaction(signal_number, &action, std::ptr::null_mut()) == 0;
    if !armed {
        return Err(ImeError::DataReadonly {
            reason: format!(
                "the crash signal could not be armed: {}",
                std::io::Error::last_os_error()
            ),
        });
    }
    Ok(())
}

/// The body the kernel calls for a fault the crash channel armed.
///
/// It reads the two facts a fault's record carries out of the `siginfo_t` the kernel
/// passed and hands them to the body that was registered for the signal, which never
/// returns. Every step between the entry and that call is a memory read or an atomic
/// load: no allocation, no lock, and none of `core::fmt`, all of which are outside what
/// a signal handler may run.
extern "C" fn on_host_signal(
    signal_number: i32,
    info: *mut libc::siginfo_t,
    _context: *mut libc::c_void,
) {
    let Some(slot) = signal::HANDLED_SIGNALS
        .iter()
        .position(|(number, _)| *number == signal_number)
    else {
        // Unreachable while the registrar is the only installer, and not actionable if
        // it ever is not: there is no channel to report on inside a handler, so the
        // process ends with the recorded-fault status rather than by returning into a
        // faulting instruction.
        libc::_exit(signal::CRASH_EXIT_CODE);
    };
    let body = ARMED_BODIES[slot].load(Ordering::Acquire);
    if body == 0 {
        // Unreachable: the body is stored before the registration that arms this
        // handler. As above, exiting is the only honest answer if it ever is not.
        libc::_exit(signal::CRASH_EXIT_CODE);
    }
    // SAFETY: the kernel passes a valid `siginfo_t` for the signal it delivered, and
    // reading `si_code` and `si_addr` out of it is the documented way to learn what
    // faulted. `si_addr` is only meaningful for a fault, so a signal another process
    // sent reports no address at all.
    let (cause, address) = unsafe {
        let info = &*info;
        let cause = SignalCause::from_si_code(info.si_code);
        let address = if cause.is_fatal() {
            Some(info.si_addr() as usize)
        } else {
            None
        };
        (cause, address)
    };
    // SAFETY: the value was stored by `register_crash_signal` as a `SignalHandler` and
    // nothing else writes the slot, so it is a valid function pointer of exactly this
    // type. The body never returns, so nothing runs after the call.
    let handler: SignalHandler = unsafe { std::mem::transmute(body) };
    handler(SignalInfo { cause, address });
}

#[cfg(test)]
mod tests;
