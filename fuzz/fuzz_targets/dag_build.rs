#![no_main]

//! Fuzz target: segmentation of an arbitrary ASCII string must never panic and must
//! answer the same thing twice in a row.
//!
//! The decoder reuses one `SyllableDag` across keystrokes, so the target builds the
//! same input twice and compares the two answers: the reuse path is what the engine
//! actually takes, and stale state left behind by it would be invisible to a target
//! that only ever built fresh.
//!
//! Latency is deliberately **not** asserted here. A wall-clock bound in a fuzz target
//! measures the machine's load as much as the code's complexity, so it fails on a busy
//! host with an input that runs fine in isolation — which is exactly what happened. The
//! segmentation budget is a `criterion` benchmark instead, where the measurement is
//! taken deliberately rather than as a side effect of fuzzing.

use ime_core::segment::SyllableDag;
use libfuzzer_sys::fuzz_target;

/// Hard cap on the input the fuzzer may hand us.
const MAX_FUZZ_INPUT: usize = 4096;

fuzz_target!(|data: &[u8]| {
    // The engine only ever hands the decoder ASCII (the input buffer filters keys),
    // so non-ASCII bytes and oversized inputs are out of contract.
    if data.len() > MAX_FUZZ_INPUT || !data.is_ascii() {
        return;
    }
    let Ok(raw) = std::str::from_utf8(data) else {
        return;
    };

    let mut dag = SyllableDag::new();
    let first = dag.build(raw);
    let second = dag.build(raw);

    // Rebuilding the same input must give the same answer: the reuse path may not
    // leave stale state behind.
    assert_eq!(first, second);
    if second.is_ok() {
        assert!(dag.has_path());
        assert!(dag.path_count() >= 1);
    } else {
        assert!(!dag.has_path());
        assert_eq!(dag.path_count(), 0);
    }
});
