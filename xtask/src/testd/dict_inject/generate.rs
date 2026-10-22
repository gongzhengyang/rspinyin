//! The named dictionary fixtures: real-shaped containers built in this process.
//!
//! Responsibility: build, on demand and deterministically, the four containers the word-source,
//! normalization and hot-swap case cards reference -- the classic synthetic build, the
//! two-layer weight build, a container with a populated `BIGRAM` section, and a staged variant
//! of the synthetic build whose ranking differs -- and verify each one with the real reader
//! before handing it out. The compile reuses the compiler's own section builder
//! ([`crate::dictc::build::compile`]) in this process, so a fixture is shaped by the same
//! normalization and the same within-key ordering the production dictionary is, without
//! reading `data/compiled/`, any fetched source, or the network.
//!
//! # Closed world
//!
//! Every fixture compiles a fixed word table declared in this file, and the bigram fixture's
//! corpus is the repository's own [`CORPUS_SNIPPET`], a few dozen sentences written for this
//! purpose. Nothing is fetched and nothing is read from the data directory: a case that needs
//! a container runs on a fresh checkout, which is the same rule the mutation channel's
//! in-memory container already follows. The expectation tables the fixtures are held to live
//! beside the corpus, in `tests/fixtures/dict/expectations.toml`, and a change to a word
//! table is a change to that file.
//!
//! # Determinism
//!
//! The tables are fixed, the compiler sorts deterministically, and the corpus is embedded, so
//! two generations of one fixture produce the same bytes -- which is the property the case
//! cards pin with `sha256` and the double-compile gate asserts.
//!
//! # What a generated fixture is verified against
//!
//! `generate` refuses to hand out a container the real reader rejects: the image is parsed
//! with [`Verify::Full`] -- header, section table and every checksum -- before the file is
//! written. A fixture that cannot be read is a broken fixture, and it fails the case's own
//! setup rather than its assertions.

use std::fs;
use std::path::Path;

use ime_dict::format::reader::{Reader, Verify};
use ime_dict::format::writer::DictWriter;
use ime_dict::format::{BigramRecord, SectionKind};
use ime_dict::fst_index::FstLexicon;

use super::error::FixtureError;
use super::sha256;
use crate::dictc::build::{self, Band};
use crate::dictc::normalize::NormalizeRules;
use crate::dictc::source::Word;

/// The corpus the `BigramPresent` fixture's pairs come from: this repository's own sentences,
/// written for the fixture and never fetched.
pub const CORPUS_SNIPPET: &str = include_str!("../../../../tests/fixtures/dict/corpus-snippet.txt");

/// The table the `Synthetic` and `StagedVariant` fixtures compile: eighteen words over twelve
/// keys, with three keys carrying more than one candidate so a within-key order exists at all.
const SYNTHETIC: &[(&str, &str, u32)] = &[
    ("你", "ni", 900),
    ("尼", "ni", 700),
    ("妮", "ni", 500),
    ("好", "hao", 880),
    ("号", "hao", 760),
    ("豪", "hao", 640),
    ("我", "wo", 860),
    ("他", "ta", 840),
    ("我们", "wo'men", 500),
    ("你们", "ni'men", 450),
    ("他们", "ta'men", 440),
    ("你好", "ni'hao", 600),
    ("你号", "ni'hao", 200),
    ("时间", "shi'jian", 400),
    ("事件", "shi'jian", 350),
    ("计算机", "ji'suan'ji", 700),
    ("图书馆", "tu'shu'guan", 500),
    ("办公室", "ban'gong'shi", 300),
];

/// The staged variant's weights: the same words and keys as [`SYNTHETIC`], with every
/// multi-candidate key's order inverted. A hot swap from one container to the other is
/// observable in the candidates each key serves.
const STAGED: &[(&str, &str, u32)] = &[
    ("你", "ni", 500),
    ("尼", "ni", 700),
    ("妮", "ni", 900),
    ("好", "hao", 640),
    ("号", "hao", 760),
    ("豪", "hao", 880),
    ("我", "wo", 860),
    ("他", "ta", 840),
    ("我们", "wo'men", 500),
    ("你们", "ni'men", 450),
    ("他们", "ta'men", 440),
    ("你好", "ni'hao", 200),
    ("你号", "ni'hao", 600),
    ("时间", "shi'jian", 350),
    ("事件", "shi'jian", 400),
    ("计算机", "ji'suan'ji", 700),
    ("图书馆", "tu'shu'guan", 500),
    ("办公室", "ban'gong'shi", 300),
];

/// The `Layered` fixture's table: words engineered to land in every normalization band at
/// once -- single characters, core two-character words above the floor, and the tail that is
/// two two-character words *below* the floor plus two three-character words. The tail is the
/// interesting half: source frequencies of 10, 5, 700 and 500 all come out flat.
const LAYERED: &[(&str, &str, u32)] = &[
    ("你", "ni", 900),
    ("好", "hao", 880),
    ("我们", "wo'men", 500),
    ("时间", "shi'jian", 400),
    ("你们", "ni'men", 300),
    ("事件", "shi'jian", 10),
    ("你号", "ni'hao", 5),
    ("计算机", "ji'suan'ji", 700),
    ("图书馆", "tu'shu'guan", 500),
];

/// The `BigramPresent` fixture's table: six words the corpus snippet's sentences chain into
/// adjacent pairs.
const BIGRAM: &[(&str, &str, u32)] = &[
    ("中国", "zhong'guo", 600),
    ("银行", "yin'hang", 400),
    ("经济", "jing'ji", 380),
    ("市场", "shi'chang", 360),
    ("发展", "fa'zhan", 340),
    ("公司", "gong'si", 320),
];

/// Named dictionary fixtures the case cards reference.
///
/// [`DictFixture::Synthetic`] stays the closed-world default; the other three exist so the
/// word-source, normalization, bigram and hot-swap cases exercise real-shaped data -- built by
/// the real compiler, verified by the real reader -- without reading anything from
/// `data/compiled/` or the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DictFixture {
    /// The classic synthetic build.
    Synthetic,
    /// Two-layer weights in one container: a core true-frequency band and a flat tail band,
    /// with two-character words *below* the floor among the tail. For the normalization cases.
    Layered,
    /// A container with a populated `BIGRAM` section, built by the compiler from
    /// [`CORPUS_SNIPPET`]. For the bigram lookup cases.
    BigramPresent,
    /// A second container staged for the hot-swap cases: valid, same keys as
    /// [`DictFixture::Synthetic`], different ranking, so a swap is observable in candidates.
    StagedVariant,
}

impl DictFixture {
    /// Every fixture, in the order the expectation table states them.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn all() -> [Self; 4] {
        [
            Self::Synthetic,
            Self::Layered,
            Self::BigramPresent,
            Self::StagedVariant,
        ]
    }

    /// The name the ledger and the expectation table record the fixture under.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn name(self) -> &'static str {
        match self {
            Self::Synthetic => "synthetic",
            Self::Layered => "layered",
            Self::BigramPresent => "bigram-present",
            Self::StagedVariant => "staged-variant",
        }
    }

    /// The file name the generated container lands as.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Synthetic => "synthetic.dict",
            Self::Layered => "layered.dict",
            Self::BigramPresent => "bigram-present.dict",
            Self::StagedVariant => "staged-variant.dict",
        }
    }

    /// Builds the fixture's container and writes it to `dest`.
    ///
    /// The write is the fixture's whole filesystem effect: the container is compiled and
    /// verified in memory, and `dest` is created with its parent when missing. The same
    /// fixture at the same build generates the same bytes, which is what the case cards'
    /// `sha256` pins hold to.
    ///
    /// # Errors
    /// Returns [`FixtureError::Generation`] when the compile pipeline refuses the word table
    /// or the image cannot be encoded, [`FixtureError::ImageUnreadable`] when the built image
    /// is not a container this build reads, and [`FixtureError::Io`] when `dest` cannot be
    /// written.
    pub fn generate(self, dest: &Path) -> Result<Generated, FixtureError> {
        let mut compiled = build::compile(
            &self.words(),
            Band {
                // Above every table's heaviest weight: no L3b expansion, one key per word.
                threshold: 100_000,
                requested: 0,
            },
            4,
            &[],
            NormalizeRules::default(),
        )
        .map_err(|error| FixtureError::Generation {
            fixture: self.name(),
            reason: error.to_string(),
        })?;
        if matches!(self, Self::BigramPresent) {
            compiled.bigram = self.bigram_section();
        }
        let mut writer = DictWriter::new();
        for (kind, payload) in [
            (SectionKind::Fst, compiled.fst),
            (SectionKind::Entries, compiled.entries),
            (SectionKind::StrPool, compiled.strpool),
            (SectionKind::Unigram, compiled.unigram),
            (SectionKind::WordList, compiled.wordlist),
            (SectionKind::Bigram, compiled.bigram),
        ] {
            writer
                .add_section(kind, payload)
                .map_err(|error| FixtureError::Generation {
                    fixture: self.name(),
                    reason: error.to_string(),
                })?;
        }
        let image = writer.encode().map_err(|error| FixtureError::Generation {
            fixture: self.name(),
            reason: error.to_string(),
        })?;
        // The load the case will run, run here first: a fixture the reader refuses is a
        // broken fixture, and the setup reports it instead of the assertions.
        Reader::parse(image.as_slice(), Verify::Full)
            .map_err(|cause| FixtureError::ImageUnreadable { cause })?;
        let parent = dest
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf);
        if let Some(parent) = &parent {
            fs::create_dir_all(parent).map_err(|source| FixtureError::Io {
                path: parent.clone(),
                source,
            })?;
        }
        fs::write(dest, &image).map_err(|source| FixtureError::Io {
            path: dest.to_path_buf(),
            source,
        })?;
        Ok(Generated {
            path: dest.to_path_buf(),
            digest: sha256(dest)?,
            words: u32::try_from(compiled.stats.words).unwrap_or(u32::MAX),
            keys: u32::try_from(compiled.stats.keys).unwrap_or(u32::MAX),
            bigram_pairs: u32::try_from(
                Reader::parse(image.as_slice(), Verify::Full)
                    .map(|reader| reader.bigram_count())
                    .unwrap_or_default(),
            )
            .unwrap_or(u32::MAX),
        })
    }

    /// Builds the fixture into `dir` under its own file name, and opens the result the way
    /// the plugin does.
    ///
    /// This is the form a case consumes: the container on disk, verified by load, with the
    /// lexicon ready for the lookups the case asserts against.
    ///
    /// # Errors
    /// As [`DictFixture::generate`], plus [`FixtureError::Unreadable`] when the written file
    /// cannot be loaded as a lexicon.
    pub fn generate_in(self, dir: &Path) -> Result<Loaded, FixtureError> {
        let path = dir.join(self.file_name());
        let generated = self.generate(&path)?;
        let lexicon = FstLexicon::load(&path).map_err(|cause| FixtureError::Unreadable {
            path: path.clone(),
            cause,
        })?;
        Ok(Loaded { generated, lexicon })
    }

    /// The fixture's word table as the compiler consumes it.
    ///
    /// # Panics
    ///
    /// Never.
    fn words(self) -> Vec<Word> {
        let table: &[(&str, &str, u32)] = match self {
            Self::Synthetic => SYNTHETIC,
            Self::Layered => LAYERED,
            Self::BigramPresent => BIGRAM,
            Self::StagedVariant => STAGED,
        };
        table
            .iter()
            .map(|(text, key, weight)| Word {
                text: (*text).to_owned(),
                key: (*key).to_owned(),
                // No L3b expansion: the band above every weight keeps one key per word, so
                // the tables stay the expectation tables' subject.
                readings: Vec::new(),
                weight: *weight,
                flags: 0,
                polyphone: false,
            })
            .collect()
    }

    /// The `BIGRAM` section the `BigramPresent` fixture compiles from the corpus snippet.
    ///
    /// # Panics
    ///
    /// Never.
    fn bigram_section(self) -> Vec<u8> {
        let index = crate::dictc::bigram::WordIndex::new(BIGRAM.iter().map(|(text, _, _)| *text));
        let counts = crate::dictc::bigram::count_cooccurrence(CORPUS_SNIPPET, &index);
        let records = crate::dictc::bigram::build_records(
            counts,
            crate::dictc::bigram::BigramConfig {
                max_pairs: 200_000,
                // The cut the production build applies is a size discipline; a closed-world
                // fixture keeps every pair its corpus produces, which is what the
                // expectation table counts.
                min_score: 0.0,
            },
        );
        records.into_iter().flat_map(BigramRecord::encode).collect()
    }
}

/// What one generated fixture came out as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generated {
    /// The file the container was written to.
    pub path: std::path::PathBuf,
    /// The SHA-256 of the file, as lowercase hexadecimal: the pin the double-generation
    /// gate holds two builds of the same fixture to.
    pub digest: String,
    /// How many words the container holds.
    pub words: u32,
    /// How many keys its index holds.
    pub keys: u32,
    /// How many bigram records it carries, zero outside `BigramPresent`.
    pub bigram_pairs: u32,
}

/// A generated fixture that has been loaded the way the plugin loads a dictionary.
///
/// The lexicon is the case's working handle: the same [`FstLexicon`] the engine reads
/// through, so a case asserts against the reads the engine itself performs.
pub struct Loaded {
    /// What the generation produced.
    pub generated: Generated,
    /// The loaded lexicon.
    pub lexicon: FstLexicon,
}
