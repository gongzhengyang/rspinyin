//! The two addons this product ships, and whether a session loaded them.
//!
//! Responsibility: read the installed addon descriptors, and turn them plus a Fcitx5 session
//! log into one state per expected addon. Boundaries: it never starts a session and never
//! reads the plugin's own configuration. The load verdict comes from a log a session wrote,
//! which is the only place Fcitx5 states one.
//!
//! # Why the verdict comes from the log
//!
//! `fcitx5-diagnose` reports whether an addon's library can be found and whether `ldd`
//! resolves, but it never prints a per-addon load verdict at all -- measured against Fcitx5
//! 5.1.7, it also hardcodes the system addon directories, so pointed at a sandbox it
//! describes the system installation instead. A session's own log says `Loaded addon <name>`
//! or one of the failure messages, and that is what the sandbox reads and what is read here.
//!
//! # Why both addons are named explicitly
//!
//! `Category` is single-valued in Fcitx5 and the input method is discovered from
//! `Category=InputMethod` addons, so the engine and the user interface cannot be one addon:
//! ADR-0003 splits them into two libraries with two descriptors. A run that loaded only one
//! of them has half an input method, so a missing addon is reported by name rather than
//! folded into a single boolean.

use std::fs;
use std::path::{Path, PathBuf};

use ime_dict::paths::BaseDirs;

use crate::testd::sandbox::parse_addons;

/// The directory addon descriptors live in, below each data directory.
const ADDON_SUBDIR: &str = "fcitx5/addon";

/// The system data directories a standard installation reads descriptors from.
const SYSTEM_DATA_DIRS: [&str; 2] = ["/usr/local/share", "/usr/share"];

/// The section a Fcitx5 addon descriptor declares itself in.
const ADDON_SECTION: &str = "[Addon]";

/// The key inside it that names the addon's category.
const CATEGORY_KEY: &str = "Category";

/// The file name extension of an addon descriptor.
const CONF_EXTENSION: &str = "conf";

/// One addon this product ships.
///
/// The name is the descriptor's file stem, which is also the name Fcitx5 logs. Each addon has
/// a library of its own (`librspinyin.so` and `librspinyin_ui.so`, per ADR-0004), but the
/// library is named by the descriptor and never by this table: the report is about what
/// loaded, and an addon's identity in Fcitx5 is its addon name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddonSpec {
    /// The addon id.
    pub name: &'static str,
    /// The category its descriptor declares.
    pub category: &'static str,
}

/// The two addons this product is packaged as.
pub const EXPECTED_ADDONS: [AddonSpec; 2] = [
    AddonSpec {
        name: "rspinyin",
        category: "InputMethod",
    },
    AddonSpec {
        name: "rspinyin-ui",
        category: "UI",
    },
];

/// An addon descriptor as its file was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddonDescriptor {
    /// The addon id, which is the descriptor's file stem.
    pub name: String,
    /// The descriptor's text.
    pub text: String,
}

/// One addon's state in the environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddonState {
    /// The addon id.
    pub name: String,
    /// Whether a session log reported this addon as loaded.
    pub loaded: bool,
    /// The category the installed descriptor declares, or the registered one when no
    /// descriptor could be read.
    pub category: String,
}

/// The addon states, from the installed descriptors and a session log.
///
/// Without a log no addon is reported as loaded: the state of a load is a fact about a
/// session, and a report that guessed it would be worse than one that says it does not know.
/// The report carries a gap for that case.
///
/// # Panics
///
/// Never.
pub fn states(descriptors: &[AddonDescriptor], log: Option<&str>) -> Vec<AddonState> {
    let readiness = log.map(parse_addons);
    EXPECTED_ADDONS
        .iter()
        .map(|spec| AddonState {
            name: spec.name.to_owned(),
            loaded: readiness
                .as_ref()
                .is_some_and(|ready| ready.is_loaded(spec.name)),
            category: category_in(descriptors, spec),
        })
        .collect()
}

/// The `Category=` value inside a descriptor's `[Addon]` section.
///
/// Section-scoped rather than a plain scan, because the same key name can legitimately appear
/// in another section and the answer is about which one. Only the first occurrence inside the
/// section is honoured, which is how Fcitx5's own parser resolves a duplicate.
///
/// # Panics
///
/// Never.
pub fn category_of(descriptor: &str) -> Option<String> {
    let mut inside = false;
    for line in descriptor.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == ADDON_SECTION;
            continue;
        }
        if !inside {
            continue;
        }
        let (key, value) = match line.split_once('=') {
            Some(pair) => pair,
            None => continue,
        };
        if key.trim() == CATEGORY_KEY {
            let value = value.trim();
            return (!value.is_empty()).then(|| value.to_owned());
        }
    }
    None
}

/// The directories addon descriptors are read from, in search order.
///
/// The system data directories come first and the user's own last, which is the order
/// Fcitx5's `StandardPath` searches them in. A sandbox run replaces all of this with
/// `FCITX_ADDON_DIRS`, which is why what is read here is the *installed* set: the report says
/// what the machine has, and the sandbox says what a run staged.
///
/// # Panics
///
/// Never.
pub fn descriptor_dirs(bases: Option<&BaseDirs>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = SYSTEM_DATA_DIRS
        .iter()
        .map(|dir| Path::new(dir).join(ADDON_SUBDIR))
        .collect();
    if let Some(bases) = bases {
        dirs.push(bases.data_home.join(ADDON_SUBDIR));
    }
    dirs
}

/// The descriptors of the addons this product ships, read from `dirs`.
///
/// Only the expected addons are looked for: every other addon on the machine says nothing
/// about this product, and reading them would make the report grow with the installation.
/// The first directory that holds a descriptor wins, which is what a search path means.
///
/// # Panics
///
/// Never.
pub fn descriptors_in(dirs: &[PathBuf]) -> Vec<AddonDescriptor> {
    let mut found = Vec::new();
    for spec in EXPECTED_ADDONS {
        let name = spec.name;
        let file = format!("{name}.{CONF_EXTENSION}");
        for dir in dirs {
            if let Ok(text) = fs::read_to_string(dir.join(&file)) {
                found.push(AddonDescriptor {
                    name: name.to_owned(),
                    text,
                });
                break;
            }
        }
    }
    found
}

/// The installed descriptors, read from the standard directories.
///
/// # Panics
///
/// Never.
pub fn descriptors(bases: Option<&BaseDirs>) -> Vec<AddonDescriptor> {
    descriptors_in(&descriptor_dirs(bases))
}

/// The category of `spec`'s addon: the one its descriptor declares, or the registered one.
fn category_in(descriptors: &[AddonDescriptor], spec: &AddonSpec) -> String {
    descriptors
        .iter()
        .find(|descriptor| descriptor.name == spec.name)
        .and_then(|descriptor| category_of(&descriptor.text))
        .unwrap_or_else(|| spec.category.to_owned())
}

// The tests live in a sibling file rather than inside this one: together they would be longer
// than the file limit allows. A `#[path]` child module keeps them inside `addons`.
#[cfg(test)]
#[path = "addons/tests.rs"]
mod tests;
