//! The symbol slot's tests: load shape, reload visibility and tear protection.

use ime_core::symbols::SymbolTable;

use super::SymbolSlot;

/// Builds a one-category table whose answer pins a label, so a test can tell the table it
/// holds from the one it replaced.
fn table_with(label: &str) -> SymbolTable {
    let document = format!("1\t{label}\t０１\n");
    let (table, report) = SymbolTable::load(&document);
    assert_eq!(report.loaded, 1, "the fixture row is usable");
    table
}

/// Reads the label through the slot's answer, the way a rebuild would.
fn installed_label(slot: &SymbolSlot) -> String {
    let live = slot.load().expect("a table is installed");
    let hit = live.longest_prefix("1").expect("the fixture code answers");
    String::from(hit.label)
}

#[test]
fn test_load_answers_the_installed_table_or_absent() {
    // `Some` installs a table the lookup answers through; `None` is the no-document
    // install whose v-mode stays off.
    let with = SymbolSlot::new(Some(table_with("数字")));
    assert_eq!(installed_label(&with), "数字");
    let without = SymbolSlot::new(None);
    assert!(without.load().is_none());
}

#[test]
fn test_store_makes_the_new_table_visible_to_the_next_reader() {
    // A reload re-reads and re-parses the document into a fresh table; the next reader
    // sees it, and a table installed later replaces the earlier one whole.
    let slot = SymbolSlot::empty();
    assert!(slot.load().is_none());
    slot.store(Some(table_with("数字")));
    assert_eq!(installed_label(&slot), "数字");
    slot.store(Some(table_with("数目")));
    assert_eq!(installed_label(&slot), "数目");
}

#[test]
fn test_store_keeps_a_held_arc_on_the_old_table() {
    // The tear the slot exists to rule out: a reader that fetched the old table before the
    // swap keeps reading the old table, and the new reader sees the new one, however the
    // two interleave.
    let slot = SymbolSlot::new(Some(table_with("数字")));
    let in_flight = slot.load().expect("a table is installed");
    let hit = in_flight.longest_prefix("1").expect("the code answers");
    assert_eq!(hit.label, "数字");

    slot.store(Some(table_with("数目")));

    // The frame built from the old table is untouched: the old `Arc` holds the bytes.
    let hit = in_flight.longest_prefix("1").expect("the code answers");
    assert_eq!(hit.label, "数字");
    // The next reader starts on the new table.
    assert_eq!(installed_label(&slot), "数目");
}

#[test]
fn test_store_restores_the_absent_state() {
    // A reload whose data directory no longer carries the document stores `None`, and the
    // panel is off again -- the degradation a fresh install without the file has.
    let slot = SymbolSlot::new(Some(table_with("数字")));
    assert!(slot.load().is_some());
    slot.store(None);
    assert!(slot.load().is_none());
}

#[test]
fn test_empty_is_a_const_static_initial_value() {
    // The slot a process installs as a static starts absent and takes a store without any
    // further construction, which is the shape the startup sequence wants.
    static SLOT: SymbolSlot = SymbolSlot::empty();
    assert!(SLOT.load().is_none());
    SLOT.store(Some(table_with("箭头")));
    assert_eq!(installed_label(&SLOT), "箭头");
}
