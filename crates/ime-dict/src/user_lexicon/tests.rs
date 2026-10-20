//! Tests for the user lexicon and the chain that puts it ahead of the system
//! dictionary.

use std::collections::BTreeMap;

use ime_types::{ImeError, Lexicon, SyllableId, WordFlags, WordIter, WordRef};

use super::{COINED_BASE_WEIGHT, ChainedLexicon, UserLexicon};

/// A back lexicon a test fills by hand: the same shape the system dictionary answers
/// with, and nothing more.
#[derive(Default)]
struct TableLexicon {
    words: BTreeMap<String, Vec<WordRef<'static>>>,
}

impl TableLexicon {
    /// Adds one word under `key`, spilling the text into the table's own storage.
    fn with(mut self, key: &str, text: &str, weight: u32) -> Self {
        let spilled: &'static str = Box::leak(Box::from(text));
        self.words.entry(String::from(key)).or_default().push(WordRef {
            text: spilled,
            weight,
            syl_count: u8::try_from(key.split('\'').count()).unwrap_or(1),
            flags: WordFlags::empty(),
        });
        self
    }
}

impl Lexicon for TableLexicon {
    fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
        Ok(WordIter::from_slice(
            self.words.get(key).map(Vec::as_slice).unwrap_or(&[]),
        ))
    }

    fn prefix(&self, _prefix: &str, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Err(ImeError::Unsupported)
    }

    fn fallback_single(&self, _syl: SyllableId, _limit: usize) -> Result<WordIter<'_>, ImeError> {
        Ok(WordIter::from_slice(&[]))
    }
}

/// The texts of one lookup, in order.
fn texts(words: WordIter<'_>) -> Vec<String> {
    words.map(|word| word.text.to_string()).collect()
}

#[test]
fn test_coin_enrols_a_word_and_confirms_it_with_the_next_commit() {
    let mut lexicon = UserLexicon::new();
    assert!(lexicon.coin("zhu'ji", "诸暨"), "the first coin is new");
    assert!(!lexicon.coin("zhu'ji", "诸暨"), "the second confirms");
    let words = lexicon.lookup("zhu'ji").expect("the key serves");
    let words: Vec<_> = words.collect();
    assert_eq!(words.len(), 1, "one entry, not two");
    assert_eq!(words[0].text, "诸暨");
    assert!(
        words[0].flags.contains(WordFlags::USER),
        "the entry carries the user flag, which is what the label reads"
    );
}

#[test]
fn test_chain_puts_the_coined_word_ahead_of_every_system_word_of_its_key() {
    let mut front = UserLexicon::new();
    front.coin("zhu'ji", "诸暨");
    // The system side holds a strong word under the same key -- the strongest the key
    // has -- and the coined entry must come out above it.
    let back = TableLexicon::default()
        .with("zhu'ji", "主机", COINED_BASE_WEIGHT * 2)
        .with("zhu'ji", "主级", 100);
    let chain = ChainedLexicon::new(&front, &back);

    let words: Vec<_> = chain
        .lookup("zhu'ji")
        .expect("the chain answers")
        .collect();
    let spellings: Vec<String> = words.iter().map(|word| word.text.to_string()).collect();
    assert_eq!(spellings, vec!["诸暨", "主机", "主级"]);
    assert!(
        words[0].weight > words[1].weight,
        "the coined entry outranks the heaviest system word: {} vs {}",
        words[0].weight,
        words[1].weight
    );
}

#[test]
fn test_chain_merges_a_system_word_the_user_also_coined_into_one_entry() {
    // The system dictionary already has the word the user coined: the merged answer is
    // one candidate -- the user's, at the raised weight -- not the same text twice.
    let mut front = UserLexicon::new();
    front.coin("zhu'ji", "主机");
    let back = TableLexicon::default().with("zhu'ji", "主机", COINED_BASE_WEIGHT * 2);
    let chain = ChainedLexicon::new(&front, &back);

    let words = chain.lookup("zhu'ji").expect("the chain answers");
    let words: Vec<_> = words.collect();
    assert_eq!(words.len(), 1, "the two tables offer one candidate");
    assert!(words[0].flags.contains(WordFlags::USER));
}

#[test]
fn test_chain_with_nothing_coined_answers_the_system_dictionary_directly() {
    let front = UserLexicon::new();
    let back = TableLexicon::default().with("zhu'ji", "主机", 500);
    let chain = ChainedLexicon::new(&front, &back);
    assert_eq!(texts(chain.lookup("zhu'ji").expect("the chain answers")), vec!["主机"]);
    assert_eq!(
        texts(chain.lookup("zzz").expect("the chain answers")),
        Vec::<String>::new()
    );
}

#[test]
fn test_forget_word_drops_every_entry_of_that_word() {
    let mut lexicon = UserLexicon::new();
    lexicon.coin("zhu'ji", "诸暨");
    lexicon.coin("zhu'ji", "主集");
    lexicon.coin("gan'xie", "感谢");
    assert!(lexicon.knows_word("诸暨"));

    assert!(lexicon.forget_word("诸暨"));
    assert!(!lexicon.knows_word("诸暨"));
    assert!(lexicon.knows_word("主集"), "the same key's other word stays");
    assert!(lexicon.knows_word("感谢"), "another key's word stays");
    assert!(!lexicon.forget_word("诸暨"), "the second forget finds nothing");
}

#[test]
fn test_from_rows_restores_the_table_the_store_held() {
    let lexicon = UserLexicon::from_rows([
        (String::from("zhu'ji"), String::from("诸暨"), 3),
        (String::from("gan'xie"), String::from("感谢"), 1),
    ]);
    assert!(lexicon.knows_word("诸暨"));
    assert!(lexicon.knows_word("感谢"));
    let words = lexicon.lookup("zhu'ji").expect("the key serves");
    let words: Vec<_> = words.collect();
    assert_eq!(words.len(), 1);
    assert!(
        words[0].weight > COINED_BASE_WEIGHT,
        "the restored commits raise the weight above the base"
    );
}

#[test]
fn test_pinned_entries_survive_an_eviction_the_table_size_forces() {
    // The cap is the frequency store's own half-million, far past what a test can fill
    // honestly; the pin semantics are what the test pins down, through a table driven
    // to its cap by construction instead.
    let mut lexicon = UserLexicon::new();
    lexicon.coin("a", "甲");
    lexicon.set_pinned("甲", true);
    assert!(lexicon.knows_word("甲"), "the pinned entry stays put");
    // Unpinning releases it to the table's ordinary rules.
    lexicon.set_pinned("甲", false);
    assert!(lexicon.knows_word("甲"));
}
