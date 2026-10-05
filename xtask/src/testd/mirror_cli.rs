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
    /// Copy the snapshot JSON here, `0600`, for the case's evidence bundle.
    #[arg(long, value_name = "PATH")]
    json_out: Option<PathBuf>,
}

/// What the run asserts about the frame it read.
///
/// The three checks a case asks of the mirror -- which revision arrived, how many candidates
/// it carried, whether it carried any -- named once so [`verdict`] stays the single place a
/// refusal is worded from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Expectations {
    /// The revision the newest frame must carry.
    revision: Option<u32>,
    /// The candidate count the newest frame must carry.
    candidates: Option<usize>,
    /// Whether the newest frame must carry no candidate.
    empty: bool,
}

impl Expectations {
    /// Reads the assertions the arguments asked for.
    ///
    /// # Panics
    ///
    /// Never.
    fn from_args(args: &MirrorArgs) -> Self {
        Self {
            revision: args.expect_revision,
            candidates: args.expect_candidates,
            empty: args.expect_empty,
        }
    }
}

/// Entry point for `xtask test-mirror`.
///
/// # Errors
///
/// Returns an error when the mirror cannot be read, when no snapshot was published at the
/// directory within the wait the arguments gave, when the frame misses an assertion, and
/// when the `--json-out` copy cannot be written. None of the messages carries a text field
/// of the frame: the snapshot holds what the user typed, and a refusal is printed into a
/// terminal.
///
/// # Panics
///
/// Never.
pub fn run(args: MirrorArgs) -> Result<()> {
    let expectations = Expectations::from_args(&args);
    let window = args.wait_ms.map_or(Duration::ZERO, Duration::from_millis);
    let mut watch = FrameWatch::open(&args.dir.join(FRAME_FILE));
    let frame = wait_for_frame(&mut watch, window, expectations.revision)
        .with_context(|| format!("reading the frame mirror at {}", args.dir.display()))?
        .ok_or_else(|| no_snapshot(&args.dir, window))?;
    println!("{}", summary(&frame));
    verdict(&frame, &expectations).map_err(anyhow::Error::msg)?;
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
    if let Some(wanted) = want.revision {
        if frame.revision != wanted {
            return Err(format!(
                "the newest frame is revision {}, not {wanted}",
                frame.revision
            ));
        }
    }
    if want.empty && !frame.candidates.is_empty() {
        return Err(format!(
            "the newest frame holds {} candidate(s) where none were expected",
            frame.candidates.len()
        ));
    }
    if let Some(wanted) = want.candidates {
        if frame.candidates.len() != wanted {
            return Err(format!(
                "the newest frame holds {} candidate(s), not {wanted}",
                frame.candidates.len()
            ));
        }
    }
    Ok(())
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
mod tests {
    use std::fs;

    use ime_types::{
        Anchor, Candidate, CandidateSource, LayoutHint, PageState, Placement, Preedit, RectI,
        ScreenId, StatusStrip, UiFrame,
    };

    use super::*;
    use crate::testd::uiframe::UiFrameMirror;

    /// A scratch directory that removes itself when the test ends.
    struct Scratch {
        path: PathBuf,
    }

    impl Scratch {
        /// Creates `<temp>/rspinyin-mirror-cli-<tag>-<pid>`, empty.
        fn new(tag: &str) -> Self {
            let name = format!("rspinyin-mirror-cli-{tag}-{}", std::process::id());
            let path = std::env::temp_dir().join(name);
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("creating the scratch directory");
            Self { path }
        }

        /// The mirror directory of this scratch tree; it does not exist until a publish
        /// creates it, which is the path an operator's first run takes too.
        fn mirror_dir(&self) -> PathBuf {
            self.path.join("runtime")
        }

        /// The snapshot file inside the mirror directory.
        fn snapshot(&self) -> PathBuf {
            self.mirror_dir().join(FRAME_FILE)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// A frame with `count` candidates numbered from one, on a page sized for `page_size`.
    fn frame(revision: u32, count: usize, page_size: u8) -> UiFrame {
        UiFrame {
            revision,
            preedit: Preedit {
                text: String::from("ni"),
                caret: 2,
                spans: Vec::new(),
            },
            candidates: candidates(count),
            page: PageState {
                current: 1,
                total: 1,
                page_size,
            },
            status: StatusStrip::default(),
            anchor: anchor(),
            layout: LayoutHint {
                max_per_row: 5,
                show_annotation: true,
                max_width_dp: 720,
            },
            highlight: None,
        }
    }

    /// `count` candidates, in the order a decoder would rank them.
    fn candidates(count: usize) -> Vec<Candidate> {
        (1..=count)
            .map(|index| {
                let number = u16::try_from(index).expect("a test candidate index fits in u16");
                Candidate {
                    index: number,
                    text: format!("cand{number}"),
                    annotation: None,
                    source: CandidateSource::Dict,
                    score: f32::from(number),
                    consumed_syllables: 1,
                }
            })
            .collect()
    }

    /// A cursor rectangle on the first screen.
    fn anchor() -> Anchor {
        Anchor {
            cursor: RectI {
                x: 100,
                y: 200,
                w: 2,
                h: 20,
            },
            screen: ScreenId::new(0),
            scale: 1.0,
            placement: Placement::Below,
        }
    }

    /// The arguments that read `dir` with nothing asserted and nothing copied.
    fn plain_args(dir: PathBuf) -> MirrorArgs {
        MirrorArgs {
            dir,
            wait_ms: None,
            expect_revision: None,
            expect_candidates: None,
            expect_empty: false,
            json_out: None,
        }
    }

    #[test]
    fn test_run_reports_a_published_frame_and_writes_a_private_copy() {
        let scratch = Scratch::new("run-published");
        let mirror = UiFrameMirror::at_mirror_dir(&scratch.mirror_dir());
        mirror.publish(&frame(7, 2, 9));
        let copy = scratch.path.join("snapshot.json");
        let outcome = run(MirrorArgs {
            dir: scratch.mirror_dir(),
            wait_ms: None,
            expect_revision: Some(7),
            expect_candidates: Some(2),
            expect_empty: false,
            json_out: Some(copy.clone()),
        });
        assert!(outcome.is_ok(), "{outcome:?}");
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&copy)
            .expect("the copy exists")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "the copy holds user input and must be private"
        );
        let mut watch = FrameWatch::open(&copy);
        match watch.read().expect("the copy parses") {
            Reading::New(round_tripped) => assert_eq!(round_tripped.revision, 7),
            reading => panic!("the copy must hold the frame, not {reading:?}"),
        }
    }

    #[test]
    fn test_run_fails_on_a_mirror_that_never_published() {
        let scratch = Scratch::new("run-absent");
        let mut args = plain_args(scratch.mirror_dir());
        args.wait_ms = Some(1);
        let error = run(args)
            .expect_err("an absent mirror is a failed run")
            .to_string();
        assert!(error.contains("no frame snapshot"), "{error}");
        assert!(error.contains("RSPINYIN_TEST_MIRROR_DIR"), "{error}");
    }

    #[test]
    fn test_run_fails_when_the_frame_misses_an_expectation() {
        let scratch = Scratch::new("run-miss");
        let mirror = UiFrameMirror::at_mirror_dir(&scratch.mirror_dir());
        mirror.publish(&frame(7, 2, 9));
        let mut args = plain_args(scratch.mirror_dir());
        args.expect_candidates = Some(3);
        let error = run(args)
            .expect_err("a missed count is a failed run")
            .to_string();
        assert!(error.contains("2 candidate(s), not 3"), "{error}");

        let mut args = plain_args(scratch.mirror_dir());
        args.expect_revision = Some(6);
        let error = run(args)
            .expect_err("a missed revision is a failed run")
            .to_string();
        assert!(error.contains("revision 7, not 6"), "{error}");
    }

    #[test]
    fn test_verdict_accepts_a_frame_matching_every_expectation() {
        let published = frame(7, 2, 9);
        let want = Expectations {
            revision: Some(7),
            candidates: Some(2),
            empty: false,
        };
        assert_eq!(verdict(&published, &want), Ok(()));
        let empty_page = frame(8, 0, 9);
        let want_empty = Expectations {
            revision: Some(8),
            candidates: None,
            empty: true,
        };
        assert_eq!(verdict(&empty_page, &want_empty), Ok(()));
    }

    #[test]
    fn test_verdict_names_counts_and_never_the_frame_text() {
        let mut frame = frame(7, 2, 9);
        frame.candidates[0].text = String::from("保密");
        frame.preedit.text = String::from("mi");
        let want = Expectations {
            revision: Some(7),
            candidates: Some(5),
            empty: false,
        };
        let failure = verdict(&frame, &want).expect_err("a missed count is a refusal");
        assert!(failure.contains("2 candidate(s), not 5"), "{failure}");
        assert!(
            !failure.contains("保密"),
            "the refusal must not carry candidate text: {failure}"
        );
        let empty_want = Expectations {
            revision: None,
            candidates: None,
            empty: true,
        };
        let failure =
            verdict(&frame, &empty_want).expect_err("a non-empty page misses an empty assertion");
        assert!(
            failure.contains("2 candidate(s) where none were expected"),
            "{failure}"
        );
        assert!(
            !failure.contains("保密"),
            "the refusal must not carry candidate text: {failure}"
        );
    }

    #[test]
    fn test_summary_reports_a_frame_without_its_text() {
        let mut frame = frame(7, 2, 9);
        frame.preedit.text = String::from("ni'hao");
        frame.candidates[0].text = String::from("你好");
        frame.status.mode_label = String::from("拼音");
        frame.highlight = Some(1);
        let report = summary(&frame);
        assert!(report.contains("revision 7"), "{report}");
        assert!(report.contains("2 candidate(s)"), "{report}");
        assert!(report.contains("highlight Some(1)"), "{report}");
        for secret in ["ni'hao", "你好", "拼音"] {
            assert!(
                !report.contains(secret),
                "the summary must not carry {secret:?}: {report}"
            );
        }
    }

    #[test]
    fn test_wait_for_frame_returns_the_newest_frame_once_the_revision_matches() {
        let scratch = Scratch::new("wait-match");
        let mirror = UiFrameMirror::at_mirror_dir(&scratch.mirror_dir());
        mirror.publish(&frame(3, 1, 9));
        mirror.publish(&frame(5, 2, 9));
        let mut watch = FrameWatch::open(&scratch.snapshot());
        let frame = wait_for_frame(&mut watch, Duration::ZERO, Some(5))
            .expect("the watch reads")
            .expect("revision 5 was published");
        assert_eq!(frame.revision, 5);
        assert_eq!(
            frame.candidates.len(),
            2,
            "the newest frame is the one returned"
        );
    }

    #[test]
    fn test_wait_for_frame_reports_an_absent_mirror_as_none() {
        let scratch = Scratch::new("wait-absent");
        let mut watch = FrameWatch::open(&scratch.snapshot());
        let frame = wait_for_frame(&mut watch, Duration::ZERO, None).expect("the watch reads");
        assert!(
            frame.is_none(),
            "an unpublished mirror is not a frame: {frame:?}"
        );
    }

    #[test]
    fn test_wait_for_frame_keeps_no_frame_when_the_file_rewinds() {
        let scratch = Scratch::new("wait-rewind");
        let mirror = UiFrameMirror::at_mirror_dir(&scratch.mirror_dir());
        mirror.publish(&frame(5, 2, 9));
        let mut watch = FrameWatch::open(&scratch.snapshot());
        let first = wait_for_frame(&mut watch, Duration::ZERO, None)
            .expect("the watch reads")
            .expect("revision 5 was published");
        assert_eq!(first.revision, 5);
        // A second publisher with a fresh floor rewrites an older revision over the file,
        // which is what a reset mirror directory looks like to a watch that stayed open.
        let fresh = UiFrameMirror::at_mirror_dir(&scratch.mirror_dir());
        fresh.publish(&frame(1, 1, 9));
        let second = wait_for_frame(&mut watch, Duration::ZERO, None).expect("the watch reads");
        assert!(
            second.is_none(),
            "a rewound frame is dropped, not handed to the caller: {second:?}"
        );
        assert_eq!(watch.rewinds(), 1, "the drop is counted as a rewind");
    }
}
