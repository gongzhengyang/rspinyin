//! The application blacklist and the identifier hash.
//!
//! Responsibility: everything that needs the plaintext application name. The name
//! arrives from the host, is matched against the configured patterns and is hashed;
//! then the plaintext is dropped. This is the only file in the crate where an
//! application name exists at all -- what leaves it is an [`AppIdHash`] and a verdict.
//!
//! Boundaries: plain Rust, with no host object, file, clock or global state. Matching
//! is a case-insensitive substring test, because the identifier an application reports
//! depends on how it was launched: `keepassxc`, `keepassxc-bin` and
//! `org.keepassxc.KeePassXC` are the same program.
//!
//! Empty patterns are dropped rather than kept. An empty substring matches every
//! application, so a stray empty entry in the configuration would silently turn
//! learning off everywhere -- a failure that looks like the plugin being broken rather
//! than like a privacy setting.
//!
//! The blacklist is empty by default, and that is deliberate: we do not assume which
//! password manager the user runs. The capability flag is the primary signal, and
//! modern password managers and browsers set it.

use ime_core::privacy::AppIdHash;

/// FNV-1a's 64-bit offset basis, which is also the hash of the empty string.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a's 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Hashes an application identifier with a fixed seed.
///
/// The hash is stable for the life of the process and across processes: the constants
/// above are part of the algorithm's definition, so the same name always lands on the
/// same value, which is what lets a diagnostic group sessions by application without a
/// name appearing in it.
///
/// The twenty lines below are hand-written because the workspace has no fixed-seed
/// hasher to adopt: the standard library's `DefaultHasher` documents its algorithm as
/// unspecified, so a log field built from it would change meaning under a toolchain
/// upgrade. `rustc-hash` or `fxhash` would be the better home for this and should
/// replace it the next time the manifest is touched. The C++ half of the ABI already
/// digests an input context's uuid with the same algorithm, for the same reason.
///
/// # Parameters
///
/// - `program`: the program name or application id, in the host's own spelling. Case is
///   significant: two spellings are two applications, and the blacklist is where
///   spelling variants are reconciled.
///
/// # Returns
///
/// The identifier the policy and the diagnostics see. [`AppIdHash::UNKNOWN`] is not a
/// possible answer: the offset basis is never zero.
///
/// # Errors
///
/// None.
///
/// # Panics
///
/// Never: the multiplication wraps and the byte loop is bounded by the input length.
///
/// # Examples
///
/// ```
/// use rspinyin::privacy_impl::hash_app_id;
///
/// // Stable for a name, and different for a different name.
/// assert_eq!(hash_app_id("firefox"), hash_app_id("firefox"));
/// assert_ne!(hash_app_id("firefox"), hash_app_id("firefox-bin"));
/// ```
pub fn hash_app_id(program: &str) -> AppIdHash {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in program.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    AppIdHash::from_raw(hash)
}

/// The configured application blacklist.
///
/// See the module documentation for why matching is a case-insensitive substring test
/// and why empty patterns are dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppBlacklist {
    /// The patterns, trimmed and lowercased, with the empty ones removed.
    patterns: Vec<String>,
}

impl AppBlacklist {
    /// Builds a blacklist from the configured patterns.
    ///
    /// # Parameters
    ///
    /// - `patterns`: the `[privacy] app_blacklist` entries as written in the file.
    ///
    /// # Returns
    ///
    /// A matcher over the trimmed, lowercased patterns.
    ///
    /// # Errors
    ///
    /// None: an entry that names no application is dropped, not rejected.
    ///
    /// # Panics
    ///
    /// Never.
    ///
    /// # Examples
    ///
    /// ```
    /// use rspinyin::privacy_impl::AppBlacklist;
    ///
    /// let blacklist = AppBlacklist::new([String::from("keepassxc")]);
    /// assert!(blacklist.matches("org.keepassxc.KeePassXC"));
    /// assert!(!blacklist.matches("firefox"));
    /// assert!(AppBlacklist::default().is_empty());
    /// ```
    pub fn new(patterns: impl IntoIterator<Item = String>) -> Self {
        let patterns = patterns
            .into_iter()
            .map(|pattern| pattern.trim().to_lowercase())
            .filter(|pattern| !pattern.is_empty())
            .collect();
        Self { patterns }
    }

    /// Whether the blacklist names no application at all.
    ///
    /// # Returns
    ///
    /// `true` for the shipped configuration and for a list whose entries were all
    /// blank.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether `program` matches a blacklist entry.
    ///
    /// # Parameters
    ///
    /// - `program`: the program name or application id as the host reported it.
    ///
    /// # Returns
    ///
    /// `true` when a lowercased pattern is a substring of the lowercased name.
    ///
    /// # Errors
    ///
    /// None.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn matches(&self, program: &str) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        let lowered = program.to_lowercase();
        self.patterns
            .iter()
            .any(|pattern| lowered.contains(pattern.as_str()))
    }
}
