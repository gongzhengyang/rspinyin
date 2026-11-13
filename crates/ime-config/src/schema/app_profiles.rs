//! The `[app_profiles]` section: the session initial values one application starts with.
//!
//! Responsibility: the typed form of the section, the key format its entries are named
//! by, and the repair rules that keep an unusable entry from reaching a session. A
//! profile is keyed by the application identifier's hash -- the same hashed identifier
//! the privacy layer computes and the diagnostics render, never the clear program name,
//! which is the one layer that is allowed to see it.
//!
//! The key is the hash channel's own rendering: `0x` followed by sixteen lower-case hex
//! digits. One canonical spelling, the one the hash tool prints, is accepted -- a bare
//! or upper-case spelling would let one application be configured twice under two keys.
//! The identifier that means "the host reported nothing" (`0x0000000000000000`) is not a
//! key a profile may use: an application the plugin knows nothing about gets the shipped
//! initial values, which is the fail-closed reading of "we do not know what program this
//! is".
//!
//! Every value of a profile is optional, and an unset value means "no opinion": the
//! session starts exactly as a build before the section existed starts. A profile is a
//! creation parameter only -- it is projected onto a session when the session is created,
//! and never again, so a preset cannot fight the switches the user flips while typing.
//!
//! Boundaries: plain data and pure decisions. Nothing here reads a file, a clock or the
//! environment; the document merge copies what the user wrote, and [`AppProfilesConfig::repair`]
//! drops what cannot be used.

use std::collections::BTreeMap;

use super::*;

/// The key spelling `app_profiles.profiles.script` is reported against.
const KEY_APP_PROFILES_SCRIPT: &str = "app_profiles.profiles.script";

/// The key prefix every per-profile diagnostic is reported under, completed with the
/// entry's own key so the user can find the offending line in the document.
const KEY_APP_PROFILES: &str = "app_profiles.profiles";

/// The key spelling of "the host reported no application", which no profile may use.
const UNKNOWN_APP_KEY: &str = "0x0000000000000000";

/// The most entries `[app_profiles.profiles]` may hold.
///
/// A preset per application the user types into is a handful; sixteen leaves room for a
/// whole desktop of them while keeping the routing layer's copy of the table a fixed
/// handful of slots. The surplus is dropped and reported, which is the section's
/// degradation path.
pub const MAX_APP_PROFILES: usize = 16;

/// Which script variant a profile's commits start out converting to.
///
/// The whitelist is the two variants the engine can convert between; a third spelling
/// would be accepted and then answered by nothing, so it is refused while the document
/// is read and the field stays unset -- the shipped simplified initial.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppScriptPref {
    /// The `simplified` spelling: commits leave as the dictionary wrote them, which is
    /// the engine's shipped behaviour and the initial every profile without the key
    /// gets.
    Simplified,
    /// The `traditional` spelling: commits convert through the variant table on their
    /// way to the application, exactly as the script toggle's traditional state does.
    Traditional,
}

impl AppScriptPref {
    /// Parses the value of a profile's `script` key.
    ///
    /// # Parameters
    ///
    /// - `raw`: the spelling as the document wrote it.
    ///
    /// # Returns
    ///
    /// The variant the spelling names.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Invalid`] naming `app_profiles.profiles.script` when the spelling
    /// is not in the whitelist.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn parse(raw: &str) -> Result<Self, ConfigError> {
        match raw {
            "simplified" => Ok(Self::Simplified),
            "traditional" => Ok(Self::Traditional),
            other => Err(invalid(
                KEY_APP_PROFILES_SCRIPT,
                format!("unknown script initial: {other}"),
            )),
        }
    }

    /// The spelling the configuration file uses, which is what [`AppScriptPref::parse`]
    /// accepts.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Simplified => "simplified",
            Self::Traditional => "traditional",
        }
    }
}

/// The session initial values one application's profiles entry names.
///
/// Every field is optional, and `None` is the shipped behaviour: a profile that names
/// nothing is a profile that changes nothing. The fields are the switches a session can
/// start in -- the language mode, the script conversion and the two width switches --
/// and no more: values the session derives from the configuration, page size and layout
/// among them, are not per-application choices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppProfile {
    /// `ascii`: whether the session starts handing every key to the application, which
    /// is the English initial a terminal or a shell wants. The mode that carries it is
    /// the temporary-English one, so the keys pass through until `Return` or `Escape`
    /// leaves it -- the same exit the chord-entered mode has.
    pub ascii: Option<bool>,
    /// `script`: the script variant the session's commits start converting to.
    pub script: Option<AppScriptPref>,
    /// `full_width`: whether the session starts widening the ASCII it commits.
    pub full_width: Option<bool>,
    /// `punct_full`: whether the session starts committing Chinese punctuation.
    pub punct_full: Option<bool>,
}

impl AppProfile {
    /// The profile that names nothing; it is what an empty routing slot holds, and an
    /// empty slot matches no application.
    ///
    /// # Panics
    ///
    /// Never.
    pub const EMPTY: Self = Self {
        ascii: None,
        script: None,
        full_width: None,
        punct_full: None,
    };
}

/// The `[app_profiles]` section: per-application session initial values, keyed by the
/// applications' hashed identifiers.
///
/// The map is ordered by key so a document round-trips through diagnostics and tests
/// deterministically, and the lookup is a `BTreeMap` probe over a bounded table -- a
/// constant handful of short-string compares on the session-creation path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppProfilesConfig {
    /// The `[app_profiles.profiles]` table: hash key -> the initial values.
    pub profiles: BTreeMap<String, AppProfile>,
}

/// The configuration key a hashed application identifier is spelled as.
///
/// This is the one place the format is written down: the loader validates a document
/// key against [`is_app_profile_key`], the hash tool prints a key with this function,
/// and the engine's lookup formats the identifier it was handed through it -- so the
/// file, the tool and the lookup cannot spell the key three different ways.
///
/// # Parameters
///
/// - `raw`: the 64-bit hash the privacy layer computed for an application.
///
/// # Returns
///
/// The key: `0x` followed by sixteen lower-case hex digits.
///
/// # Panics
///
/// Never.
pub fn app_profile_key(raw: u64) -> String {
    format!("0x{raw:016x}")
}

/// Whether `key` is a hashed application identifier a profile may be named by.
///
/// The one accepted spelling is the hash channel's own rendering, and the identifier
/// that means "no application was reported" is refused outright: a profile keyed on it
/// would apply to every context the host said nothing about.
///
/// # Parameters
///
/// - `key`: the key as the document wrote it.
///
/// # Returns
///
/// Whether the key can name a profile.
///
/// # Panics
///
/// Never.
pub fn is_app_profile_key(key: &str) -> bool {
    if key == UNKNOWN_APP_KEY {
        return false;
    }
    match key.strip_prefix("0x") {
        Some(digits) => {
            digits.len() == 16
                && digits
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        }
        None => false,
    }
}

impl AppProfilesConfig {
    /// The profile `app`'s hash names, or `None` when it names none.
    ///
    /// The unknown identifier is answered `None` before the table is consulted, so an
    /// application the host never reported can never inherit a preset -- not even the
    /// `0x0000000000000000` key, which the section refuses to hold in the first place.
    ///
    /// # Parameters
    ///
    /// - `app`: the 64-bit hash the privacy layer computed for the application.
    ///
    /// # Returns
    ///
    /// The profile the section holds for the application.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn profile_for_app(&self, app: u64) -> Option<&AppProfile> {
        if app == 0 {
            return None;
        }
        self.profiles.get(&app_profile_key(app))
    }

    /// Drops every entry this section cannot use, and reports one diagnostic per drop.
    ///
    /// A key that is not a hashed identifier cannot be repaired -- there is no default
    /// hash to fall back to -- so the entry leaves the table whole, with its reason
    /// named after the key the document wrote. The entries past the bound leave the
    /// same way, last key first, under one limit diagnostic.
    ///
    /// # Panics
    ///
    /// Never.
    pub(crate) fn repair(&mut self, warnings: &mut Warnings) {
        let profiles = std::mem::take(&mut self.profiles);
        let mut kept = BTreeMap::new();
        for (key, profile) in profiles {
            if !is_app_profile_key(&key) {
                warnings.report(
                    &format!("{KEY_APP_PROFILES}.{key}"),
                    format!("not a hashed application identifier: {key}"),
                );
                continue;
            }
            kept.insert(key, profile);
        }
        if kept.len() > MAX_APP_PROFILES {
            while kept.len() > MAX_APP_PROFILES {
                let _ = kept.pop_last();
            }
            warnings.report_limit(KEY_APP_PROFILES, MAX_APP_PROFILES);
        }
        self.profiles = kept;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A placeholder hash a test configures, with no application behind it: the
    /// discipline is that no clear program name reaches a test string, so every key
    /// here is a literal.
    const HASH_A: u64 = 0x1234_5678_9abc_def0;

    /// A second placeholder hash, for the tests that tell two applications apart.
    const HASH_B: u64 = 0xfedc_ba98_7654_3210;

    #[test]
    fn test_app_profile_key_formats_the_hash_channels_rendering() {
        assert_eq!(app_profile_key(0), UNKNOWN_APP_KEY);
        assert_eq!(app_profile_key(HASH_A), "0x123456789abcdef0");
        assert_eq!(app_profile_key(u64::MAX), "0xffffffffffffffff");
    }

    #[test]
    fn test_is_app_profile_key_accepts_only_the_canonical_spelling() {
        assert!(is_app_profile_key("0x123456789abcdef0"));
        assert!(is_app_profile_key("0x0000000000000001"));
        // The canonical spelling's boundaries: no bare form, no upper case, no other
        // width, no other alphabet, no other prefix.
        assert!(!is_app_profile_key("123456789abcdef0"));
        assert!(!is_app_profile_key("0X123456789abcdef0"));
        assert!(!is_app_profile_key("0x123456789ABCDEF0"));
        assert!(!is_app_profile_key("0x123456789abcdef"));
        assert!(!is_app_profile_key("0x123456789abcdef00"));
        assert!(!is_app_profile_key("0x123456789abcdefg"));
        assert!(!is_app_profile_key(""));
    }

    #[test]
    fn test_is_app_profile_key_refuses_the_unknown_identifier() {
        // The fail-closed rule: an entry keyed on "no application was reported" would
        // apply to every context the host said nothing about, so the key is refused
        // wherever the section reads one.
        assert!(!is_app_profile_key(UNKNOWN_APP_KEY));
    }

    #[test]
    fn test_profile_for_app_hits_and_misses_by_the_hash() {
        let mut section = AppProfilesConfig::default();
        let profile = AppProfile {
            ascii: Some(true),
            ..AppProfile::EMPTY
        };
        section.profiles.insert(app_profile_key(HASH_A), profile);

        assert_eq!(section.profile_for_app(HASH_A), Some(&profile));
        assert_eq!(section.profile_for_app(HASH_B), None);
        // The unknown identifier never reaches the table, whatever the table holds.
        assert_eq!(section.profile_for_app(0), None);
    }

    #[test]
    fn test_repair_drops_a_key_that_is_not_a_hash_and_reports_it() {
        // The refused key is a clear-text token rather than a hash: exactly the shape
        // the section exists to keep out, and no program name of anyone's.
        let mut section = AppProfilesConfig::default();
        section
            .profiles
            .insert(String::from("preset-key"), AppProfile::EMPTY);
        section
            .profiles
            .insert(app_profile_key(HASH_A), AppProfile::EMPTY);

        let mut warnings = Warnings::default();
        section.repair(&mut warnings);

        assert_eq!(section.profiles.len(), 1);
        assert!(section.profiles.contains_key(&app_profile_key(HASH_A)));
        let keys: Vec<String> = warnings
            .entries
            .iter()
            .filter_map(|warning| match warning {
                ImeError::ConfigInvalid { key, .. } => Some(key.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            keys,
            [String::from("app_profiles.profiles.preset-key")],
            "the diagnostic names the entry's own key so the line is findable"
        );
    }

    #[test]
    fn test_repair_drops_the_surplus_past_the_bound_under_one_limit() {
        let mut section = AppProfilesConfig::default();
        for raw in 1..=u64::from(MAX_APP_PROFILES as u8 + 1) {
            section
                .profiles
                .insert(app_profile_key(raw), AppProfile::EMPTY);
        }

        let mut warnings = Warnings::default();
        section.repair(&mut warnings);

        assert_eq!(section.profiles.len(), MAX_APP_PROFILES);
        // The kept entries are the first sixteen in key order, which is what makes the
        // drop deterministic; the surplus is the seventeenth and last.
        assert!(
            !section
                .profiles
                .contains_key(&app_profile_key(u64::from(MAX_APP_PROFILES as u8 + 1)))
        );
        assert_eq!(warnings.entries.len(), 1);
        let (key, _) = match &warnings.entries[0] {
            ImeError::ConfigInvalid { key, reason } => (key, reason),
            other => panic!("the surplus is a config diagnostic, not {other:?}"),
        };
        assert_eq!(key, KEY_APP_PROFILES, "the limit names the section");
    }

    #[test]
    fn test_document_app_profiles_section_is_read_whole() {
        let document = concat!(
            "[app_profiles.profiles]\n",
            "\"0x123456789abcdef0\" = { ascii = true, script = \"traditional\" }\n",
            "\"0xfedcba9876543210\" = { full_width = true, punct_full = false }\n",
        );
        let (config, warnings) = Config::from_document(document).expect("the document parses");

        assert!(warnings.is_empty(), "{warnings:?}");
        let first = config
            .app_profiles
            .profile_for_app(HASH_A)
            .expect("the first entry is read");
        assert_eq!(first.ascii, Some(true));
        assert_eq!(first.script, Some(AppScriptPref::Traditional));
        assert_eq!(first.full_width, None);
        let second = config
            .app_profiles
            .profile_for_app(HASH_B)
            .expect("the second entry is read");
        assert_eq!(second.full_width, Some(true));
        assert_eq!(second.punct_full, Some(false));
    }

    #[test]
    fn test_document_app_profile_bad_script_spelling_keeps_the_field_unset() {
        // The value-level repair: a spelling outside the whitelist leaves the field at
        // "no opinion" -- the entry stays, the other fields stay, and the diagnostic
        // names the key the spelling came from.
        let document = concat!(
            "[app_profiles.profiles]\n",
            "\"0x123456789abcdef0\" = { ascii = true, script = \"hans\" }\n",
        );
        let (config, warnings) = Config::from_document(document).expect("the document parses");

        let profile = config
            .app_profiles
            .profile_for_app(HASH_A)
            .expect("the entry stays");
        assert_eq!(profile.ascii, Some(true), "the untouched field stays");
        assert_eq!(profile.script, None, "the refused value leaves no opinion");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].to_string().contains(KEY_APP_PROFILES_SCRIPT),
            "the diagnostic names the script key: {warnings:?}"
        );
    }

    #[test]
    fn test_document_app_profile_bad_key_is_dropped_with_a_diagnostic() {
        // The key-level repair: a clear-text key can never become a hash, so the entry
        // is dropped whole and reported under the key it was written under. The refused
        // key is a neutral token, not a program name of anyone's.
        let document = concat!(
            "[app_profiles.profiles]\n",
            "\"preset-key\" = { ascii = true }\n",
            "\"0x123456789abcdef0\" = { ascii = true }\n",
        );
        let (config, warnings) = Config::from_document(document).expect("the document parses");

        assert_eq!(config.app_profiles.profiles.len(), 1);
        assert!(config.app_profiles.profile_for_app(HASH_A).is_some());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0]
                .to_string()
                .contains("app_profiles.profiles.preset-key"),
            "the diagnostic names the offending entry: {warnings:?}"
        );
    }

    #[test]
    fn test_document_app_profiles_default_is_empty_and_needs_no_repair() {
        // A build with no section at all runs with no presets, and validating the
        // defaults reports nothing: a fresh install never opens with a repaired-value
        // notice it did nothing to earn.
        assert!(Config::default().app_profiles.profiles.is_empty());
        assert!(Config::default().validate().is_empty());
    }
}
