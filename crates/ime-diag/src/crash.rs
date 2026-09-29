//! The crash channel: recording a failure that has no return path.
//!
//! Responsibility: hold what the crash paths need in common -- where records go, what
//! the session was doing, and the one action that clears the session afterwards -- and
//! provide the entry points the boundaries call when a panic has been caught.
//!
//! Boundaries: this module decides *when* a record is written and *in what order* the
//! reports go out. The shape of a record and the file it lands in belong to [`record`];
//! the signals that no `catch_unwind` can see belong to [`signal`].
//!
//! # Composition with the FFI guard
//!
//! The addon's `extern "C"` entry points already run under a guard that turns a panic
//! into a return value, because unwinding into C++ is undefined behaviour. That guard
//! is the mechanism; this module is the record it leaves behind. The two compose rather
//! than compete: the guard keeps the process alive and produces the fallback value, and
//! [`record_ffi_panic`] is what it calls on the way out. [`guard`] is the same
//! composition for a caller that would rather not repeat it.
//!
//! # Why the record comes before the log
//!
//! Every report is best-effort and every step is independent: the record file, the
//! `tracing` event, and the line on stderr. They are attempted in that order because
//! the logging layer may be the thing that is broken -- a panic raised while it holds
//! its writer would make a log line wait -- and a record that is already on disk cannot
//! be lost to that.
//!
//! # Privacy
//!
//! Nothing on this path can carry what the user typed. The structural facts a record
//! may hold are a closed set of checked keys ([`CrashContextKey`]); there is no field
//! for an input string, a preedit, a candidate or a commit text, and the free-text
//! fields that do exist are the panic message and the backtrace. The message is a
//! static template by the same rule that governs log messages, and the record bounds
//! and scrubs both before they reach a file. For the same reason the `tracing` event
//! carries structure only -- the code, the thread, the location -- and never the
//! message.

use std::any::Any;
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{io::Write as _, thread};

pub mod context;
pub mod record;
pub mod signal;

pub use context::{CrashContext, CrashContextKey};
pub use record::{CrashRecord, thread_identifier};

/// The stable code every panic recorded here is reported under.
pub const CRASH_PANIC_CODE: &str = "crash/panic";

/// The name recorded for a thread that was never named.
const UNNAMED_THREAD: &str = "<unnamed>";

/// Where crash records are written, once a caller has said so.
static CRASH_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

/// The action that clears the input session after a crash.
static RECOVERY: OnceLock<fn()> = OnceLock::new();

/// The provider of the structural facts a record carries.
static CONTEXT_PROVIDER: OnceLock<fn() -> CrashContext> = OnceLock::new();

thread_local! {
    /// Set while this thread is inside the crash path.
    ///
    /// A thread-local rather than a process-wide flag on purpose. What this guards
    /// against is recursion on one stack -- a recovery action that panics at another
    /// boundary and asks for a second record -- not concurrency between threads. The
    /// host thread and the UI thread can crash at the same time, and a process-wide flag
    /// would make the second one lose its record, which is the one thing the crash path
    /// must not do. A `const`-initialised `Cell` needs no lazy initialisation, so
    /// touching it is a plain thread-local access.
    static IN_CRASH_PATH: Cell<bool> = const { Cell::new(false) };
}

/// Records the directory crash records are written to.
///
/// The first call wins: a later one leaves the directory alone, so a plugin reload
/// cannot split the records of one process across two directories.
///
/// # Parameters
///
/// - `dir`: the directory, normally the one the layout layer resolved.
///
/// # Panics
///
/// Never.
pub fn set_crash_directory(dir: PathBuf) {
    let _ = CRASH_DIRECTORY.set(dir);
}

/// The directory crash records are written to.
///
/// # Returns
///
/// The directory, or `None` when no caller has named one. A crash that happens before
/// the plugin has prepared its layout is reported on stderr and not written to a file,
/// which is the right trade: an uninitialised process has no layout to write into.
pub fn crash_directory() -> Option<&'static Path> {
    CRASH_DIRECTORY.get().map(PathBuf::as_path)
}

/// Registers the action that clears the input session after a crash.
///
/// The action is the addon's, because only the addon knows what a session is. It runs
/// on whatever thread crashed, after the record has been written.
///
/// # Parameters
///
/// - `action`: the reset. It must not block: it is called from the crash path.
///
/// # Panics
///
/// Never.
pub fn set_crash_recovery(action: fn()) {
    let _ = RECOVERY.set(action);
}

/// Registers the provider of the structural facts a record carries.
///
/// The provider is asked for the session's state at the moment of the crash, which the
/// diagnostics layer cannot know. Everything it returns is checked by
/// [`CrashContext`], so a provider cannot smuggle an input string into a record.
///
/// # Parameters
///
/// - `provider`: the function that reads the facts.
///
/// # Panics
///
/// Never.
pub fn set_crash_context_provider(provider: fn() -> CrashContext) {
    let _ = CONTEXT_PROVIDER.set(provider);
}

/// Runs the registered recovery action, if there is one.
///
/// This is the compensation for the `AssertUnwindSafe` every guard on the boundary
/// uses. A panic can leave the session half-updated -- a selection applied without its
/// frame, a preedit without its buffer -- and the guard cannot see that, so the session
/// is cleared rather than trusted. Clearing is cheap and idempotent; a session that is
/// wrong is not.
///
/// The action runs inside `catch_unwind` because it runs while a panic is already being
/// handled: a second panic raised here would abort the process, which is the one
/// outcome the whole crash path exists to prevent.
///
/// # Panics
///
/// Never.
pub fn recover_after_crash() {
    let Some(action) = RECOVERY.get() else {
        return;
    };
    let _ = catch_unwind(AssertUnwindSafe(action));
}

/// Milliseconds since the Unix epoch.
///
/// A clock that cannot be read is reported as zero rather than failing the record: a
/// record without a time is still worth more than no record. The call is a
/// `clock_gettime(2)` through the vDSO, which is why the signal handler may make it.
pub(crate) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Builds a record for a failure that has just been caught on this thread.
///
/// The thread, the clock, the backtrace and the structural facts are read here, so that
/// every caller gets the same record; only the message and the location differ between
/// a panic hook and an FFI boundary.
pub(crate) fn capture(payload: String, location: Option<String>) -> record::CrashRecord {
    let thread = thread::current();
    CrashRecord {
        timestamp_unix_ms: now_unix_ms(),
        thread_name: thread
            .name()
            .map(str::to_owned)
            .unwrap_or_else(|| String::from(UNNAMED_THREAD)),
        thread_id: thread_identifier(thread.id()),
        location,
        payload,
        backtrace: std::backtrace::Backtrace::force_capture().to_string(),
        context: provider_context(),
    }
}

/// Asks the registered provider for the session's structural facts.
///
/// The provider reads live session state, which a panic may have left half-updated; a
/// failure there costs the context, not the record.
fn provider_context() -> CrashContext {
    let Some(provider) = CONTEXT_PROVIDER.get() else {
        return CrashContext::new();
    };
    catch_unwind(AssertUnwindSafe(provider)).unwrap_or_default()
}

/// Claims the crash path for this thread, so that a failure inside it cannot recurse.
///
/// Returns `false` when this thread is already on the crash path. The call that claimed
/// it owns the release, which is why this is a pair rather than a closure: the claim has
/// to be given up even when the body it wraps panics.
pub(crate) fn enter_crash_path() -> bool {
    IN_CRASH_PATH.with(|claimed| !claimed.replace(true))
}

/// Releases the claim [`enter_crash_path`] took on this thread.
pub(crate) fn leave_crash_path() {
    IN_CRASH_PATH.with(|claimed| claimed.set(false));
}

/// Writes `record` and reports it on every channel that is still working.
///
/// The order is deliberate: the file first, then the structured event, then stderr.
/// See the module documentation for why.
pub(crate) fn publish(record: &CrashRecord) {
    let path = write_record_file(record);
    tracing::error!(
        code = CRASH_PANIC_CODE,
        thread = %record.thread_name,
        thread_id = record.thread_id,
        location = ?record.location,
        record = ?path,
        "a crash was recorded"
    );
    let mut line = String::from("rspinyin: ");
    line.push_str(CRASH_PANIC_CODE);
    line.push_str(": ");
    line.push_str(&record::escape_line(
        &record.payload,
        record::MAX_PAYLOAD_CHARS,
    ));
    write_stderr_line(&line);
}

/// Writes the record, reporting where it went or why it did not.
fn write_record_file(record: &CrashRecord) -> Option<PathBuf> {
    let dir = crash_directory()?;
    match record::write_record(dir, record) {
        Ok(path) => Some(path),
        Err(error) => {
            let mut line = String::from("rspinyin: crash/record: ");
            line.push_str(&error.to_string());
            write_stderr_line(&line);
            None
        }
    }
}

/// Writes one line to stderr.
///
/// Deliberately not `tracing`: a crash channel must not depend on another subsystem
/// being healthy, and this one is reached before the diagnostics layer exists and while
/// a panic is being handled. Fcitx5 redirects the process's stderr into its own log,
/// which is where an operator looks after a crash.
pub(crate) fn write_stderr_line(line: &str) {
    // A failure to write is not actionable: there is no second channel to report it on,
    // and every caller is already on an error path.
    let _ = writeln!(std::io::stderr(), "{line}");
}

/// The text a panic payload carries.
///
/// # Parameters
///
/// - `payload`: the payload a `catch_unwind` or a panic hook handed over.
///
/// # Returns
///
/// The message, or a placeholder when the payload was not a string.
///
/// # Panics
///
/// Never.
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&'static str>() {
        return String::from(*text);
    }
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    String::from("<non-string panic payload>")
}

/// Records a panic caught at an `extern "C"` boundary.
///
/// # Parameters
///
/// - `entry_point`: the name of the function the panic escaped from, which is what
///   makes the record greppable next to the C++ glue that called it.
/// - `payload`: the payload the guard caught.
///
/// # Panics
///
/// Never: everything runs inside `catch_unwind`, because this is called from the error
/// branch of a guard whose whole purpose is that no panic crosses the C ABI, and a
/// panic escaping from here would do exactly that.
pub fn record_ffi_panic(entry_point: &str, payload: &(dyn Any + Send)) {
    record_ffi_panic_message(entry_point, &panic_message(payload));
}

/// Records a panic message caught at an `extern "C"` boundary.
///
/// # Parameters
///
/// - `entry_point`: the name of the function the panic escaped from.
/// - `message`: the panic message. It is a static template by project rule, never a
///   value taken from the input.
///
/// # Panics
///
/// Never, for the reason given on [`record_ffi_panic`].
pub fn record_ffi_panic_message(entry_point: &str, message: &str) {
    if !enter_crash_path() {
        // A failure raised while a record is already being written -- a recovery action
        // that panics at another boundary, say -- must not start a second round.
        write_stderr_line("rspinyin: crash/panic: the crash path was re-entered");
        return;
    }
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let record = capture(format!("{entry_point}: {message}"), None);
        publish(&record);
        recover_after_crash();
    }));
    // Released unconditionally by the call that claimed it: a failure here must not
    // disable recording for the rest of the process.
    leave_crash_path();
    if outcome.is_err() {
        write_stderr_line("rspinyin: crash/panic: recording the crash failed");
    }
}

/// Runs `body` under the crash guard, returning `fallback` when it panics.
///
/// The runtime half of the FFI entry-point guard: the process survives, the panic is
/// recorded, and the caller's documented fallback is what the C side receives. The four
/// return shapes an entry point can have are covered by the caller passing the right
/// fallback -- `()`, `false`, `0`, `std::ptr::null_mut()` -- which is why this is a
/// generic function rather than an attribute macro: no type has to be inspected.
///
/// # Parameters
///
/// - `entry_point`: the name of the function, for the record.
/// - `fallback`: what the entry point returns when `body` panics.
/// - `body`: the work.
///
/// # Returns
///
/// The body's value, or `fallback`.
///
/// # Panics
///
/// Never, provided `fallback` can be built without one: the panic is caught here.
pub fn guard<R>(entry_point: &str, fallback: R, body: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(payload) => {
            record_ffi_panic(entry_point, &*payload);
            fallback
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_panic_message_reads_both_string_shapes_and_placeholder() {
        let static_payload: &(dyn Any + Send) = &"a static message";
        assert_eq!(panic_message(static_payload), "a static message");

        let owned = String::from("an owned message");
        let owned_payload: &(dyn Any + Send) = &owned;
        assert_eq!(panic_message(owned_payload), "an owned message");

        let other: &(dyn Any + Send) = &7u32;
        assert_eq!(panic_message(other), "<non-string panic payload>");
    }

    #[test]
    fn test_now_unix_ms_never_moves_backwards() {
        // Deliberately not an assertion about the time itself: a test may not depend on
        // the clock, only on the property the record's ordering needs.
        let first = now_unix_ms();
        let second = now_unix_ms();
        assert!(second >= first, "{first} then {second}");
    }

    #[test]
    fn test_capture_records_the_current_thread_and_a_backtrace() {
        let record = capture(
            String::from("entry: failed"),
            Some(String::from("src/lib.rs:1:1")),
        );
        let expected = thread::current()
            .name()
            .map(str::to_owned)
            .unwrap_or_else(|| String::from(UNNAMED_THREAD));

        assert_eq!(record.thread_name, expected);
        assert_eq!(record.thread_id, thread_identifier(thread::current().id()));
        assert_eq!(record.location.as_deref(), Some("src/lib.rs:1:1"));
        assert_eq!(record.payload, "entry: failed");
        assert!(!record.backtrace.is_empty(), "a backtrace is captured");
        assert!(record.context.is_empty(), "no provider is registered here");
    }

    #[test]
    fn test_crash_directory_is_unset_until_a_caller_names_one() {
        // This test also documents the ordering the others rely on: nothing in this
        // crate's test binary calls `set_crash_directory`, so the crash path reports on
        // stderr and writes no file.
        assert!(crash_directory().is_none());
    }

    #[test]
    fn test_guard_passes_a_successful_body_through() {
        assert_eq!(guard("entry", 0u32, || 7), 7);
    }

    #[test]
    fn test_guard_returns_the_fallback_when_the_body_panics() {
        let zero = 0usize;
        let outcome = guard("on_key_event", false, || {
            assert!(zero > 0, "deliberate failure");
            true
        });
        assert!(
            !outcome,
            "the entry point's documented fallback is returned"
        );
    }

    #[test]
    fn test_record_ffi_panic_does_not_escape() {
        // Twice on purpose: the second call also proves that the first released the
        // crash-path claim, since a claim left held would return without recording.
        let outcome = catch_unwind(|| {
            record_ffi_panic_message("on_key_event", "deliberate failure");
            record_ffi_panic_message("on_key_event", "deliberate failure again");
        });
        assert!(
            outcome.is_ok(),
            "nothing may escape the crash path: {outcome:?}"
        );
        assert!(enter_crash_path(), "the crash path was released");
        leave_crash_path();
    }
}
