//! The phrase document as the host holds it: the built-in rows, the user's own file, and
//! the table the two merge into.
//!
//! # Responsibility
//!
//! `ime-core`'s phrase module owns the *format*: it reads a document into a table and
//! renders an entry as a row, and it touches no file. This module owns the file. It
//! resolves the document `[phrases] file` names, reads it at startup, merges it over the
//! built-in document that ships inside the binary, and hands one row to the writer thread
//! when the session asks for a phrase to be saved.
//!
//! # The table the plugin runs on
//!
//! Built-in first, the user's second: the reader keeps the last definition of a key, so a
//! key the user defined wins over the one the plugin ships -- that is the whole merge
//! rule, and it needs no second implementation. The result is one immutable snapshot
//! behind an `Arc`, so a decode that is already running keeps the snapshot it started
//! with while a saved phrase replaces the pointer.
//!
//! # Where the document is
//!
//! `[phrases] file` when the configuration names one: an empty value is the default
//! location `phrases.tsv` inside the user's configuration directory, a relative value is
//! resolved against that same directory, and an absolute value is used as it stands. The
//! directory comes from [`ime_dict::paths`] rather than from a second lookup of the XDG
//! variables, because that module is the one that knows how this plugin's user data is
//! laid out and how private each piece of it has to be.
//!
//! # Degradation
//!
//! A document that is missing, unreadable or full of rows the reader refuses never stops
//! the input method. The table degrades to what could be read, and the startup step
//! reports [`PHRASE_TABLE_UNAVAILABLE`] once, where the document was found to be
//! unusable. A fresh installation has no document at all, which is that same case.
//!
//! # Threading
//!
//! The store is installed once by the addon's startup sequence and reached through
//! [`handle`]. It sits behind a lock because a saved phrase replaces the table a decode
//! reads; the lock is held for one pointer clone or one queue hand-off, is never held
//! across file IO, and is never contended, because the engine runs on the Fcitx5 host
//! thread alone.
//!
//! At unload the addon's teardown is what stops the writer, through [`drain_writer`]:
//! the queue is drained before the thread stops, so a graceful unload loses nothing, and
//! a writer that misses the teardown's budget is recorded and detached rather than
//! holding up the host's exit.
//!
//! Saving a phrase is the one thing this module used to do itself, and it is the one thing
//! a key callback must not do: the row is rendered here, on the host thread, and the
//! append and the read-back that follows it happen on the writer thread of the `deferred`
//! module. What the host thread pays for a save is one row rendering and one `try_send`.

// The thread that performs the write and the read-back a save used to do here. Declared in
// this file rather than in the router's module list because the store is what reaches it.
mod deferred;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use ime_config::{Config, DEFAULT_PHRASE_ENTRIES, PhraseConfig};
use ime_core::phrase::{PHRASE_TABLE_UNAVAILABLE, PhraseReport, PhraseTable};
use ime_dict::paths::{self, Paths};
use ime_types::ImeError;

use crate::ffi::emit_diagnostic;

use self::deferred::{DocumentSink, PhraseSnapshot, PhraseWriter, WRITER_GONE_REASON};

/// The document the user's phrases live in, inside the configuration directory.
///
/// The default `[phrases] file` location. The name says what the file is: a phrase
/// document, tab-separated, which is the format `ime-core`'s phrase module reads.
const PHRASE_FILE_NAME: &str = "phrases.tsv";

/// The document that ships inside the binary.
///
/// The built-in rows are the plugin's own text, so they travel with the code rather than
/// with an installation step: a plugin that had to find a second file to have any phrase
/// at all would have a way to come up with none. `data/raw/phrase.tsv` is the source of
/// record and is committed, so the two cannot drift.
const BUILT_IN_DOCUMENT: &str = include_str!("../../../../../data/raw/phrase.tsv");

/// The diagnostic a saved phrase is reported with.
///
/// The spelling is registered in the design's runtime-diagnostic table and must not be
/// reworded. It carries no user content: which key was saved and what it commits are the
/// user's own text, and neither reaches a diagnostic. It is reported once the row is
/// queued, which is the point at which the save can no longer be refused to the caller.
pub const PHRASE_ADDED_CODE: &str = "phrase/added";

/// The diagnostic a save is reported with when the table was already at its entry limit.
///
/// Reported rather than failed: the row is in the document either way, and raising
/// `[phrases] max_entries` makes it live again. The read-back that finds the limit is the
/// writer thread's, so this is emitted from there.
pub const PHRASE_LIMIT_EXCEEDED_CODE: &str = "phrase/limit-exceeded";

/// Recorded when the phrase writer does not stop within the unload budget.
///
/// The addon's teardown waits for the writer on a bounded budget of its own -- a slice of
/// the destroy budget, `PHRASE_DRAIN_BUDGET` in the lifecycle module. A writer that has
/// not stopped when the budget runs out is detached rather than waited for, and this code
/// is what makes that abandonment visible in the log instead of silent. The spelling is
/// registered in the design's runtime-diagnostic table and must not be reworded.
pub const PHRASE_SHUTDOWN_TIMEOUT_CODE: &str = "phrase/shutdown-timeout";

/// Why a phrase could not be written: the configuration and the layout name no document.
const NO_DOCUMENT_REASON: &str = "no phrase document is configured";

/// Why a phrase could not be written: the document refused the write.
const WRITE_FAILED_REASON: &str = "the phrase document cannot be written";

/// The store the process runs on, installed once by the startup sequence.
static PHRASES: OnceLock<PhraseHandle> = OnceLock::new();

/// The phrase document as the host holds it.
///
/// Built once per load and reached through [`handle`]. What a caller reads is the snapshot
/// the writer thread last published, which is the merged table from the load until a row
/// has been committed.
pub struct PhraseStore {
    /// Where the user's document lives, or `None` when the configuration and the layout
    /// offer none, in which case there is nothing to write to and a save is refused before
    /// it is queued.
    user_path: Option<PathBuf>,
    /// The thread that appends a saved row and reads the document back, or `None` when
    /// there is no document to write to or the thread could not be started.
    writer: Option<PhraseWriter>,
    /// What reading the documents produced at load, answered as it stands while there is no
    /// writer to publish a newer table.
    startup: PhraseSnapshot,
}

impl PhraseStore {
    /// Loads the merged table from the document at `user_path` and starts the writer that
    /// appends to it.
    ///
    /// # Parameters
    ///
    /// - `user_path`: the user's document, or `None` when there is none to read or write.
    /// - `max_entries`: the most entries the merged table may hold. Rows past it are
    ///   dropped and counted in [`PhraseReport::over_limit`].
    /// - `is_enabled`: `[phrases] enabled`. With it off the table holds nothing, because
    ///   the key says the user wants no phrase to take part in a decode. The document is
    ///   still read, so the setting never costs the user an entry they wrote.
    ///
    /// # Returns
    ///
    /// The store, holding the built-in table alone when the document could not be read. The
    /// read happens here, on the caller's thread, because this is the startup step: the
    /// first key must have a table to match against. Every read after it happens on the
    /// writer thread, once a row has been appended.
    ///
    /// # Errors
    ///
    /// None: an unreadable document is the degradation [`PhraseStore::is_degraded`]
    /// reports, and a phrase table must not be able to stop the input method. Neither is a
    /// writer thread that could not be started -- the store keeps serving the table it read
    /// and refuses every save with `data/readonly-mode`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn load(user_path: Option<PathBuf>, max_entries: usize, is_enabled: bool) -> Self {
        let sink = DocumentSink::new(user_path.clone(), max_entries, is_enabled);
        let startup = sink.read();
        // No document means no writer: there is nothing for the thread to write to, and a
        // store that cannot save anyway should not hold a thread for the process's life.
        let writer = if user_path.is_some() {
            PhraseWriter::start(Box::new(sink), startup.clone())
        } else {
            None
        };
        Self {
            user_path,
            writer,
            startup,
        }
    }

    /// The table in force.
    ///
    /// A clone of the `Arc` rather than a copy of the table: a decode that is already
    /// running keeps the snapshot it started with when a commit replaces this one, which is
    /// what keeps a save from disturbing an input session in progress.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn table(&self) -> Arc<PhraseTable> {
        self.snapshot().table
    }

    /// What reading the documents produced: how many entries the table holds and how much
    /// of the user's document was skipped, overridden or dropped at the limit.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn report(&self) -> PhraseReport {
        self.snapshot().report
    }

    /// Whether the user's document was missing or could not be read.
    ///
    /// A fact about the document and not about the setting: a table that holds nothing
    /// because `[phrases] enabled` is off is not degraded, and a document that is not
    /// there is degraded whether or not the table would have used it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_degraded(&self) -> bool {
        self.snapshot().is_degraded
    }

    /// Hands one entry to the writer thread.
    ///
    /// The row is rendered here, on the host thread, so that an entry the reader would
    /// refuse is refused before anything is queued; the append, the read-back and the table
    /// the next decode sees all happen on the writer thread. A caller that reports "saved"
    /// on `Ok` is reporting that the row will be written.
    ///
    /// # Parameters
    ///
    /// - `key`: the shortcut the user typed, in the key alphabet the session folded it to.
    /// - `text`: the text the key commits, which is the candidate the highlight was on.
    ///
    /// # Returns
    ///
    /// `Ok(())` once the row is queued. The row is not on disk when this returns, and it is
    /// not flushed with `fsync`: a phrase lost to a crash is a phrase the user can type
    /// again, and a synchronous flush inside a key callback is the blocking work the
    /// threading contract forbids. A graceful stop loses nothing, because
    /// [`PhraseStore::shutdown`] waits for the writer to drain what it accepted.
    ///
    /// # Errors
    ///
    /// [`ImeError::ConfigInvalid`] naming `phrases.file` when the entry is not one the
    /// reader would accept; [`ImeError::DataReadonly`] when there is no document to write
    /// to, no writer to hand the row to, or a writer that is a whole queue behind. Neither
    /// carries the phrase's text or a path. A document that refuses the append cannot be
    /// reported here -- the write happens on the writer thread -- and travels on the
    /// diagnostic channel as `data/readonly-mode` instead.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn append(&self, key: &str, text: &str) -> Result<(), ImeError> {
        if self.user_path.is_none() {
            return Err(unwritable(NO_DOCUMENT_REASON));
        }
        let Some(writer) = self.writer.as_ref() else {
            return Err(unwritable(WRITER_GONE_REASON));
        };
        writer.submit(key, text)
    }

    /// Stops the writer, waiting at most `timeout` for it to finish.
    ///
    /// Every row that was accepted is written before the thread stops, which is what makes
    /// a graceful shutdown lose nothing. The addon's teardown is what calls this; a process
    /// that ends without it loses the rows still in the queue, which is the window the
    /// document has always had -- nothing here flushes with `fsync`.
    ///
    /// # Returns
    ///
    /// Whether the writer stopped within `timeout`. A store that never had one -- no
    /// document, or a thread that could not be started -- answers `true` at once, and so
    /// does a second call after the thread has stopped.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        match self.writer.as_ref() {
            Some(writer) => writer.shutdown(timeout),
            None => true,
        }
    }

    /// Takes the writer out of the store and stops it, waiting at most `budget`.
    ///
    /// The teardown form of [`PhraseStore::shutdown`]: the writer is removed rather than
    /// left in place, so a store the destructor has drained holds no writer at all -- a
    /// save that somehow arrives after the unload began is refused before it is queued,
    /// instead of being accepted into a queue nobody will drain again.
    ///
    /// # Parameters
    ///
    /// - `budget`: how long the caller may wait for the queue to drain and the thread to
    ///   stop. A writer that outlives it is detached and the answer is `false`, which the
    ///   teardown records; the host thread is never held up for longer than it asked for.
    ///
    /// # Returns
    ///
    /// Whether the writer stopped within `budget`. A store that never had one -- no
    /// document, or a thread that could not be started -- answers `true` at once, and so
    /// does a second call after the writer has been taken.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn drain_phrase_writer(&mut self, budget: Duration) -> bool {
        let Some(writer) = self.writer.take() else {
            return true;
        };
        let stopped = writer.shutdown(budget);
        // The table the writer last published outlives it: the drained store answers for
        // the rows that landed, which is the answer a store with a live writer gave too.
        self.startup = writer.snapshot();
        stopped
    }

    /// The snapshot in force: what the writer last published, or what the startup read
    /// produced while there is no writer to publish a newer one.
    ///
    /// # Panics
    ///
    /// Never.
    fn snapshot(&self) -> PhraseSnapshot {
        match self.writer.as_ref() {
            Some(writer) => writer.snapshot(),
            None => self.startup.clone(),
        }
    }

    /// Loads the document the configuration names.
    ///
    /// # Parameters
    ///
    /// - `phrases`: the `[phrases]` section, whose `file` and `max_entries` decide what is
    ///   read.
    /// - `layout`: the XDG layout, or `None` when no base directory could be resolved, in
    ///   which case there is no default location and no document.
    ///
    /// # Returns
    ///
    /// The store, already loaded.
    ///
    /// # Panics
    ///
    /// Never.
    fn from_config(phrases: &PhraseConfig, layout: Option<&Paths>) -> Self {
        let user_path = layout.map(|layout| document_path(&phrases.file, layout));
        let max_entries = entry_limit(phrases.max_entries);
        Self::load(user_path, max_entries, phrases.enabled)
    }
}

/// A shared handle to the phrase store.
///
/// The router holds one and the startup sequence installs one, which is what lets a
/// single store be reached from both without either owning the other.
#[derive(Clone)]
pub struct PhraseHandle {
    /// The store, shared so that a handle can be cloned to whoever needs it.
    inner: Arc<Mutex<PhraseStore>>,
}

impl PhraseHandle {
    /// Wraps a store in a handle.
    ///
    /// # Arguments
    ///
    /// * `store` — the store every clone of this handle reaches.
    ///
    /// # Returns
    ///
    /// The handle.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn new(store: PhraseStore) -> Self {
        Self {
            inner: Arc::new(Mutex::new(store)),
        }
    }

    /// The table in force.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn table(&self) -> Arc<PhraseTable> {
        self.locked().table()
    }

    /// Hands one entry to the writer thread.
    ///
    /// # Errors
    ///
    /// As [`PhraseStore::append`].
    ///
    /// # Panics
    ///
    /// Never.
    pub fn append(&self, key: &str, text: &str) -> Result<(), ImeError> {
        self.locked().append(key, text)
    }

    /// Stops the writer, waiting at most `timeout` for it to finish.
    ///
    /// The call a teardown makes, so that the rows a session saved before the process ends
    /// are written rather than left in the queue. See [`PhraseStore::shutdown`], which is
    /// what this reaches.
    ///
    /// # Returns
    ///
    /// Whether the writer stopped within `timeout`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        self.locked().shutdown(timeout)
    }

    /// Takes the writer out of the store and stops it, waiting at most `budget`.
    ///
    /// The call the addon's teardown makes, so the rows a session saved before the process
    /// ends are written rather than left in the queue: the queue is drained before the
    /// writer stops, which is what makes a graceful unload lose nothing. Unlike
    /// [`PhraseHandle::shutdown`] the writer is taken out of the store, so the drained
    /// store accepts no further rows.
    ///
    /// # Parameters
    ///
    /// - `budget`: how long the caller may wait. A writer that outlives it is detached and
    ///   the answer is `false`, which the teardown records.
    ///
    /// # Returns
    ///
    /// Whether the writer stopped within `budget`. A store without a writer answers `true`
    /// at once, and so does a second call after the writer has been taken.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn drain_phrase_writer(&self, budget: Duration) -> bool {
        self.locked().drain_phrase_writer(budget)
    }

    /// Borrows the store, recovering the contents of a poisoned lock.
    ///
    /// Poisoning means a holder panicked. The store holds a table and a writer handle, so
    /// the worst a recovered one can do is rebuild a table from a document it already read;
    /// refusing to answer would cost the user their phrases for the rest of the process.
    /// The diagnostic throttle's table is recovered the same way, for the same reason.
    fn locked(&self) -> MutexGuard<'_, PhraseStore> {
        match self.inner.lock() {
            Ok(store) => store,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// The handle the routing layer reads and writes through.
///
/// Answers the store the startup sequence installed, or a built-in-only store when it
/// never ran -- a process whose addon was declined, or a caller that drives the router
/// without one. Such a store has no writer either, because there is no document to write
/// to, and a save made through it is refused with `data/readonly-mode`. Nothing is
/// installed here, so a router built before the startup sequence cannot take the place
/// that sequence is about to fill.
///
/// # Panics
///
/// Never.
pub fn handle() -> PhraseHandle {
    if let Some(handle) = PHRASES.get() {
        return handle.clone();
    }
    let fallback = PhraseStore::load(None, entry_limit(DEFAULT_PHRASE_ENTRIES), true);
    PhraseHandle::new(fallback)
}

/// Stops the phrase writer the installed store holds, waiting at most `budget`.
///
/// The addon's teardown calls this once, after the sessions have gone and before the user
/// store is flushed: a phrase queued in the last keystrokes of a session is the user's own
/// writing, and this is the call that lands it. When nothing is installed -- the addon was
/// declined, or the load never ran -- the answer is `true` and nothing is touched, which is
/// what keeps a destructor that has no writer to release free.
///
/// A writer that does not stop within `budget` is recorded under
/// [`PHRASE_SHUTDOWN_TIMEOUT_CODE`] and detached: the host's exit is never held up for it,
/// and the timeout is what the log shows instead of a silent loss.
///
/// # Parameters
///
/// - `budget`: how long the caller may wait for the queue to drain and the thread to stop.
///
/// # Returns
///
/// Whether the writer stopped within `budget`, or there was no writer to stop.
///
/// # Panics
///
/// Never.
pub fn drain_writer(budget: Duration) -> bool {
    // The slot is read rather than reached through `handle`: the fallback store `handle`
    // builds for a process that never installed one reads and merges the built-in
    // document, and a destructor that has no writer to drain must not pay for a table it
    // will never use.
    let Some(handle) = PHRASES.get() else {
        return true;
    };
    let stopped = handle.drain_phrase_writer(budget);
    if !stopped {
        emit_diagnostic(PHRASE_SHUTDOWN_TIMEOUT_CODE);
    }
    stopped
}

/// Loads the phrase document this process runs on and installs it.
///
/// The addon's startup step. It resolves the layout, reads `[phrases]` out of the
/// configuration document, loads the merged table and puts it where [`handle`] answers
/// it. The first install wins: a second call leaves the store in force alone, because
/// replacing it would swap the table a session may be decoding against.
///
/// # Returns
///
/// `Ok(())` in every case. The signature is the lifecycle step's, and the one thing this
/// step must never do is stop the input method: a document that cannot be read degrades
/// to the built-in table and is reported as [`PHRASE_TABLE_UNAVAILABLE`] instead.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never.
pub fn install() -> Result<(), ImeError> {
    let layout = paths::ensure_dirs().ok();
    let configured = match layout.as_ref() {
        Some(layout) => configured_phrases(layout),
        None => Config::default().phrases,
    };
    let store = PhraseStore::from_config(&configured, layout.as_ref());
    if store.is_degraded() {
        emit_diagnostic(PHRASE_TABLE_UNAVAILABLE);
    }
    let _ = PHRASES.set(PhraseHandle::new(store));
    Ok(())
}

/// Merges the built-in document and the user's into the table the plugin runs on.
///
/// The built-in document is read first and the user's after it, which is the whole merge
/// rule: the reader keeps the last definition of a key, so a key the user defined wins.
/// A table that is switched off holds nothing, whatever the documents say.
///
/// # Parameters
///
/// - `built_in`: the document that ships with the plugin.
/// - `user`: the user's document, or `None` when it could not be read.
/// - `max_entries`: the entry limit the merged table is bounded by.
/// - `is_enabled`: whether the table takes part in a decode at all.
///
/// # Returns
///
/// The table and the counts reading the two documents produced. The counts describe the
/// merge rather than either document on its own, which is what a diagnostic wants: how
/// many entries the plugin has, and how many rows it could not use.
///
/// # Panics
///
/// Never.
fn merge_documents(
    built_in: &str,
    user: Option<&str>,
    max_entries: usize,
    is_enabled: bool,
) -> (PhraseTable, PhraseReport) {
    if !is_enabled {
        return (PhraseTable::empty(), PhraseReport::default());
    }
    let document = match user {
        Some(text) => format!("{built_in}{text}"),
        None => String::from(built_in),
    };
    PhraseTable::load(&document, max_entries)
}

/// The document `[phrases] file` names, resolved against the layout.
///
/// An empty value is the default location, and a relative one is relative to the
/// configuration directory -- which is where a user who wrote a bare file name means it,
/// and which is the directory the plugin's own `config.toml` lives in.
///
/// # Panics
///
/// Never.
fn document_path(configured: &str, layout: &Paths) -> PathBuf {
    let path = Path::new(configured);
    if path.as_os_str().is_empty() {
        return layout.config_dir.join(PHRASE_FILE_NAME);
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    layout.config_dir.join(path)
}

/// Reads the `[phrases]` section of the configuration document.
///
/// Read-only on purpose: writing the documented default, moving a corrupt file aside and
/// reporting why are the configuration loader's business, and the lifecycle step that
/// owns them is not this one. A document that is missing, unreadable or not TOML answers
/// the built-in defaults, which is the same degradation a user who never edited the file
/// runs with; the diagnostics a parse produced are dropped here for the same reason.
///
/// # Panics
///
/// Never.
fn configured_phrases(layout: &Paths) -> PhraseConfig {
    let Ok(text) = fs::read_to_string(&layout.config_file) else {
        return Config::default().phrases;
    };
    match Config::from_document(&text) {
        Ok((config, _warnings)) => config.phrases,
        Err(_) => Config::default().phrases,
    }
}

/// The entry limit as the reader wants it.
///
/// The configuration declares a `u32` and the table counts in `usize`; the conversion is
/// widened rather than narrowed, so it cannot fail on any target this plugin is built
/// for, and the saturation is what a target where it could would get.
///
/// # Panics
///
/// Never.
fn entry_limit(max_entries: u32) -> usize {
    usize::try_from(max_entries).unwrap_or(usize::MAX)
}

/// The diagnostic for a phrase that could not be saved.
///
/// `data/readonly-mode` is the contract's code for "the plugin has stopped writing": a
/// phrase document that cannot be written is the same degradation as a data directory
/// that cannot be (ASM-15), and the input method keeps working from the built-in table.
/// The reason says which half failed and names no path, because what reaches the user is
/// the stable code and never a filesystem location.
///
/// # Panics
///
/// Never.
fn unwritable(reason: &str) -> ImeError {
    ImeError::DataReadonly {
        reason: String::from(reason),
    }
}

#[cfg(test)]
mod tests;
