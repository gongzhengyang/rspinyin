//! Unit tests for the direct-drive harness.
//!
//! They cover the harness itself rather than the engine: the scenario model, the
//! divergence report, the doubles, the key-action vocabulary, and the properties the
//! acceptance criteria ask for -- every frozen code has a scenario that passes, a replay
//! is byte-identical, and the channel states which claims it cannot make. The engine's
//! own behaviour is covered by its own tests and by the built-in scenarios.
//!
//! The two acceptance criteria that are about a *run* rather than about the source -- no
//! file and no clock on the drive path, and a whole set that passes with no display
//! environment -- cannot be settled from inside a test process. They belong to
//! `--strace`, which starts this binary again as a child and reads its trace, and the
//! static scan at the foot of this file is what covers them when no child can be started.

use std::collections::{BTreeMap, BTreeSet};

use ime_types::{
    Candidate, CandidateSource, DecodeResult, KeyAction, Lexicon, PageState, Preedit,
    UserFreqSource,
};

use super::doubles::{LookupFailure, MockLexicon, MockUserFreq, OwnedWord};
// Imported from the submodule rather than through `super`'s re-exports: the re-exports
// exist for callers outside this file, and routing the tests through them made the
// non-test build report them as unused, since `mod tests` is `#[cfg(test)]`.
use super::scenario::{
    DictionarySpec, EngineFixture, Expectation, Scenario, SingleRow, Step, action_name, drives,
    parse_scenario,
};
use super::{
    REPEAT_RUNS, assert_repeatable, page_state, refused_a_lookup, run_scenario, scenarios,
};

/// Every frozen code the built-in set must assert, with a scenario that does so.
///
/// The four decode codes are the ones the acceptance criteria name; the three dictionary
/// codes are every failure the mock dictionary can answer a lookup with, which is what
/// makes the degraded branch of the decode covered for all of them rather than for
/// whichever two a scenario happened to pick.
const REQUIRED_CODES: [&str; 7] = [
    "decode/empty-input",
    "decode/no-path",
    "decode/too-long",
    "decode/invalid-char",
    "dict/unavailable",
    "dict/corrupt",
    "dict/unsupported",
];

/// Every variant the frozen `KeyAction` defines, as `(scenario name, action, drivable)`.
///
/// Written out rather than derived, because deriving it from the enum would make the list
/// follow a new variant automatically and defeat the point: an ADR appends to the frozen
/// enum, and a variant that reached this harness without a name a scenario file could
/// spell, or without a verdict on whether the harness can drive it, would be a scenario
/// author's silent no-op. The four variants the script-change ADR appended are the reason
/// the list exists at all -- a missing name there is a compile error in `action_name`,
/// but a missing entry in `drives` would only have shown up as an undrivable action.
const ALL_ACTIONS: [(&str, KeyAction, bool); 19] = [
    ("input-char:n", KeyAction::InputChar('n'), true),
    ("select-index:9", KeyAction::SelectIndex(9), false),
    ("move-highlight:-1", KeyAction::MoveHighlight(-1), false),
    ("move-caret:-1", KeyAction::MoveCaret(-1), true),
    ("backspace", KeyAction::Backspace, true),
    ("commit-highlighted", KeyAction::CommitHighlighted, false),
    ("commit-raw", KeyAction::CommitRaw, false),
    ("page-next", KeyAction::PageNext, false),
    ("page-prev", KeyAction::PagePrev, false),
    ("toggle-lang", KeyAction::ToggleLang, false),
    ("toggle-full-width", KeyAction::ToggleFullWidth, false),
    ("toggle-punct", KeyAction::TogglePunct, false),
    ("enter-temp-english", KeyAction::EnterTempEnglish, false),
    ("escape", KeyAction::Escape, true),
    ("ignore", KeyAction::Ignore, true),
    ("toggle-script", KeyAction::ToggleScript, false),
    ("forget-highlighted", KeyAction::ForgetHighlighted, false),
    ("pin-highlighted", KeyAction::PinHighlighted, false),
    ("add-phrase", KeyAction::AddPhrase, false),
];

/// One scenario file that names `action` and asserts nothing in particular.
fn scenario_naming(action: &str) -> String {
    format!(
        "name = \"test-action\"\n[[steps]]\naction = {action:?}\n\
         expect = {{ candidates = {{ first = \"\", count = 0, contains = [] }} }}\n"
    )
}

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

/// Builds one step's observation whose best candidate reads `text`.
///
/// The repeat assertion compares two of these, and a divergence between them is the one
/// thing a deterministic engine and immutable doubles cannot produce on demand, so the two
/// sides are built here rather than driven out of a scenario.
fn observation(text: &str) -> super::Observation {
    super::Observation {
        raw: String::from("ni"),
        caret: 2,
        candidates: vec![Candidate {
            index: 1,
            text: text.to_owned(),
            annotation: None,
            source: CandidateSource::Dict,
            score: 0.0,
            consumed_syllables: 1,
        }],
        segments: Vec::new(),
        degraded: false,
        preedit: Preedit {
            text: String::from("ni"),
            caret: 2,
            spans: Vec::new(),
        },
        page: PageState {
            current: 1,
            total: 1,
            page_size: 5,
        },
        codes: Vec::new(),
        refused: false,
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
    assert!(
        divergence.expected.contains('你'),
        "{}",
        divergence.expected
    );
    assert!(
        divergence.actual.contains("count=1"),
        "{}",
        divergence.actual
    );
    assert!(
        divergence.reason.contains("candidates"),
        "{}",
        divergence.reason
    );
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
    assert!(
        divergence.reason.contains("Backspace"),
        "{}",
        divergence.reason
    );
}

#[test]
fn test_run_scenario_pass_through_answer_reports_no_dictionary_code() {
    // The dictionary refuses `ni'hao`, but the input has no reading at all, so the decode
    // answers with the pass-through candidate and no lookup is ever refused. Claiming the
    // dictionary code here would be the harness inventing a fact.
    let spec = DictionarySpec {
        failures: BTreeMap::from([(String::from("ni'hao"), String::from("dict/unavailable"))]),
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

    assert!(
        divergence.reason.contains("surfaced"),
        "{}",
        divergence.reason
    );
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
fn test_engine_fixture_rejects_a_failure_code_the_mock_cannot_answer_with() {
    let spec = DictionarySpec {
        failures: BTreeMap::from([(String::from("ni"), String::from("dict/on-fire"))]),
        ..DictionarySpec::default()
    };
    let scenario = Scenario::new(
        "test-unknown-code",
        spec,
        vec![Step {
            action: KeyAction::Ignore,
            expect: Expectation::Degraded {
                code: String::from("decode/empty-input"),
            },
        }],
    );
    let error = EngineFixture::new(scenario).expect_err("the mock refuses with three codes");
    let report = format!("{error:#}");
    assert!(report.contains("dict/on-fire"), "{report}");
    assert!(report.contains("key \"ni\""), "{report}");
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
    let words = lexicon
        .lookup("ni'hao")
        .expect("an unknown key is not an error");
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
fn test_mock_lexicon_prefix_walks_its_range_and_answers_failures() {
    // Two keys share the prefix and one does not: the walk serves the matching
    // words in key order and stops at the first key that diverges, the same
    // range walk the real lexicon streams.
    let words = BTreeMap::from([
        (
            String::from("ni"),
            vec![OwnedWord::at(0, 1, "你"), OwnedWord::at(1, 1, "拟")],
        ),
        (String::from("nihao"), vec![OwnedWord::at(0, 2, "你好")]),
        (String::from("wo"), vec![OwnedWord::at(0, 1, "我")]),
    ]);
    let lexicon = MockLexicon::new(words, BTreeMap::new(), BTreeMap::new());
    let texts: Vec<&str> = lexicon
        .prefix("ni", 9)
        .expect("the prefix walk is served")
        .map(|word| word.text)
        .collect();
    assert_eq!(texts, ["你", "拟", "你好"]);
    // The limit pushes into the walk, so a query costs at most `limit` words
    // whatever the range holds.
    assert_eq!(
        lexicon
            .prefix("ni", 2)
            .expect("the capped walk is served")
            .count(),
        2
    );
    // A refused prefix is refused whole, like a refused key.
    let failures = BTreeMap::from([(String::from("ni"), LookupFailure::Corrupt)]);
    let broken = MockLexicon::new(BTreeMap::new(), BTreeMap::new(), failures);
    let error = broken.prefix("ni", 9).expect_err("the prefix is refused");
    assert_eq!(error.to_string().split(':').next(), Some("dict/corrupt"));
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
fn test_page_state_reports_the_engines_own_page_count() {
    // Five candidates a row is the shipped default, and the window reaches five pages, so
    // a list of sixteen fills four of them. The numbers are the engine's own: the same
    // call the session makes to build a frame.
    assert_eq!(page_state(0).total, 0);
    assert_eq!(page_state(0).current, 0);
    assert_eq!(page_state(1).total, 1);
    assert_eq!(page_state(5).total, 1);
    assert_eq!(page_state(6).total, 2);
    assert_eq!(page_state(16).total, 4);
    // Past the reach the count stops rather than growing: forty-five candidates are five
    // pages of nine at most, never nine pages of five.
    assert_eq!(page_state(45).total, 5);
    assert_eq!(page_state(3).current, 1);
    assert_eq!(page_state(3).page_size, 5);
}

#[test]
fn test_parse_scenario_reads_every_key_action_the_frozen_enum_defines() {
    for (name, action, drivable) in ALL_ACTIONS {
        match (parse_scenario(&scenario_naming(name)), drivable) {
            (Ok(scenario), true) => assert_eq!(scenario.steps[0].action, action, "{name}"),
            (Err(error), false) => {
                let report = format!("{error:#}");
                assert!(report.contains(name), "the refusal must name it: {report}");
                assert!(report.contains("cannot apply"), "{report}");
            }
            (Ok(_), false) => panic!("{name} is not drivable and the file was accepted"),
            (Err(error), true) => panic!("{name} must be driven, and was refused: {error:#}"),
        }
    }
}

#[test]
fn test_drives_classifies_every_action_the_frozen_enum_defines() {
    for (name, action, drivable) in ALL_ACTIONS {
        assert_eq!(drives(action), drivable, "{name}");
    }
}

#[test]
fn test_action_name_spells_every_action_the_way_a_scenario_file_does() {
    for (name, action, _) in ALL_ACTIONS {
        assert_eq!(action_name(action), name);
    }
}

#[test]
fn test_run_scenario_reports_every_action_it_cannot_drive() {
    // The other half of the classification: a step the harness cannot apply is reported
    // at its own index rather than silently skipped, so the list above cannot go stale
    // without a scenario failing.
    for (name, action, drivable) in ALL_ACTIONS {
        if drivable {
            continue;
        }
        let scenario = Scenario::new(
            "test-undrivable",
            DictionarySpec::default(),
            vec![Step {
                action,
                expect: Expectation::Degraded {
                    code: String::from("decode/empty-input"),
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
        .expect_err("an action the harness cannot drive is refused");
        assert_eq!(divergence.step, 0);
        assert_eq!(divergence.action, name);
        assert!(divergence.reason.contains("harness"), "{divergence:?}");
    }
}

#[test]
fn test_engine_module_declares_the_channel_boundary() {
    // The claim this channel cannot make is the one a reader is most likely to assume:
    // that a green run here says something about the route a real keystroke travels. The
    // statement lives in the module documentation, and this pins it there, because a
    // reader who found the harness's output quotable as an end-to-end verdict would be
    // reading a stronger result than the run produced. The injection channel's own
    // documentation makes the matching statement from its side.
    const MODULE: &str = include_str!("../engine.rs");
    for needle in [
        "XTEST",
        "must never be reported as each other",
        "not an end-to-end",
    ] {
        assert!(
            MODULE.contains(needle),
            "engine.rs must state the channel boundary, and {needle:?} is not in it"
        );
    }
}

#[test]
fn test_divergence_repeat_names_the_step_and_both_sequences() {
    let first = observation("你").print();
    let later = observation("尼").print();
    let action = Some(KeyAction::InputChar('i'));
    let divergence = super::Divergence::repeat(2, action, &first, &later, 7);

    assert_eq!(divergence.step, 2);
    assert_eq!(divergence.action, "input-char:i");
    assert!(divergence.expected.contains('你'), "{divergence:?}");
    assert!(divergence.actual.contains('尼'), "{divergence:?}");
    assert!(divergence.reason.contains("run 7"), "{divergence:?}");

    let report = divergence.describe();
    assert!(report.contains("step 2"), "{report}");
    assert!(report.contains("input-char:i"), "{report}");
    assert!(report.contains("expected:"), "{report}");
    assert!(report.contains("actual:"), "{report}");
}

#[test]
fn test_run_builtin_two_syllable_word_ranks_the_whole_word_first() {
    let outcome = run_builtin("engine-typing-reaches-a-word");
    assert!(outcome.is_ok(), "{outcome:?}");
}

#[test]
fn test_engine_source_uses_no_filesystem_or_clock() {
    // The dynamic half of this criterion is `--strace`, which cannot be run from inside a
    // test process: it starts a child under a syscall tracer, and a test that spawned one
    // would be measuring the test harness rather than the shipped drive path. This is the
    // half that runs everywhere, and it is honest about being a static one: the source of
    // every module the drive path is made of is embedded at compile time and scanned for
    // the calls that would break the property.
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
    // The two modules that do read files are deliberately outside the list: `cli.rs`
    // reads the scenario files, and `scenario/store.rs` builds a `[user_db]` scenario's
    // store and the bigram scenario's container while the fixture is assembled -- the
    // same load-time layer, never the drive.
    assert!(include_str!("cli.rs").contains("std::fs"));
    assert!(include_str!("scenario/store.rs").contains("std::fs"));
}

/// The v2 sections every built-in scenario leaves untouched.
///
/// The compatibility gate the v2 parser is held to: the built-in set was written before
/// the sections existed, and every one of them must parse and run with no `[decode]`,
/// no `[user_db]` and no `[decay]` at all -- which is what keeps a v1 fixture file and a
/// v2 harness reading the same document.
#[test]
fn test_every_builtin_scenario_stays_v1() {
    for scenario in scenarios() {
        assert!(
            scenario.decode.is_default(),
            "{}: the built-in set carries no [decode] section",
            scenario.name
        );
        assert!(
            scenario.user_db.is_default(),
            "{}: the built-in set carries no [user_db] section",
            scenario.name
        );
        assert!(
            scenario.decay.is_none(),
            "{}: the built-in set carries no [decay] section",
            scenario.name
        );
        EngineFixture::new(scenario).expect("the built-in set assembles");
    }
}

/// A v2 scenario that names the abbreviation switch through `[decode]`.
const ABBREV_V2: &str = r#"
name = "v2-abbrev"
[decode]
flags = ["ABBREV"]
[[dictionary.words]]
key = "ni'hao"
texts = ["你好"]
[[dictionary.words]]
key = "n'h"
texts = ["你好"]
[[steps]]
action = "input-char:n"
expect = { candidates = { first = "n", count = 1, contains = [] } }
[[steps]]
action = "input-char:h"
expect = { candidates = { first = "你好", count = 1, contains = [] } }
"#;

#[test]
fn test_v2_flags_reach_the_request() {
    // The v2 channel names the switch, and the switch does what the v1 field's own
    // scenario shows it doing: the initials reach the full spelling.
    let scenario = parse_scenario(ABBREV_V2).expect("the v2 scenario parses");
    assert!(!scenario.abbrev, "the v1 field stays clear");
    let fixture = EngineFixture::new(scenario).expect("the fixture assembles");
    run_scenario(
        &fixture.scenario,
        &fixture.lexicon,
        fixture.sources().user_freq,
        fixture.sources().lm,
    )
    .expect("the abbreviation reaches the full spelling through the v2 channel");
}

#[test]
fn test_v2_sections_refuse_what_cannot_be_assembled() {
    // A flag no bit carries is refused at parse, not silently ignored.
    assert!(
        parse_scenario(&ABBREV_V2.replace("ABBREV", "NOT_A_FLAG")).is_err(),
        "an unknown flag name is a parse error"
    );
    // A clock without a store folds nothing.
    let clock_no_store = r#"
name = "v2-clock"
[decay]
half_life_ticks = 139
[[dictionary.words]]
key = "ni'hao"
texts = ["你好"]
[[steps]]
action = "input-char:n"
expect = { candidates = { first = "n", count = 1, contains = [] } }
"#;
    assert!(
        parse_scenario(clock_no_store).is_err(),
        "a decay clock without a store is a parse error"
    );
    // A container model beside a spec-written one would make the score ambiguous.
    let bigram_and_spec = ABBREV_V2.replace(
        "[decode]\nflags = [\"ABBREV\"]",
        "[decode]\nbigram = \"present\"\n[[dictionary.unigrams]]\nword = \"你好\"\nscore = -128",
    );
    assert!(
        parse_scenario(bigram_and_spec.as_str()).is_err(),
        "bigram=present refuses a spec-written model"
    );
    // A real store beside a spec-written frequency table would make the boost ambiguous.
    let store_and_spec = ABBREV_V2.replace(
        "[decode]",
        "[user_db]\nfixture = \"cold\"\n[[dictionary.user_freq]]\nkey = \"你好\"\nvalue = 10\n[decode]",
    );
    assert!(
        parse_scenario(store_and_spec.as_str()).is_err(),
        "a user store refuses a spec-written frequency table"
    );
    // A fixture value that names nothing is refused.
    let bad_fixture = ABBREV_V2.replace("[decode]", "[user_db]\nfixture = \"borrowed\"\n[decode]");
    assert!(
        parse_scenario(bad_fixture.as_str()).is_err(),
        "an unknown fixture value is a parse error"
    );
}

/// The v2 scenarios of the add shard, as the repository's own fixture directory holds
/// them.
const ADD_SCENARIOS_DIR: &str = "../tests/fixtures/scenarios";

#[test]
fn test_the_add_shard_example_scenarios_replay_green() {
    // The physical replay: the fixture files are loaded the way `xtask testd-engine
    // --scenarios` loads them, and every scenario of the directory runs its steps and
    // its repeat assertion green.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(ADD_SCENARIOS_DIR);
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("reading the scenario directory") {
        let path = entry.expect("a directory entry").path();
        if path.extension().is_some_and(|ext| ext == "toml") {
            names.push(path);
        }
    }
    names.sort();
    assert!(names.len() >= 3, "the add shard's examples are all there");
    for path in &names {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let scenario =
            parse_scenario(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let fixture = EngineFixture::new(scenario)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let outcome = run_scenario(
            &fixture.scenario,
            &fixture.lexicon,
            fixture.sources().user_freq,
            fixture.sources().lm,
        )
        .map(|()| {
            assert_repeatable(&fixture.scenario, &fixture.sources(), REPEAT_RUNS)
                .map_err(|divergence| divergence.describe())
        });
        assert!(
            outcome.is_ok(),
            "{}: {:?}",
            path.display(),
            outcome.err().map(|divergence| divergence.describe())
        );
    }
}

#[test]
fn test_a_v2_scenario_parses_within_its_time_budget() {
    // The parse-increment budget: resolving a v2 file -- its sections, the flag and
    // fixture names, the cross-section rules -- costs at most a millisecond, so loading a
    // directory of scenarios stays invisible beside the replays it configures. The heaviest
    // example file is the subject: it carries all three sections and the longest step list.
    //
    // The assertion is the *fastest* of fifty parses, not each of them: the budget names
    // the parse's own cost, and a suite runs next to neighbours on the same cores, where a
    // single parse can be preempted no matter how small it is. The minimum is the one
    // number the scheduler noise cannot inflate.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(ADD_SCENARIOS_DIR)
        .join("add-user-pin.toml");
    let text = std::fs::read_to_string(&path).expect("reading the example scenario");
    let budget = std::time::Duration::from_millis(1);
    // A nextest run starts the moment the build ends, while its last compile jobs are
    // still draining; a measurement taken in that window times the builder beside it. The
    // settle is not part of the quantity measured -- it only keeps the builder out of it.
    std::thread::sleep(std::time::Duration::from_millis(150));
    let fastest = (0..50)
        .map(|_| {
            let began = std::time::Instant::now();
            parse_scenario(&text).expect("the scenario parses");
            began.elapsed()
        })
        .min()
        .expect("fifty attempts always yield a minimum");
    assert!(
        fastest <= budget,
        "the fastest of fifty parses took {fastest:?}, over the 1 ms budget"
    );
}

#[test]
fn test_the_user_db_store_serves_its_seeds() {
    // The [user_db] section replaces the spec's table with the real store, and the seed
    // file's commits are what the store answers with: the pinned word carries a
    // frequency, and the word the dictionary holds but the user never typed carries
    // none.
    let scenario = parse_scenario(
        r#"
name = "v2-user-pin"
[user_db]
fixture = "seed:pin"
[[dictionary.words]]
key = "ni'hao"
texts = ["你号", "你好"]
[[steps]]
action = "input-char:n"
expect = { candidates = { first = "n", count = 1, contains = [] } }
[[steps]]
action = "input-char:i"
expect = { candidates = { first = "ni", count = 1, contains = [] } }
[[steps]]
action = "input-char:h"
expect = { candidates = { first = "nih", count = 1, contains = [] } }
[[steps]]
action = "input-char:a"
expect = { candidates = { first = "niha", count = 1, contains = [] } }
[[steps]]
action = "input-char:o"
expect = { candidates = { first = "你好", count = 2, contains = ["你号"] } }
"#,
    )
    .expect("the scenario parses");
    let fixture = EngineFixture::new(scenario).expect("the store builds");
    let user_freq = fixture.sources().user_freq;
    assert!(
        user_freq.freq("你好") > 0,
        "the seeded word carries a frequency"
    );
    assert_eq!(user_freq.freq("你号"), 0, "the untyped word carries none");
    run_scenario(
        &fixture.scenario,
        &fixture.lexicon,
        user_freq,
        fixture.sources().lm,
    )
    .expect("the seeded word pins the key");
}

#[test]
fn test_bigram_present_takes_the_model_from_the_container() {
    // The container fixture's BIGRAM section is the model: the pair the corpus chains is
    // scored above the reversed pair the corpus never produces, and the spec is refused
    // a model of its own.
    let scenario = parse_scenario(
        r#"
name = "v2-bigram-present"
[decode]
bigram = "present"
[[dictionary.words]]
key = "zhong'guo"
texts = ["中国"]
[[dictionary.words]]
key = "yin'hang"
texts = ["银行"]
[[dictionary.words]]
key = "jing'ji"
texts = ["经济"]
[[steps]]
action = "input-char:a"
expect = { candidates = { first = "a", count = 1, contains = [] } }
"#,
    )
    .expect("the scenario parses");
    let fixture = EngineFixture::new(scenario).expect("the container builds");
    let lm = fixture.sources().lm;
    let forward = lm.bigram("中国", "银行");
    let reverse = lm.bigram("银行", "中国");
    assert!(
        forward > reverse,
        "the corpus pairs 中国 with 银行, never the reverse: {forward} vs {reverse}"
    );
    // Every word of the container is its own heaviest, so the whole unigram table
    // quantizes to the ceiling: a found word scores the Q12 maximum's log, which is
    // zero, and only a miss scores the floor.
    assert_eq!(
        lm.unigram("中国"),
        0,
        "the container's own word is found at the ceiling"
    );
    assert_ne!(
        lm.unigram("办公室"),
        0,
        "a word outside the container scores the miss"
    );
}
