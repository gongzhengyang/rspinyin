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
//! [`RoutingConfig::from_config`] is a pure function of the [`Config`] it is handed
//! (0.4 rule 4): it touches no clock, no environment and no global state, and its one
//! file read -- the layout document a `[shuangpin.custom]` section may name -- happens
//! here, in the configuration-adoption context, beside the read of `config.toml`
//! itself and never on the decode path. A reload can therefore re-run it at any moment
//! without reaching anything a composition is using, which is half of what keeps a
//! reload from disturbing the input in progress (0.4 rule 10); the other half is that
//! the result replaces the value in force whole rather than field by field, and the
//! session latches its layout while idle, so the table a running composition holds is
//! never the one a swap replaces.
//!
//! # Why a view rather than the document
//!
//! The session state machine cannot name `ime-config`'s `Config`: `ime-core` sits below it
//! in the one-way dependency order, so the values a session reads travel as a
//! [`SessionConfig`] built here. This type is the rest of that copy — the binding table, the
//! client-preedit policy and the label the window's header shows — so that a caller hands
//! the routing layer one value and a reload replaces it whole.

use std::path::{Path, PathBuf};

use ime_config::PunctMode;
use ime_config::keymap::{KeyBindings, project_keys};
use ime_config::{Config, SchemeChoice, ShareState};
use ime_core::shuangpin::SchemeTable;
use ime_core::shuangpin::layout::{
    LAYOUT_DIAGNOSTIC_KEY, LayoutDocument, LayoutTables, compile_layout, parse_layout,
};
use ime_core::state::{AssocSessionConfig, ImSwitchPolicy, SessionConfig};
use ime_core::viterbi::context::ContextBand;
use ime_types::{ImeError, SchemeId};

use super::modes::OutputProjection;

/// The most date-family triggers the routing layer keeps.
///
/// The bound is `ime-config`'s own: the document layer validates the count, and the
/// projection stores exactly that array shape so the two cannot drift.
const MAX_DATE_PREFIXES: usize = ime_config::schema::MAX_QUICK_DATE_PREFIXES;

/// The most bytes of one date-family trigger the routing layer keeps.
///
/// A trigger is typed, not read: two to four bytes name every shipped spelling, and
/// eight leaves room for a personal shorthand. A configured spelling longer than
/// this cannot reach the buffer's own input cap in one trigger plus a payload, so
/// the projection drops it with a diagnostic rather than keeping a trigger nothing
/// can type.
const MAX_PREFIX_BYTES: usize = 8;

/// The stable key a dropped date-family trigger is reported against.
const DATE_PREFIX_KEY: &str = "quick_input.date_prefixes";

/// One date-family trigger, stored in a form [`RoutingConfig`] can copy.
///
/// A trigger is a short ASCII spelling compared byte for byte against the input
/// buffer; the fixed array is what lets the whole routing configuration stay `Copy`
/// and be handed to a step by value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuickPrefix {
    /// The bytes actually held, always within [`MAX_PREFIX_BYTES`].
    len: u8,
    /// The spelling, right-padded with zeros.
    bytes: [u8; MAX_PREFIX_BYTES],
}

impl QuickPrefix {
    /// The empty prefix; it is never a match and only fills unused slots.
    const EMPTY: Self = Self {
        len: 0,
        bytes: [0; MAX_PREFIX_BYTES],
    };

    /// Stores a spelling, or `None` when it cannot be a trigger.
    ///
    /// `None` answers for an empty spelling and for one past
    /// [`MAX_PREFIX_BYTES`]; the caller reports both rather than keeping them.
    fn store(spelling: &str) -> Option<Self> {
        if spelling.is_empty() || spelling.len() > MAX_PREFIX_BYTES {
            return None;
        }
        let mut bytes = [0; MAX_PREFIX_BYTES];
        bytes[..spelling.len()].copy_from_slice(spelling.as_bytes());
        Some(Self {
            len: u8::try_from(spelling.len()).ok()?,
            bytes,
        })
    }

    /// The spelling, as the configuration wrote it.
    pub(super) fn as_str(&self) -> &str {
        // The bytes were copied from a `&str` and never mutated, so the slice is
        // valid UTF-8 by construction; a zero length answers the empty prefix.
        core::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or_default()
    }
}

/// The `[quick_input]` values the routing layer acts on.
///
/// The projection of the configuration section, stored in the copyable shape the
/// routing table passes around. With `enabled` off — the shipped default — the
/// quick-input layer costs one branch per key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuickRouting {
    /// `quick_input.enabled`: whether any quick-input domain can trigger.
    pub enabled: bool,
    /// `quick_input.calc_enabled`: whether the calculator trigger is admitted.
    pub calc_enabled: bool,
    /// The date-family triggers, in configuration order; unused slots are empty.
    pub date_prefixes: [QuickPrefix; MAX_DATE_PREFIXES],
}

impl Default for QuickRouting {
    /// The shipped section: off, with the two spellings the shipped document lists
    /// for the date family.
    fn default() -> Self {
        let mut date_prefixes = [QuickPrefix::EMPTY; MAX_DATE_PREFIXES];
        date_prefixes[0] = QuickPrefix::store("rq").unwrap_or(QuickPrefix::EMPTY);
        date_prefixes[1] = QuickPrefix::store("dt").unwrap_or(QuickPrefix::EMPTY);
        Self {
            enabled: false,
            calc_enabled: false,
            date_prefixes,
        }
    }
}

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
    /// `[quick_input]`: the trigger table and switches of the quick-input domains.
    ///
    /// Off by default, which is the whole cost a build before the panel existed
    /// pays: the step reads the switch once and routes the key exactly as it always
    /// did.
    pub quick: QuickRouting,
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
            quick: QuickRouting::default(),
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
    /// The view, and one diagnostic per value that could not be used: a `[keys]` entry
    /// that could not become a binding, and each error of a `[shuangpin.custom]` layout
    /// that failed its validation. The caller reports them; a projection never refuses,
    /// because a key binding or a layout must not be able to stop the input method from
    /// working — a refused layout leaves the selected built-in scheme answering.
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
        let (keys, mut warnings) = project_keys(&config.keys);
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
        // The custom layout is compiled once here, at adoption: what the decode path
        // reads is the compiled table, never the document.
        let custom_layout = project_custom_layout(config, &mut warnings);
        session.custom_layout = custom_layout;
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
        session.context_band =
            ContextBand::new(config.data.context_tie_band_q16).unwrap_or_default();
        session.assoc = AssocSessionConfig {
            enabled: config.assoc.enabled,
            max_candidates: config.assoc.max_candidates,
            history_depth: config.assoc.history_depth,
        };
        let routing = Self {
            keys,
            session,
            client_preedit: config.ui.client_preedit,
            scheme_hint: match custom_layout {
                // A custom layout is answering whatever `[scheme] scheme` spells, so
                // the header names the custom choice rather than a built-in layout
                // that is not running. A user who turned the hint off keeps it off.
                Some(_) if config.scheme.show_hint => Some(SchemeChoice::Custom.hint()),
                _ => config.scheme.header_hint(),
            },
            auto_english_on_uppercase: config.engine.auto_english_on_uppercase,
            passthrough_url: config.engine.passthrough_url,
            compose: config.engine.compose,
            output: OutputProjection {
                full_width: config.engine.full_width,
                punct_full: config.engine.punct_mode == PunctMode::Chinese,
            },
            share: config.engine.share_state,
            quick: project_quick(&config.quick_input, &mut warnings),
        };
        (routing, warnings)
    }
}

/// Compiles the `[shuangpin.custom]` section into the session's custom layout.
///
/// The section names a layout one of two ways -- a document `shuangpin.custom.file`
/// points at, or the three tables written inline beside it -- and the layout is parsed
/// and validated once here, at configuration adoption. A layout that fails any check
/// loads nothing, and every check that failed is one `config/invalid` diagnostic: a
/// layout that would map some keys wrongly must never half-load. The fall-back is the
/// built-in scheme the document selected, which the session configuration still
/// carries, so a refusal costs the user their custom keys and nothing else.
///
/// A layout that loads is compiled into process-lifetime memory, because the session
/// configuration carries it as a `Copy` reference that may outlive this adoption. One
/// table per adoption that activates a custom layout; reloads are rare, user-driven
/// events, and a table in force is replaced whole, never mutated or freed -- which is
/// what makes a reload that swaps the table safe for a composition in progress.
///
/// A relative `file` value resolves against the configuration directory; a named
/// document is the layout, and the inline tables beside it are then not read.
fn project_custom_layout(
    config: &Config,
    warnings: &mut Vec<ImeError>,
) -> Option<&'static SchemeTable> {
    let section = &config.shuangpin.custom;
    if section.is_unset() {
        return None;
    }
    let document = if section.file.is_empty() {
        Some(LayoutDocument::from_tables(LayoutTables {
            name: (!section.name.is_empty()).then(|| section.name.clone()),
            zero_initial_style: (!section.zero_initial_style.is_empty())
                .then(|| section.zero_initial_style.clone()),
            initials: section.initials.clone(),
            finals: section.finals.clone(),
            standalone: section.standalone.clone(),
        }))
    } else {
        read_layout_document(
            resolve_layout_file(&section.file, configuration_directory().as_deref()),
            warnings,
        )
    };
    let document = document?;
    match compile_layout(&document, SchemeId::FULL) {
        Ok(table) => Some(Box::leak(Box::new(table))),
        Err(errors) => {
            warnings.extend(errors);
            None
        }
    }
}

/// The configuration directory a relative layout document resolves against.
///
/// The section keeps the path exactly as the user wrote it; which directory a bare
/// name means is the host's knowledge, and the host's answer is the directory
/// `config.toml` itself lives in. This is the one environment read of an adoption,
/// beside the read of the configuration file in the same context.
fn configuration_directory() -> Option<PathBuf> {
    let path = ime_config::default_path()?;
    path.parent().map(Path::to_path_buf)
}

/// Resolves the layout document `[shuangpin.custom] file` names.
///
/// An absolute path is used as it stands; a relative one resolves against the
/// configuration directory. `None` answers for an empty value, and for a relative
/// name when there is no configuration directory to sit in.
fn resolve_layout_file(file: &str, config_dir: Option<&Path>) -> Option<PathBuf> {
    if file.is_empty() {
        return None;
    }
    let path = Path::new(file);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    config_dir.map(|directory| directory.join(path))
}

/// Reads the layout document `path` points at, parsed.
///
/// A document that cannot be read is one diagnostic and no layout. The reason names
/// the failure and never the path: a diagnostic reaches the user, and a configuration
/// path is nobody's business but the document's own.
fn read_layout_document(
    path: Option<PathBuf>,
    warnings: &mut Vec<ImeError>,
) -> Option<LayoutDocument> {
    let path = path?;
    match std::fs::read_to_string(&path) {
        Ok(text) => match parse_layout(&text) {
            Ok(document) => Some(document),
            Err(errors) => {
                warnings.extend(errors);
                None
            }
        },
        Err(error) => {
            let reason = if error.kind() == std::io::ErrorKind::NotFound {
                "the layout document shuangpin.custom.file names does not exist"
            } else {
                "the layout document shuangpin.custom.file names cannot be read"
            };
            warnings.push(ImeError::ConfigInvalid {
                key: String::from(LAYOUT_DIAGNOSTIC_KEY),
                reason: String::from(reason),
            });
            None
        }
    }
}

/// Projects the `[quick_input]` section onto the routing layer's copyable view.
///
/// A spelling the routing layer cannot keep — empty, or past its trigger bound — is
/// dropped with one diagnostic rather than silently ignored: a trigger the user
/// configured and never saw is a defect, and the projection is where it is caught.
fn project_quick(
    section: &ime_config::schema::QuickInputConfig,
    warnings: &mut Vec<ImeError>,
) -> QuickRouting {
    let mut date_prefixes = [QuickPrefix::EMPTY; MAX_DATE_PREFIXES];
    let mut kept = 0;
    for spelling in &section.date_prefixes {
        match QuickPrefix::store(spelling) {
            Some(prefix) => {
                if kept < MAX_DATE_PREFIXES {
                    date_prefixes[kept] = prefix;
                    kept += 1;
                }
            }
            None => warnings.push(ImeError::ConfigInvalid {
                key: String::from(DATE_PREFIX_KEY),
                reason: format!(
                    "trigger spelling must be 1..={MAX_PREFIX_BYTES} bytes, got {}",
                    spelling.len()
                ),
            }),
        }
    }
    QuickRouting {
        enabled: section.enabled,
        calc_enabled: section.calc_enabled,
        date_prefixes,
    }
}

#[cfg(test)]
mod tests;
