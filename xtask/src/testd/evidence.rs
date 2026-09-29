//! The evidence archive: what a case leaves behind, and where a reviewer finds it.
//!
//! Responsibility: give a run one directory tree, give each of its cases one directory inside
//! that tree, write the documents that say what the case found, and keep the two indexes that
//! let a person -- or a CI job -- find the newest run without walking the tree. The layout is
//! the one `features-test.md` fixes and is not this module's to change:
//!
//! ```text
//! results/
//!   runs/run-YYYYMMDD-HHMMSS/        the run, named for the instant it started
//!     index.md                       case -> evidence, one row per case
//!     <module>/<TC-ID>/              one directory per case
//!       01_default.png               written by the screenshot channel
//!       assertions.json              always: the verdict and what it rests on
//!       trace.json                   only for a failed or flawed case
//!       heal.patch                   only when the case healed something
//!   index.json                       every run, plus the `latest_run` pointer
//! ```
//!
//! Everything a case knows about itself is inside its own directory, which is the property the
//! layout exists for: a reviewer reading a failure opens one directory and finds the verdict,
//! the pixels, the trace, the log tail and the machine it all happened on. Nothing sends them
//! to a second tree.
//!
//! # The flat layouts are refused, not migrated
//!
//! `results/screenshots/` and `results/traces/` are the shapes an earlier harness wrote, and
//! `dev-check` treats anything it finds there as a legacy artefact to be archived. Writing into
//! them would produce a bundle that the report layer files somewhere else, so every path this
//! module builds is checked and a legacy one is refused -- see [`LEGACY_SUBDIRS`].
//!
//! # The archive is a privacy surface
//!
//! A bundle is written to disk and may be attached to a bug report, so the rules that keep
//! user content out of the log hold here too, and for a sharper reason: a log line is read by
//! the person who wrote it, while a bundle travels. Two mechanisms carry the rule rather than a
//! comment asking for it:
//!
//! * [`EvidenceValue`] is the only way an observed value reaches a document, and its
//!   [`withheld`](EvidenceValue::withheld) constructor records the *length* of a value and
//!   nothing else, in the same placeholder form the log's own redaction writes.
//! * [`LogExcerpt`] is taken from a [`LogTap`], never from a string a caller assembled, so the
//!   lines in a trace are exactly the lines the log's redaction decided to write.
//!
//! What is left is structural: counts, names, colours, durations, error codes, and the
//! environment facts -- a display server, a compositor, a version. An application identifier is
//! not among them and must not be added: the log records it as a hash, and a case that records
//! it at all records the same hash.
//!
//! # Modes
//!
//! Every directory this module creates is `0700` and every file it writes is `0600`, and an
//! existing file with wider permissions is corrected rather than left alone. A bundle that any
//! other account on the machine can read is a defect, not a default.
//!
//! # A bundle that cannot be written
//!
//! Writing fails loudly and never panics: every failure is an [`EvidenceError`] a caller can
//! act on, and the failure of one case's bundle does not stop the run. What it does do is
//! become an evidence gap the batch reports -- [`RunJournal::record`] takes the result of a
//! write, not the write's success, so a case whose bundle was lost is still a row in the index
//! and still visible in the verdict. Losing the evidence is a second, reportable failure.
//!
//! # What this module never does
//!
//! It writes no pixels: the PNGs are the screenshot channel's, and this module only hands out
//! the path they belong at, through [`RunDir::snapshot_path`]. It decides no verdict: the
//! status a case reached arrives as a value. It reads no clock except through [`Utc`], opens no
//! socket and starts no process, so its tests run with no display server and no Fcitx5 session.
//!
//! # Modules
//!
//! [`write`] owns the tree -- the run directory, the case bundles and the file modes -- and
//! [`index`] owns the two indexes and the batch verdict. [`error`] holds the refusals.

// The archive is exercised by the tests below and by nothing else yet: the case runner that
// would write a bundle and the subcommand tree that would publish a run both live in files this
// module does not own. Until that wiring lands, every item here is reported as dead code in a
// non-test build, and the attribute goes away with those lines.
//
// `unused_imports` is covered by the same reasoning and for the same reason: the `pub use` lines
// below are this module's surface, and a `pub use` in a *binary* crate is "unused" whenever
// nothing in the crate names it.
#![allow(dead_code, unused_imports)]

mod error;
mod index;
mod write;

#[cfg(test)]
mod tests;

pub use self::error::EvidenceError;
pub use self::index::{BatchSummary, BatchVerdict, CaseRow, RunEntry, RunIndex, RunJournal};
pub use self::write::{CaseBundle, RunDir, refuse_legacy_layout};

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use ime_diag::redact::{redaction_placeholder, shorten_home};

use crate::testd::logs::LogTap;

/// Directory the whole evidence tree lives in, relative to the repository root.
pub const RESULTS_DIR: &str = "results";

/// Directory one run's own tree lives in, under [`RESULTS_DIR`].
pub const RUNS_DIR: &str = "runs";

/// Prefix of a run directory's name; the rest is the instant the run started.
pub const RUN_PREFIX: &str = "run-";

/// Name of the run's case index, directly inside the run's directory.
pub const RUN_INDEX_FILE: &str = "index.md";

/// Name of the cross-run index, directly under [`RESULTS_DIR`].
pub const RESULTS_INDEX_FILE: &str = "index.json";

/// Name of a case's verdict record.
pub const ASSERTIONS_FILE: &str = "assertions.json";

/// Name of a case's failure trace.
pub const TRACE_FILE: &str = "trace.json";

/// Name of the patch a healed case leaves behind.
pub const HEAL_FILE: &str = "heal.patch";

/// The flat layouts under [`RESULTS_DIR`] that `dev-check` files as legacy artefacts.
///
/// A bundle written into one of them is a bundle the report layer will move, so a path that
/// names one is refused rather than accepted and tidied later.
pub const LEGACY_SUBDIRS: [&str; 2] = ["screenshots", "traces"];

/// How many log lines a failure archives.
///
/// A failure needs the end of the log -- what the writer said just before it went wrong -- and
/// not its history, and a trace that carried every line a long case drained would be a document
/// nobody reads. [`LogExcerpt::total_lines`] keeps the count that the tail leaves out.
pub const LOG_EXCERPT_LINES: usize = 200;

/// Version of the cross-run index this build writes and reads.
pub const INDEX_FORMAT_VERSION: u32 = 1;

/// A UTC instant, as the evidence layout writes it.
///
/// The layout names a run by the instant it started and stamps every document with the instant
/// its case began, so this type exists to render one number two ways. The calendar arithmetic
/// behind it is the only such code in the module: the workspace has no date crate, and adding
/// one to produce two fixed formats would be the larger change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Utc {
    /// Seconds since 1970-01-01T00:00:00Z, which is how the instant is held.
    seconds: i64,
}

impl Utc {
    /// The instant `seconds` seconds after the Unix epoch.
    ///
    /// A negative count names an instant before the epoch and is rendered as one; it is not
    /// clamped, because the only caller that could pass one is a test.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_unix_seconds(seconds: i64) -> Self {
        Self { seconds }
    }

    /// The instant `time` names, read to the second.
    ///
    /// A system clock that reads before the epoch is recorded as the epoch: the alternative
    /// would be a negative year in a directory name, and no run has ever started before 1970.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn of(time: SystemTime) -> Self {
        let seconds = time
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs().min(i64::MAX as u64) as i64);
        Self { seconds }
    }

    /// The instant this process's clock reads now.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn now() -> Self {
        Self::of(SystemTime::now())
    }

    /// The instant as a Unix second count.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn unix_seconds(self) -> i64 {
        self.seconds
    }

    /// The instant as `YYYYMMDD-HHMMSS`, the form a run directory is named from.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn compact(self) -> String {
        let (year, month, day, hour, minute, second) = fields(self.seconds);
        format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
    }

    /// The instant as `YYYY-MM-DDTHH:MM:SSZ`, the form the documents carry.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn iso8601(self) -> String {
        let (year, month, day, hour, minute, second) = fields(self.seconds);
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
    }

    /// The name of the run that starts at this instant: `run-YYYYMMDD-HHMMSS`.
    ///
    /// The name sorts in the order runs started, which is what lets the cross-run index pick
    /// the newest of two without reading a clock of its own.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn run_name(self) -> String {
        format!("{RUN_PREFIX}{}", self.compact())
    }
}

impl serde::Serialize for Utc {
    /// Writes the instant as the ISO-8601 text the documents hold.
    ///
    /// # Errors
    ///
    /// Returns the serializer's own error, which a string can only raise for a format that
    /// cannot carry one.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.iso8601())
    }
}

/// Splits a Unix second count into UTC date and time fields.
///
/// The conversion is Howard Hinnant's `civil_from_days`, which is exact for every date a
/// 64-bit second count can name. The year is shifted to begin in March, which puts a leap day
/// at the end of the shifted year and leaves the day-of-year arithmetic with no special case.
///
/// # Panics
///
/// Never.
fn fields(seconds: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (rest / 3_600, rest % 3_600 / 60, rest % 60);
    (year, month, day, hour as u32, minute as u32, second as u32)
}

/// The civil date `days` days after 1970-01-01, as `(year, month, day)`.
///
/// # Panics
///
/// Never.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Days from 0000-03-01, the start of the shifted era the algorithm counts in.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month as u32, day as u32)
}

/// How a case ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CaseStatus {
    /// Every assertion held.
    Pass,
    /// An assertion did not hold.
    Fail,
    /// The case could not be judged, or part of its evidence could not be written.
    Flawed,
}

impl CaseStatus {
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

    /// Whether a case that ended this way has to leave a `trace.json` behind.
    ///
    /// A passing case leaves none, which is what keeps the archive free of the empty
    /// placeholders a reader would have to learn to skip.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn needs_trace(self) -> bool {
        !matches!(self, Self::Pass)
    }
}

/// One side of an assertion: the value a case compared.
///
/// The archive is a privacy surface -- it is written to disk and may be attached to a bug
/// report -- so a value that could carry what the user typed never reaches the file:
/// [`EvidenceValue::withheld`] records the *length* of such a value and nothing else, in the
/// same placeholder form the log's own redaction writes. The distinction is a type rather than
/// a comment because the mistake would otherwise be silent: a candidate's text looks exactly
/// like any other string.
///
/// It is written to the document as a plain string, so a reader sees
/// `"expected": "<redacted:len=2>"` where a case withheld a value.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(transparent)]
pub struct EvidenceValue(String);

impl EvidenceValue {
    /// A structural value: a count, a name, a colour, a coordinate, a duration.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn fact(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// A value that may carry what the user typed or what the window showed.
    ///
    /// The text is counted and dropped; what is stored is the placeholder the log writes for a
    /// withheld field, so the two archives say the same thing about the same value.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn withheld(value: &str) -> Self {
        Self(redaction_placeholder(value.chars().count()))
    }

    /// The text as it will be written.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The verdict of one visual audit item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// The item measured what the specification states.
    Pass,
    /// It did not.
    Fail,
    /// It could not be judged on this machine.
    ///
    /// The third state exists because a case whose subject is a real compositor, a multi-head
    /// desktop or an eight-hour soak has no verdict on a machine that offers none of them, and
    /// recording it as a pass is the one outcome the gate must never accept.
    Unverifiable,
}

/// One assertion a case made and what it saw.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct AssertionRecord {
    /// The assertion's name, e.g. `first_candidate`.
    pub name: String,
    /// What the case expected.
    pub expected: EvidenceValue,
    /// What it saw.
    pub actual: EvidenceValue,
    /// Whether the two agreed.
    pub ok: bool,
}

/// One item of the visual audit: what the specification states and what the window measured.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct VisualRecord {
    /// The item, named as the specification's own table names it.
    pub item: String,
    /// The value the specification fixes.
    pub expected: EvidenceValue,
    /// The value the audit measured.
    pub measured: EvidenceValue,
    /// What the comparison came to.
    pub verdict: Verdict,
}

/// One self-healing edit a case made.
///
/// The three fields are identifiers and a citation -- a constant's old name, its new name and
/// the line of the specification that settles the question. Nothing here is a value the user
/// typed, which is why these are plain strings where an observed value is an [`EvidenceValue`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct HealRecord {
    /// The name the case used before the heal.
    pub old: String,
    /// The name the specification states.
    pub new: String,
    /// Where the specification states it, e.g. `features.md 3.1`.
    pub basis: String,
}

/// One fact about the machine a case ran on.
///
/// The facts are structural -- a display server, a compositor, a version, a count -- and the one
/// a case must be careful with is the application under test: the log records an application
/// identifier as a hash and never as a name, and a fact that names it must carry the same hash.
/// The archive is attached to bug reports, so a window title or a program name is as much a
/// privacy leak here as a candidate's text is.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct EnvironmentFact {
    /// The fact's name, e.g. `display_server`.
    pub key: String,
    /// What it read, e.g. `x11`.
    pub value: String,
}

impl EnvironmentFact {
    /// Reads one `key: value` line of the environment report.
    ///
    /// The report is published as lines by the environment channel, and a line without the
    /// separator keeps the whole of itself as the key with an empty value: a fact that arrived
    /// in a shape this does not recognise is still recorded, because a reader can act on a
    /// strange key and cannot act on a line that was dropped.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn of_line(line: &str) -> Self {
        match line.split_once(": ") {
            Some((key, value)) => Self {
                key: key.trim().to_owned(),
                value: value.trim().to_owned(),
            },
            None => Self {
                key: line.trim().to_owned(),
                value: String::new(),
            },
        }
    }
}

/// One case's verdict, as `assertions.json` holds it.
///
/// The document is written for every case, passing or not, because it is the only record that
/// a case ran at all: a bundle with no verdict file would be indistinguishable from a case the
/// run never reached.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CaseEvidence {
    /// The case's identifier, e.g. `TC-CORE-01`.
    pub tc: String,
    /// The module code the case belongs to, e.g. `core`.
    pub module: String,
    /// How the case ended.
    pub status: CaseStatus,
    /// The instant the case began.
    pub started_at: Utc,
    /// How long it took, in milliseconds.
    pub duration_ms: u64,
    /// The assertions the case made, in the order it made them.
    pub assertions: Vec<AssertionRecord>,
    /// What the visual audit found, item by item.
    pub visual: Vec<VisualRecord>,
    /// What the case healed, if it healed anything.
    pub healed: Vec<HealRecord>,
    /// The machine the case ran on.
    ///
    /// The facts are recorded because a verdict is only valid on the machine that produced it:
    /// a budget number taken beside a running agent is worthless, and the same report says how
    /// many were running. They come from the environment channel's own lines rather than from
    /// a second reading of the environment, so the two agree.
    pub environment: Vec<EnvironmentFact>,
}

impl CaseEvidence {
    /// The assertions that did not hold, in the order the case made them.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn failed_assertions(&self) -> Vec<&AssertionRecord> {
        self.assertions
            .iter()
            .filter(|assertion| !assertion.ok)
            .collect()
    }

    /// How many assertions the case made.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn assertion_count(&self) -> usize {
        self.assertions.len()
    }

    /// Renders the document as the JSON text `assertions.json` holds.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::Json`] when a field cannot be written as JSON. The fields are
    /// all strings, counts and booleans, so this is the encoder's own refusal rather than one
    /// this module expects to see.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn to_json(&self) -> Result<String, EvidenceError> {
        error::encode(ASSERTIONS_FILE, self)
    }
}

/// One assertion that did not hold.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct AssertionDiff {
    /// The assertion's name.
    pub name: String,
    /// What the case expected.
    pub expected: EvidenceValue,
    /// What it saw.
    pub actual: EvidenceValue,
}

/// A rectangle in an image's own pixels.
///
/// This is the evidence archive's own rectangle and not the frame mirror's view of one: a
/// defect's rectangle locates a region of a snapshot, so its units are the snapshot's pixels,
/// while the mirror's rectangle is stated in screen physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct DefectRect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: u32,
    /// Height.
    pub h: u32,
}

/// Where an image was wrong.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ImageDefect {
    /// The audit item the defect belongs to, e.g. `corner_radius`.
    pub item: String,
    /// The snapshot the defect is in, named relative to the case's own directory.
    pub image: String,
    /// The region the defect occupies.
    pub rect: DefectRect,
    /// What the specification states for the item.
    pub expected: EvidenceValue,
    /// What the pixels hold.
    pub measured: EvidenceValue,
}

/// The tail of the log a case read, as a trace holds it.
///
/// It is taken from a [`LogTap`] rather than from lines a caller assembled, which is what makes
/// the excerpt exactly the text the log's own redaction decided to write: a caller cannot
/// substitute a line it kept somewhere else, and a line the tap never handed out cannot appear.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct LogExcerpt {
    /// The log file the lines came from, or `None` when the case read no log at all.
    pub path: Option<String>,
    /// How many lines the tap had handed out, including the ones the tail leaves out.
    pub total_lines: usize,
    /// The last [`LOG_EXCERPT_LINES`] lines, oldest first.
    pub lines: Vec<String>,
}

impl LogExcerpt {
    /// Takes the excerpt from the tap a case opened.
    ///
    /// `home` is the directory whose leading prefix the path is rewritten to `~` in, or `None`
    /// to record the path as it is. It is the caller's rather than this module's reading of the
    /// environment so that a bundle built in a test describes the same machine whatever the
    /// operator's `$HOME` happens to be.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn of(tap: &LogTap, home: Option<&Path>) -> Self {
        let path = tap.path().to_string_lossy();
        let path = match home {
            Some(home) => shorten_home(&path, home).into_owned(),
            None => path.into_owned(),
        };
        let drained = tap.drained();
        let kept = drained.len().saturating_sub(LOG_EXCERPT_LINES);
        Self {
            path: Some(path),
            total_lines: drained.len(),
            lines: drained[kept..].to_vec(),
        }
    }

    /// An excerpt for a case that read no log.
    ///
    /// A trace always carries the section, so a reader never has to tell "no log was read"
    /// from "the field was left out", and a case that ran in-process records the former.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn empty() -> Self {
        Self {
            path: None,
            total_lines: 0,
            lines: Vec::new(),
        }
    }
}

/// Why a case failed, as `trace.json` holds it.
///
/// The file exists only for a failed or flawed case. Everything it holds is what a reader needs
/// to reproduce the failure without rerunning it: the assertions that did not hold with both
/// sides, the regions of the snapshots that were wrong, the stack when the failure came with
/// one, and the tail of the log the case read.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct TraceRecord {
    /// The case's identifier.
    pub tc: String,
    /// The module code the case belongs to.
    pub module: String,
    /// How the case ended; never [`CaseStatus::Pass`].
    pub status: CaseStatus,
    /// The assertions that did not hold, with both sides.
    pub diffs: Vec<AssertionDiff>,
    /// Where an image was wrong, when the failure is a visual one.
    pub defects: Vec<ImageDefect>,
    /// The stack of the failure, when it came with one.
    pub stack: Option<String>,
    /// The tail of the log the case read.
    pub logs: LogExcerpt,
}

impl TraceRecord {
    /// The trace of a case, with the diffs taken from the assertions that did not hold.
    ///
    /// Taking them from the verdict record rather than from a second list the caller keeps is
    /// what makes the two documents agree by construction: a trace cannot name an assertion the
    /// case did not make, and an assertion the case failed cannot be missing from the trace.
    ///
    /// The defects, the stack and the log are added by the case, which is the only side that
    /// has them.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn of(evidence: &CaseEvidence) -> Self {
        let diffs = evidence
            .failed_assertions()
            .into_iter()
            .map(|assertion| AssertionDiff {
                name: assertion.name.clone(),
                expected: assertion.expected.clone(),
                actual: assertion.actual.clone(),
            })
            .collect();
        Self {
            tc: evidence.tc.clone(),
            module: evidence.module.clone(),
            status: evidence.status,
            diffs,
            defects: Vec::new(),
            stack: None,
            logs: LogExcerpt::empty(),
        }
    }

    /// Renders the document as the JSON text `trace.json` holds.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::Json`] when a field cannot be written as JSON.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn to_json(&self) -> Result<String, EvidenceError> {
        error::encode(TRACE_FILE, self)
    }
}
