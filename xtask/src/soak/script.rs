//! The keystrokes a soak sends, as one cycle the run repeats.
//!
//! Responsibility: name the actions the run cycles through, expand each into the strokes
//! the injection channel types, and hold the whole cycle against the layout before the
//! first key goes out.
//!
//! # What the cycle is for
//!
//! A soak that typed `nihao` eight thousand times would exercise one path of the state
//! machine and leave the rest of it -- candidate paging, digit selection, cancellation,
//! backspace inside a composition, the activation toggle, and the commit that is the only
//! place user-frequency writes happen -- untested for eight hours. Those are exactly the
//! paths a slow leak lives on, because each of them allocates and each of them holds state
//! across keystrokes. So the cycle visits all of them and returns to the state it started
//! in, which is what lets the next cycle begin without the run having to know where it is.
//!
//! # Why the cycle is expanded before the run
//!
//! [`compiled`] turns every action into its strokes once, up front. A character the layout
//! has no key for is therefore a refusal before the first key is sent rather than a failure
//! three hours in, which is the same discipline [`crate::testd::input`] applies to one
//! string. The run then holds a flat list and needs no state beyond an index into it.

use crate::testd::TestError;
use crate::testd::keys::{
    self, CONTROL_MASK, KS_BACKSPACE, KS_ESCAPE, KS_EQUAL, KS_MINUS, KS_SPACE, KS_TAB, KeyStroke,
};

/// One thing the run does, as a unit of the cycle.
///
/// The variants are the interactions the product's state machine distinguishes, not the
/// keys they happen to be bound to: which key commits is the shipped configuration's
/// decision, and naming the action rather than the key is what keeps this list readable
/// when that configuration changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Type a pinyin string, leaving the composition open.
    Compose(&'static str),

    /// Commit the current candidate with the commit key.
    Commit,

    /// Commit the candidate at this position, named by the digit key that selects it.
    ///
    /// The position is carried as the character the key types rather than as a number, so
    /// a position no key names cannot be written down at all: `Select('2')` is a key, and
    /// anything the layout has no key for is refused by the same check every other
    /// character goes through. A numeric position would need a range check of its own, and
    /// a position of zero would otherwise be typed as the digit zero -- which selects
    /// nothing and leaves a literal character in the client instead of a commit.
    Select(char),

    /// Page to the next group of candidates.
    PageForward,

    /// Page back to the previous group of candidates.
    PageBack,

    /// Move the highlight to the next candidate.
    NextCandidate,

    /// Abandon the composition without committing anything.
    Cancel,

    /// Delete the last syllable of the composition.
    Erase,

    /// Press the activation toggle once.
    ///
    /// It appears twice in a row in [`CYCLE`], so the cycle leaves the input method in the
    /// state it found it in whichever way the host has the trigger bound. A pair that
    /// failed to toggle is a no-op rather than a run that spends seven hours typing into a
    /// disabled input method, which is the failure mode a single toggle would have.
    Toggle,
}

/// One phase of the session a soak exists to keep exercising.
///
/// The three are the loop the long-run budget is stated for. A run that only ever composed
/// would never reach the commit path -- the one place user-frequency writes happen -- and
/// never reach the teardown path either, and those are exactly the paths a slow
/// accumulation lives on: each of them allocates, and each of them holds state across
/// keystrokes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing is being composed.
    Idle,
    /// A composition is open and a candidate is highlighted.
    Composing,
    /// A candidate has just been taken.
    ///
    /// A phase the session passes *through* rather than rests in: the two actions that
    /// take a candidate enter it and leave it in the same step.
    Committing,
}

impl Action {
    /// The phases this action passes through, given the phase it is performed in.
    ///
    /// A transition is a short path rather than a single destination because taking a
    /// candidate passes through [`Phase::Committing`]; answering with one destination
    /// would leave the walk resting in a phase the session never rests in.
    ///
    /// Only the actions that move the session between phases are modelled, and each of
    /// them is modelled only in the phase it is bound in. Paging, the highlight moving, a
    /// backspace and the activation toggle are deliberately absent: what a backspace does
    /// depends on how many syllables are left, so a model that answered for it would be
    /// asserting something about the engine that this module cannot know. An action that
    /// is not modelled is one whose transition is empty, and the walk simply does not move.
    ///
    /// # Return value
    ///
    /// The phases the action passes through, in order; empty when it leaves the session
    /// where it was.
    pub fn transition(self, from: Phase) -> &'static [Phase] {
        match (self, from) {
            (Self::Compose(_), _) => &[Phase::Composing],
            (Self::Commit | Self::Select(_), Phase::Composing) => &[Phase::Committing, Phase::Idle],
            (Self::Cancel, Phase::Composing) => &[Phase::Idle],
            _ => &[],
        }
    }
}

/// The actions one cycle visits, in order.
///
/// The composition strings are ordinary pinyin a user would type, of the lengths the
/// decode budget is stated for: two syllables most of the time, four for the case where
/// segmentation has a choice to make. Nothing here is a word list -- the engine's quality
/// is measured against the held-out set, and this list only has to make the state machine
/// move.
pub const CYCLE: &[Action] = &[
    Action::Compose("nihao"),
    Action::NextCandidate,
    Action::Commit,
    Action::Compose("zhongguo"),
    Action::PageForward,
    Action::PageBack,
    Action::Select('2'),
    Action::Compose("beijingdaxue"),
    Action::Cancel,
    Action::Compose("shanghai"),
    Action::Erase,
    Action::Erase,
    Action::Compose("chifan"),
    Action::Commit,
    Action::Toggle,
    Action::Toggle,
];

/// The composition the run leaves open when its plan is exhausted.
///
/// The run ends mid-composition on purpose. A candidate window is the only thing an
/// outside observer can see that distinguishes "the addon processed the injected keys"
/// from "the keys went to a client that did nothing with them", and the window is only
/// mapped while a composition is open. Ending with one open makes the caller's check for
/// it a fact about the run rather than a race against the next commit.
pub const OPEN_PROBE: Action = Action::Compose("ni");

/// What one pass over [`CYCLE`] does to a session.
///
/// The walk is the list of phases the pass moved through rather than a pair of endpoints,
/// because the property that matters is which *paths* it took: a pass that returned to
/// [`Phase::Idle`] only by cancelling would satisfy an endpoint check while never reaching
/// the commit path -- and the commit path is the only place user-frequency writes happen,
/// which is one of the things a soak exists to keep exercising.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Walk {
    /// The phase the pass is entered in.
    pub from: Phase,
    /// The phase after each action that moved the session, in the order it moved it.
    pub phases: Vec<Phase>,
}

impl Walk {
    /// The phase the pass leaves behind.
    pub fn to(&self) -> Phase {
        self.phases.last().copied().unwrap_or(self.from)
    }

    /// Whether the pass ends where it began.
    ///
    /// A pass that does not is a pass whose state accumulates across repetitions, which is
    /// the one thing a cycle the run repeats thousands of times may not do: the resident
    /// set it left behind would be the driver's own growth rather than the plugin's, and
    /// the report would describe the harness.
    pub fn is_closed(&self) -> bool {
        self.to() == self.from
    }

    /// The phase the pass is entered in followed by every phase it moved to.
    ///
    /// The whole sequence, so that a caller can look for a path rather than a destination:
    /// the loop a soak is stated for is the contiguous run `Idle`, `Composing`,
    /// `Committing`, `Idle`, and a sequence is what a contiguity check needs.
    pub fn sequence(&self) -> Vec<Phase> {
        std::iter::once(self.from)
            .chain(self.phases.iter().copied())
            .collect()
    }

    /// How many times the pass entered [`Phase::Committing`].
    pub fn commits(&self) -> usize {
        self.phases
            .iter()
            .filter(|phase| **phase == Phase::Committing)
            .count()
    }
}

/// The walk one pass over [`CYCLE`] makes, entered from [`Phase::Idle`].
///
/// The cycle is always entered from [`Phase::Idle`] because it is always closed: every
/// pass leaves the session where it found it, so the pass after it starts where this one
/// did. That is a property of the cycle as written rather than a rule the driver enforces,
/// which is why it is asserted in the tests below and checked again before a run starts.
pub fn walk() -> Walk {
    let mut phase = Phase::Idle;
    let mut phases = Vec::new();
    for action in CYCLE {
        phases.extend(action.transition(phase));
        // The running phase is the last one the pass moved to, and the phase it was entered
        // in before it moved anywhere.
        phase = phases.last().copied().unwrap_or(phase);
    }
    Walk {
        from: Phase::Idle,
        phases,
    }
}

/// The cycle expanded to strokes, ready to be sent in order.
///
/// # Errors
///
/// Returns [`TestError::UnsupportedChar`] for a character no key of the layout produces.
pub fn compiled() -> Result<Vec<KeyStroke>, TestError> {
    let mut strokes = Vec::new();
    for action in CYCLE {
        strokes.extend(strokes_of(*action)?);
    }
    Ok(strokes)
}

/// The strokes one action is made of.
///
/// # Errors
///
/// As [`compiled`].
pub fn strokes_of(action: Action) -> Result<Vec<KeyStroke>, TestError> {
    let plain = |keysym: u32| -> Result<Vec<KeyStroke>, TestError> {
        Ok(vec![KeyStroke { keysym, state: 0 }])
    };
    match action {
        Action::Compose(text) => keys::strokes(text),
        Action::Commit => plain(KS_SPACE),
        Action::Select(position) => {
            let stroke = keys::stroke_for_char(position)
                .ok_or(TestError::UnsupportedChar { ch: position })?;
            Ok(vec![stroke])
        }
        Action::PageForward => plain(KS_EQUAL),
        Action::PageBack => plain(KS_MINUS),
        Action::NextCandidate => plain(KS_TAB),
        Action::Cancel => plain(KS_ESCAPE),
        Action::Erase => plain(KS_BACKSPACE),
        Action::Toggle => Ok(vec![KeyStroke {
            keysym: KS_SPACE,
            state: CONTROL_MASK,
        }]),
    }
}

/// The tests of the phase model, which is a function of the action list and needs no
/// report, no session and no display to drive.
///
/// They live here rather than beside the rest of the tool's tests because they read the
/// cycle this file owns: a walk that stopped closing would be a defect of the script, and
/// the assertion belongs next to the script it is about.
#[cfg(test)]
mod tests {
    use super::*;

    /// The phases of one action performed in `from`.
    fn step(action: Action, from: Phase) -> Vec<Phase> {
        action.transition(from).to_vec()
    }

    #[test]
    fn test_action_transition_moves_the_session_only_where_it_is_bound() {
        assert_eq!(step(Action::Compose("ni"), Phase::Idle), vec![Phase::Composing]);
        // The two ways of taking a candidate pass through the committing phase on the way
        // back to idle, so the walk never rests in a phase the session does not rest in.
        for action in [Action::Commit, Action::Select('2')] {
            assert_eq!(
                step(action, Phase::Composing),
                vec![Phase::Committing, Phase::Idle],
                "{action:?}"
            );
            assert!(step(action, Phase::Idle).is_empty(), "{action:?}");
        }
        assert_eq!(step(Action::Cancel, Phase::Composing), vec![Phase::Idle]);
        // An action that is not modelled leaves the session where it was: a backspace's
        // effect depends on how many syllables are left, and a model that answered for it
        // would be asserting something about the engine that this module cannot know.
        for action in [
            Action::PageForward,
            Action::PageBack,
            Action::NextCandidate,
            Action::Erase,
            Action::Toggle,
        ] {
            assert!(step(action, Phase::Composing).is_empty(), "{action:?}");
        }
        assert!(step(Action::Cancel, Phase::Idle).is_empty());
    }

    #[test]
    fn test_walk_of_the_cycle_closes_and_visits_the_commit_path() {
        let walk = walk();
        assert_eq!(walk.from, Phase::Idle);
        assert!(
            walk.is_closed(),
            "the cycle leaves the session {:?} rather than {:?}",
            walk.to(),
            walk.from
        );
        // Both commit keys are in the cycle, and each is one pass through the committing
        // phase: a walk that reached idle only by cancelling would be exercising the
        // teardown path and never the path user-frequency writes happen on.
        assert!(walk.commits() >= 2, "{:?}", walk.phases);
        let sequence = walk.sequence();
        let loop_ = [Phase::Idle, Phase::Composing, Phase::Committing, Phase::Idle];
        assert!(
            sequence.windows(loop_.len()).any(|window| window == loop_),
            "the walk never takes the whole loop: {sequence:?}"
        );
    }

    #[test]
    fn test_walk_is_not_closed_when_the_pass_leaves_a_composition_open() {
        // The negative half: a walk that stopped inside a composition is not closed, which
        // is what makes the assertion above about the cycle rather than about the shape of
        // the type. A run repeating such a cycle would accumulate state across passes and
        // its resident set would be the driver's own growth.
        let open = Walk {
            from: Phase::Idle,
            phases: vec![Phase::Composing],
        };
        assert!(!open.is_closed());
        assert_eq!(open.to(), Phase::Composing);
        assert_eq!(open.commits(), 0);
        assert_eq!(open.sequence(), vec![Phase::Idle, Phase::Composing]);
        // A pass that moved nothing at all is trivially closed, and says nothing about the
        // commit path -- which is why the assertion above is two claims and not one.
        let still = Walk {
            from: Phase::Idle,
            phases: Vec::new(),
        };
        assert!(still.is_closed());
        assert_eq!(still.commits(), 0);
        assert_eq!(still.sequence(), vec![Phase::Idle]);
    }
}
