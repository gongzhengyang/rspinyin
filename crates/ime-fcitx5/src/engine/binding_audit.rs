//! The audit that holds the key-name whitelist and the routing table together.
//!
//! Responsibility: prove that "the keys a configuration may bind" and "the keys this layer
//! routes" describe the same set. The whitelist belongs to `ime-config` -- it is what
//! `keys.flip_keys` and `keys.highlight_keys` accept -- and the table belongs here, to
//! [`super::translate_key`]. The two live in different crates and drifted silently:
//! `page_up` was accepted by the configuration, stored in `Config` and then routed by
//! nothing, so a user who wrote `flip_keys = ["page_up"]` was handed a setting that was
//! read and ignored.
//!
//! # Boundaries
//!
//! Nothing here runs in a shipped build. The module exists so that a unit test can hold
//! both halves at once, which is the only place they can be compared: the configuration
//! crate owns the whitelist and cannot see the table, and this crate sees the table but
//! does not own the names. It reads both, and touches no host, no file, no clock and no
//! display server.
//!
//! # What the assertions cover
//!
//! * Every name the whitelist accepts reaches a key the table gives an action to, once the
//!   configuration binds everything it can.
//! * Every key whose meaning the configuration decides is a key the whitelist can name, so
//!   the table carries no binding a document could never reach.
//! * `MAX_KEY_BINDINGS` promises a list no more entries than that list can route, so the
//!   bound is a capacity rather than a number.
//! * When a key is named by both binding lists, the list the router favours is the list the
//!   configuration keeps.
//! * Every [`KeyAction`] variant is reachable by at least one key press, or is registered
//!   in [`UNBOUND_WHITELIST`] with its reason -- "implemented but reachable by no key" is
//!   the state the shortcut table forbids, and it fails here as a gate error rather than
//!   living on as a comment.
//!
//! # The coupling this module expects
//!
//! A variant added to `KeyName` must be registered here as well: [`key_for`] matches every
//! variant exhaustively, so a name that reaches the configuration without a row here stops
//! the build rather than being accepted and ignored. The same holds the other way for
//! `KeyAction`: [`canonical`] matches every variant of the action enum without a wildcard,
//! so an action that reaches the executor without an entry in [`EVERY_ACTION`] stops the
//! build until the audit decides whether a key produces it or the whitelist excuses it.

use ime_config::keymap::project_keys;
use ime_config::schema::{Config, KeyName, KeysConfig, MAX_KEY_BINDINGS};
use ime_types::KeyAction;

use crate::ffi::FcitxKeyEvent;

use super::rows::{KEY_END, KEY_HOME};
use super::*;

/// The key-name whitelist, spelled out by hand.
///
/// Deliberately not derived from [`KeyName`]: a variant that nobody adds here is exactly
/// the drift these assertions exist to catch, so deriving the list would defeat them.
const WHITELIST: [&str; 12] = [
    "minus",
    "equal",
    "up",
    "down",
    "left",
    "right",
    "tab",
    "shift_tab",
    "page_up",
    "page_down",
    "home",
    "end",
];

/// The configuration key the whitelist is read under.
///
/// Only ever the label of a diagnostic that cannot be raised: every spelling in
/// [`WHITELIST`] parses by construction, and one that stopped parsing fails the assertion
/// that asked.
const WHITELIST_KEY: &str = "keys.flip_keys";

/// The keysyms the reverse-direction walk sweeps, as inclusive bounds.
///
/// The two ranges are the whole of what XKB names below the Unicode keysyms: the printable
/// range that holds ASCII, and the function range every named key lives in. A row added
/// outside them would be a keysym no keyboard produces, so sweeping the whole `u32` space
/// would cost minutes of test time to prove nothing.
const SWEPT_RANGES: [(u32, u32); 2] = [(0x0000, 0x00ff), (0xff00, 0xffff)];

/// A key press of `sym` with `state` held.
///
/// # Panics
///
/// Never.
fn press(sym: u32, state: u32) -> FcitxKeyEvent {
    FcitxKeyEvent {
        sym,
        state,
        is_release: false,
        time_ms: 0,
    }
}

/// The key event `name` stands for.
///
/// Written out rather than derived, like [`WHITELIST`], and the match is exhaustive over
/// [`KeyName`] -- which is the half of the coupling the spelling list cannot cover: a name
/// added to the configuration without a row here fails to compile. The keysyms are the
/// routing table's own constants rather than copies, because the table being audited is the
/// one that reads them, and a second set of numbers could only disagree with it.
///
/// # Panics
///
/// Never.
fn key_for(name: KeyName) -> FcitxKeyEvent {
    let (sym, state) = match name {
        KeyName::Minus => (KEY_MINUS, 0),
        KeyName::Equal => (KEY_EQUAL, 0),
        KeyName::Up => (KEY_UP, 0),
        KeyName::Down => (KEY_DOWN, 0),
        KeyName::Left => (KEY_LEFT, 0),
        KeyName::Right => (KEY_RIGHT, 0),
        KeyName::Tab => (KEY_TAB, 0),
        KeyName::ShiftTab => (KEY_TAB, SHIFT),
        KeyName::PageUp => (KEY_PAGE_UP, 0),
        KeyName::PageDown => (KEY_PAGE_DOWN, 0),
        KeyName::Home => (KEY_HOME, 0),
        KeyName::End => (KEY_END, 0),
    };
    press(sym, state)
}

/// The keysyms the whitelist reaches, one per spelling it accepts.
///
/// A spelling that stopped parsing is left out, which makes the walk that compares against
/// this list report it rather than stopping at it.
///
/// # Panics
///
/// Never.
fn named_keysyms() -> Vec<u32> {
    let mut syms = Vec::with_capacity(WHITELIST.len());
    for spelling in WHITELIST {
        if let Ok(name) = KeyName::parse(spelling, WHITELIST_KEY) {
            syms.push(key_for(name).sym);
        }
    }
    syms
}

/// The bindings that claim every key a configuration can bind.
///
/// The question this module asks -- "can this key ever be routed?" -- is only answerable
/// with everything bound: a key the configuration leaves unbound belongs to the application
/// by design, which is what `keys.flip_keys = []` means.
///
/// # Panics
///
/// Never.
fn everything_bound() -> KeyBindings {
    KeyBindings {
        flip_keys: FlipSet::all(),
        highlight_keys: HighlightSet::all(),
        ..KeyBindings::default()
    }
}

/// The bindings that claim nothing at all.
///
/// The other end of the comparison the reverse-direction walk makes: a key whose answer
/// moves between this table and [`everything_bound`] is a key the configuration decides,
/// and one the whitelist has to be able to name.
///
/// # Panics
///
/// Never.
fn nothing_bound() -> KeyBindings {
    KeyBindings {
        flip_keys: FlipSet::empty(),
        highlight_keys: HighlightSet::empty(),
        ..KeyBindings::default()
    }
}

/// Whether the routing table gives `name` an action.
///
/// # Arguments
///
/// * `name` -- a key the configuration's whitelist accepts.
/// * `keys` -- the binding table to ask. A key the table has a row for is still unrouted
///   when the configuration leaves it unbound, which is what makes this the question an
///   audit has to ask rather than a lookup of the row table alone.
///
/// # Returns
///
/// `true` when [`super::translate_key`] answers with anything but [`KeyAction::Ignore`],
/// i.e. when the table has a row for the key and `keys` reaches it.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never.
pub(crate) fn routes_any_binding(name: KeyName, keys: &KeyBindings) -> bool {
    !matches!(translate_key(&key_for(name), keys), KeyAction::Ignore)
}

/// The whitelisted spellings nothing routes under `keys`.
///
/// A list rather than a failure on the first unrouted name, so that a caller can assert on
/// it: the audit's own test asserts the list is empty, and the test that proves the audit has
/// teeth asserts the one name it names. A name that fails to parse would leave a hole in the
/// walk, so it is a failure rather than a skip.
///
/// # Panics
///
/// Never in practice: every spelling in [`WHITELIST`] is in the whitelist by construction.
/// A spelling that stopped parsing fails here, which is the assertion the walk exists for.
fn unrouted_names(keys: &KeyBindings) -> Vec<&'static str> {
    let mut unrouted = Vec::new();
    for spelling in WHITELIST {
        let name = KeyName::parse(spelling, WHITELIST_KEY).expect("the whitelist parses");
        if !routes_any_binding(name, keys) {
            unrouted.push(spelling);
        }
    }
    unrouted
}

/// How many of the whitelisted names each binding list can route.
///
/// Asked of the projection rather than counted from the flag sets: a bit the routing table
/// has no row for is a name the configuration accepts and nothing acts on, and counting
/// bits would call that a capacity. Each probe binds one name in one list, because binding
/// it in both would make the projection settle the very conflict the count is about.
///
/// # Returns
///
/// The number of names `keys.flip_keys` can route, and the number `keys.highlight_keys` can.
///
/// # Panics
///
/// Never in practice, for the reason [`unrouted_names`] gives.
fn bindable_counts() -> (usize, usize) {
    // `KeysConfig` deliberately has no `Default` of its own: the built-in defaults are
    // one document in one place, and that place is `Config::default`. Taking the section
    // from there is what keeps this audit measuring the configuration the plugin
    // actually ships with.
    let template = Config::default().keys;
    let mut pageable = 0;
    let mut highlightable = 0;
    for spelling in WHITELIST {
        let name = KeyName::parse(spelling, WHITELIST_KEY).expect("the whitelist parses");
        let page = KeysConfig {
            flip_keys: vec![name],
            highlight_keys: Vec::new(),
            ..template.clone()
        };
        let highlight = KeysConfig {
            flip_keys: Vec::new(),
            highlight_keys: vec![name],
            ..template.clone()
        };
        if !project_keys(&page).0.flip_keys.is_empty() {
            pageable += 1;
        }
        if !project_keys(&highlight).0.highlight_keys.is_empty() {
            highlightable += 1;
        }
    }
    (pageable, highlightable)
}

// ── The action audit ─────────────────────────────────────────────────────────────
//
// The whitelist above holds the configuration's half together with the table's; this
// half holds the executor's half together with it. The rule it enforces is the one the
// shortcut table states: an action is either reachable from the keyboard or excused in
// the whitelist, and there is no third state.

/// The actions the executor implements that deliberately have no binding.
///
/// The list is read by the assertions below, so an entry that stopped being true -- a
/// binding added for one of these, say -- fails a test instead of rotting into an alibi
/// for an action that has long been reachable.
///
/// * [`KeyAction::ToggleLang`] -- the language switch is the host's own hotkey
///   (`ASM-02`): Fcitx5's headers expose no per-context input-method state an engine
///   could read or write, so a chord here could only swallow the key.
/// * [`KeyAction::ToggleScript`] -- v1 does not enable the simplified/traditional
///   conversion, so binding a key that flips a display nothing changes would be the
///   "claimed a key with no effect" defect; the chord returns when the conversion ships.
/// * [`KeyAction::PinHighlighted`] -- the pin set does not exist yet: the step reports
///   `dict/unsupported` and changes nothing, so no key may be taken for it.
/// * [`KeyAction::AddPhrase`] -- waiting on the phrase-editor delivery, which binds it
///   when the workflow it belongs to exists.
const UNBOUND_WHITELIST: [KeyAction; 4] = [
    KeyAction::ToggleLang,
    KeyAction::ToggleScript,
    KeyAction::PinHighlighted,
    KeyAction::AddPhrase,
];

/// Every variant of the frozen action enum, with a representative payload for the ones
/// that carry one.
///
/// Spelled out and sized by hand, like [`WHITELIST`]: a variant nobody adds here is
/// exactly the gap the assertions below exist to catch, and the explicit size is what
/// makes adding one a conscious edit rather than an accident of enumeration.
const EVERY_ACTION: [KeyAction; 21] = [
    KeyAction::InputChar('a'),
    KeyAction::Backspace,
    KeyAction::CommitHighlighted,
    KeyAction::CommitRaw,
    KeyAction::SelectIndex(1),
    KeyAction::PageNext,
    KeyAction::PagePrev,
    KeyAction::PageFirst,
    KeyAction::PageLast,
    KeyAction::MoveHighlight(1),
    KeyAction::MoveCaret(-1),
    KeyAction::ToggleLang,
    KeyAction::ToggleFullWidth,
    KeyAction::TogglePunct,
    KeyAction::EnterTempEnglish,
    KeyAction::Escape,
    KeyAction::Ignore,
    KeyAction::ToggleScript,
    KeyAction::ForgetHighlighted,
    KeyAction::PinHighlighted,
    KeyAction::AddPhrase,
];

/// The canonical form of `action`, exhaustively matched over the frozen enum.
///
/// The match is the tripwire that pairs this module with the contract: a variant added
/// to `KeyAction` stops the build here until it has an arm and an entry in
/// [`EVERY_ACTION`], which is what turns "an action no key can ever produce" from a
/// silent gap into a compile error.
///
/// # Panics
///
/// Never.
fn canonical(action: KeyAction) -> KeyAction {
    match action {
        KeyAction::InputChar(_) => KeyAction::InputChar('a'),
        KeyAction::Backspace => KeyAction::Backspace,
        KeyAction::CommitHighlighted => KeyAction::CommitHighlighted,
        KeyAction::CommitRaw => KeyAction::CommitRaw,
        KeyAction::SelectIndex(_) => KeyAction::SelectIndex(1),
        KeyAction::PageNext => KeyAction::PageNext,
        KeyAction::PagePrev => KeyAction::PagePrev,
        KeyAction::PageFirst => KeyAction::PageFirst,
        KeyAction::PageLast => KeyAction::PageLast,
        KeyAction::MoveHighlight(_) => KeyAction::MoveHighlight(1),
        KeyAction::MoveCaret(_) => KeyAction::MoveCaret(-1),
        KeyAction::ToggleLang => KeyAction::ToggleLang,
        KeyAction::ToggleFullWidth => KeyAction::ToggleFullWidth,
        KeyAction::TogglePunct => KeyAction::TogglePunct,
        KeyAction::EnterTempEnglish => KeyAction::EnterTempEnglish,
        KeyAction::Escape => KeyAction::Escape,
        KeyAction::Ignore => KeyAction::Ignore,
        KeyAction::ToggleScript => KeyAction::ToggleScript,
        KeyAction::ForgetHighlighted => KeyAction::ForgetHighlighted,
        KeyAction::PinHighlighted => KeyAction::PinHighlighted,
        KeyAction::AddPhrase => KeyAction::AddPhrase,
    }
}

/// The modifier sets the reachability walk holds while it presses.
///
/// Nothing, Shift, Ctrl and the Ctrl+Shift pair: the four sets every chord and every
/// row of the table answers for. A key under any other set belongs to the desktop, and
/// no binding may live there.
const WALKED_STATES: [u32; 4] = [0, SHIFT, CTRL, CTRL | SHIFT];

/// Every action the table produces anywhere in the walked key space.
///
/// The walk presses each keysym [`SWEPT_RANGES`] covers, under each of
/// [`WALKED_STATES`], through two binding tables: everything the configuration can
/// bind, and nothing bound with `enter_commit_raw` on. The second table is not
/// variation for its own sake -- the caret rows answer only where the highlight rows
/// do not, and `Return` commits the raw input only when the document says so -- so
/// each of the two is the only shape that reaches some of the actions.
///
/// # Panics
///
/// Never.
fn produced_actions() -> Vec<KeyAction> {
    let tables = [
        everything_bound(),
        KeyBindings {
            enter_commit_raw: true,
            ..nothing_bound()
        },
    ];
    let mut produced = Vec::new();
    for keys in &tables {
        for (low, high) in SWEPT_RANGES {
            for sym in low..=high {
                for state in WALKED_STATES {
                    produced.push(translate_key(&press(sym, state), keys));
                }
            }
        }
    }
    produced
}

#[test]
fn test_every_whitelisted_key_name_is_routable() {
    // A name the configuration accepts and the table has no row for is a setting that is
    // read, stored and then ignored. Every name is checked with everything bound, which is
    // the only way to ask whether the key can ever be routed at all.
    let unrouted = unrouted_names(&everything_bound());
    assert!(
        unrouted.is_empty(),
        "accepted by the configuration and routed by nothing: {unrouted:?}"
    );
}

#[test]
fn test_the_whitelist_audit_names_the_key_that_stopped_being_routed() {
    // The assertion above is only worth making if it can fail, and the failure has to name
    // the key: `page_up` is the name whose row was missing when this module was written, so
    // it is the one the audit is asked to catch. The row is taken away the way a user
    // reaches it -- by unbinding the key -- because the table itself is not this module's
    // to edit; what the walk sees is the same `Ignore` a missing row produces.
    let page_up_unbound = KeyBindings {
        flip_keys: FlipSet::all() - FlipSet::PAGE_UP,
        ..everything_bound()
    };
    assert_eq!(unrouted_names(&page_up_unbound), ["page_up"]);
}

#[test]
fn test_routes_any_binding_answers_false_for_a_key_nothing_binds() {
    // The second half of "the assertion has teeth": the walk reports a key as routed
    // because the table answered, not because it always answers. `page_up`, `tab` and
    // `minus` are each carried by one list alone, so a configuration that binds neither
    // list leaves all three with nothing to reach.
    let nothing = nothing_bound();
    assert!(!routes_any_binding(KeyName::PageUp, &nothing));
    assert!(!routes_any_binding(KeyName::Tab, &nothing));
    assert!(!routes_any_binding(KeyName::Minus, &nothing));
    // The page jumps are pageable names like the others: unbound, they are the host's.
    assert!(!routes_any_binding(KeyName::Home, &nothing));
    assert!(!routes_any_binding(KeyName::End, &nothing));
    // The caret rows are the other side of it: no document can unbind `Left` or `Right`,
    // so the audit must still find them routed with the two lists empty.
    assert!(routes_any_binding(KeyName::Left, &nothing));
    assert!(routes_any_binding(KeyName::Right, &nothing));
}

#[test]
fn test_the_bound_is_reachable() {
    // `MAX_KEY_BINDINGS` is the length a document may give either list. A bound larger than
    // what a list can route is a promise the configuration cannot keep: the entries past
    // the routable ones are accepted, stored, and then do nothing -- the shape of
    // `page_up` over again, with the limit itself as the defect.
    let (pageable, highlightable) = bindable_counts();
    assert!(
        MAX_KEY_BINDINGS <= pageable,
        "keys.flip_keys may hold {MAX_KEY_BINDINGS} entries but routes {pageable} names"
    );
    assert!(
        MAX_KEY_BINDINGS <= highlightable,
        "keys.highlight_keys may hold {MAX_KEY_BINDINGS} entries but routes {highlightable} names"
    );
}

#[test]
fn test_the_router_favours_the_list_the_configuration_keeps() {
    // One key named by both lists. The configuration layer settles it by dropping the page
    // entry and reporting the conflict; the routing table settles it by answering with the
    // highlight move. Both must give one answer, because a user reads the configuration and
    // types into the router, and two answers would make the diagnostic a lie.
    for (name, delta) in [(KeyName::Up, -1), (KeyName::Down, 1)] {
        let mut conflicted = Config::default();
        conflicted.keys.flip_keys = vec![name];
        conflicted.keys.highlight_keys = vec![name];

        let (repaired, warnings) = conflicted.repaired();

        assert_eq!(warnings.len(), 1, "one conflict, one diagnostic");
        assert!(
            repaired.keys.flip_keys.is_empty(),
            "the page entry is the one that gives way"
        );
        assert_eq!(
            repaired.keys.highlight_keys,
            [name],
            "the highlight entry stays"
        );
        assert!(
            repaired.validate().is_empty(),
            "the repaired configuration has nothing left to settle"
        );

        let both_lists = everything_bound();
        assert_eq!(
            translate_key(&key_for(name), &both_lists),
            KeyAction::MoveHighlight(delta),
            "the router moves the highlight for a key both lists name"
        );
    }
}

#[test]
fn test_every_key_the_configuration_can_bind_is_named_by_the_whitelist() {
    // The other direction of the same invariant, and the half the whitelist cannot state on
    // its own: a key whose meaning the configuration decides has to be a key the
    // configuration can name, or the table carries a binding no document can ever reach --
    // an unroutable name seen from the table's side. The two ends are compared rather than
    // the table's rows, so a row that is added, reordered or renamed is covered without
    // this module reading the table's internals.
    let named = named_keysyms();
    let bound = everything_bound();
    let nothing = nothing_bound();
    let mut decided = 0;
    for (low, high) in SWEPT_RANGES {
        for sym in low..=high {
            for state in [0, SHIFT] {
                let bound_answer = translate_key(&press(sym, state), &bound);
                let empty_answer = translate_key(&press(sym, state), &nothing);
                if bound_answer == empty_answer {
                    continue;
                }
                decided += 1;
                assert!(
                    named.contains(&sym),
                    "sym {sym:#06x} state {state:#x} answers {bound_answer:?} with the lists \
                     bound and {empty_answer:?} with them empty, but no key name reaches it"
                );
            }
        }
    }
    assert!(decided > 0, "the sweep must have compared something");
}

#[test]
fn test_every_key_action_is_reachable_by_a_key_or_whitelisted() {
    // The audit that closes the executor's half: an action the executor supports but
    // no key can reach is a user who cannot delete a mis-learned word, pin one or
    // save a phrase from the keyboard, whatever they write into `[keys]`. Every
    // variant of the frozen enum is either produced somewhere in the walked key space
    // or registered in [`UNBOUND_WHITELIST`] with its reason; anything else fails
    // here as a gate error rather than living on as a comment beside the enum.
    let produced = produced_actions();
    let mut unreachable = Vec::new();
    for action in EVERY_ACTION {
        let action = canonical(action);
        if UNBOUND_WHITELIST.contains(&action) {
            continue;
        }
        if !produced.contains(&action) {
            unreachable.push(action);
        }
    }
    assert!(
        unreachable.is_empty(),
        "implemented, bound nowhere and excused nowhere: {unreachable:?}"
    );
}

#[test]
fn test_the_unbound_whitelist_stays_unbound() {
    // The same walk read the other way: a whitelist entry that a key has begun to
    // produce is a stale excuse -- the action it excuses has a binding this list
    // pretends does not exist. The entry then leaves the list, and its reason with it.
    let produced = produced_actions();
    for whitelisted in UNBOUND_WHITELIST {
        assert!(
            !produced.contains(&whitelisted),
            "{whitelisted:?} is excused as unbound and produced by a key"
        );
    }
}

#[test]
fn test_the_unbound_whitelist_names_actions_the_frozen_enum_defines() {
    // The whitelist is an annotation over the enum, so every entry has to be one of
    // the audited actions, already in the canonical form the reachability walk
    // compares: an entry that misspelled a variant or carried a payload would quietly
    // excuse nothing while looking like an excuse.
    for whitelisted in UNBOUND_WHITELIST {
        assert!(
            EVERY_ACTION.contains(&whitelisted),
            "{whitelisted:?} is not one of the audited actions"
        );
        assert_eq!(canonical(whitelisted), whitelisted);
    }
}
