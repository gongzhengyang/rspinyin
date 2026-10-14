//! What the routing layer does with a key while temporary English is on.
//!
//! The mode is the one place where "what a key means" and "which key leaves the mode"
//! come apart: the space bar and the `Return` key of the shipped configuration are
//! translated to the same action, and only one of them ends the mode. These cases drive
//! [`KeyRouter::key_event`] end to end — the table, the mode and the session — and assert
//! the three properties that make the mode a mode: the chord that enters it is the
//! plugin's, every key inside it reaches the application untouched, and the two that
//! leave it end the mode *and* reach the application in the same stroke.
//!
//! The doubles are as small as the path allows: no key of the mode is decoded, so the
//! dictionary is empty and the host records nothing.

use ime_core::lm::InMemoryLm;
use ime_core::privacy::DefaultPolicy;
use ime_core::state::{SessionEnv, SessionState};
use ime_core::viterbi::Decoder;
use ime_types::{ImeError, Lexicon, SyllableId, UiCommand, UserFreqSource, WordIter};

use crate::engine::host::Host;
use crate::ffi::FcitxKeyEvent;
use crate::privacy_impl::{AppBlacklist, ContextPrivacy, ContextReport};

use super::{
    CTRL, KEY_1, KEY_A, KEY_BACKSPACE, KEY_E, KEY_ESCAPE, KEY_RETURN, KEY_SPACE, KeyBindings,
    KeyRouter, RoutingConfig, SHIFT,
};

/// The input context every case uses.
const IC: u64 = 1;

/// A dictionary with no entry at all.
///
/// Not a shortcut: the mode is answered before any decode, so no key of it needs a
/// candidate, and a case that got as far as a decode would be exercising something other
/// than the mode.
struct EmptyLexicon;

impl Lexicon for EmptyLexicon {
    fn lookup(&self, _key: &str) -> Result<WordIter<'_>, ImeError> {
        Ok(WordIter::from_vec(Vec::new()))
    }

    fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Ok(WordIter::from_vec(Vec::new()))
    }

    fn fallback_single(&self, _syl: SyllableId, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Ok(WordIter::from_vec(Vec::new()))
    }
}

/// A user-frequency source that answers nothing and records nothing.
struct SilentUser;

impl UserFreqSource for SilentUser {
    fn freq(&self, _key: &str) -> u32 {
        0
    }

    fn record(&self, _key: &str, _weight_hint: u16) {}

    fn is_user_word(&self, _key: &str) -> bool {
        false
    }
}

/// A host that does nothing: no key of the mode reaches it, and a case that observed a
/// call would be a failure of the mode rather than of this double.
struct SilentHost;

impl Host for SilentHost {
    fn commit(&mut self, _ic: u64, _text: &str) {}

    fn set_preedit(&mut self, _ic: u64, _text: &str, _caret: u32) {}

    fn clear_preedit(&mut self, _ic: u64) {}

    fn post_ui(&mut self, _ic: u64, _command: UiCommand) {}

    fn toggle_enabled(&mut self, _ic: u64) -> bool {
        true
    }

    fn diagnose(&mut self, _ic: u64, _err: &ImeError) {}
}

/// The sources a router decodes against.
struct Fixture {
    /// The dictionary the decode reads.
    lexicon: EmptyLexicon,
    /// The user frequencies the commit path writes to.
    user: SilentUser,
    /// The language model the ranking is scored with.
    lm: InMemoryLm,
    /// The decoder, built with the shipped configuration.
    decoder: Decoder,
}

impl Fixture {
    /// Builds the doubles.
    fn new() -> Self {
        Self {
            lexicon: EmptyLexicon,
            user: SilentUser,
            lm: InMemoryLm::new(),
            decoder: Decoder::default(),
        }
    }

    /// The sources as the environment a session step reads.
    fn env(&self) -> SessionEnv<'_> {
        SessionEnv {
            decoder: &self.decoder,
            lexicon: &self.lexicon,
            user_freq: &self.user,
            lm: &self.lm,
        }
    }

    /// A router with [`IC`] activated as an ordinary, learnable context.
    fn router(&self, config: RoutingConfig) -> KeyRouter<'_> {
        let mut router = KeyRouter::new(
            self.env(),
            ContextPrivacy::new(Box::new(DefaultPolicy::default()), AppBlacklist::default()),
            config,
        );
        router.activate_reported(IC, ordinary_report());
        router
    }
}

/// What the host reports about an ordinary context: no program name, neither the
/// password nor the sensitive flag.
fn ordinary_report() -> ContextReport<'static> {
    ContextReport::Reported {
        program: None,
        password: false,
        sensitive: false,
    }
}

/// A key press of `sym` with `state` held.
fn press(sym: u32, state: u32) -> FcitxKeyEvent {
    FcitxKeyEvent {
        sym,
        state,
        is_release: false,
        time_ms: 0,
    }
}

/// Whether the session of [`IC`] is in temporary English mode.
fn is_temp_english(router: &KeyRouter<'_>) -> bool {
    router
        .session(IC)
        .is_some_and(|session| session.temp_english)
}

#[test]
fn test_temp_english_chord_is_kept_and_the_mode_hands_every_key_back() {
    // The two ends of the mode, in one session. The chord that enters it is the plugin's
    // — turning the mode on is the whole of what it did, so the key is not one the user
    // pressed and saw nothing happen on. Every key of the mode reaches the application,
    // and the mode survives all of them.
    let fixture = Fixture::new();
    let mut router = fixture.router(RoutingConfig::default());
    let mut host = SilentHost;

    assert!(
        router.key_event(IC, &press(KEY_E, CTRL | SHIFT), &mut host),
        "the chord enters the mode, so it is the plugin's key"
    );
    assert!(is_temp_english(&router), "and the mode is on");

    for sym in [KEY_A, KEY_SPACE, KEY_1, KEY_BACKSPACE] {
        assert!(
            !router.key_event(IC, &press(sym, 0), &mut host),
            "sym {sym:#06x} must reach the application in temporary English"
        );
        assert!(is_temp_english(&router), "and must not end the mode");
    }

    assert!(
        !router.key_event(IC, &press(KEY_RETURN, 0), &mut host),
        "the return key that leaves the mode reaches the application too"
    );
    assert!(!is_temp_english(&router), "and leaves it");

    assert!(
        router.key_event(IC, &press(KEY_A, 0), &mut host),
        "the plugin answers the next key again"
    );
}

#[test]
fn test_temp_english_return_leaves_the_mode_in_both_configurations() {
    // `Return` means "commit the raw input" in one document and "commit the highlighted
    // candidate" in the other, and the space bar shares the second of those actions.
    // Leaving the mode is a property of the key, so the answer is the same either way —
    // and the key reaches the application in both.
    let raw = RoutingConfig {
        keys: KeyBindings {
            enter_commit_raw: true,
            ..KeyBindings::default()
        },
        ..RoutingConfig::default()
    };
    let documents = [
        ("the shipped document", RoutingConfig::default()),
        ("enter_commit_raw", raw),
    ];
    for (label, config) in documents {
        let fixture = Fixture::new();
        let mut router = fixture.router(config);
        let mut host = SilentHost;
        assert!(router.key_event(IC, &press(KEY_E, CTRL | SHIFT), &mut host));

        assert!(
            !router.key_event(IC, &press(KEY_RETURN, 0), &mut host),
            "the return key reaches the application under {label}"
        );
        assert!(
            !is_temp_english(&router),
            "and leaves the mode under {label}"
        );
        assert_eq!(
            router.session(IC).map(|session| session.state),
            Some(SessionState::Idle),
            "leaving the mode commits nothing under {label}"
        );
    }
}

#[test]
fn test_temp_english_escape_ends_the_mode_and_is_handed_back() {
    // The control key of the mode. It ends the mode and reaches the application in the
    // same stroke, so neither half of the gesture is lost, and the session is left in a
    // state the next key can act on rather than behind a mode nothing can leave.
    let fixture = Fixture::new();
    let mut router = fixture.router(RoutingConfig::default());
    let mut host = SilentHost;
    assert!(router.key_event(IC, &press(KEY_E, CTRL | SHIFT), &mut host));
    // One key of the mode first, so the escape is not the stroke that entered it.
    assert!(!router.key_event(IC, &press(KEY_A, 0), &mut host));

    assert!(
        !router.key_event(IC, &press(KEY_ESCAPE, 0), &mut host),
        "the escape reaches the application"
    );
    assert!(!is_temp_english(&router), "and ends the mode");
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle),
        "the mode holds no composition to cancel"
    );
    assert_eq!(
        router.session(IC).map(|session| session.buf.raw()),
        Some(""),
        "and no input to take back"
    );
    assert!(
        router.key_event(IC, &press(KEY_A, 0), &mut host),
        "and the next key is the plugin's again"
    );
}
