//! The value audit of a compiled dictionary: what every frequency band costs in
//! bytes, what it contributes to held-out ranking, and what a trim pass under a
//! container target would drop.
//!
//! Responsibility: the `audit` channel reads a compiled container and reports the
//! per-band share table ([`ValueBandRow`]) plus the tail band's raw-weight
//! percentiles, which is what a human reads to choose a `--trim-target-mib`. The
//! trim planner ([`plan_trim_drops`], [`projected_container_bytes`],
//! [`floor_savings_bytes`]) lives here too because the compile-time trim pass in
//! [`super::normalize`] consumes the very same model: the report a reader judged
//! and the pass that acts on it cannot drift apart.
//!
//! # The value model
//!
//! A word's trim value is an ordering, not a number: drop the least valuable
//! first, and break every tie deterministically. Only the flattened tail band is
//! trimmable -- single characters keep their real frequency (they are what a user
//! falls back to when nothing longer matches) and head two-character words are
//! the core the held-out set exercises -- so the ordering only ever has to say
//! which tail word matters least:
//!
//! 1. lower raw corpus weight first; inside the tail the corpus frequency is the
//!    only signal left once the normalized weight is flattened;
//! 2. then words the L4 expansion band does not back: an out-of-band word is
//!    reachable under its baseline key alone, where an in-band word keeps several;
//! 3. then the more redundant word first -- the more neighbors share the word's
//!    key, the deeper the word sits in its own candidate list and the less likely
//!    a user ever reaches it;
//! 4. then shorter words first: a short word's content is largely reachable
//!    through its characters, and it frees fewer bytes, so it is the cheaper cut;
//! 5. then key order, then word order, which makes the total order independent
//!    of the order the rows arrived in.
//!
//! # The floor accounting
//!
//! The planner never promises a byte it cannot prove. Dropping a word provably
//! frees its `ENTRIES` row, its `UNIGRAM` row and its `STRPOOL` text and nothing
//! else for certain: the word may have lost its `WORDLIST` pairs to a full key
//! already, and the `FST` only shrinks when a key loses its last word. Those
//! unproven savings are deliberately counted as zero, so the projected size
//! after a planned trim is an upper bound of the real one and a build that plans
//! to fit its target does fit it. A flat per-section safety constant covers the
//! alignment padding, which can grow by a few bytes per section as the section
//! lengths shrink.

#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use clap::Args;
use fst::{IntoStreamer, Streamer};
use ime_dict::format::reader::Reader;
use ime_dict::format::{
    ENTRY_SIZE, SECTION_ALIGN, SECTIONS_OFFSET, SectionKind, UNIGRAM_ENTRY_SIZE, unpack_fst_value,
};
use serde::{Deserialize, Serialize};

use super::build::Compiled;
use super::manifest::manifest_path;
use super::normalize::{NormalizeRules, WeightBand};
use crate::tune::DEFAULT_HOLDOUT;

/// Four related per-word statistic maps returned by [`key_statistics`].
type WordStatistics<'text> = (
    BTreeMap<&'text str, u32>,
    BTreeMap<&'text str, u32>,
    BTreeMap<&'text str, f64>,
    BTreeMap<&'text str, String>,
);

/// Bytes in one mebibyte: the unit of every `*_mib` figure and flag here.
const BYTES_PER_MIB: f64 = 1024.0 * 1024.0;

/// Width of one `WORDLIST` slot: a `u32` word id, used for byte attribution.
const WORD_ID_BYTES: f64 = 4.0;

/// Slack added to the bytes a trim plan must free.
///
/// As section lengths shrink, each section's alignment padding can grow by up to
/// `SECTION_ALIGN - 1` bytes; six sections bound that growth well under this
/// constant. Counting it keeps the planner's fit promise a promise.
pub(crate) const TRIM_PADDING_SAFETY: u64 = 64;

/// Band label for single characters, mirroring [`WeightBand::CharFreq`].
pub const BAND_CHAR_FREQ: &str = "char-freq";
/// Band label for head two-character words, mirroring [`WeightBand::CoreTrueFreq`].
pub const BAND_2CHAR_CORE: &str = "2char-core";
/// Band label for the flattened tail, mirroring [`WeightBand::TailFlat`].
pub const BAND_TAIL_FLAT: &str = "tail-flat";

/// One row of the per-band value audit written next to the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ValueBandRow {
    /// Band label, e.g. "2char-core" / "3plus-tail".
    pub band: &'static str,
    /// Entries the band holds.
    pub entries: u64,
    /// Share of total container bytes this band occupies.
    pub bytes_mib: f32,
    /// Holdout top1 contribution estimate (from quality.rs band metrics).
    pub top1_share_pp: f32,
    /// Entries the trim pass would drop under the current target.
    pub trim_candidates: u64,
}

/// The per-word record the trim ordering consumes.
///
/// Both producers fill it from what they already hold: the compile-time pass
/// from its parsed word rows, the audit from the container's entries and key
/// lists. Nothing here reads a file, so the same rows order the same way in
/// both contexts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrimValue {
    /// Word text; the ordering's final tiebreak.
    pub(crate) text: String,
    /// A key the word is reachable under; the audit uses the lexicographically
    /// first one, the compile-time pass the canonical key.
    pub(crate) key: String,
    /// Raw corpus weight.
    pub(crate) weight: u32,
    /// Character count.
    pub(crate) characters: usize,
    /// Whether the L4 expansion band backs the word.
    pub(crate) in_expansion_band: bool,
    /// How many other words share the word's key.
    pub(crate) neighbors: u32,
}

/// The trim ordering over two candidates: which one a trim pass drops first.
///
/// See the module documentation for why each level sits where it does. `bool`
/// compares `false < true`, so [`TrimValue::in_expansion_band`] puts
/// out-of-band words first exactly when the lower levels tie.
pub(crate) fn trim_rank(left: &TrimValue, right: &TrimValue) -> Ordering {
    left.weight
        .cmp(&right.weight)
        .then_with(|| left.in_expansion_band.cmp(&right.in_expansion_band))
        .then_with(|| right.neighbors.cmp(&left.neighbors))
        .then_with(|| left.characters.cmp(&right.characters))
        .then_with(|| left.key.cmp(&right.key))
        .then_with(|| left.text.cmp(&right.text))
}

/// The bytes dropping one word provably frees: its `ENTRIES` row, its
/// `UNIGRAM` row and its `STRPOOL` text.
///
/// `WORDLIST` and `FST` savings are deliberately not counted; see the module
/// documentation. The floor is what makes a planned trim an upper-bound
/// guarantee rather than an estimate.
pub(crate) fn floor_savings_bytes(text_len: usize) -> u64 {
    ENTRY_SIZE as u64 + UNIGRAM_ENTRY_SIZE as u64 + text_len as u64
}

/// Converts a mebibyte target into bytes, refusing a target that cannot be one.
///
/// The conversion truncates, which errs toward the smaller target and therefore
/// toward the deeper trim: a plan built on a truncated target can only free
/// more than asked, never less.
///
/// # Errors
/// Returns an error when the target is not finite or not positive, which no
/// container target can meaningfully be.
pub(crate) fn target_bytes(target_mib: f64) -> Result<u64> {
    ensure!(
        target_mib.is_finite() && target_mib > 0.0,
        "audit: the trim target must be a finite positive number of mebibytes, got {target_mib}"
    );
    Ok((target_mib * BYTES_PER_MIB) as u64)
}

/// The exact byte size the writer will produce for these section payloads.
///
/// The layout is the writer's own: the header and section table occupy
/// [`SECTIONS_OFFSET`] bytes, every present section starts on the next
/// [`SECTION_ALIGN`] boundary in kind order, and empty sections take no room.
/// Keeping this function beside the trim planner is what lets the planner
/// reason about the artifact's real size without reading a file.
pub(crate) fn projected_container_bytes(compiled: &Compiled) -> u64 {
    let lens = [
        compiled.fst.len(),
        compiled.entries.len(),
        compiled.strpool.len(),
        compiled.unigram.len(),
        compiled.bigram.len(),
        compiled.wordlist.len(),
    ];
    let mut cursor = SECTIONS_OFFSET;
    for len in lens {
        if len == 0 {
            continue;
        }
        cursor += SECTION_ALIGN - 1;
        cursor -= cursor % SECTION_ALIGN;
        cursor += len as u64;
    }
    cursor
}

/// The outcome of one trim planning pass over the tail candidates.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct TrimPlan {
    /// Indices into the caller's own collection, in trim order.
    pub(crate) drop_indices: Vec<usize>,
    /// Floor bytes the planned drops free.
    pub(crate) freed_bytes: u64,
    /// How many candidates the tail offered in total, dropped or not.
    pub(crate) available: u64,
}

impl TrimPlan {
    /// Whether the tail could not cover the requested savings even by dropping
    /// every candidate: the caller's target then sits below the floor the
    /// protected bands set, and the compile-time pass must refuse it.
    pub(crate) fn exhausted(&self, savings_needed: u64) -> bool {
        self.freed_bytes < savings_needed
    }
}

/// Plans the deterministic drop set for one container target.
///
/// `candidates` holds `(index, value)` rows for every trimmable word in
/// whatever order the caller holds them; the plan sorts them by [`trim_rank`],
/// then walks the ranking accumulating [`floor_savings_bytes`] until
/// `savings_needed` is covered or the tail is exhausted. The walk is greedy and
/// the order total, so the same rows always plan the same drops.
pub(crate) fn plan_trim_drops(
    mut candidates: Vec<(usize, TrimValue)>,
    savings_needed: u64,
) -> TrimPlan {
    candidates.sort_by(|left, right| trim_rank(&left.1, &right.1));
    let mut plan = TrimPlan {
        available: candidates.len() as u64,
        ..TrimPlan::default()
    };
    for (index, value) in &candidates {
        if plan.freed_bytes >= savings_needed {
            break;
        }
        plan.freed_bytes = plan
            .freed_bytes
            .saturating_add(floor_savings_bytes(value.text.len()));
        plan.drop_indices.push(*index);
    }
    plan
}

/// Command-line surface of the value audit.
#[derive(Debug, Args)]
pub struct AuditArgs {
    /// Repository root; defaults to the directory containing the xtask crate.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Compiled container to audit.
    #[arg(long, default_value = super::DEFAULT_OUTPUT)]
    dict: PathBuf,
    /// Held-out set the top1 share is measured on.
    #[arg(long, default_value = DEFAULT_HOLDOUT)]
    holdout: PathBuf,
    /// Two-character weight floor dividing the core band from the tail; the
    /// compile's own default keeps the bands comparable across channels.
    #[arg(long, default_value_t = 20)]
    core_min_freq: u32,
    /// Simulate the trim pass against this container target, in mebibytes, and
    /// report per band how many entries it would drop. The audit writes
    /// nothing; the compile-time pass is `dictc --trim-target-mib`.
    #[arg(long)]
    trim_target_mib: Option<f64>,
    /// Print the machine-readable report instead of the human-readable one.
    #[arg(long)]
    json: bool,
}

/// One decile of the tail band's raw-weight distribution.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TailPercentile {
    /// The percentile this row cuts at, 10 to 100.
    pub percentile: u8,
    /// Raw weight at or below which the entries of this percentile sit.
    pub weight_ceiling: u32,
    /// Tail entries at or below the ceiling.
    pub cumulative_entries: u64,
    /// Floor bytes freeing all of them would recover, in mebibytes.
    pub cumulative_floor_mib: f32,
}

/// What the trim simulation decided for one target.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TrimSummary {
    /// The target the simulation planned against, in mebibytes.
    pub target_mib: f64,
    /// Tail entries available to drop.
    pub candidates: u64,
    /// Entries the plan drops.
    pub would_drop: u64,
    /// Floor bytes the drops free, in mebibytes.
    pub floor_freed_mib: f32,
    /// Whether the floor accounting reaches the target, padding slack included.
    pub reaches_target: bool,
}

/// The whole audit: band rows, the tail percentiles, and the trim simulation.
#[derive(Debug, Clone, Serialize)]
pub struct AuditReport {
    /// Container the audit measured, relative to the root when inside it.
    pub container: String,
    /// Size of the container in bytes.
    pub container_bytes: u64,
    /// Entries the container holds.
    pub entries: u64,
    /// One row per value band, in normalization-band order.
    pub bands: Vec<ValueBandRow>,
    /// The tail band's raw-weight deciles, for reading a cut point off.
    pub tail_percentiles: Vec<TailPercentile>,
    /// The trim simulation, when a target was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trim: Option<TrimSummary>,
}

/// The manifest fields the audit reads; everything else is ignored, so an
/// older manifest still serves the one figure the audit needs.
#[derive(Debug, Deserialize)]
struct ManifestView {
    /// The L3b expansion record, absent in a manifest that predates it.
    #[serde(default)]
    expansion: Option<ExpansionView>,
}

/// The expansion figures the audit reads.
#[derive(Debug, Deserialize)]
struct ExpansionView {
    /// Weight floor at or above which a word is expanded.
    #[serde(default)]
    band_weight: u32,
}

/// Entry point for the value audit.
///
/// Prints the per-band share table, the tail band's raw-weight percentiles and,
/// when a target was requested, the trim simulation. Nothing is written.
///
/// # Errors
/// Returns an error when the container or the held-out set cannot be read or
/// parsed, when the manifest beside the container is unreadable, or when the
/// banding knobs do not validate.
pub fn run(args: AuditArgs) -> Result<AuditReport> {
    let root = super::resolve_root(args.root.as_deref())?;
    let rules = NormalizeRules {
        core_min_freq: args.core_min_freq,
        ..NormalizeRules::default()
    };
    rules.validate()?;
    let dict = root.join(&args.dict);
    let reader = Reader::open(&dict).with_context(|| {
        format!(
            "audit: cannot open the compiled dictionary {}",
            dict.display()
        )
    })?;
    let container_bytes = fs::metadata(&dict)
        .with_context(|| format!("audit: cannot measure {}", dict.display()))?
        .len();

    let entries = reader.entry_count();
    let mut texts: Vec<&str> = Vec::with_capacity(entries as usize);
    let mut weights: Vec<u32> = Vec::with_capacity(entries as usize);
    for index in 0..entries {
        texts.push(reader.word_at(index)?);
        weights.push(reader.entry(index)?.weight);
    }

    // Per-word byte attribution: the linear sections are exact, the `FST` is
    // split evenly across keys and their member words, and whatever is left
    // (header, table, padding, `BIGRAM`) is spread evenly across entries. Every
    // byte lands in exactly one band, so the shares read as a partition of the
    // container rather than of an abstraction.
    let (neighbors, pair_counts, fst_shares, first_keys) = key_statistics(&reader, &texts)?;
    let section_total: u64 = reader.sections().iter().map(|row| row.len).sum();
    let bigram_bytes = section_len(&reader, SectionKind::Bigram)?;
    let overhead = container_bytes.saturating_sub(section_total);
    let spread = (overhead + bigram_bytes) as f64 / u64::from(entries).max(1) as f64;

    let mut band_entries = [0u64; 3];
    let mut band_bytes = [0f64; 3];
    let mut tail: Vec<(u32, usize)> = Vec::new();
    for index in 0..entries as usize {
        let text = texts[index];
        let characters = text.chars().count();
        let band = rules.band(characters, weights[index]);
        let slot = band_slot(band);
        band_entries[slot] += 1;
        let word_bytes = ENTRY_SIZE as f64
            + UNIGRAM_ENTRY_SIZE as f64
            + text.len() as f64
            + WORD_ID_BYTES * pair_counts.get(text).copied().unwrap_or(0) as f64
            + fst_shares.get(text).copied().unwrap_or(0.0)
            + spread;
        band_bytes[slot] += word_bytes;
        if band == WeightBand::TailFlat {
            tail.push((weights[index], text.len()));
        }
    }

    let holdout = root.join(&args.holdout);
    let cases = super::quality::load_holdout(&holdout)?;
    let lexicon = ime_dict::fst_index::FstLexicon::load(&dict)?;
    let rates = super::quality::measure(&cases, &lexicon)?;
    // The share each length band contributes to the held-out top1, in
    // percentage points: a band's first-choice hits over every case. The
    // quality bands cut by the expected word's length, so a sub-floor
    // two-character word counts toward the tail's byte share but toward the
    // two-character ranking share; over the common words a holdout set holds,
    // the two cuts coincide, which is why the estimate is good enough to gate.
    let case_count = cases.len() as f32;
    let top1_share_pp = [
        band_pp(rates.length_bands.single, case_count),
        band_pp(rates.length_bands.two, case_count),
        band_pp(rates.length_bands.long, case_count),
    ];

    let trim = match args.trim_target_mib {
        Some(target_mib) => {
            let band_weight = expansion_band_weight(&manifest_path(&dict))?;
            let facts = ContainerFacts {
                texts: &texts,
                weights: &weights,
                neighbors: &neighbors,
                first_keys: &first_keys,
                rules,
                band_weight,
                container_bytes,
            };
            Some(simulate_trim(&facts, target_mib)?)
        }
        None => None,
    };

    let mut bands = vec![
        ValueBandRow {
            band: BAND_CHAR_FREQ,
            entries: band_entries[0],
            bytes_mib: (band_bytes[0] / BYTES_PER_MIB) as f32,
            top1_share_pp: top1_share_pp[0],
            trim_candidates: 0,
        },
        ValueBandRow {
            band: BAND_2CHAR_CORE,
            entries: band_entries[1],
            bytes_mib: (band_bytes[1] / BYTES_PER_MIB) as f32,
            top1_share_pp: top1_share_pp[1],
            trim_candidates: 0,
        },
        ValueBandRow {
            band: BAND_TAIL_FLAT,
            entries: band_entries[2],
            bytes_mib: (band_bytes[2] / BYTES_PER_MIB) as f32,
            top1_share_pp: top1_share_pp[2],
            trim_candidates: 0,
        },
    ];
    if let Some(summary) = trim {
        bands[2].trim_candidates = summary.would_drop;
    }

    let report = AuditReport {
        container: relative_to_root(&root, &dict),
        container_bytes,
        entries: u64::from(entries),
        bands,
        tail_percentiles: tail_percentiles(tail),
        trim,
    };
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).context("rendering the audit report as JSON")?
        );
    } else {
        print_report(&report);
    }
    Ok(report)
}

/// Walks every key of the container once and collects the per-word statistics
/// the value model consumes: the busiest key each word sits on, how many key
/// slots the word occupies, its share of the `FST` section, and the
/// lexicographically first key it is reachable under.
///
/// # Errors
/// Returns an error when the container holds no keys, when a key is not a
/// syllable string, or when a word-list id points past the entry table.
fn key_statistics<'text>(
    reader: &Reader<Vec<u8>>,
    texts: &'text [&'text str],
) -> Result<WordStatistics<'text>> {
    let mut neighbors: BTreeMap<&str, u32> = BTreeMap::new();
    let mut pair_counts: BTreeMap<&str, u32> = BTreeMap::new();
    let mut fst_shares: BTreeMap<&str, f64> = BTreeMap::new();
    let mut first_keys: BTreeMap<&str, String> = BTreeMap::new();
    let fst_bytes = section_len(reader, SectionKind::Fst)?;
    // The FST split needs the key count before the first share can be
    // computed, so the stream is walked once to count and once to attribute.
    // Both walks see the keys in the FST's own order, so the sums are a
    // function of the container alone.
    let fst_map = reader.fst_map()?;
    let total_keys = {
        let mut stream = fst_map.range().into_stream();
        let mut total = 0u64;
        while stream.next().is_some() {
            total += 1;
        }
        total
    };
    ensure!(
        total_keys > 0,
        "audit: the container holds no keys, so the FST bytes cannot be attributed"
    );
    let per_key_share = fst_bytes as f64 / total_keys as f64;
    let mut stream = fst_map.range().into_stream();
    while let Some((key_bytes, packed)) = stream.next() {
        let key = std::str::from_utf8(key_bytes).map_err(|error| {
            anyhow::anyhow!("audit: the container holds a non-UTF-8 key: {error}")
        })?;
        let (start, count) = unpack_fst_value(packed);
        let count = count as usize;
        let per_member_share = per_key_share / count.max(1) as f64;
        for pair in 0..count {
            let start = usize::try_from(start)
                .map_err(|_| anyhow::anyhow!("audit: word-list start is past a usize"))?;
            let id = reader.word_list_id(start + pair)?;
            let text = texts.get(id as usize).copied().with_context(|| {
                format!(
                    "audit: word-list id {id} points past the {len}-entry table",
                    len = texts.len()
                )
            })?;
            let crowd = u32::try_from(count.saturating_sub(1)).unwrap_or(u32::MAX);
            let slot = neighbors.entry(text).or_insert(0);
            *slot = (*slot).max(crowd);
            *pair_counts.entry(text).or_insert(0) += 1;
            *fst_shares.entry(text).or_insert(0.0) += per_member_share;
            first_keys.entry(text).or_insert_with(|| key.to_owned());
        }
    }
    Ok((neighbors, pair_counts, fst_shares, first_keys))
}

/// The container facts one trim simulation reads, gathered once by [`run`].
struct ContainerFacts<'text> {
    /// Entry texts, in entry-id order.
    texts: &'text [&'text str],
    /// Entry raw weights, in entry-id order.
    weights: &'text [u32],
    /// Words sharing each word's busiest key.
    neighbors: &'text BTreeMap<&'text str, u32>,
    /// The lexicographically first key each word is reachable under.
    first_keys: &'text BTreeMap<&'text str, String>,
    /// The banding rules the entries are classified with.
    rules: NormalizeRules,
    /// The L4 expansion band weight, when the manifest states one.
    band_weight: Option<u32>,
    /// The container's byte size on disk.
    container_bytes: u64,
}

/// Simulates the trim pass against one target, on the container's own entries.
///
/// # Errors
/// Returns an error when the target cannot be converted into a byte count.
fn simulate_trim(facts: &ContainerFacts, target_mib: f64) -> Result<TrimSummary> {
    let target = target_bytes(target_mib)?;
    let band_weight = facts.band_weight.unwrap_or(0);
    let candidates: Vec<(usize, TrimValue)> = facts
        .texts
        .iter()
        .enumerate()
        .filter(|(index, text)| {
            facts
                .rules
                .band(text.chars().count(), facts.weights[*index])
                == WeightBand::TailFlat
        })
        .map(|(index, text)| {
            (
                index,
                TrimValue {
                    text: (*text).to_owned(),
                    key: facts.first_keys.get(text).cloned().unwrap_or_default(),
                    weight: facts.weights[index],
                    characters: text.chars().count(),
                    in_expansion_band: facts.weights[index] >= band_weight,
                    neighbors: facts.neighbors.get(text).copied().unwrap_or(0),
                },
            )
        })
        .collect();
    let savings_needed = if facts.container_bytes > target {
        facts
            .container_bytes
            .saturating_sub(target)
            .saturating_add(TRIM_PADDING_SAFETY)
    } else {
        // The container already fits: no trim is planned, mirroring the
        // compile-time pass, which only engages past the target.
        0
    };
    let plan = plan_trim_drops(candidates, savings_needed);
    Ok(TrimSummary {
        target_mib,
        candidates: plan.available,
        would_drop: plan.drop_indices.len() as u64,
        floor_freed_mib: plan.freed_bytes as f32 / BYTES_PER_MIB as f32,
        reaches_target: !plan.exhausted(savings_needed),
    })
}

/// Reads the expansion band weight out of the manifest beside the container.
///
/// A missing manifest is not an error: the band weight only breaks ties in the
/// value model, so the simulation proceeds with every word treated as
/// out-of-band and says so in the printed note.
///
/// # Errors
/// Returns an error when the manifest exists but cannot be parsed.
fn expansion_band_weight(manifest: &Path) -> Result<Option<u32>> {
    let Ok(text) = fs::read_to_string(manifest) else {
        println!(
            "audit: no manifest beside the container; treating every word as outside the expansion band"
        );
        return Ok(None);
    };
    let view: ManifestView = serde_json::from_str(&text)
        .with_context(|| format!("audit: unreadable manifest {}", manifest.display()))?;
    Ok(view.expansion.map(|expansion| expansion.band_weight))
}

/// The report slot of one normalization band, in the fixed row order.
fn band_slot(band: WeightBand) -> usize {
    match band {
        WeightBand::CharFreq => 0,
        WeightBand::CoreTrueFreq => 1,
        WeightBand::TailFlat => 2,
    }
}

/// One length band's contribution to the held-out top1, in percentage points.
fn band_pp(band: super::quality::Top1Band, case_count: f32) -> f32 {
    if case_count == 0.0 {
        return 0.0;
    }
    band.first as f32 / case_count * 100.0
}

/// The tail band's raw-weight deciles with cumulative floor bytes.
///
/// The rows are `(weight, text length)` pairs so the cumulative figure uses the
/// same floor model the planner plans with.
fn tail_percentiles(mut tail: Vec<(u32, usize)>) -> Vec<TailPercentile> {
    tail.sort_unstable();
    let total = tail.len();
    if total == 0 {
        return Vec::new();
    }
    let mut prefix = vec![0u64; total];
    let mut running = 0u64;
    for (slot, (_, len)) in tail.iter().enumerate() {
        running = running.saturating_add(floor_savings_bytes(*len));
        prefix[slot] = running;
    }
    (1..=10u8)
        .map(|step| {
            let percentile = step * 10;
            // The rank is the ceil of the percentile's share of the tail, so a
            // ten-entry tail cuts its first decile at the first entry.
            let rank = (percentile as usize * total).div_ceil(100).clamp(1, total) - 1;
            TailPercentile {
                percentile,
                weight_ceiling: tail[rank].0,
                cumulative_entries: rank as u64 + 1,
                cumulative_floor_mib: prefix[rank] as f32 / BYTES_PER_MIB as f32,
            }
        })
        .collect()
}

/// The container path relative to the root, for the report line.
fn relative_to_root(root: &Path, container: &Path) -> String {
    container
        .strip_prefix(root)
        .unwrap_or(container)
        .to_string_lossy()
        .into_owned()
}

/// The byte length of one container section.
fn section_len(reader: &Reader<Vec<u8>>, kind: SectionKind) -> Result<u64> {
    reader
        .sections()
        .iter()
        .find(|row| row.kind == kind)
        .map(|row| row.len)
        .with_context(|| format!("audit: the container holds no {} section row", kind.name()))
}

/// Prints the human-readable audit: one line per band, then the percentiles.
fn print_report(report: &AuditReport) {
    println!(
        "audit: {}: {} entries, {} container",
        report.container,
        report.entries,
        super::human_size(report.container_bytes)
    );
    let container_mib = report.container_bytes as f32 / BYTES_PER_MIB as f32;
    println!(
        "audit: {:<12} {:>9} {:>10} {:>8} {:>12} {:>16}",
        "band", "entries", "MiB", "share", "top1-share", "trim-candidates"
    );
    for row in &report.bands {
        let share = if container_mib > 0.0 {
            f64::from(row.bytes_mib) / f64::from(container_mib) * 100.0
        } else {
            0.0
        };
        println!(
            "audit: {:<12} {:>9} {:>10} {:>7.1}% {:>11.1}pp {:>16}",
            row.band, row.entries, row.bytes_mib, share, row.top1_share_pp, row.trim_candidates
        );
    }
    if !report.tail_percentiles.is_empty() {
        println!(
            "audit: tail raw-weight percentiles (cumulative entries and the floor bytes dropping them frees):"
        );
        for row in &report.tail_percentiles {
            println!(
                "audit:   p{:02} weight<={:<9} entries {:>9} floor {:.3}MiB",
                row.percentile,
                row.weight_ceiling,
                row.cumulative_entries,
                row.cumulative_floor_mib
            );
        }
    }
    if let Some(trim) = &report.trim {
        println!(
            "audit: trim simulation at {:.2} MiB: {} of {} tail entries would drop, floor frees {:.3}MiB, target reached {}",
            trim.target_mib,
            trim.would_drop,
            trim.candidates,
            trim.floor_freed_mib,
            if trim.reaches_target { "yes" } else { "no" }
        );
    }
}
