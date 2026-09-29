//! Tests for [`super`].
//!
//! Every test builds its own scratch worktree under the system temporary directory, so no test reads
//! the repository's own tree, the operator's `$HOME`, a display server or a Fcitx5 installation.
//! Nothing here is timed and nothing is sampled: the guard's whole input is a tree of paths and
//! bytes, which is what makes a verdict reproducible.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use super::{
    FROZEN_FILES, GuardError, GuardReport, Recovery, SourceCounts, TEST_BASELINE, TreeHash,
    Violation, admit, audit_heal_pass, audit_test_baseline, refusal, scan_source, totals, touched,
};

/// A scratch worktree that removes itself when the test ends, so a run leaves nothing behind in the
/// temporary directory.
#[derive(Debug)]
struct Scratch {
    /// The directory this guard owns.
    path: PathBuf,
}

impl Scratch {
    /// Creates `<temp>/rspinyin-guard-<tag>-<pid>`, empty.
    ///
    /// The process id keeps two test binaries apart and the tag keeps two tests in one binary apart,
    /// which is what makes the root safe to remove wholesale.
    fn new(tag: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("rspinyin-guard-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("creating the scratch directory");
        Self { path }
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Writes `contents` to `relative` under `root`, creating the parents.
fn write(root: &Path, relative: &str, contents: &str) -> PathBuf {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("creating a parent directory");
    }
    fs::write(&path, contents).expect("writing a file");
    path
}

/// A test module holding `count` test functions, each with one assertion.
fn tests_source(count: usize) -> String {
    (0..count)
        .map(|index| format!("#[test]\nfn test_{index}() {{ assert_eq!({index}, {index}); }}\n"))
        .collect()
}

/// The files of a tree, or a failure naming the tree that could not be read.
fn files(tree: &TreeHash) -> &std::collections::BTreeMap<PathBuf, super::FileHash> {
    tree.files().expect("the tree was read")
}

/// A worktree holding one product file, the guard itself, one test script and every frozen file.
///
/// The paths are the repository's own, relative to its root, because that is the form [`refusal`]
/// reads: the scratch directory stands in for the worktree, not for a different layout.
fn worktree(tag: &str) -> Scratch {
    let root = Scratch::new(tag);
    write(&root, "crates/ime-core/src/lib.rs", "pub fn decode() {}\n");
    write(
        &root,
        "xtask/src/testd/guard.rs",
        "// the guard, with its own allowlist\n",
    );
    write(&root, "xtask/src/testd/example/tests.rs", &tests_source(2));
    write(&root, "docs/dev/budgets.json", "{\"version\": 1}\n");
    write(&root, "docs/dev/features.md", "# the specification\n");
    write(&root, "Cargo.toml", "[workspace]\n");
    write(&root, "Cargo.lock", "# the lockfile\n");
    write(&root, "justfile", "check:\n    @true\n");
    write(&root, "clippy.toml", "too-many-arguments-threshold = 7\n");
    root
}

#[test]
fn test_refusal_places_every_region_of_the_worktree() {
    assert_eq!(
        refusal(Path::new("crates/ime-core/src/lib.rs")),
        Some("is product code and not a test script"),
        "the product's own source is the first red line"
    );
    for frozen in FROZEN_FILES {
        assert_eq!(
            refusal(Path::new(*frozen)),
            Some("is frozen for the whole run"),
            "{frozen}"
        );
    }
    assert_eq!(
        refusal(Path::new("xtask/src/testd/guard.rs")),
        Some("holds the red lines the pass is judged by"),
        "the file holding the allowlist is refused before the allowlist it sits inside"
    );
    assert_eq!(
        refusal(Path::new("xtask/src/testd/locator.rs")),
        None,
        "a test script may be rewritten"
    );
    assert_eq!(refusal(Path::new("docs/dev/tests/ui.md")), None);
    assert_eq!(
        refusal(Path::new("docs/dev/tests.md")),
        Some("is outside the heal allowlist")
    );
    assert_eq!(
        refusal(Path::new("scripts/check-unsafe.sh")),
        Some("is outside the heal allowlist")
    );
    // The comparison is by path component: a sibling that merely shares the first characters is
    // neither the product's source tree nor part of the allowlist.
    assert_eq!(
        refusal(Path::new("crates-extra/src/lib.rs")),
        Some("is outside the heal allowlist")
    );
    assert_eq!(
        refusal(Path::new("xtask/src/testd-extra/x.rs")),
        Some("is outside the heal allowlist")
    );
}

#[test]
fn test_scan_source_counts_only_the_attributes_and_macros_that_are_really_there() {
    // Every commented and quoted spelling below is a way to inflate the count while the tests it is
    // supposed to protect are deleted, which is what the scanner exists to stop.
    let source = r####"
        // #[test]
        // fn test_commented_out() { assert!(true); }
        /* #[test]
           assert_eq!(1, 1); */
        const LOOKS_LIKE_A_TEST: &str = "#[test] assert!(true)";
        const ALSO_LOOKS: &str = r##"#[test] assert_ne!(1, 2)"##;
        #[test]
        fn test_real() {
            assert!(true);
            assert_eq!(1, 1);
            assert_ne!(1, 2);
            assert_matches!(Some(1), Some(_));
            debug_assert_eq!(1, 1);
            my_assert!(true);
        }
    "####;
    let counts = scan_source(source);
    assert_eq!(
        counts.tests, 1,
        "only the attribute that is really there counts"
    );
    assert_eq!(
        counts.assertions, 4,
        "the four real macros count; the debug and the prefixed spellings do not"
    );
    assert_eq!(
        scan_source(""),
        SourceCounts::default(),
        "a file with nothing in it holds nothing"
    );
}

#[test]
fn test_touched_names_every_file_that_changed_in_path_order() {
    let root = Scratch::new("touched");
    write(&root, "crates/ime-core/src/lib.rs", "pub fn decode() {}\n");
    write(&root, "xtask/src/testd/guard.rs", "// the guard\n");
    let before = TreeHash::capture(&root);
    assert!(before.is_available());
    assert_eq!(files(&before).len(), 2);

    write(
        &root,
        "crates/ime-core/src/lib.rs",
        "pub fn decode() { /* changed */ }\n",
    );
    write(&root, "xtask/src/testd/locator.rs", "// new\n");
    fs::remove_file(root.join("xtask/src/testd/guard.rs")).expect("removing a file");
    let after = TreeHash::capture(&root);

    assert_eq!(
        touched(files(&before), files(&after)),
        vec![
            PathBuf::from("crates/ime-core/src/lib.rs"),
            PathBuf::from("xtask/src/testd/guard.rs"),
            PathBuf::from("xtask/src/testd/locator.rs"),
        ],
        "a modified file, a removed one and an added one, in path order: a pass that \
         deletes a file has touched it as surely as one that edits it"
    );
    assert_eq!(
        touched(files(&after), files(&after)),
        Vec::<PathBuf>::new(),
        "a tree does not differ from itself"
    );
    assert_eq!(
        touched(files(&after), files(&before)),
        vec![
            PathBuf::from("crates/ime-core/src/lib.rs"),
            PathBuf::from("xtask/src/testd/guard.rs"),
            PathBuf::from("xtask/src/testd/locator.rs"),
        ],
        "the list is the set of differing files, whichever way round the two trees are given"
    );
}

#[test]
fn test_capture_skips_the_directories_the_run_writes_to() {
    let root = Scratch::new("excluded");
    write(&root, "crates/ime-core/src/lib.rs", "// code\n");
    write(
        &root,
        "target/debug/build/out.rs",
        "#[test]\nfn test_in_target() {}\n",
    );
    write(
        &root,
        ".git/objects/ab/cdef",
        "#[test]\nfn test_in_git() {}\n",
    );
    write(
        &root,
        "RUN/core/TC-CORE-01/index.md",
        "#[test]\nfn test_in_run() {}\n",
    );
    write(
        &root,
        "results/runs/run-1/index.json",
        "#[test]\nfn test_in_results() {}\n",
    );
    write(
        &root,
        "crates/ime-core/target/nested.rs",
        "#[test]\nfn test_nested() {}\n",
    );

    let tree = TreeHash::capture(&root);
    let found = files(&tree);
    assert_eq!(found.len(), 1, "only the source file is part of the tree");
    assert_eq!(
        totals(found).tests,
        0,
        "an excluded file contributes no count"
    );
}

#[test]
fn test_capture_records_a_symbolic_link_without_following_it() {
    let root = Scratch::new("symlink");
    write(&root, "crates/ime-core/src/lib.rs", "// code\n");
    write(
        &root,
        "elsewhere/secret.rs",
        "#[test]\nfn test_behind() {}\n",
    );
    symlink(root.join("elsewhere"), root.join("linked")).expect("planting a link");
    symlink("nowhere", root.join("dangling")).expect("planting a dangling link");

    let tree = TreeHash::capture(&root);
    let found = files(&tree);
    assert_eq!(found.len(), 4, "the two links are entries of their own");
    assert_eq!(
        totals(found).tests,
        1,
        "a link is recorded, not walked through"
    );

    // A link is hashed by its own text, so repointing one is a change like any other.
    fs::remove_file(root.join("dangling")).expect("removing a link");
    symlink("somewhere-else", root.join("dangling")).expect("planting a link");
    let after = TreeHash::capture(&root);
    assert_eq!(
        touched(files(&tree), files(&after)),
        vec![PathBuf::from("dangling")]
    );
}

#[test]
fn test_capture_fails_closed_when_the_tree_cannot_be_read() {
    let root = Scratch::new("unreadable");
    let missing = TreeHash::capture(&root.join("does-not-exist"));
    assert!(!missing.is_available());
    assert!(
        matches!(missing.files(), Err(GuardError::TreeUnavailable { .. })),
        "a tree that was not read answers with a refusal and not with an empty map"
    );

    let file = write(&root, "a-file", "not a directory\n");
    let not_a_directory = TreeHash::capture(&file);
    assert!(!not_a_directory.is_available());
    assert!(matches!(
        not_a_directory.files(),
        Err(GuardError::TreeUnavailable { .. })
    ));
}

#[test]
fn test_audit_heal_pass_admits_a_rewritten_test_script() {
    let root = worktree("admit-clean");
    let before = TreeHash::capture(&root);
    // The same two tests and the same two assertions, re-derived rather than reworded.
    write(
        &root,
        "xtask/src/testd/example/tests.rs",
        "// the locator was re-derived\n#[test]\nfn test_first() { assert_eq!(0, 0); }\n\
         #[test]\nfn test_second() { assert_eq!(1, 1); }\n",
    );
    let after = TreeHash::capture(&root);

    let report = audit_heal_pass(&before, &after);
    assert!(
        report.is_clean(),
        "a rewrite inside the allowlist is admitted: {:?}",
        report.violations
    );
    assert_eq!(
        report.touched,
        vec![PathBuf::from("xtask/src/testd/example/tests.rs")]
    );
    assert!(report.assert_clean().is_ok());
}

#[test]
fn test_audit_heal_pass_refuses_a_write_under_the_product_source_tree() {
    let root = worktree("refuse-crates");
    let before = TreeHash::capture(&root);
    write(
        &root,
        "crates/ime-core/src/lib.rs",
        "pub fn decode() { /* the bar was lowered */ }\n",
    );
    let after = TreeHash::capture(&root);

    let report = audit_heal_pass(&before, &after);
    assert!(!report.is_clean(), "product code is a red line");
    assert_eq!(
        report.touched,
        vec![PathBuf::from("crates/ime-core/src/lib.rs")]
    );
    assert_eq!(
        report.violations,
        vec![Violation::OutOfScope {
            path: PathBuf::from("crates/ime-core/src/lib.rs"),
            reason: "is product code and not a test script",
        }]
    );
    assert!(
        matches!(report.assert_clean(), Err(GuardError::RedLine { .. })),
        "the run stops here rather than publishing the result"
    );

    // Taking the file away is the same red line: a pass may not remove product code either.
    fs::remove_file(root.join("crates/ime-core/src/lib.rs")).expect("removing the product file");
    let removed = TreeHash::capture(&root);
    assert_eq!(
        audit_heal_pass(&after, &removed).violations,
        vec![Violation::OutOfScope {
            path: PathBuf::from("crates/ime-core/src/lib.rs"),
            reason: "is product code and not a test script",
        }]
    );
}

#[test]
fn test_audit_heal_pass_refuses_a_frozen_file_and_the_guard_itself() {
    let root = worktree("refuse-frozen");
    let before = TreeHash::capture(&root);
    write(
        &root,
        "docs/dev/budgets.json",
        "{\"version\": 1, \"latency_ms\": {\"decode_p99\": 300.0}}\n",
    );
    write(
        &root,
        "docs/dev/features.md",
        "# the specification, loosened\n",
    );
    write(&root, "Cargo.toml", "[workspace]\nresolver = \"3\"\n");
    write(&root, "Cargo.lock", "# the lockfile, edited\n");
    write(&root, "justfile", "check:\n    @true # loosened\n");
    write(&root, "clippy.toml", "too-many-arguments-threshold = 700\n");
    write(
        &root,
        "xtask/src/testd/guard.rs",
        "// the guard, with its own allowlist widened\n",
    );
    let after = TreeHash::capture(&root);

    let report = audit_heal_pass(&before, &after);
    assert_eq!(report.touched.len(), 7);
    let mut reasons: Vec<&str> = report
        .violations
        .iter()
        .map(|violation| match violation {
            Violation::OutOfScope { reason, .. } => *reason,
            other => panic!("the pass crossed a red line this test does not expect: {other:?}"),
        })
        .collect();
    reasons.sort_unstable();
    let mut expected = vec!["holds the red lines the pass is judged by"];
    expected.extend(std::iter::repeat_n(
        "is frozen for the whole run",
        FROZEN_FILES.len(),
    ));
    expected.sort_unstable();
    assert_eq!(
        reasons, expected,
        "every frozen file and the guard itself are refused: {:?}",
        report.violations
    );
    assert!(!report.is_clean());
}

#[test]
fn test_audit_heal_pass_refuses_a_dropped_test_count() {
    let root = worktree("refuse-tests");
    let before = TreeHash::capture(&root);
    assert_eq!(totals(files(&before)).tests, 2);
    write(&root, "xtask/src/testd/example/tests.rs", &tests_source(1));
    let after = TreeHash::capture(&root);

    let report = audit_heal_pass(&before, &after);
    let removed = report
        .violations
        .iter()
        .find_map(|violation| match violation {
            Violation::TestsRemoved {
                before,
                after,
                paths,
            } => Some((*before, *after, paths.clone())),
            _ => None,
        });
    assert_eq!(
        removed,
        Some((
            2,
            1,
            vec![PathBuf::from("xtask/src/testd/example/tests.rs")]
        )),
        "the count is compared and the file that lost the test is named"
    );
    assert!(!report.is_clean());
}

#[test]
fn test_audit_heal_pass_refuses_a_weakened_assertion() {
    let root = worktree("refuse-assertions");
    let before = TreeHash::capture(&root);
    // The test functions stay exactly where they were; what they assert is replaced by a statement
    // that cannot fail.
    write(
        &root,
        "xtask/src/testd/example/tests.rs",
        "#[test]\nfn test_0() { let _ = 0; }\n#[test]\nfn test_1() { let _ = 1; }\n",
    );
    let after = TreeHash::capture(&root);
    assert_eq!(
        totals(files(&after)).tests,
        2,
        "the tests are still there, so only the assertion count can report this"
    );

    let report = audit_heal_pass(&before, &after);
    let weakened = report
        .violations
        .iter()
        .find_map(|violation| match violation {
            Violation::AssertionsWeakened {
                before,
                after,
                paths,
            } => Some((*before, *after, paths.clone())),
            _ => None,
        });
    assert_eq!(
        weakened,
        Some((
            2,
            0,
            vec![PathBuf::from("xtask/src/testd/example/tests.rs")]
        ))
    );
    assert!(!report.is_clean());
}

#[test]
fn test_audit_heal_pass_refuses_a_pass_it_cannot_audit() {
    let root = worktree("fail-closed");
    let readable = TreeHash::capture(&root);
    let missing = TreeHash::capture(&root.join("does-not-exist"));

    for (side, before, after) in [
        ("before", &missing, &readable),
        ("after", &readable, &missing),
    ] {
        let report = audit_heal_pass(before, after);
        assert!(
            !report.is_clean(),
            "an unauditable pass is refused rather than waved through ({side})"
        );
        assert!(
            report.touched.is_empty(),
            "nothing may be concluded about a tree that was not read ({side})"
        );
        assert!(matches!(
            report.violations.as_slice(),
            [Violation::Unauditable { .. }]
        ));
        assert!(report.assert_clean().is_err());
    }
}

#[test]
fn test_audit_test_baseline_refuses_a_tree_below_the_frozen_count() {
    let root = Scratch::new("baseline");
    write(
        &root,
        "xtask/src/testd/example/tests.rs",
        &tests_source(TEST_BASELINE),
    );
    let at_baseline = TreeHash::capture(&root);
    assert_eq!(totals(files(&at_baseline)).tests, TEST_BASELINE);
    assert_eq!(
        audit_test_baseline(&at_baseline),
        Vec::new(),
        "a tree that meets the floor reports nothing"
    );

    write(
        &root,
        "xtask/src/testd/example/tests.rs",
        &tests_source(TEST_BASELINE - 1),
    );
    let below = TreeHash::capture(&root);
    assert_eq!(
        audit_test_baseline(&below),
        vec![Violation::TestBaselineLost {
            baseline: TEST_BASELINE,
            actual: TEST_BASELINE - 1,
        }]
    );

    let missing = TreeHash::capture(&root.join("does-not-exist"));
    assert!(
        matches!(
            audit_test_baseline(&missing).as_slice(),
            [Violation::Unauditable { .. }]
        ),
        "a count that could not be taken is not a count that held"
    );
}

#[test]
fn test_admit_allows_only_a_re_derivation_inside_the_allowlist() {
    let allowed = Recovery::ReDeriveLocator {
        target: PathBuf::from("xtask/src/testd/locator.rs"),
    };
    assert_eq!(
        admit(&allowed),
        Ok(()),
        "re-deriving a locator from evidence the run already produced is the one admitted recovery"
    );
    assert_eq!(admit(&Recovery::TakeFocus), Err(Violation::FocusTaken));
    assert_eq!(
        admit(&Recovery::DestructiveRetry),
        Err(Violation::DestructiveRetry)
    );
    assert_eq!(admit(&Recovery::HideFailure), Err(Violation::FailureHidden));

    let aimed = |target: &str| {
        admit(&Recovery::ReDeriveLocator {
            target: PathBuf::from(target),
        })
    };
    assert_eq!(
        aimed("crates/ime-core/src/lib.rs"),
        Err(Violation::OutOfScope {
            path: PathBuf::from("crates/ime-core/src/lib.rs"),
            reason: "is product code and not a test script",
        })
    );
    assert_eq!(
        aimed("docs/dev/budgets.json"),
        Err(Violation::OutOfScope {
            path: PathBuf::from("docs/dev/budgets.json"),
            reason: "is frozen for the whole run",
        })
    );
    assert_eq!(
        aimed("xtask/src/testd/guard.rs"),
        Err(Violation::OutOfScope {
            path: PathBuf::from("xtask/src/testd/guard.rs"),
            reason: "holds the red lines the pass is judged by",
        })
    );
    assert_eq!(
        aimed("docs/dev/tests.md"),
        Err(Violation::OutOfScope {
            path: PathBuf::from("docs/dev/tests.md"),
            reason: "is outside the heal allowlist",
        })
    );
}

#[test]
fn test_guard_report_is_clean_only_when_no_red_line_was_crossed() {
    let clean = GuardReport {
        touched: Vec::new(),
        violations: Vec::new(),
    };
    assert!(clean.is_clean());
    assert!(clean.assert_clean().is_ok());

    let crossed = GuardReport {
        touched: vec![PathBuf::from("crates/ime-core/src/lib.rs")],
        violations: vec![Violation::OutOfScope {
            path: PathBuf::from("crates/ime-core/src/lib.rs"),
            reason: "is product code and not a test script",
        }],
    };
    assert!(!crossed.is_clean());
    let error = crossed
        .assert_clean()
        .expect_err("a crossed red line must stop the run");
    let message = error.to_string();
    assert!(message.contains("1 red line"), "{message}");
    assert!(message.contains("crates/ime-core/src/lib.rs"), "{message}");
    assert!(
        matches!(error, GuardError::RedLine { violations } if violations == crossed.violations),
        "the refusal carries every violation"
    );
}
