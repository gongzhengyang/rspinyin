//! The size budgets a built file is measured against.
//!
//! Responsibility: hold the two thresholds the specification states for the release
//! artifacts, read them from the machine-readable budget document, and measure one file
//! against one of them. It decides nothing about *which* files are measured -- the
//! payload table says that -- and it strips nothing: what it measures is whatever the
//! caller hands it.
//!
//! The thresholds are read from `docs/dev/budgets.json` rather than written into this
//! file, so that a budget change reaches the installer, the packager, the benchmarks and
//! the specification's table together.

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use super::layout::Sources;

/// Which size budget a payload is measured against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Budget {
    /// `BUDGET-SIZE-01`: the release addon library, measured after stripping.
    StrippedLibrary,
    /// `BUDGET-SIZE-02`: the built-in base dictionary.
    BaseDictionary,
}

impl Budget {
    /// The budget identifier as the specification writes it.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::StrippedLibrary => "BUDGET-SIZE-01",
            Self::BaseDictionary => "BUDGET-SIZE-02",
        }
    }
}

/// Size thresholds, read from the machine-readable budget document.
pub(crate) struct SizeBudgets {
    /// `BUDGET-SIZE-01`, in mebibytes.
    pub(crate) library_mb: f64,
    /// `BUDGET-SIZE-02`, in mebibytes.
    pub(crate) dictionary_mb: f64,
}

impl SizeBudgets {
    /// Reads the thresholds from `docs/dev/budgets.json`.
    ///
    /// Read rather than written into this file so that a budget change reaches the
    /// installer, the packager, the benchmarks and the specification's table together.
    ///
    /// # Errors
    ///
    /// Returns an error when the document cannot be read or parsed.
    pub(crate) fn load(sources: &Sources) -> Result<Self> {
        let path = sources.root.join(crate::budget::BUDGETS_FILE);
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let budgets = crate::budget::Budgets::from_json(&text)
            .with_context(|| format!("{}: invalid budget document", path.display()))?;
        Ok(Self {
            library_mb: budgets.size_mb.so_stripped,
            dictionary_mb: budgets.size_mb.base_dict,
        })
    }

    /// The threshold one budget names, in mebibytes.
    pub(crate) fn limit(&self, budget: Budget) -> f64 {
        match budget {
            Budget::StrippedLibrary => self.library_mb,
            Budget::BaseDictionary => self.dictionary_mb,
        }
    }
}

/// Fails when a file passes its size budget.
///
/// A budget is a gate rather than a warning: the specification's thresholds are
/// contractual, so a regression past one stops the install instead of being printed
/// and forgotten.
///
/// # Errors
///
/// Returns an error naming the budget, the measured size and the threshold.
pub(super) fn check_size(budget: Budget, path: &Path, limit_mb: f64) -> Result<()> {
    check_size_in("install", budget, path, limit_mb)
}

/// Fails when a file passes its size budget, reporting under `scope`.
///
/// `scope` is the command the measurement belongs to (`install`, `package`), so a
/// failure says which of the two stopped rather than leaving the reader to guess from
/// the paths.
///
/// # Errors
///
/// Returns an error naming the budget, the measured size and the threshold.
pub(crate) fn check_size_in(scope: &str, budget: Budget, path: &Path, limit: f64) -> Result<()> {
    let bytes = fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .len();
    let size_mb = bytes as f64 / (1024.0 * 1024.0);
    if size_mb > limit {
        report_sections(scope, path);
        anyhow::bail!(
            "{scope}: {} failed: {} is {size_mb:.2}MiB, over the {limit:.2}MiB threshold",
            budget.id(),
            path.display()
        );
    }
    println!(
        "{scope}: {} {} is {size_mb:.2}MiB, within {limit:.2}MiB",
        budget.id(),
        path.display()
    );
    Ok(())
}

/// Prints a per-section size breakdown, when a tool that can produce one is installed.
///
/// Neither `bloaty` nor `cargo-bloat` is a build dependency, so a machine without them
/// gets the measured size and nothing more rather than a failure for the wrong reason.
fn report_sections(scope: &str, path: &Path) {
    let breakdown = Command::new("bloaty")
        .arg("-n")
        .arg("0")
        .arg("-d")
        .arg("sections")
        .arg(path)
        .output();
    match breakdown {
        Ok(output) if output.status.success() => {
            println!(
                "{scope}: per-section breakdown\n{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
        _ => println!(
            "{scope}: no `bloaty` on this system, so no per-section breakdown; \
             `cargo bloat` against the build tree is the alternative"
        ),
    }
}
