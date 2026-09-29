//! A sample in flight: started before the work, recorded when its scope ends.
//!
//! Responsibility: pair the two halves of one measurement so that the caller cannot take
//! the clock reading and lose it -- or record it against the wrong histogram.
//! [`Probes::begin`] answers a [`Stamp`] naming the metric, and the stamp records the one
//! sample it stands for when it is dropped.
//!
//! # Why a scope rather than two calls
//!
//! Because the code that takes a sample runs inside an Fcitx5 callback, where the design
//! forbids blocking work and the probes are not allowed to be the reason a keystroke is
//! late. A stamp keeps both costs where they belong: [`Probes::begin`] is one relaxed
//! load and, only when the probes are on, one clock reading, and the record is a bucket
//! lookup and a relaxed increment. Switched off, no clock is read at all -- a caller that
//! held its own [`Instant`] would pay for the clock whether or not anything was
//! listening.
//!
//! Recording on the way out of the scope, rather than at a call the caller has to place,
//! is what makes the measurement survive a callback that leaves early: a rejected event
//! takes one of the branches that return before the work does, and a span that had
//! already started is still a span the user waited through.
//!
//! # What a caller owes
//!
//! Nothing inside the scope but the work being measured: no allocation, no lock, no
//! formatting, no file, no waiting on another thread. A segment whose measurement needs
//! any of those is a segment that has to be measured somewhere else.
//!
//! [`Instant`]: std::time::Instant

use std::time::Instant;

use super::{Histogram, Metric, Probes};

/// One sample of one metric, in flight for as long as the value lives.
///
/// The stamp borrows the histogram it will be recorded into, so it cannot outlive the
/// probes. It records exactly one sample, at the end of the scope it is bound in.
#[derive(Debug)]
#[must_use = "a stamp measures the scope it lives in; bind it to a variable"]
pub struct Stamp<'p> {
    /// The histogram this sample belongs to.
    histogram: &'p Histogram,
    /// When the work started, or `None` when the probes were switched off.
    start: Option<Instant>,
}

impl Probes {
    /// Starts a sample of `metric`, recorded when the returned stamp is dropped.
    ///
    /// # Arguments
    ///
    /// * `metric` — which measurement this sample belongs to.
    ///
    /// # Returns
    ///
    /// The stamp, which records exactly one sample at the end of the scope it is bound
    /// in. Switched off, the stamp carries no start time and records nothing: the two
    /// halves are then one relaxed load and one branch.
    ///
    /// # Panics
    ///
    /// Never.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_diag::probe::{Metric, Probes};
    ///
    /// let probes = Probes::new();
    /// {
    ///     let _sample = probes.begin(Metric::PostUi);
    ///     // ... the work being measured ...
    /// }
    /// assert_eq!(probes.post_ui.count(), 1);
    /// assert_eq!(probes.decode.count(), 0);
    ///
    /// // Switched off, the same scope records nothing.
    /// probes.set_enabled(false);
    /// {
    ///     let _sample = probes.begin(Metric::PostUi);
    /// }
    /// assert_eq!(probes.post_ui.count(), 1);
    /// ```
    pub fn begin(&self, metric: Metric) -> Stamp<'_> {
        let histogram = metric.histogram(self);
        Stamp {
            histogram,
            // The clock is read only when something will be recorded, so a switched-off
            // probe costs a load and a branch rather than a clock reading per key.
            start: histogram.is_enabled().then(Instant::now),
        }
    }
}

impl Drop for Stamp<'_> {
    /// Records the sample, and how long it took.
    ///
    /// A stamp taken while the probes were on and dropped after they were switched off
    /// records nothing: a sample that spans the switch is dropped rather than
    /// half-counted, exactly as an end-to-end sample whose receipt arrives after the
    /// switch is.
    fn drop(&mut self) {
        if let Some(start) = self.start.take() {
            self.histogram.record(start.elapsed());
        }
    }
}
