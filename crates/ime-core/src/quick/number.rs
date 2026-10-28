//! The amount translator behind the `R` trigger: four renderings of a digit string.
//!
//! # Responsibility
//!
//! Turn the digits a user typed after the trigger into the four forms a Chinese
//! amount can take — the plain reading, the financial capitals, and the two currency
//! forms of each — as one candidate list. The rendering rules follow the accounting
//! convention the product documents: the capitals style writes a tens digit of one as
//! 壹拾 and never a bare 拾, everywhere, including inside a group and at the head of
//! the whole number.
//!
//! # Boundaries
//!
//! Pure: a function of the digit string and nothing else. Input is admitted only
//! when it is non-empty, at most [`MAX_AMOUNT_DIGITS`] ASCII digits, and nothing
//! else; anything outside that shape renders no candidate. Leading zeros carry no
//! meaning and are dropped by the numeric parse, so `R012` renders twelve.

use super::QuickCandidate;

/// The four renderings one digit string can take.
///
/// The order is the display order of the candidate list: plain first, currency last,
/// so the reading a user most often wants sits on the first key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmountStyle {
    /// The plain Chinese reading: 一千零二十四.
    Plain,
    /// The financial capitals: 壹仟零贰拾肆, with the 壹拾 rule for tens of one.
    Upper,
    /// The plain reading closed out as an integer amount: 一千零二十四元整.
    CurrencyPlain,
    /// The capitals reading closed out as an amount with the currency head:
    /// 人民币壹仟零贰拾肆元整.
    CurrencyUpper,
}

/// The most digits one amount payload may hold.
pub const MAX_AMOUNT_DIGITS: usize = 16;

/// The digit glyphs of the plain reading, indexed by value 1..=9.
const PLAIN_DIGITS: [&str; 9] = ["一", "二", "三", "四", "五", "六", "七", "八", "九"];

/// The digit glyphs of the financial capitals, indexed by value 1..=9.
const UPPER_DIGITS: [&str; 9] = ["壹", "贰", "叁", "肆", "伍", "陆", "柒", "捌", "玖"];

/// The zero glyph both readings share.
const ZERO: &str = "零";

/// The position units inside one four-digit group, from the thousands down.
const PLAIN_GROUP_UNITS: [&str; 3] = ["千", "百", "十"];
/// The position units inside one four-digit group, capitals reading.
const UPPER_GROUP_UNITS: [&str; 3] = ["仟", "佰", "拾"];

/// The group units from the lowest group up: ones, 万, 亿, 万亿.
const GROUP_NAMES: [&str; 4] = ["", "万", "亿", "万亿"];

/// The closing an integer amount carries in the currency readings.
const CURRENCY_TAIL: &str = "元整";

/// The head the capitals currency reading carries.
const CURRENCY_HEAD: &str = "人民币";

/// Renders `digits` in one style, or `None` when the input is not an admissible
/// amount.
///
/// # Arguments
///
/// * `digits` - the payload after the trigger, expected to be ASCII digits only.
/// * `style` - which of the four renderings to produce.
///
/// # Returns
///
/// The rendered amount. `None` when the input is empty, holds a character that is
/// not an ASCII digit (a sign included: the domain has no negatives to name), or
/// exceeds [`MAX_AMOUNT_DIGITS`] digits.
///
/// # Panics
///
/// Never: the digit walk reads slices it sized itself and the numeric parse is
/// checked.
pub fn render_amount(digits: &str, style: AmountStyle) -> Option<String> {
    if digits.is_empty() || digits.len() > MAX_AMOUNT_DIGITS {
        return None;
    }
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // Sixteen digits reach just under 10^16, far inside `u64`; a parse failure is
    // therefore unreachable for admitted input, and the checked conversion is the
    // guard that keeps it that way.
    let value: u64 = digits.parse().ok()?;
    let upper = matches!(style, AmountStyle::Upper | AmountStyle::CurrencyUpper);
    let mut rendered = chinese_numeral(value, upper);
    match style {
        AmountStyle::Plain | AmountStyle::Upper => {}
        AmountStyle::CurrencyPlain | AmountStyle::CurrencyUpper => rendered.push_str(CURRENCY_TAIL),
    }
    if matches!(style, AmountStyle::CurrencyUpper) {
        rendered.insert_str(0, CURRENCY_HEAD);
    }
    Some(rendered)
}

/// Renders the four candidates the amount panel offers, in display order.
///
/// # Arguments
///
/// * `payload` - the digits after the trigger.
///
/// # Returns
///
/// One candidate per [`AmountStyle`], best first. An inadmissible payload renders an
/// empty list: the panel simply has nothing to show, exactly as a domain with no
/// snapshot has nothing to show.
///
/// # Panics
///
/// Never.
pub fn translate(payload: &str) -> Vec<QuickCandidate> {
    let styles = [
        (AmountStyle::Plain, "数字"),
        (AmountStyle::Upper, "大写"),
        (AmountStyle::CurrencyPlain, "金额"),
        (AmountStyle::CurrencyUpper, "大写金额"),
    ];
    let mut out = Vec::new();
    for (style, label) in styles {
        let Some(text) = render_amount(payload, style) else {
            continue;
        };
        out.push(QuickCandidate {
            text,
            annotation: Some(String::from(label)),
        });
    }
    out
}

/// Renders a non-negative number in the plain or capitals reading.
///
/// The number is read group by group: each four-digit group renders through
/// [`render_group`] and carries its group unit. A skipped group leaves a zero mark
/// that renders as one 零 before the next non-empty group, and a group whose value is
/// under one thousand opens with 零 for the same reason — the zeros the group's own
/// digits do not spell.
fn chinese_numeral(value: u64, upper: bool) -> String {
    if value == 0 {
        return String::from(ZERO);
    }
    let mut groups = [0_u64; 4];
    let mut rest = value;
    for slot in groups.iter_mut() {
        *slot = rest % 10_000;
        rest /= 10_000;
    }
    // The highest non-empty group is where the reading starts; groups above it are
    // structural zeros no reading spells.
    let highest = (0..4)
        .rev()
        .find(|index| groups[*index] != 0)
        .unwrap_or_default();
    let mut out = String::new();
    let mut need_zero = false;
    let mut rendered_any = false;
    for index in (0..=highest).rev() {
        let group = groups[index];
        if group == 0 {
            need_zero = true;
            continue;
        }
        if rendered_any && (need_zero || group < 1_000) {
            out.push_str(ZERO);
        }
        // The plain reading drops the leading one of a bare ten — 十二, never 一十二
        // — but only when that ten is the head of the whole number. The capitals
        // reading keeps its 壹 everywhere; that is the accounting rule.
        let bare_ten = !upper && index == highest && (10..20).contains(&group);
        render_group(group, upper, bare_ten, &mut out);
        out.push_str(GROUP_NAMES[index]);
        rendered_any = true;
        need_zero = false;
    }
    out
}

/// Renders one group of `1..=9999` into `out`, digits high to low with a zero mark
/// between every gap the digits skip.
fn render_group(group: u64, upper: bool, bare_ten: bool, out: &mut String) {
    let digits = if upper { &UPPER_DIGITS } else { &PLAIN_DIGITS };
    let units = if upper { &UPPER_GROUP_UNITS } else { &PLAIN_GROUP_UNITS };
    let mut pending_zero = false;
    // The group's own leading zeros spell nothing: until one digit has rendered,
    // a zero is a position the group does not name, not a gap between two that
    // it does.
    let mut group_started = false;
    for position in (0..4_usize).rev() {
        let digit = (group / pow10(position)) % 10;
        if digit == 0 {
            if group_started {
                pending_zero = true;
            }
            continue;
        }
        if pending_zero {
            out.push_str(ZERO);
            pending_zero = false;
        }
        // A tens digit of one under the plain reading drops its glyph when the ten
        // leads the whole number; every other position and the whole capitals
        // reading keep the glyph. The unit of a position reads from the high end
        // of the table: thousands first.
        let unit = if position == 0 { "" } else { units[3 - position] };
        if !(bare_ten && position == 1 && digit == 1) {
            // `digit` is 1..=9 here — zero took the early continue — so the index
            // is 0..=8 and the subtraction cannot underflow.
            let index = usize::try_from(digit)
                .unwrap_or(1)
                .saturating_sub(1);
            if let Some(glyph) = digits.get(index) {
                out.push_str(glyph);
            }
        }
        out.push_str(unit);
        group_started = true;
    }
}

/// `10^position`, for the three positions one group reads.
fn pow10(position: usize) -> u64 {
    match position {
        0 => 1,
        1 => 10,
        2 => 100,
        _ => 1_000,
    }
}

#[cfg(test)]
mod tests {
    use super::{AmountStyle, render_amount};

    #[test]
    fn test_render_amount_plain_basic_readings() {
        assert_eq!(
            render_amount("12", AmountStyle::Plain).as_deref(),
            Some("十二")
        );
        assert_eq!(
            render_amount("1024", AmountStyle::Plain).as_deref(),
            Some("一千零二十四")
        );
        assert_eq!(
            render_amount("110", AmountStyle::Plain).as_deref(),
            Some("一百一十")
        );
        assert_eq!(
            render_amount("123456", AmountStyle::Plain).as_deref(),
            Some("十二万三千四百五十六")
        );
    }

    #[test]
    fn test_render_amount_upper_starts_with_shi_for_ten_to_nineteen() {
        // The accounting rule the card pins: 10..19 opens with 壹拾, never a bare 拾.
        assert_eq!(
            render_amount("10", AmountStyle::Upper).as_deref(),
            Some("壹拾")
        );
        assert_eq!(
            render_amount("12", AmountStyle::Upper).as_deref(),
            Some("壹拾贰")
        );
        assert_eq!(
            render_amount("19", AmountStyle::Upper).as_deref(),
            Some("壹拾玖")
        );
        // The rule holds inside a group as well: 110 is 壹佰壹拾.
        assert_eq!(
            render_amount("110", AmountStyle::Upper).as_deref(),
            Some("壹佰壹拾")
        );
        // And at the head of a larger number: 100010 is 壹拾万零壹拾.
        assert_eq!(
            render_amount("100010", AmountStyle::Upper).as_deref(),
            Some("壹拾万零壹拾")
        );
    }

    #[test]
    fn test_render_amount_currency_styles_close_out_the_amount() {
        assert_eq!(
            render_amount("1024", AmountStyle::CurrencyPlain).as_deref(),
            Some("一千零二十四元整")
        );
        assert_eq!(
            render_amount("1024", AmountStyle::CurrencyUpper).as_deref(),
            Some("人民币壹仟零贰拾肆元整")
        );
    }

    #[test]
    fn test_render_amount_zero_renders_ling_in_every_style() {
        assert_eq!(render_amount("0", AmountStyle::Plain).as_deref(), Some("零"));
        assert_eq!(
            render_amount("0", AmountStyle::CurrencyUpper).as_deref(),
            Some("人民币零元整")
        );
        // Leading zeros carry no value and change nothing.
        assert_eq!(
            render_amount("000012", AmountStyle::Plain).as_deref(),
            Some("十二")
        );
    }

    #[test]
    fn test_render_amount_rejects_non_digits_negatives_and_empty() {
        assert_eq!(render_amount("", AmountStyle::Plain), None);
        assert_eq!(render_amount("12a", AmountStyle::Plain), None);
        assert_eq!(render_amount("-5", AmountStyle::Plain), None);
        assert_eq!(render_amount("1 2", AmountStyle::Plain), None);
    }

    #[test]
    fn test_render_amount_rejects_payloads_past_the_digit_cap() {
        let over = "9".repeat(17);
        assert_eq!(render_amount(&over, AmountStyle::Plain), None);
        let at_cap = "9".repeat(16);
        assert!(render_amount(&at_cap, AmountStyle::Plain).is_some());
    }

    #[test]
    fn test_render_amount_group_zero_rules() {
        assert_eq!(
            render_amount("105", AmountStyle::Plain).as_deref(),
            Some("一百零五")
        );
        assert_eq!(
            render_amount("1005", AmountStyle::Plain).as_deref(),
            Some("一千零五")
        );
        assert_eq!(
            render_amount("10000", AmountStyle::Plain).as_deref(),
            Some("一万")
        );
        assert_eq!(
            render_amount("100000", AmountStyle::Plain).as_deref(),
            Some("十万")
        );
        assert_eq!(
            render_amount("100000000", AmountStyle::Plain).as_deref(),
            Some("一亿")
        );
        assert_eq!(
            render_amount("100000001", AmountStyle::Plain).as_deref(),
            Some("一亿零一")
        );
        assert_eq!(
            render_amount("100010000", AmountStyle::Plain).as_deref(),
            Some("一亿零一万")
        );
        assert_eq!(
            render_amount("1000000000000", AmountStyle::Plain).as_deref(),
            Some("一万亿")
        );
        assert_eq!(
            render_amount("10000000000", AmountStyle::Plain).as_deref(),
            Some("一百亿")
        );
    }

    #[test]
    fn test_render_amount_sixteen_digits_renders_four_groups() {
        let full = "9999999999999999";
        assert_eq!(
            render_amount(full, AmountStyle::Plain).as_deref(),
            Some("九千九百九十九万亿九千九百九十九亿九千九百九十九万九千九百九十九")
        );
    }
}
