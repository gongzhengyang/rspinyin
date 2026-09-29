//! The Ziranma scheme table.
//!
//! # Source
//!
//! `double_pinyin.schema.yaml` in the Rime double-pinyin schema package
//! (`github.com/rime/rime-double-pinyin`), cross-checked against the
//! `double_pinyin_zrm_sheng` / `double_pinyin_zrm_yun` arrays of libpinyin's
//! `src/storage/double_pinyin_table.h` (`github.com/libpinyin/libpinyin`). The two
//! agree on every key:
//!
//! - `iu` `q`, `ia`/`ua` `w`, `uan`/`üan` `r`, `üe` `t`, `uai`/`ing` `y`, `uo` `o`,
//!   `un`/`ün` `p`
//! - `ai` `l`, `en` `f`, `eng` `g`, `ang` `h`, `an` `j`, `ao` `k`, `iang`/`uang` `d`,
//!   `ian` `m`, `in` `n`, `ou` `b`
//! - `ia`/`ua` `w`, `iao` `c`, `ui`/`ü` `v`, `ie` `x`, `ong`/`iong` `s`, `ei` `z`
//! - `zh` `v`, `ch` `i`, `sh` `u`
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
//! reachable through this scheme.

use ime_types::SchemeId;

use crate::shuangpin::SchemeTable;

/// The Ziranma scheme, the layout most later schemes were derived from.
#[rustfmt::skip]
pub const ZIRANMA: SchemeTable = SchemeTable {
    id: SchemeId::ZIRANMA,
    name: "自然码",
    // `zh` on `v`, `ch` on `i`, `sh` on `u`, as in every scheme this build ships.
    // `a`, `e` and `o` are the zero initials a vowel-initial syllable is written with.
    initials: [
        // a          b          c          d          e          f          g
        Some("a"),   Some("b"), Some("c"), Some("d"), Some("e"), Some("f"), Some("g"),
        // h          i          j          k          l          m          n
        Some("h"),   Some("ch"), Some("j"), Some("k"), Some("l"), Some("m"), Some("n"),
        // o          p          q          r          s          t          u
        Some("o"),   Some("p"), Some("q"), Some("r"), Some("s"), Some("t"), Some("sh"),
        // v          w          x          y          z          ;
        Some("zh"),  Some("w"), Some("x"), Some("y"), Some("z"), None,
    ],
    finals: [
        // a         b          c          d                    e         f          g
        &["a"],     &["ou"],   &["iao"],  &["uang", "iang"],   &["e"],   &["en"],   &["eng", "ng"],
        // h         i          j          k          l          m          n
        &["ang"],   &["i"],    &["an"],   &["ao"],   &["ai"],   &["ian"],  &["in"],
        // o                    p          q         r                  s
        &["uo", "o"],        &["un"],   &["iu"],  &["uan", "er"],    &["ong", "iong"],
        // t                  u         v                  w                  x
        &["üe", "ue"],      &["u"],    &["ü", "ui"],      &["ia", "ua"],     &["ie"],
        // y                  z          ;
        &["uai", "ing"],    &["ei"],   &[],
    ],
    standalone: [
        // a     b     c     d     e     f     g
        "a",    "",   "",   "",   "e",  "",   "",
        // h     i     j     k     l     m     n
        "",     "",   "",   "",   "",   "",   "",
        // o     p     q     r     s     t     u
        "o",    "",   "",   "",   "",   "",   "",
        // v     w     x     y     z     ;
        "",     "",   "",   "",   "",   "",
    ],
};

#[cfg(test)]
mod tests {
    use ime_types::SchemeId;

    use crate::segment::syllable_at;
    use crate::shuangpin::{map_syllable, rewrite_for_scheme};

    use super::ZIRANMA;

    /// Maps `keys` and returns the spelling, failing the test when the keys do not map.
    fn spelled(keys: &[u8]) -> &'static str {
        let mapped = map_syllable(&ZIRANMA, keys)
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
        // future edit to the table cannot quietly turn one scheme into the other.
        assert_eq!(spelled(b"yy"), "ying");
        assert_eq!(spelled(b"ll"), "lai");
    }

    #[test]
    fn test_ziranma_table_identifies_itself() {
        assert_eq!(ZIRANMA.id, SchemeId::ZIRANMA);
        assert_eq!(ZIRANMA.name, "自然码");
    }

    #[test]
    fn test_ziranma_rewrites_a_whole_word() {
        let scheme = SchemeId::ZIRANMA;
        let rewritten = rewrite_for_scheme("vsgo", scheme).expect("ziranma is implemented");
        // `zhong` + `guo`: `v` `s` then `g` `o`.
        assert_eq!(rewritten, "zhongguo");
    }
}
