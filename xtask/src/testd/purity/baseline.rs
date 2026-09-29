//! The frozen record of a number taken on a clean machine, and the comparisons made against it.
//!
//! Responsibility: freeze one case's measurement as the baseline every later run of that case is
//! divided by, and answer whether a later run is inside the tolerance of it. A baseline is the
//! one number the guard cannot let a busy machine contribute: a baseline frozen while other
//! agents were building bakes a false regression into the repository, and nothing later can tell
//! that it happened. [`Baseline::frozen`] therefore refuses a report that is not clean, and
//! [`Baseline::compare`] refuses a run whose machine is busy, whose processor model differs or
//! whose frequency-scaling governor differs -- a ratio across any of those is not the same
//! measurement twice.
//!
//! Boundaries: it reads and writes one JSON document per budget key, below the root it is given,
//! and resolves no directory of its own -- no `$HOME`, no XDG variable, no current directory. It
//! reads no clock: the time a baseline was frozen at is the caller's reading. It starts no
//! process, sends no signal and opens no socket.
//!
//! The tests live in the sibling `tests` module, beside the verdict's, because together they
//! would be longer than the file limit allows.

// The record is frozen and read by the benchmark gate, which lives in a file this module does not
// own. Until that wiring lands, every item here is reported as dead code in a non-test build, and
// the attribute goes away with those lines.
#![allow(dead_code)]

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::PurityReport;

/// The directory baselines are frozen in, relative to the repository root.
pub const BASELINE_DIR: &str = "results/baselines";

/// A measurement frozen from a clean run, for later runs to be compared against.
///
/// The record carries the model and the governor because an absolute number is only meaningful
/// together with the machine it was taken on: a ratio compared across a machine change or a
/// governor change is not measuring the same thing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Baseline {
    /// The budget key the number belongs to, e.g. `bench.passthrough_classify_ns`.
    pub key: String,
    /// The measured value, in the unit the key's field name states.
    pub value: f64,
    /// The model of the processor the value was taken on, when the platform names one.
    pub cpu_model: Option<String>,
    /// The frequency-scaling governor in force during the run.
    pub scaling_governor: Option<String>,
    /// When the value was frozen, in seconds since the Unix epoch.
    pub frozen_at: u64,
}

impl Baseline {
    /// Freezes `value` as the baseline for `key`, if the machine it was taken on was clean.
    ///
    /// The gate is here rather than at the call site because a baseline is the one number every
    /// later run is divided by: freezing one from a busy machine bakes a false regression into
    /// the repository, and nothing later can tell that it happened. `frozen_at` is the caller's
    /// clock reading; the guard reads no clock itself.
    ///
    /// # Errors
    /// Returns [`BaselineError::UnusableValue`] when `value` is not a positive finite number
    /// -- a divisor cannot be one -- and [`BaselineError::Dirty`] when `report` is not clean.
    ///
    /// # Panics
    /// Never.
    pub fn frozen(
        key: &str,
        value: f64,
        report: &PurityReport,
        frozen_at: u64,
    ) -> Result<Self, BaselineError> {
        if !value.is_finite() || value <= 0.0 {
            return Err(BaselineError::UnusableValue {
                key: key.to_owned(),
                value,
            });
        }
        match report.rejection_reason() {
            Some(reason) => Err(BaselineError::Dirty {
                key: key.to_owned(),
                reason,
            }),
            None => Ok(Self {
                key: key.to_owned(),
                value,
                cpu_model: report.facts.cpu_model.clone(),
                scaling_governor: report.facts.scaling_governor.clone(),
                frozen_at,
            }),
        }
    }

    /// Compares `current` with this baseline.
    ///
    /// `allowed_ratio` is how much larger than the baseline a measurement may be before it
    /// counts as a regression -- `1.10` for ten percent. The tolerance belongs to the caller:
    /// the budget document states thresholds, not tolerances. The comparison is refused rather
    /// than answered whenever it would be meaningless: a run on a machine that is not clean, a
    /// measurement that cannot be a ratio, and a pair of runs that cannot be shown to come from
    /// the same machine in the same state all come back as
    /// [`BaselineComparison::NotComparable`], never as a ratio.
    ///
    /// # Panics
    /// Never.
    pub fn compare(
        &self,
        current: f64,
        report: &PurityReport,
        allowed_ratio: f64,
    ) -> BaselineComparison {
        if let Some(reason) = report.rejection_reason() {
            return BaselineComparison::NotComparable { reason };
        }
        let facts = report.facts();
        let model = same_value(
            "processor model",
            self.cpu_model.as_deref(),
            facts.cpu_model.as_deref(),
        );
        let governor = same_value(
            "frequency-scaling governor",
            self.scaling_governor.as_deref(),
            facts.scaling_governor.as_deref(),
        );
        if let Err(reason) = model.and(governor) {
            return BaselineComparison::NotComparable { reason };
        }
        if !current.is_finite() || current <= 0.0 {
            return BaselineComparison::NotComparable {
                reason: format!("the measurement {current} is not a positive number"),
            };
        }
        let ratio = current / self.value;
        if ratio <= allowed_ratio {
            BaselineComparison::Within { ratio }
        } else {
            BaselineComparison::Regression {
                ratio,
                allowed: allowed_ratio,
            }
        }
    }

    /// Writes this baseline to `path`, which [`baseline_path`] resolves.
    ///
    /// # Errors
    /// Returns [`BaselineError::Io`] when the directory cannot be created or the file written.
    ///
    /// # Panics
    /// Never.
    pub fn write(&self, path: &Path) -> Result<(), BaselineError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| at(parent, source))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|source| at_json(path, source))?;
        fs::write(path, format!("{text}\n")).map_err(|source| at(path, source))
    }

    /// Reads the baseline `path` holds, or `None` when no file is there.
    ///
    /// A missing file is not an error -- it is the first run of a case, which is when a baseline
    /// is frozen. A file that is there and cannot be read is an error: a baseline nobody can
    /// parse must not be treated as absent.
    ///
    /// # Errors
    /// Returns [`BaselineError::Malformed`] when the document does not parse and
    /// [`BaselineError::Io`] when the file exists but cannot be read.
    ///
    /// # Panics
    /// Never.
    pub fn read(path: &Path) -> Result<Option<Self>, BaselineError> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let baseline: Self =
                    serde_json::from_str(&text).map_err(|source| at_json(path, source))?;
                Ok(Some(baseline))
            }
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(source) => Err(at(path, source)),
        }
    }
}

/// What comparing a measurement with a frozen baseline amounts to.
#[derive(Clone, Debug, PartialEq)]
pub enum BaselineComparison {
    /// The measurement is inside the tolerance of the baseline.
    Within {
        /// The measurement divided by the baseline.
        ratio: f64,
    },
    /// The measurement is past the tolerance, against a baseline taken on a clean machine of
    /// the same model under the same governor.
    Regression {
        /// The measurement divided by the baseline.
        ratio: f64,
        /// The largest ratio that would have been accepted.
        allowed: f64,
    },
    /// The two numbers are not comparable, for this reason.
    NotComparable {
        /// Why no ratio was computed.
        reason: String,
    },
}

/// The file a budget key's baseline lives in, below `root`.
///
/// # Errors
/// Returns [`BaselineError::BadKey`] when the key cannot be a file name.
///
/// # Panics
/// Never.
pub fn baseline_path(root: &Path, key: &str) -> Result<PathBuf, BaselineError> {
    match file_name(key) {
        Some(name) => Ok(root.join(BASELINE_DIR).join(name)),
        None => Err(BaselineError::BadKey { key: key.into() }),
    }
}

/// Everything freezing and reading a baseline can fail with.
#[derive(Debug, thiserror::Error)]
pub enum BaselineError {
    /// A key cannot be the name of a file in the baseline directory.
    #[error("`{key}` is not a usable baseline key")]
    BadKey { key: String },
    /// A value cannot be a baseline, because it cannot be divided by.
    #[error("`{key}` cannot be frozen: {value} is not a positive number")]
    UnusableValue { key: String, value: f64 },
    /// A baseline was offered from a machine that was not quiet.
    #[error("`{key}` cannot be frozen from a machine that is not clean: {reason}")]
    Dirty { key: String, reason: String },
    /// A file of the baseline directory could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file the operation was about.
        path: PathBuf,
        /// The failure the operation reported.
        #[source]
        source: std::io::Error,
    },
    /// A baseline document could not be rendered or parsed.
    #[error("{path}: not a usable baseline document: {source}")]
    Malformed {
        /// The file the document was read from, or would have been written to.
        path: PathBuf,
        /// What the parser reported.
        #[source]
        source: serde_json::Error,
    },
}

/// A file operation's failure, with the path it was about.
fn at(path: &Path, source: std::io::Error) -> BaselineError {
    BaselineError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// A document's failure, with the path it was about.
fn at_json(path: &Path, source: serde_json::Error) -> BaselineError {
    BaselineError::Malformed {
        path: path.to_path_buf(),
        source,
    }
}

/// Whether one fact of a baseline and of a current run is known on both sides and equal.
///
/// A fact unknown on either side is refused rather than assumed equal: two runs whose processor
/// model nobody read cannot be shown to be comparable.
fn same_value(what: &str, frozen: Option<&str>, current: Option<&str>) -> Result<(), String> {
    match (frozen, current) {
        (Some(frozen), Some(current)) if frozen == current => Ok(()),
        (Some(frozen), Some(current)) => Err(format!(
            "the baseline's {what} is `{frozen}` and this run's is `{current}`: the two numbers \
             do not measure the same thing"
        )),
        _ => Err(format!(
            "the {what} of the baseline or of this run is unknown, so the two numbers cannot be \
             shown to have been taken on the same machine"
        )),
    }
}

/// The file name a budget key maps to, or `None` when it cannot be one.
///
/// A key is a dotted path (`bench.passthrough_classify_ns`), so the mapping is the identity
/// with `.json` appended. A key holding anything else is refused rather than sanitised: two
/// keys that sanitise to one file would silently share a baseline.
fn file_name(key: &str) -> Option<String> {
    let usable = !key.is_empty()
        && key.len() <= 128
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'));
    usable.then(|| format!("{key}.json"))
}
