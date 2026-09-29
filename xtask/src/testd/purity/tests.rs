//! Tests for the measurement-purity guard.
//!
//! The verdict is a pure function of [`MachineFacts`], so almost every test here hands it a
//! machine that is not this one: an idle machine, a machine with three `cargo` processes on it,
//! a machine whose `/proc/loadavg` nobody could read. The reader is driven the same way, by a
//! scratch directory that stands in for the root of the file system, which is what makes the
//! fail-closed paths -- a missing `/proc`, an unreadable file, a document shaped differently
//! from the way the kernel writes one -- cases a suite can run on any host.
//!
//! Exactly one test reads the real machine, and it asserts only what holds on every machine:
//! that the probe answers, and that its two accessors agree about the answer. Whether this host
//! is quiet is the host's business, and a test that asserted it would fail on a busy machine
//! for a reason that has nothing to do with the guard.
//!
//! No test here starts a process, sleeps, or reads a clock: the timestamp a baseline records is
//! handed in, so the record is reproducible from the arguments.

use std::fs;
use std::path::{Path, PathBuf};

use super::baseline::{BASELINE_DIR, Baseline, BaselineComparison, BaselineError, baseline_path};
use super::*;

/// The processor model the fake machines report.
const MODEL: &str = "rspinyin test processor";

/// The governor a fake machine that is behaving runs under.
const GOVERNOR: &str = "performance";

/// The budget key the baseline tests freeze.
const KEY: &str = "bench.passthrough_classify_ns";

/// The time the baseline tests freeze at, in seconds since the Unix epoch.
const FROZEN_AT: u64 = 1_700_000_000;

/// A process id that cannot belong to a running process.
///
/// `/proc/sys/kernel/pid_max` is at most 4194304, so a row numbered above that can never be
/// this test's own process -- which matters because the counting rule excludes the probe's own
/// ancestry, and a fake row that happened to carry the real pid would change the count.
const STRANGER_PID: u32 = 5_000_001;

/// A scratch directory standing in for the root of the file system, removed when the test ends.
struct FakeRoot {
    /// The directory this guard owns.
    path: PathBuf,
}

impl FakeRoot {
    /// Creates `<temp>/rspinyin-purity-<tag>-<pid>` with nothing in it.
    fn new(tag: &str) -> Self {
        let name = format!("rspinyin-purity-{tag}-{}", std::process::id());
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("creating the scratch root");
        Self { path }
    }

    /// The root itself.
    fn path(&self) -> &Path {
        &self.path
    }

    /// Writes one file below the root, creating the directories above it.
    fn write(&self, relative: &str, text: &str) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("creating the directory a fake file lives in");
        }
        fs::write(&path, text)
            .unwrap_or_else(|error| panic!("writing {}: {error}", path.display()));
    }

    /// Writes one process table row below the root.
    fn process(&self, pid: u32, comm: &str, ppid: u32) {
        self.write(
            &format!("{PROC_DIR}/{pid}/{STATUS_FILE}"),
            &format!("Name:\t{comm}\nPPid:\t{ppid}\n"),
        );
    }

    /// Writes a `/proc/cpuinfo` document listing `count` processors of one model.
    fn cpuinfo(&self, count: u32, model: &str) {
        let mut text = String::new();
        for index in 0..count {
            text.push_str(&format!("processor\t: {index}\nmodel name\t: {model}\n"));
        }
        self.write(CPUINFO_FILE, &text);
    }

    /// Writes a `/proc/loadavg` document whose first average is `load`.
    fn loadavg(&self, load: &str) {
        self.write(LOADAVG_FILE, &format!("{load} 0.10 0.05 1/1000 4242\n"));
    }

    /// Writes a `scaling_governor` document naming `governor`.
    fn governor(&self, governor: &str) {
        self.write(GOVERNOR_FILE, &format!("{governor}\n"));
    }
}

impl Drop for FakeRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A machine with nothing else running on it.
fn quiet(cpu_count: u32) -> MachineFacts {
    MachineFacts {
        concurrent_builds: Some(0),
        load_1m: Some(0.10),
        cpu_count: Some(cpu_count),
        cpu_model: Some(MODEL.to_owned()),
        scaling_governor: Some(GOVERNOR.to_owned()),
    }
}

/// The verdict `facts` amounts to under the guard's own policy.
fn report(facts: MachineFacts) -> PurityReport {
    PurityReport::judge(facts, &PurityPolicy::DEFAULT)
}

/// The report of a machine that is quiet, which the baseline tests freeze from.
fn clean_report() -> PurityReport {
    report(quiet(8))
}

/// One row of the fake process table.
fn row(pid: u32, ppid: u32, comm: &str) -> ProcessRow {
    ProcessRow {
        pid,
        ppid,
        comm: comm.to_owned(),
    }
}

#[test]
fn test_load_1m_from_reads_the_first_of_the_three_averages() {
    assert_eq!(
        load_1m_from("0.42 0.35 0.30 1/1234 5678\n"),
        Some(0.42),
        "the one-minute average is the first field"
    );
    assert_eq!(load_1m_from("12.5 9.0 4.0 3/900 9999"), Some(12.5));
    assert_eq!(load_1m_from("0"), Some(0.0), "an idle machine reads zero");
}

#[test]
fn test_load_1m_from_refuses_a_first_field_that_is_not_a_load() {
    assert_eq!(load_1m_from(""), None);
    assert_eq!(load_1m_from("   \n"), None);
    assert_eq!(load_1m_from("busy 0.1 0.1 1/10 20"), None);
    assert_eq!(load_1m_from("1.0,0.5,0.2"), None, "a comma is not a field");
    assert_eq!(
        load_1m_from("-1.0 0.0 0.0 1/10 20"),
        None,
        "a negative load is not a reading this guard will compare with a ceiling"
    );
    assert_eq!(
        load_1m_from("NaN 0.0 0.0 1/10 20"),
        None,
        "a load that is not a number must not reach a comparison"
    );
    assert_eq!(load_1m_from("inf 0.0 0.0 1/10 20"), None);
}

#[test]
fn test_cpuinfo_from_counts_processors_and_names_the_model() {
    let text = concat!(
        "processor\t: 0\nmodel name\t: AMD Ryzen 9\n",
        "\nprocessor\t: 1\nmodel name\t: AMD Ryzen 9\n",
    );
    let cpu = cpuinfo_from(text);
    assert_eq!(cpu.count, Some(2));
    assert_eq!(cpu.model.as_deref(), Some("AMD Ryzen 9"));
}

#[test]
fn test_cpuinfo_from_reports_no_count_for_a_document_without_processors() {
    let cpu = cpuinfo_from("");
    assert_eq!(
        cpu.count, None,
        "no processor listed is not a count of zero"
    );
    assert_eq!(cpu.model, None);

    let cpu = cpuinfo_from("model name\t: a processor nobody counted\n");
    assert_eq!(cpu.count, None);
    assert_eq!(cpu.model.as_deref(), Some("a processor nobody counted"));

    let cpu = cpuinfo_from("processor\t: 0\nmodel name\t:\n");
    assert_eq!(cpu.count, Some(1));
    assert_eq!(cpu.model, None, "an empty model name is not a model");
}

#[test]
fn test_governor_from_ignores_an_empty_document() {
    assert_eq!(governor_from("performance\n").as_deref(), Some(GOVERNOR));
    assert_eq!(governor_from("  powersave  ").as_deref(), Some("powersave"));
    assert_eq!(governor_from("\n"), None);
    assert_eq!(governor_from(""), None);
}

#[test]
fn test_parse_status_reads_the_name_and_the_parent() {
    let text = "Name:\tcargo\nUmask:\t0022\nPPid:\t42\n";
    let parsed = parse_status(STRANGER_PID, text).expect("both fields are there");
    assert_eq!(parsed.comm, "cargo");
    assert_eq!(parsed.ppid, 42);
    assert_eq!(parsed.pid, STRANGER_PID);
}

#[test]
fn test_parse_status_drops_a_document_missing_a_field() {
    assert_eq!(parse_status(7, "Name:\tcargo\n"), None, "no parent");
    assert_eq!(parse_status(7, "PPid:\t1\n"), None, "no name");
    assert_eq!(parse_status(7, "Name:\t\nPPid:\t1\n"), None, "a blank name");
    assert_eq!(
        parse_status(7, "PPid:\tnine\n"),
        None,
        "a parent that is not a number"
    );
}

#[test]
fn test_count_builds_excludes_the_probes_own_ancestry() {
    // 902 is the probe itself, 901 the cargo that started it, 900 the runner above that:
    // all three are the harness, not a second agent, so none of them may be counted. The
    // probe has to appear in the table for the walk to reach its ancestry at all.
    let rows = [
        row(900, 1, "nextest"),
        row(901, 900, "cargo"),
        row(902, 901, "xtask"),
        row(STRANGER_PID, 1, "cargo"),
    ];
    assert_eq!(count_builds(&rows, 902), 1);
}

#[test]
fn test_count_builds_counts_nothing_when_only_the_harness_runs() {
    let rows = [
        row(900, 1, "nextest"),
        row(901, 900, "cargo"),
        row(902, 901, "rustc"),
    ];
    assert_eq!(count_builds(&rows, 902), 0);
    assert_eq!(count_builds(&[], 902), 0);
    assert_eq!(
        count_builds(&[row(STRANGER_PID, 1, "fcitx5")], 902),
        0,
        "a program that is not a build or test driver is not a concurrent build"
    );
}

#[test]
fn test_count_builds_counts_every_driver_a_parallel_wave_runs() {
    let rows = [
        row(STRANGER_PID, 1, "cargo"),
        row(STRANGER_PID + 1, 1, "cargo-nextest"),
        row(STRANGER_PID + 2, 1, "nextest"),
        row(STRANGER_PID + 3, 1, "rustc"),
    ];
    assert_eq!(count_builds(&rows, 902), 4);
}

#[test]
fn test_own_ancestry_ends_on_a_cycle_instead_of_looping() {
    // `/proc` cannot produce this, but a table that names one must not hang the probe.
    let rows = [row(1, 2, "a"), row(2, 1, "b")];
    let chain = own_ancestry(1, &rows);
    assert!(chain.starts_with(&[1]));
    assert!(
        chain.len() <= rows.len() + 1,
        "the walk is bounded: {chain:?}"
    );
}

#[test]
fn test_own_ancestry_stops_at_a_pid_the_table_does_not_hold() {
    let chain = own_ancestry(4242, &[]);
    assert_eq!(chain, vec![4242], "a pid nobody knows contributes itself");
}

#[test]
fn test_judge_quiet_machine_is_clean() {
    let report = report(quiet(8));
    assert!(report.is_clean());
    assert_eq!(report.verdict(), &PurityVerdict::Clean);
    assert_eq!(report.rejection_reason(), None);
    assert_eq!(report.facts().cpu_count, Some(8));
}

#[test]
fn test_judge_running_builds_are_dirty_and_the_reason_names_the_count() {
    let facts = MachineFacts {
        concurrent_builds: Some(3),
        ..quiet(8)
    };
    let report = report(facts);
    assert!(!report.is_clean());
    assert!(matches!(report.verdict(), PurityVerdict::Dirty(_)));
    let reason = report
        .rejection_reason()
        .expect("a dirty machine has a reason");
    assert!(reason.contains("3 build"), "the count is named: {reason}");
    assert!(
        reason.contains("cargo"),
        "the programs counted are named: {reason}"
    );
}

#[test]
fn test_judge_load_at_the_ceiling_is_dirty_and_just_below_is_clean() {
    // Eight processors at a quarter each put the ceiling at 2.00.
    let at_ceiling = MachineFacts {
        load_1m: Some(2.0),
        ..quiet(8)
    };
    assert!(
        !report(at_ceiling).is_clean(),
        "the criterion is strictly below"
    );

    let below = MachineFacts {
        load_1m: Some(1.99),
        ..quiet(8)
    };
    assert!(report(below).is_clean());

    let far_above = MachineFacts {
        load_1m: Some(31.5),
        ..quiet(8)
    };
    let reason = report(far_above)
        .rejection_reason()
        .expect("a loaded machine has a reason");
    assert!(reason.contains("31.50"), "the load is named: {reason}");
    assert!(reason.contains("2.00"), "the ceiling is named: {reason}");
}

#[test]
fn test_judge_unreadable_load_is_undeterminable_and_not_clean() {
    let facts = MachineFacts {
        load_1m: None,
        ..quiet(8)
    };
    let report = report(facts);
    assert!(!report.is_clean(), "an unreadable /proc fails closed");
    assert!(matches!(
        report.verdict(),
        PurityVerdict::Undeterminable(gaps) if gaps.contains(&PurityGap::LoadUnreadable)
    ));
    let reason = report
        .rejection_reason()
        .expect("a refused machine has a reason");
    assert!(reason.contains("load average"), "{reason}");
    assert!(
        reason.contains("fails"),
        "the reason says the guard fails closed: {reason}"
    );
}

#[test]
fn test_judge_unreadable_process_table_is_not_clean() {
    let facts = MachineFacts {
        concurrent_builds: None,
        ..quiet(8)
    };
    let report = report(facts);
    assert!(!report.is_clean());
    assert!(matches!(
        report.verdict(),
        PurityVerdict::Undeterminable(gaps) if gaps.contains(&PurityGap::ProcessTableUnreadable)
    ));
    assert!(
        report
            .rejection_reason()
            .expect("a refused machine has a reason")
            .contains("process table")
    );
}

#[test]
fn test_judge_unreadable_processor_count_is_not_clean() {
    let facts = MachineFacts {
        cpu_count: None,
        ..quiet(8)
    };
    let report = report(facts);
    assert!(!report.is_clean());
    assert!(matches!(
        report.verdict(),
        PurityVerdict::Undeterminable(gaps) if gaps.contains(&PurityGap::CpuCountUnreadable)
    ));
}

#[test]
fn test_judge_load_that_is_not_a_number_is_not_clean() {
    for load in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let facts = MachineFacts {
            load_1m: Some(load),
            ..quiet(8)
        };
        assert!(
            !report(facts).is_clean(),
            "a load of {load} must not be read as an idle machine"
        );
    }
}

#[test]
fn test_judge_ceiling_that_is_not_a_number_is_not_clean() {
    let policy = PurityPolicy {
        max_load_per_cpu: f64::NAN,
    };
    let report = PurityReport::judge(quiet(8), &policy);
    assert!(!report.is_clean());
    assert!(matches!(
        report.verdict(),
        PurityVerdict::Undeterminable(gaps) if gaps.contains(&PurityGap::CeilingUnusable)
    ));

    let policy = PurityPolicy {
        max_load_per_cpu: 0.0,
    };
    assert!(
        !PurityReport::judge(quiet(8), &policy).is_clean(),
        "a ceiling of zero refuses every machine that is doing anything"
    );
}

#[test]
fn test_judge_a_certain_reason_outweighs_an_unreadable_fact() {
    let facts = MachineFacts {
        concurrent_builds: Some(2),
        load_1m: None,
        ..quiet(8)
    };
    let report = report(facts);
    assert!(matches!(report.verdict(), PurityVerdict::Dirty(_)));
    assert!(
        report
            .rejection_reason()
            .expect("a dirty machine has a reason")
            .contains("2 build")
    );
}

#[test]
fn test_accept_returns_a_measurement_taken_on_a_clean_machine() {
    let accepted = clean_report()
        .accept(726.0)
        .expect("a clean machine accepts its measurement");
    assert_eq!(accepted, 726.0);
}

#[test]
fn test_accept_refuses_a_measurement_taken_on_a_dirty_machine() {
    let facts = MachineFacts {
        concurrent_builds: Some(5),
        ..quiet(8)
    };
    let report = report(facts);
    let refusal = report
        .accept(511.0)
        .expect_err("a busy machine's measurement is refused, not warned about");
    assert!(refusal.reason.contains("5 build"), "{refusal}");
    assert!(refusal.to_string().contains("refused"), "{refusal}");
    let expected = report
        .rejection_reason()
        .expect("the same machine is still dirty");
    assert_eq!(refusal.reason.as_str(), expected.as_str());
}

#[test]
fn test_accept_refuses_a_measurement_on_an_undeterminable_machine() {
    let facts = MachineFacts {
        load_1m: None,
        ..quiet(8)
    };
    let refusal = report(facts)
        .accept(511.0)
        .expect_err("a machine nobody could read is refused");
    assert!(
        refusal.reason.contains("could not be established"),
        "{refusal}"
    );
}

#[test]
fn test_accept_samples_hands_back_samples_that_agree_with_each_other() {
    let spread = clean_report()
        .accept_samples(700.0, 14.0, 0.05)
        .expect("samples two percent apart are believable");
    assert_eq!(spread.mean, 700.0);
    assert_eq!(spread.std_dev, 14.0);
    assert_eq!(spread.relative(), Some(0.02));
}

#[test]
fn test_accept_samples_refuses_samples_that_disagree_past_the_tolerance() {
    // The tolerance is inclusive, as `Baseline::compare`'s is: a run exactly at it is inside.
    assert!(
        clean_report().accept_samples(700.0, 35.0, 0.05).is_ok(),
        "five percent is the tolerance itself, not past it"
    );

    let refusal = clean_report()
        .accept_samples(700.0, 70.0, 0.05)
        .expect_err("samples ten percent apart describe the run, not the code");
    assert!(refusal.reason.contains("10.0%"), "{refusal}");
    assert!(refusal.reason.contains("5.0%"), "{refusal}");
    assert!(
        refusal.reason.contains("700"),
        "the mean is named: {refusal}"
    );

    // A tolerance of zero is a caller saying only a case that repeated itself exactly is
    // acceptable; it is a tolerance, not a malformed one.
    assert!(clean_report().accept_samples(700.0, 0.0, 0.0).is_ok());
    assert!(
        clean_report().accept_samples(700.0, 0.1, 0.0).is_err(),
        "any spread at all is past a tolerance of zero"
    );
}

#[test]
fn test_accept_samples_refuses_a_pair_that_is_not_a_mean_and_a_spread() {
    for (mean, std_dev) in [
        (0.0, 0.0),
        (-1.0, 0.5),
        (f64::NAN, 0.5),
        (f64::INFINITY, 0.5),
        (700.0, -1.0),
        (700.0, f64::NAN),
        (700.0, f64::INFINITY),
    ] {
        let refusal = clean_report()
            .accept_samples(mean, std_dev, 0.05)
            .expect_err("a pair that is not a mean and a spread is refused");
        assert!(
            refusal.reason.contains("not the mean and"),
            "mean {mean}, spread {std_dev}: {refusal}"
        );
    }
}

#[test]
fn test_accept_samples_refuses_a_tolerance_that_is_not_a_tolerance() {
    for allowed in [-0.05, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let refusal = clean_report()
            .accept_samples(700.0, 14.0, allowed)
            .expect_err("a tolerance that is not a number is refused, not read as a wide one");
        assert!(refusal.reason.contains("not a usable spread"), "{refusal}");
    }
}

#[test]
fn test_accept_samples_refuses_samples_taken_on_a_machine_that_is_not_clean() {
    let facts = MachineFacts {
        concurrent_builds: Some(5),
        ..quiet(8)
    };
    let refusal = report(facts)
        .accept_samples(700.0, 14.0, 0.05)
        .expect_err("a tight spread taken on a busy machine is still a busy machine");
    assert!(refusal.reason.contains("5 build"), "{refusal}");
}

#[test]
fn test_sample_spread_relative_refuses_a_mean_that_cannot_be_divided_by() {
    let agreed = SampleSpread {
        mean: 700.0,
        std_dev: 0.0,
    };
    assert_eq!(
        agreed.relative(),
        Some(0.0),
        "samples that agreed have no spread"
    );

    let scattered = SampleSpread {
        mean: 700.0,
        std_dev: 700.0,
    };
    assert_eq!(scattered.relative(), Some(1.0));

    for mean in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let spread = SampleSpread { mean, std_dev: 1.0 };
        assert_eq!(spread.relative(), None, "{mean} cannot be divided by");
    }
    for std_dev in [-1.0, f64::NAN, f64::INFINITY] {
        let spread = SampleSpread {
            mean: 700.0,
            std_dev,
        };
        assert_eq!(spread.relative(), None, "{std_dev} is not a spread");
    }
}

#[test]
fn test_read_from_a_root_with_no_proc_tree_leaves_every_fact_unknown() {
    let root = FakeRoot::new("empty-root");
    let facts = MachineFacts::read(root.path());
    assert_eq!(facts.concurrent_builds, None);
    assert_eq!(facts.load_1m, None);
    assert_eq!(facts.cpu_count, None);
    assert_eq!(facts.cpu_model, None);
    assert_eq!(facts.scaling_governor, None);

    let report = PurityReport::sample_below(root.path(), &PurityPolicy::DEFAULT);
    assert!(
        !report.is_clean(),
        "a root nobody can read is not a quiet machine"
    );
}

#[test]
fn test_read_from_a_fake_proc_tree_reads_every_fact() {
    let root = FakeRoot::new("populated-root");
    root.cpuinfo(4, MODEL);
    root.loadavg("0.75");
    root.governor(GOVERNOR);
    root.process(900, "nextest", 1);
    root.process(901, "cargo", 900);
    root.process(STRANGER_PID, "cargo", 1);
    root.process(STRANGER_PID + 1, "rustc", 1);

    let facts = MachineFacts::read(root.path());
    assert_eq!(facts.cpu_count, Some(4));
    assert_eq!(facts.cpu_model.as_deref(), Some(MODEL));
    assert_eq!(facts.load_1m, Some(0.75));
    assert_eq!(facts.scaling_governor.as_deref(), Some(GOVERNOR));
    // The reader excludes its own ancestry, and none of these fake rows is an ancestor of this
    // test process, so all four drivers are counted.
    assert_eq!(facts.concurrent_builds, Some(4));
}

#[test]
fn test_read_from_a_fake_proc_tree_that_is_busy_judges_it_dirty() {
    let root = FakeRoot::new("busy-root");
    root.cpuinfo(2, MODEL);
    root.loadavg("0.10");
    root.governor(GOVERNOR);
    root.process(STRANGER_PID, "cargo", 1);

    let report = PurityReport::sample_below(root.path(), &PurityPolicy::DEFAULT);
    assert!(!report.is_clean());
    assert!(
        report
            .rejection_reason()
            .expect("one cargo process is a reason")
            .contains("1 build")
    );
}

#[test]
fn test_read_from_a_fake_proc_tree_that_is_quiet_judges_it_clean() {
    let root = FakeRoot::new("quiet-root");
    root.cpuinfo(4, MODEL);
    root.loadavg("0.25");
    root.governor(GOVERNOR);
    // A session, not a build: the only process on this machine is not one the guard counts.
    root.process(STRANGER_PID, "fcitx5", 1);

    let report = PurityReport::sample_below(root.path(), &PurityPolicy::DEFAULT);
    assert!(report.is_clean(), "{:?}", report.rejection_reason());
    assert_eq!(report.facts().load_1m, Some(0.25));
    assert_eq!(report.facts().concurrent_builds, Some(0));
}

#[test]
fn test_sample_on_this_machine_answers_without_panicking() {
    let report = PurityReport::sample();
    // Whether this host is quiet is the host's business. What is asserted is the invariant
    // every caller relies on: the two accessors agree, and a clean verdict means the readings
    // that justify it were actually taken.
    assert_eq!(report.is_clean(), report.rejection_reason().is_none());
    if report.is_clean() {
        assert_eq!(report.facts().concurrent_builds, Some(0));
        assert!(report.facts().load_1m.is_some_and(|load| load.is_finite()));
        assert!(report.facts().cpu_count.is_some_and(|count| count > 0));
    }
}

#[test]
fn test_frozen_baseline_carries_the_machine_the_governor_and_the_time() {
    let baseline = Baseline::frozen(KEY, 511.0, &clean_report(), FROZEN_AT).expect("a clean run");
    assert_eq!(baseline.key, KEY);
    assert_eq!(baseline.value, 511.0);
    assert_eq!(baseline.cpu_model.as_deref(), Some(MODEL));
    assert_eq!(baseline.scaling_governor.as_deref(), Some(GOVERNOR));
    assert_eq!(baseline.frozen_at, FROZEN_AT);
}

#[test]
fn test_frozen_refuses_a_baseline_from_a_machine_that_is_not_clean() {
    let facts = MachineFacts {
        concurrent_builds: Some(5),
        ..quiet(8)
    };
    let error = Baseline::frozen(KEY, 511.0, &report(facts), FROZEN_AT)
        .expect_err("the 511ns measurement was taken on a busy machine");
    let message = error.to_string();
    assert!(message.contains(KEY), "{message}");
    assert!(message.contains("5 build"), "{message}");
    assert!(
        matches!(error, BaselineError::Dirty { .. }),
        "the refusal names the machine, not the value"
    );
}

#[test]
fn test_frozen_refuses_a_value_that_cannot_be_divided_by() {
    for value in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let error = Baseline::frozen(KEY, value, &clean_report(), FROZEN_AT)
            .expect_err("a baseline is a divisor");
        assert!(matches!(error, BaselineError::UnusableValue { .. }));
    }
}

#[test]
fn test_compare_reports_a_measurement_within_the_tolerance() {
    let baseline = Baseline::frozen(KEY, 700.0, &clean_report(), FROZEN_AT).expect("a clean run");
    let comparison = baseline.compare(710.0, &clean_report(), 1.10);
    match comparison {
        BaselineComparison::Within { ratio } => {
            assert!((ratio - 710.0 / 700.0).abs() < 1e-9, "{ratio}");
        }
        other => panic!("expected a ratio inside the tolerance, got {other:?}"),
    }
}

#[test]
fn test_compare_reports_a_regression_past_the_tolerance() {
    let baseline = Baseline::frozen(KEY, 511.0, &clean_report(), FROZEN_AT).expect("a clean run");
    // The 511ns/726ns event itself: the same case, re-measured on an idle machine, reads as a
    // regression of forty percent against a baseline taken while five agents were running.
    match baseline.compare(726.0, &clean_report(), 1.10) {
        BaselineComparison::Regression { ratio, allowed } => {
            assert!((ratio - 726.0 / 511.0).abs() < 1e-9, "{ratio}");
            assert_eq!(allowed, 1.10);
        }
        other => panic!("expected a regression, got {other:?}"),
    }
    let unchanged = baseline.compare(511.0, &clean_report(), 1.10);
    assert!(
        matches!(unchanged, BaselineComparison::Within { .. }),
        "the baseline itself is not a regression"
    );
}

#[test]
fn test_compare_refuses_a_current_run_on_a_machine_that_is_not_clean() {
    let baseline = Baseline::frozen(KEY, 700.0, &clean_report(), FROZEN_AT).expect("a clean run");
    let facts = MachineFacts {
        concurrent_builds: Some(5),
        ..quiet(8)
    };
    match baseline.compare(726.0, &report(facts), 1.10) {
        BaselineComparison::NotComparable { reason } => {
            assert!(reason.contains("5 build"), "{reason}");
        }
        other => panic!("expected the comparison to be refused, got {other:?}"),
    }
}

#[test]
fn test_compare_refuses_two_runs_from_different_machines() {
    let baseline = Baseline::frozen(KEY, 700.0, &clean_report(), FROZEN_AT).expect("a clean run");
    let facts = MachineFacts {
        cpu_model: Some("another processor".to_owned()),
        ..quiet(8)
    };
    match baseline.compare(726.0, &report(facts), 1.10) {
        BaselineComparison::NotComparable { reason } => {
            assert!(reason.contains("another processor"), "{reason}");
            assert!(reason.contains(MODEL), "{reason}");
        }
        other => panic!("expected the comparison to be refused, got {other:?}"),
    }
}

#[test]
fn test_compare_refuses_a_run_whose_governor_changed() {
    let baseline = Baseline::frozen(KEY, 700.0, &clean_report(), FROZEN_AT).expect("a clean run");
    let facts = MachineFacts {
        scaling_governor: Some("powersave".to_owned()),
        ..quiet(8)
    };
    assert!(matches!(
        baseline.compare(726.0, &report(facts), 1.10),
        BaselineComparison::NotComparable { .. }
    ));
}

#[test]
fn test_compare_refuses_a_run_whose_machine_nobody_could_read() {
    let baseline = Baseline::frozen(KEY, 700.0, &clean_report(), FROZEN_AT).expect("a clean run");
    let facts = MachineFacts {
        cpu_model: None,
        ..quiet(8)
    };
    match baseline.compare(726.0, &report(facts), 1.10) {
        BaselineComparison::NotComparable { reason } => {
            assert!(reason.contains("unknown"), "{reason}");
        }
        other => panic!("expected the comparison to be refused, got {other:?}"),
    }
}

#[test]
fn test_compare_refuses_a_measurement_that_is_not_a_positive_number() {
    let baseline = Baseline::frozen(KEY, 700.0, &clean_report(), FROZEN_AT).expect("a clean run");
    for current in [0.0, -1.0, f64::NAN] {
        assert!(
            matches!(
                baseline.compare(current, &clean_report(), 1.10),
                BaselineComparison::NotComparable { .. }
            ),
            "{current} is not a measurement"
        );
    }
}

#[test]
fn test_baseline_round_trips_through_its_document() {
    let root = FakeRoot::new("baseline-round-trip");
    let baseline = Baseline::frozen(KEY, 511.0, &clean_report(), FROZEN_AT).expect("a clean run");
    let path = baseline_path(root.path(), KEY).expect("the key is usable");
    assert!(
        path.starts_with(root.path().join(BASELINE_DIR)),
        "{}",
        path.display()
    );

    baseline
        .write(&path)
        .expect("the directory is created for it");
    assert!(path.is_file());
    let text = fs::read_to_string(&path).expect("the document is there");
    assert!(text.contains(MODEL), "the model is in the file: {text}");
    assert!(
        text.contains(GOVERNOR),
        "the governor is in the file: {text}"
    );
    assert!(
        text.contains("1700000000"),
        "the timestamp is in the file: {text}"
    );

    let read_back = Baseline::read(&path).expect("the document parses");
    assert_eq!(read_back.as_ref(), Some(&baseline));
}

#[test]
fn test_baseline_read_of_a_key_with_no_file_is_none() {
    let root = FakeRoot::new("baseline-missing");
    let path = baseline_path(root.path(), KEY).expect("the key is usable");
    assert_eq!(
        Baseline::read(&path).expect("a missing file is not an error"),
        None
    );
}

#[test]
fn test_baseline_read_of_a_corrupt_document_is_an_error() {
    let root = FakeRoot::new("baseline-corrupt");
    let path = baseline_path(root.path(), KEY).expect("the key is usable");
    fs::create_dir_all(path.parent().expect("the path has a directory")).expect("creating it");
    fs::write(&path, "{ this is not a baseline").expect("writing the corrupt document");

    let error = Baseline::read(&path).expect_err("a corrupt baseline must not read as absent");
    assert!(matches!(error, BaselineError::Malformed { .. }));
    assert!(error.to_string().contains(KEY) || error.to_string().contains("baseline"));
}

#[test]
fn test_baseline_path_refuses_a_key_that_cannot_be_a_file_name() {
    let root = FakeRoot::new("baseline-keys");
    let too_long = "a".repeat(129);
    let keys = [
        "",
        "bench/../secret",
        "bench.passthrough classify",
        too_long.as_str(),
    ];
    for key in keys {
        let error = baseline_path(root.path(), key).expect_err("that key is not a file name");
        assert!(matches!(error, BaselineError::BadKey { .. }), "{key}");
    }
    let path = baseline_path(root.path(), KEY).expect("a dotted key is usable");
    assert!(
        path.to_string_lossy()
            .ends_with("bench.passthrough_classify_ns.json")
    );
}

#[test]
fn test_gap_text_names_what_could_not_be_read() {
    let gaps = [
        PurityGap::ProcessTableUnreadable,
        PurityGap::LoadUnreadable,
        PurityGap::CpuCountUnreadable,
        PurityGap::CeilingUnusable,
    ];
    for gap in gaps {
        assert!(!gap.text().is_empty(), "{gap:?} has no text");
    }
    assert!(PurityGap::LoadUnreadable.text().contains("load average"));
    assert!(
        PurityGap::ProcessTableUnreadable
            .text()
            .contains("process table")
    );
    assert!(
        PurityGap::CpuCountUnreadable
            .text()
            .contains("processor count")
    );
    assert!(PurityGap::CeilingUnusable.text().contains("ceiling"));
    for (index, gap) in gaps.iter().enumerate() {
        for other in &gaps[index + 1..] {
            assert_ne!(gap.text(), other.text(), "two gaps read the same");
        }
    }
}

#[test]
fn test_impurity_text_names_the_ceiling_and_the_count() {
    let builds = Impurity::ConcurrentBuilds { count: 7 }.text();
    assert!(builds.contains('7'), "{builds}");
    assert!(
        builds.contains("cargo") && builds.contains("rustc"),
        "{builds}"
    );

    let impurity = Impurity::Load {
        load_1m: 3.5,
        cpu_count: 8,
        ceiling: 2.0,
    };
    let load = impurity.text();
    assert!(load.contains("3.50"), "{load}");
    assert!(load.contains('8'), "{load}");
    assert!(load.contains("2.00"), "{load}");
}

#[test]
fn test_the_agent_program_list_covers_the_drivers_a_parallel_wave_runs() {
    for program in ["cargo", "nextest", "rustc"] {
        assert!(
            AGENT_PROGRAMS.contains(&program),
            "{program} is not counted"
        );
    }
}

#[test]
fn test_governor_note_is_none_under_the_governor_a_release_figure_is_measured_under() {
    assert_eq!(
        clean_report().governor_note(),
        None,
        "`performance` is the state a number for a release figure is taken in"
    );
}

#[test]
fn test_governor_note_annotates_another_governor_and_an_unreadable_one() {
    // The governor is an annotation and not a criterion: a machine running `powersave` still
    // produces numbers, and they are comparable with a baseline frozen under `powersave` -- it
    // is a ratio across two governors that measures nothing.
    let facts = MachineFacts {
        scaling_governor: Some("powersave".to_owned()),
        ..quiet(8)
    };
    let verdict = report(facts);
    let note = verdict
        .governor_note()
        .expect("another governor is annotated");
    assert!(note.contains("powersave"), "{note}");
    assert!(
        note.contains(GOVERNOR),
        "the expected governor is named: {note}"
    );
    assert!(
        verdict.is_clean(),
        "the annotation is not a refusal: {note}"
    );

    let facts = MachineFacts {
        scaling_governor: None,
        ..quiet(8)
    };
    let verdict = report(facts);
    let note = verdict
        .governor_note()
        .expect("a governor nobody could read is annotated");
    assert!(note.contains("could not be read"), "{note}");
    assert!(
        verdict.is_clean(),
        "an unreadable governor is not a refusal either"
    );
}

#[test]
fn test_judge_uses_the_factor_the_caller_passed_rather_than_one_of_its_own() {
    // The load factor is the module's one threshold and it is the caller's: the budget document
    // states thresholds, and a second copy of one here would be the drift this project refuses.
    // Eight processors carrying a load of one is inside the guard's own quarter and outside a
    // caller's tenth, so the two policies must disagree about the same machine.
    let facts = MachineFacts {
        load_1m: Some(1.0),
        ..quiet(8)
    };
    assert!(report(facts.clone()).is_clean());

    let strict = PurityPolicy {
        max_load_per_cpu: 0.1,
    };
    let report = PurityReport::judge(facts, &strict);
    assert!(!report.is_clean(), "the caller's factor decides");
    let reason = report
        .rejection_reason()
        .expect("a loaded machine has a reason");
    assert!(
        reason.contains("0.80"),
        "the caller's ceiling is named: {reason}"
    );
}

#[test]
fn test_the_guard_needs_no_writable_home_directory() {
    // Every path the guard touches is derived from a root the caller passes in -- the scratch
    // root here, `/` in `sample` -- and it reads no `$HOME` and no XDG variable. A host whose
    // home is not writable is therefore not a host this needs. The assertion pins the choice the
    // fixture makes, so a later change to a home-based scratch directory fails here rather than
    // on the one machine where the difference shows.
    let root = FakeRoot::new("no-home-needed");
    let temp = std::env::temp_dir();
    assert!(
        root.path().starts_with(&temp),
        "the scratch root must be below {}, not under the user's home: {}",
        temp.display(),
        root.path().display()
    );

    let baseline = Baseline::frozen(KEY, 511.0, &clean_report(), FROZEN_AT).expect("a clean run");
    let path = baseline_path(root.path(), KEY).expect("the key is usable");
    assert!(path.starts_with(root.path()), "{}", path.display());
    baseline
        .write(&path)
        .expect("written below the root it was given");
    assert!(path.is_file());
    assert!(Baseline::read(&path).expect("it parses").is_some());
}
