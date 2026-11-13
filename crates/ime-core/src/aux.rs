//! Aux-code refinement: the backtick-led session that narrows the current
//! candidate list by shape codes.
//!
//! Responsibility: describe the dictionary-side aux-code contract
//! ([`AuxSource`]), the pure entry decision ([`AuxGate`]) a router consults
//! before it may consume the backtick key, and the refinement session
//! ([`AuxSession`]) itself.
//!
//! The session is snapshot-based: entering copies the candidate list, and every
//! refinement step is a prefix filter over that snapshot. `kept` is therefore
//! always an ordered subsequence of the snapshot — membership shrinks, order
//! never changes — and stepping the typed code back to empty restores the
//! snapshot verbatim. That invariant is what lets the router treat the session
//! as a view over an ordinary decode result instead of a second decoding pass.
//!
//! Boundary: this module performs no dictionary access and no decoding. The
//! router resolves each candidate's code once through [`AuxSource`] at entry
//! time and hands the paired lists to [`AuxSession::enter`]; everything after
//! that is pure string work over borrowed codes.

use ime_types::{Candidate, DictError};

/// The stable diagnostic code emitted once per process when the backtick key is
/// pressed but the active container carries no aux column
/// (`domain/action/reason` form; the router owns the emission, this module owns
/// the code).
pub const ABSENT_COLUMN_CODE: &str = "aux/absent";

/// The dictionary-side view of the compiled aux column.
///
/// A container either carries the column for every word record or carries none
/// at all; a missing per-word row inside a present column means that word's
/// first character was not in the compiler's decomposition table, not that the
/// column is absent.
///
/// # Concurrency
///
/// Implementations are `Send + Sync`, as the `Sync` supertrait requires. The
/// methods are bounded reads over an immutable mapped image: they never block,
/// never mutate, and never log the word text or the code.
pub trait AuxSource: Sync {
    /// Returns `true` when the backing store carries an aux column at all.
    ///
    /// This is the gate the router checks before opening a session; `false`
    /// means the backtick key must pass through unconsumed and the
    /// [`ABSENT_COLUMN_CODE`] diagnostic is owed (once per process).
    fn has_aux_column(&self) -> bool;

    /// Returns the aux code recorded for `word`, borrowed from the mapped
    /// image, or `None` when the word's first character carries no code.
    ///
    /// # Errors
    /// Returns [`DictError`] when the backing store is internally
    /// inconsistent, which a container that passed structural verification
    /// cannot be; a router surfaces the error rather than treating it as a
    /// miss.
    fn aux_code(&self, word: &str) -> Result<Option<&str>, DictError>;
}

/// The pure decision a router reaches for the backtick key before any session
/// exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuxEntry {
    /// The key opens a refinement session; the router consumes it.
    Enter,
    /// The key passes through unconsumed; the reason names the gate that held
    /// it, so the router can emit the matching diagnostic (the occupied-lead-key
    /// gate warns, the others are silent or logged at debug level).
    Pass(AuxPassReason),
}

/// Why the backtick key passed through unconsumed, in gate-checking order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuxPassReason {
    /// The input is already owned by another domain (reverse lookup, symbol
    /// mode, an English domain); an aux session must not be stacked on it.
    OtherDomain,
    /// Aux-code refinement is disabled in configuration.
    Disabled,
    /// The backtick key is bound to something else; the router warns.
    LeadKeyOccupied,
    /// The active container carries no aux column.
    ColumnAbsent,
    /// There is no candidate list to refine.
    NoCandidates,
}

/// The five gates a backtick key must clear before a session opens.
///
/// The gates are ordered and none of them swallows the key on failure: every
/// failed decision is [`AuxEntry::Pass`], and the router forwards the key to
/// whatever owns it next. This type is the transcription of that discipline as
/// a pure function, so the ordering is testable without a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuxGate {
    /// Whether configuration enables aux-code refinement.
    pub enabled: bool,
}

impl AuxGate {
    /// Builds a gate from the configuration projection.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub const fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    /// Decides whether the backtick key opens a session.
    ///
    /// `in_other_domain` reports that another domain already owns the input,
    /// `lead_key_free` that nothing else is bound to the backtick key, and
    /// `column_present`/`candidate_count` carry the [`AuxSource`] and current
    /// frame state the remaining gates read.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn decide(
        &self,
        column_present: bool,
        candidate_count: usize,
        in_other_domain: bool,
        lead_key_free: bool,
    ) -> AuxEntry {
        if in_other_domain {
            return AuxEntry::Pass(AuxPassReason::OtherDomain);
        }
        if !self.enabled {
            return AuxEntry::Pass(AuxPassReason::Disabled);
        }
        if !lead_key_free {
            return AuxEntry::Pass(AuxPassReason::LeadKeyOccupied);
        }
        if !column_present {
            return AuxEntry::Pass(AuxPassReason::ColumnAbsent);
        }
        if candidate_count == 0 {
            return AuxEntry::Pass(AuxPassReason::NoCandidates);
        }
        AuxEntry::Enter
    }
}

/// One refinement keystroke's outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuxStep {
    /// The typed code grew; the kept list shrank (or stayed the same).
    Refined,
    /// The typed code shrank; the kept list grew back a step.
    Restored,
    /// Already at the entry point: the router consumes the key to close the
    /// session and show the snapshot again.
    Close,
}

/// A snapshot-based refinement session over the current candidate list.
///
/// Entering takes ownership of the candidate list and of each candidate's aux
/// code (resolved by the router through [`AuxSource`]). Every keystroke
/// re-filters the snapshot by the full typed prefix, so the subsequence
/// invariant holds by construction: [`AuxSession::kept`] is always a slice of
/// candidates in snapshot order, only ever shorter.
///
/// # Concurrency
///
/// The session is owned by the host thread's input context, like the rest of
/// the session state; it shares nothing and borrows the codes from the
/// dictionary handle, which must outlive it.
#[derive(Clone, Debug)]
pub struct AuxSession<'a> {
    snapshot: Vec<Candidate>,
    codes: Vec<Option<&'a str>>,
    typed: String,
    kept: Vec<Candidate>,
}

impl<'a> AuxSession<'a> {
    /// Opens a session over `snapshot` with the router-resolved codes.
    ///
    /// `codes` pairs by index with `snapshot`; a trailing shortage reads as
    /// "unrecorded" and surplus codes are ignored. At entry nothing is typed,
    /// so the kept list is the snapshot verbatim.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn enter(snapshot: Vec<Candidate>, codes: Vec<Option<&'a str>>) -> Self {
        let kept = snapshot.clone();
        Self {
            snapshot,
            codes,
            typed: String::new(),
            kept,
        }
    }

    /// Feeds one refinement letter.
    ///
    /// Returns `None` without consuming the key when `letter` is outside the
    /// aux alphabet (a..z): the router forwards it instead. Any accepted letter
    /// re-filters the snapshot by the grown prefix.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn refine(&mut self, letter: char) -> Option<AuxStep> {
        if !letter.is_ascii_lowercase() {
            return None;
        }
        self.typed.push(letter);
        self.refilter();
        Some(AuxStep::Refined)
    }

    /// Steps the typed code back by one letter.
    ///
    /// At the entry point the session reports [`AuxStep::Close`] instead: the
    /// backspace is consumed to leave the session with the snapshot restored,
    /// not to delete preedit text underneath it.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn restore(&mut self) -> AuxStep {
        if self.typed.pop().is_none() {
            return AuxStep::Close;
        }
        self.refilter();
        AuxStep::Restored
    }

    /// The typed aux code so far, for preedit display.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn typed(&self) -> &str {
        &self.typed
    }

    /// The surviving candidates, in snapshot order.
    ///
    /// Candidates carry their snapshot identity untouched; the caller
    /// renumbers display positions when it rebuilds a page from this slice.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn kept(&self) -> &[Candidate] {
        &self.kept
    }

    /// Whether nothing has been typed yet (the session is at its entry state).
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn is_at_entry(&self) -> bool {
        self.typed.is_empty()
    }

    /// Re-filters the snapshot by the full typed prefix.
    ///
    /// A candidate with no recorded code survives only at the entry state;
    /// once a filter is active it cannot match and drops out — the user is
    /// naming a shape the dictionary may or may not have recorded.
    fn refilter(&mut self) {
        let typed = self.typed.as_str();
        self.kept.clear();
        for (candidate, code) in self.snapshot.iter().zip(&self.codes) {
            let matches = match code {
                Some(code) => code.starts_with(typed),
                None => typed.is_empty(),
            };
            if matches {
                self.kept.push(candidate.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests;
