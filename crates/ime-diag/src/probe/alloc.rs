//! The allocator probe: the working set a scope holds while it runs.
//!
//! Responsibility: count what the process's allocator is asked for -- bytes live, the
//! high-water mark of those bytes, and how many calls produced them -- so that a
//! working set too short-lived for the resident set size to show can still be measured.
//! `VmRSS` is the kernel's own accounting, read when someone asks for it, and a 40 KiB
//! buffer that lives for the length of one keystroke never moves it. The allocator sees
//! every byte of that buffer, at the moment it is handed out.
//!
//! # Why the counters are not the allocator
//!
//! A `#[global_allocator]` can only be installed by a binary crate: choosing the
//! process's allocator is not a library's decision to make, and a library that
//! installed one would decide it for every program that linked it. This module
//! therefore holds the counting and nothing else. A binary that wants the numbers
//! installs a wrapper whose methods forward here:
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOCATOR: Counting = Counting;
//!
//! struct Counting;
//!
//! // SAFETY: every method forwards to `System` unchanged, and the counting between the
//! // call and the return touches nothing but atomics: it cannot allocate, cannot lock
//! // and cannot unwind, which is what the allocator contract requires of it.
//! unsafe impl GlobalAlloc for Counting {
//!     unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
//!         let pointer = System.alloc(layout);
//!         if !pointer.is_null() {
//!             ALLOC_PROBE.note_alloc(layout.size());
//!         }
//!         pointer
//!     }
//!     // ... `dealloc`, `realloc` and `alloc_zeroed` forward the same way ...
//! }
//! ```
//!
//! # What the counting costs
//!
//! One relaxed load decides whether anything is counted at all, so a process that never
//! arms the probe pays a branch per allocation. Armed, one call is two or three relaxed
//! read-modify-writes on a structure that is already in cache. There is no lock, and
//! that is not an optimisation: an allocator can be entered by a thread that already
//! holds a lock, so a lock taken inside it can deadlock against the allocator itself.
//! There is no allocation, no formatting and no syscall either.
//!
//! # The numbers are a scope's, not the process's
//!
//! [`AllocProbe::arm`] zeroes every counter and starts counting; [`AllocProbe::disarm`]
//! answers with what the scope measured and zeroes them again. What a caller reads
//! between the two describes the work that ran between them and nothing that ran
//! before. Arming and disarming are the caller's to pair: the probe cannot know when a
//! scope ends, and a guard that disarmed on drop would do the wrong thing the moment
//! two scopes overlapped on two threads.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};

/// One scope's allocation counters.
///
/// Every method takes `&self`, so one probe is shared by every thread without a lock:
/// the counters are atomics and a lost update is impossible because each operation is a
/// read-modify-write on a single word.
#[derive(Debug)]
pub struct AllocProbe {
    /// Bytes currently live, signed because a free can name an allocation the probe
    /// never saw: a scope is armed around the work it measures, while an object
    /// allocated before the arming can be freed inside it.
    live: AtomicIsize,
    /// The highest value `live` reached while armed.
    peak: AtomicIsize,
    /// How many allocations were counted.
    allocations: AtomicU64,
    /// How many frees were counted.
    deallocations: AtomicU64,
    /// How many resizes were counted.
    reallocations: AtomicU64,
    /// Whether anything is being counted.
    armed: AtomicBool,
}

impl AllocProbe {
    /// A probe that is counting nothing.
    pub const fn new() -> Self {
        Self {
            live: AtomicIsize::new(0),
            peak: AtomicIsize::new(0),
            allocations: AtomicU64::new(0),
            deallocations: AtomicU64::new(0),
            reallocations: AtomicU64::new(0),
            armed: AtomicBool::new(false),
        }
    }

    /// Zeroes every counter and starts counting.
    ///
    /// Zeroing rather than continuing is what makes the numbers a scope's: a caller
    /// arms before the work it wants measured and disarms after it, and what it reads
    /// describes that work alone. The arming flag is set last, so a call that lands
    /// while the counters are being zeroed is dropped rather than counted against a
    /// half-reset scope.
    pub fn arm(&self) {
        self.live.store(0, Ordering::Relaxed);
        self.peak.store(0, Ordering::Relaxed);
        self.allocations.store(0, Ordering::Relaxed);
        self.deallocations.store(0, Ordering::Relaxed);
        self.reallocations.store(0, Ordering::Relaxed);
        self.armed.store(true, Ordering::Relaxed);
    }

    /// Stops counting, answers with what the scope measured, and zeroes every counter.
    ///
    /// Returning the numbers rather than leaving them for a second call is what keeps
    /// the sequence right: the snapshot a caller wants is the one taken while the
    /// scope's numbers are still there, and a caller that had to remember to take it
    /// before disarming is a caller that can forget. A second `disarm` answers with
    /// zeroes, because the counters it would have read were zeroed by the first.
    pub fn disarm(&self) -> AllocSnapshot {
        let snapshot = self.snapshot();
        self.armed.store(false, Ordering::Relaxed);
        self.live.store(0, Ordering::Relaxed);
        self.peak.store(0, Ordering::Relaxed);
        self.allocations.store(0, Ordering::Relaxed);
        self.deallocations.store(0, Ordering::Relaxed);
        self.reallocations.store(0, Ordering::Relaxed);
        snapshot
    }

    /// Whether anything is being counted.
    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Relaxed)
    }

    /// Counts one allocation of `bytes`.
    ///
    /// Nothing is counted while the probe is disarmed, so the cost of an unarmed probe
    /// is the load below and a branch. A zero-length allocation is a real call and is
    /// counted as one; it adds nothing to the working set, which is what the byte
    /// counters are for.
    pub fn note_alloc(&self, bytes: usize) {
        if !self.armed.load(Ordering::Relaxed) {
            return;
        }
        let delta = clamp(bytes);
        let live = self
            .live
            .fetch_add(delta, Ordering::Relaxed)
            .saturating_add(delta);
        self.raise_peak(live);
        self.allocations.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts one free of `bytes`.
    ///
    /// The subtraction is signed rather than saturating at zero, because a scope that
    /// frees more than it allocated is a real shape: the objects freed inside it may
    /// have been allocated before it was armed. Clamping at zero would hide the
    /// difference between a scope that freed nothing and one whose live bytes are
    /// below its baseline.
    pub fn note_dealloc(&self, bytes: usize) {
        if !self.armed.load(Ordering::Relaxed) {
            return;
        }
        self.live.fetch_sub(clamp(bytes), Ordering::Relaxed);
        self.deallocations.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts one resize from `old_bytes` to `new_bytes`.
    ///
    /// A resize is counted on its own rather than as a free and an allocation: it is
    /// one call, and a report that counted it twice would overstate the number of
    /// allocations a scope performs, which is one of the two numbers the working-set
    /// measurement is stated in.
    pub fn note_realloc(&self, old_bytes: usize, new_bytes: usize) {
        if !self.armed.load(Ordering::Relaxed) {
            return;
        }
        let delta = clamp(new_bytes).saturating_sub(clamp(old_bytes));
        let live = self
            .live
            .fetch_add(delta, Ordering::Relaxed)
            .saturating_add(delta);
        self.raise_peak(live);
        self.reallocations.fetch_add(1, Ordering::Relaxed);
    }

    /// What the counters hold at this moment.
    pub fn snapshot(&self) -> AllocSnapshot {
        AllocSnapshot {
            live_bytes: self.live.load(Ordering::Relaxed),
            peak_bytes: self.peak.load(Ordering::Relaxed).max(0),
            allocations: self.allocations.load(Ordering::Relaxed),
            deallocations: self.deallocations.load(Ordering::Relaxed),
            reallocations: self.reallocations.load(Ordering::Relaxed),
            armed: self.armed.load(Ordering::Relaxed),
        }
    }

    /// Raises the high-water mark to `live`, never lowering it.
    ///
    /// `fetch_max` is a single read-modify-write, so two threads raising the mark at
    /// the same time cannot lose one another's update the way a load-then-store could:
    /// the peak a report reads is the peak that happened, not the highest one some
    /// reader happened to observe.
    fn raise_peak(&self, live: isize) {
        self.peak.fetch_max(live, Ordering::Relaxed);
    }
}

impl Default for AllocProbe {
    fn default() -> Self {
        Self::new()
    }
}

/// The process-wide probe, for a `#[global_allocator]` wrapper to forward to.
///
/// One instance rather than one per caller because the allocator is one: the wrapper
/// sees every allocation the process makes, and there is nowhere else to put the
/// counters. A measurement run that wants the numbers arms this and reads them back
/// with [`AllocProbe::disarm`].
pub static ALLOC_PROBE: AllocProbe = AllocProbe::new();

/// One scope's allocation counters, as a measurement reads them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AllocSnapshot {
    /// Bytes live when the snapshot was taken.
    ///
    /// Negative when the scope freed more than it allocated, which happens when the
    /// objects it freed were allocated before it was armed.
    pub live_bytes: isize,
    /// The highest `live_bytes` reached while armed, never below zero.
    pub peak_bytes: isize,
    /// Allocations counted.
    pub allocations: u64,
    /// Frees counted.
    pub deallocations: u64,
    /// Resizes counted.
    pub reallocations: u64,
    /// Whether the probe was armed when the snapshot was taken.
    pub armed: bool,
}

impl AllocSnapshot {
    /// The high-water mark in kibibytes, rounded up.
    ///
    /// Rounded up because the number is judged against a ceiling: an estimate that
    /// rounds the peak down would pass a scope that in fact went over.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn peak_kib(&self) -> u64 {
        let peak = u64::try_from(self.peak_bytes.max(0)).unwrap_or(u64::MAX);
        peak.div_ceil(1024)
    }

    /// Whether the scope allocated nothing at all.
    ///
    /// The question a working-set measurement has to answer before it reports a peak:
    /// an unarmed probe and a scope that never reached the allocator both report zero,
    /// and only one of them is a measurement.
    pub fn is_empty(&self) -> bool {
        self.allocations == 0 && self.reallocations == 0
    }
}

/// A byte count as the signed counter holds it.
///
/// Saturating rather than wrapping: the conversion has to be total because the path it
/// runs on may not panic, and a layout larger than `isize::MAX` bytes is not something
/// an allocator can produce.
fn clamp(bytes: usize) -> isize {
    isize::try_from(bytes).unwrap_or(isize::MAX)
}
