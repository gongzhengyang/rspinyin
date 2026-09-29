//! The run's tree: the directory a run owns, and the bundle one of its cases leaves in it.
//!
//! Responsibility: build `results/runs/run-<stamp>/` and everything under it, hand out the path
//! a case's snapshot belongs at, write a case's documents, and keep every directory at `0700`
//! and every file at `0600`. Nothing here decides a verdict, reads a clock or touches a display
//! server: the instant a run started arrives as a [`Utc`] and the status a case reached arrives
//! as a value.
//!
//! # The shape is the screenshot channel's
//!
//! A case's directory is `RUN/<module>/<TC-ID>/`, and the PNG inside it is
//! `[step]_[state].png`. Both are the screenshot channel's to state, and this module asks it for
//! the path rather than spelling the shape a second time: two copies of a layout drift, and the
//! copy that drifts is always the one nobody reads. What this module adds is the two documents
//! beside the pixels and the mode the whole tree is created with.
//!
//! # Why a run refuses to share its name
//!
//! A run is named for the second it started in. Two runs in one second would share a directory,
//! interleave their cases, and leave a reviewer reading one run's pixels beside another run's
//! verdict -- so the directory is created with `mkdir(2)` and a name already taken is refused
//! rather than reused.
//!
//! # Why a partial bundle is an error and not a best effort
//!
//! A case's documents are written one after another, and the first failure stops the write and
//! is returned. Carrying on would leave a bundle that looks complete -- an `assertions.json`
//! beside a `trace.json` that was never written reads as a case that failed without a diff --
//! and the caller has no way to tell the two apart afterwards. The caller records the error as
//! an evidence gap instead, which is what makes the loss visible in the run's verdict.
//!
//! # Why a second write of one case leaves one bundle
//!
//! A case can be run twice inside a single run -- a retry after a flake -- and the second write
//! replaces the first rather than joining it. The documents a case always produces are rewritten
//! whole, and the two it produces conditionally are removed when this write does not produce
//! them: a passing case whose directory still held the previous write's `trace.json` would read
//! as a case that failed, and a reader would have no way to tell which of the two the verdict
//! beside it belongs to. The snapshots are the case's own and are not touched here -- a case that
//! re-captures a step writes the same name again, and one that captures a new name leaves both,
//! which is what a reviewer comparing two attempts wants.

use std::fs::DirBuilder;
use std::io::{ErrorKind, Write as _};
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};

use ime_diag::perms;

use super::error::EvidenceError;
use super::{
    ASSERTIONS_FILE, CaseEvidence, HEAL_FILE, LEGACY_SUBDIRS, RESULTS_DIR, RUN_INDEX_FILE,
    RUNS_DIR, TRACE_FILE, TraceRecord, Utc,
};
use crate::testd::capture;

/// The state tag the directory probe slugs, which is never a name a case files.
///
/// A case's directory is the parent of a snapshot path, and a snapshot path needs a state; this
/// one exists only long enough to be dropped by [`Path::parent`].
const CASE_DIR_PROBE: &str = "case";

/// One run's evidence tree: `results/runs/run-<stamp>/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunDir {
    /// The run's own directory.
    root: PathBuf,
    /// The run's directory name, e.g. `run-20260930-014455`.
    name: String,
}

impl RunDir {
    /// Creates the run's tree under `results_root`, named for the instant `started` names.
    ///
    /// The results directory and the `runs` directory below it are created when they are
    /// missing and narrowed when they are not, so the whole tree is private however the
    /// repository was laid out.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::LegacyLayout`] when `results_root` is a flat layout
    /// `dev-check` files as legacy, [`EvidenceError::RunExists`] when a run of this name is
    /// already there, and [`EvidenceError::Io`] when a directory cannot be created or its mode
    /// cannot be set.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn create(results_root: &Path, started: Utc) -> Result<Self, EvidenceError> {
        refuse_legacy_layout(results_root)?;
        let name = started.run_name();
        create_private_dir(results_root)?;
        let runs = results_root.join(RUNS_DIR);
        create_private_dir(&runs)?;
        let root = runs.join(&name);
        refuse_legacy_layout(&root)?;
        // Not `create_dir_all`: a run of this name is a run that already happened, and reusing
        // its directory would mix two runs' evidence in one tree.
        match DirBuilder::new().mode(perms::DIR_MODE).create(&root) {
            Ok(()) => Ok(Self { root, name }),
            Err(source) if source.kind() == ErrorKind::AlreadyExists => {
                Err(EvidenceError::RunExists { path: root })
            }
            Err(source) => Err(io_error(&root, source)),
        }
    }

    /// The run's own directory.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The run's directory name, e.g. `run-20260930-014455`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The path a case's snapshot is written to, inside this run's tree.
    ///
    /// The screenshot channel writes the file; this hands out where it goes, so that the
    /// directory a reviewer opens and the directory the pixels land in are the same one.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::Capture`] when a segment is empty, oversized or carries a
    /// character a path component may not, and [`EvidenceError::LegacyLayout`] when the path
    /// would land in a flat layout.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn snapshot_path(
        &self,
        module: &str,
        case_id: &str,
        step: u32,
        state: &str,
    ) -> Result<PathBuf, EvidenceError> {
        let path = capture::snapshot_path(&self.root, module, case_id, step, state)?;
        refuse_legacy_layout(&path)?;
        Ok(path)
    }

    /// Writes one case's evidence bundle.
    ///
    /// `trace` is the failure trace, and `heal` the patch a healed case leaves; both are
    /// optional, and which combinations are legal follows from the case's status: a passing
    /// case leaves no trace, and a failed or flawed one does not leave without it.
    ///
    /// Writing a case that is already in this run replaces its bundle: the documents this call
    /// produces are rewritten whole, and a `trace.json` or `heal.patch` the previous write left
    /// is removed when this one does not produce it, so one case never ends up with two
    /// verdicts' worth of artefacts.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::UnexpectedTrace`] when a passing case was handed a trace,
    /// [`EvidenceError::MissingTrace`] when a failed or flawed one was handed none,
    /// [`EvidenceError::Capture`] when a case segment is not usable, and
    /// [`EvidenceError::Io`] or [`EvidenceError::Json`] when a document cannot be written.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn write_case(
        &self,
        evidence: &CaseEvidence,
        trace: Option<&TraceRecord>,
        heal: Option<&str>,
    ) -> Result<CaseBundle, EvidenceError> {
        check_trace_pair(evidence, trace)?;
        let dir = self.case_dir(&evidence.module, &evidence.tc)?;
        create_private_dir(&dir)?;
        // The two conditional artefacts are cleared before anything is written, so a stale file
        // that cannot be removed is reported while the directory is still the previous write's,
        // rather than after half of a new bundle has landed beside it.
        let trace_path = dir.join(TRACE_FILE);
        let heal_path = dir.join(HEAL_FILE);
        if trace.is_none() {
            remove_if_present(&trace_path)?;
        }
        if heal.is_none() {
            remove_if_present(&heal_path)?;
        }
        let assertions = dir.join(ASSERTIONS_FILE);
        write_private(&assertions, evidence.to_json()?.as_bytes())?;
        let trace_path = match trace {
            Some(record) => {
                write_private(&trace_path, record.to_json()?.as_bytes())?;
                Some(trace_path)
            }
            None => None,
        };
        let heal_path = match heal {
            Some(text) => {
                write_private(&heal_path, text.as_bytes())?;
                Some(heal_path)
            }
            None => None,
        };
        Ok(CaseBundle {
            dir,
            assertions,
            trace: trace_path,
            heal: heal_path,
        })
    }

    /// Writes the run's own `index.md`, which is the table of cases the run holds.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::Io`] when the file cannot be written or its mode cannot be set.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn write_index(&self, text: &str) -> Result<PathBuf, EvidenceError> {
        let path = self.root.join(RUN_INDEX_FILE);
        write_private(&path, text.as_bytes())?;
        Ok(path)
    }

    /// The directory one case's evidence lives in: `<run>/<module>/<TC-ID>/`.
    ///
    /// # Errors
    ///
    /// As [`RunDir::snapshot_path`].
    ///
    /// # Panics
    ///
    /// Never.
    fn case_dir(&self, module: &str, case_id: &str) -> Result<PathBuf, EvidenceError> {
        // Derived from a path the screenshot channel validated rather than from a second copy
        // of its segment rules; `parent` is always `Some` for the shape it builds.
        let probe = capture::snapshot_path(&self.root, module, case_id, 0, CASE_DIR_PROBE)?;
        Ok(probe
            .parent()
            .map_or_else(|| self.root.clone(), Path::to_path_buf))
    }
}

/// What one case's evidence bundle came out as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseBundle {
    /// The case's own directory.
    pub dir: PathBuf,
    /// The verdict record, written for every case.
    pub assertions: PathBuf,
    /// The failure trace, written for a failed or flawed case.
    pub trace: Option<PathBuf>,
    /// The healing patch, written when the case healed something.
    pub heal: Option<PathBuf>,
}

/// Refuses a path that names one of the flat layouts `dev-check` files as legacy.
///
/// The test is positional rather than a search: a legacy layout is a `screenshots` or `traces`
/// directory sitting *directly* under a `results` directory. A directory of the same name
/// deeper in the tree -- under `results/runs/run-<stamp>/` -- is part of a run's own layout and
/// is left alone, which is what keeps a case named after one of them writable.
///
/// # Errors
///
/// Returns [`EvidenceError::LegacyLayout`] when the path is a legacy layout.
///
/// # Panics
///
/// Never.
pub fn refuse_legacy_layout(path: &Path) -> Result<(), EvidenceError> {
    // The path is split into its own text rather than walked component by component: what the
    // rule compares is two directory names, and a path is UTF-8 here -- it is built from the
    // repository root and a run's own names.
    let text = path.to_string_lossy();
    let mut segments = text.split('/');
    while let Some(segment) = segments.next() {
        if segment != RESULTS_DIR {
            continue;
        }
        if let Some(flat) = segments.next() {
            if LEGACY_SUBDIRS.contains(&flat) {
                return Err(EvidenceError::LegacyLayout {
                    path: path.to_path_buf(),
                    found: flat.to_owned(),
                });
            }
        }
        break;
    }
    Ok(())
}

/// Checks the trace a case was handed against the status it reached.
///
/// # Errors
///
/// Returns [`EvidenceError::UnexpectedTrace`] when a passing case was handed one, and
/// [`EvidenceError::MissingTrace`] when a failed or flawed one was handed none.
///
/// # Panics
///
/// Never.
fn check_trace_pair(
    evidence: &CaseEvidence,
    trace: Option<&TraceRecord>,
) -> Result<(), EvidenceError> {
    match (evidence.status.needs_trace(), trace) {
        (false, Some(_)) => Err(EvidenceError::UnexpectedTrace {
            module: evidence.module.clone(),
            tc: evidence.tc.clone(),
        }),
        (true, None) => Err(EvidenceError::MissingTrace {
            module: evidence.module.clone(),
            tc: evidence.tc.clone(),
        }),
        _ => Ok(()),
    }
}

/// Creates `dir` and every missing parent, and leaves `dir` private.
///
/// The mode goes to `mkdir(2)` rather than to a `chmod` afterwards, so the directory is never
/// reachable by another account, not even for one syscall. A directory that was already there
/// is narrowed by the same call.
///
/// # Errors
///
/// Returns [`EvidenceError::Io`] when a directory cannot be created or its mode cannot be set.
///
/// # Panics
///
/// Never.
fn create_private_dir(dir: &Path) -> Result<(), EvidenceError> {
    DirBuilder::new()
        .recursive(true)
        .mode(perms::DIR_MODE)
        .create(dir)
        .map_err(|source| io_error(dir, source))?;
    // `tighten` also reports the mode an existing directory had before it was narrowed; this
    // module has no diagnostic channel to report that on, and the promise it keeps -- that the
    // directory ends up private -- is what matters here.
    let _previous = perms::tighten(dir, perms::DIR_MODE).map_err(|source| io_error(dir, source))?;
    Ok(())
}

/// Writes `bytes` to `path`, creating it `0600` and narrowing one that was wider.
///
/// The file is truncated through the descriptor that was just opened rather than by a second
/// lookup of the path, so a document that shrank cannot leave the tail of its predecessor
/// behind and a document cannot be replaced by one another account opened in between.
///
/// # Errors
///
/// Returns [`EvidenceError::Io`] when the file cannot be opened, truncated, written or its mode
/// cannot be set.
///
/// # Panics
///
/// Never.
pub(super) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), EvidenceError> {
    let mut file = perms::create_private(path).map_err(|source| io_error(path, source))?;
    file.set_len(0).map_err(|source| io_error(path, source))?;
    file.write_all(bytes)
        .map_err(|source| io_error(path, source))?;
    file.flush().map_err(|source| io_error(path, source))?;
    Ok(())
}

/// Removes `path` when it is there, and succeeds when it is not.
///
/// This is what lets a second write of a case drop the conditional artefact the first one left:
/// the file's absence is the desired end state, not a failure to report.
///
/// # Errors
///
/// Returns [`EvidenceError::Io`] when the path is there and cannot be removed. A directory of
/// that name arrives here too, which is what it should be: it is not the document the bundle
/// expects to own.
///
/// # Panics
///
/// Never.
fn remove_if_present(path: &Path) -> Result<(), EvidenceError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == ErrorKind::NotFound => Ok(()),
        Err(source) => Err(io_error(path, source)),
    }
}

/// The refusal a filesystem failure is reported with.
///
/// # Panics
///
/// Never.
fn io_error(path: &Path, source: std::io::Error) -> EvidenceError {
    EvidenceError::Io {
        path: path.to_path_buf(),
        source,
    }
}
