//! The `[keys]` section as the routing layer's binding table.
//!
//! Responsibility: turn the key names a user writes in `config.toml` into the bit sets
//! the routing layer tests against, and redo that projection when the configuration
//! changes.
//!
//! Boundaries: this module reads [`crate::schema::KeysConfig`] and produces
//! [`project::KeyBindings`]. It knows nothing about the host, the session or the
//! candidate window, and it holds no state between calls -- the projection is a pure
//! function of the configuration, which is what makes a reload safe to perform while a
//! composition is in progress.

pub mod project;

pub use crate::keymap::project::{
    BINDING_CONFLICT_CODE, FlipSet, HighlightSet, KeyBindings, UNROUTABLE_BINDING_CODE,
    project_keys,
};
