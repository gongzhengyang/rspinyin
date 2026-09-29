//! Unit tests for the subscriber, the rolling file sink and the level policy.
//!
//! Responsibility: pin the size at which the active file rolls, how many rolled files are
//! kept, the modes the log directory and the files carry, and the two degradations -- the
//! `stderr` fallback with its `warn` floor, and the input-content switch that raises the
//! level without turning the recording of characters on.
//!
//! Boundaries: each test owns a scratch directory under the workspace `target/`, named from
//! `CARGO_MANIFEST_DIR` and the process id, so nothing here reads an environment variable or
//! the clock. Only one test installs the global subscriber, and it asserts the
//! once-per-process misuse rather than assuming another test left the slot free.

use std::io::Write as _;
use std::sync::atomic::AtomicUsize;

use super::*;

/// A scratch directory under the workspace `target/`. The path comes from
/// `CARGO_MANIFEST_DIR`, which the compiler resolves, so no test reads an
/// environment variable; the process id plus a counter keeps two runs apart.
fn scratch_dir(tag: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/diag-scratch")
        .join(format!("{tag}-{}-{unique}", std::process::id()));
    // A leftover from an interrupted run would otherwise be read back as if it
    // were this run's output.
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// The permission bits of `path`.
#[cfg(unix)]
fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;

    fs::metadata(path)
        .expect("the path exists")
        .permissions()
        .mode()
        & 0o777
}

/// Names of the files in `dir`, sorted.
fn file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("the directory is readable")
        .map(|entry| {
            entry
                .expect("readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// Writes `count` five-byte lines through `writer`.
fn write_lines(writer: &mut RotatingWriter, count: usize) {
    for index in 0..count {
        let line = format!("{index:04}\n");
        writer
            .write_all(line.as_bytes())
            .expect("the log file is writable");
    }
}

/// A log directory whose creation fails for every user, root included: its parent
/// is a regular file, so creating a directory under it is `NotADirectory`
/// regardless of the privileges the test runs with.
fn unwritable_log_dir(tag: &str) -> PathBuf {
    let dir = scratch_dir(tag);
    fs::create_dir_all(&dir).expect("the scratch directory is creatable");
    let blocker = dir.join("not-a-directory");
    fs::write(&blocker, b"occupied\n").expect("the blocker is writable");
    blocker.join("logs")
}

#[test]
fn test_rotation_bytes_counts_mebibytes() {
    // The design counts megabytes; the unit is a mebibyte, and this is the limit
    // the writer compares against. `DiagConfig::new`'s defaults are pinned by its
    // own doctest.
    let cfg = DiagConfig::new("/scratch/rspinyin/logs");
    assert_eq!(rotation_bytes(&cfg), 8 * MEBIBYTE);
    assert!(!cfg.log_input_content);
}

#[test]
fn test_rotating_file_rolls_over_and_bounds_the_history() {
    let dir = scratch_dir("rollover");
    // A 20-byte limit holds four five-byte lines, so 30 lines roll the file over
    // seven times.
    let mut writer =
        RotatingWriter::open(&dir, LOG_FILE_NAME, 20, 3).expect("the scratch dir is writable");
    write_lines(&mut writer, 30);

    let mut expected = vec![String::from(LOG_FILE_NAME)];
    expected.extend((1..=3).map(|index| format!("rspinyin.log.{index}")));
    assert_eq!(
        file_names(&dir),
        expected,
        "the active file plus keep_files rolled ones"
    );
    let read = |name: &str| fs::read_to_string(dir.join(name)).expect("the log is readable");
    let current = read(LOG_FILE_NAME);
    assert!(current.ends_with("0029\n"), "{current}");
    assert!(
        current.contains("0028") && !current.contains("0027"),
        "{current}"
    );
    let newest = read("rspinyin.log.1");
    assert!(
        newest.contains("0027") && !newest.contains("0023"),
        "{newest}"
    );

    // The oldest content is gone and every file stayed inside the budget: that is
    // what keeps the whole set bounded.
    for name in file_names(&dir) {
        let text = read(&name);
        assert!(
            !text.contains("0000"),
            "{name} still holds the oldest lines"
        );
        assert!(text.len() <= 20, "{name} is {} bytes", text.len());
    }
}

#[test]
fn test_rotating_file_rolls_only_when_a_limit_is_in_force() {
    // `keep_files = 0` keeps the active file alone and starts it over.
    let bare = scratch_dir("no-history");
    let mut writer =
        RotatingWriter::open(&bare, LOG_FILE_NAME, 20, 0).expect("the scratch dir is writable");
    write_lines(&mut writer, 10);
    assert_eq!(file_names(&bare), [LOG_FILE_NAME]);
    let current = fs::read_to_string(bare.join(LOG_FILE_NAME)).expect("the log is readable");
    assert!(
        current.ends_with("0009\n") && !current.contains("0000"),
        "{current}"
    );

    // A zero limit disables rolling entirely, so nothing is ever lost.
    let unlimited = scratch_dir("no-limit");
    let mut writer =
        RotatingWriter::open(&unlimited, LOG_FILE_NAME, 0, 3).expect("the scratch dir is writable");
    write_lines(&mut writer, 10);
    assert_eq!(file_names(&unlimited), [LOG_FILE_NAME]);
    let current = fs::read_to_string(unlimited.join(LOG_FILE_NAME)).expect("the log is readable");
    assert!(
        current.ends_with("0009\n") && current.contains("0000"),
        "{current}"
    );

    // A log left by an earlier run is appended to, and its size counts towards the
    // limit rather than being forgotten.
    let resumed = scratch_dir("append");
    let path = resumed.join(LOG_FILE_NAME);
    fs::create_dir_all(&resumed).expect("the scratch directory is creatable");
    fs::write(&path, "previous run\n").expect("the log is writable");
    let mut writer = RotatingWriter::open(&resumed, LOG_FILE_NAME, 1024, 3)
        .expect("the scratch dir is writable");
    write_lines(&mut writer, 1);
    let current = fs::read_to_string(&path).expect("the log is readable");
    assert!(current.starts_with("previous run\n"), "{current}");
    assert!(current.ends_with("0000\n"), "{current}");
}

#[cfg(unix)]
#[test]
fn test_log_files_and_directory_are_private() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = scratch_dir("modes");
    let mut writer =
        RotatingWriter::open(&dir, LOG_FILE_NAME, 20, 3).expect("the scratch dir is writable");
    write_lines(&mut writer, 10);

    assert_eq!(mode_of(&dir), 0o700, "the log directory is private");
    assert_eq!(mode_of(&dir.join(LOG_FILE_NAME)), 0o600);
    // Rolled files are created by the same path as the active one, so they inherit
    // the same mode.
    assert_eq!(mode_of(&dir.join("rspinyin.log.1")), 0o600);

    // A file left world-readable by an earlier version is tightened on open.
    let path = dir.join(LOG_FILE_NAME);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("mode is settable");
    RotatingWriter::open(&dir, LOG_FILE_NAME, 1024, 3).expect("the scratch dir is writable");
    assert_eq!(mode_of(&path), 0o600);
}

#[test]
fn test_prepare_resolves_the_level_and_the_notices() {
    // The input-content switch raises verbosity to `debug` and announces itself.
    let dir = scratch_dir("input-content");
    let cfg = DiagConfig {
        log_input_content: true,
        ..DiagConfig::new(dir.clone())
    };
    let prepared = Prepared::open(&cfg);
    assert_eq!(prepared.level, LevelFilter::DEBUG);
    assert_eq!(prepared.notices, [Notice::InputContentRequested]);
    assert_eq!(prepared.log_path, Some(dir.join(LOG_FILE_NAME)));
    assert!(matches!(prepared.sink, Sink::File(_)));

    // An explicit `off` means "no diagnostics at all", and a switch asking for more
    // detail does not override it.
    let off = DiagConfig {
        level: LevelFilter::OFF,
        ..cfg
    };
    assert_eq!(Prepared::open(&off).level, LevelFilter::OFF);
}

#[test]
fn test_prepare_falls_back_to_stderr_when_the_log_directory_cannot_be_created() {
    let cfg = DiagConfig::new(unwritable_log_dir("fallback"));
    let prepared = Prepared::open(&cfg);

    assert!(matches!(prepared.sink, Sink::Stderr));
    assert_eq!(prepared.log_path, None);
    // The host's log is shared, so only `warn` and above are written.
    assert_eq!(prepared.level, LevelFilter::WARN);
    assert!(matches!(
        prepared.notices.as_slice(),
        [Notice::StderrFallback { .. }]
    ));

    // A stricter configured level is kept: the fallback never lowers the bar.
    let quiet = DiagConfig {
        level: LevelFilter::ERROR,
        ..cfg
    };
    assert_eq!(Prepared::open(&quiet).level, LevelFilter::ERROR);
}

#[test]
fn test_subscriber_writes_redacted_lines_to_the_log_file() {
    let dir = scratch_dir("end-to-end");
    let cfg = DiagConfig::new(dir.clone());
    let prepared = Prepared::open(&cfg);
    assert!(matches!(prepared.sink, Sink::File(_)));
    let state = Arc::new(RedactState::new(prepared.level, None));
    let subscriber = build_subscriber(&prepared, Arc::clone(&state));
    let _guard = tracing::subscriber::set_default(subscriber);

    let raw = String::from("mima");
    tracing::info!(session = 4u64, raw = %raw, candidate_count = 2, "candidates built");

    let contents = fs::read_to_string(dir.join(LOG_FILE_NAME)).expect("the log is readable");
    assert!(contents.contains("<redacted:len=4>"), "{contents}");
    assert!(!contents.contains("mima"), "{contents}");
    assert!(contents.contains("candidate_count=2"), "{contents}");
    assert!(contents.contains("session=4"), "{contents}");
}

#[test]
fn test_init_logging_installs_once_and_reports_the_misuse() {
    let dir = scratch_dir("init");
    let cfg = DiagConfig::new(dir.clone());
    let handle = init_logging(&cfg).expect("the first call installs the subscriber");
    let path = dir.join(LOG_FILE_NAME);
    assert_eq!(handle.log_path(), Some(path.as_path()));
    assert_eq!(handle.level(), LevelFilter::INFO);

    tracing::info!(candidate_count = 3, "candidates built");

    // The second call reports the misuse and leaves the subscriber alone.
    let err = init_logging(&cfg).expect_err("the subscriber is installed once per process");
    assert!(matches!(err, ImeError::ConfigInvalid { .. }));
    let message = err.to_string();
    assert!(message.starts_with("config/invalid"), "{message}");
    assert!(message.contains("diag.log"), "{message}");

    assert!(path.is_file(), "the log file was created");
    let contents = fs::read_to_string(&path).expect("the log file is readable");
    assert!(contents.contains("candidate_count=3"), "{contents}");
    assert!(contents.contains("INFO"), "{contents}");
    #[cfg(unix)]
    assert_eq!(mode_of(&path), 0o600);
}
