//! Naming and moving a damaged file aside.
//!
//! Responsibility: turn "this file is damaged" into "this file is now
//! `<name>.corrupt.<stamp>`", choosing a name that is free and claiming it before the
//! rename. Splitting this out of [`super`] is what keeps the recovery flow's own file
//! about the flow: everything here is about names, and the flow never has to think
//! about them.
//!
//! Boundaries: it moves a file and nothing else. It does not decide *whether* a file is
//! damaged, it does not read or write a file's contents, and it does not create a
//! replacement -- the caller does all three. A move that fails is reported as the
//! underlying [`std::io::Error`] and leaves the damaged file exactly where it was.
//!
//! # Why the name is claimed before the rename
//!
//! `rename(2)` replaces its destination silently, so a plain `rename` to a name that
//! already holds an earlier quarantine would destroy that earlier file -- the very
//! loss this module exists to prevent, caused by the module itself. The destination is
//! therefore created with `O_EXCL` first: creating it either fails with `EEXIST`, which
//! sends the caller to the next name, or succeeds, which reserves the name against
//! every other writer. The rename then replaces the empty file it just made. A crash
//! between the two steps leaves an empty `.corrupt.` file behind, which costs nothing:
//! the damaged file is still where it was, and the next attempt takes the next name.

use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use super::QUARANTINE_MARK;

/// Number of `.n` suffixes tried after the plain quarantine name.
///
/// The suffix exists so that two corruptions in one second do not land on the same
/// name. A hundred of them in one second is not a case worth more code, and running out
/// is reported rather than resolved by overwriting anything.
const MAX_COLLISION_SUFFIX: u32 = 99;

/// Mode the reserved name carries. It only exists between the reservation and the
/// rename, after which the name holds the damaged file with the mode that file had.
const CLAIM_MODE: u32 = 0o600;

/// Moves `target` to its quarantine name and returns where it went.
///
/// # Parameters
/// - `target`: the damaged file; it is removed from its own name and appears under the
///   returned one.
/// - `stamp`: the second the damage was found, as [`super::unix_secs`] reads it.
///
/// # Return value
/// The path the file now has, which is `target`'s name with
/// [`QUARANTINE_MARK`](super::QUARANTINE_MARK) and `stamp` appended, plus a `.n`
/// suffix when that name was taken.
///
/// # Errors
/// Returns the underlying [`std::io::Error`] when `target` does not exist, when every
/// candidate name is taken, or when the rename fails -- the last of which is the
/// ordinary case of a file in a directory the user cannot write to, and leaves `target`
/// untouched.
pub(super) fn move_aside(target: &Path, stamp: u64) -> std::io::Result<PathBuf> {
    let destination = claim_free_name(target, stamp)?;
    match fs::rename(target, &destination) {
        Ok(()) => Ok(destination),
        Err(error) => {
            // The reservation is an empty file this module created, so it is this
            // module's to remove; leaving it behind would make the next attempt skip a
            // name that holds nothing.
            let _ = fs::remove_file(&destination);
            Err(error)
        }
    }
}

/// Reserves the first free quarantine name beside `target` and returns it.
fn claim_free_name(target: &Path, stamp: u64) -> std::io::Result<PathBuf> {
    let base = quarantine_base(target, stamp)?;
    for suffix in 0..=MAX_COLLISION_SUFFIX {
        let candidate = if suffix == 0 {
            base.clone()
        } else {
            suffixed(&base, suffix)
        };
        match claim(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        ErrorKind::AlreadyExists,
        format!("every quarantine name beside {} is taken", target.display()),
    ))
}

/// Returns `<name>` + [`QUARANTINE_MARK`](super::QUARANTINE_MARK) + `stamp` beside
/// `target`.
///
/// The suffix is appended to the file name rather than to the whole path, so the
/// quarantine is a sibling of the file it replaces: a rename across directories is not
/// atomic on every filesystem, and staying in the same directory is what makes this one
/// atomic.
fn quarantine_base(target: &Path, stamp: u64) -> std::io::Result<PathBuf> {
    let name = target.file_name().ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            "the path names no file to quarantine",
        )
    })?;
    let mut quarantined = name.to_os_string();
    quarantined.push(QUARANTINE_MARK);
    quarantined.push(stamp.to_string());
    Ok(target.with_file_name(quarantined))
}

/// Appends `.suffix` to a path's last component.
fn suffixed(base: &Path, suffix: u32) -> PathBuf {
    let mut name = base.as_os_str().to_os_string();
    name.push(format!(".{suffix}"));
    PathBuf::from(name)
}

/// Reserves `path` by creating it exclusively.
///
/// # Errors
/// Returns [`ErrorKind::AlreadyExists`] when the name is taken, and any other failure of
/// the underlying `open` as it stands.
fn claim(path: &Path) -> std::io::Result<()> {
    let reserved = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(CLAIM_MODE)
        .open(path)?;
    // Closed at once: the name is what is being reserved, and the descriptor would
    // otherwise be open across the rename that replaces this very file.
    drop(reserved);
    Ok(())
}
