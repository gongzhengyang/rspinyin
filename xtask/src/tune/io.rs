//! The raw-table layer of the tuner: reading the formats the scoring runs on.
//!
//! Responsibility: everything that knows what an input file looks like -- the L1
//! character table with every reading of every character, the evaluation set, and the
//! baseline key composed from the L1 readings -- plus the tone folding every reading goes
//! through before it can become a syllable.
//!
//! Boundaries: this layer only reads. It never ranks, never writes and never touches the
//! lattice, and it holds no state between calls. A row it cannot use is an error rather
//! than a skip, because an evaluation set is hand-written: a row that could not be read
//! would silently leave the measurement.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use ime_core::segment::normalize;

use crate::dictc::quality::ReadingSource;

/// One row of an evaluation set: the key, the word expected first, and the weight the row
/// carries into the weighted rate.
#[derive(Debug)]
pub(super) struct Row {
    /// The `'`-separated toneless syllable key.
    pub(super) key: String,
    /// The word the row expects to be ranked first.
    pub(super) word: String,
    /// Weight of the row; one when the row carries no weight column.
    pub(super) weight: u32,
}

/// Reads `path` as UTF-8 text, naming the file in the error.
///
/// # Errors
///
/// Returns an error when the file cannot be read or is not UTF-8.
pub(super) fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))
}

/// Iterates the data lines of a table, skipping blanks and `#` comments, with
/// one-based line numbers for diagnostics.
pub(super) fn data_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
}

/// Loads a `pinyin<TAB>word[<TAB>weight]` evaluation set.
///
/// # Errors
///
/// Returns an error when the file cannot be read, when a row is missing its key or its
/// word column, or when a third column is neither a weight nor a reading layer.
pub(super) fn load_rows(path: &Path) -> Result<Vec<Row>> {
    let text = read_text(path)?;
    let mut rows = Vec::new();
    for (number, line) in data_lines(&text) {
        let mut columns = line.split('\t').map(str::trim);
        let key = columns.next().unwrap_or_default();
        let word = columns.next().unwrap_or_default();
        ensure!(
            !key.is_empty() && !word.is_empty(),
            "{}:{number}: a row needs a pinyin column and a word column",
            path.display()
        );
        let weight = row_weight(columns.next(), path, number)?;
        rows.push(Row {
            key: key.to_owned(),
            word: word.to_owned(),
            weight,
        });
    }
    Ok(rows)
}

/// Parses the third column of an evaluation row: a weight, or a reading layer.
///
/// The tuner's own evaluation set writes a weight there. A held-out row writes the layer
/// the reading came from instead, and the ranking does not weight a row by its layer, so
/// a layer spelling is accepted and carries the default weight of one. Anything else is
/// refused rather than ignored: a column the reader does not understand is a column
/// nobody validated, and a row that quietly lost its weight would move the weighted rate
/// without saying so.
///
/// # Errors
///
/// Returns an error when the column is neither an unsigned integer nor one of the three
/// layer spellings.
fn row_weight(raw: Option<&str>, path: &Path, number: usize) -> Result<u32> {
    let Some(raw) = raw.filter(|column| !column.is_empty()) else {
        return Ok(1);
    };
    if let Ok(weight) = raw.parse::<u32>() {
        return Ok(weight);
    }
    ensure!(
        ReadingSource::parse(raw).is_some(),
        "{}:{number}: `{raw}` is neither a weight nor a reading layer",
        path.display()
    );
    Ok(1)
}

/// Folds one source reading onto the toneless spelling the syllable table uses;
/// the compiler's copy of this function is private to its own module tree.
pub(super) fn fold_reading(raw: &str) -> Option<String> {
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
            '\'' | ' ' | '-' | '·' | '|' => folded.push('\''),
            'b'..='z' | 'B'..='Z' => folded.push(character.to_ascii_lowercase()),
            _ => return None,
        }
    }
    let normalized = normalize(&folded).text;
    (!normalized.is_empty()).then_some(normalized)
}

/// Loads every toneless reading of every character of the L1 table.
///
/// The first reading is the one the compiler builds a word's baseline key from, but a
/// caller that asks whether a word's reading is *forced* needs the whole list: a character
/// with a second reading is a character whose first one can be wrong, which is the entire
/// reason the L3b expansion and the L3c correction table exist. Readings are folded and
/// de-duplicated in source order, so a source that lists one reading twice does not read
/// as a polyphone.
///
/// # Errors
///
/// Returns an error when the table cannot be read.
pub(super) fn load_l1_readings(path: &Path) -> Result<BTreeMap<char, Vec<String>>> {
    let text = read_text(path)?;
    let mut table: BTreeMap<char, Vec<String>> = BTreeMap::new();
    let mut unusable = 0u64;
    for (_, line) in data_lines(&text) {
        let mut columns = line.split('\t');
        let character = columns.next().and_then(|column| column.chars().next());
        let readings: Vec<String> = columns
            .next()
            .into_iter()
            .flat_map(|source| source.split(','))
            .filter_map(fold_reading)
            .collect();
        let Some(character) = character.filter(|_| !readings.is_empty()) else {
            unusable += 1;
            continue;
        };
        let slot = table.entry(character).or_default();
        for reading in readings {
            if !slot.contains(&reading) {
                slot.push(reading);
            }
        }
    }
    println!("tune: L1 {} characters, {unusable} unusable", table.len());
    Ok(table)
}

/// Loads the first toneless reading of every character of the L1 table. That first
/// reading is the one the compiler uses for the L3a baseline key.
///
/// # Errors
///
/// Returns an error when the table cannot be read.
pub(super) fn load_l1(path: &Path) -> Result<BTreeMap<char, String>> {
    let readings = load_l1_readings(path)?;
    let mut first = BTreeMap::new();
    for (character, all) in readings {
        // A character with no reading at all never enters the table, so the first
        // reading is always there; the check keeps that from becoming an index.
        if let Some(reading) = all.into_iter().next() {
            first.insert(character, reading);
        }
    }
    Ok(first)
}

/// Whether some character of `word` carries more than one reading.
///
/// A word made only of single-reading characters has exactly one legal reading, so its
/// baseline key is not a guess. One polyphone character is enough to make it one.
pub(super) fn has_polyphone(word: &str, readings: &BTreeMap<char, Vec<String>>) -> bool {
    word.chars()
        .any(|character| readings.get(&character).is_some_and(|all| all.len() > 1))
}

/// Composes the L3a baseline key of `word`. `None` means a character the L1 table
/// does not cover, which makes the word unusable -- there is no key without a
/// reading for every character.
pub(super) fn baseline_key(word: &str, l1: &BTreeMap<char, String>) -> Option<String> {
    let mut syllables: Vec<&str> = Vec::with_capacity(word.chars().count());
    for character in word.chars() {
        syllables.push(l1.get(&character)?.as_str());
    }
    Some(syllables.join("'"))
}
