//! The frozen cross-boundary error model.
//!
//! `ImeError` is the only error type that crosses crate boundaries and
//! `DictError` is the dictionary-domain error it embeds. Both render as stable
//! `domain/action/reason` codes; those codes are matched by diagnostics, probes
//! and tests, so an existing code is never reworded.
//!
//! Four further domain enums describe failures inside a single layer:
//! `UiError`, `DecodeError`, `ConfigError` and `PlatformError`. They carry the
//! codes their own layer raises (`ui/select/timeout`, `decode/empty-input`, ...)
//! and are convertible into `ImeError` so that a crate-local error can always be
//! propagated with `?`. Where a domain code has no counterpart in the frozen
//! `ImeError` list the conversion collapses it onto the closest variant; the
//! precise code is expected to be recorded as a diagnostic at the failure site.
//!
//! This module is part of the frozen contract: changing it requires an ADR under
//! `docs/dev/adr/`.

use std::path::PathBuf;

/// Unified cross-boundary error.
///
/// Every variant renders as a stable code; diagnostics and tests match on those
/// codes, so they are contractual. The enum is neither `Clone` nor `PartialEq`
/// because it embeds `DictError`, which wraps `std::io::Error`; that type is
/// neither cloneable nor comparable.
#[derive(Debug, thiserror::Error)]
pub enum ImeError {
    #[error("dict/unavailable: {path} ({cause})")]
    DictUnavailable { path: PathBuf, cause: DictError },
    #[error("dict/corrupt: {path}")]
    DictCorrupt { path: PathBuf },
    #[error("decode/too-long: len={len} max={max}")]
    DecodeTooLong { len: usize, max: usize },
    #[error("decode/invalid-char: {ch:?} at {at}")]
    DecodeInvalidChar { ch: char, at: usize },
    #[error("decode/no-path: {raw}")]
    DecodeNoPath { raw: String },
    #[error("config/invalid: {key} ({reason})")]
    ConfigInvalid { key: String, reason: String },
    #[error("data/readonly-mode: {reason}")]
    DataReadonly { reason: String },
    #[error("ui/channel-closed")]
    UiChannelClosed,
    #[error("ui/stale-select: revision={got} current={current}")]
    UiStaleSelect { got: u32, current: u32 },
    #[error("ui/font/missing-cjk")]
    UiFontMissingCjk,
    #[error("platform/fcitx5/version-mismatch: host={host} required={required}")]
    Fcitx5VersionMismatch { host: String, required: String },
    #[error("platform/fcitx5/dev-missing")]
    Fcitx5DevMissing,
    #[error("platform/compositor/unsupported: {detail}")]
    CompositorUnsupported { detail: String },
    #[error("platform/start/unsupported-os: {detail}")]
    UnsupportedOs { detail: String },
    #[error("ffi/invalid-commit")]
    FfiInvalidCommit,
    /// A capability is deliberately not available in this build phase.
    ///
    /// `Lexicon::prefix` returns this instead of inventing a code of its own:
    /// the interface is frozen now, while prefix enumeration (abbreviation
    /// expansion) is implemented in Phase 2. Appended to the frozen variant list
    /// by ADR-0001 so that the position of every variant above stays unchanged.
    #[error("dict/unsupported")]
    Unsupported,
    /// The user's data could not be backed up.
    ///
    /// Appended by ADR-0005. The code sits in the existing `data/*` segment rather
    /// than opening a new domain, which is the rule for every appended code.
    #[error("data/backup-failed: {reason}")]
    DataBackupFailed { reason: String },
    /// The requested double-pinyin scheme is one this build does not implement.
    ///
    /// Appended by ADR-0005. `scheme` is the raw number from the configuration
    /// file, which is why it is a `u8` rather than a `SchemeId`: reporting the
    /// number a newer build wrote is the point of the diagnostic, and a caller
    /// must be able to raise it without first constructing a scheme it knows is
    /// invalid.
    #[error("decode/scheme-unsupported: scheme={scheme}")]
    SchemeUnsupported { scheme: u8 },
    /// An older configuration document was migrated forward.
    ///
    /// Appended by ADR-0005 alongside the `ConfigError` variant of the same shape.
    /// It is carried on both types on purpose. `config/migrated` is in the ADR's
    /// stable-code list, and a code that no rendering can produce is a code no
    /// diagnostic or test can match; but more than that, the alternative was to
    /// fold a *successful* migration onto [`ImeError::ConfigInvalid`] in the `From`
    /// conversion, which would tell the user their configuration failed when in
    /// fact it was repaired. A level of `info` at the call site keeps it out of the
    /// error path's noise.
    #[error("config/migrated: from={from} to={to} backup={backup}")]
    ConfigMigrated { from: u16, to: u16, backup: String },
    /// Nothing was recorded under the key the caller asked to drop.
    ///
    /// Appended by ADR-0005. The code sits in the existing `dict/*` segment, but the
    /// variant lives here rather than on [`DictError`]: that type is documented as
    /// the failures raised "while validating and reading the compiled dictionary",
    /// and this one describes a lookup that succeeded and found nothing. Putting it
    /// there forced every `DictError` classifier to invent an answer -- the
    /// quarantine logic in `ime-dict`'s recovery pass would have had to decide
    /// whether "the word was not found" means the *file* is damaged.
    #[error("dict/user-word-not-found")]
    UserWordNotFound,
    /// The export would write more than the caller allows.
    ///
    /// Appended by ADR-0005, for the same reason as
    /// [`ImeError::UserWordNotFound`].
    #[error("dict/export-too-large: bytes={bytes} limit={limit}")]
    ExportTooLarge { bytes: u64, limit: u64 },
}

/// Dictionary-container failures.
///
/// These are the errors raised while validating and reading the compiled
/// dictionary; the container is untrusted input, so every length and checksum is
/// verified before any offset is trusted.
#[derive(Debug, thiserror::Error)]
pub enum DictError {
    #[error("magic mismatch")]
    MagicMismatch,
    #[error("unsupported format version {found}")]
    FormatVersion { found: u16 },
    #[error("length out of range: {field}={value}")]
    LengthOutOfRange { field: &'static str, value: u64 },
    #[error("crc mismatch: expected {expected:#010x} actual {actual:#010x}")]
    Crc { expected: u32, actual: u32 },
    #[error("fst error: {0}")]
    Fst(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// UI-layer failures (`ui/*` codes).
///
/// The subset that also exists as an `ImeError` variant converts one to one; the
/// remaining codes all mean "the UI channel cannot be used right now" and
/// collapse onto `ImeError::UiChannelClosed`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum UiError {
    /// The UI command channel is gone: the UI thread exited or never started.
    #[error("ui/channel-closed")]
    ChannelClosed,
    /// An event referred to a frame the engine has already replaced.
    #[error("ui/stale-select: revision={got} current={current}")]
    StaleSelect { got: u32, current: u32 },
    /// No usable CJK font was found; text falls back to placeholder glyphs.
    #[error("ui/font/missing-cjk")]
    FontMissingCjk,
    /// A click could not be handed over within the spin budget and was dropped.
    #[error("ui/select/timeout")]
    SelectTimeout,
    /// A frame arrived before the UI thread finished starting up.
    #[error("ui/not-ready")]
    NotReady,
    /// The UI thread died from a panic; further commands are discarded.
    #[error("ui/thread/dead")]
    ThreadDead,
}

/// Decode-layer failures (`decode/*` codes).
///
/// The decode pipeline reports these instead of returning an empty candidate
/// list, so that the caller can decide between degrading and surfacing a
/// diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// The input buffer was empty.
    #[error("decode/empty-input")]
    EmptyInput,
    /// The input exceeds the hard length limit; judged before normalization.
    #[error("decode/too-long: len={len} max={max}")]
    TooLong { len: usize, max: usize },
    /// A character outside the accepted input alphabet was dropped.
    #[error("decode/invalid-char: {ch:?} at {at}")]
    InvalidChar { ch: char, at: usize },
    /// No legal syllable segmentation exists for the input.
    #[error("decode/no-path: {raw}")]
    NoPath { raw: String },
}

/// Configuration failures (`config/*` codes).
///
/// A configuration failure never aborts startup: the loader falls back to the
/// built-in defaults, backs up the offending file and reports through this type.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// A single key failed validation.
    #[error("config/invalid: {key} ({reason})")]
    Invalid { key: String, reason: String },
    /// A section holds more entries than the schema allows; the surplus is ignored.
    #[error("config/limit-exceeded: {section} limit={limit}")]
    LimitExceeded { section: String, limit: usize },
    /// An older configuration document was migrated forward.
    ///
    /// Appended by ADR-0005. This is an *informational* outcome, not a failure --
    /// the migration succeeded and the plugin starts normally -- but it travels
    /// through `ConfigError` because that is the type the loader already reports
    /// through, and a caller that ignores it loses the one record of why the
    /// user's file changed under them. `backup` names the file the original was
    /// moved to, so the diagnostic can point at it without the loader having to
    /// hand back a second value.
    #[error("config/migrated: from={from} to={to} backup={backup}")]
    Migrated { from: u16, to: u16, backup: String },
}

/// Platform-backend failures (`platform/backend/*` codes).
///
/// Returned by `SurfaceBackend`. `NoFreeBuffer` is a normal, expected condition
/// (the compositor still owns the previous buffer): callers skip the frame
/// instead of propagating it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlatformError {
    /// No display backend could be created at all.
    #[error("platform/backend/unavailable")]
    Unavailable,
    /// No draw buffer is free yet; skip this frame, never block.
    #[error("platform/backend/no-free-buffer")]
    NoFreeBuffer,
    /// The display connection was lost, typically because the compositor restarted.
    #[error("platform/backend/disconnected")]
    Disconnected,
}

impl From<DecodeError> for ImeError {
    // An empty input is the degenerate "no segmentation path" case, so it maps
    // onto `DecodeNoPath` with an empty raw string; every other variant has an
    // exact counterpart in the frozen list.
    fn from(err: DecodeError) -> Self {
        match err {
            DecodeError::EmptyInput => ImeError::DecodeNoPath { raw: String::new() },
            DecodeError::TooLong { len, max } => ImeError::DecodeTooLong { len, max },
            DecodeError::InvalidChar { ch, at } => ImeError::DecodeInvalidChar { ch, at },
            DecodeError::NoPath { raw } => ImeError::DecodeNoPath { raw },
        }
    }
}

impl From<ConfigError> for ImeError {
    // `config/limit-exceeded` has no counterpart in the frozen list; it is
    // reported as an invalid key whose reason names the exceeded limit.
    fn from(err: ConfigError) -> Self {
        match err {
            ConfigError::Invalid { key, reason } => ImeError::ConfigInvalid { key, reason },
            ConfigError::LimitExceeded { section, limit } => ImeError::ConfigInvalid {
                key: section,
                reason: format!("limit exceeded: {limit}"),
            },
            // A migration that succeeded keeps its own identity rather than being
            // folded onto `ConfigInvalid`: see `ImeError::ConfigMigrated`.
            ConfigError::Migrated { from, to, backup } => {
                ImeError::ConfigMigrated { from, to, backup }
            }
        }
    }
}

impl From<PlatformError> for ImeError {
    // The frozen list has a single platform-domain variant, so all three backend
    // failures map onto it and the detail string keeps them distinguishable.
    fn from(err: PlatformError) -> Self {
        let detail = match err {
            PlatformError::Unavailable => "platform backend unavailable",
            PlatformError::NoFreeBuffer => "no free draw buffer",
            PlatformError::Disconnected => "display connection lost",
        };
        let detail = String::from(detail);
        ImeError::CompositorUnsupported { detail }
    }
}

impl From<UiError> for ImeError {
    // Three UI codes have no `ImeError` counterpart. All three describe the same
    // situation from the caller's point of view -- the UI channel is unusable --
    // which is exactly what `UiChannelClosed` states.
    fn from(err: UiError) -> Self {
        match err {
            UiError::ChannelClosed => ImeError::UiChannelClosed,
            UiError::StaleSelect { got, current } => ImeError::UiStaleSelect { got, current },
            UiError::FontMissingCjk => ImeError::UiFontMissingCjk,
            UiError::SelectTimeout => ImeError::UiChannelClosed,
            UiError::NotReady => ImeError::UiChannelClosed,
            UiError::ThreadDead => ImeError::UiChannelClosed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ime_error_display_matches_frozen_codes() {
        let dict_path = PathBuf::from("/d/base.dict");
        let unavailable = ImeError::DictUnavailable {
            path: dict_path.clone(),
            cause: DictError::MagicMismatch,
        };
        let corrupt = ImeError::DictCorrupt { path: dict_path };
        let mismatch = ImeError::Fcitx5VersionMismatch {
            host: String::from("5.0.0"),
            required: String::from("5.1"),
        };
        let invalid = ImeError::ConfigInvalid {
            key: String::from("ui.max_per_row"),
            reason: String::from("out of range"),
        };
        let readonly = ImeError::DataReadonly {
            reason: String::from("not writable at all"),
        };
        let unsupported = ImeError::CompositorUnsupported {
            detail: String::from("no overlay protocol"),
        };
        let os = ImeError::UnsupportedOs {
            detail: String::from("freebsd unsupported"),
        };
        let raw = String::from("zzz");
        let no_path = ImeError::DecodeNoPath { raw };
        let stale = ImeError::UiStaleSelect { got: 7, current: 3 };
        let cases = [
            (
                unavailable,
                "dict/unavailable: /d/base.dict (magic mismatch)",
            ),
            (corrupt, "dict/corrupt: /d/base.dict"),
            (
                ImeError::DecodeTooLong { len: 65, max: 64 },
                "decode/too-long: len=65 max=64",
            ),
            (
                ImeError::DecodeInvalidChar { ch: '@', at: 3 },
                "decode/invalid-char: '@' at 3",
            ),
            (no_path, "decode/no-path: zzz"),
            (invalid, "config/invalid: ui.max_per_row (out of range)"),
            (readonly, "data/readonly-mode: not writable at all"),
            (ImeError::UiChannelClosed, "ui/channel-closed"),
            (stale, "ui/stale-select: revision=7 current=3"),
            (ImeError::UiFontMissingCjk, "ui/font/missing-cjk"),
            (
                mismatch,
                "platform/fcitx5/version-mismatch: host=5.0.0 required=5.1",
            ),
            (ImeError::Fcitx5DevMissing, "platform/fcitx5/dev-missing"),
            (
                unsupported,
                "platform/compositor/unsupported: no overlay protocol",
            ),
            (os, "platform/start/unsupported-os: freebsd unsupported"),
            (ImeError::FfiInvalidCommit, "ffi/invalid-commit"),
            (ImeError::Unsupported, "dict/unsupported"),
        ];
        for (err, expected) in cases {
            assert_eq!(err.to_string(), expected);
        }
    }

    #[test]
    fn test_dict_error_display_matches_frozen_codes() {
        let crc = DictError::Crc {
            expected: 0x0000_000A,
            actual: 0x0000_000B,
        };
        let out_of_range = DictError::LengthOutOfRange {
            field: "word_off",
            value: 4_294_967_296,
        };
        let cases = [
            (DictError::MagicMismatch, "magic mismatch"),
            (
                DictError::FormatVersion { found: 2 },
                "unsupported format version 2",
            ),
            (out_of_range, "length out of range: word_off=4294967296"),
            (crc, "crc mismatch: expected 0x0000000a actual 0x0000000b"),
            (
                DictError::Fst(String::from("bad table")),
                "fst error: bad table",
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(err.to_string(), expected);
        }
    }

    #[test]
    fn test_dict_error_io_is_convertible_via_from() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let err = DictError::from(io);
        let expected = "io: gone";
        assert_eq!(err.to_string(), expected);
    }

    #[test]
    fn test_decode_error_into_ime_error_keeps_code_fields() {
        let too_long = ImeError::from(DecodeError::TooLong { len: 65, max: 64 });
        assert!(matches!(too_long, ImeError::DecodeTooLong { .. }));
        let expected = "decode/too-long: len=65 max=64";
        assert_eq!(too_long.to_string(), expected);

        let invalid = ImeError::from(DecodeError::InvalidChar { ch: '@', at: 3 });
        assert!(matches!(invalid, ImeError::DecodeInvalidChar { .. }));
        let expected = "decode/invalid-char: '@' at 3";
        assert_eq!(invalid.to_string(), expected);

        let raw = String::from("qqq");
        let no_path = ImeError::from(DecodeError::NoPath { raw });
        assert!(matches!(no_path, ImeError::DecodeNoPath { .. }));
        let expected = "decode/no-path: qqq";
        assert_eq!(no_path.to_string(), expected);
    }

    #[test]
    fn test_decode_error_empty_input_maps_to_empty_no_path() {
        let converted = ImeError::from(DecodeError::EmptyInput);
        assert!(matches!(converted, ImeError::DecodeNoPath { .. }));
        if let ImeError::DecodeNoPath { raw } = converted {
            assert!(raw.is_empty());
        }
    }

    #[test]
    fn test_config_error_into_ime_error_uses_config_invalid() {
        let invalid = ConfigError::Invalid {
            key: String::from("ui.max_per_row"),
            reason: String::from("out of range"),
        };
        let converted = ImeError::from(invalid);
        assert!(matches!(converted, ImeError::ConfigInvalid { .. }));
        let expected = "config/invalid: ui.max_per_row (out of range)";
        assert_eq!(converted.to_string(), expected);

        let exceeded = ConfigError::LimitExceeded {
            section: String::from("keys"),
            limit: 120,
        };
        let converted = ImeError::from(exceeded);
        assert!(matches!(converted, ImeError::ConfigInvalid { .. }));
        let expected = "config/invalid: keys (limit exceeded: 120)";
        assert_eq!(converted.to_string(), expected);
    }

    #[test]
    fn test_platform_error_into_ime_error_keeps_distinct_details() {
        let cases = [
            PlatformError::Unavailable,
            PlatformError::NoFreeBuffer,
            PlatformError::Disconnected,
        ];
        let mut details = Vec::new();
        for case in cases {
            match ImeError::from(case) {
                ImeError::CompositorUnsupported { detail } => details.push(detail),
                other => details.push(format!("unexpected: {other}")),
            }
        }
        assert_eq!(details.len(), 3);
        let expected = [
            "platform backend unavailable",
            "no free draw buffer",
            "display connection lost",
        ];
        for detail in &details {
            assert!(expected.contains(&detail.as_str()));
        }
    }

    #[test]
    fn test_ui_error_into_ime_error_maps_lifecycle_failures() {
        let stale = ImeError::from(UiError::StaleSelect { got: 9, current: 2 });
        assert!(matches!(stale, ImeError::UiStaleSelect { .. }));
        let expected = "ui/stale-select: revision=9 current=2";
        assert_eq!(stale.to_string(), expected);

        let closed = ImeError::from(UiError::ChannelClosed);
        assert!(matches!(closed, ImeError::UiChannelClosed));

        let font = ImeError::from(UiError::FontMissingCjk);
        assert!(matches!(font, ImeError::UiFontMissingCjk));

        let unusable = [
            UiError::SelectTimeout,
            UiError::NotReady,
            UiError::ThreadDead,
        ];
        for case in unusable {
            let converted = ImeError::from(case);
            assert!(matches!(converted, ImeError::UiChannelClosed));
        }
    }
}
