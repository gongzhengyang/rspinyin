//! Header tests: the preedit's runs, the status cluster's flags and the caret arrow.
//!
//! A submodule of `adapter::tests` rather than more of it, because the three groups below
//! are the header's own and the file beside this one is already the whole candidate grid's.
//! The fixtures they drive are the parent module's, so a frame here is the same frame the
//! grid tests draw.

use crate::renderer::mock::{MockState, MockSurface, on_own_thread};
use crate::slint_platform::RspinyinPlatform;

use super::{
    Adapter, BYTES_PER_PIXEL, PIXEL_HEIGHT_DP, PIXEL_WIDTH_DP, frame_with, metrics, run_kinds,
    run_texts, with_adapter,
};

#[test]
fn test_adapter_splits_the_preedit_around_a_caret_in_the_middle() {
    let (before, after, kinds, caret) = with_adapter(|adapter| {
        let mut frame = frame_with(1, "ni'hao", 1);
        frame.preedit.caret = 3;
        assert!(adapter.apply_frame(&frame));
        let window = adapter.window();
        (
            run_texts(&window.get_preedit_before()),
            run_texts(&window.get_preedit_after()),
            run_kinds(&window.get_preedit_before()),
            window.get_caret_visible(),
        )
    });
    assert_eq!(before, ["ni", "'"], "the runs up to the caret are drawn first");
    assert_eq!(after, ["hao"], "and the rest behind it");
    assert_eq!(
        kinds,
        [0, 1],
        "the run kinds travel as the wire values of `SpanKind`"
    );
    assert!(caret, "the caret is drawn where the span list put it");
}

#[test]
fn test_adapter_writes_the_status_cluster_flags_from_a_frame() {
    let (full_width, punctuation, readonly) = with_adapter(|adapter| {
        let mut frame = frame_with(1, "ni", 1);
        frame.status.full_width = true;
        frame.status.punctuation_full = true;
        frame.status.readonly = true;
        assert!(adapter.apply_frame(&frame));
        let window = adapter.window();
        (
            window.get_full_width(),
            window.get_punctuation_full(),
            window.get_readonly(),
        )
    });
    assert!(full_width, "the full-width marker follows the frame");
    assert!(punctuation, "so does the Chinese-punctuation marker");
    assert!(readonly, "and the read-only lock of 3.6");
}

#[test]
fn test_adapter_drops_the_secondary_status_markers_when_the_preedit_has_no_room() {
    // A panel is never narrower than 3.1.1's 220dp floor, so the only way the preedit runs
    // out of room is a status label that takes it: 3.6's dictionary notice is long enough to
    // push the preedit under the 48dp the header keeps for it.
    let (secondary, truncated, drawn) = with_adapter(|adapter| {
        let mut frame = frame_with(1, "ni'hao'a", 0);
        frame.status.mode_label = String::from("词库不可用词库不可用");
        assert!(adapter.apply_frame(&frame));
        let window = adapter.window();
        (
            window.get_show_secondary_status(),
            window.get_preedit_truncated(),
            run_texts(&window.get_preedit_before()),
        )
    });
    assert!(
        !secondary,
        "a preedit under the floor keeps the two markers the user has to know"
    );
    assert!(truncated, "and the reading no longer fits beside the notice");
    assert!(
        !drawn.is_empty(),
        "what does fit is still drawn, tail first (3.1.3)"
    );
}

#[test]
fn test_adapter_writes_the_caret_arrow_and_reports_a_repeat_as_no_change() {
    let (first, again, moved, visible, position) = with_adapter(|adapter| {
        let first = adapter.set_arrow(Some((18.0, -6.0)));
        let again = adapter.set_arrow(Some((18.0, -6.0)));
        let moved = adapter.set_arrow(Some((30.0, -6.0)));
        let window = adapter.window();
        (
            first,
            again,
            moved,
            window.get_arrow_visible(),
            (window.get_arrow_x(), window.get_arrow_y()),
        )
    });
    assert!(first, "a placement that produced an arrow draws one");
    assert!(!again, "the same placement again writes nothing");
    assert!(moved, "an arrow that moved is redrawn");
    assert!(visible);
    assert_eq!(position, (30.0, -6.0));
}

#[test]
fn test_adapter_hides_the_caret_arrow_when_the_placement_declined_one() {
    let (shown, hidden, visible, position) = with_adapter(|adapter| {
        let shown = adapter.set_arrow(Some((18.0, -6.0)));
        let hidden = adapter.set_arrow(None);
        let window = adapter.window();
        (
            shown,
            hidden,
            window.get_arrow_visible(),
            (window.get_arrow_x(), window.get_arrow_y()),
        )
    });
    assert!(shown);
    assert!(hidden, "a flipped or clamped placement takes the arrow away");
    assert!(!visible, "the shape is gated, not faded");
    assert_eq!(
        position,
        (18.0, -6.0),
        "and the coordinates it leaves behind are not read"
    );
}

#[test]
fn test_adapter_dropping_the_secondary_markers_frees_their_room() {
    // The budget arithmetic assumes the cluster is 48dp narrower once the two secondary
    // markers are dropped, and that assumption is a property of the *layout*: Slint has to
    // take an invisible element out of the flow for the room to come back to the preedit.
    // The mode dot is the cluster's leftmost marker, so where its left edge lands is what
    // says whether the room was freed -- and it is found by its own colour, which no other
    // pixel of the header carries.
    let (shown, dropped) = on_own_thread(|| {
        let (backend, state) = MockSurface::new(PIXEL_WIDTH_DP, PIXEL_HEIGHT_DP, 1.0);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        let mut adapter = Adapter::new().expect("the component binds to the platform");
        adapter
            .set_visible(true)
            .expect("the surface can be mapped");

        let mut settle = || {
            for _ in 0..4 {
                platform
                    .render_if_dirty()
                    .expect("a settling frame is drawn");
            }
            let state = state.lock().expect("the mock is not poisoned");
            dot_left(&state)
        };

        // A short status label leaves the preedit its full budget, so the cluster keeps both
        // secondary markers. The preedit itself is empty, which keeps its caret -- the
        // header's other accent-coloured run -- out of the scan below, and the label is not,
        // because the mode dot is the accent colour only while the strip carries a mode.
        let mut wide = frame_with(1, "", 1);
        wide.status.mode_label = String::from("拼音输入");
        assert!(adapter.apply_frame(&wide));
        let shown = settle();

        // 3.6's dictionary notice takes the preedit's room away, and the cluster drops them.
        let mut narrow = frame_with(2, "", 1);
        narrow.status.mode_label = String::from("词库不可用词库不可用");
        assert!(adapter.apply_frame(&narrow));
        let dropped = settle();
        (shown, dropped)
    });

    let (shown, dropped) = (
        shown.expect("the mode dot is drawn with the secondary markers"),
        dropped.expect("and without them"),
    );
    assert!(
        dropped > shown,
        "the cluster has to start further right once the two markers are dropped: {dropped} \
         against {shown}"
    );
    assert_eq!(
        dropped - shown,
        48,
        "and by exactly the width the preedit was given back"
    );
}

/// The x of the header's leftmost accent-coloured pixel: the mode dot's left edge.
///
/// The dot is the one run of the accent colour in the header, and the test tells it apart
/// from the preedit's text by its channels -- the accent is a saturated blue, the text is
/// near-white.
fn dot_left(state: &MockState) -> Option<usize> {
    let stride = PIXEL_WIDTH_DP as usize * BYTES_PER_PIXEL;
    let constants = metrics();
    let top = constants.shadow_margin as usize;
    for x in 0..PIXEL_WIDTH_DP as usize {
        for y in top..top + constants.header_height as usize {
            let pixel = state.pixel(stride, x, y);
            if pixel[0] > 200 && pixel[2] < 150 {
                return Some(x);
            }
        }
    }
    None
}

#[test]
fn test_adapter_draws_the_caret_arrow_into_the_surface() {
    // The one thing a property assertion cannot see: whether the shape reaches the
    // rasterizer at all. The scene is rendered twice, with the arrow and without it, and the
    // two frames are compared at a pixel inside the triangle -- a property that is written
    // and never drawn is exactly the defect this compares for.
    let (with_arrow, without) = on_own_thread(|| {
        let (backend, state) = MockSurface::new(PIXEL_WIDTH_DP, PIXEL_HEIGHT_DP, 1.0);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        let mut adapter = Adapter::new().expect("the component binds to the platform");
        adapter
            .set_visible(true)
            .expect("the surface can be mapped");
        assert!(adapter.apply_frame(&frame_with(1, "ni'hao", 4)));
        assert!(adapter.set_arrow(Some((18.0, -6.0))));

        let stride = PIXEL_WIDTH_DP as usize * BYTES_PER_PIXEL;
        let constants = metrics();
        // A point inside the triangle: the reserve, the arrow's own offset, and five of its
        // six dp down, which is inside the shape on every row but the tip.
        let x = constants.shadow_margin as usize + 18 + 6;
        let y = constants.shadow_margin as usize - 6 + 5;
        let mut settle = || {
            for _ in 0..4 {
                platform
                    .render_if_dirty()
                    .expect("a settling frame is drawn");
            }
            state
                .lock()
                .expect("the mock is not poisoned")
                .pixel(stride, x, y)
        };

        let with_arrow = settle();
        assert!(adapter.set_arrow(None));
        let without = settle();
        (with_arrow, without)
    });
    assert_ne!(
        with_arrow, without,
        "the arrow has to change the frame it is drawn into"
    );
    assert!(
        with_arrow[3] > without[3],
        "and it paints the panel's own colour over the transparent reserve: {with_arrow:?} \
         against {without:?}"
    );
}
