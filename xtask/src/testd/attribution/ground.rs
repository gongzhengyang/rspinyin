//! What one conclusion rests on, as the channels that produced the facts stated it.
//!
//! Responsibility: turn the facts the locator channel, the frame channel, the metrics
//! cross-check and the evidence channel already produced into the grounds of one attribution --
//! the lines a reader checks the class against -- and collect the facts the evidence does not
//! hold. Nothing here decides a class: [`super::attribute`] does that, and this module only says
//! what each class would rest on.
//!
//! # A ground is a quotation, not a summary
//!
//! Every line rendered here is the fact the channel produced, in the channel's own words: a
//! refusal's own `detail`, a mismatch's own rendering, a record's own names, points and values.
//! A ground that paraphrased would be a second account of one fact, and the account a reader
//! checks against the file has to be the channel's.
//!
//! # What is deliberately left out
//!
//! No observed value reaches a ground. A failed assertion is named and its two sides are left
//! where the archive already keeps them, in the case's own trace: a prompt travels into a report
//! and may be attached to a bug report, and the archive's rule -- an observed value reaches a
//! document only through its own value type, which withholds one that may carry what the user
//! typed -- is not this module's to weaken. An image defect is named by its item, its snapshot
//! and its rectangle for the same reason.

use crate::testd::evidence::{DefectRect, TRACE_FILE};
use crate::testd::heal::{AnchorView, HealCause, HealError};
use crate::testd::ui_metrics::{MetricMismatch, SPEC_DOCUMENT};

use super::AttributionFacts;

/// One fact a conclusion rests on, in the shape the channel that produced it holds.
///
/// The variants are the channels' facts and not a second vocabulary: a rename, a reprojection and
/// a refusal are the locator channel's, a stale revision is the frame channel's, a disagreement is
/// the cross-check's, and a failed assertion and an image defect are the evidence channel's.
/// [`Ground::Gap`] and [`Ground::RuledOut`] are the two lines that are about the evidence itself
/// rather than about the product: a fact it does not hold, and a class a reading it does hold rules
/// out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ground {
    /// A name the case used is gone, and the specification supplied it again.
    Renamed {
        /// The name the case used, which the source no longer declares.
        from: String,
        /// The name the source declares now.
        to: String,
        /// The specification rows the re-derivation is anchored on, as one citation.
        row: String,
    },
    /// A name the case used moved, and the value it named moved with it.
    Drifted {
        /// The name the case used.
        name: String,
        /// The name the source declares now.
        declared_name: String,
        /// The value the declaration carries.
        declared: String,
        /// The value the specification states.
        spec: String,
    },
    /// A locator could not resolve what the case named.
    Unresolvable {
        /// What the locator named.
        wanted: String,
        /// Why it cannot be resolved, in the locator's own words.
        detail: String,
    },
    /// A point the case recorded is not the point the specification produces.
    Reprojected {
        /// What the locator named, in the locator's own words.
        locator: String,
        /// The point the case recorded, in container pixels.
        from: (i32, i32),
        /// The point the derivation produces, in container pixels.
        to: (i32, i32),
    },
    /// The frame the case asserted on is not the frame it awaited.
    Stale {
        /// The revision the case awaited.
        expected: u32,
        /// The revision the snapshot carried when the case asserted on it.
        seen: u32,
        /// How many times the file offered a revision older than the one already read.
        rewinds: u64,
    },
    /// Two sources disagree about one item, in the cross-check's own words.
    Disagreement {
        /// The disagreement, rendered by [`MetricMismatch::describe`].
        item: String,
    },
    /// A region of a snapshot does not hold what the specification states.
    Defect {
        /// The audit item, e.g. `corner_radius`.
        item: String,
        /// The snapshot, named relative to the case's own directory.
        image: String,
        /// The region the defect occupies, in the snapshot's own pixels.
        rect: DefectRect,
    },
    /// An assertion that did not hold.
    Failed {
        /// The assertion's name.
        name: String,
    },
    /// A fact the evidence does not hold.
    Gap {
        /// What is missing, and what would fill it.
        detail: String,
    },
    /// A class the evidence rules out, and the reading that rules it out.
    RuledOut {
        /// The class, named as `Attribution::code` names it.
        class: &'static str,
        /// The reading that rules it out.
        detail: String,
    },
}

impl Ground {
    /// The ground as the one line a prompt carries.
    ///
    /// # Panics
    ///
    /// Never: every variant is rendered from its own fields.
    pub fn line(&self) -> String {
        match self {
            Self::Renamed { from, to, row } => {
                format!(
                    "用例用的常量 `{from}` 已不在源码里；自愈通道按规格重新推导出 `{to}`（依据：{row}）"
                )
            }
            Self::Drifted {
                name,
                declared_name,
                declared,
                spec,
            } => {
                format!(
                    "`{name}` 现在声明为 `{declared_name}`，其值是 {declared}，而规格行说的是 {spec}：\
                     名字和值一起动了"
                )
            }
            Self::Unresolvable { wanted, detail } => {
                format!("自愈通道无法解析 {wanted}：{detail}")
            }
            Self::Reprojected { locator, from, to } => {
                format!(
                    "用例记录的 {} 不是规格常量算出的 {}（{locator}）",
                    point(*from),
                    point(*to)
                )
            }
            Self::Stale {
                expected,
                seen,
                rewinds,
            } => {
                format!(
                    "用例断言的 revision 是 {seen}，它等的是 {expected}；快照文件回退过 {rewinds} 次：\
                     断言落在的不是它要等的那一帧"
                )
            }
            Self::Disagreement { item } => format!("规格交叉检查报出不一致：{item}"),
            Self::Defect { item, image, rect } => {
                let DefectRect { x, y, w, h } = *rect;
                format!("截图 {image} 的 {item} 区域在 {x},{y} 处 {w}x{h} 与规格不符")
            }
            Self::Failed { name } => {
                format!("断言 `{name}` 未通过；两侧的值见用例目录的 `{TRACE_FILE}`")
            }
            Self::Gap { detail } => detail.clone(),
            Self::RuledOut { class, detail } => format!("`{class}` 被排除：{detail}"),
        }
    }
}

/// The facts that say the source disagrees with the specification.
///
/// Three channels can say it and none of them is the other's substitute: the locator refuses a
/// name that moved together with its value, the cross-check compares the source with the
/// specification item by item, and the visual audit reports a region of a snapshot that holds
/// something the specification does not state. A source that could not be read is not one of
/// them -- nothing was compared -- so it reaches [`gaps`] instead.
///
/// # Panics
///
/// Never.
pub(super) fn product(facts: &AttributionFacts) -> Vec<Ground> {
    let mut grounds = Vec::new();
    for refusal in &facts.locator.refusals {
        if let HealError::Drifted {
            name,
            declared_name,
            declared,
            spec,
        } = refusal
        {
            grounds.push(Ground::Drifted {
                name: name.clone(),
                declared_name: declared_name.clone(),
                declared: declared.clone(),
                spec: spec.clone(),
            });
        }
    }
    for mismatch in &facts.mismatches {
        if !matches!(mismatch, MetricMismatch::Unavailable { .. }) {
            grounds.push(Ground::Disagreement {
                item: mismatch.describe(),
            });
        }
    }
    for defect in &facts.defects {
        grounds.push(Ground::Defect {
            item: defect.item.clone(),
            image: defect.image.clone(),
            rect: defect.rect,
        });
    }
    grounds
}

/// The facts that say a name the case used is gone.
///
/// A rename the locator re-derived and a name it could not re-derive at all are the two shapes of
/// one finding: what the case named is not what the source declares. Both are about the test
/// script, and neither is about the product -- the locator refuses rather than following a value
/// that moved, which is what keeps a rename from covering a regression.
///
/// # Panics
///
/// Never.
pub(super) fn locator(facts: &AttributionFacts) -> Vec<Ground> {
    let mut grounds = Vec::new();
    for record in &facts.locator.healed {
        if let HealCause::Renamed { from, to } = &record.cause {
            grounds.push(Ground::Renamed {
                from: from.clone(),
                to: to.clone(),
                row: citation(&record.evidence),
            });
        }
    }
    for refusal in &facts.locator.refusals {
        if let HealError::Unresolvable { wanted, detail } = refusal {
            grounds.push(Ground::Unresolvable {
                wanted: wanted.clone(),
                detail: detail.clone(),
            });
        }
    }
    grounds
}

/// The facts that say the frame the case asserted on is not the frame it awaited.
///
/// A case that reads no frame has no revision evidence and gets no ground here: the absence is
/// not a finding, and [`gaps`] is what records it.
///
/// # Panics
///
/// Never.
pub(super) fn revision(facts: &AttributionFacts) -> Vec<Ground> {
    match facts.revision {
        Some(facts) if facts.is_stale() => vec![Ground::Stale {
            expected: facts.expected,
            seen: facts.seen,
            rewinds: facts.rewinds,
        }],
        _ => Vec::new(),
    }
}

/// The facts that say a point the case recorded is not the point the specification produces.
///
/// The locator records such a point only when every constant its arithmetic is built on agrees
/// with the specification, so a reprojection is a recording that went stale and never a value
/// that drifted: a point that moved because a value moved is left to the cross-check, which is
/// what the locator's own refusal to record it means.
///
/// # Panics
///
/// Never.
pub(super) fn drift(facts: &AttributionFacts) -> Vec<Ground> {
    facts
        .locator
        .healed
        .iter()
        .filter_map(|record| match &record.cause {
            HealCause::Reprojected { from, to } => Some(Ground::Reprojected {
                locator: record.locator.clone(),
                from: *from,
                to: *to,
            }),
            HealCause::Renamed { .. } => None,
        })
        .collect()
}

/// The assertions the case made and did not hold.
///
/// Every diagnosis carries them, whichever class won: the class says why the case is believed to
/// have failed, and these say what failed. Only the names are taken, and the two sides stay in
/// the case's own trace.
///
/// # Panics
///
/// Never.
pub(super) fn failure(facts: &AttributionFacts) -> Vec<Ground> {
    facts
        .diffs
        .iter()
        .map(|diff| Ground::Failed {
            name: diff.name.clone(),
        })
        .collect()
}

/// The facts the evidence does not hold, and the classes a reading it does hold rules out.
///
/// These are the grounds of an attribution that names no cause, and they are what makes that
/// answer usable rather than empty: a reader is told which fact is missing and what would fill
/// it, so the next attempt at an attribution has something to read. A class is ruled out only by
/// a reading that could have shown it, which is why the revision is the one class this list can
/// exclude -- the frame channel read the file and the frame was the awaited one.
///
/// # Panics
///
/// Never.
pub(super) fn gaps(facts: &AttributionFacts) -> Vec<Ground> {
    let mut gaps = Vec::new();
    if facts.locator.healed.is_empty() && facts.locator.refusals.is_empty() {
        gaps.push(gap(GAP_LOCATOR));
    }
    match facts.revision {
        Some(revision) if !revision.is_stale() => {
            gaps.push(Ground::RuledOut {
                class: super::Attribution::RevisionRace.code(),
                detail: format!(
                    "用例断言的 revision 是 {}，正是它等的 {}，快照文件也没有回退过",
                    revision.seen, revision.expected
                ),
            });
        }
        Some(_) => {}
        None => gaps.push(gap(GAP_REVISION)),
    }
    for mismatch in &facts.mismatches {
        if let MetricMismatch::Unavailable { .. } = mismatch {
            gaps.push(Ground::Gap {
                detail: mismatch.describe(),
            });
        }
    }
    if facts.mismatches.is_empty() {
        gaps.push(gap(GAP_MISMATCH));
    }
    if facts.diffs.is_empty() {
        gaps.push(gap(GAP_DIFFS));
    }
    gaps
}

/// Why the evidence holds nothing from the locator channel.
const GAP_LOCATOR: &str = "\
定位器通道没有记录任何重推导或拒绝，所以常量名与坐标的失效都无从判断：用 resolve 与 resolve_hit \
复跑一次该用例的 locator，把记录交给 attribute";

/// Why the evidence holds nothing about the revision.
const GAP_REVISION: &str = "\
该用例没有读帧快照，所以竞态不是被排除而是无从判断：如果失败与候选内容或顺序有关，让用例用 \
FrameWatch 读一次快照并记下 revision";

/// Why the evidence holds nothing from the cross-check.
const GAP_MISMATCH: &str = "\
规格交叉检查没有报出任何结果，所以常量漂移没有被排除：用 runtime://ui_metrics 取一次读数，\
把 cross_check_spec 的输出交给 attribute";

/// Why the evidence holds no failed assertion.
const GAP_DIFFS: &str = "\
取证通道没有留下未通过的断言，这次失败没有可归因的事实：先确认该用例的 trace.json 是否写成";

/// A gap ground.
///
/// # Panics
///
/// Never.
fn gap(detail: &str) -> Ground {
    Ground::Gap {
        detail: detail.to_owned(),
    }
}

/// The specification rows a record is anchored on, as one citation.
///
/// The section heading is left out on purpose: a row's element label is the key the specification
/// itself indexes rows by, so the document and the label are enough to find the row, and the
/// heading's own hashes would be noise in a prompt.
///
/// # Panics
///
/// Never.
fn citation(evidence: &[AnchorView]) -> String {
    if evidence.is_empty() {
        return SPEC_DOCUMENT.to_owned();
    }
    let rows = evidence
        .iter()
        .map(|anchor| format!("{} = {}", anchor.row, anchor.value))
        .collect::<Vec<String>>()
        .join("; ");
    format!("{SPEC_DOCUMENT} 的 {rows}")
}

/// A container point as a prompt writes it.
///
/// # Panics
///
/// Never.
fn point(point: (i32, i32)) -> String {
    format!("{},{}", point.0, point.1)
}
