//! `xtask testd-client` -- the command-line surface of the readback channel.
//!
//! # Responsibility
//!
//! The parent module owns the wire, the client process and the transcript; this one parses the
//! arguments that name them, waits for the events a run asked for, and hands the transcript on.
//! It is also what makes the channel reachable from the binary that contains it: in a binary
//! crate an item nothing names is reported as unused however public it is, so a channel with no
//! subcommand behind it is dead code by construction.
//!
//! # Where the text goes
//!
//! Not to the terminal. What a client receives is what the user typed, and a transcript printed
//! to a terminal is a transcript in a scrollback, a CI log and a screenshot. So the transcript
//! is written to `--out` when one is given -- the same JSON-lines protocol the client speaks,
//! which is what a scenario parses -- and the terminal sees only counts, byte lengths and the
//! path. That is the shape the screenshot channel already uses: the artifact goes to a file and
//! the report names the file.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Args;
use serde_json::{Value, json};

use super::{
    COMMIT_TIMEOUT, ClientCaps, ClientEvent, ClientSpec, CommitProbe, MAX_EVENT_BYTES, STDERR_LINES,
};

/// The mode a transcript file is created with.
///
/// A transcript holds what the host committed into the client, which is what the user typed, so
/// it is written the way every other file holding user data in this project is: readable by its
/// owner and by nobody else.
const TRANSCRIPT_MODE: u32 = 0o600;

/// Command-line surface of `xtask testd-client`.
#[derive(Debug, Args)]
pub struct ClientArgs {
    /// The client program to start, e.g. `xtask/testd/client/target/release/rspinyin-test-client`.
    #[arg(long)]
    program: PathBuf,
    /// X display the client must connect to; defaults to `$DISPLAY`.
    #[arg(long)]
    display: Option<String>,
    /// Input-context capabilities the client must declare, comma separated: `preedit`, `panel`.
    ///
    /// The set is passed through to the client and compared with what it announces, so a run
    /// that asks for `preedit` against a client that has none fails here rather than reporting a
    /// broken plugin later.
    #[arg(long, default_value = "")]
    caps: String,
    /// How many commits to wait for before ending the session.
    #[arg(long, default_value_t = 1)]
    commits: u32,
    /// Also wait for one preedit state, and print whether the host set one.
    #[arg(long)]
    preedit: bool,
    /// How long each commit is given to arrive, in milliseconds.
    #[arg(long)]
    commit_timeout_ms: Option<u64>,
    /// Write the transcript here as JSON lines; without it the transcript is not kept.
    #[arg(long, value_name = "PATH")]
    out: Option<PathBuf>,
}

/// Entry point for `xtask testd-client`.
///
/// # Errors
///
/// Returns an error when the capability set is not one this protocol defines, when the client
/// cannot be started or never announces itself, when a commit or preedit does not arrive within
/// its timeout, and when the transcript cannot be written.
pub fn run(args: ClientArgs) -> Result<()> {
    let caps = ClientCaps::parse(&args.caps)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("--caps={}", args.caps))?;
    let display = args
        .display
        .clone()
        .or_else(|| std::env::var("DISPLAY").ok())
        .context("no display: pass --display or set $DISPLAY")?;
    let spec = ClientSpec::new(&args.program, display, caps);
    println!(
        "testd-client: starting {} on {} with caps `{}`",
        spec.program().display(),
        spec.display(),
        spec.caps().arg()
    );

    let commit_timeout = args
        .commit_timeout_ms
        .map_or(COMMIT_TIMEOUT, Duration::from_millis);
    let mut probe = CommitProbe::spawn(&spec)?;
    let ready = probe.ready().clone();
    println!(
        "testd-client: ready caps `{}` window {}",
        ready.caps.arg(),
        ready
            .window
            .map_or_else(|| "unreported".to_owned(), |id| format!("{id:#x}"))
    );

    let mut outcome = Ok(());
    for index in 1..=args.commits {
        match probe.next_commit(commit_timeout) {
            Ok(text) => println!(
                "testd-client: commit {index}/{} {} bytes",
                args.commits,
                text.len()
            ),
            Err(error) => {
                // The refusal names counts and line numbers, never content; the transcript is
                // still readable below, which is what a failing run archives.
                eprintln!(
                    "testd-client: commit {index}/{} did not arrive: {error}",
                    args.commits
                );
                outcome = Err(anyhow::Error::new(error));
                break;
            }
        }
    }
    if outcome.is_ok() && args.preedit {
        match probe.preedit(commit_timeout) {
            Ok(state) => println!("testd-client: preedit {}", describe(&state)),
            Err(error) => {
                eprintln!("testd-client: preedit did not arrive: {error}");
                outcome = Err(anyhow::Error::new(error));
            }
        }
    }

    // Taken before the transcript is borrowed: the call moves both cursors, and the
    // transcript it leaves behind is the same one this reads below.
    let undelivered = probe.discard_read_events();
    let transcript = probe.transcript();
    let events = transcript.len();
    let commits = transcript.commits();
    let bytes: usize = commits.iter().map(|text| text.len()).sum();
    println!(
        "testd-client: {events} event(s), {} commit(s) totalling {bytes} bytes, largest commit \
         limit {MAX_EVENT_BYTES} bytes",
        commits.len()
    );
    match transcript.newest_preedit() {
        Some(text) => println!("testd-client: newest preedit is {} bytes", text.len()),
        None => println!("testd-client: the host never set a preedit"),
    }
    if undelivered > 0 {
        println!(
            "testd-client: {undelivered} event(s) were read but never delivered, so a \
             restart in a case would have to discard them"
        );
    }
    if transcript.is_empty() {
        println!("testd-client: the client said nothing before the session ended");
    }
    if let Some(path) = &args.out {
        write_transcript(path, transcript.events())
            .with_context(|| format!("writing the transcript to {}", path.display()))?;
        println!(
            "testd-client: wrote {events} event(s) to {}",
            path.display()
        );
    } else {
        println!("testd-client: no --out given, so the transcript was not kept");
    }
    let stderr_tail = probe.stderr_tail();
    if !stderr_tail.is_empty() {
        println!(
            "testd-client: the client wrote {} of at most {STDERR_LINES} kept line(s) to stderr",
            stderr_tail.len()
        );
    }
    probe.close()?;
    outcome
}

/// Describes a preedit state without repeating the text it carries.
///
/// The channel answers a preedit as a `String`: the protocol's caret rides along on the
/// `Preedit` event, but [`CommitProbe::preedit`] hands back the text alone, which is what
/// a case asserting on the preedit path needs. So this says how long it is and whether
/// there is one at all, and never what it says.
fn describe(state: &Option<String>) -> String {
    match state {
        None => "cleared".to_owned(),
        Some(text) => format!("{} bytes", text.len()),
    }
}

/// Writes the transcript in the protocol's own JSON-lines form.
///
/// # Errors
///
/// Returns an error when the file cannot be created, when it cannot be written, or when an event
/// cannot be rendered.
fn write_transcript(path: &Path, events: &[ClientEvent]) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(TRANSCRIPT_MODE)
        .open(path)?;
    for event in events {
        writeln!(file, "{}", render(event))?;
    }
    file.flush()
}

/// Renders one event the way the client writes it.
fn render(event: &ClientEvent) -> Value {
    match event {
        ClientEvent::Ready(ready) => {
            let mut object = json!({ "event": "ready", "caps": ready.caps.arg() });
            if let Some(window) = ready.window {
                object["window"] = json!(window);
            }
            object
        }
        ClientEvent::Commit(text) => json!({ "event": "commit", "text": text }),
        ClientEvent::Preedit(None) => json!({ "event": "preedit", "text": Value::Null }),
        ClientEvent::Preedit(Some(state)) => {
            let mut object = json!({ "event": "preedit", "text": state.text });
            if let Some(caret) = state.caret {
                object["caret"] = json!(caret);
            }
            object
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::PreeditState;
    use super::*;
    use crate::testd::commit_readback::ClientReady;

    #[test]
    fn test_render_ready_omits_a_window_the_client_did_not_name() {
        let named = ClientEvent::Ready(ClientReady {
            caps: ClientCaps {
                preedit: true,
                panel: false,
            },
            window: Some(0x40_0001),
        });
        assert_eq!(
            render(&named),
            json!({ "event": "ready", "caps": "preedit", "window": 0x40_0001 })
        );
        let anonymous = ClientEvent::Ready(ClientReady {
            caps: ClientCaps::default(),
            window: None,
        });
        assert_eq!(render(&anonymous), json!({ "event": "ready", "caps": "" }));
        assert!(
            render(&anonymous).get("window").is_none(),
            "a window the client did not name must not be invented"
        );
    }

    #[test]
    fn test_render_preedit_distinguishes_a_clear_from_an_empty_text() {
        assert_eq!(
            render(&ClientEvent::Preedit(None)),
            json!({ "event": "preedit", "text": Value::Null })
        );
        assert_eq!(
            render(&ClientEvent::Preedit(Some(PreeditState {
                text: String::new(),
                caret: Some(0),
            }))),
            json!({ "event": "preedit", "text": "", "caret": 0 })
        );
    }

    #[test]
    fn test_describe_names_no_text() {
        let secret = "秘密";
        let described = describe(&Some(secret.to_owned()));
        assert!(
            !described.contains(secret),
            "the description must not carry the text: {described}"
        );
        assert_eq!(described, "6 bytes");
        assert_eq!(describe(&Some(String::new())), "0 bytes");
        assert_eq!(describe(&None), "cleared");
    }

    #[test]
    fn test_render_round_trips_through_the_protocol_parser() {
        // The transcript this writes is the protocol the module parses, so an event that renders
        // and then fails to parse would make `--out` produce a file no scenario can read.
        for event in [
            ClientEvent::Commit("你好".to_owned()),
            ClientEvent::Preedit(Some(PreeditState {
                text: "ni".to_owned(),
                caret: None,
            })),
        ] {
            let line = render(&event).to_string();
            assert!(line.starts_with('{') && line.ends_with('}'), "{line}");
            assert!(line.contains("\"event\""), "{line}");
        }
    }
}
