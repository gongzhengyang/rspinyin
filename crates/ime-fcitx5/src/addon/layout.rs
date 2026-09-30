//! Where the plugin keeps what it owns, for the length of one process.
//!
//! Responsibility: run the `data-dirs` step — resolve the XDG layout, create the
//! directories the plugin owns and report what the preparation pass found — and keep that
//! layout for the steps that read a file out of it. Two files are named from here: the
//! compiled dictionary the sessions decode against, and the probe snapshot a `SIGUSR1`
//! asks for.
//!
//! Boundaries: the layout itself is `ime_dict::paths`'s, down to the modes and the
//! read-only degradation; this module adds the process-wide slot and the reporting. It
//! opens no file and reads no configuration.
//!
//! # Read-only degradation
//!
//! `ASM-15`: a directory that cannot be created, or a path that is not one, is reported
//! and the plugin carries on. The layout is still recorded — it names the dictionary,
//! which is read-only anyway — and [`ime_dict::paths::is_readonly_mode`] is what the
//! status strip renders its lock from. A user whose data directory is unwritable keeps
//! their input method and loses only the learning.

use std::path::PathBuf;
use std::sync::Mutex;

use ime_dict::paths::{BaseDirs, Notice, Paths, ensure_dirs_in};
use ime_types::ImeError;

use crate::ffi::emit_diagnostic;

use super::lock;
use super::report_step_failure;

/// The name the compiled dictionary is installed under inside the data directory.
///
/// `packaging/install.sh` copies the compiled `base.dict` into `<datadir>/rspinyin/`, which
/// is [`Paths::data_dir`]. The leaf name is the one file of this layout `ime_dict::paths`
/// does not name, because it is the installer's payload rather than a file the plugin
/// creates.
const DICTIONARY_FILE: &str = "base.dict";

/// The name of the probe snapshot inside the data directory.
///
/// Taken from `ime-diag` rather than spelled again here: the file the plugin writes and the
/// file `xtask budget --memory` reads are one name, and a second literal would be a second
/// place it could drift.
const SNAPSHOT_FILE_NAME: &str = ime_diag::probe::SNAPSHOT_FILE_NAME;

/// The layout this process writes to: installed by the `data-dirs` step.
///
/// A lock over a slot rather than a `OnceLock` because the slot is written by one step and
/// read by later ones, and because a test installs a layout of its own without an
/// environment to arrange. The lock is a leaf: it is never held across a diagnostic write.
static LAYOUT: Mutex<Option<Paths>> = Mutex::new(None);

/// The `data-dirs` step: prepares the directories the plugin owns.
///
/// # Returns
///
/// `Ok(())` whether or not the layout could be prepared: a failure is reported through
/// [`report_step_failure`] and the sequence carries on.
///
/// # Errors
///
/// None. The step is not fatal by policy — a directory that cannot be created is never a
/// reason for the user to lose their input method.
///
/// # Panics
///
/// Never.
pub(super) fn prepare_data_dirs() -> Result<(), ImeError> {
    match BaseDirs::from_env() {
        Ok(bases) => prepare_data_dirs_in(&bases),
        Err(error) => {
            // The environment names no base directory at all, so there is no layout to
            // hand out. `paths` has already set the process-wide read-only flag, and the
            // error carries `data/readonly-mode`.
            report_step_failure("data-dirs", &error);
            Ok(())
        }
    }
}

/// Prepares and installs the layout under `bases`; the entry point a test drives, since
/// with the bases injected the directories and their modes are deterministic.
///
/// # Arguments
///
/// * `bases` — the configuration and data base directories the layout is derived from.
///
/// # Returns
///
/// `Ok(())` always: a preparation pass that degraded is reported, never propagated.
///
/// # Errors
///
/// None, as [`prepare_data_dirs`].
///
/// # Panics
///
/// Never.
pub(super) fn prepare_data_dirs_in(bases: &BaseDirs) -> Result<(), ImeError> {
    match ensure_dirs_in(bases) {
        Ok(layout) => {
            report_notices(layout.notices());
            install_layout(layout);
        }
        Err(error) => report_step_failure("data-dirs", &error),
    }
    Ok(())
}

/// Installs `layout` as the layout this process writes to.
///
/// The last install wins, unlike the store and session slots: a layout is a value every
/// reader takes a copy of rather than a handle anything holds across a call, so replacing
/// it cannot leave a reader with a layout nothing else knows about.
///
/// # Panics
///
/// Never.
pub(super) fn install_layout(layout: Paths) {
    *lock(&LAYOUT) = Some(layout);
}

/// The layout this process writes to, or `None` before the `data-dirs` step ran.
///
/// # Panics
///
/// Never.
pub(super) fn layout() -> Option<Paths> {
    lock(&LAYOUT).clone()
}

/// The path the compiled dictionary is installed at, or `None` when the layout is unknown.
///
/// # Panics
///
/// Never.
pub(super) fn dictionary_path() -> Option<PathBuf> {
    layout().map(|paths| paths.data_dir.join(DICTIONARY_FILE))
}

/// The path the probe snapshot is written to, or `None` when the layout is unknown.
///
/// The directory the rest of the layout lives in, so that the file the plugin writes and
/// the file `xtask budget --memory` reads are one path by construction.
///
/// # Panics
///
/// Never.
pub(super) fn snapshot_path() -> Option<PathBuf> {
    layout().map(|paths| paths.data_dir.join(SNAPSHOT_FILE_NAME))
}

/// Reports what the preparation pass found, one line per notice.
///
/// The code stays the line's first field, so a grep for `data/readonly-mode` matches
/// whether the line came from here, from the user store or from the log sink.
///
/// # Panics
///
/// Never.
fn report_notices(notices: &[Notice]) {
    for notice in notices {
        emit_diagnostic(&format!(
            "{}: {} ({})",
            notice.code,
            notice.path.display(),
            notice.detail
        ));
    }
}
