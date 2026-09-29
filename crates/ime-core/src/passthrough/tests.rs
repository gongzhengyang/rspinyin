//! Tests for the passthrough policy.
//!
//! Every test drives [`classify`] or one of the punctuation helpers from literals, so
//! nothing here needs a configuration file, a key router or a display server.

use ime_types::ConfigError;

use super::punctuation::{FULL_WIDTH_OFFSET, PUNCTUATION_TABLE, punctuation_target};
use super::*;

/// The `CommitDirectly` decision carrying `text`.
fn commit(text: &str) -> PassthroughDecision {
    PassthroughDecision::CommitDirectly(String::from(text))
}

/// Builds flags from the four `[engine]` settings, with temporary English mode
/// off; that mode has a constant of its own below.
const fn configured(
    auto_english_on_uppercase: bool,
    passthrough_url: bool,
    punct_mode: PunctMode,
    full_width: bool,
) -> PassthroughFlags {
    PassthroughFlags {
        auto_english_on_uppercase,
        passthrough_url,
        punct_mode,
        full_width,
        temp_english: false,
    }
}

/// The flags the configuration loader produces when the user has no `[engine]`
/// section at all.
const DEFAULT_FLAGS: PassthroughFlags = configured(true, true, PunctMode::Chinese, false);
/// `auto_english_on_uppercase = false`.
const NO_UPPERCASE_RULE: PassthroughFlags = configured(false, true, PunctMode::Chinese, false);
/// `passthrough_url = false`.
const NO_URL_RULE: PassthroughFlags = configured(true, false, PunctMode::Chinese, false);
/// `punct_mode = "english"`.
const ENGLISH_PUNCT: PassthroughFlags = configured(true, true, PunctMode::English, false);
/// `full_width = true`.
const FULL_WIDTH: PassthroughFlags = configured(true, true, PunctMode::Chinese, true);
/// The defaults with temporary English mode switched on.
const TEMP_ENGLISH: PassthroughFlags = PassthroughFlags {
    auto_english_on_uppercase: true,
    passthrough_url: true,
    punct_mode: PunctMode::Chinese,
    full_width: false,
    temp_english: true,
};

/// The decision a table case expects.
///
/// A stand-in for [`PassthroughDecision`] that a `const` can hold: the real
/// decision carries a `String`, and `String::from` is not a constant
/// expression. Each variant maps onto exactly one decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expected {
    /// [`PassthroughDecision::HostHandles`].
    Host,
    /// [`PassthroughDecision::CommitDirectly`] with this text.
    Commit(&'static str),
    /// [`PassthroughDecision::Decode`].
    Decode,
}

impl Expected {
    /// The decision this case expects.
    fn decision(self) -> PassthroughDecision {
        match self {
            Self::Host => PassthroughDecision::HostHandles,
            Self::Commit(text) => commit(text),
            Self::Decode => PassthroughDecision::Decode,
        }
    }
}

/// The rule table: the text a key press produced, the flags, the text around the
/// caret, and the decision the first matching rule must produce.
const RULE_CASES: &[(&str, PassthroughFlags, Option<&str>, Expected)] = &[
    // Rule 1: a key that produced no text is never ours, whatever the text
    // around the caret looks like.
    ("", DEFAULT_FLAGS, None, Expected::Host),
    ("", DEFAULT_FLAGS, Some("https://example"), Expected::Host),
    ("", TEMP_ENGLISH, None, Expected::Host),
    // Rule 2: temporary English mode passes every key through, including the
    // uppercase letters and the marks the rules below would take.
    ("a", TEMP_ENGLISH, None, Expected::Host),
    ("N", TEMP_ENGLISH, None, Expected::Host),
    ("hello", TEMP_ENGLISH, None, Expected::Host),
    (",", TEMP_ENGLISH, Some("他说："), Expected::Host),
    // Rule 3: a leading uppercase letter is committed and leaves the session.
    ("N", DEFAULT_FLAGS, None, Expected::Commit("N")),
    ("Nihao", DEFAULT_FLAGS, None, Expected::Commit("N")),
    ("N", NO_UPPERCASE_RULE, None, Expected::Decode),
    ("n", DEFAULT_FLAGS, None, Expected::Decode),
    ("Z", FULL_WIDTH, None, Expected::Commit("Ｚ")),
    ("Ä", DEFAULT_FLAGS, None, Expected::Decode),
    // The uppercase rule is ordered ahead of the URL rule.
    ("A", DEFAULT_FLAGS, Some("user@"), Expected::Commit("A")),
    // Rule 4: URL and email evidence in the text of the key press itself.
    ("@", DEFAULT_FLAGS, None, Expected::Host),
    ("www.example.com", DEFAULT_FLAGS, None, Expected::Host),
    ("http://x", DEFAULT_FLAGS, None, Expected::Host),
    ("a.com", DEFAULT_FLAGS, None, Expected::Host),
    ("@", NO_URL_RULE, None, Expected::Decode),
    ("www.example.com", NO_URL_RULE, None, Expected::Decode),
    // Rule 4: the same evidence in the token under the caret.
    ("n", DEFAULT_FLAGS, Some("https://exa"), Expected::Host),
    ("m", DEFAULT_FLAGS, Some("www.exa"), Expected::Host),
    ("n", DEFAULT_FLAGS, Some("foo@bar"), Expected::Host),
    ("o", DEFAULT_FLAGS, Some("foo.com"), Expected::Host),
    ("n", NO_URL_RULE, Some("foo@bar"), Expected::Decode),
    // Evidence the caret has already left is history, not context.
    (
        "n",
        DEFAULT_FLAGS,
        Some("https://old.example was great "),
        Expected::Decode,
    ),
    ("n", DEFAULT_FLAGS, Some("你好"), Expected::Decode),
    ("n", DEFAULT_FLAGS, Some(""), Expected::Decode),
    ("n", DEFAULT_FLAGS, None, Expected::Decode),
    // Rule 5: Chinese punctuation, one case per substituted table entry.
    (",", DEFAULT_FLAGS, None, Expected::Commit("，")),
    (".", DEFAULT_FLAGS, None, Expected::Commit("。")),
    (";", DEFAULT_FLAGS, None, Expected::Commit("；")),
    (":", DEFAULT_FLAGS, None, Expected::Commit("：")),
    ("?", DEFAULT_FLAGS, None, Expected::Commit("？")),
    ("!", DEFAULT_FLAGS, None, Expected::Commit("！")),
    ("(", DEFAULT_FLAGS, None, Expected::Commit("（")),
    (")", DEFAULT_FLAGS, None, Expected::Commit("）")),
    ("[", DEFAULT_FLAGS, None, Expected::Commit("【")),
    ("]", DEFAULT_FLAGS, None, Expected::Commit("】")),
    ("\"", DEFAULT_FLAGS, None, Expected::Commit("“")),
    (
        "\"",
        DEFAULT_FLAGS,
        Some("他说：“你好"),
        Expected::Commit("”"),
    ),
    // The apostrophe is the syllable separator and is never substituted.
    ("'", DEFAULT_FLAGS, None, Expected::Decode),
    // English punctuation mode leaves the mark to the application.
    (",", ENGLISH_PUNCT, None, Expected::Host),
    (",", ENGLISH_PUNCT, Some("你好"), Expected::Host),
    // Rule 5 fallthrough: pinyin, and characters the table does not name.
    ("ni", DEFAULT_FLAGS, None, Expected::Decode),
    ("nihao", DEFAULT_FLAGS, None, Expected::Decode),
    ("ni'hao", DEFAULT_FLAGS, None, Expected::Decode),
    ("/", DEFAULT_FLAGS, None, Expected::Decode),
    ("~", DEFAULT_FLAGS, None, Expected::Decode),
    ("，", DEFAULT_FLAGS, None, Expected::Decode),
    // Substitution applies to a lone mark; a longer input is pinyin that happens
    // to contain punctuation, and the normalizer reports the stray character
    // instead of the plugin quietly committing it.
    (",,", DEFAULT_FLAGS, None, Expected::Decode),
    ("n,", DEFAULT_FLAGS, None, Expected::Decode),
    ("ni.", DEFAULT_FLAGS, None, Expected::Decode),
    (",", FULL_WIDTH, None, Expected::Commit("，")),
];

#[test]
fn test_classify_rule_table_returns_the_expected_decision() {
    assert!(
        RULE_CASES.len() >= 40,
        "the table must keep at least 40 cases"
    );
    for (raw, flags, surrounding, expected) in RULE_CASES {
        assert_eq!(
            classify(raw, *flags, *surrounding),
            expected.decision(),
            "classify({raw:?}, {flags:?}, {surrounding:?})"
        );
    }
}

#[test]
fn test_classify_only_reads_the_token_under_the_caret() {
    let flags = DEFAULT_FLAGS;
    // An address the user has already left, outside the scan window.
    let left_behind = format!("https://old.example {}你好", "x".repeat(300));
    assert_eq!(
        classify("n", flags, Some(&left_behind)),
        PassthroughDecision::Decode
    );
    // The same evidence inside the window but not under the caret: only the
    // token after the last space counts.
    let after_a_space = format!("{}foo@bar 你好", "x".repeat(60));
    assert_eq!(
        classify("n", flags, Some(&after_a_space)),
        PassthroughDecision::Decode
    );
    // Under the caret, however much text precedes it.
    let under_caret = format!("{}foo@bar", "x".repeat(300));
    assert_eq!(
        classify("n", flags, Some(&under_caret)),
        PassthroughDecision::HostHandles
    );
}

#[test]
fn test_classify_surrounding_none_is_the_conservative_branch() {
    // `None` means the application does not report surrounding text. It must
    // behave exactly like surrounding text with no URL or email evidence, so
    // that the answer never depends on whether the host supports the feature.
    let flags = DEFAULT_FLAGS;
    let no_evidence = Some("今天天气不错 ");
    for raw in ["", "n", "N", ",", "\"", "'", "@", "nihao"] {
        assert_eq!(
            classify(raw, flags, None),
            classify(raw, flags, no_evidence),
            "classify({raw:?}) must not depend on unreported surrounding text"
        );
    }
}

#[test]
fn test_classify_punctuation_table_maps_the_twelve_ascii_marks() {
    // One assertion per table entry, in table order. The apostrophe is the
    // twelfth mark and is deliberately not substituted: in Chinese mode it is
    // the pinyin syllable separator, which the input normalizer consumes before
    // the decoder sees the buffer.
    let flags = DEFAULT_FLAGS;
    let expected = [
        (',', Some('，')),
        ('.', Some('。')),
        (';', Some('；')),
        (':', Some('：')),
        ('?', Some('？')),
        ('!', Some('！')),
        ('(', Some('（')),
        (')', Some('）')),
        ('[', Some('【')),
        (']', Some('】')),
        ('"', Some('“')),
        ('\'', None),
    ];
    assert_eq!(expected.len(), PUNCTUATION_COUNT);
    for (mark, target) in expected {
        let wanted = match target {
            Some(ch) => commit(&ch.to_string()),
            None => PassthroughDecision::Decode,
        };
        assert_eq!(
            classify(&mark.to_string(), flags, None),
            wanted,
            "mapping of {mark:?}"
        );
    }
}

#[test]
fn test_punctuation_table_entries_match_the_lookup() {
    // The table and its index lookup are two halves of one mapping. Pinning them
    // together means reordering the table without updating the `match` cannot
    // silently change what a keystroke commits.
    assert_eq!(PUNCTUATION_TABLE.len(), PUNCTUATION_COUNT);
    for (mark, target) in PUNCTUATION_TABLE {
        assert_eq!(punctuation_target(mark), target, "lookup for {mark:?}");
    }
    // Characters outside the table have no target at all.
    assert_eq!(punctuation_target('/'), None);
    assert_eq!(punctuation_target('你'), None);
    assert_eq!(punctuation_target(' '), None);
}

#[test]
fn test_classify_double_quote_follows_the_unclosed_quote_before_the_caret() {
    let flags = DEFAULT_FLAGS;
    assert_eq!(classify("\"", flags, None), commit("“"));
    assert_eq!(classify("\"", flags, Some("他说：")), commit("“"));
    assert_eq!(classify("\"", flags, Some("他说：“你好")), commit("”"));
    // Balanced again, so the next quote opens.
    assert_eq!(classify("\"", flags, Some("他说：“你好”")), commit("“"));
}

#[test]
fn test_to_full_width_maps_every_printable_ascii_character() {
    let mut mapped = 0usize;
    for code in 0x21u32..=0x7E {
        let narrow = char::from_u32(code).expect("0x21..=0x7E are scalar values");
        let wide = char::from_u32(code + FULL_WIDTH_OFFSET).expect("the offset stays in range");
        assert_eq!(to_full_width(narrow), wide, "full-width form of {narrow:?}");
        assert!(('\u{FF01}'..='\u{FF5E}').contains(&wide));
        mapped += 1;
    }
    assert_eq!(mapped, 94, "0x21..=0x7E is 94 characters");
    assert_eq!(to_full_width(' '), '\u{3000}');
}

#[test]
fn test_to_full_width_leaves_everything_else_alone() {
    // Below the printable range, above it, already full width, and CJK: all
    // unchanged, which is what makes the mapping safe to apply to the Chinese
    // punctuation the table produces.
    assert_eq!(to_full_width('\u{0}'), '\u{0}');
    assert_eq!(to_full_width('\u{1F}'), '\u{1F}');
    assert_eq!(to_full_width('\u{7F}'), '\u{7F}');
    assert_eq!(to_full_width('你'), '你');
    assert_eq!(to_full_width('，'), '，');
    assert_eq!(to_full_width('\u{FF01}'), '\u{FF01}');
    assert_eq!(to_full_width('é'), 'é');
    // Idempotent: widening a widened character is that character again.
    assert_eq!(to_full_width(to_full_width('a')), to_full_width('a'));
}

#[test]
fn test_passthrough_flags_default_matches_the_engine_defaults() {
    // The `[engine]` defaults, and the two spellings its punct_mode key accepts.
    assert_eq!(PunctMode::try_from("chinese"), Ok(PunctMode::Chinese));
    assert_eq!(PunctMode::try_from("english"), Ok(PunctMode::English));

    let flags = PassthroughFlags::default();
    assert!(flags.auto_english_on_uppercase);
    assert!(flags.passthrough_url);
    assert_eq!(flags.punct_mode, PunctMode::Chinese);
    assert!(!flags.full_width);
    assert!(!flags.temp_english);
}

#[test]
fn test_punct_mode_try_from_rejects_an_unknown_value() {
    for rejected in ["Chinese", "ENGLISH", "", "cn", "zh"] {
        let err = PunctMode::try_from(rejected).expect_err("the match is exact");
        assert!(matches!(err, ConfigError::Invalid { .. }));
        let message = err.to_string();
        assert!(message.starts_with("config/invalid"), "{message}");
        assert!(message.contains("engine.punct_mode"), "{message}");
    }
}

#[test]
fn test_passthrough_decision_reports_consumption_and_committed_text() {
    assert!(!PassthroughDecision::HostHandles.is_consumed());
    assert!(PassthroughDecision::Decode.is_consumed());
    assert!(PassthroughDecision::EnterTempEnglish.is_consumed());
    assert_eq!(PassthroughDecision::EnterTempEnglish.committed_text(), None);
    assert_eq!(PassthroughDecision::HostHandles.committed_text(), None);
    assert_eq!(PassthroughDecision::Decode.committed_text(), None);

    let committed = commit("，");
    assert!(committed.is_consumed());
    assert_eq!(committed.committed_text(), Some("，"));
}
