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
use ime_dict::format::writer::DictWriter;
use ime_dict::format::{DictEntry, PROB_Q12_MAX, SectionKind, hash_word, pack_fst_value};
use ime_dict::paths::{BaseDirs, MAX_PATH_BYTES, READONLY_CODE};
use ime_types::{CandidateSource, DecodeRequest, ImeError, Lexicon as _, UiCommand};

use crate::engine::host::Host;
use crate::engine::router::RoutingConfig;
use crate::ffi::FcitxKeyEvent;
use crate::privacy_impl::{AppBlacklist, ContextPrivacy, ContextReport};
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
    MISSING_DICTIONARY, MISSING_LANGUAGE_MODEL, SessionSources, install_session_host, lexicon_slot,
    load_lexicon, session_env, session_sources, sources_for, user_lexicon_handle,
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
    /// Every frame the window was sent, in order, for the tests that read the candidate
    /// list the session produced.
    frames: Vec<ime_types::UiFrame>,
    /// Whether the context is enabled, as the host reports it.
    enabled: bool,
}

impl RecordingHost {
    /// The top candidate of the last frame, for the tests that read the order back.
    fn top_candidate(&self) -> Option<String> {
        Some(self.frames.last()?.candidates.first()?.text.clone())
    }
}

impl Host for RecordingHost {
    fn commit(&mut self, _ic: u64, text: &str) {
        self.commits.push(text.to_owned());
    }

    fn set_preedit(&mut self, _ic: u64, _text: &str, _caret: u32) {}

    fn clear_preedit(&mut self, _ic: u64) {}

    fn post_ui(&mut self, _ic: u64, command: UiCommand) {
        if let UiCommand::Frame(frame) = command {
            self.frames.push(*frame);
        }
    }

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
        .filter(|path| {
            std::fs::metadata(path)
                .map(|meta| meta.len() == 0)
                .unwrap_or(false)
        })
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
    // Absolute as well as over-long: a relative `XDG_*` value would be ignored per the
    // XDG spec, which would leave the layout the fallback's and never exercise the
    // over-long refusal the assertion below is about.
    let overlong = format!("/{}", "a".repeat(MAX_PATH_BYTES * 2));
    let bases = BaseDirs::from_lookup(|name| {
        Some(std::ffi::OsString::from(match name {
            "XDG_DATA_HOME" | "XDG_CONFIG_HOME" => overlong.clone(),
            // `$HOME` stays answerable: the base layer only falls back to it when an
            // `XDG_*` variable is unusable, and an empty answer here would refuse the
            // bases before the over-long paths ever reach the layout that must reject
            // them.
            "HOME" => std::env::var("HOME")
                .ok()
                .filter(|value| std::path::Path::new(value).is_absolute())
                .unwrap_or_else(|| "/tmp".to_string()),
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
        &MISSING_DICTIONARY,
        sources.user_freq(),
        &MISSING_LANGUAGE_MODEL,
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
        &MISSING_DICTIONARY,
        sources.user_freq(),
        &MISSING_LANGUAGE_MODEL,
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
        &MISSING_DICTIONARY,
        sources.user_freq(),
        &MISSING_LANGUAGE_MODEL,
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
        session_host::install(
            session_env(sources),
            privacy(),
            RoutingConfig::default(),
            session_host::RouterLexicons::default(),
        ),
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
            RoutingConfig::default(),
            session_host::RouterLexicons::default(),
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

// ── the staged-dictionary reload path ──────────────────────────────────────────────
//
// The slot these tests install is a process-wide `OnceLock` with no reset, so they lean on
// the mandated one-process-per-test runner like the session-host slot tests do; a plain
// single-process `cargo test` run would make the two swapping tests race for it. The
// rejection tests are written to hold whichever container is live, so only the success
// test cares.

/// Encodes a container image: each `(key, text)` resolves as a one-syllable word, scored
/// by the unigram table at the strongest probability. The sections are the public
/// format's own encoders, so the bytes are what a real compile produces.
fn container_image(words: &[(&str, &str)]) -> Vec<u8> {
    container_image_at(words, PROB_Q12_MAX)
}

/// Encodes a container image whose every word scores `prob_q12` in the unigram table.
///
/// The language model the decoder ranks with reads this table, so the value decides how
/// hard the words are to beat: the maximum is the score of the dictionary's most frequent
/// word, and a realistic entry sits far below it.
fn container_image_at(words: &[(&str, &str)], prob_q12: u16) -> Vec<u8> {
    // Word ids are positions in hash order, which is the order the word list stores and
    // the FST keys name.
    let mut hashed: Vec<(u32, &str)> = words
        .iter()
        .map(|(_, text)| (hash_word(text), *text))
        .collect();
    hashed.sort_by_key(|(hash, text)| (*hash, *text));
    let id_of: std::collections::BTreeMap<&str, u32> = hashed
        .iter()
        .enumerate()
        .map(|(id, (_, text))| (*text, u32::try_from(id).expect("small fixture")))
        .collect();

    let mut strpool = Vec::new();
    let mut entries = Vec::new();
    let mut unigram = Vec::new();
    let mut wordlist = Vec::new();
    for (id, (_, text)) in hashed.iter().enumerate() {
        let id = u32::try_from(id).expect("small fixture");
        let offset = u32::try_from(strpool.len()).expect("small fixture");
        strpool.extend_from_slice(text.as_bytes());
        let entry = DictEntry::new(
            offset,
            u16::try_from(text.len()).expect("small fixture"),
            1,
            0,
            1,
        );
        entries.extend_from_slice(&entry.encode());
        unigram.extend_from_slice(&hash_word(text).to_le_bytes());
        unigram.extend_from_slice(&prob_q12.to_le_bytes());
        unigram.extend_from_slice(&0u16.to_le_bytes());
        wordlist.extend_from_slice(&id.to_le_bytes());
    }

    // The FST builder takes keys in ascending byte order, which the fixture's is not, so
    // the keys are sorted before it sees them.
    let mut keys: Vec<(&str, u32)> = words
        .iter()
        .map(|(key, text)| (*key, id_of[text]))
        .collect();
    keys.sort_by_key(|(key, _)| key.as_bytes().to_vec());
    let mut fst_builder = fst::MapBuilder::memory();
    for (key, id) in keys {
        let packed = pack_fst_value(u64::from(id), 1).expect("packing a word");
        fst_builder.insert(key.as_bytes(), packed).expect("one key");
    }
    let fst = fst_builder.into_inner().expect("finishing the FST");

    let mut writer = DictWriter::new();
    for (kind, payload) in [
        (SectionKind::Fst, fst),
        (SectionKind::Entries, entries),
        (SectionKind::StrPool, strpool),
        (SectionKind::Unigram, unigram),
        (SectionKind::WordList, wordlist),
    ] {
        writer.add_section(kind, payload).expect("section added");
    }
    writer.encode().expect("encoding the container")
}

/// Loads the configuration document `text` at a scratch path, as the `config` step would.
fn load_config_document(dir: &Path, text: &str) {
    let path = config_document(dir, text);
    load_config_at(Some(path.as_path())).expect("the step never fails");
}

/// The staged-path half of a configuration document, spelled once.
fn staged_document(staged: &Path) -> String {
    format!("[data]\ndict_staged_path = \"{}\"\n", staged.display())
}

#[test]
fn test_apply_staged_dictionary_swaps_a_valid_container_and_clears_the_key() {
    let dir = scratch_dir("staged-swap");
    let live = dir.join("live.dict");
    let staged = dir.join("staged.dict");
    fs::write(&live, container_image(&[("ni", "你")])).expect("writing the live container");
    fs::write(&staged, container_image(&[("de", "地")])).expect("writing the staged container");

    // The process starts on the live container, which is what puts the slot in place.
    sources_for(Some(live.as_path()));
    let slot = lexicon_slot().expect("a start that mapped a container installed the slot");
    assert!(
        slot.load()
            .lexicon()
            .lookup("ni")
            .expect("looking up")
            .count()
            > 0,
        "the live container is the one serving"
    );

    // The user's edit: the document in force names the staged file, and the reload adopts
    // it — which is the trigger the swap hangs on.
    load_config_document(&dir, "[data]\ndict_staged_path = \"\"\n");
    let edited = staged_document(&staged);
    fs::write(dir.join("config.toml"), &edited).expect("saving the edit");

    on_config_reload();

    assert_eq!(
        slot.load().path(),
        staged,
        "the slot serves the staged container"
    );
    assert!(
        slot.load()
            .lexicon()
            .lookup("de")
            .expect("looking up")
            .count()
            > 0,
        "a decode through the slot reads the new container"
    );
    assert!(
        slot.load()
            .lexicon()
            .lookup("ni")
            .expect("looking up")
            .count()
            == 0,
        "and no longer the old one"
    );
    // The served swap is written back: the key is empty in the file the user saved, so the
    // next reload attempts nothing. The write-back renders the whole `[data]` table, which
    // is its documented shape, so the assertion is on the value and on what a re-read
    // parses to, not on the bytes.
    let written = fs::read_to_string(dir.join("config.toml")).expect("reading the file back");
    assert!(
        written.contains("dict_staged_path = \"\""),
        "the key is cleared: {written}"
    );
    let (reparsed, warnings) = Config::from_document(&written).expect("the document parses");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(reparsed.data.dict_staged_path.is_empty());
}

#[test]
fn test_apply_staged_dictionary_rejects_a_damaged_file_and_keeps_the_key() {
    let dir = scratch_dir("staged-reject");
    let live = dir.join("live.dict");
    let staged = dir.join("staged.dict");
    fs::write(&live, container_image(&[("ni", "你")])).expect("writing the live container");
    // The candidate is not a container at all, which the validation refuses.
    fs::write(&staged, b"not a dictionary").expect("writing the damaged file");

    sources_for(Some(live.as_path()));
    let slot = lexicon_slot().expect("a start that mapped a container installed the slot");
    let serving = slot.load().path().to_path_buf();

    load_config_document(&dir, "[data]\ndict_staged_path = \"\"\n");
    fs::write(dir.join("config.toml"), staged_document(&staged)).expect("saving the edit");

    on_config_reload();

    // The live dictionary keeps serving, unchanged.
    assert_eq!(
        slot.load().path(),
        serving,
        "the rejected swap changes nothing"
    );
    // The damaged bytes were moved aside, so they cannot sit in the staged path.
    assert!(!staged.exists(), "the staged path is empty");
    let quarantined: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("reading the scratch directory")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".corrupt."))
        })
        .collect();
    assert_eq!(
        quarantined.len(),
        1,
        "one quarantined sibling: {quarantined:?}"
    );
    // The key stays, for the user to correct: the file still names the staged path.
    let written = fs::read_to_string(dir.join("config.toml")).expect("reading the file back");
    assert_eq!(written, staged_document(&staged), "{written}");
}

#[test]
fn test_apply_staged_dictionary_without_a_container_rejects_and_keeps_the_key() {
    let dir = scratch_dir("staged-no-slot");
    let staged = dir.join("not-there.dict");

    // No container mapped in this process: the reload reaches a swap with no slot to
    // serve it, which is refused with the same code and the same kept key.
    load_config_document(&dir, "[data]\ndict_staged_path = \"\"\n");
    fs::write(dir.join("config.toml"), staged_document(&staged)).expect("saving the edit");

    on_config_reload();

    let written = fs::read_to_string(dir.join("config.toml")).expect("reading the file back");
    assert_eq!(
        written,
        staged_document(&staged),
        "the key stays: {written}"
    );
    assert!(
        !staged.exists(),
        "a missing candidate leaves nothing to quarantine"
    );
}

#[test]
fn test_apply_staged_dictionary_with_an_empty_path_attempts_nothing() {
    let dir = scratch_dir("staged-empty");
    // The document in force names a staged file; the edit the reload adopts clears it.
    // The empty path is the value the write-back itself leaves behind, so adopting it
    // must attempt no swap and touch nothing.
    let staged = dir.join("elsewhere.dict");
    load_config_document(&dir, &staged_document(&staged));
    fs::write(dir.join("config.toml"), "[data]\ndict_staged_path = \"\"\n")
        .expect("saving the edit");

    on_config_reload();

    let written = fs::read_to_string(dir.join("config.toml")).expect("reading the file back");
    assert_eq!(written, "[data]\ndict_staged_path = \"\"\n", "{written}");
}

#[test]
fn test_committing_a_two_character_reading_coins_it_for_the_next_decode() {
    // The walkthrough the card states, end to end: the dictionary holds the two
    // characters but no word over them, the user commits the reading they spell, and the
    // next composition of the same keys offers the pair as one word of the user's own.
    let _ = session_host::shutdown();
    let _ = take_user_store();
    let dir = scratch_dir("user-coin");
    let live = dir.join("base.dict");
    // The two characters sit at a sixteenth of the strongest probability, the spread a
    // real dictionary holds between its head words and ordinary characters. At the
    // maximum the singles outrank anything a freshly coined word can score -- the
    // language model has no entry for it -- which is the opposite of the shape the
    // walkthrough starts from.
    fs::write(
        &live,
        container_image_at(&[("zhu", "诸"), ("ji", "暨")], PROB_Q12_MAX / 16),
    )
    .expect("writing the live container");

    // The store the coin persists through, and the lexicon assembled over it: both are
    // the process slots the steps install, so the test installs them in the same order.
    let db =
        ime_dict::user_db::UserDb::open(dir.join("user.redb")).expect("opening the test store");
    super::super::user_store::install_user_store(crate::addon::user_store::UserStore {
        db: std::sync::Mutex::new(db),
        backup: ime_dict::user_db::BackupConfig::disabled(dir.clone()),
    });

    sources_for(Some(live.as_path()));
    let slot = lexicon_slot().expect("the start mapped a container");
    let lexicon = lexicon_slot()
        .and(user_lexicon_handle())
        .expect("a store was installed, so the lexicon was assembled");

    assert!(
        session_host::install(
            session_env(session_sources()),
            privacy(),
            RoutingConfig::default(),
            session_host::RouterLexicons {
                system: Some(slot),
                user: Some(lexicon),
            },
        ),
        "the slot was empty, so this install wins"
    );

    let mut host = RecordingHost::default();
    // A host that reports the context as an ordinary one: learning is fail-closed for an
    // unreported context, and the coin the walkthrough is about happens on the commit.
    assert!(
        session_host::activate_reported(
            IC,
            ContextReport::Reported {
                program: Some("e2e-coin"),
                password: false,
                sensitive: false,
                preedit: false,
            },
        ),
        "the context is activated"
    );
    for sym in [KEY_Z, KEY_H, KEY_U, KEY_J, KEY_I] {
        assert!(
            session_host::key_event(IC, &press(sym), &mut host),
            "every letter of a composition is the plugin's key"
        );
    }
    assert!(
        session_host::key_event(IC, &press(KEY_SPACE), &mut host),
        "space commits the highlighted candidate"
    );
    assert_eq!(
        host.commits,
        [String::from("诸暨")],
        "the reading commits whole"
    );

    // The commit coined the pair: the memory table holds it, under the key the input
    // spelled.
    assert!(
        lexicon.lock().knows_word("诸暨"),
        "the commit enrolled the pair into the user's lexicon"
    );

    // The next composition of the same keys offers the coined word first, labelled as
    // the user's own.
    let mut host = RecordingHost::default();
    for sym in [KEY_Z, KEY_H, KEY_U, KEY_J, KEY_I] {
        assert!(session_host::key_event(IC, &press(sym), &mut host));
    }
    assert_eq!(
        host.top_candidate().as_deref(),
        Some("诸暨"),
        "the coined word is the first candidate of the next composition"
    );
    let _ = session_host::shutdown();
}

/// `FcitxKey_z`, the first letter of the walkthrough's reading.
const KEY_Z: u32 = 0x007a;
/// `FcitxKey_h`.
const KEY_H: u32 = 0x0068;
/// `FcitxKey_u`.
const KEY_U: u32 = 0x0075;
/// `FcitxKey_j`.
const KEY_J: u32 = 0x006a;
/// `FcitxKey_i`.
const KEY_I: u32 = 0x0069;

#[test]
fn test_mixed_english_offers_the_english_word_for_an_input_no_pinyin_cuts() {
    // The mixed-English walkthrough: the English container sits beside the dictionary
    // in the data directory, the switch is on, and `github` -- which no pinyin reading
    // cuts -- reaches the entry spelled `GitHub`, labelled as the English table's. An
    // input the exact walk cuts keeps its pinyin answer, which is the switch's whole
    // promise: English never competes with a reading that exists.
    let _ = session_host::shutdown();
    let dir = scratch_dir("mixed-english");
    let live = dir.join("base.dict");
    fs::write(&live, container_image(&[("ni'hao", "你好")])).expect("writing the live container");
    // The layout is what names the data directory, so it is what makes the English
    // container findable; the sources read it on their first assembly. The document
    // turns the switch on and nothing else: the defaults keep full pinyin and the
    // shipped key table, which is the shape the walkthrough runs in.
    prepare_data_dirs_in(&base_dirs(&dir)).expect("the step never fails");
    let data_dir = layout().expect("the step installed the layout").data_dir;
    fs::write(
        data_dir.join("eng.dict"),
        container_image_at(&[("github", "GitHub")], PROB_Q12_MAX / 2),
    )
    .expect("writing the English container");

    let path = config_document(&dir, "[engine]\nmixed_english = true\n");
    load_config_at(Some(path.as_path())).expect("the step never fails");
    init_key_bindings().expect("the step never fails");

    sources_for(Some(live.as_path()));
    let slot = lexicon_slot().expect("the start mapped a container");
    assert!(
        session_host::install(
            session_env(session_sources()),
            privacy(),
            routing_config(),
            session_host::RouterLexicons {
                system: Some(slot),
                user: None,
            },
        ),
        "the slot was empty, so this install wins"
    );

    let mut host = RecordingHost::default();
    assert!(session_host::activate(IC), "the context is activated");
    for sym in [KEY_G, KEY_I, KEY_T, KEY_H, KEY_U, KEY_B] {
        assert!(
            session_host::key_event(IC, &press(sym), &mut host),
            "every letter of `github` is the plugin's key"
        );
    }
    let frame = host.frames.last().expect("the composition posted a frame");
    let first = &frame.candidates[0];
    assert_eq!(first.text, "GitHub");
    assert_eq!(first.source, CandidateSource::English);
    assert_eq!(first.annotation.as_deref(), Some("英文"));

    // The control: an input the exact walk cuts is answered by the dictionary, and no
    // English candidate exists beside it -- the pinyin reading was never competed with.
    session_host::reset(IC, &mut host);
    for sym in [KEY_N, KEY_I, KEY_H, KEY_A, KEY_O] {
        assert!(session_host::key_event(IC, &press(sym), &mut host));
    }
    let frame = host.frames.last().expect("the composition posted a frame");
    assert_eq!(frame.candidates[0].text, "你好");
    assert_eq!(frame.candidates[0].source, CandidateSource::Dict);
    assert!(
        frame
            .candidates
            .iter()
            .all(|candidate| candidate.source != CandidateSource::English),
        "no English candidate exists for an input the exact walk cut"
    );
    let _ = session_host::shutdown();
}

#[test]
fn test_confusable_hint_names_the_correct_form_in_the_frame() {
    // The wrong-word hint walkthrough: the confusable table sits beside the dictionary
    // in the data directory, and the candidate that spells one of its wrong forms --
    // `按奈`, which the dictionary happily offers for `an'nai` -- carries the hint that
    // names the correct form in the very frame the window draws. The switch the hints
    // latch rides is `ui.show_annotation`, whose default is on, so the default
    // configuration is the on case.
    let _ = session_host::shutdown();
    let dir = scratch_dir("confusable-hint");
    let live = dir.join("base.dict");
    fs::write(&live, container_image(&[("an'nai", "按奈")])).expect("writing the live container");
    prepare_data_dirs_in(&base_dirs(&dir)).expect("the step never fails");
    let data_dir = layout().expect("the step installed the layout").data_dir;
    fs::write(data_dir.join("confusable.tsv"), "按奈\t按捺\n")
        .expect("writing the confusable table");

    // Every default is the walkthrough's own: full pinyin, annotations shown.
    let path = config_document(&dir, "");
    load_config_at(Some(path.as_path())).expect("the step never fails");
    init_key_bindings().expect("the step never fails");

    sources_for(Some(live.as_path()));
    let slot = lexicon_slot().expect("the start mapped a container");
    assert!(
        session_host::install(
            session_env(session_sources()),
            privacy(),
            routing_config(),
            session_host::RouterLexicons {
                system: Some(slot),
                user: None,
            },
        ),
        "the slot was empty, so this install wins"
    );

    let mut host = RecordingHost::default();
    assert!(session_host::activate(IC), "the context is activated");
    for sym in [KEY_A, KEY_N, KEY_N, KEY_A, KEY_I] {
        assert!(
            session_host::key_event(IC, &press(sym), &mut host),
            "every letter of `annai` is the plugin's key"
        );
    }
    let frame = host.frames.last().expect("the composition posted a frame");
    let first = &frame.candidates[0];
    assert_eq!(first.text, "按奈");
    assert_eq!(
        first.annotation.as_deref(),
        Some("字:→按捺"),
        "the wrong form names the correct one in the frame"
    );
    let _ = session_host::shutdown();
}

#[test]
fn test_v_mode_serves_the_symbol_panel_in_the_frame() {
    // The symbol-panel walkthrough: the table sits beside the dictionary in the
    // data directory, `v1` opens the digit category, and the frame the window
    // draws carries the symbols as ordinary candidates labelled with the
    // category's name. `vzhishi` -- which names no category -- is echoed the way
    // it was before the panel existed, which is the panel's whole promise: it
    // never competes with an input it does not serve.
    let _ = session_host::shutdown();
    let dir = scratch_dir("symbol-panel");
    let live = dir.join("base.dict");
    fs::write(&live, container_image(&[("zhishi", "知识")])).expect("writing the live container");
    prepare_data_dirs_in(&base_dirs(&dir)).expect("the step never fails");
    let data_dir = layout().expect("the step installed the layout").data_dir;
    fs::write(
        data_dir.join("symbols.tsv"),
        "1\t数字\t０１２３\nx\t箭头\t←↑→\n",
    )
    .expect("writing the symbol table");

    // Every default is the walkthrough's own: full pinyin, annotations shown.
    let path = config_document(&dir, "");
    load_config_at(Some(path.as_path())).expect("the step never fails");
    init_key_bindings().expect("the step never fails");

    sources_for(Some(live.as_path()));
    let slot = lexicon_slot().expect("the start mapped a container");
    assert!(
        session_host::install(
            session_env(session_sources()),
            privacy(),
            routing_config(),
            session_host::RouterLexicons {
                system: Some(slot),
                user: None,
            },
        ),
        "the slot was empty, so this install wins"
    );

    let mut host = RecordingHost::default();
    assert!(session_host::activate(IC), "the context is activated");
    for sym in [KEY_V, KEY_1] {
        assert!(
            session_host::key_event(IC, &press(sym), &mut host),
            "every key of `v1` is the plugin's key"
        );
    }
    let frame = host.frames.last().expect("the composition posted a frame");
    let texts: Vec<&str> = frame
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect();
    assert_eq!(
        texts,
        ["０", "１", "２", "３"],
        "file order is display order"
    );
    assert_eq!(
        frame.preedit.text, "v1",
        "the panel shows the keystrokes as typed"
    );
    assert_eq!(frame.candidates[0].annotation.as_deref(), Some("数字"));

    // The control: an input no category names is answered as it always was.
    session_host::reset(IC, &mut host);
    for sym in [KEY_V, KEY_Z, KEY_H, KEY_I, KEY_S, KEY_H, KEY_I] {
        assert!(session_host::key_event(IC, &press(sym), &mut host));
    }
    let frame = host.frames.last().expect("the composition posted a frame");
    assert_eq!(frame.candidates[0].text, "vzhishi");
    assert!(
        frame
            .candidates
            .iter()
            .all(|candidate| candidate.source != ime_types::CandidateSource::Symbol),
        "no panel candidate exists for an input no category names"
    );
    let _ = session_host::shutdown();
}

/// `FcitxKey_v`, the first key of the symbol-panel walkthrough.
const KEY_V: u32 = 0x0076;
/// `FcitxKey_1`, the digit that names the panel's number category.
const KEY_1: u32 = 0x0031;
/// `FcitxKey_s`, a letter of the fall-through control.
const KEY_S: u32 = 0x0073;
/// `FcitxKey_g`, the first letter of the mixed-English walkthrough.
const KEY_G: u32 = 0x0067;
/// `FcitxKey_t`.
const KEY_T: u32 = 0x0074;
/// `FcitxKey_b`.
const KEY_B: u32 = 0x0062;
/// `FcitxKey_a`.
const KEY_A: u32 = 0x0061;
/// `FcitxKey_o`.
const KEY_O: u32 = 0x006f;
