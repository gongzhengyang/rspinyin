//! Locating a point of the hit map, and healing a coordinate that moved.
//!
//! Responsibility: turn a locator -- a cell of the candidate grid, or a point a case computed
//! itself -- into the container-relative point the pointer is aimed at, and say whether the point
//! had to be re-derived. Nothing here talks to a connection: the point this module returns is in
//! the container's own pixels, which is the space the hit map is reported in, and
//! [`crate::testd::coords`] is what moves it onto the screen.
//!
//! # What a cell's centre is derived from
//!
//! The arithmetic is the layout's own, and every number in it comes out of the specification
//! through [`resolve`], so a constant that was renamed moves the point and heals rather than
//! breaking the case. The panel's padding and its header block put the candidate area's origin
//! where it is, and each cell after the first is one cell plus one grid gap further along.
//!
//! The one number that is not derived is the cell's *width*: the layout sizes cells from the
//! text it measured, which a harness that reads only the specification cannot know, so a cell is
//! located at the narrowest width the specification allows. A case whose candidates are wider
//! than that supplies its own point instead, and that is what [`HitRef::Point`] is for.
//!
//! # Why a point is not healed when a value moved
//!
//! A constant whose value changed moves every point derived from it, and that is exactly the
//! drift this module refuses to cover: the point comes back unhealed so that the case fails and
//! the metrics channel's cross-check names the constant that drifted. Only a point that moved
//! for a reason the specification explains -- a rename, or a recording that predates a
//! specification the source now agrees with -- is a recovery.

use crate::testd::ui_metrics::{SlintConstants, SpecTable};

use super::binding::{ConstRef, PER_ROW_LIMIT, ResolvedConst, resolve};
use super::error::HealError;

/// A reference to one point of the hit map, named by what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitRef {
    /// The centre of one candidate cell, in the container's own pixels.
    Cell {
        /// The cell's column, counted from zero along the row.
        column: u16,
        /// The cell's row, counted from zero down the page.
        row: u16,
    },
    /// A point the case computed itself, in the container's own pixels.
    ///
    /// Nothing about a supplied point is derived from the specification, so there is nothing to
    /// re-derive when one drifts; a case that records one has to keep it right itself.
    Point {
        /// The x coordinate, in container pixels.
        x: i32,
        /// The y coordinate, in container pixels.
        y: i32,
    },
}

/// What a case asks a hit locator for, and the point an earlier run recorded for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HitQuery {
    /// The point the case wants, named by what it is.
    pub target: HitRef,
    /// The point an earlier run recorded, when the case is re-checking a snapshot. A derived
    /// point that no longer matches it is a coordinate that drifted, and is healed.
    pub recorded: Option<(i32, i32)>,
}

/// A point of the hit map, located, with the constants its derivation is anchored on.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedHit {
    /// The point in the container's own pixels.
    pub point: (i32, i32),
    /// Whether the locator had to be re-derived: the point moved for a reason the specification
    /// explains, and every constant the derivation used agrees with it.
    pub healed: bool,
    /// The point the case recorded, when the derivation no longer produces it.
    pub moved_from: Option<(i32, i32)>,
    /// The constants the derivation is anchored on, in the order the arithmetic uses them.
    pub constants: Vec<ResolvedConst>,
}

/// Locates one point of the hit map, healing a coordinate that moved.
///
/// # Parameters
///
/// * `query` -- the point the case wants, and the point an earlier run recorded for it.
/// * `slint`, `spec` -- the two sources, as [`resolve`] takes them.
///
/// # Returns
///
/// The point in the container's own pixels and the constants the derivation is anchored on.
/// [`ResolvedHit::healed`] is true only when every constant the derivation used agrees with the
/// specification: a point that moved because a value changed is drift and not a rename, and it
/// comes back unhealed so that the case fails and the cross-check names the constant.
///
/// # Errors
///
/// Returns [`HealError::Unresolvable`] when a constant the derivation needs cannot be resolved,
/// when the cell is one the specification's own per-row ceiling says no window can draw, when the
/// arithmetic leaves the range a container point can hold, or when a point the case supplied
/// itself no longer matches the one it recorded -- nothing about a supplied point is derived from
/// the specification, so there is nothing to re-derive. Returns [`HealError::Drifted`] when a
/// constant the derivation needs was renamed and the declaration that took the name over carries
/// another value: the point a drifted constant produces is not a point the specification explains.
///
/// # Panics
///
/// Never: the arithmetic saturates and every narrowing is checked.
pub fn resolve_hit(
    query: &HitQuery,
    slint: &SlintConstants,
    spec: &SpecTable,
) -> Result<ResolvedHit, HealError> {
    match query.target {
        HitRef::Point { x, y } => supplied(query, (x, y)),
        HitRef::Cell { column, row } => derived(query, column, row, slint, spec),
    }
}

/// The point a case supplied itself, refused when it no longer matches what it recorded.
///
/// # Errors
///
/// Returns [`HealError::Unresolvable`] when the recorded point differs from the supplied one.
///
/// # Panics
///
/// Never.
fn supplied(query: &HitQuery, point: (i32, i32)) -> Result<ResolvedHit, HealError> {
    let drifted = query.recorded.filter(|recorded| *recorded != point);
    if let Some(recorded) = drifted {
        return Err(HealError::Unresolvable {
            wanted: describe_target(query.target),
            detail: format!(
                "the case supplies {} and records {}; a supplied point is not derived from the \
                 specification, so nothing here can re-derive it",
                point_label(point),
                point_label(recorded)
            ),
        });
    }
    Ok(ResolvedHit {
        point,
        healed: false,
        moved_from: None,
        constants: Vec::new(),
    })
}

/// The centre of a candidate cell, derived from the constants the specification fixes.
///
/// # Errors
///
/// Returns [`HealError::Unresolvable`] when a constant the arithmetic needs cannot be resolved,
/// when the column is past the per-row ceiling, or when the arithmetic leaves the range a
/// container point can hold.
///
/// # Panics
///
/// Never: the arithmetic saturates and every narrowing is checked.
fn derived(
    query: &HitQuery,
    column: u16,
    row: u16,
    slint: &SlintConstants,
    spec: &SpecTable,
) -> Result<ResolvedHit, HealError> {
    let limit = resolve(ConstRef::ByName(PER_ROW_LIMIT), slint, spec)?;
    let ceiling = limit
        .anchor
        .number()
        .ok_or_else(|| HealError::Unresolvable {
            wanted: describe_target(query.target),
            detail: format!(
                "the per-row ceiling is stated as {}",
                limit.anchor.value.describe()
            ),
        })?;
    if u32::from(column) >= u32::from(ceiling) {
        return Err(HealError::Unresolvable {
            wanted: describe_target(query.target),
            detail: format!(
                "the specification's per-row ceiling is {ceiling} cells, so column {column} names \
                 a cell no window can draw"
            ),
        });
    }
    let constants = cell_constants(slint, spec)?;
    let point = cell_point(column, row, &constants)?;
    // A recovery is a point that moved for a reason the specification explains. A constant that
    // disagrees with the specification explains nothing -- it is the drift itself -- so a point
    // that moved because of one comes back unhealed.
    let agrees = constants.iter().all(ResolvedConst::matches_spec);
    let renamed = constants.iter().any(|constant| constant.healed);
    let moved_from = query.recorded.filter(|recorded| *recorded != point);
    let healed = agrees && (renamed || moved_from.is_some());
    Ok(ResolvedHit {
        point,
        healed,
        moved_from,
        constants: Vec::from(constants),
    })
}

/// The constants a cell's centre is computed from, in the order the arithmetic uses them.
///
/// # Errors
///
/// Returns [`HealError::Unresolvable`] as [`resolve`] does, for the first one that fails.
///
/// # Panics
///
/// Never.
fn cell_constants(
    slint: &SlintConstants,
    spec: &SpecTable,
) -> Result<[ResolvedConst; 6], HealError> {
    let padding = resolve(ConstRef::ByName("container-padding"), slint, spec)?;
    let header = resolve(ConstRef::ByName("header-height"), slint, spec)?;
    let separator = resolve(ConstRef::ByName("separator-height"), slint, spec)?;
    let width = resolve(ConstRef::ByName("cell-min-width"), slint, spec)?;
    let height = resolve(ConstRef::ByName("cell-height"), slint, spec)?;
    let gap = resolve(ConstRef::ByName("grid-gap"), slint, spec)?;
    Ok([padding, header, separator, width, height, gap])
}

/// The centre of the cell at `column`, `row`, in the container's own pixels.
///
/// The candidate area starts one container padding in from the panel's left edge and one header
/// plus one separator plus one padding down from its top; every cell after the first is one cell
/// plus one grid gap further along, in both directions.
///
/// # Errors
///
/// Returns [`HealError::Unresolvable`] when the sum leaves the range a container point holds.
///
/// # Panics
///
/// Never: every sum saturates.
fn cell_point(
    column: u16,
    row: u16,
    [padding, header, separator, width, height, gap]: &[ResolvedConst; 6],
) -> Result<(i32, i32), HealError> {
    let stride_x = width.value_dp.saturating_add(gap.value_dp);
    let x = padding
        .value_dp
        .saturating_add(u32::from(column).saturating_mul(stride_x))
        .saturating_add(width.value_dp / 2);
    let stride_y = height.value_dp.saturating_add(gap.value_dp);
    let y = header
        .value_dp
        .saturating_add(separator.value_dp)
        .saturating_add(padding.value_dp)
        .saturating_add(u32::from(row).saturating_mul(stride_y))
        .saturating_add(height.value_dp / 2);
    match (i32::try_from(x), i32::try_from(y)) {
        (Ok(x), Ok(y)) => Ok((x, y)),
        _ => Err(HealError::Unresolvable {
            wanted: format!("cell {column},{row}"),
            detail: String::from("the derivation leaves the range a container point can hold"),
        }),
    }
}

/// What a hit locator names, as a report writes it.
///
/// # Panics
///
/// Never.
pub(super) fn describe_target(target: HitRef) -> String {
    match target {
        HitRef::Cell { column, row } => format!("cell {column},{row}"),
        HitRef::Point { x, y } => format!("the point {}", point_label((x, y))),
    }
}

/// A point as a report writes it.
///
/// # Panics
///
/// Never.
pub(super) fn point_label(point: (i32, i32)) -> String {
    format!("{},{}", point.0, point.1)
}
