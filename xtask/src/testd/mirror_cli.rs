//! `xtask test-mirror` -- the command-line surface of the frame snapshot channel.
//!
//! # Responsibility
//!
//! The plugin's publish half (the UI addon's mirror, armed by `RSPINYIN_TEST_MIRROR_DIR`)
//! writes the newest frame the engine posted into one JSON file; this module reads that file
//! back, reports it, asserts on it, and optionally keeps a copy of it in a case's evidence
//! bundle. It is the consumer side of the channel whose producer is the armed session, and
//! the two meet at the file format `ime_diag::uiframe` defines -- there is exactly one
//! definition of that shape in the workspace.
//!
//! # Where the text goes
//!
//! Nowhere near the terminal. A snapshot holds what the user typed -- the preedit, the
//! candidate texts -- so the report names counts, revisions, page states and geometry and
//! never a text field, the same discipline the readback channel's command line applies to a
//! transcript. The one output that carries the text is `--json-out`: an explicit request to
//! copy the snapshot into a case's evidence bundle, written `0600` like every other file
//! here that holds user data.
//!
//! # The revision rule
//!
//! Reads go through [`FrameWatch`], which applies the contract's own rule -- a frame older
//! than the newest one already read is dropped and counted -- so a mirror directory that is
//! reset in the middle of a wait shows up as a rewind count rather than as an assertion
//! against a frame the window never drew.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Args;

use crate::testd::uiframe::{FRAME_FILE, FrameError, FrameSnapshot, FrameWatch, Reading};
use ime_types::UiFrame;
use ime_types::ui::Script;

/// How long a wait sleeps between two looks at the mirror.
///
/// Short enough that a case which injects keys and then reads notices the frame within one
/// poll; long enough that a two-second wait costs one hundred reads and no more.
const POLL_STEP: Duration = Duration::from_millis(20);

/// Mode the `--json-out` copy is created with.
///
/// The copy holds the same text the mirror file does, so it is written the way every other
/// file holding user data in this project is: readable by its owner and by nobody else.
const COPY_MODE: u32 = 0o600;

/// Command-line surface of `xtask test-mirror`.
#[derive(Debug, Args)]
pub struct MirrorArgs {
    /// Mirror directory the armed plugin publishes into: the value `RSPINYIN_TEST_MIRROR_DIR`
    /// was set to when the session under test started.
    dir: PathBuf,
    /// Poll for up to this many milliseconds for a snapshot to appear, or to advance to
    /// `--expect-revision`; without it the mirror is read exactly once.
    #[arg(long)]
    wait_ms: Option<u64>,
    /// Assert the newest frame's revision is exactly this value.
    #[arg(long)]
    expect_revision: Option<u32>,
    /// Assert the newest frame carries exactly this many candidates.
    #[arg(long, conflicts_with = "expect_empty")]
    expect_candidates: Option<usize>,
    /// Assert the newest frame carries no candidate at all.
    #[arg(long)]
    expect_empty: bool,
    /// Assert the candidate at this 1-based position carries exactly this annotation, as
    /// `INDEX=TEXT`; `INDEX=-` asserts it carries none. Repeatable.
    ///
    /// The annotation is the engine's own grey hint -- a correction, a fuzzy note, a symbol
    /// category -- never user input, which is why a case may name it on the command line.
    #[arg(long, value_name = "INDEX=TEXT|-")]
    assert_annotation: Vec<String>,
    /// Assert the frame's candidates carry exactly these texts, in this order,
    /// comma-separated; an empty value asserts an empty list.
    ///
    /// The order is the property the pin and the ranking cases assert: a candidate promoted
    /// to the front shows up here before it shows up anywhere a user could see.
    #[arg(long, value_name = "TEXT[,TEXT]", conflicts_with = "expect_empty")]
    assert_full_list: Option<String>,
    /// Assert a mode-strip field carries this value, as `FIELD=VALUE`. Repeatable. The
    /// fields are `mode-label`, `script`, `chinese`, `full-width`, `punctuation-full`,
    /// `user-dict-hit` and `readonly`; the script reads `simplified` or `traditional`, the
    /// flags read `true` or `false`, and the label reads any text.
    #[arg(long, value_name = "FIELD=VALUE")]
    assert_status: Vec<String>,
    /// Copy the snapshot JSON here, `0600`, for the case's evidence bundle.
    #[arg(long, value_name = "PATH")]
    json_out: Option<PathBuf>,
}

/// One annotation assertion: the candidate at this 1-based position carries this hint.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AnnotationExpectation {
    /// The candidate's 1-based position.
    index: u16,
    /// The annotation the candidate must carry, or `None` for none.
    wanted: Option<String>,
}

/// Which mode-strip field a status assertion names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusField {
    /// The strip's mode label.
    ModeLabel,
    /// The script the strip shows.
    Script,
    /// The strip's Chinese bit.
    Chinese,
    /// Full-width characters.
    FullWidth,
    /// Full-width punctuation.
    PunctuationFull,
    /// A hit from the user's own dictionary.
    UserDictHit,
    /// Read-only mode.
    Readonly,
}

impl StatusField {
    /// Reads the field a status assertion names.
    ///
    /// # Errors
    ///
    /// A message naming the fields the assertion knows when the name is none of them.
    ///
    /// # Panics
    ///
    /// Never.
    fn parse(name: &str) -> Result<Self, String> {
        match name {
            "mode-label" => Ok(Self::ModeLabel),
            "script" => Ok(Self::Script),
            "chinese" => Ok(Self::Chinese),
            "full-width" => Ok(Self::FullWidth),
            "punctuation-full" => Ok(Self::PunctuationFull),
            "user-dict-hit" => Ok(Self::UserDictHit),
            "readonly" => Ok(Self::Readonly),
            _ => Err(format!(
                "unknown strip field {name:?}; the fields are mode-label, script, chinese, \
                 full-width, punctuation-full, user-dict-hit and readonly"
            )),
        }
    }

    /// The field as a refusal names it.
    ///
    /// # Panics
    ///
    /// Never.
    fn name(self) -> &'static str {
        match self {
            Self::ModeLabel => "mode-label",
            Self::Script => "script",
            Self::Chinese => "chinese",
            Self::FullWidth => "full-width",
            Self::PunctuationFull => "punctuation-full",
            Self::UserDictHit => "user-dict-hit",
            Self::Readonly => "readonly",
        }
    }
}

/// One status assertion: the named strip field carries this value.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StatusExpectation {
    /// The field the assertion names.
    field: StatusField,
    /// The value the field must carry, as the argument spelled it.
    wanted: String,
}

/// What the run asserts about the frame it read.
///
/// The checks a case asks of the mirror -- which revision arrived, how many candidates it
/// carried, whether it carried any, what a candidate's hint reads, what order the
/// candidates stand in, what the strip shows -- named once so [`verdict`] stays the single
/// place a refusal is worded from. Every refusal names counts, positions and field names;
/// the values a refusal would have to quote to be "helpful" are the frame's texts, and
/// those stay out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Expectations {
    /// The revision the newest frame must carry.
    revision: Option<u32>,
    /// The candidate count the newest frame must carry.
    candidates: Option<usize>,
    /// Whether the newest frame must carry no candidate.
    empty: bool,
    /// The annotation assertions, in the order the arguments named them.
    annotations: Vec<AnnotationExpectation>,
    /// The candidate texts the frame must carry, in order, when the full list is asserted.
    full_list: Option<Vec<String>>,
    /// The strip assertions, in the order the arguments named them.
    status: Vec<StatusExpectation>,
}

impl Expectations {
    /// Reads the assertions the arguments asked for.
    ///
    /// # Errors
    ///
    /// A message naming the offending argument when an annotation spec carries no `=` or a
    /// position that is not a number, when a full list is not parseable, or when a status
    /// spec names an unknown field or a value the field cannot carry.
    ///
    /// # Panics
    ///
    /// Never.
    fn from_args(args: &MirrorArgs) -> Result<Self, String> {
        let mut annotations = Vec::new();
        for spec in &args.assert_annotation {
            let (index, wanted) = spec
                .split_once('=')
                .ok_or_else(|| format!("{spec:?} does not read INDEX=TEXT"))?;
            let index: u16 = index
                .parse()
                .map_err(|_| format!("{index:?} does not carry a candidate position"))?;
            if index == 0 {
                return Err(format!(
                    "{index:?}: candidate positions are 1-based, so 0 names no candidate"
                ));
            }
            let wanted = match wanted {
                "-" => None,
                text => Some(String::from(text)),
            };
            annotations.push(AnnotationExpectation { index, wanted });
        }
        // An empty value names an empty list, the order channel's "no candidate at all":
        // `split(',')` on an empty string would yield one empty piece, which asserts a
        // candidate whose text is the empty string instead.
        let full_list = args
            .assert_full_list
            .as_ref()
            .map(|list| {
                if list.is_empty() {
                    Vec::new()
                } else {
                    list.split(',').map(String::from).collect()
                }
            });
        let mut status = Vec::new();
        for spec in &args.assert_status {
            let (name, wanted) = spec
                .split_once('=')
                .ok_or_else(|| format!("{spec:?} does not read FIELD=VALUE"))?;
            let field = StatusField::parse(name)?;
            // The label's value is free text, so it has no spelling to check; every other
            // field reads one of two words, and a misspelled word is an argument error
            // rather than an assertion that can never hold.
            match field {
                StatusField::ModeLabel => {}
                StatusField::Script => {
                    if !matches!(wanted, "simplified" | "traditional") {
                        return Err(format!(
                            "{name:?} reads simplified or traditional, not {wanted:?}"
                        ));
                    }
                }
                _ => {
                    if !matches!(wanted, "true" | "false") {
                        return Err(format!("{name:?} reads true or false, not {wanted:?}"));
                    }
                }
            }
            status.push(StatusExpectation {
                field,
                wanted: String::from(wanted),
            });
        }
        Ok(Self {
            revision: args.expect_revision,
            candidates: args.expect_candidates,
            empty: args.expect_empty,
            annotations,
            full_list,
            status,
        })
    }
}

/// Entry point for `xtask test-mirror`.
///
/// # Errors
///
/// Returns an error when the mirror cannot be read, when no snapshot was published at the
/// directory within the wait the arguments gave, when an assertion's arguments do not
/// parse, when the frame misses an assertion, and when the `--json-out` copy cannot be
/// written. None of the messages carries a text field of the frame: the snapshot holds what
/// the user typed, and a refusal is printed into a terminal.
///
/// The `ok` line each satisfied assertion group prints is the ledger's hook: a case table
/// names it in an `@regex` expectation, and the suite's command channel lands it as one
/// more claim in the case's v3 record.
///
/// # Panics
///
/// Never.
pub fn run(args: MirrorArgs) -> Result<()> {
    let expectations = Expectations::from_args(&args).map_err(anyhow::Error::msg)?;
    let window = args.wait_ms.map_or(Duration::ZERO, Duration::from_millis);
    let mut watch = FrameWatch::open(&args.dir.join(FRAME_FILE));
    let frame = wait_for_frame(&mut watch, window, expectations.revision)
        .with_context(|| format!("reading the frame mirror at {}", args.dir.display()))?
        .ok_or_else(|| no_snapshot(&args.dir, window))?;
    println!("{}", summary(&frame));
    verdict(&frame, &expectations).map_err(anyhow::Error::msg)?;
    // One line per satisfied group, so a case table can pin each group it asked for
    // without re-reading the whole summary. The wording is the command's contract with
    // those tables -- the ledger's `@regex` hook -- so it is produced in one tested place.
    for line in ok_lines(&expectations) {
        println!("{line}");
    }
    let rewinds = watch.rewinds();
    if rewinds > 0 {
        println!(
            "test-mirror: the mirror offered an older frame {rewinds} time(s) during the wait; \
             each one was dropped, not asserted on"
        );
    }
    if let Some(path) = &args.json_out {
        let bytes = write_copy(path, &frame)
            .with_context(|| format!("copying the snapshot to {}", path.display()))?;
        println!(
            "test-mirror: copied the snapshot, {bytes} bytes, to {} (0600)",
            path.display()
        );
    }
    Ok(())
}

/// The `ok` lines a satisfied run prints, one per assertion group it was asked to check.
///
/// A case table pins each of these with an `@regex` expectation -- the ledger's hook into
/// the command channel -- so the wording is part of the command's contract and is asserted
/// by tests rather than reworded casually. A group the run did not ask for prints nothing,
/// so a case that names a line knows the group ran and held.
///
/// # Panics
///
/// Never.
fn ok_lines(expectations: &Expectations) -> Vec<String> {
    let mut lines = Vec::new();
    if !expectations.annotations.is_empty() {
        lines.push(format!(
            "test-mirror: assert annotation ok ({} checked)",
            expectations.annotations.len()
        ));
    }
    if let Some(list) = &expectations.full_list {
        lines.push(format!(
            "test-mirror: assert full-list ok ({} candidate(s))",
            list.len()
        ));
    }
    if !expectations.status.is_empty() {
        lines.push(format!(
            "test-mirror: assert status ok ({} checked)",
            expectations.status.len()
        ));
    }
    lines
}

/// Reads the mirror until a frame satisfying the revision expectation arrives, or the
/// window closes.
///
/// A window of zero performs exactly one read, which is the path a case takes after it has
/// already waited for the injection to land. A frame the watch drops as a rewind leaves the
/// returned frame alone -- the newest frame is the one a previous call returned, and the
/// drop is counted on the watch for the caller to report.
///
/// # Errors
///
/// Returns the watch's own refusal when a file that exists is not a snapshot this build can
/// read; the transient half-written state a live writer produces is retried inside the
/// watch before this reports anything.
///
/// # Panics
///
/// Never.
fn wait_for_frame(
    watch: &mut FrameWatch,
    window: Duration,
    expect_revision: Option<u32>,
) -> Result<Option<UiFrame>, FrameError> {
    let deadline = deadline_after(window);
    let mut latest = None;
    loop {
        match watch.read()? {
            Reading::Absent | Reading::Repeated(_) | Reading::Rewound { .. } => {}
            Reading::New(frame) => {
                let satisfied = expect_revision.is_none_or(|wanted| frame.revision == wanted);
                latest = Some(*frame);
                if satisfied {
                    return Ok(latest);
                }
            }
        }
        let left = remaining(deadline);
        if left.is_zero() {
            return Ok(latest);
        }
        sleep(POLL_STEP.min(left));
    }
}

/// Judges a frame against what the run asserted about it.
///
/// The refusal names counts and revisions -- the numbers a case asserted on -- and never a
/// text field: this message reaches a terminal and a CI log, and the frame's texts are the
/// user's input.
///
/// # Errors
///
/// Returns the first missed assertion as a plain message; the caller renders it as the run's
/// error.
///
/// # Panics
///
/// Never.
fn verdict(frame: &UiFrame, want: &Expectations) -> Result<(), String> {
    if let Some(wanted) = want.revision
        && frame.revision != wanted
    {
        return Err(format!(
            "the newest frame is revision {}, not {wanted}",
            frame.revision
        ));
    }
    if want.empty && !frame.candidates.is_empty() {
        return Err(format!(
            "the newest frame holds {} candidate(s) where none were expected",
            frame.candidates.len()
        ));
    }
    if let Some(wanted) = want.candidates
        && frame.candidates.len() != wanted
    {
        return Err(format!(
            "the newest frame holds {} candidate(s), not {wanted}",
            frame.candidates.len()
        ));
    }
    for expected in &want.annotations {
        let Some(candidate) = frame.candidates.get(usize::from(expected.index - 1)) else {
            return Err(format!(
                "the newest frame holds {} candidate(s); the annotation assertion names \
                 candidate {}",
                frame.candidates.len(),
                expected.index
            ));
        };
        if candidate.annotation != expected.wanted {
            return Err(format!(
                "candidate {}'s annotation is not what the case asserted",
                expected.index
            ));
        }
    }
    if let Some(wanted) = &want.full_list {
        if frame.candidates.len() != wanted.len() {
            return Err(format!(
                "the newest frame holds {} candidate(s), not {}",
                frame.candidates.len(),
                wanted.len()
            ));
        }
        // The first position where the order differs is named, and nothing else: the texts
        // on either side of the difference are the user's input, and the refusal is a
        // terminal's business.
        for (position, (held, expected)) in frame.candidates.iter().zip(wanted).enumerate() {
            if held.text != *expected {
                return Err(format!(
                    "the candidates differ from position {} onward",
                    position + 1
                ));
            }
        }
    }
    for expected in &want.status {
        // The held value is read as the word the argument spelled, so a comparison is a
        // comparison of spellings and never of parsed structures.
        let held: &str = match expected.field {
            StatusField::ModeLabel => frame.status.mode_label.as_str(),
            StatusField::Script => script_word(frame.status.script),
            StatusField::Chinese => strip_flag(frame.status.chinese),
            StatusField::FullWidth => strip_flag(frame.status.full_width),
            StatusField::PunctuationFull => strip_flag(frame.status.punctuation_full),
            StatusField::UserDictHit => strip_flag(frame.status.has_user_dict_hit),
            StatusField::Readonly => strip_flag(frame.status.readonly),
        };
        if held != expected.wanted {
            return Err(format!(
                "the strip's {} is not what the case asserted",
                expected.field.name()
            ));
        }
    }
    Ok(())
}

/// The flag as the word a status assertion spells it with.
///
/// # Panics
///
/// Never.
fn strip_flag(held: bool) -> &'static str {
    if held {
        "true"
    } else {
        "false"
    }
}

/// The script as the word a status assertion spells it with.
///
/// # Panics
///
/// Never.
fn script_word(script: Script) -> &'static str {
    match script {
        Script::Simplified => "simplified",
        Script::Traditional => "traditional",
    }
}

/// The observation a run prints: every structural fact of the frame, no text field.
///
/// A case reads this to see what arrived before it reads the assertions' verdict, and a
/// failure report quotes it; both are reasons the preedit, the candidate texts, the
/// annotations and the mode label stay out of it. The mode label in particular is skipped
/// even though it is interface copy rather than user input: the authoritative mode is the
/// strip's `chinese` bit, and keeping every string out of the report means the report can
/// never need a redaction pass.
///
/// # Panics
///
/// Never.
fn summary(frame: &UiFrame) -> String {
    let fill = FrameSnapshot::of(frame).page_fill();
    let fill = if fill.is_empty() {
        "empty"
    } else if fill.is_full() {
        "full"
    } else {
        "partial"
    };
    let page = &frame.page;
    let anchor = &frame.anchor;
    let layout = &frame.layout;
    let status = &frame.status;
    format!(
        "test-mirror: revision {revision} holds {count} candidate(s), page {current}/{total} \
         sized {page_size} ({fill}), highlight {highlight:?}, anchor cursor {x},{y} {w}x{h} on \
         screen {screen} at scale {scale}, placement {placement:?}, layout rows {max_per_row} \
         annotation {show_annotation} width {max_width_dp}dp, status chinese {chinese} \
         full-width {full_width} punctuation-full {punctuation_full} user-dict {user_dict} \
         readonly {readonly} script {script:?}",
        revision = frame.revision,
        count = frame.candidates.len(),
        current = page.current,
        total = page.total,
        page_size = page.page_size,
        fill = fill,
        highlight = frame.highlight,
        x = anchor.cursor.x,
        y = anchor.cursor.y,
        w = anchor.cursor.w,
        h = anchor.cursor.h,
        screen = anchor.screen.value(),
        scale = anchor.scale,
        placement = anchor.placement,
        max_per_row = layout.max_per_row,
        show_annotation = layout.show_annotation,
        max_width_dp = layout.max_width_dp,
        chinese = status.chinese,
        full_width = status.full_width,
        punctuation_full = status.punctuation_full,
        user_dict = status.has_user_dict_hit,
        readonly = status.readonly,
        script = status.script,
    )
}

/// Copies the snapshot to `path` for a case's evidence bundle.
///
/// The copy is written through the same codec the mirror uses, so a file that cannot
/// round-trip fails here rather than surprising the case that reads it back later. It is
/// created `0600`: it holds what the user typed.
///
/// # Errors
///
/// Returns the codec's refusal for a frame whose floating-point fields cannot survive the
/// file, and an IO error when the file cannot be created or written.
///
/// # Panics
///
/// Never.
fn write_copy(path: &Path, frame: &UiFrame) -> Result<usize> {
    let text = FrameSnapshot::of(frame).to_json()?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(COPY_MODE)
        .open(path)?;
    file.write_all(text.as_bytes())?;
    file.flush()?;
    Ok(text.len())
}

/// The refusal a mirror that published nothing is reported with.
///
/// The message names the directory, the wait and the arming conditions, because the three
/// usual causes live exactly there: the variable unset, the directory outside the sandbox,
/// and the frame simply not posted yet.
///
/// # Panics
///
/// Never.
fn no_snapshot(dir: &Path, waited: Duration) -> anyhow::Error {
    let waited = if waited.is_zero() {
        String::new()
    } else {
        format!(" within {waited:?}")
    };
    anyhow::anyhow!(
        "no frame snapshot has been published at {}{waited}; the plugin publishes only when \
         RSPINYIN_TEST_MIRROR_DIR names this directory and the directory sits inside \
         $XDG_RUNTIME_DIR",
        dir.display()
    )
}

/// The instant a window of `window` from now falls on.
///
/// The addition is checked because `Instant + Duration` panics on overflow, and a panic in
/// the harness is a case that fails for a reason nobody can read. A window large enough to
/// overflow is longer than the machine will exist, so falling back to now is the honest
/// answer: every wait then ends at its first deadline check.
///
/// # Panics
///
/// Never.
fn deadline_after(window: Duration) -> Instant {
    Instant::now()
        .checked_add(window)
        .unwrap_or_else(Instant::now)
}

/// How much of a deadline is left, never negative.
///
/// # Panics
///
/// Never.
fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

#[cfg(test)]
mod tests;
