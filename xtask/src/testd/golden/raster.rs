//! The fixture raster: the candidate grid the first baseline batch was drawn with.
//!
//! It exists so the golden-diff channel has frames that are reproducible *by construction*
//! on any machine: rectangles and hairlines in integer arithmetic, no fonts, no blur, no
//! compositor. It is a closed-world stand-in in the same sense the dictionary fixtures are
//! -- the shapes it draws follow the theme system's tokens (the panel, the 4dp cell gap,
//! the hairline, the hover and selected fills), so a baseline batch drawn with it exercises
//! the comparison against real-shaped data -- but it is the fixture's own raster, not the
//! production window's. Baselines for the production renderer enter the library through
//! `--bless` from a capture, which is the one door the library has.
//!
//! # Determinism
//!
//! Every coordinate is a logical dp value multiplied by the device pixel ratio, and every
//! colour is a constant of this file. The same grid rastered twice is the same bytes, which
//! is what the baselines' zero-diff contract rests on.
//!
//! # What the states draw
//!
//! [`GridState::Default`] draws the resting grid: the panel, five cells, the hairlines
//! between them. [`GridState::Hover`] fills the second cell the way a pointer resting on it
//! does. [`GridState::Selected`] fills the first cell and draws the accent bar under it, the
//! way a highlighted candidate reads. Text is deliberately absent -- glyph shapes belong to
//! whatever font the machine resolves, which is exactly the kind of frame the library may
//! not hold.

use super::{BaselineName, RgbaFrame};

/// The bytes one pixel occupies.
const BYTES_PER_PIXEL: usize = 4;

/// The fixture grid's logical size: five candidate cells and their margins, in dp.
pub const GRID_DP: (u32, u32) = (600, 140);

/// One cell's logical width, including its trailing 4dp gap.
const CELL_DP: u32 = 112;

/// The cells' logical height.
const CELL_HEIGHT_DP: u32 = 116;

/// The logical margin between the panel edge and the first cell.
const MARGIN_DP: u32 = 12;

/// How many cells the grid holds.
const CELLS: u32 = 5;

/// Which interaction state the grid is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridState {
    /// The resting grid.
    Default,
    /// The second cell under a resting pointer.
    Hover,
    /// The first cell highlighted as the selected candidate.
    Selected,
}

impl GridState {
    /// The state a CLI argument names.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "default" => Some(Self::Default),
            "hover" => Some(Self::Hover),
            "selected" => Some(Self::Selected),
            _ => None,
        }
    }

    /// The state as a phrase, for a report line.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Hover => "hover",
            Self::Selected => "selected",
        }
    }
}

/// The palette the grid is drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridTheme {
    /// The dark palette.
    Dark,
    /// The light palette.
    Light,
}

impl GridTheme {
    /// The theme a baseline name or a CLI argument names.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }

    /// The theme as the baseline name carries it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// The five colours the palette is: `(panel, cell, hover, selected, accent)`.
    fn colors(self) -> [[u8; 4]; 5] {
        match self {
            Self::Dark => [
                [0x20, 0x21, 0x24, 0xFF], // panel
                [0x2D, 0x2E, 0x31, 0xFF], // cell
                [0x3C, 0x40, 0x43, 0xFF], // hover
                [0x3E, 0x4B, 0x5E, 0xFF], // selected
                [0x8A, 0xB4, 0xF8, 0xFF], // accent
            ],
            Self::Light => [
                [0xFF, 0xFF, 0xFF, 0xFF], // panel
                [0xF8, 0xF9, 0xFA, 0xFF], // cell
                [0xE8, 0xEA, 0xED, 0xFF], // hover
                [0xD2, 0xE3, 0xFC, 0xFF], // selected
                [0x1A, 0x73, 0xE8, 0xFF], // accent
            ],
        }
    }
}

/// Draws the fixture grid and answers the frame.
///
/// # Panics
///
/// Never.
pub fn raster(state: GridState, theme: GridTheme, scale: u32) -> RgbaFrame {
    let width = GRID_DP.0 * scale;
    let height = GRID_DP.1 * scale;
    let [panel, cell, hover, selected, accent] = theme.colors();
    let mut frame = RgbaFrame::filled(width, height, panel);
    // The five cells, each at its logical place times the ratio. The 4dp gap stays panel,
    // which is what makes the gap readable in a diff.
    for index in 0..CELLS {
        let x = (MARGIN_DP + index * CELL_DP) * scale;
        let y = MARGIN_DP * scale;
        let w = (CELL_DP - 4) * scale;
        let h = CELL_HEIGHT_DP * scale;
        let color = match state {
            GridState::Default => cell,
            GridState::Hover => {
                if index == 1 {
                    hover
                } else {
                    cell
                }
            }
            GridState::Selected => {
                if index == 0 {
                    selected
                } else {
                    cell
                }
            }
        };
        fill(&mut frame, x, y, w, h, color);
        // The selected cell's accent bar: 3dp tall, at the cell's bottom edge.
        if matches!(state, GridState::Selected) && index == 0 {
            fill(&mut frame, x, y + h - 3 * scale, w, 3 * scale, accent);
        }
    }
    frame
}

/// The baseline name this grid combination is pinned under.
///
/// The interaction state rides in the subject -- `grid_hover`, not `grid` -- because two
/// states that filed under one name would bless over each other, and the library would
/// then hold one baseline where the batch declares three.
///
/// # Panics
///
/// Never.
pub fn baseline_name(state: GridState, theme: GridTheme, scale: u32) -> String {
    BaselineName {
        subject: format!("grid_{}", state.name()),
        width: GRID_DP.0 * scale,
        height: GRID_DP.1 * scale,
        theme,
        scale,
    }
    .render()
}

/// Fills a rectangle of the frame with one colour.
///
/// Out-of-range edges are clipped rather than refused: the raster's arithmetic is the only
/// writer, and a clipped fill keeps a coordinate slip from becoming a panic.
///
/// # Panics
///
/// Never.
fn fill(frame: &mut RgbaFrame, x: u32, y: u32, w: u32, h: u32, color: [u8; 4]) {
    for row in y..(y + h).min(frame.height) {
        let row_start = (row as usize) * (frame.width as usize);
        for column in x..(x + w).min(frame.width) {
            let offset = row_start + column as usize;
            let bytes = &mut frame.pixels[offset * BYTES_PER_PIXEL..(offset + 1) * BYTES_PER_PIXEL];
            bytes.copy_from_slice(&color);
        }
    }
}
