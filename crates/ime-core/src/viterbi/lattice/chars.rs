//! The character count one lattice edge carries.
//!
//! Responsibility: answer how many characters a word has, which is the quantity the
//! length bonus of an edge score is computed from.
//!
//! Boundaries: this module counts and nothing else. It does not know what a lattice is,
//! it never reads the dictionary, and it is pure -- no file, no clock, no environment, no
//! global state.
//!
//! # Why the count is a byte scan
//!
//! The straightforward spelling of this is `text.chars().count()`, and it is what the
//! lattice used to run once per edge. `str::chars` decodes UTF-8: every byte goes
//! through the decoder's branchy state machine and yields a `char` that is then thrown
//! away, because all that is wanted is how many there were.
//!
//! In UTF-8 a character is exactly one byte that is not a continuation byte, so counting
//! the bytes that are not `0b10xx_xxxx` answers the same number with a flat byte loop
//! that vectorizes. The two definitions agree on every valid encoding, and the input is a
//! `&str` -- which Rust guarantees is valid UTF-8, and which the string pool is checked
//! to be when the compiler writes it -- so the scan is exact rather than an estimate.
//!
//! # Where the count comes from
//!
//! The compiler knows the count when it writes an entry and stores it in the record's
//! `char_count` field, which is the cheapest possible source: no scan at all. A word
//! reaches the lattice as a [`WordRef`](ime_types::WordRef) instead, and every field of
//! that type is frozen contract, so a decode cannot see the record's count without a
//! contract change; the scan below is what it pays until then. The counting is confined
//! to this module so that the change is one call site rather than a search.

/// Returns how many characters `text` has, saturating at [`u16::MAX`].
///
/// The saturation is what the edge field can hold. A word the dictionary can return is
/// far shorter than that -- the container's own record caps a word's text at 96 bytes --
/// so a text long enough to saturate cannot come out of a dictionary, and a caller that
/// hands one in gets a count rather than a panic.
///
/// # Panics
///
/// Never: the scan is a byte loop and the conversion saturates.
pub(super) fn count_characters(text: &str) -> u16 {
    // A continuation byte is `0b10xx_xxxx`; every other byte starts a character.
    let characters = text
        .as_bytes()
        .iter()
        .filter(|byte| **byte & 0b1100_0000 != 0b1000_0000)
        .count();
    u16::try_from(characters).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Texts the agreement test walks: ASCII, Han, a four-byte character, a combining
    /// mark, and the empty string.
    const SAMPLES: [&str; 7] = [
        "",
        "a",
        "abc",
        "中国",
        "a中b",
        "\u{1f600}",
        "e\u{301}\u{4e2d}\u{1f600}",
    ];

    #[test]
    fn test_count_characters_counts_code_points_not_bytes() {
        assert_eq!(count_characters(""), 0);
        assert_eq!(count_characters("abc"), 3);
        assert_eq!(count_characters("中国"), 2, "three bytes per character");
        assert_eq!(count_characters("a中b"), 3);
        assert_eq!(count_characters("\u{1f600}"), 1, "four bytes per character");
    }

    #[test]
    fn test_count_characters_agrees_with_the_decoder_on_every_sample() {
        // The scan is an optimization, not a different definition: the number it answers
        // has to be the number `chars()` yields, including for the encodings where a
        // character is not one byte.
        for sample in SAMPLES {
            assert_eq!(
                count_characters(sample),
                u16::try_from(sample.chars().count()).unwrap_or(u16::MAX),
                "{sample:?}"
            );
        }
    }

    #[test]
    fn test_count_characters_saturates_instead_of_wrapping() {
        // A text longer than the field can hold answers the field's ceiling rather than a
        // wrapped count: a word that long is out of contract, and a wrong number is worse
        // than a saturated one.
        let long = "a".repeat(usize::from(u16::MAX) + 8);
        assert_eq!(count_characters(&long), u16::MAX);
        let boundary = "a".repeat(usize::from(u16::MAX));
        assert_eq!(count_characters(&boundary), u16::MAX);
    }
}
