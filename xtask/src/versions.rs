//! Consistency between the addon descriptors and the workspace manifest.
//!
//! Fcitx5 reports the version from the addon descriptor, not from the library, so a
//! descriptor left behind by a version bump ships a plugin that mislabels itself. The
//! two are separate files with no mechanical link, which is exactly the kind of drift a
//! gate has to catch.
//!
//! Since ADR-0003 there is more than one descriptor, and the second thing that can drift
//! is the fcitx5 floor they declare. The two addons are one product, so a UI addon that
//! loads where the engine does not — or the reverse — leaves the user with half an input
//! method, and because every entry in `[Addon/Dependencies]` is *required*, the failure
//! is a silent refusal to load rather than a diagnostic.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

/// Directory holding the Fcitx5 descriptors, relative to the repository root.
const CONF_DIR: &str = "packaging/fcitx5";

/// The key Fcitx5 reads the version from.
const VERSION_KEY: &str = "Version";

/// The section an addon descriptor is identified by.
const ADDON_SECTION: &str = "Addon";

/// The section declaring the fcitx5 version floor.
const DEPENDENCY_SECTION: &str = "Addon/Dependencies";

/// Checks every addon descriptor against the workspace package version and each other.
///
/// # Errors
///
/// Returns an error when the descriptor directory cannot be read, when it holds fewer
/// than two addon descriptors, when a descriptor has no `Version` key or no `core:`
/// dependency, when a version differs from the workspace's, or when the two descriptors
/// declare different fcitx5 floors.
pub fn run() -> Result<()> {
    let root = repo_root()?;
    let manifest = version_from_manifest(&read(&root.join("Cargo.toml"))?)?;
    let descriptors = addon_descriptors(&root.join(CONF_DIR))?;

    ensure!(
        descriptors.len() >= 2,
        "{CONF_DIR} holds {} addon descriptor(s), but the plugin is two addons: the \
         engine (`Category=InputMethod`) and the user interface (`Category=UI`). \
         Fcitx5 picks the active user interface from the addons it discovered with \
         `Category=UI`, so one addon cannot serve both roles.",
        descriptors.len()
    );

    let mut floors: Vec<(PathBuf, String)> = Vec::new();
    for path in &descriptors {
        let label = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        let conf = read(path)?;

        let version = value_in_section(&conf, ADDON_SECTION, VERSION_KEY)
            .with_context(|| format!("{label} has no {VERSION_KEY} key in [{ADDON_SECTION}]"))?;
        ensure!(
            version == manifest,
            "{label} advertises version {version}, but the workspace package version is \
             {manifest}. Fcitx5 reports the descriptor's value, so a mismatch ships a \
             mislabelled plugin."
        );

        let floor = value_in_section(&conf, DEPENDENCY_SECTION, "0")
            .with_context(|| format!("{label} declares no [ {DEPENDENCY_SECTION} ] entry 0"))?;
        ensure!(
            floor.starts_with("core:"),
            "{label} declares `{floor}` as its first dependency; the core addon is the \
             one genuine dependency and must be listed as `core:<version>`."
        );
        floors.push((path.clone(), floor));
    }

    let (first_path, first_floor) = &floors[0];
    for (path, floor) in &floors[1..] {
        ensure!(
            floor == first_floor,
            "{} declares `{floor}` while {} declares `{first_floor}`. The two addons are \
             one product, and every entry in [{DEPENDENCY_SECTION}] is a required \
             dependency: a descriptor with a higher floor makes Fcitx5 suppress that \
             addon silently, leaving the user with half an input method.",
            path.display(),
            first_path.display(),
        );
    }

    println!(
        "check-versions: {} addon descriptors match the workspace version {manifest} \
         and the fcitx5 floor {first_floor}",
        descriptors.len()
    );
    Ok(())
}

/// The addon descriptors under `dir`, sorted by path.
///
/// A `.conf` file counts as an addon descriptor when it has an `[Addon]` section; the
/// input-method entry (`rspinyin-im.conf`) has none and is not one.
fn addon_descriptors(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    let mut found = Vec::new();
    for entry in entries {
        let path = entry
            .with_context(|| format!("reading {}", dir.display()))?
            .path();
        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("conf") {
            continue;
        }
        let conf = read(&path)?;
        if value_in_section(&conf, ADDON_SECTION, "Category").is_some() {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
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

/// Reads `key` from inside `[section]` of an INI-shaped Fcitx5 descriptor.
///
/// Section-scoped rather than a plain scan, because the same key name can legitimately
/// appear in more than one section and the assertions are about *which* one. Only the
/// first occurrence inside the section is honoured, which matches how Fcitx5's own
/// parser resolves a duplicated key.
fn value_in_section(conf: &str, section: &str, key: &str) -> Option<String> {
    let header = format!("[{section}]");
    let mut inside = false;
    for line in conf.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == header;
            continue;
        }
        if !inside {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() == key {
            let value = value.trim();
            if value.is_empty() {
                return None;
            }
            return Some(value.to_owned());
        }
    }
    None
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
    fn test_value_in_section_reads_the_key_among_other_sections() {
        let conf = "[Addon]\nName=Rust Pinyin\nVersion=0.1.0\nLibrary=librspinyin\n";
        assert_eq!(
            value_in_section(conf, "Addon", "Version").as_deref(),
            Some("0.1.0")
        );
    }

    #[test]
    fn test_value_in_section_does_not_read_across_sections() {
        // The whole point of the section scope: a `Version` in another section is not the
        // addon's version, and reading it would make the gate pass on the wrong value.
        let conf = "[Addon]\nName=Rust Pinyin\n\n[Addon/Dependencies]\n0=core:5.1.7\n";
        assert_eq!(value_in_section(conf, "Addon", "Version"), None);
        assert_eq!(
            value_in_section(conf, "Addon/Dependencies", "0").as_deref(),
            Some("core:5.1.7")
        );
    }

    #[test]
    fn test_value_in_section_rejects_a_missing_and_an_empty_value() {
        assert_eq!(
            value_in_section("[Addon]\nName=x\n", "Addon", "Version"),
            None
        );
        // An empty value must not silently compare equal to anything.
        assert_eq!(
            value_in_section("[Addon]\nVersion=\n", "Addon", "Version"),
            None
        );
    }

    #[test]
    fn test_addon_descriptors_skips_a_file_without_an_addon_section() {
        let scratch =
            std::env::temp_dir().join(format!("rspinyin-versions-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(
            scratch.join("engine.conf"),
            "[Addon]\nCategory=InputMethod\n",
        )
        .unwrap();
        std::fs::write(scratch.join("ui.conf"), "[Addon]\nCategory=UI\n").unwrap();
        std::fs::write(scratch.join("im.conf"), "[InputMethod]\nName=Rust Pinyin\n").unwrap();
        std::fs::write(scratch.join("notes.txt"), "[Addon]\nCategory=UI\n").unwrap();

        let found = addon_descriptors(&scratch).unwrap();
        let names: Vec<String> = found
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            ["engine.conf", "ui.conf"],
            "only `.conf` files carrying an [Addon] section are addon descriptors"
        );

        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn test_repository_files_agree() {
        // The gate itself, run against the real files.
        run().unwrap();
    }
}
