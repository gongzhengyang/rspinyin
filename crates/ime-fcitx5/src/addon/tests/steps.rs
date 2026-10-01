//! The steps that were integration points: what each one now does with a real file.
//!
//! Every test here drives a step directly rather than through `on_addon_init`, because what
//! is under test is the step's own behaviour: the diagnostics sink, the layout, the
//! configuration store, the dictionary and the session host. The two exceptions are the
//! tests that go through the process-wide slots, which is the only way to reach them.
//!
//! Nothing here reads the process environment or a real XDG directory: the layout is built
//! from [`BaseDirs`] pointing at a scratch directory, the configuration document is written
//! into that directory, and the dictionary is either a file the test wrote or one that is
//! deliberately absent. `nextest` gives a test a process of its own, which is what the
//! process-wide slots — the layout, the store, the sources, the session host — assume.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use ime_config::Config;
use ime_core::privacy::DefaultPolicy;
use ime_core::viterbi::Decoder;
use ime_diag::crash;
use ime_diag::crash::context::CrashContextKey;
use ime_diag::crash::record::{CRASH_DIR_MODE, CRASH_FILE_MODE};
use ime_diag::probe::{MemorySnapshot, ProbeSnapshot, SNAPSHOT_FILE_NAME};
use ime_dict::paths::{BaseDirs, MAX_PATH_BYTES, READONLY_CODE};
use ime_types::{CandidateSource, DecodeRequest, ImeError, UiCommand};

use crate::engine::host::Host;
use crate::engine::router::RoutingConfig;
use crate::ffi::FcitxKeyEvent;
use crate::privacy_impl::{AppBlacklist, ContextPrivacy};
use crate::session_host;

use super::{ROUTING_DOCUMENT, scratch_dir};

use super::super::config::{CONFIG, ROUTING, init_key_bindings, load_config_at, with_config_store};
use super::super::diagnostics::{
    assemble_crash_forensics_in, close_diagnostics, crash_context, init_diagnostics_in,
    is_installed,
};
use super::super::layout::{layout, prepare_data_dirs_in};
use super::super::probes;
use super::super::session::{
    SessionSources, install_session_host, load_lexicon, session_env, sources_for,
};
use super::super::user_store::take_user_store;
use super::super::{lock, on_config_reload, routing_config};

/// `FcitxKey_n`.
const KEY_N: u32 = 0x006e;

/// `FcitxKey_space`, which commits the highlighted candidate.
const KEY_SPACE: u32 = 0x0020;

/// The input context every session-host test uses.
const IC: u64 = 1;

/// The base directories of a test's own, under `dir`.
///
/// Injected rather than read from the environment, which is what makes the layout a test
/// builds the layout it asserts on.
fn base_dirs(dir: &Path) -> BaseDirs {
    BaseDirs {
        config_home: dir.join("config"),
        data_home: dir.join("data"),
    }
}

/// A key press of `sym` with no modifier held.
fn press(sym: u32) -> FcitxKeyEvent {
    FcitxKeyEvent {
        sym,
        state: 0,
        is_release: false,
        time_ms: 7,
    }
}

/// The privacy state a fresh installation has.
fn privacy() -> ContextPrivacy {
    ContextPrivacy::new(Box::new(DefaultPolicy::default()), AppBlacklist::default())
}

/// A host boundary that remembers what it was asked to commit.
#[derive(Debug, Default)]
struct RecordingHost {
    /// Every text handed to [`Host::commit`], in order.
    commits: Vec<String>,
    /// Whether the context is enabled, as the host reports it.
    enabled: bool,
}

impl Host for RecordingHost {
    fn commit(&mut self, _ic: u64, text: &str) {
        self.commits.push(text.to_owned());
    }

    fn set_preedit(&mut self, _ic: u64, _text: &str, _caret: u32) {}

    fn clear_preedit(&mut self, _ic: u64) {}

    fn post_ui(&mut self, _ic: u64, _command: UiCommand) {}

    fn toggle_enabled(&mut self, _ic: u64) -> bool {
        self.enabled = !self.enabled;
        self.enabled
    }

    fn diagnose(&mut self, _ic: u64, _err: &ImeError) {}
}

/// A document that switches the probes off, so that the switch is observable.
const PROBES_OFF_DOCUMENT: &str = r#"
[diagnostics]
probes = false
"#;

/// Writes `document` as `config.toml` in `dir` and answers its path.
fn config_document(dir: &Path, document: &str) -> PathBuf {
    let path = dir.join("config.toml");
    fs::write(&path, document).expect("writing the fixture document");
    path
}

/// The sources a second install attempt is handed, leaked for the process.
///
/// Leaked on purpose, like every source a session host is built from: the host lives in a
/// process-wide slot and outlives every caller.
fn leaked_sources() -> &'static SessionSources {
    Box::leak(Box::new(sources_for(None)))
}

// ── the diagnostics layer ──────────────────────────────────────────────────────────

#[test]
fn test_init_diagnostics_in_writes_the_log_where_it_was_told_to() {
    let dir = scratch_dir("diagnostics");
    init_diagnostics_in(Some(dir.as_path())).expect("the first install in a process succeeds");

    assert!(is_installed(), "the handle the unload closes is recorded");
    assert!(
        dir.join(ime_diag::log::LOG_FILE_NAME).is_file(),
        "the sink is the log file of the directory the step was given"
    );

    close_diagnostics();
    assert!(!is_installed(), "the unload releases the handle");
    close_diagnostics();
    assert!(
        !is_installed(),
        "a second close finds nothing, which is what makes the destructor safe to call twice"
    );
}

#[test]
fn test_init_diagnostics_in_without_a_directory_leaves_the_layer_off() {
    // The environment named no base directory, so there is nowhere to write: the step
    // succeeds without installing a sink, because the crash channel still reports.
    assert!(
        init_diagnostics_in(None).is_ok(),
        "a missing directory is a degradation, never a declined addon"
    );
}

// ── the crash forensics ────────────────────────────────────────────────────────────

#[test]
fn test_assemble_crash_forensics_in_arms_the_channel_over_the_given_directory() {
    let dir = scratch_dir("crash-forensics");
    let crash_dir = dir.join("crash");

    assemble_crash_forensics_in(Some(&crash_dir));

    assert_eq!(
        crash::crash_directory(),
        Some(crash_dir.as_path()),
        "the records the channel writes land in the layout's crash directory"
    );
    let mode = std::fs::metadata(&crash_dir)
        .expect("the arming prepared the directory")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, CRASH_DIR_MODE, "the directory is owner-only");
    // The fault-signal channel owns its record file from the moment it is armed, because
    // a handler cannot build a path; its presence here is the observable half of the
    // arming, and its mode is the privacy baseline. The armed file is the empty one: the
    // panic hook this arming installed also writes records here when anything else in the
    // process panics, and those are never empty.
    let armed: Vec<_> = std::fs::read_dir(&crash_dir)
        .expect("listing the crash directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| std::fs::metadata(path).map(|meta| meta.len() == 0).unwrap_or(false))
        .collect();
    assert_eq!(armed.len(), 1, "the signal channel armed one record");
    let file_mode = std::fs::metadata(&armed[0])
        .expect("the armed record's metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, CRASH_FILE_MODE, "the record is owner-only");
}

#[test]
fn test_assemble_crash_forensics_in_survives_a_missing_directory() {
    // Forensics must never be the reason the addon declines: an environment that names no
    // data directory leaves the channel unarmed, the condition is reported, and the step
    // still succeeds.
    assert!(
        std::panic::catch_unwind(|| assemble_crash_forensics_in(None)).is_ok(),
        "the arming fails soft when no directory is named"
    );
}

#[test]
fn test_crash_context_names_readonly_mode_when_the_layout_degraded() {
    // The provider reads atomic slots alone, because a panic may be caught on a thread
    // that already holds the lock a richer provider would want. The one fact it reads
    // today is the paths layer's process-wide read-only flag, and driving that flag
    // through a layout whose paths cannot exist is what makes the branch observable.
    let overlong = "a".repeat(MAX_PATH_BYTES * 2);
    let bases = BaseDirs::from_lookup(|name| {
        Some(std::ffi::OsString::from(match name {
            "XDG_DATA_HOME" | "XDG_CONFIG_HOME" => overlong.clone(),
            _ => String::new(),
        }))
    })
    .expect("the lookup answers every name");
    assert!(
        ime_dict::paths::Paths::from_bases(&bases).is_err(),
        "a layout of over-long paths is refused"
    );

    let context = crash_context();

    assert_eq!(
        context.get(CrashContextKey::SessionState),
        Some("readonly"),
        "the record says the session ran with writes disabled"
    );
}

// ── the data directories ───────────────────────────────────────────────────────────

#[test]
fn test_prepare_data_dirs_in_installs_the_layout_and_creates_its_directories() {
    let dir = scratch_dir("layout");
    prepare_data_dirs_in(&base_dirs(&dir)).expect("the step never fails");

    let paths = layout().expect("the layout is recorded for the steps that read a file");
    assert!(paths.data_dir.is_dir());
    assert!(paths.config_dir.is_dir());
    assert!(paths.log_dir.is_dir());
    assert!(paths.crash_dir.is_dir());
    assert!(
        !paths.is_readonly(),
        "a writable layout is not the read-only degradation"
    );
}

#[test]
fn test_prepare_data_dirs_in_degrades_to_readonly_when_the_directory_is_blocked() {
    // A file where the data directory belongs: nothing can be created through it. `ASM-15`
    // asks for a read-only plugin rather than no plugin, and the layout is still recorded,
    // because it is what names the dictionary.
    let dir = scratch_dir("layout-blocked");
    let bases = base_dirs(&dir);
    fs::create_dir_all(&bases.data_home).expect("the base directory");
    fs::write(bases.data_home.join("rspinyin"), b"not a directory").expect("taking the name");

    assert!(
        prepare_data_dirs_in(&bases).is_ok(),
        "a blocked directory is never fatal"
    );

    let paths = layout().expect("the layout is recorded even when it is read-only");
    assert!(
        paths.is_readonly(),
        "the degradation is reported by the layout"
    );
    assert!(
        paths
            .notices()
            .iter()
            .any(|notice| notice.code == READONLY_CODE),
        "and it is reported under the read-only code: {:?}",
        paths.notices()
    );
}

// ── the configuration ──────────────────────────────────────────────────────────────

#[test]
fn test_load_config_at_installs_the_store_and_the_projection_follows_it() {
    let dir = scratch_dir("config-load");
    let path = config_document(&dir, ROUTING_DOCUMENT);

    load_config_at(Some(path.as_path())).expect("the step never fails");

    assert_eq!(
        with_config_store(|store| store.current().ui.max_per_row),
        Some(7),
        "the document is the configuration in force, not the built-in defaults"
    );

    init_key_bindings().expect("the step never fails");
    assert_eq!(
        routing_config().scheme_hint,
        Some("小鹤"),
        "the projection reads the store the config step installed"
    );
}

#[test]
fn test_load_config_at_keeps_the_defaults_and_keeps_the_damaged_document() {
    let dir = scratch_dir("config-damaged");
    let path = config_document(&dir, "this is not TOML at all\n");

    assert!(
        load_config_at(Some(path.as_path())).is_ok(),
        "a document that cannot be parsed must not decline the addon"
    );

    assert_eq!(
        with_config_store(|store| store.current().engine.max_raw_len),
        Some(Config::default().engine.max_raw_len),
        "the built-in defaults are what a damaged document answers with"
    );
    let quarantined = fs::read_dir(&dir)
        .expect("reading the directory")
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().contains(".bad."));
    assert!(
        quarantined,
        "the damaged document is kept beside the new one, never deleted"
    );
}

#[test]
fn test_load_config_at_without_a_path_leaves_the_configuration_in_force() {
    // No configuration directory to read from. The step reports it and succeeds, and it
    // installs no store: what is in force is what was there before, which for a fresh
    // process is the built-in defaults the projection falls back to.
    let _ = lock(&CONFIG).take();

    assert!(
        load_config_at(None).is_ok(),
        "an environment with no configuration directory is not a failure"
    );
    assert!(
        with_config_store(|store| store.current().engine.max_raw_len).is_none(),
        "and it installs no store"
    );
}

#[test]
fn test_load_config_at_applies_the_probes_switch() {
    let dir = scratch_dir("config-probes");
    let path = config_document(&dir, PROBES_OFF_DOCUMENT);

    load_config_at(Some(path.as_path())).expect("the step never fails");

    assert!(
        !probes::is_enabled(),
        "`[diagnostics] probes` is a runtime switch and reaches the probes the process holds"
    );
}

// ── the dictionary and the session host ────────────────────────────────────────────

#[test]
fn test_sources_for_without_a_dictionary_answers_a_pass_through_candidate() {
    // The degradation this card exists to make honest: with no dictionary at all, the
    // decoder answers the input unchanged rather than nothing, so the user commits what
    // they typed instead of losing the keystroke.
    let dir = scratch_dir("lexicon-missing");
    let sources = sources_for(Some(dir.join("base.dict").as_path()));

    let decoded = Decoder::default().decode(
        &DecodeRequest::new("ni'hao"),
        sources.lexicon(),
        sources.user_freq(),
        sources.lm(),
    );

    let candidate = decoded
        .candidates
        .first()
        .expect("a decode is never answered with an empty list");
    assert_eq!(
        candidate.text, "ni'hao",
        "the text is handed back unchanged"
    );
    assert_eq!(candidate.source, CandidateSource::Passthrough);
    assert!(decoded.degraded, "and the answer says it is degraded");
}

#[test]
fn test_sources_for_with_a_damaged_dictionary_degrades_the_same_way() {
    // A file that is not a container at all: the load fails, and the plugin runs on the
    // empty dictionary rather than on a half-read one.
    let dir = scratch_dir("lexicon-damaged");
    let path = dir.join("base.dict");
    fs::write(&path, b"not a dictionary").expect("damaging the dictionary");

    let sources = sources_for(Some(path.as_path()));

    let decoded = Decoder::default().decode(
        &DecodeRequest::new("ni"),
        sources.lexicon(),
        sources.user_freq(),
        sources.lm(),
    );
    assert_eq!(
        decoded.candidates.first().map(|entry| entry.text.as_str()),
        Some("ni"),
        "a damaged dictionary is the same degradation as a missing one"
    );
    assert!(decoded.degraded);
}

#[test]
fn test_sources_for_with_no_data_directory_degrades_the_same_way() {
    let sources = sources_for(None);

    let decoded = Decoder::default().decode(
        &DecodeRequest::new("ni"),
        sources.lexicon(),
        sources.user_freq(),
        sources.lm(),
    );
    assert_eq!(
        decoded.candidates.first().map(|entry| entry.text.as_str()),
        Some("ni")
    );
    assert!(decoded.degraded);
}

#[test]
fn test_session_host_over_the_empty_dictionary_commits_what_was_typed() {
    // The end-to-end shape of the degradation: a key is claimed, a composition starts, and
    // space commits the text the user typed. Nothing is swallowed, which is the property the
    // pass-through path exists for.
    let _ = session_host::shutdown();
    let dir = scratch_dir("session-host-degraded");
    let sources: &'static SessionSources =
        Box::leak(Box::new(sources_for(Some(dir.join("base.dict").as_path()))));

    assert!(
        session_host::install(session_env(sources), privacy(), RoutingConfig::default()),
        "the slot was empty, so this install wins"
    );

    let mut host = RecordingHost::default();
    assert!(session_host::activate(IC), "the context is activated");
    assert!(
        session_host::key_event(IC, &press(KEY_N), &mut host),
        "a letter that starts a composition is the plugin's key"
    );
    assert!(
        session_host::key_event(IC, &press(KEY_SPACE), &mut host),
        "space commits the highlighted candidate"
    );
    assert_eq!(
        host.commits,
        [String::from("n")],
        "with no dictionary the candidate is the input itself"
    );
    let _ = session_host::shutdown();
}

#[test]
fn test_load_lexicon_and_install_session_host_wire_the_process_host() {
    // The two steps in order, over a layout of this test's own. The dictionary is absent, so
    // what is installed is the degraded environment — which is still a host, and a host is
    // what the plugin needs before the first key arrives.
    let _ = session_host::shutdown();
    let _ = take_user_store();
    let dir = scratch_dir("session-host-steps");
    prepare_data_dirs_in(&base_dirs(&dir)).expect("the step never fails");

    load_lexicon().expect("the step never fails");
    install_session_host().expect("the step never fails");

    assert!(
        session_host::is_installed(),
        "the sequence leaves a router behind, which is what produces candidates"
    );
    install_session_host().expect("the step is idempotent");
    assert!(
        !session_host::install(
            session_env(leaked_sources()),
            privacy(),
            RoutingConfig::default()
        ),
        "a second install is refused rather than replacing the router a key may be in"
    );
    let _ = session_host::shutdown();
}

// ── the snapshot the budget gate reads ─────────────────────────────────────────────

#[test]
fn test_write_snapshot_writes_the_latencies_and_the_memory_section() {
    let dir = scratch_dir("snapshot");
    let bases = base_dirs(&dir);
    prepare_data_dirs_in(&bases).expect("the step never fails");
    probes::mark_dictionary_baseline();

    probes::write_snapshot().expect("the snapshot is writable");

    let path = bases.data_home.join("rspinyin").join(SNAPSHOT_FILE_NAME);
    let text = fs::read_to_string(&path).expect("the snapshot was written");
    ProbeSnapshot::parse(&text).expect("the latency section parses");
    let memory = MemorySnapshot::parse(&text).expect("the memory section parses");
    assert!(
        memory.rss_kib.is_some(),
        "the reading is taken when the snapshot is written"
    );
    assert!(
        memory.baseline_rss_kib.is_some(),
        "the plugin's baseline is taken when the probes are created"
    );
    assert!(
        memory.dictionary_baseline_dirty_kib.is_some(),
        "and the dictionary's is taken with the mapping in place, which is what lets \
         `budget --memory` judge that window instead of reporting it unmeasured"
    );

    let mode = fs::metadata(&path)
        .expect("the snapshot exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "a snapshot is written private");
}

#[test]
fn test_write_snapshot_reports_a_file_it_cannot_create() {
    let dir = scratch_dir("snapshot-blocked");
    let bases = base_dirs(&dir);
    prepare_data_dirs_in(&bases).expect("the step never fails");
    // The data directory goes away between the layout being recorded and the snapshot being
    // asked for: a user who removed it, or a scratch directory that was cleaned up. The
    // write reports it rather than panicking on the way out of a signal-driven path.
    fs::remove_dir_all(&bases.data_home).expect("removing the data directory");

    assert!(
        probes::write_snapshot().is_err(),
        "a snapshot that cannot be written is reported to its caller"
    );
}

#[test]
fn test_on_config_reload_without_a_store_reports_and_keeps_the_table() {
    // The reload path the ABI calls once it carries the slot. With no store there is nothing
    // to re-read, and the table in force is kept rather than replaced by a default.
    let _ = lock(&CONFIG).take();
    let _ = lock(&ROUTING).take();
    let before = routing_config();

    assert_eq!(on_config_reload(), before);
}
