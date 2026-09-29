//! rspinyin configuration layer.
//!
//! TOML configuration loading, validation, and hot reload. A reload must preserve any
//! input session that is in progress — an active composition is never reset, because
//! losing a user's in-flight input is a worse failure than deferring the reload.
