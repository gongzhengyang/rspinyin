//! The v2 row extensions: the flags column and the quick-input and association drives.
//!
//! Responsibility: read the `flags:` column a v2 row carries, and drive the two
//! observation targets the matrix needs that a bare decode cannot reach -- the
//! quick-input panel and the post-commit association view. Everything here is glue
//! over public engine APIs: the decode flags ride the same
//! [`ime_types::DecodeFlags`] the scenario `[decode]` section names, the quick
//! recognition is `ime_core::quick`, and the association drive is the shipped session
//! state machine, so a green row says what a real session would do, not what a private
//! reimplementation does.
//!
//! # Why the flags column exists
//!
//! The matrix pins behaviours that are opt-in: fuzzy equivalence classes, the
//! abbreviation, spelling-correction and mixed-English switches, the quick panel and
//! association. A row that names none of them decodes exactly as a v1 row does, which
//! is the backward-compatibility contract: the flags column is additive, and a v1 row
//! never collides with it because no v1 tag begins with `flags:`.
//!
//! # The association drive
//!
//! Association is a session-machine behaviour, not a decode flag: its view opens after
//! the host confirms a commit landed. The drive therefore walks a small script -- type
//! a chunk, `<commit>`, `<assoc-select-N>` -- through `ime_core::state`, feeding the
//! same `CommitDone` confirmation a host feeds. `flags:assoc` drives the script with
//! the feature on; `flags:assoc-off` drives the same script with the shipped default,
//! which is what turns the pair into a controlled comparison. The provider set is
//! deliberately the empty one: the History family alone is production code that needs
//! no provider, and it is enough to pin the view's opening, its newest-first order,
//! its digit selection and its yield to typing. No association double is invented
//! here.

use anyhow::{Context, Result, bail, ensure};

use ime_core::assoc::{AssocProviders, AssocRuntime};
use ime_core::quick::{self, CivilTime};
use ime_core::state::{AssocSessionConfig, Session, SessionConfig, SessionEnv, SessionEvent, step};
use ime_types::{DecodeFlags, KeyAction};

use super::Mounted;

/// The date-family trigger spellings the quick rows are recognized against.
///
/// The shipped configuration's own default (`[quick_input] date_prefixes` in
/// `ime-config`), mirrored here so a quick row answers what a default installation
/// answers. A spelling outside this table is a parse error rather than a row that
/// silently decodes as pinyin while believing it named a panel.
pub const DATE_PREFIXES: [&str; 2] = ["rq", "dt"];

/// The decode switch names a flags column may carry.
///
/// The list is the intersection of the `[decode] flags` vocabulary and what the smoke
/// drive genuinely exercises: the fuzzy master and its eight classes, and the three
/// opt-in decode switches the matrix pins. A name outside it -- `USER_DICT` (already
/// the default), `SHUANGPIN` (needs a scheme the smoke mount does not carry),
/// `PHRASE`, `SCRIPT` (need data the smoke mount does not carry) -- is refused at load
/// with the allowed set named, because a switch that parsed but did nothing would pass
/// for the wrong reason.
const DRIVABLE_FLAG_NAMES: [&str; 12] = [
    "FUZZY",
    "FUZZY_ZH_Z",
    "FUZZY_CH_C",
    "FUZZY_SH_S",
    "FUZZY_N_L",
    "FUZZY_AN_ANG",
    "FUZZY_EN_ENG",
    "FUZZY_IN_ING",
    "FUZZY_F_H",
    "ABBREV",
    "CORRECTION",
    "ENGLISH",
];

/// The scenario switches a flags column may carry, and the decode flags of one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowFlags {
    /// The decode switches the row's requests carry, over the defaults.
    pub decode: DecodeFlags,
    /// The row observes the quick-input panel instead of a decode.
    pub quick: bool,
    /// The row's keys column is an association script, driven through the session
    /// machine.
    pub assoc: bool,
    /// The association feature's state during the drive. `assoc` sets it on;
    /// `assoc-off` keeps the shipped default while driving the same script, which is
    /// what makes the pair of rows a controlled comparison rather than two unrelated
    /// pins.
    pub assoc_enabled: bool,
}

impl Default for RowFlags {
    /// The v1 row: no switches, no scenario. Not derived, because the derived
    /// implementation would take `DecodeFlags::default()` -- whose default is the
    /// `USER_DICT` bit every request already carries -- and a row would then look
    /// switched-on when it named nothing.
    fn default() -> Self {
        Self {
            decode: DecodeFlags::empty(),
            quick: false,
            assoc: false,
            assoc_enabled: false,
        }
    }
}

impl RowFlags {
    /// Returns `true` when the row's requests need the English lexicon mounted.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn needs_english(self) -> bool {
        self.decode.contains(DecodeFlags::ENGLISH)
    }

    /// The flags column text a rewrite emits, or `None` for a plain v1 row.
    ///
    /// The order is the vocabulary's own, so a rewritten file diffs as little as the
    /// row's meaning allows. The `FUZZY` master is deliberately not emitted: the parse
    /// re-derives it from the class bits through the engine's projection, and a row
    /// naming it alone would be refused.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn column(self) -> Option<String> {
        let mut names: Vec<&str> = Vec::new();
        for (name, bit) in [
            ("FUZZY_ZH_Z", DecodeFlags::FUZZY_ZH_Z),
            ("FUZZY_CH_C", DecodeFlags::FUZZY_CH_C),
            ("FUZZY_SH_S", DecodeFlags::FUZZY_SH_S),
            ("FUZZY_N_L", DecodeFlags::FUZZY_N_L),
            ("FUZZY_AN_ANG", DecodeFlags::FUZZY_AN_ANG),
            ("FUZZY_EN_ENG", DecodeFlags::FUZZY_EN_ENG),
            ("FUZZY_IN_ING", DecodeFlags::FUZZY_IN_ING),
            ("FUZZY_F_H", DecodeFlags::FUZZY_F_H),
            ("ABBREV", DecodeFlags::ABBREV),
            ("CORRECTION", DecodeFlags::CORRECTION),
            ("ENGLISH", DecodeFlags::ENGLISH),
        ] {
            if self.decode.contains(bit) {
                names.push(name);
            }
        }
        if self.quick {
            names.push("quick");
        }
        if self.assoc {
            names.push(if self.assoc_enabled {
                "assoc"
            } else {
                "assoc-off"
            });
        }
        if names.is_empty() {
            None
        } else {
            Some(format!("flags:{}", names.join(",")))
        }
    }
}

/// Parses the `flags:` column of a v2 row.
///
/// # Errors
///
/// A column without the `flags:` prefix, an empty value, a name outside the drivable
/// vocabulary, and a master `FUZZY` with no class beside it (which the engine itself
/// reads as "off", so a row naming it alone would pin nothing) are each refused with
/// the reason named.
///
/// # Panics
///
/// Never.
pub fn parse_flags(column: &str) -> Result<RowFlags> {
    let value = column
        .strip_prefix("flags:")
        .with_context(|| format!("the column {column:?} does not carry the flags: prefix"))?;
    ensure!(
        !value.trim().is_empty(),
        "the flags column {column:?} names no flag"
    );
    let mut flags = RowFlags::default();
    let mut classes = DecodeFlags::empty();
    let mut switches = DecodeFlags::empty();
    for name in value.split(',') {
        let name = name.trim();
        ensure!(
            !name.is_empty(),
            "the flags column {column:?} holds an empty name"
        );
        match name {
            "quick" => flags.quick = true,
            "assoc" => {
                flags.assoc = true;
                flags.assoc_enabled = true;
            }
            "assoc-off" => flags.assoc = true,
            "ABBREV" => switches.insert(DecodeFlags::ABBREV),
            "CORRECTION" => switches.insert(DecodeFlags::CORRECTION),
            "ENGLISH" => switches.insert(DecodeFlags::ENGLISH),
            _ => {
                let Some(flag) = DecodeFlags::from_name(name) else {
                    bail!(
                        "unknown flag {name:?}; the drivable names are {} plus quick, \
                         assoc and assoc-off",
                        DRIVABLE_FLAG_NAMES.join(", ")
                    );
                };
                if flag == DecodeFlags::FUZZY {
                    bail!(
                        "a bare FUZZY master with no class bit is read as off by the engine, \
                         so the row would pin nothing; name the classes instead"
                    );
                }
                // The class table spells its names without the FUZZY_ prefix (the
                // configuration's own spelling), so the bit name maps to it by
                // stripping the prefix a bitflag necessarily carries.
                let class_name = name.strip_prefix("FUZZY_").unwrap_or(name);
                ensure!(
                    ime_core::fuzzy::class_by_name(class_name).is_some(),
                    "flag {name:?} parses but the smoke drive does not exercise it; \
                     the drivable names are {} plus quick, assoc and assoc-off",
                    DRIVABLE_FLAG_NAMES.join(", ")
                );
                classes.insert(flag);
            }
        }
    }
    // The class half goes through the engine's own projection, which adds the master
    // bit the classes imply, so a row cannot drift from what a configuration load
    // would have computed; the switch half is a plain union over it.
    flags.decode = ime_core::fuzzy::projection(false, classes).union(switches);
    Ok(flags)
}

/// Rejects a flags combination the row grammar does not allow.
///
/// # Errors
///
/// A row cannot observe two panels at once, a quick or association row cannot also
/// carry decode switches (their observation targets never read them), a `@today:` form
/// can only be answered by the quick clock, and an `@empty:` form can only be answered
/// by an association row (a decode is never empty -- the pass-through contract).
///
/// # Panics
///
/// Never.
pub fn check_row_cohesion(
    flags: &RowFlags,
    keys: &str,
    is_today: bool,
    is_empty: bool,
) -> Result<()> {
    if flags.quick && flags.assoc {
        bail!("a row cannot observe the quick panel and the association view at once");
    }
    if flags.quick && !flags.decode.is_empty() {
        bail!("a quick row's panel never reads decode flags; drop the switch names");
    }
    if flags.assoc && !flags.decode.is_empty() {
        bail!(
            "an association row's session decodes with the shipped configuration only; \
             drop the switch names"
        );
    }
    if is_today && !flags.quick {
        bail!("a @today form pins the quick clock, so the row must carry flags:quick");
    }
    if is_empty && !flags.assoc {
        bail!(
            "a decode or quick answer is never empty, so @empty only pins an \
             association row; carry flags:assoc or flags:assoc-off"
        );
    }
    if flags.quick {
        ensure!(
            DATE_PREFIXES.contains(&keys),
            "the quick trigger {keys:?} is not a spelling the shipped prefix table \
             ({}) names",
            DATE_PREFIXES.join(", ")
        );
    }
    Ok(())
}

/// One move of an association row's script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssocStep {
    /// Types a pinyin chunk, one character per step, the way a user types.
    Type(String),
    /// Commits the highlighted candidate, then feeds the landed-commit
    /// confirmation a host feeds -- the confirmation is what may open a view.
    Commit,
    /// Selects the candidate a digit names while a view is live, then feeds the
    /// same confirmation for the selection's commit.
    Select(u8),
}

/// Parses the keys column of an association row into its script.
///
/// # Errors
///
/// An unknown marker, a digit outside `1..=9`, and a chunk that is not a pinyin key
/// sequence are each refused with the token named.
///
/// # Panics
///
/// Never.
pub fn parse_script(keys: &str) -> Result<Vec<AssocStep>> {
    ensure!(!keys.trim().is_empty(), "the association script is empty");
    let mut script = Vec::new();
    for token in keys.split_whitespace() {
        if let Some(marker) = token
            .strip_prefix('<')
            .and_then(|rest| rest.strip_suffix('>'))
        {
            if marker == "commit" {
                script.push(AssocStep::Commit);
                continue;
            }
            if let Some(digits) = marker.strip_prefix("assoc-select-") {
                let digit = digits
                    .parse::<u8>()
                    .ok()
                    .filter(|digit| (1..=9).contains(digit))
                    .with_context(|| format!("the marker {token:?} must name a digit 1..=9"))?;
                script.push(AssocStep::Select(digit));
                continue;
            }
            bail!(
                "unknown association marker {token:?}; the markers are <commit> and <assoc-select-1..9>"
            );
        }
        super::ensure_keys(token)
            .with_context(|| format!("the association script chunk {token:?}"))?;
        script.push(AssocStep::Type(String::from(token)));
    }
    ensure!(
        script.iter().any(|step| matches!(step, AssocStep::Type(_))),
        "the association script {keys:?} types nothing"
    );
    Ok(script)
}

/// Renders the quick panel's candidate texts for one trigger.
///
/// The civil reading arrives already taken from the run's clock, so the panel's date
/// renderings answer that fixed reading and a test never meets the wall clock.
///
/// # Panics
///
/// Never.
pub fn quick_texts(keys: &str, civil: CivilTime) -> Vec<String> {
    let matched = quick::recognize(true, keys, &DATE_PREFIXES, false);
    let Some(matched) = matched else {
        // Unreachable for a row that parsed: the trigger was validated against the
        // prefix table at load. An empty answer is the honest observation anyway.
        return Vec::new();
    };
    quick::candidates(matched, keys, Some(&civil))
        .into_iter()
        .map(|candidate| candidate.text)
        .collect()
}

/// Drives one association script through the shipped session machine and answers the
/// candidate texts the final state shows.
///
/// The configuration is the shipped default with the association switch set from
/// `enabled`, so the row's answer is what a default installation with the feature
/// toggled would show. The drive is total: a key the machine cannot act on answers an
/// empty effect list, and nothing here can panic.
///
/// # Panics
///
/// Never.
pub fn assoc_texts(mounted: &Mounted, script: &[AssocStep], enabled: bool) -> Vec<String> {
    let mut session = Session::new();
    let cfg = SessionConfig {
        assoc: AssocSessionConfig {
            enabled,
            ..AssocSessionConfig::default()
        },
        ..SessionConfig::default()
    };
    let user = super::FreshUser;
    let env = SessionEnv {
        decoder: &mounted.decoder,
        lexicon: &mounted.lexicon,
        user_freq: &user,
        lm: &mounted.lm,
        english: None,
        confusables: None,
        symbols: None,
        assoc: Some(AssocRuntime {
            providers: AssocProviders::none(),
            sensitive: false,
        }),
    };
    for move_step in script {
        match move_step {
            AssocStep::Type(chunk) => {
                for ch in chunk.chars() {
                    let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
                }
            }
            AssocStep::Commit => {
                let _ = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);
                step(&mut session, SessionEvent::CommitDone, &cfg, &env);
            }
            AssocStep::Select(digit) => {
                let _ = session.handle_key(KeyAction::SelectIndex(*digit), &cfg, &env);
                step(&mut session, SessionEvent::CommitDone, &cfg, &env);
            }
        }
    }
    session
        .decoded()
        .candidates
        .iter()
        .map(|candidate| candidate.text.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use ime_types::DecodeFlags;

    use super::{AssocStep, RowFlags, check_row_cohesion, parse_flags, parse_script};

    #[test]
    fn test_parse_flags_reads_switch_names_and_scenario_switches() {
        let flags = parse_flags("flags:ABBREV,CORRECTION").expect("switch names parse");
        assert!(flags.decode.contains(DecodeFlags::ABBREV));
        assert!(flags.decode.contains(DecodeFlags::CORRECTION));
        assert!(!flags.quick && !flags.assoc);

        let fuzzy = parse_flags("flags:FUZZY_N_L,FUZZY_AN_ANG").expect("class names parse");
        assert!(fuzzy.decode.contains(DecodeFlags::FUZZY_N_L));
        assert!(fuzzy.decode.contains(DecodeFlags::FUZZY_AN_ANG));
        // The master the classes imply is the engine projection's own doing.
        assert!(fuzzy.decode.contains(DecodeFlags::FUZZY));

        let assoc = parse_flags("flags:assoc").expect("assoc parses");
        assert!(assoc.assoc && assoc.assoc_enabled);
        assert!(!assoc.quick && assoc.decode.is_empty());
        let assoc_off = parse_flags("flags:assoc-off").expect("assoc-off parses");
        assert!(assoc_off.assoc && !assoc_off.assoc_enabled);
        let quick = parse_flags("flags:quick").expect("quick parses");
        assert!(quick.quick && !quick.assoc);
    }

    #[test]
    fn test_flags_column_round_trips_through_the_parser() {
        for column in [
            "flags:ABBREV",
            "flags:FUZZY_N_L,FUZZY_AN_ANG",
            "flags:ENGLISH",
            "flags:quick",
            "flags:assoc",
            "flags:assoc-off",
        ] {
            let flags = parse_flags(column).expect("the column parses");
            assert_eq!(
                flags.column().as_deref(),
                Some(column),
                "{column} round-trips"
            );
        }
        // A class-only row re-emits without the master, which is what the parser
        // refuses when written by hand.
        assert_eq!(
            parse_flags("flags:FUZZY_N_L")
                .expect("parses")
                .column()
                .as_deref(),
            Some("flags:FUZZY_N_L")
        );
        assert_eq!(
            RowFlags::default().column(),
            None,
            "a v1 row emits no column"
        );
    }

    #[test]
    fn test_parse_flags_refuses_unknown_undrivable_and_empty_names() {
        let error = parse_flags("flags:NO_SUCH_BIT").expect_err("unknown name");
        assert!(error.to_string().contains("unknown flag"), "{error}");
        let error = parse_flags("flags:USER_DICT").expect_err("the default switch is not drivable");
        assert!(error.to_string().contains("does not exercise"), "{error}");
        let error = parse_flags("flags:FUZZY").expect_err("a bare master pins nothing");
        assert!(error.to_string().contains("bare FUZZY"), "{error}");
        let error = parse_flags("flags:").expect_err("an empty column names nothing");
        assert!(error.to_string().contains("names no flag"), "{error}");
        let error = parse_flags("ABBREV").expect_err("the prefix is required");
        assert!(error.to_string().contains("flags: prefix"), "{error}");
        assert!(
            parse_flags("flags:ENGLISH,assoc,quick").is_ok(),
            "mixed flags parse"
        );
    }

    #[test]
    fn test_row_cohesion_couples_forms_to_their_observation_target() {
        let quick = parse_flags("flags:quick").expect("quick parses");
        assert!(
            check_row_cohesion(&quick, "rq", true, false).is_ok(),
            "a quick row may pin the clock"
        );
        let assoc = parse_flags("flags:assoc").expect("assoc parses");
        assert!(
            check_row_cohesion(&assoc, "nihao <commit>", false, true).is_ok(),
            "an assoc row may pin emptiness"
        );
        let plain = RowFlags::default();
        let error = check_row_cohesion(&plain, "rq", true, false)
            .expect_err("@today needs the quick clock");
        assert!(error.to_string().contains("flags:quick"), "{error}");
        let error = check_row_cohesion(&plain, "nihao", false, true)
            .expect_err("@empty needs an association row");
        assert!(error.to_string().contains("flags:assoc"), "{error}");
        let error = check_row_cohesion(&quick, "sj", false, false)
            .expect_err("an unshipped trigger is refused");
        assert!(error.to_string().contains("prefix table"), "{error}");
        let both = RowFlags {
            quick: true,
            assoc: true,
            ..RowFlags::default()
        };
        let error = check_row_cohesion(&both, "rq", false, false).expect_err("two panels at once");
        assert!(error.to_string().contains("at once"), "{error}");
        let quick_and_flag = RowFlags {
            quick: true,
            decode: DecodeFlags::ABBREV,
            ..RowFlags::default()
        };
        let error = check_row_cohesion(&quick_and_flag, "rq", false, false)
            .expect_err("a quick row reads no decode flags");
        assert!(
            error.to_string().contains("never reads decode flags"),
            "{error}"
        );
        let assoc_and_flag = RowFlags {
            assoc: true,
            decode: DecodeFlags::ENGLISH,
            ..RowFlags::default()
        };
        let error = check_row_cohesion(&assoc_and_flag, "nihao <commit>", false, false)
            .expect_err("an assoc row decodes with the shipped configuration only");
        assert!(
            error.to_string().contains("shipped configuration"),
            "{error}"
        );
    }

    #[test]
    fn test_parse_script_reads_chunks_and_markers_in_order() {
        let script =
            parse_script("nihao <commit> zhongguo <assoc-select-1>").expect("the script parses");
        assert_eq!(
            script,
            vec![
                AssocStep::Type(String::from("nihao")),
                AssocStep::Commit,
                AssocStep::Type(String::from("zhongguo")),
                AssocStep::Select(1),
            ]
        );
    }

    #[test]
    fn test_parse_script_refuses_unknown_markers_and_bare_selects() {
        let error = parse_script("nihao <assoc-grab-1>").expect_err("an unknown marker");
        assert!(
            error.to_string().contains("unknown association marker"),
            "{error}"
        );
        let error = parse_script("nihao <assoc-select-0>").expect_err("digit zero");
        assert!(error.to_string().contains("1..=9"), "{error}");
        let error = parse_script("nihao <assoc-select-12>").expect_err("two digits");
        assert!(error.to_string().contains("1..=9"), "{error}");
        let error = parse_script("<commit>").expect_err("a script that types nothing");
        assert!(error.to_string().contains("types nothing"), "{error}");
        let error = parse_script("NI <commit>").expect_err("uppercase is not a pinyin key");
        assert!(format!("{error:#}").contains("pinyin key"), "{error:#}");
    }
}
