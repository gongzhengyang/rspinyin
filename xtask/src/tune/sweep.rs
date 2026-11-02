//! The parameter-grid sweep: dictionary-build knobs against the holdout top-1.
//!
//! Responsibility: read a declarative TOML parameter space (parameter name ->
//! grid points), compile one container per grid point through the real `dictc`
//! pipeline, measure every container against the holdout set with the same
//! quality closure `xtask quality` uses, and print the points ranked by the
//! objective with a parameter-sensitivity summary.
//!
//! # Why each point runs the compiler as a child process
//!
//! The grid points are values of real build knobs (`--core-min-freq`,
//! `--tail-flat-weight`, `--max-bigram-pairs`, `--min-bigram-score`), and the
//! compiler owns their validation, the source allowlist, the section budgets
//! and the manifest -- one compile path, the same one `xtask dictc` runs. The
//! sweep drives the already-running xtask binary itself with a `dictc`
//! subcommand per point: the child inherits the CLI defaults for every knob the
//! grid does not name, so there is no second table of shipped values to drift,
//! and a point the compiler refuses aborts the sweep with the compiler's own
//! diagnostic above it.
//!
//! # What the objective can and cannot observe
//!
//! The objective is the holdout top-1, broken by word-length band: the ranking
//! the compiler baked into the container. The four build knobs shape exactly
//! that. The project's two runtime knobs -- the user-database decay half-life
//! and the context re-rank band -- do not: the first acts on user history the
//! holdout set does not carry, the second on a near-tie re-rank that needs a
//! previous commit the holdout set does not supply. Naming either in a grid is
//! refused before anything compiles: a sweep that measured them would print
//! "no effect" where the truth is "no channel", and a plausible no-effect is
//! worse than a refusal.
//!
//! # The tool recommends; a human writes back
//!
//! The ranked table ends in a recommendation and nothing else. Writing a value
//! into the shipped defaults is a human adjudication recorded under
//! `docs/dev/research/` -- the gate floors may only rise, and no sweep run is
//! the decision.
//!
//! Boundaries: this layer reads the spec and the holdout, spawns the compile
//! child, and writes only scratch containers under `target/`, deleting each one
//! after it has been measured. It never touches a configuration file, a default
//! or the production container. The ranking and the sensitivity arithmetic live
//! in [`rank`](self::rank), pure over their inputs, so the table order is a
//! function of the run rather than of the machine.

mod rank;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use ime_dict::fst_index::FstLexicon;
use serde::Deserialize;

use crate::dictc::quality::{self, MIN_HOLDOUT_CASES};
use crate::tune::purity::PurityCheck;
use rank::{PointMetrics, PointResult, ValueReading, rank, sensitivity};

/// Where the sweep's scratch containers live, relative to the repository root.
/// Under `target/`, so a sweep never adds anything a clean tree has to carry.
const SWEEP_DIR: &str = "target/tune-sweep";

/// One dictionary-build knob a grid may name, and the `dictc` flag that carries
/// its value into the compile.
struct BuildKnob {
    /// The name the TOML parameter space spells.
    name: &'static str,
    /// The `dictc` flag the value reaches the compiler through.
    flag: &'static str,
    /// The value kind the flag accepts.
    kind: ValueKind,
    /// What the knob decides, for the refusal texts.
    summary: &'static str,
}

/// The value kind one knob accepts: whole numbers or floats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ValueKind {
    /// Whole-number points only.
    Integer,
    /// Float points; a whole number coerces, because `1` and `1.0` are the
    /// same value.
    Float,
}

/// The knobs the objective can observe, in the order the refusal text lists
/// them.
const BUILD_KNOBS: &[BuildKnob] = &[
    BuildKnob {
        name: "core_min_freq",
        flag: "--core-min-freq",
        kind: ValueKind::Integer,
        summary: "the source frequency below which a two-character word drops into \
                  the flattened tail band",
    },
    BuildKnob {
        name: "tail_flat_weight",
        flag: "--tail-flat-weight",
        kind: ValueKind::Integer,
        summary: "the flat weight every tail-band word receives after normalization",
    },
    BuildKnob {
        name: "max_pairs",
        flag: "--max-bigram-pairs",
        kind: ValueKind::Integer,
        summary: "the highest number of bigram pairs the compiler keeps",
    },
    BuildKnob {
        name: "min_score",
        flag: "--min-bigram-score",
        kind: ValueKind::Float,
        summary: "the minimum count-PMI score a bigram pair needs to survive the cut",
    },
];

/// The project's runtime knobs, which this objective cannot observe.
///
/// Each names the real configuration key and why the holdout top-1 is blind to
/// it, so a spec that names one is refused with an explanation rather than with
/// a measured non-effect.
const OBSERVATION_BLIND: &[(&str, &str)] = &[
    (
        "decay_half_life_ticks",
        "it decays user-database frequencies while the IME runs; the holdout \
         objective ranks a freshly compiled container that carries no user \
         history",
    ),
    (
        "context_tie_band_q16",
        "it widens or narrows the near-tie re-rank that needs the previous \
         commit as context; the holdout objective reads the container's baked \
         per-key order with no context at all",
    ),
];

/// One grid point's value: the number a knob takes.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum GridValue {
    /// A whole-number point, for the integer knobs.
    Integer(i64),
    /// A float point, for the float knobs.
    Float(f64),
}

impl GridValue {
    /// Renders the value the way a `dictc` flag accepts it.
    fn as_flag_value(&self) -> String {
        match self {
            Self::Integer(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
        }
    }
}

impl std::fmt::Display for GridValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.as_flag_value())
    }
}

/// One grid point: the value every named parameter takes, and the label the
/// table prints it under.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Assignment {
    /// Parameter name -> the value this point gives it.
    pub(super) values: BTreeMap<String, GridValue>,
    /// The point rendered as `name=value` pairs in parameter order.
    pub(super) label: String,
}

impl Assignment {
    /// Builds the assignment and derives its label from the values, in the
    /// map's own (parameter-name) order.
    fn new(values: BTreeMap<String, GridValue>) -> Self {
        let label = values
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(" ");
        Self { values, label }
    }
}

/// The parsed parameter space: a `[grid]` table of parameter name -> points.
#[derive(Debug)]
pub(super) struct SweepSpec {
    /// Parameter name -> the grid points, in the order the spec declares them.
    pub(super) grid: BTreeMap<String, Vec<GridValue>>,
}

/// The TOML document shape the spec parses from.
#[derive(Deserialize)]
struct SweepDocument {
    /// The `[grid]` section.
    grid: BTreeMap<String, toml::Value>,
}

impl SweepSpec {
    /// Parses and validates a parameter space.
    ///
    /// Validation is where a dishonest sweep dies early: an unknown parameter
    /// name, a runtime knob the objective cannot observe, an empty grid, or a
    /// single-point grid are all refusals here, before the first compile.
    ///
    /// # Errors
    ///
    /// Returns an error when the document has no `[grid]` section, when a
    /// parameter is unknown or observation-blind, when its points are empty or
    /// of the wrong kind for the knob, when the same value is declared twice,
    /// or when the whole grid holds fewer than two points.
    pub(super) fn parse(text: &str) -> Result<Self> {
        let document: SweepDocument = toml::from_str(text)
            .with_context(|| "the sweep spec must be a TOML document with a [grid] table")?;
        ensure!(
            !document.grid.is_empty(),
            "sweep: the [grid] table names no parameter; a sweep needs at least one"
        );
        let mut grid = BTreeMap::new();
        for (name, raw) in &document.grid {
            grid.insert(name.clone(), Self::points_of(name, raw)?);
        }
        let spec = Self { grid };
        ensure!(
            spec.point_count() >= 2,
            "sweep: the grid enumerates {} point(s); a sweep of fewer than two has \
             nothing to compare",
            spec.point_count()
        );
        Ok(spec)
    }

    /// Resolves one parameter's points, checking the name against the registry
    /// and the values against the knob's kind.
    fn points_of(name: &str, raw: &toml::Value) -> Result<Vec<GridValue>> {
        if let Some((_, reason)) = OBSERVATION_BLIND.iter().find(|(blind, _)| *blind == name) {
            bail!(
                "sweep: `{name}` is a real knob, but this sweep's objective cannot \
                 observe it: {reason}. Sweep the build knobs ({}), or give the \
                 objective a channel that can see the knob first; measuring it here \
                 would report \"no effect\" where the truth is \"no channel\"",
                BUILD_KNOBS
                    .iter()
                    .map(|knob| knob.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let knob = BUILD_KNOBS
            .iter()
            .find(|knob| knob.name == name)
            .with_context(|| {
                format!(
                    "sweep: `{name}` is not a parameter this sweep knows; the build knobs it \
                     can drive are {} and the runtime knobs the objective cannot observe are {}",
                    BUILD_KNOBS
                        .iter()
                        .map(|knob| format!("{} ({})", knob.name, knob.summary))
                        .collect::<Vec<_>>()
                        .join(", "),
                    OBSERVATION_BLIND
                        .iter()
                        .map(|(blind, _)| *blind)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        let array = raw
            .as_array()
            .with_context(|| format!("sweep: `{name}` must be a list of grid points"))?;
        ensure!(!array.is_empty(), "sweep: `{name}` declares no grid points");
        let mut points = Vec::with_capacity(array.len());
        for element in array {
            let point = match (knob.kind, element.as_integer(), element.as_float()) {
                (ValueKind::Integer, Some(value), _) => GridValue::Integer(value),
                (ValueKind::Float, Some(value), _) => GridValue::Float(value as f64),
                (ValueKind::Float, None, Some(value)) => GridValue::Float(value),
                _ => bail!(
                    "sweep: `{name}` takes {} points, and {:?} is not one",
                    match knob.kind {
                        ValueKind::Integer => "whole-number",
                        ValueKind::Float => "float",
                    },
                    element
                ),
            };
            ensure!(
                !points.contains(&point),
                "sweep: `{name}` declares {} twice; a repeated point compiles the \
                 same dictionary again for a number the table already holds",
                point
            );
            points.push(point);
        }
        Ok(points)
    }

    /// How many points the grid enumerates: the product of every parameter's
    /// points.
    fn point_count(&self) -> usize {
        self.grid.values().map(Vec::len).product()
    }

    /// Enumerates the grid, first declared parameter moving slowest.
    ///
    /// The direction makes the table group by the parameter a reader names
    /// first; the enumeration itself is a pure function of the spec, so two
    /// runs over one spec walk the same points in the same order.
    fn assignments(&self) -> Vec<Assignment> {
        let total = self.point_count();
        let parameters: Vec<(&String, &Vec<GridValue>)> = self.grid.iter().collect();
        let mut assignments = Vec::with_capacity(total);
        for index in 0..total {
            let mut values = BTreeMap::new();
            let mut rest = index;
            // Last parameter first: each division peels one digit off `rest`,
            // so walking the parameters in reverse leaves the first declared
            // parameter as the slowest-moving one.
            for (parameter, points) in parameters.iter().rev() {
                let len = points.len();
                values.insert((*parameter).clone(), points[rest % len].clone());
                rest /= len;
            }
            assignments.push(Assignment::new(values));
        }
        assignments
    }
}

/// Runs the sweep: purity gate, spec, one compile and one measurement per
/// grid point, ranked table, sensitivity summary, recommendation.
///
/// # Errors
///
/// Returns an error when the machine fails the purity gate, when the spec or
/// the holdout set cannot be read, when a compile child fails or its container
/// cannot be measured, and when the xtask binary cannot locate itself.
pub(super) fn run(root: &Path, spec_path: &Path, holdout: &Path) -> Result<()> {
    let check = PurityCheck::read()?;
    check.verify()?;
    println!(
        "sweep: machine clean: load {:.2} on {} cores (ceiling {:.2})",
        check.load_1m,
        check.cpu_count,
        check.ceiling()
    );
    let text = fs::read_to_string(spec_path)
        .with_context(|| format!("sweep: cannot read the spec {}", spec_path.display()))?;
    let spec = SweepSpec::parse(&text)
        .with_context(|| format!("{}: unusable sweep spec", spec_path.display()))?;
    let holdout_path = root.join(holdout);
    let cases = quality::load_holdout(&holdout_path)?;
    ensure!(
        cases.len() >= MIN_HOLDOUT_CASES,
        "sweep: the holdout set holds {} cases, {MIN_HOLDOUT_CASES} are required before \
         a difference between two grid points resolves anything",
        cases.len()
    );
    let assignments = spec.assignments();
    println!(
        "sweep: {} point(s) over {} parameter(s); holdout {} cases from {}",
        assignments.len(),
        spec.grid.len(),
        cases.len(),
        holdout_path.display()
    );
    println!(
        "sweep: each point compiles through dictc and is measured once; the machine has \
         to stay quiet for the whole run"
    );
    let scratch = root.join(SWEEP_DIR);
    fs::create_dir_all(&scratch)
        .with_context(|| format!("sweep: cannot create {}", scratch.display()))?;
    let exe = std::env::current_exe()
        .context("sweep: cannot locate the running xtask binary to drive dictc with")?;
    let mut results = Vec::with_capacity(assignments.len());
    for (index, assignment) in assignments.iter().enumerate() {
        println!(
            "sweep: [{}/{}] {}",
            index + 1,
            assignments.len(),
            assignment.label
        );
        let container = scratch.join(format!("point-{index:04}.dict"));
        let compile = compile_point(&exe, &container, assignment)?;
        let lexicon = FstLexicon::load(&container).with_context(|| {
            format!(
                "sweep: [{}] the container {} cannot be opened",
                assignment.label,
                container.display()
            )
        })?;
        let report = quality::measure(&cases, &lexicon).with_context(|| {
            format!(
                "sweep: [{}] the holdout measurement failed",
                assignment.label
            )
        })?;
        drop(lexicon);
        results.push(PointResult {
            assignment: assignment.clone(),
            metrics: metrics_of(&report),
            compile,
        });
        remove_scratch(&container);
        remove_scratch(&scratch.join(format!("point-{index:04}.dict.manifest.json")));
    }
    report_table(&spec, results)
}

/// Compiles one grid point through the `dictc` subcommand of the running
/// xtask binary, and returns the compile's wall time.
///
/// Only the grid's knobs are passed: every flag the grid does not name falls
/// through to the compiler's own CLI default, which keeps the shipped values
/// in one place (the compiler's argument surface) instead of a second table
/// here.
///
/// # Errors
///
/// Returns an error when the child cannot be spawned or exits non-zero; the
/// child's own log above the error names the cause.
fn compile_point(exe: &Path, container: &Path, assignment: &Assignment) -> Result<Duration> {
    let mut command = Command::new(exe);
    command.arg("dictc");
    command.arg(format!("--output={}", container.display()));
    for (name, value) in &assignment.values {
        let knob = BUILD_KNOBS
            .iter()
            .find(|knob| knob.name == *name)
            .with_context(|| format!("sweep: `{name}` has no dictc flag to carry it"))?;
        command.arg(format!("{}={}", knob.flag, value.as_flag_value()));
    }
    let started = Instant::now();
    let status = command.status().with_context(|| {
        format!(
            "sweep: [{}] cannot start the dictc compile",
            assignment.label
        )
    })?;
    let elapsed = started.elapsed();
    ensure!(
        status.success(),
        "sweep: [{}] the dictc compile exited with {status}; its log above names the cause",
        assignment.label
    );
    Ok(elapsed)
}

/// Extracts the sweep's objective numbers from one quality report.
fn metrics_of(report: &quality::QualityReport) -> PointMetrics {
    PointMetrics {
        top1_overall: report.top1_rate,
        top1_single: report.length_bands.single.top1(),
        top1_two: report.length_bands.two.top1(),
        top1_long: report.length_bands.long.top1(),
        top9_overall: report.top9_rate,
        unreachable: report.unreachable,
    }
}

/// Removes one scratch file, noting what stayed behind rather than failing the
/// run over a temp file.
fn remove_scratch(path: &Path) {
    if let Err(error) = fs::remove_file(path) {
        println!(
            "sweep: note: {} stayed behind in the scratch directory ({error})",
            path.display()
        );
    }
}

/// Prints the ranked table, the sensitivity summary and the recommendation.
///
/// # Errors
///
/// Returns an error when no point produced a measurement, which a validated
/// grid cannot reach and which therefore means the loop above lost one.
fn report_table(spec: &SweepSpec, results: Vec<PointResult>) -> Result<()> {
    let ranked = rank(results);
    let Some(best) = ranked.first() else {
        bail!("sweep: no point produced a measurement");
    };
    println!(
        "sweep: {} point(s) measured, ranked best first (ties: 2字 band, compile time, parameters):",
        ranked.len()
    );
    println!(
        "sweep: {:<4} {:<48} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>9}",
        "#", "parameters", "top1", "2字", "1字", "3+字", "top9", "unreach", "compile"
    );
    for (position, point) in ranked.iter().enumerate() {
        println!("sweep: {:<4} {}", position + 1, point.table_row());
    }
    println!("sweep: sensitivity (best top1 over the points holding each declared value):");
    for (parameter, readings) in sensitivity(&ranked, spec) {
        let lines: Vec<String> = readings.iter().map(ValueReading::line).collect();
        println!("sweep:   {parameter}: {}", lines.join(" | "));
        let measured: Vec<f64> = readings
            .iter()
            .filter_map(|reading| reading.best_top1)
            .collect();
        let lowest = measured
            .iter()
            .copied()
            .min_by(|left, right| left.total_cmp(right));
        let highest = measured
            .iter()
            .copied()
            .max_by(|left, right| left.total_cmp(right));
        if let (Some(lowest), Some(highest)) = (lowest, highest) {
            println!(
                "sweep:   {parameter}: spread {:.2}pp across the declared values",
                (highest - lowest) * 100.0
            );
        }
    }
    println!("sweep: recommendation: {}", best.assignment.label);
    println!(
        "sweep: the sweep recommends only: writing a default back is a human adjudication \
         recorded under docs/dev/research/, and a quality floor may only rise"
    );
    Ok(())
}
