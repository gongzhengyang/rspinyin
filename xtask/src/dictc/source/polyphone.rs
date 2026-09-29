//! The L3c correction table: the readings that outrank a word's baseline key.
//!
//! Responsibility: read the correction table and select the rows that actually change a
//! compiled word. Every row is checked for syllable legality where it is read: the table
//! only ever adjusts weights, but a row that cannot be read as syllables would silently
//! stop matching its word, so it is an error rather than a warning.
//!
//! Boundaries: this layer reads the table and filters it against the parsed words. It
//! never decides what a correction does -- the weight multiplier and the key a correction
//! produces belong to the container builder.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Result, bail};

use super::reading::parse_reading;

use super::Word;

/// Parses the L3c correction table.
///
/// Every row is checked for syllable legality at build time: the table only ever
/// adjusts weights, but a row that cannot be read as syllables would silently stop
/// matching its word, so it is an error rather than a warning.
pub(crate) fn load_polyphone(path: &Path) -> Result<Vec<(String, String)>> {
    let text = super::read_source(path)?;
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
