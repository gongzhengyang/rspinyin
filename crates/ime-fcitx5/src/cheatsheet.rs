//! The cheat-sheet panel: the key reference built from the bindings in force, and the
//! host-side half of opening and closing it.
//!
//! # Responsibility
//!
//! Two halves live here because they are one feature and share one rule.
//!
//! The first half is [`build`]: the panel's content, generated from the live
//! [`KeyBindings`] and from the engine's chord table rather than written out. A
//! hand-copied panel would keep showing `-` / `=` after the user moved paging onto
//! `page_up` / `page_down` in `[keys] flip_keys`, and a discovery aid that lies is worse
//! than none — the whole point of the panel is that every row is a row the routing
//! table actually answers. The three groups are the design's:
//!
//! * 组字 — the configurable bindings: `highlight_keys`, `flip_keys`, `digit_zero` and
//!   `enter_commit_raw`, with key names in their configuration spelling.
//! * 模式 — the engine's mode chords, read from the crate's chord table, plus the two
//!   fixed panel chords.
//! * 编辑 — the composing keymap's own rows, which no configuration can unbind.
//!
//! The second half is [`apply_overlay_outcome`]: the seam the caller runs after the key
//! walk. The bus ([`Dispatcher`]) answers *whether* a key is the plugin's and records
//! what a chord or a modifier release asked for; it holds no host object, so turning
//! that into commands on the candidate window is the caller's half. Three outcomes are
//! acted on here:
//!
//! * A chord asked for a panel. The cheat sheet is built and posted; the other two
//!   panels keep their existing behaviour — a diagnostic, nothing drawn — because they
//!   are later phases.
//! * An open panel was closed. The `Escape` the walk answers closes it in the bus, and
//!   this half posts the close to the window and records one
//!   [`UI_CHEATSHEET_DISMISSED_CODE`] — one per dismissal, and only for the cheat
//!   sheet, because the held-`Shift` release that ends the same panel is part of
//!   typing and would bury the channel.
//! * A held modifier was released as a long press. The held `Shift` is the gesture the
//!   cheat sheet answers to: the release writes the panel into the overlay channel and
//!   clears it again, with no diagnostic.
//!
//! # Boundary
//!
//! Pure Rust over the crate's existing boundaries. [`build`] reads two configuration
//! values and the chord table — no host, no file, no clock, no global state — which is
//! what makes the panel's content deterministic and testable. [`apply_overlay_outcome`]
//! touches only the [`Host`] boundary the routing layer already speaks and the
//! diagnostic channel, through a sink parameter so a test can read the lines back
//! without the process-wide throttle.

use ime_config::keymap::{FlipSet, HighlightSet, KeyBindings};
use ime_types::KeyAction;
use ime_types::{OverlayEntry, OverlayFrame, OverlayKind, OverlaySection, UiCommand};

use crate::engine::host::Host;
use crate::engine::{
    CHORDS, CTRL, DigitZero, Dispatcher, HoldOutcome, KEY_E, KEY_PERIOD, KEY_SPACE, Overlay,
    RoutingConfig, SHIFT,
};
use crate::ffi::emit_diagnostic;

/// Recorded when the user closes the cheat sheet with the `Escape` the panel owns.
///
/// Stable, like every other code in the project: diagnostics and tests match on it.
/// The constant lives beside the walk-outcome seam that emits it, which is where this
/// crate's codes live (`router`'s `STALE_IC_CODE`, `modifier`'s mask-mismatch code,
/// `panel`'s not-implemented code). Exactly one line per dismissal: the close is a
/// gesture of its own, and the held-`Shift` release that ends the same panel is
/// deliberately not recorded, because a hold is part of typing and the close it
/// triggers would bury the channel under the user's own keystrokes.
pub const UI_CHEATSHEET_DISMISSED_CODE: &str = "ui/cheatsheet-dismissed";

/// The title of a panel built for a configuration that shows no scheme hint.
const PLAIN_TITLE: &str = "按键速查";

/// The host-side values the overlay path needs, grouped so that a caller hands one
/// value.
///
/// The routing layer already holds all three beside each other — the configuration in
/// force, the context a key arrived for, and the boundary the window is posted through —
/// and the panel path needs exactly those, so they travel as one argument rather than as
/// three.
pub struct OverlayStage<'a> {
    /// The configuration in force: the bindings the cheat sheet is built from and the
    /// scheme the title names.
    pub config: &'a RoutingConfig,
    /// The host's identity for the input context the panel opens in.
    pub ic: u64,
    /// The boundary the panel's commands travel on — the same `post_ui` path the
    /// candidate frames take.
    pub host: &'a mut dyn Host,
}

impl OverlayStage<'_> {
    /// Posts one command to the candidate window for this context.
    fn post(&mut self, command: UiCommand) {
        self.host.post_ui(self.ic, command);
    }

    /// The scheme the panel's title names, or the empty string when the hint is off.
    fn scheme(&self) -> &str {
        self.config.scheme_hint.unwrap_or("")
    }
}

/// Builds the cheat sheet from the bindings actually in force.
///
/// # Arguments
///
/// * `keys` — the `[keys]` settings the routing table branches on. Every row of the
///   组字 group is generated from them, so a panel drawn from this frame and a key the
///   router answers cannot disagree.
/// * `scheme` — the active scheme's name for the title, or the empty string for no
///   scheme in the title.
///
/// # Returns
///
/// The frame the UI thread draws verbatim: kind [`OverlayKind::CheatSheet`], no
/// selection and an empty query, which is the cheat sheet's own shape — it has no
/// search field and nothing to highlight.
///
/// A row a configuration does not bind is left out rather than drawn with a placeholder
/// key name: a row with no key on it tells the user nothing, and the group the row
/// belonged to keeps its remaining rows.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never: the walk reads two bit sets and a fixed chord table and builds owned
/// `String`s, with no indexing and no arithmetic that can overflow.
///
/// # Examples
///
/// ```
/// use ime_config::keymap::KeyBindings;
/// use rspinyin::cheatsheet::build;
///
/// let frame = build(&KeyBindings::default(), "全拼");
/// assert_eq!(frame.title, "按键速查 · 全拼");
/// assert_eq!(frame.sections.len(), 3, "the three groups of the design");
/// ```
pub fn build(keys: &KeyBindings, scheme: &str) -> OverlayFrame {
    OverlayFrame {
        kind: OverlayKind::CheatSheet,
        title: title(scheme),
        sections: vec![composition_section(keys), mode_section(), edit_section()],
        // The cheat sheet highlights nothing and searches nothing; the fields exist
        // for the panels that come after this one.
        selected: None,
        query: String::new(),
    }
}

/// Consumes what the key walk left about panels and holds, and performs it.
///
/// The caller runs this after every key it dispatched — with the overlay state the bus
/// held *before* that key, captured from [`Dispatcher::overlay`] — whether or not the
/// walk kept the key. Three outcomes are acted on, in the order the walk can leave them:
/// a chord's panel request, an `Escape` dismissal, and a long-press release. A walk
/// that left none of the three costs one call and writes nothing.
///
/// # Arguments
///
/// * `dispatcher` — the bus the key was walked through.
/// * `overlay_before` — what [`Dispatcher::overlay`] answered before the key, or `None`
///   when no panel was open. The bus closes a panel inside the walk and records no
///   event for it, so the caller's before-snapshot is what makes a dismissal
///   observable — and observable exactly once, because the snapshot is taken again
///   before the next key.
/// * `stage` — the host side of the path: configuration, context and boundary.
///
/// # Returns
///
/// Whether anything other than a diagnostic reached the host, the same answer the
/// router's `ui_event` gives: the open, close and long-press paths post commands and
/// answer `true`, the two not-yet-implemented panels report and answer `false`.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never.
pub fn apply_overlay_outcome(
    dispatcher: &mut Dispatcher,
    overlay_before: Option<Overlay>,
    stage: &mut OverlayStage<'_>,
) -> bool {
    // The sink is a parameter of the private body so the tests can read the lines back;
    // the production path pays one monomorphised indirection for it.
    apply_overlay_outcome_with(dispatcher, overlay_before, stage, emit_diagnostic)
}

/// The body of [`apply_overlay_outcome`], over a caller-supplied diagnostic sink.
fn apply_overlay_outcome_with(
    dispatcher: &mut Dispatcher,
    overlay_before: Option<Overlay>,
    stage: &mut OverlayStage<'_>,
    mut report: impl FnMut(&str),
) -> bool {
    // The chord path first: a chord is answered ahead of the walk, so its request is
    // the newest thing the bus holds, and it replaces any panel the walk might have
    // closed.
    if let Some(requested) = dispatcher.take_overlay_request() {
        return open_requested_panel(dispatcher, requested, stage, &mut report);
    }
    // The Escape path: the walk closed a panel that was open before this key.
    if overlay_before.is_some() && dispatcher.overlay().is_none() {
        return dismiss_panel(overlay_before, stage, &mut report);
    }
    // The long-press path: a modifier release the hold machine answered.
    release_into_panel(dispatcher, stage)
}

/// Opens the panel a chord asked for, or reports that nothing draws it.
fn open_requested_panel(
    dispatcher: &mut Dispatcher,
    requested: Overlay,
    stage: &mut OverlayStage<'_>,
    report: &mut impl FnMut(&str),
) -> bool {
    match requested {
        Overlay::CheatSheet => {
            let frame = Box::new(build(&stage.config.keys, stage.scheme()));
            // The bus opens the panel as well as the window: while it is open the
            // overlay layer owns the `Escape` that closes it, which is the walk's half
            // of the mode.
            dispatcher.open_overlay(Overlay::CheatSheet);
            stage.post(UiCommand::Overlay(Some(frame)));
            true
        }
        // The other two panels are later phases. Today's behaviour is kept exactly:
        // the chord is answered and the request is reported once, on the diagnostic
        // channel, and nothing is drawn for it.
        other => {
            report(&other.not_implemented_code());
            false
        }
    }
}

/// Posts the close of a panel the walk just dismissed, and records the cheat sheet's.
fn dismiss_panel(
    opened: Option<Overlay>,
    stage: &mut OverlayStage<'_>,
    report: &mut impl FnMut(&str),
) -> bool {
    // One line, and only for the cheat sheet: the code names the panel whose
    // dismissals are worth counting, and a panel the other two chords asked for was
    // never drawn in the first place.
    if opened == Some(Overlay::CheatSheet) {
        report(UI_CHEATSHEET_DISMISSED_CODE);
    }
    stage.post(UiCommand::Overlay(None));
    true
}

/// Writes the panel a long-press release asks for, and clears it again.
///
/// The held `Shift` is the gesture the cheat sheet answers to, and the hold machine
/// answers it on the release — the one moment the hold is known to have been a hold
/// rather than a typed key. The release therefore writes the panel into the overlay
/// channel and clears it in the same stroke, and records nothing: a hold is part of
/// typing, and the close it leaves behind is the gesture's own end rather than a
/// dismissal the user asked for.
fn release_into_panel(dispatcher: &mut Dispatcher, stage: &mut OverlayStage<'_>) -> bool {
    // Taken, not peeked: the outcome is the walk's answer for one release and is acted
    // on once. A `Restore` outcome is the held-modifier mode switch, whose wiring the
    // hold path owns — nothing consumes it today, so taking it here changes no
    // observable behaviour.
    if !matches!(
        dispatcher.take_hold_outcome(),
        Some(HoldOutcome::LongPress { .. })
    ) {
        return false;
    }
    let frame = Box::new(build(&stage.config.keys, stage.scheme()));
    dispatcher.open_overlay(Overlay::CheatSheet);
    stage.post(UiCommand::Overlay(Some(frame)));
    // Cleared in the same stroke, so the panel does not outlive the gesture that asked
    // for it and no key of the user's typing is spent on a mode they did not enter.
    dispatcher.close_overlay();
    stage.post(UiCommand::Overlay(None));
    true
}

/// The panel title, with the scheme when the configuration shows one.
fn title(scheme: &str) -> String {
    if scheme.is_empty() {
        return String::from(PLAIN_TITLE);
    }
    format!("{PLAIN_TITLE} · {scheme}")
}

/// The 组字 group: the bindings the configuration actually makes.
fn composition_section(keys: &KeyBindings) -> OverlaySection {
    let mut entries = Vec::new();
    if let Some(row) = highlight_row(keys.highlight_keys) {
        entries.push(row);
    }
    if let Some(row) = flip_row(keys.flip_keys) {
        entries.push(row);
    }
    // Passthrough is the 0 key being the application's digit; a row for it would name
    // a key the plugin does not answer.
    if keys.digit_zero == DigitZero::Flip {
        entries.push(OverlayEntry {
            keys: String::from("0"),
            label: String::from("数字 0 翻到下一页"),
        });
    }
    entries.push(OverlayEntry {
        keys: String::from("Enter"),
        label: String::from(if keys.enter_commit_raw {
            "上屏原始输入"
        } else {
            "上屏高亮候选"
        }),
    });
    OverlaySection {
        title: String::from("组字"),
        entries,
    }
}

/// The 模式 group: the engine's mode chords, plus the two fixed panel chords.
fn mode_section() -> OverlaySection {
    let mut entries: Vec<OverlayEntry> = CHORDS
        .iter()
        .filter_map(|chord| {
            Some(OverlayEntry {
                keys: chord_name(chord.sym, chord.mask)?,
                label: chord_label(chord.action)?.to_string(),
            })
        })
        .collect();
    // The two chords that ask for a panel. Fixed rather than generated, because the
    // chords themselves are fixed by the design: they are matched in
    // `engine::context::panel` ahead of the walk and no configuration reaches them. A
    // chord added or removed there has to be changed in this list too, which the
    // chord-table test beside [`build`] pins.
    entries.push(OverlayEntry {
        keys: String::from("Ctrl+Shift+/"),
        label: String::from("命令面板"),
    });
    entries.push(OverlayEntry {
        keys: String::from("Ctrl+Shift+P"),
        label: String::from("诊断面板"),
    });
    OverlaySection {
        title: String::from("模式"),
        entries,
    }
}

/// The 编辑 group: the composing keymap's own rows, which no configuration unbinds.
fn edit_section() -> OverlaySection {
    OverlaySection {
        title: String::from("编辑"),
        entries: vec![
            OverlayEntry {
                keys: String::from("Esc"),
                label: String::from("取消输入"),
            },
            OverlayEntry {
                keys: String::from("BackSpace"),
                label: String::from("删除音节"),
            },
            OverlayEntry {
                keys: String::from("Left / Right"),
                label: String::from("移动光标"),
            },
            OverlayEntry {
                keys: String::from("Space"),
                label: String::from("上屏"),
            },
        ],
    }
}

/// The highlight row of the 组字 group, or `None` when no highlight key is bound.
///
/// One row for the whole list, in the order the configuration's whitelist numbers them,
/// because the row's meaning is one — move the candidate highlight — and the keys that
/// mean it are a list the user chose.
fn highlight_row(highlight_keys: HighlightSet) -> Option<OverlayEntry> {
    const ROWS: &[(HighlightSet, &str)] = &[
        (HighlightSet::TAB, "tab"),
        (HighlightSet::SHIFT_TAB, "shift_tab"),
        (HighlightSet::UP, "up"),
        (HighlightSet::DOWN, "down"),
        (HighlightSet::LEFT, "left"),
        (HighlightSet::RIGHT, "right"),
    ];
    row_of(ROWS, |flag| highlight_keys.contains(flag), "移动候选高亮")
}

/// The page row of the 组字 group, or `None` when no page key is bound.
fn flip_row(flip_keys: FlipSet) -> Option<OverlayEntry> {
    const ROWS: &[(FlipSet, &str)] = &[
        (FlipSet::MINUS, "minus"),
        (FlipSet::EQUAL, "equal"),
        (FlipSet::UP, "up"),
        (FlipSet::DOWN, "down"),
        (FlipSet::PAGE_UP, "page_up"),
        (FlipSet::PAGE_DOWN, "page_down"),
    ];
    row_of(ROWS, |flag| flip_keys.contains(flag), "候选列表翻页")
}

/// One binding-list row: the bound names joined in whitelist order, or `None` when the
/// list binds nothing.
///
/// The membership test is a parameter rather than an equality because a binding list is
/// a *set*: a default that binds four keys carries all four bits, and comparing a set
/// against each entry's single flag would match nothing. Passing the list's own
/// `contains` is what keeps the two lists' rows one piece of code apart from their two
/// flag types.
fn row_of<F: Copy>(
    rows: &[(F, &str)],
    is_bound: impl Fn(F) -> bool,
    label: &str,
) -> Option<OverlayEntry> {
    let names: Vec<&str> = rows
        .iter()
        .filter(|(flag, _)| is_bound(*flag))
        .map(|(_, name)| *name)
        .collect();
    if names.is_empty() {
        return None;
    }
    Some(OverlayEntry {
        keys: names.join(" / "),
        label: String::from(label),
    })
}

/// The name a chord is displayed under, or `None` for a keysym this table cannot name.
fn chord_name(sym: u32, mask: u32) -> Option<String> {
    let key = match sym {
        KEY_SPACE => "Space",
        KEY_PERIOD => ".",
        KEY_E => "E",
        _ => return None,
    };
    let mut name = String::new();
    if mask & CTRL != 0 {
        name.push_str("Ctrl+");
    }
    if mask & SHIFT != 0 {
        name.push_str("Shift+");
    }
    name.push_str(key);
    Some(name)
}

/// The label a chord's action is displayed under, or `None` for an action the mode
/// group does not describe.
///
/// Exhaustive over the frozen action set, so an action added to the chord table fails
/// to build here until the panel says what it does, instead of silently losing its row.
fn chord_label(action: KeyAction) -> Option<&'static str> {
    match action {
        KeyAction::ToggleLang => Some("切换中英"),
        KeyAction::ToggleFullWidth => Some("全角 / 半角"),
        KeyAction::TogglePunct => Some("中英标点"),
        KeyAction::EnterTempEnglish => Some("临时英文"),
        KeyAction::InputChar(_)
        | KeyAction::Backspace
        | KeyAction::CommitHighlighted
        | KeyAction::CommitRaw
        | KeyAction::SelectIndex(_)
        | KeyAction::PageNext
        | KeyAction::PagePrev
        | KeyAction::MoveHighlight(_)
        | KeyAction::MoveCaret(_)
        | KeyAction::Escape
        | KeyAction::Ignore
        | KeyAction::ToggleScript
        | KeyAction::ForgetHighlighted
        | KeyAction::PinHighlighted
        | KeyAction::AddPhrase => None,
    }
}

#[cfg(test)]
mod tests;
