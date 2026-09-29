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
    /// Phase 1 exercises exactly one bit, `USER_DICT`; every other bit is part of
    /// the frozen namespace and is switched on by the Phase 2 decode extensions
    /// (fuzzy syllables, initial-letter abbreviations). Adding a class means
    /// adding a bit here first: the decode pipeline may not invent flags of its
    /// own.
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
}

impl DecodeRequest {
    /// Builds a request for `raw` with the default switch set.
    pub fn new(raw: impl Into<String>) -> Self {
        Self {
            raw: raw.into(),
            flags: DecodeFlags::default(),
        }
    }

    /// Overrides the decode switches.
    pub fn with_flags(mut self, flags: DecodeFlags) -> Self {
        self.flags = flags;
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
        assert_eq!(DecodeFlags::all().bits(), 0x07FF);
    }

    #[test]
    fn test_decode_flags_unknown_bits_are_truncated() {
        let truncated = DecodeFlags::from_bits_truncate(u16::MAX);
        assert!(truncated.contains(DecodeFlags::USER_DICT));
        assert_eq!(truncated.bits(), 0x07FF);
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
