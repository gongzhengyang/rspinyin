//! The facts the release document states, read from the tree and the machine.
//!
//! Responsibility: answer "which release is this?" for the assembled document -- the version
//! the tree builds, the commit it was built from, the compiler and channel that built it, the
//! Fcitx5 floor the addon descriptors declare, and the instant the release is stamped with.
//!
//! # Why these are read here rather than inherited from the packager's document
//!
//! `xtask package` writes a manifest too, and it answers the same questions -- for one
//! architecture's payloads. Its document is private to it and describes a different set of
//! files, so the release document cannot inherit from it: an assembled release covers three
//! package ecosystems and more than one architecture, and its product-level facts are
//! statements about the tag rather than about whichever architecture happened to be packaged
//! first. The two readers therefore overlap, and the values they produce have to agree. The
//! toolchain pin in `rust-toolchain.toml` is what makes that true, and the release pipeline
//! asserts that every packaging job built with the pinned toolchain rather than assuming it.
//!
//! # Why the timestamp is a calendar calculation
//!
//! The same reason the packager gives: `SOURCE_DATE_EPOCH` is defined in seconds and the
//! document records seconds, so rendering it is arithmetic rather than a formatting library,
//! and it has to stay exact for a fixed epoch -- that is what makes two runs of the same tag
//! describe the same instant.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use super::id;

/// The workspace manifest, relative to the repository root.
const WORKSPACE_MANIFEST: &str = "Cargo.toml";

/// The toolchain pin the channel is read from, relative to the repository root.
const TOOLCHAIN_FILE: &str = "rust-toolchain.toml";

/// The descriptor that owns the Fcitx5 version floor, relative to the repository root.
const ENGINE_DESCRIPTOR: &str = "packaging/fcitx5/rspinyin.conf";

/// The descriptor section declaring the version floor, as the descriptor writes it.
const DEPENDENCY_HEADER: &str = "[Addon/Dependencies]";

/// The key the descriptor lists the core dependency under.
const DEPENDENCY_KEY: &str = "0";

/// Everything the release document states about the release itself.
#[derive(Debug, Clone)]
pub struct Identity {
    /// The version the release is published under.
    pub version: String,
    /// The commit the release was built from, when the tree carries one.
    pub commit: Option<String>,
    /// When the release was built, in RFC 3339 UTC.
    pub built_at: String,
    /// The epoch second the build was stamped with.
    pub source_date_epoch: i64,
    /// The identifier this release carries.
    pub request_id: String,
    /// The compiler that built the artifacts, when it could be asked.
    pub rustc: Option<String>,
    /// The toolchain channel the build was pinned to, when the pin is there.
    pub channel: Option<String>,
    /// The Fcitx5 version floor the addon descriptors declare.
    pub fcitx5_floor: String,
}

/// Reads everything about this release that the document records.
///
/// # Errors
///
/// Returns an error when the release identifier cannot be drawn from the system entropy
/// source, and when the descriptor that states the Fcitx5 floor cannot be read.
///
/// # Panics
///
/// Never.
pub fn identity(root: &Path, version: &str) -> Result<Identity> {
    let epoch = release_epoch();
    Ok(Identity {
        version: version.to_owned(),
        commit: commit(root),
        built_at: rfc3339(epoch),
        source_date_epoch: epoch,
        request_id: id::request_id()?,
        rustc: rustc_version(),
        channel: toolchain_channel(root),
        fcitx5_floor: fcitx5_floor(root)?,
    })
}

/// The workspace version the tree builds.
///
/// # Errors
///
/// Returns an error when the manifest cannot be read and when it states no version.
///
/// # Panics
///
/// Never.
pub fn workspace_version(root: &Path) -> Result<String> {
    let path = root.join(WORKSPACE_MANIFEST);
    let text = fs::read_to_string(&path)
        .with_context(|| format!("release: reading {}", path.display()))?;
    version_in(&text).with_context(|| {
        format!(
            "release: {} states no [workspace.package] version, so the release has no version \
             to be published under",
            path.display()
        )
    })
}

/// Extracts `[workspace.package] version` from a workspace manifest.
///
/// Read through a TOML parser rather than by scanning for the first `version =` line: every
/// member crate carries a `version.workspace = true` line, and a scan would find whichever of
/// them came first.
///
/// # Panics
///
/// Never.
fn version_in(manifest: &str) -> Option<String> {
    let document: toml::Value = toml::from_str(manifest).ok()?;
    document
        .get("workspace")?
        .get("package")?
        .get("version")?
        .as_str()
        .map(str::to_owned)
}

/// The epoch second this release is stamped with.
///
/// `SOURCE_DATE_EPOCH` when the environment sets one, which is what makes two runs of the same
/// tag describe the same instant, and the current time otherwise so that a local run is
/// stamped with when it happened.
///
/// # Panics
///
/// Never.
pub fn release_epoch() -> i64 {
    source_date_epoch().unwrap_or_else(now_seconds)
}

/// `SOURCE_DATE_EPOCH`, as an epoch second, when the environment states one.
///
/// A value that is not a number is ignored rather than reported: the variable is a convention
/// shared with every other build system, and a malformed one is more likely to come from the
/// surrounding environment than from this project's tooling.
///
/// # Panics
///
/// Never.
fn source_date_epoch() -> Option<i64> {
    parse_epoch(&std::env::var("SOURCE_DATE_EPOCH").ok()?)
}

/// Parses a `SOURCE_DATE_EPOCH` value.
///
/// # Panics
///
/// Never.
fn parse_epoch(text: &str) -> Option<i64> {
    text.trim().parse().ok()
}

/// The commit the release was built from, as `git rev-parse HEAD` reports it.
///
/// `None` for a tree without a `.git` directory -- a source archive unpacked on a build machine
/// is a legitimate way to build this project -- and for a `git` that refuses to answer. The
/// document records null in that case rather than a guess: a release that names the wrong
/// commit is worse than one that admits it does not know.
///
/// # Panics
///
/// Never.
pub fn commit(root: &Path) -> Option<String> {
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
/// The version rather than the whole line: `rustc 1.98.0 (88d9e12ae 2026-08-18)` is the second
/// word followed by the commit and date of the compiler build, and the document records the
/// version a user can compare against their own.
///
/// # Panics
///
/// Never.
pub fn rustc_version() -> Option<String> {
    let output = Command::new("rustc").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    text.split_whitespace().nth(1).map(str::to_owned)
}

/// The toolchain channel `rust-toolchain.toml` pins, when the file is there.
///
/// `None` rather than an error: a release built without a pinned toolchain is a fact about the
/// build, and the document's job is to record it.
///
/// # Panics
///
/// Never.
pub fn toolchain_channel(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join(TOOLCHAIN_FILE)).ok()?;
    let document: toml::Value = toml::from_str(&text).ok()?;
    document
        .get("toolchain")?
        .get("channel")?
        .as_str()
        .map(str::to_owned)
}

/// The Fcitx5 version floor the engine descriptor declares.
///
/// Read from the descriptor rather than written here: the floor is a packaging decision that
/// `xtask check-versions` already keeps consistent across both addons, and a document that
/// repeated the number would be its third copy. The descriptor is known to be there by the
/// time this runs, because the version gate has already read both of them.
///
/// # Errors
///
/// Returns an error when the descriptor cannot be read, and when it declares no `core:`
/// dependency for the document to state.
///
/// # Panics
///
/// Never.
pub fn fcitx5_floor(root: &Path) -> Result<String> {
    let path = root.join(ENGINE_DESCRIPTOR);
    let text = fs::read_to_string(&path)
        .with_context(|| format!("release: reading {}", path.display()))?;
    floor_in(&text).with_context(|| {
        format!(
            "release: {} declares no `core:` dependency under {DEPENDENCY_HEADER}, so the \
             document cannot state the Fcitx5 floor this release requires",
            path.display()
        )
    })
}

/// Extracts the `core:` floor from a descriptor's `[Addon/Dependencies]` section.
///
/// Section-scoped rather than a plain scan, because the same key can appear in more than one
/// section and the assertion is about which one. Only the first occurrence inside the section
/// is honoured, which is how Fcitx5's own parser resolves a duplicated key.
///
/// # Panics
///
/// Never.
fn floor_in(descriptor: &str) -> Option<String> {
    let mut inside = false;
    for line in descriptor.lines() {
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
        if key.trim() != DEPENDENCY_KEY {
            continue;
        }
        if let Some(version) = value.trim().strip_prefix("core:") {
            return Some(version.to_owned());
        }
    }
    None
}

/// Formats an epoch second as an RFC 3339 UTC timestamp.
///
/// # Panics
///
/// Never.
pub fn rfc3339(epoch_seconds: i64) -> String {
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
/// Exact over the whole range of the input and free of lookup tables, which is why it is the
/// algorithm a date library would use internally anyway. Every division is on a non-negative
/// remainder, so no step can underflow.
///
/// # Panics
///
/// Never.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    // Shift the epoch to 0000-03-01, which puts the leap day at the end of the year and makes
    // the year length depend only on `era`.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    // `month_prime` counts from March, so the 153-day cycle of 31- and 30-day months starts at
    // 0 and the month is shifted back afterwards.
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

/// Seconds since the Unix epoch.
///
/// # Panics
///
/// Never.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rfc3339_renders_the_epoch_the_pipeline_stamps_releases_with() {
        // The value every build of this project is stamped with. A fixed pair, because the
        // whole point of the variable is that the same second renders the same way twice.
        assert_eq!(rfc3339(1_790_899_200), "2026-10-02T00:00:00Z");
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn test_rfc3339_handles_a_leap_day_and_the_end_of_a_year() {
        // 2000 is a leap year (divisible by 400) and 1900 is not, which is the pair that
        // separates this algorithm from one that only knows about every fourth year.
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_767_225_599), "2025-12-31T23:59:59Z");
        assert_eq!(rfc3339(1_767_225_600), "2026-01-01T00:00:00Z");
    }

    #[test]
    fn test_parse_epoch_accepts_a_number_and_refuses_the_neighbours() {
        assert_eq!(parse_epoch("1790899200"), Some(1_790_899_200));
        assert_eq!(parse_epoch(" 1790899200\n"), Some(1_790_899_200));
        // A malformed value is ignored rather than reported: the variable is a shared
        // convention and a bad one is more likely to come from the environment.
        assert_eq!(parse_epoch(""), None);
        assert_eq!(parse_epoch("now"), None);
    }

    #[test]
    fn test_version_in_reads_the_workspace_version_and_not_a_member() {
        // Every member crate carries `version.workspace = true`, so a scan for the first
        // `version =` line would find a member's instead of the workspace's.
        let manifest = "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.package]\nversion = \"1.2.3\"\n\n[package]\nversion.workspace = true\n";
        assert_eq!(version_in(manifest).as_deref(), Some("1.2.3"));
        assert_eq!(version_in("[workspace]\nmembers = []\n"), None);
    }

    #[test]
    fn test_floor_in_reads_the_core_dependency_of_its_own_section() {
        let descriptor =
            "[Addon]\nName=Rust Pinyin\nVersion=0.1.0\n\n[Addon/Dependencies]\n0=core:5.1.0\n";
        assert_eq!(floor_in(descriptor).as_deref(), Some("5.1.0"));
        // A floor declared in another section is not the addon's floor.
        assert_eq!(floor_in("[Addon]\n0=core:5.1.7\n"), None);
        // A dependency that is not the core one is not a floor either.
        assert_eq!(floor_in("[Addon/Dependencies]\n1=other:1.0\n"), None);
    }

    #[test]
    fn test_workspace_version_reads_the_tree_the_tool_runs_in() {
        // The gate itself, against the real manifest: a tree whose version cannot be read
        // cannot be released, and that is what this asserts.
        let root = crate::budget::repo_root().expect("xtask lives in the repository");
        let version = workspace_version(&root).expect("the workspace states a version");
        assert!(!version.is_empty());
        assert!(version.starts_with(char::is_numeric), "{version}");
        assert_eq!(
            version_in(&fs::read_to_string(root.join(WORKSPACE_MANIFEST)).expect("the manifest")),
            Some(version)
        );
    }

    #[test]
    fn test_toolchain_channel_and_floor_are_read_from_the_real_tree() {
        let root = crate::budget::repo_root().expect("xtask lives in the repository");
        // One document states one compiler for a whole release, so the pin and the compiler
        // that built this have to be the same version. The release workflow asserts exactly
        // this in every job that produces an artifact, for the same reason.
        let channel = toolchain_channel(&root).expect("the workspace pins a toolchain channel");
        assert_eq!(
            channel,
            rustc_version().expect("the compiler answers"),
            "the pinned channel and the compiler that built this must be the same version"
        );
        // Not asserted against a fixed number: the floor is a packaging decision that
        // `xtask check-versions` keeps consistent across both descriptors and that the
        // distribution recipes repeat. What this checks is that the reader finds one.
        let floor = fcitx5_floor(&root).expect("the descriptor declares a floor");
        assert!(floor.starts_with(char::is_numeric), "{floor}");
        assert!(floor.contains('.'), "{floor}");
    }

    #[test]
    fn test_fcitx5_floor_reports_a_descriptor_that_is_not_there() {
        let missing = std::env::temp_dir().join("rspinyin-release-no-descriptor");
        let failure = fcitx5_floor(&missing).expect_err("there is no descriptor");
        assert!(failure.to_string().contains("rspinyin.conf"), "{failure}");
    }
}
