//! The deferred half of a saved phrase: the write, and the read-back that follows it,
//! happen on a thread of its own.
//!
//! # Responsibility
//!
//! Saving a phrase is one file write followed by a full read and parse of the document,
//! and both used to happen inside the Fcitx5 key callback that produced the save. A
//! callback must not touch the file system at all, and the read-back is the larger half
//! of the cost: it parses the built-in document and the user's every time, bounded only by
//! the entry limit. This module moves the pair onto a writer thread, so the host thread
//! pays one `try_send` and returns.
//!
//! # Boundaries
//!
//! The document format belongs to `ime-core`'s phrase module, and the merge rule and the
//! document's location belong to the parent module; this one owns the thread -- the queue
//! a row waits in, the write, the read-back, and the table the next decode reads. It never
//! decides which entry is worth saving (the caller renders the row, on the host thread, so
//! an entry the reader would refuse is refused before anything is queued) and it never
//! resolves a path.
//!
//! # What a save costs the host thread
//!
//! [`PhraseWriter::submit`] renders the row -- one small allocation, the one
//! [`ime_core::phrase::phrase_row`] already made -- and hands it to a bounded channel with
//! `try_send`. No file is opened and no document is parsed; the only lock this module takes
//! is the one around the sender slot, and it is never held across a wait.
//! [`PhraseWriter::snapshot`] is the same shape: one short lock and a pointer clone.
//!
//! # Losing a phrase
//!
//! A row that is queued is written: the writer drains the queue before it stops, so a
//! graceful shutdown loses nothing, and [`PhraseWriter::shutdown`] is the call that waits
//! for it. A process that ends without that call -- a crash, a `SIGKILL` -- loses the rows
//! still in the queue, which is the window the document has always had: nothing here
//! flushes with `fsync`, and a phrase lost that way is one the user can type again. What
//! is not allowed is a *silent* loss, and there is none: a row the queue would not take is
//! refused to the caller, and a write the document refuses is reported on the diagnostic
//! channel.
//!
//! # Why a channel and not the UI thread's `eventfd`
//!
//! The wakeup primitive `ime-ui` owns is the right one for the frame path, but this crate
//! is the engine addon and deliberately does not link the view layer: the two roles are
//! separate cdylibs (`ADR-0003`). [`std::sync::mpsc`] gives the same properties here -- a
//! producer that never blocks, a consumer parked in the kernel and woken only by a send,
//! no polling timer anywhere -- without a dependency edge, and dropping the sender is
//! itself the stop signal, so the writer needs no second mechanism to notice a shutdown.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ime_core::phrase::{PHRASE_TABLE_UNAVAILABLE, PhraseReport, PhraseTable, phrase_row};
use ime_dict::paths::{self, FILE_MODE};
use ime_types::ImeError;

use crate::ffi::emit_diagnostic;

use super::{
    BUILT_IN_DOCUMENT, NO_DOCUMENT_REASON, PHRASE_LIMIT_EXCEEDED_CODE, WRITE_FAILED_REASON,
    merge_documents, unwritable,
};

/// How many rows may wait for the writer before a save is refused.
///
/// A row is one line of a user's own document and a user saves one by choosing a candidate
/// and pressing a key, so the queue can only fill if the writer is far behind. The ceiling
/// exists so that a writer which has stopped draining cannot grow the plugin's memory
/// without bound, and a queue at the ceiling refuses the *new* row and says so: dropping an
/// older one instead would lose a phrase the user was already told had been saved.
pub(super) const OUTBOX_CAPACITY: usize = 64;

/// The name the writer thread runs under, so a stack dump names it.
const WRITER_THREAD_NAME: &str = "rspinyin-phrases";

/// The name the thread that waits for the writer runs under.
const JOIN_THREAD_NAME: &str = "rspinyin-phrases-join";

/// Why a phrase could not be handed to the writer: there is no writer to hand it to.
///
/// Shared with the parent module, which has the same failure when the writer thread could
/// not be started at all: a store without a writer still serves the table it loaded, and
/// this is the reason a save is refused with.
pub(super) const WRITER_GONE_REASON: &str = "the phrase writer is not running";

/// Why a phrase could not be handed to the writer: the writer is a whole queue behind.
const WRITER_BEHIND_REASON: &str = "the phrase writer is a queue behind";

/// The table in force, and what reading the documents produced.
///
/// One value rather than three fields because a commit replaces all three together: a
/// reader that took the table from one read and the counts from another could report a
/// table the counts do not describe.
#[derive(Clone, Debug)]
pub(super) struct PhraseSnapshot {
    /// The merged table the next decode matches against.
    pub(super) table: Arc<PhraseTable>,
    /// What reading the documents produced: how many entries the table holds and how much
    /// of the user's document was skipped, overridden or dropped at the limit.
    pub(super) report: PhraseReport,
    /// Whether the user's document was missing or could not be read.
    pub(super) is_degraded: bool,
}

/// A document a deferred write lands in.
///
/// A trait rather than a direct call so that the writer can be driven by a test without a
/// file: a test that asserted on a real document would be asserting on a thread's schedule
/// as well as on a decision. The production implementation is [`DocumentSink`].
pub(super) trait PhraseSink: Send {
    /// Appends `row` to the document and returns what it now holds.
    ///
    /// Called on the writer thread, never on the host thread, and never concurrently: the
    /// writer is a single consumer draining a queue in order.
    ///
    /// # Errors
    ///
    /// [`ImeError::DataReadonly`] when there is no document to write to, or when the
    /// document refused the write. The reason says which half failed and names no path.
    ///
    /// # Panics
    ///
    /// Never.
    fn commit(&mut self, row: &str) -> Result<PhraseSnapshot, ImeError>;
}

/// The user's phrase document, as the writer thread holds it.
///
/// Holds everything a read needs and nothing else: the location, the entry limit and
/// whether the table takes part in a decode at all. The merge rule itself stays with the
/// parent module, which owns the built-in document.
pub(super) struct DocumentSink {
    /// Where the user's document lives, or `None` when the configuration and the layout
    /// offer none, in which case there is nothing to write to.
    path: Option<PathBuf>,
    /// The most entries the merged table may hold.
    max_entries: usize,
    /// Whether the phrase table takes part in a decode at all (`[phrases] enabled`).
    is_enabled: bool,
}

impl DocumentSink {
    /// Builds the sink for the document at `path`.
    ///
    /// # Parameters
    ///
    /// - `path`: the user's document, or `None` when there is none to read or write.
    /// - `max_entries`: the most entries the merged table may hold.
    /// - `is_enabled`: `[phrases] enabled`. With it off the table holds nothing, because
    ///   the key says the user wants no phrase to take part in a decode. The document is
    ///   still read, so the setting never costs the user an entry they wrote.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn new(path: Option<PathBuf>, max_entries: usize, is_enabled: bool) -> Self {
        Self {
            path,
            max_entries,
            is_enabled,
        }
    }

    /// Reads the user's document and builds the merged table.
    ///
    /// The one place the document is read, and it runs on whichever thread asks: the
    /// startup sequence reads once, synchronously, so that the first key has a table to
    /// match against; every read after that happens on the writer thread, after a row has
    /// been appended.
    ///
    /// A document that is missing or unreadable is not an error: the table becomes the
    /// built-in one and the degradation is carried in the snapshot for the caller that
    /// reports it.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn read(&self) -> PhraseSnapshot {
        let user = match self.path.as_deref() {
            Some(path) => fs::read_to_string(path).ok(),
            None => None,
        };
        let (table, report) = merge_documents(
            BUILT_IN_DOCUMENT,
            user.as_deref(),
            self.max_entries,
            self.is_enabled,
        );
        PhraseSnapshot {
            table: Arc::new(table),
            report,
            is_degraded: user.is_none(),
        }
    }
}

impl PhraseSink for DocumentSink {
    /// Appends one row and reads the document back into a table.
    ///
    /// The row is written first and the document is read after it, so the table the next
    /// decode sees holds the entry the user just saved.
    fn commit(&mut self, row: &str) -> Result<PhraseSnapshot, ImeError> {
        let Some(path) = self.path.as_deref() else {
            return Err(unwritable(NO_DOCUMENT_REASON));
        };
        append_row(path, row).map_err(|_| unwritable(WRITE_FAILED_REASON))?;
        Ok(self.read())
    }
}

/// The phrase document's writer thread, as the host thread holds it.
///
/// Cloning is not needed and not offered: the writer is created once by the startup
/// sequence and reached through the handle that holds it.
pub(super) struct PhraseWriter {
    /// The state the host thread and the writer thread share.
    inner: Arc<WriterInner>,
}

/// What the host thread and the writer thread share.
///
/// The thread holds a [`Weak`] to this rather than an `Arc`, because the join handle below
/// points back at the thread: a strong handle on both sides would keep the writer alive
/// for as long as the process runs, and the drop that stops it would never happen.
struct WriterInner {
    /// The rows waiting to be written, or `None` once [`PhraseWriter::shutdown`] has taken
    /// the sender. Dropping it is the stop signal: the writer reads what is left out of the
    /// queue and then sees the channel closed.
    outbox: Mutex<Option<SyncSender<String>>>,
    /// The table in force and what reading produced.
    published: Mutex<PhraseSnapshot>,
    /// The writer thread, until `shutdown` takes it to wait for it.
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl PhraseWriter {
    /// Starts the writer over `sink`, publishing `initial` until the first row lands.
    ///
    /// # Parameters
    ///
    /// - `sink`: the document a row is committed to. It is moved onto the writer thread and
    ///   is never reached from the host thread again.
    /// - `initial`: the snapshot the startup sequence read, which is what the host thread
    ///   answers with until the first commit replaces it.
    ///
    /// # Returns
    ///
    /// The writer, or `None` when the thread could not be started. A store without a writer
    /// still serves the table it loaded; it only stops accepting new rows, which
    /// [`PhraseWriter::submit`] reports as read-only mode.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn start(sink: Box<dyn PhraseSink>, initial: PhraseSnapshot) -> Option<Self> {
        Self::start_with(sink, initial, OUTBOX_CAPACITY)
    }

    /// Starts the writer over `sink` with a queue of `capacity` rows.
    ///
    /// The ceiling is a parameter so that a test can reach it: a queue sized for a user who
    /// saves a phrase by hand is not one a test can fill. `capacity` must be at least one;
    /// a zero-length queue is a rendezvous and would refuse every row.
    ///
    /// # Panics
    ///
    /// Never.
    #[cfg(test)]
    pub(super) fn start_with_capacity(
        sink: Box<dyn PhraseSink>,
        initial: PhraseSnapshot,
        capacity: usize,
    ) -> Option<Self> {
        Self::start_with(sink, initial, capacity)
    }

    /// Starts the writer thread over a queue of `capacity` rows.
    fn start_with(
        sink: Box<dyn PhraseSink>,
        initial: PhraseSnapshot,
        capacity: usize,
    ) -> Option<Self> {
        let (sender, outbox) = sync_channel(capacity);
        let inner = Arc::new(WriterInner {
            outbox: Mutex::new(Some(sender)),
            published: Mutex::new(initial),
            handle: Mutex::new(None),
        });
        let writer = Arc::downgrade(&inner);
        let thread = thread::Builder::new()
            .name(String::from(WRITER_THREAD_NAME))
            .spawn(move || run(sink, &writer, &outbox))
            .ok()?;
        *lock(&inner.handle) = Some(thread);
        Some(Self { inner })
    }

    /// Hands one row to the writer.
    ///
    /// # Returns
    ///
    /// `Ok(())` once the row is queued. The row is not on disk when this returns: the
    /// writer appends it and reads the document back on its own thread, which is the whole
    /// point of this module. A caller that reports "saved" on `Ok` is reporting that the
    /// row will be written, and every outcome that would stop it is reported separately.
    ///
    /// # Parameters
    ///
    /// - `key`: the shortcut the user typed, in the key alphabet the session folded it to.
    /// - `text`: the text the key commits, which is the candidate the highlight was on.
    ///
    /// # Errors
    ///
    /// [`ImeError::ConfigInvalid`] naming `phrases.file` when the entry is not one the
    /// reader would accept. It is rendered here, on the host thread, so an entry the reader
    /// would refuse is refused before anything is queued. [`ImeError::DataReadonly`] when
    /// there is no writer to hand the row to, or when the writer is a whole queue behind.
    /// Neither carries the phrase's text or a path.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn submit(&self, key: &str, text: &str) -> Result<(), ImeError> {
        let row = phrase_row(key, text)?;
        let outbox = lock(&self.inner.outbox);
        let Some(sender) = outbox.as_ref() else {
            return Err(unwritable(WRITER_GONE_REASON));
        };
        match sender.try_send(row) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(unwritable(WRITER_BEHIND_REASON)),
            Err(TrySendError::Disconnected(_)) => Err(unwritable(WRITER_GONE_REASON)),
        }
    }

    /// The table in force, and what reading the documents produced.
    ///
    /// A clone of the `Arc` rather than a copy of the table: a decode that is already
    /// running keeps the snapshot it started with when a commit replaces this one, which is
    /// what keeps a save from disturbing an input session in progress.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn snapshot(&self) -> PhraseSnapshot {
        lock(&self.inner.published).clone()
    }

    /// Stops the writer, waiting at most `timeout` for it to finish.
    ///
    /// The queue is drained before the thread stops, so every row that was accepted is
    /// written: this is the call that makes a graceful shutdown lose nothing. The sender is
    /// dropped rather than a stop message sent, which is both the signal and the reason the
    /// writer cannot miss it -- a channel with no sender left is a wakeup, not a poll.
    ///
    /// Idempotent: a second call after the thread has stopped returns `true` at once. A
    /// writer that does not stop within the timeout is detached and the answer is `false`;
    /// the caller is never held up for longer than it asked for.
    ///
    /// # Returns
    ///
    /// Whether the writer stopped within `timeout`.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn shutdown(&self, timeout: Duration) -> bool {
        // The sender goes first, so the writer is already draining what is queued while the
        // wait below begins.
        let _ = lock(&self.inner.outbox).take();
        let Some(handle) = lock(&self.inner.handle).take() else {
            return true;
        };
        join_within(handle, timeout)
    }
}

impl Drop for WriterInner {
    /// Stops the writer without waiting for it.
    ///
    /// Dropping the sender is the stop request -- the writer reads what is left out of the
    /// queue and then sees the channel closed -- and the join handle is dropped without
    /// being joined, because a thread that is still writing must never hold up whatever is
    /// tearing the plugin down. [`PhraseWriter::shutdown`] is the call that waits.
    fn drop(&mut self) {
        if let Ok(slot) = self.outbox.get_mut() {
            let _ = slot.take();
        }
        if let Ok(slot) = self.handle.get_mut() {
            let _ = slot.take();
        }
    }
}

/// The writer thread's body: one commit per row, in the order the rows arrived.
///
/// The loop ends when the queue is empty *and* every sender is gone, which is what makes
/// the last row of a shutdown land: `recv` yields what is buffered before it reports the
/// channel closed.
fn run(mut sink: Box<dyn PhraseSink>, inner: &Weak<WriterInner>, outbox: &Receiver<String>) {
    while let Ok(row) = outbox.recv() {
        match sink.commit(&row) {
            Ok(snapshot) => publish(inner, snapshot),
            // The one failure a sink reports is the document refusing the write. The
            // callback that asked for the save returned long ago, so the failure travels on
            // the crash channel -- the one sink that exists before the diagnostics layer
            // does -- under the contract's code for "the plugin has stopped writing". The
            // row is gone, and this is what keeps that from being silent. Neither the key
            // nor the phrase's text reaches the line.
            Err(_) => emit_diagnostic(paths::READONLY_CODE),
        }
    }
}

/// Publishes what a commit produced, and reports what the read found.
///
/// The reports mirror the ones the synchronous save emitted: a document that could not be
/// read back, and a document holding more rows than the table may carry. The table is
/// published either way, a read that failed leaving the built-in rows in force, which is
/// the degradation the store has always reported.
fn publish(inner: &Weak<WriterInner>, snapshot: PhraseSnapshot) {
    if snapshot.is_degraded {
        emit_diagnostic(PHRASE_TABLE_UNAVAILABLE);
    }
    if snapshot.report.over_limit > 0 {
        emit_diagnostic(PHRASE_LIMIT_EXCEEDED_CODE);
    }
    let Some(inner) = inner.upgrade() else {
        // The last handle is gone, so there is no reader left to publish to. The row is on
        // disk either way, which is what the save promised.
        return;
    };
    *lock(&inner.published) = snapshot;
}

/// Waits for the writer thread to finish, for at most `timeout`.
///
/// [`JoinHandle::join`] has no timeout and the host must never be held up by a thread that
/// will not stop, so the join happens on a helper thread and this one waits on a channel
/// instead -- the shape the UI thread's shutdown uses, for the same reason. On timeout the
/// helper is detached: it stays parked on the join and disappears with the process, which
/// is strictly better than making the caller's exit wait for it.
///
/// # Returns
///
/// Whether the writer finished within `timeout`. A helper thread that cannot be started is
/// reported the same way, because nothing can then be waited for.
fn join_within(handle: JoinHandle<()>, timeout: Duration) -> bool {
    let (finished, done) = mpsc::channel();
    let helper = thread::Builder::new()
        .name(String::from(JOIN_THREAD_NAME))
        .spawn(move || {
            let _ = handle.join();
            let _ = finished.send(());
        });
    match helper {
        Ok(_detached) => done.recv_timeout(timeout).is_ok(),
        Err(_) => false,
    }
}

/// Borrows a mutex, recovering the contents of a poisoned lock.
///
/// Poisoning means a holder panicked. The state here is a queue, a snapshot and a join
/// handle; the worst a recovered one can do is answer with the table it last published, and
/// refusing to answer would cost the user their phrases for the rest of the process. The
/// phrase handle and the diagnostic throttle are recovered the same way, for the same
/// reason.
///
/// # Panics
///
/// Never.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Appends one row to the document at `path`, creating it private when it is new.
///
/// One `open`, one narrowing and one `write`, and no read at all: this is the whole cost a
/// saved phrase pays on the writer thread. The mode is passed to `open` rather than applied
/// afterwards, so the document never exists -- not even for one syscall -- with the umask's
/// default; a file that is already there is narrowed through [`paths::chmod_private`], the
/// same rule the rest of the user's data follows, and a file that cannot be narrowed is not
/// written to.
///
/// # Errors
///
/// The underlying [`std::io::Error`] when the document cannot be opened, narrowed or
/// written. The caller decides what that means -- a phrase that cannot be saved degrades to
/// read-only mode -- so the error is left as it is rather than wrapped in a variant this
/// layer cannot choose.
fn append_row(path: &Path, row: &str) -> std::io::Result<()> {
    let mut document = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(FILE_MODE)
        .open(path)?;
    paths::chmod_private(path)?;
    document.write_all(row.as_bytes())
}

#[cfg(test)]
mod tests;
