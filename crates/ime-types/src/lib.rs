//! rspinyin frozen-contract crate.
//!
//! **This crate is the single global contract freeze point** for the project: every
//! cross-boundary type (error model, IDs, `UiCommand`/`UiEvent`, `DecodeRequest`/
//! `DecodeResult`, the `Lexicon`/`UserFreqSource`/`LanguageModel`/`SurfaceBackend`
//! traits, and `KeyAction`) is defined here so that the engine and UI tracks can be
//! developed in parallel against a stable interface.
//!
//! # Changing this crate
//!
//! Once frozen, any modification must be recorded as an ADR under `docs/dev/adr/`
//! and reflected back into the architecture spec's boundary-contract section.
//! Cross-boundary types must never be invented inside a business crate.
//!
//! # Dependency constraints
//!
//! This crate must not depend on any other crate in this workspace, nor on `slint`,
//! `redb`, `fst`, `wayland-client`, or `x11rb`. That restriction is what keeps UI and
//! platform details from leaking across the contract boundary.

pub mod decode;
pub mod error;
pub mod ids;
pub mod key;
pub mod lexicon;
pub mod surface;
pub mod ui;
pub mod version;

pub use crate::decode::{DecodeFlags, DecodeRequest, DecodeResult, SchemeId, Segment, SyllableId};
pub use crate::error::{ConfigError, DecodeError, DictError, ImeError, PlatformError, UiError};
pub use crate::ids::{Revision, ScreenId, SessionId, WordId};
pub use crate::key::KeyAction;
pub use crate::lexicon::{
    LanguageModel, Lexicon, UserFreqSource, WORD_ITER_INLINE, WordFlags, WordIter, WordRef,
};
pub use crate::surface::{FrameToken, PixelBufferMut, SurfaceBackend, SurfaceEvent};
pub use crate::ui::{
    Anchor, Candidate, CandidateSource, ColorScheme, DismissReason, HideReason, LayoutHint,
    OverlayEntry, OverlayFrame, OverlayKind, OverlaySection, PageDir, PageState, Placement,
    Preedit, PreeditSpan, RectI, Rgba8, SelectTrigger, SpanKind, StatusStrip, ThemeSpec, UiCommand,
    UiEvent, UiFrame,
};
pub use crate::version::{
    CONFIG_SCHEMA_VERSION, DICT_FORMAT_VERSION, RSPINYIN_ABI_VERSION, check_abi,
};
