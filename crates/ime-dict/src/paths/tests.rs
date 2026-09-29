//! Unit tests for the XDG layout and the modes it enforces.
//!
//! Responsibility: pin the resolution of the base directories, the layout derived from
//! them, and what the preparation pass does to every path it owns -- creating a directory
//! private, narrowing a loose mode, refusing a symbolic link, and degrading to read-only
//! instead of failing.
//!
//! Boundaries: every test owns a scratch root under the system temp directory and injects
//! the base directories it uses, so nothing here reads the real environment or touches the
//! user's own data. The time budget is asserted on the best of several attempts, because
//! the suite runs in parallel with the rest of the workspace.

use std::os::unix::fs::symlink;
use std::time::{Duration, Instant};

use super::*;

/// A lookup over a fixed table, so that no test reads the real environment.
fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
    move |name| {
        let found = pairs.iter().find(|(key, _)| *key == name);
        found.map(|(_, value)| OsString::from(*value))
    }
}

/// A per-test root under the system temp directory.
fn temp_root(label: &str) -> PathBuf {
    let name = format!("rspinyin-paths-{}-{label}", std::process::id());
    let root = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("creating the test root");
    root
}

/// Base directories a test owns, both already present.
fn bases(label: &str) -> BaseDirs {
    let root = temp_root(label);
    let config_home = root.join("config");
    let data_home = root.join("data");
    std::fs::create_dir_all(&config_home).expect("creating the config base");
    std::fs::create_dir_all(&data_home).expect("creating the data base");
    BaseDirs {
        config_home,
        data_home,
    }
}

/// The permission bits of `path`.
fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path).expect("mode").permissions().mode() & 0o777
}

/// Sets the permission bits of `path`.
fn set_mode(path: &Path, mode: u32) {
    let permissions = std::fs::Permissions::from_mode(mode);
    std::fs::set_permissions(path, permissions).expect("setting the mode");
}

/// Whether the pass reported `code` for `path`.
fn reported(paths: &Paths, code: &str, path: &Path) -> bool {
    paths
        .notices()
        .iter()
        .any(|n| n.code == code && n.path == path)
}

#[test]
fn test_base_dirs_from_lookup_prefers_the_xdg_variables() {
    let table = [
        ("XDG_CONFIG_HOME", "/opt/conf"),
        ("XDG_DATA_HOME", "/opt/data"),
        ("HOME", "/home/tester"),
    ];
    let dirs = BaseDirs::from_lookup(lookup(&table)).expect("resolving the bases");
    assert_eq!(dirs.config_home, PathBuf::from("/opt/conf"));
    assert_eq!(dirs.data_home, PathBuf::from("/opt/data"));
}

#[test]
fn test_base_dirs_from_lookup_falls_back_to_home() {
    let dirs =
        BaseDirs::from_lookup(lookup(&[("HOME", "/home/tester")])).expect("resolving the bases");
    assert_eq!(dirs.config_home, PathBuf::from("/home/tester/.config"));
    assert_eq!(dirs.data_home, PathBuf::from("/home/tester/.local/share"));
}

#[test]
fn test_base_dirs_from_lookup_ignores_a_relative_value() {
    let table = [("XDG_DATA_HOME", "data"), ("HOME", "/home/tester")];
    let dirs = BaseDirs::from_lookup(lookup(&table)).expect("resolving the bases");
    assert_eq!(
        dirs.data_home,
        PathBuf::from("/home/tester/.local/share"),
        "a relative base must not become the data directory"
    );
    let relative_home = BaseDirs::from_lookup(lookup(&[("HOME", "home/tester")]));
    assert!(
        matches!(relative_home, Err(ImeError::DataReadonly { .. })),
        "a relative HOME would put the data directory under the working directory"
    );
}

#[test]
fn test_base_dirs_from_lookup_without_home_enters_read_only_mode() {
    let result = BaseDirs::from_lookup(lookup(&[]));
    assert!(
        matches!(result, Err(ImeError::DataReadonly { .. })),
        "no base directory can be resolved without XDG variables or HOME"
    );
    assert!(
        is_readonly_mode(),
        "the failure is detected here, so the flag is set here"
    );
}

#[test]
fn test_paths_from_bases_lays_out_under_the_injected_bases() {
    let bases = bases("layout");
    let paths = Paths::from_bases(&bases).expect("deriving the layout");
    assert_eq!(paths.config_dir, bases.config_home.join("rspinyin"));
    assert_eq!(paths.config_file, paths.config_dir.join("config.toml"));
    assert_eq!(paths.data_dir, bases.data_home.join("rspinyin"));
    assert_eq!(paths.user_db, paths.data_dir.join("user.redb"));
    assert_eq!(paths.log_dir, paths.data_dir.join("logs"));
    assert_eq!(paths.crash_dir, paths.data_dir.join("crash"));
    assert_eq!(paths.takeover, paths.data_dir.join("ui_takeover.json"));
    assert!(!paths.is_readonly(), "a fresh layout starts writable");
    assert!(paths.notices().is_empty(), "{:?}", paths.notices());
}

#[test]
fn test_paths_from_bases_rejects_a_path_past_the_limit() {
    let bases = BaseDirs {
        config_home: PathBuf::from("/tmp"),
        data_home: PathBuf::from(format!("/{}", "x".repeat(MAX_PATH_BYTES))),
    };
    let result = Paths::from_bases(&bases);
    assert!(
        matches!(result, Err(ImeError::DataReadonly { .. })),
        "an over-long layout degrades instead of panicking"
    );
}

#[test]
fn test_ensure_dirs_in_creates_every_directory_private() {
    let bases = bases("create");
    let paths = ensure_dirs_in(&bases).expect("preparing the layout");
    let dirs = [
        &paths.config_dir,
        &paths.data_dir,
        &paths.log_dir,
        &paths.crash_dir,
    ];
    for dir in dirs {
        assert!(dir.is_dir(), "{dir:?} exists");
        assert_eq!(mode_of(dir), DIR_MODE, "{dir:?} is private");
    }
    assert!(!paths.is_readonly());
    assert!(paths.notices().is_empty(), "{:?}", paths.notices());
}

#[test]
fn test_ensure_dirs_in_leaves_the_base_directories_alone() {
    let bases = bases("base-mode");
    set_mode(&bases.config_home, 0o755);
    set_mode(&bases.data_home, 0o755);
    let paths = ensure_dirs_in(&bases).expect("preparing the layout");
    assert!(!paths.is_readonly());
    assert_eq!(mode_of(&bases.config_home), 0o755, "the user's mode stands");
    assert_eq!(mode_of(&bases.data_home), 0o755, "the user's mode stands");
}

#[test]
fn test_ensure_dirs_in_tightens_a_loose_user_db() {
    let bases = bases("loose-file");
    let paths = Paths::from_bases(&bases).expect("deriving the layout");
    std::fs::create_dir_all(&paths.data_dir).expect("creating the data directory");
    std::fs::write(&paths.user_db, b"stale").expect("placing the store");
    set_mode(&paths.user_db, 0o644);

    let prepared = ensure_dirs_in(&bases).expect("preparing the layout");
    assert_eq!(mode_of(&paths.user_db), FILE_MODE, "the store is narrowed");
    assert!(reported(&prepared, PERMS_FIXED_CODE, &paths.user_db));
}

#[test]
fn test_ensure_dirs_in_tightens_a_loose_directory() {
    let bases = bases("loose-dir");
    let paths = Paths::from_bases(&bases).expect("deriving the layout");
    std::fs::create_dir_all(&paths.data_dir).expect("creating the data directory");
    set_mode(&paths.data_dir, 0o755);

    let prepared = ensure_dirs_in(&bases).expect("preparing the layout");
    assert_eq!(
        mode_of(&paths.data_dir),
        DIR_MODE,
        "the directory is narrowed"
    );
    assert!(reported(&prepared, PERMS_FIXED_CODE, &paths.data_dir));
    assert!(!prepared.is_readonly(), "narrowing a mode is not a failure");
}

#[test]
fn test_ensure_dirs_in_refuses_a_symlinked_data_directory() {
    let bases = bases("symlink-dir");
    let paths = Paths::from_bases(&bases).expect("deriving the layout");
    let elsewhere = bases.data_home.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("creating the link target");
    symlink(&elsewhere, &paths.data_dir).expect("placing the link");

    let prepared = ensure_dirs_in(&bases).expect("the layout is still handed out");
    assert!(prepared.is_readonly(), "a linked data directory is refused");
    assert!(reported(&prepared, PATH_SYMLINK_CODE, &paths.data_dir));
    let link = std::fs::symlink_metadata(&paths.data_dir).expect("reading the link");
    assert!(
        link.file_type().is_symlink(),
        "the link is left where it is"
    );
    assert!(
        !elsewhere.join("logs").exists() && !elsewhere.join("user.redb").exists(),
        "nothing is created through the link"
    );
}

#[test]
fn test_ensure_dirs_in_refuses_a_symlinked_user_db() {
    let bases = bases("symlink-file");
    let paths = Paths::from_bases(&bases).expect("deriving the layout");
    std::fs::create_dir_all(&paths.data_dir).expect("creating the data directory");
    let elsewhere = bases.data_home.join("elsewhere.redb");
    std::fs::write(&elsewhere, b"stale").expect("placing the link target");
    set_mode(&elsewhere, 0o644);
    symlink(&elsewhere, &paths.user_db).expect("placing the link");

    let prepared = ensure_dirs_in(&bases).expect("the layout is still handed out");
    assert!(prepared.is_readonly(), "a store behind a link is refused");
    assert!(reported(&prepared, PATH_SYMLINK_CODE, &paths.user_db));
    assert_eq!(mode_of(&elsewhere), 0o644, "the target is left untouched");
}

#[test]
fn test_ensure_dirs_in_degrades_when_a_base_directory_is_a_file() {
    let root = temp_root("base-is-file");
    let config_home = root.join("config");
    std::fs::create_dir_all(&config_home).expect("creating the config base");
    let data_home = root.join("data");
    std::fs::write(&data_home, b"not a directory").expect("taking the base's place");
    let bases = BaseDirs {
        config_home,
        data_home,
    };

    let prepared = ensure_dirs_in(&bases).expect("the layout is still handed out");
    assert!(
        prepared.is_readonly(),
        "an unusable data base stops the writes"
    );
    assert!(reported(&prepared, READONLY_CODE, &bases.data_home));
    assert!(
        prepared.config_dir.is_dir(),
        "the other half is still prepared"
    );
    assert!(!prepared.data_dir.exists(), "nothing is created under it");
}

#[test]
fn test_ensure_dirs_in_degrades_when_a_data_directory_is_a_file() {
    let bases = bases("dir-is-file");
    let paths = Paths::from_bases(&bases).expect("deriving the layout");
    std::fs::create_dir_all(&paths.data_dir).expect("creating the data directory");
    std::fs::write(&paths.log_dir, b"not a directory").expect("taking its place");

    let prepared = ensure_dirs_in(&bases).expect("the layout is still handed out");
    assert!(prepared.is_readonly());
    assert!(reported(&prepared, READONLY_CODE, &paths.log_dir));
    assert!(paths.crash_dir.is_dir(), "the rest is still prepared");
}

#[test]
fn test_create_private_creates_a_file_with_the_owner_only_mode() {
    let root = temp_root("create-file");
    let file = root.join("config.toml");
    let handle = create_private(&file).expect("creating the file");
    assert_eq!(
        mode_of(&file),
        FILE_MODE,
        "created private, not narrowed later"
    );
    drop(handle);
    let again = create_private(&file).expect("reopening the file");
    assert_eq!(
        mode_of(&file),
        FILE_MODE,
        "a mode that matches is left alone"
    );
    drop(again);
}

#[test]
fn test_create_private_tightens_an_existing_file() {
    let root = temp_root("create-existing");
    let file = root.join("config.toml");
    std::fs::write(&file, b"max_per_row = 9").expect("placing the file");
    set_mode(&file, 0o644);

    let _handle = create_private(&file).expect("opening the file");
    assert_eq!(mode_of(&file), FILE_MODE, "an existing file is narrowed");
    let text = std::fs::read(&file).expect("reading the file");
    assert_eq!(
        text, b"max_per_row = 9",
        "opening a file does not truncate it"
    );
}

#[test]
fn test_chmod_private_narrows_a_world_readable_file() {
    let root = temp_root("chmod");
    let file = root.join("ui_takeover.json");
    std::fs::write(&file, b"{}").expect("placing the file");
    set_mode(&file, 0o604);

    chmod_private(&file).expect("narrowing the file");
    assert_eq!(mode_of(&file), FILE_MODE);
    chmod_private(&file).expect("a file that is already private is left alone");
    assert_eq!(mode_of(&file), FILE_MODE);
}

#[test]
fn test_ensure_dirs_in_stays_within_the_time_budget() {
    let bases = bases("budget");
    // Asserted on the best of several attempts rather than on one sample. This is a
    // filesystem walk against a wall clock, and `cargo nextest` runs the workspace's
    // tests in parallel, so a single sample measures the machine's contention as much
    // as it measures the call. What the budget is about is that the call *can* finish
    // inside it, which is what a cold start on an otherwise idle session asks of it.
    let mut best = Duration::MAX;
    for _ in 0..5 {
        let started = Instant::now();
        let paths = ensure_dirs_in(&bases).expect("preparing the layout");
        best = best.min(started.elapsed());
        assert!(paths.data_dir.is_dir());
    }
    assert!(best < Duration::from_millis(5), "best of 5 took {best:?}");
}
