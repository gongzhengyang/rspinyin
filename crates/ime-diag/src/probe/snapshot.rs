//! The snapshot: what a probe measured, in the form a file carries.
//!
//! Responsibility: freeze the numbers a running plugin has collected, and read them
//! back somewhere else. The file is line-oriented text -- one `key=value` record per
//! line, naming metrics and counters by the names the contract fixes -- and the
//! first line is [`SNAPSHOT_HEADER`].
//!
//! # Why the format is ours
//!
//! The diagnostics crate carries no serialization dependency: it sits on the
//! plugin's own path, and its dependency set is deliberately the smallest in the
//! workspace. A flat record of whole numbers needs no library to write or to parse,
//! and a text file can be read, diffed and grepped with the tools already at hand --
//! the same reason the log format is compact text rather than JSON. Nothing here
//! can hold a string a user typed, so the escaping question a general-purpose
//! format exists to answer does not arise.
//!
//! # Strictness
//!
//! The file is read as untrusted input: an unknown key, an unknown metric or
//! counter name, a record written twice, a line that is not `key=value`, and a
//! value that is not a whole number are all refused with the line number, rather
//! than silently contributing a zero to a report that then reads as a pass.

use std::io::{self, Write as _};
use std::path::Path;
use std::time::Duration;

use super::HistSnapshot;
use super::counters::{COUNTER_COUNT, Counter};
use super::metric::Metric;

/// The version tag every snapshot file starts with.
pub const SNAPSHOT_HEADER: &str = "rspinyin-probe 1";

/// The name of the snapshot file, inside the plugin's data directory.
///
/// Stated beside the format it names rather than in the layout module, because the
/// two are one decision: the file that holds this text is this file's name.
pub const SNAPSHOT_FILE_NAME: &str = "probe.txt";

/// Reads one field out of a distribution.
type FieldReader = fn(&HistSnapshot) -> u64;

/// The fields of a metric's distribution, in the order the file writes them, each
/// with the reader that extracts it.
///
/// One table rather than a pair of `match` arms that have to be kept in step: the
/// writer walks it, and the parser refuses a field name that is not in it, so a
/// field cannot be written by one half of the format and unknown to the other.
const FIELDS: [(&str, FieldReader); 7] = [
    ("count", |snapshot| snapshot.count),
    ("sum_us", |snapshot| snapshot.sum_us),
    ("p50_us", |snapshot| snapshot.p50_us),
    ("p90_us", |snapshot| snapshot.p90_us),
    ("p99_us", |snapshot| snapshot.p99_us),
    ("p999_us", |snapshot| snapshot.p999_us),
    ("max_us", |snapshot| snapshot.max_us),
];

/// Every metric's distribution, as one snapshot carries them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Metrics {
    /// Key press to presented frame.
    pub key_to_present: HistSnapshot,
    /// One decode call.
    pub decode: HistSnapshot,
    /// One full-frame raster.
    pub raster_full: HistSnapshot,
    /// One partial-frame raster.
    pub raster_partial: HistSnapshot,
    /// First key to a visible window.
    pub first_key_to_visible: HistSnapshot,
    /// `eventfd` wakeup to the UI thread returning from `poll`.
    pub wakeup: HistSnapshot,
    /// One `on_key_event`.
    pub event_loop_key: HistSnapshot,
}

/// One probe's numbers at one moment.
///
/// This is what crosses the boundary between the running plugin and the report: the
/// plugin writes it out, the command line reads it back, and neither has to be the
/// other's library.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProbeSnapshot {
    /// How long the probes had been running when the snapshot was taken.
    pub sampled: Duration,
    /// How many input sessions were opened in that window.
    pub sessions: u64,
    /// How many key presses were seen in that window.
    pub keys: u64,
    /// The measured distributions.
    pub metrics: Metrics,
    /// Every counter's value, indexed by [`Counter::index`].
    pub counters: [u64; COUNTER_COUNT],
}

impl ProbeSnapshot {
    /// One counter's value.
    ///
    /// # Panics
    ///
    /// Never: the index comes from the counter itself.
    pub fn counter(&self, counter: Counter) -> u64 {
        self.counters[counter.index()]
    }

    /// Renders the snapshot as the text a snapshot file holds.
    ///
    /// The order is fixed -- header, window, metrics in [`Metric::ALL`] order,
    /// counters in [`Counter::ALL`] order -- so that two snapshots of the same
    /// session diff to the numbers that changed.
    pub fn to_text(&self) -> String {
        let mut out = String::from(SNAPSHOT_HEADER);
        out.push('\n');
        let sampled_us = u64::try_from(self.sampled.as_micros()).unwrap_or(u64::MAX);
        out.push_str(&format!("sampled_us={sampled_us}\n"));
        out.push_str(&format!("sessions={}\n", self.sessions));
        out.push_str(&format!("keys={}\n", self.keys));
        for metric in Metric::ALL {
            let name = metric.name();
            let snapshot = metric.snapshot(&self.metrics);
            for (field, read) in FIELDS {
                out.push_str(&format!("metric.{name}.{field}={}\n", read(snapshot)));
            }
        }
        for counter in Counter::ALL {
            out.push_str(&format!(
                "counter.{}={}\n",
                counter.name(),
                self.counter(counter)
            ));
        }
        out
    }

    /// Writes the snapshot to `path`, which is created `0600`.
    ///
    /// The plugin calls this when it is asked for a snapshot -- `SIGUSR1`, in the
    /// running addon -- and the command line reads the same file back, so the two
    /// halves of the dashboard meet in a file rather than in a socket inside an input
    /// method. The directory is the caller's to prepare: this module owns the file's
    /// mode, the layout module owns where the file goes.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`io::Error`] when the file cannot be created, its mode
    /// cannot be set, or the text cannot be written.
    pub fn write_to(&self, path: &Path) -> io::Result<()> {
        let mut file = crate::perms::create_private(path)?;
        // Truncated explicitly: `create_private` opens without truncating, because the
        // log sink appends through it, and a shorter snapshot left over a longer one
        // would end in the older file's tail and fail to parse.
        file.set_len(0)?;
        file.write_all(self.to_text().as_bytes())
    }

    /// Reads the text a snapshot file holds.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidData`] when the text does not open with
    /// [`SNAPSHOT_HEADER`], when a line is not a `key=value` record, when a key is
    /// written twice, when a key is not a field of a snapshot, or when a value is
    /// not a whole number. Every message names the line, so a hand-edited file says
    /// where it went wrong.
    pub fn parse(text: &str) -> io::Result<Self> {
        let mut snapshot = Self::default();
        let mut seen: Vec<&str> = Vec::new();
        let mut opened = false;
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if !opened {
                if line != SNAPSHOT_HEADER {
                    return Err(malformed(format!(
                        "line {}: expected `{SNAPSHOT_HEADER}`, found `{line}`",
                        index + 1
                    )));
                }
                opened = true;
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(malformed(format!(
                    "line {}: `{line}` is not a `key=value` record",
                    index + 1
                )));
            };
            let key = key.trim();
            if seen.contains(&key) {
                return Err(malformed(format!(
                    "line {}: `{key}` is written twice",
                    index + 1
                )));
            }
            seen.push(key);
            apply(&mut snapshot, key, number(key, value.trim())?)?;
        }
        if !opened {
            return Err(malformed(format!(
                "empty snapshot: expected `{SNAPSHOT_HEADER}` first"
            )));
        }
        Ok(snapshot)
    }
}

impl HistSnapshot {
    /// Writes the field named `field`, refusing a name the format does not carry.
    fn set_field(&mut self, field: &str, value: u64) -> io::Result<()> {
        let slot = match field {
            "count" => &mut self.count,
            "sum_us" => &mut self.sum_us,
            "p50_us" => &mut self.p50_us,
            "p90_us" => &mut self.p90_us,
            "p99_us" => &mut self.p99_us,
            "p999_us" => &mut self.p999_us,
            "max_us" => &mut self.max_us,
            other => {
                let known: Vec<&str> = FIELDS.iter().map(|(name, _)| *name).collect();
                return Err(malformed(format!(
                    "`{other}` is not a field of a metric; expected one of {}",
                    known.join(", ")
                )));
            }
        };
        *slot = value;
        Ok(())
    }
}

/// Applies one `key=value` record to `snapshot`.
fn apply(snapshot: &mut ProbeSnapshot, key: &str, value: u64) -> io::Result<()> {
    match key {
        "sampled_us" => snapshot.sampled = Duration::from_micros(value),
        "sessions" => snapshot.sessions = value,
        "keys" => snapshot.keys = value,
        _ => return apply_named(snapshot, key, value),
    }
    Ok(())
}

/// Applies a record whose key names a metric or a counter.
fn apply_named(snapshot: &mut ProbeSnapshot, key: &str, value: u64) -> io::Result<()> {
    if let Some(rest) = key.strip_prefix("metric.") {
        let Some((name, field)) = rest.split_once('.') else {
            return Err(malformed(format!(
                "`{key}` is not a metric field; expected `metric.<name>.<field>`"
            )));
        };
        let Some(metric) = Metric::from_name(name) else {
            return Err(malformed(format!("`{key}`: `{name}` is not a metric")));
        };
        return metric
            .snapshot_mut(&mut snapshot.metrics)
            .set_field(field, value);
    }
    if let Some(name) = key.strip_prefix("counter.") {
        let Some(counter) = Counter::from_name(name) else {
            return Err(malformed(format!("`{key}`: `{name}` is not a counter")));
        };
        snapshot.counters[counter.index()] = value;
        return Ok(());
    }
    Err(malformed(format!(
        "`{key}` is not a field of a probe snapshot"
    )))
}

/// A value that is not a whole number.
fn number(key: &str, value: &str) -> io::Result<u64> {
    value
        .parse::<u64>()
        .map_err(|_| malformed(format!("`{key}`: `{value}` is not a whole number")))
}

/// A malformed snapshot.
fn malformed(detail: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, detail.into())
}
