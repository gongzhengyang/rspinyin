//! The detached signature over the checksum list, and the program that checks it.
//!
//! Responsibility: decide whether the signature a release ships was made by the key the
//! manifest names, and report which of the two ways it can fail that was -- a key the local
//! keyring does not hold, or a signature that does not verify.
//!
//! # What is verified, and against what
//!
//! The signature covers `SHA256SUMS`, and `SHA256SUMS` covers every file the release ships
//! except itself and the signature. So a verified signature, together with the digest checks
//! of [`super::checksums`], is a statement about every byte of the release -- not just about
//! the archive, and not just about the manifest.
//!
//! # Why the exit status is not the only input
//!
//! `gpg --verify` reports both a missing key and a bad signature with a non-zero status, and
//! the two need different codes: the first is the user's keyring, the second is the release.
//! The status is therefore read together with the machine-readable lines gpg writes to
//! `--status-fd`, which name the condition. The exit status stays the deciding vote -- a
//! status gpg does not report is still a failure -- but it is not the only thing consulted.
//!
//! # Why an expired key is refused
//!
//! A signature made by an expired or revoked key is reported by gpg as a good signature with
//! a warning. This verifier refuses it. There is no other channel through which a user learns
//! that the key they trust has been revoked, so accepting the signature silently would be the
//! only moment the fact could have been surfaced -- and the diagnostic names the status, so
//! the user knows to fetch a newer release key rather than to suspect their download.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Command;

use super::checksums::CHECKSUMS_FILE;
use super::error::{Code, VerifyError};
use super::manifest::Manifest;

/// The program a release signature is checked with.
const GPG: &str = "gpg";

/// The prefix every machine-readable line gpg writes to `--status-fd` carries.
const STATUS_PREFIX: &str = "[GNUPG:] ";

/// The status line reporting a signature that verified.
const GOODSIG: &str = "GOODSIG";

/// The status line reporting that the key which made a signature is not in the keyring.
const NO_PUBKEY: &str = "NO_PUBKEY";

/// The status line reporting that a signature could not be checked at all.
const ERRSIG: &str = "ERRSIG";

/// The reason code an `ERRSIG` line carries when the missing key is the reason.
const ERRSIG_NO_PUBKEY: &str = "9";

/// The statuses that mean the signature must not be accepted, whatever the exit status says.
///
/// `KEYEXPIRED` and `EXPKEYSIG` are the expired-key pair, `REVKEYSIG` is a revoked key, and
/// `EXPSIG` and `SIGEXPIRED` are an expired signature. `NODATA` is gpg saying there was
/// nothing it could read as a signature.
const REFUSED: [&str; 6] = [
    "BADSIG",
    "EXPSIG",
    "SIGEXPIRED",
    "EXPKEYSIG",
    "REVKEYSIG",
    "KEYEXPIRED",
];

/// The fewest hexadecimal digits two key identifiers are compared on.
///
/// gpg reports either the long key id -- the last 16 hexadecimal digits of the fingerprint --
/// or the whole fingerprint, depending on its version and configuration. Comparing the last
/// eight digits is the shortest comparison a key id supports and the shortest gpg itself
/// offers, so it is the floor; anything shorter is refused rather than matched loosely.
const MIN_KEY_ID: usize = 8;

/// What verifies a detached signature.
///
/// A trait rather than a function so that the rules *around* the signature -- the manifest
/// records a signature, the signature was made by the key it names, the file the signature
/// covers is the one that was read -- can be asserted without a keyring, a signing key or a
/// `gpg` binary on the machine running the tests. The implementation that ships runs `gpg`.
pub trait Verifier {
    /// Verifies that `signature` is a detached signature over `signed`.
    ///
    /// Returns the key that made the signature, as the verifier reports it.
    ///
    /// # Errors
    ///
    /// Returns `dist/verify/signing-key-absent` when no public key in the local keyring can
    /// check the signature, and `dist/verify/signature-invalid` when the signature does not
    /// verify. An implementation must not use the network: the promise that this product
    /// never opens a socket is not suspended while it verifies a download.
    fn verify(&self, signature: &Path, signed: &Path) -> Result<String, VerifyError>;
}

/// The `gpg` on this machine.
#[derive(Debug, Clone)]
pub struct Gpg {
    /// The program to run.
    program: OsString,
}

impl Default for Gpg {
    fn default() -> Self {
        Self {
            program: OsString::from(GPG),
        }
    }
}

impl Gpg {
    /// The verifier that runs the `gpg` found on `PATH`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Verifier for Gpg {
    fn verify(&self, signature: &Path, signed: &Path) -> Result<String, VerifyError> {
        let output = command(&self.program, signature, signed)
            .output()
            .map_err(|error| {
                VerifyError::environment(format!(
                    "`{}` could not be run: {error}",
                    self.program.to_string_lossy()
                ))
            })?;
        // A process killed by a signal has no status code, and no status code is not a
        // success: `-1` reaches the classifier as a failure with nothing to explain it.
        let exit = output.status.code().unwrap_or(-1);
        classify(exit, &String::from_utf8_lossy(&output.stdout))
    }
}

/// The command a verification runs.
///
/// Split out from [`Gpg::verify`] so that the arguments can be asserted without running
/// anything. The two `--no-auto-*` flags are the whole of the promise that verifying a
/// release cannot reach a keyserver -- gpg retrieves a missing key automatically when its
/// configuration says so -- and a test over the command line is what keeps them there.
fn command(program: &OsStr, signature: &Path, signed: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .arg("--batch")
        .arg("--no-tty")
        .arg("--no-auto-key-locate")
        .arg("--no-auto-key-retrieve")
        // The machine-readable report goes to stdout; the human-readable one stays on
        // stderr, where it does not have to be parsed.
        .arg("--status-fd")
        .arg("1")
        .arg("--verify")
        .arg(signature)
        .arg(signed);
    command
}

/// Turns gpg's exit status and machine-readable lines into a verdict.
///
/// # Errors
///
/// Returns `dist/verify/signing-key-absent` when gpg reports that the key which made the
/// signature is not in the keyring, and `dist/verify/signature-invalid` when the signature
/// does not verify, when it was made by an expired or revoked key, and when gpg exits
/// non-zero without saying why.
fn classify(exit: i32, status: &str) -> Result<String, VerifyError> {
    let mut signer: Option<String> = None;
    let mut missing_key: Option<String> = None;
    let mut refused: Option<String> = None;
    for line in status.lines() {
        let Some(rest) = line.strip_prefix(STATUS_PREFIX) else {
            continue;
        };
        let mut fields = rest.split_whitespace();
        let Some(keyword) = fields.next() else {
            continue;
        };
        let argument = fields.next();
        match keyword {
            GOODSIG => signer = argument.map(str::to_owned),
            NO_PUBKEY => missing_key = argument.map(str::to_owned),
            ERRSIG => {
                // `ERRSIG <keyid> <pkalgo> <hashalgo> <class> <time> <rc>`: the reason is the
                // last field, and 9 is "no public key".
                if fields.last() == Some(ERRSIG_NO_PUBKEY) {
                    missing_key = argument.map(str::to_owned);
                } else {
                    refused = Some(keyword.to_owned());
                }
            }
            // Every other status gpg writes is information rather than a verdict --
            // `TRUST_UNDEFINED`, `VALIDSIG`, `NEWSIG`, `KEYEXPIRED` on an unrelated key of the
            // same owner -- and none of them changes what the exit status already said.
            other if REFUSED.contains(&other) => refused = Some(other.to_owned()),
            _ => {}
        }
    }
    if let Some(key) = missing_key {
        return Err(VerifyError::rejected(
            Code::SigningKeyAbsent,
            format!(
                "the keyring holds no public key that can check the release signature (gpg \
                 reports {key}); import the release key, which ships beside the release, and \
                 verify again"
            ),
        ));
    }
    if let Some(status) = refused {
        return Err(signature_invalid(format!(
            "gpg reported {status} for the release signature"
        )));
    }
    if exit != 0 {
        return Err(signature_invalid(format!(
            "gpg exited with status {exit} without reporting a good signature"
        )));
    }
    signer.ok_or_else(|| {
        signature_invalid(
            "gpg exited with status 0 without reporting a good signature, so nothing about \
             this release was checked",
        )
    })
}

/// Verifies the detached signature over the checksum list.
///
/// # Errors
///
/// Returns `dist/verify/signature-invalid` when the manifest records no signature, and when
/// the signature was made by a key other than the one the manifest names;
/// `dist/verify/artifact-missing` when the checksum list or the signature is not in the
/// directory; and `dist/verify/signing-key-absent` or `dist/verify/signature-invalid` as
/// `verifier` reports them.
pub fn check(
    dir: &Path,
    manifest: &Manifest,
    verifier: &dyn Verifier,
) -> Result<String, VerifyError> {
    let Some(signature) = &manifest.signature else {
        return Err(signature_invalid(
            "the manifest records no signature, so there is no key to check this release \
             against",
        ));
    };
    let signed = dir.join(CHECKSUMS_FILE);
    let detached = dir.join(&signature.detached);
    require_file(&signed)?;
    require_file(&detached)?;
    let signer = verifier.verify(&detached, &signed)?;
    if !key_matches(&signature.key_id, &signer) {
        return Err(signature_invalid(format!(
            "the manifest names key {} and the signature was made by {signer}",
            signature.key_id
        )));
    }
    Ok(signer)
}

/// Whether the key the manifest names is the key that made the signature.
///
/// The two are compared as hexadecimal suffixes rather than for equality, because gpg reports
/// either the long key id or the whole fingerprint: an equality test would accept a correct
/// signature on one machine and reject it on another.
///
/// # Panics
///
/// Never.
fn key_matches(recorded: &str, reported: &str) -> bool {
    let recorded = normalize_key_id(recorded);
    let reported = normalize_key_id(reported);
    if recorded.len() < MIN_KEY_ID || reported.len() < MIN_KEY_ID {
        return false;
    }
    reported.ends_with(&recorded) || recorded.ends_with(&reported)
}

/// A key identifier reduced to its hexadecimal digits, in one case.
///
/// Filtering rather than trimming, so that the `0x` a user is likely to paste and the spaces
/// gpg separates its columns with both disappear.
///
/// # Panics
///
/// Never.
fn normalize_key_id(key: &str) -> String {
    key.chars()
        .filter(char::is_ascii_hexdigit)
        .map(|digit| digit.to_ascii_uppercase())
        .collect()
}

/// Fails when `path` is not a file.
///
/// # Errors
///
/// Returns `dist/verify/artifact-missing`.
fn require_file(path: &Path) -> Result<(), VerifyError> {
    if path.is_file() {
        return Ok(());
    }
    Err(VerifyError::rejected(
        Code::ArtifactMissing,
        format!("{} is not in the release directory", path.display()),
    ))
}

/// A signature failure naming `detail`.
fn signature_invalid(detail: impl Into<String>) -> VerifyError {
    VerifyError::rejected(Code::SignatureInvalid, detail)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::verify::manifest::parse_str;

    /// A key identifier written out independently of the comparison under test.
    const FINGERPRINT: &str = "0123456789ABCDEF0123456789ABCDEF01234567";

    /// A scratch directory unique to this test process and tag.
    fn scratch(tag: &str) -> std::path::PathBuf {
        let name = format!("rspinyin-verify-signature-{tag}-{}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        dir
    }

    /// A manifest with the given signature block.
    fn manifest_for(signature: &str) -> Manifest {
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
                {{"name": "base.dict", "role": "dictionary", "sha256": "{}",
                  "size_bytes": 0, "size_budget_mb": null, "exports": []}}
              ],
              "signature": {signature}
            }}"#,
            "a".repeat(64)
        );
        parse_str(&document).expect("the fixture manifest is well formed")
    }

    /// A signature block naming `key_id`.
    fn signed_by(key_id: &str) -> String {
        format!(r#"{{"scheme": "openpgp", "key_id": "{key_id}", "detached": "SHA256SUMS.asc"}}"#)
    }

    /// A verifier that answers without running anything.
    struct Double {
        /// What the verifier answers: the key it reports, or the code it refuses under.
        answer: Result<String, Code>,
    }

    impl Verifier for Double {
        fn verify(&self, _signature: &Path, _signed: &Path) -> Result<String, VerifyError> {
            match &self.answer {
                Ok(key) => Ok(key.clone()),
                Err(code) => Err(VerifyError::rejected(*code, "the double refused")),
            }
        }
    }

    #[test]
    fn test_classify_accepts_a_good_signature() {
        let status = format!("[GNUPG:] NEWSIG\n[GNUPG:] GOODSIG {FINGERPRINT} rspinyin\n");
        let signer = classify(0, &status).expect("the signature verified");
        assert_eq!(signer, FINGERPRINT);
    }

    #[test]
    fn test_classify_ignores_informational_lines() {
        // Trust is a local opinion about a key, not a statement about the release: a user who
        // has not signed the maintainer's key still gets a verified download.
        let status = format!(
            "[GNUPG:] NEWSIG\n[GNUPG:] SIG_ID abc\n[GNUPG:] GOODSIG {FINGERPRINT} rspinyin\n\
             [GNUPG:] VALIDSIG {FINGERPRINT} 2026-09-29 1790899200 0 4 0 1 8 00\n\
             [GNUPG:] TRUST_UNDEFINED 0 pgp\n"
        );
        assert_eq!(classify(0, &status).expect("verified"), FINGERPRINT);
    }

    #[test]
    fn test_classify_reports_a_key_the_keyring_does_not_hold() {
        let status = format!(
            "[GNUPG:] ERRSIG {FINGERPRINT} 22 8 00 1790899200 9\n[GNUPG:] NO_PUBKEY {FINGERPRINT}\n"
        );
        let failure = classify(2, &status).expect_err("the key is not there");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::SigningKeyAbsent,
                ..
            }
        ));
    }

    #[test]
    fn test_classify_reports_a_missing_key_from_the_errsig_reason_alone() {
        // gpg does not always follow `ERRSIG` with `NO_PUBKEY`, so the reason code has to be
        // read as well; treating it as a bad signature would tell the user their download is
        // corrupt when their keyring is empty.
        let status = format!("[GNUPG:] ERRSIG {FINGERPRINT} 22 8 00 1790899200 9\n");
        let failure = classify(2, &status).expect_err("the key is not there");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::SigningKeyAbsent,
                ..
            }
        ));
    }

    #[test]
    fn test_classify_refuses_a_bad_signature() {
        let status = format!("[GNUPG:] BADSIG {FINGERPRINT} rspinyin\n");
        let failure = classify(1, &status).expect_err("the signature does not verify");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::SignatureInvalid,
                ..
            }
        ));
        assert!(failure.to_string().contains("BADSIG"), "{failure}");
    }

    #[test]
    fn test_classify_refuses_an_expired_or_revoked_key() {
        // gpg calls these a good signature with a warning. Refusing is deliberate: this is
        // the only moment at which the user can be told the key is no longer trustworthy.
        for keyword in REFUSED {
            let status = format!("[GNUPG:] GOODSIG {FINGERPRINT} rspinyin\n[GNUPG:] {keyword}\n");
            let failure = classify(0, &status).expect_err("the key is not usable");
            assert!(
                matches!(
                    failure,
                    VerifyError::Rejected {
                        code: Code::SignatureInvalid,
                        ..
                    }
                ),
                "{keyword}: {failure}"
            );
            assert!(failure.to_string().contains(keyword), "{failure}");
        }
    }

    #[test]
    fn test_classify_refuses_a_non_zero_exit_without_a_status() {
        let failure = classify(2, "").expect_err("gpg failed and said nothing");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::SignatureInvalid,
                ..
            }
        ));
        assert!(failure.to_string().contains("status 2"), "{failure}");
    }

    #[test]
    fn test_classify_refuses_a_success_without_a_good_signature() {
        // Exit status 0 with no verdict is not a verdict: accepting it would make the
        // signature check one that a silent gpg could pass.
        let failure = classify(0, "[GNUPG:] NEWSIG\n").expect_err("nothing was verified");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::SignatureInvalid,
                ..
            }
        ));
    }

    #[test]
    fn test_command_keeps_verification_off_the_network() {
        let built = command(
            OsStr::new(GPG),
            Path::new("SHA256SUMS.asc"),
            Path::new("SHA256SUMS"),
        );
        let args: Vec<String> = built
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        for flag in [
            "--batch",
            "--no-tty",
            "--no-auto-key-locate",
            "--no-auto-key-retrieve",
        ] {
            assert!(args.iter().any(|arg| arg == flag), "{args:?}");
        }
        assert!(args.windows(2).any(|pair| pair == ["--status-fd", "1"]));
        assert_eq!(
            &args[args.len() - 2..],
            ["SHA256SUMS.asc", "SHA256SUMS"],
            "the signature is named before the file it covers"
        );
    }

    #[test]
    fn test_key_matches_compares_a_key_id_with_a_fingerprint_either_way() {
        assert!(key_matches(FINGERPRINT, FINGERPRINT));
        // gpg reports the long key id for some versions and the fingerprint for others.
        assert!(key_matches(&FINGERPRINT[24..], FINGERPRINT));
        assert!(key_matches(FINGERPRINT, &FINGERPRINT[24..]));
        assert!(key_matches(
            &format!("0x{}", FINGERPRINT.to_lowercase()),
            FINGERPRINT
        ));
    }

    #[test]
    fn test_key_matches_refuses_a_different_key_and_a_too_short_one() {
        assert!(!key_matches(
            FINGERPRINT,
            "FEDCBA9876543210FEDCBA9876543210FEDCBA98"
        ));
        assert!(!key_matches("ABCD1234", FINGERPRINT));
        assert!(!key_matches("not a key", FINGERPRINT));
        assert!(!key_matches(FINGERPRINT, "ABCD1234"));
    }

    #[test]
    fn test_check_refuses_a_manifest_that_records_no_signature() {
        let dir = scratch("unsigned");
        fs::write(dir.join(CHECKSUMS_FILE), "a").expect("writing the list");
        let manifest = manifest_for("null");
        let failure = check(&dir, &manifest, &Gpg::new()).expect_err("there is no signature");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::SignatureInvalid,
                ..
            }
        ));
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_check_refuses_a_signature_file_that_is_not_there() {
        let dir = scratch("no-detached");
        fs::write(dir.join(CHECKSUMS_FILE), "a").expect("writing the list");
        let manifest = manifest_for(&signed_by(FINGERPRINT));
        let failure = check(&dir, &manifest, &Gpg::new()).expect_err("the signature is absent");
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
    fn test_check_refuses_a_signature_made_by_another_key() {
        let dir = scratch("other-key");
        fs::write(dir.join(CHECKSUMS_FILE), "a").expect("writing the list");
        fs::write(dir.join("SHA256SUMS.asc"), "a signature").expect("writing the signature");
        let manifest = manifest_for(&signed_by(&FINGERPRINT[24..]));
        let other = "FEDCBA9876543210FEDCBA9876543210FEDCBA98";
        let verifier = Double {
            answer: Ok(other.to_owned()),
        };

        let failure = check(&dir, &manifest, &verifier).expect_err("another key signed this");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::SignatureInvalid,
                ..
            }
        ));
        assert!(failure.to_string().contains(other), "{failure}");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_check_accepts_a_signature_made_by_the_named_key() {
        let dir = scratch("named-key");
        fs::write(dir.join(CHECKSUMS_FILE), "a").expect("writing the list");
        fs::write(dir.join("SHA256SUMS.asc"), "a signature").expect("writing the signature");
        let manifest = manifest_for(&signed_by(&FINGERPRINT[24..]));
        let verifier = Double {
            answer: Ok(FINGERPRINT.to_owned()),
        };

        assert_eq!(
            check(&dir, &manifest, &verifier).expect("the named key signed this"),
            FINGERPRINT
        );
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_check_reports_a_key_the_verifier_could_not_find() {
        let dir = scratch("absent-key");
        fs::write(dir.join(CHECKSUMS_FILE), "a").expect("writing the list");
        fs::write(dir.join("SHA256SUMS.asc"), "a signature").expect("writing the signature");
        let manifest = manifest_for(&signed_by(FINGERPRINT));
        let verifier = Double {
            answer: Err(Code::SigningKeyAbsent),
        };

        let failure = check(&dir, &manifest, &verifier).expect_err("the key is not there");
        assert!(matches!(
            failure,
            VerifyError::Rejected {
                code: Code::SigningKeyAbsent,
                ..
            }
        ));
        fs::remove_dir_all(&dir).expect("cleaning up");
    }
}
