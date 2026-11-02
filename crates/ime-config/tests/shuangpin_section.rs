//! Integration tests for the `[shuangpin]` section of a configuration document.
//!
//! Responsibility: pin what the document pipeline does with the section -- that an
//! inline layout travels key by key into the configuration in force, that an absent
//! section keeps the defaults, that the shipped template defines no layout, and that
//! a reload adopts a changed layout against the defaults like every other section.
//! The engine-side compile of these values and the layout loader's own validation
//! live above and below this layer and carry their tests there; what this file owns
//! is the document-to-`Config` half of the chain.
//!
//! Boundaries: every test names the file it owns, so nothing here reads `$HOME` or
//! the clock. The scratch directories live under the system temp directory, one per
//! test.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use ime_config::{Config, ConfigStore, FILE_NAME, ReloadOutcome};
use ime_types::{ImeError, SchemeId};

/// The `[shuangpin.custom]` section of a layout, written as a user writes it.
const LAYOUT_DOCUMENT: &str = "\
[shuangpin.custom]
name = \"测试布局\"
zero_initial_style = \"first-letter\"

[shuangpin.custom.initials]
zh = \"v\"
ch = \"i\"
sh = \"u\"
";
// The initials above are a few entries, not a layout: a full layout has to reach
// every syllable of the table, and this layer's test does not need one that compiles.
// It needs the section's keys to travel into the configuration in force. The compile
// and its reachability rule are the loader's, tested in `ime-core`.

/// The `config/invalid` key of each diagnostic.
fn rejected(warnings: &[ImeError]) -> Vec<String> {
    warnings
        .iter()
        .filter_map(|warning| match warning {
            ImeError::ConfigInvalid { key, .. } => Some(key.clone()),
            _ => None,
        })
        .collect()
}

/// The diagnostics of one reload, whatever outcome it had.
fn reloaded(store: &mut ConfigStore) -> (bool, Vec<ImeError>) {
    match store.reload() {
        ReloadOutcome::Updated { warnings } => (true, warnings),
        ReloadOutcome::Kept { warnings } => (false, warnings),
        ReloadOutcome::Unchanged => (false, Vec::new()),
    }
}

/// A scratch directory for one test, named after the test.
fn scratch(name: &str) -> PathBuf {
    let directory = env::temp_dir().join(format!("rspinyin-ime-config-shuangpin-{name}"));
    let _ = fs::remove_dir_all(&directory);
    assert!(fs::create_dir_all(&directory).is_ok(), "scratch directory");
    directory
}

/// Writes `text` to `path`.
fn write(path: &Path, text: &str) {
    assert!(fs::write(path, text).is_ok(), "writing {path:?}");
}

#[test]
fn test_shuangpin_section_parses_the_inline_tables_key_by_key() {
    // Every key the document sets lands in the section in force, and every key it
    // leaves out keeps its default -- which is "not written", the state the loader
    // refuses like a document that left it out.
    let document = format!("{LAYOUT_DOCUMENT}\n[ui]\nmax_per_row = 7\n");
    let (config, warnings) = Config::from_document(&document).expect("a legal document");

    assert!(warnings.is_empty(), "{warnings:?}");
    let custom = &config.shuangpin.custom;
    assert!(!custom.is_unset(), "the document names a layout");
    assert_eq!(custom.name, "测试布局");
    assert_eq!(custom.zero_initial_style, "first-letter");
    assert_eq!(custom.initials.get("zh").map(String::as_str), Some("v"));
    assert_eq!(custom.initials.get("ch").map(String::as_str), Some("i"));
    assert!(custom.finals.is_empty(), "the document left the table out");
    assert!(
        custom.standalone.is_empty(),
        "the document left the table out"
    );
    assert_eq!(
        config.ui.max_per_row, 7,
        "the other sections read as always"
    );
}

#[test]
fn test_shuangpin_section_absent_keeps_the_defaults() {
    // A document that says nothing about layouts leaves the section at its shipped
    // state: no file, no tables, no layout -- and no diagnostic, because naming no
    // layout is not naming a broken one.
    let (config, warnings) = Config::from_document("[ui]\nmax_per_row = 5\n").expect("legal");

    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(config.shuangpin.custom.is_unset());
    assert_eq!(config.shuangpin.custom.file, "");
    assert_eq!(
        config,
        Config::default(),
        "the section is exactly the default"
    );
}

#[test]
fn test_default_template_names_no_custom_layout() {
    // The template documents the section without defining a layout: it parses to the
    // built-in defaults with nothing to report, so a fresh installation opens with
    // the built-in layouts and the documentation of how to add one of its own.
    let (config, warnings) =
        Config::from_document(ime_config::reload::DEFAULT_CONFIG_TOML).expect("the template");

    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(
        config.shuangpin.custom.is_unset(),
        "the template defines no layout"
    );
    assert!(
        config.validate().is_empty(),
        "the template needs no repair: {:?}",
        config.validate()
    );
}

#[test]
fn test_reload_adopts_the_changed_custom_layout() {
    // A reload that swaps the layout re-reads the document against the defaults and
    // adopts the result whole, and the snapshot an in-flight reader holds is
    // untouched -- the store's promise for every section, this one included.
    let directory = scratch("reload");
    let path = directory.join(FILE_NAME);
    write(&path, LAYOUT_DOCUMENT);
    let (mut store, warnings) = ConfigStore::load_at(&path, 7);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        store.current().shuangpin.custom.name,
        "测试布局",
        "the layout the document named is in force"
    );
    let snapshot = store.current().clone();

    write(
        &path,
        "[shuangpin.custom]\nname = \"改名布局\"\nzero_initial_style = \"o-led\"\n",
    );
    let (updated, warnings) = reloaded(&mut store);
    assert!(updated, "a changed layout is an update");
    assert!(warnings.is_empty(), "{warnings:?}");

    let custom = &store.current().shuangpin.custom;
    assert_eq!(custom.name, "改名布局");
    assert_eq!(custom.zero_initial_style, "o-led");
    assert!(
        custom.initials.is_empty(),
        "the adopted document starts from the defaults, not the previous layout"
    );
    assert_eq!(
        snapshot.shuangpin.custom.name, "测试布局",
        "the old snapshot is untouched"
    );

    // Nothing changed on disk, so nothing changes in the store.
    assert!(matches!(store.reload(), ReloadOutcome::Unchanged));
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn test_scheme_custom_spelling_is_reported_as_superseded() {
    // The retired spelling degrades to full pinyin with one diagnostic that points at
    // the section a layout is defined in now, instead of claiming the feature does
    // not exist.
    let (config, warnings) =
        Config::from_document("[scheme]\nscheme = \"custom\"\n").expect("a legal document");

    assert_eq!(rejected(&warnings), [String::from("scheme.scheme")]);
    let rendered = warnings
        .first()
        .map(|warning| warning.to_string())
        .unwrap_or_default();
    assert!(
        rendered.contains("shuangpin.custom"),
        "the diagnostic names the section that supersedes the key: {rendered}"
    );
    assert_eq!(
        config.scheme.decode_settings(),
        (SchemeId::FULL, true),
        "the repaired spelling answers full pinyin"
    );
}
