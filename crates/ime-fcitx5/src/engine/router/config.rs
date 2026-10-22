//! The configuration the routing layer acts on, as one value.
//!
//! # Responsibility
//!
//! `ime-config` owns the document: the file format, the key-name whitelist and the
//! validation. This module owns the one place those values become the routing layer's own
//! view of them — [`RoutingConfig`] — which is the value a caller hands to
//! [`KeyRouter::new`](super::KeyRouter::new) and to
//! [`KeyRouter::reload`](super::KeyRouter::reload).
//!
//! # Boundary
//!
//! [`RoutingConfig::from_config`] is a pure function (0.4 rule 4): it reads a [`Config`]
//! that is already in memory and touches no file, no clock, no environment and no global
//! state. A reload can therefore re-run it at any moment without reaching anything a
//! composition is using, which is half of what keeps a reload from disturbing the input in
//! progress (0.4 rule 10); the other half is that the result replaces the value in force
//! whole rather than field by field.
//!
//! # Why a view rather than the document
//!
//! The session state machine cannot name `ime-config`'s `Config`: `ime-core` sits below it
//! in the one-way dependency order, so the values a session reads travel as a
//! [`SessionConfig`] built here. This type is the rest of that copy — the binding table, the
//! client-preedit policy and the label the window's header shows — so that a caller hands
//! the routing layer one value and a reload replaces it whole.

use ime_config::PunctMode;
use ime_config::keymap::{KeyBindings, project_keys};
use ime_config::{Config, SchemeChoice, ShareState};
use ime_core::state::{ImSwitchPolicy, SessionConfig};
use ime_types::ImeError;

use super::modes::OutputProjection;

/// The configuration values the routing layer acts on.
///
/// A view rather than the user's document: the routing table reads the `[keys]` rows
/// through [`KeyBindings`], the session reads its own values through [`SessionConfig`],
/// and the effect executor reads the client-preedit policy directly. Grouping them means
/// a caller hands one value to [`KeyRouter::new`](super::KeyRouter::new) and a reload hands
/// one value to [`KeyRouter::reload`](super::KeyRouter::reload).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoutingConfig {
    /// The `[keys]` settings the translation table branches on.
    pub keys: KeyBindings,
    /// The configuration the session state machine reads.
    pub session: SessionConfig,
    /// `[ui] client_preedit`: whether the composing text is written into the
    /// application's own preedit area instead of only into the candidate window.
    pub client_preedit: bool,
    /// `[scheme]`: the label the candidate window's header shows for the active layout, or
    /// `None` when the user turned the hint off.
    ///
    /// The value of [`SchemeConfig::header_hint`](ime_config::SchemeConfig::header_hint),
    /// resolved once per adoption rather than read on every frame: the routing layer shows
    /// it in the status strip of every repaint, and the header of a window that repaints on
    /// each keystroke is no place to re-decide what a section says. It is user-facing copy,
    /// so it is Chinese like the rest of the window's text.
    pub scheme_hint: Option<&'static str>,
    /// `[engine] auto_english_on_uppercase`: whether a leading uppercase letter is
    /// committed as English instead of being handed to the decoder.
    pub auto_english_on_uppercase: bool,
    /// `[engine] passthrough_url`: whether keys are left to the application while the
    /// caret sits inside a URL or an email address.
    pub passthrough_url: bool,
    /// `[engine] compose`: whether a dead-key sequence the host is composing is
    /// answered before the key table sees the key. Off by default; with a host
    /// that offers no compose support the switch is inert.
    pub compose: bool,
    /// The output bits a fresh context starts from, from the `[engine]` section's
    /// `full_width` and `punct_mode`.
    pub output: OutputProjection,
    /// `[engine] share_state`: whether the state a window switch keeps is per input
    /// context or per application.
    pub share: ShareState,
}

impl Default for RoutingConfig {
    /// The shipped defaults: the values `config/default.toml` declares for `[keys]`,
    /// `[ui]` and `[scheme]`, so a caller with no configuration yet routes keys, fills the
    /// preedit area and labels the window exactly as a fresh installation does.
    ///
    /// The label is the shipped document's rather than the engine's own Chinese / English
    /// one, because `scheme.show_hint` is on by default. A test projects the built-in
    /// configuration and requires the two to be equal, so this value and the document
    /// cannot drift apart.
    fn default() -> Self {
        Self {
            keys: KeyBindings::default(),
            session: SessionConfig::default(),
            client_preedit: false,
            scheme_hint: Some(SchemeChoice::Full.hint()),
            // The `[engine]` section's own defaults, so a caller with no document
            // classifies exactly as a fresh installation does.
            auto_english_on_uppercase: true,
            passthrough_url: true,
            compose: false,
            output: OutputProjection::default(),
            share: ShareState::Context,
        }
    }
}

impl RoutingConfig {
    /// Projects a configuration document onto the routing layer's view.
    ///
    /// # Arguments
    ///
    /// * `config` — the configuration in force, as `ime-config` repaired it. The projection
    ///   runs on the repaired value rather than on the document, so an entry the
    ///   configuration layer already dropped cannot be reported a second time.
    ///
    /// # Returns
    ///
    /// The view, and one diagnostic per `[keys]` entry that could not become a binding: a
    /// name a list cannot use, a name a list holds twice, and a key both lists bound. The
    /// caller reports them; a projection never refuses, because a key binding must not be
    /// able to stop the input method from working.
    ///
    /// # Errors
    ///
    /// None: an entry that cannot be used is a diagnostic, not a failure.
    ///
    /// # Panics
    ///
    /// Never: the walk reads the configuration's own values and two lists of at most
    /// [`MAX_KEY_BINDINGS`](ime_config::schema::MAX_KEY_BINDINGS) entries.
    pub fn from_config(config: &Config) -> (Self, Vec<ImeError>) {
        let (keys, warnings) = project_keys(&config.keys);
        let (scheme, keep_full_pinyin) = config.scheme.decode_settings();
        let mut session = SessionConfig::new(
            config.engine.max_raw_len,
            config.ui.max_per_row,
            config.ui.show_annotation,
            config.ui.max_width_dp,
        );
        // The two scheme keys are the ones `SessionConfig::new` leaves at their defaults,
        // because they are the section `ime-core` cannot name a type of its own for.
        session.scheme = scheme;
        session.keep_full_pinyin = keep_full_pinyin;
        // The fuzzy classes are projected once, here: the master bit and the class bits
        // have to agree, and one projection per configuration load is where that
        // agreement is kept. The session latches the mask beside the scheme.
        session.fuzzy =
            ime_core::fuzzy::projection(config.fuzzy.enabled, config.fuzzy.class_flags());
        session.abbrev = config.engine.abbrev;
        session.correction = config.engine.correction;
        session.english = config.engine.mixed_english;
        session.on_im_switch = match config.engine.on_im_switch {
            ime_config::schema::ImSwitch::Clear => ImSwitchPolicy::Clear,
            ime_config::schema::ImSwitch::CommitRaw => ImSwitchPolicy::CommitRaw,
        };
        let routing = Self {
            keys,
            session,
            client_preedit: config.ui.client_preedit,
            scheme_hint: config.scheme.header_hint(),
            auto_english_on_uppercase: config.engine.auto_english_on_uppercase,
            passthrough_url: config.engine.passthrough_url,
            compose: config.engine.compose,
            output: OutputProjection {
                full_width: config.engine.full_width,
                punct_full: config.engine.punct_mode == PunctMode::Chinese,
            },
            share: config.engine.share_state,
        };
        (routing, warnings)
    }
}

#[cfg(test)]
mod tests;
