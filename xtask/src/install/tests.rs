//! Tests for the install direction.
//!
//! Every test works in a scratch tree with a scratch destination root, injected through
//! [`Sources`] and [`Layout`]. Nothing here reads or writes a real Fcitx5 installation,
//! a real `$HOME` or a real build tree, and nothing elevates.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

// The names the install direction keeps in its submodules rather than at its root: the
// glob below reaches everything the module root imports, and nothing else.
use super::layout::Destination;
use super::manifest::{EntryState, FILE_MODE, MANIFEST_VERSION, Manifest};
use super::payload::{Payload, PlannedFile};
use super::size::check_size;
use super::stage::verify_factory_symbol;
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

/// Writes a minimal but real dictionary container into the scratch tree.
///
/// The install path reads the dictionary before copying it -- a `--dict` file is
/// untrusted input and the reader is the only thing that knows what a container looks
/// like -- so a fixture that wrote the word "dictionary" would be refused before the size
/// gate ever ran. This is the smallest container the reader accepts: one empty FST
/// section plus the header and checksums the writer computes.
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

/// Copies the repository's budget document into a scratch tree.
///
/// The installer reads its size thresholds from the document rather than from a constant,
/// so a tree without one cannot get as far as a plan. It is part of the fixture rather
/// than of the tests that measure something because every install reads it.
fn budget_document(root: &Path) {
    let target = root.join(crate::budget::BUDGETS_FILE);
    fs::create_dir_all(target.parent().expect("the document has a parent"))
        .expect("creating the document directory");
    fs::copy(repository().join(crate::budget::BUDGETS_FILE), &target)
        .expect("copying the budget document");
}

/// A scratch tree holding every artifact the payload table names.
///
/// One line per payload, so adding a payload to the table without teaching the fixture
/// about it fails here rather than passing a plan that names a file no build produces.
/// The icons are drawn in the repository's `assets/`, and the fixture writes its own
/// copies because `Sources` resolves every artifact against the scratch root.
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
    artifact(&root, "assets/icon-48.png", "png bytes");
    artifact(&root, "assets/icon.svg", "svg bytes");
    dictionary(&root, "data/compiled/base.dict");
    budget_document(&root);
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

/// The repository root, for the tests that read a committed file rather than a fixture.
///
/// Derived from `CARGO_MANIFEST_DIR` rather than from the working directory: a test binary
/// runs with the working directory set to the crate, while every committed path is written
/// relative to the repository root.
fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the repository")
        .to_path_buf()
}

/// The payload the table installs under `name`.
fn payload(name: &str) -> &'static Payload {
    PAYLOADS
        .iter()
        .find(|payload| payload.name == name)
        .expect("the payload table carries this file")
}

#[test]
fn test_plan_install_resolves_every_payload_in_the_order_they_are_copied() {
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
            layout.icon_dir.join("48x48/apps/fcitx-rspinyin.png"),
            layout.icon_dir.join("scalable/apps/fcitx-rspinyin.svg"),
        ],
        "every payload is planned, icons included"
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
fn test_payload_table_installs_the_icons_where_the_theme_looks_for_them() {
    // The names are what `packaging/fcitx5/rspinyin-im.conf` puts in its `Icon=` key, and
    // the two directories are the ones the hicolor theme's index lists: a lookup by name
    // finds the raster at the size it was drawn for, and the vector for every other size
    // a panel or a tray asks for.
    let raster = payload("fcitx-rspinyin.png");
    assert_eq!(raster.artifact.to_vec(), vec!["assets/icon-48.png"]);
    assert_eq!(raster.destination, Destination::Icon { size: "48x48" });
    assert!(
        !raster.optional,
        "the icon is committed, so a missing one is a broken checkout rather than an \
         artifact a build has not produced yet"
    );

    let vector = payload("fcitx-rspinyin.svg");
    assert_eq!(vector.artifact.to_vec(), vec!["assets/icon.svg"]);
    assert_eq!(vector.destination, Destination::Icon { size: "scalable" });
    assert!(!vector.optional);
}

#[test]
fn test_plan_install_refuses_a_missing_icon() {
    // The behavioural half of the table above: a checkout that lost an icon has to stop
    // the install rather than produce a plugin `fcitx5-configtool` shows with a
    // placeholder.
    for missing in ["assets/icon-48.png", "assets/icon.svg"] {
        let (root, sources, layout) = fixture("plan-icon-missing");
        fs::remove_file(root.join(missing)).expect("removing the fixture");

        let resolved = plan_install(PAYLOADS, &sources, &layout);
        let failure = resolved.expect_err("an icon that is not there is not installable");
        assert!(
            failure.to_string().contains(missing),
            "the message names what is missing: {failure}"
        );
        fs::remove_dir_all(&root).expect("cleaning up");
    }
}

#[test]
fn test_apply_installs_every_file_with_the_documented_mode() {
    let (root, sources, layout) = fixture("apply");
    let manifest = apply(&plan(&sources, &layout), &layout, Elevation::Direct)
        .expect("installing into a writable tree");

    assert_eq!(manifest.entries.len(), 8);
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
        // The icons are installed into a theme directory rather than into Fcitx5's own,
        // and they are copied by the same sequence as everything else: asserting them
        // here is what says a panel that looks the icon up by name finds a readable file
        // rather than a directory entry with nothing in it.
        (
            layout.icon_dir.join("48x48/apps/fcitx-rspinyin.png"),
            "png bytes",
        ),
        (
            layout.icon_dir.join("scalable/apps/fcitx-rspinyin.svg"),
            "svg bytes",
        ),
    ] {
        assert_eq!(read(&path), content, "{}", path.display());
        let mode = fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, FILE_MODE, "{}", path.display());
    }
    // The dictionary is checked separately: it is a binary container rather than text, so
    // what it has to satisfy is that the loader can read it back, not that it holds a
    // particular string.
    let installed_dict = layout.data_dir.join("base.dict");
    let mode = fs::metadata(&installed_dict)
        .expect("stat")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, FILE_MODE, "{}", installed_dict.display());
    ime_dict::format::reader::Reader::open_with(
        &installed_dict,
        ime_dict::format::reader::Verify::Full,
    )
    .expect("the installed dictionary is a container the loader accepts");
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
fn test_install_dry_run_changes_nothing_anywhere_in_the_tree() {
    // The card asks for a dry run that only prints, and the property a read-only
    // filesystem would enforce is the one asserted here: nothing in the tree changes,
    // not even the staged copy a real install makes of the library before stripping it.
    let (root, sources, layout) = fixture("dry-run");
    fs::create_dir_all(&layout.destdir).expect("creating the destination root");
    let before = super::reversible::capture(&root).expect("a scratch tree");
    let options = Options {
        dry_run: true,
        no_sudo: true,
    };

    install(&sources, &layout, &options).expect("a dry run reports the plan and stops");

    let after = super::reversible::capture(&root).expect("the same tree");
    let differences = before.difference(&after);
    assert!(
        differences.is_empty(),
        "a dry run writes nothing: {differences:?}"
    );
    assert!(
        !layout.addon_dir.exists(),
        "no destination directory was created either"
    );
    assert!(!sources.staging_dir.exists(), "and no library was staged");
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

/// The document with every XML comment removed.
///
/// The icon's own comments discuss the very things a self-contained drawing must not
/// contain: they name `@import` and `href` while saying the file uses neither. A scan that
/// did not skip them would fail on the explanation rather than on the drawing.
fn without_comments(svg: &str) -> String {
    let mut rest = svg;
    let mut kept = String::new();
    while let Some(start) = rest.find("<!--") {
        kept.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            // An unterminated comment swallows the rest of the document, which is what a
            // parser would make of it too.
            None => return kept,
        }
    }
    kept.push_str(rest);
    kept
}

/// The WCAG relative luminance of a colour written as `#RRGGBB`.
fn luminance(colour: &str) -> f64 {
    let channel = |offset: usize| {
        let pair = colour.get(offset..offset + 2).expect("a #RRGGBB colour");
        let value = f64::from(u8::from_str_radix(pair, 16).expect("a hexadecimal channel"));
        let value = value / 255.0;
        if value <= 0.039_28 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5)
}

/// The WCAG contrast ratio between two colours written as `#RRGGBB`.
fn contrast(first: &str, second: &str) -> f64 {
    let (first, second) = (luminance(first), luminance(second));
    let (lighter, darker) = if first > second {
        (first, second)
    } else {
        (second, first)
    };
    (lighter + 0.05) / (darker + 0.05)
}

/// The distinct `fill` colours of an SVG document, in the order they appear.
///
/// Owned strings rather than slices: the comments are stripped first, so the colours are
/// found in the copy rather than in the document the caller passed in.
fn fills(svg: &str) -> Vec<String> {
    let drawing = without_comments(svg);
    let mut found: Vec<String> = Vec::new();
    for (at, _) in drawing.match_indices("fill=\"#") {
        // `fill="#` is seven bytes, so the colour starts at the sixth and is `#RRGGBB`.
        let colour = drawing
            .get(at + 6..at + 13)
            .expect("a #RRGGBB fill")
            .to_owned();
        if !found.contains(&colour) {
            found.push(colour);
        }
    }
    found
}

#[test]
fn test_contrast_ratio_is_bounded_by_one_and_twenty_one() {
    // The helper is what the assertions below rest on, and a `contrast` that returned a
    // large number for everything would let them pass without measuring anything.
    let same = contrast("#000000", "#000000");
    let widest = contrast("#FFFFFF", "#000000");
    let reversed = contrast("#000000", "#FFFFFF");
    assert!((same - 1.0).abs() < 1e-9, "a colour against itself: {same}");
    assert!(
        (widest - 21.0).abs() < 1e-6,
        "white on black is the widest pair there is"
    );
    assert!(
        (reversed - 21.0).abs() < 1e-6,
        "and it does not depend on the order given"
    );
}

#[test]
fn test_icon_clears_the_contrast_floor_on_a_light_and_a_dark_panel() {
    // `features.md` 3.2 fixes the two surfaces a desktop can put behind an application
    // icon -- a white panel and the dark `surface.base` -- and asks for 4.5:1 against
    // whichever one it is drawn on. One flat colour cannot clear both: a colour far enough
    // from white is too close to the dark panel, which is why the drawing is a plate with
    // marks on it and every pair is measured against what it actually sits on.
    let svg = fs::read_to_string(repository().join("assets/icon.svg")).expect("the source");
    let found = fills(&svg);
    assert_eq!(
        found.len(),
        2,
        "a plate and one colour for the marks: {found:?}"
    );
    let (plate, marks) = (&found[0], &found[1]);

    // A light desktop sees the plate's own edge against the panel behind it.
    let on_light = contrast(plate, "#FFFFFF");
    assert!(
        on_light >= 4.5,
        "the plate on a white panel: {on_light:.2}:1"
    );
    // A dark desktop sees the marks: the plate is a background there, and the pair of the
    // two is deliberately not asserted, because the drawing does not rely on it.
    for (pair, ratio) in [
        ("the marks on the plate", contrast(marks, plate)),
        ("the marks on the dark panel", contrast(marks, "#1C1C1E")),
    ] {
        assert!(ratio >= 4.5, "{pair}: {ratio:.2}:1");
    }
}

#[test]
fn test_icon_asset_is_a_48_pixel_rgba_png() {
    // The bitmap is installed into `hicolor/48x48/apps`, so its own header is the only
    // thing that can say the directory and the drawing agree. Alpha is load-bearing rather
    // than incidental: the plate's corners are rounded and transparent, and an opaque
    // image would put a square of the plate's colour on every panel.
    let bytes = fs::read(repository().join("assets/icon-48.png")).expect("the icon is committed");
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "the file is a PNG");

    // The IHDR chunk is the first one and its payload starts sixteen bytes in: eight for
    // the signature, four for the length and four for the chunk type. Its fields are the
    // width, the height, the bit depth and the colour type, each four, four, one and one
    // byte wide.
    let header = |offset: usize| {
        let raw = bytes.get(offset..offset + 4).expect("a complete IHDR");
        let field: [u8; 4] = raw.try_into().expect("four bytes");
        u32::from_be_bytes(field)
    };
    let byte = |offset: usize| *bytes.get(offset).expect("a complete IHDR");
    assert_eq!(
        (header(16), header(20)),
        (48, 48),
        "the size it installs at"
    );
    assert_eq!(byte(24), 8, "eight bits per channel");
    assert_eq!(
        byte(25),
        6,
        "colour type 6 is RGBA, so the corners can be clear"
    );
}

#[test]
fn test_icon_source_is_self_contained_and_carries_no_text() {
    // A release is unpacked on a machine that may never have had the network, so the
    // drawing may not reach outside itself: no linked image, no referenced font, no
    // stylesheet, no script. It also carries no glyph -- `fcitx-rspinyin` is the icon's
    // *name*, and a word drawn inside the plate would be a second, untranslated label
    // beside the one Fcitx5 already prints.
    let svg = fs::read_to_string(repository().join("assets/icon.svg")).expect("the source");
    let drawing = without_comments(&svg);
    for forbidden in ["href=", "<image", "<use", "<script", "<style", "<text"] {
        assert!(
            !drawing.contains(forbidden),
            "the drawing references something outside itself: {forbidden}"
        );
    }
    assert!(
        drawing.contains("viewBox=\"0 0 48 48\""),
        "the drawing sits on the 48-unit canvas the rest of the UI is laid out on"
    );
}

#[test]
fn test_input_method_descriptor_names_the_icons_the_payload_table_installs() {
    // Fcitx5 resolves an input method's icon by name: the descriptor's `Icon=` key is
    // looked up in the hicolor theme, which is why the two installed files have to be
    // named after it. A key the table does not install is a placeholder in
    // `fcitx5-configtool`, and nothing else in the tree compares the two.
    let text = fs::read_to_string(repository().join("packaging/fcitx5/rspinyin-im.conf"))
        .expect("the descriptor is committed");
    let named = text.lines().find_map(|line| line.strip_prefix("Icon="));
    let icon = named
        .expect("the input method descriptor names an icon")
        .trim();
    assert!(!icon.is_empty(), "an empty icon name resolves to nothing");

    for name in [format!("{icon}.png"), format!("{icon}.svg")] {
        assert!(
            PAYLOADS.iter().any(|payload| payload.name == name.as_str()),
            "the descriptor names `{icon}` and no payload installs `{name}`"
        );
    }
}
