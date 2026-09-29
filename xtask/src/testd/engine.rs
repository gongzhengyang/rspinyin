//! `xtask testd engine` -- the in-process engine direct-drive channel.
//!
//! # What this is
//!
//! The deterministic, headless half of the test platform. It links `ime-core` into the
//! harness and calls it, so a whole decode pipeline runs with no display server, no
//! fcitx5, no dictionary file and no timing noise, and its contracts can be asserted
//! exactly.
//!
//! # How it stays separate from the XTEST path
//!
//! The other channel (`testd x11`) injects real key and pointer events into a running
//! fcitx5 session and reads the candidate window back through X11. That channel is the
//! only one that can say anything about the host, the compositor, the window geometry or
//! the end-to-end latency. This one never starts a process, never opens a display and
//! never measures a duration: it holds the engine in its own address space and calls it.
//! A result from here is a statement about the engine's contract; a claim about the host
//! belongs to the other channel, and the two must never be reported as each other.
//!
//! # Determinism
//!
//! The same scenario, the same doubles and the same configuration must produce a
//! byte-identical candidate sequence, on every run and in every process.
//! [`assert_repeatable`] is what enforces it: it replays a scenario and compares each
//! step's candidate sequence, display score included, against the first replay. That is
//! the only way to catch a candidate order that starts drifting with floating-point
//! rounding, which the fixed-point scorer exists to prevent.
//!
//! # No filesystem, no clock
//!
//! The drive path -- this module and [`doubles`], [`scenario`] and [`builtin`] -- opens
//! no file, reads no clock and consults no environment variable. The one module that
//! touches the filesystem is [`cli`], which reads scenario files because reading them is
//! a tool's job, and it is deliberately kept outside the drive path. The input buffer's
//! session stamp is never written either: `InputBuffer::mark_session_start` is the only
//! clock the buffer has, and the harness never calls it.
//!
//! # What the harness drives
//!
//! A step drives one frozen `KeyAction`, and only the actions whose whole effect the
//! engine owns: typing, Backspace, the caret, Escape and a bare decode. Committing,
//! selecting, paging and the mode toggles belong to the session state machine, which
//! `ime-core` does not export from its crate root yet; a scenario that names one is
//! reported as a divergence rather than silently ignored.

mod builtin;
mod cli;
mod doubles;
mod scenario;

#[cfg(test)]
mod tests;

pub use crate::testd::engine::builtin::scenarios;
pub use crate::testd::engine::cli::{EngineArgs, run};

// Plain `use`, not `pub use`: these two are what this file's own functions name, and
// re-exporting the rest of `scenario` was a surface nothing consumed.
use crate::testd::engine::scenario::{Divergence, Scenario};

use ime_core::input::InputBuffer;
use ime_core::preedit::build_preedit;
use ime_core::segment::{HINT_INLINE_BOUNDARIES, SyllableDag};
use ime_core::viterbi::Decoder;
use ime_types::{
    CandidateSource, DecodeRequest, DecodeResult, KeyAction, LanguageModel, Lexicon, PageState,
    UserFreqSource,
};
use smallvec::SmallVec;

use crate::testd::engine::doubles::Sources;
use crate::testd::engine::scenario::{Observation, StepPrint, decode_code, drives, ime_code};

/// How many times a scenario is replayed when a caller asks for the repeat assertion.
///
/// One hundred replays of a handful of keystrokes costs milliseconds, and it is the
/// shortest run that would have caught every ordering drift seen so far.
pub const REPEAT_RUNS: usize = 100;

/// Candidates one page of the window holds.
///
/// The shipped candidate limit is five pages of nine, which is where the page projection
/// below takes its width from.
const PAGE_SIZE: usize = 9;

/// One scripted session, driven straight against the engine.
///
/// It holds what the session state machine holds between keystrokes -- the input buffer
/// and the segmentation graph -- and rebuilds everything else per step, because a step is
/// the unit a scenario asserts on.
struct Session<'a> {
    /// The decoder under test, built from the shipped configuration.
    decoder: Decoder,
    /// The data sources injected into it.
    sources: Sources<'a>,
    /// Frozen codes the scenario's dictionary can refuse a lookup with.
    ///
    /// The decoder folds a refused lookup into `DecodeResult::degraded` and never passes
    /// the error on, so the code is read from the spec the dictionary was built from
    /// rather than from the dictionary itself. A spec may declare one code and no more,
    /// which is what makes that reading unambiguous.
    failure_codes: Vec<String>,
    /// The input buffer, kept across steps the way a session keeps it.
    buffer: InputBuffer,
    /// The segmentation graph of the current input, kept so a step allocates nothing.
    dag: SyllableDag,
}

impl<'a> Session<'a> {
    /// Builds a session over `sources`.
    ///
    /// The decoder is [`Decoder::default`], which is the shipped configuration paired
    /// with the scorer built from the same weights; building it cannot fail, and a
    /// decoder that refused the shipped configuration would be a defect rather than a
    /// scenario outcome.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn new(sources: Sources<'a>, failure_codes: Vec<String>) -> Self {
        Self {
            decoder: Decoder::default(),
            sources,
            failure_codes,
            buffer: InputBuffer::new(),
            dag: SyllableDag::new(),
        }
    }

    /// Drives one key action and answers what the engine showed afterwards.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn step(&mut self, action: KeyAction) -> Observation {
        let refusal = self.apply(action);
        self.observe(refusal)
    }

    /// Applies one key action to the input buffer.
    ///
    /// # Returns
    ///
    /// The frozen code the keystroke was refused with, or `None` when it was accepted.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn apply(&mut self, action: KeyAction) -> Option<String> {
        match action {
            KeyAction::InputChar(ch) => self
                .buffer
                .push_char(ch)
                .err()
                .map(|error| ime_code(&error)),
            KeyAction::Backspace => {
                let _ = self.buffer.backspace();
                None
            }
            KeyAction::MoveCaret(delta) => {
                let _ = self.buffer.move_caret(delta);
                None
            }
            KeyAction::Escape => {
                self.buffer.clear();
                None
            }
            KeyAction::Ignore => None,
            // Unreachable: a caller rejects any action `drives` does not cover before the
            // first step runs, so no other variant can reach here.
            _ => None,
        }
    }

    /// Decodes the buffer and collects everything the window would read.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn observe(&mut self, refusal: Option<String>) -> Observation {
        let refused = refusal.is_some();
        let mut codes = Vec::new();
        if let Some(code) = refusal {
            push_code(&mut codes, &code);
        }
        // The decoder builds its own graph and folds a build failure into a degraded
        // result, so the frozen code of a refused input is read from the layer that owns
        // it: the same segmentation entry point the decoder calls.
        if let Err(error) = self.dag.build(self.buffer.raw()) {
            push_code(&mut codes, &decode_code(&error));
        }
        if let Some(error) = self.dag.dropped_chars().first_error() {
            push_code(&mut codes, &decode_code(&error));
        }
        if self.dag.has_path() {
            self.write_back_grid();
        }
        let request = DecodeRequest::new(self.buffer.raw());
        let result = self.decoder.decode(
            &request,
            self.sources.lexicon,
            self.sources.user_freq,
            self.sources.lm,
        );
        if refused_a_lookup(&result) {
            for code in &self.failure_codes {
                push_code(&mut codes, code);
            }
        }
        let preedit = build_preedit(&self.buffer, &self.dag);
        let page = project_page(result.candidates.len());
        Observation {
            raw: self.buffer.raw().to_owned(),
            caret: self.buffer.caret(),
            candidates: result.candidates,
            segments: result.segments,
            degraded: result.degraded,
            preedit,
            page,
            codes,
            refused,
        }
    }

    /// Writes the segmentation's syllable grid into the input buffer.
    ///
    /// The hint's offsets are offsets into the normalized spelling, so it describes the
    /// buffer only when normalization was byte for byte: nothing dropped, and the same
    /// length. The session state machine maps the hint back through a helper that lives
    /// in the session module, which `ime-core` does not export from its crate root, so
    /// the harness writes the grid back in the case where the two coincide and leaves the
    /// buffer's own coarser grid alone otherwise. The check comes first because a grid
    /// that did not describe the input would trip `InputBuffer::set_boundaries`'s debug
    /// assertion rather than be ignored.
    ///
    /// # Panics
    ///
    /// Never panics.
    fn write_back_grid(&mut self) {
        let same_length = self.dag.normalized().len() == self.buffer.raw().len();
        if !self.dag.dropped_chars().is_empty() || !same_length {
            return;
        }
        let mut hint = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
        if !self.dag.best_segmentation_hint(&mut hint) {
            return;
        }
        self.buffer.set_boundaries(&hint);
    }
}

/// Runs one scenario against the three injected sources.
///
/// # Returns
///
/// `Ok(())` when every step observed what the scenario expected.
///
/// # Errors
///
/// The [`Divergence`] of the first step that diverged, naming the step, what was expected
/// and what the engine produced.
///
/// # Panics
///
/// Never panics.
pub fn run_scenario(
    scenario: &Scenario,
    lexicon: &dyn Lexicon,
    user_freq: &dyn UserFreqSource,
    lm: &dyn LanguageModel,
) -> Result<(), Divergence> {
    let sources = Sources {
        lexicon,
        user_freq,
        lm,
    };
    trace(scenario, &sources).map(|_| ())
}

/// Replays `scenario` `runs` times and compares the candidate sequences.
///
/// # Parameters
///
/// - `scenario`: the scenario to replay.
/// - `sources`: the sources to inject. The doubles hold no state a replay changes, which
///   is what makes the comparison meaningful.
/// - `runs`: total number of replays. The first is the reference, so `1` checks the
///   scenario without comparing anything.
///
/// # Returns
///
/// `Ok(())` when every replay produced the same candidate sequence at every step.
///
/// # Errors
///
/// The [`Divergence`] of the first step whose sequence differed, naming the step, the
/// sequence of the reference replay and the one the later replay produced.
///
/// # Panics
///
/// Never panics.
pub fn assert_repeatable(
    scenario: &Scenario,
    sources: &Sources<'_>,
    runs: usize,
) -> Result<(), Divergence> {
    let runs = runs.max(1);
    let reference = trace(scenario, sources)?;
    for run in 1..runs {
        let seen = trace(scenario, sources)?;
        // Both traces come from the same step list, so they have the same length and a
        // `zip` cannot hide a step.
        for (index, (first, later)) in reference.iter().zip(seen.iter()).enumerate() {
            if first != later {
                let action = scenario.steps.get(index).map(|step| step.action);
                return Err(Divergence::repeat(index, action, first, later, run));
            }
        }
    }
    Ok(())
}

/// Replays `scenario` once and answers the candidate sequence of every step.
///
/// # Errors
///
/// The [`Divergence`] of the first step that diverged from what the scenario expected.
///
/// # Panics
///
/// Never panics.
fn trace(scenario: &Scenario, sources: &Sources<'_>) -> Result<Vec<StepPrint>, Divergence> {
    let mut session = Session::new(*sources, scenario.dictionary.failure_codes());
    let mut prints = Vec::with_capacity(scenario.steps.len());
    for (index, step) in scenario.steps.iter().enumerate() {
        if !drives(step.action) {
            return Err(Divergence::undriven(index, step.action));
        }
        let seen = session.step(step.action);
        step.expect
            .check(&seen)
            .map_err(|reason| Divergence::of(index, step.action, &step.expect, &seen, reason))?;
        prints.push(seen.print());
    }
    Ok(prints)
}

/// Returns `true` when a decode degraded because the dictionary refused a lookup.
///
/// The decoder folds two different failures into `DecodeResult::degraded`. A request it
/// could not read at all is answered with the single pass-through candidate, whose source
/// says so; a lattice a refused lookup left incomplete is answered from the paths that
/// were found, whose sources are the dictionary's. Only the second means a lookup was
/// refused, and reading the two apart is what lets the harness name the frozen code the
/// scenario's dictionary declares.
///
/// # Panics
///
/// Never panics.
fn refused_a_lookup(result: &DecodeResult) -> bool {
    let pass_through = result
        .candidates
        .first()
        .is_some_and(|candidate| candidate.source == CandidateSource::Passthrough);
    result.degraded && !pass_through
}

/// Appends `code` to the diagnostics unless it is already listed.
///
/// # Panics
///
/// Never panics.
fn push_code(codes: &mut Vec<String>, code: &str) {
    if !codes.iter().any(|seen| seen == code) {
        codes.push(code.to_owned());
    }
}

/// Returns the page state the window would draw for a candidate list of `count`.
///
/// A projection, not a second paging implementation: `ime_core::state::paging` owns the
/// real paging state machine and is not exported from the crate root yet, so the harness
/// counts the pages a list fills against the frozen "five pages of nine" budget the
/// decoder's own candidate limit comes from. A scenario that only decodes therefore
/// always sees page one; the expectation exists so that a page assertion has somewhere to
/// land once the state machine is reachable, and so that the candidate list's size is
/// checked against the page budget in the meantime.
///
/// # Panics
///
/// Never panics.
fn project_page(count: usize) -> PageState {
    let total = count.div_ceil(PAGE_SIZE).max(1);
    PageState {
        current: 1,
        total: u8::try_from(total).unwrap_or(u8::MAX),
        page_size: u8::try_from(PAGE_SIZE).unwrap_or(u8::MAX),
    }
}
