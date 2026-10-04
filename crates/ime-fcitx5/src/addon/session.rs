//! The session host's sources: the dictionary, the language model and the user's words.
//!
//! Responsibility: run the `lexicon` step — map `base.dict`, build the language model over
//! its unigram table and take the user's frequency store — and the `session-host` step,
//! which hands those sources to the router every key goes through. Together they are what
//! turns a plugin that answers nothing into one that produces candidates.
//!
//! Boundaries: `ime-dict` owns the container, the mapping and the model; `ime-core` owns
//! the decoder; `session_host` owns the slot the sources are installed into. This module
//! decides what the process runs on and what it falls back to.
//!
//! # The load budget
//!
//! `BUDGET-LAT-05` gives the synchronous part of the load 120 ms, and this step is the one
//! that spends it: mapping the container, verifying its checksums and faulting in the pages
//! the first lookups touch. Nothing here runs on a key path — a decode reads the mapped
//! memory and never a file — which is what keeps the dictionary off the per-keystroke
//! budget once the plugin is up.
//!
//! # Degradation
//!
//! A dictionary that is missing, unreadable or damaged is reported and the plugin carries
//! on with an empty one. That is not a fallback in name only: the decoder's own
//! pass-through path turns an input nothing covers into one candidate holding exactly what
//! the user typed, so the degraded plugin commits what is typed instead of swallowing it.
//! The step never fails, which is why it is not fatal.

use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use ime_config::VerifyDictOnLoad;
use ime_core::privacy::DefaultPolicy;
use ime_core::state::SessionEnv;
use ime_core::viterbi::Decoder;
use ime_dict::format::reader::Verify;
use ime_dict::fst_index::{DictLm, FstLexicon};
use ime_types::{ImeError, LanguageModel, Lexicon, UserFreqSource};

use crate::privacy_impl::{AppBlacklist, ContextPrivacy};
use crate::session_host;

use super::config::routing_config;
use super::config::with_config_store;
use super::layout;
use super::probes;
use super::report_step_failure;
use super::user_store::user_store;

pub(super) mod sources;

use self::sources::{Dictionary, LanguageModelSource, UserFrequency};

/// The decoder every session steps through.
///
/// Built once and shared by reference: a decode borrows the decoder immutably and takes its
/// working storage from the caller, so one instance serves every session and no keystroke
/// pays for a second construction.
static DECODER: OnceLock<Decoder> = OnceLock::new();

/// The sources every session decodes against, built on the first call.
static SOURCES: OnceLock<SessionSources> = OnceLock::new();

/// What a session reads: the dictionary, the language model and the user's frequencies.
///
/// Held in one value rather than three slots, because the model borrows the dictionary's
/// mapping: the two are built together and dropped together, and a model that outlived its
/// lexicon would be reading unmapped memory.
pub(super) struct SessionSources {
    /// The dictionary a decode reads.
    lexicon: Dictionary,
    /// The model a decode scores with.
    lm: LanguageModelSource,
    /// The user's own frequencies.
    user_freq: UserFrequency,
}

impl SessionSources {
    /// The dictionary a decode reads.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn lexicon(&self) -> &dyn Lexicon {
        &self.lexicon
    }

    /// The language model a decode scores with.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn lm(&self) -> &dyn LanguageModel {
        &self.lm
    }

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
    if !session_host::install(environment, privacy(), routing_config()) {
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
/// # Arguments
///
/// * `sources` — the dictionary, the language model and the user's frequencies, which must
///   outlive every session built from this environment.
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
        lexicon: sources.lexicon(),
        lm: sources.lm(),
        user_freq: sources.user_freq(),
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
/// The sources, mapped or degraded. The user-frequency source is the store the process
/// adopted, or the silent one when it has none.
///
/// # Panics
///
/// Never.
pub(super) fn sources_for(dictionary: Option<&Path>) -> SessionSources {
    let Some(path) = dictionary else {
        report_step_failure("lexicon", &"no data directory");
        mark_dictionary_unavailable();
        return degraded();
    };
    let lexicon = match FstLexicon::load_with(path, verify_mode()) {
        Ok(lexicon) => lexicon,
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
            return degraded();
        }
    };
    // The model borrows the mapping rather than the value: moving the lexicon into the
    // sources below relocates the struct, while every slice the model holds points into the
    // mapped file, which does not move.
    let lm = DictLm::new(&lexicon);
    // Marked with the mapping in place and before anything reads it, so that the growth
    // covers the pages the mapping faults in: `BUDGET-MEM-03`.
    probes::mark_dictionary_baseline();
    SessionSources {
        lexicon: Dictionary::Mapped(Box::new(lexicon)),
        lm: LanguageModelSource::Mapped(Box::new(lm)),
        user_freq: user_frequency(),
    }
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

/// The sources of a start with no dictionary to map.
///
/// # Panics
///
/// Never.
fn degraded() -> SessionSources {
    SessionSources {
        lexicon: Dictionary::Missing,
        lm: LanguageModelSource::Missing,
        user_freq: user_frequency(),
    }
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
