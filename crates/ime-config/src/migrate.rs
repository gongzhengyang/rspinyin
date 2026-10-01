//! Bringing an older configuration document forward to the schema this build reads.
//!
//! Responsibility: decide what an on-disk `config.toml` is missing for the schema version
//! this build reads, add it one version at a time, and keep the file the user had. The
//! model lives in [`crate::schema`] and the ordinary reading and writing of the file
//! belongs to [`crate::reload`]; what is here is the step between the two, run on the
//! parsed document before it is deserialized, because a version-1 document may not
//! deserialize into this build's configuration at all.
//!
//! # Why the document and not the configuration
//!
//! A step rewrites a [`toml::Value`], not a [`crate::schema::Config`]. Two properties
//! follow from that, and both are the reason for the choice. A key this build does not
//! know survives the rewrite, so a user who goes back to an older build still finds their
//! settings. And a value this build would reject -- a number outside its range, a spelling
//! outside its set -- is carried over untouched rather than repaired away, because
//! repairing is [`crate::schema::Config::repaired`]'s job and it runs after the migration.
//!
//! # What a migration may not do
//!
//! It may not lose a setting, and it may not guess. The report is built by comparing the
//! document before and after the last step, so a step cannot drop a key without it
//! appearing in [`MigrationReport::dropped`]. A document that declares a version this
//! build does not know is refused rather than coerced: a newer file belongs to a newer
//! build, and reading it with older rules is how a setting is lost.
//!
//! # The file the user had is kept
//!
//! The original is copied to `<name>.v<version>` beside itself and the migrated document
//! takes its place, written in a temporary file that a rename puts where the original was.
//! Keeping the original is not a promise of a downgrade path -- there is none -- it is
//! what makes going back to a build that reads the older schema survivable. Writing
//! through a rename is what keeps the path holding a whole configuration at every instant,
//! so a crash cannot leave half a file where the user's settings were.
//!
//! Nothing here is fatal. A configuration directory the user cannot write to is a state
//! the plugin has to survive, so the migration still happens in memory and the report says
//! that nothing was written; the caller records that as read-only mode.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use ime_types::{CONFIG_SCHEMA_VERSION, ConfigError, ImeError};

/// The key that carries the schema version of a document.
const SCHEMA_VERSION_KEY: &str = "schema_version";

/// The key reported for a failure that concerns the whole document rather than one of its
/// keys.
const DOCUMENT_KEY: &str = "config";

/// How many `.1`, `.2`, ... suffixes are tried before a name is given up on.
const MAX_NAME_ATTEMPTS: u32 = 100;

/// The mode the kept original carries.
///
/// The copy is a duplicate of the user's configuration that they did not ask for, so it is
/// created readable by its owner alone. The rewritten file does not use this mode: it
/// takes the mode of the file it replaces, so a configuration the user had restricted does
/// not become readable by others because it was migrated.
const BACKUP_MODE: u32 = 0o600;

/// The mode the migrated document is built in before the rename puts it in place.
///
/// The window it covers is one write, and the rename immediately gives the file the mode
/// of the one it replaces.
const TEMP_MODE: u32 = 0o600;

/// The suffix the migrated document is built under beside the file it replaces.
const TEMP_SUFFIX: &str = ".migrating";

/// The `[phrases] max_entries` default schema version 2 introduces.
const V2_PHRASE_ENTRIES: i64 = 5_000;

/// The `[data] backup_keep` default schema version 2 introduces.
const V2_BACKUP_KEEP: i64 = 3;

/// One step from one schema version to the next.
///
/// Steps are chained rather than written as "any version to the current one": a user who
/// skipped three releases needs all three steps applied in order, and a single monolithic
/// upgrader would have to reimplement every intermediate shape anyway.
pub trait MigrationStep {
    /// The version this step reads.
    fn source_version(&self) -> u16;

    /// The version this step produces; always [`MigrationStep::source_version`] plus one.
    fn target_version(&self) -> u16;

    /// Rewrites the document in place.
    ///
    /// The document is a [`toml::Value`], not a configuration: migration happens before
    /// deserialization, because a version-1 file may not deserialize into this build's
    /// configuration at all. Working on the document also preserves keys this build does
    /// not know about, which is what lets a user roll back.
    ///
    /// # Returns
    ///
    /// What the step had to report about the rewrite -- a section it could not complete,
    /// say -- rendered as its stable code and carried into
    /// [`MigrationReport::notes`]. A step that changed nothing but what it meant to
    /// returns an empty list.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Invalid`] when the document is not a document of
    /// [`MigrationStep::from_version`] at all: one that declares another version, or one
    /// that declares none and so cannot be shown to be the version this step reads.
    ///
    /// # Panics
    ///
    /// Never.
    fn apply(&self, doc: &mut toml::Value) -> Result<Vec<ImeError>, ConfigError>;
}

/// The step from schema version 1 to schema version 2.
///
/// Version 2 adds three sections -- `[phrases]`, `[scheme]` and `[script]` -- and three
/// `[data]` keys. Nothing version 1 could hold is renamed, moved or reinterpreted, so a
/// version-1 document migrates without losing anything: every key it had keeps the value
/// it had, including the keys this step does not recognise.
pub struct V1ToV2;

impl MigrationStep for V1ToV2 {
    fn source_version(&self) -> u16 {
        1
    }

    fn target_version(&self) -> u16 {
        2
    }

    fn apply(&self, doc: &mut toml::Value) -> Result<Vec<ImeError>, ConfigError> {
        let table = doc.as_table_mut().ok_or_else(|| {
            invalid(
                DOCUMENT_KEY,
                String::from("the document is not a TOML table"),
            )
        })?;
        check_declared_version(table, self.source_version())?;

        let mut raised = Vec::new();
        add_version_two_keys(table, &mut raised);
        table.insert(
            String::from(SCHEMA_VERSION_KEY),
            toml::Value::Integer(i64::from(self.target_version())),
        );
        Ok(raised)
    }
}

/// Adds every key schema version 2 introduces, reporting a section that is not a table.
///
/// The defaults are written here rather than read from [`crate::schema`]: they are what
/// version 2 *meant*, so a later build that changes one of its own defaults must not
/// change what an old document is migrated to.
fn add_version_two_keys(table: &mut toml::Table, raised: &mut Vec<ImeError>) {
    add_defaults(
        table,
        "phrases",
        &[
            ("enabled", toml::Value::Boolean(true)),
            ("file", toml::Value::String(String::new())),
            ("max_entries", toml::Value::Integer(V2_PHRASE_ENTRIES)),
        ],
        raised,
    );
    add_defaults(
        table,
        "scheme",
        &[
            ("scheme", toml::Value::String(String::from("full"))),
            ("show_hint", toml::Value::Boolean(true)),
            ("keep_full_pinyin", toml::Value::Boolean(true)),
        ],
        raised,
    );
    add_defaults(
        table,
        "script",
        &[
            ("enabled", toml::Value::Boolean(false)),
            ("traditional", toml::Value::Boolean(false)),
            ("hotkey", toml::Value::String(String::from("ctrl+shift+f"))),
        ],
        raised,
    );
    add_defaults(
        table,
        "data",
        &[
            ("backup_enabled", toml::Value::Boolean(true)),
            ("backup_keep", toml::Value::Integer(V2_BACKUP_KEEP)),
            ("export_dir", toml::Value::String(String::new())),
        ],
        raised,
    );
}

/// Adds one section's defaults, creating the section when it is not there.
///
/// A key the document already holds is never overwritten. The migration carries settings
/// forward, so a value that is already present -- whether the user wrote it by hand or a
/// half-finished migration left it -- is the one to keep.
fn add_defaults(
    table: &mut toml::Table,
    section: &str,
    defaults: &[(&str, toml::Value)],
    raised: &mut Vec<ImeError>,
) {
    let Some(section_table) = table_section(table, section) else {
        raised.push(ImeError::from(invalid(
            section,
            String::from(
                "holds a value that is not a table, so the keys schema version 2 adds to it \
                 were not added",
            ),
        )));
        return;
    };
    for (key, value) in defaults {
        section_table
            .entry(String::from(*key))
            .or_insert_with(|| value.clone());
    }
}

/// The `[name]` table of `document`, created empty when it is not there.
///
/// `None` when the key holds something other than a table. Such a key is left exactly as
/// it is and reported by the caller: the loader already answers a wrongly typed key with a
/// diagnostic and the built-in default, and refusing the whole migration over one would
/// cost the user every other setting in the file.
fn table_section<'a>(table: &'a mut toml::Table, name: &str) -> Option<&'a mut toml::Table> {
    table
        .entry(String::from(name))
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
}

/// Refuses a document that does not declare the version a step reads.
///
/// The step is only ever reached with a document whose declared version matches, so a
/// mismatch here means the plan was built wrongly. It is checked anyway, because the cost
/// of being wrong is the opposite of what a migration is for: rewriting a document with
/// the rules of another version is how a setting is silently lost. A document that
/// declares no version is refused for the same reason -- it cannot be shown to be the
/// document this step reads.
fn check_declared_version(table: &toml::Table, expected: u16) -> Result<(), ConfigError> {
    match table.get(SCHEMA_VERSION_KEY) {
        Some(toml::Value::Integer(declared)) if *declared == i64::from(expected) => Ok(()),
        Some(other) => Err(invalid(
            SCHEMA_VERSION_KEY,
            format!(
                "this step reads schema version {expected}, the document declares {}",
                describe(other)
            ),
        )),
        None => Err(invalid(
            SCHEMA_VERSION_KEY,
            format!(
                "the document declares no schema version, so it cannot be read as version \
                 {expected}"
            ),
        )),
    }
}

/// The `v1 -> v2` step, as a value rather than a type, so that [`STEPS`] can name it.
pub const V1_TO_V2: V1ToV2 = V1ToV2;

/// Every step this build knows, in ascending version order.
pub const STEPS: &[&dyn MigrationStep] = &[&V1_TO_V2];

/// What a migration did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationReport {
    /// The version the document was at.
    pub from: u16,
    /// The version it is at now.
    pub to: u16,
    /// Where the pre-migration file was kept, if it was kept.
    ///
    /// `None` means nothing was written: the file could not be read back, or the directory
    /// holding it could not be written to. The migration itself still happened -- the
    /// caller runs on the migrated document -- and the next start migrates again, which is
    /// why a migration is idempotent.
    pub backup: Option<PathBuf>,
    /// Keys the migration added, with their defaults.
    pub added: Vec<String>,
    /// Keys the migration could not carry over.
    ///
    /// Built by comparing the document before and after the last step, so a step cannot
    /// drop a setting without it being named here. A version-1 document has nothing to
    /// lose, so a `v1 -> v2` migration leaves this empty; the list exists so that a later
    /// step that cannot preserve a setting has to say so rather than drop it quietly.
    pub dropped: Vec<String>,
    /// What the steps and the write-back had to report, each as its stable code.
    ///
    /// A step reports here a section it could not complete; so does a document that could
    /// not be written back. This is the list a caller records beside the migration itself,
    /// which names the two versions and the backup but not why a key was left alone.
    pub notes: Vec<String>,
}

/// Brings a document up to [`CONFIG_SCHEMA_VERSION`].
///
/// # Parameters
///
/// - `doc`: the parsed configuration document, rewritten in place when it is older than
///   this build's schema. It is left exactly as it was when a step fails.
/// - `path`: the file the document was read from. The file the user had is copied beside
///   it and the migrated document replaces it, so a migration is never the reason a
///   configuration is lost.
///
/// # Returns
///
/// `None` when the document is already at this build's schema version, which is the
/// ordinary case and what makes a second call a no-op. `Some(report)` otherwise, naming
/// what changed and where the original was kept.
///
/// # Errors
///
/// [`ConfigError::Invalid`] when the document declares a version this build does not know
/// -- a newer document is refused rather than read with older rules -- when the document
/// is not a TOML table, when `schema_version` holds something other than a number the
/// version field can hold, when no step reads a version on the way to the current one, or
/// when a step fails. The caller starts from the built-in defaults in that case rather
/// than refusing to start: an input method that will not start because of a configuration
/// file is worse than one that starts with default settings and says so.
///
/// # Panics
///
/// Never.
pub fn migrate(doc: &mut toml::Value, path: &Path) -> Result<Option<MigrationReport>, ConfigError> {
    run(STEPS, CONFIG_SCHEMA_VERSION, doc, path)
}

/// Brings `doc` up to `target` by walking `steps`.
///
/// The step list is a parameter rather than [`STEPS`] read directly so that a test can
/// drive a chain this build does not have, and so that the framework's own rules -- the
/// comparison the report is built from, the refusal of a gap in the chain -- can be
/// exercised without a version to migrate to.
///
/// # Errors
///
/// As [`migrate`].
fn run(
    steps: &[&dyn MigrationStep],
    target: u16,
    doc: &mut toml::Value,
    path: &Path,
) -> Result<Option<MigrationReport>, ConfigError> {
    let from = declared_version(doc, target)?;
    if from == target {
        return Ok(None);
    }
    if from > target {
        return Err(invalid(
            SCHEMA_VERSION_KEY,
            format!(
                "the document declares schema version {from} and this build reads version \
                 {target}; a newer document is left for the build that wrote it"
            ),
        ));
    }
    let chain = plan(steps, from, target)?;

    // The steps rewrite a copy, so a step that fails half way through leaves the caller's
    // document exactly as it was: a partly migrated document is not a document that any
    // version's rules describe.
    let before = flatten(doc);
    let mut migrated = doc.clone();
    let mut notes = Vec::new();
    for step in chain {
        for error in step.apply(&mut migrated)? {
            notes.push(error.to_string());
        }
    }
    let after = flatten(&migrated);

    let backup = match persist(path, &migrated, from) {
        Persisted::Kept(backup) => Some(backup),
        Persisted::NotWritten(reason) => {
            notes.push(reason);
            None
        }
    };
    let report = MigrationReport {
        from,
        to: target,
        backup,
        added: rendered(&difference(&before, &after)),
        dropped: difference(&after, &before)
            .into_iter()
            .map(|(key, _)| key)
            .collect(),
        notes,
    };
    *doc = migrated;
    Ok(Some(report))
}

/// The chain of steps that carries `from` to `target`, in the order they are applied.
///
/// # Errors
///
/// [`ConfigError::Invalid`] naming `schema_version` when no step reads `version`, or when
/// a step does not produce exactly the next version. A gap in the chain is refused rather
/// than stepped over: applying a later step's rules to a document that never took the
/// earlier one's shape is how a setting is silently lost.
fn plan<'a>(
    steps: &[&'a dyn MigrationStep],
    from: u16,
    target: u16,
) -> Result<Vec<&'a dyn MigrationStep>, ConfigError> {
    let mut chain = Vec::new();
    let mut version = from;
    // `version < target <= u16::MAX`, so the increment below cannot overflow.
    while version < target {
        let step = steps
            .iter()
            .copied()
            .find(|step| step.source_version() == version);
        let Some(step) = step else {
            return Err(invalid(
                SCHEMA_VERSION_KEY,
                format!("no migration step reads schema version {version}"),
            ));
        };
        if step.target_version() != version + 1 {
            return Err(invalid(
                SCHEMA_VERSION_KEY,
                format!(
                    "the step that reads schema version {version} produces version {}, which \
                     is not the next one",
                    step.target_version()
                ),
            ));
        }
        chain.push(step);
        version = step.target_version();
    }
    Ok(chain)
}

/// The schema version `doc` declares, or `assumed` when it declares none.
///
/// A document with no `schema_version` is taken to be current: the key is optional in
/// every version, the loader has always accepted a document without it, and rewriting a
/// file that cannot be shown to be older would change something the user did not ask to
/// change.
///
/// # Errors
///
/// [`ConfigError::Invalid`] naming the whole document when it is not a TOML table, and
/// naming `schema_version` when the key is there but does not hold a version. The number
/// is converted with `try_into` rather than a cast: a document stating a version wider
/// than the field would otherwise be truncated into one it does not mean.
fn declared_version(doc: &toml::Value, assumed: u16) -> Result<u16, ConfigError> {
    let table = doc.as_table().ok_or_else(|| {
        invalid(
            DOCUMENT_KEY,
            format!("the document is {}, not a TOML table", type_name(doc)),
        )
    })?;
    let Some(raw) = table.get(SCHEMA_VERSION_KEY) else {
        return Ok(assumed);
    };
    let number = raw.as_integer().ok_or_else(|| {
        invalid(
            SCHEMA_VERSION_KEY,
            format!("not a schema version but {}", type_name(raw)),
        )
    })?;
    u16::try_from(number).map_err(|_| {
        invalid(
            SCHEMA_VERSION_KEY,
            format!("{number} is not a schema version this field can hold"),
        )
    })
}

/// Every leaf of the document, keyed by its dotted path and valued as TOML syntax.
///
/// A table contributes the keys under it rather than an entry of its own, so what the
/// comparison in [`run`] sees is the settings themselves: a section that was created and
/// left empty is not a setting, and reporting one would tell the user nothing.
fn flatten(doc: &toml::Value) -> BTreeMap<String, String> {
    let mut leaves = BTreeMap::new();
    collect_leaves(doc, "", &mut leaves);
    leaves
}

/// Adds every leaf under `value` to `leaves`, prefixing each key with `path`.
fn collect_leaves(value: &toml::Value, path: &str, leaves: &mut BTreeMap<String, String>) {
    let toml::Value::Table(table) = value else {
        leaves.insert(String::from(path), value.to_string());
        return;
    };
    for (key, child) in table {
        let child_path = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        collect_leaves(child, &child_path, leaves);
    }
}

/// The entries of `candidate` whose key `known` does not hold.
///
/// Called both ways round: `difference(&before, &after)` is what the migration added and
/// `difference(&after, &before)` is what it could not carry over.
fn difference(
    known: &BTreeMap<String, String>,
    candidate: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    candidate
        .iter()
        .filter(|(key, _)| !known.contains_key(*key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Renders `key = value` for each entry, in the order the map holds them.
fn rendered(entries: &[(String, String)]) -> Vec<String> {
    entries
        .iter()
        .map(|(key, value)| format!("{key} = {value}"))
        .collect()
}

/// Names a value in a diagnostic: the number when it is one, and the kind of value
/// otherwise.
///
/// A table or an array echoed in full would put a copy of the document in the message, and
/// a diagnostic nobody can read is a diagnostic nobody acts on.
fn describe(value: &toml::Value) -> String {
    match value {
        toml::Value::Integer(number) => number.to_string(),
        other => String::from(type_name(other)),
    }
}

/// The name of the TOML type a value carries.
fn type_name(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "a string",
        toml::Value::Integer(_) => "an integer",
        toml::Value::Float(_) => "a float",
        toml::Value::Boolean(_) => "a boolean",
        toml::Value::Datetime(_) => "a date and time",
        toml::Value::Array(_) => "an array",
        toml::Value::Table(_) => "a table",
    }
}

/// The `config/invalid` failure for one key.
fn invalid(key: &str, reason: String) -> ConfigError {
    ConfigError::Invalid {
        key: String::from(key),
        reason,
    }
}

/// What writing a migration back to the disk produced.
enum Persisted {
    /// The file the user had is at this path and the migrated document is in its place.
    Kept(PathBuf),
    /// Nothing was written; the reason is what the caller records.
    NotWritten(String),
}

/// Writes `document` over `path`, keeping the file that was there beside it.
///
/// The original is copied rather than moved, and the new document is built in a temporary
/// file that a rename puts in its place. Two things follow. The path holds a whole
/// configuration at every instant -- either the old one or the new one -- so a crash
/// cannot leave half a file where the user's settings were. And the copy is what the user
/// goes back to when they return to a build that reads the older schema.
///
/// Nothing here is fatal. A configuration directory the user cannot write to is a state
/// the plugin has to survive, so a failure is reported as [`Persisted::NotWritten`] and
/// the caller runs on the migrated document in memory.
fn persist(path: &Path, document: &toml::Value, from: u16) -> Persisted {
    let text = match toml::to_string(document) {
        Ok(text) => text,
        Err(error) => {
            return Persisted::NotWritten(format!(
                "the migrated configuration could not be rendered: {error}"
            ));
        }
    };
    let backup = match keep_original(path, from) {
        Ok(backup) => backup,
        Err(error) => {
            return Persisted::NotWritten(format!(
                "the configuration could not be kept beside itself: {error}"
            ));
        }
    };
    match replace(path, &text) {
        Ok(()) => Persisted::Kept(backup),
        Err(error) => {
            // The copy was made for a replacement that did not happen, and the file the
            // user had is still exactly where it was. Leaving the copy behind would make
            // the next start pick a different name for the same thing.
            discard(&backup);
            Persisted::NotWritten(format!(
                "the migrated configuration could not be written: {error}"
            ))
        }
    }
}

/// Copies the file at `path` to the first free `<name>.v<from>` beside it.
///
/// The bytes are copied rather than the parsed document re-rendered, so the copy is what
/// the user actually had, comments and key order included.
///
/// # Errors
///
/// The underlying [`io::Error`] when the file cannot be read, when every candidate name is
/// taken, or when the copy cannot be written.
fn keep_original(path: &Path, from: u16) -> io::Result<PathBuf> {
    let original = fs::read(path)?;
    let base = suffixed(path, &format!(".v{from}"));
    let (backup_path, mut backup) = claim_free_file(&base, BACKUP_MODE)?;
    match backup.write_all(&original) {
        Ok(()) => Ok(backup_path),
        Err(error) => {
            discard(&backup_path);
            Err(error)
        }
    }
}

/// Builds `text` in a temporary file beside `path` and renames it over `path`.
///
/// The temporary file is given the mode of the file it replaces, so a configuration the
/// user had restricted does not become readable by others because it was migrated. The
/// written file is flushed to the device before the rename, so the name is never
/// published over bytes that are still only in the page cache.
///
/// # Errors
///
/// The underlying [`io::Error`] when the mode cannot be read, when no temporary name is
/// free, when the write or the flush fails, or when the rename fails. `path` is left
/// untouched in every one of those cases.
fn replace(path: &Path, text: &str) -> io::Result<()> {
    let permissions = fs::metadata(path)?.permissions();
    let (temp_path, mut temp) = claim_free_file(&suffixed(path, TEMP_SUFFIX), TEMP_MODE)?;
    let built = temp
        .write_all(text.as_bytes())
        .and_then(|()| fs::set_permissions(&temp_path, permissions))
        // The flush is what makes the rename meaningful: without it the name could be
        // published while the bytes are still only in the page cache, and a power loss
        // would leave a complete name over an incomplete file.
        .and_then(|()| temp.sync_all());
    drop(temp);
    if let Err(error) = built {
        discard(&temp_path);
        return Err(error);
    }
    match fs::rename(&temp_path, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            discard(&temp_path);
            Err(error)
        }
    }
}

/// Creates the first free file named `base`, `base.1`, `base.2`, ... and returns it open
/// for writing.
///
/// `create_new` is what claims the name: it either fails with `EEXIST`, which sends the
/// caller to the next name, or succeeds, which reserves the name against every other
/// writer. Nothing is ever overwritten, so a name that is already taken -- an original
/// kept by an earlier migration, a copy the user made -- costs a suffix rather than a
/// file.
///
/// # Errors
///
/// The underlying [`io::Error`] when no candidate name is free, or when the directory
/// cannot be written to.
fn claim_free_file(base: &Path, mode: u32) -> io::Result<(PathBuf, File)> {
    for attempt in 0..MAX_NAME_ATTEMPTS {
        let candidate = if attempt == 0 {
            base.to_path_buf()
        } else {
            suffixed(base, &format!(".{attempt}"))
        };
        let opened = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&candidate);
        match opened {
            Ok(file) => {
                // `open` applies the mode through the process umask, which can only clear
                // bits, so the mode is set again to what the caller asked for. It is set
                // through the descriptor that was just opened rather than through a second
                // lookup of the path, and the file never exists with a wider mode than the
                // one requested.
                if let Err(error) = file.set_permissions(fs::Permissions::from_mode(mode)) {
                    discard(&candidate);
                    return Err(error);
                }
                return Ok((candidate, file));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        String::from("every name beside the configuration file is taken"),
    ))
}

/// Removes a file this module created.
///
/// Best effort on purpose: every caller is already on a path that reports a failure, and a
/// file that could not be removed costs a name beside the configuration rather than a
/// setting.
fn discard(path: &Path) {
    let _ = fs::remove_file(path);
}

/// Appends `suffix` to `path`, which carries no separator of its own.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests;
