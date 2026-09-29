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

#[cfg(test)]
mod tests {
    use super::*;

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
}
