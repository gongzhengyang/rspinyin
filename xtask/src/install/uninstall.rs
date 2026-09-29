//! Taking the plugin back off the system.
//!
//! Responsibility: remove exactly the files the manifest lists, put back the copies
//! they displaced, return Fcitx5's active user interface to what it was, and leave the
//! user's own data alone.
//!
//! # What an uninstall will not do
//!
//! * It will not delete a file the install did not create. An entry that replaced
//!   something is restored from its backup, and an entry whose backup was never taken
//!   is left where it is -- deleting it would be the one irreversible thing an
//!   uninstall can do, and a half-finished install is exactly when it would happen.
//! * It will not remove a directory that is not empty. `/usr/share/fcitx5/addon` is
//!   where every other addon lives, and an install never owned it.
//! * It will not touch the user dictionary, the plugin configuration or the logs. The
//!   user dictionary is the product of everything the user has typed; deleting it on an
//!   uninstall would make "reinstall" mean "start over". Their location and the command
//!   that removes them are printed instead.

use std::path::PathBuf;

use anyhow::{Result, ensure};

use super::layout::Layout;
use super::manifest::{Elevation, Entry, EntryState, FILE_MODE, Manifest};
use super::takeover::{self, Outcome};
use super::{Options, manifest};

/// Removes the plugin, restores what it displaced, and puts the user interface back.
///
/// # Errors
///
/// Returns an error when the manifest or the takeover record cannot be read, when a
/// file cannot be removed or restored, and when the user's XDG base directories cannot
/// be resolved.
pub(super) fn uninstall(layout: &Layout, options: &Options) -> Result<()> {
    let user = UserData::resolve()?;
    run(layout, &user, options)
}

/// The body of [`uninstall`], with the user's directories named by the caller.
///
/// Split out so that the removal sequence can be exercised against a scratch tree,
/// without reading or writing the real `$HOME`.
///
/// # Errors
///
/// As [`uninstall`].
fn run(layout: &Layout, user: &UserData, options: &Options) -> Result<()> {
    let elevation = Elevation::detect(options.no_sudo)?;
    report_takeover(&takeover::restore(
        &user.takeover,
        &user.config_home,
        options.dry_run,
    )?);

    let Some(record) = Manifest::read(&layout.manifest_path())? else {
        println!(
            "uninstall: {} does not exist, so this installer has nothing to remove",
            layout.manifest_path().display()
        );
        print_user_data(user);
        return Ok(());
    };
    if options.dry_run {
        report_plan(&record, layout, elevation);
        print_user_data(user);
        return Ok(());
    }
    for entry in &record.entries {
        remove_entry(entry, elevation)?;
    }
    manifest::remove_file(elevation, &layout.manifest_path())?;
    manifest::remove_directory(elevation, &layout.data_dir)?;
    print_user_data(user);
    println!("uninstall: run `fcitx5 -r` to reload Fcitx5");
    Ok(())
}

/// Removes one installed file, or puts back the copy it displaced.
///
/// # Errors
///
/// Returns an error when the file cannot be removed, and when the backup cannot be
/// copied back over it.
fn remove_entry(entry: &Entry, elevation: Elevation) -> Result<()> {
    match (&entry.state, &entry.backup) {
        (EntryState::Replaced, Some(backup)) if backup.exists() => {
            manifest::place_file(elevation, backup, &entry.path, FILE_MODE)?;
            manifest::remove_file(elevation, backup)?;
            println!("uninstall: restored {}", entry.path.display());
            Ok(())
        }
        (EntryState::Replaced, _) => {
            println!(
                "uninstall: {} displaced a file that was never backed up; leaving it in place",
                entry.path.display()
            );
            Ok(())
        }
        (EntryState::Created, _) => {
            manifest::remove_file(elevation, &entry.path)?;
            println!("uninstall: removed {}", entry.path.display());
            Ok(())
        }
    }
}

/// Prints what an uninstall would remove, without removing any of it.
fn report_plan(manifest: &Manifest, layout: &Layout, elevation: Elevation) {
    println!("uninstall: dry run -- nothing will be removed");
    for entry in &manifest.entries {
        let action = match entry.state {
            EntryState::Created => "remove",
            EntryState::Replaced => "restore",
        };
        println!("uninstall:   {action} {}", entry.path.display());
    }
    println!("uninstall: remove {}", layout.manifest_path().display());
    println!(
        "uninstall: privileged operations: {}",
        if elevation.is_sudo() { "sudo" } else { "none" }
    );
}

/// Prints what happened to the Fcitx5 user interface the plugin took over.
fn report_takeover(outcome: &Outcome) {
    let (verb, file, value) = match outcome {
        Outcome::NoRecord => {
            println!("uninstall: the plugin never recorded a user interface takeover");
            return;
        }
        Outcome::Planned { file, value } => ("would put back", file, value),
        Outcome::Restored { file, value } => ("put back", file, value),
    };
    let value = match value {
        Some(value) => format!("`{value}`"),
        None => "the key did not exist before, so it is removed".to_owned(),
    };
    println!(
        "uninstall: {verb} the previous Fcitx5 user interface in {} ({value})",
        file.display()
    );
}

/// Prints where the user's own data is and how to delete it.
fn print_user_data(user: &UserData) {
    println!("uninstall: your own data was left in place:");
    println!("uninstall:   {}", user.config_dir.display());
    println!("uninstall:   {}", user.data_dir.display());
    println!(
        "uninstall: delete it with `rm -rf {} {}`",
        user.data_dir.display(),
        user.config_dir.display()
    );
}

/// The user's own copy of the plugin's state, which an uninstall never touches.
struct UserData {
    /// `$XDG_DATA_HOME/rspinyin`.
    data_dir: PathBuf,
    /// `$XDG_CONFIG_HOME/rspinyin`.
    config_dir: PathBuf,
    /// `$XDG_DATA_HOME/rspinyin/ui_takeover.json`.
    takeover: PathBuf,
    /// `$XDG_CONFIG_HOME`, where Fcitx5 keeps its own configuration.
    config_home: PathBuf,
}

impl UserData {
    /// Resolves the layout from the XDG base directories.
    ///
    /// Reuses `ime-dict`'s layout rather than re-deriving it, so that the uninstall
    /// points at the same files the plugin writes rather than at a second opinion about
    /// where they go.
    ///
    /// # Errors
    ///
    /// Returns an error when neither the `XDG_*` variables nor `HOME` name an absolute
    /// directory, and when the layout does not name the takeover record this installer
    /// knows how to read.
    fn resolve() -> Result<Self> {
        let bases = ime_dict::paths::BaseDirs::from_env()?;
        let paths = ime_dict::paths::Paths::from_bases(&bases)?;
        // The plugin writes the record through the same layout, so the two can only
        // disagree if one of them was changed on its own -- and a disagreement would
        // make the uninstall silently stop restoring the user interface.
        ensure!(
            paths.takeover.ends_with(takeover::RECORD_FILE),
            "the plugin's data layout names its takeover record {}, but this installer reads \
             {}",
            paths.takeover.display(),
            takeover::RECORD_FILE
        );
        Ok(Self {
            data_dir: paths.data_dir,
            config_dir: paths.config_dir,
            takeover: paths.takeover,
            config_home: bases.config_home,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;

    /// A scratch directory unique to this test process and tag.
    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rspinyin-uninstall-{tag}-{}", std::process::id()));
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
            icon_dir: destdir.join("usr/share/icons/hicolor"),
            prefix: PathBuf::from("/usr"),
            destdir,
        }
    }

    /// The user's directories inside a scratch root, so that no test touches `$HOME`.
    fn user(root: &Path) -> UserData {
        let data_dir = root.join("home/.local/share/rspinyin");
        UserData {
            takeover: data_dir.join(takeover::RECORD_FILE),
            data_dir,
            config_dir: root.join("home/.config/rspinyin"),
            config_home: root.join("home/.config"),
        }
    }

    /// Writes a file, creating the directories above it.
    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().expect("the fixture has a parent"))
            .expect("creating the fixture directory");
        fs::write(path, content).expect("writing the fixture");
    }

    /// Reads a file back as text.
    fn read(path: &Path) -> String {
        fs::read_to_string(path).expect("reading back")
    }

    /// Records `entries` as the manifest of `layout`.
    fn record(layout: &Layout, entries: Vec<Entry>) {
        Manifest {
            version: super::manifest::MANIFEST_VERSION,
            package_version: "0.1.0".to_owned(),
            entries,
        }
        .write(&layout.manifest_path())
        .expect("writing the manifest");
    }

    /// The options a test runs with: no elevation, and never a dry run unless asked.
    fn options(dry_run: bool) -> Options {
        Options {
            dry_run,
            no_sudo: true,
        }
    }

    #[test]
    fn test_run_removes_created_files_and_restores_replaced_ones() {
        let root = scratch("run");
        let layout = layout(&root);
        let user = user(&root);
        let created = layout.addon_dir.join("librspinyin.so");
        let replaced = layout.addon_conf_dir.join("rspinyin.conf");
        let backup = manifest::backup_path(&replaced);
        write(&created, "ours");
        write(&replaced, "ours too");
        write(&backup, "the distribution's");
        record(
            &layout,
            vec![
                Entry {
                    path: created.clone(),
                    state: EntryState::Created,
                    backup: None,
                },
                Entry {
                    path: replaced.clone(),
                    state: EntryState::Replaced,
                    backup: Some(backup.clone()),
                },
            ],
        );

        run(&layout, &user, &options(false)).expect("uninstalling");

        assert!(!created.exists(), "a file the install created is removed");
        assert_eq!(read(&replaced), "the distribution's");
        assert!(!backup.exists(), "the backup is consumed by the restore");
        assert!(!layout.manifest_path().exists(), "the manifest goes last");
        assert!(
            !layout.data_dir.exists(),
            "the directory the install owned is removed once it empties"
        );
        fs::remove_dir_all(&root).expect("cleaning up");
    }

    #[test]
    fn test_run_leaves_every_file_the_manifest_never_listed() {
        let root = scratch("stray");
        let layout = layout(&root);
        let user = user(&root);
        let ours = layout.addon_dir.join("librspinyin.so");
        let stray = layout.addon_conf_dir.join("keyboard.conf");
        write(&ours, "ours");
        write(&stray, "another addon");
        record(
            &layout,
            vec![Entry {
                path: ours.clone(),
                state: EntryState::Created,
                backup: None,
            }],
        );

        run(&layout, &user, &options(false)).expect("uninstalling");

        assert!(!ours.exists());
        assert_eq!(read(&stray), "another addon");
        assert!(
            layout.addon_conf_dir.exists(),
            "a directory that is still in use survives"
        );
        fs::remove_dir_all(&root).expect("cleaning up");
    }

    #[test]
    fn test_run_without_a_manifest_removes_nothing() {
        let root = scratch("no-manifest");
        let layout = layout(&root);
        let user = user(&root);
        let file = layout.addon_dir.join("librspinyin.so");
        write(&file, "someone else's plugin");

        run(&layout, &user, &options(false)).expect("nothing to remove is not an error");

        assert_eq!(read(&file), "someone else's plugin");
        fs::remove_dir_all(&root).expect("cleaning up");
    }

    #[test]
    fn test_run_dry_run_changes_nothing() {
        let root = scratch("dry-run");
        let layout = layout(&root);
        let user = user(&root);
        let created = layout.addon_dir.join("librspinyin.so");
        let replaced = layout.addon_conf_dir.join("rspinyin.conf");
        let backup = manifest::backup_path(&replaced);
        write(&created, "ours");
        write(&replaced, "ours too");
        write(&backup, "the distribution's");
        record(
            &layout,
            vec![
                Entry {
                    path: created.clone(),
                    state: EntryState::Created,
                    backup: None,
                },
                Entry {
                    path: replaced.clone(),
                    state: EntryState::Replaced,
                    backup: Some(backup.clone()),
                },
            ],
        );

        run(&layout, &user, &options(true)).expect("a dry run reports and stops");

        assert!(created.exists());
        assert_eq!(read(&replaced), "ours too");
        assert!(backup.exists());
        assert!(layout.manifest_path().exists());
        fs::remove_dir_all(&root).expect("cleaning up");
    }

    #[test]
    fn test_run_puts_the_fcitx5_user_interface_back() {
        let root = scratch("takeover");
        let layout = layout(&root);
        let user = user(&root);
        let config = user.config_home.join("fcitx5/config");
        write(
            &config,
            "[Behavior]\nActiveUserInterface=rspinyin\nShareInputState=No\n",
        );
        write(
            &user.takeover,
            &format!(
                r#"{{"config_file":"{}","previous":"classic"}}"#,
                config.display()
            ),
        );

        run(&layout, &user, &options(false)).expect("uninstalling");

        let restored = read(&config);
        assert!(
            restored.contains("ActiveUserInterface=classic"),
            "{restored}"
        );
        assert!(restored.contains("ShareInputState=No"), "{restored}");
        fs::remove_dir_all(&root).expect("cleaning up");
    }

    #[test]
    fn test_remove_entry_removes_a_created_file_and_tolerates_its_absence() {
        let root = scratch("created");
        let file = root.join("ours");
        write(&file, "ours");
        let entry = Entry {
            path: file.clone(),
            state: EntryState::Created,
            backup: None,
        };

        remove_entry(&entry, Elevation::Direct).expect("a file that is there");
        assert!(!file.exists());
        remove_entry(&entry, Elevation::Direct).expect("a file that is already gone");
        fs::remove_dir_all(&root).expect("cleaning up");
    }

    #[test]
    fn test_remove_entry_leaves_a_replaced_file_it_cannot_restore() {
        // The install did not get as far as taking the copy, so the file at the
        // destination belongs to whoever put it there. Deleting it would be the one
        // irreversible thing an uninstall can do.
        let root = scratch("unbacked");
        let file = root.join("someone-elses-file");
        write(&file, "the distribution's");
        let entry = Entry {
            path: file.clone(),
            state: EntryState::Replaced,
            backup: Some(manifest::backup_path(&file)),
        };

        remove_entry(&entry, Elevation::Direct).expect("leaving the file alone is not an error");

        assert_eq!(read(&file), "the distribution's");

        let without_a_backup = Entry {
            path: file.clone(),
            state: EntryState::Replaced,
            backup: None,
        };
        remove_entry(&without_a_backup, Elevation::Direct).expect("as above");
        assert_eq!(read(&file), "the distribution's");
        fs::remove_dir_all(&root).expect("cleaning up");
    }

    #[test]
    fn test_report_takeover_and_report_plan_describe_every_outcome() {
        // The reports are the user's only account of what happened, so each shape of
        // outcome has to render.
        let root = scratch("report");
        let layout = layout(&root);
        report_takeover(&Outcome::NoRecord);
        report_takeover(&Outcome::Planned {
            file: PathBuf::from("/etc/fcitx5/config"),
            value: Some("classic".to_owned()),
        });
        report_takeover(&Outcome::Restored {
            file: PathBuf::from("/etc/fcitx5/config"),
            value: None,
        });

        let manifest = Manifest {
            version: super::manifest::MANIFEST_VERSION,
            package_version: "0.1.0".to_owned(),
            entries: vec![Entry {
                path: layout.addon_dir.join("librspinyin.so"),
                state: EntryState::Replaced,
                backup: Some(PathBuf::from("/tmp/librspinyin.so.rspinyin-bak")),
            }],
        };
        report_plan(&manifest, &layout, Elevation::Sudo);
        print_user_data(&user(&root));
        fs::remove_dir_all(&root).expect("cleaning up");
    }
}
