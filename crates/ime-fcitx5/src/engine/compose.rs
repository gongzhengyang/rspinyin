//! A minimal compose fallback for hosts without dead-key support.
//!
//! Responsibility: remember a dead key when the host answers unsupported, then resolve
//! the next printable key against a small built-in table. The real host protocol remains
//! authoritative: this state is consulted only after
//! [`ComposeOutcome::Unsupported`](crate::engine::host::ComposeOutcome::Unsupported).
//!
//! Boundaries: the fallback holds no input-context identity. One composing dead key at
//! a time is enough for the physical keyboard path that reaches the router, and a reset
//! returns to the host's unsupported state without an error or a diagnostic.

use crate::ffi::FcitxKeyEvent;

const GRAVE: u32 = 0x60;
const ACUTE: u32 = 0xb4;
const CIRCUMFLEX: u32 = 0x5e;
const DIAERESIS: u32 = 0xa8;
const TILDE: u32 = 0x7e;
const CEDILLA: u32 = 0xb8;

/// A dead key and the letter that completes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ComposeKey {
    dead: u32,
    letter: char,
    result: char,
}

/// The fallback rows, ordered by dead key for a readable review.
///
/// Rows cover Latin letters only and deliberately include both letter cases. An
/// unmatched second key falls back to the dead mark itself, matching the least
/// surprising host behaviour.
const ROWS: &[ComposeKey] = &[
    ComposeKey {
        dead: GRAVE,
        letter: 'A',
        result: 'À',
    },
    ComposeKey {
        dead: GRAVE,
        letter: 'E',
        result: 'È',
    },
    ComposeKey {
        dead: GRAVE,
        letter: 'I',
        result: 'Ì',
    },
    ComposeKey {
        dead: GRAVE,
        letter: 'O',
        result: 'Ò',
    },
    ComposeKey {
        dead: GRAVE,
        letter: 'U',
        result: 'Ù',
    },
    ComposeKey {
        dead: GRAVE,
        letter: 'a',
        result: 'à',
    },
    ComposeKey {
        dead: GRAVE,
        letter: 'e',
        result: 'è',
    },
    ComposeKey {
        dead: GRAVE,
        letter: 'i',
        result: 'ì',
    },
    ComposeKey {
        dead: GRAVE,
        letter: 'o',
        result: 'ò',
    },
    ComposeKey {
        dead: GRAVE,
        letter: 'u',
        result: 'ù',
    },
    ComposeKey {
        dead: ACUTE,
        letter: 'A',
        result: 'Á',
    },
    ComposeKey {
        dead: ACUTE,
        letter: 'E',
        result: 'É',
    },
    ComposeKey {
        dead: ACUTE,
        letter: 'I',
        result: 'Í',
    },
    ComposeKey {
        dead: ACUTE,
        letter: 'O',
        result: 'Ó',
    },
    ComposeKey {
        dead: ACUTE,
        letter: 'U',
        result: 'Ú',
    },
    ComposeKey {
        dead: ACUTE,
        letter: 'Y',
        result: 'Ý',
    },
    ComposeKey {
        dead: ACUTE,
        letter: 'a',
        result: 'á',
    },
    ComposeKey {
        dead: ACUTE,
        letter: 'e',
        result: 'é',
    },
    ComposeKey {
        dead: ACUTE,
        letter: 'i',
        result: 'í',
    },
    ComposeKey {
        dead: ACUTE,
        letter: 'o',
        result: 'ó',
    },
    ComposeKey {
        dead: ACUTE,
        letter: 'u',
        result: 'ú',
    },
    ComposeKey {
        dead: ACUTE,
        letter: 'y',
        result: 'ý',
    },
    ComposeKey {
        dead: CIRCUMFLEX,
        letter: 'A',
        result: 'Â',
    },
    ComposeKey {
        dead: CIRCUMFLEX,
        letter: 'E',
        result: 'Ê',
    },
    ComposeKey {
        dead: CIRCUMFLEX,
        letter: 'I',
        result: 'Î',
    },
    ComposeKey {
        dead: CIRCUMFLEX,
        letter: 'O',
        result: 'Ô',
    },
    ComposeKey {
        dead: CIRCUMFLEX,
        letter: 'U',
        result: 'Û',
    },
    ComposeKey {
        dead: CIRCUMFLEX,
        letter: 'a',
        result: 'â',
    },
    ComposeKey {
        dead: CIRCUMFLEX,
        letter: 'e',
        result: 'ê',
    },
    ComposeKey {
        dead: CIRCUMFLEX,
        letter: 'i',
        result: 'î',
    },
    ComposeKey {
        dead: CIRCUMFLEX,
        letter: 'o',
        result: 'ô',
    },
    ComposeKey {
        dead: CIRCUMFLEX,
        letter: 'u',
        result: 'û',
    },
    ComposeKey {
        dead: DIAERESIS,
        letter: 'A',
        result: 'Ä',
    },
    ComposeKey {
        dead: DIAERESIS,
        letter: 'E',
        result: 'Ë',
    },
    ComposeKey {
        dead: DIAERESIS,
        letter: 'I',
        result: 'Ï',
    },
    ComposeKey {
        dead: DIAERESIS,
        letter: 'O',
        result: 'Ö',
    },
    ComposeKey {
        dead: DIAERESIS,
        letter: 'U',
        result: 'Ü',
    },
    ComposeKey {
        dead: DIAERESIS,
        letter: 'Y',
        result: 'Ÿ',
    },
    ComposeKey {
        dead: DIAERESIS,
        letter: 'a',
        result: 'ä',
    },
    ComposeKey {
        dead: DIAERESIS,
        letter: 'e',
        result: 'ë',
    },
    ComposeKey {
        dead: DIAERESIS,
        letter: 'i',
        result: 'ï',
    },
    ComposeKey {
        dead: DIAERESIS,
        letter: 'o',
        result: 'ö',
    },
    ComposeKey {
        dead: DIAERESIS,
        letter: 'u',
        result: 'ü',
    },
    ComposeKey {
        dead: DIAERESIS,
        letter: 'y',
        result: 'ÿ',
    },
    ComposeKey {
        dead: TILDE,
        letter: 'A',
        result: 'Ã',
    },
    ComposeKey {
        dead: TILDE,
        letter: 'N',
        result: 'Ñ',
    },
    ComposeKey {
        dead: TILDE,
        letter: 'O',
        result: 'Õ',
    },
    ComposeKey {
        dead: TILDE,
        letter: 'a',
        result: 'ã',
    },
    ComposeKey {
        dead: TILDE,
        letter: 'n',
        result: 'ñ',
    },
    ComposeKey {
        dead: TILDE,
        letter: 'o',
        result: 'õ',
    },
    ComposeKey {
        dead: CEDILLA,
        letter: 'C',
        result: 'Ç',
    },
    ComposeKey {
        dead: CEDILLA,
        letter: 'c',
        result: 'ç',
    },
];

/// Fallback compose state for one key-routing stream.
#[derive(Default)]
pub struct ComposeFallback {
    pending: Option<u32>,
}

impl ComposeFallback {
    /// Creates an inactive fallback.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn new() -> Self {
        Self { pending: None }
    }

    /// Clears a pending dead key.
    ///
    /// Context boundaries and host-supported compose turns make stale fallback state
    /// dangerous; both reset the state explicitly.
    pub fn reset(&mut self) {
        self.pending = None;
    }

    /// Processes one key when the host cannot compose.
    ///
    /// A dead key opens the sequence. The next printable key resolves it: a hit
    /// commits the composed character, and a miss commits the dead mark and leaves the
    /// current key to normal routing. A second dead key replaces the first, also
    /// committing the previous mark so no input is silently swallowed.
    ///
    /// # Arguments
    ///
    /// * `key` - the event the host could not compose.
    ///
    /// # Returns
    ///
    /// `Some` with a character to commit and whether the router should stop routing the
    /// current key, or `None` to route it normally.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn process(&mut self, key: &FcitxKeyEvent) -> Option<(char, bool)> {
        if key.is_release || key.state != 0 {
            return None;
        }
        let Some(dead) = self.pending.take() else {
            return if is_dead_key(key.sym) {
                self.pending = Some(key.sym);
                Some((char::from_u32(key.sym).unwrap_or('\u{fffd}'), true))
            } else {
                None
            };
        };
        let Some(character) = char::from_u32(key.sym) else {
            return Some((char::from_u32(dead).unwrap_or('\u{fffd}'), true));
        };
        if is_dead_key(key.sym) {
            self.pending = Some(key.sym);
            return Some((char::from_u32(dead).unwrap_or('\u{fffd}'), true));
        }
        match ROWS
            .iter()
            .find(|row| row.dead == dead && row.letter == character)
        {
            Some(row) => Some((row.result, true)),
            None => Some((char::from_u32(dead).unwrap_or('\u{fffd}'), false)),
        }
    }
}

/// Whether a keysym is one of the fallback's dead marks.
fn is_dead_key(sym: u32) -> bool {
    matches!(
        sym,
        GRAVE | ACUTE | CIRCUMFLEX | DIAERESIS | TILDE | CEDILLA
    )
}

#[cfg(test)]
mod tests {
    use super::ComposeFallback;
    use crate::ffi::FcitxKeyEvent;

    fn press(sym: u32) -> FcitxKeyEvent {
        FcitxKeyEvent {
            sym,
            state: 0,
            is_release: false,
            time_ms: 0,
        }
    }

    #[test]
    fn test_process_dead_then_letter_composes_character() {
        let mut fallback = ComposeFallback::new();
        assert_eq!(fallback.process(&press(0xb4)), Some(('´', true)));
        assert_eq!(fallback.process(&press(b'e' as u32)), Some(('é', true)));
        assert_eq!(fallback.process(&press(b'x' as u32)), None);
    }

    #[test]
    fn test_process_miss_replays_letter_for_normal_routing() {
        let mut fallback = ComposeFallback::new();
        assert_eq!(fallback.process(&press(0xa8)), Some(('¨', true)));
        assert_eq!(fallback.process(&press(b'x' as u32)), Some(('¨', false)));
        assert_eq!(fallback.process(&press(b'x' as u32)), None);
    }

    #[test]
    fn test_process_release_or_modifier_ignores_pending() {
        let mut fallback = ComposeFallback::new();
        assert_eq!(fallback.process(&press(0x7e)), Some(('~', true)));
        let mut release = press(b'n' as u32);
        release.is_release = true;
        assert_eq!(fallback.process(&release), None);
        let mut modified = press(b'n' as u32);
        modified.state = 1;
        assert_eq!(fallback.process(&modified), None);
        assert_eq!(fallback.process(&press(b'n' as u32)), Some(('ñ', true)));
    }

    #[test]
    fn test_reset_clears_pending_dead_key() {
        let mut fallback = ComposeFallback::new();
        assert_eq!(fallback.process(&press(0xb8)), Some(('¸', true)));
        fallback.reset();
        assert_eq!(fallback.process(&press(b'c' as u32)), None);
    }
}
