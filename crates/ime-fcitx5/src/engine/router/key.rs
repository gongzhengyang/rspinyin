//! The key path: how [`KeyRouter::key_event`] turns one host key into a
//! claim-or-return decision — the compose step and the routing table ahead of the
//! session, the guards that hand keys back while nothing is composing, and the
//! drain of the user-lexicon writes the step deferred.

use ime_core::assoc::AssocRuntime;
use ime_core::aux::{AuxEntry, AuxGate};
use ime_core::passthrough::{PassthroughDecision, classify};
use ime_core::state::{SessionEnv, SessionEvent, SessionState};
use ime_dict::hot_swap::LexiconSlot;
use ime_dict::symbols::SymbolSlot;
use ime_dict::user_lexicon::{ChainedLexicon, UserLexiconHandle};
use ime_types::{Candidate, KeyAction};

use crate::engine::compose::ComposeStep;
use crate::engine::host::{ComposeOutcome, Host};
use crate::engine::{
    carries_compose_blocking_modifier, claims_key, is_aux_lead_press, is_policy_mark,
    is_shift_press, is_syllable_separator, leaves_temp_english, translate_key,
};
use crate::ffi::{FcitxKeyEvent, emit_diagnostic};

use super::KeyRouter;
use super::STALE_IC_CODE;
use super::effects;
use super::step::StepCtx;

impl<'a> KeyRouter<'a> {
    pub fn key_event(&mut self, ic: u64, key: &FcitxKeyEvent, host: &mut dyn Host) -> bool {
        // The compose step runs before anything claims the key: a dead-key
        // sequence the host is composing is the layout's, and a chord with Ctrl
        // or Super held is the application's shortcut, never a letter to
        // compose through. The step asks only for a press on an idle session --
        // a composing session is never asked, which is what keeps the hot path
        // free of the call -- and with the switch off it is never asked at all,
        // so the default configuration routes every key byte for byte as a
        // build before the step did.
        if self.config.compose
            && !key.is_release
            && !carries_compose_blocking_modifier(key)
            && self.compose_may_ask(ic)
        {
            match host.process_compose(ic, key) {
                ComposeOutcome::Unsupported => {
                    // The pending dead key is the context's own: two input contexts
                    // must never resolve one another's sequences.
                    let step = self
                        .contexts
                        .get_mut(&ic)
                        .map(|context| context.compose_fallback.process(key));
                    match step {
                        Some(ComposeStep::Commit(character, stop)) => {
                            let mut text = [0u8; 4];
                            host.commit(ic, character.encode_utf8(&mut text));
                            if stop {
                                return true;
                            }
                        }
                        // A dead key opening a sequence is swallowed with no output,
                        // exactly what the host's own `Swallowed` answer does.
                        Some(ComposeStep::Swallow) => return true,
                        _ => {}
                    }
                }
                ComposeOutcome::Swallowed => {
                    self.reset_compose_fallback(ic);
                    return true;
                }
                ComposeOutcome::Committed(character) => {
                    self.reset_compose_fallback(ic);
                    let mut text = [0u8; 4];
                    host.commit(ic, character.encode_utf8(&mut text));
                    return true;
                }
            }
        }
        let action = translate_key(key, &self.config.keys);
        if !claims_key(action) || is_shift_press(key) {
            // The auxiliary-code lead key has no row of its own -- a bare grave
            // press is the table's `Ignore` -- so the refinement gate is asked on
            // exactly the keys the walk was about to hand back. A refused gate,
            // and a gate asked under the switch-off configuration, both land on
            // the `return false` below: the key's fate is byte for byte the one
            // it had before the feature existed.
            if is_aux_lead_press(key) && self.config.aux_refinement {
                return self.aux_lead_key(ic, host);
            }
            return false;
        }
        // The dictionary the step decodes against is assembled here, once: the slot's
        // current container, the user's own words locked ahead of it, and the environment
        // over the two. The guard and the bundle are held to the end of the step -- a
        // decode may not wait behind a writer, and the words a lookup hands back point
        // into both -- which is what makes the writes the step collects wait for the
        // drain at the end instead of running inside it.
        let assoc_sensitive = !self.privacy.should_learn(ic);
        let bundle = self.lexicon_slot.map(LexiconSlot::load);
        // The panel's table travels the same atomic road the dictionary does: the
        // Arc this load hands back keeps the table whole to the end of the step.
        let symbols = self.symbol_slot.and_then(SymbolSlot::load);
        let user = self.user_lexicon.map(UserLexiconHandle::lock);
        let chain = match (&user, &bundle) {
            (Some(user), Some(bundle)) => Some(ChainedLexicon::new(user, bundle.lexicon())),
            (Some(user), None) => Some(ChainedLexicon::new(user, self.env.lexicon)),
            _ => None,
        };
        let live_env = SessionEnv {
            decoder: self.env.decoder,
            lexicon: match (&chain, &bundle) {
                (Some(chain), _) => chain,
                (None, Some(bundle)) => bundle.lexicon(),
                (None, None) => self.env.lexicon,
            },
            user_freq: self.env.user_freq,
            lm: match &bundle {
                Some(bundle) => bundle.lm(),
                None => self.env.lm,
            },
            english: self.env.english,
            confusables: self.env.confusables,
            symbols: symbols.as_deref().or(self.env.symbols),
            assoc: self.env.assoc.map(|runtime| AssocRuntime {
                sensitive: assoc_sensitive,
                ..runtime
            }),
        };
        let Self {
            contexts,
            config,
            privacy,
            badge,
            user_lexicon,
            ..
        } = self;
        let mut step = StepCtx {
            env: &live_env,
            config,
            privacy,
            badge,
            reverse: self.reverse_source,
            reverse_absent_reported: &mut self.reverse_absent_reported,
            user_lexicon: *user_lexicon,
            coins: Vec::new(),
            forget_words: Vec::new(),
            pin_changes: Vec::new(),
        };
        let Some(ctx) = contexts.get_mut(&ic) else {
            emit_diagnostic(STALE_IC_CODE);
            return false;
        };
        if ctx.session.temp_english {
            // Temporary English hands every key back to the application, the two that leave
            // the mode included: what ends the mode changes nothing else, so taking the key
            // would take a keystroke the user typed for the application.
            //
            // The session is not stepped here, because the mode's only transition is the one
            // that leaves it and which key leaves it is a fact about the key rather than
            // about the action the table made of it. The space bar and the `Return` key are
            // both `CommitHighlighted` unless the document moves `Return` to `CommitRaw`, so
            // an action that ended the mode ended it on a space bar press. Asking
            // `leaves_temp_english` and taking the mode off through the session keeps the
            // two answers from being able to disagree.
            if leaves_temp_english(key) {
                ctx.session.leave_temp_english();
            }
            return false;
        }
        // The apostrophe pins a syllable boundary inside a composition and is an ordinary
        // character outside one. The session would take it as the first character of a new
        // composition -- the input alphabet accepts it, which is what makes `ni'hao`
        // typeable -- and opening a candidate window on it would take a key the user typed
        // for the application. The table is a function of the key and the modifiers and
        // cannot see the session, so the guard lives here, where the session is.
        if is_syllable_separator(action) && ctx.session.state != SessionState::Composing {
            return false;
        }
        // A text-producing key that lands on an idle session is the passthrough policy's
        // to answer, not the session's: the uppercase letter, the URL keystroke and the
        // bare punctuation mark are decisions about text the user typed, and the decoder
        // must not see them — a character outside the pinyin alphabet would only be
        // swallowed with a diagnostic, which is the defect this answers. While a
        // composition is live the session keeps every key it has always had: the
        // policy's composition half — a mark that carries the pending candidates out —
        // is session-machine work and stays there.
        if let KeyAction::InputChar(ch) = action {
            // While a composition is live a mark belongs to the application, exactly as
            // it did when the table claimed no punctuation: the policy's composition
            // half — a mark that carries the pending candidates out — is session-machine
            // work that has not landed. A letter never satisfies this, so the composing
            // input is untouched. The two marks the configuration names for the
            // character-selection route are the one exception: when the highlight sits
            // on a candidate a character can be taken from they are the route's, and
            // `effects::select_char` answers `None` for a mark that is not a selection,
            // which keeps the old answer here.
            if ctx.session.state != SessionState::Idle && is_policy_mark(ch) {
                // While a refinement view or a reverse panel is open the highlight
                // the user sees is that view's own, so a mark that takes a
                // character would act on a candidate the window may not be showing.
                // The mark keeps the answer every composing mark has -- it travels
                // back to the application -- and the view stays as it is.
                if ctx.aux.is_none() && ctx.reverse.is_none() {
                    if let Some(acted) = effects::select_char::answer(ctx, &mut step, ic, ch, host)
                    {
                        return acted;
                    }
                }
                return false;
            }
            if ctx.session.state == SessionState::Idle {
                // The typed character as the keyboard produced it, read off the raw
                // keysym: the routing table folds a shifted letter before its rows see
                // it, and the uppercase signal is the user's rather than the fold's.
                // The ASCII filter keeps the non-Latin keysym ranges — a frontend's
                // function and modifier keys — out of the policy entirely; they are not
                // `InputChar` actions anyway, so this is the guard that keeps the two
                // tables from being able to disagree.
                let typed = char::from_u32(key.sym).filter(char::is_ascii);
                if let Some(ch) = typed {
                    // The typed character is the whole raw text the policy sees; the
                    // stack buffer keeps the key path allocation-free. The surrounding
                    // text stays `None` because the `Host` boundary carries no accessor
                    // for it yet, so the URL rule sees only the evidence the keystroke
                    // itself provides.
                    let flags = ctx.modes.passthrough_flags(
                        config.auto_english_on_uppercase,
                        config.passthrough_url,
                    );
                    let mut raw = [0u8; 4];
                    match classify(ch.encode_utf8(&mut raw), flags, None) {
                        PassthroughDecision::HostHandles => return false,
                        PassthroughDecision::CommitDirectly(text) => {
                            // The same door a session commit leaves through, minus the
                            // hide: an idle session has no window to take down.
                            // Learning is untouched — typed English is not a candidate
                            // choice.
                            let text = ctx.modes.transform_output(&text, ctx.script.index());
                            host.commit(ic, &text);
                            return true;
                        }
                        // The shortcut is the router's own chord: `classify` never
                        // answers it, so the arm is unreachable, and a key the policy
                        // has no answer for belongs to the session below.
                        PassthroughDecision::EnterTempEnglish | PassthroughDecision::Decode => {}
                    }
                }
            }
        }
        // The mode bits go into the session before the step, so the frame a mode key
        // repaints carries the switch that key just made.
        let mode_acted = match action {
            // The script toggle reads a table the mode bits cannot hold, so its arm is
            // the context's own: flip the bit through the table, or report the
            // degradation and hand the key back.
            KeyAction::ToggleScript => ctx.toggle_script(),
            _ => ctx.modes.apply(action, ctx.ic, host),
        };
        // A refused script toggle is the one mode key that reaches here without acting:
        // the degradation is reported, the bit never moved, and nothing the session
        // could repaint would differ -- so the step is skipped and the key goes back to
        // the application rather than being swallowed by a switch that answered
        // nothing.
        let refused_toggle = action == KeyAction::ToggleScript && !mode_acted;
        let session_acted = if refused_toggle {
            false
        } else {
            ctx.write_frame_context(config);
            ctx.run(SessionEvent::Key(action), &mut step, host)
        };
        // `EnterTempEnglish` is the one action whose whole effect is a session flag: with
        // nothing composing it emits no effect, and the key would be handed back even
        // though the mode changed.
        let acted = mode_acted || session_acted || ctx.session.temp_english;
        // The dictionary lock outlived the step; the writes it deferred are the drain's
        // now, and every borrow they waited behind ends here, oldest first.
        let coins = std::mem::take(&mut step.coins);
        let forget_words = std::mem::take(&mut step.forget_words);
        let pin_changes = std::mem::take(&mut step.pin_changes);
        drop(step);
        drop(user);
        drop(bundle);
        self.apply_user_lexicon_changes(coins, forget_words, pin_changes);
        acted
    }

    /// Answers the auxiliary-code lead key: the five-gate decision, and the
    /// refinement view the `Enter` arm opens.
    ///
    /// Every refused gate hands the key back (`false`) with the diagnostic its
    /// reason owes: the occupied lead key is a configuration conflict the host
    /// sees, the absent column is the once-per-process degradation report, and
    /// the other three are silent because they name ordinary states of the
    /// shipped configuration. The `Enter` arm snapshots the frame the session
    /// has on screen, resolves each candidate's code through the aux source,
    /// and posts the first view frame. A code that cannot be resolved aborts
    /// the open with a diagnostic rather than treating it as a miss, which is
    /// the discipline the `AuxSource` contract names for a container whose
    /// verification lied; the key travels back, the composition untouched.
    ///
    /// # Panics
    ///
    /// Never.
    fn aux_lead_key(&mut self, ic: u64, host: &mut dyn Host) -> bool {
        let source = self.aux_source;
        let lead_free = effects::aux::lead_key_free(&self.config.keys);
        let Some(ctx) = self.contexts.get_mut(&ic) else {
            return false;
        };
        let gate = AuxGate::new(self.config.aux_refinement);
        let count = ctx.session.decoded().candidates.len();
        // The domains that own the input whole: a view or panel already on
        // screen, temporary English, and the symbol panel -- a list whose
        // candidates carry no pinyin reading, so an aux column has nothing to
        // say about them. The reverse-lookup domain owns the window the same
        // way while its panel is up.
        let other_domain = ctx.aux.is_some()
            || ctx.quick.is_some()
            || ctx.reverse.is_some()
            || ctx.session.temp_english
            || ctx.session.is_symbol_mode();
        let column = source.is_some_and(|source| source.has_aux_column());
        match gate.decide(column, count, other_domain, lead_free) {
            AuxEntry::Enter => {}
            AuxEntry::Pass(reason) => {
                effects::aux::report_refusal(reason, ic, &mut self.aux_absent_reported, host);
                return false;
            }
        }
        let Some(source) = source else {
            // Unreachable through the gate -- `column` is false without a
            // source, and `ColumnAbsent` passes -- but a defensive arm, not a
            // second judgment: the open never guesses at codes it cannot read.
            return false;
        };
        let snapshot: Vec<Candidate> = ctx.session.decoded().candidates.clone();
        let mut codes = Vec::with_capacity(snapshot.len());
        for candidate in &snapshot {
            match source.aux_code(&candidate.text) {
                Ok(code) => codes.push(code.map(String::from)),
                Err(_) => {
                    emit_diagnostic(effects::aux::ROW_UNREADABLE_CODE);
                    return false;
                }
            }
        }
        ctx.aux = Some(effects::aux::AuxOverlay::new(
            snapshot,
            codes,
            self.config.session.max_per_row,
        ));
        // The frame context is written for the same reason the ordinary key path
        // writes it: the first view frame should carry the mode bits as they stand.
        ctx.write_frame_context(&self.config);
        effects::aux::post_open_frame(ctx, &self.config, &mut self.badge, host);
        true
    }
}
