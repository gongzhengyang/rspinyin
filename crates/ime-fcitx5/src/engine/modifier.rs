//! Modifier keys: the bits this build was compiled against, the check that the host agrees
//! with them, and the machine that tracks a modifier the user is holding.
//!
//! # Why a run-time check
//!
//! The bits in [`MODIFIER_MASK`](super::MODIFIER_MASK) are Fcitx5's, copied from the
//! installed `fcitx-utils/keysym.h`. The unit test beside that constant pins them against
//! the header they came from, which is a build-time tripwire: it catches a renumbered host
//! in CI and says nothing at all when the *running* host differs from the one the build was
//! made against. This module is the run-time half. The addon asks the host for its mask
//! once, at load, and reports `platform/modifier-mask-mismatch` when the two disagree, so a
//! renumbered host shows up in the log instead of silently misreading every chord.
//!
//! The check is an equality rather than a subset test, because every bit of the mask is
//! read by the routing table: a host that adds one or drops one changes what each chord
//! means, and there is no bit of the mask the plugin does not look at.
//!
//! # Holding a modifier
//!
//! The shortcut table gives the held `Shift` key a behaviour of its own: hold it and the
//! input mode switches to English for as long as it is down, release it and the mode comes
//! back. [`KeyAction`](ime_types::KeyAction) is a frozen contract with no variant for a
//! modifier edge, and the behaviour is a host-layer mode switch rather than something a
//! session does, so the hold is engine state and it is tracked here.
//!
//! [`ModifierHold`] is driven by the events the host delivers and by nothing else. A press
//! arms it, a release closes it, and the elapsed time is the difference of the two
//! timestamps the events carry. Nothing here reads a clock and nothing wakes up on its
//! own: a polling loop is forbidden, and there is nothing to poll for, because a modifier
//! that goes down and comes up is exactly one event pair and the duration is known the
//! moment the release is answered. The machine is therefore a pure function of the events
//! handed to it, which is what makes every case below a deterministic test with no sleep
//! in it.
//!
//! A release means one of three things, and [`ModifierHold::release`] decides which:
//!
//! * The modifier was part of a chord or of a typed key — a capital letter, `Shift+Space`.
//!   [`ModifierHold::mark_used`] records that, and the release is a no-op: a modifier the
//!   user typed with must not also switch the mode.
//! * The modifier was held past [`HOLD_THRESHOLD_MS`] with nothing typed. That is a long
//!   press, which is the gesture the cheat sheet answers to.
//! * Anything else is a mode switch, and the release carries the enabled state the press
//!   interrupted so that the caller can put it back.

use ime_types::ImeError;

use super::{KEY_SHIFT_L, KEY_SHIFT_R, MODIFIER_MASK};
use crate::ffi::emit_diagnostic;

#[cfg(test)]
mod tests;

/// The stable diagnostic code for a host whose modifier bits differ from this build's.
///
/// Reported once at load. It is a developer-facing condition, never shown to the user: it
/// means the host the addon is running against is not the one it was built against.
pub const MODIFIER_MASK_MISMATCH_CODE: &str = "platform/modifier-mask-mismatch";

/// Checks the modifier bits the host reports against the ones this build was compiled
/// against, and reports the difference.
///
/// # Arguments
///
/// * `host_mask` — the host's `SimpleMask`: every bit that means "a modifier the user
///   holds deliberately". The glue reads it from the host and passes it here once.
///
/// # Returns
///
/// `Ok(())` when the two agree. On a difference the precise code
/// [`MODIFIER_MASK_MISMATCH_CODE`] is recorded on the diagnostic channel and the
/// difference comes back as an error.
///
/// # Errors
///
/// [`ImeError::Fcitx5VersionMismatch`] when `host_mask` differs from the compiled mask.
/// That rendering is the closest the frozen error model has to a host whose platform
/// layout is not the one this build was made for; the precise code travels on the
/// diagnostic channel, which is where a code with no variant of its own is recorded.
///
/// # Panics
///
/// Never.
pub fn check_modifier_mask(host_mask: u32) -> Result<(), ImeError> {
    check_modifier_mask_with(host_mask, emit_diagnostic)
}

/// The body of [`check_modifier_mask`] over a caller-supplied sink.
///
/// The sink is a parameter so that the tests can assert the property that keeps this check
/// out of the log on every ordinary start — a matching mask records nothing at all —
/// without going through the process-wide diagnostic throttle.
fn check_modifier_mask_with(host_mask: u32, mut report: impl FnMut(&str)) -> Result<(), ImeError> {
    if host_mask == MODIFIER_MASK {
        return Ok(());
    }
    report(MODIFIER_MASK_MISMATCH_CODE);
    Err(ImeError::Fcitx5VersionMismatch {
        host: format!("{host_mask:#010x}"),
        required: format!("{MODIFIER_MASK:#010x}"),
    })
}

// ── The modifier hold ────────────────────────────────────────────────────────────

/// How long a modifier must be held, with nothing typed, to count as a long press.
///
/// The threshold is read when the release arrives rather than watched by a timer. A
/// polling loop is forbidden, and there is nothing to poll for: the hold is one press and
/// one release, so the elapsed time is already known at the moment the release is
/// answered.
pub const HOLD_THRESHOLD_MS: u32 = 250;

/// A modifier key whose hold has a behaviour of its own.
///
/// One variant, because the shortcut table gives exactly one modifier a hold: the held
/// `Shift` is the temporary Chinese / English switch. The type exists rather than a bare
/// unit so that a second held modifier becomes a variant here rather than a second machine
/// beside this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierKey {
    /// `Shift_L` or `Shift_R`. The two keys are one modifier: the shortcut table names them
    /// as one gesture, and the routing table does not tell them apart either.
    Shift,
}

impl ModifierKey {
    /// The modifier a keysym names, if it names one.
    ///
    /// The keysym half of the question [`is_shift_press`](super::is_shift_press) asks of a
    /// whole event. The two are the same predicate written twice, so the tests pin their
    /// answers together over a keysym corpus and a row cannot drift from the other.
    ///
    /// # Arguments
    ///
    /// * `sym` — the XKB keysym of the event, as `FcitxKeyEvent::sym` carries it.
    ///
    /// # Returns
    ///
    /// The modifier, or `None` for every key that is not one — which is almost every key.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn from_sym(sym: u32) -> Option<Self> {
        match sym {
            KEY_SHIFT_L | KEY_SHIFT_R => Some(Self::Shift),
            _ => None,
        }
    }
}

/// What a modifier release means.
///
/// The three answers are what the caller can do about a release, which is why the machine
/// answers with one of them rather than with a flag: a hold that changed nothing, a hold
/// that has to be undone, and a hold that was a gesture of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldOutcome {
    /// The hold changed nothing and the release is a no-op.
    Nothing,
    /// The hold was a mode switch, and the release puts the interrupted state back.
    Restore {
        /// Whether the input method was enabled when the modifier went down.
        was_enabled: bool,
    },
    /// The modifier was held with nothing typed: a long press.
    LongPress {
        /// How long the modifier was down, in milliseconds.
        held_ms: u32,
    },
}

/// One held modifier and what the press interrupted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HeldModifier {
    /// The modifier the hold started with.
    key: ModifierKey,
    /// The host timestamp of the first press, for the long-press test.
    pressed_at_ms: u32,
    /// Whether the input method was enabled when the hold started.
    was_enabled: bool,
    /// Whether any key was acted on while the modifier was down.
    used: bool,
}

/// The modifier the user is holding, and what happened while it was down.
///
/// Empty until a modifier press arms it, and empty again once the release has been
/// answered. See the module documentation for what the three release answers mean.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModifierHold {
    /// The modifier currently held, if any.
    armed: Option<HeldModifier>,
}

impl ModifierHold {
    /// An empty hold.
    ///
    /// # Returns
    ///
    /// A hold that is not tracking any modifier, which is the same value
    /// [`ModifierHold::default`] produces.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn new() -> Self {
        Self { armed: None }
    }

    /// Records a modifier press and the state it interrupted.
    ///
    /// A press of the modifier that is already armed leaves the hold as it is rather than
    /// restarting it. The frontend repeats a press while a key is held down, so a hold that
    /// restarted would measure the long press from the last repeat — which never stops
    /// arriving — and would forget that a key was typed in between.
    ///
    /// # Arguments
    ///
    /// * `key` — the modifier the pressed key names.
    /// * `at_ms` — the host timestamp of the press.
    /// * `was_enabled` — whether the input method was enabled at that moment.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn arm(&mut self, key: ModifierKey, at_ms: u32, was_enabled: bool) {
        if self.armed.is_some_and(|held| held.key == key) {
            return;
        }
        self.armed = Some(HeldModifier {
            key,
            pressed_at_ms: at_ms,
            was_enabled,
            used: false,
        });
    }

    /// Records that a key was acted on while the modifier was down.
    ///
    /// This is the whole judgement of "was this `Shift` a mode switch or a typed key": a
    /// modifier the user typed with is not one they meant to hold. Doing nothing when no
    /// modifier is down is the correct answer rather than an oversight — the mark is about
    /// a hold, and there is no hold to mark.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn mark_used(&mut self) {
        if let Some(held) = self.armed.as_mut() {
            held.used = true;
        }
    }

    /// Ends the hold and answers what the release should do.
    ///
    /// # Arguments
    ///
    /// * `key` — the modifier the released key names, or `None` for a key that names none.
    ///   The host delivers a release for every key, so this is asked of every one of them;
    ///   only the held modifier's own release ends the hold.
    /// * `at_ms` — the host timestamp of the release.
    ///
    /// # Returns
    ///
    /// [`HoldOutcome::Restore`] or [`HoldOutcome::LongPress`] for the release that ends a
    /// hold worth acting on, and [`HoldOutcome::Nothing`] for every other release: one of a
    /// key that names no modifier, one of a modifier that is not the held one, and one of a
    /// modifier the user typed with.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never: the elapsed time saturates rather than underflowing. The caller is an FFI
    /// entry point, which must not unwind into C++, and a host whose two timestamps are out
    /// of order — a machine suspended and resumed between the press and the release — is a
    /// difference of zero rather than a panic.
    pub fn release(&mut self, key: Option<ModifierKey>, at_ms: u32) -> HoldOutcome {
        let Some(held) = self.armed else {
            return HoldOutcome::Nothing;
        };
        if key != Some(held.key) {
            // Another key's release. The hold stays armed, because the modifier is still
            // down: the release edge of a letter typed with `Shift` held arrives while
            // `Shift` is still held.
            return HoldOutcome::Nothing;
        }
        self.armed = None;
        if held.used {
            return HoldOutcome::Nothing;
        }
        let held_ms = at_ms.saturating_sub(held.pressed_at_ms);
        if held_ms >= HOLD_THRESHOLD_MS {
            HoldOutcome::LongPress { held_ms }
        } else {
            HoldOutcome::Restore {
                was_enabled: held.was_enabled,
            }
        }
    }

    /// Forgets the hold without acting on it.
    ///
    /// For a hold whose release will never arrive, because the input context lost focus or
    /// the host reset it. The next modifier press starts a new hold rather than continuing
    /// one whose other end is gone.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn clear(&mut self) {
        self.armed = None;
    }
}
