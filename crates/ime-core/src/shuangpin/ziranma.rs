//! The Ziranma layout.
//!
//! # Source
//!
//! `data/ziranma.toml` beside this file, whose source citations give the Rime
//! schema and the libpinyin table the data was transcribed from. The layout most
//! later schemes were derived from.
//!
//! A vowel-initial syllable is written as its own first letter followed by the final's
//! key, the same rule Xiaohe uses: `a` is `aa`, `ai` is `al`, `an` is `aj`, `ang` is
//! `ah`, `ao` is `ak`, `e` is `ee`, `en` is `ef`, `eng` is `eg`, `o` is `oo`, `ou` is
//! `ob`. `er` keeps its two letters, since no key carries it.
//!
//! # Known limitation
//!
//! `uo` and `o` share the `o` key, and the syllable table holds both `luo` and `lo`.
//! The mapping resolves the pair to `luo`, the far more common reading, so `lo` is not
//! reachable through this layout.

use ime_types::SchemeId;

use crate::shuangpin::SchemeTable;

/// The Ziranma layout, as compiled from its embedded document.
///
/// # Panics
///
/// Never.
pub fn table() -> &'static SchemeTable {
    crate::shuangpin::builtin::table(SchemeId::ZIRANMA)
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
    fn test_ziranma_golden_cases_match_the_published_layout() {
        // The keys that differ from Xiaohe are the interesting ones: `l` is `ai` here
        // and `d` is `ai` there, and `ing` sits on `y` rather than on `k`.
        let cases: [(&[u8], &str); 14] = [
            (b"vs", "zhong"),
            (b"is", "chong"),
            (b"uh", "shang"),
            (b"dl", "dai"),
            (b"dk", "dao"),
            (b"db", "dou"),
            (b"dz", "dei"),
            (b"jd", "jiang"),
            (b"gd", "guang"),
            (b"dy", "ding"),
            (b"jt", "jue"),
            (b"lt", "lüe"),
            (b"dv", "dui"),
            (b"lv", "lü"),
        ];
        for (keys, expected) in cases {
            assert_eq!(
                spelled(keys),
                expected,
                "ziranma {:?}",
                core::str::from_utf8(keys)
            );
        }
    }

    #[test]
    fn test_ziranma_golden_cases_for_vowel_initial_syllables() {
        let cases: [(&[u8], &str); 9] = [
            (b"aa", "a"),
            (b"al", "ai"),
            (b"aj", "an"),
            (b"ah", "ang"),
            (b"ak", "ao"),
            (b"ee", "e"),
            (b"oo", "o"),
            (b"ob", "ou"),
            (b"er", "er"),
        ];
        for (keys, expected) in cases {
            assert_eq!(
                spelled(keys),
                expected,
                "ziranma {:?}",
                core::str::from_utf8(keys)
            );
        }
    }

    #[test]
    fn test_ziranma_puts_ing_on_y_and_ai_on_l() {
        // The two keys that tell Ziranma apart from Xiaohe, asserted on their own so a
        // future edit to the table cannot quietly turn one layout into the other.
        assert_eq!(spelled(b"yy"), "ying");
        assert_eq!(spelled(b"ll"), "lai");
    }

    #[test]
    fn test_ziranma_table_identifies_itself() {
        assert_eq!(table().id, SchemeId::ZIRANMA);
        assert_eq!(table().name, "自然码");
    }

    #[test]
    fn test_ziranma_rewrites_a_whole_word() {
        let scheme = SchemeId::ZIRANMA;
        let rewritten = rewrite_for_scheme("vsgo", scheme).expect("ziranma is implemented");
        // `zhong` + `guo`: `v` `s` then `g` `o`.
        assert_eq!(rewritten, "zhongguo");
    }
}
