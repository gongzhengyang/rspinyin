//! Version consistency between the addon descriptor and the workspace manifest.
//!
//! Fcitx5 reports the version from the addon descriptor, not from the library, so a
//! descriptor left behind by a version bump ships a plugin that mislabels itself. The
//! two are separate files with no mechanical link, which is exactly the kind of drift a
//! gate has to catch.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

/// Path of the addon descriptor, relative to the repository root.
const ADDON_CONF: &str = "packaging/fcitx5/rspinyin.conf";

/// The key Fcitx5 reads the version from.
const VERSION_KEY: &str = "Version";

/// Checks that the addon descriptor advertises the workspace package version.
///
/// # Errors
///
/// Returns an error when either file is missing or unreadable, when the manifest has no
/// `[workspace.package] version`, when the descriptor has no `Version` key, or when the
/// two values differ.
pub fn run() -> Result<()> {
    let root = repo_root()?;
    let manifest = version_from_manifest(&read(&root.join("Cargo.toml"))?)?;
    let addon = version_from_addon_conf(&read(&root.join(ADDON_CONF))?)?;

    ensure!(
        manifest == addon,
        "{ADDON_CONF} advertises version {addon}, but the workspace package version is \
         {manifest}. Fcitx5 reports the descriptor's value, so a mismatch ships a \
         mislabelled plugin."
    );

    println!("check-versions: {ADDON_CONF} matches the workspace version {manifest}");
    Ok(())
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

/// Extracts `[workspace.package] version` from a workspace manifest.
fn version_from_manifest(manifest: &str) -> Result<String> {
    let document: toml::Value = toml::from_str(manifest).context("parsing Cargo.toml")?;
    document
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .context("Cargo.toml has no [workspace.package] version")
}

/// Extracts `Version=` from an Fcitx5 addon descriptor.
///
/// The descriptor is INI-shaped; only the first occurrence is honoured, which matches how
/// Fcitx5's own parser resolves a duplicated key.
fn version_from_addon_conf(conf: &str) -> Result<String> {
    for line in conf.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == VERSION_KEY {
            let value = value.trim();
            ensure!(
                !value.is_empty(),
                "{ADDON_CONF} has an empty {VERSION_KEY} key"
            );
            return Ok(value.to_owned());
        }
    }
    anyhow::bail!("{ADDON_CONF} has no {VERSION_KEY} key")
}

/// Repository root, derived from the compile-time location of this crate.
fn repo_root() -> Result<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(Path::to_path_buf)
        .context("xtask is expected to live in a subdirectory of the repository root")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_from_manifest_reads_the_workspace_package_version() {
        let manifest =
            "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.package]\nversion = \"1.2.3\"\n";
        assert_eq!(version_from_manifest(manifest).unwrap(), "1.2.3");
    }

    #[test]
    fn test_version_from_manifest_rejects_a_manifest_without_the_key() {
        assert!(version_from_manifest("[workspace]\nmembers = []\n").is_err());
    }

    #[test]
    fn test_version_from_addon_conf_reads_the_key_among_other_sections() {
        let conf = "[Addon]\nName=Rust Pinyin\nVersion=0.1.0\nLibrary=librspinyin\n";
        assert_eq!(version_from_addon_conf(conf).unwrap(), "0.1.0");
    }

    #[test]
    fn test_version_from_addon_conf_rejects_a_missing_key() {
        assert!(version_from_addon_conf("[Addon]\nName=Rust Pinyin\n").is_err());
    }

    #[test]
    fn test_version_from_addon_conf_rejects_an_empty_value() {
        // An empty value must not silently compare equal to anything.
        assert!(version_from_addon_conf("[Addon]\nVersion=\n").is_err());
    }

    #[test]
    fn test_repository_files_agree() {
        // The gate itself, run against the real files.
        run().unwrap();
    }
}
