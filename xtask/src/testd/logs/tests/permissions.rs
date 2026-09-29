//! The mode every path of the log family must carry.
//!
//! Responsibility: pin the third assertion of the channel -- the one the content rules cannot
//! make -- against the modes the plugin's own layout fixes, and pin that a mode wider than the
//! contract is reported against the path it belongs to rather than against a line number.
//!
//! Boundaries: it reads modes and nothing else. Every mode a case here asserts on is set by the
//! case rather than inherited from the process umask, so the verdict does not depend on the
//! machine the suite runs on.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use super::{Scratch, append, findings, roll, sibling};
use crate::testd::logs::{Denial, LogTap};

/// The permission bits `path` carries, in the low nine.
fn mode_bits(path: &Path) -> u32 {
    let metadata = fs::metadata(path).expect("reading the mode");
    metadata.permissions().mode() & 0o777
}

/// Sets the permission bits `path` carries.
fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("setting the mode");
}

/// The directory the scratch log lives in.
fn log_dir(scratch: &Scratch) -> PathBuf {
    scratch.path.join("logs")
}

#[test]
fn test_assert_private_permissions_accepts_the_modes_the_plugin_creates() {
    let scratch = Scratch::new("perms-clean");
    let path = scratch.log();
    append(&path, "INFO ime_core: candidates=3");
    set_mode(&log_dir(&scratch), 0o700);
    set_mode(&path, 0o600);
    // The fixture is asserted on as well: a scratch directory created with the process umask
    // would otherwise pass for a reason that has nothing to do with the check.
    assert_eq!(mode_bits(&log_dir(&scratch)), 0o700);
    assert_eq!(mode_bits(&path), 0o600);

    let tap = LogTap::at(&path);

    tap.assert_private_permissions()
        .expect("the modes the layout creates");
}

#[test]
fn test_assert_private_permissions_reports_a_file_a_second_account_can_read() {
    let scratch = Scratch::new("perms-file");
    let path = scratch.log();
    append(&path, "INFO ime_core: candidates=3");
    set_mode(&log_dir(&scratch), 0o700);
    set_mode(&path, 0o644);
    let tap = LogTap::at(&path);

    let error = tap
        .assert_private_permissions()
        .expect_err("a file a second account can read is a leak");
    let (found, suppressed) = findings(&error);
    assert_eq!(suppressed, 0);
    assert_eq!(found.len(), 1);
    // A mode belongs to the path rather than to one of its lines.
    assert_eq!(found[0].line, None);
    assert_eq!(
        found[0].denial,
        Denial::WorldAccessible {
            mode: 0o644,
            want: 0o600
        }
    );
    let message = error.to_string();
    assert!(message.contains("0644"), "{message}");
    assert!(
        message.contains("log-permission"),
        "the report says which rule broke: {message}"
    );
    assert!(
        message.contains("rspinyin.log"),
        "the finding names the path: {message}"
    );
}

#[test]
fn test_assert_private_permissions_reports_a_directory_a_second_account_can_traverse() {
    let scratch = Scratch::new("perms-dir");
    let path = scratch.log();
    append(&path, "INFO ime_core: candidates=3");
    set_mode(&log_dir(&scratch), 0o755);
    set_mode(&path, 0o600);
    let tap = LogTap::at(&path);

    let error = tap
        .assert_private_permissions()
        .expect_err("a traversable log directory is a leak");
    let (found, _) = findings(&error);
    assert_eq!(found.len(), 1);
    assert!(
        found[0].path.ends_with("logs"),
        "the finding points at the directory: {}",
        found[0].path.display()
    );
    assert_eq!(
        found[0].denial,
        Denial::WorldAccessible {
            mode: 0o755,
            want: 0o700
        }
    );
}

#[test]
fn test_assert_private_permissions_covers_a_rolled_sibling() {
    let scratch = Scratch::new("perms-rolled");
    let path = scratch.log();
    append(&path, "INFO ime_core: candidates=3");
    set_mode(&log_dir(&scratch), 0o700);
    set_mode(&path, 0o600);

    roll(&path);
    append(&path, "INFO ime_core: candidates=4");
    set_mode(&path, 0o600);
    set_mode(&sibling(&path, 1), 0o640);

    let tap = LogTap::at(&path);

    let error = tap
        .assert_private_permissions()
        .expect_err("a rolled sibling a group can read is a leak");
    let (found, _) = findings(&error);
    assert_eq!(found.len(), 1);
    assert!(
        found[0].path.ends_with("rspinyin.log.1"),
        "the finding points at the rolled file: {}",
        found[0].path.display()
    );
    assert_eq!(
        found[0].denial,
        Denial::WorldAccessible {
            mode: 0o640,
            want: 0o600
        }
    );
}

#[test]
fn test_assert_private_permissions_accepts_a_file_its_owner_narrowed_further() {
    let scratch = Scratch::new("perms-narrow");
    let path = scratch.log();
    append(&path, "INFO ime_core: candidates=3");
    set_mode(&log_dir(&scratch), 0o500);
    set_mode(&path, 0o400);
    let tap = LogTap::at(&path);

    tap.assert_private_permissions()
        .expect("a narrower mode keeps a second account out");

    // The scratch directory removes itself when the test ends, which needs write permission on
    // the directory the modes above took away.
    set_mode(&log_dir(&scratch), 0o700);
    set_mode(&path, 0o600);
}
