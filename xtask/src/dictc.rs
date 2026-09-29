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

mod build;
mod source;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use clap::Args;
use ime_dict::format::SectionKind;
use ime_dict::format::writer::DictWriter;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::dictc::build::{Band, Stats, compile};
use crate::dictc::source::{
    Skips, apply_polyphone, load_l1, load_polyphone, load_words, synth_words,
};

/// Default word list, relative to the repository root.
pub const DEFAULT_INPUT: &str = "data/raw/base.tsv";
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
    /// The default is tuned for the development subset committed to this repository,
    /// not for the full dictionary. ADR-0000 measured the production expansion at +23%
    /// of keys using the top 50k words of a 400k-word dictionary, and that is the value
    /// the release build matrix passes. The committed subset holds 5441 words drawn from
    /// across the frequency range, so the same cut admits 43% of it and expands to
    /// +38%, over the growth ceiling. Cutting at weight 3000 instead lands the subset at
    /// +22.5%, which is what makes it a useful stand-in for the real thing.
    #[arg(long, default_value_t = 2_856)]
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

    let mut writer = DictWriter::new();
    writer.add_section(SectionKind::Fst, compiled.fst)?;
    writer.add_section(SectionKind::Entries, compiled.entries)?;
    writer.add_section(SectionKind::StrPool, compiled.strpool)?;
    writer.add_section(SectionKind::Unigram, compiled.unigram)?;
    writer.add_section(SectionKind::WordList, compiled.wordlist)?;
    let output_path = root.join(&args.output);
    writer.finish(&output_path)?;
    let written = Instant::now();

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
/// permissive, or when an upstream source's SHA256 does not match the file.
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
    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let digest = Sha256::digest(&bytes);
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
mod tests {
    use super::*;

    /// A scratch directory unique to this test process and tag.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rspinyin-dictc-{tag}-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        dir
    }

    /// One allowlist entry.
    fn source(id: &str, kind: &str, sha256: &str) -> Source {
        Source {
            id: id.to_owned(),
            kind: kind.to_owned(),
            spdx: "MIT".to_owned(),
            license: "MIT".to_owned(),
            url: "https://example.invalid".to_owned(),
            retrieved: "2026-09-29".to_owned(),
            sha256: sha256.to_owned(),
            permissive: true,
        }
    }

    #[test]
    fn test_check_source_refuses_unregistered_and_mismatched_sources() {
        let dir = scratch("gate");
        let registered = dir.join("base.tsv");
        fs::write(&registered, "中国\n").expect("writing the fixture");
        let unregistered = dir.join("sogou.tsv");
        fs::write(&unregistered, "中国\n").expect("writing the fixture");

        let allowlist = Allowlist {
            source: vec![
                source("base", "derived", ""),
                source("pinyin-data", "upstream", &"0".repeat(64)),
            ],
        };

        check_source(&allowlist, &registered).expect("a derived source needs no hash");
        let refusal = check_source(&allowlist, &unregistered).expect_err("unregistered source");
        assert!(
            refusal.to_string().contains("sogou"),
            "the refusal names the source: {refusal}"
        );
        assert!(
            check_source(&allowlist, &dir.join("pinyin-data.tsv")).is_err(),
            "a missing pinned file must fail"
        );
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_check_source_refuses_a_hash_that_does_not_match() {
        let dir = scratch("hash");
        let path = dir.join("pinyin-data.tsv");
        fs::write(&path, "中\tzhong\n").expect("writing the fixture");
        let digest = hex(&Sha256::digest("中\tzhong\n".as_bytes()));
        let matching = Allowlist {
            source: vec![source("pinyin-data", "upstream", &digest)],
        };
        check_source(&matching, &path).expect("the pinned hash matches");

        let wrong = Allowlist {
            source: vec![source("pinyin-data", "upstream", &"a".repeat(64))],
        };
        let failure = check_source(&wrong, &path).expect_err("a wrong hash must be refused");
        assert!(failure.to_string().contains("sha256 mismatch"), "{failure}");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_load_allowlist_refuses_a_copyleft_source() {
        let dir = scratch("allow");
        let path = dir.join("sources.toml");
        fs::write(
            &path,
            "[[source]]\nid = \"luna\"\nkind = \"upstream\"\nspdx = \"LGPL-3.0\"\n\
             license = \"LGPL-3.0\"\nurl = \"https://example.invalid\"\n\
             retrieved = \"2026-09-29\"\nsha256 = \"abc\"\npermissive = false\n",
        )
        .expect("writing the fixture");
        let failure = load_allowlist(&path).expect_err("a copyleft source must be refused");
        assert!(failure.to_string().contains("luna"), "{failure}");

        fs::write(
            &path,
            "[[source]]\nid = \"base\"\nkind = \"derived\"\nspdx = \"MIT OR Apache-2.0\"\n\
             license = \"Project-owned\"\nurl = \"https://example.invalid\"\npermissive = true\n",
        )
        .expect("writing the fixture");
        let allowlist = load_allowlist(&path).expect("a derived source needs no pin");
        assert_eq!(allowlist.source.len(), 1);
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_load_allowlist_requires_a_pin_for_an_upstream_source() {
        let dir = scratch("pin");
        let path = dir.join("sources.toml");
        fs::write(
            &path,
            "[[source]]\nid = \"jieba-dict\"\nkind = \"upstream\"\nspdx = \"MIT\"\n\
             license = \"MIT\"\nurl = \"https://example.invalid\"\npermissive = true\n",
        )
        .expect("writing the fixture");
        let failure = load_allowlist(&path).expect_err("an unpinned upstream source must fail");
        assert!(failure.to_string().contains("sha256"), "{failure}");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_growth_percent_and_the_guard_boundary() {
        assert_eq!(growth_percent(0, 0), 0.0);
        assert_eq!(growth_percent(100, 100), 0.0);
        assert_eq!(growth_percent(115, 100), 15.0);
        assert_eq!(growth_percent(130, 100), 30.0);
        assert!(growth_percent(131, 100) > 30.0);
    }

    #[test]
    fn test_hex_and_human_size_render_for_the_log() {
        assert_eq!(hex(&[0x00, 0x0F, 0xFF]), "000fff");
        assert_eq!(human_size(512), "0.5KiB");
        assert_eq!(human_size(2 * 1024 * 1024), "2.00MiB");
    }
}
