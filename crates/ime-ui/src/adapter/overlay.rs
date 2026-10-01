//! The overlay adapter: one `OverlayFrame`'s worth of the component's state.
//!
//! This is the whole of `OverlayFrame -> .slint`: it turns the engine's panel frame into
//! the models the overlay component draws from, the same shape of mapping `frame.rs` is
//! for the candidate panel. Pure, like every mapping here: it reads the frame and builds
//! owned values, touching no component, no clock and no display server, which is what
//! makes it testable without a window.
//!
//! The mapping draws what the cheat sheet needs -- the title and the grouped rows.
//! [`OverlayFrame`]'s `kind`, `selected` and `query` have no row to drive yet: the cheat
//! sheet is always unselected and search-less, and the panels that use those fields are
//! later phases, which will extend this mapping with the properties they need.

use ime_types::{OverlayEntry, OverlayFrame, OverlaySection};
use slint::{ModelRc, SharedString, VecModel};

use crate::ui_generated::{OverlayEntryData, OverlaySectionData};

/// Builds the section model the overlay panel draws from.
///
/// A fresh model per call rather than one kept and written in place, which is the
/// opposite of what the candidate grid does: the overlay channel is a mode, so the call
/// runs when the host opens or closes a panel and never per keystroke, and the write it
/// performs replaces what the panel draws whole.
///
/// # Parameters
///
/// * `frame` -- the engine's overlay frame.
///
/// # Returns
///
/// The model, in the frame's display order. An empty model is a valid answer and draws a
/// panel with nothing under its title.
///
/// # Panics
///
/// Never panics.
pub(crate) fn sections(frame: &OverlayFrame) -> ModelRc<OverlaySectionData> {
    ModelRc::new(VecModel::from(
        frame.sections.iter().map(section_data).collect::<Vec<_>>(),
    ))
}

/// One group, as the panel's model holds it.
fn section_data(section: &OverlaySection) -> OverlaySectionData {
    OverlaySectionData {
        title: SharedString::from(section.title.as_str()),
        entries: ModelRc::new(VecModel::from(
            section.entries.iter().map(entry_data).collect::<Vec<_>>(),
        )),
    }
}

/// One `keys -> label` row.
fn entry_data(entry: &OverlayEntry) -> OverlayEntryData {
    OverlayEntryData {
        keys: SharedString::from(entry.keys.as_str()),
        label: SharedString::from(entry.label.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use ime_types::{OverlayKind, OverlaySection as Section};
    use slint::Model as _;

    use super::*;

    /// One frame with two groups, one of them empty, as the engine can post them.
    fn frame() -> OverlayFrame {
        OverlayFrame {
            kind: OverlayKind::CheatSheet,
            title: String::from("按键速查"),
            sections: vec![
                Section {
                    title: String::from("组字"),
                    entries: vec![OverlayEntry {
                        keys: String::from("tab"),
                        label: String::from("移动候选高亮"),
                    }],
                },
                Section {
                    title: String::from("模式"),
                    entries: Vec::new(),
                },
            ],
            selected: None,
            query: String::new(),
        }
    }

    #[test]
    fn test_sections_model_carries_the_frame_rows_in_order() {
        let model = sections(&frame());
        assert_eq!(model.row_count(), 2, "one model row per frame section");
        let first = model.row_data(0).expect("the row exists");
        assert_eq!(first.title.to_string(), "组字");
        assert_eq!(first.entries.row_count(), 1);
        let entry = first.entries.row_data(0).expect("the entry exists");
        assert_eq!(entry.keys.to_string(), "tab");
        assert_eq!(entry.label.to_string(), "移动候选高亮");
    }

    #[test]
    fn test_sections_model_carries_an_empty_group_as_an_empty_row() {
        let model = sections(&frame());
        let empty = model.row_data(1).expect("the row exists");
        assert_eq!(
            empty.entries.row_count(),
            0,
            "a group the engine posted with no rows draws none"
        );
    }

    #[test]
    fn test_sections_model_of_a_frame_without_sections_is_empty() {
        let mut frame = frame();
        frame.sections.clear();
        assert_eq!(sections(&frame).row_count(), 0);
    }
}
