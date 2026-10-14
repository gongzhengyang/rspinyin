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

mod budget;
mod build;
mod manifest;
pub mod quality;
mod source;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use clap::Args;
use ime_dict::format::SectionKind;
use ime_dict::format::collision_limit;
use ime_dict::format::writer::DictWriter;
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
/// Default source allowlist.
pub const DEFAULT_SOURCES: &str = "data/sources.toml";

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
    /// Source allowlist.
    #[arg(long, default_value = DEFAULT_SOURCES)]
    sources: PathBuf,
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
    let allowlist = load_allowlist(&root.join(&args.sources))?;

    // The allowlist gate comes first: a source that is not registered, or whose
    // bytes do not match the pinned hash, must stop the build before any of it is
    // compiled, because a copyleft word list merged into base.dict cannot be
    // removed afterwards without rebuilding every weight and every rank.
    let l1_path = root.join(&args.l1);
    let l4_path = root.join(&args.l4);
    let polyphone_path = root.join(&args.polyphone);
    let input_path = root.join(&args.input);
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
    let (words, skips) = match args.synth {
        Some(count) => (synth_words(count, &l1, band.threshold), Skips::default()),
        None => load_words(&input_path, &l1, band.threshold)?,
    };
    let polyphone = load_polyphone(&polyphone_path)?;
    let (corrections, unmatched) = apply_polyphone(&words, &polyphone);
    let mut compiled = compile(&words, band, args.expand_cap, &corrections)?;
    compiled.stats.polyphone_unmatched = unmatched;
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
    let output_path = root.join(&args.output);
    writer.finish(&output_path)?;
    let written = Instant::now();

    // The manifest is written beside the container and describes what produced it: which
    // sources, at which layer, under which licence, and what the expansion band did. It
    // is the record that makes a compiled dictionary reproducible and auditable, so it is
    // written from the same values the container was built from rather than re-derived.
    let facts = manifest::BuildFacts {
        sources: vec![
            source_identity(&allowlist, &l1_path)?,
            source_identity(&allowlist, &l4_path)?,
            source_identity(&allowlist, &polyphone_path)?,
            source_identity(&allowlist, &input_path)?,
        ],
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
        keys: compiled.stats.keys,
    };
    manifest::record_build(&root, &output_path, &ledger, facts)?;

    report(
        &output_path,
        &compiled.stats,
        &skips,
        band,
        args.expand_cap,
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
fn load_allowlist(path: &Path) -> Result<Allowlist> {
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let allowlist: Allowlist = toml::from_str(&text)
        .with_context(|| format!("{}: invalid allowlist document", path.display()))?;
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
            ensure!(
                !source.sha256.is_empty() && !source.retrieved.is_empty(),
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
