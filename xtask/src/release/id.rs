//! The identifier one release carries.
//!
//! Responsibility: mint the ULID the release document records, so that a user reporting a
//! problem and a maintainer reading a build log are talking about the same release.
//!
//! # Why the identifier is minted here rather than passed in
//!
//! An identifier a caller supplies is an identifier that can disagree with the document that
//! carries it, and the document is the only place a user ever sees it. The pipeline reads it
//! back out of the manifest for its summary instead of telling the manifest what it is.
//!
//! # Why the ULID is written out rather than depended on
//!
//! The same reason `xtask package` gives: the `ulid` crate is the ecosystem default and would
//! be the right dependency for a program whose job is identifiers. This one mints exactly one
//! identifier per release, and the format is a fixed 26-character encoding over a fixed
//! alphabet, so a dependency would be one more crate in a build tool that needs a single
//! value.
//!
//! # Re-running a tag
//!
//! Two runs of the same tag produce different identifiers and identical artifacts. The
//! identifier names the *run*; `SOURCE_DATE_EPOCH` is what makes the bytes repeatable. An
//! identifier that repeated would be one that could not tell two runs apart, which is the
//! only thing it exists for.

use std::fs;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

/// Bytes of randomness in a ULID.
const ENTROPY_BYTES: usize = 10;

/// The system entropy source.
const ENTROPY_SOURCE: &str = "/dev/urandom";

/// Characters in a ULID: 48 bits of timestamp and 80 bits of randomness, five bits each.
const LENGTH: usize = 26;

/// The alphabet ULIDs are written in: Crockford base32, without `I`, `L`, `O` and `U`.
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A fresh identifier for this release.
///
/// # Errors
///
/// Returns an error when the system entropy source cannot be read, which would otherwise
/// produce an identifier only as unique as the clock.
///
/// # Panics
///
/// Never.
pub fn request_id() -> Result<String> {
    Ok(ulid(now_millis(), entropy()?))
}

/// A ULID: 48 bits of millisecond timestamp followed by 80 bits of randomness, in Crockford
/// base32, 26 characters long.
///
/// Sorting the identifiers still orders the releases by the millisecond they were made in,
/// which is the property the format exists for.
///
/// # Panics
///
/// Never.
pub fn ulid(timestamp_ms: u64, entropy: [u8; ENTROPY_BYTES]) -> String {
    let mut id = String::with_capacity(LENGTH);
    for index in 0..10 {
        let shift = 45 - 5 * index;
        id.push(CROCKFORD[((timestamp_ms >> shift) & 0x1F) as usize] as char);
    }
    let mut random: u128 = 0;
    for byte in entropy {
        random = (random << 8) | u128::from(byte);
    }
    for index in 0..16 {
        let shift = 75 - 5 * index;
        id.push(CROCKFORD[((random >> shift) & 0x1F) as usize] as char);
    }
    id
}

/// Ten bytes from the system entropy source.
///
/// # Errors
///
/// Returns an error when the source cannot be opened or does not yield its bytes.
///
/// # Panics
///
/// Never.
fn entropy() -> Result<[u8; ENTROPY_BYTES]> {
    let mut bytes = [0u8; ENTROPY_BYTES];
    let mut source = fs::File::open(ENTROPY_SOURCE)
        .with_context(|| format!("release: opening {ENTROPY_SOURCE} for the release identifier"))?;
    source
        .read_exact(&mut bytes)
        .with_context(|| format!("release: reading {ENTROPY_BYTES} bytes from {ENTROPY_SOURCE}"))?;
    Ok(bytes)
}

/// Milliseconds since the Unix epoch.
///
/// # Panics
///
/// Never.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An entropy value written out independently of the encoder under test.
    const ENTROPY: [u8; ENTROPY_BYTES] =
        [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A];

    /// Decodes the timestamp prefix of `id` back into the millisecond it encodes.
    ///
    /// Written from the format rather than from the encoder, so that a change to either side
    /// is caught by the other: the prefix is ten five-bit groups, most significant first.
    fn decode_prefix(id: &str) -> u64 {
        id.chars().take(10).fold(0u64, |value, character| {
            let digit = CROCKFORD
                .iter()
                .position(|byte| *byte as char == character)
                .expect("the character is in the alphabet") as u64;
            (value << 5) | digit
        })
    }

    #[test]
    fn test_ulid_is_twenty_six_crockford_characters() {
        let id = ulid(1_790_899_200_000, ENTROPY);
        assert_eq!(id.len(), LENGTH);
        assert!(
            id.chars()
                .all(|character| CROCKFORD.contains(&(character as u8))),
            "every character is in the alphabet: {id}"
        );
        // The alphabet excludes the four letters a human reads as digits.
        for excluded in ['I', 'L', 'O', 'U'] {
            assert!(!id.contains(excluded), "{id}");
        }
    }

    #[test]
    fn test_ulid_encodes_the_millisecond_it_was_given() {
        // The prefix is the timestamp, and it has to decode back to the same number: the
        // ordering guarantee below is worth nothing if the field is not the clock.
        for millis in [0, 1, 1_790_899_200_000, 281_474_976_710_655] {
            let id = ulid(millis, ENTROPY);
            assert_eq!(decode_prefix(&id), millis, "{id}");
        }
    }

    #[test]
    fn test_ulid_orders_by_the_millisecond_it_was_made_in() {
        // The property the format exists for: two identifiers minted a millisecond apart sort
        // in the order they were made, which is what makes a list of them a timeline.
        let earlier = ulid(1_790_899_200_000, ENTROPY);
        let later = ulid(1_790_899_200_001, ENTROPY);
        assert!(earlier < later, "{earlier} < {later}");
        // And the same millisecond with different randomness does not collide.
        let other = ulid(1_790_899_200_000, [0xFF; ENTROPY_BYTES]);
        assert_ne!(earlier, other);
        assert_eq!(
            earlier.get(..10),
            other.get(..10),
            "the timestamp prefix is the same millisecond"
        );
    }

    #[test]
    fn test_ulid_encodes_the_zero_epoch_as_zeroes() {
        assert_eq!(ulid(0, [0; ENTROPY_BYTES]), "00000000000000000000000000");
    }

    #[test]
    fn test_request_id_draws_from_the_entropy_source() {
        // Two identifiers in a row must differ: the whole point of drawing from the system
        // entropy source rather than from the clock is that a repeated call is still unique.
        let first = request_id().expect("the entropy source is readable");
        let second = request_id().expect("the entropy source is readable");
        assert_eq!(first.len(), LENGTH);
        assert_ne!(first, second);
    }
}
