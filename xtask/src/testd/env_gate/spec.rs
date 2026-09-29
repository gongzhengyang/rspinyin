//! The register `features.md` 0.5.5 keeps, and the cross-check that binds the table to it.
//!
//! Responsibility: read the specification's own list of items this machine cannot verify, and
//! compare it with the decision table in [`super::table`]. Boundaries: this file reads the
//! specification document and compares; it never probes the machine, and it never decides
//! anything about a case.
//!
//! # Why a cross-check rather than a copy
//!
//! The table could have been written out as 0.5.5's answers. It is not, because those answers
//! are a property of one machine and the table has to be a rule over a report -- see the module
//! documentation of [`super`]. What 0.5.5 registers is therefore held *against* the table, and
//! the comparison runs in both directions: a row that cites a task 0.5.5 no longer names is
//! reported, and a row of 0.5.5 no row answers is reported too. Rewording either side breaks
//! the agreement, which is the point.
//!
//! # The rows 0.5.5 does not speak about
//!
//! 0.5.5 registers what an *environment* cannot do. It says nothing about a case that waits on
//! code, and nothing about a capability a machine can settle, so those rows carry no citation
//! and are not compared. A citation that is absent is not a citation that agrees.

use std::path::Path;

use anyhow::{Result, ensure};

use super::table::{DECISION_TABLE, DecisionRow};

/// Path of the document the register is read from, relative to the repository root.
pub const SPEC_DOCUMENT: &str = "docs/dev/features.md";

/// The section of that document which registers what this machine cannot verify.
pub const REGISTER_SECTION: &str = "0.5.5";

/// The header row of the register's table, as the document writes it.
const REGISTER_HEADER: &str = "任务 | 不可验证的验收项 | 需要的环境";

/// One row of `features.md` 0.5.5's list of items this machine cannot verify.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterEntry {
    /// The tasks the row names, as their `TASK-n.nn.nn` identifiers.
    pub tasks: Vec<String>,
    /// The acceptance item the row says cannot be judged here.
    pub item: String,
    /// The environment the row says is needed, as the document words it.
    pub needs: String,
}

impl RegisterEntry {
    /// The environment the row names, trimmed to its first clause.
    ///
    /// A cell states the environment and then qualifies it -- a parenthesised aside, or a
    /// semicolon followed by what this machine can still verify -- so the clause before the
    /// first parenthesis or separator is the environment itself, and the rest is commentary a
    /// decision table does not have to carry.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn clause(&self) -> &str {
        let end = clause_end(&self.needs);
        self.needs[..end].trim().trim_matches('*').trim()
    }
}

/// The list of items `features.md` 0.5.5 registers as unverifiable on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnverifiableRegister {
    /// One entry per row of the table, in document order.
    pub entries: Vec<RegisterEntry>,
}

impl UnverifiableRegister {
    /// Reads the register out of the specification document's text.
    ///
    /// # Errors
    ///
    /// Returns an error when the document has no section [`REGISTER_SECTION`], when that
    /// section holds no table headed as the register's table is, and when a row of it names no
    /// task. A register that read nothing would make the cross-check agree over nothing, which
    /// is the one failure mode a cross-check must not have.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn parse(text: &str) -> Result<Self> {
        let rows = section_table(text, REGISTER_SECTION, REGISTER_HEADER)?;
        ensure!(
            !rows.is_empty(),
            "the register table in section {REGISTER_SECTION} has no rows"
        );
        let mut entries = Vec::with_capacity(rows.len());
        for cells in &rows {
            entries.push(entry(cells)?);
        }
        Ok(Self { entries })
    }

    /// The entry that names `task`, when one does.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn entry_for(&self, task: &str) -> Option<&RegisterEntry> {
        self.entries
            .iter()
            .find(|entry| entry.tasks.iter().any(|named| named == task))
    }

    /// Every task the register names, in the order the document names them.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn tasks(&self) -> Vec<&str> {
        self.entries
            .iter()
            .flat_map(|entry| entry.tasks.iter().map(String::as_str))
            .collect()
    }
}

/// One way the decision table and `features.md` 0.5.5 disagree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableDrift {
    /// A row cites a task that is not shaped like a `TASK-n.nn.nn` identifier.
    Malformed {
        /// The requirement the row answers, as the case document writes it.
        requirement: String,
        /// The citation that is not a task identifier.
        task: String,
    },
    /// A row cites a task the register does not name.
    Unregistered {
        /// The requirement the row answers.
        requirement: String,
        /// The task the register does not name.
        task: String,
    },
    /// A row's `needs` no longer starts with the environment the register names.
    Reworded {
        /// The requirement the row answers.
        requirement: String,
        /// The environments the register names for the cited tasks, joined.
        register: String,
        /// The environment the row names.
        row: String,
    },
    /// The register names a task no row of the table answers.
    Uncovered {
        /// The task the register names.
        task: String,
    },
}

impl TableDrift {
    /// The drift as a report prints it, in one line.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn describe(&self) -> String {
        match self {
            Self::Malformed { requirement, task } => {
                format!("{requirement} cites `{task}`, not a TASK-n.nn.nn identifier")
            }
            Self::Unregistered { requirement, task } => {
                format!("{requirement} cites `{task}`, which {SPEC_DOCUMENT} does not register")
            }
            Self::Reworded {
                requirement,
                register,
                row,
            } => {
                format!("{requirement} needs `{row}`; {SPEC_DOCUMENT} names `{register}`")
            }
            Self::Uncovered { task } => {
                format!("{SPEC_DOCUMENT} {REGISTER_SECTION} lists `{task}`, which no row answers")
            }
        }
    }
}

/// Reads the register out of the specification document at the repository root.
///
/// # Errors
///
/// Returns an error when the document cannot be read, and the parse errors of
/// [`UnverifiableRegister::parse`].
///
/// # Panics
///
/// Never.
pub fn read_register(root: &Path) -> Result<UnverifiableRegister> {
    UnverifiableRegister::parse(&super::read_text(&root.join(SPEC_DOCUMENT))?)
}

/// Compares the decision table with what `features.md` 0.5.5 registers.
///
/// Two directions, and both matter. Every task a row cites has to be a row of the register, and
/// the environment the row names has to start with the one that register row names -- so reword
/// 0.5.5 and the table stops agreeing with it. And every row of the register has to be cited by
/// some row of the table, so a requirement 0.5.5 lists cannot quietly lose its answer.
///
/// # Panics
///
/// Never.
pub fn cross_check_spec(register: &UnverifiableRegister) -> Vec<TableDrift> {
    let mut drift = Vec::new();
    for row in &DECISION_TABLE {
        check_row(row, register, &mut drift);
    }
    for task in register.tasks() {
        let covered = DECISION_TABLE.iter().any(|row| row.cites(task));
        if !covered {
            drift.push(TableDrift::Uncovered {
                task: task.to_owned(),
            });
        }
    }
    drift
}

/// Compares one row of the decision table with the register.
///
/// A row that cites several tasks agrees when *any* of them names an environment the row's
/// `needs` starts with: `features.md` 0.5.5 splits one machine boundary across several rows
/// whose cells word the environment differently, and a row that covers two of them can only
/// carry one wording.
///
/// # Panics
///
/// Never.
fn check_row(row: &DecisionRow, register: &UnverifiableRegister, drift: &mut Vec<TableDrift>) {
    let requirement = row.requirement.label().to_owned();
    let mut clauses = Vec::new();
    for task in row.register {
        if !is_task_id(task) {
            drift.push(TableDrift::Malformed {
                requirement: requirement.clone(),
                task: (*task).to_owned(),
            });
            continue;
        }
        match register.entry_for(task) {
            Some(found) => clauses.push(found.clause()),
            None => drift.push(TableDrift::Unregistered {
                requirement: requirement.clone(),
                task: (*task).to_owned(),
            }),
        }
    }
    let needs = row.needs();
    if !row.register.is_empty() && !clauses.iter().any(|clause| needs.starts_with(*clause)) {
        drift.push(TableDrift::Reworded {
            requirement,
            register: clauses.join(" / "),
            row: needs.to_owned(),
        });
    }
}

/// Where a register cell's first clause ends.
///
/// # Panics
///
/// Never.
fn clause_end(cell: &str) -> usize {
    let separators = ['（', '(', '；', ';', '。'];
    cell.find(separators).unwrap_or(cell.len())
}

/// The entry a register table row states.
///
/// # Errors
///
/// Returns an error when the row does not hold the three cells the table declares, and when its
/// task cell names no task.
///
/// # Panics
///
/// Never.
fn entry(cells: &[String]) -> Result<RegisterEntry> {
    ensure!(
        cells.len() == 3,
        "a register row states {} cells, not the three its header declares",
        cells.len()
    );
    let tasks = tasks_in(&cells[0]);
    ensure!(!tasks.is_empty(), "a row names no task: {}", cells[0]);
    Ok(RegisterEntry {
        tasks,
        item: cells[1].clone(),
        needs: cells[2].clone(),
    })
}

/// Every task identifier a cell names, in the order it names them.
///
/// # Panics
///
/// Never.
fn tasks_in(cell: &str) -> Vec<String> {
    // The identifier starts with `TASK-`, whose own characters are not the ones an
    // identifier continues with, so the end of the run is searched for *after* the prefix.
    // Searching from the prefix would find the `T` of `TASK` itself at offset zero, push an
    // empty task and leave `rest` where it was -- a loop that never advances.
    const PREFIX: &str = "TASK-";
    let mut tasks = Vec::new();
    let mut rest = cell;
    while let Some(start) = rest.find(PREFIX) {
        let tail = &rest[start..];
        let after = &tail[PREFIX.len()..];
        let end = after
            .find(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == '-'))
            .unwrap_or(after.len());
        let length = PREFIX.len() + end;
        tasks.push(tail[..length].to_owned());
        rest = &tail[length..];
    }
    tasks
}

/// Whether a string is shaped like a `TASK-n.nn.nn` identifier.
///
/// # Panics
///
/// Never.
fn is_task_id(task: &str) -> bool {
    let Some(rest) = task.strip_prefix("TASK-") else {
        return false;
    };
    let parts: Vec<&str> = rest.split('.').collect();
    parts.len() == 3 && parts.into_iter().all(is_number)
}

/// Whether a string is one non-empty run of ASCII digits.
///
/// # Panics
///
/// Never.
fn is_number(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|ch| ch.is_ascii_digit())
}

/// The rows of the table under a section heading, each row as its cells.
///
/// The table is located by its header line rather than by position, so a table that moves inside
/// its section is still found, and a renamed column is reported as a missing table instead of
/// silently read as a different one.
///
/// # Errors
///
/// Returns an error when the document has no section with that heading, and when the section
/// holds no table with that header.
///
/// # Panics
///
/// Never.
fn section_table(text: &str, section: &str, header: &str) -> Result<Vec<Vec<String>>> {
    let mut lines = text.lines();
    let found = lines.by_ref().any(|line| {
        line.trim()
            .strip_prefix("#### ")
            .is_some_and(|rest| rest.starts_with(section))
    });
    ensure!(found, "the specification has no section {section}");
    let mut rows = Vec::new();
    let mut in_table = false;
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("#### ") {
            break;
        }
        if !trimmed.starts_with('|') {
            if in_table {
                break;
            }
            continue;
        }
        let cells = table_cells(trimmed);
        if cells.join(" | ") == header {
            in_table = true;
            continue;
        }
        if in_table && !is_separator(&cells) {
            rows.push(cells);
        }
    }
    ensure!(in_table, "section {section} has no table headed `{header}`");
    Ok(rows)
}

/// The cells of a table row, with the outer bars removed and each cell trimmed.
///
/// # Panics
///
/// Never.
fn table_cells(line: &str) -> Vec<String> {
    line.trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect()
}

/// Whether a row is the table's `|---|---|` separator rather than content.
///
/// # Panics
///
/// Never.
fn is_separator(cells: &[String]) -> bool {
    cells
        .iter()
        .all(|cell| !cell.is_empty() && cell.chars().all(|ch| ch == '-' || ch == ':' || ch == ' '))
}
