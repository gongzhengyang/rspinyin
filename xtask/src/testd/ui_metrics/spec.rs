//! The specification tables of `features.md` 3.1 and 3.2, as the harness reads them.
//!
//! Responsibility: turn the three tables that fix the candidate window's geometry, its grid
//! exceptions and its colour tokens into values a comparison can be made against. Nothing here
//! decides what a mismatch is; that is [`super::cross_check_spec`]'s job.
//!
//! # Why the columns are anchored
//!
//! `scripts/check-ui-spec.sh` reads the same three tables and owns the completeness direction --
//! a row the document gained and nothing checks. This module reads them for the opposite
//! direction: a value the *running window* and the `.slint` source must agree with the document
//! about. Both readers anchor on the column names rather than on a row's position, because a
//! table whose columns were reordered is a table whose cells now mean something else, and a
//! positional reader would compare a size against a colour without noticing. Neither reader
//! restates a value the other states: the numbers come out of the document in both cases.

use std::collections::BTreeMap;

use ime_types::Rgba8;

use super::error::MetricError;
use super::value::{MetricValue, Unit};

/// The heading that opens the geometry table.
pub const GEOMETRY_SECTION: &str = "#### 3.1.1";

/// The heading that opens the grid, its exception table and the type scale.
pub const GRID_SECTION: &str = "#### 3.1.4";

/// The heading that opens the colour tokens.
pub const COLOUR_SECTION: &str = "### 3.2";

/// The columns the geometry table is read by.
const GEOMETRY_COLUMNS: [&str; 2] = ["元素", "规格"];

/// The columns the exception table is read by.
const EXCEPTION_COLUMNS: [&str; 3] = ["例外值", "常量", "理由"];

/// The columns the colour table is read by.
const COLOUR_COLUMNS: [&str; 4] = ["Token", "暗色", "亮色", "用途"];

/// One row of the 3.1.1 geometry table.
///
/// The row is kept whole rather than reduced to one number: several rows state more than one
/// value, in more than one unit, and which of them belongs to which constant is a mapping that
/// belongs to the reader of the row and not to the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeometryRow {
    /// The row's element label, as the document writes it.
    pub label: String,
    /// Every `<number>dp` the row spells, in the order it spells them.
    pub dp: Vec<u16>,
    /// Every `<number>sp` the row spells, in the order it spells them.
    pub sp: Vec<u16>,
    /// Every whole number the row spells, in the order it spells them.
    ///
    /// A count carries no unit, so this is the list a row such as `单行最大候选数` is read from.
    pub counts: Vec<u16>,
    /// The range the row states for a value the configuration may change, as `可配置 A~B`.
    ///
    /// A row that states a range fixes a *default* rather than a value: a window configured to
    /// the top of the range does not disagree with the document, and a reader that compared the
    /// two for equality would report every legitimate configuration as drift.
    pub configurable: Option<(u16, u16)>,
}

impl GeometryRow {
    /// The value the row states in `unit`, at `index`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn value(&self, unit: Unit, index: usize) -> Option<MetricValue> {
        let values = match unit {
            Unit::Dp => &self.dp,
            Unit::Sp => &self.sp,
            Unit::Count => &self.counts,
        };
        let value = *values.get(index)?;
        Some(match unit {
            Unit::Dp => MetricValue::LengthDp(value),
            Unit::Sp => MetricValue::FontSp(value),
            Unit::Count => MetricValue::Count(value),
        })
    }

    /// Whether `value` falls inside the range the row states.
    ///
    /// A row that states no range answers `None`: the question does not apply, which is a
    /// different answer from "outside the range" and must not be read as one.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn contains(&self, value: MetricValue) -> Option<bool> {
        let (min, max) = self.configurable?;
        let number = value.number()?;
        Some(number >= min && number <= max)
    }
}

/// One cell of the 3.2 colour table.
///
/// A cell is a colour, an alpha fraction, or both: `#1C1C1E @ 0.85` states the bytes and the
/// acrylic alpha, while `accent @ 0.18` states only a fraction of another token.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColourCell {
    /// The colour the cell spells, when it spells one.
    pub colour: Option<Rgba8>,
    /// The alpha fraction the cell spells beside it, when it spells one.
    pub alpha: Option<f32>,
}

/// One row of the 3.2 colour table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColourRow {
    /// The dark-scheme cell.
    pub dark: ColourCell,
    /// The light-scheme cell.
    pub light: ColourCell,
}

/// The specification tables of `features.md` 3.1 and 3.2.
#[derive(Clone, Debug, PartialEq)]
pub struct SpecTable {
    /// The 3.1.1 rows, in document order.
    geometry: Vec<GeometryRow>,
    /// The 3.1.4 exceptions, keyed by the constant name the row names.
    exceptions: BTreeMap<String, u16>,
    /// The 3.2 rows, keyed by token.
    colours: BTreeMap<String, ColourRow>,
}

impl SpecTable {
    /// Reads the three tables out of the text of `docs/dev/features.md`.
    ///
    /// # Errors
    ///
    /// Returns [`MetricError::SpecUnparsable`] when a heading the reader anchors on is absent,
    /// when a section holds a number of tables other than one, when a table's header is not the
    /// columns this reader expects, or when a row's cell count disagrees with the header. Every
    /// one of those is a structure change, and every one of them would otherwise leave the
    /// reader with a smaller table that every assertion against it passes.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn parse(document: &str) -> Result<Self, MetricError> {
        Ok(Self {
            geometry: geometry_rows(&section(document, GEOMETRY_SECTION)?)?,
            exceptions: exception_rows(&section(document, GRID_SECTION)?)?,
            colours: colour_rows(&section(document, COLOUR_SECTION)?)?,
        })
    }

    /// The 3.1.1 rows, in document order.
    pub fn geometry(&self) -> &[GeometryRow] {
        &self.geometry
    }

    /// The 3.1.1 row whose label is `label`.
    pub fn row(&self, label: &str) -> Option<&GeometryRow> {
        self.geometry.iter().find(|row| row.label == label)
    }

    /// The 3.1.4 exceptions, keyed by constant name.
    pub fn exceptions(&self) -> &BTreeMap<String, u16> {
        &self.exceptions
    }

    /// The value 3.1.4 lists `name` as an exception for.
    pub fn exception(&self, name: &str) -> Option<u16> {
        self.exceptions.get(name).copied()
    }

    /// The 3.2 row for `token`.
    pub fn colour(&self, token: &str) -> Option<&ColourRow> {
        self.colours.get(token)
    }

    /// The 3.2 rows, keyed by token.
    pub fn colours(&self) -> &BTreeMap<String, ColourRow> {
        &self.colours
    }
}

/// The body of the section `opening` introduces, up to the next heading.
///
/// # Errors
///
/// Returns [`MetricError::SpecUnparsable`] when the document has no such section.
///
/// # Panics
///
/// Never.
fn section(document: &str, opening: &str) -> Result<String, MetricError> {
    let mut body = String::new();
    let mut inside = false;
    for line in document.lines() {
        if !inside {
            if line.trim_start().starts_with(opening) {
                inside = true;
            }
            continue;
        }
        if is_heading(line) {
            break;
        }
        body.push_str(line);
        body.push('\n');
    }
    if !inside {
        return Err(MetricError::SpecUnparsable {
            document: super::SPEC_DOCUMENT,
            detail: format!("it no longer has the section {opening}"),
        });
    }
    Ok(body)
}

/// Whether `line` opens a Markdown heading of a depth this reader treats as a boundary.
///
/// # Panics
///
/// Never.
fn is_heading(line: &str) -> bool {
    let hashes = line.chars().take_while(|ch| *ch == '#').count();
    (2..=4).contains(&hashes) && line.chars().nth(hashes) == Some(' ')
}

/// The rows of the one table `body` holds, with its header checked against `columns`.
///
/// # Errors
///
/// Returns [`MetricError::SpecUnparsable`] when the section holds no table or more than one,
/// when the header row is not `columns`, when the row after it is not a separator, or when a
/// data row's cell count disagrees with the header.
///
/// # Panics
///
/// Never.
fn table(body: &str, columns: &[&str], place: &str) -> Result<Vec<Vec<String>>, MetricError> {
    let malformed = |detail: String| MetricError::SpecUnparsable {
        document: super::SPEC_DOCUMENT,
        detail: format!("{place}: {detail}"),
    };
    let mut tables = tables(body);
    if tables.len() != 1 {
        return Err(malformed(format!(
            "the section holds {} tables, expected exactly one",
            tables.len()
        )));
    }
    let Some(rows) = tables.pop() else {
        return Err(malformed(String::from("the section holds no table")));
    };
    let expected = columns.join(" | ");
    let Some(header) = rows.first() else {
        return Err(malformed(format!(
            "the table has no header row; expected | {expected} |"
        )));
    };
    let header: Vec<&str> = header.iter().map(String::as_str).collect();
    if header != columns {
        return Err(malformed(format!(
            "the table's columns are | {} |, expected | {expected} |",
            header.join(" | ")
        )));
    }
    if !rows.get(1).is_some_and(|row| is_separator(row)) {
        let detail = String::from("the header row is not followed by a separator row");
        return Err(malformed(detail));
    }
    let mut data = Vec::new();
    for (index, row) in rows.iter().enumerate().skip(2) {
        if row.len() != columns.len() {
            return Err(malformed(format!(
                "row {} has {} cells, expected {}",
                index + 1,
                row.len(),
                columns.len()
            )));
        }
        data.push(row.clone());
    }
    Ok(data)
}

/// Every table `body` holds, as rows of cells.
///
/// A table is a run of consecutive lines that start with a pipe; a line that does not ends it.
///
/// # Panics
///
/// Never.
fn tables(body: &str) -> Vec<Vec<Vec<String>>> {
    let mut tables: Vec<Vec<Vec<String>>> = Vec::new();
    let mut current: Vec<Vec<String>> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('|') {
            current.push(cells(trimmed));
            continue;
        }
        if !current.is_empty() {
            tables.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tables.push(current);
    }
    tables
}

/// The cells of a Markdown table row, with the outer pipes removed.
///
/// # Panics
///
/// Never.
fn cells(row: &str) -> Vec<String> {
    row.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect()
}

/// Whether a row is the `|---|---|` line that separates a header from a body.
///
/// # Panics
///
/// Never.
fn is_separator(row: &[String]) -> bool {
    !row.is_empty()
        && row.iter().all(|cell| {
            let trimmed = cell.trim_matches(':');
            trimmed.len() >= 2 && trimmed.chars().all(|ch| ch == '-')
        })
}

/// Reads the 3.1.1 geometry rows.
///
/// # Errors
///
/// As [`table`], plus [`MetricError::SpecUnparsable`] when the table has no data rows.
///
/// # Panics
///
/// Never.
fn geometry_rows(body: &str) -> Result<Vec<GeometryRow>, MetricError> {
    let mut rows = Vec::new();
    for row in table(body, &GEOMETRY_COLUMNS, "features.md 3.1.1")? {
        let spec = &row[1];
        rows.push(GeometryRow {
            label: row[0].clone(),
            dp: numbers(spec, "dp"),
            sp: numbers(spec, "sp"),
            counts: integers(spec),
            configurable: configurable(spec),
        });
    }
    if rows.is_empty() {
        return Err(MetricError::SpecUnparsable {
            document: super::SPEC_DOCUMENT,
            detail: String::from("features.md 3.1.1's table has no data rows"),
        });
    }
    Ok(rows)
}

/// Reads the 3.1.4 exception rows, keyed by constant name.
///
/// # Errors
///
/// As [`table`], plus [`MetricError::SpecUnparsable`] when no row states an exception.
///
/// # Panics
///
/// Never.
fn exception_rows(body: &str) -> Result<BTreeMap<String, u16>, MetricError> {
    let mut exceptions = BTreeMap::new();
    for row in table(body, &EXCEPTION_COLUMNS, "features.md 3.1.4")? {
        // A row whose first cell is not a `Ndp` exception is prose the table carries rather than
        // a declaration; there is nothing to anchor a value on, so it is passed over.
        let Some(value) = single_number(&row[0], "dp") else {
            continue;
        };
        for name in backticked(&row[1]) {
            exceptions.insert(name, value);
        }
    }
    if exceptions.is_empty() {
        return Err(MetricError::SpecUnparsable {
            document: super::SPEC_DOCUMENT,
            detail: String::from("features.md 3.1.4's exception table names no constant"),
        });
    }
    Ok(exceptions)
}

/// Reads the 3.2 colour rows, keyed by token.
///
/// # Errors
///
/// As [`table`], plus [`MetricError::SpecUnparsable`] when a cell is not a colour this reader
/// knows or when the table has no data rows.
///
/// # Panics
///
/// Never.
fn colour_rows(body: &str) -> Result<BTreeMap<String, ColourRow>, MetricError> {
    let mut colours = BTreeMap::new();
    for row in table(body, &COLOUR_COLUMNS, "features.md 3.2")? {
        colours.insert(
            unquote(&row[0]),
            ColourRow {
                dark: colour_cell(&row[1])?,
                light: colour_cell(&row[2])?,
            },
        );
    }
    if colours.is_empty() {
        return Err(MetricError::SpecUnparsable {
            document: super::SPEC_DOCUMENT,
            detail: String::from("features.md 3.2's table has no data rows"),
        });
    }
    Ok(colours)
}

/// Reads one 3.2 cell.
///
/// # Errors
///
/// Returns [`MetricError::SpecUnparsable`] when the cell is not one of the four forms the table
/// uses: a hex literal, a hex literal with an alpha, an `rgba()` call, or a fraction of the
/// accent token.
///
/// # Panics
///
/// Never.
fn colour_cell(cell: &str) -> Result<ColourCell, MetricError> {
    let text = unquote(cell);
    let unsupported = |text: &str| MetricError::SpecUnparsable {
        document: super::SPEC_DOCUMENT,
        detail: format!("features.md 3.2 spells a colour as {text:?}"),
    };
    if let Some((colour, alpha)) = text.split_once('@') {
        let fraction = alpha
            .trim()
            .parse::<f32>()
            .map_err(|_| unsupported(&text))?;
        let colour = colour.trim();
        if colour == "accent" {
            return Ok(ColourCell {
                colour: None,
                alpha: Some(fraction),
            });
        }
        return Ok(ColourCell {
            colour: Some(hex(colour).ok_or_else(|| unsupported(&text))?),
            alpha: Some(fraction),
        });
    }
    if let Some(channels) = text
        .strip_prefix("rgba(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let parts: Vec<&str> = channels.split(',').map(str::trim).collect();
        let [red, green, blue, alpha] = parts.as_slice() else {
            return Err(unsupported(&text));
        };
        let fraction = alpha.parse::<f32>().map_err(|_| unsupported(&text))?;
        let channel = |value: &str| value.parse::<u8>().map_err(|_| unsupported(&text));
        return Ok(ColourCell {
            colour: Some(Rgba8 {
                r: channel(red)?,
                g: channel(green)?,
                b: channel(blue)?,
                // The `rgba()` builtin truncates the fraction, which is the conversion the Slint
                // runtime applies and the one `scripts/check-ui-spec.sh` mirrors.
                a: truncate_alpha(fraction),
            }),
            alpha: Some(fraction),
        });
    }
    if text == "accent.default" {
        return Ok(ColourCell {
            colour: None,
            alpha: None,
        });
    }
    Ok(ColourCell {
        colour: Some(hex(&text).ok_or_else(|| unsupported(&text))?),
        alpha: None,
    })
}

/// The bytes a `#RRGGBB` literal spells, with a fully opaque alpha channel.
///
/// # Panics
///
/// Never.
fn hex(text: &str) -> Option<Rgba8> {
    let digits = text.strip_prefix('#')?;
    if digits.len() != 6 || !digits.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(digits.get(at..at + 2)?, 16).ok();
    Some(Rgba8 {
        r: byte(0)?,
        g: byte(2)?,
        b: byte(4)?,
        a: 255,
    })
}
/// The byte a fraction is written as when the conversion truncates.
///
/// # Panics
///
/// Never.
fn truncate_alpha(fraction: f32) -> u8 {
    let scaled = fraction * 255.0;
    if !scaled.is_finite() {
        return 0;
    }
    scaled.clamp(0.0, 255.0) as u8
}

/// Every `<number><unit>` value a cell spells, in order.
///
/// A unit that continues a word is part of a name rather than a value: `screen_width_dp` is an
/// identifier the table mentions, not a 32dp or a 720dp.
///
/// # Panics
///
/// Never.
fn numbers(cell: &str, unit: &str) -> Vec<u16> {
    let mut values = Vec::new();
    let bytes = cell.as_bytes();
    let mut from = 0;
    while let Some(found) = cell[from..].find(unit) {
        let at = from + found;
        let mut start = at;
        while start > 0 && bytes[start - 1].is_ascii_digit() {
            start -= 1;
        }
        let continues_a_word =
            start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
        if !continues_a_word && start < at {
            if let Ok(value) = cell[start..at].parse::<u16>() {
                values.push(value);
            }
        }
        from = at + unit.len();
    }
    values
}

/// The single `<number><unit>` value a cell is, or `None` when it is anything else.
///
/// # Panics
///
/// Never.
fn single_number(cell: &str, unit: &str) -> Option<u16> {
    let text = unquote(cell);
    let digits = text.strip_suffix(unit)?;
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Every whole number a cell spells, in order.
///
/// # Panics
///
/// Never.
fn integers(cell: &str) -> Vec<u16> {
    let mut values = Vec::new();
    let mut digits = String::new();
    // The trailing space flushes a run that reaches the end of the cell.
    for ch in cell.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        if !digits.is_empty() {
            if let Ok(value) = digits.parse::<u16>() {
                values.push(value);
            }
            digits.clear();
        }
    }
    values
}

/// The range a cell states for a configurable value, as `可配置 A~B`.
///
/// # Panics
///
/// Never.
fn configurable(cell: &str) -> Option<(u16, u16)> {
    let rest = cell.split("可配置").nth(1)?;
    let range = rest.split(['（', '）', '，', ',']).next()?;
    let (min, max) = range.trim().split_once('~')?;
    Some((min.trim().parse().ok()?, max.trim().parse().ok()?))
}

/// Every backticked name a cell spells, in order.
///
/// # Panics
///
/// Never.
fn backticked(cell: &str) -> Vec<String> {
    cell.split('`')
        .skip(1)
        .step_by(2)
        .filter(|name| !name.trim().is_empty())
        .map(|name| name.trim().to_owned())
        .collect()
}

/// A cell with its backticks and surrounding space removed.
///
/// # Panics
///
/// Never.
fn unquote(cell: &str) -> String {
    cell.trim().trim_matches('`').trim().to_owned()
}
