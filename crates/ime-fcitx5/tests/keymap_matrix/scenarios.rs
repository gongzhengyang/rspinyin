//! The keyboard-only end-to-end walks: one test per row of the interaction plan's
//! scenario table, driven through the routing table, the session and the host
//! boundary with no mouse anywhere in the loop.
//!
//! The table is `docs/dev/opt-keymap/phase-3.md`'s 14-row walk of the core flows.
//! The rows are the document's data, so the scenario ids below are quoted exactly as
//! they are written there, and the mapping from row to test is kept in one place:
//!
//! | Scenario | Walk | Test |
//! |---|---|---|
//! | `SC-KEY-01` basic input | `n i h a o Space` | `test_scenario_01_basic_input_commits_the_phrase_and_hides_the_window` |
//! | `SC-KEY-02` digit select | `n i h a o 3` | `test_scenario_02_digit_select_commits_the_third_candidate` |
//! | `SC-KEY-03` highlight move | `n i h a o Tab Tab Space` | `test_scenario_03_highlight_move_commits_the_same_candidate_as_the_digit` |
//! | `SC-KEY-04` paging | `n i h a o = 1` | `test_scenario_04_page_flip_selects_the_first_candidate_of_page_two` |
//! | `SC-KEY-05` syllable delete | `n i h a o BackSpace` | `test_scenario_05_backspace_removes_a_syllable_leaving_the_ni_readings` |
//! | `SC-KEY-06` caret move | `n i h a o Left` | `test_scenario_06_caret_left_lands_after_the_ni_syllable` |
//! | `SC-KEY-07` cancel | `n i h a o Escape` | `test_scenario_07_escape_delivers_no_text_and_hides_the_window` |
//! | `SC-KEY-08` mode chord | `Ctrl+Space` then letters | `test_scenario_08_the_disabled_context_hands_the_letters_back` |
//! | `SC-KEY-09` temporary English | `Ctrl+Shift+E n i h a o Return n i h a o Space` | `test_scenario_09_temporary_english_passes_the_first_phrase_through` |
//! | `SC-KEY-10` full width | `Shift+Space n i h a o Space` | `test_scenario_10_full_width_commits_the_phrase_and_flags_the_strip` |
//! | `SC-KEY-11` punctuation | `Ctrl+. ,` | `test_scenario_11_punctuation_english_hands_the_comma_back` |
//! | `SC-KEY-12` cheat sheet | hold `Shift` 300 ms, release | `test_scenario_12_long_press_shift_offers_the_cheat_sheet_and_keeps_the_mode` |
//! | `SC-KEY-13` focus loss | `n i h a o` and the focus goes | `test_scenario_13_focus_loss_delivers_no_text_and_hides_the_window` |
//! | `SC-KEY-14` no focus steal | the walks above, throughout | `test_scenario_14_focus_window_id_never_changes` |
//!
//! # Determinism
//!
//! The matrix's rules hold here too: no case reads a file, a clock, an environment
//! variable or a display server. The long-press row drives the bus's hold machine
//! with literal timestamps, which is why the gesture needs no sleeping, and the
//! focus-loss row drives the host call a focus change produces rather than a real
//! focus change.
//!
//! # Where the card's table and the engine differ
//!
//! The engine's edit unit is the syllable, not the character: Backspace removes the
//! trailing syllable and the caret steps on syllable boundaries. Two rows of the
//! card's table are written for a per-character edit model, and their stated
//! outcomes land one step earlier than their step lists imply; each walk drives the
//! row's steps and asserts the row's outcome where the engine produces it, and the
//! test that carries the row documents the difference.

use ime_types::HideReason;
use rspinyin::engine::{
    Consumed, Dispatcher, HoldOutcome, KeyBindings, KeyEvent, KeyRouter, Overlay, SessionView,
};
use rspinyin::ffi::FcitxKeyEvent;

use crate::support::{
    CTRL, FOCUS_WINDOW_ID, Fixture, IC, KEY_1, KEY_3, KEY_A, KEY_BACKSPACE, KEY_COMMA, KEY_E,
    KEY_EQUAL, KEY_ESCAPE, KEY_H, KEY_I, KEY_LEFT, KEY_N, KEY_O, KEY_PERIOD, KEY_RETURN,
    KEY_SHIFT_L, KEY_SPACE, KEY_TAB, RecordingHost, SHIFT, Situation, press,
};

// ── The shared strokes ───────────────────────────────────────────────────────────

/// The letters of the walk, in the order the table types them: `n i h a o`.
const NIHAO: [(u32, u32); 5] = [(KEY_N, 0), (KEY_I, 0), (KEY_H, 0), (KEY_A, 0), (KEY_O, 0)];

/// The candidate the phrase decodes to, and the one the first page opens with.
///
/// The two-syllable entry outranks every single-character pairing: its edge carries
/// the length bonus and its path pays one segment of the path penalty where a
/// pairing pays two, so the phrase is the top of the list on every run.
const PHRASE: &str = "你好";

/// Types `nihao` through the router, asserting every letter is kept.
///
/// The letters are the input; a letter handed back mid-composition would be the
/// swallowed-key defect, so the walk refuses to continue past one.
fn type_nihao(router: &mut KeyRouter<'_>, host: &mut RecordingHost) {
    for &(sym, state) in &NIHAO {
        assert!(
            router.key_event(IC, &press(sym, state), host),
            "the letter {sym:#06x} is kept while the phrase is being typed"
        );
    }
}

/// Routes one key the way the host thread does: a context the host reports disabled
/// is never consulted, and its keys go straight to the application.
///
/// Fcitx5 stops calling the engine once the input method is switched off; the
/// recorder's `is_enabled` is the recorded host state, and this gate is the model of
/// the host's half of the chain. Without it the harness would keep asking an engine
/// the host no longer calls.
fn route_key(router: &mut KeyRouter<'_>, host: &mut RecordingHost, event: FcitxKeyEvent) -> bool {
    if host.is_enabled {
        router.key_event(IC, &event, host)
    } else {
        false
    }
}

/// The texts of the candidates the newest frame shows, in display order.
fn page_candidate_texts(host: &RecordingHost) -> Vec<String> {
    host.newest_frame
        .as_ref()
        .map(|frame| {
            frame
                .candidates
                .iter()
                .map(|candidate| candidate.text.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// What one selection walk produced.
struct Selection {
    /// The host the walk drove, with the walk's commits, hides and frames.
    host: RecordingHost,
    /// The text the walk committed.
    commit: String,
    /// The candidate texts of the page on show at the moment of selection.
    page: Vec<String>,
}

/// Types `nihao` and names the third candidate with the digit `3`.
fn select_with_digit_three() -> Selection {
    let fixture = Fixture::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router(KeyBindings::default());
    type_nihao(&mut router, &mut host);
    let page = page_candidate_texts(&host);
    assert!(
        page.len() >= 3,
        "the fixture offers a third candidate to name: {page:?}"
    );
    assert!(
        router.key_event(IC, &press(KEY_3, 0), &mut host),
        "the digit names a candidate while a composition is live"
    );
    assert!(
        !host.commits.is_empty(),
        "the digit committed the candidate it named"
    );
    let commit = host.commits.last().cloned().unwrap_or_default();
    Selection { host, commit, page }
}

/// Types `nihao`, moves the highlight two candidates down with `Tab` and commits
/// with `Space`.
fn select_with_tab_tab_space() -> Selection {
    let fixture = Fixture::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router(KeyBindings::default());
    type_nihao(&mut router, &mut host);
    let page = page_candidate_texts(&host);
    assert!(
        page.len() >= 3,
        "the fixture offers a third candidate to move to: {page:?}"
    );
    for step in 1..=2 {
        assert!(
            router.key_event(IC, &press(KEY_TAB, 0), &mut host),
            "Tab moves the highlight, step {step}"
        );
    }
    assert!(
        router.key_event(IC, &press(KEY_SPACE, 0), &mut host),
        "Space commits the highlighted candidate"
    );
    assert!(
        !host.commits.is_empty(),
        "Space committed the candidate the highlight was on"
    );
    let commit = host.commits.last().cloned().unwrap_or_default();
    Selection { host, commit, page }
}

// ── The walks ────────────────────────────────────────────────────────────────────

/// `SC-KEY-01`: the plain input path. The application receives the phrase and the
/// candidate window is hidden once it is committed.
#[test]
fn test_scenario_01_basic_input_commits_the_phrase_and_hides_the_window() {
    let fixture = Fixture::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router(KeyBindings::default());

    type_nihao(&mut router, &mut host);
    assert!(
        host.newest_candidates >= 1,
        "the composition put a candidate window on screen"
    );

    assert!(
        router.key_event(IC, &press(KEY_SPACE, 0), &mut host),
        "Space commits the highlighted candidate"
    );
    assert_eq!(
        host.commits,
        vec![PHRASE],
        "the application received the phrase"
    );
    assert_eq!(
        host.hides,
        vec![HideReason::Committed],
        "the window was hidden because the commit landed"
    );
    assert!(
        host.diagnostics.is_empty(),
        "a clean walk reports nothing: {:?}",
        host.diagnostics
    );
}

/// `SC-KEY-02`: the digit path. The application receives the third candidate, which
/// is not the phrase the first page opens with.
#[test]
fn test_scenario_02_digit_select_commits_the_third_candidate() {
    let Selection { host, commit, page } = select_with_digit_three();

    assert_eq!(
        Some(commit.as_str()),
        page.get(2).map(String::as_str),
        "the digit named the third candidate on the page on show"
    );
    assert_ne!(
        commit, PHRASE,
        "the third candidate differs from the one the plain path commits"
    );
    assert_eq!(
        host.hides,
        vec![HideReason::Committed],
        "the selection hid the window"
    );
}

/// `SC-KEY-03`: the highlight path. `Tab` `Tab` `Space` lands on the third candidate
/// and commits exactly what the digit `3` commits in `SC-KEY-02`: the keyboard path
/// and the digit path are equivalent.
#[test]
fn test_scenario_03_highlight_move_commits_the_same_candidate_as_the_digit() {
    let via_digit = select_with_digit_three();
    let via_tabs = select_with_tab_tab_space();

    assert_eq!(
        Some(via_tabs.commit.as_str()),
        via_tabs.page.get(2).map(String::as_str),
        "two Tabs put the highlight on the third candidate"
    );
    assert_eq!(
        via_tabs.commit, via_digit.commit,
        "the highlight path and the digit path commit the same candidate"
    );
    assert_ne!(
        via_tabs.commit, PHRASE,
        "the walk left the candidate the plain path commits"
    );
}

/// `SC-KEY-04`: the paging path. `=` turns to the second page and `1` commits that
/// page's first candidate.
#[test]
fn test_scenario_04_page_flip_selects_the_first_candidate_of_page_two() {
    let fixture = Fixture::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router(KeyBindings::default());

    type_nihao(&mut router, &mut host);
    assert!(
        host.newest_page.map_or(0, |page| page.total) >= 2,
        "the fixture offers a second page to turn to"
    );

    assert!(
        router.key_event(IC, &press(KEY_EQUAL, 0), &mut host),
        "the shipped configuration binds = to page forward"
    );
    let frame = host
        .newest_frame
        .as_ref()
        .expect("the paged frame is on record");
    assert_eq!(frame.page.current, 2, "the second page is on show");
    let expected = frame.candidates[0].text.clone();

    assert!(
        router.key_event(IC, &press(KEY_1, 0), &mut host),
        "digit 1 names the first item of the page on show"
    );
    assert_eq!(
        host.commits,
        vec![expected],
        "the commit is the candidate the second page opened with"
    );
}

/// `SC-KEY-05`: the syllable-deletion path. Backspace removes the trailing
/// syllable, leaving the preedit at `ni` with only the `ni` readings on show.
///
/// The card lists two BackSpaces for a `nihao` input; the engine deletes by the
/// syllable grid, so the first one lands on exactly the state the row describes and
/// the second takes the last syllable back, which empties the input and ends the
/// composition. Both steps are driven and both outcomes are asserted.
#[test]
fn test_scenario_05_backspace_removes_a_syllable_leaving_the_ni_readings() {
    let fixture = Fixture::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router(KeyBindings::default());

    type_nihao(&mut router, &mut host);
    assert!(
        router.key_event(IC, &press(KEY_BACKSPACE, 0), &mut host),
        "backspace removes the trailing syllable"
    );
    let frame = host
        .newest_frame
        .as_ref()
        .expect("the frame after the deletion is on record");
    assert_eq!(frame.preedit.text, "ni", "the preedit is what is left");
    assert!(
        !frame.candidates.is_empty(),
        "the remaining syllable still has readings"
    );
    for candidate in &frame.candidates {
        assert_eq!(
            candidate.consumed_syllables, 1,
            "only the single-syllable readings remain: {candidate:?}"
        );
    }

    assert!(
        router.key_event(IC, &press(KEY_BACKSPACE, 0), &mut host),
        "backspace on the last syllable ends the composition"
    );
    assert!(
        host.commits.is_empty(),
        "deleting commits nothing: {:?}",
        host.commits
    );
    assert_eq!(
        host.hides.last(),
        Some(&HideReason::EmptyInput),
        "an emptied input takes the window with it"
    );
}

/// `SC-KEY-06`: the caret path. `Left` moves the caret one syllable boundary back,
/// which puts it after the `ni` syllable, and the candidate list is untouched.
///
/// The caret steps on the boundaries of the syllable grid, so one step left of the
/// end of `nihao` is the boundary after `ni` -- the state the row describes. The
/// card lists two `Left`s; the second one reaches the start boundary, and the walk
/// drives it to pin that the caret stops at the ends of the input instead of
/// wrapping.
#[test]
fn test_scenario_06_caret_left_lands_after_the_ni_syllable() {
    let fixture = Fixture::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router(KeyBindings::default());

    type_nihao(&mut router, &mut host);
    let before = page_candidate_texts(&host);

    assert!(
        router.key_event(IC, &press(KEY_LEFT, 0), &mut host),
        "Left moves the caret"
    );
    let frame = host
        .newest_frame
        .as_ref()
        .expect("the frame after the caret move is on record");
    assert_eq!(
        frame.preedit.text, "ni'hao",
        "the preedit spells the phrase"
    );
    // A caret on a boundary belongs to the syllable that starts there, which puts
    // it after the separator in front of `hao`: offset 3 in `ni'hao`.
    assert_eq!(
        frame.preedit.caret, 3,
        "the caret sits after the ni syllable"
    );
    assert_eq!(
        page_candidate_texts(&host),
        before,
        "moving the caret changes the preedit and nothing else"
    );

    assert!(
        router.key_event(IC, &press(KEY_LEFT, 0), &mut host),
        "the second Left reaches the start boundary"
    );
    let frame = host
        .newest_frame
        .as_ref()
        .expect("the frame after the second caret move is on record");
    assert_eq!(
        frame.preedit.caret, 0,
        "the caret stops at the start of the input"
    );
    assert_eq!(
        page_candidate_texts(&host),
        before,
        "the candidates survive both moves"
    );
}

/// `SC-KEY-07`: the cancel path. The application has received no text at all -- the
/// whole commit log is empty, not a log of empty strings -- and the candidate
/// window is hidden.
#[test]
fn test_scenario_07_escape_delivers_no_text_and_hides_the_window() {
    let fixture = Fixture::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router(KeyBindings::default());

    type_nihao(&mut router, &mut host);
    assert!(
        router.key_event(IC, &press(KEY_ESCAPE, 0), &mut host),
        "Escape cancels a live composition"
    );
    assert!(
        host.commits.is_empty(),
        "the application received no text at all: {:?}",
        host.commits
    );
    assert_eq!(
        host.hides,
        vec![HideReason::Cancelled],
        "the window was hidden because the user cancelled"
    );
}

/// `SC-KEY-08`: the mode chord. `Ctrl+Space` hands the keyboard back to the
/// application, and the letters that follow reach it as the literal keystrokes
/// `nihao`.
#[test]
fn test_scenario_08_the_disabled_context_hands_the_letters_back() {
    let fixture = Fixture::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router(KeyBindings::default());

    assert!(
        router.key_event(IC, &press(KEY_SPACE, CTRL), &mut host),
        "the language chord is the plugin's in any state"
    );
    assert_eq!(
        host.toggles, 1,
        "the chord flipped the host's input-method state once"
    );
    assert!(!host.is_enabled, "the host reports the context disabled");

    for &(sym, state) in &NIHAO {
        assert!(
            !route_key(&mut router, &mut host, press(sym, state)),
            "with the input method off, {sym:#06x} reaches the application untranslated"
        );
    }
    assert!(
        host.commits.is_empty(),
        "the plugin committed nothing: the application received the literal \
         keystrokes nihao: {:?}",
        host.commits
    );
    assert_eq!(
        host.toggles, 1,
        "nothing in the walk switched the mode back"
    );
}

/// `SC-KEY-09`: the temporary-English path. The first `nihao` reaches the
/// application as raw keystrokes, the `Return` that ends the mode reaches it too,
/// and the second `nihao` composes and commits the phrase.
#[test]
fn test_scenario_09_temporary_english_passes_the_first_phrase_through() {
    let fixture = Fixture::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router(KeyBindings::default());

    assert!(
        router.key_event(IC, &press(KEY_E, CTRL | SHIFT), &mut host),
        "the chord enters temporary English"
    );

    for &(sym, state) in &NIHAO {
        assert!(
            !router.key_event(IC, &press(sym, state), &mut host),
            "in temporary English {sym:#06x} reaches the application"
        );
    }
    assert!(
        host.commits.is_empty(),
        "the first pass committed nothing: the application received the literal \
         text: {:?}",
        host.commits
    );
    assert!(
        !router.key_event(IC, &press(KEY_RETURN, 0), &mut host),
        "Return passes through and leaves the mode"
    );

    for &(sym, state) in &NIHAO {
        assert!(
            router.key_event(IC, &press(sym, state), &mut host),
            "with the mode off, {sym:#06x} composes again"
        );
    }
    assert!(
        router.key_event(IC, &press(KEY_SPACE, 0), &mut host),
        "Space commits the second pass"
    );
    assert_eq!(
        host.commits,
        vec![PHRASE],
        "the only commit of the walk is the second pass's phrase"
    );
}

/// `SC-KEY-10`: the full-width path. `Shift+Space` turns the plugin's output to
/// full width, the strip the candidate window draws flags it, and the phrase
/// commits as itself: full width rewrites the plugin's own punctuation output, not
/// a Chinese candidate.
#[test]
fn test_scenario_10_full_width_commits_the_phrase_and_flags_the_strip() {
    let fixture = Fixture::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router(KeyBindings::default());

    assert!(
        router.key_event(IC, &press(KEY_SPACE, SHIFT), &mut host),
        "the full-width chord is the plugin's"
    );
    assert_eq!(
        host.toggles, 0,
        "the switch is the plugin's own bit, not the host's"
    );

    type_nihao(&mut router, &mut host);
    let frame = host
        .newest_frame
        .as_ref()
        .expect("the composing frame is on record");
    assert!(
        frame.status.full_width,
        "the strip the window draws flags full width"
    );

    assert!(
        router.key_event(IC, &press(KEY_SPACE, 0), &mut host),
        "Space commits the highlighted candidate"
    );
    assert_eq!(
        host.commits,
        vec![PHRASE],
        "the application received the full-width text: the phrase itself"
    );
}

/// `SC-KEY-11`: the punctuation path. `Ctrl+.` switches the plugin's punctuation
/// output to English, and the `,` the user presses reaches the application as the
/// English comma they typed: the composing keymap has no comma of its own to take.
#[test]
fn test_scenario_11_punctuation_english_hands_the_comma_back() {
    let fixture = Fixture::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router(KeyBindings::default());

    assert!(
        router.key_event(IC, &press(KEY_PERIOD, CTRL), &mut host),
        "the punctuation chord is the plugin's"
    );
    assert_eq!(
        host.toggles, 0,
        "the switch is the plugin's own bit, not the host's"
    );
    assert!(
        !router.key_event(IC, &press(KEY_COMMA, 0), &mut host),
        "the comma is the application's: it arrives as the English comma the user typed"
    );
    assert!(
        host.commits.is_empty(),
        "the plugin committed nothing on the comma: {:?}",
        host.commits
    );
}

/// `SC-KEY-12`: the cheat-sheet gesture. A `Shift` held for 300 ms with nothing
/// typed is a long press, the release that ends it is the plugin's, the cheat sheet
/// opens and closes again, and no mode was switched on the way.
///
/// The gesture lives in the bus's hold machine and the panel it asks for lives in
/// the caller that takes the outcome, so this walk drives the bus the way the
/// engine's `keyEvent` does: it arms the hold on the press, dispatches both edges,
/// opens the panel the outcome names and closes it with the panel's own `Escape`.
/// Nothing draws the panel yet, so what the walk asserts is the bus's whole part of
/// the gesture: the outcome, the open-close round trip and the untouched mode.
#[test]
fn test_scenario_12_long_press_shift_offers_the_cheat_sheet_and_keeps_the_mode() {
    let fixture = Fixture::default();
    let session = fixture.session_in(Situation::Idle);
    let host = RecordingHost::default();
    let mut bus = Dispatcher::new(KeyBindings::default());

    // The press at t=100 and its release at t=400: 300 ms held, past the 250 ms
    // threshold the hold machine answers long presses at.
    let down = KeyEvent {
        sym: KEY_SHIFT_L,
        state: SHIFT,
        is_release: false,
        time_ms: 100,
    };
    let up = KeyEvent {
        sym: KEY_SHIFT_L,
        state: SHIFT,
        is_release: true,
        time_ms: 400,
    };

    assert!(
        bus.arm_hold(&down, host.is_enabled),
        "the press arms the hold machine"
    );
    assert_eq!(
        bus.dispatch(&down, &SessionView::new(&session)),
        Consumed::Ignored,
        "the press itself reaches the application"
    );
    assert_eq!(
        bus.dispatch(&up, &SessionView::new(&session)),
        Consumed::Consumed,
        "the release that ends a long press is the plugin's"
    );
    assert_eq!(
        bus.take_hold_outcome(),
        Some(HoldOutcome::LongPress { held_ms: 300 }),
        "the release means: the cheat-sheet gesture, 300 ms held"
    );

    bus.open_overlay(Overlay::CheatSheet);
    assert_eq!(
        bus.overlay(),
        Some(Overlay::CheatSheet),
        "the panel is open"
    );
    assert_eq!(
        bus.dispatch(
            &KeyEvent {
                sym: KEY_ESCAPE,
                state: 0,
                is_release: false,
                time_ms: 500,
            },
            &SessionView::new(&session),
        ),
        Consumed::Consumed,
        "the panel's own Escape closes it"
    );
    assert_eq!(bus.overlay(), None, "the panel is gone again");

    assert_eq!(
        host.toggles, 0,
        "the gesture switched no mode: holding Shift is not the Chinese / English chord"
    );
    assert!(
        host.is_enabled,
        "the input method is as enabled as the walk found it"
    );
}

/// `SC-KEY-13`: the focus-loss path. The host reports a focus change without the
/// context going away; the plugin takes the composition back, the application
/// receives no text at all, and the window is told to hide with the focus-lost
/// reason.
///
/// The 90 ms the card asks for is a rendering budget the display-backed harness
/// measures; the engine's half of it is the hide posted here, at once, on the host
/// thread.
#[test]
fn test_scenario_13_focus_loss_delivers_no_text_and_hides_the_window() {
    let fixture = Fixture::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router(KeyBindings::default());

    type_nihao(&mut router, &mut host);
    router.reset(IC, &mut host);

    assert!(
        host.commits.is_empty(),
        "the application received no text at all: {:?}",
        host.commits
    );
    assert_eq!(
        host.hides.last(),
        Some(&HideReason::FocusLost),
        "the window begins dissipating: the hide was posted before any frame could follow"
    );
}

/// `SC-KEY-14`: the no-focus-steal invariant, over the walks above on one context.
///
/// The harness runs with no display server, so the focus window is a record the
/// host sets once; the routing layer has no call that could move it, and a walk
/// that gained one would show up as a changed record. Every stage of the walk
/// reads the record back.
#[test]
fn test_scenario_14_focus_window_id_never_changes() {
    let fixture = Fixture::default();
    let mut host = RecordingHost::default();
    let mut router = fixture.router(KeyBindings::default());

    // The commit walk.
    type_nihao(&mut router, &mut host);
    assert!(router.key_event(IC, &press(KEY_SPACE, 0), &mut host));
    assert_eq!(host.window_id, FOCUS_WINDOW_ID, "after the commit walk");

    // The cancel walk.
    type_nihao(&mut router, &mut host);
    assert!(router.key_event(IC, &press(KEY_ESCAPE, 0), &mut host));
    assert_eq!(host.window_id, FOCUS_WINDOW_ID, "after the cancel walk");

    // The paging and selection walks.
    type_nihao(&mut router, &mut host);
    assert!(router.key_event(IC, &press(KEY_EQUAL, 0), &mut host));
    assert!(router.key_event(IC, &press(KEY_1, 0), &mut host));
    assert_eq!(host.window_id, FOCUS_WINDOW_ID, "after the paging walk");

    // The focus-loss walk.
    type_nihao(&mut router, &mut host);
    router.reset(IC, &mut host);
    assert_eq!(host.window_id, FOCUS_WINDOW_ID, "after the focus change");

    // The mode walks, and the punctuation key.
    assert!(router.key_event(IC, &press(KEY_SPACE, SHIFT), &mut host));
    assert!(router.key_event(IC, &press(KEY_PERIOD, CTRL), &mut host));
    assert!(!router.key_event(IC, &press(KEY_COMMA, 0), &mut host));
    assert_eq!(host.window_id, FOCUS_WINDOW_ID, "after the mode walks");

    // The language chord, which flips the host's own input-method state and still
    // may not touch the focus.
    assert!(router.key_event(IC, &press(KEY_SPACE, CTRL), &mut host));
    assert_eq!(host.window_id, FOCUS_WINDOW_ID, "after the language chord");

    assert!(
        host.commits.iter().all(|text| !text.is_empty()),
        "every commit of the walk carried text: {:?}",
        host.commits
    );
}
