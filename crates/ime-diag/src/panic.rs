//! The process-wide panic hook.
//!
//! Responsibility: turn a panic into a record and a report, without letting the process
//! die and without recursing into itself.
//!
//! Boundaries: this module installs the hook and fixes the order of the steps. The
//! record's shape and the file it lands in belong to [`crate::crash::record`]; the
//! action that clears the input session afterwards is registered by the addon, which is
//! the only layer that knows what a session is.
//!
//! # Why a panic must not end the process
//!
//! The plugin is a library inside the user's Fcitx5 process. A panic that escaped an
//! input-method callback would take the host down with it, and every application on the
//! desktop would lose its input method at once. The hook's contract is therefore the
//! opposite of the default one: record everything, then let the unwinding continue and
//! let the boundary that caught the panic decide what to do with it.
//!
//! # Order of the steps
//!
//! 1. the record is written into the crash directory,
//! 2. the event is reported to `tracing` -- structure only, never the message,
//! 3. the message is written to stderr, which Fcitx5 captures into its own log,
//! 4. the process is *not* aborted and the default hook is *not* restored,
//! 5. the session is cleared through the registered recovery action.
//!
//! The record comes first on purpose: the logging layer may be the thing that is
//! broken, and a record that is already on disk cannot be lost to it.
//!
//! # The hook must not panic
//!
//! A panic raised while a panic is being handled aborts the process, which is the one
//! outcome this module exists to prevent, and non-test code may not use `panic!`,
//! `unwrap` or `expect` in the first place. Every step here is therefore either
//! infallible or wrapped in [`catch_unwind`], including the recovery action.

use std::panic::{AssertUnwindSafe, PanicHookInfo, catch_unwind, set_hook};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::crash;

/// Set once the hook is installed, so that a second installation changes nothing.
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Installs the process-wide panic hook.
///
/// Installing is idempotent: the first call wins and a later one changes nothing, so a
/// plugin reload cannot end up with two hooks writing a record for the same panic.
///
/// # Parameters
///
/// - `crash_dir`: the directory records are written into. It is created with mode `0700`
///   when the first record is written, so a directory that does not exist yet is not an
///   error here; a crash that happens before the plugin has prepared its layout is
///   reported on stderr instead.
///
/// # Panics
///
/// Never: `set_hook` does not fail, and the hook it installs cannot panic.
pub fn install_panic_hook(crash_dir: PathBuf) {
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }
    crash::set_crash_directory(crash_dir);
    set_hook(Box::new(run));
}

/// The installed hook: reads the panic's facts and hands them over.
fn run(info: &PanicHookInfo<'_>) {
    handle_panic(
        info.location().map(|location| location.to_string()),
        crash::panic_message(info.payload()),
    );
}

/// Runs the hook's five steps for one panic.
///
/// Split out from the hook itself so that its behaviour can be exercised without
/// arranging a real panic: a `PanicHookInfo` cannot be constructed, but its two facts
/// can be passed in directly.
///
/// # Parameters
///
/// - `location`: `file:line:column` of the panic, when it carried one.
/// - `payload`: the panic message.
///
/// # Panics
///
/// Never. The record and its reports run inside `catch_unwind` because they run while a
/// panic is already being handled, and the recovery action is guarded the same way.
pub(crate) fn handle_panic(location: Option<String>, payload: String) {
    if !crash::enter_crash_path() {
        // A failure raised while a record is already being written must not start a
        // second round: one crash leaves one record.
        crash::write_stderr_line("rspinyin: crash/panic: the crash path was re-entered");
        return;
    }
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let record = crash::capture(payload, location);
        crash::publish(&record);
        crash::recover_after_crash();
    }));
    // Released unconditionally by the call that claimed it: a failure here must not
    // disable recording for the rest of the process.
    crash::leave_crash_path();
    if outcome.is_err() {
        crash::write_stderr_line("rspinyin: crash/panic: the panic hook failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_panic_reports_without_a_crash_directory() {
        // The crash directory is deliberately left unset in this binary, so this is also
        // the degraded case: a panic before the layout exists is reported on stderr and
        // costs no file.
        let outcome = catch_unwind(|| handle_panic(None, String::from("deliberate failure")));
        assert!(outcome.is_ok(), "the hook may not panic while handling a panic: {outcome:?}");
    }

    #[test]
    fn test_handle_panic_releases_the_crash_path_after_a_failure() {
        // Two panics in a row, then a claim: the second panic is only recorded at all if
        // the first gave the claim up, and the claim is what the next crash needs.
        let first = catch_unwind(|| handle_panic(None, String::from("deliberate failure")));
        let second = catch_unwind(|| handle_panic(None, String::from("deliberate failure")));
        assert!(first.is_ok() && second.is_ok(), "{first:?} {second:?}");
        assert!(crash::enter_crash_path(), "the crash path was released");
        crash::leave_crash_path();
    }

    #[test]
    fn test_handle_panic_runs_the_registered_recovery_action() {
        // The only test in this binary that registers a recovery action: the slot is
        // process-wide and the first registration wins, so a second one would never run.
        static RECOVERED: AtomicBool = AtomicBool::new(false);
        crash::set_crash_recovery(|| RECOVERED.store(true, Ordering::Release));

        handle_panic(None, String::from("deliberate failure"));

        assert!(RECOVERED.load(Ordering::Acquire), "the session is cleared after a crash");
    }

    #[test]
    #[ignore = "installs a process-wide hook, which would hide other tests' panics"]
    fn test_install_panic_hook_keeps_the_process_alive_after_a_panic() {
        // End-to-end check: install the hook, panic inside a caught scope, and assert
        // that the process is still running afterwards and that a record exists.
        let root = std::path::PathBuf::from("/tmp")
            .join(format!("rspinyin-panic-hook-{}", std::process::id()));
        install_panic_hook(root.clone());

        let outcome = catch_unwind(|| {
            let zero = 0usize;
            assert!(zero > 0, "deliberate failure");
        });

        assert!(outcome.is_err(), "the panic still unwinds to its caller");
        assert!(
            std::fs::read_dir(&root)
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(false),
            "the hook wrote a record into {root:?}"
        );
    }
}
