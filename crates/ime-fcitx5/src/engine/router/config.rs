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

use ime_config::keymap::{KeyBindings, project_keys};
use ime_config::{Config, SchemeChoice};
use ime_core::state::SessionConfig;
use ime_types::ImeError;

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
        let routing = Self {
            keys,
            session,
            client_preedit: config.ui.client_preedit,
            scheme_hint: config.scheme.header_hint(),
        };
        (routing, warnings)
    }
}

#[cfg(test)]
mod tests;
