//! The sweep's arithmetic: ranking one run's points and reading the parameters'
//! sensitivity out of them.
//!
//! Responsibility: order the points the way the recommendation table presents
//! them, and summarize how much each parameter's value moved the objective. Both
//! are pure functions of their inputs -- no clock, no filesystem, no iteration
//! order they did not bring with them -- so the same run ranks the same way on
//! every machine, and a test drives both on fixture points alone.
//!
//! The ranking implements the recommendation order the sweep contract states:
//! overall top-1 first, the two-character band as the tie-break (the core band
//! is the one a tail-band gain must not be allowed to buy with a loss), then the
//! compile time (a tie should favour the cheaper build), and finally the
//! parameter assignment itself, so that two points measuring identically still
//! land in one order instead of an arbitrary one. The last rule is what makes
//! the table a pure function of the run: without it, equal rows would keep
//! whatever order the grid enumeration happened to produce, which is
//! deterministic today and invisible the day it changes.

use std::time::Duration;

use super::{Assignment, GridValue, SweepSpec};

/// What one grid point measured, as plain numbers the ranking can order.
///
/// The rates are fractions in `0.0..=1.0`, carried as they fall out of the
/// quality measurement; the table renders them as percentages at print time. A
/// band that the holdout set does not exercise is `None` rather than zero, so an
/// unmeasured band can never read as a bad one.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PointMetrics {
    /// Fraction of every case whose first candidate was the expected word.
    pub(super) top1_overall: f64,
    /// The same fraction over the single-character band.
    pub(super) top1_single: Option<f64>,
    /// The same fraction over the two-character band: the core band the
    /// tie-break protects.
    pub(super) top1_two: Option<f64>,
    /// The same fraction over the three-or-more-character band.
    pub(super) top1_long: Option<f64>,
    /// Fraction of every case whose expected word reached the first page.
    pub(super) top9_overall: f64,
    /// Cases whose expected word the container does not serve at all.
    pub(super) unreachable: u32,
}

/// One grid point's measurement, with everything the table and the ranking read.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PointResult {
    /// The point's assignment, as the values the compiler was driven with.
    pub(super) assignment: Assignment,
    /// What the holdout measured against the compiled container.
    pub(super) metrics: PointMetrics,
    /// Wall time the compile step took; the tie-break for equal metrics.
    pub(super) compile: Duration,
}

impl PointResult {
    /// The point rendered as one table row: the label, the metrics and the
    /// compile time, in the order the columns print.
    ///
    /// The band columns print `n/a` where the holdout set does not exercise the
    /// band, matching the quality closure's convention for an unmeasured band.
    pub(super) fn table_row(&self) -> String {
        let metrics = &self.metrics;
        let band = |band: Option<f64>| {
            band.map_or_else(|| "n/a".to_owned(), |rate| format!("{:.2}%", rate * 100.0))
        };
        format!(
            "{:<48} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>7.2}s",
            self.assignment.label,
            format!("{:.2}%", metrics.top1_overall * 100.0),
            band(metrics.top1_two),
            band(metrics.top1_single),
            band(metrics.top1_long),
            format!("{:.2}%", metrics.top9_overall * 100.0),
            metrics.unreachable,
            self.compile.as_secs_f64(),
        )
    }
}

/// Ranks the run's points best first, by the recommendation order.
///
/// Overall top-1 descending; ties fall to the two-character band (descending,
/// an unmeasured band counting as the worst a band can be), then to the compile
/// time (ascending, the cheaper build wins a tie), then to the assignment label
/// so equal rows land in one order. `total_cmp` orders the rates because a
/// total order on floats is the property the determinism claim rests on.
pub(super) fn rank(mut points: Vec<PointResult>) -> Vec<PointResult> {
    points.sort_by(|left, right| {
        let left_metrics = &left.metrics;
        let right_metrics = &right.metrics;
        right_metrics
            .top1_overall
            .total_cmp(&left_metrics.top1_overall)
            .then_with(|| {
                band_key(right_metrics.top1_two).total_cmp(&band_key(left_metrics.top1_two))
            })
            .then_with(|| left.compile.cmp(&right.compile))
            .then_with(|| left.assignment.label.cmp(&right.assignment.label))
    });
    points
}

/// A band rate as a total order key, with an unmeasured band at the bottom.
fn band_key(band: Option<f64>) -> f64 {
    band.unwrap_or(f64::NEG_INFINITY)
}

/// One parameter value's reading: the best top-1 any point holding it achieved.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ValueReading {
    /// The parameter value the reading is about.
    pub(super) value: GridValue,
    /// The best top-1 over the points holding the value, or `None` when no
    /// point holds it -- which a complete grid cannot produce, and which prints
    /// as unmeasured rather than as zero if it ever does.
    pub(super) best_top1: Option<f64>,
    /// How many points the reading stands on.
    pub(super) points: usize,
}

impl ValueReading {
    /// The reading rendered for the sensitivity line, the value first.
    pub(super) fn line(&self) -> String {
        let best = self
            .best_top1
            .map_or_else(|| "n/a".to_owned(), |rate| format!("{:.2}%", rate * 100.0));
        format!("{} -> {best} ({} pt)", self.value, self.points)
    }
}

/// Summarizes each parameter's sensitivity: for every declared value, the best
/// top-1 the points holding it reached.
///
/// The parameters and their values are walked in the order the spec declares
/// them -- not in an order derived from the results -- so the summary's shape is
/// the grid's shape, and two runs over the same spec print the same skeleton
/// whatever the measurements say. The spread a reader acts on is the difference
/// between a parameter's best and worst reading: a flat parameter is one the
/// objective cannot distinguish at this grid's density, which is itself a
/// finding worth recording.
pub(super) fn sensitivity(
    points: &[PointResult],
    spec: &SweepSpec,
) -> Vec<(String, Vec<ValueReading>)> {
    spec.grid
        .iter()
        .map(|(parameter, values)| {
            let readings = values
                .iter()
                .map(|value| {
                    let holding: Vec<f64> = points
                        .iter()
                        .filter(|point| point.assignment.values.get(parameter) == Some(value))
                        .map(|point| point.metrics.top1_overall)
                        .collect();
                    ValueReading {
                        value: value.clone(),
                        // An empty `holding` reads as `None`: unmeasured, never
                        // zero. The maximum goes through `total_cmp`, the same
                        // total order the ranking itself uses.
                        best_top1: holding.iter().copied().reduce(|left, right| {
                            if left.total_cmp(&right) == std::cmp::Ordering::Less {
                                right
                            } else {
                                left
                            }
                        }),
                        points: holding.len(),
                    }
                })
                .collect();
            (parameter.clone(), readings)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use super::{PointMetrics, PointResult, band_key, rank, sensitivity};
    use crate::tune::sweep::{Assignment, GridValue, SweepSpec};

    /// A point over the two-parameter fixture spec: `a` in `[1, 2]`, `b` in
    /// `[10, 20]`, with the caller choosing the metrics and the compile time.
    fn point(a: i64, b: i64, top1: f64, two: Option<f64>, compile_ms: u64) -> PointResult {
        let mut values = BTreeMap::new();
        values.insert("a".to_owned(), GridValue::Integer(a));
        values.insert("b".to_owned(), GridValue::Integer(b));
        PointResult {
            assignment: Assignment {
                values,
                label: format!("a={a} b={b}"),
            },
            metrics: PointMetrics {
                top1_overall: top1,
                top1_single: None,
                top1_two: two,
                top1_long: None,
                top9_overall: top1 + 0.1,
                unreachable: 0,
            },
            compile: Duration::from_millis(compile_ms),
        }
    }

    /// The four-point fixture grid as a spec, matching [`point`].
    fn spec() -> SweepSpec {
        let mut grid = BTreeMap::new();
        grid.insert(
            "a".to_owned(),
            vec![GridValue::Integer(1), GridValue::Integer(2)],
        );
        grid.insert(
            "b".to_owned(),
            vec![GridValue::Integer(10), GridValue::Integer(20)],
        );
        SweepSpec { grid }
    }

    #[test]
    fn test_rank_orders_by_top1_overall() {
        let ranked = rank(vec![
            point(1, 10, 0.70, Some(0.80), 9_000),
            point(2, 10, 0.74, Some(0.80), 9_000),
            point(1, 20, 0.72, Some(0.80), 9_000),
        ]);
        let top1s: Vec<f64> = ranked
            .iter()
            .map(|result| result.metrics.top1_overall)
            .collect();
        assert_eq!(top1s, vec![0.74, 0.72, 0.70]);
    }

    #[test]
    fn test_rank_tie_breaks_on_the_two_character_band() {
        let ranked = rank(vec![
            point(1, 10, 0.74, Some(0.88), 9_000),
            point(2, 10, 0.74, Some(0.89), 9_000),
        ]);
        assert_eq!(ranked[0].assignment.label, "a=2 b=10");
    }

    #[test]
    fn test_rank_tie_breaks_on_compile_duration() {
        let ranked = rank(vec![
            point(1, 10, 0.74, Some(0.88), 9_500),
            point(2, 10, 0.74, Some(0.88), 9_000),
        ]);
        assert_eq!(ranked[0].assignment.label, "a=2 b=10");
    }

    #[test]
    fn test_rank_full_tie_lands_in_label_order() {
        let ranked = rank(vec![
            point(2, 20, 0.74, Some(0.88), 9_000),
            point(1, 10, 0.74, Some(0.88), 9_000),
        ]);
        assert_eq!(ranked[0].assignment.label, "a=1 b=10");
        assert_eq!(ranked[1].assignment.label, "a=2 b=20");
    }

    #[test]
    fn test_rank_is_independent_of_the_input_permutation() {
        let one = rank(vec![
            point(1, 10, 0.70, Some(0.80), 9_000),
            point(2, 10, 0.74, Some(0.88), 9_000),
            point(1, 20, 0.74, Some(0.88), 9_000),
            point(2, 20, 0.74, Some(0.88), 9_000),
        ]);
        let two = rank(one.clone());
        let mut shuffled = two.clone();
        shuffled.reverse();
        let three = rank(shuffled);
        let labels = |points: &[PointResult]| -> Vec<String> {
            points.iter().map(|p| p.assignment.label.clone()).collect()
        };
        assert_eq!(labels(&one), labels(&two));
        assert_eq!(labels(&two), labels(&three));
    }

    #[test]
    fn test_band_key_puts_an_unmeasured_band_last() {
        assert!(band_key(None) < band_key(Some(0.0)));
        assert!(band_key(None) < band_key(Some(-1.0)));
        assert!(band_key(Some(-1.0)) < band_key(Some(0.0)));
    }

    #[test]
    fn test_sensitivity_walks_the_declared_grid_order() {
        let points = vec![
            point(1, 10, 0.70, Some(0.80), 9_000),
            point(1, 20, 0.72, Some(0.80), 9_000),
            point(2, 10, 0.74, Some(0.88), 9_000),
            point(2, 20, 0.73, Some(0.88), 9_000),
        ];
        let summary = sensitivity(&points, &spec());
        let names: Vec<&String> = summary.iter().map(|(name, _)| name).collect();
        assert_eq!(names.len(), 2);
        let (first, a_readings) = (&summary[0].0, &summary[0].1);
        assert_eq!(first, "a");
        let values: Vec<i64> = a_readings
            .iter()
            .filter_map(|reading| match reading.value {
                GridValue::Integer(value) => Some(value),
                GridValue::Float(_) => None,
            })
            .collect();
        assert_eq!(values, vec![1, 2], "values follow the declared order");
        assert_eq!(a_readings.len(), values.len(), "no reading changed kind");
        let bests: Vec<Option<f64>> = a_readings
            .iter()
            .map(|reading| reading.best_top1)
            .collect();
        assert_eq!(bests, vec![Some(0.72), Some(0.74)]);
    }

    #[test]
    fn test_sensitivity_best_takes_the_max_over_holding_points() {
        // Both points hold b=10, so b's 10 reading is the best over two points
        // while b's declared 20 reading is held by none and reads as unmeasured
        // rather than as zero.
        let points = vec![
            point(1, 10, 0.70, None, 9_000),
            point(2, 10, 0.68, None, 9_000),
        ];
        let summary = sensitivity(&points, &spec());
        let a_readings = &summary[0].1;
        assert_eq!(a_readings[0].best_top1, Some(0.70));
        assert_eq!(a_readings[0].points, 1);
        assert_eq!(a_readings[1].best_top1, Some(0.68));
        let b_readings = &summary[1].1;
        assert_eq!(b_readings[0].best_top1, Some(0.70));
        assert_eq!(b_readings[0].points, 2);
        assert_eq!(b_readings[1].best_top1, None);
        assert_eq!(b_readings[1].points, 0);
    }
}
