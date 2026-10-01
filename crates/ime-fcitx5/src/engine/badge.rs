//! The header's right-hand slot: what it shows, and who may borrow it.
//!
//! # Responsibility
//!
//! One slot of the candidate window's header — `StatusStrip::mode_label`, a field of the
//! frozen contract — carries several claimants: the mode name is the standing answer, a
//! multi-page indicator reports where in the candidate list the user is, and a first-run
//! key hint borrows the slot once per process. The window draws whatever the field holds
//! and derives nothing from it, so the ordering between the claimants is decided on the
//! engine side, and this module is the one place that knows it.
//!
//! # The ordering
//!
//! [`decide`] answers for one frame, and the order is the design's:
//!
//! 1. A frame with no candidates never leaves the standing answer. The window is hidden
//!    or about to show nothing, and the hint is only owed on a frame the user could look
//!    at — which is also why [`BadgeState`] is consumed by the frame that *shows* the
//!    hint, not by the frame that would have.
//! 2. A hint that has not been shown yet outranks the page indicator. It borrows the slot
//!    for exactly one frame — the first one that has candidates — and the grid itself
//!    already shows which page the user is on, so nothing is lost by waiting a frame.
//! 3. A candidate list of more than one page shows `current/total`, the numbers of
//!    [`PageState`].
//! 4. Everything else is the mode name.
//!
//! # Why user input cannot reach the badge
//!
//! [`render`] and [`resolve`] build the text out of key names, page numbers and the mode
//! label, and nothing else. The mode label arrives as a `&'static str` — the engine's own
//! constants and the configuration's compiled-in layout names — and every other input is
//! a bitflag table, three integers and a boolean. There is no parameter a candidate's
//! text, the preedit or a committed string could travel through, which makes the
//! prohibition structural rather than a matter of discipline.
//!
//! # Boundary
//!
//! Plain Rust and pure: no host object, no file, no clock, no global state. The one bit
//! of state the resolution consumes — whether the hint is still owed — is owned by the
//! caller, the router, which keeps it beside its other process state, and reaches this
//! module as a parameter.

use ime_config::keymap::{FlipSet, HighlightSet, KeyBindings};
use ime_types::PageState;

/// What the header's right-hand slot shows.
///
/// One slot, several claimants: the mode name is the standing answer, and a hint or a
/// page indicator may borrow it. The value says *which* claimant won the slot;
/// [`render`] turns it into the text the strip carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderSlot {
    /// The mode name: `中`, `英`, or the layout the configuration names.
    Mode,
    /// The first-run key hint, shown once per process on the first frame that has
    /// candidates.
    FirstRunHint,
    /// Where in the candidate list the user is: page `current` of `total`, both one-based
    /// as [`PageState`] numbers them.
    Page {
        /// The page on show, one-based.
        current: u8,
        /// How many pages the candidate list has.
        total: u8,
    },
}

/// The process-local state the badge resolution consumes.
///
/// The one bit the header's slot needs between frames: whether the first-run hint is
/// still owed. It is deliberately not persisted — the design's persistent variant is a
/// configuration key of its own, out of scope here — so a process that restarts owes the
/// hint again, and the bit lives beside the router's other process state rather than on
/// disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadgeState {
    /// Whether the first-run hint has not been shown yet.
    first_hint_pending: bool,
    /// Whether the candidate overflow has been recorded for the current session.
    ///
    /// The diagnostic names a state that persists while the list stays past the
    /// display limit, so it is recorded once and not once per frame: a session that
    /// types into an overflowing list would otherwise write the same line on every
    /// keystroke.
    overflow_recorded: bool,
}

impl Default for BadgeState {
    /// A process that has shown nothing yet.
    fn default() -> Self {
        Self::new()
    }
}

impl BadgeState {
    /// The state a freshly loaded addon starts with: the hint is still owed.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn new() -> Self {
        Self {
            first_hint_pending: true,
            overflow_recorded: false,
        }
    }

    /// Records an observation of the candidate overflow, answering whether it is new.
    ///
    /// The answer is what the caller emits the diagnostic on: `true` for the
    /// observation that first saw the overflow, `false` for every one after it.
    ///
    /// # Panics
    ///
    /// Never.
    fn note_overflow(&mut self, observed: bool) -> bool {
        if observed && !self.overflow_recorded {
            self.overflow_recorded = true;
            return true;
        }
        false
    }

    /// Whether the hint is due on a frame that shows `has_candidates` candidates.
    ///
    /// A frame without candidates never falls due: the window is invisible when nothing
    /// composes, and a showing nobody could see would spend the one the process has.
    ///
    /// # Panics
    ///
    /// Never.
    const fn hint_due(&self, has_candidates: bool) -> bool {
        has_candidates && self.first_hint_pending
    }

    /// Records the hint as shown, so no later frame claims the slot for it.
    ///
    /// # Panics
    ///
    /// Never.
    fn consume_hint(&mut self) {
        self.first_hint_pending = false;
    }
}

/// Decides what the slot shows for one frame.
///
/// The order is the module documentation's: a frame with candidates and an unshown hint
/// shows the hint, a multi-page list shows the page indicator, and everything else shows
/// the mode name.
///
/// # Arguments
///
/// * `page` — the paging state of the frame the window is about to draw.
/// * `has_candidates` — whether the frame carries any candidate at all.
/// * `state` — the process-local badge state, read for the hint that has not been shown.
///
/// # Returns
///
/// The claimant that wins the slot; [`render`] turns it into text.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never: the body is two comparisons over the values the caller passed in.
pub fn decide(page: PageState, has_candidates: bool, state: &BadgeState) -> HeaderSlot {
    if state.hint_due(has_candidates) {
        return HeaderSlot::FirstRunHint;
    }
    if has_candidates && page.total > 1 {
        return HeaderSlot::Page {
            current: page.current,
            total: page.total,
        };
    }
    HeaderSlot::Mode
}

/// Builds the text a slot shows.
///
/// The mode slot shows `mode` verbatim; the others are built from key names and page
/// numbers, which is what keeps user input out of the strip by construction.
///
/// # Arguments
///
/// * `slot` — the claimant [`decide`] answered.
/// * `mode` — the standing mode label the engine computed for this frame.
/// * `keys` — the bindings in force, which the hint names.
///
/// # Returns
///
/// The text `StatusStrip::mode_label` carries for this frame.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never: the body formats literals and integers, with no indexing and no arithmetic.
pub fn render(slot: HeaderSlot, mode: &'static str, keys: &KeyBindings, overflow: bool) -> String {
    match slot {
        HeaderSlot::Mode => String::from(mode),
        HeaderSlot::FirstRunHint => hint_text(keys),
        // The `+` is what says the list went past the display limit: the pages are
        // capped at five, so a fifth page that overflows would otherwise read as the
        // last one and the user would never know the list went on.
        HeaderSlot::Page { current, total } if overflow => format!("{current}/{total}+"),
        HeaderSlot::Page { current, total } => format!("{current}/{total}"),
    }
}

/// Resolves the slot for one frame about to be posted, and consumes the hint if it shows.
///
/// The one entry point the frame-production path calls: [`decide`] picks the claimant,
/// [`render`] builds the text, and the frame that showed the hint marks it as shown, so
/// the hint is displayed exactly once per process and only on a frame that has
/// candidates.
///
/// # Arguments
///
/// * `mode` — the standing mode label, as [`render`] takes it.
/// * `keys` — the bindings in force, as [`render`] takes it.
/// * `page` — the paging state of the frame.
/// * `has_candidates` — whether the frame carries any candidate.
/// * `state` — the process-local badge state, updated when the hint shows.
///
/// # Returns
///
/// The text `StatusStrip::mode_label` carries for this frame.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never.
pub fn resolve(
    mode: &'static str,
    keys: &KeyBindings,
    page: PageState,
    has_candidates: bool,
    overflow: bool,
    state: &mut BadgeState,
) -> (String, bool) {
    let slot = decide(page, has_candidates, state);
    if slot == HeaderSlot::FirstRunHint {
        state.consume_hint();
    }
    let overflow_started = state.note_overflow(overflow);
    (render(slot, mode, keys, overflow), overflow_started)
}

/// The code recorded when the candidate list outgrows the five-page display limit.
///
/// Stable, like every code in the project: the diagnostic tables and the tests match
/// on it. It is recorded once per overflowing session rather than once per frame --
/// see [`BadgeState::note_overflow`].
pub const UI_CANDIDATE_OVERFLOW_CODE: &str = "ui/candidate/overflow";

/// `↑`, as the hint spells the `Up` key.
const ARROW_UP: &str = "↑";

/// `↓`, as the hint spells the `Down` key.
const ARROW_DOWN: &str = "↓";

/// What the hint calls the action of moving onto the next candidate.
const HINT_HIGHLIGHT_LABEL: &str = "换词";

/// What the hint calls the action of paging.
const HINT_PAGE_LABEL: &str = "翻页";

/// The hint's segment for the panel chord.
///
/// The chord is the engine's own: it is answered by the panel layer ahead of any
/// configuration, no document can unbind it, and a fixed name for a fixed chord cannot
/// go stale the way a hand-written binding name would.
const HINT_PANEL_SEGMENT: &str = "Ctrl+Shift+/ 全部";

/// Between the hint's segments.
const SEPARATOR: &str = " · ";

/// The highlight keys that move onto the next candidate, most preferred first.
///
/// `Tab` leads because the design's hint names it: with the shipped bindings the segment
/// reads `Tab 换词`. A user who moved the highlight onto another key sees that key
/// instead, so the hint cannot lie about the bindings the way a hand-written text would.
const HIGHLIGHT_FORWARD: &[(HighlightSet, &str)] = &[
    (HighlightSet::TAB, "Tab"),
    (HighlightSet::DOWN, "↓"),
    (HighlightSet::RIGHT, "→"),
];

/// The page keys that go back one page, most preferred first.
///
/// The arrows lead so that the shipped bindings read `↑↓`, the design's example.
const PAGE_PREV: &[(FlipSet, &str)] = &[
    (FlipSet::UP, ARROW_UP),
    (FlipSet::MINUS, "-"),
    (FlipSet::PAGE_UP, "PgUp"),
];

/// The page keys that go forward one page, most preferred first.
const PAGE_NEXT: &[(FlipSet, &str)] = &[
    (FlipSet::DOWN, ARROW_DOWN),
    (FlipSet::EQUAL, "="),
    (FlipSet::PAGE_DOWN, "PgDn"),
];

/// The name the hint gives the first key of `table` the bindings in force name.
///
/// The tables are in preference order, so the first bound entry is the one the hint
/// spells out. A key the configuration left unbound is never named: a hint that claims a
/// key does nothing the user presses would be worse than none.
///
/// # Panics
///
/// Never: the walk reads a slice of at most three pairs.
fn bound_name(
    table: &'static [(HighlightSet, &'static str)],
    bound: HighlightSet,
) -> Option<&'static str> {
    for (bit, name) in table {
        if bound.contains(*bit) {
            return Some(*name);
        }
    }
    None
}

/// The key the hint names for moving onto the next candidate, or `None` when the
/// configuration left the highlight without a forward key.
///
/// # Panics
///
/// Never.
fn highlight_forward_name(keys: &KeyBindings) -> Option<&'static str> {
    bound_name(HIGHLIGHT_FORWARD, keys.highlight_keys)
}

/// The name of the first page key of `table` the bindings name, or `None`.
///
/// The page tables read [`FlipSet`] where the highlight table reads [`HighlightSet`];
/// the walk is the same, and keeping one walk per flag type is what lets the two tables
/// stay plain data.
///
/// # Panics
///
/// Never: the walk reads a slice of at most three pairs.
fn page_name(table: &'static [(FlipSet, &'static str)], bound: FlipSet) -> Option<&'static str> {
    for (bit, name) in table {
        if bound.contains(*bit) {
            return Some(*name);
        }
    }
    None
}

/// The hint's segment for the highlight, or `None` when nothing is bound to it.
///
/// # Panics
///
/// Never.
fn highlight_segment(keys: &KeyBindings) -> Option<String> {
    highlight_forward_name(keys).map(|name| format!("{name} {HINT_HIGHLIGHT_LABEL}"))
}

/// The hint's segment for paging, or `None` when nothing is bound to it.
///
/// Both directions are named when both are bound, which is the design's `↑↓ 翻页`. Two
/// arrow glyphs run together — `↑↓` — and any other pair is joined with a slash, which
/// is how a chord-shaped pair (`-/=`, `PgUp/PgDn`) stays readable. One direction alone
/// names that one key.
///
/// # Panics
///
/// Never.
fn page_segment(keys: &KeyBindings) -> Option<String> {
    let prev = page_name(PAGE_PREV, keys.flip_keys);
    let next = page_name(PAGE_NEXT, keys.flip_keys);
    let names = match (prev, next) {
        (Some(prev), Some(next)) if prev == ARROW_UP && next == ARROW_DOWN => {
            format!("{prev}{next}")
        }
        (Some(prev), Some(next)) => format!("{prev}/{next}"),
        (Some(name), None) | (None, Some(name)) => String::from(name),
        (None, None) => return None,
    };
    Some(format!("{names} {HINT_PAGE_LABEL}"))
}

/// The whole hint text, built from the bindings in force.
///
/// The panel chord is always named, because it is the engine's own; the other two
/// segments appear only when a key is actually bound to the action they describe.
///
/// # Panics
///
/// Never.
fn hint_text(keys: &KeyBindings) -> String {
    let mut text = String::new();
    let segments = [
        highlight_segment(keys),
        page_segment(keys),
        Some(String::from(HINT_PANEL_SEGMENT)),
    ];
    for segment in segments.into_iter().flatten() {
        if !text.is_empty() {
            text.push_str(SEPARATOR);
        }
        text.push_str(&segment);
    }
    text
}

#[cfg(test)]
mod tests {
    //! The ordering and the text, one case per claimant, plus the vocabulary walk that
    //! pins the structural guarantee about user input.

    use super::*;

    /// The standing label a router with the shipped configuration shows in Chinese mode.
    const MODE: &str = "全拼";

    /// A state whose hint has not been shown: a freshly loaded addon.
    fn pending() -> BadgeState {
        BadgeState::new()
    }

    /// A state whose hint was shown once.
    fn consumed() -> BadgeState {
        let mut state = BadgeState::new();
        state.consume_hint();
        state
    }

    /// The paging state of a frame that shows page `current` of `total`.
    fn page(current: u8, total: u8) -> PageState {
        PageState {
            current,
            total,
            page_size: 5,
        }
    }

    #[test]
    fn test_decide_first_composition_shows_the_hint() {
        let state = pending();
        assert_eq!(
            decide(page(1, 1), true, &state),
            HeaderSlot::FirstRunHint,
            "the first frame with candidates borrows the slot for the hint"
        );
    }

    #[test]
    fn test_decide_second_composition_shows_the_mode_name() {
        // The hint was shown once, so the standing answer returns for every frame after
        // it, whatever the composition.
        let state = consumed();
        assert_eq!(decide(page(1, 1), true, &state), HeaderSlot::Mode);
    }

    #[test]
    fn test_decide_multi_page_frame_shows_the_page_indicator() {
        let state = consumed();
        assert_eq!(
            decide(page(2, 3), true, &state),
            HeaderSlot::Page {
                current: 2,
                total: 3
            },
            "a list of more than one page is what the indicator reports"
        );
    }

    #[test]
    fn test_decide_frame_without_candidates_shows_the_mode_name_while_the_hint_is_pending() {
        // The window is invisible when nothing composes, and an empty frame is not a
        // frame to spend the one showing on: the hint stays owed for a frame the user
        // can look at.
        let state = pending();
        assert_eq!(decide(page(1, 1), false, &state), HeaderSlot::Mode);
        assert_eq!(decide(page(0, 0), false, &state), HeaderSlot::Mode);
    }

    #[test]
    fn test_decide_page_indicator_yields_to_a_hint_not_yet_shown() {
        // Both claimants apply on the very first frame, and the hint wins: it shows
        // once, and the page indicator takes the slot from the next frame on.
        let mut state = pending();
        assert_eq!(
            decide(page(1, 3), true, &state),
            HeaderSlot::FirstRunHint,
            "the hint's one showing outranks the indicator"
        );
        state.consume_hint();
        assert_eq!(
            decide(page(1, 3), true, &state),
            HeaderSlot::Page {
                current: 1,
                total: 3
            },
            "once the hint was shown, the page indicator wins the slot"
        );
    }

    #[test]
    fn test_resolve_consumes_the_hint_after_one_showing() {
        let keys = KeyBindings::default();
        let mut state = pending();
        let (label, _) = resolve(MODE, &keys, page(1, 1), true, false, &mut state);
        assert_eq!(
            label, "Tab 换词 · ↑↓ 翻页 · Ctrl+Shift+/ 全部",
            "the shipped bindings spell the design's example text"
        );
        let (label, _) = resolve(MODE, &keys, page(1, 1), true, false, &mut state);
        assert_eq!(label, MODE, "the hint was shown, so the mode name returns");
    }

    #[test]
    fn test_render_hint_names_the_bindings_actually_in_force() {
        // Every segment is read out of the bindings the router itself branches on, so a
        // moved or unbound key cannot leave the hint naming a key that does nothing.
        assert_eq!(
            render(
                HeaderSlot::FirstRunHint,
                MODE,
                &KeyBindings::default(),
                false
            ),
            "Tab 换词 · ↑↓ 翻页 · Ctrl+Shift+/ 全部"
        );
        let paged_by_page_keys = KeyBindings {
            flip_keys: FlipSet::PAGE_UP | FlipSet::PAGE_DOWN,
            ..KeyBindings::default()
        };
        assert_eq!(
            render(HeaderSlot::FirstRunHint, MODE, &paged_by_page_keys, false),
            "Tab 换词 · PgUp/PgDn 翻页 · Ctrl+Shift+/ 全部"
        );
        let minus_only = KeyBindings {
            flip_keys: FlipSet::MINUS,
            ..KeyBindings::default()
        };
        assert_eq!(
            render(HeaderSlot::FirstRunHint, MODE, &minus_only, false),
            "Tab 换词 · - 翻页 · Ctrl+Shift+/ 全部"
        );
        let arrow_highlight = KeyBindings {
            highlight_keys: HighlightSet::DOWN,
            ..KeyBindings::default()
        };
        assert_eq!(
            render(HeaderSlot::FirstRunHint, MODE, &arrow_highlight, false),
            "↓ 换词 · ↑↓ 翻页 · Ctrl+Shift+/ 全部"
        );
        let nothing_bound = KeyBindings {
            highlight_keys: HighlightSet::empty(),
            flip_keys: FlipSet::empty(),
            ..KeyBindings::default()
        };
        assert_eq!(
            render(HeaderSlot::FirstRunHint, MODE, &nothing_bound, false),
            "Ctrl+Shift+/ 全部",
            "an unbound action is dropped rather than named with a key that does not move it"
        );
    }

    #[test]
    fn test_render_hint_is_built_from_key_names_and_numbers_only() {
        // The structural guarantee, walked once: every character the badge builds comes
        // from the key names, the labels and the separators this module writes out, so
        // no candidate's text, no preedit and no committed string can appear here. The
        // mode label is not part of the claim — it is the standing answer the engine
        // produced, shown verbatim rather than built.
        let bindings = [
            KeyBindings::default(),
            KeyBindings {
                flip_keys: FlipSet::PAGE_UP | FlipSet::PAGE_DOWN,
                ..KeyBindings::default()
            },
            KeyBindings {
                highlight_keys: HighlightSet::DOWN | HighlightSet::RIGHT,
                flip_keys: FlipSet::MINUS | FlipSet::EQUAL,
                ..KeyBindings::default()
            },
            KeyBindings {
                highlight_keys: HighlightSet::empty(),
                flip_keys: FlipSet::empty(),
                ..KeyBindings::default()
            },
        ];
        for keys in &bindings {
            let hint = render(HeaderSlot::FirstRunHint, MODE, keys, false);
            let page_text = render(
                HeaderSlot::Page {
                    current: 2,
                    total: 3,
                },
                MODE,
                keys,
                false,
            );
            for text in [hint, page_text] {
                assert!(
                    text.chars().all(is_vocabulary_char),
                    "{text:?} leaves the badge's vocabulary"
                );
            }
        }
    }

    /// Whether `c` is one of the characters the badge's own vocabulary is written with.
    fn is_vocabulary_char(c: char) -> bool {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                '+' | '/'
                    | '-'
                    | '='
                    | '·'
                    | '↑'
                    | '↓'
                    | ' '
                    | '换'
                    | '词'
                    | '翻'
                    | '页'
                    | '全'
                    | '部'
            )
    }

    /// The shipped bindings, for the tests that resolve a slot against them.
    fn keys() -> KeyBindings {
        KeyBindings::default()
    }

    #[test]
    fn test_render_page_indicator_formats_current_over_total() {
        assert_eq!(
            render(
                HeaderSlot::Page {
                    current: 1,
                    total: 3
                },
                MODE,
                &KeyBindings::default(),
                false
            ),
            "1/3"
        );
    }

    #[test]
    fn test_render_overflow_appends_the_plus_to_the_indicator() {
        // The `+` is the whole spelling difference: a fifth page of a longer list must
        // not read as the last one.
        assert_eq!(
            render(
                HeaderSlot::Page {
                    current: 5,
                    total: 5
                },
                MODE,
                &KeyBindings::default(),
                true
            ),
            "5/5+"
        );
    }

    #[test]
    fn test_resolve_reports_the_overflow_once_and_only_once() {
        let mut state = BadgeState::new();
        let (_, first) = resolve(MODE, &keys(), page(5, 5), true, true, &mut state);
        assert!(
            first,
            "the first observation is the one the caller emits on"
        );
        let (_, second) = resolve(MODE, &keys(), page(5, 5), true, true, &mut state);
        assert!(
            !second,
            "a state that persists is not re-reported per frame"
        );
        let (_, cleared) = resolve(MODE, &keys(), page(1, 1), true, false, &mut state);
        assert!(!cleared, "a frame inside the limit records nothing");
    }

    #[test]
    fn test_render_mode_shows_the_standing_label_verbatim() {
        assert_eq!(
            render(HeaderSlot::Mode, "小鹤", &KeyBindings::default(), false),
            "小鹤"
        );
    }
}
