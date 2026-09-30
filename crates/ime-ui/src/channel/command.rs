//! The host thread's half of the command channel.
//!
//! [`CommandChannels`] is the shared state and [`UiCommandSender`] is the
//! producer handle the host thread keeps. Splitting them is what makes the
//! delivery semantics of the boundary contract visible in the types: the
//! sender's only job is to route one [`UiCommand`] to the channel that matches
//! its class, and the loop's only job is to take them back out.
//!
//! Routing is the contract:
//!
//! * `Frame`, `Theme` and `Overlay` go to a latest-wins slot, because each of
//!   them is a complete snapshot or a mode rather than a transition, and an
//!   unread one carries no information the newer one lacks.
//! * `Show` and `Hide` go to the ordered collapsing queue, because the pair
//!   means something only in order.
//! * `Shutdown` gets a flag of its own rather than sharing a slot with anything
//!   else: a terminal command that a later frame could overwrite would leave the
//!   UI thread running for the rest of the process's life.
//!
//! Every one of those routes ends with the same wakeup, so a post is one lock
//! (or one bounded staging step), one store and one syscall on the host thread.
//!
//! # Revisions
//!
//! `Show` and `Hide` carry the revision of the frame they belong to, and the
//! producer posts them in non-decreasing revision order. The rule is stated in
//! [`crate::channel`]'s module documentation; the consumer *counts* a violation
//! rather than acting on it, because the one thing worse than acting on a stale
//! revision is dropping the `Hide` that would have put the window away.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use ime_types::{ImeError, OverlayFrame, ThemeSpec, UiCommand, UiError, UiFrame};

use super::ChannelConfig;
use super::queue::{CollapsingQueue, LatestSlot};
use super::wakeup::Wakeup;

/// The state shared by the host thread and the UI thread's command channel.
///
/// # Concurrency
///
/// `Send` and `Sync`: every field is either a lock-protected queue, an atomic
/// or the wakeup counter, and none of them is held across a blocking call. The
/// consumer methods are called by the UI thread and the producer methods by the
/// host thread, which is the single-producer/single-consumer shape the contract
/// assumes; the internal locks make an accidental second producer harmless
/// rather than undefined.
#[derive(Debug)]
pub struct CommandChannels {
    frame: LatestSlot<Box<UiFrame>>,
    theme: LatestSlot<ThemeSpec>,
    /// The newest overlay state, or `Some(None)` once the host has closed the
    /// overlay.
    ///
    /// The value stored is an `Option` on purpose: "the overlay is closed" and
    /// "the host has said nothing about overlays" are different states, and a
    /// surface that conflated them would draw a panel the user has dismissed.
    overlay: LatestSlot<Option<Box<OverlayFrame>>>,
    control: CollapsingQueue<UiCommand>,
    shutdown: AtomicBool,
    /// The highest `Show`/`Hide` revision delivered so far.
    last_control_revision: AtomicU32,
    /// How many of them arrived out of revision order.
    control_revision_regressions: AtomicU64,
    wakeup: Wakeup,
}

impl CommandChannels {
    /// Creates the channel, including its wakeup counter.
    ///
    /// The configuration is checked against the boundary contract before
    /// anything is built, so an illegal one is rejected here -- at construction
    /// -- rather than producing a channel that quietly violates `features.md`
    /// 2.2.1.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::ConfigInvalid`] when `config` breaks the contract (see
    /// [`ChannelConfig::validate`]), and [`ImeError::UiChannelClosed`] when the
    /// wakeup counter cannot be created, which means no UI thread can be started.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn new(config: &ChannelConfig) -> Result<Self, ImeError> {
        config.validate()?;
        Ok(Self {
            frame: LatestSlot::new(),
            theme: LatestSlot::new(),
            overlay: LatestSlot::new(),
            control: CollapsingQueue::new(config.control_capacity, config.control_spin),
            shutdown: AtomicBool::new(false),
            last_control_revision: AtomicU32::new(0),
            control_revision_regressions: AtomicU64::new(0),
            wakeup: Wakeup::new()?,
        })
    }

    /// The wakeup counter both threads share.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn wakeup(&self) -> &Wakeup {
        &self.wakeup
    }

    /// Takes the newest frame, if one is waiting.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn take_frame(&self) -> Option<Box<UiFrame>> {
        self.frame.take()
    }

    /// Takes the newest theme, if one is waiting.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn take_theme(&self) -> Option<ThemeSpec> {
        self.theme.take()
    }

    /// Takes the newest overlay state, if the host has posted one.
    ///
    /// The outer `Option` is whether anything was waiting at all; the inner one
    /// is the state itself, so `Some(None)` means "the overlay is closed" and
    /// `None` means "the host has not spoken about overlays". A caller that
    /// collapses the two cannot tell a dismissed panel from one that was never
    /// opened, which is why the return type keeps them apart.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn take_overlay(&self) -> Option<Option<Box<OverlayFrame>>> {
        self.overlay.take()
    }

    /// Takes the oldest ordered control command, if one is waiting.
    ///
    /// A `Show` or `Hide` whose revision went backwards is delivered anyway and
    /// counted: dropping a `Hide` to punish a producer that broke the rule would
    /// leave the candidate window visible, which is the worse of the two
    /// failures.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn pop_control(&self) -> Option<UiCommand> {
        let command = self.control.pop()?;
        self.note_control_revision(&command);
        Some(command)
    }

    /// Records that the UI thread has been asked to stop.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    /// Whether the UI thread has been asked to stop.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    /// How many frames were replaced before the UI thread read them.
    ///
    /// This is the probe counter behind `ui.frame.coalesced`.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn frames_coalesced(&self) -> u64 {
        self.frame.coalesced()
    }

    /// How many themes were replaced before the UI thread read them.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn themes_coalesced(&self) -> u64 {
        self.theme.coalesced()
    }

    /// How many overlay states were replaced before the UI thread read them.
    ///
    /// A close posted over an unread open counts, exactly as a frame replacing a
    /// frame does: the older state is gone either way.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn overlays_coalesced(&self) -> u64 {
        self.overlay.coalesced()
    }

    /// How many control commands had to be collapsed.
    ///
    /// This is the probe counter behind `ui.control.dropped`.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn controls_collapsed(&self) -> u64 {
        self.control.collapsed()
    }

    /// How many `Show`/`Hide` commands arrived out of revision order.
    ///
    /// Zero is what a correct producer leaves here. A non-zero value is a
    /// producer bug rather than a channel one: the channel delivered what it was
    /// given, in the order it was given.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn control_revision_regressions(&self) -> u64 {
        self.control_revision_regressions.load(Ordering::Relaxed)
    }

    /// Counts a `Show`/`Hide` whose revision is older than one already delivered.
    ///
    /// `fetch_max` is what makes this a "went backwards" check rather than a
    /// sequence check: the consumer sees the commands in the order the queue
    /// delivered them, so comparing against the highest revision seen so far is
    /// exactly the rule the producer has to keep.
    fn note_control_revision(&self, command: &UiCommand) {
        let (UiCommand::Show { revision, .. } | UiCommand::Hide { revision, .. }) = command else {
            // Every other variant carries no revision: a frame's is inside the
            // frame, the theme and overlay slots are modes rather than
            // transitions, and shutdown is a flag of its own.
            return;
        };
        let highest = &self.last_control_revision;
        let previous = highest.fetch_max(*revision, Ordering::Relaxed);
        if *revision < previous {
            let regressions = &self.control_revision_regressions;
            regressions.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The host thread's handle on one UI thread's command channel.
///
/// Cloning the handle shares the same channel; the host keeps one copy and the
/// addon lifecycle keeps another if it needs to post from two places.
///
/// # Concurrency
///
/// `Send` and `Sync`. [`UiCommandSender::send`] is non-blocking: the ordered
/// channel stages a value that does not fit instead of waiting for room, so the
/// only work on the host thread is a lock, a store and the wakeup syscall.
#[derive(Clone, Debug)]
pub struct UiCommandSender {
    channels: Arc<CommandChannels>,
    dead: Arc<AtomicBool>,
}

impl UiCommandSender {
    /// Creates a sender over `channels` that stops posting once `dead` is set.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn new(channels: Arc<CommandChannels>, dead: Arc<AtomicBool>) -> Self {
        Self { channels, dead }
    }

    /// Posts one command and wakes the UI thread.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::ThreadDead`] when the UI thread has already exited:
    /// the command is dropped rather than queued, because nothing will ever read
    /// it, and the caller keeps working without a candidate window. That is the
    /// one failure an input method must survive.
    ///
    /// Returns [`UiError::ChannelClosed`] when the wakeup counter can no longer
    /// be written, which means the UI thread will never see the command even
    /// though it was queued.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn send(&self, command: UiCommand) -> Result<(), UiError> {
        if self.dead.load(Ordering::Acquire) {
            return Err(UiError::ThreadDead);
        }
        match command {
            UiCommand::Frame(frame) => {
                self.channels.frame.put(frame);
            }
            UiCommand::Theme(theme) => {
                self.channels.theme.put(theme);
            }
            UiCommand::Overlay(frame) => {
                self.channels.overlay.put(frame);
            }
            UiCommand::Show { .. } | UiCommand::Hide { .. } => self.channels.control.push(command),
            UiCommand::Shutdown => self.channels.request_shutdown(),
        }
        // One wakeup covers every route: the loop drains all of them whenever it
        // is woken, so a post that coalesces with a pending one loses nothing.
        self.channels.wakeup.wake()
    }

    /// The channel this sender posts into.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn channels(&self) -> &Arc<CommandChannels> {
        &self.channels
    }
}

#[cfg(test)]
mod tests {
    use ime_types::{Anchor, HideReason, OverlayKind, Placement, RectI, ScreenId};

    use super::*;

    fn channels() -> Arc<CommandChannels> {
        let config = ChannelConfig::default();
        Arc::new(CommandChannels::new(&config).expect("the wakeup counter can be created"))
    }

    fn sender(channels: &Arc<CommandChannels>) -> UiCommandSender {
        UiCommandSender::new(Arc::clone(channels), Arc::new(AtomicBool::new(false)))
    }

    fn frame(revision: u32) -> Box<UiFrame> {
        Box::new(UiFrame {
            revision,
            preedit: ime_types::Preedit {
                text: String::from("ni"),
                caret: 2,
                spans: Vec::new(),
            },
            candidates: Vec::new(),
            page: ime_types::PageState {
                current: 1,
                total: 1,
                page_size: 9,
            },
            status: ime_types::StatusStrip::default(),
            anchor: anchor(),
            layout: ime_types::LayoutHint {
                max_per_row: 5,
                show_annotation: true,
                max_width_dp: 720,
            },
        })
    }

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

    fn overlay(title: &str) -> Box<OverlayFrame> {
        Box::new(OverlayFrame {
            kind: OverlayKind::CheatSheet,
            title: String::from(title),
            sections: Vec::new(),
            selected: None,
            query: String::new(),
        })
    }

    #[test]
    fn test_command_sender_frames_are_latest_wins() {
        let channels = channels();
        let sender = sender(&channels);
        for revision in 1..=3 {
            sender
                .send(UiCommand::Frame(frame(revision)))
                .expect("the channel is open");
        }
        let newest = channels.take_frame().expect("a frame is waiting");
        assert_eq!(newest.revision, 3, "the newest frame is the one kept");
        assert_eq!(channels.frames_coalesced(), 2);
        assert!(channels.take_frame().is_none());
    }

    #[test]
    fn test_command_sender_keeps_control_commands_in_order() {
        let channels = channels();
        let sender = sender(&channels);
        let sequence = [
            UiCommand::Show {
                revision: 1,
                anchor: anchor(),
            },
            UiCommand::Hide {
                revision: 2,
                reason: HideReason::Cancelled,
            },
            UiCommand::Show {
                revision: 3,
                anchor: anchor(),
            },
        ];
        for command in &sequence {
            sender.send(command.clone()).expect("the channel is open");
        }
        let mut observed = Vec::new();
        while let Some(command) = channels.pop_control() {
            observed.push(command);
        }
        assert_eq!(observed, sequence, "Show/Hide keep their order");
        assert_eq!(channels.controls_collapsed(), 0);
        assert_eq!(channels.control_revision_regressions(), 0);
    }

    #[test]
    fn test_command_sender_shutdown_does_not_share_the_frame_slot() {
        let channels = channels();
        let sender = sender(&channels);
        sender
            .send(UiCommand::Shutdown)
            .expect("the channel is open");
        sender
            .send(UiCommand::Frame(frame(9)))
            .expect("the channel is open");
        assert!(channels.is_shutdown(), "shutdown survives a later frame");
        assert_eq!(
            channels.take_frame().map(|frame| frame.revision),
            Some(9),
            "the frame slot is untouched by the shutdown flag"
        );
    }

    #[test]
    fn test_command_sender_themes_are_latest_wins() {
        let channels = channels();
        let sender = sender(&channels);
        let theme = |alpha: u8| {
            UiCommand::Theme(ime_types::ThemeSpec {
                scheme: ime_types::ColorScheme::Dark,
                accent: ime_types::Rgba8 {
                    r: 1,
                    g: 2,
                    b: 3,
                    a: 255,
                },
                acrylic: false,
                base_alpha: alpha,
                corner_radius_dp: 12,
                scale: 1.0,
            })
        };
        sender.send(theme(100)).expect("the channel is open");
        sender.send(theme(200)).expect("the channel is open");
        let newest = channels.take_theme().expect("a theme is waiting");
        assert_eq!(newest.base_alpha, 200);
        assert_eq!(channels.themes_coalesced(), 1);
    }

    #[test]
    fn test_command_sender_overlays_are_latest_wins() {
        let channels = channels();
        let sender = sender(&channels);
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
        assert_eq!(
            channels.overlays_coalesced(),
            2,
            "the two replaced states are counted, not queued"
        );
        assert!(channels.take_overlay().is_none());
    }

    #[test]
    fn test_command_sender_overlay_close_is_a_value_not_an_absence() {
        let channels = channels();
        let sender = sender(&channels);
        sender
            .send(UiCommand::Overlay(None))
            .expect("the channel is open");
        // `Some(None)` is what a surface needs to tell "close the panel" from
        // "nothing was ever posted about panels".
        assert_eq!(
            channels.take_overlay().map(|state| state.is_none()),
            Some(true),
            "the close is delivered as a state"
        );
        assert_eq!(channels.take_overlay(), None);
    }

    #[test]
    fn test_command_sender_after_the_thread_died_reports_and_posts_nothing() {
        let channels = channels();
        let dead = Arc::new(AtomicBool::new(true));
        let sender = UiCommandSender::new(Arc::clone(&channels), Arc::clone(&dead));
        let outcome = sender.send(UiCommand::Frame(frame(1)));
        assert_eq!(outcome, Err(UiError::ThreadDead));
        assert!(
            channels.take_frame().is_none(),
            "a dead thread is never handed a command"
        );
    }

    #[test]
    fn test_command_sender_wake_is_visible_to_the_consumer() {
        let channels = channels();
        let sender = sender(&channels);
        sender
            .send(UiCommand::Frame(frame(1)))
            .expect("the channel is open");
        // The consumer side of the wakeup: draining must find the post and then
        // report an empty counter rather than blocking.
        channels.wakeup().drain().expect("the post woke the loop");
        channels
            .wakeup()
            .drain()
            .expect("the counter is empty again");
    }

    #[test]
    fn test_command_channels_reject_an_illegal_configuration() {
        // The check happens in the constructor, so a channel that would violate
        // the contract cannot exist even briefly.
        let config = ChannelConfig {
            control_capacity: 1,
            ..ChannelConfig::default()
        };
        let outcome = CommandChannels::new(&config);
        assert!(
            matches!(&outcome, Err(ImeError::ConfigInvalid { .. })),
            "a one-deep ordered queue is refused at construction"
        );
        let error = outcome.expect_err("no channel was created");
        assert!(error.to_string().contains("ui.channel.control_capacity"));
    }
}
