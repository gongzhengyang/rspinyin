//! The crash record: what is written, and what may never be.
//!
//! Responsibility: define the shape of a crash record, enforce what may go into one,
//! and write it into the crash directory with the modes the privacy baseline fixes.
//!
//! Boundaries: this module reads no clock, no environment and no configuration, and it
//! installs nothing. The timestamp, the thread identity and the panic text are handed
//! to it, which is what lets every rule below be exercised by a test.
//!
//! # What a record may contain
//!
//! The structural facts a record carries are a closed set of checked keys; see
//! [`crate::crash::context`] for what may go into them and why. There is no key for the
//! input buffer, the preedit, a candidate or a commit string, so no code path can
//! attach one.
//!
//! The two free-text fields are the panic message and the backtrace. The message is a
//! static template by the same rule that governs log messages -- never a value -- and
//! the backtrace holds symbol names and addresses. Both are bounded and stripped of
//! control characters by [`CrashRecord::render`], so nothing in them can forge a line
//! of the record or push it past its size cap.
//!
//! # Modes
//!
//! The directory is `0700` and every record is `0600`, the same baseline as the user
//! database and the logs. The file's mode is passed to the `open` call itself, so a
//! record never exists -- not even for one syscall -- where another account could read
//! it.

use std::fmt::Write as _;
use std::fs::{DirBuilder, File, Metadata, OpenOptions};
use std::hash::{Hash as _, Hasher as _};
use std::io::{Error, ErrorKind, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::thread::ThreadId;

use super::context::{CrashContext, MAX_CONTEXT_VALUE_CHARS};

/// Mode of the directory records are written to.
pub const CRASH_DIR_MODE: u32 = 0o700;
/// Mode of a crash record file.
pub const CRASH_FILE_MODE: u32 = 0o600;
/// Largest record written, in bytes.
pub const MAX_RECORD_BYTES: usize = 64 * 1024;
/// Deepest backtrace kept, in frames.
pub const MAX_BACKTRACE_FRAMES: usize = 64;
/// Longest panic message kept, in characters.
pub const MAX_PAYLOAD_CHARS: usize = 1024;
/// Longest thread name kept, in characters.
pub const MAX_NAME_CHARS: usize = 64;
/// Extension every record file carries.
pub const FILE_SUFFIX: &str = ".txt";

/// The first line of every record, shared with the signal path's own record.
pub(crate) const RECORD_HEADER: &str = "rspinyin crash record\n";

/// How many names are tried before a record is given up on.
///
/// Two threads can crash in the same millisecond, and a panic hook can fire more than
/// once in one; a handful of attempts covers both without an unbounded loop.
const MAX_NAME_ATTEMPTS: usize = 8;

/// One crash, as it is written into the crash directory.
///
/// The fields are public so that a capture site can fill them in directly. The rules
/// that keep user content out are enforced by [`CrashContext`] -- a closed set of
/// checked keys -- and by [`CrashRecord::render`], which bounds and scrubs the two
/// free-text fields before they can reach a file.
#[derive(Clone, Debug, Default)]
pub struct CrashRecord {
    /// Milliseconds since the Unix epoch, read once at the capture site.
    pub timestamp_unix_ms: u64,
    /// The name of the thread that crashed.
    pub thread_name: String,
    /// A stable identifier of that thread, unique within this process.
    pub thread_id: u64,
    /// `file:line:column` of the panic, when the panic carried a location.
    pub location: Option<String>,
    /// The panic message.
    pub payload: String,
    /// The symbolized backtrace.
    pub backtrace: String,
    /// The structural facts the capture site attached.
    pub context: CrashContext,
}

impl CrashRecord {
    /// Renders the record as the text written into the crash directory.
    ///
    /// Every value is written on one line with its newlines escaped, so a value can
    /// never forge a second line of the record. The result is at most
    /// [`MAX_RECORD_BYTES`] bytes and the backtrace at most [`MAX_BACKTRACE_FRAMES`]
    /// frames; a record that would be longer is cut at a character boundary.
    ///
    /// # Returns
    ///
    /// The record as text, ending in a newline.
    ///
    /// # Panics
    ///
    /// Never: the formatting target is a `String`, which cannot fail.
    pub fn render(&self) -> String {
        let mut text = String::with_capacity(1024);
        text.push_str(RECORD_HEADER);
        let _ = writeln!(text, "timestamp_unix_ms={}", self.timestamp_unix_ms);
        let _ = writeln!(text, "thread_name={}", escape_line(&self.thread_name, MAX_NAME_CHARS));
        let _ = writeln!(text, "thread_id={:016x}", self.thread_id);
        match &self.location {
            Some(location) => {
                let _ = writeln!(text, "location={}", escape_line(location, MAX_NAME_CHARS));
            }
            None => text.push_str("location=<none>\n"),
        }
        let _ = writeln!(text, "payload={}", escape_line(&self.payload, MAX_PAYLOAD_CHARS));
        for (key, value) in self.context.iter() {
            let _ = writeln!(text, "{key}={}", escape_line(value, MAX_CONTEXT_VALUE_CHARS));
        }
        text.push_str("backtrace:\n");
        for frame in self.backtrace.lines().take(MAX_BACKTRACE_FRAMES) {
            text.push_str(frame);
            text.push('\n');
        }
        let end = floor_char_boundary(&text, MAX_RECORD_BYTES);
        text.truncate(end);
        text
    }
}

/// Renders `text` on one line, bounded to `max_chars` characters.
///
/// Newlines, carriage returns and tabs are escaped, so a value cannot forge a second
/// line of a record; every other control character is dropped, so a record is always
/// plain text that a terminal can print without being driven by it.
pub(crate) fn escape_line(text: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max_chars));
    for ch in text.chars().take(max_chars) {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if other.is_control() => {}
            other => out.push(other),
        }
    }
    out
}

/// The largest index into `text` that is at most `max_bytes` and on a character
/// boundary.
fn floor_char_boundary(text: &str, max_bytes: usize) -> usize {
    if text.len() <= max_bytes {
        return text.len();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// The file name a crash record is written under: `<timestamp>-<thread_id>.txt`.
///
/// The timestamp is in milliseconds since the Unix epoch and the thread identifier in
/// fixed-width lowercase hexadecimal, so a directory listing sorts by time and two
/// records of the same millisecond stay distinguishable.
///
/// # Parameters
///
/// - `timestamp_unix_ms`: the record's timestamp.
/// - `thread_id`: the crashing thread's identifier.
///
/// # Returns
///
/// The file name, without any directory part.
///
/// # Panics
///
/// Never.
pub fn crash_file_name(timestamp_unix_ms: u64, thread_id: u64) -> String {
    format!("{timestamp_unix_ms}-{thread_id:016x}{FILE_SUFFIX}")
}

/// Whether `name` has the shape [`crash_file_name`] produces.
fn is_record_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(FILE_SUFFIX) else {
        return false;
    };
    let Some((stamp, rest)) = stem.split_once('-') else {
        return false;
    };
    !stamp.is_empty()
        && !rest.is_empty()
        && stamp.bytes().all(|byte| byte.is_ascii_digit())
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// The name to try on the `attempt`-th write of one record.
fn attempt_name(base: &str, attempt: usize) -> String {
    if attempt == 0 {
        return base.to_owned();
    }
    match base.strip_suffix(FILE_SUFFIX) {
        Some(stem) => format!("{stem}-{attempt}{FILE_SUFFIX}"),
        None => format!("{base}-{attempt}"),
    }
}

/// Writes `record` into `dir`, creating the directory private when it is missing.
///
/// A name that is already taken is retried with a numeric suffix, because two threads
/// can crash within the same millisecond and a panic hook can fire more than once.
///
/// # Parameters
///
/// - `dir`: the crash directory. It is created with mode [`CRASH_DIR_MODE`] when it is
///   missing and narrowed to it when it is wider.
/// - `record`: the record to write.
///
/// # Returns
///
/// The path the record was written to.
///
/// # Errors
///
/// Returns the underlying `std::io::Error` when the directory cannot be prepared, when
/// it is a symbolic link or not a directory at all, or when the record cannot be
/// written. The caller reports it on the crash channel: a crash that cannot be recorded
/// must never become a second crash.
pub fn write_record(dir: &Path, record: &CrashRecord) -> std::io::Result<PathBuf> {
    ensure_private_dir(dir)?;
    let text = record.render();
    let base = crash_file_name(record.timestamp_unix_ms, record.thread_id);
    for attempt in 0..MAX_NAME_ATTEMPTS {
        let path = dir.join(attempt_name(&base, attempt));
        match create_private_file(&path) {
            Ok(mut file) => {
                file.write_all(text.as_bytes())?;
                return Ok(path);
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(Error::new(ErrorKind::AlreadyExists, "every crash record name was taken"))
}

/// Creates and narrows the record file `name` inside `dir`.
///
/// Used by the signal path, which has to own its file descriptor before the fault it
/// records can happen.
///
/// # Errors
///
/// As [`write_record`], without the retry: the caller has already chosen the name.
pub fn create_record_file(dir: &Path, name: &str) -> std::io::Result<File> {
    ensure_private_dir(dir)?;
    create_private_file(&dir.join(name))
}

/// Removes the record files of previous runs that were armed but never written.
///
/// The signal channel opens its record file when it is armed, because a signal handler
/// cannot build a path; a run that ends without a fault therefore leaves an empty file
/// behind. This removes exactly those -- empty files whose name has the record shape --
/// and nothing else. A failure is ignored: housekeeping is never worth failing a start
/// over.
pub fn prune_empty_records(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_record_name(name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_file() && meta.len() == 0 {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Creates the record file with its final mode in the `open` call itself.
fn create_private_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(CRASH_FILE_MODE)
        .open(path)
}

/// Prepares `dir` so that only its owner can reach what is written into it.
fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() => Err(refused("a symbolic link")),
        Ok(meta) if meta.is_dir() => narrow_dir(dir, &meta),
        Ok(_) => Err(refused("not a directory")),
        Err(error) if error.kind() == ErrorKind::NotFound => create_dir(dir),
        Err(error) => Err(error),
    }
}

/// Builds the error a refused crash directory is reported with.
fn refused(what: &str) -> Error {
    Error::other(format!("the crash directory is {what}"))
}

/// Brings the directory down to [`CRASH_DIR_MODE`] when another account can reach it.
///
/// A mode the user narrowed further -- a directory without the write bit, say -- is
/// left alone; what this promises is that nothing this layer writes is reachable by a
/// second account.
fn narrow_dir(dir: &Path, meta: &Metadata) -> std::io::Result<()> {
    if meta.permissions().mode() & 0o077 == 0 {
        return Ok(());
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(CRASH_DIR_MODE))
}

/// Creates the directory tree with [`CRASH_DIR_MODE`] in the `mkdir` call itself.
fn create_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true).mode(CRASH_DIR_MODE);
    builder.create(dir)
}

/// A stable identifier for a thread, unique within this process.
///
/// `ThreadId` exposes no numeric value on stable Rust, and the kernel's tid would need
/// `libc::gettid`, which this layer may not call. The identity is therefore hashed into
/// 64 bits: stable for the lifetime of the process, which is what telling two crashing
/// threads apart needs, and carrying nothing about the thread beyond that identity.
///
/// # Parameters
///
/// - `id`: the thread's identifier.
///
/// # Returns
///
/// The identifier the records and file names are written under.
///
/// # Panics
///
/// Never.
pub fn thread_identifier(id: ThreadId) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    use crate::crash::context::CrashContextKey;

    use super::*;

    /// A scratch root this test owns.
    ///
    /// `/tmp` rather than `std::env::temp_dir()` on purpose: a test's behaviour must not
    /// depend on the environment, and `temp_dir()` reads `$TMPDIR`.
    fn scratch_root(label: &str) -> PathBuf {
        let name = format!("rspinyin-crash-record-{}-{label}", std::process::id());
        let root = PathBuf::from("/tmp").join(name);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("creating the scratch root");
        root
    }

    /// The permission bits of `path`.
    fn mode_of(path: &Path) -> u32 {
        match std::fs::metadata(path) {
            Ok(meta) => meta.permissions().mode() & 0o777,
            Err(_) => 0,
        }
    }

    /// A record with a fixed timestamp, so nothing here reads a clock.
    fn record() -> CrashRecord {
        CrashRecord {
            timestamp_unix_ms: 1_759_142_112_345,
            thread_name: String::from("ui"),
            thread_id: 0x1f3a_2c1d,
            location: Some(String::from("ime-ui/src/ui_thread.rs:214:9")),
            payload: String::from("a decode invariant failed"),
            backtrace: String::from("   0: frame\n   1: frame\n"),
            context: CrashContext::new(),
        }
    }

    #[test]
    fn test_crash_record_render_writes_the_documented_lines() {
        let mut context = CrashContext::new();
        context.insert_identifier(CrashContextKey::SessionState, "composing");
        context.insert_count(CrashContextKey::Revision, 12);
        let mut record = record();
        record.context = context;

        let text = record.render();

        assert!(text.starts_with(RECORD_HEADER), "{text}");
        assert!(text.contains("timestamp_unix_ms=1759142112345"), "{text}");
        assert!(text.contains("thread_name=ui"), "{text}");
        assert!(text.contains("thread_id=000000001f3a2c1d"), "{text}");
        assert!(text.contains("location=ime-ui/src/ui_thread.rs:214:9"), "{text}");
        assert!(text.contains("payload=a decode invariant failed"), "{text}");
        assert!(text.contains("session_state=composing"), "{text}");
        assert!(text.contains("revision=12"), "{text}");
        assert!(text.contains("backtrace:\n   0: frame\n"), "{text}");
    }

    #[test]
    fn test_crash_record_render_escapes_newlines_in_a_value() {
        let mut record = record();
        record.payload = String::from("first\nsession_state=forged");
        record.thread_name = String::from("ui\nINFO forged");

        let text = record.render();

        assert!(text.contains("payload=first\\nsession_state=forged"), "{text}");
        assert!(text.contains("thread_name=ui\\nINFO forged"), "{text}");
        assert!(
            !text.lines().any(|line| line == "session_state=forged"),
            "a value must not be able to forge a record line:\n{text}"
        );
    }

    #[test]
    fn test_crash_record_render_drops_control_characters() {
        let mut record = record();
        record.payload = String::from("bell\u{7}and\u{1b}[31mred");
        let text = record.render();
        assert!(text.contains("payload=belland[31mred"), "{text}");
    }

    #[test]
    fn test_crash_record_render_bounds_the_backtrace_frames() {
        let mut record = record();
        record.backtrace = (0..200).map(|index| format!("   {index}: frame\n")).collect();

        let text = record.render();
        let frames = text
            .lines()
            .skip_while(|line| *line != "backtrace:")
            .skip(1)
            .count();

        assert_eq!(frames, MAX_BACKTRACE_FRAMES);
    }

    #[test]
    fn test_crash_record_render_stays_within_the_size_cap() {
        let mut record = record();
        record.payload = "p".repeat(MAX_RECORD_BYTES);
        record.backtrace = "   frame with a fairly long symbol name\n".repeat(4096);

        let text = record.render();

        assert!(text.len() <= MAX_RECORD_BYTES, "{} bytes", text.len());
        assert!(text.starts_with(RECORD_HEADER), "the head is never cut");
        assert!(
            text.ends_with('\n') || text.len() == MAX_RECORD_BYTES,
            "a cut record ends where the cap is"
        );
    }

    #[test]
    fn test_escape_line_bounds_the_value_and_escapes_the_separators() {
        assert_eq!(escape_line("a\nb", 8), "a\\nb");
        assert_eq!(escape_line("a\rb\tc", 8), "a\\rb\\tc");
        assert_eq!(escape_line("abcdef", 3), "abc");
        assert_eq!(escape_line("", 3), "");
    }

    #[test]
    fn test_floor_char_boundary_stops_before_a_split_character() {
        let text = "aé";
        assert_eq!(floor_char_boundary(text, 3), 3);
        // Byte 2 is inside the two-byte `é`, so the cut moves back to byte 1.
        assert_eq!(floor_char_boundary(text, 2), 1);
        assert_eq!(floor_char_boundary(text, 0), 0);
        assert_eq!(floor_char_boundary(text, 99), text.len());
    }

    #[test]
    fn test_crash_file_name_is_stable_and_distinguishes_threads() {
        assert_eq!(
            crash_file_name(1_759_142_112_345, 0x1f3a_2c1d),
            "1759142112345-000000001f3a2c1d.txt"
        );
        assert_ne!(
            crash_file_name(1, 2),
            crash_file_name(1, 3),
            "two threads crashing in one millisecond must not share a name"
        );
    }

    #[test]
    fn test_is_record_name_accepts_a_record_and_refuses_a_foreign_file() {
        assert!(is_record_name(&crash_file_name(17, 3)));
        assert!(is_record_name("1759142112345-000000001f3a2c1d-1.txt"));
        assert!(!is_record_name("notes.txt"));
        assert!(!is_record_name("1759142112345-000000001f3a2c1d"));
        assert!(!is_record_name("1759142112345.txt"));
        assert!(!is_record_name("-000000001f3a2c1d.txt"));
    }

    #[test]
    fn test_thread_identifier_is_stable_and_differs_between_threads() {
        let first = thread_identifier(std::thread::current().id());
        let again = thread_identifier(std::thread::current().id());
        assert_eq!(first, again, "the same thread keeps its identifier");

        let spawned = std::thread::spawn(|| thread_identifier(std::thread::current().id()));
        let other = spawned.join().unwrap_or(first);
        assert_ne!(first, other, "two threads must be told apart");
    }

    #[test]
    fn test_write_record_creates_a_private_file_in_a_private_directory() {
        let root = scratch_root("modes");
        let dir = root.join("crash");
        std::fs::create_dir_all(&dir).expect("creating the scratch directory");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .expect("loosening the fixture directory");

        let path = write_record(&dir, &record()).expect("writing the record");

        assert!(path.is_file(), "the record was written");
        assert_eq!(mode_of(&path), CRASH_FILE_MODE, "the record is owner-only");
        assert_eq!(mode_of(&dir), CRASH_DIR_MODE, "the directory is narrowed");
    }

    #[test]
    fn test_write_record_creates_a_missing_directory_tree() {
        let root = scratch_root("missing-dir");
        let dir = root.join("data").join("rspinyin").join("crash");

        let path = write_record(&dir, &record()).expect("writing the record");

        assert!(path.is_file());
        assert_eq!(mode_of(&dir), CRASH_DIR_MODE);
    }

    #[test]
    fn test_write_record_keeps_two_records_of_the_same_millisecond() {
        let root = scratch_root("collision");
        let dir = root.join("crash");

        let first = write_record(&dir, &record()).expect("writing the first record");
        let second = write_record(&dir, &record()).expect("writing the second record");

        assert_ne!(first, second, "the second write must not overwrite the first");
        assert!(first.is_file() && second.is_file());
        let name = second.file_name().unwrap_or_default().to_string_lossy();
        assert!(is_record_name(&name), "{name} is still a record name");
        assert!(!std::fs::read(&second).unwrap_or_default().is_empty());
    }

    #[test]
    fn test_write_record_refuses_a_symlinked_crash_directory() {
        let root = scratch_root("symlink");
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("creating the link target");
        let dir = root.join("crash");
        symlink(&elsewhere, &dir);

        let outcome = write_record(&dir, &record());

        assert!(outcome.is_err(), "a linked crash directory is refused");
        // `Result<bool, io::Error>` has no `PartialEq` (`io::Error` does not implement it),
        // so the read is collapsed to the fact being asserted rather than compared whole.
        let nothing_written = std::fs::read_dir(&elsewhere)
            .map(|mut it| it.next().is_none())
            .unwrap_or(false);
        assert!(nothing_written, "nothing is written through the link");
    }

    #[test]
    fn test_write_record_refuses_a_path_that_is_not_a_directory() {
        let root = scratch_root("not-a-dir");
        let dir = root.join("crash");
        std::fs::write(&dir, b"not a directory");

        assert!(write_record(&dir, &record()).is_err());
    }

    #[test]
    fn test_create_record_file_opens_an_existing_name_without_truncating_it() {
        let root = scratch_root("create-record");
        let dir = root.join("crash");
        let name = crash_file_name(1, 2);

        let first = create_record_file(&dir, &name);
        assert!(first.is_ok(), "the file is created");
        assert_eq!(mode_of(&dir.join(&name)), CRASH_FILE_MODE);
        let again = create_record_file(&dir, &name);
        assert!(
            again.is_err(),
            "an armed channel owns its name; a second arming must not take it"
        );
    }

    #[test]
    fn test_prune_empty_records_removes_only_empty_record_files() {
        let root = scratch_root("prune");
        let dir = root.join("crash");
        std::fs::create_dir_all(&dir).expect("creating the scratch directory");
        let armed = dir.join(crash_file_name(1, 2));
        std::fs::write(&armed, b"").expect("placing an armed record");
        let written = dir.join(crash_file_name(3, 4));
        std::fs::write(&written, b"rspinyin crash record\n").expect("placing a record");
        let foreign = dir.join("notes.txt");
        std::fs::write(&foreign, b"").expect("placing a foreign file");

        prune_empty_records(&dir);

        assert!(!armed.exists(), "an armed but unused record is removed");
        assert!(written.exists(), "a written record is kept");
        assert!(foreign.exists(), "a file that is not a record is left alone");
    }
}
