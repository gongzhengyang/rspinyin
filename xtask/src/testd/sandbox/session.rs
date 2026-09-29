//! The Fcitx5 child: how it is started, how its readiness is read, and how a run proves it
//! loaded the sandbox's own build.
//!
//! Responsibility: own the child process, decide when a restarted session is ready, and
//! refuse to report a session that came up on somebody else's addon. It also parses the
//! two text formats the answer comes from -- a session log and a process memory map -- as
//! pure functions, so both are exercised without a display server or a running Fcitx5.
//!
//! Boundaries: it never writes into the sandbox tree beyond the log it archives, and it
//! never reads the plugin's configuration or its data. It does not decide what "ready"
//! means for a case; it decides only whether the addons the sandbox staged are there.
//!
//! # Why the log and not `fcitx5-diagnose`
//!
//! The obvious oracle does not work here. Measured on Fcitx5 5.1.7, `fcitx5-diagnose`
//! hardcodes `/usr/share/fcitx5/addon` as the addon configuration directory and three
//! library directories as the search path, and it never prints a per-addon load verdict at
//! all -- its `Addon Libraries:` section only reports whether a file can be found and
//! whether `ldd` resolves. Pointed at a sandbox it therefore describes the *system*
//! installation, which is exactly the build a sandbox exists to avoid. Readiness is
//! therefore taken from the child's own log, whose lines are produced by the session this
//! sandbox started and by nothing else.
//!
//! # Why the memory map
//!
//! A log line names an addon, not a file, so "the addon loaded" does not by itself say
//! *which* library loaded. Fcitx5 resolves `Library=` through its own `StandardPath`
//! (`FCITX_ADDON_DIRS`), not through the dynamic loader's search path, so the environment
//! is what decides and the memory map is what confirms it: `/proc/<pid>/maps` names every
//! file the process has mapped, and the addon library must be one of them, under the
//! sandbox root. This is the only offline proof that the run exercised the build under
//! test rather than the copy installed system-wide.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::reset::files_with_extension;
use super::{Sandbox, SandboxError};

/// Time a restarted session is given to report its addons loaded.
pub const READY_TIMEOUT: Duration = Duration::from_secs(5);

/// Interval between two reads of the session log while waiting for readiness.
///
/// A poll rather than a blocking read because the child writes to a file and not to a
/// pipe: nothing here has to own a descriptor, and a file survives the child.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Number of attempts one session start is given.
///
/// The second attempt is the documented recovery for a bus or socket left behind by the
/// first, and uses a bus address of its own.
const START_ATTEMPTS: u32 = 2;

/// The addon library name the packaged descriptor declares.
///
/// Used only to recognise a stale installation, never to decide where a file is installed:
/// the installer derives that from `pkg-config`.
const ADDON_LIBRARY: &str = "librspinyin.so";

/// Addon library directories a standard installation searches.
///
/// Mirrors the list `fcitx5-diagnose` itself hardcodes, and is used only to *detect* a
/// stale copy so that a run can say it saw one. It is never an installation target.
const STANDARD_ADDON_DIRS: [&str; 3] = [
    "/usr/lib/fcitx5",
    "/usr/local/lib/fcitx5",
    "/usr/lib/x86_64-linux-gnu/fcitx5",
];

/// The message Fcitx5 logs when an addon has loaded.
///
/// This and the three templates below were read out of `libFcitx5Core.so.5.1.7` on the
/// development machine (`addonloader.cpp`, `addonmanager.cpp`). They are matched as
/// substrings, so the log's own date and level prefix -- a build-time setting -- does not
/// matter. `Could not load addon ` cannot be confused with `Loaded addon ` because the
/// match is case sensitive.
const LOADED_ADDON: &str = "Loaded addon ";

/// The messages Fcitx5 logs when an addon did not load.
const FAILED_ADDON: [&str; 3] = [
    "Could not load addon ",
    "Failed to load library for addon ",
    "Failed to create addon: ",
];

/// What one session's log said about its addons.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Readiness {
    /// Addon names the session reported as loaded, in the order it reported them.
    loaded: Vec<String>,
    /// Addon names the session reported as failing to load, in the order it reported them.
    failed: Vec<String>,
}

impl Readiness {
    /// The addon names the session reported as loaded.
    pub fn loaded(&self) -> &[String] {
        &self.loaded
    }

    /// The addon names the session reported as failing to load.
    pub fn failed(&self) -> &[String] {
        &self.failed
    }

    /// Whether the session reported `name` as loaded.
    pub fn is_loaded(&self, name: &str) -> bool {
        self.loaded.iter().any(|loaded| loaded == name)
    }

    /// The names among `required` the session did not report as loaded.
    ///
    /// A missing addon is reported by name rather than folded into a timeout: the two
    /// addons the architecture calls for are packaged separately, and a run that loaded
    /// only one has to say which one it lost.
    pub fn missing(&self, required: &[String]) -> Vec<String> {
        required
            .iter()
            .filter(|name| !self.is_loaded(name.as_str()))
            .cloned()
            .collect()
    }
}

/// Reads the addon verdicts out of a Fcitx5 session log.
///
/// # Parameters
/// - `log`: the text the session wrote to its standard output and standard error.
///
/// # Return value
/// The addons the log reports as loaded and as failed. An addon the log never mentions
/// appears in neither list, which is what makes a session that never reached the addon
/// loader distinguishable from one that refused the addon.
pub fn parse_addons(log: &str) -> Readiness {
    let mut readiness = Readiness::default();
    for line in log.lines() {
        if let Some(name) = named_after(line, LOADED_ADDON) {
            push_unique(&mut readiness.loaded, name);
            continue;
        }
        for template in FAILED_ADDON {
            if let Some(name) = named_after(line, template) {
                push_unique(&mut readiness.failed, name);
                break;
            }
        }
    }
    readiness
}

/// The token that follows `template` on `line`, when the line carries it.
///
/// The trailing punctuation is trimmed because the message is written by a stream
/// formatter that may append a separator; a name is alphanumeric plus `-` and `_`, which
/// is what an addon id may contain.
fn named_after(line: &str, template: &str) -> Option<String> {
    let rest = line.split_once(template)?.1;
    let name = rest.split_whitespace().next()?;
    let name = name.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
    if name.is_empty() {
        None
    } else {
        Some(name.to_owned())
    }
}

/// Appends `name` to `names` unless it is already there.
fn push_unique(names: &mut Vec<String>, name: String) {
    if !names.contains(&name) {
        names.push(name);
    }
}

/// The paths a `/proc/<pid>/maps` listing maps from below `root`.
///
/// # Parameters
/// - `text`: the contents of a process's memory map.
/// - `root`: the only directory a mapping may come from.
///
/// # Return value
/// The distinct mapped paths under `root` whose name ends in `.so`, in sorted order. A
/// pathname holding a space is not recoverable from this format and is dropped, which only
/// ever makes the answer smaller and never invents a mapping.
pub fn parse_maps(text: &str, root: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = text
        .lines()
        .filter_map(|line| line.split_whitespace().nth(5))
        .map(PathBuf::from)
        .filter(|path| path.starts_with(root))
        .filter(|path| path.extension() == Some(std::ffi::OsStr::new("so")))
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

impl Sandbox {
    /// The archived session log.
    pub fn log_path(&self) -> PathBuf {
        self.harness_dir().join(super::SESSION_LOG)
    }

    /// The addon ids a restarted session is required to report as loaded.
    pub fn required_addons(&self) -> &[String] {
        &self.required
    }

    /// The addon libraries the sandbox has staged, in sorted order.
    ///
    /// # Errors
    /// Returns [`SandboxError::Io`] when the library directory cannot be read.
    pub fn staged_libraries(&self) -> Result<Vec<PathBuf>, SandboxError> {
        files_with_extension(&self.addon_dir, "so")
    }

    /// The system-wide copies of the addon library this sandbox shadows.
    ///
    /// Reported rather than fatal: the environment is what decides the lookup, and this is
    /// the list an operator needs to see when a case behaves like the installed build.
    pub fn shadowed_system_addons(&self) -> Vec<PathBuf> {
        STANDARD_ADDON_DIRS
            .iter()
            .map(|dir| Path::new(dir).join(ADDON_LIBRARY))
            .filter(|path| path.is_file())
            .collect()
    }

    /// Fails when a session started here could only load someone else's build.
    ///
    /// # Errors
    /// Returns [`SandboxError::NoAddonStaged`] when the sandbox's library directory holds
    /// no library at all, which is the state in which a session would silently exercise the
    /// installed copy.
    pub fn check_isolation(&self) -> Result<(), SandboxError> {
        if self.staged_libraries()?.is_empty() {
            return Err(SandboxError::NoAddonStaged {
                dir: self.addon_dir.clone(),
            });
        }
        if let Some(shadowed) = self.shadowed_system_addons().first() {
            println!(
                "testd: an addon is installed at {}; the sandbox build is the one that loads",
                shadowed.display()
            );
        }
        Ok(())
    }

    /// The shared objects the running session has mapped from inside the sandbox.
    ///
    /// # Errors
    /// Returns [`SandboxError::Io`] when the running child's memory map cannot be read. An
    /// empty list means the session mapped nothing from the sandbox, which
    /// [`Sandbox::restart_fcitx5`] treats as a foreign build.
    pub fn mapped_addon_libraries(&self) -> Result<Vec<PathBuf>, SandboxError> {
        let Some(child) = self.child.as_ref() else {
            return Ok(Vec::new());
        };
        let maps = PathBuf::from(format!("/proc/{}/maps", child.id()));
        let text =
            fs::read_to_string(&maps).map_err(|source| SandboxError::Io { path: maps, source })?;
        Ok(parse_maps(&text, &self.root))
    }

    /// Restarts Fcitx5 against this sandbox and waits for its addons to load.
    ///
    /// Readiness is the child's own log reporting every staged addon loaded -- not a fixed
    /// sleep, which would make the harness a race with the machine it runs on. A session
    /// that does not come up is retried once with a bus address of its own, because the
    /// first attempt's failure is usually a bus the previous attempt left behind.
    ///
    /// # Return value
    /// What the session reported, once every staged addon is loaded and its library is
    /// mapped from inside the sandbox.
    ///
    /// # Errors
    /// Returns [`SandboxError::NoAddonStaged`] when nothing was staged,
    /// [`SandboxError::FcitxSpawn`] when the binary cannot be started,
    /// [`SandboxError::FcitxExited`] when it exits first,
    /// [`SandboxError::FcitxAddonFailed`] when it reports a staged addon as failed,
    /// [`SandboxError::FcitxNotReady`] when the deadline passes, and
    /// [`SandboxError::ForeignAddonLoaded`] when the session did not map its addon library
    /// from inside the sandbox. Every one of them leaves the sandbox directory in place,
    /// with the session log archived under [`Sandbox::log_path`].
    pub fn restart_fcitx5(&mut self) -> Result<Readiness, SandboxError> {
        self.stop_fcitx5();
        self.check_isolation()?;
        let mut attempt = 0;
        loop {
            match self.start_and_wait(attempt) {
                Ok(readiness) => {
                    println!(
                        "testd: session ready, addons loaded: {:?}",
                        readiness.loaded()
                    );
                    return Ok(readiness);
                }
                Err(error) if attempt + 1 >= START_ATTEMPTS => return Err(error),
                Err(first) => {
                    println!(
                        "testd: session start failed ({first}); retrying with another bus address"
                    );
                }
            }
            attempt += 1;
        }
    }

    /// Stops the session this sandbox started, if one is running.
    ///
    /// Never fails: a child that has already exited is reaped and a child that refuses to
    /// die is left to the operating system rather than turned into an error a case has to
    /// handle.
    pub fn stop_fcitx5(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Starts one session and waits for it to report its addons.
    fn start_and_wait(&mut self, attempt: u32) -> Result<Readiness, SandboxError> {
        let log = self.log_path();
        self.resolve(&log)?;
        let file = File::create(&log).map_err(|source| SandboxError::Io {
            path: log.clone(),
            source,
        })?;
        let errors = file.try_clone().map_err(|source| SandboxError::Io {
            path: log.clone(),
            source,
        })?;
        let mut command = self.fcitx5_command();
        command
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                bus_address(&self.runtime_home, attempt),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::from(errors));
        let child = command
            .spawn()
            .map_err(|source| SandboxError::FcitxSpawn { source })?;
        self.child = Some(child);
        let readiness = self.wait_for_ready(&log)?;
        self.assert_own_libraries_mapped()?;
        Ok(readiness)
    }

    /// Polls the session log until every staged addon is loaded or the deadline passes.
    fn wait_for_ready(&mut self, log: &Path) -> Result<Readiness, SandboxError> {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if let Some(status) = self.exited()? {
                return Err(SandboxError::FcitxExited {
                    status: status.to_string(),
                    log: log.to_path_buf(),
                });
            }
            let readiness = parse_addons(&read_text_lenient(log));
            let missing = readiness.missing(&self.required);
            if missing.is_empty() {
                return Ok(readiness);
            }
            // A refusal is not worth waiting out: the log has already said why the addon is
            // not there, and the deadline would only delay the same answer.
            if let Some(failed) = readiness
                .failed()
                .iter()
                .find(|name| missing.contains(*name))
            {
                return Err(SandboxError::FcitxAddonFailed {
                    addon: failed.clone(),
                    log: log.to_path_buf(),
                });
            }
            if Instant::now() >= deadline {
                self.stop_fcitx5();
                return Err(SandboxError::FcitxNotReady {
                    missing,
                    timeout: READY_TIMEOUT,
                    log: log.to_path_buf(),
                });
            }
            // A test harness polling a child's log, not a plugin callback: the sleep is the
            // wait itself, and the deadline above bounds it.
            thread::sleep(POLL_INTERVAL);
        }
    }

    /// The exit status of the running child, when it has already exited.
    fn exited(&mut self) -> Result<Option<ExitStatus>, SandboxError> {
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        child
            .try_wait()
            .map_err(|source| SandboxError::FcitxWait { source })
    }

    /// Fails when the session did not map every library the sandbox staged.
    fn assert_own_libraries_mapped(&self) -> Result<(), SandboxError> {
        let loaded = self.mapped_addon_libraries()?;
        let staged = self.staged_libraries()?;
        if !staged.is_empty() && staged.iter().all(|path| loaded.contains(path)) {
            return Ok(());
        }
        Err(SandboxError::ForeignAddonLoaded {
            loaded,
            root: self.root.clone(),
        })
    }
}

/// The bus address the `attempt`-th session start uses.
///
/// Each attempt gets a path of its own under the sandbox's runtime directory, so a bus left
/// behind by a previous attempt cannot be mistaken for the current one.
///
/// Visible to the parent module so that the naming rule -- the whole of the retry's recovery
/// -- is pinned by a test without a Fcitx5 session to start.
pub(super) fn bus_address(runtime_home: &Path, attempt: u32) -> String {
    format!(
        "unix:path={}",
        runtime_home.join(format!("bus.{attempt}")).display()
    )
}

/// Reads `path` as text, tolerating a file that is being written and a file that is gone.
///
/// A session log is read while the child is still appending to it, so a partial character
/// at the end is expected and a missing file means the child has not started writing yet.
/// Neither is an error: both mean "no verdict yet".
fn read_text_lenient(path: &Path) -> String {
    match fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => String::new(),
    }
}
