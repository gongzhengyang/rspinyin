//! Tests for the recovery pass.
//!
//! Every test owns a directory under the system temp directory and passes the quarantine
//! stamp in, so nothing here reads the clock, the environment, a real XDG directory or a
//! real dictionary. The fixtures are built with the public writer, which is what makes
//! "a damaged container" a byte-level fact rather than a mocked one.

use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use ime_types::{DictError, UserFreqSource};

use super::quarantine::move_aside;
use super::*;
use crate::format::writer::DictWriter;
use crate::format::{DictEntry, PROB_Q12_MAX, SectionKind, hash_word, pack_fst_value};
use crate::user_db::UserDb;

/// The stamp every test quarantines with, so the expected file name is a literal.
const STAMP: u64 = 1_700_000_000;

/// Serial number for scratch directories; tests in one binary run in parallel threads,
/// and nextest runs several binaries at once.
static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A directory one test owns, removed when the guard drops.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    /// Creates an empty directory named after `tag` and this process.
    fn new(tag: &str) -> Self {
        let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "rspinyin-recover-{}-{tag}-{serial}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        Self { dir }
    }

    /// The path of `name` inside the directory.
    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// The names of the files in the directory, sorted.
    fn entries(&self) -> Vec<String> {
        entries(&self.dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // A test that already failed must not be turned into an abort by a second panic,
        // so the cleanup result is dropped rather than unwrapped.
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// The names of the files in `dir`, sorted.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("reading the directory")
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

/// Sets the permission bits of `path`.
fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("setting the mode");
}

/// Whether the process may create a file in `dir`.
///
/// Asked by writing, because the mode bits cannot answer it for a process the mode does
/// not bind -- which is what a test run as root is.
fn writable(dir: &Path) -> bool {
    let probe = dir.join("write-probe");
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// A container the lexicon accepts: one key, one word, every section present.
fn dictionary_image() -> Vec<u8> {
    let mut builder = fst::MapBuilder::memory();
    builder
        .insert("ni", pack_fst_value(0, 1).expect("packing the word list range"))
        .expect("inserting the key");
    let fst_bytes = builder.into_inner().expect("finishing the FST");

    let mut unigram = Vec::new();
    unigram.extend_from_slice(&hash_word("你").to_le_bytes());
    unigram.extend_from_slice(&PROB_Q12_MAX.to_le_bytes());
    unigram.extend_from_slice(&0u16.to_le_bytes());

    let mut writer = DictWriter::new();
    writer
        .add_section(SectionKind::Fst, fst_bytes)
        .expect("fst");
    writer
        .add_section(
            SectionKind::Entries,
            DictEntry::new(0, 3, 1, 0, 100).encode().to_vec(),
        )
        .expect("entries");
    writer
        .add_section(SectionKind::StrPool, "你".as_bytes().to_vec())
        .expect("strpool");
    writer
        .add_section(SectionKind::Unigram, unigram)
        .expect("unigram");
    writer
        .add_section(SectionKind::WordList, 0u32.to_le_bytes().to_vec())
        .expect("wordlist");
    writer.encode().expect("encoding the container")
}

#[test]
fn test_recover_dict_accepts_a_container_written_by_the_compiler() {
    let scratch = Scratch::new("dict-healthy");
    let path = scratch.path("base.dict");
    fs::write(&path, dictionary_image()).expect("writing the fixture");

    let outcome = recover_dict(&path);
    assert!(outcome.is_healthy(), "{outcome:?}");
    assert_eq!(outcome.code(), None, "a healthy file raises no diagnostic");
    assert!(path.exists(), "a usable dictionary is left exactly where it is");
    assert_eq!(scratch.entries(), vec!["base.dict"], "nothing was quarantined");
}

#[test]
fn test_recover_dict_quarantines_a_damaged_file_and_keeps_it() {
    // The four ways a container stops being one: its magic, its version, its checksum
    // and its length.
    let cases: [(&str, fn(&mut Vec<u8>)); 4] = [
        ("magic", |image| image[0] = b'X'),
        ("version", |image| {
            image[4..6].copy_from_slice(&2u16.to_le_bytes());
        }),
        ("checksum", |image| {
            let last = image.len() - 1;
            image[last] ^= 0xFF;
        }),
        ("truncated", |image| image.truncate(image.len() / 2)),
    ];
    for (tag, damage) in cases {
        let scratch = Scratch::new(tag);
        let path = scratch.path("base.dict");
        let mut image = dictionary_image();
        damage(&mut image);
        fs::write(&path, &image).expect("writing the damaged dictionary");

        let outcome = recover_dict_at(&path, STAMP);
        let RecoveryOutcome::DictMissing { cause, .. } = &outcome else {
            panic!("{tag}: a damaged dictionary must not be reported as usable");
        };
        assert!(is_damaged(cause), "{tag}: unexpected cause {cause}");
        assert_eq!(outcome.code(), Some(DICT_CORRUPT_CODE), "{tag}");

        let expected = scratch.path(&format!("base.dict.corrupt.{STAMP}"));
        assert_eq!(
            outcome.quarantine_path(),
            Some(expected.as_path()),
            "{tag}: the quarantine name carries the mark and the stamp"
        );
        assert_eq!(
            fs::read(&expected).expect("reading the quarantine"),
            image,
            "{tag}: the damaged bytes are kept for a manual recovery"
        );
        assert!(!path.exists(), "{tag}: the damaged file is out of the way");
    }
}

#[test]
fn test_recover_dict_quarantines_a_truncated_file_of_header_length() {
    // A file too short to hold even a header takes the length check, not the magic one:
    // the two are different branches of the parser and both have to end in a quarantine.
    let scratch = Scratch::new("dict-stub");
    let path = scratch.path("base.dict");
    fs::write(&path, vec![0u8; 16]).expect("writing the stub");

    let outcome = recover_dict_at(&path, STAMP);
    let RecoveryOutcome::DictMissing { cause, .. } = &outcome else {
        panic!("a stub must not be reported as usable: {outcome:?}");
    };
    assert!(matches!(cause, DictError::LengthOutOfRange { .. }), "{cause}");
    assert!(outcome.quarantine_path().is_some(), "the stub was moved aside");
}

#[test]
fn test_recover_dict_reports_a_missing_file_without_creating_anything() {
    let scratch = Scratch::new("dict-missing");
    let path = scratch.path("base.dict");

    let outcome = recover_dict_at(&path, STAMP);
    assert!(
        matches!(&outcome, RecoveryOutcome::DictMissing { cause: DictError::Io(_), .. }),
        "{outcome:?}"
    );
    assert!(
        outcome.quarantine_path().is_none(),
        "there is nothing to quarantine when there is no file"
    );
    assert!(scratch.entries().is_empty(), "the pass created nothing");
}

#[test]
fn test_recover_dict_keeps_a_second_quarantine_from_the_same_second() {
    let scratch = Scratch::new("dict-twice");
    let path = scratch.path("base.dict");
    for round in 0..2 {
        let mut image = dictionary_image();
        image[0] = b'X';
        fs::write(&path, &image).expect("writing the damaged dictionary");
        let outcome = recover_dict_at(&path, STAMP);
        assert!(outcome.quarantine_path().is_some(), "round {round}");
    }
    assert_eq!(
        scratch.entries(),
        vec![
            format!("base.dict.corrupt.{STAMP}"),
            format!("base.dict.corrupt.{STAMP}.1"),
        ],
        "the second quarantine takes the next name instead of replacing the first"
    );
}

#[test]
fn test_move_aside_never_overwrites_an_existing_quarantine() {
    let scratch = Scratch::new("move-aside");
    let target = scratch.path("user.redb");

    fs::write(&target, b"first damage").expect("writing the file");
    let first = move_aside(&target, STAMP).expect("moving aside");
    fs::write(&target, b"second damage").expect("writing the file again");
    let second = move_aside(&target, STAMP).expect("moving aside again");

    assert_ne!(first, second, "each quarantine gets a name of its own");
    assert_eq!(fs::read(&first).expect("reading"), b"first damage");
    assert_eq!(fs::read(&second).expect("reading"), b"second damage");
}

#[test]
fn test_move_aside_reports_a_missing_file_and_leaves_nothing_behind() {
    let scratch = Scratch::new("move-missing");
    let result = move_aside(&scratch.path("nothing.redb"), STAMP);
    assert!(
        matches!(&result, Err(error) if error.kind() == ErrorKind::NotFound),
        "{result:?}"
    );
    assert!(
        scratch.entries().is_empty(),
        "the reserved name is released when the rename cannot happen"
    );
}

#[test]
fn test_move_aside_refuses_a_path_without_a_file_name() {
    let result = move_aside(Path::new("/"), STAMP);
    assert!(
        matches!(&result, Err(error) if error.kind() == ErrorKind::InvalidInput),
        "{result:?}"
    );
}

#[test]
fn test_recover_user_db_accepts_a_store_it_just_created() {
    let scratch = Scratch::new("store-fresh");
    let path = scratch.path("user.redb");

    // A store that does not exist yet is created by the probe, exactly as the caller
    // would create it a moment later.
    let outcome = recover_user_db(&path);
    assert!(outcome.is_healthy(), "{outcome:?}");
    assert!(path.is_file(), "the store is there afterwards");
    assert_eq!(outcome.code(), None);
}

#[test]
fn test_recover_user_db_accepts_a_store_that_already_holds_records() {
    let scratch = Scratch::new("store-used");
    let path = scratch.path("user.redb");
    {
        let mut store = UserDb::open(&path).expect("opening the store");
        store.record("ni'hao", 0);
        store.final_commit().expect("flushing");
    }

    let outcome = recover_user_db_at(&path, STAMP);
    assert!(outcome.is_healthy(), "{outcome:?}");
    assert_eq!(scratch.entries(), vec!["user.redb"], "nothing was quarantined");
    let store = UserDb::open(&path).expect("reopening the store");
    assert_eq!(store.freq("ni'hao"), 1, "the records survived the pass");
}

#[test]
fn test_recover_user_db_quarantines_an_empty_file_and_replaces_it() {
    let scratch = Scratch::new("store-empty");
    let path = scratch.path("user.redb");
    fs::write(&path, b"").expect("placing an interrupted store");

    let outcome = recover_user_db_at(&path, STAMP);
    assert!(
        matches!(&outcome, RecoveryOutcome::UserDbRebuilt { .. }),
        "{outcome:?}"
    );
    assert_eq!(outcome.code(), Some(USER_DB_RECOVERED_CODE));
    let quarantine = outcome
        .quarantine_path()
        .expect("the interrupted file was moved aside");
    assert!(
        fs::read(quarantine).expect("reading").is_empty(),
        "the interrupted file held nothing, and that is kept"
    );
    assert_eq!(scratch.entries().len(), 2, "the quarantine and the new store");
}

#[test]
fn test_recover_user_db_quarantines_a_file_that_is_not_a_store() {
    let scratch = Scratch::new("store-junk");
    let path = scratch.path("user.redb");
    let junk: Vec<u8> = (0..4096u32).map(|index| (index % 251) as u8).collect();
    fs::write(&path, &junk).expect("placing a damaged store");

    let outcome = recover_user_db_at(&path, STAMP);
    assert!(
        matches!(&outcome, RecoveryOutcome::UserDbRebuilt { .. }),
        "{outcome:?}"
    );
    let quarantine = outcome.quarantine_path().expect("the damaged store was moved aside");
    assert_eq!(
        fs::read(quarantine).expect("reading"),
        junk,
        "the damaged bytes are kept for a manual recovery"
    );

    // The replacement is a working store, and it starts from zero.
    let store = UserDb::open(&path).expect("opening the replacement");
    assert_eq!(store.record_count().expect("counting"), 0);
    assert_eq!(store.freq("ni'hao"), 0, "learning starts over");
}

#[test]
fn test_recover_user_db_is_idempotent() {
    let scratch = Scratch::new("store-twice");
    let path = scratch.path("user.redb");
    fs::write(&path, b"not a store").expect("placing a damaged store");

    let first = recover_user_db_at(&path, STAMP);
    assert!(
        matches!(&first, RecoveryOutcome::UserDbRebuilt { .. }),
        "{first:?}"
    );
    let second = recover_user_db_at(&path, STAMP);
    assert!(second.is_healthy(), "the second pass has nothing left to do: {second:?}");
    assert_eq!(scratch.entries().len(), 2, "the second pass quarantined nothing");
}

#[test]
fn test_recover_user_db_stays_within_the_startup_budget() {
    let scratch = Scratch::new("budget");
    let path = scratch.path("user.redb");
    fs::write(&path, b"not a store").expect("placing a damaged store");

    let started = Instant::now();
    let outcome = recover_user_db_at(&path, STAMP);
    let elapsed = started.elapsed();

    assert!(
        matches!(&outcome, RecoveryOutcome::UserDbRebuilt { .. }),
        "{outcome:?}"
    );
    // The pass runs inside the addon's load budget, and what it costs is one rename, one
    // empty store and one diagnostic line.
    assert!(
        elapsed < Duration::from_millis(200),
        "the self-healing pass took {elapsed:?}"
    );
}

#[test]
fn test_recover_user_db_leaves_a_store_that_is_already_open_alone() {
    let scratch = Scratch::new("store-in-use");
    let path = scratch.path("user.redb");
    let open = UserDb::open(&path).expect("opening the store");

    let outcome = recover_user_db_at(&path, STAMP);
    assert!(
        matches!(&outcome, RecoveryOutcome::ReadonlyMode { .. }),
        "a store this process cannot lock is not a damaged store: {outcome:?}"
    );
    assert!(
        outcome.quarantine_path().is_none(),
        "a healthy store held by another handle must never be moved aside"
    );
    drop(open);
    assert!(path.is_file(), "the store is still where it was");
    assert!(recover_user_db_at(&path, STAMP).is_healthy(), "and it is usable again");
}

#[test]
fn test_recover_user_db_reports_a_store_path_that_is_a_directory_as_readonly() {
    let scratch = Scratch::new("store-is-dir");
    let path = scratch.path("user.redb");
    fs::create_dir_all(&path).expect("taking the store's place");

    let outcome = recover_user_db_at(&path, STAMP);
    // The probe narrows the directory's mode on its way to failing; the test puts it
    // back so that the scratch directory can still be removed.
    set_mode(&path, 0o700);
    assert!(
        matches!(&outcome, RecoveryOutcome::ReadonlyMode { .. }),
        "{outcome:?}"
    );
    assert_eq!(outcome.code(), Some(READONLY_CODE));
    assert!(outcome.quarantine_path().is_none());
    assert!(path.is_dir(), "a directory is never moved aside");
}

#[test]
fn test_recover_user_db_reports_an_unusable_data_directory_as_readonly() {
    let scratch = Scratch::new("dir-is-file");
    let data_dir = scratch.path("data");
    fs::write(&data_dir, b"not a directory").expect("taking the data directory's place");

    let outcome = recover_user_db_at(&data_dir.join("user.redb"), STAMP);
    assert!(
        matches!(&outcome, RecoveryOutcome::ReadonlyMode { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        fs::read(&data_dir).expect("reading"),
        b"not a directory",
        "the file that took the directory's place is left alone"
    );
}

#[test]
fn test_recover_user_db_reports_an_unwritable_data_directory_as_readonly() {
    let scratch = Scratch::new("readonly-dir");
    let data_dir = scratch.path("data");
    fs::create_dir_all(&data_dir).expect("creating the data directory");
    set_mode(&data_dir, 0o500);
    if writable(&data_dir) {
        // A process the mode does not bind -- root -- cannot build this case. The
        // unusable-directory case above covers the same outcome unconditionally.
        set_mode(&data_dir, 0o700);
        return;
    }

    let outcome = recover_user_db_at(&data_dir.join("user.redb"), STAMP);
    set_mode(&data_dir, 0o700);
    assert!(
        matches!(&outcome, RecoveryOutcome::ReadonlyMode { .. }),
        "{outcome:?}"
    );
    assert_eq!(outcome.code(), Some(READONLY_CODE));
    assert!(
        entries(&data_dir).is_empty(),
        "nothing may be created in a directory the user cannot write to"
    );
}

#[test]
fn test_recovery_outcome_reports_its_code_detail_and_quarantine() {
    let healthy = RecoveryOutcome::Healthy;
    assert!(healthy.is_healthy());
    assert_eq!(healthy.code(), None);
    assert!(healthy.quarantine_path().is_none());

    let quarantine = PathBuf::from("/data/user.redb.corrupt.7");
    let rebuilt = RecoveryOutcome::UserDbRebuilt {
        quarantine: quarantine.clone(),
    };
    assert!(!rebuilt.is_healthy());
    assert_eq!(rebuilt.code(), Some(USER_DB_RECOVERED_CODE));
    assert_eq!(rebuilt.quarantine_path(), Some(quarantine.as_path()));
    assert!(
        rebuilt.detail().contains("user.redb.corrupt.7"),
        "{}",
        rebuilt.detail()
    );

    let unmoved = RecoveryOutcome::UserDbRebuilt {
        quarantine: PathBuf::new(),
    };
    assert_eq!(unmoved.quarantine_path(), None, "an empty field means nothing was moved");
    assert_eq!(
        unmoved.code(),
        Some(USER_DB_RECOVERED_CODE),
        "the code names what happened, not where the file went"
    );

    let missing = RecoveryOutcome::DictMissing {
        quarantine: PathBuf::new(),
        cause: DictError::MagicMismatch,
    };
    assert_eq!(missing.code(), Some(DICT_CORRUPT_CODE));
    assert!(missing.detail().contains("magic mismatch"), "{}", missing.detail());

    let readonly = RecoveryOutcome::ReadonlyMode {
        reason: String::from("the directory is not writable"),
    };
    assert_eq!(readonly.code(), Some(READONLY_CODE));
    assert_eq!(readonly.detail(), "the directory is not writable");
    assert!(readonly.quarantine_path().is_none());
}

#[test]
fn test_atomic_replace_writes_the_bytes_and_leaves_no_temporary() {
    let scratch = Scratch::new("replace");
    let target = scratch.path("base.dict");

    atomic_replace(&target, b"the first image").expect("the first replacement");
    assert_eq!(fs::read(&target).expect("reading"), b"the first image");
    assert!(
        !writer::temp_path(&target).exists(),
        "the temporary is gone once the rename has happened"
    );

    atomic_replace(&target, b"the second image").expect("the second replacement");
    assert_eq!(fs::read(&target).expect("reading"), b"the second image");
    assert_eq!(scratch.entries(), vec!["base.dict"]);
}

#[test]
fn test_atomic_replace_creates_a_missing_parent_directory() {
    let scratch = Scratch::new("replace-nested");
    let target = scratch.path("nested/deeper/base.dict");

    atomic_replace(&target, b"the image").expect("replacing into a new directory");
    assert_eq!(fs::read(&target).expect("reading"), b"the image");
}

#[test]
fn test_atomic_replace_leaves_the_target_intact_when_the_write_cannot_start() {
    let scratch = Scratch::new("replace-blocked");
    let target = scratch.path("base.dict");
    fs::write(&target, b"the previous image").expect("placing the previous file");
    // A directory where the temporary belongs: creating the temporary fails, and the
    // failure has to leave the target alone.
    fs::create_dir_all(writer::temp_path(&target)).expect("taking the temporary's place");

    let result = atomic_replace(&target, b"the new image");
    assert!(result.is_err(), "a write that cannot start must be reported");
    assert_eq!(
        fs::read(&target).expect("reading"),
        b"the previous image",
        "the previous file is untouched"
    );
    assert!(
        writer::temp_path(&target).is_dir(),
        "the cleanup removes a temporary it created, not whatever is at that name"
    );
}

#[test]
fn test_atomic_replace_never_exposes_a_partial_file_to_a_reader() {
    let scratch = Scratch::new("replace-torn");
    let target = scratch.path("base.dict");
    let first = vec![b'A'; 8 * 1024];
    let second = vec![b'B'; 8 * 1024];
    atomic_replace(&target, &first).expect("the first image");

    let started = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let writer_thread = {
        let target = target.clone();
        let first = first.clone();
        let second = second.clone();
        let started = Arc::clone(&started);
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let mut round = 0usize;
            while !done.load(Ordering::Acquire) {
                let image = if round % 2 == 0 { &second } else { &first };
                atomic_replace(&target, image).expect("replacing under a reader");
                round += 1;
                started.store(true, Ordering::Release);
            }
        })
    };

    // Waiting for the first replacement is what makes the overlap the normal case; the
    // spin is bounded so a starved writer cannot hang the test, and a reader that ran
    // alone would still be checking a file that is always complete.
    let mut spins = 0u32;
    while !started.load(Ordering::Acquire) && spins < 10_000_000 {
        spins += 1;
        std::hint::spin_loop();
    }

    let mut torn = 0usize;
    for _ in 0..1000 {
        let seen = fs::read(&target).expect("reading the target");
        if seen != first && seen != second {
            torn += 1;
        }
    }
    done.store(true, Ordering::Release);
    writer_thread.join().expect("the writer finishes");

    assert_eq!(torn, 0, "a reader saw a file that was neither of the two images");
}

#[test]
fn test_is_out_of_space_names_the_disk_full_conditions() {
    assert!(is_out_of_space(&io::Error::from(ErrorKind::StorageFull)));
    assert!(is_out_of_space(&io::Error::from(ErrorKind::QuotaExceeded)));
    assert!(!is_out_of_space(&io::Error::from(ErrorKind::PermissionDenied)));
    assert!(!is_out_of_space(&io::Error::from(ErrorKind::NotFound)));
}
