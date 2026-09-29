//! The constants a `.slint` metrics block declares.
//!
//! Responsibility: read the numbers that fix the candidate window's geometry out of the block
//! that declares them, and report the source as unavailable when there is nothing to read.
//! Nothing here compares them with anything; that is [`super::cross_check_spec`]'s job.
//!
//! # Two declaration forms, because two documents describe one file
//!
//! The specification names the constants twice and the two names disagree:
//!
//! * `features.md` 3.1.4 says its 「常量」 column holds "`ui/candidate.slint` 的
//!   `CandidateMetrics` 属性名", and those names are kebab-case -- `stroke-width`, `grid-gap`,
//!   `header-height`.
//! * the window's own sketch writes the same values as `public constant ShadowMargin: 32px;`
//!   inside the component, in PascalCase.
//!
//! `scripts/check-ui-spec.sh` reads the first form. This module reads both, and indexes a
//! `public constant` under its own name *and* under the kebab-case spelling the specification
//! uses, so that whichever form lands, the comparison finds it. A name that neither form
//! declares is reported as missing rather than passed over -- the harness is not the party that
//! should decide which of two authoritative documents wins.
//!
//! # What is not read
//!
//! A `property` outside an `export global` block belongs to a component instance rather than to
//! the metrics the window is built from, so only the declarations inside a global are read. A
//! `public constant` is read wherever it stands, because that is where the window declares it.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use super::MetricSource;
use super::error::MetricError;
use super::value::MetricValue;

/// The block a `.slint` source opens with this keyword.
const GLOBAL_PREFIX: &str = "export global ";

/// The keyword a free-standing constant is declared with.
const CONSTANT_PREFIX: &str = "public constant ";

/// The property directions a metrics block declares.
const DIRECTIONS: [&str; 3] = ["in-out property ", "in property ", "out property "];

/// The constants a `.slint` source declares.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlintConstants {
    /// One entry per declaration, keyed by name.
    constants: BTreeMap<String, MetricValue>,
}

impl SlintConstants {
    /// Reads the constants out of the text of a `.slint` source.
    ///
    /// # Parameters
    ///
    /// * `name` -- the document's name, as a report prints it.
    /// * `text` -- its contents.
    ///
    /// # Errors
    ///
    /// Returns [`MetricError::SlintUnparsable`] for a line that is a property declaration but
    /// not one this reader can take apart. A source whose declarations cannot be read is a
    /// failure rather than an empty table, because an empty table would make every comparison
    /// against it pass.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn parse(name: &str, text: &str) -> Result<Self, MetricError> {
        let mut constants = BTreeMap::new();
        let mut inside_global = false;
        for (index, raw) in text.lines().enumerate() {
            let line = raw.split("//").next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with(GLOBAL_PREFIX) {
                inside_global = true;
                continue;
            }
            if line == "}" {
                inside_global = false;
                continue;
            }
            if let Some((constant, expression)) = constant_declaration(line) {
                let value = value_of(expression);
                // Indexed under both spellings: the specification names the constant in
                // kebab-case, and the declaration writes it in PascalCase.
                constants.insert(kebab(constant), value.clone());
                constants.insert(constant.to_owned(), value);
                continue;
            }
            if !inside_global || !line.contains("property") {
                continue;
            }
            let unreadable = |detail: String| MetricError::SlintUnparsable {
                document: name.to_owned(),
                line: index + 1,
                detail,
            };
            let Some((_type, constant, expression)) = property_declaration(line) else {
                return Err(unreadable(format!("cannot read the declaration {line:?}")));
            };
            constants.insert(constant.to_owned(), value_of(expression));
        }
        Ok(Self { constants })
    }

    /// Reads the constants out of the file at `path`.
    ///
    /// # Return value
    ///
    /// [`MetricSource::Unavailable`] when the file does not exist, and when it exists but
    /// declares no constant at all. Both are the third source being missing, which a comparison
    /// reports rather than reads as agreement: a tree that lost the file and a block that was
    /// emptied would otherwise make every case pass.
    ///
    /// # Errors
    ///
    /// Returns [`MetricError::Io`] when the file exists and cannot be read, and
    /// [`MetricError::SlintUnparsable`] when a declaration cannot be read.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn load(path: &Path) -> Result<MetricSource<Self>, MetricError> {
        let name = path.display().to_string();
        match fs::read_to_string(path) {
            Ok(text) => {
                let constants = Self::parse(&name, &text)?;
                if constants.is_empty() {
                    return Ok(MetricSource::Unavailable {
                        reason: format!("{name} declares no metrics constant"),
                    });
                }
                Ok(MetricSource::Available(constants))
            }
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(MetricSource::Unavailable {
                reason: format!("{name} does not exist"),
            }),
            Err(source) => Err(MetricError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// One constant's value.
    pub fn value(&self, name: &str) -> Option<&MetricValue> {
        self.constants.get(name)
    }

    /// Every name the source declares, in name order.
    pub fn names(&self) -> Vec<&str> {
        self.constants.keys().map(String::as_str).collect()
    }

    /// How many names the source declares.
    pub fn len(&self) -> usize {
        self.constants.len()
    }

    /// Whether the source declares no constant at all.
    pub fn is_empty(&self) -> bool {
        self.constants.is_empty()
    }
}

/// The name and expression of a `public constant <Name>: <expression>;` line.
///
/// # Panics
///
/// Never.
fn constant_declaration(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix(CONSTANT_PREFIX)?;
    let (name, expression) = rest.split_once(':')?;
    let expression = expression.trim().strip_suffix(';')?.trim();
    let name = name.trim();
    (!name.is_empty() && !expression.is_empty()).then_some((name, expression))
}

/// The type, name and expression of a `<direction> property <<type>> <name>: <expression>;` line.
///
/// # Panics
///
/// Never.
fn property_declaration(line: &str) -> Option<(&str, &str, &str)> {
    let rest = DIRECTIONS
        .iter()
        .find_map(|direction| line.strip_prefix(direction))?;
    let (type_part, rest) = rest.split_once('>')?;
    let type_name = type_part.trim().strip_prefix('<')?;
    let (name, expression) = rest.split_once(':')?;
    let expression = expression.trim().strip_suffix(';')?.trim();
    let name = name.trim();
    (!name.is_empty() && !expression.is_empty()).then_some((type_name.trim(), name, expression))
}

/// The value an expression states, or the expression itself when it is not a number.
///
/// # Panics
///
/// Never.
fn value_of(expression: &str) -> MetricValue {
    let text = expression.trim();
    if let Some(number) = number_with(text, "px") {
        return MetricValue::LengthDp(number);
    }
    if let Some(number) = number_with(text, "sp") {
        return MetricValue::FontSp(number);
    }
    match text.parse::<u16>() {
        Ok(number) => MetricValue::Count(number),
        Err(_) => MetricValue::Unreadable(text.to_owned()),
    }
}

/// The number a `<number><unit>` expression states.
///
/// # Panics
///
/// Never.
fn number_with(text: &str, unit: &str) -> Option<u16> {
    text.strip_suffix(unit)?.trim().parse().ok()
}

/// `ShadowMargin` written the way the specification's 常量 column writes it: `shadow-margin`.
///
/// # Panics
///
/// Never.
fn kebab(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}
