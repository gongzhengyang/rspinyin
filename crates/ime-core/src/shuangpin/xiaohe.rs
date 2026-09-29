//! The Xiaohe scheme table.
//!
//! # Source
//!
//! `double_pinyin_flypy.schema.yaml` in the Rime double-pinyin schema package
//! (`github.com/rime/rime-double-pinyin`), cross-checked against the
//! `double_pinyin_xhe_sheng` / `double_pinyin_xhe_yun` arrays of libpinyin's
//! `src/storage/double_pinyin_table.h` (`github.com/libpinyin/libpinyin`) -- the
//! double-pinyin implementation fcitx5 itself drives. The two agree on every key, and
//! both agree with the layout the scheme's own publisher distributes:
//!
//! - `iu` `q`, `ei` `w`, `uan`/`üan` `r`, `üe` `t`, `un`/`ün` `y`, `uo` `o`, `ie` `p`
//! - `ai` `d`, `en` `f`, `eng` `g`, `ang` `h`, `an` `j`, `ing`/`uai` `k`,
//!   `iang`/`uang` `l`, `ian` `m`, `iao` `n`, `ou` `z`
//! - `ia`/`ua` `x`, `ao` `c`, `ui`/`ü` `v`, `in` `b`, `ong`/`iong` `s`, `ong` `s`
//! - `zh` `v`, `ch` `i`, `sh` `u`
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
//! reachable through this scheme. Every other syllable the key can spell is.

use ime_types::SchemeId;

use crate::shuangpin::SchemeTable;

/// The Xiaohe scheme, the most widely used double-pinyin layout.
///
/// Its name is user-facing copy, so it is written the way the window shows it.
#[rustfmt::skip]
pub const XIAOHE: SchemeTable = SchemeTable {
    id: SchemeId::XIAOHE,
    name: "小鹤双拼",
    // The initials follow the keyboard's letter names where they exist: `zh` on `v`,
    // `ch` on `i`, `sh` on `u`. `a`, `e` and `o` are the zero initials this scheme
    // writes a vowel-initial syllable with -- `an` is `a` then the `an` key.
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
    // Where a key carries two finals the syllable table tells them apart: `k` is `uai`
    // before `g` and `ing` before `t`, and only one of the two joins into a syllable
    // either way. `üe` is listed before `ue` so that `lüe` and `nüe` keep their umlaut
    // while `jue`, `que`, `xue` and `yue` take the plain spelling.
    finals: [
        // a         b          c          d          e         f          g
        &["a"],     &["in"],   &["ao"],   &["ai"],   &["e"],   &["en"],   &["eng", "ng"],
        // h         i          j          k                  l                    m
        &["ang"],   &["i"],    &["an"],   &["uai", "ing"],   &["iang", "uang"],   &["ian"],
        // n         o                    p          q         r                  s
        &["iao"],   &["uo", "o"],        &["ie"],   &["iu"],  &["uan", "er"],    &["ong", "iong"],
        // t                  u         v                  w         x                  y
        &["üe", "ue"],      &["u"],    &["ü", "ui"],      &["ei"],  &["ia", "ua"],     &["un"],
        // z         ;
        &["ou"],    &[],
    ],
    // `a`, `e` and `o` are syllables in their own right. The other vowel-initial
    // syllables are pairs, so they carry no standalone entry.
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

    use super::XIAOHE;

    /// Maps `keys` and returns the spelling, failing the test when the keys do not map.
    fn spelled(keys: &[u8]) -> &'static str {
        let mapped = map_syllable(&XIAOHE, keys)
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
        assert_eq!(XIAOHE.id, SchemeId::XIAOHE);
        assert_eq!(XIAOHE.name, "小鹤双拼");
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
