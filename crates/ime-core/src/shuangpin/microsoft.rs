//! The Microsoft layout.
//!
//! # Source
//!
//! `data/microsoft.toml` beside this file, whose source citations give the Rime
//! schemas and the libpinyin table the data was transcribed from.
//!
//! A vowel-initial syllable takes the `o` key as its zero initial: `a` is `oa`, `ai`
//! is `ol`, `an` is `oj`, `ang` is `oh`, `ao` is `ok`, `e` is `oe`, `en` is `of`,
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

/// The Microsoft layout, as compiled from its embedded document.
///
/// # Panics
///
/// Never.
pub fn table() -> &'static SchemeTable {
    crate::shuangpin::builtin::table(SchemeId::MICROSOFT)
}

#[cfg(test)]
mod tests {
    use ime_types::SchemeId;

    use crate::segment::syllable_at;
    use crate::shuangpin::{SEMICOLON_INDEX, map_syllable, rewrite_for_scheme};

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
        let mapped = map_syllable(table(), b"d;")
            .expect("keys inside the alphabet")
            .expect("ding must map");
        assert_eq!(syllable_at(mapped.full), Some("ding"));
        let finals: Vec<&str> = table().finals[SEMICOLON_INDEX]
            .iter()
            .map(|final_text| final_text.as_ref())
            .collect();
        assert_eq!(finals, ["ing"]);
    }

    #[test]
    fn test_microsoft_table_identifies_itself() {
        assert_eq!(table().id, SchemeId::MICROSOFT);
        assert_eq!(table().name, "微软双拼");
    }

    #[test]
    fn test_microsoft_rewrites_a_whole_word() {
        let scheme = SchemeId::MICROSOFT;
        let rewritten = rewrite_for_scheme("vsgo", scheme).expect("microsoft is implemented");
        assert_eq!(rewritten, "zhongguo");
    }
}
