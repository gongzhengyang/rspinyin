//! The pool: what a released context hands to its successor.
//!
//! The shipped `[engine] share_state = "context"` tier ends a deactivated context as a
//! whole, and its successor starts fresh; the opt-in `"program"` tier parks the state
//! under the application's hash and hands it to the next context of the same program.
//! Every assertion below is a commit or a session flag, because those are the two things
//! a window switch is supposed to carry across. The doubles come from the parent module.

use ime_config::ShareState;

use super::*;

/// A third input context, for the tests that hold two live contexts of one program.
const IC3: u64 = 3;

// ── the pool: what a released context hands to its successor ────────────────────
//
// The shipped `[engine] share_state = "context"` tier ends a deactivated context as a
// whole, and its successor starts fresh; the opt-in `"program"` tier parks the state
// under the application's hash and hands it to the next context of the same program.
// Every assertion below is a commit or a session flag, because those are the two things
// a window switch is supposed to carry across.

/// What the host reports about an ordinary context of a named program.
fn program_report(name: &'static str) -> ContextReport<'static> {
    ContextReport::Reported {
        program: Some(name),
        password: false,
        sensitive: false,
    }
}

/// Types `ni` in `ic`, the shortest input the fixture decodes to a candidate.
fn type_ni_in(router: &mut KeyRouter<'_>, ic: u64, host: &mut RecordingHost) {
    assert!(router.key_event(ic, &press(KEY_N, 0), host));
    router.key_event(ic, &press(KEY_I, 0), host);
}

/// A router over `fixture` with the program tier in force.
fn program_router(fixture: &Fixture, phrases: PhraseHandle) -> KeyRouter<'_> {
    let config = RoutingConfig {
        share: ShareState::Program,
        ..RoutingConfig::default()
    };
    fixture.router_with(phrases, config)
}

#[test]
fn test_program_share_carries_the_script_bit_across_two_input_contexts() {
    let fixture = Fixture::new("share-script");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = program_router(&fixture, handle);
    router.use_script(std::sync::Arc::new(ime_dict::script::ScriptIndex::from_pairs(
        &[("你", "妳")],
    )));
    let mut host = RecordingHost::default();

    // The first window: the user flips to traditional and commits once.
    router.activate_reported(IC, program_report("editor"));
    type_ni_in(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(host.commits.last().map(String::as_str), Some("你"));
    assert!(router.key_event(IC, &press(KEY_T, CTRL | SHIFT), &mut host));

    // The window switch: the context ends, and its state parks under the program.
    router.deactivate(IC, &mut host);

    // The second window of the same program adopts what the first left behind.
    router.activate_reported(IC2, program_report("editor"));
    type_ni_in(&mut router, IC2, &mut host);
    router.key_event(IC2, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("妳"),
        "the script bit follows the program, not the window"
    );
}

#[test]
fn test_program_share_carries_temporary_english_across_a_window_switch() {
    let fixture = Fixture::new("share-temp-english");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = program_router(&fixture, handle);
    let mut host = RecordingHost::default();

    router.activate_reported(IC, program_report("editor"));
    // With nothing composing the step produces no effect -- the documented shape of the
    // one action whose whole work is a session flag -- so the assertion is the flag
    // itself, which is the thing a window switch has to carry.
    router.step_session(IC, SessionEvent::Key(KeyAction::EnterTempEnglish), &mut host);
    assert_eq!(
        router.session(IC).map(|session| session.temp_english),
        Some(true),
        "the flag the mode sets lives on the session"
    );
    router.deactivate(IC, &mut host);

    router.activate_reported(IC2, program_report("editor"));
    assert_eq!(
        router.session(IC2).map(|session| session.temp_english),
        Some(true),
        "the temporary-English switch follows the program"
    );
}

#[test]
fn test_program_share_keeps_two_programs_apart() {
    let fixture = Fixture::new("share-isolation");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = program_router(&fixture, handle);
    router.use_script(std::sync::Arc::new(ime_dict::script::ScriptIndex::from_pairs(
        &[("你", "妳")],
    )));
    let mut host = RecordingHost::default();

    router.activate_reported(IC, program_report("editor"));
    assert!(router.key_event(IC, &press(KEY_T, CTRL | SHIFT), &mut host));
    router.deactivate(IC, &mut host);

    // A different hash is a different slot: the parked state of the editor is not the
    // browser's to adopt, which is the isolation the tier promises.
    router.activate_reported(IC2, program_report("browser"));
    type_ni_in(&mut router, IC2, &mut host);
    router.key_event(IC2, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("你"),
        "another program's state stays another program's"
    );
}

#[test]
fn test_program_share_never_pools_an_unknown_application() {
    // The unknown identifier is the collision fallback's honest form: a name that
    // cannot be compared cannot be pooled by, so every context of an unreported or
    // unnamed application keeps state of its own, under either tier.
    let fixture = Fixture::new("share-unknown");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = program_router(&fixture, handle);
    router.use_script(std::sync::Arc::new(ime_dict::script::ScriptIndex::from_pairs(
        &[("你", "妳")],
    )));
    let mut host = RecordingHost::default();

    router.activate_reported(IC, ContextReport::Unreported);
    assert!(router.key_event(IC, &press(KEY_T, CTRL | SHIFT), &mut host));
    router.deactivate(IC, &mut host);

    // A second unknown context: fresh state, not the first one's.
    router.activate_reported(IC2, ContextReport::Unreported);
    type_ni_in(&mut router, IC2, &mut host);
    router.key_event(IC2, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("你"),
        "an unknown application pools nothing"
    );

    // And the same id returning is a fresh start too: nothing was parked for it.
    router.activate_reported(IC, ContextReport::Unreported);
    type_ni_in(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("你"),
        "and its own return does not resurrect what it released"
    );
}

#[test]
fn test_program_share_parks_only_the_last_live_state_of_a_program() {
    let fixture = Fixture::new("share-last-live");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = program_router(&fixture, handle);
    router.use_script(std::sync::Arc::new(ime_dict::script::ScriptIndex::from_pairs(
        &[("你", "妳")],
    )));
    let mut host = RecordingHost::default();

    // Two windows of one program live at once. The first flips to traditional; the
    // second keeps the shipped bits, and is the window the program ends on.
    router.activate_reported(IC, program_report("editor"));
    router.activate_reported(IC2, program_report("editor"));
    assert!(router.key_event(IC, &press(KEY_T, CTRL | SHIFT), &mut host));

    // The first window closes while the second is still live: its divergent state is
    // nobody's to adopt, so it is dropped rather than parked.
    router.deactivate(IC, &mut host);
    router.deactivate(IC2, &mut host);

    // The third window adopts what the last live window held.
    router.activate_reported(IC3, program_report("editor"));
    type_ni_in(&mut router, IC3, &mut host);
    router.key_event(IC3, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("你"),
        "the state that survived is the last live one's, not a stale window's"
    );
}

#[test]
fn test_policy_switch_finishes_live_contexts_and_closes_the_parked_ones() {
    // The snapshot semantics a tier switch keeps: the context in flight finishes under
    // the tier it started with, the states parked under the old tier never resurface,
    // and every activation after the switch pools under the new one.
    let fixture = Fixture::new("share-switch");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = program_router(&fixture, handle);
    router.use_script(std::sync::Arc::new(ime_dict::script::ScriptIndex::from_pairs(
        &[("你", "妳")],
    )));
    let mut host = RecordingHost::default();

    // One window parks a traditional state; the next adopts it, and the adoption is
    // proven by what commits.
    router.activate_reported(IC, program_report("editor"));
    assert!(router.key_event(IC, &press(KEY_T, CTRL | SHIFT), &mut host));
    router.deactivate(IC, &mut host);
    router.activate_reported(IC, program_report("editor"));
    type_ni_in(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("妳"),
        "the adoption happened: the parked state answers the returned window"
    );

    // The switch: the program tier is replaced by the shipped one.
    router.reload(RoutingConfig::default(), &mut host);

    // The live context finishes under the tier it started with.
    type_ni_in(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("妳"),
        "a context in flight keeps the bits its own creation gave it"
    );

    // Its state is closed, not parked: the tier it would park under is gone.
    router.deactivate(IC, &mut host);
    router.activate_reported(IC2, program_report("editor"));
    type_ni_in(&mut router, IC2, &mut host);
    router.key_event(IC2, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("你"),
        "the next window starts fresh under the new tier"
    );
}

#[test]
fn test_context_tier_ends_a_context_whole() {
    // The shipped tier's contract is today's behaviour: a deactivated context ends as a
    // whole, and its next activation starts from the configuration, not from what the
    // user had switched on before it closed.
    let fixture = Fixture::new("tier-context-whole");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = fixture.router(handle);
    router.use_script(std::sync::Arc::new(ime_dict::script::ScriptIndex::from_pairs(
        &[("你", "妳")],
    )));
    let mut host = RecordingHost::default();

    router.activate_reported(IC, ordinary_report());
    assert!(router.key_event(IC, &press(KEY_T, CTRL | SHIFT), &mut host));
    router.deactivate(IC, &mut host);

    router.activate_reported(IC, ordinary_report());
    type_ni_in(&mut router, IC, &mut host);
    router.key_event(IC, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("你"),
        "the shipped tier parks nothing: the next activation is a fresh context"
    );
}
