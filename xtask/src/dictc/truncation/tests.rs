//! Unit tests for the truncation audit: what it counts, how it ranks, and the
//! order it answers.

use super::*;

/// A one-key map holding `weights` under `key`, in arrival order.
fn map(key: &str, weights: &[u32]) -> WordMap {
    WordMap::from([(
        String::from(key),
        weights.iter().enumerate().map(|(id, weight)| (*weight, id as u32)).collect(),
    )])
}

#[test]
fn test_audit_reports_only_the_keys_that_hit_the_cap() {
    // One key at the cap's edge, one under it: the report is about the cuts, and a
    // key the ceiling never touched is not one of them.
    let mut entries = map("re'de", &[900, 800, 700, 600]);
    entries.insert(String::from("de"), vec![(500, 9)]);
    let rows = audit(&entries, 3);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].key, "re'de");
    assert_eq!(rows[0].wanted, 4);
    assert_eq!(rows[0].kept, 3);
}

#[test]
fn test_audit_measures_best_dropped_weight_in_the_writer_order() {
    // The rows arrive out of order, so the audit sorts them the way the writer
    // will -- strongest first, ties by the lower id -- and reads the cut off that
    // order: with a cap of two, the best word left behind is the 700, whatever
    // order the map happened to hold.
    let mut entries = WordMap::new();
    entries.insert(
        String::from("re'de"),
        vec![(700, 3), (900, 1), (700, 2), (800, 0)],
    );
    let rows = audit(&entries, 2);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].best_dropped_weight, 700, "{rows:?}");
    // The cap is a word count, so the audit's own arithmetic cannot be fooled by
    // one passed as a weight: the kept count is the cap, whatever the weights are.
    let rows = audit(&entries, 2);
    assert_eq!(rows[0].kept, 2);
}

#[test]
fn test_audit_ranks_rows_by_the_weight_of_what_they_lost() {
    // Two keys past the cap: the one whose cut cost the heavier word reports
    // first, because that is the row a cap decision reads, and the tie between
    // equal losses falls by key so the report never depends on map order.
    let mut entries = map("an'de", &[100, 90]);
    entries.insert(String::from("re'de"), vec![(900, 0), (800, 1), (850, 2)]);
    let rows = audit(&entries, 1);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].key, "re'de", "the heavier loss reports first");
    assert_eq!(rows[0].best_dropped_weight, 850);
    assert_eq!(rows[1].key, "an'de");
    assert_eq!(rows[1].best_dropped_weight, 90);
}

#[test]
fn test_audit_of_a_map_no_key_reaches_the_cap_is_empty() {
    let entries = map("de", &[900, 800, 700]);
    assert!(audit(&entries, 8).is_empty());
    // An empty map answers an empty report, which is what a healthy build with a
    // generous ceiling looks like.
    assert!(audit(&WordMap::new(), 1).is_empty());
}
