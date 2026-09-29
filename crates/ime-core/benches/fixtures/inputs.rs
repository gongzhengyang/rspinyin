//! The inputs the decode benchmark cases run on, and the dictionary keys they spell.
//!
//! Responsibility: hold the case table -- the five inputs whose cost the decode
//! budget is stated for -- and turn each of them into the `'`-separated keys a
//! decoder looks up while decoding it, plus the pinyin keys of the held-out
//! evaluation set, so that the generated corpus carries a word wherever a decode
//! reaches.
//!
//! Boundaries: this module knows nothing about criterion and nothing about how the
//! corpus is generated. It reads the case table and the held-out fixture and hands
//! out strings.

use ime_core::segment::{HINT_INLINE_BOUNDARIES, MAX_RAW_LEN, SyllableDag};
use smallvec::SmallVec;

/// Longest window of syllables the corpus guarantees a word for.
///
/// Four covers the words a decoder actually finds -- the two- and three-syllable
/// words and the four-character idioms -- without making the corpus carry a word
/// for every span nobody ever looks up.
pub const MAX_SEED_SYLLABLES: usize = 4;

/// The held-out evaluation set, embedded so that the benchmark reads no file.
///
/// The set is the one the language-model work maintains under `tests/fixtures/`;
/// the benchmark reuses it rather than inventing a second corpus, and the path is
/// resolved against this file so it does not depend on the working directory.
const HOLDOUT: &str = include_str!("../../tests/fixtures/lm_holdout.tsv");

/// One benchmark case: the name criterion publishes it under and the input it decodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Case {
    /// Last segment of the criterion id: the case is `decode/<name>`.
    pub name: &'static str,
    /// The raw input the case decodes.
    pub raw: &'static str,
}

/// The cases of the `decode` group.
///
/// The first four are the syllable counts the decode budget is stated for
/// (`BUDGET-LAT-02`: raw of at most twelve syllables), and the last is the hard
/// limit the input buffer accepts, which is the worst input the engine can be
/// handed at all.
///
/// The four inputs are the ones the design names, extended where the design's
/// label and its input disagreed: `jintiantianqizhenbucuo` spells seven syllables
/// by its longest-match cut and `womenmingtianqugongyuanwanba` spells nine, so the
/// eight-syllable case carries one more word and the twelve-syllable case three. A
/// case named `12syl` that decodes nine syllables would understate the budget it
/// exists to guard.
pub const CASES: &[Case] = &[
    Case {
        name: "2syl",
        raw: "nihao",
    },
    Case {
        name: "4syl",
        raw: "zhongguoxiangqi",
    },
    Case {
        name: "8syl",
        raw: "jintiantianqizhenbucuohao",
    },
    Case {
        name: "12syl",
        raw: "womenmingtianqugongyuanwanbazhongguoxue",
    },
    Case {
        name: "64byte",
        raw: MAX_LENGTH_INPUT,
    },
];

/// The longest input the buffer accepts: twelve `nihao` and one `niha`.
///
/// Spelled out rather than built at run time so that the length is settled when the
/// benchmark is compiled; the assertion below fails the build if it drifts.
pub const MAX_LENGTH_INPUT: &str =
    "nihaonihaonihaonihaonihaonihaonihaonihaonihaonihaonihaonihaoniha";

// The length is the whole point of the case, so it is checked by the compiler
// rather than by the run: a const block that panics stops the build.
const _: () = assert!(MAX_LENGTH_INPUT.len() == MAX_RAW_LEN);

/// The keys the corpus must carry a word for, one per benchmark input span.
///
/// Every input is cut by the segmentation layer's own default hint and every
/// contiguous window of up to [`MAX_SEED_SYLLABLES`] syllables of that cut becomes
/// a key. Deriving them rather than listing them keeps the seeds from drifting away
/// from the case table: an input that changes changes its keys with it.
///
/// # Errors
/// Returns a description of the first input that cannot be cut into syllables.
/// That cannot happen for the table above, and it is reported rather than assumed
/// because an input that quietly contributed no keys would leave the benchmark
/// measuring a lattice the corpus does not populate -- which reads as a fast decode
/// rather than as a broken fixture.
pub fn seed_keys() -> Result<Vec<String>, String> {
    let mut keys = Vec::new();
    for case in CASES {
        let syllables = cut(case.raw)
            .map_err(|reason| format!("case {} ({}): {reason}", case.name, case.raw))?;
        for start in 0..syllables.len() {
            for length in 1..=MAX_SEED_SYLLABLES {
                let Some(window) = syllables.get(start..start + length) else {
                    break;
                };
                keys.push(window.join("'"));
            }
        }
    }
    keys.sort();
    keys.dedup();
    Ok(keys)
}

/// The pinyin keys of the held-out evaluation set, in file order.
///
/// The set is decoded as one pass by the `decode/holdout` case, so the corpus has
/// to carry a word for every one of its keys; otherwise the case would measure how
/// fast the decoder gives up rather than how fast it decodes.
///
/// The file is `pinyin<TAB>expected first choice` with `#` comments and blank lines
/// skipped.
pub fn holdout_keys() -> Vec<String> {
    HOLDOUT
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .filter_map(|line| line.split('\t').next())
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The syllables `raw` is cut into, by the segmentation layer's default hint.
fn cut(raw: &str) -> Result<Vec<String>, String> {
    let mut dag = SyllableDag::new();
    dag.build(raw)
        .map_err(|error| format!("cannot be cut into syllables: {error}"))?;
    let mut boundaries = SmallVec::<[u16; HINT_INLINE_BOUNDARIES]>::new();
    if !dag.best_segmentation_hint(&mut boundaries) {
        return Err(String::from("has no segmentation at all"));
    }
    let normalized = dag.normalized();
    let mut syllables = Vec::with_capacity(boundaries.len());
    for pair in boundaries.windows(2) {
        let (start, end) = (usize::from(pair[0]), usize::from(pair[1]));
        let Some(text) = normalized.get(start..end) else {
            return Err(format!(
                "boundary {start}..{end} is not a byte range of the input"
            ));
        };
        // A forced boundary marker belongs to no syllable, so it is dropped from the
        // slice the hint's boundaries delimit.
        syllables.push(text.replace('\'', ""));
    }
    Ok(syllables)
}
