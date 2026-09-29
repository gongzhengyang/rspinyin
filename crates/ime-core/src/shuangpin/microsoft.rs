//! The Microsoft scheme table.
//!
//! # Source
//!
//! `double_pinyin_mspy.schema.yaml` in the Rime double-pinyin schema package
//! (`github.com/rime/rime-double-pinyin`) and the copy of it that ships with rime-ice
//! (`github.com/iDvel/rime-ice`), cross-checked against the
//! `double_pinyin_mspy_sheng` / `double_pinyin_mspy_yun` arrays of libpinyin's
//! `src/storage/double_pinyin_table.h` (`github.com/libpinyin/libpinyin`). All three
//! agree on every key:
//!
//! - `iu` `q`, `ia`/`ua` `w`, `uan`/`üan` `r`, `üe` `t`, `uai`/`ü` `y`, `uo` `o`,
//!   `un`/`ün` `p`
//! - `ai` `l`, `en` `f`, `eng` `g`, `ang` `h`, `an` `j`, `ao` `k`, `iang`/`uang` `d`,
//!   `ian` `m`, `in` `n`, `ou` `b`
//! - `iao` `c`, `ui` `v`, `ie` `x`, `ong`/`iong` `s`, `ei` `z`
//! - `zh` `v`, `ch` `i`, `sh` `u`
//! - `ing` `;` -- the key that makes this layout need a 27th column
//!
//! A vowel-initial syllable takes the `o` key as its zero initial: `a` is `oa`, `ai`
//! is `ol`, `an` is `oj`, `ang` is `oh`, `ao` is `ok`, `e` is `oe`, `en` is `ow`,
//! `eng` is `og`, `o` is `oo`, `ou` is `ob`. The un-prefixed spelling is accepted too,
//! because Rime's `derive` rule keeps it, so `al` reaches `ai` as well.
//!
//! `üe` is reachable both on `t` and on `v`, which is the only key on which this
//! layout differs from Sogou's.
//!
//! # The `;` column
//!
//! This project's input buffer accepts ASCII letters and `'` only, so `;` cannot be
//! typed today and every `-ing` syllable is out of reach until that alphabet grows.
//! The table keeps the key rather than relocating `ing` onto `y`: `y` already carries
//! `ü` here, and folding the two together would cost `lü` and `nü`.

use ime_types::SchemeId;

use crate::shuangpin::SchemeTable;

/// Microsoft's scheme, the layout its pinyin input method ships.
#[rustfmt::skip]
pub const MICROSOFT: SchemeTable = SchemeTable {
    id: SchemeId::MICROSOFT,
    name: "微软双拼",
    // The zero initial is `o` here, so `o` opens a vowel-initial syllable and
    // contributes nothing to its spelling. `a` and `e` are kept as initials as well,
    // which is the un-prefixed spelling Rime's `derive` rule also accepts.
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
        // t                  u         v                            w                  x
        &["üe", "ue"],      &["u"],    &["ui", "üe", "ue"],        &["ia", "ua"],     &["ie"],
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

    use super::MICROSOFT;

    /// Maps `keys` and returns the spelling, failing the test when the keys do not map.
    fn spelled(keys: &[u8]) -> &'static str {
        let mapped = map_syllable(&MICROSOFT, keys)
            .expect("keys inside the alphabet")
            .expect("keys must map");
        assert_eq!(
            mapped.consumed,
            u8::try_from(keys.len()).expect("a short key list")
        );
        syllable_at(mapped.full).expect("a mapped syllable is in the table")
    }

    #[test]
    fn test_microsoft_golden_cases_match_the_published_layout() {
        let cases: [(&[u8], &str); 14] = [
            (b"vs", "zhong"),
            (b"is", "chong"),
            (b"uh", "shang"),
            (b"dl", "dai"),
            (b"d;", "ding"),
            (b"jt", "jue"),
            (b"jv", "jue"),
            (b"lt", "lüe"),
            (b"ly", "lü"),
            (b"dv", "dui"),
            (b"dz", "dei"),
            (b"dk", "dao"),
            (b"db", "dou"),
            (b"gy", "guai"),
        ];
        for (keys, expected) in cases {
            assert_eq!(
                spelled(keys),
                expected,
                "microsoft {:?}",
                core::str::from_utf8(keys)
            );
        }
    }

    #[test]
    fn test_microsoft_golden_cases_for_vowel_initial_syllables() {
        // The `o` prefix is this layout's rule; the un-prefixed spelling is accepted
        // beside it because Rime's `derive` rule keeps the original.
        let cases: [(&[u8], &str); 7] = [
            (b"oj", "an"),
            (b"ol", "ai"),
            (b"oo", "o"),
            (b"of", "en"),
            (b"oz", "ei"),
            (b"aa", "a"),
            (b"al", "ai"),
        ];
        for (keys, expected) in cases {
            assert_eq!(
                spelled(keys),
                expected,
                "microsoft {:?}",
                core::str::from_utf8(keys)
            );
        }
    }

    #[test]
    fn test_microsoft_carries_ing_on_the_semicolon_key() {
        // The key that makes this layout different from every other one this build
        // ships, and the reason the tables are indexed by 27 keys.
        let mapped = map_syllable(&MICROSOFT, b"d;")
            .expect("keys inside the alphabet")
            .expect("ding must map");
        assert_eq!(syllable_at(mapped.full), Some("ding"));
        assert_eq!(
            MICROSOFT.finals[crate::shuangpin::SEMICOLON_INDEX],
            &["ing"][..]
        );
    }

    #[test]
    fn test_microsoft_table_identifies_itself() {
        assert_eq!(MICROSOFT.id, SchemeId::MICROSOFT);
        assert_eq!(MICROSOFT.name, "微软双拼");
    }

    #[test]
    fn test_microsoft_rewrites_a_whole_word() {
        let scheme = SchemeId::MICROSOFT;
        let rewritten = rewrite_for_scheme("vsgo", scheme).expect("microsoft is implemented");
        assert_eq!(rewritten, "zhongguo");
    }
}
