//! The text around the caret: the token under it and the quote it leaves open.
//!
//! Responsibility: cut the surrounding text down to the part a decision may look at --
//! the last [`SURROUNDING_TAIL_BYTES`] bytes, taken on a character boundary -- read the
//! run of non-whitespace characters that ends there, and answer whether that text leaves
//! an opening Chinese quote unpaired.
//!
//! Boundaries: this module only reads the string the host reported. It holds no state, no
//! clock and no allocation, and it decides nothing: which rule consults the token, and
//! what a URL or email marker looks like, are the parent's business.

use super::SURROUNDING_TAIL_BYTES;

/// The run of non-whitespace characters that ends the last
/// [`SURROUNDING_TAIL_BYTES`] bytes of `text`.
///
/// Only this token counts: text further back is history, and treating it as evidence
/// would keep the plugin out of the way long after the user left the address. The
/// scan walks characters, so a multi-byte space cannot split a character.
pub(super) fn trailing_token(text: &str) -> &str {
    let tail = tail_of(text);
    let mut start = 0usize;
    for (index, ch) in tail.char_indices() {
        if ch.is_whitespace() {
            start = index + ch.len_utf8();
        }
    }
    &tail[start..]
}

/// The last [`SURROUNDING_TAIL_BYTES`] bytes of `text`, cut on a character
/// boundary.
fn tail_of(text: &str) -> &str {
    if text.len() <= SURROUNDING_TAIL_BYTES {
        return text;
    }
    let mut start = text.len() - SURROUNDING_TAIL_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// Whether the text before the caret leaves an opening Chinese quote unpaired, which
/// is when the next `"` closes rather than opens. Without surrounding text there is
/// no evidence either way and the opening form is used, the conservative branch.
pub(super) fn has_unclosed_quote(surrounding: Option<&str>) -> bool {
    let text = surrounding.unwrap_or("");
    let mut depth = 0i32;
    for ch in tail_of(text).chars() {
        match ch {
            '“' => depth += 1,
            '”' => depth -= 1,
            _ => {}
        }
    }
    depth > 0
}
