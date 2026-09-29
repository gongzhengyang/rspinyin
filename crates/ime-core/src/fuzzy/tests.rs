//! Unit tests for the fuzzy syllable layer.
//!
//! They live beside the module rather than inside it because the suite is longer
//! than the code it drives; see `fuzzy.rs` for the responsibility and the
//! boundaries of what is under test.

use ime_types::DecodeFlags;

use super::*;
use crate::lm::Q;
use crate::segment::{EdgeKind, SYLLABLES, SyllableDag};

/// Every class bit the frozen namespace defines, as one flag set.
fn every_class() -> DecodeFlags {
    DecodeFlags::FUZZY
        | DecodeFlags::FUZZY_ZH_Z
        | DecodeFlags::FUZZY_CH_C
        | DecodeFlags::FUZZY_SH_S
        | DecodeFlags::FUZZY_N_L
        | DecodeFlags::FUZZY_AN_ANG
        | DecodeFlags::FUZZY_EN_ENG
        | DecodeFlags::FUZZY_IN_ING
        | DecodeFlags::FUZZY_F_H
}

/// The identifier of a syllable the table holds.
fn id_of(name: &str) -> SyllableId {
    lookup(name).expect("a syllable the table holds")
}

/// The spellings of a variant set, for an assertion that reads like the table.
fn spelled(set: &VariantSet) -> Vec<&'static str> {
    set.iter()
        .map(|id| syllable_at(*id).expect("a variant is a table entry"))
        .collect()
}

/// The spellings `name` reaches under `flags`.
fn reached(name: &str, flags: DecodeFlags) -> Vec<&'static str> {
    spelled(&variants(id_of(name), flags))
}

/// The `(class name, syllable, expected variant)` triples the shipped classes
/// are built for: one per class, each a confusion a speaker really makes.
fn class_cases() -> [(&'static str, &'static str, &'static str); 8] {
    [
        ("zh_z", "zhang", "zang"),
        ("ch_c", "chang", "cang"),
        ("sh_s", "shan", "san"),
        ("n_l", "ni", "li"),
        ("an_ang", "zhan", "zhang"),
        ("en_eng", "shen", "sheng"),
        ("in_ing", "xin", "xing"),
        ("f_h", "fan", "han"),
    ]
}

/// The `(end, syllable, kind)` triples of every edge of a graph, in node order.
///
/// A fingerprint for the test below: two graphs that answer with the same list
/// are the same segmentation of the same input, byte for byte.
fn edges_of(dag: &SyllableDag) -> Vec<(u16, u16, EdgeKind)> {
    let mut out = Vec::new();
    for node in 0..=usize::from(dag.len()) {
        for edge in dag.edges_from(node) {
            out.push((edge.end, edge.syllable.value(), edge.kind));
        }
    }
    out
}

#[test]
fn test_widening_leaves_the_segmentation_graph_byte_identical() {
    // The fuzzy layer widens which syllables a spelling reaches; it never rewrites
    // the input and never touches the graph. Widening every syllable of an input
    // under every class therefore leaves the segmentation exactly as it was, which
    // is what makes this a lattice-side addition rather than a decoder-side one --
    // and what makes a decode with the master switch clear behave as it did before
    // fuzzy matching existed.
    let mut dag = SyllableDag::new();
    assert!(dag.build("zanghaoni").is_ok());
    let before = edges_of(&dag);
    assert!(
        before.len() > 1,
        "the input must segment into several spans"
    );

    for node in 0..=usize::from(dag.len()) {
        for edge in dag.edges_from(node) {
            let widened = variants(edge.syllable, every_class());
            assert!(!widened.is_empty(), "node {node} widened to nothing");
        }
    }

    assert_eq!(edges_of(&dag), before);
    assert_eq!(dag.normalized(), "zanghaoni");
    assert!(dag.has_path());
}

#[test]
fn test_class_table_matches_the_frozen_flag_namespace() {
    assert_eq!(CLASSES.len(), 8);
    let mut seen = DecodeFlags::empty();
    for (index, class) in CLASSES.iter().enumerate() {
        // The bits run `1 << 1` through `1 << 8`, in table order, which is what
        // makes the table's order the same as the configuration's order.
        let expected = 1u16 << (index + 1);
        assert_eq!(class.flag.bits(), expected, "{}", class.name);
        assert!(!seen.contains(class.flag), "{} repeats a bit", class.name);
        seen |= class.flag;
        assert!(!class.canonical.is_empty() && !class.variant.is_empty());
        assert_ne!(class.canonical, class.variant, "{}", class.name);
        assert!(
            class.canonical.is_ascii() && class.variant.is_ascii(),
            "{} must be an ASCII spelling",
            class.name
        );
    }
    assert_eq!(seen.bits(), CLASS_BITS, "the table and the mask disagree");
    // The mask is `1 << 1` through `1 << 8`: the whole class namespace and no
    // part of the master switch.
    assert_eq!(CLASS_BITS, 0x01FE);
    assert_eq!(CLASS_BITS & DecodeFlags::FUZZY.bits(), 0);
}

#[test]
fn test_class_names_are_the_configuration_names() {
    let names: Vec<&str> = CLASSES.iter().map(|class| class.name).collect();
    assert_eq!(
        names.join(","),
        "zh_z,ch_c,sh_s,n_l,an_ang,en_eng,in_ing,f_h"
    );
    for class in &CLASSES {
        let found = class_by_name(class.name).map(|entry| entry.flag);
        assert_eq!(found, Some(class.flag));
    }
}

#[test]
fn test_each_class_reaches_the_spelling_it_names_and_nothing_else() {
    for (name, syllable, variant) in class_cases() {
        let class = class_by_name(name).expect("a shipped class");
        let flags = DecodeFlags::FUZZY | class.flag;
        let set = reached(syllable, flags);
        assert_eq!(set, [syllable, variant], "{name} on {syllable}");
    }
}

#[test]
fn test_each_class_also_reaches_back_from_the_variant_spelling() {
    // The relation is a pair, not a direction: a reader who writes `zang` for
    // `zhang` is the same reader who writes `zhang` for `zang`.
    for (name, syllable, variant) in class_cases() {
        let class = class_by_name(name).expect("a shipped class");
        let flags = DecodeFlags::FUZZY | class.flag;
        let set = reached(variant, flags);
        assert_eq!(set, [variant, syllable], "{name} on {variant}");
    }
}

#[test]
fn test_one_class_leaves_every_other_class_syllable_alone() {
    // Each class is applied on its own, against a syllable that belongs to
    // another class entirely: the widening is exactly what the class names and
    // never a general reshuffling of the initials.
    let cases = [
        ("zh_z", "shan"),
        ("ch_c", "sang"),
        ("sh_s", "cang"),
        ("n_l", "zhang"),
        ("an_ang", "zhong"),
        ("en_eng", "xing"),
        ("in_ing", "sheng"),
        ("f_h", "neng"),
    ];
    for (name, syllable) in cases {
        let class = class_by_name(name).expect("a shipped class");
        let flags = DecodeFlags::FUZZY | class.flag;
        assert_eq!(reached(syllable, flags), [syllable], "{name} on {syllable}");
    }
}

#[test]
fn test_master_switch_gates_every_class_bit() {
    for (name, syllable, variant) in class_cases() {
        let class = class_by_name(name).expect("a shipped class");
        // The class bit without the master switch does nothing at all.
        assert!(!is_enabled(class.flag), "{name} alone must not switch on");
        assert_eq!(reached(syllable, class.flag), [syllable], "{name} alone");
        assert_eq!(enabled_class_count(class.flag), 0, "{name} alone");
        // The master switch with the class bit widens.
        let both = DecodeFlags::FUZZY | class.flag;
        assert!(is_enabled(both), "{name} with the master switch");
        assert_eq!(reached(syllable, both), [syllable, variant], "{name}");
    }
}

#[test]
fn test_master_switch_without_a_class_bit_is_the_same_as_off() {
    let flags = DecodeFlags::FUZZY;
    assert!(!is_enabled(flags));
    assert_eq!(enabled_class_count(flags), 0);
    assert!(!may_truncate(flags));
    for &name in SYLLABLES {
        assert_eq!(reached(name, flags), [name]);
    }
}

#[test]
fn test_variants_with_fuzzy_off_hold_the_syllable_alone_and_stay_inline() {
    // The off path is the one every keystroke of a user who never turned fuzzy
    // matching on takes, so it must not look a syllable up or spill the set.
    let off = [
        DecodeFlags::empty(),
        DecodeFlags::default(),
        DecodeFlags::FUZZY,
        DecodeFlags::FUZZY_ZH_Z,
        every_class() & !DecodeFlags::FUZZY,
    ];
    for flags in off {
        assert!(!is_enabled(flags), "{flags:?}");
        for (index, name) in SYLLABLES.iter().enumerate() {
            let syllable = SyllableId::new(u16::try_from(index).unwrap_or(u16::MAX));
            let mut set = VariantSet::new();
            assert!(!variants_into(&mut set, syllable, flags), "{name}");
            assert_eq!(set.len(), 1, "{name}");
            assert_eq!(set[0], syllable, "{name}");
            assert!(!set.spilled(), "{name} must stay in the caller's frame");
        }
    }
}

#[test]
fn test_two_classes_compose_into_the_spelling_both_confusions_reach() {
    // `zang` read through both `zh`/`z` and `an`/`ang` is the set a reader who
    // confuses both is asking for: the class applied second widens what the
    // class applied first produced, not only the original spelling.
    let flags = DecodeFlags::FUZZY | DecodeFlags::FUZZY_ZH_Z | DecodeFlags::FUZZY_AN_ANG;
    assert_eq!(reached("zang", flags), ["zang", "zhang", "zan", "zhan"]);
    // The same set from the other side, in the order that side reaches it.
    assert_eq!(reached("zhang", flags), ["zhang", "zang", "zhan", "zan"]);
}

#[test]
fn test_variants_skip_spellings_the_table_does_not_hold() {
    // `zuang` and `sei` are not syllables, so the classes that would reach them
    // reach nothing: a variant has to be a spelling the table holds.
    assert_eq!(lookup("zuang"), None);
    assert_eq!(lookup("sei"), None);
    assert_eq!(
        reached("zhuang", DecodeFlags::FUZZY | DecodeFlags::FUZZY_ZH_Z),
        ["zhuang"]
    );
    assert_eq!(
        reached("shei", DecodeFlags::FUZZY | DecodeFlags::FUZZY_SH_S),
        ["shei"]
    );
    // `den` and `din` are not syllables either.
    assert_eq!(
        reached("deng", DecodeFlags::FUZZY | DecodeFlags::FUZZY_EN_ENG),
        ["deng"]
    );
    assert_eq!(
        reached("ding", DecodeFlags::FUZZY | DecodeFlags::FUZZY_IN_ING),
        ["ding"]
    );
}

#[test]
fn test_variants_of_the_zero_initial_syllables_reach_their_pair() {
    // `an` and `ang` are syllables with no initial at all, so the class applies
    // to the whole spelling rather than to a final behind an initial.
    let flags = DecodeFlags::FUZZY | DecodeFlags::FUZZY_AN_ANG;
    assert_eq!(reached("an", flags), ["an", "ang"]);
    assert_eq!(reached("ang", flags), ["ang", "an"]);
}

#[test]
fn test_every_syllable_reaches_at_least_itself_under_every_class_combination() {
    // A representative slice of the table crossed with all 512 combinations of
    // the master switch and the eight class bits: every set is non-empty, holds
    // the syllable it was built from first, holds no duplicate, and stays inside
    // the cap and in the caller's frame.
    let syllables = ["zhang", "zang", "chan", "shan", "shen", "xin", "fan"];
    for bits in 0..=0x01FFu16 {
        let flags = DecodeFlags::from_bits_truncate(bits);
        for name in syllables {
            let syllable = id_of(name);
            let mut set = VariantSet::new();
            variants_into(&mut set, syllable, flags);
            assert!(!set.is_empty(), "{name} under {bits:#06x}");
            assert_eq!(set[0], syllable, "{name} under {bits:#06x}");
            assert!(set.len() <= MAX_VARIANTS, "{name} under {bits:#06x}");
            assert!(!set.spilled(), "{name} under {bits:#06x}");
            for (index, id) in set.iter().enumerate() {
                assert!(syllable_at(*id).is_some(), "{name} under {bits:#06x}");
                assert!(!set[..index].contains(id), "{name} under {bits:#06x}");
            }
        }
    }
}

#[test]
fn test_every_syllable_is_widened_into_table_entries_under_every_class() {
    // Every identifier a variant set holds comes from a table lookup, so a
    // widened syllable is always a syllable the dictionary can be keyed by.
    for class in &CLASSES {
        let flags = DecodeFlags::FUZZY | class.flag;
        for (index, name) in SYLLABLES.iter().enumerate() {
            let syllable = SyllableId::new(u16::try_from(index).unwrap_or(u16::MAX));
            for id in variants(syllable, flags) {
                assert!(
                    syllable_at(id).is_some(),
                    "{name} under {} reached an identifier outside the table",
                    class.name
                );
            }
        }
    }
}

#[test]
fn test_no_shipped_class_combination_reaches_the_variant_cap() {
    // A syllable has one initial and one final, each belongs to at most one
    // class, and a class adds at most one spelling per spelling already in the
    // set -- so the widest set the shipped table produces holds four spellings
    // and the cap is a guard rather than a live limit.
    for (index, name) in SYLLABLES.iter().enumerate() {
        let syllable = SyllableId::new(u16::try_from(index).unwrap_or(u16::MAX));
        let mut set = VariantSet::new();
        let truncated = variants_into(&mut set, syllable, every_class());
        assert!(!truncated, "{name} hit the cap");
        assert!(set.len() <= 4, "{name} reached {} spellings", set.len());
    }
}

#[test]
fn test_expand_into_reports_truncation_when_the_cap_bites() {
    // The shipped cap is never reached, so the truncation branch is exercised
    // through the cap the private expansion takes as a parameter.
    let flags = DecodeFlags::FUZZY | DecodeFlags::FUZZY_ZH_Z | DecodeFlags::FUZZY_AN_ANG;
    let mut set = VariantSet::new();
    set.push(id_of("zang"));
    assert!(expand_into(&mut set, flags, 2), "the cap must report");
    assert_eq!(spelled(&set), ["zang", "zhang"]);
    // The same expansion without a cap that bites takes every spelling and
    // reports nothing.
    let mut roomy = VariantSet::new();
    roomy.push(id_of("zang"));
    assert!(!expand_into(&mut roomy, flags, MAX_VARIANTS));
    assert_eq!(spelled(&roomy), ["zang", "zhang", "zan", "zhan"]);
}

#[test]
fn test_expand_into_stops_at_the_first_class_the_cap_cuts_short() {
    // Two classes would each add a spelling; a cap of one leaves room for
    // neither, and the answer is the same whichever class the table reaches
    // first.
    let flags = DecodeFlags::FUZZY | DecodeFlags::FUZZY_ZH_Z | DecodeFlags::FUZZY_AN_ANG;
    let mut set = VariantSet::new();
    set.push(id_of("zang"));
    assert!(expand_into(&mut set, flags, 1));
    assert_eq!(spelled(&set), ["zang"]);
}

#[test]
fn test_may_truncate_needs_a_fourth_class() {
    // Three classes reach exactly the cap, so only a fourth can pass it. The
    // truth table is walked over every prefix of the class table.
    let mut flags = DecodeFlags::FUZZY;
    assert_eq!(enabled_class_count(flags), 0);
    assert!(!may_truncate(flags));
    for (index, class) in CLASSES.iter().enumerate() {
        flags |= class.flag;
        assert_eq!(enabled_class_count(flags), index + 1);
        assert_eq!(may_truncate(flags), index + 1 > 3, "{} classes", index + 1);
    }
    assert_eq!(enabled_class_count(every_class()), CLASSES.len());
    assert!(may_truncate(every_class()));
    // The class bits without the master switch never count.
    assert_eq!(enabled_class_count(every_class() & !DecodeFlags::FUZZY), 0);
    assert!(!may_truncate(every_class() & !DecodeFlags::FUZZY));
}

#[test]
fn test_variants_are_the_same_across_100_runs() {
    let cases = [
        (
            "zang",
            DecodeFlags::FUZZY | DecodeFlags::FUZZY_ZH_Z | DecodeFlags::FUZZY_AN_ANG,
        ),
        ("zhang", every_class()),
        (
            "ning",
            DecodeFlags::FUZZY | DecodeFlags::FUZZY_N_L | DecodeFlags::FUZZY_IN_ING,
        ),
        ("shen", every_class()),
    ];
    for (name, flags) in cases {
        let expected: Vec<u16> = variants(id_of(name), flags)
            .iter()
            .map(|id| id.value())
            .collect();
        for run in 0..100 {
            let seen: Vec<u16> = variants(id_of(name), flags)
                .iter()
                .map(|id| id.value())
                .collect();
            assert_eq!(seen, expected, "{name}, run {run}");
        }
    }
}

#[test]
fn test_variants_into_replaces_the_previous_set_and_keeps_the_buffer() {
    let mut set = VariantSet::new();
    let flags = DecodeFlags::FUZZY | DecodeFlags::FUZZY_ZH_Z | DecodeFlags::FUZZY_AN_ANG;
    assert!(!variants_into(&mut set, id_of("zang"), flags));
    assert_eq!(spelled(&set), ["zang", "zhang", "zan", "zhan"]);
    // A second call describes the second syllable and leaves nothing of the
    // first behind.
    assert!(!variants_into(&mut set, id_of("ni"), flags));
    assert_eq!(spelled(&set), ["ni"]);
    assert!(!set.spilled());
}

#[test]
fn test_class_by_name_is_case_insensitive_and_rejects_what_it_does_not_know() {
    let upper = class_by_name("ZH_Z").map(|class| class.flag);
    assert_eq!(upper, Some(DecodeFlags::FUZZY_ZH_Z));
    let mixed = class_by_name("An_Ang").map(|class| class.flag);
    assert_eq!(mixed, Some(DecodeFlags::FUZZY_AN_ANG));
    for unknown in ["", "zhz", "zh", "n-l", "n_l ", "fuzzy", "n_l_x", "全部"] {
        assert!(
            class_by_name(unknown).is_none(),
            "{unknown:?} is not a class"
        );
    }
}

#[test]
fn test_max_variants_and_the_penalty_are_the_documented_constants() {
    assert_eq!(MAX_VARIANTS, 8);
    assert_eq!(FUZZY_PENALTY_Q8, 12 << 8);
    assert_eq!(FUZZY_PENALTY_Q8, 3072);
    // A substitution replaces one spelling with another of comparable length,
    // so no class can build a spelling longer than the buffer a join uses.
    for class in &CLASSES {
        assert!(class.canonical.len() <= MAX_SPELLING / 2, "{}", class.name);
        assert!(class.variant.len() <= MAX_SPELLING / 2, "{}", class.name);
    }
}

#[test]
fn test_penalize_breaks_a_tie_toward_the_exact_match() {
    let exact = -3 * Q * Q;
    let fuzzy = penalize(exact);
    assert!(fuzzy < exact, "an exact match must outrank a fuzzy one");
    assert_eq!(exact - fuzzy, FUZZY_PENALTY_Q8);
    // A fuzzy reading likelier by more than the penalty still wins.
    assert!(penalize(exact + FUZZY_PENALTY_Q8 + 1) > exact);
    // Exactly one penalty likelier is the tie the penalty exists to remove.
    assert_eq!(penalize(exact + FUZZY_PENALTY_Q8), exact);
    // A score at the floor of the range saturates instead of wrapping.
    assert_eq!(penalize(i32::MIN), i32::MIN);
    assert_eq!(penalize(0), -FUZZY_PENALTY_Q8);
}

#[test]
fn test_penalize_costs_less_than_a_twentieth_of_one_log_probability_unit() {
    // One unit of log probability is `Q * Q` in the Q16.16 edge score the sweep
    // ranks with, so the penalty is a few per cent of it: enough to lose a tie,
    // small enough that a likelier fuzzy reading still wins.
    let penalty = penalize(0).abs();
    assert!(penalty > 0);
    assert!(penalty < (Q * Q) / 20);
}
