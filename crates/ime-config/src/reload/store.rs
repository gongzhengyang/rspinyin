//! The configuration in force, and the reload path that replaces it.
//!
//! Responsibility: [`ConfigStore`] owns the `Arc<Config>` the rest of the plugin holds,
//! the file that configuration was read from, and the binding table projected from its
//! `[keys]` section. [`ReloadOutcome`] is what one [`ConfigStore::reload`] produced.
//!
//! Boundaries: the document model and the merge over the built-in defaults live in the
//! parent module, and the filesystem half -- where the file is, and how it is read at
//! startup -- lives in the `load` sibling. This module owns what is *in force*, and the
//! one rule that shapes it: a reload replaces the configuration and everything projected
//! from it, or it replaces nothing.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ime_types::{ImeError, SchemeId};

use super::{document_error, load::unix_secs};
use crate::keymap::{KeyBindings, project_keys};
use crate::schema::Config;

/// What one reload produced.
#[derive(Debug)]
pub enum ReloadOutcome {
    /// The file parsed to a configuration different from the one in force, which is now
    /// active, and the binding table projected from its `[keys]` section is now in force
    /// with it.
    Updated {
        /// Diagnostics raised while the new document was read and while its binding table
        /// was projected.
        warnings: Vec<ImeError>,
    },
    /// The file parsed to exactly the configuration already in force, so nothing changed.
    /// The diagnostics were reported when that configuration was adopted.
    ///
    /// The binding table is a pure function of the `[keys]` section, which is part of that
    /// configuration, so it is unchanged as well and is not projected again: the
    /// diagnostics a second projection would raise are the ones adoption already reported.
    Unchanged,
    /// The file was missing or could not be parsed. The configuration in force is kept
    /// unchanged -- and with it the binding table, which a reload that has nothing to
    /// adopt must not disturb -- and the diagnostics say why.
    Kept {
        /// Diagnostics explaining why the configuration in force was kept.
        warnings: Vec<ImeError>,
    },
}

/// The configuration in force, the file it was read from, and the binding table it
/// projects to.
///
/// The store owns the `Arc` the rest of the plugin holds, so a reload replaces one
/// pointer: a component that took a copy of the previous `Arc` -- a decoder in the middle
/// of a composition, say -- keeps the configuration it started with and is never
/// rewritten underneath. That is what makes a reload unable to disturb an input session
/// that is in progress (0.4 rule 10).
///
/// The binding table is projected at the same moment the configuration is adopted and is
/// handed out by value ([`ConfigStore::bindings`]), so the two cannot disagree -- a
/// reload replaces both or neither. A table a caller already holds is a copy and stays
/// what it was, which is how the routing layer keeps the bindings a composition started
/// with.
pub struct ConfigStore {
    /// The file the configuration is read from and reloaded from.
    path: PathBuf,
    /// The configuration in force.
    current: Arc<Config>,
    /// The `[keys]` section of [`ConfigStore::current`] as the routing layer reads it.
    ///
    /// Projected once per adoption rather than per key: the routing layer's copy of this
    /// value is what the host thread reads, and a table the store recomputed would put
    /// the projection in the key path for nothing.
    bindings: KeyBindings,
}

impl ConfigStore {
    /// Loads the configuration for the first time, stamping a corrupt file's backup name
    /// with the current system time.
    ///
    /// # Errors
    ///
    /// None: the diagnostics are the report.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn load(path: &Path) -> (Self, Vec<ImeError>) {
        Self::load_at(path, unix_secs())
    }

    /// Loads the configuration for the first time, stamping a corrupt file's backup name
    /// with `unix_secs`.
    ///
    /// The `[keys]` section is projected here, once, so the binding table is in force from
    /// the first key rather than from the first reload.
    ///
    /// # Returns
    ///
    /// The store, and every diagnostic raised while reading the file *and* while
    /// projecting its binding table: a key-binding entry that cannot become a binding is
    /// reported under the same channel as an unreadable document, because the caller
    /// answers both the same way -- by carrying on with what it has.
    ///
    /// # Errors
    ///
    /// None: the diagnostics are the report.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn load_at(path: &Path, unix_secs: u64) -> (Self, Vec<ImeError>) {
        let (config, mut warnings) = Config::load_at(path, unix_secs);
        // The projection runs on the repaired configuration, never on the document: an
        // entry `Config::repaired` already dropped cannot raise a second diagnostic here.
        let (bindings, mut rejected) = project_keys(&config.keys);
        warnings.append(&mut rejected);
        let store = Self {
            path: path.to_path_buf(),
            current: Arc::new(config),
            bindings,
        };
        (store, warnings)
    }

    /// The configuration in force.
    ///
    /// The `Arc` is what a caller hands to another thread; the value behind it is never
    /// mutated, so a reader always sees one consistent configuration.
    pub fn current(&self) -> &Arc<Config> {
        &self.current
    }

    /// The binding table in force: the `[keys]` section of [`ConfigStore::current`] as the
    /// routing layer reads it.
    ///
    /// # Returns
    ///
    /// The table projected from the configuration in force, with every entry of `[keys]`
    /// that could become a binding. A copy, so a caller that took one keeps routing the
    /// way it did when it took it, whatever a later reload does to the store.
    ///
    /// # Errors
    ///
    /// None: an entry that could not become a binding was reported when the configuration
    /// was adopted, by [`ConfigStore::load_at`] or by [`ConfigStore::reload`].
    ///
    /// # Panics
    ///
    /// Never.
    pub fn bindings(&self) -> KeyBindings {
        self.bindings
    }

    /// The decode settings in force: the `[scheme]` section of [`ConfigStore::current`] as
    /// the session state machine reads it.
    ///
    /// # Returns
    ///
    /// The layout the keystrokes follow, and whether a keystroke that layout cannot read
    /// is read as full pinyin: the pair
    /// [`crate::scheme::SchemeConfig::decode_settings`] projects. The layout travels as
    /// the contract's own number rather than as the section's spelling, so a caller never
    /// has to know that a layout this build cannot compile answers full pinyin.
    ///
    /// # Errors
    ///
    /// None: a section this build cannot honour was repaired when the configuration was
    /// adopted, so the projection is always usable.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn scheme(&self) -> (SchemeId, bool) {
        self.current.scheme.decode_settings()
    }

    /// Re-reads the configuration file: the entry point fcitx5's `reloadConfig()` calls.
    ///
    /// It is deliberately not the load path. A reload improves the configuration or
    /// leaves it alone, and never writes to the file or falls back to the defaults.
    ///
    /// The `[keys]` section of the new document is projected here and the resulting table
    /// replaces the one in force whole, in the same step as the configuration it came
    /// from.
    ///
    /// # Returns
    ///
    /// [`ReloadOutcome::Updated`] with the diagnostics of the new document and of its
    /// binding table when the file parsed to a different configuration,
    /// [`ReloadOutcome::Unchanged`] when it parsed to the one already in force, and
    /// [`ReloadOutcome::Kept`] with the reason when the file could not be read or parsed.
    ///
    /// # Errors
    ///
    /// None: the outcome carries the diagnostics.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn reload(&mut self) -> ReloadOutcome {
        let mut warnings = Vec::new();
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) => {
                // A file that cannot be read -- gone, or unreadable -- leaves the
                // configuration in force alone. A missing file is far likelier to be a
                // save in progress or a mistake than a deliberate reset, and a restart is
                // the unambiguous way back to the built-in defaults.
                warnings.push(ImeError::from(document_error(format!(
                    "cannot read the configuration file: {error}"
                ))));
                return ReloadOutcome::Kept { warnings };
            }
        };
        let (config, mut parsed) = match Config::from_document(&text) {
            Ok(parsed) => parsed,
            Err(error) => {
                // The file is left exactly as it is: it may be an edit in progress, and
                // the configuration in force is known to be good.
                warnings.push(ImeError::from(error));
                return ReloadOutcome::Kept { warnings };
            }
        };
        warnings.append(&mut parsed);
        if config == *self.current {
            // The document parsed to the configuration already in force, and the table is
            // a pure function of its `[keys]` section, so the table in force is already
            // the one this document projects to. Re-projecting it would recompute the
            // diagnostics adoption already reported, which is the one thing `Unchanged`
            // promises not to do twice.
            return ReloadOutcome::Unchanged;
        }
        // The section that replaces the one in force is re-projected, and the result is
        // adopted whole: no page key of the previous configuration can survive beside a
        // highlight key of the new one. The projection reads nothing but the document and
        // writes nothing but the table, so a composition in progress keeps its input, its
        // candidates and its highlight, and the new bindings govern the keys that arrive
        // after the reload (0.4 rule 10).
        let mut bindings = self.bindings;
        let mut rejected = bindings.reproject(&config.keys);
        warnings.append(&mut rejected);
        self.current = Arc::new(config);
        self.bindings = bindings;
        ReloadOutcome::Updated { warnings }
    }
}
