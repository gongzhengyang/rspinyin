//! The host-side half of the privacy policy.
//!
//! Responsibility: turn what Fcitx5 reports about an input context into the
//! [`InputContextKind`] the policy reads, hash the application identifier before it
//! crosses into `ime-core`, apply the configured application blacklist, and hold the
//! per-context decisions the engine consults.
//!
//! Boundaries: this module is plain Rust. It touches no host object, no file, no clock
//! and no global state, so every rule below is testable with Fcitx5 absent. The one
//! thing it must never do is pass user content anywhere but the store: the key a
//! commit carries reaches [`UserFreqSource::record`] and nothing else -- not a log
//! event, not a diagnostic line, not the crash channel.
//!
//! # The three layers of suppression
//!
//! 1. **Learning.** [`LearningGate::record_commit`] is the only place a commit may
//!    reach the user-frequency store, and it asks [`ContextPrivacy::should_learn`]
//!    first. This is the layer that keeps the pinyin typed into a password box out of
//!    `user.redb`.
//! 2. **Logging.** [`ContextPrivacy::should_log_content`] answers `false` for every
//!    context. The layer that actually downgrades a sensitive session's events lives in
//!    `ime-diag`; this module supplies the decision, not the filter.
//! 3. **Crash files.** The crash writer is `ime-diag`'s as well, and the input buffer a
//!    backtrace might carry is dropped there. Nothing in this module writes a file, so
//!    there is no path from here to a crash report.
//!
//! # Layout
//!
//! `blacklist` is the only file in the crate where an application name exists: it
//! matches the configured patterns against the plaintext and hashes what it keeps.
//! Everything in this file works from the hash and the flags.
//!
//! # Integration points
//!
//! Two call sites belong to the work that creates them, and neither is in this file:
//!
//! * The C ABI carries only the input-context id, so the capability flags and the
//!   program name never reach Rust yet. [`ContextReport`] is the shape the FFI layer
//!   fills in once the vtable forwards them; until then the activation site reports
//!   [`ContextReport::Unreported`], and an unreported context is fail-closed -- the
//!   plugin learns nothing rather than learning from a password box.
//! * `apply_effects` is where a commit's `RecordUserFreq` effect is applied. It calls
//!   [`LearningGate::record_commit`] instead of the store directly.
//!
//! The diagnostics side is the same shape: marking the session sensitive is
//! `ime_diag`'s `DiagHandle::mark_sensitive_session`, and the handle is owned by the
//! addon initialisation sequence, so the call site is the addon's and not this
//! module's.

use std::collections::HashMap;

use ime_core::privacy::{AppIdHash, InputContextKind, PrivacyPolicy};
use ime_types::UserFreqSource;

use crate::ffi::emit_diagnostic;

mod blacklist;

pub use self::blacklist::{AppBlacklist, hash_app_id};

/// Diagnostic code recorded when a context suppresses learning.
///
/// One line per activation, never one per keystroke: the code says *that* the context
/// suppresses learning and carries the context id and the application hash. It has no
/// field an application name or an input string could travel in.
pub const SUPPRESSED_CODE: &str = "privacy/suppressed";

/// What the FFI layer read off the host's input context.
///
/// `Unreported` is a variant rather than an all-clear `Reported`, so a caller that
/// could not read the flags cannot describe the context as ordinary by accident.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextReport<'a> {
    /// The host reported the context's facts.
    Reported {
        /// The program name or the Wayland application id, when the host has one.
        ///
        /// It is hashed here and dropped: it is never stored, never logged, and never
        /// crosses into `ime-core`.
        program: Option<&'a str>,
        /// `fcitx::CapabilityFlag::Password`.
        password: bool,
        /// `fcitx::CapabilityFlag::Sensitive`.
        sensitive: bool,
    },
    /// The flags could not be read for this context, or the host reported nothing.
    Unreported,
}

/// Builds the kind the policy reads from what the host reported.
///
/// # Parameters
///
/// - `report`: the flags and the program name, or [`ContextReport::Unreported`].
/// - `blacklist`: the configured application blacklist.
///
/// # Returns
///
/// An unreported kind for [`ContextReport::Unreported`] -- which every policy treats as
/// sensitive -- and a reported kind carrying the flags plus the hashed identifier
/// otherwise. A report with no usable program name keeps
/// [`AppIdHash::UNKNOWN`] and cannot match the blacklist, because there is nothing to
/// match it against.
///
/// # Errors
///
/// None: every report has an answer.
///
/// # Panics
///
/// Never.
///
/// # Examples
///
/// ```
/// use rspinyin::privacy_impl::{AppBlacklist, ContextReport, classify};
///
/// let blacklist = AppBlacklist::new([String::from("keepassxc")]);
/// let password_box = ContextReport::Reported {
///     program: Some("keepassxc"),
///     password: true,
///     sensitive: false,
/// };
/// assert!(classify(password_box, &blacklist).forbids_learning());
/// // Nothing reported: the fail-closed answer.
/// assert!(classify(ContextReport::Unreported, &blacklist).forbids_learning());
/// ```
pub fn classify(report: ContextReport<'_>, blacklist: &AppBlacklist) -> InputContextKind {
    let ContextReport::Reported {
        program,
        password,
        sensitive,
    } = report
    else {
        return InputContextKind::unreported();
    };
    let (app_id, blacklisted) = match program {
        Some(name) if !name.is_empty() => (hash_app_id(name), blacklist.matches(name)),
        _ => (AppIdHash::UNKNOWN, false),
    };
    InputContextKind::reported(password, sensitive, blacklisted, app_id)
}

/// The per-context privacy decisions the engine consults.
///
/// The engine owns one of these beside its session table: it observes a context when
/// the host activates it, forgets it when the host deactivates it, and asks before
/// every commit that would reach the user-frequency store.
///
/// # Fail closed
///
/// A context that was never observed -- and one that was forgotten -- is answered as if
/// it were a password box. The safe direction of that default is deliberate: a wiring
/// mistake costs the user learned frequencies, never their password.
///
/// # Threading
///
/// The type is `Send + Sync` but holds no lock of its own: the host thread is the only
/// thread that touches it, which keeps the commit path free of lock contention.
pub struct ContextPrivacy {
    /// The policy every question is answered with.
    policy: Box<dyn PrivacyPolicy>,
    /// The configured application blacklist.
    blacklist: AppBlacklist,
    /// What is known about every observed context, keyed by the host's context id.
    contexts: HashMap<u64, InputContextKind>,
}

impl ContextPrivacy {
    /// Builds the state around one policy and one blacklist.
    ///
    /// # Parameters
    ///
    /// - `policy`: the learning, logging and display decisions.
    /// - `blacklist`: the configured application blacklist.
    ///
    /// # Returns
    ///
    /// A state that knows about no context, so every question it is asked before the
    /// first [`ContextPrivacy::observe`] is answered fail-closed.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new(policy: Box<dyn PrivacyPolicy>, blacklist: AppBlacklist) -> Self {
        Self {
            policy,
            blacklist,
            contexts: HashMap::new(),
        }
    }

    /// Notes what the host reported about one input context.
    ///
    /// Replaces whatever was known about `ic`: the host re-reports a context when it is
    /// activated again, and the later report is the true one.
    ///
    /// # Parameters
    ///
    /// - `ic`: the host's stable identity for the input context.
    /// - `report`: what the host reported about it.
    ///
    /// # Returns
    ///
    /// The kind that was stored. The caller reports [`SUPPRESSED_CODE`] once per
    /// activation when [`ContextPrivacy::should_learn`] answers `false`; the commit
    /// path itself never reports, so a password box does not write a line per
    /// keystroke.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn observe(&mut self, ic: u64, report: ContextReport<'_>) -> InputContextKind {
        let kind = classify(report, &self.blacklist);
        self.contexts.insert(ic, kind);
        kind
    }

    /// Drops what was known about one input context.
    ///
    /// Called when the host deactivates the context. Forgetting is safe in either
    /// direction: the context falls back to the fail-closed answer, so a caller that
    /// forgets too early stops learning from it rather than learning from a password
    /// box.
    ///
    /// # Parameters
    ///
    /// - `ic`: the host's identity for the input context.
    ///
    /// # Errors
    ///
    /// None: forgetting a context that was never observed does nothing.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn forget(&mut self, ic: u64) {
        self.contexts.remove(&ic);
    }

    /// What is known about one input context.
    ///
    /// # Parameters
    ///
    /// - `ic`: the host's identity for the input context.
    ///
    /// # Returns
    ///
    /// The kind stored by [`ContextPrivacy::observe`], and the unreported kind -- which
    /// every policy treats as sensitive -- for a context that was never observed.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn kind(&self, ic: u64) -> InputContextKind {
        match self.contexts.get(&ic) {
            Some(kind) => *kind,
            None => InputContextKind::unreported(),
        }
    }

    /// Whether the session in `ic` may record user frequencies.
    ///
    /// # Parameters
    ///
    /// - `ic`: the host's identity for the input context.
    ///
    /// # Returns
    ///
    /// The policy's answer for the context, or `false` when the context is unknown.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn should_learn(&self, ic: u64) -> bool {
        self.policy.should_learn(&self.kind(ic))
    }

    /// Whether the session's content may appear in a log event.
    ///
    /// # Parameters
    ///
    /// - `ic`: the host's identity for the input context.
    ///
    /// # Returns
    ///
    /// Always `false`; see [`PrivacyPolicy::should_log_content`].
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn should_log_content(&self, ic: u64) -> bool {
        self.policy.should_log_content(&self.kind(ic))
    }

    /// Whether the candidate window may be shown for `ic`.
    ///
    /// # Parameters
    ///
    /// - `ic`: the host's identity for the input context.
    ///
    /// # Returns
    ///
    /// The policy's answer. An unknown context keeps its window: hiding it protects
    /// nothing that suppressing the learning does not already protect, and a context
    /// the host said nothing about must still be typable.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn should_show_ui(&self, ic: u64) -> bool {
        self.policy.should_show_ui(&self.kind(ic))
    }

    /// How many contexts the state is holding. Test-only.
    #[cfg(test)]
    fn observed_count(&self) -> usize {
        self.contexts.len()
    }
}

/// The one place a commit may reach the user-frequency store.
///
/// `apply_effects` applies a commit's `RecordUserFreq` effect through this type rather
/// than through the store directly. The indirection is the point: there is one call to
/// [`UserFreqSource::record`] in the host layer, and it sits behind the learning
/// decision, so a future effect cannot reach the store by a second path that forgot to
/// ask.
pub struct LearningGate<'a> {
    /// The per-context decisions.
    privacy: &'a ContextPrivacy,
    /// The store a permitted commit is written to.
    source: &'a dyn UserFreqSource,
}

impl<'a> LearningGate<'a> {
    /// Builds a gate over one privacy state and one store.
    ///
    /// # Parameters
    ///
    /// - `privacy`: the state that decides whether a context may be learned from.
    /// - `source`: the store a permitted commit is written to.
    ///
    /// # Returns
    ///
    /// A gate borrowing both for as long as it lives.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new(privacy: &'a ContextPrivacy, source: &'a dyn UserFreqSource) -> Self {
        Self { privacy, source }
    }

    /// Applies one `RecordUserFreq` effect for the session in `ic`.
    ///
    /// # Parameters
    ///
    /// - `ic`: the host's identity for the input context the commit happened in.
    /// - `key`: the committed word, exactly as the session produced it. It is passed to
    ///   the store and nowhere else: it is user content, and no log event, diagnostic
    ///   line or crash report may carry it.
    /// - `weight_hint`: the session's ranking hint for that commit.
    ///
    /// # Returns
    ///
    /// `true` when the record was written, `false` when the context suppressed it. The
    /// answer is for diagnostics: the committed text is unaffected either way, because
    /// suppression is about learning and never about input.
    ///
    /// # Errors
    ///
    /// None: the store reports its own failures through its read-only flag.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn record_commit(&self, ic: u64, key: &str, weight_hint: u16) -> bool {
        if !self.privacy.should_learn(ic) {
            return false;
        }
        self.source.record(key, weight_hint);
        true
    }
}

/// The line the suppression diagnostic carries.
///
/// Built from the context id and the application hash and from nothing else: the
/// function has no parameter an input string or an application name could travel in,
/// which is what makes "this layer cannot leak content" a property of the signature
/// rather than a promise about the body.
fn suppression_line(ic: u64, kind: &InputContextKind) -> String {
    format!("{SUPPRESSED_CODE}: ic={ic} app={}", kind.app_id)
}

/// Records that `ic`'s context suppresses learning.
///
/// Called once per activation by the site that observes the context, never from the
/// commit path: a password box must not write a log line per keystroke.
///
/// # Parameters
///
/// - `ic`: the host's identity for the input context.
/// - `kind`: what was reported about it, whose hash is the only application
///   information the line carries.
///
/// # Returns
///
/// Nothing. The line goes to the crash channel, which is a stderr write the host
/// captures into its own log.
///
/// # Errors
///
/// None: a write that fails has no second channel to be reported on.
///
/// # Panics
///
/// Never.
pub fn report_suppression(ic: u64, kind: &InputContextKind) {
    emit_diagnostic(&suppression_line(ic, kind));
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use ime_core::privacy::DefaultPolicy;

    use super::*;

    /// A user-frequency source that remembers what it was asked to record.
    ///
    /// The suppression rule is about a call that must *not* happen, so the assertion
    /// the acceptance criteria ask for -- no delta in the store -- is made here, on the
    /// call itself.
    #[derive(Default)]
    struct RecordingSource {
        /// The commits the source was asked to record.
        commits: Mutex<Vec<(String, u16)>>,
    }

    impl RecordingSource {
        /// The commits recorded so far.
        fn recorded(&self) -> Vec<(String, u16)> {
            match self.commits.lock() {
                Ok(commits) => commits.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }
    }

    impl UserFreqSource for RecordingSource {
        fn freq(&self, _key: &str) -> u32 {
            0
        }

        fn record(&self, key: &str, weight_hint: u16) {
            let mut commits = match self.commits.lock() {
                Ok(commits) => commits,
                Err(poisoned) => poisoned.into_inner(),
            };
            commits.push((String::from(key), weight_hint));
        }

        fn is_user_word(&self, _key: &str) -> bool {
            false
        }
    }

    /// What the host reports about a context, in the shape the tests read best.
    fn reported(program: Option<&str>, password: bool, sensitive: bool) -> ContextReport<'_> {
        ContextReport::Reported {
            program,
            password,
            sensitive,
        }
    }

    /// The state a fresh engine has: the shipped policy, no blacklist.
    fn shipped_privacy() -> ContextPrivacy {
        ContextPrivacy::new(Box::new(DefaultPolicy::default()), AppBlacklist::default())
    }

    /// The state an enterprise configuration asks for: no window on a context the
    /// input method may not learn from.
    fn enterprise_privacy() -> ContextPrivacy {
        let policy = DefaultPolicy {
            disable_ui_on_password: true,
        };
        ContextPrivacy::new(Box::new(policy), AppBlacklist::default())
    }

    /// The blacklist the design's example configuration declares.
    fn example_blacklist() -> AppBlacklist {
        AppBlacklist::new(["keepassxc", "1password", "bitwarden"].map(String::from))
    }

    #[test]
    fn test_hash_app_id_is_stable_and_distinguishes_programs() {
        let first = hash_app_id("firefox");
        assert_eq!(
            first,
            hash_app_id("firefox"),
            "the hash has to be process-stable"
        );
        assert_ne!(first, hash_app_id("firefox-bin"));
        assert_ne!(first, hash_app_id("Firefox"), "case is part of the name");
        assert_ne!(
            first,
            AppIdHash::UNKNOWN,
            "a real name must not land on the sentinel"
        );
        assert_ne!(
            hash_app_id(""),
            AppIdHash::UNKNOWN,
            "even the empty name hashes away from the sentinel"
        );
    }

    #[test]
    fn test_classify_reported_context_carries_the_flags_and_the_hashed_program() {
        let kind = classify(
            reported(Some("firefox"), false, false),
            &AppBlacklist::default(),
        );
        assert!(kind.is_reported);
        assert!(!kind.password);
        assert!(!kind.sensitive);
        assert!(!kind.blacklisted);
        assert_eq!(kind.app_id, hash_app_id("firefox"));
        assert!(!kind.forbids_learning());
    }

    #[test]
    fn test_classify_unreported_context_is_fail_closed() {
        let kind = classify(ContextReport::Unreported, &example_blacklist());
        assert!(!kind.is_reported);
        assert!(kind.forbids_learning());
        assert_eq!(kind.app_id, AppIdHash::UNKNOWN);
    }

    #[test]
    fn test_classify_without_a_program_name_leaves_the_identifier_unknown() {
        for program in [None, Some("")] {
            let kind = classify(reported(program, false, false), &example_blacklist());
            assert!(kind.is_reported, "the flags were still reported");
            assert_eq!(kind.app_id, AppIdHash::UNKNOWN, "{program:?}");
            assert!(!kind.blacklisted, "there is no name to match against");
            assert!(
                !kind.forbids_learning(),
                "an ordinary context is still learnable"
            );
        }
    }

    #[test]
    fn test_classify_matches_the_blacklist_case_insensitively_as_a_substring() {
        let blacklist = example_blacklist();
        for program in ["keepassxc", "KeePassXC", "org.keepassxc.KeePassXC"] {
            let kind = classify(reported(Some(program), false, false), &blacklist);
            assert!(kind.blacklisted, "{program} must match");
            assert!(kind.forbids_learning(), "{program} must suppress learning");
        }
        for program in ["firefox", "org.mozilla.firefox", "keepass"] {
            let kind = classify(reported(Some(program), false, false), &blacklist);
            assert!(!kind.blacklisted, "{program} must not match");
            assert!(!kind.forbids_learning());
        }
    }

    #[test]
    fn test_app_blacklist_ignores_empty_patterns() {
        let blacklist = AppBlacklist::new([String::from(""), String::from("keepassxc")]);
        assert!(!blacklist.is_empty());
        assert!(blacklist.matches("keepassxc"));
        assert!(!blacklist.matches("firefox"));

        let empty = AppBlacklist::default();
        assert!(empty.is_empty());
        assert!(!empty.matches("anything"), "an empty list matches nothing");

        let blank = AppBlacklist::new([String::from("   ")]);
        assert!(blank.is_empty(), "a blank pattern names no application");
        let matched = blank.matches("anything");
        assert!(
            !matched,
            "a blank pattern would otherwise match every application"
        );
    }

    #[test]
    fn test_observe_follows_the_host_flags() {
        let mut privacy = shipped_privacy();
        let ordinary = privacy.observe(1, reported(None, false, false));
        assert!(ordinary.is_reported);
        assert!(privacy.should_learn(1));

        privacy.observe(2, reported(None, true, false));
        assert!(
            !privacy.should_learn(2),
            "a password box suppresses learning"
        );
        assert!(
            privacy.should_learn(1),
            "one context's verdict must not leak"
        );

        privacy.observe(3, reported(None, false, true));
        assert!(
            !privacy.should_learn(3),
            "the host's sensitive flag counts too"
        );

        let blacklist = example_blacklist();
        let policy = Box::new(DefaultPolicy::default());
        let mut with_blacklist = ContextPrivacy::new(policy, blacklist);
        with_blacklist.observe(4, reported(Some("keepassxc-bin"), false, false));
        assert!(!with_blacklist.should_learn(4));
    }

    #[test]
    fn test_should_learn_suppresses_a_context_that_was_never_observed() {
        let privacy = shipped_privacy();
        assert!(!privacy.should_learn(7), "an unknown context fails closed");
        assert!(privacy.kind(7).forbids_learning());
        assert_eq!(privacy.kind(7).app_id, AppIdHash::UNKNOWN);
    }

    #[test]
    fn test_forget_drops_the_context_and_returns_to_fail_closed() {
        let mut privacy = shipped_privacy();
        privacy.observe(7, reported(None, false, false));
        assert!(privacy.should_learn(7));
        assert_eq!(privacy.observed_count(), 1);

        privacy.forget(7);
        assert_eq!(privacy.observed_count(), 0);
        assert!(
            !privacy.should_learn(7),
            "a forgotten context is unknown again"
        );

        // Forgetting a context that was never observed is not an error.
        privacy.forget(7);
        assert_eq!(privacy.observed_count(), 0);
    }

    #[test]
    fn test_should_show_ui_keeps_the_window_of_an_unknown_context() {
        let privacy = shipped_privacy();
        assert!(
            privacy.should_show_ui(7),
            "an unknown context keeps its window"
        );
        assert!(privacy.should_show_ui(8));

        let mut enterprise = enterprise_privacy();
        enterprise.observe(1, reported(None, true, false));
        assert!(!enterprise.should_show_ui(1));
        assert!(
            !enterprise.should_show_ui(2),
            "a context treated as sensitive for learning is treated as sensitive here"
        );
        enterprise.observe(3, reported(None, false, false));
        assert!(enterprise.should_show_ui(3));
    }

    #[test]
    fn test_should_log_content_is_false_for_every_context() {
        let mut privacy = shipped_privacy();
        privacy.observe(1, reported(None, false, false));
        privacy.observe(2, reported(Some("keepassxc"), true, false));
        for ic in [1, 2, 3] {
            assert!(
                !privacy.should_log_content(ic),
                "no content logged: ic {ic}"
            );
        }
    }

    #[test]
    fn test_learning_gate_records_nothing_for_a_suppressed_context() {
        let mut privacy = shipped_privacy();
        privacy.observe(1, reported(Some("keepassxc"), true, false));
        let source = RecordingSource::default();
        let gate = LearningGate::new(&privacy, &source);

        assert!(
            !gate.record_commit(1, "ni'hao", 3),
            "a password box is suppressed"
        );
        assert!(
            !gate.record_commit(2, "ni'hao", 3),
            "an unknown context is too"
        );
        assert!(
            source.recorded().is_empty(),
            "the store must see no call at all, so it can gain no record"
        );
    }

    #[test]
    fn test_learning_gate_leaves_the_committed_text_untouched() {
        let mut privacy = shipped_privacy();
        privacy.observe(1, reported(Some("firefox"), false, false));
        privacy.observe(2, reported(Some("keepassxc"), true, false));
        let source = RecordingSource::default();
        let gate = LearningGate::new(&privacy, &source);

        // The same commit in an ordinary context and in a password box: the store sees
        // one of them, and the text the user sees is the same text either way.
        let committed = String::from("ni'hao");
        assert!(gate.record_commit(1, &committed, 9));
        assert!(!gate.record_commit(2, &committed, 9));

        assert_eq!(
            committed, "ni'hao",
            "suppression is about learning, not input"
        );
        assert_eq!(source.recorded(), vec![(String::from("ni'hao"), 9)]);
    }

    #[test]
    fn test_suppression_line_carries_no_application_name() {
        let kind = classify(
            reported(Some("keepassxc"), true, false),
            &example_blacklist(),
        );
        let line = suppression_line(7, &kind);

        assert_eq!(line, format!("{SUPPRESSED_CODE}: ic=7 app={}", kind.app_id));
        assert_eq!(SUPPRESSED_CODE, "privacy/suppressed");
        assert!(line.starts_with("privacy/"), "{line}");
        assert!(
            !line.contains("keepassxc"),
            "the line carries no name: {line}"
        );

        // Writing it is a stderr write and must never be the thing that fails.
        report_suppression(7, &kind);
    }
}
