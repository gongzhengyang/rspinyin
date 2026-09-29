//! Modifier keys: the bit definitions this build was compiled against, and the check that
//! the host agrees with them.
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

use ime_types::ImeError;

use super::MODIFIER_MASK;
use crate::ffi::emit_diagnostic;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_modifier_mask_accepts_the_compiled_mask_silently() {
        let mut reported: Vec<String> = Vec::new();
        let result = check_modifier_mask_with(MODIFIER_MASK, |code| {
            reported.push(String::from(code));
        });
        assert!(result.is_ok(), "the compiled mask is the one the host has");
        assert!(
            reported.is_empty(),
            "a matching host must record nothing: {reported:?}"
        );
    }

    #[test]
    fn test_check_modifier_mask_reports_a_host_that_added_a_bit() {
        let mut reported: Vec<String> = Vec::new();
        let result = check_modifier_mask_with(MODIFIER_MASK | (1 << 1), |code| {
            reported.push(String::from(code));
        });
        assert_eq!(reported, vec![String::from(MODIFIER_MASK_MISMATCH_CODE)]);
        let error = result.expect_err("a renumbered host is an error");
        assert!(
            error
                .to_string()
                .starts_with("platform/fcitx5/version-mismatch"),
            "the error renders as a frozen code: {error}"
        );
    }

    #[test]
    fn test_check_modifier_mask_reports_a_host_that_dropped_a_bit() {
        // Bit 0 is Shift, the bit of the mask the composing keymap reads most often.
        let mut reported: Vec<String> = Vec::new();
        let result = check_modifier_mask_with(MODIFIER_MASK & !(1 << 0), |code| {
            reported.push(String::from(code));
        });
        assert_eq!(reported, vec![String::from(MODIFIER_MASK_MISMATCH_CODE)]);
        assert!(result.is_err(), "a mask without Shift is not this build's");
    }

    #[test]
    fn test_check_modifier_mask_reports_an_empty_host_mask() {
        // The boundary: a host that reports nothing at all is the furthest a host can be
        // from this build, and it must be reported rather than read as "no modifiers".
        let mut reported: Vec<String> = Vec::new();
        let result = check_modifier_mask_with(0, |code| {
            reported.push(String::from(code));
        });
        assert_eq!(reported, vec![String::from(MODIFIER_MASK_MISMATCH_CODE)]);
        assert!(result.is_err());
    }
}
