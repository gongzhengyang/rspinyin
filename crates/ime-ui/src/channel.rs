//! The cross-thread channels of the UI thread.
//!
//! Two directions, one rule: what happens when a channel is full is part of the
//! contract, not an implementation detail. The candidate window must never be
//! left showing stale candidates because a frame was refused, and it must never
//! be left visible because a `Hide` was collapsed against a `Show`.
//!
//! # Three semantics, three overflow rules
//!
//! Every channel on this boundary belongs to exactly one of the three classes
//! [`ChannelSemantics`] names, and the class -- not the implementation -- is what
//! decides what a full channel does:
//!
//! * a **data stream** carries a complete snapshot, so the newest value
//!   supersedes the rest and collapsing is lossless;
//! * a **command** is a request that has to be answered, so it is ordered and is
//!   never silently dropped;
//! * an **event** is a one-way notification whose loss changes what the user
//!   sees, so it is ordered as well.
//!
//! The classification is not decoration: it is bound to each channel by one of
//! the `*_CHANNEL_SEMANTICS` constants below, so a channel cannot quietly change
//! class, and the contract tests at the bottom of this file assert that the shape
//! each channel actually has is the shape its class calls for.
//!
//! # The channel contract
//!
//! One row per channel. This is the boundary contract of `features.md` 2.2.1 /
//! 2.2.2 transcribed, and every row is asserted by the contract tests:
//!
//! | Channel | Semantics | Shape | Capacity | Overflow behaviour |
//! |---|---|---|---|---|
//! | `UiCommand::Frame` | data stream | latest-wins slot | 1 | older frame replaced, counted as `ui.frame.coalesced` |
//! | `UiCommand::Show` / `Hide` | command | ordered collapsing queue | 8 | the newest value is staged and the sender returns, counted as `ui.control.dropped` |
//! | `UiCommand::Theme` | data stream | latest-wins slot | 1 | older value replaced |
//! | `UiCommand::Overlay` | data stream | latest-wins slot | 1 | older value replaced; `None` closes the overlay |
//! | `UiCommand::Shutdown` | command | flag | 1 | terminal, and deliberately not shared with any other channel |
//! | `UiEvent::Select` | command | bounded ring | 64 | never dropped: the UI thread waits its budget, then abandons the click and reports `ui/select/timeout` |
//! | `UiEvent::Hover` | event | latest-wins slot plus a throttle | 1 | posted only when the hovered index changes |
//! | `UiEvent::Page` / `Dismiss` | event | ordered collapsing queue | 16 | as `Show` / `Hide` |
//! | `UiEvent::Rendered` | data stream | latest-wins slot | 1 | the newest probe sample wins |
//!
//! A latest-wins slot holds exactly one value by construction -- it takes no
//! capacity parameter -- which is why the table gives it a depth of one and why
//! nothing can configure it otherwise. The two ordered queues and the click ring
//! do have capacities, and [`ChannelConfig::validate`] refuses a configuration
//! that would break the rule they are ordered under.
//!
//! The reason `Frame` may collapse and `Show`/`Hide` may not is that a frame is a
//! complete snapshot while a visibility change is a transition: overwriting an
//! unread frame loses nothing, whereas collapsing `Show, Hide` into `Hide`
//! changes what the user sees.
//!
//! # Revisions are monotonic
//!
//! `Show` and `Hide` carry the revision of the frame they belong to, and the
//! producer posts them in non-decreasing revision order: a visibility change is
//! the answer to a frame the user has already been shown, so a command that
//! referred to an older frame than one already delivered would move the window on
//! stale information. Keeping the rule is the producer's job; the consumer
//! *counts* a violation rather than dropping the command, because a dropped
//! `Hide` leaves the window visible, which is worse than a stale one. The counter
//! is [`ChannelStats::control_revision_regressions`].
//!
//! # What is deliberately absent
//!
//! There is no unbounded channel anywhere in this module. An unbounded queue
//! turns a slow consumer into an out-of-memory crash instead of a dropped frame,
//! and it hides the backpressure decision the architecture is built around.
//! Neither is there a timer: the only wakeup is the `eventfd` both threads
//! share, which is what lets the UI thread block indefinitely while it is idle.
//!
//! # Probing
//!
//! Every counter the boundary contract names is readable from [`ChannelStats`],
//! so a diagnostics layer can publish them without the channels depending on it.

mod command;
mod event;
mod queue;
mod wakeup;

use std::time::Duration;

use ime_types::ImeError;

pub use self::command::{CommandChannels, UiCommandSender};
pub use self::event::UiEventQueue;
pub use self::queue::{CollapsingQueue, LatestSlot, RingQueue, STAGE_BUDGET};
pub use self::wakeup::Wakeup;

/// The three channel semantics the boundary contract distinguishes.
///
/// The classification is not decoration: it is what decides the overflow rule.
/// A `DataStream` value is a complete snapshot, so the newest one supersedes the
/// rest and collapsing is lossless; a `Command` is a request that must be
/// answered in order; an `Event` is a one-way notification whose loss changes
/// what the user sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelSemantics {
    /// Request/response, ordered, never silently dropped.
    Command,
    /// One-way notification, ordered, never silently dropped.
    Event,
    /// Streaming snapshot, latest-wins, collapsing is lossless.
    DataStream,
}

impl ChannelSemantics {
    /// Whether a full channel of this class has to keep its values in order.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub const fn is_ordered(self) -> bool {
        matches!(self, Self::Command | Self::Event)
    }

    /// Whether a full channel of this class may replace a value nobody has read.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub const fn collapses(self) -> bool {
        matches!(self, Self::DataStream)
    }
}

/// The capacity the contract fixes for the ordered `Show` / `Hide` queue.
pub const CONTROL_CAPACITY: usize = 8;

/// The capacity the contract fixes for the never-dropped click queue.
pub const SELECT_CAPACITY: usize = 64;

/// The capacity the contract fixes for the ordered page / dismiss queue.
pub const PAGE_CAPACITY: usize = 16;

/// Minimum spacing between two hover notifications.
pub const HOVER_THROTTLE: Duration = Duration::from_millis(16);

/// The wait budget the contract names for the ordered `Show` / `Hide` queue.
pub const CONTROL_WAIT_BUDGET: Duration = Duration::from_micros(200);

/// The wait budget the contract names for the never-dropped click queue.
pub const SELECT_WAIT_BUDGET: Duration = Duration::from_micros(500);

/// The wait budget the contract names for the ordered page / dismiss queue.
pub const PAGE_WAIT_BUDGET: Duration = Duration::from_micros(200);

/// The longest a producer on the host thread may be configured to wait.
///
/// `features.md` 2.2.1 states the ceiling for the ordered `Show` / `Hide` queue as
/// "the host thread waits at most 200us", and that is the whole budget a key
/// callback has: a keystroke that spends it has already missed the frame
/// deadline. A configuration may lower a budget -- a test needs to reach the
/// overflow path without waiting -- but [`ChannelConfig::validate`] refuses one
/// that raises it past this.
pub const HOST_CALLBACK_BUDGET: Duration = Duration::from_micros(200);

/// The smallest depth an ordered channel may be configured with.
///
/// A one-deep ordered channel is a latest-wins slot wearing an ordered queue's
/// name: it collapses exactly the pair the contract forbids losing, because
/// whichever of `Show` and `Hide` was posted last is the only one left.
const MIN_ORDERED_DEPTH: usize = 2;

/// The stable code a rejected channel configuration is reported under.
///
/// It leads the `reason` of the [`ImeError::ConfigInvalid`] that
/// [`ChannelConfig::validate`] returns, the way the layout metrics' codes are
/// embedded in theirs, so a grep for the code finds every rule that can be
/// raised.
pub const CONFIG_INVALID_CODE: &str = "ui/channel/config-invalid";

/// The semantics of the latest-wins frame slot.
///
/// A frame is a complete snapshot of what the window should draw, so an unread
/// one carries no information a newer one lacks.
pub const FRAME_CHANNEL_SEMANTICS: ChannelSemantics = ChannelSemantics::DataStream;

/// The semantics of the ordered `Show` / `Hide` queue.
///
/// See [`FRAME_CHANNEL_SEMANTICS`] for why the classes are not interchangeable:
/// collapsing this pair would leave the window in the wrong visibility state.
pub const CONTROL_CHANNEL_SEMANTICS: ChannelSemantics = ChannelSemantics::Command;

/// The semantics of the latest-wins theme slot.
///
/// See [`FRAME_CHANNEL_SEMANTICS`]: a theme is a mode, and only the newest one
/// describes what the user is looking at.
pub const THEME_CHANNEL_SEMANTICS: ChannelSemantics = ChannelSemantics::DataStream;

/// The semantics of the latest-wins overlay slot.
///
/// See [`THEME_CHANNEL_SEMANTICS`]: an overlay is a mode as well, and the slot's
/// `None` -- the host closing the panel -- is a value like any other.
pub const OVERLAY_CHANNEL_SEMANTICS: ChannelSemantics = ChannelSemantics::DataStream;

/// The semantics of the event channel as a whole.
///
/// The channel carries clicks, page turns, hovers and render receipts, and each
/// of those has its own shape in the table above; the class here is what they
/// have in common, namely that losing one changes what the user sees.
pub const EVENT_CHANNEL_SEMANTICS: ChannelSemantics = ChannelSemantics::Event;

/// Capacities and wait budgets of the command and event channels.
///
/// Every field is a contractual number rather than a tuning knob: the capacities
/// come from the boundary-contract table, and the budgets are the ceiling on how
/// long a producer may be held up by a full channel. The ordered collapsing
/// queues stage a value that does not fit instead of waiting for room (see
/// [`STAGE_BUDGET`]), so a test that needs a deterministic overflow path gets one
/// from the capacity alone; a caller may lower a budget for a test, never raise
/// one past the contract -- [`ChannelConfig::validate`] is what enforces that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelConfig {
    /// Capacity of the ordered `Show` / `Hide` queue.
    pub control_capacity: usize,
    /// The budget the boundary contract names for that queue.
    ///
    /// A ceiling rather than a wait the producer spends: a full `Show` / `Hide`
    /// queue stages the newest value and returns, so the host thread is never
    /// held up by this channel.
    pub control_spin: Duration,
    /// Capacity of the never-dropped `Select` queue.
    pub select_capacity: usize,
    /// How long the UI thread waits for room in that queue.
    pub select_spin: Duration,
    /// Capacity of the ordered `Page` / `Dismiss` queue.
    pub page_capacity: usize,
    /// The budget the boundary contract names for that queue.
    ///
    /// A ceiling rather than a wait the producer spends, exactly as
    /// [`ChannelConfig::control_spin`] documents it: the producer of these events
    /// is the UI thread, which must stay responsive to the pointer and the
    /// keyboard rather than waiting for the host to drain.
    pub page_spin: Duration,
    /// Minimum spacing between two hover notifications.
    pub hover_throttle: Duration,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            control_capacity: CONTROL_CAPACITY,
            control_spin: CONTROL_WAIT_BUDGET,
            select_capacity: SELECT_CAPACITY,
            select_spin: SELECT_WAIT_BUDGET,
            page_capacity: PAGE_CAPACITY,
            page_spin: PAGE_WAIT_BUDGET,
            hover_throttle: HOVER_THROTTLE,
        }
    }
}

impl ChannelConfig {
    /// Rejects a configuration that would break the boundary contract.
    ///
    /// A capacity is a resource decision and a caller may move it; a *semantics*
    /// decision it may not. Three rules follow from the table in the module
    /// documentation:
    ///
    /// * the ordered `Show` / `Hide` queue must hold more than one value, because
    ///   a depth of one collapses exactly the pair the contract forbids losing;
    /// * no capacity may be zero, because a channel that can never hold anything
    ///   is a channel that silently discards everything given to it;
    /// * the `Show` / `Hide` budget may not be raised past
    ///   [`HOST_CALLBACK_BUDGET`], which is the ceiling `features.md` 2.2.1
    ///   states for the host thread.
    ///
    /// The click queue's budget is deliberately not checked: it is spent by the
    /// UI thread, which is allowed to wait for the host to drain, so the host's
    /// callback budget is not a bound on it.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::ConfigInvalid`] naming the offending field, with
    /// [`CONFIG_INVALID_CODE`] leading the reason so that a diagnostic or a test
    /// can match on it.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn validate(&self) -> Result<(), ImeError> {
        if self.control_capacity < MIN_ORDERED_DEPTH {
            return Err(invalid(
                "ui.channel.control_capacity",
                format!(
                    "an ordered queue needs at least {MIN_ORDERED_DEPTH} values, got {}",
                    self.control_capacity
                ),
            ));
        }
        if self.select_capacity == 0 {
            return Err(invalid(
                "ui.channel.select_capacity",
                String::from("a channel that can never hold a value discards every one"),
            ));
        }
        if self.page_capacity == 0 {
            return Err(invalid(
                "ui.channel.page_capacity",
                String::from("a channel that can never hold a value discards every one"),
            ));
        }
        if self.control_spin > HOST_CALLBACK_BUDGET {
            return Err(invalid(
                "ui.channel.control_spin",
                format!(
                    "the host thread may not be held for {}us, past the {}us callback budget",
                    self.control_spin.as_micros(),
                    HOST_CALLBACK_BUDGET.as_micros()
                ),
            ));
        }
        Ok(())
    }
}

/// Builds the error a violated contract rule is reported as.
///
/// The stable code leads the reason rather than the key, so that the reason reads
/// as a code plus an explanation while the key still names the field a caller has
/// to change.
fn invalid(key: &str, detail: String) -> ImeError {
    ImeError::ConfigInvalid {
        key: String::from(key),
        reason: format!("{CONFIG_INVALID_CODE}: {detail}"),
    }
}

/// A snapshot of every counter the boundary contract names.
///
/// The names in the comments are the stable diagnostic codes a probe publishes;
/// they are matched by diagnostics and tests, so a counter never changes meaning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChannelStats {
    /// `ui.frame.coalesced`: frames replaced before the UI thread read them.
    pub frames_coalesced: u64,
    /// Themes replaced before the UI thread read them.
    pub themes_coalesced: u64,
    /// Overlay states replaced before the UI thread read them.
    pub overlays_coalesced: u64,
    /// `ui.control.dropped`: ordered commands that had to be collapsed.
    pub controls_collapsed: u64,
    /// `Show` / `Hide` commands delivered out of revision order.
    ///
    /// The rule these belong to is the producer's to keep (see the module
    /// documentation); a non-zero value here means it was broken, and the command
    /// itself was still delivered, because dropping a `Hide` would leave the
    /// window visible.
    pub control_revision_regressions: u64,
    /// `ui/select/timeout`: clicks abandoned because the host was not draining.
    pub select_timeouts: u64,
    /// `ui/thread/dead`: the UI thread has exited, so commands are discarded.
    pub thread_dead: bool,
    /// The UI thread exited by panicking rather than by being asked to stop.
    pub thread_panicked: bool,
    /// `ui/shutdown/timeout`: shutdown gave up waiting and detached the thread.
    pub shutdown_timeouts: u64,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    use ime_types::{
        Anchor, HideReason, LayoutHint, OverlayFrame, OverlayKind, PageState, Placement, Preedit,
        RectI, ScreenId, StatusStrip, UiCommand, UiFrame,
    };

    use super::*;

    /// How many pushes the staging assertion samples.
    ///
    /// The bound is on the work rather than on the scheduler, so the fastest sample is the
    /// one asserted; a push that waited for room cannot pass at any sample count, because
    /// every one of its samples costs the wait.
    const PUSH_SAMPLES: u32 = 16;

    /// The value that occupies the single slot of the staging test's queue.
    const SEED: u32 = 0;

    fn anchor() -> Anchor {
        Anchor {
            cursor: RectI {
                x: 10,
                y: 20,
                w: 2,
                h: 20,
            },
            screen: ScreenId::new(0),
            scale: 1.0,
            placement: Placement::Below,
        }
    }

    fn frame(revision: u32) -> Box<UiFrame> {
        Box::new(UiFrame {
            revision,
            preedit: Preedit {
                text: String::from("ni"),
                caret: 2,
                spans: Vec::new(),
            },
            candidates: Vec::new(),
            page: PageState {
                current: 1,
                total: 1,
                page_size: 9,
            },
            status: StatusStrip::default(),
            anchor: anchor(),
            layout: LayoutHint {
                max_per_row: 5,
                show_annotation: true,
                max_width_dp: 720,
            },
        })
    }

    fn overlay(title: &str) -> Box<OverlayFrame> {
        Box::new(OverlayFrame {
            kind: OverlayKind::CheatSheet,
            title: String::from(title),
            sections: Vec::new(),
            selected: None,
            query: String::new(),
        })
    }

    fn show(revision: u32) -> UiCommand {
        UiCommand::Show {
            revision,
            anchor: anchor(),
        }
    }

    fn hide(revision: u32) -> UiCommand {
        UiCommand::Hide {
            revision,
            reason: HideReason::Cancelled,
        }
    }

    /// Builds the command channel pair the contract tests drive.
    fn channels(config: &ChannelConfig) -> (Arc<CommandChannels>, UiCommandSender) {
        let channels = command_channels(config);
        let sender = sender_of(&channels);
        (channels, sender)
    }

    /// Builds the command half through the same call the UI thread's start-up
    /// uses, so a configuration the contract rejects is rejected here too.
    fn command_channels(config: &ChannelConfig) -> Arc<CommandChannels> {
        Arc::new(CommandChannels::new(config).expect("the configuration is contract-valid"))
    }

    /// The producer handle the contract tests post through.
    fn sender_of(channels: &Arc<CommandChannels>) -> UiCommandSender {
        UiCommandSender::new(Arc::clone(channels), Arc::new(AtomicBool::new(false)))
    }

    #[test]
    fn test_channel_config_default_matches_the_contract() {
        let config = ChannelConfig::default();
        assert_eq!(config.control_capacity, 8);
        assert_eq!(config.select_capacity, 64);
        assert_eq!(config.page_capacity, 16);
        assert_eq!(config.control_spin, Duration::from_micros(200));
        assert_eq!(config.select_spin, Duration::from_micros(500));
        assert_eq!(config.page_spin, Duration::from_micros(200));
        assert_eq!(config.hover_throttle, Duration::from_millis(16));
        // The same numbers again, through the constants the contract fixes them
        // as: a change to one of those without the other is what this catches.
        assert_eq!(config.control_capacity, CONTROL_CAPACITY);
        assert_eq!(config.select_capacity, SELECT_CAPACITY);
        assert_eq!(config.page_capacity, PAGE_CAPACITY);
        assert_eq!(config.control_spin, CONTROL_WAIT_BUDGET);
        assert_eq!(config.select_spin, SELECT_WAIT_BUDGET);
        assert_eq!(config.page_spin, PAGE_WAIT_BUDGET);
        assert_eq!(config.hover_throttle, HOVER_THROTTLE);
    }

    #[test]
    fn test_channel_stats_default_reports_nothing_wrong() {
        let stats = ChannelStats::default();
        assert_eq!(stats.frames_coalesced, 0);
        assert_eq!(stats.themes_coalesced, 0);
        assert_eq!(stats.overlays_coalesced, 0);
        assert_eq!(stats.controls_collapsed, 0);
        assert_eq!(stats.control_revision_regressions, 0);
        assert_eq!(stats.select_timeouts, 0);
        assert!(!stats.thread_dead);
        assert!(!stats.thread_panicked);
        assert_eq!(stats.shutdown_timeouts, 0);
    }

    #[test]
    fn test_collapsing_queue_push_when_full_stays_inside_the_stage_budget() {
        // The producer is the Fcitx5 host thread, so a queue that is full must not hold it
        // up. The queue carries the channel's own budget, which is above the staging bound,
        // so a push that waited for room would miss the assertion by the bound itself.
        let budget = ChannelConfig::default().control_spin;
        let queue: CollapsingQueue<u32> = CollapsingQueue::new(1, budget);
        assert_eq!(queue.budget(), budget);

        let mut fastest = Duration::MAX;
        for value in 1..=PUSH_SAMPLES {
            queue.push(SEED);
            let start = Instant::now();
            queue.push(value);
            fastest = fastest.min(start.elapsed());
            assert_eq!(queue.pop(), Some(SEED), "the queued value is still first");
            assert_eq!(queue.pop(), Some(value), "the staged value is the one kept");
        }
        assert_eq!(
            queue.collapsed(),
            u64::from(PUSH_SAMPLES),
            "every push that had to be staged is counted as a collapsed one"
        );
        assert!(
            fastest <= STAGE_BUDGET,
            "the fastest push into a full queue took {fastest:?}, past the {STAGE_BUDGET:?} bound"
        );
    }

    #[test]
    fn test_channel_config_validate_accepts_the_contract_defaults() {
        assert!(
            ChannelConfig::default().validate().is_ok(),
            "the contract's own numbers are the configuration the gate accepts"
        );
    }

    #[test]
    fn test_channel_config_validate_rejects_a_control_depth_below_two() {
        // Zero and one are the two boundary values: the first is a channel that
        // discards everything, the second is a latest-wins slot wearing an
        // ordered queue's name, and both collapse the `Show`/`Hide` pair.
        for depth in [0usize, 1] {
            let config = ChannelConfig {
                control_capacity: depth,
                ..ChannelConfig::default()
            };
            let error = config
                .validate()
                .expect_err("an ordered queue cannot be one deep");
            let rendered = error.to_string();
            assert!(
                matches!(error, ImeError::ConfigInvalid { .. }),
                "a rejected configuration is a config/invalid, got {rendered}"
            );
            assert!(
                rendered.contains(CONFIG_INVALID_CODE),
                "the stable code leads the reason, got {rendered}"
            );
            assert!(
                rendered.contains("ui.channel.control_capacity"),
                "the offending field is named, got {rendered}"
            );
        }
    }

    #[test]
    fn test_channel_config_validate_rejects_a_zero_capacity() {
        // A channel that can never hold a value is not a small channel: it is one
        // that silently discards everything it is given.
        let zero_select = ChannelConfig {
            select_capacity: 0,
            ..ChannelConfig::default()
        };
        let zero_page = ChannelConfig {
            page_capacity: 0,
            ..ChannelConfig::default()
        };

        let error = zero_select
            .validate()
            .expect_err("a channel that holds nothing is a contract violation");
        assert!(error.to_string().contains("ui.channel.select_capacity"));
        let error = zero_page
            .validate()
            .expect_err("a channel that holds nothing is a contract violation");
        assert!(error.to_string().contains("ui.channel.page_capacity"));
    }

    #[test]
    fn test_channel_config_validate_rejects_a_control_wait_past_the_host_budget() {
        let over = ChannelConfig {
            control_spin: HOST_CALLBACK_BUDGET + Duration::from_micros(1),
            ..ChannelConfig::default()
        };
        let error = over
            .validate()
            .expect_err("the host thread's budget is a ceiling, not a suggestion");
        assert!(
            error.to_string().contains("ui.channel.control_spin"),
            "the offending field is named, got {error}"
        );

        // The ceiling itself is inside the contract, so the check is on the
        // budget and not on the value being below the default.
        let at_ceiling = ChannelConfig {
            control_spin: HOST_CALLBACK_BUDGET,
            ..ChannelConfig::default()
        };
        assert!(at_ceiling.validate().is_ok());

        // A lowered budget is how a test reaches the overflow path without
        // waiting, and lowering is always allowed.
        let lowered = ChannelConfig {
            control_spin: Duration::ZERO,
            ..ChannelConfig::default()
        };
        assert!(lowered.validate().is_ok());
    }

    #[test]
    fn test_contract_every_channel_is_classified_by_its_overflow_rule() {
        assert_eq!(FRAME_CHANNEL_SEMANTICS, ChannelSemantics::DataStream);
        assert_eq!(THEME_CHANNEL_SEMANTICS, ChannelSemantics::DataStream);
        assert_eq!(OVERLAY_CHANNEL_SEMANTICS, ChannelSemantics::DataStream);
        assert_eq!(CONTROL_CHANNEL_SEMANTICS, ChannelSemantics::Command);
        assert_eq!(EVENT_CHANNEL_SEMANTICS, ChannelSemantics::Event);

        // The two predicates are the contract: a channel may collapse *or* it
        // may be ordered, and the classes do not overlap.
        assert!(FRAME_CHANNEL_SEMANTICS.collapses(), "a snapshot");
        assert!(!FRAME_CHANNEL_SEMANTICS.is_ordered());
        assert!(THEME_CHANNEL_SEMANTICS.collapses(), "a snapshot");
        assert!(!THEME_CHANNEL_SEMANTICS.is_ordered());
        assert!(OVERLAY_CHANNEL_SEMANTICS.collapses(), "a mode");
        assert!(!OVERLAY_CHANNEL_SEMANTICS.is_ordered());
        assert!(CONTROL_CHANNEL_SEMANTICS.is_ordered(), "in order");
        assert!(!CONTROL_CHANNEL_SEMANTICS.collapses());
        assert!(EVENT_CHANNEL_SEMANTICS.is_ordered(), "in order");
        assert!(!EVENT_CHANNEL_SEMANTICS.collapses());
    }

    #[test]
    fn test_contract_frame_channel_coalesces_to_the_newest_frame() {
        let (channels, sender) = channels(&ChannelConfig::default());
        for revision in 1..=3 {
            sender
                .send(UiCommand::Frame(frame(revision)))
                .expect("the channel is open");
        }
        let newest = channels.take_frame().expect("a frame is waiting");
        assert_eq!(newest.revision, 3, "the newest frame is the one kept");
        assert_eq!(
            channels.frames_coalesced(),
            2,
            "each replaced frame is counted, which is what `ui.frame.coalesced` reports"
        );
        assert!(
            channels.take_frame().is_none(),
            "the slot holds one value, not a queue"
        );
    }

    #[test]
    fn test_contract_control_channel_keeps_show_hide_in_order() {
        let (channels, sender) = channels(&ChannelConfig::default());
        let sequence = [show(1), hide(2), show(3)];
        for command in &sequence {
            sender.send(command.clone()).expect("the channel is open");
        }
        let mut observed = Vec::new();
        while let Some(command) = channels.pop_control() {
            observed.push(command);
        }
        assert_eq!(observed, sequence, "Show/Hide keep their order");
        assert_eq!(channels.controls_collapsed(), 0, "nothing had to collapse");
        assert_eq!(
            channels.control_revision_regressions(),
            0,
            "the revisions ascend, so the producer kept the rule"
        );
    }

    #[test]
    fn test_contract_control_channel_keeps_a_pair_that_arrives_on_a_full_queue() {
        let (channels, sender) = channels(&ChannelConfig::default());
        for revision in 0..CONTROL_CAPACITY as u32 {
            sender.send(show(revision)).expect("the channel is open");
        }
        // The queue is full, so this `Hide` has to be staged rather than queued;
        // it is still delivered, and it still arrives after the `Show` it closes.
        sender.send(hide(99)).expect("the channel is open");
        assert_eq!(
            channels.controls_collapsed(),
            1,
            "the staged push is what `ui.control.dropped` counts"
        );

        let mut revisions = Vec::new();
        while let Some(command) = channels.pop_control() {
            match command {
                UiCommand::Show { revision, .. } | UiCommand::Hide { revision, .. } => {
                    revisions.push(revision);
                }
                other => panic!("only visibility changes were posted, got {other:?}"),
            }
        }
        let mut expected: Vec<u32> = (0..CONTROL_CAPACITY as u32).collect();
        expected.push(99);
        assert_eq!(
            revisions, expected,
            "the pair survives a full queue, in order and complete"
        );
        assert_eq!(
            channels.control_revision_regressions(),
            0,
            "a collapsed queue does not reorder what it does deliver"
        );
    }

    #[test]
    fn test_contract_overlay_channel_keeps_the_newest_and_distinguishes_closed() {
        let (channels, sender) = channels(&ChannelConfig::default());
        for title in ["one", "two", "three"] {
            sender
                .send(UiCommand::Overlay(Some(overlay(title))))
                .expect("the channel is open");
        }
        let newest = channels
            .take_overlay()
            .expect("an overlay state is waiting")
            .expect("the newest state is an open overlay");
        assert_eq!(newest.title, "three", "the newest overlay is the one kept");
        assert_eq!(channels.overlays_coalesced(), 2);

        // Closing is a value, not an absence. A surface that could not tell the
        // two apart would keep drawing a panel the user has dismissed.
        sender
            .send(UiCommand::Overlay(None))
            .expect("the channel is open");
        assert!(
            matches!(channels.take_overlay(), Some(None)),
            "the close is delivered as the state it is"
        );
        assert_eq!(
            channels.take_overlay(),
            None,
            "and the slot is empty afterwards"
        );
    }

    #[test]
    fn test_contract_a_control_revision_that_goes_backwards_is_counted_and_delivered() {
        let (channels, sender) = channels(&ChannelConfig::default());
        for revision in [4u32, 9, 6] {
            sender.send(hide(revision)).expect("the channel is open");
        }
        let mut revisions = Vec::new();
        while let Some(command) = channels.pop_control() {
            if let UiCommand::Hide { revision, .. } = command {
                revisions.push(revision);
            }
        }
        assert_eq!(
            revisions,
            vec![4, 9, 6],
            "a regressing revision is delivered anyway: dropping a Hide leaves the window up"
        );
        assert_eq!(
            channels.control_revision_regressions(),
            1,
            "and the violation is counted exactly once"
        );
    }
}
