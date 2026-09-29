//! The client-text channel: the text an application really committed.
//!
//! Responsibility: start the client under test, read the events it writes to its standard
//! output, and hand a case the commits it received. An assertion made here is about what the
//! application got. The `UiFrame` snapshot and the log are both about what the plugin believed
//! it sent, and the commit path is the end of the product's whole chain -- the easiest place to
//! be wrong while every internal view still looks right.
//!
//! Reading the screen instead, by OCR over a terminal, is deliberately excluded: it cannot tell
//! "the application received the text" from "the candidate window happens to show the same
//! text", it depends on font rendering and antialiasing, and its accuracy on CJK is not
//! something an assertion can be built on. So the text is read from a real client process:
//! [`ClientSpec`] names the program, the program writes one JSON object per line to its standard
//! output, and [`CommitProbe`] reads those lines back.
//!
//! # The line protocol
//!
//! One JSON object per line, in the order the events happened, flushed after each one:
//!
//! ```json
//! {"event":"ready","caps":"preedit,panel","window":4194305}
//! {"event":"preedit","text":"ni","caret":2}
//! {"event":"commit","text":"你好"}
//! ```
//!
//! | Event | Fields | Meaning |
//! |---|---|---|
//! | `ready` | `caps` (required), `window` (optional) | the window is mapped and the client has declared the capabilities it was started with |
//! | `preedit` | `text` (`null` clears it), `caret` (optional) | the host set the client's preedit |
//! | `commit` | `text` (required) | the host committed this text into the client |
//!
//! The program is started as `<program> --display=<display> --caps=<cap,cap>`, with `DISPLAY`
//! set to the same display; [`ClientSpec::command`] builds exactly that. Anything else on the
//! wire -- a line that is not JSON, an event the protocol does not define, a field the event
//! does not take, a field the event needs and does not have -- is refused rather than skipped: a
//! harness that ignored what it could not read would report "no commit arrived" for a client
//! that had been committing all along.
//!
//! # Commits are events, the preedit is state
//!
//! [`CommitProbe::next_commit`] delivers every commit exactly once, in the order the client
//! received it. [`CommitProbe::preedit`] answers with the newest preedit state and discards the
//! ones it overtook, because a preedit is a state the client is in and a commit is an event that
//! happened. The contract's own queues draw the same line between a latest-wins slot and an
//! ordered channel. "No preedit arrived" is asserted by expecting
//! [`ReadbackError::PreeditTimeout`], which is what the `client_preedit = false` half of the
//! policy is checked with; the error carries the events that did arrive, so a run that received
//! the wrong text is diagnosable.
//!
//! # What reaches a message, and what does not
//!
//! Committed and preedit text is what a case asserts on, so the probe returns it and keeps it in
//! the [`Transcript`]. It never reaches a log line and never reaches an error message: a message
//! is printed into a terminal and archived as evidence, and the project's logging rules put
//! input content on the redaction denylist for exactly that reason. Every refusal here therefore
//! carries its content as a structured field -- the transcript, the client's standard error --
//! while the message it renders names counts, line numbers, capability sets and exit statuses. A
//! case that needs to know *what* arrived reads the field; a developer reading a terminal is
//! told where to look.
//!
//! # What this module does not do
//!
//! It injects no key, reads no pixel and walks no accessibility tree: the injection channel, the
//! screenshot channel and the frame snapshot do those, and none of the four stands in for
//! another. It links no toolkit, opens no socket and talks to no display server itself, which is
//! what lets its tests run against a scripted stream with no X server and no Fcitx5 present.
//!
//! # Modules
//!
//! `stream` holds the wire -- the stream the probe reads, the process behind it and the JSON
//! decoder -- `error` holds the refusals, and `cli` is the subcommand that drives the channel
//! from a command line.

mod cli;
mod error;
mod stream;

#[cfg(test)]
mod tests;

pub use self::cli::{ClientArgs, run};
pub use self::error::ReadbackError;
pub use self::stream::{MAX_EVENT_BYTES, STDERR_LINES};

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use self::stream::{Arrival, ClientStream, ProcessStream};

/// How long a client is given to map its window and announce itself.
///
/// The same budget the Fcitx5 session gets for the same kind of wait, and for the same reason:
/// it bounds how long a case may hang and predicts nothing about how long a start takes. A
/// client that misses it is started once more (see [`CommitProbe::spawn`]).
pub const READY_TIMEOUT: Duration = Duration::from_secs(5);

/// The timeout a case should pass to [`CommitProbe::next_commit`].
///
/// A commit follows the keys that caused it by microseconds; a second of slack is there for a
/// machine that is busy with something else, and a case that has to wait longer than this is
/// asserting on something other than the commit path.
pub const COMMIT_TIMEOUT: Duration = Duration::from_secs(2);

/// How many times a client is started before the harness gives up on it.
const START_ATTEMPTS: u32 = 2;

/// The name the protocol gives the preedit capability.
const PREEDIT_NAME: &str = "preedit";

/// The name the protocol gives the client-side input panel capability.
const PANEL_NAME: &str = "panel";

/// The input-context capabilities the client under test declares to the host.
///
/// The two flags decide which paths a run can reach at all: a host only calls `setPreedit` on an
/// input context that declared `Preedit`, and only a context that declared
/// `ClientSideInputPanel` shows a panel of its own. A case that asserted on the preedit path
/// against a client that never declared it would report a broken plugin where the harness had
/// configured the wrong client, so the set is named on both sides of the protocol and compared
/// when the client announces itself.
///
/// The flags are independent and cannot be put into an inconsistent state, so they are named
/// public fields rather than accessors, like the geometry values in [`crate::testd::coords`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClientCaps {
    /// Whether the host may write composing text into the client.
    pub preedit: bool,
    /// Whether the client draws the input panel itself.
    pub panel: bool,
}

impl ClientCaps {
    /// Reads a capability set written the way the protocol writes it.
    ///
    /// Surrounding spaces and empty names are ignored, so `"preedit, panel"` and
    /// `"preedit,panel"` are the same set, and the empty string is no capability at all.
    ///
    /// # Errors
    ///
    /// Returns the reason as a plain message when a name is not one this protocol defines; the
    /// caller adds the line the name arrived on. A name is a protocol token from a closed
    /// vocabulary and never content, which is what makes naming it in the message safe.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut caps = Self::default();
        for name in text
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            match name {
                PREEDIT_NAME => caps.preedit = true,
                PANEL_NAME => caps.panel = true,
                other => {
                    return Err(format!(
                        "`{other}` is not a capability this protocol defines; it defines \
                         `{PREEDIT_NAME}` and `{PANEL_NAME}`"
                    ));
                }
            }
        }
        Ok(caps)
    }

    /// The set written the way the protocol writes it, preedit first.
    pub fn arg(&self) -> String {
        let mut names = Vec::new();
        if self.preedit {
            names.push(PREEDIT_NAME);
        }
        if self.panel {
            names.push(PANEL_NAME);
        }
        names.join(",")
    }

    /// Whether every capability of `other` is in this set.
    ///
    /// The empty set is contained by every set, the way a subset works, so a case that asked for
    /// no capability is satisfied by any client.
    pub const fn contains(self, other: Self) -> bool {
        (!other.preedit || self.preedit) && (!other.panel || self.panel)
    }
}

impl std::fmt::Display for ClientCaps {
    /// Writes the set as the protocol writes it, or `(none)` for the empty set.
    ///
    /// The empty set would otherwise render as an empty string, which reads as a missing value
    /// rather than as a client that declared nothing.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.preedit || self.panel {
            return formatter.write_str(&self.arg());
        }
        formatter.write_str("(none)")
    }
}

/// How the client under test is started.
///
/// The program is not the harness's to find: a case names it, so the same channel drives the
/// GTK, Qt or XIM client a run was built against without this module knowing which one it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientSpec {
    /// The client program.
    program: PathBuf,
    /// The X display it must connect to.
    display: String,
    /// The input-context capabilities it must declare.
    caps: ClientCaps,
}

impl ClientSpec {
    /// Names a client to start.
    pub fn new(program: impl Into<PathBuf>, display: impl Into<String>, caps: ClientCaps) -> Self {
        Self {
            program: program.into(),
            display: display.into(),
            caps,
        }
    }

    /// The client program.
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// The display the client is told to connect to.
    pub fn display(&self) -> &str {
        &self.display
    }

    /// The capabilities the client must declare.
    pub fn caps(&self) -> ClientCaps {
        self.caps
    }

    /// The command that starts the client.
    ///
    /// The shape is the other half of the protocol this module documents: the program is given
    /// `--display=<display>` and `--caps=<cap,cap>`, and the display is exported as `DISPLAY` as
    /// well, because a toolkit reads it from the environment while the client's own code reads
    /// the flag. Both are set to the same value, so a client cannot connect somewhere the case
    /// did not name -- in particular not to the operator's own session.
    ///
    /// Standard input is closed, so a client that reads it sees end of file instead of waiting on
    /// a terminal that is not there, and both output streams are pipes, which the stream takes
    /// over.
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .arg(format!("--display={}", self.display))
            .arg(format!("--caps={}", self.caps.arg()))
            .env("DISPLAY", &self.display)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
}

/// What the client announced about itself when its window was mapped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientReady {
    /// The capabilities the client declared it has.
    pub caps: ClientCaps,
    /// The window it mapped, when it named one.
    ///
    /// This is the window the injection channel has to focus, which is why the client reports
    /// it: the two channels compose into one case -- start the client, take its window from
    /// here, type into it, and read the commits back from the same probe.
    pub window: Option<u32>,
}

/// The client's preedit at one moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreeditState {
    /// The text the host set as the preedit.
    pub text: String,
    /// The caret position the host asked for, when it named one.
    pub caret: Option<u32>,
}

/// One event the client reported, in the order it reported it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientEvent {
    /// The client's window was mapped and it declared its capabilities.
    Ready(ClientReady),
    /// The host committed this text into the client.
    Commit(String),
    /// The host set the client's preedit, or cleared it with `None`.
    Preedit(Option<PreeditState>),
}

/// Everything one client said, in order.
///
/// The transcript is the evidence a case asserts on and the record a failure is diagnosed from,
/// which is why the probe keeps every event it read rather than only the ones a call happened to
/// return. It is deliberately not `Display`: it holds what the user typed, and a transcript that
/// could be interpolated into a message would be a transcript in a terminal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Transcript {
    /// The events, in the order the client wrote them.
    events: Vec<ClientEvent>,
}

impl Transcript {
    /// How many events the client reported.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the client reported nothing at all.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Every event, in order.
    pub fn events(&self) -> &[ClientEvent] {
        &self.events
    }

    /// The texts the host committed into the client, in order.
    ///
    /// This is what a case asserts on: a commit is an event, and an assertion about the commit
    /// path is an assertion about this sequence.
    pub fn commits(&self) -> Vec<&str> {
        self.events
            .iter()
            .filter_map(|event| match event {
                ClientEvent::Commit(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The newest preedit text the client was given.
    ///
    /// `Some("")` means the newest preedit state is empty -- the host cleared it -- and `None`
    /// means the client was never given a preedit at all. A case that has to tell those two apart
    /// reads [`Transcript::events`], where a clear is a `Preedit(None)` event and a client that
    /// never received one has no `Preedit` event in the transcript.
    pub fn newest_preedit(&self) -> Option<&str> {
        self.events.iter().rev().find_map(|event| match event {
            ClientEvent::Preedit(state) => {
                Some(state.as_ref().map_or("", |state| state.text.as_str()))
            }
            _ => None,
        })
    }

    /// Appends an event.
    fn push(&mut self, event: ClientEvent) {
        self.events.push(event);
    }
}

/// The readback channel for one client under test.
///
/// The probe owns the client process, the transcript of everything it said, and the cursors that
/// decide what the next call returns. It is the only thing a case needs to hold.
#[derive(Debug)]
pub struct CommitProbe {
    /// The client's side of the wire.
    stream: Box<dyn ClientStream>,
    /// Everything the client said, in order.
    transcript: Transcript,
    /// What the client announced about itself.
    ready: ClientReady,
    /// How many lines have been read, for the line numbers a refusal names.
    lines_read: usize,
    /// Index of the next commit in the transcript that has not been delivered.
    commit_cursor: usize,
    /// Index of the next preedit in the transcript that has not been considered.
    preedit_cursor: usize,
    /// Whether the session has been closed.
    closed: bool,
}

impl CommitProbe {
    /// Starts a client, waits for it to announce itself, and binds a probe to it.
    ///
    /// A client that misses [`READY_TIMEOUT`] is started once more before the run fails: the
    /// first attempt usually lost a race with the display server, and the second is the
    /// documented recovery. A client that stops writing instead is reported at once -- its
    /// standard error says why, and starting it again would only repeat that.
    ///
    /// # Errors
    ///
    /// Returns [`ReadbackError::ClientSpawn`] when the program cannot be executed,
    /// [`ReadbackError::ClientNotReady`] when it never announces itself,
    /// [`ReadbackError::ClientExited`] when it stops before it does, and
    /// [`ReadbackError::CapsMismatch`] when it does not declare every capability it was started
    /// with.
    pub fn spawn(spec: &ClientSpec) -> Result<Self, ReadbackError> {
        start_with(spec, READY_TIMEOUT, live)
    }

    /// What the client announced about itself.
    pub fn ready(&self) -> &ClientReady {
        &self.ready
    }

    /// Everything the client has said so far.
    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// The tail of the client's standard error.
    ///
    /// This is what a failing case archives: a client that died left its reason here, and the
    /// refusal that says it died names the count and leaves the text to this call.
    pub fn stderr_tail(&self) -> Vec<String> {
        self.stream.stderr_tail()
    }

    /// Waits for the next commit and returns the text the client received.
    ///
    /// Events that are not commits are recorded in the transcript and passed over, so a run with
    /// `client_preedit` on reads the same commits as one with it off. Each commit is delivered
    /// once, in the order the client received it.
    ///
    /// # Errors
    ///
    /// Returns [`ReadbackError::CommitTimeout`] when no commit arrives within `timeout`, with
    /// everything that did arrive attached, [`ReadbackError::ClientExited`] when the client stops
    /// writing, [`ReadbackError::Protocol`] when a line is not an event, and
    /// [`ReadbackError::Closed`] when the probe has been closed.
    pub fn next_commit(&mut self, timeout: Duration) -> Result<String, ReadbackError> {
        self.check_open()?;
        let deadline = deadline_after(timeout);
        loop {
            if let Some(text) = self.take_commit() {
                return Ok(text);
            }
            // The deadline is checked here as well as passed to the read: a client that writes
            // faster than the loop reads would otherwise keep it turning forever, because every
            // read would find another line already waiting.
            let left = remaining(deadline);
            if left.is_zero() {
                return Err(self.commit_timeout(timeout));
            }
            let arrival = self.pull(left)?;
            match arrival {
                Arrival::Line(_) => {}
                Arrival::Idle => return Err(self.commit_timeout(timeout)),
                Arrival::End => return Err(self.exited()),
            }
        }
    }

    /// Waits for a preedit and returns the newest state the client was given.
    ///
    /// A preedit is a state rather than an event, so a burst of updates is answered with the last
    /// of them: a case that typed `nihao` asks what the client's preedit is now, not what it was
    /// after the first key. `Ok(None)` means the host cleared it.
    ///
    /// # Errors
    ///
    /// Returns [`ReadbackError::PreeditTimeout`] when no preedit arrives within `timeout`, which
    /// is also the refusal a `client_preedit = false` case asserts on,
    /// [`ReadbackError::ClientExited`] when the client stops writing,
    /// [`ReadbackError::Protocol`] when a line is not an event, and [`ReadbackError::Closed`] when
    /// the probe has been closed.
    pub fn preedit(&mut self, timeout: Duration) -> Result<Option<String>, ReadbackError> {
        self.check_open()?;
        let deadline = deadline_after(timeout);
        loop {
            // The answer is the *newest* preedit, so the loop keeps reading while the
            // client is still writing: a burst of updates reaches the probe one line at a
            // time, and returning on the first of them would answer with the state the
            // client had after the first key rather than the one it is in now.
            let left = remaining(deadline);
            if left.is_zero() {
                return self.settled_preedit(timeout);
            }
            match self.pull(left)? {
                Arrival::Line(_) => {}
                // Nothing more is waiting, so whatever the client last said is what it is
                // in. An idle stream with no preedit at all is the timeout.
                Arrival::Idle => return self.settled_preedit(timeout),
                Arrival::End => return Err(self.exited()),
            }
        }
    }

    /// The newest preedit the transcript holds, or the refusal a caller waiting for one gets.
    fn settled_preedit(&mut self, timeout: Duration) -> Result<Option<String>, ReadbackError> {
        self.take_preedit()
            .ok_or_else(|| self.preedit_timeout(timeout))
    }

    /// Ends the session and reaps the client.
    ///
    /// The transcript and the standard error tail stay readable afterwards, which is what a
    /// failing case archives, and every read refuses once the probe is closed rather than
    /// answering with the silence of a stream nobody is writing to.
    ///
    /// # Errors
    ///
    /// Returns [`ReadbackError::Closed`] when the probe has already been closed.
    pub fn close(&mut self) -> Result<(), ReadbackError> {
        self.check_open()?;
        self.closed = true;
        self.stream.close()
    }

    /// Binds a probe to a stream that has already announced itself.
    fn adopt(stream: Box<dyn ClientStream>, ready: ClientReady, leading: Vec<ClientEvent>) -> Self {
        let mut transcript = Transcript::default();
        for event in leading {
            transcript.push(event);
        }
        transcript.push(ClientEvent::Ready(ready.clone()));
        Self {
            stream,
            transcript,
            ready,
            lines_read: 0,
            commit_cursor: 0,
            preedit_cursor: 0,
            closed: false,
        }
    }

    /// Drops every event the probe has read but not yet delivered, and says how many.
    ///
    /// This is what a case that restarts the host calls between the two halves of its
    /// run. A restart makes everything the client said before it irrelevant -- those
    /// commits belong to a session that no longer exists -- and a probe that kept them
    /// would hand the next assertion an answer the old session produced. The transcript
    /// itself is not touched: it is the evidence a failure is diagnosed from, and
    /// discarding the cursor is not the same as discarding the record.
    ///
    /// The count is how many events were still reachable by a reader and are not any
    /// more, which is what lets a case assert that the restart landed where it thought
    /// it did.
    pub fn discard_read_events(&mut self) -> usize {
        // The two readers have their own cursors, so the first event either of them could
        // still return is the lower of the two. Everything from there on is what this
        // call takes away.
        let reachable = self.commit_cursor.min(self.preedit_cursor);
        self.commit_cursor = self.transcript.len();
        self.preedit_cursor = self.transcript.len();
        self.transcript.len() - reachable
    }

    /// Refuses every read once the probe is closed.
    fn check_open(&self) -> Result<(), ReadbackError> {
        if self.closed {
            return Err(ReadbackError::Closed);
        }
        Ok(())
    }

    /// Reads one line and records the event it carried.
    fn pull(&mut self, timeout: Duration) -> Result<Arrival, ReadbackError> {
        let arrival = self.stream.next_line(timeout)?;
        if let Arrival::Line(text) = &arrival {
            self.lines_read += 1;
            let event = stream::decode(text, self.lines_read)?;
            self.transcript.push(event);
        }
        Ok(arrival)
    }

    /// Takes the first commit the transcript has not delivered yet.
    fn take_commit(&mut self) -> Option<String> {
        let waiting = self.waiting_from(self.commit_cursor);
        let found = waiting
            .iter()
            .enumerate()
            .find_map(|(offset, event)| match event {
                ClientEvent::Commit(text) => Some((offset, text.clone())),
                _ => None,
            });
        let (offset, text) = found?;
        self.commit_cursor += offset + 1;
        Some(text)
    }

    /// Takes the newest preedit the transcript has not considered yet.
    ///
    /// The whole run of preedits is consumed, not just the one returned: a preedit is a state the
    /// client is in, so the ones a later update overtook are history.
    fn take_preedit(&mut self) -> Option<Option<String>> {
        let waiting = self.waiting_from(self.preedit_cursor);
        let found = waiting.iter().rev().find_map(|event| match event {
            ClientEvent::Preedit(state) => Some(state.as_ref().map(|s| s.text.clone())),
            _ => None,
        });
        if found.is_some() {
            self.preedit_cursor = self.transcript.len();
        }
        found
    }

    /// The events at and after `cursor`.
    fn waiting_from(&self, cursor: usize) -> &[ClientEvent] {
        self.transcript.events().get(cursor..).unwrap_or(&[])
    }

    /// The refusal for a wait that no commit ended, with everything that did arrive.
    fn commit_timeout(&self, timeout: Duration) -> ReadbackError {
        ReadbackError::CommitTimeout {
            timeout,
            received: self.transcript.len(),
            transcript: self.transcript.clone(),
        }
    }

    /// The refusal for a wait that no preedit ended, with everything that did arrive.
    fn preedit_timeout(&self, timeout: Duration) -> ReadbackError {
        ReadbackError::PreeditTimeout {
            timeout,
            received: self.transcript.len(),
            transcript: self.transcript.clone(),
        }
    }

    /// The refusal for a client that stopped writing.
    fn exited(&mut self) -> ReadbackError {
        let status = self
            .stream
            .exit_status()
            .unwrap_or_else(|| String::from("still running"));
        ReadbackError::ClientExited {
            status,
            stderr: self.stream.stderr_tail(),
        }
    }
}

/// Starts a real client.
fn live(spec: &ClientSpec) -> Result<Box<dyn ClientStream>, ReadbackError> {
    Ok(Box::new(ProcessStream::start(spec)?))
}

/// Starts a client and waits for it to announce itself, retrying a missed deadline once.
fn start_with<L>(
    spec: &ClientSpec,
    ready_timeout: Duration,
    mut launch: L,
) -> Result<CommitProbe, ReadbackError>
where
    L: FnMut(&ClientSpec) -> Result<Box<dyn ClientStream>, ReadbackError>,
{
    let mut attempt = 0;
    loop {
        attempt += 1;
        // A program that cannot be executed is not retried: the second attempt would report the
        // same thing, and the caller needs to read it now.
        let mut stream = launch(spec)?;
        let outcome = await_ready(stream.as_mut(), ready_timeout);
        match outcome {
            Ok(announcement) => {
                // The check is containment, not equality: what a case must not do is assert on a
                // path the client never declared, which is the direction that turns a harness
                // mistake into a report of a broken plugin. A client that declares *more* than it
                // was started with is a client that declared something extra, and refusing it
                // would reject a run over a fact that cannot make its assertions wrong.
                if !announcement.ready.caps.contains(spec.caps) {
                    let _ = stream.close();
                    return Err(ReadbackError::CapsMismatch {
                        asked: spec.caps,
                        declared: announcement.ready.caps,
                    });
                }
                let mut probe =
                    CommitProbe::adopt(stream, announcement.ready, announcement.leading);
                // The lines the client wrote before it announced itself are numbered too, so a
                // refusal later in the run names the line the client actually wrote.
                probe.lines_read = announcement.lines;
                return Ok(probe);
            }
            Err(ReadbackError::ClientNotReady {
                timeout, stderr, ..
            }) => {
                // A client that cannot be closed is the operating system's problem, and the
                // reason the attempt failed is already in hand.
                let _ = stream.close();
                if attempt >= START_ATTEMPTS {
                    return Err(ReadbackError::ClientNotReady {
                        timeout,
                        attempts: attempt,
                        stderr,
                    });
                }
                println!(
                    "testd: the client did not announce itself within {timeout:?}; starting it \
                     once more"
                );
            }
            Err(error) => {
                let _ = stream.close();
                return Err(error);
            }
        }
    }
}

/// What a client said before it announced itself, and how many lines that took.
///
/// A client is free to write events before its `ready` line -- the protocol does not
/// require it to announce itself first -- and those events belong to the transcript in
/// the order they arrived. The line count travels with them because a refusal later in
/// the run names the line the client actually wrote, and the lines before `ready` are
/// part of that count.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Announcement {
    /// What the client announced about itself.
    ready: ClientReady,
    /// The events it wrote before announcing itself, in order.
    leading: Vec<ClientEvent>,
    /// How many lines had been read when `ready` arrived, `ready` included.
    lines: usize,
}

/// Waits for a client's `ready` line, keeping what it wrote before it.
fn await_ready(
    stream: &mut dyn ClientStream,
    timeout: Duration,
) -> Result<Announcement, ReadbackError> {
    let deadline = deadline_after(timeout);
    let mut leading = Vec::new();
    let mut lines = 0;
    loop {
        // The deadline is checked here as well as passed to the read: a client that writes events
        // without ever announcing itself would otherwise keep this loop turning.
        let left = remaining(deadline);
        if left.is_zero() {
            return Err(not_ready(stream, timeout));
        }
        let arrival = stream.next_line(left)?;
        match arrival {
            Arrival::Line(text) => {
                lines += 1;
                match stream::decode(&text, lines)? {
                    ClientEvent::Ready(ready) => {
                        return Ok(Announcement {
                            ready,
                            leading,
                            lines,
                        });
                    }
                    event => leading.push(event),
                }
            }
            Arrival::Idle => return Err(not_ready(stream, timeout)),
            Arrival::End => {
                return Err(ReadbackError::ClientExited {
                    status: stream
                        .exit_status()
                        .unwrap_or_else(|| String::from("still running")),
                    stderr: stream.stderr_tail(),
                });
            }
        }
    }
}

/// The refusal for a client that did not announce itself before its deadline.
fn not_ready(stream: &dyn ClientStream, timeout: Duration) -> ReadbackError {
    ReadbackError::ClientNotReady {
        timeout,
        // `start_with` stamps the attempt count it knows; a wait on its own does not know how
        // many attempts it is part of.
        attempts: 0,
        stderr: stream.stderr_tail(),
    }
}

/// The instant a timeout of `timeout` from now falls on.
///
/// The addition is checked because `Instant + Duration` panics on overflow, and a panic in the
/// harness is a case that fails for a reason nobody can read. A timeout large enough to overflow
/// is longer than the machine will exist, so falling back to now is the honest answer: every wait
/// then ends at its first deadline check.
fn deadline_after(timeout: Duration) -> Instant {
    Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now)
}

/// How much of a deadline is left, never negative.
fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}
