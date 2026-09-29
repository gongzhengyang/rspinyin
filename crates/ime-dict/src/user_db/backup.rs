//! Dated copies of what the user has learned, and the rollback a damaged store is rebuilt
//! from.
//!
//! Responsibility: write a backup generation when one is due, prune the generations past
//! the configured count, and hand the recovery pass a document it can rebuild a store from.
//! A generation is the store's own TSV interchange document and not a byte copy of
//! `user.redb`: the document survives a `redb` format upgrade, and the user can read it.
//!
//! Boundaries: this module owns the backup directory and nothing else. It never decodes,
//! never ranks and never opens the store's tables; it asks
//! [`UserFreqSource::export_tsv`] for the document and [`UserDb::import_tsv`] to read one
//! back, and it reports what happened as a [`BackupOutcome`] rather than logging, because
//! this crate has no logger. The damaged store itself is the recovery pass's business --
//! [`recover_user_db_with_backup`] calls [`crate::recover::recover_user_db`] and adds the
//! rollback on top, so the "isolate, never delete" rule keeps its single implementation.
//!
//! ```text
//! $XDG_DATA_HOME/rspinyin/
//! ├── user.redb
//! ├── user.redb.corrupt.<unix_ts>        the recovery pass's quarantine, never deleted
//! └── backups/                          0700
//!     ├── user-20260929-120000.tsv      0600
//!     ├── user-20260928-120000.tsv
//!     └── user-20260927-120000.tsv
//! ```
//!
//! # Nothing here runs on the input path
//!
//! A backup writes a file, so it may not run inside an fcitx5 callback (`ASM-04`,
//! `ASM-20`): the caller runs [`run_backup`] from the idle window the eviction sweep
//! already owns, and no thread is added for it. The entry point is a free function rather
//! than a method on the store for that reason -- there is no way to reach it from the
//! record path by accident.
//!
//! Whether a backup is *due* is a property of what is on disk and not of a counter held in
//! memory: the newest generation's name carries the stamp it was written at, so a caller
//! that asks in every idle window pays one directory listing and is told
//! [`BackupOutcome::Skipped`] until the interval has passed.
//!
//! # Degrading instead of failing
//!
//! A directory that cannot be prepared leaves the plugin exactly as it was: input keeps
//! working, learning continues, and [`BackupOutcome::Readonly`] says why the copies
//! stopped (`ASM-15`). A write that fails *after* the directory was prepared is
//! [`ImeError::DataBackupFailed`] instead, because a full disk is a condition the user can
//! act on.
//!
//! The rotation writes the new generation before it removes the old ones. The other order
//! would leave a window -- between the removal and the write -- in which the user has no
//! copy at all, and the window is exactly when a crash costs everything.
//!
//! # Trust model
//!
//! A generation is a file on disk like any other, and this module treats it as untrusted
//! input: a name that does not match the generation pattern is not a generation, a file
//! larger than [`EXPORT_LIMIT_BYTES`] is not one this build wrote, and a document whose
//! rows do not validate is refused whole by the import that reads it. A generation that
//! cannot be read is skipped and the next one is tried, newest first, so one bad copy does
//! not cost the user the rest.

use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

use crate::format::writer;

use super::*;

/// Directory the generations live in, inside the data directory.
pub const BACKUP_SUBDIR: &str = "backups";

/// The interval between automatic backups, in milliseconds.
///
/// One day. Learning is slow and the file is small, so a shorter interval would spend
/// writes to protect against a loss of at most one day of learning -- which the user would
/// barely notice.
pub const BACKUP_INTERVAL_MS: u64 = 24 * 60 * 60 * 1000;

/// The most backups kept, matching the `data.backup_keep` default.
pub const BACKUP_KEEP_DEFAULT: u8 = 3;

/// Ceiling on `data.backup_keep`.
///
/// One generation is at most [`EXPORT_LIMIT_BYTES`] on disk, so this bounds what the backup
/// directory can grow to at 256 MiB.
pub const MAX_BACKUP_KEEP: u8 = 32;

/// Suffixes tried after a generation name before the write is given up on.
///
/// Two backups stamped in the same second are a second process or a clock that went
/// backwards; a hundred of them in one second is not a case worth more code, and running
/// out is reported rather than resolved by overwriting anything.
const MAX_NAME_SUFFIX: u32 = 99;

/// Diagnostic code reported when a backup could not be written.
pub const BACKUP_FAILED_CODE: &str = "data/backup-failed";

/// Diagnostic code reported when a damaged store was rebuilt from a backup.
///
/// Informational rather than an error: the plugin recovered, and the user is told which
/// generation their words came back from.
pub const BACKUP_RESTORED_CODE: &str = "data/backup-restored";

/// One backup generation on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupFile {
    /// The path the generation is at.
    pub path: PathBuf,
    /// When the backup was taken, in unix milliseconds.
    ///
    /// Read from the generation's name, which carries whole seconds, so the value is always
    /// a whole second. Reading it from the name rather than from the file's modification
    /// time is what keeps the answer the same after a copy or a restore.
    pub taken_at_unix: u64,
    /// Rows it holds; counted from the document's data lines.
    pub rows: u64,
    /// Size in bytes.
    pub bytes: u64,
}

/// What a backup attempt did.
///
/// Not `Copy`, unlike the store's other outcome types: the two failure variants carry the
/// reason they failed with, and a reason is a `String`. Swallowing it instead would leave
/// the caller with an outcome it cannot report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackupOutcome {
    /// A new generation was written; the generations past `keep` were removed.
    Written,
    /// The newest generation is recent enough that another one was pointless.
    Skipped,
    /// The store or its backup directory cannot be written; the plugin runs read-only.
    ///
    /// Input keeps working and learning continues (`ASM-15`): what stopped is the copying.
    /// The caller reports `data/readonly-mode`, the code `crate::paths` declares.
    Readonly {
        /// Developer-facing explanation of what failed.
        reason: String,
    },
    /// The user turned backups off.
    Disabled,
    /// The store could not produce a backup document, so nothing was written.
    ///
    /// The store itself is healthy -- this is a store past the export ceiling, or one
    /// holding a key the interchange format cannot carry -- so the caller reports
    /// [`BACKUP_FAILED_CODE`] and carries on. A *write* that failed after the document was
    /// rendered is [`ImeError::DataBackupFailed`] instead, because the filesystem is the
    /// part the user can act on.
    Failed {
        /// Developer-facing explanation of what failed.
        reason: String,
    },
}

/// What a backup-aware recovery pass did with a damaged user store.
///
/// The counterpart of [`crate::recover::RecoveryOutcome`], which answers the same question
/// without a rollback. It is a second type rather than a variant of that one because the
/// damaged-file handling is not this module's to change: `recover_user_db` keeps its
/// outcome, and this one is what the backup layer adds to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreOutcome {
    /// The store was readable; nothing happened.
    Healthy,
    /// The store was damaged, isolated, and rebuilt empty.
    ///
    /// The damaged file is where the recovery pass put it -- renamed, never deleted -- and
    /// there was no backup to rebuild from.
    RebuiltEmpty,
    /// The store was damaged, isolated, and rebuilt from the newest usable backup.
    RestoredFromBackup {
        /// The generation that was used, in unix milliseconds.
        from_unix: u64,
        /// Rows it held.
        rows: u64,
    },
}

/// The backup policy, resolved from the `[data]` section of the configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupConfig {
    /// The directory the generations live in; [`backup_dir`] derives it from the layout.
    pub directory: PathBuf,
    /// `data.backup_enabled`: whether automatic backups run at all.
    pub enabled: bool,
    /// `data.backup_keep`: how many generations are kept, within
    /// `1..=`[`MAX_BACKUP_KEEP`].
    pub keep: u8,
    /// The interval between two automatic backups, in milliseconds.
    pub interval_ms: u64,
}

impl BackupConfig {
    /// The default policy for `directory`: enabled, [`BACKUP_KEEP_DEFAULT`] generations,
    /// [`BACKUP_INTERVAL_MS`] apart.
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            enabled: true,
            keep: BACKUP_KEEP_DEFAULT,
            interval_ms: BACKUP_INTERVAL_MS,
        }
    }

    /// The policy of a user who set `backup_enabled = false`.
    pub fn disabled(directory: impl Into<PathBuf>) -> Self {
        Self {
            enabled: false,
            ..Self::new(directory)
        }
    }

    /// Sets how many generations are kept, clamped to what the rotation can honour.
    ///
    /// Zero is clamped to one rather than honoured: a rotation that keeps nothing would
    /// remove the generation it had just written, which is the opposite of what the setting
    /// is for. The configuration layer validates the key against `1..=`[`MAX_BACKUP_KEEP`]
    /// and reports a value outside it; the clamp is the second line of defence for a caller
    /// that builds the policy itself.
    pub fn with_keep(mut self, keep: u8) -> Self {
        self.keep = keep.clamp(1, MAX_BACKUP_KEEP);
        self
    }

    /// Sets the interval between two automatic backups, in milliseconds.
    ///
    /// Zero means "always due", which is what a caller that backs up on demand wants.
    pub fn with_interval_ms(mut self, interval_ms: u64) -> Self {
        self.interval_ms = interval_ms;
        self
    }
}

/// The directory the generations of a data directory live in.
///
/// Derived rather than stored so that the backup layout cannot drift from the layout
/// [`crate::paths`] hands out: both name the same data directory.
pub fn backup_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(BACKUP_SUBDIR)
}

/// Lists the backup generations under `directory`, newest first.
///
/// A file that is not a generation -- a name that does not match the pattern, a symbolic
/// link, one larger than the export ceiling -- is not listed. A tie on the stamp is broken
/// by the path, so two listings of an unchanged directory agree.
///
/// # Errors
/// Returns [`ImeError::DictUnavailable`] naming `directory` when it exists and cannot be
/// read. A directory that does not exist is an empty listing rather than an error: the
/// first backup is what creates it.
pub fn list_backups(directory: &Path) -> Result<Vec<BackupFile>, ImeError> {
    scan_generations(directory)
}

/// Writes a backup if one is due, then rotates the old generations.
///
/// The caller runs this from the idle window and not from a key callback: it lists a
/// directory and writes a file, and nothing on the input path may do either (`ASM-04`,
/// `ASM-20`).
///
/// # Parameters
/// - `store`: the user frequency store to copy. Only its interchange document is read.
/// - `cfg`: the policy; a disabled one answers [`BackupOutcome::Disabled`] without touching
///   the disk.
/// - `now_unix_ms`: the wall-clock stamp the generation is named after and the "is one
///   due?" question is answered against. Injected because a test may not read the clock.
///
/// # Return value
/// [`BackupOutcome::Written`] when a generation landed, [`BackupOutcome::Skipped`] when the
/// newest one is younger than the interval, [`BackupOutcome::Disabled`] when the user
/// turned backups off, [`BackupOutcome::Readonly`] when the store or the directory cannot
/// be written, and [`BackupOutcome::Failed`] when the store could not render a document.
///
/// # Errors
/// Returns [`ImeError::DataBackupFailed`] only when a backup was due and could not be
/// written. A skipped, disabled or read-only backup is a success: the caller has nothing to
/// do about any of them, and treating them as errors would put a failure in the log every
/// time a user typed.
///
/// # Panics
/// Never: every failure of the listing, the rendering, the write and the rotation is
/// carried in the outcome.
pub fn run_backup(
    store: &UserDb,
    cfg: &BackupConfig,
    now_unix_ms: u64,
) -> Result<BackupOutcome, ImeError> {
    if !cfg.enabled {
        return Ok(BackupOutcome::Disabled);
    }
    if store.is_readonly() {
        return Ok(BackupOutcome::Readonly {
            reason: String::from("the user store is read-only"),
        });
    }
    // A listing that fails means the directory cannot be read, and a directory that cannot
    // be read cannot be written either: the answer is the read-only degradation and not an
    // error, exactly as an unwritable directory is.
    let existing = match scan_generations(&cfg.directory) {
        Ok(found) => found,
        Err(error) => {
            return Ok(BackupOutcome::Readonly {
                reason: error.to_string(),
            });
        }
    };
    if let Some(newest) = existing.first() {
        // Saturating: a clock that went backwards makes the newest generation look like it
        // was written in the future, and a backup that is never due is the safe reading of
        // that. The alternative -- an unsigned subtraction that wraps -- would make every
        // idle window a backup window.
        if now_unix_ms.saturating_sub(newest.taken_at_unix) < cfg.interval_ms {
            return Ok(BackupOutcome::Skipped);
        }
    }
    let mut document = Vec::new();
    if let Err(error) = store.export_tsv(&mut document) {
        return Ok(BackupOutcome::Failed {
            reason: error.to_string(),
        });
    }
    if !prepare_directory(&cfg.directory) {
        return Ok(BackupOutcome::Readonly {
            reason: format!(
                "the backup directory {} could not be prepared",
                cfg.directory.display()
            ),
        });
    }
    write_document(&cfg.directory, now_unix_ms, &document)?;
    prune(&cfg.directory, cfg.keep);
    Ok(BackupOutcome::Written)
}

/// Isolates a damaged store and rebuilds it, preferring a backup.
///
/// The damaged file is renamed by the recovery pass, never deleted: a user who wants to
/// salvage rows by hand must still be able to, and the backup may itself be older than the
/// damage. Every generation is tried newest-first and the first one that imports cleanly
/// wins, so one bad generation does not cost the user everything. The order the caller
/// passes them in does not matter -- they are sorted here, so the function cannot be called
/// wrong.
///
/// # Parameters
/// - `path`: the store, normally `user.redb`.
/// - `backups`: the generations to try, normally [`list_backups`] of [`backup_dir`]. An
///   empty slice is the no-backup path, which is the behaviour of
///   [`crate::recover::recover_user_db`].
///
/// # Return value
/// [`RestoreOutcome::Healthy`] when the store opens and nothing was touched,
/// [`RestoreOutcome::RebuiltEmpty`] when a damaged store was replaced by an empty one and
/// there was nothing to restore from, and [`RestoreOutcome::RestoredFromBackup`] when one
/// was.
///
/// # Errors
/// Returns [`ImeError::DataBackupFailed`] when a generation existed but none could be
/// imported; the store is still left usable and empty in that case, so input keeps working.
/// Returns [`ImeError::DataReadonly`] -- the `data/readonly-mode` code -- when the damaged
/// store could not be replaced at all.
///
/// # Panics
/// Never: a recovery pass runs during start-up and may not cost the user their input
/// method, so every failure it can meet is carried in the outcome or returned.
pub fn recover_user_db_with_backup(
    path: &Path,
    backups: &[BackupFile],
) -> Result<RestoreOutcome, ImeError> {
    if let Ok(store) = UserDb::open(path) {
        // The handle owns the store's lock; it is dropped here so that the caller can open
        // the store itself.
        drop(store);
        return Ok(RestoreOutcome::Healthy);
    }
    if crate::recover::recover_user_db(path).is_healthy() {
        // The probe above refused the file and this pass accepted it: the file changed
        // between the two calls. Nothing was moved, so there is nothing to rebuild and
        // nothing to restore -- rebuilding a store that works would throw the user's own
        // records away.
        return Ok(RestoreOutcome::Healthy);
    }
    let fresh = match UserDb::open(path) {
        Ok(store) => store,
        Err(error) => {
            return Err(ImeError::DataReadonly {
                reason: format!("the damaged user store could not be replaced: {error}"),
            });
        }
    };
    if backups.is_empty() {
        return Ok(RestoreOutcome::RebuiltEmpty);
    }
    let mut tried = 0usize;
    for candidate in newest_first(backups) {
        tried = tried.saturating_add(1);
        if let Some(rows) = import_generation(&fresh, candidate) {
            return Ok(RestoreOutcome::RestoredFromBackup {
                from_unix: candidate.taken_at_unix,
                rows,
            });
        }
    }
    Err(ImeError::DataBackupFailed {
        reason: format!("{tried} backup generations were tried and none could be imported"),
    })
}

/// Returns the generations newest first, whatever order they were given in.
fn newest_first(backups: &[BackupFile]) -> Vec<&BackupFile> {
    let mut ordered: Vec<&BackupFile> = backups.iter().collect();
    ordered.sort_by(|left, right| {
        right
            .taken_at_unix
            .cmp(&left.taken_at_unix)
            .then_with(|| left.path.cmp(&right.path))
    });
    ordered
}

/// Imports one generation into `store`, or answers `None` when it does not import cleanly.
///
/// The store this runs against is the empty one the recovery pass just created, so the
/// import *is* the restore: `import_tsv` merges rather than replaces, and there is nothing
/// to merge with.
///
/// A document that is not recognised, or one holding a row that failed validation, is
/// refused whole by the import and reported as a generation that did not work out.
fn import_generation(store: &UserDb, generation: &BackupFile) -> Option<u64> {
    let file = File::open(&generation.path).ok()?;
    let mut reader = BufReader::new(file);
    let report = store.import_tsv(&mut reader).ok()?;
    if !report.recognised || report.rejected > 0 {
        return None;
    }
    Some(report.applied)
}

/// Reads every generation under `directory`, newest first.
///
/// # Errors
/// Returns [`ImeError::DictUnavailable`] naming `directory` when it exists and cannot be
/// read.
fn scan_generations(directory: &Path) -> Result<Vec<BackupFile>, ImeError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(store_error(directory, &error)),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| store_error(directory, &error))?;
        if let Some(generation) = read_generation(&entry.path()) {
            found.push(generation);
        }
    }
    found.sort_by(|left, right| {
        right
            .taken_at_unix
            .cmp(&left.taken_at_unix)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(found)
}

/// Reads one generation from `path`, or answers `None` when the file is not one.
///
/// The file's *contents* are never parsed here -- only its name, its size and the number of
/// lines that carry data. What a row means is the import's question, and it is the only
/// reader that has to trust the answer.
fn read_generation(path: &Path) -> Option<BackupFile> {
    let name = path.file_name()?.to_str()?;
    let taken_at_unix = stamp_of_name(name)?;
    let meta = std::fs::symlink_metadata(path).ok()?;
    // `symlink_metadata` does not follow the link, so a symbolic link is neither a file nor
    // a generation: the plugin never wrote one, and following it would let whatever it
    // points at pass itself off as the user's words.
    if !meta.is_file() {
        return None;
    }
    let bytes = meta.len();
    if bytes > EXPORT_LIMIT_BYTES {
        return None;
    }
    let rows = count_rows(path)?;
    Some(BackupFile {
        path: path.to_path_buf(),
        taken_at_unix,
        rows,
        bytes,
    })
}

/// Counts the lines of `path` that carry data: no blank lines and no comments.
///
/// The interchange format this build writes names its columns and its marker in the header
/// but carries no row count, so the count comes from the body. Reading it line by line
/// validates nothing -- a document whose rows are wrong is caught by the import -- and the
/// work is bounded by the size ceiling [`read_generation`] has already checked.
fn count_rows(path: &Path) -> Option<u64> {
    let file = File::open(path).ok()?;
    let mut rows = 0u64;
    for line in BufReader::new(file).lines() {
        // A line that is not UTF-8 cannot be one of ours, and there is no row after it that
        // could be counted: the document ends here.
        let Ok(line) = line else { break };
        let line = line.trim_end();
        if !line.is_empty() && !line.starts_with('#') {
            rows = rows.saturating_add(1);
        }
    }
    Some(rows)
}

/// Creates the backup directory with [`DIR_MODE`] and refuses one that is a symbolic link.
///
/// Returns whether the directory is usable afterwards. A link is refused for the reason the
/// layout refuses one: a base directory is a place another account could pre-place a link,
/// and writing the user's vocabulary through it would put the copies somewhere the user did
/// not choose. An existing directory is narrowed rather than trusted, because a wider mode
/// survives a copy or a restore from a backup.
fn prepare_directory(directory: &Path) -> bool {
    match std::fs::symlink_metadata(directory) {
        Ok(meta) if meta.file_type().is_symlink() => return false,
        Ok(meta) if meta.is_dir() => {
            return tighten_directory(directory, meta.permissions().mode());
        }
        Ok(_) => return false,
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(_) => return false,
    }
    if std::fs::DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(directory)
        .is_err()
    {
        return false;
    }
    tighten_directory(directory, DIR_MODE)
}

/// Narrows a directory another account can still reach, and reports whether it is usable.
///
/// The test is "does group or other hold a bit", not "is the mode exactly [`DIR_MODE`]": a
/// mode the user narrowed on purpose is left alone, while what the baseline promises is
/// that nothing the plugin owns is reachable by a second account.
fn tighten_directory(directory: &Path, mode_bits: u32) -> bool {
    if mode_bits & 0o077 == 0 {
        return true;
    }
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(DIR_MODE)).is_ok()
}

/// Writes `document` into a new generation under `directory` and returns its path.
///
/// # Errors
/// Returns [`ImeError::DataBackupFailed`] when no generation name is free, or when the
/// temporary file cannot be written or renamed onto it.
fn write_document(directory: &Path, stamp_ms: u64, document: &[u8]) -> Result<PathBuf, ImeError> {
    let Some(path) = free_generation_path(directory, stamp_ms) else {
        return Err(ImeError::DataBackupFailed {
            reason: format!(
                "no free generation name is available under {}",
                directory.display()
            ),
        });
    };
    let temp = writer::temp_path(&path);
    match write_and_rename(&temp, &path, document) {
        Ok(()) => Ok(path),
        Err(error) => {
            // The temporary holds at best a prefix of the document, and the claimed name
            // holds the empty file the claim created: both are this call's own litter, and
            // neither may be left where a later listing would read it as a generation.
            let _ = std::fs::remove_file(&temp);
            let _ = std::fs::remove_file(&path);
            Err(ImeError::DataBackupFailed {
                reason: format!("{} could not be written: {error}", path.display()),
            })
        }
    }
}

/// Writes `bytes` to `temp` and renames `temp` onto `target`, atomically.
///
/// The temporary is created private from the moment it exists -- a backup holds the user's
/// whole vocabulary -- and flushed to the device before the rename, so the name is never
/// published over bytes that are still only in the page cache.
///
/// # Errors
/// Returns the underlying [`std::io::Error`] when the temporary cannot be created, written
/// or flushed, or when the rename fails.
fn write_and_rename(temp: &Path, target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = crate::paths::create_private(temp)?;
    // A temporary left behind by a crash still holds the previous document and `open` does
    // not truncate, so the tail of the older one would survive past the end of the new one.
    file.set_len(0)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    // Closed before the rename: a descriptor is not a name, and leaving one open across the
    // rename would keep a handle to the temporary alive after the name is gone.
    drop(file);
    std::fs::rename(temp, target)
}

/// Reserves the first free generation name for `stamp_ms`.
///
/// `rename(2)` replaces its destination silently, so the name is claimed exclusively before
/// anything is written: two generations stamped in the same second -- a second process, or
/// a clock that went backwards -- land on two names instead of one destroying the other.
fn free_generation_path(directory: &Path, stamp_ms: u64) -> Option<PathBuf> {
    let base = directory.join(generation_name(stamp_ms));
    for suffix in 0..=MAX_NAME_SUFFIX {
        let candidate = if suffix == 0 {
            base.clone()
        } else {
            suffixed(&base, suffix)
        };
        match claim(&candidate) {
            Ok(()) => return Some(candidate),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

/// Reserves `path` by creating it exclusively, with mode [`FILE_MODE`].
///
/// # Errors
/// Returns [`ErrorKind::AlreadyExists`] when the name is taken, and any other failure of
/// the underlying `open` as it stands.
fn claim(path: &Path) -> std::io::Result<()> {
    let reserved = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(path)?;
    drop(reserved);
    Ok(())
}

/// Appends `.suffix` to a generation name, before its extension, so that the suffixed
/// spelling still reads as a generation of the same stamp.
fn suffixed(base: &Path, suffix: u32) -> PathBuf {
    let mut name = base.file_stem().unwrap_or_default().to_os_string();
    name.push(format!(".{suffix}.tsv"));
    base.with_file_name(name)
}

/// Removes every generation past the newest `keep`, oldest first.
///
/// Best effort, and deliberately last: the new generation is on disk before this runs, so
/// the user is never left with no copy at all, and a generation that cannot be removed is
/// left where it is for the next attempt rather than reported. The removal's own failure is
/// not an error the caller could act on -- the copies the policy promised are all there.
fn prune(directory: &Path, keep: u8) {
    let Ok(found) = scan_generations(directory) else {
        return;
    };
    for stale in found.into_iter().skip(usize::from(keep)) {
        let _ = std::fs::remove_file(&stale.path);
    }
}

/// The name a generation written at `stamp_ms` carries.
///
/// `user-YYYYMMDD-HHMMSS.tsv`, in UTC: a name the user can read and sort, and one whose
/// stamp survives being copied to another machine, which a modification time does not.
fn generation_name(stamp_ms: u64) -> String {
    let seconds = stamp_ms / 1_000;
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
    let rest = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "user-{year:04}{month:02}{day:02}-{:02}{:02}{:02}.tsv",
        rest / 3_600,
        (rest % 3_600) / 60,
        rest % 60
    )
}

/// The unix millisecond stamp a generation's name carries, or `None` when `name` is not a
/// generation of this format.
///
/// The pattern is matched exactly rather than leniently: a name this build did not write is
/// not a generation, and a file the user dropped into the directory must not become one.
/// A suffix claimed for a second generation in the same second is accepted, and the stamp
/// is what both spellings share.
fn stamp_of_name(name: &str) -> Option<u64> {
    let rest = name.strip_prefix("user-")?.strip_suffix(".tsv")?;
    // Every field below is indexed by byte, so the ASCII test is what makes the slicing
    // below sound rather than merely probable.
    if !rest.is_ascii() {
        return None;
    }
    let stamp = rest.split('.').next()?;
    if stamp.len() != 15 || stamp.as_bytes()[8] != b'-' {
        return None;
    }
    let year = decimal(&stamp[0..4])?;
    let month = decimal(&stamp[4..6])?;
    let day = decimal(&stamp[6..8])?;
    let hour = decimal(&stamp[9..11])?;
    let minute = decimal(&stamp[11..13])?;
    let second = decimal(&stamp[13..15])?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days = days_from_civil(i64::from(year), month, day);
    // A name before 1970 is not a stamp this build wrote, and `u64` has nowhere to put it.
    let days = u64::try_from(days).ok()?;
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(u64::from(hour) * 3_600 + u64::from(minute) * 60 + u64::from(second))?;
    seconds.checked_mul(1_000)
}

/// Reads a run of ASCII digits as a number, refusing anything else.
fn decimal(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Converts days since 1970-01-01 into a civil `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`, which is exact for every day an `i64` can hold and
/// needs no table. Written out rather than taken from a crate because the only date
/// arithmetic this crate performs is naming a backup file, and a calendar crate is a
/// dependency to review and carry for one `format!` call.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // The epoch is shifted to 0000-03-01, which puts the leap day at the end of the year
    // and makes every era exactly 146_097 days long.
    let shifted = days.saturating_add(719_468);
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month as u32, day as u32)
}

/// Converts a civil `(year, month, day)` back into days since 1970-01-01.
///
/// The inverse of [`civil_from_days`], and the half that reads a stamp out of a name.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let month = i64::from(month);
    let day = i64::from(day);
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests;
