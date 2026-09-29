//! Memory and resource sampling: what a running process holds, as numbers a budget can be
//! asserted against.
//!
//! Responsibility: read a process's resident set size and its rolled-up memory-map counters
//! out of `/proc`, follow them over a window, and judge the result against the ceilings
//! `docs/dev/budgets.json` states. This is the input half of the `runtime://memory_profile`
//! resource.
//!
//! Boundaries: it reads `/proc` and nothing else. It starts no process, sends no signal,
//! opens no socket and writes nothing anywhere. Which process a case watches, and when its
//! window opens, are the case's decisions rather than this module's, and a sample is never a
//! verdict on its own: [`MemoryMonitor::judge`] is the one place a ceiling is applied.
//!
//! # Two bases, and why they are never mixed
//!
//! `VmRSS` counts every resident page, the pages of the dictionary's read-only mapping among
//! them. Those pages are a file's page cache: the kernel may drop them and fault them back in,
//! so a process whose resident set grew because a case walked the dictionary has not grown in
//! the sense a leak budget is about. The dictionary budget is therefore about the *anonymous*
//! part of that mapping, which `smaps_rollup` states as `Anonymous` and `Private_Dirty`. The
//! two bases answer different questions, [`Measure`] names them, and [`MemoryBudget`] binds
//! each budget to exactly one -- asserting a rollup budget against a resident set size is the
//! mistake this split exists to prevent.
//!
//! # A budget is a difference, not a number
//!
//! The rendering layer's budget is its *increment*. The process being sampled is Fcitx5, which
//! was resident long before the candidate window existed, so its absolute resident set says
//! nothing about what the renderer costs. A case takes a baseline before the interaction it
//! measures and reads the growth after it; [`MemoryError::NoBaseline`] is how this module
//! refuses to turn a missing baseline into a pass.
//!
//! # A degraded sample says so
//!
//! `smaps_rollup` is not on every kernel, and a hardened `/proc` can withhold it. When the
//! file cannot be read the sample degrades to the resident set alone and carries
//! [`SampleBasis::RssOnly`], with the reason, so that a report shows the degradation and
//! [`MemoryError::BasisDegraded`] stops a budget that reads a missing counter from passing
//! against a zero nobody measured. A file that is *there* but is not shaped the way the
//! kernel writes it is a different thing, and it is an error rather than a degradation.
//!
//! # The ceilings live in one place
//!
//! Every ceiling comes from `docs/dev/budgets.json` through
//! [`MemoryBudgets::from_thresholds`], which takes the document flattened to `(dotted path,
//! value)` pairs -- the shape the workspace's other threshold readers take. No ceiling is
//! written down in this module, and the tests assert that none is.
//!
//! # The calls a case makes
//!
//! Read the document into [`MemoryBudgets`], take a baseline before the window exists, sample
//! after it, and judge: `monitor.mark_baseline()?`, `monitor.sample()?`,
//! `monitor.judge(&ceilings, MemoryBudget::UiRss)?` -- a regression is that last error.
//!
//! # What lives where
//!
//! This root holds what the two halves share: the names `/proc` writes, the parser for one
//! counter line, the budget table and the error type. The reading itself is in `sample`, and
//! the window that follows it and applies a ceiling is in `monitor`. The ceiling audit in the
//! sibling tests scans this file, so everything that states or converts a ceiling stays here.
//!
//! The tests live in a sibling file rather than inside this one: together they would be
//! longer than the file limit allows.

// The channel is exercised by the tests below and by nothing else yet: the case runner that
// would open a monitor, the evidence archiver and the subcommand tree all live in files this
// module does not own. Until that wiring lands, every item here is reported as dead code in a
// non-test build, and the attribute goes away with those lines.
//
// `unused_imports` is covered by the same reasoning and for the same reason: the `pub use`
// lines below are this module's surface, and a `pub use` in a *binary* crate is "unused"
// whenever nothing in the crate names it.
#![allow(dead_code, unused_imports)]

mod monitor;
mod sample;

use std::path::PathBuf;

pub use self::monitor::MemoryMonitor;
pub use self::sample::{MemorySample, SampleBasis};

#[cfg(test)]
mod tests;

/// The directory the kernel exposes the process table under.
const PROC: &str = "/proc";

/// The file in a process directory that carries the resident set size.
const STATUS_FILE: &str = "status";

/// The file in a process directory that carries the rolled-up memory-map counters.
const ROLLUP_FILE: &str = "smaps_rollup";

/// The field in `status` that carries the resident set size.
const RSS_FIELD: &str = "VmRSS:";

/// The field in `smaps_rollup` that carries the anonymous resident part of the mappings.
const ANONYMOUS_FIELD: &str = "Anonymous:";

/// The field in `smaps_rollup` that carries the privately dirtied part of the mappings.
const PRIVATE_DIRTY_FIELD: &str = "Private_Dirty:";

/// The unit every memory counter of these two files is stated in.
const PROC_UNIT: &str = "kB";

/// Kibibytes in one mebibyte.
///
/// The budget document states its memory ceilings in mebibytes and `/proc` states every
/// counter in kibibytes. The two units meet in [`MemoryBudgets::from_thresholds`] and nowhere
/// else.
const KB_PER_MB: f64 = 1024.0;

/// The document key of the rendering layer's resident growth.
const UI_RSS_KEY: &str = "memory_mb.ui_rss";

/// The document key of the plugin's total resident growth.
const PLUGIN_RSS_KEY: &str = "memory_mb.plugin_rss";

/// The document key of the dictionary mapping's resident part.
const DICT_MMAP_KEY: &str = "memory_mb.dict_mmap_rss";

/// The document key of the resident drift a soak run may show.
const SOAK_DRIFT_KEY: &str = "robustness.rss_drift_mb";

/// Which number of a sample a budget is asserted against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Measure {
    /// The resident growth from the baseline sample.
    RssDelta,
    /// The resident growth the window reached, from its first sample to its highest.
    RssDrift,
    /// The anonymous resident part of the mappings, `smaps_rollup`'s `Anonymous`.
    Anonymous,
    /// The privately dirtied part of the mappings, `smaps_rollup`'s `Private_Dirty`.
    PrivateDirty,
}

impl Measure {
    /// The label a report prints.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::RssDelta => "rss-delta",
            Self::RssDrift => "rss-drift",
            Self::Anonymous => "anonymous",
            Self::PrivateDirty => "private-dirty",
        }
    }
}

/// One budget of the memory table: the key that states its ceiling, and the number it governs.
///
/// This list is the module's binding table -- which key owns which measurement, never a value
/// -- so the ceilings exist only in `docs/dev/budgets.json`. Two variants share one key
/// because the dictionary budget is asserted on both of its bases separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryBudget {
    /// The rendering layer's resident growth, `memory_mb.ui_rss`.
    UiRss,
    /// The plugin's total resident growth, dictionary page cache included,
    /// `memory_mb.plugin_rss`.
    PluginRss,
    /// The dictionary mapping's anonymous resident part, `memory_mb.dict_mmap_rss`.
    DictMmapAnonymous,
    /// The dictionary mapping's privately dirtied part, `memory_mb.dict_mmap_rss`.
    DictMmapPrivateDirty,
    /// The resident drift a soak run may show, `robustness.rss_drift_mb`.
    SoakDrift,
}

impl MemoryBudget {
    /// Every budget this module binds, in the order a report lists them.
    pub const ALL: [Self; 5] = [
        Self::UiRss,
        Self::PluginRss,
        Self::DictMmapAnonymous,
        Self::DictMmapPrivateDirty,
        Self::SoakDrift,
    ];

    /// The dotted path of this budget's ceiling in the budget document.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn key(self) -> &'static str {
        match self {
            Self::UiRss => UI_RSS_KEY,
            Self::PluginRss => PLUGIN_RSS_KEY,
            Self::DictMmapAnonymous | Self::DictMmapPrivateDirty => DICT_MMAP_KEY,
            Self::SoakDrift => SOAK_DRIFT_KEY,
        }
    }

    /// Which number of a sample this budget governs.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn measure(self) -> Measure {
        match self {
            Self::UiRss | Self::PluginRss => Measure::RssDelta,
            Self::DictMmapAnonymous => Measure::Anonymous,
            Self::DictMmapPrivateDirty => Measure::PrivateDirty,
            Self::SoakDrift => Measure::RssDrift,
        }
    }
}

/// The memory ceilings the budget document states, in kibibytes.
///
/// A field is `None` when the document states no usable ceiling for that key, and such a budget
/// is refused rather than judged against a default: [`Self::default`] budgets nothing, the
/// honest reading of "nobody stated a number".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryBudgets {
    /// `memory_mb.ui_rss`: the growth the rendering layer may add.
    pub ui_rss_kb: Option<u64>,
    /// `memory_mb.plugin_rss`: the growth the plugin may add.
    pub plugin_rss_kb: Option<u64>,
    /// `memory_mb.dict_mmap_rss`: the resident part of the dictionary mapping.
    pub dict_mmap_kb: Option<u64>,
    /// `robustness.rss_drift_mb`: the drift a soak run may show.
    pub soak_drift_kb: Option<u64>,
}

impl MemoryBudgets {
    /// The ceilings `thresholds` states, in kibibytes.
    ///
    /// `thresholds` is the budget document flattened to `(dotted path, value)` pairs, which is
    /// what `Budgets::thresholds` produces. The values arrive in mebibytes and are converted
    /// once, here; a key the document does not carry, or carries as a number that cannot be a
    /// ceiling, stays `None` rather than becoming a default.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_thresholds(thresholds: &[(&'static str, f64)]) -> Self {
        Self {
            ui_rss_kb: threshold_kb(thresholds, UI_RSS_KEY),
            plugin_rss_kb: threshold_kb(thresholds, PLUGIN_RSS_KEY),
            dict_mmap_kb: threshold_kb(thresholds, DICT_MMAP_KEY),
            soak_drift_kb: threshold_kb(thresholds, SOAK_DRIFT_KEY),
        }
    }

    /// The ceiling `budget` is judged against, when the document states a usable one.
    fn limit_kb(&self, budget: MemoryBudget) -> Option<u64> {
        match budget {
            MemoryBudget::UiRss => self.ui_rss_kb,
            MemoryBudget::PluginRss => self.plugin_rss_kb,
            MemoryBudget::DictMmapAnonymous | MemoryBudget::DictMmapPrivateDirty => {
                self.dict_mmap_kb
            }
            MemoryBudget::SoakDrift => self.soak_drift_kb,
        }
    }
}

/// What one budget's measurement came to.
///
/// A verdict exists only for a measurement inside its ceiling: [`MemoryMonitor::judge`] refuses
/// the run when one is past it, so a caller that holds a verdict holds a pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetVerdict {
    /// The budget that was judged.
    pub budget: MemoryBudget,
    /// What the window measured, in kibibytes.
    pub measured_kb: u64,
    /// The ceiling the document states, in kibibytes.
    pub limit_kb: u64,
}

impl BudgetVerdict {
    /// The line a report prints for this verdict.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn describe(self) -> String {
        format!(
            "{} ({}): {}KiB of {}KiB",
            self.budget.key(),
            self.budget.measure().label(),
            self.measured_kb,
            self.limit_kb
        )
    }
}

/// Everything the memory channel can refuse to do.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    /// A file of a process's `/proc` directory could not be read.
    ///
    /// The wrapped [`std::io::Error`] is what carries the errno: its message names the operating
    /// system's own reason, which is what tells a caller whether the process exited or whether
    /// this one may not look at it.
    #[error("{path}: {source}")]
    ProcUnreadable {
        /// The file the read was about.
        path: PathBuf,
        /// The failure the read reported.
        #[source]
        source: std::io::Error,
    },

    /// A file of a process's `/proc` directory was read, and does not state a counter.
    ///
    /// This is a malformed reading rather than a degraded one: a `smaps_rollup` that is not there
    /// is a kernel that does not have it, while a `status` without `VmRSS` is a file nobody can
    /// interpret.
    #[error("{path}: no usable `{field}` field")]
    Malformed {
        /// The file that was read.
        path: PathBuf,
        /// The field that is missing, or that does not state a number of kibibytes.
        field: &'static str,
    },

    /// The budget document states no usable ceiling for a budget.
    #[error("`{key}` has no usable threshold in the budget document")]
    Unbudgeted {
        /// The dotted path of the ceiling that is missing.
        key: &'static str,
    },

    /// A budget that reads a `smaps_rollup` counter was judged against a sample without one.
    #[error(
        "process {pid}: `{key}` reads `{measure}`, which this sample does not carry because \
         `smaps_rollup` could not be read; the budget is unmeasured, not satisfied"
    )]
    BasisDegraded {
        /// The process that was sampled.
        pid: u32,
        /// The dotted path of the budget.
        key: &'static str,
        /// The counter the budget reads.
        measure: &'static str,
    },

    /// No sample has been taken yet.
    #[error("process {pid}: no sample has been taken")]
    NoSample {
        /// The process that was to be sampled.
        pid: u32,
    },

    /// A budget measured from a baseline was judged without one.
    #[error("process {pid}: `{key}` is a growth from a baseline and no baseline was taken")]
    NoBaseline {
        /// The process that was to be sampled.
        pid: u32,
        /// The dotted path of the budget.
        key: &'static str,
    },

    /// A budget measured across the window was judged against fewer than two samples.
    #[error("process {pid}: `{key}` is a drift and the window holds fewer than two samples")]
    TooFewSamples {
        /// The process that was to be sampled.
        pid: u32,
        /// The dotted path of the budget.
        key: &'static str,
    },

    /// A measurement is past the ceiling the document states for it.
    #[error("{key} ({measure}): {measured_kb}KiB is past the {limit_kb}KiB threshold")]
    OverBudget {
        /// The dotted path of the budget.
        key: &'static str,
        /// The counter the budget reads.
        measure: &'static str,
        /// What the window measured, in kibibytes.
        measured_kb: u64,
        /// The ceiling the document states, in kibibytes.
        limit_kb: u64,
    },
}

/// The number of a `/proc` counter line, when the line is there and states one.
///
/// Every memory counter of these files is a whole number of kibibytes and the kernel writes
/// exactly one number and one unit on the line. A line that says anything else is not the
/// counter this reader asked for, and reading its number as kibibytes would be a silent unit
/// error -- which a budget assertion must not have.
fn counter_kb(text: &str, field: &str) -> Option<u64> {
    let line = text.lines().find(|line| line.starts_with(field))?;
    let mut rest = line[field.len()..].split_whitespace();
    let value = rest.next()?.parse::<u64>().ok()?;
    let unit = rest.next()?;
    if unit != PROC_UNIT || rest.next().is_some() {
        return None;
    }
    Some(value)
}

/// The ceiling `key` states in `thresholds`, in kibibytes.
fn threshold_kb(thresholds: &[(&'static str, f64)], key: &str) -> Option<u64> {
    thresholds
        .iter()
        .find(|(name, _)| *name == key)
        .and_then(|(_, value)| mb_to_kb(*value))
}

/// A ceiling stated in mebibytes as kibibytes, when it is a number one can be inside.
///
/// `None` for a value that is not a finite, non-negative number: such a ceiling is refused by
/// [`MemoryError::Unbudgeted`] rather than read as a pass.
fn mb_to_kb(mebibytes: f64) -> Option<u64> {
    if !mebibytes.is_finite() || mebibytes < 0.0 {
        return None;
    }
    // `f64 as u64` saturates rather than wrapping, so a ceiling beyond what a `u64` holds
    // becomes the largest one instead of a small number every sample would be inside.
    Some((mebibytes * KB_PER_MB).round() as u64)
}
