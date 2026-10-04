//! The cross-addon frame transport's post-path benchmark.
//!
//! Responsibility: measure what one `UiCommand::post` costs on the engine's main loop
//! thread — the wire assembly and the sink call — which is the number the `post_ui`
//! budget key is anchored against. The sink is a no-op: what this measures is the
//! producing side, not the UI thread's parse, which the UI addon's own budgets own.

use std::ffi::c_void;

use criterion::{Criterion, criterion_group, criterion_main};

use ime_types::ui::Script;
use ime_types::{
    Candidate, LayoutHint, PageState, Preedit, PreeditSpan, StatusStrip, UiCommand, UiFrame,
};
use rspinyin::ffi::abi::engine::transport;

/// The no-op sink: the benchmark's stand-in for the UI side.
extern "C" fn noop_frame(_ctx: *mut c_void, _wire: *const transport::RspinyinFrameWire) {}
extern "C" fn noop_overlay(_ctx: *mut c_void, _wire: *const transport::RspinyinOverlayWire) {}

static SINK: transport::RspinyinUiSink = transport::RspinyinUiSink {
    ctx: std::ptr::null_mut(),
    frame: noop_frame,
    overlay: noop_overlay,
};

fn sample_frame() -> UiFrame {
    UiFrame {
        revision: 42,
        preedit: Preedit {
            text: "ni'hao'.shi'jie".into(),
            caret: 6,
            spans: vec![
                PreeditSpan {
                    start: 0,
                    end: 2,
                    kind: ime_types::SpanKind::Syllable,
                },
                PreeditSpan {
                    start: 3,
                    end: 5,
                    kind: ime_types::SpanKind::Separator,
                },
                PreeditSpan {
                    start: 6,
                    end: 16,
                    kind: ime_types::SpanKind::Passthrough,
                },
            ],
        },
        candidates: (0..5)
            .map(|index| Candidate {
                index,
                text: format!("候选{index}"),
                annotation: Some("annotation".into()),
                source: ime_types::CandidateSource::Dict,
                score: 0.5,
                consumed_syllables: 2,
            })
            .collect(),
        page: PageState {
            current: 1,
            total: 3,
            page_size: 5,
        },
        status: StatusStrip {
            mode_label: "拼音".into(),
            full_width: false,
            punctuation_full: false,
            has_user_dict_hit: false,
            readonly: false,
            chinese: true,
            script: Script::Simplified,
        },
        anchor: ime_types::Anchor {
            cursor: ime_types::RectI {
                x: 100,
                y: 200,
                w: 30,
                h: 40,
            },
            screen: ime_types::ScreenId::new(0),
            scale: 1.0,
            placement: ime_types::Placement::Below,
        },
        layout: LayoutHint {
            max_per_row: 5,
            show_annotation: true,
            max_width_dp: 156,
        },
        highlight: Some(0),
    }
}

fn bench_post_frame(c: &mut Criterion) {
    assert!(transport::register_for_bench(&SINK));
    let command = UiCommand::Frame(Box::new(sample_frame()));
    let mut group = c.benchmark_group("transport");
    group.bench_function("post_frame", |b| b.iter(|| dispatch_once(&command)));
    group.finish();
}

/// One dispatch, wrapped so the bench body stays panic-free by construction.
fn dispatch_once(command: &UiCommand) -> bool {
    transport::dispatch(command)
}

criterion_group!(benches, bench_post_frame);
criterion_main!(benches);
