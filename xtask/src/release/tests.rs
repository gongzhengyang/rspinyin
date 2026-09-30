//! End-to-end tests for the two release verbs.
//!
//! Every test here drives the command line's own entry points rather than the pieces behind
//! them, so that what is asserted is the behaviour a workflow sees: the version a tag has to
//! name, the `kind:architecture` list the pipeline declares, and the document that comes out
//! of an assembled release directory.
//!
//! Nothing here signs anything. The signature state belongs to `xtask verify`, which the
//! release pipeline runs after signing; what these tests cover is the document that signature
//! covers and the gate in front of it.

use std::fs;
use std::path::PathBuf;

use super::artifacts::SIGNING_KEY_FILE;
use super::*;

/// A scratch directory unique to this test process and tag.
fn scratch(tag: &str) -> PathBuf {
    let name = format!("rspinyin-release-{tag}-{}", std::process::id());
    let dir = std::env::temp_dir().join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

/// The version this tree builds.
fn version() -> String {
    let root = crate::budget::repo_root().expect("xtask lives in the repository");
    facts::workspace_version(&root).expect("the workspace states a version")
}

/// The `kind:architecture` list the release workflow declares.
fn matrix() -> Vec<String> {
    [
        "deb:x86_64",
        "deb:aarch64",
        "rpm:x86_64",
        "rpm:aarch64",
        "pkg:x86_64",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Writes the files a complete release publishes into `dir`.
fn publish(dir: &Path, version: &str) {
    let names = [
        format!("rspinyin-{version}.tar.gz"),
        format!("rspinyin_{version}-1_amd64.deb"),
        format!("rspinyin_{version}-1_arm64.deb"),
        format!("rspinyin-{version}-1.fc42.x86_64.rpm"),
        format!("rspinyin-{version}-1.fc42.aarch64.rpm"),
        format!("rspinyin-{version}-1-x86_64.pkg.tar.zst"),
        SIGNING_KEY_FILE.to_owned(),
    ];
    for name in names {
        fs::write(dir.join(&name), name.as_bytes()).expect("writing the fixture file");
    }
}

#[test]
fn test_check_tag_accepts_a_tag_that_names_the_version_this_tree_builds() {
    let version = version();
    check_tag(&format!("v{version}")).expect("the tag names this tree's version");
    check_tag(&version).expect("a tag without the leading v names it too");
}

#[test]
fn test_check_tag_refuses_a_tag_that_names_another_version() {
    // The failure the gate exists for: a tag pushed before the version was bumped publishes a
    // release nothing in it was built from.
    let failure = check_tag("v0.0.1-not-this-tree").expect_err("the tag names another version");
    let text = failure.to_string();
    assert!(text.contains("0.0.1-not-this-tree"), "{text}");
    assert!(text.contains(&version()), "{text}");
}

#[test]
fn test_named_version_strips_one_leading_v() {
    assert_eq!(named_version("v0.1.0"), "0.1.0");
    assert_eq!(named_version("0.1.0"), "0.1.0");
    // Only the tag prefix is stripped: a version that happens to start with a v keeps it.
    assert_eq!(named_version("vv1"), "v1");
}

#[test]
fn test_expect_packages_parses_the_matrix_the_workflow_declares() {
    let parsed = expect_packages(&matrix()).expect("the matrix is well formed");
    assert_eq!(parsed.len(), 5);
    assert_eq!(parsed[0], (PackageKind::Deb, Architecture::X86_64));
    assert_eq!(parsed[4], (PackageKind::Pkg, Architecture::X86_64));
    assert_eq!(
        parsed
            .iter()
            .filter(|(_, arch)| *arch == Architecture::Aarch64)
            .count(),
        2
    );
}

#[test]
fn test_expect_packages_refuses_the_neighbours() {
    for entry in [
        // Not a pair at all.
        "deb".to_owned(),
        // A format this project does not ship.
        "apk:x86_64".to_owned(),
        // An architecture the delivery matrix does not cover.
        "deb:riscv64".to_owned(),
        // An architecture named the way one ecosystem spells it rather than the way the
        // matrix does, which is the mistake that would publish a package nobody built.
        "deb:amd64".to_owned(),
    ] {
        let failure = expect_packages(&[entry.clone()]).expect_err("not a declared package");
        assert!(
            failure.to_string().starts_with("release:"),
            "{entry}: {failure}"
        );
    }
}

#[test]
fn test_expect_packages_refuses_a_duplicate_and_an_empty_list() {
    let duplicated = ["deb:x86_64".to_owned(), "deb:x86_64".to_owned()];
    let failure = expect_packages(&duplicated).expect_err("one package, declared twice");
    assert!(failure.to_string().contains("twice"), "{failure}");

    // An empty list would let a release publish no distribution package at all while still
    // claiming to be a release.
    let failure = expect_packages(&[]).expect_err("nothing declared");
    assert!(failure.to_string().contains("no packages"), "{failure}");
}

#[test]
fn test_manifest_writes_the_document_an_assembled_release_is_published_with() {
    let version = version();
    let dir = scratch("manifest");
    publish(&dir, &version);

    manifest(&ManifestArgs {
        artifacts: dir.clone(),
        version: version.clone(),
        packages: matrix(),
    })
    .expect("the release is complete");

    let out = dir.join(document::MANIFEST_FILE);
    let text = fs::read_to_string(&out).expect("the document is written");
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("the document is JSON");
    assert_eq!(parsed["manifest_version"], serde_json::json!(1));
    assert_eq!(parsed["release"]["version"], serde_json::json!(version));
    assert_eq!(
        parsed["compatibility"]["architectures"],
        serde_json::json!(["x86_64", "aarch64"])
    );
    assert_eq!(parsed["signature"], serde_json::Value::Null);

    let artifacts = parsed["artifacts"].as_array().expect("an artifact list");
    assert_eq!(artifacts.len(), 7, "seven files were published");
    let roles: Vec<&str> = artifacts
        .iter()
        .map(|artifact| artifact["role"].as_str().expect("a role"))
        .collect();
    assert_eq!(
        roles,
        [
            // `-` sorts before `_`, so the archives, the packages and the key come before
            // the two Debian packages, and the manifest records them in name order.
            "package-pkg",
            "package-rpm",
            "package-rpm",
            "source-archive",
            "signing-key",
            "package-deb",
            "package-deb",
        ],
        "the manifest records every published file, in name order"
    );

    // The identifier is a ULID: 26 Crockford characters, which is what the summary a
    // maintainer reads and the identifier a user reports are the same string means.
    let request_id = parsed["request_id"].as_str().expect("a request id");
    assert_eq!(request_id.len(), 26, "{request_id}");

    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_manifest_refuses_a_version_that_is_not_the_one_this_tree_builds() {
    // The version comes from the tag, and the artifacts come from the tree; a pipeline that
    // passed one for the other would publish a document that describes neither.
    let dir = scratch("wrong-version");
    publish(&dir, "0.0.1");
    let failure = manifest(&ManifestArgs {
        artifacts: dir.clone(),
        version: "0.0.1".to_owned(),
        packages: matrix(),
    })
    .expect_err("this tree does not build 0.0.1");
    assert!(failure.to_string().contains(&version()), "{failure}");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_manifest_refuses_a_release_directory_that_is_short_a_file() {
    let version = version();
    let dir = scratch("incomplete");
    publish(&dir, &version);
    fs::remove_file(dir.join(format!("rspinyin-{version}-1.fc42.aarch64.rpm")))
        .expect("removing one artifact");

    let failure = manifest(&ManifestArgs {
        artifacts: dir.clone(),
        version,
        packages: matrix(),
    })
    .expect_err("a package is missing");
    assert!(failure.to_string().contains("package-rpm"), "{failure}");
    fs::remove_dir_all(&dir).expect("cleaning up");
}
