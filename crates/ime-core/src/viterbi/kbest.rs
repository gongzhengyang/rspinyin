//! The bounded min-heap the K-best sweep merges paths with.
//!
//! Responsibility: keep the `cap` greatest elements of a stream and hand them
//! back in descending order. That is exactly what one lattice node needs -- the
//! best few paths that reach it -- and nothing else.
//!
//! Boundaries: this module knows nothing about lattices, words or scores. It is a
//! data structure over `T: Ord`, so the ordering, and with it the whole
//! determinism story, stays with the type the decoder puts in it.
//!
//! # Why the storage is borrowed
//!
//! The obvious shape is one `BinaryHeap` per node, which allocates once per node
//! and per decode. The decode path is allocation-sensitive, so a heap here is a
//! view over a slice the caller owns: the decoder keeps one flat buffer for every
//! node's heap and hands each node a disjoint window of it. The length lives in a
//! slot of its own for the same reason -- several edges reach the same node, and
//! each merge has to continue where the previous one stopped.
//!
//! # Ties
//!
//! A bounded heap answers "which of these is the weakest" through `Ord`, so
//! elements that compare equal are interchangeable and which of them survives is
//! unspecified. A caller that needs a reproducible order must make its `Ord`
//! total; the decoder's path state carries a monotone sequence number for exactly
//! that reason.

/// A bounded min-heap over a caller-owned slice.
///
/// Keeps the `cap` greatest elements pushed into it and drops the rest as they
/// arrive, so the work stays proportional to the number of pushes rather than to
/// the size of the stream.
///
/// # Examples
///
/// ```
/// use ime_core::viterbi::TopK;
///
/// let mut slots = [0i32; 4];
/// let mut len = 0usize;
/// let mut top = TopK::new(&mut slots, 2, &mut len);
/// for value in [3, 9, 4, 7] {
///     top.push(value);
/// }
/// top.sort_desc();
/// // Two greatest of the four, best first.
/// assert_eq!(&slots[..len], &[9, 7]);
/// ```
pub struct TopK<'a, T> {
    /// Storage for the heap; only `slots[..*len]` is live.
    slots: &'a mut [T],
    /// Greatest number of elements kept.
    cap: usize,
    /// Length of the heap, kept outside it so that a caller can park it between
    /// two merges into the same window.
    len: &'a mut usize,
}

impl<'a, T> TopK<'a, T> {
    /// Bounds a heap to the `cap` greatest elements of everything pushed into it.
    ///
    /// `slots` is the storage and `len` is the heap's length: pass `0` for an
    /// empty heap, or the length of a valid heap already living in
    /// `slots[..*len]` to continue merging into it. The caller keeps ownership of
    /// both, which is what lets one flat buffer hold the heap of every node.
    ///
    /// # Panics
    ///
    /// Never: a `cap` of zero keeps nothing, a `slots` shorter than `cap` lowers
    /// the capacity to what fits, and a `len` past the capacity is clamped.
    pub fn new(slots: &'a mut [T], cap: usize, len: &'a mut usize) -> Self {
        let cap = cap.min(slots.len());
        *len = (*len).min(cap);
        Self { slots, cap, len }
    }

    /// Returns how many elements the heap currently holds.
    pub fn len(&self) -> usize {
        *self.len
    }

    /// Returns whether the heap holds nothing.
    pub fn is_empty(&self) -> bool {
        *self.len == 0
    }
}

impl<T: Ord> TopK<'_, T> {
    /// Offers one element to the heap, keeping it only when it belongs to the
    /// `cap` greatest seen so far.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn push(&mut self, item: T) {
        let len = *self.len;
        if self.cap == 0 {
            return;
        }
        if len < self.cap {
            self.slots[len] = item;
            *self.len = len + 1;
            self.sift_up(len);
            return;
        }
        // Full: the root is the weakest survivor, so a better element takes its
        // place and sinks to where it belongs. The comparison is strict, so an
        // element that only ties with the weakest survivor is dropped and the
        // survivor stays, which keeps the outcome independent of the order the
        // equal elements arrived in.
        if item > self.slots[0] {
            self.slots[0] = item;
            self.sift_down(0, len);
        }
    }

    /// Reorders the live elements into descending order, in place.
    ///
    /// Nothing is copied out: the sorted elements stay in the caller's slice, so
    /// the caller reads them from the same buffer it lent the heap. The live
    /// length is left unchanged.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn sort_desc(&mut self) {
        let len = *self.len;
        // The root is the smallest survivor, so moving it to the end of the live
        // region and shrinking the heap builds the descending order from the
        // back -- heapsort, over the slice the caller already owns.
        for end in (1..len).rev() {
            self.slots.swap(0, end);
            self.sift_down(0, end);
        }
    }

    /// Moves the element at `at` up until its parent is no greater than it.
    fn sift_up(&mut self, mut at: usize) {
        while at > 0 {
            let parent = (at - 1) / 2;
            if self.slots[at] >= self.slots[parent] {
                return;
            }
            self.slots.swap(at, parent);
            at = parent;
        }
    }

    /// Moves the element at `at` down while one of its children is smaller.
    ///
    /// `heap_len` is the size of the heap inside `slots`, which is the whole live
    /// length during a push and the shrinking live region during a sort.
    fn sift_down(&mut self, mut at: usize, heap_len: usize) {
        loop {
            let left = 2 * at + 1;
            if left >= heap_len {
                return;
            }
            let right = left + 1;
            let child = if right < heap_len && self.slots[right] < self.slots[left] {
                right
            } else {
                left
            };
            if self.slots[child] >= self.slots[at] {
                return;
            }
            self.slots.swap(at, child);
            at = child;
        }
    }
}

/// Fixed per-rank cutoff falloff, in Q8.8.
///
/// The value models librime's `cutoff_threshold * (1 - 1/N)` behaviour with integer
/// arithmetic: each whole-sentence rank gets a floor one step lower than the rank before
/// it, so a clearly weaker tail stops occupying candidate slots.
pub const DECAY_STEP_Q88: i32 = 24;

/// Returns the lowest whole-sentence score that `rank` may retain.
///
/// `rank` is zero-based in candidate order, so the first sentence uses `base` itself.
/// Saturating arithmetic keeps an unusually deep rank or an already-small base from
/// overflowing; a saturated floor is still monotonic in rank.
///
/// # Panics
///
/// Never panics.
pub fn decay_floor(rank: usize, base: i32) -> i32 {
    let rank = i32::try_from(rank).unwrap_or(i32::MAX);
    base.saturating_sub(rank.saturating_mul(DECAY_STEP_Q88))
}

/// Truncates weak whole-sentence candidates by the rank-decay floor.
///
/// The sentence predicate separates whole sentences from word-level candidates. A stable
/// score sort can interleave the two, so decay counts sentence ranks independently and
/// leaves every word candidate in place. Once a sentence misses its floor, the weaker
/// sentence tail is dropped; the retained sentence prefix keeps its original order. The
/// function returns the number of sentences removed.
///
/// The caller supplies scores in Q8.8 and must provide candidates in descending score
/// order; a caller that cannot guarantee that order must not call this helper.
///
/// # Panics
///
/// Never panics: an empty list, an absent sentence prefix and a first-sentence miss are
/// all safe.
pub fn retain_rank_decayed_sentences<T>(
    candidates: &mut Vec<T>,
    is_sentence: impl Fn(&T) -> bool,
    score: impl Fn(&T) -> i32,
) -> usize {
    let original_len = candidates.len();
    let Some((_, base)) = candidates
        .iter()
        .find(|candidate| is_sentence(candidate))
        .map(|candidate| (candidate, score(candidate)))
    else {
        return 0;
    };
    let mut retained_count = 0usize;
    let mut sentence_count = 0usize;
    let mut decay_stopped = false;
    for candidate in candidates.iter() {
        if !is_sentence(candidate) {
            continue;
        }
        let rank = sentence_count;
        sentence_count += 1;
        // Once one sentence misses, the whole weaker sentence tail is dropped even though
        // word candidates interleaved with it remain eligible.
        if !decay_stopped && score(candidate) >= decay_floor(rank, base) {
            retained_count += 1;
        } else {
            decay_stopped = true;
        }
    }

    // Compact in place, preserving every retained candidate's relative position. Sentence
    // ranks are counted independently because word candidates may share the score band.
    let mut write = 0usize;
    let mut seen_sentences = 0usize;
    for read in 0..original_len {
        let retain = !is_sentence(&candidates[read]) || seen_sentences < retained_count;
        if is_sentence(&candidates[read]) {
            seen_sentences += 1;
        }
        if retain {
            candidates.swap(write, read);
            write += 1;
        }
    }
    candidates.truncate(write);
    sentence_count - retained_count
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for a materialized candidate: text carries identity, `score_q88`
    /// carries the rank-decision value, and the flag separates sentence paths.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Item {
        text: &'static str,
        score_q88: i32,
        is_sentence: bool,
    }

    impl Item {
        fn sentence(text: &'static str, score_q88: i32) -> Self {
            Self {
                text,
                score_q88,
                is_sentence: true,
            }
        }

        fn word(text: &'static str, score_q88: i32) -> Self {
            Self {
                text,
                score_q88,
                is_sentence: false,
            }
        }
    }

    fn decay(items: &mut Vec<Item>) -> usize {
        retain_rank_decayed_sentences(
            items,
            |item| item.is_sentence,
            |item| item.score_q88,
        )
    }

    /// Offers `values` to a heap of `cap` slots and returns the survivors, best
    /// first.
    fn keep(values: &[i32], cap: usize) -> Vec<i32> {
        let mut slots = [0i32; 8];
        let mut len = 0usize;
        let mut top = TopK::new(&mut slots, cap, &mut len);
        for value in values {
            top.push(*value);
        }
        top.sort_desc();
        slots[..len].to_vec()
    }

    #[test]
    fn test_topk_keeps_the_greatest_values_in_descending_order() {
        assert_eq!(keep(&[3, 1, 4, 1, 5, 9, 2, 6], 3), vec![9, 6, 5]);
        assert_eq!(keep(&[1, 2, 3], 3), vec![3, 2, 1]);
    }

    #[test]
    fn test_topk_with_a_capacity_above_the_element_count_keeps_everything() {
        assert_eq!(keep(&[2, 7], 5), vec![7, 2]);
        assert_eq!(keep(&[5], 8), vec![5]);
    }

    #[test]
    fn test_topk_with_a_capacity_of_one_keeps_the_single_greatest() {
        assert_eq!(keep(&[4, 9, 2], 1), vec![9]);
        // A stream that never improves on the first element keeps it.
        assert_eq!(keep(&[9, 4, 2], 1), vec![9]);
    }

    #[test]
    fn test_topk_with_equal_values_keeps_exactly_the_capacity() {
        assert_eq!(keep(&[7, 7, 7, 7], 2), vec![7, 7]);
        assert_eq!(keep(&[7, 7], 2), vec![7, 7]);
        // Equal values must not push the survivor out either.
        assert_eq!(keep(&[7, 7, 7], 1), vec![7]);
    }

    #[test]
    fn test_topk_without_elements_keeps_nothing() {
        assert_eq!(keep(&[], 3), Vec::<i32>::new());
    }

    #[test]
    fn test_topk_with_a_capacity_of_zero_keeps_nothing() {
        assert_eq!(keep(&[1, 2, 3], 0), Vec::<i32>::new());
    }

    #[test]
    fn test_topk_survivors_do_not_depend_on_the_push_order() {
        let reference = {
            let mut sorted = keep(&[3, 1, 4, 1, 5, 9, 2, 6], 3);
            sorted.sort_unstable();
            sorted
        };
        for rotated in [
            [1, 4, 1, 5, 9, 2, 6, 3],
            [9, 6, 5, 3, 2, 1, 4, 1],
            [6, 2, 9, 5, 1, 4, 1, 3],
        ] {
            let mut sorted = keep(&rotated, 3);
            sorted.sort_unstable();
            assert_eq!(sorted, reference, "order {rotated:?}");
        }
    }

    #[test]
    fn test_topk_keeps_the_weakest_out_once_it_is_full() {
        let mut slots = [0i32; 2];
        let mut len = 0usize;
        let mut top = TopK::new(&mut slots, 2, &mut len);
        top.push(1);
        top.push(2);
        assert_eq!(top.len(), 2);
        top.push(0);
        assert_eq!(top.len(), 2);
        top.push(2);
        top.sort_desc();
        assert_eq!(&slots[..len], &[2, 2]);
    }

    #[test]
    fn test_topk_resumes_a_heap_that_already_holds_elements() {
        // The decoder parks a node's heap between two edges that reach it, so a
        // resumed heap must continue where the previous merge stopped.
        let mut slots = [0i32; 2];
        let mut len = 0usize;
        {
            let mut top = TopK::new(&mut slots, 2, &mut len);
            top.push(1);
            top.push(2);
        }
        assert_eq!(len, 2);
        {
            let mut top = TopK::new(&mut slots, 2, &mut len);
            top.push(3);
        }
        let mut top = TopK::new(&mut slots, 2, &mut len);
        assert_eq!(top.len(), 2);
        top.sort_desc();
        assert_eq!(&slots[..len], &[3, 2]);
    }

    #[test]
    fn test_topk_new_clamps_a_length_past_the_capacity() {
        let mut slots = [0i32; 2];
        let mut len = 9usize;
        let top = TopK::new(&mut slots, 2, &mut len);
        assert_eq!(top.len(), 2);
        assert!(!top.is_empty());
        // A capacity of zero leaves nothing live, whatever the caller passed in.
        let mut zero = 4usize;
        let empty = TopK::new(&mut slots, 0, &mut zero);
        assert_eq!(empty.len(), 0);
        assert!(empty.is_empty());
    }

    #[test]
    fn test_topk_new_lowers_the_capacity_to_the_storage_it_was_lent() {
        // The decoder lends each node a window of one flat buffer, so a capacity
        // wider than the window is lowered to what the window holds rather than
        // writing past the end of the slice.
        let mut slots = [0i32; 2];
        let mut len = 0usize;
        let mut top = TopK::new(&mut slots, 5, &mut len);
        for value in [1, 2, 3, 4, 5] {
            top.push(value);
        }
        assert_eq!(top.len(), 2, "the capacity is what the storage holds");
        top.sort_desc();
        assert_eq!(&slots[..len], &[5, 4]);
    }

    #[test]
    fn test_rank_decay_floor_is_monotonic_in_rank() {
        assert_eq!(decay_floor(0, 1_000), 1_000);
        assert_eq!(decay_floor(1, 1_000), 976);
        assert_eq!(decay_floor(2, 1_000), 952);
        assert!(decay_floor(1, 1_000) > decay_floor(2, 1_000));
        assert!(decay_floor(usize::MAX, i32::MIN) == i32::MIN);
    }

    #[test]
    fn test_rank_decay_keeps_a_stable_prefix_and_frees_only_the_weak_tail() {
        let original = vec![
            Item::sentence("s0", 1_000),
            Item::sentence("s1", 976),
            Item::sentence("s2", 500),
            Item::sentence("s3", 480),
            Item::word("w0", 470),
            Item::word("w1", 460),
        ];
        let expected = vec![
            original[0], original[1], original[4], original[5],
        ];
        let mut actual = original;
        assert_eq!(decay(&mut actual), 2);
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_rank_decay_retains_a_score_at_its_floor_and_keeps_order_bit_for_bit() {
        let original = vec![
            Item::sentence("best", 900),
            Item::sentence("floor", 876),
            Item::sentence("below", 851),
            Item::word("word", 800),
        ];
        let mut actual = original.clone();
        assert_eq!(decay(&mut actual), 1);
        assert_eq!(
            actual,
            vec![original[0], original[1], original[3]],
            "the retained prefix must be unchanged"
        );
        assert_eq!(actual[0].score_q88.to_le(), original[0].score_q88.to_le());
        assert_eq!(actual[1].score_q88.to_le(), original[1].score_q88.to_le());
    }

    #[test]
    fn test_rank_decay_with_k_one_is_identical_to_the_undecayed_list() {
        let one = vec![Item::sentence("only", i32::MIN + 1)];
        let mut actual = one.clone();
        assert_eq!(decay(&mut actual), 0);
        assert_eq!(actual, one);
    }

    #[test]
    fn test_rank_decay_is_safe_for_empty_and_word_only_lists() {
        let mut empty: Vec<Item> = Vec::new();
        assert_eq!(decay(&mut empty), 0);
        assert!(empty.is_empty());

        let mut words = vec![Item::word("w0", 10), Item::word("w1", 0)];
        assert_eq!(decay(&mut words), 0);
        assert_eq!(words, vec![Item::word("w0", 10), Item::word("w1", 0)]);
    }

    #[test]
    fn test_rank_decay_never_visits_candidates_after_the_sentence_prefix() {
        // Even a very low word cannot open a new decay walk or be removed: the scope is
        // established by the leading contiguous sentence segment, not by score.
        let mut candidates = vec![
            Item::sentence("s0", 100),
            Item::sentence("s1", 10),
            Item::word("w0", -1_000),
        ];
        assert_eq!(decay(&mut candidates), 1);
        assert_eq!(
            candidates,
            vec![Item::sentence("s0", 100), Item::word("w0", -1_000)]
        );
    }
}
