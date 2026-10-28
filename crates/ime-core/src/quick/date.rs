//! The date-family translator: the `rq`/`sj`/`xq`/`ts`/`dt` spellings and their
//! formats.
//!
//! # Responsibility
//!
//! Render civil time into the candidate lists the date-family triggers serve: four
//! date formats under `rq`, four time formats (one carrying the Chinese day period)
//! under `sj`, three weekday formats under `xq`, one timestamp under `ts`, and the
//! two ISO 8601 renderings under `dt`. Which spellings are live is the
//! configuration's decision; this module only answers for the spellings it knows.
//!
//! # Boundaries
//!
//! Pure: every field the formats use comes from the [`CivilTime`] value the caller
//! passes, never from a clock. A snapshot whose fields fall outside the ranges the
//! host boundary documents (a month of zero, a day past the month) renders the
//! candidates it can and skips the ones it cannot, rather than guessing; the numeric
//! formats are written from the raw fields, the Chinese forms demand the ranges.

use super::{CivilTime, QuickCandidate};

/// The glyph the Chinese year form reads each decimal digit as.
const YEAR_DIGITS: [&str; 10] = ["〇", "一", "二", "三", "四", "五", "六", "七", "八", "九"];

/// The Chinese names of the months, indexed by month 1..=12 (slot zero unused).
const MONTH_NAMES: [&str; 13] = [
    "", "一", "二", "三", "四", "五", "六", "七", "八", "九", "十", "十一", "十二",
];

/// The weekday names of the three formats, indexed with Monday at zero.
const WEEKDAY_FULL: [&str; 7] = [
    "星期一", "星期二", "星期三", "星期四", "星期五", "星期六", "星期日",
];
/// The short weekday names, same indexing.
const WEEKDAY_SHORT: [&str; 7] = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
/// The colloquial weekday names, same indexing.
const WEEKDAY_COLLOQUIAL: [&str; 7] = [
    "礼拜一", "礼拜二", "礼拜三", "礼拜四", "礼拜五", "礼拜六", "礼拜日",
];

/// Renders the candidate list one date-family trigger answers with.
///
/// # Arguments
///
/// * `trigger` - the whole buffer, which the trigger table already matched against a
///   configured spelling. Only the five spellings this module knows render.
/// * `time` - the civil time every field is read from.
///
/// # Returns
///
/// The candidates in display order, best first. An unknown trigger, or a snapshot
/// whose fields are outside the ranges the Chinese forms need, renders an empty
/// list.
///
/// # Panics
///
/// Never.
pub fn translate(trigger: &str, time: &CivilTime) -> Vec<QuickCandidate> {
    match trigger {
        "rq" => date_candidates(time),
        "sj" => time_candidates(time),
        "xq" => weekday_candidates(time),
        "ts" => vec![plain_candidate(timestamp(time), "时间戳")],
        "dt" => iso_candidates(time),
        _ => Vec::new(),
    }
}

/// The four date formats: numeric with dashes, numeric Chinese, Chinese capitals,
/// and the month-day short form.
fn date_candidates(time: &CivilTime) -> Vec<QuickCandidate> {
    let numeric = format!("{:04}-{:02}-{:02}", time.year, time.month, time.day);
    let mut out = vec![plain_candidate(numeric, "日期")];
    if let Some(chinese) = chinese_date(time) {
        out.push(plain_candidate(
            format!("{:04}年{}月{}日", time.year, time.month, time.day),
            "日期",
        ));
        out.push(plain_candidate(chinese, "日期"));
        // The month-day short form stays numeric, the reading a message usually
        // wants when the year is understood.
        out.push(plain_candidate(
            format!("{}月{}日", time.month, time.day),
            "日期",
        ));
    }
    out
}

/// The four time formats: hour-minute, hour-minute-second, the Chinese reading
/// carrying the day period, and the day period with a padded twelve-hour clock.
fn time_candidates(time: &CivilTime) -> Vec<QuickCandidate> {
    let mut out = vec![
        plain_candidate(format!("{:02}:{:02}", time.hour, time.minute), "时间"),
        plain_candidate(
            format!("{:02}:{:02}:{:02}", time.hour, time.minute, time.second),
            "时间",
        ),
    ];
    let period = day_period(time.hour);
    let twelve = twelve_hour(time.hour);
    out.push(plain_candidate(
        format!("{period}{twelve}时{}分", time.minute),
        "时间",
    ));
    out.push(plain_candidate(
        format!("{period}{twelve:02}:{:02}", time.minute),
        "时间",
    ));
    out
}

/// The three weekday formats: full, short, colloquial.
fn weekday_candidates(time: &CivilTime) -> Vec<QuickCandidate> {
    // The host boundary documents Monday as zero through Sunday as six; a value
    // outside that range is a malformed snapshot and renders nothing rather than a
    // wrong day.
    let index = usize::from(time.weekday);
    let Some(full) = WEEKDAY_FULL.get(index) else {
        return Vec::new();
    };
    let Some(short) = WEEKDAY_SHORT.get(index) else {
        return Vec::new();
    };
    let Some(colloquial) = WEEKDAY_COLLOQUIAL.get(index) else {
        return Vec::new();
    };
    vec![
        plain_candidate(*full, "星期"),
        plain_candidate(*short, "星期"),
        plain_candidate(*colloquial, "星期"),
    ]
}

/// The two ISO 8601 renderings: the `Z` designator and the explicit `+00:00` offset.
///
/// The snapshot the host supplies is a UTC reading, so both spellings name the same
/// instant; the offset form exists for the applications that want the offset spelled
/// out.
fn iso_candidates(time: &CivilTime) -> Vec<QuickCandidate> {
    let head = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        time.year, time.month, time.day, time.hour, time.minute, time.second
    );
    vec![
        plain_candidate(format!("{head}Z"), "UTC"),
        plain_candidate(format!("{head}+00:00"), "UTC"),
    ]
}

/// The timestamp rendering: numeric date, a space, and the full clock.
fn timestamp(time: &CivilTime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        time.year, time.month, time.day, time.hour, time.minute, time.second
    )
}

/// The Chinese capitals date: year digit by digit, month and day as readings.
///
/// `None` when the snapshot's fields cannot back the form — a non-positive year, a
/// month outside `1..=12`, or a day outside `1..=31`.
fn chinese_date(time: &CivilTime) -> Option<String> {
    if time.year < 1 || time.month == 0 || time.month > 12 || time.day == 0 || time.day > 31 {
        return None;
    }
    let mut year = String::new();
    for digit in format!("{}", time.year).chars() {
        let index = digit.to_digit(10)?;
        year.push_str(YEAR_DIGITS[usize::try_from(index).ok()?]);
    }
    Some(format!(
        "{year}年{}月{}日",
        chinese_month(time.month),
        chinese_day(time.day)
    ))
}

/// The Chinese month name, `1..=12` already validated by the caller.
fn chinese_month(month: u8) -> &'static str {
    MONTH_NAMES
        .get(usize::from(month))
        .unwrap_or(&MONTH_NAMES[0])
}

/// The Chinese day-of-month reading, `1..=31`.
fn chinese_day(day: u8) -> String {
    small_numeral(u32::from(day))
}

/// Reads `1..=99` the way dates read their day: a bare ten, compounds of ten, and
/// the tens otherwise.
fn small_numeral(value: u32) -> String {
    let name = |digit: u32| {
        let index = usize::try_from(digit).unwrap_or(0);
        String::from(*YEAR_DIGITS.get(index).unwrap_or(&YEAR_DIGITS[0]))
    };
    match value {
        0 => String::new(),
        1..=10 => {
            if value == 10 {
                String::from("十")
            } else {
                name(value)
            }
        }
        11..=19 => format!("十{}", name(value - 10)),
        _ => {
            let tens = value / 10;
            let ones = value % 10;
            if ones == 0 {
                format!("{}十", name(tens))
            } else {
                format!("{}十{}", name(tens), name(ones))
            }
        }
    }
}

/// The Chinese day-period name, five segments over the twenty-four hours:
/// 凌晨 before six, 早上 to nine, 上午 to noon, 下午 to eighteen, 晚上 after.
fn day_period(hour: u8) -> &'static str {
    match hour {
        0..=5 => "凌晨",
        6..=8 => "早上",
        9..=11 => "上午",
        12..=17 => "下午",
        // The snapshot documents `0..=23`; any wider value still lands in a
        // period rather than a panic.
        _ => "晚上",
    }
}

/// The twelve-hour reading of an hour: midnight stays zero, noon stays twelve, the
/// afternoon counts down from twelve.
fn twelve_hour(hour: u8) -> u8 {
    match hour {
        0 => 0,
        1..=12 => hour,
        _ => hour - 12,
    }
}

/// One candidate with the given text and annotation.
fn plain_candidate(text: impl Into<String>, annotation: &str) -> QuickCandidate {
    QuickCandidate {
        text: text.into(),
        annotation: Some(String::from(annotation)),
    }
}

#[cfg(test)]
mod tests {
    use super::{CivilTime, translate};
    use crate::quick::tests::pinned_time;

    /// A snapshot variant with the fields the caller overrides.
    fn time_at(year: i32, month: u8, day: u8, hour: u8, minute: u8, second: u8, weekday: u8) -> CivilTime {
        CivilTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
            weekday,
        }
    }

    #[test]
    fn test_translate_rq_pins_four_formats() {
        let texts: Vec<String> = translate("rq", &pinned_time())
            .into_iter()
            .map(|candidate| candidate.text)
            .collect();
        assert_eq!(
            texts,
            [
                "2026-10-28",
                "2026年10月28日",
                "二〇二六年十月二十八日",
                "10月28日",
            ]
        );
    }

    #[test]
    fn test_translate_sj_pins_formats_with_period_chinese() {
        let texts: Vec<String> = translate("sj", &pinned_time())
            .into_iter()
            .map(|candidate| candidate.text)
            .collect();
        assert_eq!(texts, ["12:34", "12:34:56", "下午12时34分", "下午12:34"]);
    }

    #[test]
    fn test_translate_period_names_cover_five_segments() {
        let period_of = |hour: u8| {
            translate("sj", &time_at(2026, 1, 2, hour, 5, 0, 0))
                .into_iter()
                .nth(2)
                .map(|candidate| candidate.text)
                .unwrap_or_default()
        };
        assert!(period_of(0).starts_with("凌晨"), "{}", period_of(0));
        assert!(period_of(5).starts_with("凌晨"));
        assert!(period_of(6).starts_with("早上"));
        assert!(period_of(9).starts_with("上午"));
        assert!(period_of(13).starts_with("下午"));
        assert!(period_of(18).starts_with("晚上"));
        assert!(period_of(23).starts_with("晚上"));
        // Twelve-hour arithmetic across the two ends of the day.
        assert_eq!(period_of(0), "凌晨0时5分");
        assert_eq!(period_of(14), "下午2时5分");
    }

    #[test]
    fn test_translate_xq_pins_three_formats() {
        let texts: Vec<String> = translate("xq", &pinned_time())
            .into_iter()
            .map(|candidate| candidate.text)
            .collect();
        assert_eq!(texts, ["星期三", "周三", "礼拜三"]);
    }

    #[test]
    fn test_translate_weekday_index_bounds_reach_both_ends() {
        let monday = translate("xq", &time_at(2026, 10, 26, 0, 0, 0, 0))
            .into_iter()
            .next()
            .map(|candidate| candidate.text);
        assert_eq!(monday.as_deref(), Some("星期一"));
        let sunday = translate("xq", &time_at(2026, 11, 1, 0, 0, 0, 6))
            .into_iter()
            .next()
            .map(|candidate| candidate.text);
        assert_eq!(sunday.as_deref(), Some("星期日"));
        // A weekday outside the documented range renders nothing rather than a
        // wrong day.
        assert!(translate("xq", &time_at(2026, 11, 1, 0, 0, 0, 7)).is_empty());
    }

    #[test]
    fn test_translate_dt_pins_iso_8601_renderings() {
        let texts: Vec<String> = translate("dt", &pinned_time())
            .into_iter()
            .map(|candidate| candidate.text)
            .collect();
        assert_eq!(
            texts,
            ["2026-10-28T12:34:56Z", "2026-10-28T12:34:56+00:00"]
        );
    }

    #[test]
    fn test_translate_ts_pins_timestamp() {
        let texts: Vec<String> = translate("ts", &pinned_time())
            .into_iter()
            .map(|candidate| candidate.text)
            .collect();
        assert_eq!(texts, ["2026-10-28 12:34:56"]);
    }

    #[test]
    fn test_translate_unknown_trigger_renders_nothing() {
        assert!(translate("zz", &pinned_time()).is_empty());
        assert!(translate("", &pinned_time()).is_empty());
    }

    #[test]
    fn test_translate_leap_day_and_zero_padding() {
        let leap = translate("rq", &time_at(2024, 2, 29, 0, 0, 0, 4))
            .into_iter()
            .map(|candidate| candidate.text)
            .collect::<Vec<_>>();
        assert_eq!(leap[0], "2024-02-29");
        let padded = translate("rq", &time_at(2026, 1, 5, 0, 0, 0, 4))
            .into_iter()
            .map(|candidate| candidate.text)
            .collect::<Vec<_>>();
        assert_eq!(padded[0], "2026-01-05");
        assert_eq!(padded[2], "二〇二六年一月五日");
    }

    #[test]
    fn test_translate_malformed_snapshot_skips_chinese_forms() {
        // A month of zero cannot back a Chinese reading; the numeric candidates
        // still render, faithfully to the broken fields they were handed.
        let out = translate("rq", &time_at(2026, 0, 5, 0, 0, 0, 4));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "2026-00-05");
    }
}
