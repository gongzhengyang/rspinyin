//! The reverse-index build pass: the decomposition table compiled into the
//! "code fragment -> characters" tail block.
//!
//! Responsibility: read the same self-built decomposition table the aux pass reads
//! (`data/raw/chaizi.tsv`, through the aux pass's own reader), derive one
//! [`IndexEntry`] per character, and encode the block `ime_dict::reverse` lays out.
//! The block is the offline half of the reverse domain: the runtime answers a
//! `u`-payload with one binary search and one posting slice, never a table scan.
//!
//! Boundaries: this module owns the entry derivation and the pass orchestration,
//! and nothing else. It does not parse the table (that is `aux::load_table`'s, so
//! both passes judge a row the same way), does not assemble the bytes (that is
//! `ime_dict::reverse::build_index`), and does not place the block in the file
//! (that is the writer's, which stacks it after the AUX block when that one is
//! present). The entries are a pure function of the table, so recompiling the same
//! sources reproduces the same bytes.
//!
//! # Why there is no word-list alignment to check
//!
//! The aux column addresses its rows by word id, so a pass that ran against a
//! different word list than the compiler saw misaligns every row silently -- the
//! failure `aux::check_alignment` exists to catch. The reverse block references no
//! word: its characters are keyed by codepoint and its counts live in its own
//! header, so there is no second list it can drift from. The sibling check this
//! pass owes is therefore the one the container itself performs on open -- the
//! header describing a payload that exactly fills the block -- run here, at build
//! time, so a build that produced a malformed block fails before it ships rather
//! than at the reader.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, anyhow, ensure};
use ime_dict::reverse::{
    IndexEntry, REVERSE_CHAR_ROW_SIZE, REVERSE_HEADER_SIZE, REVERSE_KEY_ROW_SIZE, REVERSE_MAGIC,
    ReverseIndex, build_index,
};

use crate::dictc::aux;
use crate::dictc::budget::SegmentBudgets;

/// The reverse block's per-mille share of the container ceiling.
///
/// Like the aux column, the block has no section-table slot, so it is accounted
/// separately from the payload sections, by the same per-mille discipline the size
/// ledger applies to the six payload sections. At the planned first-phase scale --
/// the common characters covered, one code of at most four letters each -- the
/// block is a few hundred thousand postings and the fragment keys that tile them,
/// of order one mebibyte; 70 per-mille of the 20 MiB ceiling is about 1.4 MiB, so
/// the share holds that scale with headroom without inviting unbounded growth.
const REVERSE_SHARE_PER_MILLE: u32 = 70;

/// Denominator of the per-mille share.
const PER_MILLE: u128 = 1000;

/// The compiled block, with the counts the build log prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Block {
    /// The encoded block, header included.
    pub(crate) bytes: Vec<u8>,
    /// Distinct characters the block carries.
    pub(crate) char_count: usize,
    /// Distinct fragment keys the subsequence enumeration produced.
    pub(crate) key_count: usize,
    /// Total postings across all keys.
    pub(crate) posting_count: usize,
}

/// Encodes the decomposition table into the block `ime_dict` lays out.
///
/// One entry per table row, character with its code; the encoder deduplicates
/// characters, enumerates every non-empty subsequence of every code into fragment
/// keys, and answers the counts the build log prints.
///
/// # Errors
/// Returns the encoder's own errors: an empty table, or a code that is empty,
/// longer than four letters, or holds anything but lowercase letters. The table
/// reader refuses every one of those shapes already, so an error here names a
/// reader the table slipped past.
pub(crate) fn build_block(table: &BTreeMap<char, String>) -> Result<Block> {
    let entries: Vec<IndexEntry<'_>> = table
        .iter()
        .map(|(character, code)| IndexEntry {
            character: *character,
            code: code.as_str(),
        })
        .collect();
    let ReverseIndex {
        bytes,
        char_count,
        key_count,
        posting_count,
    } = build_index(&entries)?;
    Ok(Block {
        bytes,
        char_count,
        key_count,
        posting_count,
    })
}

/// Enforces the reverse block's per-mille ceiling against the budget document.
///
/// The block is not one of the container's six table sections, so the size ledger
/// does not see it; the ceiling is derived here from the same container ceiling,
/// and a build past it fails the same way a section would.
///
/// # Errors
/// Returns an error when the budget document cannot be read or the block is past
/// its share of the container ceiling.
pub(crate) fn enforce_budget(root: &Path, bytes: u64) -> Result<()> {
    let budgets = SegmentBudgets::load(root)?;
    let scaled = u128::from(budgets.container()) * u128::from(REVERSE_SHARE_PER_MILLE);
    let ceiling = u64::try_from(scaled / PER_MILLE).unwrap_or(u64::MAX);
    println!(
        "dictc: reverse index budget: {bytes} bytes against {ceiling} \
         ({REVERSE_SHARE_PER_MILLE} per-mille of the container ceiling)"
    );
    ensure!(
        bytes <= ceiling,
        "dictc: reverse index {bytes} bytes is past its {ceiling}-byte ceiling \
         ({REVERSE_SHARE_PER_MILLE} per-mille of the container ceiling); the remedy is \
         a narrower decomposition table, not a raised ceiling"
    );
    Ok(())
}

/// Runs the whole pass: the allowlist gate, the table load, the block build, and
/// the budget enforcement, with the build log lines.
///
/// A missing table is the ordinary state of a checkout that predates it; the
/// container is built without the block and the runtime degrades
/// (`reverse/absent`). A present table passes through `allowlist_check` first,
/// because a source that is not registered must stop the build before any of it is
/// compiled, and is then read by the aux pass's own reader, so both passes judge
/// one row the same way.
///
/// # Errors
/// Returns the errors of [`aux::load_table`], [`enforce_budget`], and the
/// allowlist check.
pub(crate) fn build_from(
    root: &Path,
    chaizi: &Path,
    allowlist_check: &dyn Fn(&Path) -> Result<()>,
) -> Result<Option<Block>> {
    if !chaizi.is_file() {
        println!(
            "dictc: decomposition table {} is missing; building without the reverse index",
            chaizi.display()
        );
        return Ok(None);
    }
    allowlist_check(chaizi)?;
    let table = aux::load_table(chaizi)?.with_context(|| {
        format!(
            "dictc: decomposition table {} exists but could not be parsed",
            chaizi.display()
        )
    })?;
    let block = build_block(&table)?;
    enforce_budget(root, block.bytes.len() as u64)?;
    println!(
        "dictc: reverse index: {} characters, {} fragment keys, {} postings",
        block.char_count, block.key_count, block.posting_count
    );
    Ok(Some(block))
}

/// Checks a built block against the shape the container expects of it.
///
/// The aux pass checks its rows against the compiled `ENTRIES`; the reverse block
/// references no word, so its counterpart is the container's own expectation, the
/// one the writer and the reader both state: the block starts with the block
/// magic, and the three header counts name the build's own counters and describe a
/// payload that exactly fills the block. A block that fails this is one whose
/// bytes and counts disagree -- a build defect, caught before the container ships.
/// No block is trivially aligned.
///
/// # Errors
/// Returns an error when the header is truncated, carries the wrong magic,
/// disagrees with the build's counters, or describes a payload of a different
/// length than the block holds.
pub(crate) fn check_alignment(block: Option<&Block>) -> Result<()> {
    let Some(block) = block else {
        return Ok(());
    };
    let bytes = &block.bytes;
    ensure!(
        bytes.len() >= REVERSE_HEADER_SIZE && bytes.get(0..4) == Some(&REVERSE_MAGIC[..]),
        "dictc: reverse block does not start with its magic and header"
    );
    let field = |at: usize| -> Result<usize> {
        let raw = bytes
            .get(at..at + 4)
            .and_then(|raw| <[u8; 4]>::try_from(raw).ok())
            .ok_or_else(|| anyhow!("dictc: reverse block is truncated in its header"))?;
        Ok(u32::from_le_bytes(raw) as usize)
    };
    let char_count = field(8)?;
    let key_count = field(12)?;
    let posting_count = field(16)?;
    ensure!(
        (char_count, key_count, posting_count)
            == (block.char_count, block.key_count, block.posting_count),
        "dictc: reverse block header holds ({char_count}, {key_count}, {posting_count}) \
         for a build that recorded ({}, {}, {}); the bytes and the build disagree",
        block.char_count,
        block.key_count,
        block.posting_count
    );
    let expected =
        char_count * REVERSE_CHAR_ROW_SIZE + key_count * REVERSE_KEY_ROW_SIZE + posting_count * 4;
    let actual = bytes.len() - REVERSE_HEADER_SIZE;
    ensure!(
        actual == expected,
        "dictc: reverse block header describes a {expected}-byte payload for a \
         {actual}-byte one; the block does not fill the shape it announces"
    );
    Ok(())
}

#[cfg(test)]
mod tests;
