//! The cross-thread channels of the UI thread.
//!
//! Two directions, four shapes, and one rule: what happens when a channel is
//! full is part of the contract, not an implementation detail. The candidate
//! window must never be left showing stale candidates because a frame was
//! refused, and it must never be left visible because a `Hide` was collapsed
//! against a `Show`.
//!
//! | Channel | Shape | Capacity | Overflow behaviour |
//! |---|---|---|---|
//! | `UiCommand::Frame` | latest-wins slot | 1 | older frame replaced, counted as `ui.frame.coalesced` |
//! | `UiCommand::Show` / `Hide` | ordered collapsing queue | 8 | a full queue stages the newest value, counted as `ui.control.dropped`; the sender never waits |
//! | `UiCommand::Theme` | latest-wins slot | 1 | older value replaced |
//! | `UiCommand::Shutdown` | flag | 1 | terminal, and deliberately not shared with any other channel |
//! | `UiEvent::Select` | bounded ring | 64 | never dropped: the UI thread waits its budget, then abandons the click and reports `ui/select/timeout` |
//! | `UiEvent::Page` / `Dismiss` | ordered collapsing queue | 16 | as `Show` / `Hide` |
//! | `UiEvent::Hover` | latest-wins slot plus a throttle | 1 | posted only when the hovered index changes |
//! | `UiEvent::Rendered` | latest-wins slot | 1 | newest probe sample wins |
//!
//! The reason `Frame` may collapse and `Show`/`Hide` may not is that a frame is a
//! complete snapshot while a visibility change is a transition: overwriting an
//! unread frame loses nothing, whereas collapsing `Show, Hide` into `Hide`
//! changes what the user sees.
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

pub use self::command::{CommandChannels, UiCommandSender};
pub use self::event::UiEventQueue;
pub use self::queue::{CollapsingQueue, LatestSlot, RingQueue, STAGE_BUDGET};
pub use self::wakeup::Wakeup;

/// Capacities and wait budgets of the command and event channels.
///
/// Every field is a contractual number rather than a tuning knob: the capacities
/// come from the boundary-contract table, and the budgets are the ceiling on how
/// long a producer may be held up by a full channel. The ordered collapsing
/// queues stage a value that does not fit instead of waiting for room (see
/// [`STAGE_BUDGET`]), so a test that needs a deterministic overflow path gets one
/// from the capacity alone; a caller may lower a budget for a test, never raise
/// one past the contract.
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
            control_capacity: 8,
            control_spin: Duration::from_micros(200),
            select_capacity: 64,
            select_spin: Duration::from_micros(500),
            page_capacity: 16,
            page_spin: Duration::from_micros(200),
            hover_throttle: Duration::from_millis(16),
        }
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
    /// `ui.control.dropped`: ordered commands that had to be collapsed.
    pub controls_collapsed: u64,
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
    use std::time::Instant;

    use super::*;

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
    }

    #[test]
    fn test_channel_stats_default_reports_nothing_wrong() {
        let stats = ChannelStats::default();
        assert_eq!(stats.frames_coalesced, 0);
        assert_eq!(stats.controls_collapsed, 0);
        assert_eq!(stats.select_timeouts, 0);
        assert!(!stats.thread_dead);
        assert!(!stats.thread_panicked);
        assert_eq!(stats.shutdown_timeouts, 0);
    }

    /// How many pushes the staging assertion samples.
    ///
    /// The bound is on the work rather than on the scheduler, so the fastest sample is the
    /// one asserted; a push that waited for room cannot pass at any sample count, because
    /// every one of its samples costs the wait.
    const PUSH_SAMPLES: u32 = 16;

    /// The value that occupies the single slot of the staging test's queue.
    const SEED: u32 = 0;

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
}
