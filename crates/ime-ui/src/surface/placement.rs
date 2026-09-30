//! Where the panel goes, and what the pointer can hit.
//!
//! Responsibility: turn the frame being drawn, the anchor the host sent and the component's
//! own constants into a placement, write the side it landed on into the component, and publish
//! the two things the rest of the surface reads back -- the panel's rectangle, which becomes
//! the window's interactive region, and the hit map the pointer is tested against.
//!
//! Boundaries: this module decides nothing about what a candidate is worth and keeps no input
//! state of its own; a pointer position is only ever tested against the map, never remembered
//! here. It is the only caller of the geometry pass, and it reads the component through the
//! adapter rather than writing to it beyond the placement the frame implies.
//!
//! # Why the region is the panel and not the window
//!
//! The window carries a transparent shadow reserve on all four sides, and that reserve has to
//! stay out of the interactive region: a click landing in it must reach the application
//! underneath instead of being swallowed by a candidate window that draws nothing there. The
//! region therefore comes from the placement's container and not from its window, and a backend
//! that refuses to shape it is counted rather than treated as fatal -- the window still draws,
//! and the counter is what a diagnostic reports.

use ime_types::RectI;

use crate::adapter::DrawState;
use crate::geometry::{self, Desktop, Panel, PlacementRequest};
use crate::layout::ContainerSize;

use super::CandidateSurface;

impl CandidateSurface {
    /// Places the window for the frame it is drawing and applies its interactive region.
    pub(super) fn place(&mut self) {
        let Some(frame) = self.frame.as_deref() else {
            return;
        };
        let anchor = self.anchor.unwrap_or(frame.anchor);
        let panel = {
            let state = self.adapter.state();
            Panel {
                size: ContainerSize {
                    width: state.container_width,
                    height: state.container_height,
                },
                cell_width: state.cell_width,
                columns: columns(state),
            }
        };
        let request = PlacementRequest::new(
            &anchor,
            // The outputs of the virtual desktop are a platform capability this layer does not
            // have, so the pass falls back to the anchor's own output and keeps the window
            // where the caret puts it: without a known output it can neither flip the window
            // above the caret nor clamp it to the screen edge.
            Desktop {
                screens: &[],
                primary: anchor.screen,
            },
            panel,
            frame,
            self.metrics,
        );
        let geometry = geometry::compute(&request);
        // Where the panel landed decides which edge the appear motion grows from: a window
        // flipped above the caret has to grow downwards, or the panel would slide out from
        // under the cursor.
        self.adapter.set_placement(geometry.placement);
        // The caret arrow points from the panel's edge at the caret. Its position is a
        // function of the geometry the placement just produced, so it is written here and
        // nowhere else: a caller that recomputed it would be a second answer to the same
        // question, and the two would drift the first time the geometry changed.
        self.adapter
            .set_arrow(crate::adapter::arrow_in_container(&geometry));
        let region = RectI {
            x: geometry.container_offset.0,
            y: geometry.container_offset.1,
            w: geometry.container_size.0,
            h: geometry.container_size.1,
        };
        self.geometry = Some(geometry);
        self.region = Some(region);
        // The router re-hit-tests a pointer that is not moving against the new cells, and it
        // needs the event queue to report the hover that follows. This path has none, so the
        // adoption is left for the next `drain_events`, which does.
        self.adoption_pending = true;
        if self.platform.set_input_region(&[region]).is_err() {
            self.region_failures = self.region_failures.saturating_add(1);
        }
    }

    /// The panel's rectangle in window-relative physical pixels, once a frame has been placed.
    ///
    /// This is the region the pointer can hit; everything outside it is the transparent shadow
    /// reserve, which stays out of the interactive region so that a click there reaches the
    /// application underneath.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn input_region(&self) -> Option<RectI> {
        self.region
    }

    /// One entry per visible candidate of the last placement, in container-relative physical
    /// pixels, paired with the candidate's global index.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn hit_map(&self) -> &[(RectI, u16)] {
        match self.geometry.as_ref() {
            Some(geometry) => &geometry.hit_map,
            None => &[],
        }
    }

    /// How many times the interactive region could not be applied.
    ///
    /// A surface whose region cannot be shaped still draws, but the pointer falls through the
    /// whole window instead of only through the reserve around the panel, so the counter is
    /// what a diagnostic reports.
    ///
    /// # Errors
    ///
    /// This function is infallible: it returns no `Result`.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn region_failures(&self) -> u64 {
        self.region_failures
    }
}

/// Candidates per row as the component draws them, for the placement pass.
fn columns(state: &DrawState) -> u8 {
    state.max_per_row.clamp(1, i32::from(u8::MAX)) as u8
}
