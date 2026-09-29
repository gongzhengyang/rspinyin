//! The pinyin syllable table, its lookup, and input normalization.
//!
//! Responsibility: answer "is this string one legal toneless pinyin syllable?"
//! and turn an arbitrary raw input string into the canonical spelling the table
//! is written in. Everything here is a pure function over borrowed data, and the
//! crate-internal `normalize_into` writes into a caller-owned buffer so the
//! segmentation DAG can normalize without allocating.
//!
//! Boundaries: this module knows nothing about segmentation, dictionaries or the
//! engine. It owns the syllable alphabet and the input alphabet (`a`-`z`, `'`,
//! and the umlaut forms of `ue`), and nothing else.
//!
//! # Canonical spelling
//!
//! The table uses standard pinyin orthography, which is also the spelling the
//! dictionary compiler reads from its `pinyin-data`-derived source: the umlaut is
//! written only after `l` and `n` (`lü`, `nü`, `lüe`, `nüe`), while the same sound
//! after `j`, `q`, `x` and `y` is written as a plain `u` (`ju`, `que`, `xuan`,
//! `yu`). Normalization therefore folds every input form of that sound onto the one
//! spelling the table stores: `v` becomes `ü`, and a `u` or `ü` typed after
//! `j`/`q`/`x`/`y` folds back to `u`. Both `lv` and `lü` reach `lü`, and both `jv`
//! and `ju` reach `ju`.

use ime_types::{DecodeError, SyllableId};

/// Hard upper bound, in bytes, on the raw input one decode request may carry.
pub const MAX_RAW_LEN: usize = 64;

/// Byte length of the longest syllable in the table (`zhuang`, `chuang`, `shuang`).
pub const MAX_SYLLABLE_LEN: usize = 6;

/// Number of entries in [`SYLLABLES`].
pub const SYLLABLE_COUNT: usize = 411;

/// Longest normalized string an input within [`MAX_RAW_LEN`] can produce.
///
/// Normalization never grows the input by more than a factor of two, and only the
/// one-byte `v` grows at all (into the two-byte `ü`), so a legal request always
/// fits in this many bytes.
pub const MAX_NORMALIZED_LEN: usize = MAX_RAW_LEN * 2;

/// Number of byte positions a normalized string can address, including the end.
pub const MAX_NODES: usize = MAX_NORMALIZED_LEN + 1;

/// How many discarded-input diagnostics [`DroppedChars`] retains.
pub const DROPPED_CAP: usize = 16;

/// The 411 toneless pinyin syllables, in ascending byte order.
///
/// This is the standard syllable table. Besides the ordinary syllables it carries
/// the interjection syllables that are legal input even though they have no
/// initial: `m`, `n`, `ng`, `hm`, `hng` and `ê`.
///
/// The order is `str` order, that is UTF-8 byte order, which is what
/// [`lookup`]'s binary search relies on. Because `ü` (U+00FC) and `ê` (U+00EA)
/// sort above every ASCII letter, the umlaut syllables sit inside their initial's
/// block (`luo` < `lü` < `lüe`) and `ê` is the final entry of the whole table.
// `#[rustfmt::skip]`: this is a data table, and grouping it by initial keeps it
// reviewable and keeps the file inside the project's line limit.
#[rustfmt::skip]
pub static SYLLABLES: &[&str] = &[
    // a (zero initial)
    "a", "ai", "an", "ang", "ao",
    // b
    "ba", "bai", "ban", "bang", "bao", "bei", "ben", "beng", "bi", "bian",
    "biao", "bie", "bin", "bing", "bo", "bu",
    // c and ch
    "ca", "cai", "can", "cang", "cao", "ce", "cen", "ceng", "cha", "chai",
    "chan", "chang", "chao", "che", "chen", "cheng", "chi", "chong", "chou",
    "chu", "chuai", "chuan", "chuang", "chui", "chun", "chuo", "ci", "cong",
    "cou", "cu", "cuan", "cui", "cun", "cuo",
    // d
    "da", "dai", "dan", "dang", "dao", "de", "dei", "deng", "di", "dian",
    "diao", "die", "ding", "diu", "dong", "dou", "du", "duan", "dui", "dun",
    "duo",
    // e (zero initial)
    "e", "ei", "en", "eng", "er",
    // f
    "fa", "fan", "fang", "fei", "fen", "feng", "fo", "fou", "fu",
    // g
    "ga", "gai", "gan", "gang", "gao", "ge", "gei", "gen", "geng", "gong",
    "gou", "gu", "gua", "guai", "guan", "guang", "gui", "gun", "guo",
    // h, including the interjections hm and hng
    "ha", "hai", "han", "hang", "hao", "he", "hei", "hen", "heng", "hm",
    "hng", "hong", "hou", "hu", "hua", "huai", "huan", "huang", "hui", "hun",
    "huo",
    // j
    "ji", "jia", "jian", "jiang", "jiao", "jie", "jin", "jing", "jiong", "jiu",
    "ju", "juan", "jue", "jun",
    // k
    "ka", "kai", "kan", "kang", "kao", "ke", "ken", "keng", "kong", "kou",
    "ku", "kua", "kuai", "kuan", "kuang", "kui", "kun", "kuo",
    // l
    "la", "lai", "lan", "lang", "lao", "le", "lei", "leng", "li", "lia",
    "lian", "liang", "liao", "lie", "lin", "ling", "liu", "lo", "long", "lou",
    "lu", "luan", "lun", "luo", "lü", "lüe",
    // m and the interjection m
    "m", "ma", "mai", "man", "mang", "mao", "me", "mei", "men", "meng", "mi",
    "mian", "miao", "mie", "min", "ming", "miu", "mo", "mou", "mu",
    // n and the interjections n and ng
    "n", "na", "nai", "nan", "nang", "nao", "ne", "nei", "nen", "neng", "ng",
    "ni", "nian", "niang", "niao", "nie", "nin", "ning", "niu", "nong", "nou",
    "nu", "nuan", "nuo", "nü", "nüe",
    // o (zero initial)
    "o", "ou",
    // p
    "pa", "pai", "pan", "pang", "pao", "pei", "pen", "peng", "pi", "pian",
    "piao", "pie", "pin", "ping", "po", "pou", "pu",
    // q
    "qi", "qia", "qian", "qiang", "qiao", "qie", "qin", "qing", "qiong", "qiu",
    "qu", "quan", "que", "qun",
    // r
    "ran", "rang", "rao", "re", "ren", "reng", "ri", "rong", "rou", "ru",
    "ruan", "rui", "run", "ruo",
    // s and sh
    "sa", "sai", "san", "sang", "sao", "se", "sen", "seng", "sha", "shai",
    "shan", "shang", "shao", "she", "shei", "shen", "sheng", "shi", "shou",
    "shu", "shua", "shuai", "shuan", "shuang", "shui", "shun", "shuo", "si",
    "song", "sou", "su", "suan", "sui", "sun", "suo",
    // t
    "ta", "tai", "tan", "tang", "tao", "te", "teng", "ti", "tian", "tiao",
    "tie", "ting", "tong", "tou", "tu", "tuan", "tui", "tun", "tuo",
    // w
    "wa", "wai", "wan", "wang", "wei", "wen", "weng", "wo", "wu",
    // x
    "xi", "xia", "xian", "xiang", "xiao", "xie", "xin", "xing", "xiong", "xiu",
    "xu", "xuan", "xue", "xun",
    // y
    "ya", "yan", "yang", "yao", "ye", "yi", "yin", "ying", "yo", "yong",
    "you", "yu", "yuan", "yue", "yun",
    // z and zh
    "za", "zai", "zan", "zang", "zao", "ze", "zei", "zen", "zeng", "zha",
    "zhai", "zhan", "zhang", "zhao", "zhe", "zhei", "zhen", "zheng", "zhi",
    "zhong", "zhou", "zhu", "zhua", "zhuai", "zhuan", "zhuang", "zhui", "zhun",
    "zhuo", "zi", "zong", "zou", "zu", "zuan", "zui", "zun", "zuo",
    // ê sorts above every ASCII initial, so it closes the table
    "ê",
];

/// Looks one already-normalized syllable up in the table.
///
/// The argument must be in canonical spelling (see the module documentation);
/// callers that start from raw user input normalize it first. The lookup is a
/// binary search over [`SYLLABLES`], so it costs about nine comparisons.
///
/// # Examples
///
/// ```
/// use ime_core::segment::syllable::lookup;
///
/// assert!(lookup("ni").is_some());
/// assert!(lookup("zhuang").is_some());
/// assert!(lookup("x").is_none());
/// ```
pub fn lookup(s: &str) -> Option<SyllableId> {
    SYLLABLES
        .binary_search(&s)
        .ok()
        .and_then(|index| u16::try_from(index).ok())
        .map(SyllableId::new)
}

/// Returns the table entry a [`SyllableId`] refers to.
///
/// The dictionary layer uses this to turn an identifier back into the key it has
/// to look a fallback single-character word up under. Returns `None` for an index
/// outside the table rather than panicking.
pub fn syllable_at(id: SyllableId) -> Option<&'static str> {
    SYLLABLES.get(usize::from(id.value())).copied()
}

/// Diagnostics for the input characters that normalization discarded.
///
/// The first [`DROPPED_CAP`] discarded characters are kept with their byte offset
/// in the raw input; any further ones are only counted, so that a pathological
/// input cannot make the report grow. The whole report is a fixed-size value,
/// which is what lets the segmentation layer surface `decode/invalid-char`
/// diagnostics without allocating on the decode path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DroppedChars {
    entries: [(char, u32); DROPPED_CAP],
    len: u8,
    overflow: u32,
}

impl Default for DroppedChars {
    fn default() -> Self {
        Self {
            entries: [('\0', 0); DROPPED_CAP],
            len: 0,
            overflow: 0,
        }
    }
}

impl DroppedChars {
    /// Returns the retained `(character, byte offset in the raw input)` pairs, in
    /// the order they were discarded.
    pub fn entries(&self) -> &[(char, u32)] {
        &self.entries[..usize::from(self.len)]
    }

    /// Returns how many discarded characters were not retained.
    pub fn overflow(&self) -> u32 {
        self.overflow
    }

    /// Returns how many characters normalization discarded in total.
    pub fn total(&self) -> u32 {
        u32::from(self.len).saturating_add(self.overflow)
    }

    /// Returns `true` when normalization discarded nothing.
    pub fn is_empty(&self) -> bool {
        self.len == 0 && self.overflow == 0
    }

    /// Returns the first discarded character as the contract's
    /// `decode/invalid-char` error, or `None` when nothing was discarded.
    pub fn first_error(&self) -> Option<DecodeError> {
        self.entries()
            .first()
            .map(|&(ch, at)| DecodeError::InvalidChar {
                ch,
                at: usize::try_from(at).unwrap_or(usize::MAX),
            })
    }

    /// Forgets every recorded character.
    pub fn clear(&mut self) {
        self.len = 0;
        self.overflow = 0;
    }

    /// Records one discarded character; beyond [`DROPPED_CAP`] only counts it.
    fn push(&mut self, ch: char, at: usize) {
        let slot = usize::from(self.len);
        if slot < DROPPED_CAP {
            self.entries[slot] = (ch, u32::try_from(at).unwrap_or(u32::MAX));
            self.len = self.len.saturating_add(1);
        } else {
            self.overflow = self.overflow.saturating_add(1);
        }
    }
}

/// The canonical form of one raw input string, plus the diagnostics for whatever
/// was discarded on the way there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedResult {
    /// The canonical spelling: lower-case, every input form of the umlaut folded
    /// onto the table's spelling, `'` kept as a forced syllable boundary, and
    /// every other character removed.
    pub text: String,
    /// The characters that were removed, for `decode/invalid-char` diagnostics.
    pub dropped: DroppedChars,
}

/// Normalizes `raw` into the spelling [`SYLLABLES`] is written in.
///
/// The rules, in the order they are applied:
///
/// 1. ASCII letters are lower-cased.
/// 2. `v` becomes `ü`; a `u` or `ü` written after `j`, `q`, `x` or `y` folds back
///    to `u`, so that all input forms of the umlaut sound reach the single
///    spelling the table stores.
/// 3. `'` is kept as a forced syllable boundary, except for the ones that carry no
///    information: a leading `'`, a trailing `'` and a doubled `'` are folded away.
/// 4. Every other character is discarded. The one exception is `ê`, which the table
///    carries as a syllable of its own and which is therefore kept: it is what makes
///    every table entry reachable from input.
///
/// Nothing here fails. The result is meant to be handed to the segmentation layer,
/// which reports `decode/no-path` when the canonical form is not segmentable; the
/// discarded characters are reported separately so that the caller can raise a
/// `decode/invalid-char` diagnostic without the normalization itself failing.
///
/// This variant allocates; the segmentation DAG calls `normalize_into` instead, the
/// in-place variant that performs no allocation at all.
///
/// # Examples
///
/// ```
/// use ime_core::segment::syllable::normalize;
///
/// let result = normalize("Ni3'Hao");
/// assert_eq!(result.text, "ni'hao");
/// assert_eq!(result.dropped.total(), 1);
/// assert_eq!(normalize("lv").text, "lü");
/// assert_eq!(normalize("jv").text, "ju");
/// ```
pub fn normalize(raw: &str) -> NormalizedResult {
    let mut text = String::with_capacity(raw.len());
    let mut dropped = DroppedChars::default();
    normalize_into(raw, &mut text, &mut dropped);
    NormalizedResult { text, dropped }
}

/// Normalizes `raw` into a caller-owned buffer, without allocating.
///
/// `out` is cleared first and then filled, so a caller that keeps one `String`
/// across keystrokes never reallocates: the buffer's capacity is reused. `dropped`
/// is cleared and refilled with the characters that were discarded, together with
/// their byte offsets in `raw`.
///
/// The result is never longer than `2 * raw.len()` bytes, because the only growth
/// rule is the one-byte `v` becoming the two-byte `ü`.
pub(crate) fn normalize_into(raw: &str, out: &mut String, dropped: &mut DroppedChars) {
    out.clear();
    dropped.clear();
    // Offset of the boundary marker currently held at the end of `out`, if any.
    // A marker is only kept provisionally: a trailing one is folded away below.
    let mut trailing_marker: Option<usize> = None;
    for (at, ch) in raw.char_indices() {
        match ch {
            '\'' => {
                if out.is_empty() || out.ends_with('\'') {
                    // A leading marker, or one that doubles the previous marker,
                    // adds no information: record it and drop it.
                    dropped.push(ch, at);
                } else {
                    out.push('\'');
                    trailing_marker = Some(at);
                }
            }
            'v' | 'V' | 'ü' => {
                push_umlaut(out, 'ü');
                trailing_marker = None;
            }
            'ê' => {
                out.push('ê');
                trailing_marker = None;
            }
            'a'..='z' | 'A'..='Z' => {
                out.push(ch.to_ascii_lowercase());
                trailing_marker = None;
            }
            _ => dropped.push(ch, at),
        }
    }
    if let Some(at) = trailing_marker {
        if out.ends_with('\'') {
            out.pop();
            dropped.push('\'', at);
        }
    }
}

/// Appends the umlaut sound in the spelling the table stores.
///
/// Standard orthography drops the umlaut after `j`, `q`, `x` and `y`, so the sound
/// is written `u` there and `ü` everywhere else.
fn push_umlaut(out: &mut String, umlaut: char) {
    let drops_umlaut = matches!(out.as_bytes().last(), Some(b'j' | b'q' | b'x' | b'y'));
    out.push(if drops_umlaut { 'u' } else { umlaut });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_syllables_table_has_exactly_411_entries() {
        assert_eq!(SYLLABLES.len(), SYLLABLE_COUNT);
        assert_eq!(SYLLABLE_COUNT, 411);
    }

    #[test]
    fn test_syllables_table_is_strictly_ascending_without_duplicates() {
        for pair in SYLLABLES.windows(2) {
            let (left, right) = (pair[0], pair[1]);
            assert!(
                left < right,
                "table is not strictly ascending around {left:?} / {right:?}"
            );
        }
    }

    #[test]
    fn test_syllables_longest_entry_is_six_bytes() {
        let longest = SYLLABLES.iter().map(|s| s.len()).max().unwrap_or(0);
        assert_eq!(longest, MAX_SYLLABLE_LEN);
        let six_byte = ["chuang", "shuang", "zhuang"];
        for entry in six_byte {
            assert!(SYLLABLES.contains(&entry), "{entry} must be in the table");
            assert_eq!(entry.len(), MAX_SYLLABLE_LEN);
        }
    }

    #[test]
    fn test_syllables_table_contains_the_required_special_syllables() {
        let required = concat!(
            "a o e ai ei ao ou an en ang eng er ",
            "yi ya ye yao you yan yin yang ying yong ",
            "wu wa wo wai wei wan wen wang weng ",
            "yu yue yuan yun m n ng hm hng ê",
        );
        for entry in required.split_whitespace() {
            assert!(
                SYLLABLES.contains(&entry),
                "required syllable {entry:?} is missing"
            );
        }
    }

    #[test]
    fn test_lookup_finds_every_table_entry_by_its_index() {
        for (index, entry) in SYLLABLES.iter().enumerate() {
            let id = lookup(entry);
            assert!(id.is_some(), "lookup missed table entry {entry:?}");
            let expected = u16::try_from(index).unwrap_or(u16::MAX);
            assert_eq!(id.map(SyllableId::value), Some(expected));
            assert_eq!(syllable_at(SyllableId::new(expected)), Some(*entry));
        }
    }

    #[test]
    fn test_lookup_rejects_input_that_is_not_a_syllable() {
        for raw in [
            "x", "z", "q", "i", "ü", "io", "zzz", "biang", "Ni", "ni'hao", "",
        ] {
            assert!(lookup(raw).is_none(), "{raw:?} must not be a syllable");
        }
    }

    #[test]
    fn test_syllable_at_rejects_an_index_beyond_the_table() {
        assert_eq!(syllable_at(SyllableId::new(SYLLABLE_COUNT as u16)), None);
        assert_eq!(syllable_at(SyllableId::new(u16::MAX)), None);
    }

    #[test]
    fn test_normalize_lowercases_ascii_letters_and_keeps_them_in_order() {
        assert_eq!(normalize("NiHao").text, "nihao");
        assert_eq!(normalize("ZHUANG").text, "zhuang");
        assert!(normalize("Nihao").dropped.is_empty());
    }

    #[test]
    fn test_normalize_folds_every_umlaut_input_form_onto_the_table_spelling() {
        let cases = [
            ("lv", "lü"),
            ("nv", "nü"),
            ("lve", "lüe"),
            ("nve", "nüe"),
            ("lü", "lü"),
            ("lu", "lu"),
            ("jv", "ju"),
            ("qv", "qu"),
            ("xv", "xu"),
            ("yv", "yu"),
            ("jü", "ju"),
            ("yüe", "yue"),
            ("jue", "jue"),
            ("yue", "yue"),
            ("xuan", "xuan"),
            ("juan", "juan"),
        ];
        for (raw, expected) in cases {
            let result = normalize(raw);
            assert_eq!(result.text, expected, "normalizing {raw:?}");
            assert!(
                lookup(&result.text).is_some(),
                "{raw:?} normalized to {expected:?}, which is not in the table"
            );
        }
    }

    #[test]
    fn test_normalize_keeps_the_non_ascii_syllable() {
        let result = normalize("ê");
        assert_eq!(result.text, "ê");
        assert!(result.dropped.is_empty());
        assert!(lookup(&result.text).is_some());
    }

    #[test]
    fn test_normalize_folds_apostrophes_that_carry_no_boundary_information() {
        let cases = [
            ("ni'hao", "ni'hao", 0),
            ("'ni", "ni", 1),
            ("ni'", "ni", 1),
            ("ni''hao", "ni'hao", 1),
            ("''", "", 2),
            ("'", "", 1),
            ("a''b", "a'b", 1),
            ("''ni''hao''", "ni'hao", 5),
        ];
        for (raw, expected, dropped) in cases {
            let result = normalize(raw);
            assert_eq!(result.text, expected, "normalizing {raw:?}");
            assert_eq!(result.dropped.total(), dropped, "dropped count for {raw:?}");
        }
    }

    #[test]
    fn test_normalize_drops_characters_outside_the_alphabet_with_offsets() {
        let result = normalize("ni3hao");
        assert_eq!(result.text, "nihao");
        assert_eq!(result.dropped.total(), 1);
        assert_eq!(result.dropped.entries(), &[('3', 2)]);

        let spaced = normalize("ni hao");
        assert_eq!(spaced.text, "nihao");
        assert_eq!(spaced.dropped.entries(), &[(' ', 2)]);

        let cjk = normalize("你好");
        assert_eq!(cjk.text, "");
        assert_eq!(cjk.dropped.total(), 2);
        assert_eq!(cjk.dropped.entries(), &[('你', 0), ('好', 3)]);
    }

    #[test]
    fn test_dropped_chars_reports_overflow_beyond_capacity() {
        let raw = "#".repeat(DROPPED_CAP + 4);
        let result = normalize(&raw);
        assert_eq!(result.text, "");
        assert_eq!(result.dropped.entries().len(), DROPPED_CAP);
        assert_eq!(result.dropped.overflow(), 4);
        assert_eq!(result.dropped.total(), (DROPPED_CAP + 4) as u32);
        assert!(!result.dropped.is_empty());
    }

    #[test]
    fn test_dropped_chars_first_error_maps_to_the_contract_error() {
        assert_eq!(normalize("nihao").dropped.first_error(), None);
        let expected = DecodeError::InvalidChar { ch: '3', at: 2 };
        assert_eq!(normalize("ni3hao").dropped.first_error(), Some(expected));
    }

    #[test]
    fn test_normalize_never_grows_input_beyond_twice_its_length() {
        let raw = "v".repeat(MAX_RAW_LEN);
        let result = normalize(&raw);
        assert_eq!(result.text.len(), MAX_NORMALIZED_LEN);
        assert!(result.text.len() <= 2 * raw.len());
    }

    #[test]
    fn test_normalize_into_reuses_the_buffer_capacity() {
        let mut out = String::with_capacity(MAX_NORMALIZED_LEN);
        let mut dropped = DroppedChars::default();
        let capacity = out.capacity();

        normalize_into("zhongguoxiangqi", &mut out, &mut dropped);
        assert_eq!(out, "zhongguoxiangqi");
        normalize_into("nihao", &mut out, &mut dropped);
        assert_eq!(out, "nihao");
        assert_eq!(out.capacity(), capacity);
        assert!(dropped.is_empty());
    }
}
