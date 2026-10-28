//! The configuration model: the resolved settings, their defaults, and the rules
//! that decide which values may be used.
//!
//! Responsibility: define what a rspinyin configuration *is* -- the `[engine]`,
//! `[ui]`, `[theme]`, `[keys]`, `[phrases]`, `[data]` and `[diagnostics]` sections of
//! the design -- with the built-in defaults and the validation rules. The document
//! those defaults are written out as, and every touch of the filesystem, belong to
//! `crate::reload`. Everything here is pure (0.4 rule 4): no filesystem, no clock,
//! no environment, no global state.
//!
//! An unusable value never fails a startup: `Config::repaired` replaces it with its
//! default and returns one `config/invalid` diagnostic per replaced key. Where a
//! key's values are a closed set of spellings the rule is enforced by the type
//! instead -- `Rgb`, `LogLevel`, `KeyName` and the other enums cannot hold a value
//! outside their set, so such a value is caught while the document is merged and
//! never reaches a `Config` at all.
//!
//! One rule is not about a single key: `keys.flip_keys` and `keys.highlight_keys` are two
//! settings over one keymap, so a key both of them name is a conflict rather than a value
//! to check. It is settled in the direction the routing table already applies -- the
//! highlight entry stays and the page entry gives way -- and reported under the code the
//! projection uses for the same condition, so that the two layers answer with one code.

use std::fmt;
use std::ops::RangeInclusive;

use ime_types::{CONFIG_SCHEMA_VERSION, ConfigError, ImeError};

#[cfg(test)]
use crate::keymap::BINDING_CONFLICT_CODE;
use crate::scheme::SchemeConfig;

mod config;
mod data;
mod repair;

pub use self::config::{
    AnimationConfig, AssocConfig, Config, EngineConfig, FuzzyConfig, KeysConfig, PhraseConfig,
    QuickInputConfig, ThemeConfig, UiConfig,
};
pub use self::data::{DataConfig, DiagnosticsConfig};

#[cfg(test)]
use self::repair::both_lists_can_route;
use self::repair::{repair_bindings, repair_cross_list_conflicts};

/// The largest number of keys a configuration document may hold.
///
/// `ASM-19` bounds the user-visible configuration at 120 keys so that the settings
/// surface stays reviewable. The entries of `keys.flip_keys` and
/// `keys.highlight_keys` are part of their list and do not count on their own. The
/// surplus is ignored and reported, which is the assumption's degradation path.
pub const MAX_DOCUMENT_KEYS: usize = 192;

/// The largest number of entries one key-binding list may hold.
///
/// Six, a reviewable length rather than a count of the whitelist: the whitelist has
/// since grown past it -- eight pageable names and six highlightable ones -- and a
/// list may hold six of whichever it likes. The bound used to be eight, which
/// described neither the whitelist nor the table: a list of eight entries could be
/// built only out of names one of the two lists cannot route, so the limit was
/// reachable as a length and never as a set of working keys. The routing layer's
/// `binding_audit` asserts the relation from the other side, so the two cannot drift
/// apart again.
pub const MAX_KEY_BINDINGS: usize = 6;

/// The largest value `engine.max_raw_len` accepts, in characters of raw input.
pub const MAX_RAW_LEN: u8 = 64;

/// The largest `phrases.max_entries` accepts.
///
/// The phrase table is rebuilt from the document on every load and matched on every
/// keystroke, so the limit bounds both the memory a document can claim and the work
/// one load does. A document past it is read up to the limit and the surplus is
/// reported rather than refused, because a phrase table is a convenience and must not
/// be able to stop the input method from starting.
pub const MAX_PHRASE_ENTRIES: u32 = 50_000;

/// The entry limit the shipped configuration declares.
pub const DEFAULT_PHRASE_ENTRIES: u32 = 5_000;

/// The largest `data.backup_keep` accepts.
///
/// One backup generation is at most the export ceiling of the user store on disk, so this
/// bounds the backup directory's growth. It is the ceiling the backup module clamps a
/// hand-built policy to; the two are written out separately and must stay equal, because
/// this crate does not depend on the dictionary crate that owns that module.
pub const MAX_BACKUP_KEEP: u8 = 32;

/// The `data.backup_keep` the shipped configuration declares.
pub const DEFAULT_BACKUP_KEEP: u8 = 3;

/// The `data.decay_half_life_ticks` the shipped configuration declares.
///
/// Two hundred commits is the librime e-folding constant; its half-life is `200 *
/// ln(2)`, rounded to the nearest whole logical commit.
pub const DEFAULT_DECAY_HALF_LIFE_TICKS: u32 = 139;

/// The largest `data.decay_half_life_ticks` accepts.
///
/// The fold saturates for any `u32`, but a ceiling keeps an unusable policy from
/// silently disabling decay forever when a magnitude was meant to be a smaller count.
pub const MAX_DECAY_HALF_LIFE_TICKS: u32 = 1_000_000_000;

/// The `data.context_tie_band_q16` the shipped configuration declares.
pub const DEFAULT_CONTEXT_TIE_BAND_Q16: i64 = 1 << 8;

/// The largest `data.decay_floor` accepts, in Q16.16.
///
/// One integer word of accumulator is far above the useful stop-acting threshold and
/// bounds the floor independently of the platform's `i64` width.
pub const MAX_DECAY_FLOOR_Q16: i64 = 1 << 31;

/// The `keys.flip_keys` key.
pub(crate) const KEY_FLIP_KEYS: &str = "keys.flip_keys";
/// The `keys.highlight_keys` key.
pub(crate) const KEY_HIGHLIGHT_KEYS: &str = "keys.highlight_keys";
/// The largest number of `quick_input.date_prefixes` accepts.
///
/// The trigger path compares the buffer against entries, so the bound keeps the
/// miss path bounded while leaving room for a personal shorthand set.
pub const MAX_QUICK_DATE_PREFIXES: usize = 16;
/// The smallest `assoc.max_candidates` accepts; zero would silently disable an enabled feature.
pub const MIN_ASSOC_CANDIDATES: u8 = 1;
/// The largest `assoc.max_candidates` accepts, matching the digit/page limit.
pub const MAX_ASSOC_CANDIDATES: u8 = 9;
/// The largest `assoc.history_depth` accepts, bounding session-only memory.
pub const MAX_ASSOC_HISTORY_DEPTH: u8 = 64;

/// The `config/invalid` diagnostic for one key.
fn invalid(key: &str, reason: String) -> ConfigError {
    ConfigError::Invalid {
        key: String::from(key),
        reason,
    }
}

/// Declares a configuration key whose values are a closed set of spellings.
///
/// Six keys have this shape and differ only in their names, so the shape is written
/// once: the enum, the default variant, and the `TryFrom` that reports a value outside
/// the set against the key it came from. Spelled out by hand it is a hundred lines of
/// `match` arms identical but for their spellings.
macro_rules! closed_set {
    (
        $(#[$meta:meta])*
        $name:ident, key = $key:literal, message = $message:literal,
        default = $default:ident, values = { $($variant:ident => $text:literal),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum $name {
            $(
                #[doc = concat!("The `", $text, "` spelling.")]
                $variant,
            )+
        }

        impl Default for $name {
            /// The spelling the shipped `config/default.toml` declares.
            fn default() -> Self {
                Self::$default
            }
        }

        impl TryFrom<&str> for $name {
            type Error = ConfigError;

            /// Parses the value of the key this set belongs to.
            ///
            /// # Errors
            ///
            /// [`ConfigError::Invalid`] naming the key when the value is not one of
            /// the spellings above.
            // Fully qualified because the macro also generates other conversions for the
            // same type, which makes a bare `Self::Error` ambiguous.
            fn try_from(value: &str) -> Result<Self, <Self as TryFrom<&str>>::Error> {
                match value {
                    $(
                        $text => Ok(Self::$variant),
                    )+
                    other => Err(invalid($key, format!("{}: {other}", $message))),
                }
            }
        }
    };
}

closed_set! {
    /// How Chinese punctuation substitution is configured: the `engine.punct_mode`
    /// key. The configuration's own spelling of the value rather than the decoder's
    /// `ime_core::passthrough::PunctMode`, which this layer does not depend on.
    PunctMode, key = "engine.punct_mode", message = "unknown punctuation mode",
    default = Chinese, values = { Chinese => "chinese", English => "english" }
}

closed_set! {
    /// How much of the dictionary is verified at load: `engine.verify_dict_on_load`.
    VerifyDictOnLoad, key = "engine.verify_dict_on_load", message = "unknown mode",
    default = Full, values = { Full => "full", Header => "header" }
}

closed_set! {
    /// What happens to a live composition when the input method is switched away
    /// from or reset: the `engine.on_im_switch` key.
    ///
    /// `Clear` is the shipped behaviour: the composing text is taken back and
    /// nothing reaches the application. `CommitRaw` commits the raw input first,
    /// so a user who switches away mid-word keeps the letters they typed. The
    /// two finer policies other engines offer -- committing the converted text
    /// and committing a preview -- collapse onto `CommitRaw` here, because this
    /// engine's conversion chain runs at commit time on the effect path, where a
    /// switch-away commit is the same door.
    ImSwitch, key = "engine.on_im_switch", message = "unknown switch policy",
    default = Clear, values = { Clear => "clear", CommitRaw => "commit_raw" }
}

closed_set! {
    /// How sessions are shared across input contexts: the `engine.share_state` key.
    ///
    /// `Context` keeps one session per input context, which is the historical
    /// behaviour: two windows of one application hold independent mode bits. `Program`
    /// shares one session across every input context of the same application, so the
    /// mode bits survive a window switch inside one application; the sessions of two
    /// different applications never touch. Applications are told apart by the hashed
    /// identifier the privacy layer computes, never by a name.
    ShareState, key = "engine.share_state", message = "unknown sharing mode",
    default = Context, values = { Context => "context", Program => "program" }
}

closed_set! {
    /// Which colour scheme the candidate window uses: the `theme.scheme` key.
    ThemeScheme, key = "theme.scheme", message = "unknown scheme",
    default = Auto, values = { Auto => "auto", Light => "light", Dark => "dark" }
}

closed_set! {
    /// What the `0` key does while a composition is active: `keys.digit_zero`.
    DigitZero, key = "keys.digit_zero", message = "unknown digit-zero action",
    default = Passthrough, values = { Passthrough => "passthrough", Flip => "flip" }
}

closed_set! {
    /// How user-frequency writes reach the disk: the `data.durability` key.
    Durability, key = "data.durability", message = "unknown durability",
    default = Eventual, values = { Eventual => "eventual", Immediate => "immediate" }
}

closed_set! {
    /// The verbosity of the log: `diagnostics.level`. The spellings match `tracing`'s
    /// levels one for one; this crate does not depend on `tracing`.
    LogLevel, key = "diagnostics.level", message = "unknown level",
    default = Info, values = {
        Error => "error", Warn => "warn", Info => "info", Debug => "debug", Trace => "trace"
    }
}

/// A key that `keys.flip_keys` and `keys.highlight_keys` may name.
///
/// The whitelist is the set of keys the key translator gives a page or a highlight
/// action to; a name outside it would be accepted and then never translated, so it is
/// rejected while the document is read. The design enumerates the whitelist only
/// through the defaults it ships.
///
/// A variant added here is a key the routing layer has to route: it must be registered in
/// `ime-fcitx5`'s `binding_audit` module as well, whose assertions hold this whitelist and
/// the routing table together, and whose table of keysyms is exhaustive over this enum so
/// that a name added on one side alone fails to build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyName {
    /// `-`.
    Minus,
    /// `=`.
    Equal,
    /// `Up`.
    Up,
    /// `Down`.
    Down,
    /// `Left`.
    Left,
    /// `Right`.
    Right,
    /// `Tab`.
    Tab,
    /// `Shift+Tab`.
    ShiftTab,
    /// `Page_Up`.
    PageUp,
    /// `Page_Down`.
    PageDown,
    /// `Home`, which jumps to the first page of candidates.
    Home,
    /// `End`, which jumps to the last page of candidates.
    End,
}

impl KeyName {
    /// Parses one key name.
    ///
    /// # Parameters
    ///
    /// - `raw`: the name as the document spells it, in lower snake case.
    /// - `key`: the configuration key the name came from, so that the diagnostic
    ///   names `keys.flip_keys` or `keys.highlight_keys` rather than the section.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Invalid`] naming `key` when the name is not in the whitelist.
    pub fn parse(raw: &str, key: &str) -> Result<Self, ConfigError> {
        match raw {
            "minus" => Ok(Self::Minus),
            "equal" => Ok(Self::Equal),
            "up" => Ok(Self::Up),
            "down" => Ok(Self::Down),
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            "tab" => Ok(Self::Tab),
            "shift_tab" => Ok(Self::ShiftTab),
            "page_up" => Ok(Self::PageUp),
            "page_down" => Ok(Self::PageDown),
            "home" => Ok(Self::Home),
            "end" => Ok(Self::End),
            other => Err(invalid(key, format!("unknown key name: {other}"))),
        }
    }

    /// The spelling the configuration file uses, which is what [`KeyName::parse`]
    /// accepts.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minus => "minus",
            Self::Equal => "equal",
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
            Self::Tab => "tab",
            Self::ShiftTab => "shift_tab",
            Self::PageUp => "page_up",
            Self::PageDown => "page_down",
            Self::Home => "home",
            Self::End => "end",
        }
    }
}

/// A colour from the `theme.accent` key.
///
/// A colour rather than the `#RRGGBB` string it is written as: parsing it while the
/// document is read means an unusable colour is reported against the key that carried
/// it and can never reach a renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    /// Red channel.
    red: u8,
    /// Green channel.
    green: u8,
    /// Blue channel.
    blue: u8,
}

impl Rgb {
    /// Builds a colour from its three channels.
    ///
    /// # Parameters
    ///
    /// - `red`, `green`, `blue`: the channel values.
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// The three channels, in the order a framebuffer takes them.
    pub const fn rgb(self) -> [u8; 3] {
        [self.red, self.green, self.blue]
    }
}

impl TryFrom<&str> for Rgb {
    type Error = ConfigError;

    /// Parses the `theme.accent` value, which is written as `#RRGGBB`.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Invalid`] naming `theme.accent` when the value is not `#RRGGBB`.
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let digits = value
            .strip_prefix('#')
            .filter(|digits| digits.len() == 6)
            .and_then(|digits| u32::from_str_radix(digits, 16).ok());
        match digits {
            Some(packed) => Ok(Self::new(
                (packed >> 16) as u8,
                (packed >> 8) as u8,
                packed as u8,
            )),
            None => Err(invalid(
                "theme.accent",
                format!("not a #RRGGBB colour: {value}"),
            )),
        }
    }
}

/// The warnings collected while a configuration is checked or a document is merged.
///
/// One collector rather than a `Vec` threaded through every rule: a rule reports what
/// it found and never decides what to do about it.
#[derive(Default)]
pub(crate) struct Warnings {
    /// One diagnostic per key that was rejected or replaced, in the order the rules
    /// ran.
    pub(crate) entries: Vec<ImeError>,
}

impl Warnings {
    /// Records that `key` holds a value that cannot be used, and why.
    pub(crate) fn report(&mut self, key: &str, reason: String) {
        self.entries.push(ImeError::from(invalid(key, reason)));
    }

    /// Records a diagnostic a rule has already built.
    pub(crate) fn report_error(&mut self, error: ConfigError) {
        self.entries.push(ImeError::from(error));
    }

    /// Records a diagnostic a rule has already folded onto the frozen error type.
    ///
    /// Distinct from [`Warnings::report_error`], which takes the crate-local
    /// [`ConfigError`]: a rule that had to name a stable code this enum does not carry
    /// has already built the `ImeError` and must not have it converted back.
    pub(crate) fn report_ime_error(&mut self, error: ImeError) {
        self.entries.push(error);
    }

    /// Records that a section holds more entries than it may.
    pub(crate) fn report_limit(&mut self, section: &str, limit: usize) {
        self.entries
            .push(ImeError::from(ConfigError::LimitExceeded {
                section: String::from(section),
                limit,
            }));
    }

    /// Returns `value` when it lies inside `range`, and records a diagnostic and
    /// returns `default` when it does not.
    fn in_range<T: Copy + PartialOrd + fmt::Display>(
        &mut self,
        value: T,
        range: RangeInclusive<T>,
        default: T,
        key: &str,
    ) -> T {
        if range.contains(&value) {
            return value;
        }
        self.report(
            key,
            format!("{value} is outside {}..={}", range.start(), range.end()),
        );
        default
    }
}

impl Config {
    /// Reports every key of this configuration that breaks a rule, without changing
    /// anything.
    ///
    /// # Returns
    ///
    /// One `config/invalid` diagnostic per unusable key, in schema order, and an empty
    /// list when the configuration is valid: the read-only view of
    /// [`Config::repaired`].
    ///
    /// # Errors
    ///
    /// None: a violation is a diagnostic, not a failure.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn validate(&self) -> Vec<ImeError> {
        self.clone().repaired().1
    }

    /// Replaces every value that breaks a rule with its built-in default.
    ///
    /// # Returns
    ///
    /// The usable configuration, and one diagnostic per key that was replaced, in
    /// schema order. An empty diagnostic list means the configuration was valid.
    ///
    /// # Errors
    ///
    /// None: an unusable value is repaired rather than rejected, because a
    /// configuration must never be able to stop the input method from working.
    ///
    /// # Panics
    ///
    /// Never: every rule is a range test on a value that is already in memory.
    pub fn repaired(mut self) -> (Self, Vec<ImeError>) {
        let mut warnings = Warnings::default();
        let defaults = Self::default();
        if self.schema_version != CONFIG_SCHEMA_VERSION {
            warnings.report(
                "schema_version",
                format!("unsupported schema version: {}", self.schema_version),
            );
            self.schema_version = defaults.schema_version;
        }
        // The scheme section is repaired by taking it out and putting it back: its
        // repair borrows the warnings it reports through, and the section itself is not
        // `Copy` because it carries the user's custom table.
        let scheme = std::mem::take(&mut self.scheme);
        self.scheme = scheme.repair(&mut warnings);
        let engine = &mut self.engine;
        engine.max_raw_len = warnings.in_range(
            engine.max_raw_len,
            1..=MAX_RAW_LEN,
            defaults.engine.max_raw_len,
            "engine.max_raw_len",
        );
        let ui = &mut self.ui;
        ui.max_per_row = warnings.in_range(
            ui.max_per_row,
            3..=9,
            defaults.ui.max_per_row,
            "ui.max_per_row",
        );
        ui.max_width_dp = warnings.in_range(
            ui.max_width_dp,
            220..=1200,
            defaults.ui.max_width_dp,
            "ui.max_width_dp",
        );
        ui.corner_radius_dp = warnings.in_range(
            ui.corner_radius_dp,
            8..=20,
            defaults.ui.corner_radius_dp,
            "ui.corner_radius_dp",
        );
        let animation = &mut ui.animation;
        animation.omega0 = warnings.in_range(
            animation.omega0,
            4.0..=80.0,
            defaults.ui.animation.omega0,
            "ui.animation.omega0",
        );
        animation.zeta = warnings.in_range(
            animation.zeta,
            0.3..=2.0,
            defaults.ui.animation.zeta,
            "ui.animation.zeta",
        );
        animation.appear_ms = warnings.in_range(
            animation.appear_ms,
            0..=600,
            defaults.ui.animation.appear_ms,
            "ui.animation.appear_ms",
        );
        animation.disappear_ms = warnings.in_range(
            animation.disappear_ms,
            0..=600,
            defaults.ui.animation.disappear_ms,
            "ui.animation.disappear_ms",
        );
        let assoc = &mut self.assoc;
        assoc.max_candidates = warnings.in_range(
            assoc.max_candidates,
            MIN_ASSOC_CANDIDATES..=MAX_ASSOC_CANDIDATES,
            defaults.assoc.max_candidates,
            "assoc.max_candidates",
        );
        assoc.history_depth = warnings.in_range(
            assoc.history_depth,
            0..=MAX_ASSOC_HISTORY_DEPTH,
            defaults.assoc.history_depth,
            "assoc.history_depth",
        );
        let phrases = &mut self.phrases;
        phrases.max_entries = warnings.in_range(
            phrases.max_entries,
            1..=MAX_PHRASE_ENTRIES,
            defaults.phrases.max_entries,
            "phrases.max_entries",
        );
        let data = &mut self.data;
        data.backup_keep = warnings.in_range(
            data.backup_keep,
            1..=MAX_BACKUP_KEEP,
            defaults.data.backup_keep,
            "data.backup_keep",
        );
        data.decay_half_life_ticks = warnings.in_range(
            data.decay_half_life_ticks,
            1..=MAX_DECAY_HALF_LIFE_TICKS,
            defaults.data.decay_half_life_ticks,
            "data.decay_half_life_ticks",
        );
        data.decay_floor = warnings.in_range(
            data.decay_floor,
            0..=MAX_DECAY_FLOOR_Q16,
            defaults.data.decay_floor,
            "data.decay_floor",
        );
        if data.context_tie_band_q16 <= 0 {
            warnings.report(
                "data.context_tie_band_q16",
                format!(
                    "{} is not a positive score width",
                    data.context_tie_band_q16
                ),
            );
            data.context_tie_band_q16 = defaults.data.context_tie_band_q16;
        }
        repair_bindings(&mut self.keys.flip_keys, KEY_FLIP_KEYS, &mut warnings);
        repair_bindings(
            &mut self.keys.highlight_keys,
            KEY_HIGHLIGHT_KEYS,
            &mut warnings,
        );
        // The order matters: each list is deduplicated and bounded first, so the overlap
        // this settles is between two lists that are already usable.
        repair_cross_list_conflicts(&mut self.keys, &mut warnings);
        (self, warnings.entries)
    }
}

#[cfg(test)]
mod tests;
