//! The two-slot `wl_shm` pool's bookkeeping: which slot is free, and how large the pool is.
//!
//! Responsibility: decide which slot `acquire_buffer` hands out, remember which slots the
//! compositor still holds, and keep the pool's size a function of the window size alone.
//! Boundaries: this module owns no file descriptor, no mapping and no protocol object. The
//! connection binding owns the `wl_shm` pool and the pixel memory; it asks here which slot to
//! draw into, and reports here when the compositor releases one.
//!
//! # Why a buffer is not reused early
//!
//! A buffer that has been attached to a surface may not be written again until the compositor
//! has released it: the frame on screen is read out of that memory, and writing it mid-scan
//! tears the picture. There are two ways out -- wait for the release, or skip the frame -- and
//! waiting would block the UI thread, so the frame is skipped and
//! [`PlatformError::NoFreeBuffer`] tells the caller to try again later. Correctness is worth
//! more than the frame here, and the contract's `NoFreeBuffer` exists for exactly this.
//!
//! # Why a resize is deferred rather than applied
//!
//! A resize means new buffers, and the old ones are still mapped into the compositor. The new
//! size is therefore recorded and adopted at the first moment both slots are back, so no pool
//! is ever torn down while the compositor is still reading it. Until then the surface keeps
//! the size it had, which is what the caller sees through the backend's `geometry`.
//!
//! # Why the size never grows
//!
//! The pool is two buffers of `stride * height` bytes and nothing else -- 2.7 MB for a
//! 1200x280 window at a scale of 2, which is what the memory budget allows. It is reallocated
//! only when the window's size changes, never grown in place, and never enlarged by a
//! show/hide cycle.

use ime_types::PlatformError;

use crate::platform::{BYTES_PER_PIXEL, clamp_dimension};

/// Number of draw buffers in the pool.
///
/// Two, fixed: one in flight with the compositor and one being drawn into. A third would buy
/// nothing, because the UI thread skips a frame it cannot draw rather than queueing one.
pub(crate) const SLOT_COUNT: usize = 2;

/// Consecutive frames skipped for want of a buffer that count as one reportable episode.
pub(crate) const STARVATION_FRAMES: u32 = 3;

/// Recorded when frames are skipped because the compositor still holds both buffers.
///
/// The backend counts episodes; the UI thread owns the diagnostics sink and records this code
/// once per episode.
pub const STARVATION_CODE: &str = "ui/buffer/starvation";

/// The two-slot pool's bookkeeping.
#[derive(Clone, Debug)]
pub(crate) struct BufferPool {
    /// Whether the compositor still holds each slot.
    busy: [bool; SLOT_COUNT],
    /// The slot handed out most recently, so the two buffers are used in turn.
    last: usize,
    width_px: u32,
    height_px: u32,
    /// A size the surface asked for that could not be adopted yet.
    pending: Option<(u32, u32)>,
    /// Frames skipped in a row, reset by the next successful acquire.
    starved_run: u32,
    /// Runs that reached [`STARVATION_FRAMES`], counted once each.
    episodes: u32,
    /// The longest run seen, for diagnostics.
    worst_run: u32,
}

impl BufferPool {
    /// Builds a pool for a window of the given physical size.
    pub(crate) fn new(width_px: u32, height_px: u32) -> Self {
        Self {
            busy: [false; SLOT_COUNT],
            last: SLOT_COUNT - 1,
            width_px: clamp_dimension(width_px),
            height_px: clamp_dimension(height_px),
            pending: None,
            starved_run: 0,
            episodes: 0,
            worst_run: 0,
        }
    }

    /// The pool's width in physical pixels.
    pub(crate) fn width_px(&self) -> u32 {
        self.width_px
    }

    /// The pool's height in physical pixels.
    pub(crate) fn height_px(&self) -> u32 {
        self.height_px
    }

    /// Bytes per row of one buffer.
    pub(crate) fn stride(&self) -> usize {
        self.width_px as usize * BYTES_PER_PIXEL as usize
    }

    /// Bytes the `wl_shm` pool needs for both buffers.
    ///
    /// The size is fixed at construction and after a resize, never grown: this is the number
    /// the memory budget is stated in, and the number a leak would show up in.
    pub(crate) fn pool_bytes(&self) -> usize {
        self.stride() * self.height_px as usize * SLOT_COUNT
    }

    /// Whether every slot is back from the compositor.
    pub(crate) fn is_free(&self) -> bool {
        self.busy.iter().all(|busy| !busy)
    }

    /// Whether a requested size is still waiting for the slots to come back.
    pub(crate) fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Runs of consecutive starved frames that reached [`STARVATION_FRAMES`].
    pub(crate) fn episodes(&self) -> u32 {
        self.episodes
    }

    /// The longest run of consecutive starved frames seen.
    pub(crate) fn worst_run(&self) -> u32 {
        self.worst_run
    }

    /// Records the buffer size the surface needs.
    ///
    /// Recorded rather than applied: [`BufferPool::adopt_pending`] takes it at the first
    /// moment both slots are back, so a pool is never torn down while the compositor is still
    /// reading it. Until then the surface keeps the size it had.
    pub(crate) fn request_resize(&mut self, width_px: u32, height_px: u32) {
        let requested = (clamp_dimension(width_px), clamp_dimension(height_px));
        if requested == (self.width_px, self.height_px) {
            // A configure that asks for the size already in use is not a resize.
            self.pending = None;
            return;
        }
        self.pending = Some(requested);
    }

    /// Adopts a requested size if both slots are free; reports whether the size changed.
    ///
    /// The caller must recreate the pool's memory when this returns `true`.
    pub(crate) fn adopt_pending(&mut self) -> bool {
        if !self.is_free() {
            return false;
        }
        let Some((width_px, height_px)) = self.pending.take() else {
            return false;
        };
        self.width_px = width_px;
        self.height_px = height_px;
        true
    }

    /// Hands out a free slot and marks it in flight.
    ///
    /// A pending resize is deliberately *not* adopted here: adopting it means the caller has
    /// to recreate the pool's memory, and doing that inside an acquire would hand out a slot
    /// whose buffer is still the old size. [`BufferPool::adopt_pending`] is the caller's step,
    /// taken before the acquire.
    ///
    /// # Errors
    ///
    /// Returns [`PlatformError::NoFreeBuffer`] when the compositor still holds both slots; the
    /// caller skips the frame rather than waiting for one.
    pub(crate) fn acquire(&mut self) -> Result<usize, PlatformError> {
        if let Some(slot) = self.free_slot() {
            self.busy[slot] = true;
            self.last = slot;
            self.starved_run = 0;
            return Ok(slot);
        }
        self.starved_run = self.starved_run.saturating_add(1);
        self.worst_run = self.worst_run.max(self.starved_run);
        if self.starved_run == STARVATION_FRAMES {
            // Once per run: a run that lasts twenty frames is one episode, not eighteen.
            self.episodes = self.episodes.saturating_add(1);
        }
        Err(PlatformError::NoFreeBuffer)
    }

    /// Marks a slot free again, from `wl_buffer.release`.
    ///
    /// A release for a slot that is not busy is ignored: a compositor may send one for a
    /// buffer this backend has already recycled, and that is not an error.
    pub(crate) fn release(&mut self, slot: usize) {
        if let Some(busy) = self.busy.get_mut(slot) {
            *busy = false;
        }
    }

    /// Marks every slot free, for the one case where no release can be expected.
    ///
    /// When the surface a buffer was attached to is destroyed -- a tier that failed and is
    /// being replaced -- the compositor drops its reference and may never send a release for
    /// it. Waiting for one that cannot come would starve every later frame, and the buffer is
    /// no longer on screen, so nothing can be torn by drawing into it again.
    pub(crate) fn release_all(&mut self) {
        self.busy = [false; SLOT_COUNT];
    }

    /// The next free slot, alternating so both buffers are used in turn.
    fn free_slot(&self) -> Option<usize> {
        (1..=SLOT_COUNT)
            .map(|step| (self.last + step) % SLOT_COUNT)
            .find(|slot| self.busy.get(*slot).is_some_and(|busy| !busy))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_bytes_matches_the_memory_budget() {
        let pool = BufferPool::new(1200, 280);
        assert_eq!(pool.stride(), 4800);
        assert_eq!(pool.pool_bytes(), 2 * 4800 * 280);
        assert_eq!(pool.pool_bytes(), 2_688_000);
        assert_eq!(SLOT_COUNT, 2, "the budget is stated for two buffers");
    }

    #[test]
    fn test_acquire_alternates_between_the_two_slots() {
        let mut pool = BufferPool::new(100, 50);
        let first = pool.acquire().expect("a slot is free at the start");
        let second = pool.acquire().expect("the other slot is free");
        assert_ne!(first, second);
        pool.release(first);
        assert_eq!(
            pool.acquire().expect("the released slot is free again"),
            first
        );
    }

    #[test]
    fn test_acquire_without_a_release_reports_no_free_buffer() {
        // The contract's `NoFreeBuffer`: a buffer the compositor still holds is never reused.
        let mut pool = BufferPool::new(100, 50);
        let _first = pool.acquire().expect("slot one");
        let _second = pool.acquire().expect("slot two");
        assert_eq!(pool.acquire(), Err(PlatformError::NoFreeBuffer));
        assert_eq!(pool.acquire(), Err(PlatformError::NoFreeBuffer));
        assert_eq!(pool.episodes(), 0, "two starved frames are not an episode");
        assert_eq!(pool.acquire(), Err(PlatformError::NoFreeBuffer));
        assert_eq!(
            pool.episodes(),
            1,
            "the third starved frame is reported once"
        );
        assert_eq!(pool.worst_run(), 3);
        assert_eq!(pool.acquire(), Err(PlatformError::NoFreeBuffer));
        assert_eq!(
            pool.episodes(),
            1,
            "the run is reported once, not per frame"
        );
    }

    #[test]
    fn test_a_release_resets_the_starvation_run() {
        let mut pool = BufferPool::new(100, 50);
        let first = pool.acquire().expect("slot one");
        let _second = pool.acquire().expect("slot two");
        let _ = pool.acquire();
        let _ = pool.acquire();
        pool.release(first);
        let _ = pool.acquire().expect("the released slot is free again");
        assert_eq!(pool.episodes(), 0);
    }

    #[test]
    fn test_a_resize_waits_until_both_slots_are_back() {
        let mut pool = BufferPool::new(100, 50);
        let _first = pool.acquire().expect("slot one");
        pool.request_resize(200, 100);
        assert!(pool.has_pending());
        assert_eq!(
            pool.width_px(),
            100,
            "the old size is kept while a slot is out"
        );
        assert!(!pool.adopt_pending());
        pool.release(0);
        assert!(pool.adopt_pending());
        assert_eq!((pool.width_px(), pool.height_px()), (200, 100));
        assert_eq!(pool.stride(), 800);
        assert!(!pool.has_pending());
    }

    #[test]
    fn test_a_resize_is_adopted_only_when_the_caller_asks_for_it() {
        let mut pool = BufferPool::new(100, 50);
        pool.request_resize(200, 100);
        assert!(pool.has_pending(), "the pool does not resize on its own");
        assert_eq!(
            pool.width_px(),
            100,
            "a slot handed out before the adoption is still the old size"
        );
        assert!(pool.adopt_pending());
        assert_eq!((pool.width_px(), pool.height_px()), (200, 100));
        assert!(!pool.has_pending());
    }

    #[test]
    fn test_release_all_frees_both_slots() {
        let mut pool = BufferPool::new(10, 10);
        let _first = pool.acquire().expect("slot one");
        let _second = pool.acquire().expect("slot two");
        assert!(!pool.is_free());
        pool.release_all();
        assert!(pool.is_free());
        assert_eq!(pool.acquire().map(|slot| slot < SLOT_COUNT), Ok(true));
    }

    #[test]
    fn test_a_configure_for_the_current_size_is_not_a_resize() {
        let mut pool = BufferPool::new(100, 50);
        pool.request_resize(100, 50);
        assert!(
            !pool.has_pending(),
            "the size already in use is not a resize"
        );
        assert_eq!((pool.width_px(), pool.height_px()), (100, 50));
    }

    #[test]
    fn test_a_superseded_resize_is_replaced_rather_than_queued() {
        let mut pool = BufferPool::new(100, 50);
        let _held = pool.acquire().expect("slot one");
        pool.request_resize(200, 100);
        pool.request_resize(300, 150);
        assert_eq!(pool.width_px(), 100, "the slot is still out");
        pool.release(0);
        assert!(pool.adopt_pending());
        assert_eq!(
            (pool.width_px(), pool.height_px()),
            (300, 150),
            "the newest request wins"
        );
    }

    #[test]
    fn test_a_release_for_an_unknown_slot_is_ignored() {
        let mut pool = BufferPool::new(10, 10);
        pool.release(SLOT_COUNT);
        pool.release(usize::MAX);
        assert!(pool.is_free());
    }

    #[test]
    fn test_show_hide_cycles_do_not_grow_the_pool() {
        // DoD 7: a hundred show/hide cycles must leave the pool exactly the size it started.
        let mut pool = BufferPool::new(1200, 280);
        let baseline = pool.pool_bytes();
        for _ in 0..100 {
            let slot = pool.acquire().expect("the frame's slot");
            pool.release(slot);
        }
        assert_eq!(pool.pool_bytes(), baseline);
        assert!(!pool.has_pending());
        assert!(pool.is_free());
    }

    #[test]
    fn test_pool_clamps_a_size_a_compositor_could_not_have_meant() {
        let pool = BufferPool::new(0, u32::MAX);
        assert_eq!(pool.height_px(), crate::platform::MAX_DIMENSION);
        assert_eq!(pool.width_px(), 1);
    }
}
