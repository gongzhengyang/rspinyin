//! The auxiliary-code refinement view, driven end to end through the router.
//!
//! Five gates, one fixture: every refused gate hands the lead key back with the
//! composition and the configuration untouched, an open view narrows the frame
//! the session decoded without ever reordering it, the ways out — backspace to
//! the entry point, Escape — restore the snapshot verbatim, and a selection
//! commits the kept candidate through the ordinary door. Everything is in
//! memory; the clock is installed once with the same snapshot the quick-input
//! tests read, so installation order cannot change an assertion.

use std::collections::BTreeMap;

use ime_config::keymap::KeyBindings;
use ime_config::schema::SelectCharKey;
use ime_core::aux::{AuxPassReason, AuxSource};
use ime_types::{DictError, SelectTrigger, UiEvent};

use crate::engine::host::{ClockSnapshot, ClockSource};
use crate::engine::router::effects::aux;
use crate::engine::router::effects::quick::install_clock;
use crate::engine::router::phrases::{PhraseHandle, PhraseStore};
use crate::engine::router::{KeyRouter, RoutingConfig};
use crate::engine::{CTRL, KEY_E, SHIFT};

use super::*;

/// `FcitxKey_grave`, the lead key of the refinement session.
const KEY_GRAVE: u32 = 0x0060;
/// `FcitxKey_a`, the first refinement letter.
const KEY_A: u32 = 0x0061;
/// `FcitxKey_c`, a refinement letter that names a different candidate.
const KEY_C: u32 = 0x0063;
/// `FcitxKey_2`, the digit that names the second candidate on the page.
const KEY_2: u32 = 0x0032;
/// `FcitxKey_BackSpace`.
const KEY_BACKSPACE: u32 = 0xff08;
/// `FcitxKey_Escape`.
const KEY_ESCAPE: u32 = 0xff1b;
/// `FcitxKey_Left`, which the shipped configuration binds as a caret key.
const KEY_LEFT: u32 = 0xff51;

/// The one civil-time snapshot every quick-input test pins its formats against.
///
/// The same snapshot `tests/quick.rs` pins, for the same reason: the clock is a
/// once-per-process slot, and whichever test installs it first decides what every
/// other test of either suite reads.
const SNAPSHOT: ClockSnapshot = ClockSnapshot {
    year: 2026,
    month: 10,
    day: 28,
    hour: 12,
    minute: 34,
    second: 56,
    weekday: 2,
};

/// A clock that always answers [`SNAPSHOT`].
struct FixedClock;

impl ClockSource for FixedClock {
    fn now(&self) -> ClockSnapshot {
        SNAPSHOT
    }
}

/// Installs the pinned clock, once per test process.
fn pinned_clock() {
    let _ = install_clock(std::sync::Arc::new(FixedClock));
}

/// One entry of the fixture's aux column, keyed by the word it codes.
type Codes<'a> = &'a [(&'static str, &'static str)];

/// The aux-column view the tests hand the router.
///
/// A missing word answers `None` — the unrecorded-row shape of a real column —
/// and `present: false` is the container without the column at all.
struct MockColumn {
    present: bool,
    /// When set, every code read fails instead: the defensive arm for a
    /// container whose verification lied.
    unreadable: bool,
    codes: BTreeMap<&'static str, &'static str>,
}

impl MockColumn {
    /// A present column with `codes` and every other word unrecorded.
    fn with_codes(codes: Codes<'_>) -> Self {
        Self {
            present: true,
            unreadable: false,
            codes: codes.iter().copied().collect(),
        }
    }

    /// A column whose reads fail, the condition the contract calls impossible.
    fn unreadable() -> Self {
        Self {
            present: true,
            unreadable: true,
            codes: BTreeMap::new(),
        }
    }
}

impl AuxSource for MockColumn {
    fn has_aux_column(&self) -> bool {
        self.present
    }

    fn aux_code(&self, word: &str) -> Result<Option<&str>, DictError> {
        if self.unreadable {
            return Err(DictError::MagicMismatch);
        }
        if !self.present {
            return Ok(None);
        }
        Ok(self.codes.get(word).copied())
    }
}

/// A fixture whose dictionary carries four `ni` homophones, so a refinement has
/// membership to shrink and an order to keep.
fn aux_fixture(label: &str) -> Fixture {
    let name = format!("rspinyin-router-aux-{label}-{}", std::process::id());
    let dir = std::env::temp_dir().join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    Fixture {
        lexicon: TestLexicon::new(&[
            ("ni", "你", 900_000, 1),
            ("ni", "尼", 800_000, 1),
            ("ni", "泥", 700_000, 1),
            ("ni", "妮", 600_000, 1),
        ]),
        user: Box::new(SilentUser),
        lm: InMemoryLm::new(),
        decoder: Decoder::default(),
        dir,
    }
}

/// The routing configuration a refinement test runs under: the feature on.
///
/// The switch is the document's own `[engine] aux_refinement`, projected the same
/// way the running plugin projects it.
fn aux_config() -> RoutingConfig {
    let mut document = ime_config::Config::default();
    document.engine.aux_refinement = true;
    let (config, _) = RoutingConfig::from_config(&document);
    config
}

/// The configuration for the test that runs the view beside the quick panel:
/// both features on, the date trigger live.
fn aux_and_quick_config() -> RoutingConfig {
    let mut document = ime_config::Config::default();
    document.engine.aux_refinement = true;
    document.quick_input.enabled = true;
    let (config, _) = RoutingConfig::from_config(&document);
    config
}

/// A router with the refinement switch on and `source` answering its column.
fn aux_router<'a>(fixture: &'a Fixture, source: &'a MockColumn) -> KeyRouter<'a> {
    fixture
        .router_with(
            PhraseHandle::new(PhraseStore::load(None, 100, true)),
            aux_config(),
        )
        .with_aux_source(source)
}

/// A router with the shipped configuration: the refinement switch off.
fn disabled_router(fixture: &Fixture) -> KeyRouter<'_> {
    fixture.router(PhraseHandle::new(PhraseStore::load(None, 100, true)))
}

/// The snapshot order of the fixture's `ni` list, best first.
///
/// `泥` is deliberately the word whose first character carries no code: the
/// unrecorded-row shape of a real column.
const SNAPSHOT_ORDER: [&str; 4] = ["你", "尼", "泥", "妮"];

/// The fixture's aux column: `你=ab`, `尼=cd`, `妮=ef`, and `泥` unrecorded.
fn fixture_codes() -> Codes<'static> {
    &[("你", "ab"), ("尼", "cd"), ("妮", "ef")]
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

#[test]
fn test_aux_gate_disabled_passes_the_lead_key_through_and_composes_on() {
    // The shipped configuration: `[engine] aux_refinement` off, which is what
    // every build before the session shipped with. The lead key keeps the fate
    // the table gave it -- handed back -- and the composition it would have
    // refined keeps answering every key it always answered.
    let fixture = aux_fixture("disabled");
    let column = MockColumn::with_codes(fixture_codes());
    let mut router = disabled_router(&fixture).with_aux_source(&column);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    let frames_before = host.frames;
    assert!(
        !router.key_event(IC, &press(KEY_GRAVE, 0), &mut host),
        "a refused gate hands the key back"
    );
    assert_eq!(host.frames, frames_before, "no view frame was posted");
    assert!(host.diagnostics.is_empty(), "{:?}", host.diagnostics);
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("ni")),
        "the composition the gate refused to refine is untouched"
    );
    // The ordinary gestures still act: space commits the decode's own first
    // candidate, exactly as it did before the feature existed.
    assert!(router.key_event(IC, &press(KEY_SPACE, 0), &mut host));
    assert_eq!(host.commits, ["你"]);
}

#[test]
fn test_aux_gate_other_domain_temp_english_passes_the_lead_key_through() {
    // Temporary English owns the input, so no session may open over it: the
    // first gate names the reason and the key travels to the application, the
    // keys of that mode included.
    let fixture = aux_fixture("temp-english");
    let column = MockColumn::with_codes(fixture_codes());
    let mut router = aux_router(&fixture, &column);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    assert!(router.key_event(IC, &press(KEY_E, CTRL | SHIFT), &mut host));
    assert!(
        !router.key_event(IC, &press(KEY_GRAVE, 0), &mut host),
        "the foreign domain refuses the session and hands the key back"
    );
    assert!(host.diagnostics.is_empty(), "{:?}", host.diagnostics);
    assert!(host.last_frame.is_none(), "no view opened over the mode");
}

#[test]
fn test_aux_gate_other_domain_quick_panel_passes_the_lead_key_through() {
    // The quick panel owns the window while it is up, so the lead key passes:
    // the panel's own frame stays on screen and the backtick goes to the
    // application, the same key it was before either feature existed.
    pinned_clock();
    let fixture = aux_fixture("quick-domain");
    let column = MockColumn::with_codes(fixture_codes());
    let phrases = PhraseHandle::new(PhraseStore::load(None, 100, true));
    let mut router = fixture
        .router_with(phrases, aux_and_quick_config())
        .with_aux_source(&column);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    assert!(router.key_event(IC, &press(0x0072, 0), &mut host), "r");
    assert!(router.key_event(IC, &press(0x0071, 0), &mut host), "q");
    assert_eq!(
        frame(&host).preedit.text,
        "rq",
        "the panel is up, preediting its own trigger"
    );
    assert!(
        !router.key_event(IC, &press(KEY_GRAVE, 0), &mut host),
        "a domain that owns the input refuses the session"
    );
    assert_eq!(
        frame(&host).preedit.text,
        "rq",
        "the panel's frame is still the one on screen"
    );
    assert!(host.diagnostics.is_empty(), "{:?}", host.diagnostics);
}

#[test]
fn test_aux_gate_no_column_passes_the_lead_key_through() {
    // The router assembled no aux-column view: the degradation a container
    // without the column answers with, and the same shape as every build the
    // startup sequence of which installs no reader yet.
    let fixture = aux_fixture("no-column");
    let mut router = aux_config_router_without_source(&fixture);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    assert!(
        !router.key_event(IC, &press(KEY_GRAVE, 0), &mut host),
        "the absent column refuses the session"
    );
    assert!(
        host.diagnostics.is_empty(),
        "the absent-column report is a process diagnostic, not a host error: {:?}",
        host.diagnostics
    );
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("ni")),
        "the composition survives the refused open"
    );
}

/// A router with the refinement on and no column view installed.
fn aux_config_router_without_source(fixture: &Fixture) -> KeyRouter<'_> {
    fixture.router_with(
        PhraseHandle::new(PhraseStore::load(None, 100, true)),
        aux_config(),
    )
}

#[test]
fn test_aux_gate_no_candidates_passes_the_lead_key_through() {
    // Nothing is composing, so there is no list to refine: the backtick is the
    // application's, which is the fate every idle backtick has ever had.
    let fixture = aux_fixture("no-candidates");
    let column = MockColumn::with_codes(fixture_codes());
    let mut router = aux_router(&fixture, &column);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    assert!(!router.key_event(IC, &press(KEY_GRAVE, 0), &mut host));
    assert!(host.last_frame.is_none(), "an idle session opens no window");
}

#[test]
fn test_aux_open_refine_keeps_order_and_shrinks_membership() {
    // The invariant the card pins, observed through the router: after the open
    // the frame shows the snapshot verbatim, and every refinement letter leaves
    // the frame an ordered subsequence of it -- membership shrinks, order never
    // moves. Backspacing grows the list back the same way.
    let fixture = aux_fixture("refine");
    let column = MockColumn::with_codes(fixture_codes());
    let mut router = aux_router(&fixture, &column);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    assert!(router.key_event(IC, &press(KEY_GRAVE, 0), &mut host));
    assert_eq!(shown(&host), SNAPSHOT_ORDER, "entry shows the snapshot");
    assert_eq!(frame(&host).preedit.text, "ni`", "the lead mark is on show");

    assert!(router.key_event(IC, &press(KEY_A, 0), &mut host), "a");
    assert_eq!(shown(&host), ["你"], "`a` keeps only the `ab` candidate");
    assert_eq!(frame(&host).preedit.text, "ni`a", "the code is on show");

    assert!(router.key_event(IC, &press(KEY_BACKSPACE, 0), &mut host));
    assert_eq!(
        shown(&host),
        SNAPSHOT_ORDER,
        "stepping back grows the list in snapshot order"
    );
    // The word with no recorded code drops out of every filter and comes back
    // only at the entry state: `c` names `尼` alone.
    assert!(router.key_event(IC, &press(KEY_C, 0), &mut host), "c");
    assert_eq!(shown(&host), ["尼"], "`c` keeps only the `cd` candidate");

    assert!(router.key_event(IC, &press(KEY_BACKSPACE, 0), &mut host));
    assert_eq!(
        shown(&host),
        SNAPSHOT_ORDER,
        "the unrecorded candidate comes back with its place in line"
    );
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("ni")),
        "no refinement letter ever reached the pinyin buffer"
    );
}

#[test]
fn test_aux_backspace_to_entry_closes_and_restores_the_snapshot() {
    // The close the contract names: a backspace on the entry state is consumed
    // -- so the pinyin underneath keeps its letter -- and the session's own
    // frame goes back on screen.
    let fixture = aux_fixture("backspace-close");
    let column = MockColumn::with_codes(fixture_codes());
    let mut router = aux_router(&fixture, &column);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    assert!(router.key_event(IC, &press(KEY_GRAVE, 0), &mut host));
    assert!(router.key_event(IC, &press(KEY_A, 0), &mut host));
    assert!(router.key_event(IC, &press(KEY_BACKSPACE, 0), &mut host));
    assert_eq!(
        frame(&host).preedit.text,
        "ni`",
        "the first backspace stepped the code back, not out"
    );

    assert!(
        router.key_event(IC, &press(KEY_BACKSPACE, 0), &mut host),
        "the backspace at the entry point is the view's"
    );
    assert_eq!(
        frame(&host).preedit.text,
        "ni",
        "the session's own frame is back, without the lead mark"
    );
    assert_eq!(shown(&host), SNAPSHOT_ORDER, "the snapshot, verbatim");
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("ni")),
        "the pinyin lost no letter to the closing backspace"
    );
    // The composition continues to own the buffer: the next backspace is the
    // session's again, and the buffer stores syllables, not letters — one press
    // takes the whole "ni" syllable out and ends the composition with it.
    assert!(router.key_event(IC, &press(KEY_BACKSPACE, 0), &mut host));
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle),
    );
}

#[test]
fn test_aux_escape_restores_the_snapshot_and_keeps_the_composition() {
    // Escape leaves the session without committing anything and without ending
    // the composition: the full list is back, and the next Escape -- no longer
    // answered by a view -- is the cancellation it always was.
    let fixture = aux_fixture("escape");
    let column = MockColumn::with_codes(fixture_codes());
    let mut router = aux_router(&fixture, &column);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    assert!(router.key_event(IC, &press(KEY_GRAVE, 0), &mut host));
    assert!(router.key_event(IC, &press(KEY_A, 0), &mut host));
    assert_eq!(shown(&host), ["你"]);

    assert!(router.key_event(IC, &press(KEY_ESCAPE, 0), &mut host));
    assert!(host.commits.is_empty(), "{:?}", host.commits);
    assert_eq!(frame(&host).preedit.text, "ni", "the snapshot is restored");
    assert_eq!(shown(&host), SNAPSHOT_ORDER, "verbatim, in snapshot order");
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("ni")),
        "the composition the view refined survives the escape"
    );

    // The second Escape is the session's own again.
    assert!(router.key_event(IC, &press(KEY_ESCAPE, 0), &mut host));
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle)
    );
}

#[test]
fn test_aux_digit_commits_the_kept_target_and_ends_the_composition() {
    // The digit names its page position on the kept list: with one letter typed
    // the second position is `尼`, the word the filter left there -- and the
    // commit ends the composition, the way an ordinary selection does.
    let fixture = aux_fixture("digit");
    let column = MockColumn::with_codes(fixture_codes());
    let mut router = aux_router(&fixture, &column);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    assert!(router.key_event(IC, &press(KEY_GRAVE, 0), &mut host));
    assert!(router.key_event(IC, &press(KEY_A, 0), &mut host));
    // `a` keeps `你` alone, so digit 2 names nothing on the kept list: the
    // digit goes back to the application, the way a digit past a page is on a
    // decoded list, and the frame it refused to act on stays as it is.
    assert!(!router.key_event(IC, &press(KEY_2, 0), &mut host));
    assert!(host.commits.is_empty(), "{:?}", host.commits);
    assert_eq!(shown(&host), ["你"], "the refused digit changed nothing");

    // Step the code back: `尼` is the second candidate again, and the digit
    // commits exactly that word.
    assert!(router.key_event(IC, &press(KEY_BACKSPACE, 0), &mut host));
    assert!(router.key_event(IC, &press(KEY_2, 0), &mut host));
    assert_eq!(host.commits, ["尼"], "the kept target is what commits");
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle),
        "the composition ends with the commit"
    );
    assert!(host.hides >= 1, "the window goes with it");
}

#[test]
fn test_aux_space_commits_the_highlighted_kept_candidate() {
    let fixture = aux_fixture("space");
    let column = MockColumn::with_codes(fixture_codes());
    let mut router = aux_router(&fixture, &column);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    assert!(router.key_event(IC, &press(KEY_GRAVE, 0), &mut host));
    assert!(router.key_event(IC, &press(KEY_C, 0), &mut host));
    assert_eq!(shown(&host), ["尼"], "the filter moved `尼` to the front");
    assert!(router.key_event(IC, &press(KEY_SPACE, 0), &mut host));
    assert_eq!(
        host.commits,
        ["尼"],
        "the highlight commits what it sits on"
    );
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle)
    );
}

#[test]
fn test_aux_click_commits_the_clicked_kept_candidate() {
    let fixture = aux_fixture("click");
    let column = MockColumn::with_codes(fixture_codes());
    let mut router = aux_router(&fixture, &column);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    assert!(router.key_event(IC, &press(KEY_GRAVE, 0), &mut host));
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
        "a click on a live view candidate is the view's"
    );
    assert_eq!(host.commits, ["尼"], "the click commits its own candidate");
    assert_eq!(
        router.session(IC).map(|session| session.state),
        Some(SessionState::Idle)
    );

    // A click carrying any other revision is stale: nothing commits and the
    // answer claims nothing, exactly as it did before the view existed.
    assert!(!router.ui_event(
        IC,
        UiEvent::Select {
            revision,
            index: 0,
            trigger: SelectTrigger::Mouse,
        },
        &mut host,
    ));
    assert_eq!(host.commits.len(), 1, "{:?}", host.commits);
}

#[test]
fn test_aux_unowned_key_falls_through_to_the_pipeline() {
    // A key the view does not own closes it and answers through the pipeline:
    // the caret key does what it always did on the composition, and the frame
    // on screen is the session's own again.
    let fixture = aux_fixture("fallthrough");
    let column = MockColumn::with_codes(fixture_codes());
    let mut router = aux_router(&fixture, &column);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    assert!(router.key_event(IC, &press(KEY_GRAVE, 0), &mut host));
    assert_eq!(frame(&host).preedit.text, "ni`");

    assert!(
        router.key_event(IC, &press(KEY_LEFT, 0), &mut host),
        "the caret key is the pipeline's to act on"
    );
    assert_eq!(
        frame(&host).preedit.text,
        "ni",
        "the view closed: the session's own frame is back"
    );
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("ni")),
        "the caret key typed nothing into the buffer"
    );
}

#[test]
fn test_aux_reload_closes_the_view_and_keeps_the_composition() {
    // A reload that switches the refinement off closes the view; the
    // composition it refined survives it, per the reload rule, and the next
    // lead press is refused by the disabled switch.
    let fixture = aux_fixture("reload");
    let column = MockColumn::with_codes(fixture_codes());
    let mut router = aux_router(&fixture, &column);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    assert!(router.key_event(IC, &press(KEY_GRAVE, 0), &mut host));
    assert!(router.key_event(IC, &press(KEY_A, 0), &mut host));
    assert_eq!(shown(&host), ["你"]);

    router.reload(RoutingConfig::default(), &mut host);
    assert_eq!(
        frame(&host).preedit.text,
        "ni",
        "the view closed with the reload; the session repainted its own frame"
    );
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("ni")),
        "the reload rule keeps the composition"
    );
    assert!(
        !router.key_event(IC, &press(KEY_GRAVE, 0), &mut host),
        "the switch the reload carried is off"
    );
    // The composition still answers its ordinary keys: space commits the
    // decode's own first candidate.
    assert!(router.key_event(IC, &press(KEY_SPACE, 0), &mut host));
    assert_eq!(host.commits, ["你"]);
}

#[test]
fn test_aux_unreadable_code_aborts_the_open_and_hands_the_key_back() {
    // A code the column cannot resolve is the condition the contract calls
    // impossible for a verified container; the open aborts, the key travels
    // back, and the composition is untouched rather than refined against
    // guessed codes.
    let fixture = aux_fixture("unreadable");
    let column = MockColumn::unreadable();
    let mut router = aux_router(&fixture, &column);
    router.activate_reported(IC, ordinary_report());
    let mut host = RecordingHost::default();

    type_ni(&mut router, &mut host);
    assert!(
        !router.key_event(IC, &press(KEY_GRAVE, 0), &mut host),
        "an open that cannot resolve its codes does not take the key"
    );
    assert_eq!(
        router
            .session(IC)
            .map(|session| session.buf.raw().to_owned()),
        Some(String::from("ni"))
    );
    // The next key is the pipeline's: the ordinary selection still commits.
    assert!(router.key_event(IC, &press(KEY_SPACE, 0), &mut host));
    assert_eq!(host.commits, ["你"]);
}

#[test]
fn test_aux_lead_conflict_reports_one_diagnostic_per_refused_press() {
    // The occupied-lead-key gate owes a host-visible warning, once per refused
    // press, under the feature's own configuration key.
    let mut host = RecordingHost::default();
    let mut reported = false;
    aux::report_refusal(AuxPassReason::LeadKeyOccupied, IC, &mut reported, &mut host);
    assert_eq!(host.diagnostics.len(), 1, "{:?}", host.diagnostics);
    let line = &host.diagnostics[0];
    assert!(line.starts_with("config/invalid:"), "{line}");
    assert!(
        line.contains("engine.aux_refinement"),
        "the conflict names the feature's key: {line}"
    );
    assert!(
        !reported,
        "the conflict does not spend the absent-column flag"
    );

    // A second refused press warns again -- once per press, never twice.
    aux::report_refusal(AuxPassReason::LeadKeyOccupied, IC, &mut reported, &mut host);
    assert_eq!(host.diagnostics.len(), 2, "{:?}", host.diagnostics);

    // The silent gates stay silent: a disabled switch and an idle backtick
    // name the shipped configuration, and the host sees nothing.
    aux::report_refusal(AuxPassReason::Disabled, IC, &mut reported, &mut host);
    aux::report_refusal(AuxPassReason::NoCandidates, IC, &mut reported, &mut host);
    aux::report_refusal(AuxPassReason::OtherDomain, IC, &mut reported, &mut host);
    assert_eq!(host.diagnostics.len(), 2, "{:?}", host.diagnostics);
}

#[test]
fn test_aux_lead_key_is_free_under_every_character_binding() {
    // No binding the schema can spell holds the lead mark, so the gate's
    // occupied arm has nothing to refuse today. Pinned over every variant of
    // the mark type: if the schema grows a key that binds the backtick, this
    // test fails and the gate becomes live instead of silently shadowed.
    let marks = [
        SelectCharKey::Off,
        SelectCharKey::Exclamation,
        SelectCharKey::Quote,
        SelectCharKey::ParenLeft,
        SelectCharKey::ParenRight,
        SelectCharKey::Comma,
        SelectCharKey::Period,
        SelectCharKey::Colon,
        SelectCharKey::Semicolon,
        SelectCharKey::Question,
        SelectCharKey::At,
        SelectCharKey::BracketLeft,
        SelectCharKey::BracketRight,
    ];
    for mark in marks {
        let keys = KeyBindings {
            select_first_char: mark,
            ..KeyBindings::default()
        };
        assert!(
            aux::lead_key_free(&keys),
            "no select_first_char binding holds the lead mark: {mark:?}"
        );
        let keys = KeyBindings {
            select_last_char: mark,
            ..KeyBindings::default()
        };
        assert!(
            aux::lead_key_free(&keys),
            "no select_last_char binding holds the lead mark: {mark:?}"
        );
    }
}

#[test]
fn test_is_aux_lead_press_answers_the_bare_grave_press_only() {
    use crate::engine::is_aux_lead_press;
    assert!(is_aux_lead_press(&press(KEY_GRAVE, 0)));
    // A release is the application's key-up, whatever key it belongs to.
    assert!(!is_aux_lead_press(&FcitxKeyEvent {
        sym: KEY_GRAVE,
        state: 0,
        is_release: true,
        time_ms: 0,
    }));
    // Shift produces the tilde's own keysym, and any held modifier makes the
    // key the application's chord -- the exact-modifier rule the chord rows
    // apply.
    assert!(!is_aux_lead_press(&press(0x007e, 0)));
    for state in [SHIFT, CTRL, CTRL | SHIFT] {
        assert!(
            !is_aux_lead_press(&press(KEY_GRAVE, state)),
            "grave with {state:#x} held is not the lead key"
        );
    }
    // Another key is never the lead.
    assert!(!is_aux_lead_press(&press(KEY_A, 0)));
}

#[test]
fn test_aux_absent_column_refusal_spends_the_once_flag_once() {
    // The absent-column report is a once-per-process diagnostic: the first
    // refusal spends the router's flag, and the flag is what keeps every later
    // one silent. The report travels the process diagnostic channel, which the
    // test host cannot read, so the flag is what the test pins.
    let mut reported = false;
    aux::report_refusal(
        AuxPassReason::ColumnAbsent,
        IC,
        &mut reported,
        &mut RecordingHost::default(),
    );
    assert!(reported, "the first refusal spends the process flag");

    aux::report_refusal(
        AuxPassReason::ColumnAbsent,
        IC,
        &mut reported,
        &mut RecordingHost::default(),
    );
    assert!(
        reported,
        "the flag stays spent: the guard kept the second report silent"
    );

    // No silent gate touches the flag, either way.
    aux::report_refusal(
        AuxPassReason::Disabled,
        IC,
        &mut reported,
        &mut RecordingHost::default(),
    );
    assert!(reported);
}
