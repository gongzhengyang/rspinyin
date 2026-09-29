//! The declarative side of the harness: scenarios, the dictionary they describe, and the
//! key-action vocabulary a scenario file is written in.
//!
//! A scenario is data, not code. The steps and the expected outcome of each one live in a
//! `*.toml` file so that a case in `tests.md` can name the file it runs, and so that a
//! reviewer who does not read Rust can still see what a case asserts.
//!
//! # What a scenario may name
//!
//! A step drives one frozen `KeyAction`. Only the actions whose whole effect the engine
//! owns are driven here -- typing, Backspace, the caret, Escape and a bare decode -- and a
//! scenario that names one of the others is refused when it is parsed: committing,
//! selecting, paging and the mode toggles belong to the session state machine, which
//! `ime-core` does not export from its crate root yet.
//!
//! # Layout
//!
//! - [`report`] owns what a step expects, what it saw, and the divergence it reports.
//! - [`fixture`] owns the doubles a scenario's dictionary builds into.

mod fixture;
mod report;

pub use crate::testd::engine::scenario::fixture::EngineFixture;
pub use crate::testd::engine::scenario::report::{
    Divergence, Expectation, Observation, StepPrint, decode_code, ime_code,
};

use std::collections::BTreeMap;

use anyhow::{Context, bail, ensure};
use ime_types::KeyAction;
use serde::{Deserialize, Serialize};

/// One scripted scenario: the keystrokes and what each must produce.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Name the harness reports the scenario under.
    pub name: String,
    /// The data the scenario runs against, when it carries its own.
    ///
    /// A scenario file holds its dictionary so that the file is self-contained;
    /// `run_scenario` takes the doubles as arguments instead, so a caller that supplies
    /// its own is free to leave this empty.
    #[serde(default, skip_serializing_if = "DictionarySpec::is_empty")]
    pub dictionary: DictionarySpec,
    /// The keystrokes, in order.
    ///
    /// Defaulted so that a file which forgot them parses and is refused with a message
    /// about the scenario rather than one about a missing field.
    #[serde(default)]
    pub steps: Vec<Step>,
}

impl Scenario {
    /// Builds a scenario from its name, its data and its steps.
    ///
    /// # Returns
    ///
    /// The scenario.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn new(name: impl Into<String>, dictionary: DictionarySpec, steps: Vec<Step>) -> Self {
        Self {
            name: name.into(),
            dictionary,
            steps,
        }
    }
}

/// One step of a scenario: a key action and what it must produce.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// The key action the step drives.
    #[serde(with = "key_action")]
    pub action: KeyAction,
    /// What the step must observe.
    pub expect: Expectation,
}

/// The in-memory data a scenario runs against, described as data.
///
/// Every field is a table or a table array and none is a bare value, so a rendered
/// scenario never places a value after a table header -- the one shape a TOML document
/// cannot hold.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DictionarySpec {
    /// One row per key the dictionary holds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<WordRow>,
    /// One row per syllable the dictionary falls back to single characters for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub singles: Vec<SingleRow>,
    /// One row per word the user is credited with coining.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub user_words: Vec<UserWordRow>,
    /// One row per conditional score the model holds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bigrams: Vec<BigramRow>,
    /// Keys whose lookup is refused, and the frozen code the refusal renders as.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub failures: BTreeMap<String, String>,
    /// Word text to the number of times the user has committed it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub user_freq: BTreeMap<String, u32>,
    /// Word text to its quantized log probability, in Q8.8.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unigrams: BTreeMap<String, i32>,
}

impl DictionarySpec {
    /// Returns the distinct frozen codes the spec's refused keys answer with.
    ///
    /// A scenario's dictionary may refuse lookups with one code and no more: the decoder
    /// folds a refusal into `DecodeResult::degraded` without passing the error on, so the
    /// harness names the code from the spec rather than from the dictionary, and a spec
    /// that declared two would make that name ambiguous. [`EngineFixture::new`] is what
    /// enforces the limit.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn failure_codes(&self) -> Vec<String> {
        let mut codes: Vec<String> = self.failures.values().cloned().collect();
        codes.sort();
        codes.dedup();
        codes
    }

    /// Returns `true` when the spec describes nothing.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
            && self.singles.is_empty()
            && self.user_words.is_empty()
            && self.bigrams.is_empty()
            && self.failures.is_empty()
            && self.user_freq.is_empty()
            && self.unigrams.is_empty()
    }
}

/// The words one key of a mock dictionary holds, strongest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WordRow {
    /// The `'`-separated syllable key.
    pub key: String,
    /// The words stored under it, strongest first.
    pub texts: Vec<String>,
}

/// The single-character fallbacks of one syllable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SingleRow {
    /// The syllable as the table spells it.
    pub syllable: String,
    /// The single characters, strongest first.
    pub texts: Vec<String>,
}

/// One word the user is credited with coining.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserWordRow {
    /// The word's text.
    pub text: String,
}

/// One conditional score of the mock model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BigramRow {
    /// The word the score conditions on.
    pub prev: String,
    /// The word being scored.
    pub word: String,
    /// The conditional log probability, in Q8.8.
    pub score: i32,
}

/// Returns `true` when the harness can apply `action` to the engine on its own.
///
/// The actions it cannot drive are the ones whose effect is the session state machine's:
/// committing, selecting a candidate, paging and the mode toggles. Naming one in a
/// scenario is reported as a divergence rather than ignored, because a step that asserted
/// nothing would look like a passing one.
///
/// The machine itself is reachable -- `ime_core::state` exports it -- so what keeps these
/// actions out is the expectation model, not a missing interface: no variant of
/// [`Expectation`] describes a commit, a selection or a page flip, so a step that drove
/// one of them would have nothing to assert against.
///
/// # Panics
///
/// Never panics.
pub fn drives(action: KeyAction) -> bool {
    matches!(
        action,
        KeyAction::InputChar(_)
            | KeyAction::Backspace
            | KeyAction::MoveCaret(_)
            | KeyAction::Escape
            | KeyAction::Ignore
    )
}

/// Parses a scenario out of the TOML text a fixture file holds.
///
/// # Returns
///
/// The scenario.
///
/// # Errors
///
/// Returns an error when the text is not TOML, when it carries a field the scenario model
/// does not define, when a key action it names is not one the model knows, when it holds
/// no steps, or when a step names an action the harness cannot drive.
///
/// # Panics
///
/// Never panics.
pub fn parse_scenario(text: &str) -> anyhow::Result<Scenario> {
    let scenario: Scenario = toml::from_str(text).context("parsing a scenario")?;
    ensure!(
        !scenario.steps.is_empty(),
        "{}: a scenario without steps asserts nothing",
        scenario.name
    );
    for (index, step) in scenario.steps.iter().enumerate() {
        if !drives(step.action) {
            bail!(
                "{}: step {index} names {}, which the direct-drive harness cannot apply",
                scenario.name,
                action_name(step.action)
            );
        }
    }
    Ok(scenario)
}

/// Renders a key action as the string a scenario file holds.
///
/// # Panics
///
/// Never panics.
pub fn action_name(action: KeyAction) -> String {
    match action {
        KeyAction::InputChar(ch) => format!("input-char:{ch}"),
        KeyAction::SelectIndex(digit) => format!("select-index:{digit}"),
        KeyAction::MoveHighlight(step) => format!("move-highlight:{step}"),
        KeyAction::MoveCaret(step) => format!("move-caret:{step}"),
        KeyAction::Backspace => String::from("backspace"),
        KeyAction::CommitHighlighted => String::from("commit-highlighted"),
        KeyAction::CommitRaw => String::from("commit-raw"),
        KeyAction::PageNext => String::from("page-next"),
        KeyAction::PagePrev => String::from("page-prev"),
        KeyAction::ToggleLang => String::from("toggle-lang"),
        KeyAction::ToggleFullWidth => String::from("toggle-full-width"),
        KeyAction::TogglePunct => String::from("toggle-punct"),
        KeyAction::EnterTempEnglish => String::from("enter-temp-english"),
        KeyAction::Escape => String::from("escape"),
        KeyAction::Ignore => String::from("ignore"),
        // Appended by ADR-0005. The names match `KeyAction`'s own `label()` in
        // `crates/ime-types/src/key.rs`, so a scenario file and a diagnostic agree.
        KeyAction::ToggleScript => String::from("toggle-script"),
        KeyAction::ForgetHighlighted => String::from("forget-highlighted"),
        KeyAction::PinHighlighted => String::from("pin-highlighted"),
        KeyAction::AddPhrase => String::from("add-phrase"),
    }
}

/// Reads the key action a scenario file names.
///
/// # Errors
///
/// A message naming the offending string when it is not a known action, when a
/// payload-carrying action carries no payload, or when a payload does not fit.
///
/// # Panics
///
/// Never panics.
fn parse_action(text: &str) -> Result<KeyAction, String> {
    let (name, payload) = match text.split_once(':') {
        Some((name, payload)) => (name, Some(payload)),
        None => (text, None),
    };
    match (name, payload) {
        ("input-char", Some(payload)) => Ok(KeyAction::InputChar(payload_char(payload, text)?)),
        ("select-index", Some(payload)) => {
            Ok(KeyAction::SelectIndex(payload_number(payload, text)?))
        }
        ("move-highlight", Some(payload)) => {
            Ok(KeyAction::MoveHighlight(payload_number(payload, text)?))
        }
        ("move-caret", Some(payload)) => Ok(KeyAction::MoveCaret(payload_number(payload, text)?)),
        ("backspace", None) => Ok(KeyAction::Backspace),
        ("commit-highlighted", None) => Ok(KeyAction::CommitHighlighted),
        ("commit-raw", None) => Ok(KeyAction::CommitRaw),
        ("page-next", None) => Ok(KeyAction::PageNext),
        ("page-prev", None) => Ok(KeyAction::PagePrev),
        ("toggle-lang", None) => Ok(KeyAction::ToggleLang),
        ("toggle-full-width", None) => Ok(KeyAction::ToggleFullWidth),
        ("toggle-punct", None) => Ok(KeyAction::TogglePunct),
        ("enter-temp-english", None) => Ok(KeyAction::EnterTempEnglish),
        ("escape", None) => Ok(KeyAction::Escape),
        ("ignore", None) => Ok(KeyAction::Ignore),
        ("toggle-script", None) => Ok(KeyAction::ToggleScript),
        ("forget-highlighted", None) => Ok(KeyAction::ForgetHighlighted),
        ("pin-highlighted", None) => Ok(KeyAction::PinHighlighted),
        ("add-phrase", None) => Ok(KeyAction::AddPhrase),
        _ => Err(format!("unknown key action {text:?}")),
    }
}

/// Reads the single character an `input-char` action carries.
///
/// # Errors
///
/// A message naming the action when its payload is not exactly one character.
///
/// # Panics
///
/// Never panics.
fn payload_char(payload: &str, text: &str) -> Result<char, String> {
    let mut chars = payload.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Ok(ch),
        _ => Err(format!("{text:?} must carry exactly one character")),
    }
}

/// Reads the number a payload-carrying action carries.
///
/// # Errors
///
/// A message naming the action when its payload is not a number of the right type.
///
/// # Panics
///
/// Never panics.
fn payload_number<T: std::str::FromStr>(payload: &str, text: &str) -> Result<T, String> {
    payload
        .parse::<T>()
        .map_err(|_| format!("{text:?} does not carry a number"))
}

/// Serializes a `KeyAction` as one string, and reads it back.
///
/// The frozen enum is not serde-derived -- it is a contract type, and its payload variants
/// would serialize as tables -- so a scenario file carries the action as `<name>` or
/// `<name>:<payload>`.
mod key_action {
    use ime_types::KeyAction;
    use serde::{Deserialize, Deserializer, Serializer};

    use super::{action_name, parse_action};

    /// Writes the action as the string a scenario file holds.
    ///
    /// # Errors
    ///
    /// Whatever the serializer reports.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(super) fn serialize<S: Serializer>(action: &KeyAction, out: S) -> Result<S::Ok, S::Error> {
        out.serialize_str(&action_name(*action))
    }

    /// Reads the action a scenario file names.
    ///
    /// # Errors
    ///
    /// Whatever the deserializer reports, including an unknown action name.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(input: D) -> Result<KeyAction, D::Error> {
        let text = String::deserialize(input)?;
        parse_action(&text).map_err(serde::de::Error::custom)
    }
}
