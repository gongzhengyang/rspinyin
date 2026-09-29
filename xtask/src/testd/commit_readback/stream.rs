//! The wire under the probe: the client's two output streams, and the JSON on one of them.
//!
//! Responsibility: turn a client process into a sequence of decoded events, and turn every way
//! that can go wrong -- a client that stops writing, one that never writes a newline, one that
//! writes something the protocol does not define -- into a refusal a case can act on. Nothing
//! here decides what a wait means; that is the probe's business.
//!
//! # Why a thread reads the pipe
//!
//! A pipe read cannot be given a deadline in safe Rust: `read` on a blocking descriptor blocks
//! until the client writes or closes it, and this workspace denies `unsafe`, so `poll(2)` is not
//! available here. The deadline is therefore moved to a channel -- a thread reads lines as they
//! arrive, and the probe waits on the channel with the timeout it was given -- which also keeps
//! the pipe drained, so a client that writes a lot cannot block on a full pipe while the harness
//! is busy asserting on something else.
//!
//! The thread is deliberately not joined when its pump is dropped. Dropping the receiver ends
//! it, and a client that left a copy of the write end open in a child of its own would otherwise
//! hang the harness at the end of a case rather than leave one thread to exit on its own.
//!
//! # What a refusal never repeats
//!
//! A line of this protocol can hold what the user typed, and a message is printed into a
//! terminal and archived as evidence. [`decode`] therefore reports the parser's category and
//! column rather than its message -- `serde_json` quotes the offending value in some of them --
//! and the line number it arrived on. A protocol token, which comes from a closed vocabulary, is
//! the only text a refusal here names.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::process::Child;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use serde::Deserialize;

use super::{ClientCaps, ClientEvent, ClientReady, ClientSpec, PreeditState, ReadbackError};

/// The most bytes one line of the protocol may take.
///
/// A commit is a phrase and an event is a handful of fields, so this is far past anything the
/// protocol needs; it is here so a client that never writes a newline cannot make the harness
/// allocate without bound. A longer line ends the stream with a refusal rather than being
/// truncated into an event nobody sent.
pub const MAX_EVENT_BYTES: usize = 64 * 1024;

/// How many lines of the client's standard error are kept.
///
/// A crash is diagnosed from the tail of what the client said, and a client in a loop can write
/// without bound. The oldest lines are dropped first.
pub const STDERR_LINES: usize = 200;

/// What a stream answered when the next line was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Arrival {
    /// The client wrote this line.
    Line(String),
    /// Nothing arrived within the timeout. The client may still write later.
    Idle,
    /// The stream is over: no further line will ever arrive.
    End,
}

/// The client's side of the wire, as the probe reads it.
///
/// The trait exists so every decision the probe makes -- which event is delivered, what a timeout
/// says, what a client that stopped mid-sentence reports -- is exercised against a scripted
/// stream, with no process, no display server and no clock. The one implementation that starts a
/// real process is [`ProcessStream`].
///
/// # Concurrency
///
/// A stream belongs to the case that opened it and is not shared between threads: every method
/// takes `&mut self`, and none of them is reentrant. [`ClientStream::next_line`] blocks for at
/// most the timeout it is given, which is a deadline and not a sleep: a line that is already
/// there is returned at once. A stream that has been closed refuses every further call rather
/// than answering an empty one.
pub(super) trait ClientStream: std::fmt::Debug {
    /// The next line the client wrote, waiting at most `timeout`.
    ///
    /// # Errors
    ///
    /// Returns [`ReadbackError::Stream`] when the output cannot be read, and
    /// [`ReadbackError::Closed`] when the stream has been closed. A client that has stopped
    /// writing is [`Arrival::End`], which is not an error: whether it is a failure depends on
    /// what the caller was waiting for.
    fn next_line(&mut self, timeout: Duration) -> Result<Arrival, ReadbackError>;

    /// How the client exited, when it has exited and been reaped.
    ///
    /// `None` means the client is still running, or that its status cannot be read. The second is
    /// not a separate verdict: a stream that has ended is over either way, and the standard error
    /// tail is what the diagnosis is built on.
    fn exit_status(&mut self) -> Option<String>;

    /// The tail of the client's standard error.
    fn stderr_tail(&self) -> Vec<String>;

    /// Ends the session.
    ///
    /// # Errors
    ///
    /// Returns [`ReadbackError::Closed`] when the stream was already closed.
    fn close(&mut self) -> Result<(), ReadbackError>;
}

/// Lines arriving from a reader on a thread of their own.
#[derive(Debug)]
pub(super) struct LinePump {
    /// The lines the reader thread has read, and the refusal that ended it.
    lines: Receiver<Result<String, String>>,
}

impl LinePump {
    /// Starts a thread that reads `reader` line by line.
    pub(super) fn start<R: Read + Send + 'static>(reader: R) -> Self {
        let (sender, lines) = mpsc::channel();
        let _reading = thread::spawn(move || pump_lines(reader, sender));
        Self { lines }
    }

    /// The next line the reader produced, waiting at most `timeout`.
    ///
    /// # Errors
    ///
    /// Returns [`ReadbackError::Stream`] when the reader refused a line, which ends the stream:
    /// the reader thread stops after reporting one.
    pub(super) fn next_line(&mut self, timeout: Duration) -> Result<Arrival, ReadbackError> {
        match self.lines.recv_timeout(timeout) {
            Ok(Ok(line)) => Ok(Arrival::Line(line)),
            Ok(Err(detail)) => Err(ReadbackError::Stream { detail }),
            Err(RecvTimeoutError::Timeout) => Ok(Arrival::Idle),
            // The thread is gone: it ended at end of file, or it reported a refusal and stopped.
            // Either way no further line will arrive.
            Err(RecvTimeoutError::Disconnected) => Ok(Arrival::End),
        }
    }
}

/// The client's standard output and standard error, as a live process writes them.
#[derive(Debug)]
pub(super) struct ProcessStream {
    /// The client process.
    child: Child,
    /// Its standard output, framed into lines.
    lines: LinePump,
    /// The tail of its standard error, filled by the thread that drains it.
    errors: Arc<Mutex<VecDeque<String>>>,
    /// How the client exited, once it has.
    exit: Option<String>,
    /// Whether the session has been closed.
    closed: bool,
}

impl ProcessStream {
    /// Starts the client `spec` names and binds a stream to it.
    ///
    /// # Errors
    ///
    /// Returns [`ReadbackError::ClientSpawn`] when the program cannot be executed, which covers a
    /// path that is not there, one that is not executable, and one that is not a program at all.
    /// The refusal is not retried by the probe: a program that cannot be started cannot be
    /// started on the second attempt either.
    pub(super) fn start(spec: &ClientSpec) -> Result<Self, ReadbackError> {
        let mut child = spawn(spec)?;
        // `ClientSpec::command` asks for a pipe on both streams, so neither branch below can be
        // reached by a caller; refusing loudly beats reading a stream that is not there.
        let Some(stdout) = child.stdout.take() else {
            return Err(no_pipe("standard output"));
        };
        let Some(stderr) = child.stderr.take() else {
            return Err(no_pipe("standard error"));
        };
        let errors = Arc::new(Mutex::new(VecDeque::new()));
        let sink = Arc::clone(&errors);
        // Draining standard error is what keeps a client that writes a lot to it from blocking
        // on a full pipe before it ever commits anything.
        let _draining = thread::spawn(move || pump_errors(stderr, sink));
        Ok(Self {
            child,
            lines: LinePump::start(stdout),
            errors,
            exit: None,
            closed: false,
        })
    }
}

impl ClientStream for ProcessStream {
    fn next_line(&mut self, timeout: Duration) -> Result<Arrival, ReadbackError> {
        if self.closed {
            return Err(ReadbackError::Closed);
        }
        self.lines.next_line(timeout)
    }

    fn exit_status(&mut self) -> Option<String> {
        if self.exit.is_none() {
            if let Ok(Some(status)) = self.child.try_wait() {
                self.exit = Some(status.to_string());
            }
        }
        self.exit.clone()
    }

    fn stderr_tail(&self) -> Vec<String> {
        lock(&self.errors).iter().cloned().collect()
    }

    fn close(&mut self) -> Result<(), ReadbackError> {
        if self.closed {
            return Err(ReadbackError::Closed);
        }
        self.closed = true;
        // Neither call can turn the session into a failure worth reporting: a client that has
        // already exited is reaped, and one that refuses to die is left to the operating system
        // rather than to a case that has nothing left to assert.
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }
}

/// Starts the client and maps a failed execution onto the channel's refusal.
fn spawn(spec: &ClientSpec) -> Result<Child, ReadbackError> {
    spec.command()
        .spawn()
        .map_err(|source| ReadbackError::ClientSpawn {
            program: spec.program().to_path_buf(),
            detail: source.to_string(),
        })
}

/// The refusal for a stream that was started without the pipe the protocol needs.
fn no_pipe(stream: &str) -> ReadbackError {
    ReadbackError::Stream {
        detail: format!("the client was started without a pipe on its {stream}"),
    }
}

/// Reads the client's standard output into lines on a thread of its own.
fn pump_lines<R: Read + Send + 'static>(reader: R, lines: Sender<Result<String, String>>) {
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        match read_bounded_line(&mut reader, &mut line) {
            Ok(true) => {
                // A send that fails means the probe is gone, which is the end of this thread's
                // work rather than a failure of it.
                if lines.send(Ok(trimmed(&line))).is_err() {
                    return;
                }
            }
            Ok(false) => return,
            Err(detail) => {
                let _ = lines.send(Err(detail));
                return;
            }
        }
    }
}

/// Keeps the tail of the client's standard error on a thread of its own.
pub(super) fn pump_errors<R: Read + Send + 'static>(reader: R, sink: Arc<Mutex<VecDeque<String>>>) {
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        match read_bounded_line(&mut reader, &mut line) {
            Ok(true) => push_bounded(&sink, trimmed(&line)),
            Ok(false) => return,
            Err(detail) => {
                // The lines already kept are the tail the archive needs; this says why there will
                // be no more of them, marked so it is not read as the client's own words.
                push_bounded(&sink, format!("[rspinyin harness] {detail}"));
                return;
            }
        }
    }
}

/// Reads one line, refusing one that is past the protocol's ceiling.
///
/// # Return value
/// `Ok(true)` when `line` holds one line, `Ok(false)` at end of input.
///
/// # Errors
/// Returns what to report as a plain message when the input cannot be read, and when a line is
/// longer than [`MAX_EVENT_BYTES`] without a newline in it. A refusal ends the stream rather than
/// being truncated into an event the client never wrote.
fn read_bounded_line<R: BufRead>(reader: &mut R, line: &mut String) -> Result<bool, String> {
    line.clear();
    let limit = u64::try_from(MAX_EVENT_BYTES).unwrap_or(u64::MAX);
    // The limit is re-armed for every line, so it bounds one event rather than the stream.
    let read = reader
        .by_ref()
        .take(limit)
        .read_line(line)
        .map_err(|error| error.to_string())?;
    if read == 0 {
        return Ok(false);
    }
    // A line that filled the limit without reaching a newline was longer than the protocol
    // allows; one that is shorter and unterminated simply ended at end of file.
    if !line.ends_with('\n') && line.len() >= MAX_EVENT_BYTES {
        return Err(format!(
            "a line of {MAX_EVENT_BYTES} bytes or more arrived without a newline; the protocol is \
             one JSON object per line"
        ));
    }
    Ok(true)
}

/// One line without its terminator.
fn trimmed(line: &str) -> String {
    line.trim_end_matches(['\n', '\r']).to_owned()
}

/// Appends one line to the tail, dropping the oldest past the bound.
fn push_bounded(sink: &Arc<Mutex<VecDeque<String>>>, line: String) {
    let mut lines = lock(sink);
    while lines.len() >= STDERR_LINES {
        lines.pop_front();
    }
    lines.push_back(line);
}

/// The lock on the standard error tail, tolerating a poisoned one.
fn lock(sink: &Arc<Mutex<VecDeque<String>>>) -> MutexGuard<'_, VecDeque<String>> {
    match sink.lock() {
        Ok(guard) => guard,
        // A poisoned lock still holds the lines the archive needs: the panic that poisoned it
        // happened on another thread and says nothing about what the client wrote.
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Reads one line of the protocol into the event it carries.
///
/// # Errors
///
/// Returns [`ReadbackError::Protocol`] for a line that is not JSON, for one holding a field the
/// event does not take, for one missing a field the event needs, and for an event this protocol
/// does not define. The message names the line and what is wrong with its shape; it never repeats
/// the line, which can hold what the user typed.
pub(super) fn decode(text: &str, line: usize) -> Result<ClientEvent, ReadbackError> {
    let wire: WireEvent = serde_json::from_str(text).map_err(|error| ReadbackError::Protocol {
        line,
        // The parser's own message can quote the offending value, which is exactly the content
        // this channel must not put into a message; its category and column locate the fault
        // without repeating it.
        detail: format!("{:?} at column {}", error.classify(), error.column()),
    })?;
    match wire.event.as_str() {
        "ready" => {
            let Some(caps) = wire.caps else {
                return Err(protocol(line, "a `ready` event without a `caps` field"));
            };
            let caps = ClientCaps::parse(&caps).map_err(|reason| protocol(line, &reason))?;
            Ok(ClientEvent::Ready(ClientReady {
                caps,
                window: wire.window,
            }))
        }
        "commit" => {
            let Some(text) = wire.text else {
                return Err(protocol(line, "a `commit` event without a `text` field"));
            };
            Ok(ClientEvent::Commit(text))
        }
        "preedit" => Ok(ClientEvent::Preedit(wire.text.map(|text| PreeditState {
            text,
            caret: wire.caret,
        }))),
        other => Err(protocol(
            line,
            &format!("`{other}` is not an event this protocol defines"),
        )),
    }
}

/// The refusal for a line whose shape the protocol does not allow.
fn protocol(line: usize, detail: &str) -> ReadbackError {
    ReadbackError::Protocol {
        line,
        detail: detail.to_owned(),
    }
}

/// One line of the client's output, as it is written on the wire.
///
/// The shape is a flat object rather than a tagged enum so that a field the event does not take is
/// refused by the parser itself, before anything decides what the event means. The fields are all
/// optional here and checked per event, which is what lets the refusal say *which* field is
/// missing.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireEvent {
    /// Which event this line is.
    event: String,
    /// The text a `commit` carries, or a `preedit`'s text; `null` clears a preedit.
    #[serde(default)]
    text: Option<String>,
    /// Where a `preedit`'s caret sits, when the host named a position.
    #[serde(default)]
    caret: Option<u32>,
    /// The capabilities a `ready` declares.
    #[serde(default)]
    caps: Option<String>,
    /// The window a `ready` mapped, when it names one.
    #[serde(default)]
    window: Option<u32>,
}
