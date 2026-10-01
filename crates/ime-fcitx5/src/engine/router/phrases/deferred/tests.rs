//! Unit tests for the deferred write path.
//!
//! The writer runs on a thread of its own, so a test that drove the production sink would
//! be asserting on a schedule as well as on a decision. Every test below but the ones that
//! cover [`DocumentSink`] therefore drives the writer through a sink of its own, and every
//! wait is on a condition the sink signals rather than on elapsed time. The one exception
//! is the wedged-writer test, whose subject is the bound itself: it asserts the teardown's
//! wait ended inside the budget the unload pays, which is a fact about time on purpose.
//!
//! A document is a file, so the [`DocumentSink`] tests write one: each takes a directory of
//! its own under the system's temporary directory, empties it first and leaves it behind,
//! which is the shape the parent module's tests use for the same reason.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ime_core::phrase::{PhraseReport, PhraseTable};
use ime_dict::paths::FILE_MODE;
use ime_types::ImeError;

use crate::addon::{DESTROY_BUDGET, PHRASE_DRAIN_BUDGET};

use super::super::BUILT_IN_DOCUMENT;
use super::{DocumentSink, PhraseSink, PhraseSnapshot, PhraseWriter, WRITER_THREAD_NAME};

/// How long a test waits for the writer before it gives up.
///
/// Generous on purpose: the assertions are about ordering and counting, not about how fast
/// a loaded machine is, and a shorter bound would only make the suite flaky.
const PATIENCE: Duration = Duration::from_secs(5);

/// The ceiling one host callback may spend.
///
/// A key callback runs on the fcitx5 main loop, which is why the bound is orders of
/// magnitude below a frame interval: `submit` is one row rendered and one `try_send`, and
/// the commit that follows it belongs to the writer thread. A `submit` that waited for the
/// writer would miss this bound by thousands of times.
const HOST_CALLBACK_BUDGET: Duration = Duration::from_micros(200);

/// How many submissions the budget assertion samples.
///
/// The bound is on the work rather than on the scheduler: a test thread that is descheduled
/// in the middle of a call measures the scheduler, so the fastest sample is the one
/// asserted. A `submit` that performed the commit itself cannot pass at any sample count,
/// because every one of its samples would have to wait for the sink.
const BUDGET_SAMPLES: usize = 16;

/// A latch a test can hold the writer on, so that the queue behind it can be filled.
///
/// The queue's ceiling is only reachable while the writer is not draining, and a fixed
/// delay would only keep the writer away for as long as the machine happened to cooperate:
/// the assertion would become a guess about scheduling. The latch makes it a fact.
#[derive(Default)]
struct Latch {
    /// Whether a commit has reached the latch.
    entered: Mutex<bool>,
    /// Signals a test that a commit reached the latch.
    on_entered: Condvar,
    /// Whether the latch has been opened.
    opened: Mutex<bool>,
    /// Signals a held commit that it may continue.
    on_opened: Condvar,
}

impl Latch {
    /// Blocks the calling thread until the test opens the latch.
    fn hold(&self) {
        {
            let mut entered = self
                .entered
                .lock()
                .expect("the latch lock is never poisoned by a test");
            *entered = true;
        }
        self.on_entered.notify_all();
        let mut opened = self
            .opened
            .lock()
            .expect("the latch lock is never poisoned by a test");
        while !*opened {
            opened = self
                .on_opened
                .wait(opened)
                .expect("the latch lock is never poisoned by a test");
        }
    }

    /// Waits until a commit has reached the latch.
    fn wait_entered(&self) {
        let deadline = Instant::now() + PATIENCE;
        let mut entered = self
            .entered
            .lock()
            .expect("the latch lock is never poisoned by a test");
        while !*entered {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (guard, _) = self
                .on_entered
                .wait_timeout(entered, remaining)
                .expect("the latch lock is never poisoned by a test");
            entered = guard;
        }
        assert!(*entered, "a commit reached the latch");
    }

    /// Lets the held commit through.
    fn open(&self) {
        {
            let mut opened = self
                .opened
                .lock()
                .expect("the latch lock is never poisoned by a test");
            *opened = true;
        }
        self.on_opened.notify_all();
    }
}

/// The state a recording sink shares with the test that drives it.
#[derive(Default)]
struct Observed {
    /// The rows the sink was handed, in order.
    rows: Mutex<Vec<String>>,
    /// Signals a test that a row arrived.
    on_row: Condvar,
    /// The latch a commit holds, when the test asked for one.
    latch: Option<Arc<Latch>>,
    /// How many of the next rows the sink refuses, standing in for a document that cannot
    /// be written.
    refusals: Mutex<usize>,
    /// The name of the thread each commit ran on, in order.
    ///
    /// The sink stands in for the document, so the thread it is entered on is the thread the
    /// file IO would have run on. Recording the name is what lets a test assert that no save
    /// reaches the document from the caller's own thread.
    threads: Mutex<Vec<Option<String>>>,
}

impl Observed {
    /// Waits until at least `count` rows have been recorded, and returns what was recorded.
    fn wait_for(&self, count: usize) -> Vec<String> {
        let deadline = Instant::now() + PATIENCE;
        let mut rows = self
            .rows
            .lock()
            .expect("the rows lock is never poisoned by a test");
        while rows.len() < count {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (guard, _) = self
                .on_row
                .wait_timeout(rows, remaining)
                .expect("the rows lock is never poisoned by a test");
            rows = guard;
        }
        rows.clone()
    }

    /// The names of the threads the commits ran on, in order.
    fn threads(&self) -> Vec<Option<String>> {
        self.threads
            .lock()
            .expect("the thread lock is never poisoned by a test")
            .clone()
    }
}

/// A sink that records what it is handed instead of writing a document.
struct Recorder {
    observed: Arc<Observed>,
}

impl Recorder {
    fn new(observed: Arc<Observed>) -> Self {
        Self { observed }
    }
}

impl PhraseSink for Recorder {
    /// Records the row and answers the table the recorded document reads as.
    ///
    /// The table is built by the real reader, so a test that asserts on the published
    /// snapshot is asserting on what a decode would match, not on a stub.
    fn commit(&mut self, row: &str) -> Result<PhraseSnapshot, ImeError> {
        self.observed
            .threads
            .lock()
            .expect("the thread lock is never poisoned by a test")
            .push(thread::current().name().map(String::from));
        if let Some(latch) = &self.observed.latch {
            latch.hold();
        }
        let refused = {
            let mut refusals = self
                .observed
                .refusals
                .lock()
                .expect("the refusal lock is never poisoned by a test");
            if *refusals > 0 {
                *refusals -= 1;
                true
            } else {
                false
            }
        };
        if refused {
            return Err(ImeError::DataReadonly {
                reason: String::from("the sink refuses this row"),
            });
        }
        let document = {
            let mut rows = self
                .observed
                .rows
                .lock()
                .expect("the rows lock is never poisoned by a test");
            rows.push(String::from(row));
            rows.concat()
        };
        self.observed.on_row.notify_all();
        let (table, report) = PhraseTable::load(&document, 100);
        Ok(PhraseSnapshot {
            table: Arc::new(table),
            report,
            is_degraded: false,
        })
    }
}

/// A recording sink and the state the test reads it through.
fn recording() -> (Arc<Observed>, Recorder) {
    let observed = Arc::new(Observed::default());
    let sink = Recorder::new(Arc::clone(&observed));
    (observed, sink)
}

/// A recording sink whose first commit holds `latch`, together with that latch.
fn recording_gated() -> (Arc<Observed>, Recorder, Arc<Latch>) {
    let latch = Arc::new(Latch::default());
    let observed = Arc::new(Observed {
        latch: Some(Arc::clone(&latch)),
        ..Observed::default()
    });
    let sink = Recorder::new(Arc::clone(&observed));
    (observed, sink, latch)
}

/// The snapshot a writer is started with: the table an empty document produces.
fn initial() -> PhraseSnapshot {
    PhraseSnapshot {
        table: Arc::new(PhraseTable::empty()),
        report: PhraseReport::default(),
        is_degraded: false,
    }
}

/// Waits until `condition` holds, answering whether it did.
///
/// The writer publishes on its own thread, so a test has to give it the chance to; a fixed
/// sleep would be a guess about how long that takes, and this is a bounded wait on the very
/// condition the test asserts.
fn wait_until(condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::yield_now();
    }
    condition()
}

/// An empty directory of this test's own.
///
/// Emptied rather than merely created, so that a second run of the suite does not read what
/// the first one left behind. The directory is not removed afterwards: it holds nothing a
/// later run depends on, and a test that fails leaves its evidence in place.
fn scratch_dir(label: &str) -> PathBuf {
    let name = format!("rspinyin-deferred-{}-{label}", std::process::id());
    let dir = std::env::temp_dir().join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// How many entries the built-in document alone contributes to a table of `limit` entries.
fn built_in_entries(limit: usize) -> usize {
    usize::try_from(PhraseTable::load(BUILT_IN_DOCUMENT, limit).0.len()).unwrap_or(usize::MAX)
}

#[test]
fn test_submit_returns_while_the_writer_is_still_committing() {
    // The whole point of the module: the host thread's call returns while the writer is
    // still inside its commit. The latch holds the writer on the first row, and `submit`
    // returning at all is the assertion -- a `submit` that waited for the write would hang
    // here rather than fail.
    let (observed, sink, latch) = recording_gated();
    let writer = PhraseWriter::start(Box::new(sink), initial()).expect("the writer starts");

    assert!(
        writer.submit("rq", "2026-09-29").is_ok(),
        "the row is queued"
    );
    latch.wait_entered();
    let recorded = observed.rows.lock().expect("the rows lock is clean").len();
    assert_eq!(recorded, 0, "the writer has not finished the row yet");

    latch.open();
    assert_eq!(observed.wait_for(1), vec!["rq\t2026-09-29\n"]);
    assert!(writer.shutdown(PATIENCE), "the writer stops");
}

#[test]
fn test_submit_stays_within_the_host_callback_budget_while_the_writer_is_busy() {
    // The host thread pays one `try_send` and returns, whatever the writer is doing. The
    // writer is held inside its first commit, so a `submit` that performed the commit -- the
    // synchronous shape this module replaced -- would measure the latch rather than the
    // queue and miss the bound below by three orders of magnitude.
    let (observed, sink, latch) = recording_gated();
    let writer = PhraseWriter::start(Box::new(sink), initial()).expect("the writer starts");
    assert!(
        writer.submit("aa", "one").is_ok(),
        "the writer takes the first row"
    );
    latch.wait_entered();
    assert!(
        observed.wait_for(0).is_empty(),
        "the writer is parked inside its commit"
    );

    let mut fastest = Duration::MAX;
    for _ in 0..BUDGET_SAMPLES {
        let start = Instant::now();
        let queued = writer.submit("bb", "two");
        fastest = fastest.min(start.elapsed());
        assert!(queued.is_ok(), "the queue holds every sample");
    }
    assert!(
        fastest <= HOST_CALLBACK_BUDGET,
        "the fastest submit cost {fastest:?}, past the {HOST_CALLBACK_BUDGET:?} a host callback may spend"
    );
    assert!(
        observed.wait_for(0).is_empty(),
        "the host thread committed nothing itself"
    );

    latch.open();
    assert!(writer.shutdown(PATIENCE), "the writer stops");
    assert_eq!(
        observed.wait_for(BUDGET_SAMPLES + 1).len(),
        BUDGET_SAMPLES + 1,
        "every accepted row is written once the writer is let go"
    );
}

#[test]
fn test_commit_never_runs_on_the_callers_thread() {
    // The deferred contract in one assertion: every commit ran on the writer thread, and the
    // thread that submitted the rows never entered the sink -- which is where the document
    // is opened, written and read back.
    let (observed, sink) = recording();
    let writer = PhraseWriter::start(Box::new(sink), initial()).expect("the writer starts");
    for (key, text) in [("aa", "one"), ("bb", "two")] {
        assert!(writer.submit(key, text).is_ok(), "the row is queued");
    }
    assert!(writer.shutdown(PATIENCE), "the writer stops");
    assert_eq!(observed.wait_for(2).len(), 2, "both rows reached the sink");

    let expected = vec![Some(String::from(WRITER_THREAD_NAME)); 2];
    assert_eq!(
        observed.threads(),
        expected,
        "every commit ran on the writer thread"
    );
    let caller = thread::current().name().map(String::from);
    assert!(
        !observed.threads().contains(&caller),
        "the caller's own thread never entered the sink"
    );
}

#[test]
fn test_submit_refuses_a_row_when_the_queue_is_full() {
    // The boundary the ceiling exists for. The writer is held on the first row, so the
    // queue behind it can be filled exactly: one row waits, and the next has nowhere to go.
    let (observed, sink, latch) = recording_gated();
    let writer =
        PhraseWriter::start_with_capacity(Box::new(sink), initial(), 1).expect("the writer starts");

    assert!(
        writer.submit("aa", "one").is_ok(),
        "the writer takes the first row"
    );
    latch.wait_entered();
    assert!(
        writer.submit("bb", "two").is_ok(),
        "the queue holds one row while the writer is busy"
    );

    let refused = writer.submit("cc", "three");
    assert!(
        refused.is_err(),
        "a full queue refuses the new row rather than dropping one already accepted"
    );
    if let Err(error) = refused {
        let rendered = error.to_string();
        assert!(rendered.starts_with("data/readonly-mode: "), "{rendered}");
        assert!(matches!(error, ImeError::DataReadonly { .. }));
    }

    latch.open();
    assert_eq!(
        observed.wait_for(2),
        vec!["aa\tone\n", "bb\ttwo\n"],
        "the two rows the queue took are written, in order"
    );
    assert!(writer.shutdown(PATIENCE), "the writer stops");
}

#[test]
fn test_submit_refuses_an_entry_the_reader_would_refuse() {
    let (observed, sink) = recording();
    let writer = PhraseWriter::start(Box::new(sink), initial()).expect("the writer starts");

    let refused = writer.submit("r q", "2026-09-29");
    assert!(refused.is_err(), "a key outside the alphabet is refused");
    if let Err(error) = refused {
        let rendered = error.to_string();
        assert!(
            rendered.starts_with("config/invalid: phrases.file ("),
            "{rendered}"
        );
        assert!(!rendered.contains("2026-09-29"), "the entry is not quoted");
    }

    assert!(
        observed.wait_for(0).is_empty(),
        "a refused entry never reaches the queue"
    );
    assert!(writer.shutdown(PATIENCE), "the writer stops");
}

#[test]
fn test_submit_after_shutdown_reports_read_only_mode() {
    let (observed, sink) = recording();
    let writer = PhraseWriter::start(Box::new(sink), initial()).expect("the writer starts");
    assert!(writer.shutdown(PATIENCE), "the writer stops");

    let refused = writer.submit("rq", "2026-09-29");
    assert!(refused.is_err(), "a stopped writer takes no more rows");
    if let Err(error) = refused {
        let rendered = error.to_string();
        assert!(rendered.starts_with("data/readonly-mode: "), "{rendered}");
        let reason = rendered
            .strip_prefix("data/readonly-mode: ")
            .expect("the rendering carries the code and then the reason");
        assert!(!reason.contains('/'), "the reason names no path: {reason}");
    }
    assert!(observed.wait_for(0).is_empty(), "nothing was written");
}

#[test]
fn test_shutdown_writes_the_rows_that_were_still_queued() {
    // The graceful-stop guarantee: the queue is drained before the thread exits, so a row
    // that was accepted is written even when the stop arrives first.
    let (observed, sink) = recording();
    let writer = PhraseWriter::start(Box::new(sink), initial()).expect("the writer starts");
    for (key, text) in [("aa", "one"), ("bb", "two"), ("cc", "three")] {
        assert!(writer.submit(key, text).is_ok(), "the row is queued");
    }

    assert!(writer.shutdown(PATIENCE), "the writer stops");
    assert_eq!(
        observed.wait_for(3),
        vec!["aa\tone\n", "bb\ttwo\n", "cc\tthree\n"],
        "every accepted row is on the document after the writer stopped"
    );
}

#[test]
fn test_shutdown_of_a_stopped_writer_answers_at_once() {
    let (_, sink) = recording();
    let writer = PhraseWriter::start(Box::new(sink), initial()).expect("the writer starts");
    assert!(
        writer.shutdown(PATIENCE),
        "the first stop waits for the thread"
    );
    assert!(
        writer.shutdown(Duration::ZERO),
        "a writer that already stopped is not waited for a second time"
    );
}

#[test]
fn test_shutdown_of_a_wedged_writer_times_out_inside_the_unload_budget() {
    // A disk that never answers: the sink holds its first commit open, so the writer is
    // parked inside it while the unload asks it to stop. The teardown answers within the
    // budget the destroy sequence pays for this step -- never longer -- and the `false`
    // answer is what the teardown records the timeout from. The one elapsed-time
    // assertion in this file is the point of the test: the bound is its subject.
    let (observed, sink, latch) = recording_gated();
    let writer = PhraseWriter::start(Box::new(sink), initial()).expect("the writer starts");
    assert!(
        writer.submit("aa", "one").is_ok(),
        "the writer takes the row the disk is stuck on"
    );
    latch.wait_entered();
    assert!(
        writer.submit("bb", "two").is_ok(),
        "the row behind the stuck one is queued"
    );

    let started = Instant::now();
    let stopped = writer.shutdown(PHRASE_DRAIN_BUDGET);
    let elapsed = started.elapsed();
    assert!(
        !stopped,
        "a writer parked on a stuck disk cannot answer within the budget"
    );
    assert!(
        elapsed <= DESTROY_BUDGET,
        "the wait ran {elapsed:?}, past the {DESTROY_BUDGET:?} the unload may spend in total"
    );

    // The detached writer is not abandoned: the drain dropped its handle without joining,
    // and the thread drains every row it accepted once the disk lets go -- the property
    // that keeps a timed-out unload a late write rather than a lost one.
    latch.open();
    assert_eq!(
        observed.wait_for(2),
        vec!["aa\tone\n", "bb\ttwo\n"],
        "the detached writer still writes the rows it accepted"
    );
}

#[test]
fn test_snapshot_publishes_the_table_a_commit_produced() {
    let (observed, sink) = recording();
    let writer = PhraseWriter::start(Box::new(sink), initial()).expect("the writer starts");
    assert!(
        writer.snapshot().table.is_empty(),
        "before the first commit the snapshot is the one the writer started with"
    );

    assert!(
        writer.submit("rq", "2026-09-29").is_ok(),
        "the row is queued"
    );
    assert_eq!(
        observed.wait_for(1).len(),
        1,
        "the writer committed the row"
    );
    let published = wait_until(|| !writer.snapshot().table.is_empty());
    assert!(published, "the commit publishes the table it read back");

    let snapshot = writer.snapshot();
    let hit = snapshot.table.longest_match("rq", 0);
    assert_eq!(
        hit.map(|hit| snapshot.table.text(hit)),
        Some("2026-09-29"),
        "the published table matches the row that was saved"
    );
    assert_eq!(snapshot.report.loaded, 1);
    assert!(
        !snapshot.is_degraded,
        "the document that was written reads back"
    );
    assert!(writer.shutdown(PATIENCE), "the writer stops");
}

#[test]
fn test_snapshot_before_any_commit_answers_the_table_it_started_with() {
    // The boundary of `snapshot`: a writer that has committed nothing must still answer,
    // and must answer with exactly what the startup read produced -- including its report
    // and its degradation.
    let (_, sink) = recording();
    let started = PhraseSnapshot {
        table: Arc::new(PhraseTable::load("rq\tbuilt-in\n", 100).0),
        report: PhraseReport::default(),
        is_degraded: true,
    };
    let writer = PhraseWriter::start(Box::new(sink), started).expect("the writer starts");

    let snapshot = writer.snapshot();
    assert!(
        snapshot.is_degraded,
        "the degradation the read reported is kept"
    );
    assert_eq!(snapshot.table.len(), 1);
    assert_eq!(snapshot.report.loaded, 0);
    assert!(writer.shutdown(PATIENCE), "the writer stops");
}

#[test]
fn test_writer_keeps_committing_after_a_document_refuses_a_row() {
    // A save that cannot land must not stop the next one: every save is an independent
    // append, and a disk that was full for one row is not a store that has to be given up.
    let observed = Arc::new(Observed {
        refusals: Mutex::new(1),
        ..Observed::default()
    });
    let writer = PhraseWriter::start(Box::new(Recorder::new(Arc::clone(&observed))), initial())
        .expect("the writer starts");

    assert!(
        writer.submit("aa", "one").is_ok(),
        "the first row is queued"
    );
    assert!(
        writer.submit("bb", "two").is_ok(),
        "the second row is queued"
    );
    assert!(writer.shutdown(PATIENCE), "the writer stops");

    assert_eq!(
        observed.wait_for(1),
        vec!["bb\ttwo\n"],
        "the row the document took is written, and the refused one is not"
    );
}

#[test]
fn test_document_sink_appends_the_row_and_reads_the_document_back() {
    // The two halves of the format meet here: the row is rendered by `ime-core`'s writer,
    // written to a file, and read back by this layer into a table.
    let dir = scratch_dir("append");
    let document = dir.join("phrases.tsv");
    let mut sink = DocumentSink::new(Some(document.clone()), 100, true);
    assert!(
        sink.read().is_degraded,
        "there is no document until one is written"
    );

    let snapshot = sink.commit("zzz\t2026-09-29\n").expect("the row lands");
    assert_eq!(
        fs::read_to_string(&document).unwrap_or_default(),
        "zzz\t2026-09-29\n"
    );
    assert!(
        !snapshot.is_degraded,
        "the row that was just written reads back"
    );
    assert_eq!(
        snapshot.report.loaded,
        built_in_entries(100) + 1,
        "the saved row is one entry more than the built-in table"
    );
    let hit = snapshot.table.longest_match("zzz", 0);
    assert_eq!(
        hit.map(|hit| snapshot.table.text(hit)),
        Some("2026-09-29"),
        "the table the commit produced holds the row"
    );
}

#[test]
fn test_document_sink_creates_the_document_private() {
    // A phrase is the user's own text, so the document is created the way every other file
    // this plugin writes is: 0600, in a directory the layout made 0700.
    let dir = scratch_dir("private");
    let document = dir.join("phrases.tsv");
    let mut sink = DocumentSink::new(Some(document.clone()), 100, true);
    assert!(sink.commit("zzz\t2026-09-29\n").is_ok());

    let mode = fs::metadata(&document).map(|meta| meta.permissions().mode());
    assert_eq!(mode.ok().map(|mode| mode & 0o777), Some(FILE_MODE));
}

#[test]
fn test_document_sink_without_a_document_reports_read_only_mode() {
    // The degradation ASM-15 names: the plugin stops writing and keeps working, so the
    // failure is the read-only code and not a phrase-table failure.
    let mut sink = DocumentSink::new(None, 100, true);
    let refused = sink.commit("zzz\t2026-09-29\n");
    assert!(refused.is_err(), "there is no document to write to");
    if let Err(error) = refused {
        let rendered = error.to_string();
        assert!(rendered.starts_with("data/readonly-mode: "), "{rendered}");
        let reason = rendered
            .strip_prefix("data/readonly-mode: ")
            .expect("the rendering carries the code and then the reason");
        assert!(!reason.contains('/'), "the reason names no path: {reason}");
        assert!(matches!(error, ImeError::DataReadonly { .. }));
    }
}

#[test]
fn test_document_sink_reports_a_write_the_document_refused() {
    let dir = scratch_dir("unwritable");
    let missing = dir.join("absent").join("phrases.tsv");
    let mut sink = DocumentSink::new(Some(missing.clone()), 100, true);

    let refused = sink.commit("zzz\t2026-09-29\n");
    assert!(
        refused.is_err(),
        "a directory that is not there refuses the write"
    );
    if let Err(error) = refused {
        let rendered = error.to_string();
        assert!(rendered.starts_with("data/readonly-mode: "), "{rendered}");
        let reason = rendered
            .strip_prefix("data/readonly-mode: ")
            .expect("the rendering carries the code and then the reason");
        assert!(!reason.contains('/'), "the reason names no path: {reason}");
    }
    assert!(!missing.exists(), "a refused write creates nothing");
}

#[test]
fn test_document_sink_of_a_disabled_table_holds_nothing() {
    // `[phrases] enabled = false` is the user saying no phrase takes part in a decode. The
    // document is still read, so the setting never costs the user an entry they wrote.
    let dir = scratch_dir("disabled");
    let document = dir.join("phrases.tsv");
    fs::write(&document, "zzz\t2026-09-29\n").expect("a document");

    let snapshot = DocumentSink::new(Some(document), 100, false).read();
    assert!(
        snapshot.table.is_empty(),
        "a disabled table matches nothing"
    );
    assert_eq!(snapshot.report.loaded, 0);
    assert!(
        !snapshot.is_degraded,
        "the document was read; the setting is what emptied the table"
    );
}
