//! The layout data model: the three tables a double-pinyin layout is defined by,
//! the TOML document they travel in, and the validator that decides whether a
//! document may become a [`SchemeTable`].
//!
//! Responsibility: parse a layout document (`[layout]`, `[initials]`, `[finals]`
//! and an optional `[standalone]` table), validate it against the syllable table,
//! and compile it into the table shape the rewrite walks. The five published
//! layouts this build ships are themselves data in [`data`](self) -- see
//! `crate::shuangpin::builtin` -- so a scheme and a user's own layout go through
//! exactly the same loader and the same validator.
//!
//! # Fail fast, then fall back
//!
//! A layout is data, not key-value settings: a wrong layout must never half-work,
//! because a layout that maps *some* keys wrongly is worse than one that maps
//! nothing. A document with any error is refused whole, and the caller answers by
//! falling back to the built-in scheme the user selected. Every error is reported
//! as one `config/invalid` diagnostic: conflicting mappings, an unreachable
//! final, an invalid key name and the two zero-initial schools mixed together are
//! each named on their own.
//!
//! # The two zero-initial schools
//!
//! A vowel-initial syllable (`a`, `ai`, `an`, ...) has no consonant to type
//! first, and the published layouts solve that in two mutually exclusive ways:
//!
//! - `o-led`: one key -- `o` in every layout that uses the school -- is the zero
//!   initial. It opens the syllable and contributes nothing to its spelling, so
//!   `an` is `o` followed by the `an` key.
//! - `first-letter`: the syllable is opened with its own first letter, so `an` is
//!   the `a` key followed by the `an` key; `a`, `e` and `o` carry themselves as
//!   initials.
//!
//! The declared school and the declared initials must agree, and a document that
//! mixes the two -- a zero initial beside the first-letter rule, or an `o-led`
//! declaration without the zero initial -- is refused.
//!
//! # Boundaries
//!
//! Parsing and validation are pure functions of the document text and the syllable
//! table (0.4 rule 4): no file, no clock, no environment, no global state. The
//! load itself belongs to the configuration-reload context and runs once per
//! reload; the compiled table is what the decode hot path reads, and the decode
//! stays a table lookup with no allocation of its own.
//!
//! # Privacy
//!
//! A layout document is the user's data. Diagnostics name configuration keys and
//! the linguistic spellings of the syllable table (an initial or a final, which
//! are the language's, not the user's), and never quote a table entry's value.

use std::collections::BTreeMap;

use ime_types::{ImeError, SchemeId};
use serde::Deserialize;

use super::{SCHEME_KEYS, SchemeTable, compose_into, key_index};
use crate::segment::syllable::{SYLLABLES, lookup, syllable_at};

/// The configuration key every layout diagnostic is reported under.
///
/// One key for the whole document, because the errors of the three tables are
/// errors of one layout; the reason names the table and the entry.
pub const LAYOUT_DIAGNOSTIC_KEY: &str = "shuangpin.custom.layout";

/// The syllable initials a layout may name, the zero initial included and the
/// three vowels beside them -- the first-letter school opens a vowel-initial
/// syllable with its own letter, so `a`, `e` and `o` are initials there.
///
/// A closed set on purpose: a spelling outside it is a typo the loader reports
/// rather than an initial the syllable table would never match.
const KNOWN_INITIALS: [&str; 27] = [
    "", "a", "e", "o", "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x", "r",
    "z", "c", "s", "y", "w", "zh", "ch", "sh",
];

/// The one-letter entries of [`KNOWN_INITIALS`], which is what a syllable's first
/// byte is tested against when the table is decomposed.
const SINGLE_INITIALS: [&str; 20] = [
    "b", "p", "m", "f", "d", "t", "n", "l", "g", "k", "h", "j", "q", "x", "r", "z", "c", "s", "y",
    "w",
];

/// The syllables a valid layout may leave unreachable, and why each one may be.
///
/// `m`, `n`, `ng`, `hm`, `hng` and `ê` are the interjection syllables the table
/// carries beside the ordinary ones; the published layouts put them on no key of
/// their own (`hng` happens to be reachable through `h` + `ng` in several, but
/// none of them is required). `lo` is crowded out by `luo` on the shared `uo`/`o`
/// key of every published layout, whose finals list `uo` first on purpose. A
/// custom layout is held to the same bar as the published ones, so these are the
/// only syllables it may miss.
const OPTIONAL_SYLLABLES: [&str; 7] = ["m", "n", "ng", "hm", "hng", "ê", "lo"];

/// The `[layout]` table of a layout document: the name and the zero-initial school.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct LayoutMeta {
    /// `name`: the display name of the layout. User-facing copy, so it is written
    /// the way the window shows it.
    name: Option<String>,
    /// `zero_initial_style`: which of the two schools the layout belongs to.
    zero_initial_style: Option<String>,
}

/// A double-pinyin layout as a document: the three tables and the meta block.
///
/// The fields are private so that the only way to compile one is
/// [`compile_layout`], which is where the validation lives. A caller that holds
/// the tables as typed values rather than as TOML text -- the configuration's
/// inline `[shuangpin.custom]` tables, for one -- builds a document with
/// [`LayoutDocument::from_tables`].
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct LayoutDocument {
    /// The `[layout]` table. A key inside it that the format does not define is
    /// ignored here and caught by the `name`/`zero_initial_style` checks instead,
    /// which is where its absence is a reportable mistake.
    layout: LayoutMeta,
    /// The `[initials]` table: key -> the initial the key opens a syllable with.
    initials: BTreeMap<String, String>,
    /// The `[finals]` table: key -> the finals the key closes a syllable with, in
    /// trial order, space-separated.
    finals: BTreeMap<String, String>,
    /// The `[standalone]` table: key -> the syllable the key spells on its own.
    standalone: BTreeMap<String, String>,
}

/// The three tables and the two meta fields of a layout, as plain data.
///
/// The typed form of a layout the caller already holds outside TOML: the
/// configuration's inline `[shuangpin.custom]` tables arrive exactly like this.
/// Every field is optional, and `None`/empty means "not written", which the
/// validator reports like a document that left it out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LayoutTables {
    /// The display name of the layout.
    pub name: Option<String>,
    /// Which zero-initial school the layout belongs to.
    pub zero_initial_style: Option<String>,
    /// Key -> the initial the key opens a syllable with.
    pub initials: BTreeMap<String, String>,
    /// Key -> the finals the key closes a syllable with, in trial order,
    /// space-separated.
    pub finals: BTreeMap<String, String>,
    /// Key -> the syllable the key spells on its own.
    pub standalone: BTreeMap<String, String>,
}

/// The `[layout] zero_initial_style` key: which of the two schools a layout
/// belongs to.
///
/// The schools are mutually exclusive by definition -- one spells a vowel-initial
/// syllable with a dedicated zero initial, the other with the syllable's own
/// first letter -- so a layout belongs to exactly one, and a document whose
/// initials disagree with its declaration is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZeroInitialStyle {
    /// The school that spells a vowel-initial syllable with the zero initial on
    /// the `o` key: `an` is `o` followed by the `an` key.
    OLed,
    /// The school that spells a vowel-initial syllable with its own first letter:
    /// `an` is the `a` key followed by the `an` key.
    FirstLetter,
}

impl ZeroInitialStyle {
    /// Parses the value of the `layout.zero_initial_style` key.
    ///
    /// # Parameters
    ///
    /// - `spelling`: the value as the document spells it.
    ///
    /// # Returns
    ///
    /// The school `spelling` names, or `None` for anything else. The two
    /// spellings are the format's whole set; there is no default school, because
    /// a document that does not declare one cannot be checked for mixing.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn parse(spelling: &str) -> Option<Self> {
        match spelling {
            "o-led" => Some(Self::OLed),
            "first-letter" => Some(Self::FirstLetter),
            _ => None,
        }
    }

    /// The spelling the configuration document uses, which is what
    /// [`ZeroInitialStyle::parse`] accepts.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OLed => "o-led",
            Self::FirstLetter => "first-letter",
        }
    }
}

impl LayoutDocument {
    /// Builds a document from tables the caller holds as typed values.
    ///
    /// This is the entry point for a layout that did not travel as TOML text; the
    /// parsed and the typed form are equal once built, so the validator treats
    /// them identically.
    ///
    /// # Parameters
    ///
    /// - `tables`: the meta fields and the three tables, as the configuration
    ///   section holds them.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_tables(tables: LayoutTables) -> Self {
        Self {
            layout: LayoutMeta {
                name: tables.name,
                zero_initial_style: tables.zero_initial_style,
            },
            initials: tables.initials,
            finals: tables.finals,
            standalone: tables.standalone,
        }
    }
}

/// Parses a layout document from its TOML text.
///
/// # Parameters
///
/// - `text`: the contents of a layout document, as `[shuangpin.custom.file]`
///   points at one.
///
/// # Returns
///
/// The document, or one diagnostic when the text is not a valid layout document.
/// The diagnostic carries the line the error sits on and never the text of the
/// value it involves, because a layout document is the user's data.
///
/// # Errors
///
/// A diagnostic under [`LAYOUT_DIAGNOSTIC_KEY`] when `text` is not TOML, holds a
/// value of the wrong type, or names a table or key the format does not define.
///
/// # Panics
///
/// Never.
pub fn parse_layout(text: &str) -> Result<LayoutDocument, Vec<ImeError>> {
    toml::from_str(text).map_err(|error| vec![invalid(parse_error_reason(text, &error))])
}

/// Validates a layout document and compiles it into a scheme table.
///
/// Validation is fail-fast in the aggregate sense: every error is reported, one
/// `config/invalid` diagnostic each, and the document is refused whole if there
/// was any. The caller answers a refusal by falling back to the built-in scheme
/// the user selected -- a layout that would map some keys wrongly must never
/// load.
///
/// # Parameters
///
/// - `document`: the layout, parsed or built from typed tables.
/// - `id`: the scheme number the compiled table identifies itself by. A custom
///   layout has no number of its own in the frozen numbering; its caller passes
///   [`SchemeId::FULL`], which [`table_for`](super::table_for) never resolves, so
///   a compiled custom table cannot be mistaken for a published one through a
///   lookup.
///
/// # Returns
///
/// The compiled table.
///
/// # Errors
///
/// One `config/invalid` diagnostic per error, under [`LAYOUT_DIAGNOSTIC_KEY`]: a
/// missing or empty table, an invalid key name, an initial or final the syllable
/// table does not know, an initial claimed by two keys, the declared school
/// disagreeing with the declared initials, and syllables no keystroke of the
/// layout reaches.
///
/// # Panics
///
/// Never.
pub fn compile_layout(
    document: &LayoutDocument,
    id: SchemeId,
) -> Result<SchemeTable, Vec<ImeError>> {
    let mut errors = Vec::new();
    if document.layout.name.as_deref().unwrap_or("").is_empty() {
        errors.push(invalid(String::from(
            "[layout] name is missing: a layout must name itself",
        )));
    }
    // A spelling outside the two schools and a missing declaration are different
    // mistakes, and both leave `style` unset: the checks that need a school are
    // skipped, and the `Err` below carries the diagnostics.
    let style = match document.layout.zero_initial_style.as_deref() {
        Some(spelling) => match ZeroInitialStyle::parse(spelling) {
            Some(style) => Some(style),
            None => {
                errors.push(invalid(String::from(
                    "unknown zero_initial_style spelling: the two schools are \"o-led\" and \"first-letter\"",
                )));
                None
            }
        },
        None => {
            errors.push(invalid(String::from(
                "[layout] zero_initial_style is missing: declare \"o-led\" or \"first-letter\"",
            )));
            None
        }
    };
    validate_tables(document, &mut errors);
    validate_conflicts(document, &mut errors);
    if let Some(style) = style {
        validate_zero_initial(document, style, &mut errors);
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    // The checks above have already refused a document with no name or no school;
    // these defaults only shape values the `Err` arm is about to discard.
    let style = style.unwrap_or(ZeroInitialStyle::FirstLetter);
    let name = document.layout.name.clone().unwrap_or_default();
    let table = build_table(document, id, &name, style);
    validate_typeability(&table, &mut errors);
    if errors.is_empty() {
        Ok(table)
    } else {
        Err(errors)
    }
}

/// The `config/invalid` diagnostic for one layout error.
fn invalid(reason: String) -> ImeError {
    ImeError::ConfigInvalid {
        key: String::from(LAYOUT_DIAGNOSTIC_KEY),
        reason,
    }
}

/// Builds the parse diagnostic: the position, never the offending text.
fn parse_error_reason(text: &str, error: &toml::de::Error) -> String {
    match error.span() {
        Some(span) => {
            let line = text
                .get(..span.start)
                .map_or(1, |head| head.matches('\n').count() + 1);
            if error.message().starts_with("unknown field") {
                format!("not a valid layout document: unknown table or key at line {line}")
            } else {
                format!("not a valid layout document: TOML error at line {line}")
            }
        }
        None => String::from("not a valid layout document"),
    }
}

/// Reports the per-entry shape errors: key names, empty tables and spellings the
/// syllable table does not know.
fn validate_tables(document: &LayoutDocument, errors: &mut Vec<ImeError>) {
    if document.initials.is_empty() {
        errors.push(invalid(String::from(
            "[initials] is missing or empty: a layout that opens no syllable spells nothing",
        )));
    }
    if document.finals.is_empty() {
        errors.push(invalid(String::from(
            "[finals] is missing or empty: a layout that closes no syllable spells nothing",
        )));
    }
    let known_finals = linguistic_finals();
    for (key, spelling) in &document.initials {
        check_key_name(key, "initials", errors);
        if !KNOWN_INITIALS.contains(&spelling.as_str()) {
            errors.push(invalid(format!(
                "[initials] key `{key}` carries a value that is not a syllable initial"
            )));
        }
    }
    for (key, spelling) in &document.finals {
        check_key_name(key, "finals", errors);
        if spelling.is_empty() {
            errors.push(invalid(format!(
                "[finals] key `{key}` carries an empty value: a final entry names at least one final"
            )));
            continue;
        }
        if spelling
            .split_whitespace()
            .any(|final_text| !known_finals.contains(&final_text))
        {
            errors.push(invalid(format!(
                "[finals] key `{key}` carries a value that is not a syllable final"
            )));
        }
    }
    for (key, spelling) in &document.standalone {
        check_key_name(key, "standalone", errors);
        if lookup(spelling).is_none() {
            errors.push(invalid(format!(
                "[standalone] key `{key}` carries a value that is not a syllable of the table"
            )));
        }
    }
}

/// Reports a key that is not one of the 27 keys a layout is indexed by.
fn check_key_name(key: &str, table: &str, errors: &mut Vec<ImeError>) {
    let valid = key.len() == 1 && key_index(key.as_bytes()[0]).is_some();
    if !valid {
        errors.push(invalid(format!(
            "[{table}] key `{key}` is not a scheme key: a key is one of a-z or `;`"
        )));
    }
}

/// Reports one spelling claimed by two keys, per table.
///
/// A final may legitimately sit on two keys -- Microsoft reaches `üe` through
/// both `t` and `v`, two ways of typing one sound -- so the finals table is not
/// checked for cross-key claims. An initial is one sound per key by construction,
/// and two keys claiming the same one is a transcription error the loader names.
fn validate_conflicts(document: &LayoutDocument, errors: &mut Vec<ImeError>) {
    let mut claimed: Vec<(&str, &str)> = Vec::new();
    for (key, spelling) in &document.initials {
        let holder = claimed
            .iter()
            .find(|(claimed_spelling, _)| *claimed_spelling == spelling)
            .map(|(holder, _)| *holder);
        match holder {
            Some(first) => {
                let what = if spelling.is_empty() {
                    String::from("the zero initial")
                } else {
                    format!("the initial `{spelling}`")
                };
                errors.push(invalid(format!(
                    "conflicting mapping: keys `{first}` and `{key}` both claim {what}"
                )));
            }
            None => claimed.push((spelling, key)),
        }
    }
}

/// Reports a declaration that disagrees with the school it declares.
fn validate_zero_initial(
    document: &LayoutDocument,
    style: ZeroInitialStyle,
    errors: &mut Vec<ImeError>,
) {
    match style {
        ZeroInitialStyle::FirstLetter => {
            for vowel in ["a", "e", "o"] {
                if document.initials.get(vowel).map(String::as_str) != Some(vowel) {
                    errors.push(invalid(format!(
                        "a first-letter layout opens a vowel-initial syllable with its own letter: key `{vowel}` must carry the initial `{vowel}`"
                    )));
                }
            }
            for (key, spelling) in &document.initials {
                if spelling.is_empty() {
                    errors.push(invalid(format!(
                        "an empty initial is o-led data: a first-letter layout carries no zero initial, and key `{key}` declares one"
                    )));
                }
            }
        }
        ZeroInitialStyle::OLed => {
            match document.initials.get("o").map(String::as_str) {
                Some("") => {}
                Some(_) => errors.push(invalid(String::from(
                    "an o-led layout opens a vowel-initial syllable with the zero initial: key `o` must carry the empty initial",
                ))),
                None => errors.push(invalid(String::from(
                    "an o-led layout needs the zero initial on key `o`, and no empty initial was declared",
                ))),
            }
            for (key, spelling) in &document.initials {
                if spelling.is_empty() && key != "o" {
                    errors.push(invalid(format!(
                        "only key `o` may carry the zero initial: key `{key}` declares a second one"
                    )));
                }
            }
        }
    }
}

/// Reports every syllable no keystroke of the compiled table reaches.
///
/// One diagnostic per unreachable (initial, final) pair, so a layout missing one
/// final is told once, with a syllable that shows the loss, rather than once per
/// syllable.
fn validate_typeability(table: &SchemeTable, errors: &mut Vec<ImeError>) {
    let reachable = reachable_spellings(table);
    let mut reported: Vec<(&str, &str)> = Vec::new();
    for syllable in SYLLABLES {
        if reachable.contains(syllable) || OPTIONAL_SYLLABLES.contains(syllable) {
            continue;
        }
        let (initial, final_text) = decompose(syllable);
        if reported.contains(&(initial, final_text)) {
            continue;
        }
        reported.push((initial, final_text));
        let reason = if initial.is_empty() {
            format!(
                "no keystroke reaches the final `{final_text}`: `{syllable}` cannot be spelled"
            )
        } else {
            format!(
                "no keystroke reaches the initial `{initial}` with the final `{final_text}`: `{syllable}` cannot be spelled"
            )
        };
        errors.push(invalid(reason));
    }
}

/// Compiles the validated document into the table shape the rewrite walks.
fn build_table(
    document: &LayoutDocument,
    id: SchemeId,
    name: &str,
    style: ZeroInitialStyle,
) -> SchemeTable {
    // The school is declarative: validation has already held the initials to it,
    // and the compiled table is built from the entries alone.
    let _ = style;
    let mut initials: [Option<Box<str>>; SCHEME_KEYS] = std::array::from_fn(|_| None);
    for (key, spelling) in &document.initials {
        if let Some(slot) = slot_of(key) {
            initials[slot] = Some(Box::from(spelling.as_str()));
        }
    }
    let mut finals: [Vec<Box<str>>; SCHEME_KEYS] = std::array::from_fn(|_| Vec::new());
    for (key, spelling) in &document.finals {
        if let Some(slot) = slot_of(key) {
            finals[slot] = spelling.split_whitespace().map(Box::from).collect();
        }
    }
    // The default standalone syllables are the three the published layouts all
    // give their own keys; a document's entries override them.
    let mut standalone: [Box<str>; SCHEME_KEYS] = std::array::from_fn(|_| Box::from(""));
    for vowel in ["a", "e", "o"] {
        if let Some(slot) = slot_of(vowel) {
            standalone[slot] = Box::from(vowel);
        }
    }
    for (key, spelling) in &document.standalone {
        if let Some(slot) = slot_of(key) {
            standalone[slot] = Box::from(spelling.as_str());
        }
    }
    SchemeTable {
        id,
        name: String::from(name),
        initials,
        finals,
        standalone,
    }
}

/// Returns the table slot a document key names, or `None` for a key outside the
/// alphabet. Validation has already reported such a key.
fn slot_of(key: &str) -> Option<usize> {
    if key.len() != 1 {
        return None;
    }
    key_index(key.as_bytes()[0])
}

/// Collects the spellings the table's initials and finals can compose into, plus
/// its standalone syllables.
fn reachable_spellings(table: &SchemeTable) -> Vec<&'static str> {
    let mut reachable = Vec::new();
    let mut scratch = String::new();
    for initial_slot in 0..SCHEME_KEYS {
        let Some(initial) = table
            .initials
            .get(initial_slot)
            .and_then(|slot| slot.as_deref())
        else {
            continue;
        };
        for final_slot in 0..SCHEME_KEYS {
            let Some(finals) = table.finals.get(final_slot) else {
                continue;
            };
            for final_text in finals {
                if let Some(full) = compose_into(initial, final_text, &mut scratch)
                    && let Some(spelling) = syllable_at(full)
                {
                    reachable.push(spelling);
                }
            }
        }
    }
    for slot in 0..SCHEME_KEYS {
        let Some(text) = table.standalone.get(slot).map(|text| text.as_ref()) else {
            continue;
        };
        if let Some(full) = lookup(text)
            && let Some(spelling) = syllable_at(full)
        {
            reachable.push(spelling);
        }
    }
    reachable.sort_unstable();
    reachable.dedup();
    reachable
}

/// Splits a syllable into its (initial, final) reading, the way the syllable
/// table itself decomposes: longest initial prefix first, and the whole spelling
/// as the final when no initial prefix matches.
fn decompose(syllable: &'static str) -> (&'static str, &'static str) {
    for initial in ["zh", "ch", "sh"] {
        if let Some(rest) = syllable.strip_prefix(initial) {
            return (initial, rest);
        }
    }
    // The table's entries lead with one ASCII letter, except the multi-byte
    // vowel-led interjections, which have no initial prefix. `get(..1)` answers
    // `None` off a char boundary, so the slicing below stays inside the string.
    if syllable.len() >= 2
        && let Some(head) = syllable.get(..1)
        && SINGLE_INITIALS.contains(&head)
    {
        return (head, &syllable[1..]);
    }
    ("", syllable)
}

/// Collects the distinct finals the syllable table decomposes into, which is the
/// closed set a layout's `[finals]` values may name.
fn linguistic_finals() -> Vec<&'static str> {
    let mut finals = Vec::new();
    for syllable in SYLLABLES {
        let (_, final_text) = decompose(syllable);
        if !finals.contains(&final_text) {
            finals.push(final_text);
        }
    }
    finals
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_initial_style_parse_accepts_both_schools_and_nothing_else() {
        assert_eq!(ZeroInitialStyle::parse("o-led"), Some(ZeroInitialStyle::OLed));
        assert_eq!(
            ZeroInitialStyle::parse("first-letter"),
            Some(ZeroInitialStyle::FirstLetter)
        );
        assert_eq!(ZeroInitialStyle::parse("oled"), None);
        assert_eq!(ZeroInitialStyle::parse(""), None);
    }

    #[test]
    fn test_zero_initial_style_as_str_round_trips_through_parse() {
        for style in [
            ZeroInitialStyle::OLed,
            ZeroInitialStyle::FirstLetter,
        ] {
            assert_eq!(ZeroInitialStyle::parse(style.as_str()), Some(style));
        }
    }

    #[test]
    fn test_decompose_splits_the_retroflexes_before_the_single_initials() {
        assert_eq!(decompose("zhong"), ("zh", "ong"));
        assert_eq!(decompose("shi"), ("sh", "i"));
        assert_eq!(decompose("shai"), ("sh", "ai"));
        assert_eq!(decompose("san"), ("s", "an"));
        assert_eq!(decompose("zh"), ("zh", ""));
        assert_eq!(decompose("a"), ("", "a"));
        assert_eq!(decompose("hng"), ("h", "ng"));
        assert_eq!(decompose("ê"), ("", "ê"));
    }

    #[test]
    fn test_linguistic_finals_hold_the_set_the_published_layouts_name() {
        let finals = linguistic_finals();
        for final_text in ["a", "ai", "an", "ang", "ao", "e", "en", "eng", "er", "i", "u", "ü",
            "üe", "ong", "iong", "iang", "uang", "uan", "uai", "ia", "ua", "ui", "un", "uo", "ou",
            "in", "ing", "ian", "iao", "ie", "iu", "o", "ei", "ao", "ng", "m", "n", "ê"]
        {
            assert!(
                finals.contains(&final_text),
                "{final_text} must be a syllable-table final"
            );
        }
    }

    #[test]
    fn test_slot_of_folds_nothing_and_rejects_everything_but_the_alphabet() {
        assert_eq!(slot_of("a"), Some(0));
        assert_eq!(slot_of(";"), Some(super::super::SEMICOLON_INDEX));
        assert_eq!(slot_of("A"), None, "key names are lower case");
        assert_eq!(slot_of("vv"), None);
        assert_eq!(slot_of("1"), None);
        assert_eq!(slot_of(""), None);
    }
}
