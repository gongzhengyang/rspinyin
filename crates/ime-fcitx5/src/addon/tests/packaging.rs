//! What ships: the packaging files Fcitx5 reads to find and run this addon.
//!
//! The addon is discovered through a `.conf` descriptor rather than by name, so a
//! descriptor that names the wrong library, the wrong category or a required frontend the
//! system does not have leaves the plugin installed but unreachable. These tests read the
//! descriptors that ship and pin what the host resolves from them.
//!
//! Split out of the lifecycle tests because the assertions are about the packaging rather
//! than about the sequence: nothing here drives a step.

use std::fs;
use std::path::Path;

/// The addon description, relative to this crate's manifest directory.
const ADDON_CONF: &str = "../../packaging/fcitx5/rspinyin.conf";

/// The input-method description, relative to this crate's manifest directory.
const INPUT_METHOD_CONF: &str = "../../packaging/fcitx5/rspinyin-im.conf";

/// Reads a packaging file, or `None` when it is missing.
fn packaging_file(relative: &str) -> Option<String> {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)).ok()
}

/// Body of the named INI section, or an empty string when the section is absent.
///
/// Section-scoped rather than a plain `contains`, because the whole point of these
/// assertions is *which* section a key lives in.
fn conf_section(conf: &str, header: &str) -> String {
    let mut body = String::new();
    let mut inside = false;
    for line in conf.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            inside = trimmed == header;
            continue;
        }
        if inside {
            body.push_str(trimmed);
            body.push('\n');
        }
    }
    body
}

#[test]
fn test_addon_conf_pins_what_fcitx5_resolves() {
    let conf = packaging_file(ADDON_CONF);
    assert!(conf.is_some(), "the addon description must ship");
    if let Some(conf) = conf {
        // The artifact is librspinyin.so: a bare `Library=rspinyin` would not resolve.
        assert!(
            conf.contains("Library=librspinyin"),
            "Library must name the artifact this crate builds"
        );
        assert!(conf.contains("Type=SharedLibrary"));
        assert!(
            conf.contains("OnDemand=False"),
            "the engine has to exist before the first key arrives"
        );
        // The engine addon is an input method and nothing else. `Category` is a
        // single-valued enum in Fcitx5, so declaring `UI` here would make the host
        // look for a `fcitx::UserInterface` this library does not provide — and
        // dispatch through a vtable slot the object does not have.
        assert!(
            conf.contains("Category=InputMethod"),
            "the engine addon must be registered under Category=InputMethod"
        );
        // The frontends must be optional, not required. fcitx5 treats every entry in
        // [Addon/Dependencies] as mandatory, so listing xcb and wayland there makes
        // the plugin refuse to load on an X11-only or Wayland-only system.
        let required = conf_section(&conf, "[Addon/Dependencies]");
        let optional = conf_section(&conf, "[Addon/OptionalDependencies]");
        assert!(
            !required.contains("xcb") && !required.contains("wayland"),
            "a frontend in [Addon/Dependencies] makes the plugin unloadable when that \
             frontend is absent; required section was: {required:?}"
        );
        assert!(
            optional.contains("xcb") && optional.contains("wayland"),
            "both frontends belong in [Addon/OptionalDependencies]"
        );
        assert!(
            required.contains("core"),
            "the core addon is the one genuine dependency"
        );
        // The packaged version has to track the crate version.
        let version = format!("Version={}", env!("CARGO_PKG_VERSION"));
        assert!(conf.contains(&version), "must declare {version}");
    }
}

#[test]
fn test_input_method_conf_points_at_the_addon() {
    let conf = packaging_file(INPUT_METHOD_CONF);
    assert!(conf.is_some(), "the input-method entry must ship");
    if let Some(conf) = conf {
        assert!(
            conf.contains("Addon=rspinyin"),
            "the entry must select this addon"
        );
        assert!(
            conf.contains("Name=Rust Pinyin"),
            "this is the name configtool lists"
        );
        assert!(conf.contains("LangCode=zh_CN"));
    }
}
