//! Per-section size accounting for the compiled dictionary.
//!
//! Responsibility: turn the payload sizes one compilation produced into a verdict
//! against the section ceilings the architecture spec states (`ASM-05`), and render
//! the table the size gate parses. The five payload sections are accounted
//! separately because they fail for different reasons -- `ENTRIES` grows with the
//! word count, `WORDLIST` grows with the L3b multi-key expansion, `FST` grows with
//! the key count -- so a single total tells a reader nothing about which input
//! caused the regression.
//!
//! Boundaries: this layer never builds a section and never reads a raw source. It
//! reads exactly one number from outside, the container ceiling in
//! `docs/dev/budgets.json`, and derives every other ceiling from it.
//!
//! # Where the ceilings come from
//!
//! `docs/dev/budgets.json` holds the one authoritative size threshold,
//! `size_mb.base_dict` (`BUDGET-SIZE-02`). `ASM-05` splits that container into five
//! payload ceilings plus the room the header, the section table and the alignment
//! padding need. Both are stated below as shares of the container ceiling in
//! per-mille, which is what makes a change to the budget document move every ceiling
//! together instead of leaving a stale copy of a number behind.
//!
//! The section ceilings are individually loose on purpose: a build can sit inside
//! every one of them and still be over the payload total, and the total is the
//! binding constraint. Failing on it is the intended behaviour -- the remedy `ASM-05`
//! names is to truncate the lowest-weight words, not to raise the ceiling.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, anyhow, ensure};
use ime_dict::format::SectionKind;
use serde::Serialize;

use super::build::Compiled;
use crate::budget::{BUDGETS_FILE, read_budgets};

/// Payload section ceilings as per-mille shares of the container ceiling.
///
/// At the container ceiling `docs/dev/budgets.json` states, these give 4 MiB, 6.5 MiB,
/// 2 MiB, 3.8 MiB and 3.5 MiB. The order is the order the `--stats` table lists the
/// rows in, which is the order the size gate parses, so it is a build-time contract
/// rather than a presentation choice.
///
/// The `Fst` share is the one measurement moved. `ASM-05` originally estimated it at
/// 125 per-mille (2.5 MiB) from a ~5.9 B/key density guess; the first full-scale build
/// measured 11.65 B/key over 292,315 keys (3,404,030 bytes), and even the expansion-free
/// baseline needs ~2.9 MB -- the original ceiling is unreachable at the entry floor, let
/// alone the 400k-entry window. 190 per-mille (3.8 MiB) fits that window's worst case
/// (~335k keys) with headroom, and `docs/dev/features.md` `ASM-05` states the same
/// number.
const SECTION_SHARES: [(SectionKind, u32); 5] = [
    (SectionKind::StrPool, 200),
    (SectionKind::Entries, 325),
    (SectionKind::WordList, 100),
    (SectionKind::Fst, 190),
    (SectionKind::Unigram, 175),
];

/// Share of the container ceiling the five payload sections may use together.
///
/// `ASM-05` leaves 2.5 MiB of a 20 MiB container to the header, the section table and
/// the alignment padding, which is 875 per-mille of the container. The section shares
/// sum to more than that on purpose -- 19.8 MiB against a 17.5 MiB payload budget --
/// because a build inside every section ceiling and past the payload total has to fail
/// on the total, and a build past one section ceiling says which input grew.
const PAYLOAD_SHARE: u32 = 875;

/// Denominator of every share in this module.
const PER_MILLE: u64 = 1000;

/// Width of one `WORDLIST` slot: a `u32` word id.
///
/// The section is a bare `[u32; pair_count]`, so its entry count is its byte length
/// divided by this.
const WORD_ID_SIZE: usize = 4;

/// Cell a row with no entry count shows.
const ABSENT: &str = "—";

/// The byte ceilings one compiled container is measured against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentBudgets {
    /// `BUDGET-SIZE-02`: the ceiling on the whole `base.dict` container.
    container_bytes: u64,
    /// The ceiling on the five payload sections together.
    payload_bytes: u64,
    /// The ceiling on each payload section.
    sections: BTreeMap<SectionKind, u64>,
}

impl SegmentBudgets {
    /// Derives every ceiling from one container ceiling.
    ///
    /// A container ceiling of 20 MiB, the value `docs/dev/budgets.json` states,
    /// yields the `ASM-05` table: 4 MiB, 6.5 MiB, 2 MiB, 3.8 MiB and 3.5 MiB per
    /// section. Those five add up to 19.8 MiB, which is *more* than the 17.5 MiB the
    /// five may use together -- see [`PAYLOAD_SHARE`] -- and the difference is the
    /// point: the section ceilings are loose so that a build past one of them says
    /// which input grew, while the total is the ceiling a build has to respect.
    pub fn from_container_ceiling(container_bytes: u64) -> Self {
        let sections = SECTION_SHARES
            .iter()
            .map(|(kind, share)| (*kind, share_of(container_bytes, *share)))
            .collect();
        Self {
            container_bytes,
            payload_bytes: share_of(container_bytes, PAYLOAD_SHARE),
            sections,
        }
    }

    /// Reads `docs/dev/budgets.json` and derives every ceiling from it.
    ///
    /// `root` is the repository root; the document is resolved relative to it.
    ///
    /// # Errors
    /// Returns an error when the budget document cannot be read, does not satisfy its
    /// schema, or states a `size_mb.base_dict` that is not a usable byte count.
    pub fn load(root: &Path) -> Result<Self> {
        let budgets = read_budgets(root)?;
        let path = root.join(BUDGETS_FILE);
        let container_bytes = mebibytes_to_bytes(budgets.size_mb.base_dict).with_context(|| {
            format!(
                "{}: `size_mb.base_dict` is not a usable container ceiling",
                path.display()
            )
        })?;
        Ok(Self::from_container_ceiling(container_bytes))
    }

    /// The container ceiling the section ceilings were derived from.
    pub fn container(&self) -> u64 {
        self.container_bytes
    }

    /// The ceiling on the five payload sections together.
    pub fn total(&self) -> u64 {
        self.payload_bytes
    }

    /// The ceiling on one payload section, or `None` for a section no budget names.
    pub fn section(&self, kind: SectionKind) -> Option<u64> {
        self.sections.get(&kind).copied()
    }

    /// Every budgeted section with its ceiling, in the order the table lists them.
    pub fn sections(&self) -> Vec<(SectionKind, u64)> {
        SECTION_SHARES
            .iter()
            .filter_map(|(kind, _)| self.section(*kind).map(|cap| (*kind, cap)))
            .collect()
    }
}

/// One payload section's measured size, and the entry count it holds when it has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SectionUsage {
    /// Which payload section this row accounts for.
    ///
    /// Serialised as the name the container's own diagnostics use, because the
    /// contract type carries no serde derive and a reader of the manifest should not
    /// have to know the container's discriminant numbering.
    #[serde(serialize_with = "serialize_kind")]
    pub kind: SectionKind,
    /// Entries the section holds; `None` for a section that is pure bytes.
    pub entries: Option<u64>,
    /// Bytes the section occupies in the container.
    pub bytes: u64,
}

/// What a ledger is measured against: one payload section, or all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// A single payload section.
    Section(SectionKind),
    /// Every payload section together.
    Total,
}

impl Scope {
    /// The label this scope is reported under.
    ///
    /// These are the labels the size gate parses, so they are a build-time contract.
    /// They are deliberately not [`SectionKind::name`]: that one spells the sections
    /// the way the container's diagnostics do (`STRPOOL`), and this one the way the
    /// table does (`StrPool`).
    pub fn label(self) -> &'static str {
        match self {
            Scope::Total => "TOTAL",
            Scope::Section(SectionKind::Fst) => "Fst",
            Scope::Section(SectionKind::Entries) => "Entries",
            Scope::Section(SectionKind::StrPool) => "StrPool",
            Scope::Section(SectionKind::Unigram) => "Unigram",
            Scope::Section(SectionKind::Bigram) => "Bigram",
            Scope::Section(SectionKind::WordList) => "WordList",
        }
    }
}

/// One ceiling a ledger is past.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Violation {
    /// The ceiling that was exceeded.
    pub scope: Scope,
    /// Bytes the build measured.
    pub bytes: u64,
    /// Bytes the ceiling allows.
    pub budget: u64,
}

impl Violation {
    /// How far past the ceiling the build is.
    pub fn excess(&self) -> u64 {
        self.bytes.saturating_sub(self.budget)
    }
}

/// The section sizes one compilation produced, and the ceilings they are measured
/// against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ledger {
    /// The ceilings every row is measured against.
    budgets: SegmentBudgets,
    /// The rows recorded so far, in the order they were recorded.
    rows: Vec<SectionUsage>,
}

impl Ledger {
    /// Creates an empty ledger measured against `budgets`.
    pub fn new(budgets: SegmentBudgets) -> Self {
        Self {
            budgets,
            rows: Vec::new(),
        }
    }

    /// The ceilings this ledger is measured against.
    pub fn budgets(&self) -> &SegmentBudgets {
        &self.budgets
    }

    /// Records one payload section.
    ///
    /// # Errors
    /// Returns an error when the section was already recorded. A section emitted
    /// twice would be counted twice in the total and reported twice in the table, so
    /// it is refused rather than merged.
    pub fn record(&mut self, kind: SectionKind, entries: Option<u64>, bytes: u64) -> Result<()> {
        ensure!(
            self.usage_of(kind).is_none(),
            "{} is recorded twice in the size ledger",
            kind.name()
        );
        self.rows.push(SectionUsage {
            kind,
            entries,
            bytes,
        });
        Ok(())
    }

    /// The sections recorded so far, in the order they were recorded.
    pub fn usage(&self) -> &[SectionUsage] {
        &self.rows
    }

    /// The row recorded for one section.
    pub fn usage_of(&self, kind: SectionKind) -> Option<&SectionUsage> {
        self.usage().iter().find(|row| row.kind == kind)
    }

    /// Every payload section's bytes added up.
    pub fn total_bytes(&self) -> u64 {
        self.rows
            .iter()
            .fold(0u64, |total, row| total.saturating_add(row.bytes))
    }

    /// The ceiling the five payload sections share.
    pub fn total_budget(&self) -> u64 {
        self.budgets.total()
    }

    /// Every ceiling the build is past, sections in table order and the total last.
    pub fn violations(&self) -> Vec<Violation> {
        let mut violations = Vec::new();
        for (kind, cap) in self.budgets.sections() {
            let Some(row) = self.usage_of(kind) else {
                continue;
            };
            if row.bytes > cap {
                violations.push(Violation {
                    scope: Scope::Section(kind),
                    bytes: row.bytes,
                    budget: cap,
                });
            }
        }
        let total = self.total_bytes();
        let cap = self.total_budget();
        if total > cap {
            violations.push(Violation {
                scope: Scope::Total,
                bytes: total,
                budget: cap,
            });
        }
        violations
    }

    /// Whether every section and the payload total are inside their ceilings.
    pub fn is_within(&self) -> bool {
        self.violations().is_empty()
    }

    /// Renders the section table.
    ///
    /// Columns are fixed-width so a parser can address them by position, and every
    /// count carries thousands separators so a human can read the magnitudes at a
    /// glance. The table has no trailing newline.
    pub fn render(&self) -> String {
        let mut table = format!(
            "{:<9} {:>9} {:>11} {:>11}",
            "section", "entries", "bytes", "budget"
        );
        for (kind, cap) in self.budgets.sections() {
            let row = self.usage_of(kind);
            table.push('\n');
            table.push_str(&row_of(
                Scope::Section(kind).label(),
                row.and_then(|row| row.entries),
                row.map_or(0, |row| row.bytes),
                cap,
            ));
        }
        table.push('\n');
        table.push_str(&row_of(
            Scope::Total.label(),
            None,
            self.total_bytes(),
            self.total_budget(),
        ));
        table
    }
}

/// Accounts one compiled container against the per-section budgets.
///
/// The entry counts come from the compiler's own counters: `ENTRIES` and `UNIGRAM`
/// hold one row per word, and `WORDLIST` holds one `u32` per (key, word) pair, which
/// is what makes it the section the L3b expansion grows.
///
/// # Errors
/// Returns an error when a section is recorded twice.
pub fn account(compiled: &Compiled, budgets: &SegmentBudgets) -> Result<Ledger> {
    let mut ledger = Ledger::new(budgets.clone());
    let words = compiled.stats.words;
    ledger.record(SectionKind::Fst, None, compiled.fst.len() as u64)?;
    ledger.record(
        SectionKind::Entries,
        Some(words),
        compiled.entries.len() as u64,
    )?;
    ledger.record(SectionKind::StrPool, None, compiled.strpool.len() as u64)?;
    ledger.record(
        SectionKind::Unigram,
        Some(words),
        compiled.unigram.len() as u64,
    )?;
    ledger.record(
        SectionKind::WordList,
        Some((compiled.wordlist.len() / WORD_ID_SIZE) as u64),
        compiled.wordlist.len() as u64,
    )?;
    Ok(ledger)
}

/// Accounts one compiled container, prints the section table, and fails the build
/// when a ceiling is exceeded.
///
/// The table is printed before the verdict so a failing run leaves the numbers that
/// explain it in the log, rather than only the name of the section that failed, and
/// the line above it names the document the ceilings came from so a reader can see
/// which budget a build was measured against.
///
/// # Errors
/// Returns an error when `docs/dev/budgets.json` cannot be read, when a section is
/// recorded twice, or when any section or the payload total is past its ceiling.
pub fn enforce(root: &Path, compiled: &Compiled) -> Result<Ledger> {
    let budgets = SegmentBudgets::load(root)?;
    let ledger = account(compiled, &budgets)?;
    println!(
        "dictc: section budgets derived from {} (container {} bytes, payload {} bytes)",
        BUDGETS_FILE,
        ledger.budgets().container(),
        ledger.total_budget()
    );
    println!("{}", ledger.render());
    if !ledger.is_within() {
        let violations = ledger.violations();
        return Err(anyhow!(
            "dictc: {} budget(s) exceeded against {}: {}; the remedy is to truncate the \
             lowest-weight words until the build fits, not to raise the ceiling",
            violations.len(),
            BUDGETS_FILE,
            violations
                .iter()
                .map(|violation| format!(
                    "{} {} bytes over {}",
                    violation.scope.label(),
                    violation.excess(),
                    violation.budget
                ))
                .collect::<Vec<String>>()
                .join("; ")
        ));
    }
    Ok(ledger)
}

/// Renders one table row at the fixed column widths.
fn row_of(label: &str, entries: Option<u64>, bytes: u64, budget: u64) -> String {
    let entries = entries.map_or_else(|| ABSENT.to_owned(), grouped);
    format!(
        "{label:<9} {:>9} {:>11} {:>11}",
        entries,
        grouped(bytes),
        grouped(budget)
    )
}

/// Applies one per-mille share to a byte count.
///
/// The multiplication runs in `u128` so the result is the exact fraction for every
/// ceiling, including one at the type's limit, and it is truncated rather than
/// rounded, so a ceiling can only come out a byte too small, which fails closed.
fn share_of(bytes: u64, share: u32) -> u64 {
    let scaled = u128::from(bytes) * u128::from(share) / u128::from(PER_MILLE);
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

/// Converts a mebibyte count from the budget document into whole bytes.
///
/// Returns `None` for a value that is not a finite, positive, plausible byte count,
/// so a hand-edited document cannot turn into a nonsense ceiling. The upper bound is
/// far past any budget and keeps the conversion exact in `f64`.
fn mebibytes_to_bytes(mebibytes: f64) -> Option<u64> {
    // Bytes in one mebibyte, as the conversion factor.
    const BYTES_PER_MEBIBYTE: f64 = 1024.0 * 1024.0;
    // One tebibyte; no dictionary budget is anywhere near this.
    const MAX_MEBIBYTES: f64 = 1_048_576.0;
    if !mebibytes.is_finite() || mebibytes <= 0.0 || mebibytes > MAX_MEBIBYTES {
        return None;
    }
    Some((mebibytes * BYTES_PER_MEBIBYTE).round() as u64)
}

/// Renders a count with thousands separators.
///
/// The standard formatting machinery has no grouping flag and a decimal-grouping
/// crate would be a dependency for ten lines, so the digits are grouped here.
fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Serialises a section kind as the name the container's own diagnostics use.
fn serialize_kind<S>(kind: &SectionKind, serializer: S) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(kind.name())
}

#[cfg(test)]
mod tests;
