//! The fixtures every case in this channel builds on: a scratch directory, a container the
//! loader accepts, and the paths of the repository's own compiled dictionary.
//!
//! Responsibility: hand a case a directory it owns, a container built with the real writer,
//! and the resolved location of the artifact the compiled-dictionary case pins. It asserts
//! nothing itself.
//!
//! Boundaries: test-only, and it is `#[cfg(test)]` for that reason. The container is built
//! here rather than read from `data/compiled/` so that the bulk of the cases depend on no
//! artifact at all -- a compiled dictionary is a build output and is not in the repository,
//! so a case that needed one would not run on a fresh checkout.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use ime_dict::format::{
    DictEntry, PROB_Q12_MAX, SectionKind, hash_word, pack_fst_value, writer::DictWriter,
};

use super::DictFixture;

/// Serial number for scratch directories; tests in one binary run in parallel threads, and
/// nextest runs several binaries at once.
static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Where the compiled dictionary sits inside the workspace.
const COMPILED_DICTIONARY: &str = "data/compiled/base.dict";

/// A directory one test owns, removed when the guard drops.
pub(super) struct Scratch {
    /// The directory.
    dir: PathBuf,
}

impl Scratch {
    /// Creates an empty directory named after `tag` and this process.
    pub(super) fn new(tag: &str) -> Self {
        let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "rspinyin-dict-inject-{}-{tag}-{serial}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        Self { dir }
    }

    /// The path of `name` inside the directory.
    pub(super) fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// The names of the files in the directory, sorted.
    pub(super) fn names(&self) -> Vec<String> {
        dir_names(&self.dir)
    }
}

impl Drop for Scratch {
    /// Removes the directory and everything a failed case left in it.
    fn drop(&mut self) {
        // A test that already failed must not be turned into an abort by a second panic, so
        // the cleanup result is dropped rather than unwrapped.
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// The container the fixtures are built from, and what a case has to know about it.
pub(super) struct Source {
    /// The image.
    pub(super) image: Vec<u8>,
    /// The byte length of its string pool.
    pub(super) pool_len: u32,
    /// One of the two keys its index holds.
    pub(super) key: &'static str,
    /// The other key its index holds.
    pub(super) other_key: &'static str,
}

/// Builds a container the loader accepts: two words, two keys, every section present.
///
/// It is built with the real writer, so a case starts from the shape the compiler produces
/// rather than from a hand-rolled image the reader would refuse for the wrong reason.
pub(super) fn source() -> Source {
    let words = ["你", "你好"];
    let mut strpool = Vec::new();
    let mut entries = Vec::new();
    let mut unigram = Vec::new();
    for (index, word) in words.iter().enumerate() {
        let offset = u32::try_from(strpool.len()).expect("the fixture is small");
        strpool.extend_from_slice(word.as_bytes());
        let syllables = u8::try_from(word.chars().count()).expect("the fixture is short");
        let weight = 900 - u32::try_from(index).expect("the fixture is short");
        entries.extend_from_slice(
            &DictEntry::new(offset, word.len() as u16, syllables, 0, weight).encode(),
        );
        unigram.extend_from_slice(&hash_word(word).to_le_bytes());
        unigram.extend_from_slice(&PROB_Q12_MAX.to_le_bytes());
        unigram.extend_from_slice(&0u16.to_le_bytes());
    }
    let pool_len = u32::try_from(strpool.len()).expect("the fixture is small");

    // One word id per key, in the order the keys are inserted below.
    let mut wordlist = Vec::new();
    for id in [0u32, 1] {
        wordlist.extend_from_slice(&id.to_le_bytes());
    }
    let mut index = fst::MapBuilder::memory();
    for (key, start) in [("hao", 1u64), ("ni", 0)] {
        index
            .insert(key, pack_fst_value(start, 1).expect("packing the range"))
            .expect("inserting the key");
    }

    let mut writer = DictWriter::new();
    for (kind, payload) in [
        (
            SectionKind::Fst,
            index.into_inner().expect("finishing the index"),
        ),
        (SectionKind::Entries, entries),
        (SectionKind::StrPool, strpool),
        (SectionKind::Unigram, unigram),
        (SectionKind::WordList, wordlist),
    ] {
        writer.add_section(kind, payload).expect("adding a section");
    }
    Source {
        image: writer.encode().expect("encoding the container"),
        pool_len,
        key: "ni",
        other_key: "hao",
    }
}

/// Writes [`source`]'s container into `scratch` and builds a fixture over a copy of it.
pub(super) fn fixture(scratch: &Scratch) -> (DictFixture, Source) {
    let source = source();
    let origin = scratch.path("base.dict");
    fs::write(&origin, &source.image).expect("writing the source container");
    let fixture =
        DictFixture::from_compiled(&origin, &scratch.path("copy.dict")).expect("copying it");
    (fixture, source)
}

/// The workspace root, resolved from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The repository's compiled dictionary, whether or not it has been built.
pub(super) fn compiled_dictionary() -> PathBuf {
    workspace_root().join(COMPILED_DICTIONARY)
}

/// The directory the compiled dictionary is written into.
pub(super) fn compiled_dir() -> PathBuf {
    workspace_root().join("data/compiled")
}

/// The names of the entries in `dir`, sorted, or an empty vector when it does not exist.
pub(super) fn dir_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .map(|entry| {
            entry
                .expect("reading a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}
