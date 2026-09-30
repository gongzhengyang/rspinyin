//! Refreshing the desktop's icon index after the icons have landed.
//!
//! Responsibility: run the icon-theme cache tool over the theme directory the icons were
//! installed into, once, at the end of an install. Without it a panel that looks the
//! input method's icon up by name finds the file but not the theme's index of it, which
//! is the difference between an icon and a placeholder that only shows up after a reboot.
//!
//! Skipped for a `DESTDIR` staging tree: the cache has to describe the tree the icons end
//! up in, which is the packaging step's business, and indexing the staging path would
//! leave an index describing directories that do not exist on the target system.
//!
//! Boundaries: it runs one external tool and reads nothing. A machine without the tool
//! gets a note rather than a failure -- the icons are installed either way, and the index
//! is rebuilt by whatever else touches the theme.

use std::io::ErrorKind;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};

use super::layout::Layout;
use super::manifest::{Elevation, run_sudo};

/// The tool that rebuilds an icon theme's index.
const TOOL: &str = "gtk-update-icon-cache";

/// Refreshes the icon theme cache, when the tool that does it is installed.
///
/// `elevation` is the one the icons were installed through: the theme directory belongs
/// to root on every distribution that ships one, and the index goes into it, so running
/// the tool in this process would fail on a permission the install itself never hit --
/// and would do so after the plugin was already in place.
///
/// # Errors
///
/// Returns an error when the tool runs and fails.
pub(super) fn update_icon_cache(layout: &Layout, elevation: Elevation) -> Result<()> {
    if !layout.destdir.as_os_str().is_empty() || !layout.icon_dir.exists() {
        return Ok(());
    }
    if !is_installed() {
        println!("install: `{TOOL}` is not installed; the icon theme cache was not refreshed");
        return Ok(());
    }
    let arguments = vec![
        "-q".to_owned(),
        "-t".to_owned(),
        "-f".to_owned(),
        layout.icon_dir.display().to_string(),
    ];
    if elevation.is_sudo() {
        return run_sudo(TOOL, &arguments);
    }
    let output = Command::new(TOOL)
        .args(&arguments)
        .output()
        .with_context(|| format!("running {TOOL} on {}", layout.icon_dir.display()))?;
    ensure!(
        output.status.success(),
        "{TOOL} {} failed: {}",
        layout.icon_dir.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// Whether the icon-theme tool is on this machine at all.
///
/// Asked by running it, because the two branches answer the question differently: without
/// elevation a missing tool is a spawn failure, and through `sudo` it is `sudo`'s own exit
/// status, which is indistinguishable from the tool failing. Probing first is what lets
/// both branches report a missing tool as the note it is.
fn is_installed() -> bool {
    !matches!(
        Command::new(TOOL)
            .arg("--help")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status(),
        Err(error) if error.kind() == ErrorKind::NotFound
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A layout whose icons live in a scratch theme directory.
    fn layout(root: &std::path::Path, destdir: &str) -> Layout {
        let destdir = PathBuf::from(destdir);
        Layout {
            addon_dir: destdir.join("usr/lib/fcitx5"),
            addon_conf_dir: destdir.join("usr/share/fcitx5/addon"),
            input_method_dir: destdir.join("usr/share/fcitx5/inputmethod"),
            data_dir: destdir.join("usr/share/rspinyin"),
            metainfo_dir: destdir.join("usr/share/metainfo"),
            icon_dir: root.join("usr/share/icons/hicolor"),
            prefix: PathBuf::from("/usr"),
            destdir,
        }
    }

    #[test]
    fn test_update_icon_cache_skips_a_staging_tree() {
        // The cache has to describe the tree the icons end up in, so indexing a `DESTDIR`
        // staging path would leave an index describing directories that do not exist on
        // the target system. A tool that ran anyway would fail on a directory with no
        // theme index in it, and the assertion below is what says it did not run. The
        // elevation is never `Sudo` here: a test must not be able to reach `sudo`.
        let root = std::env::temp_dir().join(format!("rspinyin-icon-cache-{}", std::process::id()));
        let icon_dir = root.join("usr/share/icons/hicolor");
        std::fs::create_dir_all(icon_dir.join("48x48/apps")).expect("creating the theme tree");
        let layout = layout(&root, "stage");

        update_icon_cache(&layout, Elevation::Direct).expect("a staging tree is skipped");

        assert!(
            !icon_dir.join("icon-theme.cache").exists(),
            "nothing was indexed for a tree the icons are not installed into"
        );
        std::fs::remove_dir_all(&root).expect("cleaning up");
    }

    #[test]
    fn test_update_icon_cache_skips_a_theme_directory_that_is_not_there() {
        let root =
            std::env::temp_dir().join(format!("rspinyin-icon-absent-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("creating the scratch directory");
        let layout = layout(&root, "");

        update_icon_cache(&layout, Elevation::Direct)
            .expect("an install that has not put any icons down has no theme to index");
        assert!(!layout.icon_dir.exists());
        std::fs::remove_dir_all(&root).expect("cleaning up");
    }
}
