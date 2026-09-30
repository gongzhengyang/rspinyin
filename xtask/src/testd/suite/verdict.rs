//! What a case's commands came to, and the evidence they leave behind.
//!
//! Responsibility: judge a case from the command runs it produced, write the assertions its
//! bundle holds, and render the block a case that did not pass prints to the console. Boundaries:
//! it starts no process, reads no clock and touches no file -- the runs arrive as values, which is
//! what lets every rule here be asserted without running a command at all.
//!
//! # Three verdicts, and the precedence between them
//!
//! * every command line exited `0` -> [`CaseStatus::Pass`];
//! * a command line exited non-zero, or a signal killed it -> [`CaseStatus::Fail`];
//! * a command line was never started, or the case declares none -> [`CaseStatus::Flawed`].
//!
//! A failure outranks a gap, which is the precedence
//! [`BatchSummary::verdict`](crate::testd::evidence::BatchSummary::verdict) states for a whole
//! run: a case that both failed and lost a command is reported as the failure it certainly was,
//! with the run's gap count still carrying the loss.
//!
//! What a non-zero exit *means* is deliberately not this module's judgement. A build that failed
//! to compile, a filter that matched no test, a feature this workspace does not declare and a
//! test that genuinely failed all arrive here as the same fact, and the runner does not read the
//! tool's prose to tell them apart -- a heuristic over a tool's output is exactly the reading
//! that lets a broken run be filed as a green one. Only "the process was never started" is a gap,
//! and a gap is never a pass.
//!
//! # The assertions a bundle holds
//!
//! One group per command line, named after its position (`command_1`, `exit_code_1`,
//! `stdout_tail_1`, `stderr_tail_1`), then the case's own criterion. The names carry the position
//! because a case may state two commands -- a benchmark run and the gate that judges what it
//! measured -- and a reader has to be able to tell which of them exited non-zero.
//!
//! The two tail assertions are where the privacy rule is enforced rather than promised: the
//! captured text goes through [`EvidenceValue::withheld`], so the document records how many
//! characters of output there were and never what they said. A command's output can quote what
//! the user typed -- an assertion message that prints a pinyin buffer, a candidate sequence --
//! and the bundle travels.

use crate::testd::evidence::{AssertionRecord, CaseStatus, EvidenceValue};

use super::cases::CaseLine;
use super::command::CommandRun;

/// What a case came to, judged from the command runs it produced.
///
/// # Panics
///
/// Never.
pub fn judge(runs: &[CommandRun]) -> CaseStatus {
    if runs.is_empty() {
        return CaseStatus::Flawed;
    }
    if runs.iter().any(|run| run.outcome.is_failure()) {
        return CaseStatus::Fail;
    }
    if runs.iter().any(|run| run.outcome.is_not_started()) {
        return CaseStatus::Flawed;
    }
    CaseStatus::Pass
}

/// The assertions one case's run leaves in its bundle.
///
/// The list is built from the runs rather than from a second record of them, so a bundle cannot
/// name a command the case did not run, and a command it ran cannot be missing from the bundle.
///
/// # Panics
///
/// Never.
pub fn assertions_of(case: &CaseLine, runs: &[CommandRun]) -> Vec<AssertionRecord> {
    let mut records = Vec::new();
    if runs.is_empty() {
        records.push(undeclared_command(case));
    }
    for (index, run) in runs.iter().enumerate() {
        let position = index + 1;
        records.push(command_assertion(position, run));
        records.push(exit_code_assertion(position, run));
        records.push(tail_assertion(format!("stdout_tail_{position}"), &run.stdout_tail));
        records.push(tail_assertion(format!("stderr_tail_{position}"), &run.stderr_tail));
    }
    records.push(criteria_assertion(case));
    records
}

/// The block a case that did not pass prints to the console.
///
/// The console is the operator's own channel and is not a document: what a failing case printed
/// is here, in full, because a failure whose output nobody can read is a failure nobody can fix.
/// It is no more than running the command by hand would have printed, and none of it is written
/// to a file of this tool's own -- the bundle beside it records the length of each tail and not
/// its text.
///
/// # Panics
///
/// Never.
pub fn tail_block(case: &CaseLine, runs: &[CommandRun]) -> String {
    if runs.is_empty() {
        return format!(
            "suite: {} declares no command, so nothing ran (precondition: {})\n",
            case.id, case.precondition
        );
    }
    let mut text = String::new();
    for run in runs {
        text.push_str(&format!(
            "suite: {}: `{}` {}\n",
            case.id,
            run.declared,
            run.outcome.describe()
        ));
        for (stream, tail) in [("stdout", &run.stdout_tail), ("stderr", &run.stderr_tail)] {
            if tail.trim().is_empty() {
                continue;
            }
            let lines = tail.lines().count();
            text.push_str(&format!(
                "--- {} {} (last {lines} line(s); the bundle withholds this text) ---\n{tail}\n",
                case.id, stream
            ));
        }
    }
    text
}

/// The assertion that the command which ran is the command the document states.
///
/// It compares the document's own line against the invocation rebuilt from the program and the
/// arguments, which is the only check in the archive that can catch a table row transcribed with
/// a word out of place. A row that agrees -- every row today -- records the same line on both
/// sides, which is what makes the bundle readable on its own: the reader sees what was run.
///
/// # Panics
///
/// Never.
fn command_assertion(position: usize, run: &CommandRun) -> AssertionRecord {
    AssertionRecord {
        name: format!("command_{position}"),
        expected: EvidenceValue::fact(run.declared.as_str()),
        actual: EvidenceValue::fact(run.executed.as_str()),
        ok: run.declared == run.executed,
    }
}

/// The assertion that the command finished successfully.
///
/// A command that was never started is a failure of this assertion rather than a missing one:
/// the expected `0` is what the case asked for, and "not started" is what it got.
///
/// # Panics
///
/// Never.
fn exit_code_assertion(position: usize, run: &CommandRun) -> AssertionRecord {
    AssertionRecord {
        name: format!("exit_code_{position}"),
        expected: EvidenceValue::fact("0"),
        actual: EvidenceValue::fact(run.outcome.code_text()),
        ok: run.outcome.is_success(),
    }
}

/// The record of one stream's tail, with its text withheld.
///
/// The assertion holds whenever the tail was captured, including when it is empty: an empty tail
/// is a command that said nothing, which is a reading and not a failure.
///
/// # Panics
///
/// Never.
fn tail_assertion(name: String, tail: &str) -> AssertionRecord {
    AssertionRecord {
        name,
        expected: EvidenceValue::fact("a captured tail"),
        actual: EvidenceValue::withheld(tail),
        ok: true,
    }
}

/// The case's own pass criterion, recorded so a bundle says what it is evidence for.
///
/// # Panics
///
/// Never.
fn criteria_assertion(case: &CaseLine) -> AssertionRecord {
    AssertionRecord {
        name: String::from("pass_criteria"),
        expected: EvidenceValue::fact("the case document states a pass criterion"),
        actual: EvidenceValue::fact(case.criteria),
        ok: !case.criteria.trim().is_empty(),
    }
}

/// The assertion that stands in for a case whose document declares no command.
///
/// It fails, which is what puts the reason into the case's `trace.json` as a diff: a case that
/// could not be run has to leave a reader something to act on, and the document's own
/// precondition is the actionable part -- it names the double or the fixture the case is
/// expressed against, which is where a command has to come from.
///
/// # Panics
///
/// Never.
fn undeclared_command(case: &CaseLine) -> AssertionRecord {
    AssertionRecord {
        name: String::from("command_1"),
        expected: EvidenceValue::fact("a command line the runner can execute"),
        actual: EvidenceValue::fact(format!(
            "the case document declares no command; its precondition is: {}",
            case.precondition
        )),
        ok: false,
    }
}
