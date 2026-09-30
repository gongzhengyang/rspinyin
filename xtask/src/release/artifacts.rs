//! What a release publishes, and what it must publish.
//!
//! Responsibility: decide what each file in an assembled release directory is, refuse a
//! directory that holds a file a release does not publish, and refuse one that is missing a
//! file it must.
//!
//! # Why the directory is checked at all
//!
//! A manifest written from whatever happens to be in the directory describes whatever the
//! pipeline produced -- including a release that is short one architecture's package because
//! a matrix job failed quietly, or one that carries a stray file nobody meant to publish. The
//! set of files a release publishes is stated here and checked against the directory, so an
//! incomplete release stops before it is signed rather than after it is public.
//!
//! # What this is not
//!
//! It is not a verification. A file whose bytes are wrong is still the file this module
//! expects: the digest recorded for it is read from the file itself, and it is the signature
//! over the checksum list that makes that digest mean anything. `xtask verify` is the command
//! that checks a release, and the pipeline runs it after signing.
//!
//! # Why an architecture is a type rather than a string
//!
//! The three ecosystems spell the same two architectures three ways: `amd64` and `arm64` in a
//! Debian package's name, `x86_64` and `aarch64` everywhere else. A release that mixed them
//! up would publish a package the runner did not build, and a string-typed architecture is
//! exactly where that mistake hides.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

/// File name of the public signing key a release publishes beside its artifacts.
///
/// The name the repository commits it under, so the pipeline copies rather than renames it:
/// a user follows `packaging/keys/README.md` and imports the file it names.
pub const SIGNING_KEY_FILE: &str = "rspinyin-signing-key.asc";

/// An architecture the delivery matrix covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Architecture {
    /// 64-bit x86, which `uname -m` reports as `x86_64`.
    X86_64,
    /// 64-bit ARM, which `uname -m` reports as `aarch64`.
    Aarch64,
}

impl Architecture {
    /// Every architecture, in the order the manifest records them.
    pub const ALL: [Self; 2] = [Self::X86_64, Self::Aarch64];

    /// The spelling `uname -m` reports and the manifest records.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn name(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }

    /// Parses the `uname -m` spelling.
    ///
    /// # Errors
    ///
    /// Returns an error for anything else, naming what the delivery matrix covers.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn parse(text: &str) -> Result<Self> {
        if let Some(arch) = Self::ALL.into_iter().find(|arch| arch.name() == text) {
            return Ok(arch);
        }
        bail!(
            "release: `{text}` is not an architecture the delivery matrix covers ({}). A \
             release is built for the architectures the matrix lists, so a third one needs a \
             matrix row, a runner and a packaging recipe before it can be named here.",
            Self::ALL.map(Self::name).join(", ")
        )
    }
}

/// One of the three distribution package formats a release ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PackageKind {
    /// A Debian package, installed with `apt` or `dpkg`.
    Deb,
    /// An RPM package, installed with `dnf` or `rpm`.
    Rpm,
    /// A pacman package, installed with `pacman`.
    Pkg,
}

impl PackageKind {
    /// Every kind, in the order the manifest records them.
    pub const ALL: [Self; 3] = [Self::Deb, Self::Rpm, Self::Pkg];

    /// The name the pipeline declares it by.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn name(self) -> &'static str {
        match self {
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::Pkg => "pkg",
        }
    }

    /// The extension every package of this kind carries.
    ///
    /// `pkg.tar.zst` rather than `zst`: a pacman package is a tar archive that happens to be
    /// zstd-compressed, and a rule that matched the compression alone would claim any
    /// compressed file in the release directory.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Deb => ".deb",
            Self::Rpm => ".rpm",
            Self::Pkg => ".pkg.tar.zst",
        }
    }

    /// The role the manifest records for a package of this kind.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn role(self) -> &'static str {
        match self {
            Self::Deb => "package-deb",
            Self::Rpm => "package-rpm",
            Self::Pkg => "package-pkg",
        }
    }

    /// How this ecosystem spells `arch` in a file name.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn spelling(self, arch: Architecture) -> &'static str {
        match (self, arch) {
            (Self::Deb, Architecture::X86_64) => "amd64",
            (Self::Deb, Architecture::Aarch64) => "arm64",
            (_, arch) => arch.name(),
        }
    }

    /// Parses the name the pipeline declares a kind by.
    ///
    /// # Errors
    ///
    /// Returns an error for anything that is not one of the three package formats.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn parse(text: &str) -> Result<Self> {
        if let Some(kind) = Self::ALL.into_iter().find(|kind| kind.name() == text) {
            return Ok(kind);
        }
        bail!(
            "release: `{text}` is not a package format this project ships ({}). The delivery \
             matrix fixes the three, and a fourth needs a packaging recipe, a matrix row and a \
             role in this module before it can be named here.",
            Self::ALL.map(Self::name).join(", ")
        )
    }
}

/// What one published file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    /// The source archive, which all three packaging recipes build from.
    SourceArchive,
    /// One distribution package.
    Package {
        /// Which of the three formats it is in.
        kind: PackageKind,
        /// The architecture it was built for.
        arch: Architecture,
    },
    /// The public signing key a user imports before checking the release.
    SigningKey,
}

impl Role {
    /// The role the manifest records for this file.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(&self) -> &'static str {
        match self {
            Self::SourceArchive => "source-archive",
            Self::SigningKey => "signing-key",
            Self::Package { kind, .. } => kind.role(),
        }
    }
}

/// One file the release publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    /// File name inside the release directory.
    pub name: String,
    /// What the file is.
    pub role: Role,
}

impl Published {
    /// The architecture this file was built for, when it is a distribution package.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn arch(&self) -> Option<Architecture> {
        match &self.role {
            Role::Package { arch, .. } => Some(*arch),
            Role::SourceArchive | Role::SigningKey => None,
        }
    }
}

/// The files a release publishes, in the order the manifest records them.
#[derive(Debug, Clone)]
pub struct Set {
    /// Directory the files were read from.
    dir: PathBuf,
    /// The files, sorted by name.
    files: Vec<Published>,
}

impl Set {
    /// Reads the release directory and checks it against what a release must publish.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be read, when it holds a file that is not
    /// one a release publishes, and when a file the release must publish is missing or
    /// appears more than once.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn read(
        dir: &Path,
        version: &str,
        packages: &[(PackageKind, Architecture)],
    ) -> Result<Self> {
        let set = Self {
            dir: dir.to_path_buf(),
            files: collect(dir, version)?,
        };
        set.check(packages)?;
        Ok(set)
    }

    /// The directory the files were read from.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The files, in the order the manifest records them.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn files(&self) -> &[Published] {
        &self.files
    }

    /// The architectures the release was built for, in the order the matrix lists them.
    ///
    /// Read from the packages rather than declared: an architecture the release claims but
    /// published nothing for would be a claim nothing supports.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn architectures(&self) -> Vec<Architecture> {
        let mut found: Vec<Architecture> = Vec::new();
        for file in &self.files {
            if let Some(arch) = file.arch() {
                if !found.contains(&arch) {
                    found.push(arch);
                }
            }
        }
        found.sort();
        found
    }

    /// Fails unless every file the release must publish is there exactly once.
    ///
    /// # Errors
    ///
    /// Returns an error naming the role that is missing, or that is present more than once.
    ///
    /// # Panics
    ///
    /// Never.
    fn check(&self, packages: &[(PackageKind, Architecture)]) -> Result<()> {
        let declared = packages.iter().map(|(kind, arch)| Role::Package {
            kind: *kind,
            arch: *arch,
        });
        let required = std::iter::once(Role::SourceArchive)
            .chain(declared)
            .chain(std::iter::once(Role::SigningKey));
        for role in required {
            let found = self.files.iter().filter(|file| file.role == role).count();
            ensure!(found == 1, "{}", not_exactly_one(&role, found));
        }
        Ok(())
    }
}

/// The refusal for a file the release must publish exactly once.
///
/// # Panics
///
/// Never.
fn not_exactly_one(role: &Role, found: usize) -> String {
    let label = role.label();
    if found == 0 {
        return format!(
            "release: the release directory holds no file with the role `{label}`. A release \
             publishes the source archive, one package per declared `kind:architecture`, and \
             the public signing key ({SIGNING_KEY_FILE}); a directory that is short one of \
             them describes a release that is not complete."
        );
    }
    format!(
        "release: the release directory holds {found} files with the role `{label}`. The \
         manifest records one digest per file, so two files claiming one role would be a \
         release nobody can tell apart from another."
    )
}

/// Reads the files of `dir`, classified, sorted by name.
///
/// # Errors
///
/// Returns an error when the directory cannot be read, and when it holds a file that is not
/// one a release publishes.
///
/// # Panics
///
/// Never.
fn collect(dir: &Path, version: &str) -> Result<Vec<Published>> {
    let entries =
        fs::read_dir(dir).with_context(|| format!("release: reading {}", dir.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("release: reading {}", dir.display()))?;
        let kind = entry
            .file_type()
            .with_context(|| format!("release: reading {}", entry.path().display()))?;
        // Directories are skipped rather than refused: what a release publishes is a set of
        // files, and an unpacked tree beside them has not changed that set.
        if !kind.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let role = classify(&name, version).with_context(|| unclassified(&name))?;
        files.push(Published { name, role });
    }
    files.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(files)
}

/// The refusal for a file no role describes.
///
/// # Panics
///
/// Never.
fn unclassified(name: &str) -> String {
    format!(
        "release: {name} is in the release directory and no role describes it. A release \
         publishes the source archive (`rspinyin-<version>.tar.gz`), one package per declared \
         `kind:architecture`, and the public signing key ({SIGNING_KEY_FILE}). Anything else \
         would be signed into the checksum list without being part of the release."
    )
}

/// What `name` is, or `None` when a release does not publish it.
///
/// # Panics
///
/// Never.
fn classify(name: &str, version: &str) -> Option<Role> {
    if name == SIGNING_KEY_FILE {
        return Some(Role::SigningKey);
    }
    if name == source_archive_name(version) {
        return Some(Role::SourceArchive);
    }
    for kind in PackageKind::ALL {
        if !name.ends_with(kind.suffix()) {
            continue;
        }
        let arch = Architecture::ALL
            .into_iter()
            .find(|arch| carries(name, kind.spelling(*arch)))?;
        return Some(Role::Package { kind, arch });
    }
    None
}

/// The name of the source archive a release publishes.
///
/// The name all three packaging recipes ask for: the RPM spec's `Source0`, the PKGBUILD's
/// `source` and the Debian build all name `rspinyin-<version>.tar.gz`, so the release
/// publishes it under that name rather than renaming it on the way out.
///
/// # Panics
///
/// Never.
pub fn source_archive_name(version: &str) -> String {
    format!("rspinyin-{version}.tar.gz")
}

/// Whether `name` carries `spelling` as a whole component.
///
/// An occurrence delimited by characters that are not alphanumeric, or by the ends of the
/// name. Delimited rather than a plain substring, because `aarch64` contains `64` and
/// `x86_64bit` contains `x86_64`: a substring test would let a file name that merely mentions
/// an architecture pass for one built for it.
///
/// The delimiter is any non-alphanumeric character, which is what makes the three ecosystems'
/// spellings all work: `_` and `.` in `rspinyin_0.1.0-1_amd64.deb`, `.` in
/// `rspinyin-0.1.0-1.fc42.x86_64.rpm`, `-` and `.` in `rspinyin-0.1.0-1-x86_64.pkg.tar.zst`.
/// The underscore inside `x86_64` is not a delimiter there because it is inside the spelling
/// being looked for, and one that *precedes* a spelling -- the `_` in `_amd64` -- is.
///
/// # Panics
///
/// Never.
fn carries(name: &str, spelling: &str) -> bool {
    name.match_indices(spelling).any(|(start, _)| {
        let before = name[..start].chars().next_back();
        let after = name[start + spelling.len()..].chars().next();
        !before.is_some_and(|character| character.is_ascii_alphanumeric())
            && !after.is_some_and(|character| character.is_ascii_alphanumeric())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory unique to this test process and tag.
    fn scratch(tag: &str) -> PathBuf {
        let name = format!("rspinyin-release-set-{tag}-{}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        dir
    }

    /// The release the fixtures below describe.
    const VERSION: &str = "0.1.0";

    /// The packages a complete fixture release publishes.
    fn packages() -> Vec<(PackageKind, Architecture)> {
        vec![
            (PackageKind::Deb, Architecture::X86_64),
            (PackageKind::Deb, Architecture::Aarch64),
            (PackageKind::Rpm, Architecture::X86_64),
            (PackageKind::Rpm, Architecture::Aarch64),
            (PackageKind::Pkg, Architecture::X86_64),
        ]
    }

    /// The file names a complete fixture release consists of.
    fn complete() -> Vec<String> {
        vec![
            source_archive_name(VERSION),
            "rspinyin_0.1.0-1_amd64.deb".to_owned(),
            "rspinyin_0.1.0-1_arm64.deb".to_owned(),
            "rspinyin-0.1.0-1.fc42.x86_64.rpm".to_owned(),
            "rspinyin-0.1.0-1.fc42.aarch64.rpm".to_owned(),
            "rspinyin-0.1.0-1-x86_64.pkg.tar.zst".to_owned(),
            SIGNING_KEY_FILE.to_owned(),
        ]
    }

    /// Writes `names` into a fresh scratch directory.
    fn release(tag: &str, names: &[String]) -> PathBuf {
        let dir = scratch(tag);
        for name in names {
            fs::write(dir.join(name), name.as_bytes()).expect("writing the fixture file");
        }
        dir
    }

    #[test]
    fn test_architectures_parse_the_uname_spelling_and_refuse_another() {
        assert_eq!(
            Architecture::parse("x86_64").expect("x86_64 is covered"),
            Architecture::X86_64
        );
        assert_eq!(
            Architecture::parse("aarch64").expect("aarch64 is covered"),
            Architecture::Aarch64
        );
        let failure = Architecture::parse("amd64").expect_err("the ecosystem spelling is not");
        assert!(failure.to_string().contains("x86_64"), "{failure}");
    }

    #[test]
    fn test_package_kind_parses_its_name_and_refuses_another() {
        assert_eq!(
            PackageKind::parse("deb").expect("deb is a kind"),
            PackageKind::Deb
        );
        assert_eq!(
            PackageKind::parse("rpm").expect("rpm is a kind"),
            PackageKind::Rpm
        );
        assert_eq!(
            PackageKind::parse("pkg").expect("pkg is a kind"),
            PackageKind::Pkg
        );
        let failure = PackageKind::parse("apk").expect_err("apk is not a kind");
        assert!(failure.to_string().contains("deb"), "{failure}");
    }

    #[test]
    fn test_spelling_uses_the_ecosystems_own_architecture_names() {
        // Debian names the architecture differently from everyone else, and a package
        // labelled with the wrong one installs on a machine that cannot load it.
        assert_eq!(PackageKind::Deb.spelling(Architecture::X86_64), "amd64");
        assert_eq!(PackageKind::Deb.spelling(Architecture::Aarch64), "arm64");
        assert_eq!(PackageKind::Rpm.spelling(Architecture::X86_64), "x86_64");
        assert_eq!(PackageKind::Pkg.spelling(Architecture::Aarch64), "aarch64");
    }

    #[test]
    fn test_carries_matches_a_delimited_occurrence_and_not_a_substring() {
        assert!(carries("rspinyin-0.1.0-1.fc42.x86_64.rpm", "x86_64"));
        assert!(carries("rspinyin-0.1.0-1-x86_64.pkg.tar.zst", "x86_64"));
        // The underscore that precedes a Debian architecture name is a delimiter; the one
        // inside `x86_64` is part of the name being looked for. A rule that treated `_` as
        // part of a word would read `_amd64` as one token with the version before it and
        // find no architecture at all.
        assert!(carries("rspinyin_0.1.0-1_amd64.deb", "amd64"));
        assert!(carries("rspinyin_0.1.0-1_arm64.deb", "arm64"));
        // At the ends of the name, where there is no character to delimit against.
        assert!(carries("amd64.deb", "amd64"));
        // A version that merely contains the digits must not pass for the architecture, and
        // neither must an architecture that runs into the next word.
        assert!(!carries("rspinyin-0.1.0-1.fc42.aarch64.rpm", "x86_64"));
        assert!(!carries("rspinyin-1.0.64.deb", "aarch64"));
        assert!(!carries("rspinyin-0.1.0-x86_64bit.tar.gz", "x86_64"));
    }

    #[test]
    fn test_classify_reads_every_name_a_release_publishes() {
        assert_eq!(
            classify("rspinyin-0.1.0.tar.gz", VERSION),
            Some(Role::SourceArchive)
        );
        assert_eq!(classify(SIGNING_KEY_FILE, VERSION), Some(Role::SigningKey));
        assert_eq!(
            classify("rspinyin_0.1.0-1_arm64.deb", VERSION),
            Some(Role::Package {
                kind: PackageKind::Deb,
                arch: Architecture::Aarch64,
            })
        );
        assert_eq!(
            classify("rspinyin-0.1.0-1.fc42.x86_64.rpm", VERSION),
            Some(Role::Package {
                kind: PackageKind::Rpm,
                arch: Architecture::X86_64,
            })
        );
        assert_eq!(
            classify("rspinyin-0.1.0-1-x86_64.pkg.tar.zst", VERSION),
            Some(Role::Package {
                kind: PackageKind::Pkg,
                arch: Architecture::X86_64,
            })
        );
    }

    #[test]
    fn test_classify_refuses_a_neighbour_of_a_published_name() {
        // A per-architecture release archive is what `xtask package` writes, not what a
        // release publishes: the source archive is the one that ships, and accepting the
        // other would silently make a release that carries neither.
        assert_eq!(classify("rspinyin-0.1.0-x86_64.tar.gz", VERSION), None);
        assert_eq!(classify("rspinyin-0.2.0.tar.gz", VERSION), None);
        assert_eq!(classify("librspinyin.so", VERSION), None);
        assert_eq!(classify("SHA256SUMS", VERSION), None);
        assert_eq!(classify("SHA256SUMS.asc", VERSION), None);
        // The right suffix with no architecture this release was built for.
        assert_eq!(classify("rspinyin_0.1.0-1_riscv64.deb", VERSION), None);
    }

    #[test]
    fn test_read_accepts_a_complete_release_and_orders_it_by_name() {
        let dir = release("complete", &complete());
        let set = Set::read(&dir, VERSION, &packages()).expect("the release is complete");

        assert_eq!(set.files().len(), complete().len());
        let names: Vec<&str> = set.files().iter().map(|file| file.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(
            names, sorted,
            "the manifest records the files in name order"
        );
        assert_eq!(
            set.architectures(),
            [Architecture::X86_64, Architecture::Aarch64],
            "the architectures come from the packages, not from a declaration"
        );
        assert_eq!(set.dir(), dir.as_path());
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_read_refuses_a_release_that_is_short_a_package() {
        let mut names = complete();
        names.retain(|name| !name.ends_with("arm64.deb"));
        let dir = release("short", &names);

        let failure = Set::read(&dir, VERSION, &packages()).expect_err("a package is missing");
        assert!(failure.to_string().contains("package-deb"), "{failure}");
        assert!(failure.to_string().contains("no file"), "{failure}");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_read_refuses_a_release_that_is_short_the_signing_key() {
        // The key is a published file like any other: without it the release's own
        // instructions cannot be followed, and the manifest would not record it either.
        let names: Vec<String> = complete()
            .into_iter()
            .filter(|name| name != SIGNING_KEY_FILE)
            .collect();
        let dir = release("no-key", &names);

        let failure = Set::read(&dir, VERSION, &packages()).expect_err("the key is missing");
        assert!(failure.to_string().contains("signing-key"), "{failure}");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_read_refuses_a_file_a_release_does_not_publish() {
        let mut names = complete();
        names.push("notes.txt".to_owned());
        let dir = release("stray", &names);

        let failure = Set::read(&dir, VERSION, &packages()).expect_err("nothing publishes that");
        assert!(failure.to_string().contains("notes.txt"), "{failure}");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_read_refuses_two_files_claiming_one_role() {
        // Two packages for one architecture would give the manifest two digests for one
        // role, and a user no way to tell which of them the release meant.
        let mut names = complete();
        names.push("rspinyin_0.1.0-2_amd64.deb".to_owned());
        let dir = release("twice", &names);

        let failure = Set::read(&dir, VERSION, &packages()).expect_err("one role, two files");
        assert!(failure.to_string().contains("2 files"), "{failure}");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_read_skips_a_directory_beside_the_release() {
        let dir = release("directory", &complete());
        fs::create_dir(dir.join("unpacked")).expect("creating the subdirectory");
        Set::read(&dir, VERSION, &packages()).expect("a subdirectory is not a published file");
        fs::remove_dir_all(&dir).expect("cleaning up");
    }

    #[test]
    fn test_read_reports_a_directory_that_is_not_there() {
        let dir =
            std::env::temp_dir().join(format!("rspinyin-release-absent-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let failure = Set::read(&dir, VERSION, &packages()).expect_err("there is no release");
        assert!(failure.to_string().contains("release:"), "{failure}");
    }

    #[test]
    fn test_source_archive_name_is_the_one_the_recipes_ask_for() {
        // The RPM spec's `Source0`, the PKGBUILD's `source` and the Debian build all name
        // this file; a release that published it under another name would leave all three
        // pointing at a URL that resolves to something else.
        assert_eq!(source_archive_name("1.2.3"), "rspinyin-1.2.3.tar.gz");
    }
}
