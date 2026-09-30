//! Tests for [`super`].
//!
//! They cover the two things the runner rests on: the table, which is a transcription and has to
//! be held to the document it was transcribed from, and the verdict rule, which is the only place
//! a case can be recorded as something it is not. Neither is exercised by running a command --
//! the verdict tests inject the command results, and the table tests read the table -- so the
//! whole file runs on a machine with no toolchain, no display server and no repository state.
//!
//! The two bundle tests at the end are the exception to "no process at all": they drive a case
//! through [`RunDir`] into a scratch directory, which is how the runner's own archive contract --
//! a passing case leaves no trace, a failing one leaves a trace with its diff -- is held to the
//! same rules the evidence module's own tests hold it to.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;

use clap::Command;

use super::*;
use crate::testd::env::{DisplayServer, EnvCapabilities, WaylandTier};
use crate::testd::evidence::EnvironmentFact;

// Named from the module that defines them rather than through the parent's glob: a test that
// says where a name comes from is a test a reader can follow without opening the module root.
use super::cases::{CORE_CASES, CORE_MODULE, CaseLine};
use super::command::{CommandLine, CommandOutcome, CommandRun, TAIL_LINES, tail};
use super::environment::{Environment, wsl_of};
use super::error::SuiteError;
use super::verdict;

/// A scratch directory that removes itself when the test ends.
#[derive(Debug)]
struct Scratch {
    /// The directory this guard owns.
    path: PathBuf,
}

impl Scratch {
    /// Creates `<temp>/rspinyin-suite-<tag>-<pid>`, empty.
    fn new(tag: &str) -> Self {
        let name = format!("rspinyin-suite-{tag}-{}", std::process::id());
        let path = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("creating the scratch directory");
        Self { path }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// The instant every test names as a run's start, so no test reads a clock.
fn started() -> Utc {
    Utc::from_unix_seconds(1_700_000_000)
}

/// A command run that exited with `code`.
fn exited(code: i32) -> CommandRun {
    CommandRun {
        declared: String::from("cargo nextest run -p ime-core segment::syllable"),
        executed: String::from("cargo nextest run -p ime-core segment::syllable"),
        outcome: CommandOutcome::Exited { code: Some(code) },
        duration_ms: 12,
        stdout_tail: String::from("Summary [ 0.12s] 30 tests run: 30 passed"),
        stderr_tail: String::new(),
    }
}

/// A command run that was never started.
fn not_started(reason: &str) -> CommandRun {
    CommandRun {
        declared: String::from("cargo nextest run -p ime-core"),
        executed: String::from("cargo nextest run -p ime-core"),
        outcome: CommandOutcome::NotStarted {
            reason: reason.to_owned(),
        },
        duration_ms: 0,
        stdout_tail: String::new(),
        stderr_tail: String::new(),
    }
}

/// The case of the table with this identifier.
fn case(id: &str) -> &'static CaseLine {
    CORE_CASES
        .iter()
        .find(|case| case.id == id)
        .expect("the table holds the case the test names")
}

/// One fact's value, or `None` when the list does not carry the key.
fn fact<'a>(facts: &'a [EnvironmentFact], key: &str) -> Option<&'a str> {
    facts
        .iter()
        .find(|fact| fact.key == key)
        .map(|fact| fact.value.as_str())
}

/// An environment report of a machine that is not this one.
fn report() -> EnvCapabilities {
    EnvCapabilities {
        fcitx5_version: Some(String::from("5.1.7")),
        dev_packages: true,
        addons_loaded: Vec::new(),
        display_server: DisplayServer::X11,
        compositor: Some(String::from("weston")),
        tier: WaylandTier::NotApplicable,
        wlr_layer_shell: false,
        argb_visual: true,
        compositor_present: false,
        cjk_font_count: 12,
        writable_data_dir: true,
        concurrent_agents: 3,
        display: Some(String::from(":0")),
        wayland_display: None,
        gaps: Vec::new(),
    }
}

/// An environment whose two added facts are read from a tree that is not a repository.
///
/// `/proc/version` is read for real -- the value is asserted as one of three labels rather than
/// as a fixed one -- and the commit is read from a path that cannot be a repository, which is
/// what makes the `unknown` branch reachable without depending on the machine.
fn environment() -> Environment {
    Environment::of(&report(), Path::new("/rspinyin-suite-no-such-tree"))
}

#[test]
fn test_cases_hold_the_thirty_baseline_cases_in_document_order() {
    assert_eq!(CORE_CASES.len(), 30, "the P0 baseline is thirty cases");
    for (index, case) in CORE_CASES.iter().enumerate() {
        let expected = format!("TC-CORE-{:02}", index + 1);
        assert_eq!(case.id, expected, "the table is in the document's order");
        assert_eq!(case.module, CORE_MODULE);
        assert_eq!(case.module, "core");
    }
}

#[test]
fn test_every_case_states_a_criterion_and_a_precondition() {
    for case in &CORE_CASES {
        assert!(
            !case.criteria.trim().is_empty(),
            "{} states no pass criterion, so nothing judges it",
            case.id
        );
        assert!(
            !case.precondition.trim().is_empty(),
            "{} states no precondition",
            case.id
        );
        // The command and the precondition are split apart when the table is written, so a
        // criterion that still carried a command would mean the split went wrong.
        assert!(
            !case.criteria.contains("cargo "),
            "{}: the criterion still carries a command: {}",
            case.id,
            case.criteria
        );
    }
}

#[test]
fn test_the_table_declares_eighteen_commands_and_twelve_cases_without_one() {
    let declared = CORE_CASES.iter().filter(|c| !c.commands.is_empty()).count();
    let undeclared = CORE_CASES.iter().filter(|c| c.commands.is_empty()).count();
    assert_eq!(declared + undeclared, CORE_CASES.len());
    assert_eq!(
        declared, 18,
        "eighteen cases of the document state a command to run"
    );
    assert_eq!(
        undeclared, 12,
        "twelve cases state a precondition and no command, and none of them may be given one"
    );
}

#[test]
fn test_every_declared_command_is_one_the_runner_can_split() {
    for case in &CORE_CASES {
        for command in case.commands {
            let line = CommandLine::parse(command)
                .unwrap_or_else(|reason| panic!("{}: {command}: {reason}", case.id));
            assert!(
                !line.program().is_empty(),
                "{}: {command} names no program",
                case.id
            );
            assert_eq!(line.text(), command.trim());
        }
    }
}

#[test]
fn test_every_nextest_command_names_the_package_under_test() {
    let mut nextest = 0;
    for case in &CORE_CASES {
        for command in case.commands {
            if let Some(rest) = command.strip_prefix("cargo nextest run ") {
                nextest += 1;
                assert!(
                    rest.contains("-p ime-core"),
                    "{}: `{command}` does not name the package",
                    case.id
                );
            }
        }
    }
    assert_eq!(nextest, 16, "sixteen of the eighteen commands run nextest");
}

#[test]
fn test_selected_without_a_selection_returns_the_whole_module() {
    let all = cases::selected(CORE_MODULE, &[]).expect("the module the table is written for");
    assert_eq!(all.len(), CORE_CASES.len());
    assert_eq!(all.first().map(|case| case.id), Some("TC-CORE-01"));
    assert_eq!(all.last().map(|case| case.id), Some("TC-CORE-30"));
}

#[test]
fn test_selected_of_one_case_returns_that_case_alone() {
    let one = cases::selected(CORE_MODULE, &[String::from("TC-CORE-07")])
        .expect("a case the table holds");
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].id, "TC-CORE-07");
    assert_eq!(one[0].commands, &["cargo nextest run -p ime-core segment::dag"]);
}

#[test]
fn test_selected_keeps_the_documents_order_whatever_order_is_named() {
    let selection = vec![String::from("TC-CORE-19"), String::from("TC-CORE-02")];
    let named = cases::selected(CORE_MODULE, &selection).expect("two cases the table holds");
    let ids: Vec<&str> = named.iter().map(|case| case.id).collect();
    assert_eq!(ids, ["TC-CORE-02", "TC-CORE-19"]);
}

#[test]
fn test_selected_refuses_a_case_the_table_does_not_hold() {
    let selection = vec![String::from("TC-CORE-02"), String::from("TC-CORE-99")];
    let refusal = cases::selected(CORE_MODULE, &selection).expect_err("TC-CORE-99 is not held");
    assert_eq!(refusal.code(), "suite/case/unknown");
    assert!(refusal.to_string().contains("TC-CORE-99"), "{refusal}");
}

#[test]
fn test_selected_refuses_a_module_the_table_does_not_cover() {
    let refusal = cases::selected("dict", &[]).expect_err("the table holds no dict case");
    assert_eq!(refusal.code(), "suite/selection/empty");
    assert!(refusal.to_string().contains("dict"), "{refusal}");
}

#[test]
fn test_judge_a_zero_exit_is_a_pass() {
    assert_eq!(verdict::judge(&[exited(0)]), CaseStatus::Pass);
    assert_eq!(verdict::judge(&[exited(0), exited(0)]), CaseStatus::Pass);
}

#[test]
fn test_judge_a_non_zero_exit_is_a_fail() {
    assert_eq!(verdict::judge(&[exited(1)]), CaseStatus::Fail);
    assert_eq!(verdict::judge(&[exited(101)]), CaseStatus::Fail);
    assert_eq!(verdict::judge(&[exited(0), exited(100)]), CaseStatus::Fail);
}

#[test]
fn test_judge_a_signalled_process_is_a_failure() {
    let killed = CommandRun {
        outcome: CommandOutcome::Exited { code: None },
        ..exited(0)
    };
    assert_eq!(verdict::judge(&[killed]), CaseStatus::Fail);
}

#[test]
fn test_judge_a_command_that_never_started_is_flawed_and_never_a_pass() {
    let status = verdict::judge(&[not_started("No such file or directory")]);
    assert_eq!(status, CaseStatus::Flawed);
    assert_ne!(
        status,
        CaseStatus::Pass,
        "a case that could not be run may never be recorded as one that passed"
    );
}

#[test]
fn test_judge_a_case_with_no_command_is_flawed() {
    assert_eq!(verdict::judge(&[]), CaseStatus::Flawed);
}

#[test]
fn test_judge_a_failure_outranks_a_command_that_never_started() {
    // The run's own verdict states the precedence for a whole batch -- a failure outranks a gap
    // -- and a case keeps the same precedence, with the gap still counted beside it.
    let status = verdict::judge(&[exited(101), not_started("no such program")]);
    assert_eq!(status, CaseStatus::Fail);
}

#[test]
fn test_command_line_parse_splits_a_declared_line() {
    let line = CommandLine::parse("cargo nextest run -p ime-core segment::syllable")
        .expect("a line with a first word");
    assert_eq!(line.program(), "cargo");
    assert_eq!(
        line.args(),
        &["nextest", "run", "-p", "ime-core", "segment::syllable"]
    );
    assert_eq!(line.text(), "cargo nextest run -p ime-core segment::syllable");
}

#[test]
fn test_command_line_parse_refuses_a_line_with_no_first_word() {
    assert!(CommandLine::parse("").is_err());
    assert!(CommandLine::parse("   \t ").is_err());
    // The refusal has to say what is wrong with the line rather than only that it is wrong.
    let reason = CommandLine::parse("").expect_err("an empty line names no program");
    assert!(reason.contains("program"), "{reason}");
}

#[test]
fn test_tail_keeps_the_last_lines_and_drops_the_rest() {
    let text: String = (1..=TAIL_LINES + 5)
        .map(|index| format!("line {index}\n"))
        .collect();
    let kept = tail(&text);
    assert_eq!(kept.lines().count(), TAIL_LINES);
    assert!(kept.starts_with("line 6\n"), "{kept}");
    assert!(kept.ends_with(&format!("line {}", TAIL_LINES + 5)));
    assert!(!kept.contains("line 1\n"), "the head is dropped: {kept}");

    // A command shorter than the ceiling is kept whole, and an empty one stays empty.
    assert_eq!(tail("one\ntwo\n"), "one\ntwo");
    assert_eq!(tail(""), "");
}

#[test]
fn test_execute_reports_the_exit_status_of_a_real_command() {
    let cwd = std::env::temp_dir();
    let success = command::execute("true", &cwd);
    assert_eq!(success.outcome, CommandOutcome::Exited { code: Some(0) });
    assert!(success.outcome.is_success());
    assert_eq!(success.executed, "true");

    let failure = command::execute("false", &cwd);
    assert_eq!(failure.outcome, CommandOutcome::Exited { code: Some(1) });
    assert!(failure.outcome.is_failure());
}

#[test]
fn test_execute_reports_a_program_that_is_not_there() {
    let run = command::execute("rspinyin-suite-no-such-program", &std::env::temp_dir());
    assert!(run.outcome.is_not_started(), "{:?}", run.outcome);
    assert!(!run.outcome.is_success());
    assert!(run.outcome.describe().contains("could not be started"));

    let empty = command::execute("", &std::env::temp_dir());
    assert!(empty.outcome.is_not_started(), "{:?}", empty.outcome);
}

#[test]
fn test_assertions_record_each_commands_exit_code() {
    let case = case("TC-CORE-01");
    let records = verdict::assertions_of(case, &[exited(0), not_started("no such program")]);
    let names: Vec<&str> = records.iter().map(|record| record.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "command_1",
            "exit_code_1",
            "stdout_tail_1",
            "stderr_tail_1",
            "command_2",
            "exit_code_2",
            "stdout_tail_2",
            "stderr_tail_2",
            "pass_criteria",
        ]
    );
    assert!(records[1].ok, "the first command exited 0");
    assert_eq!(records[1].actual.as_str(), "0");
    assert!(!records[5].ok, "the second command never ran");
    assert_eq!(records[5].actual.as_str(), "not started");
    assert_eq!(records[0].expected.as_str(), case.commands[0]);
}

#[test]
fn test_assertions_withhold_every_captured_tail() {
    // The text a tail can carry is what the user typed -- a test name built from a pinyin
    // buffer, a candidate sequence in an assertion message -- so it may reach a document only
    // through the withheld form. This is the assertion that holds the rule.
    let secret = "nihao 你好";
    let run = CommandRun {
        stdout_tail: format!("assertion failed: decode(\"{secret}\")"),
        stderr_tail: format!("candidate `{secret}` was not first"),
        ..exited(101)
    };
    let case = case("TC-CORE-16");
    let evidence = CaseEvidence {
        tc: case.id.to_owned(),
        module: case.module.to_owned(),
        status: verdict::judge(std::slice::from_ref(&run)),
        started_at: started(),
        duration_ms: 7,
        assertions: verdict::assertions_of(case, std::slice::from_ref(&run)),
        visual: Vec::new(),
        healed: Vec::new(),
        environment: Vec::new(),
    };
    let document = evidence.to_json().expect("the record is JSON");
    assert!(
        !document.contains(secret),
        "the bundle must not carry the text a command printed: {document}"
    );
    assert!(document.contains("<redacted:len="), "{document}");
    assert!(document.contains("stdout_tail_1"), "{document}");
    // The count is kept, which is what tells a reader there was output at all.
    assert!(
        document.contains(&format!("<redacted:len={}>", run.stdout_tail.chars().count())),
        "{document}"
    );
}

#[test]
fn test_assertions_name_a_case_without_a_command_as_a_failed_assertion() {
    let case = case("TC-CORE-24");
    let records = verdict::assertions_of(case, &[]);
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[0].name, "command_1");
    assert!(!records[0].ok, "a case with nothing to run did not hold");
    assert!(
        records[0].actual.as_str().contains(case.precondition),
        "the diff has to carry the reason: {:?}",
        records[0].actual
    );
    assert_eq!(records[1].name, "pass_criteria");
}

#[test]
fn test_assertions_carry_the_documents_criterion() {
    let case = case("TC-CORE-01");
    let records = verdict::assertions_of(case, &[exited(0)]);
    let criteria = records
        .iter()
        .find(|record| record.name == "pass_criteria")
        .expect("the criterion is recorded");
    assert_eq!(criteria.actual.as_str(), case.criteria);
    assert!(criteria.ok);
}

#[test]
fn test_tail_block_names_a_case_that_declared_no_command() {
    let case = case("TC-CORE-22");
    let block = verdict::tail_block(case, &[]);
    assert!(block.contains("TC-CORE-22"), "{block}");
    assert!(block.contains(case.precondition), "{block}");
}

#[test]
fn test_environment_takes_the_channels_own_lines() {
    let environment = environment();
    let facts = environment.facts();
    // These come from `EnvCapabilities::lines`, which is the environment channel's own report;
    // the runner re-probes none of them.
    assert_eq!(fact(facts, "display_server"), Some("x11"));
    assert_eq!(fact(facts, "concurrent_agents"), Some("3"));
    assert_eq!(fact(facts, "fcitx5_version"), Some("5.1.7"));
    assert_eq!(fact(facts, "compositor"), Some("weston"));
    assert_eq!(fact(facts, "display"), Some(":0"));
    assert_eq!(fact(facts, "wayland_display"), Some("none"));
}

#[test]
fn test_environment_records_wsl_and_the_commit() {
    let environment = environment();
    let facts = environment.facts();
    let wsl = fact(facts, "wsl").expect("the machine's kind is recorded");
    assert!(
        matches!(wsl, "yes" | "no" | "unknown"),
        "the kernel reading is one of three labels, not {wsl}"
    );
    assert_eq!(
        fact(facts, "commit"),
        Some("unknown"),
        "a tree that is not a repository has no commit, and unknown is not a guess"
    );
}

#[test]
fn test_wsl_of_reads_the_kernels_own_version_text() {
    assert_eq!(
        wsl_of(Some(
            "Linux version 6.18.40.1-microsoft-standard-WSL2 (gcc 13) #1 SMP"
        )),
        "yes"
    );
    assert_eq!(wsl_of(Some("Linux version 6.8.0-generic (gcc 13)")), "no");
    assert_eq!(
        wsl_of(None),
        "unknown",
        "a machine nobody could ask is not a machine that answered"
    );
}

#[test]
fn test_commit_of_a_tree_that_is_not_a_repository_is_unknown() {
    let scratch = Scratch::new("commit");
    let environment = Environment::of(&report(), &scratch.path);
    assert_eq!(fact(environment.facts(), "commit"), Some("unknown"));
}

#[test]
fn test_suite_error_codes_are_stable() {
    let unknown = SuiteError::UnknownCase {
        tc: String::from("TC-CORE-99"),
        module: String::from("core"),
    };
    let empty = SuiteError::NoCases {
        module: String::from("dict"),
    };
    let not_green = SuiteError::BatchNotGreen {
        verdict: BatchVerdict::Flawed,
        passed: 18,
        failed: 0,
        flawed: 12,
        gaps: 0,
    };
    assert_eq!(unknown.code(), "suite/case/unknown");
    assert_eq!(empty.code(), "suite/selection/empty");
    assert_eq!(not_green.code(), "suite/batch/not-green");
    // The batch's own verdict is carried as a value, so a caller can tell a failed run from one
    // whose evidence is incomplete.
    assert!(not_green.to_string().contains("flawed"), "{not_green}");
    assert!(not_green.to_string().contains("12"), "{not_green}");
}

#[test]
fn test_run_case_of_a_failing_case_leaves_a_bundle_with_a_trace() {
    let scratch = Scratch::new("failing");
    let run_dir = RunDir::create(&scratch.path, started()).expect("a fresh run directory");
    let case = case("TC-CORE-11");
    let evidence = run_case(case, &environment(), &[exited(101)], 41);
    assert_eq!(evidence.status, CaseStatus::Fail);
    assert_eq!(evidence.tc, "TC-CORE-11");
    assert_eq!(evidence.module, CORE_MODULE);
    assert_eq!(evidence.duration_ms, 41);
    assert!(!evidence.environment.is_empty(), "the machine is recorded");
    assert!(evidence.assertions.len() >= 4);

    let trace = TraceRecord::of(&evidence);
    assert!(!trace.diffs.is_empty(), "a failure leaves its diff");
    let bundle = run_dir
        .write_case(&evidence, Some(&trace), None)
        .expect("the bundle is written");
    assert_eq!(
        bundle.dir,
        scratch.path.join("runs").join(run_dir.name()).join("core").join("TC-CORE-11")
    );
    assert!(bundle.trace.is_some(), "a failed case leaves a trace");
    let document = fs::read_to_string(&bundle.assertions).expect("assertions.json is there");
    assert!(document.contains("\"fail\""), "{document}");
    assert!(document.contains("<redacted:len="), "{document}");
    assert_eq!(
        fs::metadata(&bundle.assertions)
            .expect("the file is there")
            .permissions()
            .mode()
            & 0o777,
        0o600,
        "a bundle is readable by its owner alone"
    );
}

#[test]
fn test_run_case_of_a_passing_case_leaves_no_trace() {
    let scratch = Scratch::new("passing");
    let run_dir = RunDir::create(&scratch.path, started()).expect("a fresh run directory");
    let case = case("TC-CORE-01");
    let evidence = run_case(case, &environment(), &[exited(0)], 9);
    assert_eq!(evidence.status, CaseStatus::Pass);
    assert!(!evidence.status.needs_trace());
    let bundle = run_dir
        .write_case(&evidence, None, None)
        .expect("the bundle is written");
    assert!(bundle.trace.is_none(), "a passing case leaves no trace");
    assert!(bundle.assertions.is_file());
}

#[test]
fn test_run_case_of_a_case_without_a_command_is_flawed() {
    let case = case("TC-CORE-25");
    let evidence = run_case(case, &environment(), &[], 0);
    assert_eq!(evidence.status, CaseStatus::Flawed);
    assert!(
        evidence.status.needs_trace(),
        "a case that could not be judged has to leave a reader the reason"
    );
    assert!(!evidence.failed_assertions().is_empty());
}

#[test]
fn test_suite_args_default_to_the_results_tree_and_the_core_module() {
    let matches = SuiteArgs::augment_args(Command::new("suite")).get_matches_from(["suite"]);
    assert_eq!(
        matches.get_one::<PathBuf>("results_root"),
        Some(&PathBuf::from("results"))
    );
    assert_eq!(
        matches.get_one::<String>("module"),
        Some(&String::from(CORE_MODULE)),
        "the default module and the module the table is written for cannot drift apart"
    );
    assert!(
        matches.get_many::<String>("only").is_none(),
        "without --only, the whole module runs"
    );

    let named = SuiteArgs::augment_args(Command::new("suite")).get_matches_from([
        "suite",
        "--module",
        "core",
        "--only",
        "TC-CORE-07",
        "--only",
        "TC-CORE-08",
        "--results-root",
        "/tmp/rspinyin-results",
    ]);
    let only: Vec<&str> = named
        .get_many::<String>("only")
        .expect("two identifiers")
        .map(String::as_str)
        .collect();
    assert_eq!(only, ["TC-CORE-07", "TC-CORE-08"]);
    assert_eq!(
        named.get_one::<PathBuf>("results_root"),
        Some(&PathBuf::from("/tmp/rspinyin-results"))
    );
}

#[test]
fn test_results_root_resolves_a_relative_path_against_the_repository() {
    let root = Path::new("/repo");
    assert_eq!(
        results_root(Path::new("results"), root),
        PathBuf::from("/repo/results")
    );
    assert_eq!(
        results_root(Path::new("/elsewhere/results"), root),
        PathBuf::from("/elsewhere/results")
    );
}

#[test]
fn test_outcome_is_green_only_for_a_batch_that_passed() {
    let green = BatchSummary {
        passed: 30,
        ..BatchSummary::default()
    };
    assert!(outcome(green).is_ok());

    let failed = BatchSummary {
        passed: 18,
        failed: 1,
        ..BatchSummary::default()
    };
    assert_eq!(
        outcome(failed)
            .expect_err("a failed case fails the run")
            .code(),
        "suite/batch/not-green"
    );

    let flawed = BatchSummary {
        passed: 18,
        flawed: 12,
        ..BatchSummary::default()
    };
    let refusal = outcome(flawed).expect_err("a case that could not be judged fails the run");
    assert_eq!(refusal.code(), "suite/batch/not-green");
    assert!(
        refusal.to_string().contains("could not be judged"),
        "{refusal}"
    );

    let empty = BatchSummary::default();
    let refusal = outcome(empty).expect_err("a batch that judged nothing may not come out green");
    assert_eq!(refusal.code(), "suite/batch/empty");
}

#[test]
fn test_line_of_reports_the_bundle_a_case_left() {
    let scratch = Scratch::new("line");
    let run_dir = RunDir::create(&scratch.path, started()).expect("a fresh run directory");
    let case = case("TC-CORE-01");
    let evidence = run_case(case, &environment(), &[exited(0)], 9);
    let complete = run_dir.write_case(&evidence, None, None);
    let line = line_of(&evidence, &complete);
    assert!(line.contains("TC-CORE-01"), "{line}");
    assert!(line.contains("pass"), "{line}");
    assert!(line.contains("complete"), "{line}");
    assert!(line.contains("9 ms"), "{line}");
}

#[test]
fn test_line_of_reports_a_bundle_that_could_not_be_written() {
    // A write that failed is a second failure beside the verdict, and the line has to show both.
    // The reason the archive hands back is path-free by construction, so the line a run's index
    // records carries no filesystem path.
    let evidence = run_case(case("TC-CORE-01"), &environment(), &[exited(0)], 9);
    let refusal: Result<CaseBundle, EvidenceError> = Err(EvidenceError::RunExists {
        path: PathBuf::from("/repo/results/runs/run-20231114-221320"),
    });
    let line = line_of(&evidence, &refusal);
    assert!(line.contains("gap:"), "{line}");
    assert!(
        !line.contains("/repo"),
        "the index line carries no filesystem path: {line}"
    );
}
