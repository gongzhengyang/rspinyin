//! What the plugin does when a data file is damaged.
//!
//! Responsibility: decide what a damaged dictionary or user store means, move the
//! damaged file aside without losing it, and rebuild what can be rebuilt. The container
//! reader and the user store report *that* a file is wrong; this module owns what
//! happens next.
//!
//! Boundaries: it never parses a container and never opens a table of its own. It asks
//! the two owners of those formats -- [`FstLexicon::load`] and [`UserDb::open`] --
//! whether a file is usable, and asks `redb`, the owner of the store's format, what a
//! file is when the store refuses it. It does not log: this crate has no logger, so what
//! happened leaves as a [`RecoveryOutcome`] and the caller reports it with the code
//! [`RecoveryOutcome::code`] names.
//!
//! # A damaged file is never lost
//!
//! Damage is answered by *renaming* the file to `<name>.corrupt.<unix_ts>`, never by
//! deleting it and never by overwriting it in place: a user who lost their store may
//! still want it back, and nothing here can know that they do not. The rename is atomic
//! and stays inside the file's own directory, and the destination name is claimed
//! exclusively before the rename, so a second corruption in the same second gets a name
//! of its own instead of destroying the first one's file.
//!
//! # Degrading instead of failing
//!
//! Every step can fail and no failure is fatal. A dictionary that cannot be moved aside
//! -- the packaged one lives under `/usr/share`, which the user cannot write to -- still
//! yields [`RecoveryOutcome::DictMissing`], and a store that cannot be replaced yields
//! [`RecoveryOutcome::ReadonlyMode`]. Input keeps working in every one of those cases:
//! candidates are dropped, every keystroke goes through as passthrough, learning stops,
//! and the plugin stays usable (`ASM-15`).
//!
//! # Trust model
//!
//! A data file is untrusted input, and nothing here trusts a byte of it. The verdict
//! comes from the loader that would read the file anyway, and this module never reads a
//! file's *contents* at all: it stats, renames and creates, so a crafted file cannot
//! make the recovery pass read out of bounds or allocate on a length it chose.

use std::fs::{self, File};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ime_types::{DictError, ImeError};

use crate::format::writer;
use crate::fst_index::FstLexicon;
use crate::user_db::UserDb;

mod quarantine;

use self::quarantine::move_aside;

#[cfg(test)]
mod tests;

/// Marker a quarantined file's name carries: `base.dict` becomes
/// `base.dict.corrupt.<unix_ts>`.
///
/// The same shape the repository ignores (`*.corrupt.*`), so a quarantine never shows
/// up as an untracked file in a working copy.
pub const QUARANTINE_MARK: &str = ".corrupt.";

/// Diagnostic code reported when a damaged user store was moved aside and replaced.
pub const USER_DB_RECOVERED_CODE: &str = "data/db/recovered";

/// Diagnostic code reported when the dictionary was found damaged or absent.
pub const DICT_CORRUPT_CODE: &str = "dict/corrupt";

/// Diagnostic code reported when a step failed and the plugin stopped writing.
///
/// The same code the data-layout module reports: the plugin has one read-only state,
/// and a second name for it would read as a second state.
pub const READONLY_CODE: &str = "data/readonly-mode";

/// What a recovery pass found, and what it did about it.
///
/// The variant answers one question for the caller: may the plugin carry on exactly as
/// it would have without a recovery pass? [`RecoveryOutcome::Healthy`] is the only
/// variant that says yes.
///
/// The type is neither `Clone` nor `PartialEq` because it embeds [`DictError`], which
/// wraps a `std::io::Error`; that type is neither cloneable nor comparable.
#[derive(Debug)]
pub enum RecoveryOutcome {
    /// Everything is as it should be; nothing was touched.
    Healthy,
    /// The user store was damaged: it was moved aside and replaced by an empty one.
    ///
    /// Input is complete and learning starts over from zero. `quarantine` is where the
    /// damaged file went, or empty when it could not be moved aside -- a store in a
    /// directory the user cannot write to -- in which case the file is still where it
    /// was. [`RecoveryOutcome::quarantine_path`] is the accessor that tells the two
    /// apart.
    UserDbRebuilt {
        /// Where the damaged store was moved to; empty when nothing was moved.
        quarantine: PathBuf,
    },
    /// The dictionary is damaged or absent, so there are no candidates.
    ///
    /// The engine disables candidate selection and sends every keystroke through as
    /// passthrough, and the candidate window says the dictionary is unavailable.
    DictMissing {
        /// Where the damaged dictionary was moved to; empty when nothing was moved.
        quarantine: PathBuf,
        /// Why the loader refused the file.
        cause: DictError,
    },
    /// A step failed and the plugin stopped writing.
    ///
    /// Input is complete -- the dictionary may be perfectly usable -- but nothing is
    /// learned and no file is written until the next start.
    ReadonlyMode {
        /// Developer-facing explanation of what failed.
        reason: String,
    },
}

impl RecoveryOutcome {
    /// Whether the caller may carry on exactly as if no recovery pass had run.
    pub fn is_healthy(&self) -> bool {
        matches!(self, Self::Healthy)
    }

    /// Returns the path the damaged file was moved to, or `None` when nothing was moved.
    ///
    /// The two quarantine fields are empty rather than absent when the file could not be
    /// moved aside, because the frozen shape of the outcome has no room for an
    /// `Option`; this accessor is where that difference becomes explicit, and a caller
    /// that wants to tell the user where their data went asks here.
    pub fn quarantine_path(&self) -> Option<&Path> {
        let path = match self {
            Self::UserDbRebuilt { quarantine } | Self::DictMissing { quarantine, .. } => {
                quarantine
            }
            Self::Healthy | Self::ReadonlyMode { .. } => return None,
        };
        (!path.as_os_str().is_empty()).then_some(path.as_path())
    }

    /// Returns the stable diagnostic code the caller reports, or `None` when there is
    /// nothing to report.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Healthy => None,
            Self::UserDbRebuilt { .. } => Some(USER_DB_RECOVERED_CODE),
            Self::DictMissing { .. } => Some(DICT_CORRUPT_CODE),
            Self::ReadonlyMode { .. } => Some(READONLY_CODE),
        }
    }

    /// Returns the developer-facing detail for the diagnostic's log line.
    ///
    /// The text may name a path -- a quarantine file is what a user has to find if they
    /// want their data back -- and never carries user input, so it is safe to log as
    /// it stands.
    pub fn detail(&self) -> String {
        match self {
            Self::Healthy => String::from("the data files are usable"),
            Self::UserDbRebuilt { quarantine } if quarantine.as_os_str().is_empty() => {
                String::from("the damaged store could not be moved aside; a new one replaced it")
            }
            Self::UserDbRebuilt { quarantine } => {
                format!("damaged user store moved to {}", quarantine.display())
            }
            Self::DictMissing { quarantine, cause } if quarantine.as_os_str().is_empty() => {
                format!("the dictionary is unusable: {cause}")
            }
            Self::DictMissing { quarantine, cause } => {
                format!("dictionary moved to {}: {cause}", quarantine.display())
            }
            Self::ReadonlyMode { reason } => reason.clone(),
        }
    }
}

/// Checks the dictionary at `path` and isolates it when it cannot be used.
///
/// The verdict is [`FstLexicon::load`]'s, so a file this function calls healthy is one
/// the engine can load, and one it calls damaged is one the engine would refuse.
///
/// # Parameters
/// - `path`: the compiled container, normally `base.dict`.
///
/// # Return value
/// [`RecoveryOutcome::Healthy`] when the loader accepts the file, and
/// [`RecoveryOutcome::DictMissing`] when it does not. A dictionary that cannot be moved
/// aside -- it lives in a directory the user cannot write to -- is reported the same
/// way with an empty quarantine: the caller drops candidates either way, and the file
/// stays where the user can look at it.
///
/// # Panics
/// Never. A recovery pass runs during start-up and may not cost the user their input
/// method, so every failure it can meet is carried in the outcome instead of raised.
pub fn recover_dict(path: &Path) -> RecoveryOutcome {
    recover_dict_at(path, unix_secs())
}

/// [`recover_dict`] with the quarantine stamp supplied by the caller.
///
/// The stamp is the only part of a quarantine's name that is not a function of the
/// input, so it is a parameter: a test can name the file it expects instead of globbing
/// for it, and no test has to read the clock.
fn recover_dict_at(path: &Path, stamp: u64) -> RecoveryOutcome {
    if !path.exists() {
        return RecoveryOutcome::DictMissing {
            quarantine: PathBuf::new(),
            cause: DictError::Io(io::Error::new(
                ErrorKind::NotFound,
                "the dictionary file does not exist",
            )),
        };
    }
    let cause = match FstLexicon::load(path) {
        Ok(_) => return RecoveryOutcome::Healthy,
        Err(cause) => cause,
    };
    // Only a file the loader refused *because of its contents* is moved aside. A file
    // that could not be read at all stays exactly where it is: moving a file the
    // process cannot read is the one action that could lose it for good.
    let quarantine = if is_damaged(&cause) {
        move_aside(path, stamp).unwrap_or_default()
    } else {
        PathBuf::new()
    };
    RecoveryOutcome::DictMissing { quarantine, cause }
}

/// Returns `true` when the loader refused `cause` because of what the file contains.
fn is_damaged(cause: &DictError) -> bool {
    match cause {
        DictError::MagicMismatch
        | DictError::FormatVersion { .. }
        | DictError::LengthOutOfRange { .. }
        | DictError::Crc { .. }
        | DictError::Fst(_) => true,
        // The file was never read: it is gone, or the process may not open it. Neither
        // says anything about its contents.
        DictError::Io(_) => false,
    }
}

/// Checks the user store at `path`, replacing it when it is damaged.
///
/// The probe is the open the caller is about to perform, so a store this function calls
/// healthy is one the caller can open. A store that does not exist yet is created here
/// exactly as the caller would create it a moment later.
///
/// # Parameters
/// - `path`: the store, normally `user.redb`.
///
/// # Return value
/// [`RecoveryOutcome::Healthy`] when the store opens,
/// [`RecoveryOutcome::UserDbRebuilt`] when a damaged store was moved aside and replaced
/// by an empty one, and [`RecoveryOutcome::ReadonlyMode`] when the store could neither
/// be used nor replaced -- an unwritable directory, a store another process holds open,
/// or a schema version this build does not know. Learning stops in that last case;
/// input does not.
///
/// # Panics
/// Never, for the reason [`recover_dict`] gives.
pub fn recover_user_db(path: &Path) -> RecoveryOutcome {
    recover_user_db_at(path, unix_secs())
}

/// [`recover_user_db`] with the quarantine stamp supplied by the caller.
fn recover_user_db_at(path: &Path, stamp: u64) -> RecoveryOutcome {
    // A store that exists and holds no bytes is the residue of a create that was
    // interrupted before `redb` wrote its header. `redb` would quietly initialize such
    // a file, which hides the interruption instead of reporting it, so the length is
    // read first: the file has nothing to lose, and moving it aside keeps the evidence.
    if is_empty_file(path) {
        return rebuild(path, stamp, "the store is empty");
    }
    match UserDb::open(path) {
        Ok(db) => {
            // The handle owns the store's lock; it is dropped here so that the caller
            // can open the store itself.
            drop(db);
            RecoveryOutcome::Healthy
        }
        Err(error) => recover_from_store_failure(path, error, stamp),
    }
}

/// Whether `path` is a file that holds no bytes.
fn is_empty_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() == 0)
}

/// Turns a refused store into the outcome the caller acts on.
fn recover_from_store_failure(path: &Path, error: ImeError, stamp: u64) -> RecoveryOutcome {
    let cause = match error {
        ImeError::DictUnavailable { cause, .. } => cause,
        // `UserDb::open` reports every failure as an unavailable store. A variant that
        // is not one would be a contract change, and refusing to touch the file is the
        // safe answer until that change is made deliberately.
        other => {
            return RecoveryOutcome::ReadonlyMode {
                reason: other.to_string(),
            };
        }
    };
    let reason = cause.to_string();
    match classify_store_failure(path, &cause) {
        StoreFailure::Damaged => rebuild(path, stamp, &reason),
        StoreFailure::Unusable => RecoveryOutcome::ReadonlyMode { reason },
    }
}

/// What a refused store means for the file on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StoreFailure {
    /// The bytes are not a store: the file is moved aside and replaced.
    Damaged,
    /// The file is a store, or may be one, but this build cannot use it here.
    Unusable,
}

/// Classifies why the store refused `path`.
fn classify_store_failure(path: &Path, cause: &DictError) -> StoreFailure {
    match cause {
        // A store written by another schema version is the user's data, not damage: a
        // build that knows the version may still read it, so it is left alone. The
        // dictionary takes the opposite decision on the same error because a dictionary
        // is a build artifact the compiler replaces, never something the user wrote.
        DictError::FormatVersion { .. } => StoreFailure::Unusable,
        DictError::MagicMismatch
        | DictError::Crc { .. }
        | DictError::LengthOutOfRange { .. }
        | DictError::Fst(_) => StoreFailure::Damaged,
        // The store renders a `redb` failure into an `io::Error`, which erases the
        // variant. Rather than read the message, the owner of the format is asked: only
        // `redb` can tell a store it refuses because of its contents from one it refuses
        // because another process holds it open, and only the first may be moved aside.
        DictError::Io(_) => ask_redb(path),
    }
}

/// Asks `redb`, the owner of the store's format, what the file at `path` is.
///
/// The probe is a plain create: it neither reads a table nor writes one, and the handle
/// it returns -- when it returns one -- is dropped immediately, releasing the lock.
fn ask_redb(path: &Path) -> StoreFailure {
    match redb::Database::create(path) {
        // The file *is* a store, so whatever the open failed on belonged to the
        // environment -- a second handle, a mode, a transient IO error -- and a file
        // that is a store is never moved aside.
        Ok(_) => StoreFailure::Unusable,
        Err(redb::DatabaseError::Storage(redb::StorageError::Corrupted(_))) => {
            StoreFailure::Damaged
        }
        Err(redb::DatabaseError::RepairAborted) => StoreFailure::Damaged,
        // `redb` reports a file whose magic number does not match as `InvalidData`.
        Err(redb::DatabaseError::Storage(redb::StorageError::Io(error)))
            if error.kind() == ErrorKind::InvalidData =>
        {
            StoreFailure::Damaged
        }
        // `DatabaseAlreadyOpen`, `UpgradeRequired`, an IO failure: none of them is
        // evidence about what the file contains.
        Err(_) => StoreFailure::Unusable,
    }
}

/// Moves a damaged store aside and creates an empty one in its place.
///
/// # Return value
/// [`RecoveryOutcome::UserDbRebuilt`] when the replacement could be opened, and
/// [`RecoveryOutcome::ReadonlyMode`] when it could not: a store that cannot be replaced
/// must not cost the user their input method.
fn rebuild(path: &Path, stamp: u64, reason: &str) -> RecoveryOutcome {
    let quarantine = move_aside(path, stamp).unwrap_or_default();
    match UserDb::open(path) {
        Ok(db) => {
            drop(db);
            RecoveryOutcome::UserDbRebuilt { quarantine }
        }
        Err(error) => RecoveryOutcome::ReadonlyMode {
            reason: format!("{reason}; the replacement store could not be opened: {error}"),
        },
    }
}

/// Seconds since the Unix epoch, the stamp a quarantine's name carries.
///
/// A clock that cannot be read yields zero rather than an error: the stamp only has to
/// keep two quarantines of one file apart, and the collision suffix does that on its
/// own when the clock says nothing useful.
fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Replaces `target` with `bytes`, atomically.
///
/// The bytes go to the sibling temporary file [`writer::temp_path`] names, are flushed
/// with `File::sync_all`, and the temporary is then renamed onto the target. The target
/// is never opened for writing and is never truncated, so a reader observes either the
/// whole previous file or the whole new one, and a crash or a full disk leaves the
/// previous file in place.
///
/// [`DictWriter::finish`](crate::format::writer::DictWriter::finish) applies the same
/// discipline to a container this crate assembles itself; the temporary's name comes
/// from that module so that the two paths cannot disagree about it. The new file carries
/// the mode the process umask produces, exactly as the writer's output does -- the store
/// that has to be private is created by `user_db`, not here.
///
/// # Parameters
/// - `target`: the file to replace; a missing parent directory is created.
/// - `bytes`: the complete new contents.
///
/// # Errors
/// Returns the underlying [`io::Error`] when the parent directory cannot be created, the
/// temporary cannot be written or flushed, or the rename fails. A temporary left behind
/// by the failed write is removed first. A caller that sees a full disk -- which
/// [`is_out_of_space`] answers -- reports "no space left" rather than "write failed",
/// because that is the part the user can act on.
///
/// # Panics
/// Never: every failure of the write, the flush and the rename is returned.
///
/// # Examples
///
/// ```
/// use ime_dict::recover::atomic_replace;
///
/// let path = std::env::temp_dir().join(format!("rspinyin-replace-{}", std::process::id()));
/// atomic_replace(&path, b"the image").expect("replacing");
/// assert_eq!(std::fs::read(&path).expect("reading"), b"the image");
/// std::fs::remove_file(&path).expect("removing the fixture");
/// ```
pub fn atomic_replace(target: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let temp = writer::temp_path(target);
    match write_and_rename(&temp, target, bytes) {
        Ok(()) => Ok(()),
        Err(error) => {
            // The write failed, so the temporary holds at best a prefix of the image.
            // Removing it is what keeps the next attempt from trusting it. The removal's
            // own failure is not reported: the error that stopped the write is the one
            // the caller can act on, and a temporary left behind is inert -- nothing
            // reads it, and the next attempt truncates it.
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// Writes `bytes` to `temp`, flushes them to the device and renames `temp` onto
/// `target`.
fn write_and_rename(temp: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(temp)?;
    file.write_all(bytes)?;
    // The flush is what makes the rename meaningful: without it the name could be
    // published while the bytes are still only in the page cache, and a power loss
    // would leave a complete name over an incomplete file.
    file.sync_all()?;
    // Closed before the rename: a descriptor is not a name, and leaving one open across
    // the rename would keep a handle to the temporary alive after the name is gone.
    drop(file);
    fs::rename(temp, target)
}

/// Whether `error` means the filesystem ran out of room.
///
/// The distinction the dictionary compiler needs: a full disk is a condition the user
/// can act on, and any other write failure is not. `StorageFull` is `ENOSPC`;
/// `QuotaExceeded` is `EDQUOT`, the same condition seen through a quota.
pub fn is_out_of_space(error: &io::Error) -> bool {
    matches!(error.kind(), ErrorKind::StorageFull | ErrorKind::QuotaExceeded)
}
