//! Tests for the round-trip verification.
//!
//! Every test works in a scratch tree with a scratch destination root and a scratch
//! home, injected through [`Sources`], [`Layout`] and [`UserData`]. Nothing here reads
//! or writes a real Fcitx5 installation, a real `$HOME` or a real build tree, and
//! nothing elevates.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::*;
use crate::install::layout::Destination;
use crate::install::manifest::{EntryState, MANIFEST_VERSION, backup_path};
use crate::install::takeover::RECORD_FILE;

/// A scratch directory unique to this test process and tag.
fn scratch(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("rspinyin-reversible-{tag}-{}", std::process::id()));
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

/// The user's directories inside a scratch root, so that no test touches `$HOME`.
fn user(root: &Path) -> UserData {
    let data_dir = root.join("home/.local/share/rspinyin");
    UserData {
        takeover: data_dir.join(RECORD_FILE),
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

/// Writes one build artifact into the scratch tree.
fn artifact(root: &Path, relative: &str, content: &str) {
    write(&root.join(relative), content);
}

/// Writes a minimal but real dictionary container into the scratch tree.
///
/// The install path reads the dictionary before copying it -- it is untrusted input and
/// the reader is the only thing that knows what a container looks like -- so a fixture
/// that wrote the word "dictionary" would be refused before the round trip started.
fn dictionary(root: &Path, relative: &str) {
    use ime_dict::format::SectionKind;
    use ime_dict::format::writer::DictWriter;

    let fst = fst::MapBuilder::new(Vec::new())
        .expect("an empty builder")
        .into_inner()
        .expect("an empty map encodes");
    let mut writer = DictWriter::new();
    writer
        .add_section(SectionKind::Fst, fst)
        .expect("the section is added");
    let image = writer.encode().expect("the container encodes");
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("an artifact has a parent"))
        .expect("creating the artifact directory");
    fs::write(&path, image).expect("writing the artifact");
}

/// Creates the directories a system that already runs Fcitx5 carries.
///
/// They are what the installer writes into rather than what it owns, so a real
/// installation finds them in place and an uninstall leaves them alone. Creating them
/// before the first capture is what makes the equality at the end a statement about
/// the plugin's files rather than about the directories Fcitx5 shares between every
/// addon. `Destination::Data` is deliberately absent: that directory is the plugin's
/// own, an install creates it, and an uninstall removes it again.
fn shared_directories(layout: &Layout) {
    for destination in [
        Destination::AddonLibrary,
        Destination::AddonDescriptor,
        Destination::InputMethodDescriptor,
        Destination::Icon { size: "48x48" },
        Destination::Icon { size: "scalable" },
    ] {
        fs::create_dir_all(layout.directory(destination)).expect("creating a shared directory");
    }
}

/// A scratch tree holding every artifact the payload table names, on a system whose
/// shared directories are already there.
fn fixture(tag: &str) -> (PathBuf, Sources, Layout, UserData) {
    let root = scratch(tag);
    artifact(&root, "target/release/librspinyin.so", "library bytes");
    artifact(&root, "target/release/librspinyin_ui.so", "library bytes");
    artifact(&root, "packaging/fcitx5/rspinyin.conf", "addon descriptor");
    artifact(
        &root,
        "packaging/fcitx5/rspinyin-ui.conf",
        "addon descriptor",
    );
    artifact(&root, "packaging/fcitx5/rspinyin-im.conf", "input method");
    artifact(
        &root,
        "packaging/metainfo/org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml",
        "metainfo",
    );
    artifact(&root, "assets/icon-48.png", "icon bytes");
    artifact(&root, "assets/icon.svg", "<svg/>");
    dictionary(&root, "data/compiled/base.dict");
    let sources = Sources::new(root.clone());
    let layout = layout(&root);
    shared_directories(&layout);
    let user = user(&root);
    (root, sources, layout, user)
}

/// The same tree on a system where the shared directories do not exist yet, which is
/// the state of a machine this plugin is the first addon on.
fn bare_fixture(tag: &str) -> (PathBuf, Sources, Layout, UserData) {
    let (root, sources, layout, user) = fixture(tag);
    fs::remove_dir_all(&layout.destdir).expect("removing the shared directories");
    fs::create_dir_all(&layout.destdir).expect("creating an empty destination root");
    (root, sources, layout, user)
}

/// The options a verification runs with: no elevation, and never a dry run.
fn options() -> Options {
    Options {
        dry_run: false,
        no_sudo: true,
    }
}

/// The files a difference list says were added, as paths under `root`.
///
/// A capture stores every path relative to the root it was taken from, while a plan
/// names absolute destinations; rebasing here is what lets the two be compared.
fn added_files(differences: &[Difference], root: &Path) -> Vec<PathBuf> {
    differences
        .iter()
        .filter_map(|difference| match difference {
            Difference::Appeared {
                path,
                kind: Kind::File,
            } => Some(root.join(path)),
            _ => None,
        })
        .collect()
}

#[test]
fn test_capture_and_difference_observe_every_kind_of_change() {
    // The whole check rests on this comparison being able to see a difference at all: a
    // `difference` that returned an empty list for a tree that had changed would make
    // every round trip below pass without proving anything.
    let root = scratch("difference");
    write(&root.join("kept"), "kept");
    write(&root.join("removed"), "removed");
    write(&root.join("changed"), "before");
    write(&root.join("remoded"), "unchanged bytes");
    write(&root.join("swapped"), "a file");
    fs::create_dir_all(root.join("gone-dir")).expect("creating the fixture directory");
    fs::set_permissions(root.join("remoded"), fs::Permissions::from_mode(0o600))
        .expect("setting the fixture mode");

    let before = capture(&root).expect("a scratch tree");
    fs::remove_file(root.join("removed")).expect("removing the fixture");
    write(&root.join("changed"), "after");
    write(&root.join("added"), "added");
    fs::set_permissions(root.join("remoded"), fs::Permissions::from_mode(0o644))
        .expect("setting the fixture mode");
    fs::remove_file(root.join("swapped")).expect("removing the fixture");
    fs::create_dir_all(root.join("swapped")).expect("replacing the file with a directory");
    fs::remove_dir_all(root.join("gone-dir")).expect("removing the fixture directory");
    let after = capture(&root).expect("the same tree, changed");

    let removed = Difference::Missing {
        path: PathBuf::from("removed"),
    };
    let gone = Difference::Missing {
        path: PathBuf::from("gone-dir"),
    };
    let added = Difference::Appeared {
        path: PathBuf::from("added"),
        kind: Kind::File,
    };
    let changed = Difference::Content {
        path: PathBuf::from("changed"),
    };
    let remoded = Difference::Mode {
        path: PathBuf::from("remoded"),
        before: 0o600,
        after: 0o644,
    };
    let swapped = Difference::KindChanged {
        path: PathBuf::from("swapped"),
        before: Kind::File,
        after: Kind::Directory,
    };

    let differences = before.difference(&after);
    assert_eq!(differences.len(), 6, "{differences:?}");
    assert!(differences.contains(&removed));
    assert!(differences.contains(&gone));
    assert!(differences.contains(&added));
    assert!(differences.contains(&changed));
    assert!(differences.contains(&remoded));
    assert!(differences.contains(&swapped));
    assert!(
        !differences
            .iter()
            .any(|difference| difference.path() == Path::new("kept")),
        "a path that did not change is not reported: {differences:?}"
    );
    assert!(
        before.difference(&before).is_empty(),
        "a tree compared with itself is unchanged"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_capture_refuses_a_root_that_is_not_a_directory() {
    let root = scratch("capture-refused");
    write(&root.join("a-file"), "content");

    // A tree that is not there would capture as empty, and an empty capture compares
    // equal to another empty one -- a round trip against it would pass vacuously.
    let missing = capture(&root.join("nowhere"));
    let failure = missing.expect_err("a missing tree is refused");
    assert!(failure.to_string().contains("not a directory"), "{failure}");
    assert!(
        capture(&root.join("a-file")).is_err(),
        "a file is not a tree"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_round_trip_restores_the_destination_tree_byte_for_byte() {
    let (root, sources, layout, user) = fixture("restore");
    // A distribution had already shipped a descriptor at the path this plugin installs
    // to, which is the case the manifest exists for.
    let displaced = layout.addon_conf_dir.join("rspinyin.conf");
    write(&displaced, "the distribution's descriptor");
    let stray = layout.addon_conf_dir.join("keyboard.conf");
    write(&stray, "another addon's descriptor");

    let report = round_trip(&sources, &layout, &user, &options()).expect("a staging tree");

    // The install really happened. Without this the equality below would hold just as
    // well for a round trip that copied nothing at all. It adds three groups of files:
    // the payloads it planned, the copy of whatever it displaced, and the manifest that
    // records both -- naming all three is what makes this an equality, not a lower bound.
    //
    // The displaced descriptor is the one planned path that is *not* an addition: it was
    // already there and the install overwrote it, so the snapshot reports it as a change
    // of content rather than as a file that appeared. What proves it landed is the
    // manifest saying `Replaced` below, and the restored bytes the uninstall check reads
    // back at the end.
    let mut landed = added_files(&report.landed(), &layout.destdir);
    let mut expected: Vec<PathBuf> = report
        .planned
        .iter()
        .filter(|path| **path != displaced)
        .cloned()
        .collect();
    expected.push(layout.manifest_path());
    expected.push(backup_path(&displaced));
    landed.sort();
    expected.sort();
    assert_eq!(landed, expected, "every planned file landed");
    assert!(
        report.planned.contains(&displaced),
        "the displaced descriptor is still a planned payload"
    );
    assert_eq!(
        report.planned.len(),
        9,
        "six payloads, two icons and the metainfo"
    );
    assert_eq!(report.manifest.version, MANIFEST_VERSION);
    assert_eq!(
        report.manifest.package_version,
        crate::install::PACKAGE_VERSION
    );
    let descriptor = report.manifest.entry(&displaced).expect("recorded");
    assert_eq!(descriptor.state, EntryState::Replaced);
    let library = layout.addon_dir.join("librspinyin.so");
    let entry = report.manifest.entry(&library).expect("recorded");
    assert_eq!(entry.state, EntryState::Created);

    // And the uninstall really undid it.
    let differences = report.differences();
    assert!(
        differences.is_empty(),
        "the tree came back: {differences:?}"
    );
    assert_eq!(read(&displaced), "the distribution's descriptor");
    assert_eq!(
        read(&stray),
        "another addon's descriptor",
        "a file the install never created is not removed"
    );
    assert!(
        !layout.manifest_path().exists(),
        "the manifest is consumed by the uninstall"
    );
    assert!(
        !layout.data_dir.exists(),
        "the directory the install owned is removed once it empties"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_round_trip_puts_the_fcitx5_user_interface_back() {
    let (root, sources, layout, user) = fixture("takeover");
    let config = user.config_home.join("fcitx5/config");
    let classic = "[Behavior]\nActiveUserInterface=classic\nShareInputState=No\n";
    write(&config, classic);
    // Loading the plugin pins Fcitx5's active user interface to it and records what it
    // displaced. The install does not do this -- it happens at runtime -- so the fixture
    // arranges the state the uninstall has to undo, and the round trip asserts that the
    // displaced value comes back.
    let config_path = config.display().to_string();
    let record = format!(r#"{{"config_file":"{config_path}","previous":"classic"}}"#);
    write(&user.takeover, &record);
    write(
        &config,
        "[Behavior]\nActiveUserInterface=rspinyin\nShareInputState=No\n",
    );

    round_trip(&sources, &layout, &user, &options()).expect("a staging tree");

    let restored = read(&config);
    assert!(
        restored.contains("ActiveUserInterface=classic"),
        "{restored}"
    );
    assert!(
        !restored.contains("rspinyin"),
        "the plugin's own name does not survive an uninstall: {restored}"
    );
    assert!(
        restored.contains("ShareInputState=No"),
        "every other line comes back untouched: {restored}"
    );
    assert!(
        user.takeover.exists(),
        "the plugin's own record is user data and is left in place"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_round_trip_leaves_only_the_directories_it_had_to_create() {
    let (root, sources, layout, user) = bare_fixture("bare");
    let report = round_trip(&sources, &layout, &user, &options()).expect("a staging tree");

    assert!(!report.landed().is_empty(), "the install happened");
    let differences = report.differences();
    assert!(
        !differences.is_empty(),
        "an install creates the directories Fcitx5's layout names"
    );
    // This is the residue a caller comparing two `find` listings has to allow for: the
    // directories are shared with every other addon, the manifest records files rather
    // than directories, and an uninstall that removed them would take the rest of the
    // user's addons with them.
    assert!(
        differences.iter().all(Difference::is_added_directory),
        "only directories are left behind: {differences:?}"
    );
    assert!(
        !layout.data_dir.exists(),
        "the directory the plugin owns is not among them"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_round_trip_restores_a_displaced_files_mode_as_well_as_its_content() {
    let (root, sources, layout, user) = fixture("mode");
    let displaced = layout.addon_dir.join("librspinyin.so");
    let shipped = "a library a distribution had put there";
    write(&displaced, shipped);
    fs::set_permissions(&displaced, fs::Permissions::from_mode(0o755))
        .expect("setting the fixture mode");

    let report = round_trip(&sources, &layout, &user, &options()).expect("a staging tree");

    let mode = fs::metadata(&displaced).expect("stat").permissions().mode();
    assert_eq!(
        read(&displaced),
        shipped,
        "the content of a displaced file comes back byte for byte"
    );
    // The permission bits are what the red line of this task is about: a file that comes
    // back with different bits is a system that was changed, not one that was restored.
    // The copy the install takes carries the displaced file's own mode, so this is the
    // assertion that the mode survived the round trip -- content equality alone would
    // hold just as well for a 0644 file put back where a 0755 one was.
    assert_eq!(
        mode & 0o777,
        0o755,
        "the mode of a displaced file comes back with it"
    );
    let differences = report.differences();
    assert!(
        differences.is_empty(),
        "the tree came back: {differences:?}"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_round_trip_refuses_a_destination_it_could_not_put_back() {
    // A symbolic link at a destination cannot survive the copy-and-put-back the install
    // relies on, so the round trip has to stop rather than convert the link into a
    // regular file. Stopping has to leave the link exactly where it was, and it has to
    // stop before the manifest is written: a record of an install that never happened is
    // worse than no record at all.
    let (root, sources, layout, user) = fixture("symlink");
    let linked = layout.addon_dir.join("librspinyin.so");
    let target = root.join("elsewhere/librspinyin.so");
    write(&target, "the file the link points at");
    std::os::unix::fs::symlink(&target, &linked).expect("creating the fixture link");

    let refused = round_trip(&sources, &layout, &user, &options());

    let failure = refused.expect_err("a link is not a destination this install can own");
    assert!(failure.to_string().contains("symbolic link"), "{failure}");
    assert_eq!(
        fs::read_link(&linked).expect("reading the link"),
        target,
        "the link is left exactly where it was"
    );
    assert!(
        !layout.manifest_path().exists(),
        "nothing was recorded, because nothing was installed"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_check_confined_accepts_a_staging_tree_without_elevation() {
    let (root, _sources, layout, _user) = fixture("confined");
    check_confined(&layout, Elevation::Direct).expect("a staging tree is allowed");
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_check_confined_refuses_a_layout_with_no_destdir() {
    let (root, _sources, mut layout, _user) = fixture("confined-destdir");
    layout.destdir = PathBuf::new();

    let confined = check_confined(&layout, Elevation::Direct);
    let failure = confined.expect_err("a real installation is not a staging tree");
    assert!(failure.to_string().contains("DESTDIR"), "{failure}");
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_check_confined_refuses_an_elevated_run() {
    // Asserted against the resolved elevation rather than against the flag, so the check
    // cannot be defeated by running as root while `--no-sudo` is absent.
    let (root, _sources, layout, _user) = fixture("confined-sudo");
    let confined = check_confined(&layout, Elevation::Sudo);
    let failure = confined.expect_err("a round trip never elevates");
    assert!(failure.to_string().contains("elevate"), "{failure}");
    fs::remove_dir_all(&root).expect("cleaning up");
}
