//! The L1 character table and the syllable handling every reading goes through.
//!
//! Responsibility: read the L1 source, fold a written reading onto the toneless spelling
//! the syllable table uses, and split a reading into exactly as many syllables as its
//! word has characters.
//!
//! Boundaries: this layer knows the syllable alphabet and nothing else -- no word list,
//! no weights, no flags. It is the only place that decides what a legal reading is, so a
//! caller that needs one takes it from here rather than testing syllables itself.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use ime_core::segment::{SYLLABLE_COUNT, lookup, normalize};

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
pub(super) fn parse_reading(raw: &str, count: usize) -> Option<Vec<String>> {
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
