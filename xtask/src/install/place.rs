//! Carrying a plan out: classify every destination, record what was displaced, copy.
//!
//! Responsibility: the half of an install that changes the target system, and the record
//! that makes it reversible. It refuses a destination it could not put back, it writes the
//! manifest before the first file on the system changes, and it copies aside whatever a
//! destination already held rather than overwriting it, so an uninstall restores instead
//! of deletes.
//!
//! The three steps are one sequence rather than three entry points, and they are not part
//! of [`super::install`] because a caller has to be able to drive them over a scratch tree
//! without a build and without `sudo`: that is what the round-trip verification does, and
//! it can only compare a tree before and after if it can install into one.
//!
//! # Why the order is classify, record, copy
//!
//! Every destination is classified -- and every refusal made -- before anything on the
//! system changes, because an install that stops part way must not leave the system in a
//! state no uninstall can describe. The record then goes down before the first copy, and
//! before the first displaced file is copied aside, so a run interrupted anywhere after it
//! is still fully reversible: the manifest says what the install owns, and an entry whose
//! copy never happened is one the uninstall recognises and leaves alone.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};

use super::PACKAGE_VERSION;
use super::layout::Layout;
use super::manifest::{self, Elevation, Entry, EntryState, FILE_MODE, MANIFEST_VERSION, Manifest};
use super::payload::PlannedFile;

/// What an install will do to one destination.
///
/// Decided for the whole plan before anything is touched, so that a destination this
/// install could not put back is refused while the system is still exactly as it was
/// found.
#[derive(Debug)]
enum Action {
    /// Nothing is there: the install creates the file and an uninstall removes it.
    Create,
    /// A regular file is there; it is copied to `backup` before it is replaced.
    Displace {
        /// Path the displaced copy is kept at.
        backup: PathBuf,
    },
    /// An earlier run of this installer already owns the file. Its record, and the copy
    /// that record points at, are left exactly as they are.
    Recorded(Entry),
}

/// What is at a destination before an install writes to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Occupant {
    /// Nothing is there.
    Absent,
    /// A regular file, which an install can copy aside and put back.
    File,
    /// A symbolic link.
    Symlink,
    /// A directory.
    Directory,
    /// A device node, a socket or a FIFO: something that is neither of the above.
    Other,
}

/// Records a plan and copies it into place: the reversible half of an install.
///
/// Split out from [`super::install`] so that the sequence that makes an install
/// reversible -- classify, record, copy -- can be exercised against a scratch tree, with
/// no build, no target system and no `sudo`.
///
/// # Errors
///
/// Returns an error when a destination is one this install could not put back, when the
/// manifest cannot be written, when a displaced file cannot be copied aside, and when a
/// copy fails. The manifest is on disk before the first file on the system changes, so an
/// uninstall can still undo a run that failed part way.
pub(super) fn apply(
    plan: &[PlannedFile],
    layout: &Layout,
    elevation: Elevation,
) -> Result<Manifest> {
    let actions = classify(plan, layout)?;
    let manifest = Manifest {
        version: MANIFEST_VERSION,
        package_version: PACKAGE_VERSION.to_owned(),
        entries: entries_of(plan, &actions),
    };
    // The manifest goes down before the first file on the system changes, so an install
    // interrupted by a crash or a Ctrl-C is still fully reversible: an uninstall can see
    // which files are ours, and which of them displaced something that has to come back.
    manifest.write(elevation, &layout.manifest_path())?;
    take_backups(plan, &actions, elevation)?;
    copy_into_place(plan, elevation)?;
    Ok(manifest)
}

/// Decides what each destination is, refusing every one this install could not put back.
///
/// Reads the system and writes nothing: the whole plan is classified before the first
/// backup is taken, so a refusal leaves no half-taken copies behind.
///
/// # Errors
///
/// Returns an error when a manifest left by an earlier install cannot be read, and as
/// [`action_for`].
fn classify(plan: &[PlannedFile], layout: &Layout) -> Result<Vec<Action>> {
    let previous = Manifest::read(&layout.manifest_path())?;
    let mut actions = Vec::with_capacity(plan.len());
    for file in plan {
        // A destination this installer already owns keeps the entry it had: the backup
        // taken the first time holds what was there before, and overwriting it with the
        // file we installed would lose the only copy of it.
        let action = match previous
            .as_ref()
            .and_then(|manifest| manifest.entry(&file.destination))
        {
            Some(entry) => Action::Recorded(entry.clone()),
            None => action_for(file)?,
        };
        actions.push(action);
    }
    Ok(actions)
}

/// Decides what an install does to one destination it does not already own.
///
/// # Errors
///
/// Returns an error when the destination holds something this install could not put back
/// -- a symbolic link, a directory, a device -- and when it holds a copy of a file
/// displaced by a run whose manifest is gone, which is the only copy of that file.
fn action_for(file: &PlannedFile) -> Result<Action> {
    let destination = &file.destination;
    match occupant(destination) {
        Occupant::Absent => Ok(Action::Create),
        Occupant::File => {
            let backup = manifest::backup_path(destination);
            ensure!(
                !backup.exists(),
                "install: {} is already there, and the manifest that would say what it holds \
                 is gone. It is the copy an earlier install took of the file it displaced, \
                 and overwriting it would lose the only copy of that file. Move it aside by \
                 hand and re-run.",
                backup.display()
            );
            Ok(Action::Displace { backup })
        }
        Occupant::Symlink => anyhow::bail!(
            "install: {} is a symbolic link. An install copies a destination aside and puts \
             it back on the way out, and a link does not survive that round trip: the copy \
             holds the file the link points at, so an uninstall would leave a regular file \
             where the link was. Remove the link if this installer should own that path.",
            destination.display()
        ),
        Occupant::Directory => anyhow::bail!(
            "install: {} is a directory, and a file cannot be installed over it. Remove it, \
             or install somewhere else with `--prefix`.",
            destination.display()
        ),
        Occupant::Other => anyhow::bail!(
            "install: {} is neither a regular file nor a symbolic link, so there is no copy \
             of it this installer could put back. Remove it, or install somewhere else with \
             `--prefix`.",
            destination.display()
        ),
    }
}

/// What is at `path` right now.
///
/// `symlink_metadata` rather than `exists`: a link is an entry in its own right, and the
/// difference decides whether an install can be undone. A dangling link in particular is
/// invisible to `exists` -- it follows the link, finds nothing and reports the path as
/// free -- so an install that asked `exists` would record the link as a file it created
/// and delete it on the way out.
///
/// A path whose metadata cannot be read at all is reported as [`Occupant::Absent`]: the
/// copy that follows fails with the real reason, which is a better diagnostic than a
/// classification made from a guess.
fn occupant(path: &Path) -> Occupant {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return Occupant::Absent;
    };
    let kind = metadata.file_type();
    if kind.is_file() {
        Occupant::File
    } else if kind.is_symlink() {
        Occupant::Symlink
    } else if kind.is_dir() {
        Occupant::Directory
    } else {
        Occupant::Other
    }
}

/// The manifest entries a classified plan produces.
///
/// Pure, and that is the point: it names the copies [`take_backups`] is about to take,
/// which is what lets the record go down before the first of them does.
fn entries_of(plan: &[PlannedFile], actions: &[Action]) -> Vec<Entry> {
    plan.iter()
        .zip(actions)
        .map(|(file, action)| match action {
            Action::Create => Entry {
                path: file.destination.clone(),
                state: EntryState::Created,
                backup: None,
            },
            Action::Displace { backup } => Entry {
                path: file.destination.clone(),
                state: EntryState::Replaced,
                backup: Some(backup.clone()),
            },
            Action::Recorded(entry) => entry.clone(),
        })
        .collect()
}

/// Copies aside every file the plan displaces.
///
/// # Errors
///
/// Returns an error when a displaced file cannot be copied to its backup path. The
/// manifest is already on disk at that point, and an entry whose copy never happened is
/// one the uninstall recognises: it leaves the file where it is rather than deleting it.
fn take_backups(plan: &[PlannedFile], actions: &[Action], elevation: Elevation) -> Result<()> {
    for (file, action) in plan.iter().zip(actions) {
        let Action::Displace { backup } = action else {
            continue;
        };
        // The copy carries the displaced file's own mode, not the installer's: that is
        // what lets an uninstall put back the permissions as well as the bytes. A
        // distribution ships `librspinyin.so` at 0755, and an uninstall that left it at
        // 0644 would have changed the system it claimed to restore.
        let displaced = manifest::mode_of(&file.destination);
        manifest::place_file(elevation, &file.destination, backup, displaced)?;
        println!(
            "install: {} was already there; the copy it displaced is kept at {}",
            file.destination.display(),
            backup.display()
        );
    }
    Ok(())
}

/// Copies every planned file to its destination.
///
/// # Errors
///
/// Returns an error when a copy fails. The manifest is already on disk at that point,
/// so an uninstall can still undo what was done.
fn copy_into_place(plan: &[PlannedFile], elevation: Elevation) -> Result<()> {
    for file in plan {
        manifest::place_file(elevation, &file.source, &file.destination, FILE_MODE)?;
        println!(
            "install: {} -> {}",
            file.source.display(),
            file.destination.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A scratch directory unique to this test process and tag.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rspinyin-place-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        dir
    }

    /// The destination layout of a scratch root, shaped like a real one.
    fn layout(root: &Path) -> Layout {
        let destdir = root.join("dest");
        Layout {
            addon_dir: destdir.join("usr/lib/fcitx5"),
            addon_conf_dir: destdir.join("usr/share/fcitx5/addon"),
            input_method_dir: destdir.join("usr/share/fcitx5/inputmethod"),
            data_dir: destdir.join("usr/share/rspinyin"),
            metainfo_dir: destdir.join("usr/share/metainfo"),
            icon_dir: destdir.join("usr/share/icons/hicolor"),
            prefix: PathBuf::from("/usr"),
            destdir,
        }
    }

    /// One planned file, with its built artifact written into the scratch tree.
    fn planned(root: &Path, layout: &Layout, name: &'static str, content: &str) -> PlannedFile {
        let source = root.join("build").join(name);
        fs::create_dir_all(source.parent().expect("an artifact has a parent"))
            .expect("creating the build directory");
        fs::write(&source, content).expect("writing the artifact");
        PlannedFile {
            name,
            source,
            destination: layout.addon_dir.join(name),
            is_addon_library: true,
            budget: None,
        }
    }

    /// The backups a classified plan will take, in plan order.
    fn backups(actions: &[Action]) -> Vec<PathBuf> {
        actions
            .iter()
            .filter_map(|action| match action {
                Action::Displace { backup } => Some(backup.clone()),
                Action::Create | Action::Recorded(_) => None,
            })
            .collect()
    }

    /// Writes a file, creating the directories above it.
    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().expect("the fixture has a parent"))
            .expect("creating the fixture directory");
        fs::write(path, content).expect("writing the fixture");
    }

    #[test]
    fn test_classify_creates_a_file_that_is_absent_and_displaces_one_that_is_there() {
        let root = scratch("classify");
        let layout = layout(&root);
        let absent = planned(&root, &layout, "librspinyin.so", "library bytes");
        let present = planned(&root, &layout, "base.dict", "dictionary bytes");
        write(&present.destination, "what a distribution shipped");
        let plan = vec![absent, present];

        let actions = classify(&plan, &layout).expect("every destination is usable");

        assert_eq!(
            backups(&actions),
            vec![manifest::backup_path(&plan[1].destination)],
            "only the destination that is already there needs a copy taken"
        );
        let entries = entries_of(&plan, &actions);
        assert_eq!(entries[0].path, plan[0].destination);
        assert_eq!(entries[0].state, EntryState::Created);
        assert_eq!(entries[0].backup, None);
        assert_eq!(entries[1].path, plan[1].destination);
        assert_eq!(entries[1].state, EntryState::Replaced);
        assert_eq!(
            entries[1].backup,
            Some(manifest::backup_path(&plan[1].destination))
        );
        fs::remove_dir_all(&root).expect("cleaning up");
    }

    #[test]
    fn test_take_backups_keeps_what_it_displaced_with_its_own_mode() {
        // The copy is what an uninstall restores from, so it has to hold the file as it
        // was: its content, and the permission bits a distribution shipped it with.
        let root = scratch("backups");
        let layout = layout(&root);
        let file = planned(&root, &layout, "librspinyin.so", "library bytes");
        write(&file.destination, "a library a distribution put there");
        fs::set_permissions(&file.destination, fs::Permissions::from_mode(0o755))
            .expect("marking the fixture executable");
        let plan = vec![file];
        let actions = classify(&plan, &layout).expect("a writable tree");

        take_backups(&plan, &actions, Elevation::Direct).expect("taking the copy");

        let backup = manifest::backup_path(&plan[0].destination);
        let displaced = fs::read_to_string(&backup).expect("reading the copy");
        let original = fs::read_to_string(&plan[0].destination).expect("reading the original");
        let mode = fs::metadata(&backup).expect("stat").permissions().mode();
        assert_eq!(displaced, "a library a distribution put there");
        assert_eq!(
            mode & 0o777,
            0o755,
            "the copy carries the mode of the file it displaced"
        );
        assert_eq!(
            original, "a library a distribution put there",
            "taking the copy does not move the original"
        );
        fs::remove_dir_all(&root).expect("cleaning up");
    }

    #[test]
    fn test_classify_refuses_a_symbolic_link_destination() {
        // A link is an entry in its own right, and the copy an install would take of it
        // holds the file the link points at: an uninstall would then leave a regular file
        // where the link was. The dangling case is the one `exists` gets wrong -- it
        // reports the path as free and the install would delete the link on the way out
        // -- so both are asserted.
        for target_exists in [false, true] {
            let root = scratch("classify-symlink");
            let layout = layout(&root);
            let file = planned(&root, &layout, "librspinyin.so", "library bytes");
            let target = root.join("elsewhere/librspinyin.so");
            if target_exists {
                write(&target, "the file the link points at");
            }
            fs::create_dir_all(file.destination.parent().expect("a parent")).expect("creating");
            std::os::unix::fs::symlink(&target, &file.destination).expect("creating the link");
            let plan = vec![file];

            let failure = classify(&plan, &layout).expect_err("a link is not replaceable");

            assert!(failure.to_string().contains("symbolic link"), "{failure}");
            assert_eq!(
                fs::read_link(&plan[0].destination).expect("reading the link"),
                target,
                "the link is left exactly where it was"
            );
            assert!(
                !manifest::backup_path(&plan[0].destination).exists(),
                "and nothing was copied aside for it"
            );
            fs::remove_dir_all(&root).expect("cleaning up");
        }
    }

    #[test]
    fn test_classify_refuses_a_directory_destination() {
        let root = scratch("classify-directory");
        let layout = layout(&root);
        let file = planned(&root, &layout, "librspinyin.so", "library bytes");
        fs::create_dir_all(&file.destination).expect("putting a directory in the way");
        let plan = vec![file];

        let failure =
            classify(&plan, &layout).expect_err("a file cannot be installed over a directory");

        assert!(failure.to_string().contains("directory"), "{failure}");
        assert!(
            plan[0].destination.is_dir(),
            "the directory is left exactly where it was"
        );
        fs::remove_dir_all(&root).expect("cleaning up");
    }

    #[test]
    fn test_classify_refuses_a_backup_it_cannot_account_for_before_taking_any() {
        // A `.rspinyin-bak` no manifest mentions is what an earlier install left behind
        // when the record of it was lost, and it holds the only copy of the file that
        // install displaced. Overwriting it would destroy the one file an uninstall
        // exists to put back, so the install stops instead -- and stops before it takes
        // a copy of anything else, which is what leaves the system exactly as it was.
        let root = scratch("classify-stale");
        let layout = layout(&root);
        let first = planned(&root, &layout, "librspinyin.so", "library bytes");
        let second = planned(&root, &layout, "base.dict", "dictionary bytes");
        write(&first.destination, "what a distribution shipped");
        write(&second.destination, "what a distribution shipped");
        let stale = manifest::backup_path(&second.destination);
        fs::write(&stale, "the file an earlier run displaced").expect("writing the fixture");
        let plan = vec![first, second];

        let failure = classify(&plan, &layout).expect_err("a backup it did not take is refused");

        assert!(failure.to_string().contains("rspinyin-bak"), "{failure}");
        assert_eq!(
            fs::read_to_string(&stale).expect("reading the copy"),
            "the file an earlier run displaced",
            "the only copy of what was displaced survives the refusal"
        );
        assert!(
            !manifest::backup_path(&plan[0].destination).exists(),
            "and the destinations ahead of it were not touched either"
        );
        fs::remove_dir_all(&root).expect("cleaning up");
    }
}
