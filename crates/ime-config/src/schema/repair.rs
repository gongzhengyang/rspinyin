//! The repair rules for the `[keys]` section.
//!
//! Responsibility: the two rules a configuration's key bindings answer to -- the
//! deduplication and length bound of each list, and the settlement of a key both
//! lists claim -- and the diagnostic the settlement reports. They live apart from
//! the schema root because they are the rules that edit a list rather than replace
//! a scalar, and because the settlement asks the keymap's projection which of the
//! two lists a key can serve. They are private helpers of
//! [`crate::schema::Config::repaired`].

use ime_types::ImeError;

use super::{KEY_FLIP_KEYS, KEY_HIGHLIGHT_KEYS, KeyName, KeysConfig, MAX_KEY_BINDINGS, Warnings};
use crate::keymap::{BINDING_CONFLICT_CODE, project_keys};

/// Drops from `keys.flip_keys` every key `keys.highlight_keys` already claims, reporting
/// each one.
///
/// The two lists are separate settings but one keymap: a key cannot page the candidate list
/// and move the highlight at the same time. The overlap used to be accepted silently, and
/// the routing table then settled it by evaluation order -- a decision the user never saw.
/// Settling it here, in the direction the router already applies, makes the configuration
/// and the router agree by construction, and the repair is idempotent: what it produces has
/// no overlap left to report.
///
/// Only a key both lists can act on is a conflict. A name one of the two lists cannot route
/// is the projection's diagnostic rather than this one's, and reporting it here would
/// describe a clash that never existed.
pub(super) fn repair_cross_list_conflicts(keys: &mut KeysConfig, warnings: &mut Warnings) {
    let claimed = keys.highlight_keys.clone();
    let mut shared: Vec<KeyName> = Vec::new();
    for name in &keys.flip_keys {
        if claimed.contains(name) && !shared.contains(name) && both_lists_can_route(*name, keys) {
            shared.push(*name);
        }
    }
    if shared.is_empty() {
        return;
    }
    keys.flip_keys.retain(|name| !shared.contains(name));
    for name in shared {
        warnings.report_ime_error(cross_list_conflict(name));
    }
}

/// Whether `keys.flip_keys` and `keys.highlight_keys` can both act on `name`.
///
/// Asked of the projection rather than restated here: which names a list can carry is that
/// module's table, and a second copy of it in this one would be a second answer, free to
/// drift from the one the router reads. The name is bound in one list at a time, because
/// binding it in both would make the projection settle the very conflict this asks about.
///
/// # Panics
///
/// Never: the two probes read a list of one name each and write a flag set.
pub(super) fn both_lists_can_route(name: KeyName, keys: &KeysConfig) -> bool {
    let page = KeysConfig {
        flip_keys: vec![name],
        highlight_keys: Vec::new(),
        ..keys.clone()
    };
    let highlight = KeysConfig {
        flip_keys: Vec::new(),
        highlight_keys: vec![name],
        ..keys.clone()
    };
    !project_keys(&page).0.flip_keys.is_empty()
        && !project_keys(&highlight).0.highlight_keys.is_empty()
}

/// The diagnostic for a key both binding lists claim.
///
/// The code is the projection's own: the two layers answer one question -- which list owns
/// a key both of them named -- and a second code for it would make a diagnostic and a test
/// match on two spellings of one condition. The reason names the list that gives way, so
/// the rendered message says which line of the document to edit.
///
/// # Panics
///
/// Never.
fn cross_list_conflict(name: KeyName) -> ImeError {
    ImeError::ConfigInvalid {
        key: String::from(BINDING_CONFLICT_CODE),
        reason: format!(
            "{KEY_FLIP_KEYS} names \"{}\", which {KEY_HIGHLIGHT_KEYS} also claims; \
             the highlight binding is kept",
            name.as_str()
        ),
    }
}

/// Drops the repeated entries of one key-binding list and then any entry past
/// [`MAX_KEY_BINDINGS`], reporting each one.
///
/// The first position of a repeated key is the one that survives, so a list the user
/// edited by hand keeps the order they gave it. The length bound is the number of keys a
/// list can route, which is what makes it a capacity rather than a number: an entry past
/// it could never have done anything, and dropping it is the honest answer to a list longer
/// than the keymap it describes.
pub(super) fn repair_bindings(names: &mut Vec<KeyName>, key: &str, warnings: &mut Warnings) {
    let mut unique: Vec<KeyName> = Vec::with_capacity(names.len());
    for name in names.iter().copied() {
        if unique.contains(&name) {
            warnings.report(key, format!("repeated key binding: {}", name.as_str()));
        } else {
            unique.push(name);
        }
    }
    if unique.len() > MAX_KEY_BINDINGS {
        warnings.report_limit(key, MAX_KEY_BINDINGS);
        unique.truncate(MAX_KEY_BINDINGS);
    }
    *names = unique;
}
