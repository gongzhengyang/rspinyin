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

mod data;
mod repair;

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

/// The `keys.flip_keys` key.
pub(crate) const KEY_FLIP_KEYS: &str = "keys.flip_keys";
/// The `keys.highlight_keys` key.
pub(crate) const KEY_HIGHLIGHT_KEYS: &str = "keys.highlight_keys";

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

/// The `[engine]` section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineConfig {
    /// `punct_mode`: how ASCII punctuation is treated.
    pub punct_mode: PunctMode,
    /// `full_width`: whether committed ASCII text is widened.
    pub full_width: bool,
    /// `auto_english_on_uppercase`: whether a leading uppercase letter is committed
    /// instead of decoded.
    pub auto_english_on_uppercase: bool,
    /// `passthrough_url`: whether keys are left to the application while the caret is
    /// inside a URL or an email address.
    pub passthrough_url: bool,
    /// `max_raw_len`: the hard limit on the raw input buffer, `1..=MAX_RAW_LEN`.
    /// Lowering it never truncates a buffer that is already longer, so a reload cannot
    /// disturb the input the user is in the middle of typing.
    pub max_raw_len: u8,
    /// `verify_dict_on_load`: how much of the dictionary is verified.
    pub verify_dict_on_load: VerifyDictOnLoad,
    /// `abbrev`: whether an initial-letter abbreviation is expanded, so that `nh` reaches
    /// a word spelled `ni'hao` and not only `nihao` does.
    ///
    /// Off by default: an abbreviation is ambiguous by nature, and a user who never asked
    /// for one must not have it answered ahead of the full spelling. The key is the
    /// configuration's half of the engine's `DecodeFlags::ABBREV` switch; the decoder
    /// reads no configuration, so the host layer turns this flag into that bit.
    pub abbrev: bool,
}

/// The `[ui.animation]` section: how the candidate window fades in and out.
///
/// The section is read once, when the UI thread builds the window: a reload collects
/// new values, but the spring and the durations of a living window are not re-tuned,
/// so a change takes effect when fcitx5 builds the addon again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimationConfig {
    /// `enabled`: whether the window animates at all. With it off every motion lands
    /// on its end state on the first frame.
    pub enabled: bool,
    /// `omega0`: the undamped angular frequency, in rad/s, of the spring that carries
    /// the window's scale through the appear and disappear fades. The highlight, page
    /// and resize springs keep the design's constants and are not configurable.
    pub omega0: f32,
    /// `zeta`: the damping ratio of that spring. Below `1.0` the scale arrives early
    /// and is clamped, so the window never grows past the size it was placed at.
    pub zeta: f32,
    /// `appear_ms`: how long the appear fade is given, in milliseconds. `0` shows the
    /// window directly.
    pub appear_ms: u16,
    /// `disappear_ms`: how long the exit fade is given, in milliseconds. `0` hides it
    /// directly.
    pub disappear_ms: u16,
}

/// The `[ui]` section.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiConfig {
    /// `client_preedit`: whether the composing text is shown in the application's own
    /// preedit area instead of the candidate window.
    pub client_preedit: bool,
    /// `max_per_row`: candidates per row, `3..=9`.
    pub max_per_row: u8,
    /// `show_annotation`: whether the word annotation is shown beside a candidate.
    pub show_annotation: bool,
    /// `max_width_dp`: the widest the window may be, in dp.
    pub max_width_dp: u16,
    /// `corner_radius_dp`: the corner radius, in dp.
    pub corner_radius_dp: u8,
    /// `base_alpha`: the background opacity, `0..=255`.
    pub base_alpha: u8,
    /// The `[ui.animation]` section.
    pub animation: AnimationConfig,
}

/// The `[theme]` section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeConfig {
    /// `scheme`: which palette the window uses.
    pub scheme: ThemeScheme,
    /// `accent`: the accent colour.
    pub accent: Rgb,
}

/// The `[keys]` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeysConfig {
    /// `digit_zero`: what `0` does while a composition is active.
    pub digit_zero: DigitZero,
    /// `enter_commit_raw`: whether Enter commits the raw input instead of the
    /// highlighted candidate.
    pub enter_commit_raw: bool,
    /// `flip_keys`: the keys that page the candidate list, at most
    /// [`MAX_KEY_BINDINGS`] of them, without repeats and without a key
    /// `highlight_keys` already claims.
    pub flip_keys: Vec<KeyName>,
    /// `highlight_keys`: the keys that move the highlight, at most
    /// [`MAX_KEY_BINDINGS`] of them and without repeats. A key both lists name stays
    /// here, and the entry that gives way is the one in `flip_keys`.
    pub highlight_keys: Vec<KeyName>,
}

/// The `[phrases]` section: the shortcuts the user defined for their own text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhraseConfig {
    /// `enabled`: whether the phrase table takes part in a decode at all. With it off
    /// the dictionary answers alone, which is what a user who wants no shortcut to
    /// outrank a word asks for.
    pub enabled: bool,
    /// `file`: the phrase document to read. An empty string is the default location
    /// under the user's configuration directory.
    ///
    /// The value is kept exactly as the document spells it rather than resolved into a
    /// path: this layer is pure, and which directory an empty or relative value
    /// resolves against is the host layer's business.
    pub file: String,
    /// `max_entries`: how many entries the table may hold, `1..=MAX_PHRASE_ENTRIES`.
    /// Zero is refused rather than accepted, because a table that may hold nothing is a
    /// disabled table and `enabled` is the key that says so.
    pub max_entries: u32,
}

/// The whole configuration, after the defaults, the user's document and validation
/// have been applied.
///
/// Plain data: every field is a resolved value, so a consumer never has to know about
/// defaults or about the document. It is handed around as `Arc<Config>` and the value
/// behind that pointer is never mutated.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// `schema_version`: always [`CONFIG_SCHEMA_VERSION`] once the configuration has
    /// been validated.
    pub schema_version: u16,
    /// The `[engine]` section.
    pub engine: EngineConfig,
    /// The `[ui]` section.
    pub ui: UiConfig,
    /// The `[theme]` section.
    pub theme: ThemeConfig,
    /// The `[keys]` section.
    pub keys: KeysConfig,
    /// The `[scheme]` section.
    pub scheme: SchemeConfig,
    /// The `[phrases]` section.
    pub phrases: PhraseConfig,
    /// The `[data]` section.
    pub data: DataConfig,
    /// The `[diagnostics]` section.
    pub diagnostics: DiagnosticsConfig,
}

impl Default for Config {
    /// The built-in defaults: what a user who has no configuration file runs with.
    ///
    /// Every section is spelled out here rather than given an `impl Default` of its
    /// own, so that the defaults are one document in one place and the shipped
    /// `config/default.toml` can be checked against them as a whole.
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            scheme: SchemeConfig::default(),
            engine: EngineConfig {
                punct_mode: PunctMode::Chinese,
                full_width: false,
                auto_english_on_uppercase: true,
                passthrough_url: true,
                max_raw_len: MAX_RAW_LEN,
                verify_dict_on_load: VerifyDictOnLoad::Full,
                abbrev: false,
            },
            ui: UiConfig {
                client_preedit: false,
                max_per_row: 5,
                show_annotation: true,
                max_width_dp: 720,
                corner_radius_dp: 12,
                base_alpha: 217,
                animation: AnimationConfig {
                    enabled: true,
                    omega0: 26.0,
                    zeta: 0.85,
                    appear_ms: 110,
                    disappear_ms: 90,
                },
            },
            theme: ThemeConfig {
                scheme: ThemeScheme::Auto,
                accent: Rgb::new(0x4C, 0x9A, 0xFF),
            },
            keys: KeysConfig {
                digit_zero: DigitZero::Passthrough,
                enter_commit_raw: false,
                flip_keys: vec![
                    KeyName::Minus,
                    KeyName::Equal,
                    KeyName::Up,
                    KeyName::Down,
                    KeyName::Home,
                    KeyName::End,
                ],
                highlight_keys: vec![KeyName::Tab, KeyName::ShiftTab],
            },
            phrases: PhraseConfig {
                enabled: true,
                // Empty rather than a path: the default location is the host layer's to
                // resolve, and a path spelled out here would be wrong on every machine
                // but the one it was written on.
                file: String::new(),
                max_entries: DEFAULT_PHRASE_ENTRIES,
            },
            data: DataConfig {
                durability: Durability::Eventual,
                backup_enabled: true,
                backup_keep: DEFAULT_BACKUP_KEEP,
            },
            diagnostics: DiagnosticsConfig {
                level: LogLevel::Info,
                log_rotation_mb: 8,
                log_keep_files: 3,
                log_input_content: false,
                probes: true,
            },
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
