//! The machine facts a verdict is only valid on, taken from the environment channel's own lines.
//!
//! Responsibility: turn the environment report into the list of [`EnvironmentFact`]s a case's
//! evidence carries, and add the two facts that report does not hold. Boundaries: it probes
//! nothing the environment channel already probes. The display server, the compositor, the
//! Fcitx5 development package, the font count and the concurrent build count are read once, by
//! [`EnvCapabilities`], and this module consumes its lines -- so a case's evidence and the
//! environment gate's verdict describe the same machine by construction rather than by two
//! readings that happen to agree.
//!
//! # Why two facts are added here
//!
//! The environment channel answers capability questions: what this machine can exercise. Two
//! facts a verdict depends on are not capability questions and are not in its report:
//!
//! * whether the kernel is WSL's, because a budget number taken under WSL measures the virtual
//!   machine's scheduler as much as the code, and `features.md` 0.5.5 registers the development
//!   machine as WSL2;
//! * the commit the tree is at, because a verdict is a statement about a revision, and a bundle
//!   that does not name one cannot be compared with the bundle before it.
//!
//! Both are recorded as `unknown` when they cannot be read, and never as a negative answer. That
//! is the discipline the environment channel states for its own readings -- "nobody could look"
//! and "the answer is no" are different answers -- and it applies to these two for the same
//! reason: a verdict whose machine is unknown is a verdict that has to be read with that in mind.

use std::path::Path;
use std::process::Command;

use crate::testd::env::EnvCapabilities;
use crate::testd::evidence::EnvironmentFact;

/// The file the running kernel states its own version in.
const PROC_VERSION: &str = "/proc/version";

/// The word a WSL kernel carries in its version string and a bare Linux kernel does not.
///
/// The marker is the vendor name WSL patches into the kernel it ships; there is no other
/// reliable signal from inside the guest, since `$WSL_DISTRO_NAME` and `$WSL_INTEROP` are set by
/// the login session and are absent from a process that was started without one.
const WSL_MARKER: &str = "microsoft";

/// The value a fact carries when the reading could not be taken.
pub const UNKNOWN: &str = "unknown";

/// The machine a run's cases ran on, as one case's evidence records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Environment {
    /// The facts, in the order the environment report states them, then the two added here.
    facts: Vec<EnvironmentFact>,
}

impl Environment {
    /// Collects the facts a case's evidence carries.
    ///
    /// `root` is the repository root, which is where the commit is read from.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn of(report: &EnvCapabilities, root: &Path) -> Self {
        let lines = report.lines();
        let mut facts: Vec<EnvironmentFact> = Vec::with_capacity(lines.len() + 2);
        for line in &lines {
            facts.push(EnvironmentFact::of_line(line));
        }
        let version = std::fs::read_to_string(PROC_VERSION).ok();
        facts.push(EnvironmentFact {
            key: String::from("wsl"),
            value: wsl_of(version.as_deref()).to_owned(),
        });
        facts.push(EnvironmentFact {
            key: String::from("commit"),
            value: commit_of(root).unwrap_or_else(|| UNKNOWN.to_owned()),
        });
        Self { facts }
    }

    /// The facts, in the order they were collected.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn facts(&self) -> &[EnvironmentFact] {
        &self.facts
    }
}

/// Whether the kernel is WSL's, from the kernel's own version text.
///
/// `None` is a version string that could not be read, which is `unknown` rather than `no`: a
/// machine nobody could ask is not a machine that answered.
///
/// # Panics
///
/// Never.
pub fn wsl_of(version: Option<&str>) -> &'static str {
    match version {
        None => UNKNOWN,
        Some(text) if text.to_ascii_lowercase().contains(WSL_MARKER) => "yes",
        Some(_) => "no",
    }
}

/// The commit the tree under `root` is at, as `git rev-parse HEAD` reports it.
///
/// `None` for a tree that is not a repository and for a `git` that refuses to answer: a bundle
/// that named the wrong revision would be worse than one that admits it does not know which
/// revision it describes.
///
/// The full hash rather than a short one: the short form is what a document quotes to a reader,
/// and this is what a machine compares.
///
/// # Panics
///
/// Never.
fn commit_of(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let hash = String::from_utf8(output.stdout).ok()?;
    let hash = hash.trim();
    (!hash.is_empty()).then(|| hash.to_owned())
}
