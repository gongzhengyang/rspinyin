//! The layered key-context bus: which layer of the plugin owns a key.
//!
//! # Responsibility
//!
//! [`translate_key`](super::translate_key) answers what a key *means*, and it answers from
//! the key alone. The host asks a second question the table cannot see: *may I keep this
//! key?* The answer depends on what is live. A `Space` with a composition in flight
//! commits the highlighted candidate and is the plugin's; the same `Space` with nothing
//! composing belongs to the application; a `Space` with a panel open belongs to the panel.
//! [`Dispatcher`] gives that answer, and the claim decision it rests on is
//! [`arbitrate`](super::arbitrate)'s alone.
//!
//! # The tree
//!
//! Four layers, tried highest priority first. The first layer that claims a key ends the
//! walk, so a key a higher layer took never reaches a lower one:
//!
//! * [`KeyContext::ModalOverlay`] — a panel owns the keyboard while it is open.
//! * [`KeyContext::Composition`] — a composition is live: the composing keymap applies.
//! * [`KeyContext::Session`] — a session exists but nothing is composing: the mode
//!   chords, and the two actions an idle session acts on.
//! * [`KeyContext::Host`] — nothing of the plugin's is live: every key is the host's.
//!
//! The layering is what closes the swallowed-key defect the table alone could not see. A
//! `Backspace`, a digit, a `Return`, an `Escape` or a `Space` with nothing composing is
//! declined by the composition layer and again by the session layer, so the walk reaches
//! the host layer and the key travels on to the application.
//!
//! # The claim decision is the arbitrator's
//!
//! A layer does not decide on its own whether a key may be kept: it asks
//! [`arbitrate`](super::arbitrate), which answers from the routing table **and** from what
//! the session would do with the action the table named. Meaning is not permission — the
//! table cannot see that a `Space` with nothing composing has no candidate to commit, nor
//! that a digit names no candidate on the page on show — and the second condition has one
//! home, so a key cannot be kept by one layer and declared useless by another. The bus adds
//! the layering, which of the plugin's modes is live, and nothing else.
//!
//! # Boundary
//!
//! Plain Rust, like the table beside it: no host object, no file, no clock, no global
//! state and no `unsafe`. The event is the bus's own [`KeyEvent`] rather than the
//! `#[repr(C)]` struct the C++ glue fills in, so the bus and its tests stay free of the
//! ABI type; the conversion between the two happens once, at the boundary.
//!
//! The bus decides *which key* and *whether*; it executes nothing. Committing text,
//! filling the preedit area, posting to the candidate window and reporting a diagnostic
//! all go through [`Host`](super::host::Host), and the session is stepped by the caller
//! that owns it, with the action [`Dispatcher::action_for`] hands back.
//!
//! # The three rules ahead of the walk
//!
//! A key release is not a layer's key: the host delivers both edges of every key, and
//! taking one would eat the application's key-up. The one release the plugin watches is the
//! edge that ends a held modifier — it is the other half of the gesture
//! [`ModifierHold`](super::modifier::ModifierHold) tracks — so it is answered by the hold
//! rather than by a layer, and every other release travels on. A modifier press is not the
//! plugin's either, because taking it would take the first half of every capital letter
//! from the application. None of the three is a property of one layer, so all of them live
//! in [`Dispatcher::dispatch`], ahead of the walk.
//!
//! # Temporary English, the one state where the two decisions differ
//!
//! Temporary English hands every key to the application, the `Return` and `Escape` that
//! leave the mode included: what ends the mode changes no other state and produces no
//! effect, so taking the key would take a keystroke the user typed for the application.
//! The walk therefore answers [`Consumed::Ignored`] for every key in that state, and the
//! caller steps the session with the action [`Dispatcher::action_for`] gives it even
//! though the walk declined the key — without that step the mode could never be left.

use ime_core::state::{Session, SessionConfig, SessionState};
use ime_types::KeyAction;

use super::arbiter;
use super::modifier::{HoldOutcome, ModifierHold, ModifierKey};
use super::{KEY_DOWN, KEY_ESCAPE, KEY_RETURN, KEY_UP, KeyBindings, is_shift_press, translate_key};
use crate::ffi::FcitxKeyEvent;

#[cfg(test)]
mod tests;

/// Which layer of the plugin owns a key.
///
/// Ordered by priority: a lower variant wins over a higher one, and the derived `Ord` is
/// what lets the walk be written as an array in priority order rather than as a list
/// somebody has to keep sorted by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeyContext {
    /// A modal overlay owns the keyboard: the cheat sheet or the command palette.
    ModalOverlay,
    /// A composition is live: the full composing keymap applies.
    Composition,
    /// A session exists but nothing is composing.
    Session,
    /// Nothing of the plugin's is live; every key belongs to the host.
    Host,
}

/// What the bus decided about one key.
///
/// The three answers are the three things the host's `keyEvent` can do with an event,
/// which is why the walk stops on two of them: a key that opened a sequence is already
/// ours even though nothing has been committed yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consumed {
    /// The plugin acted on the key: the caller calls `filterAndAccept`.
    Consumed,
    /// The key belongs to the host: the caller leaves the event alone.
    Ignored,
    /// The key opened a sequence and the plugin is waiting for the next one. The key
    /// itself is consumed, but nothing is committed yet.
    ChainPending,
}

/// One key event as the bus sees it.
///
/// The same four values as the `#[repr(C)]` [`FcitxKeyEvent`] the C++ glue fills in, as a
/// value of the bus's own: the bus reads a keysym, a modifier mask and the edge, and
/// nothing about it should depend on the ABI layout. [`KeyEvent::as_host_event`] and the
/// [`From`] conversion are the whole bridge between the two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    /// XKB keysym, passed through from `fcitx::Key::sym()`.
    pub sym: u32,
    /// Fcitx5 modifier bit mask, passed through from `fcitx::Key::states()`.
    pub state: u32,
    /// `true` for the release edge.
    pub is_release: bool,
    /// Host timestamp in milliseconds. The routing table reads nothing from it — the table
    /// is a function of the key and the modifiers — and the one decision that does read it
    /// is the hold machine's long-press test, which has to know how long a modifier was
    /// down.
    pub time_ms: u32,
}

impl KeyEvent {
    /// The event as the routing table and the modifier helpers read it.
    ///
    /// # Returns
    ///
    /// The same key as the `#[repr(C)]` event the host delivers, so that the table's
    /// signature and the host's do not have to change for the bus to exist.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn as_host_event(&self) -> FcitxKeyEvent {
        FcitxKeyEvent {
            sym: self.sym,
            state: self.state,
            is_release: self.is_release,
            time_ms: self.time_ms,
        }
    }
}

impl From<FcitxKeyEvent> for KeyEvent {
    /// Widens a host event into the bus's own; the two carry the same four values.
    fn from(event: FcitxKeyEvent) -> Self {
        Self {
            sym: event.sym,
            state: event.state,
            is_release: event.is_release,
            time_ms: event.time_ms,
        }
    }
}

/// A modal overlay: a panel that owns the keyboard while it is open.
///
/// The three the design names. Which one is open changes nothing about *whether* a key is
/// the overlay's — the panel key domain is the same for all three — so the bus keeps the
/// identity for the layer that draws the panel rather than branching on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    /// The keyboard cheat sheet.
    CheatSheet,
    /// The command palette.
    CommandPalette,
    /// The diagnostics panel.
    Diagnostics,
}

/// What the bus may read about the live session of one input context.
///
/// A view rather than `&Session`: a layer asks one question about the session — whether it
/// would act on the action a key was translated to — and the input buffer, the candidate
/// list and the paging state are read by [`arbitrate`](super::arbitrate), which needs them
/// to answer it. The view is how the session reaches a layer, so that the call which hands
/// the session over is written once here rather than in every layer.
#[derive(Clone, Copy, Debug)]
pub struct SessionView<'a> {
    /// The session of the input context, or `None` when the host delivered a key for a
    /// context it never activated.
    session: Option<&'a Session>,
}

impl<'a> SessionView<'a> {
    /// The view of an input context that has no session.
    ///
    /// # Returns
    ///
    /// A view whose [`SessionView::state`] is `None` and whose counts are zero.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn absent() -> Self {
        Self { session: None }
    }

    /// The view of a live session.
    ///
    /// # Arguments
    ///
    /// * `session` — the session of the input context the key arrived in.
    ///
    /// # Returns
    ///
    /// A view that borrows the session and never outlives it.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn new(session: &'a Session) -> Self {
        Self {
            session: Some(session),
        }
    }

    /// Whether the input context has no session at all.
    ///
    /// # Returns
    ///
    /// `true` for the view [`SessionView::absent`] builds. An input context with no
    /// session and one with an idle session are different things to the walk: only the
    /// second one is a layer.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn is_absent(&self) -> bool {
        self.session.is_none()
    }

    /// Where the session is in its life.
    ///
    /// # Returns
    ///
    /// The state, or `None` when there is no session. `None` is deliberately not `Idle`:
    /// a caller that cannot tell the two apart cannot tell "nothing is composing" from
    /// "there is nothing here at all".
    ///
    /// # Panics
    ///
    /// Never.
    pub fn state(&self) -> Option<SessionState> {
        self.session.map(|session| session.state)
    }

    /// Whether temporary English is on, which hands every key to the application.
    ///
    /// # Returns
    ///
    /// The flag, and `false` when there is no session: a context that does not exist is
    /// not in temporary English.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn temp_english(&self) -> bool {
        self.session.is_some_and(|session| session.temp_english)
    }

    /// How many candidates the last decode produced.
    ///
    /// # Returns
    ///
    /// The length of the candidate list, and zero when there is no session. A layer that has
    /// to decide whether a digit names a candidate asks [`SessionView::arbitrate`] rather
    /// than comparing this against the digit: the paging state is what says which candidate
    /// a digit names and which page is on show, and a count cannot answer either.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn candidate_count(&self) -> usize {
        self.session
            .map_or(0, |session| session.decoded().candidates.len())
    }

    /// Whether the plugin may keep the key this action came from.
    ///
    /// The routing table's half of the question is the caller's — it holds the key — and
    /// this is the session's half and the whole of the answer: the one place in this crate
    /// that decides whether an action is worth keeping a key for is asked here.
    ///
    /// # Arguments
    ///
    /// * `action` — what the routing table made of the key, from
    ///   [`translate_key`](super::translate_key).
    /// * `cfg` — the configuration in force, which the input length limit a typed character
    ///   is measured against is read from.
    ///
    /// # Returns
    ///
    /// [`Consumed::Consumed`] when the table names the action and the session would act on
    /// it, and [`Consumed::Ignored`] otherwise — a view with no session behind it included.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn arbitrate(&self, action: KeyAction, cfg: &SessionConfig) -> Consumed {
        arbiter::arbitrate(action, self.session, cfg)
    }
}

/// The state the key walk itself needs.
///
/// Small on purpose. The two trackers that run ahead of the walk — the modifier a user
/// holds and the sequence a key opened — are structures of their own, so that the tree,
/// the hold and the sequence can each be built and verified without the other two. The
/// configuration is held rather than handed to every call, because the walk reads it on
/// every key and a reload adopts it in place.
#[derive(Debug)]
pub struct Dispatcher {
    /// The `[keys]` settings the composition and session layers branch on.
    bindings: KeyBindings,
    /// The session values the arbitration measures a key against: the state the session is
    /// in, and the input length limit a typed character is compared with. The `[keys]` rows
    /// say what a key means; these say whether anything can act on it.
    session: SessionConfig,
    /// The overlay that owns the keyboard, if one is open.
    overlay: Option<Overlay>,
    /// The modifier the user is holding, if any.
    hold: ModifierHold,
    /// What the last modifier release meant, waiting for the caller to take it. The walk
    /// answers a release with a single [`Consumed`], so what the release *meant* needs a
    /// place of its own to travel to the layer that can act on it.
    pending_hold: Option<HoldOutcome>,
}

impl Dispatcher {
    /// Builds a dispatcher with no overlay open and no modifier held.
    ///
    /// # Arguments
    ///
    /// * `bindings` — the `[keys]` settings the composition and session layers branch on.
    ///
    /// # Returns
    ///
    /// A dispatcher whose walk answers exactly what the configuration in force says. The
    /// session values are the shipped defaults; a caller that has the user's document hands
    /// them over with [`Dispatcher::set_session_config`].
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new(bindings: KeyBindings) -> Self {
        Self {
            bindings,
            session: SessionConfig::default(),
            overlay: None,
            hold: ModifierHold::new(),
            pending_hold: None,
        }
    }

    /// Adopts reloaded `[keys]` settings.
    ///
    /// The overlay and everything else the bus holds survive the change: a reload must
    /// not disturb what the user is in the middle of (0.4 rule 10).
    ///
    /// # Arguments
    ///
    /// * `bindings` — the settings that replace the ones in force.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn set_bindings(&mut self, bindings: KeyBindings) {
        self.bindings = bindings;
    }

    /// Adopts the session values a reloaded configuration declares.
    ///
    /// The arbitration measures a typed character against the input length limit these
    /// values carry, so a reload that changes it changes which keys the plugin keeps — and
    /// nothing else about the bus: an open overlay stays open and a composition in flight
    /// is untouched (0.4 rule 10).
    ///
    /// # Arguments
    ///
    /// * `session` — the values that replace the ones in force.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn set_session_config(&mut self, session: SessionConfig) {
        self.session = session;
    }

    /// The overlay that owns the keyboard, if one is open.
    ///
    /// # Returns
    ///
    /// The open overlay, or `None` when the keyboard belongs to the layers below.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn overlay(&self) -> Option<Overlay> {
        self.overlay
    }

    /// Gives the keyboard to a modal overlay.
    ///
    /// The layer that draws the panel is the one that opens it, and it is also the one
    /// that executes what the overlay layer decides: the bus only answers whether a key is
    /// the panel's.
    ///
    /// # Arguments
    ///
    /// * `overlay` — the panel that is now open.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn open_overlay(&mut self, overlay: Overlay) {
        self.overlay = Some(overlay);
    }

    /// Takes the keyboard back from the overlay, if one is open.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn close_overlay(&mut self) {
        self.overlay = None;
    }

    /// Records a modifier press, so that its release can be answered.
    ///
    /// The caller observes the host's input-method state and passes it in: the bus reads no
    /// host object, and whether the input method is enabled is a fact about the host rather
    /// than about the session. Arming changes nothing about the key itself — the press is
    /// still the application's, and the walk still answers it with [`Consumed::Ignored`].
    ///
    /// # Arguments
    ///
    /// * `event` — the key as the host delivered it.
    /// * `was_enabled` — whether the host has the input method enabled for this context.
    ///
    /// # Returns
    ///
    /// Whether the event was a modifier press and the hold now tracks it. Every other
    /// event — a release, a letter, any key that names no modifier — answers `false` and
    /// changes nothing, so a caller may hand every event to this without checking first.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn arm_hold(&mut self, event: &KeyEvent, was_enabled: bool) -> bool {
        if event.is_release {
            return false;
        }
        let Some(key) = ModifierKey::from_sym(event.sym) else {
            return false;
        };
        self.hold.arm(key, event.time_ms, was_enabled);
        true
    }

    /// Takes what the last modifier release meant, if it meant anything.
    ///
    /// # Returns
    ///
    /// The outcome of the release the walk answered [`Consumed::Consumed`] for, and `None`
    /// when no such release has happened since the last call. The caller is the layer that
    /// can act on it — putting the host's input state back, opening the cheat sheet — and
    /// taking it rather than reading it is what keeps one release from being acted on
    /// twice.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn take_hold_outcome(&mut self) -> Option<HoldOutcome> {
        self.pending_hold.take()
    }

    /// Forgets the modifier hold and anything waiting on it.
    ///
    /// For a hold whose release will never arrive, because the input context lost focus or
    /// the host reset it. The next modifier press then starts a new hold rather than
    /// continuing one whose other end is gone, and no stale outcome is left for a caller
    /// that will never see the release it belongs to.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn clear_hold(&mut self) {
        self.hold.clear();
        self.pending_hold = None;
    }

    /// Dispatches one key through the context tree, highest priority first.
    ///
    /// # Arguments
    ///
    /// * `event` — the key as the host delivered it.
    /// * `session` — what the bus may read about the session of the input context the
    ///   key arrived in, or [`SessionView::absent`] for a context that has none.
    ///
    /// # Returns
    ///
    /// [`Consumed::Consumed`] or [`Consumed::ChainPending`] when a layer claimed the key
    /// — the two answers that mean the caller keeps it — and [`Consumed::Ignored`] when
    /// every layer declined it and the key belongs to the application. One release can be
    /// claimed as well: the edge that ends a modifier the hold is tracking, whose meaning
    /// the caller takes from [`Dispatcher::take_hold_outcome`].
    ///
    /// A layer that reads the session claims a key only when the arbitration says the
    /// session would act on the action the table gave the key, so a routed key nothing can
    /// act on is handed back rather than eaten; see the module documentation.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never: every layer answers for every key, and the walk has no branch it cannot
    /// take. The guarantee matters because the caller is an FFI entry point, which must
    /// not unwind into C++.
    pub fn dispatch(&mut self, event: &KeyEvent, session: &SessionView<'_>) -> Consumed {
        // The host delivers both edges of every key, and taking a release would eat the
        // application's key-up. The one exception is the release that ends a modifier the
        // hold is tracking: the gesture is a press and a release, so the machine needs
        // both, and it answers for every other release with "not mine".
        if event.is_release {
            return self.release_held_modifier(event);
        }
        // A modifier press is not ours either. The table names Shift as the temporary
        // Chinese / English switch, but Fcitx5 delivers the modifier to the application
        // and implements that behaviour itself; an input method that took the press would
        // take the first half of every capital letter.
        if is_shift_press(&event.as_host_event()) {
            return Consumed::Ignored;
        }
        for context in self.active_contexts() {
            match self.dispatch_in(context, event, session) {
                Consumed::Ignored => continue,
                decided => {
                    // A key the plugin acted on while a modifier was down is a key the
                    // user typed with that modifier, not a mode switch it was held for.
                    // Only a claimed key marks the hold: a key that fell through every
                    // layer changed nothing and must leave a long press intact.
                    self.hold.mark_used();
                    return decided;
                }
            }
        }
        Consumed::Ignored
    }

    /// The release edge of the modifier the hold is tracking.
    ///
    /// The one release the walk answers, and it is not a layer's key: the hold decides, and
    /// it answers [`HoldOutcome::Nothing`] for every release but the one that ends its own
    /// gesture. An outcome worth acting on is kept for the caller, because the walk can
    /// only report that the key was kept and not what keeping it means.
    fn release_held_modifier(&mut self, event: &KeyEvent) -> Consumed {
        match self
            .hold
            .release(ModifierKey::from_sym(event.sym), event.time_ms)
        {
            HoldOutcome::Nothing => Consumed::Ignored,
            outcome => {
                self.pending_hold = Some(outcome);
                Consumed::Consumed
            }
        }
    }

    /// The action the routing table gives this event.
    ///
    /// Exposed rather than kept private because the caller steps the session with the
    /// action the walk decided on, and both read it from the same pure function: one
    /// translation, called twice, is what keeps the two from drifting apart.
    ///
    /// # Arguments
    ///
    /// * `event` — the key as the host delivered it.
    ///
    /// # Returns
    ///
    /// The action, or [`KeyAction::Ignore`] for a key the table does not name and for
    /// every release.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn action_for(&self, event: &KeyEvent) -> KeyAction {
        translate_key(&event.as_host_event(), &self.bindings)
    }

    /// The layers to try, highest priority first.
    ///
    /// Three entries rather than four. Three layers decide, and the fourth — the host
    /// layer — is the answer the walk falls through to, so it only needs a slot when
    /// nothing else can claim the key, which is exactly when no overlay is open. A fixed
    /// array rather than a `Vec`: this runs inside a host callback with a hundred
    /// microseconds to spend.
    fn active_contexts(&self) -> [KeyContext; 3] {
        if self.overlay.is_some() {
            [
                KeyContext::ModalOverlay,
                KeyContext::Composition,
                KeyContext::Session,
            ]
        } else {
            [
                KeyContext::Composition,
                KeyContext::Session,
                KeyContext::Host,
            ]
        }
    }

    /// One layer's answer for one key.
    ///
    /// Every layer answers for every key: `Ignored` is a decision and not a missing case,
    /// which is what makes the walk total.
    fn dispatch_in(
        &mut self,
        context: KeyContext,
        event: &KeyEvent,
        session: &SessionView<'_>,
    ) -> Consumed {
        match context {
            KeyContext::ModalOverlay => self.dispatch_in_overlay(event),
            KeyContext::Composition => self.dispatch_in_composition(event, session),
            KeyContext::Session => self.dispatch_in_session(event, session),
            // The host layer consumes nothing. It is written as a layer so that the tree
            // has a row for the case rather than an implicit fall-through.
            KeyContext::Host => Consumed::Ignored,
        }
    }

    /// The overlay layer: the keys a panel owns while it is open.
    ///
    /// The panel's domain is the four keys the design gives it — `Escape` closes it, the
    /// arrows move inside it and `Return` confirms it — and every other key falls through
    /// to the layers below, because a panel is an overlay and not a keyboard grab. What
    /// the four keys *do* inside the panel is the panel's own state machine; this layer
    /// only answers that they are not the application's.
    fn dispatch_in_overlay(&mut self, event: &KeyEvent) -> Consumed {
        if self.overlay.is_none() {
            return Consumed::Ignored;
        }
        if event.sym == KEY_ESCAPE {
            self.close_overlay();
            return Consumed::Consumed;
        }
        if matches!(event.sym, KEY_UP | KEY_DOWN | KEY_RETURN) {
            return Consumed::Consumed;
        }
        Consumed::Ignored
    }

    /// The composition layer: the composing keymap, which is the routing table.
    ///
    /// The layer is entered by a live composition alone. Temporary English suspends it,
    /// because the mode hands the keyboard to the application. Inside it the answer is the
    /// arbitrator's: a key is the plugin's when the table names it **and** the session would
    /// act on the action it was translated to, and every other key falls through to the
    /// layers below. A live composition is therefore not a keyboard grab — a digit past the
    /// end of the page, a page key with no page to turn to and an arrow pointing off the end
    /// of the input all reach the application.
    fn dispatch_in_composition(&self, event: &KeyEvent, session: &SessionView<'_>) -> Consumed {
        if session.state() != Some(SessionState::Composing) || session.temp_english() {
            return Consumed::Ignored;
        }
        session.arbitrate(self.action_for(event), &self.session)
    }

    /// The session layer: the keys that are the plugin's with nothing composing.
    ///
    /// The layer exists because the table answers what a key *means* and cannot answer
    /// whether anything is there to act on it. Its whole answer is the arbitration, so it
    /// holds no rule of its own: the mode chords the engine owns, the letter an idle session
    /// starts a composition with, and the `Space`, digits, `Return`, `BackSpace`, `Escape`
    /// and arrows an idle session has nothing to do with are rows of that one answer. With
    /// nothing composing the layer is what the walk reaches for every key of the composing
    /// keymap, and it is what hands them all back.
    fn dispatch_in_session(&self, event: &KeyEvent, session: &SessionView<'_>) -> Consumed {
        session.arbitrate(self.action_for(event), &self.session)
    }
}
