//! The word list: `word<TAB>reading<TAB>weight<TAB>flags` rows into parsed words.
//!
//! Responsibility: read the word list, resolve each row's key and weight, and count the
//! rows that could not be used. A row whose reading column is absent or empty takes the
//! L3a baseline: the first reading of each character, concatenated.
//!
//! Boundaries: this layer reads the word list and nothing else. It never reads the
//! correction table (that is [`super::polyphone`]) and never assembles a container; the
//! [`Word`]s it produces are the compiler's input.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use ime_dict::format::{self, ENTRY_FLAG_MASK, MAX_WORD_LEN};

use super::reading::parse_reading;

use super::{Skips, Word};

/// Weight given to a single-character word whose row carries no weight column.
pub(super) const DEFAULT_WEIGHT_SINGLE: u32 = 40_000;

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
pub(super) fn parse_flags(raw: &str) -> Option<u8> {
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
pub(super) fn compose_readings(
    word: &str,
    l1: &BTreeMap<char, Vec<String>>,
) -> Option<Vec<Vec<String>>> {
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
pub(super) fn baseline_key(readings: &[Vec<String>]) -> Option<String> {
    let mut syllables = Vec::with_capacity(readings.len());
    for list in readings {
        syllables.push(list.first()?.clone());
    }
    Some(syllables.join("'"))
}
