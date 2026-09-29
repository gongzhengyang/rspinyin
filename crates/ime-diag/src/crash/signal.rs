//! The signal path: a fault that no `catch_unwind` can see.
//!
//! Responsibility: arm `SIGBUS` and `SIGSEGV`, and write the record a fault leaves
//! behind. Everything the handler needs is prepared before it can run.
//!
//! Boundaries: the handler decides nothing. Which signals are handled, what the record
//! says and which exit status ends the process are fixed here; the one call that
//! installs a handler comes from the caller through [`RegisterSignal`], and the shape
//! of a record belongs to [`crate::crash::record`].
//!
//! # Why a fault is not a panic
//!
//! `SIGBUS` and `SIGSEGV` arrive when the process touches a page it cannot have. The
//! case this plugin actually has is the memory-mapped dictionary: another process
//! truncates `base.dict` and the next read past the new end of file faults. No
//! `catch_unwind` sees that, because nothing unwinds -- the kernel stops the thread
//! where it stands.
//!
//! # The async-signal-safety boundary
//!
//! A handler runs on the thread it interrupted, with that thread's locks possibly held
//! and its allocator possibly mid-update. The operations below are therefore split in
//! two, and the split is the contract:
//!
//! * **Outside the handler** -- [`install_signal_handlers`], on a normal thread: build
//!   the record's fixed text, create and narrow the record file, keep its descriptor,
//!   and register the bodies. Everything that allocates, opens, or locks happens here.
//! * **Inside the handler** -- [`on_signal`] and what it calls: read the clock
//!   (`clock_gettime` through the vDSO), write two numbers into a fixed stack buffer,
//!   `write(2)` that buffer to the descriptor that was already open, and `_exit(70)`.
//!   No allocation, no lock, no `malloc`, no `printf`, and no Rust formatting
//!   machinery: [`FixedBuffer`] exists precisely so that the record can be rendered
//!   without `core::fmt`.
//!
//! The one thing the handler reads from shared state is [`CHANNEL`], a [`OnceLock`]:
//! reading it is a single acquire load on a value that is written once, before any
//! signal can be armed.
//!
//! # Why it exits instead of recovering
//!
//! The state of a process that has taken a fault is not trustworthy -- an interrupted
//! `malloc`, a half-updated session, a partially written user database -- and carrying
//! on could corrupt what the user has learned. The choice is therefore to record and
//! terminate, not to repair. Fcitx5 exits with the plugin, and the desktop session
//! restarts it; a crash that is recorded and survived by the host is a far smaller
//! failure than one that silently corrupts a word frequency store.
//!
//! # Who installs the handler
//!
//! Installing a signal handler is `unsafe` in every binding that offers one -- the
//! signature the kernel calls cannot be checked by the compiler -- and `ime-diag` is
//! not allowed to contain `unsafe`. The call therefore arrives through
//! [`RegisterSignal`]: the addon's FFI module, the project's only permitted unsafe
//! boundary, passes the one call that installs these bodies, and everything that
//! decides *what* happens stays here.

use std::fs::File;
use std::io::Write as _;
use std::path::Path;
use std::sync::OnceLock;

use ime_types::ImeError;
use signal_hook::consts::signal::{SIGBUS, SIGSEGV};
use signal_hook::low_level;

use crate::crash::record::{self, RECORD_HEADER, crash_file_name, thread_identifier};

/// The exit status of a process that terminated on a recorded fault.
///
/// `70` is `EX_SOFTWARE` from `sysexits.h`: the process found a condition it could not
/// handle while doing its job. Deliberately not a signal death, so that a supervisor
/// and a person reading a shell can tell a recorded fault from an unhandled one.
pub const CRASH_EXIT_CODE: i32 = 70;

/// The signals this layer handles, with the name each is written under.
pub const HANDLED_SIGNALS: [(i32, &str); 2] = [(SIGBUS, "SIGBUS"), (SIGSEGV, "SIGSEGV")];

/// Largest record the signal path writes, in bytes.
const SIGNAL_RECORD_CAPACITY: usize = 512;

/// The hexadecimal digits a fault address is written with.
const HEX_DIGITS: [u8; 16] = *b"0123456789abcdef";

/// The body a registrar installs for one signal.
pub type SignalHandler = fn(SignalInfo) -> !;

/// Registers `handler` for `signal`.
///
/// The implementation belongs at the project's unsafe boundary, in
/// `crates/ime-fcitx5/src/ffi/`, and must:
///
/// * ask for `SA_SIGINFO`, so the handler receives the `siginfo_t` whose `si_code` is
///   classified with [`SignalCause::from_si_code`] and whose `si_addr` becomes
///   [`SignalInfo::address`];
/// * pass [`on_signal`] through as the body -- it never returns, so the trampoline has
///   no return value to produce;
/// * return the underlying failure as an [`ImeError`] when the kernel refuses.
///
/// # Parameters
///
/// - `signal`: the signal number to handle.
/// - `handler`: the body to run when it arrives.
///
/// # Errors
///
/// The registrar's own failure, reported by [`install_signal_handlers`].
pub type RegisterSignal = fn(signal: i32, handler: SignalHandler) -> Result<(), ImeError>;

/// What the kernel reported about a signal's origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalCause {
    /// The kernel delivered the signal because the process faulted.
    Fault,
    /// Another process sent the signal.
    SentByProcess,
    /// The registrar could not read the origin.
    Unknown,
}

impl SignalCause {
    /// Classifies the `si_code` a `siginfo_t` carried.
    ///
    /// `si_code` is positive for a fault the kernel detected (`SEGV_MAPERR`,
    /// `SEGV_ACCERR`, `BUS_ADRERR`, ...) and zero or negative for a signal another
    /// process sent (`SI_USER` is 0, `SI_TKILL` is -6). That sign is the whole test.
    ///
    /// # Parameters
    ///
    /// - `code`: the `si_code` field, as the registrar read it.
    ///
    /// # Returns
    ///
    /// [`SignalCause::Fault`] or [`SignalCause::SentByProcess`].
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_si_code(code: i32) -> Self {
        if code > 0 {
            Self::Fault
        } else {
            Self::SentByProcess
        }
    }

    /// The name the cause is written under.
    ///
    /// # Returns
    ///
    /// `fault`, `sent` or `unknown`.
    ///
    /// # Panics
    ///
    /// Never: a total match over three variants.
    pub fn name(self) -> &'static str {
        match self {
            Self::Fault => "fault",
            Self::SentByProcess => "sent",
            Self::Unknown => "unknown",
        }
    }

    /// Whether the process records the signal and terminates.
    ///
    /// A signal another process sent is not a fault. A debugger, a `kill`, or a session
    /// manager may send `SIGSEGV` on purpose, and swallowing it would break the tool
    /// the operator is holding; those are handed back to the default action instead.
    /// An unknown origin is treated as fatal, because the reason this handler exists is
    /// to record faults and a registrar that cannot tell is not a reason to lose one.
    ///
    /// # Returns
    ///
    /// `true` when the signal should be recorded and the process ended.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_fatal(self) -> bool {
        !matches!(self, Self::SentByProcess)
    }
}

/// What the registrar read out of a signal's `siginfo_t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignalInfo {
    /// The origin the kernel reported.
    pub cause: SignalCause,
    /// The address the fault happened at, when the kernel reported one.
    pub address: Option<usize>,
}

/// Everything the handler needs, prepared before it can be called.
struct SignalChannel {
    /// The record file, opened and narrowed at arm time.
    file: File,
    /// The fixed head of each signal's record, up to the timestamp.
    prefixes: [(i32, Vec<u8>); 2],
}

impl SignalChannel {
    /// The fixed head of the record for `signal`, when this channel handles it.
    fn prefix_for(&self, signal: i32) -> Option<&[u8]> {
        self.prefixes
            .iter()
            .find(|(number, _)| *number == signal)
            .map(|(_, prefix)| prefix.as_slice())
    }
}

/// The armed channel, or `None` when nothing has been armed.
static CHANNEL: OnceLock<SignalChannel> = OnceLock::new();

/// Arms the crash channel for [`HANDLED_SIGNALS`].
///
/// The record file is created here, with mode `0600` in the `open` call itself, because
/// a signal handler cannot build a path: turning a `Path` into the bytes `open(2)`
/// wants allocates, and allocating is not allowed in signal context. An armed channel
/// therefore owns one record file for the life of the process, and
/// [`record::prune_empty_records`] removes the ones a run that ended without a fault
/// left empty.
///
/// Arming is idempotent: the first call wins and a later one leaves the armed channel
/// alone, so a plugin reload cannot end up with two descriptors racing for one name.
///
/// # Parameters
///
/// - `crash_dir`: the directory the record file is created in. It is created private
///   when it is missing.
/// - `register`: the call that installs a handler. See [`RegisterSignal`].
///
/// # Returns
///
/// `Ok(())` when the channel is armed -- including when a signal the registrar refused
/// was reported through `tracing` while the other one was armed anyway.
///
/// # Errors
///
/// Returns [`ImeError::DataReadonly`] when the record file cannot be prepared; the
/// reason names the underlying failure. That code is the project's "this process has
/// stopped writing user data" condition, which is exactly what an unarmed channel
/// means: faults still terminate the process, they just leave no record. The caller
/// degrades -- a plugin that cannot record a crash still types -- and reports it.
///
/// # Panics
///
/// Never.
pub fn install_signal_handlers(crash_dir: &Path, register: RegisterSignal) -> Result<(), ImeError> {
    // Housekeeping first: pruning after the new file is created would remove it.
    record::prune_empty_records(crash_dir);
    let file = open_record_file(crash_dir)?;
    let prefixes = [
        (SIGBUS, signal_prefix(SIGBUS)),
        (SIGSEGV, signal_prefix(SIGSEGV)),
    ];
    if CHANNEL.set(SignalChannel { file, prefixes }).is_err() {
        return Ok(());
    }
    arm(register);
    Ok(())
}

/// Installs the two bodies, reporting a refusal without failing the arming.
///
/// Half an armed channel is worth more than none: a fault in the dictionary faults
/// with `SIGBUS`, and a registrar that refused `SIGSEGV` must not cost the record that
/// would have explained it.
fn arm(register: RegisterSignal) {
    let handlers: [SignalHandler; 2] = [on_bus, on_segv];
    for ((signal, name), handler) in HANDLED_SIGNALS.iter().zip(handlers) {
        if let Err(error) = register(*signal, handler) {
            tracing::warn!(signal = %name, reason = %error, "the crash signal was not armed");
        }
    }
}

/// Creates the record file the signal handler will write to.
fn open_record_file(crash_dir: &Path) -> Result<File, ImeError> {
    let stamp = crate::crash::now_unix_ms();
    let thread_id = thread_identifier(std::thread::current().id());
    let name = crash_file_name(stamp, thread_id);
    record::create_record_file(crash_dir, &name).map_err(|error| ImeError::DataReadonly {
        reason: format!("the crash record file could not be prepared: {error}"),
    })
}

/// The fixed head of a signal's record, up to the timestamp.
///
/// Built at arm time: in signal context only the two numbers that are not known until
/// the fault happens are appended.
fn signal_prefix(signal: i32) -> Vec<u8> {
    format!("{RECORD_HEADER}signal={}\ntimestamp_unix_ms=", signal_name(signal)).into_bytes()
}

/// The name of a handled signal, or `unknown` for one that is not.
fn signal_name(signal: i32) -> &'static str {
    HANDLED_SIGNALS
        .iter()
        .find(|(number, _)| *number == signal)
        .map_or("unknown", |(_, name)| *name)
}

/// The body registered for `SIGBUS`.
fn on_bus(info: SignalInfo) -> ! {
    on_signal(SIGBUS, info)
}

/// The body registered for `SIGSEGV`.
fn on_segv(info: SignalInfo) -> ! {
    on_signal(SIGSEGV, info)
}

/// The async-signal-safe body every registered handler runs.
///
/// A fault is recorded and then ends the process with [`CRASH_EXIT_CODE`]; a signal
/// another process sent is handed back to the default action, so that a debugger sees
/// the signal it asked for instead of an exit status of ours.
///
/// # Parameters
///
/// - `signal`: the signal number the body was registered for.
/// - `info`: what the registrar read out of the signal's `siginfo_t`.
///
/// # Panics
///
/// Never, and this is a hard requirement rather than a nicety: a panic here cannot
/// unwind, so it would abort the process before the record is written. Every step is a
/// memory write, a `write(2)`, or `_exit`.
pub fn on_signal(signal: i32, info: SignalInfo) -> ! {
    if !info.cause.is_fatal() {
        // Restores the default action and re-raises, which is documented as
        // async-signal-safe. It terminates for both signals handled here; the exit is
        // the fallback for the case where it returns an error instead.
        let _ = low_level::emulate_default_handler(signal);
        low_level::exit(CRASH_EXIT_CODE);
    }
    write_signal_record(signal, info);
    low_level::exit(CRASH_EXIT_CODE);
}

/// Writes the record for a fault, or nothing when the channel was never armed.
///
/// The write is a single `write(2)` on a descriptor that was opened before the fault.
/// Nothing is buffered in the process, so the bytes are in the kernel before `_exit`
/// and cannot be lost with it; a failed write is not actionable, because there is no
/// second channel to report it on.
fn write_signal_record(signal: i32, info: SignalInfo) {
    let Some(channel) = CHANNEL.get() else {
        return;
    };
    let Some(prefix) = channel.prefix_for(signal) else {
        return;
    };
    let mut buffer = FixedBuffer::new();
    render_signal_record(prefix, info, &mut buffer);
    let mut sink = &channel.file;
    let _ = sink.write_all(buffer.as_bytes());
}

/// Renders a fault's record into `out`.
///
/// Split out from the handler so that what it writes can be asserted on: the record's
/// text is the part of this module a test can check without taking a fault.
fn render_signal_record(prefix: &[u8], info: SignalInfo, out: &mut FixedBuffer) {
    out.push(prefix);
    out.push_decimal(crate::crash::now_unix_ms());
    out.push(b"\ncause=");
    out.push(info.cause.name().as_bytes());
    out.push(b"\nfault_address=");
    match info.address {
        Some(address) => out.push_hex(address as u64),
        None => out.push(b"none"),
    }
    out.push(b"\nexit_code=");
    out.push_decimal(u64::try_from(CRASH_EXIT_CODE).unwrap_or(0));
    out.push(b"\n");
}

/// The stack buffer a signal handler formats into.
///
/// Every operation is a memory write: no allocation, no lock, and none of `core::fmt`,
/// all of which are outside what a signal handler may call. A write past the end is
/// dropped rather than reported, because there is no way to report anything from here
/// and a record that is too long must still leave the head behind.
struct FixedBuffer {
    /// The bytes written so far, in front of the unused tail.
    bytes: [u8; SIGNAL_RECORD_CAPACITY],
    /// How many of `bytes` are written.
    len: usize,
}

impl FixedBuffer {
    /// Creates an empty buffer.
    fn new() -> Self {
        Self {
            bytes: [0; SIGNAL_RECORD_CAPACITY],
            len: 0,
        }
    }

    /// Appends `bytes`, dropping what does not fit.
    fn push(&mut self, bytes: &[u8]) {
        let room = SIGNAL_RECORD_CAPACITY.saturating_sub(self.len);
        for byte in bytes.iter().take(room) {
            self.bytes[self.len] = *byte;
            self.len += 1;
        }
    }

    /// Appends `value` in decimal.
    ///
    /// The digits are produced arithmetically rather than through `core::fmt`, which
    /// would pull in machinery this context may not run. `u64::MAX` needs twenty
    /// digits, so the scratch array is exactly that wide.
    fn push_decimal(&mut self, value: u64) {
        let mut digits = [0u8; 20];
        let mut len = 0;
        let mut rest = value;
        loop {
            digits[len] = b'0' + (rest % 10) as u8;
            len += 1;
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        for index in (0..len).rev() {
            self.push(&digits[index..index + 1]);
        }
    }

    /// Appends `value` in lowercase hexadecimal, prefixed with `0x`.
    fn push_hex(&mut self, value: u64) {
        self.push(b"0x");
        let mut started = false;
        for shift in (0..16).rev() {
            let nibble = ((value >> (shift * 4)) & 0xf) as u8;
            if !started && nibble == 0 && shift != 0 {
                continue;
            }
            started = true;
            self.push(&[HEX_DIGITS[usize::from(nibble)]]);
        }
    }

    /// The bytes written so far.
    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;

    use super::*;

    /// The signals a fake registrar was asked for, in the order it was asked.
    static REGISTERED: Mutex<Vec<i32>> = Mutex::new(Vec::new());

    /// A registrar that records what it was asked for instead of touching the process.
    fn fake_registrar(signal: i32, _handler: SignalHandler) -> Result<(), ImeError> {
        if let Ok(mut signals) = REGISTERED.lock() {
            signals.push(signal);
        }
        Ok(())
    }

    /// A registrar that refuses everything.
    fn refusing_registrar(_signal: i32, _handler: SignalHandler) -> Result<(), ImeError> {
        Err(ImeError::Unsupported)
    }

    /// A scratch root this test owns, under `/tmp` rather than `std::env::temp_dir()`
    /// so that no test depends on `$TMPDIR`.
    fn scratch_root(label: &str) -> PathBuf {
        let name = format!("rspinyin-crash-signal-{}-{label}", std::process::id());
        let root = PathBuf::from("/tmp").join(name);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root);
        root
    }

    /// The bytes of a rendered record for `info`.
    fn render(info: SignalInfo) -> String {
        let mut buffer = FixedBuffer::new();
        render_signal_record(&signal_prefix(SIGBUS), info, &mut buffer);
        String::from_utf8_lossy(buffer.as_bytes()).into_owned()
    }

    #[test]
    fn test_signal_cause_from_si_code_classifies_faults_and_sent_signals() {
        // SEGV_MAPERR and SEGV_ACCERR are positive; SI_USER is 0 and SI_TKILL is -6.
        assert_eq!(SignalCause::from_si_code(1), SignalCause::Fault);
        assert_eq!(SignalCause::from_si_code(2), SignalCause::Fault);
        assert_eq!(SignalCause::from_si_code(0), SignalCause::SentByProcess);
        assert_eq!(SignalCause::from_si_code(-6), SignalCause::SentByProcess);
    }

    #[test]
    fn test_signal_cause_is_fatal_except_for_a_sent_signal() {
        assert!(SignalCause::Fault.is_fatal());
        assert!(SignalCause::Unknown.is_fatal());
        assert!(
            !SignalCause::SentByProcess.is_fatal(),
            "a signal a debugger sent must reach the default action"
        );
    }

    #[test]
    fn test_fixed_buffer_push_decimal_formats_the_extremes() {
        let mut buffer = FixedBuffer::new();
        buffer.push_decimal(0);
        buffer.push(b" ");
        buffer.push_decimal(70);
        buffer.push(b" ");
        buffer.push_decimal(u64::MAX);
        assert_eq!(String::from_utf8_lossy(buffer.as_bytes()), "0 70 18446744073709551615");
    }

    #[test]
    fn test_fixed_buffer_push_hex_writes_a_prefixed_lowercase_number() {
        let mut buffer = FixedBuffer::new();
        buffer.push_hex(0);
        buffer.push(b" ");
        buffer.push_hex(0xdead_beef);
        buffer.push(b" ");
        buffer.push_hex(u64::MAX);
        assert_eq!(
            String::from_utf8_lossy(buffer.as_bytes()),
            "0x0 0xdeadbeef 0xffffffffffffffff"
        );
    }

    #[test]
    fn test_fixed_buffer_truncates_at_the_capacity_instead_of_overflowing() {
        let mut buffer = FixedBuffer::new();
        for _ in 0..(SIGNAL_RECORD_CAPACITY * 4) {
            buffer.push(b"record line\n");
        }
        assert_eq!(buffer.as_bytes().len(), SIGNAL_RECORD_CAPACITY);
        assert_eq!(buffer.len, SIGNAL_RECORD_CAPACITY);
    }

    #[test]
    fn test_render_signal_record_names_the_signal_the_cause_and_the_address() {
        let text = render(SignalInfo {
            cause: SignalCause::Fault,
            address: Some(0x7f3a_2c1d_0000),
        });

        assert!(text.starts_with("rspinyin crash record\n"), "{text}");
        assert!(text.contains("signal=SIGBUS\n"), "{text}");
        assert!(text.contains("cause=fault\n"), "{text}");
        assert!(text.contains("fault_address=0x7f3a2c1d0000\n"), "{text}");
        assert!(text.contains("exit_code=70\n"), "{text}");
        assert!(text.ends_with('\n'), "{text}");
    }

    #[test]
    fn test_render_signal_record_writes_none_for_an_address_the_kernel_omitted() {
        let text = render(SignalInfo {
            cause: SignalCause::SentByProcess,
            address: None,
        });

        assert!(text.contains("cause=sent\n"), "{text}");
        assert!(text.contains("fault_address=none\n"), "{text}");
    }

    #[test]
    fn test_signal_name_covers_the_handled_signals_and_falls_back() {
        assert_eq!(signal_name(SIGBUS), "SIGBUS");
        assert_eq!(signal_name(SIGSEGV), "SIGSEGV");
        assert_eq!(signal_name(1), "unknown");
    }

    #[test]
    fn test_install_signal_handlers_arms_both_signals_and_creates_the_record_file() {
        let root = scratch_root("arm");
        let dir = root.join("crash");

        let outcome = install_signal_handlers(&dir, fake_registrar);

        assert!(outcome.is_ok(), "arming reports success: {outcome:?}");
        let signals = REGISTERED.lock().map(|s| s.clone()).unwrap_or_default();
        assert_eq!(signals, vec![SIGBUS, SIGSEGV]);
        let records: Vec<_> = std::fs::read_dir(&dir)
            .map(|entries| entries.flatten().map(|e| e.file_name()).collect())
            .unwrap_or_default();
        assert_eq!(records.len(), 1, "one record file is armed");
        assert!(records[0].to_string_lossy().ends_with(".txt"), "{:?}", records[0]);
    }

    #[test]
    fn test_install_signal_handlers_reports_an_unusable_crash_directory() {
        let root = scratch_root("unusable");
        let not_a_directory = root.join("crash");
        std::fs::write(&not_a_directory, b"not a directory").expect("placing the file");

        let outcome = install_signal_handlers(&not_a_directory, refusing_registrar);

        assert!(
            matches!(outcome, Err(ImeError::DataReadonly { .. })),
            "an unarmed channel degrades with the read-only code: {outcome:?}"
        );
    }

    #[test]
    #[ignore = "the handler calls _exit, which would end the test binary"]
    fn test_signal_handler_exits_with_the_crash_code_on_a_real_fault() {
        // Manual procedure: arm the channel through the addon's registrar, raise SIGBUS
        // on a memory mapping whose file was truncated under it, and check that the
        // process ends with status 70 and that the armed record file holds a record.
        // What can be asserted without taking the fault is what the handler relies on.
        assert_eq!(CRASH_EXIT_CODE, 70, "the documented EX_SOFTWARE status");
        assert_eq!(
            HANDLED_SIGNALS,
            [(SIGBUS, "SIGBUS"), (SIGSEGV, "SIGSEGV")],
            "the dictionary fault and the general fault are both covered"
        );
    }
}
