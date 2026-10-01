//! The quality closure of the compiler: how well a compiled dictionary ranks a
//! held-out set, and whether the L3c correction table is the right size to move that
//! number.
//!
//! Responsibility: measure one compiled container against a held-out set of typed
//! pinyin, and audit the correction table the compiler consumes. The two halves sit in
//! one module because they answer one question: the held-out set says how often the
//! expected word is ranked first, and the audit says whether the table that is supposed
//! to move that number is worth measuring at all -- large enough to be visible, and small
//! enough to still be a correction table. `ASM-A-21` fixes the measurement -- a set of at
//! least five thousand rows, and two rates, the first choice and the expected word inside
//! the first nine candidates. The two-hundred-row golden set cannot resolve a difference
//! of a couple of percent, so it is deliberately not what this module reads.
//!
//! Boundaries: this layer reads three files and writes none. It never compiles a
//! container (that is [`super::build`]) and never parses a word list (that is
//! [`super::source`]). It queries the artifact through the same [`Lexicon`] contract the
//! engine queries, so a number it reports is one the input method can reproduce.
//!
//! # What the rates measure, and what they do not
//!
//! The candidate order is the container's own. `dictc` writes each key's words in
//! descending weight and the read path never sorts, so ranking a key is a read of the
//! compiled ranking rather than a second scoring pass. That is deliberate: the question
//! here is whether the *dictionary* puts the right word first, while the scorer's length
//! bonus and segment penalty -- which `xtask tune` searches -- answer whether the
//! *decoder* does. Measuring the container alone is what keeps a regression traceable to
//! the word list, to the L3b expansion or to the L3c table rather than to the weights.
//!
//! A word the key does not serve at all is counted on its own, so a word the dictionary
//! is missing is never averaged into the same number as a word it ranks badly. The two
//! rates alone would do exactly that, since an unreachable case is a first-choice miss by
//! construction, so the first-choice rate is reported a second time over the cases the key
//! does serve. The pair is what tells a reader whether a dictionary is thin or badly
//! ordered: the rate over every case moves when either one does, while the rate over the
//! served cases moves only with the ordering. A word ranked tenth is neither missing nor
//! served: it is not first, it is not inside the page, and it is not absent.
//!
//! # Why the correction table is bounded twice
//!
//! The L3c table is audited against a floor and a ceiling, and both are failures rather
//! than warnings. A table under the floor cannot move a rate the held-out set resolves, so
//! a run that presents one as evidence of anything is presenting noise. A table over the
//! ceiling is no longer the exceptions to the word list but a second word list, and one
//! the compiler carries into every container it builds. The ceiling is what keeps the
//! table's growth a decision rather than an accident.
//!
//! # Reproducibility
//!
//! Nothing here samples, times or hashes into an unordered map, and every comparison
//! ends on the text. The same held-out set, the same container and the same correction
//! table produce the same rates on every machine, which is what lets one run record a
//! baseline and a later run claim an improvement over it.

mod tables;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use clap::Args;
use ime_core::segment::{HINT_INLINE_BOUNDARIES, SyllableDag};
use ime_dict::fst_index::FstLexicon;
use ime_types::Lexicon;
use smallvec::SmallVec;

use crate::tune::{DEFAULT_HOLDOUT, REPORT_CANDIDATES};

use self::tables::{
    data_lines, parse_layer, parse_origin, parse_weight, read_table, unsegmentable,
};

use super::{DEFAULT_OUTPUT, DEFAULT_POLYPHONE};

/// Row floor the L3c correction table has to reach before it is worth measuring.
///
/// It is a default rather than a property of the format so that the floor can be raised
/// without a code change once the table clears it. The floor is a resolvability bound,
/// not a target: over the 6,000-case held-out set a miss rate of one in four carries a
/// binomial noise of about 0.6 points, so a table has to be able to move several times
/// that before the movement is distinguishable from the set's own noise. Corrections are
/// exceptions for words people actually type, so only a fraction of a table's rows meet
/// a held-out case at all; even under that discount, 2,500 rows can move the rate by
/// roughly six points -- an order of magnitude above the noise -- while a table of a few
/// hundred rows could not move it measurably. The shipped table sits just above this
/// floor by design: the v1.2 re-design made `L3b` expansion the primary wrong-reading
/// mechanism and this table the exceptions the expansion cannot reach, so the table
/// stays small on purpose. A run that finds one below the floor refuses to present the
/// rates as evidence of anything.
pub const MIN_CORRECTIONS: usize = 2_500;

/// Row ceiling the L3c correction table may not pass.
///
/// A correction table is the exceptions to the word list: it exists to override the
/// readings the L1 table would otherwise give, and it carries every row into the container
/// it is compiled with. A table that grows past a small multiple of the floor has stopped
/// being a list of exceptions -- at that point the reading it states for a word is the
/// ordinary case and the L1 table is the exception, which is a word list's job and not
/// this table's. The ceiling is a default for the same reason the floor is, and it is what
/// makes the table's size a decision: a table can only pass it deliberately.
pub const MAX_CORRECTIONS: usize = 20_000;

/// Case floor the held-out set has to reach before a rate over it resolves anything.
///
/// `ASM-A-21` states the floor: the two-hundred-row golden set cannot resolve the
/// difference between two dictionaries, and a set of at least five thousand rows can.
/// Like the correction floor this is a default rather than a property of the format, so
/// a narrower set can still be measured deliberately with `--min-cases`, but never by
/// accident.
pub const MIN_HOLDOUT_CASES: usize = 5_000;

/// Longest correction row the table format defines, in columns.
const MAX_CORRECTION_COLUMNS: usize = 4;

/// Which layer of the reading pipeline supplied the reading a case is attributed to.
///
/// The held-out set is stratified by this column and the report is broken down by it,
/// because the three layers fail for different reasons: an `L1` case that misses is a
/// ranking problem in the word list, an `L3b` case that misses is a key the expansion
/// never generated, and an `L3c` case that misses is a correction the table does not
/// state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadingSource {
    /// The L1 single-character table: the L3a baseline reading of every character.
    L1,
    /// The L3b frequency-targeted multi-key expansion.
    L3b,
    /// The L3c weight-correction table.
    L3c,
}

impl ReadingSource {
    /// Every layer, in the order the report lists them.
    pub const ALL: [ReadingSource; 3] = [Self::L1, Self::L3b, Self::L3c];

    /// Parses the `source` column of a held-out row.
    ///
    /// The spelling is the layer name as the architecture document writes it, and it is
    /// matched exactly: a row that says `l3b` is refused rather than folded, because the
    /// column is written by a generator and a near miss there is a defect in the set
    /// rather than a preference of the reader.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "L1" => Some(Self::L1),
            "L3b" => Some(Self::L3b),
            "L3c" => Some(Self::L3c),
            _ => None,
        }
    }

    /// The spelling the `source` column uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::L1 => "L1",
            Self::L3b => "L3b",
            Self::L3c => "L3c",
        }
    }
}

/// One row of the held-out set, resolved to the key the container is queried with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldoutCase {
    /// The `'`-separated key the row's pinyin column segments into.
    pub key: String,
    /// The word that has to rank first for the row to count as a first-choice hit.
    pub expected: String,
    /// The layer the row is attributed to, for the per-layer breakdown.
    pub source: ReadingSource,
}

/// Quality of one decode over the whole hold-out set.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QualityReport {
    /// Fraction of cases whose first candidate is the expected word.
    pub top1_rate: f64,
    /// Fraction of cases where the expected word appears in the first nine
    /// candidates. This is the reachability half: L3b fixes this one.
    pub top9_rate: f64,
    /// Cases the decoder could not spell at all, reported separately so a
    /// missing word is never confused with a badly ranked one.
    pub unreachable: u32,
    /// Fraction of the cases the key serves whose first candidate is the expected word.
    ///
    /// [`QualityReport::top1_rate`] divides by every case, so a word the dictionary does
    /// not hold counts against it exactly like a word it ranks second. This rate divides
    /// by the cases the key serves, which is what isolates the ordering from the coverage:
    /// a dictionary that gains a word moves this number while the other stays where it is,
    /// and a dictionary that reorders its candidates moves both.
    pub served_top1_rate: f64,
}

/// What one pass counted, before the rates were divided out.
///
/// The counts are kept alongside the rates rather than only as rates so that a per-layer
/// line can state how many cases it stands on. A rate over a handful of cases and a rate
/// over five thousand print the same way; the count is what tells them apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Tally {
    /// Cases the pass looked at.
    cases: u64,
    /// Cases whose expected word came first.
    first: u64,
    /// Cases whose expected word was inside the reported page.
    within: u64,
    /// Cases whose expected word the key does not serve at all.
    missing: u64,
}

impl Tally {
    /// The rates the counts imply; an empty pass reports zero rather than dividing by
    /// zero.
    fn report(self) -> QualityReport {
        if self.cases == 0 {
            return QualityReport::default();
        }
        let total = self.cases as f64;
        QualityReport {
            top1_rate: self.first as f64 / total,
            top9_rate: self.within as f64 / total,
            unreachable: u32::try_from(self.missing).unwrap_or(u32::MAX),
            served_top1_rate: self.served_first(),
        }
    }

    /// Cases the key serves at all, which is the subset an ordering can be read from.
    fn served(self) -> u64 {
        self.cases.saturating_sub(self.missing)
    }

    /// Fraction of the cases the key serves whose expected word came first; a pass that
    /// served nothing answers zero rather than dividing by zero.
    fn served_first(self) -> f64 {
        let served = self.served();
        if served == 0 {
            return 0.0;
        }
        self.first as f64 / served as f64
    }
}

/// Reads a held-out set: `pinyin<TAB>expected[<TAB>source]` rows.
///
/// The pinyin column is what a user types (`nihao`), and it is segmented with the same
/// syllable graph the engine builds a keystroke from, so a case is looked up under the
/// key the input method would query. A column that already carries `'` is accepted
/// unchanged, which is the form the tuner's holdout generator writes.
///
/// The `source` column is optional and defaults to [`ReadingSource::L1`]: a row that
/// states no layer states the baseline, which is the layer every word has. That is what
/// lets a two-column set written before the column existed still be measured, and it is
/// why the column is a report of what the row measures rather than a requirement on the
/// file.
///
/// # Errors
///
/// Returns an error when the file cannot be read, when it holds no rows at all, when a
/// row is missing its pinyin or its expected-word column, when the `source` column is
/// not one of `L1` / `L3b` / `L3c`, or when the pinyin column does not segment into
/// syllables. A row that cannot be turned into a key is refused rather than skipped: it
/// would otherwise leave the measurement silently, and a rate computed over a set whose
/// holes nobody can see is a rate that cannot be compared with anything.
pub fn load_holdout(path: &Path) -> Result<Vec<HoldoutCase>> {
    let text = read_table(path)?;
    let mut cases = Vec::new();
    for (number, line) in data_lines(&text) {
        let mut columns = line.split('\t').map(str::trim);
        let typed = columns.next().unwrap_or_default();
        let expected = columns.next().unwrap_or_default();
        ensure!(
            !typed.is_empty() && !expected.is_empty(),
            "{}:{number}: a row needs a pinyin column and an expected-word column",
            path.display()
        );
        let source = match columns.next() {
            Some(raw) if !raw.is_empty() => parse_layer(path, number, raw)?,
            _ => ReadingSource::L1,
        };
        let key = key_of(typed).with_context(|| unsegmentable(path, number))?;
        cases.push(HoldoutCase {
            key,
            expected: expected.to_owned(),
            source,
        });
    }
    ensure!(
        !cases.is_empty(),
        "{}: the held-out set holds no rows",
        path.display()
    );
    Ok(cases)
}

/// Segments a typed pinyin column into the canonical `'`-separated key.
///
/// The cut is the one the engine's preedit shows before any scoring has run: the fewest
/// syllables, and among equally short cuts the longest syllable first. Using the engine's
/// own graph rather than a second segmenter is what makes the key a case is measured
/// under the key a user's keystrokes reach, so `nihao` cuts as `ni'hao` while a column
/// that already carries `'` keeps the boundaries it states.
///
/// Returns `None` when the input has no legal segmentation at all, which is the caller's
/// signal that the row cannot be measured.
///
/// The holdout generator calls this too, to turn a correction row's reading into the key
/// that row would serve: a second segmentation of the same spelling would be a second
/// answer to the question of what key a reading denotes.
pub(crate) fn key_of(typed: &str) -> Option<String> {
    let mut graph = SyllableDag::new();
    graph.build(typed).ok()?;
    let mut boundaries = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
    if !graph.best_segmentation_hint(&mut boundaries) {
        return None;
    }
    let mut syllables = Vec::with_capacity(boundaries.len().saturating_sub(1));
    for pair in boundaries.windows(2) {
        syllables.push(syllable_text(&graph, pair[0], pair[1])?.to_owned());
    }
    (!syllables.is_empty()).then(|| syllables.join("'"))
}

/// The text of the syllable the edge from node `start` to node `end` covers.
///
/// The boundaries come from [`SyllableDag::best_segmentation_hint`], so the edge exists
/// by construction; the lookup goes through [`SyllableDag::syllable_text`] rather than
/// slicing the normalized string because a boundary the user pinned with `'` sits inside
/// the edge's byte range, and a plain slice would carry the marker into the key.
fn syllable_text(graph: &SyllableDag, start: u16, end: u16) -> Option<&str> {
    let edges = graph.edges_from(usize::from(start));
    let edge = edges.iter().find(|edge| edge.end == end)?;
    Some(graph.syllable_text(start, *edge))
}

/// Measures one held-out set against one lexicon.
///
/// The lexicon is queried through the frozen [`Lexicon`] contract, so the same
/// measurement runs against a compiled container, against the in-memory stand-in the
/// tuner ranks with, or against a test double, and a number produced by one is
/// comparable with a number produced by another.
///
/// # Errors
///
/// Returns an error when a lookup fails, which for a compiled container means the
/// container is unreadable or corrupt. A key the lexicon serves no words for is not an
/// error: it is what [`QualityReport::unreachable`] counts.
pub fn measure<L: Lexicon>(cases: &[HoldoutCase], lexicon: &L) -> Result<QualityReport> {
    Ok(tally(cases, lexicon, None)?.report())
}

/// Counts one pass over `cases`, optionally restricted to one reading layer.
///
/// The filter is applied here rather than by the caller so that the per-layer breakdown
/// reads the same cases as the whole-set pass without copying them.
fn tally<L: Lexicon>(
    cases: &[HoldoutCase],
    lexicon: &L,
    only: Option<ReadingSource>,
) -> Result<Tally> {
    let mut counted = Tally::default();
    for case in cases {
        if only.is_some_and(|source| source != case.source) {
            continue;
        }
        counted.cases += 1;
        match rank_of(lexicon, &case.key, &case.expected)? {
            None => counted.missing += 1,
            Some(0) => {
                counted.first += 1;
                counted.within += 1;
            }
            Some(rank) if rank < REPORT_CANDIDATES => counted.within += 1,
            Some(_) => {}
        }
    }
    Ok(counted)
}

/// Position of `expected` in the words `key` serves, or `None` when the key does not
/// serve the word at all.
///
/// The first match wins. A word appears once per key, so this is its rank, and stopping
/// at the match keeps a key with a long word list from being walked to its end for an
/// answer already known.
fn rank_of<L: Lexicon>(lexicon: &L, key: &str, expected: &str) -> Result<Option<usize>> {
    for (rank, word) in lexicon.lookup(key)?.enumerate() {
        if word.text == expected {
            return Ok(Some(rank));
        }
    }
    Ok(None)
}

/// Where a correction row's reading came from.
///
/// The column exists so that a reader of the table can tell a reading derived from a
/// permissively licensed source from one this project wrote, without consulting the
/// allowlist: `unihan` rows carry the Unicode licence and no further obligation,
/// `manual` rows are project-owned, and `corpus` rows are counted from the project's own
/// word list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Derived from `data/raw/unihan.tsv`.
    Unihan,
    /// Written by hand for this project.
    Manual,
    /// Counted from the project's own word list.
    Corpus,
}

impl Origin {
    /// Parses the `origin` column.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "unihan" => Some(Self::Unihan),
            "manual" => Some(Self::Manual),
            "corpus" => Some(Self::Corpus),
            _ => None,
        }
    }
}

/// One row of the L3c correction table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Correction {
    /// The word the row corrects.
    pub word: String,
    /// The reading the row states, spelled as the file spells it.
    pub reading: String,
    /// The weight column; absent in the two-column form.
    pub weight: Option<u32>,
    /// The origin column; absent in the two- and three-column forms.
    pub origin: Option<Origin>,
}

/// The origin mix of one correction table.
///
/// A row with no `origin` column is counted separately rather than folded onto one of the
/// three values: "this row states no origin" and "this row states that it came from
/// Unihan" are different claims, and only one of them is a licence statement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OriginMix {
    /// Rows carrying no `origin` column.
    pub unspecified: u32,
    /// Rows marked `unihan`.
    pub unihan: u32,
    /// Rows marked `manual`.
    pub manual: u32,
    /// Rows marked `corpus`.
    pub corpus: u32,
}

impl OriginMix {
    /// Tallies one table.
    pub fn of(rows: &[Correction]) -> Self {
        let mut mix = Self::default();
        for row in rows {
            mix.record(row.origin);
        }
        mix
    }

    /// Records one row's origin.
    pub fn record(&mut self, origin: Option<Origin>) {
        let slot = match origin {
            None => &mut self.unspecified,
            Some(Origin::Unihan) => &mut self.unihan,
            Some(Origin::Manual) => &mut self.manual,
            Some(Origin::Corpus) => &mut self.corpus,
        };
        *slot = slot.saturating_add(1);
    }

    /// Rows the mix accounts for.
    pub fn total(&self) -> u32 {
        self.unspecified
            .saturating_add(self.unihan)
            .saturating_add(self.manual)
            .saturating_add(self.corpus)
    }

    /// Rows of one origin.
    pub fn count(&self, origin: Origin) -> u32 {
        match origin {
            Origin::Unihan => self.unihan,
            Origin::Manual => self.manual,
            Origin::Corpus => self.corpus,
        }
    }
}

/// Reads the L3c correction table.
///
/// Three column counts are accepted, and they are the three the format has had:
/// `word<TAB>reading` (the original), `word<TAB>reading<TAB>weight`, and
/// `word<TAB>reading<TAB>weight<TAB>origin`. A row reads the same way whatever its column
/// count, so a table that gains the `origin` column keeps compiling exactly as it did
/// before, which is what lets the column be added without a flag day. A row past the
/// fourth column is refused rather than ignored: an unread column is a column whose
/// content nobody validated, and an origin nobody validated is worse than none.
///
/// The reading itself is not re-parsed here. The compiler's own reader asserts that every
/// row is a legal syllable sequence for its word before the table can affect a build, and
/// a second implementation of that check would be a second answer to the same question.
///
/// # Errors
///
/// Returns an error when the file cannot be read or has not been fetched, when a row
/// holds fewer than two or more than [`MAX_CORRECTION_COLUMNS`] columns, when its word or
/// reading column is empty, when the weight column is not an unsigned integer, or when
/// the origin column is not one of `unihan` / `manual` / `corpus`.
pub fn load_corrections(path: &Path) -> Result<Vec<Correction>> {
    let text = super::source::read_source(path)?;
    let mut rows = Vec::new();
    for (number, line) in data_lines(&text) {
        let columns: Vec<&str> = line.split('\t').map(str::trim).collect();
        ensure!(
            (2..=MAX_CORRECTION_COLUMNS).contains(&columns.len()),
            "{}:{number}: a correction row holds 2 to {MAX_CORRECTION_COLUMNS} columns, \
             found {}",
            path.display(),
            columns.len()
        );
        let word = columns.first().copied().unwrap_or_default();
        let reading = columns.get(1).copied().unwrap_or_default();
        ensure!(
            !word.is_empty() && !reading.is_empty(),
            "{}:{number}: a correction row needs a word column and a reading column",
            path.display()
        );
        let weight = match columns.get(2).copied() {
            Some(raw) if !raw.is_empty() => parse_weight(path, number, raw)?,
            _ => None,
        };
        let origin = match columns.get(3).copied() {
            Some(raw) if !raw.is_empty() => Some(parse_origin(path, number, raw)?),
            _ => None,
        };
        rows.push(Correction {
            word: word.to_owned(),
            reading: reading.to_owned(),
            weight,
            origin,
        });
    }
    Ok(rows)
}

/// Command-line surface of the quality closure.
#[derive(Debug, Args)]
pub struct QualityArgs {
    /// Repository root; defaults to the directory containing the xtask crate.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Compiled container to measure.
    #[arg(long, default_value = DEFAULT_OUTPUT)]
    dict: PathBuf,
    /// Held-out set, in `pinyin<TAB>expected[<TAB>source]` columns.
    #[arg(long, default_value = DEFAULT_HOLDOUT)]
    holdout: PathBuf,
    /// L3c correction table to audit.
    #[arg(long, default_value = DEFAULT_POLYPHONE)]
    polyphone: PathBuf,
    /// Row floor the correction table has to reach.
    #[arg(long, default_value_t = MIN_CORRECTIONS)]
    min_corrections: usize,
    /// Row ceiling the correction table may not pass.
    #[arg(long, default_value_t = MAX_CORRECTIONS)]
    max_corrections: usize,
    /// Case floor the held-out set has to reach.
    #[arg(long, default_value_t = MIN_HOLDOUT_CASES)]
    min_cases: usize,
}

/// The two bounds one correction table is audited against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bounds {
    /// Rows below this many fail: the table cannot move a rate the set resolves.
    floor: usize,
    /// Rows above this many fail: the table has become a second word list.
    ceiling: usize,
}

/// Entry point for the quality closure.
///
/// Prints the two rates over the whole held-out set, the same rates per reading layer, and
/// the correction table's origin mix -- counts and rates only, never a row's text. The
/// held-out set is a fixture rather than user input, but a build log is copied into
/// bug reports, and a rule that keeps words out of it is only as good as its exceptions.
///
/// # Errors
///
/// Returns an error when the held-out set, the container or the correction table cannot
/// be read, when a row of either table is malformed, when the held-out set is below
/// `--min-cases`, when the correction table is outside `--min-corrections` and
/// `--max-corrections`, or when a row of the table carries no origin. The three bounds are
/// failures rather than warnings: a rate over too few cases, a table too small to move it
/// and a table too large to be a correction table all look exactly like a result, and none
/// of them is one.
pub fn run(args: QualityArgs) -> Result<()> {
    ensure!(
        args.min_corrections <= args.max_corrections,
        "quality: --min-corrections {} is past --max-corrections {}, so no table could pass",
        args.min_corrections,
        args.max_corrections
    );
    let root = super::resolve_root(args.root.as_deref())?;
    let holdout = root.join(&args.holdout);
    let cases = load_holdout(&holdout)?;
    require_cases(&holdout, cases.len(), args.min_cases)?;
    let dict = root.join(&args.dict);
    let lexicon = FstLexicon::load(&dict).with_context(|| container_missing(&dict))?;
    println!(
        "quality: {} entries, {} cases from {}",
        lexicon.entry_count(),
        cases.len(),
        holdout.display()
    );
    report("all", &measure(&cases, &lexicon)?, cases.len());
    for source in ReadingSource::ALL {
        let counted = tally(&cases, &lexicon, Some(source))?;
        if counted.cases > 0 {
            report(
                source.as_str(),
                &counted.report(),
                usize::try_from(counted.cases).unwrap_or(usize::MAX),
            );
        }
    }
    let polyphone = root.join(&args.polyphone);
    let rows = load_corrections(&polyphone)?;
    report_coverage(&rows, &lexicon)?;
    audit(
        &polyphone,
        &OriginMix::of(&rows),
        count_repeats(&rows),
        Bounds {
            floor: args.min_corrections,
            ceiling: args.max_corrections,
        },
    )
}

/// Prints one line of the measurement.
fn report(label: &str, rates: &QualityReport, cases: usize) {
    let top1 = rates.top1_rate * 100.0;
    let top9 = rates.top9_rate * 100.0;
    println!("quality: {label}: {cases} cases, top1 {top1:.1}%, top9 {top9:.1}%");
    println!("quality: {label}: unreachable {}", rates.unreachable);
    let served = cases.saturating_sub(usize::try_from(rates.unreachable).unwrap_or(usize::MAX));
    let served_first = rates.served_top1_rate * 100.0;
    println!(
        "quality: {label}: first choice {served_first:.1}% of the {served} cases the key serves"
    );
}

/// Refuses a held-out set too small for a rate over it to resolve anything.
///
/// A rate over a handful of cases and a rate over five thousand print the same way, and
/// only one of them can support a claim that one dictionary ranks better than another.
fn require_cases(path: &Path, cases: usize, floor: usize) -> Result<()> {
    ensure!(
        cases >= floor,
        "{}: the held-out set holds {cases} cases, {floor} are required before a rate \
         over it resolves a difference between two dictionaries",
        path.display()
    );
    Ok(())
}

/// Prints how much of the correction table the compiled container actually carries.
///
/// A row counts when the container serves its word under the reading the row states,
/// because that is the key the compiler adds or reweights. The gap between the row count
/// and this one is the difference between a table that is large and a table that is
/// effective, which is the whole reason the table is being grown.
///
/// The reading is segmented with the same graph a typed keystroke goes through, so a row
/// that spells its reading as one run of letters counts the same way a separated one
/// does. A reading with no legal segmentation is a row this build cannot place.
fn report_coverage(rows: &[Correction], lexicon: &FstLexicon) -> Result<()> {
    let mut served_rows = 0u32;
    let mut weighted = 0u32;
    for row in rows {
        weighted += u32::from(row.weight.is_some());
        let Some(key) = key_of(&row.reading) else {
            continue;
        };
        served_rows += u32::from(serves(lexicon, &key, &row.word)?);
    }
    println!("quality: L3c {served_rows} rows the container serves");
    println!("quality: L3c {weighted} weighted rows, unread by the build");
    Ok(())
}

/// Whether `lexicon` serves `word` under `key`.
///
/// # Errors
///
/// Returns an error when the lookup fails, which for a compiled container means the
/// container is unreadable or corrupt.
fn serves(lexicon: &FstLexicon, key: &str, word: &str) -> Result<bool> {
    Ok(lexicon.lookup(key)?.any(|found| found.text == word))
}

/// Counts rows that repeat a `(word, reading)` pair an earlier row already states.
///
/// The pair is compared as the file spells it, because this module deliberately does not
/// re-parse a reading: a repeat that spells one reading two ways is a different defect
/// from a row that says the same thing twice, and telling them apart needs the compiler's
/// syllable reader rather than a second one written here. A word stated twice with two
/// different readings is not a repeat -- that is exactly what the table is for.
fn count_repeats(rows: &[Correction]) -> u32 {
    let mut seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut repeats = 0u32;
    for row in rows {
        if !seen.insert((row.word.as_str(), row.reading.as_str())) {
            repeats = repeats.saturating_add(1);
        }
    }
    repeats
}

/// Prints the correction table's origin mix and refuses a table that cannot be used as
/// evidence.
///
/// Four conditions fail the run, and they are reported separately because they need
/// different work: a table under the floor needs more rows, a table over the ceiling needs
/// a smaller one, a table with unannotated rows needs the origin column filled in on rows
/// that already exist, and a table with repeats needs them removed before its row count
/// means anything.
fn audit(path: &Path, mix: &OriginMix, repeats: u32, bounds: Bounds) -> Result<()> {
    let unihan = mix.count(Origin::Unihan);
    let manual = mix.count(Origin::Manual);
    let corpus = mix.count(Origin::Corpus);
    println!("quality: L3c {} rows", mix.total());
    println!("quality:   unihan {unihan}, manual {manual}, corpus {corpus}");
    println!("quality:   unspecified {}", mix.unspecified);
    println!("quality:   {repeats} rows repeat a (word, reading) pair");
    let floor = u32::try_from(bounds.floor).context("the correction floor is past a u32")?;
    let ceiling = u32::try_from(bounds.ceiling).context("the correction ceiling is past a u32")?;
    ensure!(
        mix.total() >= floor,
        "{}: the L3c correction table holds {} rows, {floor} are required before its effect \
         on the held-out set is larger than the set's own noise",
        path.display(),
        mix.total()
    );
    ensure!(
        mix.total() <= ceiling,
        "{}: the L3c correction table holds {} rows, past the {ceiling}-row ceiling; a \
         correction table states the exceptions to the word list, and one this large has \
         become a second word list",
        path.display(),
        mix.total()
    );
    ensure!(
        mix.unspecified == 0,
        "{}: {} rows carry no `origin` column; every row needs one of unihan, manual, corpus",
        path.display(),
        mix.unspecified
    );
    ensure!(
        repeats == 0,
        "{}: {repeats} rows repeat a (word, reading) pair an earlier row already states; a \
         repeat inflates the row count without adding a reading, which is what the floor \
         above is read from. Find them with `cut -f1,2 {} | sort | uniq -d`",
        path.display(),
        path.display()
    );
    Ok(())
}

/// The diagnostic for a container that has not been built yet.
fn container_missing(path: &Path) -> String {
    format!(
        "{}: cannot open the compiled dictionary; build it with `xtask dictc`",
        path.display()
    )
}
