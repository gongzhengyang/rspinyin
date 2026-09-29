//! The window: a process's counters followed over time, and the one place a ceiling is applied.
//!
//! Responsibility: hold the samples a case takes, answer the two differences a budget is stated
//! against, and judge one budget at a time against the ceilings the case read out of
//! `docs/dev/budgets.json`. [`MemoryMonitor::judge`] is the module's only verdict, and a
//! measurement past its ceiling is an error rather than a value, so a case that writes that
//! call has asserted its budget and there is no way to hold a [`BudgetVerdict`] for a window
//! that broke one.
//!
//! Boundaries: it reads nothing itself -- the reading is `sample`'s -- and it states no ceiling
//! of its own, which arrives as a [`MemoryBudgets`] the case built from the budget document.
//! Which process is followed, and when the window opens, are the case's decisions; this module
//! follows and judges, and does nothing else.
//!
//! # Two differences, and why both are needed
//!
//! [`MemoryMonitor::delta_from_baseline_kb`] is the net growth since the case took its baseline,
//! which is what the rendering layer's budget is about: the process being sampled was resident
//! long before the candidate window existed, so its absolute resident set says nothing about
//! what the renderer costs. [`MemoryMonitor::drift_kb`] is the growth the window reached from
//! its first sample to its highest, which is what a soak run asserts: a process that climbed and
//! then held its ground has grown, and reading that plateau as no growth is the false pass the
//! drift exists to prevent.
//!
//! # The report
//!
//! [`MemoryMonitor::lines`] is the shape `xtask` prints and the resource layer publishes: the
//! window's counters, the basis, a degradation when there is one, and both differences with
//! their sign. A difference that could not be computed prints `none` rather than a zero,
//! because a zero is a measurement nobody took.

// The channel is exercised by the tests beside the module root and by nothing else yet: the
// case runner that would open a monitor, the evidence archiver and the subcommand tree all live
// in files this module does not own, and the root carries the full note. Until that wiring
// lands, every item here is reported as dead code in a non-test build, and the attribute goes
// away together with the root's.
#![allow(dead_code)]

use std::path::PathBuf;

use super::sample::{MemorySample, SampleBasis, read_sample};

use super::{BudgetVerdict, Measure, MemoryBudget, MemoryBudgets, MemoryError, PROC};

/// A process's memory counters, followed over a window.
///
/// # Concurrency
///
/// `Send`, and not shared: a monitor belongs to the case that opened it, every method takes
/// `&self` or `&mut self`, and none of them blocks beyond the reads themselves.
#[derive(Debug)]
pub struct MemoryMonitor {
    /// The directory the process's counters are exposed under.
    proc_dir: PathBuf,
    /// The process being followed.
    pid: u32,
    /// The sample a growth is measured from, once the case has taken one.
    baseline: Option<MemorySample>,
    /// Every sample taken, in the order it was taken.
    samples: Vec<MemorySample>,
}

impl MemoryMonitor {
    /// Follows `pid` as the kernel exposes it under `/proc`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new(pid: u32) -> Self {
        Self::under(PathBuf::from(PROC), pid)
    }

    /// Follows `pid` as `proc_dir` exposes it.
    ///
    /// The directory is a parameter so that a case can be driven against a recorded tree: the
    /// reading code is the same either way, which is what makes the degradation path -- a
    /// process whose `smaps_rollup` cannot be read -- a case a test can run at all.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn under(proc_dir: PathBuf, pid: u32) -> Self {
        Self {
            proc_dir,
            pid,
            baseline: None,
            samples: Vec::new(),
        }
    }

    /// Follows the process this harness is running in: what a test drives the reader against.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn of_current_process() -> Self {
        Self::new(std::process::id())
    }

    /// The process being followed.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Reads the process's counters, records them, and answers the reading.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::ProcUnreadable`] when the process's `status` cannot be read --
    /// it has exited, or this process may not look at it -- and [`MemoryError::Malformed`]
    /// when a file that was read does not state the counter it must.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn sample(&mut self) -> Result<MemorySample, MemoryError> {
        let sample = read_sample(&self.proc_dir, self.pid)?;
        self.samples.push(sample);
        Ok(sample)
    }

    /// Takes a sample and makes it the baseline a growth is measured from.
    ///
    /// The sample is recorded in the window like any other, so the baseline is visible in a
    /// report rather than being a number the module keeps on the side.
    ///
    /// # Errors
    ///
    /// As [`Self::sample`].
    ///
    /// # Panics
    ///
    /// Never.
    pub fn mark_baseline(&mut self) -> Result<MemorySample, MemoryError> {
        let sample = self.sample()?;
        self.baseline = Some(sample);
        Ok(sample)
    }

    /// The sample a growth is measured from, once one has been taken.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn baseline(&self) -> Option<&MemorySample> {
        self.baseline.as_ref()
    }

    /// The window's samples, in the order they were taken.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn samples(&self) -> &[MemorySample] {
        &self.samples
    }

    /// The most recent sample.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn latest(&self) -> Option<&MemorySample> {
        self.samples.last()
    }

    /// The resident growth from the first sample of the window to its highest.
    ///
    /// This is the drift a soak asserts on, and it is deliberately not the difference between
    /// the last sample and the first: a process that climbed and then held its ground has
    /// grown, and reading that plateau as no growth is the false pass the assertion exists to
    /// prevent. `None` when the window holds fewer than two samples -- one reading is not a
    /// drift, and zero would say it was.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn drift_kb(&self) -> Option<i64> {
        // A drift is a change, and one reading cannot show one: the window has to hold a
        // second sample before there is anything to measure. Answering zero would let a
        // growth budget pass on a window that was never measured. The second reading is
        // taken by pulling twice rather than by comparing a length against a number,
        // because every ceiling this module measures against is a number too.
        let mut readings = self.samples.iter();
        let first = readings.next()?;
        readings.next()?;
        let highest = self.samples.iter().map(|sample| sample.vm_rss_kb).max()?;
        // The counters are unsigned and a window that shrank has a negative drift, so the
        // difference is taken in `i64`. A resident set larger than `i64` can hold is not a
        // number a process has, and `try_from` refusing it is the honest answer to a counter
        // that could not have come from one.
        let first = i64::try_from(first.vm_rss_kb).ok()?;
        let highest = i64::try_from(highest).ok()?;
        Some(highest.saturating_sub(first))
    }

    /// The resident growth since the baseline sample.
    ///
    /// Negative when the process gave memory back, which is why it is signed: a case reporting
    /// a window wants the direction. [`Self::judge`] is where the sign stops mattering, since
    /// a ceiling is about growth and a shrink is inside every one of them. `None` when no
    /// baseline has been taken, or when the window is empty -- a delta from nothing is not
    /// zero, and reporting it as zero is how a budget passes without ever being measured.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn delta_from_baseline_kb(&self) -> Option<i64> {
        let baseline = self.baseline?;
        let latest = self.latest()?;
        let baseline = i64::try_from(baseline.vm_rss_kb).ok()?;
        let latest = i64::try_from(latest.vm_rss_kb).ok()?;
        Some(latest.saturating_sub(baseline))
    }

    /// Judges one budget against this monitor's window.
    ///
    /// This is the module's only verdict, and a measurement past its ceiling is an error
    /// rather than a value: a case that writes this call has asserted its budget, and there is
    /// no way to hold a [`BudgetVerdict`] for a window that broke one.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::Unbudgeted`] when the ceiling table states no usable ceiling for
    /// `budget`, [`MemoryError::NoSample`] when nothing has been sampled yet,
    /// [`MemoryError::NoBaseline`] when the budget is a growth and no baseline was taken,
    /// [`MemoryError::TooFewSamples`] when it is a drift and the window holds fewer than two
    /// samples, [`MemoryError::BasisDegraded`] when it reads a rollup counter the sample does
    /// not carry, and [`MemoryError::OverBudget`] when the measurement is past its ceiling.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn judge(
        &self,
        budgets: &MemoryBudgets,
        budget: MemoryBudget,
    ) -> Result<BudgetVerdict, MemoryError> {
        let key = budget.key();
        let measure = budget.measure();
        let limit_kb = budgets
            .limit_kb(budget)
            .ok_or(MemoryError::Unbudgeted { key })?;
        let measured_kb = self.measure(measure, key)?;
        if measured_kb > limit_kb {
            return Err(MemoryError::OverBudget {
                key,
                measure: measure.label(),
                measured_kb,
                limit_kb,
            });
        }
        Ok(BudgetVerdict {
            budget,
            measured_kb,
            limit_kb,
        })
    }

    /// The monitor's report as lines of text, for `xtask` to print and for the resource layer
    /// to publish.
    ///
    /// The shape is stable: `pid` and `samples` always, then the latest sample's `basis` --
    /// `none` before the first sample -- the window's first and latest `vm_rss_kb` when there
    /// is one, a `basis_degraded` line when the rollup counters are not measurements, and the
    /// two differences last, printed with their sign and `none` when they cannot be computed.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("pid: {}", self.pid),
            format!("samples: {}", self.samples.len()),
        ];
        match self.latest() {
            None => lines.push("basis: none".to_owned()),
            Some(sample) => {
                let basis = sample.basis.label();
                lines.push(format!("basis: {basis}"));
                let first = self
                    .samples
                    .first()
                    .map_or(sample.vm_rss_kb, |s| s.vm_rss_kb);
                let latest = sample.vm_rss_kb;
                lines.push(format!("vm_rss_kb: first {first} latest {latest}"));
                if let SampleBasis::RssOnly(reason) = sample.basis {
                    lines.push(format!(
                        "basis_degraded: smaps_rollup was unreadable ({reason:?}), so the \
                         anonymous and private-dirty counters are not measurements"
                    ));
                }
            }
        }
        lines.push(format!("drift_kb: {}", signed_or_none(self.drift_kb())));
        let delta = signed_or_none(self.delta_from_baseline_kb());
        lines.push(format!("delta_from_baseline_kb: {delta}"));
        lines
    }

    /// The number of this monitor's window that `measure` names.
    fn measure(&self, measure: Measure, key: &'static str) -> Result<u64, MemoryError> {
        let pid = self.pid;
        let latest = self.latest().ok_or(MemoryError::NoSample { pid })?;
        match measure {
            Measure::RssDelta => {
                let growth = self
                    .delta_from_baseline_kb()
                    .ok_or(MemoryError::NoBaseline { pid, key })?;
                // A window that shrank has no growth for a ceiling to be about, and the report
                // carries the signed number regardless.
                Ok(growth.max(0) as u64)
            }
            Measure::RssDrift => {
                let drift = self
                    .drift_kb()
                    .ok_or(MemoryError::TooFewSamples { pid, key })?;
                Ok(drift.max(0) as u64)
            }
            Measure::Anonymous | Measure::PrivateDirty => {
                if !latest.basis.has_rollup() {
                    return Err(MemoryError::BasisDegraded {
                        pid,
                        key,
                        measure: measure.label(),
                    });
                }
                if measure == Measure::Anonymous {
                    Ok(latest.anonymous_kb)
                } else {
                    Ok(latest.private_dirty_kb)
                }
            }
        }
    }
}

/// A signed counter as a report prints it, `none` when it cannot be computed.
fn signed_or_none(value: Option<i64>) -> String {
    value.map_or_else(|| "none".to_owned(), |value| value.to_string())
}
