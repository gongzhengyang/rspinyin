//! The v3 assertion record and the batch checklist: the ledger `dev-check` reconciles against.
//!
//! Responsibility: hold the per-case record the tests v3.0 contract fixes -- the tier a case
//! belongs to, the dimensions it covers, the screenshots it owes, the budget keys its
//! thresholds come from, and the claims it settled -- and the batch-level checklist that says,
//! for every case of a run, which artefacts actually exist. The v1 records
//! ([`super::CaseEvidence`]) stay as they are: the v3 record rides beside them in the same
//! `assertions.json`, so a reader written for v1 keeps working and the new fields are additive.
//!
//! # The tier and the dimensions are the case matrix's vocabulary
//!
//! [`Tier`] is the suite ladder `docs/dev/test-architecture.md` fixes (`S`/`A`/`T`), and
//! [`Dim`] is the coverage legend `docs/dev/tests.md` §2 states (①–⑤). Both are enums rather
//! than strings so a case card that misspells one fails to compile here rather than publishing
//! a record whose coverage nobody can query.
//!
//! # What a claim may say
//!
//! A [`Claim`] carries plain strings because most claims are structural -- a count, a name, a
//! duration -- but the evidence archive is a privacy surface, and a claim's `expect` or
//! `actual` could carry what the user typed exactly the way any other string could. The rule is
//! the archive's own: a value that reached a claim through
//! [`EvidenceValue`](super::EvidenceValue) is already safe to copy verbatim (a fact is
//! structural, a withheld value is the redaction placeholder), and a claim built from any other
//! source applies the same judgement before the text lands here.
//!
//! # The checklist is the batch's evidence-existence table
//!
//! One row per case: the tier, the status, the claim counts, and -- the part a green summary
//! cannot be trusted without -- whether the artefacts the case owes are actually on disk. A
//! case card that declares screenshots which never landed is a *platform defect* and not a
//! case failure: the row is red, the batch's verdict becomes [`BatchVerdict::Flawed`], and the
//! missing names are listed in the row itself, but the case's own verdict is not rewritten.

use std::path::Path;

use super::error::{self, EvidenceError};
use super::{BatchVerdict, CHECKLIST_FILE, CaseStatus};

/// The suite tier a case belongs to, as `docs/dev/test-architecture.md` fixes the ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// The fast set every invocation runs first.
    Smoke,
    /// The full set, run serially on a real display.
    Acceptance,
    /// Soak, allocation and scale cards; machine-exclusive, never a CI gate.
    Stress,
}

impl Tier {
    /// The tier a case-card annotation such as `[Acceptance]` names.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "smoke" | "s" => Some(Self::Smoke),
            "acceptance" | "a" => Some(Self::Acceptance),
            "stress" | "t" => Some(Self::Stress),
            _ => None,
        }
    }

    /// The label a report prints.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::Smoke => "smoke",
            Self::Acceptance => "acceptance",
            Self::Stress => "stress",
        }
    }
}

/// One coverage dimension of the case matrix, as the legend `docs/dev/tests.md` states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Dim {
    /// Dimension ①: the core business path.
    #[serde(rename = "d1")]
    CorePath,
    /// Dimension ②: boundary and fault tolerance.
    #[serde(rename = "d2")]
    Boundary,
    /// Dimension ③: the all-state defence matrix.
    #[serde(rename = "d3")]
    StateDefense,
    /// Dimension ④: craft and performance resilience.
    #[serde(rename = "d4")]
    Craft,
    /// Dimension ⑤: ergonomics and control flow.
    #[serde(rename = "d5")]
    Ergonomics,
}

/// One claim a case settled: what was expected, what was measured, whether they agreed.
///
/// A `pass` of `false` also covers the "not verifiable on this machine" state, which must carry
/// the env-gate remedy text in `actual` -- the card contract fixes this, so a reader never has
/// to distinguish "the claim failed" from "this machine cannot judge it" by anything but the
/// text of `actual`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Claim {
    /// The claim's name, e.g. `first_candidate`.
    pub name: String,
    /// What was expected.
    pub expect: String,
    /// What was measured, or the remedy text when the machine cannot judge the claim.
    pub actual: String,
    /// Whether the claim held.
    pub pass: bool,
}

/// One case's structured assertion record, v3.
///
/// The tier/dims/evidence fields are the contract `dev-check` reconciles against the tests
/// v3.0 case cards; the budget keys name `budgets.json` entries instead of restating numbers,
/// so an assertion can never drift from its threshold source.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AssertionV3 {
    /// Stable case id, e.g. `TC-UI-07`.
    pub case: String,
    /// The suite tier the case belongs to.
    pub tier: Tier,
    /// Covered dimensions, a subset of `d1`–`d5`.
    ///
    /// Empty while the case table has not transcribed the matrix's dimension column; an empty
    /// subset is legal, and a reader that needs coverage joins this against the matrix rather
    /// than treating emptiness as a verdict.
    #[serde(default)]
    pub dims: Vec<Dim>,
    /// Whether the case owes PNG evidence (GUI plane) or a log dump.
    #[serde(default)]
    pub requires_screenshots: bool,
    /// The exact file names the evidence dir must contain.
    ///
    /// The set is checked against the case's own bundle directory when the run journal records
    /// the case: a name declared here and missing from the directory is the checklist's red
    /// row -- a platform defect, not a case failure.
    #[serde(default)]
    pub screenshots_expected: Vec<String>,
    /// `budgets.json` keys this case's thresholds come from (names only).
    #[serde(default)]
    pub budget_keys: Vec<String>,
    /// Per-claim verdicts: what was expected, what was measured.
    #[serde(default)]
    pub claims: Vec<Claim>,
    /// The golden-diff comparisons the case made: pixel counts and diff boxes.
    ///
    /// Empty for every case that judged nothing against a baseline, and readable by a
    /// reader written before the field existed -- the record rides under its own key, and
    /// an absent `diffs` means the same thing as an empty one.
    #[serde(default)]
    pub diffs: Vec<crate::testd::golden::DiffReport>,
}

impl AssertionV3 {
    /// The claims that did not hold, in the order the case made them.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn failed_claims(&self) -> impl Iterator<Item = &Claim> {
        self.claims.iter().filter(|claim| !claim.pass)
    }

    /// The declared screenshots that are not in `bundle_dir`.
    ///
    /// Nothing is written and nothing is created: the diff is read off the directory as it
    /// stands. A case whose record declares no screenshots has an empty diff by construction.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn missing_screenshots(&self, bundle_dir: &Path) -> Vec<String> {
        self.screenshots_expected
            .iter()
            .filter(|name| !bundle_dir.join(name.as_str()).exists())
            .cloned()
            .collect()
    }

    /// Renders the record as the JSON text the v3 section of `assertions.json` holds.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::Json`] when a field cannot be written as JSON.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn to_json(&self) -> Result<String, EvidenceError> {
        error::encode(super::ASSERTIONS_FILE, self)
    }

    /// Reads a record from the JSON text a bundle holds.
    ///
    /// Damaged input is refused as a value, never unwrapped through: a record a reader cannot
    /// parse is an [`EvidenceError::Json`] naming the document, and the caller decides what a
    /// damaged record means -- the archive never guesses.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::Json`] when the text is not a record this build understands.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_json(text: &str) -> Result<Self, EvidenceError> {
        serde_json::from_str(text).map_err(|error| EvidenceError::Json {
            document: super::ASSERTIONS_FILE.to_owned(),
            detail: error.to_string(),
        })
    }
}

/// What the batch checklist says about one case.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChecklistRow {
    /// The module code the case belongs to.
    pub module: String,
    /// The case's identifier.
    pub case: String,
    /// The tier the case's v3 record declares, or `None` when it carried none.
    pub tier: Option<Tier>,
    /// How the case ended.
    pub status: CaseStatus,
    /// How many claims the case settled.
    pub claims: usize,
    /// How many of them did not hold.
    pub failed_claims: usize,
    /// Whether the verdict record was written.
    pub assertions: bool,
    /// Whether the failure trace was written, for the cases that owe one.
    pub trace: bool,
    /// How many screenshots the case's record declares.
    pub screenshots_expected: usize,
    /// How many of them are on disk.
    pub screenshots_present: usize,
    /// The declared names that are not on disk: the red row's content.
    pub screenshots_missing: Vec<String>,
    /// The bundle write's own reason, when the bundle could not be written.
    pub gap: Option<String>,
    /// The gate's withheld state the case was recorded under, when it was skipped.
    ///
    /// `pending-impl` or `needs-environment` -- the eligibility claim's own prefix -- so a
    /// report can tell a case the implementation has not reached from one this machine
    /// cannot judge. Absent for every case that ran, and for checklists written before the
    /// field existed.
    #[serde(default)]
    pub withheld: Option<String>,
    /// The coverage dimensions the case's v3 record declares.
    #[serde(default)]
    pub dims: Vec<Dim>,
    /// The budget keys the case's v3 record declares its thresholds from (names only).
    #[serde(default)]
    pub budget_keys: Vec<String>,
}

impl ChecklistRow {
    /// Whether every artefact the case owes is accounted for.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_complete(&self) -> bool {
        self.gap.is_none() && self.screenshots_missing.is_empty()
    }

    /// The label a report prints for the row's evidence state.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn evidence_label(&self) -> String {
        if let Some(reason) = &self.gap {
            return format!("gap: {reason}");
        }
        if self.screenshots_missing.is_empty() {
            return String::from("complete");
        }
        format!("missing: {}", self.screenshots_missing.join(", "))
    }
}

/// The batch-level checklist, written as `checklist.json` beside the run's `index.md`.
///
/// The generator owns no state of its own: a checklist is built from the run journal's rows,
/// and a report can always be regenerated from the run directory by reading this document
/// back.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Checklist {
    /// Version of the checklist format this build writes.
    pub format: u32,
    /// The run's directory name, e.g. `run-20260930-014455`.
    pub run: String,
    /// What the batch came to.
    pub verdict: BatchVerdict,
    /// One row per case, in the order the cases were recorded.
    pub rows: Vec<ChecklistRow>,
}

impl Checklist {
    /// The checklist of a run, from its name and the rows the journal recorded.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn of(run: &str, verdict: BatchVerdict, rows: Vec<ChecklistRow>) -> Self {
        Self {
            format: super::CHECKLIST_FORMAT_VERSION,
            run: run.to_owned(),
            verdict,
            rows,
        }
    }

    /// The rows that are not complete: the platform defects the batch has to answer for.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn red_rows(&self) -> impl Iterator<Item = &ChecklistRow> {
        self.rows.iter().filter(|row| !row.is_complete())
    }

    /// Renders the checklist as the JSON text `checklist.json` holds.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::Json`] when a field cannot be written as JSON.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn to_json(&self) -> Result<String, EvidenceError> {
        error::encode(CHECKLIST_FILE, self)
    }

    /// Reads a checklist from the JSON text a run directory holds.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::Json`] when the text is not a checklist this build understands.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_json(text: &str) -> Result<Self, EvidenceError> {
        serde_json::from_str(text).map_err(|error| EvidenceError::Json {
            document: CHECKLIST_FILE.to_owned(),
            detail: error.to_string(),
        })
    }
}
