//! The eight metrics the runtime probes record.
//!
//! Responsibility: name each metric once and say how it is reached -- which
//! histogram on [`Probes`](super::Probes) it lives in, which field of a snapshot
//! carries it, and which unit a report prints it in. Everything that walks the
//! metrics (the snapshot writer, the parser, both renderers) walks
//! [`Metric::ALL`], so a metric added here appears everywhere without another list
//! to keep in step.
//!
//! Boundaries: a metric is a measurement, not a threshold. Which threshold governs
//! which metric is [`crate::report`]'s table, and what the numbers mean is the
//! reader's business.

use super::snapshot::Metrics;
use super::{HistSnapshot, Histogram, Probes};

/// The number of metrics a probe records.
///
/// A constant rather than `Metric::ALL.len()` so that it can be an array length;
/// a test keeps the two in step.
pub const METRIC_COUNT: usize = 8;

/// The unit a metric is printed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// Milliseconds, printed with two decimals.
    Millis,
    /// Whole microseconds.
    Micros,
}

impl Unit {
    /// The suffix a report prints after a value, and the unit name it writes into
    /// the machine-readable form.
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Millis => "ms",
            Self::Micros => "us",
        }
    }

    /// Renders a value given in microseconds in this unit.
    ///
    /// Latencies are stated in milliseconds when they are milliseconds and in
    /// microseconds when they are not: a wakeup latency printed as `0.02ms` hides the
    /// magnitude the budget is about.
    pub fn format(self, us: f64) -> String {
        match self {
            Self::Millis => format!("{:.2}ms", us / 1_000.0),
            Self::Micros => format!("{us:.0}us"),
        }
    }
}

/// One measured metric.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Metric {
    /// Key press to the candidate frame being presented, end to end across the two
    /// threads.
    KeyToPresent,
    /// One decode call.
    Decode,
    /// One full-frame software raster.
    RasterFull,
    /// One partial-frame software raster. The design states no budget for it; it is
    /// recorded because it is what most frames cost.
    RasterPartial,
    /// First key press to the candidate window being visible.
    FirstKeyToVisible,
    /// Writing the wakeup `eventfd` to the UI thread returning from `poll`.
    Wakeup,
    /// One `on_key_event`, decode and posting included.
    EventLoopKey,
    /// One command handed to the UI thread, as the host thread experiences it: the
    /// channel hand-off and the wakeup that follows it.
    ///
    /// The last segment of the host path, and the only one where the design lets the
    /// host thread wait at all: the ordered `Show` / `Hide` queue may make the poster
    /// spin before it collapses a pair. A key whose end-to-end latency regressed has
    /// to be attributable to the work before the post or to the post itself, and
    /// without this metric the two cannot be told apart.
    ///
    /// The design states no threshold for it, so a report prints it without a budget
    /// rather than against an invented one -- the same way it treats
    /// [`Metric::RasterPartial`].
    PostUi,
}

impl Metric {
    /// Every metric, in the order a report lists them.
    ///
    /// The order is the order a report's rows and a snapshot's records are written in,
    /// so a metric is appended rather than inserted: a reader diffing two reports then
    /// sees the numbers that moved instead of the whole table.
    pub const ALL: [Self; METRIC_COUNT] = [
        Self::KeyToPresent,
        Self::Decode,
        Self::RasterFull,
        Self::RasterPartial,
        Self::FirstKeyToVisible,
        Self::Wakeup,
        Self::EventLoopKey,
        Self::PostUi,
    ];

    /// The name a report prints and a snapshot file carries.
    pub const fn name(self) -> &'static str {
        match self {
            Self::KeyToPresent => "key_to_present",
            Self::Decode => "decode",
            Self::RasterFull => "raster_full",
            Self::RasterPartial => "raster_partial",
            Self::FirstKeyToVisible => "first_key_to_visible",
            Self::Wakeup => "wakeup",
            Self::EventLoopKey => "event_loop_key",
            Self::PostUi => "post_ui",
        }
    }

    /// The unit a report prints this metric in.
    pub const fn unit(self) -> Unit {
        match self {
            Self::Wakeup | Self::PostUi => Unit::Micros,
            Self::KeyToPresent
            | Self::Decode
            | Self::RasterFull
            | Self::RasterPartial
            | Self::FirstKeyToVisible
            | Self::EventLoopKey => Unit::Millis,
        }
    }

    /// The index this metric occupies in [`Metric::ALL`].
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The metric a name refers to.
    ///
    /// A name that is not one of [`Metric::ALL`] is `None` rather than a new metric,
    /// so that a snapshot file naming something unknown is refused.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|metric| metric.name() == name)
    }

    /// This metric's histogram on `probes`.
    pub fn histogram(self, probes: &Probes) -> &Histogram {
        match self {
            Self::KeyToPresent => &probes.key_to_present,
            Self::Decode => &probes.decode,
            Self::RasterFull => &probes.raster_full,
            Self::RasterPartial => &probes.raster_partial,
            Self::FirstKeyToVisible => &probes.first_key_to_visible,
            Self::Wakeup => &probes.wakeup,
            Self::EventLoopKey => &probes.event_loop_key,
            Self::PostUi => &probes.post_ui,
        }
    }

    /// This metric's distribution in `metrics`.
    pub fn snapshot(self, metrics: &Metrics) -> &HistSnapshot {
        match self {
            Self::KeyToPresent => &metrics.key_to_present,
            Self::Decode => &metrics.decode,
            Self::RasterFull => &metrics.raster_full,
            Self::RasterPartial => &metrics.raster_partial,
            Self::FirstKeyToVisible => &metrics.first_key_to_visible,
            Self::Wakeup => &metrics.wakeup,
            Self::EventLoopKey => &metrics.event_loop_key,
            Self::PostUi => &metrics.post_ui,
        }
    }

    /// This metric's distribution in `metrics`, for writing.
    pub fn snapshot_mut(self, metrics: &mut Metrics) -> &mut HistSnapshot {
        match self {
            Self::KeyToPresent => &mut metrics.key_to_present,
            Self::Decode => &mut metrics.decode,
            Self::RasterFull => &mut metrics.raster_full,
            Self::RasterPartial => &mut metrics.raster_partial,
            Self::FirstKeyToVisible => &mut metrics.first_key_to_visible,
            Self::Wakeup => &mut metrics.wakeup,
            Self::EventLoopKey => &mut metrics.event_loop_key,
            Self::PostUi => &mut metrics.post_ui,
        }
    }
}
