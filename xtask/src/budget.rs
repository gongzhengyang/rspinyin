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

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value};

use self::SpecCell::{MetricAfter, ThresholdAfter, ThresholdFirst, ThresholdZero};

/// Path of the machine-readable budget file, relative to the repository root.
pub const BUDGETS_FILE: &str = "docs/dev/budgets.json";

/// Path of the document that owns the authoritative thresholds.
pub const SPEC_FILE: &str = "docs/dev/features.md";

/// Section of [`SPEC_FILE`] that holds the budget table.
pub const SPEC_SECTION: &str = "0.5.3";

/// Tolerance for float comparisons; both sides are short decimal literals.
const EPSILON: f64 = 1e-9;

/// Top-level keys that are metadata rather than thresholds.
const META_KEYS: &[&str] = &["version", "source", "notes"];

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

impl Budgets {
    /// Parse and schema-check a `budgets.json` document.
    ///
    /// # Errors
    /// Returns an error when the text is not valid JSON, when `version` is
    /// unsupported, when a required key is missing, when a value is not a finite
    /// number of the expected shape, or when an unknown key is present. Unknown
    /// keys are rejected rather than ignored so a typo cannot silently disable a
    /// threshold.
    pub fn from_json(text: &str) -> Result<Self> {
        let parsed: Value = serde_json::from_str(text).context("document is not valid JSON")?;
        let root = parsed
            .as_object()
            .context("top level must be a JSON object")?;

        let version = root
            .get("version")
            .context("top level: missing key `version`")?
            .as_u64()
            .context("top level: `version` must be a non-negative integer")?;
        ensure!(
            version == 1,
            "top level: unsupported `version` {version}, expected 1"
        );
        root.get("source")
            .context("top level: missing key `source`")?
            .as_str()
            .context("top level: `source` must name the authoritative table")?;
        if let Some(notes) = root.get("notes") {
            for note in notes
                .as_array()
                .context("top level: `notes` must be strings")?
            {
                note.as_str()
                    .context("top level: `notes` must be strings")?;
            }
        }

        let latency = section(root, "latency_ms")?;
        let memory = section(root, "memory_mb")?;
        let cpu = section(root, "cpu_pct")?;
        let size = section(root, "size_mb")?;
        let robust = section(root, "robustness")?;

        let budgets = Self {
            version,
            latency_ms: LatencyMs {
                key_to_present_p50: positive(latency, "latency_ms", "key_to_present_p50")?,
                key_to_present_p99: positive(latency, "latency_ms", "key_to_present_p99")?,
                key_to_present_p99_144hz: positive(
                    latency,
                    "latency_ms",
                    "key_to_present_p99_144hz",
                )?,
                decode_p99: positive(latency, "latency_ms", "decode_p99")?,
                decode_p999: positive(latency, "latency_ms", "decode_p999")?,
                raster_p99: positive(latency, "latency_ms", "raster_p99")?,
                first_key_to_visible_p99: positive(
                    latency,
                    "latency_ms",
                    "first_key_to_visible_p99",
                )?,
                addon_load: positive(latency, "latency_ms", "addon_load")?,
            },
            memory_mb: MemoryMb {
                ui_rss: positive(memory, "memory_mb", "ui_rss")?,
                plugin_rss: positive(memory, "memory_mb", "plugin_rss")?,
                dict_mmap_rss: positive(memory, "memory_mb", "dict_mmap_rss")?,
            },
            cpu_pct: CpuPct {
                idle: positive(cpu, "cpu_pct", "idle")?,
                typing_10cps: positive(cpu, "cpu_pct", "typing_10cps")?,
                idle_redraw_count: count(cpu, "cpu_pct", "idle_redraw_count")?,
                idle_poll_timer_count: count(cpu, "cpu_pct", "idle_poll_timer_count")?,
            },
            size_mb: SizeMb {
                so_stripped: positive(size, "size_mb", "so_stripped")?,
                base_dict: positive(size, "size_mb", "base_dict")?,
            },
            robustness: Robustness {
                soak_hours: positive(robust, "robustness", "soak_hours")?,
                rss_drift_mb: positive(robust, "robustness", "rss_drift_mb")?,
                pass_rate_pct: positive(robust, "robustness", "pass_rate_pct")?,
            },
            net_sockets: count(root, "top level", "net_sockets")?,
        };
        budgets.reject_unknown_paths(&parsed)?;
        Ok(budgets)
    }

    /// Flatten the document into the numeric thresholds it carries.
    ///
    /// The keys are the dotted paths used by `BINDINGS`, which is what lets the
    /// spec comparison run without a second copy of the numbers anywhere.
    pub fn thresholds(&self) -> Vec<Threshold> {
        let (l, m, c, s, r) = (
            &self.latency_ms,
            &self.memory_mb,
            &self.cpu_pct,
            &self.size_mb,
            &self.robustness,
        );
        [
            Threshold("latency_ms.key_to_present_p50", l.key_to_present_p50),
            Threshold("latency_ms.key_to_present_p99", l.key_to_present_p99),
            Threshold(
                "latency_ms.key_to_present_p99_144hz",
                l.key_to_present_p99_144hz,
            ),
            Threshold("latency_ms.decode_p99", l.decode_p99),
            Threshold("latency_ms.decode_p999", l.decode_p999),
            Threshold("latency_ms.raster_p99", l.raster_p99),
            Threshold(
                "latency_ms.first_key_to_visible_p99",
                l.first_key_to_visible_p99,
            ),
            Threshold("latency_ms.addon_load", l.addon_load),
            Threshold("memory_mb.ui_rss", m.ui_rss),
            Threshold("memory_mb.plugin_rss", m.plugin_rss),
            Threshold("memory_mb.dict_mmap_rss", m.dict_mmap_rss),
            Threshold("cpu_pct.idle", c.idle),
            Threshold("cpu_pct.typing_10cps", c.typing_10cps),
            Threshold("cpu_pct.idle_redraw_count", c.idle_redraw_count as f64),
            Threshold(
                "cpu_pct.idle_poll_timer_count",
                c.idle_poll_timer_count as f64,
            ),
            Threshold("size_mb.so_stripped", s.so_stripped),
            Threshold("size_mb.base_dict", s.base_dict),
            Threshold("robustness.soak_hours", r.soak_hours),
            Threshold("robustness.rss_drift_mb", r.rss_drift_mb),
            Threshold("robustness.pass_rate_pct", r.pass_rate_pct),
            Threshold("net_sockets", self.net_sockets as f64),
        ]
        .to_vec()
    }

    /// Reject every value that is neither metadata nor a known threshold.
    fn reject_unknown_paths(&self, parsed: &Value) -> Result<()> {
        let mut known: Vec<String> = META_KEYS.iter().map(|key| (*key).to_owned()).collect();
        known.extend(self.thresholds().iter().map(|t| t.0.to_owned()));
        walk(parsed, "", &known)
    }
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

/// Parse the budget table out of the spec document.
///
/// The table is located by its `#### <section>` heading and read until the first
/// line that is not a table row. Rows whose first cell is not a backticked
/// `BUDGET-*` identifier (the header and separator rows) are skipped.
///
/// # Errors
/// Returns an error when the section heading is absent or when the section holds
/// no budget rows, so a renamed section fails loudly instead of validating
/// nothing.
pub fn parse_spec_table(spec: &str) -> Result<BTreeMap<String, SpecRow>> {
    let mut rows = BTreeMap::new();
    let mut in_section = false;
    let mut seen_table = false;
    for line in spec.lines() {
        let trimmed = line.trim();
        if !in_section {
            if let Some(rest) = trimmed.strip_prefix("#### ") {
                if rest.starts_with(SPEC_SECTION) {
                    in_section = true;
                }
            }
            continue;
        }
        if trimmed.starts_with('|') {
            seen_table = true;
            if let Some((id, row)) = parse_spec_row(trimmed) {
                rows.insert(id, row);
            }
            continue;
        }
        if seen_table {
            break;
        }
    }
    ensure!(
        in_section,
        "section `{SPEC_SECTION}` not found in the spec document"
    );
    ensure!(
        !rows.is_empty(),
        "section `{SPEC_SECTION}` has no `BUDGET-*` table rows"
    );
    Ok(rows)
}

/// Read one spec table row, if it is a budget row.
fn parse_spec_row(line: &str) -> Option<(String, SpecRow)> {
    let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
    if cells.len() < 3 {
        return None;
    }
    let id = cells[0].strip_prefix('`')?.strip_suffix('`')?;
    if !id.starts_with("BUDGET-") {
        return None;
    }
    let row = SpecRow {
        budget_id: id.to_owned(),
        metric: cells[1].to_owned(),
        threshold: cells[2].to_owned(),
    };
    Some((id.to_owned(), row))
}

/// Return the first decimal number in `text` at or after `needle`.
///
/// An empty `needle` starts the search at the beginning of the cell. The spec
/// writes thresholds as prose (`P99 ≤ 16ms`, `连续 8 小时`, `重绘次数 = 0`), so the
/// scan ignores the comparison operator and takes the number that follows the
/// anchor.
fn first_number_after(text: &str, needle: &str) -> Option<f64> {
    let start = if needle.is_empty() {
        0
    } else {
        text.find(needle)? + needle.len()
    };
    let tail = text.get(start..)?;
    let mut chars = tail.char_indices().peekable();
    let mut previous: Option<char> = None;
    while let Some((offset, ch)) = chars.next() {
        if !ch.is_ascii_digit() {
            previous = Some(ch);
            continue;
        }
        // A digit run introduced by `P` is a percentile label, not a threshold.
        // The spec packs several percentiles into one cell (`144Hz 环境 P99 ≤ 12ms`),
        // so anchoring on `144Hz` would otherwise read the `99` of `P99`.
        let is_percentile_label = matches!(previous, Some('P' | 'p'));
        let mut end = offset + 1;
        while let Some(&(next_offset, next_ch)) = chars.peek() {
            if next_ch.is_ascii_digit() || next_ch == '.' {
                end = next_offset + 1;
                chars.next();
            } else {
                break;
            }
        }
        if is_percentile_label {
            previous = Some('9');
            continue;
        }
        return tail.get(offset..end)?.parse::<f64>().ok();
    }
    None
}

/// Compare every threshold against the spec table.
///
/// # Errors
/// Returns an error listing every disagreement found, so one run reports the
/// whole drift instead of the first symptom.
pub fn compare(budgets: &Budgets, table: &BTreeMap<String, SpecRow>) -> Result<Report> {
    let thresholds = budgets.thresholds();
    let mut problems: Vec<String> = Vec::new();

    for threshold in &thresholds {
        let (key, value) = (threshold.0, threshold.1);
        let Some(binding) = BINDINGS.iter().find(|b| b.0 == key) else {
            problems.push(format!(
                "{key}: no spec binding is declared for this threshold; add it to BINDINGS"
            ));
            continue;
        };
        let (id, cell) = (binding.1, binding.2);
        let Some(row) = table.get(id) else {
            problems.push(format!(
                "{key}: {id} is missing from the spec table (renamed or removed?)"
            ));
            continue;
        };
        match cell {
            ThresholdFirst => check_number(&mut problems, key, value, row, "", &row.threshold),
            ThresholdAfter(needle) => {
                check_number(&mut problems, key, value, row, needle, &row.threshold);
            }
            MetricAfter(needle) => {
                check_number(&mut problems, key, value, row, needle, &row.metric);
            }
            ThresholdZero(needle) => check_zero(&mut problems, key, value, row, needle),
        }
    }
    for binding in BINDINGS {
        if !thresholds.iter().any(|t| t.0 == binding.0) {
            problems.push(format!(
                "{}: bound in BINDINGS but absent from {BUDGETS_FILE}",
                binding.0
            ));
        }
    }

    if problems.is_empty() {
        return Ok(Report {
            version: budgets.version,
            checked: thresholds.len(),
        });
    }
    bail!(
        "{BUDGETS_FILE} disagrees with {SPEC_FILE} section {SPEC_SECTION} ({} problem(s)):\n  - {}\n\
         edit {BUDGETS_FILE} to match the spec table, or change the spec table first; the spec is authoritative",
        problems.len(),
        problems.join("\n  - ")
    )
}

/// Record a problem when the spec cell does not state the budgeted number.
fn check_number(
    problems: &mut Vec<String>,
    key: &str,
    value: f64,
    row: &SpecRow,
    needle: &str,
    text: &str,
) {
    match first_number_after(text, needle) {
        None => {
            let anchor = if needle.is_empty() {
                "the cell".to_owned()
            } else {
                format!("`{needle}`")
            };
            problems.push(format!(
                "{key} ({}): no number follows {anchor} in the spec cell \"{text}\"",
                row.budget_id
            ));
        }
        Some(spec_value) if (spec_value - value).abs() > EPSILON => {
            problems.push(format!(
                "{key} ({}): the spec says {spec_value}, {BUDGETS_FILE} says {value}",
                row.budget_id
            ));
        }
        Some(_) => {}
    }
}

/// Record a problem when a zero-assertion in the spec is not honoured.
fn check_zero(problems: &mut Vec<String>, key: &str, value: f64, row: &SpecRow, needle: &str) {
    if !row.threshold.contains(needle) {
        problems.push(format!(
            "{key} ({}): the spec cell no longer mentions `{needle}`; review this binding",
            row.budget_id
        ));
    } else if value != 0.0 {
        problems.push(format!(
            "{key} ({}): the spec requires `{needle}`, so the budget must be 0, but {BUDGETS_FILE} says {value}",
            row.budget_id
        ));
    }
}

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

/// Reject every leaf of `value` whose dotted path is not in `known`.
fn walk(value: &Value, path: &str, known: &[String]) -> Result<()> {
    let Some(map) = value.as_object() else {
        ensure!(
            known.iter().any(|k| k == path),
            "unknown key `{path}` in {BUDGETS_FILE} (known keys: {})",
            known.join(", ")
        );
        return Ok(());
    };
    for (key, child) in map {
        let child_path = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        walk(child, &child_path, known)?;
    }
    Ok(())
}

/// Fetch a nested object.
fn section<'a>(root: &'a Map<String, Value>, name: &str) -> Result<&'a Map<String, Value>> {
    root.get(name)
        .with_context(|| format!("top level: missing section `{name}`"))?
        .as_object()
        .with_context(|| format!("top level: `{name}` must be an object"))
}

/// Fetch a strictly positive finite number.
fn positive(object: &Map<String, Value>, where_: &str, key: &str) -> Result<f64> {
    let value = object
        .get(key)
        .with_context(|| format!("{where_}: missing key `{key}`"))?
        .as_f64()
        .with_context(|| format!("{where_}.{key}: must be a number"))?;
    ensure!(
        value.is_finite() && value > 0.0,
        "{where_}.{key}: must be a finite number greater than zero, found {value}"
    );
    Ok(value)
}

/// Fetch a non-negative integer count.
fn count(object: &Map<String, Value>, where_: &str, key: &str) -> Result<u64> {
    object
        .get(key)
        .with_context(|| format!("{where_}: missing key `{key}`"))?
        .as_u64()
        .with_context(|| format!("{where_}.{key}: must be a non-negative integer"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real budget document, as a mutable JSON value.
    fn real_json() -> Result<Value> {
        let path = repo_root()?.join(BUDGETS_FILE);
        let text =
            fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))
    }

    /// The real budget document, typed.
    fn real_budgets() -> Result<Budgets> {
        Budgets::from_json(&real_json()?.to_string())
    }

    /// The real spec table, parsed.
    fn real_table() -> Result<BTreeMap<String, SpecRow>> {
        let path = repo_root()?.join(SPEC_FILE);
        let text =
            fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
        parse_spec_table(&text)
    }

    /// Apply `edit` to one section of the real document and re-serialise it.
    fn edited(name: &str, edit: impl FnOnce(&mut Map<String, Value>)) -> Result<String> {
        let mut root = real_json()?;
        let section = root
            .as_object_mut()
            .context("top level must be an object")?
            .get_mut(name)
            .and_then(Value::as_object_mut)
            .with_context(|| format!("{name} must be an object"))?;
        edit(section);
        Ok(root.to_string())
    }

    #[test]
    fn test_parse_spec_table_reads_repository_section() -> Result<()> {
        let table = real_table()?;
        let ids = "BUDGET-LAT-01 BUDGET-LAT-02 BUDGET-LAT-03 BUDGET-LAT-04 BUDGET-LAT-05 BUDGET-MEM-01 \
                   BUDGET-MEM-02 BUDGET-MEM-03 BUDGET-CPU-01 BUDGET-CPU-02 BUDGET-SIZE-01 \
                   BUDGET-SIZE-02 BUDGET-NET-01 BUDGET-ROB-01";
        for id in ids.split_whitespace() {
            let row = table.get(id).with_context(|| format!("{id} missing"))?;
            assert!(!row.metric.is_empty(), "{id}: empty metric column");
            assert!(!row.threshold.is_empty(), "{id}: empty threshold column");
        }
        Ok(())
    }

    #[test]
    fn test_parse_spec_table_without_section_is_error() {
        let result = parse_spec_table("# rspinyin\n\nno budget table here\n");
        assert!(result.is_err(), "a document without the section must fail");
    }

    #[test]
    fn test_parse_spec_table_skips_header_and_separator_rows() -> Result<()> {
        let spec = "#### 0.5.3 budget\n\n| id | metric | threshold |\n|---|---|---|\n\
                    | `BUDGET-LAT-09` | sample metric | P99 ≤ 7ms |\n\ntrailing prose\n";
        let table = parse_spec_table(spec)?;
        assert_eq!(table.len(), 1);
        let row = table.get("BUDGET-LAT-09").context("row was not parsed")?;
        assert_eq!(row.threshold, "P99 ≤ 7ms");
        Ok(())
    }

    #[test]
    fn test_first_number_after_handles_prose_cells() {
        assert_eq!(
            first_number_after("P50 ≤ 4ms，P99 ≤ 16ms", "P99"),
            Some(16.0)
        );
        assert_eq!(
            first_number_after("144Hz 环境 P99 ≤ 12ms", "144Hz"),
            Some(12.0)
        );
        assert_eq!(
            first_number_after("连续 8 小时（RSS 漂移 ≤ 2MB）", "RSS"),
            Some(2.0)
        );
        assert_eq!(first_number_after("≤ 120ms", ""), Some(120.0));
        assert_eq!(
            first_number_after("重绘次数 = 0；无轮询定时器", "重绘次数"),
            Some(0.0)
        );
        assert_eq!(first_number_after("no digits here", ""), None);
        assert_eq!(first_number_after("P99 ≤ 3ms", "P999"), None);
    }

    #[test]
    fn test_budgets_from_json_rejects_missing_section() {
        let json = r#"{"version":1,"source":"x","latency_ms":{}}"#;
        assert!(
            Budgets::from_json(json).is_err(),
            "a document without memory_mb must fail"
        );
    }

    #[test]
    fn test_budgets_from_json_rejects_unknown_key() -> Result<()> {
        let text = edited("latency_ms", |section| {
            section.insert("decode_p98".to_owned(), Value::from(9.0));
        })?;
        assert!(
            Budgets::from_json(&text).is_err(),
            "a misspelled threshold must not be ignored"
        );
        Ok(())
    }

    #[test]
    fn test_budgets_from_json_rejects_non_numeric_value() -> Result<()> {
        let text = edited("latency_ms", |section| {
            section.insert("decode_p99".to_owned(), Value::from("3ms"));
        })?;
        assert!(
            Budgets::from_json(&text).is_err(),
            "a string where a number is required must fail"
        );
        Ok(())
    }

    #[test]
    fn test_budgets_from_json_rejects_unsupported_version() -> Result<()> {
        let mut root = real_json()?;
        root.as_object_mut()
            .context("top level must be an object")?
            .insert("version".to_owned(), Value::from(2));
        assert!(
            Budgets::from_json(&root.to_string()).is_err(),
            "an unsupported schema version must fail"
        );
        Ok(())
    }

    #[test]
    fn test_validate_accepts_repository_files() -> Result<()> {
        let report = validate(&repo_root()?)?;
        assert_eq!(report.checked, BINDINGS.len(), "all bindings compared");
        assert_eq!(report.version, 1, "the budget schema version is 1");
        Ok(())
    }

    #[test]
    fn test_validate_detects_threshold_drift() -> Result<()> {
        let mut budgets = real_budgets()?;
        budgets.latency_ms.decode_p99 = 0.001;
        let message = format!("{:?}", compare(&budgets, &real_table()?).err());
        assert!(message.contains("BUDGET-LAT-02"), "drift: {message}");
        assert!(message.contains("latency_ms.decode_p99"), "key: {message}");
        Ok(())
    }

    #[test]
    fn test_compare_reports_zero_assertion_violation() -> Result<()> {
        let mut budgets = real_budgets()?;
        budgets.net_sockets = 1;
        let message = format!("{:?}", compare(&budgets, &real_table()?).err());
        assert!(message.contains("net_sockets"), "zero assertion: {message}");
        Ok(())
    }

    #[test]
    fn test_every_binding_names_a_real_spec_row() -> Result<()> {
        let table = real_table()?;
        for binding in BINDINGS {
            assert!(
                table.contains_key(binding.1),
                "{} is bound to {}, which is not in the spec table",
                binding.0,
                binding.1
            );
        }
        Ok(())
    }
}
