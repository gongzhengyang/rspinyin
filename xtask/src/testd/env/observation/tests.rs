//! Tests for [`super`]'s readers.
//!
//! This file is the body of the `tests` module declared in `observation.rs`; it lives beside
//! that file only because the two together exceed the project's file-length limit. The two
//! live tests at the end run wherever the suite runs -- with a display, without one, with
//! `fc-list` installed or not -- and assert only what holds in every one of those cases.

use std::os::unix::fs::PermissionsExt;

use super::*;

#[test]
fn test_count_cjk_fonts_counts_the_lines_a_listing_has() {
    assert_eq!(count_cjk_fonts(""), 0);
    assert_eq!(count_cjk_fonts("\n \n"), 0, "a blank line is not a font");
    assert_eq!(
        count_cjk_fonts("/usr/share/fonts/a.otf: A:style=Regular\n"),
        1
    );
    assert_eq!(count_cjk_fonts("a\nb\nc\n"), 3);
}

#[test]
fn test_is_argb_visual_requires_the_argb8888_channel_order() {
    assert!(is_argb_visual(
        32,
        VisualClass::TRUE_COLOR,
        0x00ff_0000,
        0x0000_ff00,
        0x0000_00ff
    ));
    assert!(
        !is_argb_visual(
            24,
            VisualClass::TRUE_COLOR,
            0x00ff_0000,
            0x0000_ff00,
            0x0000_00ff
        ),
        "a 24-bit visual has no alpha channel"
    );
    assert!(
        !is_argb_visual(
            32,
            VisualClass::TRUE_COLOR,
            0x0000_00ff,
            0x0000_ff00,
            0x00ff_0000
        ),
        "swapped channels would draw blue and red the wrong way round"
    );
    assert!(
        !is_argb_visual(
            32,
            VisualClass::DIRECT_COLOR,
            0x00ff_0000,
            0x0000_ff00,
            0x0000_00ff
        ),
        "only a TrueColor visual carries the masks the buffer format assumes"
    );
}

#[test]
fn test_socket_path_resolves_a_relative_name_against_the_runtime_directory() {
    let runtime = PathBuf::from("/run/user/1000");
    assert_eq!(
        socket_path("wayland-0", Some(&runtime)),
        Some(PathBuf::from("/run/user/1000/wayland-0"))
    );
    assert_eq!(
        socket_path("/tmp/wayland-9", Some(&runtime)),
        Some(PathBuf::from("/tmp/wayland-9")),
        "an absolute name is already a path"
    );
    assert_eq!(
        socket_path("wayland-0", None),
        None,
        "a relative name with no runtime directory is nowhere"
    );
}

#[test]
fn test_writable_dir_judges_a_missing_directory_by_its_nearest_ancestor() {
    let root = std::env::temp_dir().join(format!("rspinyin-env-writable-{}", std::process::id()));
    let data = root.join("data").join("rspinyin");
    std::fs::create_dir_all(&data).unwrap();
    assert!(writable_dir(&data));
    assert!(
        writable_dir(&data.join("logs")),
        "a directory that does not exist yet is judged by the directory that would hold it"
    );

    let file = data.join("user.redb");
    std::fs::write(&file, b"").unwrap();
    assert!(
        !writable_dir(&file.join("below-a-file")),
        "nothing can be created below a file"
    );

    let read_only = data.join("read-only");
    std::fs::create_dir_all(&read_only).unwrap();
    std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o555)).unwrap();
    assert!(!writable_dir(&read_only));
    // Restore the mode so that the tree can be removed again.
    std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o755)).unwrap();

    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn test_x11_facts_reports_nothing_for_a_display_that_cannot_be_opened() {
    // A display name without a colon cannot be parsed, so the call fails before any socket is
    // opened and no display server is needed to test it.
    assert_eq!(x11_facts(Some("rspinyin-invalid")), None);
}

#[test]
fn test_read_reports_the_process_it_runs_in() {
    let observation = Observation::read();
    assert_eq!(observation.own_pid, std::process::id());
    assert!(
        observation.processes.is_some(),
        "/proc is readable on Linux"
    );
    assert_eq!(
        observation.registry,
        RegistryFacts::default(),
        "no reader fills the registry in yet: when one does, this assertion is what fails first"
    );
    assert!(
        observation.session_log.is_none(),
        "a session log is the caller's to bring"
    );
}

#[test]
fn test_read_runs_the_two_tools_without_a_shell_and_trims_what_they_say() {
    let observation = Observation::read();
    if let Some(version) = &observation.fcitx5_version {
        assert!(
            version.starts_with(char::is_numeric),
            "a version starts with a digit: {version:?}"
        );
        assert!(
            !version.contains('\n'),
            "the version is trimmed: {version:?}"
        );
    }
    if let Some(listing) = &observation.cjk_fonts {
        assert_eq!(
            count_cjk_fonts(listing) > 0,
            !listing.trim().is_empty(),
            "an empty listing counts as no fonts"
        );
    }
}
