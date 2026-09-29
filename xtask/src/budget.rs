//! Performance-budget validation for `docs/dev/budgets.json`.
//!
//! The architecture spec owns the authoritative performance and resource
//! thresholds; `docs/dev/budgets.json` is their machine-readable mirror, read by
//! benchmarks and probes. Two copies of the same numbers drift apart silently,
//! so this module re-reads both and fails on any disagreement.
//!
//! The comparison is bidirectional: every threshold in the JSON must be bound to
//! a spec row and carry exactly the value that row states, every binding must be
//! backed by a threshold, and a missing section, missing row, unknown key, or
//! unreadable cell is an error rather than a silent pass.
//!
//! Comparing criterion output against these thresholds is a separate action.
//!
//! The module is split by responsibility: this root holds the document types, the
//! binding table and the entry points, [`schema`] turns the document's text into
//! those types, and [`spec`] reads the spec's table and compares the two.

mod schema;
mod spec;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

use self::SpecCell::{MetricAfter, ThresholdAfter, ThresholdFirst, ThresholdZero};
pub(crate) use self::spec::{compare, parse_spec_table};

#[cfg(test)]
mod tests;

/// Path of the machine-readable budget file, relative to the repository root.
pub const BUDGETS_FILE: &str = "docs/dev/budgets.json";

/// Path of the document that owns the authoritative thresholds.
pub const SPEC_FILE: &str = "docs/dev/features.md";

/// Section of [`SPEC_FILE`] that holds the budget table.
pub const SPEC_SECTION: &str = "0.5.3";

/// Latency thresholds in milliseconds.
#[derive(Debug, Clone, PartialEq)]
pub struct LatencyMs {
    /// Key press to candidate frame presented, median.
    pub key_to_present_p50: f64,
    /// Key press to candidate frame presented, 99th percentile.
    pub key_to_present_p99: f64,
    /// As above on a 144 Hz display.
    pub key_to_present_p99_144hz: f64,
    /// One decode call, 99th percentile.
    pub decode_p99: f64,
    /// One decode call, 99.9th percentile.
    pub decode_p999: f64,
    /// One software-rasterized frame, 99th percentile.
    pub raster_p99: f64,
    /// First key press to a visible window, 99th percentile.
    pub first_key_to_visible_p99: f64,
    /// Addon load during Fcitx5 startup.
    pub addon_load: f64,
}

/// Memory thresholds in mebibytes of resident set size.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryMb {
    /// Resident growth of the UI rendering layer.
    pub ui_rss: f64,
    /// Total resident growth of the plugin, including dictionary page cache.
    pub plugin_rss: f64,
    /// Anonymous resident part of the dictionary mapping.
    pub dict_mmap_rss: f64,
}

/// CPU thresholds as a percentage of one core, plus idle invariants.
#[derive(Debug, Clone, PartialEq)]
pub struct CpuPct {
    /// Idle usage with no input and the candidate window hidden.
    pub idle: f64,
    /// Usage while typing ten characters per second.
    pub typing_10cps: f64,
    /// Redraws while idle; the spec requires zero.
    pub idle_redraw_count: u64,
    /// Polling timers while idle; the spec requires zero.
    pub idle_poll_timer_count: u64,
}

/// Artifact size thresholds in mebibytes.
#[derive(Debug, Clone, PartialEq)]
pub struct SizeMb {
    /// Release, stripped `librspinyin.so`.
    pub so_stripped: f64,
    /// Built-in base dictionary.
    pub base_dict: f64,
}

/// Long-run robustness thresholds.
#[derive(Debug, Clone, PartialEq)]
pub struct Robustness {
    /// Continuous typing with no crash and no memory growth.
    pub soak_hours: f64,
    /// Allowed RSS drift over the soak run.
    pub rss_drift_mb: f64,
    /// Required pass rate of the soak run.
    pub pass_rate_pct: f64,
}

/// The parsed contents of `docs/dev/budgets.json`.
///
/// Fields are public so callers can build a modified copy and re-run [`compare`]
/// without touching the file on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct Budgets {
    /// Schema version of the budget document.
    pub version: u64,
    /// Latency thresholds.
    pub latency_ms: LatencyMs,
    /// Memory thresholds.
    pub memory_mb: MemoryMb,
    /// CPU thresholds.
    pub cpu_pct: CpuPct,
    /// Artifact size thresholds.
    pub size_mb: SizeMb,
    /// Robustness thresholds.
    pub robustness: Robustness,
    /// Process-external sockets allowed at runtime; the spec requires zero.
    pub net_sockets: u64,
}

/// One numeric threshold: its dotted path in the budget document, and its value.
#[derive(Debug, Clone, PartialEq)]
pub struct Threshold(pub &'static str, pub f64);

/// One row of the spec's budget table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecRow {
    /// Budget identifier as written in the table, e.g. `BUDGET-LAT-02`.
    pub budget_id: String,
    /// Metric column of the row.
    pub metric: String,
    /// Threshold column of the row.
    pub threshold: String,
}

/// Outcome of a successful validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Schema version of the budget document that was validated.
    pub version: u64,
    /// Number of thresholds that were compared against the spec table.
    pub checked: usize,
}

/// Where in a spec row a bound threshold is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpecCell {
    /// Threshold column, first number in the cell.
    ThresholdFirst,
    /// Threshold column, first number at or after the needle.
    ThresholdAfter(&'static str),
    /// Metric column, first number at or after the needle.
    MetricAfter(&'static str),
    /// Threshold column must mention the needle, and the budget must be zero.
    ThresholdZero(&'static str),
}

/// Binds one value of `budgets.json` to the spec row that owns it.
///
/// Fields, in order: the dotted path of the value in the budget document, the
/// spec row that states it, and where in that row the value is written.
struct Binding(&'static str, &'static str, SpecCell);

/// The correspondence between the budget document and the spec table.
///
/// This table is the only place where a number is not written down twice: it
/// records *which* spec row owns each JSON value, never the value itself.
const BINDINGS: &[Binding] = &[
    Binding(
        "latency_ms.key_to_present_p50",
        "BUDGET-LAT-01",
        ThresholdAfter("P50"),
    ),
    Binding(
        "latency_ms.key_to_present_p99",
        "BUDGET-LAT-01",
        ThresholdAfter("P99"),
    ),
    Binding(
        "latency_ms.key_to_present_p99_144hz",
        "BUDGET-LAT-01",
        ThresholdAfter("144Hz"),
    ),
    Binding(
        "latency_ms.decode_p99",
        "BUDGET-LAT-02",
        ThresholdAfter("P99"),
    ),
    Binding(
        "latency_ms.decode_p999",
        "BUDGET-LAT-02",
        ThresholdAfter("P999"),
    ),
    Binding(
        "latency_ms.raster_p99",
        "BUDGET-LAT-03",
        ThresholdAfter("P99"),
    ),
    Binding(
        "latency_ms.first_key_to_visible_p99",
        "BUDGET-LAT-04",
        ThresholdAfter("P99"),
    ),
    Binding("latency_ms.addon_load", "BUDGET-LAT-05", ThresholdFirst),
    Binding("memory_mb.ui_rss", "BUDGET-MEM-01", ThresholdFirst),
    Binding("memory_mb.plugin_rss", "BUDGET-MEM-02", ThresholdFirst),
    Binding("memory_mb.dict_mmap_rss", "BUDGET-MEM-03", ThresholdFirst),
    Binding("cpu_pct.idle", "BUDGET-CPU-01", ThresholdFirst),
    Binding("cpu_pct.typing_10cps", "BUDGET-CPU-02", ThresholdFirst),
    Binding(
        "cpu_pct.idle_redraw_count",
        "BUDGET-CPU-01",
        ThresholdAfter("重绘次数"),
    ),
    Binding(
        "cpu_pct.idle_poll_timer_count",
        "BUDGET-CPU-01",
        ThresholdZero("无轮询定时器"),
    ),
    Binding("size_mb.so_stripped", "BUDGET-SIZE-01", ThresholdFirst),
    Binding("size_mb.base_dict", "BUDGET-SIZE-02", ThresholdFirst),
    Binding(
        "robustness.soak_hours",
        "BUDGET-ROB-01",
        MetricAfter("连续"),
    ),
    Binding(
        "robustness.rss_drift_mb",
        "BUDGET-ROB-01",
        MetricAfter("RSS"),
    ),
    Binding("robustness.pass_rate_pct", "BUDGET-ROB-01", ThresholdFirst),
    Binding("net_sockets", "BUDGET-NET-01", ThresholdFirst),
];

/// Validate the repository's budget document against the spec table.
///
/// `root` is the repository root; both files are resolved relative to it.
///
/// # Errors
/// Returns an error when either file cannot be read or parsed, or when any
/// threshold disagrees with the spec table.
pub fn validate(root: &Path) -> Result<Report> {
    let spec_path = root.join(SPEC_FILE);
    let budgets_path = root.join(BUDGETS_FILE);
    let spec = fs::read_to_string(&spec_path)
        .with_context(|| format!("cannot read {}", spec_path.display()))?;
    let text = fs::read_to_string(&budgets_path)
        .with_context(|| format!("cannot read {}", budgets_path.display()))?;
    let table = parse_spec_table(&spec)
        .with_context(|| format!("{}: unusable budget table", spec_path.display()))?;
    let budgets = Budgets::from_json(&text)
        .with_context(|| format!("{}: invalid budget document", budgets_path.display()))?;
    compare(&budgets, &table)
}

/// Entry point for `xtask budget`.
///
/// # Errors
/// Returns an error when no action was selected, or when the validation fails.
pub fn run(validate_requested: bool) -> Result<()> {
    ensure!(
        validate_requested,
        "xtask budget: no action selected; pass --validate to compare {BUDGETS_FILE} \
         with {SPEC_FILE} section {SPEC_SECTION}"
    );
    let report = validate(&repo_root()?)?;
    println!(
        "budget: schema v{} - {} thresholds match {SPEC_FILE} section {SPEC_SECTION} ({BUDGETS_FILE} verified)",
        report.version, report.checked
    );
    Ok(())
}

/// Repository root, derived from the compile-time location of this crate.
fn repo_root() -> Result<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .context("xtask is expected to live in a subdirectory of the repository root")
}
