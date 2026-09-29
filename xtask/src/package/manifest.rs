//! The release manifest `rspinyin-release.json`, and the checksum list beside it.
//!
//! Responsibility: describe a release in the structured form the delivery contract fixes
//! -- what it is, what built it, which sessions it claims, and one record per shipped
//! file carrying the file's digest, its size, its size ceiling and the symbols a consumer
//! can resolve from it -- and write it next to the artifacts.
//!
//! # Why the digests are read here rather than taken from the caller
//!
//! A manifest whose digests were computed somewhere else records that other computation,
//! not the file. Every digest in this module is read from the file that is about to ship,
//! in the same pass that writes the record, so a mismatch between the two cannot exist
//! without the write failing.
//!
//! # Why the two product statements are constants
//!
//! `glibc_min` and `session_tiers` are statements about the product rather than
//! measurements of the artifact: the platform baseline owns the first and the design owns
//! the second. Everything else in the document is read from the machine, from a file that
//! already owns the fact, or from the artifact itself.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::stamp;

/// Schema version of the manifest this build writes.
pub(super) const MANIFEST_VERSION: u64 = 1;

/// File name of the manifest inside the release directory.
pub(super) const MANIFEST_FILE: &str = "rspinyin-release.json";

/// File name of the checksum list inside the release directory.
pub(super) const CHECKSUMS_FILE: &str = "SHA256SUMS";

/// The glibc floor the platform baseline states.
///
/// The distribution matrix in the specification owns this number; the manifest has to
/// repeat it because a user reads the manifest and not the specification, and a
/// distribution packager reads it to decide what to depend on.
const GLIBC_MIN: &str = "2.35";

/// The session tiers a release is expected to work on.
///
/// A product statement, not a measurement: the design commits to one X11 session and
/// three Wayland compositors, and verifying each is its own task. Recording them is what
/// lets a user see which sessions the release claims instead of inferring it from the
/// code.
const SESSION_TIERS: [&str; 4] = ["x11", "wayland-wlroots", "wayland-kwin", "wayland-mutter"];

/// One file the release ships.
#[derive(Debug, Clone)]
pub(super) struct Artifact {
    /// File name in the release directory and inside the archive.
    pub(super) name: String,
    /// What the file is for, as the manifest records it.
    pub(super) role: &'static str,
    /// Path of the staged file the record is read from.
    pub(super) path: PathBuf,
    /// The size threshold the file was measured against, in mebibytes.
    pub(super) budget_mb: Option<f64>,
    /// Symbols the file exports, as read back from its dynamic symbol table.
    pub(super) exports: Vec<String>,
}

impl Artifact {
    /// The artifact's record, with its digest and size read from the staged file.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read.
    pub(super) fn record(&self) -> Result<Value> {
        let sha256 = sha256_file(&self.path)?;
        let size_bytes = size_bytes(&self.path)?;
        Ok(json!({
            "name": self.name,
            "role": self.role,
            "sha256": sha256,
            "size_bytes": size_bytes,
            "size_budget_mb": self.budget_mb,
            "exports": self.exports,
        }))
    }
}

/// The document a release publishes beside its artifacts.
#[derive(Debug, Clone)]
pub(super) struct Release {
    /// Workspace version this release was built from.
    pub(super) version: String,
    /// Architecture the release is labelled with.
    pub(super) arch: String,
    /// Identifier unique to this release.
    pub(super) request_id: String,
    /// When the release was built, in RFC 3339 UTC.
    pub(super) built_at: String,
    /// The epoch second the build was stamped with.
    pub(super) source_date_epoch: i64,
    /// Commit the release was built from, when the tree carries one.
    pub(super) commit: Option<String>,
    /// Compiler that built the artifacts, when it could be asked.
    pub(super) rustc: Option<String>,
    /// Toolchain channel the build was pinned to, when the pin is there.
    pub(super) channel: Option<String>,
    /// Fcitx5 version floor the addon descriptors declare.
    pub(super) fcitx5_floor: String,
    /// One record per shipped file.
    pub(super) artifacts: Vec<Artifact>,
}

impl Release {
    /// Reads everything about this release that is not already known.
    ///
    /// # Errors
    ///
    /// Returns an error when the release identifier cannot be drawn from the system
    /// entropy source, and when the addon descriptor that states the Fcitx5 floor cannot
    /// be read.
    pub(super) fn capture(
        version: &str,
        arch: &str,
        artifacts: &[Artifact],
        epoch: i64,
    ) -> Result<Self> {
        let root = crate::budget::repo_root()?;
        let commit = stamp::commit(&root);
        if commit.is_none() {
            println!(
                "package: no commit recorded: `git rev-parse HEAD` did not answer for {}",
                root.display()
            );
        }
        Ok(Self {
            version: version.to_owned(),
            arch: arch.to_owned(),
            request_id: stamp::request_id()?,
            built_at: stamp::rfc3339(epoch),
            source_date_epoch: epoch,
            commit,
            rustc: stamp::rustc_version(),
            channel: stamp::toolchain_channel(&root),
            fcitx5_floor: stamp::fcitx5_floor(&root)?,
            artifacts: artifacts.to_vec(),
        })
    }

    /// Renders the document.
    ///
    /// `signature` is `null`: a release carries no detached signature until the signing
    /// step runs, and a placeholder key id would read like a signature that was checked.
    ///
    /// # Errors
    ///
    /// Returns an error when an artifact's digest cannot be read.
    pub(super) fn to_json(&self) -> Result<String> {
        let artifacts: Vec<Value> = self
            .artifacts
            .iter()
            .map(Artifact::record)
            .collect::<Result<_>>()?;
        let document = json!({
            "manifest_version": MANIFEST_VERSION,
            "request_id": self.request_id,
            "release": {
                "version": self.version,
                "commit": self.commit,
                "built_at": self.built_at,
                "source_date_epoch": self.source_date_epoch,
            },
            "toolchain": {
                "rustc": self.rustc,
                "channel": self.channel,
                "glibc_min": GLIBC_MIN,
            },
            "compatibility": {
                "fcitx5": format!(">={}", self.fcitx5_floor),
                "architectures": [self.arch],
                "session_tiers": SESSION_TIERS,
            },
            "artifacts": artifacts,
            "signature": Value::Null,
        });
        Ok(format!("{document:#}\n"))
    }

    /// Writes the manifest into `dir`, returning the path it wrote.
    ///
    /// # Errors
    ///
    /// Returns an error when the document cannot be rendered or the file cannot be
    /// written.
    pub(super) fn write(&self, dir: &Path) -> Result<PathBuf> {
        let path = dir.join(MANIFEST_FILE);
        fs::write(&path, self.to_json()?).with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }
}

/// Writes `SHA256SUMS` over `names`, in the format `sha256sum --check` reads.
///
/// The format is that tool's own rather than a private one: the point of the file is that
/// a user can verify a download with what is already on their machine, and that a release
/// signature, when there is one, covers a list every tool can parse.
///
/// # Errors
///
/// Returns an error when a named file cannot be read, and when the list cannot be written.
pub(super) fn write_checksums(dir: &Path, names: &[String]) -> Result<PathBuf> {
    let mut list = String::new();
    for name in names {
        let digest = sha256_file(&dir.join(name))?;
        list.push_str(&format!("{digest}  {name}\n"));
    }
    let path = dir.join(CHECKSUMS_FILE);
    fs::write(&path, list).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// The SHA-256 digest of a file, as lower-case hexadecimal.
///
/// # Errors
///
/// Returns an error when the file cannot be read.
pub(super) fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(hex(&Sha256::digest(bytes)))
}

/// The size of a file in bytes.
///
/// # Errors
///
/// Returns an error when the file cannot be read.
pub(super) fn size_bytes(path: &Path) -> Result<u64> {
    Ok(fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .len())
}

/// Renders a digest as lower-case hexadecimal.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
