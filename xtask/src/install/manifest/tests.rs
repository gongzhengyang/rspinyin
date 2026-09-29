//! The install manifest's own tests.
//!
//! Split out of `manifest.rs` to keep that file inside the line limit; the module is
//! private to the manifest, so everything here reaches its items through `super`.

use super::*;

/// A scratch directory unique to this test process and tag.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rspinyin-install-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

/// One entry, for the manifest fixtures.
fn entry(path: &Path, state: EntryState, backup: Option<PathBuf>) -> Entry {
    Entry {
        path: path.to_path_buf(),
        state,
        backup,
    }
}

#[test]
fn test_mode_of_reads_the_permission_bits_and_falls_back_to_the_installed_mode() {
    let dir = scratch("mode-of");
    let file = dir.join("shipped");
    fs::write(&file, "a library a distribution put there").expect("writing the fixture");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("marking it");
    assert_eq!(
        mode_of(&file),
        0o755,
        "the bits a distribution shipped survive"
    );

    // A file with the setuid bit keeps only the permission bits: restoring a setuid
    // bit an installer never wrote would be a privilege escalation, not a restoration.
    fs::set_permissions(&file, fs::Permissions::from_mode(0o4755)).expect("marking it");
    assert_eq!(mode_of(&file), 0o755);

    assert_eq!(
        mode_of(&dir.join("never-written")),
        FILE_MODE,
        "a path with no mode has nothing to preserve, so the installer's own is used"
    );
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_place_file_writes_the_content_and_the_mode() {
    let dir = scratch("place");
    let source = dir.join("source");
    fs::write(&source, b"payload").expect("writing the fixture");
    let destination = dir.join("nested/deeper/target");

    place_file(Elevation::Direct, &source, &destination, FILE_MODE).expect("placing the file");
    assert_eq!(
        fs::read(&destination).expect("reading back"),
        b"payload".to_vec()
    );
    let mode = fs::metadata(&destination)
        .expect("stat")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, FILE_MODE);
    assert!(
        !temporary_path(&destination)
            .expect("a temporary name")
            .exists(),
        "the staged copy is renamed away"
    );
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_place_file_replaces_an_existing_file_and_leaves_no_temporary_behind() {
    let dir = scratch("replace");
    let source = dir.join("source");
    let destination = dir.join("target");
    fs::write(&source, b"new").expect("writing the fixture");
    fs::write(&destination, b"old").expect("writing the fixture");

    place_file(Elevation::Direct, &source, &destination, FILE_MODE).expect("placing the file");
    assert_eq!(
        fs::read(&destination).expect("reading back"),
        b"new".to_vec()
    );
    let leftovers: Vec<_> = fs::read_dir(&dir)
        .expect("listing the scratch directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("rspinyin-tmp"))
        .collect();
    assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_place_file_reports_a_missing_source() {
    let dir = scratch("missing");
    let failure = place_file(
        Elevation::Direct,
        &dir.join("absent"),
        &dir.join("target"),
        FILE_MODE,
    )
    .expect_err("a missing source must fail");
    assert!(failure.to_string().contains("target"), "{failure}");
    assert!(
        !dir.join("target").exists(),
        "nothing is installed from a failed copy"
    );
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_remove_file_and_remove_directory_treat_absence_as_done() {
    let dir = scratch("remove");
    let file = dir.join("file");
    let nested = dir.join("nested");
    fs::create_dir_all(&nested).expect("creating the fixture");
    fs::write(&file, b"x").expect("writing the fixture");

    remove_file(Elevation::Direct, &file).expect("removing a file that is there");
    remove_file(Elevation::Direct, &file).expect("removing a file that is not");
    remove_directory(Elevation::Direct, &nested).expect("removing an empty directory");
    remove_directory(Elevation::Direct, &nested).expect("removing a directory that is not");
    assert!(!file.exists() && !nested.exists());
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_remove_directory_leaves_a_directory_that_is_not_empty() {
    let dir = scratch("nonempty");
    let nested = dir.join("nested");
    fs::create_dir_all(&nested).expect("creating the fixture");
    fs::write(nested.join("someone-elses-file"), b"x").expect("writing the fixture");

    remove_directory(Elevation::Direct, &nested).expect("a non-empty directory is not an error");
    assert!(
        nested.join("someone-elses-file").exists(),
        "the other file must survive"
    );
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_backup_path_sits_next_to_the_file_it_copies() {
    assert_eq!(
        backup_path(Path::new("/usr/share/fcitx5/addon/rspinyin.conf")),
        PathBuf::from("/usr/share/fcitx5/addon/rspinyin.conf.rspinyin-bak")
    );
}

#[test]
fn test_manifest_round_trips_through_json() {
    let dir = scratch("manifest");
    let path = dir.join("nested/install-manifest.json");
    let manifest = Manifest {
        version: MANIFEST_VERSION,
        package_version: "0.1.0".to_owned(),
        entries: vec![
            entry(
                Path::new("/usr/lib/fcitx5/librspinyin.so"),
                EntryState::Created,
                None,
            ),
            entry(
                Path::new("/usr/share/fcitx5/addon/rspinyin.conf"),
                EntryState::Replaced,
                Some(PathBuf::from(
                    "/usr/share/fcitx5/addon/rspinyin.conf.rspinyin-bak",
                )),
            ),
        ],
    };

    manifest
        .write(Elevation::Direct, &path)
        .expect("writing the manifest");
    let read = Manifest::read(&path)
        .expect("reading the manifest")
        .expect("the manifest exists");
    assert_eq!(read, manifest);
    assert_eq!(
        read.entry(Path::new("/usr/lib/fcitx5/librspinyin.so"))
            .expect("the entry is found")
            .state,
        EntryState::Created
    );
    assert!(read.entry(Path::new("/nowhere")).is_none());
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_manifest_read_reports_absence_and_refuses_a_newer_schema() {
    let dir = scratch("schema");
    let path = dir.join("install-manifest.json");
    assert!(
        Manifest::read(&path)
            .expect("absence is not an error")
            .is_none()
    );

    fs::write(
        &path,
        r#"{"version":99,"package_version":"0.1.0","entries":[]}"#,
    )
    .expect("writing the fixture");
    let failure = Manifest::read(&path).expect_err("a newer schema is refused");
    assert!(failure.to_string().contains("99"), "{failure}");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_check_writable_accepts_a_scratch_directory_and_reports_a_missing_ancestor() {
    let dir = scratch("writable");
    check_writable(&[dir.join("not-created-yet"), dir.clone()])
        .expect("a scratch directory is writable");

    let failure = check_writable(&[PathBuf::from("rspinyin-nowhere/nowhere")])
        .expect_err("a relative path has no ancestor to probe");
    assert!(failure.to_string().contains("ancestor"), "{failure}");

    let leftovers: Vec<_> = fs::read_dir(&dir)
        .expect("listing the scratch directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("rspinyin-probe"))
        .collect();
    assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_check_writable_refuses_a_path_a_file_is_in_the_way_of() {
    // The probe creates its file in the nearest existing ancestor, and a regular file
    // is one of those. Reporting it as unwritable would name the wrong cause, and
    // letting it through would move the failure to the first copy, where the message
    // is about a path the user never chose.
    let dir = scratch("writable-file");
    let file = dir.join("a-file");
    fs::write(&file, "not a directory").expect("writing the fixture");

    let failure = check_writable(&[file.join("nested")])
        .expect_err("a file is not a directory to install into");
    assert!(failure.to_string().contains("not a directory"), "{failure}");
    assert!(
        !file.join("nested").exists(),
        "the probe creates nothing under a file"
    );
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_check_writable_refuses_a_directory_this_account_cannot_write() {
    let dir = scratch("writable-mode");
    let sealed = dir.join("sealed");
    fs::create_dir_all(&sealed).expect("creating the fixture directory");
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o500)).expect("sealing it");

    match check_writable(std::slice::from_ref(&sealed)) {
        Err(error) => assert!(error.to_string().contains("not writable"), "{error}"),
        // Only the superuser may write into a directory whose permission bits exclude
        // the owner, so reaching this arm says which account ran the test rather than
        // that the probe is wrong.
        Ok(()) => assert_eq!(
            effective_uid().expect("the effective user id is readable"),
            0,
            "a directory at 0500 is writable by root alone"
        ),
    }
    // Reopened so that the cleanup can empty and remove it either way.
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o755)).expect("reopening it");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_place_file_names_the_way_out_of_a_permission_failure() {
    // The one failure a user hits on a machine where the installer may not escalate:
    // the message has to say what to do about it rather than repeat the errno.
    let dir = scratch("place-denied");
    let source = dir.join("source");
    fs::write(&source, b"payload").expect("writing the fixture");
    let sealed = dir.join("sealed");
    fs::create_dir_all(&sealed).expect("creating the fixture directory");
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o500)).expect("sealing it");

    match place_file(
        Elevation::Direct,
        &source,
        &sealed.join("target"),
        FILE_MODE,
    ) {
        Err(error) => {
            let message = format!("{error:?}");
            assert!(message.contains("not writable"), "{message}");
            assert!(message.contains("--no-sudo"), "{message}");
        }
        Ok(()) => assert_eq!(
            effective_uid().expect("the effective user id is readable"),
            0,
            "a directory at 0500 is writable by root alone"
        ),
    }
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o755)).expect("reopening it");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_is_permission_denied_reads_through_the_context_chain() {
    // `place_file` classifies the error it gets back from a copy, and the cause it
    // has to find is the `io::Error` underneath the context the copy added. A
    // classifier that only looked at the outermost message would never fire.
    let denied: anyhow::Error = std::io::Error::from(ErrorKind::PermissionDenied).into();
    let wrapped = denied.context("installing /usr/lib/fcitx5/librspinyin.so");
    assert!(is_permission_denied(&wrapped));

    let missing: anyhow::Error = std::io::Error::from(ErrorKind::NotFound).into();
    let absent = missing.context("installing /usr/lib/fcitx5/librspinyin.so");
    assert!(!is_permission_denied(&absent));

    let bare = anyhow::anyhow!("a failure with no cause at all");
    assert!(
        !is_permission_denied(&bare),
        "a failure that names no cause is not a permission failure"
    );
}

#[test]
fn test_staging_path_names_a_file_this_account_can_write() {
    // The escalated branch serializes the manifest here before handing it to `sudo`,
    // so the path has to be one this account owns rather than one beside the
    // destination, which belongs to root.
    let staged = staging_path(Path::new("/usr/share/rspinyin/install-manifest.json"))
        .expect("a destination with a file name");
    assert_eq!(staged.parent(), Some(std::env::temp_dir().as_path()));
    let name = staged
        .file_name()
        .expect("the staged file has a name")
        .to_string_lossy()
        .into_owned();
    assert!(name.contains("install-manifest.json"), "{name}");
    assert!(
        name.contains(&std::process::id().to_string()),
        "two runs cannot collide in it: {name}"
    );

    let failure = staging_path(Path::new("/")).expect_err("a root has no file name");
    assert!(failure.to_string().contains("file name"), "{failure}");
}

#[test]
fn test_nearest_existing_walks_up_to_the_first_directory_that_is_there() {
    let dir = scratch("nearest");
    assert_eq!(
        nearest_existing(&dir.join("a/b/c")).expect("the scratch directory exists"),
        dir
    );
    assert_eq!(
        nearest_existing(&dir).expect("the scratch directory exists"),
        dir
    );
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_elevation_detect_honours_the_no_sudo_flag() {
    // The flag has to win regardless of which user runs the tests, which is the
    // property a `DESTDIR` staging step depends on.
    assert_eq!(
        Elevation::detect(true).expect("the effective user id is readable"),
        Elevation::Direct
    );
    assert!(!Elevation::Direct.is_sudo());
    assert!(Elevation::Sudo.is_sudo());
}

#[test]
fn test_effective_uid_from_reads_the_effective_field() {
    // The `Uid:` line is `real effective saved filesystem`; reading the wrong field
    // would escalate on a setuid invocation that needs no escalation.
    assert_eq!(
        effective_uid_from("Name:\tcat\nUid:\t1000\t1001\t1002\t1003\n").expect("a full line"),
        1001
    );
}

#[test]
fn test_effective_uid_from_rejects_text_it_cannot_read() {
    assert!(effective_uid_from("Name:\tcat\n").is_err(), "no Uid: line");
    assert!(
        effective_uid_from("Uid:\t1000\n").is_err(),
        "too few fields"
    );
    assert!(
        effective_uid_from("Uid:\t1000\troot\t1000\t1000\n").is_err(),
        "the effective field is not a number"
    );
}
