//! Reading one log line: the fields it carries, and the rules it has to satisfy.
//!
//! Responsibility: pull a named field's value out of a rendered log line, and apply the privacy
//! rules and the error-code rule to that line. Nothing here reads a file, and nothing here
//! decides whether a line is worth keeping.
//!
//! # What a rendered line looks like
//!
//! [`ime_diag::redact::RedactFormat`] writes one line per event: a timestamp, the level, the
//! emitting module, the message, and then the event's fields as `name=value` pairs.
//!
//! ```text
//! 2026-09-29T10:35:12.345678Z INFO ime_fcitx5::addon: session: start session=42 app=0x8f3a2c1d
//! ```
//!
//! A value is written as it stands, with newlines and carriage returns escaped and *spaces left
//! alone*, so a line cannot be split into fields by whitespace: `reason=not writable at all` is
//! one field whose value contains three words. This module therefore never tries to reconstruct
//! the field list. It looks for the fields it cares about by name, and it reads each value up to
//! the next space -- which is exact for every field it examines, because all of them hold a
//! placeholder, a hash, a code or a path, and none of those contains a space.
//!
//! # The rules
//!
//! | Rule | What it catches |
//! |---|---|
//! | [`Denial::Unredacted`] | a field on the redaction denylist written as it was, instead of as `<redacted:len=N>` |
//! | [`Denial::ClearAppIdentifier`] | an application identifier written in clear text instead of as a hash |
//! | [`Denial::UnshortenedHome`] | a home directory written in full instead of shortened to `~` |
//! | [`Denial::Literal`] | a substring the case itself asked to be absent |
//! | [`Denial::UnknownCode`] | a `code=` value outside the frozen error-code set |
//!
//! The first two read their definitions from [`ime_diag::redact`] rather than restating them:
//! the denylist is [`DENIED_FIELDS`], the placeholder is derived from
//! [`redaction_placeholder`], and the hash shape is the `0x` prefix the log's own documented
//! example writes. A rule copied out of that module would be a second place for the policy to
//! live, and the two would drift.

use std::path::Path;

use ime_diag::redact::{DENIED_FIELDS, redaction_placeholder};

use super::error::Denial;

/// The field an application identifier is logged under.
const APP_FIELD: &str = "app";

/// The field an error code is logged under.
const CODE_FIELD: &str = "code";

/// The prefix an application identifier's hash is written with.
const HASH_PREFIX: &str = "0x";

/// Every value the line writes for the field `name`.
///
/// A field is recognised by its name standing at the start of a field -- at the beginning of the
/// line or after a space -- followed by `=`. The boundary is what keeps `text=` from matching
/// the tail of `candidate_text=`, which is a different field with a different rule.
///
/// # Panics
///
/// Never.
pub fn field_values<'a>(line: &'a str, name: &str) -> Vec<&'a str> {
    let mut values = Vec::new();
    let mut rest = line;
    while let Some(at) = find_field(rest, name) {
        let tail = &rest[at + name.len() + 1..];
        let end = tail.find(' ').unwrap_or(tail.len());
        values.push(&tail[..end]);
        rest = &tail[end..];
    }
    values
}

/// Every `code=` value the line writes.
///
/// # Panics
///
/// Never.
pub fn codes_in(line: &str) -> Vec<&str> {
    field_values(line, CODE_FIELD)
}

/// Every rule the line breaks.
///
/// `home` is the directory whose prefix the log must rewrite to `~`, or `None` when the reader
/// could not resolve one -- in which case the home rule is not applied, because a rule that
/// guessed at the prefix would fail every line.
///
/// # Panics
///
/// Never.
pub fn violations(line: &str, home: Option<&Path>, literals: &[&str]) -> Vec<Denial> {
    let mut denials = Vec::new();
    let placeholder = placeholder_prefix();
    for field in DENIED_FIELDS {
        for value in field_values(line, field) {
            if !is_placeholder(value, &placeholder) {
                denials.push(Denial::Unredacted { field });
            }
        }
    }
    for value in field_values(line, APP_FIELD) {
        if !value.starts_with(HASH_PREFIX) {
            denials.push(Denial::ClearAppIdentifier);
        }
    }
    if let Some(home) = home.and_then(Path::to_str) {
        let home = home.trim_end_matches('/');
        if !home.is_empty() && line.contains(home) {
            denials.push(Denial::UnshortenedHome);
        }
    }
    for literal in literals {
        if !literal.is_empty() && line.contains(literal) {
            denials.push(Denial::Literal {
                pattern: (*literal).to_owned(),
            });
        }
    }
    denials
}

/// Every `code=` value the line writes that `known` does not declare.
///
/// # Panics
///
/// Never.
pub fn unknown_codes(line: &str, known: &[&str]) -> Vec<Denial> {
    codes_in(line)
        .into_iter()
        .filter(|code| !known.contains(code))
        .map(|code| Denial::UnknownCode {
            code: code.to_owned(),
        })
        .collect()
}

/// The offset of `name=` in `text`, where the name starts a field rather than continuing one.
///
/// # Panics
///
/// Never: the offsets are byte positions of an exact ASCII match, so every slice falls on a
/// character boundary.
fn find_field(text: &str, name: &str) -> Option<usize> {
    if name.is_empty() {
        // A search for the empty string matches everywhere and would advance by nothing.
        return None;
    }
    let mut from = 0;
    while let Some(found) = text[from..].find(name) {
        let at = from + found;
        let starts_field = match text[..at].chars().next_back() {
            None => true,
            Some(before) => before == ' ',
        };
        if starts_field && text[at + name.len()..].starts_with('=') {
            return Some(at);
        }
        from = at + name.len();
    }
    None
}

/// The text that precedes the length of a redaction placeholder.
///
/// Taken from the renderer rather than written out here: the placeholder for a withheld value of
/// length zero, cut at its first digit, is the prefix every placeholder starts with. A change to
/// the spelling in [`ime_diag::redact`] therefore moves this with it instead of leaving a rule
/// that accepts a form the redaction layer no longer writes.
///
/// # Panics
///
/// Never.
fn placeholder_prefix() -> String {
    let sample = redaction_placeholder(0);
    match sample.find(|ch: char| ch.is_ascii_digit()) {
        Some(at) => sample[..at].to_owned(),
        None => sample,
    }
}

/// Whether `value` is a redaction placeholder: the prefix, a count, and a closing bracket.
///
/// # Panics
///
/// Never.
fn is_placeholder(value: &str, prefix: &str) -> bool {
    let Some(rest) = value.strip_prefix(prefix) else {
        return false;
    };
    let Some(count) = rest.strip_suffix('>') else {
        return false;
    };
    !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit())
}
