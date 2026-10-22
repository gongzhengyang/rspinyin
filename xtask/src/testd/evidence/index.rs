//! The two indexes: the run's own table of cases, and the pointer CI reads.
//!
//! Responsibility: hold one row per case as the run goes, render the run's `index.md` from
//! those rows, keep the cross-run `results/index.json` up to date, and answer the one question
//! a CI job asks of the results tree -- which run is the newest one. Nothing here writes a
//! case's documents; that is [`super::write`]'s side.
//!
//! # Why the journal takes the write's result
//!
//! [`RunJournal::record`] is handed the *result* of writing a case's bundle rather than a
//! boolean, so a case whose evidence could not be written is a row in the index with the reason
//! attached and the run's verdict is `flawed` unless something else failed outright. A harness
//! that only counted its own assertions would report a green run in which half the evidence is
//! missing, which is the failure this whole task exists to make visible.
//!
//! Losing a bundle does not hide the verdict the case reached: the row's status becomes
//! [`CaseStatus::Flawed`] because that is what the case's *evidence* is, and the assertions that
//! did not hold are counted in [`CaseRow::failed`] beside it. A reader sees both -- the case
//! that failed and the evidence that is missing -- rather than one standing in for the other.
//!
//! # Why the newest run is the greatest name
//!
//! A run directory is named `run-YYYYMMDD-HHMMSS`, a fixed-width stamp of the instant it
//! started, so the names sort in the order the runs started. The cross-run index therefore
//! picks the greatest name rather than the last one it happened to record: a run reported late
//! -- or twice -- cannot move the pointer backwards onto a finished run.

use std::path::{Path, PathBuf};

use super::error::EvidenceError;
use super::report::{dedupe_runs, read_runs, register_run, registry_path};
use super::write::{CaseBundle, RunDir, refuse_legacy_layout};
use super::{
    CaseEvidence, CaseStatus, Checklist, ChecklistRow, Dim, RESULTS_DIR, RESULTS_INDEX_FILE,
    RUNS_DIR, Tier, Utc,
};
use crate::testd::env_gate::ELIGIBILITY_CLAIM;

/// The verdict of a whole run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BatchVerdict {
    /// Every case passed and every bundle was written.
    Pass,
    /// At least one case failed.
    Fail,
    /// Nothing failed, but part of the evidence could not be written.
    Flawed,
}

impl BatchVerdict {
    /// The label a report prints.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Flawed => "flawed",
        }
    }
}

/// What a run's index says about one case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseRow {
    /// The module code the case belongs to.
    pub module: String,
    /// The case's identifier.
    pub tc: String,
    /// How the case ended, or [`CaseStatus::Flawed`] when its evidence could not be written.
    pub status: CaseStatus,
    /// How long the case took, in milliseconds.
    pub duration_ms: u64,
    /// How many assertions the case made.
    pub assertions: usize,
    /// How many of them did not hold.
    ///
    /// The count is kept beside the status because a case whose bundle was lost is reported as
    /// flawed, and without this column a reader could not tell whether it had also failed.
    pub failed: usize,
    /// Whether a `trace.json` was written.
    pub trace: bool,
    /// How many edits the case healed.
    pub healed: usize,
    /// Why the bundle is incomplete, when it is.
    pub gap: Option<String>,
    /// The tier the case's v3 record declares, or `None` when it carried none.
    pub tier: Option<Tier>,
    /// The screenshots the case's v3 record declares and the bundle directory does not hold.
    ///
    /// A name here is a *platform defect* rather than a case failure: the case card declared
    /// evidence the harness never wrote. The row stays red in the checklist, the batch's
    /// verdict becomes flawed through [`BatchSummary::screenshot_gaps`], and the case's own
    /// status is not rewritten.
    pub screenshots_missing: Vec<String>,
    /// How many claims the case's v3 record settled.
    pub claims: usize,
    /// How many of those claims did not hold.
    pub failed_claims: usize,
    /// How many screenshots the case's v3 record declares.
    pub screenshots_expected: usize,
    /// The gate's withheld state, when the case was skipped: `pending-impl` or
    /// `needs-environment`.
    pub withheld: Option<String>,
    /// The coverage dimensions the case's v3 record declares.
    pub dims: Vec<Dim>,
    /// The budget keys the case's v3 record declares its thresholds from.
    pub budget_keys: Vec<String>,
}

impl CaseRow {
    /// Whether every artefact the case owes was written.
    ///
    /// Two losses keep a row red: a bundle the write could not produce, and a screenshot the
    /// case's v3 record declares but its directory does not hold. Both are losses a green
    /// summary cannot be allowed to hide, which is what this one predicate is for.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_complete(&self) -> bool {
        self.gap.is_none() && self.screenshots_missing.is_empty()
    }
}

/// What a run came to, counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BatchSummary {
    /// Cases that passed.
    pub passed: usize,
    /// Cases that failed an assertion.
    pub failed: usize,
    /// Cases that could not be judged, or whose evidence could not be written.
    pub flawed: usize,
    /// Cases the gate withheld before they ran.
    ///
    /// Neither a pass nor a failure -- nothing was judged -- so they are listed separately
    /// and are counted by neither [`BatchSummary::cases`] nor the verdict: a batch whose
    /// every case was skipped has judged nothing, and its counts say so.
    pub skipped: usize,
    /// Cases whose bundle is incomplete.
    pub evidence_gaps: usize,
    /// Cases whose v3 record declares screenshots the bundle directory does not hold.
    ///
    /// A declared screenshot that never landed is a platform defect, not a case failure, so it
    /// is counted here rather than in [`BatchSummary::failed`] -- but a run that lost evidence
    /// is still not a run that can be published green, which is what
    /// [`BatchSummary::verdict`] decides.
    pub screenshot_gaps: usize,
    /// Self-healing edits the run made.
    pub healed: usize,
}

impl BatchSummary {
    /// How many cases the run holds.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn cases(&self) -> usize {
        self.passed + self.failed + self.flawed
    }

    /// What the run came to.
    ///
    /// A failure outranks a gap: a run in which one case failed and another lost its evidence
    /// is a failed run, and the gap is still reported beside it by
    /// [`BatchSummary::evidence_gaps`]. A screenshot the evidence declares but the directory
    /// does not hold is the same class of loss -- the run's evidence is incomplete -- so it
    /// holds the verdict at flawed by the same branch.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn verdict(&self) -> BatchVerdict {
        if self.failed > 0 {
            BatchVerdict::Fail
        } else if self.flawed > 0 || self.evidence_gaps > 0 || self.screenshot_gaps > 0 {
            BatchVerdict::Flawed
        } else {
            BatchVerdict::Pass
        }
    }
}

/// The run's own record: one row per case, and the verdict that follows from them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunJournal {
    /// The instant the run started.
    started: Utc,
    /// The rows, in the order the cases were recorded.
    rows: Vec<CaseRow>,
}

impl RunJournal {
    /// A journal for a run that started at `started`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new(started: Utc) -> Self {
        Self {
            started,
            rows: Vec::new(),
        }
    }

    /// Records a case and the bundle it produced.
    ///
    /// `bundle` is the result of writing the case's evidence, not a success: a write that
    /// failed marks the case flawed and carries the reason into the index, which is what makes
    /// the loss visible in the run's verdict rather than only in the output of the harness.
    ///
    /// When the case carries a v3 record that declares screenshots, the names it declares are
    /// checked against the bundle's own directory as it stands -- the one existence question a
    /// journal can answer, because the directory it reads is the one the write just produced.
    /// A declared name that is not there becomes the row's red list and counts the run's
    /// screenshot gap, but the case's own verdict is not rewritten: missing evidence is a
    /// platform defect, not a case failure.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn record(&mut self, evidence: &CaseEvidence, bundle: Result<CaseBundle, EvidenceError>) {
        let (status, trace, gap) = match &bundle {
            Ok(bundle) => (evidence.status, bundle.trace.is_some(), None),
            Err(error) => (CaseStatus::Flawed, false, Some(error.reason())),
        };
        // The existence probe reads only the directory the bundle names, and only for the names
        // the case's own record declares; a case with no v3 record, or one that declares no
        // screenshots, touches nothing.
        let (tier, screenshots_missing, claims, failed_claims, screenshots_expected) =
            match (&evidence.v3, &bundle) {
                (Some(record), Ok(bundle)) => (
                    Some(record.tier),
                    record.missing_screenshots(&bundle.dir),
                    record.claims.len(),
                    record.failed_claims().count(),
                    record.screenshots_expected.len(),
                ),
                (Some(record), Err(_)) => (
                    Some(record.tier),
                    Vec::new(),
                    record.claims.len(),
                    record.failed_claims().count(),
                    record.screenshots_expected.len(),
                ),
                (None, _) => (None, Vec::new(), 0, 0, 0),
            };
        // The withheld state, the dimensions and the budget keys are the v3 record's own, so
        // the checklist a report is regenerated from carries everything the report states.
        let v3 = evidence.v3.as_ref();
        let withheld = v3.and_then(|record| {
            record
                .claims
                .iter()
                .find(|claim| claim.name == ELIGIBILITY_CLAIM && !claim.pass)
                .and_then(|claim| claim.actual.split(':').next())
                .map(str::to_owned)
        });
        let dims = v3.map(|record| record.dims.clone()).unwrap_or_default();
        let budget_keys = v3
            .map(|record| record.budget_keys.clone())
            .unwrap_or_default();
        self.rows.push(CaseRow {
            module: evidence.module.clone(),
            tc: evidence.tc.clone(),
            status,
            duration_ms: evidence.duration_ms,
            assertions: evidence.assertion_count(),
            failed: evidence.failed_assertions().len(),
            trace,
            healed: evidence.healed.len(),
            gap,
            tier,
            screenshots_missing,
            claims,
            failed_claims,
            screenshots_expected,
            withheld,
            dims,
            budget_keys,
        });
    }

    /// The rows, in the order the cases were recorded.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn rows(&self) -> &[CaseRow] {
        &self.rows
    }

    /// What the run came to, counted.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn summary(&self) -> BatchSummary {
        let mut summary = BatchSummary::default();
        for row in &self.rows {
            match row.status {
                CaseStatus::Pass => summary.passed += 1,
                CaseStatus::Fail => summary.failed += 1,
                CaseStatus::Flawed => summary.flawed += 1,
                CaseStatus::Skipped => summary.skipped += 1,
            }
            if !row.is_complete() {
                summary.evidence_gaps += 1;
            }
            if !row.screenshots_missing.is_empty() {
                summary.screenshot_gaps += 1;
            }
            summary.healed += row.healed;
        }
        summary
    }

    /// Renders the run's `index.md`: the table that says which case left what behind.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn render_index(&self, run: &RunDir) -> String {
        let summary = self.summary();
        let mut text = String::new();
        text.push_str(&format!("# Run {}\n\n", run.name()));
        text.push_str(&format!("- started: {}\n", self.started.iso8601()));
        text.push_str(&format!(
            "- cases: {} (pass {}, fail {}, flawed {}, skipped {})\n",
            summary.cases(),
            summary.passed,
            summary.failed,
            summary.flawed,
            summary.skipped
        ));
        text.push_str(&format!("- evidence gaps: {}\n", summary.evidence_gaps));
        text.push_str(&format!("- healed: {}\n", summary.healed));
        text.push_str(&format!("- verdict: {}\n\n", summary.verdict().label()));
        text.push_str(
            "| module | case | tier | status | duration | assertions | failed | trace | healed | evidence |\n",
        );
        text.push_str("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n");
        for row in &self.rows {
            let tier = row.tier.map(Tier::label).unwrap_or("-");
            let evidence = match (&row.gap, row.screenshots_missing.is_empty()) {
                (Some(reason), _) => format!("gap: {}", cell(reason)),
                (None, true) => String::from("complete"),
                (None, false) => {
                    format!("missing: {}", cell(&row.screenshots_missing.join(", ")))
                }
            };
            text.push_str(&format!(
                "| {} | {} | {} | {} | {} ms | {} | {} | {} | {} | {} |\n",
                cell(&row.module),
                cell(&row.tc),
                tier,
                row.status.label(),
                row.duration_ms,
                row.assertions,
                row.failed,
                if row.trace { "yes" } else { "no" },
                row.healed,
                evidence
            ));
        }
        text
    }

    /// Writes the run's `index.md` into the run's own tree.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::Io`] when the file cannot be written or its mode cannot be set.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn write_index(&self, run: &RunDir) -> Result<PathBuf, EvidenceError> {
        run.write_index(&self.render_index(run))
    }

    /// Builds the batch checklist the run's directory holds.
    ///
    /// The rows are the journal's own: the checklist owns no state a second copy could drift
    /// from, which is what lets a report be regenerated from the run directory alone.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn checklist(&self, run: &RunDir) -> Checklist {
        let rows = self
            .rows
            .iter()
            .map(|row| ChecklistRow {
                module: row.module.clone(),
                case: row.tc.clone(),
                tier: row.tier,
                status: row.status,
                claims: row.claims,
                failed_claims: row.failed_claims,
                assertions: row.gap.is_none(),
                trace: row.trace,
                screenshots_expected: row.screenshots_expected,
                screenshots_present: row
                    .screenshots_expected
                    .saturating_sub(row.screenshots_missing.len()),
                screenshots_missing: row.screenshots_missing.clone(),
                gap: row.gap.clone(),
                withheld: row.withheld.clone(),
                dims: row.dims.clone(),
                budget_keys: row.budget_keys.clone(),
            })
            .collect();
        Checklist::of(run.name(), self.summary().verdict(), rows)
    }

    /// Writes the run's index and appends the run to the cross-run index.
    ///
    /// The two are written together because a run that appears in one and not the other is a
    /// run a reader has to reconcile by hand: the run's own table is what a person reads, and
    /// the cross-run document is what CI resolves `latest_run` from.
    ///
    /// # Errors
    ///
    /// As [`RunJournal::write_index`] and [`register_run`].
    ///
    /// # Panics
    ///
    /// Never.
    pub fn publish(&self, run: &RunDir, results_root: &Path) -> Result<(), EvidenceError> {
        self.write_index(run)?;
        // The checklist is the batch's evidence-existence table; it is published beside the
        // index so one run directory carries both the human table and the machine-checkable
        // one, and neither can be had without the other.
        let checklist = self.checklist(run);
        run.write_checklist(&checklist.to_json()?)?;
        register_run(&registry_path(results_root), self.entry(run))
    }

    /// The run as the cross-run index holds it.
    ///
    /// # Panics
    ///
    /// Never.
    fn entry(&self, run: &RunDir) -> RunEntry {
        let summary = self.summary();
        RunEntry {
            run: run.name().to_owned(),
            path: format!("{RESULTS_DIR}/{RUNS_DIR}/{}", run.name()),
            started_at: self.started.iso8601(),
            verdict: summary.verdict(),
            cases: summary.cases(),
            passed: summary.passed,
            failed: summary.failed,
            flawed: summary.flawed,
            evidence_gaps: summary.evidence_gaps,
            healed: summary.healed,
        }
    }
}

/// One run, as the cross-run index holds it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RunEntry {
    /// The run's directory name, e.g. `run-20260930-014455`.
    pub run: String,
    /// The run's directory, relative to the results tree.
    pub path: String,
    /// The instant the run started, in UTC.
    pub started_at: String,
    /// What the run came to.
    pub verdict: BatchVerdict,
    /// How many cases it held.
    pub cases: usize,
    /// How many passed.
    pub passed: usize,
    /// How many failed.
    pub failed: usize,
    /// How many were flawed.
    pub flawed: usize,
    /// How many bundles are incomplete.
    pub evidence_gaps: usize,
    /// How many edits the run healed.
    pub healed: usize,
}

/// The cross-run index: every run the results tree holds, and which one is newest.
///
/// The registry the index reads is append-only -- one [`RunEntry`] per line, written by
/// [`register_run`] -- so opening it never rewrites anything, and a line that cannot be
/// parsed costs a warning and one lost entry rather than the whole history.
#[derive(Clone, Debug)]
pub struct RunIndex {
    /// The file the registry is read from.
    path: PathBuf,
    /// The runs the registry holds, deduplicated to the last line that named each run.
    runs: Vec<RunEntry>,
    /// The newest run the registry holds, the greatest run *name* among them.
    latest_run: Option<String>,
}

impl RunIndex {
    /// Opens the cross-run index under `results_root`, reading it when it is there.
    ///
    /// A results tree with no index yet is a tree whose first run has not finished, so a
    /// missing file opens an empty index. A line the registry holds that this build cannot
    /// parse is skipped with a warning rather than refused: the entries around it survive,
    /// and the file is left exactly as it was found.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::LegacyLayout`] when the root is a flat layout, and
    /// [`EvidenceError::Io`] when the registry exists and cannot be read.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn open(results_root: &Path) -> Result<Self, EvidenceError> {
        refuse_legacy_layout(results_root)?;
        let path = registry_path(results_root);
        // A run that was published twice is one entry, and the line appended later is the
        // more current statement about it.
        let runs = dedupe_runs(read_runs(&path)?);
        let latest_run = runs
            .iter()
            .map(|held| held.run.as_str())
            .max()
            .map(str::to_owned);
        Ok(Self {
            path,
            runs,
            latest_run,
        })
    }

    /// The file the registry is read from.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The run the index points at as the newest, or `None` when it holds no run yet.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn latest_run(&self) -> Option<&str> {
        self.latest_run.as_deref()
    }

    /// The runs the index holds, in the order they were first recorded.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn runs(&self) -> &[RunEntry] {
        &self.runs
    }
}

/// One cell of the run's table, with the two characters a cell cannot carry removed.
///
/// A markdown table cell ends at a pipe and a row ends at a newline, so a reason that carried
/// either would shift every column after it. A filesystem message never does, and a cell that
/// silently broke the table would be worse than one that lost a character.
///
/// # Panics
///
/// Never.
fn cell(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '|' => '/',
            '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}
