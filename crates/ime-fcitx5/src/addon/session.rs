//! The session host's sources: the live dictionary, the language model and the user's
//! words.
//!
//! Responsibility: run the `lexicon` step — map `base.dict` into the hot-swap slot that
//! owns it for the life of the process, build the language model over its unigram table and
//! take the user's frequency store — and the `session-host` step, which hands the sources to
//! the router every key goes through. It is also where a configuration reload lands a
//! staged dictionary: [`apply_staged_dictionary`] validates the staged file through the
//! slot's swap and, when it passes, writes the triggering key back to empty.
//!
//! Boundaries: `ime-dict` owns the container, the mapping, the model and the slot;
//! `ime-core` owns the decoder; `session_host` owns the slot the sources are installed into.
//! This module decides what the process runs on and what it falls back to.
//!
//! # The load budget
//!
//! `BUDGET-LAT-05` gives the synchronous part of the load 120 ms, and this step is the one
//! that spends it: mapping the container, verifying its checksums and faulting in the pages
//! the first lookups touch. Nothing here runs on a key path — a decode reads the mapped
//! memory and never a file — which is what keeps the dictionary off the per-keystroke
//! budget once the plugin is up. A swap runs on the reload callback, also off the key path,
//! and its full validation costs about a millisecond on the shipped container, page cache
//! warm — measured, and far inside the reload budget the task names.
//!
//! # Degradation
//!
//! A dictionary that is missing, unreadable or damaged is reported and the plugin carries
//! on with an empty one. That is not a fallback in name only: the decoder's own
//! pass-through path turns an input nothing covers into one candidate holding exactly what
//! the user typed, so the degraded plugin commits what is typed instead of swallowing it.
//! The step never fails, which is why it is not fatal. A start that mapped nothing installs
//! no slot, so it has no container to swap either: a staged file is refused, and the
//! configuration key stays for the user to correct.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ime_config::{DataConfig, VerifyDictOnLoad, write_data};
use ime_core::privacy::DefaultPolicy;
use ime_core::state::SessionEnv;
use ime_core::viterbi::Decoder;
use ime_dict::format::reader::Verify;
use ime_dict::hot_swap::{LexiconBundle, LexiconSlot};
use ime_dict::script::ScriptIndex;
use ime_types::{DictError, ImeError, UserFreqSource};

use crate::ffi::emit_diagnostic;
use crate::privacy_impl::{AppBlacklist, ContextPrivacy};
use crate::session_host;

use super::config::routing_config;
use super::config::with_config_store;
use super::layout;
use super::probes;
use super::report_step_failure;
use super::user_store::user_store;

pub(super) mod sources;

// Re-exported `pub(super)` so the addon's own tests can name the stand-ins: the
// environment of a degraded start is exactly these values.
pub(super) use self::sources::{MISSING_DICTIONARY, MISSING_LANGUAGE_MODEL, UserFrequency};

/// Diagnostic reported once when a mapped dictionary has no BIGRAM model.
const BIGRAM_ABSENT_CODE: &str = "dict/bigram/absent";

/// Diagnostic reported when a staged container was validated and swapped in.
const DICT_SWAP_APPLIED_CODE: &str = "dict/swap/applied";

/// Diagnostic reported when a staged container failed its validation and the live
/// dictionary was kept.
const DICT_SWAP_REJECTED_CODE: &str = "dict/swap/rejected";

/// The decoder every session steps through.
///
/// Built once and shared by reference: a decode borrows the decoder immutably and takes its
/// working storage from the caller, so one instance serves every session and no keystroke
/// pays for a second construction.
static DECODER: OnceLock<Decoder> = OnceLock::new();

/// The live dictionary: the container a decode reads, replaceable by a validated swap.
///
/// Installed by the `lexicon` step when the start mapped a container, and empty when it did
/// not — a degraded start has nothing to swap. The router reaches it through
/// [`session_host::install`]; a reload reaches it through [`lexicon_slot`].
static LEXICON_SLOT: OnceLock<LexiconSlot> = OnceLock::new();

/// The user-frequency source of the process, built on the first call.
///
/// The one source that never changes over the process's life — a swap replaces the
/// dictionary and its model together, never the user's own words — so it stays a plain
/// static rather than joining the slot.
static SOURCES: OnceLock<SessionSources> = OnceLock::new();

/// What every session reads besides the dictionary: the user's own frequencies.
///
/// The dictionary and the model live in [`LEXICON_SLOT`] rather than here, because they are
/// the pair a swap replaces as one — the model reads the mapping the lexicon owns, so the
/// two are swapped and dropped together, which is the slot's own guarantee.
pub(super) struct SessionSources {
    /// The user's own frequencies.
    user_freq: UserFrequency,
}

impl SessionSources {
    /// The user's own frequencies.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn user_freq(&self) -> &dyn UserFreqSource {
        &self.user_freq
    }
}

/// The `lexicon` step: maps the compiled dictionary and builds the sources.
///
/// # Returns
///
/// `Ok(())` whether or not a dictionary could be mapped: a dictionary that is missing,
/// unreadable or damaged is reported and the plugin runs on an empty one.
///
/// # Errors
///
/// None. A damaged dictionary never costs the user their input method.
///
/// # Panics
///
/// Never.
pub(super) fn load_lexicon() -> Result<(), ImeError> {
    // The sources are built on first use and the step is that first use, so the reporting
    // inside the builder lands on this step's line rather than on a later one's.
    let _ = session_sources();
    Ok(())
}

/// The `session-host` step: hands the sources to the router every key goes through.
///
/// A second install is refused rather than replacing the first: the router already there is
/// the one a key may be running through, and swapping it would drop the composition the
/// user is in the middle of.
///
/// # Returns
///
/// `Ok(())` whether or not the host was installed.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never.
pub(super) fn install_session_host() -> Result<(), ImeError> {
    let environment = session_env(session_sources());
    if session_host::install(environment, privacy(), routing_config(), lexicon_slot()) {
        // The variant table the script mode converts through: the built-in seed, which
        // this build always carries. A container-sourced table replaces this call, not
        // this wiring; a build whose table is missing holds
        // [`ScriptState::Missing`](ime_fcitx5::engine::router) and the toggle reports
        // `ui/script/unavailable` instead of flipping.
        session_host::install_script(std::sync::Arc::new(ScriptIndex::seed()));
    } else {
        report_step_failure("session-host", &"a session host was already installed");
    }
    Ok(())
}

/// The environment a session decodes against, over `sources`.
///
/// The entry point a test drives when it wants a host over sources of its own: the
/// references are `'static` because the host outlives every callback that reaches it, which
/// is why the caller owns the sources for the life of the process.
///
/// The dictionary and the model here are the degraded stand-ins. A start that mapped a
/// container has it in the slot, and the router reads the slot at every step; these
/// references are what a start without a container decodes against, which is the
/// pass-through degradation.
///
/// # Arguments
///
/// * `sources` — the user's frequencies, which must outlive every session built from this
///   environment.
///
/// # Returns
///
/// The environment [`crate::session_host::install`] takes.
///
/// # Panics
///
/// Never.
pub(super) fn session_env(sources: &'static SessionSources) -> SessionEnv<'static> {
    SessionEnv {
        decoder: decoder(),
        lexicon: &MISSING_DICTIONARY,
        user_freq: sources.user_freq(),
        lm: &MISSING_LANGUAGE_MODEL,
    }
}

/// The sources every session decodes against, built on the first call.
///
/// The dictionary is the one the `data-dirs` step's layout names. A process with no layout
/// has no dictionary to map, which is the degraded start rather than an error.
///
/// # Panics
///
/// Never.
pub(super) fn session_sources() -> &'static SessionSources {
    SOURCES.get_or_init(|| sources_for(layout::dictionary_path().as_deref()))
}

/// The live dictionary of the process, when the start mapped one.
///
/// `None` for a start that mapped nothing: there is no container to replace, so a staged
/// swap is refused rather than pretending there is something to keep.
///
/// # Panics
///
/// Never.
pub(super) fn lexicon_slot() -> Option<&'static LexiconSlot> {
    LEXICON_SLOT.get()
}

/// Builds the sources a session decodes against, for the dictionary at `dictionary`.
///
/// The entry point a test drives: with the path injected, whether a dictionary is mapped or
/// the empty one is used depends only on the file, and no environment is read.
///
/// # Arguments
///
/// * `dictionary` — the compiled container to map, or `None` when the layout is unknown.
///
/// # Returns
///
/// The sources. A mapped container goes into [`LEXICON_SLOT`] whole — mapping, index and
/// model together — and the returned value carries only the user's frequencies.
///
/// # Panics
///
/// Never.
pub(super) fn sources_for(dictionary: Option<&Path>) -> SessionSources {
    let sources = SessionSources {
        user_freq: user_frequency(),
    };
    let Some(path) = dictionary else {
        report_step_failure("lexicon", &"no data directory");
        mark_dictionary_unavailable();
        return sources;
    };
    let bundle = match LexiconBundle::load(path, verify_mode()) {
        Ok(bundle) => bundle,
        Err(cause) => {
            // Reported as unavailable rather than as corrupt: the dictionary is shipped
            // data the user reinstalls, not user data the plugin repairs, so nothing is
            // moved aside and the cause names what was wrong with the file.
            let error = ImeError::DictUnavailable {
                path: path.to_path_buf(),
                cause,
            };
            report_step_failure("lexicon", &error);
            mark_dictionary_unavailable();
            return sources;
        }
    };
    if bundle.lexicon().bigram_section().is_empty() {
        emit_diagnostic(BIGRAM_ABSENT_CODE);
    }
    // Marked with the mapping in place and before anything reads it, so that the growth
    // covers the pages the mapping faults in: `BUDGET-MEM-03`.
    probes::mark_dictionary_baseline();
    // The process's one slot: a second install would mean a second `sources_for`, which
    // the `OnceLock` over [`SOURCES`] already makes unreachable.
    let _ = LEXICON_SLOT.set(LexiconSlot::new(bundle));
    sources
}

/// Whether the process started with no usable dictionary.
///
/// Set by [`sources_for`] on either degraded path and read by the engine's notice
/// selection through the addon's re-export, which is the one visibility bridge the
/// router's private module layout allows. Sticky for the process's life: remapping is
/// not attempted after a failed load, so a cleared flag could only misreport.
static DICTIONARY_UNAVAILABLE: AtomicBool = AtomicBool::new(false);

/// Records that this process has no usable dictionary.
fn mark_dictionary_unavailable() {
    DICTIONARY_UNAVAILABLE.store(true, Ordering::Release);
}

/// Whether this process started with no usable dictionary.
///
/// The engine's notice table reads this through `crate::addon`'s re-export to decide
/// whether the `dict/unavailable` notice takes the status label.
pub fn dictionary_unavailable() -> bool {
    DICTIONARY_UNAVAILABLE.load(Ordering::Acquire)
}

/// The user's frequencies: the store the process adopted, or the silent source.
///
/// # Panics
///
/// Never.
fn user_frequency() -> UserFrequency {
    match user_store() {
        Some(store) => UserFrequency::Store(store),
        None => UserFrequency::Missing,
    }
}

/// Swaps in the staged container a reloaded configuration names, when it names one.
///
/// The reload path's dictionary half: `[data] dict_staged_path` set to a file makes the
/// reload that adopts that document validate the file through [`LexiconSlot::try_swap`] —
/// the container load's own full validation — and, when it passes, replace the live
/// dictionary with it. A composition in progress is untouched: the session state holds no
/// reference into the old mapping, so the next decode simply reads the new container, which
/// is the semantics the frozen `Arc` handout gives every reader.
///
/// On success the key is written back to empty through the writeback channel, so the same
/// file is not re-validated at every later reload; the configuration document in force
/// still names the path until the next reload adopts the cleared file, which is harmless —
/// an empty staged path means no swap is attempted. On failure the live dictionary keeps
/// serving, the reason is reported as `dict/swap/rejected`, the key stays for the user to
/// correct, and a file that is present but is not a valid container is moved aside, so the
/// damaged bytes cannot sit in the staged path waiting to be tried again.
///
/// # Arguments
///
/// * `data` — the `[data]` section of the document the reload adopted.
///
/// # Panics
///
/// Never.
pub(super) fn apply_staged_dictionary(data: &DataConfig) {
    let staged = data.dict_staged_path.as_str();
    if staged.is_empty() {
        return;
    }
    let staged_path = Path::new(staged);
    let outcome = match lexicon_slot() {
        Some(slot) => slot.try_swap(staged_path).map(|_| ()),
        // A start that mapped no container has no slot, and nothing live to keep serving
        // either: the staged file is refused with the same code, and the key stays for the
        // user to correct.
        None => Err(DictError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no live dictionary to replace",
        ))),
    };
    match outcome {
        Ok(()) => {
            emit_diagnostic(DICT_SWAP_APPLIED_CODE);
            clear_staged_key(data);
        }
        Err(cause) => match quarantine_staged(staged_path) {
            Some(Ok(quarantined)) => emit_diagnostic(&format!(
                "{DICT_SWAP_REJECTED_CODE}: {cause}; quarantined to {}",
                quarantined.display()
            )),
            Some(Err(error)) => emit_diagnostic(&format!(
                "{DICT_SWAP_REJECTED_CODE}: {cause}; quarantine failed: {error}"
            )),
            // Nothing to move: the file was never written, or is gone already. The
            // rejection line is the whole report.
            None => emit_diagnostic(&format!("{DICT_SWAP_REJECTED_CODE}: {cause}")),
        },
    }
}

/// Moves a rejected staged file aside, so damaged bytes cannot sit in the staged path.
///
/// The candidate is renamed beside itself with a `.corrupt.` stamp — the suffix the
/// repository's ignore rules already cover — so the user's correction flow starts from a
/// clean staged path and the rejected bytes stay inspectable.
///
/// # Returns
///
/// `None` when there is no file to move — the candidate was never written, or is gone
/// already — and otherwise the rename's outcome, with the path the file now sits at.
///
/// # Panics
///
/// Never.
fn quarantine_staged(staged: &Path) -> Option<std::io::Result<PathBuf>> {
    if !staged.is_file() {
        return None;
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    let mut quarantined = staged.as_os_str().to_os_string();
    quarantined.push(format!(".corrupt.{stamp}"));
    let quarantined = PathBuf::from(quarantined);
    Some(fs::rename(staged, &quarantined).map(|()| quarantined))
}

/// Writes `[data] dict_staged_path` back to empty, so a served swap is not re-attempted.
///
/// The write goes through the same channel the configuration's own writers use, race check
/// included: a `[data]` region the user has edited since the reload read it refuses the
/// clear, and the key stays — the user's edit wins over this module's bookkeeping. The
/// configuration store's in-force copy still names the path until the next reload adopts
/// the cleared file; a document naming an empty path attempts nothing, so the divergence
/// costs one harmless adoption.
///
/// # Panics
///
/// Never.
fn clear_staged_key(previous: &DataConfig) {
    let mut cleared = previous.clone();
    cleared.dict_staged_path = String::new();
    let path = with_config_store(|store| store.path().to_path_buf());
    let Some(path) = path else {
        report_step_failure("dict-swap", &"no configuration store to clear the staged key");
        return;
    };
    if let Err(error) = write_data(&path, previous, &cleared) {
        // The swap stands — only the bookkeeping failed — so the key stays and the same
        // file is validated again at the next reload, which is wasteful once and correct
        // always.
        report_step_failure("dict-swap", &ImeError::from(error));
    }
}

/// The decoder every session steps through, built on the first call.
///
/// # Panics
///
/// Never.
fn decoder() -> &'static Decoder {
    DECODER.get_or_init(Decoder::default)
}

/// The per-context privacy state a fresh router is built with.
///
/// The shipped policy and an empty application blacklist, which is what the configuration
/// document asks for: it carries no key for either yet, so the built-in defaults are the
/// configuration in force.
///
/// # Panics
///
/// Never.
fn privacy() -> ContextPrivacy {
    ContextPrivacy::new(Box::new(DefaultPolicy::default()), AppBlacklist::default())
}

/// How much of the dictionary this load verifies, as `[engine] verify_dict_on_load` asks.
///
/// With no configuration store the answer is the key's own default, `full`, which is what a
/// fresh installation asks for.
///
/// # Panics
///
/// Never.
fn verify_mode() -> Verify {
    with_config_store(|store| match store.current().engine.verify_dict_on_load {
        VerifyDictOnLoad::Full => Verify::Full,
        VerifyDictOnLoad::Header => Verify::Header,
    })
    .unwrap_or_default()
}
