//! The `[data]` and `[diagnostics]` sections of the configuration model.
//!
//! Responsibility: the types behind the keys that govern the user database -- how
//! its writes reach the disk and how its backups rotate -- and the keys that govern
//! the log. They live apart from the schema root because they are plain data over
//! the store and the log, carrying none of the decoding-adjacent spellings the root
//! holds. They are re-exported from [`crate::schema`], so the public path of every
//! type is the one it has always had.

use super::{DEFAULT_BACKUP_KEEP, Durability, LogLevel};

/// The `[data]` section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataConfig {
    /// `durability`: how user-frequency writes reach the disk. Defaults to
    /// [`Durability::Eventual`], the variant [`Durability`] marks as its default.
    pub durability: Durability,
    /// `backup_enabled`: whether the user's learned words are copied automatically.
    ///
    /// On by default: the store cannot be rebuilt from anywhere else, and the user it
    /// costs everything is exactly the one who never thought about backups. With the key
    /// off no generation is written, and the ones already on disk are left alone.
    pub backup_enabled: bool,
    /// `backup_keep`: how many backup generations are kept, `1..=MAX_BACKUP_KEEP`.
    ///
    /// The rotation removes the oldest generations past this count once a new one has
    /// landed, so zero would remove the copy that was just written. Such a value is
    /// reported and replaced by [`crate::schema::Config::repaired`].
    pub backup_keep: u8,
}

impl Default for DataConfig {
    /// The shipped defaults: eventual writes, backups on, [`DEFAULT_BACKUP_KEEP`]
    /// generations kept.
    ///
    /// Written out rather than derived, because two of the three keys are not
    /// zero-valued: a derived default would turn the copies off and ask for no
    /// generations at all.
    fn default() -> Self {
        Self {
            durability: Durability::Eventual,
            backup_enabled: true,
            backup_keep: DEFAULT_BACKUP_KEEP,
        }
    }
}

/// The `[diagnostics]` section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiagnosticsConfig {
    /// `level`: the verbosity of the log.
    pub level: LogLevel,
    /// `log_rotation_mb`: the size a log file reaches before it is rolled.
    pub log_rotation_mb: u32,
    /// `log_keep_files`: how many rolled log files are kept.
    pub log_keep_files: u8,
    /// `log_input_content`: accepted so that the user can see and control the
    /// setting, and deliberately without effect on what is recorded. The plugin never
    /// writes the characters a user types to the log; turning this on extends the
    /// diagnostics to input lengths and syllable counts only.
    pub log_input_content: bool,
    /// `probes`: whether the diagnostic probes are switched on.
    pub probes: bool,
}

impl DiagnosticsConfig {
    /// Whether the characters a user types may be written to the log.
    ///
    /// # Returns
    ///
    /// Always `false`, whatever `log_input_content` says. This is the single place
    /// that answers the question, so the promise holds by construction rather than by
    /// every call site remembering it.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn logs_input_characters(&self) -> bool {
        false
    }
}
