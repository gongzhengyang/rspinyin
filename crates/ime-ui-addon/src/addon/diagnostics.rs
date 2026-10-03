//! The crash forensics half of the diagnostics step.
//!
//! Responsibility: arm the crash channel — name the record directory, install the panic
//! hook, arm the fault signals behind the FFI module's registrar, and register the
//! context provider the records are written with. This is the addon's copy of the
//! assembly the engine addon's `diagnostics` step performs: the two libraries are
//! `dlopen`'d separately and share no state, so a crash in the candidate window needs a
//! channel armed in *this* library to be recorded.
//!
//! Boundaries: `ime-diag` owns the whole crash channel; this module decides only *where*
//! the records go and *when* the channel is armed, and it is the only place in this crate
//! that calls the arming sequence.
//!
//! # Why the arming fails soft
//!
//! Every step of the sequence is first-wins or idempotent, and none of them can decline
//! the addon: forensics that cost the user their input method would be a defect worse
//! than the crashes it records. An environment that names no data directory leaves the
//! channel unarmed — faults still terminate the process, they just leave no file behind,
//! and the crash channel's stderr half keeps reporting.
//!
//! # What the context provider may say
//!
//! The provider answers from atomic slots alone. A panic can be caught anywhere,
//! including on a thread that holds a lock the provider would otherwise want, so taking
//! a lock here could deadlock against the very panic being described. The facts it reads
//! are single atomic loads — the candidate window's readiness flag and the platform
//! backend's identifier — and the key set they go into is closed: there is no key a
//! free-form value could be smuggled through.

use std::path::{Path, PathBuf};

use ime_diag::crash::{self, CrashContext, CrashContextKey};
use ime_types::ImeError;

use crate::ffi::emit_diagnostic;

/// Reported while this addon's logging initialiser is still an integration point.
///
/// The text is the one the step has always reported, and it stays until installing the
/// process's subscriber is safe for a *second* addon in the process: `tracing` installs
/// a global subscriber exactly once, the two addons are separate libraries with separate
/// copies of every static, and a UI addon that called the initialiser after the engine
/// claimed the subscriber would be declined for a log file that was never going to be
/// its own.
const PENDING_LOGGING: &str =
    "lifecycle/pending: diagnostics awaits the diagnostics logging initialiser";

/// The `diagnostics` step: arms the crash forensics and reports what is still pending.
///
/// # Returns
///
/// Always `Ok(())`: the arming fails soft by design, and a forensics layer that could
/// decline the addon would be a worse defect than the crashes it records.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never.
pub(super) fn init_diagnostics() -> Result<(), ImeError> {
    assemble_crash_forensics_in(crash_directory().as_deref());
    emit_diagnostic(PENDING_LOGGING);
    Ok(())
}

/// Arms the crash forensics over `crash_dir`: the four calls the crash channel deals in.
///
/// The order is the channel's own: name the directory first, so a record that arrives
/// before the rest of the arming has somewhere to go; install the panic hook; arm the
/// fault signals behind the FFI module's registrar; register the context provider. A
/// refused signal registration is reported and the step still succeeds.
///
/// The entry point a test drives: with the directory injected, the arming is
/// deterministic and no environment is read.
///
/// # Panics
///
/// Never.
fn assemble_crash_forensics_in(crash_dir: Option<&Path>) {
    let Some(dir) = crash_dir else {
        // Nowhere to write a record, and `ime-diag` deliberately has no default of its
        // own. Faults still terminate the process; they just leave no file behind, and
        // the crash channel's stderr half keeps reporting.
        emit_diagnostic(
            "lifecycle/pending: crash forensics awaits a data directory to record into",
        );
        return;
    };
    crash::set_crash_directory(dir.to_path_buf());
    ime_diag::panic::install_panic_hook(dir.to_path_buf());
    crash::set_crash_context_provider(crash_context);
    if let Err(error) =
        crash::signal::install_signal_handlers(dir, crate::ffi::register_crash_signal)
    {
        emit_diagnostic(&format!("lifecycle/step-failed: crash-forensics ({error})"));
    }
}

/// The crash directory the layout names, or `None` when the environment names no base
/// directory.
///
/// Resolved through the same layout module the engine addon uses, so that both addons'
/// records land in one directory the operator looks in once. The layout is not prepared
/// here: the channel's writers create the directory they need with the mode the privacy
/// baseline fixes.
///
/// # Panics
///
/// Never.
fn crash_directory() -> Option<PathBuf> {
    let bases = ime_dict::paths::BaseDirs::from_env().ok()?;
    let paths = ime_dict::paths::Paths::from_bases(&bases).ok()?;
    Some(paths.crash_dir)
}

/// The structural facts a crash record carries, read from atomic slots alone.
///
/// The readiness flag is the one bit of session state this library keeps in an atomic,
/// and the backend identifier is a value written once and read through a `OnceLock`, so
/// both are safe to read while a panic is being handled anywhere in the process. A value
/// that is not an identifier-shaped fact is refused by the context itself rather than
/// truncated, so the record cannot grow a free-form field.
///
/// # Panics
///
/// Never.
fn crash_context() -> CrashContext {
    let mut context = CrashContext::new();
    let state = if super::candidate_window_ready() {
        "ready"
    } else {
        "not-ready"
    };
    context.insert_identifier(CrashContextKey::SessionState, state);
    if let Some(backend) = crate::platform::backend_id() {
        context.insert_identifier(CrashContextKey::BackendId, backend);
    }
    context
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::PathBuf;

    use super::*;
    use ime_diag::crash::record::{CRASH_DIR_MODE, CRASH_FILE_MODE};

    /// A scratch root this test owns, under `/tmp` rather than `std::env::temp_dir()` so
    /// that no test depends on `$TMPDIR`.
    fn scratch_root(label: &str) -> PathBuf {
        let name = format!("rspinyin-ui-crash-{}-{label}", std::process::id());
        let root = PathBuf::from("/tmp").join(name);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("creating the scratch root");
        root
    }

    #[test]
    fn test_assemble_crash_forensics_in_arms_the_channel_over_the_given_directory() {
        let dir = scratch_root("arm");
        let crash_dir = dir.join("crash");

        assemble_crash_forensics_in(Some(&crash_dir));

        assert_eq!(
            crash::crash_directory(),
            Some(crash_dir.as_path()),
            "the records the channel writes land in the layout's crash directory"
        );
        let mode = std::fs::metadata(&crash_dir)
            .expect("the arming prepared the directory")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, CRASH_DIR_MODE, "the directory is owner-only");
        // The fault-signal channel owns its record file from the moment it is armed,
        // because a handler cannot build a path; its presence here is the observable
        // half of the arming, and its mode is the privacy baseline. The armed file is
        // the empty one: the panic hook this arming installed also writes records here
        // when anything else in the process panics, and those are never empty.
        let armed: Vec<PathBuf> = std::fs::read_dir(&crash_dir)
            .expect("listing the crash directory")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                std::fs::metadata(path)
                    .map(|meta| meta.len() == 0)
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(armed.len(), 1, "the signal channel armed one record");
        let file_mode = std::fs::metadata(&armed[0])
            .expect("the armed record's metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, CRASH_FILE_MODE, "the record is owner-only");
    }

    #[test]
    fn test_assemble_crash_forensics_in_survives_a_missing_directory() {
        // Forensics must never be the reason the addon declines: an environment that
        // names no data directory leaves the channel unarmed, the condition is reported,
        // and the step still succeeds.
        assert!(
            std::panic::catch_unwind(|| assemble_crash_forensics_in(None)).is_ok(),
            "the arming fails soft when no directory is named"
        );
    }

    #[test]
    fn test_crash_context_agrees_with_the_readiness_and_backend_slots() {
        // The provider is a mapping from atomic slots into the closed key set, so what
        // the test asserts is the mapping: the record's session state is the readiness
        // flag's state at the moment of the read, and the backend, when the process has
        // probed one, is the identifier the platform layer is holding. The flags are
        // process-wide, which is why the mapping is asserted against the slots rather
        // than against fixed values.
        let context = crash_context();

        let expected = if crate::addon::candidate_window_ready() {
            "ready"
        } else {
            "not-ready"
        };
        assert_eq!(
            context.get(CrashContextKey::SessionState),
            Some(expected),
            "the record's session state follows the readiness flag"
        );
        match crate::platform::backend_id() {
            Some(backend) => assert_eq!(
                context.get(CrashContextKey::BackendId),
                Some(backend),
                "the record names the backend the probe constructed"
            ),
            None => assert_eq!(
                context.get(CrashContextKey::BackendId),
                None,
                "a process that probed no backend records none"
            ),
        }
        // The set of keys a record may carry is closed, and the two facts above are the
        // only ones this provider fills; anything else in a context would be a key that
        // did not go through review.
        for name in context.iter().map(|(key, _)| key) {
            assert!(
                CrashContextKey::ALL.iter().any(|key| key.name() == name),
                "{name} is not a key of the closed context set"
            );
        }
    }
}
