//! The Sogou layout.
//!
//! # Source
//!
//! `data/sogou.toml` beside this file, whose source citations give the rime-ice
//! schema and the publisher's own rules the data was transcribed from.
//!
//! Sogou's own description of the layout states the two rules the data encodes: the
//! retroflex initials are on `v`, `i` and `u`, and a vowel-initial syllable is written
//! either with a leading `o` or by repeating its first letter.
//!
//! The layout is the same as Microsoft's on every key except one: `üe` here is reached
//! through `t` only, where Microsoft also accepts it on `v`. That is exactly the
//! difference between the two Rime schemas, and it is asserted by a test below.
//!
//! # The `;` column
//!
//! As with Microsoft's layout, `ing` sits on `;`, which this project's input buffer
//! cannot produce yet. See the module documentation of [`crate::shuangpin`] for why the
//! key is kept rather than folded onto another letter.

use ime_types::SchemeId;

use crate::shuangpin::SchemeTable;

/// The Sogou layout, as compiled from its embedded document.
///
/// # Panics
///
/// Never.
pub fn table() -> &'static SchemeTable {
    crate::shuangpin::builtin::table(SchemeId::SOGOU)
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
        // The `o` prefix is the layout's own rule; the repeated first letter is the
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
            map_syllable(table(), b"jv").expect("keys inside the alphabet"),
            None
        );
        let finals: Vec<&str> = table().finals[21]
            .iter()
            .map(|final_text| final_text.as_ref())
            .collect();
        assert_eq!(finals, ["ui"], "v carries ui alone in this layout");
    }

    #[test]
    fn test_sogou_table_identifies_itself() {
        assert_eq!(table().id, SchemeId::SOGOU);
        assert_eq!(table().name, "搜狗双拼");
    }

    #[test]
    fn test_sogou_rewrites_a_whole_word() {
        let scheme = SchemeId::SOGOU;
        let rewritten = rewrite_for_scheme("vsgo", scheme).expect("sogou is implemented");
        assert_eq!(rewritten, "zhongguo");
    }
}
