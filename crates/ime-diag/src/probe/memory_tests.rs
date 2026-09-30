//! Unit tests for the allocator probe, the memory reader and the memory section.
//!
//! Responsibility: pin what the design fixes about the two memory instruments -- that an
//! unarmed allocator probe counts nothing and costs a branch, that a scope's numbers are
//! the scope's and not the process's, that the high-water mark never falls, that a
//! reading which failed is not a reading of zero, and that the section's text survives a
//! round trip through the file.
//!
//! Boundaries: nothing here arms the process's global allocator, which no crate of this
//! workspace may install -- `unsafe_code = "deny"` refuses the `GlobalAlloc` implementation
//! it would need -- so the counters are driven through the same entry points a wrapper
//! forwards to and the wrapper itself is a binary's decision. The tests that read `/proc`
//! are the ones that say so: they assert a real reading of this process, which is what
//! makes the reader's contract checkable at all.
//!
//! The one test that reads a source file rather than a value asserts the property the
//! acceptance criterion states as a review item: no mutex anywhere in the allocator
//! probe, because a lock taken inside an allocator can deadlock against the allocator.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;

use super::*;

/// A scratch directory under the workspace `target/`.
fn scratch_dir(tag: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/diag-scratch")
        .join(format!("memory-{tag}-{}-{unique}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// A `/proc/self/status` dump carrying `VmRSS`.
const STATUS: &str = "Name:\trspinyin\nVmPeak:\t  123456 kB\nVmSize:\t  111111 kB\n\
                      VmRSS:\t   45678 kB\nRssAnon:\t   40000 kB\nRssFile:\t    5000 kB\n";

/// A `smaps_rollup` dump carrying the two totals the dictionary budget is stated in.
const SMAPS_ROLLUP: &str = "Rss:               45678 kB\nPss:               30000 kB\n\
                            Pss_Anon:          20000 kB\nRss_Anon:          40000 kB\n\
                            Anonymous:         40000 kB\nAnonHugePages:         0 kB\n\
                            Private_Clean:      1000 kB\nPrivate_Dirty:      9000 kB\n";

#[test]
fn test_alloc_probe_unarmed_counts_nothing() {
    // A probe that was never armed is the shipped configuration: the wrapper forwards
    // every allocation of the process to it, and none of them may be counted, because
    // the cost of counting is the reason arming is a decision.
    let probe = AllocProbe::new();
    assert!(!probe.is_armed());
    probe.note_alloc(4096);
    probe.note_dealloc(1024);
    probe.note_realloc(64, 128);
    let snapshot = probe.snapshot();
    assert_eq!(snapshot, AllocSnapshot::default());
    assert!(snapshot.is_empty());
    assert!(!snapshot.armed);
}

#[test]
fn test_alloc_probe_armed_counts_bytes_and_calls() {
    let probe = AllocProbe::new();
    probe.arm();
    assert!(probe.is_armed());
    probe.note_alloc(1_000);
    probe.note_alloc(500);
    probe.note_dealloc(200);
    let snapshot = probe.snapshot();
    assert_eq!(snapshot.live_bytes, 1_300);
    assert_eq!(snapshot.peak_bytes, 1_500);
    assert_eq!(snapshot.allocations, 2);
    assert_eq!(snapshot.deallocations, 1);
    assert_eq!(snapshot.reallocations, 0);
    assert!(snapshot.armed);
    assert!(!snapshot.is_empty());
}

#[test]
fn test_alloc_probe_peak_never_lowers() {
    // The peak is the working set's high-water mark, which is the number the memory
    // budget is about: a scope that allocated 40 KiB and freed it before returning has
    // still held 40 KiB, and a peak that tracked the live count instead would report
    // the zero it ended at.
    let probe = AllocProbe::new();
    probe.arm();
    probe.note_alloc(40_960);
    probe.note_dealloc(40_960);
    probe.note_alloc(16);
    let snapshot = probe.snapshot();
    assert_eq!(snapshot.live_bytes, 16);
    assert_eq!(snapshot.peak_bytes, 40_960);
}

#[test]
fn test_alloc_probe_disarm_answers_with_the_scope_and_zeroes() {
    let probe = AllocProbe::new();
    probe.arm();
    probe.note_alloc(8_192);
    let measured = probe.disarm();
    assert_eq!(measured.peak_bytes, 8_192);
    assert_eq!(measured.allocations, 1);
    assert!(!probe.is_armed());
    // Everything the scope measured is gone: a reader that arrives after the disarm
    // cannot mistake the last scope's numbers for this one's, and a second disarm
    // answers with zeroes rather than repeating the first.
    assert_eq!(probe.snapshot(), AllocSnapshot::default());
    assert_eq!(probe.disarm(), AllocSnapshot::default());
}

#[test]
fn test_alloc_probe_free_below_the_baseline_reads_negative() {
    // A scope is armed around the work it measures, while an object allocated before
    // the arming can be freed inside it. Clamping that at zero would hide the
    // difference between a scope that freed nothing and one whose live bytes are below
    // where they started.
    let probe = AllocProbe::new();
    probe.arm();
    probe.note_dealloc(4_096);
    let snapshot = probe.snapshot();
    assert_eq!(snapshot.live_bytes, -4_096);
    assert_eq!(snapshot.peak_bytes, 0);
    assert_eq!(snapshot.deallocations, 1);
    // A free is a call, not an allocation, so the scope is not a measurement of one.
    assert!(snapshot.is_empty());
}

#[test]
fn test_alloc_probe_realloc_moves_live_and_raises_the_peak() {
    let probe = AllocProbe::new();
    probe.arm();
    probe.note_alloc(1_024);
    probe.note_realloc(1_024, 4_096);
    probe.note_realloc(4_096, 2_048);
    let snapshot = probe.snapshot();
    assert_eq!(snapshot.live_bytes, 2_048);
    assert_eq!(snapshot.peak_bytes, 4_096);
    assert_eq!(snapshot.allocations, 1, "a resize is one call, not two");
    assert_eq!(snapshot.deallocations, 0);
    assert_eq!(snapshot.reallocations, 2);
    assert!(
        !snapshot.is_empty(),
        "a scope that only resized measured something"
    );
}

#[test]
fn test_alloc_probe_peak_kib_rounds_up() {
    // The number is judged against a ceiling, so it must not round the peak down: a
    // scope of 1,025 bytes is a scope of two kibibytes against a budget stated in them.
    let probe = AllocProbe::new();
    probe.arm();
    probe.note_alloc(1_025);
    assert_eq!(probe.snapshot().peak_kib(), 2);
    probe.disarm();
    probe.arm();
    probe.note_alloc(1_024);
    assert_eq!(probe.snapshot().peak_kib(), 1);
    probe.disarm();
    assert_eq!(probe.snapshot().peak_kib(), 0);
}

#[test]
fn test_alloc_probe_arm_after_a_scope_starts_the_next_one_empty() {
    let probe = AllocProbe::new();
    probe.arm();
    probe.note_alloc(100_000);
    probe.disarm();
    probe.arm();
    probe.note_alloc(64);
    let snapshot = probe.snapshot();
    assert_eq!(
        snapshot.allocations, 1,
        "the previous scope is not in this one"
    );
    assert_eq!(snapshot.peak_bytes, 64);
    assert_eq!(snapshot.live_bytes, 64);
}

#[test]
fn test_alloc_probe_source_holds_no_mutex() {
    // The acceptance criterion states this as a review item, so it is asserted here:
    // an allocator can be entered by a thread that already holds a lock, so a lock
    // taken inside it can deadlock against the allocator itself. The counting is
    // atomics and nothing else.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/probe/alloc.rs");
    let text = fs::read_to_string(&path).expect("the allocator probe is readable");
    for lock in ["Mutex", "RwLock", "OnceLock"] {
        assert!(
            !text.contains(lock),
            "{} must not take a lock: `{lock}`",
            path.display()
        );
    }
}

#[test]
fn test_memory_reading_parse_status_reads_the_resident_set() {
    let rss_kib = MemoryReading::parse_status(STATUS).expect("the dump carries VmRSS");
    assert_eq!(rss_kib, 45_678);
    // The neighbouring fields are not the one asked for, which is what keeps `RssAnon`
    // from being read as the resident set.
    assert_ne!(rss_kib, 40_000);
}

#[test]
fn test_memory_reading_parse_status_refuses_a_dump_without_the_line() {
    let error = MemoryReading::parse_status("Name:\trspinyin\n").expect_err("no VmRSS");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("VmRSS"), "{error}");
}

#[test]
fn test_memory_reading_parse_status_refuses_a_value_that_is_not_a_number() {
    let error = MemoryReading::parse_status("VmRSS:\t  twelve kB\n").expect_err("not a number");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("twelve"), "{error}");
}

#[test]
fn test_memory_reading_parse_smaps_rollup_reads_both_totals() {
    let (anonymous_kib, private_dirty_kib) =
        MemoryReading::parse_smaps_rollup(SMAPS_ROLLUP).expect("both totals are there");
    assert_eq!(anonymous_kib, 40_000);
    assert_eq!(private_dirty_kib, 9_000);
}

#[test]
fn test_memory_reading_parse_smaps_rollup_is_not_confused_by_similar_names() {
    // `Rss_Anon`, `Pss_Anon`, `AnonHugePages` and `Private_Clean` all sit next to the
    // two fields the budget is stated in; a scan that matched a prefix rather than the
    // whole name would read one of them.
    let dump = "Rss_Anon:          11111 kB\nPss_Anon:          22222 kB\n\
                AnonHugePages:     33333 kB\nPrivate_Clean:     44444 kB\n\
                Anonymous:             5 kB\nPrivate_Dirty:         6 kB\n";
    let (anonymous_kib, private_dirty_kib) =
        MemoryReading::parse_smaps_rollup(dump).expect("both totals are there");
    assert_eq!(anonymous_kib, 5);
    assert_eq!(private_dirty_kib, 6);
}

#[test]
fn test_memory_reading_parse_smaps_rollup_refuses_a_missing_total() {
    let error = MemoryReading::parse_smaps_rollup("Anonymous: 5 kB\n")
        .expect_err("Private_Dirty is missing");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("Private_Dirty"), "{error}");
}

#[test]
fn test_memory_reading_dirty_kib_is_the_sum_the_budget_is_stated_in() {
    let reading = MemoryReading {
        rss_kib: 45_678,
        anonymous_kib: 40_000,
        private_dirty_kib: 9_000,
    };
    // The budget's own measurement is the pair summed, not a disjoint partition: an
    // anonymous private page is in both totals, so the sum is an upper bound. It is the
    // number the threshold names, which is why it is the number a report compares.
    assert_eq!(reading.dirty_kib(), 49_000);
}

#[cfg(target_os = "linux")]
#[test]
fn test_memory_reading_now_reads_this_process() {
    let reading = MemoryReading::now().expect("this process can be read");
    assert!(reading.rss_kib > 0, "a running process is resident");
    assert!(reading.dirty_kib() > 0, "a Rust process has private pages");
    // A second reading is a second reading of the same process, not a copy of the
    // first: the reader goes back to the kernel every time.
    let again = MemoryReading::now().expect("this process can be read");
    assert!(again.rss_kib > 0);
}

#[test]
fn test_memory_snapshot_text_round_trips_through_the_parser() {
    let snapshot = MemorySnapshot {
        rss_kib: Some(45_678),
        dirty_kib: Some(49_000),
        baseline_rss_kib: Some(40_000),
        baseline_dirty_kib: Some(41_000),
        ui_baseline_rss_kib: Some(42_000),
        dictionary_baseline_dirty_kib: Some(43_000),
    };
    let text = snapshot.to_text();
    assert!(text.starts_with(MEMORY_PREFIX));
    assert_eq!(text.lines().count(), 6);
    assert_eq!(
        MemorySnapshot::parse(&text).expect("the writer's own text parses"),
        snapshot
    );
}

#[test]
fn test_memory_snapshot_to_text_omits_what_nothing_measured() {
    // An empty field is left out rather than written as a zero, which is what makes an
    // unmeasured growth survive the file: a reader sees an absent record, and a zero
    // would pass every ceiling there is.
    let snapshot = MemorySnapshot {
        rss_kib: Some(1_024),
        ..MemorySnapshot::default()
    };
    let text = snapshot.to_text();
    assert_eq!(text, "memory.rss_kib=1024\n");
    assert_eq!(MemorySnapshot::parse(&text).expect("it parses"), snapshot);
    assert_eq!(MemorySnapshot::default().to_text(), "");
}

#[test]
fn test_memory_snapshot_parse_skips_the_snapshots_own_records() {
    // The two sections share one file, so each reader is handed the whole of it.
    let text = format!(
        "{SNAPSHOT_HEADER}\nsampled_us=1000\nkeys=3\nmetric.decode.count=1\n\
         memory.rss_kib=2048\ncounter.probe.lost=1\nmemory.dirty_kib=1024\n"
    );
    let memory = MemorySnapshot::parse(&text).expect("the memory records parse");
    assert_eq!(memory.rss_kib, Some(2_048));
    assert_eq!(memory.dirty_kib, Some(1_024));
    // And the snapshot's own reader accepts the same text, skipping the other section.
    let snapshot = ProbeSnapshot::parse(&text).expect("the snapshot records parse");
    assert_eq!(snapshot.keys, 3);
    assert_eq!(snapshot.metrics.decode.count, 1);
    assert_eq!(snapshot.counter(Counter::ProbeLost), 1);
}

#[test]
fn test_memory_snapshot_parse_refuses_an_unknown_field() {
    let error = MemorySnapshot::parse("memory.vm_rss_kib=1\n").expect_err("not a field");
    assert!(error.to_string().contains("memory.vm_rss_kib"), "{error}");
    assert!(error.to_string().contains("memory.rss_kib"), "{error}");
}

#[test]
fn test_memory_snapshot_parse_refuses_a_repeated_field() {
    let error =
        MemorySnapshot::parse("memory.rss_kib=1\nmemory.rss_kib=2\n").expect_err("written twice");
    assert!(error.to_string().contains("written twice"), "{error}");
    assert!(error.to_string().contains("line 2"), "{error}");
}

#[test]
fn test_memory_snapshot_parse_refuses_a_line_that_is_not_a_record() {
    let error = MemorySnapshot::parse("memory.rss_kib\n").expect_err("no `=`");
    assert!(error.to_string().contains("key=value"), "{error}");
}

#[test]
fn test_memory_snapshot_parse_refuses_a_value_that_is_not_a_whole_number() {
    let error = MemorySnapshot::parse("memory.rss_kib=1.5\n").expect_err("not a whole number");
    assert!(error.to_string().contains("1.5"), "{error}");
    assert!(MemorySnapshot::parse("memory.rss_kib=-1\n").is_err());
}

#[test]
fn test_memory_snapshot_is_key_matches_the_prefix() {
    assert!(MemorySnapshot::is_key("memory.rss_kib"));
    assert!(MemorySnapshot::is_key("memory."));
    assert!(!MemorySnapshot::is_key("metric.decode.count"));
    assert!(!MemorySnapshot::is_key("counter.probe.lost"));
    assert!(!MemorySnapshot::is_key("memory"));
}

#[test]
fn test_memory_snapshot_growth_is_missing_without_a_baseline() {
    // Every growth a budget is stated for needs two readings. One of them missing is a
    // budget nobody measured, which is not the same as a growth of zero.
    let snapshot = MemorySnapshot {
        rss_kib: Some(10_240),
        dirty_kib: Some(2_048),
        ..MemorySnapshot::default()
    };
    assert_eq!(snapshot.plugin_growth_kib(), None);
    assert_eq!(snapshot.ui_growth_kib(), None);
    assert_eq!(snapshot.dictionary_growth_kib(), None);
    // And a baseline without a reading is the same case from the other side.
    let baseline_only = MemorySnapshot {
        baseline_rss_kib: Some(1_024),
        ..MemorySnapshot::default()
    };
    assert_eq!(baseline_only.plugin_growth_kib(), None);
}

#[test]
fn test_memory_snapshot_growth_is_the_difference_and_never_negative() {
    let snapshot = MemorySnapshot {
        rss_kib: Some(51_200),
        dirty_kib: Some(3_072),
        baseline_rss_kib: Some(40_960),
        baseline_dirty_kib: Some(1_024),
        ui_baseline_rss_kib: Some(50_176),
        dictionary_baseline_dirty_kib: Some(4_096),
    };
    assert_eq!(snapshot.plugin_growth_kib(), Some(10_240));
    assert_eq!(snapshot.ui_growth_kib(), Some(1_024));
    // The dictionary's growth is measured in the dirty totals, not in the resident set.
    assert_eq!(snapshot.dictionary_growth_kib(), Some(0));
    assert_ne!(
        snapshot.dictionary_growth_kib(),
        snapshot.plugin_growth_kib()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_probes_memory_carries_the_process_baseline_and_no_other() {
    let probes = Probes::new();
    let memory = probes.memory();
    assert!(memory.rss_kib.is_some(), "this process can be read");
    assert!(memory.baseline_rss_kib.is_some());
    assert!(memory.plugin_growth_kib().is_some());
    // The two scopes nobody has marked yet report no growth, which is what a report
    // states as an unmeasured budget rather than as a pass.
    assert_eq!(memory.ui_baseline_rss_kib, None);
    assert_eq!(memory.dictionary_baseline_dirty_kib, None);
    assert_eq!(memory.ui_growth_kib(), None);
    assert_eq!(memory.dictionary_growth_kib(), None);
}

#[cfg(target_os = "linux")]
#[test]
fn test_probes_marks_take_the_baselines_they_name() {
    let probes = Probes::new();
    probes.mark_ui_baseline();
    probes.mark_dictionary_baseline();
    let memory = probes.memory();
    assert!(memory.ui_baseline_rss_kib.is_some());
    assert!(memory.dictionary_baseline_dirty_kib.is_some());
    assert!(memory.ui_growth_kib().is_some());
    assert!(memory.dictionary_growth_kib().is_some());
    // A second mark keeps the first reading: a baseline is taken once, and moving it
    // would narrow the window a report is about.
    let first = memory.ui_baseline_rss_kib;
    probes.mark_ui_baseline();
    assert_eq!(probes.memory().ui_baseline_rss_kib, first);
}

#[cfg(target_os = "linux")]
#[test]
fn test_probes_write_snapshot_writes_both_sections() {
    let dir = scratch_dir("write-snapshot");
    fs::create_dir_all(&dir).expect("the scratch directory is creatable");
    let path = dir.join("probe.txt");
    let probes = Probes::new();
    probes.decode.record(Duration::from_micros(250));
    probes
        .write_snapshot(&path)
        .expect("the snapshot is writable");

    let text = fs::read_to_string(&path).expect("the file is readable");
    let snapshot = ProbeSnapshot::parse(&text).expect("the snapshot section parses");
    assert_eq!(snapshot.metrics.decode.count, 1);
    let memory = MemorySnapshot::parse(&text).expect("the memory section parses");
    assert!(memory.rss_kib.is_some(), "the reading is in the file");
    assert!(memory.baseline_rss_kib.is_some());
    // The file is private, like every other file the diagnostics layer writes.
    use std::os::unix::fs::PermissionsExt as _;
    let mode = fs::metadata(&path)
        .expect("the file exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_probes_write_snapshot_over_a_longer_file_leaves_no_tail() {
    // The writer truncates, so a snapshot written over a longer file does not end in the
    // older file's bytes -- which the parser would then refuse, having found no header
    // first.
    let dir = scratch_dir("truncate");
    fs::create_dir_all(&dir).expect("the scratch directory is creatable");
    let path = dir.join("probe.txt");
    fs::write(&path, "x".repeat(64 * 1024)).expect("the longer file is writable");
    let probes = Probes::new();
    probes.write_snapshot(&path).expect("the write succeeds");
    let text = fs::read_to_string(&path).expect("the file is readable");
    assert_eq!(
        text.matches(SNAPSHOT_HEADER).count(),
        1,
        "one header, and it is the first line"
    );
    assert!(text.starts_with(SNAPSHOT_HEADER), "the tail is gone");
    ProbeSnapshot::parse(&text).expect("the second write left a parseable file");
    let _ = fs::remove_dir_all(&dir);
}
