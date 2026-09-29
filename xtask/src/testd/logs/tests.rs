//! Tests for [`super`].
//!
//! They cover the channel rather than the plugin: what a drain hands out and what it holds back,
//! what happens to a reader when the writer rolls the file out from under it, the privacy rules,
//! the error-code rule, and the properties the acceptance criteria name -- a whitelist extracted
//! from the sources rather than copied, and a finding that points at a line without repeating it.
//! [`permissions`] holds the third assertion, on the mode every path of the family must carry,
//! and the helpers below are what it builds its fixtures with.
//!
//! Every test that needs a file builds its own scratch directory under the system temporary
//! directory, so no test reads the operator's log, needs a display server, or leaves anything
//! behind. The two that read the repository's own sources read them from `$CARGO_MANIFEST_DIR`,
//! which is where the build put them.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use super::codes::is_code_shape;
use super::{
    Assertion, Cursor, Denial, ERROR_SOURCE, EVIDENCE_LINES, EnumCodes, ErrorCodeIndex,
    ErrorCodeSource, Finding, Identity, LogError, LogFamily, LogTap, MAX_FINDINGS, SPEC_SOURCE,
    spec_enums,
};
use crate::testd::sandbox::Sandbox;

mod permissions;

/// A scratch directory that removes itself when the test ends.
#[derive(Debug)]
struct Scratch {
    /// The directory this guard owns.
    path: PathBuf,
}

impl Scratch {
    /// Creates `<temp>/rspinyin-logs-<tag>-<pid>`, empty.
    ///
    /// The process id keeps two test binaries apart and the tag keeps two tests in one binary
    /// apart, which is what makes the root safe to remove wholesale.
    fn new(tag: &str) -> Self {
        let name = format!("rspinyin-logs-{tag}-{}", std::process::id());
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("creating the scratch directory");
        Self { path }
    }

    /// The active log file, inside a log directory of its own.
    ///
    /// The directory deliberately does not exist yet: every test that writes goes through the
    /// creation path, which is the one the plugin's first run takes too.
    fn log(&self) -> PathBuf {
        self.path.join("logs").join("rspinyin.log")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// The rolled sibling of `path` at `index`, named the way the writer names it.
fn sibling(path: &Path, index: usize) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{index}"));
    PathBuf::from(name)
}

/// Creates the log's directory, so that a write through a plain `fs` call lands.
fn prepare(path: &Path) {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory).expect("creating the log directory");
    }
}

/// Creates the log's directory and appends one complete line to `path`.
fn append(path: &Path, line: &str) {
    prepare(path);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("opening the log for appending");
    file.write_all(line.as_bytes()).expect("writing the line");
    file.write_all(b"\n").expect("writing the terminator");
}

/// Appends raw bytes, without a terminator.
fn append_raw(path: &Path, text: &str) {
    prepare(path);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("opening the log for appending");
    file.write_all(text.as_bytes()).expect("writing the text");
}

/// The lines as borrowed slices, so an assertion reads as the text a case asserts on.
fn borrowed(lines: &[String]) -> Vec<&str> {
    lines.iter().map(String::as_str).collect()
}

/// The findings of a failed assertion.
fn findings(error: &LogError) -> (&[Finding], usize) {
    match error {
        LogError::Failed {
            findings,
            suppressed,
            ..
        } => (findings, *suppressed),
        other => panic!("expected a failed assertion, got {other}"),
    }
}

/// A set of codes, for a readable assertion.
fn code_set(codes: &[&str]) -> BTreeSet<String> {
    codes.iter().map(|code| (*code).to_owned()).collect()
}

/// A set of declaration sources, for a readable assertion.
fn source_set(sources: &[ErrorCodeSource]) -> BTreeSet<ErrorCodeSource> {
    sources.iter().copied().collect()
}

/// Rolls `path` over the way the writer does: the active file becomes `.1`, and a fresh active
/// file takes its name.
fn roll(path: &Path) {
    fs::rename(path, sibling(path, 1)).expect("rolling the log over");
    fs::write(path, "").expect("opening a fresh active file");
}

/// A miniature of `features.md` 2.2.4: the section, its fenced copy of an enum, and one
/// diagnostic-code table.
const SPEC_FIXTURE: &str = r#"#### 2.2.4 error codes

```rust
#[derive(Debug, thiserror::Error)]
pub enum ImeError {
    #[error("dict/corrupt: {path}")]
    DictCorrupt { path: PathBuf },
}
```

| code | raiser |
|---|---|
| `ui/select/timeout` | the channel |

### 2.3 the next section
"#;

/// A miniature of `crates/ime-types/src/error.rs`.
const ERRORS_FIXTURE: &str = r#"pub enum ImeError {
    #[error("dict/corrupt: {path}")]
    DictCorrupt { path: PathBuf },
}

pub enum DictError {
    #[error("magic mismatch")]
    MagicMismatch,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
"#;

// ---------------------------------------------------------------------------
// Following the file
// ---------------------------------------------------------------------------

#[test]
fn test_drain_returns_only_what_appeared_since_the_last_call() {
    let scratch = Scratch::new("drain-incremental");
    let path = scratch.log();
    append(&path, "first");
    let mut tap = LogTap::at(&path);

    assert_eq!(borrowed(&tap.drain().expect("the log reads")), ["first"]);
    assert_eq!(
        tap.drain().expect("the log reads"),
        Vec::<String>::new(),
        "a second look finds nothing new"
    );
    append(&path, "second");
    assert_eq!(borrowed(&tap.drain().expect("the log reads")), ["second"]);
    assert_eq!(
        borrowed(tap.drained()),
        ["first", "second"],
        "the tap keeps everything it handed out, for the evidence archive"
    );
}

#[test]
fn test_drain_hands_out_a_line_only_once_it_is_complete() {
    let scratch = Scratch::new("drain-partial");
    let path = scratch.log();
    // A writer caught halfway through a line leaves a tail without its terminator.
    prepare(&path);
    fs::write(&path, "complete\nhalf").expect("seeding the log");
    let mut tap = LogTap::at(&path);

    assert_eq!(borrowed(&tap.drain().expect("the log reads")), ["complete"]);
    append_raw(&path, " a line\n");
    assert_eq!(
        borrowed(&tap.drain().expect("the log reads")),
        ["half a line"],
        "the tail is handed out once the writer has finished it"
    );
}

#[test]
fn test_drain_follows_a_rolled_file_to_the_new_one() {
    let scratch = Scratch::new("drain-roll");
    let path = scratch.log();
    append(&path, "before the roll");
    let mut tap = LogTap::at(&path);
    assert_eq!(
        borrowed(&tap.drain().expect("the log reads")),
        ["before the roll"]
    );

    roll(&path);
    append(&path, "after the roll");
    assert_eq!(
        borrowed(&tap.drain().expect("the log reads")),
        ["after the roll"],
        "the content that rolled away is not handed out a second time"
    );
    assert_eq!(tap.restarts(), 1, "the roll is visible as a restart");
}

#[test]
fn test_drain_reports_a_restart_when_the_writer_truncates_the_active_file() {
    let scratch = Scratch::new("drain-truncate");
    let path = scratch.log();
    append(&path, "first run");
    let mut tap = LogTap::at(&path);
    assert_eq!(
        borrowed(&tap.drain().expect("the log reads")),
        ["first run"]
    );

    // A writer configured to keep no history truncates the active file where it stands, which
    // leaves the inode alone and the offset past the end of what is there now.
    fs::write(&path, "").expect("truncating the active file");
    append(&path, "second run");
    assert_eq!(
        borrowed(&tap.drain().expect("the log reads")),
        ["second run"]
    );
    assert_eq!(tap.restarts(), 1);
}

#[test]
fn test_drain_treats_a_log_that_does_not_exist_yet_as_empty() {
    let scratch = Scratch::new("drain-absent");
    let path = scratch.log();
    let mut tap = LogTap::at(&path);

    assert_eq!(
        tap.drain().expect("an absent log is not an error"),
        Vec::<String>::new()
    );
    assert_eq!(tap.restarts(), 0);
    append(&path, "the plugin started");
    assert_eq!(
        borrowed(&tap.drain().expect("the log reads")),
        ["the plugin started"],
        "a tap opened before the plugin wrote anything still sees its first line"
    );
}

#[test]
fn test_family_lists_the_active_file_before_its_rolled_siblings() {
    let scratch = Scratch::new("family-order");
    let path = scratch.log();
    append(&path, "active");
    fs::write(sibling(&path, 1), "newer\n").expect("writing .1");
    fs::write(sibling(&path, 2), "older\n").expect("writing .2");
    // Neither of these is a name the writer produces.
    fs::write(sibling(&path, 0), "not a sibling\n").expect("writing .0");
    fs::write(scratch.path.join("logs").join("rspinyin.log.old"), "x\n").expect("writing a stray");

    let family = LogFamily::new(&path);
    let files: Vec<String> = family
        .files()
        .expect("the directory lists")
        .iter()
        .map(|file| {
            file.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(files, ["rspinyin.log", "rspinyin.log.1", "rspinyin.log.2"]);
    assert_eq!(family.read().expect("the family reads").len(), 3);
}

#[test]
fn test_cursor_remembers_the_file_it_read_and_how_far_it_got() {
    let scratch = Scratch::new("cursor-state");
    let path = scratch.log();
    append(&path, "one");
    let mut cursor = Cursor::new();

    // A cursor that has read nothing has no file to name and no offset to report.
    assert_eq!(cursor.identity(), None);
    assert_eq!(cursor.offset(), 0);

    let (lines, restarted) = cursor.read_new(&path).expect("the log reads");
    assert_eq!(borrowed(&lines), ["one"]);
    assert!(!restarted, "the first read of a file is not a restart");
    // `one` and its terminator are the four bytes the cursor has handed out.
    assert_eq!(cursor.offset(), 4);
    let identity = Identity::of(&path).expect("the log reads");
    assert_eq!(cursor.identity(), identity);

    append(&path, "two");
    let (lines, restarted) = cursor.read_new(&path).expect("the log reads");
    assert_eq!(borrowed(&lines), ["two"]);
    assert!(!restarted);
    assert_eq!(cursor.offset(), 8);
}

#[test]
fn test_identity_of_tells_two_files_apart_and_an_absent_one_from_both() {
    let scratch = Scratch::new("identity");
    let path = scratch.log();
    append(&path, "one");
    let rolled = sibling(&path, 1);
    append(&rolled, "one");

    let active = Identity::of(&path).expect("the log reads");
    let rolled_identity = Identity::of(&rolled).expect("the log reads");
    let missing = scratch.path.join("absent.log");
    let absent = Identity::of(&missing).expect("the metadata reads");

    // The same file answers with the same identity, which is what makes the identity the one
    // thing a cursor can remember across a roll.
    assert_eq!(active, Identity::of(&path).expect("the log reads"));
    assert!(active.is_some() && rolled_identity.is_some());
    assert_ne!(active, rolled_identity, "two files are two identities");
    assert_eq!(absent, None, "an absent path is not an error");
}

#[test]
fn test_drain_reports_a_restart_when_the_active_file_shrinks_under_the_cursor() {
    let scratch = Scratch::new("drain-shrink");
    let path = scratch.log();
    append(&path, "the first run wrote a line");
    append(&path, "and then a second one");
    let mut tap = LogTap::at(&path);
    assert_eq!(tap.drain().expect("the log reads").len(), 2);

    // A writer that keeps no history starts the active file over where it stands. The inode
    // survives the rewrite and what is there now is shorter than the offset the cursor holds,
    // which is the shape neither the identity nor the remembered tail can see.
    fs::write(&path, "second run\n").expect("rewriting the active file");
    assert_eq!(
        borrowed(&tap.drain().expect("the log reads")),
        ["second run"]
    );
    assert_eq!(tap.restarts(), 1, "the rewrite is visible as a restart");
}

#[test]
fn test_drain_follows_a_second_roll_that_shifts_the_history_up() {
    let scratch = Scratch::new("drain-roll-twice");
    let path = scratch.log();
    append(&path, "first");
    let mut tap = LogTap::at(&path);
    assert_eq!(borrowed(&tap.drain().expect("the log reads")), ["first"]);

    roll(&path);
    append(&path, "second");
    assert_eq!(borrowed(&tap.drain().expect("the log reads")), ["second"]);

    // What the writer does before its second roll: the history shifts up by one, so what `.1`
    // held becomes `.2` and the fresh roll takes `.1`.
    fs::rename(sibling(&path, 1), sibling(&path, 2)).expect("shifting the history up");
    roll(&path);
    append(&path, "third");
    assert_eq!(
        borrowed(&tap.drain().expect("the log reads")),
        ["third"],
        "the second roll is followed exactly as the first was"
    );
    assert_eq!(tap.restarts(), 2, "each roll is visible as a restart");

    let files: Vec<String> = tap
        .family()
        .files()
        .expect("the directory lists")
        .iter()
        .map(|file| {
            file.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(files, ["rspinyin.log", "rspinyin.log.1", "rspinyin.log.2"]);
}

#[test]
fn test_drain_keeps_only_the_tail_of_what_it_handed_out() {
    let scratch = Scratch::new("evidence-cap");
    let path = scratch.log();
    prepare(&path);
    let mut text = String::new();
    for index in 0..EVIDENCE_LINES + 2 {
        text.push_str(&format!("line {index}\n"));
    }
    fs::write(&path, text).expect("seeding a log past the evidence cap");
    let mut tap = LogTap::at(&path);

    assert_eq!(
        tap.drain().expect("the log reads").len(),
        EVIDENCE_LINES + 2,
        "every line the file holds is handed out"
    );
    let kept = tap.drained();
    assert_eq!(kept.len(), EVIDENCE_LINES, "the archive keeps the tail");
    assert_eq!(kept[0], "line 2", "the oldest lines are the ones dropped");
    let last = format!("line {}", EVIDENCE_LINES + 1);
    assert_eq!(kept[kept.len() - 1], last);
}

#[test]
fn test_private_paths_names_the_directory_first_and_every_file_after_it() {
    let scratch = Scratch::new("private-paths");
    let path = scratch.log();
    append(&path, "active");
    fs::write(sibling(&path, 1), "newer\n").expect("writing .1");

    let family = LogFamily::new(&path);
    let paths = family.private_paths().expect("the directory lists");

    // The directory comes first, then the active file and the rolled siblings newest first.
    assert_eq!(paths.len(), 3);
    assert_eq!(
        paths[0].path,
        path.parent().expect("the log file sits in a directory")
    );
    assert_eq!(paths[0].mode, 0o700);
    assert_eq!(paths[1].path, path);
    assert_eq!(paths[1].mode, 0o600);
    assert_eq!(paths[2].path, sibling(&path, 1));
    assert_eq!(paths[2].mode, 0o600);
}

#[test]
fn test_in_sandbox_takes_the_log_path_from_the_plugins_own_layout() {
    let scratch = Scratch::new("in-sandbox");
    let dictionary = scratch.path.join("base.dict");
    fs::write(&dictionary, "not a dictionary").expect("seeding the dictionary");
    let sandbox = Sandbox::create_with_dict(&scratch.path.join("sandbox"), &dictionary)
        .expect("the sandbox builds");

    let tap = LogTap::in_sandbox(&sandbox).expect("the layout resolves");

    let expected = sandbox.data_home().join("rspinyin/logs/rspinyin.log");
    assert_eq!(tap.path(), expected.as_path());

    let files = tap.family().files().expect("the directory lists");
    assert!(files.is_empty(), "the tap follows no file yet");
}

// ---------------------------------------------------------------------------
// The privacy rules
// ---------------------------------------------------------------------------

#[test]
fn test_assert_absent_accepts_what_the_redaction_layer_writes() {
    let scratch = Scratch::new("absent-clean");
    let path = scratch.log();
    append(
        &path,
        "2026-09-29T10:35:12.345678Z INFO ime_core: decode raw=<redacted:len=5> \
         candidate_text=<redacted:len=2> session=42 app=0x8f3a2c1d",
    );
    append(
        &path,
        "2026-09-29T10:35:12.345679Z WARN ime_dict::paths: data/readonly-mode: \
         ~/.local/share/rspinyin is not writable code=data/readonly-mode",
    );
    let tap = LogTap::at(&path);

    tap.assert_absent(&[])
        .expect("a redacted line is what the policy asks for");
}

#[test]
fn test_assert_absent_reports_an_unredacted_field() {
    let scratch = Scratch::new("absent-field");
    let path = scratch.log();
    append(&path, "INFO ime_core: decode raw=example-input app=0x1");
    let tap = LogTap::at(&path);

    let error = tap
        .assert_absent(&[])
        .expect_err("a withheld field in clear text is a leak");
    let (found, suppressed) = findings(&error);
    assert_eq!(suppressed, 0);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].line, Some(1));
    assert_eq!(found[0].denial, Denial::Unredacted { field: "raw" });
}

#[test]
fn test_assert_absent_reports_a_clear_application_identifier() {
    let scratch = Scratch::new("absent-app");
    let path = scratch.log();
    append(&path, "INFO ime_fcitx5::addon: session: start app=firefox");
    let tap = LogTap::at(&path);

    let error = tap
        .assert_absent(&[])
        .expect_err("an application name is not a hash");
    let (found, _) = findings(&error);
    assert_eq!(found[0].denial, Denial::ClearAppIdentifier);
    assert!(
        error.to_string().contains("rspinyin.log:1"),
        "the finding points at the line: {error}"
    );
}

#[test]
fn test_assert_absent_reports_an_unshortened_home_directory() {
    let home = std::env::var("HOME").expect("the tests run with a home directory");
    assert!(home.starts_with('/') && home.len() > 1, "HOME is {home}");

    let scratch = Scratch::new("absent-home");
    let path = scratch.log();
    append(
        &path,
        &format!("WARN ime_dict::paths: {home}/.local/share/rspinyin is not writable"),
    );
    let tap = LogTap::at(&path);

    let error = tap
        .assert_absent(&[])
        .expect_err("a home directory must be shortened to `~`");
    let (found, _) = findings(&error);
    assert_eq!(found[0].denial, Denial::UnshortenedHome);
    assert!(
        !error.to_string().contains(&home),
        "the finding must not repeat the path it objected to: {error}"
    );
}

#[test]
fn test_assert_absent_reports_the_literal_the_case_named() {
    let scratch = Scratch::new("absent-literal");
    let path = scratch.log();
    append(&path, "INFO ime_core: candidates=3");
    let tap = LogTap::at(&path);

    tap.assert_absent(&["nihao"])
        .expect("the case asked for a needle the log does not hold");
    let error = tap
        .assert_absent(&["candidates=3", "ime_core"])
        .expect_err("the case asked for a needle the log holds");
    let (found, _) = findings(&error);
    assert_eq!(
        found[0].denial,
        Denial::Literal {
            pattern: String::from("candidates=3")
        },
        "the first needle the line matches is the one reported"
    );
}

#[test]
fn test_assert_absent_never_repeats_the_line_it_objected_to() {
    let scratch = Scratch::new("absent-no-echo");
    let path = scratch.log();
    // A token that is obviously not something a person typed: a fixture may hold a fabricated
    // log line, and it must not hold text that reads like a real one.
    let typed = "example-input";
    append(&path, &format!("INFO ime_core: decode raw={typed}"));
    let tap = LogTap::at(&path);

    let error = tap
        .assert_absent(&[])
        .expect_err("the field is in clear text");
    let message = error.to_string();
    assert!(
        !message.contains(typed),
        "the report must not undo the redaction it is checking: {message}"
    );
    assert!(
        message.contains("raw") && message.contains("rspinyin.log:1"),
        "the report names the field and where it was written: {message}"
    );
}

#[test]
fn test_assert_absent_scans_a_rolled_sibling() {
    let scratch = Scratch::new("absent-rolled");
    let path = scratch.log();
    let tap = LogTap::at(&path);
    append(&path, "INFO ime_fcitx5::addon: session: start app=firefox");

    roll(&path);
    append(&path, "INFO ime_fcitx5::addon: session: start app=0x2");

    let error = tap
        .assert_absent(&[])
        .expect_err("the leak rolled into a sibling must still be found");
    let (found, _) = findings(&error);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].line, Some(1));
    assert!(
        found[0].path.ends_with("rspinyin.log.1"),
        "the finding points at the rolled file: {}",
        found[0].path.display()
    );
}

#[test]
fn test_assert_absent_scans_the_oldest_rolled_sibling() {
    let scratch = Scratch::new("absent-history");
    let path = scratch.log();
    append(&path, "INFO ime_fcitx5::addon: session: start app=firefox");
    roll(&path);
    append(&path, "INFO ime_fcitx5::addon: session: start app=0x2");
    // The writer shifts the history up before it rolls, so the leak ends up in `.2` -- a file
    // the assertion has to read for the promise "rolled content is still scanned" to hold.
    fs::rename(sibling(&path, 1), sibling(&path, 2)).expect("shifting the history up");
    roll(&path);
    append(&path, "INFO ime_fcitx5::addon: session: start app=0x3");
    let tap = LogTap::at(&path);

    let error = tap
        .assert_absent(&[])
        .expect_err("the leak that rolled out of the active file is still found");
    let (found, _) = findings(&error);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].line, Some(1));
    assert!(
        found[0].path.ends_with("rspinyin.log.2"),
        "the finding points at the oldest sibling: {}",
        found[0].path.display()
    );
}

#[test]
fn test_assert_absent_reads_the_torn_last_line_a_drain_withholds() {
    let scratch = Scratch::new("torn-line");
    let path = scratch.log();
    // A process killed in the middle of a line leaves the tail without its terminator.
    prepare(&path);
    fs::write(
        &path,
        "INFO ime_core: candidates=3\nINFO ime_core: decode raw=example-input",
    )
    .expect("seeding the log");
    let mut tap = LogTap::at(&path);

    assert_eq!(
        borrowed(&tap.drain().expect("the log reads")),
        ["INFO ime_core: candidates=3"],
        "the unterminated tail is left for the call that follows the writer"
    );
    let error = tap
        .assert_absent(&[])
        .expect_err("the torn line is scanned like any other");
    let (found, suppressed) = findings(&error);
    assert_eq!(suppressed, 0);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].line, Some(2));
    assert_eq!(found[0].denial, Denial::Unredacted { field: "raw" });
}

#[test]
fn test_assert_absent_caps_the_findings_it_lists() {
    let scratch = Scratch::new("absent-cap");
    let path = scratch.log();
    for index in 0..MAX_FINDINGS + 4 {
        append(&path, &format!("INFO ime_core: decode raw=secret{index}"));
    }
    let tap = LogTap::at(&path);

    let error = tap.assert_absent(&[]).expect_err("every line leaks");
    let (found, suppressed) = findings(&error);
    assert_eq!(found.len(), MAX_FINDINGS);
    assert_eq!(suppressed, 4);
    assert!(
        error.to_string().contains("4 not listed"),
        "the report says how many findings it left out: {error}"
    );
}

#[test]
fn test_assert_absent_ignores_a_field_name_inside_a_longer_one() {
    let scratch = Scratch::new("absent-boundary");
    let path = scratch.log();
    // `text` is on the denylist; `candidate_text` is a different field, and the redacted value
    // after it must not be read as `text`'s.
    append(
        &path,
        "INFO ime_core: candidate_text=<redacted:len=2> text=<redacted:len=1>",
    );
    let tap = LogTap::at(&path);

    tap.assert_absent(&[])
        .expect("both fields carry the placeholder");
}

#[test]
fn test_assert_absent_accepts_a_placeholder_with_a_zero_length() {
    let scratch = Scratch::new("absent-zero");
    let path = scratch.log();
    append(&path, "INFO ime_core: text=<redacted:len=0>");
    let tap = LogTap::at(&path);

    tap.assert_absent(&[])
        .expect("an empty value is redacted like any other");
}

// ---------------------------------------------------------------------------
// The error-code rule
// ---------------------------------------------------------------------------

#[test]
fn test_assert_error_codes_known_accepts_a_declared_code() {
    let scratch = Scratch::new("codes-known");
    let path = scratch.log();
    append(
        &path,
        "WARN ime_dict::paths: data/readonly-mode: the directory is not writable \
         code=data/readonly-mode",
    );
    append(&path, "INFO ime_core: candidates=3");
    let tap = LogTap::at(&path);

    tap.assert_error_codes_known(&["data/readonly-mode"])
        .expect("the code is declared, and a line without one is not an error line");
}

#[test]
fn test_assert_error_codes_known_reports_a_code_outside_the_frozen_set() {
    let scratch = Scratch::new("codes-unknown");
    let path = scratch.log();
    append(
        &path,
        "WARN ime_core: something happened code=dict/nonsense",
    );
    let tap = LogTap::at(&path);

    let error = tap
        .assert_error_codes_known(&["data/readonly-mode"])
        .expect_err("the code is not one the contract declares");
    let (found, _) = findings(&error);
    assert_eq!(
        found[0].denial,
        Denial::UnknownCode {
            code: String::from("dict/nonsense")
        }
    );
}

#[test]
fn test_assert_error_codes_known_checks_every_code_on_a_line() {
    let scratch = Scratch::new("codes-several");
    let path = scratch.log();
    append(
        &path,
        "WARN ime_dict: code=data/readonly-mode then code=dict/corrupt",
    );
    let tap = LogTap::at(&path);

    let error = tap
        .assert_error_codes_known(&["data/readonly-mode"])
        .expect_err("the second code is not declared");
    let (found, _) = findings(&error);
    assert_eq!(
        found[0].denial,
        Denial::UnknownCode {
            code: String::from("dict/corrupt")
        }
    );
}

#[test]
fn test_assert_error_codes_known_reports_the_error_code_rule() {
    let scratch = Scratch::new("codes-assertion");
    let path = scratch.log();
    append(&path, "WARN ime_core: code=dict/nonsense");
    let tap = LogTap::at(&path);

    let error = tap.assert_error_codes_known(&[]).expect_err("unknown");
    match &error {
        LogError::Failed { assertion, .. } => assert_eq!(*assertion, Assertion::ErrorCodes),
        other => panic!("expected a failed assertion, got {other}"),
    }
    assert!(
        error.to_string().contains("error-code"),
        "the report says which rule broke: {error}"
    );
}

// ---------------------------------------------------------------------------
// Reading a line
// ---------------------------------------------------------------------------

#[test]
fn test_field_values_does_not_match_the_tail_of_a_longer_name() {
    assert_eq!(super::scan::field_values("a text=hello", "text"), ["hello"]);
    assert_eq!(
        super::scan::field_values("a candidate_text=hello", "text"),
        Vec::<&str>::new(),
        "`candidate_text` is a different field from `text`"
    );
    assert_eq!(super::scan::field_values("text=hello", "text"), ["hello"]);
    assert_eq!(super::scan::codes_in("x code=a/b code=c/d"), ["a/b", "c/d"]);
}

// ---------------------------------------------------------------------------
// The frozen error codes
// ---------------------------------------------------------------------------

#[test]
fn test_enum_codes_reads_every_enum_the_source_declares() {
    let codes = EnumCodes::parse(ERRORS_FIXTURE);

    assert_eq!(codes.enums(), ["DictError", "ImeError"]);
    assert_eq!(codes.codes("ImeError"), Some(&code_set(&["dict/corrupt"])));
    assert_eq!(
        codes.codes("DictError"),
        Some(&BTreeSet::new()),
        "an enum whose attributes are all prose is recorded as declaring no code"
    );
}

#[test]
fn test_enum_codes_reads_an_attribute_spread_over_several_lines() {
    let source = "\
pub enum Wrapped {
    #[error(
        \"dict/corrupt: the file at {path} cannot be \\
         read\"
    )]
    Corrupt { path: PathBuf },
}
";
    let codes = EnumCodes::parse(source);

    assert_eq!(codes.codes("Wrapped"), Some(&code_set(&["dict/corrupt"])));
}

#[test]
fn test_code_shape_accepts_the_contracts_codes_and_refuses_prose() {
    for code in [
        "dict/corrupt",
        "ui/stale-select",
        "decode/too-long",
        "platform/x11/no-compositor",
        "platform/fcitx5/version-mismatch",
        "data/readonly-mode",
    ] {
        assert!(is_code_shape(code), "{code} is a contract code");
    }
    for text in [
        "",
        "io",
        "magic mismatch",
        "dict/unavailable: {path}",
        "crc mismatch: expected 0x0a",
        "docs/dev/features.md",
        "dict/Corrupt",
        "dict//corrupt",
    ] {
        assert!(!is_code_shape(text), "{text:?} is not a code");
    }
    // The shape alone cannot tell a code from a path -- `crates/ime-ui/ui` has the same shape --
    // which is why the whitelist is read from the documents and the constants that declare a
    // code, never from a scan of every string in the tree.
    assert!(is_code_shape("crates/ime-ui/ui"));
}

#[test]
fn test_error_code_index_reads_the_specifications_diagnostic_tables() {
    let index = ErrorCodeIndex::from_sources(ERRORS_FIXTURE, SPEC_FIXTURE).expect("both parse");

    assert!(index.contains("dict/corrupt"));
    assert!(index.contains("ui/select/timeout"));
    assert_eq!(
        index.sources_of("ui/select/timeout"),
        Some(&source_set(&[ErrorCodeSource::SpecTables])),
        "a tabulated code is registered as coming from the table"
    );
    assert_eq!(
        index.sources_of("dict/corrupt"),
        Some(&source_set(&[
            ErrorCodeSource::ErrorEnum,
            ErrorCodeSource::SpecEnums
        ])),
        "a code the enums and the copy both declare is registered under both"
    );
}

#[test]
fn test_error_code_index_refuses_a_source_that_declares_no_enum() {
    let error = ErrorCodeIndex::from_sources("fn main() {}", SPEC_FIXTURE)
        .expect_err("a source with no enum declares no code");
    assert!(error.to_string().contains(ERROR_SOURCE), "{error}");
}

#[test]
fn test_error_code_index_refuses_a_specification_without_the_code_section() {
    let error = ErrorCodeIndex::from_sources(ERRORS_FIXTURE, "# a document with no 2.2.4\n")
        .expect_err("a document without the section declares no code");
    assert!(error.to_string().contains(SPEC_SOURCE), "{error}");
}

#[test]
fn test_error_code_index_refuses_a_section_that_copies_no_enum() {
    let spec = "#### 2.2.4 error codes\n\n| code | raiser |\n|---|---|\n| `ui/x` | y |\n";
    let error = ErrorCodeIndex::from_sources(ERRORS_FIXTURE, spec)
        .expect_err("a section that copies no enum is not a usable source");
    assert!(error.to_string().contains(SPEC_SOURCE), "{error}");
}

#[test]
fn test_error_code_index_ignores_a_table_row_that_names_no_code() {
    // The tables are read by position, so the reader has to tell a code row from the header
    // above it and from a row whose first column is prose. A whitelist that swallowed either
    // would accept a code the contract never declared, which is the direction that matters:
    // the assertion exists to reject an undeclared code.
    let spec = r#"#### 2.2.4 error codes

```rust
pub enum ImeError {
    #[error("dict/corrupt: {path}")]
    DictCorrupt { path: PathBuf },
}
```

| dict/header | the row above the separator names the column, not a code |
|---|---|
| `ui/select/timeout` | the channel |
| a plain sentence | nobody |
|  | nothing at all |
"#;
    let index = ErrorCodeIndex::from_sources(ERRORS_FIXTURE, spec).expect("the sources parse");

    assert_eq!(
        index.codes(),
        ["dict/corrupt", "ui/select/timeout"],
        "only the row that carries a code is read"
    );
}

#[test]
fn test_spec_enums_matches_the_source_enum_by_enum() {
    // `features.md` 2.2.4 promises its fenced copy of the error enums is verbatim. This is the
    // assertion that promise is checked with, and it is the reason the whitelist is extracted
    // from both documents instead of copied into a list here.
    let root = repo_root();
    let errors = fs::read_to_string(root.join(ERROR_SOURCE)).expect("reading the error enums");
    let spec = fs::read_to_string(root.join(SPEC_SOURCE)).expect("reading the specification");
    let declared = EnumCodes::parse(&errors);
    let copied = spec_enums(&spec).expect("the specification copies the enums");

    assert!(
        copied.len() >= 3,
        "the document copies at least three enums"
    );
    for name in copied.enums() {
        assert_eq!(
            copied.codes(name),
            declared.codes(name),
            "the copy of {name} in {SPEC_SOURCE} disagrees with {ERROR_SOURCE}"
        );
    }
}

#[test]
fn test_error_code_index_reads_the_workspace() {
    let root = repo_root();
    let index = ErrorCodeIndex::from_repo(&root).expect("the repository declares its codes");

    for code in [
        "dict/corrupt",
        "ui/stale-select",
        "decode/too-long",
        "ui/select/timeout",
        "ffi/invalid-commit",
        "lifecycle/step-failed",
        "data/perms/fixed",
        "crash/panic",
    ] {
        assert!(
            index.contains(code),
            "{code} is a code the contract declares"
        );
    }
    assert!(
        index
            .sources_of("data/perms/fixed")
            .is_some_and(|sources| sources.contains(&ErrorCodeSource::Constant)),
        "a code only a constant declares is found by the constant reader"
    );
    assert!(
        index.contains("phrase/table-unavailable"),
        "a code whose constant is named without the `CODE` suffix is still in the set, because \
         the specification's table declares it"
    );
    assert!(
        index.len() > 40,
        "the workspace declares more than forty codes, found {}",
        index.len()
    );
    let codes = index.codes();
    let mut sorted = codes.clone();
    sorted.sort_unstable();
    assert_eq!(codes, sorted, "the codes come out in name order");
}

#[test]
fn test_error_code_index_reads_a_code_constant_and_skips_other_strings() {
    let scratch = Scratch::new("codes-constants");
    let source = scratch.path.join("crates").join("demo").join("src");
    fs::create_dir_all(&source).expect("creating the crate directory");
    fs::write(
        source.join("lib.rs"),
        "\
pub const PERMS_FIXED_CODE: &str = \"data/perms/fixed\";
const REPEATED_CODE: &str = \"ffi/null-key-event\";
pub const UI_DIR: &str = \"crates/ime-ui/ui\";
pub const SPEC_FILE: &str = \"docs/dev/features.md\";
pub const LOG_FILE_NAME: &str = \"rspinyin.log\";
",
    )
    .expect("writing the fixture crate");

    let mut index = ErrorCodeIndex::default();
    let added = index
        .add_constants(&scratch.path.join("crates"))
        .expect("the fixture crate reads");

    assert_eq!(added, 2);
    assert!(index.contains("data/perms/fixed"));
    assert!(index.contains("ffi/null-key-event"));
    assert!(
        !index.contains("crates/ime-ui/ui"),
        "a path constant is not a code"
    );
}

/// The repository root, derived from the compile-time location of this crate.
fn repo_root() -> PathBuf {
    crate::budget::repo_root().expect("xtask lives in a subdirectory of the repository root")
}
