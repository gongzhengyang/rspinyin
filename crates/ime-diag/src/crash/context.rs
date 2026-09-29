//! What a crash record is allowed to say about the session.
//!
//! Responsibility: hold the closed set of structural facts a record may carry, and
//! check every value that goes into one.
//!
//! Boundaries: this module knows nothing about files, threads, signals or the clock. It
//! is the privacy half of the crash path on its own, which is what makes it testable
//! without any of them.
//!
//! # Why a whitelist and not a denylist
//!
//! A crash record is the easiest place to leak a user's keystrokes by accident: a
//! session dump that quotes the input buffer would put what the user typed into a file
//! that outlives the session. The log layer's denylist names the fields that must never
//! be written; here the set is closed instead, because a record has no legitimate reason
//! to hold anything else. There is no key for the input buffer, the preedit, a candidate
//! or a commit string, so no code path can attach one -- and a diagnostic that needs a
//! new fact has to add a variant, which puts the decision in front of a reviewer.
//!
//! The value shapes are checked for the same reason. A count is a number; a name is a
//! short identifier drawn from a small alphabet. A sentence from the input buffer is
//! neither, so it is refused rather than truncated.

use std::collections::BTreeMap;

/// Longest identifier a context value may be, in characters.
pub const MAX_CONTEXT_VALUE_CHARS: usize = 64;

/// The structural facts a crash record may carry.
///
/// The set is closed on purpose. A diagnostic that needs a new fact adds a variant
/// here, which puts the addition in front of a reviewer instead of letting a free-form
/// key slip into a record unnoticed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CrashContextKey {
    /// The state the input session was in when the crash happened.
    SessionState,
    /// The revision of the session or frame the crash interrupted.
    Revision,
    /// The platform backend that was active.
    BackendId,
    /// How many characters the input buffer held. The count, never the characters.
    RawLen,
    /// How many candidates the last decode produced.
    CandidateCount,
}

impl CrashContextKey {
    /// Every key, so that a check can be applied to all of them.
    pub const ALL: [Self; 5] = [
        Self::SessionState,
        Self::Revision,
        Self::BackendId,
        Self::RawLen,
        Self::CandidateCount,
    ];

    /// The name the key is written under.
    ///
    /// # Returns
    ///
    /// The lower-snake-case name of the fact.
    ///
    /// # Panics
    ///
    /// Never: a total match over five variants.
    pub fn name(self) -> &'static str {
        match self {
            Self::SessionState => "session_state",
            Self::Revision => "revision",
            Self::BackendId => "backend_id",
            Self::RawLen => "raw_len",
            Self::CandidateCount => "candidate_count",
        }
    }
}

/// The structural facts attached to one crash record.
///
/// The fields are private and the two insertions check their value, so the type cannot
/// hold anything but the facts its keys name. That is the whole point: a caller that
/// would like to record the input buffer has no method to do it with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CrashContext {
    /// Keyed by the written name, so the order is the sorted one.
    fields: BTreeMap<&'static str, String>,
}

impl CrashContext {
    /// Creates an empty context.
    ///
    /// # Returns
    ///
    /// A context with no facts in it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a count under `key`.
    ///
    /// # Parameters
    ///
    /// - `key`: the fact being recorded.
    /// - `value`: the number itself. A count of characters is a structural fact; the
    ///   characters are not.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn insert_count(&mut self, key: CrashContextKey, value: u64) {
        self.fields.insert(key.name(), value.to_string());
    }

    /// Records a short identifier under `key`.
    ///
    /// The value must be an identifier -- ASCII letters and digits, plus `_`, `-`, `.`
    /// and `:` -- and at most [`MAX_CONTEXT_VALUE_CHARS`] characters. Anything else is
    /// refused rather than truncated: a value that is not an identifier is not a
    /// structural fact, and there is no way to tell afterwards what it was.
    ///
    /// # Parameters
    ///
    /// - `key`: the fact being recorded.
    /// - `value`: the identifier.
    ///
    /// # Returns
    ///
    /// `true` when the value was recorded, `false` when it was refused.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn insert_identifier(&mut self, key: CrashContextKey, value: &str) -> bool {
        if !is_identifier(value) {
            return false;
        }
        self.fields.insert(key.name(), value.to_owned());
        true
    }

    /// The value recorded under `key`.
    ///
    /// # Returns
    ///
    /// The value, or `None` when nothing was recorded under that key.
    pub fn get(&self, key: CrashContextKey) -> Option<&str> {
        self.fields.get(key.name()).map(String::as_str)
    }

    /// Every fact recorded, ordered by name.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &str)> {
        self.fields
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
    }

    /// Whether the context holds nothing.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// Whether `value` has the shape of an identifier.
///
/// The shape check is what makes a value safe to write next to a key: an identifier
/// cannot carry a newline, a control character, or a sentence from the input buffer.
fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_CONTEXT_VALUE_CHARS
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crash_context_refuses_a_value_that_is_not_an_identifier() {
        let mut context = CrashContext::new();
        assert!(
            !context.insert_identifier(CrashContextKey::BackendId, "ni hao"),
            "a value with a space is not a structural fact"
        );
        assert!(context.get(CrashContextKey::BackendId).is_none());
        assert!(
            !context.insert_identifier(CrashContextKey::BackendId, "nihao\nrevision=99"),
            "a value must not be able to forge a record line"
        );
        assert!(context.insert_identifier(CrashContextKey::BackendId, "wayland/layer-shell"));
        assert_eq!(context.get(CrashContextKey::BackendId), Some("wayland/layer-shell"));
    }

    #[test]
    fn test_crash_context_refuses_an_over_long_identifier() {
        let mut context = CrashContext::new();
        let long = "a".repeat(MAX_CONTEXT_VALUE_CHARS + 1);
        assert!(!context.insert_identifier(CrashContextKey::SessionState, &long));
        let at_limit = "a".repeat(MAX_CONTEXT_VALUE_CHARS);
        assert!(context.insert_identifier(CrashContextKey::SessionState, &at_limit));
        assert!(
            !context.insert_identifier(CrashContextKey::SessionState, ""),
            "an empty identifier says nothing"
        );
    }

    #[test]
    fn test_crash_context_insert_count_writes_the_number() {
        let mut context = CrashContext::new();
        context.insert_count(CrashContextKey::RawLen, 6);
        context.insert_count(CrashContextKey::CandidateCount, 0);
        assert_eq!(context.get(CrashContextKey::RawLen), Some("6"));
        assert_eq!(context.get(CrashContextKey::CandidateCount), Some("0"));
        assert!(!context.is_empty());
    }

    #[test]
    fn test_crash_context_keys_cover_the_documented_names() {
        let names: Vec<&str> = CrashContextKey::ALL.iter().map(|key| key.name()).collect();
        assert_eq!(
            names,
            vec![
                "session_state",
                "revision",
                "backend_id",
                "raw_len",
                "candidate_count"
            ]
        );
        // There is deliberately no key for the input itself; this is the assertion that
        // would fail first if somebody added one.
        for denied in [
            "raw",
            "text",
            "preedit",
            "input",
            "candidate_text",
            "commit_text",
        ] {
            assert!(!names.contains(&denied), "{denied} must have no key");
        }
    }

    #[test]
    fn test_crash_context_iter_is_ordered_by_name() {
        let mut context = CrashContext::new();
        context.insert_count(CrashContextKey::Revision, 12);
        context.insert_identifier(CrashContextKey::SessionState, "composing");

        let names: Vec<&str> = context.iter().map(|(name, _)| name).collect();

        assert_eq!(names, vec!["revision", "session_state"]);
    }
}
