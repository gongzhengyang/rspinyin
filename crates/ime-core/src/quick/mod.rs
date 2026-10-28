//! Pure recognition and rendering for the quick-input trigger table.
//!
//! # Responsibility
//!
//! This module decides whether a raw buffer is asking for a quick-input domain and,
//! when it does, renders the candidates the domain answers with. Seven translators
//! share the entry point: the date family (`rq`/`sj`/`xq`/`ts`/`dt` spellings), the
//! amount renderer behind the `R` trigger, the codepoint renderer behind `U`, and the
//! calculator behind `cC`. Keeping recognition and rendering here lets the routing
//! layer plug the panel beside the state machine without making the decoder depend on
//! configuration or time.
//!
//! # Boundaries
//!
//! Everything here is pure (0.4 rule 4): no file, no clock, no environment, no global
//! state. Time-domain output derives exclusively from the [`CivilTime`] value the
//! caller passes; a caller with no snapshot gets no time-domain candidates, which is
//! the documented degradation rather than an error. The calculator evaluates through
//! `meval` behind a length and character-whitelist gate, so a malformed expression is
//! answered with an echo candidate carrying the stable `quick/expr/invalid` code
//! instead of a panic.
//!
//! Recognition is whole-buffer: a trigger is complete only when the entire spelling
//! (and, for the payload domains, the whole payload) has been typed. An unmatched
//! buffer costs a handful of byte comparisons and no allocation; only a hit renders,
//! and a render allocates the candidate list it returns.

/// The quick-input domains the trigger layer can recognise.
///
/// The calculator is a separate trigger because its admission is configured
/// independently of the date-family prefixes. The amount and codepoint triggers are
/// fixed spellings (`R` and `U`) that the configuration does not rename.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuickDomain {
    /// One of the configured date-family trigger spellings was matched.
    Date,
    /// The calculator trigger was matched and the calculator is enabled.
    Calc,
    /// The amount trigger (`R` followed by digits) was matched.
    Number,
    /// The codepoint trigger (`U` followed by hex digits) was matched.
    Codepoint,
}

/// A trigger decision: which quick-input domain the buffer selects.
///
/// `trigger_len` counts the bytes of the trigger itself, so a caller can name the
/// payload the user typed after it. Matching performs no allocation on a miss.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuickMatch {
    /// The domain the matched trigger selects.
    pub domain: QuickDomain,
    /// The number of bytes in the matched trigger spelling.
    pub trigger_len: usize,
}

/// One candidate a quick-input domain renders.
///
/// The text is what a selection commits; the annotation is the grey right-hand hint
/// the candidate window already renders. An annotation may carry a stable error code
/// (`quick/expr/invalid`) when the domain could not render what was asked, in which
/// case the text echoes the input the user typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickCandidate {
    /// The text a selection commits.
    pub text: String,
    /// The annotation the candidate window shows beside the text.
    pub annotation: Option<String>,
}

/// A civil-time value the time-domain translators render from.
///
/// The engine never reads the clock: the host injects one of these per render, which
/// keeps every output deterministic under test and the hot path free of hidden system
/// calls. Field ranges follow the host's own snapshot type: month `1..=12`, day
/// `1..=31` as valid for the month, hour `0..=23`, minute and second `0..=59`,
/// weekday zero-based from Monday.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CivilTime {
    /// The Gregorian year.
    pub year: i32,
    /// The month, `1..=12`.
    pub month: u8,
    /// The day of month, `1..=31`.
    pub day: u8,
    /// The hour, `0..=23`.
    pub hour: u8,
    /// The minute, `0..=59`.
    pub minute: u8,
    /// The second, `0..=59`.
    pub second: u8,
    /// The weekday, with Monday represented by zero.
    pub weekday: u8,
}

/// The most hex digits a codepoint payload may hold before the trigger stops matching.
///
/// Six hex digits cover every Unicode scalar; the echo rendering of an over-long
/// payload is still matched up to this bound so the panel can say why nothing
/// rendered, and past it the buffer is left to the ordinary decode.
const MAX_CODEPOINT_ECHO_DIGITS: usize = 16;

/// Returns the quick-input domain selected by `buffer`, if any.
///
/// The lookup is whole-buffer: a date-family trigger must equal a configured spelling
/// in full, the calculator must name its trigger plus a non-empty admissible
/// expression, and the amount and codepoint triggers must carry a payload of at least
/// one digit. A bare `cc`/`cC` is deliberately *not* a match: with nothing to
/// evaluate the calculator has nothing to offer, so the buffer keeps composing as
/// ordinary (dead) input until a digit arrives. This keeps the function deterministic
/// and allocation-free, and makes an unmatched buffer cost at most one byte
/// comparison per configured spelling.
///
/// # Arguments
///
/// * `enabled` - the quick-input master switch.
/// * `buffer` - the raw spelling currently in the input buffer.
/// * `date_prefixes` - the enabled date-family trigger spellings, in configuration
///   order. Empty entries are skipped.
/// * `calc_enabled` - whether the calculator trigger may be recognised.
///
/// # Returns
///
/// `Some` with the first matching trigger when quick input is enabled and the buffer
/// names an active trigger; `None` otherwise. The date family wins over the fixed
/// spellings, so a user who binds `cc` as a date prefix gets the date domain.
///
/// # Panics
///
/// Never.
pub fn recognize(
    enabled: bool,
    buffer: &str,
    date_prefixes: &[&str],
    calc_enabled: bool,
) -> Option<QuickMatch> {
    if !enabled {
        return None;
    }
    for prefix in date_prefixes {
        if prefix.is_empty() {
            continue;
        }
        if buffer == *prefix {
            return Some(QuickMatch {
                domain: QuickDomain::Date,
                trigger_len: prefix.len(),
            });
        }
    }
    // The amount and codepoint triggers accept either case of their letter. The
    // capital is the documented spelling, but the routing table folds a shifted
    // letter before the buffer ever sees it and gives a bare uppercase keysym no
    // row at all, so on a real keyboard the lowercase form is the one a user can
    // type. Refusing it would make the domain unreachable; accepting both costs
    // one comparison.
    if let Some(rest) = buffer
        .strip_prefix('R')
        .or_else(|| buffer.strip_prefix('r'))
    {
        if is_digit_payload(rest, number::MAX_AMOUNT_DIGITS) {
            return Some(QuickMatch {
                domain: QuickDomain::Number,
                trigger_len: 1,
            });
        }
    }
    if let Some(rest) = buffer
        .strip_prefix('U')
        .or_else(|| buffer.strip_prefix('u'))
    {
        // Hex case does not matter for the match; the renderer folds it.
        if !rest.is_empty()
            && rest.len() <= MAX_CODEPOINT_ECHO_DIGITS
            && rest.chars().all(|ch| ch.is_ascii_hexdigit())
        {
            return Some(QuickMatch {
                domain: QuickDomain::Codepoint,
                trigger_len: 1,
            });
        }
    }
    if calc_enabled {
        if let Some(expr) = calc_expression(buffer) {
            if is_admissible_expression(expr) {
                return Some(QuickMatch {
                    domain: QuickDomain::Calc,
                    trigger_len: 2,
                });
            }
        }
    }
    None
}

/// Renders the candidate list one matched trigger answers with.
///
/// The date family consumes the snapshot; a `None` snapshot renders nothing, which is
/// the documented degradation when the host supplies no clock. The other domains
/// ignore the snapshot.
///
/// # Arguments
///
/// * `matched` - the trigger [`recognize`] returned.
/// * `buffer` - the same raw buffer the match came from.
/// * `clock` - the civil time the date family renders, when the host supplied one.
///
/// # Returns
///
/// The candidates in display order, best first, at most seventeen for the codepoint
/// domain (the character itself plus its successors) and at most eight for every
/// other domain. An empty list means the domain has nothing to offer this time.
///
/// # Panics
///
/// Never.
pub fn candidates(
    matched: QuickMatch,
    buffer: &str,
    clock: Option<&CivilTime>,
) -> Vec<QuickCandidate> {
    match matched.domain {
        QuickDomain::Date => match clock {
            Some(time) => date::translate(buffer, time),
            None => Vec::new(),
        },
        QuickDomain::Number => {
            let payload = buffer
                .get(matched.trigger_len..)
                .unwrap_or_default();
            number::translate(payload)
        }
        QuickDomain::Codepoint => {
            let payload = buffer
                .get(matched.trigger_len..)
                .unwrap_or_default();
            codepoint::translate(payload)
        }
        QuickDomain::Calc => {
            let expr = calc_expression(buffer).unwrap_or_default();
            calc::translate(expr)
        }
    }
}

/// Splits a calculator buffer into its trigger and expression halves.
///
/// Both trigger spellings name the same domain: the routing table folds a shifted
/// letter before the buffer ever sees it, so the uppercase form is reachable only on
/// a frontend that preserves bare uppercase (a CapsLock press), while the lowercase
/// form is what a plain keyboard types. Accepting both costs one comparison and
/// keeps the documented trigger usable either way.
fn calc_expression(buffer: &str) -> Option<&str> {
    buffer
        .strip_prefix("cC")
        .or_else(|| buffer.strip_prefix("cc"))
}

/// Whether `payload` is `1..=cap` ASCII digits.
fn is_digit_payload(payload: &str, cap: usize) -> bool {
    !payload.is_empty()
        && payload.len() <= cap
        && payload.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether an expression passes the calculator's admission gate: short enough to
/// evaluate and made of nothing but the arithmetic characters the whitelist names.
///
/// Letters are excluded on purpose: `meval` would read them as function or variable
/// names, and a quick-input panel that evaluates `system(...)` is not a panel this
/// product ships.
fn is_admissible_expression(expr: &str) -> bool {
    if expr.is_empty() || expr.len() > calc::MAX_EXPRESSION_LEN {
        return false;
    }
    expr.chars().all(|ch| {
        matches!(
            ch,
            '0'..='9' | '+' | '-' | '*' | '/' | '(' | ')' | '.'
        )
    })
}

pub mod calc;
pub mod codepoint;
pub mod date;
pub mod number;

#[cfg(test)]
mod tests {
    use super::{CivilTime, QuickDomain, recognize};

    /// The snapshot the date-family tests pin their formats against.
    pub(super) fn pinned_time() -> CivilTime {
        CivilTime {
            year: 2026,
            month: 10,
            day: 28,
            hour: 12,
            minute: 34,
            second: 56,
            weekday: 2,
        }
    }

    #[test]
    fn test_recognize_disabled_returns_no_match() {
        assert_eq!(recognize(false, "rq", &["rq"], true), None);
    }

    #[test]
    fn test_recognize_date_prefix_requires_whole_trigger() {
        assert!(recognize(true, "rq", &["rq", "dt"], true).is_some());
        assert!(recognize(true, "rqx", &["rq", "dt"], true).is_none());
        assert!(recognize(true, "", &["rq", "dt"], true).is_none());
    }

    #[test]
    fn test_recognize_calc_only_when_enabled() {
        // A bare trigger is pending, not active: with no expression there is
        // nothing to evaluate, so neither spelling matches until a digit follows.
        assert_eq!(recognize(true, "cC", &[], true), None);
        assert_eq!(recognize(true, "cc", &[], true), None);
        assert_eq!(
            recognize(true, "cc1", &[], false),
            None,
            "the calculator switch gates the domain"
        );
        assert_eq!(
            recognize(true, "cc1", &[], true).map(|found| found.domain),
            Some(QuickDomain::Calc)
        );
        assert_eq!(
            recognize(true, "cC1+2", &[], true).map(|found| found.domain),
            Some(QuickDomain::Calc)
        );
    }

    #[test]
    fn test_recognize_first_date_prefix_wins() {
        let matched = recognize(true, "dt", &["dt", "rq"], true);
        assert_eq!(matched.map(|found| found.trigger_len), Some(2));
    }

    #[test]
    fn test_recognize_number_trigger_needs_digits() {
        assert_eq!(recognize(true, "R", &[], true), None);
        assert_eq!(
            recognize(true, "R12", &[], true).map(|found| found.domain),
            Some(QuickDomain::Number)
        );
        // The lowercase spelling is the one a keyboard can type: the routing table
        // folds a shifted letter before the buffer sees it.
        assert_eq!(
            recognize(true, "r12", &[], true).map(|found| found.domain),
            Some(QuickDomain::Number)
        );
    }

    #[test]
    fn test_recognize_number_payload_rejects_non_digits_and_overrun() {
        assert_eq!(recognize(true, "R1a", &[], true), None);
        assert_eq!(recognize(true, "R-5", &[], true), None);
        let over = format!("R{}", "9".repeat(17));
        assert_eq!(recognize(true, &over, &[], true), None);
        let at_cap = format!("R{}", "9".repeat(16));
        assert!(recognize(true, &at_cap, &[], true).is_some());
    }

    #[test]
    fn test_recognize_codepoint_hex_bounds() {
        assert_eq!(recognize(true, "U", &[], true), None);
        assert_eq!(
            recognize(true, "U4f60", &[], true).map(|found| found.domain),
            Some(QuickDomain::Codepoint)
        );
        assert_eq!(
            recognize(true, "u4f60", &[], true).map(|found| found.domain),
            Some(QuickDomain::Codepoint)
        );
        // Seven digits still match so the panel can echo why they render nothing;
        // past the echo bound the buffer is ordinary input again.
        assert!(recognize(true, "U1234567", &[], true).is_some());
        assert_eq!(recognize(true, "U12345678901234567", &[], true), None);
        assert_eq!(recognize(true, "U4g60", &[], true), None);
    }

    #[test]
    fn test_recognize_empty_date_entries_are_skipped() {
        assert_eq!(recognize(true, "", &[""], true), None);
        assert_eq!(
            recognize(true, "rq", &["", "rq"], true).map(|found| found.domain),
            Some(QuickDomain::Date)
        );
    }
}
