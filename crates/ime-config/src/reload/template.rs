//! The configuration a user who has none is given: the template `config.toml` is
//! written from, and the documentation of every key.
//!
//! Split out of the parent to keep that file inside the line limit; this module owns
//! one constant and nothing else. The public path is unchanged -- the parent
//! re-exports the constant, so every reader of
//! [`ime_config::reload::DEFAULT_CONFIG_TOML`](crate::reload::DEFAULT_CONFIG_TOML)
//! reads the same item it always has.

/// The configuration a user who has none is given, and the documentation of every key.
///
/// This is what the loader writes when there is no `config.toml` yet: a user finds the
/// available keys by reading the file rather than by reading the crate. It is also the
/// statement of the built-in defaults -- a test parses it and requires the result to
/// equal `Config::default`, so the document and the defaults cannot drift apart.
///
/// The design places this document in `config/default.toml` and pulls it in with
/// `include_str!`; it is inlined as a string so that the crate compiles from
/// `crates/ime-config` alone, with no build-time dependency on a repository path.
pub const DEFAULT_CONFIG_TOML: &str = r##"# rspinyin configuration.
#
# Every key is optional: an absent key keeps its built-in default, which is the value
# written here. The file is read at startup and again whenever the host asks the addon
# to reload. A file that cannot be read or parsed never stops the input method.
#
# Every key's comment also ends with how a change to it reaches the running plugin:
#
#   Takes effect on reload -- the host's configuration reload applies it
#   Takes effect at the next fcitx5 restart -- read once, at start-up
#   Not wired yet -- accepted by this build, but nothing reads it

# Format version. Only 2 is understood; a file written by an older build is migrated
# once, with the original kept beside it. Read on every load and every reload: it
# selects the migration, never a behaviour.
schema_version = 2

[engine]
# "chinese" replaces ASCII punctuation with its Chinese mark; "english" leaves it to
# the application. Takes effect on reload: new contexts start where the document says,
# and the chords keep flipping both switches at runtime.
punct_mode = "chinese"
# Widen the committed text of ASCII characters. Takes effect on reload: new contexts
# start where the document says, and the chord keeps flipping the switch at runtime.
full_width = false
# Commit a leading uppercase letter instead of decoding it, so typing English needs no
# mode switch. Takes effect on reload.
auto_english_on_uppercase = true
# Leave the keys to the application while the caret is inside a URL or an email.
# Takes effect on reload.
passthrough_url = true
# Hard limit on the length of the raw input, 1..=64. Takes effect on reload, and it
# applies to the next character pushed, never to the input already on screen.
max_raw_len = 64
# "full" verifies the whole dictionary when it is loaded, "header" only its header.
# Read once, when the dictionary is mapped. Takes effect at the next fcitx5 restart.
verify_dict_on_load = "full"
# Expand initial-letter abbreviations, so that `nh` reaches `ni'hao`. Off by default:
# an abbreviation is ambiguous by nature, and the abbreviation candidate rides a
# penalty so it never outranks the full spelling at equal dictionary weight. Takes
# effect on reload: the switch latches with the scheme, so a change answers the next
# composition and never re-reads one in progress.
abbrev = false
# Correct one-step typing slips -- a keyboard-neighbour substitution or an adjacent
# transposition -- for an input no exact spelling covers, so that `zhegn` still
# reaches the word spelled `zheng`. Off by default: a corrected candidate rides a
# heavy penalty and is marked in its annotation, and an input that spells a word is
# never corrected at all. Takes effect on reload: the switch latches with the scheme,
# so a change answers the next composition and never re-reads one in progress.
correction = false
# Offer the English lexicon's entry for an input no pinyin reading cuts, so that
# `github` reaches the word spelled `GitHub`. Off by default: a user who never asked
# for English candidates must not have their compositions answered from a table they
# did not know was there. Takes effect on reload: the switch latches with the scheme,
# so a change answers the next composition and never re-reads one in progress.
mixed_english = false
# What happens to a live composition when the input method is switched away from
# or reset. "clear" takes the composing text back and commits nothing; "commit_raw"
# commits the raw input first, so a user who switches away mid-word keeps the
# letters they typed. Takes effect on reload from the next switch away.
on_im_switch = "clear"
# Answer a dead-key sequence the host is composing -- an acute accent followed by
# `e`, for one -- before the key table sees the key. Off by default: a pass that
# swallows keys changes what a European keyboard types, and a host that offers no
# compose support leaves the switch inert.
compose = false
# "context" keeps one session per input context; "program" shares one session across
# every input context of the same application, so the Chinese/English mode and the
# other mode bits survive a window switch inside one application. Applications are
# told apart by a hash of their identifier, never by a name. Takes effect on reload:
# the sessions already alive finish under the policy they started with, and the next
# activation pools under the new one.
share_state = "context"
# Let the backtick key open the auxiliary-code refinement session over a live
# composition: further letters narrow the candidate list, and Escape or a backspace
# to the entry point hands the full list back. Off by default, and it needs a
# dictionary that carries the aux column. Takes effect on reload.
aux_refinement = false
# Let a lowercase `u` at the start of the input open the decomposition
# reverse-lookup domain: the letters after it name the components of a character
# you cannot pronounce, and the window lists every character whose decomposition
# contains that fragment, each annotated with its full code. Off by default, and
# it needs a dictionary that carries the reverse index. Takes effect on reload.
reverse_lookup = false

[ui]
# Show the composing text in the application's preedit area instead of the window.
# An application the host reports as unable to render a preedit area is never
# written one, whatever this says: the candidate window's header takes over.
# Takes effect on reload.
client_preedit = false
# Candidates per row, 3..=9. Takes effect on reload.
max_per_row = 5
# Show the word annotation beside each candidate. Takes effect on reload.
show_annotation = true
# Widest candidate window, 220..=1200 dp. Takes effect on reload.
max_width_dp = 720
# Corner radius, 8..=20 dp. The window look is built once, at the user-interface
# addon's start-up. Takes effect at the next fcitx5 restart.
corner_radius_dp = 12
# Background opacity, 0..=255. The default is 0.85. Takes effect at the next fcitx5
# restart.
base_alpha = 217

[ui.animation]
# Animate the window with a spring: angular frequency in rad/s, 4.0..=80.0, and the
# damping ratio, 0.3..=2.0. The durations are 0..=600 ms. The whole section is read
# when the window is built. Takes effect at the next fcitx5 restart.
enabled = true
omega0 = 26.0
zeta = 0.85
appear_ms = 110
disappear_ms = 90

[theme]
# "auto" follows the desktop's colour scheme; "light" and "dark" pin it. Read by the
# user-interface addon once, at start-up. Takes effect at the next fcitx5 restart.
scheme = "auto"
# Accent colour, written as #RRGGBB. Takes effect at the next fcitx5 restart.
accent = "#4C9AFF"

[keys]
# What the "0" key does once the candidate list reaches ten entries: "passthrough"
# sends the digit to the application, "flip" turns the page. Takes effect on reload.
digit_zero = "passthrough"
# Commit the raw input on Enter instead of the highlighted candidate. Takes effect on
# reload.
enter_commit_raw = false
# Keys that page the candidate list, at most six, and keys that move the highlight.
# "home" jumps to the first page and "end" to the last. Takes effect on reload.
flip_keys = ["minus", "equal", "up", "down", "home", "end"]
highlight_keys = ["tab", "shift_tab"]
# The mark that commits the highlighted candidate's first character, and the mark
# that commits its last one: a single character picked out of a word, for the moment
# the input method cannot rank it first. With the mark pressed while the highlight
# sits on a candidate, that one character is committed and the rest of the input
# keeps composing. A value names one mark of the punctuation row -- exclamation,
# quote, paren_left, paren_right, comma, period, colon, semicolon, question, at,
# bracket_left or bracket_right -- and "off" hands every mark back to the
# application. Takes effect on reload.
select_first_char = "bracket_left"
select_last_char = "bracket_right"

[scheme]
# The layout the keystrokes follow: "full" is plain pinyin, and "xiaohe", "ziranma",
# "microsoft", "sogou" and "ziguang" are the double-pinyin layouts. Takes effect on
# reload: a new layout answers the next composition and never re-reads one in progress.
scheme = "full"
# Name the active layout in the candidate window's header. Takes effect on reload.
show_hint = true
# Still read a full-pinyin syllable typed while a double-pinyin layout is active.
# Takes effect on reload.
keep_full_pinyin = true

[shuangpin]
# A double-pinyin layout of your own, defined under [shuangpin.custom] below. The
# layout is data, not switches: it is compiled once per load and per reload, and a
# layout that fails any check is refused whole -- the layout selected under [scheme]
# keeps answering the keys, and each error is reported as its own config/invalid
# diagnostic. When the section names a working layout, it is the layout that answers
# the keys, whatever [scheme] selects. Takes effect on reload.

[shuangpin.custom]
# The layout, given either as a document named by `file`, or written out inline in
# the three tables below. An empty section defines no layout, and the double-pinyin
# layouts are exactly the built-in ones. Naming both makes the file the layout and
# reports the ambiguity.
#
# `file` is the layout document to read, as you spell the path: absolute, or relative
# to the directory this configuration file lives in.
#file = "my-layout.toml"
#
# The inline form: the display name, which of the two zero-initial schools the layout
# belongs to -- "first-letter" opens a vowel-initial syllable with its own letter
# (`an` is the `a` key then the `an` key), "o-led" opens it with a zero initial on
# `o` that contributes nothing -- and the three tables. A key maps to one initial, a
# space-separated list of finals tried in order, or a standalone syllable; a key name
# is one of a-z or `;`.
#name = "我的双拼"
#zero_initial_style = "first-letter"
#[shuangpin.custom.initials]
#zh = "v"
#ch = "i"
#sh = "u"
#[shuangpin.custom.finals]
#ang = "h"
#eng = "g"
#[shuangpin.custom.standalone]
#a = "a"

[fuzzy]
# Each key selects one fuzzy-equivalence class the decoder widens with: `zh_z` makes
# `zh` and `z` read as each other, `n_l` makes `n` and `l` do the same, and so on for
# every pair the class table defines. `enabled = true` selects every class at once;
# an individual `true` selects its class even with the master switch off. Takes
# effect on reload: the classes latch with the scheme, so a change answers the next
# composition and never re-reads one in progress.
enabled = false
zh_z = false
ch_c = false
sh_s = false
n_l = false
f_h = false
an_ang = false
en_eng = false
in_ing = false

[assoc]
# Offer next-word association candidates after a successful commit. Off by default:
# a new candidate window after every commit is an explicit opt-in. Takes effect on
# reload.
enabled = false
# How many association candidates one commit may offer, 1..=9. Takes effect on
# reload.
max_candidates = 5
# How many recent commits the in-session history source may retain, 0..=64. This
# memory dies with the session and is never written to disk. Takes effect on reload.
history_depth = 8

[english]
# How a commit an English candidate produces meets the text already on the screen:
# "auto" takes the one space the boundary between the two texts asks for -- Chinese
# into English and English into English -- and "off" commits every candidate exactly
# as it is spelled. The casing the candidate goes out in is not this key's to
# switch. Takes effect on reload.
spacing = "auto"

[quick_input]
# Recognise whole-buffer quick-input triggers such as the date family and the
# calculator. Off by default: a trigger that turns ordinary spelling into a domain is
# an explicit opt-in. Trigger recognition only; translators are not wired yet.
enabled = false
# Whole-buffer spellings that open the date family. Takes effect on reload.
date_prefixes = ["rq", "dt"]
# Recognise the calculator trigger. Off by default. Takes effect on reload.
calc_enabled = false

[phrases]
# Let the phrase table take part in a decode. With it off the dictionary answers alone.
# The table is read once, at start-up. Takes effect at the next fcitx5 restart.
enabled = true
# The phrase document to read. Empty means the default location under the user's
# configuration directory. Takes effect at the next fcitx5 restart.
file = ""
# How many entries the table may hold, 1..=50000. Takes effect at the next fcitx5
# restart.
max_entries = 5000

[data]
# "eventual" batches user-frequency writes; "immediate" flushes each one. The batch
# window is the store's construction-time policy. Takes effect at the next fcitx5
# restart.
durability = "eventual"
# Copy the user's learned words automatically. On by default: the store cannot be
# rebuilt from anywhere else. Takes effect at the next fcitx5 restart.
backup_enabled = true
# How many backup generations are kept, 1..=32. The oldest is removed once a new one
# has landed. Takes effect at the next fcitx5 restart.
backup_keep = 3
# The dictionary file the next reload swaps in whole: the reload validates the file
# completely, swaps it for the live one without touching the composition in progress,
# and clears this key. A file the validation rejects is reported as
# dict/swap/rejected and the key stands for you to fix. Empty means nothing to swap.
dict_staged_path = ""

[diagnostics]
# "error" | "warn" | "info" | "debug" | "trace". Takes effect on reload.
level = "info"
# When the log file reaches this many MiB it rolls over, and this many rolled files
# are kept. Takes effect on reload.

# Logical-clock half-life of user-word decay, in commits. The default matches the
# librime e-folding constant. Takes effect on reload.
decay_half_life_ticks = 139

# Smallest user-decay accumulator, in Q16.16; a record at or below it stops
# ranking. Takes effect on reload.
decay_floor = 65536
log_rotation_mb = 8
log_keep_files = 3
# Accepted, and deliberately without effect on what is recorded: the plugin never
# writes the characters you type to the log, whether this is false or true.
log_input_content = false
# Switch the diagnostic probes on. Read once, at start-up, with the configuration.
# Takes effect at the next fcitx5 restart.
probes = true
"##;
