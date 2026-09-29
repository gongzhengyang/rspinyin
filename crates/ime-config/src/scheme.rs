//! The `[scheme]` configuration section.
//!
//! Responsibility: name the double-pinyin layout the user's keystrokes follow, and
//! carry the switches that shape a scheme session -- whether the candidate window's
//! header names the active layout, and whether a keystroke the layout cannot read is
//! still answered as full pinyin.
//!
//! Boundaries: everything here is pure (0.4 rule 4). It parses spellings that are
//! already in memory and never touches the filesystem, the clock or the environment.
//! The layout tables themselves live in `ime-core`, which this layer does not depend
//! on; the one thing that crosses is [`SchemeId`], the contract's number for a layout.
//!
//! # The keys of the section
//!
//! - `scheme.scheme`: which layout is active, one of the spellings
//!   [`SchemeChoice::parse`] accepts.
//! - `scheme.show_hint`: whether the candidate window's header names the layout.
//! - `scheme.keep_full_pinyin`: whether a keystroke the layout cannot read is read as
//!   full pinyin instead.
//! - `scheme.custom.initials` and `scheme.custom.finals`: the two lists of a layout
//!   the user writes out themselves.
//!
//! # Why `keep_full_pinyin` exists
//!
//! A user who switched to a double-pinyin layout and then types a full-pinyin
//! syllable gets nothing at all, which reads as "the input method broke" rather than
//! as "you are in the wrong mode". Every mainstream implementation therefore keeps
//! reading the occasional full-pinyin syllable inside a scheme session, and the key is
//! on by default for that reason.
//!
//! # Why a custom table is validated and then set aside
//!
//! `[scheme.custom]` is read and checked so that a user who writes one out is told
//! what is wrong with it, but the choice is repaired to full pinyin either way: the
//! frozen [`SchemeId`] numbering names the five published layouts and has no slot a
//! user's own table could travel in, so no build -- this one or a later one -- could
//! read that table back. Reporting the repair is the honest answer; decoding full
//! pinyin while the document still says `custom` would leave the user looking for a
//! layout that is not running.
//!
//! # What the layers below take from here
//!
//! The section reaches the rest of the plugin through two calls, so that the mapping
//! between a configuration key and the value a layer acts on lives here rather than in
//! the layer itself:
//!
//! - [`SchemeConfig::decode_settings`] is what the session state machine is configured
//!   from: the contract's layout number, and the mixed-input switch beside it.
//! - [`SchemeConfig::header_hint`] is what the candidate window's header shows, or
//!   nothing when the user turned the hint off.
//!
//! Neither call touches the custom table: a table this build cannot compile is repaired
//! away by [`SchemeConfig::repaired`] before either of them is reached.

use ime_types::{ConfigError, ImeError, SchemeId};

use crate::schema::Warnings;

/// The `scheme.scheme` key.
pub(crate) const KEY_SCHEME: &str = "scheme.scheme";
/// The `scheme.custom.initials` key.
pub(crate) const KEY_CUSTOM_INITIALS: &str = "scheme.custom.initials";
/// The `scheme.custom.finals` key.
pub(crate) const KEY_CUSTOM_FINALS: &str = "scheme.custom.finals";

/// The number of entries a custom table's `initials` and `finals` lists must hold: one
/// per letter of the `a`-`z` alphabet.
///
/// The `;` key that the shipped Microsoft, Sogou and Ziguang layouts carry `ing` on is
/// deliberately outside a custom table. A layout written out by hand is expected to
/// fit the 26 letters a keyboard shows, and leaving the column out is a limitation the
/// user can see, where silently reading a 27th entry as something else is not.
pub const CUSTOM_TABLE_KEYS: usize = 26;

/// The `config/invalid` diagnostic for one key of this section.
fn invalid(key: &str, reason: String) -> ConfigError {
    ConfigError::Invalid {
        key: String::from(key),
        reason,
    }
}

/// The `scheme.scheme` key: which layout the keystrokes follow.
///
/// The spellings are the ones a configuration document writes, and they are matched
/// case-insensitively: `"XiaoHe"` and `"xiaohe"` name the same layout, because the key
/// is written by hand and a capital letter is not a mistake worth refusing a document
/// over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SchemeChoice {
    /// Full pinyin: the input is used exactly as typed. The default, so a document
    /// that says nothing about schemes behaves the way it did before schemes existed.
    #[default]
    Full,
    /// Xiaohe, the most widely used double-pinyin layout.
    Xiaohe,
    /// Ziranma, the layout most later schemes were derived from.
    Ziranma,
    /// The layout Microsoft's input method ships.
    Microsoft,
    /// The layout Sogou's input method ships as its default.
    Sogou,
    /// The layout the Ziguang input method ships.
    Ziguang,
    /// A layout the user defines in `[scheme.custom]`.
    Custom,
}

impl SchemeChoice {
    /// Parses the value of the `scheme.scheme` key.
    ///
    /// # Parameters
    ///
    /// - `raw`: the value as the document spells it, in any case.
    ///
    /// # Returns
    ///
    /// The layout `raw` names.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Invalid`] naming `scheme.scheme` when `raw` is not one of the
    /// documented spellings. The caller answers it by keeping the default and
    /// reporting the diagnostic: a layout name the user mistyped is never a reason to
    /// refuse to start.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn parse(raw: &str) -> Result<Self, ConfigError> {
        match raw.to_ascii_lowercase().as_str() {
            "full" => Ok(Self::Full),
            "xiaohe" => Ok(Self::Xiaohe),
            "ziranma" => Ok(Self::Ziranma),
            "microsoft" => Ok(Self::Microsoft),
            "sogou" => Ok(Self::Sogou),
            "ziguang" => Ok(Self::Ziguang),
            "custom" => Ok(Self::Custom),
            // The reason quotes `raw` rather than the folded form, so the diagnostic
            // shows the user what they actually wrote.
            _ => Err(invalid(KEY_SCHEME, format!("unknown scheme: {raw}"))),
        }
    }

    /// The spelling the configuration document uses, which is what
    /// [`SchemeChoice::parse`] accepts.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Xiaohe => "xiaohe",
            Self::Ziranma => "ziranma",
            Self::Microsoft => "microsoft",
            Self::Sogou => "sogou",
            Self::Ziguang => "ziguang",
            Self::Custom => "custom",
        }
    }

    /// The short label the candidate window's header shows for this choice.
    ///
    /// User-facing copy, so it is Chinese like the rest of the window's text. It is
    /// what `scheme.show_hint` displays, and the reason that key exists at all: a user
    /// who forgot they switched layouts can see which one is answering their keys.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Full => "全拼",
            Self::Xiaohe => "小鹤",
            Self::Ziranma => "自然码",
            Self::Microsoft => "微软",
            Self::Sogou => "搜狗",
            Self::Ziguang => "紫光",
            Self::Custom => "自定义",
        }
    }

    /// Whether the choice names a double-pinyin layout rather than full pinyin.
    ///
    /// This is the switch that decides whether the engine sets
    /// `DecodeFlags::SHUANGPIN` and fills `DecodeRequest::scheme`: full pinyin is the
    /// identity and needs neither.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn is_double_pinyin(self) -> bool {
        !matches!(self, Self::Full)
    }
}

impl TryFrom<&str> for SchemeChoice {
    type Error = ConfigError;

    /// Parses the value of the `scheme.scheme` key.
    ///
    /// The document loader reads every closed set of spellings through this trait, so
    /// implementing it is what lets the section be merged like every other one.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Invalid`] naming `scheme.scheme`; see [`SchemeChoice::parse`].
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl From<SchemeChoice> for SchemeId {
    /// The contract's number for the layout.
    ///
    /// [`SchemeChoice::Custom`] maps to [`SchemeId::FULL`] because the frozen
    /// numbering has no slot for a layout this build cannot compile: the five numbers
    /// past `FULL` are the five published layouts, and a table no other build can
    /// interpret must not borrow one of their numbers. A repaired configuration has
    /// already replaced `Custom` with `Full` (see [`SchemeConfig::repaired`]), so this
    /// arm answers only for a value that has not been through the repair rules.
    fn from(choice: SchemeChoice) -> Self {
        match choice {
            SchemeChoice::Full | SchemeChoice::Custom => Self::FULL,
            SchemeChoice::Xiaohe => Self::XIAOHE,
            SchemeChoice::Ziranma => Self::ZIRANMA,
            SchemeChoice::Microsoft => Self::MICROSOFT,
            SchemeChoice::Sogou => Self::SOGOU,
            SchemeChoice::Ziguang => Self::ZIGUANG,
        }
    }
}

/// The `[scheme.custom]` table: a layout the user writes out themselves.
///
/// Both lists are indexed by the letter they belong to: entry `i` says what the `i`-th
/// letter of the alphabet stands for, so `initials[25]` is what `z` opens a syllable
/// with and `finals[25]` is what it closes one with. An empty entry means the key
/// carries nothing in that position, exactly as an empty entry in a shipped table
/// does.
///
/// The table is validated and then set aside; see the module documentation for why.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CustomSchemeConfig {
    /// `initials`: what each of the 26 letters stands for as a syllable initial.
    pub initials: Vec<String>,
    /// `finals`: what each of the 26 letters stands for as a syllable final.
    pub finals: Vec<String>,
}

impl CustomSchemeConfig {
    /// Reports every list that is not a usable table.
    ///
    /// A list of the wrong length is not read at all: a table indexed by the wrong
    /// alphabet would map some keys to the wrong letters rather than fail, and a
    /// layout that is silently shifted by one position is worse than no layout.
    ///
    /// # Returns
    ///
    /// One `config/invalid` diagnostic per unusable list, naming that list's key, and
    /// an empty list when both are usable. A table that was never written out has two
    /// empty lists and so names both keys.
    ///
    /// # Errors
    ///
    /// None: an unusable table is a diagnostic, not a failure.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn validate(&self) -> Vec<ImeError> {
        // A table the document never wrote is not a table with a mistake in it. The
        // shipped default has no custom layout at all, and reporting both lists as
        // short would put two diagnostics on a configuration nobody has touched.
        if self.initials.is_empty() && self.finals.is_empty() {
            return Vec::new();
        }
        let mut warnings = Vec::new();
        for (key, list) in [
            (KEY_CUSTOM_INITIALS, &self.initials),
            (KEY_CUSTOM_FINALS, &self.finals),
        ] {
            if list.len() != CUSTOM_TABLE_KEYS {
                warnings.push(ImeError::from(invalid(
                    key,
                    format!("expected {CUSTOM_TABLE_KEYS} entries, found {}", list.len()),
                )));
            }
        }
        warnings
    }
}

/// The `[scheme]` section.
///
/// Not `Copy`, unlike the section's scalar keys on their own: the custom table is a
/// pair of lists read out of the document, and a section that carries a user's table
/// cannot be a `Copy` value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemeConfig {
    /// `scheme`: which layout the keystrokes follow.
    pub scheme: SchemeChoice,
    /// `show_hint`: whether the candidate window's header names the active layout.
    pub show_hint: bool,
    /// `keep_full_pinyin`: whether a keystroke the layout cannot read is read as full
    /// pinyin.
    ///
    /// Without this, a user who switched to a scheme and then typed a full-pinyin
    /// syllable gets nothing, which reads as "the input method broke".
    pub keep_full_pinyin: bool,
    /// The `[scheme.custom]` table.
    pub custom: CustomSchemeConfig,
}

impl Default for SchemeConfig {
    /// The shipped defaults: full pinyin, the header names the layout, and a
    /// full-pinyin syllable typed inside a scheme session is still read.
    ///
    /// `show_hint` and `keep_full_pinyin` are on although the default layout is full
    /// pinyin, where neither has anything to do: the defaults describe the section a
    /// user gets after switching the layout on, which is the moment both keys matter.
    fn default() -> Self {
        Self {
            scheme: SchemeChoice::Full,
            show_hint: true,
            keep_full_pinyin: true,
            custom: CustomSchemeConfig::default(),
        }
    }
}

impl SchemeConfig {
    /// Replaces every value that breaks a rule with its built-in default.
    ///
    /// # Returns
    ///
    /// The usable section, and one diagnostic per key that was replaced. An empty
    /// diagnostic list means the section was valid.
    ///
    /// # Errors
    ///
    /// None: an unusable value is repaired rather than rejected, because a
    /// configuration must never be able to stop the input method from working.
    ///
    /// # Panics
    ///
    /// Never: every rule is a length test on a value that is already in memory.
    pub fn repaired(self) -> (Self, Vec<ImeError>) {
        let mut warnings = Warnings::default();
        let repaired = self.repair(&mut warnings);
        (repaired, warnings.entries)
    }

    /// Reports every key of this section that breaks a rule, without changing
    /// anything.
    ///
    /// # Returns
    ///
    /// One `config/invalid` diagnostic per unusable key, and an empty list when the
    /// section is valid: the read-only view of [`SchemeConfig::repaired`].
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

    /// The two settings the decode runs under, as the session state machine takes them.
    ///
    /// The projection this section exists for: every other key of it shapes the window
    /// rather than the decode, so this pair is the whole of what a host layer has to
    /// carry over. The layout travels as the contract's own number rather than as
    /// [`SchemeChoice`], which is the mapping [`From<SchemeChoice> for SchemeId`] owns:
    /// keeping it here is what leaves a caller free to read the section's keys without
    /// also having to know that a layout this build cannot compile maps onto full pinyin.
    ///
    /// # Returns
    ///
    /// The layout the keystrokes follow, and whether a keystroke the layout cannot read
    /// is read as full pinyin. [`SchemeChoice::Custom`] answers [`SchemeId::FULL`], the
    /// one layout a build with no custom table can honour; see the module documentation
    /// for why a custom table is set aside rather than carried.
    ///
    /// # Errors
    ///
    /// None: the section is usable as it stands, because [`SchemeConfig::repaired`] has
    /// already replaced every value that broke a rule.
    ///
    /// # Panics
    ///
    /// Never.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_config::scheme::{SchemeChoice, SchemeConfig};
    /// use ime_types::SchemeId;
    ///
    /// let section = SchemeConfig {
    ///     scheme: SchemeChoice::Xiaohe,
    ///     ..SchemeConfig::default()
    /// };
    ///
    /// assert_eq!(section.decode_settings(), (SchemeId::XIAOHE, true));
    /// ```
    pub fn decode_settings(&self) -> (SchemeId, bool) {
        (SchemeId::from(self.scheme), self.keep_full_pinyin)
    }

    /// The label the candidate window's header shows for the active layout, if any.
    ///
    /// The projection `scheme.show_hint` exists for. The header names the layout so that
    /// a user who forgot they switched can see which one is answering their keys; a user
    /// who finds the label noise turns it off, and `None` is that case -- the engine then
    /// shows its own Chinese / English label instead of a layout name.
    ///
    /// The label itself is [`SchemeChoice::hint`], which is user-facing copy and so
    /// written in Chinese like the rest of the window's text.
    ///
    /// # Returns
    ///
    /// The label, or `None` when `show_hint` is off.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_config::scheme::{SchemeChoice, SchemeConfig};
    ///
    /// let quiet = SchemeConfig {
    ///     scheme: SchemeChoice::Xiaohe,
    ///     show_hint: false,
    ///     ..SchemeConfig::default()
    /// };
    ///
    /// assert_eq!(quiet.header_hint(), None);
    /// assert_eq!(SchemeConfig::default().header_hint(), Some("全拼"));
    /// ```
    pub fn header_hint(&self) -> Option<&'static str> {
        self.show_hint.then(|| self.scheme.hint())
    }

    /// The repair rules, writing into a caller's collector.
    ///
    /// `Config::repaired` calls this so that the section's diagnostics join the rest in
    /// one list, in schema order: `scheme.scheme` first, then the custom table's two
    /// lists.
    ///
    /// # Panics
    ///
    /// Never.
    pub(crate) fn repair(mut self, warnings: &mut Warnings) -> Self {
        if self.scheme == SchemeChoice::Custom {
            // The table is checked below, but the choice is set aside either way: the
            // frozen `SchemeId` numbering has no slot a custom layout could travel in,
            // so no build could read the table back even when it is perfect.
            warnings.report(
                KEY_SCHEME,
                String::from("custom layouts are not implemented in this build"),
            );
            self.scheme = SchemeChoice::Full;
        }
        for error in self.custom.validate() {
            warnings.report_ime_error(error);
        }
        self
    }
}

#[cfg(test)]
mod tests;
