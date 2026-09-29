//! Tests for [`super`]'s process table and agent count.
//!
//! This file is the body of the `tests` module declared in `process.rs`; it lives beside that
//! file only because the two together exceed the project's file-length limit. Every rule about
//! ancestry and counting is driven with a table the test writes out itself, so a failure here
//! names a rule rather than the machine the suite happens to run on.

use super::*;

/// One row, as the reader would have built it.
fn row(pid: u32, ppid: u32, comm: &str) -> ProcessEntry {
    ProcessEntry {
        pid,
        ppid,
        comm: comm.to_owned(),
    }
}

#[test]
fn test_parse_status_reads_the_name_and_the_parent() {
    let text = "Name:\tnextest\nUmask:\t0022\nPPid:\t4242\nState:\tS (sleeping)\n";
    assert_eq!(
        parse_status(77, text),
        Some(ProcessEntry {
            pid: 77,
            ppid: 4242,
            comm: "nextest".to_owned()
        })
    );
}

#[test]
fn test_parse_status_rejects_a_document_without_both_fields() {
    assert_eq!(
        parse_status(1, "Name:\tinit\n"),
        None,
        "a status document names a parent"
    );
    assert_eq!(
        parse_status(1, "PPid:\t0\n"),
        None,
        "a status document names itself"
    );
    assert_eq!(
        parse_status(1, "Name:\t\nPPid:\t1\n"),
        None,
        "an empty name is not a name"
    );
    assert_eq!(parse_status(1, "Name:\tx\nPPid:\tnot-a-number\n"), None);
    assert_eq!(parse_status(1, ""), None);
}

#[test]
fn test_parse_status_accepts_a_process_with_no_parent() {
    assert_eq!(
        parse_status(9, "Name:\torphan\nPPid:\t0\n"),
        Some(ProcessEntry {
            pid: 9,
            ppid: 0,
            comm: "orphan".to_owned()
        }),
        "a reparented process has no parent, which is a reading and not a missing field"
    );
}

#[test]
fn test_ancestors_walk_the_parent_chain_up_to_the_root() {
    let table = [
        row(9, 7, "cargo"),
        row(7, 1, "nextest"),
        row(1, 0, "systemd"),
    ];
    assert_eq!(ancestors(9, &table), [9, 7, 1]);
}

#[test]
fn test_ancestors_of_a_process_that_is_not_in_the_table_is_itself() {
    assert_eq!(ancestors(9, &[]), [9]);
    assert_eq!(ancestors(9, &[row(50, 1, "cargo")]), [9]);
}

#[test]
fn test_ancestors_stop_at_a_cycle_instead_of_looping() {
    let table = [row(9, 7, "a"), row(7, 9, "b")];
    assert_eq!(ancestors(9, &table), [9, 7]);
}

#[test]
fn test_count_agents_ignores_the_probe_s_own_ancestry() {
    let table = [
        row(7, 1, "cargo-nextest"),
        row(9, 7, "cargo"),
        row(50, 1, "cargo"),
        row(51, 50, "rustc"),
        row(52, 1, "vim"),
    ];
    assert_eq!(count_agents(&table, 9), 2, "only the other build counts");
}

#[test]
fn test_count_agents_is_zero_when_only_the_probe_s_own_chain_is_building() {
    let table = [row(7, 1, "cargo-nextest"), row(9, 7, "cargo")];
    assert_eq!(count_agents(&table, 9), 0);
}

#[test]
fn test_count_agents_counts_a_second_build_of_the_same_program() {
    let table = [
        row(9, 1, "xtask"),
        row(50, 1, "cargo-nextest"),
        row(51, 50, "cargo"),
    ];
    assert_eq!(
        count_agents(&table, 9),
        2,
        "another agent's build is exactly what the count is for"
    );
}

#[test]
fn test_count_agents_ignores_a_process_that_only_looks_like_one() {
    let table = [
        row(9, 1, "xtask"),
        row(50, 1, "cargo-watch"),
        row(51, 1, "rustc-wrapper"),
    ];
    assert_eq!(count_agents(&table, 9), 0, "the names are matched whole");
}

#[test]
fn test_table_reads_this_process_out_of_proc() {
    let table = table().expect("/proc is readable on Linux");
    let own = table
        .iter()
        .find(|row| row.pid == std::process::id())
        .expect("the probe's own row is in the table it reads");
    assert_ne!(own.ppid, own.pid, "a process is not its own parent");
    assert!(!own.comm.is_empty(), "the kernel names every process");
}
