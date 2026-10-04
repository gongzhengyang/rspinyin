//! The Chinese punctuation rule: the substitution table and the full-width mapping.
//!
//! Responsibility: answer what a key press that produced one ASCII mark commits -- the
//! Chinese mark the table names, or nothing when the mode leaves the mark to the
//! application -- and widen one character to its full-width form, which is how the
//! substituted marks and the leading uppercase letter reach the screen. The same two
//! mappings, applied over whole committed text, are the output half of the policy:
//! [`transform_committed`] is what a host calls on the one string that leaves for the
//! application.
//!
//! Boundaries: this module owns the table, the mode and the mapping, and nothing else. It
//! never sees the whole input, never orders the rules (the parent applies them in order)
//! and never allocates more than the one-character payload a decision carries. Pairing a
//! double quote asks [`has_unclosed_quote`] instead of reading the text around the caret
//! itself, so every scan of that text lives in one place.

use std::borrow::Cow;

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

/// Rewrites one piece of committed text by the output half of the passthrough policy.
///
/// This is what the full-width switch and the punctuation mode mean once the plugin has
/// text on its way out: with the punctuation mode on Chinese, every ASCII mark the table
/// names is replaced with the mark it commits on a keystroke; with the full-width flag
/// on, every remaining printable ASCII character -- the letters, digits and space a raw
/// commit is made of -- is widened. Chinese text and the already substituted marks pass
/// both mappings unchanged, which is what makes applying this to a decoded candidate a
/// no-op rather than a rewrite.
///
/// The two flags are the plugin's live mode bits rather than a [`PassthroughFlags`],
/// because the switches are the engine's to flip while it runs; the flags struct carries
/// the configuration's *initial* values, and this function must follow the switches.
///
/// # Parameters
///
/// - `text`: the text a commit is about to hand to the application.
/// - `punct_chinese`: whether the plugin's punctuation output is Chinese
///   (`[engine] punct_mode` "chinese", the live switch's position).
/// - `full_width`: whether the plugin's ASCII output is written full width
///   (`[engine] full_width`, the live switch's position).
///
/// # Returns
///
/// The text to commit. It borrows `text` whenever no character changes -- the whole hot
/// path, a commit of Chinese candidates included -- so a commit that the mode bits cannot
/// alter allocates nothing.
///
/// # Panics
///
/// Never: the body is a per-character table lookup over `text`'s own `char_indices`, and
/// the slice split before the first divergence is on a boundary `char_indices` produced.
///
/// # Examples
///
/// ```
/// use std::borrow::Cow;
///
/// use ime_core::passthrough::transform_committed;
///
/// // Full width widens the plugin's own ASCII output ...
/// assert_eq!(
///     transform_committed("nihao", false, true).as_ref(),
///     "ｎｉｈａｏ"
/// );
/// // ... and leaves text that has no ASCII in it borrowed, so the hot path is free.
/// assert_eq!(
///     transform_committed("你好", false, true),
///     Cow::Borrowed("你好")
/// );
/// // Chinese punctuation substitutes the marks the table names.
/// assert_eq!(
///     transform_committed("a,b", true, false).as_ref(),
///     "a，b"
/// );
/// // With both switches off the text is the application's, unchanged.
/// assert_eq!(
///     transform_committed("a,b", false, false),
///     Cow::Borrowed("a,b")
/// );
/// ```
pub fn transform_committed<'a>(
    text: &'a str,
    punct_chinese: bool,
    full_width: bool,
) -> Cow<'a, str> {
    if !punct_chinese && !full_width {
        return Cow::Borrowed(text);
    }
    // The rewrite allocates at most once, and only when a character actually changes:
    // until the first divergence the borrow stands, and everything the divergence
    // leaves behind is copied in one `String::from` rather than pushed char by char.
    let mut rewritten: Option<String> = None;
    for (index, ch) in text.char_indices() {
        let substituted = if punct_chinese {
            punctuation_target(ch).unwrap_or(ch)
        } else {
            ch
        };
        let widened = if full_width {
            to_full_width(substituted)
        } else {
            substituted
        };
        if widened == ch {
            if let Some(out) = rewritten.as_mut() {
                out.push(ch);
            }
        } else {
            let out = rewritten.get_or_insert_with(|| String::from(&text[..index]));
            out.push(widened);
        }
    }
    match rewritten {
        Some(out) => Cow::Owned(out),
        None => Cow::Borrowed(text),
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{PUNCTUATION_TABLE, transform_committed};

    /// The ASCII text every case below rewrites, chosen so that letters, a digit, a
    /// space and a mark of the table all appear in one string.
    const SAMPLE: &str = "ni hao, 3 ok";

    #[test]
    fn test_transform_committed_with_both_switches_off_borrows_the_text() {
        assert_eq!(
            transform_committed(SAMPLE, false, false),
            Cow::Borrowed(SAMPLE),
            "the switches off leave every character, and the allocation, out"
        );
        assert_eq!(transform_committed("", false, false), Cow::Borrowed(""));
    }

    #[test]
    fn test_transform_committed_full_width_widens_ascii_and_spares_cjk() {
        let out = transform_committed("nihao 3", false, true);
        assert_eq!(
            out.as_ref(),
            "ｎｉｈａｏ　３",
            "every printable ASCII widens"
        );
        // The ideographic space is what the ASCII space widened into.
        assert!(matches!(out, Cow::Owned(_)));
        // CJK text has no printable ASCII in it, so the borrow survives the flag.
        assert_eq!(
            transform_committed("你好", false, true),
            Cow::Borrowed("你好"),
            "already full-width text is not rewritten"
        );
    }

    #[test]
    fn test_transform_committed_chinese_punct_substitutes_every_mark_the_table_commits() {
        // One committed character per table row, in the table's own order, so the
        // substitution and the table cannot drift apart silently. The apostrophe
        // carries `None`: it is the syllable separator, deliberately not a mark the
        // output rewrites.
        let input: String = PUNCTUATION_TABLE.iter().map(|(mark, _)| *mark).collect();
        let expected: String = PUNCTUATION_TABLE
            .iter()
            .map(|(mark, target)| target.unwrap_or(*mark))
            .collect();
        assert_eq!(
            transform_committed(&input, true, false).as_ref(),
            expected,
            "the eleven committed marks substitute and the apostrophe stands"
        );
        assert_eq!(
            transform_committed("a,b", true, false).as_ref(),
            "a，b",
            "a mark inside ordinary text is substituted where it stands"
        );
    }

    #[test]
    fn test_transform_committed_english_punct_keeps_marks_that_full_width_then_widens() {
        // English punctuation mode leaves the ASCII marks alone ...
        assert_eq!(
            transform_committed("a,b", false, false),
            Cow::Borrowed("a,b"),
            "and with full width off too, the text is borrowed whole"
        );
        // ... but the full-width switch is its own axis: with it on, every printable
        // ASCII widens, the marks included -- the mark and its widened form are the same
        // character the substitution would have produced.
        assert_eq!(
            transform_committed("a,b", false, true).as_ref(),
            "ａ，ｂ",
            "the widened mark and the substituted one are the same character"
        );
    }

    #[test]
    fn test_transform_committed_substitution_then_widening_leaves_marks_full_width() {
        // Chinese punctuation plus full width is the shipped switched-on state: the
        // substitution lands first, and widening covers every printable ASCII the table
        // did not substitute -- letters included, which is the mapping features.md 3.4
        // freezes (`0x21..=0x7E` -> `0xFF01..=0xFF5E`). The two switches compose because
        // widening a mark that is already full width changes nothing.
        assert_eq!(transform_committed("a,b", true, true).as_ref(), "ａ，ｂ");
    }

    #[test]
    fn test_transform_committed_twice_is_the_same_as_once() {
        let once = transform_committed(SAMPLE, true, true);
        let twice = transform_committed(once.as_ref(), true, true);
        assert_eq!(
            twice.as_ref(),
            once.as_ref(),
            "the mappings are idempotent, so a rewritten text is stable"
        );
    }

    #[test]
    fn test_transform_committed_partial_rewrite_keeps_the_untouched_head() {
        // The rewrite starts at the first character that changes; everything before it
        // must survive byte for byte, which is what the borrowed-head copy guards.
        let out = transform_committed("ab,c", true, false);
        assert_eq!(out.as_ref(), "ab，c");
        assert!(matches!(out, Cow::Owned(_)));
    }
}
