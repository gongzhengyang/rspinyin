//! The retired-key list and the prune that removes it from a document.
//!
//! Responsibility: name every configuration key this build no longer reads, and delete
//! those keys from a document physically, so that a key a rename or a merge has left
//! behind cannot linger in a user's file forever. The list is the data and the prune is
//! the mechanism; the reading pipeline in the parent module owns the ordering that
//! makes pruning safe, and this module owns the rule that ordering follows.
//!
//! # The ordering iron rule
//!
//! "Migrate-and-persist first, prune after." A key that retires while its value moves
//! somewhere else must have the value moved and written to the disk before the old key
//! is deleted, because the migration reads the document the file spells out: a file
//! that lost the key first has lost the value with it, and no later run can migrate
//! what is no longer there. The pipeline honours the rule by pruning only after
//! [`crate::migrate::migrate`] has returned -- which is after the migrated document has
//! been written back -- so an interrupted run always finds the values on the disk and
//! at worst prunes the dead keys a second time.
//!
//! # What a prune may not do
//!
//! It removes exactly the keys the list names and nothing else. A path whose section is
//! absent, or whose section holds something that is not a table, names a key that
//! cannot be there and is skipped; a key the list does not name is never visited,
//! because the prune walks the list rather than the document and its cost is linear in
//! the length of the list alone.
//!
//! # Why the document is a [`toml::Value`]
//!
//! The document is the loader's own model -- the same one [`crate::migrate`] rewrites --
//! rather than a formatting-preserving document model, which would cost a direct
//! `toml_edit` dependency this crate does not carry. Nothing is lost by the choice: the
//! prune runs between the parse and the deserialization, where the file's formatting
//! has already been left behind, and a persistence that renders the document has the
//! same behaviour with either model.

/// The keys this version no longer reads, each as its path from the document root,
/// one segment per table level.
///
/// Empty in this build: the mechanism ships before the first retirement, so the list
/// names nothing and the prune over any document removes nothing. Fill it the moment a
/// key actually retires -- a rename, a merge, a spelling no build reads any more -- and
/// drop the key from [`super::DEFAULT_CONFIG_TOML`] in the same change: a template that
/// still spells a retired key would hand every fresh install a dead key for the next
/// load to prune away again.
///
/// A value that moves as a key retires must already have been migrated and persisted
/// when the entry goes in; the iron rule above is what keeps an interrupted run from
/// losing the value the old key held.
///
/// ```text
/// // An entry retires one key and leaves the section's other keys alone:
/// // &["engine", "old_key_name"] deletes old_key_name from [engine].
/// ```
pub const RETIRED_KEYS: &[&[&str]] = &[];

/// Removes every key of [`RETIRED_KEYS`] from `document`, physically, and returns how
/// many were removed.
///
/// # Parameters
///
/// - `document`: the parsed configuration document, rewritten in place. A key the list
///   names and the document holds is gone from the document the loader deserializes, so
///   a retired spelling stops answering the load that pruned it and every reload after
///   it.
///
/// # Returns
///
/// How many of the listed keys the document held and lost. The document's other keys
/// are not visited: the prune walks the list, not the document.
///
/// # Panics
///
/// Never.
pub fn prune_retired(document: &mut toml::Value) -> usize {
    prune_with(RETIRED_KEYS, document)
}

/// The pruner [`prune_retired`] runs, over the list `retired` names instead of
/// [`RETIRED_KEYS`].
///
/// The list is a parameter so that a test can drive the mechanism before the shipped
/// list holds its first entry: every guard -- the coverage of the list, the idempotence
/// of the prune, the ordering rule, the round trip -- is exercised against a key the
/// test retires for its own duration, and the shipped empty list stays a statement that
/// this build retires nothing.
///
/// # Parameters
///
/// - `retired`: the key paths to remove, in the form [`RETIRED_KEYS`] documents.
/// - `document`: the parsed configuration document, rewritten in place.
///
/// # Returns
///
/// How many of the listed keys the document held and lost.
///
/// # Panics
///
/// Never.
pub(super) fn prune_with(retired: &[&[&str]], document: &mut toml::Value) -> usize {
    let mut removed = 0;
    for path in retired {
        if remove_key(document, path) {
            removed += 1;
        }
    }
    removed
}

/// Removes the one key `path` names from `document`.
///
/// The walk stops at the first segment that does not lead into a table: a path into an
/// absent section, or into a value that is not a table, names a key that cannot be
/// there, and whatever the document holds where the walk stopped is left exactly as it
/// is.
fn remove_key(document: &mut toml::Value, path: &[&str]) -> bool {
    let Some((key, sections)) = path.split_last() else {
        // An empty path names nothing, so an empty entry of the list retires nothing
        // rather than clearing the document.
        return false;
    };
    let mut value = document;
    for section in sections {
        let Some(nested) = value
            .as_table_mut()
            .and_then(|table| table.get_mut(*section))
        else {
            return false;
        };
        value = nested;
    }
    match value.as_table_mut() {
        Some(table) => table.remove(*key).is_some(),
        None => false,
    }
}
