//! Passthrough policy: deciding which key presses the engine must not touch.
//!
//! Responsibility: answer, for one key press, whether the input method should
//! handle it at all -- and when it should, whether the plugin commits something
//! itself or hands the text to the decoder. This is the single place where the
//! URL and email heuristics, the leading-uppercase rule, temporary English mode,
//! Chinese punctuation substitution and full-width output are ordered against each
//! other.
//!
//! Boundaries: everything here is pure (0.4 rule 4). A decision is a function of
//! the text the key produced, the flags derived from `[engine]`, and the optional
//! text around the caret -- no clock, no filesystem, no environment, no global
//! state, no regular expression, and no allocation of the classifier's own. The
//! module does not decode, does not build a `UiFrame`, and does not know what a
//! `KeyAction` is; the key router turns the answer into `on_key_event`'s return
//! value and into the host-side effect.
//!
//! # Decision order
//!
//! [`classify`] applies these rules in order and returns the first match:
//!
//! | # | Condition | Decision |
//! |---|---|---|
//! | 1 | the key produced no text | [`PassthroughDecision::HostHandles`] |
//! | 2 | temporary English mode is active | [`PassthroughDecision::HostHandles`] |
//! | 3 | the text starts with an uppercase letter, and the rule is enabled | [`PassthroughDecision::CommitDirectly`] |
//! | 4 | the text, or the token under the caret, looks like a URL or an email | [`PassthroughDecision::HostHandles`] |
//! | 5 | the text is one mark of the Chinese punctuation table | [`PassthroughDecision::CommitDirectly`] |
//! | 5 | anything else | [`PassthroughDecision::Decode`] |
//!
//! Rule 2 is evaluated ahead of rules 3 and 4, which the design lists in the other
//! order. Temporary English mode exists so that every key reaches the application
//! unchanged; leaving those two rules ahead of it would let a single uppercase
//! letter leave the mode and be committed by the plugin instead of by the
//! application, which is the opposite of what the mode is for.

mod punctuation;
mod surrounding;

pub use self::punctuation::{PUNCTUATION_COUNT, PunctMode, to_full_width, transform_committed};

use self::punctuation::{commit_text, punctuation_decision};
use self::surrounding::trailing_token;

/// How much of the surrounding text is scanned for URL or email evidence.
///
/// A URL or an email address under the caret is short; 64 bytes cover the longest
/// realistic one, and capping the scan keeps [`classify`] inside its 500ns budget
/// no matter how much text the application reports.
const SURROUNDING_TAIL_BYTES: usize = 64;

/// The `[engine]` settings that shape the passthrough decision, plus the session's
/// temporary-English flag.
///
/// The first four fields map one to one onto configuration keys of the `[engine]`
/// section, and [`PassthroughFlags::default`] holds the documented defaults, so the
/// configuration loader can start from `Default` and overwrite only the keys the
/// user set. The struct is `Copy` and stays small: it is passed by value on every
/// key press.
///
/// `temp_english` is session state rather than a configuration key. The key router
/// sets it when the temporary-English shortcut fires and clears it on Enter or
/// Escape. It is carried here because the decision order consults it in the same
/// place as the configured flags, and because one struct keeps [`classify`] a
/// single pure entry point that the whole policy can be driven through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassthroughFlags {
    /// `[engine] auto_english_on_uppercase`; default `true`.
    pub auto_english_on_uppercase: bool,
    /// `[engine] passthrough_url`; default `true`.
    pub passthrough_url: bool,
    /// `[engine] punct_mode`; default [`PunctMode::Chinese`].
    pub punct_mode: PunctMode,
    /// `[engine] full_width`; default `false`.
    pub full_width: bool,
    /// Whether temporary English mode is currently active; default `false`.
    pub temp_english: bool,
}

impl Default for PassthroughFlags {
    fn default() -> Self {
        Self {
            auto_english_on_uppercase: true,
            passthrough_url: true,
            punct_mode: PunctMode::Chinese,
            full_width: false,
            temp_english: false,
        }
    }
}

/// What the engine should do with one key press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PassthroughDecision {
    /// The host handles the key: the plugin consumes nothing and `on_key_event`
    /// returns `false`, so the key keeps travelling down the input stack.
    HostHandles,
    /// The plugin commits this text itself instead of decoding it, which is how
    /// Chinese punctuation and the leading uppercase letter reach the screen.
    CommitDirectly(String),
    /// Enter temporary English mode: every key then passes through to the host
    /// until Enter or Escape leaves the mode.
    ///
    /// The key router returns this for the shortcut itself, not [`classify`]: the
    /// shortcut produces no text, so there is nothing to inspect. The mode it
    /// enters is what [`PassthroughFlags::temp_english`] reports on later keys.
    EnterTempEnglish,
    /// Decode the input as pinyin.
    Decode,
}

impl PassthroughDecision {
    /// Whether the plugin consumes the key, which is what `on_key_event` returns.
    ///
    /// # Returns
    ///
    /// `false` for [`PassthroughDecision::HostHandles`] only, which is what keeps the
    /// plugin from eating keys it has no use for. Entering temporary English mode
    /// counts as consuming the shortcut, because the mode change is our own effect.
    pub fn is_consumed(&self) -> bool {
        !matches!(self, Self::HostHandles)
    }

    /// The text to commit, for the decisions that carry text.
    ///
    /// # Returns
    ///
    /// The payload of [`PassthroughDecision::CommitDirectly`], and `None` otherwise,
    /// so the caller can commit without matching on the variant.
    pub fn committed_text(&self) -> Option<&str> {
        match self {
            Self::CommitDirectly(text) => Some(text),
            _ => None,
        }
    }
}

/// Decides what to do with one key press.
///
/// # Parameters
///
/// - `raw`: the text the key press produced, as it stands in the input buffer. It
///   is empty when the key produced no text at all.
/// - `flags`: the `[engine]` settings plus the session's temporary-English flag.
/// - `surrounding`: the text around the caret as reported by the host, or `None`
///   when the application does not report it. Only the token immediately before the
///   caret is inspected, so text further back -- the URL the user has already left,
///   for instance -- cannot keep the plugin out of the way. `None` is therefore
///   exactly equivalent to surrounding text that carries no URL or email evidence,
///   which is the conservative branch and makes the answer independent of whether
///   the application supports the feature.
///
/// # Returns
///
/// The first rule of the order documented at the top of this module that matches.
///
/// # Errors
///
/// None: every input has a decision. Malformed input is answered rather than
/// rejected -- an empty `raw` is rule 1, and a `&str` that is not on a character
/// boundary cannot be constructed.
///
/// # Panics
///
/// Never: the body is a chain of character tests with no `unwrap`, no arithmetic
/// that can underflow and no indexing.
///
/// # Examples
///
/// ```
/// use ime_core::passthrough::{PassthroughDecision, PassthroughFlags, classify};
///
/// let flags = PassthroughFlags::default();
/// // An uppercase letter is committed instead of decoded, and a Chinese-mode
/// // comma becomes the Chinese comma.
/// assert_eq!(classify("N", flags, None), PassthroughDecision::CommitDirectly(String::from("N")));
/// assert_eq!(classify(",", flags, None), PassthroughDecision::CommitDirectly(String::from("，")));
/// // Pinyin goes to the decoder, and a key with no text goes to the host.
/// assert_eq!(classify("nihao", flags, None), PassthroughDecision::Decode);
/// assert_eq!(classify("", flags, None), PassthroughDecision::HostHandles);
/// ```
pub fn classify(
    raw: &str,
    flags: PassthroughFlags,
    surrounding: Option<&str>,
) -> PassthroughDecision {
    // Rule 1: a key that produced no text is not ours to handle. Space, Enter,
    // Escape and the digits are translated into their own key actions before this
    // point, so an empty buffer means there is nothing to decode.
    let Some(first) = raw.chars().next() else {
        return PassthroughDecision::HostHandles;
    };

    // Rule 2: temporary English mode is absolute.
    if flags.temp_english {
        return PassthroughDecision::HostHandles;
    }

    // Rule 3: an uppercase letter means the user is typing English; commit it and
    // let the key router leave the session.
    if flags.auto_english_on_uppercase && first.is_ascii_uppercase() {
        return PassthroughDecision::CommitDirectly(commit_text(first, flags.full_width));
    }

    // Rule 4: the user is inside a URL or an email address, so the plugin must not
    // turn the keystroke into pinyin.
    if flags.passthrough_url && is_url_or_email(raw, surrounding) {
        return PassthroughDecision::HostHandles;
    }

    // Rule 5: Chinese punctuation, and otherwise the decoder.
    match punctuation_decision(raw, flags, surrounding) {
        Some(decision) => decision,
        None => PassthroughDecision::Decode,
    }
}

/// Whether the key press lands inside a URL or an email address: the evidence is the
/// text the key produced, plus the token immediately before the caret, because the
/// host reports surrounding text for the application's buffer.
fn is_url_or_email(raw: &str, surrounding: Option<&str>) -> bool {
    if has_url_marker(raw) {
        return true;
    }
    match surrounding {
        Some(text) => has_url_marker(trailing_token(text)),
        None => false,
    }
}

/// Whether `text` carries one of the URL or email markers the design names:
/// `://` anywhere, `@` anywhere, `.com` anywhere, or a leading `www.`.
fn has_url_marker(text: &str) -> bool {
    text.contains("://") || text.contains('@') || text.contains(".com") || text.starts_with("www.")
}

#[cfg(test)]
mod tests;
