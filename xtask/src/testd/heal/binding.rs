//! Locating a constant of the metrics block, and re-deriving a name that moved.
//!
//! Responsibility: turn a locator -- a name, or the specification row that fixes one -- into the
//! constant the `.slint` metrics block declares, together with the row it is anchored on.
//! Nothing here renders a record; [`super::report`] does that.
//!
//! # Why a locator is anchored on a row rather than on a value
//!
//! The specification fixes values and the source declares names, and the two are held together
//! by the pairing in [`BINDINGS`]. A locator is therefore answered from the document first: the
//! row states what the value is, and the source is only asked whether it declares that value
//! under the name. That ordering is what makes a rename healable at all -- the value is known
//! without the name -- and it is also what makes a *value* change unhealable, because a
//! declaration that no longer carries the value the row states fails the one test every
//! candidate has to pass.
//!
//! # Two rows may fix one constant
//!
//! The document states the container padding twice, once for the panel and once for the
//! candidate area, and the grid gap twice, once per column and once per row. Both rows fix the
//! same declaration, so a name resolves against the first of them and the rest have to agree;
//! two rows that state different values for one constant are a contradiction in the document
//! itself, and a locator refuses rather than picking a side.
//!
//! # When the name and the value both moved
//!
//! A name that moved on its own is a rename and is re-derived. A value that moved on its own is
//! drift: the declaration is found by name, comes back unhealed, and the metrics cross-check
//! reports the disagreement. A name that moved together with its value is neither, and it is
//! refused as [`HealError::Drifted`] rather than as an unresolvable name -- a reader has to be
//! able to tell a locator that lost its constant, which is a test script to repair, from a
//! constant that was replaced by one stating another value, which is the product.
//!
//! # Why one re-derivation is the last one
//!
//! A re-derivation is a function of the two sources and of nothing else: it never reads what an
//! earlier pass wrote, so resolving one locator twice answers the same thing twice and a pass has
//! no state to accumulate. The candidate test is exact -- a run of words, and the value the row
//! states in the unit the row states it in -- so a heal can never land on a name that merely
//! resembles the one that went missing and drift a little further on the next pass. When nothing
//! survives both tests the locator refuses, and the pass stops there instead of iterating.

use std::collections::BTreeMap;

use crate::testd::ui_metrics::{GEOMETRY_SECTION, MetricValue, SlintConstants, SpecTable, Unit};

use super::error::HealError;
use super::words::{is_rename_of, words};

/// The name the metrics block declares the per-row ceiling under.
///
/// It is the one bound the cell locator takes from the specification rather than from the
/// configuration: the configuration may ask for fewer cells in a row, never for more, so a
/// column past this limit names a cell no window can draw.
pub(super) const PER_ROW_LIMIT: &str = "max-per-row-limit";

/// One constant of the metrics block, and the row of the specification that fixes it.
struct Binding {
    /// The name the metrics block declares, in the kebab-case spelling the specification's
    /// constant column uses.
    name: &'static str,
    /// The row's element label, as the document spells it.
    row: &'static str,
    /// The unit the row states the value in.
    unit: Unit,
    /// Which of the row's values of that unit, counted from zero.
    index: usize,
}

/// Every constant of the metrics block a locator may name, and the row that fixes it.
///
/// This is a mapping and not a second copy of the specification: the values are read out of the
/// document by [`SpecTable`] every time, and only the pairing of a declaration's name with the
/// row that states it is written down here. The pairing is by the document's own element label
/// rather than by position, so a row that moves is still found, and a row that is renamed is
/// reported as gone rather than compared against whatever now sits where it used to.
///
/// Every row a locator may name is a row of the geometry table, which is why the section is this
/// module's own constant rather than a field of each entry.
static BINDINGS: &[Binding] = &[
    Binding {
        name: "shadow-margin",
        row: "窗口外边距（阴影预留）",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "container-radius",
        row: "容器圆角",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "stroke-width",
        row: "容器描边",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "container-padding",
        row: "容器内边距",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "container-padding",
        row: "候选区内边距",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "min-width",
        row: "候选框最小宽度",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "max-width",
        row: "候选框最大宽度",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "header-height",
        row: "Header 高度",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "header-height-compact",
        row: "Header 高度",
        unit: Unit::Dp,
        index: 1,
    },
    Binding {
        name: "header-padding-h",
        row: "Header 水平内边距",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "font-size-header",
        row: "Header 字号/字重",
        unit: Unit::Sp,
        index: 0,
    },
    Binding {
        name: "font-size-small",
        row: "Header 字号/字重",
        unit: Unit::Sp,
        index: 2,
    },
    Binding {
        name: "header-icon-size",
        row: "Header 状态图标",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "header-icon-gap",
        row: "Header 状态图标",
        unit: Unit::Dp,
        index: 2,
    },
    Binding {
        name: "header-text-gap",
        row: "Header 状态图标",
        unit: Unit::Dp,
        index: 3,
    },
    Binding {
        name: "separator-height",
        row: "Header 分隔线",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "cell-min-width",
        row: "候选单元最小宽度",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "cell-height",
        row: "候选单元高度",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "cell-padding-h",
        row: "候选单元内边距",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "cell-padding-v",
        row: "候选单元内边距",
        unit: Unit::Dp,
        index: 1,
    },
    Binding {
        name: "cell-radius",
        row: "候选单元圆角",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "font-size-small",
        row: "候选序号字号",
        unit: Unit::Sp,
        index: 0,
    },
    Binding {
        name: "number-gap",
        row: "候选序号字号",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "font-size-cell",
        row: "候选文本字号/字重",
        unit: Unit::Sp,
        index: 0,
    },
    Binding {
        name: "font-size-small",
        row: "候选注音字号",
        unit: Unit::Sp,
        index: 0,
    },
    Binding {
        name: "annotation-gap",
        row: "候选注音字号",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "grid-gap",
        row: "网格列间距",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "grid-gap",
        row: "网格行间距",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "max-per-row",
        row: "单行最大候选数",
        unit: Unit::Count,
        index: 0,
    },
    Binding {
        name: "min-per-row",
        row: "单行最大候选数",
        unit: Unit::Count,
        index: 1,
    },
    Binding {
        name: PER_ROW_LIMIT,
        row: "单行最大候选数",
        unit: Unit::Count,
        index: 2,
    },
    Binding {
        name: "cursor-arrow-height",
        row: "光标指示箭头",
        unit: Unit::Dp,
        index: 0,
    },
    Binding {
        name: "cursor-arrow-width",
        row: "光标指示箭头",
        unit: Unit::Dp,
        index: 1,
    },
];

/// A reference to one constant of the metrics block.
///
/// The name is preferred because it is what a case reads out of the source; the row is what a
/// case falls back to, and what every resolution is anchored on. A name is compared as the words
/// it is spelled with, so either spelling the project uses -- the kebab-case one the
/// specification's constant column writes, or the PascalCase one a component declares -- names
/// the same constant. No other spelling matches.
///
/// The borrow is not tied to a literal: a case may build a locator from a name it read out of a
/// file, which is why the reference borrows rather than demanding a `'static` string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstRef<'a> {
    /// The constant's own name, as the metrics block declares it.
    ByName(&'a str),
    /// The specification row that fixes the constant, by heading and element label.
    BySpecRow {
        /// The heading of the section the row lives in, in full or by its number.
        section: &'a str,
        /// The row's element label, as the document spells it.
        row: &'a str,
    },
}

/// The specification row a constant is anchored on, and the value it states.
#[derive(Clone, Debug, PartialEq)]
pub struct SpecAnchor {
    /// The heading of the section the row lives in, as the document spells it.
    pub section: &'static str,
    /// The row's element label, as the document spells it.
    pub row: &'static str,
    /// The unit the row states the value in.
    pub unit: Unit,
    /// Which of the row's values of that unit, counted from zero.
    pub index: usize,
    /// The value the row states.
    pub value: MetricValue,
}

impl SpecAnchor {
    /// The value the row states, as the whole number a comparison is made on.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn number(&self) -> Option<u16> {
        self.value.number()
    }
}

/// One constant of the metrics block, located, with the row it is anchored on.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedConst {
    /// The name the metrics block declares the constant under now.
    pub name: String,
    /// The value the declaration carries, in the unit the row states it in: device-independent
    /// pixels for a length row, scaled points for a font-size row, a whole count for a count row.
    /// [`ResolvedConst::anchor`] says which.
    pub value_dp: u32,
    /// Whether the name had to be re-derived from the specification.
    pub healed: bool,
    /// The specification row the resolution is anchored on.
    pub anchor: SpecAnchor,
    /// The name the locator used, when the metrics block no longer declares it.
    pub renamed_from: Option<&'static str>,
}

impl ResolvedConst {
    /// Whether the value the declaration carries is the value the specification states.
    ///
    /// A healed constant always agrees, because agreement is what qualified it as a candidate. An
    /// unhealed one may not, and a caller that finds it does not is looking at drift: the
    /// cross-check of the metrics channel is what reports it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn matches_spec(&self) -> bool {
        self.anchor.number().map(u32::from) == Some(self.value_dp)
    }
}

/// Locates one constant of the metrics block, healing a name that moved.
///
/// # Parameters
///
/// * `reference` -- what the case named: the constant itself, or the specification row that
///   fixes it.
/// * `slint` -- the metrics block, as the caller read it.
/// * `spec` -- the specification tables, as the caller read them.
///
/// # Returns
///
/// The constant, with the row it is anchored on. [`ResolvedConst::healed`] is true when the name
/// had to be re-derived. The value is the one the declaration carries, which a healed constant
/// has already been checked against the specification and an unhealed one may disagree with --
/// that disagreement is the cross-check's to report, not this module's to hide.
///
/// # Errors
///
/// Returns [`HealError::Unresolvable`] when the name is one no row of the specification fixes,
/// when the row does not exist or states no value of the unit the constant is read in, when the
/// row fixes more than one constant, when two rows that fix one constant state different values,
/// or when a missing name cannot be re-derived to exactly one candidate; and
/// [`HealError::Drifted`] when exactly one declaration took the name over carrying another value.
///
/// # Panics
///
/// Never: every lookup is a total function of the two sources.
pub fn resolve(
    reference: ConstRef<'_>,
    slint: &SlintConstants,
    spec: &SpecTable,
) -> Result<ResolvedConst, HealError> {
    let (binding, anchor) = binding_for(reference, spec)?;
    match slint.value(binding.name) {
        Some(declared) => Ok(ResolvedConst {
            name: binding.name.to_owned(),
            value_dp: number_of(declared, binding.name)?,
            healed: false,
            anchor,
            renamed_from: None,
        }),
        None => rederive(binding.name, anchor, slint),
    }
}

/// The constant a locator names, with the row of the specification that fixes it.
///
/// # Errors
///
/// Returns [`HealError::Unresolvable`] when no row of the specification fixes the name, when the
/// row does not exist, when the row fixes more than one constant, or when two rows that fix one
/// constant state different values.
///
/// # Panics
///
/// Never.
fn binding_for(
    reference: ConstRef<'_>,
    spec: &SpecTable,
) -> Result<(&'static Binding, SpecAnchor), HealError> {
    let wanted = describe_reference(reference);
    let found: Vec<&'static Binding> = match reference {
        ConstRef::ByName(name) => {
            let name = words(name);
            BINDINGS
                .iter()
                .filter(|binding| words(binding.name) == name)
                .collect()
        }
        ConstRef::BySpecRow { section, row } => BINDINGS
            .iter()
            .filter(|binding| headings_agree(GEOMETRY_SECTION, section) && binding.row == row)
            .collect(),
    };
    let Some(first) = found.first().copied() else {
        return Err(HealError::Unresolvable {
            wanted,
            detail: String::from(
                "no row of the specification fixes it, and the harness knows no constant by that \
                 name",
            ),
        });
    };
    // A row that states two values names two constants, and a locator that names the row alone
    // has not said which of them it means. One constant two rows state is a different case: the
    // rows agree or they do not, and the loop below is what checks that.
    if matches!(reference, ConstRef::BySpecRow { .. }) && found.len() > 1 {
        return Err(HealError::Unresolvable {
            wanted,
            detail: format!(
                "the row fixes {} constants, so name the constant itself",
                found.len()
            ),
        });
    }
    let anchor = anchor_of(first, spec)?;
    for other in found.iter().skip(1) {
        let stated = anchor_of(other, spec)?;
        if stated.number() != anchor.number() {
            return Err(HealError::Unresolvable {
                wanted,
                detail: format!(
                    "the rows `{}` and `{}` both fix `{}` but state different values",
                    first.row, other.row, first.name
                ),
            });
        }
    }
    Ok((first, anchor))
}

/// The row of the specification a binding is anchored on, and the value it states.
///
/// # Errors
///
/// Returns [`HealError::Unresolvable`] when the document has no such row, or when the row states
/// no value of the unit the constant is read in at the position the binding names.
///
/// # Panics
///
/// Never.
fn anchor_of(binding: &Binding, spec: &SpecTable) -> Result<SpecAnchor, HealError> {
    let wanted = format!("the constant {}", binding.name);
    let row = spec
        .row(binding.row)
        .ok_or_else(|| HealError::Unresolvable {
            wanted: wanted.clone(),
            detail: format!("the specification has no row `{}`", binding.row),
        })?;
    let value = row
        .value(binding.unit, binding.index)
        .ok_or_else(|| HealError::Unresolvable {
            wanted,
            detail: format!(
                "the row `{}` states no {} value at position {}",
                binding.row,
                unit_name(binding.unit),
                binding.index
            ),
        })?;
    Ok(SpecAnchor {
        section: GEOMETRY_SECTION,
        row: binding.row,
        unit: binding.unit,
        index: binding.index,
        value,
    })
}

/// The constant a locator names, re-derived from the specification after its name went away.
///
/// # Errors
///
/// Returns [`HealError::Unresolvable`] when the row states no whole number, when no declaration is
/// built on the name the locator used, or when more than one candidate survives both tests; and
/// [`HealError::Drifted`] when exactly one declaration took the name over carrying another value.
///
/// # Panics
///
/// Never.
fn rederive(
    old: &'static str,
    anchor: SpecAnchor,
    slint: &SlintConstants,
) -> Result<ResolvedConst, HealError> {
    let unresolvable = |detail: String| HealError::Unresolvable {
        wanted: format!("the constant {old}"),
        detail,
    };
    let Some(wanted) = anchor.number() else {
        return Err(unresolvable(format!(
            "the row it is anchored on states {}",
            anchor.value.describe()
        )));
    };
    // The first test: a declaration whose name is the one the locator lost with words added at
    // either end, which is the rename the project's own vocabulary produces.
    let mut named: NameGroups<'_> = BTreeMap::new();
    for name in slint.names() {
        if name != old && is_rename_of(old, name) {
            named.entry(words(name)).or_default().push(name);
        }
    }
    // The second test: the value the row states, in the unit the row states it in. A declaration
    // that fails this one is not a candidate at all, which is what keeps a rename from covering a
    // value that moved.
    let mut carrying: NameGroups<'_> = BTreeMap::new();
    for name in spelled_out(&named) {
        if slint
            .value(name)
            .is_some_and(|value| matches_row(&anchor, value))
        {
            carrying.entry(words(name)).or_default().push(name);
        }
    }
    if carrying.len() > 1 {
        return Err(unresolvable(ambiguous_report(&carrying, old, wanted)));
    }
    let Some(group) = carrying.into_values().next() else {
        return Err(not_carried(old, wanted, &anchor, &named, slint));
    };
    let chosen = group
        .iter()
        .find(|name| name.contains('-') == old.contains('-'))
        .or_else(|| group.first())
        .copied();
    let Some(name) = chosen else {
        return Err(unresolvable(format!(
            "no declaration carries the {wanted} the row fixes under a name built on `{old}`"
        )));
    };
    Ok(ResolvedConst {
        name: name.to_owned(),
        value_dp: u32::from(wanted),
        healed: true,
        anchor,
        renamed_from: Some(old),
    })
}

/// Whether a declaration states the value the anchor's row fixes, in the unit the row states it.
///
/// # Panics
///
/// Never.
fn matches_row(anchor: &SpecAnchor, declared: &MetricValue) -> bool {
    states(anchor.unit, declared) && declared.number() == anchor.number()
}

/// Whether a declaration states its value in `unit`.
///
/// A row states a value in one unit and a declaration carries one in its own: a font size stated
/// in scaled points is not the value a length stated in device-independent pixels is, even when
/// the two numbers are equal. Comparing the numbers alone would let a rename land on a
/// declaration of another unit and report it as the constant the row fixes.
///
/// # Panics
///
/// Never.
fn states(unit: Unit, declared: &MetricValue) -> bool {
    matches!(
        (unit, declared),
        (Unit::Dp, MetricValue::LengthDp(_))
            | (Unit::Sp, MetricValue::FontSp(_))
            | (Unit::Count, MetricValue::Count(_))
    )
}

/// The declarations of a metrics block, grouped by the words their names spell.
///
/// One group is one constant: the metrics reader indexes a `public constant` under its own spelling
/// and under the kebab-case one the specification uses, so a single declaration can be found twice,
/// and the spelling that matches the locator's own is the one a report names.
type NameGroups<'a> = BTreeMap<Vec<String>, Vec<&'a str>>;

/// Every name the groups hold, in group order.
///
/// # Panics
///
/// Never.
fn spelled_out<'a>(groups: &NameGroups<'a>) -> Vec<&'a str> {
    groups
        .values()
        .flat_map(|group| group.iter().copied())
        .collect()
}

/// The refusal a name that moved earns when no declaration built on it carries the row's value.
///
/// One such declaration is drift and not a rename -- the name moved and the value moved with it --
/// and it is refused as [`HealError::Drifted`], so that a report can say which side moved. None,
/// or more than one, leaves the question open, and the locator refuses without a guess.
///
/// # Panics
///
/// Never.
fn not_carried(
    old: &'static str,
    wanted: u16,
    anchor: &SpecAnchor,
    named: &NameGroups<'_>,
    slint: &SlintConstants,
) -> HealError {
    let sole = if named.len() == 1 {
        named
            .values()
            .next()
            .and_then(|group| group.first().copied())
    } else {
        None
    };
    match sole.and_then(|name| slint.value(name).map(|declared| (name, declared))) {
        Some((name, declared)) => HealError::Drifted {
            name: old.to_owned(),
            declared_name: name.to_owned(),
            declared: declared.describe(),
            spec: anchor.value.describe(),
        },
        None => HealError::Unresolvable {
            wanted: format!("the constant {old}"),
            detail: no_candidate_report(named, old, wanted),
        },
    }
}

/// Why no declaration built on a name carries the value the row fixes, in a form a developer can
/// act on.
///
/// # Panics
///
/// Never.
fn no_candidate_report(named: &NameGroups<'_>, old: &str, wanted: u16) -> String {
    let names = spelled_out(named);
    if names.is_empty() {
        return format!(
            "no declaration is built on `{old}`, so either the constant was removed or its name \
             was changed beyond recognition"
        );
    }
    format!(
        "{} declaration(s) are built on `{old}` and none carries the {wanted} the row fixes: {}",
        names.len(),
        names.join(", ")
    )
}

/// Why more than one declaration is a candidate, in a form a developer can act on.
///
/// # Panics
///
/// Never.
fn ambiguous_report(carrying: &NameGroups<'_>, old: &str, wanted: u16) -> String {
    let names = spelled_out(carrying);
    format!(
        "{} declarations carry the {wanted} under a name built on `{old}`, so the name alone \
         cannot tell them apart: {}",
        names.len(),
        names.join(", ")
    )
}

/// The whole number a declaration carries.
///
/// # Errors
///
/// Returns [`HealError::Unresolvable`] when the declaration is not a whole number.
///
/// # Panics
///
/// Never.
fn number_of(declared: &MetricValue, name: &str) -> Result<u32, HealError> {
    declared
        .number()
        .map(u32::from)
        .ok_or_else(|| HealError::Unresolvable {
            wanted: format!("the constant {name}"),
            detail: format!(
                "the metrics block declares it as {}, which is not a whole number",
                declared.describe()
            ),
        })
}

/// What a locator names, as a report writes it.
///
/// # Panics
///
/// Never.
fn describe_reference(reference: ConstRef<'_>) -> String {
    match reference {
        ConstRef::ByName(name) => format!("the constant {name}"),
        ConstRef::BySpecRow { section, row } => {
            format!("the row `{row}` of {}", bare_heading(section))
        }
    }
}

/// Whether two section headings name the same section.
///
/// A locator may write the section by its number, with the hashes of a Markdown heading, or as
/// the heading in full, so the comparison drops the hashes and then asks whether either text
/// begins the other; nothing else about the two is forgiven.
///
/// # Panics
///
/// Never.
fn headings_agree(left: &str, right: &str) -> bool {
    let (left, right) = (bare_heading(left), bare_heading(right));
    left.starts_with(right) || right.starts_with(left)
}

/// A heading with its hashes removed, as a report writes it.
///
/// # Panics
///
/// Never.
pub(super) fn bare_heading(section: &str) -> &str {
    section.trim_start_matches('#').trim_start()
}

/// The unit a row states a value in, as a report writes it.
///
/// The suffix the document itself writes is empty for a count, so a report names that unit
/// rather than leaving a gap where the unit would be.
///
/// # Panics
///
/// Never.
pub(super) fn unit_name(unit: Unit) -> &'static str {
    match unit {
        Unit::Dp => "dp",
        Unit::Sp => "sp",
        Unit::Count => "count",
    }
}
