//! Unit tests for the direct-drive harness.
//!
//! They cover the harness itself rather than the engine: the scenario model, the
//! divergence report, the doubles, and the two properties the acceptance criteria ask for
//! -- every frozen code has a scenario that passes, and a replay is byte-identical. The
//! engine's own behaviour is covered by its own tests and by the built-in scenarios.

use std::collections::{BTreeMap, BTreeSet};

use ime_types::{Candidate, CandidateSource, DecodeResult, KeyAction, Lexicon, UserFreqSource};

use super::{
    DictionarySpec, EngineFixture, Expectation, LookupFailure, MockLexicon, MockUserFreq,
    REPEAT_RUNS, Scenario, SingleRow, Step, assert_repeatable, parse_scenario, project_page,
    refused_a_lookup, run_scenario, scenarios,
};

/// Every frozen code the built-in set must assert, with a scenario that does so.
const REQUIRED_CODES: [&str; 6] = [
    "decode/empty-input",
    "decode/no-path",
    "decode/too-long",
    "decode/invalid-char",
    "dict/unavailable",
    "dict/unsupported",
];

/// Returns `true` when one of `scenario`'s steps expects `code` to be surfaced.
fn asserts_code(scenario: &Scenario, code: &str) -> bool {
    scenario.steps.iter().any(|step| {
        matches!(&step.expect, Expectation::Degraded { code: seen } if seen.as_str() == code)
    })
}

/// Runs one built-in scenario through the four-argument entry point.
fn run_builtin(name: &str) -> Result<(), super::Divergence> {
    let scenario = scenarios()
        .into_iter()
        .find(|scenario| scenario.name == name)
        .unwrap_or_else(|| panic!("no built-in scenario named {name}"));
    let fixture = EngineFixture::new(scenario).expect("a built-in fixture builds");
    run_scenario(
        &fixture.scenario,
        &fixture.lexicon,
        &fixture.user_freq,
        &fixture.lm,
    )
}

/// Builds the candidate a decode result carries in the `refused_a_lookup` test.
fn candidate(source: CandidateSource) -> Candidate {
    Candidate {
        index: 1,
        text: String::from("你"),
        annotation: None,
        source,
        score: 0.0,
        consumed_syllables: 1,
    }
}

#[test]
fn test_run_scenario_every_frozen_code_has_a_passing_scenario() {
    for code in REQUIRED_CODES {
        let scenario = scenarios()
            .into_iter()
            .find(|scenario| asserts_code(scenario, code))
            .unwrap_or_else(|| panic!("no built-in scenario asserts {code}"));
        let fixture = EngineFixture::new(scenario).expect("a built-in fixture builds");
        let outcome = run_scenario(
            &fixture.scenario,
            &fixture.lexicon,
            &fixture.user_freq,
            &fixture.lm,
        );
        assert!(outcome.is_ok(), "{code}: {outcome:?}");
    }
}

#[test]
fn test_assert_repeatable_one_hundred_replays_agree_on_every_scenario() {
    for scenario in scenarios() {
        let name = scenario.name.clone();
        let fixture = EngineFixture::new(scenario).expect("a built-in fixture builds");
        let outcome = assert_repeatable(&fixture.scenario, &fixture.sources(), REPEAT_RUNS);
        assert!(outcome.is_ok(), "{name}: {outcome:?}");
    }
}

#[test]
fn test_run_scenario_wrong_expectation_names_step_expected_and_actual() {
    let scenario = Scenario::new(
        "test-wrong-expectation",
        DictionarySpec::default(),
        vec![Step {
            action: KeyAction::InputChar('z'),
            expect: Expectation::Candidates {
                first: String::from("你"),
                count: 3,
                contains: Vec::new(),
            },
        }],
    );
    let fixture = EngineFixture::new(scenario).expect("the fixture builds");
    let divergence = run_scenario(
        &fixture.scenario,
        &fixture.lexicon,
        &fixture.user_freq,
        &fixture.lm,
    )
    .expect_err("the expectation is wrong");

    assert_eq!(divergence.step, 0);
    assert_eq!(divergence.action, "input-char:z");
    assert!(divergence.expected.contains('你'), "{}", divergence.expected);
    assert!(divergence.actual.contains("count=1"), "{}", divergence.actual);
    assert!(divergence.reason.contains("candidates"), "{}", divergence.reason);
    let report = divergence.describe();
    assert!(report.contains("step 0"), "{report}");
    assert!(report.contains("expected:"), "{report}");
    assert!(report.contains("actual:"), "{report}");
}

#[test]
fn test_run_scenario_undriven_action_is_reported_at_its_step() {
    let scenario = Scenario::new(
        "test-undriven",
        DictionarySpec::default(),
        vec![
            Step {
                action: KeyAction::Ignore,
                expect: Expectation::Degraded {
                    code: String::from("decode/empty-input"),
                },
            },
            Step {
                action: KeyAction::CommitHighlighted,
                expect: Expectation::Candidates {
                    first: String::new(),
                    count: 0,
                    contains: Vec::new(),
                },
            },
        ],
    );
    let fixture = EngineFixture::new(scenario).expect("the fixture builds");
    let divergence = run_scenario(
        &fixture.scenario,
        &fixture.lexicon,
        &fixture.user_freq,
        &fixture.lm,
    )
    .expect_err("the second step is not one the harness can apply");

    assert_eq!(divergence.step, 1);
    assert_eq!(divergence.action, "commit-highlighted");
    assert!(divergence.reason.contains("Backspace"), "{}", divergence.reason);
}

#[test]
fn test_run_scenario_pass_through_answer_reports_no_dictionary_code() {
    // The dictionary refuses `ni'hao`, but the input has no reading at all, so the decode
    // answers with the pass-through candidate and no lookup is ever refused. Claiming the
    // dictionary code here would be the harness inventing a fact.
    let spec = DictionarySpec {
        failures: BTreeMap::from([(
            String::from("ni'hao"),
            String::from("dict/unavailable"),
        )]),
        ..DictionarySpec::default()
    };
    let scenario = Scenario::new(
        "test-pass-through-is-not-a-refusal",
        spec,
        vec![Step {
            action: KeyAction::InputChar('z'),
            expect: Expectation::Degraded {
                code: String::from("dict/unavailable"),
            },
        }],
    );
    let fixture = EngineFixture::new(scenario).expect("the fixture builds");
    let divergence = run_scenario(
        &fixture.scenario,
        &fixture.lexicon,
        &fixture.user_freq,
        &fixture.lm,
    )
    .expect_err("no lookup was refused");

    assert!(divergence.reason.contains("surfaced"), "{}", divergence.reason);
}

#[test]
fn test_refused_a_lookup_reads_the_pass_through_answer_apart() {
    let pass_through = DecodeResult {
        candidates: vec![candidate(CandidateSource::Passthrough)],
        segments: Vec::new(),
        degraded: true,
    };
    assert!(!refused_a_lookup(&pass_through));

    let incomplete = DecodeResult {
        candidates: vec![candidate(CandidateSource::Dict)],
        segments: Vec::new(),
        degraded: true,
    };
    assert!(refused_a_lookup(&incomplete));

    let healthy = DecodeResult {
        candidates: vec![candidate(CandidateSource::Dict)],
        segments: Vec::new(),
        degraded: false,
    };
    assert!(!refused_a_lookup(&healthy));
}

#[test]
fn test_parse_scenario_reads_a_dictionary_and_its_steps() {
    let text = r#"
name = "test-parse"
[[dictionary.words]]
key = "ni"
texts = ["你"]
[[dictionary.user_words]]
text = "你"
[dictionary.user_freq]
"你" = 7
[[steps]]
action = "input-char:n"
expect = { candidates = { first = "n", count = 1, contains = [] } }
[[steps]]
action = "input-char:i"
expect = { preedit = { text = "ni", caret = 2, spans = 2 } }
"#;
    let scenario = parse_scenario(text).expect("the file parses");
    assert_eq!(scenario.name, "test-parse");
    assert_eq!(scenario.dictionary.words.len(), 1);
    assert_eq!(scenario.dictionary.words[0].key, "ni");
    assert_eq!(scenario.dictionary.words[0].texts, vec![String::from("你")]);
    assert_eq!(scenario.dictionary.user_freq.get("你"), Some(&7));
    assert_eq!(scenario.dictionary.user_words.len(), 1);
    assert_eq!(scenario.steps.len(), 2);
    assert_eq!(scenario.steps[1].action, KeyAction::InputChar('i'));
    assert_eq!(
        scenario.steps[1].expect,
        Expectation::Preedit {
            text: String::from("ni"),
            caret: 2,
            spans: 2,
        }
    );

    // The parsed scenario is runnable, which is what a fixture file has to be.
    let fixture = EngineFixture::new(scenario).expect("the fixture builds");
    let outcome = run_scenario(
        &fixture.scenario,
        &fixture.lexicon,
        &fixture.user_freq,
        &fixture.lm,
    );
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[test]
fn test_parse_scenario_rejects_an_unknown_key_action() {
    let text = r#"
name = "test-unknown-action"
[[steps]]
action = "teleport"
expect = { candidates = { first = "a", count = 1, contains = [] } }
"#;
    let error = parse_scenario(text).expect_err("teleport is not a key action");
    let report = format!("{error:#}");
    assert!(report.contains("unknown key action"), "{report}");
}

#[test]
fn test_parse_scenario_rejects_a_scenario_without_steps() {
    let error = parse_scenario("name = \"test-empty\"\n").expect_err("a scenario needs steps");
    let report = format!("{error:#}");
    assert!(report.contains("asserts nothing"), "{report}");
}

#[test]
fn test_parse_scenario_rejects_an_action_that_cannot_be_driven() {
    let text = r#"
name = "test-paging"
[[steps]]
action = "page-next"
expect = { page = { current = 2, total = 2 } }
"#;
    let error = parse_scenario(text).expect_err("paging belongs to the state machine");
    let report = format!("{error:#}");
    assert!(report.contains("cannot apply"), "{report}");
}

#[test]
fn test_scenarios_round_trip_through_toml() {
    for scenario in scenarios() {
        let name = scenario.name.clone();
        let text = toml::to_string_pretty(&scenario).expect("a scenario renders as TOML");
        let parsed = parse_scenario(&text).expect("a rendered scenario parses again");
        assert_eq!(parsed, scenario, "{name}");
    }
}

#[test]
fn test_engine_fixture_rejects_two_failure_codes() {
    let spec = DictionarySpec {
        failures: BTreeMap::from([
            (String::from("ni"), String::from("dict/unavailable")),
            (String::from("hao"), String::from("dict/unsupported")),
        ]),
        ..DictionarySpec::default()
    };
    let scenario = Scenario::new(
        "test-two-codes",
        spec,
        vec![Step {
            action: KeyAction::Ignore,
            expect: Expectation::Degraded {
                code: String::from("dict/unavailable"),
            },
        }],
    );
    let error = EngineFixture::new(scenario).expect_err("two codes make the report ambiguous");
    let report = format!("{error:#}");
    assert!(report.contains("one code"), "{report}");
}

#[test]
fn test_engine_fixture_rejects_a_fallback_that_is_not_a_syllable() {
    let spec = DictionarySpec {
        singles: vec![SingleRow {
            syllable: String::from("zzz"),
            texts: vec![String::from("字")],
        }],
        ..DictionarySpec::default()
    };
    let scenario = Scenario::new(
        "test-bad-syllable",
        spec,
        vec![Step {
            action: KeyAction::Ignore,
            expect: Expectation::Degraded {
                code: String::from("decode/empty-input"),
            },
        }],
    );
    let error = EngineFixture::new(scenario).expect_err("zzz is not a syllable");
    let report = format!("{error:#}");
    assert!(report.contains("not a syllable"), "{report}");
}

#[test]
fn test_mock_lexicon_unknown_key_answers_an_empty_list() {
    let lexicon = MockLexicon::default();
    let words = lexicon.lookup("ni'hao").expect("an unknown key is not an error");
    assert_eq!(words.len(), 0);
    assert_eq!(
        lexicon
            .fallback_single(ime_types::SyllableId::new(0), 9)
            .expect("an empty fallback is not an error")
            .count(),
        0
    );
}

#[test]
fn test_mock_lexicon_failing_key_answers_its_frozen_code() {
    let failures = BTreeMap::from([(String::from("ni'hao"), LookupFailure::Unavailable)]);
    let lexicon = MockLexicon::new(BTreeMap::new(), BTreeMap::new(), failures);
    let error = lexicon.lookup("ni'hao").expect_err("the key is refused");
    assert_eq!(
        error.to_string().split(':').next(),
        Some("dict/unavailable")
    );
    // A key that is not refused is still served, so one bad key cannot take the
    // dictionary out of the scenario.
    assert!(lexicon.lookup("ni").is_ok());
    assert_eq!(LookupFailure::Corrupt.code(), "dict/corrupt");
    assert_eq!(LookupFailure::Unsupported.code(), "dict/unsupported");
}

#[test]
fn test_mock_lexicon_prefix_reports_the_phase_two_code() {
    let lexicon = MockLexicon::default();
    let error = lexicon
        .prefix("ni", 9)
        .expect_err("prefix enumeration is a Phase 2 capability");
    assert_eq!(error.to_string(), "dict/unsupported");
}

#[test]
fn test_mock_user_freq_record_leaves_the_count_alone() {
    let counts = BTreeMap::from([(String::from("你好"), 3)]);
    let user = MockUserFreq::new(counts, BTreeSet::from([String::from("得")]));
    assert_eq!(user.freq("你好"), 3);
    assert_eq!(user.freq("unknown"), 0);
    assert!(user.is_user_word("得"));
    assert!(!user.is_user_word("你好"));

    user.record("你好", 100);
    assert_eq!(
        user.freq("你好"),
        3,
        "a replay must not change what the next replay reads"
    );
}

#[test]
fn test_project_page_counts_nine_candidates_per_page() {
    assert_eq!(project_page(0).total, 1);
    assert_eq!(project_page(1).total, 1);
    assert_eq!(project_page(9).total, 1);
    assert_eq!(project_page(10).total, 2);
    assert_eq!(project_page(45).total, 5);
    assert_eq!(project_page(3).current, 1);
    assert_eq!(project_page(3).page_size, 9);
}

#[test]
fn test_run_builtin_two_syllable_word_ranks_the_whole_word_first() {
    let outcome = run_builtin("engine-typing-reaches-a-word");
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[test]
fn test_engine_source_uses_no_filesystem_or_clock() {
    // The acceptance criterion asks for a `strace` run, which needs a command this
    // harness cannot issue from inside its own process. This is the strongest check that
    // can be made in-process: the source of every module the drive path is made of is
    // embedded at compile time and scanned for the calls that would break the property.
    // It is a static check and it says so -- the `strace` run is the dynamic half, and it
    // belongs to whoever runs the gates.
    const DRIVE_PATH: [(&str, &str); 6] = [
        ("engine.rs", include_str!("../engine.rs")),
        ("doubles.rs", include_str!("doubles.rs")),
        ("scenario.rs", include_str!("scenario.rs")),
        ("scenario/report.rs", include_str!("scenario/report.rs")),
        ("scenario/fixture.rs", include_str!("scenario/fixture.rs")),
        ("builtin.rs", include_str!("builtin.rs")),
    ];
    const NEEDLES: [&str; 7] = [
        "std::fs",
        "std::time",
        "std::env",
        "std::process",
        "SystemTime",
        "Instant",
        "File::",
    ];
    for (name, source) in DRIVE_PATH {
        for needle in NEEDLES {
            assert!(
                !source.contains(needle),
                "{name} names {needle}, which the drive path may not use"
            );
        }
    }
    // The one module that does read files is deliberately outside the list.
    assert!(include_str!("cli.rs").contains("std::fs"));
}
