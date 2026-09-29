//! Frame-rate sampling and dropped-frame detection.
//!
//! # Where a frame rate comes from on this platform
//!
//! X11 has no compositor frame callback: `SurfaceBackend::request_frame` returns `None`, and
//! the UI thread paces animation from a timer of its own. So the rate a case can measure is
//! not the refresh rate of a monitor -- nothing on this tier knows it -- but the rate the
//! window was actually presented at, which is the sequence of `commit` calls the UI thread
//! made.
//!
//! That sequence is what this module records. [`InstrumentedBackend`] wraps a backend and
//! writes one [`CommitRecord`] per commit; [`CommitLog::frame_stats`] turns the records into
//! a rate, the interval percentiles, and the count of gaps that missed the deadline the
//! caller's [`FrameRatePolicy`] states.
//!
//! # The decorator changes nothing
//!
//! Every method forwards, in the same order, with the same arguments, and returns what the
//! inner backend returned -- including a failure, which is passed through rather than
//! swallowed. The one thing it adds is a timestamp and a count. That restraint is the whole
//! point: a harness that skipped a commit or merged two damage sets to make its own numbers
//! look better would be measuring a frame rate no user ever saw, so the wrapper never
//! decides anything.
//!
//! # What a case can and cannot assert
//!
//! A rate and a drop count are statements about the run that produced them. The design's
//! idle budget is a count of *redraws* (`BUDGET-CPU-01`, `idle_redraw_count = 0`), which is
//! a claim about how many commits happened at all rather than how evenly they were spaced;
//! that assertion reads [`CommitLog::frames`] directly. Both numbers are only meaningful on
//! a machine that is not busy with something else, which is the purity guard's business and
//! not this module's.
//!
//! # Modules
//!
//! `log` holds the records and the statistics; this file holds the decorator that fills
//! them in.

// The channel is exercised by the tests below and is not yet reachable from `xtask`'s
// subcommand tree, which lives in `xtask/src/main.rs` and in `xtask/src/testd/mod.rs` -- two
// files this module does not own. Until that wiring lands, every item here is reported as
// dead code in a non-test build, and the attribute goes away with those lines.
//
// `unused_imports` is covered by the same reasoning: the `pub use` lines below are this
// module's surface, and a `pub use` in a *binary* crate is reported as unused whenever
// nothing in the crate names it.
#![allow(dead_code, unused_imports)]

use std::time::Instant;

use ime_types::{FrameToken, PixelBufferMut, PlatformError, RectI, SurfaceBackend, SurfaceEvent};

mod log;

#[cfg(test)]
mod tests;

pub use self::log::{
    CommitLog, CommitRecord, DEFAULT_TARGET_HZ, DROP_FACTOR, FrameRatePolicy, FrameStats, WINDOW,
    damage_px, normalize_drop_factor, normalize_target,
};

/// A backend that records what was committed through it.
///
/// It is generic over the backend rather than written for one, because the same measurement
/// has to hold for every tier: a Wayland run records the same records from a backend whose
/// `request_frame` answers a token, and the frame callback that redeemed the token is then
/// visible in the record it was attached to.
pub struct InstrumentedBackend<B: SurfaceBackend> {
    /// The backend every call is forwarded to.
    inner: B,
    /// What has been committed through it.
    log: CommitLog,
    /// The frame callback the next commit answers, when the backend asks for one.
    pending_frame: Option<FrameToken>,
}

impl<B: SurfaceBackend> InstrumentedBackend<B> {
    /// Wraps a backend, recording what is committed through it from now on.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new(inner: B) -> Self {
        Self {
            inner,
            log: CommitLog::new(),
            pending_frame: None,
        }
    }

    /// The backend back, for a caller that is done measuring.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn into_inner(self) -> B {
        self.inner
    }

    /// What has been committed so far.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn log(&self) -> &CommitLog {
        &self.log
    }

    /// The frame statistics of the commits so far, against a target rate.
    ///
    /// A convenience over [`Self::frame_stats_with`] for a case that has a rate and wants the
    /// design's own drop criterion.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn frame_stats(&self, target_hz: f32) -> FrameStats {
        self.log.frame_stats(target_hz)
    }

    /// The frame statistics of the commits so far, against a criterion.
    ///
    /// This is the form a case that measures a tier paced at a rate of its own uses: the
    /// commits were recorded whatever the criterion says, so the same run can be judged
    /// against several of them.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn frame_stats_with(&self, policy: &FrameRatePolicy) -> FrameStats {
        self.log.frame_stats_with(policy)
    }

    /// How many buffers were not free.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn acquire_misses(&self) -> u32 {
        self.log.acquire_misses()
    }
}

impl<B: SurfaceBackend> SurfaceBackend for InstrumentedBackend<B> {
    fn acquire_buffer(&mut self) -> Result<PixelBufferMut<'_>, PlatformError> {
        let acquired = self.inner.acquire_buffer();
        // Counted, and then handed on unchanged: a starved buffer is a fact about the run,
        // and the caller still has to skip the frame rather than be told it succeeded.
        if let Err(PlatformError::NoFreeBuffer) = &acquired {
            self.log.count_acquire_miss();
        }
        acquired
    }

    fn commit(&mut self, damage: &[RectI]) -> Result<(), PlatformError> {
        // The timestamp is taken before the frame goes out, so the interval between two
        // commits includes the previous one's cost: a commit that took longer than a frame
        // is exactly what a dropped frame looks like from the outside.
        let at = Instant::now();
        let damage_px = damage_px(damage);
        let frame_token = self.pending_frame.take();
        let committed = self.inner.commit(damage);
        // Recorded whatever the commit answered. A commit that failed still happened at that
        // moment, and dropping it would report a rate the run never had.
        self.log.push(at, damage_px, frame_token);
        committed
    }

    fn set_input_region(&mut self, rects: &[RectI]) -> Result<(), PlatformError> {
        self.inner.set_input_region(rects)
    }

    fn set_visible(&mut self, visible: bool) -> Result<(), PlatformError> {
        self.inner.set_visible(visible)
    }

    fn request_frame(&mut self) -> Option<FrameToken> {
        let token = self.inner.request_frame();
        // The token belongs to the next frame to be presented, which is the next commit;
        // holding it until then is what lets a record say whether the backend asked for a
        // callback at all.
        self.pending_frame = token;
        token
    }

    fn poll_events(&mut self, out: &mut Vec<SurfaceEvent>) -> Result<(), PlatformError> {
        self.inner.poll_events(out)
    }

    fn geometry(&self) -> (u32, u32, f32) {
        self.inner.geometry()
    }

    fn backend_id(&self) -> &'static str {
        self.inner.backend_id()
    }
}
