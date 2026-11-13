//! The reverse-lookup index: the decomposition table compiled into a
//! "code fragment -> characters" block for the `u`-led reverse domain.
//!
//! Responsibility: define the byte layout of the REVERSE tail block, build the
//! index from decomposition entries at compile time, validate and read it back
//! for the reader, and expose the [`ReverseSource`] view of it.
//!
//! Like the AUX column, the block is not a section-table section: the table's
//! six slots are fixed by the version-1/2 layout. The block lives at the
//! aligned tail of the file — after the AUX block when that one is present —
//! and is announced by [`crate::format::FLAG_REVERSE_PRESENT`] in the header.
//! A container without the flag is exactly the pre-reverse layout and reads
//! identically; a container with the flag has its block validated on open, so
//! the accessors below never index anything the parse step has not checked.
//!
//! # Block layout
//!
//! ```text
//! offset  size  field
//!      0     4  magic          b"RVX1"
//!      4     4  crc32          CRC32 of the payload
//!      8     4  char_count     rows in the characters table
//!     12     4  key_count      rows in the fragment index
//!     16     4  posting_count  entries in the postings array
//!     20  8*cc  chars          character rows, ascending codepoint
//!         12*kc keys           fragment rows, ascending key bytes
//!          4*pc postings      u32 character indices, grouped by key
//! ```
//!
//! One characters row is [`REVERSE_CHAR_ROW_SIZE`] bytes: the codepoint as a
//! little-endian `u32` followed by the character's full decomposition code,
//! up to [`REVERSE_MAX_CODE_LEN`] lowercase letters, zero-filled. One key row
//! is [`REVERSE_KEY_ROW_SIZE`] bytes: the fragment (a subsequence of some
//! code, so at most [`REVERSE_MAX_CODE_LEN`] letters, zero-filled) followed by
//! the `start` and `count` of its postings run. The runs are contiguous, so
//! the keys tile the postings array exactly.
//!
//! # Why every subsequence is a key
//!
//! A reverse query must match a code by subsequence — the user spells the
//! components they are sure of, in order, and may skip ones they cannot name —
//! which a prefix index cannot answer directly. Because a code is at most four
//! letters, a character has at most fifteen non-empty subsequences, so the
//! compiler enumerates all of them and materializes each as a key. The runtime
//! query stays one binary search and one posting slice, the same shape the
//! unigram and bigram tables offer, and the match semantics stay exactly
//! "payload is a subsequence of the code, order preserved". A key only exists
//! when some character carries it, and the postings of a key are the matching
//! characters in ascending index order, so a query's candidate order is
//! deterministic in the container alone.

use ime_core::reverse::{ReverseHit, ReverseSource};
use ime_types::DictError;

use crate::format::reader::Verify;
use crate::format::{crc32, read_u32};

/// The REVERSE block magic.
pub const REVERSE_MAGIC: [u8; 4] = *b"RVX1";

/// Size of the self-describing block header in bytes.
pub const REVERSE_HEADER_SIZE: usize = 20;

/// Width of one characters row: a `u32` codepoint plus a zero-filled code.
pub const REVERSE_CHAR_ROW_SIZE: usize = 8;

/// Width of one key row: a zero-filled fragment plus its postings range.
pub const REVERSE_KEY_ROW_SIZE: usize = 12;

/// Longest decomposition code the block can hold, in letters.
///
/// The decomposition table's own rule caps a code at four letters; the block
/// inherits that cap, which also bounds a fragment key and keeps the
/// subsequence enumeration finite.
pub const REVERSE_MAX_CODE_LEN: usize = 4;

/// One compile-time entry of the index under construction.
///
/// A pair of the character and its full decomposition code. A character
/// repeated with a second code keeps the first code in the characters table
/// (the annotation a candidate shows) while both codes contribute fragment
/// keys, which is the polyphonic-expansion path the table may grow into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexEntry<'a> {
    /// The character the entry commits.
    pub character: char,
    /// The character's full decomposition code, one to four lowercase letters.
    pub code: &'a str,
}

/// The compiled index block, with the counts the build report prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReverseIndex {
    /// The encoded block, header included.
    pub bytes: Vec<u8>,
    /// Distinct characters the index carries.
    pub char_count: usize,
    /// Distinct fragment keys the subsequence enumeration produced.
    pub key_count: usize,
    /// Total postings across all keys.
    pub posting_count: usize,
}

/// Builds the reverse-index block from decomposition entries.
///
/// The characters are deduplicated and laid out in ascending codepoint order;
/// every non-empty subsequence of every entry's code becomes a key whose
/// postings hold the matching characters in ascending index order. The result
/// is a pure function of `entries`: the same input encodes to the same bytes,
/// which is what makes a compiled container reproducible.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] when `entries` is empty, a code is
/// empty or longer than [`REVERSE_MAX_CODE_LEN`] letters or holds anything but
/// lowercase letters, or a count would not fit the block's `u32` fields.
///
/// # Examples
///
/// ```
/// use ime_dict::reverse::{IndexEntry, build_index};
///
/// let index = build_index(&[IndexEntry { character: '房', code: "hf" }])
///     .expect("a valid entry");
/// assert_eq!(&index.bytes[0..4], b"RVX1");
/// // "hf" and both of its single-letter subsequences became keys.
/// assert_eq!(index.key_count, 3);
/// ```
pub fn build_index(entries: &[IndexEntry<'_>]) -> Result<ReverseIndex, DictError> {
    if entries.is_empty() {
        return Err(DictError::LengthOutOfRange {
            field: "reverse_empty",
            value: 0,
        });
    }
    // Distinct characters in codepoint order, each with the first code any
    // entry recorded for it; the per-entry index list pairs every entry with
    // its character's row so the fragment pass needs no second lookup.
    let mut seen: Vec<char> = Vec::with_capacity(entries.len());
    let mut primary: Vec<&str> = Vec::with_capacity(entries.len());
    let mut indexed: Vec<(u32, &str)> = Vec::with_capacity(entries.len());
    for entry in entries {
        validate_code(entry.code)?;
        let index = match seen.binary_search(&entry.character) {
            Ok(at) => at,
            Err(at) => {
                seen.insert(at, entry.character);
                primary.insert(at, entry.code);
                at
            }
        };
        let index = u32::try_from(index).map_err(|_| DictError::LengthOutOfRange {
            field: "reverse_char_count",
            value: seen.len() as u64,
        })?;
        indexed.push((index, entry.code));
    }
    // Fragment keys, ascending byte order because BTreeMap iterates sorted;
    // each key accumulates the character indices whose code contains it.
    let mut keys: std::collections::BTreeMap<[u8; REVERSE_MAX_CODE_LEN], Vec<u32>> =
        std::collections::BTreeMap::new();
    for (index, code) in &indexed {
        let code = code.as_bytes();
        for mask in 1..(1u16 << code.len()) {
            let mut fragment = [0u8; REVERSE_MAX_CODE_LEN];
            let mut written = 0usize;
            for (bit, byte) in code.iter().enumerate() {
                if mask & (1 << bit) != 0 {
                    fragment[written] = *byte;
                    written += 1;
                }
            }
            let postings = keys.entry(fragment).or_default();
            if !postings.contains(index) {
                postings.push(*index);
            }
        }
    }
    let posting_count: usize = keys.values().map(Vec::len).sum();
    let char_count = seen.len();
    // Every count lands in a `u32` block field, so the overflow of one is the
    // layout's error, not an arithmetic accident.
    let char_field = count_field(char_count, "reverse_char_count")?;
    let key_field = count_field(keys.len(), "reverse_key_count")?;
    let posting_field = count_field(posting_count, "reverse_posting_count")?;
    // The payload: characters, then keys with their ranges, then postings.
    let payload_cap =
        char_count * REVERSE_CHAR_ROW_SIZE + keys.len() * REVERSE_KEY_ROW_SIZE + posting_count * 4;
    let mut payload = Vec::with_capacity(payload_cap);
    for (at, character) in seen.iter().enumerate() {
        payload.extend_from_slice(&(*character as u32).to_le_bytes());
        let mut row = [0u8; REVERSE_MAX_CODE_LEN];
        let code = primary[at].as_bytes();
        row[..code.len()].copy_from_slice(code);
        payload.extend_from_slice(&row);
    }
    let mut postings_out = Vec::with_capacity(posting_count * 4);
    let mut running = 0u32;
    for (fragment, indices) in &keys {
        payload.extend_from_slice(fragment);
        payload.extend_from_slice(&running.to_le_bytes());
        payload
            .extend_from_slice(&count_field(indices.len(), "reverse_posting_count")?.to_le_bytes());
        for index in indices {
            postings_out.extend_from_slice(&index.to_le_bytes());
        }
        running += indices.len() as u32;
    }
    payload.extend_from_slice(&postings_out);
    // The header: magic, payload checksum, the three counts.
    let mut block = Vec::with_capacity(REVERSE_HEADER_SIZE + payload.len());
    block.extend_from_slice(&REVERSE_MAGIC);
    block.extend_from_slice(&crc32(&payload).to_le_bytes());
    block.extend_from_slice(&char_field.to_le_bytes());
    block.extend_from_slice(&key_field.to_le_bytes());
    block.extend_from_slice(&posting_field.to_le_bytes());
    block.extend_from_slice(&payload);
    Ok(ReverseIndex {
        bytes: block,
        char_count,
        key_count: keys.len(),
        posting_count,
    })
}

/// Converts a count into the `u32` a block field carries.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] naming `field` when the count does
/// not fit.
fn count_field(count: usize, field: &'static str) -> Result<u32, DictError> {
    u32::try_from(count).map_err(|_| DictError::LengthOutOfRange {
        field,
        value: count as u64,
    })
}

/// Checks a decomposition code's shape: one to four lowercase letters.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] naming `reverse_code` when the
/// shape is broken.
fn validate_code(code: &str) -> Result<(), DictError> {
    if code.is_empty()
        || code.len() > REVERSE_MAX_CODE_LEN
        || !code.bytes().all(|byte| byte.is_ascii_lowercase())
    {
        return Err(DictError::LengthOutOfRange {
            field: "reverse_code",
            value: code.len() as u64,
        });
    }
    Ok(())
}

/// Validates the REVERSE block at `start` of an untrusted image.
///
/// The checks are the trust model's, in order: the block fits in the file, the
/// magic is intact, the three counts describe a payload that exactly fills the
/// file tail, every character row names a scalar and a well-shaped code, every
/// key row is a well-shaped fragment, the keys ascend, the postings runs are
/// contiguous and every posting names a character row — and, under
/// [`Verify::Full`], the payload checksum matches.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] naming the failing field, or
/// [`DictError::Crc`] when the payload checksum disagrees.
pub(crate) fn validate_block(data: &[u8], start: u64, verify: Verify) -> Result<(), DictError> {
    let start = usize::try_from(start).map_err(|_| DictError::LengthOutOfRange {
        field: "reverse_offset",
        value: start,
    })?;
    let block = data.get(start..).ok_or(DictError::LengthOutOfRange {
        field: "reverse_offset",
        value: start as u64,
    })?;
    if block.len() < REVERSE_HEADER_SIZE {
        return Err(DictError::LengthOutOfRange {
            field: "reverse_len",
            value: block.len() as u64,
        });
    }
    if block.get(0..4) != Some(&REVERSE_MAGIC[..]) {
        return Err(DictError::LengthOutOfRange {
            field: "reverse_magic",
            value: 1,
        });
    }
    let stored_crc = read_u32(block, 4, "reverse_crc")?;
    let char_count = read_u32(block, 8, "reverse_char_count")? as usize;
    let key_count = read_u32(block, 12, "reverse_key_count")? as usize;
    let posting_count = read_u32(block, 16, "reverse_posting_count")? as usize;
    let payload_len = char_count
        .checked_mul(REVERSE_CHAR_ROW_SIZE)
        .and_then(|len| len.checked_add(key_count.checked_mul(REVERSE_KEY_ROW_SIZE)?))
        .and_then(|len| len.checked_add(posting_count.checked_mul(4)?))
        .ok_or(DictError::LengthOutOfRange {
            field: "reverse_len",
            value: block.len() as u64,
        })?;
    let payload = block
        .get(REVERSE_HEADER_SIZE..)
        .filter(|payload| payload.len() == payload_len);
    let Some(payload) = payload else {
        return Err(DictError::LengthOutOfRange {
            field: "reverse_len",
            value: block.len() as u64,
        });
    };
    let chars_len = char_count * REVERSE_CHAR_ROW_SIZE;
    let keys_len = key_count * REVERSE_KEY_ROW_SIZE;
    validate_chars(&payload[..chars_len])?;
    validate_keys(&payload[chars_len..][..keys_len], posting_count)?;
    validate_postings(&payload[chars_len + keys_len..], char_count)?;
    if verify == Verify::Full {
        let actual = crc32(payload);
        if actual != stored_crc {
            return Err(DictError::Crc {
                expected: stored_crc,
                actual,
            });
        }
    }
    Ok(())
}

/// Checks every characters row: a valid scalar followed by a zero-filled code.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] naming `reverse_char` for the first
/// row that breaks the shape.
fn validate_chars(rows: &[u8]) -> Result<(), DictError> {
    for row in rows.chunks_exact(REVERSE_CHAR_ROW_SIZE) {
        let codepoint = read_u32(row, 0, "reverse_char")?;
        if char::from_u32(codepoint).is_none() {
            return Err(DictError::LengthOutOfRange {
                field: "reverse_char",
                value: u64::from(codepoint),
            });
        }
        validate_code_row(&row[4..])?;
    }
    Ok(())
}

/// Checks one zero-filled code row: lowercase letters, then nothing.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] naming `reverse_code` when the
/// shape is broken.
fn validate_code_row(row: &[u8]) -> Result<(), DictError> {
    let end = code_row_end(row)?;
    if row[end..].iter().any(|byte| *byte != 0) {
        return Err(DictError::LengthOutOfRange {
            field: "reverse_code",
            value: 1,
        });
    }
    Ok(())
}

/// Finds where the letters of a zero-filled code row end.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] naming `field` when the row holds
/// anything but lowercase letters up to its first zero byte.
fn code_row_end(row: &[u8]) -> Result<usize, DictError> {
    let mut end = row.len();
    for (index, byte) in row.iter().enumerate() {
        match byte {
            0 => {
                end = index;
                break;
            }
            b'a'..=b'z' => {}
            _ => {
                return Err(DictError::LengthOutOfRange {
                    field: "reverse_code",
                    value: u64::from(*byte),
                });
            }
        }
    }
    Ok(end)
}

/// Checks every key row and its postings range.
///
/// A key is a one-to-four-letter fragment followed by zero fill; the keys
/// ascend so a query can binary-search; each range is contiguous with the
/// previous one, so the runs tile the postings array exactly and the last one
/// ends at `posting_count`.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] naming `reverse_key` for a broken
/// fragment, `reverse_order` for a descending key, or `reverse_range` for a
/// broken or non-contiguous postings range.
fn validate_keys(rows: &[u8], posting_count: usize) -> Result<(), DictError> {
    let mut running = 0usize;
    let mut previous: Option<[u8; REVERSE_MAX_CODE_LEN]> = None;
    for row in rows.chunks_exact(REVERSE_KEY_ROW_SIZE) {
        let fragment: [u8; REVERSE_MAX_CODE_LEN] =
            row[..REVERSE_MAX_CODE_LEN]
                .try_into()
                .map_err(|_| DictError::LengthOutOfRange {
                    field: "reverse_key",
                    value: 1,
                })?;
        validate_fragment(&fragment)?;
        if previous.is_some_and(|previous| fragment <= previous) {
            return Err(DictError::LengthOutOfRange {
                field: "reverse_order",
                value: u64::from(fragment[0]),
            });
        }
        previous = Some(fragment);
        let start = read_u32(row, 4, "reverse_range")? as usize;
        let count = read_u32(row, 8, "reverse_range")? as usize;
        if count == 0 || start != running {
            return Err(DictError::LengthOutOfRange {
                field: "reverse_range",
                value: start as u64,
            });
        }
        running = running
            .checked_add(count)
            .ok_or(DictError::LengthOutOfRange {
                field: "reverse_range",
                value: posting_count as u64,
            })?;
    }
    if running != posting_count {
        return Err(DictError::LengthOutOfRange {
            field: "reverse_range",
            value: running as u64,
        });
    }
    Ok(())
}

/// Checks a fragment's shape: one to four lowercase letters, zero-filled.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] naming `reverse_key` when the
/// shape is broken.
fn validate_fragment(fragment: &[u8; REVERSE_MAX_CODE_LEN]) -> Result<(), DictError> {
    let end = code_row_end(fragment)?;
    if end == 0 {
        return Err(DictError::LengthOutOfRange {
            field: "reverse_key",
            value: 0,
        });
    }
    if fragment[end..].iter().any(|byte| *byte != 0) {
        return Err(DictError::LengthOutOfRange {
            field: "reverse_key",
            value: 1,
        });
    }
    Ok(())
}

/// Checks every posting: an index inside the characters table.
///
/// The postings slice is exactly `4 * posting_count` bytes when the payload
/// length check above passed, so a partial trailing row cannot occur; the loop
/// re-checks it anyway rather than trusting the arithmetic.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] naming `reverse_posting` for a
/// truncated array or the first posting that names a row past `char_count`.
fn validate_postings(postings: &[u8], char_count: usize) -> Result<(), DictError> {
    if postings.len() % 4 != 0 {
        return Err(DictError::LengthOutOfRange {
            field: "reverse_posting",
            value: postings.len() as u64,
        });
    }
    for posting in postings.chunks_exact(4) {
        let index = read_u32(posting, 0, "reverse_posting")? as usize;
        if index >= char_count {
            return Err(DictError::LengthOutOfRange {
                field: "reverse_posting",
                value: index as u64,
            });
        }
    }
    Ok(())
}

impl<D: AsRef<[u8]>> crate::format::reader::Reader<D> {
    /// Returns `true` when the container carries the tail reverse index.
    ///
    /// A v1 or pre-reverse container reads as `false`; that is the state the
    /// reverse domain degrades from (`reverse/absent`), not an error.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn reverse_index_present(&self) -> bool {
        self.reverse_start.is_some()
    }

    /// Returns the characters whose decomposition code contains `payload` as a
    /// subsequence, in ascending codepoint order, each with its full code.
    ///
    /// A payload the reverse domain would not recognize — empty, longer than
    /// [`REVERSE_MAX_CODE_LEN`] letters, or holding anything but lowercase
    /// letters — names no key and reads as an empty list, not an error.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] when the index is absent or the
    /// image is inconsistent with the layout the parse step verified.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn reverse_lookup(&self, payload: &str) -> Result<Vec<ReverseHit<'_>>, DictError> {
        let Some(start) = self.reverse_start else {
            return Err(DictError::LengthOutOfRange {
                field: "reverse_absent",
                value: payload.len() as u64,
            });
        };
        if payload.is_empty()
            || payload.len() > REVERSE_MAX_CODE_LEN
            || !payload.bytes().all(|byte| byte.is_ascii_lowercase())
        {
            return Ok(Vec::new());
        }
        let data = self.data.as_ref();
        let block_at = start as usize;
        let char_count = read_u32(data, block_at + 8, "reverse_char_count")? as usize;
        let key_count = read_u32(data, block_at + 12, "reverse_key_count")? as usize;
        let posting_count = read_u32(data, block_at + 16, "reverse_posting_count")? as usize;
        // The payload offsets the parse step validated, re-derived here so the
        // accessor trusts only arithmetic it performed itself.
        let payload_at = block_at + REVERSE_HEADER_SIZE;
        let keys_at = payload_at + char_count * REVERSE_CHAR_ROW_SIZE;
        let postings_at = keys_at + key_count * REVERSE_KEY_ROW_SIZE;
        let mut fragment = [0u8; REVERSE_MAX_CODE_LEN];
        fragment[..payload.len()].copy_from_slice(payload.as_bytes());
        let Some(row_at) = self.find_key_row(keys_at, key_count, &fragment)? else {
            return Ok(Vec::new());
        };
        let range_start = read_u32(data, row_at + 4, "reverse_range")? as usize;
        let range_count = read_u32(data, row_at + 8, "reverse_range")? as usize;
        if range_start
            .checked_add(range_count)
            .is_none_or(|end| end > posting_count)
        {
            return Err(DictError::LengthOutOfRange {
                field: "reverse_range",
                value: range_start as u64,
            });
        }
        let mut hits = Vec::with_capacity(range_count);
        for offset in 0..range_count {
            let posting_at = postings_at + (range_start + offset) * 4;
            let index = read_u32(data, posting_at, "reverse_posting")? as usize;
            if index >= char_count {
                return Err(DictError::LengthOutOfRange {
                    field: "reverse_posting",
                    value: index as u64,
                });
            }
            let char_at = payload_at + index * REVERSE_CHAR_ROW_SIZE;
            let codepoint = read_u32(data, char_at, "reverse_char")?;
            let character = char::from_u32(codepoint).ok_or(DictError::LengthOutOfRange {
                field: "reverse_char",
                value: u64::from(codepoint),
            })?;
            let code = self.code_of(char_at, index)?;
            hits.push(ReverseHit { character, code });
        }
        Ok(hits)
    }

    /// Reads the zero-filled code of the characters row at `char_at`.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] when the row is truncated or
    /// its bytes are not the lowercase letters the parse step validated.
    fn code_of(&self, char_at: usize, index: usize) -> Result<&str, DictError> {
        let row = self
            .data
            .as_ref()
            .get(char_at + 4..char_at + REVERSE_CHAR_ROW_SIZE)
            .ok_or(DictError::LengthOutOfRange {
                field: "reverse_char",
                value: index as u64,
            })?;
        let end = row
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(REVERSE_MAX_CODE_LEN);
        if end == 0 {
            return Err(DictError::LengthOutOfRange {
                field: "reverse_code",
                value: index as u64,
            });
        }
        // The parse step validated the charset, so this cannot fail; an error
        // is reported rather than guessed at.
        std::str::from_utf8(&row[..end]).map_err(|_| DictError::LengthOutOfRange {
            field: "reverse_code",
            value: index as u64,
        })
    }

    /// Binary-searches the key rows for `fragment`, returning the file offset
    /// of the matching row.
    ///
    /// The keys ascend by their zero-filled bytes, which orders fragments
    /// first by their letters and then by length — the same total order the
    /// padded query bytes sort under, so the comparison is a plain byte-wise
    /// one.
    fn find_key_row(
        &self,
        keys_at: usize,
        key_count: usize,
        fragment: &[u8; REVERSE_MAX_CODE_LEN],
    ) -> Result<Option<usize>, DictError> {
        let data = self.data.as_ref();
        let mut low = 0usize;
        let mut high = key_count;
        while low < high {
            let middle = low + (high - low) / 2;
            let row_at = keys_at + middle * REVERSE_KEY_ROW_SIZE;
            let row = data.get(row_at..row_at + REVERSE_KEY_ROW_SIZE).ok_or(
                DictError::LengthOutOfRange {
                    field: "reverse_key",
                    value: middle as u64,
                },
            )?;
            match row[..REVERSE_MAX_CODE_LEN].cmp(fragment) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return Ok(Some(row_at)),
            }
        }
        Ok(None)
    }
}

// `D` is a byte image the caller keeps mapped or owned; every real image kind
// is `Sync`, and the trait requires it for the router's static session table.
impl<D: AsRef<[u8]> + Sync> ReverseSource for crate::format::reader::Reader<D> {
    fn has_reverse_index(&self) -> bool {
        self.reverse_index_present()
    }

    fn reverse_lookup(&self, payload: &str) -> Result<Vec<ReverseHit<'_>>, DictError> {
        // The inherent method wins method resolution anyway; the qualified call
        // keeps the delegation explicit against the day a second `reverse_lookup`
        // trait lands in scope.
        crate::format::reader::Reader::reverse_lookup(self, payload)
    }
}

#[cfg(test)]
mod tests;
