//! `xtask release` -- the release pipeline's own two gates.
//!
//! Responsibility: assert that a pushed tag names the version this tree builds, and write
//! the manifest that describes an assembled release directory.
//!
//! # Two documents, one schema
//!
//! `xtask package` writes a `rspinyin-release.json` too, and the two describe different sets
//! of files. The packager's describes one architecture's payloads -- the addon libraries, the
//! dictionary, the descriptors and the icons, each with the digest and the size ceiling it
//! was measured against -- and it is what somebody who unpacks a release archive holds. The
//! document written here describes what a *release publishes*: the source archive, one
//! distribution package per ecosystem and architecture, the public signing key, and nothing
//! else. Both are the schema of `docs/dev/opt-deploy.md` section 3.3.1, and `xtask verify`
//! reads either.
//!
//! A release publishes the assembled document rather than any packager's, because one
//! `rspinyin-release.json` has to describe everything the release page offers, and no single
//! `xtask package` run sees the other architectures or the other ecosystems.
//!
//! # What a refusal here is, and what it is not
//!
//! Every refusal in this module is a pipeline failure: a tag that names another version, a
//! file that is missing, a file that is not one a release publishes. None of them is one of
//! the nine `dist/*` codes of the delivery contract, and none is reported under one. Those
//! nine describe a release a user downloaded and that does not verify; reusing one here would
//! send a maintainer looking for a user's problem. `xtask verify` is the command that speaks
//! them, and it is the command the pipeline runs after signing.
//!
//! # Modules
//!
//! [`facts`] reads what the tree and the machine say about the release, [`id`] mints the
//! identifier the document carries, [`artifacts`] decides what each file in the release
//! directory is and refuses a directory that is short one, and [`document`] renders it.

mod artifacts;
mod document;
mod facts;
mod id;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};

use self::artifacts::{Architecture, PackageKind, Set};

/// Command-line surface of `xtask release`.
#[derive(Debug, Args)]
pub struct ReleaseArgs {
    /// The verb to run.
    #[command(subcommand)]
    command: ReleaseCommand,
}

/// What `xtask release` can be asked to do.
#[derive(Debug, Subcommand)]
pub enum ReleaseCommand {
    /// Assert that a pushed tag names the version this tree builds.
    ///
    /// The version is the only thing on stdout, so that a workflow can capture it; the
    /// report goes to stderr.
    CheckTag(CheckTagArgs),
    /// Write the release manifest for an assembled release directory.
    Manifest(ManifestArgs),
}

/// Arguments of `xtask release check-tag`.
#[derive(Debug, Args)]
pub struct CheckTagArgs {
    /// The tag, as `GITHUB_REF_NAME` reports it, with or without a leading `v`.
    #[arg(value_name = "TAG")]
    tag: String,
}

/// Arguments of `xtask release manifest`.
#[derive(Debug, Args)]
pub struct ManifestArgs {
    /// Directory holding the files the release publishes. The manifest is written into it.
    #[arg(long, value_name = "DIR")]
    artifacts: PathBuf,
    /// The version the release is published under.
    #[arg(long, value_name = "VERSION")]
    version: String,
    /// One `kind:architecture` per distribution package the release publishes, for example
    /// `deb:x86_64,deb:aarch64,rpm:x86_64,rpm:aarch64,pkg:x86_64`.
    #[arg(long, value_name = "LIST", value_delimiter = ',', required = true)]
    packages: Vec<String>,
}

/// Entry point for `xtask release`.
///
/// # Errors
///
/// Returns the failure of whichever verb ran: a tag that does not name the version this tree
/// builds, a release directory that is missing a file or holds one a release does not
/// publish, or a document that cannot be written.
///
/// # Panics
///
/// Never.
pub fn run(args: ReleaseArgs) -> Result<()> {
    match args.command {
        ReleaseCommand::CheckTag(args) => check_tag(&args.tag),
        ReleaseCommand::Manifest(args) => manifest(&args),
    }
}

/// Asserts that `tag` names the version this tree builds, and prints that version.
///
/// # Errors
///
/// Returns an error when the workspace manifest cannot be read or states no version, and
/// when the tag names another version.
///
/// # Panics
///
/// Never.
fn check_tag(tag: &str) -> Result<()> {
    let root = crate::budget::repo_root()?;
    let version = facts::workspace_version(&root)?;
    let named = named_version(tag);
    ensure!(
        named == version,
        "release: the tag `{tag}` names version {named} and this tree builds {version}. A \
         release published under a version nothing in it was built from cannot be traced back \
         to a commit; push a tag that matches, or bump the workspace version first."
    );
    // The version alone on stdout: a workflow captures this stream to name the artifacts, so
    // a report line mixed into it would become part of a file name.
    println!("{version}");
    eprintln!("release: the tag {tag} names the version this tree builds ({version})");
    Ok(())
}

/// The version a tag names: the tag without its optional leading `v`.
///
/// # Panics
///
/// Never.
fn named_version(tag: &str) -> &str {
    tag.strip_prefix('v').unwrap_or(tag)
}

/// Writes the release manifest for the assembled release directory.
///
/// # Errors
///
/// Returns an error when the version the caller names is not the one this tree builds, when
/// the release directory cannot be read, when it is missing a file the release must publish
/// or holds one that is none of them, and when the document cannot be written.
///
/// # Panics
///
/// Never.
fn manifest(args: &ManifestArgs) -> Result<()> {
    let root = crate::budget::repo_root()?;
    let built = facts::workspace_version(&root)?;
    ensure!(
        args.version == built,
        "release: the manifest was asked for version {} and this tree builds {built}. The \
         release directory holds artifacts built from this tree, so the two cannot differ; the \
         version comes from the tag the pipeline was triggered by.",
        args.version
    );
    let expected = expect_packages(&args.packages)?;
    let set = Set::read(&args.artifacts, &args.version, &expected)?;
    let identity = facts::identity(&root, &args.version)?;
    let written = document::write(&identity, &set)?;
    report(&identity, &set, &written);
    Ok(())
}

/// Parses the `kind:architecture` list the pipeline declares.
///
/// # Errors
///
/// Returns an error for an entry that is not a `kind:architecture` pair, for a kind that is
/// not one of the three package formats, for an architecture the delivery matrix does not
/// cover, for a pair declared twice, and for a list with no pair in it.
///
/// # Panics
///
/// Never.
fn expect_packages(entries: &[String]) -> Result<Vec<(PackageKind, Architecture)>> {
    let mut expected: Vec<(PackageKind, Architecture)> = Vec::with_capacity(entries.len());
    for entry in entries {
        let (kind, arch) = entry.split_once(':').with_context(|| {
            format!("release: `{entry}` is not a `kind:architecture` pair, such as `deb:x86_64`")
        })?;
        let pair = (PackageKind::parse(kind)?, Architecture::parse(arch)?);
        ensure!(
            !expected.contains(&pair),
            "release: `{entry}` is declared twice, so the release would be asked to publish two \
             files for one package"
        );
        expected.push(pair);
    }
    ensure!(
        !expected.is_empty(),
        "release: no packages were declared, so the release would publish no distribution \
         package at all"
    );
    Ok(expected)
}

/// Prints what the document records, and where it went.
fn report(identity: &facts::Identity, set: &Set, out: &Path) {
    for file in set.files() {
        println!("release: {} {}", file.role.label(), file.name);
    }
    println!(
        "release: {} {} for {} architecture(s), {} file(s)",
        identity.version,
        identity.request_id,
        set.architectures().len(),
        set.files().len()
    );
    println!("release: wrote {}", out.display());
    println!(
        "release: the signature block is null until the release is signed -- the signing step \
         records the key before the checksum list is written, so that the list covers it"
    );
}
