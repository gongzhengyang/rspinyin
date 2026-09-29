//! The 3.1 items: the candidate window's sizes, its grid, its material stack and its truncation
//! rules.
//!
//! Responsibility: state, for every row of `features.md` 3.1, what a reviewer has to look at and
//! what the row says it should be. Nothing here measures anything: the items are instructions and
//! the numbers in them are quoted from the document by a person reading both.
//!
//! # Why a row is an item and not a sentence in a paragraph
//!
//! A criterion a reviewer cannot act on is a criterion nobody checks. Every row of the three
//! tables below names one observable thing -- a pixel at a coordinate, a distance between two
//! edges, a colour read off a stroke -- so an answer of "pass" carries a coordinate and a value
//! that a script can re-derive from the same PNG. The rows whose only observable is a number the
//! running window reports rather than a pixel are marked as such, because the two are answered
//! from different sources and a reviewer that mixed them up would report agreement with a
//! document it never read.
//!
//! # The anchors are rows, not line numbers
//!
//! Each item cites the row it comes from by the row's own element label, which is what the
//! document's writers keep stable, and [`super::super::anchor_line`] resolves that citation to a
//! line when a report wants one. A line number written down here would go stale the first time the
//! document gained a paragraph above it, and a stale citation points a reviewer at an unrelated
//! row with full confidence.

use super::super::{CheckItem, Dimension, Observable, SpecRef};

/// The heading that opens the geometry table.
const GEOMETRY: &str = "#### 3.1.1";

/// The heading that opens the material stack.
const MATERIAL: &str = "#### 3.1.2";

/// The heading that opens the truncation rules.
const TRUNCATION: &str = "#### 3.1.3";

/// The heading that opens the grid rule and its exception list.
const GRID: &str = "#### 3.1.4";

/// Every 3.1 item, in document order.
pub(super) fn items() -> Vec<CheckItem> {
    let mut items = geometry();
    items.extend(material());
    items.extend(truncation());
    items.extend(grid());
    items
}

/// The rows of the 3.1.1 geometry table.
fn geometry() -> Vec<CheckItem> {
    vec![
        CheckItem {
            id: "space.window-margin",
            dimension: Dimension::Space,
            criterion: "窗口外边距（阴影预留）四周 32dp；该区域内完全透明，且输入区域为空（点击穿透）",
            observable: Observable::Pixel,
            sampling: "在窗口最外侧 4px 的环形带内采样 alpha（应为 0），并在该带内点击一次，确认事件落在下层窗口",
            anchor: SpecRef::of(GEOMETRY, "窗口外边距（阴影预留）"),
        },
        CheckItem {
            id: "space.corner-radius",
            dimension: Dimension::Space,
            criterion: "容器圆角 = 12dp × scale（`corner_radius_dp`，可配置 8~20）",
            observable: Observable::Pixel,
            sampling: "沿容器左上角 45° 向内 3px 采样圆角像素；四个圆角外侧各采样 2px，断言 alpha = 0",
            anchor: SpecRef::of(GEOMETRY, "容器圆角"),
        },
        CheckItem {
            id: "space.container-stroke",
            dimension: Dimension::Space,
            criterion: "容器描边 1dp，颜色 `surface.stroke`",
            observable: Observable::Pixel,
            sampling: "在容器左边界处水平采样 4px，量出描边的物理宽度并读出它的颜色",
            anchor: SpecRef::of(GEOMETRY, "容器描边"),
        },
        CheckItem {
            id: "space.container-padding",
            dimension: Dimension::Space,
            criterion: "容器内边距 8dp（四周）",
            observable: Observable::Pixel,
            sampling: "量取描边内侧到 Header 顶边、到候选区左边缘的两段距离",
            anchor: SpecRef::of(GEOMETRY, "容器内边距"),
        },
        CheckItem {
            id: "space.min-width",
            dimension: Dimension::Space,
            criterion: "候选框最小宽度 220dp",
            observable: Observable::Metric,
            sampling: "候选数为 1 时量取容器宽度，并与 `runtime://ui_metrics` 报告的窗口逻辑宽度比对",
            anchor: SpecRef::of(GEOMETRY, "候选框最小宽度"),
        },
        CheckItem {
            id: "space.max-width",
            dimension: Dimension::Space,
            criterion: "候选框最大宽度 = min(720dp, screen_width_dp − 32dp)；超出时按 3.1.3 换行",
            observable: Observable::Metric,
            sampling: "读 `runtime://ui_metrics` 的窗口逻辑宽度与屏幕宽度，并量取截图里的容器宽度",
            anchor: SpecRef::of(GEOMETRY, "候选框最大宽度"),
        },
        CheckItem {
            id: "space.header-height",
            dimension: Dimension::Space,
            criterion: "Header 高 34dp；仅有 preedit 无候选时压缩为 28dp",
            observable: Observable::Pixel,
            sampling: "从容器顶边向下扫描到 Header 分隔线，量出 Header 高度；有候选与无候选两种状态各量一次",
            anchor: SpecRef::of(GEOMETRY, "Header 高度"),
        },
        CheckItem {
            id: "space.header-padding",
            dimension: Dimension::Space,
            criterion: "Header 水平内边距 10dp",
            observable: Observable::Pixel,
            sampling: "量取拼音串首字符左边缘到容器内边距之间的距离",
            anchor: SpecRef::of(GEOMETRY, "Header 水平内边距"),
        },
        CheckItem {
            id: "space.header-type",
            dimension: Dimension::Space,
            criterion: "Header 字号/字重：拼音串 14sp / 500，切分符 14sp / 400 且 opacity 0.40，状态文本 11sp / 500",
            observable: Observable::Pixel,
            sampling: "量取拼音串与状态文本的字形高度，并采样切分符字形像素的 alpha",
            anchor: SpecRef::of(GEOMETRY, "Header 字号/字重"),
        },
        CheckItem {
            id: "space.header-icons",
            dimension: Dimension::Space,
            criterion: "Header 状态图标 16dp × 16dp，图标间距 8dp，图标与文本间距 6dp",
            observable: Observable::Pixel,
            sampling: "量取每个状态图标的外接框，以及图标之间、图标与相邻文本之间的距离",
            anchor: SpecRef::of(GEOMETRY, "Header 状态图标"),
        },
        CheckItem {
            id: "space.header-separator",
            dimension: Dimension::Space,
            criterion: "Header 分隔线 1dp，颜色 `separator`，紧贴 Header 底部",
            observable: Observable::Pixel,
            sampling: "在 Header 底边所在行垂直采样 4px，量出线的宽度并读出它的颜色",
            anchor: SpecRef::of(GEOMETRY, "Header 分隔线"),
        },
        CheckItem {
            id: "space.grid-padding",
            dimension: Dimension::Space,
            criterion: "候选区内边距 8dp",
            observable: Observable::Pixel,
            sampling: "量取分隔线到首行候选单元顶边之间的距离",
            anchor: SpecRef::of(GEOMETRY, "候选区内边距"),
        },
        CheckItem {
            id: "space.cell-min-width",
            dimension: Dimension::Space,
            criterion: "候选单元最小宽度 64dp",
            observable: Observable::Pixel,
            sampling: "候选数为 1 时量取该候选单元的宽度",
            anchor: SpecRef::of(GEOMETRY, "候选单元最小宽度"),
        },
        CheckItem {
            id: "space.cell-height",
            dimension: Dimension::Space,
            criterion: "候选单元高度 36dp",
            observable: Observable::Pixel,
            sampling: "量取候选单元顶边与底边之间的距离",
            anchor: SpecRef::of(GEOMETRY, "候选单元高度"),
        },
        CheckItem {
            id: "space.cell-padding",
            dimension: Dimension::Space,
            criterion: "候选单元内边距：水平 10dp、垂直 6dp",
            observable: Observable::Pixel,
            sampling: "量取序号槽左边缘到单元左边缘的距离，以及文本行上下的留白",
            anchor: SpecRef::of(GEOMETRY, "候选单元内边距"),
        },
        CheckItem {
            id: "space.cell-radius",
            dimension: Dimension::Space,
            criterion: "候选单元圆角 4dp（同心圆角 = 容器圆角 12dp − 容器内边距 8dp）",
            observable: Observable::Pixel,
            sampling: "沿候选单元左上角 45° 向内 2px 采样圆角像素；在单元角外 1px 处采样，断言该处为容器底色",
            anchor: SpecRef::of(GEOMETRY, "候选单元圆角"),
        },
        CheckItem {
            id: "space.number-type",
            dimension: Dimension::Space,
            criterion: "候选序号 11sp / 500，有效 opacity 0.55，与候选文本间距 6dp",
            observable: Observable::Pixel,
            sampling: "采样序号字形像素的 alpha（有效 α 应为 0.55，不得是 0.48 × 0.55 = 0.264）",
            anchor: SpecRef::of(GEOMETRY, "候选序号字号"),
        },
        CheckItem {
            id: "space.cell-type",
            dimension: Dimension::Space,
            criterion: "候选文本 15sp / 400；首选项与键盘高亮项为 15sp / 500",
            observable: Observable::Pixel,
            sampling: "在同一行比较首选项与普通候选的字形高度与笔画密度",
            anchor: SpecRef::of(GEOMETRY, "候选文本字号/字重"),
        },
        CheckItem {
            id: "space.annotation-type",
            dimension: Dimension::Space,
            criterion: "候选注音 11sp / 400，有效 opacity 0.50，与候选文本间距 6dp",
            observable: Observable::Pixel,
            sampling: "采样注音字形像素的 alpha 与字形高度",
            anchor: SpecRef::of(GEOMETRY, "候选注音字号"),
        },
        CheckItem {
            id: "space.grid-column-gap",
            dimension: Dimension::Space,
            criterion: "网格列间距 6dp",
            observable: Observable::Pixel,
            sampling: "量取同一行相邻两个候选单元之间的水平距离",
            anchor: SpecRef::of(GEOMETRY, "网格列间距"),
        },
        CheckItem {
            id: "space.grid-row-gap",
            dimension: Dimension::Space,
            criterion: "网格行间距 6dp",
            observable: Observable::Pixel,
            sampling: "量取相邻两行候选单元之间的垂直距离",
            anchor: SpecRef::of(GEOMETRY, "网格行间距"),
        },
        CheckItem {
            id: "space.max-per-row",
            dimension: Dimension::Space,
            criterion: "单行最大候选数 5（`layout.max_per_row`，可配置 3~9）",
            observable: Observable::Metric,
            sampling: "读 `runtime://ui_metrics` 的 max_per_row，并数出截图里每一行的候选单元数",
            anchor: SpecRef::of(GEOMETRY, "单行最大候选数"),
        },
        CheckItem {
            id: "space.cursor-arrow",
            dimension: Dimension::Space,
            criterion: "光标指示箭头高 6dp、宽 12dp，居中于光标水平位置；仅 placement 为 Below 且候选框未被夹取时绘制",
            observable: Observable::Pixel,
            sampling: "量取箭头三角形的外接框，并与光标水平位置比较是否居中",
            anchor: SpecRef::of(GEOMETRY, "光标指示箭头"),
        },
    ]
}

/// The layers of the 3.1.2 material stack.
fn material() -> Vec<CheckItem> {
    vec![
        CheckItem {
            id: "material.base",
            dimension: Dimension::Material,
            criterion: "L0 基础底 = `surface.base`：暗色 `#1C1C1E @ alpha 0.85`，亮色 `#FFFFFF @ alpha 0.85`",
            observable: Observable::Pixel,
            sampling: "采样容器中心的纯背景像素（避开文本与网格），读出 RGB 与 alpha",
            anchor: SpecRef::of(MATERIAL, "L0 基础底"),
        },
        CheckItem {
            id: "material.blur",
            dimension: Dimension::Material,
            criterion: "L1 合成器模糊：对 `surface.base` 的非透明区域做 24dp 高斯模糊；不可用时降级为 `alpha 1.0` 纯色底",
            observable: Observable::Pixel,
            sampling: "比较框内背景与框外同一条纹理的清晰度；`get_image` 读不到模糊时记 `无法判定` 并写出原因，不得记通过",
            anchor: SpecRef::of(MATERIAL, "L1 合成器模糊"),
        },
        CheckItem {
            id: "material.shadow-inner",
            dimension: Dimension::Material,
            criterion: "L2 内层硬阴影 `0 1dp 2dp shadow.inner`",
            observable: Observable::Pixel,
            sampling: "沿容器上边缘垂直采样 4px 带的 alpha 曲线，量出偏移与扩散范围",
            anchor: SpecRef::of(MATERIAL, "L2 内层硬阴影"),
        },
        CheckItem {
            id: "material.shadow-outer",
            dimension: Dimension::Material,
            criterion: "L3 外层软阴影 `0 8dp 28dp shadow.outer`",
            observable: Observable::Pixel,
            sampling: "在容器外沿垂直方向每 4px 采样一次 alpha，断言最暗处落在 8dp 附近、28dp 之外为 0",
            anchor: SpecRef::of(MATERIAL, "L3 外层软阴影"),
        },
        CheckItem {
            id: "material.stroke-inset",
            dimension: Dimension::Material,
            criterion: "L4 描边 1dp `surface.stroke`，绘制在容器边界内侧（inset），不改变外部尺寸",
            observable: Observable::Pixel,
            sampling: "比较描边内侧与外侧量到的容器尺寸，两者相等即说明描边是 inset",
            anchor: SpecRef::of(MATERIAL, "L4 描边"),
        },
    ]
}

/// The 3.1.3 truncation and degradation rules.
fn truncation() -> Vec<CheckItem> {
    vec![
        CheckItem {
            id: "space.truncate-cell",
            dimension: Dimension::Space,
            criterion: "候选文本宽于 120dp 时尾部截断为 `…`；上屏仍使用完整文本",
            observable: Observable::Pixel,
            sampling: "量取超长候选单元的文本宽度，并确认末尾字形是 `…`",
            anchor: SpecRef::of(TRUNCATION, "候选文本 > `120dp` 宽"),
        },
        CheckItem {
            id: "space.truncate-32",
            dimension: Dimension::Space,
            criterion: "候选文本超过 32 字符时截断展示串，并在 `Candidate.annotation` 追加 `…`",
            observable: Observable::Pixel,
            sampling: "读超长候选的 annotation 字段，并量出截图中文本的字数上限",
            anchor: SpecRef::of(TRUNCATION, "候选文本 > 32 字符"),
        },
        CheckItem {
            id: "space.truncate-preedit",
            dimension: Dimension::Space,
            criterion: "拼音 preedit 宽于「候选框最大宽度 − 状态区宽度」时从左侧截断并绘制 `…`，不换行",
            observable: Observable::Pixel,
            sampling: "输入超长拼音串，检查 Header 是否仍为单行、左端是否为 `…`",
            anchor: SpecRef::of(TRUNCATION, "拼音 preedit 宽度 >"),
        },
        CheckItem {
            id: "space.overflow-pages",
            dimension: Dimension::Space,
            criterion: "候选总数超过 max_per_row × 5 时只展示前 5 页，状态区显示 `5/5+`",
            observable: Observable::Pixel,
            sampling: "读状态区的页码文本，并数出实际可见的候选页数",
            anchor: SpecRef::of(TRUNCATION, "候选总数 >"),
        },
        CheckItem {
            id: "space.short-screen",
            dimension: Dimension::Space,
            criterion: "屏幕可用高度不足时减少可见行数至至少 2 行；仍不足则只显示 1 行",
            observable: Observable::Pixel,
            sampling: "在受限高度下量出可见的候选行数",
            anchor: SpecRef::of(TRUNCATION, "屏幕可用高度不足"),
        },
        CheckItem {
            id: "space.narrow-screen",
            dimension: Dimension::Space,
            criterion: "屏幕可用宽度小于 220dp 时容器宽 = 屏幕宽 − 16dp、max_per_row 降为 3、字号不变",
            observable: Observable::Pixel,
            sampling: "量出容器宽度与每行候选单元数，并比较候选文本字形高度是否仍为 15sp",
            anchor: SpecRef::of(TRUNCATION, "屏幕可用宽度 <"),
        },
    ]
}

/// The 4dp grid rule and the exception list that qualifies it.
fn grid() -> Vec<CheckItem> {
    vec![
        CheckItem {
            id: "space.grid-4dp",
            dimension: Dimension::Space,
            criterion: "所有间距与尺寸默认是 4dp 的整数倍；除此之外的新尺寸都必须落在网格上",
            observable: Observable::Pixel,
            sampling: "把每个量到的物理像素距离除以 scale，断言为 4 的整数倍，或落在 3.1.4 的例外清单里",
            anchor: SpecRef::of(GRID, "显式例外清单"),
        },
        CheckItem {
            id: "space.exception-1dp",
            dimension: Dimension::Space,
            criterion: "例外 1dp：`stroke-width`、`separator-height`、`shadow-inner-offset-y`、候选单元 Focus Ring 描边",
            observable: Observable::Pixel,
            sampling: "量出这几条线的物理宽度，断言为 1 × scale（±1px）",
            anchor: SpecRef::of(GRID, "`1dp`"),
        },
        CheckItem {
            id: "space.exception-2dp",
            dimension: Dimension::Space,
            criterion: "例外 2dp：`shadow-inner-spread`（内层阴影的模糊半径）",
            observable: Observable::Pixel,
            sampling: "量出内层阴影从容器边缘向内的扩散距离",
            anchor: SpecRef::of(GRID, "`2dp`"),
        },
        CheckItem {
            id: "space.exception-6dp",
            dimension: Dimension::Space,
            criterion: "例外 6dp：`cursor-arrow-height`、`header-text-gap`、`grid-gap`、`cell-padding-v`、`number-gap`、`annotation-gap`",
            observable: Observable::Pixel,
            sampling: "量出箭头高度、Header 文本间距、网格间距、单元垂直内边距、序号间距与注音间距",
            anchor: SpecRef::of(GRID, "`6dp`"),
        },
        CheckItem {
            id: "space.exception-10dp",
            dimension: Dimension::Space,
            criterion: "例外 10dp：`header-padding-h`、`cell-padding-h`",
            observable: Observable::Pixel,
            sampling: "量出 Header 水平内边距与候选单元水平内边距",
            anchor: SpecRef::of(GRID, "`10dp`"),
        },
        CheckItem {
            id: "space.exception-34dp",
            dimension: Dimension::Space,
            criterion: "例外 34dp：`header-height`",
            observable: Observable::Pixel,
            sampling: "量出 Header 的高度",
            anchor: SpecRef::of(GRID, "`34dp`"),
        },
    ]
}
