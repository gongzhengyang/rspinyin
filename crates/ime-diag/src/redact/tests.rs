//! Unit tests for the redaction layer and the compact-text formatter.
//!
//! Responsibility: pin what may reach a sink -- the denylist and the character count of the
//! placeholder that replaces a withheld value, the home-directory shortening, the escaping
//! of a newline so a value cannot forge a second line, and the downgrade of every event of
//! a session marked sensitive.
//!
//! Boundaries: each test installs the pipeline as a thread-local default over an in-memory
//! sink, so no test writes a file, reads the environment, or competes with another test for
//! the process-wide subscriber.

use std::io;

use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::registry;

use super::*;

/// The home directory the tests shorten, chosen so that a sibling directory shares its
/// first characters and proves the boundary check.
const TEST_HOME: &str = "/home/tester";

/// A `MakeWriter` sink that keeps everything written to it in memory.
#[derive(Clone, Debug, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    /// Everything written so far, as text.
    fn text(&self) -> String {
        let bytes = self.0.lock().expect("the capture lock is not poisoned");
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("not poisoned").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Installs the redaction pipeline on the current thread.
///
/// The subscriber is a thread-local default rather than the global one, so tests stay
/// independent of each other. The returned guard keeps it installed and must be held.
fn capture(state: &Arc<RedactState>) -> (Capture, tracing::subscriber::DefaultGuard) {
    let sink = Capture::default();
    let writer = sink.clone();
    let subscriber = registry().with(RedactLayer::new(Arc::clone(state))).with(
        fmt::layer()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .event_format(RedactFormat::new(Arc::clone(state))),
    );
    let guard = tracing::subscriber::set_default(subscriber);
    (sink, guard)
}

/// A state that filters at `level` and shortens [`TEST_HOME`].
fn state_at(level: LevelFilter) -> Arc<RedactState> {
    Arc::new(RedactState::new(
        level,
        LevelFilter::ERROR,
        Some(PathBuf::from(TEST_HOME)),
    ))
}

/// The line of `text` that contains `needle`.
fn line_with<'a>(text: &'a str, needle: &str) -> &'a str {
    text.lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no line contains {needle:?} in:\n{text}"))
}

#[test]
fn test_redact_format_withholds_every_denied_field() {
    let state = state_at(LevelFilter::INFO);
    let (sink, _guard) = capture(&state);
    let raw = String::from("nihao");
    let debugged = String::from("mima");

    tracing::info!(raw = %raw, candidate_count = 7, "candidates built");
    tracing::info!(preedit = "ni'hao", text = %"shang", "committed");
    // `?raw` renders through `Debug` instead of borrowing the string; it is withheld there
    // too, with the quoting included in the count.
    tracing::info!(raw = ?debugged, "key handled");
    // Below the configured level, so it never reaches the sink at all.
    tracing::debug!(raw = %raw, "dropped");

    let text = sink.text();
    assert_eq!(text.lines().count(), 3, "{text}");
    for withheld in ["<redacted:len=5>", "<redacted:len=6>"] {
        assert!(text.contains(withheld), "{withheld} missing from:\n{text}");
    }
    // Two six-character placeholders: `ni'hao`, and the `Debug` rendering of `mima`, which
    // carries its quotes into the count.
    assert_eq!(text.matches("<redacted:len=6>").count(), 2, "{text}");
    for leaked in ["nihao", "ni'hao", "shang", "mima"] {
        assert!(!text.contains(leaked), "{leaked} leaked into:\n{text}");
    }
    // The structural fields around the withheld one are what the diagnostics are for.
    assert!(text.contains("candidate_count=7"), "{text}");
}

#[test]
fn test_redact_format_redacts_denied_fields_at_debug_level_too() {
    // The input-content switch only raises the level; it cannot turn characters back on,
    // so the denylist holds at `Debug` exactly as it does at `Info`.
    let state = state_at(LevelFilter::DEBUG);
    let (sink, _guard) = capture(&state);
    let preedit = String::from("ni hao");

    tracing::debug!(preedit = %preedit, dag_edges = 12, "segmentation built");

    let text = sink.text();
    assert!(text.contains("<redacted:len=6>"), "{text}");
    assert!(!text.contains("ni hao"), "{text}");
    // The structural substitute the design asks for is present.
    assert!(text.contains("dag_edges=12"), "{text}");
}

#[test]
fn test_redact_format_downgrades_every_event_of_a_sensitive_session() {
    let state = state_at(LevelFilter::INFO);
    let (sink, _guard) = capture(&state);

    // The flag arrives with the session's first event, which is when a host that reads
    // `CapabilityFlag::Password` knows the context is a password box.
    tracing::info!(
        session = 42u64,
        app = 0x8f3a_2c1du64,
        password = true,
        "session: start"
    );
    tracing::info!(
        session = 42u64,
        candidate_count = 7,
        raw_len = 5,
        "candidates built"
    );
    tracing::info!(session = 7u64, candidate_count = 3, "candidates built");

    let text = sink.text();
    assert!(!text.contains("session=42"), "{text}");
    // Neither the flag, nor the input length, nor the candidate count, nor the message of
    // the sensitive session is written.
    for withheld in ["password", "raw_len", "candidate_count=7", "session: start"] {
        assert!(!text.contains(withheld), "{withheld} leaked into:\n{text}");
    }
    let downgraded = text
        .lines()
        .filter(|l| l.contains("session=redacted"))
        .count();
    assert_eq!(downgraded, 2, "{text}");
    // The one field a downgraded event keeps is the application hash, and the other session
    // is untouched.
    let sensitive = line_with(&text, "session=redacted");
    assert!(sensitive.contains("app=2402954269"), "{sensitive}");
    assert!(
        line_with(&text, "candidate_count=3").contains("session=7"),
        "{text}"
    );
}

#[test]
fn test_redact_state_mark_sensitive_session_downgrades_following_events() {
    // The path a host that has just seen `CapabilityFlag::Password` takes: no event has to
    // carry the flag, so no level change can lose the mark.
    let state = state_at(LevelFilter::INFO);
    let (sink, _guard) = capture(&state);
    state.mark_sensitive_session(9);

    tracing::info!(session = 9u64, raw_len = 4, "key handled");

    let text = sink.text();
    assert!(text.contains("session=redacted"), "{text}");
    assert!(
        !text.contains("raw_len") && !text.contains("session=9"),
        "{text}"
    );
}

#[test]
fn test_observe_visitor_reads_the_flag_from_a_rendered_boolean() {
    // `password = %flag` renders through `Display` instead of `record_bool`; both
    // spellings have to mark the session.
    let state = state_at(LevelFilter::INFO);
    let (sink, _guard) = capture(&state);
    let flagged = true;

    tracing::info!(session = 3u64, password = %flagged, "session: start");
    tracing::info!(session = 3u64, raw_len = 8, "key handled");

    let text = sink.text();
    assert!(!text.contains("raw_len"), "{text}");
    let downgraded = text
        .lines()
        .filter(|l| l.contains("session=redacted"))
        .count();
    assert_eq!(downgraded, 2, "{text}");
}

#[test]
fn test_redact_format_shortens_the_home_prefix() {
    let state = state_at(LevelFilter::INFO);
    let (sink, _guard) = capture(&state);
    let path = String::from("/home/tester/.local/share/rspinyin/user.redb");

    tracing::info!(path = %path, "user database opened");
    tracing::info!(path = "/home/tester", "home itself");

    let text = sink.text();
    assert!(
        text.contains("path=~/.local/share/rspinyin/user.redb"),
        "{text}"
    );
    let home_line = line_with(&text, "home itself").trim_end();
    assert!(home_line.ends_with("path=~"), "{home_line}");
    assert!(!text.contains(TEST_HOME), "{text}");
}

#[test]
fn test_shorten_home_requires_a_path_component_boundary() {
    let home = Path::new(TEST_HOME);
    assert_eq!(shorten_home("/home/tester2/log", home), "/home/tester2/log");
    assert_eq!(shorten_home("/home/test", home), "/home/test");
    assert_eq!(shorten_home("", home), "");
    // A home directory given with a trailing separator behaves the same way.
    assert_eq!(
        shorten_home("/home/tester/log", Path::new("/home/tester/")),
        "~/log"
    );
    // A home directory that is not valid UTF-8 cannot be matched as text.
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;

        let raw = std::ffi::OsStr::from_bytes(b"/home/\xff");
        assert_eq!(shorten_home("/home/x/log", Path::new(raw)), "/home/x/log");
    }
}

#[test]
fn test_redact_format_escapes_newlines_and_reports_the_event_identity() {
    let state = state_at(LevelFilter::INFO);
    let (sink, _guard) = capture(&state);

    // A value that carries a newline must not be able to forge a second line.
    tracing::info!(path = %"first\nINFO forged: line", "user database opened");
    tracing::warn!(session = 5u64, "decode gave up");

    let text = sink.text();
    assert_eq!(text.lines().count(), 2, "{text}");
    assert!(text.contains("first\\nINFO forged: line"), "{text}");
    // The module target is what tells a reader which crate emitted the line.
    let line = line_with(&text, "session=5");
    assert!(line.contains("WARN"), "{line}");
    assert!(line.contains("ime_diag::redact::tests"), "{line}");
    assert!(line.contains("decode gave up"), "{line:?}");
    // The terminator check has to be made against the whole capture: `line_with`
    // reads through `str::lines`, which has already dropped it, so asserting on
    // `line` here could never hold.
    assert!(text.ends_with('\n'), "{text:?}");
}

#[test]
fn test_is_denied_field_matches_the_documented_list() {
    let denied = [
        "raw",
        "text",
        "preedit",
        "input",
        "candidate_text",
        "word",
        "commit_text",
    ];
    assert_eq!(DENIED_FIELDS, denied);
    for name in denied {
        assert!(is_denied_field(name), "{name}");
    }
    // Structural facts about the input, and the identity fields, are not content.
    for allowed in [
        "raw_len",
        "candidate_count",
        "app",
        "session",
        "path",
        "message",
    ] {
        assert!(!is_denied_field(allowed), "{allowed}");
    }
}

#[test]
fn test_redaction_helpers_count_characters_and_read_session_ids() {
    // `chars().count()` makes a multi-byte input report its length rather than its size.
    assert_eq!(
        redaction_placeholder("你好".chars().count()),
        "<redacted:len=2>"
    );
    assert_eq!(parse_session("42"), Some(42));
    assert_eq!(parse_session("SessionId(42)"), Some(42));
    assert_eq!(parse_session("none"), None);
}

#[test]
fn test_redact_format_withholds_every_denied_name_at_the_event_level() {
    // The denylist is asserted as a list by `test_is_denied_field_matches_the_documented_list`;
    // this is the assertion that the formatter applies it to every name on that list --
    // including the ones no caller uses today, and a value that arrives as a number
    // rather than as a string.
    let state = state_at(LevelFilter::INFO);
    let (sink, _guard) = capture(&state);
    let planted = String::from("fixture-alpha");

    tracing::info!(
        raw = %planted,
        text = %planted,
        preedit = %planted,
        input = %planted,
        candidate_text = %planted,
        word = %planted,
        commit_text = %planted,
        "candidates built"
    );
    tracing::info!(raw = 42u64, "key handled");

    let text = sink.text();
    assert!(!text.contains("fixture-alpha"), "{text}");
    assert!(
        !text.contains("raw=42"),
        "a denied value is withheld whatever its type:\n{text}"
    );
    for name in DENIED_FIELDS {
        let withheld_pair = format!("{name}=<redacted:len=");
        assert!(
            text.contains(withheld_pair.as_str()),
            "{name} is on the denylist but reached the sink:\n{text}"
        );
    }
}

#[test]
fn test_redact_format_scrubs_a_denied_assignment_inside_the_message() {
    // A message is a static template by project rule, but a template that quotes a value
    // is free text all the same: the denylist has to reach the message too, or the one
    // place a value could be interpolated would be the one place it is not checked.
    let state = state_at(LevelFilter::INFO);
    let (sink, _guard) = capture(&state);
    let planted = String::from("fixture-alpha");

    tracing::info!("decode gave up: raw={planted}");
    tracing::info!(raw_len = 13u64, "candidates built");

    let text = sink.text();
    assert!(!text.contains("fixture-alpha"), "{text}");
    assert!(text.contains("raw=<redacted:len=13>"), "{text}");
    // A structural name that merely starts with a denied one is not a value.
    assert!(text.contains("raw_len=13"), "{text}");
}

#[test]
fn test_scrub_denied_values_replaces_only_denied_assignments() {
    assert_eq!(&*scrub_denied_values("raw=abc"), "raw=<redacted:len=3>");
    // A quoted value runs through its closing quote, which is what a `Debug` rendering
    // carries and what the placeholder's count has to include.
    assert_eq!(
        &*scrub_denied_values("preedit=\"fixture-alpha\""),
        "preedit=<redacted:len=15>"
    );
    // Two pairs on one line are both replaced, and the text between them survives.
    assert_eq!(
        &*scrub_denied_values("raw=ab, text=cd"),
        "raw=<redacted:len=2>, text=<redacted:len=2>"
    );
    // A value at the end of the text, and a pair with no value at all.
    assert_eq!(&*scrub_denied_values("word=ab"), "word=<redacted:len=2>");
    assert_eq!(&*scrub_denied_values("word="), "word=<redacted:len=0>");
    // The longest name wins, so `commit_text` is not read as `text`.
    assert_eq!(
        &*scrub_denied_values("commit_text=ab"),
        "commit_text=<redacted:len=2>"
    );
    // A name only counts at a word boundary and only with an `=` after it, so the
    // structural names and the near misses are left exactly as they were.
    for untouched in [
        "raw_len=6",
        "input_buffer=2",
        "keyword=abc",
        "commit_tex=1",
        "raw: 6",
        "",
    ] {
        assert_eq!(&*scrub_denied_values(untouched), untouched, "{untouched}");
    }
}
