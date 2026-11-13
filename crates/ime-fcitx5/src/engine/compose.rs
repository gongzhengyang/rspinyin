//! A minimal compose fallback for hosts without dead-key support.
//!
//! Responsibility: remember a dead key when the host answers unsupported, then resolve
//! the next key against a small built-in table. The real host protocol remains
//! authoritative: this state is consulted only after
//! [`ComposeOutcome::Unsupported`](crate::engine::host::ComposeOutcome::Unsupported),
//! and its answers mirror the host's own three ways -- a dead key opening a sequence is
//! swallowed with no output, a resolved sequence commits its character once, and a
//! sequence that dies without a result commits its mark exactly once and hands the key
//! back. Nothing is ever committed twice for one gesture.
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

/// What an unparseable keysym degrades to when it must be committed.
///
/// The fallback commits the dead mark it holds whenever a sequence ends without a
/// result; a keysym outside the Unicode scalar range has no such mark, so the
/// replacement character stands in rather than the commit being skipped -- swallowing
/// input the user typed is worse than committing a visible placeholder.
const REPLACEMENT: char = '\u{fffd}';

/// One step the fallback asks of the router, the counterpart of the host's own
/// [`ComposeOutcome`](crate::engine::host::ComposeOutcome).
///
/// The ways a dead-key sequence can leave a key mirror the host protocol: commit and
/// stop, commit and let the key travel on, or swallow with no output at all. The last
/// answer, `Pass`, is the key the sequence never touched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComposeStep {
    /// Commits this character. The flag answers whether the key itself stops here:
    /// `true` for a resolved or replaced sequence, `false` when the key that ended the
    /// sequence keeps its own route -- the miss's single placeholder, and the mark a
    /// sequence cancelled by a non-character key leaves behind.
    Commit(char, bool),
    /// The key is consumed and the application sees nothing: a dead key opening a
    /// sequence, which is the host's `Swallowed` semantics.
    Swallow,
    /// The sequence did not touch this key; it routes as usual.
    Pass,
}

/// One completion of a dead mark: the letter that follows it and the character the
/// pair produces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ComposeKey {
    letter: char,
    result: char,
}

/// The completions of the grave mark.
const GRAVE_ROWS: &[ComposeKey] = &[
    ComposeKey {
        letter: 'A',
        result: 'À',
    },
    ComposeKey {
        letter: 'E',
        result: 'È',
    },
    ComposeKey {
        letter: 'I',
        result: 'Ì',
    },
    ComposeKey {
        letter: 'O',
        result: 'Ò',
    },
    ComposeKey {
        letter: 'U',
        result: 'Ù',
    },
    ComposeKey {
        letter: 'a',
        result: 'à',
    },
    ComposeKey {
        letter: 'e',
        result: 'è',
    },
    ComposeKey {
        letter: 'i',
        result: 'ì',
    },
    ComposeKey {
        letter: 'o',
        result: 'ò',
    },
    ComposeKey {
        letter: 'u',
        result: 'ù',
    },
];

/// The completions of the acute mark.
const ACUTE_ROWS: &[ComposeKey] = &[
    ComposeKey {
        letter: 'A',
        result: 'Á',
    },
    ComposeKey {
        letter: 'E',
        result: 'É',
    },
    ComposeKey {
        letter: 'I',
        result: 'Í',
    },
    ComposeKey {
        letter: 'O',
        result: 'Ó',
    },
    ComposeKey {
        letter: 'U',
        result: 'Ú',
    },
    ComposeKey {
        letter: 'Y',
        result: 'Ý',
    },
    ComposeKey {
        letter: 'a',
        result: 'á',
    },
    ComposeKey {
        letter: 'e',
        result: 'é',
    },
    ComposeKey {
        letter: 'i',
        result: 'í',
    },
    ComposeKey {
        letter: 'o',
        result: 'ó',
    },
    ComposeKey {
        letter: 'u',
        result: 'ú',
    },
    ComposeKey {
        letter: 'y',
        result: 'ý',
    },
];

/// The completions of the circumflex mark.
const CIRCUMFLEX_ROWS: &[ComposeKey] = &[
    ComposeKey {
        letter: 'A',
        result: 'Â',
    },
    ComposeKey {
        letter: 'E',
        result: 'Ê',
    },
    ComposeKey {
        letter: 'I',
        result: 'Î',
    },
    ComposeKey {
        letter: 'O',
        result: 'Ô',
    },
    ComposeKey {
        letter: 'U',
        result: 'Û',
    },
    ComposeKey {
        letter: 'a',
        result: 'â',
    },
    ComposeKey {
        letter: 'e',
        result: 'ê',
    },
    ComposeKey {
        letter: 'i',
        result: 'î',
    },
    ComposeKey {
        letter: 'o',
        result: 'ô',
    },
    ComposeKey {
        letter: 'u',
        result: 'û',
    },
];

/// The completions of the diaeresis mark.
const DIAERESIS_ROWS: &[ComposeKey] = &[
    ComposeKey {
        letter: 'A',
        result: 'Ä',
    },
    ComposeKey {
        letter: 'E',
        result: 'Ë',
    },
    ComposeKey {
        letter: 'I',
        result: 'Ï',
    },
    ComposeKey {
        letter: 'O',
        result: 'Ö',
    },
    ComposeKey {
        letter: 'U',
        result: 'Ü',
    },
    ComposeKey {
        letter: 'Y',
        result: 'Ÿ',
    },
    ComposeKey {
        letter: 'a',
        result: 'ä',
    },
    ComposeKey {
        letter: 'e',
        result: 'ë',
    },
    ComposeKey {
        letter: 'i',
        result: 'ï',
    },
    ComposeKey {
        letter: 'o',
        result: 'ö',
    },
    ComposeKey {
        letter: 'u',
        result: 'ü',
    },
    ComposeKey {
        letter: 'y',
        result: 'ÿ',
    },
];

/// The completions of the tilde mark.
const TILDE_ROWS: &[ComposeKey] = &[
    ComposeKey {
        letter: 'A',
        result: 'Ã',
    },
    ComposeKey {
        letter: 'N',
        result: 'Ñ',
    },
    ComposeKey {
        letter: 'O',
        result: 'Õ',
    },
    ComposeKey {
        letter: 'a',
        result: 'ã',
    },
    ComposeKey {
        letter: 'n',
        result: 'ñ',
    },
    ComposeKey {
        letter: 'o',
        result: 'õ',
    },
];

/// The completions of the cedilla mark.
const CEDILLA_ROWS: &[ComposeKey] = &[
    ComposeKey {
        letter: 'C',
        result: 'Ç',
    },
    ComposeKey {
        letter: 'c',
        result: 'ç',
    },
];

/// Resolves one dead mark against the letter that followed it.
///
/// The first character dispatches: the mark picks its bucket in one `match` step, so a
/// lookup never crosses a mark it does not belong to, and every bucket is a dozen rows
/// or fewer. The whole query is therefore a small constant no matter how many rows the
/// table grows to in total.
///
/// # Panics
///
/// Never.
fn lookup(dead: u32, letter: char) -> Option<char> {
    let rows = match dead {
        GRAVE => GRAVE_ROWS,
        ACUTE => ACUTE_ROWS,
        CIRCUMFLEX => CIRCUMFLEX_ROWS,
        DIAERESIS => DIAERESIS_ROWS,
        TILDE => TILDE_ROWS,
        CEDILLA => CEDILLA_ROWS,
        _ => return None,
    };
    rows.iter()
        .find(|row| row.letter == letter)
        .map(|row| row.result)
}

/// The mark a keysym commits as, when a sequence ends without a result.
///
/// # Panics
///
/// Never.
fn mark_of(sym: u32) -> char {
    char::from_u32(sym).unwrap_or(REPLACEMENT)
}

/// Whether a keysym is one of the fallback's dead marks.
fn is_dead_key(sym: u32) -> bool {
    matches!(
        sym,
        GRAVE | ACUTE | CIRCUMFLEX | DIAERESIS | TILDE | CEDILLA
    )
}

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
    /// A dead key opens the sequence and is swallowed with no output, which is the
    /// host's own `Swallowed` semantics: nothing reaches the application until the
    /// sequence resolves. The next key resolves it. A hit commits the composed
    /// character and stops the key. A miss commits the dead mark once -- the single
    /// placeholder the failed sequence owed -- and leaves the current key to normal
    /// routing, so `´x` reads `´x` and not `´´x`. A second dead key commits the
    /// previous mark and takes its place, so `´` then `` ` `` then `a` reads `´à`. A
    /// key that is no character at all cancels the sequence the same way a miss does:
    /// the mark stands in once and the key keeps its own route.
    ///
    /// # Arguments
    ///
    /// * `key` - the event the host could not compose.
    ///
    /// # Returns
    ///
    /// The step the router takes, as [`ComposeStep`].
    ///
    /// # Panics
    ///
    /// Never.
    pub fn process(&mut self, key: &FcitxKeyEvent) -> ComposeStep {
        if key.is_release || key.state != 0 {
            return ComposeStep::Pass;
        }
        let Some(dead) = self.pending.take() else {
            return if is_dead_key(key.sym) {
                self.pending = Some(key.sym);
                ComposeStep::Swallow
            } else {
                ComposeStep::Pass
            };
        };
        let Some(character) = char::from_u32(key.sym) else {
            return ComposeStep::Commit(mark_of(dead), false);
        };
        if is_dead_key(key.sym) {
            self.pending = Some(key.sym);
            return ComposeStep::Commit(mark_of(dead), true);
        }
        match lookup(dead, character) {
            Some(result) => ComposeStep::Commit(result, true),
            None => ComposeStep::Commit(mark_of(dead), false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ACUTE, CEDILLA, CIRCUMFLEX, ComposeFallback, ComposeStep, DIAERESIS, GRAVE, TILDE, lookup,
    };
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
        // The opening dead key is swallowed with no output, the host's own semantics.
        assert_eq!(fallback.process(&press(0xb4)), ComposeStep::Swallow);
        assert_eq!(
            fallback.process(&press(b'e' as u32)),
            ComposeStep::Commit('é', true)
        );
        assert_eq!(fallback.process(&press(b'x' as u32)), ComposeStep::Pass);
    }

    #[test]
    fn test_process_miss_replays_letter_for_normal_routing() {
        let mut fallback = ComposeFallback::new();
        assert_eq!(fallback.process(&press(0xa8)), ComposeStep::Swallow);
        // One placeholder, and the letter keeps its route: the application reads x.
        assert_eq!(
            fallback.process(&press(b'x' as u32)),
            ComposeStep::Commit('¨', false)
        );
        assert_eq!(fallback.process(&press(b'x' as u32)), ComposeStep::Pass);
    }

    #[test]
    fn test_process_release_or_modifier_ignores_pending() {
        let mut fallback = ComposeFallback::new();
        assert_eq!(fallback.process(&press(0x7e)), ComposeStep::Swallow);
        let mut release = press(b'n' as u32);
        release.is_release = true;
        assert_eq!(fallback.process(&release), ComposeStep::Pass);
        let mut modified = press(b'n' as u32);
        modified.state = 1;
        assert_eq!(fallback.process(&modified), ComposeStep::Pass);
        // The pending mark survived both edges and resolves on the plain press.
        assert_eq!(
            fallback.process(&press(b'n' as u32)),
            ComposeStep::Commit('ñ', true)
        );
    }

    #[test]
    fn test_process_uppercase_letter_composes_the_uppercase_character() {
        let mut fallback = ComposeFallback::new();
        assert_eq!(fallback.process(&press(0xb4)), ComposeStep::Swallow);
        assert_eq!(
            fallback.process(&press(b'E' as u32)),
            ComposeStep::Commit('É', true)
        );
    }

    #[test]
    fn test_process_second_dead_key_replaces_pending_and_commits_the_previous_mark() {
        let mut fallback = ComposeFallback::new();
        assert_eq!(fallback.process(&press(0xb4)), ComposeStep::Swallow);
        // The grave takes the acute's place, and the acute's mark is committed once:
        // the application reads ´ before whatever the grave goes on to compose.
        assert_eq!(
            fallback.process(&press(0x60)),
            ComposeStep::Commit('´', true)
        );
        assert_eq!(
            fallback.process(&press(b'a' as u32)),
            ComposeStep::Commit('à', true)
        );
    }

    #[test]
    fn test_process_function_key_cancels_sequence_and_keeps_its_own_route() {
        let mut fallback = ComposeFallback::new();
        assert_eq!(fallback.process(&press(0xa8)), ComposeStep::Swallow);
        // A keysym that is no character at all cannot complete a sequence: the mark
        // stands in once, the sequence ends, and the function key travels on.
        assert_eq!(
            fallback.process(&press(0xffbe)),
            ComposeStep::Commit('¨', false)
        );
        assert_eq!(fallback.process(&press(b'c' as u32)), ComposeStep::Pass);
    }

    #[test]
    fn test_reset_clears_pending_dead_key() {
        let mut fallback = ComposeFallback::new();
        assert_eq!(fallback.process(&press(0xb8)), ComposeStep::Swallow);
        fallback.reset();
        assert_eq!(fallback.process(&press(b'c' as u32)), ComposeStep::Pass);
    }

    #[test]
    fn test_lookup_resolves_every_builtin_row() {
        // Every row the built-in buckets declare must resolve through the dispatched
        // lookup, which is the guarantee that the per-mark buckets hold the same data
        // the table's rows spell out.
        let buckets = [
            (GRAVE, super::GRAVE_ROWS),
            (ACUTE, super::ACUTE_ROWS),
            (CIRCUMFLEX, super::CIRCUMFLEX_ROWS),
            (DIAERESIS, super::DIAERESIS_ROWS),
            (TILDE, super::TILDE_ROWS),
            (CEDILLA, super::CEDILLA_ROWS),
        ];
        for (mark, rows) in buckets {
            for row in rows {
                assert_eq!(lookup(mark, row.letter), Some(row.result));
            }
        }
    }

    #[test]
    fn test_lookup_unknown_mark_or_letter_answers_none() {
        assert_eq!(lookup(b'x' as u32, 'e'), None, "not a dead mark");
        assert_eq!(lookup(ACUTE, 'z'), None, "a letter no row completes");
    }
}
