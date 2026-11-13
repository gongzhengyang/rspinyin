//! The decomposition reverse-lookup panel, driven end to end through the router.
//!
//! One fixture, the two directions of the case split: a `u`-led letter payload
//! opens a panel whose candidates carry their decomposition codes as annotations,
//! a digit payload stays with the lowercase codepoint spelling, the gates — the
//! switch off, the index absent, another domain owning the input, a miss — each
//! leave the buffer composing as ordinary pinyin, and the ways out (backspace,
//! Escape, a reload) hand control back to the session without swallowing a
//! keystroke. Everything is in memory; no test touches the clock, the environment
//! or a display server.

use ime_core::aux::AuxSource;
use ime_core::reverse::{ReverseHit, ReverseSource};
use ime_core::state::SessionState;
use ime_types::{CandidateSource, DictError, SelectTrigger, UiEvent};

use crate::engine::router::phrases::{PhraseHandle, PhraseStore};
use crate::engine::router::{KeyRouter, RoutingConfig};
use crate::engine::{CTRL, KEY_E, KEY_SPACE, SHIFT};

use super::*;

/// `FcitxKey_u`, the reverse domain's trigger.
const KEY_U: u32 = 0x0075;
/// `FcitxKey_h`, a payload letter.
const KEY_H: u32 = 0x0068;
/// `FcitxKey_f`, a payload letter.
const KEY_F: u32 = 0x0066;
/// `FcitxKey_g`, a payload letter.
const KEY_G: u32 = 0x0067;
/// `FcitxKey_j`, a payload letter.
const KEY_J: u32 = 0x006a;
/// `FcitxKey_n`, the letter that pushes a payload past the recognizer's bound.
const KEY_N: u32 = 0x006e;
/// `FcitxKey_a`, a payload letter that is also a hex digit.
const KEY_A: u32 = 0x0061;
/// `FcitxKey_z`, a payload letter the fixture's index holds nothing for.
const KEY_Z: u32 = 0x007a;
/// `FcitxKey_4`, the digit that starts the lowercase codepoint spelling.
const KEY_4: u32 = 0x0034;
/// `FcitxKey_BackSpace`.
const KEY_BACKSPACE: u32 = 0xff08;
/// `FcitxKey_Escape`.
const KEY_ESCAPE: u32 = 0xff1b;
/// `FcitxKey_grave`, the aux refinement's lead key.
const KEY_GRAVE: u32 = 0x0060;

/// Whether the newest frame on the host is a panel frame.
///
/// The fixture loads no symbol table, so a panel's `Symbol`-source candidates are
/// the only candidates of that source any frame can carry — which makes the source
/// the panel's signature, and this helper the honest answer to "is a panel up"
/// that does not depend on what the session's own decode offered beside it.
fn panel_open(host: &RecordingHost) -> bool {
    host.last_frame.as_ref().is_some_and(|frame| {
        frame
            .candidates
            .iter()
            .any(|candidate| candidate.source == CandidateSource::Symbol)
    })
}

/// The newest frame the host was sent, failing the test when none is.
fn frame(host: &RecordingHost) -> &UiFrame {
    host.last_frame
        .as_ref()
        .expect("a frame reached the window")
}

/// The texts of the newest frame's candidates, in display order.
fn shown(host: &RecordingHost) -> Vec<&str> {
    frame(host)
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect()
}

/// One entry of the fixture's decomposition table: a character and its code.
type Entries = Vec<(char, &'static str)>;

/// The reverse-index view the tests hand the router.
///
/// The lookup applies the same subsequence semantics the real index materializes,
/// at toy scale: a payload matches a character whose code holds its letters in
/// order. `present: false` is the container without the index at all, and
/// `unreadable` is the defensive arm for a container whose verification lied.
struct MockReverse {
    present: bool,
    unreadable: bool,
    entries: Entries,
}

impl MockReverse {
    /// A present index over `entries`, ascending codepoint like the real block.
    fn with_entries(entries: Entries) -> Self {
        Self {
            present: true,
            unreadable: false,
            entries,
        }
    }

    /// An index whose reads fail, the condition the contract calls impossible.
    fn unreadable() -> Self {
        Self {
            present: true,
            unreadable: true,
            entries: Vec::new(),
        }
    }
}

/// Whether every byte of `needle` appears in `haystack`, in order.
fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut code = haystack.bytes();
    needle.bytes().all(|byte| code.any(|other| other == byte))
}

impl ReverseSource for MockReverse {
    fn has_reverse_index(&self) -> bool {
        self.present
    }

    fn reverse_lookup(&self, payload: &str) -> Result<Vec<ReverseHit<'_>>, DictError> {
        if !self.present {
            return Err(DictError::LengthOutOfRange {
                field: "reverse_absent",
                value: payload.len() as u64,
            });
        }
        if self.unreadable {
            return Err(DictError::MagicMismatch);
        }
        Ok(self
            .entries
            .iter()
            .filter(|(_, code)| is_subsequence(payload, code))
            .map(|(character, code)| ReverseHit {
                character: *character,
                code,
            })
            .collect())
    }
}

/// The fixture's decomposition table, ascending codepoint like the compiled block.
///
/// The codes are chosen for the assertions that pin them: `h` matches four of the
/// five characters, `hf` two, `hfg` one, `hfgj` the same one at the full payload
/// bound, `f` three, and `a` exactly the character whose code is the hex-adjacent
/// `ab`.
fn fixture_entries() -> Entries {
    vec![
        ('啊', "ab"),
        ('户', "hdp"),
        ('房', "hf"),
        ('放', "fhg"),
        ('点', "hfgj"),
    ]
}

/// The routing configuration a reverse test runs under: the domain on.
///
/// The switch is the document's own `[engine] reverse_lookup`, projected the same
/// way the running plugin projects it.
fn reverse_config() -> RoutingConfig {
    let mut document = ime_config::Config::default();
    document.engine.reverse_lookup = true;
    let (config, _) = RoutingConfig::from_config(&document);
    config
}

/// The configuration for the tests that run the domain beside the quick panel:
/// both features on, so the case split is live in both directions.
fn reverse_and_quick_config() -> RoutingConfig {
    let mut document = reverse_config_document();
    document.quick_input.enabled = true;
    let (config, _) = RoutingConfig::from_config(&document);
    config
}

/// The document a reverse test tweaks, before projection.
fn reverse_config_document() -> ime_config::Config {
    let mut document = ime_config::Config::default();
    document.engine.reverse_lookup = true;
    document
}

/// The configuration for the test that runs the domain beside the refinement view.
fn reverse_and_aux_config() -> RoutingConfig {
    let mut document = reverse_config_document();
    document.engine.aux_refinement = true;
    let (config, _) = RoutingConfig::from_config(&document);
    config
}

/// A router with the reverse switch on, `source` answering its index, and the
/// configuration in force.
fn reverse_router<'a>(
    fixture: &'a Fixture,
    source: &'a MockReverse,
    config: RoutingConfig,
) -> KeyRouter<'a> {
    fixture
        .router_with(
            PhraseHandle::new(PhraseStore::load(None, 100, true)),
            config,
        )
        .with_reverse_source(source)
}

/// The aux-column view the lead-key test hands the router; its answers do not
/// matter, because the gate refuses before any code is read.
struct MockAuxColumn;

impl AuxSource for MockAuxColumn {
    fn has_aux_column(&self) -> bool {
        true
    }

    fn aux_code(&self, _word: &str) -> Result<Option<&str>, DictError> {
        Ok(None)
    }
}

#[test]
fn test_reverse_disabled_leaves_the_u_buffer_composing_ordinarily() {
    // The shipped configuration: `[engine] reverse_lookup` off, which is what every
    // build before the domain shipped with. A `u`-led buffer composes as ordinary
    // input, every letter of it, and no panel ever opens.
    let fixture = Fixture::new("reverse-disabled");
    let index = MockReverse::with_entries(fixture_entries());
    let mut router = reverse_router(&fixture, &index, RoutingConfig::default());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    for key in [KEY_U, KEY_H, KEY_Z] {
        router.key_event(IC, &press(key, 0), &mut host);
    }
    assert!(!panel_open(&host), "no panel opened with the switch off");
    assert!(host.diagnostics.is_empty(), "{:?}", host.diagnostics);
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("uhz")),
        "the buffer composed letter by letter as ordinary pinyin"
    );
}

#[test]
fn test_reverse_hit_opens_the_panel_with_annotated_candidates() {
    // The open the card pins: `u` composes (a bare trigger is pinyin), the second
    // letter completes a query the index answers, and the panel lists the matching
    // characters each with its full decomposition code.
    let fixture = Fixture::new("reverse-open");
    let index = MockReverse::with_entries(fixture_entries());
    let mut router = reverse_router(&fixture, &index, reverse_config());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    assert!(router.key_event(IC, &press(KEY_U, 0), &mut host), "u");
    assert!(
        !panel_open(&host),
        "a bare trigger is a pinyin syllable, not a query"
    );
    assert!(router.key_event(IC, &press(KEY_H, 0), &mut host), "h");
    assert!(panel_open(&host), "the query `h` opened the panel");
    assert_eq!(
        frame(&host).preedit.text,
        "uh",
        "the panel preedits its buffer"
    );
    assert_eq!(
        shown(&host),
        ["户", "房", "放", "点"],
        "the characters that match"
    );
    let annotations: Vec<Option<&str>> = frame(&host)
        .candidates
        .iter()
        .map(|candidate| candidate.annotation.as_deref())
        .collect();
    assert_eq!(
        annotations,
        [Some("hdp"), Some("hf"), Some("fhg"), Some("hfgj")],
        "each candidate is annotated with its full code"
    );
}

#[test]
fn test_reverse_payload_grows_and_narrows_the_list() {
    let fixture = Fixture::new("reverse-grow");
    let index = MockReverse::with_entries(fixture_entries());
    let mut router = reverse_router(&fixture, &index, reverse_config());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    router.key_event(IC, &press(KEY_U, 0), &mut host);
    assert!(router.key_event(IC, &press(KEY_F, 0), &mut host), "f");
    assert_eq!(frame(&host).preedit.text, "uf", "the payload grew");
    assert_eq!(shown(&host), ["房", "放", "点"], "every code holding `f`");
    router.key_event(IC, &press(KEY_G, 0), &mut host);
    assert_eq!(frame(&host).preedit.text, "ufg");
    assert_eq!(shown(&host), ["放", "点"], "the list narrowed with it");
    router.key_event(IC, &press(KEY_J, 0), &mut host);
    assert_eq!(frame(&host).preedit.text, "ufgj");
    assert_eq!(shown(&host), ["点"], "`fgj` names one character");
}

#[test]
fn test_reverse_miss_opens_no_panel_and_keeps_composing() {
    // A miss is an answer, not an error: no character decomposes this way, so the
    // buffer keeps composing as ordinary pinyin and nothing is reported.
    let fixture = Fixture::new("reverse-miss");
    let index = MockReverse::with_entries(fixture_entries());
    let mut router = reverse_router(&fixture, &index, reverse_config());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    router.key_event(IC, &press(KEY_U, 0), &mut host);
    router.key_event(IC, &press(KEY_Z, 0), &mut host);
    assert!(!panel_open(&host), "a query with no hits opens nothing");
    assert!(host.diagnostics.is_empty(), "{:?}", host.diagnostics);
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("uz")),
        "the buffer was not claimed by a panel that never opened"
    );
}

#[test]
fn test_reverse_absent_index_falls_through_without_a_host_error() {
    // A container without the index is the degradation the once-per-process
    // diagnostic names: the payload passes through to the ordinary decode, the
    // composition is untouched, and the report travels the process channel the
    // host's error log is not.
    let fixture = Fixture::new("reverse-absent");
    let absent = MockReverse {
        present: false,
        unreadable: false,
        entries: Vec::new(),
    };
    let mut router = reverse_router(&fixture, &absent, reverse_config());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    router.key_event(IC, &press(KEY_U, 0), &mut host);
    router.key_event(IC, &press(KEY_H, 0), &mut host);
    assert!(!panel_open(&host), "no panel opens without an index");
    assert!(
        host.diagnostics.is_empty(),
        "the absence is a process diagnostic, not a host error: {:?}",
        host.diagnostics
    );
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("uh")),
        "the composition survives the refused open"
    );
    assert!(
        !panel_open(&host),
        "no panel comes back on the next key either"
    );
    router.key_event(IC, &press(KEY_Z, 0), &mut host);
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("uhz")),
        "the ordinary gestures still act on the composition"
    );
}

#[test]
fn test_reverse_unreadable_index_aborts_the_open_and_keeps_composing() {
    // A query the index cannot answer out of a verified container is surfaced, not
    // treated as a miss: no panel opens, the composition is untouched, and the
    // next key is the pipeline's.
    let fixture = Fixture::new("reverse-unreadable");
    let index = MockReverse::unreadable();
    let mut router = reverse_router(&fixture, &index, reverse_config());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    router.key_event(IC, &press(KEY_U, 0), &mut host);
    router.key_event(IC, &press(KEY_H, 0), &mut host);
    assert!(!panel_open(&host), "an unreadable index opens no panel");
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("uh"))
    );
    assert!(
        !panel_open(&host),
        "the next query is refused the same way, not silently tolerated"
    );
    router.key_event(IC, &press(KEY_Z, 0), &mut host);
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("uhz")),
        "the composition keeps answering its ordinary keys"
    );
}

#[test]
fn test_reverse_space_commits_the_highlighted_character_and_ends_the_composition() {
    let fixture = Fixture::new("reverse-commit");
    let index = MockReverse::with_entries(fixture_entries());
    let mut router = reverse_router(&fixture, &index, reverse_config());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    router.key_event(IC, &press(KEY_U, 0), &mut host);
    router.key_event(IC, &press(KEY_H, 0), &mut host);
    assert!(router.key_event(IC, &press(KEY_SPACE, 0), &mut host));
    assert_eq!(
        host.commits,
        ["户"],
        "the highlight commits the character it sits on"
    );
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle),
        "the composition ends with the commit"
    );
    assert!(host.hides >= 1, "the window goes with it");
}

#[test]
fn test_reverse_click_commits_the_clicked_candidate() {
    let fixture = Fixture::new("reverse-click");
    let index = MockReverse::with_entries(fixture_entries());
    let mut router = reverse_router(&fixture, &index, reverse_config());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    router.key_event(IC, &press(KEY_U, 0), &mut host);
    router.key_event(IC, &press(KEY_H, 0), &mut host);
    let revision = frame(&host).revision;
    assert!(
        router.ui_event(
            IC,
            UiEvent::Select {
                revision,
                index: 1,
                trigger: SelectTrigger::Mouse,
            },
            &mut host,
        ),
        "a click on a live panel candidate is the panel's"
    );
    assert_eq!(host.commits, ["房"], "the click commits its own candidate");

    // A click carrying any other revision is stale: nothing commits and the answer
    // claims nothing, exactly as it did before the panel existed.
    let stale = router.ui_event(
        IC,
        UiEvent::Select {
            revision,
            index: 0,
            trigger: SelectTrigger::Mouse,
        },
        &mut host,
    );
    assert!(!stale);
    assert_eq!(host.commits.len(), 1, "{:?}", host.commits);
}

#[test]
fn test_reverse_backspace_steps_the_payload_back_and_closes_at_the_trigger() {
    let fixture = Fixture::new("reverse-backspace");
    let index = MockReverse::with_entries(fixture_entries());
    let mut router = reverse_router(&fixture, &index, reverse_config());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    router.key_event(IC, &press(KEY_U, 0), &mut host);
    router.key_event(IC, &press(KEY_H, 0), &mut host);
    router.key_event(IC, &press(KEY_F, 0), &mut host);
    assert_eq!(frame(&host).preedit.text, "uhf");
    assert_eq!(shown(&host), ["房", "点"], "`hf` names two codes");

    assert!(router.key_event(IC, &press(KEY_BACKSPACE, 0), &mut host));
    assert_eq!(
        frame(&host).preedit.text,
        "uh",
        "the first backspace stepped the payload back, not out"
    );
    assert_eq!(
        shown(&host),
        ["户", "房", "放", "点"],
        "the query widened again"
    );
    assert!(
        router.key_event(IC, &press(KEY_BACKSPACE, 0), &mut host),
        "the backspace at the bare trigger is the panel's to hand back"
    );
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("u")),
        "the session's own backspace took the same key"
    );
    assert!(router.key_event(IC, &press(KEY_BACKSPACE, 0), &mut host));
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle),
    );
}

#[test]
fn test_reverse_escape_cancels_the_panel_and_the_composition() {
    let fixture = Fixture::new("reverse-escape");
    let index = MockReverse::with_entries(fixture_entries());
    let mut router = reverse_router(&fixture, &index, reverse_config());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    router.key_event(IC, &press(KEY_U, 0), &mut host);
    router.key_event(IC, &press(KEY_H, 0), &mut host);
    assert!(panel_open(&host), "the panel is up");

    assert!(router.key_event(IC, &press(KEY_ESCAPE, 0), &mut host));
    assert!(host.commits.is_empty(), "{:?}", host.commits);
    assert!(host.hides >= 1);
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle)
    );
}

#[test]
fn test_reverse_overrun_payload_clamps_and_keeps_the_panel() {
    // A payload past the recognizer's four-letter bound is ordinary input again:
    // the panel keeps its last good rendering and the key stays claimed, because a
    // letter that leaked to the session would act on a list the user cannot see.
    let fixture = Fixture::new("reverse-overrun");
    let index = MockReverse::with_entries(fixture_entries());
    let mut router = reverse_router(&fixture, &index, reverse_config());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    for key in [KEY_U, KEY_H, KEY_F, KEY_G, KEY_J] {
        router.key_event(IC, &press(key, 0), &mut host);
    }
    assert_eq!(frame(&host).preedit.text, "uhfgj");
    assert_eq!(shown(&host), ["点"], "`hfgj` fills the whole bound");
    let frames_before = host.frames;
    assert!(
        router.key_event(IC, &press(KEY_N, 0), &mut host),
        "the clamped key is still the panel's"
    );
    assert_eq!(
        frame(&host).preedit.text,
        "uhfgj",
        "the payload did not grow past the bound"
    );
    assert!(
        host.frames == frames_before,
        "no repaint was owed: the rendering did not move"
    );
    assert_eq!(shown(&host), ["点"]);
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("uh")),
        "the clamped letters never reached the pinyin buffer"
    );
}

#[test]
fn test_reverse_letter_payload_claims_before_the_codepoint_spelling() {
    // The overlap the frozen contract resolves by priority: `ua` is both a reverse
    // query and the start of a codepoint spelling, and the reverse domain is
    // consulted first, so the indexed fragment wins.
    let fixture = Fixture::new("reverse-over-codepoint");
    let index = MockReverse::with_entries(fixture_entries());
    let mut router = reverse_router(&fixture, &index, reverse_and_quick_config());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    router.key_event(IC, &press(KEY_U, 0), &mut host);
    router.key_event(IC, &press(KEY_A, 0), &mut host);
    assert!(panel_open(&host), "the indexed fragment claimed the buffer");
    assert_eq!(frame(&host).preedit.text, "ua");
    assert_eq!(shown(&host), ["啊"], "`a` names the one code holding it");
    assert_eq!(
        frame(&host).candidates[0].annotation.as_deref(),
        Some("ab"),
        "the annotation is the decomposition code, not the hex spelling"
    );
}

#[test]
fn test_reverse_digit_payload_stays_with_the_codepoint_domain() {
    // The other half of the case split: a payload holding a digit never selects
    // this domain, so the lowercase codepoint spelling keeps working beside it.
    let fixture = Fixture::new("reverse-digit");
    let index = MockReverse::with_entries(fixture_entries());
    let mut router = reverse_router(&fixture, &index, reverse_and_quick_config());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    router.key_event(IC, &press(KEY_U, 0), &mut host);
    assert!(router.key_event(IC, &press(KEY_4, 0), &mut host));
    assert!(panel_open(&host), "the codepoint spelling opened a panel");
    assert_eq!(frame(&host).preedit.text, "u4");
    assert_eq!(
        frame(&host).candidates[0].annotation.as_deref(),
        Some("U0004"),
        "the annotations are hex spellings, not decomposition codes"
    );
}

#[test]
fn test_reverse_open_panel_refuses_the_aux_lead_key() {
    // The other-domain discipline, in the aux view's direction: while the reverse
    // panel owns the window, the refinement lead key passes to the application and
    // the panel stays as it is.
    let fixture = Fixture::new("reverse-aux-lead");
    let index = MockReverse::with_entries(fixture_entries());
    let column = MockAuxColumn;
    let mut router =
        reverse_router(&fixture, &index, reverse_and_aux_config()).with_aux_source(&column);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    router.key_event(IC, &press(KEY_U, 0), &mut host);
    router.key_event(IC, &press(KEY_H, 0), &mut host);
    assert!(panel_open(&host), "the panel is up");
    assert!(
        !router.key_event(IC, &press(KEY_GRAVE, 0), &mut host),
        "a domain that owns the input refuses the refinement session"
    );
    assert!(host.diagnostics.is_empty(), "{:?}", host.diagnostics);
    assert_eq!(
        frame(&host).preedit.text,
        "uh",
        "the panel's frame is still the one on screen"
    );
}

#[test]
fn test_reverse_temp_english_owns_the_input_and_no_panel_opens() {
    // The first gate of the other-domain discipline: temporary English hands every
    // key to the application, so a `u` typed there is the application's `u`, and
    // no session buffer ever forms for the panel to open over.
    let fixture = Fixture::new("reverse-temp-english");
    let index = MockReverse::with_entries(fixture_entries());
    let mut router = reverse_router(&fixture, &index, reverse_config());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    assert!(router.key_event(IC, &press(KEY_E, CTRL | SHIFT), &mut host));
    assert!(
        !router.key_event(IC, &press(KEY_U, 0), &mut host),
        "temporary English hands the key to the application"
    );
    assert!(!router.key_event(IC, &press(KEY_H, 0), &mut host));
    assert!(!panel_open(&host), "no panel opened over the mode");
    assert!(host.last_frame.is_none(), "no frame was posted at all");
}

#[test]
fn test_reverse_reload_closes_the_panel_and_disables_the_domain() {
    // A reload that switches the domain off closes the panel; the composition it
    // was answering survives it, per the reload rule, and the next letter is an
    // ordinary composing key over a buffer the domain no longer claims.
    let fixture = Fixture::new("reverse-reload");
    let index = MockReverse::with_entries(fixture_entries());
    let mut router = reverse_router(&fixture, &index, reverse_config());
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    router.key_event(IC, &press(KEY_U, 0), &mut host);
    router.key_event(IC, &press(KEY_H, 0), &mut host);
    assert!(panel_open(&host), "the panel is up");

    router.reload(RoutingConfig::default(), &mut host);
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("uh")),
        "the composition the panel answered survives the reload"
    );
    router.key_event(IC, &press(KEY_F, 0), &mut host);
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("uhf")),
        "the letter composed as ordinary input: the switch the reload carried is off, \
         so the domain claimed nothing"
    );
}
