//! Tests for [`super`].
//!
//! They cover the channel itself rather than the engine: the wire shape, the three page
//! states a case asserts on, the revision rules on both sides of the file, and the two
//! properties the acceptance criteria name -- a write that fails never reaches the caller,
//! and a half-written file is diagnosed rather than unwrapped.
//!
//! Every test builds its own scratch root under the system temporary directory, so no test
//! reads the operator's `$XDG_RUNTIME_DIR`, needs a display server, or leaves a file behind.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use ime_types::ui::Script;
use ime_types::{
    Anchor, Candidate, CandidateSource, LayoutHint, PageState, Placement, Preedit, PreeditSpan,
    RectI, ScreenId, SpanKind, StatusStrip, UiFrame,
};

use super::{
    FRAME_FILE, FRAME_FORMAT_VERSION, FrameError, FrameSnapshot, FrameWatch, PageFill, Publish,
    ReadRetry, Reading, UiFrameMirror,
};

/// A scratch directory that removes itself when the test ends.
#[derive(Debug)]
struct Scratch {
    /// The directory this guard owns.
    path: PathBuf,
}

impl Scratch {
    /// Creates `<temp>/rspinyin-uiframe-<tag>-<pid>`, empty.
    ///
    /// The process id keeps two test binaries apart and the tag keeps two tests in one
    /// binary apart, which is what makes the root safe to remove wholesale.
    fn new(tag: &str) -> Self {
        let name = format!("rspinyin-uiframe-{tag}-{}", std::process::id());
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("creating the scratch directory");
        Self { path }
    }

    /// The snapshot file of this scratch directory, inside a mirror directory of its own.
    ///
    /// The mirror directory deliberately does not exist yet: every test that publishes goes
    /// through the creation path, which is the one an operator's first run takes too.
    fn mirror(&self) -> PathBuf {
        self.path.join("runtime").join(FRAME_FILE)
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

/// A frame with every field set to something the defaults would not produce.
fn populated() -> UiFrame {
    let mut frame = frame(7, 2, 9);
    frame.preedit = Preedit {
        text: String::from("ni'hao"),
        caret: 3,
        spans: vec![
            PreeditSpan {
                start: 0,
                end: 2,
                kind: SpanKind::Syllable,
            },
            PreeditSpan {
                start: 2,
                end: 3,
                kind: SpanKind::Separator,
            },
            PreeditSpan {
                start: 3,
                end: 6,
                kind: SpanKind::Cursor,
            },
        ],
    };
    frame.candidates[0].annotation = Some(String::from("dictionary"));
    frame.candidates[1].source = CandidateSource::UserDict;
    frame.candidates[1].consumed_syllables = 2;
    frame.highlight = Some(1);
    frame.page = PageState {
        current: 2,
        total: 3,
        page_size: 9,
    };
    frame.status = StatusStrip {
        mode_label: String::from("拼音"),
        full_width: true,
        punctuation_full: true,
        has_user_dict_hit: true,
        readonly: true,
        script: Script::Traditional,
    };
    frame.anchor = Anchor {
        cursor: RectI {
            x: -5,
            y: 1_000,
            w: 3,
            h: 24,
        },
        screen: ScreenId::new(1),
        scale: 1.5,
        placement: Placement::Auto,
    };
    frame.layout = LayoutHint {
        max_per_row: 9,
        show_annotation: false,
        max_width_dp: 1_024,
    };
    frame
}

/// The temporary file a write to `path` goes through, as the writer names it.
fn temporary_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".tmp");
    PathBuf::from(name)
}

/// The permission bits of `path`.
fn mode_of(path: &Path) -> u32 {
    fs::metadata(path).expect("mode").permissions().mode() & 0o777
}

/// Creates the directory a mirror path lives in.
fn prepare(path: &Path) {
    let dir = path.parent().expect("the mirror directory");
    fs::create_dir_all(dir).expect("creating the mirror directory");
}

/// Reads the mirror and reports the frame it holds, or a message saying it held none.
fn read_frame(path: &Path) -> UiFrame {
    let frame = UiFrameMirror::read_latest(path).expect("reading the mirror");
    frame.expect("a snapshot")
}

#[test]
fn test_snapshot_of_frame_without_candidates_reports_an_empty_page() {
    let frame = frame(1, 0, 9);
    let snapshot = FrameSnapshot::of(&frame);

    assert_eq!(snapshot.revision, 1);
    assert_eq!(snapshot.candidate_count(), 0);
    assert!(snapshot.is_empty());
    assert!(snapshot.texts().is_empty());
    assert!(!snapshot.is_page_full());

    let fill = snapshot.page_fill();
    assert_eq!(
        fill,
        PageFill {
            occupied: 0,
            capacity: 9
        }
    );
    assert!(fill.is_empty());
    assert!(!fill.is_partial());
    assert!(!fill.is_full());

    // A frame with no candidates is still a frame: the window draws a header over an empty
    // grid, and the preedit line is what tells the two apart from "nothing to show".
    assert_eq!(snapshot.preedit.text, "ni");
    assert_eq!(snapshot.to_frame(), frame);
}

#[test]
fn test_snapshot_of_full_page_reports_a_full_grid() {
    let frame = frame(3, 9, 9);
    let snapshot = FrameSnapshot::of(&frame);

    assert_eq!(snapshot.candidate_count(), 9);
    assert!(!snapshot.is_empty());
    assert!(snapshot.is_page_full());
    assert_eq!(
        snapshot.page_fill(),
        PageFill {
            occupied: 9,
            capacity: 9
        }
    );

    let fill = snapshot.page_fill();
    assert!(!fill.is_partial());

    // The order is the one thing the window may not change while drawing, so the texts come
    // back in the order the decoder ranked them.
    let texts = snapshot.texts();
    assert_eq!(texts.first().copied(), Some("cand1"));
    assert_eq!(texts.last().copied(), Some("cand9"));
    assert_eq!(snapshot.to_frame(), frame);
}

#[test]
fn test_snapshot_of_partly_filled_page_reports_the_free_cells() {
    let frame = frame(4, 4, 9);
    let snapshot = FrameSnapshot::of(&frame);

    let fill = snapshot.page_fill();
    assert_eq!(
        fill,
        PageFill {
            occupied: 4,
            capacity: 9
        }
    );
    assert!(fill.is_partial());
    assert!(!fill.is_full());
    assert!(!fill.is_empty());
    assert!(!snapshot.is_page_full());
    assert_eq!(snapshot.to_frame(), frame);
}

#[test]
fn test_page_fill_with_zero_capacity_is_empty_and_not_full() {
    let nothing = PageFill {
        occupied: 0,
        capacity: 0,
    };
    assert!(nothing.is_empty());
    assert!(!nothing.is_full());
    assert!(!nothing.is_partial());

    // More candidates than the page claims room for is a full page, not a broken one: a page
    // state that disagrees with the candidate list is the case's to report, not this type's.
    let over = PageFill {
        occupied: 5,
        capacity: 3,
    };
    assert!(over.is_full());
    assert!(!over.is_partial());
    assert!(!over.is_empty());
}

#[test]
fn test_snapshot_round_trip_preserves_every_field() {
    let frame = populated();
    let snapshot = FrameSnapshot::of(&frame);

    assert_eq!(snapshot.format, FRAME_FORMAT_VERSION);
    let text = snapshot.to_json().expect("a populated frame renders");
    let decoded: FrameSnapshot = serde_json::from_str(&text).expect("the snapshot parses back");

    assert_eq!(decoded, snapshot);
    assert_eq!(decoded.to_frame(), frame);
}

#[test]
fn test_snapshot_json_names_every_field_it_holds() {
    let text = FrameSnapshot::of(&populated())
        .to_json()
        .expect("a frame renders");
    let value: serde_json::Value = serde_json::from_str(&text).expect("the snapshot is JSON");
    let object = value.as_object().expect("a snapshot is a JSON object");

    // The count plus one check per field is what makes this exhaustive: a field that was
    // dropped and another that was added cannot cancel each other out.
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys.len(), 9);
    for field in [
        "anchor",
        "candidates",
        "format",
        "highlight",
        "layout",
        "revision",
        "status",
    ] {
        assert!(keys.contains(&field), "the snapshot must name {field}");
    }
    assert!(keys.contains(&"page"));
    assert!(keys.contains(&"preedit"));
    let version = object.get("format").and_then(serde_json::Value::as_u64);
    assert_eq!(version, Some(u64::from(FRAME_FORMAT_VERSION)));

    let list = object
        .get("candidates")
        .and_then(serde_json::Value::as_array);
    let first = list
        .and_then(|list| list.first())
        .expect("the page holds a candidate");
    let candidate = first.as_object().expect("a candidate is a JSON object");
    let mut inner: Vec<&str> = candidate.keys().map(String::as_str).collect();
    inner.sort_unstable();
    assert_eq!(inner.len(), 6);
    for field in ["annotation", "consumed_syllables", "index", "score"] {
        assert!(inner.contains(&field), "a candidate must carry {field}");
    }
    assert!(inner.contains(&"source"));
    assert!(inner.contains(&"text"));
}

#[test]
fn test_publish_writes_a_snapshot_that_reads_back_identically() {
    let scratch = Scratch::new("publish");
    let path = scratch.mirror();
    let mirror = UiFrameMirror::new(&path);
    let frame = frame(1, 3, 9);

    assert_eq!(mirror.held(), None);
    assert_eq!(mirror.failures(), 0);
    let absent = UiFrameMirror::read_latest(&path).expect("reading an absent mirror");
    assert_eq!(absent, None);

    let outcome = mirror.publish(&frame);
    assert!(outcome.is_written(), "{outcome:?}");
    assert!(matches!(outcome, Publish::Written { revision: 1 }));
    assert_eq!(mirror.held(), Some(1));
    assert_eq!(mirror.failures(), 0);
    assert_eq!(read_frame(&path), frame);

    // The rename moved the temporary file, so a reader that looks for it finds nothing and
    // falls through to the named file.
    assert!(!temporary_of(&path).exists());
}

#[test]
fn test_publish_keeps_the_newer_snapshot_when_an_older_frame_arrives() {
    let scratch = Scratch::new("revision");
    let path = scratch.mirror();
    let mirror = UiFrameMirror::new(&path);

    assert!(mirror.publish(&frame(5, 1, 9)).is_written());

    // An older frame must not overwrite a newer snapshot, and a repeat of the frame already
    // in the file is not a write either: the file is a slot, and rewriting it with the same
    // frame would only move its modification time.
    let older = mirror.publish(&frame(4, 2, 9));
    let skipped = matches!(older, Publish::Skipped { held: 5, seen: 4 });
    assert!(skipped, "{older:?}");
    let repeat = mirror.publish(&frame(5, 1, 9));
    let unchanged = matches!(repeat, Publish::Skipped { held: 5, seen: 5 });
    assert!(unchanged, "{repeat:?}");
    assert_eq!(mirror.held(), Some(5));
    assert_eq!(mirror.failures(), 0);

    let latest = read_frame(&path);
    assert_eq!(latest.revision, 5);
    assert_eq!(latest.candidates.len(), 1);

    assert!(mirror.publish(&frame(6, 2, 9)).is_written());
    assert_eq!(mirror.held(), Some(6));
}

#[test]
fn test_publish_write_failure_does_not_reach_the_caller() {
    // A directory that refuses the write is simulated with a path whose parent is a regular
    // file. The failure is the one a read-only directory produces, and unlike a permission
    // bit it does not depend on the account the tests run as.
    let scratch = Scratch::new("failure");
    let blocker = scratch.path.join("blocker");
    fs::write(&blocker, b"not a directory").expect("writing the blocker");
    let mirror = UiFrameMirror::new(&blocker.join(FRAME_FILE));

    let outcome = mirror.publish(&frame(1, 1, 9));
    let failed = matches!(outcome, Publish::Failed(FrameError::Io { .. }));
    assert!(failed, "{outcome:?}");
    assert_eq!(mirror.failures(), 1);
    let floor = mirror.held();
    assert_eq!(floor, None, "a failed write must not raise the floor");

    // The next frame is tried rather than skipped as already written, so a directory that
    // becomes writable again starts working without a restart.
    let again = mirror.publish(&frame(1, 1, 9));
    assert!(matches!(again, Publish::Failed(_)), "{again:?}");
    assert_eq!(mirror.failures(), 2);
}

#[test]
fn test_publish_refuses_a_frame_whose_score_is_not_a_number() {
    let scratch = Scratch::new("nan");
    let path = scratch.mirror();
    let mirror = UiFrameMirror::new(&path);

    assert!(mirror.publish(&frame(1, 1, 9)).is_written());

    let mut broken = frame(2, 1, 9);
    broken.candidates[0].score = f32::NAN;
    let outcome = mirror.publish(&broken);
    let refused = matches!(outcome, Publish::Failed(FrameError::Encode { .. }));
    assert!(refused, "{outcome:?}");
    assert_eq!(mirror.failures(), 1);
    let floor = mirror.held();
    assert_eq!(floor, Some(1), "a refused frame must not raise the floor");

    // The snapshot already in the file survives: the refusal happens before the temporary
    // file is opened, so there is nothing half-written to clean up either.
    assert_eq!(read_frame(&path).revision, 1);
    assert!(!temporary_of(&path).exists());
}

#[test]
fn test_read_latest_refuses_files_that_are_not_snapshots() {
    let scratch = Scratch::new("malformed");
    let path = scratch.mirror();
    prepare(&path);

    let cases = [
        // A writer that was killed before it wrote anything.
        "",
        // A file caught mid-write.
        "{\"format\": 1, \"revis",
        // Something else entirely.
        "not json at all",
        // A snapshot from another schema.
        "{\"revision\": 7, \"unexpected\": 1}",
    ];
    for text in cases {
        fs::write(&path, text).expect("writing the file");
        let outcome = UiFrameMirror::read_latest(&path);
        let error = outcome.expect_err("a file that is not a snapshot must be refused");
        assert!(matches!(error, FrameError::Malformed { .. }), "{error}");
        assert!(error.to_string().contains("ui_frame.json"), "{error}");
    }
}

#[test]
fn test_malformed_snapshot_error_does_not_repeat_the_file_content() {
    let scratch = Scratch::new("redaction");
    let path = scratch.mirror();
    prepare(&path);
    // The text the user typed, in a field the contract says is a number. `serde_json`'s own
    // message quotes the offending value, and this message is printed and archived.
    let text = "{\"format\": 1, \"revision\": \"ni'hao\"}";
    fs::write(&path, text).expect("writing the file");

    let outcome = UiFrameMirror::read_latest(&path);
    let error = outcome.expect_err("a mistyped field is refused");
    let message = error.to_string();
    assert!(!message.contains("ni'hao"), "{message}");
    assert!(message.contains("line 1"), "{message}");
    assert!(message.contains("not a frame snapshot"), "{message}");
    assert!(matches!(error, FrameError::Malformed { .. }), "{error}");
}

#[test]
fn test_read_latest_recovers_from_a_half_written_named_file() {
    let scratch = Scratch::new("recovery");
    let path = scratch.mirror();
    let mirror = UiFrameMirror::new(&path);
    assert!(mirror.publish(&frame(1, 1, 9)).is_written());

    // A writer that was interrupted between its write and its rename leaves a complete
    // temporary file beside a named file that is being rewritten.
    let newer = FrameSnapshot::of(&frame(7, 2, 9));
    let text = newer.to_json().expect("rendering");
    fs::write(temporary_of(&path), text).expect("writing the temporary file");
    fs::write(&path, "{\"format\": 1, \"revis").expect("truncating the named file");

    let found = read_frame(&path);
    assert_eq!(found.revision, 7);
    assert_eq!(found.candidates.len(), 2);

    // With no temporary file to fall back on, the same file is reported rather than
    // unwrapped: a case fails with a diagnosis instead of a panic.
    fs::remove_file(temporary_of(&path)).expect("removing the temporary file");
    let outcome = UiFrameMirror::read_latest(&path);
    let error = outcome.expect_err("a half-written file is not a snapshot");
    assert!(matches!(error, FrameError::Malformed { .. }), "{error}");
}

#[test]
fn test_read_latest_retries_until_a_concurrent_write_lands() {
    let scratch = Scratch::new("retry");
    let path = scratch.mirror();
    prepare(&path);
    fs::write(&path, "{\"format\": 1, \"revis").expect("leaving a half-written snapshot");

    // The writer lands well after the reader's first look, so the retry is what makes this
    // pass. The budget is deliberately far larger than the delay: the machine may be busy,
    // and a case that fails because a thread was scheduled late teaches nothing.
    let waiting = path.clone();
    let writer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        let mirror = UiFrameMirror::new(&waiting);
        let outcome = mirror.publish(&frame(9, 1, 9));
        assert!(outcome.is_written(), "{outcome:?}");
    });

    let retry = ReadRetry {
        attempts: 200,
        pause: Duration::from_millis(5),
    };
    let outcome = UiFrameMirror::read_latest_with(&path, retry);
    let read = outcome.expect("reading while the writer lands");
    let found = read.expect("a snapshot");
    assert_eq!(found.revision, 9);
    writer.join().expect("the writer thread finishes");
}

#[test]
fn test_read_latest_refuses_a_snapshot_from_another_format() {
    let scratch = Scratch::new("format");
    let path = scratch.mirror();
    prepare(&path);
    let text = FrameSnapshot::of(&frame(1, 1, 9))
        .to_json()
        .expect("rendering");

    let mut value: serde_json::Value = serde_json::from_str(&text).expect("it is JSON");
    value["format"] = serde_json::Value::from(99_u32);
    fs::write(&path, value.to_string()).expect("writing the file");

    let outcome = UiFrameMirror::read_latest(&path);
    let error = outcome.expect_err("another format is refused");
    let found = matches!(error, FrameError::Format { found: 99, .. });
    assert!(found, "{error}");
    let message = error.to_string();
    assert!(message.contains("99"), "{message}");
    assert!(message.contains("ui_frame.json"), "{message}");

    // The same file at the version this build writes reads back, so the refusal is about the
    // version and not about the rewrite.
    fs::write(&path, text).expect("writing the original");
    let again = UiFrameMirror::read_latest(&path).expect("reading");
    assert!(again.is_some());
}

#[test]
fn test_read_latest_refuses_a_snapshot_from_the_previous_format() {
    // The first format wrote no `highlight`. The field's serde default is what makes
    // such a document *parse*, so the refusal comes from the version check -- the one
    // gate the format's evolution rule names -- instead of from the shape of the file.
    let scratch = Scratch::new("old-format");
    let path = scratch.mirror();
    prepare(&path);
    let text = FrameSnapshot::of(&frame(1, 1, 9))
        .to_json()
        .expect("rendering");
    let mut value: serde_json::Value = serde_json::from_str(&text).expect("it is JSON");
    let object = value.as_object_mut().expect("a snapshot is a JSON object");
    object.remove("highlight");
    object.insert(String::from("format"), serde_json::Value::from(1_u32));
    fs::write(&path, value.to_string()).expect("writing the older snapshot");

    let outcome = UiFrameMirror::read_latest(&path);
    let error = outcome.expect_err("a snapshot from an older format is refused");
    assert!(
        matches!(error, FrameError::Format { found: 1, .. }),
        "{error}"
    );
}

#[test]
fn test_read_latest_reports_an_io_error_for_a_path_that_is_a_directory() {
    let scratch = Scratch::new("directory");
    let path = scratch.path.join("a-directory");
    fs::create_dir_all(&path).expect("creating the directory");

    let outcome = UiFrameMirror::read_latest(&path);
    let error = outcome.expect_err("a directory is not a snapshot");
    assert!(matches!(error, FrameError::Io { .. }), "{error}");
    assert!(error.to_string().contains("a-directory"), "{error}");
}

#[test]
fn test_frame_watch_reports_new_repeated_and_rewound_snapshots() {
    let scratch = Scratch::new("watch");
    let path = scratch.mirror();
    let mirror = UiFrameMirror::new(&path);
    let mut watch = FrameWatch::open(&path);

    assert_eq!(watch.path(), path.as_path());
    assert_eq!(watch.held(), None);
    let absent = watch.read().expect("reading an absent mirror");
    assert_eq!(absent, Reading::Absent);

    assert!(mirror.publish(&frame(1, 1, 9)).is_written());
    let first = watch.read().expect("reading the first frame");
    match first {
        Reading::New(frame) => assert_eq!(frame.revision, 1),
        other => panic!("the first frame must be new: {other:?}"),
    }
    assert_eq!(watch.held(), Some(1));
    assert_eq!(watch.rewinds(), 0);

    // The same snapshot again is not news: a case that polls must not treat it as a frame it
    // has not seen yet.
    let repeat = watch.read().expect("reading the same frame");
    assert_eq!(repeat, Reading::Repeated(1));

    assert!(mirror.publish(&frame(2, 1, 9)).is_written());
    let next = watch.read().expect("reading the next frame");
    assert!(matches!(next, Reading::New(frame) if frame.revision == 2));
    assert_eq!(watch.held(), Some(2));

    // A file that went backwards -- a replayed snapshot, or a mirror that was reset without
    // a new watch -- is reported and counted, and the held revision does not move back.
    let older = FrameSnapshot::of(&frame(1, 1, 9));
    fs::write(&path, older.to_json().expect("rendering")).expect("writing an older snapshot");
    let rewound = watch.read().expect("reading a rewound snapshot");
    assert_eq!(rewound, Reading::Rewound { held: 2, seen: 1 });
    assert_eq!(watch.held(), Some(2));
    assert_eq!(watch.rewinds(), 1);
}

#[test]
fn test_frame_watch_starts_clean_after_a_reset() {
    let scratch = Scratch::new("reset");
    let path = scratch.mirror();
    let mirror = UiFrameMirror::new(&path);
    let mut watch = FrameWatch::open(&path);

    assert!(mirror.publish(&frame(4, 1, 9)).is_written());
    assert!(matches!(watch.read().expect("reading"), Reading::New(_)));
    assert_eq!(watch.held(), Some(4));

    // A new case resets the mirror and starts a new mirror and a new watch, which is what
    // keeps the reset from looking like a rewind.
    fs::remove_file(&path).expect("resetting the mirror");
    let fresh_mirror = UiFrameMirror::new(&path);
    let mut fresh_watch = FrameWatch::open(&path);

    let absent = fresh_watch.read().expect("reading an absent mirror");
    assert_eq!(absent, Reading::Absent);
    assert!(fresh_mirror.publish(&frame(1, 1, 9)).is_written());
    let first = fresh_watch.read().expect("reading the first frame");
    assert!(matches!(first, Reading::New(frame) if frame.revision == 1));
    assert_eq!(fresh_watch.rewinds(), 0);
}

#[test]
fn test_frame_watch_read_with_one_attempt_reports_a_half_written_file() {
    let scratch = Scratch::new("one-attempt");
    let path = scratch.mirror();
    prepare(&path);
    fs::write(&path, "{\"format\": 1, \"revis").expect("writing a half-written snapshot");

    let mut watch = FrameWatch::open(&path);
    let retry = ReadRetry {
        attempts: 1,
        pause: Duration::from_millis(50),
    };
    let outcome = watch.read_with(retry);
    let error = outcome.expect_err("one look at a broken file is a failure");
    assert!(matches!(error, FrameError::Malformed { .. }), "{error}");
    assert_eq!(watch.held(), None);
    assert_eq!(watch.rewinds(), 0);
}

#[test]
fn test_publish_creates_a_private_mirror_directory() {
    let scratch = Scratch::new("modes");
    let dir = scratch.path.join("runtime");
    let mirror = UiFrameMirror::at_mirror_dir(&dir);
    assert_eq!(mirror.path(), dir.join(FRAME_FILE));

    assert!(mirror.publish(&frame(1, 1, 9)).is_written());
    assert_eq!(mode_of(&dir), 0o700);
    assert_eq!(mode_of(mirror.path()), 0o600);
    // The name the sandbox clears between cases is the name the mirror writes.
    assert_eq!(FRAME_FILE, "ui_frame.json");
    assert!(dir.join(FRAME_FILE).is_file());
}

#[test]
fn test_frame_error_messages_name_the_path_and_the_reason() {
    let path = PathBuf::from("/tmp/mirror/ui_frame.json");
    let io = FrameError::Io {
        path: path.clone(),
        source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
    };
    assert!(io.to_string().contains("ui_frame.json"), "{io}");
    assert!(io.to_string().contains("denied"), "{io}");

    let format = FrameError::Format {
        path: path.clone(),
        found: 99,
        expected: FRAME_FORMAT_VERSION,
    };
    let message = format.to_string();
    assert!(message.contains("99"), "{message}");
    assert!(message.contains("ui_frame.json"), "{message}");

    let detail = String::from("candidate 3 has a display score that is not a finite number");
    let encode = FrameError::Encode { detail };
    assert!(encode.to_string().contains("candidate 3"), "{encode}");

    let malformed = FrameError::Malformed {
        path,
        detail: String::from("the file is empty"),
    };
    let message = malformed.to_string();
    assert!(message.contains("the file is empty"), "{message}");
    assert!(message.contains("ui_frame.json"), "{message}");
}
