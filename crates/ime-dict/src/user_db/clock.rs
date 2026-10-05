//! The time sources of the user frequency store.
//!
//! Responsibility: define the clock the store reads and the production
//! implementation behind it. The commit interval, the `last_used_ms` stamps and the
//! idle sweep are all timing decisions, and no test in this workspace may depend on
//! the wall clock, so the time arrives as an injected [`Clock`] rather than as
//! calls to the standard library scattered through the store.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The time source the store reads.
///
/// Injected rather than called directly because the commit interval, the `last_used_ms`
/// stamps and the idle sweep are all timing decisions, and no test in this workspace may
/// depend on the wall clock.
pub trait Clock: Send + Sync {
    /// Wall-clock milliseconds since the Unix epoch, stored per record.
    fn now_ms(&self) -> u64;
    /// Nanoseconds of a monotonic clock; only differences are meaningful.
    fn now_nanos(&self) -> u64;
}

/// The clock a running store uses: the system wall clock and a monotonic counter started
/// when the store was opened.
#[derive(Debug)]
pub struct SystemClock {
    epoch: Instant,
}

impl SystemClock {
    /// Starts a clock whose monotonic reading is zero now.
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            })
    }

    fn now_nanos(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}
