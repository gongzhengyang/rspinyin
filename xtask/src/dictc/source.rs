//! The raw-source layer of the compiler: reading the TSV formats under
//! `data/raw/` into words, readings and corrections.
//!
//! Responsibility: everything that knows what a source file looks like -- the L1
//! character table, the word list, the L3c correction table -- plus the syllable
//! folding every reading goes through before it can become a key.
//!
//! Boundaries: this layer never touches the container format and never writes
//! anything. It reports what it could not use instead of failing, because a raw
//! source is a moving target: a row that names a character the L1 table does not
//! cover must be counted and skipped, not turned into a build failure. The one
//! exception is the correction table, whose rows are asserted to be legal
//! syllables, since a row that cannot be read would silently stop matching.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use ime_core::segment::{SYLLABLE_COUNT, lookup, normalize};
use ime_dict::format::{self, ENTRY_FLAG_MASK, MAX_WORD_LEN};

/// Weight given to a single-character word whose row carries no weight column.
const DEFAULT_WEIGHT_SINGLE: u32 = 40_000;

/// Weight given to a two-character word whose row carries no weight column.
const DEFAULT_WEIGHT_TWO: u32 = 30_000;

/// Weight given to a three-character word whose row carries no weight column.
const DEFAULT_WEIGHT_THREE: u32 = 20_000;

/// Weight given to a four-character word whose row carries no weight column.
const DEFAULT_WEIGHT_FOUR: u32 = 15_000;

/// Weight given to a five-character-or-longer word with no weight column.
const DEFAULT_WEIGHT_LONG: u32 = 8_000;

/// Longest word the compiler accepts, in characters.
///
/// The container stores a syllable count in one byte with a documented ceiling of
/// 16, and a word text of at most 96 bytes, so a word that fits both limits is at
/// most 16 characters long whatever the characters weigh.
const MAX_WORD_CHARS: usize = 16;

/// Seed of the synthetic word generator; fixed so a probe run is reproducible.
const SYNTH_SEED: u64 = 0x5253_5044_0000_0001;

/// Width of the synthetic generator's weight range. Chosen so that about the same
/// share of synthetic words falls inside the band as in a real source.
const SYNTH_WEIGHT_SPAN: u64 = 116;

/// One parsed word, with the reading information the compiler needs.
#[derive(Debug)]
pub(crate) struct Word {
    /// Word text.
    pub(crate) text: String,
    /// Canonical key: the L3a reading, or the reading the row states.
    pub(crate) key: String,
    /// Every reading of every character, in source order, filled only for words
    /// inside the expansion band; empty means the word is not expanded.
    pub(crate) readings: Vec<Vec<String>>,
    /// Ranking weight.
    pub(crate) weight: u32,
    /// Bit set of `ime_dict::format::FLAG_*`.
    pub(crate) flags: u8,
    /// Whether some character of the word carries more than one reading, which is
    /// what makes the baseline reading capable of being wrong.
    pub(crate) polyphone: bool,
}

/// Counters for rows the compiler refused to use, by reason.
#[derive(Debug, Default)]
pub(crate) struct Skips {
    /// Rows whose word is not made of Han characters.
    pub(crate) non_han: u64,
    /// Rows whose word is longer than the container can store.
    pub(crate) too_long: u64,
    /// Rows whose weight column is not a number.
    pub(crate) bad_weight: u64,
    /// Rows whose flags column names no known flag.
    pub(crate) bad_flags: u64,
    /// Rows whose reading is not a legal syllable sequence.
    pub(crate) bad_reading: u64,
    /// Rows naming a character the L1 source does not cover.
    pub(crate) unknown_char: u64,
    /// Rows repeating a word already seen; the heavier row is kept.
    pub(crate) duplicate: u64,
    /// A few examples, so a skipped row can be reproduced without a debugger.
    pub(crate) examples: Vec<String>,
}

impl Skips {
    /// Records one skipped row.
    fn note(&mut self, reason: &str, line: u64, detail: &str) {
        match reason {
            "non-han" => self.non_han += 1,
            "too-long" => self.too_long += 1,
            "bad-weight" => self.bad_weight += 1,
            "bad-flags" => self.bad_flags += 1,
            "bad-reading" => self.bad_reading += 1,
            "unknown-char" => self.unknown_char += 1,
            "duplicate" => self.duplicate += 1,
            _ => {}
        }
        if self.examples.len() < 8 {
            self.examples
                .push(format!("{reason}: line {line}: {detail}"));
        }
    }

    /// Total number of skipped rows.
    pub(crate) fn total(&self) -> u64 {
        self.non_han
            + self.too_long
            + self.bad_weight
            + self.bad_flags
            + self.bad_reading
            + self.unknown_char
            + self.duplicate
    }
}

/// Loads the L1 single-character reading table.
///
/// The order of the source's reading column is preserved, because the first
/// reading is the one the L3a baseline uses. Readings that fold onto the same
/// toneless syllable are deduplicated: two spellings of one syllable would
/// otherwise produce two identical keys.
pub(crate) fn load_l1(path: &Path) -> Result<BTreeMap<char, Vec<String>>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut table: BTreeMap<char, Vec<String>> = BTreeMap::new();
    let mut illegal = 0u64;
    for line in text.lines() {
        let mut columns = line.split('\t');
        let Some(character) = columns.next().and_then(|column| column.chars().next()) else {
            continue;
        };
        let Some(readings) = columns.next() else {
            continue;
        };
        let entry = table.entry(character).or_default();
        for reading in readings.split(',') {
            match fold_reading(reading) {
                Some(syllable) if lookup(&syllable).is_some() => {
                    if !entry.contains(&syllable) {
                        entry.push(syllable);
                    }
                }
                _ => illegal += 1,
            }
        }
    }
    println!(
        "dictc: L1 {} characters, {illegal} readings outside the {SYLLABLE_COUNT}-syllable table",
        table.len()
    );
    Ok(table)
}

/// Folds one source reading onto the toneless spelling the syllable table uses.
///
/// Tone marks, tone digits, separators and the umlaut are removed; the result is
/// what `ime_core::segment::normalize` produces for the same sound, so `lv`, `lü`
/// and `lǚ` all reach `lü`.
fn fold_reading(raw: &str) -> Option<String> {
    let mut folded = String::with_capacity(raw.len());
    for character in raw.chars() {
        match character {
            'ā' | 'á' | 'ǎ' | 'à' | 'a' | 'A' => folded.push('a'),
            'ē' | 'é' | 'ě' | 'è' | 'e' | 'E' => folded.push('e'),
            'ī' | 'í' | 'ǐ' | 'ì' | 'i' | 'I' => folded.push('i'),
            'ō' | 'ó' | 'ǒ' | 'ò' | 'o' | 'O' => folded.push('o'),
            'ū' | 'ú' | 'ǔ' | 'ù' | 'u' | 'U' => folded.push('u'),
            'ǖ' | 'ǘ' | 'ǚ' | 'ǜ' | 'ü' | 'v' | 'V' => folded.push('v'),
            'ń' | 'ň' | 'ǹ' | 'n' | 'N' => folded.push('n'),
            'ḿ' | 'm' | 'M' => folded.push('m'),
            'ế' | 'ề' | 'ể' | 'ễ' | 'ệ' | 'ê' => folded.push('ê'),
            '0'..='5' => {}
            '\'' | ' ' | '-' | '·' => folded.push('\''),
            'b'..='z' | 'B'..='Z' => folded.push(character.to_ascii_lowercase()),
            _ => return None,
        }
    }
    let normalized = normalize(&folded).text;
    if normalized.is_empty() {
        return None;
    }
    Some(normalized)
}

/// Splits a reading into syllables, requiring exactly `count` of them.
///
/// Separators (`'` or whitespace) fix the cuts when the source writes them. A
/// reading written as one run of letters is segmented with a dynamic program over
/// the syllable table, preferring the longest syllable at each step, so `anan`
/// cuts as `an|an` rather than `a|nan`. The count has to come out exactly, which
/// is what makes `xian` invalid for a two-character word.
fn parse_reading(raw: &str, count: usize) -> Option<Vec<String>> {
    let mut pieces: Vec<String> = Vec::new();
    let mut current = String::new();
    for character in raw.chars() {
        if matches!(character, '\'' | ' ' | '-' | '·' | '|') {
            if !current.is_empty() {
                pieces.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(character);
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    if count > 0 && pieces.len() == count {
        let mut syllables = Vec::with_capacity(count);
        for piece in pieces {
            let syllable = fold_reading(&piece)?;
            lookup(&syllable)?;
            syllables.push(syllable);
        }
        return Some(syllables);
    }
    segment_exact(&fold_reading(raw)?, count)
}

/// Segments `text` into exactly `count` syllables, longest syllable first.
///
/// Greedy on syllable length with backtracking: at each position the longest legal
/// syllable is tried first, so `anan` splits as `an|an` rather than `a|nan`.
///
/// A plain forward dynamic program cannot express that preference. It records one
/// predecessor per `(end, used)` state and keeps whichever arrives first, and because
/// states are visited in increasing offset order the first arrival is the *shortest*
/// first syllable — the exact opposite of what the caller asked for.
fn segment_exact(text: &str, count: usize) -> Option<Vec<String>> {
    if count == 0 || count > text.len() {
        return None;
    }
    let mut chosen = Vec::with_capacity(count);
    // `dead[offset][remaining]` marks a state already proven unsplittable, which keeps
    // the search polynomial on inputs that backtrack repeatedly.
    let mut dead = vec![vec![false; count + 1]; text.len() + 1];
    if take_syllables(text, 0, count, &mut chosen, &mut dead) {
        Some(chosen)
    } else {
        None
    }
}

/// Depth-first step of [`segment_exact`]: longest syllable first, backtracking when the
/// remainder cannot supply the syllables still owed.
fn take_syllables(
    text: &str,
    offset: usize,
    remaining: usize,
    chosen: &mut Vec<String>,
    dead: &mut [Vec<bool>],
) -> bool {
    if remaining == 0 {
        return offset == text.len();
    }
    if dead[offset][remaining] {
        return false;
    }
    let mut length = ime_core::segment::MAX_SYLLABLE_LEN.min(text.len() - offset);
    while length >= 1 {
        let end = offset + length;
        if text.is_char_boundary(end)
            && lookup(&text[offset..end]).is_some()
            && let Some(syllable) = text.get(offset..end)
        {
            chosen.push(syllable.to_owned());
            if take_syllables(text, end, remaining - 1, chosen, dead) {
                return true;
            }
            chosen.pop();
        }
        length -= 1;
    }
    dead[offset][remaining] = true;
    false
}

/// Returns the default weight of a word of `characters` characters.
fn default_weight(characters: usize) -> u32 {
    match characters {
        1 => DEFAULT_WEIGHT_SINGLE,
        2 => DEFAULT_WEIGHT_TWO,
        3 => DEFAULT_WEIGHT_THREE,
        4 => DEFAULT_WEIGHT_FOUR,
        _ => DEFAULT_WEIGHT_LONG,
    }
}

/// Returns `true` for the Han ranges the dictionary accepts as word characters.
fn is_han(character: char) -> bool {
    matches!(character as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x2_0000..=0x2_FA1F)
}

/// Parses one flag column: either a bit mask or a comma-separated name list.
fn parse_flags(raw: &str) -> Option<u8> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Some(0);
    }
    let flags = match trimmed.parse::<u8>() {
        Ok(mask) => mask,
        Err(_) => {
            let mut flags = 0u8;
            for name in trimmed.split(',') {
                flags |= match name.trim() {
                    "surname" => format::FLAG_SURNAME,
                    "place" => format::FLAG_PLACE,
                    "term" => format::FLAG_TERM,
                    "user" => format::FLAG_USER,
                    _ => return None,
                };
            }
            flags
        }
    };
    if flags & !ENTRY_FLAG_MASK != 0 {
        return None;
    }
    Some(flags)
}

/// Parses the word list.
///
/// Columns are `word`, `reading`, `weight` and `flags`; everything after the word
/// is optional, and a second column that is all digits is read as the weight
/// rather than as a reading. A row whose reading column is absent or empty takes
/// the L3a baseline: the first reading of each character, concatenated.
pub(crate) fn load_words(
    path: &Path,
    l1: &BTreeMap<char, Vec<String>>,
    band_threshold: u32,
) -> Result<(Vec<Word>, Skips)> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut skips = Skips::default();
    let mut by_word: BTreeMap<String, Word> = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line_number = index as u64 + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let columns: Vec<&str> = trimmed.split('\t').map(str::trim).collect();
        let word = columns.first().copied().unwrap_or_default();
        if word.is_empty() {
            continue;
        }
        let characters = word.chars().count();
        if !word.chars().all(is_han) {
            skips.note("non-han", line_number, word);
            continue;
        }
        if characters > MAX_WORD_CHARS || word.len() > usize::from(MAX_WORD_LEN) {
            skips.note("too-long", line_number, word);
            continue;
        }
        let rest = columns.get(1..).unwrap_or_default();
        let second = rest.first().copied().unwrap_or_default();
        let is_reading = !second.is_empty() && !second.bytes().all(|byte| byte.is_ascii_digit());
        let digits_after_empty = second.is_empty()
            && rest.get(1).is_some_and(|column| {
                !column.is_empty() && column.bytes().all(|byte| byte.is_ascii_digit())
            });
        let (reading, weight_column, flags_column) = if is_reading {
            (
                Some(second),
                rest.get(1).copied().unwrap_or_default(),
                rest.get(2).copied().unwrap_or_default(),
            )
        } else if digits_after_empty {
            // `word<TAB><TAB>weight<TAB>flags`: the reading column is present but
            // empty, which is how a row states "compose the baseline reading".
            (
                None,
                rest.get(1).copied().unwrap_or_default(),
                rest.get(2).copied().unwrap_or_default(),
            )
        } else {
            (None, second, rest.get(1).copied().unwrap_or_default())
        };
        let weight = if weight_column.is_empty() {
            default_weight(characters)
        } else {
            match weight_column.parse::<u32>() {
                Ok(weight) => weight,
                Err(_) => {
                    skips.note("bad-weight", line_number, word);
                    continue;
                }
            }
        };
        let Some(flags) = parse_flags(flags_column) else {
            skips.note("bad-flags", line_number, word);
            continue;
        };

        // The reading lists hold every reading of every character: the first entry
        // of each is the baseline, the rest feed the L3b expansion.
        let (key, readings) = match reading {
            Some(explicit) => match parse_reading(explicit, characters) {
                Some(syllables) => (
                    syllables.join("'"),
                    compose_readings(word, l1).unwrap_or_default(),
                ),
                None => {
                    skips.note("bad-reading", line_number, word);
                    continue;
                }
            },
            None => match compose_readings(word, l1)
                .and_then(|readings| baseline_key(&readings).map(|key| (key, readings)))
            {
                Some(parsed) => parsed,
                None => {
                    skips.note("unknown-char", line_number, word);
                    continue;
                }
            },
        };
        let polyphone = readings.iter().any(|readings| readings.len() > 1);
        let entry = Word {
            text: word.to_owned(),
            key,
            readings: if weight >= band_threshold {
                readings
            } else {
                Vec::new()
            },
            weight,
            flags,
            polyphone,
        };
        match by_word.get(word) {
            Some(existing) if existing.weight >= entry.weight => {
                skips.note("duplicate", line_number, word);
                continue;
            }
            Some(_) => skips.note("duplicate", line_number, word),
            None => {}
        }
        by_word.insert(word.to_owned(), entry);
    }
    let words: Vec<Word> = by_word.into_values().collect();
    Ok((words, skips))
}

/// Returns every reading of every character of `word`, in source order.
///
/// Returns `None` when any character is missing from the L1 table, which is what
/// makes a row unusable: without a reading for every character there is neither a
/// baseline key nor an expansion.
fn compose_readings(word: &str, l1: &BTreeMap<char, Vec<String>>) -> Option<Vec<Vec<String>>> {
    let mut per_character = Vec::with_capacity(word.chars().count());
    for character in word.chars() {
        let readings = l1.get(&character)?;
        if readings.is_empty() {
            return None;
        }
        per_character.push(readings.clone());
    }
    Some(per_character)
}

/// Builds the L3a baseline key: the first reading of each character.
fn baseline_key(readings: &[Vec<String>]) -> Option<String> {
    let mut syllables = Vec::with_capacity(readings.len());
    for list in readings {
        syllables.push(list.first()?.clone());
    }
    Some(syllables.join("'"))
}

/// Parses the L3c correction table.
///
/// Every row is checked for syllable legality at build time: the table only ever
/// adjusts weights, but a row that cannot be read as syllables would silently stop
/// matching its word, so it is an error rather than a warning.
pub(crate) fn load_polyphone(path: &Path) -> Result<Vec<(String, String)>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut rows = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut columns = trimmed.split('\t').map(str::trim);
        let word = columns.next().unwrap_or_default();
        let reading = columns.next().unwrap_or_default();
        let characters = word.chars().count();
        let Some(syllables) = parse_reading(reading, characters) else {
            bail!(
                "{}:{line_number}: `{word}` reading `{reading}` is not a legal \
                 {characters}-syllable sequence",
                path.display()
            );
        };
        rows.push((word.to_owned(), syllables.join("'")));
    }
    println!("dictc: L3c {} polyphone rows", rows.len());
    Ok(rows)
}

/// Selects the correction rows that actually change a compiled word.
///
/// A row whose reading is already the word's canonical key is a confirmation and
/// is dropped. Rows naming a word outside the compiled list are counted, not
/// reported as errors: the table covers the language, the subset does not.
pub(crate) fn apply_polyphone(
    words: &[Word],
    rows: &[(String, String)],
) -> (Vec<(String, String)>, u64) {
    let mut index: BTreeMap<&str, &str> = BTreeMap::new();
    for word in words {
        index.insert(word.text.as_str(), word.key.as_str());
    }
    let mut corrections = Vec::new();
    let mut unmatched = 0u64;
    for (word, reading) in rows {
        match index.get(word.as_str()) {
            None => unmatched += 1,
            Some(key) if *key == reading.as_str() => {}
            Some(_) => corrections.push((word.clone(), reading.clone())),
        }
    }
    (corrections, unmatched)
}

/// Generates `count` synthetic words for the timing and size probe.
///
/// The generator is a fixed-seed xorshift: the same count always produces the same
/// list, so two probe runs measure the same work. Words are two characters drawn
/// from the CJK unified block, weighted so that roughly the same share falls
/// inside the expansion band as in a real source.
pub(crate) fn synth_words(
    count: u32,
    l1: &BTreeMap<char, Vec<String>>,
    band_threshold: u32,
) -> Vec<Word> {
    let mut state = SYNTH_SEED;
    let mut words = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let mut text = String::with_capacity(6);
        while text.chars().count() < 2 {
            let codepoint = 0x4E00 + (next_random(&mut state) % 3_000) as u32;
            if let Some(character) = char::from_u32(codepoint) {
                text.push(character);
            }
        }
        let weight = 1 + (next_random(&mut state) % SYNTH_WEIGHT_SPAN) as u32;
        let Some(readings) = compose_readings(&text, l1) else {
            continue;
        };
        let Some(key) = baseline_key(&readings) else {
            continue;
        };
        let polyphone = readings.iter().any(|readings| readings.len() > 1);
        words.push(Word {
            text,
            key,
            readings: if weight >= band_threshold {
                readings
            } else {
                Vec::new()
            },
            weight,
            flags: 0,
            polyphone,
        });
    }
    words
}

/// Advances the synthetic generator's xorshift state.
fn next_random(state: &mut u64) -> u64 {
    let mut value = *state;
    value ^= value >> 12;
    value ^= value << 25;
    value ^= value >> 27;
    *state = value;
    value.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

    /// A scratch directory unique to this test process and tag.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rspinyin-dictc-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        dir
    }

    #[test]
    fn test_fold_reading_strips_tones_digits_and_umlauts() {
        let cases = [
            ("nǐ", "ni"),
            ("hǎo", "hao"),
            ("ni3", "ni"),
            ("lǚ", "lü"),
            ("lv", "lü"),
            ("lüè", "lüe"),
            ("lve", "lüe"),
            ("jü", "ju"),
            ("zhōng", "zhong"),
            ("ê", "ê"),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                fold_reading(raw).as_deref(),
                Some(expected),
                "folding {raw:?}"
            );
            assert!(
                lookup(expected).is_some(),
                "{expected:?} must be a syllable"
            );
        }
        assert_eq!(fold_reading(""), None);
        assert_eq!(fold_reading("你好"), None);
    }

    #[test]
    fn test_parse_reading_requires_one_syllable_per_character() {
        let expected = Some(vec!["zhong".to_owned(), "guo".to_owned()]);
        assert_eq!(parse_reading("zhong'guo", 2), expected);
        assert_eq!(parse_reading("zhōngguó", 2), expected);
        assert_eq!(
            parse_reading("yinhang", 2),
            Some(vec!["yin".to_owned(), "hang".to_owned()])
        );
        assert_eq!(
            parse_reading("anan", 2),
            Some(vec!["an".to_owned(), "an".to_owned()]),
            "the longest syllable wins the tie"
        );
        // Longest syllable first, so `xian` splits as `xia|n` rather than `xi|an`. The
        // split is genuinely ambiguous without knowing the word, and no single rule
        // picks the linguistically likely reading for every case — `xi'an` and `xia'n`
        // are both real. Separated input is how a caller says which one it means, and
        // that path is covered by the `zhong'guo` case above; unseparated input falls
        // back to the documented longest-first rule.
        assert_eq!(
            parse_reading("xian", 2),
            Some(vec!["xia".to_owned(), "n".to_owned()])
        );
        assert_eq!(parse_reading("xian", 1), Some(vec!["xian".to_owned()]));
        // Three syllables is a legal split of `zhongguo`; the parser is not told which
        // characters the word has, so it cannot know the caller meant two.
        assert_eq!(
            parse_reading("zhongguo", 3),
            Some(vec!["zhong".to_owned(), "gu".to_owned(), "o".to_owned()])
        );
        assert_eq!(
            parse_reading("zhongguo", 9),
            None,
            "more syllables than letters"
        );
        assert_eq!(parse_reading("zzz", 1), None);
        assert_eq!(parse_reading("", 0), None);
    }

    #[test]
    fn test_parse_flags_accepts_names_and_masks() {
        assert_eq!(parse_flags(""), Some(0));
        assert_eq!(parse_flags("3"), Some(3));
        assert_eq!(
            parse_flags("surname,place"),
            Some(format::FLAG_SURNAME | format::FLAG_PLACE)
        );
        assert_eq!(parse_flags("nonsense"), None);
        assert_eq!(parse_flags("200"), None, "an undefined flag bit is refused");
    }

    #[test]
    fn test_load_words_reads_columns_weights_and_skips() {
        let dir = scratch("words");
        let path = dir.join("base.tsv");
        let text = "# comment\n\
                    中国\tzhong'guo\t500\n\
                    中国\t\t900\n\
                    行\thang\n\
                    好\n\
                    心\n\
                    银行\tyin'hang\t10\n\
                    银行\tzzz\t10\n\
                    hello\t\t10\n\
                    这个字很长很长很长很长很长很长很长很长很长很长很长很长很长很长很长很长很长\t\t1\n";
        fs::write(&path, text).expect("writing the fixture");

        let (words, skips) = load_words(&path, &test_l1(), 100).expect("parsing");
        let by_text: BTreeMap<&str, &Word> = words
            .iter()
            .map(|word| (word.text.as_str(), word))
            .collect();
        assert_eq!(words.len(), 5, "five distinct words survive");
        let zhong_guo = by_text.get("中国").expect("中国");
        assert_eq!(zhong_guo.weight, 900, "the heavier row wins");
        assert_eq!(zhong_guo.key, "zhong'guo");
        // Weight 900 clears the band threshold of 100, so the word carries every
        // character's readings for the multi-key expansion.
        assert_eq!(
            zhong_guo.readings,
            vec![vec!["zhong".to_owned()], vec!["guo".to_owned()]],
            "inside the band"
        );
        let xing = by_text.get("行").expect("行");
        assert_eq!(xing.key, "hang", "an explicit reading wins");
        assert_eq!(xing.readings.len(), 1, "inside the band");
        assert!(xing.polyphone);
        assert_eq!(by_text.get("好").expect("好").key, "hao");
        assert_eq!(by_text.get("好").expect("好").weight, DEFAULT_WEIGHT_SINGLE);
        assert_eq!(by_text.get("银行").expect("银行").key, "yin'hang");
        assert_eq!(skips.duplicate, 1);
        assert_eq!(skips.bad_reading, 1);
        assert_eq!(skips.non_han, 1);
        assert_eq!(skips.too_long, 1);
        assert_eq!(skips.bad_weight, 0);
        assert!(skips.total() >= 4);
        assert!(!skips.examples.is_empty());

        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_load_words_skips_rows_with_an_unknown_character() {
        let dir = scratch("unknown");
        let path = dir.join("base.tsv");
        fs::write(&path, "银龘\t\t10\n").expect("writing the fixture");
        let (words, skips) = load_words(&path, &test_l1(), 0).expect("parsing");
        assert!(
            words.is_empty(),
            "a word with an uncovered character is skipped"
        );
        assert_eq!(skips.unknown_char, 1);
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_load_polyphone_refuses_an_illegal_row() {
        let dir = scratch("poly");
        let path = dir.join("polyphone.tsv");
        fs::write(&path, "银行\tyin'hang\n行长\tzhang'zhang\n").expect("writing the fixture");
        let rows = load_polyphone(&path).expect("legal rows parse");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], ("银行".to_owned(), "yin'hang".to_owned()));

        fs::write(&path, "银行\tyin'hang\n行长\tzhangzhang'x\n").expect("writing the fixture");
        let failure = load_polyphone(&path).expect_err("an illegal row must fail");
        assert!(
            failure.to_string().contains("polyphone.tsv:2"),
            "the error names the row: {failure}"
        );
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_apply_polyphone_counts_rows_outside_the_subset() {
        let l1 = test_l1();
        let (words, _) = {
            let dir = scratch("apply");
            let path = dir.join("base.tsv");
            fs::write(&path, "中国\t\t100\n").expect("writing the fixture");
            let parsed = load_words(&path, &l1, 0).expect("parsing");
            fs::remove_dir_all(&dir).expect("cleaning up");
            parsed
        };
        let rows = vec![
            ("中国".to_owned(), "zhong'guo".to_owned()),
            ("银行".to_owned(), "yin'hang".to_owned()),
        ];
        let (corrections, unmatched) = apply_polyphone(&words, &rows);
        assert!(corrections.is_empty(), "a confirmation is not a correction");
        assert_eq!(unmatched, 1);
    }

    #[test]
    fn test_synth_words_are_deterministic_and_keep_the_band_a_minority() {
        let l1 = wide_l1();
        let first = synth_words(400, &l1, 100);
        let second = synth_words(400, &l1, 100);
        assert_eq!(
            first.len(),
            400,
            "the generator produces the requested count"
        );
        for (left, right) in first.iter().zip(second.iter()) {
            assert_eq!(left.text, right.text);
            assert_eq!(left.weight, right.weight);
        }
        let in_band = first.iter().filter(|word| word.weight >= 100).count();
        assert!(
            in_band * 4 < first.len(),
            "the synthetic band stays a minority: {in_band} of {}",
            first.len()
        );
    }

    /// An L1 table covering the synthetic generator's character range.
    ///
    /// Every third character carries a second reading, so the generator sees both
    /// monophonic and polyphonic words.
    fn wide_l1() -> BTreeMap<char, Vec<String>> {
        let syllables = [
            "zhong", "guo", "xin", "hang", "xing", "hao", "chang", "zhang",
        ];
        let mut table = BTreeMap::new();
        for offset in 0..3_000u32 {
            let Some(character) = char::from_u32(0x4E00 + offset) else {
                continue;
            };
            let index = offset as usize % syllables.len();
            let mut readings = vec![syllables[index].to_owned()];
            if offset % 3 == 0 {
                readings.push(syllables[(index + 3) % syllables.len()].to_owned());
            }
            table.insert(character, readings);
        }
        table
    }
}
