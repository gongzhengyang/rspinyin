//! The character-selection keys, driven end to end through the router.
//!
//! The route lives in `effects::select_char`; what these cases pin is what a key
//! does from the host's point of view: which character goes out, what stays in
//! the composing input, which keys keep the answer the punctuation row has
//! always given, and which travel back to the application. Everything is in
//! memory; no test here touches a dictionary file, the clock, the environment or
//! a display server.

use super::*;

/// `FcitxKey_bracketleft`, the first-character key.
const KEY_BRACKET_LEFT: u32 = 0x005b;
/// `FcitxKey_bracketright`, the last-character key.
const KEY_BRACKET_RIGHT: u32 = 0x005d;
/// `FcitxKey_i`, the second letter of the fixture's reading.
const KEY_I: u32 = 0x0069;
/// `FcitxKey_h`, the first letter of the second syllable.
const KEY_H: u32 = 0x0068;
/// `FcitxKey_a`.
const KEY_A: u32 = 0x0061;
/// `FcitxKey_o`.
const KEY_O: u32 = 0x006f;
/// `FcitxKey_v`, a letter no reading of the fixture cuts.
const KEY_V: u32 = 0x0076;
/// `FcitxKey_period`, a mark the route does not touch.
const KEY_PERIOD: u32 = 0x002e;

/// A router over the fixture with an empty phrase document, the shape every
/// other case in this suite runs with.
fn bracket_router(fixture: &Fixture) -> KeyRouter<'_> {
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    fixture.router(handle)
}

/// Types `nihao`, the shortest input the fixture decodes to a two-character
/// candidate, and answers what came out on top.
fn type_nihao(router: &mut KeyRouter<'_>, host: &mut RecordingHost) {
    for sym in [KEY_N, KEY_I, KEY_H, KEY_A, KEY_O] {
        router.key_event(IC, &press(sym, 0), host);
    }
}

#[test]
fn test_key_event_bracket_left_commits_the_first_character_and_keeps_the_rest() {
    let fixture = Fixture::new("select-first");
    let mut router = bracket_router(&fixture);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_nihao(&mut router, &mut host);
    assert_eq!(
        host.highlighted.as_deref(),
        Some("你好"),
        "the setup decodes"
    );

    assert!(
        router.key_event(IC, &press(KEY_BRACKET_LEFT, 0), &mut host),
        "a selection is the plugin's"
    );
    assert_eq!(host.commits, ["你"], "the first character went out");
    let session = router.session(IC).expect("the context is live");
    assert_eq!(session.buf.raw(), "hao", "the rest keeps composing");
    assert_eq!(session.state, SessionState::Composing);
}

#[test]
fn test_key_event_bracket_right_commits_the_last_character_and_keeps_the_rest() {
    let fixture = Fixture::new("select-last");
    let mut router = bracket_router(&fixture);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_nihao(&mut router, &mut host);
    assert_eq!(
        host.highlighted.as_deref(),
        Some("你好"),
        "the setup decodes"
    );

    assert!(
        router.key_event(IC, &press(KEY_BRACKET_RIGHT, 0), &mut host),
        "a selection is the plugin's"
    );
    assert_eq!(host.commits, ["好"], "the last character went out");
    let session = router.session(IC).expect("the context is live");
    assert_eq!(session.buf.raw(), "ni", "the rest keeps composing");
    assert_eq!(session.state, SessionState::Composing);
}

#[test]
fn test_key_event_idle_bracket_left_still_commits_the_chinese_mark() {
    // The default the route must not touch: with nothing composing the bracket
    // is punctuation, answered by the policy exactly as it always was.
    let fixture = Fixture::new("idle-mark");
    let mut router = bracket_router(&fixture);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    assert!(
        router.key_event(IC, &press(KEY_BRACKET_LEFT, 0), &mut host),
        "the punctuation answer is the plugin's"
    );
    assert_eq!(
        host.commits,
        ["【"],
        "the policy answered, as it always has"
    );
}

#[test]
fn test_key_event_composing_bracket_on_a_passthrough_candidate_is_handed_back() {
    // An input no reading cuts degrades to a pass-through candidate, which is
    // not a reading a character can be taken from: the key keeps the old answer
    // and travels back to the application, exactly as a composing mark does.
    let fixture = Fixture::new("passthrough");
    let mut router = bracket_router(&fixture);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    for _ in 0..3 {
        router.key_event(IC, &press(KEY_V, 0), &mut host);
    }
    assert!(
        !router.key_event(IC, &press(KEY_BRACKET_LEFT, 0), &mut host),
        "a pass-through candidate is not a selection"
    );
    assert!(host.commits.is_empty(), "nothing went out");
}

#[test]
fn test_key_event_composing_mark_the_route_does_not_name_is_handed_back() {
    // A mark that is not one of the two brackets keeps the old answer without
    // reaching the route at all.
    let fixture = Fixture::new("other-mark");
    let mut router = bracket_router(&fixture);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    router.key_event(IC, &press(KEY_N, 0), &mut host);
    router.key_event(IC, &press(KEY_I, 0), &mut host);
    assert!(
        !router.key_event(IC, &press(KEY_PERIOD, 0), &mut host),
        "a composing period still belongs to the application"
    );
    assert!(host.commits.is_empty());
}
