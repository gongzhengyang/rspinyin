//! A counting global allocator, for asserting what a hot path allocates.
//!
//! # Why this crate exists, and why it is the fourth allowed `unsafe` path
//!
//! The decoder's allocation budget is a contract (`BUDGET-MEM-04`, `TC-CORE-09`: a single
//! decode must not allocate), and a contract nothing can measure is a comment. Counting
//! allocations needs a `#[global_allocator]`, and installing one needs
//! `unsafe impl GlobalAlloc` — the only `unsafe` in the workspace that is neither an FFI
//! boundary nor the dictionary's `mmap`.
//!
//! So it lives alone, in a crate that **nothing that ships may depend on**. That is not a
//! convention: `scripts/check-unsafe.sh` fails the build if this crate appears anywhere
//! except under a `[dev-dependencies]` section, and fails it outright if either cdylib
//! names it. The allowance is therefore scoped to code that cannot reach a release
//! artifact, which is what makes it narrower than it looks.
//!
//! # How to use it
//!
//! A binary target installs it; a library cannot, and neither can a `#[test]` inside a
//! library crate. The pattern is a bench or an integration test:
//!
//! ```
//! use alloc_count::Counting;
//!
//! #[global_allocator]
//! static ALLOCATOR: Counting = Counting::new();
//!
//! let before = alloc_count::allocations();
//! let _ = String::from("work");
//! assert!(alloc_count::allocations() > before);
//! ```
//!
//! The counters are process-global and monotonic. Tests that use them must run alone
//! (`cargo nextest` gives each test its own process, which is exactly what this needs);
//! under `cargo test`, where tests share a process, a count read by one test is polluted
//! by every other one running beside it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Bytes handed out by [`Counting`] and not yet returned, as a running total.
///
/// A `usize` rather than an `isize` because the count is only ever read as "how many
/// allocations happened between these two points": a live-byte figure would need
/// subtraction and could go negative when a buffer allocated before the reading is freed
/// after it. What the budget asks is how many times the allocator was called.
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

/// Bytes requested across every allocation [`Counting`] has served.
static BYTES: AtomicUsize = AtomicUsize::new(0);

/// A pass-through allocator that counts what it is asked for.
///
/// It forwards every call to [`System`] unchanged: no pooling, no alignment change, no
/// pointer arithmetic. That is deliberate — an allocator that altered behaviour would make
/// the thing it measures different from the thing that ships.
#[derive(Debug, Default, Clone, Copy)]
pub struct Counting;

impl Counting {
    /// Creates the allocator.
    ///
    /// A `const fn` because `#[global_allocator]` is a static initialiser, so the value has
    /// to be constructible in a constant expression.
    ///
    /// # Panics
    ///
    /// Never.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

// SAFETY: every method forwards to `System` with the same layout and the same pointer it
// was given. The `GlobalAlloc` contract is therefore upheld by `System`; this type adds two
// relaxed atomic increments, which allocate nothing, return nothing and cannot fail. No
// method returns a pointer `System` did not return, and no layout is altered.
//
// The `allow` is item-scoped rather than crate-scoped on purpose: `#![allow(unsafe_code)]`
// would switch the workspace lint off for every future line of this crate, and
// `scripts/check-unsafe.sh` rejects crate-level allows for exactly that reason. Here it
// covers the four methods below and nothing else.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller of `alloc` upholds `GlobalAlloc::alloc`'s contract, which is
        // exactly the contract `System::alloc` requires.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from this allocator's `alloc` (which returned `System`'s
        // pointer) and `layout` is the one it was allocated with, which is what
        // `GlobalAlloc::dealloc` requires.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: as `alloc` above.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(new_size, Ordering::Relaxed);
        // SAFETY: `ptr` and `layout` are the pair `alloc` returned, and `new_size` is the
        // caller's requested size, which is what `GlobalAlloc::realloc` requires.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// How many times the allocator has been called since the process started.
///
/// Monotonic, and shared by every thread. Read it before and after the work under test and
/// compare the difference.
///
/// # Panics
///
/// Never.
#[must_use]
pub fn allocations() -> usize {
    ALLOCATIONS.load(Ordering::Relaxed)
}

/// How many bytes have been requested since the process started.
///
/// Counts every request, including the ones later returned, so it is a measure of churn
/// rather than of live memory. [`allocations`] is the figure the decoder's budget is
/// written against; this one exists for the cases where the size of the allocation is the
/// interesting part.
///
/// # Panics
///
/// Never.
#[must_use]
pub fn bytes() -> usize {
    BYTES.load(Ordering::Relaxed)
}

#[cfg(test)]
// The crate's own tests cannot install `Counting` as the process allocator -- a library
// target has no `#[global_allocator]` slot -- so they drive the methods directly. What that
// leaves untested is the wiring, and the wiring is a one-line static in a bench or
// integration test, which is where the real measurements live.
#[allow(unsafe_code)]
mod tests {
    use super::*;

    /// A layout the tests can allocate and free with.
    fn layout() -> Layout {
        Layout::from_size_align(64, 8).expect("64 bytes at align 8 is a valid layout")
    }

    #[test]
    fn test_alloc_counts_the_request_and_returns_usable_memory() {
        let allocator = Counting::new();
        let before_allocations = allocations();
        let before_bytes = bytes();

        // SAFETY: the layout is valid and non-zero, which is `alloc`'s contract; the
        // pointer is returned to `dealloc` below with the same layout.
        let pointer = unsafe { allocator.alloc(layout()) };
        assert!(!pointer.is_null(), "a 64-byte allocation must succeed");

        assert_eq!(allocations(), before_allocations + 1);
        assert_eq!(bytes(), before_bytes + 64);

        // SAFETY: `pointer` came from `alloc` above, with exactly this layout.
        unsafe { allocator.dealloc(pointer, layout()) };
    }

    #[test]
    fn test_alloc_zeroed_hands_back_zeroed_bytes_and_counts_once() {
        let allocator = Counting::new();
        let before = allocations();

        // SAFETY: as above.
        let pointer = unsafe { allocator.alloc_zeroed(layout()) };
        assert!(!pointer.is_null());
        assert_eq!(allocations(), before + 1);

        // SAFETY: `pointer` came from `alloc_zeroed` above with this layout. Reading 64
        // bytes from it is in bounds, and `System` guarantees they are zero.
        let seen = unsafe { std::slice::from_raw_parts(pointer, 64) };
        assert!(seen.iter().all(|byte| *byte == 0));

        // SAFETY: as above.
        unsafe { allocator.dealloc(pointer, layout()) };
    }

    #[test]
    fn test_realloc_counts_the_new_size_and_keeps_the_contents() {
        let allocator = Counting::new();
        // SAFETY: the layout is valid; the pointer is reallocated and finally freed with
        // the same layout the allocation used.
        let pointer = unsafe { allocator.alloc(layout()) };
        assert!(!pointer.is_null());
        // SAFETY: `pointer` is a live allocation of 64 bytes.
        unsafe { pointer.write_bytes(0xAB, 64) };

        let before = allocations();
        // SAFETY: `pointer` is live and was allocated with `layout()`; 128 is the new size.
        let grown = unsafe { allocator.realloc(pointer, layout(), 128) };
        assert!(!grown.is_null());
        assert_eq!(allocations(), before + 1);

        // SAFETY: `grown` is a live 128-byte allocation whose first 64 bytes were copied
        // from the original.
        let seen = unsafe { std::slice::from_raw_parts(grown, 64) };
        assert!(seen.iter().all(|byte| *byte == 0xAB));

        // SAFETY: `grown` came from `realloc` with a 128-byte layout.
        let grown_layout = Layout::from_size_align(128, 8).expect("a valid layout");
        unsafe { allocator.dealloc(grown, grown_layout) };
    }

    #[test]
    fn test_dealloc_does_not_move_the_allocation_count() {
        let allocator = Counting::new();
        // SAFETY: as in the first test.
        let pointer = unsafe { allocator.alloc(layout()) };
        let before = allocations();
        // SAFETY: `pointer` came from `alloc` with this layout.
        unsafe { allocator.dealloc(pointer, layout()) };
        assert_eq!(
            allocations(),
            before,
            "freeing is not allocating; a dealloc that moved the counter would make every \
             reading off by the number of drops in the window"
        );
    }

    #[test]
    fn test_counting_is_constructible_in_a_constant_expression() {
        // The static initialiser in a bench or integration test is the only place this
        // type is used, and it has to be a `const` expression.
        const INSTANCE: Counting = Counting::new();
        let _ = INSTANCE;
    }
}
