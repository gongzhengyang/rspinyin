//! The gates a whole plan passes before the first file is copied.
//!
//! Responsibility: measure every file that carries a size budget against its threshold,
//! and read the dictionary as the untrusted input it is. Both can stop an install that
//! would otherwise produce a plugin that is present and does not work -- a file past the
//! size the specification allows, and a container this build cannot read.
//!
//! The symbol check a library passes is [`super::stage`]'s: it belongs to the copy that
//! is made, not to the plan that is measured, and it has to run before this module sees
//! the file at all.

use std::path::Path;

use anyhow::{Context, Result};

use super::payload::PlannedFile;
use super::size::{Budget, SizeBudgets, check_size};

/// Measures every payload that carries a size budget, and reads the dictionary.
///
/// # Errors
///
/// Returns an error when a file passes its budget, or when the dictionary payload is not
/// a container this build can read.
pub(super) fn verify_sizes(plan: &[PlannedFile], budgets: &SizeBudgets) -> Result<()> {
    for file in plan {
        let Some(budget) = file.budget else {
            continue;
        };
        check_size(budget, &file.source, budgets.limit(budget))?;
        if budget == Budget::BaseDictionary {
            check_dictionary(&file.source)?;
        }
    }
    Ok(())
}

/// Fails when `path` is not a dictionary container this build can read.
///
/// The dictionary is installed from wherever the caller pointed `--dict`, so it is
/// untrusted input by the same rule as any other file the plugin reads: the magic, the
/// format version, the section table and every length and offset are validated *before*
/// the file is copied into the addon's data directory. `Reader::open_with` is the one
/// place that knows how to do that, so this delegates to it rather than growing a second
/// magic-number check that could disagree with the loader's.
///
/// # Errors
///
/// Returns an error naming the file, with the reader's own diagnostic as the cause.
fn check_dictionary(path: &Path) -> Result<()> {
    ime_dict::format::reader::Reader::open_with(path, ime_dict::format::reader::Verify::Full)
        .map_err(anyhow::Error::from)
        .with_context(|| {
            format!(
                "install: {} is not a dictionary this build can read; compile one with \
                 `cargo run -p xtask -- dictc`, or pass a base.dict from a release archive",
                path.display()
            )
        })?;
    println!(
        "install: {} is a readable dictionary container",
        path.display()
    );
    Ok(())
}
