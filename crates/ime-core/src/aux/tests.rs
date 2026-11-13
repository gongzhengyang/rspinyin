//! Tests for the aux refinement module: gate ordering, the subsequence
//! invariant, verbatim restoration, and the alphabet boundary.

use ime_types::DictError;
use ime_types::{Candidate, CandidateSource};

use super::{AuxEntry, AuxGate, AuxPassReason, AuxSession, AuxSource, AuxStep};

/// A candidate identified by its text; other fields are display plumbing.
fn candidate(text: &str) -> Candidate {
    Candidate {
        index: 1,
        text: text.to_owned(),
        annotation: None,
        source: CandidateSource::Dict,
        score: 0.0,
        consumed_syllables: 1,
    }
}

/// A snapshot of four homophone-ish characters with distinct recorded codes.
///
/// `明` ("ry") is deliberately given no code, so every filter exercise covers
/// the unrecorded-row path too.
fn snapshot() -> Vec<Candidate> {
    ["命", "明", "名", "铭"]
        .iter()
        .map(|text| candidate(text))
        .collect()
}

/// Codes pairing with [`snapshot`]; the values are fixture data, not real
/// decompositions: 命=rk, 明=None (unrecorded), 名=xk, 铭=jk.
fn codes() -> Vec<Option<&'static str>> {
    [Some("rk"), None, Some("xk"), Some("jk")]
        .into_iter()
        .collect()
}

/// Asserts `kept` is an ordered subsequence of `snapshot`, by text.
fn assert_subsequence(snapshot: &[Candidate], kept: &[Candidate]) {
    let mut cursor = 0;
    for candidate in kept {
        let position = snapshot[cursor..]
            .iter()
            .position(|c| c.text == candidate.text)
            .unwrap_or_else(|| {
                panic!(
                    "kept candidate {:?} not found in snapshot after index {cursor}",
                    candidate.text
                )
            });
        cursor += position + 1;
    }
}

#[test]
fn test_aux_gate_each_reason_passes_through_in_order() {
    let gate = AuxGate::new(true);
    let cases = [
        (
            (false, 9, true, true),
            AuxPassReason::OtherDomain,
            "another domain owns the input",
        ),
        (
            (false, 9, false, false),
            AuxPassReason::LeadKeyOccupied,
            "lead key occupied",
        ),
        (
            (false, 9, false, true),
            AuxPassReason::ColumnAbsent,
            "no aux column",
        ),
        (
            (true, 0, false, true),
            AuxPassReason::NoCandidates,
            "no candidate list",
        ),
    ];
    for ((column, count, other, free), reason, why) in cases {
        assert_eq!(
            gate.decide(column, count, other, free),
            AuxEntry::Pass(reason),
            "gate failed on: {why}"
        );
    }
    // The disabled gate sits second in the order: with the refinement off, an
    // occupied lead key and a missing column both stand, and `Disabled` still
    // names the pass -- a switched-off domain must not leak other reasons.
    let disabled = AuxGate::new(false);
    assert_eq!(
        disabled.decide(false, 0, false, false),
        AuxEntry::Pass(AuxPassReason::Disabled),
        "refinement disabled wins over every later gate"
    );
    assert_eq!(
        gate.decide(true, 9, false, true),
        AuxEntry::Enter,
        "all gates clear"
    );
}

#[test]
fn test_aux_gate_other_domain_wins_over_all_later_gates() {
    // Every later gate violated at once: the first gate in order names the
    // reason, which pins the check order the router relies on.
    let gate = AuxGate::new(false);
    assert_eq!(
        gate.decide(false, 0, true, false),
        AuxEntry::Pass(AuxPassReason::OtherDomain)
    );
}

#[test]
fn test_aux_session_kept_is_ordered_subsequence_for_any_code_sequence() {
    // Every deterministic refinement path (grown, stepped back, re-grown) must
    // leave kept an ordered subsequence of the snapshot.
    let sequences = [
        &["r", "k"][..],
        &["k", "r"][..],
        &["r", "k", "j", "b"][..],
        &["j", "x"][..],
    ];
    for sequence in sequences {
        let mut session = AuxSession::enter(snapshot(), codes());
        for letter in sequence {
            let step = session.refine(letter.chars().next().unwrap_or('r'));
            assert_eq!(step, Some(AuxStep::Refined), "letter {letter} refines");
            assert_subsequence(&snapshot(), session.kept());
        }
    }
}

#[test]
fn test_aux_session_refilter_shrinks_membership() {
    let mut session = AuxSession::enter(snapshot(), codes());
    // "r" keeps only 命 (rk); 明 has no code and drops out of every filter.
    session.refine('r').expect("ascii letter refines");
    assert_eq!(
        session
            .kept()
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>(),
        ["命"]
    );
    // "rk" still keeps 命; "rkb" keeps nothing (rk is 命's full code).
    session.refine('k').expect("ascii letter refines");
    assert_eq!(session.kept().len(), 1);
    session.refine('b').expect("ascii letter refines");
    assert!(session.kept().is_empty());
}

#[test]
fn test_aux_session_restore_to_entry_equals_snapshot_verbatim() {
    let mut session = AuxSession::enter(snapshot(), codes());
    for letter in "rk".chars() {
        session.refine(letter).expect("ascii letter refines");
    }
    assert!(session.kept().len() < snapshot().len());

    // Stepping back letter by letter grows the list again; at the entry point
    // the kept list is the snapshot, character for character.
    let mut steps = 0;
    while !session.is_at_entry() {
        assert_eq!(session.restore(), AuxStep::Restored);
        assert_subsequence(&snapshot(), session.kept());
        steps += 1;
    }
    assert_eq!(steps, 2, "two typed letters were restored");
    assert_eq!(
        session
            .kept()
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>(),
        ["命", "明", "名", "铭"]
    );
    // One backspace past the entry point closes the session instead of
    // deleting anything.
    assert_eq!(session.restore(), AuxStep::Close);
}

#[test]
fn test_aux_session_refine_rejects_keys_outside_the_aux_alphabet() {
    let mut session = AuxSession::enter(snapshot(), codes());
    for letter in ['A', '1', '`', '-', 'é'] {
        assert_eq!(
            session.refine(letter),
            None,
            "{letter} is outside a..z and must not be consumed"
        );
        assert!(session.is_at_entry(), "{letter} must not enter the code");
    }
    assert_eq!(
        session
            .kept()
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>(),
        ["命", "明", "名", "铭"]
    );
}

#[test]
fn test_aux_session_codes_shorter_than_snapshot_read_as_unrecorded() {
    // A shortage pairs by index: 明 has no row, the trailing candidates are
    // unrecorded and behave the same under a filter.
    let mut session = AuxSession::enter(snapshot(), vec![Some("rk")]);
    session.refine('r').expect("ascii letter refines");
    assert_eq!(
        session
            .kept()
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>(),
        ["命"]
    );
}

/// A stub [`AuxSource`] standing in for the dictionary handle the router holds.
struct ColumnSource {
    present: bool,
}

impl AuxSource for ColumnSource {
    fn has_aux_column(&self) -> bool {
        self.present
    }

    fn aux_code(&self, word: &str) -> Result<Option<&str>, DictError> {
        if !self.present {
            return Ok(None);
        }
        Ok(match word {
            "命" => Some("rk"),
            _ => None,
        })
    }
}

#[test]
fn test_aux_source_column_absent_reports_no_codes_and_gate_holds() {
    let absent = ColumnSource { present: false };
    assert!(!absent.has_aux_column());
    assert_eq!(absent.aux_code("命").expect("lookup"), None);

    let gate = AuxGate::new(true);
    assert_eq!(
        gate.decide(absent.has_aux_column(), 4, false, true),
        AuxEntry::Pass(AuxPassReason::ColumnAbsent)
    );

    let present = ColumnSource { present: true };
    assert_eq!(
        present.aux_code("命").expect("lookup"),
        Some("rk"),
        "the recorded code is borrowed, not copied"
    );
}
