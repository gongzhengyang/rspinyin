//! The dictionary-injection channel: malformed data files, built on purpose.
//!
//! # What this channel is for
//!
//! A dictionary and a user store are untrusted input. The loader has to check the magic,
//! the format version, every length field and every checksum *before* it trusts an offset,
//! and it has to answer a file it cannot use with a typed error rather than a panic or a
//! read past the end of a mapping. That promise is worth exactly what it is tested
//! against, and a genuinely damaged file is not something a case can wait for -- so this
//! module builds one, names the field it breaks, and reports which step of the load caught
//! it.
//!
//! # A mutation has to leave the rest of the container valid
//!
//! This is the rule the module is built around, and the reason a naive byte flip proves
//! nothing. A checksum covers a whole section and the file body, so a byte flipped inside
//! `ENTRIES` breaks *the checksum* first: a loader that refuses the file is refusing it for
//! the checksum, and the case would report a pass while never reaching the entry field it
//! meant to test. Every mutation therefore declares whether it *is* the checksum break
//! ([`DictMutation::SectionCrc`]) or a field break, and a field break has the container's
//! checksums recomputed afterwards so that the field is the only thing left wrong.
//! [`DictFixture::mutate`] refuses a change that would leave the container valid outright:
//! a fixture that silently produces a usable file is worse than no fixture at all.
//!
//! # Which step catches which field
//!
//! The steps are not interchangeable, and the difference is worth stating rather than
//! smoothing over. [`RefusalPoint`] names all four:
//!
//! - The header, the section table and the checksums are checked when the container is
//!   opened, so a wrong magic, a future version, a wrong length or a wrong checksum never
//!   becomes a lexicon at all.
//! - A record's `word_len` is checked when the record is decoded, which happens on the read
//!   path: `DictEntry::validate` rejects a zero or an oversized length, and the loader
//!   itself never walks `ENTRIES`.
//! - A record's `word_off` is *not* checked by that validation, so a record naming a range
//!   outside the string pool decodes cleanly and is caught one step later, when the record
//!   is converted into a word. A pool cut shorter than the records that name it arrives at
//!   the same step from the other side, and for the same reason: nothing in the layout says
//!   how long the pool has to be, so only a conversion can notice.
//! - An FST value is read when a key is looked up, so a word-list range that leaves the
//!   section is caught by the lookup.
//!
//! [`DictFixture::assert_rejected`] checks the step as well as the kind of error, and
//! [`DictFixture::assert_rejected_field`] checks which range was refused. Both halves
//! matter: every length and offset failure shares one `DictError` variant, so a case that
//! asserted only the variant could pass on a container refused for a different field, and a
//! case that asserted only the field could pass on a container refused at the wrong step.
//!
//! # What a mutation is allowed to touch
//!
//! A mutation that patches bytes writes exactly one field. The rest of what changes is the
//! checksum arithmetic the container's own layout forces -- a patched record moves its
//! section's checksum and the file's -- and the byte-level module reports the field it names
//! as a value, so a case can hold the byte diff to it. A mutation that wrote somewhere else
//! would be a fixture whose refusal came from a fault it never declared, which is the failure
//! this rule exists to rule out.
//!
//! # The scratch copy
//!
//! Nothing here writes to the file it was given. [`DictFixture::from_compiled`] copies the
//! source to the destination the caller names, every mutation is applied to a fresh copy of
//! the *pristine* image rather than to the previous result -- so two mutations never
//! compound into a container broken two ways -- and [`Drop`] takes the copy away again. A
//! case's copy belongs under `RUN/<module>/<TC-ID>/`, the layout the rest of the harness
//! uses; `data/compiled/` is read and never written, and [`sha256`] is what pins that.
//!
//! # What this channel does not do
//!
//! It never runs the decoder, never starts a session and never decides what the plugin does
//! next. It builds a malformed file and reports what the loader said about it; quarantining
//! the file, degrading to passthrough and going read-only belong to the recovery pass, and
//! are asserted there.
//!
//! # The one deferral
//!
//! A container truncated *while it is mapped* raises `SIGBUS` on the first access past the
//! end of the file, which no `catch_unwind` can take back. [`DictMutation::Truncate`]
//! truncates before the file is mapped, so what this module asserts is the load-time length
//! check; the signal path needs the crash handler and is not covered here.

// The channel is exercised by the tests below and by the cases that will call it, which live
// in a module this one does not own. Until those call sites land, every item here is reported
// as dead code in a non-test build, and the attribute goes away with them.
//
// `unused_imports` is covered by the same reasoning: the `use` lines below are this module's
// surface, and an import that a *binary* crate's non-test build never names is reported as
// unused.
#![allow(dead_code, unused_imports)]

use std::fs;
use std::path::{Path, PathBuf};

use ime_dict::format::reader::{Reader, Verify};
use ime_dict::fst_index::FstLexicon;
use ime_dict::user_db::UserDb;
use ime_types::{DictError, ImeError, Lexicon};
use sha2::{Digest, Sha256};

mod error;
mod mutate;
mod mutation;

#[cfg(test)]
mod mutation_tests;
#[cfg(test)]
mod support;
#[cfg(test)]
mod tests;

pub use self::error::FixtureError;
pub use self::mutation::{DictErrorKind, DictMutation, Refusal, RefusalPoint};

/// The name a user store carries inside the plugin's data directory.
pub const USER_STORE_NAME: &str = "user.redb";

/// Returns the SHA-256 of the file at `path`, as lowercase hexadecimal.
///
/// A case uses this to pin the file a mutation was made *from*. The whole point of the
/// scratch copy is that the source is read and never written, and a digest taken before and
/// after is what turns that from a claim into an assertion.
///
/// # Errors
/// Returns [`FixtureError::Io`] when the file cannot be read.
pub fn sha256(path: &Path) -> Result<String, FixtureError> {
    let bytes = fs::read(path).map_err(|source| FixtureError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// A scratch copy of a container, with one named mutation applied to it.
///
/// The pristine image is held so that every call to [`DictFixture::mutate`] starts from the
/// file as the compiler wrote it, not from the result of the previous mutation: a container
/// broken two ways can be refused for the wrong one, which is the failure mode this type
/// exists to rule out.
pub struct DictFixture {
    /// The scratch copy, written by `mutate` and removed on drop.
    path: PathBuf,
    /// Whether this fixture created `path`'s parent directory.
    created_parent: bool,
    /// The container as it was handed in.
    pristine: Vec<u8>,
    /// The mutation currently applied, or `None` while the copy is pristine.
    applied: Option<DictMutation>,
}

impl DictFixture {
    /// Copies `src` to `dest` and prepares to mutate the copy.
    ///
    /// # Errors
    /// Returns [`FixtureError::Io`] when the source cannot be read, when the destination's
    /// parent cannot be created or the copy cannot be written, and
    /// [`FixtureError::Unreadable`] when `src` is not a container this build reads. A source
    /// the loader already refuses would make every later assertion meaningless, so it is
    /// rejected here rather than mutated.
    pub fn from_compiled(src: &Path, dest: &Path) -> Result<Self, FixtureError> {
        let pristine = fs::read(src).map_err(|source| FixtureError::Io {
            path: src.to_path_buf(),
            source,
        })?;
        Reader::parse(pristine.as_slice(), Verify::Full).map_err(|cause| {
            FixtureError::Unreadable {
                path: src.to_path_buf(),
                cause,
            }
        })?;
        let created_parent = create_parent(dest)?;
        fs::write(dest, &pristine).map_err(|source| FixtureError::Io {
            path: dest.to_path_buf(),
            source,
        })?;
        Ok(Self {
            path: dest.to_path_buf(),
            created_parent,
            pristine,
            applied: None,
        })
    }

    /// The scratch copy.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The mutation currently applied, or `None` while the copy is pristine.
    pub fn applied(&self) -> Option<&DictMutation> {
        self.applied.as_ref()
    }

    /// Applies `mutation` to a fresh copy of the pristine image and writes it out.
    ///
    /// # Errors
    /// Returns [`FixtureError::InertMutation`] when the change would leave the container
    /// valid, [`FixtureError::Inapplicable`] when the container lacks the section, the entry
    /// or the key the mutation names, and [`FixtureError::Io`] when the copy cannot be
    /// written. A refused mutation leaves the copy exactly as it was.
    pub fn mutate(&mut self, mutation: DictMutation) -> Result<(), FixtureError> {
        let image = mutate::apply(&mutation, &self.pristine)?;
        // The catch-all: whatever a mutation meant to do, a change that produced the file it
        // started from has proved nothing and must not be reported as a case.
        if image == self.pristine {
            return Err(FixtureError::InertMutation {
                mutation: mutation.name(),
            });
        }
        fs::write(&self.path, &image).map_err(|source| FixtureError::Io {
            path: self.path.clone(),
            source,
        })?;
        self.applied = Some(mutation);
        Ok(())
    }

    /// Reads the mutated copy the way the loader does, and answers the first refusal.
    ///
    /// The container is opened first whatever the mutation's step is: a container the
    /// loader refuses never reaches an accessor, so a refusal at [`RefusalPoint::Load`] is
    /// reported for any mutation.
    ///
    /// # Errors
    /// Returns [`FixtureError::NoMutation`] when nothing has been applied yet,
    /// [`FixtureError::NotRejected`] when the container was accepted, and
    /// [`FixtureError::NonDictRefusal`] when a lookup was refused with a code that is not a
    /// dictionary failure.
    pub fn refusal(&self) -> Result<Refusal, FixtureError> {
        let applied = self
            .applied
            .as_ref()
            .ok_or_else(|| FixtureError::NoMutation {
                path: self.path.clone(),
            })?;
        let lexicon = match FstLexicon::load(&self.path) {
            Ok(lexicon) => lexicon,
            Err(error) => {
                return Ok(Refusal {
                    point: RefusalPoint::Load,
                    error,
                });
            }
        };
        // The step comes from the accessor that raised the refusal, never from what the
        // mutation declared: a step read off the mutation would make the check in
        // `assert_rejected` compare a value against itself.
        let refusal = match applied {
            DictMutation::EntryLength { index, .. } | DictMutation::EntryOffset { index, .. } => {
                self.refuse_record(&lexicon, *index)?
            }
            DictMutation::StrPoolTruncate { .. } => self.refuse_pool(&lexicon)?,
            DictMutation::WordlistRange { key } => self.refuse_lookup(&lexicon, key)?,
            // The load-time mutations: the load above accepted the container, so the
            // mutation was not caught.
            DictMutation::Magic
            | DictMutation::FormatVersion { .. }
            | DictMutation::Truncate { .. }
            | DictMutation::SectionCrc { .. }
            | DictMutation::EmptyFile
            | DictMutation::DeclaredLength { .. }
            | DictMutation::SectionLength { .. }
            | DictMutation::SectionOffset { .. }
            | DictMutation::EntryCount { .. }
            | DictMutation::Append { .. } => None,
        };
        refusal.ok_or(FixtureError::NotRejected {
            mutation: applied.name(),
        })
    }

    /// Asserts the mutated container is refused, at the step the mutation names and with
    /// the expected kind of error.
    ///
    /// Both halves are checked. A container refused at the wrong step is a case that proves
    /// nothing about the field it named: an entry the loader rejected at open would be
    /// caught by the header checks, and the record's own field would never be read. A case
    /// that also needs to pin *which* range was refused wants
    /// [`DictFixture::assert_rejected_field`], which layers that check on this one.
    ///
    /// # Errors
    /// Returns [`FixtureError::NoMutation`] when nothing has been applied,
    /// [`FixtureError::NotRejected`] when the container was accepted,
    /// [`FixtureError::WrongStep`] when the refusal came from another step, and
    /// [`FixtureError::WrongKind`] when it is another kind of error.
    pub fn assert_rejected(&self, expect: DictErrorKind) -> Result<(), FixtureError> {
        let applied = self
            .applied
            .as_ref()
            .ok_or_else(|| FixtureError::NoMutation {
                path: self.path.clone(),
            })?;
        let refusal = self.refusal()?;
        let expected = applied.refusal_point();
        if refusal.point != expected {
            return Err(FixtureError::WrongStep {
                mutation: applied.name(),
                expected,
                actual: refusal.point,
                cause: refusal.error,
            });
        }
        let actual = DictErrorKind::of(&refusal.error);
        if actual != expect {
            return Err(FixtureError::WrongKind {
                mutation: applied.name(),
                expected: expect,
                actual,
                cause: refusal.error,
            });
        }
        Ok(())
    }

    /// Asserts the mutated container is refused, at the step the mutation names, with the
    /// expected kind of error, and naming the expected field.
    ///
    /// The third half is what tells one bounds failure from another. Every length and every
    /// offset failure shares `DictError::LengthOutOfRange`, so a case that stopped at the
    /// variant would accept a container refused because the entry count disagrees with its
    /// table where the case meant to break a section length. A refusal of another kind is
    /// reported by the first check; a refusal of the right kind that names no field at all --
    /// a wrong magic, say -- is reported here, because the case asserted one.
    ///
    /// # Errors
    /// Returns the errors of [`DictFixture::assert_rejected`], and
    /// [`FixtureError::WrongField`] when the bounds failure names another field.
    pub fn assert_rejected_field(
        &self,
        expect: DictErrorKind,
        field: &'static str,
    ) -> Result<(), FixtureError> {
        self.assert_rejected(expect)?;
        let applied = self
            .applied
            .as_ref()
            .ok_or_else(|| FixtureError::NoMutation {
                path: self.path.clone(),
            })?;
        let refusal = self.refusal()?;
        let named = DictErrorKind::field(&refusal.error);
        if named != Some(field) {
            return Err(FixtureError::WrongField {
                mutation: applied.name(),
                expected: field,
                actual: refusal.error.to_string(),
            });
        }
        Ok(())
    }

    /// Reads the record at `index` out of the mutated copy and converts it into a word.
    ///
    /// The two errors the read path can raise are told apart here, and that is the point:
    /// the record's own validation runs first and rejects a `word_len` outside its contract,
    /// while a `word_off` outside the string pool survives the decode and is caught by the
    /// conversion. The step travels with the refusal, so a mutation that names one of them
    /// cannot be satisfied by the other.
    fn refuse_record(
        &self,
        lexicon: &FstLexicon,
        index: u32,
    ) -> Result<Option<Refusal>, FixtureError> {
        let reader = Reader::open(&self.path).map_err(|cause| self.unreadable(cause))?;
        let record = match reader.entry(index) {
            Ok(record) => record,
            Err(error) => {
                return Ok(Some(Refusal {
                    point: RefusalPoint::RecordDecode,
                    error,
                }));
            }
        };
        Ok(lexicon.entry_to_ref(&record).err().map(|error| Refusal {
            point: RefusalPoint::EntryToRef,
            error,
        }))
    }

    /// Converts every record of the mutated copy into a word, and answers the first refusal.
    ///
    /// A shortened string pool is the one fault the container's own parse cannot see: no
    /// field of the layout says how long the pool has to be, so a pool cut shorter than the
    /// records that name it is accepted at load and only a conversion can notice. The walk
    /// is therefore over the whole entry table rather than over one index, and it answers
    /// the first record the pool can no longer serve.
    ///
    /// # Errors
    /// Returns [`FixtureError::Unreadable`] when the copy cannot be read at all, which is a
    /// container this mutation did not produce.
    fn refuse_pool(&self, lexicon: &FstLexicon) -> Result<Option<Refusal>, FixtureError> {
        let reader = Reader::open(&self.path).map_err(|cause| self.unreadable(cause))?;
        for index in 0..reader.entry_count() {
            let record = reader
                .entry(index)
                .map_err(|cause| self.unreadable(cause))?;
            if let Err(error) = lexicon.entry_to_ref(&record) {
                return Ok(Some(Refusal {
                    point: RefusalPoint::EntryToRef,
                    error,
                }));
            }
        }
        Ok(None)
    }

    /// Reads the words of `key` and answers the refusal, if there is one.
    fn refuse_lookup(
        &self,
        lexicon: &FstLexicon,
        key: &str,
    ) -> Result<Option<Refusal>, FixtureError> {
        match lexicon.lookup(key) {
            Ok(_) => Ok(None),
            Err(ImeError::DictUnavailable { cause, .. }) => Ok(Some(Refusal {
                point: RefusalPoint::Lookup,
                error: cause,
            })),
            Err(other) => Err(FixtureError::NonDictRefusal {
                what: format!("the key {key:?}"),
                detail: other.to_string(),
            }),
        }
    }

    /// The refusal a container that could not be read is reported with.
    fn unreadable(&self, cause: DictError) -> FixtureError {
        FixtureError::Unreadable {
            path: self.path.clone(),
            cause,
        }
    }
}

impl Drop for DictFixture {
    /// Takes the scratch copy away, and the directory this fixture created for it.
    fn drop(&mut self) {
        // A case that already failed must not be turned into an abort by a second panic, so
        // the cleanup result is dropped rather than unwrapped.
        let _ = fs::remove_file(&self.path);
        if self.created_parent {
            // Only the directory this fixture made, and only while it is empty: a
            // `remove_dir` that finds anything else in it fails, which is the right outcome
            // for a directory another case is using.
            if let Some(parent) = self.path.parent() {
                let _ = fs::remove_dir(parent);
            }
        }
    }
}

/// One way of breaking a user store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserStoreMutation {
    /// A zero-byte file: the residue of a create interrupted before the store's own header
    /// reached the disk.
    ///
    /// The loader does *not* refuse this one. The embedded key-value store initialises a
    /// zero-length file rather than reporting it, which is exactly why the recovery pass
    /// reads the file's length before it opens it. The case asserts that difference instead
    /// of papering over it.
    Interrupted,
    /// A file of bytes that are not a store, which the loader refuses outright.
    Garbage,
}

impl UserStoreMutation {
    /// The name the shape of damage is reported under.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Interrupted => "interrupted",
            Self::Garbage => "garbage",
        }
    }
}

/// What the store loader did with a malformed store.
#[derive(Debug)]
pub enum StoreVerdict {
    /// The store opened. The handle is dropped before the verdict is returned, so the file
    /// is not left locked.
    Opened,
    /// The loader refused the file, with the error it reported.
    Refused(ImeError),
}

/// A malformed user store in a scratch directory.
///
/// The file is removed when the fixture drops. The directory is the caller's, so it is left
/// exactly as it was found apart from the store itself.
pub struct UserStoreFixture {
    /// The file the fixture placed.
    path: PathBuf,
    /// The shape of damage that was placed.
    mutation: UserStoreMutation,
}

impl UserStoreFixture {
    /// Places the store named by `mutation` in `dir`.
    ///
    /// # Errors
    /// Returns [`FixtureError::Io`] when `dir` cannot be created or the file cannot be
    /// written.
    pub fn create(dir: &Path, mutation: UserStoreMutation) -> Result<Self, FixtureError> {
        fs::create_dir_all(dir).map_err(|source| FixtureError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = dir.join(USER_STORE_NAME);
        let bytes = match mutation {
            UserStoreMutation::Interrupted => Vec::new(),
            UserStoreMutation::Garbage => mutate::garbage(mutate::GARBAGE_BYTES),
        };
        fs::write(&path, &bytes).map_err(|source| FixtureError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(Self { path, mutation })
    }

    /// The store the fixture placed.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The shape of damage that was placed.
    pub fn mutation(&self) -> UserStoreMutation {
        self.mutation
    }

    /// Opens the store the way the plugin does, and reports what happened.
    ///
    /// # Panics
    /// Never.
    pub fn verdict(&self) -> StoreVerdict {
        match UserDb::open(&self.path) {
            Ok(store) => {
                drop(store);
                StoreVerdict::Opened
            }
            Err(error) => StoreVerdict::Refused(error),
        }
    }

    /// Asserts the loader refused the store, with the expected kind of cause.
    ///
    /// # Errors
    /// Returns [`FixtureError::WrongVerdict`] when the loader opened the store,
    /// [`FixtureError::NonDictRefusal`] when it refused with a code that is not a dictionary
    /// failure, and [`FixtureError::WrongKind`] when the cause is another kind.
    pub fn assert_refused(&self, expect: DictErrorKind) -> Result<(), FixtureError> {
        let cause = match self.verdict() {
            StoreVerdict::Refused(ImeError::DictUnavailable { cause, .. }) => cause,
            StoreVerdict::Refused(other) => {
                return Err(FixtureError::NonDictRefusal {
                    what: format!("the {} store", self.mutation.name()),
                    detail: other.to_string(),
                });
            }
            StoreVerdict::Opened => {
                return Err(FixtureError::WrongVerdict {
                    mutation: self.mutation.name(),
                    expected: "refused",
                    actual: String::from("opened"),
                });
            }
        };
        let actual = DictErrorKind::of(&cause);
        if actual != expect {
            return Err(FixtureError::WrongKind {
                mutation: self.mutation.name(),
                expected: expect,
                actual,
                cause,
            });
        }
        Ok(())
    }

    /// Asserts the loader opened the store.
    ///
    /// # Errors
    /// Returns [`FixtureError::WrongVerdict`] when the loader refused the file.
    pub fn assert_opened(&self) -> Result<(), FixtureError> {
        match self.verdict() {
            StoreVerdict::Opened => Ok(()),
            StoreVerdict::Refused(error) => Err(FixtureError::WrongVerdict {
                mutation: self.mutation.name(),
                expected: "opened",
                actual: format!("was refused: {error}"),
            }),
        }
    }
}

impl Drop for UserStoreFixture {
    /// Takes the store away again.
    fn drop(&mut self) {
        // Dropped rather than unwrapped, for the reason [`DictFixture`]'s drop gives.
        let _ = fs::remove_file(&self.path);
    }
}

/// Creates `path`'s parent directory when it is missing.
///
/// # Return value
/// `true` when this call created it, which is what lets the fixture's drop take away a
/// directory it made and leave one the caller made alone.
///
/// # Errors
/// Returns [`FixtureError::Io`] when the directory cannot be created.
fn create_parent(path: &Path) -> Result<bool, FixtureError> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(false);
    };
    if parent.exists() {
        return Ok(false);
    }
    fs::create_dir_all(parent).map_err(|source| FixtureError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    Ok(true)
}
