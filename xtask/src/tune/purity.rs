//! The collection-purity gate at the tool boundary: whether a machine is quiet
//! enough for a latency-sensitive number to be taken on it at all.
//!
//! Responsibility: refuse -- explicitly, and before any measurement starts --
//! when the one-minute load average is already too high for a number taken now
//! to mean anything. The discipline this enforces used to live only in prose
//! ("collect on an idle machine"); the ledger of mis-measurements it produced is
//! recorded in [`crate::testd::purity`], whose per-case probe answers a different
//! question -- what a finished run's own samples say about the machine they were
//! taken on. This module answers the question before the run: one read of
//! `/proc/loadavg` and one processor count, and a busy machine gets an explicit
//! refusal instead of a number that later reads as a regression.
//!
//! The check is deliberately small. A refusal must cost almost nothing to
//! produce, and every rule must be a case a test can drive with injected values:
//! [`PurityCheck`] is plain data over the two facts, [`PurityCheck::verify`] reads
//! nothing, and the one file read lives in [`PurityCheck::read`], which a test
//! never calls. A fact that cannot be read -- a load average that parses to
//! nothing, a processor count the platform will not give -- refuses too, because
//! a gate that silently passes when its evidence is missing is a gate that
//! passes dirty machines.
//!
//! What the gate cannot do is watch the run: between the check and the last
//! grid point hours can pass, and something else may start in between. Keeping
//! the window quiet is the operator's discipline; the gate's job is to make
//! starting on a provably busy machine impossible.
//!
//! Boundaries: it reads one `/proc` file and the parallelism of the process, and
//! it writes nothing. The three collection entries -- the benchmark-budget gate,
//! the tuner's sweep, and the soak driver -- call it first; the judging paths
//! (`--assert` style reports) do not, because judging an old number does not
//! take a new one.

use std::fs;
use std::thread;

/// The file the kernel exposes the one-minute load average in.
const LOADAVG_FILE: &str = "/proc/loadavg";

/// The load ceiling, as a fraction of the processor count: refuse when the
/// one-minute load average exceeds this share of the cores.
///
/// A quarter of the machine is enough headroom for the run's own work to stay
/// the dominant signal while leaving the desktop and the kernel responsive; the
/// factor is policy, not physics, and tightening it is a one-constant change.
const LOAD_CEILING_FACTOR: f32 = 0.25;

/// Whether this machine is quiet enough to collect on, decided over two
/// injected facts.
///
/// The struct is the whole check: a caller that has the facts from somewhere
/// other than this process's own `/proc` -- a test, a future remote reader --
/// builds one directly and calls [`PurityCheck::verify`]. The fields are public
/// for exactly that reason, and the type carries no behaviour beyond the
/// verdict.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PurityCheck {
    /// The one-minute load average the check judges.
    pub load_1m: f32,
    /// The processor count the ceiling is derived from.
    pub cpu_count: u32,
}

impl PurityCheck {
    /// Reads the facts this process runs under: the one-minute load average
    /// from `/proc/loadavg` and the processors available to the process.
    ///
    /// A reading that cannot be taken is a refusal, not a default: the caller is
    /// about to spend minutes or hours collecting, and a guessed fact is exactly
    /// the silent pass the fail-closed rule forbids.
    ///
    /// # Errors
    ///
    /// Returns [`PurityRefusal::Unmeasurable`] when `/proc/loadavg` cannot be
    /// read or does not parse, and when the processor count cannot be resolved.
    pub fn read() -> Result<Self, PurityRefusal> {
        let text = fs::read_to_string(LOADAVG_FILE).map_err(|error| {
            PurityRefusal::unmeasurable("the one-minute load average", &error.to_string())
        })?;
        let load_1m = parse_loadavg(&text).ok_or_else(|| {
            PurityRefusal::unmeasurable(
                "the one-minute load average",
                "the file did not parse into a number",
            )
        })?;
        // A platform that cannot say how many processors it has also cannot have
        // a ceiling derived from one; `u32::MAX` is unreachable for a real core
        // count, and clamping there keeps the ceiling permissive rather than
        // inventing a smaller machine than the host has.
        let count = thread::available_parallelism()
            .map_err(|error| {
                PurityRefusal::unmeasurable("the processor count", &error.to_string())
            })?
            .get();
        let cpu_count = u32::try_from(count).unwrap_or(u32::MAX);
        Ok(Self { load_1m, cpu_count })
    }

    /// The load ceiling this machine's facts derive: the core count times the
    /// policy's factor. `verify` compares against it, and the entry points
    /// print it, so the arithmetic lives in one place.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn ceiling(&self) -> f32 {
        LOAD_CEILING_FACTOR * self.cpu_count as f32
    }

    /// Judges the injected facts, refusing when the machine is busy or when a
    /// fact cannot be trusted.
    ///
    /// The comparison is strict: a load exactly at the ceiling passes, because
    /// the ceiling is the headroom the policy grants, not a number a machine is
    /// punished for touching.
    ///
    /// # Errors
    ///
    /// Returns [`PurityRefusal::Overloaded`] when the one-minute load average
    /// exceeds the ceiling [`PurityCheck::ceiling`] derives, and
    /// [`PurityRefusal::Unmeasurable`] when the load is not a finite
    /// non-negative number or the processor count is zero -- none of which can
    /// be read as clean.
    pub fn verify(&self) -> Result<(), PurityRefusal> {
        if !self.load_1m.is_finite() || self.load_1m < 0.0 {
            return Err(PurityRefusal::unmeasurable(
                "the one-minute load average",
                "the reading is not a finite non-negative number",
            ));
        }
        if self.cpu_count == 0 {
            return Err(PurityRefusal::unmeasurable(
                "the processor count",
                "zero cores cannot derive a load ceiling",
            ));
        }
        let ceiling = self.ceiling();
        if self.load_1m > ceiling {
            return Err(PurityRefusal::Overloaded {
                load_1m: self.load_1m,
                cpu_count: self.cpu_count,
                ceiling,
            });
        }
        Ok(())
    }
}

/// The one-minute load average in the first field of a `/proc/loadavg` dump.
///
/// The kernel writes `0.52 0.58 0.59 1/467 12345`; only the first field is the
/// one-minute average, and only a finite parse counts. Anything else -- an empty
/// file, a non-numeric field, an infinity -- is `None`, which the caller turns
/// into a refusal.
fn parse_loadavg(text: &str) -> Option<f32> {
    let field = text.split_whitespace().next()?;
    let load = field.parse::<f32>().ok()?;
    load.is_finite().then_some(load)
}

/// Why a collection path refused to start.
///
/// Every variant renders as the actionable guidance the refusing tool prints:
/// a refusal that does not say what to do next just moves the confusion from
/// the number to the exit code.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("{}", PurityRefusal::text(self))]
pub enum PurityRefusal {
    /// The one-minute load average is past the ceiling the cores derive.
    Overloaded {
        /// The load average that was read.
        load_1m: f32,
        /// The processor count the ceiling was derived from.
        cpu_count: u32,
        /// The ceiling itself: the core count times the policy's factor.
        ceiling: f32,
    },
    /// A fact the verdict needed could not be read into a trustworthy number.
    Unmeasurable {
        /// Which fact failed.
        fact: &'static str,
        /// Why it failed, for the refusal text.
        detail: String,
    },
}

impl PurityRefusal {
    /// Builds the `Unmeasurable` variant; a constructor rather than a literal so
    /// every call site spells the same two parts in the same order.
    fn unmeasurable(fact: &'static str, detail: &str) -> Self {
        Self::Unmeasurable {
            fact,
            detail: detail.to_owned(),
        }
    }

    /// The refusal as the reader should see it: the facts, then the remedies.
    ///
    /// The one-minute average trails reality by up to a minute, so a refusal
    /// that arrives right after a heavy build can be that build's tail rather
    /// than a live load; the text says so, because sending a user chasing a load
    /// that has already gone would teach them to ignore the gate.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn text(&self) -> String {
        match self {
            Self::Overloaded {
                load_1m,
                cpu_count,
                ceiling,
            } => format!(
                "the machine is too busy to collect on: the one-minute load average is \
                 {load_1m:.2}, past the {ceiling:.2} ceiling ({cpu_count} cores, quarter \
                 load). Stop the background work (parallel builds, test agents, \
                 indexers), or wait for the average to settle and re-run; a sweep can \
                 also shrink its grid so the machine has to stay quiet for less time",
            ),
            Self::Unmeasurable { fact, detail } => format!(
                "the machine cannot be judged clean or busy: reading {fact} failed \
                 ({detail}). Fix the reading and re-run; refusing rather than guessing \
                 is what keeps a dirty machine from producing a plausible number",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PurityCheck;
    use super::PurityRefusal;

    /// An idle eight-core reading: load well under a quarter of the cores.
    fn idle() -> PurityCheck {
        PurityCheck {
            load_1m: 0.5,
            cpu_count: 8,
        }
    }

    #[test]
    fn test_verify_idle_machine_passes() {
        assert_eq!(idle().verify(), Ok(()));
    }

    #[test]
    fn test_verify_load_at_the_ceiling_passes() {
        // The ceiling is granted headroom, not a tripwire: exactly a quarter of
        // the cores is still quiet enough.
        let check = PurityCheck {
            load_1m: 2.0,
            cpu_count: 8,
        };
        assert_eq!(check.verify(), Ok(()));
    }

    #[test]
    fn test_verify_loaded_machine_refuses_overloaded() {
        let check = PurityCheck {
            load_1m: 3.0,
            cpu_count: 8,
        };
        assert_eq!(
            check.verify(),
            Err(PurityRefusal::Overloaded {
                load_1m: 3.0,
                cpu_count: 8,
                ceiling: 2.0,
            })
        );
    }

    #[test]
    fn test_verify_single_core_refuses_immediately_under_load() {
        let check = PurityCheck {
            load_1m: 0.3,
            cpu_count: 1,
        };
        assert!(matches!(
            check.verify(),
            Err(PurityRefusal::Overloaded { ceiling, .. }) if ceiling == 0.25,
        ));
    }

    #[test]
    fn test_verify_nan_load_refuses_fail_closed() {
        let check = PurityCheck {
            load_1m: f32::NAN,
            cpu_count: 8,
        };
        assert!(matches!(
            check.verify(),
            Err(PurityRefusal::Unmeasurable { .. })
        ));
    }

    #[test]
    fn test_verify_infinite_load_refuses_fail_closed() {
        let check = PurityCheck {
            load_1m: f32::INFINITY,
            cpu_count: 8,
        };
        assert!(matches!(
            check.verify(),
            Err(PurityRefusal::Unmeasurable { .. })
        ));
    }

    #[test]
    fn test_verify_negative_load_refuses_fail_closed() {
        let check = PurityCheck {
            load_1m: -0.1,
            cpu_count: 8,
        };
        assert!(matches!(
            check.verify(),
            Err(PurityRefusal::Unmeasurable { .. })
        ));
    }

    #[test]
    fn test_verify_zero_core_count_refuses_fail_closed() {
        let check = PurityCheck {
            load_1m: 0.0,
            cpu_count: 0,
        };
        assert!(matches!(
            check.verify(),
            Err(PurityRefusal::Unmeasurable { .. })
        ));
    }

    #[test]
    fn test_parse_loadavg_reads_the_one_minute_field() {
        assert_eq!(
            super::parse_loadavg("0.52 0.58 0.59 1/467 12345\n"),
            Some(0.52)
        );
    }

    #[test]
    fn test_parse_loadavg_garbage_is_none() {
        assert_eq!(super::parse_loadavg(""), None);
        assert_eq!(super::parse_loadavg("busy\n"), None);
        assert_eq!(super::parse_loadavg("nan 0.1 0.1 1/1 1"), None);
    }

    #[test]
    fn test_refusal_text_names_the_facts_and_the_remedies() {
        let refusal = PurityRefusal::Overloaded {
            load_1m: 3.0,
            cpu_count: 8,
            ceiling: 2.0,
        };
        let text = refusal.text();
        assert!(
            text.contains("3.00"),
            "the refusal carries the load: {text}"
        );
        assert!(
            text.contains("2.00"),
            "the refusal carries the ceiling: {text}"
        );
        assert!(
            text.contains("settle") || text.contains("Stop"),
            "the refusal names a remedy: {text}"
        );
        let unreadable = PurityRefusal::unmeasurable("the processor count", "no answer");
        assert!(
            unreadable.text().contains("cannot be judged"),
            "an unmeasurable fact refuses rather than passes: {}",
            unreadable.text()
        );
    }
}
