//! The load-side half of the v2 scenarios: the real user store a `[user_db]` section
//! names, built while the fixture is assembled.
//!
//! Responsibility: open the store a v2 scenario runs against, at a scratch path, with the
//! fixed clock and the decay policy its sections state, and record the seed file's
//! commits into it, and take the scratch directories away again when a fixture drops.
//! Boundaries: everything here runs at load and at teardown, beside the code that reads
//! the scenario file itself, and nothing here runs during the drive -- a replay step opens
//! no file and reads no clock, which is the purity the source scan and `--strace` hold the
//! drive path to.
//!
//! # Where the ageing comes from
//!
//! The store's logical clock advances once per `record`, so the seeds are their own
//! clock: a word seeded early is older than a word seeded late by exactly the commits
//! recorded between them, and the `[decay]` section pins the rate that ageing runs at.
//! The wall-clock half of every stamp answers zero, because no scenario pins a wall
//! clock and a frozen one cannot surprise a replay.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::Context;
use ime_dict::user_db::{Clock, DecayParams, UserDb};
use ime_types::UserFreqSource;

use super::v2::{UserDbFixture, read_seed};

/// Serial number for scratch directories; tests in one binary run in parallel threads.
static COUNTER: AtomicU32 = AtomicU32::new(0);

/// The real user store a `[user_db]` scenario runs against.
pub(super) struct StoreFixture {
    /// The store handle the fixture injects.
    pub(super) db: UserDb,
    /// The directory the store's file lives in, removed when the fixture drops.
    pub(super) dir: PathBuf,
}

/// A clock that answers nothing: the stamps a replay's store carries are the records'
/// logical ticks, and the wall-clock half of a stamp has no reading a scenario pins.
#[derive(Debug)]
struct FixedClock;

impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        0
    }

    fn now_nanos(&self) -> u64 {
        0
    }
}

/// Builds the real store a `[user_db]` section names.
///
/// The store is opened with a fixed clock and the section's decay policy, and a seeded
/// fixture records each word the seed file names once per commit the file states.
///
/// # Errors
///
/// An error naming the store when its directory or file cannot be created, when the decay
/// policy is degenerate, or when the seed file cannot be read.
///
/// # Panics
///
/// Never panics.
pub(super) fn build_store(
    fixture: &UserDbFixture,
    decay: Option<super::DecayClock>,
) -> anyhow::Result<StoreFixture> {
    let dir = scratch_dir("userdb")?;
    let path = dir.join("user.redb");
    let params = match decay {
        Some(clock) => DecayParams::new(clock.half_life_ticks, 1 << 16)
            .map_err(|error| anyhow::anyhow!("the [decay] clock is degenerate: {error}"))?,
        None => DecayParams::default(),
    };
    let db = UserDb::open_with_decay(&path, Box::new(FixedClock), params)
        .map_err(|error| anyhow::anyhow!("opening the store at {}: {error}", path.display()))?;
    for (text, commits) in read_seed(fixture)? {
        for _ in 0..commits {
            db.record(&text, 0);
        }
    }
    Ok(StoreFixture { db, dir })
}

/// Removes a scratch directory a fixture is done with, as its teardown.
///
/// Best effort by design: a directory that would not go is a leaked empty scratch tree,
/// which is what the next fixture's serial number exists to tolerate, and a teardown that
/// panicked would turn a failed case into an abort.
///
/// # Panics
///
/// Never panics.
pub(super) fn remove_scratch(dir: PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

/// Creates a scratch directory one fixture owns, named after `tag` and this process.
///
/// # Errors
///
/// An error when the directory cannot be created.
///
/// # Panics
///
/// Never panics.
pub(super) fn scratch_dir(tag: &str) -> anyhow::Result<PathBuf> {
    let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "rspinyin-engine-{tag}-{}-{serial}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating the {tag} scratch directory at {}", dir.display()))?;
    Ok(dir)
}
