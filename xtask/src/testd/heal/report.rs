//! What every recovery leaves behind, and how it is rendered.
//!
//! Responsibility: turn a resolution that had to be re-derived into a record a human can review,
//! collect a run's records, and hand the evidence archive the records `assertions.json` holds.
//! Nothing here resolves anything: a record is a rendering of a decision the locators already
//! made.
//!
//! # Why a recovery has to be visible
//!
//! A locator that quietly re-derived itself would hide exactly the drift the case exists to
//! catch, so a recovery is never silent: it names what the case asked for, what the source
//! declares now, and the specification rows the derivation is anchored on. Writing the files is
//! the report channel's job -- this module produces values and writes nothing.
//!
//! # One shape per file
//!
//! `assertions.json` holds the archive's own record type, so a recovery reaches that file as a
//! [`evidence::HealRecord`] and never as a second shape under the same key: the archive owns the
//! document and this module supplies the values it holds. A batch's `index.md` takes the summary
//! [`HealLog::index_markdown`] renders, which is the same decision in prose.

use super::binding::{ResolvedConst, SpecAnchor, bare_heading, unit_name};
use super::hit::{HitQuery, ResolvedHit, describe_target, point_label};
use crate::testd::evidence;
use crate::testd::ui_metrics::SPEC_DOCUMENT;

/// The marker every recovery carries, in the report and in the log.
pub const HEAL_MARKER: &str = "[HEALED]";

/// Why a locator was re-derived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HealCause {
    /// A constant is declared under a name built on the one the locator used.
    Renamed {
        /// The name the locator used, which the metrics block no longer declares.
        from: String,
        /// The name the metrics block declares now.
        to: String,
    },
    /// A point the case recorded is not the point the specification's constants produce.
    Reprojected {
        /// The point the case recorded, in container pixels.
        from: (i32, i32),
        /// The point the derivation produces, in container pixels.
        to: (i32, i32),
    },
}

/// One specification row, as a report writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnchorView {
    /// The heading of the section the row lives in, as the document spells it.
    pub section: &'static str,
    /// The row's element label, as the document spells it.
    pub row: &'static str,
    /// The unit the row states the value in: `dp`, `sp` or `count`.
    pub unit: &'static str,
    /// The value the row states, rendered the way the document states it.
    pub value: String,
}

impl AnchorView {
    /// The view of `anchor`.
    ///
    /// # Panics
    ///
    /// Never.
    fn of(anchor: &SpecAnchor) -> Self {
        Self {
            section: anchor.section,
            row: anchor.row,
            unit: unit_name(anchor.unit),
            value: anchor.value.describe(),
        }
    }

    /// The row as one line: where it is, what it is called and what it states.
    ///
    /// # Panics
    ///
    /// Never.
    fn line(&self) -> String {
        format!(
            "{} {} = {}",
            bare_heading(self.section),
            self.row,
            self.value
        )
    }
}

/// One recovery: what a locator named, what it had to change, and why that is the same thing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealRecord {
    /// The marker every record carries, so a reader of the report can grep for it.
    pub marker: &'static str,
    /// What the case named.
    pub locator: String,
    /// What changed.
    pub cause: HealCause,
    /// The specification rows the derivation is anchored on.
    pub evidence: Vec<AnchorView>,
}

impl HealRecord {
    /// The record as the one line a batch's `index.md` carries.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn line(&self) -> String {
        let evidence = self
            .evidence
            .iter()
            .map(AnchorView::line)
            .collect::<Vec<String>>()
            .join("; ");
        let marker = self.marker;
        let locator = &self.locator;
        match &self.cause {
            HealCause::Renamed { from, to } => {
                format!("- {marker} {locator}: `{from}` -> `{to}`; anchored on {evidence}")
            }
            HealCause::Reprojected { from, to } => format!(
                "- {marker} {locator}: {} -> {}; anchored on {evidence}",
                point_label(*from),
                point_label(*to)
            ),
        }
    }
}

/// The recoveries of one run, in the order they happened.
///
/// The log is the report side of healing and nothing else: it holds no state a locator reads, so
/// a case that ignores it resolves exactly the same way. It hands the archive the records
/// `assertions.json` carries, through [`HealLog::evidence`], and renders the summary a batch's
/// `index.md` holds, through [`HealLog::index_markdown`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HealLog {
    /// The recoveries, oldest first.
    records: Vec<HealRecord>,
}

impl HealLog {
    /// An empty log.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the record of `resolved`, when it was healed.
    ///
    /// # Returns
    ///
    /// Whether a record was added. A constant that resolved by name adds nothing, which is the
    /// point: only a recovery is worth reviewing.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn note(&mut self, resolved: &ResolvedConst) -> bool {
        match record_of(resolved) {
            Some(record) => {
                self.records.push(record);
                true
            }
            None => false,
        }
    }

    /// Adds the records of `hit`, when it was healed.
    ///
    /// # Returns
    ///
    /// Whether a record was added. A hit adds one record per constant whose name was re-derived,
    /// and one more when the point itself moved.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn note_hit(&mut self, query: &HitQuery, hit: &ResolvedHit) -> bool {
        let mut noted = false;
        for constant in &hit.constants {
            noted |= self.note(constant);
        }
        // A point that moved is recorded only when the locator healed, which is to say only when
        // the specification explains the move; a move it does not explain is the drift the case
        // has to fail on, and recording it as a recovery would hide exactly that.
        let moved = hit.moved_from.filter(|_| hit.healed);
        if let Some(from) = moved {
            self.records.push(HealRecord {
                marker: HEAL_MARKER,
                locator: describe_target(query.target),
                cause: HealCause::Reprojected {
                    from,
                    to: hit.point,
                },
                evidence: hit
                    .constants
                    .iter()
                    .map(|constant| AnchorView::of(&constant.anchor))
                    .collect(),
            });
            noted = true;
        }
        noted
    }

    /// The recoveries, oldest first.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn records(&self) -> &[HealRecord] {
        &self.records
    }

    /// How many recoveries the run produced.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the run re-derived nothing.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The recoveries as the records `assertions.json` holds.
    ///
    /// The archive owns that shape -- the name a locator used, the name the source declares now,
    /// and the citation that settles the question -- and this is the one place a recovery becomes
    /// one of its records, so the file has a single writer and a single shape. The citation is
    /// built here rather than by a caller for the same reason: a recovery that reached the file
    /// without one would be a recovery nobody can review.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn evidence(&self) -> Vec<evidence::HealRecord> {
        self.records.iter().map(archive_record).collect()
    }

    /// The log as the section a batch's `index.md` carries.
    ///
    /// A run that re-derived nothing says so rather than leaving the section out, because a
    /// missing section and an empty one read the same way to a reviewer.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn index_markdown(&self) -> String {
        if self.records.is_empty() {
            return format!(
                "No locator was re-derived; {HEAL_MARKER} appears nowhere in this run.\n"
            );
        }
        let mut text = format!(
            "## {HEAL_MARKER} {} locator(s) re-derived\n\n",
            self.records.len()
        );
        for record in &self.records {
            text.push_str(&record.line());
            text.push('\n');
        }
        text
    }
}

/// The record of `resolved`, when it was healed.
///
/// # Panics
///
/// Never.
fn record_of(resolved: &ResolvedConst) -> Option<HealRecord> {
    let from = resolved.renamed_from?;
    Some(HealRecord {
        marker: HEAL_MARKER,
        locator: format!("the constant {from}"),
        cause: HealCause::Renamed {
            from: from.to_owned(),
            to: resolved.name.clone(),
        },
        evidence: vec![AnchorView::of(&resolved.anchor)],
    })
}

/// One recovery as the archive holds it.
///
/// A rename is the two names; a reprojection is the two points, rendered the way every other
/// point in a report is. The citation is the same for both, because it answers the same question:
/// which rows of the specification say that the new value is the old one.
///
/// # Panics
///
/// Never.
fn archive_record(record: &HealRecord) -> evidence::HealRecord {
    let (old, new) = match &record.cause {
        HealCause::Renamed { from, to } => (from.clone(), to.clone()),
        HealCause::Reprojected { from, to } => (point_label(*from), point_label(*to)),
    };
    evidence::HealRecord {
        old,
        new,
        basis: citation(&record.evidence),
    }
}

/// The rows a recovery is anchored on, as the one citation line the archive holds.
///
/// The document is named as the repository holds it rather than by a second spelling written
/// here, so a citation names the file a reader opens.
///
/// # Panics
///
/// Never.
fn citation(anchors: &[AnchorView]) -> String {
    let rows: Vec<String> = anchors.iter().map(AnchorView::line).collect();
    format!("{SPEC_DOCUMENT} {}", rows.join("; "))
}
