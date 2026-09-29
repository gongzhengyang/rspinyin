//! The identity of one release: its name, its identifier, its timestamps, and what built
//! it.
//!
//! Responsibility: answer "which release is this?" for the manifest. Everything here is
//! either read from the machine (`rustc --version`, the system clock, the entropy source),
//! read from a file that already owns the fact (`rust-toolchain.toml`, the addon
//! descriptor) or derived from those.
//!
//! # Why the timestamp is not a dependency
//!
//! Two dates are needed: the epoch second the archive's members are stamped with, and an
//! RFC 3339 rendering of it for the manifest. `SOURCE_DATE_EPOCH` is defined in seconds
//! and the manifest records seconds, so the conversion is a calendar calculation rather
//! than a formatting library -- and it has to stay exact for a fixed epoch, because that
//! is what makes two builds of the same commit agree. The same reasoning covers the ULID:
//! one identifier per release, 26 characters of a fixed alphabet.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

/// Bytes of randomness in a ULID.
const ENTROPY_BYTES: usize = 10;

/// The system entropy source.
const ENTROPY_SOURCE: &str = "/dev/urandom";

/// The toolchain pin the channel is read from, relative to the repository root.
const TOOLCHAIN_FILE: &str = "rust-toolchain.toml";

/// The descriptor that owns the Fcitx5 version floor, relative to the repository root.
const ENGINE_DESCRIPTOR: &str = "packaging/fcitx5/rspinyin.conf";

/// The descriptor section declaring the version floor, as the descriptor writes it.
const DEPENDENCY_HEADER: &str = "[Addon/Dependencies]";

/// The alphabet ULIDs are written in: Crockford base32, without `I`, `L`, `O` and `U`.
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The name of the archive a release produces, without its extension.
///
/// `rspinyin-<version>-<arch>`, which is also the name of the directory the archive
/// unpacks into, so a user can tell two releases apart without opening either.
pub(super) fn archive_stem(version: &str, arch: &str) -> String {
    format!("rspinyin-{version}-{arch}")
}

/// The epoch second this release is stamped with.
///
/// `SOURCE_DATE_EPOCH` when the environment sets one -- which is what makes two builds of
/// the same commit agree on the archive's member timestamps -- and the current time
/// otherwise, so a developer's local build is still stamped with when it happened.
pub(super) fn release_epoch() -> i64 {
    source_date_epoch().unwrap_or_else(now_seconds)
}

/// `SOURCE_DATE_EPOCH`, as an epoch second, when the environment states one.
///
/// A value that is not a number is ignored rather than reported: the variable is a
/// convention shared with every other build system, and a malformed one is more likely to
/// come from the surrounding environment than from this project's tooling.
fn source_date_epoch() -> Option<i64> {
    std::env::var("SOURCE_DATE_EPOCH").ok()?.trim().parse().ok()
}

/// A fresh identifier for this release.
///
/// # Errors
///
/// Returns an error when the system entropy source cannot be read, which would otherwise
/// produce an identifier that is only as unique as the clock.
pub(super) fn request_id() -> Result<String> {
    Ok(ulid(now_millis(), entropy()?))
}

/// A ULID: 48 bits of millisecond timestamp followed by 80 bits of randomness, in
/// Crockford base32, 26 characters long.
///
/// The `ulid` crate is the ecosystem default for this and would be the right dependency
/// for a program whose job is identifiers. This one needs exactly one identifier per
/// release, and the format is a fixed 26-character encoding over a fixed alphabet, so it
/// is written out here rather than adding a dependency to the packaging tool for it.
/// Sorting the identifiers still orders the releases by the millisecond they were made in,
/// which is the property the format exists for.
pub(super) fn ulid(timestamp_ms: u64, entropy: [u8; ENTROPY_BYTES]) -> String {
    let mut id = String::with_capacity(26);
    for index in 0..10 {
        let shift = 45 - 5 * index;
        id.push(CROCKFORD[((timestamp_ms >> shift) & 0x1F) as usize] as char);
    }
    let mut random: u128 = 0;
    for byte in entropy {
        random = (random << 8) | u128::from(byte);
    }
    for index in 0..16 {
        let shift = 75 - 5 * index;
        id.push(CROCKFORD[((random >> shift) & 0x1F) as usize] as char);
    }
    id
}

/// Ten bytes from the system entropy source.
///
/// # Errors
///
/// Returns an error when the source cannot be opened or does not yield its bytes.
fn entropy() -> Result<[u8; ENTROPY_BYTES]> {
    let mut bytes = [0u8; ENTROPY_BYTES];
    let mut source = fs::File::open(ENTROPY_SOURCE)
        .with_context(|| format!("opening {ENTROPY_SOURCE} for the release identifier"))?;
    source
        .read_exact(&mut bytes)
        .with_context(|| format!("reading {ENTROPY_BYTES} bytes from {ENTROPY_SOURCE}"))?;
    Ok(bytes)
}

/// Milliseconds since the Unix epoch.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Seconds since the Unix epoch.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// Formats an epoch second as an RFC 3339 UTC timestamp.
pub(super) fn rfc3339(epoch_seconds: i64) -> String {
    let days = epoch_seconds.div_euclid(86_400);
    let seconds = epoch_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3_600,
        (seconds / 60) % 60,
        seconds % 60
    )
}

/// The civil date `days` after 1970-01-01, by Howard Hinnant's `civil_from_days`.
///
/// Exact over the whole range of the input and free of lookup tables, which is why it is
/// the algorithm a date library would use internally anyway. Every division is on a
/// non-negative remainder, so no step can underflow.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    // Shift the epoch to 0000-03-01, which puts the leap day at the end of the year and
    // makes the year length depend only on `era`.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    // `month_prime` counts from March, so the 153-day cycle of 31- and 30-day months
    // starts at 0 and the month is shifted back afterwards.
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

/// The commit the release was built from, as `git rev-parse HEAD` reports it.
///
/// `None` for a tree without a `.git` directory -- a source tarball unpacked on a build
/// machine is a legitimate way to build this project -- and for a `git` that refuses to
/// answer. The manifest records `null` in that case rather than a guess: a release that
/// names the wrong commit is worse than one that admits it does not know.
pub(super) fn commit(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let hash = String::from_utf8(output.stdout).ok()?;
    let hash = hash.trim();
    (!hash.is_empty()).then(|| hash.to_owned())
}

/// The compiler that built the artifacts, as `rustc --version` reports it.
///
/// The version rather than the whole line: `rustc 1.98.0 (88d9e12ae 2026-08-18)` is the
/// second word followed by the commit and date of the compiler build, and the manifest
/// records the version a user can compare against their own.
pub(super) fn rustc_version() -> Option<String> {
    let output = Command::new("rustc").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    text.split_whitespace().nth(1).map(str::to_owned)
}

/// The toolchain channel `rust-toolchain.toml` pins, when the file is there.
///
/// `None` rather than an error: a release built without a pinned toolchain is a fact
/// about the build, and the manifest's job is to record it.
pub(super) fn toolchain_channel(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join(TOOLCHAIN_FILE)).ok()?;
    let document: toml::Value = toml::from_str(&text).ok()?;
    let channel = document.get("toolchain")?.get("channel")?;
    channel.as_str().map(str::to_owned)
}

/// The Fcitx5 version floor the engine descriptor declares.
///
/// Read from the descriptor rather than written here: the floor is a packaging decision
/// that `xtask check-versions` already keeps consistent across both addons, and a manifest
/// that repeated the number would be its third copy. The descriptor is known to be there
/// by the time this runs -- the version gate has already read both of them.
///
/// # Errors
///
/// Returns an error when the descriptor cannot be read, and when it declares no `core:`
/// dependency for the manifest to state.
pub(super) fn fcitx5_floor(root: &Path) -> Result<String> {
    let path = root.join(ENGINE_DESCRIPTOR);
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == DEPENDENCY_HEADER;
            continue;
        }
        if !inside {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "0" {
            continue;
        }
        if let Some(version) = value.trim().strip_prefix("core:") {
            return Ok(version.to_owned());
        }
    }
    anyhow::bail!(
        "{} declares no `core:` dependency in {DEPENDENCY_HEADER}, so the manifest cannot \
         state the Fcitx5 floor this release requires",
        path.display()
    )
}
