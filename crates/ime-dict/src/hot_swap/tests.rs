//! Tests for the hot-swap slot: what a validated swap serves, and what a rejected
//! one leaves standing.

use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;

use ime_types::{LanguageModel, Lexicon};

use crate::format::reader::Verify;
use crate::fst_index::tests::fixture_image_with_bigrams;
use crate::hot_swap::{LexiconBundle, LexiconSlot};

/// A scratch directory for one test, named after the test and the process id.
fn scratch(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "rspinyin-ime-dict-hot-swap-{}-{name}",
        process::id()
    ));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).expect("creating the scratch directory");
    directory
}

/// Writes `bytes` to `path`, returning it.
fn staged(path: &Path, bytes: &[u8]) -> PathBuf {
    fs::write(path, bytes).expect("writing the staged container");
    path.to_path_buf()
}

/// The bundle the slot starts on, over a container of the test's own.
fn bundle_in(dir: &Path, label: &str) -> (PathBuf, LexiconBundle) {
    let path = staged(
        &dir.join(format!("{label}.dict")),
        &fixture_image_with_bigrams(None, &[]),
    );
    let bundle = LexiconBundle::load(&path, Verify::Full).expect("the container loads");
    (path, bundle)
}

#[test]
fn test_try_swap_serves_the_staged_container_after_it_validates() {
    let dir = scratch("swap-serves");
    let (_, live) = bundle_in(&dir, "live");
    let slot = LexiconSlot::new(live);
    // The staged candidate is a different file: a copy under its own name, so the
    // swap is a real change of container and not a re-read of the live one.
    let candidate = staged(
        &dir.join("staged.dict"),
        &fixture_image_with_bigrams(None, &[]),
    );

    let swapped = slot.try_swap(&candidate).expect("the candidate validates");

    assert_eq!(
        swapped.path(),
        candidate,
        "the live bundle is the staged one"
    );
    assert_eq!(slot.load().path(), candidate, "the slot serves it");
    // The model swapped with the container: a word the new unigram table scores is
    // answered by the new model, not by the old one's table. The fixture scores 你
    // through both containers, so the lookup must answer without panicking and from
    // the model the swap installed.
    let model = slot.load();
    assert_eq!(
        model.lm().unigram("你"),
        swapped.lm().unigram("你"),
        "the model the slot serves is the swapped bundle's"
    );
}

#[test]
fn test_try_swap_rejects_a_damaged_candidate_and_keeps_the_live_container() {
    let dir = scratch("swap-rejects");
    let (live_path, live) = bundle_in(&dir, "live");
    let slot = LexiconSlot::new(live);
    // The staged candidate carries a flipped byte in its body: the checksums the
    // full validation computes are the writer's, so the corruption is the load's to
    // catch and no hand-built broken section is needed.
    let mut bytes = fixture_image_with_bigrams(None, &[]);
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xFF;
    let candidate = staged(&dir.join("damaged.dict"), &bytes);

    let outcome = slot.try_swap(&candidate);

    assert!(outcome.is_err(), "the damaged candidate is rejected");
    assert_eq!(
        slot.load().path(),
        live_path,
        "the live container keeps serving"
    );
}

#[test]
fn test_try_swap_rejects_a_missing_candidate_and_keeps_the_live_container() {
    let dir = scratch("swap-missing");
    let (live_path, live) = bundle_in(&dir, "live");
    let slot = LexiconSlot::new(live);
    let candidate = dir.join("not-there.dict");

    let outcome = slot.try_swap(&candidate);

    assert!(outcome.is_err(), "a file that is not there cannot validate");
    assert_eq!(slot.load().path(), live_path);
}

#[test]
fn test_readers_keep_their_bundle_alive_across_a_swap() {
    // A decode that fetched the live bundle before the swap holds its own `Arc`, so
    // the container it reads stays mapped however many swaps land afterwards -- and
    // the reader's answers stay the old container's until it lets go.
    let dir = scratch("swap-reader");
    let (live_path, live) = bundle_in(&dir, "live");
    let slot = LexiconSlot::new(live);
    let held: Arc<LexiconBundle> = slot.load();
    let candidate = staged(
        &dir.join("next.dict"),
        &fixture_image_with_bigrams(None, &[]),
    );

    slot.try_swap(&candidate).expect("the swap lands");

    assert_eq!(slot.load().path(), candidate, "the slot moved on");
    assert_eq!(held.path(), live_path, "the held bundle is the old one");
    // The held bundle still answers: its mapping is alive because the reader's Arc
    // says so, which is the whole reason the slot hands out `Arc`s and not borrows.
    assert!(
        held.lexicon().lookup("ni").is_ok(),
        "the held container still serves lookups"
    );
}
