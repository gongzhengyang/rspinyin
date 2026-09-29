//! Performance-measurement purity: whether a number was taken on a machine quiet enough to
//! be believed.
//!
//! Responsibility: decide whether the machine a performance measurement was taken on was
//! quiet enough for the number to mean anything, and refuse -- not warn -- when it was not.
//! A refused number is not reported, and a baseline is not frozen from it either: a baseline
//! taken on a busy machine is the false regression every later run is measured against.
//!
//! The ledger records what that costs: with five agents saturating the CPU,
//! `passthrough/classify` measured 511ns, and minutes later on an idle machine the same case
//! measured 726ns, which criterion reported as a regression of forty percent. The baseline had
//! been taken on a busy machine, and every later run was divided by it -- which is why a
//! warning at the point of measurement would have been scrolled past, and this module refuses.
//!
//! [`PurityVerdict`] is `Clean`, `Dirty` with its reasons, or `Undeterminable` with the facts
//! that could not be read, and only `Clean` lets a measurement through.
//! [`PurityReport::judge`] reads nothing -- no clock, no `/proc`, no environment -- so every
//! rule is a case a test can run on any host, and [`MachineFacts::read`] is the thin layer
//! that fills the record in.
//!
//! Two facts about a machine are annotations rather than criteria, because a number stays
//! comparable with a baseline frozen in the same state whatever that state is.
//! [`PurityReport::governor_note`] says so about the frequency-scaling governor, and
//! [`PurityReport::accept_samples`] refuses a case whose own samples disagree with each other
//! past the caller's tolerance: a spread that wide is a reading of the machine's noise as much
//! as of the code, and it is refused exactly as a busy machine is.
//!
//! Boundaries: it reads `/proc` and `/sys` and lists the process table; it starts no process,
//! sends no signal, opens no socket, and writes nothing of its own -- the one document a
//! measurement leaves behind is the frozen record in [`baseline`]. The tests live in a sibling
//! file, because together they would be longer than the file limit allows.

// Nothing in the crate names this module yet: the case runner that would sample it, the
// benchmark-budget gate that would call `accept`, and the subcommand that would print the
// report all live in files this one does not own. Until that wiring lands, every item here is
// reported as dead code in a non-test build, and the attribute goes away with those lines.
#![allow(dead_code)]

use std::fs;
use std::path::Path;

pub mod baseline;

#[cfg(test)]
mod tests;

/// The root every path below is resolved against: `/` here, or a directory a test wrote.
const ROOT: &str = "/";
/// The directory the kernel exposes the process table under, below [`ROOT`].
const PROC_DIR: &str = "proc";
/// The file the three load averages are read from, below [`ROOT`].
const LOADAVG_FILE: &str = "proc/loadavg";
/// The file the processors are listed in, below [`ROOT`].
const CPUINFO_FILE: &str = "proc/cpuinfo";
/// The file the frequency-scaling governor of the first processor is read from.
const GOVERNOR_FILE: &str = "sys/devices/system/cpu/cpu0/cpufreq/scaling_governor";
/// The file in a process directory that carries the process name and its parent.
const STATUS_FILE: &str = "status";
const NAME_FIELD: &str = "Name:";
const PARENT_FIELD: &str = "PPid:";
/// The key of the `cpuinfo` line that introduces one processor.
const PROCESSOR_FIELD: &str = "processor";
/// The key of the `cpuinfo` line that names the processor model.
const MODEL_FIELD: &str = "model name";
/// The governor a machine measured for a release figure is expected to run.
const PERFORMANCE_GOVERNOR: &str = "performance";

/// The programs whose presence means a build or a test run is in progress.
///
/// The environment probe keeps the same list by hand for its `concurrent_agents` field,
/// because that probe's `process` module is private to `env`.
pub const AGENT_PROGRAMS: [&str; 4] = ["cargo", "cargo-nextest", "nextest", "rustc"];

/// Why a machine is too busy for a measurement taken on it to be believed.
#[derive(Clone, Debug, PartialEq)]
pub enum Impurity {
    /// Build or test processes are running outside this probe's own ancestry.
    ConcurrentBuilds {
        /// How many were counted.
        count: u32,
    },
    /// The one-minute load average is at or past the ceiling the policy derives.
    Load {
        /// The one-minute load average that was read.
        load_1m: f64,
        /// The number of processors the ceiling was derived from.
        cpu_count: u32,
        /// The ceiling itself: the processor count times the policy's factor.
        ceiling: f64,
    },
}

impl Impurity {
    /// The reason as one sentence, for the refusal a caller prints.
    ///
    /// The build count is spelled out: the number tells a reader whether one background job or
    /// a whole parallel wave is in the way.
    ///
    /// # Panics
    /// Never.
    pub fn text(&self) -> String {
        match self {
            Self::ConcurrentBuilds { count } => format!(
                "{count} build or test process(es) are running outside this probe's own \
                 ancestry ({}), so the CPU is shared with work that is not being measured",
                AGENT_PROGRAMS.join(", ")
            ),
            Self::Load {
                load_1m,
                cpu_count,
                ceiling,
            } => format!(
                "the one-minute load average is {load_1m:.2} on {cpu_count} processors, at or \
                 past the {ceiling:.2} ceiling"
            ),
        }
    }
}

/// A fact the verdict needed and could not read: a gap refuses the measurement exactly as a
/// reason does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PurityGap {
    /// The process table could not be listed, so the number of concurrent builds is unknown.
    ProcessTableUnreadable,
    /// The one-minute load average could not be read.
    LoadUnreadable,
    /// The processor count could not be read, so no load ceiling can be derived.
    CpuCountUnreadable,
    /// The load ceiling the policy states is not a usable number.
    CeilingUnusable,
}

impl PurityGap {
    /// The gap as one clause, naming what could not be read.
    ///
    /// # Panics
    /// Never.
    pub fn text(self) -> &'static str {
        match self {
            Self::ProcessTableUnreadable => "the process table could not be listed",
            Self::LoadUnreadable => "the one-minute load average could not be read",
            Self::CpuCountUnreadable => "the processor count could not be read",
            Self::CeilingUnusable => "the load ceiling the policy states is not a usable number",
        }
    }
}

/// The guard's answer about one machine.
#[derive(Clone, Debug, PartialEq)]
pub enum PurityVerdict {
    /// The machine is quiet enough for a measurement taken on it to be believed.
    Clean,
    /// The machine is busy, for these reasons, and the measurement is refused. A second reading
    /// that also failed does not soften it: the refusal rests on a fact that is certain.
    Dirty(Vec<Impurity>),
    /// Facts the verdict needs could not be read, so the measurement is refused.
    Undeterminable(Vec<PurityGap>),
}

/// What the machine was doing when the verdict was made.
///
/// `None` never means "nothing": it means the reading was not taken, which is what
/// [`PurityGap`] records. A load average that *was* read is always finite and non-negative.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MachineFacts {
    /// Build and test processes running outside this probe's own ancestry.
    pub concurrent_builds: Option<u32>,
    /// The one-minute load average, when it could be read.
    pub load_1m: Option<f64>,
    /// How many processors the kernel reports, when it lists any.
    pub cpu_count: Option<u32>,
    /// The model of the first processor, when the platform names one.
    pub cpu_model: Option<String>,
    /// The frequency-scaling governor of the first processor, when the platform has one.
    pub scaling_governor: Option<String>,
}

impl MachineFacts {
    /// Reads the machine below `root`, which is `/` on the machine this runs on.
    ///
    /// The root is a parameter so that a test can point the reader at a `/proc` tree it wrote
    /// itself. The build count excludes this process's own ancestry, because under `cargo
    /// nextest` or `cargo bench` the probe is itself a child of a build and test process.
    ///
    /// # Panics
    /// Never.
    pub fn read(root: &Path) -> Self {
        let cpu = fs::read_to_string(root.join(CPUINFO_FILE))
            .ok()
            .map_or_else(CpuInfo::default, |text| cpuinfo_from(&text));
        Self {
            concurrent_builds: read_table(&root.join(PROC_DIR))
                .map(|rows| count_builds(&rows, std::process::id())),
            load_1m: fs::read_to_string(root.join(LOADAVG_FILE))
                .ok()
                .and_then(|text| load_1m_from(&text)),
            cpu_count: cpu.count,
            cpu_model: cpu.model,
            scaling_governor: fs::read_to_string(root.join(GOVERNOR_FILE))
                .ok()
                .and_then(|text| governor_from(&text)),
        }
    }
}

/// The thresholds a verdict is made with.
///
/// They are a parameter rather than constants inside [`PurityReport::judge`], so that a caller
/// holding a threshold from `docs/dev/budgets.json` passes it in rather than this module
/// keeping a second copy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PurityPolicy {
    /// Ceiling on the one-minute load average, as a multiple of the processor count.
    ///
    /// A machine is quiet only while the load is *below* it. The number must be finite: a
    /// factor that is not is reported as [`PurityGap::CeilingUnusable`] rather than compared.
    pub max_load_per_cpu: f64,
}

impl PurityPolicy {
    /// The guard's own policy: a quarter of the processor count.
    ///
    /// The governor is deliberately not part of it: a machine running `powersave` still
    /// produces comparable numbers when its baseline was frozen under the same governor.
    pub const DEFAULT: Self = Self {
        max_load_per_cpu: 0.25,
    };
}

/// What one sampling of the machine amounts to.
#[derive(Clone, Debug, PartialEq)]
pub struct PurityReport {
    /// What the machine was doing.
    facts: MachineFacts,
    /// The verdict.
    verdict: PurityVerdict,
}

impl PurityReport {
    /// Judges `facts` under `policy`, reading nothing outside its two arguments.
    ///
    /// # Panics
    /// Never.
    pub fn judge(facts: MachineFacts, policy: &PurityPolicy) -> Self {
        let mut reasons = Vec::new();
        let mut gaps = Vec::new();
        match facts.concurrent_builds {
            Some(0) => {}
            Some(count) => reasons.push(Impurity::ConcurrentBuilds { count }),
            None => gaps.push(PurityGap::ProcessTableUnreadable),
        }
        judge_load(&facts, policy, &mut reasons, &mut gaps);
        let verdict = if !reasons.is_empty() {
            PurityVerdict::Dirty(reasons)
        } else if !gaps.is_empty() {
            PurityVerdict::Undeterminable(gaps)
        } else {
            PurityVerdict::Clean
        };
        Self { facts, verdict }
    }

    /// Samples this machine and judges it with the guard's own policy.
    ///
    /// # Panics
    /// Never.
    pub fn sample() -> Self {
        Self::sample_below(Path::new(ROOT), &PurityPolicy::DEFAULT)
    }

    /// Samples the machine below `root` and judges it with `policy`.
    ///
    /// # Panics
    /// Never.
    pub fn sample_below(root: &Path, policy: &PurityPolicy) -> Self {
        Self::judge(MachineFacts::read(root), policy)
    }

    /// What the machine was doing.
    ///
    /// # Panics
    /// Never.
    pub fn facts(&self) -> &MachineFacts {
        &self.facts
    }

    /// The verdict.
    ///
    /// # Panics
    /// Never.
    pub fn verdict(&self) -> &PurityVerdict {
        &self.verdict
    }

    /// Whether a measurement taken on this machine may be believed.
    ///
    /// `false` for a busy machine and an unreadable one alike: there is no third answer that
    /// lets a number through.
    ///
    /// # Panics
    /// Never.
    pub fn is_clean(&self) -> bool {
        matches!(self.verdict, PurityVerdict::Clean)
    }

    /// Why a measurement is refused, or `None` when the machine is clean.
    ///
    /// # Panics
    /// Never.
    pub fn rejection_reason(&self) -> Option<String> {
        match &self.verdict {
            PurityVerdict::Clean => None,
            PurityVerdict::Dirty(reasons) => Some(format!(
                "the machine is not quiet enough to believe a performance number: {}; \
                 re-measure when nothing else is building",
                reasons
                    .iter()
                    .map(Impurity::text)
                    .collect::<Vec<String>>()
                    .join("; ")
            )),
            PurityVerdict::Undeterminable(gaps) => Some(format!(
                "whether the machine is quiet could not be established: {}; this guard fails \
                 closed, so the measurement is refused rather than assumed clean",
                gaps.iter()
                    .map(|gap| gap.text())
                    .collect::<Vec<&str>>()
                    .join("; ")
            )),
        }
    }

    /// A note about the frequency-scaling governor, or `None` when the machine runs the one a
    /// measurement for a release figure is expected to be taken under.
    ///
    /// The governor is an annotation rather than a criterion: a machine running `powersave`
    /// still produces numbers that can be compared with a baseline frozen under `powersave`,
    /// which is why this never makes [`Self::is_clean`] false. It is the fact a reader of the
    /// number needs, because a ratio taken under one governor and read against a number taken
    /// under another is not the same measurement -- a pair [`baseline::Baseline::compare`]
    /// refuses outright.
    ///
    /// # Panics
    /// Never.
    pub fn governor_note(&self) -> Option<String> {
        match self.facts.scaling_governor.as_deref() {
            Some(PERFORMANCE_GOVERNOR) => None,
            Some(governor) => Some(format!(
                "the frequency-scaling governor is `{governor}`, not `{PERFORMANCE_GOVERNOR}`: \
                 this number is comparable only with a baseline frozen under the same governor"
            )),
            None => Some(
                "the frequency-scaling governor could not be read, so this number cannot be \
                 shown to have been taken in the same state as an earlier one"
                    .to_owned(),
            ),
        }
    }

    /// Hands `measurement` back, or refuses it when this report is not clean.
    ///
    /// This is the point of the module: a caller with a number to report calls this instead of
    /// printing it, so a refusal becomes the caller's failure rather than a warning beside a
    /// number nobody should have quoted.
    ///
    /// # Errors
    /// Returns [`PurityRefusal`] when [`Self::is_clean`] is false, carrying
    /// [`Self::rejection_reason`].
    ///
    /// # Panics
    /// Never.
    pub fn accept<T>(&self, measurement: T) -> Result<T, PurityRefusal> {
        match self.rejection_reason() {
            None => Ok(measurement),
            Some(reason) => Err(PurityRefusal { reason }),
        }
    }

    /// Hands the samples of one case back, or refuses them when the machine was busy or the
    /// samples disagree with each other.
    ///
    /// This is the second half of "may this number be believed". A machine can be idle and the
    /// case still answer with samples that disagree with each other -- a page fault, a scheduler
    /// decision, another process evicting the cache -- and a mean over samples that wide
    /// describes the run rather than the code. The refusal is a failure, not a warning beside
    /// the number, exactly as [`Self::accept`]'s is.
    ///
    /// `allowed_relative_spread` is how far `std_dev / mean` may go before the samples are
    /// judged to disagree -- `0.05` for five percent, and at it is still inside. The tolerance
    /// belongs to the caller for the same reason [`baseline::Baseline::compare`]'s does: the
    /// budget document states thresholds, not tolerances.
    ///
    /// # Errors
    /// Returns [`PurityRefusal`] when this report is not clean, when `mean` and `std_dev` are
    /// not the mean and the spread of a set of samples, and when the relative spread is past
    /// `allowed_relative_spread`. A tolerance that is not a finite, non-negative number is
    /// refused rather than read as a wide one.
    ///
    /// # Panics
    /// Never.
    pub fn accept_samples(
        &self,
        mean: f64,
        std_dev: f64,
        allowed_relative_spread: f64,
    ) -> Result<SampleSpread, PurityRefusal> {
        // The machine is judged first: a tight spread taken on a busy machine is still a number
        // that measures the other build as much as this code.
        if let Some(reason) = self.rejection_reason() {
            return Err(PurityRefusal { reason });
        }
        if !allowed_relative_spread.is_finite() || allowed_relative_spread < 0.0 {
            return Err(PurityRefusal {
                reason: format!(
                    "the tolerance {allowed_relative_spread} is not a usable spread: it must be a \
                     finite number that is not negative"
                ),
            });
        }
        let spread = SampleSpread { mean, std_dev };
        let Some(relative) = spread.relative() else {
            return Err(PurityRefusal {
                reason: format!(
                    "a mean of {mean} and a standard deviation of {std_dev} are not the mean and \
                     the spread of a set of samples, so the two cannot be judged"
                ),
            });
        };
        if relative > allowed_relative_spread {
            return Err(PurityRefusal {
                reason: format!(
                    "the samples of this case disagree by {:.1}% of their mean (a standard \
                     deviation of {std_dev} against a mean of {mean}), past the {:.1}% this run \
                     allows: the number reads the machine's noise as much as the code",
                    relative * 100.0,
                    allowed_relative_spread * 100.0
                ),
            });
        }
        Ok(spread)
    }
}

/// A measurement the guard refused to believe.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("performance measurement refused: {reason}")]
pub struct PurityRefusal {
    /// Why the measurement was refused.
    reason: String,
}

/// The mean and the spread of the samples one case answered with.
///
/// Neither number is meaningful without the other: a standard deviation of fourteen nanoseconds
/// is a tight run around 700 and a loose one around 40. The pair travels together because it is
/// what a refusal is about, and because [`SampleSpread::relative`] is the only reading of it that
/// can be compared across cases.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleSpread {
    /// The mean of the samples, in whatever unit the case is measured in.
    pub mean: f64,
    /// The standard deviation of the samples, in the same unit.
    pub std_dev: f64,
}

impl SampleSpread {
    /// The standard deviation as a fraction of the mean, or `None` when the two numbers are not
    /// the mean and the spread of a set of samples.
    ///
    /// A mean that is not a positive finite number cannot be divided by, and a spread that is
    /// not a finite, non-negative one is not a spread. Both are `None` rather than zero: a zero
    /// here would read as "the samples agreed perfectly", which is the one answer this guard
    /// must never invent.
    ///
    /// # Panics
    /// Never.
    pub fn relative(&self) -> Option<f64> {
        let usable = self.mean.is_finite()
            && self.mean > 0.0
            && self.std_dev.is_finite()
            && self.std_dev >= 0.0;
        if usable {
            Some(self.std_dev / self.mean)
        } else {
            None
        }
    }
}

/// Whether the load average is inside the ceiling, as the reason it is not, or the gap left.
///
/// The ceiling's factor is checked rather than trusted: a non-finite ceiling would make every
/// comparison answer "clean", so it is reported as a gap instead.
fn judge_load(
    facts: &MachineFacts,
    policy: &PurityPolicy,
    reasons: &mut Vec<Impurity>,
    gaps: &mut Vec<PurityGap>,
) {
    // A load that is not a finite number is refused rather than compared: `NaN >= ceiling` is
    // false, and reading that as "clean" is the one direction this guard must never fail in.
    let Some(load_1m) = facts.load_1m.filter(|load| load.is_finite()) else {
        return gaps.push(PurityGap::LoadUnreadable);
    };
    let Some(cpu_count) = facts.cpu_count else {
        return gaps.push(PurityGap::CpuCountUnreadable);
    };
    let ceiling = f64::from(cpu_count) * policy.max_load_per_cpu;
    if !ceiling.is_finite() {
        return gaps.push(PurityGap::CeilingUnusable);
    }
    // At the ceiling is already too busy: the criterion is that the load stays below it.
    if load_1m >= ceiling {
        reasons.push(Impurity::Load {
            load_1m,
            cpu_count,
            ceiling,
        });
    }
}

/// The one-minute load average a `/proc/loadavg` document states.
///
/// The file holds three averages, the running and total process counts, and the last process
/// id: `0.42 0.35 0.30 1/1234 5678`. A first field that is not a finite, non-negative number
/// is `None` rather than a zero: a load nobody could read must not be compared with a ceiling
/// as though the machine were idle.
fn load_1m_from(text: &str) -> Option<f64> {
    let load = text.split_whitespace().next()?.parse::<f64>().ok()?;
    (load.is_finite() && load >= 0.0).then_some(load)
}

/// The processors a `/proc/cpuinfo` document lists, and the model they are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CpuInfo {
    /// How many processors the document lists, when it lists any.
    count: Option<u32>,
    /// The `model name` of the first processor, when the platform states one.
    model: Option<String>,
}

/// Reads the processor count and the model out of a `/proc/cpuinfo` document.
///
/// A document that lists no processor has no count rather than a count of zero: a ceiling of
/// zero would refuse every machine, which is a reading error reported as a busy machine. The
/// count saturates, so more `processor` lines than a `u32` can hold cannot wrap to zero.
fn cpuinfo_from(text: &str) -> CpuInfo {
    let mut count: u32 = 0;
    let mut model = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() == PROCESSOR_FIELD {
            count = count.saturating_add(1);
        } else if key.trim() == MODEL_FIELD && model.is_none() {
            let value = value.trim();
            if !value.is_empty() {
                model = Some(value.to_owned());
            }
        }
    }
    CpuInfo {
        count: (count > 0).then_some(count),
        model,
    }
}

/// The governor a `scaling_governor` document names.
fn governor_from(text: &str) -> Option<String> {
    let governor = text.trim();
    (!governor.is_empty()).then(|| governor.to_owned())
}

/// One row of the process table, as the counting rule needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ProcessRow {
    /// The process id.
    pid: u32,
    /// The parent process id.
    ppid: u32,
    /// The process name, as the kernel truncates it to fifteen characters.
    comm: String,
}

/// Reads the process table under `proc_dir`.
///
/// `None` when the directory cannot be listed at all, which is what makes the build count
/// unknown rather than zero. A row that cannot be read is skipped: a process that exited while
/// the table was walked is normal.
fn read_table(proc_dir: &Path) -> Option<Vec<ProcessRow>> {
    let entries = fs::read_dir(proc_dir).ok()?;
    let mut rows = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        if let Some(row) = read_status(proc_dir, pid) {
            rows.push(row);
        }
    }
    Some(rows)
}

/// Reads one process's row out of its `status` file.
fn read_status(proc_dir: &Path, pid: u32) -> Option<ProcessRow> {
    let path = proc_dir.join(pid.to_string()).join(STATUS_FILE);
    parse_status(pid, &fs::read_to_string(path).ok()?)
}

/// Parses the two fields a row needs out of a `status` document.
///
/// A document missing either is not a process status document: the row is dropped, not guessed.
fn parse_status(pid: u32, text: &str) -> Option<ProcessRow> {
    let mut comm = None;
    let mut ppid = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix(NAME_FIELD) {
            comm = Some(value.trim()).filter(|name| !name.is_empty());
        } else if let Some(value) = line.strip_prefix(PARENT_FIELD) {
            ppid = value.trim().parse::<u32>().ok();
        }
    }
    Some(ProcessRow {
        pid,
        ppid: ppid?,
        comm: comm?.to_owned(),
    })
}

/// The process ids from `own_pid` up to the root of the tree, `own_pid` first.
///
/// The walk is bounded by the table's length, so a table that names a cycle -- which `/proc`
/// cannot produce but a test can -- ends the walk instead of looping.
fn own_ancestry(own_pid: u32, rows: &[ProcessRow]) -> Vec<u32> {
    let mut chain = vec![own_pid];
    let mut current = own_pid;
    for _ in 0..rows.len() {
        let Some(row) = rows.iter().find(|row| row.pid == current) else {
            break;
        };
        if row.ppid == 0 || chain.contains(&row.ppid) {
            break;
        }
        chain.push(row.ppid);
        current = row.ppid;
    }
    chain
}

/// The build and test processes of `rows` that are not in `own_pid`'s own ancestry.
///
/// Zero therefore means nothing else on this machine is building: the probe's own chain is not
/// a claim about the machine, it is the harness the probe is running under.
fn count_builds(rows: &[ProcessRow], own_pid: u32) -> u32 {
    let own = own_ancestry(own_pid, rows);
    let count = rows
        .iter()
        .filter(|row| AGENT_PROGRAMS.contains(&row.comm.as_str()))
        .filter(|row| !own.contains(&row.pid))
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}
