//! Schema creation, validation and the one-way schema-1 migration.

use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use ime_types::{DictError, ImeError};
use redb::{Database, Durability, ReadableTable};

use super::{
    DIR_MODE, FILE_MODE, META, META_TICK, SCHEMA_KEY, SCHEMA_VERSION, SCHEMA_VERSION_V1, TICK_KEY,
    USER_KEY_PINS, USER_META, USER_WORDS, USER_WORDS_V2, store_error,
};

#[cfg(test)]
use super::take_injected_migration_failure;

/// Sets the mode of `path` when it differs from `mode`.
///
/// # Errors
/// Returns the underlying `std::io::Error` when the metadata or the mode change fails.
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    let current = std::fs::metadata(path)?.permissions().mode() & 0o777;
    if current == mode {
        return Ok(());
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// Creates the directory and the file the store needs, with the modes it must keep.
///
/// The file is created here rather than by `redb`, which would create it `0644` under the
/// user's umask, and the user database is private data. An existing file is tightened
/// rather than trusted, because a wider mode survives a copy or a restore from a backup.
///
/// # Errors
/// Returns [`ImeError::DictUnavailable`] when the directory cannot be created, or the file
/// cannot be created or tightened.
pub(super) fn prepare_path(path: &Path) -> Result<(), ImeError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent).map_err(|error| store_error(path, &error))?;
        set_mode(parent, DIR_MODE).map_err(|error| store_error(path, &error))?;
    }
    if path.exists() {
        return set_mode(path, FILE_MODE).map_err(|error| store_error(path, &error));
    }
    let created = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(FILE_MODE)
        .open(path)
        .map_err(|error| store_error(path, &error))?;
    drop(created);
    Ok(())
}

/// Creates the tables and stamps the schema version this build writes.
///
/// # Errors
/// Returns [`ImeError::DictUnavailable`] when the store cannot be written, or when it
/// already carries a schema version this build does not know.
fn stamp_schema(path: &Path, db: &Database) -> Result<(), ImeError> {
    let mut txn = db
        .begin_write()
        .map_err(|error| store_error(path, &error))?;
    {
        let mut meta = txn
            .open_table(META)
            .map_err(|error| store_error(path, &error))?;
        let found = meta
            .get(SCHEMA_KEY)
            .map_err(|error| store_error(path, &error))?
            .map(|guard| guard.value());
        if let Some(version) = found
            && version != SCHEMA_VERSION
        {
            return Err(ImeError::DictUnavailable {
                path: path.to_path_buf(),
                cause: DictError::FormatVersion {
                    found: u16::try_from(version).unwrap_or(u16::MAX),
                },
            });
        }
        meta.insert(SCHEMA_KEY, SCHEMA_VERSION)
            .map_err(|error| store_error(path, &error))?;
        txn.open_table(USER_WORDS_V2)
            .map_err(|error| store_error(path, &error))?;
        // Created here rather than on first use: a read transaction cannot create a table,
        // so a store whose metadata table was never written would fail the first
        // enumeration instead of answering an empty one.
        txn.open_table(USER_META)
            .map_err(|error| store_error(path, &error))?;
        txn.open_table(USER_KEY_PINS)
            .map_err(|error| store_error(path, &error))?;
        let mut tick = txn
            .open_table(META_TICK)
            .map_err(|error| store_error(path, &error))?;
        tick.insert(TICK_KEY, 0)
            .map_err(|error| store_error(path, &error))?;
    }
    txn.set_durability(Durability::Immediate);
    txn.commit().map_err(|error| store_error(path, &error))?;
    Ok(())
}

/// The decay accumulator's storage encoding: f32 bits. A storage encoding only -- the
/// scoring arithmetic on the value is integer fixed point, so no float order can leak
/// into a ranking.
pub(super) fn dee_bits(dee_q16: i64) -> u32 {
    f32::to_bits(dee_q16 as f32 / 65_536.0)
}

/// Reads the global logical clock the file holds.
///
/// # Errors
/// Returns [`ImeError::DictUnavailable`] when the read fails; the clock row is written
/// with the tables, so a store that opens without one is a store this build did not
/// prepare and cannot serve.
pub(super) fn read_tick(path: &Path, db: &Database) -> Result<u64, ImeError> {
    let txn = db.begin_read().map_err(|error| store_error(path, &error))?;
    let tick = txn
        .open_table(META_TICK)
        .map_err(|error| store_error(path, &error))?;
    tick.get(TICK_KEY)
        .map_err(|error| store_error(path, &error))?
        .map(|guard| guard.value())
        .ok_or_else(|| {
            store_error(
                path,
                &"the logical clock row is missing from the tick table",
            )
        })
}

/// The schema state an open finds, and what this build did about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SchemaOutcome {
    /// A fresh file: the tables were created and the version stamped.
    Fresh,
    /// A store this build already wrote; nothing to do.
    Current,
    /// A schema-1 store this build migrated to schema 2.
    Migrated,
    /// A schema-1 store whose migration failed; the store opens read-only over v1
    /// and the caller reports [`SCHEMA_MIGRATED_CODE`].
    V1Degraded,
}

/// Brings the store's schema to the version this build writes.
///
/// A fresh file is stamped; a current one is left alone; a schema-1 store is migrated
/// in one write transaction (see the module documentation); and a migration whose
/// transaction fails degrades to [`SchemaOutcome::V1Degraded`] rather than refusing to
/// open -- a user database that cannot be upgraded must still serve the history it
/// holds, and the readonly discipline is how a store that cannot be written stays
/// useful.
///
/// # Errors
/// Returns [`ImeError::DictUnavailable`] when a store neither fresh nor readable names a
/// schema version this build does not know, or when a table or transaction outside the
/// migration itself fails.
pub(super) fn prepare_schema(path: &Path, db: &Database) -> Result<SchemaOutcome, ImeError> {
    let version = {
        let txn = db.begin_read().map_err(|error| store_error(path, &error))?;
        match txn.open_table(META) {
            Ok(meta) => meta
                .get(SCHEMA_KEY)
                .map_err(|error| store_error(path, &error))?
                .map(|guard| guard.value()),
            // A file with no `meta` table at all is a fresh one: the store this build
            // is about to create, or the zero-byte file `prepare_path` made before
            // `redb` wrote anything. That is the "no version" state, not a failure.
            Err(redb::TableError::TableDoesNotExist(_)) => None,
            Err(error) => return Err(store_error(path, &error)),
        }
    };
    match version {
        None => {
            stamp_schema(path, db)?;
            Ok(SchemaOutcome::Fresh)
        }
        Some(found) if found == SCHEMA_VERSION => {
            ensure_tables(path, db)?;
            Ok(SchemaOutcome::Current)
        }
        Some(found) if found == SCHEMA_VERSION_V1 => {
            if migrate_v1(path, db).is_ok() {
                Ok(SchemaOutcome::Migrated)
            } else {
                Ok(SchemaOutcome::V1Degraded)
            }
        }
        Some(other) => Err(ImeError::DictUnavailable {
            path: path.to_path_buf(),
            cause: DictError::FormatVersion {
                found: u16::try_from(other).unwrap_or(u16::MAX),
            },
        }),
    }
}

/// Opens every table a current store carries, creating the ones a fresh schema would
/// have made.
///
/// A read transaction cannot create a table, so a store whose auxiliary table was never
/// written would fail its first enumeration instead of answering an empty one; the
/// creation belongs to the open path, as it did under schema 1.
///
/// # Errors
/// Returns [`ImeError::DictUnavailable`] when a table cannot be created.
fn ensure_tables(path: &Path, db: &Database) -> Result<(), ImeError> {
    let txn = db
        .begin_write()
        .map_err(|error| store_error(path, &error))?;
    txn.open_table(USER_WORDS_V2)
        .map_err(|error| store_error(path, &error))?;
    txn.open_table(USER_META)
        .map_err(|error| store_error(path, &error))?;
    txn.open_table(USER_KEY_PINS)
        .map_err(|error| store_error(path, &error))?;
    txn.open_table(META_TICK)
        .map_err(|error| store_error(path, &error))?;
    txn.commit().map_err(|error| store_error(path, &error))?;
    Ok(())
}

/// Migrates a schema-1 store to schema 2 in one write transaction.
///
/// Every v1 row enters v2 with `tick = 0` and `dee = commits`: the migration folds no
/// history, because a v1 row has no tick trace to fold from. The clock itself starts at
/// zero, so at the migration instant the rows are un-aged and the fold reads them exactly
/// as v1 ranked them; the decay begins as the user's next commits advance the clock. The
/// v1 table is dropped once its rows are copied -- the upgrade is one-way (`ASM-A-09`),
/// and a crash rolls the whole transaction back, leaving the file a v1 store the next
/// open migrates again.
///
/// # Errors
/// Returns [`ImeError::DictUnavailable`] when the transaction or any table fails; the
/// caller degrades to a read-only v1 store on any of them.
fn migrate_v1(path: &Path, db: &Database) -> Result<(), ImeError> {
    #[cfg(test)]
    if take_injected_migration_failure() {
        return Err(store_error(path, &"injected migration failure"));
    }
    let mut txn = db
        .begin_write()
        .map_err(|error| store_error(path, &error))?;
    {
        let v1 = txn
            .open_table(USER_WORDS)
            .map_err(|error| store_error(path, &error))?;
        let mut v2 = txn
            .open_table(USER_WORDS_V2)
            .map_err(|error| store_error(path, &error))?;
        for entry in v1.iter().map_err(|error| store_error(path, &error))? {
            let (key, value) = entry.map_err(|error| store_error(path, &error))?;
            let (commits, last_used) = value.value();
            let dee = i64::from(commits) << 16;
            v2.insert(key.value(), (commits, last_used, 0, dee_bits(dee)))
                .map_err(|error| store_error(path, &error))?;
        }
        let mut tick = txn
            .open_table(META_TICK)
            .map_err(|error| store_error(path, &error))?;
        tick.insert(TICK_KEY, 0)
            .map_err(|error| store_error(path, &error))?;
        // The auxiliary v2 tables are created with the rows: a store whose metadata
        // table does not exist would fail its first enumeration (a read transaction
        // cannot create a table) and its whole load with it, so the migration leaves
        // behind every table a fresh schema would have.
        txn.open_table(USER_META)
            .map_err(|error| store_error(path, &error))?;
        txn.open_table(USER_KEY_PINS)
            .map_err(|error| store_error(path, &error))?;
        let mut meta = txn
            .open_table(META)
            .map_err(|error| store_error(path, &error))?;
        meta.insert(SCHEMA_KEY, SCHEMA_VERSION)
            .map_err(|error| store_error(path, &error))?;
    }
    // The borrow of `v1` ends with the block above, so the drop here is allowed. Reopening
    // a v1-shaped table afterwards would answer an empty one, which is why the drop and
    // the version stamp share the transaction: either both land or neither does.
    txn.delete_table(USER_WORDS)
        .map_err(|error| store_error(path, &error))?;
    txn.set_durability(Durability::Immediate);
    txn.commit().map_err(|error| store_error(path, &error))?;
    Ok(())
}
