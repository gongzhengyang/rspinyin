//! `xtask report`: the budget dashboard's command line.
//!
//! Responsibility: read the snapshot a running plugin wrote and the thresholds the
//! budget document states, judge one against the other, and print the result -- a
//! table for a person, `--json` for a CI job.
//!
//! Boundaries: this module reads files and prints. The comparison and both renderings
//! live in `ime-diag`, so the verdict a developer sees here is the same verdict the
//! plugin's own report would produce, and the thresholds come from the budget
//! document through the budget gate's own parser: a number here and a number in
//! `xtask budget --validate` are one number, not two copies of it.
//!
//! The command does not fail a build when a threshold is missed -- it prints the
//! verdict and exits zero, and the job that decides what a missed threshold means
//! reads the document `--json` writes. A failure to read or parse the snapshot, on
//! the other hand, is an error: a report that silently judged nothing would read as a
//! pass.
//!
//! # Pulling from a running plugin
//!
//! The snapshot is a file: the plugin writes it when it receives `SIGUSR1`, and this
//! command reads it afterwards, which is what keeps the report out of the plugin's
//! process and the plugin free of a socket. A live pull over a Unix socket is
//! deliberately not implemented here -- it would put a listening endpoint inside an
//! input method for the convenience of a development session.

use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use ime_diag::probe::{ProbeSnapshot, SNAPSHOT_FILE_NAME};
use ime_diag::report::{Limits, ProbeReport};
use ime_dict::paths::{BaseDirs, Paths};

use crate::budget::{self, Budgets, repo_root};

/// Microseconds in a millisecond.
///
/// The budget document states its latency thresholds in milliseconds (its own section
/// is `latency_ms`) and a histogram records microseconds. The two units meet here and
/// nowhere else.
const MS_TO_US: f64 = 1_000.0;

/// The threshold prefix that carries the millisecond unit.
const LATENCY_SECTION: &str = "latency_ms.";

/// Arguments of `xtask report`.
#[derive(Debug, Args)]
pub struct ReportArgs {
    /// The snapshot file to read, or `-` for standard input.
    ///
    /// Defaults to the file the plugin writes under the XDG data directory.
    #[arg(long, value_name = "PATH")]
    input: Option<PathBuf>,
    /// Print the machine-readable document instead of the text report.
    #[arg(long)]
    json: bool,
}

/// Entry point for `xtask report`.
///
/// # Errors
///
/// Returns an error when the snapshot cannot be read or does not parse, when the
/// budget document cannot be read or does not satisfy its schema, and when the XDG
/// data directory cannot be resolved while looking for the default snapshot.
pub fn run(args: ReportArgs) -> Result<()> {
    let text = read_snapshot(args.input.as_deref())?;
    let snapshot = ProbeSnapshot::parse(&text).context("the probe snapshot is unusable")?;
    let budgets = budget::read_budgets(&repo_root()?)?;
    let report = ProbeReport::compare(&snapshot, &limits_from(&budgets));
    if args.json {
        print!("{}", report.render_json());
    } else {
        print!("{}", report.render_text());
    }
    Ok(())
}

/// The thresholds the budget document states for the metrics a probe records.
///
/// Only the document's `latency_ms` section is converted: every threshold the report
/// judges is a row of it, and the other sections are stated in their own units. The
/// conversion is applied here, where the document is read, so that the report itself
/// never restates a unit.
fn limits_from(budgets: &Budgets) -> Limits {
    let thresholds: Vec<(&'static str, f64)> = budgets
        .thresholds()
        .iter()
        .filter(|threshold| threshold.0.starts_with(LATENCY_SECTION))
        .map(|threshold| (threshold.0, threshold.1 * MS_TO_US))
        .collect();
    Limits::from_thresholds(&thresholds)
}

/// The snapshot text `input` names, or the plugin's own snapshot when it names none.
fn read_snapshot(input: Option<&Path>) -> Result<String> {
    match input {
        Some(path) if path == Path::new("-") => {
            let mut text = String::new();
            io::stdin()
                .read_to_string(&mut text)
                .context("reading the snapshot from standard input")?;
            Ok(text)
        }
        Some(path) => {
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
        }
        None => {
            let path = snapshot_path(&BaseDirs::from_env()?)?;
            fs::read_to_string(&path).with_context(|| {
                format!(
                    "reading {}; the plugin writes it when it receives SIGUSR1",
                    path.display()
                )
            })
        }
    }
}

/// Where the plugin's snapshot file lives in `bases`.
///
/// Derived from the layout the rest of the plugin writes to rather than assembled
/// here, so that the file the plugin writes and the file this command reads are one
/// path by construction.
///
/// # Errors
///
/// Returns an error when the layout's paths would not fit the kernel's path limit.
fn snapshot_path(bases: &BaseDirs) -> Result<PathBuf> {
    Ok(Paths::from_bases(bases)?.data_dir.join(SNAPSHOT_FILE_NAME))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use serde_json::Value;

    use super::*;
    use ime_diag::probe::{HistSnapshot, Metric};

    /// A distribution with the given percentiles.
    fn distribution(p50: u64, p90: u64, p99: u64, p999: u64) -> HistSnapshot {
        HistSnapshot {
            count: 1_832,
            sum_us: 4_000_000,
            p50_us: p50,
            p90_us: p90,
            p99_us: p99,
            p999_us: p999,
            max_us: p999,
        }
    }

    /// A snapshot with one metric filled in.
    fn snapshot_with(metric: Metric, sample: HistSnapshot) -> ProbeSnapshot {
        let mut metrics = ime_diag::probe::Metrics::default();
        *metric.snapshot_mut(&mut metrics) = sample;
        ProbeSnapshot {
            sampled: std::time::Duration::from_secs(312),
            sessions: 47,
            keys: 1_832,
            metrics,
            counters: [0_u64; ime_diag::probe::COUNTER_COUNT],
        }
    }

    #[test]
    fn test_limits_from_the_repository_document_are_microseconds() {
        let budgets = budget::read_budgets(&repo_root().expect("the repository root resolves"))
            .expect("the budget document is valid");
        let limits = limits_from(&budgets);
        let checks: Vec<&ime_diag::report::BudgetCheck> =
            limits.checks(Metric::KeyToPresent).collect();
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].limit_us, 4_000.0);
        assert_eq!(checks[1].limit_us, 16_000.0);
        let decode: Vec<&ime_diag::report::BudgetCheck> = limits.checks(Metric::Decode).collect();
        assert_eq!(decode[0].limit_us, 3_000.0);
        assert_eq!(decode[1].limit_us, 8_000.0);
        // The two thresholds the cards fix but the document does not carry yet are
        // reported without a budget rather than against a number invented here.
        assert_eq!(limits.checks(Metric::Wakeup).count(), 0);
        assert_eq!(limits.checks(Metric::RasterPartial).count(), 0);
    }

    #[test]
    fn test_report_fails_when_the_document_states_an_impossible_threshold() {
        // The reverse validation the design asks for, run through the document's own
        // parser: the threshold is patched to a tenth of a millisecond, and the same
        // measurement that passes at 16ms must then fail.
        let root = repo_root().expect("the repository root resolves");
        let text = fs::read_to_string(root.join(budget::BUDGETS_FILE))
            .expect("the budget document is readable");
        let sample = distribution(2_100, 5_800, 11_300, 18_700);
        let generous = budget::read_budgets(&root).expect("the document is valid");
        let report = ProbeReport::compare(
            &snapshot_with(Metric::KeyToPresent, sample),
            &limits_from(&generous),
        );
        assert_eq!(report.verdict(), ime_diag::report::Verdict::Pass);
        let patched = text.replace(
            "\"key_to_present_p99\": 16.0",
            "\"key_to_present_p99\": 0.1",
        );
        assert_ne!(patched, text, "the threshold to patch is in the document");
        let impossible = Budgets::from_json(&patched).expect("the patched document is valid");
        let report = ProbeReport::compare(
            &snapshot_with(Metric::KeyToPresent, sample),
            &limits_from(&impossible),
        );
        assert_eq!(
            report.verdict(),
            ime_diag::report::Verdict::Fail(ime_diag::probe::Percentile::P99)
        );
        let violations = report.violations();
        assert_eq!(violations.len(), 1);
        assert!(violations[0].contains("latency_ms.key_to_present_p99"));
    }

    #[test]
    fn test_render_json_parses_as_a_json_document() {
        let budgets = budget::read_budgets(&repo_root().expect("the repository root resolves"))
            .expect("the budget document is valid");
        let report = ProbeReport::compare(
            &snapshot_with(Metric::Decode, distribution(420, 1_100, 2_400, 4_800)),
            &limits_from(&budgets),
        );
        let document: Value =
            serde_json::from_str(&report.render_json()).expect("the report is valid JSON");
        assert_eq!(document["verdict"], Value::from("PASS"));
        assert_eq!(document["keys"], Value::from(1_832));
        assert_eq!(
            document["metrics"].as_array().map(Vec::len),
            Some(ime_diag::probe::METRIC_COUNT)
        );
        assert!(document["counters"]["ui.frame.coalesced"].is_u64());
        assert_eq!(document["metrics"][1]["name"], Value::from("decode"));
        assert_eq!(document["metrics"][1]["p99_us"], Value::from(2_400));
    }

    #[test]
    fn test_read_snapshot_reads_the_file_it_is_given() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/xtask-report-scratch");
        fs::create_dir_all(&dir).expect("the scratch directory is creatable");
        let path = dir.join("snapshot.txt");
        let written = ProbeSnapshot::default().to_text();
        fs::write(&path, &written).expect("the snapshot is writable");
        let read = read_snapshot(Some(path.as_path())).expect("the snapshot is readable");
        assert_eq!(read, written);
        assert_eq!(
            ProbeSnapshot::parse(&read).expect("it parses"),
            ProbeSnapshot::default()
        );
        assert!(read_snapshot(Some(dir.join("absent.txt").as_path())).is_err());
    }

    #[test]
    fn test_snapshot_path_lives_under_the_plugin_data_directory() {
        let bases = BaseDirs::from_lookup(|name| match name {
            "HOME" => Some(OsString::from("/home/tester")),
            _ => None,
        })
        .expect("an absolute HOME resolves the bases");
        assert_eq!(
            snapshot_path(&bases).expect("the layout fits"),
            PathBuf::from("/home/tester/.local/share/rspinyin/probe.txt")
        );
    }
}
