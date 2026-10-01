//! The file side of the snapshot channel: one slot, written atomically, read tolerantly.
//!
//! Responsibility: put the newest frame where a test can read it, and read it back without
//! ever handing the caller a half-written file. Nothing here decodes, renders or decides
//! what a frame should contain.
//!
//! # Why a file and not a socket
//!
//! A control socket would need a listener, a protocol and a lifetime in the plugin, and the
//! project's zero-network promise is easier to keep when the only channel between the two
//! processes is a path the sandbox already owns. Writing a small JSON file costs a fraction
//! of a millisecond, survives a crash of either side, and can be read after the fact.
//!
//! # Why the write goes through a rename
//!
//! A reader must never see a file that is being written. The snapshot is written to
//! `<name>.tmp` and renamed over the named file, so a reader finds either the whole previous
//! snapshot or the whole new one. The rename is deliberately not followed by an `fsync`: the
//! reader is a live process on the same machine, and durability across a power cut is not
//! what a frame mirror is for.
//!
//! # The two revision rules
//!
//! [`UiFrameMirror::publish`] never writes a frame older than the one the file already
//! holds, and [`FrameWatch`] never hands a test a frame older than the one it has already
//! read. Both are the contract's own rule -- a frame older than the one already held is
//! dropped -- applied on the two sides of the file, and between them they make a replayed or
//! reordered frame visible as a counter rather than as a wrong assertion.

use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::sleep;
use std::time::Duration;

use ime_types::UiFrame;

use super::FRAME_FORMAT_VERSION;
use super::FrameSnapshot;
use super::error::FrameError;

/// Name of the snapshot file inside the mirror directory.
///
/// The sandbox clears this directory between cases, which is why the name lives here and
/// not in the caller: one name, one place that resets it.
pub const FRAME_FILE: &str = "ui_frame.json";

/// Suffix the writer's temporary file carries while a snapshot is being written.
const TEMP_SUFFIX: &str = ".tmp";

/// Mode the mirror directory is created with.
const DIR_MODE: u32 = 0o700;

/// Mode the snapshot file is created with.
const FILE_MODE: u32 = 0o600;

/// How hard a read tries before it reports a file as unreadable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadRetry {
    /// How many times the two candidate files are looked at. Values below one are treated as
    /// one, because a read that never looks is not a read.
    pub attempts: u32,
    /// How long the reader waits between two attempts.
    pub pause: Duration,
}

impl Default for ReadRetry {
    /// One retry, which is what the writer's own sequence needs.
    ///
    /// Between writing the temporary file and renaming it, a reader can find a named file
    /// that another writer is halfway through. A second look, a moment later, finds the
    /// rename done; more attempts than that only lengthen the failure path.
    fn default() -> Self {
        Self {
            attempts: 2,
            pause: Duration::from_millis(2),
        }
    }
}

/// What one publish did.
#[derive(Debug)]
pub enum Publish {
    /// The file now holds the frame.
    Written {
        /// The revision that was written.
        revision: u32,
    },
    /// The file already held a frame at least as new, so nothing was written.
    Skipped {
        /// The revision the file holds.
        held: u32,
        /// The revision that arrived.
        seen: u32,
    },
    /// The write did not happen.
    ///
    /// The frame is not in the file. The failure has been counted and is deliberately not
    /// propagated: a mirror that could fail a keystroke would be a worse defect than the
    /// missing snapshot it was reporting.
    Failed(FrameError),
}

impl Publish {
    /// Returns `true` when the frame reached the file.
    pub fn is_written(&self) -> bool {
        matches!(self, Self::Written { .. })
    }
}

/// What one look at the mirror found, as the reader's revision rule sees it.
#[derive(Clone, Debug, PartialEq)]
pub enum Reading {
    /// No snapshot has been written yet, which is the state before the engine's first frame.
    Absent,
    /// A snapshot newer than every one this watch has read.
    ///
    /// Boxed because a frame is a wide value and the other readings are a few bytes: the
    /// alternative is every reading paying for the frame's size.
    New(Box<UiFrame>),
    /// The snapshot carries the revision this watch read last, so nothing has moved.
    Repeated(u32),
    /// The snapshot is older than one this watch has already read.
    ///
    /// The frame itself is not returned: the contract drops such a frame, so a test that
    /// read it would be asserting on a frame the window never drew. The revision the watch
    /// still holds is in `held`, the one the file offered in `seen`.
    Rewound {
        /// The newest revision this watch has read.
        held: u32,
        /// The revision the file offered.
        seen: u32,
    },
}

/// The frame mirror: one file holding the newest frame the engine posted.
///
/// Publishing is total: [`UiFrameMirror::publish`] never returns an error a caller has to
/// handle, and a failed write is counted rather than raised. The floor it keeps is in
/// memory and belongs to this instance, so a restarted session starts from a clean floor --
/// which is safe because a new case resets the mirror directory before it starts, and a
/// case that did not would be comparing against the previous case's frame anyway.
#[derive(Debug)]
pub struct UiFrameMirror {
    /// The file the snapshots go to.
    path: PathBuf,
    /// The revision in the file, plus one; `0` means nothing has been written yet.
    ///
    /// The offset exists because revision `0` is a legal `UiFrame.revision`, so `0` cannot
    /// be the sentinel in a `u32` without conflating "no frame yet" with "the frame the
    /// engine never stamped".
    held: AtomicU64,
    /// Writes that did not happen, counted rather than propagated.
    failures: AtomicU64,
}

impl UiFrameMirror {
    /// Binds a mirror to the snapshot file at `path`.
    ///
    /// Nothing is read or written until the first publish; a mirror over a directory that
    /// does not exist yet is created on demand by the write.
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            held: AtomicU64::new(0),
            failures: AtomicU64::new(0),
        }
    }

    /// Binds a mirror to `ui_frame.json` inside `dir`.
    ///
    /// `dir` is the mirror directory the sandbox hands out, never a path this module
    /// resolves itself: a harness that read `$XDG_RUNTIME_DIR` on its own could write into
    /// the operator's real runtime directory, which is the one thing the sandbox exists to
    /// prevent.
    pub fn at_mirror_dir(dir: &Path) -> Self {
        Self::new(&dir.join(FRAME_FILE))
    }

    /// The file this mirror writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The revision the file holds, or `None` before the first write.
    ///
    /// This is the mirror's own floor, not a read of the file: a second process writing the
    /// same path is not visible here.
    pub fn held(&self) -> Option<u32> {
        match self.held.load(Ordering::Acquire) {
            0 => None,
            stored => Some(revision_of(stored)),
        }
    }

    /// How many writes have failed since this mirror was created.
    ///
    /// A non-zero count on a case that asserts on snapshots means the case read a file that
    /// is not the frame it thinks it is.
    pub fn failures(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }

    /// Writes `frame` to the mirror file, atomically and without ever failing the caller.
    ///
    /// A frame older than the one the file holds, and a repeat of the frame it holds, are
    /// both skipped: the file is a single slot for the newest frame, and letting a stale
    /// frame through would make every later read answer a question about the past.
    ///
    /// # Return value
    ///
    /// What happened, as [`Publish`]. A failure is counted in [`UiFrameMirror::failures`]
    /// and is otherwise ignored, and it does not raise the floor, so the next publish tries
    /// again rather than treating the missing file as written.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn publish(&self, frame: &UiFrame) -> Publish {
        let seen = u64::from(frame.revision) + 1;
        let held = self.held.load(Ordering::Acquire);
        if seen <= held {
            return Publish::Skipped {
                held: revision_of(held),
                seen: frame.revision,
            };
        }
        let text = match FrameSnapshot::of(frame).to_json() {
            Ok(text) => text,
            Err(error) => return self.failed(error),
        };
        if let Err(error) = write_atomically(&self.path, &text) {
            return self.failed(error);
        }
        // Only a written frame raises the floor, so a failed write is retried by the next
        // publish; `fetch_max` keeps a concurrent publish from lowering a floor another
        // thread has already raised.
        self.held.fetch_max(seen, Ordering::Release);
        Publish::Written {
            revision: frame.revision,
        }
    }

    /// Reads the newest snapshot at `path`.
    ///
    /// # Return value
    ///
    /// `Ok(None)` when no snapshot has been written yet, which is the state before the
    /// engine posts its first frame.
    ///
    /// # Errors
    ///
    /// Returns [`FrameError::Io`] when a file that exists cannot be read,
    /// [`FrameError::Malformed`] when neither the named file nor the writer's temporary file
    /// is a snapshot this build can parse, and [`FrameError::Format`] when the file declares
    /// another format version.
    ///
    /// # Panics
    ///
    /// Never panics: a half-written or empty file is reported, never unwrapped.
    pub fn read_latest(path: &Path) -> Result<Option<UiFrame>, FrameError> {
        Self::read_latest_with(path, ReadRetry::default())
    }

    /// Reads the newest snapshot at `path`, retrying as `retry` asks.
    ///
    /// The two files are looked at in the order the writer makes them observable: the
    /// temporary file first, because between the write and the rename it holds the newer
    /// snapshot while the named file still holds the older one. A file that does not exist
    /// is not a failure; only a file that exists and does not parse is, and only when
    /// neither file answers.
    ///
    /// # Errors
    ///
    /// As [`UiFrameMirror::read_latest`].
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn read_latest_with(path: &Path, retry: ReadRetry) -> Result<Option<UiFrame>, FrameError> {
        read_snapshot(path, retry).map(|found| found.map(|snapshot| snapshot.to_frame()))
    }

    /// Counts a failure and answers with it.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn failed(&self, error: FrameError) -> Publish {
        self.failures.fetch_add(1, Ordering::Relaxed);
        Publish::Failed(error)
    }
}

/// A reader that holds the newest revision it has seen.
///
/// The rule it applies is the contract's own: a frame older than the one already held is
/// dropped, which is what makes the host / UI thread split safe against reordering and
/// replays. A test uses it to assert that what it just read is the newest frame rather than
/// a replay of an earlier one, and [`FrameWatch::rewinds`] is the count of times the file
/// went backwards -- a non-zero count is a defect to report, not a number to ignore.
///
/// A watch belongs to one case. A case that resets the mirror starts a new watch, because a
/// reset lowers the file's revision and the old watch would report every frame after it as a
/// rewind.
#[derive(Debug)]
pub struct FrameWatch {
    /// The file this watch reads.
    path: PathBuf,
    /// The newest revision read so far, or `None` before the first frame.
    held: Option<u32>,
    /// How many times a snapshot older than the held one was found.
    rewinds: u64,
}

impl FrameWatch {
    /// Opens a watch on the mirror file at `path`.
    pub fn open(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            held: None,
            rewinds: 0,
        }
    }

    /// The file this watch reads.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The newest revision this watch has read, or `None` before the first frame.
    pub fn held(&self) -> Option<u32> {
        self.held
    }

    /// How many times the file offered a revision older than the held one.
    pub fn rewinds(&self) -> u64 {
        self.rewinds
    }

    /// Reads the newest snapshot and applies the revision rule to it.
    ///
    /// # Errors
    ///
    /// As [`UiFrameMirror::read_latest`].
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn read(&mut self) -> Result<Reading, FrameError> {
        self.read_with(ReadRetry::default())
    }

    /// Reads the newest snapshot, retrying as `retry` asks.
    ///
    /// # Errors
    ///
    /// As [`UiFrameMirror::read_latest`].
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn read_with(&mut self, retry: ReadRetry) -> Result<Reading, FrameError> {
        let frame = match UiFrameMirror::read_latest_with(&self.path, retry)? {
            Some(frame) => frame,
            None => return Ok(Reading::Absent),
        };
        let held = self.held;
        match held {
            Some(held) if frame.revision < held => {
                self.rewinds += 1;
                Ok(Reading::Rewound {
                    held,
                    seen: frame.revision,
                })
            }
            Some(held) if frame.revision == held => Ok(Reading::Repeated(held)),
            _ => {
                self.held = Some(frame.revision);
                Ok(Reading::New(Box::new(frame)))
            }
        }
    }
}

/// The revision a stored floor stands for.
///
/// The floor is stored as `revision + 1`, so it never exceeds `u32::MAX + 1` and the
/// saturating fallback is unreachable. It exists because the type system cannot narrow a
/// `u64` to a `u32`, and a case that cannot fail must not be written as one that can.
fn revision_of(stored: u64) -> u32 {
    u32::try_from(stored.saturating_sub(1)).unwrap_or(u32::MAX)
}

/// What one look at a mirror path found.
enum Look {
    /// A snapshot was readable.
    Found(Box<FrameSnapshot>),
    /// Neither the named file nor the temporary file exists yet.
    Absent,
    /// A file exists and is not a snapshot this build can read.
    Broken(FrameError),
}

/// Reads the newest snapshot a mirror path holds, retrying as `retry` asks.
///
/// # Errors
///
/// The error of the last file that existed and did not parse.
///
/// # Panics
///
/// Never panics.
fn read_snapshot(path: &Path, retry: ReadRetry) -> Result<Option<FrameSnapshot>, FrameError> {
    let attempts = retry.attempts.max(1);
    for attempt in 1..=attempts {
        match look_once(path) {
            Look::Found(snapshot) => return Ok(Some(*snapshot)),
            Look::Absent => return Ok(None),
            Look::Broken(error) if attempt == attempts => return Err(error),
            // A writer that is halfway through its rename leaves a file this look could not
            // parse and a moment later cannot: that window is what the retry is for.
            Look::Broken(_) => sleep(retry.pause),
        }
    }
    // Unreachable: `attempts` is at least one, so the loop's last iteration returns.
    Ok(None)
}

/// Looks once at the temporary file and then at the named file.
///
/// # Panics
///
/// Never panics.
fn look_once(path: &Path) -> Look {
    let temporary = temporary_path(path);
    let mut broken = None;
    for candidate in [temporary.as_path(), path] {
        match parse_at(candidate) {
            Ok(Some(snapshot)) => return Look::Found(Box::new(snapshot)),
            Ok(None) => {}
            Err(error) => broken = Some(error),
        }
    }
    match broken {
        Some(error) => Look::Broken(error),
        None => Look::Absent,
    }
}

/// Reads and parses one snapshot file.
///
/// # Return value
///
/// `Ok(None)` when the file does not exist.
///
/// # Errors
///
/// Returns [`FrameError::Io`] for a file that exists and cannot be read as text, and
/// [`FrameError::Malformed`] or [`FrameError::Format`] for one that is not a readable
/// snapshot.
///
/// # Panics
///
/// Never panics.
fn parse_at(path: &Path) -> Result<Option<FrameSnapshot>, FrameError> {
    match fs::read_to_string(path) {
        Ok(text) => parse_text(path, &text).map(Some),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(source) => Err(FrameError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Parses the text of a snapshot file.
///
/// # Errors
///
/// Returns [`FrameError::Malformed`] for text that is empty or is not a snapshot, and
/// [`FrameError::Format`] for a snapshot written by a build that declares another format
/// version.
///
/// # Panics
///
/// Never panics.
fn parse_text(path: &Path, text: &str) -> Result<FrameSnapshot, FrameError> {
    if text.trim().is_empty() {
        return Err(FrameError::Malformed {
            path: path.to_path_buf(),
            detail: String::from("the file is empty"),
        });
    }
    let snapshot: FrameSnapshot =
        serde_json::from_str(text).map_err(|error| FrameError::Malformed {
            path: path.to_path_buf(),
            detail: describe(&error),
        })?;
    if snapshot.format != FRAME_FORMAT_VERSION {
        return Err(FrameError::Format {
            path: path.to_path_buf(),
            found: snapshot.format,
            expected: FRAME_FORMAT_VERSION,
        });
    }
    Ok(snapshot)
}

/// Names what the JSON parser objected to, without repeating the file's own content.
///
/// `serde_json`'s own message quotes the offending value, and the offending value of a
/// snapshot is the text the user typed. An error is printed into a terminal and archived as
/// evidence, so it carries the category and the position instead: enough to find the field,
/// and nothing that came out of the user's keyboard.
///
/// # Panics
///
/// Never panics.
fn describe(error: &serde_json::Error) -> String {
    let category = match error.classify() {
        serde_json::error::Category::Io => "the file could not be read",
        serde_json::error::Category::Syntax => "the file is not JSON",
        serde_json::error::Category::Data => "the file is JSON but not a frame snapshot",
        serde_json::error::Category::Eof => "the file stops in the middle of a snapshot",
    };
    format!(
        "{category}, at line {} column {}",
        error.line(),
        error.column()
    )
}

/// Writes `text` to `path` by way of a temporary file and a rename.
///
/// # Errors
///
/// Returns [`FrameError::Io`] when the directory cannot be created, the temporary file
/// cannot be written, or the rename fails.
///
/// # Panics
///
/// Never panics.
fn write_atomically(path: &Path, text: &str) -> Result<(), FrameError> {
    let temporary = temporary_path(path);
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        create_private_dir(dir)?;
    }
    write_private(&temporary, text)?;
    fs::rename(&temporary, path).map_err(|source| FrameError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Creates `dir` and its parents with the mode the plugin uses for its own directories.
///
/// # Errors
///
/// Returns [`FrameError::Io`] when a component cannot be created.
///
/// # Panics
///
/// Never panics.
fn create_private_dir(dir: &Path) -> Result<(), FrameError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(dir)
        .map_err(|source| FrameError::Io {
            path: dir.to_path_buf(),
            source,
        })
}

/// Creates `path` private and writes `text` into it.
///
/// # Errors
///
/// Returns [`FrameError::Io`] when the file cannot be created or the write fails.
///
/// # Panics
///
/// Never panics.
fn write_private(path: &Path, text: &str) -> Result<(), FrameError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(FILE_MODE)
        .open(path)
        .map_err(|source| FrameError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(text.as_bytes())
        .map_err(|source| FrameError::Io {
            path: path.to_path_buf(),
            source,
        })
}

/// The temporary file a write to `path` goes through.
///
/// The suffix is appended to the last component rather than replacing the extension, so a
/// caller that names its snapshot file something else still gets a temporary file beside it.
///
/// # Panics
///
/// Never panics.
fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(TEMP_SUFFIX);
    PathBuf::from(name)
}
