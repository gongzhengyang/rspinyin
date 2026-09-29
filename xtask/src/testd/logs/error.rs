//! Everything the log channel can refuse to do.
//!
//! Responsibility: name each refusal, carry where it happened, and render a message a developer
//! can act on. Splitting this out of [`super`] keeps the channel's own file about the channel:
//! nothing here touches the filesystem and nothing here reads a log line.
//!
//! # The one rule this module keeps
//!
//! No message it renders carries the text of a log line. A line can hold what the user typed --
//! that is precisely what the privacy assertion exists to catch -- and a message is printed into
//! a terminal and archived as evidence, so echoing the line would undo the redaction the channel
//! is there to verify. A [`Finding`] names the path, the line within it where the rule was about
//! a line, and the rule that was broken, and stops there; the text itself stays in the log,
//! where the redaction policy put it.

use std::path::PathBuf;

use ime_types::ImeError;

/// How many findings an assertion reports before it stops listing them.
///
/// A leak repeats on every line the writer emits, so the tenth finding says nothing the first
/// one did not, and a case that failed would otherwise print its whole log back at the operator.
/// The number of violations left out is reported alongside, so the cap never hides that there
/// were more.
pub const MAX_FINDINGS: usize = 8;

/// Which assertion failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assertion {
    /// The privacy rules: no withheld field in clear text, no clear application identifier, no
    /// unshortened home directory, and none of the literals the case named.
    Privacy,
    /// The frozen error-code set: every `code=` field names a code the contract declares.
    ErrorCodes,
    /// The modes the contract fixes for the log: the directory readable by its owner alone,
    /// and every file inside it likewise.
    Permissions,
}

impl Assertion {
    /// The label a report prints.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::Privacy => "privacy",
            Self::ErrorCodes => "error-code",
            Self::Permissions => "log-permission",
        }
    }
}

/// What one assertion objected to, on one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Denial {
    /// A field the redaction policy must replace with `<redacted:len=N>` was written as it was.
    Unredacted {
        /// The field's name, as the denylist states it.
        field: &'static str,
    },
    /// An application identifier was written in clear text rather than as a hash.
    ClearAppIdentifier,
    /// A home directory was written in full rather than shortened to `~`.
    UnshortenedHome,
    /// A literal the case asked to be absent.
    Literal {
        /// The needle the case passed in, which is the case's own string and not the log's.
        pattern: String,
    },
    /// A `code=` field names a code the frozen set does not declare.
    UnknownCode {
        /// The code the line carried.
        code: String,
    },
    /// A path of the log family carries a mode an account other than its owner can read.
    WorldAccessible {
        /// The mode the path carries, in its low nine bits.
        mode: u32,
        /// The mode the contract fixes for a path of this kind.
        want: u32,
    },
}

impl Denial {
    /// What the denial says about the line, without the line.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn describe(&self) -> String {
        match self {
            Self::Unredacted { field } => {
                format!("`{field}` was written in clear text instead of as a placeholder")
            }
            Self::ClearAppIdentifier => {
                String::from("an application identifier was written in clear text, not as a hash")
            }
            Self::UnshortenedHome => {
                String::from("a home directory was written in full instead of as `~`")
            }
            Self::Literal { pattern } => {
                format!("the line contains the literal `{pattern}` the case asked to be absent")
            }
            Self::UnknownCode { code } => {
                format!("`{code}` is not a code the frozen error model declares")
            }
            Self::WorldAccessible { mode, want } => {
                format!("it carries mode {mode:04o}, not {want:04o}, readable by others")
            }
        }
    }
}

/// One place an assertion objected to.
///
/// It carries where the place is, never what is written there. A place is a line of a file or
/// the file itself: a rule about what a line says is broken by a line, while a rule about a
/// mode is broken by the path as a whole, and the two are told apart by [`Finding::line`]
/// rather than by a line number that stands for "no line".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The file the line is in, or the path the finding is about.
    pub path: PathBuf,
    /// The line's number within that file, counting from one -- matching what a text editor
    /// shows and what `grep -n` prints -- or `None` when the finding is about the path itself
    /// rather than about one of its lines.
    pub line: Option<usize>,
    /// What was wrong with it.
    pub denial: Denial,
}

/// Everything the log channel can refuse to do.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    /// A file of the log family could not be read.
    #[error("{path}: {source}")]
    Io {
        /// The path the operation was about.
        path: PathBuf,
        /// The failure the operation reported.
        #[source]
        source: std::io::Error,
    },

    /// A complete line of the log is not valid UTF-8.
    ///
    /// The writer renders every value through a character-oriented writer, so this means the
    /// file was not written by it -- or that something else has been appending to it.
    #[error("{path}: the line starting at byte {offset} is not valid UTF-8")]
    NotText {
        /// The file that holds the line.
        path: PathBuf,
        /// The byte the line starts at.
        offset: u64,
    },

    /// The plugin's own layout could not be derived from the sandbox's base directories.
    #[error("the plugin's layout cannot be derived for this sandbox: {0}")]
    Layout(#[from] ImeError),

    /// An assertion over the log failed.
    #[error("{}", describe(*assertion, findings, *suppressed))]
    Failed {
        /// Which assertion failed.
        assertion: Assertion,
        /// The places that broke the rule, in file order, capped at [`MAX_FINDINGS`].
        findings: Vec<Finding>,
        /// How many further violations were found and left out of `findings`.
        suppressed: usize,
    },

    /// A document that declares error codes could not be read or declares none.
    ///
    /// A whitelist that came out empty would make every code look unknown, and a whitelist that
    /// silently lost a document would make a whole domain look unknown -- so the extraction
    /// fails here rather than returning a set that is smaller than it claims to be.
    #[error("{document} declares no usable error codes: {detail}")]
    CodeSource {
        /// The document that was read.
        document: &'static str,
        /// What was wrong with it.
        detail: String,
    },
}

/// Renders a failed assertion as one line, naming every finding it kept.
///
/// # Panics
///
/// Never.
fn describe(assertion: Assertion, findings: &[Finding], suppressed: usize) -> String {
    let label = assertion.label();
    let mut text = format!(
        "the log breaks the {label} rule: {} finding(s)",
        findings.len() + suppressed
    );
    if suppressed > 0 {
        text.push_str(&format!(" ({suppressed} not listed)"));
    }
    text.push(':');
    for finding in findings {
        text.push_str(&format!(
            " {}: {};",
            location(finding),
            finding.denial.describe()
        ));
    }
    text
}

/// Where a finding is: `path:line` for a rule a line broke, the path alone for a rule the path
/// itself broke.
///
/// # Panics
///
/// Never.
fn location(finding: &Finding) -> String {
    let path = finding.path.display();
    match finding.line {
        Some(line) => format!("{path}:{line}"),
        None => path.to_string(),
    }
}
