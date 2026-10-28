//! The state one input context carries, and the pool a released state parks in.
//!
//! # Responsibility
//!
//! [`Context`] is what the engine holds per input context: the composing session, the
//! mode bits, the anchor and the two shared handles. [`PoolKey`] is what a context's
//! state belongs to -- per input context under the shipped tier, per application under
//! the opt-in `"program"` tier of `[engine] share_state` -- and [`PoolSlot`] is what a
//! released context parks there for its successor.
//!
//! # Boundary
//!
//! Plain state and pure decisions. Nothing here touches the host, a file or a clock:
//! the two constructors build a context from what the router hands them, and the pool
//! key is a function of the tier in force and the privacy layer's identifier.

use ime_config::ShareState;
use ime_core::privacy::AppIdHash;
use ime_core::state::{FrameContext, Session};
use ime_dict::script::ScriptIndex;
use ime_types::Anchor;

use super::RoutingConfig;
use super::effects::quick::QuickPanel;
use super::modes::{Modes, OutputProjection};
use super::phrases::PhraseHandle;

/// The key a context's state is pooled by, from `[engine] share_state`.
///
/// `Context` is the shipped tier and today's behaviour: every input context carries its
/// own state, and a deactivated one ends as a whole. `Program` is the opt-in tier: the
/// state one context of an application leaves behind is handed to the next context of the
/// same application, so the mode bits and the temporary-English switch survive a window
/// switch inside one program. There is deliberately no `All` tier: sharing state across
/// different programs costs more than it buys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum PoolKey {
    /// One state per host input context, whatever application owns it.
    Context(u64),
    /// One state shared by every input context of one application, told apart by the
    /// privacy layer's hash of its identifier -- never by the clear program name, which
    /// is the one layer that is allowed to see it.
    ///
    /// Two different applications collide with probability 2^-64 per pair: the pool
    /// cannot compare clear names to double-check, because a name that crossed this
    /// boundary would end the privacy discipline the hash exists for. The one
    /// identifier that carries no trust at all gets the stronger answer instead:
    /// [`AppIdHash::UNKNOWN`] never pools, so contexts of unreported or unnamed
    /// applications keep state of their own -- the fail-closed reading of "we do not
    /// know what program this is".
    Program(AppIdHash),
}

/// Decides the pool key of an activation from the tier in force and what the host
/// reported about the context.
///
/// The unknown identifier is the boundary case the comment on
/// [`PoolKey::Program`] explains: pooling by "unknown" would make every
/// unreported context share one state, so an unknown application is pooled
/// per context even under the program tier.
pub(super) fn pool_key(tier: ShareState, ic: u64, app_id: AppIdHash) -> PoolKey {
    match tier {
        ShareState::Context => PoolKey::Context(ic),
        ShareState::Program if app_id != AppIdHash::UNKNOWN => PoolKey::Program(app_id),
        ShareState::Program => PoolKey::Context(ic),
    }
}

/// The state one pool slot parks: the session and the mode bits.
///
/// Everything else a [`Context`] holds is either the host's identity for the context
/// (`ic`), state the window owns per window (`anchor`), or a shared handle the router
/// re-points wholesale (`phrases`, `script`) -- none of it is what a window switch must
/// carry across, and none of it parks.
pub(super) struct PoolSlot {
    /// The composing session: the input buffer, the candidate list and the switches the
    /// session owns -- temporary English among them.
    pub(super) session: Session,
    /// The mode bits the engine owns: full width, punctuation width and the script
    /// conversion.
    pub(super) modes: Modes,
}

/// The code a script toggle reports when this build holds no variant table.
///
/// The code is registered for exactly this degradation: the index the conversion
/// reads is absent, the mode bit stays where it was, and the key goes back to the
/// application. Registered in `features.md` 2.2.4 long before this reporter existed;
/// this constant is the reporter.
pub const SCRIPT_UNAVAILABLE_CODE: &str = "ui/script/unavailable";

/// The script half of the output pipeline.
///
/// The table is process-wide and read-only, so the contexts share one handle; the
/// absence is its own state rather than an `Option` because the two answers a toggle
/// needs -- convert through what, or report what -- read differently.
#[derive(Clone)]
pub(super) enum ScriptState {
    /// A variant table to convert committed text through.
    Ready(std::sync::Arc<ScriptIndex>),
    /// No table this build could read: the toggle reports
    /// [`SCRIPT_UNAVAILABLE_CODE`] and leaves the mode bit alone.
    Missing,
}

impl ScriptState {
    /// The table a commit converts through, when there is one.
    pub(super) fn index(&self) -> Option<&ScriptIndex> {
        match self {
            Self::Ready(index) => Some(index),
            Self::Missing => None,
        }
    }
}

/// What the engine holds for one input context.
/// Resolves the client-preedit decision for one input context.
///
/// The user's configuration is an upper bound and the application's capability is
/// the floor: a client that cannot render a preedit area gets the candidate
/// window's header instead, never a broken inline string. A `true` for the
/// capability is also what an application the host reported nothing about gets,
/// because the negotiation only ever narrows what the configuration asks for.
///
/// # Panics
///
/// Never.
pub(super) fn resolve_client_preedit(cfg: bool, ic_has_preedit: bool) -> bool {
    cfg && ic_has_preedit
}

pub(super) struct Context {
    /// The host's stable identity for this context, for the calls that go back to it.
    pub(super) ic: u64,
    /// The pool key this context's state belongs to; see [`PoolKey`]. Written once, at
    /// activation: the tier the context was created under is the tier that decides what
    /// happens to its state when the context ends, whatever the configuration says by
    /// then.
    pub(super) key: PoolKey,
    /// The composing session: the input, the candidates and the window's state.
    pub(super) session: Session,
    /// The mode bits the engine owns; see [`Modes`].
    pub(super) modes: Modes,
    /// Where the host last reported the caret. Written into every frame, so a frame
    /// emitted on a key the host said nothing about still lands in the right place.
    pub(super) anchor: Anchor,
    /// The phrase document a saved phrase is appended to.
    ///
    /// A clone of the router's handle rather than a reference to the router: the effect
    /// that writes a phrase is applied here, where the session's own state is, and the
    /// handle is shared precisely so that both can reach one store.
    pub(super) phrases: PhraseHandle,
    /// The variant table the script mode converts through, or its absence.
    pub(super) script: ScriptState,
    /// Whether the application behind this context can render a client-side
    /// preedit, as the host reported at activation. A context the host said
    /// nothing about holds `true`, which is the behaviour every build before the
    /// negotiation had: the negotiation only ever narrows, never widens.
    pub(super) preedit_supported: bool,
    /// The quick-input panel this context has open, when one is. The panel is a
    /// routing-layer composition of its own — the candidates it shows are rendered,
    /// not decoded — so it lives beside the session rather than inside it, and dies
    /// with the context: nothing parked in a pool slot survives a release.
    pub(super) quick: Option<QuickPanel>,
}

impl Context {
    /// A context whose state starts fresh.
    ///
    /// The session is created from `session_base`, the start of this context's session id
    /// range; the output bits start where the configuration in force projects them; the
    /// script bit starts off in every context, because a toggle the user made is
    /// per-context state rather than a document default.
    pub(super) fn fresh(
        ic: u64,
        key: PoolKey,
        session_base: u64,
        phrases: PhraseHandle,
        script: ScriptState,
        output: OutputProjection,
        preedit_supported: bool,
    ) -> Self {
        Self {
            ic,
            key,
            session: Session::new_with_counter(session_base),
            modes: Modes::from_projection(output),
            phrases,
            script,
            // Until the host reports a cursor rectangle, the window is anchored at the
            // origin of the first screen with no scaling; the cursor work replaces this
            // through `KeyRouter::set_anchor`.
            anchor: FrameContext::default().anchor,
            preedit_supported,
            quick: None,
        }
    }

    /// A context that adopts the state a previous context of the same pool key parked.
    ///
    /// The session and the mode bits travel; the identity, the anchor and the shared
    /// handles are the new context's own, because the window that just took focus is a
    /// different window at a different caret, whatever it shares with its predecessor.
    pub(super) fn adopted(
        ic: u64,
        key: PoolKey,
        slot: PoolSlot,
        phrases: PhraseHandle,
        script: ScriptState,
        preedit_supported: bool,
    ) -> Self {
        Self {
            ic,
            key,
            session: slot.session,
            modes: slot.modes,
            phrases,
            script,
            // The capability is the context's own, not the pool slot's: the window
            // that just took focus is a different application's window, whatever
            // its predecessor shared.
            preedit_supported,
            anchor: FrameContext::default().anchor,
            quick: None,
        }
    }

    /// Flips the script bit, or reports why the flip did not happen.
    ///
    /// With a table the bit flips and the frame the session re-sends carries the new
    /// mode; without one the code the degradation registered is reported and the key
    /// goes back to the application, the same discipline a phrase save without a
    /// document follows.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn toggle_script(&mut self) -> bool {
        match &self.script {
            ScriptState::Ready(_) => {
                self.modes.is_traditional = !self.modes.is_traditional;
                true
            }
            ScriptState::Missing => {
                crate::ffi::emit_diagnostic(SCRIPT_UNAVAILABLE_CODE);
                false
            }
        }
    }

    /// Writes the cursor position and the mode bits into the session.
    ///
    /// Called before every step that may emit a frame, so the frame carries where the
    /// window goes and what its status strip says. The strip written here is the standing
    /// answer — the mode name — because at this point the step has not run and the frame
    /// it will produce does not exist; the header's slot is resolved again when a frame
    /// is posted, where the frame's own candidates and page state are known. See the
    /// `engine::badge` module.
    ///
    /// # Arguments
    ///
    /// * `config` — the configuration in force, whose scheme hint the label shows.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn write_frame_context(&mut self, config: &RoutingConfig) {
        let status = self
            .modes
            .status(self.session.temp_english, config.scheme_hint);
        self.session.set_frame_context(self.anchor, status);
    }
}
