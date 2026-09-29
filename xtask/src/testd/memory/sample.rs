//! One reading of a process's counters, and the `/proc` files it is read out of.
//!
//! Responsibility: turn a process directory into one [`MemorySample`] -- the resident set
//! `status` states, the anonymous and privately dirtied parts of the mappings `smaps_rollup`
//! states -- and refuse a file that is there but is not shaped the way the kernel writes it.
//!
//! Boundaries: it reads `/proc` and decides nothing. A sample is never a verdict, and a
//! `smaps_rollup` that cannot be read degrades the sample rather than failing the case. The
//! names of the files and the fields, the parser for one counter line, the budget table and the
//! error type stay in the module root; the window that follows the readings is `monitor`.
//!
//! # A missing rollup degrades, a malformed one is an error
//!
//! `smaps_rollup` is not on every kernel and a hardened `/proc` can withhold it, so a read that
//! fails leaves the rollup counters zero and marks the sample [`SampleBasis::RssOnly`] with the
//! reason it failed. A file that is there and does not state its counters is a different thing:
//! nobody can interpret it, and it is refused as [`MemoryError::Malformed`] rather than read as
//! a process that holds nothing.

// The channel is exercised by the tests beside the module root and by nothing else yet: the
// case runner that would open a monitor, the evidence archiver and the subcommand tree all live
// in files this module does not own, and the root carries the full note. Until that wiring
// lands, every item here is reported as dead code in a non-test build, and the attribute goes
// away together with the root's.
#![allow(dead_code)]

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::MemoryError;
use super::counter_kb;

use super::{ANONYMOUS_FIELD, PRIVATE_DIRTY_FIELD, ROLLUP_FILE, RSS_FIELD, STATUS_FILE};

/// Which counters of a sample are measurements.
///
/// This distinction is the module's central discipline: a resident set size is not an answer
/// to a question about anonymous memory, so a sample that could not read the rollup says so
/// instead of letting its zeroes be read as measurements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleBasis {
    /// `/proc/<pid>/smaps_rollup` was read, so `anonymous_kb` and `private_dirty_kb` are
    /// measurements.
    Rollup,
    /// `smaps_rollup` could not be read, for the reason carried here.
    ///
    /// The rollup counters of such a sample are zero and are *not* measurements: a budget that
    /// reads one is refused with [`MemoryError::BasisDegraded`] rather than passed. The reason
    /// is an [`ErrorKind`] rather than the [`std::io::Error`] because a sample is copied into a
    /// window of hundreds of them, and because the kind is what a reader acts on: `NotFound` is
    /// a kernel without the file, `PermissionDenied` a `/proc` that withholds it.
    RssOnly(ErrorKind),
}

impl SampleBasis {
    /// The label a report prints.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::Rollup => "smaps-rollup",
            Self::RssOnly(_) => "rss-only",
        }
    }

    /// Whether the sample carries the `smaps_rollup` counters.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn has_rollup(self) -> bool {
        matches!(self, Self::Rollup)
    }
}

/// One reading of a process's memory counters, cheap enough to copy by the hundred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemorySample {
    /// Resident set size, `VmRSS`, in kibibytes.
    pub vm_rss_kb: u64,
    /// The anonymous resident part of the mappings, in kibibytes; zero when [`Self::basis`]
    /// is not [`SampleBasis::Rollup`].
    pub anonymous_kb: u64,
    /// The privately dirtied part of the mappings, in kibibytes; zero when [`Self::basis`] is
    /// not [`SampleBasis::Rollup`].
    pub private_dirty_kb: u64,
    /// Which of the counters above are measurements.
    pub basis: SampleBasis,
    /// When the reading was taken.
    pub at: Instant,
}

/// Reads one sample of `pid` out of the `proc_dir` that exposes it, or the refusal that says why.
///
/// # Errors
///
/// Returns [`MemoryError::ProcUnreadable`] when the process's `status` cannot be read -- it has
/// exited, or this process may not look at it -- and [`MemoryError::Malformed`] when a file that
/// was read does not state the counter it must.
///
/// # Panics
///
/// Never.
pub(super) fn read_sample(proc_dir: &Path, pid: u32) -> Result<MemorySample, MemoryError> {
    let dir = proc_dir.join(pid.to_string());
    let status_path = dir.join(STATUS_FILE);
    let status = read_file(&status_path)?;
    let vm_rss_kb = required_counter(&status, RSS_FIELD, &status_path)?;
    let rollup_path = dir.join(ROLLUP_FILE);
    let (basis, anonymous_kb, private_dirty_kb) = match fs::read_to_string(&rollup_path) {
        Ok(text) => {
            let anonymous_kb = required_counter(&text, ANONYMOUS_FIELD, &rollup_path)?;
            let private_dirty_kb = required_counter(&text, PRIVATE_DIRTY_FIELD, &rollup_path)?;
            (SampleBasis::Rollup, anonymous_kb, private_dirty_kb)
        }
        // The file is not on every kernel and a hardened `/proc` can withhold it. The sample
        // degrades to the resident set and records why, rather than failing the case: the
        // counters it would have carried are marked absent, and a budget that reads one
        // refuses to be judged against such a sample.
        Err(error) => (SampleBasis::RssOnly(error.kind()), 0, 0),
    };
    Ok(MemorySample {
        vm_rss_kb,
        anonymous_kb,
        private_dirty_kb,
        basis,
        at: Instant::now(),
    })
}

/// Reads a `/proc` file, naming the path when it fails.
fn read_file(path: &Path) -> Result<String, MemoryError> {
    fs::read_to_string(path).map_err(|source| MemoryError::ProcUnreadable {
        path: path.to_path_buf(),
        source,
    })
}

/// The counter `field` states in `text`, or the refusal that names the file that failed to state it.
fn required_counter(text: &str, field: &'static str, path: &Path) -> Result<u64, MemoryError> {
    match counter_kb(text, field) {
        Some(value) => Ok(value),
        None => Err(MemoryError::Malformed {
            path: path.to_path_buf(),
            field,
        }),
    }
}
