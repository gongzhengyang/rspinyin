//! Tests for the value model: the trim ordering, the floor accounting, the
//! writer-layout projection and the percentile table. Pure functions over
//! synthetic rows, so every case runs in microseconds and nothing here depends
//! on a real dictionary.

use super::*;

/// A candidate row from text alone; every field the ranking reads derives.
fn value(text: &str, key: &str, weight: u32, in_band: bool, neighbors: u32) -> TrimValue {
    TrimValue {
        text: text.to_owned(),
        key: key.to_owned(),
        weight,
        characters: text.chars().count(),
        in_expansion_band: in_band,
        neighbors,
    }
}

#[test]
fn test_trim_rank_orders_low_weight_before_high_weight() {
    let rare = value("稀有词", "xi'you'ci", 5, false, 0);
    let common = value("不像话", "bu'xiang'hua", 30, true, 0);
    assert_eq!(trim_rank(&rare, &common), Ordering::Less);
    assert_eq!(trim_rank(&common, &rare), Ordering::Greater);
}

#[test]
fn test_trim_rank_prefers_out_of_band_over_in_band_at_equal_weight() {
    let backed = value("不像话", "bu'xiang'hua", 30, true, 0);
    let lone = value("稀有词", "xi'you'ci", 30, false, 0);
    assert_eq!(trim_rank(&lone, &backed), Ordering::Less);
}

#[test]
fn test_trim_rank_prefers_crowded_keys_over_quiet_ones() {
    let crowded = value("不像话", "bu'xiang'hua", 30, false, 9);
    let quiet = value("稀有词", "xi'you'ci", 30, false, 0);
    assert_eq!(trim_rank(&crowded, &quiet), Ordering::Less);
}

#[test]
fn test_trim_rank_prefers_short_words_at_every_level_tied() {
    let short = value("东西", "dong'xi", 30, false, 0);
    let long = value("不像话", "bu'xiang'hua", 30, false, 0);
    assert_eq!(trim_rank(&short, &long), Ordering::Less);
}

#[test]
fn test_trim_rank_falls_to_key_then_text_and_ignores_row_order() {
    // Equal weights, band membership, crowding and length: the keys decide,
    // and `bei'ci` sorts before `jia'ci`.
    let left = value("乙词", "bei'ci", 30, false, 0);
    let right = value("甲词", "jia'ci", 30, false, 0);
    assert_eq!(trim_rank(&left, &right), Ordering::Less);
    assert_eq!(trim_rank(&right, &left), Ordering::Greater);
    let twin_one = value("甲词", "jia'ci", 30, false, 0);
    let twin_two = value("甲词", "jia'ci", 30, false, 0);
    assert_eq!(trim_rank(&twin_one, &twin_two), Ordering::Equal);
}

#[test]
fn test_floor_savings_covers_the_linear_rows_of_one_word() {
    // `ENTRIES` row + `UNIGRAM` row + `STRPOOL` text, and nothing unproven.
    assert_eq!(floor_savings_bytes(0), 24);
    assert_eq!(floor_savings_bytes(6), 30, "a two-character word's text");
    assert_eq!(floor_savings_bytes(9), 33, "a three-character word's text");
}

#[test]
fn test_target_bytes_rejects_nonpositive_and_nonfinite_targets() {
    assert!(target_bytes(0.0).is_err());
    assert!(target_bytes(-1.0).is_err());
    assert!(target_bytes(f64::NAN).is_err());
    assert!(target_bytes(f64::INFINITY).is_err());
    assert_eq!(target_bytes(1.0).expect("one mebibyte"), 1_048_576);
}

#[test]
fn test_projected_container_bytes_replicates_the_writer_layout() {
    // A `Compiled` with hand-picked section lengths, two of them unaligned and
    // one empty, against the arithmetic the writer performs: 208 bytes of
    // header and table, then every present section on its next 8-byte boundary.
    let compiled = Compiled {
        entries: vec![0; 10],
        strpool: vec![0; 7],
        unigram: vec![0; 8],
        wordlist: vec![0; 4],
        fst: Vec::new(),
        bigram: Vec::new(),
        stats: super::super::build::Stats::default(),
    };
    // Sections land on their next 8-byte boundary after the 208-byte head:
    // entries at 208 (10 bytes, next cursor 224), strpool at 224 (7 bytes,
    // next 232), unigram at 232 (8 bytes, next 240), wordlist at 240 (4 bytes,
    // end 244). The empty fst and bigram take no room.
    assert_eq!(projected_container_bytes(&compiled), 244);
}

#[test]
fn test_plan_trim_drops_stop_at_the_target_and_report_the_tail() {
    let candidates = vec![
        (0usize, value("稀有词", "xi'you'ci", 5, false, 0)),
        (1, value("不像话", "bu'xiang'hua", 10, false, 0)),
        (2, value("东西", "dong'xi", 19, false, 0)),
    ];
    // The walk stops as soon as the accumulated floor covers the ask, so the
    // last drop counts whole even when it overshoots: two rows free 66 bytes
    // against an ask of 63.
    let plan = plan_trim_drops(candidates.clone(), 63);
    assert_eq!(plan.drop_indices, vec![0, 1]);
    assert_eq!(plan.freed_bytes, 66);
    assert_eq!(plan.available, 3);
    assert!(!plan.exhausted(63));

    // An ask of 64 is also covered by the same two drops.
    let plan = plan_trim_drops(candidates.clone(), 64);
    assert_eq!(plan.drop_indices, vec![0, 1]);
    assert!(!plan.exhausted(64));

    // An ask past the whole tail drops everything and reports exhaustion.
    let plan = plan_trim_drops(candidates, 200);
    assert_eq!(plan.drop_indices.len(), 3);
    assert_eq!(plan.freed_bytes, 96);
    assert!(plan.exhausted(200));
}

#[test]
fn test_plan_trim_drops_is_independent_of_candidate_order() {
    // Two layouts of the same three rows; only the indices differ.
    let forward_rows = vec![
        (3usize, value("稀有词", "xi'you'ci", 5, false, 0)),
        (7, value("不像话", "bu'xiang'hua", 10, false, 0)),
        (9, value("东西", "dong'xi", 19, false, 0)),
    ];
    let mut shuffled_rows = forward_rows.clone();
    shuffled_rows.rotate_left(1);
    let forward = plan_trim_drops(forward_rows, 63);
    let shuffled = plan_trim_drops(shuffled_rows, 63);
    // The same words drop no matter which order the rows arrived in, and the
    // plan states them in the caller's own indices.
    let dropped = |plan: &TrimPlan| -> Vec<String> {
        plan.drop_indices
            .iter()
            .map(|index| match index {
                3 => "稀有词".to_owned(),
                7 => "不像话".to_owned(),
                _ => "东西".to_owned(),
            })
            .collect()
    };
    assert_eq!(forward.drop_indices, vec![3, 7]);
    assert_eq!(dropped(&forward), dropped(&shuffled));
    assert_eq!(forward.freed_bytes, shuffled.freed_bytes);
}

#[test]
fn test_tail_percentiles_cut_deciles_with_cumulative_floor_bytes() {
    // Ten tail rows at weights 1 to 10, text length 9 each (floor 33 bytes).
    let tail: Vec<(u32, usize)> = (1..=10).map(|weight| (weight, 9usize)).collect();
    let rows = tail_percentiles(tail);
    assert_eq!(rows.len(), 10);
    assert_eq!(rows[0].percentile, 10);
    assert_eq!(rows[0].weight_ceiling, 1);
    assert_eq!(rows[0].cumulative_entries, 1);
    assert_eq!(rows[9].percentile, 100);
    assert_eq!(rows[9].weight_ceiling, 10);
    assert_eq!(rows[9].cumulative_entries, 10);
    assert_eq!(rows[9].cumulative_floor_mib, 330.0 / BYTES_PER_MIB as f32);
}

#[test]
fn test_tail_percentiles_of_an_empty_tail_is_empty() {
    assert!(tail_percentiles(Vec::new()).is_empty());
}

/// Writes a three-word container the audit can read: one character-band word
/// (`中`, true frequency), one core-band word (`中国`, heavy two-character) and
/// one tail-band word (`不像话`, three characters), each under its own
/// single-candidate key. The sections are hand-assembled because the fixture
/// exercises the audit's reading path, not the compiler's writing path.
fn write_fixture_container(path: &std::path::Path) {
    use ime_dict::format::writer::DictWriter;
    use ime_dict::format::{DictEntry, SectionKind, hash_word, pack_fst_value};

    // Entry slots follow ids 0..3; `keys` is indexed by slot for the entry
    // rows, `key_rows` states the FST's lexicographic insert order.
    let texts = ["中", "中国", "不像话"];
    let keys = ["zhong", "zhong'guo", "bu'xiang'hua"];
    let weights = [800_000u32, 5_000, 30];
    let key_rows = [("bu'xiang'hua", 2usize), ("zhong", 0), ("zhong'guo", 1)];
    let mut fst = fst::MapBuilder::memory();
    let mut strpool = Vec::new();
    let mut entries = Vec::new();
    let mut unigram = Vec::new();
    for (slot, text) in texts.iter().enumerate() {
        let offset = strpool.len() as u32;
        strpool.extend_from_slice(text.as_bytes());
        let syllables = keys[slot].matches('\'').count() + 1;
        entries.extend_from_slice(
            &DictEntry::new(offset, text.len() as u16, syllables as u8, 0, weights[slot])
                .with_characters(text.chars().count() as u8)
                .encode(),
        );
        unigram.extend_from_slice(&hash_word(text).to_le_bytes());
        unigram.extend_from_slice(&4096u16.to_le_bytes());
        unigram.extend_from_slice(&0u16.to_le_bytes());
    }
    for (key, slot) in key_rows {
        fst.insert(key.as_bytes(), pack_fst_value(slot as u64, 1).expect("packing the value"))
            .expect("inserting the key");
    }
    let wordlist: Vec<u8> = (0u32..3).flat_map(|id| id.to_le_bytes()).collect();
    let mut writer = DictWriter::new();
    writer
        .add_section(SectionKind::Fst, fst.into_inner().expect("building the fst"))
        .expect("adding the fst section");
    writer
        .add_section(SectionKind::Entries, entries)
        .expect("adding the entries section");
    writer
        .add_section(SectionKind::StrPool, strpool)
        .expect("adding the strpool section");
    writer
        .add_section(SectionKind::Unigram, unigram)
        .expect("adding the unigram section");
    writer
        .add_section(SectionKind::Bigram, Vec::new())
        .expect("adding the bigram section");
    writer
        .add_section(SectionKind::WordList, wordlist)
        .expect("adding the wordlist section");
    writer.finish(path).expect("writing the fixture container");
}

/// Runs the audit channel over the fixture container and returns the report.
fn audit_fixture(tag: &str, trim_target_mib: Option<f64>) -> AuditReport {
    let dir = std::env::temp_dir().join(format!("rspinyin-audit-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("creating the audit fixture directory");
    let dict = dir.join("base.dict");
    write_fixture_container(&dict);
    let holdout = dir.join("holdout.tsv");
    std::fs::write(&holdout, "zhongguo\t中国\nbuxianghua\t不像话\n")
        .expect("writing the holdout fixture");

    let args = AuditArgs {
        root: Some(dir.clone()),
        dict: std::path::PathBuf::from("base.dict"),
        holdout: std::path::PathBuf::from("holdout.tsv"),
        core_min_freq: 20,
        trim_target_mib,
        json: false,
    };
    let report = run(args).expect("the audit runs");
    std::fs::remove_dir_all(&dir).expect("cleaning up");
    report
}

#[test]
fn test_audit_channel_partitions_the_container_into_three_bands() {
    let report = audit_fixture("bands", None);
    assert_eq!(report.entries, 3);
    assert_eq!(report.bands.len(), 3);
    let char_freq = &report.bands[0];
    let core = &report.bands[1];
    let tail = &report.bands[2];
    assert_eq!(char_freq.band, "char-freq");
    assert_eq!(core.band, "2char-core");
    assert_eq!(tail.band, "tail-flat");
    assert_eq!(char_freq.entries, 1);
    assert_eq!(core.entries, 1);
    assert_eq!(tail.entries, 1);

    // Every byte is attributed exactly once, so the band shares sum to the
    // container; the f32 rows keep a small rounding slack.
    let container_mib = report.container_bytes as f32 / BYTES_PER_MIB as f32;
    let summed: f32 = report.bands.iter().map(|row| row.bytes_mib).sum();
    assert!(
        (summed - container_mib).abs() < 0.01,
        "band shares {summed} must reconstruct the {container_mib} MiB container"
    );

    // The holdout asks for the core word and the tail word, and both rank
    // first: each band carries half of the set's top1.
    assert_eq!(char_freq.top1_share_pp, 0.0);
    assert_eq!(core.top1_share_pp, 50.0);
    assert_eq!(tail.top1_share_pp, 50.0);
    assert_eq!(char_freq.trim_candidates, 0, "the protected bands never trim");
    assert_eq!(core.trim_candidates, 0, "the protected bands never trim");
    assert_eq!(tail.trim_candidates, 0, "no target was simulated");
}

#[test]
fn test_audit_channel_simulates_the_trim_per_band() {
    // An unreachable target sends the simulation over the whole tail: one
    // candidate, reported on the tail row and nowhere else.
    let report = audit_fixture("trim-sim", Some(0.000001));
    let trim = report.trim.expect("the target was requested");
    assert_eq!(trim.candidates, 1);
    assert_eq!(trim.would_drop, 1);
    assert!(!trim.reaches_target, "one 33-byte word cannot reach any target");
    assert_eq!(report.bands[2].trim_candidates, 1);
    assert_eq!(report.bands[0].trim_candidates, 0);
    assert_eq!(report.bands[1].trim_candidates, 0);

    // A target the tiny fixture already fits plans no drops.
    let report = audit_fixture("trim-sim-fits", Some(1_000.0));
    let trim = report.trim.expect("the target was requested");
    assert_eq!(trim.would_drop, 0);
    assert!(trim.reaches_target);
}
