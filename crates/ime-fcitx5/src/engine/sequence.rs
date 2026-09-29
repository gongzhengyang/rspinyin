//! Sequences: a stroke that opens a chord, and the strokes that complete it.
//!
//! # Responsibility
//!
//! [`translate_key`](super::translate_key) answers what one key means, and it answers from
//! that key alone. A sequence is the case one key cannot express: `Ctrl+K` on its own means
//! nothing, and what it means is decided by the key that follows it. [`KeySequence`] holds
//! that half-typed state, and [`SequenceTable`] is the trie of bindings a stroke is matched
//! against.
//!
//! # The answers a stroke can get
//!
//! * It leads on and further strokes are still to come: [`SequenceDecision::Opened`]. The
//!   key is the plugin's from that moment — the caller must keep it — but nothing has been
//!   executed yet.
//! * It ends a sequence: [`SequenceDecision::Completed`], which carries the binding. The
//!   caller keeps the key and executes what the binding says.
//! * It leads nowhere: [`SequenceDecision::Pass`] when nothing was in flight, and
//!   [`SequenceDecision::Abandoned`] when a sequence was. Both hand the key on; the second
//!   also reports the strokes the dropped sequence had already taken.
//!
//! # What the machine never does
//!
//! A stroke that leads nowhere is never consumed. The caller keeps a key only for
//! [`SequenceDecision::Opened`], [`SequenceDecision::Completed`] and
//! [`SequenceDecision::Cancelled`]; the other two answers leave the key travelling, to the
//! context tree and then, if no layer claims it, to the application. That is the property
//! this module is built around: a sequence may take a stroke and wait, but it may never
//! take one it cannot place.
//!
//! # No timer
//!
//! A half-typed sequence is judged when the next key arrives rather than by a timer of its
//! own. The architecture forbids a polling loop — nothing may wake the host thread on its
//! own — and a sequence nobody continues needs nothing done to it: the next key either
//! continues it or finds it gone. The deadline is read off the timestamp the host put on
//! the event ([`KeyEvent::time_ms`]), which is also what makes the machine a pure function
//! of the events it is given: no clock is read here, and no test sleeps.
//!
//! A sequence that runs out of time is dropped without a report of its own. There is no
//! event to attach one to, and the stroke that opened it was already accepted by the host
//! when the sequence took it, so it cannot be replayed either. What the machine still owes
//! the caller is the stroke that arrives late: that one is judged from scratch, so it either
//! opens a sequence of its own or passes.
//!
//! # Ending a sequence from outside
//!
//! A sequence the user has moved on from is ended by the caller as well: the input context
//! losing focus, the host resetting it, a session being torn down. [`KeySequence::reset`]
//! is that path, and it is deliberate rather than a convenience — a sequence left in flight
//! outlives the keystrokes it was typed with, and the next stroke, in whatever context the
//! user moved to, could complete a chord that was started in another application.
//!
//! # The invariant that makes the deadline total
//!
//! [`SequenceTable::bind`] refuses a sequence that is a prefix of one already bound, and
//! one that extends a bound sequence. That refusal is what lets an expired sequence be
//! dropped without losing anything: no binding can be waiting at the node an unfinished
//! sequence reached, so there is never a command to fire late. A table that admitted such a
//! binding would have to choose between running a command the user has moved on from and
//! dropping one they asked for; refusing the binding is the third answer.
//!
//! # Boundary
//!
//! Plain Rust: no host object, no file, no clock, no global state and no `unsafe`. The
//! machine decides *which key* and *whether*; the caller executes what a completed sequence
//! asks for, exactly as it does for a single key.
//!
//! The caller also owns the table: this module neither registers a sequence nor knows what
//! one means, and a caller that has registered none gets [`SequenceDecision::Pass`] for
//! every key — which is what makes the machine invisible until a sequence is bound.

use ime_types::ImeError;

use super::{KEY_ESCAPE, KeyEvent};

#[cfg(test)]
mod tests;

/// How long a half-typed sequence stays open.
///
/// Judged when the next key arrives rather than by a timer, so that no part of the plugin
/// has to wake up on its own. A sequence is open while at most this many milliseconds have
/// passed since the stroke that opened it; the stroke that arrives later than that is
/// judged from scratch, as if the sequence had never been opened.
pub const SEQUENCE_TIMEOUT_MS: u32 = 1_500;

/// The longest sequence the machine can hold open.
///
/// A sequence is a few strokes of muscle memory, and the bound is what keeps the pending
/// state an array rather than a `Vec`: the routing layer runs inside a host callback with a
/// hundred microseconds to spend, and a state that owned a heap buffer would have to
/// allocate and free one per stroke. A longer binding is refused by
/// [`SequenceTable::bind`].
pub const MAX_SEQUENCE_STROKES: usize = 4;

/// The code a binding carries when the table refuses it alongside the ones already bound.
///
/// Stable, like every other code in the project: diagnostics and tests match on it. It
/// travels in the `key` field of the [`ImeError`] [`SequenceTable::bind`] returns, which is
/// where a code with no variant of its own is legible without a call to the process-wide
/// diagnostic channel.
pub const SEQUENCE_CONFLICT_CODE: &str = "keys/sequence-conflict";

/// The code a binding longer than [`MAX_SEQUENCE_STROKES`] carries.
///
/// The same rendering path as [`SEQUENCE_CONFLICT_CODE`]; the two are separate because they
/// ask for different fixes — one is a table that disagrees with itself, the other a table
/// the machine cannot hold.
pub const SEQUENCE_TOO_LONG_CODE: &str = "keys/sequence-too-long";

/// One stroke of a sequence: a keysym and the exact modifier mask it carries.
///
/// The mask is matched exactly rather than as a subset, so a stroke that names `Ctrl+K` is
/// not matched by `Ctrl+Shift+K`. Exact matching is what makes a table readable: what is
/// written down is what the user presses, and no chord can be shadowed by a shorter one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequencePrefix {
    /// XKB keysym of the stroke.
    pub sym: u32,
    /// The modifier mask the stroke must carry exactly.
    pub state: u32,
}

/// The value an unused slot of a pending path holds.
///
/// Keysym `0` is XKB's `NoSymbol`, which no stroke carries, so a slot the machine is not
/// using can never be mistaken for a stroke even if one were read.
const UNUSED_STROKE: SequencePrefix = SequencePrefix { sym: 0, state: 0 };

/// The index of the root node in the table's node arena.
///
/// The root is always the first node pushed, so an index rather than a reference is what
/// lets the walk start somewhere fixed.
const ROOT: usize = 0;

/// The bindings a sequence is matched against.
///
/// A trie keyed by stroke. Every node is either the end of a bound sequence or the start of
/// a longer one and never both — see the module documentation for why that invariant is
/// what makes an expired sequence safe to drop.
///
/// The nodes live in one flat `Vec` addressed by index rather than in a tree of owned
/// children. Navigation is then an index comparison with no borrow to thread through a
/// recursive walk, and every lookup goes through `Vec::get`, so no path through this type
/// can index out of bounds. The table is built once at startup or on a reload; nothing here
/// allocates on the key path.
#[derive(Debug)]
pub struct SequenceTable<T> {
    /// Every node, the root first.
    nodes: Vec<Node<T>>,
    /// How many sequences are bound.
    len: usize,
}

/// One node of the binding trie.
#[derive(Debug)]
struct Node<T> {
    /// The strokes that lead on from here, as `(stroke, node index)`.
    edges: Vec<(SequencePrefix, usize)>,
    /// The binding that ends here, when a sequence does.
    binding: Option<T>,
}

impl<T> Node<T> {
    /// A node that ends no sequence and leads nowhere.
    const fn new() -> Self {
        Self {
            edges: Vec::new(),
            binding: None,
        }
    }
}

impl<T> SequenceTable<T> {
    /// An empty table: every stroke passes.
    ///
    /// # Returns
    ///
    /// A table with no binding in it, which is the state a caller with no sequences
    /// registered is in and the state in which this module changes nothing about how a key
    /// is routed.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new() -> Self {
        Self {
            nodes: vec![Node::new()],
            len: 0,
        }
    }

    /// How many sequences are bound.
    ///
    /// # Returns
    ///
    /// The number of bindings, not the number of nodes: a three-stroke sequence counts
    /// once.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether no sequence is bound at all.
    ///
    /// # Returns
    ///
    /// `true` for a table no [`SequenceTable::bind`] has succeeded against. Such a table
    /// can never open a sequence, so a machine driven by one answers
    /// [`SequenceDecision::Pass`] for every key.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Registers a sequence.
    ///
    /// # Arguments
    ///
    /// * `strokes` — the strokes of the sequence, in the order the user types them. At
    ///   least one and at most [`MAX_SEQUENCE_STROKES`].
    /// * `binding` — what the caller should do when the sequence completes. The table
    ///   neither reads nor executes it; it is handed back through
    ///   [`SequenceDecision::Completed`].
    ///
    /// # Returns
    ///
    /// `Ok(())` once the sequence is registered. A refused binding leaves the table exactly
    /// as it was: the check runs before anything is inserted, so a caller that ignores the
    /// error does not end up with a half-built path.
    ///
    /// # Errors
    ///
    /// [`ImeError::ConfigInvalid`] in two shapes, told apart by the `key` field:
    ///
    /// * [`SEQUENCE_CONFLICT_CODE`] — the sequence is empty, is already bound, is a prefix
    ///   of a bound sequence, or extends one. Any of the four would make one of the two
    ///   bindings unreachable, which is the state the invariant forbids.
    /// * [`SEQUENCE_TOO_LONG_CODE`] — more than [`MAX_SEQUENCE_STROKES`] strokes.
    ///
    /// # Panics
    ///
    /// Never: the walk is bounded by [`MAX_SEQUENCE_STROKES`] and every lookup goes through
    /// `Vec::get`.
    ///
    /// # Examples
    ///
    /// ```
    /// use rspinyin::engine::sequence::{SequencePrefix, SequenceTable};
    ///
    /// let ctrl_k = SequencePrefix { sym: 0x6b, state: 0x04 };
    /// let ctrl_s = SequencePrefix { sym: 0x73, state: 0x04 };
    /// let mut table = SequenceTable::new();
    /// assert!(table.is_empty());
    ///
    /// assert!(table.bind(&[ctrl_k, ctrl_s], "save").is_ok());
    /// assert_eq!(table.len(), 1, "a two-stroke sequence counts once");
    ///
    /// // A second binding that starts the same way would make one of the two unreachable.
    /// assert!(table.bind(&[ctrl_k], "palette").is_err());
    /// assert_eq!(table.len(), 1, "a refused binding changes nothing");
    /// ```
    pub fn bind(&mut self, strokes: &[SequencePrefix], binding: T) -> Result<(), ImeError> {
        self.check(strokes)?;
        self.insert(strokes, binding);
        self.len = self.len.saturating_add(1);
        Ok(())
    }

    /// Whether the binding may be registered, without registering it.
    ///
    /// A read-only pass, so that a refusal cannot leave a path behind in the trie. The two
    /// rules are the two halves of the invariant in the module documentation: no stroke of
    /// the new sequence may land on a node that already ends a sequence, and the node the
    /// new sequence ends at may neither end one nor lead on to another.
    fn check(&self, strokes: &[SequencePrefix]) -> Result<(), ImeError> {
        let Some((last, leading)) = strokes.split_last() else {
            // The empty sequence is a prefix of every sequence, so it is refused for the
            // same reason the explicit conflicts are: it would shadow all of them.
            return Err(conflict("a sequence needs at least one stroke"));
        };
        if strokes.len() > MAX_SEQUENCE_STROKES {
            return Err(too_long(strokes.len()));
        }
        let mut at = ROOT;
        for stroke in leading {
            let Some(next) = self.step(at, *stroke) else {
                // The path leaves the trie here, so nothing below it can conflict.
                return Ok(());
            };
            if self.binding_at(next).is_some() {
                return Err(conflict("a sequence that extends one already bound"));
            }
            at = next;
        }
        let Some(end) = self.step(at, *last) else {
            // Nothing below the end of the path, so nothing there can conflict either.
            return Ok(());
        };
        if self.binding_at(end).is_some() {
            return Err(conflict("a sequence that is already bound"));
        }
        if self.has_edges(end) {
            return Err(conflict("a sequence that is a prefix of one already bound"));
        }
        Ok(())
    }

    /// Adds the path and its binding, assuming [`SequenceTable::check`] accepted them.
    fn insert(&mut self, strokes: &[SequencePrefix], binding: T) {
        let mut at = ROOT;
        for stroke in strokes {
            at = match self.step(at, *stroke) {
                Some(next) => next,
                None => self.push_child(at, *stroke),
            };
        }
        if let Some(node) = self.nodes.get_mut(at) {
            node.binding = Some(binding);
        }
    }

    /// Appends a node and the edge that leads to it, and answers its index.
    fn push_child(&mut self, at: usize, stroke: SequencePrefix) -> usize {
        let next = self.nodes.len();
        self.nodes.push(Node::new());
        if let Some(node) = self.nodes.get_mut(at) {
            node.edges.push((stroke, next));
        }
        next
    }

    /// The node one stroke leads to from `at`.
    fn step(&self, at: usize, stroke: SequencePrefix) -> Option<usize> {
        let node = self.nodes.get(at)?;
        let (_, next) = node.edges.iter().find(|(edge, _)| *edge == stroke)?;
        Some(*next)
    }

    /// The node a whole path leads to.
    ///
    /// `None` when the table no longer holds the path — a table rebuilt under a sequence
    /// that is still in flight — which the caller answers the same way it answers a stroke
    /// that leads nowhere.
    fn walk(&self, strokes: &[SequencePrefix]) -> Option<usize> {
        let mut at = ROOT;
        for stroke in strokes {
            at = self.step(at, *stroke)?;
        }
        Some(at)
    }

    /// The binding that ends at a node, when a sequence ends there.
    fn binding_at(&self, at: usize) -> Option<&T> {
        self.nodes.get(at)?.binding.as_ref()
    }

    /// Whether a node has strokes leading on from it.
    fn has_edges(&self, at: usize) -> bool {
        self.nodes
            .get(at)
            .is_some_and(|node| !node.edges.is_empty())
    }
}

impl<T> Default for SequenceTable<T> {
    /// The same empty table [`SequenceTable::new`] builds.
    fn default() -> Self {
        Self::new()
    }
}

/// Where the sequence machine is.
///
/// Two states and no more, which is the whole point of the machine: a stroke either finds
/// nothing in flight or finds a sequence waiting for the stroke that continues it. The
/// state is exclusive, so it is one value and not a pair of flags that can disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequenceState {
    /// Nothing is half-typed: a stroke either opens a sequence or passes.
    Idle,
    /// A sequence is open and waiting for the stroke that continues it.
    Pending,
}

/// A sequence in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pending {
    /// The strokes typed so far, in order. Only the first `len` are meaningful.
    strokes: [SequencePrefix; MAX_SEQUENCE_STROKES],
    /// How many of `strokes` are meaningful.
    len: u8,
    /// The host timestamp of the stroke that opened the sequence.
    started_at_ms: u32,
}

impl Pending {
    /// A sequence opened by `stroke`.
    const fn opened(stroke: SequencePrefix, at_ms: u32) -> Self {
        let mut strokes = [UNUSED_STROKE; MAX_SEQUENCE_STROKES];
        strokes[0] = stroke;
        Self {
            strokes,
            len: 1,
            started_at_ms: at_ms,
        }
    }

    /// The same sequence with one more stroke at its end.
    ///
    /// The table refuses a binding longer than [`MAX_SEQUENCE_STROKES`], so a node reached
    /// at full depth has no edges and this is never called on a full path. The slot is
    /// looked up rather than indexed anyway, so that the machine has no branch it could
    /// panic on even if a table were built some other way.
    fn pushed(mut self, stroke: SequencePrefix) -> Self {
        if let Some(slot) = self.strokes.get_mut(usize::from(self.len)) {
            *slot = stroke;
            self.len = self.len.saturating_add(1);
        }
        self
    }

    /// The strokes typed so far.
    fn path(&self) -> &[SequencePrefix] {
        let end = usize::from(self.len).min(MAX_SEQUENCE_STROKES);
        self.strokes.get(..end).unwrap_or(&[])
    }

    /// The stroke that opened the sequence.
    const fn prefix(&self) -> SequencePrefix {
        self.strokes[0]
    }

    /// How many strokes the sequence has taken.
    const fn depth(&self) -> u8 {
        self.len
    }

    /// How long the sequence has been open at `now_ms`.
    ///
    /// `wrapping_sub` on purpose. The host's timestamp is a `u32` and wraps every 49 days,
    /// so a wrapped clock reads as a small elapsed time rather than an enormous one. A
    /// clock that moves *backwards* wraps the other way and reads as enormous, and the
    /// sequence is treated as expired — the safe direction, because the alternative is a
    /// sequence that never ends.
    fn elapsed_ms(&self, now_ms: u32) -> u32 {
        now_ms.wrapping_sub(self.started_at_ms)
    }
}

/// What the sequence machine decided about one key.
///
/// Three of the five answers mean the plugin keeps the key, and the two that do not are the
/// two that hand it back. Which is which is not left to the caller to infer: the machine
/// decided it, and [`SequenceDecision::keeps_key`] states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequenceDecision<T> {
    /// Nothing is in flight and the key does not open a sequence. The context tree decides,
    /// and the plugin keeps the key only if one of its layers claims it.
    Pass,
    /// The key opened a sequence, or continued one that has further strokes to come. The
    /// key is the plugin's from that moment — the caller must not hand it back — even
    /// though nothing has been executed.
    Opened,
    /// The key ended a sequence. The caller keeps the key and executes the binding.
    Completed {
        /// What the finished sequence asks the caller to do.
        binding: T,
    },
    /// The key cancelled the sequence in flight with `Escape`. The caller keeps the key and
    /// executes nothing.
    Cancelled,
    /// The key does not lead on from the sequence in flight. The sequence is dropped and
    /// the plugin keeps nothing: the key travels on to the context tree and, if no layer
    /// claims it, to the application.
    ///
    /// The strokes the dropped sequence had already taken are reported rather than silently
    /// forgotten. They cannot be replayed — the host accepted the opening stroke when the
    /// sequence took it — but a caller that accounts for keystrokes needs to know they were
    /// spent here.
    Abandoned {
        /// The stroke that opened the dropped sequence.
        prefix: SequencePrefix,
        /// How many strokes the dropped sequence had taken, the opening stroke included.
        buffered: u8,
    },
}

impl<T> SequenceDecision<T> {
    /// Whether the plugin keeps the key this decision is about.
    ///
    /// # Returns
    ///
    /// `true` for [`SequenceDecision::Opened`], [`SequenceDecision::Completed`] and
    /// [`SequenceDecision::Cancelled`] — the answers that mean the key stops with the
    /// plugin — and `false` for the two that leave it travelling.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn keeps_key(&self) -> bool {
        matches!(
            self,
            Self::Opened | Self::Completed { .. } | Self::Cancelled
        )
    }
}

/// The chord state machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeySequence {
    /// The sequence in flight, if any.
    pending: Option<Pending>,
}

impl KeySequence {
    /// A machine with nothing in flight.
    ///
    /// # Returns
    ///
    /// A machine in [`SequenceState::Idle`], which is the state a freshly started plugin
    /// and a session that has typed nothing are both in.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn new() -> Self {
        Self { pending: None }
    }

    /// Where the machine is.
    ///
    /// # Returns
    ///
    /// [`SequenceState::Pending`] while a sequence is waiting for the stroke that
    /// continues it, [`SequenceState::Idle`] otherwise. A caller that draws a hint while a
    /// leader key is held reads it here rather than tracking the strokes itself.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn state(&self) -> SequenceState {
        if self.pending.is_some() {
            SequenceState::Pending
        } else {
            SequenceState::Idle
        }
    }

    /// The stroke that opened the sequence in flight.
    ///
    /// # Returns
    ///
    /// The first stroke of the sequence waiting for its next one, or `None` when nothing is
    /// in flight. [`KeySequence::state`] answers *whether* a sequence is open; a caller that
    /// draws a hint needs to name the leader key as well, and the name is this stroke.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn leader(&self) -> Option<SequencePrefix> {
        match self.pending {
            Some(pending) => Some(pending.prefix()),
            None => None,
        }
    }

    /// Drops the sequence in flight, if there is one.
    ///
    /// For the events that end a context rather than a stroke: the input context losing
    /// focus, the host resetting it, the session being torn down. A half-typed sequence
    /// belongs to the keystrokes of one context, and one left in flight would let a stroke
    /// typed in the next context complete a chord that was opened in the previous one.
    ///
    /// Nothing is reported and nothing is replayed, which is what tells this apart from the
    /// drop [`KeySequence::offer`] answers with [`SequenceDecision::Abandoned`]: the strokes
    /// the sequence had taken were accepted by the host when the sequence took them, and
    /// there is no stroke arriving now to hand back. The caller reaches for this on a
    /// context change rather than waiting for the deadline to pass, because until it does a
    /// stroke could still complete the sequence.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn reset(&mut self) {
        self.pending = None;
    }

    /// Offers one key to the machine and answers what it decided.
    ///
    /// A prefix answers [`SequenceDecision::Opened`]; the key is the plugin's from that
    /// moment, so the caller must not hand it back even though nothing has happened yet.
    ///
    /// # Arguments
    ///
    /// * `event` — the key as the host delivered it. The timestamp is what the deadline is
    ///   judged against; no clock is read here.
    /// * `table` — the bindings a stroke is matched against.
    ///
    /// # Returns
    ///
    /// The decision, which says both what the machine did with its own state and whether
    /// the caller keeps the key.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never: a pending path is at most [`MAX_SEQUENCE_STROKES`] long and every lookup goes
    /// through `Vec::get`. The guarantee matters because the caller is an FFI entry point,
    /// which must not unwind into C++.
    ///
    /// # Examples
    ///
    /// ```
    /// use rspinyin::engine::KeyEvent;
    /// use rspinyin::engine::sequence::{
    ///     KeySequence, SEQUENCE_TIMEOUT_MS, SequenceDecision, SequencePrefix, SequenceTable,
    /// };
    ///
    /// let ctrl_k = SequencePrefix { sym: 0x6b, state: 0x04 };
    /// let ctrl_s = SequencePrefix { sym: 0x73, state: 0x04 };
    /// let mut table = SequenceTable::new();
    /// assert!(table.bind(&[ctrl_k, ctrl_s], "save").is_ok());
    ///
    /// let press = |sym: u32, time_ms: u32| KeyEvent {
    ///     sym,
    ///     state: 0x04,
    ///     is_release: false,
    ///     time_ms,
    /// };
    ///
    /// let mut sequence = KeySequence::new();
    /// assert_eq!(sequence.offer(&press(0x6b, 0), &table), SequenceDecision::Opened);
    /// assert_eq!(
    ///     sequence.offer(&press(0x73, 40), &table),
    ///     SequenceDecision::Completed { binding: "save" }
    /// );
    ///
    /// // The same two strokes, but the second one arrives after the deadline: the
    /// // sequence is gone by then and the stroke leads nowhere, so it is handed back
    /// // rather than swallowed.
    /// assert_eq!(sequence.offer(&press(0x6b, 0), &table), SequenceDecision::Opened);
    /// assert!(matches!(
    ///     sequence.offer(&press(0x73, SEQUENCE_TIMEOUT_MS + 1), &table),
    ///     SequenceDecision::Pass
    /// ));
    /// ```
    pub fn offer<T: Copy>(
        &mut self,
        event: &KeyEvent,
        table: &SequenceTable<T>,
    ) -> SequenceDecision<T> {
        if self.has_expired(event.time_ms) {
            self.pending = None;
        }
        // A release is never a stroke: the host delivers both edges of every key, and a
        // sequence that took one would eat the application's key-up. A sequence in flight
        // survives it, so that releasing the modifier the prefix was typed with does not
        // cancel what the user is in the middle of.
        if event.is_release {
            return SequenceDecision::Pass;
        }
        let stroke = SequencePrefix {
            sym: event.sym,
            state: event.state,
        };
        match self.pending {
            None => self.open_or_complete(stroke, event.time_ms, table),
            Some(pending) => self.continue_or_abandon(pending, stroke, table),
        }
    }

    /// Whether a sequence in flight has run out of time at `now_ms`.
    fn has_expired(&self, now_ms: u32) -> bool {
        match self.pending {
            Some(pending) => pending.elapsed_ms(now_ms) > SEQUENCE_TIMEOUT_MS,
            None => false,
        }
    }

    /// The answer for a stroke offered with nothing in flight.
    fn open_or_complete<T: Copy>(
        &mut self,
        stroke: SequencePrefix,
        at_ms: u32,
        table: &SequenceTable<T>,
    ) -> SequenceDecision<T> {
        let Some(at) = table.step(ROOT, stroke) else {
            // No sequence starts with this stroke.
            return SequenceDecision::Pass;
        };
        if table.has_edges(at) {
            self.pending = Some(Pending::opened(stroke, at_ms));
            return SequenceDecision::Opened;
        }
        match table.binding_at(at) {
            Some(binding) => SequenceDecision::Completed { binding: *binding },
            // A node that neither ends a sequence nor leads on cannot be built: `bind`
            // refuses a sequence that prefixes another, so every node it creates is one or
            // the other. The answer is written out rather than assumed, because a table
            // that somehow held one must hand the key back rather than swallow it.
            None => SequenceDecision::Pass,
        }
    }

    /// The answer for a stroke offered while a sequence is in flight.
    fn continue_or_abandon<T: Copy>(
        &mut self,
        pending: Pending,
        stroke: SequencePrefix,
        table: &SequenceTable<T>,
    ) -> SequenceDecision<T> {
        let from = table.walk(pending.path());
        let next = from.and_then(|at| table.step(at, stroke));
        match next {
            Some(at) if table.has_edges(at) => {
                self.pending = Some(pending.pushed(stroke));
                SequenceDecision::Opened
            }
            Some(at) => {
                self.pending = None;
                match table.binding_at(at) {
                    Some(binding) => SequenceDecision::Completed { binding: *binding },
                    None => SequenceDecision::Pass,
                }
            }
            None => {
                self.pending = None;
                // `Escape` is the one stroke that leads nowhere and is still the plugin's.
                // The user is telling the sequence to stop, and handing the key on would,
                // in most applications, close something. The table is consulted first, so
                // a table that binds `Escape` as a stroke keeps it.
                if stroke.sym == KEY_ESCAPE {
                    SequenceDecision::Cancelled
                } else {
                    SequenceDecision::Abandoned {
                        prefix: pending.prefix(),
                        buffered: pending.depth(),
                    }
                }
            }
        }
    }
}

/// The error a binding the table refuses alongside the ones already bound comes back as.
///
/// The frozen error model has no variant for a key-binding table, so the failure folds onto
/// [`ImeError::ConfigInvalid`] and the precise code travels in the `key` field, where a
/// diagnostic or a test can match it. Recording the code on the diagnostic channel instead
/// would put process-wide state in a module that is otherwise a pure function, and would
/// leave the caller that has to act on the refusal with nothing to match.
fn conflict(reason: &str) -> ImeError {
    ImeError::ConfigInvalid {
        key: String::from(SEQUENCE_CONFLICT_CODE),
        reason: String::from(reason),
    }
}

/// The error a binding longer than the machine can hold comes back as.
fn too_long(len: usize) -> ImeError {
    ImeError::ConfigInvalid {
        key: String::from(SEQUENCE_TOO_LONG_CODE),
        reason: format!("len={len} max={MAX_SEQUENCE_STROKES}"),
    }
}
