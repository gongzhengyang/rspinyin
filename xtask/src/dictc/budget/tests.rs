//! Tests for the per-section size accounting.
//!
//! The derivation tests use a small synthetic container ceiling, so they state what
//! the arithmetic does rather than restating the numbers a budget happens to hold
//! today. One test drives the real `docs/dev/budgets.json`, because the whole point
//! of deriving the section ceilings from it is that the two cannot disagree, and
//! that is only observable against the document the release gate reads.

use ime_dict::format::SectionKind;

use crate::budget::repo_root;
use crate::dictc::build::{Compiled, Stats};

use super::*;

/// A compiled container holding `words` entries and sections of the given sizes.
///
/// The sizes are passed as `[STRPOOL, ENTRIES, WORDLIST, FST, UNIGRAM]`, which is
/// the order the size table lists them in.
fn compiled(words: u64, sizes: [usize; 5]) -> Compiled {
    let [strpool, entries, wordlist, fst, unigram] = sizes;
    Compiled {
        entries: vec![0; entries],
        strpool: vec![0; strpool],
        unigram: vec![0; unigram],
        wordlist: vec![0; wordlist],
        fst: vec![0; fst],
        stats: Stats {
            words,
            ..Stats::default()
        },
    }
}

/// A ledger over a 1 MiB container ceiling, which puts every cap at a readable value.
fn small_budgets() -> SegmentBudgets {
    SegmentBudgets::from_container_ceiling(1000 * 1024)
}

#[test]
fn test_from_container_ceiling_reproduces_the_spec_table() {
    // The container ceiling docs/dev/budgets.json states today. At this value the
    // per-mille shares are exactly the ASM-05 numbers, which is what makes the share
    // table a derivation rather than a second copy of the budget.
    let budgets = SegmentBudgets::from_container_ceiling(20 * 1024 * 1024);
    assert_eq!(budgets.container(), 20_971_520);
    assert_eq!(budgets.section(SectionKind::StrPool), Some(4_194_304));
    assert_eq!(budgets.section(SectionKind::Entries), Some(6_815_744));
    assert_eq!(budgets.section(SectionKind::WordList), Some(2_097_152));
    assert_eq!(budgets.section(SectionKind::Fst), Some(3_984_588));
    assert_eq!(budgets.section(SectionKind::Unigram), Some(3_670_016));
    assert_eq!(budgets.total(), 18_350_080);
}

#[test]
fn test_from_container_ceiling_of_zero_yields_zero_ceilings() {
    let budgets = SegmentBudgets::from_container_ceiling(0);
    assert_eq!(budgets.total(), 0);
    for (_, cap) in budgets.sections() {
        assert_eq!(cap, 0, "every cap scales with the container");
    }
    assert_eq!(
        budgets.sections().len(),
        5,
        "a zero ceiling still names all five sections"
    );
}

#[test]
fn test_from_container_ceiling_does_not_overflow_at_the_extreme() {
    // A ceiling at the type's limit is absurd, but the share arithmetic still has to
    // produce a monotone allocation rather than wrap around. 875 per-mille of
    // u64::MAX, truncated, is 16,140,901,064,495,857,663.
    let budgets = SegmentBudgets::from_container_ceiling(u64::MAX);
    let total = budgets.total();
    assert_eq!(total, 16_140_901_064_495_857_663);
    for (kind, cap) in budgets.sections() {
        assert!(cap <= total, "{} is past the payload total", kind.name());
    }
}

#[test]
fn test_share_of_truncates_rather_than_rounding() {
    // 1999 * 875 / 1000 = 1749.125; truncation is the direction that fails closed.
    assert_eq!(share_of(1999, 875), 1749);
    assert_eq!(share_of(1000, 875), 875);
    assert_eq!(share_of(0, 875), 0);
    assert_eq!(
        share_of(u64::MAX, 1000),
        u64::MAX,
        "a whole ceiling is returned whole"
    );
}

#[test]
fn test_mebibytes_to_bytes_rejects_unusable_values() {
    assert_eq!(mebibytes_to_bytes(20.0), Some(20_971_520));
    assert_eq!(mebibytes_to_bytes(0.5), Some(524_288));
    assert_eq!(
        mebibytes_to_bytes(0.0),
        None,
        "a zero ceiling is not a budget"
    );
    assert_eq!(
        mebibytes_to_bytes(-1.0),
        None,
        "a negative ceiling is not one"
    );
    assert_eq!(mebibytes_to_bytes(f64::NAN), None);
    assert_eq!(mebibytes_to_bytes(f64::INFINITY), None);
    assert_eq!(
        mebibytes_to_bytes(2_000_000.0),
        None,
        "past the sanity bound"
    );
}

#[test]
fn test_load_reads_the_container_ceiling_from_the_budget_document() {
    let root = repo_root().expect("the repository root");
    let budgets = SegmentBudgets::load(&root).expect("the committed budget document");
    let expected = SegmentBudgets::from_container_ceiling(20 * 1024 * 1024);
    assert_eq!(
        budgets, expected,
        "the section caps must be a function of size_mb.base_dict and nothing else"
    );
}

#[test]
fn test_load_reports_a_missing_budget_document() {
    let root = repo_root().expect("the repository root");
    let missing = root.join("target").join("no-such-budget-root");
    let failure = SegmentBudgets::load(&missing).expect_err("a missing document must fail");
    assert!(
        failure.to_string().contains("budgets.json"),
        "the failure names the document it wanted: {failure}"
    );
}

#[test]
fn test_sections_lists_the_table_rows_in_order() {
    let kinds: Vec<SectionKind> = small_budgets()
        .sections()
        .into_iter()
        .map(|(kind, _)| kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            SectionKind::StrPool,
            SectionKind::Entries,
            SectionKind::WordList,
            SectionKind::Fst,
            SectionKind::Unigram,
        ],
        "the order is the order the size gate parses"
    );
}

#[test]
fn test_section_of_an_unbudgeted_kind_is_none() {
    let budgets = small_budgets();
    assert_eq!(
        budgets.section(SectionKind::Bigram),
        None,
        "the bigram section is reserved and always empty in version 1"
    );
    assert!(budgets.section(SectionKind::Fst).is_some());
}

#[test]
fn test_scope_label_matches_the_table_contract() {
    assert_eq!(Scope::Total.label(), "TOTAL");
    assert_eq!(Scope::Section(SectionKind::StrPool).label(), "StrPool");
    assert_eq!(Scope::Section(SectionKind::Entries).label(), "Entries");
    assert_eq!(Scope::Section(SectionKind::WordList).label(), "WordList");
    assert_eq!(Scope::Section(SectionKind::Fst).label(), "Fst");
    assert_eq!(Scope::Section(SectionKind::Unigram).label(), "Unigram");
    assert_ne!(
        Scope::Section(SectionKind::StrPool).label(),
        SectionKind::StrPool.name(),
        "the table's spelling is not the container's diagnostic spelling"
    );
}

#[test]
fn test_record_refuses_a_section_recorded_twice() {
    let mut ledger = Ledger::new(small_budgets());
    ledger
        .record(SectionKind::Fst, None, 1_000)
        .expect("the first record");
    let failure = ledger
        .record(SectionKind::Fst, None, 2_000)
        .expect_err("the second record of the same section must fail");
    assert!(failure.to_string().contains("FST"), "{failure}");
    assert_eq!(ledger.usage().len(), 1, "the refused row was not added");
    assert_eq!(
        ledger.usage_of(SectionKind::Fst).map(|row| row.bytes),
        Some(1_000),
        "the recorded row is untouched"
    );
}

#[test]
fn test_ledger_sums_the_recorded_sections_and_an_empty_ledger_sums_to_zero() {
    let mut ledger = Ledger::new(small_budgets());
    assert_eq!(ledger.total_bytes(), 0, "an empty ledger holds no bytes");
    assert!(ledger.is_within());
    assert_eq!(ledger.total_budget(), 896_000);

    ledger
        .record(SectionKind::Fst, None, 10_000)
        .expect("recording the fst");
    ledger
        .record(SectionKind::Entries, Some(7), 20_000)
        .expect("recording the entries");
    assert_eq!(ledger.total_bytes(), 30_000);
    assert_eq!(ledger.usage().len(), 2);
    assert_eq!(
        ledger
            .usage_of(SectionKind::Entries)
            .and_then(|row| row.entries),
        Some(7)
    );
    assert_eq!(ledger.usage_of(SectionKind::StrPool), None);
}

#[test]
fn test_violations_reports_a_section_over_its_ceiling() {
    let mut ledger = Ledger::new(small_budgets());
    // The FST cap is 190 per-mille of the 1,024,000-byte ceiling, which is 194,560
    // bytes.
    ledger
        .record(SectionKind::Fst, None, 194_561)
        .expect("recording the fst");
    let violations = ledger.violations();
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].scope, Scope::Section(SectionKind::Fst));
    assert_eq!(violations[0].excess(), 1);
    assert!(!ledger.is_within());
}

#[test]
fn test_violations_is_empty_when_a_row_sits_exactly_at_its_ceiling() {
    let mut ledger = Ledger::new(small_budgets());
    ledger
        .record(SectionKind::Fst, None, 194_560)
        .expect("recording the fst");
    assert!(
        ledger.is_within(),
        "a ceiling is a ceiling, not a strict bound"
    );
    assert!(ledger.violations().is_empty());
}

#[test]
fn test_violations_reports_the_total_even_when_every_section_fits() {
    // The section caps sum to more than the payload cap, so a build can sit inside
    // every one of them and still be over. That is the case the total exists for.
    let budgets = small_budgets();
    let mut ledger = Ledger::new(budgets.clone());
    let mut sum = 0u64;
    for (kind, cap) in budgets.sections() {
        ledger.record(kind, None, cap).expect("recording a section");
        sum = sum.saturating_add(cap);
    }
    assert!(
        sum > budgets.total(),
        "the caps really do sum past the total"
    );
    let violations = ledger.violations();
    assert_eq!(
        violations.len(),
        1,
        "only the total is over: {violations:?}"
    );
    assert_eq!(violations[0].scope, Scope::Total);
    assert_eq!(violations[0].excess(), sum - budgets.total());
}

#[test]
fn test_violation_excess_is_zero_at_the_ceiling_and_never_wraps() {
    let at = Violation {
        scope: Scope::Total,
        bytes: 100,
        budget: 100,
    };
    assert_eq!(at.excess(), 0);
    let under = Violation {
        scope: Scope::Total,
        bytes: 40,
        budget: 100,
    };
    assert_eq!(under.excess(), 0, "an under-budget row has no excess");
}

#[test]
fn test_grouped_inserts_thousands_separators() {
    assert_eq!(grouped(0), "0");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1_000), "1,000");
    assert_eq!(grouped(18_350_080), "18,350,080");
    assert_eq!(grouped(u64::MAX), "18,446,744,073,709,551,615");
}

#[test]
fn test_render_matches_the_stats_table_contract() {
    let mut ledger = Ledger::new(small_budgets());
    ledger
        .record(SectionKind::StrPool, None, 100_000)
        .expect("strpool");
    ledger
        .record(SectionKind::Entries, Some(1_234), 200_000)
        .expect("entries");
    ledger
        .record(SectionKind::WordList, Some(4_567), 90_000)
        .expect("wordlist");
    ledger.record(SectionKind::Fst, None, 120_000).expect("fst");
    ledger
        .record(SectionKind::Unigram, Some(1_234), 170_000)
        .expect("unigram");

    let expected = [
        "section     entries       bytes      budget",
        "StrPool           —     100,000     204,800",
        "Entries       1,234     200,000     332,800",
        "WordList      4,567      90,000     102,400",
        "Fst               —     120,000     194,560",
        "Unigram       1,234     170,000     179,200",
        "TOTAL             —     680,000     896,000",
    ]
    .join("\n");
    assert_eq!(ledger.render(), expected);
    assert!(
        !ledger.render().ends_with('\n'),
        "the caller decides how the table is terminated"
    );
}

#[test]
fn test_account_measures_the_five_payload_sections() {
    // Sizes are [STRPOOL, ENTRIES, WORDLIST, FST, UNIGRAM].
    let container = compiled(
        320_000,
        [3_142_016, 5_120_000, 1_656_000, 2_310_144, 2_560_000],
    );
    let ledger = account(&container, &small_budgets()).expect("accounting");

    assert_eq!(ledger.usage().len(), 5);
    assert_eq!(
        ledger
            .usage_of(SectionKind::Entries)
            .and_then(|row| row.entries),
        Some(320_000),
        "ENTRIES holds one row per word"
    );
    assert_eq!(
        ledger
            .usage_of(SectionKind::Unigram)
            .and_then(|row| row.entries),
        Some(320_000),
        "UNIGRAM holds one row per word too"
    );
    assert_eq!(
        ledger
            .usage_of(SectionKind::WordList)
            .and_then(|row| row.entries),
        Some(414_000),
        "WORDLIST holds one u32 per (key, word) pair"
    );
    assert_eq!(
        ledger
            .usage_of(SectionKind::StrPool)
            .and_then(|row| row.entries),
        None,
        "the string pool is pure bytes and has no entry count"
    );
    assert_eq!(
        ledger.total_bytes(),
        3_142_016 + 5_120_000 + 1_656_000 + 2_310_144 + 2_560_000
    );
}

#[test]
fn test_account_of_an_empty_compilation_is_within_budget() {
    let container = compiled(0, [0, 0, 0, 0, 0]);
    let ledger = account(&container, &small_budgets()).expect("accounting");
    assert_eq!(ledger.total_bytes(), 0);
    assert!(
        ledger.is_within(),
        "an empty container is trivially inside every ceiling"
    );
}

#[test]
fn test_enforce_accepts_a_build_inside_the_budgets() {
    let root = repo_root().expect("the repository root");
    let container = compiled(1_000, [1_000, 16_000, 4_000, 8_000, 8_000]);
    let ledger = enforce(&root, &container).expect("a small build fits");
    assert_eq!(ledger.total_bytes(), 37_000);
    assert!(ledger.is_within());
}

#[test]
fn test_enforce_fails_a_build_past_a_section_ceiling() {
    let root = repo_root().expect("the repository root");
    let budgets = SegmentBudgets::load(&root).expect("the committed budget document");
    let cap = budgets
        .section(SectionKind::Fst)
        .expect("the fst has a ceiling");
    let container = compiled(1, [0, 0, 0, cap as usize + 1, 0]);
    let failure = enforce(&root, &container).expect_err("one byte past the ceiling must fail");
    let message = failure.to_string();
    assert!(
        message.contains("Fst"),
        "the failure names the section: {message}"
    );
    assert!(
        message.contains("budgets.json"),
        "the failure names the document that owns the ceiling: {message}"
    );
}
