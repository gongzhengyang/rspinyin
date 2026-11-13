//! The per-application presets: what a fresh session starts from when the configuration
//! names its application by its hashed identifier.
//!
//! Every assertion here reads what the host sees -- a key claimed or handed back, a
//! commit's text -- and, for the temporary-English initial, the session flag the
//! hand-back comes from. The matrix the card asks for is covered by the last two tests:
//! a preset applies to a session the pool has no state for, and never re-imposes
//! itself over a state a predecessor window left behind.
//!
//! The profiles are keyed by the hash the privacy layer computes, so a test needs no
//! program name in a configuration at all: it names a role word to the report (the
//! same shape the pool tests use) and keys the preset by that word's hash. No clear
//! program name reaches a preset table, here or anywhere else.

use ime_config::ShareState;
use ime_config::schema::{AppProfile, AppScriptPref, MAX_APP_PROFILES};

use super::*;
use crate::engine::router::config::AppPreset;

/// `FcitxKey_Escape`, which leaves temporary English.
const KEY_ESCAPE: u32 = 0xff1b;

/// `FcitxKey_N` -- the shifted spelling a frontend delivers for a capital letter, read
/// with the Shift bit held: the table folds it before its rows see it, and the raw
/// keysym is what the uppercase passthrough reads.
const KEY_UPPER_N: u32 = 0x004e;

/// `FcitxKey_comma`, whose commit the punctuation initial decides.
const KEY_COMMA: u32 = 0x002c;

/// What the host reports about an ordinary context of `name` -- a role word, which the
/// privacy layer hashes before anything else sees it.
fn named_report(name: &'static str) -> ContextReport<'static> {
    ContextReport::Reported {
        program: Some(name),
        password: false,
        sensitive: false,
        preedit: false,
    }
}

/// The hash the privacy layer computes for `name`, which is what a preset is keyed by.
fn preset_app(name: &'static str) -> u64 {
    crate::privacy_impl::hash_app_id(name).raw()
}

/// A configuration whose presets name `modes` for `app` alone.
fn config_with_preset(app: u64, modes: AppProfile) -> RoutingConfig {
    let mut presets = [AppPreset::EMPTY; MAX_APP_PROFILES];
    presets[0] = AppPreset { app, modes };
    RoutingConfig {
        presets,
        ..RoutingConfig::default()
    }
}

/// A router over `fixture` with the program tier in force, the setup the pool matrix
/// runs under.
fn program_router(
    fixture: &Fixture,
    phrases: PhraseHandle,
    app: u64,
    modes: AppProfile,
) -> KeyRouter<'_> {
    let config = RoutingConfig {
        share: ShareState::Program,
        ..config_with_preset(app, modes)
    };
    fixture.router_with(phrases, config)
}

#[test]
fn test_profile_preset_ascii_opens_the_session_in_temporary_english() {
    let fixture = Fixture::new("preset-ascii");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let modes = AppProfile {
        ascii: Some(true),
        ..AppProfile::EMPTY
    };
    let config = config_with_preset(preset_app("preset-terminal"), modes);
    let mut router = fixture.router_with(handle, config);
    let mut host = RecordingHost::default();

    router.activate_reported(IC, named_report("preset-terminal"));
    assert!(
        !router.key_event(IC, &press(KEY_N, 0), &mut host),
        "a session whose profile names English hands every key to the application"
    );
    assert!(
        host.last_frame.is_none(),
        "and opens no candidate window for them"
    );

    // Escape is the mode's own exit, and the key after it composes as it always did:
    // the initial is the temporary-English mode, not a second answer about keys.
    assert!(
        !router.key_event(IC, &press(KEY_ESCAPE, 0), &mut host),
        "the exit key travels to the application too"
    );
    assert!(
        router.key_event(IC, &press(KEY_N, 0), &mut host),
        "once the mode is left the session decodes as the shipped one does"
    );
    let preedit = host
        .last_frame
        .as_ref()
        .map(|frame| frame.preedit.text.as_str());
    assert_eq!(
        preedit,
        Some("n"),
        "the composition the left-open session starts is an ordinary one"
    );
}

#[test]
fn test_profile_preset_without_a_match_starts_the_shipped_way() {
    // The un-matched side of the lookup: an application the configuration names no
    // profile for starts exactly where the document projects it.
    let fixture = Fixture::new("preset-miss");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let modes = AppProfile {
        ascii: Some(true),
        ..AppProfile::EMPTY
    };
    let config = config_with_preset(preset_app("preset-editor"), modes);
    let mut router = fixture.router_with(handle, config);
    let mut host = RecordingHost::default();

    router.activate_reported(IC, named_report("preset-terminal"));
    assert_eq!(
        router.session(IC).map(|session| session.temp_english),
        Some(false),
        "no profile, no English initial"
    );
    assert!(
        router.key_event(IC, &press(KEY_N, 0), &mut host),
        "and the first letter composes, as it always did"
    );
}

#[test]
fn test_profile_preset_unreported_application_starts_the_shipped_way() {
    // The fail-closed reading: a context whose application the host did not name has
    // no hash to look up, so it can never inherit a preset -- not even by a colliding
    // key, because the unknown identifier is refused before the slots are read.
    let fixture = Fixture::new("preset-unknown");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let modes = AppProfile {
        ascii: Some(true),
        ..AppProfile::EMPTY
    };
    let config = config_with_preset(preset_app("preset-editor"), modes);
    let mut router = fixture.router_with(handle, config);
    let mut host = RecordingHost::default();

    router.activate_reported(IC, ContextReport::Unreported);
    assert!(
        router.key_event(IC, &press(KEY_N, 0), &mut host),
        "an application the host said nothing about starts where the document does"
    );
}

#[test]
fn test_profile_preset_full_width_initial_widens_the_ascii_commit() {
    let fixture = Fixture::new("preset-full-width");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let modes = AppProfile {
        full_width: Some(true),
        ..AppProfile::EMPTY
    };
    let config = config_with_preset(preset_app("preset-terminal"), modes);
    let mut router = fixture.router_with(handle, config);
    let mut host = RecordingHost::default();

    // The profiled context starts wide. The event is the shape a frontend delivers for
    // a capital letter: the shifted spelling with the Shift bit held, which the routing
    // table folds to the letter row before its rows see it, and whose raw keysym is
    // what the uppercase passthrough reads.
    router.activate_reported(IC, named_report("preset-terminal"));
    assert!(router.key_event(IC, &press(KEY_UPPER_N, SHIFT), &mut host));
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("Ｎ"),
        "the full-width initial is where the commit's widening comes from"
    );

    // ... and the unprofiled one beside it starts as the document projects it.
    router.activate_reported(IC2, named_report("preset-editor"));
    assert!(router.key_event(IC2, &press(KEY_UPPER_N, SHIFT), &mut host));
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("N"),
        "the context without a profile keeps the shipped half width"
    );
}

#[test]
fn test_profile_preset_half_width_punctuation_leaves_marks_to_the_application() {
    let fixture = Fixture::new("preset-punct");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let modes = AppProfile {
        punct_full: Some(false),
        ..AppProfile::EMPTY
    };
    let config = config_with_preset(preset_app("preset-terminal"), modes);
    let mut router = fixture.router_with(handle, config);
    let mut host = RecordingHost::default();

    // The profiled context starts on English punctuation: a mark is the application's,
    // which is the only way it reaches the screen unaltered.
    router.activate_reported(IC, named_report("preset-terminal"));
    assert!(
        !router.key_event(IC, &press(KEY_COMMA, 0), &mut host),
        "a mark is handed back while the punctuation initial is English"
    );
    assert!(
        host.commits.is_empty(),
        "and the plugin commits nothing for it"
    );

    // The unprofiled context beside it starts on Chinese punctuation, as shipped.
    router.activate_reported(IC2, named_report("preset-editor"));
    assert!(router.key_event(IC2, &press(KEY_COMMA, 0), &mut host));
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("，"),
        "the context without a profile substitutes the Chinese mark"
    );
}

#[test]
fn test_profile_preset_traditional_script_initial_converts_commits() {
    let fixture = Fixture::new("preset-script");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let modes = AppProfile {
        script: Some(AppScriptPref::Traditional),
        ..AppProfile::EMPTY
    };
    let config = config_with_preset(preset_app("preset-editor"), modes);
    let mut router = fixture.router_with(handle, config);
    router.use_script(std::sync::Arc::new(
        ime_dict::script::ScriptIndex::from_pairs(&[("你", "妳")]),
    ));
    let mut host = RecordingHost::default();

    router.activate_reported(IC, named_report("preset-editor"));
    assert!(router.key_event(IC, &press(KEY_N, 0), &mut host));
    router.key_event(IC, &press(KEY_I, 0), &mut host);
    router.key_event(IC, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("妳"),
        "the session the profile opened converts through the variant table"
    );
}

#[test]
fn test_profile_preset_traditional_script_initial_without_a_table_stays_simplified() {
    // The degradation the script toggle itself has: a preset a build cannot convert
    // through is not honoured, and the shipped simplified initial stays in force
    // rather than a bit nothing can answer.
    let fixture = Fixture::new("preset-script-missing");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let modes = AppProfile {
        script: Some(AppScriptPref::Traditional),
        ..AppProfile::EMPTY
    };
    let config = config_with_preset(preset_app("preset-editor"), modes);
    let mut router = fixture.router_with(handle, config);
    let mut host = RecordingHost::default();

    router.activate_reported(IC, named_report("preset-editor"));
    assert!(router.key_event(IC, &press(KEY_N, 0), &mut host));
    router.key_event(IC, &press(KEY_I, 0), &mut host);
    router.key_event(IC, &press(KEY_SPACE, 0), &mut host);
    assert_eq!(
        host.commits.last().map(String::as_str),
        Some("你"),
        "without a variant table the commit leaves as the dictionary wrote it"
    );
}

#[test]
fn test_profile_preset_applies_to_a_fresh_session_of_the_program_tier_and_not_to_an_adopted_one() {
    // The matrix the card asks for, in one walk. Under the program tier the preset is
    // a creation parameter: the first window of the program is fresh, so the preset
    // applies; the state the user leaves behind is what parks; and the window that
    // adopts it finds the state the user left, never a preset laid over it again.
    let fixture = Fixture::new("preset-pool");
    let handle = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let modes = AppProfile {
        ascii: Some(true),
        ..AppProfile::EMPTY
    };
    let mut router = program_router(&fixture, handle, preset_app("preset-terminal"), modes);
    let mut host = RecordingHost::default();

    // Fresh + hit: the pool holds nothing for the program, so the context is created
    // and the preset applies.
    router.activate_reported(IC, named_report("preset-terminal"));
    assert_eq!(
        router.session(IC).map(|session| session.temp_english),
        Some(true),
        "a fresh context of the program tier starts where its profile says"
    );

    // The user leaves the mode; that runtime state is what the window switch parks.
    router.key_event(IC, &press(KEY_ESCAPE, 0), &mut host);
    router.deactivate(IC, &mut host);

    // Adopted: the successor of the same program finds the parked state, and the
    // preset stays out of it.
    router.activate_reported(IC2, named_report("preset-terminal"));
    assert_eq!(
        router.session(IC2).map(|session| session.temp_english),
        Some(false),
        "an adopted session keeps the mode the user left it in"
    );
    assert!(
        router.key_event(IC2, &press(KEY_N, 0), &mut host),
        "and composes from its first letter"
    );

    // A window of another program adopts nothing, and has no profile to apply: it
    // starts the shipped way, fresh.
    router.activate_reported(IC, named_report("preset-editor"));
    assert_eq!(
        router.session(IC).map(|session| session.temp_english),
        Some(false),
        "another program's fresh context is no preset's to open"
    );
}
