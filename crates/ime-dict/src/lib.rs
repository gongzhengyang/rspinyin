//! rspinyin dictionary layer.
//!
//! Provides read/write support for the compiled `base.dict` format, FST index
//! construction, zero-copy read-only `mmap` loading, and the `redb`-backed user
//! frequency store.
//!
//! # Unsafe boundary
//!
//! Only `src/mmap.rs` in this crate may contain `unsafe`. Everywhere else must stay
//! safe Rust; the restriction is enforced in CI by the unsafe-audit script.

pub mod format;
