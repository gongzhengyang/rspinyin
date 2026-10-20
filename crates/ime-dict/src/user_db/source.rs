//! `UserDb` ownership teardown and the frozen frequency-source boundary.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use ime_types::{ImeError, UserFreqSource, WordRef};

use super::{FLUSH_SETTLE_TIMEOUT_MS, ForgetOutcome, Inner, PINNED_FREQ, UserDb, lock};

impl Drop for UserDb {
    /// Gives the flush thread a moment to step out of the file.
    ///
    /// The thread holds the store only for the length of one flush, but that is long enough
    /// to keep the `redb` file lock standing after the last clone has dropped -- which is
    /// what an immediate reopen (the recovery pass, a test) would otherwise meet. The wait
    /// is bounded by [`FLUSH_SETTLE_TIMEOUT_MS`], the same bound an explicit flush waits
    /// with; past it the flush is still running, and the lock lifts when it ends rather than
    /// when this drop does.
    fn drop(&mut self) {
        // Best effort by design, and the guard is what keeps it so: only the last clone
        // settles, because only it can be the one whose release ends the store. A clone
        // that appears while this one drops keeps the store -- and its thread -- alive and
        // owns the next settle itself.
        if Arc::strong_count(&self.inner) != 1 {
            return;
        }
        if let Some(writer) = self.inner.writer.get().and_then(|set| set.as_ref()) {
            writer.settle(Duration::from_millis(FLUSH_SETTLE_TIMEOUT_MS));
        }
    }
}

impl UserFreqSource for UserDb {
    fn freq(&self, key: &str) -> u32 {
        // Both halves of the answer are in memory, so a decode never opens a read
        // transaction: the frozen `UserFreqSource` contract requires this call not to block
        // and not to take a lock a writer can hold, and a read transaction does both. The
        // two locks are taken in the order the module fixes, `committed` before `pending`,
        // and both are released before the call returns.
        if self.inner.is_hydrated {
            let now = self.inner.tick.load(Ordering::Relaxed);
            let mut committed = lock(&self.inner.committed);
            // The delta is read under its own lock while `committed` is still held, in the
            // order the module fixes: `committed` first, then `pending`, never the reverse.
            let (forgotten, pending) = {
                let pending = lock(&self.inner.pending);
                (
                    pending.removed.contains(key),
                    pending.entries.get(key).copied(),
                )
            };
            let answer = committed.get_mut(key).map_or_else(
                || {
                    pending.map_or(0, |delta| {
                        self.inner
                            .decay
                            .fold(i64::from(delta.count) << 16, delta.tick, now)
                    })
                },
                |record| match pending {
                    Some(delta) => {
                        let anchor = record.tick.max(delta.tick);
                        let aged = self.inner.decay.fold(record.dee_q16, record.tick, anchor);
                        let entered = aged.saturating_add(i64::from(delta.count) << 16);
                        self.inner.decay.fold(entered, anchor, now)
                    }
                    None => {
                        record.dee_q16 = self.inner.decay.fold(record.dee_q16, record.tick, now);
                        record.tick = record.tick.max(now);
                        record.dee_q16
                    }
                },
            );
            let commits = committed.get(key).map_or(0, |record| record.commits);
            let commits = pending.map_or(commits, |delta| commits.saturating_add(delta.count));
            // A tombstone outranks the count the file still holds: the user asked for the
            // word to go, and the flush that makes the file agree has not run yet.
            return if forgotten {
                0
            } else {
                if lock(&self.inner.pinned).contains(key) {
                    PINNED_FREQ
                } else {
                    self.inner.decay.effective_freq(commits, answer)
                }
            };
        }
        // The store was too large to load, so the answer has to come from the file. The
        // fallback is bounded by the ceiling rather than by the word count: `on_demand`
        // opens one transaction per distinct word per session.
        if self.inner.is_pinned(key) {
            PINNED_FREQ
        } else {
            self.inner.on_demand(key)
        }
    }

    /// Records one commit of `key`, without blocking and without allocating when the key is
    /// already pending.
    ///
    /// The wall clock is not read, the cache is not touched, and the write transaction is not
    /// performed here: the delta map's own entry point owns the batching policy, and the flush
    /// thread owns the disk. What is left on this path is one monotonic reading, one lock of
    /// the delta map, one hash lookup, and -- only for a key the map has not seen -- one
    /// allocation.
    fn record(&self, key: &str, _weight_hint: u16) {
        if self.is_readonly() {
            return;
        }
        if self.inner.record_delta(key) {
            self.inner.request_flush();
        }
    }

    fn is_user_word(&self, key: &str) -> bool {
        // The answer is the coin table's, from memory: the word → rows map the open
        // filled and the coin path keeps in step. A word the user has only typed often
        // is a frequency entry, not a coined one, and answers `false` -- the question is
        // about words that exist because the user made them, which is what the delete
        // flow and the candidate labels ask.
        !self.inner.coined_composites(key).is_empty()
    }

    fn forget(&self, key: &str) -> bool {
        // The frozen method answers whether a record went away; `forget_with_outcome` is
        // the same call with the reason kept, which is what the host reports a diagnostic
        // from. A read-only store answers `false`, because nothing was removed -- the
        // caller that needs to tell the two refusals apart asks for the outcome instead.
        // A pinned record is not a refusal: the removal takes the pin with it.
        self.forget_with_outcome(key) == ForgetOutcome::Removed
    }

    fn list(&self, offset: u64, limit: u16) -> Result<Vec<WordRef<'_>>, ImeError> {
        // The store cannot serve this method, and the reason is a property of the signature
        // rather than a gap in the implementation. `WordRef<'a>`'s text borrows for as long
        // as `&self`, and the store's word set changes under it: a borrow can be taken out
        // of a mutex guard only for as long as the guard lives, and the store's keys live
        // behind one -- `committed` for a loaded store, the cache for a fallback one --
        // because `record` and `forget` take `&self` and must be able to write. There is no
        // safe way to publish them for `'self`, and the two ways to fake it are both
        // forbidden here: leaking the keys grows the resident set with the user's
        // vocabulary, and reaching past the guard needs `unsafe`, which this crate confines
        // to `mmap.rs`.
        //
        // `dict/unsupported` is the contract's code for a capability the build does not
        // provide, and it is what a caller falls back from: `UserDb::list_words` answers the
        // same page as owned rows, and `UserFreqSource::export_tsv` answers the whole store.
        let _ = (offset, limit);
        Err(ImeError::Unsupported)
    }

    fn export_tsv(&self, writer: &mut dyn std::io::Write) -> Result<u64, ImeError> {
        self.inner.write_export(writer)
    }

    fn set_pinned(&self, key: &str, word: Option<&str>) -> Result<(), ImeError> {
        // The inherent method of the same name carries the behaviour; inside this impl
        // the name resolves to that one first, so the call is delegation, not recursion.
        self.set_pinned(key, word)
    }
}

impl UserDb {
    /// Toggles, moves or clears the pin of one input key (ADR-0013).
    ///
    /// `Some(word)` is the UI's action: a word not pinned under `key` is pinned, the
    /// same word again is released, and a different word takes the pin from the one
    /// that held it -- one pinned word per key, so the word the user highlighted last
    /// is the one that rises. `None` clears the key's pin outright. The in-memory state
    /// changes on this call, which is what lets the next decode rank the new pin
    /// without waiting for the disk; the durable write rides the flush the toggle
    /// requests.
    ///
    /// # Errors
    ///
    /// Returns [`ImeError::DataReadonly`] when the store can no longer write. Nothing
    /// is changed in that case, so a caller never sees a pin the file did not agree to.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn set_pinned(&self, key: &str, word: Option<&str>) -> Result<(), ImeError> {
        if self.is_readonly() {
            return Err(ImeError::DataReadonly {
                reason: String::from("data/readonly-mode"),
            });
        }
        if self.inner.toggle_pin(key, word) {
            self.inner.request_flush();
        }
        Ok(())
    }
}

impl Inner {
    /// Applies one pin request to the in-memory state, reporting whether the durable
    /// state changed.
    ///
    /// A word can be pinned under more than one key -- the user may pin it typed short
    /// and typed long -- so a release removes a word from the ranked set only when no
    /// other key still names it: the set is what `freq` and eviction read, and it must
    /// hold a word while any of its pins stands.
    ///
    /// The three structures take their turns in separate scopes -- the key map, the
    /// word set, the pending queues -- because the module's lock order keeps them
    /// leaves: none is held while another is taken. Between the scopes the state is
    /// momentarily uneven, but the only writer of all three is this function, and its
    /// caller holds no lock across it.
    ///
    /// # Panics
    ///
    /// Never.
    fn toggle_pin(&self, key: &str, word: Option<&str>) -> bool {
        // The key map owns the decision: it alone knows what the key holds now.
        let (released, installed) = {
            let mut pins = lock(&self.key_pins);
            match word {
                None => (pins.remove(key), None),
                Some(word) if pins.get(key).is_some_and(|held| held.as_ref() == word) => {
                    // The same word again: the second press of the toggle.
                    (pins.remove(key), None)
                }
                Some(word) => {
                    let installed: Box<str> = Box::from(word);
                    (pins.insert(Box::from(key), installed.clone()), Some(installed))
                }
            }
        };
        {
            let mut pinned = lock(&self.pinned);
            if let Some(released) = &released {
                let still_named = lock(&self.key_pins)
                    .values()
                    .any(|held| held == released);
                if !still_named {
                    pinned.remove(released.as_ref());
                }
            }
            if let Some(installed) = &installed {
                pinned.insert(Box::from(installed.as_ref()));
            }
        }
        {
            let mut pending = lock(&self.pending);
            if let Some(released) = &released {
                pending.pins.insert(released.clone(), false);
            }
            if let Some(installed) = &installed {
                pending.pins.insert(installed.clone(), true);
            }
            pending.key_rows.insert(Box::from(key), installed.clone());
        }
        released.is_some() || installed.is_some()
    }

    /// Answers whether `word` is pinned, including a toggle the flush has not yet written.
    ///
    /// The set is mutated on the toggle call itself, so the pending queues hold nothing
    /// the set does not already show.
    ///
    /// # Panics
    ///
    /// Never.
    fn is_pinned(&self, word: &str) -> bool {
        lock(&self.pinned).contains(word)
    }
}
