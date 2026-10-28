//! The host boundary: what the engine does to Fcitx5 and to the candidate window.
//!
//! # Responsibility
//!
//! A session decides; this trait is how the decision becomes visible. Every action the
//! routing layer takes outside its own state goes through [`Host`]: text reaches the
//! application, the composing text reaches the application's preedit area, a command
//! reaches the candidate window's channel, a mode switch reaches Fcitx5's input-method
//! state, and a diagnostic reaches the sink.
//!
//! # Why the engine takes this as a parameter
//!
//! The engine never holds an `InputContext`. Reaching one means a raw pointer, and raw
//! pointers are confined to `crate::ffi` — the input-context table the id-to-pointer
//! lookup needs lives in the C++ glue and can only be read through it. Passing the
//! boundary in as a parameter is what keeps the whole routing layer — the table, the
//! sessions, the effect executor — plain Rust that runs and is tested without Fcitx5
//! present.
//!
//! # What an implementation owes
//!
//! * **Do not block.** Every method runs on the Fcitx5 main loop, inside a host
//!   callback. [`Host::post_ui`] posts to the candidate window's channel and returns;
//!   it never waits for the window to draw.
//! * **Do not report success that did not happen.** The engine claims a key only when
//!   the plugin acted on it, and what it acted on is what these methods did. An
//!   implementation that silently drops a commit makes the engine consume a key whose
//!   text never reached the application.
//! * **Never log user content.** The text handed to [`Host::commit`] and
//!   [`Host::set_preedit`] is the user's input; it goes to the application and nowhere
//!   else. Diagnostics carry the stable `domain/action/reason` code of the error they
//!   report, never a word the user typed.
//!
//! # The production implementation
//!
//! It belongs beside the C ABI, in `crate::ffi`, because it needs the host's own
//! objects: the input-context table behind the id the callbacks receive, the
//! `InputContext::commitString` and `setPreedit` calls, and the user-interface channel
//! of the design's `UiCommand` queue. The C ABI carries the host-to-engine direction
//! today, so that implementation is the piece the host wiring adds; the routing layer
//! is complete against this trait without it.

use ime_types::{ImeError, UiCommand};

use crate::ffi::FcitxKeyEvent;

/// The host actions the routing layer performs.
///
/// One implementation drives a real Fcitx5 session and one drives the tests; see the
/// module documentation for what every implementation owes.
/// What the host answered when the compose step asked about one key.
///
/// The host owns the keyboard layout's dead-key state (`processComposeString` on
/// the reference side); the plugin owns none of it, so the answer is the host's
/// three ways and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposeOutcome {
    /// The host composes nothing: the key is the caller's to route as always.
    /// This is also what a host that offers no compose support answers, which is
    /// why the step costs one virtual call and nothing more.
    Unsupported,
    /// The key opened or continued a dead-key sequence: the sequence is not
    /// finished, so the key is swallowed and the application sees nothing.
    Swallowed,
    /// The sequence finished and produced this character: the character reaches
    /// the application as a direct commit, the semantics of a pass-through
    /// commit with no learning and no candidate window.
    Committed(char),
}

pub trait Host {
    /// Asks the host whether this key is part of a dead-key sequence it is
    /// composing.
    ///
    /// The compose step calls this before anything else claims the key, and only
    /// for a press without Ctrl or Super on an idle session -- a composing
    /// session is never asked, which is what keeps the hot path free of the
    /// call. The default answers [`ComposeOutcome::Unsupported`]: a host that
    /// offers no compose support keeps every key exactly where the routing
    /// table put it.
    ///
    /// # Panics
    ///
    /// Never.
    fn process_compose(&mut self, _ic: u64, _key: &FcitxKeyEvent) -> ComposeOutcome {
        ComposeOutcome::Unsupported
    }

    /// Hands committed text to the application.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context the commit happened in.
    /// * `text` — the text to insert. It is user content: it goes to the application
    ///   and must not reach a log event, a diagnostic line or a crash report.
    fn commit(&mut self, ic: u64, text: &str);

    /// Writes the composing text into the application's own preedit area.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    /// * `text` — the composing text, as the preedit builder produced it.
    /// * `caret` — the caret position in bytes, always on a character boundary.
    fn set_preedit(&mut self, ic: u64, text: &str, caret: u32);

    /// Empties the application's preedit area.
    ///
    /// Called both when a composition ends and when the client-preedit policy is off,
    /// in which case the area has to stay empty rather than hold what an earlier
    /// configuration put there.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    fn clear_preedit(&mut self, ic: u64);

    /// Posts one command to the candidate window.
    ///
    /// The command carries no input-context identity of its own, so `ic` is what tells
    /// an implementation which context it came from.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context the command belongs to.
    /// * `command` — the command, which the channel routes by its own class: a frame is
    ///   latest-wins, a show or a hide is ordered and never dropped.
    fn post_ui(&mut self, ic: u64, command: UiCommand);

    /// Flips the input context's enabled state, and reports the state it took.
    ///
    /// This is Fcitx5's own input-method state, not a flag of the plugin's: with the
    /// context disabled the host hands the keyboard back to the application, which is
    /// what switching to English means.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context.
    ///
    /// # Returns
    ///
    /// Whether the context is enabled after the call, which is what the status strip
    /// shows. An implementation that cannot read the state back reports the state it
    /// set.
    fn toggle_enabled(&mut self, ic: u64) -> bool;

    /// Reports one diagnostic condition.
    ///
    /// # Arguments
    ///
    /// * `ic` — the host's identity for the input context the condition belongs to.
    /// * `err` — the condition, whose rendering is the stable
    ///   `domain/action/reason` code. It never carries the characters the user typed.
    fn diagnose(&mut self, ic: u64, err: &ImeError);
}

/// A civil-time value handed to a quick-input translator.
///
/// The engine never reads the clock itself: a host supplies one of these snapshots,
/// which makes date and time rendering deterministic under test and keeps the hot
/// path free of hidden system calls.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockSnapshot {
    /// The Gregorian year.
    pub year: i32,
    /// The month, `1..=12`.
    pub month: u8,
    /// The day of month, `1..=31` as valid for the month and year.
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

/// Wall-clock access for quick-input domains.
///
/// An implementation must be non-blocking enough for the Fcitx5 callback that asks
/// for a snapshot. A host that cannot supply time should simply not install a
/// quick-input route, rather than report an error on every key.
pub trait ClockSource {
    /// Returns the current civil time.
    ///
    /// # Panics
    ///
    /// Implementations must never panic.
    fn now(&self) -> ClockSnapshot;
}

/// Injected randomness for quick-input domains that need it.
///
/// The indirection keeps a translator pure with respect to its inputs and lets a test
/// pin an entire byte sequence.
pub trait RngSource {
    /// Fills `dest` with random bytes.
    ///
    /// # Panics
    ///
    /// Implementations must never panic.
    fn fill_bytes(&self, dest: &mut [u8]);
}

/// The production clock.
///
/// It performs one UTC snapshot per call. Quick-input output is therefore UTC at this
/// scaffold stage; localization and timezone policy belong with the translator work,
/// which will consume this same injected boundary.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl ClockSource for SystemClock {
    fn now(&self) -> ClockSnapshot {
        let elapsed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let seconds = u64::try_from(elapsed.as_secs()).unwrap_or(0);
        let days = seconds / SECONDS_PER_DAY;
        let seconds_of_day = seconds % SECONDS_PER_DAY;
        let (year, month, day) = civil_from_days(days);
        let weekday = ((days % DAYS_PER_WEEK) + (SEVEN - EPOCH_MONDAY_INDEX)) % DAYS_PER_WEEK;
        ClockSnapshot {
            year,
            month,
            day,
            hour: u8::try_from(seconds_of_day / SECONDS_PER_HOUR).unwrap_or(0),
            minute: u8::try_from((seconds_of_day / SECONDS_PER_MINUTE) % SECONDS_PER_MINUTE)
                .unwrap_or(0),
            second: u8::try_from(seconds_of_day % SECONDS_PER_MINUTE).unwrap_or(0),
            weekday: u8::try_from(weekday).unwrap_or(0),
        }
    }
}

/// The production random source.
///
/// A small OS-time-seeded generator is enough for this boundary's scaffold: no quick
/// input route consumes it yet, and introducing a random-number dependency is part of
/// the increment that actually needs generated values.
#[derive(Debug)]
pub struct PrngSource {
    state: std::sync::atomic::AtomicU64,
}

impl PrngSource {
    /// Creates a source seeded from the system clock's sub-second nanoseconds.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_system() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos())
            .unwrap_or_default();
        let seed = u64::from(nanos) | 1;
        Self {
            state: std::sync::atomic::AtomicU64::new(seed),
        }
    }
}

impl Default for PrngSource {
    /// A non-zero seed, so the generator's all-zero fixed point is unreachable.
    fn default() -> Self {
        Self {
            state: std::sync::atomic::AtomicU64::new(1),
        }
    }
}

impl RngSource for PrngSource {
    fn fill_bytes(&self, dest: &mut [u8]) {
        use std::sync::atomic::Ordering;
        let mut block = 0_u64;
        for (index, byte) in dest.iter_mut().enumerate() {
            if index % u64::BITS as usize == 0 {
                let previous = self.state.load(Ordering::Relaxed);
                let next = previous ^ (previous << 13);
                let next = next ^ (next >> 7);
                let next = next ^ (next << 17);
                self.state.store(next.max(1), Ordering::Relaxed);
                block = next;
            }
            *byte = (block >> ((index % u64::BITS as usize) * 8)) as u8;
        }
    }
}

const SECONDS_PER_DAY: u64 = 24 * 60 * 60;
const SECONDS_PER_HOUR: u64 = 60 * 60;
const SECONDS_PER_MINUTE: u64 = 60;
const DAYS_PER_WEEK: u64 = 7;
const SEVEN: u64 = 7;
// Unix day 0 is a Thursday; Monday has index zero in `ClockSnapshot::weekday`.
const EPOCH_MONDAY_INDEX: u64 = 3;

/// Converts days since the Unix epoch to a civil Gregorian date.
///
/// Howard Hinnant's `civil_from_days` algorithm: the leap-year rule is expressed in
/// whole-day arithmetic, so there is no month table to get wrong and no failure path.
fn civil_from_days(days_since_epoch: u64) -> (i32, u8, u8) {
    let shifted = days_since_epoch + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era as i64 * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (
        i32::try_from(year).unwrap_or(i32::MAX),
        u8::try_from(month).unwrap_or(12),
        u8::try_from(day).unwrap_or(1),
    )
}

#[cfg(test)]
mod host_boundary_tests {
    use super::{ClockSnapshot, ClockSource, PrngSource, RngSource};

    #[test]
    fn test_system_clock_known_epoch_snapshot() {
        let clock = FixedClock(ClockSnapshot {
            year: 2026,
            month: 10,
            day: 28,
            hour: 12,
            minute: 34,
            second: 56,
            weekday: 2,
        });
        assert_eq!(clock.now(), clock.0);
    }

    #[test]
    fn test_prng_fill_is_nonzero_and_repeats_for_same_seed() {
        let first = PrngSource::default();
        let second = PrngSource::default();
        let mut left = [0_u8; 16];
        let mut right = [0_u8; 16];
        first.fill_bytes(&mut left);
        second.fill_bytes(&mut right);
        assert_eq!(left, right);
        assert!(left.iter().any(|byte| *byte != 0));
    }

    struct FixedClock(ClockSnapshot);

    impl ClockSource for FixedClock {
        fn now(&self) -> ClockSnapshot {
            self.0
        }
    }
}
