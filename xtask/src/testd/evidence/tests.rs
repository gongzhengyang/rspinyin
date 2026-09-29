//! Tests for [`super`].
//!
//! They cover the archive rather than a case: the naming a run gets, the documents a case
//! leaves, the two indexes, and the rules the acceptance criteria name -- a passing case leaves
//! no trace, a failing one leaves a trace with its diff and its defect, a case segment a path
//! cannot carry is refused before anything is created, a flat legacy directory is refused,
//! `latest_run` follows the newest batch, a second write of a case replaces the first bundle
//! rather than joining it, and a bundle that cannot be written becomes an evidence gap that
//! stops neither the rest of the batch nor the run's verdict from reporting it.
//!
//! Every test that writes builds its own scratch directory under the system temporary
//! directory, so nothing here touches the repository, the operator's home or a display server,
//! and no test reads a clock: the instants they use are named explicitly.
//!
//! The two indexes have tests of their own beside this file, in `index`.

mod index;

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use serde_json::Value;

use super::*;
use crate::testd::logs::LogTap;

/// A scratch directory that removes itself when the test ends.
#[derive(Debug)]
struct Scratch {
    /// The directory this guard owns.
    path: PathBuf,
}

impl Scratch {
    /// Creates `<temp>/rspinyin-evidence-<tag>-<pid>`, empty.
    ///
    /// The process id keeps two test binaries apart and the tag keeps two tests in one binary
    /// apart, which is what makes the root safe to remove wholesale.
    fn new(tag: &str) -> Self {
        let name = format!("rspinyin-evidence-{tag}-{}", std::process::id());
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("creating the scratch directory");
        Self { path }
    }

    /// The results tree a run would write into.
    fn results(&self) -> PathBuf {
        self.path.join("results")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// The permission bits of `path`.
fn mode_of(path: &Path) -> u32 {
    fs::metadata(path)
        .expect("the path is there")
        .permissions()
        .mode()
        & 0o777
}

/// The instant every test names as the run's start, so no test reads a clock.
fn started() -> Utc {
    Utc::from_unix_seconds(1_700_000_000)
}

/// A case's verdict record with nothing recorded about it yet.
fn evidence(module: &str, tc: &str, status: CaseStatus) -> CaseEvidence {
    CaseEvidence {
        tc: tc.to_owned(),
        module: module.to_owned(),
        status,
        started_at: started(),
        duration_ms: 12,
        assertions: Vec::new(),
        visual: Vec::new(),
        healed: Vec::new(),
        environment: Vec::new(),
    }
}

/// A verdict record with one assertion that held and one that did not.
///
/// The failing one compares values that could carry what the user typed, which is what makes it
/// the test of the withheld form: the text goes in, a length comes out.
fn evidence_with_assertions(module: &str, tc: &str, status: CaseStatus) -> CaseEvidence {
    let mut record = evidence(module, tc, status);
    record.assertions = vec![
        AssertionRecord {
            name: String::from("candidate_count"),
            expected: EvidenceValue::fact("5"),
            actual: EvidenceValue::fact("5"),
            ok: true,
        },
        AssertionRecord {
            name: String::from("first_candidate"),
            expected: EvidenceValue::withheld("你好"),
            actual: EvidenceValue::withheld("你好吧"),
            ok: false,
        },
    ];
    record
}

/// A failure trace for `record`, with a defect, a stack and no log.
fn trace_of(record: &CaseEvidence) -> TraceRecord {
    let mut trace = TraceRecord::of(record);
    trace.defects.push(ImageDefect {
        item: String::from("corner_radius"),
        image: String::from("01_default.png"),
        rect: DefectRect {
            x: 4,
            y: 4,
            w: 8,
            h: 8,
        },
        expected: EvidenceValue::fact("#1e1e1e"),
        measured: EvidenceValue::fact("#3c3c3c"),
    });
    trace.stack = Some(String::from("the frame was not the one the case posted"));
    trace
}

/// A cross-run entry for `run`, as the index holds it.
fn entry_of(run: &RunDir, started: Utc, verdict: BatchVerdict) -> RunEntry {
    RunEntry {
        run: run.name().to_owned(),
        path: format!("results/runs/{}", run.name()),
        started_at: started.iso8601(),
        verdict,
        cases: 1,
        passed: 1,
        failed: 0,
        flawed: 0,
        evidence_gaps: 0,
        healed: 0,
    }
}

/// Reads a document the archive wrote.
fn read_json(path: &Path) -> Value {
    let text = fs::read_to_string(path).expect("the document is there");
    serde_json::from_str(&text).expect("the document is JSON")
}

#[test]
fn test_utc_renders_known_instants_in_both_forms() {
    let epoch = Utc::from_unix_seconds(0);
    assert_eq!(epoch.compact(), "19700101-000000");
    assert_eq!(epoch.iso8601(), "1970-01-01T00:00:00Z");
    assert_eq!(epoch.run_name(), "run-19700101-000000");
    assert_eq!(epoch.unix_seconds(), 0);

    assert_eq!(
        Utc::from_unix_seconds(1_000_000_000).compact(),
        "20010909-014640"
    );
    assert_eq!(
        Utc::from_unix_seconds(1_700_000_000).iso8601(),
        "2023-11-14T22:13:20Z"
    );
    // 2000 is a leap year: the shifted-year arithmetic must not lose the 29th of February.
    assert_eq!(
        Utc::from_unix_seconds(951_782_400).compact(),
        "20000229-000000"
    );
    // The second before a new year belongs to the old one.
    assert_eq!(
        Utc::from_unix_seconds(1_735_689_599).iso8601(),
        "2024-12-31T23:59:59Z"
    );
    // The 32-bit rollover, which is as far as a signed half-word reaches.
    assert_eq!(
        Utc::from_unix_seconds(2_147_483_647).iso8601(),
        "2038-01-19T03:14:07Z"
    );
    // A count before the epoch is a date before it, not a wrapped one.
    assert_eq!(Utc::from_unix_seconds(-1).iso8601(), "1969-12-31T23:59:59Z");
}

#[test]
fn test_utc_of_a_system_time_reads_seconds_and_clamps_a_clock_before_the_epoch() {
    let after = UNIX_EPOCH + Duration::from_secs(1_000_000_000) + Duration::from_millis(999);
    assert_eq!(
        Utc::of(after).unix_seconds(),
        1_000_000_000,
        "the sub-second part is dropped"
    );
    assert_eq!(Utc::of(UNIX_EPOCH).compact(), "19700101-000000");
    // A directory name cannot carry a negative year, so a clock before 1970 reads as 1970.
    let before = UNIX_EPOCH - Duration::from_secs(60);
    assert_eq!(Utc::of(before).unix_seconds(), 0);
}

#[test]
fn test_utc_now_reads_the_process_clock() {
    let now = Utc::now();
    assert!(
        now.unix_seconds() >= 1_700_000_000,
        "the clock reads after November 2023, and {now:?} does not"
    );
    assert!(
        Utc::now() >= now,
        "a clock that runs forwards does not go back between two reads"
    );
}

#[test]
fn test_case_status_labels_and_names_its_trace_need() {
    assert_eq!(CaseStatus::Pass.label(), "pass");
    assert_eq!(CaseStatus::Fail.label(), "fail");
    assert_eq!(CaseStatus::Flawed.label(), "flawed");
    assert!(!CaseStatus::Pass.needs_trace(), "a pass leaves no trace");
    assert!(CaseStatus::Fail.needs_trace());
    assert!(CaseStatus::Flawed.needs_trace());
    assert_eq!(
        serde_json::to_string(&CaseStatus::Flawed).expect("a status is JSON"),
        "\"flawed\""
    );
}

#[test]
fn test_evidence_value_keeps_a_fact_and_withholds_a_value() {
    let fact = EvidenceValue::fact("12dp");
    assert_eq!(fact.as_str(), "12dp");

    let withheld = EvidenceValue::withheld("你好");
    assert_eq!(withheld.as_str(), "<redacted:len=2>");
    assert!(
        !withheld.as_str().contains('你'),
        "the value itself is dropped, not escaped"
    );
    assert_eq!(
        EvidenceValue::withheld("ni").as_str(),
        "<redacted:len=2>",
        "characters are counted, not bytes"
    );
    assert_eq!(EvidenceValue::withheld("").as_str(), "<redacted:len=0>");
}

#[test]
fn test_case_evidence_json_holds_the_documented_fields() {
    let mut record = evidence_with_assertions("core", "TC-CORE-01", CaseStatus::Fail);
    record.visual.push(VisualRecord {
        item: String::from("corner_radius"),
        expected: EvidenceValue::fact("12dp"),
        measured: EvidenceValue::fact("24px"),
        verdict: Verdict::Fail,
    });
    // An item this machine cannot judge is recorded as such rather than as a pass: the gate
    // reads the three states apart.
    record.visual.push(VisualRecord {
        item: String::from("acrylic_blur"),
        expected: EvidenceValue::fact("blurred"),
        measured: EvidenceValue::fact("opaque"),
        verdict: Verdict::Unverifiable,
    });
    record.healed.push(HealRecord {
        old: String::from("CellHeight"),
        new: String::from("CandidateCellHeight"),
        basis: String::from("features.md 3.1"),
    });
    record.environment.push(EnvironmentFact {
        key: String::from("display_server"),
        value: String::from("x11"),
    });

    let text = record.to_json().expect("the verdict record is JSON");
    let document: Value = serde_json::from_str(&text).expect("the document is JSON");
    assert_eq!(document["tc"], "TC-CORE-01");
    assert_eq!(document["module"], "core");
    assert_eq!(document["status"], "fail");
    assert_eq!(document["started_at"], "2023-11-14T22:13:20Z");
    assert_eq!(document["duration_ms"], 12);
    assert_eq!(document["assertions"][1]["name"], "first_candidate");
    assert_eq!(document["assertions"][1]["ok"], false);
    assert_eq!(document["visual"][0]["verdict"], "fail");
    assert_eq!(document["visual"][1]["item"], "acrylic_blur");
    assert_eq!(document["visual"][1]["verdict"], "unverifiable");
    assert_eq!(document["healed"][0]["new"], "CandidateCellHeight");
    assert_eq!(document["environment"][0]["key"], "display_server");
    // The archive is a privacy surface: what a case withheld is nowhere in the document.
    assert!(!text.contains('你'), "{text}");
    assert!(text.contains("<redacted:len=3>"), "{text}");
}

#[test]
fn test_case_evidence_counts_its_assertions() {
    let record = evidence_with_assertions("core", "TC-CORE-01", CaseStatus::Fail);
    assert_eq!(record.assertion_count(), 2);
    assert_eq!(record.failed_assertions().len(), 1);
    assert_eq!(record.failed_assertions()[0].name, "first_candidate");

    let clean = evidence("core", "TC-CORE-02", CaseStatus::Pass);
    assert_eq!(clean.assertion_count(), 0);
    assert!(clean.failed_assertions().is_empty());
}

#[test]
fn test_environment_fact_splits_a_line_on_its_first_separator() {
    let fact = EnvironmentFact::of_line("display_server: x11");
    assert_eq!(fact.key, "display_server");
    assert_eq!(fact.value, "x11");

    let addon = EnvironmentFact::of_line("addon rspinyin: category InputMethod, loaded true");
    assert_eq!(addon.key, "addon rspinyin");
    assert_eq!(addon.value, "category InputMethod, loaded true");

    // A line in a shape this does not recognise is recorded rather than dropped.
    let odd = EnvironmentFact::of_line("  a fact with no separator  ");
    assert_eq!(odd.key, "a fact with no separator");
    assert!(odd.value.is_empty());

    let empty = EnvironmentFact::of_line("");
    assert!(empty.key.is_empty());
    assert!(empty.value.is_empty());
}

#[test]
fn test_log_excerpt_keeps_the_tail_of_what_the_tap_handed_out() {
    let scratch = Scratch::new("log-excerpt");
    let path = scratch.path.join("logs").join("rspinyin.log");
    fs::create_dir_all(path.parent().expect("the log has a directory")).expect("the log directory");
    let mut text = String::new();
    for index in 0..(LOG_EXCERPT_LINES + 5) {
        text.push_str(&format!("line {index}\n"));
    }
    fs::write(&path, text).expect("the log is written");

    let mut tap = LogTap::at(&path);
    tap.drain().expect("the log is readable");
    let excerpt = LogExcerpt::of(&tap, None);

    assert_eq!(excerpt.total_lines, LOG_EXCERPT_LINES + 5);
    assert_eq!(excerpt.lines.len(), LOG_EXCERPT_LINES);
    assert_eq!(
        excerpt.lines.first().map(String::as_str),
        Some("line 5"),
        "the oldest lines are the ones the tail leaves out"
    );
    let newest = format!("line {}", LOG_EXCERPT_LINES + 4);
    assert_eq!(
        excerpt.lines.last().map(String::as_str),
        Some(newest.as_str())
    );
    assert_eq!(
        excerpt.path.as_deref(),
        Some(path.to_string_lossy().as_ref()),
        "the excerpt names the file it came from"
    );
}

#[test]
fn test_log_excerpt_shortens_a_home_prefix() {
    let scratch = Scratch::new("log-home");
    let home = scratch.path.join("home");
    let path = home
        .join("data")
        .join("rspinyin")
        .join("logs")
        .join("rspinyin.log");
    fs::create_dir_all(path.parent().expect("the log has a directory")).expect("the log directory");
    fs::write(&path, "one line\n").expect("the log is written");

    let mut tap = LogTap::at(&path);
    tap.drain().expect("the log is readable");
    let excerpt = LogExcerpt::of(&tap, Some(&home));
    assert_eq!(
        excerpt.path.as_deref(),
        Some("~/data/rspinyin/logs/rspinyin.log")
    );
    assert_eq!(excerpt.total_lines, 1);
    assert_eq!(excerpt.lines, vec!["one line"]);
}

#[test]
fn test_log_excerpt_empty_holds_no_line_and_no_path() {
    let excerpt = LogExcerpt::empty();
    assert!(excerpt.path.is_none());
    assert_eq!(excerpt.total_lines, 0);
    assert!(excerpt.lines.is_empty());
}

#[test]
fn test_trace_record_takes_its_diffs_from_the_failed_assertions() {
    let record = evidence_with_assertions("core", "TC-CORE-01", CaseStatus::Fail);
    let trace = TraceRecord::of(&record);

    assert_eq!(trace.tc, "TC-CORE-01");
    assert_eq!(trace.module, "core");
    assert_eq!(trace.status, CaseStatus::Fail);
    assert_eq!(trace.diffs.len(), 1, "only the assertion that did not hold");
    assert_eq!(trace.diffs[0].name, "first_candidate");
    assert_eq!(trace.diffs[0].expected.as_str(), "<redacted:len=2>");
    assert_eq!(trace.diffs[0].actual.as_str(), "<redacted:len=3>");
    assert!(trace.defects.is_empty());
    assert!(trace.stack.is_none());
    assert_eq!(trace.logs, LogExcerpt::empty());
}

#[test]
fn test_trace_json_holds_the_diff_the_defect_and_the_log() {
    let record = evidence_with_assertions("ui", "TC-UI-01", CaseStatus::Fail);
    let mut trace = trace_of(&record);
    trace.logs = LogExcerpt {
        path: Some(String::from("~/data/rspinyin/logs/rspinyin.log")),
        total_lines: 3,
        lines: vec![String::from("one"), String::from("two")],
    };

    let text = trace.to_json().expect("the trace is JSON");
    let document: Value = serde_json::from_str(&text).expect("the document is JSON");
    assert_eq!(document["tc"], "TC-UI-01");
    assert_eq!(document["diffs"][0]["name"], "first_candidate");
    assert_eq!(document["defects"][0]["item"], "corner_radius");
    assert_eq!(document["defects"][0]["rect"]["w"], 8);
    assert_eq!(document["defects"][0]["expected"], "#1e1e1e");
    assert_eq!(
        document["stack"],
        "the frame was not the one the case posted"
    );
    assert_eq!(document["logs"]["total_lines"], 3);
    assert_eq!(document["logs"]["lines"][1], "two");
    assert!(!text.contains('你'), "{text}");
}

#[test]
fn test_run_dir_creates_a_private_tree_named_for_the_instant() {
    let scratch = Scratch::new("run-create");
    let results = scratch.results();
    let run = RunDir::create(&results, started()).expect("creating the run");

    assert_eq!(run.name(), "run-20231114-221320");
    assert_eq!(run.root(), results.join("runs").join(run.name()));
    assert!(run.root().is_dir());
    assert_eq!(mode_of(&results), 0o700);
    assert_eq!(mode_of(&results.join("runs")), 0o700);
    assert_eq!(mode_of(run.root()), 0o700);
}

#[test]
fn test_run_dir_refuses_a_second_run_in_the_same_second() {
    let scratch = Scratch::new("run-exists");
    let results = scratch.results();
    let first = RunDir::create(&results, started()).expect("creating the run");

    match RunDir::create(&results, started()) {
        Err(EvidenceError::RunExists { path }) => assert_eq!(path, first.root()),
        other => panic!("a second run in the same second must be refused, got {other:?}"),
    }

    // The next second is a new run, and the first run's tree is left where it is.
    let later = RunDir::create(&results, Utc::from_unix_seconds(1_700_000_001))
        .expect("creating the later run");
    assert_ne!(later.root(), first.root());
    assert!(first.root().is_dir());
}

#[test]
fn test_run_dir_refuses_a_legacy_results_root() {
    let scratch = Scratch::new("run-legacy");
    let legacy = scratch.results().join("screenshots");

    match RunDir::create(&legacy, started()) {
        Err(EvidenceError::LegacyLayout { found, .. }) => assert_eq!(found, "screenshots"),
        other => panic!("a legacy results root must be refused, got {other:?}"),
    }
    assert!(!legacy.exists(), "a refused root is not created");
}

#[test]
fn test_snapshot_path_lands_in_the_case_directory() {
    let scratch = Scratch::new("snapshot-path");
    let run = RunDir::create(&scratch.results(), started()).expect("creating the run");

    let path = run
        .snapshot_path("core", "TC-CORE-01", 1, "Default")
        .expect("a usable path");
    assert_eq!(
        path,
        run.root()
            .join("core")
            .join("TC-CORE-01")
            .join("01_default.png")
    );

    let later = run
        .snapshot_path("core", "TC-CORE-01", 12, "hover cell")
        .expect("a usable path");
    assert!(
        later.to_string_lossy().ends_with("12_hover-cell.png"),
        "the step is padded and the state slugified: {later:?}"
    );

    // A burst of one step names its frames in the state tag, which is how `01_default-1.png`
    // and `01_default-2.png` are reached without a second naming rule: the tag is a slug, and
    // a digit after a separator survives it.
    for frame in 1..=2 {
        let burst = run
            .snapshot_path("core", "TC-CORE-01", 1, &format!("default-{frame}"))
            .expect("a usable path");
        let name = format!("01_default-{frame}.png");
        assert!(
            burst.to_string_lossy().ends_with(&name),
            "the burst frame is named {name}: {burst:?}"
        );
    }

    let escaped = run.snapshot_path("..", "TC-CORE-01", 1, "default");
    assert!(
        matches!(escaped, Err(EvidenceError::Capture(_))),
        "a segment that steps out of the run is refused, got {escaped:?}"
    );
}

#[test]
fn test_write_case_refuses_a_case_segment_a_path_cannot_carry() {
    let scratch = Scratch::new("write-bad-segment");
    let run = RunDir::create(&scratch.results(), started()).expect("creating the run");

    // Every segment a case could name that would place its directory somewhere other than the
    // module it belongs to, or under a name a listing cannot be read from.
    let refused = [
        ("", "TC-CORE-01"),
        ("core", ""),
        (".", "TC-CORE-01"),
        ("core", ".."),
        ("core", "TC/CORE/01"),
        ("core", "TC CORE 01"),
        ("core", "TC-CORE-01\n"),
    ];
    for (module, tc) in refused {
        let record = evidence(module, tc, CaseStatus::Pass);
        let written = run.write_case(&record, None, None);
        assert!(
            matches!(written, Err(EvidenceError::Capture(_))),
            "the segment {module:?}/{tc:?} must be refused, got {written:?}"
        );
    }

    // A segment longer than a name may be is refused; one exactly at the ceiling is not.
    let over = "T".repeat(65);
    let record = evidence("core", &over, CaseStatus::Pass);
    assert!(
        matches!(
            run.write_case(&record, None, None),
            Err(EvidenceError::Capture(_))
        ),
        "a 65 byte case id is past the ceiling a segment may take"
    );
    assert_eq!(
        fs::read_dir(run.root())
            .expect("the run directory is readable")
            .count(),
        0,
        "a refused case leaves no directory behind"
    );

    let at_ceiling = "T".repeat(64);
    let record = evidence("core", &at_ceiling, CaseStatus::Pass);
    let bundle = run
        .write_case(&record, None, None)
        .expect("a 64 byte segment is still a name");
    assert_eq!(bundle.dir, run.root().join("core").join(&at_ceiling));
}

#[test]
fn test_write_case_of_a_passing_case_leaves_no_trace() {
    let scratch = Scratch::new("write-pass");
    let run = RunDir::create(&scratch.results(), started()).expect("creating the run");
    let record = evidence("core", "TC-CORE-01", CaseStatus::Pass);

    let bundle = run
        .write_case(&record, None, None)
        .expect("writing the bundle");

    assert_eq!(bundle.dir, run.root().join("core").join("TC-CORE-01"));
    assert!(bundle.assertions.is_file());
    assert!(bundle.trace.is_none());
    assert!(bundle.heal.is_none());
    assert!(
        !bundle.dir.join(TRACE_FILE).exists(),
        "a passing case leaves no trace to skip"
    );
    assert_eq!(read_json(&bundle.assertions)["status"], "pass");
    assert_eq!(mode_of(&bundle.dir), 0o700);
    assert_eq!(mode_of(&bundle.assertions), 0o600);
}

#[test]
fn test_write_case_of_a_failed_case_writes_a_trace() {
    let scratch = Scratch::new("write-fail");
    let run = RunDir::create(&scratch.results(), started()).expect("creating the run");
    let record = evidence_with_assertions("ui", "TC-UI-01", CaseStatus::Fail);
    let trace = trace_of(&record);

    let bundle = run
        .write_case(&record, Some(&trace), Some("--- a\n+++ b\n"))
        .expect("writing the bundle");

    // A case that failed leaves its whole bundle: the trace is what a reader opens first, and
    // the verdict beside it is what says which assertions the case actually made. A failure
    // that produced only a trace would be a failure nobody could reproduce.
    let verdict = read_json(&bundle.assertions);
    assert_eq!(verdict["status"], "fail");
    assert_eq!(verdict["assertions"][0]["ok"], true);
    assert_eq!(verdict["assertions"][1]["name"], "first_candidate");
    assert_eq!(verdict["assertions"][1]["expected"], "<redacted:len=2>");
    assert_eq!(verdict["assertions"][1]["actual"], "<redacted:len=3>");
    assert_eq!(mode_of(&bundle.assertions), 0o600);

    let trace_path = bundle.trace.as_ref().expect("a failed case leaves a trace");
    assert!(trace_path.is_file());
    assert_eq!(mode_of(trace_path), 0o600);
    let document = read_json(trace_path);
    assert_eq!(document["status"], "fail");
    assert_eq!(document["diffs"][0]["name"], "first_candidate");
    assert_eq!(document["diffs"][0]["expected"], "<redacted:len=2>");
    assert_eq!(document["diffs"][0]["actual"], "<redacted:len=3>");
    assert_eq!(document["defects"][0]["rect"]["x"], 4);
    assert_eq!(document["defects"][0]["image"], "01_default.png");
    assert_eq!(document["defects"][0]["measured"], "#3c3c3c");

    let heal = bundle.heal.as_ref().expect("the heal patch was written");
    assert_eq!(mode_of(heal), 0o600);
    assert_eq!(
        fs::read_to_string(heal).expect("the patch is text"),
        "--- a\n+++ b\n"
    );
}

#[test]
fn test_write_case_of_a_flawed_case_writes_a_trace_with_no_diff() {
    let scratch = Scratch::new("write-flawed");
    let run = RunDir::create(&scratch.results(), started()).expect("creating the run");
    let record = evidence("ui", "TC-UI-01", CaseStatus::Flawed);
    let trace = trace_of(&record);

    let bundle = run
        .write_case(&record, Some(&trace), None)
        .expect("writing the bundle");

    let document = read_json(bundle.trace.as_ref().expect("a flawed case leaves a trace"));
    assert_eq!(document["status"], "flawed");
    assert_eq!(document["diffs"].as_array().map(Vec::len), Some(0));
    assert_eq!(
        document["stack"],
        "the frame was not the one the case posted"
    );
}

#[test]
fn test_write_case_replaces_a_previous_bundle_rather_than_adding_to_it() {
    let scratch = Scratch::new("write-rewrite");
    let run = RunDir::create(&scratch.results(), started()).expect("creating the run");
    let case_dir = run.root().join("core").join("TC-CORE-01");

    let failing = evidence_with_assertions("core", "TC-CORE-01", CaseStatus::Fail);
    let trace = trace_of(&failing);
    let first = run
        .write_case(&failing, Some(&trace), Some("--- a\n+++ b\n--- c\n+++ d\n"))
        .expect("writing the first bundle");
    assert!(first.trace.is_some() && first.heal.is_some());
    let before = fs::read_to_string(&first.assertions).expect("the first verdict is text");
    assert!(before.contains("\"fail\""), "{before}");

    // The same case again -- a retry -- this time passing. The second write replaces the
    // first instead of joining it, and the shorter document leaves no tail of the longer one.
    let passing = evidence("core", "TC-CORE-01", CaseStatus::Pass);
    let second = run
        .write_case(&passing, None, None)
        .expect("writing the second bundle");

    assert_eq!(second.dir, case_dir);
    assert!(second.trace.is_none() && second.heal.is_none());
    assert!(
        !case_dir.join(TRACE_FILE).exists(),
        "the first write's trace cannot outlive the verdict it belonged to"
    );
    assert!(
        !case_dir.join(HEAL_FILE).exists(),
        "the first write's patch cannot outlive the verdict it belonged to"
    );
    let after = fs::read_to_string(&second.assertions).expect("the second verdict is text");
    assert!(
        after.len() < before.len(),
        "the second document is the shorter of the two"
    );
    assert!(
        !after.contains("\"fail\""),
        "no tail of the first document is left behind: {after}"
    );
    assert_eq!(read_json(&second.assertions)["status"], "pass");
    assert_eq!(
        fs::read_dir(&case_dir)
            .expect("the case directory is readable")
            .count(),
        1,
        "one case holds one bundle: the verdict, and nothing the first write left"
    );
}

#[test]
fn test_write_case_narrows_a_case_directory_that_was_wider() {
    let scratch = Scratch::new("write-narrow-dir");
    let run = RunDir::create(&scratch.results(), started()).expect("creating the run");
    let module_dir = run.root().join("core");
    let case_dir = module_dir.join("TC-CORE-01");
    fs::create_dir_all(&case_dir).expect("placing the case directory");
    fs::set_permissions(&case_dir, fs::Permissions::from_mode(0o755)).expect("widening the mode");

    let record = evidence("core", "TC-CORE-01", CaseStatus::Pass);
    let bundle = run
        .write_case(&record, None, None)
        .expect("writing the bundle");

    assert_eq!(bundle.dir, case_dir);
    assert_eq!(
        mode_of(&case_dir),
        0o700,
        "a directory this tree adopts is narrowed, not left reachable by another account"
    );
    assert_eq!(mode_of(&bundle.assertions), 0o600);
}

#[test]
fn test_write_case_refuses_a_trace_for_a_passing_case() {
    let scratch = Scratch::new("write-unexpected");
    let run = RunDir::create(&scratch.results(), started()).expect("creating the run");
    let record = evidence("core", "TC-CORE-01", CaseStatus::Pass);
    let trace = TraceRecord::of(&record);

    match run.write_case(&record, Some(&trace), None) {
        Err(EvidenceError::UnexpectedTrace { tc, module }) => {
            assert_eq!((module.as_str(), tc.as_str()), ("core", "TC-CORE-01"));
        }
        other => panic!("a passing case must not carry a trace, got {other:?}"),
    }
    assert!(
        !run.root().join("core").join("TC-CORE-01").exists(),
        "the refusal comes before anything is written"
    );
}

#[test]
fn test_write_case_refuses_a_failed_case_without_a_trace() {
    let scratch = Scratch::new("write-missing");
    let run = RunDir::create(&scratch.results(), started()).expect("creating the run");
    let record = evidence("core", "TC-CORE-01", CaseStatus::Fail);

    match run.write_case(&record, None, None) {
        Err(EvidenceError::MissingTrace { tc, .. }) => assert_eq!(tc, "TC-CORE-01"),
        other => panic!("a failed case must carry a trace, got {other:?}"),
    }
}

#[test]
fn test_a_bundle_that_cannot_be_written_becomes_an_evidence_gap() {
    let scratch = Scratch::new("write-gap");
    let results = scratch.results();
    let run = RunDir::create(&results, started()).expect("creating the run");
    // A file where the case's module directory has to go: no directory can be made out of it,
    // which is the failure a full disk produces too.
    fs::write(run.root().join("core"), b"not a directory").expect("blocking the module directory");

    let record = evidence("core", "TC-CORE-01", CaseStatus::Pass);
    let bundle = run.write_case(&record, None, None);
    assert!(bundle.is_err(), "the write fails loudly: {bundle:?}");

    let mut journal = RunJournal::new(started());
    journal.record(&record, bundle);
    let rows = journal.rows();
    assert_eq!(rows.len(), 1, "the case is still a row of the index");
    assert_eq!(rows[0].status, CaseStatus::Flawed);
    assert!(!rows[0].is_complete());

    let gap = rows[0].gap.as_deref().expect("the row carries the reason");
    assert!(gap.contains("the evidence could not be written"), "{gap}");
    assert!(
        !gap.contains(&scratch.path.to_string_lossy().to_string()),
        "the index is evidence and carries no path: {gap}"
    );

    let summary = journal.summary();
    assert_eq!(summary.cases(), 1);
    assert_eq!(summary.flawed, 1);
    assert_eq!(summary.evidence_gaps, 1);
    assert_eq!(summary.verdict(), BatchVerdict::Flawed);

    let text = journal.render_index(&run);
    assert!(text.contains("- evidence gaps: 1"), "{text}");
    assert!(text.contains("- verdict: flawed"), "{text}");
    assert!(
        text.contains("gap: the evidence could not be written"),
        "{text}"
    );
}

#[test]
fn test_a_lost_bundle_does_not_stop_the_batch() {
    let scratch = Scratch::new("batch-gap");
    let results = scratch.results();
    let run = RunDir::create(&results, started()).expect("creating the run");
    // A file where the module directory has to go, which is what a full disk produces too.
    fs::write(run.root().join("core"), b"not a directory").expect("blocking the module directory");

    let mut journal = RunJournal::new(started());
    let lost = evidence_with_assertions("core", "TC-CORE-01", CaseStatus::Fail);
    let bundle = run.write_case(&lost, Some(&trace_of(&lost)), None);
    assert!(
        bundle.is_err(),
        "the first case's evidence is lost: {bundle:?}"
    );
    journal.record(&lost, bundle);

    // The rest of the batch runs: one lost bundle is a gap in the evidence, not the end of
    // the run, and the cases after it are judged and archived as usual.
    let kept = evidence("ui", "TC-UI-01", CaseStatus::Pass);
    let bundle = run
        .write_case(&kept, None, None)
        .expect("the second bundle is written");
    journal.record(&kept, Ok(bundle));

    let rows = journal.rows();
    assert_eq!(rows.len(), 2, "both cases are rows of the index");
    assert_eq!(rows[0].status, CaseStatus::Flawed);
    assert_eq!(
        rows[0].failed, 1,
        "the assertion that did not hold is still counted beside the gap"
    );
    assert!(rows[0].gap.is_some());
    assert_eq!(rows[1].status, CaseStatus::Pass);
    assert!(rows[1].is_complete());

    let summary = journal.summary();
    assert_eq!(summary.cases(), 2);
    assert_eq!((summary.passed, summary.failed, summary.flawed), (1, 0, 1));
    assert_eq!(summary.evidence_gaps, 1);
    assert_eq!(summary.verdict(), BatchVerdict::Flawed);

    let text = journal.render_index(&run);
    assert!(
        text.contains("- cases: 2 (pass 1, fail 0, flawed 1)"),
        "{text}"
    );
    assert!(text.contains("| ui | TC-UI-01 | pass |"), "{text}");
    assert!(text.contains("| core | TC-CORE-01 | flawed |"), "{text}");
    assert!(text.contains("| 2 | 1 | no | 0 | gap: "), "{text}");
}

#[test]
fn test_write_index_narrows_a_wider_file_and_replaces_its_content() {
    let scratch = Scratch::new("write-narrow");
    let run = RunDir::create(&scratch.results(), started()).expect("creating the run");
    let path = run.root().join(RUN_INDEX_FILE);
    fs::write(&path, "a document much longer than the one that follows it").expect("an index");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("widening the mode");

    let written = run.write_index("# Run\n").expect("writing the index");
    assert_eq!(written, path);
    assert_eq!(mode_of(&path), 0o600);
    assert_eq!(
        fs::read_to_string(&path).expect("the index is text"),
        "# Run\n",
        "the shorter document leaves no tail of its predecessor behind"
    );
}

#[test]
fn test_run_journal_renders_the_case_table_and_writes_it() {
    let scratch = Scratch::new("journal-index");
    let results = scratch.results();
    let run = RunDir::create(&results, started()).expect("creating the run");

    let mut journal = RunJournal::new(started());
    let passing = evidence("core", "TC-CORE-01", CaseStatus::Pass);
    let bundle = run
        .write_case(&passing, None, None)
        .expect("writing the passing bundle");
    journal.record(&passing, Ok(bundle));

    let mut failing = evidence_with_assertions("ui", "TC-UI-01", CaseStatus::Fail);
    failing.healed.push(HealRecord {
        old: String::from("CellHeight"),
        new: String::from("CandidateCellHeight"),
        basis: String::from("features.md 3.1"),
    });
    let trace = trace_of(&failing);
    let bundle = run
        .write_case(&failing, Some(&trace), None)
        .expect("writing the failing bundle");
    journal.record(&failing, Ok(bundle));

    let text = journal.render_index(&run);
    assert!(text.contains("# Run run-20231114-221320"), "{text}");
    assert!(text.contains("- started: 2023-11-14T22:13:20Z"), "{text}");
    assert!(
        text.contains("- cases: 2 (pass 1, fail 1, flawed 0)"),
        "{text}"
    );
    assert!(text.contains("- evidence gaps: 0"), "{text}");
    assert!(text.contains("- healed: 1"), "{text}");
    assert!(text.contains("- verdict: fail"), "{text}");
    assert!(
        text.contains("| core | TC-CORE-01 | pass | 12 ms | 0 | 0 | no | 0 | complete |"),
        "{text}"
    );
    assert!(
        text.contains("| ui | TC-UI-01 | fail | 12 ms | 2 | 1 | yes | 1 | complete |"),
        "{text}"
    );

    let path = journal.write_index(&run).expect("writing the run's index");
    assert_eq!(path, run.root().join(RUN_INDEX_FILE));
    assert_eq!(fs::read_to_string(&path).expect("the index is text"), text);
    assert_eq!(mode_of(&path), 0o600);

    let summary = journal.summary();
    assert_eq!(summary.cases(), 2);
    assert_eq!((summary.passed, summary.failed, summary.flawed), (1, 1, 0));
    assert_eq!(summary.evidence_gaps, 0);
    assert_eq!(summary.healed, 1);
    assert_eq!(summary.verdict(), BatchVerdict::Fail);
}

#[test]
fn test_publish_writes_the_run_index_and_the_cross_run_pointer() {
    let scratch = Scratch::new("journal-publish");
    let results = scratch.results();
    let run = RunDir::create(&results, started()).expect("creating the run");

    let mut journal = RunJournal::new(started());
    let passing = evidence("core", "TC-CORE-01", CaseStatus::Pass);
    let bundle = run
        .write_case(&passing, None, None)
        .expect("writing the bundle");
    journal.record(&passing, Ok(bundle));

    journal.publish(&run, &results).expect("publishing the run");

    assert!(run.root().join(RUN_INDEX_FILE).is_file());
    let index = RunIndex::open(&results).expect("opening the cross-run index");
    assert_eq!(index.latest_run(), Some(run.name()));
    let entry = index.runs().first().expect("the run is in the index");
    assert_eq!(entry.run, run.name());
    assert_eq!(entry.path, format!("results/runs/{}", run.name()));
    assert_eq!(entry.started_at, "2023-11-14T22:13:20Z");
    assert_eq!(entry.verdict, BatchVerdict::Pass);
    assert_eq!((entry.cases, entry.passed, entry.failed), (1, 1, 0));
    assert_eq!(entry.evidence_gaps, 0);
}

#[test]
fn test_batch_summary_counts_and_ranks_the_verdicts() {
    let empty = BatchSummary::default();
    assert_eq!(empty.cases(), 0);
    assert_eq!(empty.verdict(), BatchVerdict::Pass);

    let clean = BatchSummary {
        passed: 3,
        ..BatchSummary::default()
    };
    assert_eq!(clean.cases(), 3);
    assert_eq!(clean.verdict(), BatchVerdict::Pass);

    let gapped = BatchSummary {
        passed: 2,
        flawed: 1,
        evidence_gaps: 1,
        ..BatchSummary::default()
    };
    assert_eq!(gapped.cases(), 3);
    assert_eq!(gapped.verdict(), BatchVerdict::Flawed);

    // A failure outranks a gap, and the gap is still counted beside it.
    let mixed = BatchSummary {
        passed: 1,
        failed: 1,
        flawed: 1,
        evidence_gaps: 1,
        healed: 2,
    };
    assert_eq!(mixed.cases(), 3);
    assert_eq!(mixed.verdict(), BatchVerdict::Fail);
    assert_eq!(mixed.healed, 2);

    assert_eq!(BatchVerdict::Pass.label(), "pass");
    assert_eq!(BatchVerdict::Fail.label(), "fail");
    assert_eq!(BatchVerdict::Flawed.label(), "flawed");
}

#[test]
fn test_case_row_is_complete_only_without_a_gap() {
    let row = CaseRow {
        module: String::from("core"),
        tc: String::from("TC-CORE-01"),
        status: CaseStatus::Pass,
        duration_ms: 5,
        assertions: 1,
        failed: 0,
        trace: false,
        healed: 0,
        gap: None,
    };
    assert!(row.is_complete());

    let gapped = CaseRow {
        gap: Some(String::from("the evidence could not be written")),
        ..row
    };
    assert!(!gapped.is_complete());
}

#[test]
fn test_evidence_error_reasons_name_what_went_wrong_and_no_path() {
    let io = EvidenceError::Io {
        path: PathBuf::from("/home/someone/results/runs/run-1/core"),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such file or directory"),
    };
    let reason = io.reason();
    assert!(reason.contains("no such file or directory"), "{reason}");
    assert!(
        !reason.contains("someone"),
        "a reason names no path: {reason}"
    );

    assert!(
        EvidenceError::RunExists {
            path: PathBuf::from("/home/someone/results/runs/run-1"),
        }
        .reason()
        .contains("already in the results tree")
    );
    assert!(
        EvidenceError::LegacyLayout {
            path: PathBuf::from("results/traces"),
            found: String::from("traces"),
        }
        .reason()
        .contains("legacy `traces` layout")
    );
    assert!(
        EvidenceError::MissingTrace {
            module: String::from("core"),
            tc: String::from("TC-CORE-01"),
        }
        .reason()
        .contains("left no failure trace")
    );
    assert!(
        EvidenceError::UnexpectedTrace {
            module: String::from("core"),
            tc: String::from("TC-CORE-01"),
        }
        .reason()
        .contains("passing case")
    );
    assert!(
        EvidenceError::Json {
            document: String::from("trace.json"),
            detail: String::from("a float was not finite"),
        }
        .reason()
        .contains("trace.json")
    );
    assert!(
        EvidenceError::Index {
            path: PathBuf::from("/home/someone/results/index.json"),
            detail: String::from("it is written in format 99"),
        }
        .reason()
        .contains("format 99")
    );
}
