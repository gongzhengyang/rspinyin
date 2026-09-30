//! The schedule a soak run follows: how long it runs, how fast it types, how often it
//! samples, and how much of the start counts as warm-up.
//!
//! Responsibility: validate the numbers a caller asked for, derive the two counts the run
//! and its report are measured against, and answer which of the two schedules is due next.
//!
//! # Why the counts are derived rather than counted
//!
//! A pass rate is a ratio, and a ratio whose denominator is "however much the loop happened
//! to do" is always 100%: a run that sent ten strokes and completed ten of them would read
//! as a perfect one. The denominator is the plan -- `duration x rate` strokes and
//! `duration / interval + 1` samples -- and it is fixed before the first key goes out, so a
//! run that stopped early reports the fraction it really completed. That is the whole
//! reason the schedule is computed here instead of being read off the run afterwards.
//!
//! # Why the schedule is a merge
//!
//! One thread drives both the injection and the sampling, so the two share a clock.
//! [`Plan::due`] answers which of them is due first from two counters and no other state,
//! which is what makes the whole schedule -- including the instant where a stroke and a
//! sample fall on the same tick -- a case an ordinary unit test drives without waiting for
//! either. The tie goes to the sample: a reading taken at the instant a stroke is due
//! describes the state *before* that stroke, which keeps the series lined up with the
//! stroke count instead of one ahead of it.

use std::time::Duration;

/// The smallest steady window a plan may leave.
///
/// An envelope needs two readings and a straight line through them needs two points, and a
/// window with fewer has neither. A plan that leaves less than this is refused rather than
/// reported as a run whose drift could not be computed.
pub const MIN_STEADY_SAMPLES: u64 = 2;

/// Everything that makes a soak plan unusable.
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    /// A number the caller gave is not one a schedule can be built from.
    #[error("`{field}` must be a finite number greater than zero, found {value}")]
    NotPositive {
        /// The option that was refused.
        field: &'static str,
        /// The value it was given.
        value: f64,
    },

    /// A duration the caller gave is outside what the platform can represent.
    ///
    /// The check exists because the rest of this module converts its fields to
    /// [`Duration`] freely, and a conversion that cannot fail is what keeps a panic out of
    /// a tool that runs unattended for eight hours.
    #[error("`{field}` ({value}) is outside the range a duration can hold")]
    TooLong {
        /// The option that was refused.
        field: &'static str,
        /// The value it was given.
        value: f64,
    },

    /// The plan comes to no strokes at all.
    #[error(
        "a {duration_s}s run at {rate_hz} keys per second comes to no strokes; a run that \
         types nothing measures nothing"
    )]
    NoStrokes {
        /// The run's duration, in seconds.
        duration_s: f64,
        /// The typing rate that was asked for.
        rate_hz: f64,
    },

    /// The warm-up leaves too small a steady window inside the run.
    #[error(
        "a {warmup_s}s warm-up leaves {steady_samples} steady sample(s) inside a \
         {duration_s}s run sampled every {interval_s}s; the run must be long enough for at \
         least {min} of them after the warm-up"
    )]
    WarmupTooLong {
        /// The warm-up that was asked for, in seconds.
        warmup_s: f64,
        /// The run's duration, in seconds.
        duration_s: f64,
        /// The sampling interval that was asked for, in seconds.
        interval_s: f64,
        /// How many samples the steady window would hold.
        steady_samples: u64,
        /// The smallest steady window this module accepts.
        min: u64,
    },
}

/// Which of the run's two schedules is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Type the next stroke of the cycle.
    Stroke,
    /// Read the plugin's counters.
    Sample,
}

/// The schedule one soak run follows.
///
/// Built through [`Plan::new`], which refuses a set of numbers no schedule can be made
/// from; every accessor below therefore answers a number that has already been checked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// How long the run lasts, in seconds.
    duration_s: f64,
    /// How many strokes per second the run types.
    rate_hz: f64,
    /// How many seconds apart the samples are.
    interval_s: f64,
    /// How many seconds at the start are excluded from the drift statistic.
    warmup_s: f64,
}

impl Plan {
    /// The plan the four numbers describe, or the reason they do not describe one.
    ///
    /// # Errors
    ///
    /// Returns [`PlanError::NotPositive`] for a value that is not a finite positive number,
    /// [`PlanError::TooLong`] for a duration the platform cannot represent,
    /// [`PlanError::NoStrokes`] when the duration and the rate come to no stroke at all, and
    /// [`PlanError::WarmupTooLong`] when the warm-up leaves fewer than
    /// [`MIN_STEADY_SAMPLES`] samples behind it.
    pub fn new(
        hours: f64,
        rate_hz: f64,
        interval_s: f64,
        warmup_s: f64,
    ) -> Result<Self, PlanError> {
        for (field, value) in [
            ("--hours", hours),
            ("--rate-hz", rate_hz),
            ("--interval-s", interval_s),
            ("--warmup-s", warmup_s),
        ] {
            representable(field, value)?;
        }
        let duration_s = hours * SECONDS_PER_HOUR;
        // The schedule is built from a duration in seconds and a key period rather than from
        // the hours and the rate the caller typed, so the two derived values are checked as
        // well: a product that overflows is a schedule nothing could run.
        representable("--hours", duration_s)?;
        representable("--rate-hz", 1.0 / rate_hz)?;
        let plan = Self {
            duration_s,
            rate_hz,
            interval_s,
            warmup_s,
        };
        if plan.planned_strokes() == 0 {
            return Err(PlanError::NoStrokes {
                duration_s,
                rate_hz,
            });
        }
        let steady = plan.steady_samples();
        if steady < MIN_STEADY_SAMPLES {
            return Err(PlanError::WarmupTooLong {
                warmup_s,
                duration_s,
                interval_s,
                steady_samples: steady,
                min: MIN_STEADY_SAMPLES,
            });
        }
        Ok(plan)
    }

    /// How long the run lasts, in seconds.
    pub fn duration_s(&self) -> f64 {
        self.duration_s
    }

    /// How many strokes per second the run types.
    pub fn rate_hz(&self) -> f64 {
        self.rate_hz
    }

    /// How many seconds apart the samples are.
    pub fn interval_s(&self) -> f64 {
        self.interval_s
    }

    /// How many seconds at the start are excluded from the drift statistic.
    pub fn warmup_s(&self) -> f64 {
        self.warmup_s
    }

    /// The run's duration.
    ///
    /// # Panics
    ///
    /// Never: [`Plan::new`] converted this field once already and refuses a value it cannot
    /// convert, so the conversion here repeats one that has succeeded.
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.duration_s)
    }

    /// The interval between two samples.
    ///
    /// # Panics
    ///
    /// Never, for the reason [`Plan::duration`] gives.
    pub fn sample_interval(&self) -> Duration {
        Duration::from_secs_f64(self.interval_s)
    }

    /// The interval between two strokes.
    ///
    /// # Panics
    ///
    /// Never, for the reason [`Plan::duration`] gives.
    pub fn key_period(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.rate_hz)
    }

    /// How many strokes the run plans to send.
    ///
    /// The run sends exactly this many, so the pass rate a report states is a fraction of
    /// something that was decided in advance rather than of whatever the loop reached.
    pub fn planned_strokes(&self) -> u64 {
        (self.duration_s * self.rate_hz).floor().max(0.0) as u64
    }

    /// How many samples the run plans to take.
    ///
    /// The first is taken at the instant the run starts, so a duration that divides evenly
    /// by the interval comes to one more sample than the quotient.
    pub fn planned_samples(&self) -> u64 {
        (self.duration_s / self.interval_s).floor().max(0.0) as u64 + 1
    }

    /// The index of the first sample that is inside the steady window.
    ///
    /// The window opens at the first sample taken at or after the warm-up ends, so a
    /// warm-up that is not a whole number of intervals is rounded up rather than down: the
    /// point of the warm-up is to be outside the measurement, and rounding down would put
    /// its last moments inside it.
    pub fn steady_from(&self) -> u64 {
        (self.warmup_s / self.interval_s).ceil().max(0.0) as u64
    }

    /// How many samples the steady window holds.
    pub fn steady_samples(&self) -> u64 {
        self.planned_samples().saturating_sub(self.steady_from())
    }

    /// Which schedule is due next, and how far into the run it falls.
    ///
    /// `strokes` and `samples` are how many of each the run has already done, so the whole
    /// schedule is a function of two counters: nothing here remembers anything, which is
    /// what lets a test walk a plan of any length without running it. `None` when both
    /// schedules are exhausted.
    ///
    /// A stroke and a sample that fall on the same instant are answered as the sample; see
    /// the module documentation for why.
    ///
    /// # Panics
    ///
    /// Never: both instants are computed from counters that are below the plan's own counts,
    /// so neither can exceed the duration [`Plan::new`] already converted.
    pub fn due(&self, strokes: u64, samples: u64) -> Option<(Step, Duration)> {
        let stroke_at = if strokes < self.planned_strokes() {
            Some(Duration::from_secs_f64(strokes as f64 / self.rate_hz))
        } else {
            None
        };
        let sample_at = if samples < self.planned_samples() {
            Some(Duration::from_secs_f64(samples as f64 * self.interval_s))
        } else {
            None
        };
        match (stroke_at, sample_at) {
            (Some(stroke), Some(sample)) => Some(if sample <= stroke {
                (Step::Sample, sample)
            } else {
                (Step::Stroke, stroke)
            }),
            (Some(stroke), None) => Some((Step::Stroke, stroke)),
            (None, Some(sample)) => Some((Step::Sample, sample)),
            (None, None) => None,
        }
    }
}

/// Seconds in one hour.
///
/// Public because the report states its duration in seconds and prints it in hours, and the
/// two conversions have to be the same number rather than two copies of it.
pub const SECONDS_PER_HOUR: f64 = 3600.0;

/// Refuses a number the rest of the module would have to convert fallibly.
fn representable(field: &'static str, value: f64) -> Result<(), PlanError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(PlanError::NotPositive { field, value });
    }
    Duration::try_from_secs_f64(value)
        .map(|_| ())
        .map_err(|_| PlanError::TooLong { field, value })
}
