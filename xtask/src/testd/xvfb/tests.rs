//! Tests for [`super`].
//!
//! Every one works through the seams the arrangement and the poll expose, so none of
//! them starts a real server or opens a real display: the tooling reading is a path
//! value, the numbers already taken are a closure, the spawner is a closure, and the
//! readiness poll takes its probe as a closure. The one `#[ignore]`d test starts a real
//! server, and it exists for the machine the channel is run on.

use std::env::join_paths;

use super::*;

/// A scratch directory that removes itself when the test ends.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("rspinyin-xvfb-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("creating the scratch directory");
        Self { path }
    }

    /// An empty subdirectory, for the cases a reading needs a directory of its own.
    fn subdir(&self, name: &str) -> PathBuf {
        let dir = self.path.join(name);
        fs::create_dir_all(&dir).expect("creating the subdirectory");
        dir
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

/// Writes `name` into `dir` with the given mode bits.
fn write_file(dir: &Path, name: &str, mode: u32) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, b"").expect("writing the file");
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("setting the mode");
    path
}

/// A display that owns no child and cleans up inside the scratch directories.
fn bare(number: u32, socket_dir: &Path, lock_dir: &Path) -> VirtualDisplay {
    VirtualDisplay {
        number,
        display: format!(":{number}"),
        child: None,
        socket_dir: socket_dir.to_path_buf(),
        lock_dir: lock_dir.to_path_buf(),
    }
}

#[test]
fn test_find_on_path_accepts_an_executable_file_and_refuses_the_rest() {
    let scratch = Scratch::new("find");
    let path = join_paths([scratch.as_os_str()]).expect("joining one directory");
    let _ = write_file(&scratch, "tool", 0o755);
    let _ = write_file(&scratch, "dormant", 0o644);
    let _ = scratch.subdir("subdir");

    assert!(find_on_path("tool", Some(path.as_os_str())));
    assert!(
        !find_on_path("dormant", Some(path.as_os_str())),
        "a non-executable file is not a tool"
    );
    assert!(
        !find_on_path("subdir", Some(path.as_os_str())),
        "a directory is not a tool"
    );
    assert!(!find_on_path("absent", Some(path.as_os_str())));
    assert!(!find_on_path("tool", None), "no path names no tools");
}

#[test]
fn test_tooling_reports_which_of_the_two_tools_is_on_the_path() {
    let scratch = Scratch::new("tooling");
    let empty = scratch.subdir("empty");
    let _ = write_file(&scratch, "Xvfb", 0o755);
    let path = join_paths([scratch.as_os_str()]).expect("joining one directory");

    let tooling = Tooling::from_path(Some(path.as_os_str()));
    assert_eq!(
        tooling,
        Tooling {
            server: true,
            runner: false,
        }
    );
    assert!(tooling.is_available());
    assert_eq!(tooling.missing(), [RUNNER]);

    let absent = Tooling::from_path(Some(empty.as_os_str()));
    assert_eq!(
        absent,
        Tooling {
            server: false,
            runner: false,
        }
    );
    assert!(!absent.is_available());
    assert_eq!(absent.missing(), [SERVER, RUNNER]);
    assert_eq!(Tooling::from_path(None).missing(), [SERVER, RUNNER]);
}

#[test]
fn test_pick_display_number_takes_the_first_free_number_and_refuses_a_full_range() {
    let taken = |number: u32| matches!(number, 90 | 91);
    assert_eq!(pick_display_number(&taken, 90, 5), Some(92));

    let full = |_: u32| true;
    assert_eq!(pick_display_number(&full, 90, 4), None);
}

#[test]
fn test_display_is_taken_reads_the_socket_and_the_lock_file() {
    let scratch = Scratch::new("taken");
    let unix = scratch.subdir("unix");
    let tmp = scratch.subdir("tmp");
    assert!(!display_is_taken(&unix, &tmp, 5));

    let socket = socket_path(&unix, 5);
    fs::write(&socket, b"").expect("writing the socket placeholder");
    assert!(
        display_is_taken(&unix, &tmp, 5),
        "a socket claims the number"
    );

    fs::remove_file(&socket).expect("removing the placeholder");
    let lock = lock_path(&tmp, 5);
    fs::write(&lock, b"").expect("writing the lock placeholder");
    assert!(
        display_is_taken(&unix, &tmp, 5),
        "a lock file claims the number too"
    );
}

#[test]
fn test_poll_ready_succeeds_on_the_first_consumed_probe_key() {
    let mut probes = 0;
    let waiting = Waiting {
        deadline: Instant::now() + Duration::from_secs(30),
        interval: POLL_INTERVAL,
        timeout: READY_TIMEOUT,
    };
    poll_ready(
        ":90",
        &mut || {
            probes += 1;
            true
        },
        &mut || None,
        waiting,
    )
    .expect("a consumed probe key is readiness");
    assert_eq!(probes, 1, "a ready display is not probed again");
}

#[test]
fn test_poll_ready_reports_a_timeout_when_the_deadline_has_passed() {
    let mut probes = 0;
    let waiting = Waiting {
        deadline: Instant::now(),
        interval: POLL_INTERVAL,
        timeout: READY_TIMEOUT,
    };
    let error = poll_ready(
        ":91",
        &mut || {
            probes += 1;
            false
        },
        &mut || None,
        waiting,
    )
    .expect_err("a display no probe accepts is not ready");
    assert!(
        matches!(error, XvfbError::NotReady { ref display, .. } if display == ":91"),
        "{error:?}"
    );
    assert_eq!(probes, 1, "an expired deadline does not buy another round");
}

#[test]
fn test_poll_ready_reports_a_server_that_exited_first() {
    let waiting = Waiting {
        deadline: Instant::now() + Duration::from_secs(30),
        interval: POLL_INTERVAL,
        timeout: READY_TIMEOUT,
    };
    let error = poll_ready(
        ":92",
        &mut || {
            panic!("an exited server is reported before any probe runs");
        },
        &mut || Some(String::from("signal 6 (ABRT)")),
        waiting,
    )
    .expect_err("a server that exited first is its own failure");
    assert!(
        matches!(error, XvfbError::ServerExited { ref display, ref status }
            if display == ":92" && status.contains("signal")),
        "{error:?}"
    );
}

#[test]
fn test_arrange_refuses_a_machine_without_the_server_tool() {
    let scratch = Scratch::new("arrange");
    let unix = scratch.subdir("unix");
    let tmp = scratch.subdir("tmp");
    let tooling = Tooling {
        server: false,
        runner: true,
    };
    let arrangement = Arrangement {
        tooling: &tooling,
        taken: &|_| false,
        spawn: &|_number: u32| -> io::Result<Child> {
            panic!("no server is spawned where no tool exists");
        },
        socket_dir: &unix,
        lock_dir: &tmp,
    };
    let error = VirtualDisplay::arrange(arrangement).expect_err("no tool, no channel");
    assert!(
        matches!(error, XvfbError::ToolingMissing { ref missing, remedy }
            if missing.contains(SERVER) && !missing.contains(RUNNER) && remedy.contains("install xvfb")),
        "the refusal names exactly what is missing and the install remedy: {error:?}"
    );
}

#[test]
fn test_stop_kills_the_child_and_cleans_the_display_files() {
    let scratch = Scratch::new("stop-kill");
    let unix = scratch.subdir("unix");
    let tmp = scratch.subdir("tmp");
    let _ = write_file(&unix, "X7", 0o644);
    let _ = write_file(&tmp, ".X7-lock", 0o644);
    let child = Command::new("sleep").arg("30").spawn().expect("a sleeper");
    let mut display = VirtualDisplay {
        number: 7,
        display: ":7".to_owned(),
        child: Some(child),
        socket_dir: unix.clone(),
        lock_dir: tmp.clone(),
    };

    let status = display.stop().expect("a running child was stopped");
    assert_eq!(
        status.code(),
        None,
        "the child was killed by a signal, not allowed to finish"
    );
    assert!(!socket_path(&unix, 7).exists(), "the socket was removed");
    assert!(!lock_path(&tmp, 7).exists(), "the lock file was removed");
}

#[test]
fn test_stop_without_a_child_still_cleans_the_display_files() {
    let scratch = Scratch::new("stop-bare");
    let unix = scratch.subdir("unix");
    let tmp = scratch.subdir("tmp");
    let _ = write_file(&unix, "X8", 0o644);
    let _ = write_file(&tmp, ".X8-lock", 0o644);
    let mut display = bare(8, &unix, &tmp);

    assert_eq!(display.stop(), None, "no child, no status");
    assert!(!socket_path(&unix, 8).exists());
    assert!(!lock_path(&tmp, 8).exists());
}

#[test]
fn test_unix_socket_servable_asks_for_the_sticky_bit_the_transport_layer_wants() {
    let scratch = Scratch::new("servable");
    let strict = scratch.subdir("strict");
    fs::set_permissions(&strict, fs::Permissions::from_mode(0o1777))
        .expect("setting the sticky mode");
    assert!(
        unix_socket_servable(&strict),
        "1777 is what xtrans asks for"
    );

    let lax = scratch.subdir("lax");
    fs::set_permissions(&lax, fs::Permissions::from_mode(0o777)).expect("setting 0777");
    assert!(
        !unix_socket_servable(&lax),
        "a directory without the sticky bit cannot serve a unix socket"
    );
    assert!(
        !unix_socket_servable(&scratch.join("absent")),
        "a directory that does not exist serves nothing"
    );
}

#[test]
fn test_tcp_fallback_note_appears_only_where_unix_sockets_cannot_serve() {
    let scratch = Scratch::new("note");
    let strict = scratch.subdir("strict");
    fs::set_permissions(&strict, fs::Permissions::from_mode(0o1777))
        .expect("setting the sticky mode");
    assert_eq!(tcp_fallback_note(&strict), None);

    let lax = scratch.subdir("lax");
    fs::set_permissions(&lax, fs::Permissions::from_mode(0o777)).expect("setting 0777");
    let note = tcp_fallback_note(&lax).expect("the fallback carries an environment note");
    assert!(note.contains("TCP"), "{note}");
}

#[test]
fn test_display_env_carries_the_display_a_child_must_inherit() {
    let scratch = Scratch::new("env");
    let display = bare(91, &scratch.subdir("unix"), &scratch.subdir("tmp"));
    let mut command = Command::new("true");
    command.envs([display.display_env()]);
    let injected = command
        .get_envs()
        .find(|(name, _)| *name == OsStr::new(DISPLAY_VAR))
        .expect("the pair was put on the command")
        .1;
    assert_eq!(
        injected,
        Some(OsStr::new(":91")),
        "the pair is the injection a child lands on the virtual display with"
    );
}

#[test]
fn test_channel_env_injects_display_only_when_the_channel_is_up() {
    assert!(channel_env(None).is_empty(), "no channel, no injection");
    assert_eq!(
        channel_env(Some(":90")),
        vec![("DISPLAY".to_owned(), ":90".to_owned())]
    );
}

#[test]
#[ignore = "starts a real Xvfb server; run it where Xvfb is installed with --ignored"]
fn test_open_serves_a_display_the_injection_probe_accepts() {
    let mut display = VirtualDisplay::open().expect("a machine with Xvfb installed");
    display.wait_ready().expect("a fresh server is ready");
    assert!(
        display.display().starts_with(':'),
        "{:?}",
        display.display()
    );

    let session = X11Session::connect(Some(display.display()))
        .expect("the virtual display answers a connection");
    assert_eq!(
        session.screen(),
        (1280, 800),
        "the fixed known geometry the golden rule pins"
    );
}
