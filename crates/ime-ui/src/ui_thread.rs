//! The UI thread: its event loop, its channels and its lifecycle.
//!
//! One thread owns everything that touches the display -- the platform object,
//! the raster buffers and the connection to the compositor -- and the host
//! thread reaches it only through a channel and a wakeup. That separation is
//! what lets the host thread answer a keystroke without ever waiting for a
//! frame, and it is why this module exists rather than the rendering being
//! called from the fcitx5 callback.
//!
//! # Lifecycle
//!
//! [`UiThread::spawn`] creates the channels and starts the thread, which builds
//! the surface through the factory it was given and then runs [`UiLoop`] until
//! the host asks it to stop. The factory runs *on* the UI thread because the
//! platform object has to be installed there; it receives a [`UiContext`] whose
//! only job is to hand back the wakeup handle the surface needs when it wants a
//! redraw.
//!
//! [`UiThread::shutdown`] sets the stop flag, wakes the thread and waits for it
//! for at most the timeout it is given. It is idempotent, and it never blocks
//! for longer than that timeout: a UI thread that will not stop is detached
//! rather than allowed to hold up the host's exit.
//!
//! # Failure isolation
//!
//! A panic on the UI thread is caught, the thread is marked dead, and every
//! later post is discarded with [`UiError::ThreadDead`]. Typing keeps working
//! without a candidate window, which is the correct trade for an input method:
//! losing the window is an inconvenience, losing the keyboard is not.

mod event_loop;
mod surface;

#[cfg(test)]
mod tests;

pub use self::event_loop::UiLoop;
pub use self::surface::{SurfaceUpdate, UiSurface};

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ime_types::{ImeError, UiCommand, UiError, UiEvent};

use crate::channel::{
    ChannelConfig, ChannelStats, CommandChannels, UiCommandSender, UiEventQueue, Wakeup,
};

/// The stack the UI thread runs on, in bytes.
///
/// Software rasterization is iterative, so the thread needs far less stack than
/// a default thread, and a small fixed size keeps the thread's footprint inside
/// the memory budget.
const UI_THREAD_STACK_SIZE: usize = 512 * 1024;

/// The thread name, chosen so that `ps -T` and the diagnostics agree on it.
const UI_THREAD_NAME: &str = "rspinyin-ui";

/// Tuning of the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiThreadConfig {
    /// Thread name, which is what a diagnostic or `ps -T` shows.
    pub thread_name: &'static str,
    /// Stack size in bytes; zero means the platform default.
    pub stack_size: usize,
    /// Most surface events one loop iteration may consume, so that a burst of
    /// pointer motion cannot starve rendering.
    pub event_batch: usize,
    /// Capacities and wait budgets of the channels.
    pub channels: ChannelConfig,
}

impl Default for UiThreadConfig {
    fn default() -> Self {
        Self {
            thread_name: UI_THREAD_NAME,
            stack_size: UI_THREAD_STACK_SIZE,
            event_batch: 64,
            channels: ChannelConfig::default(),
        }
    }
}

/// What the surface factory is handed when the UI thread starts.
///
/// The context exists so that the factory's signature can grow without breaking
/// every implementation of it; today it carries the wakeup handle and nothing
/// else.
#[derive(Clone, Debug)]
pub struct UiContext {
    waker: Wakeup,
}

impl UiContext {
    /// The handle a surface uses to ask for a redraw.
    ///
    /// Waking the loop is how a surface that changed itself -- a dirty flag set
    /// from a callback, a frame callback that arrived -- gets rendered; it costs
    /// one syscall and never blocks.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn waker(&self) -> &Wakeup {
        &self.waker
    }
}

/// The host thread's handle on one running UI thread.
///
/// # Concurrency
///
/// `Send` and `Sync`. The host thread calls the methods below; none of them
/// blocks beyond the budget it is given, and [`UiThread::send`] blocks only for
/// the ordered channel's bounded wait.
pub struct UiThread {
    handle: Mutex<Option<JoinHandle<()>>>,
    sender: UiCommandSender,
    events: Arc<UiEventQueue>,
    dead: Arc<AtomicBool>,
    panicked: Arc<AtomicBool>,
    shutdown_timeouts: AtomicU64,
}

impl UiThread {
    /// Starts the UI thread.
    ///
    /// `build` runs on the new thread and produces the surface the loop drives.
    /// It is a factory rather than a value because the platform object has to be
    /// installed on the thread that will run the loop, and because a surface
    /// that fails to start must fail here, before the host believes it has a
    /// candidate window.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::UiChannelClosed`] when the wakeup counter cannot be
    /// created or the thread cannot be started, which means the candidate window
    /// will not appear and the host should stay on its fallback path.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn spawn<F>(config: UiThreadConfig, build: F) -> Result<Self, ImeError>
    where
        F: FnOnce(UiContext) -> Result<Box<dyn UiSurface>, ImeError> + Send + 'static,
    {
        let channels = Arc::new(CommandChannels::new(&config.channels)?);
        let events = Arc::new(UiEventQueue::new(&config.channels));
        let dead = Arc::new(AtomicBool::new(false));
        let panicked = Arc::new(AtomicBool::new(false));
        let context = UiContext {
            waker: channels.wakeup().clone(),
        };
        let name = config.thread_name.to_owned();
        let stack_size = config.stack_size;
        let state = ThreadState {
            config,
            channels: Arc::clone(&channels),
            events: Arc::clone(&events),
            dead: Arc::clone(&dead),
            panicked: Arc::clone(&panicked),
            build: Some(build),
        };
        let mut builder = thread::Builder::new().name(name);
        if stack_size > 0 {
            builder = builder.stack_size(stack_size);
        }
        let handle = builder
            .spawn(move || run_ui_thread(state, context))
            .map_err(|_| ImeError::UiChannelClosed)?;
        Ok(Self {
            handle: Mutex::new(Some(handle)),
            sender: UiCommandSender::new(channels, Arc::clone(&dead)),
            events,
            dead,
            panicked,
            shutdown_timeouts: AtomicU64::new(0),
        })
    }

    /// Posts one command to the UI thread.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::ThreadDead`] when the UI thread has already exited, in
    /// which case the command is discarded rather than queued. Returns
    /// [`UiError::ChannelClosed`] when the wakeup cannot be delivered, which
    /// means the command will never be seen.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn send(&self, command: UiCommand) -> Result<(), UiError> {
        self.sender.send(command)
    }

    /// Waits up to `timeout` for the next event from the UI thread.
    ///
    /// Pass [`Duration::ZERO`] from the host's own event loop: an input method
    /// must never block the thread that is handling keys. A non-zero timeout is
    /// for a caller that owns a thread of its own.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn poll_event(&self, timeout: Duration) -> Option<UiEvent> {
        self.events.poll(timeout)
    }

    /// Asks the UI thread to stop and waits up to `timeout` for it to do so.
    ///
    /// Idempotent: a second call after the thread has been joined returns
    /// immediately. A thread that does not stop within the timeout is detached,
    /// and the counter behind `ui/shutdown/timeout` records it; the host is
    /// never held up for longer than the timeout it asked for.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::UiChannelClosed`] when the stop request cannot be
    /// delivered, which means the thread will keep running until the process
    /// exits.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn shutdown(&self, timeout: Duration) -> Result<(), ImeError> {
        let channels = self.sender.channels();
        channels.request_shutdown();
        channels.wakeup().wake()?;
        let handle = match self.handle.lock() {
            Ok(mut slot) => slot.take(),
            // A panic while the handle was locked cannot leave the `Option`
            // inconsistent, so the handle is still the one to join.
            Err(poisoned) => poisoned.into_inner().take(),
        };
        let Some(handle) = handle else {
            return Ok(());
        };
        join_within(handle, timeout, &self.shutdown_timeouts)
    }

    /// Whether the UI thread is still running.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn is_alive(&self) -> bool {
        !self.dead.load(Ordering::Acquire)
    }

    /// A snapshot of every counter the channels maintain.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn stats(&self) -> ChannelStats {
        let channels = self.sender.channels();
        ChannelStats {
            frames_coalesced: channels.frames_coalesced(),
            themes_coalesced: channels.themes_coalesced(),
            controls_collapsed: channels.controls_collapsed(),
            select_timeouts: self.events.select_timeouts(),
            thread_dead: self.dead.load(Ordering::Acquire),
            thread_panicked: self.panicked.load(Ordering::Acquire),
            shutdown_timeouts: self.shutdown_timeouts.load(Ordering::Relaxed),
        }
    }
}

impl Drop for UiThread {
    fn drop(&mut self) {
        let channels = self.sender.channels();
        channels.request_shutdown();
        let _ = channels.wakeup().wake();
        // Dropping the handle detaches the thread rather than joining it. A
        // handle that is dropped while the thread is still running must never
        // hold up whatever is tearing the addon down; an explicit
        // `shutdown(timeout)` is how a caller waits.
        if let Ok(mut slot) = self.handle.lock() {
            let _ = slot.take();
        }
    }
}

/// The state the UI thread owns for its whole life.
struct ThreadState<F> {
    config: UiThreadConfig,
    channels: Arc<CommandChannels>,
    events: Arc<UiEventQueue>,
    dead: Arc<AtomicBool>,
    panicked: Arc<AtomicBool>,
    build: Option<F>,
}

impl<F> ThreadState<F>
where
    F: FnOnce(UiContext) -> Result<Box<dyn UiSurface>, ImeError>,
{
    /// Builds the surface and runs the loop.
    fn run(&mut self, context: UiContext) -> Result<(), ImeError> {
        let build = self.build.take().ok_or(ImeError::UiChannelClosed)?;
        let surface = build(context)?;
        let mut ui_loop = UiLoop::new(
            surface,
            Arc::clone(&self.channels),
            Arc::clone(&self.events),
            self.config.clone(),
        );
        ui_loop.run()
    }
}

/// The UI thread's entry point.
///
/// The whole body is wrapped in a panic guard: the surface is code this module
/// does not control, and a panic that unwound into the host process would take
/// the user's typing down with the candidate window.
fn run_ui_thread<F>(mut state: ThreadState<F>, context: UiContext)
where
    F: FnOnce(UiContext) -> Result<Box<dyn UiSurface>, ImeError> + Send + 'static,
{
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| state.run(context)));
    if outcome.is_err() {
        state.panicked.store(true, Ordering::Release);
    }
    // Stored last, so that a `send` which observes it knows the loop is gone
    // rather than merely about to be.
    state.dead.store(true, Ordering::Release);
}

/// Waits for the UI thread to finish, for at most `timeout`.
///
/// `JoinHandle::join` has no timeout and the host must never be held up by a
/// thread that will not stop, so the join happens on a helper thread and the
/// caller waits on a channel instead. On timeout the helper is detached: it
/// stays parked on the join and disappears with the process, which is strictly
/// better than making the host's exit wait for it.
fn join_within(
    handle: JoinHandle<()>,
    timeout: Duration,
    timeouts: &AtomicU64,
) -> Result<(), ImeError> {
    let (finished, done) = mpsc::channel();
    let _helper = thread::Builder::new()
        .name(String::from("rspinyin-ui-join"))
        .spawn(move || {
            let _ = handle.join();
            let _ = finished.send(());
        })
        .map_err(|_| ImeError::UiChannelClosed)?;
    match done.recv_timeout(timeout) {
        Ok(()) => Ok(()),
        Err(_) => {
            timeouts.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }
}
