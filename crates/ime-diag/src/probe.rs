//! Runtime probes: the numbers behind the performance budgets.
//!
//! Responsibility: measure how long the key path takes, where it waits, and how
//! often the design's degradations fire -- and stay cheap enough that the
//! measurement never shows up in the budget it exists to verify. A sample is a
//! bucket lookup and a relaxed atomic add: no lock, no allocation, no formatting
//! anywhere on the path.
//!
//! # Boundaries
//!
//! This module measures and stores; it interprets nothing. Turning a snapshot into a
//! verdict against `docs/dev/budgets.json` is [`crate::report`]'s job, and deciding
//! *where* to take a sample is the wiring's: the host thread stamps a key on its way
//! in and completes the sample when the render receipt comes back, the UI thread
//! stamps the wakeup and the first-visible latencies.
//!
//! The probes never reach into the decoder. `ime-core` and `ime-dict` may not depend
//! on this crate at all -- the dependency audit enforces it -- so a decode is timed
//! by the caller that calls it, and the decode boundary stays a pure function of its
//! inputs, with no clock and no shared state inside it.
//!
//! # Privacy
//!
//! A probe records durations and counts. There is no field here that could hold a
//! character a user typed, and the snapshot carries metric names, counter names and
//! numbers, which is what makes a report safe to attach to a bug report.
//!
//! # Cost
//!
//! One sample is two or three relaxed atomic adds on a structure that is already in
//! cache, and a key press produces a handful of them, so the probes cost a few
//! hundred nanoseconds against a budget stated in milliseconds. Switched off, every
//! entry point returns before it reads the clock, so the configuration's switch is
//! free rather than merely cheap.
//!
//! # The host thread's path
//!
//! A key press crosses four points on its way out of the plugin: the callback the host
//! calls, the session step that turns the key into an effect, the frame that step
//! produced, and the hand-off of that frame to the UI thread. The path is sampled at
//! both ends, because the budget is stated for the whole of it while a regression has
//! to be attributed to a part: [`Metric::EventLoopKey`] spans the callback from its
//! entry to the point where the post has returned, and [`Metric::PostUi`] spans the
//! post itself. The step in the middle is [`Metric::Decode`], stamped by the caller
//! that runs the state machine: the frame is built inside that step, so the decode and
//! the frame it produces are one segment rather than two, and nothing can be placed
//! between them because the code between them may not depend on this crate.
//!
//! Taking a sample never does the work it measures. [`Probes::begin`] reads the clock
//! once, the [`Stamp`] it answers with adds one to a bucket when its scope ends, and that
//! is the whole of what a callback body has to do to be measured: no allocation, no lock,
//! no formatting, no file, no syscall. Everything a reader wants -- the percentiles, the
//! verdict, the text of a report -- is produced when the snapshot is taken, which happens
//! off the key path. Switched off, both halves return before they touch the clock.

mod counters;
mod histogram;
mod metric;
mod snapshot;
mod stamp;

#[cfg(test)]
mod tests;

pub use self::counters::{COUNTER_COUNT, Counter};
pub use self::histogram::{BUCKETS, HistSnapshot, Histogram, Percentile, TOP_US};
pub use self::metric::{METRIC_COUNT, Metric, Unit};
pub use self::snapshot::{Metrics, ProbeSnapshot, SNAPSHOT_FILE_NAME, SNAPSHOT_HEADER};
pub use self::stamp::Stamp;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A key press waiting for its render receipt.
///
/// Taken on the host thread when the key arrives and carried beside the frame to the
/// UI thread, which returns it once the frame has been committed; the sample is
/// completed on the host thread when the receipt comes back. The token is `Copy` and
/// carries no reference to the probes, so it can travel through the channel that
/// carries the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyToken {
    /// When the key arrived, or `None` when the probes were switched off.
    start: Option<Instant>,
    /// Which sample this is, counted from the probes' creation.
    seq: u64,
}

impl KeyToken {
    /// When the key arrived, or `None` when the probes were switched off.
    pub fn start(&self) -> Option<Instant> {
        self.start
    }

    /// Which sample this is.
    ///
    /// Only unique within one process run, and only among the samples that were
    /// taken while the probes were on; it exists to pair a receipt with its stamp in
    /// a log, not to be an identifier anything else relies on.
    pub fn seq(&self) -> u64 {
        self.seq
    }
}

/// Every probe one plugin instance records into.
///
/// The structure is created by the addon, lives as long as the plugin, and is shared
/// by reference between the host thread and the UI thread. Nothing here blocks, so
/// neither thread can be delayed by the other through it.
#[derive(Debug)]
pub struct Probes {
    /// Key press to presented frame.
    pub key_to_present: Histogram,
    /// One decode call.
    pub decode: Histogram,
    /// One full-frame raster.
    pub raster_full: Histogram,
    /// One partial-frame raster.
    pub raster_partial: Histogram,
    /// First key to a visible window.
    pub first_key_to_visible: Histogram,
    /// `eventfd` wakeup to the UI thread returning from `poll`.
    pub wakeup: Histogram,
    /// One `on_key_event`, decode and posting included.
    pub event_loop_key: Histogram,
    /// One command handed to the UI thread, from the host thread's side.
    pub post_ui: Histogram,
    counters: [AtomicU64; COUNTER_COUNT],
    sessions: AtomicU64,
    keys: AtomicU64,
    seq: AtomicU64,
    enabled: AtomicBool,
    start: Instant,
}

impl Probes {
    /// Probes with nothing recorded yet, switched on.
    pub fn new() -> Self {
        Self {
            key_to_present: Histogram::new(),
            decode: Histogram::new(),
            raster_full: Histogram::new(),
            raster_partial: Histogram::new(),
            first_key_to_visible: Histogram::new(),
            wakeup: Histogram::new(),
            event_loop_key: Histogram::new(),
            post_ui: Histogram::new(),
            counters: [const { AtomicU64::new(0) }; COUNTER_COUNT],
            sessions: AtomicU64::new(0),
            keys: AtomicU64::new(0),
            seq: AtomicU64::new(0),
            enabled: AtomicBool::new(true),
            start: Instant::now(),
        }
    }

    /// Switches every probe on or off.
    ///
    /// Switched off, the histograms stop recording and the counters stop counting:
    /// nothing is measured, so nothing half-measured reaches a report.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        for metric in Metric::ALL {
            metric.histogram(self).set_enabled(enabled);
        }
    }

    /// Whether the probes are switched on.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Counts one occurrence of `counter`.
    pub fn bump(&self, counter: Counter) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        self.counters[counter.index()].fetch_add(1, Ordering::Relaxed);
    }

    /// One counter's value.
    ///
    /// # Panics
    ///
    /// Never: the index comes from the counter itself.
    pub fn counter(&self, counter: Counter) -> u64 {
        self.counters[counter.index()].load(Ordering::Relaxed)
    }

    /// Records that an input session was opened.
    ///
    /// The count is what tells a reader how many sessions a window of typing covered:
    /// five minutes of one session and five minutes of two hundred are not the same
    /// sample.
    pub fn session_started(&self) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        self.sessions.fetch_add(1, Ordering::Relaxed);
    }

    /// How long the probes have been running.
    pub fn sampled(&self) -> Duration {
        self.start.elapsed()
    }

    /// Starts an end-to-end latency sample and returns the token that completes it.
    ///
    /// Called on the host thread as a key event arrives. The token carries the
    /// instant rather than the probes holding a start time, because the two halves of
    /// the measurement run on different threads and several samples can be in flight
    /// at once.
    pub fn begin_key_to_present(&self) -> KeyToken {
        if !self.enabled.load(Ordering::Relaxed) {
            return KeyToken {
                start: None,
                seq: 0,
            };
        }
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        self.keys.fetch_add(1, Ordering::Relaxed);
        KeyToken {
            start: Some(Instant::now()),
            seq,
        }
    }

    /// Completes an end-to-end latency sample.
    ///
    /// Called on the host thread when the render receipt for the frame the token
    /// travelled with comes back. A token taken while the probes were off records
    /// nothing, and so does one whose receipt arrived after the probes were switched
    /// off: a sample that spans the switch is dropped rather than half-counted.
    ///
    /// A sample whose receipt never arrives -- the receipt is a latest-wins slot, so a
    /// burst of keystrokes can replace one -- is not completed at all, and the caller
    /// counts it as `probe.lost` so that a report can say its sample was incomplete.
    pub fn end_key_to_present(&self, token: KeyToken) {
        let Some(start) = token.start else {
            return;
        };
        self.key_to_present.record(start.elapsed());
    }

    /// Every measurement, as a report reads it.
    pub fn snapshot(&self) -> ProbeSnapshot {
        let mut counters = [0_u64; COUNTER_COUNT];
        for (slot, counter) in counters.iter_mut().zip(Counter::ALL) {
            *slot = self.counter(counter);
        }
        ProbeSnapshot {
            sampled: self.sampled(),
            sessions: self.sessions.load(Ordering::Relaxed),
            keys: self.keys.load(Ordering::Relaxed),
            metrics: Metrics {
                key_to_present: self.key_to_present.snapshot(),
                decode: self.decode.snapshot(),
                raster_full: self.raster_full.snapshot(),
                raster_partial: self.raster_partial.snapshot(),
                first_key_to_visible: self.first_key_to_visible.snapshot(),
                wakeup: self.wakeup.snapshot(),
                event_loop_key: self.event_loop_key.snapshot(),
                post_ui: self.post_ui.snapshot(),
            },
            counters,
        }
    }
}

impl Default for Probes {
    fn default() -> Self {
        Self::new()
    }
}
