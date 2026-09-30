//! The memory-budget gate: a running plugin's readings against the document.
//!
//! Responsibility: read the memory section of a probe snapshot, turn it into the three
//! growths the memory budgets are stated for, and judge each one against the ceiling
//! `docs/dev/budgets.json` carries. A growth past its ceiling, and a budget the document
//! states that nothing measured, are both failures of the gate rather than lines a
//! reviewer has to notice.
//!
//! # Why this is not `--validate`
//!
//! `--validate` compares the document against the authoritative table in the
//! specification and says nothing about what a run measured. This is the other half:
//! the numbers a running plugin recorded, against the numbers the document states.
//!
//! # `Missing` is not `Pass`
//!
//! Every field of a [`MemorySnapshot`] is an `Option`, and an empty one means nothing
//! measured that growth. A budget the document states whose growth nobody measured is
//! reported as unmeasured and fails the gate: "nothing was measured, therefore nothing
//! is wrong" is the reading this gate exists to refuse, and a memory budget that a
//! missing reading turned into a pass would be exactly that.
//!
//! # The ceilings are read, never restated
//!
//! The binding table names *which* growth each budget is stated for and nothing else;
//! every ceiling comes out of the document. A key the table names and the document does
//! not carry is reported as a defect of this module rather than skipped, and so is a
//! `memory_mb.*` key the document carries that the table does not name -- a threshold
//! nothing asserts is a threshold nobody keeps.
//!
//! # The growths are the snapshot's, not this module's
//!
//! Which reading a growth is the difference between is fixed by the design and computed
//! by [`MemorySnapshot`], where the baselines live. This module states which of those
//! growths a budget is about, converts kibibytes to the mebibytes the document is stated
//! in, and compares.

// Nothing here is reachable from `xtask`'s subcommand tree yet: the wiring that would
// call it lives in `xtask/src/main.rs`, which declares the `--memory` flag, and that
// file is the module root's rather than this module's. Until it lands every item here is
// reported as dead code in a non-test build, and the attribute goes away with those
// lines.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use ime_diag::probe::{MemorySnapshot, ProbeSnapshot, SNAPSHOT_FILE_NAME};
use ime_dict::paths::{BaseDirs, Paths};

use super::{BUDGETS_FILE, Budgets, bench, repo_root};

/// The prefix of the budget document's section that holds the memory ceilings.
const MEMORY_SECTION: &str = "memory_mb.";

/// Kibibytes in one mebibyte.
///
/// The document states its memory ceilings in mebibytes (its own section name says so)
/// and a reading is in kibibytes, which is the unit the kernel reports. The two units
/// meet here and nowhere else.
const KIB_PER_MIB: f64 = 1_024.0;

/// Which growth of the process a memory budget is stated for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    /// The whole plugin process, since the probes were created.
    Plugin,
    /// The UI rendering layer, since it marked its baseline.
    Ui,
    /// The dictionary mapping's private pages, since it was mapped.
    Dictionary,
}

impl Scope {
    /// The growth this scope measures, in kibibytes, or `None` when nothing measured it.
    fn growth_kib(self, memory: &MemorySnapshot) -> Option<u64> {
        match self {
            Self::Plugin => memory.plugin_growth_kib(),
            Self::Ui => memory.ui_growth_kib(),
            Self::Dictionary => memory.dictionary_growth_kib(),
        }
    }

    /// What a report says about a scope whose reading the snapshot does not carry.
    ///
    /// The message names the mark that would have taken the baseline, because an
    /// unmeasured budget is a wiring that has not happened yet and the reader of the
    /// failure is the person who can do it.
    const fn unmeasured(self) -> &'static str {
        match self {
            Self::Plugin => {
                "the snapshot carries no process reading; the probes take one when they are created"
            }
            Self::Ui => {
                "the snapshot carries no UI baseline; mark one with `Probes::mark_ui_baseline` \
                 when the UI layer starts"
            }
            Self::Dictionary => {
                "the snapshot carries no dictionary baseline; mark one with \
                 `Probes::mark_dictionary_baseline` once the mapping is in place"
            }
        }
    }
}

/// One memory budget, and the growth it is stated for.
///
/// Both fields are names: the dotted path of the threshold in the budget document, and
/// which reading it is a growth of. Neither is a number, so this table cannot restate a
/// ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MemoryBinding {
    /// Dotted path of the threshold in the budget document.
    key: &'static str,
    /// The growth the threshold is stated for.
    scope: Scope,
}

/// The memory budgets the gate asserts.
///
/// The three are the document's `memory_mb` section, and each is a growth the snapshot
/// carries: the plugin's resident growth since it loaded, the UI layer's since it
/// started, and the dictionary mapping's private growth since it was mapped.
const BINDINGS: &[MemoryBinding] = &[
    MemoryBinding {
        key: "memory_mb.plugin_rss",
        scope: Scope::Plugin,
    },
    MemoryBinding {
        key: "memory_mb.ui_rss",
        scope: Scope::Ui,
    },
    MemoryBinding {
        key: "memory_mb.dict_mmap_rss",
        scope: Scope::Dictionary,
    },
];

/// Outcome of a memory-budget check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryReport {
    /// Budgets that were measured and are inside their ceiling, one message each.
    pub passed: Vec<String>,
    /// Budgets whose measurement is past its ceiling, one message each.
    pub violations: Vec<String>,
    /// Budgets the document states that the snapshot carries no measurement for, one
    /// message each.
    pub missing: Vec<String>,
}

/// Judges a snapshot's memory section against every memory budget of `budgets`.
///
/// `memory` is the section a snapshot file carried and `budgets` is the validated
/// document. The document is a parameter rather than something this function reads for
/// itself, so a test can drive the comparison with a ceiling it made impossible -- which
/// is what shows the assertion reads the document rather than a constant.
///
/// # Errors
///
/// Returns an error when a binding names a key the document does not carry, and when the
/// document carries a `memory_mb.*` key no binding names. Both are defects in this
/// module rather than in the documents, and both are reported rather than skipped: a
/// budget nobody asserts is a budget nobody keeps.
pub fn judge(memory: &MemorySnapshot, budgets: &Budgets) -> Result<MemoryReport> {
    let mut report = MemoryReport::default();
    for binding in BINDINGS {
        let MemoryBinding { key, scope } = *binding;
        let Some(ceiling_mib) = bench::threshold(budgets, key) else {
            bail!("{key}: bound in the gate's table but absent from {BUDGETS_FILE}");
        };
        let Some(measured_kib) = scope.growth_kib(memory) else {
            report.missing.push(format!(
                "budget/memory-unmeasured: {} {key}: {}",
                bench::owner(key),
                scope.unmeasured()
            ));
            continue;
        };
        let ceiling_kib = ceiling_mib * KIB_PER_MIB;
        let measured = format!(
            "{} {key} {:.2}MiB of {ceiling_mib:.2}MiB",
            bench::owner(key),
            measured_kib as f64 / KIB_PER_MIB
        );
        if measured_kib as f64 <= ceiling_kib {
            report.passed.push(measured);
        } else {
            report
                .violations
                .push(format!("budget/memory-exceeded: {measured}"));
        }
    }
    for threshold in budgets.thresholds() {
        if threshold.0.starts_with(MEMORY_SECTION)
            && !BINDINGS.iter().any(|binding| binding.key == threshold.0)
        {
            bail!(
                "{}: {BUDGETS_FILE} states a memory budget this gate has no measurement for; \
                 add it to the gate's table or remove it from the document",
                threshold.0
            );
        }
    }
    Ok(report)
}

/// Entry point for the memory-budget gate.
///
/// `input` names the snapshot file to read, or `None` for the file the plugin writes
/// under the XDG data directory. The command fails when the file cannot be read or
/// parsed, when a budget is past its ceiling, and when a budget the document states
/// carries no measurement.
///
/// # Errors
///
/// Returns an error when the snapshot cannot be read or does not parse, when the budget
/// document cannot be read or does not satisfy its schema, when the XDG data directory
/// cannot be resolved while looking for the default snapshot, and when the report holds
/// a violation or an unmeasured budget.
pub fn run(input: Option<&Path>) -> Result<()> {
    let text = read_snapshot(input)?;
    // The snapshot's own section is parsed as well, so that a file which is not a
    // snapshot at all is reported as one rather than read for whatever memory records
    // it happens to contain.
    ProbeSnapshot::parse(&text).context("the probe snapshot is unusable")?;
    let memory = MemorySnapshot::parse(&text).context("the memory section is unusable")?;
    let budgets = super::read_budgets(&repo_root()?)?;
    let report = judge(&memory, &budgets)?;
    for line in &report.passed {
        println!("budget: {line} within budget");
    }
    for line in &report.missing {
        println!("budget: {line}");
    }
    ensure!(
        report.violations.is_empty(),
        "{} memory budget(s) past their ceiling:\n  {}",
        report.violations.len(),
        report.violations.join("\n  ")
    );
    ensure!(
        report.missing.is_empty(),
        "{} memory budget(s) the document states were never measured:\n  {}",
        report.missing.len(),
        report.missing.join("\n  ")
    );
    println!(
        "budget: {} memory budget(s) within budget",
        report.passed.len()
    );
    Ok(())
}

/// The snapshot text `input` names, or the plugin's own snapshot when it names none.
fn read_snapshot(input: Option<&Path>) -> Result<String> {
    match input {
        Some(path) => {
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
        }
        None => {
            let path = snapshot_path(&BaseDirs::from_env()?)?;
            fs::read_to_string(&path).with_context(|| {
                format!(
                    "reading {}; the plugin writes it when it receives SIGUSR1",
                    path.display()
                )
            })
        }
    }
}

/// Where the plugin's snapshot file lives in `bases`.
///
/// Derived from the layout the rest of the plugin writes to rather than assembled here,
/// so that the file the plugin writes and the file this gate reads are one path by
/// construction.
///
/// # Errors
///
/// Returns an error when the layout's paths would not fit the kernel's path limit.
fn snapshot_path(bases: &BaseDirs) -> Result<PathBuf> {
    Ok(Paths::from_bases(bases)?.data_dir.join(SNAPSHOT_FILE_NAME))
}
