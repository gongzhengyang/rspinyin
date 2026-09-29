//! The 3.2, 3.3, 3.4 and 0.5.x items: the colours, the motion, the five states and the
//! degradations the capability matrix promises.
//!
//! Responsibility: state, for every colour token, every motion parameter, every component state
//! and every visually observable degradation, what a reviewer has to look at. Nothing here
//! measures anything; the items are instructions.
//!
//! # Why the colour tokens are one item each
//!
//! A token is the smallest unit the theme is written in, so a reviewer that reported "the
//! colours look right" would be reporting on eighteen decisions at once. One item per token means
//! a disagreement names the token -- which is the name the `.slint` source and the diagnostics
//! use, so the report can be acted on without a second round of reading.
//!
//! # Why a degradation is an item and not a note
//!
//! Half the capability matrix's cells are a promise about what happens when something is missing:
//! no compositor, no blur, no CJK font, a compositor that clamps a popup. Each of those has a
//! stated appearance, and an appearance nobody samples is a promise nobody keeps. The items below
//! therefore audit the degraded window rather than the ideal one, and the sampling says which
//! fact about the machine decides which answer is the right one.

use super::super::{CheckItem, Dimension, Observable, SpecRef};

/// The heading that opens the colour tokens.
const COLOUR: &str = "### 3.2";

/// The heading that opens the spring parameters.
const SPRING: &str = "#### 3.3.1";

/// The heading that opens the animation table.
const MOTION: &str = "#### 3.3.2";

/// The heading that opens the five-state table.
const STATE: &str = "### 3.4";

/// The heading that opens the platform baseline.
const PLATFORM: &str = "#### 0.5.1";

/// The heading that opens the capability matrix.
const MATRIX: &str = "#### 0.5.2";

/// Every 3.2, 3.3, 3.4 and 0.5.x item, in document order.
pub(super) fn items() -> Vec<CheckItem> {
    let mut items = colours();
    items.extend(motion());
    items.extend(states());
    items.extend(degradations());
    items
}

/// The rows of the 3.2 colour table.
///
/// The token's row is cited with the backticks the document writes it in, which is also what
/// keeps `separator` from resolving to the earlier `text.separator` row.
fn colours() -> Vec<CheckItem> {
    vec![
        CheckItem {
            id: "colour.surface-base",
            dimension: Dimension::Colour,
            criterion: "`surface.base` = 暗色 `#1C1C1E @ 0.85` / 亮色 `#FFFFFF @ 0.85`（候选框底）",
            observable: Observable::Token,
            sampling: "采样容器中心的纯底像素，比较 RGB 与 alpha（0.85 × 255 ≈ 217）",
            anchor: SpecRef::of(COLOUR, "`surface.base`"),
        },
        CheckItem {
            id: "colour.surface-stroke",
            dimension: Dimension::Colour,
            criterion: "`surface.stroke` = 暗色 `rgba(255,255,255,0.10)` / 亮色 `rgba(0,0,0,0.06)`（容器描边）",
            observable: Observable::Token,
            sampling: "采样容器 1dp 描边的像素，读出颜色与 alpha",
            anchor: SpecRef::of(COLOUR, "`surface.stroke`"),
        },
        CheckItem {
            id: "colour.text-primary",
            dimension: Dimension::Colour,
            criterion: "`text.primary` = 暗色 `#F2F2F7` / 亮色 `#1C1C1E`（候选文本、拼音串）",
            observable: Observable::Token,
            sampling: "采样候选文本与拼音串最实的字形像素颜色",
            anchor: SpecRef::of(COLOUR, "`text.primary`"),
        },
        CheckItem {
            id: "colour.text-secondary",
            dimension: Dimension::Colour,
            criterion: "`text.secondary` = 暗色 `rgba(242,242,247,0.62)` / 亮色 `rgba(28,28,30,0.60)`（状态区文本）",
            observable: Observable::Token,
            sampling: "采样状态区文本的字形像素颜色与 alpha",
            anchor: SpecRef::of(COLOUR, "`text.secondary`"),
        },
        CheckItem {
            id: "colour.text-annotation",
            dimension: Dimension::Colour,
            criterion: "`text.annotation` = 暗色 `rgba(242,242,247,0.48)` / 亮色 `rgba(28,28,30,0.45)`；该 Token 的 α 不得与 3.1.1 的 opacity 相乘",
            observable: Observable::Token,
            sampling: "采样注音与序号像素，断言有效 α 是 3.1.1 给出的 0.50 / 0.55，而不是 0.24 / 0.264",
            anchor: SpecRef::of(COLOUR, "`text.annotation`"),
        },
        CheckItem {
            id: "colour.text-separator",
            dimension: Dimension::Colour,
            criterion: "`text.separator` = 暗色 `rgba(242,242,247,0.40)` / 亮色 `rgba(28,28,30,0.35)`（拼音切分符）",
            observable: Observable::Token,
            sampling: "采样拼音切分符 `'` 的像素颜色与 alpha",
            anchor: SpecRef::of(COLOUR, "`text.separator`"),
        },
        CheckItem {
            id: "colour.accent-default",
            dimension: Dimension::Colour,
            criterion: "`accent.default` = 暗色 `#4C9AFF` / 亮色 `#0A6CFF`（强调色，可被系统强调色覆盖）",
            observable: Observable::Token,
            sampling: "采样首选项描边或状态指示点的像素；系统强调色生效时以 ThemeSpec.accent 为准并说明",
            anchor: SpecRef::of(COLOUR, "`accent.default`"),
        },
        CheckItem {
            id: "colour.accent-on",
            dimension: Dimension::Colour,
            criterion: "`accent.on` = `#FFFFFF`（强调色上的文本，当前未使用、预留）；窗口内若没有任何像素使用它，本项记 `无法判定` 并说明",
            observable: Observable::Token,
            sampling: "若强调色底上出现文本，采样其字形像素；没有这样的像素时按上述口径记 `无法判定`",
            anchor: SpecRef::of(COLOUR, "`accent.on`"),
        },
        CheckItem {
            id: "colour.state-hover",
            dimension: Dimension::Colour,
            criterion: "`state.hover` = 暗色 `rgba(242,242,247,0.08)` / 亮色 `rgba(28,28,30,0.06)`（鼠标悬停，非高亮项）",
            observable: Observable::Token,
            sampling: "悬停一个非高亮的候选单元，采样其背景像素",
            anchor: SpecRef::of(COLOUR, "`state.hover`"),
        },
        CheckItem {
            id: "colour.selected-bg",
            dimension: Dimension::Colour,
            criterion: "`state.selected.bg` = 暗色 `accent @ 0.18` / 亮色 `accent @ 0.14`（首选项 / 键盘高亮项背景）",
            observable: Observable::Token,
            sampling: "采样首选项的背景像素，并与强调色按该 α 合成后的值比较",
            anchor: SpecRef::of(COLOUR, "`state.selected.bg`"),
        },
        CheckItem {
            id: "colour.selected-stroke",
            dimension: Dimension::Colour,
            criterion: "`state.selected.stroke` = 暗色 `accent @ 0.55` / 亮色 `accent @ 0.50`（首选项描边）",
            observable: Observable::Token,
            sampling: "采样首选项 1dp 描边的像素，并与强调色按该 α 合成后的值比较",
            anchor: SpecRef::of(COLOUR, "`state.selected.stroke`"),
        },
        CheckItem {
            id: "colour.state-pressed",
            dimension: Dimension::Colour,
            criterion: "`state.pressed` = 暗色 `rgba(242,242,247,0.14)` / 亮色 `rgba(28,28,30,0.12)`（鼠标按下）",
            observable: Observable::Token,
            sampling: "按住一个候选单元，在按下期间采样其背景像素",
            anchor: SpecRef::of(COLOUR, "`state.pressed`"),
        },
        CheckItem {
            id: "colour.separator",
            dimension: Dimension::Colour,
            criterion: "`separator` = 暗色 `rgba(242,242,247,0.10)` / 亮色 `rgba(28,28,30,0.08)`（Header 分隔线）",
            observable: Observable::Token,
            sampling: "采样 Header 分隔线的像素颜色与 alpha",
            anchor: SpecRef::of(COLOUR, "`separator`"),
        },
        CheckItem {
            id: "colour.shadow-inner",
            dimension: Dimension::Colour,
            criterion: "`shadow.inner` = 暗色 `rgba(0,0,0,0.35)` / 亮色 `rgba(0,0,0,0.08)`（内层硬阴影）",
            observable: Observable::Token,
            sampling: "采样容器上边缘内侧最暗一个像素的颜色",
            anchor: SpecRef::of(COLOUR, "`shadow.inner`"),
        },
        CheckItem {
            id: "colour.shadow-outer",
            dimension: Dimension::Colour,
            criterion: "`shadow.outer` = 暗色 `rgba(0,0,0,0.42)` / 亮色 `rgba(0,0,0,0.16)`（外层软阴影）",
            observable: Observable::Token,
            sampling: "采样容器外最暗一层阴影像素的颜色",
            anchor: SpecRef::of(COLOUR, "`shadow.outer`"),
        },
        CheckItem {
            id: "colour.status-dot-active",
            dimension: Dimension::Colour,
            criterion: "`status.dot.active` = `accent.default`（中文模式指示点）",
            observable: Observable::Token,
            sampling: "中文模式下采样状态指示点的像素",
            anchor: SpecRef::of(COLOUR, "`status.dot.active`"),
        },
        CheckItem {
            id: "colour.status-dot-idle",
            dimension: Dimension::Colour,
            criterion: "`status.dot.idle` = 暗色 `rgba(242,242,247,0.35)` / 亮色 `rgba(28,28,30,0.30)`（英文模式指示点）",
            observable: Observable::Token,
            sampling: "英文模式下采样状态指示点的像素",
            anchor: SpecRef::of(COLOUR, "`status.dot.idle`"),
        },
    ]
}

/// The 3.3 motion parameters, sampled as frame sequences.
fn motion() -> Vec<CheckItem> {
    vec![
        CheckItem {
            id: "motion.appear",
            dimension: Dimension::Motion,
            criterion: "候选框出现：`opacity 0 → 1`、`scale 0.96 → 1.0`（锚点为光标侧边缘），110ms，`cubic-bezier(0.22, 1.0, 0.36, 1.0)`",
            observable: Observable::Frame,
            sampling: "以不少于 6 帧采样出现过程，量出首帧与稳定帧的容器外接框和整体 alpha",
            anchor: SpecRef::of(MOTION, "候选框出现"),
        },
        CheckItem {
            id: "motion.disappear",
            dimension: Dimension::Motion,
            criterion: "候选框消失：`opacity 1 → 0`、`scale 1.0 → 0.98`，90ms，`cubic-bezier(0.4, 0.0, 1.0, 1.0)`",
            observable: Observable::Frame,
            sampling: "以不少于 6 帧采样消失过程，量出末帧是否已完全透明",
            anchor: SpecRef::of(MOTION, "候选框消失"),
        },
        CheckItem {
            id: "motion.page-slide",
            dimension: Dimension::Motion,
            criterion: "翻页内容位移：`translateX ±12dp → 0`，160ms，Spring（ω₀ = 32.0，ζ = 0.90，稳定 ≈ 139ms）",
            observable: Observable::Frame,
            sampling: "翻页后逐帧量出内容区首个候选单元的 x 位移",
            anchor: SpecRef::of(MOTION, "翻页内容位移"),
        },
        CheckItem {
            id: "motion.resize",
            dimension: Dimension::Motion,
            criterion: "候选框尺寸变化：`width/height` 变化 140ms，Spring（ω₀ = 30.0，ζ = 0.92）",
            observable: Observable::Frame,
            sampling: "改变候选数后逐帧量出容器的宽与高",
            anchor: SpecRef::of(MOTION, "候选框尺寸变化"),
        },
        CheckItem {
            id: "motion.status-crossfade",
            dimension: Dimension::Motion,
            criterion: "状态图标切换：`opacity` crossfade 120ms，`ease-in-out`",
            observable: Observable::Frame,
            sampling: "切换中/英之后逐帧采样状态图标的 alpha",
            anchor: SpecRef::of(MOTION, "状态图标切换"),
        },
        CheckItem {
            id: "motion.theme-crossfade",
            dimension: Dimension::Motion,
            criterion: "主题切换：所有颜色 Token 120ms，`ease-in-out`",
            observable: Observable::Frame,
            sampling: "切换深浅色之后逐帧采样同一个背景像素的 RGB",
            anchor: SpecRef::of(MOTION, "主题切换"),
        },
        CheckItem {
            id: "motion.highlight-spring",
            dimension: Dimension::Motion,
            criterion: "高亮滑动：稳定时间（±2% 带）≈ 181ms、过冲 ≈ 0.63%；快速改向时保留当前速度 v 续接，不得重置为 0；位移 < 0.5dp 且速度 < 20dp/s 时判定收敛",
            observable: Observable::Frame,
            sampling: "逐帧记录高亮框的位置序列，断言收敛时间在 181ms 量级，并在位移 < 0.5dp 之后不再重绘",
            anchor: SpecRef::of(SPRING, "稳定时间"),
        },
        CheckItem {
            id: "motion.no-linear",
            dimension: Dimension::Motion,
            criterion: "禁止使用 linear 动效：所有动效必须使用 `Spring` 积分或带缓动的 `cubic-bezier`",
            observable: Observable::Frame,
            sampling: "对出现、消失、翻页三条动效各采样不少于 6 帧，把进度归一化后与直线拟合，残差为 0 即违反",
            anchor: SpecRef::of("### 3.3", "禁止使用 linear 动效"),
        },
        CheckItem {
            id: "motion.disabled",
            dimension: Dimension::Motion,
            criterion: "动效可关闭：`[ui.animation] enabled = false` 时全部动效时长置 0，Spring 积分器直接跳到 target",
            observable: Observable::Frame,
            sampling: "关闭动效后在同一场景采样两帧，断言第二帧已到稳态且与第一帧的差异只是最终状态",
            anchor: SpecRef::of(MOTION, "动效可关闭"),
        },
    ]
}

/// The five states of 3.4, plus the priority rule between them.
fn states() -> Vec<CheckItem> {
    vec![
        CheckItem {
            id: "state.default",
            dimension: Dimension::State,
            criterion: "`Default`：背景透明；文本 `text.primary`；序号 `text.primary` + `opacity 0.55`",
            observable: Observable::Pixel,
            sampling: "采样普通候选单元的背景像素（应等于容器底色）与序号字形 alpha",
            anchor: SpecRef::of(STATE, "`Default`"),
        },
        CheckItem {
            id: "state.hover",
            dimension: Dimension::State,
            criterion: "`Hover`：背景 `state.hover`；`border-radius 4dp`",
            observable: Observable::Pixel,
            sampling: "悬停一个非高亮单元，采样其背景像素与圆角",
            anchor: SpecRef::of(STATE, "`Hover`"),
        },
        CheckItem {
            id: "state.active",
            dimension: Dimension::State,
            criterion: "`Active`（按下）：背景 `state.pressed`；整体 `scale 0.97`（60ms）",
            observable: Observable::Pixel,
            sampling: "按住一个单元，在按下期间采样其背景像素并量出单元外接框（应缩到 0.97）",
            anchor: SpecRef::of(STATE, "`Active`"),
        },
        CheckItem {
            id: "state.focus-ring",
            dimension: Dimension::State,
            criterion: "`Focus Ring`（= 键盘高亮项）：背景 `state.selected.bg`；描边 `1dp state.selected.stroke`；文本 `15sp / 500`",
            observable: Observable::Pixel,
            sampling: "采样首选项的背景、1dp 描边与字形笔画密度",
            anchor: SpecRef::of(STATE, "`Focus Ring`"),
        },
        CheckItem {
            id: "state.disabled",
            dimension: Dimension::State,
            criterion: "`Disabled`：文本 `opacity 0.32`；无 hover 响应；鼠标指针为 `default`",
            observable: Observable::Pixel,
            sampling: "采样被禁用单元的文本 alpha，悬停后再次采样（两次应一致）",
            anchor: SpecRef::of(STATE, "`Disabled`"),
        },
        CheckItem {
            id: "state.priority",
            dimension: Dimension::State,
            criterion: "状态优先级 `Disabled` > `Active` > `Focus Ring` > `Hover` > `Default`；键盘高亮与鼠标悬停并存时以 `Focus Ring` 为准",
            observable: Observable::Pixel,
            sampling: "同时对同一个候选单元施加悬停与键盘高亮，采样其背景与描边",
            anchor: SpecRef::of(STATE, "状态优先级"),
        },
        CheckItem {
            id: "state.icon-states",
            dimension: Dimension::State,
            criterion: "状态图标五态：`opacity 0.72` / `0.92` + 24dp 圆形底 / 同 Hover + `scale 0.94` / 外圈 `2dp accent @ 0.7` offset 2dp / `0.28`",
            observable: Observable::Pixel,
            sampling: "逐个施加五个状态，采样图标的 alpha、圆形底直径与焦点外圈描边",
            anchor: SpecRef::of(STATE, "状态图标表现"),
        },
        CheckItem {
            id: "state.skeleton",
            dimension: Dimension::State,
            criterion: "词库加载期骨架：单行占位「词库加载中…」，高 34dp，`opacity 0.6`，不可交互",
            observable: Observable::Pixel,
            sampling: "在词库加载期截图，量出占位行的高度与 alpha，并点击一次确认无响应",
            anchor: SpecRef::of(STATE, "骨架屏"),
        },
    ]
}

/// The degradations the capability matrix promises, as they look on screen.
fn degradations() -> Vec<CheckItem> {
    vec![
        CheckItem {
            id: "degrade.blur-unavailable",
            dimension: Dimension::Degradation,
            criterion: "半透明亚克力背景：合成器模糊不可用时降级为 `alpha 1.0` 的 85% 不透明纯色底",
            observable: Observable::Pixel,
            sampling: "采样容器中心像素并断言 alpha = 255；同时比对 `runtime://ui_metrics` 的 base_alpha 与 expected_base_alpha",
            anchor: SpecRef::of(MATRIX, "半透明亚克力背景"),
        },
        CheckItem {
            id: "degrade.contrast-floor",
            dimension: Dimension::Degradation,
            criterion: "`text.primary` 在 `surface.base` 上的对比度 ≥ 7:1；亚克力不可用时最坏情况（底叠在纯白或纯黑之上）仍 ≥ 4.5:1",
            observable: Observable::Pixel,
            sampling: "采样文本前景与背景像素，按相对亮度算出对比度比值",
            anchor: SpecRef::of(COLOUR, "对比度硬约束"),
        },
        CheckItem {
            id: "degrade.no-compositor",
            dimension: Dimension::Degradation,
            criterion: "真透明（ARGB 32-bit）：无活跃合成器时降级为不透明底；此时圆角外不透明是已知限制，记入 `assertions.json` 而非判为缺陷",
            observable: Observable::Pixel,
            sampling: "读 `runtime://ui_metrics` 的 compositor_present 与 argb_visual，再采样圆角外像素的 alpha，按两者共同判定",
            anchor: SpecRef::of(MATRIX, "真透明（ARGB 32-bit）"),
        },
        CheckItem {
            id: "degrade.font-missing-cjk",
            dimension: Dimension::Degradation,
            criterion: "CJK 字体缺失时候选框仍可用：Header 降级为内置拉丁字形，并报 `ui/font/missing-cjk`",
            observable: Observable::Pixel,
            sampling: "在无 CJK 字体的沙盒里截图，断言候选序号与拉丁字形仍可读、中文字形宽度为 0 或为方框",
            anchor: SpecRef::of(PLATFORM, "Noto Sans CJK SC"),
        },
        CheckItem {
            id: "degrade.position-offset",
            dimension: Dimension::Degradation,
            criterion: "候选框绝对定位跟随光标：KWin / Mutter 档接受 ≤ 1px 的定位偏差（合成器夹取后按翻转与夹取规则重算）",
            observable: Observable::Pixel,
            sampling: "比较 hit_map 算出的容器原点与截图里容器的实际原点，量出两者的差",
            anchor: SpecRef::of(MATRIX, "候选框绝对定位跟随光标"),
        },
    ]
}
