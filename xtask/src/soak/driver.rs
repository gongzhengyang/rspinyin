//! Driving a soak: the loop that types, samples and records.
//!
//! Responsibility: resolve the process and the window a run is about, connect the
//! injection channel to it, walk the schedule [`Plan::due`] lays out, and write the report
//! whatever happened.
//!
//! # What is a failure and what is an outcome
//!
//! The two are deliberately different, and the split is what makes the CI job's two steps
//! mean something. A run that could not start -- no Fictx5 to sample, no display, no
//! XTEST, a window the server refuses to focus -- is a failure: this function returns an
//! error and writes nothing, because there is no run to describe. A run that started and
//! then went badly -- the plugin crashed, the candidate window took the focus, a stroke
//! could not be delivered -- is an *outcome*: the report is written and this function
//! returns success, because the report is the product and the verdict belongs to
//! [`super::judge`]. A tool that failed its own exit code on a crash would leave the job
//! with no report to read and no budget step to run, which is the opposite of what a soak
//! is for.
//!
//! # Why every stroke takes the guard
//!
//! Losing the keyboard focus is this project's highest-severity defect class, and a soak is
//! the one run long enough to see it happen. Every stroke goes through
//! [`X11Injector::key`], which asks the server for the focus before and after the event, so
//! a candidate window that took the focus stops the run at the keystroke that caused it
//! instead of quietly redirecting the next eight hours of input. The reason travels in the
//! report, because a job that only saw a non-zero exit would not know which defect it hit.
//!
//! # What this module does not do
//!
//! It does not decide whether the addon processed anything: that is the caller's check on
//! the candidate window the run leaves open. It does not judge the numbers it records --
//! [`super::judge`] does, against the budget document -- and it starts no process, sends no
//! signal and opens no socket. The X connection and the `/proc` reads are the whole of what
//! it touches.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use ime_diag::crash::record::FILE_SUFFIX;
use ime_dict::paths::{BaseDirs, Paths};

use crate::testd::TestError;
use crate::testd::input::{FocusGuard, X11Injector};
use crate::testd::keys::KeyStroke;
use crate::testd::memory::{MemoryError, MemoryMonitor};
use crate::testd::x11::Window;

use super::SoakError;
use super::plan::{Plan, Step};
use super::report::{
    CrashCount, CycleReport, Host, NOTES, PlanReport, REPORT_VERSION, Sample, SoakReport,
    StopReason, Target,
};
use super::script;
use super::stats::RssStats;

/// The directory the kernel exposes the process table under.
const PROC: &str = "/proc";

/// The symbolic link in a process directory that names its executable.
const EXE: &str = "exe";

/// The kernel file the load averages are read from.
const LOADAVG: &str = "/proc/loadavg";

/// The name the Fcitx5 daemon's executable carries.
///
/// The addon is a shared library inside that process rather than a process of its own, so
/// the resident set a robustness budget is about is the daemon's.
const FCITX5_PROGRAM: &str = "fcitx5";

/// The largest window id the X server uses for `None` or `PointerRoot`.
///
/// Neither is a window: keys sent to one go to whatever the pointer is over, which no guard
/// can check. A run that resolved the focus to one of them is refused rather than started
/// against a target nobody named.
const NOT_A_WINDOW: Window = 1;

/// Everything a run needs that is not the schedule itself.
#[derive(Debug)]
pub struct DriverArgs<'a> {
    /// The schedule to follow.
    pub plan: Plan,
    /// X display to inject into; `None` uses `$DISPLAY`.
    pub display: Option<&'a str>,
    /// Window that must keep the focus; `None` uses whatever holds it now.
    pub window: Option<Window>,
    /// Process to sample; `None` looks for the only Fcitx5 on the machine.
    pub pid: Option<u32>,
    /// Device pixel ratio the candidate window is rasterised with.
    pub scale: f32,
    /// Where the report is written.
    pub report: &'a Path,
}

/// Runs the soak `args` describes and writes its report.
///
/// # Errors
///
/// Returns an error when no process can be resolved to sample, when the display cannot be
/// opened or has no XTEST extension, when the window to type into cannot be resolved or
/// focused, when the stroke cycle is empty or does not return the session to the phase it
/// entered it in, and when the report cannot be written. A run that started and then went
/// badly is *not* an error: it writes its report and returns success, and the verdict is the
/// budget gate's.
pub fn run(args: DriverArgs<'_>) -> Result<()> {
    let pid = resolve_pid(args.pid)?;
    let target_pid = Executable::of(pid)?;
    let injector = X11Injector::connect(args.display, args.scale)?;
    let window = resolve_window(&injector, args.window)?;
    let strokes = script::compiled()?;
    if strokes.is_empty() {
        return Err(SoakError::EmptyScript.into());
    }
    // A cycle the run repeats thousands of times has to close. One that does not accumulates
    // state across passes, so what the resident set grew by over the run would be the
    // driver's own growth rather than the plugin's and the report would describe the
    // harness. Refused before the first key goes out: there is no run worth describing.
    let walk = script::walk();
    if !walk.is_closed() {
        return Err(SoakError::CycleNotClosed {
            from: walk.from,
            to: walk.to(),
        }
        .into());
    }
    let guard = injector.focus(window)?;

    let mut run = Run::start(Target {
        pid,
        executable: target_pid.describe(),
        display: injector.display().to_owned(),
        window,
    });
    run.execute(&args.plan, &injector, &guard, &strokes);
    run.probe(&injector, &guard);
    let report = run.report(&args.plan, strokes.len());
    report.write(args.report)?;
    let stats = RssStats::of(&report.samples, args.plan.warmup_s());
    for line in report.lines(stats.as_ref()) {
        println!("soak: {line}");
    }
    println!("soak: report written to {}", args.report.display());
    Ok(())
}

/// One soak run, as it accumulates.
struct Run {
    /// The process and window the run drives.
    target: Target,
    /// The process's counters, followed over the run.
    monitor: MemoryMonitor,
    /// The plugin's crash-record directory, as the run found it.
    crashes: Option<CrashWatch>,
    /// Processors the machine reported when the run started.
    cpus: u32,
    /// The one-minute load average when the run started, or absent when it could not be read.
    load1_at_start: Option<f64>,
    /// When the run started, on the monotonic clock the sample offsets are measured on.
    started: Instant,
    /// When the run started, as a Unix timestamp, for correlating the report with a job.
    started_at_unix_s: u64,
    /// How many strokes have gone out.
    delivered_strokes: u64,
    /// Every reading taken, in order.
    samples: Vec<Sample>,
    /// Why the run stopped, when it stopped before its plan was exhausted.
    stopped_early: Option<StopReason>,
}

impl Run {
    /// Starts a run against `target`, taking the reading the series begins with.
    fn start(target: Target) -> Self {
        let pid = target.pid;
        // Everything that reads the filesystem is done before the clock starts, so that the
        // instant the series is measured from is the instant of the first sample rather than
        // the end of a directory walk.
        let crashes = CrashWatch::capture();
        let cpus = host_cpus();
        let load1_at_start = load1();
        let mut run = Self {
            monitor: MemoryMonitor::new(pid),
            target,
            crashes,
            cpus,
            load1_at_start,
            started: Instant::now(),
            started_at_unix_s: now_unix_s(),
            delivered_strokes: 0,
            samples: Vec::new(),
            stopped_early: None,
        };
        // The first reading is taken at the instant the run starts, which is the state
        // before the first key: the series then begins before anything happened, and the
        // loop below picks up from the second sample onwards.
        run.take_sample();
        run
    }

    /// Types and samples until the plan is exhausted, or until something stops the run.
    ///
    /// # Panics
    ///
    /// Never: `strokes` is non-empty (the caller refuses an empty cycle) and the index is
    /// taken modulo its length, so the slice access is in range by construction.
    fn execute(
        &mut self,
        plan: &Plan,
        injector: &X11Injector,
        guard: &FocusGuard,
        strokes: &[KeyStroke],
    ) {
        let mut sent = 0_u64;
        let mut taken = 1_u64;
        while let Some((step, at)) = plan.due(sent, taken) {
            wait_until(self.started, at);
            match step {
                Step::Stroke => {
                    let stroke = strokes[sent as usize % strokes.len()];
                    if let Err(error) = injector.key(guard, stroke.keysym, stroke.state) {
                        self.stopped_early = Some(stop_for(&error));
                        return;
                    }
                    sent += 1;
                    self.delivered_strokes = sent;
                }
                Step::Sample => {
                    taken += 1;
                    self.take_sample();
                    if self.stopped_early.is_some() {
                        return;
                    }
                }
            }
        }
    }

    /// Leaves a composition open so the caller can observe the candidate window.
    ///
    /// A failure here is recorded rather than returned: the report is what the caller came
    /// for, and the reason the last two keys did not arrive is more useful inside it than as
    /// an error that discards everything the run measured.
    fn probe(&mut self, injector: &X11Injector, guard: &FocusGuard) {
        if self.stopped_early.is_some() {
            return;
        }
        let injected = script::strokes_of(script::OPEN_PROBE).and_then(|probe| {
            probe
                .iter()
                .try_for_each(|stroke| injector.key(guard, stroke.keysym, stroke.state))
        });
        if let Err(error) = injected {
            self.stopped_early = Some(stop_for(&error));
        }
    }

    /// Reads the process's counters once, recording what the reading came to.
    fn take_sample(&mut self) {
        let t_s = self.started.elapsed().as_secs_f64();
        match self.monitor.sample() {
            Ok(sample) => self.samples.push(Sample {
                t_s,
                rss_kb: Some(sample.vm_rss_kb),
                anonymous_kb: sample.basis.has_rollup().then_some(sample.anonymous_kb),
            }),
            Err(MemoryError::ProcUnreadable { source, .. })
                if source.kind() == ErrorKind::NotFound =>
            {
                self.samples.push(Sample {
                    t_s,
                    rss_kb: None,
                    anonymous_kb: None,
                });
                self.stopped_early = Some(StopReason::process_died(format!(
                    "process {} ({}) exited during the run",
                    self.target.pid, self.target.executable
                )));
            }
            Err(error) => {
                self.samples.push(Sample {
                    t_s,
                    rss_kb: None,
                    anonymous_kb: None,
                });
                self.stopped_early = Some(StopReason::sampling_failed(error.to_string()));
            }
        }
    }

    /// The report of everything the run observed.
    ///
    /// `strokes_per_cycle` is the length of the cycle the run repeated, which is what turns
    /// the delivered stroke count into whole passes over the state loop.
    fn report(&self, plan: &Plan, strokes_per_cycle: usize) -> SoakReport {
        let per_cycle = u64::try_from(strokes_per_cycle).unwrap_or(u64::MAX);
        SoakReport {
            report_version: REPORT_VERSION,
            plan: PlanReport::of(plan),
            target: self.target.clone(),
            host: Host {
                cpus: self.cpus,
                load1_at_start: self.load1_at_start,
                load1_at_end: load1(),
            },
            started_at_unix_s: self.started_at_unix_s,
            elapsed_s: self.started.elapsed().as_secs_f64(),
            delivered_strokes: self.delivered_strokes,
            cycles: CycleReport::of(plan, per_cycle, self.delivered_strokes),
            crashes: self.crashes.as_ref().map(CrashWatch::report),
            stopped_early: self.stopped_early.clone(),
            samples: self.samples.clone(),
            notes: NOTES.iter().map(|note| (*note).to_owned()).collect(),
        }
    }
}

/// The plugin's crash-record directory, as a run found it.
///
/// The directory is resolved through the plugin's own layout rather than assembled here, so
/// that the directory the plugin writes and the directory this counts are one path by
/// construction. The count is the plugin's and not this process's: a panic inside the addon
/// is caught at the C ABI boundary and written to a record rather than killing the daemon, so
/// a run in which the plugin crashed is a run that stays alive with a flat resident set.
struct CrashWatch {
    /// The directory the records are counted in.
    directory: PathBuf,
    /// How many records it held when the run started.
    before: Option<u64>,
}

impl CrashWatch {
    /// Takes the reading a run starts from.
    ///
    /// `None` when the plugin's layout cannot be resolved at all -- no `$HOME` and no
    /// `$XDG_DATA_HOME` -- which is a reading nobody took rather than a count of zero.
    fn capture() -> Option<Self> {
        let paths = Paths::from_bases(&BaseDirs::from_env().ok()?).ok()?;
        let directory = paths.crash_dir.clone();
        let before = count_records(&directory);
        Some(Self { directory, before })
    }

    /// The count the report states, taken now.
    fn report(&self) -> CrashCount {
        CrashCount {
            directory: self.directory.display().to_string(),
            before: self.before,
            after: count_records(&self.directory),
        }
    }
}

/// How many crash records `dir` holds.
///
/// Only the files a record is written as are counted, and the suffix comes from the module
/// that writes them rather than from a second copy of it, so a stray file in the directory
/// cannot be read as a crash.
///
/// # Return value
///
/// Zero for a directory that is not there, because a plugin that has never crashed has never
/// created one; `None` for a directory that is there and cannot be read, which is a reading
/// nobody took and must not be read as a count of zero.
fn count_records(dir: &Path) -> Option<u64> {
    if fs::symlink_metadata(dir).is_err() {
        return Some(0);
    }
    let entries = fs::read_dir(dir).ok()?;
    Some(
        entries
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.ends_with(FILE_SUFFIX))
            })
            .count() as u64,
    )
}

/// How many processors the machine reports, or one when it cannot say.
///
/// One rather than zero: the number is what the load average is read against, and a zero
/// would make every run look loaded. The count is the one the process is allowed to use, so
/// a cgroup limit is honoured rather than the host's physical count.
fn host_cpus() -> u32 {
    let count = thread::available_parallelism().map_or(1, |count| count.get());
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// The machine's one-minute load average, or `None` when it cannot be read.
///
/// A diagnostic and not a measurement: it is recorded so that a run taken on a busy machine
/// is visibly not comparable with one taken on an idle one, which is the only condition
/// under which the drift it measures means anything. It is printed and never asserted,
/// because the budget document states no ceiling for it.
fn load1() -> Option<f64> {
    let text = fs::read_to_string(LOADAVG).ok()?;
    text.split_whitespace().next()?.parse::<f64>().ok()
}

/// The executable a process is running, as `/proc` names it.
struct Executable(PathBuf);

impl Executable {
    /// The executable of `pid`.
    ///
    /// # Errors
    ///
    /// Returns [`SoakError::NoSuchProcess`] when the process is not there, and an error
    /// naming the path when it is there but its executable cannot be resolved.
    fn of(pid: u32) -> Result<Self> {
        let path = Path::new(PROC).join(pid.to_string()).join(EXE);
        fs::read_link(&path).map(Self).map_err(|source| {
            if source.kind() == ErrorKind::NotFound {
                anyhow::Error::from(SoakError::NoSuchProcess { pid })
            } else {
                anyhow::Error::new(source).context(format!("reading {}", path.display()))
            }
        })
    }

    /// The executable's path, as the report states it.
    fn describe(&self) -> String {
        self.0.display().to_string()
    }
}

/// The process the run samples: the one named, or the only Fcitx5 on the machine.
///
/// # Errors
///
/// Returns [`SoakError::NoProcess`] when nothing named Fcitx5 is running and nothing was
/// named, [`SoakError::AmbiguousProcess`] when several are, and whatever [`Executable::of`]
/// returns for a process that is not there.
fn resolve_pid(explicit: Option<u32>) -> Result<u32> {
    match explicit {
        Some(pid) => {
            Executable::of(pid)?;
            Ok(pid)
        }
        None => Ok(pick_process(&fcitx5_processes())?),
    }
}

/// The one Fcitx5 process, or a refusal naming the candidates.
///
/// Several Fcitx5 processes are possible -- a session that was replaced without being shut
/// down, or a second one under another user -- and sampling the wrong one would produce a
/// report about a process that never saw a keystroke. So a choice is refused rather than
/// guessed.
///
/// # Errors
///
/// Returns [`SoakError::NoProcess`] for an empty list and [`SoakError::AmbiguousProcess`]
/// for a list longer than one.
fn pick_process(found: &[(u32, PathBuf)]) -> Result<u32, SoakError> {
    if let [(pid, _)] = found {
        return Ok(*pid);
    }
    if found.is_empty() {
        return Err(SoakError::NoProcess {
            name: FCITX5_PROGRAM,
        });
    }
    let pids = found
        .iter()
        .map(|(pid, _)| pid.to_string())
        .collect::<Vec<String>>()
        .join(", ");
    Err(SoakError::AmbiguousProcess { pids })
}

/// Every process whose executable is named Fcitx5.
///
/// A process that exits between the listing and the link read is skipped rather than
/// failing the scan: the table is a snapshot, and a pid that disappeared while it was being
/// walked is normal. An unreadable process table comes back empty, which the caller reports
/// as no process found.
fn fcitx5_processes() -> Vec<(u32, PathBuf)> {
    let Ok(entries) = fs::read_dir(PROC) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            let executable = fs::read_link(entry.path().join(EXE)).ok()?;
            let named = executable
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == FCITX5_PROGRAM);
            named.then_some((pid, executable))
        })
        .collect()
}

/// The window the run types into: the one named, or whatever holds the input focus.
///
/// # Errors
///
/// Returns [`SoakError::NoFocusWindow`] when nothing was named and the server's focus is
/// `None` or `PointerRoot`, and whatever the focus query reports when it cannot be answered.
fn resolve_window(injector: &X11Injector, explicit: Option<Window>) -> Result<Window> {
    if let Some(window) = explicit {
        return Ok(window);
    }
    let focus = injector.input_focus()?;
    if focus <= NOT_A_WINDOW {
        return Err(SoakError::NoFocusWindow {
            display: injector.display().to_owned(),
            focus,
        }
        .into());
    }
    Ok(focus)
}

/// Waits until `start + offset`, or returns at once when that instant has passed.
///
/// The instant is computed with a checked addition and the remaining time with a saturating
/// subtraction, so a schedule far enough in the future to overflow -- which no plan this
/// tool builds produces -- skips the wait instead of panicking on it.
fn wait_until(start: Instant, offset: Duration) {
    if let Some(deadline) = start.checked_add(offset) {
        thread::sleep(deadline.saturating_duration_since(Instant::now()));
    }
}

/// The reason an injection refusal stops a run.
fn stop_for(error: &TestError) -> StopReason {
    match error {
        TestError::FocusStolen { .. } => StopReason::focus_stolen(error.to_string()),
        _ => StopReason::injection_failed(error.to_string()),
    }
}

/// The current time as a Unix timestamp in seconds.
///
/// A clock set before 1970 is recorded as zero rather than failing a run that is otherwise
/// fine: the timestamp exists to correlate a report with the job that produced it, and it is
/// not a measurement of anything.
fn now_unix_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The tests of the decisions in this file that can be made without a session.
///
/// They live here rather than beside the rest of the tool's tests because what they drive is
/// this file's own: `pick_process` reads nothing at all -- it is a function of the rows it is
/// handed -- and the readings that do touch the filesystem are about the shapes this file has
/// to tell apart, not about the values a particular machine reports.
#[cfg(test)]
mod tests {
    use super::*;

    /// A row for a process running Fcitx5.
    fn row(pid: u32) -> (u32, PathBuf) {
        (pid, PathBuf::from("/usr/bin/fcitx5"))
    }

    /// A directory under the workspace target tree that a test may write into.
    fn scratch(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/xtask-soak-scratch")
            .join(name)
    }

    #[test]
    fn test_pick_process_takes_the_only_fcitx5_it_is_given() {
        // `matches!` rather than `assert_eq!`: `SoakError` carries no `PartialEq`, and
        // giving it one would put an equality the error model has no use for onto a type
        // whose whole job is to be displayed.
        assert!(matches!(pick_process(&[row(101)]), Ok(101)));
    }

    #[test]
    fn test_pick_process_refuses_an_empty_table_and_a_crowded_one() {
        let refused = pick_process(&[]);
        assert!(
            matches!(refused, Err(SoakError::NoProcess { name: "fcitx5" })),
            "{refused:?}"
        );
        // Two daemons are possible -- a session that was replaced without being shut down,
        // or a second one under another user -- and a report about the wrong one would
        // describe a session that never saw a keystroke.
        let refused = pick_process(&[row(101), row(202)]);
        assert!(
            matches!(&refused, Err(SoakError::AmbiguousProcess { pids }) if pids == "101, 202"),
            "{refused:?}"
        );
    }

    #[test]
    fn test_count_records_counts_only_the_files_a_record_is_written_as() -> Result<()> {
        // A directory that is not there is a plugin that has never crashed, which is a
        // measured zero -- the one case in which zero is a reading rather than a guess.
        assert_eq!(count_records(&scratch("no-such-run").join("crash")), Some(0));

        let dir = scratch("count-records");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join(format!("one{FILE_SUFFIX}")), b"a record")?;
        fs::write(dir.join(format!("two{FILE_SUFFIX}")), b"another record")?;
        // The suffix comes from the module that writes the records, so a file that is not
        // one cannot be read as a crash.
        fs::write(dir.join("notes.md"), b"not a record")?;
        fs::write(dir.join("crash"), b"nor is a file with no suffix")?;
        assert_eq!(count_records(&dir), Some(2));
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn test_host_readings_are_absent_or_sane_and_never_a_guess() {
        // Both are diagnostics and neither is asserted, so what a test can hold them to is
        // the shape: a load average that is there is a finite non-negative number, and the
        // processor count is at least one so that a load read against it cannot look loaded
        // on an idle machine.
        assert!(load1().is_none_or(|load| load.is_finite() && load >= 0.0));
        assert!(host_cpus() >= 1);
    }
}
