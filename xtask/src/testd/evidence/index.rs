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

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::error::{self, EvidenceError};
use super::write::{CaseBundle, RunDir, refuse_legacy_layout, write_private};
use super::{
    CaseEvidence, CaseStatus, INDEX_FORMAT_VERSION, RESULTS_DIR, RESULTS_INDEX_FILE, RUNS_DIR, Utc,
};

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
}

impl CaseRow {
    /// Whether every artefact of the case's bundle was written.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_complete(&self) -> bool {
        self.gap.is_none()
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
    /// Cases whose bundle is incomplete.
    pub evidence_gaps: usize,
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
    /// [`BatchSummary::evidence_gaps`].
    ///
    /// # Panics
    ///
    /// Never.
    pub fn verdict(&self) -> BatchVerdict {
        if self.failed > 0 {
            BatchVerdict::Fail
        } else if self.flawed > 0 || self.evidence_gaps > 0 {
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
    /// # Panics
    ///
    /// Never.
    pub fn record(&mut self, evidence: &CaseEvidence, bundle: Result<CaseBundle, EvidenceError>) {
        let (status, trace, gap) = match &bundle {
            Ok(bundle) => (evidence.status, bundle.trace.is_some(), None),
            Err(error) => (CaseStatus::Flawed, false, Some(error.reason())),
        };
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
            }
            if !row.is_complete() {
                summary.evidence_gaps += 1;
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
            "- cases: {} (pass {}, fail {}, flawed {})\n",
            summary.cases(),
            summary.passed,
            summary.failed,
            summary.flawed
        ));
        text.push_str(&format!("- evidence gaps: {}\n", summary.evidence_gaps));
        text.push_str(&format!("- healed: {}\n", summary.healed));
        text.push_str(&format!("- verdict: {}\n\n", summary.verdict().label()));
        text.push_str(
            "| module | case | status | duration | assertions | failed | trace | healed | evidence |\n",
        );
        text.push_str("| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n");
        for row in &self.rows {
            let evidence = match &row.gap {
                Some(reason) => format!("gap: {}", cell(reason)),
                None => String::from("complete"),
            };
            text.push_str(&format!(
                "| {} | {} | {} | {} ms | {} | {} | {} | {} | {} |\n",
                cell(&row.module),
                cell(&row.tc),
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

    /// Writes the run's index and appends the run to the cross-run index.
    ///
    /// The two are written together because a run that appears in one and not the other is a
    /// run a reader has to reconcile by hand: the run's own table is what a person reads, and
    /// the cross-run document is what CI resolves `latest_run` from.
    ///
    /// # Errors
    ///
    /// As [`RunJournal::write_index`] and [`RunIndex::write`].
    ///
    /// # Panics
    ///
    /// Never.
    pub fn publish(&self, run: &RunDir, results_root: &Path) -> Result<(), EvidenceError> {
        self.write_index(run)?;
        let mut index = RunIndex::open(results_root)?;
        index.record(self.entry(run));
        index.write()
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
#[derive(Clone, Debug)]
pub struct RunIndex {
    /// The file the document is read from and written to.
    path: PathBuf,
    /// The document itself.
    document: IndexDocument,
}

impl RunIndex {
    /// Opens the cross-run index under `results_root`, reading it when it is there.
    ///
    /// A results tree with no index yet is a tree whose first run has not finished, so a
    /// missing file opens an empty document. A file that is there and cannot be read is
    /// refused instead of replaced: it holds every run before this one.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::LegacyLayout`] when the root is a flat layout,
    /// [`EvidenceError::Index`] when the document is not one this build understands, and
    /// [`EvidenceError::Io`] when it is there and cannot be read.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn open(results_root: &Path) -> Result<Self, EvidenceError> {
        refuse_legacy_layout(results_root)?;
        let path = results_root.join(RESULTS_INDEX_FILE);
        let document = match std::fs::read_to_string(&path) {
            Ok(text) => read_document(&path, &text)?,
            Err(source) if source.kind() == ErrorKind::NotFound => IndexDocument::empty(),
            Err(source) => return Err(EvidenceError::Io { path, source }),
        };
        Ok(Self { path, document })
    }

    /// The file the index is read from and written to.
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
        self.document.latest_run.as_deref()
    }

    /// The runs the index holds, in the order they were recorded.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn runs(&self) -> &[RunEntry] {
        &self.document.runs
    }

    /// Records a run and moves the pointer to the newest run the index holds.
    ///
    /// A run already in the document is replaced rather than appended to, so a run published
    /// twice is one entry and not two. The pointer is the greatest run *name* the document
    /// holds rather than the one recorded last, so a run published late cannot move it
    /// backwards onto a finished run.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn record(&mut self, entry: RunEntry) {
        // The position is taken before anything is written: holding the mutable borrow a
        // `find_mut` would return across the match is the borrow the compiler cannot split.
        let position = self
            .document
            .runs
            .iter()
            .position(|held| held.run == entry.run);
        match position {
            Some(index) => self.document.runs[index] = entry,
            None => self.document.runs.push(entry),
        }
        let newest = self
            .document
            .runs
            .iter()
            .map(|held| held.run.as_str())
            .max()
            .map(str::to_owned);
        self.document.latest_run = newest;
    }

    /// Writes the document back to the results tree.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::Json`] when the document cannot be rendered, and
    /// [`EvidenceError::Io`] when the file cannot be written or its mode cannot be set.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn write(&self) -> Result<(), EvidenceError> {
        let text = error::encode(RESULTS_INDEX_FILE, &self.document)?;
        write_private(&self.path, text.as_bytes())
    }
}

/// The document `results/index.json` holds.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct IndexDocument {
    /// Version of the document format this build writes.
    format: u32,
    /// The newest run the document holds.
    latest_run: Option<String>,
    /// Every run, in the order it was recorded.
    runs: Vec<RunEntry>,
}

impl IndexDocument {
    /// The document a results tree without an index opens as.
    fn empty() -> Self {
        Self {
            format: INDEX_FORMAT_VERSION,
            latest_run: None,
            runs: Vec::new(),
        }
    }
}

/// Reads the cross-run document, refusing a format this build does not write.
///
/// # Errors
///
/// Returns [`EvidenceError::Index`] when the text is not the document this build understands.
///
/// # Panics
///
/// Never.
fn read_document(path: &Path, text: &str) -> Result<IndexDocument, EvidenceError> {
    let document: IndexDocument =
        serde_json::from_str(text).map_err(|error| EvidenceError::Index {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;
    if document.format != INDEX_FORMAT_VERSION {
        return Err(EvidenceError::Index {
            path: path.to_path_buf(),
            detail: format!(
                "it is written in format {} and this build writes {INDEX_FORMAT_VERSION}",
                document.format
            ),
        });
    }
    Ok(document)
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
