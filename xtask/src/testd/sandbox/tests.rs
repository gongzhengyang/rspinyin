//! Tests for [`super`].
//!
//! Every test builds its own scratch root under the system temporary directory and a
//! synthetic dictionary, so no test reads the operator's `$HOME`, the real `$XDG_*`
//! directories, `data/compiled/base.dict`, or the Fcitx5 installed on the machine.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use super::{
    DIR_MODE, RESET_MARK, RealDirWitness, RealDirs, Sandbox, SandboxError, owned_real_dir,
    parse_addons, parse_maps,
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
