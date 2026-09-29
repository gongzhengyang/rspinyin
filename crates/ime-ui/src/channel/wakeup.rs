//! The wakeup and waiting primitive that ties the host thread to the UI thread.
//!
//! One `eventfd` is shared by both threads. The host writes to it after every
//! post, and the UI thread waits on it -- together with the display connection's
//! own descriptor when there is one -- so a post costs the host one syscall and
//! the UI thread never has to poll: it blocks until something it already watches
//! becomes ready.
//!
//! # Why a counter and not a pipe
//!
//! An `eventfd` is a single 64-bit counter with read-resets-to-zero semantics,
//! so coalescing is free: ten posts before the UI thread wakes are one `read`,
//! and the wakeup path cannot back up behind its own queue. The price is that
//! the reader must drain until `EAGAIN` rather than read once -- a single read
//! would drop every wakeup that arrived while the loop was busy -- which is
//! exactly what [`Wakeup::drain`] does.
//!
//! # Why this file is the only one that names a platform crate
//!
//! Creating an `eventfd`, waiting on it and reading from it are syscalls, `libc`
//! is not a dependency of this workspace, and `ime-ui` is not on the allowlist
//! that permits low-level system calls. The safe wrapper used here is `rustix`,
//! and this module is the only place in the crate that mentions it: the event
//! loop and the channels see the [`Wakeup`] handle and the `std` descriptor
//! types it exposes, and nothing else.

use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::Arc;
use std::time::Duration;

use ime_types::UiError;
use rustix::event::{EventfdFlags, PollFd, PollFlags, Timespec, eventfd, poll};
use rustix::io::{Errno, read, write};

/// The wakeup counter shared by the host thread and the UI thread.
///
/// Cloning the handle clones a reference to the same counter, so the host can
/// keep a copy for as long as it needs one. The descriptor is closed when the
/// last handle is dropped.
///
/// # Concurrency
///
/// `Send` and `Sync`: the descriptor is owned by an `Arc` and the only
/// operations are the syscalls, which are atomic with respect to each other. No
/// method blocks except [`Wakeup::wait`], and that one blocks for at most the
/// timeout it is given.
#[derive(Clone, Debug)]
pub struct Wakeup {
    fd: Arc<OwnedFd>,
}

impl Wakeup {
    /// Creates the wakeup counter.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::ChannelClosed`] when the kernel refuses a new
    /// descriptor, which means the process is out of file descriptors and no UI
    /// thread can be started.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn new() -> Result<Self, UiError> {
        // `EFD_CLOEXEC` keeps the descriptor out of any child the host spawns;
        // `EFD_NONBLOCK` is what lets the producer's write and the consumer's
        // drain both return instead of ever blocking a thread.
        let fd = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)
            .map_err(|_| UiError::ChannelClosed)?;
        Ok(Self { fd: Arc::new(fd) })
    }

    /// Signals the other thread that there is something to look at.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::ChannelClosed`] when the descriptor is no longer
    /// usable, which means the UI thread can never be woken again.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn wake(&self) -> Result<(), UiError> {
        // An `eventfd` rejects a write that is not exactly eight bytes wide.
        let mut counter = 1u64.to_ne_bytes();
        match write(self.fd.as_fd(), &mut counter[..]) {
            Ok(_) => Ok(()),
            // A saturated counter means a wakeup is already pending, which is
            // the state the caller asked for; reporting a failure here would
            // turn a harmless coalescing into a spurious error.
            Err(Errno::AGAIN) => Ok(()),
            Err(_) => Err(UiError::ChannelClosed),
        }
    }

    /// Consumes every pending wakeup.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::ChannelClosed`] when the descriptor is no longer
    /// usable.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn drain(&self) -> Result<(), UiError> {
        let mut buf = [0u8; 8];
        loop {
            match read(self.fd.as_fd(), &mut buf[..]) {
                // Each read returns the accumulated counter and resets it, so
                // stopping after the first one would lose every wakeup that
                // arrived while this loop was running.
                Ok(_) => {}
                Err(Errno::AGAIN) => return Ok(()),
                Err(_) => return Err(UiError::ChannelClosed),
            }
        }
    }

    /// Blocks until the counter is written or `surface` becomes readable.
    ///
    /// `surface` is the display connection's descriptor, which is `None` for a
    /// surface that has no connection of its own. `timeout` of `None` waits
    /// indefinitely, which is what makes an idle UI thread cost nothing; a
    /// timeout is only passed while an animation is running and the next frame
    /// is due.
    ///
    /// Returns whether the wakeup counter fired. A `false` return therefore
    /// means either that the surface became readable or that the timeout
    /// expired, and the caller treats both the same way: look at everything,
    /// then wait again.
    ///
    /// # Errors
    ///
    /// Returns [`UiError::ChannelClosed`] when the descriptors can no longer be
    /// watched, which means the UI thread can never be woken again.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn wait(
        &self,
        surface: Option<BorrowedFd<'_>>,
        timeout: Option<Duration>,
    ) -> Result<bool, UiError> {
        // A duration too large for a `timespec` cannot occur for a frame
        // deadline; an indefinite wait is the harmless reading of one that
        // somehow did.
        let timespec = timeout.and_then(|remaining| Timespec::try_from(remaining).ok());
        poll_once(self.fd(), surface, timespec.as_ref())
    }

    /// The descriptor to add to a `poll` set.
    ///
    /// # Panics
    ///
    /// This function does not panic.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

/// One `poll(2)` over the counter and an optional second descriptor.
///
/// `timeout` of `None` waits indefinitely. Returns whether the counter fired.
fn poll_once(
    wakeup: BorrowedFd<'_>,
    surface: Option<BorrowedFd<'_>>,
    timeout: Option<&Timespec>,
) -> Result<bool, UiError> {
    let mut fds = [
        PollFd::from_borrowed_fd(wakeup, PollFlags::IN),
        // Without a surface descriptor the second slot points at the same
        // counter and is polled for nothing, which keeps the array one type and
        // the call allocation-free. Its readiness is not consulted: a surface
        // with no descriptor has no events to report.
        PollFd::from_borrowed_fd(surface.unwrap_or(wakeup), PollFlags::IN),
    ];
    let count = if surface.is_some() { 2 } else { 1 };
    match poll(&mut fds[..count], timeout) {
        Ok(_) => {}
        // A signal is not an event: reporting a wakeup that did not happen lets
        // the caller recompute its deadline and wait again, which is cheaper
        // than propagating an error the caller cannot act on.
        Err(Errno::INTR) => return Ok(false),
        Err(_) => return Err(UiError::ChannelClosed),
    }
    Ok(fds[0].revents().contains(PollFlags::IN))
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd;

    use super::*;

    #[test]
    fn test_wakeup_wake_then_drain_consumes_the_counter() {
        let wakeup = Wakeup::new().expect("an eventfd can be created");
        wakeup.wake().expect("the counter accepts a write");
        // The first drain consumes the pending value, the second finds the
        // counter empty and must return rather than block or fail.
        wakeup.drain().expect("the pending wakeup is consumed");
        wakeup.drain().expect("an empty counter is not an error");
    }

    #[test]
    fn test_wakeup_drain_clears_a_burst_of_wakes() {
        let wakeup = Wakeup::new().expect("an eventfd can be created");
        for _ in 0..64 {
            wakeup.wake().expect("the counter accepts a write");
        }
        // One drain must clear all sixty-four: the counter accumulates and the
        // read resets it, so a second drain is not a second wakeup.
        wakeup.drain().expect("the burst is consumed");
        wakeup.drain().expect("the counter is empty afterwards");
    }

    #[test]
    fn test_wakeup_clone_shares_one_counter() {
        let producer = Wakeup::new().expect("an eventfd can be created");
        let consumer = producer.clone();
        producer.wake().expect("the counter accepts a write");
        consumer
            .drain()
            .expect("a clone observes the same counter and clears it");
        consumer.drain().expect("the counter is empty afterwards");
    }

    #[test]
    fn test_wakeup_each_counter_has_its_own_descriptor() {
        let first = Wakeup::new().expect("an eventfd can be created");
        let second = Wakeup::new().expect("an eventfd can be created");
        assert_ne!(first.fd().as_raw_fd(), second.fd().as_raw_fd());
    }

    #[test]
    fn test_wakeup_wait_reports_a_posted_wakeup() {
        let wakeup = Wakeup::new().expect("an eventfd can be created");
        wakeup.wake().expect("the counter accepts a write");
        let woken = wakeup
            .wait(None, Some(Duration::ZERO))
            .expect("polling a ready descriptor does not fail");
        assert!(woken, "the posted wakeup is what made it ready");
    }

    #[test]
    fn test_wakeup_wait_without_any_event_times_out() {
        let wakeup = Wakeup::new().expect("an eventfd can be created");
        let woken = wakeup
            .wait(None, Some(Duration::ZERO))
            .expect("an expired timeout is not an error");
        assert!(!woken, "an empty counter is not a wakeup");
    }

    #[test]
    fn test_wakeup_wait_without_a_timeout_returns_when_woken() {
        let wakeup = Wakeup::new().expect("an eventfd can be created");
        let producer = wakeup.clone();
        let poster = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            producer.wake().expect("the counter accepts a write");
        });
        // An indefinite wait is the idle case: it must still return as soon as
        // the host posts, and must not be a polling loop.
        let woken = wakeup
            .wait(None, None)
            .expect("an indefinite wait returns when woken");
        assert!(woken);
        poster.join().expect("the poster finishes");
    }
}
