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

use crate::crash::record::write_record;
use crate::crash::{CrashContext, CrashContextKey, CrashRecord};

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
    // The directory holds the active file plus `keep_files` rolled ones, which is the
    // 32 MiB the acceptance criterion bounds the whole set by.
    assert_eq!(
        (cfg.keep_files as u64 + 1) * rotation_bytes(&cfg),
        32 * MEBIBYTE,
        "the log directory is bounded at 32 MiB"
    );
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
    let state = Arc::new(RedactState::new(prepared.level, prepared.floor, None));
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

#[test]
fn test_prepare_announces_the_input_content_switch_without_recording_characters() {
    // The switch buys structure and never content, and it says so: a user who turned it
    // on has to be told what it did, or the promise that their input is never recorded
    // would be invisible to them.
    let dir = scratch_dir("announce");
    let cfg = DiagConfig {
        log_input_content: true,
        ..DiagConfig::new(dir.clone())
    };
    let prepared = Prepared::open(&cfg);
    let state = Arc::new(RedactState::new(prepared.level, prepared.floor, None));
    let subscriber = build_subscriber(&prepared, Arc::clone(&state));
    let _guard = tracing::subscriber::set_default(subscriber);

    prepared.announce();

    let planted = String::from("fixture-alpha");
    tracing::debug!(raw = %planted, dag_edges = 12, "segmentation built");

    let contents = fs::read_to_string(dir.join(LOG_FILE_NAME)).expect("the log is readable");
    assert!(
        contents.contains("does not record what you type"),
        "the switch announces itself:\n{contents}"
    );
    assert!(
        !contents.contains("fixture-alpha"),
        "the switch cannot turn the recording of characters on:\n{contents}"
    );
    assert!(contents.contains("<redacted:len=13>"), "{contents}");
    // What it does add is the structural detail the notice promises.
    assert!(contents.contains("dag_edges=12"), "{contents}");
}

#[test]
fn test_stderr_fallback_admits_only_warn_and_above() {
    // The fallback's own sink is the process's stderr, which a test cannot read back.
    // What the fallback decides is the level it resolved to, so that level is applied
    // here to a sink the test can read: the same subscriber, one writable sink.
    let fallback = Prepared::open(&DiagConfig::new(unwritable_log_dir("fallback-level")));
    assert!(matches!(fallback.sink, Sink::Stderr));
    assert_eq!(fallback.level, LevelFilter::WARN);

    let dir = scratch_dir("fallback-level-log");
    let cfg = DiagConfig {
        level: fallback.level,
        ..DiagConfig::new(dir.clone())
    };
    let prepared = Prepared::open(&cfg);
    let state = Arc::new(RedactState::new(prepared.level, prepared.floor, None));
    let subscriber = build_subscriber(&prepared, Arc::clone(&state));
    let _guard = tracing::subscriber::set_default(subscriber);

    tracing::info!(candidate_count = 3, "candidates built");
    tracing::warn!(code = "data/readonly-mode", "the store is read-only");

    let contents = fs::read_to_string(dir.join(LOG_FILE_NAME)).expect("the log is readable");
    assert!(
        !contents.contains("candidates built"),
        "a log shared with the host is not flooded with per-keystroke detail:\n{contents}"
    );
    assert!(contents.contains("the store is read-only"), "{contents}");
}

#[test]
fn test_zero_trace_scan_finds_no_input_content_in_the_log_or_the_crash_record() {
    // The assertion the privacy rule rests on: a password-box session that also crashes
    // leaves neither a log file nor a crash record quoting what was typed. The fixture is
    // a synthetic token, never anything that reads like a keystroke.
    let planted = "fixture-alpha-9c3f";
    let planted_len = planted.chars().count();
    let dir = scratch_dir("zero-trace");
    let cfg = DiagConfig::new(dir.clone());
    let prepared = Prepared::open(&cfg);
    let state = Arc::new(RedactState::new(prepared.level, prepared.floor, None));
    let subscriber = build_subscriber(&prepared, Arc::clone(&state));
    let _guard = tracing::subscriber::set_default(subscriber);

    // A password box: the host flags the context, and the session is downgraded from the
    // event that carried the flag onwards.
    tracing::info!(
        session = 11u64,
        app = 0x8f3a_2c1du64,
        password = true,
        "session: start"
    );
    tracing::info!(session = 11u64, raw = %planted, raw_len = 18u64, "key handled");
    // A session that is not sensitive withholds the value through the denylist instead.
    tracing::info!(
        session = 12u64,
        preedit = %planted,
        raw_len = 18u64,
        candidate_count = 4,
        "candidates built"
    );
    // And a message that quotes one is scrubbed, since free text reaches no field rule.
    tracing::warn!("decode gave up: raw={planted}");

    let mut context = CrashContext::new();
    context.insert_count(CrashContextKey::RawLen, 18);
    let record = CrashRecord {
        timestamp_unix_ms: 1_759_142_112_345,
        thread_name: String::from("ui"),
        thread_id: 0x1f3a_2c1d,
        location: Some(String::from("ime-ui/src/ui_thread.rs:214:9")),
        payload: format!("on_key_event: assertion failed: raw={planted}"),
        backtrace: String::from("   0: frame\n   1: frame\n"),
        context,
    };
    let crash_path = write_record(&dir.join("crash"), &record).expect("the record is writable");

    let log = fs::read_to_string(dir.join(LOG_FILE_NAME)).expect("the log is readable");
    let crash_text = fs::read_to_string(&crash_path).expect("the record is readable");
    for text in [&log, &crash_text] {
        assert!(
            !text.contains(planted),
            "the planted value reached a file:\n{text}"
        );
    }

    // What the diagnostics substitute for the content is still there, so the files a
    // crash leaves behind remain worth reading.
    let placeholder = format!("<redacted:len={planted_len}>");
    let scrubbed_pair = format!("raw={placeholder}");
    assert!(log.contains("session=redacted"), "{log}");
    assert!(log.contains(placeholder.as_str()), "{log}");
    assert!(log.contains("candidate_count=4"), "{log}");
    assert!(crash_text.contains(scrubbed_pair.as_str()), "{crash_text}");
    assert!(crash_text.contains("raw_len=18"), "{crash_text}");
    // The sensitive session records neither the value nor its length: the length of a
    // password is a secret of its own, so the only line carrying one is the other session's.
    assert_eq!(
        log.lines().filter(|line| line.contains("raw_len")).count(),
        1,
        "a sensitive session records no input length:\n{log}"
    );
}
