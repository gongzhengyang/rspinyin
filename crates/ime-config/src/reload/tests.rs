//! Unit tests for the configuration loader and the reload path.
//!
//! Responsibility: pin what reading and re-reading `config.toml` does to the filesystem --
//! the template a fresh install is given, the backup a corrupt file is moved to, and the
//! rule that a reload improves the configuration in force or leaves it alone.
//!
//! Boundaries: every test names the file it owns and passes the stamp a backup name
//! carries, so nothing here reads `$HOME` or the clock. The scratch directories live under
//! the system temp directory, one per test.

use super::*;
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
        "schema_version = 1\n\
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
    assert!(read(&path).starts_with("schema_version = 1"));
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
    assert!(warnings[0].to_string().contains("limit exceeded: 120"));
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
