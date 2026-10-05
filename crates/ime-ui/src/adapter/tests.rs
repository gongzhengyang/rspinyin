//! Unit tests for the frame adapter, and the frame fixtures its neighbours share.
//!
//! Every scene here runs on a thread of its own: Slint installs its platform per thread and
//! refuses a second one on the same thread, so a test that needs a component gets a fresh
//! thread. That is also why the assertions are collected inside the scene and returned as
//! plain values -- the component is reference counted and cannot cross the thread boundary.

use std::sync::{Arc, Mutex};

use ime_types::ui::{OverlayEntry, OverlayFrame, OverlayKind, OverlaySection};
use ime_types::{
    Anchor, Candidate, CandidateSource, ColorScheme, LayoutHint, PageState, Placement, Preedit,
    PreeditSpan, RectI, Rgba8, ScreenId, SpanKind, StatusStrip, ThemeSpec, UiFrame,
};
use slint::{ComponentHandle as _, Model as _};

use super::{Adapter, COMPONENT_FAILED_CODE, PointerState};
use crate::layout;
use crate::renderer::mock::{MockState, MockSurface, on_own_thread};
use crate::slint_platform::RspinyinPlatform;
use crate::theme::{BlurNegotiation, ThemeResolution};
use crate::ui_generated::{PreeditRun, Theme};

mod crossfade;
mod header;
mod motion;
mod pixels;
mod press;

use motion::FRAME_S;

/// The caret position every fixture anchors to.
pub(crate) fn anchor() -> Anchor {
    Anchor {
        cursor: RectI {
            x: 100,
            y: 200,
            w: 2,
            h: 20,
        },
        screen: ScreenId::new(0),
        scale: 1.0,
        placement: Placement::Below,
    }
}

/// The preedit of a reading typed to the end, as the preedit builder produces it.
///
/// The text is the canonical spelling -- syllables joined by the separator the user did not
/// have to type -- and the span list tiles it exactly with one zero-width cursor span at the
/// caret, which is what `ime_types::Preedit` guarantees and what the header is written
/// against.
pub(crate) fn preedit_fixture(reading: &str) -> Preedit {
    let mut text = String::new();
    let mut spans = Vec::new();
    for (index, part) in reading.split('\'').enumerate() {
        if index > 0 {
            let start = text.len();
            text.push('\'');
            spans.push(PreeditSpan {
                start: start as u16,
                end: text.len() as u16,
                kind: SpanKind::Separator,
            });
        }
        let start = text.len();
        text.push_str(part);
        spans.push(PreeditSpan {
            start: start as u16,
            end: text.len() as u16,
            kind: SpanKind::Syllable,
        });
    }
    let caret = text.len() as u16;
    spans.push(PreeditSpan {
        start: caret,
        end: caret,
        kind: SpanKind::Cursor,
    });
    Preedit {
        text,
        caret: u32::from(caret),
        spans,
    }
}

/// A frame with `count` candidates, each drawing two glyphs, and a preedit.
///
/// The candidate text is deliberately two characters wide: it pins the cell width the layout
/// derives, which is what the mapping tests assert on.
pub(crate) fn frame_with(revision: u32, preedit: &str, count: usize) -> UiFrame {
    let candidates = (0..count)
        .map(|n| Candidate {
            index: (n + 1) as u16,
            text: String::from("你好"),
            annotation: None,
            source: CandidateSource::Dict,
            score: 0.0,
            consumed_syllables: 1,
        })
        .collect();
    UiFrame {
        revision,
        preedit: preedit_fixture(preedit),
        candidates,
        page: PageState {
            current: 1,
            total: 1,
            page_size: 9,
        },
        status: StatusStrip::default(),
        anchor: anchor(),
        layout: LayoutHint {
            max_per_row: 5,
            show_annotation: true,
            max_width_dp: 720,
        },
        // A fresh decode highlights the first candidate; an empty page has none.
        highlight: (count > 0).then_some(0),
    }
}

/// The text of every run of a preedit model, in order.
fn run_texts(model: &slint::ModelRc<PreeditRun>) -> Vec<String> {
    (0..model.row_count())
        .filter_map(|index| model.row_data(index))
        .map(|run| run.text.to_string())
        .collect()
}

/// The kind of every run of a preedit model, in order.
fn run_kinds(model: &slint::ModelRc<PreeditRun>) -> Vec<i32> {
    (0..model.row_count())
        .filter_map(|index| model.row_data(index))
        .map(|run| run.kind)
        .collect()
}

/// Builds an adapter on a fresh thread and runs `scene` against it.
fn with_adapter<R: Send + 'static>(scene: impl FnOnce(&mut Adapter) -> R + Send + 'static) -> R {
    on_own_thread(move || {
        let (backend, _state) = MockSurface::new(320, 160, 1.0);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        let mut adapter = Adapter::new().expect("the component binds to the platform");
        scene(&mut adapter)
    })
}

/// Builds an adapter over a mock surface the test keeps a handle on, on a fresh thread.
///
/// The plain [`with_adapter`] throws the observation state away; the staged-unmap scenes
/// watch the mock's mapped flag across the fade, so they need it back.
fn with_mapped_adapter<R: Send + 'static>(
    scene: impl FnOnce(&mut Adapter, Arc<Mutex<MockState>>) -> R + Send + 'static,
) -> R {
    on_own_thread(move || {
        let (backend, state) = MockSurface::new(160, 64, 1.0);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        let mut adapter = Adapter::new().expect("the component binds to the platform");
        scene(&mut adapter, state)
    })
}

#[test]
fn test_adapter_writes_the_panel_properties_from_a_frame() {
    let (count, rows, width, height, header, per_row, before, after) = with_adapter(|adapter| {
        let frame = frame_with(1, "ni'hao", 9);
        assert!(adapter.apply_frame(&frame), "the first frame is drawn");
        let window = adapter.window();
        (
            window.get_item_count(),
            window.get_grid_rows(),
            window.get_container_width(),
            window.get_container_height(),
            window.get_header_height(),
            window.get_max_per_row(),
            run_texts(&window.get_preedit_before()),
            run_texts(&window.get_preedit_after()),
        )
    });
    assert_eq!(count, 9);
    assert_eq!(rows, 2, "nine candidates at five per row");
    assert_eq!(width, 370.0);
    assert_eq!(height, 129.0);
    assert_eq!(
        header, 34.0,
        "the full strip is drawn when there is a candidate"
    );
    assert_eq!(per_row, 5);
    assert_eq!(
        before,
        ["ni", "'", "hao"],
        "the whole reading is drawn before a caret at its end"
    );
    assert!(after.is_empty());
}

#[test]
fn test_adapter_draws_the_compressed_header_when_there_is_no_candidate() {
    let (rows, count, header, height) = with_adapter(|adapter| {
        let frame = frame_with(1, "ni", 0);
        assert!(adapter.apply_frame(&frame));
        let window = adapter.window();
        (
            window.get_grid_rows(),
            window.get_item_count(),
            window.get_header_height(),
            window.get_container_height(),
        )
    });
    assert_eq!(count, 0);
    assert_eq!(rows, 0);
    assert_eq!(header, 28.0, "the compact strip of a preedit-only window");
    assert_eq!(height, 45.0, "the panel the layout sized for it");
}

#[test]
fn test_adapter_drops_a_frame_older_than_the_one_it_draws() {
    let (stale, drawn) = with_adapter(|adapter| {
        assert!(adapter.apply_frame(&frame_with(5, "ni", 9)));
        let stale = adapter.apply_frame(&frame_with(3, "ni", 1));
        (stale, adapter.window().get_item_count())
    });
    assert!(!stale, "a frame older than the drawn one is dropped");
    assert_eq!(drawn, 9, "the window keeps drawing the newest frame");
}

#[test]
fn test_adapter_theme_reaches_the_component_global() {
    let (dark, approved, written, radius, width) = with_adapter(|adapter| {
        let spec = ThemeSpec {
            scheme: ColorScheme::Light,
            accent: Rgba8 {
                r: 0x0A,
                g: 0x6C,
                b: 0xFF,
                a: 255,
            },
            acrylic: false,
            base_alpha: 217,
            corner_radius_dp: 14,
            scale: 1.0,
        };
        let resolution = ThemeResolution::resolve(&spec, BlurNegotiation::Disabled);
        adapter.apply_theme(&spec, &resolution.tokens);
        let theme = adapter.window().global::<Theme>();
        (
            theme.get_dark(),
            f32::from(resolution.tokens.base_alpha) / 255.0,
            theme.get_base_alpha(),
            adapter.window().get_container_radius(),
            adapter.window().get_container_width(),
        )
    });
    assert!(
        !dark,
        "the light scheme reaches the component's theme global"
    );
    assert!(
        approved > 0.0 && approved <= 1.0,
        "the resolution approved an alpha the component can paint with, got {approved}"
    );
    assert!(
        (written - approved).abs() < 1.0e-6,
        "the alpha the contrast gate approved is the one written, got {written}"
    );
    assert_eq!(radius, 14.0, "the configured corner radius is drawn");
    assert_eq!(
        width, 220.0,
        "a theme switch writes no geometry: the panel keeps the width it was given"
    );
}

#[test]
fn test_adapter_second_component_on_the_same_thread_is_refused() {
    let (first_ok, second_error) = on_own_thread(|| {
        let (backend, _state) = MockSurface::new(160, 64, 1.0);
        let platform = RspinyinPlatform::new(Box::new(backend));
        platform
            .install()
            .expect("a fresh thread has no Slint platform yet");
        // The first component has to stay alive across the second call: the platform keeps
        // only a weak reference to the window it handed out, so a dropped component looks
        // like no window at all and a second one would be served.
        let first = Adapter::new();
        let second = Adapter::new();
        (first.is_ok(), second.err().map(|error| error.to_string()))
    });
    assert!(first_ok, "the first component binds to the platform");
    assert_eq!(
        second_error.as_deref(),
        Some("platform/compositor/unsupported: ui/slint/component"),
        "one surface, one window: the second component is refused with the stable code"
    );
    assert_eq!(COMPONENT_FAILED_CODE, "ui/slint/component");
}

/// The size of the surface the pixel scenes draw into, in logical pixels at a scale of 1.0.
///
/// The panel is drawn at the shadow margin and sized by the frame, so the surface only has

#[test]
fn test_adapter_keeps_the_full_text_of_a_truncated_candidate() {
    let text: String = "你好世界".chars().cycle().take(32).collect();
    let expected = text.clone();
    let (drawn, full, index) = with_adapter(move |adapter| {
        let mut frame = frame_with(22, "ni", 1);
        frame.candidates[0].text = text;
        assert!(adapter.apply_frame(&frame));
        let items = adapter.window().get_items();
        let cell = items.row_data(0).expect("the page holds one candidate");
        (
            cell.display_text.to_string(),
            cell.text.to_string(),
            cell.index,
        )
    });
    assert!(drawn.ends_with('…'), "the cell shows a cut text: {drawn:?}");
    assert_ne!(drawn, full);
    assert_eq!(
        full, expected,
        "the text a selection commits travels beside the text that is drawn"
    );
    assert_eq!(
        index, 1,
        "and the selection names the candidate by index, never by its text"
    );
}
/// One cheat-sheet frame, as the engine builds it from the bindings in force.
fn overlay_frame() -> OverlayFrame {
    OverlayFrame {
        kind: OverlayKind::CheatSheet,
        title: String::from("按键速查"),
        sections: vec![OverlaySection {
            title: String::from("组字"),
            entries: vec![OverlayEntry {
                keys: String::from("tab"),
                label: String::from("移动候选高亮"),
            }],
        }],
        selected: None,
        query: String::new(),
    }
}

#[test]
fn test_adapter_frame_highlight_drives_the_ring() {
    // The highlight is the frame's word: a frame that names another cell moves the ring
    // to it, `None` hides it although candidates remain, and a position with no cell --
    // a drifted wire, never a well-formed frame -- draws nothing either.
    let (moved, hidden, unmarked, past_end) = with_adapter(|adapter| {
        // A parameter, not a capture: the closure runs on both sides of the mutable calls.
        let marked = |adapter: &Adapter, position: usize| {
            adapter
                .window()
                .get_items()
                .row_data(position)
                .map(|cell| cell.is_highlighted)
        };
        let mut moved = frame_with(2, "ni'hao", 3);
        moved.highlight = Some(2);
        assert!(adapter.apply_frame(&moved));
        let moved = (marked(adapter, 0), marked(adapter, 2));
        let mut emptied = frame_with(3, "ni'hao", 3);
        emptied.highlight = None;
        assert!(adapter.apply_frame(&emptied));
        adapter.advance(FRAME_S).expect("the motion advances");
        let hidden = adapter.window().get_highlight_visible();
        let mut drifted = frame_with(3, "ni'hao", 3);
        drifted.highlight = Some(9);
        assert!(adapter.apply_frame(&drifted));
        // Every cell the page holds answers with an explicit "not highlighted": the
        // model carries a state per row, so the drift shows up as no row carrying the
        // focus state rather than as a missing row.
        let unmarked = (0..3).all(|position| marked(adapter, position) == Some(false));
        (
            moved,
            hidden,
            unmarked,
            adapter.window().get_highlight_visible(),
        )
    });
    assert_eq!(
        moved,
        (Some(false), Some(true)),
        "naming another cell moves the ring to it"
    );
    assert!(!hidden, "`None` hides the ring although candidates remain");
    assert!(
        unmarked,
        "a drifted frame leaves no cell carrying the focus state"
    );
    assert!(!past_end, "an out-of-page highlight draws nothing");
}

#[test]
fn test_adapter_apply_overlay_writes_the_overlay_properties() {
    // The properties are what the binding actually produced, so reading them back covers
    // the write path -- the component-side half of "the overlay draws" -- and not only
    // the mapping that feeds it.
    let (visible, title, sections, entries, hidden_after) = with_adapter(|adapter| {
        let frame = overlay_frame();
        adapter.apply_overlay(Some(&frame));
        let window = adapter.window();
        let open = (
            window.get_overlay_visible(),
            window.get_overlay_title().to_string(),
            window.get_overlay_sections().row_count(),
            window
                .get_overlay_sections()
                .row_data(0)
                .map(|section| section.entries.row_count()),
        );
        adapter.apply_overlay(None);
        let hidden_after = adapter.window().get_overlay_visible();
        (open.0, open.1, open.2, open.3, hidden_after)
    });
    assert!(visible, "the overlay mode is on while a frame is drawn");
    assert_eq!(title, "按键速查", "the title arrives from the frame");
    assert_eq!(sections, 1, "one model row per frame section");
    assert_eq!(entries, Some(1), "the group's rows travel with it");
    assert!(
        !hidden_after,
        "closing the overlay uncovers the candidate panel the adapter still holds"
    );
}

#[test]
fn test_draw_state_chinese_is_the_strip_bit_not_a_label_inference() {
    use super::{DrawState, container_cap};

    // `StatusStrip::chinese` (ADR-0005) is the mode dot's source, copied into the draw
    // state untouched. The label slot beside the dot may carry a degradation notice, so
    // the mapping must not derive the bit from the label's text or its emptiness: a
    // notice in an English frame keeps the dot dark, and a Chinese frame keeps it lit
    // whatever the label says.
    let metrics = layout::metrics().expect("the component declares the metrics block");
    let mut state = DrawState::default();

    let mut notice = frame_with(1, "ni'hao", 1);
    notice.status = StatusStrip {
        mode_label: String::from("词典不可用，仅直通输入"),
        chinese: false,
        ..StatusStrip::default()
    };
    state.update(&notice, container_cap(&notice, metrics), metrics);
    assert!(
        !state.chinese,
        "a notice label in an English frame must not light the mode dot"
    );

    let mut chinese = frame_with(2, "ni'hao", 1);
    chinese.status = StatusStrip {
        mode_label: String::from("词典不可用，仅直通输入"),
        chinese: true,
        ..StatusStrip::default()
    };
    state.update(&chinese, container_cap(&chinese, metrics), metrics);
    assert!(
        state.chinese,
        "the engine's mode bit is drawn as written, notice or not"
    );
}

#[test]
fn test_draw_state_status_delta_flags_a_chinese_bit_change() {
    use super::{DrawState, container_cap};

    // Boundary of the change detection: the bit is part of the `status` group, so a
    // frame that flips nothing else still reaches the component. A frame that changes
    // no flag at all stays an empty delta, which is what keeps a repeated keystroke
    // free of property writes.
    let metrics = layout::metrics().expect("the component declares the metrics block");
    let mut state = DrawState::default();

    let mut first = frame_with(1, "ni'hao", 1);
    first.status = StatusStrip {
        chinese: true,
        ..StatusStrip::default()
    };
    state.update(&first, container_cap(&first, metrics), metrics);

    let mut english = frame_with(2, "ni'hao", 1);
    english.status = StatusStrip {
        chinese: false,
        ..StatusStrip::default()
    };
    let flipped = state.update(&english, container_cap(&english, metrics), metrics);
    assert!(flipped.status, "a mode-bit flip is a visible change");
    assert!(!state.chinese);

    let replay = state.update(&english, container_cap(&english, metrics), metrics);
    assert!(
        replay.is_empty(),
        "a frame that changes nothing writes nothing"
    );
}
