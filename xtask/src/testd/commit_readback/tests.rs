//! Tests for [`super`].
//!
//! Every test here runs with no display server, no Fcitx5 and no client process: the probe is
//! driven against a stream that answers from a script, so a timeout is a scripted answer rather
//! than a wall-clock wait and a client that died is a scripted end of input rather than a killed
//! process. The one test that touches a real process asks for a program that is not there, which
//! is refused before anything is executed.
//!
//! The two halves of the channel are covered separately. The stream's own tests drive
//! [`LinePump`] over an in-memory reader -- the framing of a line split across reads, a line past
//! the ceiling, a read that fails -- and the probe's tests drive the decisions: which event is
//! delivered, what a timeout carries, and what a client that stopped mid-sentence reports.

use std::collections::VecDeque;
use std::io::{Cursor, Read};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::stream::{Arrival, ClientStream, LinePump, decode, pump_errors};
use super::*;

/// The window the scripted clients report.
const WINDOW: u32 = 4_194_305;

/// What a scripted stream does when the next line is asked for.
#[derive(Clone, Debug)]
enum Step {
    /// Answer with this line.
    Line(String),
    /// Answer that nothing arrived within the timeout.
    Idle,
    /// Answer that the stream is over.
    End,
}

/// A client that answers from a script.
///
/// The timeout is deliberately ignored: a scripted stream has no clock, so "nothing arrived" is a
/// step rather than a wait. That is what makes the probe's deadline handling testable without the
/// test taking any time at all.
#[derive(Debug)]
struct FakeStream {
    /// The answers left, in order.
    steps: VecDeque<Step>,
    /// What `exit_status` reports.
    status: Option<String>,
    /// What `stderr_tail` reports.
    stderr: Vec<String>,
    /// Whether the stream has been closed.
    closed: bool,
}

impl FakeStream {
    /// A stream that answers the scripted steps, then answers that nothing arrives.
    fn new(steps: Vec<Step>) -> Self {
        Self {
            steps: VecDeque::from(steps),
            status: None,
            stderr: Vec::new(),
            closed: false,
        }
    }

    /// The same stream, reporting this exit status.
    fn with_status(mut self, status: &str) -> Self {
        self.status = Some(status.to_owned());
        self
    }

    /// The same stream, reporting this standard error tail.
    fn with_stderr(mut self, lines: &[&str]) -> Self {
        self.stderr = lines.iter().map(|line| (*line).to_owned()).collect();
        self
    }
}

impl ClientStream for FakeStream {
    fn next_line(&mut self, _timeout: Duration) -> Result<Arrival, ReadbackError> {
        if self.closed {
            return Err(ReadbackError::Closed);
        }
        match self.steps.pop_front() {
            Some(Step::Line(text)) => Ok(Arrival::Line(text)),
            // A script that has run out behaves like a client that has nothing more to say: it is
            // the shape every timeout test needs, and it is answered without waiting.
            Some(Step::Idle) | None => Ok(Arrival::Idle),
            Some(Step::End) => Ok(Arrival::End),
        }
    }

    fn exit_status(&mut self) -> Option<String> {
        self.status.clone()
    }

    fn stderr_tail(&self) -> Vec<String> {
        self.stderr.clone()
    }

    fn close(&mut self) -> Result<(), ReadbackError> {
        if self.closed {
            return Err(ReadbackError::Closed);
        }
        self.closed = true;
        Ok(())
    }
}

/// A reader that hands out one byte per call, which is how a line arrives split across reads.
#[derive(Debug)]
struct OneByteAtATime {
    /// The bytes left to hand out.
    bytes: Vec<u8>,
}

impl OneByteAtATime {
    /// A reader over `text`.
    fn new(text: &str) -> Self {
        Self {
            bytes: text.as_bytes().to_vec(),
        }
    }
}

impl Read for OneByteAtATime {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.bytes.is_empty() || out.is_empty() {
            return Ok(0);
        }
        out[0] = self.bytes.remove(0);
        Ok(1)
    }
}

/// A reader that answers end of file only after a delay, which is what a client that is still
/// starting up looks like from the harness's side.
#[derive(Debug)]
struct Slow {
    /// How long every read takes before it answers.
    delay: Duration,
}

impl Read for Slow {
    fn read(&mut self, _out: &mut [u8]) -> std::io::Result<usize> {
        thread::sleep(self.delay);
        Ok(0)
    }
}

/// A reader whose descriptor is gone.
#[derive(Debug)]
struct Failing;

impl Read for Failing {
    fn read(&mut self, _out: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("the pipe is gone"))
    }
}

/// The `ready` line a client announces itself with.
fn ready_step(caps: &str) -> Step {
    let line = serde_json::json!({
        "event": "ready",
        "caps": caps,
        "window": WINDOW,
    });
    Step::Line(line.to_string())
}

/// A `commit` line carrying `text`.
fn commit_step(text: &str) -> Step {
    let line = serde_json::json!({ "event": "commit", "text": text });
    Step::Line(line.to_string())
}

/// A `preedit` line carrying `text` and a caret.
fn preedit_step(text: &str, caret: u32) -> Step {
    let line = serde_json::json!({ "event": "preedit", "text": text, "caret": caret });
    Step::Line(line.to_string())
}

/// A `preedit` line that clears the preedit.
fn cleared_step() -> Step {
    Step::Line(cleared_line())
}

/// The line a client writes when it clears the preedit.
fn cleared_line() -> String {
    serde_json::json!({ "event": "preedit", "text": null }).to_string()
}

/// The spec the startup tests start, with a program that is never executed.
fn spec(caps: ClientCaps) -> ClientSpec {
    ClientSpec::new("/nonexistent/rspinyin-test-client", ":99", caps)
}

/// Both capabilities, which is what a case covering either path starts with.
fn both() -> ClientCaps {
    ClientCaps {
        preedit: true,
        panel: true,
    }
}

/// A probe over a scripted client, already announced.
fn probe_with(stream: FakeStream) -> CommitProbe {
    let ready = ClientReady {
        caps: both(),
        window: Some(WINDOW),
    };
    CommitProbe::adopt(Box::new(stream), ready, Vec::new())
}

/// A probe over a client that answers the scripted steps.
fn probe(steps: Vec<Step>) -> CommitProbe {
    probe_with(FakeStream::new(steps))
}

/// A launcher that answers the scripted streams in order.
fn launcher(
    scripts: Vec<FakeStream>,
) -> impl FnMut(&ClientSpec) -> Result<Box<dyn ClientStream>, ReadbackError> {
    let mut scripts = VecDeque::from(scripts);
    move |_spec| {
        let script = scripts.pop_front().expect("one script per attempt");
        let stream: Box<dyn ClientStream> = Box::new(script);
        Ok(stream)
    }
}

#[test]
fn test_client_caps_parse_reads_the_documented_names() {
    assert_eq!(
        ClientCaps::parse("preedit,panel"),
        Ok(both()),
        "both names, in the order the protocol writes them"
    );
    assert_eq!(
        ClientCaps::parse(" panel "),
        Ok(ClientCaps {
            preedit: false,
            panel: true
        })
    );
    assert_eq!(
        ClientCaps::parse(""),
        Ok(ClientCaps::default()),
        "an empty set is a client that declares nothing"
    );
    assert_eq!(both().arg(), "preedit,panel");
    assert_eq!(both().to_string(), "preedit,panel");
    assert_eq!(ClientCaps::default().to_string(), "(none)");
    assert!(
        both().contains(ClientCaps {
            preedit: true,
            panel: false
        }),
        "a set contains each of its capabilities"
    );
    assert!(
        !ClientCaps {
            preedit: true,
            panel: false
        }
        .contains(both()),
        "a set does not contain a capability it lacks"
    );
    assert!(
        both().contains(ClientCaps::default()),
        "every set contains the empty one"
    );
}

#[test]
fn test_client_caps_parse_refuses_a_name_it_does_not_know() {
    let error = ClientCaps::parse("preedit,sensitive").expect_err("an unknown capability");
    assert!(error.contains("sensitive"), "{error}");
    assert!(
        error.contains("preedit") && error.contains("panel"),
        "the refusal must name what the protocol does define: {error}"
    );
    assert!(ClientCaps::parse("password").is_err());
    assert!(
        ClientCaps::parse("preedit,panel,preedit").is_ok(),
        "a repeated name"
    );
}

#[test]
fn test_client_spec_command_carries_the_display_and_the_capability_set() {
    let client = spec(ClientCaps {
        preedit: true,
        panel: false,
    });
    assert_eq!(
        client.program(),
        std::path::Path::new("/nonexistent/rspinyin-test-client")
    );
    assert_eq!(client.display(), ":99");
    assert_eq!(
        client.caps(),
        ClientCaps {
            preedit: true,
            panel: false
        }
    );

    let command = client.command();
    assert_eq!(command.get_program(), "/nonexistent/rspinyin-test-client");
    let args: Vec<String> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        ["--display=:99", "--caps=preedit"],
        "the argv contract the client implements"
    );
    let display = command
        .get_envs()
        .find_map(|(name, value)| (name == "DISPLAY").then_some(value))
        .flatten();
    assert_eq!(
        display.map(|value| value.to_string_lossy().into_owned()),
        Some(String::from(":99")),
        "the toolkit reads the display from the environment, so it is set too"
    );
}

#[test]
fn test_decode_reads_each_event_kind_the_protocol_defines() {
    let ready = serde_json::json!({ "event": "ready", "caps": "preedit,panel", "window": WINDOW });
    assert_eq!(
        decode(&ready.to_string(), 1),
        Ok(ClientEvent::Ready(ClientReady {
            caps: both(),
            window: Some(WINDOW),
        }))
    );

    let no_window = serde_json::json!({ "event": "ready", "caps": "panel" });
    assert_eq!(
        decode(&no_window.to_string(), 2),
        Ok(ClientEvent::Ready(ClientReady {
            caps: ClientCaps {
                preedit: false,
                panel: true
            },
            window: None,
        })),
        "a client that does not name its window still announces itself"
    );

    assert_eq!(
        decode(
            &serde_json::json!({ "event": "commit", "text": "你好" }).to_string(),
            3
        ),
        Ok(ClientEvent::Commit(String::from("你好")))
    );
    assert_eq!(
        decode(
            &serde_json::json!({ "event": "preedit", "text": "ni", "caret": 2 }).to_string(),
            4
        ),
        Ok(ClientEvent::Preedit(Some(PreeditState {
            text: String::from("ni"),
            caret: Some(2),
        })))
    );
    assert_eq!(
        decode(&cleared_line(), 5),
        Ok(ClientEvent::Preedit(None)),
        "a null text is the host clearing the preedit"
    );
}

#[test]
fn test_decode_refuses_a_line_without_repeating_what_it_held() {
    // The text a user typed, in a line that is not JSON at all.
    let secret = "你好secret";
    let line = format!("{{\"event\":\"commit\",\"text\":{secret}}}");
    let error = decode(&line, 7).expect_err("an unquoted value is not JSON");
    assert!(
        matches!(error, ReadbackError::Protocol { line: 7, .. }),
        "{error}"
    );
    let message = error.to_string();
    assert!(
        !message.contains(secret),
        "a message must not repeat the line, which holds what the user typed: {message}"
    );
    assert!(
        message.contains('7'),
        "the line number is what locates it: {message}"
    );

    // The same rule for a field the event does not take, which is a shape error the parser
    // catches itself.
    let extra = serde_json::json!({ "event": "commit", "text": "你好", "raw": secret }).to_string();
    let error = decode(&extra, 8).expect_err("a field the event does not take");
    assert!(
        matches!(error, ReadbackError::Protocol { line: 8, .. }),
        "{error}"
    );
    assert!(!error.to_string().contains(secret), "{error}");
}

#[test]
fn test_decode_refuses_an_event_kind_it_does_not_know() {
    let line = serde_json::json!({ "event": "paint", "text": "x" }).to_string();
    let error = decode(&line, 2).expect_err("an event the protocol does not define");
    let message = error.to_string();
    assert!(
        message.contains("paint"),
        "the refusal names the token, which is protocol vocabulary and never content: {message}"
    );
}

#[test]
fn test_decode_refuses_an_event_missing_a_field_it_needs() {
    let commit = serde_json::json!({ "event": "commit" }).to_string();
    let error = decode(&commit, 1).expect_err("a commit without text");
    assert!(error.to_string().contains("text"), "{error}");

    let ready = serde_json::json!({ "event": "ready", "window": WINDOW }).to_string();
    let error = decode(&ready, 1).expect_err("a ready without caps");
    assert!(error.to_string().contains("caps"), "{error}");

    let unknown = serde_json::json!({ "event": "ready", "caps": "everything" }).to_string();
    assert!(
        decode(&unknown, 1).is_err(),
        "a capability set the protocol does not define is refused rather than ignored"
    );
}

#[test]
fn test_line_pump_reassembles_a_line_split_across_reads() {
    let mut pump = LinePump::start(OneByteAtATime::new("one\ntwo\nthree"));
    assert_eq!(
        pump.next_line(Duration::from_secs(5)),
        Ok(Arrival::Line(String::from("one")))
    );
    assert_eq!(
        pump.next_line(Duration::from_secs(5)),
        Ok(Arrival::Line(String::from("two")))
    );
    assert_eq!(
        pump.next_line(Duration::from_secs(5)),
        Ok(Arrival::Line(String::from("three"))),
        "a last line with no newline is still a line"
    );
    assert_eq!(
        pump.next_line(Duration::from_secs(5)),
        Ok(Arrival::End),
        "and end of file follows it"
    );
}

#[test]
fn test_line_pump_keeps_the_order_of_two_hundred_lines() {
    let expected: Vec<String> = (0..200).map(|index| format!("line {index}")).collect();
    let text = format!("{}\n", expected.join("\n"));
    let mut pump = LinePump::start(Cursor::new(text.into_bytes()));
    for (index, line) in expected.iter().enumerate() {
        assert_eq!(
            pump.next_line(Duration::from_secs(5)),
            Ok(Arrival::Line(line.clone())),
            "line {index} arrives in the order it was written"
        );
    }
    assert_eq!(pump.next_line(Duration::from_secs(5)), Ok(Arrival::End));
}

#[test]
fn test_line_pump_reports_idle_when_nothing_arrives_in_time() {
    // The delay is a hundred times the deadline, so the answer cannot be a race: the budget is
    // deliberately generous because a machine under load is not what this test is about.
    let mut pump = LinePump::start(Slow {
        delay: Duration::from_millis(100),
    });
    assert_eq!(
        pump.next_line(Duration::from_millis(1)),
        Ok(Arrival::Idle),
        "a client that has not written yet is not a client that is gone"
    );
    assert_eq!(
        pump.next_line(Duration::from_secs(5)),
        Ok(Arrival::End),
        "the same stream reports end of file once the reader finishes"
    );
}

#[test]
fn test_line_pump_refuses_a_line_past_the_ceiling() {
    let long = "x".repeat(MAX_EVENT_BYTES + 1);
    let mut pump = LinePump::start(Cursor::new(long.into_bytes()));
    let error = pump
        .next_line(Duration::from_secs(5))
        .expect_err("a line past the ceiling is refused");
    assert!(matches!(error, ReadbackError::Stream { .. }), "{error}");
    assert!(
        error.to_string().contains(&MAX_EVENT_BYTES.to_string()),
        "the refusal names the ceiling: {error}"
    );
}

#[test]
fn test_line_pump_reports_a_read_error_rather_than_ending_quietly() {
    let mut pump = LinePump::start(Failing);
    let error = pump
        .next_line(Duration::from_secs(5))
        .expect_err("a read error is reported");
    assert!(matches!(error, ReadbackError::Stream { .. }), "{error}");
    assert!(error.to_string().contains("the pipe is gone"), "{error}");
}

#[test]
fn test_error_pump_keeps_the_tail_of_a_chatty_client() {
    let mut text = String::new();
    for index in 0..STDERR_LINES + 5 {
        text.push_str(&format!("line {index}\n"));
    }
    let sink = Arc::new(Mutex::new(VecDeque::new()));
    pump_errors(Cursor::new(text.into_bytes()), Arc::clone(&sink));
    let kept: Vec<String> = sink
        .lock()
        .expect("the tail lock is not poisoned")
        .iter()
        .cloned()
        .collect();
    assert_eq!(kept.len(), STDERR_LINES, "the tail is bounded");
    assert_eq!(
        kept.first().map(String::as_str),
        Some("line 5"),
        "the oldest lines are the ones dropped"
    );
    assert_eq!(kept.last().map(String::as_str), Some("line 204"));
}

#[test]
fn test_error_pump_records_a_read_failure_that_ends_it() {
    let sink = Arc::new(Mutex::new(VecDeque::new()));
    pump_errors(Failing, Arc::clone(&sink));
    let kept: Vec<String> = sink
        .lock()
        .expect("the tail lock is not poisoned")
        .iter()
        .cloned()
        .collect();
    assert_eq!(kept.len(), 1, "one line says why the tail stops here");
    let note = &kept[0];
    assert!(
        note.starts_with("[rspinyin harness]"),
        "the note is marked so it is not read as the client's own words: {note}"
    );
    assert!(note.contains("the pipe is gone"), "{note}");
}

#[test]
fn test_deadline_after_survives_a_timeout_that_would_overflow() {
    let before = Instant::now();
    let deadline = deadline_after(Duration::MAX);
    assert!(
        deadline >= before,
        "a timeout that cannot be added falls back to now instead of panicking"
    );
    assert!(
        remaining(deadline) <= Duration::from_secs(1),
        "and every wait on it ends at its first deadline check"
    );
}

#[test]
fn test_next_commit_returns_the_text_the_client_received() {
    let mut probe = probe(vec![preedit_step("nihao", 5), commit_step("你好")]);
    assert_eq!(probe.next_commit(COMMIT_TIMEOUT), Ok(String::from("你好")));
    assert_eq!(
        probe.transcript().commits(),
        ["你好"],
        "and the transcript keeps it"
    );
}

#[test]
fn test_next_commit_skips_preedit_events_and_records_them() {
    let mut probe = probe(vec![
        preedit_step("n", 1),
        preedit_step("ni", 2),
        commit_step("你"),
    ]);
    assert_eq!(probe.next_commit(COMMIT_TIMEOUT), Ok(String::from("你")));
    assert_eq!(
        probe.transcript().newest_preedit(),
        Some("ni"),
        "a run with client_preedit on still delivers the commit, and keeps the preedit"
    );
}

#[test]
fn test_next_commit_reports_a_timeout_with_the_events_that_did_arrive() {
    let secret = "secretpreedit";
    let mut probe = probe(vec![preedit_step(secret, 2), Step::Idle]);
    let error = probe
        .next_commit(COMMIT_TIMEOUT)
        .expect_err("no commit arrived");
    let message = error.to_string();
    assert!(
        !message.contains(secret),
        "a message must not carry the text: {message}"
    );
    assert!(
        message.contains("2 event(s)"),
        "the refusal counts what did arrive: {message}"
    );
    match error {
        ReadbackError::CommitTimeout {
            received,
            transcript,
            ..
        } => {
            assert_eq!(received, 2, "the announcement and the preedit");
            assert_eq!(
                transcript.newest_preedit(),
                Some(secret),
                "and the transcript is attached, so the case can see what arrived instead"
            );
        }
        other => panic!("a commit timeout was expected, not {other}"),
    }
}

#[test]
fn test_next_commit_reports_a_client_that_stopped_without_committing() {
    let stream = FakeStream::new(vec![Step::End])
        .with_status("exit status: 1")
        .with_stderr(&["the display could not be opened"]);
    let mut probe = probe_with(stream);
    let error = probe
        .next_commit(COMMIT_TIMEOUT)
        .expect_err("the client is gone");
    let message = error.to_string();
    assert!(message.contains("exit status: 1"), "{message}");
    assert!(
        !message.contains("the display could not be opened"),
        "the tail is attached as a field, not printed: {message}"
    );
    assert!(
        matches!(error, ReadbackError::ClientExited { .. }),
        "{error}"
    );
    assert_eq!(
        probe.stderr_tail(),
        ["the display could not be opened"],
        "and the case reads the tail from the probe"
    );
}

#[test]
fn test_next_commit_delivers_every_commit_once_and_in_order() {
    let expected: Vec<String> = (0..200).map(|index| format!("phrase {index}")).collect();
    let mut steps = Vec::new();
    for text in &expected {
        steps.push(commit_step(text));
    }
    let mut probe = probe(steps);
    for (index, text) in expected.iter().enumerate() {
        assert_eq!(
            probe.next_commit(COMMIT_TIMEOUT),
            Ok(text.clone()),
            "commit {index} arrives once, in order"
        );
    }
    let delivered = probe.transcript().commits();
    assert_eq!(delivered.len(), expected.len(), "none is lost");
    let mut ordered = Vec::new();
    for text in &expected {
        ordered.push(text.as_str());
    }
    assert_eq!(delivered, ordered, "none is reordered");
}

#[test]
fn test_preedit_returns_the_newest_state_of_a_burst() {
    let mut probe = probe(vec![
        preedit_step("n", 1),
        preedit_step("ni", 2),
        preedit_step("nihao", 5),
    ]);
    assert_eq!(
        probe.preedit(COMMIT_TIMEOUT),
        Ok(Some(String::from("nihao"))),
        "a preedit is a state, so the newest one is the answer"
    );
    assert_eq!(probe.transcript().newest_preedit(), Some("nihao"));
}

#[test]
fn test_preedit_returns_none_when_the_client_cleared_it() {
    let mut probe = probe(vec![preedit_step("ni", 2), cleared_step()]);
    assert_eq!(probe.preedit(COMMIT_TIMEOUT), Ok(None));
    assert_eq!(
        probe.transcript().newest_preedit(),
        Some(""),
        "a cleared preedit is an empty state, not a missing one"
    );
}

#[test]
fn test_preedit_reports_a_timeout_when_no_preedit_arrives() {
    let mut probe = probe(vec![commit_step("你好"), Step::Idle]);
    let error = probe
        .preedit(COMMIT_TIMEOUT)
        .expect_err("no preedit arrived");
    assert!(
        matches!(error, ReadbackError::PreeditTimeout { received: 2, .. }),
        "{error}"
    );
    assert_eq!(
        probe.transcript().commits(),
        ["你好"],
        "the commit that arrived while the preedit was waited for is kept for the next call"
    );
    assert_eq!(
        probe.next_commit(COMMIT_TIMEOUT),
        Ok(String::from("你好")),
        "and it is still delivered"
    );
}

#[test]
fn test_probe_refuses_to_read_after_close() {
    let mut probe = probe(vec![commit_step("你好")]);
    assert_eq!(probe.close(), Ok(()));
    assert_eq!(
        probe.next_commit(COMMIT_TIMEOUT),
        Err(ReadbackError::Closed),
        "a closed probe fails loudly rather than answering with a stream nobody writes to"
    );
    assert_eq!(probe.preedit(COMMIT_TIMEOUT), Err(ReadbackError::Closed));
    assert_eq!(probe.close(), Err(ReadbackError::Closed));
}

#[test]
fn test_spawn_waits_for_the_client_to_announce_itself() {
    let stream = FakeStream::new(vec![ready_step("preedit,panel"), commit_step("你好")]);
    let client = spec(both());
    let mut probe = start_with(&client, READY_TIMEOUT, launcher(vec![stream]))
        .expect("the client announced itself");
    assert_eq!(probe.ready().caps, client.caps());
    assert_eq!(
        probe.ready().window,
        Some(WINDOW),
        "the window the injection channel has to focus comes from here"
    );
    assert_eq!(probe.next_commit(COMMIT_TIMEOUT), Ok(String::from("你好")));
}

#[test]
fn test_spawn_keeps_the_events_written_before_the_announcement() {
    let stream = FakeStream::new(vec![preedit_step("ni", 2), ready_step("preedit,panel")]);
    let probe = start_with(&spec(both()), READY_TIMEOUT, launcher(vec![stream]))
        .expect("the client announced itself");
    assert_eq!(
        probe.transcript().newest_preedit(),
        Some("ni"),
        "an event written before the announcement is part of the transcript"
    );
    assert!(matches!(
        probe.transcript().events().last(),
        Some(ClientEvent::Ready(_))
    ));
}

#[test]
fn test_spawn_numbers_lines_after_the_announcement() {
    let stream = FakeStream::new(vec![
        ready_step("preedit,panel"),
        Step::Line(String::from("{")),
    ]);
    let mut probe = start_with(&spec(both()), READY_TIMEOUT, launcher(vec![stream]))
        .expect("the client announced itself");
    let error = probe
        .next_commit(COMMIT_TIMEOUT)
        .expect_err("the line is not an event");
    assert!(
        matches!(error, ReadbackError::Protocol { line: 2, .. }),
        "the announcement is line one, so the next line the client wrote is line two: {error}"
    );
}

#[test]
fn test_spawn_retries_a_client_that_missed_its_deadline_once() {
    let scripts = vec![
        FakeStream::new(vec![Step::Idle]),
        FakeStream::new(vec![ready_step("preedit,panel")]),
    ];
    let started = Arc::new(AtomicU32::new(0));
    let counting = Arc::clone(&started);
    let mut scripts = VecDeque::from(scripts);
    let launch = move |_spec: &ClientSpec| {
        counting.fetch_add(1, Ordering::Relaxed);
        let script = scripts.pop_front().expect("one script per attempt");
        let stream: Box<dyn ClientStream> = Box::new(script);
        Ok(stream)
    };
    let probe = start_with(&spec(both()), Duration::from_millis(50), launch)
        .expect("the second attempt announces itself");
    assert_eq!(
        started.load(Ordering::Relaxed),
        2,
        "a missed deadline is retried exactly once"
    );
    assert_eq!(probe.ready().caps, both());
}

#[test]
fn test_spawn_reports_a_client_that_never_announced_itself() {
    let scripts = vec![
        FakeStream::new(vec![Step::Idle]),
        FakeStream::new(vec![Step::Idle]).with_stderr(&["no display"]),
    ];
    let error = start_with(&spec(both()), Duration::from_millis(50), launcher(scripts))
        .expect_err("a client that never announces itself fails the run");
    let message = error.to_string();
    assert!(
        message.contains("2 attempt(s)"),
        "the refusal counts the attempts: {message}"
    );
    assert!(
        !message.contains("no display"),
        "the tail is attached, not printed: {message}"
    );
    match error {
        ReadbackError::ClientNotReady {
            attempts, stderr, ..
        } => {
            assert_eq!(attempts, 2);
            assert_eq!(stderr, ["no display"]);
        }
        other => panic!("a not-ready refusal was expected, not {other}"),
    }
}

#[test]
fn test_spawn_reports_a_client_that_died_before_announcing_itself() {
    let scripts = vec![
        FakeStream::new(vec![Step::End])
            .with_status("signal: 11 (SIGSEGV)")
            .with_stderr(&["segmentation fault"]),
        FakeStream::new(vec![ready_step("preedit,panel")]),
    ];
    let started = Arc::new(AtomicU32::new(0));
    let counting = Arc::clone(&started);
    let mut scripts = VecDeque::from(scripts);
    let launch = move |_spec: &ClientSpec| {
        counting.fetch_add(1, Ordering::Relaxed);
        let script = scripts.pop_front().expect("one script per attempt");
        let stream: Box<dyn ClientStream> = Box::new(script);
        Ok(stream)
    };
    let error = start_with(&spec(both()), READY_TIMEOUT, launch)
        .expect_err("a client that stopped writing is reported at once");
    assert!(
        matches!(error, ReadbackError::ClientExited { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("SIGSEGV"), "{error}");
    assert_eq!(
        started.load(Ordering::Relaxed),
        1,
        "a crash is not a missed deadline: the client is not started again"
    );
}

#[test]
fn test_spawn_refuses_a_capability_set_the_client_did_not_declare() {
    let stream = FakeStream::new(vec![ready_step("panel")]);
    let error = start_with(&spec(both()), READY_TIMEOUT, launcher(vec![stream]))
        .expect_err("the declared set is not the one the client was started with");
    assert!(
        matches!(error, ReadbackError::CapsMismatch { .. }),
        "{error}"
    );
    let message = error.to_string();
    assert!(message.contains("`preedit,panel`"), "{message}");
    assert!(message.contains("`panel`"), "{message}");
}

#[test]
fn test_spawn_reports_a_program_it_cannot_execute() {
    let missing =
        std::env::temp_dir().join(format!("rspinyin-no-such-client-{}", std::process::id()));
    assert!(
        !missing.exists(),
        "the test needs a path that is not a program"
    );
    let client = ClientSpec::new(missing, ":99", both());
    let error = CommitProbe::spawn(&client).expect_err("a program that is not there is refused");
    assert!(
        matches!(error, ReadbackError::ClientSpawn { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("no-such-client"), "{error}");
}

#[test]
fn test_transcript_answers_what_a_case_asserts_on() {
    let mut probe = probe(vec![
        preedit_step("ni", 2),
        commit_step("你好"),
        commit_step("世界"),
    ]);
    assert_eq!(probe.next_commit(COMMIT_TIMEOUT), Ok(String::from("你好")));
    assert_eq!(probe.next_commit(COMMIT_TIMEOUT), Ok(String::from("世界")));
    let transcript = probe.transcript();
    assert_eq!(transcript.commits(), ["你好", "世界"]);
    assert_eq!(transcript.newest_preedit(), Some("ni"));
    assert_eq!(
        transcript.len(),
        4,
        "the announcement, the preedit and both commits"
    );
    assert!(!transcript.is_empty());
    assert!(matches!(
        transcript.events().first(),
        Some(ClientEvent::Ready(ready)) if ready.window == Some(WINDOW)
    ));
    assert_eq!(
        transcript.commits().len(),
        2,
        "a commit is never delivered twice"
    );
}
