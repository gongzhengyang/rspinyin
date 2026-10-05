//! The configuration in force: the store the `config` step installs, the routing table
//! projected from it, and the reload the host triggers.
//!
//! Responsibility: read `$XDG_CONFIG_HOME/rspinyin/config.toml` into a [`ConfigStore`],
//! keep that store in the process-wide slot every later reader goes through, project its
//! `[keys]` section onto the routing layer's table, hand the `[diagnostics]` and `[data]`
//! sections the engine applies itself to the steps that own them — at load and at every
//! reload that adopts a document — and re-read the file when the host reports that it
//! changed.
//!
//! Boundaries: `ime-config` owns the document, the merge over the built-in defaults and
//! the projection; this module owns where the store lives and which steps read it. It never
//! writes the file, and it never resets a composition — a reload replaces the configuration
//! and everything projected from it, or it replaces nothing (`AGENTS.md` prohibition 23).
//!
//! # Nothing here is fatal
//!
//! A document that cannot be read or parsed leaves the built-in defaults in force, which is
//! what a fresh installation routes with; the damaged file is moved aside by the loader and
//! reported as `config/invalid`. An environment with no configuration directory at all
//! leaves the defaults in force as well. Neither costs the user their input method.

use std::path::Path;
use std::sync::Mutex;

use ime_config::{Config, ConfigStore, ReloadOutcome};
use ime_types::ImeError;

use crate::engine::router::RoutingConfig;
use crate::ffi::emit_diagnostic;

use super::{diagnostics, lock, probes, report_step_failure, user_store};

/// The configuration in force: installed by the `config` step, re-read by a reload.
///
/// A lock over a slot rather than a `OnceLock` because a reload replaces the store in place.
/// The lock is a leaf: it is never held across a diagnostic write, and no other lock is
/// taken while it is held.
pub(super) static CONFIG: Mutex<Option<ConfigStore>> = Mutex::new(None);

/// The routing layer's view of [`CONFIG`]: installed by the `key-bindings` step.
///
/// A router keeps its own copy — `RoutingConfig` is `Copy` — so the key path reads no lock
/// at all, and a reload reaches a live router through `KeyRouter::reload` rather than
/// through this slot.
pub(super) static ROUTING: Mutex<Option<RoutingConfig>> = Mutex::new(None);

/// The `config` step: reads the user's configuration into the store in force.
///
/// # Returns
///
/// `Ok(())` whether or not a document could be read: a document that is missing, unreadable
/// or not TOML leaves the built-in defaults in force, and an environment with no
/// configuration directory leaves the store uninstalled.
///
/// # Errors
///
/// None. A configuration problem must never cost the user their input method.
///
/// # Panics
///
/// Never.
pub(super) fn load_config() -> Result<(), ImeError> {
    load_config_at(ime_config::default_path().as_deref())
}

/// Loads the configuration document at `path` into the store in force.
///
/// The entry point a test drives, since with the path injected the document is the test's
/// own and no environment is read.
///
/// # Arguments
///
/// * `path` — the document to read, or `None` when the environment names no configuration
///   directory.
///
/// # Returns
///
/// `Ok(())` always: what the load found is reported on the diagnostic channel.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never.
pub(super) fn load_config_at(path: Option<&Path>) -> Result<(), ImeError> {
    let Some(path) = path else {
        // No configuration directory to read from, so the shipped defaults are the
        // configuration in force. Reported rather than left to be discovered: a plugin
        // whose routing table is not the user's is worth a line in the log.
        report_step_failure("config", &"no XDG configuration directory");
        return Ok(());
    };
    let (store, warnings) = ConfigStore::load(path);
    for warning in &warnings {
        emit_diagnostic(&warning.to_string());
    }
    // The subscriber was installed before this step, deliberately: `tracing` allows one
    // subscriber per process and the diagnostics step runs first (`BUDGET-LAT-05`). What
    // this step does with the `[diagnostics]` section is therefore re-aim the policy the
    // installed sink applies — the level, the rotation size and the kept-file count —
    // through the handle, which is an atomic swap and no reinstall, and to switch the
    // probes, which are a runtime flag on a structure that already exists.
    probes::set_enabled(store.current().diagnostics.probes);
    diagnostics::apply_diagnostics_config(&store.current().diagnostics);
    install_config_store(store);
    Ok(())
}

/// Installs `store` as the configuration in force.
///
/// Replaces whatever was there: a load or a reload adopts a whole document at once, and a
/// half-replaced configuration is the state the store exists to make impossible.
///
/// # Panics
///
/// Never.
pub(super) fn install_config_store(store: ConfigStore) {
    *lock(&CONFIG) = Some(store);
}

/// The `key-bindings` step: projects the configuration in force onto the routing layer.
///
/// `[keys]` becomes the binding table, `[ui]` and `[engine]` the values a session is stepped
/// with, and `[scheme]` the label the window's header shows. What it installs is what
/// [`routing_config`] answers.
///
/// The step never fails, which is why it is not fatal. With no configuration store — an
/// environment that names no configuration directory — the built-in defaults are the
/// configuration in force, which is what a fresh installation routes with. An entry of
/// `[keys]` that cannot become a binding is reported, because this is where it is dropped,
/// and a typo in a key binding costs the user that binding and never their input method.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never.
pub(super) fn init_key_bindings() -> Result<(), ImeError> {
    let projected = with_config_store(|store| RoutingConfig::from_config(store.current()));
    let (routing, warnings) = match projected {
        Some(projected) => projected,
        None => {
            report_step_failure("key-bindings", &"no configuration store");
            RoutingConfig::from_config(&Config::default())
        }
    };
    for warning in &warnings {
        emit_diagnostic(&warning.to_string());
    }
    install_routing_config(routing);
    Ok(())
}

/// The `config-watch` step: records the reload subscription the host reaches.
///
/// The subscription is the handler the host calls when Fcitx5 reports that the
/// configuration changed: [`on_config_reload`] re-reads the document, reports what it found
/// and adopts the routing table the new document projects to. The store it re-reads and the
/// table it replaces are what the steps above install, so this step is where the sequence
/// records the subscription. The trigger itself is the host's `reloadConfig()` callback,
/// which the glue's override forwards to the `rspinyin_config_reload` export; what the
/// step's `pending` line says is the one thing still missing, whichever of the two halves
/// of the subscription that is.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never.
pub(super) fn start_config_watch() -> Result<(), ImeError> {
    // Which half of the subscription is missing, when either is: a build with no store has
    // nothing to re-read, and one with a store is wired end to end — the glue's override
    // reaches the export, the export reaches the reload handler — and only waits for the
    // host to fire it, which at load time it never yet has.
    let awaiting = if has_config_store() {
        "the host's first reloadConfig call"
    } else {
        "a configuration store"
    };
    super::pending_step("config-watch", awaiting);
    Ok(())
}

/// The routing configuration in force.
///
/// The value a caller builds a `KeyRouter` with: the one the `key-bindings` step projected,
/// or the shipped defaults when that step ran with no configuration store.
///
/// # Returns
///
/// A copy: a caller that takes one keeps routing the way it did, whatever a later reload
/// installs.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never.
pub fn routing_config() -> RoutingConfig {
    lock(&ROUTING).as_ref().copied().unwrap_or_default()
}

/// Installs `routing` as the routing configuration in force.
///
/// # Panics
///
/// Never.
pub(super) fn install_routing_config(routing: RoutingConfig) {
    *lock(&ROUTING) = Some(routing);
}

/// Whether a configuration store is installed.
///
/// # Panics
///
/// Never.
fn has_config_store() -> bool {
    lock(&CONFIG).is_some()
}

/// Borrows the configuration store and runs `read` against it, or answers `None` when none
/// is installed.
///
/// The lock is held for the call, so `read` must not block on anything else and must not
/// report a diagnostic itself: a write to the crash channel is the one thing that must never
/// happen while this lock is held.
///
/// # Panics
///
/// Never.
pub(super) fn with_config_store<T>(read: impl FnOnce(&ConfigStore) -> T) -> Option<T> {
    lock(&CONFIG).as_ref().map(read)
}

/// Re-reads the configuration file and adopts what changed.
///
/// The entry point the host's `reloadConfig()` callback calls; [`start_config_watch`] is
/// where the sequence records the slot that reaches it. A reload may improve the
/// configuration and nothing else: the file is never written, a document that cannot be
/// parsed never replaces one that can, and a composition in progress is never reset.
///
/// What the engine applies itself is applied from the document that was adopted: the
/// routing table is installed, and the `[diagnostics]` and `[data]` halves the engine
/// owns are re-aimed in place — the log policy through the diagnostics handle, the
/// user-store batching window through the store. A document that is kept because it
/// cannot be parsed replaces nothing, so nothing is re-aimed either.
///
/// # Returns
///
/// The routing configuration in force after the reload: the value a caller hands to
/// [`KeyRouter::reload`](crate::engine::router::KeyRouter::reload) so that a live router
/// adopts it, and the value [`routing_config`] answers from now on.
///
/// # Errors
///
/// None: a document that cannot be read or parsed keeps the configuration in force, and the
/// reason travels on the diagnostic channel.
///
/// # Panics
///
/// Never.
pub fn on_config_reload() -> RoutingConfig {
    // The re-read and the projection happen under one lock, which is released before
    // anything is reported: a diagnostic write must never happen while it is held.
    let reloaded = {
        let mut slot = lock(&CONFIG);
        slot.as_mut().map(|store| {
            let outcome = store.reload();
            let routing = match &outcome {
                ReloadOutcome::Updated { .. } => Some(RoutingConfig::from_config(store.current())),
                ReloadOutcome::Unchanged | ReloadOutcome::Kept { .. } => None,
            };
            // The sections the engine applies itself ride along, so they are read from the
            // document that was adopted rather than fetched again after the lock is
            // released. Both are `Copy`, so the snapshot is two small values.
            let applied = match &outcome {
                ReloadOutcome::Updated { .. } => {
                    Some((store.current().diagnostics, store.current().data))
                }
                ReloadOutcome::Unchanged | ReloadOutcome::Kept { .. } => None,
            };
            (outcome, routing, applied)
        })
    };
    let Some((outcome, routing, applied)) = reloaded else {
        // Nothing has been read, so nothing can have changed.
        report_step_failure("config-reload", &"no configuration store");
        return routing_config();
    };
    report_reload(&outcome);
    let Some((routing, warnings)) = routing else {
        return routing_config();
    };
    for warning in &warnings {
        emit_diagnostic(&warning.to_string());
    }
    // `applied` is `Some` exactly when `routing` is — both match the same outcome — so the
    // adopted document's engine halves land whenever the routing table does.
    if let Some((diag, data)) = applied {
        diagnostics::apply_diagnostics_config(&diag);
        user_store::apply_data_config(&data);
    }
    install_routing_config(routing);
    routing
}

/// Reports what one reload produced on the diagnostic channel.
///
/// `Unchanged` needs no line: nothing changed, and the document in force was reported when
/// it was adopted. The other two carry the diagnostics of the re-read, and a line each is
/// what tells a user why their edit did or did not land.
///
/// # Panics
///
/// Never.
fn report_reload(outcome: &ReloadOutcome) {
    let warnings = match outcome {
        ReloadOutcome::Updated { warnings } | ReloadOutcome::Kept { warnings } => warnings,
        ReloadOutcome::Unchanged => return,
    };
    for warning in warnings {
        emit_diagnostic(&warning.to_string());
    }
}
