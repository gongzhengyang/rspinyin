//! rspinyin configuration layer.
//!
//! TOML configuration loading, validation, and hot reload. A reload must preserve any
//! input session that is in progress — an active composition is never reset, because
//! losing a user's in-flight input is a worse failure than deferring the reload.

pub mod reload;
pub mod schema;

pub use crate::reload::{ConfigStore, FILE_NAME, ReloadOutcome, default_path};
pub use crate::schema::{
    AnimationConfig, Config, DataConfig, DiagnosticsConfig, DigitZero, Durability, EngineConfig,
    KeyName, KeysConfig, LogLevel, PunctMode, Rgb, ThemeConfig, ThemeScheme, UiConfig,
    VerifyDictOnLoad,
};
