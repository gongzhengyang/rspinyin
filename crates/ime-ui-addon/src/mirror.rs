//! The test-only frame mirror: what the engine's sink received, on disk.
//!
//! This is the publish half of the E2E verification channel: every `Frame` the sink
//! accepts can land in a file the harness reads back and asserts against, which is what
//! turns "the window drew something" from an inference into an observation. The module
//! exists only under the `test-mirror` feature; a shipped addon carries none of it.
//!
//! # The privacy gate
//!
//! A mirror frame carries user input — preedit text, candidate text — so the default is
//! zero publishing, and the switch is an explicit environment variable the test harness
//! sets, not a directory the code happens to find. The directory must live under
//! `$XDG_RUNTIME_DIR` (tmpfs, wiped on reboot); a variable pointing anywhere else
//! refuses to arm and records why, because "the directory existed" is not an acceptable
//! story for how user input reached a disk.
//!
//! # Format authority
//!
//! The snapshot format is [`ime_diag::uiframe`]'s, the same codec the harness reads
//! with. There is exactly one definition of the file shape in the workspace.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use ime_diag::uiframe::{FRAME_FILE, FrameSnapshot};
use ime_types::UiFrame;

use crate::ffi::emit_diagnostic;

/// The environment variable that arms the mirror. Set by the harness, never by the
/// plugin's own configuration.
const MIRROR_DIR_ENV: &str = "RSPINYIN_TEST_MIRROR_DIR";

/// The sandbox root the mirror directory has to live under.
const SANDBOX_ROOT_ENV: &str = "XDG_RUNTIME_DIR";

/// What one publish decision found, for the arming diagnostic.
enum ArmState {
    /// The variable is unset: the plugin runs as shipped, no disk work at all.
    NotRequested,
    /// The variable is set but names a directory outside the sandbox root.
    OutsideSandbox,
    /// Armed and writing.
    Armed(PathBuf),
}

static ARM: OnceLock<ArmState> = OnceLock::new();
/// The revision the mirror file holds, plus one; `0` means nothing written yet. The
/// offset is why a legal revision `0` cannot double as the sentinel.
static HELD: AtomicU64 = AtomicU64::new(0);
static FAILURES: AtomicU64 = AtomicU64::new(0);

/// Resolves the arm state once, on the first frame.
fn arm() -> &'static ArmState {
    ARM.get_or_init(|| {
        let Some(dir) = std::env::var_os(MIRROR_DIR_ENV) else {
            return ArmState::NotRequested;
        };
        let Some(sandbox) = std::env::var_os(SANDBOX_ROOT_ENV) else {
            emit_diagnostic("mirror/sandbox-unresolvable");
            return ArmState::OutsideSandbox;
        };
        let dir = PathBuf::from(&dir);
        let sandbox = PathBuf::from(&sandbox);
        if !dir.starts_with(&sandbox) {
            // One line, and only on the frames that follow: a mirror pointed outside
            // the tmpfs sandbox would be the one place user input could outlive a
            // reboot, and that refusal is worth being able to grep for.
            emit_diagnostic("mirror/refused-outside-sandbox");
            return ArmState::OutsideSandbox;
        }
        ArmState::Armed(dir)
    })
}

/// Publishes one frame to the mirror, when the mirror is armed.
///
/// Never fails the caller: a failed write is counted and the next frame tries again.
/// A frame whose revision is not newer than the file's is skipped, matching the
/// harness-side mirror's newest-frame slot semantics.
pub fn publish(frame: &UiFrame) {
    let ArmState::Armed(dir) = arm() else {
        return;
    };
    let seen = u64::from(frame.revision) + 1;
    let held = HELD.load(Ordering::Acquire);
    if seen <= held {
        return;
    }
    let Ok(text) = FrameSnapshot::of(frame).to_json() else {
        FAILURES.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let path = dir.join(FRAME_FILE);
    let temp = dir.join(format!("{FRAME_FILE}.writing"));
    // The mirror directory is the harness's sandbox; creating it here (0700, the file
    // 0600) keeps the plugin from depending on setup order in a fresh container.
    if std::fs::create_dir_all(dir).is_err() {
        FAILURES.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(text.as_bytes())?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, &path)
    };
    match write() {
        Ok(()) => {
            HELD.store(seen, Ordering::Release);
        }
        Err(_) => {
            FAILURES.fetch_add(1, Ordering::Relaxed);
            let _ = std::fs::remove_file(&temp);
        }
    }
}

/// The mirror directory the arm state resolved, for the harness's own assertions.
pub fn armed_dir() -> Option<&'static PathBuf> {
    match arm() {
        ArmState::Armed(dir) => Some(dir),
        _ => None,
    }
}
