//! Round-trip verification: an install followed by an uninstall has to leave the
//! destination tree exactly as it was.
//!
//! Responsibility: capture a tree, drive the installer's own two directions over it,
//! and report every path that did not come back. It decides nothing about the
//! installation itself -- the payload table and the copy sequence belong to [`super`],
//! the record that makes an install reversible belongs to [`super::manifest`], and the
//! removal sequence belongs to [`super::uninstall`]. This module drives all three
//! rather than reimplementing any of them.
//!
//! Boundaries: a verification, never a system change. [`round_trip`] refuses a layout
//! with no `DESTDIR` and refuses to run anything through `sudo`, because the sequence
//! it drives removes files and the only tree it may remove them from is a staging
//! tree. It neither strips a library nor measures one against its budget: those decide
//! whether a build may be installed at all, they need a release build of a real shared
//! object, and a scratch tree has neither.
//!
//! # Why the check needs three captures
//!
//! "The tree came back" is a comparison between two points, and on its own it is
//! vacuous: an install that copied nothing and an uninstall that removed nothing leave
//! a tree equal to itself. The capture taken between the two directions is what makes
//! the equality mean something. [`RoundTrip::landed`] has to name the files the
//! installer's plan named, and only then does [`RoundTrip::differences`] being empty
//! say that the uninstall undid them.
//!
//! # Why the uninstall is driven from the manifest
//!
//! The installer records, per destination, whether it created the file or displaced
//! one and where the displaced copy went; the uninstall works from that record and not
//! from a second copy of the payload list. That is what lets it tell a file this plugin
//! owns from one a distribution package had already put there, and it is the property
//! a round trip has to exercise: a check that re-derived the file list would agree with
//! itself no matter what the real uninstall did with the manifest.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};

use super::layout::{Layout, Sources};
use super::manifest::{Elevation, Manifest};
use super::uninstall::{self, UserData};
use super::{Options, PAYLOADS, apply, plan_install};

/// What a captured path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink,
}

/// One path of a captured tree.
///
/// The mode is stored without its file-type bits: the kind is carried by the variant,
/// and a permission mask that also encoded it would report a difference every time a
/// file was compared with a directory, on top of the difference that says so.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Item {
    /// A regular file, by length, mode and content digest.
    File {
        /// Length in bytes.
        size: u64,
        /// Permission bits.
        mode: u32,
        /// SHA-256 of the contents.
        digest: Vec<u8>,
    },
    /// A directory and its permission bits.
    Directory {
        /// Permission bits.
        mode: u32,
    },
    /// A symbolic link and the path it points at.
    Symlink {
        /// Link target as stored, which may be relative.
        target: PathBuf,
    },
}

impl Item {
    /// What this path is.
    fn kind(&self) -> Kind {
        match self {
            Self::File { .. } => Kind::File,
            Self::Directory { .. } => Kind::Directory,
            Self::Symlink { .. } => Kind::Symlink,
        }
    }

    /// Appends every way this entry disagrees with `after`.
    ///
    /// A change of kind ends the comparison: a directory where a file was has no
    /// contents to compare, and reporting both would bury the one difference that
    /// explains the other.
    fn difference(&self, path: &Path, after: &Item, differences: &mut Vec<Difference>) {
        if self.kind() != after.kind() {
            differences.push(Difference::KindChanged {
                path: path.to_path_buf(),
                before: self.kind(),
                after: after.kind(),
            });
            return;
        }
        match (self, after) {
            (
                Self::File {
                    size: size_before,
                    mode: mode_before,
                    digest: digest_before,
                },
                Self::File {
                    size: size_after,
                    mode: mode_after,
                    digest: digest_after,
                },
            ) => {
                if digest_before != digest_after || size_before != size_after {
                    differences.push(Difference::Content {
                        path: path.to_path_buf(),
                    });
                }
                if mode_before != mode_after {
                    differences.push(Difference::Mode {
                        path: path.to_path_buf(),
                        before: *mode_before,
                        after: *mode_after,
                    });
                }
            }
            (Self::Directory { mode: mode_before }, Self::Directory { mode: mode_after })
                if mode_before != mode_after =>
            {
                differences.push(Difference::Mode {
                    path: path.to_path_buf(),
                    before: *mode_before,
                    after: *mode_after,
                })
            }
            (
                Self::Symlink {
                    target: target_before,
                },
                Self::Symlink {
                    target: target_after,
                },
            ) if target_before != target_after => differences.push(Difference::Content {
                path: path.to_path_buf(),
            }),
            // The kind is known to match here, so what is left is an entry that did
            // not change at all.
            _ => {}
        }
    }
}

/// Every path under one root, as it was when the snapshot was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Snapshot {
    /// One entry per path, keyed by its path relative to the captured root.
    ///
    /// Relative so that two captures compare independently of where the tree is, and
    /// ordered so that a difference list is reproducible from one run to the next.
    items: BTreeMap<PathBuf, Item>,
}

impl Snapshot {
    /// Every path the two captures disagree about, in path order.
    ///
    /// A path only one of them holds is a difference in its own right: a file the
    /// install left behind and a file the uninstall deleted although it had not
    /// created it are the two failures this check exists to catch.
    pub(crate) fn difference(&self, other: &Snapshot) -> Vec<Difference> {
        let paths: BTreeSet<&PathBuf> = self.items.keys().chain(other.items.keys()).collect();
        let mut differences = Vec::new();
        for path in paths {
            match (self.items.get(path), other.items.get(path)) {
                (None, Some(item)) => differences.push(Difference::Appeared {
                    path: path.clone(),
                    kind: item.kind(),
                }),
                (Some(_), None) => differences.push(Difference::Missing { path: path.clone() }),
                (Some(before), Some(after)) => before.difference(path, after, &mut differences),
                // The set is the union of both key sets, so a path neither capture
                // holds cannot be in it.
                (None, None) => {}
            }
        }
        differences
    }
}

/// One way two captures of the same tree disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Difference {
    /// The path is in the first capture and not in the second.
    Missing {
        /// The path that went away.
        path: PathBuf,
    },
    /// The path is in the second capture and not in the first.
    Appeared {
        /// The path that was added.
        path: PathBuf,
        /// What was added there.
        kind: Kind,
    },
    /// The path is in both, with different content; for a symbolic link, a different
    /// target.
    Content {
        /// The path whose content changed.
        path: PathBuf,
    },
    /// The path is in both, with different permission bits.
    Mode {
        /// The path whose mode changed.
        path: PathBuf,
        /// Permission bits before.
        before: u32,
        /// Permission bits after.
        after: u32,
    },
    /// The path is in both captures as a different kind of entry.
    KindChanged {
        /// The path whose kind changed.
        path: PathBuf,
        /// What was there before.
        before: Kind,
        /// What is there now.
        after: Kind,
    },
}

impl Difference {
    /// The path this difference is about.
    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::Missing { path }
            | Self::Appeared { path, .. }
            | Self::Content { path }
            | Self::Mode { path, .. }
            | Self::KindChanged { path, .. } => path,
        }
    }

    /// Whether this is a directory that was not there before.
    ///
    /// An install creates the directories Fcitx5's layout names, and an uninstall
    /// leaves them: `/usr/share/fcitx5/addon` is shared with every other addon, the
    /// manifest records files rather than directories, and a plugin that removed it
    /// would take the rest of the user's addons down with it. The distinction is what
    /// lets a caller tell that residue apart from a file that was not put back.
    pub(crate) fn is_added_directory(&self) -> bool {
        matches!(
            self,
            Self::Appeared {
                kind: Kind::Directory,
                ..
            }
        )
    }
}

/// One install and uninstall, with the captures that make it evidence.
#[derive(Debug)]
pub(crate) struct RoundTrip {
    /// The destination tree before the install.
    pub(crate) before: Snapshot,
    /// The destination tree after the install and before the uninstall.
    ///
    /// This capture is what keeps the check from passing vacuously: an install that
    /// copied nothing and an uninstall that removed nothing leave a tree equal to
    /// itself, and only a capture taken in between tells that apart from a round trip
    /// that really happened.
    pub(crate) installed: Snapshot,
    /// The destination tree after the uninstall, which has to equal [`Self::before`].
    pub(crate) after: Snapshot,
    /// Absolute destination path of every file the install planned, in copy order.
    ///
    /// Absolute because that is what the layout names; a capture stores the same files
    /// relative to the root it was taken from, so a caller comparing the two rebases
    /// one of them.
    pub(crate) planned: Vec<PathBuf>,
    /// What the install recorded, which is what the uninstall worked from.
    pub(crate) manifest: Manifest,
}

impl RoundTrip {
    /// What the install added to the tree.
    pub(crate) fn landed(&self) -> Vec<Difference> {
        self.before.difference(&self.installed)
    }

    /// What the whole round trip left behind.
    pub(crate) fn differences(&self) -> Vec<Difference> {
        self.before.difference(&self.after)
    }
}

/// Records every path under `root`, with the bytes and the mode each one has.
///
/// Paths are stored relative to `root`, so two captures compare independently of where
/// the tree was.
///
/// # Errors
///
/// Returns an error when `root` is not a directory -- a tree that is not there would
/// otherwise capture as empty and compare equal to another empty capture -- when a
/// directory cannot be listed, when a file cannot be read, and when an entry is neither
/// a regular file, a directory nor a symbolic link.
pub(crate) fn capture(root: &Path) -> Result<Snapshot> {
    ensure!(
        root.is_dir(),
        "{} is not a directory, so there is no tree to capture",
        root.display()
    );
    let mut items = BTreeMap::new();
    walk(root, Path::new(""), &mut items)?;
    Ok(Snapshot { items })
}

/// Captures one directory's entries and descends into its subdirectories.
///
/// `symlink_metadata` rather than `metadata`: following a link would record the tree
/// the link points into instead of the link, and an install that replaced a link with
/// a file would then compare equal.
fn walk(root: &Path, relative: &Path, items: &mut BTreeMap<PathBuf, Item>) -> Result<()> {
    let directory = root.join(relative);
    let entries =
        fs::read_dir(&directory).with_context(|| format!("listing {}", directory.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("listing {}", directory.display()))?;
        let path = entry.path();
        let child = relative.join(entry.file_name());
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("reading the metadata of {}", path.display()))?;
        let kind = metadata.file_type();
        let item = if kind.is_dir() {
            walk(root, &child, items)?;
            Item::Directory {
                mode: mode(&metadata),
            }
        } else if kind.is_file() {
            Item::File {
                size: metadata.len(),
                mode: mode(&metadata),
                digest: digest(&path)?,
            }
        } else if kind.is_symlink() {
            Item::Symlink {
                target: fs::read_link(&path)
                    .with_context(|| format!("reading the link {}", path.display()))?,
            }
        } else {
            anyhow::bail!(
                "{} is neither a file, a directory nor a symbolic link, so it cannot be \
                 compared",
                path.display()
            );
        };
        items.insert(child, item);
    }
    Ok(())
}

/// The permission bits of a metadata record, without the file-type bits.
fn mode(metadata: &fs::Metadata) -> u32 {
    metadata.permissions().mode() & 0o7777
}

/// SHA-256 of the file at `path`.
///
/// A digest rather than the bytes themselves: a capture of a real installation holds
/// whole libraries, and what a comparison needs is a fixed-size answer to "are these
/// two the same file", not a second copy of the tree.
fn digest(path: &Path) -> Result<Vec<u8>> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(Sha256::digest(&bytes).to_vec())
}

/// Drives an install and an uninstall over a staging tree and reports what changed.
///
/// The sequence is the installer's own: the payload table is resolved the way
/// `xtask install` resolves it, the manifest is written the way `xtask install` writes
/// it, and the removal works from that manifest the way `xtask uninstall` works from
/// it. `user` names the directories the uninstall reads the user's Fcitx5
/// configuration from, so a caller must point it at a scratch home rather than at the
/// real one.
///
/// # Errors
///
/// Returns an error when the layout names no `DESTDIR`, when the resolved elevation
/// would run a filesystem operation through `sudo`, when the destination tree cannot
/// be captured, when a payload is missing or its plan cannot be built, when a copy
/// fails, and when the uninstall cannot read the manifest or remove a file.
pub(crate) fn round_trip(
    sources: &Sources,
    layout: &Layout,
    user: &UserData,
    options: &Options,
) -> Result<RoundTrip> {
    let elevation = Elevation::detect(options.no_sudo)?;
    check_confined(layout, elevation)?;

    let before = capture(&layout.destdir)?;
    let planned = plan_install(PAYLOADS, sources, layout)?;
    let manifest = apply(&planned, layout, elevation)?;
    let installed = capture(&layout.destdir)?;
    uninstall::run(layout, user, options)?;
    let after = capture(&layout.destdir)?;

    Ok(RoundTrip {
        before,
        installed,
        after,
        planned: planned
            .iter()
            .map(|file| file.destination.clone())
            .collect(),
        manifest,
    })
}

/// Refuses a round trip that would not be confined to a staging tree.
///
/// Two conditions have to hold before a single file is touched, and neither is a
/// property of the tree being checked: the layout has to name a `DESTDIR`, and the
/// filesystem operations have to run in this process. Checking them here rather than
/// inline in [`round_trip`] is what lets both be asserted without depending on which
/// user runs the tests.
///
/// # Errors
///
/// Returns an error when `layout` has no `DESTDIR`, and when `elevation` would route
/// an operation through `sudo`.
fn check_confined(layout: &Layout, elevation: Elevation) -> Result<()> {
    ensure!(
        !layout.destdir.as_os_str().is_empty(),
        "a round trip installs and then uninstalls, so it may only ever run against a \
         DESTDIR staging tree; this layout has no DESTDIR, and running it would remove \
         a real installation"
    );
    ensure!(
        !elevation.is_sudo(),
        "a round trip must not elevate: it removes the files it installed, and the only \
         tree it may do that to is a staging tree it owns. Run it as the user that owns \
         the tree, or as root with --no-sudo"
    );
    Ok(())
}

#[cfg(test)]
mod tests;
