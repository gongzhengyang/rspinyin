//! The tail AUX column: the per-word refinement codes of the container.
//!
//! Responsibility: define the byte layout of the AUX block, encode it for the
//! writer, validate and read it back for the reader, and expose the
//! [`AuxSource`] view of it.
//!
//! The column is not a section-table section: the table's six slots are fixed
//! by the version-1/2 layout, and growing it would make every container an
//! older reader can open into an unreadable one. The block instead lives at the
//! aligned tail of the file -- past every table section -- and is announced by
//! [`FLAG_AUX_PRESENT`] in the header. A container without the flag is exactly
//! the pre-AUX layout, and reads identically; a container with the flag has its
//! block validated on open, so the accessors below never index anything the
//! parse step has not checked.
//!
//! # Block layout
//!
//! ```text
//! offset  size  field
//!      0     4  magic      b"AUX1"
//!      4     4  crc32      CRC32 of the payload rows
//!      8     4  row_count  must equal the header's entry_count
//!     12     4  reserved   all zero
//!     16     4  row 0      one row per word record, ENTRIES ordering
//!     ...
//! ```
//!
//! One row is [`AUX_ROW_SIZE`] bytes: up to [`AUX_MAX_CODE_LEN`] lowercase
//! ASCII letters followed by zero fill. An all-zero row (an empty code) means
//! the word's first character carries no decomposition; it is data, not a
//! fault, because the decomposition table is a subset by design.

use ime_core::aux::AuxSource;
use ime_types::DictError;

use crate::format::reader::Verify;
use crate::format::{self, Header, crc32};

/// The AUX block magic.
pub const AUX_MAGIC: [u8; 4] = *b"AUX1";

/// Size of the self-describing block header in bytes.
pub const AUX_HEADER_SIZE: usize = 16;

/// Width of one aux-code row: up to four letters, zero-filled.
pub const AUX_ROW_SIZE: usize = 4;

/// Longest aux code a row can hold, in letters.
pub const AUX_MAX_CODE_LEN: usize = 4;

/// Encodes the block the writer places at the aligned tail of the file.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] when `rows` is not a whole number
/// of [`AUX_ROW_SIZE`]-byte rows or the row count does not fit `u32`; both are
/// writer-internal invariants the column API already checks.
pub(crate) fn encode_block(row_count: u32, rows: &[u8]) -> Result<Vec<u8>, DictError> {
    if rows.len() % AUX_ROW_SIZE != 0 {
        return Err(DictError::LengthOutOfRange {
            field: "aux_len",
            value: rows.len() as u64,
        });
    }
    let count = rows.len() / AUX_ROW_SIZE;
    if count != row_count as usize {
        return Err(DictError::LengthOutOfRange {
            field: "aux_count",
            value: count as u64,
        });
    }
    let mut block = Vec::with_capacity(AUX_HEADER_SIZE + rows.len());
    block.extend_from_slice(&AUX_MAGIC);
    block.extend_from_slice(&crc32(rows).to_le_bytes());
    block.extend_from_slice(&row_count.to_le_bytes());
    block.extend_from_slice(&0u32.to_le_bytes());
    block.extend_from_slice(rows);
    Ok(block)
}

/// Validates the AUX block at `start` of an untrusted image.
///
/// The checks are the trust model's, in order: the block fits in the file, the
/// magic and the reserved field are intact, the row count equals the header's
/// entry count, the payload is a whole number of rows, every row is lowercase
/// letters followed by zero fill, and -- under [`Verify::Full`] -- the payload
/// checksum matches.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] naming the failing field, or
/// [`DictError::Crc`] when the payload checksum disagrees.
pub(crate) fn validate_block(
    data: &[u8],
    start: u64,
    header: &Header,
    verify: Verify,
) -> Result<(), DictError> {
    let start = usize::try_from(start).map_err(|_| DictError::LengthOutOfRange {
        field: "aux_offset",
        value: start,
    })?;
    let block = data.get(start..).ok_or(DictError::LengthOutOfRange {
        field: "aux_offset",
        value: start as u64,
    })?;
    if block.len() < AUX_HEADER_SIZE {
        return Err(DictError::LengthOutOfRange {
            field: "aux_len",
            value: block.len() as u64,
        });
    }
    if block.get(0..4) != Some(&AUX_MAGIC[..]) {
        return Err(DictError::LengthOutOfRange {
            field: "aux_magic",
            value: 1,
        });
    }
    let stored_crc = format::read_u32(block, 4, "aux_crc")?;
    let row_count = format::read_u32(block, 8, "aux_count")?;
    let reserved = format::read_u32(block, 12, "aux_reserved")?;
    if reserved != 0 {
        return Err(DictError::LengthOutOfRange {
            field: "aux_reserved",
            value: u64::from(reserved),
        });
    }
    if row_count != header.entry_count {
        return Err(DictError::LengthOutOfRange {
            field: "aux_count",
            value: u64::from(row_count),
        });
    }
    // The block's own extent ends after `row_count` rows: bytes beyond it are
    // the next tail block (the REVERSE index stacks directly after AUX), not
    // part of this one, so neither the row scan nor the CRC may read them.
    let rows_len = usize::try_from(row_count)
        .ok()
        .and_then(|count| count.checked_mul(AUX_ROW_SIZE))
        .ok_or(DictError::LengthOutOfRange {
            field: "aux_len",
            value: u64::from(row_count),
        })?;
    let rows = block
        .get(AUX_HEADER_SIZE..AUX_HEADER_SIZE + rows_len)
        .ok_or(DictError::LengthOutOfRange {
            field: "aux_len",
            value: block.len().saturating_sub(AUX_HEADER_SIZE) as u64,
        })?;
    validate_rows(rows)?;
    if verify == Verify::Full {
        let actual = crc32(rows);
        if actual != stored_crc {
            return Err(DictError::Crc {
                expected: stored_crc,
                actual,
            });
        }
    }
    Ok(())
}

/// Checks every row's charset: lowercase letters, zero-filled, no gaps.
///
/// # Errors
/// Returns [`DictError::LengthOutOfRange`] naming `aux_row` for the first row
/// that breaks the shape.
fn validate_rows(rows: &[u8]) -> Result<(), DictError> {
    for row in rows.chunks_exact(AUX_ROW_SIZE) {
        let mut length = AUX_MAX_CODE_LEN;
        for (index, byte) in row.iter().enumerate() {
            match byte {
                0 => {
                    length = index;
                    break;
                }
                b'a'..=b'z' => {}
                _ => {
                    return Err(DictError::LengthOutOfRange {
                        field: "aux_row",
                        value: u64::from(*byte),
                    });
                }
            }
        }
        if row[length..].iter().any(|byte| *byte != 0) {
            return Err(DictError::LengthOutOfRange {
                field: "aux_row",
                value: 1,
            });
        }
    }
    Ok(())
}

impl<D: AsRef<[u8]>> super::reader::Reader<D> {
    /// Returns `true` when the container carries the tail AUX column.
    ///
    /// A v1 or pre-AUX container reads as `false`; that is the state the
    /// refinement session degrades from (`aux/absent`), not an error.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub fn aux_column_present(&self) -> bool {
        self.aux_start.is_some()
    }

    /// Returns the aux code of the word record at `word_id`, or `None` when
    /// the word's first character carries no decomposition.
    ///
    /// # Errors
    /// Returns [`DictError::LengthOutOfRange`] when the column is absent or
    /// `word_id` is past it.
    pub fn aux_row(&self, word_id: u32) -> Result<Option<&str>, DictError> {
        let start = self.aux_start.ok_or(DictError::LengthOutOfRange {
            field: "aux_absent",
            value: u64::from(word_id),
        })?;
        if word_id >= self.header.entry_count {
            return Err(DictError::LengthOutOfRange {
                field: "aux_index",
                value: u64::from(word_id),
            });
        }
        let at = start as usize + AUX_HEADER_SIZE + word_id as usize * AUX_ROW_SIZE;
        let row =
            self.data
                .as_ref()
                .get(at..at + AUX_ROW_SIZE)
                .ok_or(DictError::LengthOutOfRange {
                    field: "aux_index",
                    value: u64::from(word_id),
                })?;
        let end = row
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(AUX_ROW_SIZE);
        if end == 0 {
            return Ok(None);
        }
        // The parse step validated the charset, so this cannot fail; an error
        // is reported rather than guessed at.
        std::str::from_utf8(&row[..end])
            .map(Some)
            .map_err(|_| DictError::LengthOutOfRange {
                field: "aux_row",
                value: u64::from(word_id),
            })
    }

    /// Looks a word's aux code up by text.
    ///
    /// The word resolves to its record through the same hash-ordered lookup
    /// the unigram table uses, so the read is a binary search plus one row
    /// slice, with no allocation. A word outside the container or without a
    /// recorded code reads as `None`.
    ///
    /// # Errors
    /// Returns the errors of [`super::reader::Reader::word_id`] and
    /// [`Reader::aux_row`].
    pub fn aux_lookup(&self, word: &str) -> Result<Option<&str>, DictError> {
        let Some(word_id) = self.word_id(word)? else {
            return Ok(None);
        };
        self.aux_row(word_id)
    }
}

// `D` is a byte image the caller keeps mapped or owned; every real image kind
// is `Sync`, and the trait requires it for the router's static session table.
impl<D: AsRef<[u8]> + Sync> AuxSource for super::reader::Reader<D> {
    fn has_aux_column(&self) -> bool {
        self.aux_column_present()
    }

    fn aux_code(&self, word: &str) -> Result<Option<&str>, DictError> {
        self.aux_lookup(word)
    }
}

#[cfg(test)]
mod tests;
