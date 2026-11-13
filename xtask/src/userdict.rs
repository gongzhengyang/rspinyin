//! The user-dictionary maintenance CLI (`xtask user-dict`).
//!
//! Responsibility: give the user-frequency store's management surface a documented
//! command-line entry point -- listing, exporting, importing, forgetting, pinning and
//! statistics -- by calling the same library APIs the running plugin uses. No storage
//! logic lives here: the store is opened through `UserDb::open`, which carries the
//! runtime's own migration and read-only discipline, and every mutation goes through
//! the store's public methods.
//!
//! Boundaries: this module never touches the store's tables and never interprets its
//! file. The two read-only views (list, stats) read the TSV interchange document the
//! store itself renders (`UserFreqSource::export_tsv`), so what they show is exactly
//! what an export would carry, pending deltas included.
//!
//! Privacy: `stats` prints counts only -- bucket sizes, totals, file size. The words
//! behind those counts are withheld unless `--full` is given; the same rule the
//! diagnostics layer enforces on logs applies here to the terminal.
//!
//! Output contract: results go to stdout as `key<TAB>value` lines (word lists as TSV
//! rows under a header line), so a script can consume them; notes go to stderr and
//! carry the store's stable diagnostic codes.

use std::fmt::Write as _;
use std::io::{BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use ime_dict::paths::{READONLY_CODE, ensure_dirs};
use ime_dict::user_db::{
    Clock, ForgetOutcome, ImportReport, SCHEMA_MIGRATED_CODE, SystemClock, UserDb,
};
use ime_types::UserFreqSource;

/// Entries `list` shows when `--limit` is not given.
const DEFAULT_LIMIT: usize = 20;
/// File `export` writes when `--out` is not given.
const DEFAULT_EXPORT: &str = "rspinyin-user-dict.tsv";
/// The word an interactive operator must type to confirm a destructive operation.
const CONFIRM_WORD: &str = "yes";
/// The prompt shown before a destructive operation.
const CONFIRM_PROMPT: &str = "this changes the user dictionary; type 'yes' to continue: ";
/// The stable diagnostic code for a word the store never learned.
const USER_WORD_NOT_FOUND_CODE: &str = "dict/user-word-not-found";
/// The prefix of the line that opens the interchange document's coined section.
const COINED_MARKER_PREFIX: &str = "# Coined words:";
/// Milliseconds in a day; the narrowest recency bucket of `stats`.
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// Milliseconds in seven days; the second recency bucket of `stats`.
const WEEK_MS: u64 = 7 * DAY_MS;
/// Milliseconds in thirty days; the last bounded recency bucket of `stats`.
const MONTH_MS: u64 = 30 * DAY_MS;
/// The `stats` bucket names, in [`Recency`] discriminant order.
const RECENCY_NAMES: [&str; 5] = ["last_24h", "last_7d", "last_30d", "older", "never"];

/// Arguments of `xtask user-dict`.
///
/// The two flags are global, so they may be given before or after the subcommand.
#[derive(Debug, clap::Parser)]
pub struct UserDictArgs {
    /// The user database to operate on; the layout's `user.redb` by default.
    ///
    /// An explicit path is opened exactly as the runtime opens its own store: the
    /// schema migration and the read-only discipline are the library's, not this
    /// command's, and a missing file is created the way a first run would.
    #[arg(long, value_name = "PATH", global = true)]
    db: Option<PathBuf>,

    /// Answer the destructive subcommands' confirmation without asking.
    ///
    /// `import` and `forget` rewrite the store; without this flag they ask on a
    /// terminal, and refuse in anything else.
    #[arg(long, global = true)]
    yes: bool,

    /// The operation to run.
    #[command(subcommand)]
    command: UserDictCommand,
}

/// The operations `xtask user-dict` performs.
#[derive(Debug, clap::Subcommand)]
pub enum UserDictCommand {
    /// List learned words by frequency, highest first; coined pairs follow.
    List {
        /// How many entries each section shows.
        #[arg(long, value_name = "N", default_value_t = DEFAULT_LIMIT)]
        limit: usize,
    },
    /// Export the store to a TSV document.
    ///
    /// The same interchange document a backup generation carries: atomic, written
    /// private, and bounded by the store's own 8 MiB ceiling.
    Export {
        /// Where the document is written.
        #[arg(long, value_name = "FILE", default_value = DEFAULT_EXPORT)]
        out: PathBuf,
    },
    /// Merge a TSV document into the store, never lowering what the user earned.
    ///
    /// Idempotent: importing the same document twice leaves the store unchanged.
    /// Destructive enough to ask: a bad document is refused whole, but a good one
    /// writes every row it carries.
    Import {
        /// The document to read.
        file: PathBuf,
    },
    /// Drop one learned word, with its coin rows and its pins.
    ///
    /// Destructive enough to ask: the removal is durable once this returns.
    Forget {
        /// The word to drop, exactly as it was committed.
        word: String,
    },
    /// Toggle the pin of a word under an input key.
    ///
    /// The store pins per input key: pinning a second word under the same key moves
    /// the pin, and pinning the key's own word again releases it.
    Pin {
        /// The word to pin, exactly as it was committed.
        word: String,
        /// The input key the pin belongs to.
        #[arg(long, value_name = "KEY")]
        key: String,
    },
    /// Release the word pinned under an input key.
    ///
    /// The store pins per input key, so the key names what is released; there is no
    /// word-level release, because a word can be pinned under several keys at once.
    Unpin {
        /// The input key whose pin is released.
        key: String,
    },
    /// Print counts: entries, file size, and the recency distribution of commits.
    ///
    /// Counts only, by the same privacy rule the logs obey; `--full` is the explicit
    /// unlock that adds the words behind the counts.
    Stats {
        /// Include the words behind the counts.
        #[arg(long)]
        full: bool,
    },
}

/// Runs one `user-dict` operation.
///
/// The store is opened once, through the runtime's own open path; what that open did
/// (a schema migration, a read-only degradation) is reported on stderr before the
/// operation runs.
///
/// # Errors
/// Returns the open errors, the confirmation refusals, and the chosen operation's
/// errors.
pub fn run(args: UserDictArgs) -> anyhow::Result<()> {
    let path = resolve_db(args.db)?;
    let mut store = UserDb::open(&path)
        .with_context(|| format!("opening the user database at {}", path.display()))?;
    open_notes(&store);
    match args.command {
        UserDictCommand::List { limit } => run_list(&store, limit),
        UserDictCommand::Export { out } => run_export(&store, &out),
        UserDictCommand::Import { file } => {
            ensure_confirmed(args.yes, &format!("import {}", file.display()))?;
            run_import(&store, &file)
        }
        UserDictCommand::Forget { word } => {
            ensure_confirmed(args.yes, &format!("forget {word}"))?;
            run_forget(&mut store, &word)
        }
        UserDictCommand::Pin { word, key } => run_pin(&mut store, &key, &word),
        UserDictCommand::Unpin { key } => run_unpin(&mut store, &key),
        UserDictCommand::Stats { full } => run_stats(&store, &path, full),
    }
}

/// The database this invocation targets: the explicit `--db`, or the layout's own
/// `user.redb`, whose directories are prepared exactly as the runtime prepares them.
///
/// # Errors
/// Returns the layout resolution error when the default path is needed and the
/// environment names no usable base directories.
fn resolve_db(db: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    match db {
        Some(path) => Ok(path),
        None => Ok(ensure_dirs()
            .context("resolving the default user database path")?
            .user_db),
    }
}

/// Reports, on stderr, what the open did: a schema migration, or a read-only
/// degradation, with the store's own stable codes.
fn open_notes(store: &UserDb) {
    if store.schema_migrated() {
        eprintln!(
            "note\t{}\tmigration ran at open; read-only={}",
            SCHEMA_MIGRATED_CODE,
            store.is_readonly()
        );
    } else if store.is_readonly() {
        eprintln!("note\t{READONLY_CODE}\tthe store is read-only; writes are refused");
    }
}

/// Lists the store by frequency, highest first, and prints the listing.
///
/// # Errors
/// As [`read_document`].
fn run_list(store: &UserDb, limit: usize) -> anyhow::Result<()> {
    let document = read_document(store)?;
    print!("{}", list_report(&document, limit));
    Ok(())
}

/// Exports the store to `out` and prints where the document went.
///
/// # Errors
/// As `UserDb::export_to_file`.
fn run_export(store: &UserDb, out: &Path) -> anyhow::Result<()> {
    let rows = store
        .export_to_file(out)
        .with_context(|| format!("writing the export to {}", out.display()))?;
    println!("rows\t{rows}");
    println!("path\t{}", out.display());
    Ok(())
}

/// Imports `file`, prints the store's report, and refuses a refused document.
///
/// A document the store did not recognise, or one carrying an invalid row, wrote
/// nothing; that outcome is an error so a scripted call notices.
///
/// # Errors
/// As [`import_file`], plus the refusal of the document itself.
fn run_import(store: &UserDb, file: &Path) -> anyhow::Result<()> {
    let report = import_file(store, file)?;
    println!("recognised\t{}", report.recognised);
    println!("applied\t{}", report.applied);
    println!("accepted\t{}", report.accepted);
    println!("rejected\t{}", report.rejected);
    if !report.recognised || report.rejected > 0 {
        bail!("the document was refused whole; nothing was applied");
    }
    Ok(())
}

/// Forgets `word` and prints the outcome with its stable code.
///
/// # Errors
/// As [`forget_word`].
fn run_forget(store: &mut UserDb, word: &str) -> anyhow::Result<()> {
    let outcome = forget_word(store, word)?;
    println!("word\t{word}");
    match outcome {
        ForgetOutcome::Removed => println!("outcome\tremoved"),
        ForgetOutcome::NotFound => {
            println!("outcome\tnot-found");
            println!("code\t{USER_WORD_NOT_FOUND_CODE}");
        }
        ForgetOutcome::Readonly => {
            println!("outcome\treadonly");
            println!("code\t{READONLY_CODE}");
        }
    }
    Ok(())
}

/// Applies the pin toggle to `(key, word)` and prints the post-state.
///
/// # Errors
/// As [`pin_word`].
fn run_pin(store: &mut UserDb, key: &str, word: &str) -> anyhow::Result<()> {
    let pinned = pin_word(store, key, word)?;
    println!("key\t{key}");
    println!("word\t{word}");
    println!("pinned\t{pinned}");
    Ok(())
}

/// Releases `key`'s pin and prints what happened.
///
/// # Errors
/// As [`unpin_key`].
fn run_unpin(store: &mut UserDb, key: &str) -> anyhow::Result<()> {
    unpin_key(store, key)?;
    println!("key\t{key}");
    println!("result\tpin-cleared");
    Ok(())
}

/// Prints the statistics view of the store at `db_path`.
///
/// # Errors
/// As [`read_document`], plus the file-size read.
fn run_stats(store: &UserDb, db_path: &Path, full: bool) -> anyhow::Result<()> {
    let document = read_document(store)?;
    let bytes = std::fs::metadata(db_path)
        .with_context(|| format!("measuring {}", db_path.display()))?
        .len();
    print!("{}", stats_report(&document, bytes, now_unix_ms(), full));
    Ok(())
}

/// Reads `file` and merges it into the store, returning what the import did.
///
/// The document is validated whole by the store before a row lands, so a refused
/// document changes nothing; the caller decides what the report means for the exit.
///
/// # Errors
/// Returns the file read errors and the store's import errors.
fn import_file(store: &UserDb, file: &Path) -> anyhow::Result<ImportReport> {
    let opened =
        std::fs::File::open(file).with_context(|| format!("reading {}", file.display()))?;
    let mut reader = BufReader::new(opened);
    store
        .import_tsv(&mut reader)
        .with_context(|| format!("importing {}", file.display()))
}

/// Drops `word` from the store and makes the removal durable.
///
/// The removal is a tombstone until the next flush, and a CLI process earns no
/// further keystrokes to flush for it, so the flush is made here; a failure of it
/// propagates, because the word would come back on the next open otherwise.
///
/// # Errors
/// Returns the flush error when the tombstone cannot be written.
fn forget_word(store: &mut UserDb, word: &str) -> anyhow::Result<ForgetOutcome> {
    let outcome = store.forget_with_outcome(word);
    if outcome == ForgetOutcome::Removed {
        store.final_commit().context("flushing the removal")?;
    }
    Ok(outcome)
}

/// Applies the store's pin toggle to `(key, word)` and makes it durable, answering
/// whether the word is pinned afterwards.
///
/// # Errors
/// Returns the store's pin and flush errors, and the document read behind the answer.
fn pin_word(store: &mut UserDb, key: &str, word: &str) -> anyhow::Result<bool> {
    store
        .set_pinned(key, Some(word))
        .with_context(|| format!("toggling the pin of {word} under {key}"))?;
    store.final_commit().context("flushing the pin")?;
    pinned_now(store, word)
}

/// Releases whatever `key` pins and makes the release durable.
///
/// # Errors
/// Returns the store's pin and flush errors.
fn unpin_key(store: &mut UserDb, key: &str) -> anyhow::Result<()> {
    store
        .set_pinned(key, None)
        .with_context(|| format!("releasing the pin of {key}"))?;
    store.final_commit().context("flushing the pin release")?;
    Ok(())
}

/// Answers whether `word` is pinned, from the document the store renders.
///
/// # Errors
/// As [`read_document`].
fn pinned_now(store: &UserDb, word: &str) -> anyhow::Result<bool> {
    Ok(read_document(store)?
        .entries
        .iter()
        .find(|row| row.word == word)
        .is_some_and(|row| row.pinned))
}

/// Reads the store's interchange document into memory.
///
/// The document is bounded by the store's own export ceiling, so this is a bounded
/// read; the parse below only re-reads what the store itself wrote.
///
/// # Errors
/// Returns the store's rendering errors, and [`parse_document`]'s.
fn read_document(store: &UserDb) -> anyhow::Result<Document> {
    let mut buffer = Vec::new();
    store
        .export_tsv(&mut buffer)
        .context("rendering the user dictionary document")?;
    parse_document(&buffer)
}

/// Parses the frequency and coined rows out of an interchange document.
///
/// Comment lines are skipped; the line that opens the coined section switches the
/// rows that follow to the coined shape. The store writes every row itself, so a row
/// the parser cannot carry is a store this build cannot read, and is reported rather
/// than skipped.
///
/// # Errors
/// Returns an error when the document is not UTF-8, or a row misses a column or a
/// decimal number.
fn parse_document(bytes: &[u8]) -> anyhow::Result<Document> {
    let text = std::str::from_utf8(bytes).context("the document is not UTF-8")?;
    let mut document = Document::default();
    let mut in_coined = false;
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            in_coined = line.starts_with(COINED_MARKER_PREFIX);
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if in_coined {
            if fields.len() < 4 {
                bail!("coined row has {} of 4 columns: {line:?}", fields.len());
            }
            document.coined.push(CoinedRow {
                key: fields[0].to_owned(),
                word: fields[1].to_owned(),
                commits: parse_field(fields[2])?,
            });
        } else {
            if fields.len() < 5 {
                bail!("frequency row has {} of 5 columns: {line:?}", fields.len());
            }
            document.entries.push(EntryRow {
                word: fields[0].to_owned(),
                weight: parse_field(fields[1])?,
                last_used_ms: parse_field(fields[2])?,
                pinned: parse_field::<u8>(fields[4])? != 0,
            });
        }
    }
    Ok(document)
}

/// Reads one decimal field of the interchange document.
///
/// # Errors
/// Returns an error when `text` is not a decimal number of `T`.
fn parse_field<T: std::str::FromStr>(text: &str) -> anyhow::Result<T> {
    text.parse::<T>()
        .map_err(|_| anyhow::anyhow!("document field {text:?} is not a decimal number"))
}

/// One learned entry as the interchange document carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct EntryRow {
    /// The word the user committed; the key every other operation names.
    word: String,
    /// The accumulated commit count.
    weight: u32,
    /// Last use, in unix milliseconds, under the column name the document uses.
    last_used_ms: u64,
    /// Whether any input key pins the word.
    pinned: bool,
}

/// One coined pair as the interchange document carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CoinedRow {
    /// The pinyin the pair was spelled with.
    key: String,
    /// The word itself.
    word: String,
    /// How many commits confirmed the pair.
    commits: u32,
}

/// The store's interchange document, parsed back for the listing and statistics
/// views.
#[derive(Debug, Default, PartialEq, Eq)]
struct Document {
    /// The frequency rows, in the document's key order.
    entries: Vec<EntryRow>,
    /// The coined rows, in the document's key order.
    coined: Vec<CoinedRow>,
}

/// Renders the listing view: entries by weight, highest first, then coined pairs.
///
/// Both sections are cut to `limit`; a limit of zero leaves the headers alone. Ties
/// break on the word, so two listings of an unchanged store agree.
fn list_report(document: &Document, limit: usize) -> String {
    let mut out = String::new();
    out.push_str("weight\tlast_used_unix\tpinned\tword\n");
    let mut entries = document.entries.clone();
    entries.sort_by(|left, right| {
        right
            .weight
            .cmp(&left.weight)
            .then_with(|| left.word.cmp(&right.word))
    });
    // Writing into a `String` cannot fail, so the `fmt::Result` is discarded; the
    // same shape the store's own document renderer uses.
    for row in entries.iter().take(limit) {
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}",
            row.weight,
            row.last_used_ms,
            u8::from(row.pinned),
            row.word
        );
    }
    if document.coined.is_empty() {
        return out;
    }
    out.push_str("coined_key\tcommits\tword\n");
    let mut coined = document.coined.clone();
    coined.sort_by(|left, right| {
        right
            .commits
            .cmp(&left.commits)
            .then_with(|| left.word.cmp(&right.word))
    });
    for row in coined.iter().take(limit) {
        let _ = writeln!(out, "{}\t{}\t{}", row.key, row.commits, row.word);
    }
    out
}

/// Renders the statistics view: totals, file size, and the recency distribution.
///
/// Counts only, whatever `full` is; `full` additionally lists, under one header per
/// non-empty bucket, the weights and words behind the counts -- the explicit unlock
/// the privacy rule demands. The coined pairs are counted but never listed, under
/// either setting.
fn stats_report(document: &Document, bytes: u64, now_ms: u64, full: bool) -> String {
    let mut out = String::new();
    put_field(&mut out, "entries", document.entries.len());
    put_field(&mut out, "coined_entries", document.coined.len());
    put_field(&mut out, "bytes", bytes);
    let mut buckets: [Vec<&EntryRow>; 5] = Default::default();
    for row in &document.entries {
        buckets[bucket_of(row.last_used_ms, now_ms) as usize].push(row);
    }
    for (name, bucket) in RECENCY_NAMES.iter().zip(&buckets) {
        put_field(&mut out, name, bucket.len());
    }
    if !full {
        return out;
    }
    for (name, bucket) in RECENCY_NAMES.iter().zip(&buckets) {
        if bucket.is_empty() {
            continue;
        }
        let _ = writeln!(out, "words_{name}\nweight\tword");
        let mut rows: Vec<&EntryRow> = bucket.clone();
        rows.sort_by(|left, right| {
            right
                .weight
                .cmp(&left.weight)
                .then_with(|| left.word.cmp(&right.word))
        });
        for row in rows {
            let _ = writeln!(out, "{}\t{}", row.weight, row.word);
        }
    }
    out
}

/// Appends one `key<TAB>value` line to `out`.
///
/// Writing into a `String` cannot fail, so the `fmt::Result` is discarded.
fn put_field<T: std::fmt::Display>(out: &mut String, key: &str, value: T) {
    let _ = writeln!(out, "{key}\t{value}");
}

/// Which recency bucket a last-used stamp belongs to, counted from `now_ms`.
///
/// A stamp of zero is a record no commit has stamped -- the "never" bucket -- rather
/// than a record from 1970.
fn bucket_of(last_used_ms: u64, now_ms: u64) -> Recency {
    if last_used_ms == 0 {
        return Recency::Never;
    }
    let age = now_ms.saturating_sub(last_used_ms);
    if age < DAY_MS {
        Recency::Day
    } else if age < WEEK_MS {
        Recency::Week
    } else if age < MONTH_MS {
        Recency::Month
    } else {
        Recency::Older
    }
}

/// The recency buckets of `stats`, narrowest first; the discriminant is the index
/// into [`RECENCY_NAMES`] and the bucket arrays.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Recency {
    /// Used within the last day.
    Day,
    /// Used within the last week.
    Week,
    /// Used within the last thirty days.
    Month,
    /// Used longer ago than that.
    Older,
    /// Never stamped: a record with no commit of its own.
    Never,
}

/// Refuses a destructive operation unless the operator confirmed it.
///
/// # Errors
/// Returns an error naming the refused action when the confirmation is missing.
fn ensure_confirmed(yes: bool, action: &str) -> anyhow::Result<()> {
    if confirmed(yes, std::io::stdin().is_terminal(), read_stdin_line) {
        return Ok(());
    }
    bail!("refusing to {action}: pass --yes (or answer 'yes' at the prompt) to confirm")
}

/// The confirmation gate behind [`ensure_confirmed`], with its inputs injected so a
/// test can drive every branch without a terminal.
///
/// `--yes` confirms outright. Without it, an interactive terminal is asked and must
/// answer [`CONFIRM_WORD`]; anything else -- a decline, a pipe, an absent terminal --
/// refuses, because silence must never be read as consent.
fn confirmed(yes: bool, interactive: bool, mut answer: impl FnMut() -> Option<String>) -> bool {
    if yes {
        return true;
    }
    if !interactive {
        return false;
    }
    print!("{CONFIRM_PROMPT}");
    let _ = std::io::stdout().flush();
    answer().is_some_and(|line| line.trim() == CONFIRM_WORD)
}

/// Reads one line of stdin, or nothing when stdin cannot be read.
fn read_stdin_line() -> Option<String> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok().map(|_| line)
}

/// Wall-clock milliseconds, read through the store's own time source.
fn now_unix_ms() -> u64 {
    SystemClock::new().now_ms()
}

#[cfg(test)]
mod tests;
