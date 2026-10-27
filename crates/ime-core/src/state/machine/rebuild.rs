//! The half of a session that reacts to change: re-segmentation, re-decode, and the
//! frame the graph derives.
//!
//! Split out of `machine.rs` to keep that file inside the line limit. [`Session::refresh`]
//! is the spine -- the input changed underneath, so the request is rewritten through the
//! active layout, decoded again, and every derived surface (preedit, paging, highlight,
//! the window's frame) is rebuilt from the new graph rather than patched in place. The
//! transitions in [`crate::state::transitions`] call the emit half once their own edits
//! are done, which is why the methods here are visible to the whole `state` module and
//! to nothing wider.

use super::*;

impl Session {
    /// Re-segments and re-decodes the current input, then repairs everything derived.
    ///
    /// The candidate text the highlight is on is read before the decode and handed to
    /// [`Paging::reconcile`], so a keystroke moves the highlight with the word the
    /// user had chosen rather than resetting it to the first candidate.
    ///
    /// That text is read as a borrow rather than copied: the copy this used to take
    /// existed only to outlive the decode below, which needs the workspace mutably, and
    /// the borrow ends at the `reconcile` call. The list is parked in
    /// [`Session::previous`] for the length of the decode, and the buffers of the answer
    /// before it go back into the workspace, so that the decode writes into storage that
    /// is already sized. The two fields are named directly instead of through
    /// [`Session::highlighted_candidate`], because that accessor borrows the whole
    /// session and the mutable borrow `reconcile` takes of the paging state would
    /// collide with it.
    ///
    /// A double-pinyin layout is applied here, before segmentation: the request carries
    /// the layout's full-pinyin spelling of the keystrokes rather than the keystrokes
    /// themselves, which is what leaves the graph, the lattice, the Viterbi pass and
    /// the preedit builder untouched. The buffer keeps what the user typed, and its
    /// syllable grid is written from the rewrite's own alignment, so Backspace and the
    /// caret stay on the units the user pressed.
    pub(in crate::state) fn refresh(&mut self, env: &SessionEnv<'_>) {
        let parked = core::mem::replace(&mut self.previous, self.scratch.take_result());
        self.scratch.recycle(parked);
        self.request.raw.clear();
        self.request.raw.push_str(self.buf.raw());
        // The v-mode gate runs before the rewrite, because the panel serves what
        // the user typed, not what a layout would have spelled: a raw that names
        // a category of the symbol table -- or the bare `v`, the panel's entry --
        // is answered from the table, and every other input decodes as before.
        if let Some(table) = env.symbols {
            if self.symbol_table_serves(table) {
                self.refresh_symbols(table);
                return;
            }
        }
        self.symbol_mode = false;
        let rewritten = self.rewrite_input();
        // Read after the rewrite rather than before: `rewrite_input` takes the whole
        // session mutably, so a borrow of the previous candidate taken first would have to
        // be released before it runs. The rewrite touches the request and the scheme and
        // neither the previous result nor the paging state, so the word the highlight was
        // on is the same either way.
        let prev = self
            .previous
            .candidates
            .get(usize::from(self.paging.highlight))
            .map(|held| held.text.as_str());
        self.request.scheme = if rewritten {
            self.scheme.id()
        } else {
            SchemeId::FULL
        };
        self.request.flags.set(DecodeFlags::SHUANGPIN, rewritten);
        self.request.flags.set(DecodeFlags::ABBREV, self.abbrev);
        // The correction switch rides the same rule as its siblings: the bit is
        // set from the latch every keystroke, so a reload that turned it off
        // clears it instead of leaving it set behind the keystroke before.
        self.request
            .flags
            .set(DecodeFlags::CORRECTION, self.correction);
        // The fuzzy classes arrive as the mask the projection latched with the scheme:
        // removed first, so a reload that turned a class off clears its bit instead of
        // leaving it set from the keystroke before, then inserted whole. With the mask
        // empty this is a no-op, which is what keeps the all-off decode byte-identical
        // to a build without the section.
        self.request.flags.remove(crate::fuzzy::FLAG_MASK);
        self.request.flags.insert(self.fuzzy);
        // The mixed-English switch rides the same rule as its siblings: the bit is
        // set from the latch every keystroke, so a reload that turned it off
        // clears it instead of leaving it set behind the keystroke before.
        self.request.flags.set(DecodeFlags::ENGLISH, self.english);
        // The hint channel short-circuits here rather than inside the decode: with
        // the switch off the table is `None` all the way down, which is the whole
        // cost of the switch being off.
        let hints = if self.hints { env.confusables } else { None };
        env.decoder.decode_into(
            &mut self.scratch,
            &self.request,
            DecodeSources {
                lexicon: env.lexicon,
                user_freq: env.user_freq,
                lm: env.lm,
                english: env.english,
                hints,
            },
        );
        // The context re-rank is the session's own post-pass, and it runs before the
        // repairs below read the list: the order the user sees, the order the
        // highlight reconciles against and the order the preedit's page carries are
        // one and the same. The context word is this session's memory of the last
        // commit and nothing else, so with none -- a first composition, or one whose
        // context expired -- the call is never made and the list is the decode's own.
        // The workspace runs it rather than the session, because the drafts the
        // candidates came from have to be permuted by the same moves, and those are
        // the workspace's own type.
        if let Some(context) = self.context_word.as_deref() {
            self.scratch.rerank_with_context(Some(context), env.lm);
        }
        self.paging
            .reconcile(prev, &self.scratch.result().candidates);
        if rewritten {
            // The rewrite walked the keystrokes in order, so it is the only pass that
            // can say where a scheme syllable started; the graph describes the
            // rewritten spelling and its boundaries are the layout's, not the user's.
            self.buf.set_boundaries(self.scheme.grid());
        } else {
            // The graph comes out of the workspace the decode just wrote, so the input
            // is segmented once per keystroke rather than twice: `decode_into` builds
            // it, and this only maps its syllable boundaries back onto the raw input.
            write_boundaries(&mut self.buf, self.scratch.dag());
        }
        build_preedit_into(&self.buf, self.scratch.dag(), &mut self.preedit);
    }

    /// Rewrites the request's input through the active layout.
    ///
    /// # Returns
    ///
    /// `true` when [`Session::request`] now carries the layout's full-pinyin spelling
    /// of the keystrokes. `false` when the input is decoded exactly as typed, which is
    /// full pinyin and a layout this build does not implement.
    fn rewrite_input(&mut self) -> bool {
        if !self.scheme.is_active() {
            return false;
        }
        // The scheme key alphabet is lower case while the buffer keeps what the user
        // pressed, so the copy the rewrite reads is folded here. Folding rewrites byte
        // values and moves no offset, so the grid that comes back still names
        // positions in the keystrokes in `Session::buf`.
        self.request.raw.make_ascii_lowercase();
        if let Err(err) = self.scheme.rewrite(self.request.raw.as_str()) {
            // A layout a newer build wrote. The input is read as typed and the host is
            // told why, which is what the contract asks of a scheme number this build
            // cannot honour: the user keeps typing rather than losing the composition
            // to a configuration key.
            self.scheme_error = Some(err);
            self.request.raw.clear();
            self.request.raw.push_str(self.buf.raw());
            return false;
        }
        self.request.raw.clear();
        self.request.raw.push_str(self.scheme.text());
        if self.request.raw.is_empty() {
            // A stream the layout has nothing to say about rewrites to nothing: a lone
            // `v` -- the key every `zh` syllable starts on -- is dropped rather than
            // carried, because the normalizer would fold it onto `ü`. Reading the
            // keystrokes as typed instead keeps the window from showing a candidate
            // with no text, which looks to the user like an input method that has
            // stopped answering.
            self.request.raw.push_str(self.buf.raw());
            return false;
        }
        true
    }

    /// Rebuilds the preedit from the current input and graph.
    pub(in crate::state) fn rebuild_preedit(&mut self) {
        build_preedit_into(&self.buf, self.scratch.dag(), &mut self.preedit);
    }

    /// Adopts a new page size and puts the highlight back on the word it was on.
    ///
    /// The candidate list is read from the workspace and the highlight is written into
    /// the paging state, which are different fields of the session; the list stays
    /// borrowed while the paging state is written, which is why this is one call here
    /// rather than two calls from the transition. A transition reaches the list only
    /// through [`Session::decoded`], and that accessor borrows the whole session, so it
    /// cannot be held across a mutable borrow of the paging state.
    pub(in crate::state) fn adopt_page_size(&mut self, size: u8) {
        let prev = self
            .scratch
            .result()
            .candidates
            .get(usize::from(self.paging.highlight))
            .map(|held| held.text.as_str());
        self.paging.set_page_size(size);
        self.paging
            .reconcile(prev, &self.scratch.result().candidates);
    }

    /// Drops the candidate at `index` and moves the highlight onto the word that takes its
    /// place, returning the text of the word that was dropped.
    ///
    /// The list is the session's own: the store cannot answer until the effect that forgets
    /// the word has run, and the user has to see the candidate go on the keystroke, so the
    /// removal happens here as well as there. Renumbering keeps the invariant every decode
    /// establishes -- a candidate's display number is its position plus one -- because the
    /// number keys and the click indices are read against it.
    ///
    /// [`Paging::reconcile`] is reused rather than the highlight being moved by hand: it
    /// already looks a word up by its text and turns to the page that holds it, which is
    /// what keeps the highlight on a word the user was looking at instead of sending it back
    /// to the first candidate. The anchor is the word that took the removed one's place, or
    /// the word before it when the last candidate went -- there is no list position left to
    /// stand on at the end.
    ///
    /// # Panics
    ///
    /// Never panics.
    pub(in crate::state) fn drop_candidate_at(&mut self, index: u16) -> Option<String> {
        let mut result = self.scratch.take_result();
        let position = usize::from(index);
        let dropped = result
            .candidates
            .get(position)
            .map(|held| held.text.clone());
        let anchor = match dropped {
            Some(_) => {
                result.candidates.remove(position);
                renumber(&mut result.candidates);
                result
                    .candidates
                    .get(position)
                    .or_else(|| {
                        position
                            .checked_sub(1)
                            .and_then(|before| result.candidates.get(before))
                    })
                    .map(|held| held.text.clone())
            }
            None => None,
        };
        self.scratch.recycle(result);
        if dropped.is_some() {
            self.paging
                .reconcile(anchor.as_deref(), &self.scratch.result().candidates);
        }
        dropped
    }

    /// Builds the frame the window draws from the session's current state.
    ///
    /// The frame leaves the session for the UI thread, so everything in it is owned and
    /// this call allocates: the preedit text and its span list, the page's candidates,
    /// and the mode strip. None of those copies can be dropped without changing
    /// [`UiFrame`], which is frozen -- a frame built out of borrows would outlive the
    /// session state it points at. What the build does *not* do is copy anything twice:
    /// the preedit it snapshots is the buffer [`build_preedit_into`] refills in place
    /// across keystrokes, and the candidates are one clone of the page the decoder
    /// produced, taken with an exact-sized allocation.
    fn build_frame(&self, cfg: &SessionConfig, revision: Revision) -> UiFrame {
        let candidates = &self.decoded().candidates;
        let total = u16::try_from(candidates.len()).unwrap_or(u16::MAX);
        let start = usize::from(self.paging.page_start()).min(candidates.len());
        let end = usize::from(self.paging.page_end(total)).min(candidates.len());
        // Page-local position of the keyboard highlight, or `None` when the page holds
        // no candidate for it. The view treats `None` as "hide the ring", never as
        // "keep the previous one": a ring that outlives its candidate is the defect
        // this field exists to prevent. The conversion runs here, on the engine side of
        // the boundary, so the frame and the candidate slice it carries share one basis
        // and the view never re-derives the position.
        let highlight = self.paging.highlight_position_in_page(total);
        UiFrame {
            revision: revision.value(),
            preedit: self.preedit.clone(),
            candidates: candidates[start..end].to_vec(),
            page: self.paging.page_state(total),
            status: self.ctx.status.clone(),
            anchor: self.ctx.anchor,
            layout: LayoutHint {
                max_per_row: self.paging.page_size,
                show_annotation: cfg.show_annotation,
                max_width_dp: cfg.max_width_dp,
            },
            highlight,
        }
    }

    /// Emits the preedit, the show and the frame that put a composition on screen.
    ///
    /// The show precedes the frame because the window drops a frame that arrives
    /// while it is hidden, so the two cannot be reordered.
    ///
    /// The preedit the application's preedit area is told to show is taken from the
    /// frame rather than read from the session a second time, so the two carriers of
    /// the composing text are the same value: the window header and the application
    /// cannot disagree about it, which two independent reads could only guarantee by
    /// construction.
    pub(in crate::state) fn emit_window(&mut self, ctx: &mut Ctx<'_>) {
        let revision = self.revision.next();
        let frame = Box::new(self.build_frame(ctx.cfg, revision));
        ctx.push(Effect::UpdatePreedit(frame.preedit.clone()));
        ctx.push(Effect::Show(AnchorHint {
            revision,
            placement: Placement::Auto,
        }));
        ctx.push(Effect::SendFrame(frame));
    }

    /// Emits the preedit and the frame after a change to the composing text.
    ///
    /// The application's preedit comes from the frame, as in [`Session::emit_window`].
    pub(in crate::state) fn emit_update(&mut self, ctx: &mut Ctx<'_>) {
        let revision = self.revision.next();
        let frame = Box::new(self.build_frame(ctx.cfg, revision));
        ctx.push(Effect::UpdatePreedit(frame.preedit.clone()));
        ctx.push(Effect::SendFrame(frame));
    }

    /// Emits the frame alone, after a change that leaves the composing text as it was.
    ///
    /// `cfg` is passed explicitly rather than read from the context because a
    /// configuration reload repaints the window under the *new* values.
    pub(in crate::state) fn emit_frame(&mut self, cfg: &SessionConfig, ctx: &mut Ctx<'_>) {
        let revision = self.revision.next();
        let frame = Box::new(self.build_frame(cfg, revision));
        ctx.push(Effect::SendFrame(frame));
    }
}
