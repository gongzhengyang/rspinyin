//! Tests for [`super`].
//!
//! Every test builds its own scratch root under the system temporary directory and a
//! synthetic dictionary, so no test reads `data/compiled/base.dict` or the Fcitx5 installed
//! on the machine.
//!
//! Two tests read the operator's real `$HOME` and `$XDG_*` directories, and neither writes to
//! them: the isolation promise is a claim about those directories, and the only way to check
//! it is to look at them. Both take a [`RealDirWitness`] -- an existence and modification
//! time per path -- before and after the work, and both have nothing to protect on a machine
//! with no resolvable home directory, where they still exercise the sandbox itself.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use super::reset::{MAX_RESET_SUFFIX, sha256_file};
use super::session::bus_address;
use super::{
    DIR_MODE, RESET_MARK, RealDirWitness, RealDirs, ResetReport, Sandbox, SandboxError,
    owned_real_dir, parse_addons, parse_maps,
};

/// A scratch directory that removes itself when the test ends, so a run leaves nothing
/// behind in the temporary directory.
#[derive(Debug)]
struct Scratch {
    /// The directory this guard owns.
    path: PathBuf,
}

impl Scratch {
    /// Creates `<temp>/rspinyin-sandbox-<tag>-<pid>`, empty.
    ///
    /// The process id keeps two test binaries apart and the tag keeps two tests in one
    /// binary apart, which is what makes the root safe to remove wholesale.
    fn new(tag: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("rspinyin-sandbox-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("creating the scratch directory");
        Self { path }
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A synthetic compiled dictionary, so no test needs the repository's own build.
fn dictionary(root: &Path) -> PathBuf {
    let path = root.join("pristine.dict");
    fs::write(&path, b"the pristine dictionary image").expect("writing the dictionary source");
    path
}

/// A sandbox under `<scratch>/sandbox`, with the pristine dictionary beside it.
fn fresh(tag: &str) -> (Scratch, Sandbox) {
    let root = Scratch::new(tag);
    let source = dictionary(&root);
    let sandbox =
        Sandbox::create_with_dict(&root.join("sandbox"), &source).expect("creating the sandbox");
    (root, sandbox)
}

/// A packaging directory holding one addon descriptor and one input-method descriptor.
fn packaging(root: &Path) -> PathBuf {
    let dir = root.join("packaging");
    fs::create_dir_all(&dir).expect("creating the packaging directory");
    fs::write(
        dir.join("rspinyin.conf"),
        "[Addon]\nName=Rust Pinyin\nCategory=InputMethod\nVersion=0.1.0\nLibrary=librspinyin\n",
    )
    .expect("writing the addon descriptor");
    fs::write(
        dir.join("rspinyin-im.conf"),
        "[InputMethod]\nName=Rust Pinyin\nAddon=rspinyin\n",
    )
    .expect("writing the input-method descriptor");
    dir
}

/// A build directory holding the library the addon descriptor names.
fn build(root: &Path) -> PathBuf {
    let dir = root.join("release");
    fs::create_dir_all(&dir).expect("creating the build directory");
    fs::write(dir.join("librspinyin.so"), b"\x7fELF staged").expect("writing the library");
    dir
}

/// The permission bits of `path`.
fn mode_of(path: &Path) -> u32 {
    fs::metadata(path).expect("mode").permissions().mode() & 0o777
}

#[test]
fn test_create_with_dict_builds_the_plugin_layout_inside_the_root() {
    let (root, sandbox) = fresh("layout");
    let expected = root.join("sandbox");

    assert_eq!(sandbox.root(), expected.as_path());
    assert_eq!(sandbox.data_home(), expected.join("data").as_path());
    assert_eq!(sandbox.config_home(), expected.join("config").as_path());
    assert_eq!(sandbox.data_dir(), expected.join("data/rspinyin").as_path());
    assert_eq!(
        sandbox.config_dir(),
        expected.join("config/rspinyin").as_path()
    );
    assert_eq!(
        sandbox.user_db(),
        expected.join("data/rspinyin/user.redb").as_path()
    );
    assert_eq!(
        sandbox.config_file(),
        expected.join("config/rspinyin/config.toml").as_path()
    );
    assert_eq!(sandbox.mirror_dir(), expected.join("runtime/rspinyin-test"));
    assert_eq!(mode_of(&expected), DIR_MODE);
    assert_eq!(mode_of(sandbox.data_dir()), DIR_MODE);
    assert!(
        sandbox.dict_path().is_file(),
        "the dictionary copy is seeded"
    );
    assert!(sandbox.dict_matches_source().expect("hashing the copy"));
    assert!(!sandbox.user_db().exists(), "a fresh sandbox has no store");
    assert!(
        !sandbox.config_file().exists(),
        "a fresh sandbox has no configuration"
    );
}

#[test]
fn test_create_with_dict_refuses_a_relative_root() {
    let result = Sandbox::create_with_dict(Path::new("relative/sandbox"), Path::new("/dev/null"));
    assert!(
        matches!(result, Err(SandboxError::RootNotAbsolute { .. })),
        "a relative root would be resolved against the working directory"
    );
}

#[test]
fn test_create_with_dict_refuses_a_missing_dictionary_source() {
    let root = Scratch::new("no-dict");
    let result = Sandbox::create_with_dict(&root.join("sandbox"), &root.join("absent.dict"));
    assert!(
        matches!(result, Err(SandboxError::DictSourceMissing { .. })),
        "a sandbox without a dictionary would run every case against nothing"
    );
}

#[test]
fn test_owned_real_dir_finds_the_directory_a_root_would_own() {
    let real = RealDirs {
        home: PathBuf::from("/home/tester"),
        data_home: PathBuf::from("/home/tester/.local/share"),
        config_home: PathBuf::from("/home/tester/.config"),
    };

    assert_eq!(
        owned_real_dir(Path::new("/home/tester"), &real),
        Some(PathBuf::from("/home/tester")),
        "a root that is the home directory would own everything under it"
    );
    assert_eq!(
        owned_real_dir(Path::new("/home"), &real),
        Some(PathBuf::from("/home/tester")),
        "an ancestor of the home directory is refused too"
    );
    assert_eq!(
        owned_real_dir(Path::new("/home/tester/.local"), &real),
        Some(PathBuf::from("/home/tester/.local/share"))
    );
    assert_eq!(
        owned_real_dir(Path::new("/tmp/rspinyin-run"), &real),
        None,
        "scratch space under /tmp is safely separate"
    );
    assert_eq!(
        owned_real_dir(Path::new("/home/tester2"), &real),
        None,
        "a sibling with a shared prefix is not an ancestor"
    );
}

#[test]
fn test_reset_of_a_clean_sandbox_is_a_noop() {
    let (_root, mut sandbox) = fresh("clean");
    let report = sandbox.reset().expect("resetting a clean sandbox");

    assert!(
        report.moved.is_empty(),
        "nothing was there to move: {:?}",
        report.moved
    );
    assert!(
        !report.dict_restored,
        "the dictionary copy already matched its source"
    );
    assert!(sandbox.dict_matches_source().expect("hashing the copy"));
}

#[test]
fn test_reset_moves_dirty_state_aside() {
    let (_root, mut sandbox) = fresh("dirty");
    fs::write(sandbox.user_db(), b"learned frequencies").expect("writing the store");
    fs::write(sandbox.config_file(), b"max_per_row = 3").expect("writing the configuration");
    fs::write(
        sandbox.mirror_dir().join("ui_frame.json"),
        b"{\"revision\":7}",
    )
    .expect("writing the frame mirror");

    let report = sandbox.reset().expect("resetting a dirty sandbox");

    assert!(
        !sandbox.user_db().exists(),
        "the store is gone from its own name"
    );
    assert!(
        !sandbox.config_file().exists(),
        "the configuration is gone from its own name"
    );
    let live: Vec<String> = fs::read_dir(sandbox.mirror_dir())
        .expect("reading the mirror")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| !name.contains(RESET_MARK))
        .collect();
    assert!(
        live.is_empty(),
        "the mirror holds no live entry, so the next case starts at revision zero: {live:?}"
    );
    let aside: Vec<String> = report
        .moved
        .iter()
        .map(|path| {
            path.file_name()
                .expect("a name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        aside,
        vec![
            "user.redb.reset.0",
            "config.toml.reset.0",
            "ui_frame.json.reset.0"
        ]
    );
    assert_eq!(
        fs::read(sandbox.data_dir().join("user.redb.reset.0")).expect("reading the artifact"),
        b"learned frequencies",
        "a reset moves state aside rather than destroying it"
    );
}

#[test]
fn test_reset_restores_a_mutated_dictionary_copy() {
    let (_root, mut sandbox) = fresh("dict");
    fs::write(sandbox.dict_path(), b"a mutated dictionary").expect("mutating the copy");
    assert!(!sandbox.dict_matches_source().expect("hashing the copy"));

    let report = sandbox.reset().expect("resetting a mutated sandbox");

    assert!(
        report.dict_restored,
        "the copy no longer hashed to its source"
    );
    assert!(sandbox.dict_matches_source().expect("hashing the copy"));
    assert_eq!(
        fs::read(sandbox.data_dir().join("base.dict.reset.0")).expect("reading the artifact"),
        b"a mutated dictionary",
        "the mutated copy is kept for diagnosis"
    );
}

#[test]
fn test_reset_refuses_a_symlinked_store() {
    let (root, mut sandbox) = fresh("symlink-store");
    let elsewhere = root.join("elsewhere.redb");
    fs::write(&elsewhere, b"the operator's data").expect("writing the link target");
    symlink(&elsewhere, sandbox.user_db()).expect("placing the link");

    let result = sandbox.reset();

    assert!(
        matches!(result, Err(SandboxError::Symlink { .. })),
        "a link is a path somebody else chose, so a reset refuses it"
    );
    assert_eq!(
        fs::read(&elsewhere).expect("reading the target"),
        b"the operator's data",
        "nothing is moved through the link"
    );
}

#[test]
fn test_resolve_refuses_a_path_outside_the_root() {
    let (root, sandbox) = fresh("outside");
    let outside = root.join("outside");

    let result = sandbox.resolve(&outside);

    assert!(
        matches!(result, Err(SandboxError::OutsideRoot { .. })),
        "the sandbox root is the only region a reset may write to"
    );
    assert!(
        sandbox.resolve(sandbox.data_dir()).is_ok(),
        "a path inside is accepted"
    );
}

#[test]
fn test_resolve_refuses_a_symlinked_component() {
    let (root, sandbox) = fresh("symlink-component");
    let elsewhere = root.join("elsewhere");
    fs::create_dir_all(&elsewhere).expect("creating the link target");
    let link = sandbox.data_dir().join("mirror");
    symlink(&elsewhere, &link).expect("placing the link");

    let result = sandbox.resolve(&link.join("ui_frame.json"));

    assert!(
        matches!(result, Err(SandboxError::Symlink { .. })),
        "a link below the root is refused, even when the target is inside the root"
    );
}

#[test]
fn test_env_redirects_every_variable_into_the_sandbox() {
    let (root, sandbox) = fresh("env");
    let expected = root.join("sandbox");
    let vars: Vec<(String, String)> = sandbox.env();

    let value = |name: &str| {
        vars.iter()
            .find(|entry| entry.0 == name)
            .map(|entry| entry.1.clone())
            .expect("the variable is set")
    };
    assert_eq!(
        value("XDG_DATA_HOME"),
        expected.join("data").display().to_string()
    );
    assert_eq!(
        value("XDG_CONFIG_HOME"),
        expected.join("config").display().to_string()
    );
    assert_eq!(
        value("XDG_CACHE_HOME"),
        expected.join("cache").display().to_string()
    );
    assert_eq!(
        value("XDG_RUNTIME_DIR"),
        expected.join("runtime").display().to_string()
    );
    assert_eq!(
        value("FCITX_ADDON_DIRS"),
        expected.join("addon").display().to_string()
    );
    assert!(
        value("FCITX_DATA_DIRS").starts_with(&format!("{}:", expected.join("share").display())),
        "the sandbox's data directory comes first"
    );
    assert!(
        !vars.iter().any(|entry| entry.0 == "HOME"),
        "the operator's home directory is left exactly as it is"
    );
}

#[test]
fn test_fcitx5_command_carries_the_sandbox_addon_directory() {
    let (root, sandbox) = fresh("command");
    let command = sandbox.fcitx5_command();
    let addon_dir = command
        .get_envs()
        .find(|entry| entry.0 == std::ffi::OsStr::new("FCITX_ADDON_DIRS"))
        .and_then(|entry| entry.1)
        .map(PathBuf::from);

    assert_eq!(
        addon_dir,
        Some(root.join("sandbox/addon")),
        "the only child constructor must point the addon lookup at the sandbox"
    );
    assert_eq!(command.get_program(), std::ffi::OsStr::new("fcitx5"));
}

#[test]
fn test_check_isolation_refuses_a_sandbox_with_no_staged_addon() {
    let (_root, sandbox) = fresh("no-addon");

    let result = sandbox.check_isolation();

    assert!(
        matches!(result, Err(SandboxError::NoAddonStaged { .. })),
        "a session with nothing staged would exercise the installed build"
    );
    assert!(
        sandbox
            .staged_libraries()
            .expect("listing the addon directory")
            .is_empty()
    );
}

#[test]
fn test_stage_plugin_copies_descriptors_and_the_library_they_name() {
    let (root, mut sandbox) = fresh("stage");
    let addons = sandbox
        .stage_plugin(&packaging(&root), &build(&root))
        .expect("staging the plugin");

    assert_eq!(
        addons,
        vec!["rspinyin"],
        "only the addon descriptor names an addon"
    );
    assert_eq!(
        sandbox.required_addons().to_vec(),
        vec!["rspinyin".to_owned()]
    );
    assert!(sandbox.check_isolation().is_ok(), "a library was staged");
    assert_eq!(
        sandbox
            .staged_libraries()
            .expect("listing the addon directory"),
        vec![root.join("sandbox/addon/librspinyin.so")]
    );
    assert!(
        root.join("sandbox/share/fcitx5/addon/rspinyin.conf")
            .is_file()
    );
    assert!(
        root.join("sandbox/share/fcitx5/inputmethod/rspinyin-im.conf")
            .is_file()
    );
}

#[test]
fn test_stage_plugin_refuses_a_library_the_build_did_not_produce() {
    let (root, mut sandbox) = fresh("stage-missing");
    let empty_build = root.join("empty");
    fs::create_dir_all(&empty_build).expect("creating the empty build directory");

    let result = sandbox.stage_plugin(&packaging(&root), &empty_build);

    assert!(
        matches!(result, Err(SandboxError::AddonLibraryMissing { .. })),
        "a descriptor naming a library nobody built is a packaging error, not a skip"
    );
}

#[test]
fn test_stage_plugin_refuses_a_descriptor_it_does_not_understand() {
    let (root, mut sandbox) = fresh("stage-unknown");
    let dir = packaging(&root);
    fs::write(dir.join("mystery.conf"), "[Something]\nName=Mystery\n")
        .expect("writing the unknown descriptor");

    let result = sandbox.stage_plugin(&dir, &build(&root));

    assert!(
        matches!(result, Err(SandboxError::UnknownDescriptor { .. })),
        "an unrecognised section means the packaging changed, which is not ours to guess at"
    );
}

#[test]
fn test_parse_addons_reads_the_verdicts_fcitx5_logs() {
    let log = "\
I2026-09-29 10:00:00.000000 addonmanager.cpp:120] Loaded addon core
I2026-09-29 10:00:00.100000 addonmanager.cpp:120] Loaded addon rspinyin
E2026-09-29 10:00:00.200000 addonmanager.cpp:131] Could not load addon rspinyin-ui
E2026-09-29 10:00:00.300000 addonmanager.cpp:133] Failed to create addon: clipboard
";

    let readiness = parse_addons(log);

    assert_eq!(
        readiness.loaded().to_vec(),
        vec!["core".to_owned(), "rspinyin".to_owned()]
    );
    assert_eq!(
        readiness.failed().to_vec(),
        vec!["rspinyin-ui".to_owned(), "clipboard".to_owned()]
    );
    assert!(readiness.is_loaded("rspinyin"));
    assert!(
        !readiness.is_loaded("rspinyin-ui"),
        "a refusal is never counted as a load"
    );
}

#[test]
fn test_parse_addons_reports_an_addon_the_log_never_mentions() {
    let log = "I2026-09-29 10:00:00.000000 addonmanager.cpp:120] Loaded addon core\n";
    let readiness = parse_addons(log);
    let required = [
        "core".to_owned(),
        "rspinyin".to_owned(),
        "rspinyin-ui".to_owned(),
    ];

    assert_eq!(
        readiness.missing(&required),
        vec!["rspinyin".to_owned(), "rspinyin-ui".to_owned()],
        "an addon the log never mentions is reported by name rather than assumed"
    );
    assert!(
        readiness.failed().is_empty(),
        "nothing was refused, it simply never loaded"
    );
}

#[test]
fn test_parse_maps_keeps_only_shared_objects_from_the_sandbox() {
    let root = Path::new("/run/case/sandbox");
    let maps = "\
7f0000000000-7f0000001000 r--p 00000000 08:01 100 /run/case/sandbox/addon/librspinyin.so
7f0000001000-7f0000002000 r-xp 00000000 08:01 100 /run/case/sandbox/addon/librspinyin.so
7f0000002000-7f0000003000 r-xp 00000000 08:01 101 /usr/lib/x86_64-linux-gnu/fcitx5/libcore.so
7f0000003000-7f0000004000 r-xp 00000000 08:01 102 /run/case/sandbox/harness/fcitx5.log (deleted)
7f0000004000-7f0000005000 rw-p 00000000 00:00 0
7f0000005000-7f0000006000 rw-p 00000000 00:00 0                          [heap]
";

    let mapped = parse_maps(maps, root);

    assert_eq!(
        mapped,
        vec![PathBuf::from("/run/case/sandbox/addon/librspinyin.so")],
        "a duplicate mapping is listed once and a foreign library is not ours"
    );
}

#[test]
fn test_real_dir_witness_reports_a_changed_directory() {
    let root = Scratch::new("witness");
    let watched = root.join("rspinyin");
    fs::create_dir_all(&watched).expect("creating the watched directory");
    let witness = RealDirWitness::of(&[watched.clone(), root.join("never-created")]);

    assert!(witness.changed().is_empty(), "nothing moved yet");
    assert!(witness.assert_unchanged().is_ok());

    fs::write(watched.join("user.redb"), b"learned frequencies").expect("writing into the tree");
    let changed = witness.changed();
    assert_eq!(
        changed,
        vec![watched],
        "a write into the real directory is reported"
    );
    assert!(matches!(
        witness.assert_unchanged(),
        Err(SandboxError::RealDirsChanged { .. })
    ));
}

#[test]
fn test_real_dir_witness_reports_a_directory_that_appeared() {
    let root = Scratch::new("witness-created");
    let absent = root.join("rspinyin");
    let witness = RealDirWitness::of(std::slice::from_ref(&absent));

    fs::create_dir_all(&absent).expect("creating the directory after the witness");

    assert_eq!(
        witness.changed(),
        vec![absent],
        "a directory that appeared where there was none is a change"
    );
}

#[test]
fn test_two_sandboxes_under_one_parent_do_not_share_state() {
    let root = Scratch::new("two-cases");
    let source = dictionary(&root);
    let first = Sandbox::create_with_dict(&root.join("case-one/sandbox"), &source)
        .expect("creating the first sandbox");
    let mut second = Sandbox::create_with_dict(&root.join("case-two/sandbox"), &source)
        .expect("creating the second sandbox");

    fs::write(first.user_db(), b"the first case's frequencies").expect("writing the store");
    fs::write(
        first.mirror_dir().join("ui_frame.json"),
        b"{\"revision\":9}",
    )
    .expect("writing the frame mirror");

    let report = second.reset().expect("resetting the second sandbox");

    assert!(
        report.moved.is_empty(),
        "the second sandbox never saw the first one's state"
    );
    assert!(
        !second.user_db().exists() && !second.mirror_dir().join("ui_frame.json").exists(),
        "no state carried over from the first case"
    );
    assert!(
        first.user_db().exists(),
        "the first sandbox is untouched by the second reset"
    );
}

#[test]
fn test_create_with_dict_refuses_a_root_that_would_own_the_real_directories() {
    // The one test that names a real path: the guard exists for the operator's own
    // directories, and a guard is checked by aiming at what it protects. The root preparation
    // refuses before its first filesystem call, so the refusal has written nothing -- which is
    // what the witness here checks. A machine whose home directory cannot be resolved has no
    // directory to protect, and the pure `owned_real_dir` test above is the whole of the rule
    // there. The dictionary source is never looked at: the guard refuses before the source is.
    let Some(real) = RealDirs::from_env() else {
        return;
    };
    assert!(
        real.home.is_absolute() && real.data_home.is_absolute() && real.config_home.is_absolute(),
        "a relative base directory is not one a sandbox could protect: {real:?}"
    );
    let root = real.data_home;
    let witness = RealDirWitness::of(std::slice::from_ref(&root));

    let result = Sandbox::create_with_dict(&root, Path::new("/dev/null"));

    assert!(
        matches!(&result, Err(SandboxError::RootOwnsRealDirs { .. })),
        "a sandbox rooted at the operator's own data directory could reset it: {result:?}"
    );
    let changed = witness.changed();
    assert!(
        changed.is_empty(),
        "the refusal comes before anything under the real data directory is created: {changed:?}"
    );
}

#[test]
fn test_a_full_case_leaves_the_real_directories_alone() {
    // The assertion the isolation promise is checked with: everything a case does -- build a
    // tree, stage the plugin, write state, reset -- runs while the operator's own plugin
    // directories are watched, and none of it may reach them. The scratch tree is removed
    // with the case, which is the other half of what a case is allowed to leave behind.
    let witness = RealDirWitness::capture();
    let root =
        std::env::temp_dir().join(format!("rspinyin-sandbox-lifecycle-{}", std::process::id()));
    {
        let scratch = Scratch::new("lifecycle");
        assert_eq!(
            scratch.to_path_buf(),
            root,
            "the scratch tree this test removes"
        );
        let source = dictionary(&scratch);
        let mut sandbox = Sandbox::create_with_dict(&scratch.join("sandbox"), &source)
            .expect("creating the sandbox");
        sandbox
            .stage_plugin(&packaging(&scratch), &build(&scratch))
            .expect("staging the plugin");
        fs::write(sandbox.user_db(), b"learned frequencies").expect("writing the store");
        fs::write(sandbox.config_file(), b"max_per_row = 3").expect("writing the configuration");
        let frame = sandbox.mirror_dir().join("ui_frame.json");
        fs::write(&frame, b"{\"revision\":4}").expect("writing the frame mirror");
        sandbox.reset().expect("resetting the case's sandbox");
        if let Some(real) = RealDirs::from_env() {
            assert!(
                owned_real_dir(&root, &real).is_none(),
                "a case's tree is built outside the operator's own directories"
            );
        }
    }

    let changed = witness.changed();
    assert!(
        changed.is_empty(),
        "a case must not touch the real plugin directories: {changed:?}"
    );
    assert!(
        !root.exists(),
        "a case that has ended leaves nothing behind in the temporary directory"
    );
}

#[test]
fn test_reset_twice_leaves_the_state_of_one_reset() {
    let (_root, mut sandbox) = fresh("twice");
    fs::write(sandbox.user_db(), b"learned frequencies").expect("writing the store");
    let frame = sandbox.mirror_dir().join("ui_frame.json");
    fs::write(&frame, b"{\"revision\":7}").expect("writing the frame mirror");

    let first = sandbox.reset().expect("the first reset");
    let second = sandbox.reset().expect("the second reset");

    assert!(
        !first.moved.is_empty(),
        "the first reset took the case's state away: {first:?}"
    );
    assert_eq!(
        second,
        ResetReport::default(),
        "a second reset finds nothing left to take away, which is what makes it idempotent"
    );
    assert!(sandbox.dict_matches_source().expect("hashing the copy"));
    assert!(
        sandbox.data_dir().join("user.redb.reset.0").is_file(),
        "the first reset's evidence survives the second"
    );
    assert!(
        !sandbox.user_db().exists() && !sandbox.mirror_dir().join("ui_frame.json").exists(),
        "the state the first reset took away is still gone"
    );
}

#[test]
fn test_reset_reports_the_path_it_could_not_move_and_stops_there() {
    let (_root, mut sandbox) = fresh("unmovable");
    fs::write(sandbox.config_file(), b"max_per_row = 3").expect("writing the configuration");
    // A file where the data directory belongs: the store below it cannot be inspected, let
    // alone moved, and the reset has to say which path it was.
    fs::remove_dir_all(sandbox.data_dir()).expect("removing the data directory");
    fs::write(sandbox.data_dir(), b"not a directory").expect("blocking the data directory");
    let store = sandbox.user_db().to_path_buf();

    let result = sandbox.reset();

    assert!(
        matches!(&result, Err(SandboxError::Io { path, .. }) if *path == store),
        "the refusal names the path it could not inspect: {result:?}"
    );
    assert!(
        sandbox.config_file().is_file(),
        "the reset stops at the first refusal rather than half-applying itself"
    );
}

#[test]
fn test_reset_refuses_when_every_name_beside_the_store_is_taken() {
    let (_root, mut sandbox) = fresh("exhausted");
    fs::write(sandbox.config_file(), b"max_per_row = 3").expect("writing the configuration");
    let store = sandbox.user_db().to_path_buf();
    fs::write(&store, b"learned frequencies").expect("writing the store");
    let name = store.file_name().expect("the store has a name");
    for suffix in 0..=MAX_RESET_SUFFIX {
        let mut aside = name.to_os_string();
        aside.push(format!("{RESET_MARK}{suffix}"));
        fs::write(store.with_file_name(aside), b"an earlier reset").expect("taking the name");
    }

    let result = sandbox.reset();

    assert!(
        matches!(&result, Err(SandboxError::ResetNameExhausted { path }) if *path == store),
        "the refusal names the file whose names are taken: {result:?}"
    );
    assert_eq!(
        fs::read(&store).expect("reading the store"),
        b"learned frequencies",
        "nothing was overwritten to make room for a reset"
    );
    assert!(
        sandbox.config_file().is_file(),
        "the reset stops at the refusal rather than half-applying itself"
    );
}

#[test]
fn test_reset_rebuilds_a_dictionary_copy_that_is_gone() {
    let (_root, mut sandbox) = fresh("dict-gone");
    fs::remove_file(sandbox.dict_path()).expect("removing the copy");
    assert!(
        !sandbox.dict_matches_source().expect("hashing the copy"),
        "a copy that is not there does not match its source"
    );

    let report = sandbox.reset().expect("resetting a sandbox with no copy");

    assert!(report.dict_restored, "the copy had to be rebuilt");
    assert!(
        sandbox.dict_path().is_file() && sandbox.dict_matches_source().expect("hashing the copy"),
        "the rebuilt copy hashes to the pristine source"
    );
}

#[test]
fn test_sha256_file_matches_the_published_digest() {
    let root = Scratch::new("sha256");
    let empty = root.join("empty.dict");
    let abc = root.join("abc.dict");
    fs::write(&empty, b"").expect("writing the empty file");
    fs::write(&abc, b"abc").expect("writing the three-byte file");

    assert_eq!(
        sha256_file(&empty).expect("hashing the empty file"),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "the digest is written as lowercase hexadecimal"
    );
    assert_eq!(
        sha256_file(&abc).expect("hashing `abc`"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        "the dictionary copy is compared with its source through this digest"
    );
}

#[test]
fn test_sha256_file_reports_a_file_it_cannot_read() {
    let root = Scratch::new("sha256-missing");

    let result = sha256_file(&root.join("absent.dict"));

    assert!(
        matches!(result, Err(SandboxError::Io { .. })),
        "a file that is not there is a refusal rather than an empty digest"
    );
}

#[test]
fn test_bus_address_gives_each_start_attempt_a_path_of_its_own() {
    let runtime = Path::new("/run/case/sandbox/runtime");

    assert_eq!(
        bus_address(runtime, 0),
        "unix:path=/run/case/sandbox/runtime/bus.0"
    );
    assert_ne!(
        bus_address(runtime, 0),
        bus_address(runtime, 1),
        "a retry must not reuse the bus the attempt that failed left behind"
    );
}

#[test]
fn test_log_path_is_the_archived_session_log() {
    let (root, sandbox) = fresh("log-path");

    assert_eq!(sandbox.log_path(), root.join("sandbox/harness/fcitx5.log"));
}

#[test]
fn test_mapped_addon_libraries_without_a_session_is_empty() {
    let (_root, sandbox) = fresh("no-session");

    assert!(
        sandbox
            .mapped_addon_libraries()
            .expect("reading the memory map")
            .is_empty(),
        "a sandbox that started nothing has mapped nothing"
    );
}

#[test]
fn test_stop_fcitx5_without_a_session_is_a_noop() {
    let (_root, mut sandbox) = fresh("no-child");

    sandbox.stop_fcitx5();
    sandbox.stop_fcitx5();

    assert!(
        sandbox.dict_matches_source().expect("hashing the copy"),
        "stopping a session that was never started leaves the sandbox usable"
    );
}
