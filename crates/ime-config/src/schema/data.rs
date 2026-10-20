//! The `[data]` and `[diagnostics]` sections of the configuration model.
//!
//! Responsibility: the types behind the keys that govern the user database -- how
//! its writes reach the disk and how its backups rotate -- and the keys that govern
//! the log. They live apart from the schema root because they are plain data over
//! the store and the log, carrying none of the decoding-adjacent spellings the root
//! holds. They are re-exported from [`crate::schema`], so the public path of every
//! type is the one it has always had.

use super::{DEFAULT_BACKUP_KEEP, DEFAULT_DECAY_HALF_LIFE_TICKS, Durability, LogLevel};

/// The `[data]` section.
///
/// Not `Copy`: `dict_staged_path` is a path a user typed, and a section that holds one
/// text holds a heap value. Every hand-off of the section past this change is a clone,
/// which is a reload-path cost of one allocation and nothing the decode path touches.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// `decay_half_life_ticks`: logical commits over which user usage halves.
    pub decay_half_life_ticks: u32,
    /// `decay_floor`: the user accumulator floor in Q16.16.
    pub decay_floor: i64,
    /// `dict_staged_path`: the dictionary file a reload should swap in, or empty when
    /// there is none.
    ///
    /// The hot-swap trigger: a reload that finds the key naming a readable container
    /// validates it whole and swaps it for the live one, then writes the key back
    /// empty -- the path is a one-shot instruction, not a setting. A swap the
    /// validation rejects leaves the key standing for the user to fix, and reports
    /// `dict/swap/rejected`.
    pub dict_staged_path: String,
}

impl Default for DataConfig {
    /// The shipped defaults: eventual writes, backups on, [`DEFAULT_BACKUP_KEEP`]
    /// generations kept, and decay enabled at the librime-equivalent half-life with a
    /// one-use accumulator floor. The staged path is empty: nothing to swap until a
    /// user or a tool writes a path there.
    ///
    /// Written out rather than derived, because two of the four scalar keys are not
    /// zero-valued: a derived default would turn the copies off and ask for no
    /// generations at all.
    fn default() -> Self {
        Self {
            durability: Durability::Eventual,
            backup_enabled: true,
            backup_keep: DEFAULT_BACKUP_KEEP,
            decay_half_life_ticks: DEFAULT_DECAY_HALF_LIFE_TICKS,
            decay_floor: 1 << 16,
            dict_staged_path: String::new(),
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
