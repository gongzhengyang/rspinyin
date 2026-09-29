//! The `runtime://logs` channel: what the plugin and the host wrote down, as a case reads it.
//!
//! Responsibility: follow the diagnostics log the plugin keeps -- across the rolls the writer
//! performs -- hand a case the lines that appeared since it last looked, and answer the three
//! questions a case is really asking of a log: did anything leak, was every error code one the
//! contract declares, and is the log readable by its owner alone.
//!
//! # The three streams the channel covers, and the one it does not
//!
//! `features-test.md` 0.4 lists three outputs behind this resource. Two of them are files this
//! module can follow, and one is not a file at all:
//!
//! | Stream | Where it is | How it is read |
//! |---|---|---|
//! | The plugin's structured log | `$XDG_DATA_HOME/rspinyin/logs/rspinyin.log` | [`LogTap::in_sandbox`] |
//! | Fcitx5's own stdout and stderr | the sandbox's `harness/fcitx5.log` | [`LogTap::at`] on [`Sandbox::log_path`] |
//! | The audit scripts' output | the scripts' own standard output | **not tapped here** |
//!
//! The third one is deliberately absent rather than approximated: the audit scripts are run by
//! `just ci`, they write to the terminal that invoked them, and this harness never starts them.
//! A tap that claimed to cover them would be reporting a stream nothing routed to it. What a
//! case can do instead is archive the text it captured itself through [`LogTap::at`], which is
//! the same machinery on a different path.
//!
//! [`Sandbox::log_path`]: super::sandbox::Sandbox::log_path
//!
//! # Where the log really is
//!
//! The path is `$XDG_DATA_HOME/rspinyin/logs/rspinyin.log`, and the directory name is
//! `ime-dict`'s to state: [`LogTap::in_sandbox`] asks the plugin's own layout for `log_dir`
//! rather than spelling `logs` a second time. The file's *name* comes from
//! [`ime_diag::log::LOG_FILE_NAME`], which is where the writer states it. Neither is copied
//! here, so a layout change moves both sides at once.
//!
//! # Rolling, and why a tap follows it
//!
//! The writer rolls the active file over once it passes its size limit, renaming
//! `rspinyin.log` to `rspinyin.log.1` and shifting the history up. A tap that kept reading the
//! path would see an empty file and conclude that nothing more had been logged, which is the
//! one failure mode that makes a log-based assertion pass for the wrong reason. The cursor
//! therefore tracks the *file* -- its device and inode -- rather than the name, and a reader
//! that finds a different file under the same name starts again from its beginning.
//!
//! A roll is also why [`LogTap::assert_absent`] reads the whole family from disk rather than
//! the lines this tap happens to have handed out: content that rolled into a sibling would
//! otherwise escape the privacy assertion, which is the one assertion that must not have a
//! blind spot.
//!
//! # The two readers, and why they disagree about the last line
//!
//! [`LogTap::drain`] withholds an unterminated tail, because a case that asserts on a line the
//! writer is still producing would be asserting on half of it. The assertions read the family
//! whole and keep that tail: a process killed mid-write leaves its last line without a
//! terminator, and content that escaped the privacy scan is the one outcome an assertion may
//! not have. The two answers are deliberately different, and the difference is only ever the
//! last line of the active file.
//!
//! # The mode of the family
//!
//! [`LogTap::assert_private_permissions`] is the third assertion, and the one the other two
//! cannot make: a log whose content is redacted perfectly is still a leak if a second account
//! can open the file. It checks the mode of every path of the family, the rolled siblings
//! included, against the modes the plugin's own layout fixes -- see [`PrivatePath`].
//!
//! # What this module never does
//!
//! It never writes to the log directory, never starts a process and opens no socket. It never
//! resolves the log path from the environment on its own: the directory comes from the
//! sandbox, so a harness cannot read -- or point a case at -- the operator's real log. And no
//! error it raises carries the text of a log line: a line can hold what the user typed, which
//! is exactly what the privacy assertion exists to catch, and an error message is printed into
//! a terminal and archived as evidence. A finding names the path, the line within it where the
//! rule was about a line, and the rule that was broken, and stops there.
//!
//! # Modules
//!
//! [`rotation`] holds the file side -- the family, the cursor and the identity rule --
//! [`scan`] reads one line, [`codes`] builds the frozen error-code set from the documents that
//! declare it, and [`error`] holds the refusals.

// The channel is exercised by the tests below and by nothing else yet: the case runner that
// would open a tap, the evidence archiver of `GUARD-04` and the subcommand tree all live in
// files this module does not own. Until that wiring lands, every item here is reported as dead
// code in a non-test build, and the attribute goes away with those lines.
//
// `unused_imports` is covered by the same reasoning and for the same reason: the `pub use`
// lines below are this module's surface, and a `pub use` in a *binary* crate is "unused"
// whenever nothing in the crate names it.
#![allow(dead_code, unused_imports)]

mod codes;
mod error;
mod rotation;
mod scan;

#[cfg(test)]
mod tests;

pub use self::codes::{
    ERROR_SOURCE, EnumCodes, ErrorCodeIndex, ErrorCodeSource, SPEC_SECTION, SPEC_SOURCE, spec_enums,
};
pub use self::error::{Assertion, Denial, Finding, LogError, MAX_FINDINGS};
pub use self::rotation::{Cursor, Identity, LogFamily, LogText, PrivatePath};

use std::path::{Path, PathBuf};

use ime_diag::log::LOG_FILE_NAME;
use ime_dict::paths::{BaseDirs, Paths};

use super::sandbox::Sandbox;

/// How many lines a tap keeps for the evidence archive.
///
/// A failure needs the tail of the log, not its whole history, and an unbounded buffer would
/// let a long soak grow without limit. The oldest lines are dropped first, which is why the
/// buffer is a `Vec` this module trims rather than a ring: the number of lines a case hands out
/// between two assertions is small, so the trim is rare and a plain `Vec` keeps the code
/// readable.
pub const EVIDENCE_LINES: usize = 4096;

/// A reader that follows one log file family and remembers what it has handed out.
///
/// # Concurrency
///
/// `Send`, and not shared: a tap belongs to the case that opened it. Every method takes
/// `&mut self` or `&self` and none of them block beyond the read itself, so a tap can be driven
/// from the case's own thread.
#[derive(Debug)]
pub struct LogTap {
    /// The active log and the rolled siblings the writer keeps behind it.
    family: LogFamily,
    /// How much of the active file has been handed out, and which file that was.
    cursor: Cursor,
    /// The tail of everything this tap has handed out, for the evidence archive.
    lines: Vec<String>,
    /// The home directory whose prefix the log must shorten to `~`.
    ///
    /// Read once, when the tap is opened, from `$HOME` -- the same source the plugin's own
    /// redaction layer reads, so the two agree about which prefix has to be rewritten.
    home: Option<PathBuf>,
    /// How many times the active file was replaced or restarted under this tap.
    restarts: u64,
}

impl LogTap {
    /// Opens a tap on the log family whose active file is `path`.
    ///
    /// Nothing is read until the first [`LogTap::drain`], and the first drain hands out
    /// everything the active file already holds: a tap's baseline is "nothing has been handed
    /// out yet", not "start at the end", so a case that opens a tap on a session log sees the
    /// session rather than only what happens after it started looking.
    pub fn at(path: &Path) -> Self {
        Self {
            family: LogFamily::new(path),
            cursor: Cursor::new(),
            lines: Vec::new(),
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|home| !home.as_os_str().is_empty()),
            restarts: 0,
        }
    }

    /// Opens a tap on the plugin's own log inside `sandbox`.
    ///
    /// The directory is the one the plugin's layout resolves for the sandbox's bases, not a
    /// `logs` this module spells out: a harness that assembled the path itself could read a
    /// directory the plugin never writes to, and a case would then assert on an empty log.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Layout`] when the plugin's layout cannot be derived from the
    /// sandbox's base directories.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn in_sandbox(sandbox: &Sandbox) -> Result<Self, LogError> {
        let bases = BaseDirs {
            config_home: sandbox.config_home().to_path_buf(),
            data_home: sandbox.data_home().to_path_buf(),
        };
        let paths = Paths::from_bases(&bases)?;
        Ok(Self::at(&paths.log_dir.join(LOG_FILE_NAME)))
    }

    /// The active log file this tap follows.
    pub fn path(&self) -> &Path {
        self.family.active()
    }

    /// The files this tap reads: the active log and its rolled siblings.
    pub fn family(&self) -> &LogFamily {
        &self.family
    }

    /// The tail of the lines this tap has handed out, oldest first.
    ///
    /// This is what a failing case archives into its `trace.json`; it is the same text the log
    /// already holds, so it carries nothing the log's own redaction did not decide to write.
    pub fn drained(&self) -> &[String] {
        &self.lines
    }

    /// How many times the active file was replaced or restarted under this tap.
    ///
    /// A non-zero count is how a case knows that the rotation path was exercised, which is what
    /// makes the promise "the privacy assertion covers rolled files too" checkable rather than
    /// assumed.
    pub fn restarts(&self) -> u64 {
        self.restarts
    }

    /// Hands out the lines that appeared since the previous call.
    ///
    /// Only whole lines are returned. A writer that is halfway through a line leaves a tail
    /// without its terminator, and that tail is handed out by the next call, once the writer
    /// has finished it -- so a case never asserts on a line that was still being written.
    ///
    /// # Return value
    ///
    /// The new lines, oldest first, and an empty vector when nothing has been appended.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Io`] when the active file exists and cannot be read, and
    /// [`LogError::NotText`] when a complete line is not valid UTF-8.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn drain(&mut self) -> Result<Vec<String>, LogError> {
        let (lines, restarted) = self.cursor.read_new(self.family.active())?;
        if restarted {
            self.restarts += 1;
        }
        self.lines.extend(lines.iter().cloned());
        if self.lines.len() > EVIDENCE_LINES {
            // `split_off` keeps the tail, which is the part a failure needs.
            let excess = self.lines.len() - EVIDENCE_LINES;
            self.lines = self.lines.split_off(excess);
        }
        Ok(lines)
    }

    /// Asserts that no line of the log breaks the privacy rules.
    ///
    /// The rules are the ones the contract fixes, and they are checked against the *whole*
    /// family -- the active file and every rolled sibling -- read fresh from disk, so a caller
    /// cannot pass the assertion by forgetting to drain first, and content that rolled out of
    /// the active file cannot escape it.
    ///
    /// `literals` are the case's own needles, for a value the case knows it wrote. They are
    /// matched as plain substrings.
    ///
    /// An unterminated last line is read like any other, unlike the lines [`LogTap::drain`]
    /// hands out: a log the writer was killed in the middle of is still a log whose content
    /// must not have leaked, and a blind spot is worse here than a finding on a line the
    /// writer never finished.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Failed`] with one [`Finding`] per offending line -- capped at
    /// [`MAX_FINDINGS`], with the number of further violations reported alongside -- and
    /// [`LogError::Io`] when a file of the family cannot be read.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn assert_absent(&self, literals: &[&str]) -> Result<(), LogError> {
        let home = self.home.as_deref();
        self.assert_over(Assertion::Privacy, |line| {
            scan::violations(line, home, literals)
        })
    }

    /// Asserts that every error code the log carries is one the contract declares.
    ///
    /// The codes are read out of the `code=` fields the log writes; a line that carries none is
    /// not an error line and is passed over. The set to check against is
    /// [`ErrorCodeIndex`]'s, which is extracted from the documents that declare the codes
    /// rather than copied into a case.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Failed`] with one [`Finding`] per offending line -- capped at
    /// [`MAX_FINDINGS`] -- and [`LogError::Io`] when a file of the family cannot be read.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn assert_error_codes_known(&self, known: &[&str]) -> Result<(), LogError> {
        self.assert_over(Assertion::ErrorCodes, |line| {
            scan::unknown_codes(line, known)
        })
    }

    /// Asserts that no path of the log family is reachable by another account.
    ///
    /// The contract fixes the mode of the directory the log lives in and of every file inside
    /// it, so that what the diagnostics wrote is readable by its owner alone. Every path the
    /// family occupies is checked, the rolled siblings included: a file that is world-readable
    /// leaks the same content whether or not the writer still writes to it, and a mode an older
    /// build or a laxer umask left behind is exactly what this exists to find.
    ///
    /// The test is the one the plugin's own layout applies when it adopts a path -- does group
    /// or other hold a bit -- rather than equality with the documented mode, so a file the
    /// owner narrowed further is accepted while nothing a second account can reach is.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Failed`] with one [`Finding`] per path whose mode is wider than the
    /// contract allows -- capped at [`MAX_FINDINGS`], with the number of further violations
    /// reported alongside -- and [`LogError::Io`] when a mode cannot be read or the log
    /// directory cannot be listed.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn assert_private_permissions(&self) -> Result<(), LogError> {
        let mut findings: Vec<Finding> = Vec::new();
        let mut total = 0usize;
        for entry in self.family.private_paths()? {
            let path = entry.path;
            let want = entry.mode;
            // A path that is not there is not a fault: the writer creates the directory and the
            // file at start-up, and a case may look before it has.
            let Some(mode) = rotation::mode_of(&path)? else {
                continue;
            };
            // Group and other must hold no bit at all, which is the predicate the layout itself
            // narrows a path with; equality with `want` would refuse a file its owner had
            // narrowed further.
            if mode & 0o077 == 0 {
                continue;
            }
            total += 1;
            if findings.len() < MAX_FINDINGS {
                findings.push(Finding {
                    path,
                    // A mode belongs to the path, not to one of its lines.
                    line: None,
                    denial: Denial::WorldAccessible { mode, want },
                });
            }
        }
        if total == 0 {
            return Ok(());
        }
        let suppressed = total - findings.len();
        Err(LogError::Failed {
            assertion: Assertion::Permissions,
            findings,
            suppressed,
        })
    }

    /// Reads the whole family and reports the lines `violations` objects to.
    ///
    /// The family is read whole rather than drained, so an unterminated last line is among the
    /// lines this reports on -- see [`LogTap::assert_absent`] for why that is the answer here.
    ///
    /// # Errors
    ///
    /// As [`LogTap::assert_absent`].
    ///
    /// # Panics
    ///
    /// Never.
    fn assert_over(
        &self,
        assertion: Assertion,
        violations: impl Fn(&str) -> Vec<Denial>,
    ) -> Result<(), LogError> {
        let mut findings: Vec<Finding> = Vec::new();
        let mut total = 0usize;
        for file in self.family.read()? {
            for (index, line) in file.lines.iter().enumerate() {
                for denial in violations(line) {
                    total += 1;
                    if findings.len() < MAX_FINDINGS {
                        findings.push(Finding {
                            path: file.path.clone(),
                            line: Some(index + 1),
                            denial,
                        });
                    }
                }
            }
        }
        if total == 0 {
            return Ok(());
        }
        let suppressed = total - findings.len();
        Err(LogError::Failed {
            assertion,
            findings,
            suppressed,
        })
    }
}
