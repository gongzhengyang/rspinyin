//! The rows of the routing table: which key, with which modifiers, means what.
//!
//! # Responsibility
//!
//! One table of data and the small functions that read the configuration for it.
//! [`ROWS`] holds every key the plugin routes that is not a chord, in evaluation order: the
//! rows `keys.highlight_keys` binds come first, then the rows `keys.flip_keys` binds, then
//! the rows the configuration cannot unbind. [`action`] is the whole table's answer for one
//! key.
//!
//! The chords — the keys whose modifiers are part of the key, `Ctrl+Space` and its
//! neighbours — are the caller's, because a chord is matched before any row is read and
//! matches on an exact modifier set rather than on a tolerated one.
//!
//! # Why a row is data
//!
//! A row is a set of keysyms plus the predicate deciding which modifier sets it accepts, so a
//! key can be read by a shift-tolerant row and by a bare row without being written twice.
//! Keeping the two groups in one table is what lets the configuration reach both: when they
//! were two functions, `Tab` lived in one and the arrows in the other, and the
//! `keys.highlight_keys` document reached neither.
//!
//! A row the configuration did not bind answers `None` and the walk goes on to the next row
//! that matches the keysym, which is how one key can be a highlight key in one document and a
//! page key in another, and how a key bound as neither falls through to the row that names it
//! for everyone.
//!
//! # Boundary
//!
//! Plain Rust and pure: no host object, no file, no clock and no global state. Every row is a
//! function of the keysym, the modifier mask and the `[keys]` settings, which is what makes
//! the whole table testable one row at a time without Fcitx5 present.

use ime_types::KeyAction;

use super::{
    CTRL, DigitZero, FlipSet, HighlightSet, KEY_0, KEY_1, KEY_9, KEY_A, KEY_APOSTROPHE,
    KEY_BACKSPACE, KEY_DOWN, KEY_EQUAL, KEY_ESCAPE, KEY_LEFT, KEY_MINUS, KEY_PAGE_DOWN,
    KEY_PAGE_UP, KEY_RETURN, KEY_RIGHT, KEY_SPACE, KEY_TAB, KEY_UP, KEY_Z, KeyBindings,
    MODIFIER_MASK, NON_SHIFT_MODIFIERS, SHIFT, SYLLABLE_SEPARATOR,
};

// The keysyms of the three rows this module added, declared beside the rows that read
// them: each has exactly one reader, and the module root's keysym table holds the keys
// the whole routing layer names.

/// `FcitxKey_Home`, which jumps to the first page when `keys.flip_keys` names it.
pub(super) const KEY_HOME: u32 = 0xff50;

/// `FcitxKey_End`, which jumps to the last page when `keys.flip_keys` names it.
pub(super) const KEY_END: u32 = 0xff57;

/// `FcitxKey_Delete`, whose `Ctrl` chord drops the highlighted word from the user's
/// learned frequencies.
pub(super) const KEY_DELETE: u32 = 0xffff;

/// The table's answer for one key press, or `None` when no row names the key.
///
/// The caller has already refused the release edge, folded a case-folded symbol back to its
/// lowercase shape and asked the chords; this sees the key the rows are written against.
///
/// # Arguments
///
/// * `sym` — the keysym, folded.
/// * `state` — the modifier mask as the host delivered it.
/// * `keys` — the `[keys]` settings the rows branch on.
///
/// # Returns
///
/// The action of the first row that matches the key and answers for it, or `None` when no
/// row does. `None` is what hands the key back to the application.
///
/// # Panics
///
/// Never: every row is a comparison over two integers and a read of a value the caller
/// passed in, with no indexing and no arithmetic that can overflow.
pub(super) fn action(sym: u32, state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    for row in ROWS {
        if !row.keys.contains(sym) || !row.accepts.tolerates(state) {
            continue;
        }
        if let Some(action) = (row.action)(sym, state, keys) {
            return Some(action);
        }
    }
    None
}

/// One row of the routing table.
struct Row {
    /// The keysyms this row matches.
    keys: Keys,
    /// Which modifier sets the row accepts.
    accepts: Accepts,
    /// What the row means, given the bindings.
    action: RowAction,
}

/// What a row means, given the key and the configuration.
///
/// The state is handed over rather than only tested because two rows read it: `Tab` reverses
/// its direction when Shift is held, and a digit names the candidate the keysym itself
/// carries. The configuration is the third input because most rows can be unbound.
///
/// `None` means "not this row's key after all" — a binding the configuration did not make —
/// and sends the walk on to the next row that matches the keysym.
type RowAction = fn(u32, u32, &KeyBindings) -> Option<KeyAction>;

/// The keysyms one row matches.
///
/// A range as well as a single keysym, because two rows are ranges: the ten digits and the
/// twenty-six letters. Writing those out as thirty-six rows would be a table nobody could
/// keep in step with the alphabet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Keys {
    /// One keysym.
    One(u32),
    /// An inclusive range of keysyms.
    Range(u32, u32),
    /// An explicit set, for the marks that share one row.
    Set(&'static [u32]),
}

impl Keys {
    /// Whether `sym` is one of this row's keysyms.
    ///
    /// # Panics
    ///
    /// Never.
    const fn contains(self, sym: u32) -> bool {
        match self {
            Self::One(one) => sym == one,
            Self::Range(low, high) => sym >= low && sym <= high,
            // `slice::contains` is not `const`, and the walk runs in one, so the scan
            // is a loop over a list the compiler bounds at twelve entries.
            Self::Set(set) => {
                let mut found = false;
                let mut index = 0;
                while index < set.len() {
                    found = found || set[index] == sym;
                    index += 1;
                }
                found
            }
        }
    }
}

/// The modifier sets a row tolerates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Accepts {
    /// Only the bare press; any modifier makes it somebody else's key.
    BareOnly,
    /// A bare press, or one with Shift held. Shift is how the uppercase form of a letter is
    /// typed and how `Shift+Tab` reverses a direction, so a row that accepts it reads the
    /// state to tell its two shapes apart.
    BareOrShift,
    /// Only with `Ctrl` held, and nothing beside it: the exact set, compared the way a
    /// chord's is, so `Ctrl+Shift+Delete` stays the application's exactly as an
    /// over-specified chord does.
    CtrlOnly,
}

impl Accepts {
    /// Whether a row that accepts this much tolerates `state`.
    ///
    /// # Panics
    ///
    /// Never.
    const fn tolerates(self, state: u32) -> bool {
        match self {
            Self::BareOnly => (state & MODIFIER_MASK) == 0,
            Self::BareOrShift => (state & NON_SHIFT_MODIFIERS) == 0,
            Self::CtrlOnly => (state & MODIFIER_MASK) == CTRL,
        }
    }
}

/// The routing table, in evaluation order.
///
/// The order is the precedence the configuration documents. `keys.highlight_keys` comes
/// first because moving the highlight is the more frequent gesture inside a composition, and
/// a key both lists name therefore moves the highlight: the page row below it is reached only
/// when the highlight row answered `None`, which is exactly when the configuration did not
/// bind the key as a highlight key.
///
/// The rows after the two bindable groups are the composing keymap's own, and no document can
/// unbind them.
const ROWS: &[Row] = &[
    // `keys.highlight_keys`.
    Row {
        keys: Keys::One(KEY_TAB),
        accepts: Accepts::BareOrShift,
        action: tab_highlight,
    },
    Row {
        keys: Keys::One(KEY_UP),
        accepts: Accepts::BareOnly,
        action: up_highlight,
    },
    Row {
        keys: Keys::One(KEY_DOWN),
        accepts: Accepts::BareOnly,
        action: down_highlight,
    },
    Row {
        keys: Keys::One(KEY_LEFT),
        accepts: Accepts::BareOnly,
        action: left_highlight,
    },
    Row {
        keys: Keys::One(KEY_RIGHT),
        accepts: Accepts::BareOnly,
        action: right_highlight,
    },
    // `keys.flip_keys`.
    Row {
        keys: Keys::One(KEY_MINUS),
        accepts: Accepts::BareOnly,
        action: minus_page,
    },
    Row {
        keys: Keys::One(KEY_EQUAL),
        accepts: Accepts::BareOnly,
        action: equal_page,
    },
    Row {
        keys: Keys::One(KEY_UP),
        accepts: Accepts::BareOnly,
        action: up_page,
    },
    Row {
        keys: Keys::One(KEY_DOWN),
        accepts: Accepts::BareOnly,
        action: down_page,
    },
    Row {
        keys: Keys::One(KEY_PAGE_UP),
        accepts: Accepts::BareOnly,
        action: page_up,
    },
    Row {
        keys: Keys::One(KEY_PAGE_DOWN),
        accepts: Accepts::BareOnly,
        action: page_down,
    },
    Row {
        keys: Keys::One(KEY_HOME),
        accepts: Accepts::BareOnly,
        action: home_page,
    },
    Row {
        keys: Keys::One(KEY_END),
        accepts: Accepts::BareOnly,
        action: end_page,
    },
    // The digits, `0` among them because `keys.digit_zero` decides what it does.
    Row {
        keys: Keys::Range(KEY_0, KEY_9),
        accepts: Accepts::BareOnly,
        action: digit,
    },
    // The letters, which tolerate Shift because that is how their capital is typed.
    Row {
        keys: Keys::Range(KEY_A, KEY_Z),
        accepts: Accepts::BareOrShift,
        action: letter,
    },
    // The syllable separator, which only means something inside a composition.
    Row {
        keys: Keys::One(KEY_APOSTROPHE),
        accepts: Accepts::BareOnly,
        action: separator,
    },
    // The punctuation the passthrough policy answers: the eleven substitutable marks and
    // the at-sign. The row claims the key so the policy can decide what it becomes; the
    // claim is not a promise to keep it, and a mark the policy hands back reaches the
    // application exactly as an unclaimed key would. The apostrophe is the separator's
    // key and deliberately absent here.
    Row {
        keys: Keys::Set(POLICY_MARKS),
        accepts: Accepts::BareOnly,
        action: policy_mark,
    },
    // The composing keymap: no document can unbind these.
    Row {
        keys: Keys::One(KEY_SPACE),
        accepts: Accepts::BareOnly,
        action: commit_highlighted,
    },
    Row {
        keys: Keys::One(KEY_RETURN),
        accepts: Accepts::BareOnly,
        action: enter,
    },
    Row {
        keys: Keys::One(KEY_LEFT),
        accepts: Accepts::BareOnly,
        action: caret_left,
    },
    Row {
        keys: Keys::One(KEY_RIGHT),
        accepts: Accepts::BareOnly,
        action: caret_right,
    },
    Row {
        keys: Keys::One(KEY_ESCAPE),
        accepts: Accepts::BareOnly,
        action: escape,
    },
    Row {
        keys: Keys::One(KEY_BACKSPACE),
        accepts: Accepts::BareOnly,
        action: backspace,
    },
    // The user-word chord: `Ctrl+Delete` drops the highlighted word from the learned
    // frequencies. Like the composing keymap above it, no document can unbind it -- and
    // unlike the rows before it, it answers only while a composition has a candidate to
    // act on, which is the session's and the arbitrator's to decide, not the table's.
    Row {
        keys: Keys::One(KEY_DELETE),
        accepts: Accepts::CtrlOnly,
        action: ctrl_delete_forget,
    },
];

/// The `Tab` row, whose two shapes `keys.highlight_keys` names separately.
///
/// The bare shape is `tab` and the shifted one is `shift_tab`, so a document that binds only
/// one of them leaves the other to the application. Shift is the direction rather than a
/// different key, which is why the row tolerates it at all.
///
/// # Panics
///
/// Never.
fn tab_highlight(_sym: u32, state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    if (state & SHIFT) == 0 {
        bound_highlight(keys.highlight_keys, HighlightSet::TAB, 1)
    } else {
        bound_highlight(keys.highlight_keys, HighlightSet::SHIFT_TAB, -1)
    }
}

/// The `Up` row of `keys.highlight_keys`.
///
/// # Panics
///
/// Never.
fn up_highlight(_sym: u32, _state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    bound_highlight(keys.highlight_keys, HighlightSet::UP, -1)
}

/// The `Down` row of `keys.highlight_keys`.
///
/// # Panics
///
/// Never.
fn down_highlight(_sym: u32, _state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    bound_highlight(keys.highlight_keys, HighlightSet::DOWN, 1)
}

/// The `Left` row of `keys.highlight_keys`.
///
/// # Panics
///
/// Never.
fn left_highlight(_sym: u32, _state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    bound_highlight(keys.highlight_keys, HighlightSet::LEFT, -1)
}

/// The `Right` row of `keys.highlight_keys`.
///
/// # Panics
///
/// Never.
fn right_highlight(_sym: u32, _state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    bound_highlight(keys.highlight_keys, HighlightSet::RIGHT, 1)
}

/// The `-` row of `keys.flip_keys`.
///
/// # Panics
///
/// Never.
fn minus_page(_sym: u32, _state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    bound_page(keys.flip_keys, FlipSet::MINUS, KeyAction::PagePrev)
}

/// The `=` row of `keys.flip_keys`.
///
/// # Panics
///
/// Never.
fn equal_page(_sym: u32, _state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    bound_page(keys.flip_keys, FlipSet::EQUAL, KeyAction::PageNext)
}

/// The `Up` row of `keys.flip_keys`.
///
/// # Panics
///
/// Never.
fn up_page(_sym: u32, _state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    bound_page(keys.flip_keys, FlipSet::UP, KeyAction::PagePrev)
}

/// The `Down` row of `keys.flip_keys`.
///
/// # Panics
///
/// Never.
fn down_page(_sym: u32, _state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    bound_page(keys.flip_keys, FlipSet::DOWN, KeyAction::PageNext)
}

/// The `Page_Up` row of `keys.flip_keys`.
///
/// # Panics
///
/// Never.
fn page_up(_sym: u32, _state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    bound_page(keys.flip_keys, FlipSet::PAGE_UP, KeyAction::PagePrev)
}

/// The `Page_Down` row of `keys.flip_keys`.
///
/// # Panics
///
/// Never.
fn page_down(_sym: u32, _state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    bound_page(keys.flip_keys, FlipSet::PAGE_DOWN, KeyAction::PageNext)
}

/// The `Home` row of `keys.flip_keys`: jump straight to the first page.
///
/// # Panics
///
/// Never.
fn home_page(_sym: u32, _state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    bound_page(keys.flip_keys, FlipSet::HOME, KeyAction::PageFirst)
}

/// The `End` row of `keys.flip_keys`: jump straight to the last page.
///
/// # Panics
///
/// Never.
fn end_page(_sym: u32, _state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    bound_page(keys.flip_keys, FlipSet::END, KeyAction::PageLast)
}

/// The digit row: `1`-`9` select a candidate, `0` pages or stays with the host.
///
/// One row rather than ten, because the ten keysyms are one key: the action carries the digit
/// itself. `0` is the odd one out — `keys.digit_zero` decides whether it turns the page — and
/// in passthrough mode it is the host's digit rather than a candidate index, so the row
/// answers `None` and lets the walk hand the key back.
///
/// # Panics
///
/// Never.
fn digit(sym: u32, _state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    match sym {
        KEY_0 if keys.digit_zero == DigitZero::Flip => Some(KeyAction::PageNext),
        KEY_0 => None,
        KEY_1..=KEY_9 => Some(KeyAction::SelectIndex((sym - KEY_0) as u8)),
        // The row matches `0`-`9`; the arm keeps the function total rather than resting on the
        // row's range.
        _ => None,
    }
}

/// The letter row: the character the keysym carries.
///
/// Shift is tolerated because that is how a capital is typed, and the fold has already turned
/// a case-folded symbol back into its lowercase shape, so the row only ever sees `a`-`z`.
///
/// # Panics
///
/// Never.
fn letter(sym: u32, _state: u32, _keys: &KeyBindings) -> Option<KeyAction> {
    if !(KEY_A..=KEY_Z).contains(&sym) {
        return None;
    }
    // The range is ASCII, so the low byte is the character.
    Some(KeyAction::InputChar(char::from(sym as u8)))
}

/// The `'` row: the syllable separator the composing input accepts.
///
/// The row names the key in every context, and the guard that keeps it inside a composition
/// is [`is_syllable_separator`](super::is_syllable_separator)'s, asked by the layers that hold
/// a session: the table sees a key and its modifiers and cannot see whether anything is
/// composing.
///
/// # Panics
///
/// Never.
fn separator(_sym: u32, _state: u32, _keys: &KeyBindings) -> Option<KeyAction> {
    Some(KeyAction::InputChar(SYLLABLE_SEPARATOR))
}

/// The keysyms of the punctuation row, as their ASCII codes.
///
/// The eleven marks the substitution table carries, plus the at-sign — the one URL
/// marker a single keystroke can produce, which is what gives `engine.passthrough_url`
/// a key of its own to answer. The apostrophe is not among them: it is the separator's.
const POLICY_MARKS: &[u32] = &[
    0x21, // !
    0x22, // "
    0x28, // (
    0x29, // )
    0x2c, // ,
    0x2e, // .
    0x3a, // :
    0x3b, // ;
    0x3f, // ?
    0x40, // @
    0x5b, // [
    0x5d, // ]
];

/// The punctuation row: the key becomes the mark itself, and the passthrough policy —
/// not the table — decides what the mark is worth. The table is a function of the key
/// and the modifiers and can see neither the mode bits nor the session, which is why
/// the row answers with the bare character and stops.
///
/// # Panics
///
/// Never.
fn policy_mark(sym: u32, _state: u32, _keys: &KeyBindings) -> Option<KeyAction> {
    // The set holds ASCII codes only, so the low byte is the character.
    Some(KeyAction::InputChar(char::from_u32(sym)?))
}

/// Whether a typed character is one of the punctuation row's marks.
///
/// The router asks this where the table cannot: while a composition is live, a mark is
/// handed back to the application — the shape it had when the table claimed nothing —
/// because the policy's composition half, a mark that carries the pending candidates
/// out, is session-machine work that has not landed. A letter never satisfies this, so
/// the composing input is untouched.
///
/// # Panics
///
/// Never.
pub(crate) fn is_policy_mark(ch: char) -> bool {
    POLICY_MARKS.contains(&(ch as u32))
}

/// The space bar: commit the candidate the highlight is on.
///
/// A bare press only. `Ctrl+Space` and `Shift+Space` are chords and are answered before the
/// rows are read, and the space bar with any other modifier belongs to the host. That the key
/// only commits while something is composing is the session's answer, not this row's.
///
/// # Panics
///
/// Never.
fn commit_highlighted(_sym: u32, _state: u32, _keys: &KeyBindings) -> Option<KeyAction> {
    Some(KeyAction::CommitHighlighted)
}

/// The `Return` row, which `keys.enter_commit_raw` decides between two actions.
///
/// # Panics
///
/// Never.
fn enter(_sym: u32, _state: u32, keys: &KeyBindings) -> Option<KeyAction> {
    if keys.enter_commit_raw {
        Some(KeyAction::CommitRaw)
    } else {
        Some(KeyAction::CommitHighlighted)
    }
}

/// The `Left` row: move the preedit caret one syllable back.
///
/// # Panics
///
/// Never.
fn caret_left(_sym: u32, _state: u32, _keys: &KeyBindings) -> Option<KeyAction> {
    Some(KeyAction::MoveCaret(-1))
}

/// The `Right` row: move the preedit caret one syllable on.
///
/// # Panics
///
/// Never.
fn caret_right(_sym: u32, _state: u32, _keys: &KeyBindings) -> Option<KeyAction> {
    Some(KeyAction::MoveCaret(1))
}

/// The `Escape` row: cancel the composition.
///
/// # Panics
///
/// Never.
fn escape(_sym: u32, _state: u32, _keys: &KeyBindings) -> Option<KeyAction> {
    Some(KeyAction::Escape)
}

/// The `BackSpace` row: remove the trailing unit of input.
///
/// # Panics
///
/// Never.
fn backspace(_sym: u32, _state: u32, _keys: &KeyBindings) -> Option<KeyAction> {
    Some(KeyAction::Backspace)
}

/// The `Ctrl+Delete` row: drop the highlighted word from the learned frequencies.
///
/// The row names the key in every context, and the guards that keep it inside a
/// composition are the arbitrator's and the session's, asked by the layers that hold a
/// session: with nothing composing, or with no candidate under the highlight, the key
/// travels on to the application instead of being swallowed by a row that could do
/// nothing with it. The answer is fixed rather than read from `[keys]`, because a
/// binding that could delete a learned word is the one gesture this table must never
/// let a stray document silence.
///
/// # Panics
///
/// Never.
fn ctrl_delete_forget(_sym: u32, _state: u32, _keys: &KeyBindings) -> Option<KeyAction> {
    Some(KeyAction::ForgetHighlighted)
}

/// The highlight move a binding names, or `None` when the configuration did not bind it.
///
/// # Panics
///
/// Never.
fn bound_highlight(set: HighlightSet, binding: HighlightSet, delta: i8) -> Option<KeyAction> {
    set.contains(binding)
        .then_some(KeyAction::MoveHighlight(delta))
}

/// The page action a binding names, or `None` when the configuration did not bind it.
///
/// # Panics
///
/// Never.
fn bound_page(set: FlipSet, binding: FlipSet, action: KeyAction) -> Option<KeyAction> {
    set.contains(binding).then_some(action)
}

#[cfg(test)]
mod tests {
    //! The rows this module added, one case per row: the two page jumps the
    //! configuration binds, and the one fixed chord no document touches.
    //!
    //! The rest of the table is walked row by row in `engine::tests::table`; these live
    //! beside the rows they cover so that a row and its case cannot drift apart in two
    //! files.

    use ime_types::KeyAction;

    use crate::engine::{CTRL, FlipSet, KeyBindings, SHIFT, translate_key};
    use crate::ffi::FcitxKeyEvent;

    use super::{KEY_DELETE, KEY_END, KEY_HOME};

    /// A key press of `sym` with `state` held.
    fn press(sym: u32, state: u32) -> FcitxKeyEvent {
        FcitxKeyEvent {
            sym,
            state,
            is_release: false,
            time_ms: 0,
        }
    }

    #[test]
    fn test_home_and_end_jump_pages_when_the_configuration_names_them() {
        // The shipped document binds both jumps; they are page keys like any other, so
        // a configuration that leaves them out hands the keys back.
        let shipped = KeyBindings::default();
        assert_eq!(
            translate_key(&press(KEY_HOME, 0), &shipped),
            KeyAction::PageFirst
        );
        assert_eq!(
            translate_key(&press(KEY_END, 0), &shipped),
            KeyAction::PageLast
        );
        // The jump rows are bare presses, like every other page row.
        assert_eq!(
            translate_key(&press(KEY_HOME, SHIFT), &shipped),
            KeyAction::Ignore
        );
        let jumps_dropped = KeyBindings {
            flip_keys: FlipSet::MINUS | FlipSet::EQUAL,
            ..KeyBindings::default()
        };
        assert_eq!(
            translate_key(&press(KEY_HOME, 0), &jumps_dropped),
            KeyAction::Ignore
        );
        assert_eq!(
            translate_key(&press(KEY_END, 0), &jumps_dropped),
            KeyAction::Ignore
        );
    }

    #[test]
    fn test_ctrl_delete_is_the_fixed_user_word_chord() {
        // The exact modifier set is part of the key, the way a chord's is: one modifier
        // more or fewer and the key belongs to the application. The bare key has no row
        // at all.
        let keys = KeyBindings::default();
        assert_eq!(
            translate_key(&press(KEY_DELETE, CTRL), &keys),
            KeyAction::ForgetHighlighted
        );
        for state in [0, SHIFT, CTRL | SHIFT] {
            assert_eq!(
                translate_key(&press(KEY_DELETE, state), &keys),
                KeyAction::Ignore,
                "delete with {state:#x} is the application's"
            );
        }
        // The chord is on Delete, not on its neighbour.
        assert_eq!(
            translate_key(&press(crate::engine::KEY_BACKSPACE, CTRL), &keys),
            KeyAction::Ignore
        );
    }
}
