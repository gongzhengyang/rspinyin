//! The process table: who is running, and how many of them are building or testing.
//!
//! Responsibility: read `/proc` into one row per process, and answer the question the report
//! asks of it -- how many build or test processes are running outside this probe's own
//! ancestry. It also names the compositor a session runs, because a process name is the only
//! answer Wayland has to that question.
//!
//! Boundaries: it reads `/proc` and nothing else. It never signals, waits for or inspects
//! another process, and it holds no state between calls. The rows are plain data, so the
//! counting rules below are testable without a process table at all.
//!
//! # Why the probe's own ancestry is excluded
//!
//! A probe that counts `cargo` and `nextest` processes is, when it runs under `nextest`, part
//! of the set it is counting: the harness's own parent chain is a build and test process by
//! definition, so a raw count would make every machine look busy. What the purity guard needs
//! is whether *something else* is building, so the count is of processes outside the probe's
//! own ancestry and zero means this machine is otherwise quiet.

use std::fs;
use std::path::Path;

/// The programs whose presence means a build or a test run is in progress.
///
/// `cargo` is the driver, `cargo-nextest` and `nextest` the two names the test runner runs
/// under, and `rustc` what a build spends its time in. A criterion run under `cargo bench`
/// is covered by the first and the last of them.
pub const AGENT_PROGRAMS: [&str; 4] = ["cargo", "cargo-nextest", "nextest", "rustc"];

/// The directory the kernel exposes the process table under.
const PROC: &str = "/proc";

/// The field in `/proc/<pid>/status` that carries the process name.
const NAME_FIELD: &str = "Name:";

/// The field in `/proc/<pid>/status` that carries the parent process id.
const PARENT_FIELD: &str = "PPid:";

/// One row of the process table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessEntry {
    /// The process id.
    pub pid: u32,
    /// The parent process id.
    pub ppid: u32,
    /// The process name, as the kernel truncates it to fifteen characters.
    pub comm: String,
}

/// Reads the process table.
///
/// `None` when `/proc` cannot be listed at all. A single unreadable process is skipped rather
/// than failing the read: a process that exited while the table was being walked is normal,
/// and its row is simply not there.
///
/// # Panics
///
/// Never.
pub fn table() -> Option<Vec<ProcessEntry>> {
    let entries = fs::read_dir(PROC).ok()?;
    let mut rows = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = match name.to_str().and_then(|name| name.parse::<u32>().ok()) {
            Some(pid) => pid,
            None => continue,
        };
        if let Some(row) = read(pid) {
            rows.push(row);
        }
    }
    Some(rows)
}

/// Parses the two fields a row needs out of a `/proc/<pid>/status` document.
///
/// `Name` and `PPid` are the only fields read, and a document missing either is not a process
/// status document: the row is dropped rather than guessed at.
///
/// # Panics
///
/// Never.
pub fn parse_status(pid: u32, text: &str) -> Option<ProcessEntry> {
    let mut comm = None;
    let mut ppid = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix(NAME_FIELD) {
            comm = Some(value.trim()).filter(|name| !name.is_empty());
        } else if let Some(value) = line.strip_prefix(PARENT_FIELD) {
            ppid = value.trim().parse::<u32>().ok();
        }
    }
    Some(ProcessEntry {
        pid,
        ppid: ppid?,
        comm: comm?.to_owned(),
    })
}

/// The process ids from `own_pid` up to the root of the tree, `own_pid` first.
///
/// The walk is bounded by the table's length, so a table that names a cycle -- which `/proc`
/// cannot produce but a test can -- ends the walk instead of looping. A pid that is not in
/// the table contributes itself and stops: the probe's own row is always there, and its
/// ancestors are what the count needs.
///
/// # Panics
///
/// Never.
pub fn ancestors(own_pid: u32, table: &[ProcessEntry]) -> Vec<u32> {
    let mut chain = vec![own_pid];
    let mut current = own_pid;
    for _ in 0..table.len() {
        let row = match table.iter().find(|row| row.pid == current) {
            Some(row) => row,
            None => break,
        };
        if row.ppid == 0 || chain.contains(&row.ppid) {
            break;
        }
        chain.push(row.ppid);
        current = row.ppid;
    }
    chain
}

/// The number of build or test processes outside `own_pid`'s own ancestry.
///
/// Zero therefore means nothing else on this machine is building, which is what a performance
/// measurement needs to be believed. The probe's own chain is not a claim about the machine:
/// it is the harness the probe is running under.
///
/// # Panics
///
/// Never.
pub fn count_agents(table: &[ProcessEntry], own_pid: u32) -> u32 {
    let own = ancestors(own_pid, table);
    let count = table
        .iter()
        .filter(|row| AGENT_PROGRAMS.contains(&row.comm.as_str()))
        .filter(|row| !own.contains(&row.pid))
        .count();
    count as u32
}

/// Reads one process's row out of `/proc/<pid>/status`.
fn read(pid: u32) -> Option<ProcessEntry> {
    let path = Path::new(PROC).join(pid.to_string()).join("status");
    let text = fs::read_to_string(path).ok()?;
    parse_status(pid, &text)
}

// The tests live in a sibling file rather than inside this one: together they would be longer
// than the file limit allows. A `#[path]` child module keeps them inside `process`.
#[cfg(test)]
#[path = "process/tests.rs"]
mod tests;
