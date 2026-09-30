//! The process's diagnostic probes: the numbers behind the performance budgets.
//!
//! Responsibility: own the one set of probes this process records into, mark the moments
//! the memory budgets are measured from, and write the snapshot file `xtask report` and
//! `xtask budget --memory` read.
//!
//! Boundaries: `ime-diag` owns the probes, the histograms and the file format. This module
//! decides only when the process's set is created and which moments are marked, which are
//! facts about this plugin's lifecycle rather than about the measurement.
//!
//! # One set per process
//!
//! A `OnceLock` over the probes, created on the first call. It is created by the
//! `diagnostics` step, before anything is loaded, because [`Probes::new`] takes the
//! process's memory baseline and the dictionary's resident pages have to fall *inside* the
//! plugin's growth rather than below it. A second set would be a second baseline for one
//! process, which is the one thing the memory budgets cannot be stated against.
//!
//! # The two baselines
//!
//! [`mark_dictionary_baseline`] is marked once the mapping is in place, which is what
//! `BUDGET-MEM-03` measures the dictionary's private growth from. [`mark_ui_baseline`] is
//! the user-interface layer's own moment, marked by whatever brings that layer up; a
//! snapshot taken without it reports the UI budget as unmeasured rather than judging it
//! against the whole process.
//!
//! # Writing the snapshot
//!
//! [`write_snapshot`] is the one write path, and it goes through [`Probes::write_snapshot`]
//! so that the file carries the process's memory section as well as the latencies: a
//! snapshot written from `ProbeSnapshot::write_to` alone would leave the three memory
//! budgets unmeasured, which the budget gate reports as a failure rather than as a pass.
//!
//! The caller is the signal path, and it is not this module: a `SIGUSR1` handler may not do
//! file IO, so the handler records that a snapshot was asked for and the write happens off
//! the handler. What this module provides is the write itself.

use std::io;
use std::sync::OnceLock;

use ime_diag::probe::Probes;

use super::layout;

/// The one set of probes this process records into.
static PROBES: OnceLock<Probes> = OnceLock::new();

/// Creates the probes unless something already has.
///
/// Called by the `diagnostics` step, so that the process's memory baseline is taken before
/// the dictionary is mapped.
///
/// # Panics
///
/// Never.
pub(super) fn create() {
    let _ = probes();
}

/// The probes every measurement goes into, created on the first call.
///
/// # Panics
///
/// Never: the initialiser is a plain construction with no fallible step.
fn probes() -> &'static Probes {
    PROBES.get_or_init(Probes::new)
}

/// Marks the moment the dictionary had been mapped.
///
/// Called by the `lexicon` step once the mapping is in place and before anything reads it,
/// so that the growth covers the pages the mapping faults in.
///
/// # Panics
///
/// Never.
pub(super) fn mark_dictionary_baseline() {
    probes().mark_dictionary_baseline();
}

/// Marks the moment the user-interface layer started.
///
/// The entry point that layer calls when it comes up, which is what `BUDGET-MEM-01` is
/// measured from. A run in which it is never called reports the UI budget as unmeasured
/// rather than as zero.
///
/// # Panics
///
/// Never.
pub fn mark_ui_baseline() {
    probes().mark_ui_baseline();
}

/// Switches the probes on or off, as the `[diagnostics] probes` key asks.
///
/// Switched off, the histograms stop recording and the counters stop counting, so nothing
/// half-measured reaches a report. The switch is applied by the `config` step, which is
/// where the document carrying it is read.
///
/// # Panics
///
/// Never.
pub(super) fn set_enabled(enabled: bool) {
    probes().set_enabled(enabled);
}

/// Whether the probes are switched on.
///
/// # Panics
///
/// Never.
#[cfg(test)]
pub(super) fn is_enabled() -> bool {
    probes().is_enabled()
}

/// Writes the probe snapshot to the file `xtask report` and `xtask budget --memory` read.
///
/// One file carries the latencies and the process's memory, which is why this goes through
/// [`Probes::write_snapshot`] rather than writing a [`ProbeSnapshot`] of its own: the
/// memory budgets are stated for the growths that section records.
///
/// [`ProbeSnapshot`]: ime_diag::probe::ProbeSnapshot
///
/// # Returns
///
/// `Ok(())` once the file has been written, created `0600` inside the data directory.
///
/// # Errors
///
/// Returns an [`io::Error`] when the data directory is not known — the `data-dirs` step
/// runs before any caller can reach this, so an error here is a wiring defect rather than a
/// user-visible condition — and when the file cannot be created, its mode cannot be set or
/// the text cannot be written.
///
/// # Panics
///
/// Never.
pub fn write_snapshot() -> io::Result<()> {
    let Some(path) = layout::snapshot_path() else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the data directory is not known, so there is nowhere to write a snapshot",
        ));
    };
    probes().write_snapshot(&path)
}
