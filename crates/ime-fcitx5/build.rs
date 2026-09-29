//! Build script for the Fcitx5 host integration layer.
//!
//! Probes the Fcitx5 development environment at build time and fails with a readable
//! message when it is absent, rather than letting the link step emit an opaque
//! undefined-symbol error.
//!
//! The `fcitx5-host` feature gates the probe so that a plain `cargo check --workspace`
//! works on machines without the Fcitx5 development packages — that is what keeps the
//! pure-Rust CI job dependency-free.

fn main() {
    // The probe only runs when the host ABI is actually being linked. Without the
    // feature we must not touch pkg-config at all, or every developer without the
    // Fcitx5 development packages would be unable to build the workspace.
    if std::env::var_os("CARGO_FEATURE_FCITX5_HOST").is_none() {
        println!("cargo::rerun-if-changed=build.rs");
        return;
    }

    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=src/ffi/cpp");

    // Libraries the C++ glue layer links against. Fcitx5Qt is deliberately excluded:
    // this plugin has no Qt dependency.
    const REQUIRED: [&str; 3] = ["Fcitx5Core", "Fcitx5Utils", "Fcitx5Config"];

    for lib in REQUIRED {
        if let Err(err) = pkg_config::probe_library(lib) {
            abort_missing_dev_package(lib, &err);
        }
    }

    // Lets Rust code distinguish "linked against a real Fcitx5" from "mock host only".
    println!("cargo::rustc-check-cfg=cfg(fcitx5_host)");
    println!("cargo::rustc-cfg=fcitx5_host");
}

/// Aborts the build with an actionable message when an Fcitx5 development package is
/// missing.
///
/// A build script has no error channel other than failing, so this aborts rather than
/// returning. The `platform/fcitx5/dev-missing` prefix is part of the diagnostic
/// contract and is asserted by the acceptance tests, so it must not be reworded.
#[allow(clippy::panic)] // Build scripts have no error channel; aborting is the contract.
fn abort_missing_dev_package(lib: &str, err: &pkg_config::Error) -> ! {
    panic!(
        "platform/fcitx5/dev-missing: could not find the Fcitx5 development package \
         `{lib}` via pkg-config ({err}).\n\
         Install it with one of:\n  \
         Debian/Ubuntu: sudo apt install libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev\n  \
         Fedora:        sudo dnf install fcitx5-devel\n  \
         Arch:          sudo pacman -S fcitx5\n\
         Alternatively, build without the `fcitx5-host` feature to compile only the \
         pure-Rust side."
    )
}
