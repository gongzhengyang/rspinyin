//! Attribution: telling a locator that went stale from a product that broke.
//!
//! Responsibility: read the structured facts two channels already produced -- the locator
//! channel's recoveries and refusals, the frame channel's revision, the metrics cross-check, and
//! the evidence channel's failed assertions and image defects -- and answer with one class and one
//! executable prompt. Nothing here reads a file, opens a display, starts a process or reads a
//! clock: every fact arrives as a value, which is what lets the whole module be tested with no
//! display server and no Fcitx5 session.
//!
//! # Why a class has to come before a repair
//!
//! A case that fails intermittently has two causes that are specific to an input method, and
//! repairing the wrong one is worse than not repairing at all:
//!
//! * the candidate window is placed from the cursor's position, which is derived from the client
//!   window's geometry, so an assertion written against absolute screen pixels stops holding when
//!   anything in that chain moves;
//! * a frame carries a monotonic revision and the UI side drops a frame older than the one it
//!   holds, so a case that asserts on whichever frame a replay happened to leave in the mirror
//!   fails on a loaded machine and passes on an idle one.
//!
//! Both are test defects, and both are repaired in the test script. The third possibility is that
//! the product is wrong, and repairing that by editing the case is the most dangerous failure this
//! harness has: it reports green for a broken product. So the class is decided first, from
//! evidence, and a repair is only ever produced *by* a class -- [`Attribution::repair`] answers
//! `None` for the two classes that have none, which makes "a repair without a class" unexpressible
//! rather than merely forbidden.
//!
//! # The classes, in the order they win
//!
//! | Class | What the evidence says | What is done about it |
//! |---|---|---|
//! | [`Attribution::ProductDefect`] | a source or the window disagrees with the specification | nothing: the finding is reported |
//! | [`Attribution::StaleLocator`] | a name the case used is gone | it is re-derived from the specification |
//! | [`Attribution::RevisionRace`] | the frame asserted on is not the frame awaited | the revision is awaited |
//! | [`Attribution::CoordinateDrift`] | the point recorded is not the point derived | the geometry relation is asserted |
//! | [`Attribution::InsufficientEvidence`] | none of the above | nothing: the gaps are named |
//!
//! More than one class can be supported by one case -- a renamed constant moves every point
//! derived from it, and the locator channel records both -- so the order above is a precedence and
//! not a partition. The declaration order of [`Attribution`] *is* that precedence, [`attribute`]
//! sorts by it, and a class that is supported but outranked is carried in
//! [`Diagnosis::secondary`] rather than dropped. The order is not alphabetical and not by
//! frequency: a finding about the product outranks every repair, a name that is gone is a more
//! specific statement about the script than a point that moved, and a frame that is not the one
//! awaited makes every coordinate read from it meaningless.
//!
//! # The one answer that is never guessed
//!
//! [`Attribution::InsufficientEvidence`] is a reachable conclusion and not a failure to reach one:
//! when nothing in the evidence names a cause, [`attribute`] says so and the prompt lists the facts
//! that are missing and what would fill them. Guessing a class there would turn the attribution
//! itself into a source of false information -- a reader would repair a case on the strength of a
//! class nothing supports -- so the class with no evidence behind it is a class, and its prompt
//! asks for evidence rather than for a change.
//!
//! # The language of the prompt
//!
//! The prompt is the human-facing half of the self-healing template and is written in Chinese, as
//! that template is; everything the code names -- types, functions, fields, this documentation --
//! is English, as the project's conventions require. The red lines the prompt carries are rendered
//! from `crate::testd::guard`'s own constants rather than spelled again here, so the prompt cannot
//! name an allowlist the gate would refuse.
//!
//! # Modules
//!
//! [`ground`] holds the facts a class rests on and the facts the evidence does not hold.

// Nothing in the crate names this module yet: the case runner that would collect the facts and the
// subcommand that would print a prompt both live in files this one does not own. Until that wiring
// lands, every item here is reported as dead code in a non-test build, and the attribute goes away
// with those lines.
//
// `unused_imports` is covered by the same reasoning and for the same reason: the `pub use` line
// below is this module's surface, and a `pub use` in a *binary* crate is "unused" whenever nothing
// in the crate names it.
#![allow(dead_code, unused_imports)]

mod ground;

#[cfg(test)]
mod tests;

pub use self::ground::Ground;

use crate::testd::evidence::{AssertionDiff, ImageDefect, RESULTS_DIR, RUNS_DIR};
use crate::testd::guard::{BUSINESS_CODE_PREFIXES, FROZEN_FILES, HEAL_ALLOWED_PREFIXES};
use crate::testd::heal::{HealError, HealRecord};
use crate::testd::ui_metrics::MetricMismatch;

/// What the evidence adds up to, and in which order the classes win.
///
/// The declaration order is the precedence order, and that is a contract rather than an accident:
/// [`attribute`] sorts the classes the evidence supports by it, so the first variant wins whenever
/// two are supported at once. The module documentation says why each class outranks the one below
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Attribution {
    /// A source or the window disagrees with the specification.
    ///
    /// This is a finding about the product: it is never repaired here, because the one class of
    /// regression the specification exists to catch is the one class a repair could cover.
    ProductDefect,
    /// A name the case used is gone from the source.
    ///
    /// A finding about the test script, and the most specific one: it names the identifier that
    /// is wrong, and the specification supplies the replacement.
    StaleLocator,
    /// The frame the case asserted on is not the frame it awaited.
    ///
    /// A finding about *when* the case read. It outranks a coordinate that drifted because a
    /// stale frame's coordinates describe a frame the window never drew, so re-projecting them
    /// would be repairing the wrong thing.
    RevisionRace,
    /// The point the case recorded is not the point the specification produces.
    ///
    /// A finding about what the case recorded, which the geometry relation repairs.
    CoordinateDrift,
    /// The evidence names no cause.
    InsufficientEvidence,
}

/// What is done about a finding that is about the product.
const DISPOSITION_PRODUCT_DEFECT: &str = "\
这是关于产品的发现，不是 flaky：停止自愈，不产出任何修复建议。下一步：把「证据」段整段交给主 Agent，\
并附上本用例证据目录里的 assertions.json、trace.json 与截图；不要改断言、不要改阈值、不要改产品代码。";

/// What is done about a name the case used that is gone.
const DISPOSITION_STALE_LOCATOR: &str = "\
用例用的常量名已经过期，自愈通道已按规格重新推导出新的名字（见「证据」）。下一步：把用例里那一处旧名\
改成重推导出的新名，再重跑该用例；重跑仍失败时不要再动名字，按下面的红线交给主 Agent。";

/// What is done about a frame that is not the one awaited.
const DISPOSITION_REVISION_RACE: &str = "\
用例断言的那一帧不是它等的那一帧。下一步：把读快照的那一处换成 FrameWatch（见 \
xtask/src/testd/uiframe/mirror.rs），等到 revision 单调递增到期望值再断言、超时才算失败，不要用 \
sleep；若用例本来就在等而超时了，说明那一帧没有在 deadline 内出现，先用 runtime://ui_metrics 与引擎\
的延迟预算量一次，再把测量结果交给主 Agent。";

/// What is done about a point that drifted.
const DISPOSITION_COORDINATE_DRIFT: &str = "\
用例记录的点已经不是规格常量算出来的点。下一步：把绝对像素断言换成基于 hit_map 的相对坐标加几何\
关系断言（候选框水平中心 ≈ 光标水平中心 ± 2px、垂直紧贴 ± 4px），屏幕坐标的换算交给 \
xtask/src/testd/coords.rs，不要在用例里自己算。";

/// What is done when the evidence names no cause.
const DISPOSITION_INSUFFICIENT_EVIDENCE: &str = "\
两个通道都没有给出能归因的事实，因此不产出修复建议，也不给类别以外的结论——不猜。下一步：按「证据」\
段列出的缺口补齐事实（补齐后用同一份 facts 再调一次 attribute），再重跑该用例。";

impl Attribution {
    /// The class as the prompt writes it: the letter the template fixes for the class, and its
    /// Chinese name.
    ///
    /// The letters are the ones the self-healing template fixes for the three classes it names, so
    /// a reader who knows the template recognises the class the prompt reports.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::ProductDefect => "C 真实缺陷",
            Self::StaleLocator => "D 定位器常量过期",
            Self::RevisionRace => "B revision 竞态",
            Self::CoordinateDrift => "A 坐标漂移",
            Self::InsufficientEvidence => "E 证据不足",
        }
    }

    /// The class as the code names it, for a report a tool reads rather than a person.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn code(self) -> &'static str {
        match self {
            Self::ProductDefect => "ProductDefect",
            Self::StaleLocator => "StaleLocator",
            Self::RevisionRace => "RevisionRace",
            Self::CoordinateDrift => "CoordinateDrift",
            Self::InsufficientEvidence => "InsufficientEvidence",
        }
    }

    /// What to do about this class, as the prompt's disposition section writes it.
    ///
    /// Every class has one, because "there is nothing to repair here" is itself a disposition and
    /// not a missing answer: the two classes that propose no repair say so and name what is done
    /// instead.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn disposition(self) -> &'static str {
        match self {
            Self::ProductDefect => DISPOSITION_PRODUCT_DEFECT,
            Self::StaleLocator => DISPOSITION_STALE_LOCATOR,
            Self::RevisionRace => DISPOSITION_REVISION_RACE,
            Self::CoordinateDrift => DISPOSITION_COORDINATE_DRIFT,
            Self::InsufficientEvidence => DISPOSITION_INSUFFICIENT_EVIDENCE,
        }
    }

    /// The repair this class proposes, or `None` when it proposes none.
    ///
    /// This is the whole of "a class before a repair": the answer is a function of the class, so
    /// there is no way to obtain one without the other, and a caller that wants a repair has to
    /// name the class it believes in first.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn repair(self) -> Option<&'static str> {
        match self {
            Self::ProductDefect | Self::InsufficientEvidence => None,
            Self::StaleLocator | Self::RevisionRace | Self::CoordinateDrift => {
                Some(self.disposition())
            }
        }
    }
}

/// The case the facts belong to, as the evidence layout names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseRef {
    /// The case's identifier, e.g. `TC-CORE-01`.
    pub tc: String,
    /// The module code the case belongs to, e.g. `core`.
    pub module: String,
}

impl CaseRef {
    /// The directory the case's evidence bundle is written to.
    ///
    /// The run's own directory is named for the instant the run started, which nothing here knows,
    /// so the path is rendered with `<run>` in its place: a reader opens the newest directory
    /// under the runs directory and finds the case inside it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn bundle_dir(&self) -> String {
        format!(
            "{RESULTS_DIR}/{RUNS_DIR}/<run>/{}/{}/",
            self.module, self.tc
        )
    }

    /// The case as the prompt's own line writes it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn line(&self) -> String {
        format!(
            "用例：{} / {}，证据目录 `{}`",
            self.module,
            self.tc,
            self.bundle_dir()
        )
    }
}

/// What the locator channel did for one case.
///
/// The two lists are the channel's two outcomes and not a summary of them: a recovery is a name
/// that had to be re-derived, and a refusal is a locator that would not guess. Both are kept in
/// the order the case met them, because a case can meet several.
#[derive(Debug, Default, PartialEq)]
pub struct LocatorFacts {
    /// The recoveries the case's locators made, oldest first.
    pub healed: Vec<HealRecord>,
    /// The refusals the case's locators met, in the order they met them.
    pub refusals: Vec<HealError>,
}

/// What the frame channel saw of the revision the case asserted on.
///
/// The case's own expectation is part of the record because the channel cannot know it: the mirror
/// reports the revision the file holds, and only the case knows which revision it was waiting for.
/// A race is therefore a disagreement between the two, or a file that went backwards, and never a
/// number on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevisionFacts {
    /// The revision the case awaited.
    pub expected: u32,
    /// The revision the snapshot carried when the case asserted on it.
    pub seen: u32,
    /// How many times the file offered a revision older than the one already read.
    pub rewinds: u64,
}

impl RevisionFacts {
    /// Whether the case asserted on a frame other than the one it awaited.
    ///
    /// Three readings say yes, and each is the same finding seen from a different side: the frame
    /// was older than the awaited one, so it had not arrived; it was newer, so what the case
    /// asserted on is not what it meant to assert on; or the file offered a revision older than
    /// one already read, which is a replay or a reorder. A file that went backwards is reported
    /// even when the revision the case finally read is the awaited one, because the stale frame
    /// was seen.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn is_stale(&self) -> bool {
        self.seen != self.expected || self.rewinds > 0
    }
}

/// The structured facts one failing case's channels already produced.
///
/// Every field is a value a channel handed over, and the module reads nothing else. The defaults
/// are the empty answer -- no recovery, no refusal, no reading, nothing compared -- which is the
/// state a case that failed without any of the channels having run leaves behind.
#[derive(Debug, Default, PartialEq)]
pub struct AttributionFacts {
    /// The case the facts belong to, when the caller knows it.
    pub case: Option<CaseRef>,
    /// What the locator channel did.
    pub locator: LocatorFacts,
    /// What the frame channel saw of the revision, for a case that asserts on a frame.
    pub revision: Option<RevisionFacts>,
    /// What the metrics cross-check found, in the order it reported it.
    pub mismatches: Vec<MetricMismatch>,
    /// The assertions that did not hold, as the case's trace holds them.
    pub diffs: Vec<AssertionDiff>,
    /// Where a snapshot held something the specification does not state.
    pub defects: Vec<ImageDefect>,
}

/// What one failing case's evidence adds up to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnosis {
    /// The class the evidence supports.
    class: Attribution,
    /// The case the diagnosis is about, when the caller named it.
    case: Option<CaseRef>,
    /// The facts the class and the failure rest on, in the order they were read.
    grounds: Vec<Ground>,
    /// The classes the evidence also supports, in precedence order.
    secondary: Vec<Attribution>,
}

impl Diagnosis {
    /// The class the evidence supports.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn class(&self) -> Attribution {
        self.class
    }

    /// The case the diagnosis is about, or `None` when the caller named none.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn case(&self) -> Option<&CaseRef> {
        self.case.as_ref()
    }

    /// The facts the diagnosis rests on, the failed assertions last.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn grounds(&self) -> &[Ground] {
        &self.grounds
    }

    /// The classes the evidence also supports, in precedence order.
    ///
    /// Empty when the evidence supports one class or none. A class here is not a weaker guess: it
    /// is supported by the evidence and lost the precedence, so its own disposition is what a
    /// reader applies once the reported class is settled.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn secondary(&self) -> &[Attribution] {
        &self.secondary
    }

    /// The repair the diagnosis proposes, or `None` when its class proposes none.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn repair(&self) -> Option<&'static str> {
        self.class.repair()
    }

    /// The diagnosis as the prompt a reader acts on.
    ///
    /// The four sections are the ones the self-healing template fixes: the conclusion, the
    /// evidence it rests on, what is done about it, and the red lines. The conclusion comes first
    /// by construction, so a repair is never read before the class it belongs to.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn prompt(&self) -> String {
        let mut text = String::from("# 归因与自愈\n\n## 结论\n");
        line(
            &mut text,
            &format!("类别：{}（`{}`）", self.class.label(), self.class.code()),
        );
        if let Some(case) = &self.case {
            line(&mut text, &case.line());
        }
        if !self.secondary.is_empty() {
            let labels = self
                .secondary
                .iter()
                .map(|class| class.label())
                .collect::<Vec<&str>>()
                .join("、");
            line(
                &mut text,
                &format!("同时成立、按优先级排在后面的类别：{labels}"),
            );
        }
        text.push_str("\n## 证据\n");
        for ground in &self.grounds {
            line(&mut text, &ground.line());
        }
        text.push_str("\n## 处置\n");
        text.push_str(self.class.disposition());
        text.push_str("\n\n");
        text.push_str(&red_lines());
        text
    }
}

/// Reads `facts` and answers with the class they support and the prompt that follows from it.
///
/// # Parameters
///
/// * `facts` -- what the locator channel, the frame channel, the metrics cross-check and the
///   evidence channel produced for one failing case.
///
/// # Returns
///
/// The diagnosis. Its class is the first of the supported classes in the order [`Attribution`]
/// declares, its grounds are the facts that class rests on followed by the assertions the case
/// made and did not hold, and the classes it outranked are in [`Diagnosis::secondary`]. Evidence
/// that supports no class at all is answered with [`Attribution::InsufficientEvidence`] and the
/// facts the evidence does not hold.
///
/// # Panics
///
/// Never: every fact is read by value and every collection is total.
pub fn attribute(facts: &AttributionFacts) -> Diagnosis {
    let mut supported = vec![
        (Attribution::ProductDefect, ground::product(facts)),
        (Attribution::StaleLocator, ground::locator(facts)),
        (Attribution::RevisionRace, ground::revision(facts)),
        (Attribution::CoordinateDrift, ground::drift(facts)),
    ];
    supported.retain(|(_, grounds)| !grounds.is_empty());
    // Sorting by the class is what decides a case two classes both explain. The pushes above are
    // already in precedence order; sorting makes that a property of the type rather than of the
    // order somebody happened to write them in.
    supported.sort_by_key(|(class, _)| *class);
    let class = supported
        .first()
        .map_or(Attribution::InsufficientEvidence, |(class, _)| *class);
    let mut grounds = match supported.first() {
        Some((_, grounds)) => grounds.clone(),
        None => ground::gaps(facts),
    };
    grounds.extend(ground::failure(facts));
    let secondary = supported.iter().skip(1).map(|(class, _)| *class).collect();
    Diagnosis {
        class,
        case: facts.case.clone(),
        grounds,
        secondary,
    }
}

/// Appends one bullet of the prompt.
///
/// # Panics
///
/// Never.
fn line(text: &mut String, body: &str) {
    text.push_str("- ");
    text.push_str(body);
    text.push('\n');
}

/// The red lines every prompt carries, rendered from the guard's own constants.
///
/// The three lines are the guard's three refusals in prose, and they are built from the constants
/// the audit itself uses rather than written again: a prompt that named a path the gate would
/// refuse, or left one out, would send a reader into a red-line violation.
///
/// # Panics
///
/// Never.
fn red_lines() -> String {
    format!(
        "## 红线\n- 产品代码，禁止修改：{business}\n- 阈值与构建文件，禁止放宽：{frozen}\n\
         - 修复只能落在这两处：{allowed}\n- 以上三行由自愈守卫按同一组常量校验，越界即整轮失败。\n",
        business = listed(BUSINESS_CODE_PREFIXES),
        frozen = listed(FROZEN_FILES),
        allowed = listed(HEAL_ALLOWED_PREFIXES),
    )
}

/// The paths as one line of backticked names.
///
/// # Panics
///
/// Never.
fn listed(paths: &[&str]) -> String {
    paths
        .iter()
        .map(|path| format!("`{path}`"))
        .collect::<Vec<String>>()
        .join("、")
}
