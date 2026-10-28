//! Association candidates produced from explicitly injected engine sources.
//!
//! Responsibility: describe one association hit, name where it came from, hold the
//! session's recent-commit ring, and merge source results with a fixed deterministic
//! policy. The merge function is pure: it receives snapshots as slices and never calls
//! a dictionary, clock, filesystem, environment or other process-wide service.
//!
//! Boundary: this module does not wire associations into decoding. A caller gathers
//! Bigram, history and prefix results first, then uses [`merge_assoc`]; absent sources
//! are represented by empty slices and mean that no association candidate is produced.

use std::collections::HashSet;

/// Number of recent commits retained by [`HistorySource::new`].
pub const DEFAULT_HISTORY_DEPTH: usize = 8;

/// The association limits a host projects from configuration.
///
/// This is the engine-side counterpart of the configuration section; it is internal
/// to `ime-core` and deliberately separate from the frozen candidate contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssocLimits {
    /// Whether a successful commit may open an association view.
    pub enabled: bool,
    /// The greatest number of candidates one view may hold, `1..=9`.
    pub max_candidates: u8,
    /// How many recent commits the session history may retain, `0..=64`.
    pub history_depth: u8,
}

impl Default for AssocLimits {
    /// Association is off unless the user's configuration explicitly enables it.
    fn default() -> Self {
        Self {
            enabled: false,
            max_candidates: 5,
            history_depth: u8::try_from(DEFAULT_HISTORY_DEPTH).unwrap_or(u8::MAX),
        }
    }
}

/// A non-blocking, deterministic provider of successors after one committed word.
///
/// # Concurrency
///
/// Implementations must be `Send + Sync`; the method is a bounded, reentrant read
/// and never blocks the host thread or mutates provider state.
/// The explicitly injected Bigram and Prefix providers one session may read.
///
/// A missing reference means that source family is absent, not that its hits are merely
/// empty: the host cannot synthesize a provider and the session cannot discover one.
/// History is deliberately not here because it is session-owned memory.
#[derive(Clone, Copy)]
pub struct AssocProviders<'a> {
    /// The compiled Bigram provider, when the dictionary supplies one.
    pub bigram: Option<&'a dyn AssocProvider>,
    /// The prefix-completion provider, when the engine supplies one.
    pub prefix: Option<&'a dyn AssocProvider>,
}

impl<'a> AssocProviders<'a> {
    /// Builds a provider pair with both source families absent.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub const fn none() -> Self {
        Self {
            bigram: None,
            prefix: None,
        }
    }
}

impl Default for AssocProviders<'_> {
    fn default() -> Self {
        Self::none()
    }
}

/// The per-context association policy and read dependencies handed to one step.
///
/// The sensitivity bit is projected by the host from the same privacy decision that
/// gates learning, so an unreported, password, sensitive or blacklisted context never
/// opens or extends an association view. The providers are read-only and bounded.
#[derive(Clone, Copy)]
pub struct AssocRuntime<'a> {
    /// The Bigram and Prefix providers available to this session.
    pub providers: AssocProviders<'a>,
    /// Whether the active input context suppresses association output and history.
    pub sensitive: bool,
}

impl<'a> AssocRuntime<'a> {
    /// Builds an ordinary runtime whose source families are all absent.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub const fn absent() -> Self {
        Self {
            providers: AssocProviders::none(),
            sensitive: false,
        }
    }
}

impl Default for AssocRuntime<'_> {
    fn default() -> Self {
        Self::absent()
    }
}

pub trait AssocProvider: Send + Sync {
    /// Names the source family this provider serves.
    fn kind(&self) -> AssocSourceKind;

    /// Returns at most `limit` ranked hits after `previous`, highest priority first.
    ///
    /// A limit of zero returns an empty vector. Implementations must not log candidate
    /// or input text.
    fn hits_after(&self, previous: &str, limit: u8) -> Vec<AssocHit>;
}

/// One association candidate.
///
/// `text` is shown to the user, while `commit` is the text inserted if it is selected.
/// Keeping the two fields separate lets a prefix completion show a concise form while
/// inserting its full dictionary word.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct AssocHit {
    /// Candidate text shown in the association window.
    pub text: String,
    /// Text inserted when the candidate is selected.
    pub commit: String,
    /// Source that produced and owns the candidate.
    pub source: AssocSourceKind,
}

impl AssocHit {
    /// Creates a hit without copying either text field.
    pub fn new(
        text: impl Into<String>,
        commit: impl Into<String>,
        source: AssocSourceKind,
    ) -> Self {
        Self {
            text: text.into(),
            commit: commit.into(),
            source,
        }
    }
}

/// The fixed-priority association source families.
///
/// Scores from different families use different units, so the merge never compares
/// them; priority is represented by the variant order and the merge policy alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum AssocSourceKind {
    /// A successor learned from the compiled Bigram table.
    Bigram,
    /// A recently committed word retained only for this session.
    History,
    /// A longer dictionary word extending the previous word as a prefix.
    Prefix,
}

/// A snapshot of association hits in provider ranking order.
///
/// Implementations must return the same order on repeated calls for an unchanged
/// source state. The merge deliberately works on borrowed slices, so producers remain
/// free to gather their data without this layer taking ownership or querying them.
pub trait AssocSource {
    /// Names the source family represented by this snapshot.
    fn kind(&self) -> AssocSourceKind;

    /// Returns the hits in provider ranking order, highest-priority candidate first.
    fn hits(&self) -> &[AssocHit];
}

/// An immutable, explicitly injected hit list for Bigram or Prefix results.
///
/// The slice is borrowed so gathering several sources can reuse caller-owned buffers.
/// It is `Clone` only in the borrowed sense: cloning does not duplicate the hit data.
#[derive(Clone, Copy, Debug)]
pub struct SliceSource<'a> {
    kind: AssocSourceKind,
    hits: &'a [AssocHit],
}

impl<'a> SliceSource<'a> {
    /// Wraps a ranked hit list under an explicit source family.
    pub fn new(kind: AssocSourceKind, hits: &'a [AssocHit]) -> Self {
        Self { kind, hits }
    }
}

impl AssocSource for SliceSource<'_> {
    fn kind(&self) -> AssocSourceKind {
        self.kind
    }

    fn hits(&self) -> &[AssocHit] {
        self.hits
    }
}

/// A bounded in-memory ring of the most recent committed association words.
///
/// The ring is intentionally not persistent and carries no timestamps: insertion order
/// is the only temporal information. Callers choose what text enters it, and this type
/// performs no logging, so recent input cannot reach a diagnostic through this module.
///
/// # Concurrency
///
/// The ring is neither `Sync` nor protected internally. A session owns it on the host
/// thread; callers that need cross-thread transfer should move a snapshot explicitly.
#[derive(Clone, Debug)]
pub struct HistorySource {
    hits: Vec<AssocHit>,
    depth: usize,
}

impl HistorySource {
    /// Creates a ring with the project default depth.
    pub fn new() -> Self {
        Self::with_depth(DEFAULT_HISTORY_DEPTH)
    }

    /// Creates a ring holding at most `depth` hits.
    ///
    /// A depth of zero disables the source and keeps it allocation-free. Larger values
    /// are accepted as given so configuration remains explicit; association quota and
    /// normal usage keep practical depths small.
    pub fn with_depth(depth: usize) -> Self {
        Self {
            hits: Vec::new(),
            depth,
        }
    }

    /// Returns the configured maximum number of retained hits.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Re-bounds the ring, keeping the newest entries that still fit.
    ///
    /// A reload may lower the depth without forgetting the user's session thread; a
    /// raised depth takes effect as later commits arrive.
    pub fn set_depth(&mut self, depth: usize) {
        self.depth = depth;
        if self.hits.len() > depth {
            let keep_from = self.hits.len() - depth;
            self.hits.drain(0..keep_from);
        }
    }

    /// Removes every retained hit, leaving the configured depth unchanged.
    ///
    /// A focus or reset boundary must not leave recent input available to a later
    /// association view, so the caller drops the ring with the ephemeral view.
    pub fn clear(&mut self) {
        self.hits.clear();
    }

    /// Records one committed hit as the newest history entry.
    ///
    /// The source family is assigned here, preventing a caller from accidentally
    /// presenting history data as Bigram or Prefix evidence. When the ring is full the
    /// oldest hit is discarded.
    pub fn push(&mut self, text: impl Into<String>, commit: impl Into<String>) {
        if self.depth == 0 {
            return;
        }
        if self.hits.len() >= self.depth {
            self.hits.remove(0);
        }
        self.hits
            .push(AssocHit::new(text, commit, AssocSourceKind::History));
    }
}

impl Default for HistorySource {
    fn default() -> Self {
        Self::new()
    }
}

impl AssocSource for HistorySource {
    fn kind(&self) -> AssocSourceKind {
        AssocSourceKind::History
    }

    fn hits(&self) -> &[AssocHit] {
        &self.hits
    }
}

/// Merges association hits with fixed family priority and per-source round-robin.
///
/// Source priority is Bigram > History > Prefix. The output visits the sources in that
/// order for each quota slot, preserves provider order within each source, and stops
/// when the quota or every source is exhausted. Deduplication is by `text`; the first
/// occurrence therefore always belongs to the highest-priority source that contains it.
/// Duplicate hits within one source are skipped as well, and empty sources simply have
/// no turn. With no hits at all the result is an empty vector, not a fallback to
/// ordinary decoding.
pub fn merge_assoc(
    quota: u8,
    bigram: &[AssocHit],
    history: &[AssocHit],
    prefix: &[AssocHit],
) -> Vec<AssocHit> {
    let mut selected = Vec::with_capacity(usize::from(quota));
    if quota == 0 {
        return selected;
    }

    let sources = [bigram, history, prefix];
    let mut next = [0usize; 3];
    let mut seen = HashSet::with_capacity(usize::from(quota));
    for _ in 0..usize::from(quota) {
        for (source_index, source) in sources.iter().enumerate() {
            let Some(hit) = source.get(next[source_index]) else {
                continue;
            };
            next[source_index] += 1;
            // A set, rather than an equality walk, keeps duplicate checks bounded even
            // when a caller asks for the full u8 quota from three long provider lists.
            if seen.insert(hit.text.as_str()) {
                let mut owned = hit.clone();
                owned.source = AssocSourceKind::from_index(source_index);
                selected.push(owned);
            }
            if selected.len() == usize::from(quota) {
                return selected;
            }
        }
    }
    selected
}

impl AssocSourceKind {
    /// Maps the merge loop's source position back to its stable source family.
    ///
    /// The input is always 0, 1 or 2 because the merge iterates its own three-slice
    /// array; `Bigram` is the compile-time-safe answer for any unexpected value.
    fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Bigram,
            1 => Self::History,
            _ => Self::Prefix,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a hit whose display and commit text both identify it in assertions.
    fn hit(text: &str, source: AssocSourceKind) -> AssocHit {
        AssocHit::new(text, text, source)
    }

    #[test]
    fn test_merge_assoc_quota_preserves_source_round_robin() {
        let bigram = [
            hit("b1", AssocSourceKind::Bigram),
            hit("b2", AssocSourceKind::Bigram),
        ];
        let history = [
            hit("h1", AssocSourceKind::History),
            hit("h2", AssocSourceKind::History),
        ];
        let prefix = [
            hit("p1", AssocSourceKind::Prefix),
            hit("p2", AssocSourceKind::Prefix),
        ];

        let merged = merge_assoc(4, &bigram, &history, &prefix);

        let texts: Vec<_> = merged
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect();
        assert_eq!(texts, ["b1", "h1", "p1", "b2"]);
        assert_eq!(merged[0].source, AssocSourceKind::Bigram);
        assert_eq!(merged[1].source, AssocSourceKind::History);
        assert_eq!(merged[2].source, AssocSourceKind::Prefix);
    }

    #[test]
    fn test_merge_assoc_repeats_sequence_when_quota_exceeds_first_turn() {
        let bigram = [
            hit("b1", AssocSourceKind::Bigram),
            hit("b2", AssocSourceKind::Bigram),
        ];
        let history = [hit("h1", AssocSourceKind::History)];
        let prefix = [hit("p1", AssocSourceKind::Prefix)];

        let merged = merge_assoc(5, &bigram, &history, &prefix);

        let texts: Vec<_> = merged
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect();
        assert_eq!(texts, ["b1", "h1", "p1", "b2"]);
    }

    #[test]
    fn test_merge_assoc_deduplicates_and_keeps_highest_priority_owner() {
        let bigram = [
            hit("shared", AssocSourceKind::Bigram),
            hit("only-bigram", AssocSourceKind::Bigram),
        ];
        let history = [
            hit("shared", AssocSourceKind::History),
            hit("only-history", AssocSourceKind::History),
        ];
        let prefix = [
            hit("shared", AssocSourceKind::Prefix),
            hit("only-history", AssocSourceKind::Prefix),
            hit("only-prefix", AssocSourceKind::Prefix),
        ];

        let merged = merge_assoc(8, &bigram, &history, &prefix);

        let actual: Vec<_> = merged
            .iter()
            .map(|candidate| (candidate.text.as_str(), candidate.source))
            .collect();
        assert_eq!(
            actual,
            [
                ("shared", AssocSourceKind::Bigram),
                ("only-bigram", AssocSourceKind::Bigram),
                ("only-history", AssocSourceKind::History),
                ("only-prefix", AssocSourceKind::Prefix),
            ]
        );
    }

    #[test]
    fn test_merge_assoc_deduplicates_within_one_provider() {
        let bigram = [
            hit("same", AssocSourceKind::Bigram),
            hit("same", AssocSourceKind::Bigram),
        ];
        let prefix = [hit("other", AssocSourceKind::Prefix)];

        let merged = merge_assoc(3, &bigram, &[], &prefix);

        let texts: Vec<_> = merged
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect();
        assert_eq!(texts, ["same", "other"]);
    }

    #[test]
    fn test_merge_assoc_empty_sources_return_no_candidates() {
        assert!(merge_assoc(8, &[], &[], &[]).is_empty());
    }

    #[test]
    fn test_merge_assoc_zero_quota_returns_no_candidates() {
        let bigram = [hit("first", AssocSourceKind::Bigram)];
        assert!(merge_assoc(0, &bigram, &[], &[]).is_empty());
    }

    #[test]
    fn test_history_source_default_depth_and_ring_eviction() {
        let mut history = HistorySource::new();
        assert_eq!(history.depth(), DEFAULT_HISTORY_DEPTH);

        for ordinal in 0..10 {
            history.push(format!("word-{ordinal}"), format!("commit-{ordinal}"));
        }

        let texts: Vec<_> = history
            .hits()
            .iter()
            .map(|candidate| candidate.text.as_str())
            .collect();
        assert_eq!(
            texts,
            [
                "word-2", "word-3", "word-4", "word-5", "word-6", "word-7", "word-8", "word-9"
            ]
        );
        assert!(
            history
                .hits()
                .iter()
                .all(|hit| hit.source == AssocSourceKind::History)
        );
    }

    #[test]
    fn test_history_source_zero_depth_is_disabled() {
        let mut history = HistorySource::with_depth(0);
        history.push("word", "commit");
        assert_eq!(history.depth(), 0);
        assert!(history.hits().is_empty());
    }

    #[test]
    fn test_assoc_sources_expose_kind_and_ranked_hits() {
        let hits = [hit("candidate", AssocSourceKind::Prefix)];
        let slice = SliceSource::new(AssocSourceKind::Prefix, &hits);
        let mut history = HistorySource::with_depth(1);
        history.push("recent", "recent");

        assert_eq!(slice.kind(), AssocSourceKind::Prefix);
        assert_eq!(slice.hits(), hits);
        assert_eq!(history.kind(), AssocSourceKind::History);
        assert_eq!(
            history.hits().first().map(|hit| hit.text.as_str()),
            Some("recent")
        );
    }
}
