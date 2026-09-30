//! The installed layout: which directories the plugin's files belong in, and where
//! the build leaves the copies that go there.
//!
//! Responsibility: resolve the destination directories from the target system's own
//! Fcitx5 installation, apply `PREFIX` and `DESTDIR`, and name every destination path.
//! It decides *where*; the payload table in [`super`] decides *what*, and
//! [`super::manifest`] decides *how* the files get there. Nothing here touches the
//! filesystem beyond running `pkg-config`.
//!
//! # Why the directories are queried instead of assumed
//!
//! Fcitx5 installs its addons under the library directory its own build chose:
//! `/usr/lib/fcitx5` on Arch, `/usr/lib64/fcitx5` on Fedora, and
//! `/usr/lib/x86_64-linux-gnu/fcitx5` on Debian and Ubuntu. A hardcoded path installs
//! the plugin where Fcitx5 never looks for it, so every directory is derived from the
//! target system's `pkg-config`.
//!
//! The specification this module implements asks for
//! `pkg-config --variable=addondir Fcitx5Core`. Measured against Fcitx5 5.1.7 on
//! Ubuntu 24.04, `Fcitx5Core.pc` defines `prefix`, `exec_prefix`, `libdir` and
//! `includedir` only -- `addondir`, `icondir` and `datadir` are all absent, and
//! `pkg-config --variable=addondir Fcitx5Core` prints nothing at all. The variables are
//! therefore queried first, so a distribution that does define them is honoured, and
//! the absent ones are derived from the present ones. That derivation reproduces the
//! paths Fcitx5's own build installs to: addons under `${CMAKE_INSTALL_LIBDIR}/fcitx5`
//! and data under `${CMAKE_INSTALL_DATADIR}`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, ensure};

/// The `pkg-config` module Fcitx5 publishes its core installation under.
const PC_MODULE: &str = "Fcitx5Core";

/// Directory Fcitx5 appends to the library directory for its addons.
const ADDON_SUBDIR: &str = "fcitx5";

/// Directory the plugin owns inside the data directory.
const PROGRAM_DIR: &str = "rspinyin";

/// Directory the icon theme keeps its application icons in.
const ICON_THEME: &str = "hicolor";

/// File name of the manifest that records what an install put on the system.
pub const MANIFEST_FILE: &str = "install-manifest.json";

/// Suffix the installer gives the copy of a file it displaced.
pub const BACKUP_SUFFIX: &str = ".rspinyin-bak";

/// The `platform/fcitx5/dev-missing` diagnostic, and the packages that clear it.
///
/// The code is part of the diagnostic contract and is asserted by the acceptance
/// tests, so it must not be reworded. The wording mirrors `ime-fcitx5`'s build script,
/// because the same missing development package breaks both and a user who sees one
/// message should recognise the other.
const DEV_MISSING: &str = "platform/fcitx5/dev-missing: could not find the Fcitx5 \
     development package `Fcitx5Core` via pkg-config.\n\
     Install it with one of:\n  \
     Debian/Ubuntu: sudo apt install libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev\n  \
     Fedora:        sudo dnf install fcitx5-devel\n  \
     Arch:          sudo pacman -S fcitx5";

/// One destination directory of the installed layout, as Fcitx5 looks it up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    /// Shared libraries Fcitx5 `dlopen`s, under `<libdir>/fcitx5`.
    AddonLibrary,
    /// Addon descriptors, under `<datadir>/fcitx5/addon`.
    AddonDescriptor,
    /// Input-method descriptors, under `<datadir>/fcitx5/inputmethod`.
    InputMethodDescriptor,
    /// Plugin data, under `<datadir>/rspinyin`.
    Data,
    /// AppStream metadata, under `<datadir>/metainfo`.
    ///
    /// Separate from [`Destination::Data`] because the directory is shared with every
    /// other AppStream component on the system, which is also why uninstall must not
    /// remove it: only the file this plugin put there is the plugin's to delete.
    MetaInfo,
    /// Application icons, under `<datadir>/icons/hicolor/<size>/apps`.
    Icon {
        /// Icon size directory: `48x48` for the bitmap, `scalable` for the vector.
        size: &'static str,
    },
}

/// Every directory the plugin installs into, with `DESTDIR` already applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// `DESTDIR` as given; empty for an install straight into the target system.
    pub destdir: PathBuf,
    /// Installation prefix every destination is rooted at.
    pub prefix: PathBuf,
    /// `<libdir>/fcitx5`.
    pub addon_dir: PathBuf,
    /// `<datadir>/fcitx5/addon`.
    pub addon_conf_dir: PathBuf,
    /// `<datadir>/fcitx5/inputmethod`.
    pub input_method_dir: PathBuf,
    /// `<datadir>/rspinyin`.
    pub data_dir: PathBuf,
    /// `<datadir>/metainfo`.
    pub metainfo_dir: PathBuf,
    /// `<datadir>/icons/hicolor`.
    pub icon_dir: PathBuf,
}

/// The `pkg-config` variables the layout is derived from.
///
/// Kept as a plain value rather than read inside the derivation, so that the derivation
/// -- the part that decides whether the plugin lands where Fcitx5 looks for it -- can be
/// exercised against the variables a distribution reports without that distribution
/// being installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variables {
    /// `prefix`, the installation prefix Fcitx5 was built for.
    pub prefix: Option<String>,
    /// `libdir`; the addon directory is derived from it when `addondir` is absent.
    pub libdir: Option<String>,
    /// `datadir`; the descriptors and the plugin's own data are derived from it.
    pub datadir: Option<String>,
    /// `addondir`, where Fcitx5 looks for the shared libraries it `dlopen`s.
    pub addondir: Option<String>,
    /// `icondir`, the root of the icon theme.
    pub icondir: Option<String>,
}

impl Variables {
    /// Reads every variable the layout needs from the target system's `pkg-config`.
    ///
    /// # Errors
    ///
    /// Returns an error when `pkg-config` cannot be run, when it cannot find
    /// `Fcitx5Core` -- the message carries the `platform/fcitx5/dev-missing` code and
    /// the packages that clear it -- and when its output is not UTF-8.
    pub fn probe() -> Result<Self> {
        Ok(Self {
            prefix: variable(PC_MODULE, "prefix")?,
            libdir: variable(PC_MODULE, "libdir")?,
            datadir: variable(PC_MODULE, "datadir")?,
            addondir: variable(PC_MODULE, "addondir")?,
            icondir: variable(PC_MODULE, "icondir")?,
        })
    }
}

impl Layout {
    /// Resolves the layout from the target system's Fcitx5 installation.
    ///
    /// `prefix_override` replaces the installation prefix `pkg-config` reports, which is
    /// what a distribution package or an install into `/usr/local` needs; `destdir` is
    /// prepended to every resolved path and is left empty for an install straight into
    /// the target system.
    ///
    /// # Errors
    ///
    /// Returns an error when `pkg-config` cannot find the Fcitx5 development package,
    /// and as [`Layout::from_variables`].
    pub fn probe(prefix_override: Option<&Path>, destdir: Option<&Path>) -> Result<Self> {
        Self::from_variables(&Variables::probe()?, prefix_override, destdir)
    }

    /// Derives the layout from the variables `pkg-config` reported.
    ///
    /// # Errors
    ///
    /// Returns an error when the variables name no installation prefix, which would
    /// leave every destination undefined.
    pub fn from_variables(
        variables: &Variables,
        prefix_override: Option<&Path>,
        destdir: Option<&Path>,
    ) -> Result<Self> {
        let reported = PathBuf::from(
            variables
                .prefix
                .as_deref()
                .context("pkg-config reports no installation prefix for Fcitx5Core")?,
        );
        let libdir = reported_or(&variables.libdir, reported.join("lib"));
        let datadir = reported_or(&variables.datadir, reported.join("share"));
        let addon_dir = reported_or(&variables.addondir, libdir.join(ADDON_SUBDIR));
        let icon_dir = reported_or(&variables.icondir, datadir.join("icons"));
        let prefix = prefix_override.map_or_else(|| reported.clone(), Path::to_path_buf);
        let destdir = destdir.map_or_else(PathBuf::new, Path::to_path_buf);
        let place =
            |path: PathBuf| under_destdir(&destdir, &rebase_prefix(&path, &reported, &prefix));
        Ok(Self {
            addon_dir: place(addon_dir),
            addon_conf_dir: place(datadir.join(ADDON_SUBDIR).join("addon")),
            input_method_dir: place(datadir.join(ADDON_SUBDIR).join("inputmethod")),
            data_dir: place(datadir.join(PROGRAM_DIR)),
            metainfo_dir: place(datadir.join("metainfo")),
            icon_dir: place(icon_dir.join(ICON_THEME)),
            destdir,
            prefix,
        })
    }

    /// Names the directory one destination role installs into.
    pub fn directory(&self, destination: Destination) -> PathBuf {
        match destination {
            Destination::AddonLibrary => self.addon_dir.clone(),
            Destination::AddonDescriptor => self.addon_conf_dir.clone(),
            Destination::InputMethodDescriptor => self.input_method_dir.clone(),
            Destination::Data => self.data_dir.clone(),
            Destination::MetaInfo => self.metainfo_dir.clone(),
            Destination::Icon { size } => self.icon_dir.join(size).join("apps"),
        }
    }

    /// Names the destination path of one payload file.
    pub fn destination(&self, destination: Destination, name: &str) -> PathBuf {
        self.directory(destination).join(name)
    }

    /// Every directory an install writes into.
    ///
    /// Used to check writability before the first copy, so that an install which cannot
    /// finish does not start.
    pub fn destination_directories(&self) -> Vec<PathBuf> {
        [
            Destination::AddonLibrary,
            Destination::AddonDescriptor,
            Destination::InputMethodDescriptor,
            Destination::Data,
            Destination::MetaInfo,
            Destination::Icon { size: "48x48" },
            Destination::Icon { size: "scalable" },
        ]
        .into_iter()
        .map(|destination| self.directory(destination))
        .collect()
    }

    /// Path of the manifest that records this installation.
    pub fn manifest_path(&self) -> PathBuf {
        self.data_dir.join(MANIFEST_FILE)
    }
}

/// The build outputs an install copies from, and the scratch space it stages them in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    /// Repository root.
    pub root: PathBuf,
    /// `target/install`, where a library is stripped and verified before it is copied.
    pub staging_dir: PathBuf,
}

impl Sources {
    /// Derives the build directories from a repository root.
    pub fn new(root: PathBuf) -> Self {
        Self {
            staging_dir: root.join("target").join("install"),
            root,
        }
    }

    /// Resolves the built file of a payload from its candidate paths.
    ///
    /// The candidates exist because a build artifact can legitimately be written to
    /// more than one place; the first one present is the one an install uses.
    ///
    /// # Errors
    ///
    /// Returns an error naming every candidate when none of them exists, so the caller
    /// can see which build step has not run.
    pub fn artifact(&self, candidates: &[&str]) -> Result<PathBuf> {
        for candidate in candidates {
            let path = self.root.join(candidate);
            if path.exists() {
                return Ok(path);
            }
        }
        anyhow::bail!(
            "none of the build outputs exists: {}",
            candidates.join(", ")
        )
    }
}

/// Reads one variable of a `pkg-config` module.
///
/// `module` is a parameter rather than the constant, so that the answer a machine without
/// the Fcitx5 development package gets can be asserted without that package being absent.
///
/// # Errors
///
/// Returns an error when `pkg-config` cannot be run, when it exits non-zero -- which is
/// how it reports a module it cannot find, and therefore how a missing development
/// package surfaces -- or when its output is not UTF-8. A variable that is undefined or
/// defined but empty is `Ok(None)`: `pkg-config` prints nothing for both, and the
/// caller derives the path instead.
fn variable(module: &str, name: &str) -> Result<Option<String>> {
    let output = Command::new("pkg-config")
        .arg(format!("--variable={name}"))
        .arg(module)
        .output()
        .with_context(|| format!("{DEV_MISSING}\n(`pkg-config` could not be run)"))?;
    ensure!(output.status.success(), "{DEV_MISSING}");
    let value = String::from_utf8(output.stdout)
        .context("pkg-config wrote a variable that is not UTF-8")?;
    let value = value.trim();
    Ok((!value.is_empty()).then(|| value.to_owned()))
}

/// A variable's value as a path, or the path derived from it when it is unset.
///
/// `pkg-config` prints nothing for both an undefined variable and an empty one, so the
/// two are the same answer here: derive the path.
fn reported_or(reported: &Option<String>, derived: PathBuf) -> PathBuf {
    match reported {
        Some(value) => PathBuf::from(value),
        None => derived,
    }
}

/// Rewrites a path `pkg-config` reported under `from` so that it sits under `to`.
///
/// A path outside `from` is returned unchanged: it was configured as an absolute
/// location independent of the prefix, and moving it would put the file somewhere the
/// user did not ask for.
fn rebase_prefix(path: &Path, from: &Path, to: &Path) -> PathBuf {
    match path.strip_prefix(from) {
        Ok(rest) => to.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

/// Prepends `destdir` to an absolute `path`.
///
/// `DESTDIR` is a staging prefix, not a relocation: the file still belongs at `path`
/// once the staging tree is unpacked onto the target system, so only the leading
/// separator is dropped before the join.
fn under_destdir(destdir: &Path, path: &Path) -> PathBuf {
    if destdir.as_os_str().is_empty() {
        return path.to_path_buf();
    }
    destdir.join(path.strip_prefix("/").unwrap_or(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A layout built by hand, so that the path naming is tested without a target
    /// system's Fcitx5 installation.
    fn layout(destdir: &str) -> Layout {
        let destdir = PathBuf::from(destdir);
        // Delegates to the production `under_destdir` instead of repeating its
        // `strip_prefix` by hand. An earlier version of this fixture did repeat it and
        // dropped the empty-`DESTDIR` case, so it produced relative paths and the
        // assertions below were checking a layout the installer never builds. The closure
        // owns its own copy so the original can still be moved into the struct below; a
        // borrowed capture would keep it alive past the move.
        let place = {
            let root = destdir.clone();
            move |path: &str| under_destdir(&root, Path::new(path))
        };
        Layout {
            destdir,
            prefix: PathBuf::from("/usr"),
            addon_dir: place("/usr/lib/x86_64-linux-gnu/fcitx5"),
            addon_conf_dir: place("/usr/share/fcitx5/addon"),
            input_method_dir: place("/usr/share/fcitx5/inputmethod"),
            data_dir: place("/usr/share/rspinyin"),
            metainfo_dir: place("/usr/share/metainfo"),
            icon_dir: place("/usr/share/icons/hicolor"),
        }
    }

    /// The variables a Debian or Ubuntu Fcitx5 reports.
    ///
    /// `Fcitx5Core.pc` there defines `prefix` and `libdir` and nothing else, which is
    /// what makes the derivation -- rather than the variable itself -- the part that
    /// decides where the plugin lands.
    fn ubuntu_variables() -> Variables {
        Variables {
            prefix: Some("/usr".to_owned()),
            libdir: Some("/usr/lib/x86_64-linux-gnu".to_owned()),
            datadir: None,
            addondir: None,
            icondir: None,
        }
    }

    #[test]
    fn test_from_variables_derives_every_directory_when_only_the_prefix_is_reported() {
        let layout =
            Layout::from_variables(&ubuntu_variables(), None, None).expect("a prefix is reported");
        assert_eq!(
            layout.addon_dir,
            PathBuf::from("/usr/lib/x86_64-linux-gnu/fcitx5"),
            "the addon directory follows libdir, which is where Fcitx5's build put it"
        );
        assert_eq!(
            layout.addon_conf_dir,
            PathBuf::from("/usr/share/fcitx5/addon")
        );
        assert_eq!(
            layout.input_method_dir,
            PathBuf::from("/usr/share/fcitx5/inputmethod")
        );
        assert_eq!(layout.data_dir, PathBuf::from("/usr/share/rspinyin"));
        assert_eq!(layout.icon_dir, PathBuf::from("/usr/share/icons/hicolor"));
    }

    #[test]
    fn test_from_variables_prefers_a_reported_directory_over_the_derived_one() {
        let variables = Variables {
            datadir: Some("/usr/local/share".to_owned()),
            addondir: Some("/usr/lib64/fcitx5".to_owned()),
            icondir: Some("/usr/local/share/pixmaps".to_owned()),
            ..ubuntu_variables()
        };
        let layout = Layout::from_variables(&variables, None, None).expect("a prefix is reported");

        assert_eq!(layout.addon_dir, PathBuf::from("/usr/lib64/fcitx5"));
        assert_eq!(
            layout.addon_conf_dir,
            PathBuf::from("/usr/local/share/fcitx5/addon")
        );
        assert_eq!(
            layout.icon_dir,
            PathBuf::from("/usr/local/share/pixmaps/hicolor")
        );
    }

    #[test]
    fn test_from_variables_rebases_every_path_onto_an_overridden_prefix() {
        let layout =
            Layout::from_variables(&ubuntu_variables(), Some(Path::new("/usr/local")), None)
                .expect("a prefix is reported");

        assert_eq!(layout.prefix, PathBuf::from("/usr/local"));
        assert_eq!(
            layout.destdir,
            PathBuf::new(),
            "no staging tree was asked for"
        );
        assert_eq!(
            layout.addon_dir,
            PathBuf::from("/usr/local/lib/x86_64-linux-gnu/fcitx5")
        );
        assert_eq!(
            layout.addon_conf_dir,
            PathBuf::from("/usr/local/share/fcitx5/addon")
        );
        assert_eq!(
            layout.icon_dir,
            PathBuf::from("/usr/local/share/icons/hicolor")
        );
    }

    #[test]
    fn test_from_variables_prepends_the_destdir_to_every_path() {
        let layout = Layout::from_variables(&ubuntu_variables(), None, Some(Path::new("stage")))
            .expect("a prefix is reported");

        assert_eq!(layout.destdir, PathBuf::from("stage"));
        assert_eq!(
            layout.addon_dir,
            PathBuf::from("stage/usr/lib/x86_64-linux-gnu/fcitx5")
        );
        assert_eq!(layout.data_dir, PathBuf::from("stage/usr/share/rspinyin"));
    }

    #[test]
    fn test_from_variables_refuses_variables_without_a_prefix() {
        let variables = Variables {
            prefix: None,
            ..ubuntu_variables()
        };
        let failure = Layout::from_variables(&variables, None, None)
            .expect_err("a layout cannot be derived without a prefix");
        assert!(failure.to_string().contains("prefix"), "{failure}");
    }

    #[test]
    fn test_rebase_prefix_rewrites_a_path_under_the_reported_prefix() {
        // The multiarch library directory is under `/usr`, so a `--prefix /usr/local`
        // install keeps its shape and only the root moves.
        assert_eq!(
            rebase_prefix(
                Path::new("/usr/lib/x86_64-linux-gnu/fcitx5"),
                Path::new("/usr"),
                Path::new("/usr/local")
            ),
            PathBuf::from("/usr/local/lib/x86_64-linux-gnu/fcitx5")
        );
    }

    #[test]
    fn test_rebase_prefix_leaves_a_path_outside_the_reported_prefix_alone() {
        // An absolute path configured independently of the prefix is not ours to move.
        assert_eq!(
            rebase_prefix(
                Path::new("/opt/fcitx5"),
                Path::new("/usr"),
                Path::new("/usr/local")
            ),
            PathBuf::from("/opt/fcitx5")
        );
    }

    #[test]
    fn test_under_destdir_prepends_only_for_a_non_empty_destdir() {
        let path = Path::new("/usr/share/fcitx5/addon/rspinyin.conf");
        assert_eq!(under_destdir(Path::new(""), path), path.to_path_buf());
        assert_eq!(
            under_destdir(Path::new("debian/rspinyin"), path),
            PathBuf::from("debian/rspinyin/usr/share/fcitx5/addon/rspinyin.conf")
        );
    }

    #[test]
    fn test_destination_names_every_role_under_its_own_directory() {
        let layout = layout("");
        assert_eq!(
            layout.destination(Destination::AddonLibrary, "librspinyin.so"),
            PathBuf::from("/usr/lib/x86_64-linux-gnu/fcitx5/librspinyin.so")
        );
        assert_eq!(
            layout.destination(Destination::AddonDescriptor, "rspinyin.conf"),
            PathBuf::from("/usr/share/fcitx5/addon/rspinyin.conf")
        );
        assert_eq!(
            layout.destination(Destination::InputMethodDescriptor, "rspinyin.conf"),
            PathBuf::from("/usr/share/fcitx5/inputmethod/rspinyin.conf")
        );
        assert_eq!(
            layout.destination(Destination::Data, "base.dict"),
            PathBuf::from("/usr/share/rspinyin/base.dict")
        );
        assert_eq!(
            layout.destination(Destination::Icon { size: "48x48" }, "fcitx-rspinyin.png"),
            PathBuf::from("/usr/share/icons/hicolor/48x48/apps/fcitx-rspinyin.png")
        );
        assert_eq!(
            layout.destination(Destination::Icon { size: "scalable" }, "fcitx-rspinyin.svg"),
            PathBuf::from("/usr/share/icons/hicolor/scalable/apps/fcitx-rspinyin.svg")
        );
    }

    #[test]
    fn test_destination_directories_carries_the_destdir_into_every_entry() {
        let directories = layout("stage").destination_directories();
        assert_eq!(directories.len(), 6);
        assert!(
            directories.iter().all(|dir| dir.starts_with("stage/usr")),
            "every directory is staged: {directories:?}"
        );
        assert!(directories.contains(&PathBuf::from("stage/usr/share/fcitx5/addon")));
    }

    #[test]
    fn test_manifest_path_lives_in_the_data_directory() {
        assert_eq!(
            layout("").manifest_path(),
            PathBuf::from("/usr/share/rspinyin/install-manifest.json")
        );
    }

    #[test]
    fn test_sources_artifact_returns_the_first_candidate_that_exists() {
        let root = std::env::temp_dir().join(format!("rspinyin-layout-{}", std::process::id()));
        std::fs::create_dir_all(root.join("second")).expect("creating the scratch directory");
        std::fs::write(root.join("second/base.dict"), b"x").expect("writing the fixture");
        let sources = Sources::new(root.clone());

        assert_eq!(
            sources
                .artifact(&["first/base.dict", "second/base.dict"])
                .expect("the second candidate exists"),
            root.join("second/base.dict")
        );
        let failure = sources
            .artifact(&["first/base.dict", "third/base.dict"])
            .expect_err("no candidate exists");
        assert!(failure.to_string().contains("third/base.dict"), "{failure}");
        std::fs::remove_dir_all(&root).expect("cleaning up");
    }

    #[test]
    fn test_variable_reports_the_dev_missing_diagnostic_for_a_module_that_is_not_there() {
        // The diagnostic is the one thing that turns "cannot find Fcitx5Core" into an
        // installable prerequisite, so it has to survive both ways `pkg-config` can fail:
        // a module it cannot find, and a machine that has no `pkg-config` at all.
        let failure = variable("RspinyinNoSuchPkgConfigModule", "prefix")
            .expect_err("no such pkg-config module exists");
        assert!(
            failure.to_string().contains("platform/fcitx5/dev-missing"),
            "{failure}"
        );
        assert!(
            failure.to_string().contains("libfcitx5core-dev"),
            "the message names the package that clears it: {failure}"
        );
    }

    #[test]
    fn test_variables_probe_reports_an_absolute_prefix_or_the_missing_package() {
        // This is the call that asks the target system where its Fcitx5 lives, and the
        // answer decides where the plugin lands. Either the development package is here,
        // in which case the probe reports the installation it describes, or it is not,
        // in which case the answer has to be the diagnostic rather than a layout derived
        // from nothing.
        match Variables::probe() {
            Ok(variables) => assert!(
                variables
                    .prefix
                    .as_deref()
                    .is_some_and(|prefix| prefix.starts_with('/')),
                "a probed installation prefix is absolute: {:?}",
                variables.prefix
            ),
            Err(error) => assert!(
                error.to_string().contains("platform/fcitx5/dev-missing"),
                "{error}"
            ),
        }
    }
}
