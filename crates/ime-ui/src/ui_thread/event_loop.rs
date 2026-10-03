//! The UI thread's `poll(2)` event loop.
//!
//! The loop waits on exactly two descriptors: the wakeup counter both threads
//! share, and the surface's own connection when it has one. It has no timer of
//! its own. While nothing is animating the timeout is infinite, so an idle
//! candidate window costs nothing at all -- no wakeups, no redraws, no periodic
//! work -- which is what the idle-CPU budget requires. The only thing that makes
//! the loop wait a bounded time is a surface that reports the instant its next
//! animation frame is due.
//!
//! Each iteration is: wait, drain the wakeup, apply whatever the host posted,
//! let the surface consume its input, notice a shutdown request, and render.
//! Rendering is a call rather than a decision: the surface knows whether it is
//! dirty, and a surface that is not dirty does nothing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ime_types::ImeError;

use crate::channel::{CommandChannels, UiEventQueue};

use super::UiThreadConfig;
use super::surface::{SurfaceUpdate, UiSurface};

/// The shortest wait the loop allows while an animation is running.
///
/// A surface computes its own frame deadline, and this floor only exists so that
/// a surface which keeps reporting a deadline in the past cannot turn the loop
/// into a busy poll. It is far below a frame interval at any refresh rate, so it
/// never delays an animation.
const MIN_FRAME_TIMEOUT: Duration = Duration::from_millis(1);

/// The UI thread's event loop and the state it owns.
///
/// # Concurrency
///
/// Owned by the UI thread and never shared: the loop drives the surface, while
/// the host thread reaches the same channels through the handles it already
/// holds. Nothing here is `Sync`, and nothing here is meant to be.
pub struct UiLoop {
    surface: Box<dyn UiSurface>,
    channels: Arc<CommandChannels>,
    events: Arc<UiEventQueue>,
    config: UiThreadConfig,
    /// When the next animation frame is due, or `None` while the surface is
    /// idle and the loop may block indefinitely.
    deadline: Option<Instant>,
}

impl UiLoop {
    /// Assembles the loop.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn new(
        surface: Box<dyn UiSurface>,
        channels: Arc<CommandChannels>,
        events: Arc<UiEventQueue>,
        config: UiThreadConfig,
    ) -> Self {
        Self {
            surface,
            channels,
            events,
            config,
            deadline: None,
        }
    }

    /// Runs until the host asks the thread to stop.
    ///
    /// # Errors
    ///
    /// Returns the first error the surface reports, and the channel error that
    /// means the loop can never be woken again. Either way the thread exits and
    /// the host's later posts are discarded, because a candidate window that
    /// cannot be driven must not take typing down with it.
    ///
    /// # Panics
    ///
    /// This function does not panic. The thread that calls it wraps it in a
    /// panic guard regardless, because a surface is code this module does not
    /// control.
    pub fn run(&mut self) -> Result<(), ImeError> {
        // One render before blocking: a surface that has something to show from
        // construction must not have to wait for the first command to appear.
        // The call is dirty-checked, so an idle surface still renders nothing.
        self.deadline = self.surface.render(Instant::now())?;
        loop {
            if self.wait()? {
                // Draining before applying is what makes the counter reliable: a
                // post that arrived while this iteration ran is folded into the
                // same batch, and the counter is left empty for the next wait to
                // block on.
                self.channels.wakeup().drain()?;
                self.apply_commands()?;
            }
            // Drained on every iteration rather than only when the connection
            // was readable: a surface with no descriptor of its own -- a
            // headless one, or a backend that reads its own socket -- must still
            // get to post, and a non-blocking poll of an idle connection costs
            // one failed read.
            self.surface
                .drain_events(&self.events, self.config.event_batch)?;
            if self.channels.is_shutdown() {
                return self.surface.close();
            }
            self.deadline = self.surface.render(Instant::now())?;
        }
    }

    /// Blocks until a descriptor is ready or the frame deadline passes.
    ///
    /// Returns whether the wakeup counter fired, which is the signal that the
    /// host posted something and the counter needs draining.
    fn wait(&mut self) -> Result<bool, ImeError> {
        let timeout = self.poll_timeout();
        let surface_fd = self.surface.event_fd();
        Ok(self.channels.wakeup().wait(surface_fd, timeout)?)
    }

    /// The wait: infinite while idle, the frame deadline while animating.
    fn poll_timeout(&self) -> Option<Duration> {
        let deadline = self.deadline?;
        Some(
            deadline
                .saturating_duration_since(Instant::now())
                .max(MIN_FRAME_TIMEOUT),
        )
    }

    /// Applies everything the host posted before the loop was woken.
    ///
    /// The frame, the theme and the overlay are taken before the ordered commands, so a
    /// `Show` that arrives with the frame it belongs to is applied against the
    /// current state rather than the previous one. The overlay is a mode like the theme
    /// -- a latest-wins slot whose newest value is the only one that describes what the
    /// user is looking at -- and an open that coalesced into a pending close applies the
    /// close, which is the state the user asked for last.
    fn apply_commands(&mut self) -> Result<(), ImeError> {
        if let Some(frame) = self.channels.take_frame() {
            self.surface.apply(SurfaceUpdate::Frame(frame))?;
        }
        if let Some(theme) = self.channels.take_theme() {
            self.surface.apply(SurfaceUpdate::Theme(theme))?;
        }
        if let Some(overlay) = self.channels.take_overlay() {
            self.surface.apply(SurfaceUpdate::Overlay(overlay))?;
        }
        while let Some(command) = self.channels.pop_control() {
            if let Some(update) = SurfaceUpdate::from_control(command) {
                self.surface.apply(update)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::os::fd::BorrowedFd;
    use std::rc::Rc;
    use std::sync::atomic::AtomicBool;

    use ime_types::UiCommand;
    use ime_types::ui::{OverlayFrame, OverlayKind};

    use crate::channel::{ChannelConfig, UiCommandSender, Wakeup};
    use crate::ui_thread::UiThreadConfig;

    use super::*;

    /// A surface that never reports anything, used to reach the timeout
    /// computation without starting a thread.
    struct Idle;

    impl UiSurface for Idle {
        fn event_fd(&self) -> Option<BorrowedFd<'_>> {
            None
        }

        fn apply(&mut self, _update: SurfaceUpdate) -> Result<(), ImeError> {
            Ok(())
        }

        fn drain_events(&mut self, _events: &UiEventQueue, _limit: usize) -> Result<(), ImeError> {
            Ok(())
        }

        fn render(&mut self, _now: Instant) -> Result<Option<Instant>, ImeError> {
            Ok(None)
        }

        fn close(&mut self) -> Result<(), ImeError> {
            Ok(())
        }
    }

    fn test_loop(deadline: Option<Instant>) -> UiLoop {
        test_loop_with(Box::new(Idle), deadline)
    }

    /// Assembles the loop around the surface the case hands in.
    fn test_loop_with(surface: Box<dyn UiSurface>, deadline: Option<Instant>) -> UiLoop {
        let config = ChannelConfig::default();
        let channels =
            Arc::new(CommandChannels::new(&config).expect("the wakeup counter can be created"));
        let events = Arc::new(UiEventQueue::new(&config));
        let mut loop_state = UiLoop::new(
            surface,
            channels,
            events,
            UiThreadConfig {
                thread_name: "rspinyin-ui-test",
                stack_size: 0,
                event_batch: 4,
                channels: config,
            },
        );
        loop_state.deadline = deadline;
        loop_state
    }

    /// A surface that reports one descriptor of its own: the stand-in for a display
    /// connection the loop is expected to watch beside the wakeup counter.
    struct Connected(Wakeup);

    impl UiSurface for Connected {
        fn event_fd(&self) -> Option<BorrowedFd<'_>> {
            Some(self.0.fd())
        }

        fn apply(&mut self, _update: SurfaceUpdate) -> Result<(), ImeError> {
            Ok(())
        }

        fn drain_events(&mut self, _events: &UiEventQueue, _limit: usize) -> Result<(), ImeError> {
            Ok(())
        }

        fn render(&mut self, _now: Instant) -> Result<Option<Instant>, ImeError> {
            Ok(None)
        }

        fn close(&mut self) -> Result<(), ImeError> {
            Ok(())
        }
    }

    #[test]
    fn test_wait_ends_when_the_surface_descriptor_turns_ready() {
        // The connection descriptor rides in the poll set beside the wakeup counter, so
        // its readiness has to end an otherwise indefinite wait -- that is the whole point
        // of putting it there. It must also not be reported as a host wakeup: a pointer
        // event costs a surface drain, never a counter drain and a command pass
        // (`BUDGET-CPU-01` counts the false wakeups).
        let connection = Wakeup::new().expect("an eventfd can be created");
        connection.wake().expect("the descriptor accepts a wake");
        let mut loop_state = test_loop_with(
            Box::new(Connected(connection)),
            Some(Instant::now() + Duration::from_secs(5)),
        );
        let started = Instant::now();
        let woken = loop_state.wait().expect("a ready descriptor is not an error");
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "the wait ended on the surface descriptor, not on its timeout"
        );
        assert!(!woken, "a pointer event is not a host wakeup");
    }

    #[test]
    fn test_poll_timeout_without_a_deadline_waits_forever() {
        let loop_state = test_loop(None);
        assert!(
            loop_state.poll_timeout().is_none(),
            "an idle surface must be allowed to block indefinitely"
        );
    }

    #[test]
    fn test_poll_timeout_with_a_past_deadline_is_clamped_to_the_floor() {
        let loop_state = test_loop(Some(Instant::now() - Duration::from_secs(5)));
        let timeout = loop_state.poll_timeout().expect("a deadline is set");
        // The floor is what stops a surface that keeps reporting a past deadline
        // from turning the loop into a busy poll.
        assert_eq!(timeout, MIN_FRAME_TIMEOUT);
    }

    #[test]
    fn test_poll_timeout_with_a_future_deadline_waits_for_it() {
        let loop_state = test_loop(Some(Instant::now() + Duration::from_millis(7)));
        let timeout = loop_state.poll_timeout().expect("a deadline is set");
        // The wait tracks the deadline rather than the floor: a frame interval
        // at any refresh rate is far above one millisecond.
        assert!(
            (MIN_FRAME_TIMEOUT..=Duration::from_millis(7)).contains(&timeout),
            "the wait is the remaining frame interval, got {timeout:?}"
        );
    }

    /// A surface that records every update it is applied, in order.
    struct Recorder(Rc<RefCell<Vec<SurfaceUpdate>>>);

    impl UiSurface for Recorder {
        fn event_fd(&self) -> Option<BorrowedFd<'_>> {
            None
        }

        fn apply(&mut self, update: SurfaceUpdate) -> Result<(), ImeError> {
            self.0.borrow_mut().push(update);
            Ok(())
        }

        fn drain_events(&mut self, _events: &UiEventQueue, _limit: usize) -> Result<(), ImeError> {
            Ok(())
        }

        fn render(&mut self, _now: Instant) -> Result<Option<Instant>, ImeError> {
            Ok(None)
        }

        fn close(&mut self) -> Result<(), ImeError> {
            Ok(())
        }
    }

    /// One overlay frame, as the engine posts it.
    fn overlay_frame() -> Box<OverlayFrame> {
        Box::new(OverlayFrame {
            kind: OverlayKind::CheatSheet,
            title: String::from("快捷键"),
            sections: Vec::new(),
            selected: None,
            query: String::new(),
        })
    }

    #[test]
    fn test_apply_commands_drains_the_overlay_slot_into_the_surface() {
        let config = ChannelConfig::default();
        let channels =
            Arc::new(CommandChannels::new(&config).expect("the wakeup counter can be created"));
        let events = Arc::new(UiEventQueue::new(&config));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut loop_state = UiLoop::new(
            Box::new(Recorder(Rc::clone(&seen))),
            Arc::clone(&channels),
            events,
            UiThreadConfig {
                thread_name: "rspinyin-ui-test",
                stack_size: 0,
                event_batch: 4,
                channels: config,
            },
        );
        let sender = UiCommandSender::new(Arc::clone(&channels), Arc::new(AtomicBool::new(false)));
        sender
            .send(UiCommand::Overlay(Some(overlay_frame())))
            .expect("the channel is open");
        sender
            .send(UiCommand::Overlay(None))
            .expect("the channel is open");
        loop_state.apply_commands().expect("the drain is applied");
        let overlays: Vec<SurfaceUpdate> = seen
            .borrow()
            .iter()
            .filter(|update| matches!(update, SurfaceUpdate::Overlay(_)))
            .cloned()
            .collect();
        assert_eq!(
            overlays,
            [SurfaceUpdate::Overlay(None)],
            "the slot kept only the newest state -- the close -- and the loop applied it \
             to the surface, which is what lets a closed panel close"
        );
        assert!(
            !sender.channels().is_shutdown(),
            "the drain never touches the shutdown flag"
        );
    }
}
