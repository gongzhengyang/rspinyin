//! Compiles the candidate window's Slint sources into Rust.
//!
//! `slint_build::compile` records the path of the generated file in
//! `SLINT_INCLUDE_GENERATED`, which is what `slint::include_modules!()` expands to. The
//! bindings are pulled in by a **private** module in `src/lib.rs`, and that privacy is the
//! point: `slint::*` must not appear in this crate's public API, so the candidate window is
//! never a Slint surface a third party could program against. That is a condition of the
//! Slint royalty-free licence rather than a style preference (ADR-0000 `OB-4`), and
//! `scripts/check-slint-leak.sh` enforces it over the built API.

fn main() {
    println!("cargo::rerun-if-changed=ui/candidate.slint");
    // Every file `candidate.slint` imports has to be listed, not just the one handed to the
    // compiler: Slint reads the imports while compiling the root, so editing an imported
    // file changes the output without changing the file cargo is watching. Missing this
    // line means an edit to the grid regenerates nothing until something else happens to
    // touch `candidate.slint` too.
    println!("cargo::rerun-if-changed=ui/candidate_grid.slint");
    println!("cargo::rerun-if-changed=ui/theme.slint");
    compile();
}

#[allow(clippy::panic)] // Build scripts have no error channel; aborting is the contract.
fn compile() {
    if let Err(error) = slint_build::compile("ui/candidate.slint") {
        panic!("ui/candidate.slint: {error}");
    }
}
