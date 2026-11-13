//! Tests for the reverse-lookup module: the domain decision, the case split
//! with the codepoint domain, the payload bounds, and the candidate rendering.

use crate::quick::{self, QuickDomain};
use ime_types::DictError;

use super::{MAX_CANDIDATES, MAX_PAYLOAD_LEN, ReverseHit, ReverseSource, candidates, recognize};

#[test]
fn test_recognize_letters_after_lowercase_u_open_the_domain() {
    let matched = recognize("uhf").expect("a letter payload opens the domain");
    assert_eq!(matched.trigger_len, 1);
    assert_eq!(matched.payload_len, 2);
    assert_eq!(recognize("ua").map(|found| found.payload_len), Some(1));
    assert_eq!(recognize("uhfx").map(|found| found.payload_len), Some(3));
}

#[test]
fn test_recognize_bare_u_keeps_composing_as_pinyin() {
    // A bare trigger is an ordinary syllable: the domain opens only once a
    // payload letter stands behind it.
    assert_eq!(recognize("u"), None);
    assert_eq!(recognize(""), None);
}

#[test]
fn test_recognize_uppercase_trigger_belongs_to_the_codepoint_domain() {
    // The case split is the contract: the uppercase spelling is the codepoint
    // domain's, whatever follows it.
    assert_eq!(recognize("U4f60"), None);
    assert_eq!(recognize("Uhf"), None);
    assert_eq!(recognize("Uh"), None);
}

#[test]
fn test_recognize_digit_payload_leaves_the_codepoint_spelling_alone() {
    // `u4f60` is the lowercase codepoint spelling, so any payload holding a
    // digit — not only a digit-initial one — must stay out of this domain.
    assert_eq!(recognize("u4f60"), None);
    assert_eq!(recognize("uh4"), None);
    assert_eq!(recognize("u-hf"), None);
}

#[test]
fn test_recognize_overrun_payload_returns_to_ordinary_input() {
    // Four letters can still be a subsequence of a code; five cannot, so past
    // the bound the buffer is ordinary input again.
    let at_bound = format!("u{}", "h".repeat(MAX_PAYLOAD_LEN));
    assert!(recognize(&at_bound).is_some(), "{at_bound} is in domain");
    let past_bound = format!("u{}", "h".repeat(MAX_PAYLOAD_LEN + 1));
    assert_eq!(recognize(&past_bound), None);
}

#[test]
fn test_recognize_reverse_and_codepoint_domains_split_by_case_and_digits() {
    // The recognizers overlap on pure-hex-letter payloads (`ab` is both a
    // component fragment and hex digits); the router resolves that overlap by
    // consulting this domain first for lowercase-`u` buffers, so the pinned
    // facts are: the uppercase spelling is never ours, digit payloads are never
    // ours, and a letter payload is ours even when quick would also take it.
    assert_eq!(recognize("u4f60"), None);
    assert_eq!(
        quick::recognize(true, "u4f60", &[], false).map(|found| found.domain),
        Some(QuickDomain::Codepoint),
        "the lowercase codepoint spelling keeps digit payloads"
    );
    assert_eq!(recognize("Ubadf"), None);
    assert_eq!(
        quick::recognize(true, "Ubadf", &[], false).map(|found| found.domain),
        Some(QuickDomain::Codepoint),
        "the uppercase spelling stays the codepoint domain's"
    );
    assert!(recognize("uab").is_some());
    assert!(
        quick::recognize(true, "uab", &[], false).is_some(),
        "the overlap the router resolves by priority"
    );
    // A pure component fragment quick has no claim on is unambiguously ours.
    assert!(recognize("uhf").is_some());
    assert!(quick::recognize(true, "uhf", &[], false).is_none());
}

/// A hit list covering the rendering contract: one normal character, one
/// polyphonic character whose annotation is its decomposition code, and enough
/// padding to exercise the cap.
fn hits(count: usize) -> Vec<ReverseHit<'static>> {
    const CODES: [&str; 3] = ["hf", "sz", "kg"];
    (0..count)
        .map(|index| {
            let code = CODES[index % CODES.len()];
            ReverseHit {
                character: char::from_u32(0x4E00 + index as u32).unwrap_or('一'),
                code,
            }
        })
        .collect()
}

#[test]
fn test_candidates_render_character_text_with_code_annotation() {
    let rendered = candidates(&hits(3));
    assert_eq!(rendered.len(), 3);
    assert_eq!(rendered[0].text, "一");
    assert_eq!(rendered[0].annotation.as_deref(), Some("hf"));
    assert_eq!(rendered[2].annotation.as_deref(), Some("kg"));
}

#[test]
fn test_candidates_polyphone_annotation_is_the_decomposition_code() {
    // The annotation must describe the decomposition, not the character's own
    // reading: a polyphonic character (长, read chang or zhang) keeps the code
    // its components spell, so the hint is stable across both readings.
    let chang = [ReverseHit {
        character: '长',
        code: "py",
    }];
    let rendered = candidates(&chang);
    assert_eq!(rendered[0].text, "长");
    assert_eq!(rendered[0].annotation.as_deref(), Some("py"));
}

#[test]
fn test_candidates_cap_truncates_a_broad_query_deterministically() {
    let rendered = candidates(&hits(MAX_CANDIDATES + 50));
    assert_eq!(rendered.len(), MAX_CANDIDATES);
    // The truncation keeps the head in container order: the last kept
    // candidate is the hit just before the cap, not the last hit typed.
    let last = char::from_u32(0x4E00 + MAX_CANDIDATES as u32 - 1).expect("in range");
    assert_eq!(rendered[MAX_CANDIDATES - 1].text, String::from(last));
}

#[test]
fn test_candidates_empty_hits_render_an_empty_list() {
    // The empty answer is the "no character decomposes this way" case, not an
    // error: the panel shows the query with no candidates.
    assert!(candidates(&[]).is_empty());
}

/// A source over a fixed hit table, the in-memory double the router-side tests
/// use; it also proves the trait is object-safe over `&dyn`.
struct MockSource {
    present: bool,
}

impl ReverseSource for MockSource {
    fn has_reverse_index(&self) -> bool {
        self.present
    }

    fn reverse_lookup(&self, payload: &str) -> Result<Vec<ReverseHit<'_>>, DictError> {
        if !self.present {
            return Err(DictError::LengthOutOfRange {
                field: "reverse_absent",
                value: payload.len() as u64,
            });
        }
        Ok(codes())
    }
}

/// Codes for the mock: every query answers with the fixture pair, so the
/// assertion is about plumbing, not data. The literals are `'static`, which
/// coerces down to the `&self`-borrowed hit lifetime the trait hands out.
fn codes() -> Vec<ReverseHit<'static>> {
    vec![ReverseHit {
        character: '房',
        code: "hf",
    }]
}

#[test]
fn test_reverse_source_absent_index_is_the_degradation_gate() {
    let source = MockSource { present: false };
    assert!(!source.has_reverse_index());
    assert!(matches!(
        source.reverse_lookup("hf"),
        Err(DictError::LengthOutOfRange {
            field: "reverse_absent",
            ..
        })
    ));
}

#[test]
fn test_reverse_source_present_index_answers_borrowed_hits() {
    let source = MockSource { present: true };
    assert!(source.has_reverse_index());
    let found: Vec<ReverseHit<'_>> = source.reverse_lookup("hf").expect("hits");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].character, '房');
    assert_eq!(found[0].code, "hf");
}

#[test]
fn test_reverse_source_object_safe_over_a_dyn_bound() {
    let source = MockSource { present: true };
    let source: &dyn ReverseSource = &source;
    assert!(source.has_reverse_index());
    assert_eq!(source.reverse_lookup("hf").expect("hits")[0].code, "hf");
}
