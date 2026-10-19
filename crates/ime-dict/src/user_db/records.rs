//! Stored user records and the schema-adaptive words-table view.

use std::path::Path;

use ime_types::ImeError;
use redb::ReadableTable;

use super::{dee_bits, store_error};

/// The working form of a stored accumulator: Q16.16 fixed point.
pub(super) fn dee_q16(bits: u32) -> i64 {
    let held = f32::from_bits(bits);
    // The `as` cast saturates instead of overflowing, and a value past the integer
    // domain was already unusable as a weight -- the saturation is the floor behaviour
    // the decay expects, not a silent error.
    (held * 65_536.0) as i64
}

/// One hydrated record: what the store flushed, in the form the read paths consume.
///
/// `dee_q16` is the decay accumulator in Q16.16 and `tick` the record's own logical
/// stamp; both are the inputs the decay fold consumes. Until that fold
/// arrives the read paths answer `commits`, which is the v1 semantic the store keeps
/// for an un-decayed record.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Stored {
    /// Commits so far, saturating at `u32::MAX`.
    pub(crate) commits: u32,
    /// The decay accumulator in Q16.16.
    pub(crate) dee_q16: i64,
    /// The record's logical tick.
    pub(crate) tick: u64,
}

impl Stored {
    /// The record a fresh write of `commits` commits carries, with no decay applied.
    pub(crate) fn identity(commits: u32) -> Self {
        Self {
            commits,
            dee_q16: i64::from(commits) << 16,
            tick: 0,
        }
    }

    /// The stored encoding of this record's accumulator.
    pub(crate) fn dee_bits(&self) -> u32 {
        dee_bits(self.dee_q16)
    }
}

/// One record as a file row holds it: a [`Stored`] plus the wall-clock column the
/// management surface still speaks.
#[derive(Clone, Copy, Debug)]
pub(super) struct StoredRow {
    /// Commits so far.
    pub(super) commits: u32,
    /// Last use, in unix milliseconds.
    pub(super) last_used_ms: u64,
    /// The record's logical tick.
    pub(super) tick: u64,
    /// The decay accumulator in Q16.16.
    pub(super) dee_q16: i64,
}

impl StoredRow {
    /// The in-memory form the hot read paths hold.
    pub(super) fn as_stored(&self) -> Stored {
        Stored {
            commits: self.commits,
            dee_q16: self.dee_q16,
            tick: self.tick,
        }
    }

    /// The row a v1 file's record reads as: no tick, an un-decayed accumulator.
    pub(super) fn from_v1(commits: u32, last_used_ms: u64) -> Self {
        Self {
            commits,
            last_used_ms,
            tick: 0,
            dee_q16: i64::from(commits) << 16,
        }
    }
}

/// The words table the store actually carries, in the schema it was opened with.
///
/// A store whose migration failed serves its v1 table read-only, so every read of the
/// words table goes through this view rather than naming a table. A v1 row reads as
/// its v2 equivalent with no tick and an un-decayed accumulator -- which is exactly
/// what it is.
pub(super) enum WordsView {
    /// The v1 table of a store whose migration failed.
    V1(redb::ReadOnlyTable<&'static str, (u32, u64)>),
    /// The table this build writes.
    V2(redb::ReadOnlyTable<&'static str, (u32, u64, u64, u32)>),
}

impl WordsView {
    /// The stored record `key` maps to, or `None` when the store does not hold it.
    ///
    /// # Errors
    /// Returns the underlying `redb` failure; every caller renders it against the
    /// store's own path.
    pub(super) fn get(&self, key: &str) -> Result<Option<Stored>, redb::StorageError> {
        match self {
            Self::V1(table) => Ok(table
                .get(key)?
                .map(|guard| Stored::identity(guard.value().0))),
            Self::V2(table) => Ok(table.get(key)?.map(|guard| {
                let (commits, _, tick, dee) = guard.value();
                Stored {
                    commits,
                    dee_q16: dee_q16(dee),
                    tick,
                }
            })),
        }
    }

    /// Feeds every record, in key order, to `visit`.
    ///
    /// # Errors
    /// Returns [`ImeError::DictUnavailable`] for the iteration itself -- rendered against
    /// `path`, the store the view was opened from -- and whatever `visit` reports for a
    /// row.
    pub(super) fn for_each<F>(&self, path: &Path, mut visit: F) -> Result<(), ImeError>
    where
        F: FnMut(&str, StoredRow) -> Result<(), ImeError>,
    {
        let failed = |error: redb::StorageError| store_error(path, &error);
        match self {
            Self::V1(table) => {
                for entry in table.iter().map_err(failed)? {
                    let (key, value) = entry.map_err(failed)?;
                    let (commits, last_used_ms) = value.value();
                    visit(key.value(), StoredRow::from_v1(commits, last_used_ms))?;
                }
            }
            Self::V2(table) => {
                for entry in table.iter().map_err(failed)? {
                    let (key, value) = entry.map_err(failed)?;
                    let (commits, last_used_ms, tick, dee) = value.value();
                    visit(
                        key.value(),
                        StoredRow {
                            commits,
                            last_used_ms,
                            tick,
                            dee_q16: dee_q16(dee),
                        },
                    )?;
                }
            }
        }
        Ok(())
    }
}
