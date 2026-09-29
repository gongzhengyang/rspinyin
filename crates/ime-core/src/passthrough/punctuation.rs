//! The Chinese punctuation rule: the substitution table and the full-width mapping.
//!
//! Responsibility: answer what a key press that produced one ASCII mark commits -- the
//! Chinese mark the table names, or nothing when the mode leaves the mark to the
//! application -- and widen one character to its full-width form, which is how the
//! substituted marks and the leading uppercase letter reach the screen.
//!
//! Boundaries: this module owns the table, the mode and the mapping, and nothing else. It
//! never sees the whole input, never orders the rules (the parent applies them in order)
//! and never allocates more than the one-character payload a decision carries. Pairing a
//! double quote asks [`has_unclosed_quote`] instead of reading the text around the caret
//! itself, so every scan of that text lives in one place.

use ime_types::ConfigError;

use super::surrounding::has_unclosed_quote;
use super::{PassthroughDecision, PassthroughFlags};

/// Widest UTF-8 encoding of a single character, in bytes.
///
/// [`PassthroughDecision::CommitDirectly`] always carries exactly one character,
/// so a buffer of this size never has to grow.
const MAX_UTF8_LEN: usize = 4;

/// Offset between a printable ASCII character and its full-width counterpart:
/// `0x21` maps to `0xFF01`, and `0x7E` to `0xFF5E`.
pub(super) const FULL_WIDTH_OFFSET: u32 = 0xFEE0;

/// Number of ASCII marks the Chinese punctuation table covers.
pub const PUNCTUATION_COUNT: usize = 12;

/// The Chinese punctuation table: one entry per ASCII mark the design names.
///
/// A `const` array indexed by the `match` in `punctuation_target` rather than a map,
/// because the lookup is a jump over twelve fixed keys: hashing, and the allocation
/// of a lazily built map on first use, would spend the 500ns budget on nothing.
///
/// The apostrophe carries `None`: in Chinese mode it is the pinyin syllable
/// separator and not a quotation mark, so it is deliberately not substituted, and
/// the input normalizer consumes it before the decoder sees the buffer. It stays in
/// the table so the omission reads as a decision rather than a missing case.
pub(super) const PUNCTUATION_TABLE: [(char, Option<char>); PUNCTUATION_COUNT] = [
    (',', Some('，')),
    ('.', Some('。')),
    (';', Some('；')),
    (':', Some('：')),
    ('?', Some('？')),
    ('!', Some('！')),
    ('(', Some('（')),
    (')', Some('）')),
    ('[', Some('【')),
    (']', Some('】')),
    ('"', Some('“')),
    ('\'', None),
];

/// The Chinese mark that replaces one ASCII mark: `Some(mark)` for the eleven
/// substituted marks, and `None` for everything else, the apostrophe included.
///
/// The `match` is the table's index lookup, and it sits next to the table so that
/// reordering one without the other is caught by the test rather than changing what
/// a keystroke commits. The index is taken with `get`, so an out-of-range index
/// answers `None` instead of panicking.
pub(super) fn punctuation_target(ch: char) -> Option<char> {
    let index = match ch {
        ',' => 0,
        '.' => 1,
        ';' => 2,
        ':' => 3,
        '?' => 4,
        '!' => 5,
        '(' => 6,
        ')' => 7,
        '[' => 8,
        ']' => 9,
        '"' => 10,
        '\'' => 11,
        _ => return None,
    };
    PUNCTUATION_TABLE.get(index).and_then(|(_, target)| *target)
}

/// Widens one printable ASCII character to its full-width form.
///
/// The graphic ASCII characters, `0x21..=0x7E`, map onto `0xFF01..=0xFF5E`, and the
/// ASCII space maps onto the ideographic space `U+3000`. Every other character is
/// returned unchanged -- including the Chinese punctuation the substitution table
/// produces, which is already full width -- so applying this twice is the same as
/// applying it once.
///
/// # Parameters
///
/// - `ch`: the character to widen.
///
/// # Returns
///
/// The full-width form of `ch`, or `ch` itself when it has none.
///
/// # Panics
///
/// Never: the only arithmetic is an addition on a value below `0x7F`, and the
/// result is converted with `char::from_u32`, which answers `None` rather than
/// panicking on an invalid scalar value.
///
/// # Examples
///
/// ```
/// use ime_core::passthrough::to_full_width;
///
/// assert_eq!(to_full_width('a'), 'ａ');
/// assert_eq!(to_full_width(' '), '\u{3000}');
/// // Already full width, or not ASCII at all: unchanged.
/// assert_eq!(to_full_width('，'), '，');
/// assert_eq!(to_full_width('你'), '你');
/// ```
pub fn to_full_width(ch: char) -> char {
    if ch == ' ' {
        return '\u{3000}';
    }
    if !ch.is_ascii_graphic() {
        return ch;
    }
    char::from_u32(ch as u32 + FULL_WIDTH_OFFSET).unwrap_or(ch)
}

/// How Chinese punctuation substitution is configured.
///
/// Mirrors the `[engine] punct_mode` key, whose two documented values are
/// `"chinese"` (the default) and `"english"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PunctMode {
    /// ASCII punctuation is replaced with the Chinese mark the table names.
    #[default]
    Chinese,
    /// ASCII punctuation is left to the application, which inserts the ASCII mark
    /// itself; the plugin consumes nothing.
    English,
}

impl TryFrom<&str> for PunctMode {
    type Error = ConfigError;

    /// Parses the `[engine] punct_mode` value.
    ///
    /// # Parameters
    ///
    /// - `value`: the raw configuration string.
    ///
    /// # Returns
    ///
    /// The mode the value names.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Invalid`] naming the `engine.punct_mode` key when the value is
    /// neither of the two documented spellings; the loader turns that into
    /// `config/invalid`, keeps the default and does not fail startup.
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "chinese" => Ok(Self::Chinese),
            "english" => Ok(Self::English),
            other => Err(ConfigError::Invalid {
                key: String::from("engine.punct_mode"),
                reason: format!("unknown punctuation mode: {other}"),
            }),
        }
    }
}

/// The decision for a key press whose text may be one Chinese punctuation mark,
/// with `flags` read for the punctuation mode and the full-width flag, and
/// `surrounding` used to pair the double quote.
///
/// Returns `None` when `raw` is not a lone mark of the table, so that the caller
/// falls through to decoding.
pub(super) fn punctuation_decision(
    raw: &str,
    flags: PassthroughFlags,
    surrounding: Option<&str>,
) -> Option<PassthroughDecision> {
    let mut chars = raw.chars();
    let mark = chars.next()?;
    // Substitution applies to a lone mark. A longer input is pinyin that happens to
    // contain punctuation, and the normalizer reports the stray character instead
    // of the plugin quietly committing it.
    if chars.next().is_some() {
        return None;
    }
    let target = punctuation_target(mark)?;
    if flags.punct_mode == PunctMode::English {
        // English punctuation mode leaves the ASCII mark to the application, which
        // is the only way it reaches the screen unaltered: the plugin commits
        // nothing here, so full-width output does not apply either.
        return Some(PassthroughDecision::HostHandles);
    }
    // The design lists both quote forms for `"`; which one applies depends on the
    // text before the caret, because a pure function has no memory of the previous
    // keystrokes.
    let target = if mark == '"' && has_unclosed_quote(surrounding) {
        '”'
    } else {
        target
    };
    Some(PassthroughDecision::CommitDirectly(commit_text(
        target,
        flags.full_width,
    )))
}

/// The one-character payload of [`PassthroughDecision::CommitDirectly`].
///
/// The buffer is sized for the widest UTF-8 encoding of a single character, so the
/// decision allocates once and never reallocates.
pub(super) fn commit_text(ch: char, full_width: bool) -> String {
    let ch = if full_width { to_full_width(ch) } else { ch };
    let mut text = String::with_capacity(MAX_UTF8_LEN);
    text.push(ch);
    text
}
