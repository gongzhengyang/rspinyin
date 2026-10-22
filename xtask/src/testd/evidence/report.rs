//! The batch report and the cross-run registry: what a finished run tells the executor.
//!
//! Responsibility: render the batch-level report a person and a CI job read first -- who
//! passed, who was withheld and why, which budget keys a failure touched, which defects
//! stand between this batch and green -- and register a finished run in the cross-batch
//! registry `dev-check` resumes from. Both read only what the run directory and the
//! registry already hold: the report is regenerated from the checklist the ledger wrote,
//! and the registry is appended to, so neither owns state that could drift from the
//! evidence it describes.
//!
//! # The registry is append-only and line-tolerant
//!
//! `results/index.json` is one [`RunEntry`] per line. Registration appends a line and
//! never rewrites the file, so a batch that publishes twice leaves the earlier line in
//! place and a reader takes the later one. Reading skips a line it cannot parse with a
//! warning instead of refusing the whole registry: one damaged line is one lost entry,
//! not a lost history, and the file the bad line sits in is left exactly as it was found.
//! A first line written by the earlier *document* format (a versioned object holding every
//! run) is adopted rather than skipped, so a registry an older build wrote keeps its
//! history.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use super::{
    CaseStatus, Checklist, ChecklistRow, Dim, EvidenceError, RESULTS_DIR, RESULTS_INDEX_FILE,
    RUNS_DIR, RunEntry, TRACE_FILE, Tier,
};

/// The name the run's batch report is written as.
pub const FINAL_REPORT_FILE: &str = "final-report.md";

/// One case's verdict, as the batch report holds it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CaseVerdict {
    /// The suite tier the case's record declares, or `None` when it declared none.
    pub tier: Option<Tier>,
    /// The coverage dimensions the case's record declares.
    pub dims: Vec<Dim>,
    /// How the case came out.
    pub verdict: CaseOutcome,
}

/// How one case came out, in the report's own vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CaseOutcome {
    /// The case ran and held.
    Pass,
    /// The case ran and did not hold.
    Fail,
    /// The implementation behind the case is not merged yet; the case card anchors it to
    /// the tasks that owe the subject.
    PendingImpl,
    /// This machine could not judge the case -- the environment it needs is absent, or the
    /// evidence it owed could not be written.
    Unverifiable,
}

impl CaseOutcome {
    /// The outcome one checklist row lands as.
    ///
    /// A skipped case is `PendingImpl` by default -- the gate withheld it before it ran --
    /// and `Unverifiable` when its eligibility claim says the environment, not the
    /// implementation, is what is missing.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn of(row: &ChecklistRow) -> Self {
        match row.status {
            CaseStatus::Pass => Self::Pass,
            CaseStatus::Fail => Self::Fail,
            CaseStatus::Flawed => Self::Unverifiable,
            CaseStatus::Skipped => match row.withheld.as_deref() {
                Some("needs-environment") => Self::Unverifiable,
                _ => Self::PendingImpl,
            },
        }
    }

    /// The word the report prints.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pass => "通过",
            Self::Fail => "失败",
            Self::PendingImpl => "待实现",
            Self::Unverifiable => "无法验证",
        }
    }
}

/// One honest blocker: an isolated defect a reader goes and looks at, or a platform defect
/// the batch has to answer for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blocker {
    /// The case the blocker belongs to.
    pub case: String,
    /// What went wrong, in the checklist's own words.
    pub reason: String,
    /// The failure trace's path, relative to the repository root, for the failing cases
    /// that wrote one.
    pub trace: Option<String>,
}

/// The batch-level report the executor and humans read first.
///
/// It renders from the checklist the ledger already wrote -- the generator owns no state of
/// its own, so a report can always be regenerated from the run directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalReport {
    /// Case id -> what the case came out as.
    pub verdicts: BTreeMap<String, CaseVerdict>,
    /// Names-only list of budget keys any failing case touched.
    pub budget_violations: Vec<String>,
    /// Honest blockers: isolated defects with their trace paths, and the platform defects
    /// a red checklist row records.
    pub blockers: Vec<Blocker>,
}

impl FinalReport {
    /// Derives the report from the checklist a run published.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn of(checklist: &Checklist) -> Self {
        let mut verdicts = BTreeMap::new();
        let mut budget_violations: Vec<String> = Vec::new();
        let mut blockers: Vec<Blocker> = Vec::new();
        for row in &checklist.rows {
            let outcome = CaseOutcome::of(row);
            verdicts.insert(
                row.case.clone(),
                CaseVerdict {
                    tier: row.tier,
                    dims: row.dims.clone(),
                    verdict: outcome,
                },
            );
            if outcome == CaseOutcome::Fail {
                for key in &row.budget_keys {
                    if !budget_violations.contains(key) {
                        budget_violations.push(key.clone());
                    }
                }
                blockers.push(Blocker {
                    case: row.case.clone(),
                    reason: format!("{}/{} 条 claim 未成立", row.failed_claims, row.claims),
                    trace: row.trace.then(|| {
                        format!(
                            "{RESULTS_DIR}/{RUNS_DIR}/{}/{}/{}/{}",
                            checklist.run,
                            cell(&row.module),
                            cell(&row.case),
                            TRACE_FILE
                        )
                    }),
                });
            }
            if !row.is_complete() {
                // A red row is a *platform defect* -- evidence the card declared and the
                // harness never wrote -- and it is the batch's answer, not the case's.
                blockers.push(Blocker {
                    case: row.case.clone(),
                    reason: format!("证据缺口：{}", row.evidence_label()),
                    trace: None,
                });
            }
        }
        budget_violations.sort();
        Self {
            verdicts,
            budget_violations,
            blockers,
        }
    }

    /// Renders the report as the markdown a person reads.
    ///
    /// The report is written in Chinese, the project's documentation language; case ids,
    /// budget keys and paths stay as the ledger wrote them.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn render(&self) -> String {
        let passed = self
            .verdicts
            .values()
            .filter(|verdict| verdict.verdict == CaseOutcome::Pass)
            .count();
        let failed = self
            .verdicts
            .values()
            .filter(|verdict| verdict.verdict == CaseOutcome::Fail)
            .count();
        let pending = self
            .verdicts
            .values()
            .filter(|verdict| verdict.verdict == CaseOutcome::PendingImpl)
            .count();
        let unverifiable = self.verdicts.len() - passed - failed - pending;
        let mut text = String::new();
        text.push_str("# 批次报告\n\n");
        text.push_str(&format!(
            "- 用例：{}（通过 {passed}，失败 {failed}，待实现 {pending}，无法验证 \
             {unverifiable}）\n",
            self.verdicts.len()
        ));
        text.push_str(&format!(
            "- 预算违例：{}\n",
            if self.budget_violations.is_empty() {
                String::from("无")
            } else {
                self.budget_violations.join("、")
            }
        ));
        text.push_str(&format!(
            "- 阻塞项：{}\n",
            if self.blockers.is_empty() {
                String::from("无")
            } else {
                format!("{}", self.blockers.len())
            }
        ));
        text.push_str("\n| 用例 | 层级 | 维度 | 判定 |\n| --- | --- | --- | --- |\n");
        for (case, verdict) in &self.verdicts {
            let dims = if verdict.dims.is_empty() {
                String::from("-")
            } else {
                verdict
                    .dims
                    .iter()
                    .map(|dim| dim_name(*dim))
                    .collect::<Vec<_>>()
                    .join("、")
            };
            text.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                cell(case),
                verdict.tier.map(tier_label).unwrap_or("-"),
                dims,
                verdict.verdict.label()
            ));
        }
        if !self.blockers.is_empty() {
            text.push_str("\n## 阻塞项\n\n");
            for blocker in &self.blockers {
                let trace = match &blocker.trace {
                    Some(path) => format!("，trace：`{path}`"),
                    None => String::new(),
                };
                text.push_str(&format!(
                    "- {}：{}{}\n",
                    cell(&blocker.case),
                    blocker.reason,
                    trace
                ));
            }
        }
        text
    }
}

/// Reads a tier as the report prints it.
///
/// # Panics
///
/// Never.
fn tier_label(tier: Tier) -> &'static str {
    match tier {
        Tier::Smoke => "Smoke",
        Tier::Acceptance => "Acceptance",
        Tier::Stress => "Stress",
    }
}

/// Reads a dimension as the report prints it, in the legend the case matrix states.
///
/// # Panics
///
/// Never.
fn dim_name(dim: Dim) -> &'static str {
    match dim {
        Dim::CorePath => "①",
        Dim::Boundary => "②",
        Dim::StateDefense => "③",
        Dim::Craft => "④",
        Dim::Ergonomics => "⑤",
    }
}

/// One cell of the report's table, with the characters a table cell cannot carry removed.
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

/// Registers a finished run in the cross-batch registry.
///
/// The registry is append-only: the entry lands as one more line and the bytes already
/// there are never rewritten, so a registry holding a line this build cannot parse keeps
/// everything it had. One shape is the exception -- a registry an older build wrote as a
/// single versioned *document* is migrated once, its every entry carried over as lines
/// beside the new one, because a document has no lines to append after. The file is the
/// owner's alone (`0600`), the same discipline the rest of the evidence tree keeps.
///
/// # Errors
///
/// Returns [`EvidenceError::Io`] when the registry cannot be opened, appended to, or its
/// mode cannot be set, and [`EvidenceError::Json`] when the entry cannot be rendered.
///
/// # Panics
///
/// Never.
pub fn register_run(index_path: &Path, entry: RunEntry) -> Result<(), EvidenceError> {
    if let Some(parent) = index_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|source| EvidenceError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let line = serde_json::to_string(&entry).map_err(|error| EvidenceError::Json {
        document: RESULTS_INDEX_FILE.to_owned(),
        detail: error.to_string(),
    })?;
    // A legacy document has no lines to append after -- and a document and a line glued
    // together would be unreadable to both formats -- so it is migrated in place: its runs
    // carried over as lines, the new entry beside them. Every entry survives; only the
    // wrapper changes.
    if let Some(runs) = legacy_document(index_path)? {
        let mut text = String::new();
        for held in runs {
            text.push_str(
                &serde_json::to_string(&held).map_err(|error| EvidenceError::Json {
                    document: RESULTS_INDEX_FILE.to_owned(),
                    detail: error.to_string(),
                })?,
            );
            text.push('\n');
        }
        text.push_str(&line);
        text.push('\n');
        super::write::write_private(index_path, text.as_bytes())?;
        return Ok(());
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(index_path)
        .map_err(|source| EvidenceError::Io {
            path: index_path.to_path_buf(),
            source,
        })?;
    writeln!(file, "{line}").map_err(|source| EvidenceError::Io {
        path: index_path.to_path_buf(),
        source,
    })?;
    // The mode is set after the write the same way the rest of the tree does it: a file
    // that already exists keeps its mode, and one created here gets the owner-only shape.
    std::fs::set_permissions(index_path, std::fs::Permissions::from_mode(0o600)).map_err(
        |source| EvidenceError::Io {
            path: index_path.to_path_buf(),
            source,
        },
    )?;
    Ok(())
}

/// Reads every run the registry holds, skipping the lines it cannot parse.
///
/// A damaged line costs one warning on stderr and nothing else: the entries around it are
/// returned, and the registry itself is left exactly as it was found. The first line of a
/// registry an older build wrote -- a versioned document holding every run in one object --
/// is adopted instead of skipped, so that history survives the format change.
///
/// # Errors
///
/// Returns [`EvidenceError::Io`] when the registry exists and cannot be read. A registry
/// that is not there yet is a registry whose first run has not finished, and reads as
/// empty.
///
/// # Panics
///
/// Never.
pub fn read_runs(index_path: &Path) -> Result<Vec<RunEntry>, EvidenceError> {
    let text = match std::fs::read_to_string(index_path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(EvidenceError::Io {
                path: index_path.to_path_buf(),
                source,
            });
        }
    };
    // The earlier format is one pretty-printed document, many lines long: no line of it
    // parses on its own, so the whole text is tried as a document before it is split.
    if let Some(document) = whole_text_document(&text) {
        return Ok(document.runs);
    }
    let mut entries = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<RunEntry>(line) {
            entries.push(entry);
            continue;
        }
        // A single-line document, as an older build may have written one: adopted, not
        // skipped, so the runs it held stay in the registry this build reads.
        if let Ok(document) = serde_json::from_str::<AdoptedDocument>(line) {
            entries.extend(document.runs);
            continue;
        }
        eprintln!(
            "{RESULTS_INDEX_FILE}: line {} skipped, it is neither a run entry nor a document \
             this build reads",
            number + 1
        );
    }
    Ok(entries)
}

/// The legacy document a whole registry text holds, when it holds one.
///
/// A document is detected, never guessed: the text must parse as one, carrying the `format`
/// field a run entry never has. `None` means the text is line-oriented, or is damage the
/// line reader will warn about line by line.
///
/// # Panics
///
/// Never.
fn whole_text_document(text: &str) -> Option<AdoptedDocument> {
    serde_json::from_str(text).ok()
}

/// The registry document the earlier build wrote, as this build adopts it.
#[derive(serde::Deserialize)]
struct AdoptedDocument {
    /// The format the document was written in; present in every document, in no run entry.
    #[allow(dead_code)]
    format: u32,
    /// Every run the document held.
    #[serde(default)]
    runs: Vec<RunEntry>,
}

/// Reads the legacy document a registry file holds, when its whole text is one.
///
/// # Errors
///
/// Returns [`EvidenceError::Io`] when the file cannot be read. A file that is not there,
/// or one that is line-oriented, answers `None`.
///
/// # Panics
///
/// Never.
fn legacy_document(index_path: &Path) -> Result<Option<Vec<RunEntry>>, EvidenceError> {
    match std::fs::read_to_string(index_path) {
        Ok(text) => Ok(whole_text_document(&text).map(|document| document.runs)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(EvidenceError::Io {
            path: index_path.to_path_buf(),
            source,
        }),
    }
}

/// Deduplicates the registry's entries, keeping the last line that named each run.
///
/// A run that was published twice is one entry and not two, and the one a reader sees is
/// the line appended later -- the re-publication is the more current statement about the
/// run.
///
/// # Panics
///
/// Never.
pub fn dedupe_runs(entries: Vec<RunEntry>) -> Vec<RunEntry> {
    let mut order: Vec<String> = Vec::new();
    let mut by_name: BTreeMap<String, RunEntry> = BTreeMap::new();
    for entry in entries {
        if !order.contains(&entry.run) {
            order.push(entry.run.clone());
        }
        by_name.insert(entry.run.clone(), entry);
    }
    order
        .into_iter()
        .filter_map(|name| by_name.remove(&name))
        .collect()
}

/// The path the registry lives at, under the results tree.
///
/// # Panics
///
/// Never.
pub fn registry_path(results_root: &Path) -> PathBuf {
    results_root.join(RESULTS_INDEX_FILE)
}
