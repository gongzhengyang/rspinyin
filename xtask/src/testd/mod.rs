//! `xtask testd` -- the input half of the automated test platform.
//!
//! Responsibility: turn a test's intent ("type `nihao` into this window", "click that
//! candidate cell") into real X11 input events, and refuse to report success when the
//! server disagrees about where those events went.
//!
//! # The path events take
//!
//! Events go out through the XTEST extension, so they travel the same route a human's key
//! does: X server, `xcb` frontend, Fcitx5, the addon. Driving the engine in-process would
//! be faster and would prove nothing about that route; the two channels stay separate, and
//! neither stands in for the other.
//!
//! # The rule this harness exists to keep
//!
//! Losing the keyboard focus is the project's highest-severity defect, so injection is
//! impossible without naming the window that must keep it: every key, click and scroll
//! takes a [`FocusGuard`](input::FocusGuard), which asks the server for the focus before
//! and after each event and fails the run the moment it moves.
//!
//! # What this channel does not do
//!
//! It cannot read what the application received -- that is the client-text channel's job --
//! and it asserts no pixel. This module produces input and a focus verdict, nothing else.
//!
//! # Modules
//!
//! [`keys`] holds the keysym, modifier and character tables, [`coords`] the coordinate
//! conversion, [`x11`] the connection and the protocol, and [`input`] the injection policy.

pub mod coords;
pub mod engine;
pub mod input;
pub mod keys;
pub mod sandbox;
pub mod x11;

use std::time::Duration;

use anyhow::{Context, Result};
use clap::Args;

use crate::testd::input::{DEFAULT_KEY_DELAY, FocusGuard, X11Injector};
use crate::testd::x11::Window;

/// Everything the injection channel can fail with.
///
/// The variants are the channel's contract with the caller: `FocusRefused` and
/// `FocusStolen` are different verdicts -- the first says the focus never arrived, the
/// second says something took it while events were being sent -- and a test runner maps
/// them onto different severities.
#[derive(Debug, thiserror::Error)]
pub enum TestError {
    /// The X server could not be reached.
    #[error("cannot open the X display {display}: {detail}")]
    NoDisplay {
        /// The display name that was tried, or `$DISPLAY` when none was given.
        display: String,
        /// What the connection attempt reported.
        detail: String,
    },

    /// The server does not offer the XTEST extension.
    #[error(
        "the X server on {display} has no usable XTEST extension, so no key or pointer \
         event can be injected; run against a server that has one (Xvfb and Xephyr do, and \
         some nested servers do not), or drive the engine through the in-process channel \
         instead"
    )]
    NoXtest {
        /// The display name that was tried.
        display: String,
    },

    /// The server would not move the input focus to the window under test.
    #[error("the input focus is {actual:#x}, not {expected:#x}: the server refused the request")]
    FocusRefused {
        /// The window the test asked for.
        expected: Window,
        /// The window the server reports.
        actual: Window,
    },

    /// The input focus left the window under test while events were being injected.
    #[error(
        "the input focus moved from {expected:#x} to {actual:#x} during injection \
         (that window is the candidate window: {by_candidate_window})"
    )]
    FocusStolen {
        /// The window that was supposed to keep the focus.
        expected: Window,
        /// The window that holds it now.
        actual: Window,
        /// Whether `actual` carries the candidate window's `WM_CLASS`.
        by_candidate_window: bool,
    },

    /// A screen coordinate fell outside the screen.
    #[error("the point {x},{y} is not on the {width}x{height} screen")]
    OffScreen {
        /// The x coordinate that was asked for.
        x: i64,
        /// The y coordinate that was asked for.
        y: i64,
        /// The screen's width in pixels.
        width: u16,
        /// The screen's height in pixels.
        height: u16,
    },

    /// The keyboard mapping has no key that produces a keysym.
    #[error("no keycode in the server's keyboard mapping produces keysym {keysym:#x}")]
    NoKeycode {
        /// The keysym that could not be typed.
        keysym: u32,
    },

    /// A modifier mask names a modifier the channel cannot hold.
    #[error(
        "the modifier mask {state:#06x} asks for a modifier this channel cannot hold; \
         it holds Shift, Control, Alt and Super"
    )]
    UnsupportedModifiers {
        /// The mask the caller asked for.
        state: u16,
    },

    /// A character has no key on the layout the channel types on.
    #[error("the character {ch:?} has no key on the US layout this channel types on")]
    UnsupportedChar {
        /// The character that could not be typed.
        ch: char,
    },

    /// An X request failed.
    #[error("X request failed: {detail}")]
    Request {
        /// What the failing request reported.
        detail: String,
    },
}

/// Command-line surface of `xtask testd`.
#[derive(Debug, Args)]
pub struct TestdArgs {
    /// X display to inject into; defaults to `$DISPLAY`.
    #[arg(long)]
    display: Option<String>,
    /// Device pixel ratio the candidate window is rasterised with.
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
    /// Report the environment and the current focus, then exit without injecting.
    #[arg(long)]
    probe: bool,
    /// Window to focus before injecting: decimal, or hexadecimal with a `0x` prefix.
    #[arg(long, value_parser = parse_window)]
    window: Option<Window>,
    /// Text to type into the focused window, one key per character.
    #[arg(long)]
    text: Option<String>,
    /// How many times to type `--text`; twenty rounds of `nihao` is the hundred-key check.
    #[arg(long, default_value_t = 1)]
    repeat: u32,
    /// Delay between two injected keys, in milliseconds; `--delay-ms 0` removes it.
    #[arg(long)]
    delay_ms: Option<u64>,
    /// Named key to press once: `tab`, `return`, `escape`, `backspace`, `left`, `up`,
    /// `right`, `down`, `space`, `minus`, `equal`, `shift`, `ctrl`, `alt` or `super`.
    #[arg(long)]
    key: Option<String>,
    /// Modifiers to hold for `--key`, comma separated: `shift`, `ctrl`, `alt`, `super`.
    #[arg(long)]
    mods: Option<String>,
    /// Click at `X,Y` in screen pixels.
    #[arg(long, value_parser = parse_point)]
    click: Option<(i32, i32)>,
    /// Click at `X,Y` in candidate-container pixels, resolved through the window's own
    /// placement; this is how a test aims at a cell of the hit map.
    #[arg(long, value_parser = parse_point)]
    hit: Option<(i32, i32)>,
    /// Pointer button for `--click` and `--hit`.
    #[arg(long, default_value_t = 1)]
    button: u8,
    /// Scroll at `X,Y`, in screen pixels.
    #[arg(long, value_parser = parse_point)]
    scroll: Option<(i32, i32)>,
    /// Wheel notches for `--scroll`; negative scrolls back.
    #[arg(long, default_value_t = 1)]
    notches: i32,
}

/// Entry point for `xtask testd`.
///
/// # Errors
///
/// Returns an error when the display cannot be opened, when it has no XTEST extension,
/// when the focus cannot be placed on the window under test, or when any injected event
/// moves the focus away from it.
pub fn run(args: TestdArgs) -> Result<()> {
    let injector = X11Injector::connect(args.display.as_deref(), args.scale)?;
    report(&injector);
    if args.probe {
        return Ok(());
    }
    let window = args
        .window
        .context("--window <id> is required unless --probe is given")?;
    let guard = injector.focus(window)?;
    println!("testd: the server confirms the focus is on {window:#x}");
    inject(&injector, &guard, &args)?;
    guard.verify(&injector)?;
    let window = guard.window();
    println!("testd: the focus is still on {window:#x} after the injection");
    Ok(())
}

/// Types, presses, clicks and scrolls whatever the arguments asked for.
fn inject(injector: &X11Injector, guard: &FocusGuard, args: &TestdArgs) -> Result<()> {
    inject_keys(injector, guard, args)?;
    inject_pointer(injector, guard, args)
}

/// Sends the text and the named key the arguments asked for.
fn inject_keys(injector: &X11Injector, guard: &FocusGuard, args: &TestdArgs) -> Result<()> {
    let delay = args.delay_ms.map_or(DEFAULT_KEY_DELAY, Duration::from_millis);
    if let Some(text) = &args.text {
        let keys = keys::strokes(text)?.len();
        let rounds = args.repeat;
        for round in 1..=rounds {
            injector.type_text(guard, text, delay)?;
            println!("testd: round {round}/{rounds} typed {keys} keys");
        }
    }
    if let Some(name) = &args.key {
        let keysym = keys::named_keysym(name)
            .with_context(|| format!("`{name}` is not a key name this channel knows"))?;
        let state = args.mods.as_deref().map_or(Ok(0), keys::modifier_mask)?;
        injector.key(guard, keysym, state)?;
        println!("testd: pressed {name} with modifiers {state:#06x}");
    }
    Ok(())
}

/// Sends the pointer events the arguments asked for.
fn inject_pointer(injector: &X11Injector, guard: &FocusGuard, args: &TestdArgs) -> Result<()> {
    let button = args.button;
    if let Some(point) = args.click {
        let point = injector.on_screen(point)?;
        let (x, y) = point;
        injector.click(guard, point, button)?;
        println!("testd: clicked button {button} at {x},{y}");
    }
    if let Some(point) = args.hit {
        let placement = injector.placement_of(guard.window())?;
        let (left, top) = placement.origin;
        let (width, height) = placement.size;
        let scale = placement.scale;
        let window = guard.window();
        println!("testd: window {window:#x} at {left},{top} size {width}x{height} scale {scale}");
        let point = injector.hit_point(&placement, point)?;
        let (x, y) = point;
        injector.click(guard, point, button)?;
        println!("testd: clicked button {button} at {x},{y}");
    }
    if let Some(point) = args.scroll {
        let point = injector.on_screen(point)?;
        let (x, y) = point;
        let notches = args.notches;
        injector.scroll(guard, point, notches)?;
        println!("testd: scrolled {notches} notch(es) at {x},{y}");
    }
    Ok(())
}

/// Prints what the channel sees, before anything is injected.
fn report(injector: &X11Injector) {
    let (width, height) = injector.screen();
    println!(
        "testd: display {} root {:#x} screen {width}x{height} scale {}",
        injector.display(),
        injector.root(),
        injector.scale()
    );
    println!(
        "testd: XTEST {}.{} available on fd {}",
        injector.xtest().major,
        injector.xtest().minor,
        injector.connection_fd()
    );
    match injector.input_focus() {
        Ok(window) => println!("testd: focus {window:#x}"),
        Err(error) => println!("testd: focus unavailable: {error}"),
    }
}

/// Parses a window id in decimal, or hexadecimal with a `0x` prefix.
fn parse_window(text: &str) -> Result<Window, String> {
    let trimmed = text.trim();
    let (digits, radix) = match trimmed.strip_prefix("0x").or_else(|| trimmed.strip_prefix("0X")) {
        Some(hex) => (hex, 16),
        None => (trimmed, 10),
    };
    Window::from_str_radix(digits, radix).map_err(|_| format!("`{text}` is not a window id"))
}

/// Parses a screen point written as `X,Y`.
fn parse_point(text: &str) -> Result<(i32, i32), String> {
    let (x, y) = text.split_once(',').ok_or_else(|| point_error(text))?;
    let x = x.trim().parse::<i32>().map_err(|_| point_error(text))?;
    let y = y.trim().parse::<i32>().map_err(|_| point_error(text))?;
    Ok((x, y))
}

/// The refusal a malformed point is reported with.
fn point_error(text: &str) -> String {
    format!("`{text}` is not an X,Y point")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_window_accepts_decimal_and_hexadecimal_ids() {
        assert_eq!(parse_window("4194305"), Ok(4194305));
        assert_eq!(parse_window("0x400001"), Ok(4194305));
        assert_eq!(parse_window(" 0X400001 "), Ok(4194305));
        assert!(parse_window("0x").is_err());
        assert!(parse_window("").is_err());
        assert!(parse_window("the candidate window").is_err());
        assert!(parse_window("-1").is_err());
    }

    #[test]
    fn test_parse_point_accepts_an_x_y_pair() {
        assert_eq!(parse_point("100,200"), Ok((100, 200)));
        assert_eq!(parse_point(" -5 , 7 "), Ok((-5, 7)));
        assert!(parse_point("100").is_err());
        assert!(parse_point("100,").is_err());
        assert!(parse_point("x,y").is_err());
        assert!(parse_point("100,200,300").is_err());
    }

    #[test]
    fn test_no_xtest_message_names_the_extension_and_the_way_out() {
        let refusal = TestError::NoXtest {
            display: ":0".to_owned(),
        };
        let message = refusal.to_string();
        assert!(message.contains(":0"), "{message}");
        assert!(message.contains("XTEST"), "{message}");
        assert!(
            message.contains("in-process"),
            "the message must name the alternative channel: {message}"
        );
    }

    #[test]
    fn test_focus_errors_are_distinguishable_and_name_both_windows() {
        let refused = TestError::FocusRefused {
            expected: 0x400001,
            actual: 0x400002,
        };
        let stolen = TestError::FocusStolen {
            expected: 0x400001,
            actual: 0x600003,
            by_candidate_window: true,
        };
        assert_ne!(refused.to_string(), stolen.to_string());
        assert!(refused.to_string().contains("0x400002"), "{refused}");
        assert!(stolen.to_string().contains("0x600003"), "{stolen}");
        assert!(
            stolen.to_string().contains("candidate window: true"),
            "the verdict must say whether the candidate window is the culprit: {stolen}"
        );
    }
}
