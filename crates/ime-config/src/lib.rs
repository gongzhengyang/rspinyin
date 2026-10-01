//! rspinyin configuration layer.
//!
//! TOML configuration loading, validation, and hot reload. A reload must preserve any
//! input session that is in progress — an active composition is never reset, because
//! losing a user's in-flight input is a worse failure than deferring the reload.

pub mod keymap;
pub mod migrate;
pub mod reload;
pub mod schema;
pub mod scheme;
pub mod writeback;

pub use crate::writeback::{WRITEBACK_RACE_CODE, WritebackError, write_keys};

pub use crate::reload::{ConfigStore, FILE_NAME, ReloadOutcome, default_path};
pub use crate::schema::{
    AnimationConfig, Config, DEFAULT_BACKUP_KEEP, DEFAULT_PHRASE_ENTRIES, DataConfig,
    DiagnosticsConfig, DigitZero, Durability, EngineConfig, KeyName, KeysConfig, LogLevel,
    MAX_BACKUP_KEEP, MAX_PHRASE_ENTRIES, PhraseConfig, PunctMode, Rgb, ThemeConfig, ThemeScheme,
    UiConfig, VerifyDictOnLoad,
};
pub use crate::scheme::{CustomSchemeConfig, SchemeChoice, SchemeConfig};
