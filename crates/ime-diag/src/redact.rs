//! Field-level redaction for the diagnostics stream.
//!
//! Responsibility: decide what may reach a log sink. [`RedactLayer`] applies the level filter
//! and records which sessions are sensitive; [`RedactFormat`] renders the compact text line
//! under the same rules. Both share one [`RedactState`], so the policy holds whichever order
//! the layers were composed in. Files, rotation and subscriber installation belong to
//! `crate::log`; this module never decides *whether* to log, only *what* may be written, and it
//! reads no clock, environment or configuration.
//!
//! # Rules
//!
//! | Field | What is written |
//! |---|---|
//! | `raw`, `text`, `preedit`, `input`, `candidate_text`, `word`, `commit_text` | `<redacted:len=N>`, `N` being the character count of the withheld value |
//! | `app` and anything else | the value, with a leading home-directory prefix rewritten to `~` |
//! | any field of a sensitive session | nothing, except the `app` hash |
//! | the message of a sensitive session | nothing |
//!
//! `N` counts the value as it would have been rendered: one recorded with `%` or as a plain
//! `&str` renders as itself, while one recorded with `?` renders with its `Debug` quoting.
//!
//! # The two lines of defence
//!
//! The code must never pass user content to a log event at all, which is what the zero-trace
//! assertion of the privacy task checks. This module is the second line, for a field passed by
//! mistake, and it is not a licence to pass content.
//!
//! # Sensitive sessions
//!
//! A session becomes sensitive when an event carrying `session = <id>` also carries
//! `password = true` (or `sensitive = true`, the flag's name in the host API). From then on
//! every event of that session is downgraded to `session=redacted` plus the application hash,
//! and the message, the fields and the input length are all dropped -- the length goes too,
//! because a password's length is itself a secret.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::{Event, Metadata, Subscriber};
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::fmt::format::{FormatEvent, Writer};
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

/// Field names whose values are never written to a sink. They are matched exactly, not by
/// substring: `raw_len` and `raw_bytes` are structural facts the diagnostics are supposed to
/// carry, while `raw` is the input itself.
pub const DENIED_FIELDS: [&str; 7] = [
    "raw",
    "text",
    "preedit",
    "input",
    "candidate_text",
    "word",
    "commit_text",
];

/// The field that names the session an event belongs to.
const SESSION_FIELD: &str = "session";

/// The fields whose `true` value marks their session as a sensitive context. `password` is the
/// flag the design names; `sensitive` is the second bit the host exposes, accepted so that a
/// caller reading only that one still gets the protection.
const SENSITIVE_FIELDS: [&str; 2] = ["password", "sensitive"];

/// The fields that stay visible in a sensitive session; `app` is already a hash.
const SENSITIVE_ALLOWED_FIELDS: [&str; 1] = ["app"];

/// The field that carries the event's message.
const MESSAGE_FIELD: &str = "message";

/// The text written in place of a withheld value.
const PLACEHOLDER_PREFIX: &str = "<redacted:len=";

/// Whether `name` is on the denylist, and its value must therefore be withheld. `name` is the
/// field name as `tracing` reports it; `raw_len` is not `raw`, so a structural fact about the
/// input stays readable while the input itself does not.
pub fn is_denied_field(name: &str) -> bool {
    DENIED_FIELDS.contains(&name)
}

/// Renders the placeholder that replaces a withheld value: `<redacted:len=N>`, where `len` is
/// the character count of the value that was withheld -- characters rather than bytes, because a
/// byte count of a pinyin buffer is a poor description of it.
pub fn redaction_placeholder(len: usize) -> String {
    format!("{PLACEHOLDER_PREFIX}{len}>")
}

/// Rewrites a leading home-directory prefix of `value` to `~`.
///
/// `home` is the directory to shorten; one that is not valid UTF-8, or the filesystem root, is
/// left alone because there is no shorter form to write. The value itself is returned when the
/// prefix does not match: a sibling directory that merely shares the first characters --
/// `/home/ann` against `/home/annette` -- does not match, because only a full path component
/// counts. Never panics.
///
/// # Examples
///
/// ```
/// use std::path::Path;
///
/// use ime_diag::redact::shorten_home;
///
/// let home = Path::new("/home/ann");
/// assert_eq!(shorten_home("/home/ann/log", home), "~/log");
/// assert_eq!(shorten_home("/home/ann", home), "~");
/// assert_eq!(shorten_home("/home/annette/log", home), "/home/annette/log");
/// ```
pub fn shorten_home<'a>(value: &'a str, home: &Path) -> Cow<'a, str> {
    let Some(home) = home.to_str() else {
        return Cow::Borrowed(value);
    };
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return Cow::Borrowed(value);
    }
    let Some(rest) = value.strip_prefix(home) else {
        return Cow::Borrowed(value);
    };
    if !rest.is_empty() && !rest.starts_with('/') {
        return Cow::Borrowed(value);
    }
    Cow::Owned(format!("~{rest}"))
}

/// What the observation pass learned about one event: the session it belongs to, when it
/// carries one, and whether the event itself carries the sensitive-context flag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Observation {
    session: Option<u64>,
    flagged: bool,
}

/// The state the redaction layer and the formatter share.
///
/// One instance is created per process and held in an [`Arc`] by the subscriber and by the
/// handle `init_logging` returns, which is how a caller that has just seen a password context
/// marks a session without going through a log event.
///
/// # Concurrency
///
/// `Send + Sync`. Both threads of the plugin log, so the sensitive-session set is behind a
/// mutex; the common case -- no session marked yet -- is answered from an atomic counter
/// without taking the lock, because the check sits on the path of every event.
#[derive(Debug)]
pub struct RedactState {
    level: LevelFilter,
    home: Option<PathBuf>,
    /// Number of marked sessions; zero short-circuits the lock.
    sensitive_count: AtomicUsize,
    sensitive: Mutex<BTreeSet<u64>>,
}

impl RedactState {
    /// Creates the state with the level filter to apply and the home directory to shorten, with
    /// no session marked sensitive. `home` may be `None` to leave paths as they are.
    pub fn new(level: LevelFilter, home: Option<PathBuf>) -> Self {
        Self {
            level,
            home,
            sensitive_count: AtomicUsize::new(0),
            sensitive: Mutex::new(BTreeSet::new()),
        }
    }

    /// The level filter this state applies, after `crate::log` has folded in the
    /// degradations it decided on: the `warn` floor of the stderr fallback and the `debug`
    /// floor of the input-content switch.
    pub fn level(&self) -> LevelFilter {
        self.level
    }

    /// The home directory whose prefix is rewritten to `~`, or `None` when paths are left as
    /// they are.
    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    /// Marks the session named by `session` as a sensitive context.
    ///
    /// Everything the session logs from now on is downgraded to `session=redacted` plus the
    /// application hash. Marking is idempotent and permanent: a session is never unmarked,
    /// because a password box that closes does not make the events it already logged safe.
    ///
    /// # Panics
    ///
    /// Never: a poisoned lock is recovered rather than propagated, since the set it guards
    /// is still consistent.
    pub fn mark_sensitive_session(&self, session: u64) {
        let mut sessions = self.lock_sessions();
        if sessions.insert(session) {
            // Release: a thread that observes a non-zero count must also observe the insert,
            // and it takes the lock before reading the set.
            self.sensitive_count.fetch_add(1, Ordering::Release);
        }
    }

    /// Whether `session` has been marked sensitive, through
    /// [`RedactState::mark_sensitive_session`] or by an event carrying the flag. Never
    /// panics.
    pub fn is_session_sensitive(&self, session: u64) -> bool {
        if self.sensitive_count.load(Ordering::Acquire) == 0 {
            return false;
        }
        self.lock_sessions().contains(&session)
    }

    /// Reads the two fields the policy keys on and marks the session they flag. The marking
    /// happens for the event that carries the flag as well as for the events after it, so a
    /// `password = true` line never reaches a sink in its undowngraded form.
    fn observe(&self, event: &Event<'_>) -> Observation {
        let mut observation = Observation::default();
        event.record(&mut ObserveVisitor {
            observation: &mut observation,
        });
        if observation.flagged {
            if let Some(session) = observation.session {
                self.mark_sensitive_session(session);
            }
        }
        observation
    }

    /// Takes the sensitive-session lock, recovering from poisoning.
    fn lock_sessions(&self) -> MutexGuard<'_, BTreeSet<u64>> {
        match self.sensitive.lock() {
            Ok(guard) => guard,
            // Poisoning means another thread panicked while holding the lock. The set is a
            // plain collection of integers and stays consistent, so the logging path recovers
            // instead of turning one panic into many.
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// The visitor of the observation pass: it reads only the fields the policy keys on and
/// renders nothing, which keeps the pass cheap enough to run on every event.
struct ObserveVisitor<'a> {
    observation: &'a mut Observation,
}

impl ObserveVisitor<'_> {
    /// Reads one already-rendered field value.
    fn read(&mut self, name: &str, text: &str) {
        if name == SESSION_FIELD {
            self.observation.session = parse_session(text);
        } else if SENSITIVE_FIELDS.contains(&name) && text.trim() == "true" {
            self.observation.flagged = true;
        }
    }
}

impl Visit for ObserveVisitor<'_> {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == SESSION_FIELD {
            self.observation.session = Some(value);
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if value && SENSITIVE_FIELDS.contains(&field.name()) {
            self.observation.flagged = true;
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.read(field.name(), value);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let name = field.name();
        if name != SESSION_FIELD && !SENSITIVE_FIELDS.contains(&name) {
            return;
        }
        // A session id or a flag arriving through `%` or `?` is rendered here rather than
        // through a shared buffer: only these two names are ever rendered, so the allocation
        // cannot show up on a hot path.
        self.read(name, &format!("{value:?}"));
    }
}

/// Reads a session id out of a rendered field value: the id may arrive as a bare number
/// (`session = id.value()`) or wrapped in a newtype's `Debug` output (`session = ?id` renders
/// `SessionId(42)`), so the first run of decimal digits is what counts.
fn parse_session(text: &str) -> Option<u64> {
    let start = text.find(|ch: char| ch.is_ascii_digit())?;
    let end = text[start..]
        .find(|ch: char| !ch.is_ascii_digit())
        .map_or(text.len(), |offset| start + offset);
    text.get(start..end)?.parse().ok()
}

/// The redaction layer: applies the level filter and marks sensitive sessions.
///
/// It is the layer half of the policy; the rendering half is [`RedactFormat`]. Both observe
/// events, so the session set is correct no matter which of the two the subscriber notifies
/// first, and so any future consumer of the same [`RedactState`] sees the same marks.
/// `Send + Sync`, and it holds no per-event state.
#[derive(Clone, Debug)]
pub struct RedactLayer {
    state: Arc<RedactState>,
}

impl RedactLayer {
    /// Creates the layer over shared redaction state. The formatter has to be given the same
    /// instance, or the two halves of the policy disagree. Never panics.
    pub fn new(state: Arc<RedactState>) -> Self {
        Self { state }
    }
}

impl<S: Subscriber> Layer<S> for RedactLayer {
    fn enabled(&self, metadata: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        metadata.level() <= &self.state.level
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        // Publishing the filter as a hint is what makes a `debug!` below the configured level
        // cost a comparison in the macro rather than a call into the subscriber.
        Some(self.state.level)
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        self.state.observe(event);
    }
}

/// The compact-text event formatter: one line per event.
///
/// ```text
/// 2026-09-29T10:35:12.345678Z INFO ime_fcitx5::addon: session: start session=42 app=0x8f3a2c1d
/// ```
///
/// Compact text rather than JSON because the log is read by a person debugging a live input
/// method. The module target after the level tells a reader which crate emitted the line, and
/// fields are rendered lazily: one the level filter drops is never formatted at all. `Send +
/// Sync`; the shared state is synchronized.
#[derive(Debug)]
pub struct RedactFormat {
    state: Arc<RedactState>,
    timer: SystemTime,
}

impl RedactFormat {
    /// Creates the formatter over shared redaction state -- the same instance the layer was
    /// given, since that is where it reads the home directory from.
    pub fn new(state: Arc<RedactState>) -> Self {
        Self {
            state,
            timer: SystemTime,
        }
    }
}

impl<S, N> FormatEvent<S, N> for RedactFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> tracing_subscriber::fmt::format::FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let metadata = event.metadata();
        // Observe before rendering: the event that carries the flag is downgraded too.
        let observation = self.state.observe(event);
        let sensitive = observation.flagged
            || observation
                .session
                .is_some_and(|session| self.state.is_session_sensitive(session));

        // A timer that fails costs the timestamp, not the line: a diagnostic that cannot be
        // read is worse than one without a clock reading.
        if self.timer.format_time(&mut writer).is_err() {
            writer.write_str("<unknown time>")?;
        }
        // The header ends with the separator the first field relies on; the visitor pads the rest.
        write!(writer, " {} {}: ", metadata.level(), metadata.target())?;
        if sensitive {
            writer.write_str("session=redacted")?;
        }

        let mut visitor = RedactVisitor::new(writer, self.state.home(), sensitive);
        event.record(&mut visitor);
        visitor.finish()
    }
}

/// The visitor that renders an event's fields under the redaction rules.
///
/// It owns the writer, so the terminating newline is written by [`Self::finish`]. A formatting
/// error is remembered and returned once, the way `tracing-subscriber`'s own field visitor does
/// it, so one failed field does not silently produce a half line.
struct RedactVisitor<'w, 'h> {
    writer: Writer<'w>,
    home: Option<&'h Path>,
    sensitive: bool,
    /// Whether the next field is the first one on the line.
    is_empty: bool,
    result: fmt::Result,
    /// Reused across the fields of one event, so rendering a `Debug` value costs no
    /// allocation per field.
    scratch: String,
}

impl<'w, 'h> RedactVisitor<'w, 'h> {
    /// Creates the visitor for one event. `is_empty` starts out as `!sensitive`, because a
    /// downgraded event already carries `session=redacted` and the next field still needs its
    /// separator.
    fn new(writer: Writer<'w>, home: Option<&'h Path>, sensitive: bool) -> Self {
        Self {
            writer,
            home,
            sensitive,
            is_empty: !sensitive,
            result: Ok(()),
            scratch: String::new(),
        }
    }

    /// Writes the terminating newline and reports the first formatting error.
    fn finish(mut self) -> fmt::Result {
        if self.result.is_err() {
            return self.result;
        }
        writeln!(self.writer)
    }

    /// Writes the separator that precedes a field, if one is needed.
    fn pad(&mut self) {
        if self.is_empty {
            self.is_empty = false;
        } else {
            self.result = self.writer.write_str(" ");
        }
    }

    /// Writes one already-decided value.
    ///
    /// Newlines and carriage returns are escaped so that a value can never forge a second log
    /// line. The first formatting error is kept: a later successful write must not turn a
    /// truncated line back into an accepted one.
    fn write_value(&mut self, text: &str) {
        for ch in text.chars() {
            if self.result.is_err() {
                return;
            }
            self.result = match ch {
                '\n' => self.writer.write_str("\\n"),
                '\r' => self.writer.write_str("\\r"),
                other => self.writer.write_char(other),
            };
        }
    }

    /// Writes one `name=value` pair.
    fn write_pair(&mut self, name: &str, value: &str) {
        if self.result.is_err() {
            return;
        }
        self.pad();
        self.result = self.writer.write_str(name);
        if self.result.is_err() {
            return;
        }
        self.result = self.writer.write_char('=');
        if self.result.is_err() {
            return;
        }
        self.write_value(value);
    }

    /// Applies the policy to one field and writes what is left of it.
    fn record_text(&mut self, name: &str, text: &str) {
        if self.result.is_err() {
            return;
        }
        if self.sensitive {
            // A sensitive session keeps the application hash and nothing else -- not the
            // message, not the fields, and not the input length, which would leak the length
            // of a password.
            if SENSITIVE_ALLOWED_FIELDS.contains(&name) {
                self.write_pair(name, text);
            }
            return;
        }
        if is_denied_field(name) {
            let placeholder = redaction_placeholder(text.chars().count());
            self.write_pair(name, &placeholder);
            return;
        }
        let value = match self.home {
            Some(home) => shorten_home(text, home),
            None => Cow::Borrowed(text),
        };
        if name == MESSAGE_FIELD {
            // The message is a static template by project rule, so it is written as it is --
            // with the home prefix shortened like any other value.
            self.pad();
            self.write_value(&value);
            return;
        }
        self.write_pair(name, &value);
    }
}

impl Visit for RedactVisitor<'_, '_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        // A `&str` value arrives here unrendered, which is what gives the placeholder an
        // exact character count.
        self.record_text(field.name(), value);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if self.result.is_err() {
            return;
        }
        // `%value` and `?value` both arrive here, and a `Debug` value carries no borrowable
        // string, so it is rendered once into the reusable buffer and inspected from there. The
        // buffer is taken out of `self` so the rendered text can be read while the visitor is
        // written to.
        self.scratch.clear();
        if write!(self.scratch, "{value:?}").is_err() {
            // A `Debug` implementation that fails halfway must not drop the field from the line
            // without saying so.
            self.scratch.clear();
            self.scratch.push_str("<unformattable>");
        }
        let text = std::mem::take(&mut self.scratch);
        self.record_text(field.name(), &text);
        self.scratch = text;
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use tracing_subscriber::fmt;
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::registry;

    use super::*;

    /// The home directory the tests shorten, chosen so that a sibling directory shares its
    /// first characters and proves the boundary check.
    const TEST_HOME: &str = "/home/tester";

    /// A `MakeWriter` sink that keeps everything written to it in memory.
    #[derive(Clone, Debug, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Capture {
        /// Everything written so far, as text.
        fn text(&self) -> String {
            let bytes = self.0.lock().expect("the capture lock is not poisoned");
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }

    impl io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("not poisoned").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Installs the redaction pipeline on the current thread.
    ///
    /// The subscriber is a thread-local default rather than the global one, so tests stay
    /// independent of each other. The returned guard keeps it installed and must be held.
    fn capture(state: &Arc<RedactState>) -> (Capture, tracing::subscriber::DefaultGuard) {
        let sink = Capture::default();
        let writer = sink.clone();
        let subscriber = registry()
            .with(RedactLayer::new(Arc::clone(state)))
            .with(
                fmt::layer()
                    .with_ansi(false)
                    .with_writer(move || writer.clone())
                    .event_format(RedactFormat::new(Arc::clone(state))),
            );
        let guard = tracing::subscriber::set_default(subscriber);
        (sink, guard)
    }

    /// A state that filters at `level` and shortens [`TEST_HOME`].
    fn state_at(level: LevelFilter) -> Arc<RedactState> {
        Arc::new(RedactState::new(level, Some(PathBuf::from(TEST_HOME))))
    }

    /// The line of `text` that contains `needle`.
    fn line_with<'a>(text: &'a str, needle: &str) -> &'a str {
        text.lines()
            .find(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("no line contains {needle:?} in:\n{text}"))
    }

    #[test]
    fn test_redact_format_withholds_every_denied_field() {
        let state = state_at(LevelFilter::INFO);
        let (sink, _guard) = capture(&state);
        let raw = String::from("nihao");
        let debugged = String::from("mima");

        tracing::info!(raw = %raw, candidate_count = 7, "candidates built");
        tracing::info!(preedit = "ni'hao", text = %"shang", "committed");
        // `?raw` renders through `Debug` instead of borrowing the string; it is withheld there
        // too, with the quoting included in the count.
        tracing::info!(raw = ?debugged, "key handled");
        // Below the configured level, so it never reaches the sink at all.
        tracing::debug!(raw = %raw, "dropped");

        let text = sink.text();
        assert_eq!(text.lines().count(), 3, "{text}");
        for withheld in ["<redacted:len=5>", "<redacted:len=6>"] {
            assert!(text.contains(withheld), "{withheld} missing from:\n{text}");
        }
        // Two six-character placeholders: `ni'hao`, and the `Debug` rendering of `mima`, which
        // carries its quotes into the count.
        assert_eq!(text.matches("<redacted:len=6>").count(), 2, "{text}");
        for leaked in ["nihao", "ni'hao", "shang", "mima"] {
            assert!(!text.contains(leaked), "{leaked} leaked into:\n{text}");
        }
        // The structural fields around the withheld one are what the diagnostics are for.
        assert!(text.contains("candidate_count=7"), "{text}");
    }

    #[test]
    fn test_redact_format_redacts_denied_fields_at_debug_level_too() {
        // The input-content switch only raises the level; it cannot turn characters back on,
        // so the denylist holds at `Debug` exactly as it does at `Info`.
        let state = state_at(LevelFilter::DEBUG);
        let (sink, _guard) = capture(&state);
        let preedit = String::from("ni hao");

        tracing::debug!(preedit = %preedit, dag_edges = 12, "segmentation built");

        let text = sink.text();
        assert!(text.contains("<redacted:len=6>"), "{text}");
        assert!(!text.contains("ni hao"), "{text}");
        // The structural substitute the design asks for is present.
        assert!(text.contains("dag_edges=12"), "{text}");
    }

    #[test]
    fn test_redact_format_downgrades_every_event_of_a_sensitive_session() {
        let state = state_at(LevelFilter::INFO);
        let (sink, _guard) = capture(&state);

        // The flag arrives with the session's first event, which is when a host that reads
        // `CapabilityFlag::Password` knows the context is a password box.
        tracing::info!(session = 42u64, app = 0x8f3a_2c1du64, password = true, "session: start");
        tracing::info!(session = 42u64, candidate_count = 7, raw_len = 5, "candidates built");
        tracing::info!(session = 7u64, candidate_count = 3, "candidates built");

        let text = sink.text();
        assert!(!text.contains("session=42"), "{text}");
        // Neither the flag, nor the input length, nor the candidate count, nor the message of
        // the sensitive session is written.
        for withheld in ["password", "raw_len", "candidate_count=7", "session: start"] {
            assert!(!text.contains(withheld), "{withheld} leaked into:\n{text}");
        }
        let downgraded = text.lines().filter(|l| l.contains("session=redacted")).count();
        assert_eq!(downgraded, 2, "{text}");
        // The one field a downgraded event keeps is the application hash, and the other session
        // is untouched.
        let sensitive = line_with(&text, "session=redacted");
        assert!(sensitive.contains("app=2402954269"), "{sensitive}");
        assert!(line_with(&text, "candidate_count=3").contains("session=7"), "{text}");
    }

    #[test]
    fn test_redact_state_mark_sensitive_session_downgrades_following_events() {
        // The path a host that has just seen `CapabilityFlag::Password` takes: no event has to
        // carry the flag, so no level change can lose the mark.
        let state = state_at(LevelFilter::INFO);
        let (sink, _guard) = capture(&state);
        state.mark_sensitive_session(9);

        tracing::info!(session = 9u64, raw_len = 4, "key handled");

        let text = sink.text();
        assert!(text.contains("session=redacted"), "{text}");
        assert!(!text.contains("raw_len") && !text.contains("session=9"), "{text}");
    }

    #[test]
    fn test_observe_visitor_reads_the_flag_from_a_rendered_boolean() {
        // `password = %flag` renders through `Display` instead of `record_bool`; both
        // spellings have to mark the session.
        let state = state_at(LevelFilter::INFO);
        let (sink, _guard) = capture(&state);
        let flagged = true;

        tracing::info!(session = 3u64, password = %flagged, "session: start");
        tracing::info!(session = 3u64, raw_len = 8, "key handled");

        let text = sink.text();
        assert!(!text.contains("raw_len"), "{text}");
        let downgraded = text.lines().filter(|l| l.contains("session=redacted")).count();
        assert_eq!(downgraded, 2, "{text}");
    }

    #[test]
    fn test_redact_format_shortens_the_home_prefix() {
        let state = state_at(LevelFilter::INFO);
        let (sink, _guard) = capture(&state);
        let path = String::from("/home/tester/.local/share/rspinyin/user.redb");

        tracing::info!(path = %path, "user database opened");
        tracing::info!(path = "/home/tester", "home itself");

        let text = sink.text();
        assert!(text.contains("path=~/.local/share/rspinyin/user.redb"), "{text}");
        let home_line = line_with(&text, "home itself").trim_end();
        assert!(home_line.ends_with("path=~"), "{home_line}");
        assert!(!text.contains(TEST_HOME), "{text}");
    }

    #[test]
    fn test_shorten_home_requires_a_path_component_boundary() {
        let home = Path::new(TEST_HOME);
        assert_eq!(shorten_home("/home/tester2/log", home), "/home/tester2/log");
        assert_eq!(shorten_home("/home/test", home), "/home/test");
        assert_eq!(shorten_home("", home), "");
        // A home directory given with a trailing separator behaves the same way.
        assert_eq!(shorten_home("/home/tester/log", Path::new("/home/tester/")), "~/log");
        // A home directory that is not valid UTF-8 cannot be matched as text.
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;

            let raw = std::ffi::OsStr::from_bytes(b"/home/\xff");
            assert_eq!(shorten_home("/home/x/log", Path::new(raw)), "/home/x/log");
        }
    }

    #[test]
    fn test_redact_format_escapes_newlines_and_reports_the_event_identity() {
        let state = state_at(LevelFilter::INFO);
        let (sink, _guard) = capture(&state);

        // A value that carries a newline must not be able to forge a second line.
        tracing::info!(path = %"first\nINFO forged: line", "user database opened");
        tracing::warn!(session = 5u64, "decode gave up");

        let text = sink.text();
        assert_eq!(text.lines().count(), 2, "{text}");
        assert!(text.contains("first\\nINFO forged: line"), "{text}");
        // The module target is what tells a reader which crate emitted the line.
        let line = line_with(&text, "session=5");
        assert!(line.contains("WARN"), "{line}");
        assert!(line.contains("ime_diag::redact::tests"), "{line}");
        assert!(line.contains("decode gave up") && line.ends_with('\n'), "{line:?}");
    }

    #[test]
    fn test_is_denied_field_matches_the_documented_list() {
        let denied = [
            "raw",
            "text",
            "preedit",
            "input",
            "candidate_text",
            "word",
            "commit_text",
        ];
        assert_eq!(DENIED_FIELDS, denied);
        for name in denied {
            assert!(is_denied_field(name), "{name}");
        }
        // Structural facts about the input, and the identity fields, are not content.
        for allowed in ["raw_len", "candidate_count", "app", "session", "path", "message"] {
            assert!(!is_denied_field(allowed), "{allowed}");
        }
    }

    #[test]
    fn test_redaction_helpers_count_characters_and_read_session_ids() {
        // `chars().count()` makes a multi-byte input report its length rather than its size.
        assert_eq!(redaction_placeholder("你好".chars().count()), "<redacted:len=2>");
        assert_eq!(parse_session("42"), Some(42));
        assert_eq!(parse_session("SessionId(42)"), Some(42));
        assert_eq!(parse_session("none"), None);
    }
}
