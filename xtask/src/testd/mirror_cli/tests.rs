//! Tests for the command-line surface of the frame snapshot channel.
//!
//! Split out of `mirror_cli.rs` to keep that file inside the line limit. They hold the run's
//! three faces: what it reports (never a text field), what it asserts (the refusals name
//! counts, positions and field names), and what it copies (`0600`, into the case's bundle).

use std::fs;

use ime_types::{
    Anchor, Candidate, CandidateSource, LayoutHint, PageState, Placement, Preedit, RectI, ScreenId,
    StatusStrip, UiFrame,
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
        assert_annotation: Vec::new(),
        assert_full_list: None,
        assert_status: Vec::new(),
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
        assert_annotation: Vec::new(),
        assert_full_list: None,
        assert_status: Vec::new(),
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
        annotations: Vec::new(),
        full_list: None,
        status: Vec::new(),
    };
    assert_eq!(verdict(&published, &want), Ok(()));
    let empty_page = frame(8, 0, 9);
    let want_empty = Expectations {
        revision: Some(8),
        candidates: None,
        empty: true,
        annotations: Vec::new(),
        full_list: None,
        status: Vec::new(),
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
        annotations: Vec::new(),
        full_list: None,
        status: Vec::new(),
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
        annotations: Vec::new(),
        full_list: None,
        status: Vec::new(),
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

#[test]
fn test_verdict_accepts_annotations_at_their_positions_and_none_claims() {
    let mut frame = frame(7, 2, 9);
    frame.candidates[0].annotation = Some(String::from("multi-tone: de"));
    let want = Expectations {
        annotations: vec![
            AnnotationExpectation {
                index: 1,
                wanted: Some(String::from("multi-tone: de")),
            },
            AnnotationExpectation {
                index: 2,
                wanted: None,
            },
        ],
        ..Expectations::default()
    };
    assert_eq!(verdict(&frame, &want), Ok(()));
}

#[test]
fn test_verdict_refuses_an_annotation_miss_without_quoting_it() {
    let mut frame = frame(7, 2, 9);
    frame.candidates[0].annotation = Some(String::from("multi-tone: de"));
    let missed = Expectations {
        annotations: vec![AnnotationExpectation {
            index: 1,
            wanted: Some(String::from("fuzzy: shi -> si")),
        }],
        ..Expectations::default()
    };
    let failure = verdict(&frame, &missed).expect_err("a differing annotation is a refusal");
    assert!(
        failure.contains("candidate 1") && failure.contains("annotation"),
        "{failure}"
    );
    for secret in ["multi-tone", "fuzzy"] {
        assert!(
            !failure.contains(secret),
            "the refusal must not carry annotation text: {failure}"
        );
    }
    let out_of_range = Expectations {
        annotations: vec![AnnotationExpectation {
            index: 5,
            wanted: None,
        }],
        ..Expectations::default()
    };
    let failure =
        verdict(&frame, &out_of_range).expect_err("a position past the page is a refusal");
    assert!(failure.contains("names candidate 5"), "{failure}");
}

#[test]
fn test_verdict_names_the_first_differing_position_of_a_full_list() {
    let frame = frame(7, 3, 9);
    let want = Expectations {
        full_list: Some(vec![
            String::from("cand1"),
            String::from("cand2"),
            String::from("cand3"),
        ]),
        ..Expectations::default()
    };
    assert_eq!(verdict(&frame, &want), Ok(()));
    let reordered = Expectations {
        full_list: Some(vec![
            String::from("cand1"),
            String::from("cand3"),
            String::from("cand2"),
        ]),
        ..Expectations::default()
    };
    let failure = verdict(&frame, &reordered).expect_err("a reordered list is a refusal");
    assert!(failure.contains("position 2 onward"), "{failure}");
    let shorter = Expectations {
        full_list: Some(vec![String::from("cand1"), String::from("cand2")]),
        ..Expectations::default()
    };
    let failure = verdict(&frame, &shorter).expect_err("a shorter list is a refusal");
    assert!(failure.contains("3 candidate(s), not 2"), "{failure}");
}

#[test]
fn test_verdict_checks_strip_fields_and_names_the_field_on_a_miss() {
    let mut frame = frame(7, 1, 9);
    frame.status.mode_label = String::from("拼音");
    frame.status.chinese = true;
    frame.status.full_width = true;
    frame.status.script = Script::Traditional;
    let want = Expectations {
        status: vec![
            StatusExpectation {
                field: StatusField::ModeLabel,
                wanted: String::from("拼音"),
            },
            StatusExpectation {
                field: StatusField::Script,
                wanted: String::from("traditional"),
            },
            StatusExpectation {
                field: StatusField::Chinese,
                wanted: String::from("true"),
            },
            StatusExpectation {
                field: StatusField::Readonly,
                wanted: String::from("false"),
            },
        ],
        ..Expectations::default()
    };
    assert_eq!(verdict(&frame, &want), Ok(()));
    let missed_flag = Expectations {
        status: vec![StatusExpectation {
            field: StatusField::Chinese,
            wanted: String::from("false"),
        }],
        ..Expectations::default()
    };
    let failure = verdict(&frame, &missed_flag).expect_err("a missed flag is a refusal");
    assert!(failure.contains("the strip's chinese"), "{failure}");
    let missed_label = Expectations {
        status: vec![StatusExpectation {
            field: StatusField::ModeLabel,
            wanted: String::from("英文"),
        }],
        ..Expectations::default()
    };
    let failure = verdict(&frame, &missed_label).expect_err("a missed label is a refusal");
    assert!(failure.contains("the strip's mode-label"), "{failure}");
    assert!(
        !failure.contains("拼音"),
        "the refusal must not carry the label's text: {failure}"
    );
}

#[test]
fn test_from_args_reads_every_assertion_spelling() {
    let args = MirrorArgs {
        dir: PathBuf::new(),
        wait_ms: None,
        expect_revision: None,
        expect_candidates: None,
        expect_empty: false,
        assert_annotation: vec![String::from("1=multi-tone: de"), String::from("3=-")],
        assert_full_list: Some(String::from("你,好")),
        assert_status: vec![
            String::from("chinese=true"),
            String::from("script=traditional"),
            String::from("mode-label=拼音"),
        ],
        json_out: None,
    };
    let want = Expectations::from_args(&args).expect("well-formed assertions");
    assert_eq!(
        want.annotations,
        vec![
            AnnotationExpectation {
                index: 1,
                wanted: Some(String::from("multi-tone: de")),
            },
            AnnotationExpectation {
                index: 3,
                wanted: None,
            },
        ]
    );
    assert_eq!(
        want.full_list,
        Some(vec![String::from("你"), String::from("好")])
    );
    assert_eq!(
        want.status,
        vec![
            StatusExpectation {
                field: StatusField::Chinese,
                wanted: String::from("true"),
            },
            StatusExpectation {
                field: StatusField::Script,
                wanted: String::from("traditional"),
            },
            StatusExpectation {
                field: StatusField::ModeLabel,
                wanted: String::from("拼音"),
            },
        ]
    );
}

#[test]
fn test_from_args_refuses_malformed_assertions() {
    let annotations = |specs: &[&str]| {
        Expectations::from_args(&MirrorArgs {
            assert_annotation: specs.iter().map(|spec| String::from(*spec)).collect(),
            ..plain_args(PathBuf::new())
        })
    };
    let failure = annotations(&["1-no-equals"]).expect_err("no `=` is not a position spec");
    assert!(failure.contains("does not read INDEX=TEXT"), "{failure}");
    let failure = annotations(&["x=text"]).expect_err("a non-number names no candidate");
    assert!(
        failure.contains("does not carry a candidate position"),
        "{failure}"
    );
    let failure = annotations(&["0=text"]).expect_err("position 0 names no candidate");
    assert!(failure.contains("1-based"), "{failure}");
    let status = |specs: &[&str]| {
        Expectations::from_args(&MirrorArgs {
            assert_status: specs.iter().map(|spec| String::from(*spec)).collect(),
            ..plain_args(PathBuf::new())
        })
    };
    let failure = status(&["volume"]).expect_err("no `=` is not a field spec");
    assert!(failure.contains("does not read FIELD=VALUE"), "{failure}");
    let failure = status(&["volume=high"]).expect_err("an unknown field is an argument error");
    assert!(failure.contains("unknown strip field"), "{failure}");
    let failure = status(&["chinese=yes"]).expect_err("a flag reads true or false");
    assert!(failure.contains("reads true or false"), "{failure}");
    let failure = status(&["script=both"]).expect_err("the script reads one of two words");
    assert!(
        failure.contains("reads simplified or traditional"),
        "{failure}"
    );
}

#[test]
fn test_from_args_reads_an_empty_full_list_as_an_empty_list() {
    let args = MirrorArgs {
        assert_full_list: Some(String::new()),
        ..plain_args(PathBuf::new())
    };
    let want = Expectations::from_args(&args).expect("an empty value is well-formed");
    assert_eq!(want.full_list, Some(Vec::new()));
    let empty = frame(1, 0, 9);
    assert_eq!(verdict(&empty, &want), Ok(()));
    let occupied = frame(1, 1, 9);
    let failure = verdict(&occupied, &want).expect_err("a candidate misses an empty list");
    assert!(failure.contains("1 candidate(s), not 0"), "{failure}");
}

#[test]
fn test_ok_lines_name_exactly_the_groups_a_run_checked() {
    let all = Expectations {
        annotations: vec![AnnotationExpectation {
            index: 1,
            wanted: None,
        }],
        full_list: Some(vec![String::from("cand1")]),
        status: vec![StatusExpectation {
            field: StatusField::Readonly,
            wanted: String::from("false"),
        }],
        ..Expectations::default()
    };
    assert_eq!(
        ok_lines(&all),
        vec![
            String::from("test-mirror: assert annotation ok (1 checked)"),
            String::from("test-mirror: assert full-list ok (1 candidate(s))"),
            String::from("test-mirror: assert status ok (1 checked)"),
        ]
    );
    assert!(
        ok_lines(&Expectations::default()).is_empty(),
        "a run that asserted nothing prints no group line"
    );
}

#[test]
fn test_run_asserts_annotations_order_and_strip_together() {
    let scratch = Scratch::new("run-assert");
    let mirror = UiFrameMirror::at_mirror_dir(&scratch.mirror_dir());
    let mut published = frame(9, 2, 9);
    published.candidates[0].annotation = Some(String::from("multi-tone: de"));
    published.status.readonly = true;
    mirror.publish(&published);
    let outcome = run(MirrorArgs {
        dir: scratch.mirror_dir(),
        wait_ms: None,
        expect_revision: Some(9),
        expect_candidates: None,
        expect_empty: false,
        assert_annotation: vec![String::from("1=multi-tone: de"), String::from("2=-")],
        assert_full_list: Some(String::from("cand1,cand2")),
        assert_status: vec![
            String::from("readonly=true"),
            String::from("script=simplified"),
        ],
        json_out: None,
    });
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[test]
fn test_run_refuses_an_annotation_miss_without_saying_the_text() {
    let scratch = Scratch::new("run-assert-miss");
    let mirror = UiFrameMirror::at_mirror_dir(&scratch.mirror_dir());
    let mut published = frame(9, 1, 9);
    published.candidates[0].annotation = Some(String::from("multi-tone: de"));
    mirror.publish(&published);
    let error = run(MirrorArgs {
        dir: scratch.mirror_dir(),
        wait_ms: None,
        expect_revision: None,
        expect_candidates: None,
        expect_empty: false,
        assert_annotation: vec![String::from("1=fuzzy: shi -> si")],
        assert_full_list: None,
        assert_status: Vec::new(),
        json_out: None,
    })
    .expect_err("a missed annotation is a failed run")
    .to_string();
    assert!(error.contains("candidate 1"), "{error}");
    for secret in ["multi-tone", "fuzzy"] {
        assert!(
            !error.contains(secret),
            "the refusal must not carry annotation text: {error}"
        );
    }
}
