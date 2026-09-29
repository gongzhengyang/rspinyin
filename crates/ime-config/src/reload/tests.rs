//! Unit tests for the configuration loader and the reload path.
//!
//! Responsibility: pin what reading and re-reading `config.toml` does to the filesystem --
//! the template a fresh install is given and the defaults it states, the backup a corrupt
//! file is moved to, the rule that a reload improves the configuration in force or leaves
//! it alone, and the binding table and decode settings a reload adopts together with the
//! configuration.
//!
//! Boundaries: every test names the file it owns and passes the stamp a backup name
//! carries, so nothing here reads `$HOME` or the clock. The scratch directories live under
//! the system temp directory, one per test.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use ime_types::{ImeError, SchemeId};

use super::*;
use crate::keymap::{
    BINDING_CONFLICT_CODE, FlipSet, HighlightSet, UNROUTABLE_BINDING_CODE, project_keys,
};
use crate::schema::{DigitZero, Durability, LogLevel, PunctMode, ThemeScheme};

/// A scratch directory for one test, named after the test so two tests never share
/// one. The loader is given an explicit path and an explicit stamp, so no test reads
/// `$HOME` or the clock.
fn scratch(name: &str) -> PathBuf {
    let directory = env::temp_dir().join(format!("rspinyin-ime-config-{name}"));
    let _ = fs::remove_dir_all(&directory);
    assert!(fs::create_dir_all(&directory).is_ok(), "scratch directory");
    directory
}

/// Writes `text` to `path`.
fn write(path: &Path, text: &str) {
    assert!(fs::write(path, text).is_ok(), "writing {path:?}");
}

/// The text of `path`, or an empty string when it cannot be read; the caller's
/// assertion is what fails the test.
fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

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

/// The diagnostics of a reload that was expected to keep the configuration in force.
///
/// `Kept` is the only outcome that carries a reason, so any other outcome leaves the
/// vector empty and fails the caller's assertion.
fn kept_by(store: &mut ConfigStore) -> Vec<String> {
    match store.reload() {
        ReloadOutcome::Kept { warnings } => rejected(&warnings),
        ReloadOutcome::Updated { .. } | ReloadOutcome::Unchanged => Vec::new(),
    }
}

#[test]
fn test_default_path_ends_in_the_configuration_file() {
    // Asserted on the tail only: the test must not depend on what `$HOME` holds.
    if let Some(path) = default_path() {
        assert!(
            path.ends_with(Path::new("rspinyin").join(FILE_NAME)),
            "{path:?}"
        );
    }
}

#[test]
fn test_load_missing_file_uses_the_defaults_and_writes_the_template() {
    let path = scratch("missing").join(FILE_NAME);
    let (config, warnings) = Config::load_at(&path, 7);
    assert_eq!(
        config,
        Config::default(),
        "a fresh install runs on defaults"
    );
    assert!(warnings.is_empty(), "having no file yet is not a problem");
    assert_eq!(read(&path), DEFAULT_CONFIG_TOML, "the template is written");
}

#[test]
fn test_load_corrupt_file_is_backed_up_and_the_defaults_are_used() {
    let directory = scratch("corrupt");
    let path = directory.join(FILE_NAME);
    let corrupt = "this is not TOML\n";
    write(&path, corrupt);

    let (config, warnings) = Config::load_at(&path, 7);
    assert_eq!(config, Config::default());
    assert_eq!(rejected(&warnings), [String::from(DOCUMENT_KEY)]);
    assert_eq!(
        read(&directory.join(format!("{FILE_NAME}.bad.7"))),
        corrupt,
        "the user's file is moved aside, never deleted"
    );
    assert_eq!(
        read(&path),
        DEFAULT_CONFIG_TOML,
        "a template takes its place"
    );

    // The same second, a second corrupt file: the backup name is taken, so the next
    // one is used rather than the first backup being overwritten.
    write(&path, corrupt);
    assert_eq!(Config::load_at(&path, 7).0, Config::default());
    assert_eq!(
        read(&directory.join(format!("{FILE_NAME}.bad.7.1"))),
        corrupt
    );
}

#[test]
fn test_load_reads_every_key_from_a_document() {
    let path = scratch("every-key").join(FILE_NAME);
    write(
        &path,
        "schema_version = 2\n\
         [engine]\npunct_mode = \"english\"\nfull_width = true\n\
         auto_english_on_uppercase = false\npassthrough_url = false\nmax_raw_len = 1\n\
         verify_dict_on_load = \"header\"\n\
         [ui]\nclient_preedit = true\nmax_per_row = 9\nshow_annotation = false\n\
         max_width_dp = 1200\ncorner_radius_dp = 20\nbase_alpha = 255\n\
         [ui.animation]\nenabled = false\nomega0 = 4.0\nzeta = 0.3\n\
         appear_ms = 600\ndisappear_ms = 0\n\
         [theme]\nscheme = \"dark\"\naccent = \"#4c9aff\"\n\
         [keys]\ndigit_zero = \"flip\"\nenter_commit_raw = true\n\
         flip_keys = [\"minus\", \"page_up\"]\nhighlight_keys = [\"tab\"]\n\
         [data]\ndurability = \"immediate\"\n\
         [diagnostics]\nlevel = \"trace\"\nlog_rotation_mb = 1\nlog_keep_files = 1\n\
         log_input_content = true\nprobes = false\n",
    );

    let (config, warnings) = Config::load_at(&path, 7);
    assert!(warnings.is_empty(), "every value is in range: {warnings:?}");
    assert_eq!(config.engine.punct_mode, PunctMode::English);
    assert!(config.engine.full_width && !config.engine.passthrough_url);
    assert!(!config.engine.auto_english_on_uppercase);
    assert_eq!(config.engine.max_raw_len, 1);
    assert!(config.ui.client_preedit && !config.ui.show_annotation);
    assert_eq!(config.ui.max_per_row, 9);
    assert_eq!(config.ui.max_width_dp, 1200);
    assert_eq!(config.ui.corner_radius_dp, 20);
    assert_eq!(config.ui.base_alpha, 255);
    assert!(!config.ui.animation.enabled);
    assert_eq!(config.ui.animation.omega0, 4.0);
    assert_eq!(config.ui.animation.zeta, 0.3);
    assert_eq!(config.ui.animation.appear_ms, 600);
    assert_eq!(config.ui.animation.disappear_ms, 0);
    assert_eq!(config.theme.scheme, ThemeScheme::Dark);
    assert_eq!(config.theme.accent.rgb(), [0x4C, 0x9A, 0xFF]);
    assert_eq!(config.keys.digit_zero, DigitZero::Flip);
    assert!(config.keys.enter_commit_raw);
    assert_eq!(config.keys.flip_keys.len(), 2);
    assert_eq!(config.keys.highlight_keys, [KeyName::Tab]);
    assert_eq!(config.data.durability, Durability::Immediate);
    assert_eq!(config.diagnostics.level, LogLevel::Trace);
    assert!(config.diagnostics.log_input_content && !config.diagnostics.probes);

    // The document was read, not written: the loader never rewrites a user's file.
    assert!(read(&path).starts_with("schema_version = 2"));
}

#[test]
fn test_load_reports_every_kind_of_unusable_document() {
    // A value inside its field but outside its documented range, and a repeat in a
    // binding list: both are repaired, and each names the key it came from.
    let path = scratch("out-of-range").join(FILE_NAME);
    write(
        &path,
        "[ui]\nmax_per_row = 2\n[keys]\nflip_keys = [\"minus\", \"minus\"]\n",
    );
    let (config, warnings) = Config::load_at(&path, 7);
    assert_eq!(config.ui.max_per_row, 5, "the default is restored");
    assert_eq!(config.keys.flip_keys.len(), 1, "the repeat is dropped");
    assert_eq!(
        rejected(&warnings),
        [String::from("ui.max_per_row"), String::from(KEY_FLIP_KEYS)]
    );

    // A spelling outside its set, and a name outside the whitelist. The merge visits
    // the sections in schema order, so the diagnostics arrive in that order, and every
    // closed set is exercised.
    let path = scratch("bad-spelling").join(FILE_NAME);
    write(
        &path,
        "[engine]\npunct_mode = \"Chinese\"\nverify_dict_on_load = \"whole\"\n\
         [theme]\nscheme = \"system\"\naccent = \"4C9AFF\"\n\
         [keys]\ndigit_zero = \"page\"\nflip_keys = [\"minus\", \"esc\"]\n\
         [data]\ndurability = \"always\"\n[diagnostics]\nlevel = \"verbose\"\n",
    );
    let (config, warnings) = Config::load_at(&path, 7);
    assert_eq!(
        config.engine.punct_mode,
        Config::default().engine.punct_mode
    );
    assert_eq!(config.keys.flip_keys, [KeyName::Minus], "the rest survives");
    assert_eq!(config.theme.accent, Config::default().theme.accent);
    assert_eq!(
        rejected(&warnings),
        [
            String::from("engine.punct_mode"),
            String::from("engine.verify_dict_on_load"),
            String::from("theme.scheme"),
            String::from("theme.accent"),
            String::from("keys.digit_zero"),
            String::from(KEY_FLIP_KEYS),
            String::from("data.durability"),
            String::from("diagnostics.level"),
        ]
    );

    // More keys than `ASM-19` allows: the surplus is ignored and reported, and the
    // document is still read. The keys have to be distinct — TOML rejects a repeated
    // key, so a document of one key repeated would fail to parse before the limit was
    // ever consulted, and the assertion below would be testing the wrong thing.
    let path = scratch("too-many-keys").join(FILE_NAME);
    let at_limit: String = (0..MAX_DOCUMENT_KEYS)
        .map(|n| format!("k{n} = 1\n"))
        .collect();
    write(&path, &at_limit);
    assert!(
        Config::load_at(&path, 7).1.is_empty(),
        "the limit is inclusive"
    );

    write(&path, &format!("{at_limit}k_extra = 2\n"));
    let warnings = Config::load_at(&path, 7).1;
    assert_eq!(rejected(&warnings), [String::from(DOCUMENT_KEY)]);
    assert!(
        warnings[0]
            .to_string()
            .contains(&format!("limit exceeded: {MAX_DOCUMENT_KEYS}")),
        "the message names the ceiling in force: {}",
        warnings[0]
    );
}

#[test]
fn test_reload_picks_up_a_changed_value() {
    let path = scratch("reload").join(FILE_NAME);
    write(&path, "[ui]\nmax_per_row = 5\n");
    let (mut store, warnings) = ConfigStore::load_at(&path, 7);
    assert!(warnings.is_empty());
    assert_eq!(store.current().ui.max_per_row, 5);

    // The configuration a component took before the reload, as an in-flight
    // composition would have: it must keep seeing what it started with.
    let snapshot = store.current().clone();
    write(&path, "[ui]\nmax_per_row = 7\n");
    assert!(matches!(store.reload(), ReloadOutcome::Updated { .. }));
    assert_eq!(store.current().ui.max_per_row, 7);
    assert_eq!(snapshot.ui.max_per_row, 5, "the old snapshot is untouched");

    // Nothing changed on disk, so nothing changes in the store.
    assert!(matches!(store.reload(), ReloadOutcome::Unchanged));
}

#[test]
fn test_reload_keeps_the_configuration_it_cannot_replace() {
    let directory = scratch("reload-kept");
    let path = directory.join(FILE_NAME);
    write(&path, "[ui]\nmax_per_row = 7\n");
    let (mut store, _) = ConfigStore::load_at(&path, 7);
    assert_eq!(store.current().ui.max_per_row, 7);

    // Gone: a file that disappears during a reload is likelier to be a save in
    // progress than a deliberate reset, and a restart is the way back to the
    // defaults.
    assert!(fs::remove_file(&path).is_ok());
    assert_eq!(kept_by(&mut store), [String::from(DOCUMENT_KEY)]);
    assert_eq!(store.current().ui.max_per_row, 7, "the value survives");
    assert!(!path.exists(), "a reload never writes the file back");

    // Corrupt: the file is left exactly as it is, because it may be an edit in
    // progress and the configuration in force is known to be good.
    let corrupt = "this is not TOML\n";
    write(&path, corrupt);
    assert_eq!(kept_by(&mut store), [String::from(DOCUMENT_KEY)]);
    assert_eq!(read(&path), corrupt, "the user's edit is left alone");
    assert!(
        !directory.join(format!("{FILE_NAME}.bad.7")).exists(),
        "a reload never moves a file aside"
    );

    // Unreadable: a directory in the file's place fails to read on every platform and
    // for every user, which a permission bit does not.
    let unreadable = directory.join("a-directory");
    assert!(fs::create_dir(&unreadable).is_ok());
    let (mut store, warnings) = ConfigStore::load_at(&unreadable, 7);
    assert_eq!(rejected(&warnings), [String::from(DOCUMENT_KEY)]);
    assert_eq!(**store.current(), Config::default());
    assert_eq!(kept_by(&mut store), [String::from(DOCUMENT_KEY)]);
}

#[test]
fn test_load_projects_the_keys_section_into_the_binding_table() {
    let path = scratch("load-bindings").join(FILE_NAME);
    write(
        &path,
        "[keys]\ndigit_zero = \"flip\"\nenter_commit_raw = true\n\
         flip_keys = [\"page_up\", \"page_down\"]\nhighlight_keys = [\"shift_tab\"]\n",
    );

    let (store, warnings) = ConfigStore::load_at(&path, 7);

    assert!(warnings.is_empty(), "every entry is routable: {warnings:?}");
    let bindings = store.bindings();
    assert_eq!(bindings.digit_zero, DigitZero::Flip);
    assert!(bindings.enter_commit_raw);
    assert_eq!(bindings.flip_keys, FlipSet::PAGE_UP | FlipSet::PAGE_DOWN);
    assert_eq!(bindings.highlight_keys, HighlightSet::SHIFT_TAB);
    assert!(
        !bindings.highlight_keys.contains(HighlightSet::TAB),
        "the document's list replaces the built-in one rather than adding to it"
    );
}

#[test]
fn test_load_reports_a_binding_the_projection_cannot_route() {
    // `left` is in the key-name whitelist, so the document is read without complaint --
    // but it pages nothing, and an entry that cannot become a binding is reported rather
    // than dropped in silence.
    let path = scratch("load-unroutable").join(FILE_NAME);
    write(&path, "[keys]\nflip_keys = [\"minus\", \"left\"]\n");

    let (store, warnings) = ConfigStore::load_at(&path, 7);

    assert_eq!(rejected(&warnings), [String::from(UNROUTABLE_BINDING_CODE)]);
    assert_eq!(
        store.bindings().flip_keys,
        FlipSet::MINUS,
        "the entry beside the rejected one still binds"
    );
}

#[test]
fn test_load_reports_an_unknown_key_name_once() {
    // A name outside the whitelist is the schema's to refuse, and the entry is dropped
    // before the projection ever sees it: one mistake must cost the user one diagnostic,
    // not one per layer.
    let path = scratch("load-unknown-name").join(FILE_NAME);
    write(&path, "[keys]\nflip_keys = [\"minus\", \"esc\"]\n");

    let (store, warnings) = ConfigStore::load_at(&path, 7);

    assert_eq!(rejected(&warnings), [String::from(KEY_FLIP_KEYS)]);
    assert_eq!(store.bindings().flip_keys, FlipSet::MINUS);
}

#[test]
fn test_reload_adopts_the_reprojected_binding_table() {
    let path = scratch("reload-bindings").join(FILE_NAME);
    write(&path, "[keys]\nflip_keys = [\"minus\", \"equal\"]\n");
    let (mut store, _) = ConfigStore::load_at(&path, 7);
    assert_eq!(store.bindings().flip_keys, FlipSet::MINUS | FlipSet::EQUAL);

    write(
        &path,
        "[keys]\nflip_keys = [\"page_up\", \"page_down\"]\nhighlight_keys = [\"left\", \"right\"]\n",
    );

    assert!(matches!(store.reload(), ReloadOutcome::Updated { .. }));
    let bindings = store.bindings();
    assert_eq!(bindings.flip_keys, FlipSet::PAGE_UP | FlipSet::PAGE_DOWN);
    assert_eq!(
        bindings.highlight_keys,
        HighlightSet::LEFT | HighlightSet::RIGHT
    );
}

#[test]
fn test_reload_replaces_the_binding_table_whole() {
    let path = scratch("reload-bindings-whole").join(FILE_NAME);
    write(
        &path,
        "[keys]\nflip_keys = [\"minus\"]\nhighlight_keys = [\"tab\"]\n",
    );
    let (mut store, _) = ConfigStore::load_at(&path, 7);

    write(
        &path,
        "[keys]\nflip_keys = [\"page_up\"]\nhighlight_keys = [\"shift_tab\"]\n",
    );
    assert!(matches!(store.reload(), ReloadOutcome::Updated { .. }));

    let bindings = store.bindings();
    assert_eq!(bindings.flip_keys, FlipSet::PAGE_UP);
    assert_eq!(bindings.highlight_keys, HighlightSet::SHIFT_TAB);
    assert!(
        !bindings.flip_keys.contains(FlipSet::MINUS),
        "no page key of the previous configuration survives"
    );
    assert!(
        !bindings.highlight_keys.contains(HighlightSet::TAB),
        "no highlight key of the previous configuration survives"
    );
}

#[test]
fn test_reload_reports_a_binding_conflict_through_the_reload_diagnostics() {
    let path = scratch("reload-bindings-conflict").join(FILE_NAME);
    write(&path, "[keys]\nflip_keys = [\"minus\"]\n");
    let (mut store, _) = ConfigStore::load_at(&path, 7);

    // One key claimed by both lists. The configuration layer's own repair settles a repeat
    // inside one list, so a cross-list conflict reaches the projection, which is where it
    // is reported.
    write(
        &path,
        "[keys]\nflip_keys = [\"up\"]\nhighlight_keys = [\"up\"]\n",
    );

    let warnings = match store.reload() {
        ReloadOutcome::Updated { warnings } => rejected(&warnings),
        ReloadOutcome::Unchanged | ReloadOutcome::Kept { .. } => Vec::new(),
    };

    assert_eq!(warnings, [String::from(BINDING_CONFLICT_CODE)]);
    let bindings = store.bindings();
    assert!(
        !bindings.flip_keys.contains(FlipSet::UP),
        "the page binding is the one that gives way"
    );
    assert!(bindings.highlight_keys.contains(HighlightSet::UP));
}

#[test]
fn test_reload_keeps_the_binding_table_when_the_file_cannot_be_read() {
    let directory = scratch("reload-bindings-kept");
    let path = directory.join(FILE_NAME);
    write(&path, "[keys]\nflip_keys = [\"page_down\"]\n");
    let (mut store, _) = ConfigStore::load_at(&path, 7);
    let before = store.bindings();
    assert_eq!(before.flip_keys, FlipSet::PAGE_DOWN);

    // Gone, then corrupt: neither is something to adopt, so the table in force stays.
    assert!(fs::remove_file(&path).is_ok());
    assert_eq!(kept_by(&mut store), [String::from(DOCUMENT_KEY)]);
    assert_eq!(
        store.bindings(),
        before,
        "a kept configuration keeps its binding table"
    );

    write(&path, "this is not TOML\n");
    assert_eq!(kept_by(&mut store), [String::from(DOCUMENT_KEY)]);
    assert_eq!(store.bindings(), before);
}

#[test]
fn test_reload_of_an_unchanged_document_leaves_the_binding_table_alone() {
    let path = scratch("reload-bindings-unchanged").join(FILE_NAME);
    // `left` pages nothing, so reading this document raises a diagnostic every time --
    // which is why the unchanged case must not project again: a reload with nothing to
    // adopt has nothing to report either.
    write(&path, "[keys]\nflip_keys = [\"minus\", \"left\"]\n");
    let (mut store, warnings) = ConfigStore::load_at(&path, 7);
    assert_eq!(rejected(&warnings), [String::from(UNROUTABLE_BINDING_CODE)]);
    let before = store.bindings();

    assert!(matches!(store.reload(), ReloadOutcome::Unchanged));

    assert_eq!(store.bindings(), before);
    assert_eq!(before.flip_keys, FlipSet::MINUS);
}

#[test]
fn test_reload_leaves_a_binding_table_a_caller_already_holds_untouched() {
    // The configuration layer's half of the rule that a reload never disturbs a
    // composition in progress: the table a component took before the reload is a copy,
    // and the reload replaces the store's value rather than the copy.
    let path = scratch("reload-bindings-snapshot").join(FILE_NAME);
    write(&path, "[keys]\nflip_keys = [\"minus\"]\n");
    let (mut store, _) = ConfigStore::load_at(&path, 7);
    let in_flight = store.bindings();

    write(&path, "[keys]\nflip_keys = [\"page_up\"]\n");
    assert!(matches!(store.reload(), ReloadOutcome::Updated { .. }));

    assert_eq!(
        in_flight.flip_keys,
        FlipSet::MINUS,
        "the table a component started with is not rewritten underneath it"
    );
    assert_eq!(store.bindings().flip_keys, FlipSet::PAGE_UP);
}

#[test]
fn test_store_bindings_match_the_configuration_in_force() {
    // The invariant the store exists to keep: the table it hands out is always the
    // projection of the configuration it hands out, at load and after every reload.
    let path = scratch("bindings-invariant").join(FILE_NAME);
    write(
        &path,
        "[keys]\nflip_keys = [\"minus\", \"left\"]\nhighlight_keys = [\"tab\", \"page_up\"]\n",
    );
    let (mut store, _) = ConfigStore::load_at(&path, 7);

    assert_eq!(store.bindings(), project_keys(&store.current().keys).0);

    write(
        &path,
        "[keys]\nflip_keys = [\"equal\"]\nhighlight_keys = [\"right\"]\n",
    );
    assert!(matches!(store.reload(), ReloadOutcome::Updated { .. }));

    assert_eq!(store.bindings(), project_keys(&store.current().keys).0);
}

#[test]
fn test_default_template_parses_to_the_built_in_defaults() {
    // The template is two things at once: the documentation of every key a user may
    // write, and the statement of the built-in defaults. A key added to one side and
    // forgotten on the other is caught here rather than by a user whose fresh
    // configuration does not behave the way the file in front of them says it does.
    let (config, warnings) = match Config::from_document(DEFAULT_CONFIG_TOML) {
        Ok(parsed) => parsed,
        Err(error) => panic!("the shipped template must be readable: {error}"),
    };

    assert_eq!(config, Config::default(), "the template is the defaults");
    assert!(
        rejected(&warnings).is_empty(),
        "no key of the template is unusable: {warnings:?}"
    );
    assert!(
        config.validate().is_empty(),
        "the template needs no repair: {:?}",
        config.validate()
    );
}

#[test]
fn test_load_reads_the_scheme_section_into_the_decode_settings() {
    let path = scratch("scheme-section").join(FILE_NAME);
    write(
        &path,
        "[scheme]\nscheme = \"XiaoHe\"\nshow_hint = false\nkeep_full_pinyin = false\n",
    );

    let (store, warnings) = ConfigStore::load_at(&path, 7);

    assert!(warnings.is_empty(), "every value is usable: {warnings:?}");
    assert_eq!(
        store.scheme(),
        (SchemeId::XIAOHE, false),
        "the layout is read in any case and the mixed-input switch with it"
    );
    assert_eq!(
        store.scheme(),
        store.current().scheme.decode_settings(),
        "the accessor is the projection of the configuration in force"
    );
}

#[test]
fn test_store_scheme_defaults_to_full_pinyin_with_no_document() {
    let path = scratch("scheme-default").join(FILE_NAME);

    let (store, warnings) = ConfigStore::load_at(&path, 7);

    assert!(warnings.is_empty(), "having no file yet is not a problem");
    assert_eq!(
        store.scheme(),
        (SchemeId::FULL, true),
        "a fresh install decodes full pinyin and still reads it inside a scheme"
    );
    assert_eq!(store.scheme(), store.current().scheme.decode_settings());
}

#[test]
fn test_load_repairs_an_unimplemented_scheme_before_the_accessor_sees_it() {
    // The boundary: a layout this build cannot compile. The section is repaired while
    // the configuration is adopted, so the accessor can never hand a caller a layout
    // that nothing decodes.
    let path = scratch("scheme-custom").join(FILE_NAME);
    write(
        &path,
        "[scheme]\nscheme = \"custom\"\nkeep_full_pinyin = false\n",
    );

    let (store, warnings) = ConfigStore::load_at(&path, 7);

    assert_eq!(rejected(&warnings), [String::from("scheme.scheme")]);
    assert_eq!(
        store.scheme(),
        (SchemeId::FULL, false),
        "the repaired layout answers full pinyin, the key beside it is kept"
    );
    assert_eq!(store.scheme(), store.current().scheme.decode_settings());
}

#[test]
fn test_reload_adopts_the_decode_settings_of_the_new_document() {
    let path = scratch("reload-scheme").join(FILE_NAME);
    write(&path, "[scheme]\nscheme = \"ziranma\"\n");
    let (mut store, _) = ConfigStore::load_at(&path, 7);
    assert_eq!(store.scheme(), (SchemeId::ZIRANMA, true));

    write(
        &path,
        "[scheme]\nscheme = \"sogou\"\nkeep_full_pinyin = false\n",
    );
    assert!(matches!(store.reload(), ReloadOutcome::Updated { .. }));

    assert_eq!(store.scheme(), (SchemeId::SOGOU, false));
    assert_eq!(
        store.scheme(),
        store.current().scheme.decode_settings(),
        "the settings follow the configuration the reload adopted"
    );
}

#[test]
fn test_reload_keeps_the_decode_settings_when_the_file_cannot_be_read() {
    let directory = scratch("reload-scheme-kept");
    let path = directory.join(FILE_NAME);
    write(
        &path,
        "[scheme]\nscheme = \"xiaohe\"\nkeep_full_pinyin = false\n",
    );
    let (mut store, _) = ConfigStore::load_at(&path, 7);
    let before = store.scheme();
    assert_eq!(before, (SchemeId::XIAOHE, false));

    // Gone: nothing to adopt, so the configuration in force -- and the decode settings
    // projected from it -- stay exactly as they were.
    assert!(fs::remove_file(&path).is_ok());
    assert_eq!(kept_by(&mut store), [String::from(DOCUMENT_KEY)]);
    assert_eq!(
        store.scheme(),
        before,
        "a kept configuration keeps its decode settings"
    );
}
