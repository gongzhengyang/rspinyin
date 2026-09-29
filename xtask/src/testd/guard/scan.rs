//! The source lexer the guard counts with.
//!
//! Split out of `guard.rs` to keep that file inside the line limit. It answers one question --
//! how many `#[test]` attributes and how many assertions a source file really contains -- and
//! it answers it by walking the bytes rather than by matching lines, because the two things
//! that would defeat a line match are exactly the two the guard exists to catch: an attribute
//! quoted inside a string, and one commented out.
//!
//! Nothing here reads a file or a path; the text arrives as a `&str` and the counts leave as a
//! value, which is what lets every rule be asserted against a fixture.

use super::ASSERTION_MACROS;

/// What one source file holds, as the guard counts it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceCounts {
    /// `#[test]` attributes.
    pub tests: usize,
    /// Invocations of the macros in [`ASSERTION_MACROS`].
    pub assertions: usize,
}

/// Counts the tests and assertions in `source`.
///
/// Comments and string literals are skipped, and that is the point of the scanner rather than an
/// optimisation: the cheapest way to fake a count is to write the attribute into a comment, so a
/// count that reads raw text is a count a pass can inflate while it deletes the tests the count
/// exists to protect. Character literals need no state: one holds a single character, so it can
/// never contain the two-character sequences `//`, `/*` or `*/`.
///
/// # Panics
///
/// Never.
pub fn scan_source(source: &str) -> SourceCounts {
    let bytes = source.as_bytes();
    let mut counts = SourceCounts::default();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            index = after_line_comment(bytes, index);
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index = after_block_comment(bytes, index);
        } else if byte == b'"' {
            index = after_string(bytes, index);
        } else if byte == b'#' && bytes.get(index + 1) == Some(&b'[') {
            index = after_attribute(source, bytes, index, &mut counts);
        } else if is_identifier_byte(byte) && !is_identifier_byte(before(bytes, index)) {
            index = after_word(source, bytes, index, &mut counts);
        } else {
            index += 1;
        }
    }
    counts
}

/// The byte before `index`, or zero at the start of the source, which is not an identifier byte.
fn before(bytes: &[u8], index: usize) -> u8 {
    index
        .checked_sub(1)
        .and_then(|earlier| bytes.get(earlier))
        .copied()
        .unwrap_or(0)
}

/// Whether `byte` can appear in an identifier.
fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The index just past the line comment that starts at `index`.
fn after_line_comment(bytes: &[u8], index: usize) -> usize {
    bytes
        .get(index..)
        .and_then(|rest| rest.iter().position(|byte| *byte == b'\n'))
        .map_or(bytes.len(), |offset| index + offset + 1)
}

/// The index just past the block comment that starts at `index`.
///
/// Rust's block comments nest, so the scan counts depth rather than stopping at the first `*/`.
fn after_block_comment(bytes: &[u8], index: usize) -> usize {
    let mut depth = 0usize;
    let mut at = index;
    while let Some(current) = bytes.get(at).copied() {
        let next = bytes.get(at + 1).copied();
        if current == b'/' && next == Some(b'*') {
            depth += 1;
            at += 2;
        } else if current == b'*' && next == Some(b'/') {
            depth = depth.saturating_sub(1);
            at += 2;
            if depth == 0 {
                return at;
            }
        } else {
            at += 1;
        }
    }
    bytes.len()
}

/// The index just past the string literal that starts at `index`.
///
/// The escape skips two bytes whatever follows it, so `\"` cannot close the literal. A raw string
/// needs no state of its own: its body is skipped from the first `"` to the next one, which is the
/// right answer for every raw string whose own text holds no quote -- including `r#"#[test]"#`, the
/// shape a count that could be inflated with one would use.
fn after_string(bytes: &[u8], index: usize) -> usize {
    let mut at = index + 1;
    while let Some(current) = bytes.get(at).copied() {
        match current {
            b'\\' => at += 2,
            b'"' => return at + 1,
            _ => at += 1,
        }
    }
    bytes.len()
}

/// The index just past the attribute that starts at `index`, counting it when it is `#[test]`.
///
/// An attribute that does not parse as one is left to the ordinary scan, which keeps a stray `#[`
/// from swallowing the rest of a file.
fn after_attribute(source: &str, bytes: &[u8], index: usize, counts: &mut SourceCounts) -> usize {
    let Some((word, end)) = word_at(source, bytes, skip_spaces(bytes, index + 2)) else {
        return index + 1;
    };
    let close = skip_spaces(bytes, end);
    if bytes.get(close) != Some(&b']') {
        return index + 1;
    }
    if word == "test" {
        counts.tests += 1;
    }
    close + 1
}

/// The index just past the word at `index`, counting it when it is one of the assertion macros.
fn after_word(source: &str, bytes: &[u8], index: usize, counts: &mut SourceCounts) -> usize {
    let Some((word, end)) = word_at(source, bytes, index) else {
        return index + 1;
    };
    if ASSERTION_MACROS.contains(&word) && bytes.get(skip_spaces(bytes, end)) == Some(&b'!') {
        counts.assertions += 1;
    }
    end
}

/// The identifier that starts at `start`, with the index just past it.
///
/// `None` when no identifier starts there. Every byte of an identifier is ASCII, so both offsets are
/// character boundaries of `source` and the slice cannot split a character.
fn word_at<'a>(source: &'a str, bytes: &[u8], start: usize) -> Option<(&'a str, usize)> {
    let mut end = start;
    while is_identifier_byte(bytes.get(end).copied().unwrap_or(0)) {
        end += 1;
    }
    if end == start {
        return None;
    }
    source.get(start..end).map(|word| (word, end))
}

/// The index of the first byte that is not a space, starting at `at`.
fn skip_spaces(bytes: &[u8], at: usize) -> usize {
    let mut index = at;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    index
}
