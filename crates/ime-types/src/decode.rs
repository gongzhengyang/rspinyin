//! The decode request / result contract.
//!
//! This is the payload the engine hands around on the hot path: one
//! `DecodeRequest` goes in, one `DecodeResult` comes out, and the decoder itself
//! stays a pure function that touches no file, clock, environment variable or
//! global state.
//!
//! This module is part of the frozen contract: changing it requires an ADR under
//! `docs/dev/adr/`.

use crate::ui::{Candidate, CandidateSource};

/// Index of one syllable in the frozen 411-entry syllable table.
///
/// The table itself lives in the segmentation layer; the contract only fixes the
/// identifier, so that `Lexicon::fallback_single` can name a syllable without the
/// dictionary crate depending on the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SyllableId(u16);

impl SyllableId {
    /// Wraps a raw index into the syllable table.
    pub const fn new(index: u16) -> Self {
        Self(index)
    }

    /// Returns the raw index into the syllable table.
    pub const fn value(self) -> u16 {
        self.0
    }
}

impl From<SyllableId> for u16 {
    fn from(id: SyllableId) -> Self {
        id.0
    }
}

bitflags::bitflags! {
    /// Per-request decode switches.
    ///
    /// Phase 1 exercises exactly one bit, `USER_DICT`; the fuzzy and abbreviation
    /// bits are part of the frozen namespace and are switched on by the Phase 2
    /// decode extensions. Adding a class means adding a bit here first: the decode
    /// pipeline may not invent flags of its own.
    ///
    /// The three bits from `SHUANGPIN` up were appended by ADR-0005. A bitfield is
    /// forward-compatible by construction -- a reader built before the addition
    /// drops the bit through `from_bits_truncate` and ignores it -- which is why
    /// this extension did not need a version bump the way a struct change does.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct DecodeFlags: u16 {
        /// Master switch for fuzzy syllable matching; the class bits below only
        /// take effect while this bit is set.
        const FUZZY = 1 << 0;
        /// `zh` also matches `z`.
        const FUZZY_ZH_Z = 1 << 1;
        /// `ch` also matches `c`.
        const FUZZY_CH_C = 1 << 2;
        /// `sh` also matches `s`.
        const FUZZY_SH_S = 1 << 3;
        /// `n` also matches `l`.
        const FUZZY_N_L = 1 << 4;
        /// `an` also matches `ang`.
        const FUZZY_AN_ANG = 1 << 5;
        /// `en` also matches `eng`.
        const FUZZY_EN_ENG = 1 << 6;
        /// `in` also matches `ing`.
        const FUZZY_IN_ING = 1 << 7;
        /// `f` also matches `h`.
        const FUZZY_F_H = 1 << 8;
        /// Expand initial-letter abbreviations, so that `nh` reaches a word
        /// spelled `ni'hao`.
        const ABBREV = 1 << 9;
        /// Merge the user's learned words and frequencies into the candidates.
        const USER_DICT = 1 << 10;
        /// The input is a double-pinyin scheme's keystrokes and has to be mapped
        /// back to a full-pinyin spelling before segmentation. The scheme itself
        /// travels in [`DecodeRequest::scheme`]; this bit only says that the
        /// mapping is wanted.
        const SHUANGPIN = 1 << 11;
        /// Merge the user's own phrases into the candidates.
        const PHRASE = 1 << 12;
        /// Convert the committed text to the script selected by `ScriptConfig`.
        const SCRIPT = 1 << 13;
    }
}

impl Default for DecodeFlags {
    /// Phase 1 default: user-dictionary augmentation on, every Phase 2 extension
    /// off.
    fn default() -> Self {
        Self::USER_DICT
    }
}

// Serialized as the raw bit set. Unknown bits are dropped on the way in, which
// keeps a config or snapshot written by a newer build readable here.
#[cfg(feature = "serde")]
impl serde::Serialize for DecodeFlags {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u16(self.bits())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for DecodeFlags {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bits = <u16 as serde::Deserialize<'de>>::deserialize(deserializer)?;
        Ok(Self::from_bits_truncate(bits))
    }
}

/// Which double-pinyin scheme the user's keystrokes follow.
///
/// A newtype rather than a bare `u8` so that a scheme cannot be confused with the
/// several other small integers travelling beside it. `Full` is the identity
/// scheme: the input is already full pinyin, and the decoder maps nothing. It is
/// also the default, so a caller that has never heard of double pinyin keeps the
/// Phase 1 behaviour exactly.
///
/// The numbering is part of the contract -- it is what a configuration file
/// stores -- so a scheme is appended, never renumbered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SchemeId(u8);

impl SchemeId {
    /// Full pinyin: the input is used as typed.
    pub const FULL: Self = Self(0);
    /// Xiaohe (小鹤双拼).
    pub const XIAOHE: Self = Self(1);
    /// Ziranma (自然码).
    pub const ZIRANMA: Self = Self(2);
    /// Microsoft's scheme.
    pub const MICROSOFT: Self = Self(3);
    /// Sogou's scheme.
    pub const SOGOU: Self = Self(4);
    /// Ziguang (紫光).
    pub const ZIGUANG: Self = Self(5);
    /// How many schemes the numbering defines, including [`SchemeId::FULL`].
    pub const COUNT: u8 = 6;

    /// Wraps a raw scheme number.
    ///
    /// Kept `const` and total so that a scheme read out of a configuration file can
    /// be represented even when this build does not know it; the decode layer is
    /// what rejects an unknown one, with `decode/scheme-unsupported`, rather than
    /// this constructor panicking on input the user can type.
    pub const fn from_u8(value: u8) -> Self {
        Self(value)
    }

    /// The raw scheme number, as stored in a configuration file.
    pub const fn value(self) -> u8 {
        self.0
    }

    /// Whether this build implements the scheme.
    ///
    /// A number past [`SchemeId::COUNT`] is one a newer build wrote and this one
    /// cannot honour; the caller reports `decode/scheme-unsupported` and falls back
    /// to full pinyin instead of refusing to start.
    pub const fn is_known(self) -> bool {
        self.0 < Self::COUNT
    }

    /// The identity scheme, used when the caller has no preference.
    pub const fn full() -> Self {
        Self::FULL
    }
}

impl Default for SchemeId {
    /// Full pinyin, so that a [`DecodeRequest`] built without a scheme decodes the
    /// way it did before double-pinyin support existed.
    fn default() -> Self {
        Self::FULL
    }
}

/// One decode request: the normalized ASCII input plus the switch set.
///
/// `raw` is the input buffer as typed (lower-case ASCII letters plus `'` as the
/// forced syllable boundary). Normalization of `v` / `u` after `j`, `q`, `x` and
/// `y`, and the dropping of stray characters, happen inside the segmentation
/// layer, not here.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DecodeRequest {
    /// Normalized ASCII input; never longer than the 64-byte hard limit.
    pub raw: String,
    /// Decode switches for this request.
    pub flags: DecodeFlags,
    /// Which double-pinyin scheme `raw` follows.
    ///
    /// Appended by ADR-0005, which lists this as the one breaking change in the
    /// extension: the struct has no `#[non_exhaustive]`, so every construction site
    /// had to be updated in the same commit. Every site in this workspace uses
    /// [`DecodeRequest::new`], which is why the change was cheap here and is why
    /// the constructors exist at all.
    pub scheme: SchemeId,
}

impl DecodeRequest {
    /// Builds a request for `raw` with the default switch set and full pinyin.
    pub fn new(raw: impl Into<String>) -> Self {
        Self {
            raw: raw.into(),
            flags: DecodeFlags::default(),
            scheme: SchemeId::default(),
        }
    }

    /// Overrides the decode switches.
    pub fn with_flags(mut self, flags: DecodeFlags) -> Self {
        self.flags = flags;
        self
    }

    /// Overrides the double-pinyin scheme.
    pub fn with_scheme(mut self, scheme: SchemeId) -> Self {
        self.scheme = scheme;
        self
    }
}

/// One edge of the winning segmentation.
///
/// The engine fills these in from the best path so that the UI and the
/// diagnostics can show which syllables a candidate consumed without re-running
/// the decoder.
///
/// Not serde-serializable: this is transient per-keystroke state that is never
/// persisted, and it embeds UI types that carry no serialization contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Index of the first syllable covered, inclusive.
    pub start: u16,
    /// One past the last syllable covered, exclusive.
    pub end: u16,
    /// The text this segment contributes to the committed string.
    pub text: String,
    /// Where the segment came from.
    pub source: CandidateSource,
}

/// Everything one decode produces, owned and ready to cross to the UI thread.
///
/// Not serde-serializable, for the same reason as [`Segment`].
#[derive(Clone, Debug, PartialEq)]
pub struct DecodeResult {
    /// Ordered candidates, best first. Never empty: a degraded decode still
    /// returns a single pass-through candidate, because a candidate list that
    /// renders as nothing looks like a hung input method to the user.
    pub candidates: Vec<Candidate>,
    /// Syllable segmentation of the winning path.
    pub segments: Vec<Segment>,
    /// Set when the result is a degraded one: no usable dictionary, or no legal
    /// segmentation path.
    pub degraded: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_flags_default_enables_only_user_dict() {
        let flags = DecodeFlags::default();
        assert!(flags.contains(DecodeFlags::USER_DICT));
        assert!(!flags.contains(DecodeFlags::FUZZY));
        assert!(!flags.contains(DecodeFlags::ABBREV));
    }

    #[test]
    fn test_decode_flags_bit_operations() {
        let combined = DecodeFlags::FUZZY | DecodeFlags::FUZZY_ZH_Z;
        assert!(combined.contains(DecodeFlags::FUZZY));
        assert!(combined.contains(DecodeFlags::FUZZY_ZH_Z));
        assert!(!combined.contains(DecodeFlags::ABBREV));

        let removed = combined & !DecodeFlags::FUZZY_ZH_Z;
        assert!(removed.contains(DecodeFlags::FUZZY));
        assert!(!removed.contains(DecodeFlags::FUZZY_ZH_Z));

        let intersection = DecodeFlags::FUZZY & DecodeFlags::ABBREV;
        assert!(intersection.is_empty());
    }

    #[test]
    fn test_decode_flags_bits_round_trip() {
        let flags = DecodeFlags::USER_DICT | DecodeFlags::ABBREV;
        let bits = flags.bits();
        assert_eq!(bits, (1 << 10) | (1 << 9));
        assert_eq!(DecodeFlags::from_bits(bits), Some(flags));
    }

    #[test]
    fn test_decode_flags_all_covers_every_defined_bit() {
        // Widened from 0x07FF by ADR-0005, which appended SHUANGPIN, PHRASE and
        // SCRIPT. This assertion is the guard that makes an appended bit a
        // deliberate act rather than a silent widening of the namespace.
        assert_eq!(DecodeFlags::all().bits(), 0x3FFF);
    }

    #[test]
    fn test_decode_flags_unknown_bits_are_truncated() {
        let truncated = DecodeFlags::from_bits_truncate(u16::MAX);
        assert!(truncated.contains(DecodeFlags::USER_DICT));
        assert_eq!(truncated.bits(), 0x3FFF);
    }

    #[test]
    fn test_scheme_id_defaults_to_full_pinyin() {
        assert_eq!(SchemeId::default(), SchemeId::FULL);
        assert_eq!(SchemeId::default().value(), 0);
        assert!(SchemeId::default().is_known());
    }

    #[test]
    fn test_scheme_id_round_trips_every_defined_number() {
        for value in 0..SchemeId::COUNT {
            let scheme = SchemeId::from_u8(value);
            assert_eq!(scheme.value(), value);
            assert!(scheme.is_known(), "scheme {value} is inside COUNT");
        }
    }

    #[test]
    fn test_scheme_id_reports_a_number_past_the_table_as_unknown() {
        // A configuration written by a newer build must not panic here: it is
        // representable, and the decode layer is what reports
        // `decode/scheme-unsupported` and falls back to full pinyin.
        let future = SchemeId::from_u8(SchemeId::COUNT);
        assert!(!future.is_known());
        assert_eq!(future.value(), SchemeId::COUNT);
        assert!(!SchemeId::from_u8(u8::MAX).is_known());
    }

    #[test]
    fn test_decode_request_with_scheme_overrides_full_pinyin() {
        let request = DecodeRequest::new("nihao").with_scheme(SchemeId::XIAOHE);
        assert_eq!(request.raw, "nihao");
        assert_eq!(request.scheme, SchemeId::XIAOHE);
        assert_eq!(request.flags, DecodeFlags::default());
    }

    #[test]
    fn test_syllable_id_value_round_trips() {
        let id = SyllableId::new(410);
        assert_eq!(id.value(), 410);
        assert_eq!(u16::from(id), 410);
        assert_eq!(SyllableId::new(u16::MAX).value(), u16::MAX);
    }

    #[test]
    fn test_decode_request_new_uses_default_flags() {
        let request = DecodeRequest::new("nihao");
        assert_eq!(request.raw, "nihao");
        assert_eq!(request.flags, DecodeFlags::default());
    }

    #[test]
    fn test_decode_request_with_flags_overrides_default() {
        let raw = String::from("nihao");
        let request = DecodeRequest::new(raw).with_flags(DecodeFlags::empty());
        assert_eq!(request.raw, "nihao");
        assert!(request.flags.is_empty());
    }

    #[test]
    fn test_segment_records_syllable_span_and_source() {
        let segment = Segment {
            start: 0,
            end: 2,
            text: String::from("ni'hao"),
            source: CandidateSource::Dict,
        };
        assert_eq!(segment.end - segment.start, 2);
        assert_eq!(segment.text, "ni'hao");
        assert_eq!(segment.source, CandidateSource::Dict);
    }

    #[test]
    fn test_decode_result_keeps_degraded_passthrough_candidate() {
        let candidate = Candidate {
            index: 1,
            text: String::from("zzz"),
            annotation: None,
            source: CandidateSource::Passthrough,
            score: 0.0,
            consumed_syllables: 0,
        };
        let result = DecodeResult {
            candidates: vec![candidate],
            segments: Vec::new(),
            degraded: true,
        };
        assert_eq!(result.candidates.len(), 1);
        assert!(result.degraded);
        assert_eq!(result.candidates[0].source, CandidateSource::Passthrough);
    }
}
