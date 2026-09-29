//! The `runtime://ui_metrics` channel: what the window is, as numbers rather than as pixels.
//!
//! Responsibility: assemble the three sources that together describe the candidate window's
//! geometry, materials and theme, and cross-check them against each other and against the
//! specification tables that fix them.
//!
//! # Why this channel stands in for "computed style"
//!
//! `features-test.md` 0.4 lists `runtime://style_computed` among the primitives this platform
//! has to substitute, and the substitution is forced by the architecture rather than chosen: the
//! candidate window is drawn by the plugin itself, pixel by pixel, into a shared buffer. There
//! is no toolkit, no widget tree, no stylesheet, and therefore nothing that could compute a
//! style. A harness that reported "the computed style" would be describing a mechanism that does
//! not exist.
//!
//! What does exist is three sets of numbers, and each one can be wrong in a way the others
//! cannot see:
//!
//! | Source | What it holds | What it detects |
//! |---|---|---|
//! | `features.md` 3.1 and 3.2 | the authoritative sizes, grid exceptions and colour tokens | the specification itself being edited |
//! | the `.slint` metrics block | the constants the window is built from | code drifting from the specification |
//! | the running window | the backend's geometry, the physical size of its surface and its effective alpha, the frame's layout hint and the theme it was sent | a degradation that did not take effect, a geometry computed wrong, a surface sized for another scale |
//!
//! No one of them is a substitute for another, and none of them is a substitute for a pixel: what
//! the window actually *looks* like is the screenshot channel's business. This module answers
//! what the window *is* -- its size in both units, its scale, the alpha its base is painted with,
//! and the numbers it was built from.
//!
//! # A source that cannot be read
//!
//! `crates/ime-ui/ui/candidate.slint` declares the metrics block, so the third source is read
//! wherever the tree is complete. The channel still treats a source it cannot read as
//! [`MetricSource::Unavailable`] and reports that as a [`MetricMismatch`] of its own -- never as
//! an empty table, which would make every comparison against it pass. That is the whole reason
//! the third source is modelled as a value rather than as a `&SlintConstants`: a caller cannot
//! hand this module an empty block by accident, and a tree that lost the file is a finding rather
//! than a quietly weaker run.
//!
//! # What this module never does
//!
//! It opens no socket, starts no process and writes nothing. It does not link the renderer:
//! `xtask` depends on the frozen contract and not on `ime-ui`, so the two backend rules it needs
//! are stated here and have to be kept in step with the code that applies them -- the effective
//! alpha as [`effective_base_alpha`], against `ime_ui::platform::x11`, and the
//! logical-to-physical conversion as [`physical_dimension`], against `ime_ui::platform`. The
//! Wayland tier vocabulary is repeated in [`super::env`] for the same reason. It also never reads
//! a frame: the layout and the theme arrive as values the caller took from the frame mirror,
//! which keeps this module testable with no display server.
//!
//! # Modules
//!
//! [`spec`] reads the specification tables, [`slint`] the `.slint` metrics block, [`value`] the
//! one value type all three sources reduce to, and [`error`] the refusals.

// The channel is exercised by the tests below and by nothing else yet: the case runner that
// would assemble a reading and the subcommand tree that would print it both live in files this
// module does not own. Until that wiring lands, every item here is reported as dead code in a
// non-test build, and the attribute goes away with those lines.
//
// `unused_imports` is covered by the same reasoning and for the same reason: the `pub use` lines
// below are this module's surface, and a `pub use` in a *binary* crate is "unused" whenever
// nothing in the crate names it.
#![allow(dead_code, unused_imports)]

mod error;
mod slint;
mod spec;
mod value;

#[cfg(test)]
mod tests;

pub use self::error::MetricError;
pub use self::slint::SlintConstants;
pub use self::spec::{
    COLOUR_SECTION, ColourCell, ColourRow, GEOMETRY_SECTION, GRID_SECTION, GeometryRow, SpecTable,
};
pub use self::value::{MetricValue, Unit, alpha_byte};

use std::fs;
use std::path::Path;

use ime_types::{ColorScheme, LayoutHint, ThemeSpec};

use super::env::X11Facts;

/// Path of the document the specification tables live in, relative to the repository root.
pub const SPEC_DOCUMENT: &str = "docs/dev/features.md";

/// Path of the `.slint` source whose metrics block the window is built from, relative to the
/// repository root.
pub const SLINT_DOCUMENT: &str = "crates/ime-ui/ui/candidate.slint";

/// The alpha an opaque surface base carries.
pub const OPAQUE_ALPHA: u8 = 255;

/// The colour token whose alpha the acrylic surface base uses.
const BASE_ALPHA_TOKEN: &str = "surface.base";

/// The item name the specification's own base alpha is reported under.
const BASE_ALPHA_ITEM: &str = "surface.base @ alpha";

/// The item name the alpha the backend paints with is reported under.
const EFFECTIVE_ALPHA_ITEM: &str = "the effective base alpha";

/// A source that may or may not have been readable.
///
/// The type exists so that "the source is missing" cannot be expressed as "the source is
/// empty". An empty table makes every comparison against it agree, which is the one failure mode
/// a cross-check must not have; a caller that has to name the missing source has to decide what
/// the case should do about it.
#[derive(Clone, Debug, PartialEq)]
pub enum MetricSource<T> {
    /// The source was read.
    Available(T),
    /// The source could not be read, so nothing about it was checked.
    Unavailable {
        /// Why, in a form a developer can act on.
        reason: String,
    },
}

impl<T> MetricSource<T> {
    /// Whether the source was read.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available(_))
    }

    /// Why the source could not be read, or `None` when it was.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Available(_) => None,
            Self::Unavailable { reason } => Some(reason),
        }
    }

    /// The source, when it was read.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn available(self) -> Option<T> {
        match self {
            Self::Available(value) => Some(value),
            Self::Unavailable { .. } => None,
        }
    }
}

/// Which of the three sources a value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MetricSourceId {
    /// The specification tables of `features.md` 3.1 and 3.2.
    Spec,
    /// The `.slint` metrics block.
    Slint,
    /// The running window: the backend's reading, the frame's layout and its theme.
    Runtime,
}

impl MetricSourceId {
    /// The label a report prints.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::Spec => "features.md 3.1/3.2",
            Self::Slint => SLINT_DOCUMENT,
            Self::Runtime => "the running window",
        }
    }
}

/// One disagreement the cross-check found, or one source it could not read.
///
/// Every variant but [`Self::Geometry`] reports two sources disagreeing about one item. That one
/// reports the running window disagreeing with itself, which is why its two numbers are plain
/// physical pixel counts rather than [`MetricValue`]s: no document states a surface's size in
/// pixels, so there is nothing for the type that makes the three sources comparable to carry.
#[derive(Clone, Debug, PartialEq)]
pub enum MetricMismatch {
    /// Two sources state different values for one item.
    Value {
        /// The item, named the way the source that carries the *code* side names it.
        name: String,
        /// What the specification states.
        spec: MetricValue,
        /// What the code states.
        code: MetricValue,
        /// The source the code side came from.
        source: MetricSourceId,
    },

    /// A value the specification fixes a range for falls outside it.
    OutOfRange {
        /// The item, named as the document names its row.
        name: String,
        /// The value the code carries.
        value: MetricValue,
        /// The lowest value the specification allows.
        min: u16,
        /// The highest value the specification allows.
        max: u16,
        /// The source the value came from.
        source: MetricSourceId,
    },

    /// A source could not be read, so nothing about it was checked.
    Unavailable {
        /// The source that could not be read.
        source: MetricSourceId,
        /// Why.
        reason: String,
    },

    /// The specification names an item a source does not state.
    Missing {
        /// The item.
        name: String,
        /// The source that should have stated it.
        source: MetricSourceId,
    },

    /// The window's physical size does not follow from its logical size at its scale.
    ///
    /// The one verdict no document is needed for. The window states the same quantity twice, in
    /// two units -- `SurfaceBackend::geometry` reports a logical size beside a device pixel ratio,
    /// and the buffer the backend hands out is measured in physical pixels -- and the first two
    /// fix the third. A backend that allocated its buffer for a scale other than the one it
    /// reports, or a scale that was corrected somewhere between the configuration and the surface,
    /// is visible here and nowhere else.
    Geometry {
        /// Which dimension: `"width"` or `"height"`.
        axis: &'static str,
        /// That dimension's logical size, as the backend reports it.
        dp: u32,
        /// The device pixel ratio the window is rasterised at.
        scale: f32,
        /// The physical size `dp` and `scale` fix.
        expected: u32,
        /// The physical size the surface reports.
        found: u32,
    },
}

impl MetricMismatch {
    /// The mismatch as a report prints it, in one line.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn describe(&self) -> String {
        match self {
            Self::Value {
                name,
                spec,
                code,
                source,
            } => format!(
                "{name}: the specification says {}, {} says {}",
                spec.describe(),
                source.label(),
                code.describe()
            ),
            Self::OutOfRange {
                name,
                value,
                min,
                max,
                source,
            } => format!(
                "{name}: {} says {min}..{max}, {} says {}",
                MetricSourceId::Spec.label(),
                source.label(),
                value.describe()
            ),
            Self::Unavailable { source, reason } => {
                format!("{} could not be read: {reason}", source.label())
            }
            Self::Missing { name, source } => {
                format!("{name} is missing from {}", source.label())
            }
            Self::Geometry {
                axis,
                dp,
                scale,
                expected,
                found,
            } => format!(
                "the window's {axis} is {dp}dp at scale {scale}, which is {expected}px, \
                 but the surface reports {found}px"
            ),
        }
    }
}

/// The two sources that come from files, read together.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricSources {
    /// The specification tables of `features.md` 3.1 and 3.2.
    pub spec: SpecTable,
    /// The `.slint` metrics block, or the reason there is none.
    pub slint: MetricSource<SlintConstants>,
}

impl MetricSources {
    /// Reads both sources out of the repository rooted at `root`.
    ///
    /// # Errors
    ///
    /// Returns [`MetricError::Io`] when a file exists and cannot be read, and the parse errors of
    /// [`SpecTable::parse`] and [`SlintConstants::parse`].
    ///
    /// # Panics
    ///
    /// Never.
    pub fn read(root: &Path) -> Result<Self, MetricError> {
        let path = root.join(SPEC_DOCUMENT);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(source) => return Err(MetricError::Io { path, source }),
        };
        Ok(Self {
            spec: SpecTable::parse(&text)?,
            slint: SlintConstants::load(&root.join(SLINT_DOCUMENT))?,
        })
    }
}

/// What the display backend reports about the window it is drawing into.
///
/// The fields are the backend's own readings rather than values this module derives, so that a
/// case can hand over what it measured and have the rules applied to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BackendReading {
    /// The backend's identifier, as `SurfaceBackend::backend_id` returns it.
    pub backend_id: &'static str,
    /// The window's size in logical pixels, as `SurfaceBackend::geometry` returns it.
    pub window_dp: (u32, u32),
    /// The surface's size in physical pixels: the size of the buffer the backend hands out.
    ///
    /// The trait does not report this, so a caller reads it from the `PixelBufferMut` the backend
    /// returns. It is carried rather than derived, because the point of checking it is that it may
    /// not be what the logical size and the scale fix.
    pub window_px: (u32, u32),
    /// The device pixel ratio, as `SurfaceBackend::geometry` returns it.
    pub scale: f32,
    /// Whether the window was created with a 32-bit ARGB visual.
    pub argb_visual: bool,
    /// Whether an active compositor owned the selection when the window was created.
    pub compositor_present: bool,
    /// The alpha the backend reports it paints the surface base with, after its own fallback.
    pub base_alpha: u8,
}

impl BackendReading {
    /// Reads the two capability flags out of an environment probe's X11 facts.
    ///
    /// # Parameters
    ///
    /// * `facts` -- what the X server answered, from [`super::env`].
    /// * `geometry` -- the triple `SurfaceBackend::geometry` returns: the logical size and the
    ///   device pixel ratio. The probe measures neither, so the caller brings them.
    /// * `window_px` -- the physical size of the surface, which the backend reports through the
    ///   buffer it hands out rather than through `geometry`.
    /// * `base_alpha` -- the alpha the backend reports it paints the base with.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_facts(
        facts: &X11Facts,
        geometry: (u32, u32, f32),
        window_px: (u32, u32),
        base_alpha: u8,
    ) -> Self {
        let (width_dp, height_dp, scale) = geometry;
        Self {
            backend_id: "x11",
            window_dp: (width_dp, height_dp),
            window_px,
            scale,
            argb_visual: facts.argb_visual,
            compositor_present: facts.compositor_present,
            base_alpha,
        }
    }
}

/// The running window, as the three sources describe it together.
///
/// `window_dp` is the backend's *logical* size, not its physical one: `SurfaceBackend::geometry`
/// returns device-independent pixels beside the scale factor, and every size in `features.md` 3.1
/// is stated in the same unit. Comparing physical pixels against a `dp` table would report a
/// 2x display as twice as wide as the specification allows. The physical size is carried beside
/// it as `window_px`, and the two are compared with each other rather than with a table: see
/// [`MetricMismatch::Geometry`].
#[derive(Clone, Debug, PartialEq)]
pub struct UiMetrics {
    /// The window's size in logical pixels.
    pub window_dp: (u32, u32),
    /// The surface's size in physical pixels.
    pub window_px: (u32, u32),
    /// The device pixel ratio the window is rasterised at.
    pub scale: f32,
    /// The alpha the surface base is actually painted with, after the backend's fallback.
    pub base_alpha: u8,
    /// Whether an active compositor is present.
    pub compositor_present: bool,
    /// Whether the window can carry an alpha channel.
    pub argb_visual: bool,
    /// The backend's identifier: `"x11"`, `"wlr-layer-shell"`, `"mock"`.
    pub backend_id: &'static str,
    /// The layout constraints the engine sent with the frame.
    pub layout: LayoutHint,
    /// The theme the engine sent with the frame.
    pub theme: ThemeSpec,
}

impl UiMetrics {
    /// Assembles the live view from a backend reading, a frame's layout and a theme.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new(backend: BackendReading, layout: LayoutHint, theme: ThemeSpec) -> Self {
        Self {
            window_dp: backend.window_dp,
            window_px: backend.window_px,
            scale: backend.scale,
            base_alpha: backend.base_alpha,
            compositor_present: backend.compositor_present,
            argb_visual: backend.argb_visual,
            backend_id: backend.backend_id,
            layout,
            theme,
        }
    }

    /// The alpha the base should be painted with, given what the window can carry.
    ///
    /// This is the rule applied to this window's own flags; comparing it with
    /// [`UiMetrics::base_alpha`] is what detects a degradation that did not take effect.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn expected_base_alpha(&self) -> u8 {
        effective_base_alpha(
            self.theme.base_alpha,
            self.argb_visual,
            self.compositor_present,
        )
    }
}

/// Which value of the running window a row of the specification is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeField {
    /// The theme's container corner radius.
    CornerRadiusDp,
    /// The frame's maximum container width.
    MaxWidthDp,
    /// The frame's maximum candidates per row.
    MaxPerRow,
}

impl RuntimeField {
    /// The value the running window carries.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn value(self, live: &UiMetrics) -> MetricValue {
        match self {
            Self::CornerRadiusDp => MetricValue::LengthDp(live.theme.corner_radius_dp),
            Self::MaxWidthDp => MetricValue::LengthDp(live.layout.max_width_dp),
            Self::MaxPerRow => MetricValue::Count(u16::from(live.layout.max_per_row)),
        }
    }
}

/// One row of the specification, and where the running window states the same value.
struct RuntimeBinding {
    /// The row's element label, as the document writes it.
    label: &'static str,
    /// The unit the row states the value in.
    unit: Unit,
    /// Which of the row's values of that unit this binding reads, counted from zero.
    index: usize,
    /// The value of the running window it is compared against.
    field: RuntimeField,
}

/// The rows of `features.md` 3.1.1 whose value the running window also carries.
///
/// The mapping is by the document's own element label rather than by position, so a row that
/// moves is still found, and a row that is renamed is reported as gone rather than compared
/// against whatever now sits where it used to. Only three rows are bound, because only three
/// values the window carries are stated by this table at all; the rest of the table describes
/// how the window is drawn, which is the screenshot channel's business.
const RUNTIME_BINDINGS: [RuntimeBinding; 3] = [
    RuntimeBinding {
        label: "容器圆角",
        unit: Unit::Dp,
        index: 0,
        field: RuntimeField::CornerRadiusDp,
    },
    RuntimeBinding {
        label: "候选框最大宽度",
        unit: Unit::Dp,
        index: 0,
        field: RuntimeField::MaxWidthDp,
    },
    RuntimeBinding {
        label: "单行最大候选数",
        unit: Unit::Count,
        index: 0,
        field: RuntimeField::MaxPerRow,
    },
];

/// Largest surface dimension any backend will create, in physical pixels.
///
/// Mirrors `ime_ui::platform::MAX_DIMENSION`, which is the ceiling the backend clamps to before
/// it allocates a buffer. It is repeated here for the same reason [`effective_base_alpha`] is --
/// `xtask` links the frozen contract and not the renderer -- and a change to one is a change to
/// both.
const MAX_SURFACE_DIMENSION: u32 = 8192;

/// One logical dimension as the physical pixels the surface is created with.
///
/// This is the conversion `features.md` 3 states -- every size is a `dp` and is multiplied by the
/// scale and rounded to a physical pixel before anything is drawn -- and the one
/// `ime_ui::platform::physical_dimension` applies: the product is rounded, and a result a surface
/// cannot have is clamped rather than propagated, because the number is what a buffer is
/// allocated from. Keeping the two in step is what makes the geometry half of
/// [`cross_check_spec`] a check of the backend rather than of this module.
///
/// # Return value
///
/// The physical size, clamped to the range a surface can have: never zero, and never past the
/// ceiling the backend would have clamped it to itself.
///
/// # Panics
///
/// Never.
pub fn physical_dimension(dp: u32, scale: f32) -> u32 {
    let px = (dp as f32 * scale).round();
    let px = if px.is_finite() { px } else { 1.0 };
    px.clamp(1.0, MAX_SURFACE_DIMENSION as f32) as u32
}

/// The alpha the surface base is painted with, given what the window can carry.
///
/// The rule the X11 backend applies: a requested alpha survives only when the window has a
/// 32-bit ARGB visual *and* a compositor will blend it; otherwise the base is forced opaque,
/// because an unblended alpha channel shows as black. A Wayland backend with a negotiated blur
/// follows the same shape -- the request survives only where something can honour it.
///
/// # Panics
///
/// Never.
pub fn effective_base_alpha(requested: u8, argb_visual: bool, compositor_present: bool) -> u8 {
    if argb_visual && compositor_present {
        requested
    } else {
        OPAQUE_ALPHA
    }
}

/// Compares the specification with the live metrics and the `.slint` constants.
///
/// Every mismatch is reported rather than the first: one run has to show the whole drift, since
/// a specification, a source and a window that disagree in four places are four things to fix.
/// An empty result means the three sources agree about everything this module knows how to
/// compare, and a result that holds only [`MetricMismatch::Unavailable`] means the agreement is
/// over less than the caller asked for.
///
/// The window is also checked against itself: its physical size has to follow from its logical
/// size at the scale it reports. That comparison involves no document, which is exactly why it
/// is here -- a backend that got its own arithmetic wrong would otherwise agree with every table
/// it was compared against.
///
/// # Parameters
///
/// * `spec` -- the tables read out of `features.md`.
/// * `live` -- what the backend, the frame and the theme say about the running window.
/// * `slint` -- the metrics block, or the reason there is none.
///
/// # Panics
///
/// Never.
pub fn cross_check_spec(
    spec: &SpecTable,
    live: &UiMetrics,
    slint: &MetricSource<SlintConstants>,
) -> Vec<MetricMismatch> {
    let mut mismatches = Vec::new();
    check_window_size(live, &mut mismatches);
    check_slint_constants(spec, slint, &mut mismatches);
    check_runtime_rows(spec, live, &mut mismatches);
    check_base_alpha(spec, live, &mut mismatches);
    mismatches
}

/// Compares the surface's physical size with the size its logical one and its scale fix.
///
/// One verdict per dimension, so a surface that is wrong in one of them says which. The check
/// runs whether or not any document was readable, because the numbers it compares are the
/// window's own: a scale the backend corrected, a buffer sized before a scale change landed, or
/// an off-by-one in the rounding are all cases where every source agrees with every table and
/// the window still draws at the wrong size.
///
/// # Panics
///
/// Never.
fn check_window_size(live: &UiMetrics, mismatches: &mut Vec<MetricMismatch>) {
    let axes = [
        ("width", live.window_dp.0, live.window_px.0),
        ("height", live.window_dp.1, live.window_px.1),
    ];
    for (axis, dp, px) in axes {
        let expected = physical_dimension(dp, live.scale);
        if expected != px {
            mismatches.push(MetricMismatch::Geometry {
                axis,
                dp,
                scale: live.scale,
                expected,
                found: px,
            });
        }
    }
}

/// Compares every exception 3.1.4 names with the constant the `.slint` block declares.
///
/// # Panics
///
/// Never.
fn check_slint_constants(
    spec: &SpecTable,
    slint: &MetricSource<SlintConstants>,
    mismatches: &mut Vec<MetricMismatch>,
) {
    let constants = match slint {
        MetricSource::Unavailable { reason } => {
            mismatches.push(MetricMismatch::Unavailable {
                source: MetricSourceId::Slint,
                reason: reason.clone(),
            });
            return;
        }
        MetricSource::Available(constants) => constants,
    };
    for (name, value) in spec.exceptions() {
        let stated = MetricValue::LengthDp(*value);
        match constants.value(name) {
            None => mismatches.push(MetricMismatch::Missing {
                name: name.clone(),
                source: MetricSourceId::Slint,
            }),
            Some(found) if *found == stated => {}
            Some(found) => mismatches.push(MetricMismatch::Value {
                name: name.clone(),
                spec: stated,
                code: found.clone(),
                source: MetricSourceId::Slint,
            }),
        }
    }
}

/// Compares the rows of 3.1.1 the running window also carries.
///
/// # Panics
///
/// Never.
fn check_runtime_rows(spec: &SpecTable, live: &UiMetrics, mismatches: &mut Vec<MetricMismatch>) {
    for binding in &RUNTIME_BINDINGS {
        let Some(row) = spec.row(binding.label) else {
            mismatches.push(MetricMismatch::Missing {
                name: String::from(binding.label),
                source: MetricSourceId::Spec,
            });
            continue;
        };
        let Some(stated) = row.value(binding.unit, binding.index) else {
            mismatches.push(MetricMismatch::Unavailable {
                source: MetricSourceId::Spec,
                reason: format!(
                    "3.1.1 `{}` states no `{}` value",
                    binding.label,
                    binding.unit.suffix()
                ),
            });
            continue;
        };
        let carried = binding.field.value(live);
        match row.configurable {
            // The row states a range, so it fixes a default rather than a value: a window
            // configured anywhere inside the range agrees with it.
            Some((min, max)) => {
                if row.contains(carried.clone()) == Some(false) {
                    mismatches.push(MetricMismatch::OutOfRange {
                        name: String::from(binding.label),
                        value: carried,
                        min,
                        max,
                        source: MetricSourceId::Runtime,
                    });
                }
            }
            None => {
                if stated != carried {
                    mismatches.push(MetricMismatch::Value {
                        name: String::from(binding.label),
                        spec: stated,
                        code: carried,
                        source: MetricSourceId::Runtime,
                    });
                }
            }
        }
    }
}

/// Compares the acrylic base alpha with the specification and with the backend's own rule.
///
/// The scheme the window is in decides which column of 3.2 applies. Reading the dark column for
/// a light window would compare it against an alpha nothing asked it to have: a mismatch the day
/// the two columns differ, and a drift nobody sees until then.
///
/// # Panics
///
/// Never.
fn check_base_alpha(spec: &SpecTable, live: &UiMetrics, mismatches: &mut Vec<MetricMismatch>) {
    let column = spec
        .colour(BASE_ALPHA_TOKEN)
        .map(|row| match live.theme.scheme {
            ColorScheme::Dark => row.dark,
            ColorScheme::Light => row.light,
        });
    match column.and_then(|cell| cell.alpha) {
        Some(fraction) => {
            let stated = MetricValue::Alpha8(alpha_byte(fraction));
            let requested = MetricValue::Alpha8(live.theme.base_alpha);
            if stated != requested {
                mismatches.push(MetricMismatch::Value {
                    name: String::from(BASE_ALPHA_ITEM),
                    spec: stated,
                    code: requested,
                    source: MetricSourceId::Runtime,
                });
            }
        }
        None => mismatches.push(MetricMismatch::Missing {
            name: String::from(BASE_ALPHA_ITEM),
            source: MetricSourceId::Spec,
        }),
    }
    // The second half is the degradation itself: an alpha that survived where nothing can blend
    // it shows as black, and an alpha forced opaque where the compositor could have blended it
    // is the acrylic the design asks for, missing.
    let expected = MetricValue::Alpha8(live.expected_base_alpha());
    let painted = MetricValue::Alpha8(live.base_alpha);
    if expected != painted {
        mismatches.push(MetricMismatch::Value {
            name: String::from(EFFECTIVE_ALPHA_ITEM),
            spec: expected,
            code: painted,
            source: MetricSourceId::Runtime,
        });
    }
}
