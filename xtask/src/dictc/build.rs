//! The container-building half of the compiler: keys, sections and the
//! measurements taken while assembling them.
//!
//! Responsibility: turn parsed words into the five payloads of the container --
//! the L3b expansion, the key -> word mapping, the entry table, the string pool
//! and the unigram table -- and resolve the frequency band that decides which
//! words are expanded.
//!
//! Boundaries: this layer never reads a source file's text (that is [`super::source`])
//! and never writes the container (that is `ime_dict::format::writer`). It is
//! deterministic by construction: every map it iterates is ordered, so the same
//! words and thresholds always produce the same bytes.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
use ime_dict::format::{
    DictEntry, ENTRY_SIZE, MAX_SYL_COUNT, MAX_WORDS_PER_KEY, PROB_Q12_MAX, UNIGRAM_ENTRY_SIZE,
    hash_word, pack_fst_value,
};

use super::source::Word;

/// Weight multiplier applied to the key a polyphone correction names.
///
/// The correction table is a ranking aid, not a correctness source: the key it
/// names is ranked above the baseline key for the same word, so the reading the
/// table states wins the tie inside that word's own candidate list.
const POLYPHONE_WEIGHT_FACTOR: u32 = 4;

/// Weight floor used when the frequency source is unavailable, so the band still
/// selects the head of the list instead of silently expanding everything.
const FALLBACK_BAND_THRESHOLD: u32 = 100;

/// How many expanded words the build log shows as a sample.
const EXPANDED_EXAMPLES: usize = 12;

/// What the compiler measured, for the build log.
#[derive(Debug, Default)]
pub(crate) struct Stats {
    /// Words that made it into the container.
    pub(crate) words: u64,
    /// Words inside the expansion band.
    pub(crate) band_words: u64,
    /// Words of each character length, indices 1 to 4, the rest in index 5.
    pub(crate) shape: [u64; 5],
    /// Words with at least one character that carries several readings.
    pub(crate) polyphone_risk: u64,
    /// Words inside the band that produced more than one key.
    pub(crate) expanded_words: u64,
    /// The first few expanded words with their key counts, so the expansion can
    /// be checked by hand without a second tool.
    pub(crate) expanded_examples: Vec<(String, u32)>,
    /// Keys produced by the L3b expansion beyond each word's baseline key.
    pub(crate) expanded_keys: u64,
    /// Keys the L3c correction table added or reweighted.
    pub(crate) polyphone_keys: u64,
    /// Polyphone rows that named a word outside the compiled list.
    pub(crate) polyphone_unmatched: u64,
    /// (key, word) pairs dropped because the key was already at its word ceiling.
    pub(crate) truncated_pairs: u64,
    /// Total FST keys.
    pub(crate) keys: u64,
}

impl Stats {
    /// Keys the L3a baseline alone would have produced.
    ///
    /// The L3b expansion and the L3c corrections are the only things that add keys on
    /// top of the baseline, so subtracting them recovers it. This is the denominator
    /// the growth figure has to use: comparing keys to *words* instead measures how much
    /// words collapse onto shared keys, which is unrelated to the expansion.
    pub(crate) fn baseline_keys(&self) -> u64 {
        self.keys
            .saturating_sub(self.expanded_keys)
            .saturating_sub(self.polyphone_keys)
    }
}

/// The expansion band: which words receive L3b multi-key expansion.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Band {
    /// Weight floor; a word at or above it is expanded.
    pub(crate) threshold: u32,
    /// The rank the caller asked for, for the build log.
    pub(crate) requested: u64,
}

/// The built sections and the measurements taken while building them.
pub(crate) struct Compiled {
    /// `ENTRIES` payload.
    pub(crate) entries: Vec<u8>,
    /// `STRPOOL` payload.
    pub(crate) strpool: Vec<u8>,
    /// `UNIGRAM` payload.
    pub(crate) unigram: Vec<u8>,
    /// `WORDLIST` payload.
    pub(crate) wordlist: Vec<u8>,
    /// `FST` payload.
    pub(crate) fst: Vec<u8>,
    /// Measurements for the build log.
    pub(crate) stats: Stats,
}

/// Builds every section of the container.
///
/// Word ids follow hash order, which is what makes `ENTRIES` and `UNIGRAM`
/// addressable by the same index and lets the unigram table be searched by hash.
pub(crate) fn compile(
    words: &[Word],
    band: Band,
    cap: usize,
    corrections: &[(String, String)],
) -> Result<Compiled> {
    let mut ordered: Vec<&Word> = words.iter().collect();
    ordered.sort_by(|left, right| {
        hash_word(&left.text)
            .cmp(&hash_word(&right.text))
            .then_with(|| left.text.cmp(&right.text))
    });

    let mut stats = Stats {
        words: ordered.len() as u64,
        ..Stats::default()
    };
    let mut corrections_by_word: BTreeMap<&str, &str> = BTreeMap::new();
    for (word, reading) in corrections {
        corrections_by_word.insert(word.as_str(), reading.as_str());
    }

    // One bucket per FST key. Iterating a `BTreeMap` yields the byte order the FST
    // builder requires, which is also why no hash map appears in this pipeline.
    let mut keys: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
    for (id, word) in ordered.iter().enumerate() {
        let id = id as u32;
        let characters = word.text.chars().count();
        stats.shape[characters.clamp(1, 5) - 1] += 1;
        if word.polyphone {
            stats.polyphone_risk += 1;
        }
        let in_band = word.weight >= band.threshold;
        if in_band {
            stats.band_words += 1;
        }
        let mut generated = if in_band && !word.readings.is_empty() {
            expand_keys(&word.readings, cap)
        } else {
            Vec::new()
        };
        if generated.len() > 1 {
            stats.expanded_words += 1;
            stats.expanded_keys += (generated.len() - 1) as u64;
            if stats.expanded_examples.len() < EXPANDED_EXAMPLES {
                stats
                    .expanded_examples
                    .push((word.text.clone(), generated.len() as u32));
            }
        }
        if generated.is_empty() {
            generated.push(word.key.clone());
        } else if !generated.contains(&word.key) {
            // The cap may have dropped the baseline combination; the word must
            // still be reachable under its own reading.
            generated.pop();
            generated.push(word.key.clone());
        }
        for key in generated {
            keys.entry(key).or_default().push((word.weight, id));
        }
        if let Some(reading) = corrections_by_word.get(word.text.as_str()) {
            let weight = word.weight.saturating_mul(POLYPHONE_WEIGHT_FACTOR);
            let candidates = keys.entry((*reading).to_owned()).or_default();
            match candidates
                .iter_mut()
                .find(|(_, candidate)| *candidate == id)
            {
                Some(pair) => pair.0 = weight,
                None => candidates.push((weight, id)),
            }
            stats.polyphone_keys += 1;
        }
    }

    let mut wordlist = Vec::new();
    let mut fst_builder = fst::MapBuilder::memory();
    for (key, mut candidates) in keys {
        candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        if candidates.len() > MAX_WORDS_PER_KEY as usize {
            stats.truncated_pairs += (candidates.len() - MAX_WORDS_PER_KEY as usize) as u64;
            candidates.truncate(MAX_WORDS_PER_KEY as usize);
        }
        let start = (wordlist.len() / 4) as u64;
        for (_, id) in &candidates {
            wordlist.extend_from_slice(&id.to_le_bytes());
        }
        fst_builder
            .insert(
                key.as_bytes(),
                pack_fst_value(start, candidates.len() as u32)?,
            )
            .map_err(|err| anyhow::anyhow!("building the FST: {err}"))?;
        stats.keys += 1;
    }

    let max_weight = ordered.iter().map(|word| word.weight).max().unwrap_or(0);
    let mut entries = Vec::with_capacity(ordered.len() * ENTRY_SIZE);
    let mut strpool = Vec::new();
    let mut unigram = Vec::with_capacity(ordered.len() * UNIGRAM_ENTRY_SIZE);
    for word in &ordered {
        let offset = strpool.len() as u32;
        strpool.extend_from_slice(word.text.as_bytes());
        let syllables = word.key.matches('\'').count() + 1;
        entries.extend_from_slice(
            &DictEntry::new(
                offset,
                word.text.len() as u16,
                syllables.min(MAX_SYL_COUNT as usize) as u8,
                word.flags,
                word.weight,
            )
            .encode(),
        );
        unigram.extend_from_slice(&hash_word(&word.text).to_le_bytes());
        unigram.extend_from_slice(&prob_q12(word.weight, max_weight).to_le_bytes());
        unigram.extend_from_slice(&0u16.to_le_bytes());
    }

    Ok(Compiled {
        entries,
        strpool,
        unigram,
        wordlist,
        fst: fst_builder.into_inner()?,
        stats,
    })
}

/// Generates the keys of one word: the cartesian product of its characters'
/// readings, most likely first.
///
/// Combinations are ordered by the sum of the reading ranks and then
/// lexicographically, so the first entry takes every character's most frequent
/// reading and the last ones are the least likely. The product is truncated to
/// `cap` after every character, which keeps a long word from materialising
/// millions of candidates while still producing the prefix of the fully sorted
/// product: a dropped prefix can only tie with a kept one, and ties are broken
/// lexicographically in favour of the kept one.
fn expand_keys(readings: &[Vec<String>], cap: usize) -> Vec<String> {
    if cap == 0 || readings.is_empty() {
        return Vec::new();
    }
    let mut combinations: Vec<(u32, Vec<usize>)> = vec![(0, Vec::new())];
    for choices in readings {
        let mut extended = Vec::with_capacity(combinations.len() * choices.len().max(1));
        for (rank, indices) in &combinations {
            for (index, _) in choices.iter().enumerate() {
                let mut next = indices.clone();
                next.push(index);
                extended.push((rank + index as u32, next));
            }
        }
        extended.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        extended.truncate(cap);
        combinations = extended;
    }
    combinations
        .into_iter()
        .filter_map(|(_, indices)| {
            let mut syllables = Vec::with_capacity(indices.len());
            for (position, index) in indices.iter().enumerate() {
                syllables.push(readings.get(position)?.get(*index)?.clone());
            }
            Some(syllables.join("'"))
        })
        .collect()
}

/// Scales a word's weight into the Q12 unigram score.
fn prob_q12(weight: u32, max_weight: u32) -> u16 {
    if max_weight == 0 {
        return 1;
    }
    let scaled = u64::from(weight) * u64::from(PROB_Q12_MAX) / u64::from(max_weight);
    scaled.clamp(1, u64::from(PROB_Q12_MAX)) as u16
}

/// Resolves the expansion band from the L4 source's frequency ranking.
///
/// The threshold is the weight of the `expand_top`-th most frequent entry of the
/// source, which is the frequency cut ADR-0000 measured as "the top 50k words". A
/// subset compiled from the head of that ranking therefore reproduces the
/// production expansion ratio instead of expanding every word it holds.
pub(crate) fn band_threshold(
    path: &Path,
    expand_top: u64,
    override_threshold: Option<u32>,
) -> Result<Band> {
    if expand_top == 0 {
        return Ok(Band {
            threshold: u32::MAX,
            requested: 0,
        });
    }
    let threshold = match override_threshold {
        Some(weight) => weight,
        None => match fs::read_to_string(path) {
            Ok(text) => {
                let mut weights: Vec<u32> = text
                    .lines()
                    .filter_map(|line| line.rsplit('\t').next())
                    .filter_map(|column| column.trim().parse::<u32>().ok())
                    .collect();
                if weights.is_empty() {
                    println!(
                        "dictc: frequency source {} has no frequencies; using the built-in band floor {FALLBACK_BAND_THRESHOLD}",
                        path.display()
                    );
                    FALLBACK_BAND_THRESHOLD
                } else {
                    weights.sort_unstable_by(|left, right| right.cmp(left));
                    let rank = usize::try_from(expand_top).unwrap_or(usize::MAX);
                    weights[rank.saturating_sub(1).min(weights.len() - 1)]
                }
            }
            Err(_) => {
                println!(
                    "dictc: frequency source {} is unavailable; using the built-in band floor {FALLBACK_BAND_THRESHOLD}",
                    path.display()
                );
                FALLBACK_BAND_THRESHOLD
            }
        },
    };
    Ok(Band {
        threshold,
        requested: expand_top,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use ime_dict::format::SectionKind;
    use ime_dict::format::reader::{Reader, Verify};
    use ime_dict::format::writer::DictWriter;

    /// A tiny L1 table covering the characters the tests use.
    fn test_l1() -> BTreeMap<char, Vec<String>> {
        let rows = [
            ('中', vec!["zhong"]),
            ('国', vec!["guo"]),
            ('心', vec!["xin"]),
            ('行', vec!["xing", "hang"]),
            ('银', vec!["yin"]),
            ('好', vec!["hao"]),
            ('长', vec!["chang", "zhang"]),
        ];
        rows.into_iter()
            .map(|(character, readings)| {
                (
                    character,
                    readings
                        .into_iter()
                        .map(str::to_owned)
                        .collect::<Vec<String>>(),
                )
            })
            .collect()
    }

    /// Wraps one word for the compiler, with its readings ready for expansion.
    fn word(text: &str, key: &str, weight: u32, l1: &BTreeMap<char, Vec<String>>) -> Word {
        let readings: Vec<Vec<String>> = text
            .chars()
            .map(|character| l1.get(&character).cloned().unwrap_or_default())
            .collect();
        let polyphone = readings.iter().any(|readings| readings.len() > 1);
        Word {
            text: text.to_owned(),
            key: key.to_owned(),
            readings,
            weight,
            flags: 0,
            polyphone,
        }
    }

    /// Encodes compiled sections into a container image.
    fn image_of(compiled: &Compiled) -> Vec<u8> {
        let mut writer = DictWriter::new();
        writer
            .add_section(SectionKind::Fst, compiled.fst.clone())
            .expect("fst");
        writer
            .add_section(SectionKind::Entries, compiled.entries.clone())
            .expect("entries");
        writer
            .add_section(SectionKind::StrPool, compiled.strpool.clone())
            .expect("strpool");
        writer
            .add_section(SectionKind::Unigram, compiled.unigram.clone())
            .expect("unigram");
        writer
            .add_section(SectionKind::WordList, compiled.wordlist.clone())
            .expect("wordlist");
        writer.encode().expect("encoding")
    }

    /// A scratch directory unique to this test process and tag.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rspinyin-dictc-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        dir
    }

    #[test]
    fn test_compile_round_trips_through_the_reader() {
        let l1 = test_l1();
        let words = vec![
            word("中国", "zhong'guo", 5_000, &l1),
            word("银行", "yin'hang", 4_000, &l1),
            word("行长", "hang'zhang", 300, &l1),
        ];
        let band = Band {
            threshold: 1_000,
            requested: 50_000,
        };
        let compiled = compile(&words, band, 4, &[]).expect("compiling");
        assert_eq!(compiled.stats.words, 3);
        assert_eq!(compiled.stats.band_words, 2);
        assert_eq!(compiled.stats.polyphone_risk, 2);
        assert_eq!(compiled.stats.expanded_words, 1, "only 银行 has a choice");
        assert_eq!(compiled.stats.expanded_keys, 1);

        let reader = Reader::parse(image_of(&compiled), Verify::Full).expect("parsing");
        assert_eq!(reader.entry_count(), 3);
        assert_eq!(reader.key_words("zhong'guo").expect("lookup"), vec!["中国"]);
        assert_eq!(reader.key_words("yin'hang").expect("lookup"), vec!["银行"]);
        assert_eq!(
            reader.key_words("yin'xing").expect("lookup"),
            vec!["银行"],
            "the second reading is reachable"
        );
        assert_eq!(
            reader.key_words("hang'zhang").expect("lookup"),
            vec!["行长"]
        );
        assert!(reader.unigram_lookup("银行").expect("unigram").is_some());
    }

    #[test]
    fn test_compile_is_deterministic() {
        let l1 = test_l1();
        let words = vec![
            word("中国", "zhong'guo", 5_000, &l1),
            word("银行", "yin'hang", 4_000, &l1),
            word("好心", "hao'xin", 200, &l1),
        ];
        let band = Band {
            threshold: 100,
            requested: 50_000,
        };
        let corrections = vec![("银行".to_owned(), "yin'hang".to_owned())];
        let first = compile(&words, band, 4, &corrections).expect("first");
        let second = compile(&words, band, 4, &corrections).expect("second");
        assert_eq!(first.fst, second.fst, "the FST must be byte-identical");
        assert_eq!(first.entries, second.entries);
        assert_eq!(first.strpool, second.strpool);
        assert_eq!(first.unigram, second.unigram);
        assert_eq!(first.wordlist, second.wordlist);
        // The container is what a build ships and what a release records the digest of, so
        // the determinism claim is about its bytes rather than about the payloads alone:
        // the header, the section table and the alignment padding are part of what two
        // runs over the same inputs have to agree on.
        let image = image_of(&first);
        assert_eq!(
            image,
            image_of(&second),
            "two runs over one input must produce one container"
        );
        assert_eq!(&image[0..4], b"RSPD", "the image is a container");
    }

    #[test]
    fn test_compile_applies_polyphone_corrections_without_duplicating_a_word() {
        let l1 = test_l1();
        let words = vec![word("银行", "yin'hang", 1_000, &l1)];
        let band = Band {
            threshold: 1_000,
            requested: 50_000,
        };
        let corrections = vec![("银行".to_owned(), "yin'hang".to_owned())];
        let compiled = compile(&words, band, 4, &corrections).expect("compiling");
        assert_eq!(compiled.stats.polyphone_keys, 1);
        let reader = Reader::parse(image_of(&compiled), Verify::Full).expect("parsing");
        assert_eq!(
            reader.key_words("yin'hang").expect("lookup"),
            vec!["银行"],
            "the corrected key holds the word exactly once"
        );
    }

    #[test]
    fn test_compile_adds_a_corrected_key_outside_the_band() {
        let l1 = test_l1();
        let words = vec![word("银行", "yin'xing", 5, &l1)];
        let band = Band {
            threshold: 1_000,
            requested: 50_000,
        };
        let corrections = vec![("银行".to_owned(), "yin'hang".to_owned())];
        let compiled = compile(&words, band, 4, &corrections).expect("compiling");
        assert_eq!(
            compiled.stats.expanded_words, 0,
            "the word is outside the band"
        );
        let reader = Reader::parse(image_of(&compiled), Verify::Full).expect("parsing");
        assert_eq!(reader.key_words("yin'hang").expect("lookup"), vec!["银行"]);
    }

    #[test]
    fn test_compile_keeps_the_baseline_key_when_the_cap_drops_it() {
        let readings = vec![
            vec!["xing".to_owned(), "hang".to_owned()],
            vec!["ye".to_owned(), "xie".to_owned()],
        ];
        // The product is four combinations; a cap of two keeps the first two, so a
        // word whose stated reading is the third must still be reachable.
        let expanded = expand_keys(&readings, 2);
        assert_eq!(expanded, vec!["xing'ye", "xing'xie"]);
        let l1 = test_l1();
        let words = vec![word("行长", "hang'zhang", 5_000, &l1)];
        let band = Band {
            threshold: 1_000,
            requested: 50_000,
        };
        let compiled = compile(&words, band, 2, &[]).expect("compiling");
        let reader = Reader::parse(image_of(&compiled), Verify::Full).expect("parsing");
        assert_eq!(
            reader.key_words("hang'zhang").expect("lookup"),
            vec!["行长"],
            "the word keeps its own key"
        );
    }

    #[test]
    fn test_expand_keys_is_capped_and_ordered_most_likely_first() {
        let readings = vec![
            vec!["xing".to_owned(), "hang".to_owned()],
            vec!["ye".to_owned()],
        ];
        assert_eq!(expand_keys(&readings, 4), vec!["xing'ye", "hang'ye"]);
        assert_eq!(expand_keys(&readings, 1), vec!["xing'ye"]);
        assert!(expand_keys(&readings, 0).is_empty());

        let both = vec![
            vec!["a".to_owned(), "b".to_owned()],
            vec!["c".to_owned(), "d".to_owned()],
        ];
        assert_eq!(expand_keys(&both, 4), vec!["a'c", "a'd", "b'c", "b'd"]);
        assert_eq!(expand_keys(&both, 2), vec!["a'c", "a'd"]);
    }

    /// The readings of the ten characters the expansion fixture gives two readings to.
    ///
    /// Every one is a syllable of the real table, which the test asserts: an invented
    /// reading would make the fixture measure an expansion no source could produce.
    const POLYPHONE_READINGS: [(&str, &str); 10] = [
        ("xing", "hang"),
        ("chang", "zhang"),
        ("zhong", "chong"),
        ("huan", "hai"),
        ("dou", "du"),
        ("le", "yue"),
        ("jue", "jiao"),
        ("shuo", "shui"),
        ("shu", "cu"),
        ("qiang", "jiang"),
    ];

    /// The reading of each of the ten characters that only ever start an unexpanded word.
    const MONOPHONE_FIRST: [&str; 10] = [
        "tian", "guo", "xin", "hao", "yin", "shan", "feng", "yun", "mao", "lin",
    ];

    /// The reading of each character that ends an expanded word.
    const SECOND_IN_BAND: [&str; 2] = ["ban", "cai"];

    /// The reading of each character that ends an unexpanded word.
    const SECOND_OUT_OF_BAND: [&str; 8] = ["da", "er", "fa", "ge", "he", "ji", "ke", "la"];

    /// Returns the `offset`-th character the expansion fixture uses.
    ///
    /// The characters are drawn from the CJK unified block one per fixture slot, so every
    /// character the fixture names is distinct and no two of its words can share one by
    /// accident.
    fn fixture_character(offset: usize) -> char {
        let codepoint = 0x4E00 + u32::try_from(offset).expect("the fixture is small");
        char::from_u32(codepoint).expect("a CJK code point")
    }

    /// The fixture the two expansion tests share.
    ///
    /// A hundred two-character words with a hundred distinct baseline keys, of which the
    /// twenty weighted into the band each carry a first character with two readings, so
    /// the expansion has a second key to generate, and the eighty weighted below it carry
    /// one reading each. That shape is what turns the key growth into a number rather than
    /// an accident -- twenty extra keys over a hundred baseline keys is twenty percent --
    /// and it is the shape the release build's word list has, in miniature.
    ///
    /// Returns the words, and for each in-band word its text and both of its keys: the
    /// baseline first, then the reading the expansion generates from it.
    fn expansion_fixture() -> (Vec<Word>, Vec<(String, String, String)>) {
        let mut l1: BTreeMap<char, Vec<String>> = BTreeMap::new();
        let polyphone: Vec<char> = POLYPHONE_READINGS
            .iter()
            .enumerate()
            .map(|(index, (heavy, light))| {
                let character = fixture_character(index);
                l1.insert(character, vec![heavy.to_string(), light.to_string()]);
                character
            })
            .collect();
        let monophone: Vec<char> = MONOPHONE_FIRST
            .iter()
            .enumerate()
            .map(|(index, reading)| {
                let character = fixture_character(POLYPHONE_READINGS.len() + index);
                l1.insert(character, vec![reading.to_string()]);
                character
            })
            .collect();
        let in_band_tail: Vec<char> = SECOND_IN_BAND
            .iter()
            .enumerate()
            .map(|(index, reading)| {
                let offset = POLYPHONE_READINGS.len() + MONOPHONE_FIRST.len() + index;
                let character = fixture_character(offset);
                l1.insert(character, vec![reading.to_string()]);
                character
            })
            .collect();
        let out_of_band_tail: Vec<char> = SECOND_OUT_OF_BAND
            .iter()
            .enumerate()
            .map(|(index, reading)| {
                let offset =
                    POLYPHONE_READINGS.len() + MONOPHONE_FIRST.len() + SECOND_IN_BAND.len() + index;
                let character = fixture_character(offset);
                l1.insert(character, vec![reading.to_string()]);
                character
            })
            .collect();

        let mut words = Vec::new();
        let mut in_band = Vec::new();
        for (index, first) in polyphone.iter().enumerate() {
            let (heavy, light) = POLYPHONE_READINGS[index];
            for (slot, second) in in_band_tail.iter().enumerate() {
                let text = format!("{first}{second}");
                let key = format!("{heavy}'{}", SECOND_IN_BAND[slot]);
                let alternate = format!("{light}'{}", SECOND_IN_BAND[slot]);
                words.push(word(&text, &key, 5_000, &l1));
                in_band.push((text, key, alternate));
            }
        }
        for (index, first) in monophone.iter().enumerate() {
            for (slot, second) in out_of_band_tail.iter().enumerate() {
                let text = format!("{first}{second}");
                let key = format!("{}'{}", MONOPHONE_FIRST[index], SECOND_OUT_OF_BAND[slot]);
                words.push(word(&text, &key, 100, &l1));
            }
        }
        (words, in_band)
    }

    /// The band the expansion fixture is measured against: the twenty words weighted 5000
    /// are inside it, the eighty weighted 100 are not.
    fn expansion_band() -> Band {
        Band {
            threshold: 1_000,
            requested: 50_000,
        }
    }

    #[test]
    fn test_compile_expands_every_polyphone_word_of_the_band() {
        let (words, in_band) = expansion_fixture();
        assert_eq!(words.len(), 100);
        assert_eq!(in_band.len(), 20);
        for (text, key, alternate) in &in_band {
            assert_ne!(key, alternate, "{text} must have a second reading");
            for syllable in key.split('\'').chain(alternate.split('\'')) {
                assert!(
                    ime_core::segment::lookup(syllable).is_some(),
                    "{syllable} is not a syllable of the table"
                );
            }
        }

        let compiled = compile(&words, expansion_band(), 4, &[]).expect("compiling");
        assert_eq!(compiled.stats.words, 100);
        // The twenty heavy words are the band, every one of them carries a character with a
        // second reading, and every one of them therefore generates a second key.
        assert_eq!(compiled.stats.band_words, 20);
        assert_eq!(compiled.stats.polyphone_risk, 20);
        assert_eq!(compiled.stats.expanded_words, 20);
        assert_eq!(compiled.stats.expanded_keys, 20);

        let reader = Reader::parse(image_of(&compiled), Verify::Full).expect("parsing");
        for (text, key, alternate) in &in_band {
            for candidate in [key.as_str(), alternate.as_str()] {
                let served = reader.key_words(candidate).expect("lookup");
                assert_eq!(served, vec![text.as_str()], "{candidate} serves {text}");
            }
        }
    }

    #[test]
    fn test_compile_keeps_the_key_growth_inside_the_target_band() {
        let (words, _) = expansion_fixture();
        let compiled = compile(&words, expansion_band(), 4, &[]).expect("compiling");
        let baseline = compiled.stats.baseline_keys();
        assert_eq!(baseline, 100, "one baseline key per word");
        // Twenty words gained one key each, over a hundred baseline keys.
        assert_eq!(compiled.stats.keys, 120);
        // The band is the one ADR-0000 measured the expansion at: a growth below it means
        // the expansion is not reaching the words people type, and the ceiling is what
        // `--max-key-growth` refuses a build for.
        let growth = crate::dictc::growth_percent(compiled.stats.keys, baseline);
        assert!(
            (15.0..=30.0).contains(&growth),
            "the expansion grew the key count by {growth:.1}%, outside the 15%..30% band"
        );
    }

    #[test]
    fn test_prob_q12_scales_to_the_heaviest_word() {
        assert_eq!(prob_q12(0, 0), 1);
        assert_eq!(prob_q12(1_000, 1_000), PROB_Q12_MAX);
        assert_eq!(prob_q12(500, 1_000), PROB_Q12_MAX / 2);
        assert_eq!(prob_q12(0, 1_000), 1, "a zero weight still scores");
    }

    #[test]
    fn test_band_threshold_reads_the_frequency_ranking() {
        let dir = scratch("band");
        let path = dir.join("jieba-dict.tsv");
        fs::write(&path, "中\t500\n国\t300\n心\t100\n好\t40\n的\t10\n").expect("writing");

        let band = band_threshold(&path, 2, None).expect("band");
        assert_eq!(
            band.threshold, 300,
            "the second entry's weight is the floor"
        );
        let band = band_threshold(&path, 0, None).expect("band");
        assert_eq!(
            band.threshold,
            u32::MAX,
            "no band when nothing is requested"
        );
        let band = band_threshold(&path, 50_000, None).expect("band");
        assert_eq!(
            band.threshold, 10,
            "a rank past the end clamps to the last entry"
        );
        let band = band_threshold(&path, 50_000, Some(7)).expect("band");
        assert_eq!(band.threshold, 7, "an explicit floor wins");
        let band = band_threshold(&dir.join("missing.tsv"), 50_000, None).expect("band");
        assert_eq!(band.threshold, FALLBACK_BAND_THRESHOLD);

        fs::remove_dir_all(&dir).expect("cleaning up");
    }
}
