//! The Sogou scheme table.
//!
//! # Source
//!
//! `double_pinyin_sogou.schema.yaml` in the rime-ice schema collection
//! (`github.com/iDvel/rime-ice`), which is the maintained Rime schema for this layout.
//! Sogou's own description of the scheme family (`pinyin.sogou.com/help.php`) states
//! the two rules the table encodes: the retroflex initials are on `v`, `i` and `u`,
//! and a vowel-initial syllable is written either with a leading `o` or by repeating
//! its first letter.
//!
//! The layout is the same as Microsoft's on every key except one: `üe` here is reached
//! through `t` only, where Microsoft also accepts it on `v`. That is exactly the
//! difference between the two Rime schemas, and it is asserted by a test below.
//!
//! - `iu` `q`, `ia`/`ua` `w`, `uan`/`üan` `r`, `üe` `t`, `uai`/`ü` `y`, `uo` `o`,
//!   `un`/`ün` `p`
//! - `ai` `l`, `en` `f`, `eng` `g`, `ang` `h`, `an` `j`, `ao` `k`, `iang`/`uang` `d`,
//!   `ian` `m`, `in` `n`, `ou` `b`
//! - `iao` `c`, `ui` `v`, `ie` `x`, `ong`/`iong` `s`, `ei` `z`
//! - `zh` `v`, `ch` `i`, `sh` `u`
//! - `ing` `;`
//!
//! # The `;` column
//!
//! As with Microsoft's layout, `ing` sits on `;`, which this project's input buffer
//! cannot produce yet. See the module documentation of [`crate::shuangpin`] for why the
//! key is kept rather than folded onto another letter.

use ime_types::SchemeId;

use crate::shuangpin::SchemeTable;

/// Sogou's scheme, the layout its input method ships as the default.
#[rustfmt::skip]
pub const SOGOU: SchemeTable = SchemeTable {
    id: SchemeId::SOGOU,
    name: "搜狗双拼",
    initials: [
        // a          b          c          d          e          f          g
        Some("a"),   Some("b"), Some("c"), Some("d"), Some("e"), Some("f"), Some("g"),
        // h          i          j          k          l          m          n
        Some("h"),   Some("ch"), Some("j"), Some("k"), Some("l"), Some("m"), Some("n"),
        // o          p          q          r          s          t          u
        Some(""),    Some("p"), Some("q"), Some("r"), Some("s"), Some("t"), Some("sh"),
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
        // t                  u         v          w                  x
        &["üe", "ue"],      &["u"],    &["ui"],   &["ia", "ua"],     &["ie"],
        // y                  z          ;
        &["uai", "ü"],      &["ei"],   &["ing"],
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

    use super::SOGOU;

    /// Maps `keys` and returns the spelling, failing the test when the keys do not map.
    fn spelled(keys: &[u8]) -> &'static str {
        let mapped = map_syllable(&SOGOU, keys)
            .expect("keys inside the alphabet")
            .expect("keys must map");
        assert_eq!(
            mapped.consumed,
            u8::try_from(keys.len()).expect("a short key list")
        );
        syllable_at(mapped.full).expect("a mapped syllable is in the table")
    }

    #[test]
    fn test_sogou_golden_cases_match_the_published_layout() {
        let cases: [(&[u8], &str); 14] = [
            (b"vs", "zhong"),
            (b"is", "chong"),
            (b"uh", "shang"),
            (b"dl", "dai"),
            (b"d;", "ding"),
            (b"jt", "jue"),
            (b"lt", "lüe"),
            (b"ly", "lü"),
            (b"dv", "dui"),
            (b"dz", "dei"),
            (b"dk", "dao"),
            (b"db", "dou"),
            (b"gy", "guai"),
            (b"jd", "jiang"),
        ];
        for (keys, expected) in cases {
            assert_eq!(
                spelled(keys),
                expected,
                "sogou {:?}",
                core::str::from_utf8(keys)
            );
        }
    }

    #[test]
    fn test_sogou_accepts_both_spellings_of_a_vowel_initial_syllable() {
        // The `o` prefix is the scheme's own rule; the repeated first letter is the
        // other spelling Sogou's own documentation gives.
        let cases: [(&[u8], &str); 8] = [
            (b"oj", "an"),
            (b"aj", "an"),
            (b"ol", "ai"),
            (b"oo", "o"),
            (b"aa", "a"),
            (b"of", "en"),
            (b"ef", "en"),
            (b"oz", "ei"),
        ];
        for (keys, expected) in cases {
            assert_eq!(
                spelled(keys),
                expected,
                "sogou {:?}",
                core::str::from_utf8(keys)
            );
        }
    }

    #[test]
    fn test_sogou_leaves_ue_off_the_v_key_unlike_microsoft() {
        // The single key on which this layout differs from Microsoft's: `v` carries
        // `ui` here and nothing else, so `jv` is not `jue`.
        assert_eq!(
            map_syllable(&SOGOU, b"jv").expect("keys inside the alphabet"),
            None
        );
        assert_eq!(
            SOGOU.finals[21],
            &["ui"][..],
            "v carries ui alone in this layout"
        );
    }

    #[test]
    fn test_sogou_table_identifies_itself() {
        assert_eq!(SOGOU.id, SchemeId::SOGOU);
        assert_eq!(SOGOU.name, "搜狗双拼");
    }

    #[test]
    fn test_sogou_rewrites_a_whole_word() {
        let scheme = SchemeId::SOGOU;
        let rewritten = rewrite_for_scheme("vsgo", scheme).expect("sogou is implemented");
        assert_eq!(rewritten, "zhongguo");
    }
}
