//! Tests for the install direction.
//!
//! Every test works in a scratch tree with a scratch destination root, injected through
//! [`Sources`] and [`Layout`]. Nothing here reads or writes a real Fcitx5 installation,
//! a real `$HOME` or a real build tree, and nothing elevates.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::*;

/// A scratch directory unique to this test process and tag.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rspinyin-install-{tag}-{}", std::process::id()));
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

/// Writes one build artifact into the scratch tree.
fn artifact(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("an artifact has a parent"))
        .expect("creating the artifact directory");
    fs::write(&path, content).expect("writing the artifact");
}

/// A scratch tree holding every artifact the payload table names, minus the icons,
/// which are optional by design and absent from the repository.
///
/// One line per mandatory payload, so adding a payload to the table without teaching the
/// fixture about it fails here rather than passing a plan that names a file no build
/// produces.
fn fixture(tag: &str) -> (PathBuf, Sources, Layout) {
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
    artifact(&root, "data/compiled/base.dict", "dictionary");
    let sources = Sources::new(root.clone());
    let layout = layout(&root);
    (root, sources, layout)
}

/// The plan the fixture produces.
fn plan(sources: &Sources, layout: &Layout) -> Vec<PlannedFile> {
    plan_install(PAYLOADS, sources, layout).expect("every mandatory artifact exists")
}

/// Reads a file back as text.
fn read(path: &Path) -> String {
    fs::read_to_string(path).expect("reading back")
}

#[test]
fn test_plan_install_resolves_every_payload_and_skips_a_missing_optional_one() {
    let (root, sources, layout) = fixture("plan");
    let plan = plan(&sources, &layout);

    let destinations: Vec<PathBuf> = plan.iter().map(|file| file.destination.clone()).collect();
    assert_eq!(
        destinations,
        vec![
            layout.addon_dir.join("librspinyin.so"),
            layout.addon_dir.join("librspinyin_ui.so"),
            layout.addon_conf_dir.join("rspinyin.conf"),
            layout.addon_conf_dir.join("rspinyin-ui.conf"),
            layout.input_method_dir.join("rspinyin.conf"),
            layout.data_dir.join("base.dict"),
        ],
        "the icons are optional and absent, so they are not planned"
    );
    assert_eq!(
        plan.iter().filter(|file| file.is_addon_library).count(),
        2,
        "both cdylibs go through the symbol check, one per addon"
    );
    assert_eq!(
        plan.iter()
            .filter(|file| file.budget == Some(Budget::BaseDictionary))
            .count(),
        1
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_plan_install_reports_a_missing_mandatory_artifact() {
    let (root, sources, layout) = fixture("plan-missing");
    fs::remove_file(root.join("target/release/librspinyin.so")).expect("removing the fixture");
    let failure = plan_install(PAYLOADS, &sources, &layout).expect_err("the library is mandatory");
    assert!(
        failure.to_string().contains("librspinyin.so"),
        "the message names what is missing: {failure}"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_apply_installs_every_file_with_the_documented_mode() {
    let (root, sources, layout) = fixture("apply");
    let manifest = apply(&plan(&sources, &layout), &layout, Elevation::Direct)
        .expect("installing into a writable tree");

    assert_eq!(manifest.entries.len(), 6);
    assert_eq!(manifest.version, MANIFEST_VERSION);
    assert_eq!(manifest.package_version, PACKAGE_VERSION);
    assert!(
        manifest
            .entries
            .iter()
            .all(|entry| entry.state == EntryState::Created && entry.backup.is_none()),
        "an empty destination root means every file is created: {:?}",
        manifest.entries
    );
    for (path, content) in [
        (layout.addon_dir.join("librspinyin.so"), "library bytes"),
        (layout.addon_dir.join("librspinyin_ui.so"), "library bytes"),
        (
            layout.addon_conf_dir.join("rspinyin.conf"),
            "addon descriptor",
        ),
        (
            layout.addon_conf_dir.join("rspinyin-ui.conf"),
            "addon descriptor",
        ),
        (
            layout.input_method_dir.join("rspinyin.conf"),
            "input method",
        ),
        (layout.data_dir.join("base.dict"), "dictionary"),
    ] {
        assert_eq!(read(&path), content, "{}", path.display());
        let mode = fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, FILE_MODE, "{}", path.display());
    }
    assert!(
        Manifest::read(&layout.manifest_path())
            .expect("reading the manifest")
            .is_some(),
        "the manifest is on disk"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_apply_twice_is_idempotent_and_keeps_the_first_backup() {
    let (root, sources, layout) = fixture("idempotent");
    // The distribution shipped a descriptor at the same path before this installer ran.
    let descriptor = layout.addon_conf_dir.join("rspinyin.conf");
    fs::create_dir_all(descriptor.parent().expect("the fixture has a parent"))
        .expect("creating the fixture directory");
    fs::write(&descriptor, "distribution version").expect("writing the fixture");

    let first = apply(&plan(&sources, &layout), &layout, Elevation::Direct).expect("first run");
    assert_eq!(read(&descriptor), "addon descriptor");
    let backup = manifest::backup_path(&descriptor);
    assert_eq!(read(&backup), "distribution version");
    assert_eq!(
        first.entry(&descriptor).expect("the entry").state,
        EntryState::Replaced
    );

    // A second run, with a rebuilt artifact, must not lose the only copy of what was
    // there before: that is the whole reason the manifest exists.
    artifact(
        &root,
        "packaging/fcitx5/rspinyin.conf",
        "rebuilt descriptor",
    );
    let second = apply(&plan(&sources, &layout), &layout, Elevation::Direct).expect("second run");
    assert_eq!(read(&descriptor), "rebuilt descriptor");
    assert_eq!(
        read(&backup),
        "distribution version",
        "the backup still holds what the distribution shipped"
    );
    assert_eq!(second.entries.len(), first.entries.len(), "no duplicates");
    assert_eq!(second.entries, first.entries);
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_prepare_entry_classifies_a_destination_and_copies_what_it_displaces() {
    let (root, sources, layout) = fixture("classify");
    let plan = plan(&sources, &layout);
    let file = plan.first().expect("the library is first");

    let created = prepare_entry(file, Elevation::Direct).expect("an absent destination");
    assert_eq!(created.state, EntryState::Created);
    assert!(created.backup.is_none());

    fs::create_dir_all(file.destination.parent().expect("a parent")).expect("creating");
    fs::write(&file.destination, "someone else's file").expect("writing the fixture");
    let replaced = prepare_entry(file, Elevation::Direct).expect("an existing destination");
    assert_eq!(replaced.state, EntryState::Replaced);
    let backup = replaced.backup.expect("a replaced file is backed up");
    assert_eq!(read(&backup), "someone else's file");
    assert_eq!(
        read(&file.destination),
        "someone else's file",
        "taking the copy does not move the original"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_check_size_accepts_the_threshold_and_rejects_one_byte_more() {
    let root = scratch("size");
    let file = root.join("artifact");
    let mebibyte = 1024 * 1024;

    fs::write(&file, vec![0u8; mebibyte]).expect("writing the fixture");
    check_size(Budget::StrippedLibrary, &file, 1.0).expect("exactly at the threshold passes");

    fs::write(&file, vec![0u8; mebibyte + 1]).expect("writing the fixture");
    let failure =
        check_size(Budget::StrippedLibrary, &file, 1.0).expect_err("one byte over is a failure");
    assert!(failure.to_string().contains("BUDGET-SIZE-01"), "{failure}");
    assert!(failure.to_string().contains("1.00MiB"), "{failure}");
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_verify_sizes_measures_every_payload_that_carries_a_budget() {
    let (root, sources, layout) = fixture("sizes");
    let plan = plan(&sources, &layout);
    let budgets = SizeBudgets {
        library_mb: 0.000_001,
        dictionary_mb: 20.0,
    };

    // The library is over its budget, the dictionary is not: the first one named in the
    // plan is the one that stops the install.
    let failure = verify_sizes(&plan, &budgets).expect_err("the library is over budget");
    assert!(failure.to_string().contains("BUDGET-SIZE-01"), "{failure}");

    let generous = SizeBudgets {
        library_mb: 12.0,
        dictionary_mb: 20.0,
    };
    verify_sizes(&plan, &generous).expect("both artifacts are within budget");
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_verify_factory_symbol_accepts_a_library_that_exports_it() {
    let root = scratch("symbol-ok");
    let library = root.join("librspinyin.so");
    fs::write(
        &library,
        elf::synthetic_image(&[(FACTORY_SYMBOL, true), ("rspinyin_plugin_init", true)]),
    )
    .expect("writing the fixture");

    verify_factory_symbol(&library).expect("the factory symbol is exported");
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_verify_factory_symbol_refuses_a_library_that_does_not_export_it() {
    let root = scratch("symbol-missing");
    let library = root.join("librspinyin.so");
    // What `strip --strip-all` leaves behind: the name survives in the string table of
    // some other image, but nothing here defines it.
    fs::write(
        &library,
        elf::synthetic_image(&[(FACTORY_SYMBOL, false), ("rspinyin_plugin_init", true)]),
    )
    .expect("writing the fixture");

    let failure = verify_factory_symbol(&library).expect_err("an unexported symbol is refused");
    assert!(failure.to_string().contains(FACTORY_SYMBOL), "{failure}");
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_verify_factory_symbol_refuses_a_file_that_is_not_an_elf_image() {
    let root = scratch("symbol-foreign");
    let library = root.join("librspinyin.so");
    fs::write(&library, b"a library that was never an ELF").expect("writing the fixture");

    // Fail closed: a file whose exports cannot be read is not one to install.
    let failure = verify_factory_symbol(&library).expect_err("an unreadable image is refused");
    assert!(failure.to_string().contains("librspinyin.so"), "{failure}");
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_stage_library_refuses_a_library_that_is_not_a_shared_object() {
    let (root, sources, layout) = fixture("stage-refused");
    let mut plan = plan(&sources, &layout);

    // Whether `strip` is installed or not, a fixture that is not an ELF image cannot
    // pass the symbol check, so the install stops before anything is copied.
    let failure = stage_library(&sources, &mut plan).expect_err("a text file is not a library");
    let message = failure.to_string();
    assert!(
        message.contains(FACTORY_SYMBOL) || message.contains("strip"),
        "the refusal says why: {message}"
    );
    assert!(
        !layout.addon_dir.join("librspinyin.so").exists(),
        "nothing reached the destination"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_report_plan_and_report_installed_describe_the_run() {
    // The two reporting functions are the user's only account of what happened, so they
    // have to run over a real plan without panicking.
    let (root, sources, layout) = fixture("report");
    let plan = plan(&sources, &layout);
    let budgets = SizeBudgets {
        library_mb: 12.0,
        dictionary_mb: 20.0,
    };
    report_plan(&plan, &layout, Elevation::Sudo, &budgets);
    let manifest = apply(&plan, &layout, Elevation::Direct).expect("installing");
    report_installed(&manifest, &layout);
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_budget_identifiers_match_the_specification() {
    // The identifiers are matched by diagnostics, so they are not free to drift.
    assert_eq!(Budget::StrippedLibrary.id(), "BUDGET-SIZE-01");
    assert_eq!(Budget::BaseDictionary.id(), "BUDGET-SIZE-02");
}

#[test]
fn test_size_budgets_load_reads_the_repository_thresholds() {
    // The installer must take its thresholds from the machine-readable budget document
    // rather than from a second copy of the numbers.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the repository");
    let budgets = SizeBudgets::load(&Sources::new(root.to_path_buf()))
        .expect("the repository budget document is readable");
    assert!(budgets.library_mb > 0.0);
    assert!(budgets.dictionary_mb > 0.0);
}
