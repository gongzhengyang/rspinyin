//! End-to-end tests for the verification state machine.
//!
//! Every test builds a complete release in a scratch directory -- two libraries, a
//! dictionary, a descriptor, the archive, the manifest and the checksum list -- and then
//! breaks exactly one thing about it, so that the code a failure is reported under is
//! asserted rather than the fact that something failed. The nine codes of the delivery
//! contract each have one case here.
//!
//! Nothing in this module runs `gpg`. The signature state is driven through a verifier that
//! answers from the test, while the gpg parser and the command line it builds are asserted in
//! [`super::signature`], where they can be exercised without a keyring, a signing key or the
//! program itself being present.

use std::fs;
use std::path::{Path, PathBuf};

use fst::MapBuilder;
use ime_dict::format::SectionKind;
use ime_dict::format::writer::DictWriter;

use super::checksums::{CHECKSUMS_FILE, sha256_file};
use super::error::{Code, VerifyError};
use super::manifest::DICTIONARY_ROLE;
use super::signature::Verifier;
use super::state::State;
use super::{Outcome, verify};
use crate::install::FACTORY_SYMBOL;
use crate::install::elf::synthetic_image;
use crate::package::DICTIONARY_FILE;

/// The name the fixture manifest signs under.
const KEY_ID: &str = "0123456789ABCDEF";

/// The detached signature the fixture ships.
const SIGNATURE_FILE: &str = "SHA256SUMS.asc";

/// The manifest the fixture writes.
const MANIFEST_FILE: &str = "rspinyin-release.json";

/// The archive the fixture ships, which the manifest does not describe.
///
/// A release is more than the files the manifest lists: the archive carries them, the
/// manifest describes the archive's contents, and the checksum list covers both. The fixture
/// ships one so that a check which assumed the directory and the manifest were the same set
/// of files would fail here rather than in a release.
const ARCHIVE_FILE: &str = "rspinyin-0.1.0-x86_64.tar.gz";

/// The artifacts the fixture release ships, as `(name, role, exports)`.
const ARTIFACTS: [(&str, &str, &[&str]); 4] = [
    ("librspinyin.so", "addon-inputmethod", &[FACTORY_SYMBOL]),
    ("librspinyin_ui.so", "addon-ui", &[FACTORY_SYMBOL]),
    ("rspinyin.conf", "addon-descriptor", &[]),
    (DICTIONARY_FILE, DICTIONARY_ROLE, &[]),
];

/// The role an artifact with no contents to read is recorded under.
const DESCRIPTOR_ROLE: &str = "addon-descriptor";

/// The signature block a release signed by [`KEY_ID`] carries.
fn signed_by(key_id: &str) -> String {
    format!(r#"{{"scheme": "openpgp", "key_id": "{key_id}", "detached": "{SIGNATURE_FILE}"}}"#)
}

/// A scratch directory unique to this test process and tag.
fn scratch(tag: &str) -> PathBuf {
    let name = format!("rspinyin-verify-{tag}-{}", std::process::id());
    let dir = std::env::temp_dir().join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

/// Writes one file into a scratch directory.
fn write(dir: &Path, name: &str, bytes: &[u8]) {
    fs::write(dir.join(name), bytes).expect("writing the fixture file");
}

/// A dictionary container the engine's reader accepts.
fn dictionary_image() -> Vec<u8> {
    let fst = MapBuilder::new(Vec::new())
        .expect("an empty builder")
        .into_inner()
        .expect("an empty map encodes");
    let mut writer = DictWriter::new();
    writer
        .add_section(SectionKind::Fst, fst)
        .expect("the section is added");
    writer.encode().expect("the container encodes")
}

/// The manifest for the files in `dir`.
///
/// Every digest and size is read from the file that is actually there, so a test that wants a
/// record to disagree with its file changes the file after the release is sealed rather than
/// editing the record.
fn manifest_text(dir: &Path, version: u64, signature: &str, budget_mb: f64) -> String {
    let mut records = Vec::new();
    for (name, role, exports) in ARTIFACTS {
        let path = dir.join(name);
        let sha256 = sha256_file(&path).expect("the fixture file is there");
        let size_bytes = fs::metadata(&path)
            .expect("the fixture file is there")
            .len();
        let budget = if role == DICTIONARY_ROLE {
            format!("{budget_mb}")
        } else {
            "null".to_owned()
        };
        let exports: Vec<String> = exports
            .iter()
            .map(|symbol| format!("\"{symbol}\""))
            .collect();
        records.push(format!(
            r#"    {{"name": "{name}", "role": "{role}", "sha256": "{sha256}",
      "size_bytes": {size_bytes}, "size_budget_mb": {budget},
      "exports": [{}]}}"#,
            exports.join(", ")
        ));
    }
    format!(
        r#"{{
  "manifest_version": {version},
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
  "artifacts": [
{}
  ],
  "signature": {signature}
}}"#,
        records.join(",\n")
    )
}

/// Writes the checksum list over every file in `dir` except the list and the signature.
fn write_checksums(dir: &Path) {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("the fixture directory is readable")
        .map(|entry| {
            entry
                .expect("the entry is readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name != CHECKSUMS_FILE && name != SIGNATURE_FILE)
        .collect();
    names.sort();
    let mut list = String::new();
    for name in names {
        let digest = sha256_file(&dir.join(&name)).expect("the fixture file is there");
        list.push_str(&format!("{digest}  {name}\n"));
    }
    fs::write(dir.join(CHECKSUMS_FILE), list).expect("writing the checksum list");
}

/// A release directory built for a test.
struct Release {
    /// Where every file of the release is written.
    dir: PathBuf,
    /// The size ceiling the manifest records for the dictionary, in mebibytes.
    budget_mb: f64,
    /// Whether the two libraries export the addon factory symbol.
    factory_symbol: bool,
    /// Whether the dictionary is a container this build can read.
    readable_dictionary: bool,
}

impl Release {
    /// A release whose records all agree.
    fn build(tag: &str) -> Self {
        Self::build_with(tag, 20.0, true, true)
    }

    /// A release with the three knobs the sabotage tests turn.
    ///
    /// Each knob decides what a file *is* rather than what the records say about it, which is
    /// what makes the release still self-consistent: every digest and size in the manifest
    /// and the checksum list is read from the file that was written.
    fn build_with(
        tag: &str,
        budget_mb: f64,
        factory_symbol: bool,
        readable_dictionary: bool,
    ) -> Self {
        let release = Self {
            dir: scratch(tag),
            budget_mb,
            factory_symbol,
            readable_dictionary,
        };
        release.write_artifacts();
        write(
            &release.dir,
            ARCHIVE_FILE,
            b"the archive the manifest describes",
        );
        write(&release.dir, SIGNATURE_FILE, b"a detached signature");
        release.seal(1, &signed_by(KEY_ID));
        release
    }

    /// Writes the four artifacts the manifest describes.
    ///
    /// What each file *is* comes from the knob the test turned, so the release stays
    /// self-consistent: the digests and sizes recorded for it are read from the file that was
    /// written, whatever it contains.
    fn write_artifacts(&self) {
        for (name, role, _) in ARTIFACTS {
            let bytes = if name == DICTIONARY_FILE {
                if self.readable_dictionary {
                    dictionary_image()
                } else {
                    b"the word dictionary".to_vec()
                }
            } else if role == DESCRIPTOR_ROLE {
                b"[Addon]\nName=Rust Pinyin\n".to_vec()
            } else {
                synthetic_image(&[(FACTORY_SYMBOL, self.factory_symbol)])
            };
            write(&self.dir, name, &bytes);
        }
    }

    /// Writes the manifest and the checksum list over what is in the directory.
    ///
    /// `version` is the schema version to declare and `signature` the signature block, so a
    /// test can produce a document this build does not understand, or one that records no
    /// signature, without writing a manifest by hand.
    fn seal(&self, version: u64, signature: &str) {
        let manifest = manifest_text(&self.dir, version, signature, self.budget_mb);
        write(&self.dir, MANIFEST_FILE, manifest.as_bytes());
        write_checksums(&self.dir);
    }

    /// The path of the manifest, which is what a verification is pointed at.
    fn manifest(&self) -> PathBuf {
        self.dir.join(MANIFEST_FILE)
    }

    /// Every file in the release directory, with its size and digest.
    ///
    /// Used to assert that a verification changed nothing: the promise is that this command
    /// reads a release and reports on it, so a file that appeared, vanished or changed while
    /// it ran would break the promise even though the verdict was right.
    fn snapshot(&self) -> Vec<(String, u64, String)> {
        let mut entries: Vec<(String, u64, String)> = fs::read_dir(&self.dir)
            .expect("the fixture directory is readable")
            .map(|entry| {
                let path = entry.expect("the entry is readable").path();
                let name = path
                    .file_name()
                    .expect("a file has a name")
                    .to_string_lossy()
                    .into_owned();
                let size = fs::metadata(&path).expect("the file is there").len();
                let digest = sha256_file(&path).expect("the file is readable");
                (name, size, digest)
            })
            .collect();
        entries.sort();
        entries
    }
}

impl Drop for Release {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// A verifier that accepts any signature as made by [`KEY_ID`].
struct Accepted;

impl Verifier for Accepted {
    fn verify(&self, _signature: &Path, _signed: &Path) -> Result<String, VerifyError> {
        Ok(KEY_ID.to_owned())
    }
}

/// A verifier that refuses under `code`.
struct Refused(Code);

impl Verifier for Refused {
    fn verify(&self, _signature: &Path, _signed: &Path) -> Result<String, VerifyError> {
        Err(VerifyError::rejected(
            self.0,
            "the fixture verifier refused",
        ))
    }
}

/// The delivery code a failure was reported under.
fn code_of(failure: &VerifyError) -> Option<Code> {
    match failure {
        VerifyError::Rejected { code, .. } => Some(*code),
        VerifyError::Environment { .. } => None,
    }
}

/// Runs a verification and returns the code it refused under.
fn refusal(release: &Release, verifier: &dyn Verifier) -> Code {
    let failure = verify(&release.manifest(), &release.dir, verifier)
        .expect_err("the release was expected to be refused");
    code_of(&failure).expect("a refusal of a release carries a delivery code")
}

#[test]
fn test_verify_accepts_a_release_whose_records_agree() {
    let release = Release::build("accepts");
    let outcome = verify(&release.manifest(), &release.dir, &Accepted)
        .expect("every record agrees with the file it describes");

    assert_eq!(outcome.state, State::Verified);
    assert_eq!(outcome.manifest.release.version, "0.1.0");
    assert_eq!(outcome.signing_key, KEY_ID);
    assert_eq!(
        outcome.checked.len(),
        ARTIFACTS.len(),
        "the manifest describes four artifacts and the archive is not one of them"
    );
    assert!(outcome.checked.iter().all(|file| file.digest.len() == 64));
}

#[test]
fn test_verify_reports_a_schema_version_it_does_not_understand() {
    let release = Release::build("unsupported-version");
    release.seal(2, &signed_by(KEY_ID));
    assert_eq!(refusal(&release, &Accepted), Code::UnsupportedVersion);
}

#[test]
fn test_verify_reports_a_manifest_that_is_not_a_manifest() {
    let release = Release::build("malformed");
    write(&release.dir, MANIFEST_FILE, b"this is not a manifest");
    assert_eq!(refusal(&release, &Accepted), Code::Malformed);
}

#[test]
fn test_verify_reports_a_file_whose_bytes_changed() {
    let release = Release::build("digest-mismatch");
    write(&release.dir, DICTIONARY_FILE, b"the word dictionary");
    assert_eq!(refusal(&release, &Accepted), Code::DigestMismatch);
}

#[test]
fn test_verify_reports_a_file_no_record_names() {
    // The other direction of the same check: nothing is missing, but the directory holds a
    // file the manifest and the checksum list do not describe.
    let release = Release::build("unaccounted");
    write(&release.dir, "extra.so", b"nobody signed this");
    assert_eq!(refusal(&release, &Accepted), Code::DigestMismatch);
}

#[test]
fn test_verify_reports_an_artifact_that_is_not_there() {
    let release = Release::build("artifact-missing");
    fs::remove_file(release.dir.join(DICTIONARY_FILE)).expect("removing the artifact");
    assert_eq!(refusal(&release, &Accepted), Code::ArtifactMissing);
}

#[test]
fn test_verify_reports_a_file_past_its_size_ceiling() {
    let release = Release::build_with("size-budget", 0.00001, true, true);
    assert_eq!(refusal(&release, &Accepted), Code::SizeBudgetExceeded);
}

#[test]
fn test_verify_reports_a_key_the_keyring_does_not_hold() {
    let release = Release::build("signing-key-absent");
    assert_eq!(
        refusal(&release, &Refused(Code::SigningKeyAbsent)),
        Code::SigningKeyAbsent
    );
}

#[test]
fn test_verify_reports_a_signature_that_does_not_verify() {
    let release = Release::build("signature-invalid");
    assert_eq!(
        refusal(&release, &Refused(Code::SignatureInvalid)),
        Code::SignatureInvalid
    );
}

#[test]
fn test_verify_reports_a_manifest_that_records_no_signature() {
    // An unsigned release cannot be checked against anything, so it is refused rather than
    // reported as intact on the strength of its digests alone.
    let release = Release::build("unsigned");
    release.seal(1, "null");
    assert_eq!(refusal(&release, &Accepted), Code::SignatureInvalid);
}

#[test]
fn test_verify_reports_a_library_that_lost_its_factory_symbol() {
    let release = Release::build_with("factory-symbol", 20.0, false, true);
    assert_eq!(refusal(&release, &Accepted), Code::FactorySymbolMissing);
}

#[test]
fn test_verify_reports_a_dictionary_that_is_not_a_container() {
    let release = Release::build_with("dictionary", 20.0, true, false);
    assert_eq!(refusal(&release, &Accepted), Code::DictionaryInvalid);
}

#[test]
fn test_verify_changes_nothing_in_the_release_directory() {
    // The read-only promise, asserted where it is observable: the same files with the same
    // sizes and the same digests before and after a verification that succeeded.
    let release = Release::build("read-only");
    let before = release.snapshot();
    verify(&release.manifest(), &release.dir, &Accepted).expect("the release verifies");
    let after = release.snapshot();

    assert_eq!(before, after);
    assert!(before.iter().any(|(name, _, _)| name == MANIFEST_FILE));
}

#[test]
fn test_verify_leaves_a_release_it_refused_unchanged_too() {
    let release = Release::build("read-only-refused");
    let before = release.snapshot();
    let _ = verify(
        &release.manifest(),
        &release.dir,
        &Refused(Code::SignatureInvalid),
    );
    assert_eq!(before, release.snapshot());
}

#[test]
fn test_verify_reports_the_outcome_it_reached() {
    // The machine's state is part of the answer: a caller can tell a release that was checked
    // all the way through from one that was accepted on the strength of its digests.
    let release = Release::build("outcome");
    let outcome: Outcome = verify(&release.manifest(), &release.dir, &Accepted).expect("verifies");
    assert_eq!(outcome.state.label(), "S5 verified");
    assert_eq!(outcome.manifest.request_id, "01J8ZQ4K7N3M2P8R5T6V9W0X1Y");
    assert_eq!(outcome.manifest.compatibility.architectures, ["x86_64"]);
}
