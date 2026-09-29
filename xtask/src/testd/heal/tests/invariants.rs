//! The invariants a re-derivation keeps, swept over every constant the table anchors.
//!
//! The sweeps walk every constant the specification's own exception table names rather than one
//! chosen example, because the properties they assert are meant to hold for all of them: a heal
//! lands on a declaration that states what the specification states, a value that moved is never
//! covered, and a name that moved is found again in one step whatever words it gained. A single
//! re-derivation can show that one constant behaves; the sweep is what makes the rule a rule.
//!
//! The three cases beside the sweeps are the boundaries the same rule has: the unit is part of
//! the value, a reordered name is a different name, and the answer does not depend on how often
//! it is asked for.

use super::{cell_declarations, declarations, repository_constants, spec, stated, with_renamed};
use crate::testd::heal::{ConstRef, HealError, resolve};
use crate::testd::ui_metrics::{SlintConstants, SpecTable, Unit};

/// A metrics block declaring one constant, with an expression written in `unit`.
///
/// The declaration is built in the unit the row states rather than always as a length, because
/// the unit is part of the value: a fixture that declared a font size as a length would be
/// describing a declaration the specification does not.
///
/// # Panics
///
/// Panics when the fixture does not parse, which no fixture this builds can do.
fn declaration(unit: Unit, name: &str, value: u16) -> SlintConstants {
    let (type_name, expression) = match unit {
        Unit::Dp => ("length", format!("{value}px")),
        Unit::Sp => ("length", format!("{value}sp")),
        Unit::Count => ("count", value.to_string()),
    };
    let text = format!(
        "export global CandidateMetrics {{\n    out property <{type_name}> {name}: \
         {expression};\n}}\n"
    );
    SlintConstants::parse("a one-constant fixture", &text).expect("the fixture parses")
}

/// Every constant of the metrics block the exception table anchors, with the unit and the value
/// the row that fixes it states.
///
/// The name, the unit and the value all come out of the two sources the cases read rather than
/// out of a list written here: a constant the table no longer anchors drops out of the sweep, and
/// one it gained is swept without this file changing.
///
/// # Panics
///
/// Panics when the repository's own sources cannot be read, which is the same failure the cases
/// in the parent module report.
fn anchored_by_the_table(spec: &SpecTable) -> Vec<(String, Unit, u16)> {
    let constants = repository_constants();
    spec.exceptions()
        .keys()
        .filter_map(|name| {
            let resolved = resolve(ConstRef::ByName(name.as_str()), &constants, spec).ok()?;
            let value = resolved.anchor.number()?;
            Some((name.clone(), resolved.anchor.unit, value))
        })
        .collect()
}

#[test]
fn test_resolve_never_heals_a_declaration_that_disagrees_with_the_specification() {
    let spec = spec();
    let anchored = anchored_by_the_table(&spec);
    assert!(
        anchored.len() >= 11,
        "the sweep has to walk the constants the table anchors rather than pass over them: {}",
        anchored.len()
    );

    for (name, unit, value) in &anchored {
        let renamed = format!("candidate-{name}");
        let source = declaration(*unit, &renamed, *value);

        let healed = resolve(ConstRef::ByName(name.as_str()), &source, &spec)
            .unwrap_or_else(|error| panic!("{name} is declared as `{renamed}`: {error}"));

        assert!(
            healed.healed,
            "{name}: the name is gone, so the row has to supply it again"
        );
        assert_eq!(healed.name, renamed, "{name}");
        assert_eq!(
            healed.renamed_from.map(|from| from.to_owned()),
            Some(name.clone()),
            "{name}: the record has to carry the name the locator used"
        );
        assert!(
            healed.matches_spec(),
            "{name}: a heal that carried a value the row does not state would be a bar lowered \
             rather than a name found again"
        );
    }
}

#[test]
fn test_resolve_never_heals_a_declaration_whose_value_moved() {
    let spec = spec();
    let anchored = anchored_by_the_table(&spec);
    assert!(
        anchored.len() >= 11,
        "the sweep has to walk the constants the table anchors rather than pass over them: {}",
        anchored.len()
    );

    for (name, unit, value) in &anchored {
        let drifted = value.saturating_add(1);
        let source = declaration(*unit, name, drifted);

        let found = resolve(ConstRef::ByName(name.as_str()), &source, &spec)
            .unwrap_or_else(|error| panic!("{name} is declared: {error}"));

        assert!(
            !found.healed,
            "{name}: a value that moved is not a rename, so nothing is healed"
        );
        assert_eq!(found.value_dp, u32::from(drifted), "{name}");
        assert!(
            !found.matches_spec(),
            "{name}: the declaration and the specification disagree, and the case has to fail on \
             it rather than a heal cover it"
        );
    }
}

#[test]
fn test_resolve_reports_a_rename_that_states_the_row_value_in_another_unit_as_drift() {
    let spec = spec();
    let value = stated(&spec, "候选单元高度", Unit::Dp, 0);
    // The row states a length in device-independent pixels; the declaration that took over the
    // name states the same number as a font size. The numbers agree and the values do not, which
    // is exactly what a comparison on numbers alone would miss.
    let source = declaration(Unit::Sp, "candidate-cell-height", value);

    let resolved = resolve(ConstRef::ByName("cell-height"), &source, &spec);

    assert_eq!(
        resolved,
        Err(HealError::Drifted {
            name: String::from("cell-height"),
            declared_name: String::from("candidate-cell-height"),
            declared: format!("{value}sp"),
            spec: format!("{value}dp"),
        }),
        "a declaration of another unit is not the constant the row fixes, so it is drift rather \
         than a rename"
    );
}

#[test]
fn test_resolve_heals_a_rename_that_gained_words_at_both_ends_in_one_step() {
    let spec = spec();
    let renamed = with_renamed(
        &cell_declarations(&spec),
        "cell-height",
        "candidate-cell-height-dp",
    );
    let constants = declarations(&renamed);

    let healed =
        resolve(ConstRef::ByName("cell-height"), &constants, &spec).expect("the rename heals");
    assert!(healed.healed);
    assert_eq!(healed.name, "candidate-cell-height-dp");
    assert!(healed.matches_spec());

    // A second resolution is a second call of the same function and not a second step of a loop:
    // the answer is the one above, so a pass has nothing left to iterate towards.
    let again =
        resolve(ConstRef::ByName("cell-height"), &constants, &spec).expect("the rename heals");
    assert_eq!(
        again, healed,
        "a re-derivation is a function of the two sources and of nothing else"
    );
}

#[test]
fn test_resolve_refuses_a_reordered_rename_rather_than_guessing() {
    let spec = spec();
    let renamed = with_renamed(&cell_declarations(&spec), "cell-height", "height-cell");
    let constants = declarations(&renamed);

    let resolved = resolve(ConstRef::ByName("cell-height"), &constants, &spec);

    assert!(
        matches!(resolved, Err(HealError::Unresolvable { .. })),
        "the same words in another order are a different name, and the candidate test is a run of \
         words rather than a similarity: {resolved:?}"
    );
}
