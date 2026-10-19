//! `xtask dictc` -- the offline dictionary compiler.
//!
//! Responsibility: turn the whitelisted raw sources under `data/raw/` into the
//! container described by `ime_dict::format`, and report what it did. The
//! pipeline is: source allowlist -> L1 single-character readings -> word list
//! (L3a baseline keys) -> L3b frequency-targeted multi-key expansion -> L3c
//! polyphone weight correction -> ENTRIES/STRPOOL/UNIGRAM/WORDLIST/FST -> one
//! atomic write.
//!
//! Boundaries: this is a build-time tool. It is the only place that knows the raw
//! source formats, it never runs at IME runtime, and it holds no state between
//! runs. Every diagnostic is English and none of them contains user input.
//!
//! The module is split by responsibility: this root holds the command line, the
//! source allowlist and the pipeline, [`source`] reads the raw text formats, and
//! [`build`] assembles the container.
//!
//! # Why the expansion is bounded
//!
//! Expanding every word into the cartesian product of its characters' readings
//! costs about +176% FST keys and buys little over expanding only the words people
//! actually type: the highest-frequency band recovers most of the available
//! benefit for a fraction of the cost. The band is therefore a frequency
//! threshold, and the build fails when the measured key growth passes
//! `--max-key-growth`.

mod bigram;
mod budget;
// `pub(crate)` because the smoke baseline's fixture compiles its container through
// the same `compile` the tool uses: a test evaluation that passes passes against
// what `dictc` writes, and a hand-built section would prove only that the reader
// agrees with the test.
pub(crate) mod build;
mod manifest;
pub(crate) mod normalize;
pub mod quality;
pub(crate) mod source;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use clap::Args;
use ime_dict::format::writer::DictWriter;
use ime_dict::format::{BigramRecord, SectionKind, collision_limit};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::dictc::build::{Band, Stats, compile};
use crate::dictc::source::{
    Skips, apply_polyphone, load_l1, load_polyphone, load_words, read_source, synth_words,
};

/// Default word list, relative to the repository root.
///
/// The word list and the L4 frequency source are the same file at production scale:
/// jieba's rows are `word<TAB>frequency`, which [`load_words`] reads directly -- an
/// all-digit second column is a weight, and the baseline reading is composed from L1.
/// The development subset compiled by the tasks' original matrix passes
/// `--input data/raw/base.tsv --expand-top 2856` instead.
pub const DEFAULT_INPUT: &str = "data/raw/jieba-dict.tsv";
/// Default output file, relative to the repository root.
pub const DEFAULT_OUTPUT: &str = "data/compiled/base.dict";
/// Default L3c weight-correction table.
pub const DEFAULT_POLYPHONE: &str = "data/raw/polyphone.tsv";
/// Default L1 source: one character per row, readings separated by commas.
pub const DEFAULT_L1: &str = "data/raw/pinyin-data.tsv";
/// Default L4 source: one word per row, word frequency in the second column.
pub const DEFAULT_L4: &str = "data/raw/jieba-dict.tsv";
/// Default L2 source: the two-character layer that replaces the input's.
pub const DEFAULT_L2: &str = "data/raw/huayu.tsv";
/// Default source allowlist.
pub const DEFAULT_SOURCES: &str = "data/sources.toml";
/// Optional corpus used to generate the bigram section; absence yields an empty v2.
pub const DEFAULT_CORPUS: &str = "data/raw/corpus/zh-wikipedia.txt";

/// Command-line surface of `xtask dictc`.
#[derive(Debug, Args)]
pub struct DictcArgs {
    /// Repository root; defaults to the directory containing the xtask crate.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Word list to compile, in `word<TAB>reading<TAB>weight<TAB>flags` columns.
    #[arg(long, default_value = DEFAULT_INPUT)]
    input: PathBuf,
    /// Container to write.
    #[arg(long, default_value = DEFAULT_OUTPUT)]
    output: PathBuf,
    /// L3c weight-correction table, in `word<TAB>reading` columns.
    #[arg(long, default_value = DEFAULT_POLYPHONE)]
    polyphone: PathBuf,
    /// L1 source of single-character readings.
    #[arg(long, default_value = DEFAULT_L1)]
    l1: PathBuf,
    /// L4 source whose frequency ranking defines the expansion band.
    #[arg(long, default_value = DEFAULT_L4)]
    l4: PathBuf,
    /// L2 source whose two-character layer replaces the input's: its weight and
    /// reading win where both sources hold a word, and its words fill the gaps.
    /// A path that does not exist leaves the input untouched, which is what a
    /// checkout that has not fetched this source builds from.
    #[arg(long, default_value = DEFAULT_L2)]
    l2: PathBuf,
    /// Two-character entries of the L2 source below this frequency are not
    /// admitted at all. The remedy when the merged build outgrows its budget
    /// is a tighter floor here, not a raised ceiling.
    #[arg(long, default_value_t = 0)]
    l2_min_freq: u32,
    /// Reading-frequency floor, in percent of the character's top reading, a
    /// stated reading must clear to survive the automatic polyphone check.
    /// Rime-ice measures 5:100 on its own list.
    #[arg(long, default_value_t = 5)]
    polyphone_freq_ratio: u8,
    /// Source allowlist.
    #[arg(long, default_value = DEFAULT_SOURCES)]
    sources: PathBuf,
    /// Corpus used for adjacent-word co-occurrence; omitted when not fetched.
    #[arg(long, default_value = DEFAULT_CORPUS)]
    corpus: PathBuf,
    /// Expand the N highest-frequency words of the L4 source; 0 disables L3b.
    ///
    /// The default is ADR-0000's production value: the top 50k words of the frequency
    /// source, measured at +17.9% keys over the full 349k-word list with the committed
    /// polyphone table, inside the 30% ceiling. The development subset compiled by the
    /// original task matrix holds 5441 words drawn from across the frequency range, so
    /// the same cut would admit 43% of it and break the ceiling; that matrix passes
    /// `--expand-top 2856` (a weight cut, landing the subset at +22.5%) together with
    /// its `--input`.
    #[arg(long, default_value_t = 50_000)]
    expand_top: u64,
    /// L3b cap on the number of keys generated per word.
    #[arg(long, default_value_t = 4)]
    expand_cap: usize,
    /// Absolute weight floor for the expansion band, overriding `--expand-top`.
    #[arg(long)]
    expand_min_weight: Option<u32>,
    /// Key-growth ceiling in percent; a larger growth fails the build.
    #[arg(long, default_value_t = 30.0)]
    max_key_growth: f64,
    /// Highest number of bigram pairs the compiler will emit.
    #[arg(long, default_value_t = 200_000)]
    max_bigram_pairs: u32,
    /// Minimum `count * PMI` a bigram pair needs to survive the cut.
    #[arg(long, default_value_t = 1.0)]
    min_bigram_score: f32,
    /// The constant every tail-band word receives after weight normalization:
    /// three-or-more character words, and two-character words below
    /// `--core-min-freq`.
    #[arg(long, default_value_t = 100)]
    tail_flat_weight: u32,
    /// Two-character words below this source frequency drop into the tail band:
    /// below it the frequency is noise, not signal. A word at the floor stays in
    /// the core band.
    #[arg(long, default_value_t = 20)]
    core_min_freq: u32,
    /// Compile N synthetic words instead of reading the input file.
    #[arg(long, hide = true)]
    synth: Option<u32>,
}

/// One row of the source allowlist.
#[derive(Debug, Deserialize)]
struct Source {
    /// File name stem: the raw file is `data/raw/<id>.tsv`.
    id: String,
    /// `upstream` (hash-pinned download) or `derived` (generated here).
    kind: String,
    /// Licence identifier.
    spdx: String,
    /// Human-readable licence name.
    license: String,
    /// Source URL.
    url: String,
    /// Download date, empty until the file is fetched.
    #[serde(default)]
    retrieved: String,
    /// Pinned SHA256, empty for derived sources.
    #[serde(default)]
    sha256: String,
    /// Whether the licence permits redistribution of derived data.
    permissive: bool,
    /// ADR-0000 layer the source feeds: `L1`, `L2`, `L3`, `L4` or `L5`.
    ///
    /// The manifest records it, so a reader of a compiled dictionary can trace a weight
    /// back to the layer that supplied it without consulting this file.
    layer: String,
}

/// The parsed allowlist document.
#[derive(Debug, Deserialize)]
struct Allowlist {
    /// Registered sources.
    source: Vec<Source>,
}

/// Entry point for `xtask dictc`.
///
/// # Errors
/// Returns an error when a source is not on the allowlist or fails its hash pin,
/// when a polyphone row is not a legal syllable sequence, when the key growth
/// passes the ceiling, or when any file cannot be read or written. The error names
/// the file and row, so the offending input can be fixed directly.
pub fn run(args: DictcArgs) -> Result<()> {
    let root = resolve_root(args.root.as_deref())?;
    // The normalization rules are checked before any file is read: a combination
    // that would flatten the whole core band is a caller error, and the atomic
    // write downstream is for crashes, not for refuse-able arguments.
    let normalize_rules = normalize::NormalizeRules {
        tail_flat_weight: args.tail_flat_weight,
        core_min_freq: args.core_min_freq,
        ..normalize::NormalizeRules::default()
    };
    normalize_rules.validate()?;
    let allowlist = load_allowlist(&root.join(&args.sources))?;

    // The allowlist gate comes first: a source that is not registered, or whose
    // bytes do not match the pinned hash, must stop the build before any of it is
    // compiled, because a copyleft word list merged into base.dict cannot be
    // removed afterwards without rebuilding every weight and every rank.
    let l1_path = root.join(&args.l1);
    let l4_path = root.join(&args.l4);
    let polyphone_path = root.join(&args.polyphone);
    let input_path = root.join(&args.input);
    // The L2 source is merged only when it is there: a checkout that has not
    // fetched it builds the un-merged list, so the absence is ordinary state
    // rather than a refusal -- but a file that IS there must be on the
    // allowlist and match its pin before any of the build runs.
    let l2_path = root.join(&args.l2);
    let l2_fetched = l2_path.is_file();
    if l2_fetched {
        check_source(&allowlist, &l2_path)?;
    }
    check_source(&allowlist, &l1_path)?;
    check_source(&allowlist, &l4_path)?;
    check_source(&allowlist, &polyphone_path)?;
    if args.synth.is_none() {
        check_source(&allowlist, &input_path)?;
    }

    let started = Instant::now();
    let l1 = load_l1(&l1_path)?;
    let band = build::band_threshold(&l4_path, args.expand_top, args.expand_min_weight)?;
    let parsed = Instant::now();
    let (mut words, skips) = match args.synth {
        Some(count) => (synth_words(count, &l1, band.threshold), Skips::default()),
        None => load_words(&input_path, &l1, band.threshold)?,
    };
    let polyphone = load_polyphone(&polyphone_path)?;
    let (corrections, unmatched) = apply_polyphone(&words, &polyphone);

    // The two-character layer merge: the huayu entries win where both sources
    // hold a word, jieba fills the gaps, and the automatic polyphone check then
    // judges the readings the merge carried in against the list's own usage.
    let mut auto_counts = build::PolyphoneAutoCounts::default();
    if l2_fetched {
        let (huayu_words, huayu_skips) = load_words(&l2_path, &l1, band.threshold)?;
        println!(
            "dictc: L2 two-character layer from {}: {} rows parsed, {} skipped",
            l2_path.display(),
            huayu_words.len(),
            huayu_skips.total()
        );
        let merge_stats =
            build::merge_two_char_layer(&mut words, huayu_words, args.l2_min_freq);
        println!(
            "dictc: L2 two-character layer (floor {}): {} weights overridden, {} words added",
            args.l2_min_freq, merge_stats.overridden, merge_stats.added
        );
        let manual: BTreeSet<String> = corrections
            .iter()
            .map(|(word, _)| word.clone())
            .collect();
        auto_counts = build::apply_polyphone_rule(
            &mut words,
            &l1,
            &manual,
            build::PolyphoneRule {
                freq_ratio_pct: args.polyphone_freq_ratio,
            },
        );
        println!(
            "dictc: polyphone auto-check (floor {}%): {} readings kept, {} words flattened, {} manual rows, {} examined",
            args.polyphone_freq_ratio,
            auto_counts.assigned,
            auto_counts.flattened,
            auto_counts.manual,
            auto_counts.total
        );
    } else if !args.l2.as_os_str().is_empty() {
        println!(
            "dictc: L2 source {} is not fetched; building the un-merged word list",
            l2_path.display()
        );
    }
    // The raw weights drive the container's within-key order (the signal the
    // quality harness measures); the normalization rules re-band only the
    // unigram channel, where words of different lengths compete. `compile`
    // carries the per-band audit out in its stats.
    let mut compiled = compile(&words, band, args.expand_cap, &corrections, normalize_rules)?;
    compiled.stats.polyphone_unmatched = unmatched;
    let index = bigram::WordIndex::new(words.iter().map(|word| word.text.as_str()));
    let corpus = fs::read_to_string(&args.corpus).unwrap_or_default();
    let counts = bigram::count_cooccurrence(&corpus, &index);
    let records = bigram::build_records(
        counts,
        bigram::BigramConfig {
            max_pairs: args.max_bigram_pairs,
            min_score: args.min_bigram_score,
        },
    );
    compiled.bigram = records.into_iter().flat_map(BigramRecord::encode).collect();
    let built = Instant::now();

    // The section budgets are checked before anything is written: a container that is
    // over its ceiling is one to refuse, not one to emit and then complain about.
    let ledger = budget::enforce(&root, &compiled)?;

    let mut writer = DictWriter::new();
    writer.add_section(SectionKind::Fst, compiled.fst)?;
    writer.add_section(SectionKind::Entries, compiled.entries)?;
    writer.add_section(SectionKind::StrPool, compiled.strpool)?;
    writer.add_section(SectionKind::Unigram, compiled.unigram)?;
    writer.add_section(SectionKind::WordList, compiled.wordlist)?;
    writer.add_section(SectionKind::Bigram, compiled.bigram)?;
    let output_path = root.join(&args.output);
    writer.finish(&output_path)?;
    let written = Instant::now();

    // The manifest is written beside the container and describes what produced it: which
    // sources, at which layer, under which licence, and what the expansion band did. It
    // is the record that makes a compiled dictionary reproducible and auditable, so it is
    // written from the same values the container was built from rather than re-derived.
    let mut sources = vec![
        source_identity(&allowlist, &l1_path)?,
        source_identity(&allowlist, &l4_path)?,
        source_identity(&allowlist, &polyphone_path)?,
        source_identity(&allowlist, &input_path)?,
    ];
    if l2_fetched {
        sources.push(source_identity(&allowlist, &l2_path)?);
    }
    let facts = manifest::BuildFacts {
        sources,
        expansion: manifest::ExpansionRecord {
            band_weight: band.threshold,
            band_rank: band.requested,
            // The manifest carries the cap as the `u32` the container records it in; a
            // cap past that range is one no container could describe.
            cap: u32::try_from(args.expand_cap)
                .context("dictc: --expand-cap is past the range a container records")?,
            band_words: compiled.stats.band_words,
            expanded_words: compiled.stats.expanded_words,
            added_keys: compiled.stats.expanded_keys,
        },
        polyphone_auto: manifest::PolyphoneAutoRecord {
            assigned: auto_counts.assigned,
            flattened: auto_counts.flattened,
            manual: auto_counts.manual,
            total: auto_counts.total,
        },
        keys: compiled.stats.keys,
    };
    manifest::record_build(&root, &output_path, &ledger, facts)?;

    report(
        &output_path,
        &compiled.stats,
        &skips,
        band,
        args.expand_cap,
        normalize_rules,
        [
            parsed.duration_since(started),
            built.duration_since(parsed),
            written.duration_since(built),
            written.duration_since(started),
        ],
    )?;

    let growth = growth_percent(compiled.stats.keys, compiled.stats.baseline_keys());
    ensure!(
        growth <= args.max_key_growth,
        "dictc: key growth {growth:.1}% exceeds the {:.1}% ceiling; \
         lower --expand-top or --expand-cap",
        args.max_key_growth
    );
    Ok(())
}

/// Prints the build log: what went in, what came out, and what was dropped.
fn report(
    output: &Path,
    stats: &Stats,
    skips: &Skips,
    band: Band,
    cap: usize,
    normalize_rules: normalize::NormalizeRules,
    times: [Duration; 4],
) -> Result<()> {
    let size = fs::metadata(output)
        .with_context(|| format!("reading back {}", output.display()))?
        .len();
    let digest = Sha256::digest(fs::read(output)?);
    println!(
        "dictc: {} words, {} keys, {} (key growth {:+.1}%)",
        stats.words,
        stats.keys,
        human_size(size),
        growth_percent(stats.keys, stats.baseline_keys())
    );
    println!(
        "dictc: shape 1/2/3/4/5+ = {}/{}/{}/{}/{}",
        stats.shape[0], stats.shape[1], stats.shape[2], stats.shape[3], stats.shape[4]
    );
    println!(
        "dictc: L3b band weight >= {} (top {} of the frequency source): {} words in band, \
         cap {cap}, {} words expanded, {} keys generated",
        band.threshold, band.requested, stats.band_words, stats.expanded_words, stats.expanded_keys
    );
    println!(
        "dictc: L3c polyphone corrections: {} keys added or reweighted, {} rows unmatched, \
         {} (key, word) pairs dropped at the 32-word ceiling",
        stats.polyphone_keys, stats.polyphone_unmatched, stats.truncated_pairs
    );
    println!(
        "dictc: weight bands (unigram channel): {} char-freq, {} core-true-freq, {} tail-flat \
         (flat {}, core floor {})",
        stats.weight_bands.char_freq,
        stats.weight_bands.core_true_freq,
        stats.weight_bands.tail_flat,
        normalize_rules.tail_flat_weight,
        normalize_rules.core_min_freq
    );
    println!(
        "dictc: {} polyphone-risk words (a character of the word has several readings)",
        stats.polyphone_risk
    );
    // The read path resolves these by text; the count is the hash-health signal the
    // format layer documents, and the build has already refused to get here if it
    // passed the bound.
    println!(
        "dictc: {} hash collisions (the bound for {} words is {})",
        stats.hash_collisions,
        stats.words,
        collision_limit(stats.words as usize)
    );
    if !stats.expanded_examples.is_empty() {
        let sample: Vec<String> = stats
            .expanded_examples
            .iter()
            .map(|(word, keys)| format!("{word}={keys}"))
            .collect();
        println!("dictc: L3b sample, word=keys: {}", sample.join(" "));
    }
    if skips.total() > 0 {
        println!(
            "dictc: skipped {} rows (non-han {}, too-long {}, bad-weight {}, bad-flags {}, \
             bad-reading {}, unknown-char {}, duplicate {})",
            skips.total(),
            skips.non_han,
            skips.too_long,
            skips.bad_weight,
            skips.bad_flags,
            skips.bad_reading,
            skips.unknown_char,
            skips.duplicate
        );
        for example in &skips.examples {
            println!("dictc:   {example}");
        }
    }
    println!(
        "dictc: {} written, sha256 {}, parse {:.0}ms build {:.0}ms write {:.0}ms total {:.0}ms",
        output.display(),
        hex(&digest),
        times[0].as_secs_f64() * 1e3,
        times[1].as_secs_f64() * 1e3,
        times[2].as_secs_f64() * 1e3,
        times[3].as_secs_f64() * 1e3
    );
    Ok(())
}

/// Resolves the repository root.
fn resolve_root(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(root) = explicit {
        return Ok(root.to_path_buf());
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .context("xtask is expected to live in a subdirectory of the repository root")
}

/// Loads and schema-checks the source allowlist.
///
/// A source registered before its file is fetched carries an empty pin by design
/// (the header of `data/sources.toml` states the discipline): the pin is required
/// only once `data/raw/<id>.tsv` exists, which is what lets an allowlist entry
/// land ahead of its download without failing every build in between. A fetched
/// file with an empty or stale pin still refuses, which is the "unfinished entry
/// cannot pass" half of the same discipline.
///
/// # Errors
/// Returns an error when a registered source misses a required field, is not
/// marked permissive, or names a fetched upstream file whose pin is empty.
fn load_allowlist(path: &Path) -> Result<Allowlist> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let allowlist: Allowlist = toml::from_str(&text)
        .with_context(|| format!("{}: invalid allowlist document", path.display()))?;
    // `<root>/data/sources.toml` is the layout the allowlist documents; the raw
    // files it names live under `<root>/data/raw/`.
    let raw_dir = path
        .parent()
        .and_then(Path::parent)
        .map(|root| root.join("data").join("raw"));
    for source in &allowlist.source {
        ensure!(
            !source.id.is_empty()
                && !source.kind.is_empty()
                && !source.spdx.is_empty()
                && !source.license.is_empty()
                && !source.url.is_empty(),
            "{}: source `{}` is missing one of id/kind/spdx/license/url",
            path.display(),
            source.id
        );
        ensure!(
            source.permissive,
            "{}: source `{}` is not marked permissive; a copyleft source needs an ADR \
             before it may enter base.dict",
            path.display(),
            source.id
        );
        if source.kind == "upstream" {
            let fetched = raw_dir
                .as_ref()
                .is_some_and(|dir| dir.join(format!("{}.tsv", source.id)).exists());
            ensure!(
                !fetched || (!source.sha256.is_empty() && !source.retrieved.is_empty()),
                "{}: upstream source `{}` needs both `sha256` and `retrieved` before it \
                 can be compiled",
                path.display(),
                source.id
            );
        }
    }
    Ok(allowlist)
}

/// Enforces the allowlist for one input file.
///
/// The file's stem is its source id, which is what ties `data/raw/<id>.tsv` to the
/// registered entry. An unregistered file is refused outright, and the error names
/// the id so the caller can see which source is missing from the allowlist.
///
/// # Errors
/// Returns an error when the file's id is not registered, when the source is not
/// permissive, when an upstream source's file cannot be read, or when its SHA256 does
/// not match the pin. A file that is simply absent is reported as the stable
/// `dict/source/missing` diagnostic, because the raw sources are fetched rather than
/// committed and "not fetched yet" is the state a fresh checkout is in.
/// The manifest's identity for one source file, taken from the allowlist.
///
/// # Errors
///
/// Returns an error when the file is not a registered source: the manifest has to name
/// the id, the layer and the licence a compiled dictionary was built from, and a file
/// with none of those is one the build should not have read in the first place.
fn source_identity(allowlist: &Allowlist, path: &Path) -> Result<manifest::SourceIdentity> {
    let id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .with_context(|| format!("{}: unusable file name", path.display()))?;
    let source = allowlist
        .source
        .iter()
        .find(|source| source.id == id)
        .with_context(|| format!("{}: source `{id}` is not registered", path.display()))?;
    Ok(manifest::SourceIdentity {
        id: source.id.clone(),
        kind: source.kind.clone(),
        layer: source.layer.clone(),
        spdx: source.spdx.clone(),
        path: path.to_path_buf(),
    })
}

fn check_source(allowlist: &Allowlist, path: &Path) -> Result<()> {
    let id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .with_context(|| format!("{}: unusable file name", path.display()))?;
    let source = allowlist
        .source
        .iter()
        .find(|source| source.id == id)
        .with_context(|| {
            format!(
                "{}: source `{id}` is not registered in the allowlist; \
                 add it to data/sources.toml or remove the file",
                path.display()
            )
        })?;
    ensure!(
        source.permissive,
        "{}: source `{id}` is not permissive",
        path.display()
    );
    if source.kind != "upstream" {
        return Ok(());
    }
    let text = read_source(path)?;
    let digest = Sha256::digest(text.as_bytes());
    let actual = hex(&digest);
    ensure!(
        actual.eq_ignore_ascii_case(&source.sha256),
        "{}: source `{id}` sha256 mismatch: allowlist says {}, file hashes to {actual}",
        path.display(),
        source.sha256
    );
    Ok(())
}

/// Key growth in percent: how many keys the expansion added over the L3a baseline.
///
/// The denominator is the baseline *key* count, not the word count. Words collapse onto
/// shared keys — 5441 words can yield fewer distinct keys than there are words — so a
/// keys-per-word ratio mostly measures that collapse and can report zero growth while
/// the expansion is in fact adding hundreds of keys. The recorded figure for the full
/// dictionary (`+23%`, 337k → 414k) is a key-to-key comparison of the same shape.
fn growth_percent(keys: u64, baseline_keys: u64) -> f64 {
    if baseline_keys == 0 {
        return 0.0;
    }
    (keys.saturating_sub(baseline_keys)) as f64 * 100.0 / baseline_keys as f64
}

/// Renders a byte count for the build log.
fn human_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2}MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}KiB", bytes as f64 / 1024.0)
    }
}

/// Renders a digest as lower-case hexadecimal.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests;
