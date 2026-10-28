//! The Xiaohe layout.
//!
//! # Source
//!
//! `data/xiaohe.toml` beside this file, whose source citations give the Rime
//! schema and the libpinyin table the data was transcribed from. This module is
//! the accessor the rest of the crate and the tests read the compiled table
//! through.
//!
//! A vowel-initial syllable is written as its own first letter followed by the final's
//! key: `a` is `aa`, `ai` is `ad`, `an` is `aj`, `ang` is `ah`, `ao` is `ac`, `e` is
//! `ee`, `en` is `ef`, `eng` is `eg`, `o` is `oo`, `ou` is `oz`. `er` keeps its two
//! letters, since no key carries it.
//!
//! # Known limitation
//!
//! `uo` and `o` share the `o` key, and the syllable table holds both `luo` and `lo`.
//! The mapping resolves the pair to `luo`, the far more common reading, so `lo` is not
//! reachable through this layout. Every other syllable the key can spell is.

use ime_types::SchemeId;

use crate::shuangpin::SchemeTable;

/// The Xiaohe layout, the most widely used double-pinyin layout, as compiled from
/// its embedded document.
///
/// # Panics
///
/// Never.
pub fn table() -> &'static SchemeTable {
    crate::shuangpin::builtin::table(SchemeId::XIAOHE)
}

#[cfg(test)]
mod tests {
    use ime_types::SchemeId;

    use crate::segment::syllable_at;
    use crate::shuangpin::{map_syllable, rewrite_for_scheme};

    use super::table;

    /// Maps `keys` and returns the spelling, failing the test when the keys do not map.
    fn spelled(keys: &[u8]) -> &'static str {
        let mapped = map_syllable(table(), keys)
            .expect("keys inside the alphabet")
            .expect("keys must map");
        assert_eq!(
            mapped.consumed,
            u8::try_from(keys.len()).expect("a short key list")
        );
        syllable_at(mapped.full).expect("a mapped syllable is in the table")
    }

    #[test]
    fn test_xiaohe_golden_cases_match_the_published_layout() {
        // One case per initial whose key is not the letter's own name, plus the finals
        // the layout is famous for.
        let cases: [(&[u8], &str); 14] = [
            (b"vs", "zhong"),
            (b"is", "chong"),
            (b"uh", "shang"),
            (b"dd", "dai"),
            (b"xn", "xiao"),
            (b"bb", "bin"),
            (b"lv", "lü"),
            (b"jt", "jue"),
            (b"lt", "lüe"),
            (b"gy", "gun"),
            (b"gk", "guai"),
            (b"tk", "ting"),
            (b"zz", "zou"),
            (b"yy", "yun"),
        ];
        for (keys, expected) in cases {
            assert_eq!(
                spelled(keys),
                expected,
                "xiaohe {:?}",
                core::str::from_utf8(keys)
            );
        }
    }

    #[test]
    fn test_xiaohe_golden_cases_for_vowel_initial_syllables() {
        // The vowel-initial syllables repeat the first letter rather than prefixing it.
        let cases: [(&[u8], &str); 8] = [
            (b"aa", "a"),
            (b"ad", "ai"),
            (b"aj", "an"),
            (b"ah", "ang"),
            (b"ac", "ao"),
            (b"ee", "e"),
            (b"oo", "o"),
            (b"oz", "ou"),
        ];
        for (keys, expected) in cases {
            assert_eq!(
                spelled(keys),
                expected,
                "xiaohe {:?}",
                core::str::from_utf8(keys)
            );
        }
    }

    #[test]
    fn test_xiaohe_er_keeps_its_own_spelling() {
        assert_eq!(spelled(b"er"), "er");
    }

    #[test]
    fn test_xiaohe_table_identifies_itself() {
        assert_eq!(table().id, SchemeId::XIAOHE);
        assert_eq!(table().name, "小鹤双拼");
    }

    #[test]
    fn test_xiaohe_rewrites_the_two_examples_the_scheme_is_known_for() {
        let scheme = SchemeId::XIAOHE;
        let rewritten = rewrite_for_scheme("vs", scheme).expect("xiaohe is implemented");
        assert_eq!(rewritten, "zhong");
        let rewritten = rewrite_for_scheme("nihc", scheme).expect("xiaohe is implemented");
        assert_eq!(rewritten, "nihao");
    }
}
