//! The commit-side bridge: the one space between the text on the screen and the
//! text an English commit is about to add to it.
//!
//! Responsibility: decide, from the character classes of the last landed commit and
//! of the commit being produced, whether a single ASCII space belongs between them,
//! and dress the commit text with it. The rule table is the card's: Chinese into
//! English and English into English take the space; every other pairing -- English
//! into Chinese included -- and every pairing whose either side has no single class
//! takes nothing. The detection is O(text length) over the two texts and unknown
//! classes answer no insertion, which is the fail-safe the mixed `X光`-form entries
//! live under: a text that is part Han and part alphanumeric names no class, so no
//! space is ever inserted beside it.
//!
//! The casing half of the commit dressing lives here too, because the two transforms
//! apply at the same door: an English candidate's commit text takes the casing form
//! the raw input was typed in ([`crate::segment::english`]). Both transforms are
//! output-side only -- the candidate list the window shows, the case-folded match
//! that answered it and the frequency record the commit teaches are untouched -- so
//! the dictionary never learns a casing or a spacing it did not author, which is the
//! trade the benchmark's own filter comments on and this design avoids.
//!
//! Boundaries: plain Rust over values. The state is the session's memory of what
//! last landed on the screen; nothing here touches the host, and a commit the host
//! never confirms leaves the memory exactly as it was.

use ime_types::CandidateSource;

use crate::segment::english::{apply_shape, input_shape};

/// How the commit bridge behaves.
///
/// The configuration's own spelling of the choice lives in `ime-config` (`[english]
/// spacing`); the engine projects it into this enum once per load, the way it
/// projects every other switch the session reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpacingMode {
    /// The shipped behaviour: the boundary rule decides, and a boundary whose
    /// classes ask for the space takes it.
    #[default]
    Auto,
    /// The bridge is shut: every commit leaves exactly as the candidate spelled it.
    Off,
}

/// The character class of a commit text, as the boundary rule reads it.
///
/// A text whose characters do not all share one class -- the `X光` form, a trailing
/// mark -- names no class at all, which the rule table treats as "no opinion": no
/// insertion happens beside it. That collapse is what makes the rule fail-safe
/// rather than wrong on the texts the rule was never written for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundaryClass {
    /// Every character is a CJK ideograph.
    Han,
    /// Every character is an ASCII letter or digit.
    Alnum,
}

/// The class of a commit text, or `None` when it names no single class.
///
/// Empty text names no class, a non-ideograph non-ASCII-alphanumeric character
/// (a mark, a space) names none, and two characters of different classes collapse
/// to none: the rule asks about the text the commit carries as a whole, so a mixed
/// one has no answer.
fn class_of(text: &str) -> Option<BoundaryClass> {
    let mut class: Option<BoundaryClass> = None;
    for ch in text.chars() {
        // The two ideograph blocks the shipped dictionaries draw from: the
        // extension-A block and the unified block. Anything else -- a mark, a
        // full-width form, a letter outside ASCII -- refuses the whole text.
        let this = if matches!(ch, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}') {
            BoundaryClass::Han
        } else if ch.is_ascii_alphanumeric() {
            BoundaryClass::Alnum
        } else {
            return None;
        };
        match class {
            None => class = Some(this),
            Some(held) if held == this => {}
            // A second class is the `X光` form: no opinion, no insertion.
            Some(_) => return None,
        }
    }
    class
}

/// The memory of what the screen holds, and of the commit waiting to land.
///
/// Session memory and nothing else: neither value reaches an effect, a log line or
/// the frame, under the same privacy rule the candidate and commit texts already
/// live under. The tail survives a cancelled composition -- the text it names is
/// still on the screen whatever happened to the input that was going to follow it
/// -- and it is read by nothing but the bridge.
#[derive(Debug, Default)]
pub(super) struct SpacingState {
    /// The text the last landed commit left on the screen, as the bridge dressed
    /// it: the casing and the space it went out with are part of what the next
    /// boundary reads.
    tail: Option<String>,
    /// The commit text produced for the commit in flight, staged until the host
    /// confirms the landing. A commit that never lands never promotes its text,
    /// so the screen and the tail cannot drift apart.
    staged: Option<String>,
}

impl SpacingState {
    /// Dresses the text a highlighted candidate commits as.
    ///
    /// The dressing is the two output-side transforms at the commit door: an
    /// English candidate's text takes the casing form `raw` was typed in, and the
    /// boundary rule takes its single space between the text on the screen and
    /// this one. The text is staged, and reaches the bridge's tail only if the
    /// host confirms the commit landed -- a refused commit teaches the boundary
    /// nothing, exactly as it teaches the dictionary nothing.
    ///
    /// The candidate's own text, which is what the window showed and what the
    /// frequency record will be keyed by, is deliberately not the return value's
    /// input alone: the caller passes it as `entry` and keeps it for the record,
    /// so a `GITHUB` commit still learns `GitLab`'s authored spelling and not the
    /// casing the user happened to type.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn outgoing(
        &mut self,
        entry: &str,
        source: CandidateSource,
        raw: &str,
        mode: SpacingMode,
    ) -> String {
        let shaped = match source {
            CandidateSource::English => {
                apply_shape(entry, input_shape(raw))
            }
            _ => String::from(entry),
        };
        let text = match mode {
            SpacingMode::Off => shaped,
            SpacingMode::Auto => match (
                // The tail is the dressed text the screen holds, so an English
                // tail carries the bridge space it landed with. The boundary
                // reads its class through that space -- a leading space names no
                // class of its own -- or every English-into-English commit after
                // the first would lose its word boundary.
                self.tail
                    .as_deref()
                    .map(str::trim_start)
                    .and_then(class_of),
                class_of(&shaped),
            ) {
                // Chinese into English, and English into English: the word
                // boundary the two texts would otherwise lose.
                (
                    Some(BoundaryClass::Han) | Some(BoundaryClass::Alnum),
                    Some(BoundaryClass::Alnum),
                ) => {
                    let mut bridged = String::with_capacity(shaped.len() + 1);
                    bridged.push(' ');
                    bridged.push_str(&shaped);
                    bridged
                }
                // English into Chinese, Chinese into Chinese, and every pairing
                // either side of which has no class to name.
                _ => shaped,
            },
        };
        self.staged = Some(text.clone());
        text
    }

    /// Promotes the staged commit text to the tail: the host confirmed the commit
    /// landed, so its text is what the screen now holds.
    ///
    /// A landing with nothing staged -- an association or character selection, whose
    /// text the bridge never dressed -- leaves the tail as it is. Those commits are
    /// continuation text of the word already on the screen, so the tail's class
    /// answers the next boundary as theirs would.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn landed(&mut self) {
        if let Some(staged) = self.staged.take() {
            self.tail = Some(staged);
        }
    }

    /// Drops the text staged for a commit that never landed.
    ///
    /// The focus going away mid-commit is the one path that ends the in-flight
    /// state without a landing; the staged text describes a commit the application
    /// refused, so the tail must not learn it.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn dropped(&mut self) {
        self.staged = None;
    }
}

#[cfg(test)]
impl SpacingState {
    /// The tail, for the state-machine tests that pin the promotion.
    ///
    /// The field is bridge-private by design; the tests read it through this
    /// door, the way the context word's own test door works.
    pub(super) fn tail_for_test(&self) -> Option<&str> {
        self.tail.as_deref()
    }

    /// The staged commit text, for the same tests.
    pub(super) fn staged_for_test(&self) -> Option<&str> {
        self.staged.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::{BoundaryClass, SpacingMode, SpacingState, class_of};
    use ime_types::CandidateSource;

    /// A spacing state whose tail the test names, as a landed commit would have
    /// left it.
    fn state_with_tail(tail: Option<&str>) -> SpacingState {
        let mut state = SpacingState::default();
        if let Some(tail) = tail {
            state.tail = Some(String::from(tail));
        }
        state
    }

    /// The commit text `outgoing` produces for an entry, over a tail the test names.
    fn bridged(tail: Option<&str>, entry: &str, source: CandidateSource) -> String {
        let mut state = state_with_tail(tail);
        state.outgoing(entry, source, "", SpacingMode::Auto)
    }

    #[test]
    fn test_class_of_reads_han_alnum_and_refuses_the_mixed_and_unknown() {
        assert_eq!(class_of("你好"), Some(BoundaryClass::Han));
        assert_eq!(class_of("陈列"), Some(BoundaryClass::Han));
        assert_eq!(class_of("hello"), Some(BoundaryClass::Alnum));
        assert_eq!(class_of("IPv6"), Some(BoundaryClass::Alnum));
        assert_eq!(class_of("123"), Some(BoundaryClass::Alnum));
        // The `X光` form: two classes in one text, so no class at all.
        assert_eq!(class_of("X光"), None);
        // A mark, an empty text and a full-width form name no class either.
        assert_eq!(class_of("你好。"), None);
        assert_eq!(class_of(""), None);
        assert_eq!(class_of("ｈｉ"), None);
    }

    #[test]
    fn test_outgoing_han_into_english_takes_the_space() {
        assert_eq!(
            bridged(Some("你好"), "hello", CandidateSource::English),
            " hello"
        );
    }

    #[test]
    fn test_outgoing_english_into_english_takes_the_word_boundary() {
        assert_eq!(
            bridged(Some("hello"), "world", CandidateSource::English),
            " world"
        );
    }

    #[test]
    fn test_outgoing_english_into_chinese_takes_nothing() {
        assert_eq!(bridged(Some("hello"), "你好", CandidateSource::Dict), "你好");
    }

    #[test]
    fn test_outgoing_han_into_han_takes_nothing() {
        assert_eq!(bridged(Some("你好"), "再见", CandidateSource::Dict), "再见");
    }

    #[test]
    fn test_outgoing_digits_boundaries_take_the_rule_their_class_names() {
        // Digits are alphanumeric, so the same two rules answer: Chinese into
        // digits takes the space, digits into digits keep the word boundary, and
        // digits into Chinese take nothing.
        assert_eq!(
            bridged(Some("你好"), "123", CandidateSource::English),
            " 123"
        );
        assert_eq!(bridged(Some("123"), "456", CandidateSource::English), " 456");
        assert_eq!(bridged(Some("123"), "你好", CandidateSource::Dict), "你好");
    }

    #[test]
    fn test_outgoing_mixed_texts_insert_nothing() {
        // The `X光` guard: a mixed candidate beside a Han tail, and a mixed tail
        // beside an English candidate, both refuse the space -- the classes the
        // rule reads do not exist for either side.
        assert_eq!(bridged(Some("你好"), "X光", CandidateSource::Dict), "X光");
        assert_eq!(
            bridged(Some("X光"), "hello", CandidateSource::English),
            "hello"
        );
    }

    #[test]
    fn test_outgoing_marks_in_the_tail_insert_nothing() {
        // A tail that ends in a mark names no class, so the fail-safe answer is
        // the one the rule was never asked about: nothing.
        assert_eq!(
            bridged(Some("你好。"), "hello", CandidateSource::English),
            "hello"
        );
    }

    #[test]
    fn test_outgoing_without_a_tail_inserts_nothing() {
        assert_eq!(bridged(None, "hello", CandidateSource::English), "hello");
    }

    #[test]
    fn test_outgoing_off_dresses_the_casing_but_skips_the_space() {
        // The switch shuts the bridge, not the casing: the two transforms live at
        // the same door and the configuration names only the spacing.
        let mut state = state_with_tail(Some("你好"));
        assert_eq!(
            state.outgoing("hello", CandidateSource::English, "", SpacingMode::Off),
            "hello"
        );
    }

    #[test]
    fn test_outgoing_stages_and_landing_promotes_the_dressed_text() {
        let mut state = state_with_tail(Some("你好"));
        let text = state.outgoing("hello", CandidateSource::English, "", SpacingMode::Auto);
        assert_eq!(text, " hello");
        // Until the landing the tail still names the text before the commit, so
        // the next dressed commit bridges one space against 你好 -- never two
        // against its own staged text, which is what would count the commit in
        // flight twice.
        let next = state.outgoing("world", CandidateSource::English, "", SpacingMode::Auto);
        assert_eq!(next, " world");
        state.landed();
        // The landing moved the tail to the last staged text, which is what the
        // screen now holds; the next boundary reads it as English.
        let after = state.outgoing("again", CandidateSource::English, "", SpacingMode::Auto);
        assert_eq!(after, " again");
    }

    #[test]
    fn test_outgoing_casing_applies_from_the_raw_input() {
        // The casing and the spacing compose: an all-caps spelling of an entry the
        // lexicon authored `GitLab`, over a Han tail, comes out spaced and uppercased.
        let mut state = state_with_tail(Some("你好"));
        let text = state.outgoing("GitLab", CandidateSource::English, "GITLAB", SpacingMode::Auto);
        assert_eq!(text, " GITLAB");
        // And the mixed spelling names no shape, so the entry's own casing stands.
        let identity = state.outgoing("GitLab", CandidateSource::English, "GitH", SpacingMode::Auto);
        assert_eq!(identity, " GitLab");
    }

    #[test]
    fn test_landing_without_a_staged_commit_keeps_the_tail() {
        // An association or character selection lands without the bridge having
        // dressed anything: the tail stays what the last dressed commit left.
        let mut state = state_with_tail(Some("hello"));
        state.landed();
        let next = state.outgoing("world", CandidateSource::English, "", SpacingMode::Auto);
        assert_eq!(next, " world");
    }

    #[test]
    fn test_dropped_staged_commit_never_reaches_the_tail() {
        let mut state = state_with_tail(Some("你好"));
        let text = state.outgoing("hello", CandidateSource::English, "", SpacingMode::Auto);
        assert_eq!(text, " hello", "the commit in flight is dressed");
        state.dropped();
        // The refused commit is forgotten, so the next dressed commit bridges
        // against the tail the screen actually holds.
        let next = state.outgoing("world", CandidateSource::English, "", SpacingMode::Auto);
        assert_eq!(next, " world");
        state.landed();
        let after = state.outgoing("again", CandidateSource::English, "", SpacingMode::Auto);
        assert_eq!(
            after, " again",
            "the tail is the landed one, never the refused commit's"
        );
    }

    #[test]
    fn test_spacing_mode_default_is_auto() {
        assert_eq!(SpacingMode::default(), SpacingMode::Auto);
    }
}
