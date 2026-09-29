//! Privacy policy: which input contexts the engine may learn from.
//!
//! Responsibility: decide, for one input context, whether the session running in it
//! may record user frequencies, whether its content may reach a log event, and whether
//! the candidate window is shown at all. The decision is a pure function of what the
//! host reported -- three flags and a hashed application identifier -- so every row of
//! the policy is testable with no host, no filesystem and no clock.
//!
//! Boundaries: this module holds no application name and no input text. The half of the
//! policy that needs the plaintext -- reading the capability flags off the host's input
//! context, matching the configured application blacklist, and hashing the program name
//! -- lives in `ime-fcitx5`'s `privacy_impl`, because that is the only layer the name
//! ever exists in. The identifier reaches this module as an [`AppIdHash`] and nothing
//! else, so a build of this crate on its own cannot leak an application name it was
//! never given.
//!
//! # Fail closed
//!
//! A context the host reported nothing about is *not* an ordinary context: it might be
//! a password box whose flags could not be read. [`PrivacyPolicy::should_learn`]
//! therefore answers `false` for it, and [`InputContextKind::default`] is that same
//! unreported value rather than an all-clear one.
//!
//! The asymmetry is deliberate and runs through the whole module. Learning fails
//! closed, because a wrong answer there costs the user their password. Display fails
//! open, because hiding the candidate window has never protected anything and a wrong
//! answer there would cost every user their input.
//!
//! # What suppression is and is not
//!
//! Suppression is about learning, never about typing. A sensitive context still
//! composes, still ranks, still commits, and -- by default -- still shows its candidate
//! window, because the user has to be able to see what they are typing. What it never
//! does is reach the user-frequency store. The text committed in a password box is
//! byte for byte the text the same keystrokes would have committed anywhere else.

use core::fmt;

/// The hash of an application identifier.
///
/// The identifier itself -- `firefox`, `org.mozilla.firefox`, `keepassxc` -- never
/// crosses into this crate. The host layer hashes it with a fixed seed, so one
/// application hashes to one value for the whole life of the process, which is what
/// lets diagnostics group a session's events by application without a name ever
/// appearing in them.
///
/// The hash is a stable identifier rather than a secret: it is chosen so that a log
/// line cannot be *read* for an application name, not so that an attacker holding the
/// log cannot guess a name. Nothing here depends on that distinction. The application
/// name is not user input, and no input text ever reaches this type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AppIdHash(u64);

impl AppIdHash {
    /// The identifier of a context whose application the host did not report.
    ///
    /// A hash of a real name is never this value, so "no name" is distinguishable
    /// from "the name that happens to hash to zero".
    pub const UNKNOWN: Self = Self(0);

    /// Wraps a raw hash.
    ///
    /// # Parameters
    ///
    /// - `raw`: the 64-bit hash the host layer computed.
    ///
    /// # Returns
    ///
    /// The identifier that renders as `0x` followed by sixteen hex digits.
    ///
    /// # Errors
    ///
    /// None: every `u64` is a valid hash.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw hash.
    ///
    /// # Returns
    ///
    /// The 64 bits [`AppIdHash::from_raw`] was given.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for AppIdHash {
    /// Renders the identifier as the `app=0x...` field of a diagnostic line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:016x}", self.0)
    }
}

/// What the host reported about one input context.
///
/// This is the entire input to the policy. It carries no text the user typed and no
/// name the user could be identified by: the three flags are the host's own
/// classification of the context, and the identifier is already hashed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputContextKind {
    /// Whether the host reported this context's capability flags at all.
    ///
    /// `false` is the fail-closed answer: the flags below are then meaningless and the
    /// context is treated as if it were sensitive.
    pub is_reported: bool,
    /// `fcitx::CapabilityFlag::Password`, as the host reported it.
    ///
    /// Setting the flag is the client application's responsibility, so this is a
    /// best-effort signal: a password box whose application does not set it cannot be
    /// recognised here.
    pub password: bool,
    /// `fcitx::CapabilityFlag::Sensitive`, as the host reported it.
    ///
    /// The second bit the host offers for the same purpose, used by applications that
    /// treat "sensitive" as broader than "password".
    pub sensitive: bool,
    /// Whether the application matched the configured blacklist.
    ///
    /// Decided by the host layer, which is the only place the plaintext program name
    /// exists; see the module documentation.
    pub blacklisted: bool,
    /// The application identifier, hashed.
    pub app_id: AppIdHash,
}

impl InputContextKind {
    /// The kind of a context the host reported nothing about.
    ///
    /// # Returns
    ///
    /// An unreported kind with no identifier, which every policy must treat as
    /// sensitive.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn unreported() -> Self {
        Self {
            is_reported: false,
            password: false,
            sensitive: false,
            blacklisted: false,
            app_id: AppIdHash::UNKNOWN,
        }
    }

    /// The kind of a context the host reported.
    ///
    /// # Parameters
    ///
    /// - `password`: whether `CapabilityFlag::Password` was set.
    /// - `sensitive`: whether `CapabilityFlag::Sensitive` was set.
    /// - `blacklisted`: whether the application matched the configured blacklist.
    /// - `app_id`: the hashed application identifier.
    ///
    /// # Returns
    ///
    /// A reported kind carrying the four values unchanged.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn reported(
        password: bool,
        sensitive: bool,
        blacklisted: bool,
        app_id: AppIdHash,
    ) -> Self {
        Self {
            is_reported: true,
            password,
            sensitive,
            blacklisted,
            app_id,
        }
    }

    /// Whether this context forbids learning from it.
    ///
    /// # Returns
    ///
    /// `true` for a password context, a context the host marked sensitive, an
    /// application on the blacklist, and -- fail-closed -- a context the host reported
    /// nothing about.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn forbids_learning(&self) -> bool {
        !self.is_reported || self.password || self.sensitive || self.blacklisted
    }
}

impl Default for InputContextKind {
    /// The fail-closed default: an unreported context.
    ///
    /// Deliberately not an all-clear one. A caller that builds a kind without knowing
    /// anything about the context gets "do not learn from this", which is the answer
    /// that cannot cost the user a password.
    fn default() -> Self {
        Self::unreported()
    }
}

/// The learning, logging and display decisions for one input context.
///
/// # Concurrency
///
/// Implementations must be `Send + Sync`: the host thread reads the policy while the
/// UI thread holds its own copy of a frame. Every method is a pure read of the context
/// it is given -- no lock, no allocation, no IO -- and runs inside the commit path,
/// which is what keeps `should_learn` inside its 200ns budget.
pub trait PrivacyPolicy: Send + Sync {
    /// Whether the session in `ctx` may record user frequencies.
    ///
    /// # Returns
    ///
    /// `false` means [`ime_types::UserFreqSource::record`] is never called for that
    /// session: the pinyin the user typed there never reaches `user.redb`, and the
    /// ranking signal it would have produced never exists.
    ///
    /// An implementation must answer `false` whenever it cannot tell whether `ctx` is
    /// sensitive. Learning is the only thing this decision protects, so the cost of
    /// the uncertain answer is ranking quality; the cost of the other one is the
    /// user's password.
    fn should_learn(&self, ctx: &InputContextKind) -> bool;

    /// Whether the session's content may appear in a log event.
    ///
    /// # Returns
    ///
    /// Always `false`. The method and its parameter are kept so that a policy can be
    /// asked about logging next to learning and the answer is a decision rather than
    /// an omission -- but the answer is a product promise, and no implementation may
    /// return `true`. The redaction layer in `ime-diag` is the second line of defence,
    /// not a licence to log content.
    fn should_log_content(&self, _ctx: &InputContextKind) -> bool {
        false
    }

    /// Whether the candidate window may be shown for `ctx`.
    ///
    /// # Returns
    ///
    /// `true` by default, password boxes included: the user has to see what they are
    /// typing, and hiding the window protects nothing that suppressing the learning
    /// does not already protect. An enterprise policy that wants the input method out
    /// of the way entirely returns `false` here, which obliges the caller to pass
    /// every key through to the application for that context.
    fn should_show_ui(&self, _ctx: &InputContextKind) -> bool {
        true
    }
}

/// The shipped policy.
///
/// Sensitivity is read off [`InputContextKind`] alone: the capability flags, the
/// blacklist verdict the host layer folded into the kind, and the unreported case.
/// Nothing else in the policy is configurable, because nothing else can make a context
/// safe to learn from.
///
/// # Examples
///
/// ```
/// use ime_core::privacy::{AppIdHash, DefaultPolicy, InputContextKind, PrivacyPolicy};
///
/// let policy = DefaultPolicy::default();
/// let password_box = InputContextKind::reported(true, false, false, AppIdHash::UNKNOWN);
/// assert!(!policy.should_learn(&password_box));
/// // A password box still shows its candidate window: the user has to see what
/// // they are typing.
/// assert!(policy.should_show_ui(&password_box));
/// // A context the host said nothing about is not an ordinary one.
/// assert!(!policy.should_learn(&InputContextKind::unreported()));
/// assert!(policy.should_learn(&InputContextKind::reported(
///     false,
///     false,
///     false,
///     AppIdHash::from_raw(0x8f3a_2c1d),
/// )));
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DefaultPolicy {
    /// Whether the input method steps aside completely for a context it may not learn
    /// from, instead of composing and showing candidates.
    ///
    /// The `[privacy] disable_ui_on_password` setting; `false` is the shipped default,
    /// which matches what other input methods do. `true` is the enterprise shape: the
    /// candidate window is not shown and every key reaches the application.
    pub disable_ui_on_password: bool,
}

impl PrivacyPolicy for DefaultPolicy {
    fn should_learn(&self, ctx: &InputContextKind) -> bool {
        !ctx.forbids_learning()
    }

    fn should_show_ui(&self, ctx: &InputContextKind) -> bool {
        // The switch is about the contexts the input method is not allowed to learn
        // from, which is the same set the learning decision refuses -- the unreported
        // case included, because a context whose flags could not be read is treated as
        // a password box everywhere else in this module.
        !(self.disable_ui_on_password && ctx.forbids_learning())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reported context with the given flags and no application identifier.
    fn reported(password: bool, sensitive: bool, blacklisted: bool) -> InputContextKind {
        InputContextKind::reported(password, sensitive, blacklisted, AppIdHash::UNKNOWN)
    }

    /// Every shape of context the policy has an answer for.
    fn every_kind() -> [InputContextKind; 5] {
        [
            InputContextKind::unreported(),
            reported(false, false, false),
            reported(true, false, false),
            reported(false, true, false),
            reported(false, false, true),
        ]
    }

    #[test]
    fn test_should_learn_suppresses_a_password_context() {
        let policy = DefaultPolicy::default();
        assert!(
            !policy.should_learn(&reported(true, false, false)),
            "a password box must never reach the frequency store"
        );
        assert!(
            !policy.should_learn(&reported(false, true, false)),
            "the host's sensitive flag must suppress learning too"
        );
    }

    #[test]
    fn test_should_learn_allows_an_ordinary_context() {
        let policy = DefaultPolicy::default();
        assert!(policy.should_learn(&reported(false, false, false)));
    }

    #[test]
    fn test_should_learn_suppresses_an_unreported_context() {
        let policy = DefaultPolicy::default();
        assert!(
            !policy.should_learn(&InputContextKind::unreported()),
            "an unknown context might be a password box, so it fails closed"
        );
        assert!(
            !policy.should_learn(&InputContextKind::default()),
            "the default kind must be the fail-closed one"
        );
    }

    #[test]
    fn test_should_learn_suppresses_a_blacklisted_context() {
        let policy = DefaultPolicy::default();
        assert!(!policy.should_learn(&reported(false, false, true)));
    }

    #[test]
    fn test_should_learn_ignores_the_application_identifier() {
        // The decision is a read of the flags. Two contexts that differ only in which
        // application they belong to must be answered the same way, or the policy
        // would be leaking a per-application judgement it has no name to make.
        let policy = DefaultPolicy::default();
        let first = InputContextKind::reported(false, false, false, AppIdHash::from_raw(1));
        let second = InputContextKind::reported(false, false, false, AppIdHash::from_raw(u64::MAX));
        assert_eq!(policy.should_learn(&first), policy.should_learn(&second));
        assert!(policy.should_learn(&first));
        let unknown = InputContextKind::reported(true, false, false, AppIdHash::UNKNOWN);
        assert!(!policy.should_learn(&unknown));
    }

    #[test]
    fn test_should_log_content_is_false_for_every_kind() {
        // The reserved extension point must stay closed for the shipped policy and for
        // the trait's own default: the product promise is that no input content is
        // ever logged, and this method is the only place a policy could claim
        // otherwise.
        let policy = DefaultPolicy::default();
        for kind in every_kind() {
            let logged = policy.should_log_content(&kind);
            assert!(!logged, "no context may have its content logged: {kind:?}");
        }
    }

    #[test]
    fn test_should_show_ui_shows_the_candidate_window_in_a_password_box() {
        let policy = DefaultPolicy::default();
        for kind in every_kind() {
            let shown = policy.should_show_ui(&kind);
            assert!(
                shown,
                "the shipped policy always shows the window: {kind:?}"
            );
        }
    }

    #[test]
    fn test_should_show_ui_passes_through_when_the_enterprise_switch_is_set() {
        let policy = DefaultPolicy {
            disable_ui_on_password: true,
        };
        assert!(!policy.should_show_ui(&reported(true, false, false)));
        assert!(!policy.should_show_ui(&reported(false, true, false)));
        assert!(!policy.should_show_ui(&reported(false, false, true)));
        assert!(
            !policy.should_show_ui(&InputContextKind::unreported()),
            "a context treated as sensitive for learning is treated as sensitive here"
        );
        assert!(
            policy.should_show_ui(&reported(false, false, false)),
            "an ordinary context keeps its window even in enterprise mode"
        );
        // The switch changes nothing about learning.
        assert!(!policy.should_learn(&reported(true, false, false)));
        assert!(policy.should_learn(&reported(false, false, false)));
    }

    #[test]
    fn test_default_policy_default_matches_the_shipped_configuration() {
        assert!(
            !DefaultPolicy::default().disable_ui_on_password,
            "a password box shows its candidates unless an administrator says otherwise"
        );
    }

    #[test]
    fn test_forbids_learning_covers_every_reason() {
        assert!(reported(true, false, false).forbids_learning());
        assert!(reported(false, true, false).forbids_learning());
        assert!(reported(false, false, true).forbids_learning());
        assert!(InputContextKind::unreported().forbids_learning());
        assert!(!reported(false, false, false).forbids_learning());
    }

    #[test]
    fn test_app_id_hash_renders_the_hash_and_round_trips() {
        assert_eq!(AppIdHash::UNKNOWN.raw(), 0);
        assert_eq!(AppIdHash::from_raw(0x8f3a_2c1d).raw(), 0x8f3a_2c1d);
        assert_eq!(
            AppIdHash::from_raw(0x8f3a_2c1d).to_string(),
            "0x000000008f3a2c1d"
        );
        assert_eq!(AppIdHash::UNKNOWN.to_string(), "0x0000000000000000");
        // The rendering carries the hash and nothing else, so a diagnostic built from
        // it cannot be read for an application name.
        assert!(
            !AppIdHash::from_raw(0xdead_beef)
                .to_string()
                .contains("firefox")
        );
    }
}
