//! What a step expects, what it saw, and where the two parted company.
//!
//! The expectations are deliberately narrow: a variant covers one part of what the engine
//! shows, and nothing here asserts a *time*. This channel exists so that a contract can be
//! checked without the timing noise a host-driven run carries.

use ime_types::{
    Candidate, CandidateSource, DecodeError, ImeError, KeyAction, PageState, Preedit, Segment,
};
use serde::{Deserialize, Serialize};

use crate::testd::engine::scenario::action_name;

/// What one step must observe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Expectation {
    /// The ordered candidate list.
    Candidates {
        /// Text of the best candidate.
        first: String,
        /// Exact number of candidates.
        count: usize,
        /// Texts that must appear somewhere in the list.
        contains: Vec<String>,
    },
    /// The preedit line the header draws.
    Preedit {
        /// The preedit text.
        text: String,
        /// Caret position, as a byte offset into the text.
        caret: u32,
        /// Exact number of spans.
        spans: usize,
    },
    /// A frozen error code the step must surface.
    ///
    /// A decode never answers with an error: a request it cannot read comes back degraded,
    /// and a keystroke it cannot accept is refused. Both leave a frozen code in the step's
    /// diagnostics, and this variant asserts that the code is there *and* that the step
    /// was degraded or refused rather than decoded normally.
    Degraded {
        /// The code, in its stable `domain/action/reason` form.
        code: String,
    },
    /// The paging state the window would draw.
    Page {
        /// One-based page the window shows.
        current: u8,
        /// Number of pages the candidate list fills.
        total: u8,
    },
}

impl Expectation {
    /// Renders the expectation as the one line a divergence reports.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn describe(&self) -> String {
        match self {
            Self::Candidates {
                first,
                count,
                contains,
            } => format!("candidates first={first:?} count={count} contains={contains:?}"),
            Self::Preedit { text, caret, spans } => {
                format!("preedit text={text:?} caret={caret} spans={spans}")
            }
            Self::Degraded { code } => {
                format!("the code {code:?} surfaced on a degraded or refused step")
            }
            Self::Page { current, total } => format!("page {current}/{total}"),
        }
    }

    /// Checks one observation against what the scenario expects.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the observation matches, and a one-line reason naming what was found
    /// instead when it does not.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(crate) fn check(&self, seen: &Observation) -> Result<(), String> {
        match self {
            Self::Candidates {
                first,
                count,
                contains,
            } => seen.check_candidates(first, *count, contains),
            Self::Preedit { text, caret, spans } => seen.check_preedit(text, *caret, *spans),
            Self::Degraded { code } => seen.check_degraded(code),
            Self::Page { current, total } => seen.check_page(*current, *total),
        }
    }
}

/// What one step left the engine showing.
#[derive(Clone, Debug, PartialEq)]
pub struct Observation {
    /// Raw input the buffer holds after the step.
    pub raw: String,
    /// Caret, as a byte offset into `raw`.
    pub caret: u32,
    /// Ordered candidates, best first.
    pub candidates: Vec<Candidate>,
    /// The cut of the winning path.
    pub segments: Vec<Segment>,
    /// Whether the decode degraded.
    pub degraded: bool,
    /// The preedit the header would draw.
    pub preedit: Preedit,
    /// The page state the window would draw.
    pub page: PageState,
    /// Frozen codes the engine surfaced during the step, in the order they arose.
    pub codes: Vec<String>,
    /// Whether the keystroke itself was refused.
    pub refused: bool,
}

impl Observation {
    /// Renders what the step produced as the one line a divergence reports.
    ///
    /// The line carries candidate and preedit text. That is deliberate -- a test report
    /// that hid what the engine produced could not be acted on -- and it is safe because
    /// this output goes to the terminal of whoever ran the harness and never into the
    /// plugin's log stream.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn describe(&self) -> String {
        let head = self
            .candidates
            .first()
            .map_or("<none>", |candidate| candidate.text.as_str());
        format!(
            "candidates first={head:?} count={} degraded={} refused={} codes={:?} \
             preedit={:?} caret={} spans={} page={}/{} raw={:?}",
            self.candidates.len(),
            self.degraded,
            self.refused,
            self.codes,
            self.preedit.text,
            self.preedit.caret,
            self.preedit.spans.len(),
            self.page.current,
            self.page.total,
            self.raw,
        )
    }

    /// Reduces the candidate sequence to the values the repeat assertion compares.
    ///
    /// # Returns
    ///
    /// The candidate sequence, with the display score compared by its bits so that
    /// "identical" means the same `f32` rather than a numerically equal one.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn print(&self) -> StepPrint {
        StepPrint {
            degraded: self.degraded,
            candidates: self
                .candidates
                .iter()
                .map(|candidate| CandidatePrint {
                    index: candidate.index,
                    text: candidate.text.clone(),
                    source: candidate.source,
                    consumed_syllables: candidate.consumed_syllables,
                    score_bits: candidate.score.to_bits(),
                })
                .collect(),
        }
    }

    /// Checks the candidate list against what a scenario expects.
    ///
    /// # Errors
    ///
    /// A one-line reason naming the count, the best candidate or the missing text.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn check_candidates(
        &self,
        first: &str,
        count: usize,
        contains: &[String],
    ) -> Result<(), String> {
        if self.candidates.len() != count {
            return Err(format!(
                "the list holds {} candidates, not {count}",
                self.candidates.len()
            ));
        }
        let Some(head) = self.candidates.first() else {
            return Err(String::from(
                "the list is empty, and a decode never answers with an empty list",
            ));
        };
        if head.text != first {
            return Err(format!("the best candidate is {:?}, not {first:?}", head.text));
        }
        for text in contains {
            if !self.candidates.iter().any(|candidate| candidate.text == *text) {
                return Err(format!("{text:?} is not among the candidates"));
            }
        }
        Ok(())
    }

    /// Checks the preedit against what a scenario expects.
    ///
    /// # Errors
    ///
    /// A one-line reason naming the text, the caret or the span count.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn check_preedit(&self, text: &str, caret: u32, spans: usize) -> Result<(), String> {
        if self.preedit.text != text {
            return Err(format!(
                "the preedit reads {:?}, not {text:?}",
                self.preedit.text
            ));
        }
        if self.preedit.caret != caret {
            return Err(format!(
                "the caret sits at {}, not {caret}",
                self.preedit.caret
            ));
        }
        if self.preedit.spans.len() != spans {
            return Err(format!(
                "the preedit holds {} spans, not {spans}",
                self.preedit.spans.len()
            ));
        }
        Ok(())
    }

    /// Checks that a frozen code was surfaced by a degraded or refused step.
    ///
    /// # Errors
    ///
    /// A one-line reason naming the codes that were surfaced, or the fact that the step
    /// went through normally.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn check_degraded(&self, code: &str) -> Result<(), String> {
        if !self.codes.iter().any(|seen| seen.as_str() == code) {
            return Err(format!("the engine surfaced {:?}, not {code:?}", self.codes));
        }
        if !self.degraded && !self.refused {
            return Err(format!(
                "{code:?} was surfaced, but the step neither degraded nor was refused"
            ));
        }
        Ok(())
    }

    /// Checks the page state against what a scenario expects.
    ///
    /// # Errors
    ///
    /// A one-line reason naming the page that was found.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn check_page(&self, current: u8, total: u8) -> Result<(), String> {
        if self.page.current != current || self.page.total != total {
            return Err(format!(
                "the page is {}/{}, not {current}/{total}",
                self.page.current, self.page.total
            ));
        }
        Ok(())
    }
}

/// One candidate reduced to the values the repeat assertion compares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidatePrint {
    /// Display number, 1-based.
    pub index: u16,
    /// Text the candidate commits.
    pub text: String,
    /// Where the candidate came from.
    pub source: CandidateSource,
    /// Syllables the candidate consumes.
    pub consumed_syllables: u16,
    /// The display score, by its bits.
    pub score_bits: u32,
}

/// The candidate sequence of one step, as the repeat assertion compares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepPrint {
    /// Whether the decode degraded.
    pub degraded: bool,
    /// The candidates, best first.
    pub candidates: Vec<CandidatePrint>,
}

impl StepPrint {
    /// Renders the sequence as the one line a divergence reports.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn describe(&self) -> String {
        let texts: Vec<&str> = self
            .candidates
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect();
        format!("degraded={} candidates={texts:?}", self.degraded)
    }
}

/// Where a scenario and the engine parted company.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Divergence {
    /// Zero-based index of the step the scenario's own step list holds.
    pub step: usize,
    /// The key action the step drove.
    pub action: String,
    /// What the scenario expected, rendered.
    pub expected: String,
    /// What the engine produced, rendered.
    pub actual: String,
    /// What was wrong, in one line.
    pub reason: String,
}

impl Divergence {
    /// Builds the divergence one failing step reports.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn of(
        step: usize,
        action: KeyAction,
        expect: &Expectation,
        seen: &Observation,
        reason: String,
    ) -> Self {
        Self {
            step,
            action: action_name(action),
            expected: expect.describe(),
            actual: seen.describe(),
            reason,
        }
    }

    /// Builds the divergence a step the harness cannot drive reports.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn undriven(step: usize, action: KeyAction) -> Self {
        let name = action_name(action);
        Self {
            step,
            expected: String::from("a key action the direct-drive harness can apply"),
            actual: format!("{name} is the session state machine's, which ime-core does not export"),
            reason: String::from(
                "the harness drives typing, Backspace, the caret, Escape and a bare decode",
            ),
            action: name,
        }
    }

    /// Builds the divergence two runs of one scenario report.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn repeat(
        step: usize,
        action: Option<KeyAction>,
        first: &StepPrint,
        later: &StepPrint,
        run: usize,
    ) -> Self {
        Self {
            step,
            action: action.map_or_else(|| String::from("<none>"), action_name),
            expected: first.describe(),
            actual: later.describe(),
            reason: format!("run {run} produced a different candidate sequence"),
        }
    }

    /// Renders the divergence as the report a failing run prints.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn describe(&self) -> String {
        format!(
            "step {} ({}) diverged\n  expected: {}\n  actual:   {}\n  reason:   {}",
            self.step, self.action, self.expected, self.actual, self.reason
        )
    }
}

/// Returns the stable `domain/action/reason` code a decode error renders as.
///
/// # Panics
///
/// Never panics.
pub fn decode_code(error: &DecodeError) -> String {
    frozen_code(error)
}

/// Returns the stable `domain/action/reason` code an engine error renders as.
///
/// # Panics
///
/// Never panics.
pub fn ime_code(error: &ImeError) -> String {
    frozen_code(error)
}

/// Returns the code in front of the first separator of a rendered error.
///
/// Every variant of the frozen model renders as `domain/action/reason` followed by its
/// detail, so the code is read off the message rather than kept in a second table that
/// could drift away from `ime-types`.
///
/// # Panics
///
/// Never panics: `split` yields at least one piece for any string.
fn frozen_code(error: &impl std::fmt::Display) -> String {
    let rendered = error.to_string();
    rendered
        .split([':', ' '])
        .next()
        .unwrap_or_default()
        .to_owned()
}
