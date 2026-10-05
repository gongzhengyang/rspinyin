//! Structured logging: the subscriber, the rolling file sink, and the level policy.
//!
//! Responsibility: install the one `tracing` subscriber this project has, decide
//! where its output goes, and decide how much of it is kept. The subscriber is a
//! `Registry` with two layers -- [`RedactLayer`], which filters by level and marks
//! sensitive sessions, and a `fmt` layer that renders each event with
//! [`RedactFormat`] into the sink.
//!
//! Boundaries: this module owns the file, its rotation and its permissions, and it
//! is the only place in the workspace that installs a subscriber -- business crates
//! emit through the `tracing` macros and never install one. What may be *written* is
//! [`crate::redact`]'s decision, not this module's.
//!
//! # Where the log goes
//!
//! `log_dir/rspinyin.log`, created `0600` inside a directory created `0700`, next to its
//! rolled siblings `rspinyin.log.1` upwards. When the directory cannot be created or
//! written, the sink degrades to `stderr` -- which the host writes into its own log --
//! and the level is raised to `warn`, so a log shared with every other addon is not
//! flooded with our per-keystroke detail. Diagnostics never fail startup.
//!
//! # Rolling
//!
//! The active file rolls over as soon as a write would push it past `rotation_mb`, and at
//! most `keep_files` rolled files are kept. `tracing-appender` is the ecosystem crate for
//! log files, but its `Rotation` is a *time* schedule (`MINUTELY` .. `NEVER`) with no size
//! policy, and the design asks for a size threshold, so the file handling here is our own;
//! the crate stays a dependency for the non-blocking writer the design reserves for the
//! case where a measured P99 write exceeds 20 microseconds.

use std::cmp;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use ime_types::ImeError;
use tracing::Subscriber;
// Re-exported so a host crate can spell the level vocabulary this module's API speaks
// (`DiagConfig::level`, `DiagHandle::reconfigure`) without taking a `tracing`
// dependency of its own: the host layer maps the configuration's `diagnostics.level`
// onto these filters when it re-aims the policy.
pub use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::registry;

use crate::redact::{RedactFormat, RedactLayer, RedactState};

/// The name of the active log file inside `DiagConfig::log_dir`.
pub const LOG_FILE_NAME: &str = "rspinyin.log";

/// The number of bytes one mebibyte of `DiagConfig::rotation_mb` counts.
const MEBIBYTE: u64 = 1024 * 1024;

/// Whether a subscriber has already been installed in this process.
static INITIALISED: AtomicBool = AtomicBool::new(false);

/// Configuration of the diagnostics layer.
///
/// The defaults are the ones the design fixes. The log directory itself is not
/// defaulted: it comes from the XDG layout the paths module owns, because this crate
/// must not guess where user data lives.
#[derive(Clone, Debug)]
pub struct DiagConfig {
    /// The lowest level that reaches a sink; default [`LevelFilter::INFO`].
    pub level: LevelFilter,
    /// The directory that holds the log file and its rolled siblings; created with
    /// mode `0700` when missing.
    pub log_dir: PathBuf,
    /// Roll the active file over once it exceeds this many mebibytes (2^20 bytes);
    /// default 8. Zero disables rolling, leaving one file that grows without bound.
    pub rotation_mb: u64,
    /// How many rolled files to keep behind the active one; default 3, so the
    /// directory holds at most `keep_files + 1` files.
    pub keep_files: usize,
    /// Whether the user asked for more diagnostics; default `false`.
    ///
    /// This switch cannot turn the recording of characters on -- nothing can. It raises
    /// the effective level to `debug`, which adds *structural* detail (DAG sizes,
    /// candidate source distributions) to the log, and makes [`init_logging`] print a
    /// notice saying so. An explicit [`LevelFilter::OFF`] is left alone: it means "no
    /// diagnostics at all".
    pub log_input_content: bool,
}

impl DiagConfig {
    /// Creates a configuration for `log_dir` with the documented defaults: `info`,
    /// 8 mebibytes, three kept files, and no input-content logging.
    ///
    /// # Panics
    ///
    /// Never.
    ///
    /// # Examples
    ///
    /// ```
    /// use ime_diag::log::DiagConfig;
    /// use tracing::level_filters::LevelFilter;
    ///
    /// let cfg = DiagConfig::new("/home/ann/.local/share/rspinyin/logs");
    /// assert_eq!(cfg.level, LevelFilter::INFO);
    /// assert_eq!((cfg.rotation_mb, cfg.keep_files), (8, 3));
    /// assert!(!cfg.log_input_content);
    /// ```
    pub fn new(log_dir: impl Into<PathBuf>) -> Self {
        Self {
            level: LevelFilter::INFO,
            log_dir: log_dir.into(),
            rotation_mb: 8,
            keep_files: 3,
            log_input_content: false,
        }
    }
}

/// Handle to the installed diagnostics subscriber.
///
/// It keeps the redaction state reachable, which is how a caller that has just seen a
/// password context marks a session without waiting for an event to carry the flag,
/// and it reports where the log is going. Dropping it uninstalls nothing: `tracing`
/// has no way to remove a global subscriber, and diagnostics must outlive the caller
/// that started them.
#[derive(Clone, Debug)]
pub struct DiagHandle {
    state: Arc<RedactState>,
    log_path: Option<PathBuf>,
    /// A clone of the sink the subscriber writes through, so
    /// [`DiagHandle::reconfigure`] can reach the rolling policy without touching the
    /// subscriber. `None` when the sink degraded to stderr, where a rolling policy has
    /// nothing to apply to.
    sink: Option<Sink>,
}

impl DiagHandle {
    /// Marks the session named by `session` as a sensitive context.
    ///
    /// Everything that session logs from now on is downgraded to `session=redacted`
    /// plus the application hash. This is the call to make when the host reports
    /// `CapabilityFlag::Password` at input-context activation: unlike a flag carried by
    /// an event, it cannot be lost to a level change.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn mark_sensitive_session(&self, session: u64) {
        self.state.mark_sensitive_session(session);
    }

    /// Whether `session` has been marked sensitive, through
    /// [`Self::mark_sensitive_session`] or by an event that carried the flag.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_session_sensitive(&self, session: u64) -> bool {
        self.state.is_session_sensitive(session)
    }

    /// The active log file, or `None` when the sink degraded to `stderr`.
    pub fn log_path(&self) -> Option<&Path> {
        self.log_path.as_deref()
    }

    /// The level filter in force: the configured level after the degradations of this
    /// module, raised to `debug` by the input-content switch and to `warn` by the
    /// stderr fallback.
    pub fn level(&self) -> LevelFilter {
        self.state.level()
    }

    /// Re-aims the policy the diagnostics layer applies, without reinstalling the
    /// subscriber.
    ///
    /// `tracing` installs its global subscriber exactly once per process, but the
    /// values the policy reads -- the level and the rolling limits -- need not be
    /// frozen with it. This call swaps them in place: the level rank is one atomic
    /// store the redaction layer reads with one lock-free load per event, and the
    /// rolling limits are atomics the write path reads where it already holds the
    /// file lock. No lock is taken to write a parameter, so a reconfiguration that
    /// races a log call cannot wedge either side.
    ///
    /// The degradation floors survive a reconfiguration: a log that went to stderr
    /// stays capped at `warn` and the input-content switch keeps the process at
    /// `debug`, whatever this call asks for.
    ///
    /// # Arguments
    ///
    /// * `level` -- the lowest level that reaches a sink from now on. `OFF` means no
    ///   diagnostics at all.
    /// * `rotation_mb` -- the size at which the active file rolls over, in mebibytes;
    ///   zero disables rolling.
    /// * `keep_files` -- how many rolled files stay behind the active one.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn reconfigure(&self, level: LevelFilter, rotation_mb: u64, keep_files: usize) {
        self.state.set_configured_level(level);
        if let Some(Sink::File(writer)) = &self.sink {
            writer.set_policy(rotation_mb.saturating_mul(MEBIBYTE), keep_files);
        }
    }
}

/// Installs the process-wide `tracing` subscriber.
///
/// The subscriber is installed exactly once per process; a second call reports the
/// misuse instead of replacing the subscriber that is already serving the plugin,
/// and never panics. Failing to open the log file is not a failure of this function:
/// the sink degrades to `stderr` and the plugin keeps running.
///
/// # Parameters
///
/// - `cfg`: the level, the log directory, the rolling policy and the input-content
///   switch. The home directory whose prefix the log shortens is taken from `$HOME`;
///   a process without one logs paths as they are.
///
/// # Returns
///
/// The handle that marks sensitive sessions and reports the log path.
///
/// # Errors
///
/// [`ImeError::ConfigInvalid`] with the key `diag.log` when a subscriber was already
/// installed -- by an earlier call to this function or by another library in the
/// process. A caller that wants to carry on regardless can ignore the error and keep
/// logging: the events go to whichever subscriber won.
///
/// # Panics
///
/// Never: the once-per-process guard is a compare-and-swap on an atomic, and every
/// failure path returns an error.
pub fn init_logging(cfg: &DiagConfig) -> Result<DiagHandle, ImeError> {
    let installed = INITIALISED.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire);
    if installed.is_err() {
        return Err(ImeError::ConfigInvalid {
            key: String::from("diag.log"),
            reason: String::from("init_logging was already called in this process"),
        });
    }

    let prepared = Prepared::open(cfg);
    let state = Arc::new(RedactState::new(
        prepared.level,
        prepared.floor,
        prepared.log_path.is_none(),
        home_dir(),
    ));
    let subscriber = build_subscriber(&prepared, Arc::clone(&state));
    if let Err(err) = tracing::subscriber::set_global_default(subscriber) {
        // Another library claimed the global subscriber first. Clearing the guard
        // keeps the state honest about what was installed, and a later call reports
        // the same conflict rather than pretending to have succeeded.
        INITIALISED.store(false, Ordering::Release);
        return Err(ImeError::ConfigInvalid {
            key: String::from("diag.log"),
            reason: err.to_string(),
        });
    }

    prepared.announce();
    Ok(DiagHandle {
        state,
        log_path: prepared.log_path.clone(),
        sink: Some(prepared.sink.clone()),
    })
}

/// The home directory whose prefix the log shortens to `~`, taken from `$HOME` --
/// the only source reachable from here without inverting the dependency direction.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| !home.as_os_str().is_empty())
}

/// Assembles the subscriber: the redaction layer, then the formatting layer.
fn build_subscriber(
    prepared: &Prepared,
    state: Arc<RedactState>,
) -> impl Subscriber + Send + Sync + 'static {
    let sink = prepared.sink.clone();
    registry().with(RedactLayer::new(Arc::clone(&state))).with(
        fmt::layer()
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .event_format(RedactFormat::new(state)),
    )
}

/// What `init_logging` emits once the subscriber is installed: either the log
/// directory could not be opened, or the input-content switch is on -- which changes
/// nothing about what is recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Notice {
    StderrFallback { reason: String },
    InputContentRequested,
}

/// The sink, the effective level and the startup notices, resolved before the
/// subscriber is built.
///
/// Splitting this out of [`init_logging`] is what makes the decisions testable: the
/// subscriber can be installed only once per process, but the policy that decides
/// where it writes and at which level can be exercised as often as a test likes.
struct Prepared {
    sink: Sink,
    level: LevelFilter,
    /// The verbosity floor the degradations impose, which a later reconfiguration of
    /// the configured level must respect.
    floor: LevelFilter,
    notices: Vec<Notice>,
    log_path: Option<PathBuf>,
}

impl Prepared {
    /// Opens the sink and folds the degradations into the level.
    fn open(cfg: &DiagConfig) -> Self {
        let path = cfg.log_dir.join(LOG_FILE_NAME);
        let opened = RotatingWriter::open(
            &cfg.log_dir,
            LOG_FILE_NAME,
            rotation_bytes(cfg),
            cfg.keep_files,
        );
        let (sink, log_path, failure) = match opened {
            Ok(writer) => (Sink::File(writer), Some(path), None),
            Err(err) => (Sink::Stderr, None, Some(err.to_string())),
        };

        let mut notices = Vec::new();
        if let Some(reason) = failure {
            notices.push(Notice::StderrFallback { reason });
        }
        if cfg.log_input_content {
            notices.push(Notice::InputContentRequested);
        }

        // The fallback shares the host's log with every other addon, so it writes
        // only what is worth a reader's attention. `LevelFilter` orders from least to
        // most verbose, so `min` is the stricter of the two -- `max` would pick the
        // more talkative one and defeat the point.
        let level = match log_path {
            Some(_) => effective_level(cfg),
            None => cmp::min(effective_level(cfg), LevelFilter::WARN),
        };
        // The floor mirrors the two degradations: a log that went to stderr is capped
        // at `warn` however talkative the configuration asks the process to be, and the
        // input-content switch raises the whole process to `debug` for good.
        let floor = match log_path {
            Some(_) if !cfg.log_input_content => LevelFilter::ERROR,
            Some(_) => LevelFilter::DEBUG,
            None => LevelFilter::WARN,
        };
        Self {
            sink,
            level,
            floor,
            notices,
            log_path,
        }
    }

    /// Emits the startup notices through the freshly installed subscriber.
    fn announce(&self) {
        for notice in &self.notices {
            match notice {
                Notice::StderrFallback { reason } => tracing::warn!(
                    code = "data/readonly-mode",
                    reason = %reason,
                    "the log directory is not writable; diagnostics go to stderr at warn and above"
                ),
                Notice::InputContentRequested => tracing::info!(
                    "rspinyin does not record what you type; this switch only adds structural logging"
                ),
            }
        }
    }
}

/// The level the configuration asks for, before the sink has its say.
fn effective_level(cfg: &DiagConfig) -> LevelFilter {
    match cfg.level {
        // `off` means "no diagnostics at all"; a switch that asks for more detail
        // does not override an explicit decision to log nothing.
        LevelFilter::OFF => LevelFilter::OFF,
        // `LevelFilter`'s ordering runs from least to most verbose
        // (`off < error < warn < info < debug < trace`), so a level that is *less*
        // verbose than `debug` is the one that has to be lowered. Getting this backwards
        // is silent: the default `info` never satisfies `> debug`, so the switch would
        // appear to do nothing at all.
        level if cfg.log_input_content && level < LevelFilter::DEBUG => LevelFilter::DEBUG,
        level => level,
    }
}

/// The size limit of the active log file, in bytes.
fn rotation_bytes(cfg: &DiagConfig) -> u64 {
    cfg.rotation_mb.saturating_mul(MEBIBYTE)
}

/// The destination of the diagnostics stream.
#[derive(Clone, Debug)]
enum Sink {
    /// The rolling file under the configured log directory.
    File(RotatingWriter),
    /// The host's stderr, used when the log directory cannot be opened.
    Stderr,
}
impl io::Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::File(writer) => writer.write(buf),
            Self::Stderr => io::stderr().write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::File(writer) => writer.flush(),
            Self::Stderr => io::stderr().flush(),
        }
    }
}

/// A log file that rolls over once it exceeds its size limit.
///
/// Cloning shares the file, the byte count and the rotation state, which is what the
/// subscriber needs: `MakeWriter` is called once per event, and the state has to
/// survive the write.
#[derive(Clone, Debug)]
struct RotatingWriter {
    inner: Arc<Mutex<RotatingFile>>,
}

impl RotatingWriter {
    /// Opens the active log file, creating the directory and the file as needed.
    ///
    /// `max_bytes` is the size at which the file rolls over, zero never rolls, and
    /// `keep_files` is how many rolled files stay behind the active one. The
    /// `io::Error` of creating the directory, opening the file or applying its
    /// permissions is returned to the caller, which degrades to `stderr`.
    fn open(dir: &Path, file_name: &str, max_bytes: u64, keep_files: usize) -> io::Result<Self> {
        let file = RotatingFile::open(dir, file_name, max_bytes, keep_files)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(file)),
        })
    }

    /// Swaps the rolling policy in place.
    ///
    /// The limits are atomics on the shared file state, so this is two lock-free
    /// stores: a log call that is mid-write keeps the limits it loaded, and the next
    /// one sees the new policy.
    fn set_policy(&self, max_bytes: u64, keep_files: usize) {
        self.lock().set_policy(max_bytes, keep_files);
    }
    /// Takes the file lock, recovering from poisoning.
    fn lock(&self) -> MutexGuard<'_, RotatingFile> {
        match self.inner.lock() {
            Ok(guard) => guard,
            // Poisoning means a thread panicked while writing a log line. The file
            // handle and the byte count are still consistent, so the logging path
            // recovers instead of turning one panic into many.
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl io::Write for RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut file = self.lock();
        if file.should_roll(buf.len()) {
            file.roll()?;
        }
        file.handle.write_all(buf)?;
        file.written += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.lock().handle.flush()
    }
}

/// The state behind the lock: the open file, its size, and the rolling policy.
#[derive(Debug)]
struct RotatingFile {
    dir: PathBuf,
    file_name: String,
    handle: File,
    written: u64,
    /// The size the active file rolls over at, in bytes; zero never rolls. Atomic so
    /// a configuration reload can re-aim it while writes are in flight.
    max_bytes: AtomicU64,
    /// How many rolled files stay behind the active one. Atomic for the same reason.
    keep_files: AtomicUsize,
}

impl RotatingFile {
    /// Opens `dir/file_name` for appending.
    fn open(dir: &Path, file_name: &str, max_bytes: u64, keep_files: usize) -> io::Result<Self> {
        create_private_dir(dir)?;
        let handle = open_private(&dir.join(file_name), true)?;
        // A log left behind by an earlier run counts towards the limit, so the first
        // line of this run can already be the one that rolls the file over.
        let written = handle.metadata()?.len();
        Ok(Self {
            dir: dir.to_path_buf(),
            file_name: String::from(file_name),
            handle,
            written,
            max_bytes: AtomicU64::new(max_bytes),
            keep_files: AtomicUsize::new(keep_files),
        })
    }

    /// Whether the next write would push the active file past its limit.
    fn should_roll(&self, incoming: usize) -> bool {
        // An empty file never rolls: a single line larger than the whole budget
        // would otherwise roll the file before every write and keep nothing.
        let max_bytes = self.max_bytes.load(Ordering::Acquire);
        max_bytes > 0
            && self.written > 0
            && self.written.saturating_add(incoming as u64) > max_bytes
    }

    /// Swaps the rolling policy in place, as [`RotatingWriter::set_policy`] describes.
    fn set_policy(&mut self, max_bytes: u64, keep_files: usize) {
        self.max_bytes.store(max_bytes, Ordering::Release);
        self.keep_files.store(keep_files, Ordering::Release);
    }

    /// Rolls the active file over to `.1`, shifting the history up, and opens a
    /// fresh active file.
    ///
    /// The lock is held throughout, so no write can interleave with the renames. The
    /// oldest file is removed before the shift, which is what bounds the directory
    /// at `keep_files + 1` files.
    fn roll(&mut self) -> io::Result<()> {
        let current = self.dir.join(&self.file_name);
        let keep_files = self.keep_files.load(Ordering::Acquire);
        if keep_files == 0 {
            // No history is kept, so the active file starts over where it is --
            // which also avoids unlinking a file this process still holds open.
            self.handle = open_private(&current, false)?;
        } else {
            remove_if_exists(&self.history_path(keep_files))?;
            for index in (1..keep_files).rev() {
                rename_if_exists(&self.history_path(index), &self.history_path(index + 1))?;
            }
            rename_if_exists(&current, &self.history_path(1))?;
            self.handle = open_private(&current, true)?;
        }
        self.written = 0;
        Ok(())
    }

    /// The path of the rolled file at `index`, where `.1` is the newest.
    fn history_path(&self, index: usize) -> PathBuf {
        self.dir.join(format!("{}.{index}", self.file_name))
    }
}

/// Creates `dir` and its parents with mode `0700`.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    enforce_mode(dir, 0o700)
}

/// Opens `path` for writing, creating it with mode `0600` when it is missing, and
/// appending to it when `append` is set.
///
/// A file left at `0644` by an earlier version, or by a laxer umask, is corrected
/// here: the mode passed to `open` applies only at creation, and a world-readable
/// log file is exactly what the privacy rule forbids.
fn open_private(path: &Path, append: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).write(true);
    if append {
        options.append(true);
    } else {
        options.truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // The umask can only clear bits from this, so the file is never wider.
        options.mode(0o600);
    }
    let handle = options.open(path)?;
    enforce_mode(path, 0o600)?;
    Ok(handle)
}

/// Sets `mode` on `path` unless it already has exactly that.
#[cfg(unix)]
fn enforce_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let permissions = fs::metadata(path)?.permissions();
    if permissions.mode() & 0o777 != mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

/// Hosts without mode bits create their files with the platform's own rules.
#[cfg(not(unix))]
fn enforce_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

/// Renames `from` to `to`, treating a missing source as success.
fn rename_if_exists(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// Removes `path`, treating a missing file as success.
fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests;
