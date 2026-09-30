//! The checksum list a release publishes beside its artifacts, and the checks it allows.
//!
//! Responsibility: read `SHA256SUMS` in the format `sha256sum --check` reads, and check the
//! three records of a release against each other and against the files -- the manifest, the
//! checksum list, and the directory.
//!
//! # The check runs in both directions
//!
//! A release is three records of the same set of files: the manifest describes them, the
//! checksum list digests them, and the directory holds them. Comparing one direction only
//! leaves the other open -- a file the manifest names but the directory lacks would pass a
//! list-only check, and a file the directory holds but no record names would pass a
//! manifest-only check. So every pair is compared both ways:
//!
//! | Direction | Failure | Code |
//! |---|---|---|
//! | the manifest names it, the directory lacks it | an artifact is missing | `dist/verify/artifact-missing` |
//! | the manifest names it, the list does not | the file is outside the signed record | `dist/verify/artifact-missing` |
//! | the list names it, the directory lacks it | an artifact is missing | `dist/verify/artifact-missing` |
//! | the directory holds it, no record names it | bytes nothing accounts for | `dist/verify/digest-mismatch` |
//!
//! # Why an unaccounted-for file is a digest failure
//!
//! The contract fixes nine codes and does not add a tenth, so a file the directory holds and
//! no record names has to be reported under one of them. It is reported as
//! `dist/verify/digest-mismatch`: the code's meaning is that the bytes do not agree with the
//! record, and a file with no record at all is the limiting case of that -- there is no
//! digest in the manifest that agrees with these bytes. `dist/verify/artifact-missing` would
//! say the opposite of what happened, and `dist/manifest/malformed` is about the document
//! rather than about the directory.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::error::{Code, VerifyError};
use super::manifest::{Artifact, Manifest, is_file_name, is_sha256};

/// File name of the checksum list inside the release directory.
///
/// Kept in step with the file the packager writes: the signature covers this name, and a
/// verifier looking for a differently named list would report a signed release as unsigned.
pub const CHECKSUMS_FILE: &str = "SHA256SUMS";

/// Bytes in a mebibyte: the unit the manifest states its size ceilings in.
const MEBIBYTE: f64 = 1024.0 * 1024.0;

/// One file that matched the records published for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// File name in the release directory.
    pub name: String,
    /// What the file is for, as the manifest records it.
    pub role: String,
    /// Measured size, in bytes.
    pub size_bytes: u64,
    /// Measured digest, lower-case hexadecimal.
    pub digest: String,
}

/// The checksum list, as it was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checksums {
    /// One entry per line, in the order the list writes them.
    entries: Vec<Entry>,
}

/// One line of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    /// File name the line names.
    name: String,
    /// Digest the line records, lower-case hexadecimal.
    digest: String,
}

impl Checksums {
    /// The digest the list records for `name`, when it records one.
    ///
    /// # Panics
    ///
    /// Never.
    fn digest_of(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.digest.as_str())
    }
}

/// Checks every file in `dir` against the records the release publishes for it.
///
/// # Errors
///
/// Returns `dist/verify/artifact-missing` when the checksum list is not in the directory,
/// when a file a record names is not there, and when a file the manifest names is not in the
/// checksum list; `dist/verify/digest-mismatch` when a file's bytes disagree with the digest
/// or the size recorded for it, when two records disagree with each other, and when the
/// directory holds a file no record names; `dist/verify/size-budget-exceeded` when a file is
/// past the ceiling its record states; and `dist/manifest/malformed` when the checksum list
/// is not a list of records.
pub fn verify(dir: &Path, manifest: &Manifest) -> Result<Vec<Checked>, VerifyError> {
    let list = read(dir)?;
    let exempt = exempt_names(manifest);
    // The order is the order a reader would take the three records in: what the signed list
    // says, then what the manifest says, then what the directory holds. It decides which code
    // a release that fails more than one check is reported under, and the more specific
    // statement is the one worth making -- a file the manifest describes and the list does not
    // is an artifact the signature does not cover, not an unaccounted-for file.
    check_listed_files(dir, &list)?;
    let checked = check_artifacts(dir, manifest, &list)?;
    check_directory_entries(dir, &list, &exempt)?;
    Ok(checked)
}

/// Reads the checksum list from `dir`.
///
/// # Errors
///
/// Returns `dist/verify/artifact-missing` when there is no list, `dist/manifest/malformed`
/// when it cannot be read as text or a line is not a record.
pub fn read(dir: &Path) -> Result<Checksums, VerifyError> {
    let path = dir.join(CHECKSUMS_FILE);
    let text = fs::read_to_string(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            VerifyError::rejected(
                Code::ArtifactMissing,
                format!("{} is not in the release directory", path.display()),
            )
        } else {
            malformed(format!("{} cannot be read: {error}", path.display()))
        }
    })?;
    parse(&text)
}

/// Parses the text of a checksum list.
///
/// The format is `sha256sum`'s own: a digest, then a separator, then the file name. The
/// separator is a space or a space and an asterisk, the latter being the marker that tool
/// writes for the binary mode. Anything else on a line is refused rather than skipped,
/// because a line this build cannot read is a line whose file would otherwise go unchecked.
///
/// # Errors
///
/// Returns `dist/manifest/malformed` when a line is not a record, when a digest is not a
/// SHA-256, when a name is not a file name, and when a file is listed twice.
pub fn parse(text: &str) -> Result<Checksums, VerifyError> {
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        if line.trim().is_empty() {
            continue;
        }
        let Some((digest, rest)) = line.split_once(' ') else {
            return Err(malformed(format!(
                "line {number} of the checksum list records no file name"
            )));
        };
        let digest = digest.to_ascii_lowercase();
        if !is_sha256(&digest) {
            return Err(malformed(format!(
                "line {number} of the checksum list records `{digest}`, which is not a \
                 SHA-256 digest"
            )));
        }
        let name = rest.trim_start_matches([' ', '*']);
        if !is_file_name(name) {
            return Err(malformed(format!(
                "line {number} of the checksum list names `{name}`, which is not a file name \
                 in the release directory"
            )));
        }
        if !seen.insert(name.to_owned()) {
            return Err(malformed(format!(
                "the checksum list names `{name}` more than once, so it is not a record of \
                 distinct files"
            )));
        }
        entries.push(Entry {
            name: name.to_owned(),
            digest,
        });
    }
    if entries.is_empty() {
        return Err(malformed("the checksum list records no files"));
    }
    Ok(Checksums { entries })
}

/// Checks every file the list names against the digest it records.
///
/// # Errors
///
/// Returns `dist/verify/artifact-missing` for a listed file that is not there, and
/// `dist/verify/digest-mismatch` for one whose bytes disagree with the record.
fn check_listed_files(dir: &Path, list: &Checksums) -> Result<(), VerifyError> {
    for entry in &list.entries {
        let path = dir.join(&entry.name);
        // Existence first: a file that is not there is missing rather than unreadable, and
        // the two are different statements about the release.
        file_size(&path)?;
        let digest = sha256_file(&path)?;
        if !digest.eq_ignore_ascii_case(&entry.digest) {
            return Err(digest_mismatch(format!(
                "the checksum list records {} for {}, and the file is {digest}",
                entry.digest, entry.name
            )));
        }
    }
    Ok(())
}

/// Checks that every file in the directory is named by the list.
///
/// Directories are skipped: a record names files, and a release directory that has grown a
/// subdirectory has not thereby changed what the release is.
///
/// # Errors
///
/// Returns `dist/verify/digest-mismatch` for a file no record names, and a machine failure
/// when the directory cannot be read.
fn check_directory_entries(
    dir: &Path,
    list: &Checksums,
    exempt: &BTreeSet<String>,
) -> Result<(), VerifyError> {
    let entries = fs::read_dir(dir).map_err(|error| {
        VerifyError::environment(format!("{} cannot be read: {error}", dir.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            VerifyError::environment(format!("{} cannot be read: {error}", dir.display()))
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if exempt.contains(&name) {
            continue;
        }
        let path = entry.path();
        if fs::metadata(&path).is_ok_and(|meta| meta.is_dir()) {
            continue;
        }
        if list.digest_of(&name).is_none() {
            return Err(digest_mismatch(format!(
                "{} is in the release directory and no record names it: the manifest and the \
                 checksum list describe {CHECKSUMS_FILE}, and the file is not part of what \
                 they cover",
                path.display()
            )));
        }
    }
    Ok(())
}

/// Checks every artifact the manifest describes against the file and against the list.
///
/// # Errors
///
/// Returns `dist/verify/artifact-missing` for an artifact that is not there or is not in the
/// checksum list, `dist/verify/digest-mismatch` for one whose bytes or size disagree with a
/// record, and `dist/verify/size-budget-exceeded` for one past its ceiling.
fn check_artifacts(
    dir: &Path,
    manifest: &Manifest,
    list: &Checksums,
) -> Result<Vec<Checked>, VerifyError> {
    let mut checked = Vec::with_capacity(manifest.artifacts.len());
    for artifact in &manifest.artifacts {
        let path = dir.join(&artifact.name);
        let size_bytes = file_size(&path)?;
        let recorded = list.digest_of(&artifact.name).ok_or_else(|| {
            VerifyError::rejected(
                Code::ArtifactMissing,
                format!(
                    "the manifest describes `{}` and the checksum list does not record it, so \
                     the signature does not cover it",
                    artifact.name
                ),
            )
        })?;
        let digest = sha256_file(&path)?;
        if !digest.eq_ignore_ascii_case(&artifact.sha256) {
            return Err(digest_mismatch(format!(
                "the manifest records {} for {}, and the file is {digest}",
                artifact.sha256, artifact.name
            )));
        }
        if !digest.eq_ignore_ascii_case(recorded) {
            return Err(digest_mismatch(format!(
                "the checksum list records {recorded} for {}, and the manifest records {}",
                artifact.name, artifact.sha256
            )));
        }
        if size_bytes != artifact.size_bytes {
            return Err(digest_mismatch(format!(
                "the manifest records {} bytes for {}, and the file is {size_bytes} bytes",
                artifact.size_bytes, artifact.name
            )));
        }
        check_budget(artifact, size_bytes)?;
        checked.push(Checked {
            name: artifact.name.clone(),
            role: artifact.role.clone(),
            size_bytes,
            digest,
        });
    }
    Ok(checked)
}

/// Fails when a file is past the size ceiling its manifest record states.
///
/// The ceiling comes from the manifest rather than from the repository's budget document:
/// somebody verifying a download has the manifest and nothing else, so a threshold read from
/// a file they do not have would be a check that silently does not run.
///
/// # Errors
///
/// Returns `dist/verify/size-budget-exceeded` naming the artifact and the ceiling.
fn check_budget(artifact: &Artifact, size_bytes: u64) -> Result<(), VerifyError> {
    let Some(limit_mb) = artifact.size_budget_mb else {
        return Ok(());
    };
    let size_mb = size_bytes as f64 / MEBIBYTE;
    if size_mb > limit_mb {
        return Err(VerifyError::rejected(
            Code::SizeBudgetExceeded,
            format!(
                "{} is {size_mb:.2}MiB, over the {limit_mb:.2}MiB ceiling the manifest \
                 records for it",
                artifact.name
            ),
        ));
    }
    Ok(())
}

/// The names the directory check does not require a record for.
///
/// Two files can never be in the list: the list itself, because a file cannot carry its own
/// digest, and the detached signature over it, because the signature is produced after the
/// list it covers. Everything else in the directory has to be accounted for.
fn exempt_names(manifest: &Manifest) -> BTreeSet<String> {
    let mut exempt = BTreeSet::new();
    exempt.insert(CHECKSUMS_FILE.to_owned());
    if let Some(signature) = &manifest.signature {
        exempt.insert(signature.detached.clone());
    }
    exempt
}

/// The size of `path` in bytes, refusing anything that is not a file.
///
/// # Errors
///
/// Returns `dist/verify/artifact-missing` when the path is not there or is not a file.
fn file_size(path: &Path) -> Result<u64, VerifyError> {
    let metadata = fs::metadata(path).map_err(|error| {
        VerifyError::rejected(
            Code::ArtifactMissing,
            format!("{} is not in the release directory: {error}", path.display()),
        )
    })?;
    if !metadata.is_file() {
        return Err(VerifyError::rejected(
            Code::ArtifactMissing,
            format!("{} is not a file", path.display()),
        ));
    }
    Ok(metadata.len())
}

/// The SHA-256 of a file, as lower-case hexadecimal.
///
/// The packager has these three lines too, but its manifest module is private to it and one
/// digest function is not a reason to open it up.
///
/// # Errors
///
/// Returns a machine failure when the file cannot be read, which is a different statement
/// from "the file is not there": the caller has already established that it is.
pub(super) fn sha256_file(path: &Path) -> Result<String, VerifyError> {
    let bytes = fs::read(path).map_err(|error| {
        VerifyError::environment(format!("{} cannot be read: {error}", path.display()))
    })?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// A digest failure naming `detail`.
fn digest_mismatch(detail: impl Into<String>) -> VerifyError {
    VerifyError::rejected(Code::DigestMismatch, detail)
}

/// A malformed checksum list naming `detail`.
fn malformed(detail: impl Into<String>) -> VerifyError {
    VerifyError::rejected(Code::Malformed, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::verify::manifest;

    /// A digest that satisfies the parser, written out independently of it.
    fn digest(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    /// A manifest describing one artifact named `name` with `sha256` and `size`.
    fn manifest_for(name: &str, sha256: &str, size: u64) -> Manifest {
        let document = format!(
            r#"{{
              "manifest_version": 1,
              "request_id": "01J8ZQ4K7N3M2P8R5T6V9W0X1Y",
              "release": {{
                "version": "0.1.0",
                "commit": null,
                "built_at": "2026-09-29T00:00:00Z",
                "source_date_epoch": 1790899200
              }},
              "toolchain": {{ "rustc": null, "channel": null, "glibc_min": "2.35" }},
              "compatibility": {{
                "fcitx5": ">=5.1.7",
                "architectures": ["x86_64"],
                "session_tiers": ["x11"]
              }},
              "artifacts": [
                {{"name": "{name}", "role": "dictionary", "sha256": "{sha256}",
                  "size_bytes": {size}, "size_budget_mb": null, "exports": []}}
              ],
              "signature": null
            }}"#
        );
        manifest::parse_str(&document).expect("the fixture manifest is well formed")
    }

    /// A scratch directory unique to this test process and tag.
    fn scratch(tag: &str) -> std::path::PathBuf {
        let name = format!("rspinyin-verify-list-{tag}-{}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        dir
    }

    #[test]
    fn test_parse_reads_the_sha256sum_format_in_both_modes() {
        let list = parse(&format!(
            "{}  base.dict\n{} *librspinyin.so\n",
            digest('a'),
            digest('b')
        ))
        .expect("both lines are records");
        assert_eq!(list.digest_of("base.dict"), Some(digest('a').as_str()));
        assert_eq!(list.digest_of("librspinyin.so"), Some(digest('b').as_str()));
        assert_eq!(list.digest_of("absent"), None);
    }

    #[test]
    fn test_parse_accepts_an_upper_case_digest_and_a_blank_line() {
        let list = parse(&format!("\n{}  base.dict\n\n", digest('A')))
            .expect("the digest is a digest either way");
        assert_eq!(list.digest_of("base.dict"), Some(digest('a').as_str()));
    }

    #[test]
    fn test_parse_refuses_a_line_that_is_not_a_record() {
        let cases = [
            // No file name at all.
            "base.dict".to_owned(),
            // A digest that is not a SHA-256.
            "xyz  base.dict".to_owned(),
            // A name that leaves the release directory.
            format!("{}  ../../etc/shadow", digest('a')),
            // The same file twice, which is not a record of distinct files.
            format!("{}  base.dict\n{}  base.dict", digest('a'), digest('b')),
            // A list with no records.
            "\n\n".to_owned(),
        ];
        for text in cases {
            let failure = parse(&text).expect_err("not a record");
            assert!(
                matches!(
                    failure,
                    VerifyError::Rejected {
                        code: Code::Malformed,
                        ..
                    }
                ),
                "{text:?}: {failure}"
            );
        }
    }

    #[test]
    fn test_read_reports_a_missing_list_as_an_artifact_that_is_not_there() {
        let dir = scratch("missing-list");
        let failure = read(&dir).expect_err("the list is not there");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::ArtifactMissing,
                ..
            }
        ));
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_verify_accepts_a_release_whose_three_records_agree() {
        let dir = scratch("agreeing");
        fs::write(dir.join("base.dict"), b"dictionary bytes").expect("writing the artifact");
        let sha256 = sha256_file(&dir.join("base.dict")).expect("the digest is readable");
        let manifest = manifest_for("base.dict", &sha256, 16);
        fs::write(
            dir.join(CHECKSUMS_FILE),
            format!("{sha256}  base.dict\n"),
        )
        .expect("writing the list");

        let checked = verify(&dir, &manifest).expect("the three records agree");
        assert_eq!(checked.len(), 1);
        assert_eq!(checked[0].name, "base.dict");
        assert_eq!(checked[0].size_bytes, 16);
        assert_eq!(checked[0].digest, sha256);
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_verify_refuses_a_file_whose_bytes_changed() {
        let dir = scratch("digest");
        fs::write(dir.join("base.dict"), b"dictionary bytes").expect("writing the artifact");
        let sha256 = sha256_file(&dir.join("base.dict")).expect("the digest is readable");
        let manifest = manifest_for("base.dict", &sha256, 16);
        fs::write(dir.join("base.dict"), b"tampered bytes!").expect("rewriting the artifact");
        fs::write(dir.join(CHECKSUMS_FILE), format!("{sha256}  base.dict\n"))
            .expect("writing the list");

        let failure = verify(&dir, &manifest).expect_err("the bytes moved");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::DigestMismatch,
                ..
            }
        ));
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_verify_refuses_a_file_the_manifest_names_and_the_directory_lacks() {
        let dir = scratch("missing-artifact");
        let manifest = manifest_for("base.dict", &digest('a'), 16);
        fs::write(dir.join(CHECKSUMS_FILE), format!("{}  base.dict\n", digest('a')))
            .expect("writing the list");

        let failure = verify(&dir, &manifest).expect_err("the file is not there");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::ArtifactMissing,
                ..
            }
        ));
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_verify_refuses_a_file_the_list_names_and_the_directory_lacks() {
        // The other direction: the signed record names a file that is not in the directory.
        let dir = scratch("missing-listed");
        fs::write(dir.join("base.dict"), b"dictionary bytes").expect("writing the artifact");
        let sha256 = sha256_file(&dir.join("base.dict")).expect("the digest is readable");
        let manifest = manifest_for("base.dict", &sha256, 16);
        fs::write(
            dir.join(CHECKSUMS_FILE),
            format!("{sha256}  base.dict\n{}  absent.tar.gz\n", digest('c')),
        )
        .expect("writing the list");

        let failure = verify(&dir, &manifest).expect_err("the listed file is not there");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::ArtifactMissing,
                ..
            }
        ));
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_verify_refuses_a_file_the_directory_holds_and_no_record_names() {
        let dir = scratch("unlisted");
        fs::write(dir.join("base.dict"), b"dictionary bytes").expect("writing the artifact");
        let sha256 = sha256_file(&dir.join("base.dict")).expect("the digest is readable");
        let manifest = manifest_for("base.dict", &sha256, 16);
        fs::write(dir.join(CHECKSUMS_FILE), format!("{sha256}  base.dict\n"))
            .expect("writing the list");
        fs::write(dir.join("extra.so"), b"nobody signed this").expect("writing the extra file");

        let failure = verify(&dir, &manifest).expect_err("the extra file is unaccounted for");
        assert!(
            matches!(
                failure,
                VerifyError::Rejected {
                    code: Code::DigestMismatch,
                    ..
                }
            ),
            "{failure}"
        );
        assert!(failure.to_string().contains("extra.so"), "{failure}");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_verify_accepts_the_list_and_the_signature_as_exempt() {
        // A file cannot carry its own digest and a signature is produced after the list it
        // covers, so the two are the only names the directory check may skip.
        let dir = scratch("exempt");
        fs::write(dir.join("base.dict"), b"dictionary bytes").expect("writing the artifact");
        let sha256 = sha256_file(&dir.join("base.dict")).expect("the digest is readable");
        let manifest = manifest_for("base.dict", &sha256, 16);
        fs::write(dir.join(CHECKSUMS_FILE), format!("{sha256}  base.dict\n"))
            .expect("writing the list");
        fs::write(dir.join("SHA256SUMS.asc"), b"signature").expect("writing the signature");

        verify(&dir, &manifest).expect("the list and its signature are exempt");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_verify_refuses_a_manifest_artifact_the_list_does_not_cover() {
        let dir = scratch("not-covered");
        fs::write(dir.join("base.dict"), b"dictionary bytes").expect("writing the artifact");
        let sha256 = sha256_file(&dir.join("base.dict")).expect("the digest is readable");
        let manifest = manifest_for("base.dict", &sha256, 16);
        // The list covers a different file, so the manifest's artifact is outside the
        // signature even though the directory holds it.
        fs::write(dir.join("extra.tar.gz"), b"archive").expect("writing the archive");
        let archive = sha256_file(&dir.join("extra.tar.gz")).expect("the digest is readable");
        fs::write(
            dir.join(CHECKSUMS_FILE),
            format!("{archive}  extra.tar.gz\n"),
        )
        .expect("writing the list");

        let failure = verify(&dir, &manifest).expect_err("base.dict is outside the record");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::ArtifactMissing,
                ..
            }
        ));
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_verify_refuses_a_file_past_its_size_ceiling() {
        let dir = scratch("budget");
        let bytes = vec![0u8; 3 * 1024 * 1024];
        fs::write(dir.join("base.dict"), &bytes).expect("writing the artifact");
        let sha256 = sha256_file(&dir.join("base.dict")).expect("the digest is readable");
        let mut manifest = manifest_for("base.dict", &sha256, bytes.len() as u64);
        manifest.artifacts[0].size_budget_mb = Some(1.0);
        fs::write(dir.join(CHECKSUMS_FILE), format!("{sha256}  base.dict\n"))
            .expect("writing the list");

        let failure = verify(&dir, &manifest).expect_err("3MiB is over a 1MiB ceiling");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::SizeBudgetExceeded,
                ..
            }
        ));
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_verify_refuses_a_size_the_manifest_records_wrongly() {
        let dir = scratch("size");
        fs::write(dir.join("base.dict"), b"dictionary bytes").expect("writing the artifact");
        let sha256 = sha256_file(&dir.join("base.dict")).expect("the digest is readable");
        let manifest = manifest_for("base.dict", &sha256, 999);
        fs::write(dir.join(CHECKSUMS_FILE), format!("{sha256}  base.dict\n"))
            .expect("writing the list");

        let failure = verify(&dir, &manifest).expect_err("the size disagrees");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::DigestMismatch,
                ..
            }
        ));
        assert!(failure.to_string().contains("999"), "{failure}");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_check_budget_accepts_a_file_exactly_at_its_ceiling() {
        // The ceiling is a ceiling, not a target: a file exactly at it passes, which is the
        // boundary the packager's own gate draws.
        let dir = scratch("budget-edge");
        let bytes = vec![0u8; 1024 * 1024];
        fs::write(dir.join("base.dict"), &bytes).expect("writing the artifact");
        let sha256 = sha256_file(&dir.join("base.dict")).expect("the digest is readable");
        let mut manifest = manifest_for("base.dict", &sha256, bytes.len() as u64);
        manifest.artifacts[0].size_budget_mb = Some(1.0);
        fs::write(dir.join(CHECKSUMS_FILE), format!("{sha256}  base.dict\n"))
            .expect("writing the list");

        verify(&dir, &manifest).expect("a file exactly at its ceiling is within it");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }
}
