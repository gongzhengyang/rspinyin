//! The view's side of the window's shadow material: the geometry
//! `ui/candidate.slint` declares, and how its rings tile the two band tables of
//! `ui/theme.slint`. Each ramp's height lives only in its table -- the peak scalars
//! `CandidateMetrics` once declared are gone -- and `slint_palette.rs` holds the
//! tables to the falloff 3.1.2 describes.

use ime_types::ColorScheme;

use crate::theme::slint_palette::{SLINT_CANDIDATE, SLINT_THEME, evaluate_bands};

// ---------------------------------------------------------------------------
// The candidate window's shadow material (3.1.2)
// ---------------------------------------------------------------------------
//
// Three files hold one contract: 3.1.2 fixes the geometry, `ui/theme.slint` carries the falloff
// as per-band colours, and `ui/candidate.slint` tiles it. `slint_palette.rs` pins the ramps'
// shape and height; the tests below pin the view's side -- its geometry, and that the rings
// tile the tables it paints from.

/// The number `ui/candidate.slint` declares for the `CandidateMetrics` constant `name`.
///
/// The block is read as text, the way `src/layout/metrics.rs` reads it: a generated binding
/// needs a live component and a display device, and a test may require neither.
fn candidate_metric(name: &str) -> Option<f32> {
    SLINT_CANDIDATE.lines().find_map(|line| {
        let text = line.split("//").next().unwrap_or_default().trim();
        let rest = text.strip_prefix("out property <")?;
        let (_, rest) = rest.split_once('>')?;
        let (declared, value) = rest.trim().split_once(':')?;
        if declared.trim() != name {
            return None;
        }
        let value = value.trim().strip_suffix(';')?.trim();
        let digits = value.strip_suffix("px").unwrap_or(value);
        digits.trim().parse::<f32>().ok()
    })
}

/// Whether any line of `source` outside a comment mentions `needle`.
///
/// The same rule the audit scripts apply: a file that explains why it does *not* use a
/// construct must not read as a file that uses it.
fn code_mentions(source: &str, needle: &str) -> bool {
    source
        .lines()
        .any(|line| line.split("//").next().unwrap_or_default().contains(needle))
}

/// The body of the `export component CandidateShadow` declaration.
///
/// Sliced by name, because the assertions about it are about one component: a binding elsewhere
/// in the file belongs to another component, and the same binding inside this block is the
/// defect those assertions exist for.
fn shadow_component(source: &str) -> &str {
    let opening = "export component CandidateShadow";
    let start = source
        .find(opening)
        .expect("the shadow component is declared");
    let rest = source.get(start..).expect("the offset is a boundary");
    let end = rest.find("\nexport component ").unwrap_or(rest.len());
    rest.get(..end).expect("the slice is a boundary")
}

#[test]
fn test_view_shadow_material_matches_the_material_table() {
    // 3.1.2's two layers are `0 8dp 28dp` for L3 and `0 1dp 2dp` for L2, and 3.1.1 gives the
    // window a 32dp reserve around the panel to draw them in. These five numbers are the whole
    // geometry of the material, so pinning them is what keeps a later edit from widening the
    // inner layer back into the outline 3.1.2 rules out, or from moving the panel inside the
    // reserve the placement pass computes the surface from.
    assert_eq!(candidate_metric("shadow-margin"), Some(32.0));
    assert_eq!(candidate_metric("shadow-blur"), Some(28.0));
    assert_eq!(candidate_metric("shadow-offset-y"), Some(8.0));
    assert_eq!(candidate_metric("shadow-inner-spread"), Some(2.0));
    assert_eq!(candidate_metric("shadow-inner-offset-y"), Some(1.0));
}

#[test]
fn test_view_shadow_bands_tile_the_blur_without_a_gap() {
    // The view paints each band as a ring one step wide whose outer edge sits at the band's own
    // distance from the panel, so the rings tile the blur only while the step divides it
    // exactly: otherwise two bands overlap or a transparent seam opens between them, and either
    // one reads as a step in what is supposed to be a gradient.
    let blur = candidate_metric("shadow-blur").expect("3.1.2 states the blur");
    let count = candidate_metric("shadow-band-count").expect("the layer counts its bands");
    let step = blur / count;

    assert_eq!(
        count, 8.0,
        "eight bands of 3.5dp read as a gradient at 1.0x"
    );
    assert_eq!(step, 3.5, "the bands divide the 28dp blur evenly");
    assert_eq!(
        blur,
        step * count,
        "so the outermost band ends on the blur's edge"
    );
    assert!(
        step >= 1.0,
        "a band narrower than a pixel would band the 1.0x case"
    );
}

#[test]
fn test_view_inner_shadow_stays_within_its_two_dp() {
    // L2 keeps the acrylic edge from blurring into the desktop, and 3.1.2 makes it 2dp wide. A
    // wider ring, or one band carrying the whole spread, is the 4dp 35%-black halo an earlier
    // revision drew -- a line around the panel, which is the opposite of an edge darkening. Both
    // bounds are asserted, because either alone is satisfied by a layer that still reads as one.
    let spread = candidate_metric("shadow-inner-spread").expect("3.1.2 states the spread");
    let count = candidate_metric("shadow-inner-band-count").expect("the layer counts its bands");
    let step = spread / count;

    assert!(
        spread <= 2.0,
        "the layer is at most 2dp wide, found {spread}dp"
    );
    assert!(count >= 2.0, "one band of the whole spread is an outline");
    assert_eq!(step, 1.0, "so every band is a 1dp hairline");
    assert_eq!(
        spread,
        step * count,
        "and the bands tile the spread exactly"
    );
    // The layer's peak is the alpha of the band against the panel, read straight out of
    // the table the view paints from: a fully opaque ring is a drawn line, not an edge
    // darkening.
    let inner = evaluate_bands(SLINT_THEME, "shadow-inner-bands", ColorScheme::Dark);
    let peak = inner
        .last()
        .expect("the table holds the band against the panel")
        .a;
    let peak = f32::from(peak) / 255.0;
    assert!(
        peak < 1.0,
        "a fully opaque ring is a drawn line, found {peak}"
    );
}

#[test]
fn test_view_shadow_layers_paint_from_the_theme_band_tables() {
    // The falloff cannot be an element `opacity`: the software renderer this project ships
    // takes a *bound* opacity down a compositing path that draws nothing at all, which is what
    // once turned the whole window transparent. The ramp therefore reaches the rasterizer as
    // the per-band colours `ui/theme.slint` declares, and the view has to read those tables
    // instead of restating an alpha of its own. Slint's `drop-shadow-*` is the other way this
    // could be drawn and the one the renderer stubs out, so neither may come back.
    let shadow = shadow_component(SLINT_CANDIDATE);

    assert!(
        shadow.contains("Theme.shadow-outer-bands[band]"),
        "{shadow}"
    );
    assert!(
        shadow.contains("Theme.shadow-inner-bands[band]"),
        "{shadow}"
    );
    assert!(
        !code_mentions(shadow, "opacity"),
        "a bound opacity draws nothing on this renderer"
    );
    assert!(
        !code_mentions(SLINT_CANDIDATE, "drop-shadow"),
        "the software renderer's draw_box_shadow is a stub"
    );
}

#[test]
fn test_view_shadow_band_counts_match_the_theme_tables() {
    // The view tiles each layer with the count `CandidateMetrics` declares and paints the ring
    // at index `band` with the table entry at that index, so the count and the table's length
    // are one contract: a count past the table would index out of it, and one short of it would
    // leave the outermost bands -- the faint ones the falloff is made of -- unpainted.
    for scheme in [ColorScheme::Dark, ColorScheme::Light] {
        let outer = evaluate_bands(SLINT_THEME, "shadow-outer-bands", scheme);
        let inner = evaluate_bands(SLINT_THEME, "shadow-inner-bands", scheme);

        assert_eq!(
            candidate_metric("shadow-band-count"),
            Some(outer.len() as f32),
            "{scheme:?}: the outer layer tiles the table it paints from"
        );
        assert_eq!(
            candidate_metric("shadow-inner-band-count"),
            Some(inner.len() as f32),
            "{scheme:?}: the inner layer tiles the table it paints from"
        );
    }
}
