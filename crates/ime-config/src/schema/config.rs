//! The configuration sections: the shape of one `config.toml`, section by section.
//!
//! Split out of `schema.rs` to keep that file inside the line limit. Every struct here is
//! the typed form of one document section, `Default for Config` is the one document the
//! shipped `config/default.toml` is checked against, and nothing here reads a file --
//! parsing, repair and checking live in the parent module and its own submodules.

use super::*;

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
    /// `on_im_switch`: what a live composition does when the input method is
    /// switched away from or reset.
    ///
    /// `Clear` -- the default -- takes the composing text back and commits
    /// nothing; `CommitRaw` commits the raw input first. See the type's own
    /// documentation for why the finer policies collapse onto `CommitRaw`.
    pub on_im_switch: ImSwitch,
    /// `correction`: whether a one-step typing slip is corrected for an input no exact
    /// spelling covers, so that `zhegn` reaches a word spelled `zheng`.
    ///
    /// Off by default: a corrected candidate is what the user asked for only by
    /// accident, so it rides a heavy penalty and never displaces a reading the input
    /// spells. The key is the configuration's half of the engine's
    /// `DecodeFlags::CORRECTION` switch (ADR-0014); the decoder reads no
    /// configuration, so the host layer turns this flag into that bit.
    pub correction: bool,
    /// `compose`: whether a dead-key sequence the host is composing -- an acute
    /// accent followed by `e`, for one -- is answered before the key table sees
    /// the key.
    ///
    /// Off by default: a compose pass that swallows keys changes what a European
    /// keyboard types into the application, and a user whose key table already
    /// claims those keys must not have them taken. The host the plugin runs
    /// under has to offer compose support for the switch to do anything at all;
    /// with a host that offers none, the switch is inert and every key routes
    /// exactly as it did.
    pub compose: bool,
    /// `mixed_english`: whether an input no pinyin reading cuts is offered the
    /// English lexicon's entry for it, so that `github` reaches the word spelled
    /// `GitHub`.
    ///
    /// Off by default: a user who never asked for English candidates must not have
    /// their compositions answered from a table they did not know was there. The
    /// key is the configuration's half of the engine's `DecodeFlags::ENGLISH`
    /// switch (ADR-0016); the decoder reads no configuration, so the host layer
    /// turns this flag into that bit.
    pub mixed_english: bool,
    /// `share_state`: how sessions are shared across input contexts.
    pub share_state: ShareState,
}

/// The `[fuzzy]` section: the fuzzy-equivalence classes the decoder widens with.
///
/// Every key is a bool and no value a bool can hold is invalid, so the section needs
/// no repair -- the only rule it carries is the projection's, which turns the section
/// into the decode flags and is `ime-core`'s to state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FuzzyConfig {
    /// `enabled`: the master switch. On, it selects every class at once.
    pub enabled: bool,
    /// `zh_z`: `zh` and `z` read as each other.
    pub zh_z: bool,
    /// `ch_c`: `ch` and `c` read as each other.
    pub ch_c: bool,
    /// `sh_s`: `sh` and `s` read as each other.
    pub sh_s: bool,
    /// `n_l`: `n` and `l` read as each other.
    pub n_l: bool,
    /// `f_h`: `f` and `h` read as each other.
    pub f_h: bool,
    /// `an_ang`: `an` and `ang` read as each other.
    pub an_ang: bool,
    /// `en_eng`: `en` and `eng` read as each other.
    pub en_eng: bool,
    /// `in_ing`: `in` and `ing` read as each other.
    pub in_ing: bool,
}

impl FuzzyConfig {
    /// The class bits this section selects, without the master switch.
    ///
    /// A `true` selects its class; the master switch is not consulted here because
    /// the projection is what OR-scope it over the selection. An all-false section
    /// answers an empty mask, which projects to no flags at all.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn class_flags(&self) -> ime_types::DecodeFlags {
        let mut flags = ime_types::DecodeFlags::empty();
        for (selected, flag) in [
            (self.zh_z, ime_types::DecodeFlags::FUZZY_ZH_Z),
            (self.ch_c, ime_types::DecodeFlags::FUZZY_CH_C),
            (self.sh_s, ime_types::DecodeFlags::FUZZY_SH_S),
            (self.n_l, ime_types::DecodeFlags::FUZZY_N_L),
            (self.f_h, ime_types::DecodeFlags::FUZZY_F_H),
            (self.an_ang, ime_types::DecodeFlags::FUZZY_AN_ANG),
            (self.en_eng, ime_types::DecodeFlags::FUZZY_EN_ENG),
            (self.in_ing, ime_types::DecodeFlags::FUZZY_IN_ING),
        ] {
            if selected {
                flags.insert(flag);
            }
        }
        flags
    }
}

impl Default for FuzzyConfig {
    /// The shipped values: everything off. A build with no `[fuzzy]` section at all
    /// decodes exactly as this default decodes.
    fn default() -> Self {
        Self {
            enabled: false,
            zh_z: false,
            ch_c: false,
            sh_s: false,
            n_l: false,
            f_h: false,
            an_ang: false,
            en_eng: false,
            in_ing: false,
        }
    }
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

/// The `[assoc]` section: next-word association candidates offered after a commit.
///
/// The typed `[assoc]` section projected into a session by the engine. The
/// document merge, repair rules and writeback all treat it like every other
/// internal (non-frozen) configuration section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssocConfig {
    /// `enabled`: whether a commit is followed by association candidates at all.
    ///
    /// Off by default: candidates appearing after every commit change what the
    /// window shows without the user asking for it, so the feature arrives as an
    /// explicit opt-in.
    pub enabled: bool,
    /// `max_candidates`: how many association candidates one commit may offer,
    /// at most.
    ///
    /// Five is a page's worth at the shipped layout; a bound rather than a stream,
    /// so an over-eager source cannot flood the window after every commit.
    pub max_candidates: u8,
    /// `history_depth`: how many recent commits the in-session history source may
    /// look back through.
    ///
    /// Session memory only -- the ring holds no more than this and is never
    /// persisted, so the bound is also the privacy statement: nothing outside the
    /// live session is readable from it.
    pub history_depth: u8,
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
    /// The `[fuzzy]` section.
    pub fuzzy: FuzzyConfig,
    /// The `[phrases]` section.
    pub phrases: PhraseConfig,
    /// The `[data]` section.
    pub data: DataConfig,
    /// The `[diagnostics]` section.
    pub diagnostics: DiagnosticsConfig,
    /// The `[assoc]` section.
    pub assoc: AssocConfig,
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
            fuzzy: FuzzyConfig::default(),
            engine: EngineConfig {
                punct_mode: PunctMode::Chinese,
                on_im_switch: ImSwitch::Clear,
                full_width: false,
                auto_english_on_uppercase: true,
                passthrough_url: true,
                max_raw_len: MAX_RAW_LEN,
                verify_dict_on_load: VerifyDictOnLoad::Full,
                abbrev: false,
                correction: false,
                mixed_english: false,
                compose: false,
                share_state: ShareState::Context,
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
                decay_half_life_ticks: DEFAULT_DECAY_HALF_LIFE_TICKS,
                decay_floor: 1 << 16,
                context_tie_band_q16: DEFAULT_CONTEXT_TIE_BAND_Q16,
                dict_staged_path: String::new(),
            },
            diagnostics: DiagnosticsConfig {
                level: LogLevel::Info,
                log_rotation_mb: 8,
                log_keep_files: 3,
                log_input_content: false,
                probes: true,
            },
            // Off and empty: a session that has committed nothing offers nothing
            // after the commit, which is what every configuration that predates
            // the section behaves like.
            assoc: AssocConfig {
                enabled: false,
                max_candidates: 5,
                history_depth: 8,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default_assoc_section_is_disabled_with_shipped_bounds() {
        // The values the section ships with, pinned here so a default that moves
        // is a conscious edit: the section is an opt-in feature, and a default
        // that drifted to enabled would put candidates after every commit for
        // users who never asked for them. The document-level drift test in
        // `reload` covers the same defaults from the template side; this one
        // holds the type side on its own.
        let assoc = Config::default().assoc;
        assert!(!assoc.enabled);
        assert_eq!(assoc.max_candidates, 5);
        assert_eq!(assoc.history_depth, 8);
    }

    #[test]
    fn test_config_default_assoc_section_needs_no_repair() {
        // The shipped section sits inside every range the repair rules hold, so
        // validating the defaults reports nothing: a fresh install never opens
        // with a repaired-value notice it did nothing to earn.
        assert!(Config::default().validate().is_empty());
    }

    #[test]
    fn test_document_assoc_section_is_parsed_and_repaired_by_key() {
        let document = concat!(
            "[assoc]\n",
            "enabled = true\n",
            "max_candidates = 0\n",
            "history_depth = 65\n",
        );
        let (config, warnings) = Config::from_document(document).expect("the document parses");

        assert!(config.assoc.enabled);
        assert_eq!(config.assoc.max_candidates, 5);
        assert_eq!(config.assoc.history_depth, 8);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        for (warning, key) in warnings.iter().zip(["assoc.max_candidates", "assoc.history_depth"]) {
            assert!(warning.to_string().contains(key), "{warning}");
        }
    }
}
