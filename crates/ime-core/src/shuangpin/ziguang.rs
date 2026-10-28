//! The Ziguang layout.
//!
//! # Source
//!
//! `data/ziguang.toml` beside this file, whose source citations give the libpinyin
//! table and the rime-ice schema the data was transcribed from. The two independent
//! sources agree on every key.
//!
//! This layout moves all three retroflex initials off the `v`, `i`, `u` positions
//! every other layout this build ships uses: `zh` is on `u`, `ch` on `a` and `sh` on
//! `i`. The `e` and `v` keys open no syllable, and the `c` key carries no final, so
//! `c` opens a syllable but cannot close one.
//!
//! A vowel-initial syllable takes the `o` key as its zero initial: `a` is `oa`, `ai`
//! is `op`, `an` is `or`, `ang` is `os`, `ao` is `oq`, `e` is `oe`, `en` is `ow`,
//! `eng` is `ot`, `er` is `oj`, `o` is `oo`, `ou` is `oz`.
//!
//! # The `;` column
//!
//! As with Microsoft's and Sogou's layouts, `ing` sits on `;`, which this project's
//! input buffer cannot produce yet. See the module documentation of
//! [`crate::shuangpin`] for why the key is kept rather than folded onto another letter.

use ime_types::SchemeId;

use crate::shuangpin::SchemeTable;

/// The Ziguang layout, as compiled from its embedded document.
///
/// # Panics
///
/// Never.
pub fn table() -> &'static SchemeTable {
    crate::shuangpin::builtin::table(SchemeId::ZIGUANG)
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
        let compiled = table();
        assert_eq!(compiled.initials[0].as_deref(), Some("ch"));
        assert_eq!(compiled.initials[8].as_deref(), Some("sh"));
        assert_eq!(compiled.initials[20].as_deref(), Some("zh"));
        assert_eq!(
            compiled.initials[4].as_deref(),
            None,
            "e is not an initial in this layout"
        );
        assert_eq!(
            compiled.initials[21].as_deref(),
            None,
            "v is not an initial in this layout"
        );
        assert_eq!(spelled(b"aa"), "cha");
    }

    #[test]
    fn test_ziguang_table_identifies_itself() {
        assert_eq!(table().id, SchemeId::ZIGUANG);
        assert_eq!(table().name, "紫光双拼");
    }

    #[test]
    fn test_ziguang_rewrites_a_whole_word() {
        let scheme = SchemeId::ZIGUANG;
        let rewritten = rewrite_for_scheme("uhgo", scheme).expect("ziguang is implemented");
        // `zhong` + `guo`: `u` `h` then `g` `o`.
        assert_eq!(rewritten, "zhongguo");
    }
}
