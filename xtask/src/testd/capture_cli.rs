//! `xtask capture` -- the command-line surface of the screenshot channel.
//!
//! # Responsibility
//!
//! Connect to the X server the session under test runs on, read one window's pixels at the
//! resolution the window itself holds, and write them as a PNG. The parent module owns the
//! frame arithmetic, the byte order and the read loop; this module parses the arguments that
//! name them and reports what was written. It is also what makes the channel reachable from
//! the binary that contains it: in a binary crate an item nothing names is dead code however
//! public it is.
//!
//! # What the report names
//!
//! The display, the pixel dimensions, the recorded scale and the path -- never the pixels
//! themselves, which is what the PNG is for. The image is the evidence and the terminal
//! names the file, the shape every other channel of this harness reports in.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;

use crate::testd::capture::{CaptureRequest, CaptureTarget, CapturedImage, capture};
use crate::testd::coords::normalize_scale;
use crate::testd::x11::{Window, X11Session};

/// Command-line surface of `xtask capture`.
#[derive(Debug, Args)]
pub struct CaptureArgs {
    /// X display to capture from; defaults to `$DISPLAY`.
    #[arg(long)]
    display: Option<String>,
    /// Window to capture: decimal, or hexadecimal with a `0x` prefix.
    #[arg(long, value_parser = crate::testd::parse_window)]
    window: Window,
    /// Device pixel ratio the window was rasterised at; the value written into the PNG.
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
    /// PNG file to write; its directory is created if it is missing.
    #[arg(long, value_name = "PATH")]
    out: PathBuf,
}

/// The request the arguments describe.
///
/// The pointer is never asked for: `get_image` reads a drawable's own pixels and cannot
/// composite the cursor, so the channel refuses a cursor-inclusive capture rather than
/// answering with a frame that quietly lacks it. The scale is normalised here as well as in
/// the capture itself -- idempotently -- so a degenerate `--scale` cannot survive to the
/// ratio the PNG records.
///
/// # Panics
///
/// Never.
fn build_request(args: &CaptureArgs) -> CaptureRequest {
    CaptureRequest {
        target: CaptureTarget::Window(args.window),
        include_cursor: false,
        scale: normalize_scale(args.scale),
        out_path: args.out.clone(),
    }
}

/// Entry point for `xtask capture`.
///
/// # Errors
///
/// Returns an error when the display cannot be opened, when the capture is refused -- an
/// unmapped window, an unsupported visual, a frame past the size ceiling, a drawable that
/// moved while it was being read -- and when the PNG cannot be written.
///
/// # Panics
///
/// Never.
pub fn run(args: CaptureArgs) -> Result<()> {
    let request = build_request(&args);
    let session = X11Session::connect(args.display.as_deref())
        .context("opening the display the capture reads from")?;
    let CapturedImage {
        width_px,
        height_px,
        scale,
    } = capture(&session, &request)?;
    println!(
        "capture: {} holds {width_px}x{height_px} physical pixels at recorded scale {scale}, \
         written to {}",
        session.display(),
        args.out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_request_names_the_window_output_and_a_cursor_free_capture() {
        let args = CaptureArgs {
            display: None,
            window: 0x40_0001,
            scale: 2.0,
            out: PathBuf::from("/tmp/run/cell.png"),
        };
        let request = build_request(&args);
        assert_eq!(request.target, CaptureTarget::Window(0x40_0001));
        assert!(
            !request.include_cursor,
            "the channel cannot composite the pointer, so the request must not ask for it"
        );
        assert_eq!(request.scale, 2.0);
        assert_eq!(request.out_path, PathBuf::from("/tmp/run/cell.png"));
    }

    #[test]
    fn test_build_request_normalizes_a_degenerate_scale() {
        for degenerate in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let args = CaptureArgs {
                display: None,
                window: 1,
                scale: degenerate,
                out: PathBuf::from("out.png"),
            };
            assert_eq!(
                build_request(&args).scale,
                1.0,
                "a degenerate scale {degenerate} must not survive into the request"
            );
        }
    }
}
