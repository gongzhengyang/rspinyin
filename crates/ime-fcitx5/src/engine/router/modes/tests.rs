//! Tests of the mode bits, the notice slot and the strip they paint.
//!
//! The notice selection is driven with injected facts, so every branch of the frozen table
//! is covered without touching the process-wide state the live reads answer from.

use super::*;

/// A host that answers every effect with nothing, for the mode keys that flip a bit.
struct SilentHost;

impl crate::engine::host::Host for SilentHost {
    fn commit(&mut self, _ic: u64, _text: &str) {}
    fn set_preedit(&mut self, _ic: u64, _text: &str, _caret: u32) {}
    fn clear_preedit(&mut self, _ic: u64) {}
    fn post_ui(&mut self, _ic: u64, _command: ime_types::UiCommand) {}
    fn toggle_enabled(&mut self, _ic: u64) -> bool {
        false
    }
    fn diagnose(&mut self, _ic: u64, _err: &ime_types::ImeError) {}
}

#[test]
fn test_notice_table_maps_the_frozen_codes_to_copy_and_priority() {
    // The table is a frozen mapping (code, Chinese copy, priority). The copy is
    // user-visible text and the priorities are contract decisions, so the whole table is
    // spelled out here: editing a sentence or reordering a priority has to be a
    // deliberate act that changes this assertion with it.
    let rows: [(&str, &str, u8); 4] = [
        ("dict/unavailable", "词典不可用，仅直通输入", 1),
        ("data/readonly-mode", "用户词库只读，学习已暂停", 2),
        ("ui/font/missing-cjk", "未找到中文字体，显示可能异常", 3),
        ("ui/theme/blur-unavailable", "桌面不支持模糊，已切换不透明", 4),
    ];
    assert_eq!(NOTICE_TABLE.len(), rows.len());
    for (notice, (code, text, priority)) in NOTICE_TABLE.iter().zip(rows) {
        assert_eq!(notice.code, code, "the frozen code of one row moved");
        assert_eq!(notice.text, text, "the frozen copy of one row moved");
        assert_eq!(notice.priority, priority, "a frozen priority moved");
    }
    // Equal priorities would make the selection order-dependent, which the frozen table
    // does not contain.
    for (index, notice) in NOTICE_TABLE.iter().enumerate() {
        assert!(
            NOTICE_TABLE[..index]
                .iter()
                .all(|earlier| earlier.priority != notice.priority),
            "priority {} appears twice; the tie case is not part of the table",
            notice.priority
        );
    }
}

#[test]
fn test_select_notice_without_facts_is_none() {
    let facts = NoticeFacts::default();
    assert_eq!(
        select_notice(facts),
        None,
        "a process with no degradation shows no notice"
    );
}

#[test]
fn test_select_notice_single_fact_shows_its_own_sentence() {
    // Each row of the table, driven alone: the sentence that reaches the label slot is
    // the one the fact's code maps to.
    let cases = [
        (
            NoticeFacts {
                dict_unavailable: true,
                ..NoticeFacts::default()
            },
            "词典不可用，仅直通输入",
        ),
        (
            NoticeFacts {
                readonly: true,
                ..NoticeFacts::default()
            },
            "用户词库只读，学习已暂停",
        ),
        (
            NoticeFacts {
                font_missing_cjk: true,
                ..NoticeFacts::default()
            },
            "未找到中文字体，显示可能异常",
        ),
        (
            NoticeFacts {
                blur_unavailable: true,
                ..NoticeFacts::default()
            },
            "桌面不支持模糊，已切换不透明",
        ),
    ];
    for (facts, expected) in cases {
        assert_eq!(select_notice(facts), Some(expected));
    }
}

#[test]
fn test_select_notice_stacked_facts_let_the_higher_priority_win() {
    // The card's stacking case: a read-only data directory on top of a missing
    // dictionary. The dictionary is the deeper degradation — without it nothing but
    // pass-through works at all — so its sentence wins and the readonly one waits.
    let stacked = NoticeFacts {
        dict_unavailable: true,
        readonly: true,
        ..NoticeFacts::default()
    };
    assert_eq!(
        select_notice(stacked),
        Some("词典不可用，仅直通输入"),
        "priority 1 outranks priority 2 when both hold"
    );
    // The whole table at once still answers with the single highest-priority sentence:
    // the slot holds one line, never a list.
    let everything = NoticeFacts {
        dict_unavailable: true,
        readonly: true,
        font_missing_cjk: true,
        blur_unavailable: true,
    };
    assert_eq!(select_notice(everything), Some("词典不可用，仅直通输入"));
}

#[test]
fn test_strip_label_notice_occupies_the_label_slot_and_yields_it_back() {
    // With a fact holding, the notice replaces the mode's own text in the slot —
    // Chinese mode and English mode alike, because the slot is one line wide.
    let readonly = NoticeFacts {
        readonly: true,
        ..NoticeFacts::default()
    };
    assert_eq!(strip_label(readonly, MODE_LABEL_CHINESE), "用户词库只读，学习已暂停");
    assert_eq!(strip_label(readonly, MODE_LABEL_ENGLISH), "用户词库只读，学习已暂停");
    // Boundary: with no fact the slot returns exactly what it was given, so a strip
    // with no degradation is byte-identical to the strip this module painted before
    // the notice slot existed.
    assert_eq!(strip_label(NoticeFacts::default(), MODE_LABEL_CHINESE), "中");
    assert_eq!(strip_label(NoticeFacts::default(), MODE_LABEL_ENGLISH), "英");
}

#[test]
fn test_status_carries_the_mode_bits_without_inference() {
    // The `chinese` bit is the mode bit itself, so the view never has to read anything
    // into the label's text. A fresh process carries no degradation, so the default
    // strip shows the mode label and no notice.
    let status = Modes::default().status(false, None);
    assert!(status.chinese, "the default mode is Chinese and the bit says so");
    assert_eq!(status.mode_label, "中");
    assert!(!status.full_width);
    assert!(status.punctuation_full);
    assert!(!status.readonly, "a clean process holds no readonly fact");

    // Temporary English changes the label, never the bit: the mode is still Chinese,
    // and the dot has to keep saying so while the label reads the English marker.
    let temp_english = Modes::default().status(true, None);
    assert!(temp_english.chinese);
    assert_eq!(temp_english.mode_label, "英");
}

#[test]
fn test_status_chinese_follows_the_toggled_mode_bit() {
    // The bit follows the engine's own state, which is what the host answers on a
    // language toggle — here `false`, the English answer.
    let mut modes = Modes::default();
    assert!(modes.apply(ime_types::KeyAction::ToggleLang, 1, &mut SilentHost));
    let status = modes.status(false, None);
    assert!(!status.chinese, "the bit is the toggled state, not a default");
    assert_eq!(status.mode_label, "英");
}

#[test]
fn test_status_layout_hint_still_names_the_label_when_no_notice_holds() {
    // The layout hint flows through the same label construction as before; a notice
    // only ever replaces it, so the no-fact path must be unchanged for the hint too.
    let status = Modes::default().status(false, Some("双拼"));
    assert_eq!(status.mode_label, "双拼");
    assert!(status.chinese);
}
