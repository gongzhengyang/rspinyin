//! Tests for [`super`]'s addon registry, descriptor reading and load verdicts.
//!
//! This file is the body of the `tests` module declared in `addons.rs`; it lives beside that
//! file only because the two together exceed the project's file-length limit. The descriptors
//! and the session logs are written out by the tests, so the assertions are about this
//! product's two addons rather than about whatever is installed on the machine.

use super::*;

/// A descriptor as the reader would have read it.
fn descriptor(name: &str, text: &str) -> AddonDescriptor {
    AddonDescriptor {
        name: name.to_owned(),
        text: text.to_owned(),
    }
}

#[test]
fn test_category_of_reads_the_addon_section_and_not_another_one() {
    let conf = "[Addon]\nName=Rust Pinyin\nCategory=InputMethod\n\n[InputMethod]\nCategory=UI\n";
    assert_eq!(category_of(conf).as_deref(), Some("InputMethod"));
    assert_eq!(
        category_of("[InputMethod]\nCategory=UI\n"),
        None,
        "the key must be inside [Addon]"
    );
    assert_eq!(
        category_of("[Addon]\nCategory=\n"),
        None,
        "an empty value is not a category"
    );
    assert_eq!(category_of(""), None);
}

#[test]
fn test_category_of_stops_at_the_first_value_in_the_section() {
    let conf = "[Addon]\nCategory=UI\nCategory=InputMethod\n";
    assert_eq!(category_of(conf).as_deref(), Some("UI"));
}

#[test]
fn test_states_take_the_category_from_the_descriptor_and_the_verdict_from_the_log() {
    let descriptors = [
        descriptor("rspinyin", "[Addon]\nCategory=InputMethod\n"),
        descriptor("rspinyin-ui", "[Addon]\nCategory=UI\n"),
    ];
    let states = states(&descriptors, Some("Loaded addon rspinyin\n"));
    assert_eq!(
        states,
        [
            AddonState {
                name: "rspinyin".to_owned(),
                loaded: true,
                category: "InputMethod".to_owned(),
            },
            AddonState {
                name: "rspinyin-ui".to_owned(),
                loaded: false,
                category: "UI".to_owned(),
            },
        ],
        "the engine loaded and the user interface did not"
    );
}

#[test]
fn test_states_without_a_log_report_no_addon_as_loaded() {
    let states = states(&[], None);
    assert_eq!(states.len(), EXPECTED_ADDONS.len());
    assert!(states.iter().all(|state| !state.loaded));
    assert_eq!(
        states[0].category, "InputMethod",
        "the registered category stands in for a descriptor that could not be read"
    );
    assert_eq!(states[1].category, "UI");
}

#[test]
fn test_states_mark_an_addon_a_session_reported_as_failed() {
    let log = "Could not load addon rspinyin\nLoaded addon rspinyin-ui\n";
    let states = states(&[], Some(log));
    assert!(!states[0].loaded);
    assert!(
        states[1].loaded,
        "one refused addon does not hide the other"
    );
}

#[test]
fn test_states_report_both_addons_as_unloaded_when_the_log_names_neither() {
    // A session that never reached the addon loader says nothing about either addon, and
    // silence must not read as success.
    let states = states(&[], Some("Fcitx5 is starting\n"));
    assert!(states.iter().all(|state| !state.loaded));
}

#[test]
fn test_descriptor_dirs_search_the_system_before_the_user() {
    let bases = BaseDirs {
        config_home: PathBuf::from("/home/u/.config"),
        data_home: PathBuf::from("/home/u/.local/share"),
    };
    let dirs = descriptor_dirs(Some(&bases));
    assert_eq!(
        dirs,
        [
            PathBuf::from("/usr/local/share/fcitx5/addon"),
            PathBuf::from("/usr/share/fcitx5/addon"),
            PathBuf::from("/home/u/.local/share/fcitx5/addon"),
        ],
        "the user's own directory is searched last, as Fcitx5's own path lookup does"
    );
    assert_eq!(
        descriptor_dirs(None).len(),
        2,
        "without XDG bases only the system ones are read"
    );
}

#[test]
fn test_descriptors_in_reads_only_the_addons_this_product_ships() {
    let dir = std::env::temp_dir().join(format!("rspinyin-env-addons-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("rspinyin.conf"), "[Addon]\nCategory=InputMethod\n").unwrap();
    std::fs::write(dir.join("rspinyin-ui.conf"), "[Addon]\nCategory=UI\n").unwrap();
    // Another addon's descriptor, and the input-method entry that is not an addon at all.
    std::fs::write(dir.join("classicui.conf"), "[Addon]\nCategory=UI\n").unwrap();
    std::fs::write(
        dir.join("rspinyin-im.conf"),
        "[InputMethod]\nName=Rust Pinyin\n",
    )
    .unwrap();

    let found = descriptors_in(std::slice::from_ref(&dir));
    let names: Vec<&str> = found.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, ["rspinyin", "rspinyin-ui"]);
    assert!(found[0].text.contains("InputMethod"));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn test_descriptors_in_takes_the_first_directory_that_holds_a_descriptor() {
    let root =
        std::env::temp_dir().join(format!("rspinyin-env-addons-path-{}", std::process::id()));
    let first = root.join("first");
    let second = root.join("second");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    std::fs::write(first.join("rspinyin.conf"), "[Addon]\nCategory=First\n").unwrap();
    std::fs::write(second.join("rspinyin.conf"), "[Addon]\nCategory=Second\n").unwrap();

    let found = descriptors_in(&[first, second]);
    assert_eq!(
        found.len(),
        1,
        "a search path stops at the first descriptor it finds"
    );
    assert!(found[0].text.contains("First"));

    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn test_descriptors_read_this_machine_without_inventing_an_addon() {
    // Whatever is installed here, the reader reports this product's addons or nothing: it
    // must not pick up another package's descriptor.
    let found = descriptors(BaseDirs::from_env().ok().as_ref());
    for entry in &found {
        assert!(
            EXPECTED_ADDONS.iter().any(|spec| spec.name == entry.name),
            "{} is not an addon this product ships",
            entry.name
        );
    }
}
