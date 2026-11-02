//! The Xvfb channel: a private virtual display for the rendering regression tier.
//!
//! Responsibility: arrange one Xvfb server at a fixed geometry, prove it ready through the
//! real injection path, and hand out the `DISPLAY` value a sandboxed session (or any case
//! command that opens a display) has to carry. The suite, screenshot and golden flow that
//! runs on top of it is unchanged: this module owns the display and nothing else.
//!
//! # Why the geometry is fixed
//!
//! `SCREEN_GEOMETRY` pins one resolution and depth for every run of the channel. A golden
//! comparison is only ever meaningful between captures of the same geometry, and a virtual
//! display that wandered between runs would make the golden library a library of unreproducible
//! pictures. The constant is the one place the geometry is written down.
//!
//! # Readiness is the injection path, not a timer
//!
//! A display whose socket exists is not a display a case can type into. Readiness is
//! therefore proven the way the cases themselves will work: a connection is opened, the
//! XTEST extension is probed, the keyboard mapping is read, and one probe key is pressed
//! and released through the same route every injected key takes. A display that fails any
//! of those is not ready, however long it has been up, and no sleep-based timer decides
//! the question. The retry loop below polls the probe and nothing else.
//!
//! # The display listens on unix sockets where it can, on TCP where it must
//!
//! The client library the injection route goes through reaches a display over a filesystem
//! unix socket or over TCP; the abstract namespace is not one of its answers. The X
//! transport layer, in turn, only binds a filesystem socket under a directory that carries
//! the sticky bit, and serves through the abstract namespace alone when it refuses -- which
//! is exactly what happens on hosts where `/tmp/.X11-unix` was created without it (observed
//! on WSLg, where the directory is `0777`). A machine in that state would otherwise keep a
//! server alive that no probe can ever reach, so the channel looks at the directory before
//! it spawns: sticky bit present, unix sockets only, desktop parity; anything else, a TCP
//! listener is allowed and the run records the fact as an environment note.
//!
//! # The teardown invariant
//!
//! [`VirtualDisplay`] owns the server child, and dropping it stops the child and removes
//! the socket and lock files of the display number it owns. Every path out of a case -- a
//! failed assertion, an error return, an early `?` -- runs the drop, so no failure path can
//! orphan a server or leave a display number wedged for the next run. The standard library
//! kills with `SIGKILL`, which a server does not clean up after, so the file removal is
//! part of the teardown and not an optimisation.
//!
//! # Environment note, not a regression assertion
//!
//! Cases run slower on a virtual display than on a real one. That overhead is an
//! environment fact a run records beside its durations; it is never an assertion, and no
//! test here measures it.
//!
//! # What this module does not do
//!
//! It judges no case, reads no screenshot, and never touches a golden library. Whether a
//! case passed is the suite's verdict; whether a capture matches a baseline is the golden
//! channel's; this module only makes sure both have a display to run against.

// The channel is consumed by the suite's `--xvfb` flag and by the environment probe; the
// accessors a later harness driver will reach for are part of the surface today and are
// reported as dead code in a non-test build until that driver lands. The attribute goes
// away with those callers.
#![allow(dead_code, unused_imports)]

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::testd::keys::KS_SPACE;
use crate::testd::x11::X11Session;

/// What a machine without the channel's tooling has to install.
///
/// The decision table cites this verbatim for the Xvfb requirement, so a withheld case and
/// the channel's own refusal name the same fix.
pub const MISSING_REMEDY: &str = "install xvfb (Debian/Ubuntu) or xorg-x11-server-Xvfb (Fedora)";

/// The screen geometry every server of this channel runs at, as Xvfb spells it.
///
/// Fixed and known: golden comparisons are only ever made between captures of this
/// geometry, and a run that reports its screen size reports this one.
pub const SCREEN_GEOMETRY: &str = "1280x800x24";

/// How long a starting server is given before the injection probe gives up.
pub const READY_TIMEOUT: Duration = Duration::from_secs(10);

/// Interval between two probes of a starting server.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The Xvfb server binary the channel manages.
const SERVER: &str = "Xvfb";

/// The wrapper binary the detection also looks for, so a refusal can name what is missing.
const RUNNER: &str = "xvfb-run";

/// The first display number the channel tries, well above the numbers a desktop session
/// takes for itself.
const DISPLAY_BASE: u32 = 90;

/// How many display numbers the channel scans upward from [`DISPLAY_BASE`].
const DISPLAY_SPAN: u32 = 40;

/// The directory the X server sockets live in.
pub const SOCKET_DIR: &str = "/tmp/.X11-unix";

/// The directory the legacy lock files live in.
const LOCK_DIR: &str = "/tmp";

/// The variable a child must carry to land on the virtual display.
const DISPLAY_VAR: &str = "DISPLAY";

/// The key the readiness probe types. Space is on every keyboard mapping a server serves,
/// and a press-and-release pair changes no state a case could observe.
const PROBE_KEYSYM: u32 = KS_SPACE;

/// Which of the channel's tools are on `$PATH`.
///
/// The server is what the channel runs; the wrapper is detected so a refusal can say
/// exactly which piece is missing instead of one word for both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tooling {
    /// Whether `Xvfb` is on `$PATH` as an executable file.
    pub server: bool,
    /// Whether `xvfb-run` is on `$PATH` as an executable file.
    pub runner: bool,
}

impl Tooling {
    /// Reads the tooling this machine has right now.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn probe() -> Self {
        let path = std::env::var_os("PATH");
        Self {
            server: find_on_path(SERVER, path.as_deref()),
            runner: find_on_path(RUNNER, path.as_deref()),
        }
    }

    /// Reads the tooling a given `$PATH` value offers.
    ///
    /// `None` is a path that was never set, which is a machine nobody pointed at its tools.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_path(path: Option<&OsStr>) -> Self {
        Self {
            server: find_on_path(SERVER, path),
            runner: find_on_path(RUNNER, path),
        }
    }

    /// Whether the channel can run at all, which is what the environment gate asks.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_available(&self) -> bool {
        self.server
    }

    /// The tools this reading does not show, for a refusal to name.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn missing(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if !self.server {
            missing.push(SERVER);
        }
        if !self.runner {
            missing.push(RUNNER);
        }
        missing
    }
}

/// Whether `name` is an executable file on `path`.
///
/// A directory named like the tool is not the tool, and a non-executable file is not
/// either: both would fail at spawn with a less actionable error than this reading.
fn find_on_path(name: &str, path: Option<&OsStr>) -> bool {
    let Some(path) = path else {
        return false;
    };
    std::env::split_paths(path).any(|dir| is_executable_file(&dir.join(name)))
}

/// Whether `path` is a file any of the execute bits is set on.
fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Everything that can go wrong while arranging the channel.
#[derive(Debug, Error)]
pub enum XvfbError {
    /// The machine has none of the tooling the channel runs.
    #[error("the Xvfb channel has nothing to run: {missing} not on $PATH -- remedy: {remedy}")]
    ToolingMissing {
        /// The tools the detection did not find.
        missing: String,
        /// What to install, verbatim as the decision table cites it.
        remedy: &'static str,
    },
    /// Every display number in the scan range is taken.
    #[error("no free display number between :{base} and :{end}")]
    NoFreeDisplay {
        /// The first number the scan tried.
        base: u32,
        /// One past the last number the scan tried.
        end: u32,
    },
    /// The server process could not be started.
    #[error("Xvfb could not be started on :{number}: {source}")]
    Spawn {
        /// The display number the server was to take.
        number: u32,
        /// What the spawn reported.
        #[source]
        source: io::Error,
    },
    /// The server exited before the injection probe succeeded once.
    #[error("the Xvfb server on {display} exited before it was ready: {status}")]
    ServerExited {
        /// The display name the server was serving.
        display: String,
        /// The exit status the child reported.
        status: String,
    },
    /// The injection probe never succeeded within the readiness budget.
    #[error("the injection probe on {display} never succeeded within {timeout:?}")]
    NotReady {
        /// The display name the probe was typing into.
        display: String,
        /// The readiness budget that elapsed.
        timeout: Duration,
    },
}

/// The deadlines one readiness poll works against.
struct Waiting {
    /// When the poll gives up.
    deadline: Instant,
    /// How long one round of the poll waits between probes.
    interval: Duration,
    /// The budget the refusal reports.
    timeout: Duration,
}

/// One managed Xvfb server.
///
/// The display number is chosen here rather than by a wrapper, so the geometry, the
/// readiness probe and the teardown are all this channel's own decisions. Dropping the
/// value stops the server; see the [teardown invariant](self#the-teardown-invariant).
#[derive(Debug)]
pub struct VirtualDisplay {
    /// The display number, as the server was started with it.
    number: u32,
    /// The display name a child's `$DISPLAY` carries, e.g. `:90`.
    display: String,
    /// The server child, until it is stopped.
    child: Option<Child>,
    /// Where the socket of this display lives.
    socket_dir: PathBuf,
    /// Where the lock file of this display lives.
    lock_dir: PathBuf,
}

impl VirtualDisplay {
    /// Arranges a server on this machine.
    ///
    /// # Errors
    ///
    /// Returns [`XvfbError::ToolingMissing`] when no Xvfb binary is on `$PATH`,
    /// [`XvfbError::NoFreeDisplay`] when every scanned number is taken,
    /// [`XvfbError::Spawn`] when the server cannot be started. Readiness is a separate
    /// step: see [`VirtualDisplay::wait_ready`].
    ///
    /// # Panics
    ///
    /// Never.
    pub fn open() -> Result<Self, XvfbError> {
        let tooling = Tooling::probe();
        Self::arrange(Arrangement {
            tooling: &tooling,
            taken: &taken_on_this_machine,
            spawn: &spawn_server,
            socket_dir: Path::new(SOCKET_DIR),
            lock_dir: Path::new(LOCK_DIR),
        })
    }

    /// Arranges a server through the given seams.
    ///
    /// The seam is what keeps the arrangement testable without a real server: the tests
    /// name the tooling reading, the numbers already taken, the spawner and the
    /// directories, and this function decides as it always does.
    fn arrange(arrangement: Arrangement) -> Result<Self, XvfbError> {
        if !arrangement.tooling.is_available() {
            return Err(XvfbError::ToolingMissing {
                missing: arrangement.tooling.missing().join(" and "),
                remedy: MISSING_REMEDY,
            });
        }
        let Some(number) = pick_display_number(arrangement.taken, DISPLAY_BASE, DISPLAY_SPAN)
        else {
            return Err(XvfbError::NoFreeDisplay {
                base: DISPLAY_BASE,
                end: DISPLAY_BASE + DISPLAY_SPAN,
            });
        };
        let child =
            (arrangement.spawn)(number).map_err(|source| XvfbError::Spawn { number, source })?;
        Ok(Self {
            number,
            display: format!(":{number}"),
            child: Some(child),
            socket_dir: arrangement.socket_dir.to_path_buf(),
            lock_dir: arrangement.lock_dir.to_path_buf(),
        })
    }

    /// The display name a child's `$DISPLAY` carries, e.g. `:90`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn display(&self) -> &str {
        &self.display
    }

    /// The environment pair a sandboxed session has to carry to land on this display.
    ///
    /// This is the injection point: [`crate::testd::sandbox::Sandbox::fcitx5_command`]
    /// builds the session's environment, and this pair layered on top of it is what puts
    /// the session on the virtual display.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn display_env(&self) -> (String, String) {
        (DISPLAY_VAR.to_owned(), self.display.clone())
    }

    /// Waits until the real injection path works on this display.
    ///
    /// A server that exits first and a probe that never succeeds are different failures,
    /// and they are reported as different errors: the first is a server problem, the
    /// second is one this channel cannot see past.
    ///
    /// # Errors
    ///
    /// Returns [`XvfbError::ServerExited`] when the server exits first and
    /// [`XvfbError::NotReady`] when the budget elapses without one successful probe.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn wait_ready(&mut self) -> Result<(), XvfbError> {
        let waiting = Waiting {
            deadline: Instant::now() + READY_TIMEOUT,
            interval: POLL_INTERVAL,
            timeout: READY_TIMEOUT,
        };
        let display = self.display.clone();
        poll_ready(
            &display,
            &mut || probe_injection(&display),
            &mut || self.child_status(),
            waiting,
        )
    }

    /// The server's exit status, when it has already exited.
    fn child_status(&mut self) -> Option<String> {
        let status = self.child.as_mut()?.try_wait().ok().flatten()?.to_string();
        Some(status)
    }

    /// Stops the server and removes the display files of the number it owns.
    ///
    /// # Return value
    /// The exit status of the stopped child, `None` when no child was running. A killed
    /// child's status carries no exit code, which is what a caller that wants to know the
    /// teardown happened checks for.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn stop(&mut self) -> Option<ExitStatus> {
        let status = self.child.as_mut().and_then(|child| {
            let _ = child.kill();
            child.wait().ok()
        });
        self.child = None;
        remove_display_files(&self.socket_dir, &self.lock_dir, self.number);
        status
    }
}

impl Drop for VirtualDisplay {
    fn drop(&mut self) {
        // The teardown invariant: every path out of a case runs this, so no failure path
        // can orphan a server or leave its display number wedged for the next run.
        let _ = self.stop();
    }
}

/// The seams [`VirtualDisplay::arrange`] works through.
struct Arrangement<'a> {
    /// The tooling reading the availability decision is made from.
    tooling: &'a Tooling,
    /// Whether a display number is already taken.
    taken: &'a dyn Fn(u32) -> bool,
    /// How the server is started.
    spawn: &'a dyn Fn(u32) -> io::Result<Child>,
    /// Where the socket of the display lives.
    socket_dir: &'a Path,
    /// Where the lock file of the display lives.
    lock_dir: &'a Path,
}

/// Waits for one successful injection probe, bounded by a deadline.
///
/// The probe is the readiness signal and nothing else is: a display that becomes ready
/// early is never kept waiting, and one that never becomes ready is bounded by the
/// deadline alone. A server that dies while it is being probed is reported before the
/// probe runs, so an exit is never read as "not ready yet".
fn poll_ready<F, G>(
    display: &str,
    probe: &mut F,
    exited: &mut G,
    waiting: Waiting,
) -> Result<(), XvfbError>
where
    F: FnMut() -> bool,
    G: FnMut() -> Option<String>,
{
    loop {
        if let Some(status) = exited() {
            return Err(XvfbError::ServerExited {
                display: display.to_owned(),
                status,
            });
        }
        if probe() {
            return Ok(());
        }
        if Instant::now() >= waiting.deadline {
            return Err(XvfbError::NotReady {
                display: display.to_owned(),
                timeout: waiting.timeout,
            });
        }
        // A harness polling a server it started, not a plugin callback: the sleep is the
        // wait itself, and the deadline above bounds it.
        thread::sleep(waiting.interval);
    }
}

/// One key press and release through the same route every injected key takes.
///
/// A probe key is "consumed" when the whole route answers without an error: the connection
/// opens, the server reports a usable XTEST extension, the keyboard mapping carries the
/// probe key, and both halves of the press and release are flushed. That is the readiness
/// the cases need -- a timer could say none of it.
fn probe_injection(display: &str) -> bool {
    let Ok(session) = X11Session::connect(Some(display)) else {
        return false;
    };
    let Ok(keycode) = session.keycode(PROBE_KEYSYM) else {
        return false;
    };
    if session.fake_key(keycode, true).is_err() {
        return false;
    }
    session.fake_key(keycode, false).is_ok()
}

/// The first display number in `[base, base + span)` that `taken` does not hold.
///
/// # Panics
///
/// Never.
fn pick_display_number(taken: &dyn Fn(u32) -> bool, base: u32, span: u32) -> Option<u32> {
    (base..base + span).find(|number| !taken(*number))
}

/// Whether the machine already serves `number`.
fn taken_on_this_machine(number: u32) -> bool {
    display_is_taken(Path::new(SOCKET_DIR), Path::new(LOCK_DIR), number)
}

/// Whether the socket or the lock file of `number` exists under the given directories.
///
/// Both files claim the number: a socket left by a killed server and a lock file left by
/// the same are equally good reasons to scan past it.
///
/// # Panics
///
/// Never.
fn display_is_taken(socket_dir: &Path, lock_dir: &Path, number: u32) -> bool {
    socket_path(socket_dir, number).exists() || lock_path(lock_dir, number).exists()
}

/// The socket file of display `number` under `dir`.
///
/// # Panics
///
/// Never.
fn socket_path(dir: &Path, number: u32) -> PathBuf {
    dir.join(format!("X{number}"))
}

/// The lock file of display `number` under `dir`.
///
/// # Panics
///
/// Never.
fn lock_path(dir: &Path, number: u32) -> PathBuf {
    dir.join(format!(".X{number}-lock"))
}

/// Starts one Xvfb server on `number` at the fixed geometry.
///
/// The listeners follow [`unix_socket_servable`]: where the machine's X socket directory
/// can serve unix sockets the server is desktop parity and listens on unix only; where it
/// cannot, a TCP listener is allowed so the injection route has a path the client library
/// can reach.
fn spawn_server(number: u32) -> io::Result<Child> {
    let mut command = Command::new(SERVER);
    command.arg(format!(":{number}"));
    command.args(["-screen", "0", SCREEN_GEOMETRY]);
    if unix_socket_servable(Path::new(SOCKET_DIR)) {
        command.arg("-nolisten").arg("tcp");
    } else {
        command.arg("-listen").arg("tcp");
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.spawn()
}

/// Whether the machine's X socket directory can serve a unix socket at all.
///
/// The X transport layer binds a filesystem socket only under a directory carrying the
/// sticky bit; under anything else it warns, refuses to bind, and serves through the
/// abstract namespace alone -- which the injection route's client library cannot reach.
/// The observed shape is a WSLg host, whose `/tmp/.X11-unix` is `0777` and owned by root,
/// so no user process can repair it either. Only the exact directory the transport layer
/// asks for counts as servable: anything else falls back to a TCP listener.
fn unix_socket_servable(socket_dir: &Path) -> bool {
    fs::metadata(socket_dir).is_ok_and(|meta| meta.permissions().mode() & 0o7777 == 0o1777)
}

/// The environment note a run records when the display has to listen on TCP as well.
///
/// The note is a fact about the machine, never an assertion: the channel's own timings stay
/// environment notes, and so does this.
///
/// # Panics
///
/// Never.
pub fn tcp_fallback_note(socket_dir: &Path) -> Option<&'static str> {
    (!unix_socket_servable(socket_dir)).then_some(
        "the X socket directory cannot serve unix sockets, so the display also listens on TCP",
    )
}

/// Removes the socket and the lock file of display `number`.
///
/// Teardown must never fail, and a file that is already gone is not a failure: both
/// removals are best-effort on purpose.
fn remove_display_files(socket_dir: &Path, lock_dir: &Path, number: u32) {
    let _ = fs::remove_file(socket_path(socket_dir, number));
    let _ = fs::remove_file(lock_path(lock_dir, number));
}

/// The environment a channel-backed run injects into every case command.
///
/// `Some` carries the one pair that puts a child on the virtual display, and it overrides
/// whatever display the session itself had: a developer's `$DISPLAY` must not leak into a
/// run that asked for the virtual one. `None` is a run without the channel, which injects
/// nothing and leaves every case command exactly as it was.
///
/// # Panics
///
/// Never.
pub fn channel_env(display: Option<&str>) -> Vec<(String, String)> {
    display
        .map(|name| vec![(DISPLAY_VAR.to_owned(), name.to_owned())])
        .unwrap_or_default()
}

// The tests live in a sibling file rather than inside this one: together they would be
// longer than the file limit allows. A `#[path]` child module keeps them inside `xvfb`.
#[cfg(test)]
#[path = "xvfb/tests.rs"]
mod tests;
