//! The command-line surface of `xtask testd-engine`.
//!
//! This is the one file of the harness that touches the filesystem: it reads the
//! scenario files a caller names, writes the built-in set out, and starts the child
//! process the purity check is traced through. The drive path itself opens no file, which
//! is what lets the unit tests assert that property over its source.
//!
//! Nothing here reads a clock either. A scenario is replayed as fast as the engine
//! answers, and the only number the output carries is a count of runs.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use anyhow::{Context, Result, bail, ensure};
use clap::Args;

use crate::budget::repo_root;
use crate::testd::engine::REPEAT_RUNS;
use crate::testd::engine::assert_repeatable;
use crate::testd::engine::run_scenario;
use crate::testd::engine::scenario::{EngineFixture, parse_scenario};
use crate::testd::engine::scenarios;

/// Program the purity check starts the child under.
const STRACE_BINARY: &str = "strace";

/// The syscalls the purity check records.
///
/// The open family and the clock, which are the two things the drive path may not use: a
/// file it opened would be a dictionary or a scenario it read, and a clock it read would
/// be a duration it measured. The card that asks for the check names `openat` and
/// `clock_gettime`; the two further spellings of the same two capabilities are traced
/// because a call that arrived through either of them would be the same defect.
const TRACED_SYSCALLS: &str = "openat,open,openat2,clock_gettime";

/// The subcommand the child process is started with.
const ENGINE_SUBCOMMAND: &str = "testd-engine";

/// How many offending trace lines a refusal quotes before it counts the rest.
///
/// A refusal has to be readable, and a drive path that opened files would produce
/// hundreds of lines that all say the same thing.
const MAX_REPORTED_LINES: usize = 10;

/// Command-line surface of `xtask testd-engine`.
#[derive(Debug, Args)]
pub struct EngineArgs {
    /// Directory of TOML scenario files; every `*.toml` in it is loaded.
    ///
    /// Without it the built-in scenario set runs, which is what makes the channel usable
    /// with no fixture files on disk at all.
    #[arg(long, value_name = "DIR", conflicts_with = "strace")]
    pub scenarios: Option<PathBuf>,
    /// Replay every scenario this many times and compare the candidate sequences.
    ///
    /// One is the scenario check without a comparison; the default is the repeat count
    /// the determinism contract is stated at.
    #[arg(long, default_value_t = REPEAT_RUNS)]
    pub repeat: usize,
    /// Write the built-in scenario set to stdout as TOML and exit.
    ///
    /// The built-in set is what a `tests/fixtures/scenarios/` directory should hold, so
    /// this is how the fixture files are materialised without anyone transcribing them.
    #[arg(long, conflicts_with = "strace")]
    pub dump: bool,
    /// Run the built-in set in a child process under `strace` and assert what a source
    /// scan cannot settle.
    ///
    /// Two of the acceptance criteria are dynamic: that the drive path opens no file and
    /// reads no clock, and that the whole set passes with no display environment. Both
    /// are assertions about a run rather than about the source, so the check makes one:
    /// this binary is started again as a child with `DISPLAY`, `WAYLAND_DISPLAY` and
    /// `LD_LIBRARY_PATH` removed, under a trace of the open family and the clock, and the
    /// trace is refused if it names either.
    ///
    /// Needs `strace` on the `PATH`. It refuses `--scenarios` because a run that read a
    /// scenario file is a run that opens a file on purpose.
    #[arg(long)]
    pub strace: bool,
}

/// Entry point for `xtask testd-engine`.
///
/// # Returns
///
/// `Ok(())` when every scenario ran, every step observed what it expected and every
/// replay produced the same candidate sequence.
///
/// # Errors
///
/// Returns an error when a scenario file cannot be read or parsed, when the built-in set
/// describes a dictionary the doubles cannot build, and when any scenario diverged --
/// the divergences are printed as they are found and the run ends with a non-zero status.
/// With `--strace` it also returns an error when the child cannot be started or when its
/// trace names a syscall the drive path may not make.
///
/// # Panics
///
/// Never panics.
pub fn run(args: EngineArgs) -> Result<()> {
    if args.dump {
        return dump_builtin();
    }
    if args.strace {
        return check_purity(args.repeat.max(1));
    }
    let runs = args.repeat.max(1);
    let fixtures = load(args.scenarios.as_deref())?;
    let mut failed = 0usize;
    for fixture in &fixtures {
        // The single run comes first, so that a scenario which diverges is reported as a
        // scenario failure rather than as a replay that drifted: the two are different
        // defects and a reader has to be able to tell them apart.
        let outcome = run_scenario(
            &fixture.scenario,
            &fixture.lexicon,
            &fixture.user_freq,
            &fixture.lm,
        )
        .and_then(|()| assert_repeatable(&fixture.scenario, &fixture.sources(), runs));
        match outcome {
            Ok(()) => println!(
                "{}: ok ({} steps x {runs} runs)",
                fixture.scenario.name,
                fixture.scenario.steps.len()
            ),
            Err(divergence) => {
                failed += 1;
                eprintln!(
                    "{}: DIVERGED\n{}",
                    fixture.scenario.name,
                    divergence.describe()
                );
            }
        }
    }
    println!("{}", summary(fixtures.len(), failed));
    if failed > 0 {
        bail!("{failed} scenario(s) diverged");
    }
    Ok(())
}

/// The line a run ends with.
///
/// The channel's boundary is part of the line rather than only of this file's
/// documentation, because the line is what a report quotes. A result from here was
/// produced in-process, with no fcitx5, no display server and no dictionary file, so it
/// is a statement about the engine's contract and nothing else: it says nothing about the
/// route a real keystroke travels, which is the injection channel's business.
///
/// # Panics
///
/// Never panics.
fn summary(scenarios: usize, failed: usize) -> String {
    format!(
        "testd-engine: {scenarios} scenarios, {failed} diverged (engine direct drive: \
         in-process, no fcitx5, no display server, no dictionary file -- not an \
         end-to-end result)"
    )
}

/// Runs the built-in set in a child process and asserts what only a dynamic run can.
///
/// # Parameters
///
/// - `repeat`: how many times the child replays each scenario.
///
/// # Returns
///
/// `Ok(())` when the child ran every scenario without a divergence and its trace names
/// no forbidden syscall.
///
/// # Errors
///
/// Returns an error when this binary's own path cannot be resolved, when `strace` cannot
/// be started -- which includes its not being installed -- when the child did not exit
/// successfully, and when the trace names a forbidden syscall. The offending trace lines
/// are part of that last message, so a reader can tell a real defect from a line the
/// process start-up produced.
///
/// # Panics
///
/// Never panics.
fn check_purity(repeat: usize) -> Result<()> {
    let trace = trace_path();
    let status = run_traced(&trace, repeat)?;
    let text = std::fs::read_to_string(&trace)
        .with_context(|| format!("reading the trace {}", trace.display()))?;
    remove_trace(&trace);
    ensure!(status.success(), "the child run exited with {status}");
    let forbidden = forbidden_calls(&text, &repo_root()?);
    if !forbidden.is_empty() {
        bail!("{}", forbidden_report(&forbidden));
    }
    println!(
        "testd-engine: the drive path opened no file of this repository and read no clock, \
         with DISPLAY and WAYLAND_DISPLAY unset"
    );
    Ok(())
}

/// Starts this binary's built-in scenario set under `strace`, with no display environment.
///
/// The child is this same binary, so what the trace describes is the shipped drive path
/// and not a copy of it. `LD_LIBRARY_PATH` is dropped as well: it is what `cargo run`
/// hands a child to point the dynamic loader at `target/`, and a loader search that
/// reached into this repository would be reported as a file the drive path opened.
///
/// # Errors
///
/// Returns an error when the child cannot be started, which is where a missing `strace`
/// is reported.
///
/// # Panics
///
/// Never panics.
fn run_traced(trace: &Path, repeat: usize) -> Result<ExitStatus> {
    let exe = std::env::current_exe().context("locating this binary")?;
    Command::new(STRACE_BINARY)
        .arg("-f")
        .arg("-e")
        .arg(format!("trace={TRACED_SYSCALLS}"))
        .arg("-o")
        .arg(trace)
        .arg(&exe)
        .arg(ENGINE_SUBCOMMAND)
        .arg("--repeat")
        .arg(repeat.to_string())
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("LD_LIBRARY_PATH")
        .status()
        .with_context(|| format!("starting {STRACE_BINARY}; install the `strace` package"))
}

/// The file the trace is written to.
///
/// Named after this process, so that two checks running at once cannot overwrite each
/// other's trace. The file is written by `strace` rather than here, which is why it is
/// not created up front.
///
/// # Panics
///
/// Never panics.
fn trace_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "rspinyin-testd-engine-{}.strace",
        std::process::id()
    ))
}

/// Removes the trace file, reporting a failure to remove it rather than swallowing it.
///
/// A leftover file does not change the verdict, so it is not an error; it is still worth
/// a line, because a directory that keeps growing with one trace per run is something
/// somebody has to notice.
///
/// # Panics
///
/// Never panics.
fn remove_trace(trace: &Path) {
    if let Err(error) = std::fs::remove_file(trace) {
        eprintln!(
            "testd-engine: the trace {} was left behind: {error}",
            trace.display()
        );
    }
}

/// The trace lines that name a syscall the drive path may not make.
///
/// # Parameters
///
/// - `trace`: the text `strace -o` wrote.
/// - `root`: the repository root; an open of a path below it is what a dictionary, a
///   scenario file or a configuration document would be.
///
/// # Returns
///
/// One entry per offending line, verbatim and in trace order, empty when the drive path
/// made none. The lines are returned rather than counted because a refusal has to let a
/// reader attribute every one of them.
///
/// # Panics
///
/// Never panics.
fn forbidden_calls(trace: &str, root: &Path) -> Vec<String> {
    trace
        .lines()
        .filter(|line| is_forbidden(line, root))
        .map(str::to_owned)
        .collect()
}

/// Whether one trace line names a syscall the drive path may not make.
///
/// A clock is forbidden wherever it appears. An open is forbidden when it names a path
/// below the repository root and did not fail: a search that missed leaves nothing open,
/// so it is not a file anything read, and a dynamic loader that searches a directory of
/// the repository on its way to `libc` is exactly that.
///
/// # Panics
///
/// Never panics.
fn is_forbidden(line: &str, root: &Path) -> bool {
    match syscall_of(line) {
        Some("clock_gettime") => true,
        Some("openat" | "open" | "openat2") => {
            opened_path(line).is_some_and(|path| !failed(line) && Path::new(path).starts_with(root))
        }
        _ => false,
    }
}

/// The syscall name of one trace line, or `None` when the line is not a syscall.
///
/// The name is the last word in front of the argument list, which is what makes the read
/// independent of whether `strace` prefixed the line with a process id, and of whether it
/// was a line about a signal or a process rather than a call.
///
/// # Panics
///
/// Never panics.
fn syscall_of(line: &str) -> Option<&str> {
    let open = line.find('(')?;
    line.get(..open)?.split_whitespace().next_back()
}

/// The first quoted string of one trace line, which is the path an open named.
///
/// # Panics
///
/// Never panics.
fn opened_path(line: &str) -> Option<&str> {
    line.split('"').nth(1)
}

/// Whether one trace line reports a call that failed.
///
/// # Panics
///
/// Never panics.
fn failed(line: &str) -> bool {
    line.contains("= -1")
}

/// The refusal a trace with forbidden calls is reported with.
///
/// # Panics
///
/// Never panics.
fn forbidden_report(forbidden: &[String]) -> String {
    let shown: Vec<&str> = forbidden
        .iter()
        .take(MAX_REPORTED_LINES)
        .map(String::as_str)
        .collect();
    let rest = forbidden.len().saturating_sub(shown.len());
    let mut lines = vec![
        format!(
            "the drive path made {} syscall(s) it may not make, so this harness is not pure:",
            forbidden.len()
        ),
        format!("  {}", shown.join("\n  ")),
    ];
    if rest > 0 {
        lines.push(format!("  ... and {rest} more"));
    }
    lines.join("\n")
}

/// Loads the fixtures the arguments name.
///
/// # Errors
///
/// As [`load_directory`] and [`load_builtin`].
///
/// # Panics
///
/// Never panics.
fn load(directory: Option<&Path>) -> Result<Vec<EngineFixture>> {
    match directory {
        Some(directory) => load_directory(directory),
        None => load_builtin(),
    }
}

/// Builds the fixtures of the built-in scenario set.
///
/// # Errors
///
/// Returns an error when a built-in scenario describes a dictionary the doubles cannot
/// build, which is a defect in this file rather than a scenario outcome.
///
/// # Panics
///
/// Never panics.
fn load_builtin() -> Result<Vec<EngineFixture>> {
    scenarios()
        .into_iter()
        .map(|scenario| {
            let name = scenario.name.clone();
            EngineFixture::new(scenario).with_context(|| format!("building {name}"))
        })
        .collect()
}

/// Loads every `*.toml` scenario file a directory holds, in name order.
///
/// # Errors
///
/// Returns an error when the directory cannot be read, when it holds no scenario file,
/// when a file cannot be read or parsed, or when a file describes a dictionary the
/// doubles cannot build.
///
/// # Panics
///
/// Never panics.
fn load_directory(directory: &Path) -> Result<Vec<EngineFixture>> {
    let shown = directory.display();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(directory)
        .with_context(|| format!("reading {shown}"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<PathBuf>>>()
        .with_context(|| format!("reading {shown}"))?
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .collect();
    // Sorted so that a directory runs in the same order on every machine, whatever the
    // filesystem hands back.
    paths.sort();
    ensure!(!paths.is_empty(), "{shown}: no *.toml scenario files");
    paths.into_iter().map(|path| fixture_at(&path)).collect()
}

/// Builds the fixture one scenario file describes.
///
/// # Errors
///
/// Returns an error when the file cannot be read or parsed, or when its dictionary
/// cannot be built.
///
/// # Panics
///
/// Never panics.
fn fixture_at(path: &Path) -> Result<EngineFixture> {
    let shown = path.display();
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {shown}"))?;
    let scenario = parse_scenario(&text).with_context(|| format!("parsing {shown}"))?;
    EngineFixture::new(scenario).with_context(|| format!("building {shown}"))
}

/// Writes the built-in scenario set to stdout as TOML.
///
/// # Errors
///
/// Returns an error when a scenario cannot be rendered.
///
/// # Panics
///
/// Never panics.
fn dump_builtin() -> Result<()> {
    for scenario in scenarios() {
        let text = toml::to_string_pretty(&scenario)
            .with_context(|| format!("rendering {}", scenario.name))?;
        println!("# --- scenarios/{}.toml ---\n{text}", scenario.name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Tests for the command-line surface: the line a run ends with, the trace filter the
    //! purity check decides with, and the three ways a run can be started.

    use super::*;

    /// The arguments of a run that uses the built-in set.
    fn builtin_args() -> EngineArgs {
        EngineArgs {
            scenarios: None,
            repeat: 1,
            dump: false,
            strace: false,
        }
    }

    /// A trace of a process that opened files of the repository and read a clock.
    ///
    /// The two opens below the repository root are the ones the check exists for: the
    /// compiled dictionary and a scenario file a caller named. The loader's own read of
    /// `/etc/ld.so.cache` is not a finding, and the fourth line is what the clock half of
    /// the check refuses.
    const DIRTY_TRACE: &str = concat!(
        "12345 openat(AT_FDCWD, \"/etc/ld.so.cache\", O_RDONLY|O_CLOEXEC) = 3\n",
        "12345 openat(AT_FDCWD, \"/repo/data/compiled/base.dict\", O_RDONLY) = 4\n",
        "12345 openat(AT_FDCWD, \"/repo/tests/fixtures/scenarios/x.toml\", O_RDONLY) = 5\n",
        "12345 clock_gettime(CLOCK_MONOTONIC, {tv_sec=1, tv_nsec=2}) = 0\n",
    );

    #[test]
    fn test_summary_names_the_channel_and_its_boundary() {
        let line = summary(13, 0);
        assert!(line.contains("13"), "{line}");
        assert!(line.contains("testd-engine"), "{line}");
        assert!(
            line.contains("not an end-to-end result"),
            "the line a report quotes must carry the boundary: {line}"
        );
        assert!(line.contains("no fcitx5"), "{line}");
        assert!(line.contains("no display server"), "{line}");

        let failed = summary(13, 2);
        assert!(failed.contains("2 diverged"), "{failed}");
    }

    #[test]
    fn test_forbidden_calls_names_every_repository_file_that_was_opened() {
        let forbidden = forbidden_calls(DIRTY_TRACE, Path::new("/repo"));
        assert_eq!(forbidden.len(), 3, "{forbidden:?}");
        assert!(forbidden[0].contains("base.dict"), "{forbidden:?}");
        assert!(forbidden[1].contains("x.toml"), "{forbidden:?}");
        assert!(forbidden[2].contains("clock_gettime"), "{forbidden:?}");
    }

    #[test]
    fn test_forbidden_calls_reads_a_clock_as_a_finding() {
        let trace = "12345 clock_gettime(CLOCK_MONOTONIC, {tv_sec=1, tv_nsec=2}) = 0\n";
        let forbidden = forbidden_calls(trace, Path::new("/repo"));
        assert_eq!(forbidden.len(), 1, "{forbidden:?}");
        assert!(forbidden[0].contains("clock_gettime"), "{forbidden:?}");
    }

    #[test]
    fn test_forbidden_calls_ignores_the_loaders_own_file_reads() {
        // Everything a process start-up opens is outside the repository, and a search
        // that missed leaves nothing open at all. Neither is a file the drive path read.
        let trace = concat!(
            "12345 execve(\"/repo/target/debug/xtask\", [], []) = 0\n",
            "12345 openat(AT_FDCWD, \"/etc/ld.so.cache\", O_RDONLY|O_CLOEXEC) = 3\n",
            "12345 openat(AT_FDCWD, \"/lib/x86_64-linux-gnu/libc.so.6\", O_RDONLY) = 3\n",
            "12345 openat(AT_FDCWD, \"/repo/target/debug/deps/libgcc_s.so.1\", \
             O_RDONLY) = -1 ENOENT (No such file or directory)\n",
            "12345 +++ exited with 0 +++\n",
            "strace: Process 12345 attached\n",
        );
        assert!(
            forbidden_calls(trace, Path::new("/repo")).is_empty(),
            "none of these is a file the drive path read"
        );
    }

    #[test]
    fn test_forbidden_calls_ignores_a_repository_with_a_similar_name() {
        // The filter is a path-prefix test, not a string-prefix test: `/repo-other` is a
        // different directory from `/repo`.
        let trace = "12345 openat(AT_FDCWD, \"/repo-other/base.dict\", O_RDONLY) = 3\n";
        assert!(forbidden_calls(trace, Path::new("/repo")).is_empty());
    }

    #[test]
    fn test_forbidden_report_quotes_the_lines_and_counts_the_rest() {
        let forbidden: Vec<String> = (0..MAX_REPORTED_LINES + 3)
            .map(|index| format!("line {index}"))
            .collect();
        let report = forbidden_report(&forbidden);
        assert!(report.contains(&forbidden.len().to_string()), "{report}");
        assert!(report.contains("line 0"), "{report}");
        assert!(!report.contains("line 10"), "{report}");
        assert!(report.contains("and 3 more"), "{report}");
    }

    #[test]
    fn test_the_trace_is_filtered_against_the_workspace_root() {
        let root = repo_root().expect("xtask lives below the repository root");
        // What the trace is filtered against has to be the workspace root and not the
        // crate directory: a path below `xtask/` is below the repository either way, but
        // a dictionary under `data/` is only below the workspace root.
        assert!(root.join("Cargo.toml").is_file(), "{root:?}");
        assert_eq!(
            root.join("xtask"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        );
    }

    #[test]
    fn test_run_without_scenarios_runs_the_builtin_set() {
        let outcome = run(builtin_args());
        assert!(outcome.is_ok(), "{outcome:?}");
    }

    #[test]
    fn test_run_dump_writes_the_builtin_set() {
        let args = EngineArgs {
            dump: true,
            ..builtin_args()
        };
        assert!(run(args).is_ok());
    }

    #[test]
    fn test_run_of_a_missing_scenario_directory_fails() {
        let missing = std::env::temp_dir()
            .join("rspinyin-testd-engine-no-such-directory")
            .join("nested");
        let args = EngineArgs {
            scenarios: Some(missing),
            ..builtin_args()
        };
        let outcome = run(args);
        assert!(
            outcome.is_err(),
            "a directory that is not there is an error"
        );
    }
}
