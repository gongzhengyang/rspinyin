//! Performance-budget validation for `docs/dev/budgets.json`.
//!
//! The architecture spec owns the authoritative performance and resource
//! thresholds; `docs/dev/budgets.json` is their machine-readable mirror, read by
//! benchmarks and probes. Two copies of the same numbers drift apart silently,
//! so this module re-reads both and fails on any disagreement.
//!
//! The comparison is bidirectional: every threshold in the JSON must be bound to
//! a spec cell and carry exactly the value that cell states, every binding must be
//! backed by a threshold, and a missing section, missing cell, unknown key, or
//! unreadable number is an error rather than a silent pass.
//!
//! Comparing criterion output against these thresholds is [`bench`]'s job: it
//! reads the estimates a benchmark run left behind and asserts the cases the design
//! names, using the same thresholds this module validates.
//!
//! Measuring the release artifacts against the two size thresholds is [`measure`]'s:
//! it reads the files `xtask package` wrote and fails on any that is past its ceiling,
//! so an artifact that grew is a gate failure rather than something a reviewer has to
//! notice.
//!
//! Judging a running plugin's memory readings against the three memory thresholds is
//! [`memory`]'s: it reads the snapshot the plugin wrote, which carries the process's
//! resident memory beside the latencies, and fails on a budget that is past its ceiling
//! or that nothing measured.
//!
//! Judging a decode run's allocation counts against the allocation budget is
//! [`alloc`]'s: it reads the report the decode test writes, which carries the counts a
//! steady-state decode cost, and fails on a budget that is past its ceiling or that
//! nothing measured.
//!
//! The module is split by responsibility: this root holds the document types, the
//! binding table and the entry points, [`schema`] turns the document's text into
//! those types, [`spec`] reads the spec's cells and compares the two, [`bench`]
//! turns a criterion run into a verdict, and [`memory`] turns a plugin's readings into
//! one.

mod bench;
mod memory;
mod meta;
mod schema;
mod spec;

// `pub(crate)` so that `xtask/src/main.rs` can dispatch `--alloc` to it, which is the
// one line this module's wiring is still waiting for.
pub(crate) mod alloc;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

use self::SpecCell::{MetricAfter, ThresholdAfter, ThresholdFirst, ThresholdZero};
pub(crate) use self::bench::{SIGMAS, owner};
pub(crate) use self::memory::run as run_memory;
pub(crate) use self::spec::{Spec, compare};

#[cfg(test)]
mod alloc_tests;

#[cfg(test)]
mod memory_tests;

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

/// Thresholds one benchmark case is asserted against.
///
/// These are the per-case numbers a criterion run is compared with, and they are
/// not rows of the section 0.5.3 table: each one is stated by the acceptance
/// criterion of the card that owns the case. Every field name carries its unit, the
/// same way the `latency_ms` section does, and the binding table says which
/// criterion owns which field.
#[derive(Debug, Clone, PartialEq)]
pub struct Bench {
    /// One `passthrough/classify` call, in nanoseconds.
    pub passthrough_classify_ns: f64,
    /// One `push_char` and one `backspace`, in microseconds.
    pub buffer_ops_us: f64,
    /// One full pass over the held-out evaluation set, in seconds.
    pub decode_holdout_s: f64,
    /// One wakeup of the UI thread, from the host's post to the surface seeing it, in
    /// microseconds.
    pub ui_wakeup_latency_us: f64,
    /// One `UiCommand` across the addon transport (wire assembly plus the sink call),
    /// in nanoseconds.
    pub post_ui_ns: f64,
}

/// Allocation thresholds, in calls the allocator receives.
#[derive(Debug, Clone, PartialEq)]
pub struct AllocCount {
    /// Allocations one decode costs in a workspace that has already decoded once.
    pub decode_steady: u64,
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
    /// Per-case benchmark thresholds.
    pub bench: Bench,
    /// Allocation thresholds.
    pub alloc_count: AllocCount,
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

/// Where in a spec cell a bound threshold is written.
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

/// Binds one value of `budgets.json` to the spec cell that owns it.
///
/// Fields, in order: the dotted path of the value in the budget document, the cell
/// that states it, and where in that cell the number is written.
///
/// The cell is named either by a `BUDGET-*` id, which resolves to a row of the
/// section 0.5.3 table, or by `TASK-*#n`, which resolves to the `n`th acceptance
/// criterion of that task card -- the form the per-case thresholds are stated in.
struct Binding(&'static str, &'static str, SpecCell);

/// The correspondence between the budget document and the spec table.
///
/// This table is the only place where a number is not written down twice: it
/// records *which* spec cell owns each JSON value, never the value itself.
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
    Binding(
        "bench.post_ui_ns",
        "BUDGET-LAT-06",
        ThresholdFirst),
    Binding(
        "bench.passthrough_classify_ns",
        "TASK-1.02.06#2",
        ThresholdAfter("≤"),
    ),
    Binding(
        "bench.input_buffer_ops_us",
        "TASK-1.02.02#4",
        ThresholdAfter("≤"),
    ),
    Binding(
        "bench.decode_holdout_s",
        "TASK-1.02.03#7",
        ThresholdAfter("耗时"),
    ),
    Binding(
        "bench.ui_wakeup_latency_us",
        "TASK-1.05.02#1",
        ThresholdAfter("≤"),
    ),
    Binding(
        "alloc_count.decode_steady",
        "BUDGET-ALLOC-01",
        ThresholdFirst,
    ),
    Binding("net_sockets", "BUDGET-NET-01", ThresholdFirst),
];

/// Validate the repository's budget document against the spec.
///
/// `root` is the repository root; both files are resolved relative to it.
///
/// # Errors
/// Returns an error when either file cannot be read or parsed, or when any
/// threshold disagrees with the spec cell that owns it.
pub fn validate(root: &Path) -> Result<Report> {
    let budgets = read_budgets(root)?;
    let spec_path = root.join(SPEC_FILE);
    let text = fs::read_to_string(&spec_path)
        .with_context(|| format!("cannot read {}", spec_path.display()))?;
    let spec = Spec::parse(&text)
        .with_context(|| format!("{}: unusable budget table", spec_path.display()))?;
    compare(&budgets, &spec)
}

/// Reads and schema-checks the repository's budget document.
///
/// # Errors
/// Returns an error when the file cannot be read or does not satisfy the schema.
pub(crate) fn read_budgets(root: &Path) -> Result<Budgets> {
    let path = root.join(BUDGETS_FILE);
    let text =
        fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    Budgets::from_json(&text)
        .with_context(|| format!("{}: invalid budget document", path.display()))
}

/// What the command line asked the budget gate to do.
///
/// The three modes are independent and may be combined: `--validate` cross-checks the
/// document against the specification, `--check` asserts the criterion measurements, and
/// `--measure` asserts the release artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Actions<'a> {
    /// Compare every threshold with the authoritative table in the spec.
    pub validate: bool,
    /// Compare the criterion measurements under `target/criterion` against the
    /// thresholds.
    pub check: bool,
    /// Compare the release artifacts under `dist` against the size thresholds.
    pub measure: bool,
    /// Narrow `--check` to one criterion group, e.g. `decode`.
    pub bench_group: Option<&'a str>,
    /// Directory the release artifacts `--measure` reads.
    pub dist: &'a Path,
}

/// One release artifact the size gate measures, and the threshold it is measured against.
///
/// Fields, in order: the file's name in the release directory, and the dotted path of the
/// threshold in the budget document. The name is a key into the release layout and never a
/// number, and the threshold is read from the document, so neither is written down twice.
/// A test asserts the names are the ones the packager writes.
struct MeasuredArtifact(&'static str, &'static str);

/// The release artifacts `--measure` reads, in the order they are measured.
///
/// The two libraries and the dictionary are the three files whose size the specification
/// budgets: everything else a release ships is text or artwork whose size nobody is
/// claiming anything about.
const MEASURED_ARTIFACTS: &[MeasuredArtifact] = &[
    MeasuredArtifact("librspinyin.so", "size_mb.so_stripped"),
    MeasuredArtifact("librspinyin_ui.so", "size_mb.so_stripped"),
    MeasuredArtifact("base.dict", "size_mb.base_dict"),
];

/// Outcome of a size measurement run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SizeReport {
    /// Files that were measured and are inside their threshold, one message each.
    pub passed: Vec<String>,
    /// Files that are past their threshold, one message each.
    pub violations: Vec<String>,
}

/// Entry point for `xtask budget`.
///
/// `--validate` compares the document against the spec table, `--check` compares the
/// criterion measurements under `target/criterion` against the thresholds, optionally
/// narrowed to one group by `--bench`, and `--measure` measures the release artifacts
/// under `--dist`. The three are independent and may be combined.
///
/// # Errors
///
/// Returns an error when no action was selected, when the validation fails, when the
/// benchmark gate finds a missing or exceeded measurement, and when a release artifact is
/// missing or past its size threshold.
pub fn run(actions: Actions<'_>) -> Result<()> {
    ensure!(
        actions.validate || actions.check || actions.measure,
        "xtask budget: no action selected; pass --validate to compare {BUDGETS_FILE} \
         with {SPEC_FILE} section {SPEC_SECTION}, --check to compare the criterion \
         measurements under target/criterion against the thresholds, or --measure to \
         measure the release artifacts under {}",
        actions.dist.display()
    );
    if actions.validate {
        let report = validate(&repo_root()?)?;
        println!(
            "budget: schema v{} - {} thresholds match {SPEC_FILE} section {SPEC_SECTION} ({BUDGETS_FILE} verified)",
            report.version, report.checked
        );
    }
    if actions.check {
        run_check(actions.bench_group)?;
    }
    if actions.measure {
        run_measure(actions.dist)?;
    }
    Ok(())
}

/// Entry point for the benchmark-budget gate, `xtask budget --check --bench <group>`.
///
/// `group` narrows the assertion to one criterion group (`decode`, `input`,
/// `passthrough`, ...); `None` asserts every case that has a binding. The command
/// fails when a bound case has no criterion output at all, when a measured case is past
/// its threshold, and when `group` names a group no case is bound to -- a gate that
/// asserted nothing because nothing matched must not pass.
///
/// # Errors
/// Returns an error when the budget document is invalid, when the criterion output
/// cannot be read, when `group` names a group no case is bound to, when a bound case is
/// missing from the output, or when any measured case is past its threshold.
pub fn run_check(group: Option<&str>) -> Result<()> {
    let root = repo_root()?;
    let dir = bench::criterion_dir(&root);
    let recorded = meta::write(&dir)?;
    println!("budget: run environment recorded in {}", recorded.display());
    let budgets = read_budgets(&root)?;
    let report = bench::check(&budgets, &dir, group)?;
    for case in &report.unbound {
        println!("budget: {case} is measured but no threshold is bound to it");
    }
    for case in &report.passed {
        println!("budget: {case} within budget");
    }
    ensure!(
        report.missing.is_empty(),
        "no criterion output for {}; run the benchmark first (`cargo bench --workspace`)",
        report.missing.join(", ")
    );
    ensure!(
        report.violations.is_empty(),
        "{} benchmark case(s) past their budget:\n  {}",
        report.violations.len(),
        report.violations.join("\n  ")
    );
    println!(
        "budget: {} benchmark case(s) within budget, {} unasserted",
        report.passed.len(),
        report.unbound.len()
    );
    Ok(())
}

/// Entry point for the size gate, `xtask budget --measure`.
///
/// # Errors
///
/// Returns an error when a release artifact is missing from `dir`, and when one is past
/// its threshold; the failure carries `dist/verify/size-budget-exceeded`.
pub fn run_measure(dir: &Path) -> Result<()> {
    let budgets = read_budgets(&repo_root()?)?;
    let report = measure(&budgets, dir)?;
    for passed in &report.passed {
        println!("budget: {passed} within budget");
    }
    ensure!(
        report.violations.is_empty(),
        "{} release artifact(s) past their budget:\n  {}",
        report.violations.len(),
        report.violations.join("\n  ")
    );
    println!(
        "budget: {} release artifact(s) within budget",
        report.passed.len()
    );
    Ok(())
}

/// Measures the release artifacts in `dir` against `budgets`.
///
/// `dir` is the release directory `xtask package` writes, and what is measured is what
/// that step produced: the stripped libraries, not the build tree's own copies, which are
/// larger and are not what a user downloads. The document is a parameter rather than
/// something this function reads for itself, so that a test can drive the measurement
/// with a threshold it made impossible -- which is what shows the assertion reads the
/// document rather than a constant.
///
/// # Errors
///
/// Returns an error when a threshold the table names is not in the document, when an
/// artifact the release must carry is not in `dir`, and when a file cannot be read.
pub fn measure(budgets: &Budgets, dir: &Path) -> Result<SizeReport> {
    let mut report = SizeReport::default();
    for &MeasuredArtifact(name, key) in MEASURED_ARTIFACTS {
        let path = dir.join(name);
        let limit = bench::threshold(budgets, key).with_context(|| {
            format!("{name}: bound to {key}, which {BUDGETS_FILE} does not carry")
        })?;
        ensure!(
            path.is_file(),
            "dist/verify/artifact-missing: {name} is not in {}; run `just package` first",
            dir.display()
        );
        let bytes = fs::metadata(&path)
            .with_context(|| format!("reading {}", path.display()))?
            .len();
        let size_mb = bytes as f64 / (1024.0 * 1024.0);
        let measured = format!(
            "{} {name} {size_mb:.2}MiB of {limit:.2}MiB",
            bench::owner(key)
        );
        if size_mb > limit {
            // The delivery-channel code the contract defines for this condition, so a
            // release pipeline can match on it instead of on prose.
            let violation = format!("dist/verify/size-budget-exceeded: {measured}");
            report.violations.push(violation);
        } else {
            report.passed.push(measured);
        }
    }
    Ok(report)
}

/// Repository root, derived from the compile-time location of this crate.
pub(crate) fn repo_root() -> Result<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .context("xtask is expected to live in a subdirectory of the repository root")
}
