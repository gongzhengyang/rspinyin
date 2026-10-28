//! The codepoint translator behind the `U` trigger: type a scalar, get its character.
//!
//! # Responsibility
//!
//! Turn the hex digits a user typed after `U` into the Unicode scalar they name, plus
//! the run of successor scalars the panel offers as suggestions. The mechanism is the
//! codepoint-input panel every serious CJK input method ships; the table, the bounds
//! and the diagnostics are this project's own.
//!
//! # Boundaries
//!
//! Pure: a function of the hex payload and nothing else. A payload naming a value
//! outside the scalar range — past U+10FFFF, or inside the surrogate block no `char`
//! can hold — is answered with one candidate echoing the input and carrying the
//! stable `quick/expr/invalid` code, never with a panic and never with a partial
//! character. A payload longer than six digits cannot name a scalar at all and takes
//! the same echo, which is what keeps an over-typed panel honest instead of silent.

use super::QuickCandidate;

/// The highest Unicode scalar value.
const MAX_SCALAR: u32 = 0x10_FFFF;

/// The first surrogate value; the block D800..=DFFF holds no characters.
const SURROGATE_FIRST: u32 = 0xD800;

/// The last surrogate value.
const SURROGATE_LAST: u32 = 0xDFFF;

/// The most successor suggestions one valid codepoint carries.
const SUCCESSOR_COUNT: usize = 16;

/// The annotation code an inadmissible payload echoes with.
const INVALID_CODE: &str = "quick/expr/invalid";

/// Renders the candidates one codepoint payload answers with.
///
/// # Arguments
///
/// * `payload` - the hex digits after the trigger, case-insensitive.
///
/// # Returns
///
/// For an admissible payload naming a scalar: the character itself, then up to
/// [`SUCCESSOR_COUNT`] successors, each annotated with its `U`-prefixed uppercase
/// hex. Successors skip the surrogate block and stop at the end of the plane. For a
/// payload that names no scalar: one candidate echoing the input with the invalid
/// annotation. For an empty or non-hex payload: an empty list.
///
/// # Panics
///
/// Never: every bound is checked before a conversion, and the hex parse is checked.
pub fn translate(payload: &str) -> Vec<QuickCandidate> {
    // Past the echo bound the payload is ordinary typing the trigger table never
    // matched; the translator answers it the same way, with nothing.
    if payload.len() > super::MAX_CODEPOINT_ECHO_DIGITS {
        return Vec::new();
    }
    if payload.is_empty() || !payload.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Vec::new();
    }
    // The trigger table matches at most sixteen hex digits; `u32` covers a payload
    // of eight, so a parse failure on a shorter payload cannot happen and the
    // checked answer is the guard that keeps it that way.
    let Some(value) = u32::from_str_radix(payload, 16).ok() else {
        return vec![echo_candidate(payload)];
    };
    if value > MAX_SCALAR || is_surrogate(value) {
        return vec![echo_candidate(payload)];
    }
    let Some(first) = char::from_u32(value) else {
        return vec![echo_candidate(payload)];
    };
    let mut out = Vec::with_capacity(1 + SUCCESSOR_COUNT);
    out.push(candidate(first));
    // The walk counts collected successors, not steps: a run of surrogates extends
    // the walk rather than shortening the suggestion list.
    let mut next = value;
    while out.len() < 1 + SUCCESSOR_COUNT {
        let Some(candidate_value) = next.checked_add(1) else {
            break;
        };
        next = candidate_value;
        if next > MAX_SCALAR {
            break;
        }
        if is_surrogate(next) {
            continue;
        }
        let Some(character) = char::from_u32(next) else {
            continue;
        };
        out.push(candidate(character));
    }
    out
}

/// Whether `value` falls inside the surrogate block.
fn is_surrogate(value: u32) -> bool {
    (SURROGATE_FIRST..=SURROGATE_LAST).contains(&value)
}

/// The character candidate, annotated with its uppercase hex spelling.
fn candidate(character: char) -> QuickCandidate {
    QuickCandidate {
        text: String::from(character),
        annotation: Some(format!("U{:04X}", character as u32)),
    }
}

/// The echo a payload that names no scalar renders: the input the user typed, with
/// the trigger in front of it, and the stable code as the annotation.
fn echo_candidate(payload: &str) -> QuickCandidate {
    QuickCandidate {
        text: format!("U{payload}"),
        annotation: Some(String::from(INVALID_CODE)),
    }
}

#[cfg(test)]
mod tests {
    use super::translate;

    #[test]
    fn test_translate_u4f60_renders_char_with_hex_annotation() {
        let out = translate("4f60");
        assert_eq!(out.len(), 1 + 16);
        assert_eq!(out[0].text, "你");
        assert_eq!(out[0].annotation.as_deref(), Some("U4F60"));
    }

    #[test]
    fn test_translate_uppercase_hex_folds_to_the_same_character() {
        let lower = translate("4f60");
        let upper = translate("4F60");
        assert_eq!(lower, upper);
    }

    #[test]
    fn test_translate_successors_are_the_following_sixteen_scalars() {
        let out = translate("4f60");
        let successors: Vec<String> = out[1..]
            .iter()
            .map(|candidate| candidate.text.clone())
            .collect();
        let expected: Vec<String> = (0x4F61_u32..=0x4F70_u32)
            .map(|value| String::from(char::from_u32(value).unwrap_or('\0')))
            .collect();
        assert_eq!(successors, expected);
        assert_eq!(out[1].annotation.as_deref(), Some("U4F61"));
    }

    #[test]
    fn test_translate_successors_skip_the_surrogate_block() {
        // U+D7FF is the last scalar before the surrogate block, so the first
        // successor that can exist is U+E000, past the whole block.
        let out = translate("d7ff");
        assert_eq!(out.len(), 1 + 16);
        assert_eq!(
            out[0].text,
            char::from_u32(0xD7FF).unwrap_or('\0').to_string()
        );
        assert_eq!(
            out[1].text,
            char::from_u32(0xE000).unwrap_or('\0').to_string()
        );
        assert_eq!(out[1].annotation.as_deref(), Some("UE000"));
    }

    #[test]
    fn test_translate_last_scalar_has_no_successors() {
        let out = translate("10ffff");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].text,
            char::from_u32(0x10_FFFF).unwrap_or('\0').to_string()
        );
    }

    #[test]
    fn test_translate_surrogate_payload_echoes_invalid() {
        let out = translate("d800");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "Ud800");
        assert_eq!(out[0].annotation.as_deref(), Some("quick/expr/invalid"));
    }

    #[test]
    fn test_translate_value_past_plane_ten_echoes_invalid() {
        let out = translate("110000");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "U110000");
        assert_eq!(out[0].annotation.as_deref(), Some("quick/expr/invalid"));
    }

    #[test]
    fn test_translate_overlong_payload_echoes_invalid() {
        let out = translate("1234567");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "U1234567");
        assert_eq!(out[0].annotation.as_deref(), Some("quick/expr/invalid"));
    }

    #[test]
    fn test_translate_empty_or_non_hex_payload_renders_nothing() {
        assert!(translate("").is_empty());
        assert!(translate("g").is_empty());
        assert!(translate("4f6g").is_empty());
        // The trigger table never matches a payload this long; the translator
        // answers the same way if one arrives anyway.
        assert!(translate("12345678901234567").is_empty());
    }

    #[test]
    fn test_translate_lowest_codepoint_renders_the_null_character() {
        let out = translate("0");
        assert_eq!(out.len(), 17);
        assert_eq!(out[0].text, "\u{0}");
        assert_eq!(out[0].annotation.as_deref(), Some("U0000"));
    }
}
