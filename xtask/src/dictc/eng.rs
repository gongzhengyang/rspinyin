//! `xtask dictc eng` — the mixed-English container compiler.
//!
//! Responsibility: turn the mixed-English word list (`data/raw/melt-eng.tsv`, rows of
//! `word<TAB>rank`) into the same container format the pinyin dictionary uses, with the
//! lowercase spelling as the key and the word's own casing as the display text, so that
//! `github` reaches the entry spelled `GitHub` (ADR-0016).
//!
//! The compiler also applies the build-time ranking mark the collision reduce list
//! carries (`data/raw/reduce-eng.tsv`): an entry whose spelling a pinyin typist leaves
//! mid-syllable — `ranch` cuts `ran|ch` — carries a quarter of its rank into the unigram
//! scaling, which is the only rank channel an English entry has. The judgment the list
//! is built on lives in the dictc `marks` module; `--emit-collisions` writes the
//! machine-judged candidates a reviewer confirms the list from.
//!
//! Boundaries: this is a plain compiler, not the L1–L5 pipeline. There is no reading to
//! compose, no polyphone to correct and no expansion to band: an English word is its own
//! key, and the ranking it carries is the one the file states. The unigram table is
//! scaled from the ranks the file carries so that the commoner word answers first when
//! two entries share everything else.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use clap::Parser;
use ime_dict::format::writer::DictWriter;
use ime_dict::format::{
    DictEntry, ENTRY_SIZE, PROB_Q12_MAX, SectionKind, UNIGRAM_ENTRY_SIZE, hash_word, pack_fst_value,
};
use ime_types::WordFlags;

/// Command-line surface of `xtask dictc eng`.
#[derive(Debug, Parser)]
pub struct EngArgs {
    /// Repository root; defaults to the directory containing the xtask crate.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Word list to compile, in `word<TAB>rank` columns.
    #[arg(long, default_value = "data/raw/melt-eng.tsv")]
    input: PathBuf,
    /// Container to write.
    #[arg(long, default_value = "data/compiled/eng.dict")]
    output: PathBuf,
    /// Source allowlist.
    #[arg(long, default_value = "data/sources.toml")]
    sources: PathBuf,
    /// The collision reduce list: one ASCII word per row, entries whose rank
    /// the compiler scales down by the marks module's reduce factor.
    #[arg(long, default_value = super::marks::DEFAULT_REDUCE_LIST)]
    reduce_list: PathBuf,
    /// Write the machine-judged collision candidates to a review file instead
    /// of only compiling: `word<TAB>rank<TAB>cut`, heaviest first.
    #[arg(long, value_name = "PATH")]
    emit_collisions: Option<PathBuf>,
}

/// One accepted row: the word as the user typed it in the list, and the rank it carries.
struct Row {
    word: String,
    weight: u32,
}

/// Compiles the mixed-English word list into `eng.dict`.
///
/// # Errors
/// Returns an error when the source is not on the allowlist, when a row is not two
/// columns of an alphabetic word and a decimal rank, when two rows fold onto the same
/// lowercase key, or when any file cannot be read or written. The error names the file
/// and line, so the offending input can be fixed directly.
pub fn run(args: EngArgs) -> Result<()> {
    let root = super::resolve_root(args.root.as_deref())?;
    let allowlist = super::load_allowlist(&root.join(&args.sources))?;
    let input_path = root.join(&args.input);
    let output_path = root.join(&args.output);
    super::check_source(&allowlist, &input_path)?;
    // The reduce list is a source like any other: registered and permissive
    // before the build reads a row of it.
    let reduce_path = root.join(&args.reduce_list);
    super::check_source(&allowlist, &reduce_path)?;

    let started = std::time::Instant::now();
    let text = fs::read_to_string(&input_path)
        .with_context(|| format!("cannot read {}", input_path.display()))?;
    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let number = number + 1;
        let (word, weight) = line.split_once('\t').with_context(|| {
            format!(
                "{}:{number}: expected `word<TAB>rank`",
                input_path.display()
            )
        })?;
        let weight: u32 = weight
            .parse()
            .with_context(|| format!("{}:{number}: rank is not a number", input_path.display()))?;
        ensure!(
            !word.is_empty() && word.chars().all(|ch| ch.is_ascii_alphabetic()),
            "{}:{number}: `{word}` is not one ASCII word; the mixed-English key is the \
             lowercased spelling and nothing else",
            input_path.display()
        );
        let key = word.to_lowercase();
        // A repeated key keeps the stronger row: the list is hand-curated, and a
        // duplicate is two editors writing the same word, not two words.
        let row = Row {
            word: String::from(word),
            weight,
        };
        match rows.get(&key) {
            Some(held) if held.weight >= row.weight => {}
            _ => {
                rows.insert(key, row);
            }
        }
    }
    let count = rows.len();
    ensure!(count > 0, "{}: the list is empty", input_path.display());

    // The collision review artifact, when the caller asked for one: every
    // entry the judgment selects, with the rank it carries in the source list.
    // The rows are the pre-mark ranks -- the review decides what the list
    // holds, and the reduction below is what the confirmed list does.
    if let Some(path) = args.emit_collisions.as_deref() {
        let report_path = root.join(path);
        let judged = collision_rows(&rows);
        let mut report = String::from(
            "# machine-judged collision candidates: word<TAB>rank<TAB>cut.\n\
             # The cut names the syllable|initial split the judgment found;\n\
             # review the rows and move the confirmed ones into the reduce list.\n",
        );
        for (word, rank, cut) in &judged {
            report.push_str(&format!("{word}\t{rank}\t{cut}\n"));
        }
        if let Some(parent) = report_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&report_path, &report)
            .with_context(|| format!("writing {}", report_path.display()))?;
        println!(
            "eng.dict: {} collision candidates written to {}",
            judged.len(),
            report_path.display()
        );
    }

    // The collision mark: an entry the reviewed reduce list names carries a
    // quarter of its rank into the unigram scaling, the only rank channel an
    // English entry has. A word the list names that the input list does not
    // hold is simply inert.
    let reduce_set = super::marks::load_reduce_list(&reduce_path)?;
    let mut reduced = 0usize;
    for (key, row) in rows.iter_mut() {
        if reduce_set.contains(key) {
            row.weight = reduced_weight(row.weight);
            reduced += 1;
        }
    }
    if reduced > 0 {
        println!(
            "eng.dict: {} entries reduced by the collision list {}",
            reduced,
            reduce_path.display()
        );
    }

    // Word ids are positions in hash order, the order the word list stores and the
    // unigram section is searched in.
    let mut hashed: Vec<(&str, &Row)> = rows.iter().map(|(key, row)| (key.as_str(), row)).collect();
    hashed.sort_by_key(|(key, row)| (hash_word(&row.word), *key));
    let id_of: BTreeMap<&str, u32> = hashed
        .iter()
        .enumerate()
        .map(|(id, (_, row))| Ok((row.word.as_str(), u32::try_from(id)?)))
        .collect::<Result<_, anyhow::Error>>()?;

    let max_weight = hashed.iter().map(|(_, row)| row.weight).max().unwrap_or(1);
    let mut strpool = Vec::new();
    let mut entries = Vec::with_capacity(count * ENTRY_SIZE);
    let mut unigram = Vec::with_capacity(count * UNIGRAM_ENTRY_SIZE);
    let mut wordlist = Vec::with_capacity(count * 4);
    for (id, (_, row)) in hashed.iter().enumerate() {
        let id = u32::try_from(id)?;
        let offset = u32::try_from(strpool.len()).context("the string pool does not fit u32")?;
        strpool.extend_from_slice(row.word.as_bytes());
        let characters = u8::try_from(row.word.chars().count())
            .context("a mixed-English word does not fit the u8 character field")?;
        entries.extend_from_slice(
            // One syllable is the field's shape for a single-key word; the TERM bit is
            // what marks the entry as a term of the mixed-English table. The character
            // count travels with the record, as the pinyin build's does.
            &DictEntry::new(
                offset,
                row.word.len() as u16,
                1,
                WordFlags::TERM.bits(),
                row.weight,
            )
            .with_characters(characters)
            .encode(),
        );
        unigram.extend_from_slice(&hash_word(&row.word).to_le_bytes());
        unigram.extend_from_slice(&prob_q12(row.weight, max_weight).to_le_bytes());
        unigram.extend_from_slice(&0u16.to_le_bytes());
        wordlist.extend_from_slice(&id.to_le_bytes());
    }

    // The FST builder takes keys in ascending byte order, which the file's is not.
    let mut keys: Vec<(&str, u32)> = rows
        .keys()
        .map(|key| (key.as_str(), id_of[rows[key].word.as_str()]))
        .collect();
    keys.sort_by_key(|(key, _)| key.as_bytes().to_vec());
    let mut fst_builder = fst::MapBuilder::memory();
    for (key, id) in keys {
        let packed = pack_fst_value(u64::from(id), 1)?;
        fst_builder.insert(key.as_bytes(), packed)?;
    }
    let fst = fst_builder.into_inner()?;

    let mut writer = DictWriter::new();
    for (kind, payload) in [
        (SectionKind::Fst, fst),
        (SectionKind::Entries, entries),
        (SectionKind::StrPool, strpool),
        (SectionKind::Unigram, unigram),
        (SectionKind::WordList, wordlist),
    ] {
        writer.add_section(kind, payload)?;
    }
    let container = writer.encode()?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&output_path, &container)
        .with_context(|| format!("writing {}", output_path.display()))?;
    let elapsed = started.elapsed();
    println!(
        "eng.dict: {count} words, {} bytes, {elapsed:?}",
        container.len()
    );
    Ok(())
}

/// Scales one rank into the Q12 probability the unigram section stores.
///
/// The heaviest rank maps to half the ceiling, not to the ceiling itself: the ceiling is
/// the score of a dictionary's single most frequent entry, and an English word that
/// competes only with the pass-through it replaces has no business claiming it. Every
/// rank maps above zero, so no entry is invisible to the model.
fn prob_q12(weight: u32, max_weight: u32) -> u16 {
    let scaled =
        i64::from(PROB_Q12_MAX) / 2 * i64::from(weight.max(1)) / i64::from(max_weight.max(1));
    scaled.clamp(1, i64::from(PROB_Q12_MAX) / 2) as u16
}

/// The rank one collision reduction leaves: the weight over the reduce
/// denominator.
///
/// Integer division floors, and the unigram scaling clamps every rank above
/// zero, so a reduced entry stays visible to the model -- the reduction sinks
/// a candidate, it does not erase one.
fn reduced_weight(weight: u32) -> u32 {
    weight / super::marks::REDUCE_DENOMINATOR
}

/// The rows the collision judgment selects, heaviest first.
///
/// The review artifact `--emit-collisions` writes: every entry whose lowercase
/// spelling cuts into a syllable plus an initial, with the rank it carries in
/// the source list, so a reviewer reads the head of the ranking first and
/// confirms the rows the reduce list should hold.
fn collision_rows(rows: &BTreeMap<String, Row>) -> Vec<(&str, u32, String)> {
    let mut judged: Vec<(&str, u32, String)> = rows
        .iter()
        .filter_map(|(key, row)| {
            let cut = super::marks::collision_cut(key)?;
            let (head, tail) = (&key[..cut], &key[cut..]);
            Some((key.as_str(), row.weight, format!("{head}+{tail}")))
        })
        .collect();
    judged.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    judged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prob_q12_scales_the_heaviest_rank_to_half_the_ceiling() {
        assert_eq!(prob_q12(1000, 1000), PROB_Q12_MAX / 2);
        assert_eq!(prob_q12(500, 1000), PROB_Q12_MAX / 4);
    }

    #[test]
    fn test_prob_q12_keeps_every_rank_above_zero_and_clamps_outliers() {
        // A zero rank is one word's worth of weight: it maps to the smallest share the
        // division gives, never to zero, and an oversized rank clamps at the ceiling.
        assert_eq!(prob_q12(0, 1000), 2);
        assert_eq!(prob_q12(1, 1000), 2);
        assert_eq!(prob_q12(5000, 1000), PROB_Q12_MAX / 2);
    }

    #[test]
    fn test_reduced_weight_scales_the_rank_and_stays_visible() {
        assert_eq!(reduced_weight(800), 200);
        assert_eq!(reduced_weight(798), 199, "integer division floors");
        assert_eq!(reduced_weight(3), 0);
        // A floored-to-zero rank still scores: the unigram scaling clamps
        // every rank above zero, so the reduction sinks a candidate, it does
        // not erase one.
        assert!(prob_q12(reduced_weight(3), 1000) >= 1);
    }

    #[test]
    fn test_collision_rows_lists_only_the_judged_rows_heaviest_first() {
        let rows: BTreeMap<String, Row> = [
            ("butter", 796),
            ("sad", 798),
            ("mama", 500),
            ("push", 643),
        ]
        .into_iter()
        .map(|(word, weight)| {
            (
                word.to_owned(),
                Row {
                    word: word.to_owned(),
                    weight,
                },
            )
        })
        .collect();
        let judged = collision_rows(&rows);
        assert_eq!(
            judged,
            vec![
                ("sad", 798, String::from("sa+d")),
                ("push", 643, String::from("pu+sh")),
            ],
            "`butter` and `mama` cut no syllable-plus-initial"
        );
    }
}
