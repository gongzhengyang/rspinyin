//! The scheme side of a session: the rewrite that runs before segmentation, the
//! mixed-input switch, and the syllable grid the input buffer keeps.
//!
//! Responsibility: pin that a session configured with a double-pinyin layout decodes
//! the layout's full-pinyin spelling of the keystrokes, that the buffer keeps what the
//! user pressed and its grid follows the reading that was used, that a layout change
//! is deferred until the next composition, and that the walk the session runs counts
//! the same syllables the published rewrite does.
//!
//! Boundaries: driven entirely from the in-memory doubles next door. No dictionary
//! file, no clock, no environment, no display server.

use ime_types::{DecodeFlags, KeyAction, SchemeId};

use super::{
    Fixture, Session, SessionConfig, composing, diagnosed, frame_of, index_of, kinds, preedit_text,
};
use crate::shuangpin::SchemeMap;
use crate::state::scheme::SchemeSession;
use crate::state::{SessionEvent, step};

/// A configuration with the Xiaohe layout active and the mixed-input reading on.
fn xiaohe_config() -> SessionConfig {
    SessionConfig {
        scheme: SchemeId::XIAOHE,
        ..SessionConfig::default()
    }
}

/// The texts of a session's candidates, in order.
fn candidate_texts(session: &Session) -> Vec<String> {
    session
        .decoded()
        .candidates
        .iter()
        .map(|held| held.text.clone())
        .collect()
}

#[test]
fn test_scheme_session_decodes_the_rewritten_input() {
    // `nihc` is Xiaohe for ni'hao. The buffer keeps the four keys the user pressed
    // while the decode reads the spelling they stand for.
    let cfg = xiaohe_config();
    let fixture = Fixture::new();
    let env = fixture.env();
    let session = composing(&cfg, &env, "nihc");

    assert_eq!(session.buf.raw(), "nihc", "the keys are kept as typed");
    assert_eq!(
        session.dag().normalized(),
        "nihao",
        "the graph describes the spelling the layout stands for"
    );
    assert!(
        index_of(&session, "你好").is_some(),
        "the word the keystrokes spell is offered"
    );
}

#[test]
fn test_scheme_request_carries_the_layout_and_the_switch() {
    let cfg = xiaohe_config();
    let fixture = Fixture::new();
    let env = fixture.env();
    let session = composing(&cfg, &env, "nihc");

    let request = session.request();
    assert_eq!(request.scheme, SchemeId::XIAOHE);
    assert!(request.flags.contains(DecodeFlags::SHUANGPIN));
    assert_eq!(
        request.raw, "nihao",
        "the decode reads the rewritten spelling, not the keystrokes"
    );
}

#[test]
fn test_full_pinyin_request_carries_no_scheme_switch() {
    // The boundary of the switch: a session with no layout rewrites nothing, so its
    // request is the one the decoder saw before schemes existed.
    let cfg = SessionConfig::default();
    let fixture = Fixture::new();
    let env = fixture.env();
    let session = composing(&cfg, &env, "nihao");

    let request = session.request();
    assert_eq!(request.scheme, SchemeId::FULL);
    assert!(!request.flags.contains(DecodeFlags::SHUANGPIN));
    assert_eq!(request.raw, "nihao");
}

#[test]
fn test_scheme_session_answers_the_full_pinyin_candidates() {
    // The whole point of the layer: a layout is a different encoding of the same
    // syllables, so the candidate list it produces is the one full pinyin produces for
    // the spelling it stands for, text for text.
    let fixture = Fixture::new();
    let env = fixture.env();
    let via_scheme = composing(&xiaohe_config(), &env, "nihc");
    let typed = composing(&SessionConfig::default(), &env, "nihao");

    let expected = candidate_texts(&typed);
    assert!(!expected.is_empty(), "the fixture knows the word");
    assert_eq!(candidate_texts(&via_scheme), expected);
}

#[test]
fn test_scheme_preedit_cuts_the_rewritten_text_at_the_scheme_syllables() {
    // The preedit is the rewritten spelling, cut once per scheme syllable: two
    // keystroke pairs are two syllables, so the header shows one separator between
    // them, and the caret sits at the end of what the user has typed.
    let cfg = xiaohe_config();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "nih");

    let effects = session.handle_key(KeyAction::InputChar('c'), &cfg, &env);

    assert_eq!(preedit_text(&effects), Some("ni'hao"));
    assert_eq!(
        frame_of(&effects).map(|frame| frame.preedit.caret),
        Some(6),
        "the caret sits at the end of the rewritten text"
    );
}

#[test]
fn test_scheme_backspace_removes_a_whole_scheme_syllable() {
    // One key removes the trailing scheme syllable, not one character: that is what
    // the grid the rewrite reports is for, and the graph of the input it leaves behind
    // is the spelling of the keys that remain.
    let cfg = xiaohe_config();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "nihc");

    let effects = session.handle_key(KeyAction::Backspace, &cfg, &env);

    assert_eq!(kinds(&effects), ["update-preedit", "send-frame"]);
    assert_eq!(session.buf.raw(), "ni", "both keys of `hao` went together");
    assert_eq!(session.dag().normalized(), "ni");
}

#[test]
fn test_scheme_backspace_follows_the_reading_the_switch_asked_for() {
    // With the mixed-input reading off, `hng` is the layout's own `h` + `neng`, so the
    // grid has one unit per keystroke group and the key removes the two keys of `neng`.
    let cfg = SessionConfig {
        keep_full_pinyin: false,
        ..xiaohe_config()
    };
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "hng");

    let effects = session.handle_key(KeyAction::Backspace, &cfg, &env);

    assert_eq!(kinds(&effects), ["update-preedit", "send-frame"]);
    assert_eq!(session.buf.raw(), "h");
}

#[test]
fn test_full_pinyin_fallback_inside_scheme_session() {
    // `hng` is a full-pinyin syllable Xiaohe cannot spell as one: the layout reads it
    // as `h` + `neng`. With the mixed-input reading on, the whole stream is read as the
    // syllable it is; with it off, the layout's own reading stands.
    let fixture = Fixture::new();
    let env = fixture.env();
    let kept = composing(&xiaohe_config(), &env, "hng");
    let scheme_only = composing(
        &SessionConfig {
            keep_full_pinyin: false,
            ..xiaohe_config()
        },
        &env,
        "hng",
    );

    assert_eq!(kept.dag().normalized(), "hng");
    assert_eq!(scheme_only.dag().normalized(), "hneng");
}

#[test]
fn test_literal_fallback_keeps_input_typeable() {
    // `bz` is neither a Xiaohe syllable nor a full-pinyin one, so both keystrokes are
    // carried through: the input stays what the user typed and still commits, which is
    // what "the user can always type" means for a half-typed syllable.
    let cfg = xiaohe_config();
    let fixture = Fixture::new();
    let env = fixture.env();
    let session = composing(&cfg, &env, "bz");

    assert_eq!(session.buf.raw(), "bz");
    assert_eq!(
        session.dag().normalized(),
        "bz",
        "the keystrokes are carried into the spelling"
    );
    assert!(session.decoded().degraded, "nothing to read them as");
    let first = session.decoded().candidates.first();
    assert_eq!(
        first.map(|held| held.text.as_str()),
        Some("bz"),
        "the input is what the pass-through candidate commits"
    );
}

#[test]
fn test_scheme_reading_that_is_empty_keeps_the_keystrokes() {
    // `v` is the layout's `zh` key, so a user types it on the way to every `zh`
    // syllable. On its own it is not a syllable, and the layout drops it rather than
    // carrying it -- the normalizer would fold it onto `ü` -- which would leave the
    // window with a candidate that has no text at all. The keystrokes are read as typed
    // instead, so the user can see and commit what they pressed.
    let cfg = xiaohe_config();
    let fixture = Fixture::new();
    let env = fixture.env();
    let session = composing(&cfg, &env, "v");

    assert_eq!(session.buf.raw(), "v");
    assert_eq!(session.request().scheme, SchemeId::FULL);
    assert_eq!(session.request().raw, "v", "the key is read as typed");
    let first = session.decoded().candidates.first();
    assert_eq!(
        first.map(|held| held.text.as_str()),
        Some("v"),
        "the keystroke is what the pass-through candidate commits"
    );
}

#[test]
fn test_fallback_reading_keeps_a_forced_boundary_marker() {
    // The marker pins the boundary the user asked for, and the full-pinyin reading of
    // the second syllable takes all three of its keys: the rewritten spelling has to
    // carry both.
    let cfg = xiaohe_config();
    let fixture = Fixture::new();
    let env = fixture.env();
    let session = composing(&cfg, &env, "ni'hng");

    assert_eq!(session.buf.raw(), "ni'hng");
    assert_eq!(session.dag().normalized(), "ni'hng");
}

#[test]
fn test_scheme_uppercase_keystrokes_read_like_lowercase() {
    // The buffer keeps what the user pressed, so a keystroke with Shift or Caps Lock
    // held reaches the session in upper case. The scheme alphabet is lower case, so the
    // reading and the grid have to come out the same as for lower-case keys.
    let cfg = xiaohe_config();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "VSHC");

    assert_eq!(session.dag().normalized(), "zhonghao");
    let effects = session.handle_key(KeyAction::Backspace, &cfg, &env);
    assert_eq!(kinds(&effects), ["update-preedit", "send-frame"]);
    assert_eq!(session.buf.raw(), "VS", "the keys of `hao` went together");
}

#[test]
fn test_scheme_change_is_deferred_until_the_next_composition() {
    // A reload that switches the layout off does not re-read the input in progress:
    // the composition keeps the layout it started under, and the new one answers the
    // next composition (0.4 rule 10).
    let cfg = xiaohe_config();
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = composing(&cfg, &env, "nihc");
    let full_pinyin = SessionConfig::default();

    let _ = step(
        &mut session,
        SessionEvent::ConfigReloaded(full_pinyin),
        &cfg,
        &env,
    );

    assert_eq!(session.buf.raw(), "nihc");
    assert_eq!(
        session.dag().normalized(),
        "nihao",
        "the reading did not change under the caret"
    );
    assert_eq!(session.request().scheme, SchemeId::XIAOHE);

    // Finish the composition and start the next one, which the new configuration
    // answers.
    let _ = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);
    let _ = step(&mut session, SessionEvent::CommitDone, &cfg, &env);
    let _ = session.handle_key(KeyAction::InputChar('n'), &full_pinyin, &env);

    assert_eq!(session.request().scheme, SchemeId::FULL);
    assert_eq!(session.request().raw, "n");
}

#[test]
fn test_scheme_this_build_does_not_implement_falls_back_to_full_pinyin() {
    // A layout number a newer build wrote is reported and read as full pinyin rather
    // than refusing to compose: the user keeps typing, and the diagnostic says why the
    // keys are not being mapped.
    let future = SessionConfig {
        scheme: SchemeId::from_u8(SchemeId::COUNT),
        ..SessionConfig::default()
    };
    let fixture = Fixture::new();
    let env = fixture.env();
    let mut session = Session::new();

    let effects = session.handle_key(KeyAction::InputChar('n'), &future, &env);

    assert!(
        diagnosed(&effects).is_some_and(|code| code.starts_with("decode/scheme-unsupported")),
        "the host is told which scheme number it cannot honour"
    );
    assert_eq!(session.request().scheme, SchemeId::FULL);
    assert!(!session.request().flags.contains(DecodeFlags::SHUANGPIN));
    assert_eq!(session.request().raw, "n", "the key is read as typed");
    assert_eq!(session.buf.raw(), "n");
}

#[test]
fn test_scheme_walk_counts_the_syllables_the_rewrite_counts() {
    // The session walks the keystrokes itself, and `SchemeMap` is the published rewrite
    // of the same stream. The grid the walk reports is what Backspace deletes by, so
    // its unit count has to be the syllable count the rewrite reports -- and the grid
    // has to describe the whole input, which is the shape the buffer accepts.
    let corpus = [
        "nihc", "vsgo", "bz", "ni'hc", "''nihc", "a", "oo", "vx", "qqqq", "vshc", "hng", "er", "aa",
    ];
    for scheme in [
        SchemeId::XIAOHE,
        SchemeId::ZIRANMA,
        SchemeId::MICROSOFT,
        SchemeId::SOGOU,
        SchemeId::ZIGUANG,
    ] {
        for raw in corpus {
            let mut session = SchemeSession::default();
            session.latch(scheme, false);
            session
                .rewrite(raw)
                .expect("the layouts in the corpus are implemented");
            let map = SchemeMap::build(raw, scheme)
                .expect("the layouts in the corpus are implemented")
                .expect("a scheme rewrites");

            let units = session.grid().len().saturating_sub(1);
            assert_eq!(units, usize::from(map.syllables()), "{scheme:?} {raw:?}");
            assert_eq!(session.grid().first().copied(), Some(0), "{raw:?}");
            assert_eq!(
                session.grid().last().copied(),
                Some(u16::try_from(raw.len()).unwrap_or(u16::MAX)),
                "{raw:?} must end at the end of the input"
            );
        }
    }
}
