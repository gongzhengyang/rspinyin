//! Keysyms, modifier masks, the character table and the keyboard mapping.
//!
//! Responsibility: everything the injection path needs to turn "type this character" into
//! "press this keycode with these modifiers". Nothing here touches a connection, so the
//! whole module is covered by ordinary unit tests.
//!
//! # Keysym values
//!
//! The constants below are the values `fcitx-utils/keysymgen.h` defines, which is the
//! vocabulary the engine's routing table matches on. A test pins them against the host's
//! numbering, because a table that drifts would inject keys the IME silently ignores --
//! a failure that looks like a product bug.

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as XprotoExt, GetKeyboardMappingReply};
use x11rb::rust_connection::RustConnection;

use super::TestError;
use super::x11::request;

// ── Modifier masks ───────────────────────────────────────────────────────────────
//
// The values are the X protocol's, and they are the same bits an event carries in its
// `state` field: a stroke is a keysym plus the mask that must be held while it is typed.

/// `ShiftMask`.
pub const SHIFT_MASK: u16 = 1 << 0;
/// `ControlMask`.
pub const CONTROL_MASK: u16 = 1 << 2;
/// `Mod1Mask`, the Alt key on every layout this project targets.
pub const MOD1_MASK: u16 = 1 << 3;
/// `Mod4Mask`, the Super key.
pub const MOD4_MASK: u16 = 1 << 6;

/// The modifier bits this channel can hold down.
///
/// The latch masks are absent on purpose: CapsLock and NumLock are states the server
/// keeps, not keys a test presses, and a caller that asks for one is refused rather than
/// quietly given something else.
pub const HOLDABLE_MASKS: u16 = SHIFT_MASK | CONTROL_MASK | MOD1_MASK | MOD4_MASK;

/// The masks the channel presses, in the order it presses them.
pub const HOLDABLE_ORDER: [u16; 4] = [SHIFT_MASK, CONTROL_MASK, MOD1_MASK, MOD4_MASK];

// ── Key symbols ──────────────────────────────────────────────────────────────────

/// `XK_space`.
pub const KS_SPACE: u32 = 0x0020;
/// `XK_minus`, the "page back" key of the shipped configuration.
pub const KS_MINUS: u32 = 0x002d;
/// `XK_equal`, the "page forward" key of the shipped configuration.
pub const KS_EQUAL: u32 = 0x003d;
/// `XK_BackSpace`.
pub const KS_BACKSPACE: u32 = 0xff08;
/// `XK_Tab`, the "next candidate" key.
pub const KS_TAB: u32 = 0xff09;
/// `XK_Return`, the commit key.
pub const KS_RETURN: u32 = 0xff0d;
/// `XK_Escape`.
pub const KS_ESCAPE: u32 = 0xff1b;
/// `XK_Left`.
pub const KS_LEFT: u32 = 0xff51;
/// `XK_Up`.
pub const KS_UP: u32 = 0xff52;
/// `XK_Right`.
pub const KS_RIGHT: u32 = 0xff53;
/// `XK_Down`.
pub const KS_DOWN: u32 = 0xff54;
/// `XK_Shift_L`.
pub const KS_SHIFT_L: u32 = 0xffe1;
/// `XK_Control_L`.
pub const KS_CONTROL_L: u32 = 0xffe3;
/// `XK_Alt_L`.
pub const KS_ALT_L: u32 = 0xffe9;
/// `XK_Super_L`.
pub const KS_SUPER_L: u32 = 0xffeb;

/// Every keysym that makes a keycode a modifier key.
///
/// The list is the union of what X11 treats as a modifier: the four lock and shift keys,
/// the Alt/Meta/Super/Hyper pairs, and the two level-shift keys a non-US layout adds. A
/// key whose row contains one of these is a modifier, and a physically held modifier is
/// what the `clearmodifiers` pass releases before it injects anything.
const MODIFIER_KEYSYMS: [u32; 16] = [
    0xffe1, 0xffe2, // Shift_L, Shift_R
    0xffe3, 0xffe4, // Control_L, Control_R
    0xffe5, 0xffe6, // Caps_Lock, Shift_Lock
    0xffe7, 0xffe8, // Meta_L, Meta_R
    0xffe9, 0xffea, // Alt_L, Alt_R
    0xffeb, 0xffec, // Super_L, Super_R
    0xffed, 0xffee, // Hyper_L, Hyper_R
    0xff7e, 0xfe03, // Mode_switch, ISO_Level3_Shift
];

/// Whether `keysym` is one of the keys that make a keycode a modifier.
pub fn is_modifier_keysym(keysym: u32) -> bool {
    MODIFIER_KEYSYMS.contains(&keysym)
}

/// The keysym of the modifier a mask bit stands for.
///
/// # Returns
///
/// The left-hand key of the pair -- `Shift_L`, `Control_L`, `Alt_L`, `Super_L` -- or
/// `None` for a mask the channel cannot hold. The latch masks are in that second group:
/// `LockMask` and `Mod2Mask` are states the server keeps, not keys this channel presses.
pub fn modifier_keysym(mask: u16) -> Option<u32> {
    match mask {
        SHIFT_MASK => Some(KS_SHIFT_L),
        CONTROL_MASK => Some(KS_CONTROL_L),
        MOD1_MASK => Some(KS_ALT_L),
        MOD4_MASK => Some(KS_SUPER_L),
        _ => None,
    }
}

/// The keysym a key name stands for.
///
/// The names are the ones the test platform writes in a case: the editing and navigation
/// keys, the two punctuation keys the shipped configuration pages with, and the four
/// modifiers a chord can hold. The lookup is case-insensitive, and `enter` and `esc` are
/// accepted beside `return` and `escape`.
///
/// # Returns
///
/// The keysym, or `None` for a name the table does not carry.
pub fn named_keysym(name: &str) -> Option<u32> {
    let name = name.trim().to_ascii_lowercase();
    let keysym = match name.as_str() {
        "space" => KS_SPACE,
        "minus" => KS_MINUS,
        "equal" => KS_EQUAL,
        "backspace" => KS_BACKSPACE,
        "tab" => KS_TAB,
        "return" | "enter" => KS_RETURN,
        "escape" | "esc" => KS_ESCAPE,
        "left" => KS_LEFT,
        "up" => KS_UP,
        "right" => KS_RIGHT,
        "down" => KS_DOWN,
        "shift" => KS_SHIFT_L,
        "ctrl" | "control" => KS_CONTROL_L,
        "alt" => KS_ALT_L,
        "super" => KS_SUPER_L,
        _ => return None,
    };
    Some(keysym)
}

/// The modifier mask a comma-separated list of modifier names stands for.
///
/// An empty list is no modifiers at all. Every name is checked before anything is
/// returned, so a list with one bad name in it is refused rather than partly applied --
/// a chord that silently lost a modifier would make the key under test arrive as a
/// different key.
///
/// # Errors
///
/// Returns the offending name when it is not one of `shift`, `ctrl`, `alt` and `super`.
pub fn modifier_mask(names: &str) -> Result<u16, String> {
    let mut mask = 0;
    for name in names
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let bit = match name.to_ascii_lowercase().as_str() {
            "shift" => SHIFT_MASK,
            "ctrl" | "control" => CONTROL_MASK,
            "alt" => MOD1_MASK,
            "super" => MOD4_MASK,
            _ => return Err(format!("`{name}` is not a modifier name")),
        };
        mask |= bit;
    }
    Ok(mask)
}

/// One key as the channel sends it: a keysym plus the modifiers held around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyStroke {
    /// The keysym of the key itself.
    pub keysym: u32,
    /// The modifier mask held down while it is pressed.
    pub state: u16,
}

/// The stroke that types one ASCII character on the layout the harness assumes.
///
/// The layout is the plain US one every X server starts with: letters, digits and
/// punctuation sit where they do on a US keyboard, and the shifted forms are the same keys
/// with Shift held.
///
/// # Returns
///
/// The keysym and modifier mask for the character, or `None` for a character no key
/// produces -- which is every non-ASCII one. The harness types pinyin, not Chinese, so an
/// unsupported character is a test bug rather than something to work around.
pub fn stroke_for_char(ch: char) -> Option<KeyStroke> {
    let plain = |keysym| KeyStroke { keysym, state: 0 };
    let shifted = |keysym| KeyStroke {
        keysym,
        state: SHIFT_MASK,
    };
    let stroke = match ch {
        'a'..='z' => plain(ch as u32),
        'A'..='Z' => shifted(ch.to_ascii_lowercase() as u32),
        '0'..='9' => plain(ch as u32),
        // The unshifted punctuation.
        ' ' => plain(KS_SPACE),
        '-' => plain(KS_MINUS),
        '=' => plain(KS_EQUAL),
        '`' => plain(0x0060),
        '[' => plain(0x005b),
        ']' => plain(0x005d),
        '\\' => plain(0x005c),
        ';' => plain(0x003b),
        '\'' => plain(0x0027),
        ',' => plain(0x002c),
        '.' => plain(0x002e),
        '/' => plain(0x002f),
        // The shifted forms of the same keys.
        '!' => shifted(0x0031),
        '@' => shifted(0x0032),
        '#' => shifted(0x0033),
        '$' => shifted(0x0034),
        '%' => shifted(0x0035),
        '^' => shifted(0x0036),
        '&' => shifted(0x0037),
        '*' => shifted(0x0038),
        '(' => shifted(0x0039),
        ')' => shifted(0x0030),
        '_' => shifted(KS_MINUS),
        '+' => shifted(KS_EQUAL),
        '{' => shifted(0x005b),
        '}' => shifted(0x005d),
        '|' => shifted(0x005c),
        ':' => shifted(0x003b),
        '"' => shifted(0x0027),
        '<' => shifted(0x002c),
        '>' => shifted(0x002e),
        '?' => shifted(0x002f),
        '~' => shifted(0x0060),
        _ => return None,
    };
    Some(stroke)
}

/// The stroke sequence that types `text`, one stroke per character.
///
/// # Errors
///
/// Returns [`TestError::UnsupportedChar`] for a character [`stroke_for_char`] has no key
/// for. The whole text is refused rather than partly typed, so a run never leaves half a
/// string in the application under test.
///
/// # Panics
///
/// Never.
pub fn strokes(text: &str) -> Result<Vec<KeyStroke>, TestError> {
    let mut out = Vec::with_capacity(text.len());
    for ch in text.chars() {
        let stroke = stroke_for_char(ch).ok_or(TestError::UnsupportedChar { ch })?;
        out.push(stroke);
    }
    Ok(out)
}

// ── Keyboard mapping ─────────────────────────────────────────────────────────────

/// The server's keyboard mapping, reduced to the lookups the channel makes.
///
/// It is read once per session: a keycode for a keysym does not change while a run is
/// going, and a lookup per keystroke would add a round trip to every key of every test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keymap {
    /// Keycode of the first row.
    min_keycode: u8,
    /// Keysyms each keycode carries, as the server reported it.
    per_keycode: u8,
    /// The mapping, flattened: the row of keycode `min_keycode + n` starts at
    /// `n * per_keycode`.
    keysyms: Vec<u32>,
}

impl Keymap {
    /// Builds a lookup table from the parts of a `GetKeyboardMapping` reply.
    pub fn new(min_keycode: u8, keysyms_per_keycode: u8, keysyms: Vec<u32>) -> Self {
        Self {
            min_keycode,
            per_keycode: keysyms_per_keycode,
            keysyms,
        }
    }

    /// Reads the mapping from the server.
    ///
    /// # Errors
    ///
    /// Returns [`TestError::Request`] when the request or its reply fails.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn load(conn: &RustConnection) -> Result<Self, TestError> {
        let setup = conn.setup();
        let count = setup
            .max_keycode
            .saturating_sub(setup.min_keycode)
            .saturating_add(1);
        let reply: GetKeyboardMappingReply = conn
            .get_keyboard_mapping(setup.min_keycode, count)
            .map_err(request)?
            .reply()
            .map_err(request)?;
        Ok(Self::new(
            setup.min_keycode,
            reply.keysyms_per_keycode,
            reply.keysyms,
        ))
    }

    /// The first keycode whose row carries `keysym`.
    ///
    /// # Returns
    ///
    /// The keycode, or `None` when no row carries the keysym.
    pub fn keycode(&self, keysym: u32) -> Option<u8> {
        let per_row = usize::from(self.per_keycode).max(1);
        let index = self.keysyms.iter().position(|row| *row == keysym)?;
        let offset = u8::try_from(index / per_row).ok()?;
        self.min_keycode.checked_add(offset)
    }

    /// The keysyms one keycode's row carries.
    fn row(&self, keycode: u8) -> &[u32] {
        let per_row = usize::from(self.per_keycode);
        if per_row == 0 {
            return &[];
        }
        let start = usize::from(keycode.saturating_sub(self.min_keycode)) * per_row;
        let end = start.saturating_add(per_row);
        self.keysyms.get(start..end).unwrap_or(&[])
    }

    /// Whether the row of `keycode` carries a modifier keysym.
    pub fn is_modifier_keycode(&self, keycode: u8) -> bool {
        self.row(keycode).iter().copied().any(is_modifier_keysym)
    }
}

/// The keycodes a `QueryKeymap` reply reports as physically down.
///
/// The reply is a bitmap of 256 bits, one per keycode, least significant bit first: byte
/// `n`, bit `b` is keycode `n * 8 + b`. The keycodes are reported as the bitmap numbers
/// them, so the caller can release exactly what it finds.
pub fn pressed_keycodes(bitmap: &[u8; 32]) -> Vec<u8> {
    let mut down = Vec::new();
    for (byte, bits) in bitmap.iter().enumerate() {
        for bit in 0..8 {
            if bits & (1 << bit) != 0 {
                // The largest value is 31 * 8 + 7, which fits the keycode type.
                down.push((byte * 8 + bit) as u8);
            }
        }
    }
    down
}

/// The physically held modifier keycodes among the pressed ones.
///
/// This is what the `clearmodifiers` pass releases: a test that runs while the user -- or
/// a previous test -- holds Shift must not have its keys arrive as chords.
pub fn pressed_modifiers(bitmap: &[u8; 32], keymap: &Keymap) -> Vec<u8> {
    pressed_keycodes(bitmap)
        .into_iter()
        .filter(|keycode| keymap.is_modifier_keycode(*keycode))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A keymap with two keysyms per row, the shape every server reports.
    fn keymap() -> Keymap {
        // Keycode 8 is `a`/`A`, keycode 9 is Shift, keycode 10 is space.
        Keymap::new(8, 2, vec![0x61, 0x41, 0xffe1, 0xffe1, 0x20, 0x20])
    }

    /// A bitmap with the named keycodes held down.
    fn bitmap(keycodes: &[u8]) -> [u8; 32] {
        let mut bits = [0u8; 32];
        for keycode in keycodes {
            bits[usize::from(*keycode) / 8] |= 1 << (*keycode % 8);
        }
        bits
    }

    #[test]
    fn test_modifier_masks_match_the_x11_definitions() {
        assert_eq!(SHIFT_MASK, 0x0001);
        assert_eq!(CONTROL_MASK, 0x0004);
        assert_eq!(MOD1_MASK, 0x0008);
        assert_eq!(MOD4_MASK, 0x0040);
        assert_eq!(HOLDABLE_MASKS, 0x004d);
        let union = HOLDABLE_ORDER.iter().fold(0u16, |all, mask| all | mask);
        assert_eq!(
            union, HOLDABLE_MASKS,
            "the press order covers every holdable mask"
        );
    }

    #[test]
    fn test_keysym_table_matches_the_hosts_vocabulary() {
        // The values the engine's routing table names, taken from the installed
        // `fcitx-utils/keysymgen.h`; a drift here would inject keys the IME ignores.
        assert_eq!(KS_SPACE, 0x0020);
        assert_eq!(KS_MINUS, 0x002d);
        assert_eq!(KS_EQUAL, 0x003d);
        assert_eq!(KS_BACKSPACE, 0xff08);
        assert_eq!(KS_TAB, 0xff09);
        assert_eq!(KS_RETURN, 0xff0d);
        assert_eq!(KS_ESCAPE, 0xff1b);
        assert_eq!(
            [KS_LEFT, KS_UP, KS_RIGHT, KS_DOWN],
            [0xff51, 0xff52, 0xff53, 0xff54]
        );
        assert_eq!(KS_SHIFT_L, 0xffe1);
        assert_eq!(KS_CONTROL_L, 0xffe3);
        assert_eq!(KS_ALT_L, 0xffe9);
        assert_eq!(KS_SUPER_L, 0xffeb);
    }

    #[test]
    fn test_modifier_keysym_names_the_four_holdable_masks() {
        assert_eq!(modifier_keysym(SHIFT_MASK), Some(KS_SHIFT_L));
        assert_eq!(modifier_keysym(CONTROL_MASK), Some(KS_CONTROL_L));
        assert_eq!(modifier_keysym(MOD1_MASK), Some(KS_ALT_L));
        assert_eq!(modifier_keysym(MOD4_MASK), Some(KS_SUPER_L));
        // A latch is a state, not a key this channel presses: LockMask is 0x0002 and
        // Mod2Mask, the NumLock bit, is 0x0010.
        assert_eq!(modifier_keysym(0x0002), None);
        assert_eq!(modifier_keysym(0x0010), None);
        assert_eq!(modifier_keysym(0), None);
    }

    #[test]
    fn test_named_keysym_covers_the_navigation_and_modifier_keys() {
        assert_eq!(named_keysym("tab"), Some(KS_TAB));
        assert_eq!(named_keysym(" Tab "), Some(KS_TAB));
        assert_eq!(named_keysym("RETURN"), Some(KS_RETURN));
        assert_eq!(named_keysym("enter"), Some(KS_RETURN));
        assert_eq!(named_keysym("escape"), Some(KS_ESCAPE));
        assert_eq!(named_keysym("backspace"), Some(KS_BACKSPACE));
        assert_eq!(named_keysym("left"), Some(KS_LEFT));
        assert_eq!(named_keysym("up"), Some(KS_UP));
        assert_eq!(named_keysym("right"), Some(KS_RIGHT));
        assert_eq!(named_keysym("down"), Some(KS_DOWN));
        assert_eq!(named_keysym("space"), Some(KS_SPACE));
        assert_eq!(named_keysym("shift"), Some(KS_SHIFT_L));
        assert_eq!(named_keysym("ctrl"), Some(KS_CONTROL_L));
        assert_eq!(named_keysym("alt"), Some(KS_ALT_L));
        assert_eq!(named_keysym("super"), Some(KS_SUPER_L));
        assert_eq!(named_keysym("meta"), None, "an unlisted name is refused");
        assert_eq!(named_keysym(""), None);
    }

    #[test]
    fn test_modifier_mask_combines_names_and_refuses_a_bad_one() {
        assert_eq!(modifier_mask(""), Ok(0));
        assert_eq!(modifier_mask("shift"), Ok(SHIFT_MASK));
        assert_eq!(modifier_mask("shift,ctrl"), Ok(SHIFT_MASK | CONTROL_MASK));
        assert_eq!(
            modifier_mask(" SHIFT , control "),
            Ok(SHIFT_MASK | CONTROL_MASK)
        );
        assert_eq!(modifier_mask("alt,super"), Ok(MOD1_MASK | MOD4_MASK));
        let refusal = modifier_mask("shift,hyper").expect_err("hyper is not a holdable modifier");
        assert!(refusal.contains("hyper"), "{refusal}");
    }

    #[test]
    fn test_stroke_for_char_covers_letters_digits_and_punctuation() {
        let plain = |keysym| KeyStroke { keysym, state: 0 };
        let shifted = |keysym| KeyStroke {
            keysym,
            state: SHIFT_MASK,
        };
        assert_eq!(stroke_for_char('n'), Some(plain(0x006e)));
        assert_eq!(stroke_for_char('N'), Some(shifted(0x006e)));
        assert_eq!(stroke_for_char('5'), Some(plain(0x0035)));
        assert_eq!(stroke_for_char('-'), Some(plain(KS_MINUS)));
        assert_eq!(stroke_for_char('='), Some(plain(KS_EQUAL)));
        assert_eq!(stroke_for_char(' '), Some(plain(KS_SPACE)));
        assert_eq!(stroke_for_char('\''), Some(plain(0x0027)));
        assert_eq!(stroke_for_char('_'), Some(shifted(KS_MINUS)));
        assert_eq!(stroke_for_char('?'), Some(shifted(0x002f)));
        assert_eq!(
            stroke_for_char('中'),
            None,
            "the harness types pinyin, not Chinese"
        );
    }

    #[test]
    fn test_strokes_lists_one_stroke_per_character_and_refuses_others() {
        let typed = strokes("ni hao").expect("every character of the sample has a key");
        assert_eq!(typed.len(), 6, "one stroke per character, spaces included");
        assert_eq!(typed[0].keysym, 0x006e);
        assert_eq!(typed[1].keysym, 0x0069);
        assert_eq!(typed[2].keysym, KS_SPACE);
        assert_eq!(typed[3].keysym, 0x0068);

        let refusal = strokes("nǐ").expect_err("a tone-marked vowel has no key");
        assert!(refusal.to_string().contains('ǐ'), "{refusal}");
        assert!(
            strokes("中国").is_err(),
            "the harness types pinyin, not Chinese"
        );
    }

    #[test]
    fn test_keymap_lookup_finds_the_row_of_a_keysym() {
        let map = keymap();
        assert_eq!(map.keycode(0x61), Some(8), "the lowercase form of row 8");
        assert_eq!(
            map.keycode(0x41),
            Some(8),
            "the uppercase form of the same row"
        );
        assert_eq!(map.keycode(KS_SHIFT_L), Some(9));
        assert_eq!(map.keycode(0x20), Some(10));
        assert_eq!(map.keycode(0x62), None, "a keysym no row carries");
        assert_eq!(Keymap::new(8, 0, Vec::new()).keycode(0x61), None);
    }

    #[test]
    fn test_keymap_identifies_modifier_rows() {
        let map = keymap();
        assert!(map.is_modifier_keycode(9), "row 9 carries Shift_L");
        assert!(!map.is_modifier_keycode(8), "row 8 carries letters");
        assert!(
            !map.is_modifier_keycode(200),
            "a keycode outside the mapping"
        );
        assert!(is_modifier_keysym(0xffe3) && is_modifier_keysym(0xfe03));
        assert!(!is_modifier_keysym(0x61));
    }

    #[test]
    fn test_pressed_keycodes_decodes_the_query_bitmap() {
        assert!(pressed_keycodes(&bitmap(&[])).is_empty());
        assert_eq!(pressed_keycodes(&bitmap(&[50])), vec![50]);
        assert_eq!(pressed_keycodes(&bitmap(&[50, 8])), vec![8, 50]);
        assert_eq!(pressed_keycodes(&bitmap(&[255])), vec![255]);
    }

    #[test]
    fn test_pressed_modifiers_keeps_only_the_modifier_rows() {
        let map = keymap();
        assert_eq!(pressed_modifiers(&bitmap(&[8, 9, 10]), &map), vec![9]);
        assert!(pressed_modifiers(&bitmap(&[8, 10]), &map).is_empty());
    }
}
