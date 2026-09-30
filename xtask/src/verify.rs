//! `xtask verify` -- check a downloaded release against its manifest and signature.
//!
//! Responsibility: implement the check state machine a user runs before installing an
//! archive they downloaded -- parse the manifest, check every artifact digest, size and size
//! ceiling, verify the detached signature over the checksum list, and read each shared
//! library's dynamic symbol table to confirm the addon factory symbol is present, plus the
//! dictionary's own container checks.
//!
//! # What this deliberately does not do
//!
//! It does not install, replace or remove anything, and it does not look for a newer
//! release. rspinyin has no update channel by design: upgrades arrive through the
//! distribution's package manager, and a component that could rewrite its own installation
//! would be a code path that a network-reachable program could drive. Verification is a
//! read-only operation whose only output is a verdict and an exit status. The three steps of
//! the lifecycle it does not take are named in [`state::Lifecycle`] and printed by the
//! report, so that what is missing is visible rather than merely absent.
//!
//! # The states
//!
//! [`state::Machine`] holds the contract's S0 to S5 and moves only when a check passes, so a
//! refusal reports the state that refused. [`verify`] is the sequence of those checks;
//! [`run`] is the command line around it.
//!
//! # Error codes
//!
//! Every refusal carries one of the nine `dist/*` codes of the delivery contract in
//! `docs/dev/opt-deploy.md` -- the section that draws the verification state machine and
//! lists the codes it can fail under -- held in [`error::Code`]. The codes are matched by
//! diagnostics and by the acceptance tests, so they must not be reworded.
//!
//! # Modules
//!
//! [`manifest`] is the document, [`checksums`] is the checksum list and the checks that
//! compare the release's records against each other and against the directory, [`signature`]
//! is the detached signature and the program that checks it, [`contents`] is what the
//! artifacts contain beyond their bytes, [`state`] is the machine, and [`error`] is what a
//! failure is reported as.

mod checksums;
mod contents;
mod error;
mod manifest;
mod signature;
mod state;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use clap::Args;

use self::checksums::Checked;
use self::error::VerifyError;
use self::manifest::Manifest;
use self::signature::{Gpg, Verifier};
use self::state::{Lifecycle, Machine, State};

/// Bytes in a mebibyte, for the sizes the report prints.
const MEBIBYTE: f64 = 1024.0 * 1024.0;

/// Command-line surface of `xtask verify`.
///
/// Two paths and nothing else: everything else a verification needs is inside the release it
/// is pointed at, and anything it does not need would be a way to verify something other
/// than what the user downloaded.
#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// The release manifest to verify against.
    #[arg(long, value_name = "PATH")]
    manifest: PathBuf,
    /// The directory holding the artifacts the manifest describes.
    #[arg(long, value_name = "DIR")]
    artifacts: PathBuf,
}

/// What a verification that reached its verdict found.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// The state the machine finished in.
    pub state: State,
    /// The manifest that was verified.
    pub manifest: Manifest,
    /// One record per file that matched the records published for it.
    pub checked: Vec<Checked>,
    /// The key the detached signature was made by.
    pub signing_key: String,
}

/// Entry point for `xtask verify`.
///
/// # Errors
///
/// Returns the refusal of whichever state failed, carrying its `dist/*` code, and a failure
/// of the machine itself when the check cannot be run at all.
///
/// # Panics
///
/// Never.
pub fn run(args: VerifyArgs) -> anyhow::Result<()> {
    let outcome = verify(&args.manifest, &args.artifacts, &Gpg::new())?;
    report(&outcome);
    Ok(())
}

/// Runs the verification state machine over the release in `artifacts`.
///
/// The states run in the contract's order -- manifest, digests, signature, contents -- and
/// the signature is checked after the digests rather than before, which is the order the
/// contract fixes. A caller that wants the signature to be the first thing checked should
/// read the contract's diagram again before changing this: the states are numbered there,
/// and a failure is reported under the code of the state that refused.
///
/// # Errors
///
/// Returns the refusal of whichever state failed, carrying its `dist/*` code.
pub fn verify(
    manifest_path: &Path,
    artifacts: &Path,
    verifier: &dyn Verifier,
) -> Result<Outcome, VerifyError> {
    let mut machine = Machine::new();
    let manifest = machine.advance(|| manifest::parse(manifest_path))?;
    let checked = machine.advance(|| checksums::verify(artifacts, &manifest))?;
    let signing_key = machine.advance(|| signature::check(artifacts, &manifest, verifier))?;
    machine.advance(|| contents::verify(artifacts, &manifest))?;
    Ok(Outcome {
        state: machine.state(),
        manifest,
        checked,
        signing_key,
    })
}

/// Prints what was verified, and what the verification did not do.
fn report(outcome: &Outcome) {
    let manifest = &outcome.manifest;
    let release = &manifest.release;
    let toolchain = &manifest.toolchain;
    let compatibility = &manifest.compatibility;
    println!("verify: rspinyin {} ({})", release.version, manifest.request_id);
    let origin = match &release.commit {
        Some(commit) => format!("built from {commit}"),
        None => "built from a tree with no commit recorded".to_owned(),
    };
    println!(
        "verify: {origin} at {} (source date epoch {})",
        release.built_at,
        release.source_date_epoch
    );
    println!(
        "verify: built by rustc {} on channel {}, glibc >= {}",
        toolchain.rustc.as_deref().unwrap_or("unrecorded"),
        toolchain.channel.as_deref().unwrap_or("unrecorded"),
        toolchain.glibc_min
    );
    println!(
        "verify: for {}, requiring fcitx5 {}, on sessions {}",
        compatibility.architectures.join(", "),
        compatibility.fcitx5,
        compatibility.session_tiers.join(", ")
    );
    for file in &outcome.checked {
        println!(
            "verify: {} {} {} sha256:{}",
            file.role,
            file.name,
            human_size(file.size_bytes),
            short(&file.digest)
        );
    }
    println!(
        "verify: {} files match the digests and sizes recorded for them",
        outcome.checked.len()
    );
    println!(
        "verify: the detached signature was made by {}",
        outcome.signing_key
    );
    println!("verify: {} -- the release is intact", outcome.state.label());
    println!("verify: {}", lifecycle_note());
}

/// The line the report ends with: what this command did, and what it deliberately did not.
fn lifecycle_note() -> String {
    let mut note = String::from("this command performs the check only");
    for step in Lifecycle::ALL {
        if !step.is_implemented() {
            note.push_str(&format!("; {} belongs to {}", step.label(), step.owner()));
        }
    }
    note
}

/// Renders a byte count for the report.
fn human_size(bytes: u64) -> String {
    let size = bytes as f64;
    if size >= MEBIBYTE {
        format!("{:.2}MiB", size / MEBIBYTE)
    } else {
        format!("{:.1}KiB", size / 1024.0)
    }
}

/// The first characters of a digest, for a report line that has to stay readable.
fn short(digest: &str) -> &str {
    digest.get(..12).unwrap_or(digest)
}
