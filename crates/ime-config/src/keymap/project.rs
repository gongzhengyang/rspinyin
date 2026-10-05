//! The `[keys]` section as the routing layer's binding table.
//!
//! Responsibility: turn the settings of `[keys]` into [`KeyBindings`] -- the table the
//! routing layer reads one key at a time -- and report every entry that cannot become a
//! binding. `crate::schema` owns the file format, the key-name whitelist and the
//! validation; this module is the one place those values become the router's own types.
//!
//! # Boundaries
//!
//! Pure (0.4 rule 4): no filesystem, no clock, no environment, no global state. It reads a
//! [`KeysConfig`] and answers with a value, so a projection is deterministic and testable
//! with no dictionary, no display server and no host.
//!
//! # Hot reload
//!
//! A reload re-runs the projection and adopts the result as a whole
//! ([`KeyBindings::reproject`]). Two properties of this module are what keep a reload from
//! disturbing a composition in progress (0.4 rule 10):
//!
//! * The projection reads nothing but the configuration and keeps no session state, so a
//!   reload can only change how *later* keys are routed. The input buffer, the candidate
//!   list and the highlight live in the session, which this module never sees.
//! * The table is replaced whole rather than patched field by field, so no reload can leave
//!   the page keys of one configuration beside the highlight keys of another.
//!
//! A configuration that cannot be used never fails a projection: the entry is dropped and
//! reported, because a typo in a key binding must not be able to cost the user their input
//! method.
//!
//! # Diagnostics
//!
//! Two stable codes travel in the `key` field of [`ImeError::ConfigInvalid`], which is
//! where a code with no variant of its own stays matchable without a call to the
//! process-wide diagnostic channel:
//!
//! * [`UNROUTABLE_BINDING_CODE`] -- a name the whitelist accepts that a list has no binding
//!   for.
//! * [`BINDING_CONFLICT_CODE`] -- one key claimed twice: the same name twice in one list,
//!   or one name in both lists.

use ime_types::ImeError;

use crate::schema::{DigitZero, KEY_FLIP_KEYS, KEY_HIGHLIGHT_KEYS, KeyName, KeysConfig};

#[cfg(test)]
mod tests;

/// The code a key name a list cannot use carries.
///
/// Stable, like every other code in the project: diagnostics and tests match on it. It
/// renders as `config/invalid: keys/unroutable-binding (...)`, and the reason names the
/// list, the offending name and the names that list does accept.
pub const UNROUTABLE_BINDING_CODE: &str = "keys/unroutable-binding";

/// The code a key claimed twice carries.
///
/// Two shapes share it, and the reason tells them apart: the same name twice in one list,
/// and one name in both lists. Both used to be settled by whichever entry the table read
/// first -- a decision the user never saw -- so both are reported instead.
pub const BINDING_CONFLICT_CODE: &str = "keys/binding-conflict";

/// The names `keys.flip_keys` can page with, in the order the diagnostics list them.
const PAGEABLE: &[KeyName] = &[
    KeyName::Minus,
    KeyName::Equal,
    KeyName::Up,
    KeyName::Down,
    KeyName::PageUp,
    KeyName::PageDown,
    KeyName::Home,
    KeyName::End,
];

/// The names `keys.highlight_keys` can move the highlight with.
const HIGHLIGHTABLE: &[KeyName] = &[
    KeyName::Tab,
    KeyName::ShiftTab,
    KeyName::Up,
    KeyName::Down,
    KeyName::Left,
    KeyName::Right,
];

bitflags::bitflags! {
    /// The keys that page the candidate list (`[keys] flip_keys`).
    ///
    /// A flag set rather than the four-field struct this used to be: the whitelist keeps
    /// outgrowing any fixed shape a struct could carry — the arrows, the minus and equal
    /// pair, the page pair and now the `Home`/`End` jumps — and a field that does not
    /// exist for a name means the configuration accepts it and a binding drops it in
    /// silence.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct FlipSet: u8 {
        /// `-`.
        const MINUS = 1 << 0;
        /// `=`.
        const EQUAL = 1 << 1;
        /// `Up`.
        const UP = 1 << 2;
        /// `Down`.
        const DOWN = 1 << 3;
        /// `Page_Up`.
        const PAGE_UP = 1 << 4;
        /// `Page_Down`.
        const PAGE_DOWN = 1 << 5;
        /// `Home`, which jumps to the first page.
        const HOME = 1 << 6;
        /// `End`, which jumps to the last page.
        const END = 1 << 7;
    }

    /// The keys that move the candidate highlight (`[keys] highlight_keys`).
    ///
    /// The setting behind it was parsed, stored and read by nothing: `Tab` moved the
    /// highlight because a row of the routing table said so, whatever the user wrote.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct HighlightSet: u8 {
        /// `Tab`.
        const TAB = 1 << 0;
        /// `Shift+Tab`.
        const SHIFT_TAB = 1 << 1;
        /// `Up`.
        const UP = 1 << 2;
        /// `Down`.
        const DOWN = 1 << 3;
        /// `Left`.
        const LEFT = 1 << 4;
        /// `Right`.
        const RIGHT = 1 << 5;
    }
}

/// The `[keys]` settings the routing layer reads.
///
/// A value, not a view: a reload replaces it whole and nothing in it refers to a session,
/// which is what makes a reload unable to disturb a composition in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyBindings {
    /// What `0` does while a composition is active (`keys.digit_zero`).
    pub digit_zero: DigitZero,
    /// Whether `Enter` commits the raw input instead of the highlighted candidate
    /// (`keys.enter_commit_raw`).
    pub enter_commit_raw: bool,
    /// The keys that page the candidate list (`keys.flip_keys`).
    pub flip_keys: FlipSet,
    /// The keys that move the candidate highlight (`keys.highlight_keys`).
    pub highlight_keys: HighlightSet,
}

impl Default for KeyBindings {
    /// The table the shipped `config/default.toml` declares.
    ///
    /// Written out rather than derived from a [`KeysConfig`], so that a caller which has no
    /// configuration yet routes keys exactly as a fresh installation does. A test projects
    /// the built-in defaults and requires the two to be equal, so the table and the document
    /// cannot drift apart.
    fn default() -> Self {
        Self {
            digit_zero: DigitZero::Passthrough,
            enter_commit_raw: false,
            flip_keys: FlipSet::MINUS
                | FlipSet::EQUAL
                | FlipSet::UP
                | FlipSet::DOWN
                | FlipSet::HOME
                | FlipSet::END,
            highlight_keys: HighlightSet::TAB | HighlightSet::SHIFT_TAB,
        }
    }
}

impl KeyBindings {
    /// Re-projects the table from a re-read `[keys]` section.
    ///
    /// The hot-reload path: the addon calls this when the host asks it to reload the
    /// configuration, and the table is replaced as a whole. Two things follow from the
    /// signature, and both are what keeps a reload from disturbing a composition in progress
    /// (0.4 rule 10): the only thing this can reach is the binding table, so no reload can
    /// touch a session; and the replacement is whole, so no reload can leave the page keys of
    /// one configuration beside the highlight keys of another.
    ///
    /// # Arguments
    ///
    /// * `keys` -- the `[keys]` section of the configuration that replaces the one in force.
    ///
    /// # Returns
    ///
    /// One diagnostic per entry that could not become a binding, exactly as [`project_keys`]
    /// reports them. Re-projecting an unchanged document therefore leaves the same table in
    /// force and raises the same diagnostics the load before it raised.
    ///
    /// # Errors
    ///
    /// None: an entry that cannot be used is a diagnostic, not a failure.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn reproject(&mut self, keys: &KeysConfig) -> Vec<ImeError> {
        let (projected, warnings) = project_keys(keys);
        *self = projected;
        warnings
    }
}

/// Projects the `[keys]` section onto the routing layer's binding table.
///
/// # Arguments
///
/// * `keys` -- the `[keys]` section of the configuration in force.
///
/// # Returns
///
/// The table the routing layer reads, and one diagnostic per entry that could not become a
/// binding. A projection always answers with a usable table: an entry a list cannot use, an
/// entry a list names twice, and a key both lists bound are dropped and reported rather than
/// refused, because a key binding must not be able to stop the input method from working.
///
/// # Errors
///
/// None: an entry that cannot be used is a diagnostic, not a failure.
///
/// # Panics
///
/// Never: the walk reads two lists and writes the bits of a `u8`, with no indexing and no
/// arithmetic that can overflow.
///
/// # Examples
///
/// ```
/// use ime_config::Config;
/// use ime_config::keymap::project::{FlipSet, project_keys};
///
/// let (bindings, warnings) = project_keys(&Config::default().keys);
///
/// assert!(warnings.is_empty(), "every shipped default is routable");
/// assert_eq!(
///     bindings.flip_keys,
///     FlipSet::MINUS
///         | FlipSet::EQUAL
///         | FlipSet::UP
///         | FlipSet::DOWN
///         | FlipSet::HOME
///         | FlipSet::END
/// );
/// ```
pub fn project_keys(keys: &KeysConfig) -> (KeyBindings, Vec<ImeError>) {
    let mut warnings = Vec::new();
    let mut flip_keys = FlipSet::empty();
    let mut highlight_keys = HighlightSet::empty();
    for name in &keys.flip_keys {
        record_page(*name, &mut flip_keys, &mut warnings);
    }
    for name in &keys.highlight_keys {
        record_highlight(*name, &mut highlight_keys, &mut warnings);
    }
    // A key cannot page the list and move the highlight at once. The page binding is the
    // one that gives way, which is the precedence the routing layer applies to a key both
    // lists name, so the configuration and the router answer with one table.
    //
    // The overlap is taken from the names, not from the bit positions. The two sets happen
    // to number `Up` and `Down` alike today, and an intersection built on that coincidence
    // would quietly stop finding anything the moment one set gained a flag.
    let mut collisions = FlipSet::empty();
    for name in &keys.flip_keys {
        if keys.highlight_keys.contains(name) {
            if let Some(bit) = page_bit(*name) {
                collisions |= bit;
            }
        }
    }
    flip_keys.remove(collisions);
    report_collisions(collisions, &mut warnings);
    let bindings = KeyBindings {
        digit_zero: keys.digit_zero,
        enter_commit_raw: keys.enter_commit_raw,
        flip_keys,
        highlight_keys,
    };
    (bindings, warnings)
}

/// The bit `name` carries in `keys.flip_keys`, or `None` when that list cannot use it.
///
/// Exhaustive over the whitelist, so a name added to `KeyName` cannot reach a list without
/// someone deciding which binding it carries.
fn page_bit(name: KeyName) -> Option<FlipSet> {
    match name {
        KeyName::Minus => Some(FlipSet::MINUS),
        KeyName::Equal => Some(FlipSet::EQUAL),
        KeyName::Up => Some(FlipSet::UP),
        KeyName::Down => Some(FlipSet::DOWN),
        KeyName::PageUp => Some(FlipSet::PAGE_UP),
        KeyName::PageDown => Some(FlipSet::PAGE_DOWN),
        KeyName::Home => Some(FlipSet::HOME),
        KeyName::End => Some(FlipSet::END),
        KeyName::Left | KeyName::Right | KeyName::Tab | KeyName::ShiftTab => None,
    }
}

/// The bit `name` carries in `keys.highlight_keys`, or `None` when that list cannot use it.
fn highlight_bit(name: KeyName) -> Option<HighlightSet> {
    match name {
        KeyName::Tab => Some(HighlightSet::TAB),
        KeyName::ShiftTab => Some(HighlightSet::SHIFT_TAB),
        KeyName::Up => Some(HighlightSet::UP),
        KeyName::Down => Some(HighlightSet::DOWN),
        KeyName::Left => Some(HighlightSet::LEFT),
        KeyName::Right => Some(HighlightSet::RIGHT),
        KeyName::Minus | KeyName::Equal | KeyName::PageUp | KeyName::PageDown => None,
        // Home and End page, they do not highlight: the two bits they carry live in
        // `FlipSet`, and the highlight list has no use for them.
        KeyName::Home | KeyName::End => None,
    }
}

/// Adds one entry of `keys.flip_keys` to `set`, or reports why it cannot be added.
///
/// The two lists are walked by a function each rather than by one shared walk: the bit a
/// name carries is a different flag type per list and the two diagnostics are worded
/// differently. What the two must not duplicate is the table of which name carries which
/// bit, and they do not -- [`page_bit`] and [`highlight_bit`] are the only copies of it.
fn record_page(name: KeyName, set: &mut FlipSet, warnings: &mut Vec<ImeError>) {
    let Some(bit) = page_bit(name) else {
        warnings.push(binding_error(
            UNROUTABLE_BINDING_CODE,
            unroutable(KEY_FLIP_KEYS, "page", name, PAGEABLE),
        ));
        return;
    };
    if set.contains(bit) {
        warnings.push(binding_error(
            BINDING_CONFLICT_CODE,
            repeated(KEY_FLIP_KEYS, name),
        ));
        return;
    }
    *set |= bit;
}

/// Adds one entry of `keys.highlight_keys` to `set`, or reports why it cannot be added.
fn record_highlight(name: KeyName, set: &mut HighlightSet, warnings: &mut Vec<ImeError>) {
    let Some(bit) = highlight_bit(name) else {
        warnings.push(binding_error(
            UNROUTABLE_BINDING_CODE,
            unroutable(KEY_HIGHLIGHT_KEYS, "highlight", name, HIGHLIGHTABLE),
        ));
        return;
    };
    if set.contains(bit) {
        warnings.push(binding_error(
            BINDING_CONFLICT_CODE,
            repeated(KEY_HIGHLIGHT_KEYS, name),
        ));
        return;
    }
    *set |= bit;
}

/// Reports every key that both lists bound.
///
/// The caller has already removed `shared` from the page set; this names what it removed, so
/// that a key which was silently read by whichever row came first is now a diagnostic. The
/// walk goes over the page whitelist because a bit can be named from either list and the
/// page list is the one that gives way.
fn report_collisions(shared: FlipSet, warnings: &mut Vec<ImeError>) {
    for name in PAGEABLE {
        // The table answers `Some` for every name in `PAGEABLE`; the guard keeps the walk
        // total rather than relying on that, and a test pins the two together.
        let Some(bit) = page_bit(*name) else {
            continue;
        };
        if !shared.contains(bit) {
            continue;
        }
        warnings.push(binding_error(BINDING_CONFLICT_CODE, collision(*name)));
    }
}

/// The message for a name a list cannot use.
///
/// The accepted names are listed rather than described: a user who wrote `left` into
/// `keys.flip_keys` learns from the diagnostic what to write instead.
fn unroutable(list: &str, binding: &str, name: KeyName, allowed: &[KeyName]) -> String {
    format!(
        "{list} cannot use \"{}\": it has no {binding} binding; use one of {}",
        name.as_str(),
        spellings(allowed)
    )
}

/// The message for a name a list already bound.
fn repeated(list: &str, name: KeyName) -> String {
    format!(
        "{list} names \"{}\" twice; the second entry is ignored",
        name.as_str()
    )
}

/// The message for a key both lists bound.
fn collision(name: KeyName) -> String {
    format!(
        "\"{}\" is bound by both {KEY_FLIP_KEYS} (page) and {KEY_HIGHLIGHT_KEYS} (highlight); \
         the highlight binding is kept",
        name.as_str()
    )
}

/// The spellings of a list of names, as a diagnostic writes them.
fn spellings(names: &[KeyName]) -> String {
    names
        .iter()
        .map(|name| name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The error a binding the projection cannot record comes back as.
///
/// The frozen error model has no variant for a key-binding table, so the failure folds onto
/// [`ImeError::ConfigInvalid`] and the stable code travels in the `key` field, where a
/// diagnostic or a test can match it. Recording the code on the process-wide diagnostic
/// channel instead would put global state in a module that is otherwise a pure function, and
/// would leave the caller that has to act on the refusal with nothing to match.
fn binding_error(code: &str, reason: String) -> ImeError {
    ImeError::ConfigInvalid {
        key: String::from(code),
        reason,
    }
}
