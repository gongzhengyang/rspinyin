//! The release manifest, rendered.
//!
//! Responsibility: turn the facts about a release and the set of files it publishes into the
//! document `docs/dev/opt-deploy.md` section 3.3.1 fixes, and write it beside the files.
//!
//! # Why the digests are read from the files that are about to ship
//!
//! The same reason the packager gives: a digest taken from a caller records what the caller
//! said rather than what is there. Every `sha256` and `size_bytes` below is read from the file
//! in the release directory in the same pass that writes the record for it, so a mismatch
//! cannot exist without the write failing.
//!
//! # Why no published file records a size ceiling
//!
//! `size_budget_mb` is the ceiling an artifact was measured against, and the budgets of this
//! project cover the payloads -- the addon libraries and the dictionary -- not the archives
//! and packages that carry them. The packager measures those payloads, per architecture,
//! before it writes its own document; a release archive is a container whose size follows from
//! them, and a ceiling invented for it here would be a number nothing measured. The field is
//! null rather than a guess, which is the value the schema reserves for exactly this.
//!
//! # Why no published file lists exports
//!
//! `exports` names the symbols a consumer can resolve from a file with `dlsym`, and nothing a
//! release publishes is a library: the addon libraries are inside the archives and inside the
//! packages, and the packager records their exports in the per-architecture document that
//! describes them. An archive is not an ELF image, and `xtask verify` would report
//! `dist/verify/factory-symbol-missing` for one that claimed otherwise.
//!
//! # Why the signature block is null here
//!
//! The signing step records the key, and it has to happen before the checksum list is written
//! because the list covers the manifest. A key id written here would be a key id recorded
//! before anything was signed -- a claim, in the document a user reads, that no signature
//! stood behind yet.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::artifacts::{Published, Set};
use super::facts::Identity;

/// Schema version of the manifest this module writes.
pub const MANIFEST_VERSION: u64 = 1;

/// File name of the manifest inside the release directory.
pub const MANIFEST_FILE: &str = "rspinyin-release.json";

/// The glibc floor the platform baseline states.
///
/// The distribution matrix in the specification owns this number; the manifest repeats it
/// because a user reads the manifest and not the specification, and a distribution packager
/// reads it to decide what to depend on. `xtask package` states the same value in its own
/// document, and the two must not drift.
const GLIBC_MIN: &str = "2.35";

/// The session tiers a release is expected to work on.
///
/// A product statement rather than a measurement: the design commits to one X11 session and
/// three Wayland compositors, and verifying each is its own task. Recording them is what lets
/// a user see which sessions the release claims instead of inferring it from the code.
const SESSION_TIERS: [&str; 4] = ["x11", "wayland-wlroots", "wayland-kwin", "wayland-mutter"];

/// Writes the manifest into the release directory `set` was read from.
///
/// The file name is not a parameter: the document and the artifacts it describes are
/// published together, and a manifest written somewhere else is a record separated from what
/// it records. The path is returned so the caller can report where it went.
///
/// # Errors
///
/// Returns an error when a published file cannot be read, and when the document cannot be
/// written.
///
/// # Panics
///
/// Never.
pub fn write(identity: &Identity, set: &Set) -> Result<PathBuf> {
    let document = render(identity, set)?;
    let path = set.dir().join(MANIFEST_FILE);
    fs::write(&path, document).with_context(|| format!("release: writing {}", path.display()))?;
    Ok(path)
}

/// Renders the document, without writing it.
///
/// Separate from [`write`] so that the schema can be asserted without a file, and so that
/// writing is the only part of this module that touches the filesystem.
///
/// # Errors
///
/// Returns an error when a published file cannot be read.
///
/// # Panics
///
/// Never.
pub fn render(identity: &Identity, set: &Set) -> Result<String> {
    let artifacts: Vec<Value> = set
        .files()
        .iter()
        .map(|file| record(set.dir(), file))
        .collect::<Result<_>>()?;
    let architectures: Vec<&str> = set.architectures().iter().map(|arch| arch.name()).collect();
    let document = json!({
        "manifest_version": MANIFEST_VERSION,
        "request_id": identity.request_id,
        "release": {
            "version": identity.version,
            "commit": identity.commit,
            "built_at": identity.built_at,
            "source_date_epoch": identity.source_date_epoch,
        },
        "toolchain": {
            "rustc": identity.rustc,
            "channel": identity.channel,
            "glibc_min": GLIBC_MIN,
        },
        "compatibility": {
            "fcitx5": format!(">={}", identity.fcitx5_floor),
            "architectures": architectures,
            "session_tiers": SESSION_TIERS,
        },
        "artifacts": artifacts,
        "signature": Value::Null,
    });
    // Pretty-printed with a trailing newline, which is what the packager writes and what the
    // signing step's own rewrite of the document preserves.
    Ok(format!("{document:#}\n"))
}

/// The record of one published file, with its digest and size read from the file itself.
///
/// # Errors
///
/// Returns an error when the file cannot be read.
///
/// # Panics
///
/// Never.
fn record(dir: &Path, file: &Published) -> Result<Value> {
    let path = dir.join(&file.name);
    Ok(json!({
        "name": file.name,
        "role": file.role.label(),
        "sha256": sha256(&path)?,
        "size_bytes": size_bytes(&path)?,
        "size_budget_mb": Value::Null,
        "exports": Vec::<String>::new(),
    }))
}

/// The SHA-256 digest of a file, as lower-case hexadecimal.
///
/// # Errors
///
/// Returns an error when the file cannot be read.
///
/// # Panics
///
/// Never.
fn sha256(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("release: reading {}", path.display()))?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// The size of a file in bytes.
///
/// # Errors
///
/// Returns an error when the file cannot be read.
///
/// # Panics
///
/// Never.
fn size_bytes(path: &Path) -> Result<u64> {
    Ok(fs::metadata(path)
        .with_context(|| format!("release: reading {}", path.display()))?
        .len())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use serde_json::Map;

    use super::super::artifacts::{Architecture, PackageKind};

    /// A scratch directory unique to this test process and tag.
    fn scratch(tag: &str) -> PathBuf {
        let name = format!("rspinyin-release-doc-{tag}-{}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        dir
    }

    /// The facts a fixture document is rendered from.
    fn identity() -> Identity {
        Identity {
            version: "0.1.0".to_owned(),
            commit: Some("ee0dbfb1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7".to_owned()),
            built_at: "2026-10-02T00:00:00Z".to_owned(),
            source_date_epoch: 1_790_899_200,
            request_id: "01J8ZQ4K7N3M2P8R5T6V9W0X1Y".to_owned(),
            rustc: Some("1.98.0".to_owned()),
            channel: Some("1.98.0".to_owned()),
            fcitx5_floor: "5.1.0".to_owned(),
        }
    }

    /// A release directory holding the two files the fixtures publish.
    fn release(tag: &str) -> (PathBuf, Set) {
        let dir = scratch(tag);
        fs::write(dir.join("rspinyin-0.1.0.tar.gz"), b"the source archive")
            .expect("writing the archive");
        fs::write(dir.join("rspinyin-signing-key.asc"), b"the public key")
            .expect("writing the key");
        let set = Set::read(&dir, "0.1.0", &[]).expect("the release publishes both files");
        (dir, set)
    }

    /// The rendered document of a fixture release, as a JSON object.
    fn document(tag: &str) -> (PathBuf, Map<String, Value>) {
        let (dir, set) = release(tag);
        let text = render(&identity(), &set).expect("the document renders");
        let parsed: Value = serde_json::from_str(&text).expect("the document is JSON");
        let object = parsed
            .as_object()
            .expect("the document is a JSON object")
            .clone();
        (dir, object)
    }

    #[test]
    fn test_render_carries_every_field_the_schema_requires() {
        // The field set is the schema of the delivery contract, and `xtask verify` refuses a
        // manifest that omits any of it -- an absent field would skip the check behind it.
        let (dir, object) = document("schema");
        for key in [
            "manifest_version",
            "request_id",
            "release",
            "toolchain",
            "compatibility",
            "artifacts",
            "signature",
        ] {
            assert!(object.contains_key(key), "{key} is missing");
        }
        assert_eq!(object["manifest_version"], json!(MANIFEST_VERSION));
        assert_eq!(object["request_id"], json!("01J8ZQ4K7N3M2P8R5T6V9W0X1Y"));
        assert_eq!(object["signature"], Value::Null, "nothing is signed yet");

        let release = object["release"].as_object().expect("a release block");
        for key in ["version", "commit", "built_at", "source_date_epoch"] {
            assert!(release.contains_key(key), "release.{key} is missing");
        }
        let toolchain = object["toolchain"].as_object().expect("a toolchain block");
        for key in ["rustc", "channel", "glibc_min"] {
            assert!(toolchain.contains_key(key), "toolchain.{key} is missing");
        }
        assert_eq!(toolchain["glibc_min"], json!(GLIBC_MIN));

        let compatibility = object["compatibility"]
            .as_object()
            .expect("a compatibility block");
        for key in ["fcitx5", "architectures", "session_tiers"] {
            assert!(
                compatibility.contains_key(key),
                "compatibility.{key} is missing"
            );
        }
        assert_eq!(compatibility["fcitx5"], json!(">=5.1.0"));
        assert_eq!(
            compatibility["session_tiers"],
            json!(SESSION_TIERS),
            "the four session tiers the design commits to"
        );
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_render_records_a_digest_and_size_that_match_the_file() {
        let (dir, set) = release("digest");
        let text = render(&identity(), &set).expect("the document renders");
        let parsed: Value = serde_json::from_str(&text).expect("the document is JSON");
        let artifacts = parsed["artifacts"].as_array().expect("an artifact list");
        assert_eq!(artifacts.len(), 2);

        for artifact in artifacts {
            let name = artifact["name"].as_str().expect("a name");
            let bytes = fs::read(dir.join(name)).expect("the fixture file is there");
            let digest = Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(artifact["sha256"], json!(digest), "{name}");
            assert_eq!(artifact["size_bytes"], json!(bytes.len() as u64), "{name}");
            // No budget covers an archive or a key, and a number invented here would be one
            // that no measurement stands behind.
            assert_eq!(artifact["size_budget_mb"], Value::Null, "{name}");
            assert_eq!(artifact["exports"], json!([]), "{name}");
        }

        let roles: Vec<&str> = artifacts
            .iter()
            .map(|artifact| artifact["role"].as_str().expect("a role"))
            .collect();
        assert_eq!(roles, ["source-archive", "signing-key"]);
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_render_names_the_architectures_the_packages_were_built_for() {
        let dir = scratch("architectures");
        fs::write(dir.join("rspinyin-0.1.0.tar.gz"), b"archive").expect("writing the archive");
        fs::write(dir.join("rspinyin-signing-key.asc"), b"key").expect("writing the key");
        fs::write(dir.join("rspinyin_0.1.0-1_amd64.deb"), b"deb").expect("writing the deb");
        fs::write(dir.join("rspinyin_0.1.0-1_arm64.deb"), b"deb").expect("writing the deb");
        let set = Set::read(
            &dir,
            "0.1.0",
            &[
                (PackageKind::Deb, Architecture::X86_64),
                (PackageKind::Deb, Architecture::Aarch64),
            ],
        )
        .expect("the release is complete");

        let parsed: Value =
            serde_json::from_str(&render(&identity(), &set).expect("the document renders"))
                .expect("the document is JSON");
        assert_eq!(
            parsed["compatibility"]["architectures"],
            json!(["x86_64", "aarch64"]),
            "the architectures are the ones the packages were built for"
        );
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_write_puts_the_document_in_the_release_directory() {
        let (dir, set) = release("write");
        let out = write(&identity(), &set).expect("the document is written");
        assert_eq!(out, dir.join(MANIFEST_FILE));
        let written = fs::read_to_string(&out).expect("the file is there");
        assert!(written.ends_with("}\n"), "the document ends with a newline");
        assert!(written.contains("\"manifest_version\": 1"), "{written}");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_render_reports_a_published_file_that_is_not_there() {
        // The directory is read and the files are read separately, and the second read is
        // what fails when something removes a file between the two.
        let (dir, set) = release("vanished");
        fs::remove_file(dir.join("rspinyin-0.1.0.tar.gz")).expect("removing the archive");
        let failure = render(&identity(), &set).expect_err("the archive is gone");
        assert!(
            failure.to_string().contains("rspinyin-0.1.0.tar.gz"),
            "{failure}"
        );
        fs::remove_dir_all(&dir).expect("cleaning up");
    }
}
