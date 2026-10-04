//! The engine's own domain: the three mode chords, and the bridge from the
//! sequence machine's decisions to the vocabulary the walk hands the host.

use ime_types::KeyAction;

use crate::engine::*;

use super::{KEY_K, KEY_Q, KEY_S, actions, arbitrate_sequence, ctrl, is_mode_chord, press};

// ── The engine's own domain, and the sequence bridge ─────────────────────────────

/// Exactly the three mode bits are the engine's, and nothing else is.
#[test]
fn test_is_mode_chord_names_exactly_the_three_engine_bits() {
    let named: Vec<KeyAction> = actions()
        .into_iter()
        .filter(|action| is_mode_chord(*action))
        .collect();
    assert_eq!(
        named,
        vec![
            KeyAction::ToggleLang,
            KeyAction::ToggleFullWidth,
            KeyAction::TogglePunct
        ],
        "the three mode bits and nothing else"
    );
}

/// Every answer the sequence machine gives has a place in the walk's vocabulary.
///
/// The decisions come out of the machine rather than out of a literal, so the bridge is
/// tested against what the machine actually answers.
#[test]
fn test_arbitrate_sequence_maps_every_decision() {
    let ctrl_k = ctrl(KEY_K);
    let ctrl_s = ctrl(KEY_S);
    let mut table = SequenceTable::new();
    assert!(table.bind(&[ctrl_k, ctrl_s], "save").is_ok());
    let mut machine = KeySequence::new();

    // A prefix stroke: ours from that moment, nothing executed yet.
    let opened = machine.offer(&press(ctrl_k.sym, ctrl_k.state), &table);
    assert_eq!(arbitrate_sequence(&opened), Consumed::ChainPending);
    // The stroke that ends it.
    let completed = machine.offer(&press(ctrl_s.sym, ctrl_s.state), &table);
    assert_eq!(arbitrate_sequence(&completed), Consumed::Consumed);
    // A stroke with nothing in flight and no sequence to open.
    let pass = machine.offer(&press(KEY_Q, CTRL), &table);
    assert_eq!(arbitrate_sequence(&pass), Consumed::Ignored);
    // A prefix, then a stroke that leads nowhere: the second one travels on.
    let reopened = machine.offer(&press(ctrl_k.sym, ctrl_k.state), &table);
    assert_eq!(arbitrate_sequence(&reopened), Consumed::ChainPending);
    let abandoned = machine.offer(&press(KEY_Q, CTRL), &table);
    assert_eq!(arbitrate_sequence(&abandoned), Consumed::Ignored);
    // `Escape` while a sequence is open: ours, and it cancels.
    let resend = machine.offer(&press(ctrl_k.sym, ctrl_k.state), &table);
    assert_eq!(arbitrate_sequence(&resend), Consumed::ChainPending);
    let cancelled = machine.offer(&press(KEY_ESCAPE, 0), &table);
    assert_eq!(arbitrate_sequence(&cancelled), Consumed::Consumed);
}

/// The bridge keeps a key exactly when the sequence machine says it does.
///
/// The two are written independently — one as a match over this crate's vocabulary, the
/// other as a method on the decision — so a decision added to the machine is caught here
/// rather than silently mapped to "the application's".
#[test]
fn test_arbitrate_sequence_agrees_with_keeps_key() {
    let ctrl_k = ctrl(KEY_K);
    let ctrl_s = ctrl(KEY_S);
    let mut table = SequenceTable::new();
    assert!(table.bind(&[ctrl_k, ctrl_s], "save").is_ok());
    let mut machine = KeySequence::new();
    let strokes = [
        (ctrl_k.sym, ctrl_k.state),
        (ctrl_s.sym, ctrl_s.state),
        (KEY_Q, CTRL),
        (ctrl_k.sym, ctrl_k.state),
        (KEY_ESCAPE, 0),
        (KEY_Q, CTRL),
    ];
    let mut seen = 0usize;
    for (sym, state) in strokes {
        let decision = machine.offer(&press(sym, state), &table);
        let kept = arbitrate_sequence(&decision) != Consumed::Ignored;
        assert_eq!(kept, decision.keeps_key(), "{sym:#x} {decision:?}");
        seen += 1;
    }
    assert_eq!(seen, strokes.len(), "every stroke was offered");
}
