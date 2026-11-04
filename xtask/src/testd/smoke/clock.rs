//! The injected clock and the civil-time renderer behind `@today:`.
//!
//! Responsibility: turn a clock reading into the civil fields the assertions and the
//! quick domains consume, and render a `@today:` format string against those fields.
//! The clock is a value the caller injects -- [`Clock::System`] is the CLI's choice and
//! [`Clock::Fixed`] is what the tests pin -- so a `@today:` row answers identically on
//! every machine that names the same instant, and a test never reads the wall clock.
//!
//! Boundaries: this is the only smoke module that touches a clock, and the touch is
//! one `SystemTime::now` behind [`Clock::System`]. The decode path proper stays
//! clock-free; a row that names no `@today:` form never reaches this module.
//!
//! # Why the renderer is hand-rolled
//!
//! `@today:<fmt>` names a strftime-style format, and the workspace carries no time
//! crate to borrow one from (and must not gain one: no new dependency is budgeted for
//! this card). The directive set is therefore deliberately small -- `%Y %m %d %H %M %S`
//! and the `%%` escape -- which is every field the quick date family renders and the
//! whole surface the card's column contract names. A directive outside the set is a
//! parse error at case-load time, never a silent miss rendered at run time.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, ensure};

use ime_core::quick::CivilTime;

/// The clock a run renders `@today:` against.
///
/// A value, not a service: the run carries one, and every `@today:` row of the run
/// renders against the same reading, which is what makes a run's verdicts comparable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clock {
    /// The process wall clock, read once per render. The CLI's choice.
    System,
    /// A fixed reading, in whole seconds since the Unix epoch. The tests' choice: the
    /// same instant renders the same text forever.
    // The CLI names `System` alone, so the non-test build sees no constructor for
    // this variant; its consumers are the smoke tests, which is the documented role.
    #[allow(dead_code)]
    Fixed {
        /// Seconds since the Unix epoch the clock always answers.
        epoch_secs: i64,
    },
}

impl Clock {
    /// Answers the clock's reading in whole seconds since the Unix epoch.
    ///
    /// # Panics
    ///
    /// Never. A system clock that reads before the epoch saturates to zero rather than
    /// failing the run: no host reaches that state, and a total evaluation is worth more
    /// than an unreachable error branch.
    pub fn now_secs(self) -> i64 {
        match self {
            Self::System => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
                .unwrap_or(0),
            Self::Fixed { epoch_secs } => epoch_secs,
        }
    }

    /// Answers the clock's reading split into civil fields.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn civil(self) -> CivilTime {
        civil_from_secs(self.now_secs())
    }
}

/// Splits whole seconds since the Unix epoch into civil fields.
///
/// The day conversion is Howard Hinnant's `civil_from_days` algorithm, which maps the
/// proleptic Gregorian day count to a year-month-day triple without a table; the
/// weekday comes from the same day count, because 1970-01-01 was a Thursday and the
/// engine's weekday contract counts from Monday.
///
/// # Panics
///
/// Never.
pub fn civil_from_secs(epoch_secs: i64) -> CivilTime {
    let days = epoch_secs.div_euclid(86_400);
    let secs_of_day = epoch_secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    CivilTime {
        year: i32::try_from(year).unwrap_or(i32::MAX),
        month: u8::try_from(month).unwrap_or(1),
        day: u8::try_from(day).unwrap_or(1),
        hour: u8::try_from(secs_of_day / 3_600).unwrap_or(0),
        minute: u8::try_from((secs_of_day % 3_600) / 60).unwrap_or(0),
        second: u8::try_from(secs_of_day % 60).unwrap_or(0),
        // 1970-01-01 was a Thursday; Monday is zero, so Thursday is three.
        weekday: u8::try_from((days + 3).rem_euclid(7)).unwrap_or(0),
    }
}

/// Maps a Gregorian day count (days since 1970-01-01) to `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`: shift into the Gregorian epoch of 0000-03-01 so
/// the leap day lands at the end of the cycle year, then read the era, the day of era
/// and the month pointer out of the shifted count. Every division is Euclidean, so a
/// day count before the epoch converts as correctly as one after it.
///
/// # Panics
///
/// Never.
fn civil_from_days(days: i64) -> (i64, u64, u64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (
        year,
        u64::try_from(m).unwrap_or(1),
        u64::try_from(d).unwrap_or(1),
    )
}

/// Renders `time` through a `@today:` format string.
///
/// The directives are `%Y %m %d %H %M %S`; `%%` escapes one `%`; every other byte is
/// copied through. The format is validated at case-load time by
/// [`validate_format`], so this function never meets a directive it does not know and
/// needs no error channel.
///
/// # Panics
///
/// Never.
pub fn format_civil(time: &CivilTime, fmt: &str) -> String {
    let mut out = String::with_capacity(fmt.len() + 4);
    let mut chars = fmt.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('Y') => out.push_str(&format!("{:04}", time.year)),
            Some('m') => out.push_str(&format!("{:02}", time.month)),
            Some('d') => out.push_str(&format!("{:02}", time.day)),
            Some('H') => out.push_str(&format!("{:02}", time.hour)),
            Some('M') => out.push_str(&format!("{:02}", time.minute)),
            Some('S') => out.push_str(&format!("{:02}", time.second)),
            Some('%') => out.push('%'),
            // Unreachable for a format `validate_format` accepted; copied verbatim so a
            // caller that skipped the validation still gets a deterministic string.
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// Validates the format a `@today:` row names.
///
/// # Errors
///
/// Names the offending directive when the format carries a `%` the renderer does not
/// know, or a trailing `%` with nothing after it. Refusing at load time is what keeps a
/// misspelled directive from passing every run while asserting nothing a user meant.
///
/// # Panics
///
/// Never.
pub fn validate_format(fmt: &str) -> Result<()> {
    ensure!(!fmt.is_empty(), "the @today format is empty");
    let mut chars = fmt.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            continue;
        }
        match chars.next() {
            Some('Y' | 'm' | 'd' | 'H' | 'M' | 'S' | '%') => {}
            Some(other) => {
                return Err(anyhow::anyhow!(
                    "the @today format carries %{other}, which is not a \
                     directive this renderer knows (Y m d H M S %)"
                ));
            }
            None => {
                return Err(anyhow::anyhow!(
                    "the @today format ends in a bare % with no directive after it"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Clock, civil_from_secs, format_civil, validate_format};

    /// Seconds for 2026-10-28 12:34:56 UTC, the same pinned instant the quick module's
    /// own tests use, so a rendered answer is comparable across the two.
    const PINNED_SECS: i64 = 1_793_190_896;

    #[test]
    fn test_epoch_zero_is_the_1970_thursday() {
        let time = civil_from_secs(0);
        assert_eq!(time.year, 1970);
        assert_eq!(time.month, 1);
        assert_eq!(time.day, 1);
        assert_eq!(time.weekday, 3, "1970-01-01 was a Thursday, Monday is zero");
        assert_eq!(time.hour, 0);
        assert_eq!(time.minute, 0);
        assert_eq!(time.second, 0);
    }

    #[test]
    fn test_civil_and_format_pin_the_known_instant() {
        let time = civil_from_secs(PINNED_SECS);
        assert_eq!(time.year, 2026);
        assert_eq!(time.month, 10);
        assert_eq!(time.day, 28);
        assert_eq!(time.hour, 12);
        assert_eq!(time.minute, 34);
        assert_eq!(time.second, 56);
        assert_eq!(time.weekday, 2, "2026-10-28 was a Wednesday");
        assert_eq!(
            format_civil(&time, "%Y-%m-%d %H:%M:%S"),
            "2026-10-28 12:34:56"
        );
    }

    #[test]
    fn test_civil_converts_a_leap_day_and_the_day_before_it() {
        // 2024-02-29 00:00:00 UTC and 2024-02-28 23:59:59 UTC must land on the
        // opposite sides of the leap day: the era arithmetic is where a plain
        // 365-day conversion would drift.
        let leap = civil_from_secs(1_709_164_800);
        assert_eq!((leap.year, leap.month, leap.day), (2024, 2, 29));
        let before = civil_from_secs(1_709_164_799);
        assert_eq!((before.year, before.month, before.day), (2024, 2, 28));
        assert_eq!(before.second, 59);
    }

    #[test]
    fn test_civil_converts_a_pre_epoch_reading() {
        // One second before the epoch: the Euclidean divisions must carry the
        // borrow, not wrap into a wrong year.
        let time = civil_from_secs(-1);
        assert_eq!((time.year, time.month, time.day), (1969, 12, 31));
        assert_eq!((time.hour, time.minute, time.second), (23, 59, 59));
    }

    #[test]
    fn test_format_copies_literal_text_and_escapes_percent() {
        let time = civil_from_secs(PINNED_SECS);
        assert_eq!(format_civil(&time, "%Y年%m月"), "2026年10月");
        assert_eq!(format_civil(&time, "100%%"), "100%");
        assert_eq!(format_civil(&time, "plain"), "plain");
        assert_eq!(
            format_civil(&time, "%Y-%m-%dT%H:%M:%SZ"),
            "2026-10-28T12:34:56Z"
        );
    }

    #[test]
    fn test_validate_format_accepts_the_known_directives_and_refuses_others() {
        validate_format("%Y-%m-%d").expect("the date directives are legal");
        validate_format("%H:%M:%S %%").expect("the escape is legal");
        let error = validate_format("%Q").expect_err("an unknown directive is refused");
        assert!(error.to_string().contains("%Q"), "{error}");
        let error = validate_format("trailing %").expect_err("a bare % is refused");
        assert!(error.to_string().contains("bare %"), "{error}");
        let error = validate_format("").expect_err("an empty format pins nothing");
        assert!(error.to_string().contains("empty"), "{error}");
    }

    #[test]
    fn test_clock_fixed_is_stable_and_system_moves_forward() {
        let fixed = Clock::Fixed {
            epoch_secs: PINNED_SECS,
        };
        assert_eq!(fixed.now_secs(), PINNED_SECS);
        assert_eq!(
            fixed.now_secs(),
            fixed.now_secs(),
            "a fixed clock cannot drift"
        );
        // A floor, not an equality: the wall clock only has to be later than a
        // reading that is already years past, which keeps the test off the clock.
        assert!(
            Clock::System.now_secs() >= 1_600_000_000,
            "the system clock reads after 2020-09"
        );
    }

    #[test]
    fn test_a_clock_offset_changes_what_today_renders() {
        // The card's mutation probe: two clocks one day apart must render two
        // dates, which is what lets a red `@today` row name a skew and not a
        // ranking change.
        let day = 86_400;
        let earlier = format_civil(
            &Clock::Fixed {
                epoch_secs: PINNED_SECS,
            }
            .civil(),
            "%Y-%m-%d",
        );
        let later = format_civil(
            &Clock::Fixed {
                epoch_secs: PINNED_SECS + day,
            }
            .civil(),
            "%Y-%m-%d",
        );
        assert_eq!(earlier, "2026-10-28");
        assert_eq!(later, "2026-10-29");
    }
}
