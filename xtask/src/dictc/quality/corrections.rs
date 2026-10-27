//! The correction table: where each reading came from, and the table itself.
//!
//! Split out of `quality.rs` to keep that file inside the line limit. The provenance
//! column is the licence story in data form: a reading this project wrote and one
//! carried over from a permissively licensed source travel in the same table but never
//! blur together.

use super::*;

/// Where a correction row's reading came from.
///
/// The column exists so that a reader of the table can tell a reading derived from a
/// permissively licensed source from one this project wrote, without consulting the
/// allowlist: `unihan` rows carry the Unicode licence and no further obligation,
/// `manual` rows are project-owned, and `corpus` rows are counted from the project's own
/// word list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Derived from `data/raw/unihan.tsv`.
    Unihan,
    /// Written by hand for this project.
    Manual,
    /// Counted from the project's own word list.
    Corpus,
    /// Carried over from the huayu word list's own readings.
    Huayu,
}

impl Origin {
    /// Parses the `origin` column.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "unihan" => Some(Self::Unihan),
            "manual" => Some(Self::Manual),
            "corpus" => Some(Self::Corpus),
            "huayu" => Some(Self::Huayu),
            _ => None,
        }
    }
}

/// One row of the L3c correction table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Correction {
    /// The word the row corrects.
    pub word: String,
    /// The reading the row states, spelled as the file spells it.
    pub reading: String,
    /// The weight column; absent in the two-column form.
    pub weight: Option<u32>,
    /// The origin column; absent in the two- and three-column forms.
    pub origin: Option<Origin>,
}

/// The origin mix of one correction table.
///
/// A row with no `origin` column is counted separately rather than folded onto one of the
/// three values: "this row states no origin" and "this row states that it came from
/// Unihan" are different claims, and only one of them is a licence statement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OriginMix {
    /// Rows carrying no `origin` column.
    pub unspecified: u32,
    /// Rows marked `unihan`.
    pub unihan: u32,
    /// Rows marked `manual`.
    pub manual: u32,
    /// Rows marked `corpus`.
    pub corpus: u32,
    /// Rows marked `huayu`.
    pub huayu: u32,
}

impl OriginMix {
    /// Tallies one table.
    pub fn of(rows: &[Correction]) -> Self {
        let mut mix = Self::default();
        for row in rows {
            mix.record(row.origin);
        }
        mix
    }

    /// Records one row's origin.
    pub fn record(&mut self, origin: Option<Origin>) {
        let slot = match origin {
            None => &mut self.unspecified,
            Some(Origin::Unihan) => &mut self.unihan,
            Some(Origin::Manual) => &mut self.manual,
            Some(Origin::Corpus) => &mut self.corpus,
            Some(Origin::Huayu) => &mut self.huayu,
        };
        *slot = slot.saturating_add(1);
    }

    /// Rows the mix accounts for.
    pub fn total(&self) -> u32 {
        self.unspecified
            .saturating_add(self.unihan)
            .saturating_add(self.manual)
            .saturating_add(self.corpus)
            .saturating_add(self.huayu)
    }

    /// Rows of one origin.
    pub fn count(&self, origin: Origin) -> u32 {
        match origin {
            Origin::Unihan => self.unihan,
            Origin::Manual => self.manual,
            Origin::Corpus => self.corpus,
            Origin::Huayu => self.huayu,
        }
    }
}

/// Reads the L3c correction table.
///
/// Three column counts are accepted, and they are the three the format has had:
/// `word<TAB>reading` (the original), `word<TAB>reading<TAB>weight`, and
/// `word<TAB>reading<TAB>weight<TAB>origin`. A row reads the same way whatever its column
/// count, so a table that gains the `origin` column keeps compiling exactly as it did
/// before, which is what lets the column be added without a flag day. A row past the
/// fourth column is refused rather than ignored: an unread column is a column whose
/// content nobody validated, and an origin nobody validated is worse than none.
///
/// The reading itself is not re-parsed here. The compiler's own reader asserts that every
/// row is a legal syllable sequence for its word before the table can affect a build, and
/// a second implementation of that check would be a second answer to the same question.
///
/// # Errors
///
/// Returns an error when the file cannot be read or has not been fetched, when a row
/// holds fewer than two or more than [`MAX_CORRECTION_COLUMNS`] columns, when its word or
/// reading column is empty, when the weight column is not an unsigned integer, or when
/// the origin column is not one of `unihan` / `manual` / `corpus`.
pub fn load_corrections(path: &Path) -> Result<Vec<Correction>> {
    let text = crate::dictc::source::read_source(path)?;
    let mut rows = Vec::new();
    for (number, line) in data_lines(&text) {
        let columns: Vec<&str> = line.split('\t').map(str::trim).collect();
        ensure!(
            (2..=MAX_CORRECTION_COLUMNS).contains(&columns.len()),
            "{}:{number}: a correction row holds 2 to {MAX_CORRECTION_COLUMNS} columns, \
             found {}",
            path.display(),
            columns.len()
        );
        let word = columns.first().copied().unwrap_or_default();
        let reading = columns.get(1).copied().unwrap_or_default();
        ensure!(
            !word.is_empty() && !reading.is_empty(),
            "{}:{number}: a correction row needs a word column and a reading column",
            path.display()
        );
        let weight = match columns.get(2).copied() {
            Some(raw) if !raw.is_empty() => parse_weight(path, number, raw)?,
            _ => None,
        };
        let origin = match columns.get(3).copied() {
            Some(raw) if !raw.is_empty() => Some(parse_origin(path, number, raw)?),
            _ => None,
        };
        rows.push(Correction {
            word: word.to_owned(),
            reading: reading.to_owned(),
            weight,
            origin,
        });
    }
    Ok(rows)
}
