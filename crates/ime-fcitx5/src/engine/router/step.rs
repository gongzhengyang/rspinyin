//! The per-step parameter bundle: everything a session step and its effect
//! executor read besides the session itself, carried as one value so every
//! handler stays inside the parameter budget.

use ime_core::reverse::ReverseSource;
use ime_core::state::SessionEnv;
use ime_dict::user_lexicon::UserLexiconHandle;

use crate::engine::badge::BadgeState;
use crate::privacy_impl::ContextPrivacy;

use super::RoutingConfig;

/// Everything a step and its effect executor read besides the session itself.
///
/// Grouped so that a step, its effects and the commit path that closes them take the
/// session, this context and the host, which keeps every handler inside the parameter
/// budget. The badge state and the reverse-absent flag are the two mutable pieces:
/// the process-local hint bit and the once-per-process degradation flag the frame
/// executor and the reverse domain consume, which the router owns beside its other
/// process state.
pub(super) struct StepCtx<'a, 'b> {
    /// The decoder, the dictionary, the user's frequencies and the language model.
    pub(super) env: &'b SessionEnv<'a>,
    /// The configuration in force for this step.
    pub(super) config: &'b RoutingConfig,
    /// The per-context decisions the learning path asks before it records anything.
    pub(super) privacy: &'b ContextPrivacy,
    /// The process-local state the header's slot is resolved through when a frame is
    /// posted; the `engine::badge` module owns the resolution.
    pub(super) badge: &'b mut BadgeState,
    /// The reverse-lookup view of the active container, when the process assembled
    /// one: the reads a `u`-payload's gate and query resolve through. A step without
    /// one degrades the same way a container without the index does -- the payload
    /// passes through to the ordinary decode and the absence is reported once --
    /// which is the shipped configuration's shape, since the switch itself defaults
    /// off.
    pub(super) reverse: Option<&'a dyn ReverseSource>,
    /// Whether the absent-reverse-index diagnostic is already owed nothing: the code
    /// is a once-per-process report, so the flag lives on the process router beside
    /// the other state that outlives every context, and travels here mutably for the
    /// same reason the badge state does.
    pub(super) reverse_absent_reported: &'b mut bool,
    /// The user's own words, when the process assembled them: the handle the
    /// negative-feedback receipt is executed through. The frozen frequency trait the
    /// environment carries has no penalty call -- the penalty is store state the frozen
    /// contract does not name -- so the store half travels the handle the router was
    /// built with, the same store the environment's frequencies answer from.
    pub(super) user_lexicon: Option<&'a UserLexiconHandle>,
    /// The word pairs a commit coined, waiting for the user-lexicon write the step's
    /// dictionary lock forbids. Collected here, drained by the caller after the step.
    pub(super) coins: Vec<(String, String)>,
    /// The words whose in-memory coin entries a forget must remove -- the store's half
    /// travelled through the frozen trait while the step ran.
    pub(super) forget_words: Vec<String>,
    /// The words whose pin a toggle flipped, for the lexicon's own eviction-protection
    /// flag.
    pub(super) pin_changes: Vec<String>,
}
