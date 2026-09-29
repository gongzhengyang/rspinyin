//! The red lines a self-healing pass may never cross.
//!
//! Responsibility: judge what a run that repairs itself is allowed to do, and what it did. [`admit`]
//! takes the verdict before a recovery runs; [`audit_heal_pass`] takes it after, by comparing the
//! worktree against the one the pass started from. Nothing here opens a connection, reads a clock or
//! writes a file: the only input is a tree of paths and bytes.
//!
//! # Why a verdict rather than a convention
//!
//! A harness that can lower a bar until a case passes is worse than no harness at all, because it
//! reports green for a product that is broken. So the red lines are decided by [`audit_heal_pass`],
//! and a run stops on [`GuardReport::assert_clean`] rather than on a reviewer noticing afterwards.
//! [`refusal`] is the rule itself: for one path, whether a heal pass may write it, and if not, why.
//!
//! | Red line | Refusal |
//! |---|---|
//! | product code, a frozen file, the guard itself, or anything outside the allowlist was written | [`Violation::OutOfScope`] |
//! | a test or an assertion disappeared | [`Violation::TestsRemoved`], [`Violation::AssertionsWeakened`] |
//! | the tree fell below the frozen test count | [`Violation::TestBaselineLost`] |
//!
//! The recoveries are closed in the same way: re-deriving a locator from evidence the run already
//! produced is admitted, while taking the keyboard focus, re-running a step that already had an
//! effect, weakening the assertion that failed and writing outside the allowlist are not, and
//! [`Recovery`] has no variant for anything else. Losing the keyboard focus is the defect this
//! project treats as its highest-severity one (`features.md` 0.4 rule 5), so it is refused twice
//! over: here by [`Recovery::TakeFocus`], and structurally in `crate::testd::input`, where no key or
//! click can be sent without a `FocusGuard` and no `FocusGuard` exists that `X11Injector::focus` did
//! not create.
//!
//! # Failing closed
//!
//! An action whose safety cannot be established is refused rather than admitted with a warning.
//! [`TreeHash::capture`] never answers with a tree it could not read: an unreadable worktree becomes
//! [`TreeHash::Unavailable`], and [`audit_heal_pass`] turns that into [`Violation::Unauditable`]. An
//! empty diff would read as "nothing was touched", which is the one conclusion a tree that was not
//! read cannot support.

// The guard is exercised by the tests below and by nothing else yet: the heal loop that would call
// `admit`, the driver that would call `audit_heal_pass` and the subcommand tree that would print a
// report all live in files this module does not own. Until that wiring lands, every item here is
// reported as dead code in a non-test build, and the attribute goes away with those lines.
//
// `unused_imports` is covered by the same reasoning and for the same reason: the `pub use` lines
// below are this module's surface, and a `pub use` in a *binary* crate is "unused" whenever nothing
// in the crate names it.
#![allow(dead_code, unused_imports)]

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// Files a self-healing pass is allowed to touch. Anything else is a red-line violation and aborts
/// the run.
///
/// A heal pass repairs *test scripts*: neither the product those scripts exercise nor the
/// specification they are judged against is its to edit.
pub const HEAL_ALLOWED_PREFIXES: &[&str] = &["xtask/src/testd/", "docs/dev/tests/"];

/// Files whose content is frozen for the whole run; a diff fails the gate.
///
/// `budgets.json` and `features.md` carry the thresholds the cases assert, so a pass that could edit
/// them could lower any bar it failed to clear. The four build files are one step out from that: a
/// changed lint level, feature set or dependency set changes what "the tests pass" means.
pub const FROZEN_FILES: &[&str] = &[
    "docs/dev/budgets.json",
    "docs/dev/features.md",
    "Cargo.toml",
    "Cargo.lock",
    "clippy.toml",
    "justfile",
];

/// The guard's own sources, frozen for a reason of their own: the two constants above live in this
/// file, so a pass allowed to edit it could move its own red lines.
pub const GUARD_SOURCES: &[&str] = &["xtask/src/testd/guard.rs", "xtask/src/testd/guard/tests.rs"];

/// The product's own source tree, which a heal pass may never write.
///
/// The first red line, and the one that matters most: a pass that can edit the code under test can
/// make any case pass without the product changing at all.
pub const BUSINESS_CODE_PREFIXES: &[&str] = &["crates/"];

/// Directory names left out of the tree hash wherever they appear.
pub const TREE_EXCLUDED_NAMES: &[&str] = &[".git", "target"];

/// Directory names left out of the tree hash at the repository root.
///
/// `RUN` is the run's own evidence tree and `results` the directory it hangs under, so without these
/// every screenshot and trace would be reported as a file the pass touched.
pub const TREE_EXCLUDED_ROOTS: &[&str] = &["RUN", "results"];

/// The `#[test]` count the tree carried when this gate was installed.
///
/// [`audit_heal_pass`] compares a tree against the one the pass started from, which cannot see a
/// count that was already low. This floor is what stops a sequence of passes from each removing one.
pub const TEST_BASELINE: usize = 401;

/// Macro names that are an assertion.
///
/// The `debug_assert_*` family is deliberately absent: it is a real assertion in a debug build and
/// nothing at all in a release one, so rewriting `assert_eq!` as `debug_assert_eq!` would weaken a
/// case while leaving the source looking tested -- and the count drops, which reports it.
pub(super) const ASSERTION_MACROS: &[&str] =
    &["assert", "assert_eq", "assert_ne", "assert_matches"];

/// Why a heal pass may not write `relative`, or `None` when it may.
///
/// `relative` is relative to the repository root, which is the form [`TreeHash`] keys its files by.
/// Every comparison is by path component rather than by string, so `crates-extra/lib.rs` is not
/// product code and `xtask/src/testd-extra/` is not part of the allowlist. The guard's own sources
/// are refused before the allowlist they sit inside, because a pass that could rewrite the file
/// holding the allowlist could widen the allowlist.
///
/// # Panics
///
/// Never.
pub fn refusal(relative: &Path) -> Option<&'static str> {
    if is_one_of(GUARD_SOURCES, relative) {
        return Some("holds the red lines the pass is judged by");
    }
    if is_one_of(FROZEN_FILES, relative) {
        return Some("is frozen for the whole run");
    }
    if is_under(BUSINESS_CODE_PREFIXES, relative) {
        return Some("is product code and not a test script");
    }
    if is_under(HEAL_ALLOWED_PREFIXES, relative) {
        return None;
    }
    Some("is outside the heal allowlist")
}

/// Whether `relative` is one of `paths`.
fn is_one_of(paths: &[&str], relative: &Path) -> bool {
    paths.iter().any(|path| Path::new(*path) == relative)
}

/// Whether `relative` lies under one of `prefixes`.
fn is_under(prefixes: &[&str], relative: &Path) -> bool {
    prefixes
        .iter()
        .any(|prefix| relative.starts_with(Path::new(*prefix)))
}

/// What one file contributes to the tree hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHash {
    /// The lowercase hexadecimal SHA-256 of the file's contents, of a link's target, or of a kind.
    pub sha256: String,
    /// What the guard counted in the file's text, when the file is valid UTF-8.
    pub counts: SourceCounts,
}

/// The whole worktree, hashed file by file.
///
/// Taken before a self-healing pass and again after it, this is the evidence [`audit_heal_pass`]
/// judges. The type exists so that "the tree could not be read" cannot be expressed as "the tree is
/// empty", because an empty map makes every comparison against it agree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeHash {
    /// Every file of the tree, keyed by its path relative to the root, in path order.
    Available(BTreeMap<PathBuf, FileHash>),
    /// The tree could not be read, so nothing about it was checked.
    Unavailable {
        /// Why, in a form a developer can act on.
        reason: String,
    },
}

impl TreeHash {
    /// Hashes every file under `root`.
    ///
    /// Symbolic links are recorded by their own target text and never followed, so a link planted in
    /// the tree cannot make the hash read a file outside it.
    ///
    /// # Return value
    ///
    /// [`TreeHash::Unavailable`] when any part of the tree cannot be read. The failure is carried
    /// rather than returned because [`TreeHash::files`] turns that state into an error.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn capture(root: &Path) -> Self {
        let mut files = BTreeMap::new();
        match walk(root, root, &mut files) {
            Ok(()) => Self::Available(files),
            Err(error) => Self::Unavailable {
                reason: format!("{error:#}"),
            },
        }
    }

    /// Whether the tree was read.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available(_))
    }

    /// Every file of the tree, in path order.
    ///
    /// This is the gate the module is built on: a count and a verdict are both taken from the map
    /// this returns, so no judgement can be reached without a tree that was actually read.
    ///
    /// # Errors
    ///
    /// Returns [`GuardError::TreeUnavailable`] when the tree could not be read.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn files(&self) -> Result<&BTreeMap<PathBuf, FileHash>, GuardError> {
        match self {
            Self::Available(files) => Ok(files),
            Self::Unavailable { reason } => Err(GuardError::TreeUnavailable {
                reason: reason.clone(),
            }),
        }
    }
}

/// Everything the guard counts across a tree.
///
/// # Panics
///
/// Never.
pub fn totals(files: &BTreeMap<PathBuf, FileHash>) -> SourceCounts {
    files
        .values()
        .fold(SourceCounts::default(), |total, file| SourceCounts {
            tests: total.tests + file.counts.tests,
            assertions: total.assertions + file.counts.assertions,
        })
}

/// The verdict on one self-healing pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuardReport {
    /// Every file the pass added, removed or changed, in path order.
    pub touched: Vec<PathBuf>,
    /// Every red line the pass crossed; empty when it crossed none.
    pub violations: Vec<Violation>,
}

impl GuardReport {
    /// Whether the pass crossed no red line.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    /// Fails when the pass crossed a red line.
    ///
    /// This is the call a run makes before it reports anything: a pass that crossed a line has no
    /// verdict to give, so the run stops here rather than publishing a result a repair bought.
    ///
    /// # Errors
    ///
    /// Returns [`GuardError::RedLine`] carrying every violation the report holds.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn assert_clean(&self) -> Result<(), GuardError> {
        if self.violations.is_empty() {
            return Ok(());
        }
        Err(GuardError::RedLine {
            violations: self.violations.clone(),
        })
    }
}

/// Judges a self-healing pass by what it did to the tree.
///
/// The judgements are taken in one place because they are one verdict: the scope of every change and
/// the number of tests and assertions the tree holds before and after.
///
/// # Return value
///
/// A report whose `touched` list names every changed file and whose `violations` list is empty for a
/// pass that stayed inside the lines. When either tree could not be read the report holds
/// [`Violation::Unauditable`] and no touched files at all: nothing may be concluded from a tree that
/// was not read, and an empty diff would read as "nothing was touched".
///
/// # Panics
///
/// Never.
pub fn audit_heal_pass(before: &TreeHash, after: &TreeHash) -> GuardReport {
    let (earlier, later) = match (before.files(), after.files()) {
        (Ok(earlier), Ok(later)) => (earlier, later),
        (Err(error), _) | (_, Err(error)) => {
            return GuardReport {
                touched: Vec::new(),
                violations: vec![Violation::Unauditable {
                    reason: error.to_string(),
                }],
            };
        }
    };
    let paths = touched(earlier, later);
    let mut violations: Vec<Violation> = paths
        .iter()
        .filter_map(|path| {
            refusal(path).map(|reason| Violation::OutOfScope {
                path: path.clone(),
                reason,
            })
        })
        .collect();
    violations.extend(count_violations(earlier, later));
    GuardReport {
        touched: paths,
        violations,
    }
}

/// The verdict on the tree against the frozen test count.
///
/// [`audit_heal_pass`] compares a tree against the one the pass started from, which cannot see a
/// count that was already low before it ran. This comparison is against [`TEST_BASELINE`], and it is
/// what keeps a sequence of passes from each removing one test.
///
/// # Return value
///
/// One violation when the tree holds fewer tests than the baseline, and none otherwise. An
/// unreadable tree is a violation rather than a pass: a count that could not be taken is not a count
/// that held.
///
/// # Panics
///
/// Never.
pub fn audit_test_baseline(tree: &TreeHash) -> Vec<Violation> {
    let files = match tree.files() {
        Ok(files) => files,
        Err(error) => {
            return vec![Violation::Unauditable {
                reason: error.to_string(),
            }];
        }
    };
    let actual = totals(files).tests;
    if actual < TEST_BASELINE {
        return vec![Violation::TestBaselineLost {
            baseline: TEST_BASELINE,
            actual,
        }];
    }
    Vec::new()
}

/// What a self-healing pass proposes to do about a step that failed.
///
/// The vocabulary is closed on purpose. A repair has exactly one admitted action -- look again, at
/// evidence the run already produced -- and an action that is not that one cannot be expressed,
/// which is what "fails closed" means when it is a type rather than a promise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recovery {
    /// Re-derive a locator from evidence the run already produced, writing it into `target`.
    ReDeriveLocator {
        /// The test script the re-derived locator would be written to.
        target: PathBuf,
    },
    /// Move the keyboard focus, or ask the server for it.
    TakeFocus,
    /// Run a step again after it already had an effect. A retry is safe only when it is idempotent,
    /// and the steps this harness drives are not: the sandbox reset takes the user database away and
    /// a dictionary mutation rewrites a compiled file, so re-running either destroys the state the
    /// failed case was measured against.
    DestructiveRetry,
    /// Weaken, skip or delete the assertion that failed.
    HideFailure,
}

/// The verdict on a recovery, taken before it runs.
///
/// # Errors
///
/// Returns the red line the recovery would cross. Only [`Recovery::ReDeriveLocator`] aimed inside
/// [`HEAL_ALLOWED_PREFIXES`] is admitted; every other recovery is refused, and so is a re-derivation
/// aimed anywhere else -- which is what "a recovery may never widen what it is allowed to touch"
/// means when it is a function rather than a promise.
///
/// # Panics
///
/// Never.
pub fn admit(recovery: &Recovery) -> Result<(), Violation> {
    match recovery {
        Recovery::ReDeriveLocator { target } => match refusal(target) {
            None => Ok(()),
            Some(reason) => Err(Violation::OutOfScope {
                path: target.clone(),
                reason,
            }),
        },
        Recovery::TakeFocus => Err(Violation::FocusTaken),
        Recovery::DestructiveRetry => Err(Violation::DestructiveRetry),
        Recovery::HideFailure => Err(Violation::FailureHidden),
    }
}

/// A red line a self-healing pass crossed.
///
/// Every variant names what it is about, because a gate that fails without saying which file or
/// which count it failed on is a gate nobody can act on.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Violation {
    /// The pass wrote a file the heal allowlist does not cover, or one it may never write.
    #[error("the heal pass wrote {path}, which {reason}")]
    OutOfScope {
        /// The file the pass wrote.
        path: PathBuf,
        /// Why it is out of bounds, as a phrase that completes the message.
        reason: &'static str,
    },
    /// The pass left the tree with fewer test functions than it found.
    #[error("the heal pass left {after} `#[test]` functions where the tree had {before}")]
    TestsRemoved {
        /// The count before the pass.
        before: usize,
        /// The count after it.
        after: usize,
        /// The files whose own count dropped.
        paths: Vec<PathBuf>,
    },
    /// The pass left the tree with fewer assertions than it found.
    #[error("the heal pass left {after} assertions where the tree had {before}")]
    AssertionsWeakened {
        /// The count before the pass.
        before: usize,
        /// The count after it.
        after: usize,
        /// The files whose own count dropped.
        paths: Vec<PathBuf>,
    },
    /// The tree holds fewer test functions than the frozen baseline.
    #[error("the tree holds {actual} `#[test]` functions, below the baseline of {baseline}")]
    TestBaselineLost {
        /// The frozen baseline.
        baseline: usize,
        /// The count the tree actually holds.
        actual: usize,
    },
    /// A recovery asked for the keyboard focus.
    #[error("the recovery asked for the keyboard focus")]
    FocusTaken,
    /// A recovery asked to run a step again after it had already had an effect.
    #[error("the recovery asked to re-run a step that already had an effect")]
    DestructiveRetry,
    /// A recovery asked to weaken or remove the assertion that failed.
    #[error("the recovery asked to weaken the assertion that failed")]
    FailureHidden,
    /// The pass could not be judged at all.
    #[error("the heal pass could not be audited: {reason}")]
    Unauditable {
        /// Why the audit could not be taken.
        reason: String,
    },
}

/// Why the guard refused a heal pass.
#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    /// The tree could not be read, so nothing may be concluded from it.
    #[error("the worktree could not be hashed: {reason}")]
    TreeUnavailable {
        /// Why, in a form a developer can act on.
        reason: String,
    },
    /// The pass crossed a red line.
    #[error("the heal pass crossed {} red line(s): {violations:?}", .violations.len())]
    RedLine {
        /// Every red line the pass crossed.
        violations: Vec<Violation>,
    },
}

/// Reads every file under `dir` into `files`, keyed by its path relative to `root`.
///
/// Recursion depth is the depth of the tree, a handful of levels for a source repository. An excluded
/// directory is not entered, so the evidence tree is never walked while a case writes into it.
fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, FileHash>) -> Result<()> {
    let entries = fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let path = entry
            .with_context(|| format!("reading {}", dir.display()))?
            .path();
        let relative = path
            .strip_prefix(root)
            .with_context(|| format!("placing {}", path.display()))?
            .to_path_buf();
        if is_excluded(&relative) {
            continue;
        }
        let meta =
            fs::symlink_metadata(&path).with_context(|| format!("reading {}", path.display()))?;
        if meta.is_dir() {
            walk(root, &path, files)?;
            continue;
        }
        files.insert(relative, hash_entry(&path, &meta)?);
    }
    Ok(())
}

/// Whether a path relative to the repository root is left out of the tree hash.
///
/// The first component is measured against both lists, because a top-level `target` or `.git` is
/// consumed by the same step that looks for a top-level `RUN`. A name that is not valid UTF-8 is
/// hashed rather than skipped, which is the safe direction.
fn is_excluded(relative: &Path) -> bool {
    let named =
        |name: &OsStr, names: &[&str]| names.iter().any(|other| name.to_str() == Some(*other));
    let mut components = relative.components();
    let excluded_first = matches!(
        components.next(),
        Some(Component::Normal(name))
            if named(name, TREE_EXCLUDED_ROOTS) || named(name, TREE_EXCLUDED_NAMES)
    );
    if excluded_first {
        return true;
    }
    components.any(|component| {
        matches!(component, Component::Normal(name) if named(name, TREE_EXCLUDED_NAMES))
    })
}

/// What one entry of the tree hashes to.
///
/// A symbolic link is hashed by its own target text and never read through, because a link is a path
/// somebody else chose. A fifo, a socket or a device is recorded as one, so that adding or removing
/// it is still a change, and is never opened, because opening one would block or fail.
fn hash_entry(path: &Path, meta: &fs::Metadata) -> Result<FileHash> {
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        let target =
            fs::read_link(path).with_context(|| format!("reading the link {}", path.display()))?;
        return Ok(FileHash {
            sha256: digest(format!("symlink:{}", target.display()).as_bytes()),
            counts: SourceCounts::default(),
        });
    }
    if !file_type.is_file() {
        return Ok(FileHash {
            sha256: digest(b"special"),
            counts: SourceCounts::default(),
        });
    }
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let counts = match std::str::from_utf8(&bytes) {
        Ok(text) => scan_source(text),
        Err(_) => SourceCounts::default(),
    };
    Ok(FileHash {
        sha256: digest(&bytes),
        counts,
    })
}

/// The lowercase hexadecimal SHA-256 of `bytes`.
fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Every file that differs between the two maps, in path order.
///
/// A file added, removed or rewritten is the same fact for the audit -- the pass touched it -- so
/// the list is one of paths and not of change kinds.
fn touched(
    earlier: &BTreeMap<PathBuf, FileHash>,
    later: &BTreeMap<PathBuf, FileHash>,
) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = earlier
        .iter()
        .filter(|(path, hash)| later.get(*path).is_none_or(|other| other != *hash))
        .map(|(path, _)| path.clone())
        .collect();
    paths.extend(
        later
            .keys()
            .filter(|path| !earlier.contains_key(*path))
            .cloned(),
    );
    paths.sort();
    paths
}

/// The violations the two counts earn.
fn count_violations(
    earlier: &BTreeMap<PathBuf, FileHash>,
    later: &BTreeMap<PathBuf, FileHash>,
) -> Vec<Violation> {
    let before = totals(earlier);
    let after = totals(later);
    let mut violations = Vec::new();
    if after.tests < before.tests {
        violations.push(Violation::TestsRemoved {
            before: before.tests,
            after: after.tests,
            paths: dropped(earlier, later, |counts| counts.tests),
        });
    }
    if after.assertions < before.assertions {
        violations.push(Violation::AssertionsWeakened {
            before: before.assertions,
            after: after.assertions,
            paths: dropped(earlier, later, |counts| counts.assertions),
        });
    }
    violations
}

/// The files present in both trees whose own count fell.
fn dropped(
    earlier: &BTreeMap<PathBuf, FileHash>,
    later: &BTreeMap<PathBuf, FileHash>,
    of: fn(&SourceCounts) -> usize,
) -> Vec<PathBuf> {
    earlier
        .iter()
        .filter_map(|(path, before)| {
            let after = later.get(path)?;
            (of(&after.counts) < of(&before.counts)).then(|| path.clone())
        })
        .collect()
}

mod scan;
#[cfg(test)]
mod tests;

pub use self::scan::{SourceCounts, scan_source};
