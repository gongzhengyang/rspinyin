//! The file side of the log channel: which files hold the log, and where a reader is in them.
//!
//! Responsibility: name the files a log occupies -- the active one and the rolled siblings the
//! writer keeps behind it -- the mode each path of the family must carry, and hand out the bytes
//! a reader has not seen yet, across the rolls the writer performs. Nothing here interprets a
//! line; that is [`super::scan`]'s job.
//!
//! # Why the file, and not the path
//!
//! A roll renames the active file out of the way and creates a new one under the same name. A
//! reader that tracked the path alone would find an empty file after the roll and report "no
//! new lines" for content that had just been written -- the one failure mode that makes a
//! log-based assertion pass for the wrong reason. [`Cursor`] therefore remembers the device and
//! inode of the file it has been reading, and treats a different identity under the same name
//! as a new file to read from its beginning.
//!
//! The same reasoning covers the truncation the writer performs when it keeps no history at
//! all: the inode is unchanged and the file is suddenly shorter than the offset, which is the
//! second shape a restart takes.
//!
//! # The mode, read from the layout that sets it
//!
//! Every path of the family carries the mode the plugin's own layout gives it: the directory
//! holding the log is [`DIR_MODE`] and every file inside it is [`FILE_MODE`], so that what the
//! diagnostics wrote is readable by its owner alone. The two numbers are read out of
//! `ime_dict::paths` rather than spelled here, for the same reason [`super::scan`] reads the
//! denylist out of `ime_diag::redact`: that module is where the plugin creates the directory,
//! and a reader that wrote the mode out again would keep checking a number the plugin had
//! stopped using.

use std::fs;
use std::io::{ErrorKind, Read as _, Seek as _, SeekFrom};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use ime_dict::paths::{DIR_MODE, FILE_MODE};

use super::error::LogError;

/// The device and inode a file occupies.
///
/// Two looks at the same name agree on this exactly when they are looking at the same file,
/// which is what makes it the right thing for a reader to remember across a roll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    /// The device the file lives on.
    pub device: u64,
    /// The inode number within that device.
    pub inode: u64,
}

impl Identity {
    /// The identity of the file at `path`, or `None` when it does not exist.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Io`] when the path exists but its metadata cannot be read.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn of(path: &Path) -> Result<Option<Self>, LogError> {
        match fs::metadata(path) {
            Ok(meta) => Ok(Some(Self {
                device: meta.dev(),
                inode: meta.ino(),
            })),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(source) => Err(LogError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }
}

/// One file of a log family, read as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogText {
    /// The file it came from.
    pub path: PathBuf,
    /// Its lines, without their terminators.
    pub lines: Vec<String>,
}

/// One path the log family occupies, with the mode the contract fixes for it.
///
/// The two travel together because the rule is per path kind: the directory the log lives in
/// and the files inside it carry different modes, and a caller that compared one against the
/// other's would report a fault that is not there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrivatePath {
    /// The path whose mode is fixed.
    pub path: PathBuf,
    /// The mode it must carry, in the low nine permission bits.
    pub mode: u32,
}

/// The active log file and the rolled siblings the writer keeps behind it.
///
/// The family is defined by a naming rule rather than by a count: the writer's rolled files are
/// the active file's name with `.1`, `.2` and so on appended, `.1` being the newest. Enumerating
/// the directory against that rule is what keeps this module from having to know how many files
/// the writer was configured to keep -- a number that belongs to the writer's configuration and
/// not to the reader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogFamily {
    /// The name the writer keeps its active file under.
    active: PathBuf,
}

impl LogFamily {
    /// Binds a family to the active file at `path`.
    pub fn new(active: &Path) -> Self {
        Self {
            active: active.to_path_buf(),
        }
    }

    /// The active log file.
    pub fn active(&self) -> &Path {
        &self.active
    }

    /// The files of the family that exist: the active one first, then the rolled siblings from
    /// newest to oldest.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Io`] when the log directory exists and cannot be listed. A directory
    /// that does not exist is not an error: the plugin creates it at start-up, and a case may
    /// look before it has.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn files(&self) -> Result<Vec<PathBuf>, LogError> {
        let mut files = Vec::new();
        if self.active.is_file() {
            files.push(self.active.clone());
        }
        files.extend(self.siblings()?);
        Ok(files)
    }

    /// Reads every file of the family.
    ///
    /// The bytes are decoded lossily rather than strictly: this is the path an *assertion*
    /// takes, and a torn write at the end of the active file must not turn "is this pattern
    /// absent" into an I/O error. The strict decode lives in [`Cursor::read_new`], where a line
    /// is about to be handed to a case that will assert on its content.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Io`] when a file of the family exists and cannot be read.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn read(&self) -> Result<Vec<LogText>, LogError> {
        let mut files = Vec::new();
        for path in self.files()? {
            let bytes = fs::read(&path).map_err(|source| LogError::Io {
                path: path.clone(),
                source,
            })?;
            let text = String::from_utf8_lossy(&bytes);
            files.push(LogText {
                path,
                lines: text.lines().map(str::to_owned).collect(),
            });
        }
        Ok(files)
    }

    /// Every path the family occupies, with the mode the contract fixes for it.
    ///
    /// The directory the active file lives in comes first, then the files of the family from
    /// newest to oldest -- the order [`LogFamily::files`] reports them in. A path that does not
    /// exist is not among them: the writer creates the directory and the file at start-up, and
    /// a case may look before it has, so an absent family is an empty list rather than a
    /// failure.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Io`] when the log directory exists and cannot be listed.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn private_paths(&self) -> Result<Vec<PrivatePath>, LogError> {
        let mut paths = Vec::new();
        if let Some(directory) = self.active.parent() {
            paths.push(PrivatePath {
                path: directory.to_path_buf(),
                mode: DIR_MODE,
            });
        }
        for path in self.files()? {
            paths.push(PrivatePath {
                path,
                mode: FILE_MODE,
            });
        }
        Ok(paths)
    }

    /// The rolled siblings that exist, newest first.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Io`] when the log directory exists and cannot be listed.
    ///
    /// # Panics
    ///
    /// Never.
    fn siblings(&self) -> Result<Vec<PathBuf>, LogError> {
        let Some(directory) = self.active.parent() else {
            return Ok(Vec::new());
        };
        let Some(name) = self.active.file_name().and_then(|name| name.to_str()) else {
            return Ok(Vec::new());
        };
        let prefix = format!("{name}.");
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(LogError::Io {
                    path: directory.to_path_buf(),
                    source,
                });
            }
        };
        let mut found: Vec<(usize, PathBuf)> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| LogError::Io {
                path: directory.to_path_buf(),
                source,
            })?;
            let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            // `.0` is not a name the writer produces -- the active file is the one without a
            // suffix -- so a file that carries it is somebody else's and is left alone.
            let Some(index) = file_name
                .strip_prefix(&prefix)
                .and_then(|rest| rest.parse::<usize>().ok())
                .filter(|index| *index > 0)
            else {
                continue;
            };
            found.push((index, entry.path()));
        }
        // Newest first, which is the order the index runs in: `.1` holds what the active
        // file held a moment ago, and a larger index is older. Sorting by descending index
        // would put the oldest rolled file first and read the history backwards.
        found.sort_by_key(|(index, _)| *index);
        Ok(found.into_iter().map(|(_, path)| path).collect())
    }
}

/// Where a reader has got to in the active log file.
///
/// The cursor is deliberately a small `Copy` value rather than a handle: it holds no open file
/// descriptor, so a roll between two reads cannot leave it pointing at a file that has been
/// renamed out from under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// The file the offset belongs to, or `None` before the first read.
    identity: Option<Identity>,
    /// How many bytes of that file have been handed out.
    offset: u64,
    /// The last bytes handed out, so that an in-place rewrite is visible.
    ///
    /// A writer that keeps no history truncates the active file and starts again. The inode
    /// is unchanged, and the new file is soon longer than the old offset, so neither the
    /// identity nor the length can tell the two apart -- but the bytes the cursor already
    /// handed out are gone, and comparing the tail it remembers against what is there now
    /// is what sees it.
    signature: [u8; SIGNATURE_BYTES],
    /// How many of those bytes are in use.
    signature_len: usize,
}

/// How many bytes of the tail a cursor remembers.
///
/// Long enough that a rewritten file matching it by accident is not a case worth handling,
/// short enough that the check is one small read per call.
const SIGNATURE_BYTES: usize = 64;

impl Default for Cursor {
    /// A cursor that has handed nothing out.
    ///
    /// Written out rather than derived: the signature is a fixed array, and an array's
    /// `Default` is only implemented for lengths the standard library enumerates.
    fn default() -> Self {
        Self {
            identity: None,
            offset: 0,
            signature: [0; SIGNATURE_BYTES],
            signature_len: 0,
        }
    }
}

impl Cursor {
    /// A cursor that has handed nothing out.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many bytes of the active file have been handed out.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// The file the offset belongs to, or `None` before the first read.
    pub fn identity(&self) -> Option<Identity> {
        self.identity
    }

    /// Reads the whole lines of `path` that have not been handed out yet.
    ///
    /// # Return value
    ///
    /// The new lines, and whether the file restarted under the cursor -- which is what a roll
    /// looks like from here, and what a caller counts so that a case can tell a rotation it
    /// exercised from one that never happened.
    ///
    /// # Errors
    ///
    /// Returns [`LogError::Io`] when the file exists and cannot be read, and
    /// [`LogError::NotText`] when the complete lines read are not valid UTF-8.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn read_new(&mut self, path: &Path) -> Result<(Vec<String>, bool), LogError> {
        let opened = fs::File::open(path);
        let mut file = match opened {
            Ok(file) => file,
            // A file that is not there yet is not a failure: the log is created when the
            // plugin starts, and a case may look before it has.
            Err(error) if error.kind() == ErrorKind::NotFound => {
                self.identity = None;
                self.offset = 0;
                self.signature_len = 0;
                return Ok((Vec::new(), false));
            }
            Err(source) => {
                return Err(LogError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        // The identity is taken from the open handle rather than from the path, so a roll
        // between the two calls cannot make the cursor describe one file and read another.
        let meta = file.metadata().map_err(|source| LogError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let identity = Identity {
            device: meta.dev(),
            inode: meta.ino(),
        };
        let mut restarted = false;
        match self.identity {
            Some(previous) if previous != identity => restarted = true,
            // A writer with no history to keep truncates the active file in place, which leaves
            // the identity alone. Two things see it: a file now shorter than the offset, and --
            // once it has grown past the offset again -- the bytes the cursor remembers handing
            // out, which are no longer the bytes that sit before the offset.
            Some(_) if meta.len() < self.offset => restarted = true,
            Some(_) if self.signature_len > 0 => {
                let at = self.offset - self.signature_len as u64;
                let mut held = vec![0u8; self.signature_len];
                file.seek(SeekFrom::Start(at))
                    .map_err(|source| LogError::Io {
                        path: path.to_path_buf(),
                        source,
                    })?;
                // A short read means the file changed under this call, which is the same
                // situation as a mismatch and is answered the same way.
                let matched = file.read_exact(&mut held).is_ok()
                    && held[..] == self.signature[..self.signature_len];
                if !matched {
                    restarted = true;
                }
            }
            _ => {}
        }
        if restarted {
            self.offset = 0;
            self.signature_len = 0;
        }
        self.identity = Some(identity);

        file.seek(SeekFrom::Start(self.offset))
            .map_err(|source| LogError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        let mut tail = Vec::new();
        file.read_to_end(&mut tail).map_err(|source| LogError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        // Only whole lines are handed out. The writer appends one line per event, so a tail
        // without its terminator is a line still being written; it is left for the next call
        // rather than handed to a case that would assert on half of it.
        let complete = tail
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |at| at + 1);
        let text = std::str::from_utf8(&tail[..complete]).map_err(|_| LogError::NotText {
            path: path.to_path_buf(),
            offset: self.offset,
        })?;
        self.offset += complete as u64;
        // Remember the tail of what was handed out, for the next call's rewrite check.
        let handed = &tail[..complete];
        let kept = handed.len().min(SIGNATURE_BYTES);
        let carried = (self.signature_len + kept).min(SIGNATURE_BYTES) - kept;
        self.signature
            .copy_within(self.signature_len - carried..self.signature_len, 0);
        self.signature[carried..carried + kept].copy_from_slice(&handed[handed.len() - kept..]);
        self.signature_len = carried + kept;
        Ok((text.lines().map(str::to_owned).collect(), restarted))
    }
}

/// The permission bits of `path`, or `None` when it does not exist.
///
/// # Return value
///
/// The low nine bits of the path's mode. The higher bits describe the file's kind, which is
/// not what a reader comparing a mode against the contract is asking about.
///
/// # Errors
///
/// Returns [`LogError::Io`] when the path exists but its metadata cannot be read. A path that
/// does not exist is `None` rather than an error: the writer creates the log at start-up, and
/// a case may look before it has.
///
/// # Panics
///
/// Never.
pub(super) fn mode_of(path: &Path) -> Result<Option<u32>, LogError> {
    match fs::metadata(path) {
        Ok(meta) => Ok(Some(meta.mode() & 0o777)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(source) => Err(LogError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}
