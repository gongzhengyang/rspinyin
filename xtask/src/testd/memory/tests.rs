//! Tests for [`super`].
//!
//! They cover the reading and the comparison rather than a live plugin: the counters are driven
//! from a fake `/proc` tree the test writes itself, so every path -- a counter that is there, a
//! rollup that is not, a file that is not shaped the way the kernel writes it, a window that
//! shrank -- is a case the suite can run on any machine. The two tests that need a real process
//! sample this one, and assert only what holds for every process: a resident set above zero and
//! rollup counters no larger than it.
//!
//! No test asserts an absolute resident set of its own process: the allocator's state decides
//! that number, and a ceiling a fixture restated would be the second copy of the document's own
//! table that the audit test at the end of this file exists to catch. Every comparison is driven
//! by a ceiling table the test injects; only the audit reads the repository's document, because
//! auditing the source against the document is the whole of what it does.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use super::*;

/// The process id the fake trees use.
const FAKE_PID: u32 = 4242;

/// A `status` file of a process holding 40960KiB, as the kernel writes one.
const STATUS: &str = "Name:\trspinyin-test\nVmRSS:\t  40960 kB\nVmHWM:\t  45056 kB\n";

/// A `smaps_rollup` file whose anonymous part is well inside the dictionary budget.
const ROLLUP: &str = "Rss:\t  40960 kB\nAnonymous:\t  20480 kB\nPrivate_Dirty:\t  16384 kB\n";

/// The same process after it grew by 2560KiB.
const GROWN: &str = "VmRSS:\t  43520 kB\n";

/// The same process after it gave memory back.
const SHRUNK: &str = "VmRSS:\t  39936 kB\n";

/// The ceilings a test injects, in mebibytes.
///
/// They are the test's own numbers rather than the document's: a fixture that restated the real
/// ceilings would be the second copy the audit test exists to catch, and a comparison driven by
/// a number the test chose is what shows the comparison reads its table rather than a constant.
const INJECTED: [(&str, f64); 4] = [
    (UI_RSS_KEY, 1.5),
    (PLUGIN_RSS_KEY, 3.5),
    (DICT_MMAP_KEY, 55.5),
    (SOAK_DRIFT_KEY, 7.5),
];

/// The injected ceilings, as the module reads them.
fn injected() -> MemoryBudgets {
    MemoryBudgets::from_thresholds(&INJECTED)
}

/// A scratch directory holding one process directory, removed when the test ends.
struct FakeProc {
    /// The directory this guard owns.
    path: PathBuf,
}

impl FakeProc {
    /// Creates `<temp>/rspinyin-memory-<tag>-<pid>`, with the fake process directory inside it.
    fn new(tag: &str) -> Self {
        let name = format!("rspinyin-memory-{tag}-{}", std::process::id());
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join(FAKE_PID.to_string())).expect("creating the fake process");
        Self { path }
    }

    /// Writes the fake process's `status` file.
    fn status(&self, text: &str) {
        self.write("status", text);
    }

    /// Writes the fake process's `smaps_rollup` file.
    fn rollup(&self, text: &str) {
        self.write("smaps_rollup", text);
    }

    /// Writes one file of the fake process directory.
    fn write(&self, name: &str, text: &str) {
        let path = self.path.join(FAKE_PID.to_string()).join(name);
        fs::write(&path, text)
            .unwrap_or_else(|error| panic!("writing {}: {error}", path.display()));
    }

    /// A monitor following the fake process.
    fn monitor(&self) -> MemoryMonitor {
        MemoryMonitor::under(self.path.clone(), FAKE_PID)
    }

    /// A monitor whose window holds a baseline at 40960KiB and a sample at 43520KiB.
    fn grown(&self) -> MemoryMonitor {
        self.status(STATUS);
        self.rollup(ROLLUP);
        let mut monitor = self.monitor();
        monitor
            .mark_baseline()
            .expect("the fake files are readable");
        self.status(GROWN);
        monitor.sample().expect("the fake files are readable");
        monitor
    }
}

impl Drop for FakeProc {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn test_counter_kb_reads_the_kibibyte_field_the_kernel_writes() {
    assert_eq!(counter_kb(STATUS, RSS_FIELD), Some(40960));
    assert_eq!(counter_kb(ROLLUP, ANONYMOUS_FIELD), Some(20480));
    assert_eq!(counter_kb(ROLLUP, PRIVATE_DIRTY_FIELD), Some(16384));
    assert_eq!(
        counter_kb("VmRSS:  1234 kB\n", RSS_FIELD),
        Some(1234),
        "a field written with spaces rather than tabs still reads"
    );
}

#[test]
fn test_counter_kb_refuses_a_line_that_states_no_kibibytes() {
    for text in [
        "",
        "VmHWM:\t  45056 kB\n",
        "VmRSS:\n",
        "VmRSS:\t  abc kB\n",
        "VmRSS:\t  40960\n",
        "VmRSS:\t  40960 MB\n",
        "VmRSS:\t  -1 kB\n",
        "VmRSS:\t  40960 kB extra\n",
    ] {
        assert_eq!(
            counter_kb(text, RSS_FIELD),
            None,
            "`{text}` states no resident set"
        );
    }
}

#[test]
fn test_sample_of_this_process_reads_the_resident_set() {
    let mut monitor = MemoryMonitor::of_current_process();
    let sample = monitor.sample().expect("this process is readable");
    assert!(
        sample.vm_rss_kb > 0,
        "a running process holds memory: {sample:?}"
    );
    if sample.basis.has_rollup() {
        assert!(sample.anonymous_kb <= sample.vm_rss_kb, "{sample:?}");
        assert!(sample.private_dirty_kb <= sample.vm_rss_kb, "{sample:?}");
    }
    assert_eq!(monitor.latest(), Some(&sample));
}

#[test]
fn test_sample_of_a_process_that_is_not_there_is_refused() {
    let mut monitor = MemoryMonitor::new(u32::MAX);
    let refusal = monitor.sample().expect_err("there is no such process");
    assert!(
        matches!(refusal, MemoryError::ProcUnreadable { .. }),
        "{refusal}"
    );
    assert!(refusal.to_string().contains("status"), "{refusal}");
    assert!(
        monitor.samples().is_empty(),
        "a refused reading is not a sample"
    );
}

#[test]
fn test_sample_without_smaps_rollup_degrades_to_vm_rss_and_marks_it() {
    let fake = FakeProc::new("degraded");
    fake.status(STATUS);
    let mut monitor = fake.monitor();
    let sample = monitor.sample().expect("the status file is readable");
    assert_eq!(sample.vm_rss_kb, 40960);
    assert_eq!(sample.basis, SampleBasis::RssOnly(ErrorKind::NotFound));
    assert_eq!(
        sample.anonymous_kb, 0,
        "an unread counter is not a measurement"
    );
    assert_eq!(
        sample.private_dirty_kb, 0,
        "an unread counter is not a measurement"
    );
    assert!(!sample.basis.has_rollup());
    let report = monitor.lines();
    assert!(
        report.iter().any(|line| line == "basis: rss-only"),
        "{report:?}"
    );
    assert!(
        report
            .iter()
            .any(|line| line.starts_with("basis_degraded:")),
        "a degraded sample is marked rather than silent: {report:?}"
    );
}

#[test]
fn test_sample_with_a_status_that_states_no_resident_set_is_refused() {
    let fake = FakeProc::new("no-rss");
    fake.status("Name:\trspinyin-test\nVmHWM:\t  45056 kB\n");
    let mut monitor = fake.monitor();
    let refusal = monitor
        .sample()
        .expect_err("the status file states no resident set");
    assert!(
        matches!(refusal, MemoryError::Malformed { field, .. } if field == RSS_FIELD),
        "a status file without a resident set is not a status file: {refusal}"
    );
    assert!(monitor.samples().is_empty());
}

#[test]
fn test_sample_with_a_rollup_that_states_no_counter_is_refused() {
    let fake = FakeProc::new("bad-rollup");
    fake.status(STATUS);
    fake.rollup("Rss:\t  40960 kB\nPss:\t  39936 kB\n");
    let mut monitor = fake.monitor();
    let refusal = monitor
        .sample()
        .expect_err("the rollup states no anonymous part");
    assert!(
        matches!(refusal, MemoryError::Malformed { field, .. } if field == ANONYMOUS_FIELD),
        "a rollup that is there but is not shaped as the kernel writes it is an error, \
         not a degradation: {refusal}"
    );
}

#[test]
fn test_sample_reads_the_rollup_counters_when_the_file_is_there() {
    let fake = FakeProc::new("rollup");
    fake.status(STATUS);
    fake.rollup(ROLLUP);
    let mut monitor = fake.monitor();
    let sample = monitor.sample().expect("both files are readable");
    assert_eq!(sample.basis, SampleBasis::Rollup);
    assert_eq!(sample.vm_rss_kb, 40960);
    assert_eq!(sample.anonymous_kb, 20480);
    assert_eq!(sample.private_dirty_kb, 16384);
    assert!(sample.basis.has_rollup());
    assert_eq!(SampleBasis::Rollup.label(), "smaps-rollup");
    assert_eq!(
        SampleBasis::RssOnly(ErrorKind::PermissionDenied).label(),
        "rss-only"
    );
}

#[test]
fn test_new_records_the_process_it_follows() {
    let fake = FakeProc::new("pid");
    assert_eq!(fake.monitor().pid(), FAKE_PID);
    assert_eq!(MemoryMonitor::new(FAKE_PID).pid(), FAKE_PID);
    assert_eq!(
        MemoryMonitor::of_current_process().pid(),
        std::process::id()
    );
}

#[test]
fn test_mark_baseline_records_the_sample_it_took() {
    let fake = FakeProc::new("baseline");
    fake.status(STATUS);
    let mut monitor = fake.monitor();
    assert!(
        monitor.baseline().is_none(),
        "no baseline before one is taken"
    );
    let sample = monitor
        .mark_baseline()
        .expect("the status file is readable");
    assert_eq!(monitor.baseline(), Some(&sample));
    assert_eq!(
        monitor.samples(),
        [sample],
        "the baseline is part of the window"
    );
    assert_eq!(monitor.delta_from_baseline_kb(), Some(0));
}

#[test]
fn test_delta_from_baseline_kb_is_none_without_a_baseline() {
    let fake = FakeProc::new("no-baseline");
    fake.status(STATUS);
    let mut monitor = fake.monitor();
    assert_eq!(
        monitor.delta_from_baseline_kb(),
        None,
        "an empty window has no delta"
    );
    monitor.sample().expect("the status file is readable");
    monitor.sample().expect("the status file is readable");
    assert_eq!(
        monitor.delta_from_baseline_kb(),
        None,
        "a delta from nothing is not zero, and zero is how a budget passes unmeasured"
    );
}

#[test]
fn test_delta_from_baseline_kb_is_the_net_change_since_the_baseline() {
    let fake = FakeProc::new("delta");
    fake.status(STATUS);
    let mut monitor = fake.monitor();
    monitor
        .mark_baseline()
        .expect("the status file is readable");
    fake.status(GROWN);
    monitor.sample().expect("the status file is readable");
    assert_eq!(
        monitor.delta_from_baseline_kb(),
        Some(2560),
        "the process grew"
    );
    fake.status(SHRUNK);
    monitor.sample().expect("the status file is readable");
    assert_eq!(
        monitor.delta_from_baseline_kb(),
        Some(-1024),
        "and then gave memory back, which is why the delta is signed"
    );
}

#[test]
fn test_drift_kb_needs_a_window_of_two_samples() {
    let fake = FakeProc::new("drift-window");
    fake.status(STATUS);
    let mut monitor = fake.monitor();
    assert_eq!(monitor.drift_kb(), None, "an empty window has no drift");
    monitor.sample().expect("the status file is readable");
    assert_eq!(monitor.drift_kb(), None, "one reading is not a drift");
    monitor.sample().expect("the status file is readable");
    assert_eq!(
        monitor.drift_kb(),
        Some(0),
        "a window that did not move has not drifted"
    );
}

#[test]
fn test_drift_kb_is_the_growth_the_window_reached() {
    let fake = FakeProc::new("drift");
    fake.status(STATUS);
    let mut monitor = fake.monitor();
    monitor
        .mark_baseline()
        .expect("the status file is readable");
    fake.status(GROWN);
    monitor.sample().expect("the status file is readable");
    fake.status("VmRSS:\t  41984 kB\n");
    monitor.sample().expect("the status file is readable");
    assert_eq!(
        monitor.drift_kb(),
        Some(2560),
        "the window reached 2560KiB above its first sample, not merely above its last"
    );
    assert_eq!(
        monitor.delta_from_baseline_kb(),
        Some(1024),
        "the last sample is only 1024KiB above it, which is the difference the two answer"
    );
}

#[test]
fn test_samples_and_latest_report_the_window_in_order() {
    let fake = FakeProc::new("window");
    fake.status(STATUS);
    let mut monitor = fake.monitor();
    assert!(monitor.samples().is_empty());
    assert!(monitor.latest().is_none());
    let first = monitor.sample().expect("the status file is readable");
    fake.status(GROWN);
    let second = monitor.sample().expect("the status file is readable");
    assert_eq!(monitor.samples(), [first, second]);
    assert_eq!(monitor.latest(), Some(&second));
}

#[test]
fn test_budgets_convert_mebibytes_to_kibibytes() {
    let budgets = injected();
    assert_eq!(budgets.ui_rss_kb, Some(1536));
    assert_eq!(budgets.plugin_rss_kb, Some(3584));
    assert_eq!(budgets.dict_mmap_kb, Some(56832));
    assert_eq!(budgets.soak_drift_kb, Some(7680));
    assert_eq!(
        MemoryBudgets::from_thresholds(&[(UI_RSS_KEY, 0.001)]).ui_rss_kb,
        Some(1),
        "a fraction of a mebibyte rounds to the kibibytes it holds"
    );
}

#[test]
fn test_mb_to_kb_refuses_a_number_that_cannot_be_a_ceiling() {
    assert_eq!(mb_to_kb(1.5), Some(1536));
    assert_eq!(mb_to_kb(0.0), Some(0), "a ceiling of nothing is a ceiling");
    assert_eq!(mb_to_kb(-0.5), None);
    assert_eq!(mb_to_kb(f64::NAN), None);
    assert_eq!(mb_to_kb(f64::INFINITY), None);
    assert_eq!(mb_to_kb(f64::NEG_INFINITY), None);
}

#[test]
fn test_threshold_kb_finds_the_key_it_was_asked_for() {
    let thresholds = [(UI_RSS_KEY, 1.5), ("memory_mb.other", 9.5)];
    assert_eq!(threshold_kb(&thresholds, UI_RSS_KEY), Some(1536));
    assert_eq!(threshold_kb(&thresholds, "memory_mb.other"), Some(9728));
    assert_eq!(
        threshold_kb(&thresholds, DICT_MMAP_KEY),
        None,
        "a key nobody stated"
    );
    assert_eq!(threshold_kb(&[], UI_RSS_KEY), None, "a table nobody filled");
}

#[test]
fn test_budgets_leave_a_ceiling_the_document_does_not_state_unstated() {
    let none = MemoryBudgets::from_thresholds(&[]);
    assert_eq!(
        none,
        MemoryBudgets::default(),
        "nothing stated, nothing budgeted"
    );
    assert_eq!(none.ui_rss_kb, None);
    for unusable in [f64::NAN, f64::INFINITY, -1.0] {
        let budgets = MemoryBudgets::from_thresholds(&[(UI_RSS_KEY, unusable)]);
        assert_eq!(budgets.ui_rss_kb, None, "{unusable} is not a ceiling");
    }
    let other = MemoryBudgets::from_thresholds(&[("memory_mb.other", 7.5)]);
    assert_eq!(
        other,
        MemoryBudgets::default(),
        "a key this module does not bind is ignored"
    );
}

#[test]
fn test_budget_keys_are_the_keys_the_document_carries() {
    let document = document_thresholds();
    for budget in MemoryBudget::ALL {
        assert!(
            document.iter().any(|(key, _)| *key == budget.key()),
            "{} is not a key of the budget document",
            budget.key()
        );
    }
    assert_eq!(
        MemoryBudget::DictMmapAnonymous.key(),
        MemoryBudget::DictMmapPrivateDirty.key(),
        "the two bases of the dictionary budget share one threshold"
    );
}

#[test]
fn test_measure_labels_are_distinct() {
    for (measure, label) in [
        (Measure::RssDelta, "rss-delta"),
        (Measure::RssDrift, "rss-drift"),
        (Measure::Anonymous, "anonymous"),
        (Measure::PrivateDirty, "private-dirty"),
    ] {
        assert_eq!(measure.label(), label);
    }
    let mut measures: Vec<Measure> = MemoryBudget::ALL
        .iter()
        .map(|budget| budget.measure())
        .collect();
    let bound = measures.len();
    measures.dedup();
    assert_eq!(measures.len(), 4, "the budgets read four different numbers");
    assert_eq!(
        bound,
        MemoryBudget::ALL.len(),
        "every budget is bound to one of them"
    );
}

#[test]
fn test_judge_accepts_a_measurement_inside_its_ceiling() {
    let fake = FakeProc::new("judge-pass");
    let monitor = fake.grown();
    let verdict = monitor
        .judge(&injected(), MemoryBudget::PluginRss)
        .expect("2560KiB is inside 3584KiB");
    assert_eq!(verdict.budget, MemoryBudget::PluginRss);
    assert_eq!(verdict.measured_kb, 2560);
    assert_eq!(verdict.limit_kb, 3584);
    assert!(
        verdict.describe().contains("memory_mb.plugin_rss"),
        "{}",
        verdict.describe()
    );
    assert!(
        verdict.describe().contains("rss-delta"),
        "{}",
        verdict.describe()
    );
}

#[test]
fn test_judge_accepts_a_rollup_counter_against_the_dictionary_budget() {
    let fake = FakeProc::new("judge-rollup");
    let monitor = fake.grown();
    let verdict = monitor
        .judge(&injected(), MemoryBudget::DictMmapAnonymous)
        .expect("20480KiB is inside 56832KiB");
    assert_eq!(
        verdict.measured_kb, 20480,
        "the anonymous part, not the resident set"
    );
    assert!(
        verdict.describe().contains("anonymous"),
        "{}",
        verdict.describe()
    );
}

#[test]
fn test_judge_refuses_a_measurement_past_its_ceiling() {
    let fake = FakeProc::new("judge-fail");
    let monitor = fake.grown();
    let refusal = monitor
        .judge(&injected(), MemoryBudget::UiRss)
        .expect_err("2560KiB is past the 1536KiB ceiling");
    assert!(
        matches!(
            refusal,
            MemoryError::OverBudget {
                measured_kb: 2560,
                limit_kb: 1536,
                ..
            }
        ),
        "{refusal}"
    );
    assert!(
        refusal.to_string().contains("memory_mb.ui_rss"),
        "{refusal}"
    );
    assert!(refusal.to_string().contains("rss-delta"), "{refusal}");
}

#[test]
fn test_judge_refuses_a_dict_budget_against_a_sample_without_rollup_counters() {
    let fake = FakeProc::new("judge-degraded");
    fake.status(STATUS);
    let mut monitor = fake.monitor();
    monitor
        .mark_baseline()
        .expect("the status file is readable");
    monitor.sample().expect("the status file is readable");
    for budget in [
        MemoryBudget::DictMmapAnonymous,
        MemoryBudget::DictMmapPrivateDirty,
    ] {
        let refusal = monitor
            .judge(&injected(), budget)
            .expect_err("the rollup counters are not measurements");
        assert!(
            matches!(refusal, MemoryError::BasisDegraded { .. }),
            "a zero nobody measured must not pass: {refusal}"
        );
        assert!(refusal.to_string().contains("smaps_rollup"), "{refusal}");
    }
}

#[test]
fn test_judge_refuses_a_growth_budget_without_a_baseline() {
    let fake = FakeProc::new("judge-no-baseline");
    fake.status(STATUS);
    let mut monitor = fake.monitor();
    monitor.sample().expect("the status file is readable");
    let refusal = monitor
        .judge(&injected(), MemoryBudget::UiRss)
        .expect_err("no baseline was taken");
    assert!(
        matches!(refusal, MemoryError::NoBaseline { .. }),
        "{refusal}"
    );
}

#[test]
fn test_judge_refuses_a_drift_budget_with_too_few_samples() {
    let fake = FakeProc::new("judge-drift");
    fake.status(STATUS);
    let mut monitor = fake.monitor();
    monitor.sample().expect("the status file is readable");
    let refusal = monitor
        .judge(&injected(), MemoryBudget::SoakDrift)
        .expect_err("one reading is not a drift");
    assert!(
        matches!(refusal, MemoryError::TooFewSamples { .. }),
        "{refusal}"
    );
}

#[test]
fn test_judge_refuses_before_any_sample_has_been_taken() {
    let fake = FakeProc::new("judge-empty");
    let monitor = fake.monitor();
    let refusal = monitor
        .judge(&injected(), MemoryBudget::UiRss)
        .expect_err("nothing has been sampled");
    assert!(matches!(refusal, MemoryError::NoSample { .. }), "{refusal}");
}

#[test]
fn test_judge_refuses_a_budget_the_document_does_not_state() {
    let fake = FakeProc::new("judge-unbudgeted");
    let monitor = fake.grown();
    let refusal = monitor
        .judge(&MemoryBudgets::default(), MemoryBudget::UiRss)
        .expect_err("nothing is budgeted");
    assert!(
        matches!(refusal, MemoryError::Unbudgeted { key } if key == UI_RSS_KEY),
        "{refusal}"
    );
}

#[test]
fn test_lines_report_the_window_and_name_every_counter() {
    let fake = FakeProc::new("lines");
    fake.status(STATUS);
    fake.rollup(ROLLUP);
    let mut monitor = fake.monitor();
    let empty = monitor.lines();
    assert!(empty.iter().any(|line| line == "samples: 0"), "{empty:?}");
    assert!(empty.iter().any(|line| line == "basis: none"), "{empty:?}");
    assert!(
        empty.iter().any(|line| line == "drift_kb: none"),
        "{empty:?}"
    );
    assert!(
        empty
            .iter()
            .any(|line| line == "delta_from_baseline_kb: none"),
        "{empty:?}"
    );
    monitor
        .mark_baseline()
        .expect("the fake files are readable");
    fake.status(GROWN);
    monitor.sample().expect("the fake files are readable");
    monitor.sample().expect("the fake files are readable");
    let lines = monitor.lines();
    assert!(lines.iter().any(|line| line == "pid: 4242"), "{lines:?}");
    assert!(lines.iter().any(|line| line == "samples: 3"), "{lines:?}");
    assert!(
        lines.iter().any(|line| line == "basis: smaps-rollup"),
        "{lines:?}"
    );
    let window = lines.iter().find(|line| line.starts_with("vm_rss_kb:"));
    assert_eq!(
        window.map(|line| line.as_str()),
        Some("vm_rss_kb: first 40960 latest 43520")
    );
    assert!(
        lines.iter().any(|line| line == "drift_kb: 2560"),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "delta_from_baseline_kb: 2560"),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.starts_with("basis_degraded")),
        "a sample whose counters are all measurements says nothing about a degradation: {lines:?}"
    );
}

/// The repository's own budget document, flattened the way a caller flattens it.
///
/// The audit below is the one test that reads the repository's copy of the document: the
/// document is the authority, and the point of the audit is to hold this module's source
/// against it.
fn document_thresholds() -> Vec<(&'static str, f64)> {
    let root = crate::budget::repo_root().expect("xtask lives under the repository root");
    let budgets = crate::budget::read_budgets(&root).expect("the budget document is valid");
    budgets
        .thresholds()
        .iter()
        .map(|threshold| (threshold.0, threshold.1))
        .collect()
}

/// Every number written in `text`, in the order it appears.
///
/// The scan reads the source as text rather than as tokens, so a ceiling hidden in a comment or
/// in a message is found too. A number starts where a digit is not preceded by a character that
/// could be part of an identifier, which is what keeps `u64` and `f64` from being read as
/// numbers, and it runs over digits with `.` and `_` inside it, so `1024.0` is one number and
/// not three.
fn numeric_literals(text: &str) -> Vec<f64> {
    let chars: Vec<char> = text.chars().collect();
    let mut found = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if !chars[index].is_ascii_digit() || continues_an_identifier(&chars, index) {
            index += 1;
            continue;
        }
        let start = index;
        while index < chars.len()
            && (chars[index].is_ascii_digit() || chars[index] == '.' || chars[index] == '_')
        {
            index += 1;
        }
        let run: String = chars[start..index]
            .iter()
            .filter(|ch| **ch != '_')
            .collect();
        if let Ok(value) = run.trim_end_matches('.').parse::<f64>() {
            found.push(value);
        }
    }
    found
}

/// Whether the character at `index` continues an identifier rather than starting a number.
fn continues_an_identifier(chars: &[char], index: usize) -> bool {
    match index.checked_sub(1).and_then(|before| chars.get(before)) {
        Some(before) => before.is_ascii_alphanumeric() || *before == '_',
        None => false,
    }
}

#[test]
fn test_memory_ceilings_are_read_from_the_document_rather_than_restated() {
    let document = document_thresholds();
    let ceilings: Vec<f64> = MemoryBudget::ALL
        .iter()
        .filter_map(|budget| document.iter().find(|(key, _)| *key == budget.key()))
        .map(|(_, value)| *value)
        .collect();
    assert_eq!(
        ceilings.len(),
        MemoryBudget::ALL.len(),
        "the document states every ceiling this module binds"
    );
    for source in [include_str!("../memory.rs"), include_str!("tests.rs")] {
        let written = numeric_literals(source);
        for ceiling in &ceilings {
            for value in [*ceiling, ceiling * KB_PER_MB] {
                assert!(
                    !written
                        .iter()
                        .any(|found| (found - value).abs() < f64::EPSILON),
                    "{value} is a ceiling of the budget document and must not be written \
                     down in this module's source"
                );
            }
        }
    }
}
