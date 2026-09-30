//! What the host boundary does with the calls the routing layer makes.
//!
//! Every test drives [`EffectHost`] over a recording double, so the whole adapter — the
//! context id it carries, the two preedit shapes, the toggle and the two conditions that
//! leave through the crash channel — is verified without Fcitx5, a display server or a
//! dictionary. The double is the only implementation of [`HostCtx`] the tests need: the
//! production one is a set of C ABI calls that cannot run in this process.

use ime_types::{HideReason, ImeError, UiCommand};

use crate::engine::host::Host;

use super::{EffectHost, HostCtx};

/// The input context every test drives.
const IC: u64 = 7;

/// A host boundary that remembers what it was asked to do.
#[derive(Debug, Default)]
struct RecordingCtx {
    /// Every text handed to [`HostCtx::commit`], in order.
    commits: Vec<String>,
    /// Every preedit written, with its caret, in order.
    preedits: Vec<(String, u32)>,
    /// How many times the preedit area was emptied.
    clears: usize,
    /// Every command posted, by kind.
    posts: Vec<&'static str>,
    /// The state the host reports for this context.
    enabled: bool,
    /// Whether the next commit is refused, which is the error path.
    refuse_commit: bool,
    /// Whether the next state write is dropped, which models a host that cannot honour
    /// the call.
    refuse_set_enabled: bool,
}

impl HostCtx for RecordingCtx {
    fn commit(&mut self, text: &str) -> Result<(), ImeError> {
        if self.refuse_commit {
            return Err(ImeError::FfiInvalidCommit);
        }
        self.commits.push(text.to_owned());
        Ok(())
    }

    fn set_client_preedit(&mut self, text: &str, caret: u32) -> Result<(), ImeError> {
        self.preedits.push((text.to_owned(), caret));
        Ok(())
    }

    fn clear_client_preedit(&mut self) -> Result<(), ImeError> {
        self.clears += 1;
        Ok(())
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        if !self.refuse_set_enabled {
            self.enabled = enabled;
        }
    }

    fn post(&mut self, command: UiCommand) {
        self.posts.push(command_kind(&command));
    }
}

/// Names a command by its kind, so an assertion can compare without cloning a frame.
fn command_kind(command: &UiCommand) -> &'static str {
    match command {
        UiCommand::Frame(_) => "frame",
        UiCommand::Show { .. } => "show",
        UiCommand::Hide { .. } => "hide",
        UiCommand::Theme(_) => "theme",
        UiCommand::Overlay(_) => "overlay",
        UiCommand::Shutdown => "shutdown",
    }
}

/// A boundary over a fresh double.
fn boundary(ctx: &mut RecordingCtx) -> EffectHost<'_> {
    EffectHost::new(IC, ctx)
}

#[test]
fn test_effect_host_commit_reaches_the_context() {
    let mut ctx = RecordingCtx::default();
    boundary(&mut ctx).commit(IC, "你好");
    assert_eq!(
        ctx.commits,
        [String::from("你好")],
        "a commit is the text the session handed over, unchanged"
    );
}

#[test]
fn test_effect_host_refused_commit_is_swallowed_and_changes_nothing() {
    // The error path of the boundary: a host that refuses the text must not make the
    // caller unwind, and must not leave a half-applied commit behind.
    let mut ctx = RecordingCtx {
        refuse_commit: true,
        ..RecordingCtx::default()
    };
    boundary(&mut ctx).commit(IC, "nihao");
    assert!(
        ctx.commits.is_empty(),
        "a refused commit must not be recorded as one that happened"
    );
}

#[test]
fn test_effect_host_set_and_clear_preedit_stay_distinct() {
    let mut ctx = RecordingCtx::default();
    let mut host = boundary(&mut ctx);
    host.set_preedit(IC, "ni'hao", 6);
    host.clear_preedit(IC);
    assert_eq!(
        ctx.preedits,
        [(String::from("ni'hao"), 6)],
        "the composing text and its caret reach the application's preedit area"
    );
    assert_eq!(
        ctx.clears, 1,
        "emptying the area is its own call, not a write of empty text"
    );
}

#[test]
fn test_effect_host_toggle_enabled_flips_the_context_state() {
    let mut ctx = RecordingCtx {
        enabled: true,
        ..RecordingCtx::default()
    };
    // Each call takes a fresh boundary: the adapter holds `&mut ctx` for as long as it
    // lives, and the point of these assertions is to read the context between the two
    // calls. A temporary borrow is dropped at the end of its statement, which is what
    // makes the read legal without moving the state out from under the host.
    assert!(
        !boundary(&mut ctx).toggle_enabled(IC),
        "an enabled context turns off"
    );
    assert!(!ctx.enabled, "the state the host holds is the flipped one");
    assert!(
        boundary(&mut ctx).toggle_enabled(IC),
        "and turning it back on reports on"
    );
}

#[test]
fn test_effect_host_toggle_enabled_reports_the_state_a_refused_write_left() {
    // The boundary reads the state back rather than assuming the write landed. A host
    // that cannot switch must not be reported as having switched, because the status
    // strip is drawn from this answer.
    let mut ctx = RecordingCtx {
        enabled: true,
        refuse_set_enabled: true,
        ..RecordingCtx::default()
    };
    assert!(
        boundary(&mut ctx).toggle_enabled(IC),
        "the answer is the state the host is actually in"
    );
}

#[test]
fn test_effect_host_post_ui_forwards_the_command() {
    let mut ctx = RecordingCtx::default();
    let mut host = boundary(&mut ctx);
    host.post_ui(
        IC,
        UiCommand::Hide {
            revision: 3,
            reason: HideReason::Committed,
        },
    );
    host.post_ui(IC, UiCommand::Shutdown);
    assert_eq!(
        ctx.posts,
        ["hide", "shutdown"],
        "the ordered commands reach the window in the order the session produced them"
    );
}

#[test]
fn test_effect_host_diagnose_stays_off_the_context() {
    // A diagnostic is the plugin's own record, not work for the host's objects: it must
    // not turn into a commit, a preedit write or a command.
    let mut ctx = RecordingCtx::default();
    boundary(&mut ctx).diagnose(IC, &ImeError::UiChannelClosed);
    assert!(
        ctx.commits.is_empty() && ctx.preedits.is_empty() && ctx.posts.is_empty(),
        "reporting a condition must not reach the application or the window"
    );
}

#[test]
fn test_effect_host_context_is_the_one_it_was_built_for() {
    // The adapter's whole job is to carry the context id the routing layer cannot know:
    // it serves every context and passes the id on each call.
    let mut ctx = RecordingCtx::default();
    assert_eq!(boundary(&mut ctx).context(), IC);
    let mut other = RecordingCtx::default();
    assert_eq!(EffectHost::new(9, &mut other).context(), 9);
}
