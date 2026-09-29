//! The Ziguang scheme table.
//!
//! # Source
//!
//! The `double_pinyin_zgpy_sheng` / `double_pinyin_zgpy_yun` arrays of libpinyin's
//! `src/storage/double_pinyin_table.h` (`github.com/libpinyin/libpinyin`), cross-checked
//! against `double_pinyin_ziguang.schema.yaml` in the rime-ice schema collection
//! (`github.com/iDvel/rime-ice`). The two independent sources agree on every key:
//!
//! - `zh` `u`, `ch` `a`, `sh` `i` -- this layout moves all three retroflex initials
//!   off the `v`, `i`, `u` positions every other scheme this build ships uses
//! - `iu`/`er` `j`, `ei` `k`, `uan` `l`, `un` `m`, `üe`/`ui` `n`, `ou` `z`
//! - `ai` `p`, `ao` `q`, `an` `r`, `ang` `s`, `eng` `t`, `en` `w`
//! - `iao` `b`, `ie` `d`, `ian` `f`, `iang`/`uang` `g`, `ong`/`iong` `h`
//! - `ia`/`ua` `x`, `in`/`uai` `y`, `ü` `v`, `uo`/`o` `o`, `u` `u`, `i` `i`, `a` `a`,
//!   `e` `e`
//! - `ing` `;`
//!
//! A vowel-initial syllable takes the `o` key as its zero initial: `a` is `oa`, `ai`
//! is `op`, `an` is `or`, `ang` is `os`, `ao` is `oq`, `e` is `oe`, `en` is `ow`,
//! `eng` is `ot`, `er` is `oj`, `o` is `oo`, `ou` is `oz`.
//!
//! The `c` key carries no final at all, so `c` opens a syllable but cannot close one.
//!
//! # The `;` column
//!
//! As with Microsoft's and Sogou's layouts, `ing` sits on `;`, which this project's
//! input buffer cannot produce yet. See the module documentation of
//! [`crate::shuangpin`] for why the key is kept rather than folded onto another letter.

use ime_types::SchemeId;

use crate::shuangpin::SchemeTable;

/// The Ziguang scheme, the layout the Ziguang pinyin input method ships.
#[rustfmt::skip]
pub const ZIGUANG: SchemeTable = SchemeTable {
    id: SchemeId::ZIGUANG,
    name: "紫光双拼",
    initials: [
        // a          b          c          d          e          f          g
        Some("ch"),  Some("b"), Some("c"), Some("d"), None,      Some("f"), Some("g"),
        // h          i          j          k          l          m          n
        Some("h"),   Some("sh"), Some("j"), Some("k"), Some("l"), Some("m"), Some("n"),
        // o          p          q          r          s          t          u
        Some(""),    Some("p"), Some("q"), Some("r"), Some("s"), Some("t"), Some("zh"),
        // v          w          x          y          z          ;
        None,        Some("w"), Some("x"), Some("y"), Some("z"), None,
    ],
    finals: [
        // a         b          c      d          e         f          g
        &["a"],     &["iao"],   &[],   &["ie"],   &["e"],   &["ian"],  &["iang", "uang"],
        // h                   i          j                  k          l          m
        &["ong", "iong"],    &["i"],    &["er", "iu"],     &["ei"],   &["uan"],  &["un"],
        // n                            o                    p          q          r
        &["üe", "ue", "ui"],          &["uo", "o"],        &["ai"],   &["ao"],   &["an"],
        // s         t                    u         v         w          x                  y
        &["ang"],   &["eng", "ng"],     &["u"],    &["ü"],    &["en"],   &["ia", "ua"],     &["in", "uai"],
        // z         ;
        &["ou"],    &["ing"],
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

    use super::ZIGUANG;

    /// Maps `keys` and returns the spelling, failing the test when the keys do not map.
    fn spelled(keys: &[u8]) -> &'static str {
        let mapped = map_syllable(&ZIGUANG, keys)
            .expect("keys inside the alphabet")
            .expect("keys must map");
        assert_eq!(
            mapped.consumed,
            u8::try_from(keys.len()).expect("a short key list")
        );
        syllable_at(mapped.full).expect("a mapped syllable is in the table")
    }

    #[test]
    fn test_ziguang_golden_cases_match_the_published_layout() {
        // The first three cases pin the retroflex initials, which this layout puts on
        // `u`, `a` and `i` rather than on `v`, `i` and `u`.
        let cases: [(&[u8], &str); 14] = [
            (b"uh", "zhong"),
            (b"ai", "chi"),
            (b"ui", "zhi"),
            (b"ii", "shi"),
            (b"ip", "shai"),
            (b"aq", "chao"),
            (b"at", "cheng"),
            (b"jj", "jiu"),
            (b"gx", "gua"),
            (b"d;", "ding"),
            (b"jd", "jie"),
            (b"gy", "guai"),
            (b"jn", "jue"),
            (b"lv", "lü"),
        ];
        for (keys, expected) in cases {
            assert_eq!(
                spelled(keys),
                expected,
                "ziguang {:?}",
                core::str::from_utf8(keys)
            );
        }
    }

    #[test]
    fn test_ziguang_golden_cases_for_vowel_initial_syllables() {
        // Every one of them is the `o` key plus the final's key; this layout has no
        // repeated-letter spelling, because `a` is `ch` and `e` is not an initial.
        let cases: [(&[u8], &str); 7] = [
            (b"or", "an"),
            (b"op", "ai"),
            (b"oo", "o"),
            (b"ow", "en"),
            (b"oj", "er"),
            (b"ot", "eng"),
            (b"oz", "ou"),
        ];
        for (keys, expected) in cases {
            assert_eq!(
                spelled(keys),
                expected,
                "ziguang {:?}",
                core::str::from_utf8(keys)
            );
        }
    }

    #[test]
    fn test_ziguang_moves_the_retroflex_initials_off_v_i_u() {
        // The property that tells this layout apart from every other scheme this build
        // ships: `ch` is on `a`, `sh` on `i` and `zh` on `u`, so `a` repeated is `cha`
        // and not the vowel-initial `a` the other schemes spell that way.
        assert_eq!(ZIGUANG.initials[0], Some("ch"));
        assert_eq!(ZIGUANG.initials[8], Some("sh"));
        assert_eq!(ZIGUANG.initials[20], Some("zh"));
        assert_eq!(
            ZIGUANG.initials[4], None,
            "e is not an initial in this layout"
        );
        assert_eq!(
            ZIGUANG.initials[21], None,
            "v is not an initial in this layout"
        );
        assert_eq!(spelled(b"aa"), "cha");
    }

    #[test]
    fn test_ziguang_table_identifies_itself() {
        assert_eq!(ZIGUANG.id, SchemeId::ZIGUANG);
        assert_eq!(ZIGUANG.name, "紫光双拼");
    }

    #[test]
    fn test_ziguang_rewrites_a_whole_word() {
        let scheme = SchemeId::ZIGUANG;
        let rewritten = rewrite_for_scheme("uhgo", scheme).expect("ziguang is implemented");
        // `zhong` + `guo`: `u` `h` then `g` `o`.
        assert_eq!(rewritten, "zhongguo");
    }
}
