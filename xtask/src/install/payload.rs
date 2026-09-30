//! The files an install puts on the system, and the plan they become.
//!
//! Responsibility: name every file the plugin installs, say where each one comes from and
//! where it belongs, and resolve the two into the plan an install carries out. It decides
//! *what* an install consists of; [`super::layout`] decides where each role goes, and
//! [`super::size`] says which of the files are measured against a budget.
//!
//! # Why a table rather than a list of literal paths
//!
//! The two libraries come first, and they are the reason the table exists: the installer
//! has to treat them differently from the rest -- each is staged, stripped and checked for
//! [`FACTORY_SYMBOL`](super::FACTORY_SYMBOL) -- and the second library goes through exactly
//! the same steps as the first. That is also why the destination model did not have to
//! change when ADR-0003 split the plugin in two.
//!
//! # Adding the second library
//!
//! The plugin ships as two addons -- an `InputMethod` engine and a `UI` addon -- and the
//! build produces the engine today. Everything an install does is driven by [`PAYLOADS`],
//! so the UI addon is two rows there and nothing else: its library with
//! [`Destination::AddonLibrary`], which puts it through the symbol check, and its
//! descriptor with [`Destination::AddonDescriptor`]. Uninstall needs no change at all: it
//! works from the manifest, which is written from the same table.

use std::path::PathBuf;

use anyhow::Result;

use super::DICTIONARY;
use super::layout::{Destination, Layout, Sources};
use super::size::Budget;

/// One file an install puts on the system.
pub(super) struct Payload {
    /// File name at the destination.
    pub(super) name: &'static str,
    /// Paths of the built file, relative to the repository root; the first one present
    /// is the one installed.
    pub(super) artifact: &'static [&'static str],
    /// Directory the file belongs in.
    pub(super) destination: Destination,
    /// Whether a missing artifact is skipped rather than reported.
    ///
    /// No payload is optional: the icons are committed to the repository rather than
    /// produced by the build, so an absent one is a broken checkout, and an install that
    /// skipped it would leave `fcitx5-configtool` showing a placeholder for the input
    /// method.
    pub(super) optional: bool,
    /// The size budget the file is measured against, if it has one.
    pub(super) budget: Option<Budget>,
}

/// Everything an install puts on the system, in the order it is copied.
///
/// The two libraries come first, and they are the reason the table exists rather than a
/// list of literal paths: the installer has to treat them differently from the rest --
/// each is staged, stripped and checked for [`FACTORY_SYMBOL`](super::FACTORY_SYMBOL) --
/// and the second library goes through exactly the same steps as the first. That is also
/// why the destination model did not have to change when ADR-0003 split the plugin in two.
pub(super) const PAYLOADS: &[Payload] = &[
    Payload {
        name: "librspinyin.so",
        artifact: &["target/release/librspinyin.so"],
        destination: Destination::AddonLibrary,
        optional: false,
        budget: Some(Budget::StrippedLibrary),
    },
    Payload {
        name: "librspinyin_ui.so",
        artifact: &["target/release/librspinyin_ui.so"],
        destination: Destination::AddonLibrary,
        optional: false,
        budget: Some(Budget::StrippedLibrary),
    },
    Payload {
        name: "rspinyin.conf",
        artifact: &["packaging/fcitx5/rspinyin.conf"],
        destination: Destination::AddonDescriptor,
        optional: false,
        budget: None,
    },
    Payload {
        name: "rspinyin-ui.conf",
        artifact: &["packaging/fcitx5/rspinyin-ui.conf"],
        destination: Destination::AddonDescriptor,
        optional: false,
        budget: None,
    },
    Payload {
        name: "rspinyin.conf",
        artifact: &["packaging/fcitx5/rspinyin-im.conf"],
        destination: Destination::InputMethodDescriptor,
        optional: false,
        budget: None,
    },
    Payload {
        name: "base.dict",
        artifact: DICTIONARY,
        destination: Destination::Data,
        optional: false,
        budget: Some(Budget::BaseDictionary),
    },
    Payload {
        name: "fcitx-rspinyin.png",
        artifact: &["assets/icon-48.png"],
        destination: Destination::Icon { size: "48x48" },
        optional: false,
        budget: None,
    },
    Payload {
        name: "fcitx-rspinyin.svg",
        artifact: &["assets/icon.svg"],
        destination: Destination::Icon { size: "scalable" },
        optional: false,
        budget: None,
    },
    Payload {
        name: "org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml",
        artifact: &["packaging/metainfo/org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml"],
        destination: Destination::MetaInfo,
        optional: false,
        budget: None,
    },
];

/// One file an install will copy, once both of its paths are known.
#[derive(Debug)]
pub(super) struct PlannedFile {
    /// File name at the destination.
    pub(super) name: &'static str,
    /// Path of the built file to copy; the staged copy for a library.
    pub(super) source: PathBuf,
    /// Absolute destination path, with `DESTDIR` applied.
    pub(super) destination: PathBuf,
    /// Whether the file is a shared library Fcitx5 `dlopen`s, and therefore has to
    /// export [`FACTORY_SYMBOL`](super::FACTORY_SYMBOL).
    pub(super) is_addon_library: bool,
    /// The size budget this file is measured against, if it has one.
    pub(super) budget: Option<Budget>,
}

/// Resolves every payload's built file and destination path.
///
/// The payload table is a parameter rather than a constant read here, so that the plan
/// a caller builds is the plan that gets applied.
///
/// # Errors
///
/// Returns an error when a payload that is not optional has no built file.
pub(super) fn plan_install(
    payloads: &[Payload],
    sources: &Sources,
    layout: &Layout,
) -> Result<Vec<PlannedFile>> {
    let mut plan = Vec::new();
    for payload in payloads {
        let source = match sources.artifact(payload.artifact) {
            Ok(source) => source,
            Err(error) if payload.optional => {
                println!("install: skipping {}: {error}", payload.name);
                continue;
            }
            Err(error) => return Err(error),
        };
        plan.push(PlannedFile {
            name: payload.name,
            source,
            destination: layout.destination(payload.destination, payload.name),
            is_addon_library: payload.destination == Destination::AddonLibrary,
            budget: payload.budget,
        });
    }
    Ok(plan)
}
