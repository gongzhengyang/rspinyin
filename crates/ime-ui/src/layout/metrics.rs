//! The window's geometry constants and the parser that reads them.
//!
//! The constants live in `ui/candidate.slint`, in the `CandidateMetrics` global, because
//! that is where they are used to draw. This module reads them back out of the source so
//! that the layout arithmetic and the drawing cannot disagree: there is no second copy to
//! keep in step.
//!
//! Reading the source rather than the generated Slint bindings is deliberate. A generated
//! binding is only reachable through a live component instance, and creating one needs a
//! window backend, while the layout has to stay computable -- and testable -- with no
//! display server present.

use std::sync::OnceLock;

use ime_types::ImeError;

/// The Slint source the geometry constants are read from.
///
/// Embedding the source rather than shipping it beside the binary keeps the parse
/// infallible with respect to IO: the only way it can fail is the file and this module
/// disagreeing, which is a code change, not a runtime condition.
const CANDIDATE_SLINT: &str = include_str!("../../ui/candidate.slint");

/// Opening line of the block that holds the constants.
const METRICS_GLOBAL: &str = "export global CandidateMetrics";

/// Configuration key the metrics failures are reported under.
///
/// The frozen error list has no layout-domain variant, so a metrics block that cannot be
/// read is reported as an invalid built-in key whose reason carries the precise
/// `ui/layout/*` code.
const METRICS_KEY: &str = "ui.layout";

/// Geometry constants of `ui/candidate.slint`, in logical pixels.
///
/// The field names are the kebab-case names of the Slint properties with the dashes turned
/// into underscores, so a reader can move between the two without a lookup table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    /// 3.1.1: transparent margin around the container that holds the shadow.
    pub shadow_margin: f32,
    /// 3.1.1: container outline width, drawn inside the border.
    pub stroke_width: f32,
    /// 3.1.1: container corner radius; the theme overrides it with `corner_radius_dp`.
    pub container_radius: f32,
    /// 3.1.1: container padding, which is also the candidate area's padding.
    pub container_padding: f32,
    /// 3.1.1: narrowest container.
    pub min_width: f32,
    /// 3.1.1: widest container, before the screen is taken into account.
    pub max_width: f32,
    /// 3.1.1: header height with candidates present.
    pub header_height: f32,
    /// 3.1.1: header height when there is a preedit but no candidate.
    pub header_height_compact: f32,
    /// 3.1.1: header horizontal padding.
    pub header_padding_h: f32,
    /// 3.1.1: header status icon size.
    pub header_icon_size: f32,
    /// 3.1.1: gap between two status icons.
    pub header_icon_gap: f32,
    /// 3.1.1: gap between an icon and the text next to it.
    pub header_text_gap: f32,
    /// 3.1.1: header rule height.
    pub separator_height: f32,
    /// 3.1.1: narrowest candidate cell.
    pub cell_min_width: f32,
    /// 3.1.1: candidate cell height, constant regardless of the text.
    pub cell_height: f32,
    /// 3.1.1: candidate cell horizontal padding.
    pub cell_padding_h: f32,
    /// 3.1.1: candidate cell vertical padding.
    pub cell_padding_v: f32,
    /// 3.1.1: candidate cell corner radius, concentric with the container.
    pub cell_radius: f32,
    /// 3.1.1: gap between the number label and the candidate text.
    pub number_gap: f32,
    /// 3.1.1: gap between the candidate text and its annotation.
    pub annotation_gap: f32,
    /// 3.1.1: gap between two cells and between two rows.
    pub grid_gap: f32,
    /// 3.1.1: cursor indicator arrow width.
    pub cursor_arrow_width: f32,
    /// 3.1.1: cursor indicator arrow height.
    pub cursor_arrow_height: f32,
    /// 3.1.2: outer shadow blur radius.
    pub shadow_blur: f32,
    /// 3.1.2: outer shadow downward offset.
    pub shadow_offset_y: f32,
    /// 3.1.2: inner shadow spread.
    pub shadow_inner_spread: f32,
    /// 3.1.2: inner shadow downward offset.
    pub shadow_inner_offset_y: f32,
    /// 3.1.2: how many bands the outer shadow is drawn with.
    pub shadow_band_count: u8,
    /// 3.1.2: opacity of one outer shadow band.
    pub shadow_band_opacity: f32,
    /// 3.1.2: opacity of the inner shadow layer.
    pub shadow_inner_opacity: f32,
    /// 3.1.3: text wider than this is elided instead of widening the cell.
    pub max_text_width: f32,
    /// 3.1.3: what a cell spends on everything except its text.
    pub cell_chrome_width: f32,
    /// 3.1.3: how many pages the window is willing to show.
    pub max_pages: u8,
    /// 3.1.1: smallest configurable candidates per row.
    pub min_per_row: u8,
    /// 3.1.1: default candidates per row.
    pub max_per_row: u8,
    /// 3.1.1: largest configurable candidates per row.
    pub max_per_row_limit: u8,
    /// 3.1.1: header preedit font size.
    pub font_size_header: f32,
    /// 3.1.1: candidate text font size.
    pub font_size_cell: f32,
    /// 3.1.1: number label, annotation and status font size.
    pub font_size_small: f32,
}

/// Why the metrics block could not be read, before it is rendered as a cross-boundary code.
///
/// This stays private: the constants are compiled into the binary, so every variant means
/// the Slint source and this module have drifted apart -- a failure that needs a code
/// change rather than a retry, and one that callers can only report, never recover from.
/// Keeping it internal is what lets the public surface stay on the frozen error model.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Reason {
    /// The Slint source no longer declares the metrics block.
    GlobalMissing,
    /// The metrics block no longer declares a metric the layout computes with.
    Missing(String),
    /// A metric line carries a value that is not a usable number.
    Malformed { name: String, value: String },
}

impl Reason {
    /// Renders the stable `domain/action/reason` code this failure reports.
    fn code(&self) -> String {
        match self {
            Self::GlobalMissing => format!("ui/layout/metrics-missing: {METRICS_GLOBAL}"),
            Self::Missing(name) => format!("ui/layout/metric-missing: {name}"),
            Self::Malformed { name, value } => {
                format!("ui/layout/metric-malformed: {name}={value}")
            }
        }
    }
}

impl From<Reason> for ImeError {
    // The frozen list has no layout-domain variant, and a metrics block that cannot be read
    // is a built-in constant that no longer matches its consumer -- the same shape as a
    // configuration key that fails validation, which is what it maps onto. The precise
    // `ui/layout/*` code travels in the reason so that it stays greppable in diagnostics.
    fn from(reason: Reason) -> Self {
        ImeError::ConfigInvalid {
            key: String::from(METRICS_KEY),
            reason: reason.code(),
        }
    }
}

/// The window's geometry constants, parsed once from `ui/candidate.slint`.
///
/// # Errors
///
/// Returns [`ImeError::ConfigInvalid`] if the embedded Slint source no longer declares the
/// metrics block, has dropped a metric, or carries a value that is not a number; the reason
/// names the offending metric under a `ui/layout/*` code. The result is cached, so the
/// parse happens at most once per process.
pub fn metrics() -> Result<&'static Metrics, ImeError> {
    static CACHE: OnceLock<Result<Metrics, Reason>> = OnceLock::new();
    match CACHE.get_or_init(|| read_metrics(CANDIDATE_SLINT)) {
        Ok(metrics) => Ok(metrics),
        Err(reason) => Err(ImeError::from(reason.clone())),
    }
}

/// Reads the metrics block out of a `candidate.slint` source.
///
/// The block is the `export global CandidateMetrics { ... }` declaration; every line in it
/// that spells `out property <length|int|float> name: <number>[px];` becomes one metric.
/// Lines declaring any other type are skipped, which leaves room for non-scalar helpers in
/// the same global.
///
/// # Parameters
///
/// * `source` -- the Slint source to read.
///
/// # Returns
///
/// The parsed constants, or the reason the block could not be read.
///
/// # Errors
///
/// Returns [`ImeError::ConfigInvalid`] when the block is absent
/// (`ui/layout/metrics-missing`), when a metric line's value does not parse
/// (`ui/layout/metric-malformed`), or when a metric this module computes with is not
/// declared (`ui/layout/metric-missing`).
///
/// # Panics
///
/// Never panics: the source is scanned line by line and every failure is returned.
pub fn parse_metrics(source: &str) -> Result<Metrics, ImeError> {
    read_metrics(source).map_err(ImeError::from)
}

/// Reads the metrics block, reporting failures in this module's own vocabulary.
fn read_metrics(source: &str) -> Result<Metrics, Reason> {
    let block = metrics_block(source).ok_or(Reason::GlobalMissing)?;
    let mut found = Vec::with_capacity(block.len());
    for line in block {
        if let Some(metric) = parse_metric_line(line)? {
            found.push(metric);
        }
    }
    Metrics::take_all(&mut found)
}

/// Collects the lines between the metrics global's opening line and its closing brace.
fn metrics_block(source: &str) -> Option<Vec<&str>> {
    let mut lines = Vec::new();
    let mut inside = false;
    for line in source.lines() {
        if !inside {
            inside = line.trim_start().starts_with(METRICS_GLOBAL);
            continue;
        }
        if line.trim() == "}" {
            return Some(lines);
        }
        lines.push(line);
    }
    None
}

/// Reads one scalar metric declaration.
///
/// # Errors
///
/// [`Reason::Malformed`] when the line declares a scalar metric whose value is missing,
/// negative or not a number. A line that is not a scalar metric declaration is skipped
/// with `Ok(None)`.
fn parse_metric_line(line: &str) -> Result<Option<(&str, f32)>, Reason> {
    let text = line.split("//").next().unwrap_or(line).trim();
    let Some(rest) = text.strip_prefix("out property <") else {
        return Ok(None);
    };
    let Some((kind, rest)) = rest.split_once('>') else {
        return Ok(None);
    };
    if !matches!(kind, "length" | "int" | "float") {
        return Ok(None);
    }
    let Some((name, value)) = rest.trim().split_once(':') else {
        return Ok(None);
    };
    let name = name.trim();
    let Some(value) = value.trim().strip_suffix(';') else {
        return Ok(None);
    };
    let number = match kind {
        "length" => value.strip_suffix("px"),
        _ => Some(value),
    };
    let number = number
        .and_then(|digits| digits.trim().parse::<f32>().ok())
        .filter(|parsed| parsed.is_finite() && *parsed >= 0.0);
    match number {
        Some(parsed) => Ok(Some((name, parsed))),
        None => Err(Reason::Malformed {
            name: String::from(name),
            value: String::from(value.trim()),
        }),
    }
}

/// Removes one metric from the collected set.
///
/// # Errors
///
/// [`Reason::Missing`] when the block does not declare `name`.
fn take(found: &mut Vec<(&str, f32)>, name: &str) -> Result<f32, Reason> {
    let position = found.iter().position(|(key, _)| *key == name);
    match position {
        Some(position) => Ok(found.swap_remove(position).1),
        None => Err(Reason::Missing(String::from(name))),
    }
}

/// Removes one metric that counts something, saturating into `u8`.
fn take_count(found: &mut Vec<(&str, f32)>, name: &str) -> Result<u8, Reason> {
    let value = take(found, name)?;
    // Saturating rather than wrapping: a count past 255 is a broken declaration, and
    // clamping keeps it from becoming a small, plausible-looking number.
    Ok(value.clamp(0.0, f32::from(u8::MAX)) as u8)
}

impl Metrics {
    /// Pulls every metric out of the collected set, failing on the first that is absent.
    fn take_all(found: &mut Vec<(&str, f32)>) -> Result<Self, Reason> {
        Ok(Self {
            shadow_margin: take(found, "shadow-margin")?,
            stroke_width: take(found, "stroke-width")?,
            container_radius: take(found, "container-radius")?,
            container_padding: take(found, "container-padding")?,
            min_width: take(found, "min-width")?,
            max_width: take(found, "max-width")?,
            header_height: take(found, "header-height")?,
            header_height_compact: take(found, "header-height-compact")?,
            header_padding_h: take(found, "header-padding-h")?,
            header_icon_size: take(found, "header-icon-size")?,
            header_icon_gap: take(found, "header-icon-gap")?,
            header_text_gap: take(found, "header-text-gap")?,
            separator_height: take(found, "separator-height")?,
            cell_min_width: take(found, "cell-min-width")?,
            cell_height: take(found, "cell-height")?,
            cell_padding_h: take(found, "cell-padding-h")?,
            cell_padding_v: take(found, "cell-padding-v")?,
            cell_radius: take(found, "cell-radius")?,
            number_gap: take(found, "number-gap")?,
            annotation_gap: take(found, "annotation-gap")?,
            grid_gap: take(found, "grid-gap")?,
            cursor_arrow_width: take(found, "cursor-arrow-width")?,
            cursor_arrow_height: take(found, "cursor-arrow-height")?,
            shadow_blur: take(found, "shadow-blur")?,
            shadow_offset_y: take(found, "shadow-offset-y")?,
            shadow_inner_spread: take(found, "shadow-inner-spread")?,
            shadow_inner_offset_y: take(found, "shadow-inner-offset-y")?,
            shadow_band_count: take_count(found, "shadow-band-count")?,
            shadow_band_opacity: take(found, "shadow-band-opacity")?,
            shadow_inner_opacity: take(found, "shadow-inner-opacity")?,
            max_text_width: take(found, "max-text-width")?,
            cell_chrome_width: take(found, "cell-chrome-width")?,
            max_pages: take_count(found, "max-pages")?,
            min_per_row: take_count(found, "min-per-row")?,
            max_per_row: take_count(found, "max-per-row")?,
            max_per_row_limit: take_count(found, "max-per-row-limit")?,
            font_size_header: take(found, "font-size-header")?,
            font_size_cell: take(found, "font-size-cell")?,
            font_size_small: take(found, "font-size-small")?,
        })
    }
}

#[cfg(test)]
// The tests stay in this file rather than moving to `layout/metrics/` once they pass the
// length `AGENTS.md` 3.6 sets, and the reason is not brevity: `scripts/check-ui-spec.sh`
// excludes exactly this file from its reference-closure scan, so a metric a test names here
// does not stop looking dead. The same names in a sibling file would, and the gate's "no
// constant is dead" assertion would go quiet for every one of them.
mod tests {
    use super::*;

    /// The grid's Slint source, which declares the second component the palette reuses.
    const GRID_SLINT: &str = include_str!("../../ui/candidate_grid.slint");

    /// The rendered error text, which is what diagnostics and tests match on.
    fn failure(source: &str) -> String {
        parse_metrics(source)
            .expect_err("the source is expected to fail")
            .to_string()
    }

    /// A Slint source with every `//` comment removed.
    ///
    /// The rules the structural tests below assert are written out in the file's own prose,
    /// so a search that kept the comments would report the explanation of a rule as a
    /// violation of it.
    fn code_without_comments(source: &str) -> String {
        source
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Whether a line of Slint code binds the `opacity` property.
    ///
    /// The panel's fade and the grid's four dims are bindings this way; `window-opacity`
    /// -- the appear motion's property, which the panel's binding *reads* -- must not
    /// match, and neither may a token such as `shadow-band-opacity`, whose name only ends
    /// in the same word. A *binding* is the property name at the start of the line and
    /// nothing before it.
    fn binds_opacity(line: &str) -> bool {
        // Matching the name anywhere in the line would call every band token a binding.
        line.trim_start().starts_with("opacity:")
    }

    #[test]
    fn test_metrics_matches_the_31_tables() {
        let metrics = *metrics().expect("ui/candidate.slint declares a readable metrics block");
        assert_eq!(metrics.shadow_margin, 32.0);
        assert_eq!(metrics.container_radius, 12.0);
        assert_eq!(metrics.container_padding, 8.0);
        assert_eq!(metrics.min_width, 220.0);
        assert_eq!(metrics.max_width, 720.0);
        assert_eq!(metrics.header_height, 34.0);
        assert_eq!(metrics.header_height_compact, 28.0);
        assert_eq!(metrics.header_padding_h, 10.0);
        assert_eq!(metrics.separator_height, 1.0);
        assert_eq!(metrics.cell_min_width, 64.0);
        assert_eq!(metrics.cell_height, 36.0);
        assert_eq!(metrics.cell_padding_h, 10.0);
        assert_eq!(metrics.cell_padding_v, 6.0);
        assert_eq!(metrics.cell_radius, 4.0);
        assert_eq!(metrics.grid_gap, 6.0);
        assert_eq!(metrics.max_text_width, 120.0);
        assert_eq!(metrics.max_pages, 5);
        assert_eq!(metrics.min_per_row, 3);
        assert_eq!(metrics.max_per_row, 5);
        assert_eq!(metrics.max_per_row_limit, 9);
        assert_eq!(metrics.font_size_header, 14.0);
        assert_eq!(metrics.font_size_cell, 15.0);
        assert_eq!(metrics.font_size_small, 11.0);
    }

    #[test]
    fn test_metrics_is_cached_across_calls() {
        let first = metrics().expect("the metrics parse");
        let second = metrics().expect("the metrics parse");
        assert!(core::ptr::eq(first, second));
    }

    #[test]
    fn test_parse_metrics_source_without_block_reports_missing_global() {
        let text = failure("// an empty file\n");
        assert_eq!(
            text,
            "config/invalid: ui.layout (ui/layout/metrics-missing: export global CandidateMetrics)"
        );
    }

    #[test]
    fn test_parse_metrics_malformed_value_names_the_metric() {
        let text = failure(
            "export global CandidateMetrics {\n    out property <length> shadow-margin: wide;\n}\n",
        );
        assert_eq!(
            text,
            "config/invalid: ui.layout (ui/layout/metric-malformed: shadow-margin=wide)"
        );
    }

    #[test]
    fn test_parse_metrics_negative_value_is_rejected() {
        let text = failure(
            "export global CandidateMetrics {\n    out property <length> grid-gap: -6px;\n}\n",
        );
        assert_eq!(
            text,
            "config/invalid: ui.layout (ui/layout/metric-malformed: grid-gap=-6px)"
        );
    }

    #[test]
    fn test_parse_metrics_missing_metric_names_it() {
        let text = failure(
            "export global CandidateMetrics {\n    out property <length> min-width: 220px;\n}\n",
        );
        assert_eq!(
            text,
            "config/invalid: ui.layout (ui/layout/metric-missing: shadow-margin)"
        );
    }

    #[test]
    fn test_parse_metrics_ignores_non_scalar_declarations() {
        let mut source = String::from("export global CandidateMetrics {\n");
        source.push_str("    out property <[length]> band-spreads: [28px, 20px];\n");
        source.push_str("    out property <length> shadow-margin: 32px;\n}\n");
        // The scalar is read and the array is skipped, so the failure names the metric
        // that is genuinely absent rather than the array.
        assert_eq!(
            failure(&source),
            "config/invalid: ui.layout (ui/layout/metric-missing: stroke-width)"
        );
    }

    #[test]
    fn test_parse_metrics_reads_a_minimal_block() {
        let mut source = String::from("export global CandidateMetrics {\n");
        for name in [
            "shadow-margin",
            "stroke-width",
            "container-radius",
            "container-padding",
            "min-width",
            "max-width",
            "header-height",
            "header-height-compact",
            "header-padding-h",
            "header-icon-size",
            "header-icon-gap",
            "header-text-gap",
            "separator-height",
            "cell-min-width",
            "cell-height",
            "cell-padding-h",
            "cell-padding-v",
            "cell-radius",
            "number-gap",
            "annotation-gap",
            "grid-gap",
            "cursor-arrow-width",
            "cursor-arrow-height",
            "shadow-blur",
            "shadow-offset-y",
            "shadow-inner-spread",
            "shadow-inner-offset-y",
            "max-text-width",
            "cell-chrome-width",
            "font-size-header",
            "font-size-cell",
            "font-size-small",
        ] {
            source.push_str(&format!("    out property <length> {name}: 8px;\n"));
        }
        for name in [
            "shadow-band-count",
            "max-pages",
            "min-per-row",
            "max-per-row",
            "max-per-row-limit",
        ] {
            source.push_str(&format!("    out property <int> {name}: 5;\n"));
        }
        for name in ["shadow-band-opacity", "shadow-inner-opacity"] {
            source.push_str(&format!("    out property <float> {name}: 0.5;\n"));
        }
        source.push_str("}\n");
        let metrics = parse_metrics(&source).expect("every metric is declared");
        assert_eq!(metrics.shadow_margin, 8.0);
        assert_eq!(metrics.max_pages, 5);
        assert_eq!(metrics.shadow_band_opacity, 0.5);
    }

    #[test]
    fn test_metrics_cell_radius_stays_concentric_with_the_container() {
        let metrics = *metrics().expect("ui/candidate.slint declares a readable metrics block");
        // 3.1.1 states the derivation and not only the number: the grid sits
        // `container-padding` inside the container, so the corner that reads as the same curve
        // is the container's minus that padding. `candidate.slint` derives the cell radius
        // that way, and this pins the three numbers it derives from -- a change to either of
        // the first two that leaves the table's 4dp behind is a concentricity break rather
        // than a rounding.
        assert_eq!(
            metrics.cell_radius,
            metrics.container_radius - metrics.container_padding,
            "the cell's corner must be concentric with the container's"
        );
        assert_eq!(
            metrics.cell_radius, 4.0,
            "and 3.1.1 fixes it at 4dp for the 12dp container"
        );
    }

    #[test]
    fn test_candidate_slint_header_starts_on_the_candidate_grid_axis() {
        // The preedit and the first candidate cell are drawn on one left axis: 3.1.1's header
        // sketch puts `ni'hao'a` and the first cell's edge in one column, and the strip spans
        // the panel's full width, so its left inset has to be the padding the grid insets its
        // cells by. Taking the strip's own `header-padding-h` there instead -- which the file
        // did before -- puts the first glyph 2dp right of the first cell's edge. The layout is
        // Slint's and a test may not need a display server, so the source is where the axis is
        // asserted; the pixels it produces are the adapter's scenes to check.
        let code = code_without_comments(CANDIDATE_SLINT);
        let strip = "padding-left: CandidateMetrics.container-padding;";
        let cells = "area-padding: CandidateMetrics.container-padding;";
        let right = "padding-right: CandidateMetrics.header-padding-h;";
        assert!(
            code.contains(strip),
            "the header strip must start on the candidate grid's axis"
        );
        assert!(
            code.contains(cells),
            "and the grid must inset its cells by that same axis"
        );
        assert!(
            code.contains(right),
            "the strip keeps 3.1.1's 10dp on the right, away from the container edge"
        );
    }

    #[test]
    fn test_candidate_slint_exports_the_components_a_second_document_reuses() {
        // 3.5's command palette reuses the header and the grid, so both have to be components
        // another document can import: exported, declared once, and instantiated by the window
        // rather than inlined into it. That the grid is importable is a fact of this crate's
        // own build -- the window imports it, below -- while the header's is the half a second
        // document would have to prove.
        let candidate = code_without_comments(CANDIDATE_SLINT);
        let grid = code_without_comments(GRID_SLINT);
        let header = "export component Header inherits Rectangle";
        let grid_component = "export component CandidateGrid inherits VerticalLayout";
        let import = "import { CandidateData, CandidateGrid } from \"candidate_grid.slint\";";
        assert!(
            candidate.contains(header),
            "the header is an export component of the window's own file"
        );
        assert!(
            grid.contains(grid_component),
            "the grid is an export component of its own file"
        );
        assert!(
            candidate.contains(import),
            "the window imports the grid instead of declaring one"
        );
        for component in ["Header {", "CandidateGrid {"] {
            assert!(
                candidate.contains(component),
                "the window instantiates {component}"
            );
        }
    }

    #[test]
    fn test_view_binds_only_the_whitelisted_opacity_and_takes_no_focus() {
        // Two facts of the view this project ships, and the source is the only place
        // either can be asserted.
        //
        // The renderer honours a bound opacity: i-slint-core 1.13.1 multiplies it into the
        // state alpha, culls the subtree at alpha 0.01, and folds the rest into every
        // rectangle and glyph -- which the pixel probes in `renderer/tests.rs` assert
        // against real output. An `opacity` binding is therefore a drawing decision, not a
        // no-op, and every one of them is named below: a new binding, or a changed value,
        // fails this test until it joins the whitelist with the spec row that fixes it.
        // The grid's four are 3.4's dim, folded into each drawing child; the panel's one
        // is 3.3.2's fade-in. And the candidate window must never take keyboard focus
        // (features.md 0.4 rule 5): no `TextInput`, no `forward-focus` and no `focus()`
        // call may appear in either file.
        for (source, name, allowed) in [
            (
                CANDIDATE_SLINT,
                "candidate.slint",
                &["opacity: min(root.window-opacity, 1.0);"] as &[&str],
            ),
            (
                GRID_SLINT,
                "candidate_grid.slint",
                &[
                    "opacity: 0.50 * dim;",
                    "opacity: 0.55 * dim;",
                    "opacity: dim;",
                    "opacity: dim;",
                ] as &[&str],
            ),
        ] {
            let code = code_without_comments(source);
            let mut bound: Vec<&str> = code
                .lines()
                .filter(|line| binds_opacity(line))
                .map(|line| line.trim())
                .collect();
            bound.sort_unstable();
            let mut expected = allowed.to_vec();
            expected.sort_unstable();
            assert_eq!(
                bound, expected,
                "{name} must bind exactly the whitelisted opacity lines and no others"
            );
        }
        for source in [CANDIDATE_SLINT, GRID_SLINT] {
            let code = code_without_comments(source);
            for forbidden in ["TextInput", "forward-focus", "focus("] {
                assert!(
                    !code.contains(forbidden),
                    "{forbidden} would take keyboard focus"
                );
            }
        }
    }
}
