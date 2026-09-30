//! The delivery codes a failed verification carries, and the error it is reported through.
//!
//! Responsibility: hold the nine codes the delivery contract fixes for a release that does
//! not verify, and the error type every check reports through. Nothing here decides whether
//! a check passed; it names what a failure was.
//!
//! # Why a failure is mapped onto one of nine codes rather than given its own
//!
//! The codes are matched by diagnostics, by the release jobs and by the acceptance tests of
//! this command, so they are contractual: an existing one is never reworded, and a tenth is
//! not added. Every refusal therefore has to be expressed as one of the nine. Where a
//! refusal does not fit any of them -- the machine cannot run the check at all -- it is
//! reported without a code rather than under a misleading one, because the nine describe
//! the release and that failure describes the machine.

use std::fmt;

/// One of the nine codes a failed verification is reported under.
///
/// The variants are named after the failure rather than after the code, so that a call site
/// reads as the check it is making; [`Code::as_str`] is the code itself, and that string is
/// what a diagnostic and a CI job match on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Code {
    /// `dist/manifest/unsupported-version`: the manifest declares a schema version this
    /// build does not understand.
    UnsupportedVersion,
    /// `dist/manifest/malformed`: the manifest, or the checksum list it names, is not a
    /// document the contract's schema accepts.
    Malformed,
    /// `dist/verify/artifact-missing`: a file one of the release's records names is not in
    /// the directory.
    ArtifactMissing,
    /// `dist/verify/digest-mismatch`: a file's bytes do not agree with the digest recorded
    /// for them, or a file in the directory has no record at all.
    DigestMismatch,
    /// `dist/verify/size-budget-exceeded`: a file is larger than the ceiling its record
    /// states.
    SizeBudgetExceeded,
    /// `dist/verify/signing-key-absent`: the local keyring holds no public key that can
    /// check the signature.
    SigningKeyAbsent,
    /// `dist/verify/signature-invalid`: the detached signature does not verify, or was not
    /// made by the key the manifest names.
    SignatureInvalid,
    /// `dist/verify/factory-symbol-missing`: a library does not export a symbol the
    /// manifest lists.
    FactorySymbolMissing,
    /// `dist/verify/dictionary-invalid`: the dictionary is not a container this build can
    /// read.
    DictionaryInvalid,
}

impl Code {
    /// The code as the contract writes it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedVersion => "dist/manifest/unsupported-version",
            Self::Malformed => "dist/manifest/malformed",
            Self::ArtifactMissing => "dist/verify/artifact-missing",
            Self::DigestMismatch => "dist/verify/digest-mismatch",
            Self::SizeBudgetExceeded => "dist/verify/size-budget-exceeded",
            Self::SigningKeyAbsent => "dist/verify/signing-key-absent",
            Self::SignatureInvalid => "dist/verify/signature-invalid",
            Self::FactorySymbolMissing => "dist/verify/factory-symbol-missing",
            Self::DictionaryInvalid => "dist/verify/dictionary-invalid",
        }
    }
}

impl fmt::Display for Code {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A verification that did not reach its verdict.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// The release was refused, under the delivery code the contract fixes for the failure.
    #[error("{code}: {detail}")]
    Rejected {
        /// The delivery code the refusal is reported under.
        code: Code,
        /// What was checked, and what disagreed with what.
        detail: String,
    },

    /// The machine cannot perform a check at all.
    ///
    /// The one failure that carries no delivery code: the nine describe the release, and
    /// this describes the machine. The case that arises in practice is a machine without
    /// `gpg`, where reporting `dist/verify/signature-invalid` would tell the user their
    /// download is corrupt when the real problem is a missing program.
    #[error("verify: cannot run: {detail}")]
    Environment {
        /// What is missing, or what could not be run.
        detail: String,
    },
}

impl VerifyError {
    /// A refusal carrying `code`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn rejected(code: Code, detail: impl Into<String>) -> Self {
        Self::Rejected {
            code,
            detail: detail.into(),
        }
    }

    /// A failure of the machine rather than of the release.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn environment(detail: impl Into<String>) -> Self {
        Self::Environment {
            detail: detail.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_code_as_str_renders_the_nine_contract_codes() {
        // The strings are the contract's, character for character: they are matched by
        // diagnostics and by the release jobs, so a reworded code is a broken promise even
        // when the check behind it is unchanged.
        let rendered = [
            Code::UnsupportedVersion,
            Code::Malformed,
            Code::ArtifactMissing,
            Code::DigestMismatch,
            Code::SizeBudgetExceeded,
            Code::SigningKeyAbsent,
            Code::SignatureInvalid,
            Code::FactorySymbolMissing,
            Code::DictionaryInvalid,
        ]
        .map(Code::as_str);
        assert_eq!(
            rendered,
            [
                "dist/manifest/unsupported-version",
                "dist/manifest/malformed",
                "dist/verify/artifact-missing",
                "dist/verify/digest-mismatch",
                "dist/verify/size-budget-exceeded",
                "dist/verify/signing-key-absent",
                "dist/verify/signature-invalid",
                "dist/verify/factory-symbol-missing",
                "dist/verify/dictionary-invalid",
            ]
        );
    }

    #[test]
    fn test_rejected_renders_its_code_ahead_of_the_detail() {
        let failure = VerifyError::rejected(Code::DigestMismatch, "base.dict does not match");
        assert_eq!(
            failure.to_string(),
            "dist/verify/digest-mismatch: base.dict does not match"
        );
    }

    #[test]
    fn test_environment_carries_no_delivery_code() {
        // The nine codes describe the release; a missing program does not, and reporting one
        // would send the user looking for a corrupt download.
        let failure = VerifyError::environment("gpg is not installed");
        assert_eq!(
            failure.to_string(),
            "verify: cannot run: gpg is not installed"
        );
    }
}
