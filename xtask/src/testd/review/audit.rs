//! What a reviewer submitted, and the rules that decide whether it is evidence.
//!
//! Responsibility: hold one item's answer -- its verdict, the coordinate it was sampled at, what
//! the pixels read there and what the specification states -- and judge a whole submission against
//! the checklist. Nothing here renders a prompt or reads an image: a submission arrives as values,
//! which is what makes the rules below testable without a snapshot.
//!
//! # The three states, and why there are three
//!
//! A verdict is [`Verdict`], the archive's own three-state vocabulary, so a visual audit and an
//! assertion end up in the same document in the same words. The third state is the one that
//! matters here: a compositor's blur, a real transparent surface and a multi-head desktop cannot
//! be read on a machine that offers none of them, and `get_image` returns what was drawn rather
//! than what a compositor would have blended. A reviewer that had only two answers would have to
//! call that a pass, and a run on a machine without a compositor would report a clean audit of a
//! window it never saw.
//!
//! [`Summary`] therefore counts the three states separately and reports a run as clean only when
//! every item was judged and none failed. Folding "could not judge" into either of the other two
//! is the one arithmetic error this module exists to prevent, and it is prevented in one place
//! rather than in every report that reads the counts.
//!
//! # Why a verdict without a coordinate is refused
//!
//! "The corner radius looks right" is a sentence about the reviewer, not about the snapshot: it
//! cannot be re-derived from the PNG and cannot be argued with. Requiring a coordinate, a measured
//! value and the value the specification states makes every answer checkable by a second reader
//! with the same file, which is the whole point of auditing a rendered window rather than
//! describing it. An answer that carries fewer than the three is refused rather than counted, and
//! a refusal is visible in the summary rather than silently dropped.

use super::CheckItem;
use crate::testd::evidence::Verdict;

/// One sampling point of a snapshot, and what was read there.
///
/// The coordinates are the snapshot's own physical pixels, counted from its top-left corner, which
/// is the only frame of reference both a reviewer and a script reading the same PNG share.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    /// X, counted from the snapshot's left edge, in physical pixels.
    pub x: i32,
    /// Y, counted from the snapshot's top edge, in physical pixels.
    pub y: i32,
    /// What the pixels at that point read, in the reviewer's own units.
    pub measured: String,
    /// What the specification states for the item, at the same scale.
    pub expected: String,
}

/// One item's verdict, as a reviewer submitted it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The identifier of the checklist item this answers.
    pub item: String,
    /// What the reviewer decided.
    pub verdict: Verdict,
    /// Where the verdict was sampled, when it was sampled at all.
    pub sample: Option<Sample>,
    /// Why the item could not be judged; required of an unverifiable verdict.
    pub reason: String,
}

impl Finding {
    /// Why this finding is not admissible evidence, or `None` when it is.
    ///
    /// A pass and a failure are held to the same standard, because both are claims about pixels: a
    /// failure without a coordinate is as unreviewable as a pass without one, and the one that
    /// matters more is the failure -- it is the one that costs somebody a day. An unverifiable
    /// verdict is held to the other standard: it needs no coordinate, since the whole point is
    /// that the pixels could not be read, but it needs a reason, because otherwise it cannot be
    /// told from an item nobody attempted.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn refusal(&self) -> Option<Refusal> {
        match self.verdict {
            Verdict::Unverifiable => {
                if self.reason.trim().is_empty() {
                    Some(Refusal::NoReason)
                } else {
                    None
                }
            }
            Verdict::Pass | Verdict::Fail => match &self.sample {
                None => Some(Refusal::NoSample),
                Some(sample)
                    if sample.measured.trim().is_empty() || sample.expected.trim().is_empty() =>
                {
                    Some(Refusal::NoValues)
                }
                Some(_) => None,
            },
        }
    }
}

/// Why a submitted finding is not admissible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A pass or a failure that rests on no sampling coordinate.
    NoSample,
    /// A pass or a failure that carries no measured or expected value.
    NoValues,
    /// A verdict of "could not judge" that does not say why.
    NoReason,
    /// A finding about an item the checklist does not ask about.
    UnknownItem,
}

impl Refusal {
    /// The refusal as a report prints it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn describe(self) -> &'static str {
        match self {
            Self::NoSample => "the verdict carries no sampling coordinate",
            Self::NoValues => "the verdict carries no measured or expected value",
            Self::NoReason => "the item was not judged and the finding does not say why",
            Self::UnknownItem => "the finding names an item the checklist does not ask about",
        }
    }
}

/// What a submitted audit adds up to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    /// Findings that passed with admissible evidence.
    pub passed: usize,
    /// Findings the pixels contradicted.
    pub failed: usize,
    /// Findings the reviewer could not judge.
    pub unverifiable: usize,
    /// The findings that were not admissible, each with the item it named.
    pub refused: Vec<(String, Refusal)>,
    /// The identifiers of the items the submission never answered.
    pub missing: Vec<String>,
}

impl Summary {
    /// Judges `submission` against `items`.
    ///
    /// Every finding is counted at most once, and a finding that was refused is counted nowhere
    /// else: a submission that answered an item with a coordinate-less verdict has not answered
    /// it, and counting the verdict as a pass would be the exact failure the refusal exists to
    /// prevent.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn of(submission: &[Finding], items: &[CheckItem]) -> Self {
        let mut summary = Self {
            passed: 0,
            failed: 0,
            unverifiable: 0,
            refused: Vec::new(),
            missing: Vec::new(),
        };
        for finding in submission {
            if !items.iter().any(|item| item.id == finding.item) {
                summary
                    .refused
                    .push((finding.item.clone(), Refusal::UnknownItem));
                continue;
            }
            if let Some(refusal) = finding.refusal() {
                summary.refused.push((finding.item.clone(), refusal));
                continue;
            }
            match finding.verdict {
                Verdict::Pass => summary.passed += 1,
                Verdict::Fail => summary.failed += 1,
                Verdict::Unverifiable => summary.unverifiable += 1,
            }
        }
        summary.missing = items
            .iter()
            .filter(|item| !submission.iter().any(|finding| finding.item == item.id))
            .map(|item| String::from(item.id))
            .collect();
        summary
    }

    /// Whether the audit passed.
    ///
    /// Only an audit in which every item was answered, every answer was admissible, no answer
    /// failed and no answer was left unjudged counts as clean. An unverifiable item therefore
    /// keeps the audit from passing: it is not a failure, but it is not a pass either, and the run
    /// that produced it has to say which machine would have to judge it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_clean(&self) -> bool {
        self.failed == 0
            && self.unverifiable == 0
            && self.refused.is_empty()
            && self.missing.is_empty()
    }

    /// The summary as a report prints it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn describe(&self) -> String {
        format!(
            "{} passed, {} failed, {} unverifiable, {} refused, {} unanswered",
            self.passed,
            self.failed,
            self.unverifiable,
            self.refused.len(),
            self.missing.len()
        )
    }
}
