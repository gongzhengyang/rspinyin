//! The commercial visual audit: the prompt that turns the specification's numbers into
//! item-by-item instructions, and the rules that decide whether the answers are evidence.
//!
//! Responsibility: render one review request -- a case, a snapshot and the metrics the running
//! window reported -- into a prompt whose every item names something observable, and judge what
//! comes back. Nothing here reads a file, opens a display or calls a model: the request arrives as
//! values, the answers arrive as values, and the whole module is therefore testable with no
//! display server, no compositor and no session.
//!
//! # Why an audit is a prompt and not another assertion
//!
//! The candidate window is drawn by the plugin itself, pixel by pixel: there is no widget tree, no
//! stylesheet and nothing that could compute a style. A pixel assertion can therefore only check
//! what somebody already thought to write down, and the defects that make a self-drawn window look
//! cheap are exactly the ones nobody wrote down -- a hairline that is two pixels at scale 2, a
//! corner radius that stopped being concentric when the padding changed, a highlight whose spring
//! was reset to zero on the second keypress. Reviewing a screenshot catches those, and reviewing
//! one is what a person does with their eyes and an agent does with a model.
//!
//! That only works if the review is answerable. The prompt below is what makes it so: every item
//! cites the row of `docs/dev/features.md` that fixes it, names the pixel or the metric that
//! decides it, and is answered in a vocabulary of three states. "The window looks professional" is
//! not an answer this module accepts; "`space.cell-radius` passed, sampled at (48, 12), measured
//! 8px, specification 4dp × 2.0 = 8px" is.
//!
//! # The three rules the prompt exists to enforce
//!
//! 1. **Every criterion names an observable.** A pixel at a coordinate, a distance between two
//!    edges, a number `runtime://ui_metrics` publishes, or a colour token. An item that could only
//!    be answered by an impression is not in the checklist at all.
//! 2. **"Could not judge" is a first-class verdict.** A compositor's blur and a real transparent
//!    surface cannot be read on a machine that offers neither, and `get_image` returns what was
//!    drawn rather than what a compositor would have blended. The prompt requires a reason and
//!    [`Summary::is_clean`] refuses to call such a run clean -- the same rule the environment gate
//!    applies to a case whose subject the machine cannot show.
//! 3. **An answer without a sampling coordinate is not evidence.** It cannot be re-derived from
//!    the same PNG by a second reader, so [`Finding::refusal`] refuses it rather than counting it.
//!
//! # Where the model is not
//!
//! This module renders the prompt and judges the answer; it does not run the audit. The review
//! itself happens where a model is reachable -- the MCP client on the other side of the prompt
//! resource -- and that boundary is not an accident of layering: the product promises that no
//! component of this workspace has any network capability at all, so the call cannot live here
//! even if a caller wanted it to. What crosses the boundary is text in both directions, which is
//! also what makes the judgement above testable.
//!
//! # Modules
//!
//! [`items`] holds the checklist, [`audit`] the submitted verdicts and their rules, and [`error`]
//! the refusals.

// The channel is exercised by the tests beside it and is not yet reachable from `xtask`'s
// subcommand tree, which lives in `xtask/src/main.rs` and in `xtask/src/testd/mod.rs` -- two files
// this module does not own. Until that wiring lands, every item here is reported as dead code in a
// non-test build, and the attribute goes away with those lines.
//
// `unused_imports` is covered by the same reasoning: the surface of this module is named by nothing
// in the crate yet.
#![allow(dead_code, unused_imports)]

mod audit;
mod error;
mod items;

#[cfg(test)]
mod tests;

pub use self::audit::{Finding, Refusal, Sample, Summary};
pub use self::error::ReviewError;
pub use self::items::check_items;

use std::path::{Path, PathBuf};

use ime_types::ColorScheme;

use super::coords::normalize_scale;
use super::ui_metrics::{SPEC_DOCUMENT, UiMetrics};

/// The name of the prompt resource this module renders.
pub const PROMPT_NAME: &str = "commercial_grade_visual_audit";

/// The longest a case identifier's module part may be.
const MAX_CASE_MODULE_BYTES: usize = 16;

/// The placeholder the case's identifier is substituted into.
const CASE_ID_PLACEHOLDER: &str = "{case_id}";

/// The placeholder this module's own resource name is substituted into.
const PROMPT_NAME_PLACEHOLDER: &str = "{prompt_name}";

/// The placeholder the snapshot's path is substituted into.
const SCREENSHOT_PLACEHOLDER: &str = "{screenshot}";

/// The placeholder the device pixel ratio is substituted into.
const SCALE_PLACEHOLDER: &str = "{scale}";

/// The placeholder the expected metrics are substituted into.
const METRICS_PLACEHOLDER: &str = "{metrics}";

/// The placeholder the checklist is substituted into.
const CHECKLIST_PLACEHOLDER: &str = "{checklist}";

/// The placeholder the specification's path is substituted into.
const SPEC_PLACEHOLDER: &str = "{spec}";

/// The prompt, with the placeholders [`render`] fills in.
///
/// It is Chinese because it is read by the reviewer rather than by a compiler, and because the
/// specification it quotes is Chinese; the doc comments around it stay English like the rest of
/// the workspace's code. The wording is load-bearing: the paragraph on the verdict vocabulary is
/// what stops a run on a machine without a compositor from reporting a clean audit, and the line
/// that bans an impression as an answer is what keeps the checklist falsifiable.
pub const VISUAL_AUDIT_PROMPT: &str = r#"# 商业化视觉审查：候选框（{case_id}）

> 提示资源：prompt://{prompt_name}

## 输入
- 截图：{screenshot}（物理像素；该帧的 device pixel ratio = {scale}）
- 期望度量（来自 runtime://ui_metrics）：
{metrics}
- 规范来源：{spec} 的 3.1 / 3.2 / 3.3 / 3.4 与 0.5.1 / 0.5.2

## 判定口径
- 每条判定只取三个值之一：`通过` / `不通过` / `无法判定`。
- `通过` 与 `不通过` 必须同时给出：采样坐标、实测像素值、规范值。缺任一项的判定是无效输出，
  由输出校验器拒绝：既不计入通过，也不计入不通过。
- `无法判定` 必须写出原因（例如「本机无合成器，get_image 只返回已绘制像素，读不到模糊」）。
  **`无法判定` 永远不等于 `通过`**：本次审查只有在每条判定都 `通过`、且每条判定都可采纳时才通过。
- 坐标一律是截图的物理像素，原点在左上角。规范值 × {scale} = 期望物理像素，允许 ±1px 的取整误差。
- 只写你采样到的东西。「看起来正常」「比较协调」「略显拥挤」这类结论是无效输出。

## 判定项
{checklist}
## 输出格式
逐条输出一行，字段以 ` | ` 分隔：

    <判定项 ID> | <通过|不通过|无法判定> | @(x,y) | 实测 <实测像素值> | 规范 <规范值> | <原因，无法判定时必填>

## 复核通道
像素结论会与 `runtime://ui_metrics` 的 `cross_check_spec` 独立通道交叉复核：同一个值一次从像素读到，
一次从 `.slint` 常量与运行窗口读到。两者不一致时以报告缺陷为准，不得为了对齐两边而改写判定。
"#;

/// A dimension of the specification the checklist audits.
///
/// The dimension is the table an item came from rather than a severity: it is what the prompt
/// groups the checklist by, so a reviewer reading the report can tell a colour disagreement from a
/// geometry one without opening the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dimension {
    /// 3.1: the window's sizes, its grid and its truncation rules.
    Space,
    /// 3.1.2: the material stack, from the base to the stroke.
    Material,
    /// 3.2: the colour tokens and the contrast floors they carry.
    Colour,
    /// 3.3: the motion parameters and the spring that drives them.
    Motion,
    /// 3.4: the five states a component has to cover.
    State,
    /// 0.5.1 and 0.5.2: a capability that degrades, and what the degradation has to look like.
    Degradation,
}

impl Dimension {
    /// Every dimension, in the order the prompt presents them.
    pub const ALL: [Self; 6] = [
        Self::Space,
        Self::Material,
        Self::Colour,
        Self::Motion,
        Self::State,
        Self::Degradation,
    ];

    /// The heading the prompt prints for this dimension.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::Space => "空间与尺寸（features.md 3.1）",
            Self::Material => "材质层次（features.md 3.1.2）",
            Self::Colour => "色彩 Token 与对比度（features.md 3.2）",
            Self::Motion => "物理动效（features.md 3.3）",
            Self::State => "组件五态（features.md 3.4）",
            Self::Degradation => "能力降级（features.md 0.5.1 / 0.5.2）",
        }
    }
}

/// The kind of thing a criterion is decided by.
///
/// The kind is carried rather than left to the prose because it is the one property that makes a
/// criterion falsifiable, and a test can hold every item to it: an item whose evidence is a pixel
/// is answered by sampling one, an item whose evidence is a metric is answered by reading the
/// channel, and neither can be answered by describing the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observable {
    /// A pixel of the snapshot, sampled at a stated coordinate.
    Pixel,
    /// A number `runtime://ui_metrics` publishes, which the pixels must agree with.
    Metric,
    /// A colour token of 3.2, which a sampled pixel must match.
    Token,
    /// A sequence of frames, sampled at stated instants.
    Frame,
}

impl Observable {
    /// The phrase the prompt names this kind of evidence with.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pixel => "像素采样",
            Self::Metric => "runtime://ui_metrics 度量",
            Self::Token => "色彩 Token",
            Self::Frame => "逐帧采样",
        }
    }
}

/// A citation of one row of the specification.
///
/// The row is cited by the label the document writes it under rather than by its line, because the
/// label is what the document's writers keep stable and a line number goes stale the moment a
/// paragraph is inserted above it. [`anchor_line`] turns the citation into a line when a report
/// wants one, so the two forms are always consistent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpecRef {
    /// The heading that opens the section, hashes included, as the document writes it.
    pub section: &'static str,
    /// A string that occurs on the cited row and nowhere earlier in the section.
    pub row: &'static str,
}

impl SpecRef {
    /// The citation of `row` inside `section`.
    ///
    /// # Panics
    ///
    /// Never.
    pub const fn of(section: &'static str, row: &'static str) -> Self {
        Self { section, row }
    }

    /// The citation as the prompt prints it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn describe(&self) -> String {
        let section = self.section.trim_start_matches('#').trim();
        format!("{SPEC_DOCUMENT} {section}「{}」", self.row)
    }
}

/// One item of the visual audit.
///
/// An item is a criterion and the two things a reviewer needs to answer it: where to look, and
/// where the criterion comes from. It carries no threshold of its own -- the numbers in the
/// criterion are the document's, and the row it cites is where they can be read again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckItem {
    /// The stable identifier the reviewer answers with, e.g. `space.corner-radius`.
    pub id: &'static str,
    /// Which dimension of the specification it audits.
    pub dimension: Dimension,
    /// The criterion, stated the way the reviewer reads it.
    pub criterion: &'static str,
    /// What kind of evidence decides it.
    pub observable: Observable,
    /// How to observe it: where to sample, and what the sample has to show.
    pub sampling: &'static str,
    /// The row of the specification it comes from, and the row a failure is attributed to.
    pub anchor: SpecRef,
}

/// One review request: the snapshot to audit, the case it belongs to, and what the window reported.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditRequest {
    /// The case the snapshot belongs to, e.g. `TC-UI-16`.
    pub case_id: String,
    /// The snapshot's path, as the evidence archive laid it out.
    pub screenshot: PathBuf,
    /// The device pixel ratio the snapshot was rasterised at, normalised.
    pub scale: f32,
    /// What the running window reported, when the caller read the metrics channel.
    pub metrics: Option<UiMetrics>,
}

impl AuditRequest {
    /// The prompt text: the template with this request's inputs and the checklist filled in.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn prompt(&self) -> String {
        render(self)
    }
}

/// Builds the review request for one case, whose snapshot the caller names.
///
/// The case identifier is checked rather than rewritten: the prompt quotes it back to the
/// reviewer, and a review filed under a name the suite never used is a review nobody finds.
///
/// # Errors
///
/// Returns [`ReviewError::BadCaseId`] when `case_id` is not of the form the suite names its cases
/// with, `TC-<MODULE>-<NUMBER>`.
///
/// # Panics
///
/// Never.
pub fn audit_request(
    case_id: &str,
    screenshot: &Path,
    scale: f32,
    metrics: Option<&UiMetrics>,
) -> Result<AuditRequest, ReviewError> {
    if !is_case_id(case_id) {
        return Err(ReviewError::BadCaseId {
            case_id: case_id.to_owned(),
        });
    }
    Ok(AuditRequest {
        case_id: case_id.to_owned(),
        screenshot: screenshot.to_path_buf(),
        scale: normalize_scale(scale),
        metrics: metrics.cloned(),
    })
}

/// Builds the review request for a snapshot the evidence archive laid out, whose case the path
/// names.
///
/// The layout is `RUN/<module>/<TC-ID>/<step>_<state>.png`, so the case is the directory the
/// snapshot sits in and the module is the one above it. Reading the case out of the path rather
/// than asking the caller for it a second time is what keeps a review attached to the file it was
/// run against: a caller that passed both could pass a pair that disagrees, and the prompt would
/// then cite one case while showing another's pixels.
///
/// # Errors
///
/// Returns [`ReviewError::BadSnapshotPath`] when the path is not a `.png` inside a directory named
/// like a case, inside a module directory.
///
/// # Panics
///
/// Never.
pub fn audit_request_of_snapshot(
    snapshot: &Path,
    scale: f32,
    metrics: Option<&UiMetrics>,
) -> Result<AuditRequest, ReviewError> {
    let file = snapshot
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| bad_snapshot(snapshot))?;
    if !file.ends_with(".png") {
        return Err(bad_snapshot(snapshot));
    }
    let case_id = snapshot
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .ok_or_else(|| bad_snapshot(snapshot))?;
    if !is_case_id(case_id) {
        return Err(bad_snapshot(snapshot));
    }
    let module = snapshot
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .ok_or_else(|| bad_snapshot(snapshot))?;
    if module.is_empty() || !module.chars().all(is_plain) {
        return Err(bad_snapshot(snapshot));
    }
    audit_request(case_id, snapshot, scale, metrics)
}

/// The line of `document` that states what `anchor` cites.
///
/// The section is found by its heading and the row by its label, so the citation survives the
/// document growing: a row that moved down a page still resolves, and a row that was renamed
/// resolves to nothing rather than to whatever now sits where it used to. The search stops at the
/// next heading of the same depth or shallower, which is what keeps a label that occurs in two
/// sections from resolving to the earlier one.
///
/// # Panics
///
/// Never.
pub fn anchor_line(document: &str, anchor: &SpecRef) -> Option<usize> {
    let mut inside = false;
    for (index, line) in document.lines().enumerate() {
        if !inside {
            if line.trim_start().starts_with(anchor.section) {
                inside = true;
            }
            continue;
        }
        if is_heading(line) {
            break;
        }
        if line.contains(anchor.row) {
            return Some(index + 1);
        }
    }
    None
}

/// Whether `line` opens a Markdown heading that ends the section being read.
///
/// Only depths two to four end a section: the specification's own visual baseline is written at
/// those depths, and a line that opens with a single `#` is a document title rather than a
/// boundary inside one.
///
/// # Panics
///
/// Never.
fn is_heading(line: &str) -> bool {
    let hashes = line.chars().take_while(|ch| *ch == '#').count();
    (2..=4).contains(&hashes) && line.chars().nth(hashes) == Some(' ')
}

/// Whether `case_id` is of the form the test suite names its cases with, `TC-<MODULE>-<NUMBER>`.
///
/// The module part is upper case letters only and the number is digits only, which is the spelling
/// every shard of `docs/dev/tests.md` uses: `TC-CORE-01`, `TC-UI-16`. A near miss is refused rather
/// than accepted, because the identifier reaches the prompt and the report, and the one thing a
/// reviewer must be able to do with it is find the case it names.
///
/// # Panics
///
/// Never.
fn is_case_id(case_id: &str) -> bool {
    let Some(rest) = case_id.strip_prefix("TC-") else {
        return false;
    };
    let Some((module, number)) = rest.rsplit_once('-') else {
        return false;
    };
    !module.is_empty()
        && module.len() <= MAX_CASE_MODULE_BYTES
        && module.chars().all(|ch| ch.is_ascii_uppercase())
        && !number.is_empty()
        && number.chars().all(|ch| ch.is_ascii_digit())
}

/// Whether `ch` may appear in a path segment the evidence archive builds.
///
/// # Panics
///
/// Never.
fn is_plain(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')
}

/// The refusal a path that is not a snapshot of the archive is reported with.
///
/// # Panics
///
/// Never.
fn bad_snapshot(snapshot: &Path) -> ReviewError {
    ReviewError::BadSnapshotPath {
        path: snapshot.display().to_string(),
    }
}

/// Renders the prompt for `request`.
///
/// # Panics
///
/// Never.
fn render(request: &AuditRequest) -> String {
    let screenshot = request.screenshot.display().to_string();
    let scale = request.scale.to_string();
    let metrics = metrics_block(request.metrics.as_ref());
    let checklist = render_items(&check_items());
    VISUAL_AUDIT_PROMPT
        .replace(CASE_ID_PLACEHOLDER, &request.case_id)
        .replace(PROMPT_NAME_PLACEHOLDER, PROMPT_NAME)
        .replace(SCREENSHOT_PLACEHOLDER, &screenshot)
        .replace(SCALE_PLACEHOLDER, &scale)
        .replace(METRICS_PLACEHOLDER, &metrics)
        .replace(CHECKLIST_PLACEHOLDER, &checklist)
        .replace(SPEC_PLACEHOLDER, SPEC_DOCUMENT)
}

/// Renders the checklist, grouped by dimension and numbered across the groups.
///
/// # Panics
///
/// Never.
fn render_items(items: &[CheckItem]) -> String {
    let mut text = String::new();
    let mut number = 0;
    for dimension in Dimension::ALL {
        let group: Vec<&CheckItem> = items
            .iter()
            .filter(|item| item.dimension == dimension)
            .collect();
        if group.is_empty() {
            continue;
        }
        let heading = dimension.label();
        text.push_str(&format!("### {heading}\n\n"));
        for item in group {
            number += 1;
            let id = item.id;
            let criterion = item.criterion;
            let sampling = item.sampling;
            let observable = item.observable.label();
            let anchor = item.anchor.describe();
            text.push_str(&format!(
                "{number}. [{id}] {criterion}｜证据：{sampling}（{observable}）｜规范：{anchor}\n"
            ));
        }
        text.push('\n');
    }
    text
}

/// The expected metrics, as the prompt lists them.
///
/// When the caller read none, the prompt says so rather than leaving the section out: an item
/// whose criterion needs a number the window never reported has to be answered `无法判定` with that
/// reason, and a prompt that quietly omitted the section would invite the number to be guessed.
///
/// # Panics
///
/// Never.
fn metrics_block(metrics: Option<&UiMetrics>) -> String {
    let Some(metrics) = metrics else {
        return String::from(
            "- （未注入 runtime://ui_metrics）本次审查只能用规范值判定；需要度量交叉复核的判定项记 `无法判定` 并写明原因",
        );
    };
    let theme = &metrics.theme;
    let accent = theme.accent;
    let (width_dp, height_dp) = metrics.window_dp;
    let (width_px, height_px) = metrics.window_px;
    let scale = metrics.scale;
    let corner = theme.corner_radius_dp;
    let max_width = metrics.layout.max_width_dp;
    let max_per_row = metrics.layout.max_per_row;
    let annotation = metrics.layout.show_annotation;
    let scheme = match theme.scheme {
        ColorScheme::Light => "亮色",
        ColorScheme::Dark => "暗色",
    };
    let r = accent.r;
    let g = accent.g;
    let b = accent.b;
    let acrylic = theme.acrylic;
    let requested = theme.base_alpha;
    let reported = metrics.base_alpha;
    let expected = metrics.expected_base_alpha();
    let compositor = metrics.compositor_present;
    let argb = metrics.argb_visual;
    let backend = metrics.backend_id;
    format!(
        "- 窗口：{width_dp} × {height_dp} dp = {width_px} × {height_px} px；device pixel ratio = {scale}\n\
         - 容器圆角 {corner}dp；候选框最大宽度 {max_width}dp；单行最大候选数 {max_per_row}；显示注音 {annotation}\n\
         - 主题 {scheme}；强调色 #{r:02X}{g:02X}{b:02X}；亚克力请求 {acrylic}；base_alpha 请求 {requested}，后端报告 {reported}（本窗口应为 {expected}）\n\
         - 合成器 {compositor}；ARGB visual {argb}；后端 {backend}"
    )
}
