use std::env;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process;

use ime_types::ImeError;

use super::*;
use crate::reload::{DEFAULT_CONFIG_TOML, FILE_NAME};

/// A scratch directory for one test, named after the test and the process id.
fn scratch(name: &str) -> PathBuf {
    let directory = env::temp_dir().join(format!(
        "rspinyin-ime-config-writeback-{}-{name}",
        process::id()
    ));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).expect("creating the scratch directory");
    directory
}

/// Writes `text` to `path`.
fn write_text(path: &Path, text: &str) {
    fs::write(path, text).expect("writing the test file");
}

/// The text of `path`.
fn read_text(path: &Path) -> String {
    fs::read_to_string(path).expect("reading the test file")
}

/// A `[keys]` section from its parts.
fn bindings(
    digit_zero: DigitZero,
    enter_commit_raw: bool,
    flip: &[KeyName],
    highlight: &[KeyName],
) -> KeysConfig {
    KeysConfig {
        digit_zero,
        enter_commit_raw,
        flip_keys: flip.to_vec(),
        highlight_keys: highlight.to_vec(),
    }
}

/// The configuration `text` parses to, requiring that it parses with no diagnostic.
fn parsed(text: &str) -> Config {
    let (config, warnings) = Config::from_document(text).expect("the document parses");
    assert!(warnings.is_empty(), "no diagnostics: {warnings:?}");
    config
}

/// A document with comments, blank lines and two sections around the `[keys]` one.
const PRESERVED_BEFORE: &str = concat!(
    "# header comment\n",
    "schema_version = 2\n",
    "\n",
    "[engine]\n",
    "# engine comment\n",
    "max_raw_len = 32\n",
    "\n",
);

/// The text the rewrite must keep behind the `[keys]` region.
const PRESERVED_AFTER: &str = "\n[theme]\n# theme comment\nscheme = \"dark\"\n";

/// The `[keys]` block the rendered bindings must come out as, spelled here rather
/// than taken from the module so the format is pinned, not assumed.
const PRESERVED_BLOCK: &str = concat!(
    "[keys]\n",
    "digit_zero = \"flip\"\n",
    "enter_commit_raw = true\n",
    "flip_keys = [\"minus\", \"equal\"]\n",
    "highlight_keys = [\"tab\", \"shift_tab\"]\n",
);

#[test]
fn test_write_keys_rewrites_only_the_keys_table_and_preserves_the_rest() {
    let path = scratch("preserve").join(FILE_NAME);
    let new = bindings(
        DigitZero::Flip,
        true,
        &[KeyName::Minus, KeyName::Equal],
        &[KeyName::Tab, KeyName::ShiftTab],
    );
    // What the caller loaded: the document with its own `[keys]` region intact.
    let original =
        format!("{PRESERVED_BEFORE}[keys]\ndigit_zero = \"passthrough\"\n{PRESERVED_AFTER}");
    write_text(&path, &original);
    let previous = parsed(&original).keys;

    write_keys(&path, &previous, &new).expect("the write succeeds");

    let written = read_text(&path);
    assert_eq!(
        written,
        format!("{PRESERVED_BEFORE}{PRESERVED_BLOCK}{PRESERVED_AFTER}")
    );
    assert_eq!(parsed(&written).keys, new);
}

#[test]
fn test_write_keys_without_a_keys_table_appends_one_the_parser_accepts() {
    let path = scratch("append").join(FILE_NAME);
    let original = "# my settings\n\n[engine]\nmax_raw_len = 32\n";
    write_text(&path, original);
    let new = bindings(
        DigitZero::Flip,
        false,
        &[KeyName::PageUp, KeyName::PageDown],
        &[KeyName::Up, KeyName::Down],
    );

    // The file declares no `[keys]` table, so the values in force were the defaults.
    write_keys(&path, &Config::default().keys, &new).expect("the write succeeds");

    let written = read_text(&path);
    assert!(written.starts_with(original), "the file is kept verbatim");
    assert!(
        written[original.len()..].starts_with("\n[keys]"),
        "behind a blank line"
    );
    assert_eq!(parsed(&written).keys, new);
}

#[test]
fn test_write_keys_region_changed_after_the_load_reports_a_race_and_writes_nothing() {
    let path = scratch("race").join(FILE_NAME);
    let loaded = "[keys]\ndigit_zero = \"passthrough\"\nflip_keys = [\"minus\"]\n";
    write_text(&path, loaded);
    let previous = parsed(loaded).keys;
    let new = bindings(DigitZero::Flip, true, &[KeyName::Equal], &[KeyName::Tab]);
    // Another writer replaces the section between the load and the write.
    let edited = "[keys]\ndigit_zero = \"flip\"\nflip_keys = [\"equal\"]\n";
    write_text(&path, edited);

    let result = write_keys(&path, &previous, &new);

    match result {
        Err(WritebackError::Race { reason }) => assert!(!reason.is_empty()),
        other => panic!("expected a race, got {other:?}"),
    }
    assert_eq!(read_text(&path), edited, "the file is untouched");
}

#[test]
fn test_write_keys_same_bindings_written_twice_leaves_the_file_byte_identical() {
    let path = scratch("idempotent").join(FILE_NAME);
    write_text(&path, DEFAULT_CONFIG_TOML);
    let previous = parsed(DEFAULT_CONFIG_TOML).keys;
    let new = bindings(
        DigitZero::Flip,
        true,
        &[KeyName::Up, KeyName::Down],
        &[KeyName::Left, KeyName::Right],
    );

    write_keys(&path, &previous, &new).expect("the first write succeeds");
    let once = read_text(&path);
    // The caller's loaded values after the first write are the ones it wrote.
    write_keys(&path, &new, &new).expect("the second write succeeds");

    assert_eq!(once, read_text(&path), "the second write changes nothing");
    assert_eq!(parsed(&read_text(&path)).keys, new);
}

#[test]
fn test_write_keys_unwritable_directory_returns_the_typed_error_without_panicking() {
    let directory = scratch("unwritable");
    let path = directory.join(FILE_NAME);
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o500))
        .expect("narrowing the scratch directory");
    // A process that owns the directory -- root, or a supervisor -- sees the mode
    // but not the restriction; the probe skips honestly where it cannot be tested.
    if fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .is_ok()
    {
        restore(&directory);
        return;
    }
    let new = bindings(DigitZero::Flip, false, &[KeyName::Minus], &[KeyName::Tab]);

    let result = write_keys(&path, &Config::default().keys, &new);

    restore(&directory);
    match result {
        Err(WritebackError::Io(_)) => {}
        other => panic!("expected an io error, got {other:?}"),
    }
    assert!(!path.exists(), "nothing is created where nothing can be");
}

/// Hands a narrowed scratch directory its modes back, so its cleanup can remove it.
fn restore(directory: &Path) {
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .expect("restoring the scratch directory");
}

#[test]
fn test_write_keys_created_file_and_directory_carry_the_private_modes() {
    let directory = scratch("modes");
    let path = directory.join("nested").join(FILE_NAME);
    let new = bindings(DigitZero::Passthrough, true, &[KeyName::Tab], &[]);

    write_keys(&path, &Config::default().keys, &new).expect("the write succeeds");

    let file_mode = fs::metadata(&path)
        .expect("the file exists")
        .permissions()
        .mode();
    assert_eq!(file_mode & 0o777, 0o600);
    let parent = path.parent().expect("the file has a parent");
    let dir_mode = fs::metadata(parent)
        .expect("the directory exists")
        .permissions()
        .mode();
    assert_eq!(dir_mode & 0o777, 0o700);
    assert_eq!(parsed(&read_text(&path)).keys, new);
}

#[test]
fn test_write_errors_convert_to_ime_error_with_the_stable_codes() {
    let path = scratch("codes").join(FILE_NAME);
    let loaded = "[keys]\ndigit_zero = \"passthrough\"\n";
    write_text(&path, loaded);
    let previous = parsed(loaded).keys;
    let new = bindings(DigitZero::Flip, false, &[], &[]);
    write_text(&path, "[keys]\ndigit_zero = \"flip\"\n");

    let raced = ImeError::from(write_keys(&path, &previous, &new).expect_err("races"));
    assert!(
        raced
            .to_string()
            .starts_with("config/invalid: config/writeback-race"),
        "{raced}"
    );

    // A file sitting where the configuration directory belongs cannot hold the file.
    let blocked = scratch("codes-io").join("blocker");
    write_text(&blocked, "not a directory");
    let failed = write_keys(&blocked.join(FILE_NAME), &Config::default().keys, &new)
        .expect_err("cannot be written");
    let converted = ImeError::from(failed);
    assert!(
        converted
            .to_string()
            .starts_with("config/invalid: config ("),
        "{converted}"
    );

    let refused = WritebackError::Refused {
        reason: String::from("would not parse"),
    };
    assert!(
        ImeError::from(refused)
            .to_string()
            .starts_with("config/invalid: config ("),
        "refused"
    );
}

#[test]
fn test_write_keys_string_value_shaped_like_a_header_races_and_writes_nothing() {
    let path = scratch("string-value").join(FILE_NAME);
    // The `[keys]` line is the *value* of `phrases.file`; the scanner reads it as one.
    let original = "[phrases]\nfile = \"\"\"\n[keys]\n\"\"\"\n";
    write_text(&path, original);
    let previous = parsed(original).keys;
    let new = bindings(DigitZero::Flip, false, &[], &[]);

    let result = write_keys(&path, &previous, &new);

    match result {
        Err(WritebackError::Race { .. }) => {}
        other => panic!("expected a race, got {other:?}"),
    }
    assert_eq!(read_text(&path), original, "the file is untouched");
}

#[test]
fn test_digit_zero_spelling_round_trips_through_the_closed_set() {
    for value in [DigitZero::Passthrough, DigitZero::Flip] {
        let spelling = digit_zero_spelling(value);
        assert_eq!(DigitZero::try_from(spelling).expect("in the set"), value);
    }
}
