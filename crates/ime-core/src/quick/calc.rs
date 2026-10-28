//! The calculator translator behind the `cC` trigger: evaluate one expression.
//!
//! # Responsibility
//!
//! Evaluate the arithmetic expression a user typed after the trigger and render the
//! two candidates the panel serves: the bare value, and the value spelled back with
//! its expression. An expression that cannot be evaluated — a parse failure, a
//! division by zero, anything past the admission gate — renders a single candidate
//! echoing the input with the stable `quick/expr/invalid` code, which is the whole of
//! the failure surface: no panic, no session teardown, no diagnostic storm.
//!
//! # The safety gate
//!
//! Evaluation runs through `meval`, a pure parser and evaluator with no I/O and no
//! execution. Before anything reaches it, the expression passes two checks: it is at
//! most [`MAX_EXPRESSION_LEN`] bytes, and every character is one of the arithmetic
//! whitelist. The whitelist excludes letters on purpose — `meval` would read them as
//! function or variable names, and a panel that evaluates `system(...)` is not a
//! panel this product ships.
//!
//! # Boundaries
//!
//! Pure: a function of the expression string. The result is an `f64` by nature of
//! the evaluator; the renderer turns it into text with a fixed rule so the same
//! expression always renders the same candidates.

use super::QuickCandidate;

/// The most bytes an expression may hold before the gate refuses it.
///
/// Sixty-four bytes is several times the longest expression a candidate row can
/// show, which is the point: past this bound the panel says so instead of growing.
pub const MAX_EXPRESSION_LEN: usize = 64;

/// The annotation code an expression that cannot evaluate echoes with.
const INVALID_CODE: &str = "quick/expr/invalid";

/// The annotation the successful candidates carry.
const CALC_LABEL: &str = "计算";

/// Renders the candidates one expression answers with.
///
/// # Arguments
///
/// * `expr` - the expression after the trigger, without the trigger itself.
///
/// # Returns
///
/// Two candidates on success — the value, then `expr=value` — and one echo candidate
/// with the invalid annotation when the expression cannot evaluate. An empty
/// expression renders an empty list: with nothing typed there is nothing to answer.
///
/// # Panics
///
/// Never: the admission gate bounds the input before the evaluator sees it, and the
/// numeric conversions are range-checked.
pub fn translate(expr: &str) -> Vec<QuickCandidate> {
    if expr.is_empty() {
        return Vec::new();
    }
    let value = match evaluate(expr) {
        Some(value) => value,
        None => {
            return vec![QuickCandidate {
                text: String::from(expr),
                annotation: Some(String::from(INVALID_CODE)),
            }];
        }
    };
    let rendered = format_value(value);
    vec![
        QuickCandidate {
            text: rendered.clone(),
            annotation: Some(String::from(CALC_LABEL)),
        },
        QuickCandidate {
            text: format!("{expr}={rendered}"),
            annotation: Some(String::from(CALC_LABEL)),
        },
    ]
}

/// Gates and evaluates one expression, or `None` when it cannot evaluate.
fn evaluate(expr: &str) -> Option<f64> {
    if expr.len() > MAX_EXPRESSION_LEN {
        return None;
    }
    if !expr.chars().all(|ch| {
        matches!(ch, '0'..='9' | '+' | '-' | '*' | '/' | '(' | ')' | '.')
    }) {
        return None;
    }
    let value = meval::eval_str(expr).ok()?;
    if value.is_finite() {
        Some(value)
    } else {
        // A division by zero reaches here as an infinity, and a zero-over-zero as
        // a NaN; both are "this expression has no answer", not a value.
        None
    }
}

/// The fixed rendering rule for one evaluated value: integers as integers, anything
/// else to ten decimal places with the trailing zeros trimmed.
fn format_value(value: f64) -> String {
    // An integral result inside the exact range of `i64` reads as the integer it
    // is; the range check is what keeps the `as` conversion honest.
    if value.fract() == 0.0
        && value >= i64::MIN as f64
        && value <= i64::MAX as f64
    {
        return format!("{}", value as i64);
    }
    let mut rendered = format!("{value:.10}");
    if rendered.ends_with('0') {
        while rendered.ends_with('0') {
            rendered.pop();
        }
        if rendered.ends_with('.') {
            rendered.pop();
        }
    }
    // A value smaller than the tenth place renders as a bare zero above; the
    // shortest round-trip form is the honest answer for that band.
    if (rendered == "0" || rendered == "-0") && value != 0.0 {
        return format!("{value}");
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::{MAX_EXPRESSION_LEN, translate};

    #[test]
    fn test_translate_integer_expression_renders_dual_candidates() {
        let out = translate("1+2");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "3");
        assert_eq!(out[1].text, "1+2=3");
        assert_eq!(out[0].annotation.as_deref(), Some("计算"));
    }

    #[test]
    fn test_translate_division_by_zero_renders_invalid_echo() {
        let out = translate("1/0");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "1/0");
        assert_eq!(out[0].annotation.as_deref(), Some("quick/expr/invalid"));
    }

    #[test]
    fn test_translate_parse_failure_renders_invalid_echo() {
        for broken in ["1++", "*3", "1+", "((1"] {
            let out = translate(broken);
            assert_eq!(out.len(), 1, "{broken}");
            assert_eq!(out[0].text, broken);
            assert_eq!(out[0].annotation.as_deref(), Some("quick/expr/invalid"));
        }
    }

    #[test]
    fn test_translate_characters_outside_the_whitelist_render_invalid_echo() {
        let out = translate("1;a");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "1;a");
        assert_eq!(out[0].annotation.as_deref(), Some("quick/expr/invalid"));
    }

    #[test]
    fn test_translate_overlong_expression_renders_invalid_echo() {
        // Sixty-four bytes of sums evaluate; two more do not.
        let at_cap: String = "1+".repeat(31) + "11";
        assert_eq!(at_cap.len(), MAX_EXPRESSION_LEN);
        let ok = translate(&at_cap);
        assert_eq!(ok.len(), 2, "the cap itself still evaluates");
        let over = format!("{at_cap}+1");
        assert_eq!(over.len(), MAX_EXPRESSION_LEN + 2);
        let refused = translate(&over);
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].annotation.as_deref(), Some("quick/expr/invalid"));
    }

    #[test]
    fn test_translate_fractional_values_render_ten_places_trimmed() {
        assert_eq!(translate("1/4")[0].text, "0.25");
        assert_eq!(translate("2/3")[0].text, "0.6666666667");
    }

    #[test]
    fn test_translate_unary_minus_and_parentheses_evaluate() {
        assert_eq!(translate("-(1+2)")[0].text, "-3");
        assert_eq!(translate("(1+2)*3")[0].text, "9");
        assert_eq!(translate("1-1")[0].text, "0");
        assert_eq!(translate("10/4")[0].text, "2.5");
    }

    #[test]
    fn test_translate_empty_expression_renders_nothing() {
        assert!(translate("").is_empty());
    }
}
