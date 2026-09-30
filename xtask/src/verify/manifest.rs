//! The release manifest, as a verifier reads it.
//!
//! Responsibility: turn `rspinyin-release.json` into the typed document the checks work
//! against, and refuse the two ways it can be unusable -- a schema version this build does
//! not understand, and a document that does not match the schema at all.
//!
//! # Why the version is read before the schema is enforced
//!
//! A manifest from a newer release carries fields this build has never heard of. Reporting it
//! as malformed would send the user looking for a corrupt download when what they actually
//! need is a newer verifier, so the version is read out of the raw document first and
//! answered with its own code. Only a document that claims a version this build understands
//! is then held to the schema.
//!
//! # Why the field set is closed
//!
//! Every field the contract's schema lists is required, including the ones that are null for
//! some artifacts. A manifest that simply omitted `exports` would otherwise pass verification
//! with the symbol check skipped, which is the shape of a manifest that has been edited to
//! remove an inconvenient requirement rather than of one written by a newer packager. An
//! absent field is therefore malformed, and a null is accepted only where the packager is
//! known to write null.

use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use super::error::{Code, VerifyError};

/// The manifest schema version this build understands.
pub const MANIFEST_VERSION: u64 = 1;

/// The only signature scheme this build verifies.
pub const SIGNATURE_SCHEME: &str = "openpgp";

/// The role the dictionary artifact is recorded under.
///
/// Kept in step with the role the packager writes for the compiled dictionary: the role is
/// what tells the verifier which artifact to hand to the dictionary reader, so a manifest
/// that stopped using this word would silently skip that check.
pub const DICTIONARY_ROLE: &str = "dictionary";

/// Characters in a SHA-256 digest rendered as hexadecimal.
const SHA256_HEX: usize = 64;

/// One file a release ships, as the manifest records it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Artifact {
    /// File name in the release directory and inside the archive.
    pub name: String,
    /// What the file is for.
    pub role: String,
    /// The digest the release recorded for the file, as hexadecimal.
    pub sha256: String,
    /// The size the release recorded for the file, in bytes.
    pub size_bytes: u64,
    /// The size ceiling the file was measured against, in mebibytes.
    ///
    /// Null for the files that carry no ceiling -- the descriptors, the icons, the notice and
    /// the licence texts -- because the packager writes null for those rather than a number
    /// nothing measured them against.
    pub size_budget_mb: Option<f64>,
    /// Symbols a consumer can resolve from the file with `dlsym`.
    pub exports: Vec<String>,
}

/// The release a manifest describes.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Release {
    /// Workspace version the release was built from.
    pub version: String,
    /// Commit it was built from, when the tree carried one.
    pub commit: Option<String>,
    /// When it was built, in RFC 3339 UTC.
    pub built_at: String,
    /// The epoch second the build was stamped with.
    pub source_date_epoch: i64,
}

/// What built the release.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Toolchain {
    /// Compiler that built the artifacts, when it could be asked.
    pub rustc: Option<String>,
    /// Toolchain channel the build was pinned to, when the pin was there.
    pub channel: Option<String>,
    /// The glibc floor the release claims.
    pub glibc_min: String,
}

/// What the release claims to work on.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Compatibility {
    /// The Fcitx5 version floor the release requires.
    pub fcitx5: String,
    /// The architectures the release was built for.
    pub architectures: Vec<String>,
    /// The session tiers the release claims to work on.
    pub session_tiers: Vec<String>,
}

/// The detached signature a release carries.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Signature {
    /// The signature scheme.
    pub scheme: String,
    /// The key the release was signed with.
    pub key_id: String,
    /// File name of the detached signature, beside the checksum list it covers.
    pub detached: String,
}

/// The document a release publishes beside its artifacts.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Manifest {
    /// Schema version of the document.
    pub manifest_version: u64,
    /// Identifier unique to this release.
    pub request_id: String,
    /// The release this manifest describes.
    pub release: Release,
    /// What built it.
    pub toolchain: Toolchain,
    /// What it claims to work on.
    pub compatibility: Compatibility,
    /// One record per shipped file.
    pub artifacts: Vec<Artifact>,
    /// The detached signature, null until the release has been signed.
    pub signature: Option<Signature>,
}

/// Reads the manifest at `path`.
///
/// # Errors
///
/// Returns `dist/manifest/malformed` when the file cannot be read or does not match the
/// schema, and `dist/manifest/unsupported-version` when it declares a schema version this
/// build does not understand.
pub fn parse(path: &Path) -> Result<Manifest, VerifyError> {
    let text = fs::read_to_string(path).map_err(|error| {
        VerifyError::rejected(
            Code::Malformed,
            format!("{} cannot be read: {error}", path.display()),
        )
    })?;
    parse_str(&text)
}

/// Parses a manifest from the text of the document.
///
/// Separate from [`parse`] so that the schema rules can be asserted without a file, and so
/// that reading is the only part of this module that touches the filesystem.
///
/// # Errors
///
/// As [`parse`].
pub fn parse_str(text: &str) -> Result<Manifest, VerifyError> {
    let document: Value = serde_json::from_str(text).map_err(|error| {
        VerifyError::rejected(
            Code::Malformed,
            format!("the manifest is not JSON: {error}"),
        )
    })?;
    let declared = document
        .get("manifest_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            VerifyError::rejected(
                Code::Malformed,
                "the manifest states no `manifest_version` number",
            )
        })?;
    if declared != MANIFEST_VERSION {
        return Err(unsupported_version(declared));
    }
    let manifest: Manifest = serde_json::from_value(document).map_err(|error| {
        VerifyError::rejected(
            Code::Malformed,
            format!("the manifest does not match the schema: {error}"),
        )
    })?;
    validate(&manifest)?;
    Ok(manifest)
}

/// Checks the parts of the schema a deserializer cannot express.
///
/// The file names are the security-relevant half. Every one of them is joined onto the
/// artifacts directory before it is read, so a name that reaches out of that directory --
/// through a separator, or through `..` -- would make the verifier read a file the release
/// does not describe and then report on it as though it were part of the release.
///
/// # Errors
///
/// Returns `dist/manifest/malformed` naming the field that does not hold, and
/// `dist/manifest/unsupported-version` if the version the schema check accepted disagrees
/// with the one this build implements.
fn validate(manifest: &Manifest) -> Result<(), VerifyError> {
    // The authoritative version check. The one in [`parse_str`] runs first only so that a
    // document from a newer release is reported as such before its unknown fields are.
    if manifest.manifest_version != MANIFEST_VERSION {
        return Err(unsupported_version(manifest.manifest_version));
    }
    if manifest.artifacts.is_empty() {
        return Err(malformed("the manifest describes no artifacts"));
    }
    for artifact in &manifest.artifacts {
        if !is_file_name(&artifact.name) {
            return Err(malformed(format!(
                "`{}` is not a file name in the release directory",
                artifact.name
            )));
        }
        if !is_sha256(&artifact.sha256) {
            return Err(malformed(format!(
                "`{}` records `{}` as its digest, which is not {SHA256_HEX} hexadecimal \
                 characters",
                artifact.name, artifact.sha256
            )));
        }
        if let Some(budget) = artifact.size_budget_mb {
            check_budget(&artifact.name, budget)?;
        }
    }
    if let Some(signature) = &manifest.signature {
        if signature.scheme != SIGNATURE_SCHEME {
            return Err(malformed(format!(
                "the release is signed with `{}`; this build verifies `{SIGNATURE_SCHEME}`",
                signature.scheme
            )));
        }
        if !is_file_name(&signature.detached) {
            return Err(malformed(format!(
                "`{}` is not a file name in the release directory",
                signature.detached
            )));
        }
    }
    Ok(())
}

/// Fails when a recorded size ceiling is not a size.
///
/// A negative or non-finite ceiling would compare as "no file is ever over it", which turns
/// the budget gate into a check that cannot fail.
///
/// # Errors
///
/// Returns `dist/manifest/malformed` naming the artifact.
fn check_budget(name: &str, budget: f64) -> Result<(), VerifyError> {
    if !budget.is_finite() || budget < 0.0 {
        return Err(malformed(format!(
            "`{name}` records a size ceiling of {budget}, which is not a size"
        )));
    }
    Ok(())
}

/// The refusal a schema version this build does not implement gets.
fn unsupported_version(declared: u64) -> VerifyError {
    VerifyError::rejected(
        Code::UnsupportedVersion,
        format!(
            "the manifest declares schema version {declared}; this build understands version \
             {MANIFEST_VERSION}"
        ),
    )
}

/// A malformed manifest naming `detail`.
fn malformed(detail: impl Into<String>) -> VerifyError {
    VerifyError::rejected(Code::Malformed, detail)
}

/// Whether `name` is a file name inside the release directory rather than a path.
///
/// A separator is refused outright, which also refuses an absolute name and any traversal
/// that would have to be written as a path. The two remaining single-segment traversals are
/// refused by name. Shared with the checksum list, which has the same reason to refuse one.
///
/// # Panics
///
/// Never.
pub(super) fn is_file_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\0')
}

/// Whether `digest` is a SHA-256 rendered as hexadecimal.
///
/// # Panics
///
/// Never.
pub(super) fn is_sha256(digest: &str) -> bool {
    digest.len() == SHA256_HEX && digest.chars().all(|digit| digit.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A digest that satisfies the schema, written out independently of the parser.
    fn digest() -> String {
        "a".repeat(SHA256_HEX)
    }

    /// The document, with `body` as its artifact list and `signature` as its signature block.
    fn document_with(body: &str, signature: &str) -> String {
        format!(
            r#"{{
              "manifest_version": 1,
              "request_id": "01J8ZQ4K7N3M2P8R5T6V9W0X1Y",
              "release": {{
                "version": "0.1.0",
                "commit": "ee0dbfb1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7",
                "built_at": "2026-09-29T00:00:00Z",
                "source_date_epoch": 1790899200
              }},
              "toolchain": {{ "rustc": "1.98.0", "channel": "1.98.0", "glibc_min": "2.35" }},
              "compatibility": {{
                "fcitx5": ">=5.1.7",
                "architectures": ["x86_64"],
                "session_tiers": ["x11"]
              }},
              "artifacts": {body},
              "signature": {signature}
            }}"#
        )
    }

    /// A signed document carrying `body` as its artifact list.
    fn document(body: &str) -> String {
        document_with(
            body,
            r#"{
                "scheme": "openpgp",
                "key_id": "0123456789ABCDEF",
                "detached": "SHA256SUMS.asc"
              }"#,
        )
    }

    /// One artifact record, with every field the schema requires.
    fn artifact(name: &str) -> String {
        format!(
            r#"{{"name": "{name}", "role": "addon-inputmethod", "sha256": "{}",
                "size_bytes": 12, "size_budget_mb": 12.0,
                "exports": ["fcitx_addon_factory_instance"]}}"#,
            digest()
        )
    }

    #[test]
    fn test_parse_str_reads_a_manifest_that_matches_the_schema() {
        let text = document(&format!("[{}]", artifact("librspinyin.so")));
        let manifest = parse_str(&text).expect("the document matches the schema");
        assert_eq!(manifest.manifest_version, MANIFEST_VERSION);
        assert_eq!(manifest.artifacts.len(), 1);
        assert_eq!(manifest.artifacts[0].name, "librspinyin.so");
        assert_eq!(manifest.artifacts[0].size_bytes, 12);
        assert_eq!(manifest.artifacts[0].size_budget_mb, Some(12.0));
        assert_eq!(
            manifest.artifacts[0].exports,
            ["fcitx_addon_factory_instance"]
        );
        assert_eq!(
            manifest.signature.expect("the release is signed").detached,
            "SHA256SUMS.asc"
        );
    }

    #[test]
    fn test_parse_str_reports_a_newer_schema_version_as_unsupported() {
        let text = document(&format!("[{}]", artifact("librspinyin.so")))
            .replace("\"manifest_version\": 1", "\"manifest_version\": 2");
        let failure = parse_str(&text).expect_err("version 2 is not this build's schema");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::UnsupportedVersion,
                ..
            }
        ));
        assert!(failure.to_string().contains("version 2"), "{failure}");
    }

    #[test]
    fn test_parse_str_reports_a_missing_version_as_malformed_rather_than_unsupported() {
        // A document with no version at all is not a document from a newer release; it is a
        // document that is not a manifest, and the two send the user to different places.
        let text = document(&format!("[{}]", artifact("librspinyin.so")))
            .replace("\"manifest_version\": 1,", "");
        let failure = parse_str(&text).expect_err("the version is required");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::Malformed,
                ..
            }
        ));
    }

    #[test]
    fn test_parse_str_reports_invalid_json_and_a_wrong_field_type_as_malformed() {
        let truncated = parse_str("{\"manifest_version\": 1,").expect_err("not JSON");
        assert!(matches!(
            truncated,
            VerifyError::Rejected {
                code: Code::Malformed,
                ..
            }
        ));

        let wrong_type = document_with(
            &format!("[{}]", artifact("librspinyin.so")),
            "null",
        )
        .replace("\"manifest_version\": 1", "\"manifest_version\": \"one\"");
        let failure = parse_str(&wrong_type).expect_err("the version is a number");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::Malformed,
                ..
            }
        ));
    }

    #[test]
    fn test_parse_str_refuses_a_manifest_that_omits_exports() {
        // An omitted `exports` would skip the symbol check entirely, which is exactly the
        // shape of a manifest edited to drop a requirement rather than of one from a newer
        // packager: the field is required even when it is empty.
        let without = format!(
            r#"{{"name": "librspinyin.so", "role": "addon-inputmethod", "sha256": "{}",
                "size_bytes": 12, "size_budget_mb": 12.0}}"#,
            digest()
        );
        let failure = parse_str(&document(&format!("[{without}]")))
            .expect_err("`exports` is required");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::Malformed,
                ..
            }
        ));
    }

    #[test]
    fn test_parse_str_refuses_an_empty_artifact_list() {
        let failure = parse_str(&document("[]")).expect_err("a release ships files");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::Malformed,
                ..
            }
        ));
    }

    #[test]
    fn test_parse_str_refuses_a_name_that_leaves_the_release_directory() {
        for name in ["../../etc/shadow", "/etc/shadow", "sub/base.dict", "..", "."] {
            let failure = parse_str(&document(&format!("[{}]", artifact(name))))
                .expect_err("a name is not a path");
            assert!(
                matches!(
                    failure,
                    VerifyError::Rejected {
                        code: Code::Malformed,
                        ..
                    }
                ),
                "{name} must be refused as a name: {failure}"
            );
        }
    }

    #[test]
    fn test_parse_str_refuses_a_digest_that_is_not_a_sha256() {
        let text = document(&format!("[{}]", artifact("base.dict"))).replace(&digest(), "abc");
        let failure = parse_str(&text).expect_err("a digest is 64 hexadecimal characters");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::Malformed,
                ..
            }
        ));
    }

    #[test]
    fn test_parse_str_refuses_a_size_ceiling_that_is_not_a_size() {
        // A negative ceiling would make the budget check one that cannot fail: every file is
        // smaller than a negative number of mebibytes.
        let text = document(&format!("[{}]", artifact("base.dict")))
            .replace("\"size_budget_mb\": 12.0", "\"size_budget_mb\": -1.0");
        let failure = parse_str(&text).expect_err("a ceiling is a non-negative size");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::Malformed,
                ..
            }
        ));
        assert!(failure.to_string().contains("-1"), "{failure}");
    }

    #[test]
    fn test_parse_str_accepts_a_null_size_ceiling() {
        // The packager writes null for the payloads no budget covers, so null has to be
        // accepted; it means "nothing measured this file against a ceiling".
        let text = document(&format!("[{}]", artifact("base.dict")))
            .replace("\"size_budget_mb\": 12.0", "\"size_budget_mb\": null");
        let manifest = parse_str(&text).expect("null is the absence of a ceiling");
        assert_eq!(manifest.artifacts[0].size_budget_mb, None);
    }

    #[test]
    fn test_parse_str_refuses_a_signature_scheme_this_build_cannot_check() {
        let text = document(&format!("[{}]", artifact("base.dict")))
            .replace("\"scheme\": \"openpgp\"", "\"scheme\": \"minisign\"");
        let failure = parse_str(&text).expect_err("only openpgp is verified");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::Malformed,
                ..
            }
        ));
    }

    #[test]
    fn test_parse_str_accepts_an_unsigned_manifest() {
        // The packager writes null until a release is signed, so the schema accepts it; the
        // refusal happens at the signature state, where it can say what to do about it.
        let text = document_with(&format!("[{}]", artifact("base.dict")), "null");
        let manifest = parse_str(&text).expect("an unsigned manifest is well formed");
        assert!(manifest.signature.is_none());
    }

    #[test]
    fn test_parse_reads_a_file_and_reports_one_it_cannot_read() {
        let name = format!("rspinyin-verify-manifest-{}", std::process::id());
        let root = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&root).expect("creating the scratch directory");
        let path = root.join("rspinyin-release.json");
        std::fs::write(&path, document(&format!("[{}]", artifact("base.dict"))))
            .expect("writing the fixture");
        assert!(parse(&path).is_ok());

        let missing = root.join("absent.json");
        let failure = parse(&missing).expect_err("the file is not there");
        assert!(failure.to_string().contains("cannot be read"), "{failure}");
        std::fs::remove_dir_all(&root).expect("cleaning up");
    }

    #[test]
    fn test_is_file_name_accepts_a_plain_name_and_refuses_a_path() {
        assert!(is_file_name("base.dict"));
        assert!(is_file_name("librspinyin_ui.so"));
        assert!(!is_file_name(""));
        assert!(!is_file_name("a/b"));
        assert!(!is_file_name(".."));
    }

    #[test]
    fn test_is_sha256_accepts_both_cases_and_refuses_the_neighbours() {
        assert!(is_sha256(&"a".repeat(SHA256_HEX)));
        assert!(is_sha256(&"A".repeat(SHA256_HEX)));
        assert!(!is_sha256(&"a".repeat(SHA256_HEX - 1)));
        assert!(!is_sha256(&"a".repeat(SHA256_HEX + 1)));
        assert!(!is_sha256(&"g".repeat(SHA256_HEX)));
    }
}
