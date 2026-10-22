//! The scenario v2 sections: the decode flags, the user store and the decay clock a v2
//! scenario may name.
//!
//! Responsibility: hold the three optional sections a v2 scenario file adds over the v1
//! shape, resolve the names it states into the values the replay consumes, and refuse
//! everything it cannot resolve. Parsing is total: an unknown flag, an unknown fixture
//! name and a decay clock without a store are red parse errors, never silently ignored
//! lines, because a scenario that ran on defaults while believing it had configured
//! something would pass for the wrong reason.
//!
//! # Defaults keep v1
//!
//! Every section is optional and every default is the v1 behaviour: no extra flags, the
//! model absent, the user frequencies taken from the scenario's own dictionary spec. The
//! built-in set and every fixture file written before the sections existed parse and run
//! unchanged, which is what the compatibility gate holds the parser to.

use std::collections::BTreeMap;

use anyhow::{Context, bail};
use ime_types::DecodeFlags;
use serde::Deserialize;

/// Reads the decode flag a scenario file names.
///
/// The names are `DecodeFlags`'s own, so a scenario file and a diagnostic agree on the
/// vocabulary and a misspelled switch is a parse error rather than a bit that silently
/// stayed clear.
///
/// # Errors
///
/// A message naming the offending name when no flag carries it.
///
/// # Panics
///
/// Never.
pub fn decode_flag(name: &str) -> anyhow::Result<DecodeFlags> {
    DecodeFlags::from_name(name).ok_or_else(|| anyhow::anyhow!("unknown decode flag {name:?}"))
}

/// The `[decode]` section: the switches every request carries, by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DecodeSection {
    /// The flags the section names, resolved at parse time.
    pub flags: Vec<DecodeFlags>,
    /// The bigram model the scenario decodes with.
    pub bigram: BigramMode,
}

impl DecodeSection {
    /// Returns `true` when the section names nothing, so a v1 file's rendered TOML
    /// carries no section at all.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_default(&self) -> bool {
        self.flags.is_empty() && self.bigram == BigramMode::Absent
    }
}

/// The bigram model a scenario decodes with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BigramMode {
    /// The model holds no container: a conditional score is a miss. The v1 behaviour.
    #[default]
    Absent,
    /// The model is the compiler's own container fixture, whose `BIGRAM` section is
    /// populated.
    Present,
}

impl BigramMode {
    /// Reads the mode a scenario file names.
    ///
    /// # Errors
    ///
    /// A message naming the offending value when it is neither spelling.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        match text {
            "absent" => Ok(Self::Absent),
            "present" => Ok(Self::Present),
            other => bail!(r#"bigram must be "absent" or "present", not {other:?}"#),
        }
    }
}

/// The `[user_db]` section: the user store the scenario runs against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserDbSection {
    /// The store the scenario runs against, resolved at parse time.
    pub fixture: Option<UserDbFixture>,
}

impl UserDbSection {
    /// Returns `true` when the section names no store.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_default(&self) -> bool {
        self.fixture.is_none()
    }
}

/// The user store a v2 scenario runs against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserDbFixture {
    /// A store nothing has been recorded in.
    Cold,
    /// A store seeded with the commits `tests/fixtures/userdb/<name>.toml` describes.
    Seed(String),
}

impl UserDbFixture {
    /// Reads the fixture a scenario file names.
    ///
    /// # Errors
    ///
    /// A message naming the offending value when it is neither `"cold"` nor a
    /// `"seed:<name>"` with a non-empty name.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        match text {
            "cold" => Ok(Self::Cold),
            other => match other.strip_prefix("seed:") {
                Some(name) if !name.trim().is_empty() => Ok(Self::Seed(name.trim().to_owned())),
                _ => bail!(r#"fixture must be "cold" or "seed:<name>", not {other:?}"#),
            },
        }
    }
}

/// The `[decay]` section: the half-life the user store folds with.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecaySection {
    /// Commits over which a contribution loses half its weight.
    pub half_life_ticks: u32,
}

/// The v2 sections projected to the values the replay consumes.
///
/// This is the shape the assembly point asks for: the model mode, the store and the decay
/// clock. Building one validates the cross-section rules a single section cannot see -- a
/// decay clock without a store folds nothing, and a container model beside a spec-written
/// model would make the score ambiguous. The card contract's `flags` field is
/// deliberately absent: the resolved flags are read straight off the scenario's own
/// `[decode]` section by the replay, which is the one consumer, and a second resolved
/// copy beside it would be a second truth to keep in step.
#[derive(Clone, Debug)]
pub struct ScenarioV2 {
    /// The bigram model the scenario decodes with.
    pub bigram: BigramMode,
    /// The user store the scenario runs against.
    pub user_db: Option<UserDbFixture>,
    /// The decay clock the store folds with.
    pub decay: Option<DecayClock>,
}

/// The decay clock a v2 scenario fixes: a known half-life at a known constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecayClock {
    /// Commits over which a contribution loses half its weight.
    pub half_life_ticks: u32,
}

impl ScenarioV2 {
    /// Projects the v2 sections of `scenario`, refusing what cannot be assembled.
    ///
    /// # Errors
    ///
    /// A message naming the section and the rule when a decay clock names no store, when
    /// a container model would sit beside a spec-written one, or when a user store would
    /// sit beside a spec-written frequency table.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn of(scenario: &super::Scenario) -> anyhow::Result<Self> {
        let name = &scenario.name;
        if scenario.decay.is_some() && scenario.user_db.fixture.is_none() {
            bail!("{name}: [decay] names a clock but no [user_db] store folds with it");
        }
        if scenario.decode.bigram == BigramMode::Present
            && (!scenario.dictionary.bigrams.is_empty() || !scenario.dictionary.unigrams.is_empty())
        {
            bail!(
                "{name}: bigram = \"present\" takes the whole model from the container, \
                 so [dictionary] may carry neither unigrams nor bigrams"
            );
        }
        if scenario.user_db.fixture.is_some()
            && (!scenario.dictionary.user_freq.is_empty()
                || !scenario.dictionary.user_words.is_empty())
        {
            bail!(
                "{name}: [user_db] replaces the scenario's own frequency table, so \
                 [dictionary] may carry neither user_freq nor user_words"
            );
        }
        Ok(Self {
            bigram: scenario.decode.bigram,
            user_db: scenario.user_db.fixture.clone(),
            decay: scenario.decay.as_ref().map(|decay| DecayClock {
                half_life_ticks: decay.half_life_ticks,
            }),
        })
    }
}

/// One seeded word of a user-store fixture file.
#[derive(Deserialize)]
pub struct SeedRow {
    /// The word the commits are recorded under.
    pub text: String,
    /// How many times the word was committed.
    pub commits: u32,
}

/// A user-store seed file, as `tests/fixtures/userdb/<name>.toml` holds it.
#[derive(Deserialize)]
pub struct SeedFile {
    /// Format version, read to refuse a file a later build wrote.
    pub version: u32,
    /// One row per seeded word.
    #[serde(default)]
    pub words: Vec<SeedRow>,
}

/// Reads the seed file `fixture` names.
///
/// # Errors
///
/// An error naming the file when it cannot be read, is not TOML, does not carry version 1,
/// or names a word with no commits.
///
/// # Panics
///
/// Never.
pub fn read_seed(fixture: &UserDbFixture) -> anyhow::Result<BTreeMap<String, u32>> {
    let name = match fixture {
        UserDbFixture::Cold => return Ok(BTreeMap::new()),
        UserDbFixture::Seed(name) => name,
    };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/userdb")
        .join(format!("{name}.toml"));
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading the user-store seed {}", path.display()))?;
    let file: SeedFile = toml::from_str(&text)
        .with_context(|| format!("parsing the user-store seed {}", path.display()))?;
    if file.version != 1 {
        bail!(
            "{}: seed format {}, this build reads 1",
            path.display(),
            file.version
        );
    }
    let mut seeded = BTreeMap::new();
    for row in file.words {
        if row.commits == 0 {
            bail!(
                "{}: {} names no commits; a seeded word that was never recorded is a row \
                 the fixture cannot serve",
                path.display(),
                row.text
            );
        }
        seeded.insert(row.text, row.commits);
    }
    Ok(seeded)
}

impl<'de> serde::Deserialize<'de> for DecodeSection {
    fn deserialize<D: serde::Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            #[serde(default)]
            flags: Vec<String>,
            #[serde(default)]
            bigram: Option<String>,
        }
        let raw = Raw::deserialize(input)?;
        let mut flags = Vec::new();
        for name in &raw.flags {
            flags.push(decode_flag(name).map_err(serde::de::Error::custom)?);
        }
        let bigram = match &raw.bigram {
            Some(text) => BigramMode::parse(text).map_err(serde::de::Error::custom)?,
            None => BigramMode::Absent,
        };
        Ok(Self { flags, bigram })
    }
}

impl serde::Serialize for DecodeSection {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(serde::Serialize)]
        struct Raw<'a> {
            #[serde(skip_serializing_if = "Vec::is_empty")]
            flags: Vec<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            bigram: Option<&'a str>,
        }
        let mut names = Vec::new();
        for flag in &self.flags {
            for (name, _) in flag.iter_names() {
                names.push(name);
            }
        }
        let bigram = match self.bigram {
            BigramMode::Absent => None,
            BigramMode::Present => Some("present"),
        };
        Raw {
            flags: names,
            bigram,
        }
        .serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for UserDbSection {
    fn deserialize<D: serde::Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            fixture: Option<String>,
        }
        let raw = Raw::deserialize(input)?;
        let fixture = match &raw.fixture {
            Some(text) => Some(UserDbFixture::parse(text).map_err(serde::de::Error::custom)?),
            None => None,
        };
        Ok(Self { fixture })
    }
}

impl serde::Serialize for UserDbSection {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(serde::Serialize)]
        struct Raw {
            fixture: Option<String>,
        }
        let fixture = match &self.fixture {
            Some(UserDbFixture::Cold) => Some(String::from("cold")),
            Some(UserDbFixture::Seed(name)) => Some(format!("seed:{name}")),
            None => None,
        };
        Raw { fixture }.serialize(serializer)
    }
}
