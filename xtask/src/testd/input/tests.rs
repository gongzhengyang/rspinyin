//! Tests for the injection channel.
//!
//! # Two groups
//!
//! The first group decides what needs no display server: which wheel button a delta means,
//! how many notches a request stands for, how a string is planned into strokes, and what
//! verdict a focus reading produces. Those run in the ordinary test job.
//!
//! The second group drives a live X server through XTEST and is marked `#[ignore]`. It is
//! the only place the acceptance criteria can be checked end to end -- that twenty rounds
//! of `nihao` lose no key, and that a hundred injected keys never move the focus -- and the
//! lab job runs it with a display present and the ignored tests enabled:
//!
//! ```text
//! DISPLAY=:0 cargo nextest run -p xtask --run-ignored all
//! ```
//!
//! # What the live tests cannot say
//!
//! The client they inject into is a window this file creates. The candidate window does not
//! exist yet, so nothing here can show that the focus survives the candidate window being
//! on screen; what it shows is the half of the rule this channel owns -- that injection
//! never moves the focus by itself.

use std::thread;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    ConnectionExt as XprotoExt, CreateWindowAux, EventMask, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use super::*;
use crate::testd::keys::KS_TAB;

/// Width of the window the live tests inject into, in pixels.
const RECEIVER_WIDTH: u16 = 200;

/// Height of the window the live tests inject into, in pixels.
const RECEIVER_HEIGHT: u16 = 100;

/// Rounds of `nihao` the loss check types; the criterion asks for twenty.
const ROUNDS: usize = 20;

/// Keys the focus check injects; the criterion asks for a hundred.
const KEYS: usize = 100;

/// How long a focus request is retried before the run is failed.
///
/// The retry is for a window manager that is still adopting the window, not for a refusal:
/// a window that never takes the focus is still reported as the failure it is.
const FOCUS_TIMEOUT: Duration = Duration::from_secs(2);

/// How long the event queue is left alone between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(2);

/// How long the loss check waits for the last key it expects.
const RECEIVE_TIMEOUT: Duration = Duration::from_secs(10);

#[test]
fn test_default_key_delay_is_the_pace_the_host_keeps_up_with() {
    assert_eq!(
        DEFAULT_KEY_DELAY,
        Duration::from_millis(8),
        "the default pace is what the loss check is calibrated against"
    );
}

#[test]
fn test_wheel_button_follows_the_axis_sign_convention() {
    assert_eq!(wheel_button(-1), WHEEL_UP, "backwards is button 4");
    assert_eq!(wheel_button(1), WHEEL_DOWN, "forwards is button 5");
    assert_eq!(wheel_button(-3), WHEEL_UP);
    assert_eq!(wheel_button(i32::MIN), WHEEL_UP);
}

#[test]
fn test_notches_counts_the_pairs_and_clamps_the_cap() {
    assert_eq!(notches(0), 0, "a zero delta sends nothing");
    assert_eq!(notches(-1), 1);
    assert_eq!(notches(3), 3);
    assert_eq!(notches(MAX_NOTCHES as i32 + 1), MAX_NOTCHES);
    assert_eq!(
        notches(i32::MIN),
        MAX_NOTCHES,
        "a mistyped delta cannot spin"
    );
}

#[test]
fn test_strokes_plans_the_keys_a_string_will_send() {
    let planned = strokes("nihao").expect("a pinyin string has keys");
    assert_eq!(planned.len(), 5);
    let empty = strokes("").expect("an empty string plans no keys");
    assert!(empty.is_empty());
    assert!(strokes("中").is_err());
}

#[test]
fn test_focused_accepts_the_window_the_server_confirms() {
    assert!(
        focused(0x40_0001, 0x40_0001).is_ok(),
        "the window the test asked for is the window the server reports"
    );
}

#[test]
fn test_focused_refuses_a_focus_the_server_did_not_take() {
    let refusal = focused(0x40_0001, 0x40_0002).expect_err("a different window is a refusal");
    match refusal {
        TestError::FocusRefused { expected, actual } => {
            assert_eq!(expected, 0x40_0001, "the window the test asked for");
            assert_eq!(actual, 0x40_0002, "the window the server kept instead");
        }
        other => panic!("a focus that never arrived must be refused: {other:?}"),
    }
}

#[test]
fn test_focus_held_accepts_the_guarded_window() {
    assert!(focus_held(0x40_0001, 0x40_0001, false).is_ok());
}

#[test]
fn test_focus_held_reports_a_moved_focus_and_names_the_culprit() {
    let stolen = focus_held(0x40_0001, 0x60_0003, true).expect_err("a moved focus must fail");
    match stolen {
        TestError::FocusStolen {
            expected,
            actual,
            by_candidate_window,
        } => {
            assert_eq!(expected, 0x40_0001);
            assert_eq!(actual, 0x60_0003);
            assert!(
                by_candidate_window,
                "the candidate window is named as the culprit"
            );
        }
        other => panic!("a moved focus must be reported as stolen: {other:?}"),
    }
    let elsewhere = focus_held(0x40_0001, 0x60_0003, false).expect_err("a moved focus fails");
    match elsewhere {
        TestError::FocusStolen {
            by_candidate_window,
            ..
        } => {
            assert!(
                !by_candidate_window,
                "another window is not the candidate window"
            );
        }
        other => panic!("a moved focus must be reported as stolen: {other:?}"),
    }
}

/// Creates and maps the window the live tests inject into.
///
/// It stands in for the client under test: it asks for key events and nothing else, so
/// whatever arrives in its queue was aimed at it. It is created on the injector's own
/// connection, because a second connection would be out of step about what the server has
/// already processed.
fn receiver_window(conn: &RustConnection) -> Window {
    let (root, depth, visual) = {
        let setup = conn.setup();
        let screen = setup.roots.first().expect("the server lists a screen");
        (screen.root, screen.root_depth, screen.root_visual)
    };
    let window = conn.generate_id().expect("a free window id");
    conn.create_window(
        depth,
        window,
        root,
        0,
        0,
        RECEIVER_WIDTH,
        RECEIVER_HEIGHT,
        0,
        WindowClass::INPUT_OUTPUT,
        visual,
        &CreateWindowAux {
            event_mask: Some(EventMask::KEY_PRESS | EventMask::KEY_RELEASE),
            ..Default::default()
        },
    )
    .expect("the window is created");
    conn.map_window(window).expect("the window is mapped");
    // A focus request has to find the window viewable, and viewability is only certain once
    // the server has processed the mapping, so the queue is pushed through with a round
    // trip before anything is focused.
    conn.get_input_focus()
        .expect("the query is queued")
        .reply()
        .expect("the server answers");
    window
}

/// Destroys the receiver window and pushes the request out.
fn close_receiver(conn: &RustConnection, window: Window) {
    conn.destroy_window(window).expect("the window is gone");
    conn.flush().expect("the request goes out");
}

/// Focuses `window`, waiting out a window manager that is still adopting it.
///
/// The guard that comes back has already been confirmed by the server; only a request that
/// has not been answered yet is retried.
fn focus_eventually(injector: &X11Injector, window: Window) -> FocusGuard {
    let deadline = Instant::now() + FOCUS_TIMEOUT;
    loop {
        match injector.focus(window) {
            Ok(guard) => return guard,
            Err(error) if Instant::now() >= deadline => {
                panic!("the window never took the focus: {error}")
            }
            Err(_) => thread::sleep(POLL_INTERVAL),
        }
    }
}

/// Counts the key presses delivered to the receiver before `timeout` runs out.
///
/// This is what the loss check is made of: a key the host's event loop drops never reaches
/// the window, and nothing else in the channel can tell "delivered" from "sent".
fn count_key_presses(conn: &RustConnection, expected: usize, timeout: Duration) -> usize {
    let deadline = Instant::now() + timeout;
    let mut seen = 0;
    while seen < expected && Instant::now() < deadline {
        match conn.poll_for_event() {
            Ok(Some(Event::KeyPress(_))) => seen += 1,
            Ok(Some(_)) => {}
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(_) => break,
        }
    }
    seen
}

#[test]
#[ignore = "needs a live X server with XTEST; the lab job runs it with DISPLAY set"]
fn test_injector_focuses_a_window_and_the_key_arrives_without_losing_the_focus() {
    let injector = X11Injector::connect(None, 1.0).expect("a live X server with XTEST");
    let conn = injector.session().connection();
    let window = receiver_window(conn);
    let guard = focus_eventually(&injector, window);

    injector.key(&guard, KS_TAB, 0).expect("the key arrived");
    let arrived = count_key_presses(conn, 1, RECEIVE_TIMEOUT);
    let still_held = guard.verify(&injector);

    close_receiver(conn, window);
    assert_eq!(arrived, 1, "the injected key reached the focused window");
    assert!(
        still_held.is_ok(),
        "the focus stayed on the window: {still_held:?}"
    );
}

#[test]
#[ignore = "needs a live X server with XTEST; the lab job runs it with DISPLAY set"]
fn test_type_text_twenty_rounds_of_nihao_lose_no_key() {
    let injector = X11Injector::connect(None, 1.0).expect("a live X server with XTEST");
    let conn = injector.session().connection();
    let window = receiver_window(conn);
    let guard = focus_eventually(&injector, window);

    let per_round = strokes("nihao").expect("a pinyin string has keys").len();
    for round in 1..=ROUNDS {
        injector
            .type_text(&guard, "nihao", DEFAULT_KEY_DELAY)
            .unwrap_or_else(|error| panic!("round {round} was refused: {error}"));
    }
    let expected = per_round * ROUNDS;
    let received = count_key_presses(conn, expected, RECEIVE_TIMEOUT);
    let still_held = guard.verify(&injector);

    close_receiver(conn, window);
    assert_eq!(received, expected, "every key of every round arrived");
    assert!(
        still_held.is_ok(),
        "the focus stayed on the window: {still_held:?}"
    );
}

#[test]
#[ignore = "needs a live X server with XTEST; the lab job runs it with DISPLAY set"]
fn test_a_hundred_keys_never_move_the_focus_off_the_client() {
    let injector = X11Injector::connect(None, 1.0).expect("a live X server with XTEST");
    let conn = injector.session().connection();
    let window = receiver_window(conn);
    let guard = focus_eventually(&injector, window);

    // Every call checks the focus on both sides of the key it sends, so the criterion is
    // asserted twice per key rather than once at the end.
    for index in 1..=KEYS {
        injector
            .key(&guard, KS_TAB, 0)
            .unwrap_or_else(|error| panic!("key {index} moved the focus: {error}"));
    }
    let received = count_key_presses(conn, KEYS, RECEIVE_TIMEOUT);
    let focus = injector
        .input_focus()
        .expect("the server answers the focus query");
    let still_held = guard.verify(&injector);

    close_receiver(conn, window);
    assert_eq!(
        focus, window,
        "the server still reports the client's window"
    );
    assert!(
        still_held.is_ok(),
        "the focus stayed on the window: {still_held:?}"
    );
    assert_eq!(received, KEYS, "every key arrived");
}
