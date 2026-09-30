//! Running one declared command line, and what it left behind.
//!
//! Responsibility: split a command line the case table states into a program and its arguments,
//! start it, and answer with the exit status, the two output tails and the duration. Boundaries:
//! it decides nothing about a case. Whether an exit status means the case passed is
//! [`super::verdict`]'s judgement, and this module never looks at what a command printed to work
//! anything out.
//!
//! # No shell
//!
//! A command line is split on whitespace and the process is started directly, with no shell in
//! between. That is deliberate rather than convenient: a shell would interpret a table entry as
//! a program of its own, so a row that had been mis-transcribed could run something nobody
//! intended, and a `$` or a glob in the document's text would mean something the document never
//! said. Every command the case table states is a sequence of plain words -- a program, its
//! arguments, a package name, a filter -- so splitting on whitespace reproduces exactly the
//! invocation the document names, and a line that needs quoting to be reproduced is a line this
//! runner refuses to guess at.
//!
//! # Failure is a value, not an error
//!
//! Nothing here returns a `Result`. A command that cannot be started -- the program is not on
//! the `PATH`, or the line has no first word -- is a [`CommandOutcome::NotStarted`], which is a
//! fact about the run that the case's evidence has to carry. Returning an error instead would
//! let a caller lose it in a `?`, and the one thing this runner may never do is record a case
//! it could not run as a case that passed.
//!
//! # The tails
//!
//! The last [`TAIL_LINES`] lines of each stream are kept. A tail rather than everything: a
//! failing build prints the same diagnostic hundreds of times, and the end of the output is
//! where the reason is. The tail is captured whole and reaches a document only through
//! [`EvidenceValue::withheld`](crate::testd::evidence::EvidenceValue::withheld) -- a command's
//! output can quote what the user typed, so the archive records how much of it there was and not
//! what it said.

use std::path::Path;
use std::process::Command;
use std::time::Instant;

/// How many lines of a command's output are kept.
///
/// Enough for a failing assertion, the summary line a test runner ends with, and the note a
/// missing feature produces; short enough that the console block for a failing case stays
/// readable.
pub const TAIL_LINES: usize = 40;

/// One command line of the case table, split into a program and its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandLine {
    /// The program to start, exactly as the document spells it.
    program: String,
    /// Its arguments, in the document's order.
    args: Vec<String>,
}

impl CommandLine {
    /// Splits `text` into a program and its arguments on whitespace.
    ///
    /// # Errors
    ///
    /// Returns the reason as one sentence when `text` has no first word, which is the only way a
    /// line can fail to name a program.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut words = text.split_whitespace();
        let Some(program) = words.next() else {
            return Err(String::from(
                "the declared command line has no first word, so it names no program",
            ));
        };
        Ok(Self {
            program: program.to_owned(),
            args: words.map(str::to_owned).collect(),
        })
    }

    /// The program this line names.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn program(&self) -> &str {
        &self.program
    }

    /// The arguments this line passes it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// The line as one string, rebuilt from the program and the arguments.
    ///
    /// This is what the evidence compares the document's own text against: the assertion holds
    /// when the invocation that ran is the invocation the document states, and it is the only
    /// check in the archive that can catch a table row that was transcribed wrong.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn text(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<&str>>()
            .join(" ")
    }
}

/// How one command ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandOutcome {
    /// The process was started and exited with this status.
    ///
    /// `None` is a process a signal killed. That is a failure and not an unknown: the process
    /// ran, and it did not finish successfully, and reading it as "could not be judged" would
    /// turn a crashed tool into a withheld verdict.
    Exited {
        /// The exit code the operating system reported, or `None` when a signal ended it.
        code: Option<i32>,
    },
    /// The process was never started.
    NotStarted {
        /// Why, in one sentence a reader can act on.
        reason: String,
    },
}

impl CommandOutcome {
    /// Whether the command ran and finished successfully.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Exited { code: Some(0) })
    }

    /// Whether the command ran and did not finish successfully.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_failure(&self) -> bool {
        matches!(self, Self::Exited { .. }) && !self.is_success()
    }

    /// Whether the command never ran.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_not_started(&self) -> bool {
        matches!(self, Self::NotStarted { .. })
    }

    /// The outcome as the evidence records it, beside the expected `0`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn code_text(&self) -> String {
        match self {
            Self::Exited { code: Some(code) } => code.to_string(),
            Self::Exited { code: None } => String::from("a signal"),
            Self::NotStarted { .. } => String::from("not started"),
        }
    }

    /// The outcome as one sentence, for the console block of a case that did not pass.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn describe(&self) -> String {
        match self {
            Self::Exited { code: Some(code) } => format!("exited with {code}"),
            Self::Exited { code: None } => String::from("was killed by a signal"),
            Self::NotStarted { reason } => format!("could not be started: {reason}"),
        }
    }
}

/// What running one command line produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandRun {
    /// The line the case document states, verbatim.
    pub declared: String,
    /// The invocation that was executed, rebuilt from the program and the arguments.
    pub executed: String,
    /// How the process ended.
    pub outcome: CommandOutcome,
    /// How long the process took, in milliseconds.
    pub duration_ms: u64,
    /// The last [`TAIL_LINES`] lines the process wrote to standard output.
    pub stdout_tail: String,
    /// The last [`TAIL_LINES`] lines the process wrote to standard error.
    pub stderr_tail: String,
}

/// Runs one declared command line in `cwd`.
///
/// Never fails and never panics: a line that cannot be split and a program that cannot be
/// started are both outcomes, because the case that declared the line has to record them rather
/// than have them thrown away.
///
/// The whole of the process's output is collected before the tail is taken, which is what
/// [`std::process::Command::output`] does. The commands this table states are build tools whose
/// output is bounded by what they have to say; a command that could print without end is not one
/// a case document names.
///
/// # Panics
///
/// Never.
pub fn execute(declared: &str, cwd: &Path) -> CommandRun {
    let began = Instant::now();
    let line = match CommandLine::parse(declared) {
        Ok(line) => line,
        Err(reason) => return not_started(declared, reason),
    };
    let executed = line.text();
    let output = Command::new(line.program())
        .args(line.args())
        .current_dir(cwd)
        .output();
    match output {
        Ok(output) => CommandRun {
            declared: declared.to_owned(),
            executed,
            outcome: CommandOutcome::Exited {
                code: output.status.code(),
            },
            duration_ms: elapsed_ms(began),
            stdout_tail: tail(&String::from_utf8_lossy(&output.stdout)),
            stderr_tail: tail(&String::from_utf8_lossy(&output.stderr)),
        },
        Err(source) => CommandRun {
            declared: declared.to_owned(),
            executed,
            outcome: CommandOutcome::NotStarted {
                reason: source.to_string(),
            },
            duration_ms: elapsed_ms(began),
            stdout_tail: String::new(),
            stderr_tail: String::new(),
        },
    }
}

/// The last [`TAIL_LINES`] lines of `text`, oldest first.
///
/// # Panics
///
/// Never.
pub fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let kept = lines.len().saturating_sub(TAIL_LINES);
    lines[kept..].join("\n")
}

/// A run of a line that was never started.
///
/// # Panics
///
/// Never.
fn not_started(declared: &str, reason: String) -> CommandRun {
    CommandRun {
        declared: declared.to_owned(),
        executed: String::new(),
        outcome: CommandOutcome::NotStarted { reason },
        duration_ms: 0,
        stdout_tail: String::new(),
        stderr_tail: String::new(),
    }
}

/// How long ago `began` was, in milliseconds, saturating rather than wrapping.
///
/// Shared with the runner, which measures a whole case rather than one command: the two
/// durations differ in what they span and not in how they are read, so there is one conversion.
///
/// # Panics
///
/// Never.
pub fn elapsed_ms(began: Instant) -> u64 {
    u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX)
}
