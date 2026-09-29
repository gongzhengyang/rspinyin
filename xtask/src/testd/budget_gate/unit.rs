//! The unit a budget key is stated in.
//!
//! Split out of `budget_gate.rs` to keep that file inside the line limit. The unit table is
//! the one place this gate could drift from the document, so it lives beside nothing else:
//! a reader looking for "which unit is `decode_p99` in" has exactly one file to read.

/// Nanoseconds in one microsecond.
const NANOS_PER_MICRO: f64 = 1e3;

/// Nanoseconds in one millisecond.
const NANOS_PER_MILLI: f64 = 1e6;

/// Nanoseconds in one second.
const NANOS_PER_SECOND: f64 = 1e9;

/// Nanoseconds in one hour.
const NANOS_PER_HOUR: f64 = 3.6e12;

/// The quantity a number is stated in.
///
/// A measurement without its unit is not a measurement: `2.4` is inside a three-millisecond ceiling
/// and far outside a three-nanosecond one. The variants are the units the budget document's own
/// field names carry, plus the two the CPU and socket sections are stated in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// Nanoseconds.
    Nanos,
    /// Microseconds.
    Micros,
    /// Milliseconds.
    Millis,
    /// Seconds.
    Seconds,
    /// Mebibytes of resident set or of artifact size.
    Mebibytes,
    /// Percent of one core, or a pass rate.
    Percent,
    /// Hours.
    Hours,
    /// A count of occurrences, which has no suffix of its own.
    Count,
}

impl Unit {
    /// The suffix a report prints after a number in this unit.
    ///
    /// The suffixes are the ones the document's field names use, so a report line and the key it
    /// names are read in one vocabulary. A count has no suffix: the number is the whole of it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::Nanos => "ns",
            Self::Micros => "us",
            Self::Millis => "ms",
            Self::Seconds => "s",
            Self::Mebibytes => "MiB",
            Self::Percent => "%",
            Self::Hours => "h",
            Self::Count => "",
        }
    }

    /// How many of `other` one of `self` is worth, when the two measure the same quantity.
    ///
    /// `None` when they do not: a nanosecond is a time and a mebibyte is a size, and a comparison
    /// between the two is a defect in a binding table rather than a verdict about a budget. The
    /// caller that receives a `None` reports the measurement as unusable rather than reading the
    /// two numbers against each other.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn scale_to(self, other: Self) -> Option<f64> {
        if self.dimension() != other.dimension() {
            return None;
        }
        Some(self.base_factor() / other.base_factor())
    }

    /// Which quantity this unit measures.
    fn dimension(self) -> Dimension {
        match self {
            Self::Nanos | Self::Micros | Self::Millis | Self::Seconds | Self::Hours => {
                Dimension::Time
            }
            Self::Mebibytes => Dimension::Bytes,
            Self::Percent => Dimension::Ratio,
            Self::Count => Dimension::Count,
        }
    }

    /// How many of the dimension's base unit one of this unit holds.
    ///
    /// The base of a dimension is the smallest unit this module names for it: nanoseconds for a time,
    /// mebibytes for a size, and the unit itself for the two single-member dimensions.
    fn base_factor(self) -> f64 {
        match self {
            Self::Nanos => 1.0,
            Self::Micros => NANOS_PER_MICRO,
            Self::Millis => NANOS_PER_MILLI,
            Self::Seconds => NANOS_PER_SECOND,
            Self::Hours => NANOS_PER_HOUR,
            Self::Mebibytes | Self::Percent | Self::Count => 1.0,
        }
    }
}

/// Which quantity a unit measures. Two units convert into one another only inside one dimension,
/// which keeps a nanosecond from being judged against a mebibyte ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dimension {
    /// A duration.
    Time,
    /// An amount of memory or of file.
    Bytes,
    /// A proportion, stated as a percentage.
    Ratio,
    /// A number of occurrences.
    Count,
}

/// The unit the document states `key` in, or `None` when this module knows of no such key.
pub(super) fn unit_of(key: &str) -> Option<Unit> {
    KEY_UNITS
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, unit)| *unit)
}

/// The unit the document states each budget in, read off its own field names.
///
/// This is a binding table in the sense the rest of the workspace uses the phrase: it records *which*
/// unit a key is stated in, never what the number is. It is also the one place this module could
/// drift from the document, which is why the audit in the tests holds it against the document's key
/// set in both directions.
///
/// `latency_ms` states its thresholds in milliseconds, `bench` carries the unit in every field name
/// (`*_ns`, `*_us`, `*_s`), and the two counters of the idle budget and the socket count are counts
/// rather than percentages -- the one place a section name does not name the unit of every key in it.
pub(super) const KEY_UNITS: &[(&str, Unit)] = &[
    ("latency_ms.key_to_present_p50", Unit::Millis),
    ("latency_ms.key_to_present_p99", Unit::Millis),
    ("latency_ms.key_to_present_p99_144hz", Unit::Millis),
    ("latency_ms.decode_p99", Unit::Millis),
    ("latency_ms.decode_p999", Unit::Millis),
    ("latency_ms.raster_p99", Unit::Millis),
    ("latency_ms.first_key_to_visible_p99", Unit::Millis),
    ("latency_ms.addon_load", Unit::Millis),
    ("memory_mb.ui_rss", Unit::Mebibytes),
    ("memory_mb.plugin_rss", Unit::Mebibytes),
    ("memory_mb.dict_mmap_rss", Unit::Mebibytes),
    ("cpu_pct.idle", Unit::Percent),
    ("cpu_pct.typing_10cps", Unit::Percent),
    ("cpu_pct.idle_redraw_count", Unit::Count),
    ("cpu_pct.idle_poll_timer_count", Unit::Count),
    ("size_mb.so_stripped", Unit::Mebibytes),
    ("size_mb.base_dict", Unit::Mebibytes),
    ("robustness.soak_hours", Unit::Hours),
    ("robustness.rss_drift_mb", Unit::Mebibytes),
    ("robustness.pass_rate_pct", Unit::Percent),
    ("bench.passthrough_classify_ns", Unit::Nanos),
    ("bench.input_buffer_ops_us", Unit::Micros),
    ("bench.decode_holdout_s", Unit::Seconds),
    ("bench.ui_wakeup_latency_us", Unit::Micros),
    ("net_sockets", Unit::Count),
];
