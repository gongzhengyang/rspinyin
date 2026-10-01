# opt-basic phase-2 - P1 任务卡：材质、状态完善与配置诚实化（分片）

> 文档版本: v1.0 ｜ 系统形态: Desktop GUI（Linux 桌面输入法，双 cdylib）｜
> 架构基线: Rust 2024 workspace + Slint 1.13.1 软件光栅 + Fcitx5 5.1.7 C/C++ ABI ｜
> 关联 ADR: ./docs/dev/adr/（ADR-0011 由主文档 P0.01.01 建立；本分片无新增 ADR 诉求）｜
> 最后同步 Commit: `4def355` ｜
> Hub 回链: [../opt-basic.md](../opt-basic.md)（假设清单 ASM-B-*、缺陷总清单、追溯表、关键路径均在 Hub）｜
> 逻辑自检: [已通过（随 Hub 第 2 轮收敛；本分片卡全覆盖，无独立新增发现）] ｜
> 维护约定: 代码演进后必须回写 Hub §4.1 追溯表的本分片卡状态；卡间依赖只允许指向 Hub 已定义卡号或本文件编号更小的卡

**本分片定位**：P1 = 材质、次像素与网格视觉重塑 + 状态矩阵的完善收口。全部 10 张卡的前置依赖只落在 Hub 的 P0 卡或本文件编号更小的卡上；与专项文档的边界在每张卡「外部承接」行登记，禁止越界施工。

---

#### [REFACTOR-P1.01.01] REFACTOR-P1.01.01：死 token 清理与契约对齐

- **基本属性**：
  - 绑定缺陷编号：`DEF-29`
  - 优先级与难度预估：`P1` | 低复杂度 | 预估工时: 1.0 人天
  - 前置依赖：`REFACTOR-P0.02.01`（探针结论是「删还是接」的裁决依据）
  - 关键路径：`CP: 否`
  - 并行通道：`Track B`
  - 代码落地锚点：`crates/ime-ui/ui/theme.slint`、`crates/ime-ui/src/theme.rs`、`crates/ime-ui/src/theme/slint_palette.rs`、`crates/ime-ui/src/layout/metrics.rs`、`crates/ime-ui/src/spring/set.rs`、`crates/ime-ui/src/spring/highlight.rs`、`scripts/check-ui-spec.sh`（DEFERRED_TOKENS）
  - 外部承接：无
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`accent-on` 双侧声明零引用（`theme.slint:91`，已在 `check-ui-spec.sh:119` 的 DEFERRED_TOKENS 挂账）；`text-annotation` 自 v1.4 裁决后仅剩只读锁描边使用（`candidate.slint:304`），作为「注音文本色」的文档语义已死；`shadow-band-opacity`/`shadow-inner-opacity` 被 `metrics.rs:330-331` 解析、`theme.slint:131-135` 自认「无绘制路径读者」；`HighlightStep.damage` 由弹簧计算零读者（`spring/highlight.rs:242-275`），`spring/set.rs:26` 注释（"commits highlight.damage"）失实。
  - **商业标杆对标**：Token 体系零死项是 Linear/Things 3 级设计系统的基本纪律；死 token 是未来维护者的泥潭。
- **设计规范与参数定义**：
  - `accent-on`：保留声明 + DEFERRED_TOKENS 挂账（3.2 冻结表的行，删除即契约改动——不值）。doc 注明「首个实心 accent 面板落地时启用」。
  - `text-annotation`：把只读锁描边改回 `Theme.text-secondary`（锁是信息不是注音，3.1.1 的原意），`text-annotation` 回归「注音文本色」预留语义挂账——或按 v1.4 裁决删除其文本用途 doc。两案实施时按 `check-ui-spec.sh` 死项清单最小者落地。
  - `shadow-band-opacity`/`shadow-inner-opacity`：从 `Metrics` 解析中删除 + `theme.slint` 侧两个标量声明删除（band 表是唯一事实源，`theme/tests.rs` 的对齐断言同步瘦身）；`check-ui-spec.sh` 的 SIZE/常量行同步。
  - `HighlightStep.damage`：删除字段与计算（`highlight.rs:242-275`），`set.rs:26` 注释改为指向 renderer 的 damage 真源（`renderer.rs:222-233`）。
- **工程实现方案与代码级细节**：删除均按「测试先行改」——先改断言（`theme/tests.rs` 的峰值对齐测试、`slint_palette.rs` 若涉 band 表则不动表只动标量），再删声明，最后 `check-ui-spec.sh --self-test` 全绿。
- **交互矩阵与全状态覆盖**：不涉交互变化；深浅色双模式走查确认阴影视觉无回归（band 表未动）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 探针结论复核（Hub P0.02.01 的产出）确认各项处置方向。
  2. `[步骤 2]` 四项逐一删除/改注释，每项独立提交粒度。
  3. `[步骤 3]` `check-ui-spec.sh --self-test` 与 `just ci` 全绿。
- **验收标准 (DoD)**：
  - [ ] `Metrics` 无「解析后零读者」字段（新 grep 门禁加入 `check-self-tests`）。[自动]
  - [ ] DEFERRED_TOKENS 每项的挂账理由与 `features.md` 行号可追溯。[自动]
  - [ ] 阴影视觉无回归（band 表逐字节不变，`theme/tests.rs` 证明）。[自动]

---

#### [REFACTOR-P1.02.01] REFACTOR-P1.02.01：Overlay 渲染面（速查表/命令面板的绘制路径）

- **基本属性**：
  - 绑定缺陷编号：`DEF-06`（渲染与关闭两个断点；触发断点外部承接）
  - 优先级与难度预估：`P1` | 高复杂度 | 预估工时: 4.0 人天
  - 前置依赖：`REFACTOR-P0.01.01`（Overlay 命令需通道）、`REFACTOR-P0.01.05`（overlay 继承主题）
  - 关键路径：`CP: 否`
  - 并行通道：`Track B`
  - 代码落地锚点：`crates/ime-ui/ui/overlay.slint`（新建）、`crates/ime-ui/ui/candidate.slint`（窗口分支）、`crates/ime-ui/src/adapter/overlay.rs`（新建）、`crates/ime-ui/src/surface.rs`（`SurfaceUpdate::Overlay` 真身）、`crates/ime-ui/src/adapter/frame.rs`（DrawState 扩展）
  - 外部承接：触发键位与 `UiCommand` 契约 → `KEY-P2.02.01`/`KEY-P2.02.02`；命令面板功能与模糊搜索 → `ADD-FEAT-P1.02.01`；本卡只交付「`OverlayFrame` 到像素」的绘制面 + Escape 关闭回程。
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`SurfaceUpdate::Overlay` 仅把帧存进槽位（`surface.rs:202-209`），`.slint` 无任何 overlay 组件——三种面板（CheatSheet/CommandPalette/Diagnostics）的契约数据（`ime-types/src/ui.rs:84-110`：title/sections/selected/query）无渲染出口。
  - **商业标杆对标**：Raycast 面板：同族材质、键盘高亮滑动、Esc 即关；`features-add.md` C-1 约束要求与候选框共用 `SurfaceBackend`/`ThemeTokens`/弹簧。
- **设计规范与参数定义**：
  - **布局**（复用 3.1.1 几何常量，零新尺寸 token；组件复用裁决 IMPR-04）：行组件**复用候选网格的既有形态**（键位徽章 = `number-slot-width` 对齐规则、行高 = `cell-height`、文本/elide 规则同 `CandidateData`），仅新增「分组标题行」（`header-height-compact`，`text-secondary`）与「搜索框形态首行」（`OverlayFrame.query` 有值时：描边 + 光标条，复用 preedit 光标画法）两类元素；面板容器 = 候选窗容器同款（`container-radius`/`stroke`/双阴影层复用 `CandidateShadow`）。收益：`.slint` 新增面收敛到 ~60 行，`check-ui-spec.sh` 的常量白名单零扩展。
  - **键盘高亮**：`selected` 索引行画 `state-selected-bg` + `state-selected-stroke` 环；移动走既有四弹簧（highlight 机制全复用）。
  - **状态覆盖**：sections 空 → 空态行「无匹配项」（`text-secondary`）；rows 超页面 → 滚动由引擎分页（overlay 是模态，v1 不引入指针滚动）。
- **工程实现方案与代码级细节**：`adapter/overlay.rs` 把 `OverlayFrame` → 模型（`VecModel<OverlayRow>`，结构体含 section 标题/条目/选中位）；`candidate.slint` 的 `CandidateWindow` 增加 `overlay: OverlayPanel` 分支（frame 与 overlay 互斥显示，切换是属性写不重建）；Escape 关闭：UI 侧把 Escape 翻译为 `UiEvent::Dismiss { reason: OverlayClosed }`（`DismissReason` 若无此值则登记契约追加 ADR-0005——`DismissReason` 已有 `OutsideClick`/`ScrollUpEmpty`/`FocusLost`，追加一枚属增量）。
- **交互矩阵与全状态覆盖**：Default/Hover（v1 无指针）/Focus（selected 行）/禁用（无）；空态、长键位串截断（`max-text-width` 复用）、多 section 分组；打开/关闭与候选窗显隐的互斥矩阵（overlay 打开时 Hide 命令的语义：先关 overlay 再收窗）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `overlay.slint` 组件 + metrics 复用断言（`check-ui-spec.sh` 扩展）。
  2. `[步骤 2]` adapter 模型映射 + surface 分支 + 单测。
  3. `[步骤 3]` Escape 回程 + 与引擎侧（KEY 卡）联调。
- **验收标准 (DoD)**：
  - [ ] 注入 OverlayFrame（CheatSheet 三组八行）→ 截图行数/分组/高亮正确。[自动（mock 渲染断言）]
  - [ ] Esc 关闭 → `Dismiss` 到达引擎且窗口回候选态。[自动]
  - [ ] 打开/关闭全程零轮询、零新定时器（`BUDGET-CPU-01`）。[自动]
  - [ ] `just ci` 全绿。[自动]

---

#### [REFACTOR-P1.02.02] REFACTOR-P1.02.02：消失动效真渲染（Hide 先淡出后 unmap + 弹簧空转消除）

- **基本属性**：
  - 绑定缺陷编号：`DEF-22`
  - 优先级与难度预估：`P1` | 中复杂度 | 预估工时: 2.0 人天
  - 前置依赖：`REFACTOR-P0.02.01`（淡变靠 opacity 生效结论与接线方式）
  - 关键路径：`CP: 否`
  - 并行通道：`Track B`
  - 代码落地锚点：`crates/ime-ui/src/surface.rs`、`crates/ime-ui/src/adapter.rs`、`crates/ime-ui/src/ui_thread/event_loop.rs`、`crates/ime-ui/src/renderer.rs`、`crates/ime-ui/src/spring/set.rs`
  - 外部承接：无
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`set_visible(false)` 先 `window.hide()` 后启动 `motion.disappear()`（`adapter.rs:348-365`）——90ms 弹簧在不可见窗口里空转；普通 Hide 路径上 `advance` 报告 `animating` 使循环以 6.944ms 截止连醒（`surface.rs:260-273`），`render_if_dirty` 不查可见性（`renderer.rs:451-469`）——**每次隐藏违反 `BUDGET-CPU-01` 的空闲纪律，且 3.3.2 的退场淡出不存在**；`close()` 同病（`surface.rs:275-289`）。
  - **商业标杆对标**：macOS 弹出层退场与入场对称；「出现有动画、消失硬切」是最典型的半成品手感。
- **设计规范与参数定义**：
  - **时序**：`Hide` 到达 → 保持 mapped → `motion.disappear()`（α 1→0 + 缩放沿既有 `disappear_s`）→ 弹簧收敛（`is_settled()`）→ `window.hide()` + 属性复位。`UiThreadConfig` 的 shutdown 路径（`close()`）维持无动画直 unmap（预算 200ms 不让位）——两路径分工写进 `UiSurface::close` doc（其 doc 现在错误地承诺 close 可动画，一并修正）。
  - **预算**：淡出期间才允许 deadline 唤醒；收敛后零唤醒（回归 `ASM-B-04`）。
- **工程实现方案与代码级细节**：`adapter` 增 `pending_hide: bool`；`advance` 返回 `animating=false` 且 `pending_hide` 时执行真正的 `window.hide()`；`renderer.rs` 的 `render_if_dirty` 增「不可见且无 pending 时不进光栅」早退（消除空转的兜底，防御未来再引入同类路径）。
- **交互矩阵与全状态覆盖**：淡出中途来 Show → `appear()` 从当前 α 续接（`set.rs:120-127` 既有语义）；淡出中途进程 shutdown → 直收（测试覆盖竞态）；`[ui.animation] enabled=false` → Hide 直收不淡出。
- **逐步落地实施步骤**：
  1. `[步骤 1]` pending_hide 时序 + 收敛判定。
  2. `[步骤 2]` renderer 早退兜底 + doc 修正。
  3. `[步骤 3]` 竞态与开关回归测试。
- **验收标准 (DoD)**：
  - [ ] Hide 后 90ms 内 α 递减至 0，随后 unmap；淡出完成 60s 空闲零唤醒（`BUDGET-CPU-01` 回归断言）。[自动]
  - [ ] Show/Hide 快速交替 100 次无窗口残留、无状态错乱。[自动]
  - [ ] `just ci` 全绿。[自动]

---

#### [REFACTOR-P1.02.03] REFACTOR-P1.02.03：对比度门禁补全（五对降 α 文本纳入 ContrastReport）

- **基本属性**：
  - 绑定缺陷编号：`DEF-25`
  - 优先级与难度预估：`P1` | 低复杂度 | 预估工时: 1.0 人天
  - 前置依赖：`REFACTOR-P0.02.01`（α 生效结论决定「降 α」的真实值）
  - 关键路径：`CP: 否`
  - 并行通道：`Track B`
  - 代码落地锚点：`crates/ime-ui/src/theme.rs`、`crates/ime-ui/src/theme/tests.rs`、`crates/ime-ui/src/theme/color.rs`
  - 外部承接：无
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`ContrastReport`（`theme.rs:307-317`）只测全 α `text.primary` 三对；实际绘制的五对降 α 文本（序号 0.55、注音 0.50、音节分隔 0.40、mode 标签 0.62、直通 run 0.62）不在测。手工核算：不透明底全过，**亚克力最坏合成底 `#3E3E40` 上 ≈4.1/3.8/4.7/2.9 全线告急**——半透明档（P1.02.04 接通后）回归时，门禁不会发现用户最常读的文本不可读。
  - **商业标杆对标**：WCAG 纪律覆盖每一个绘制的文本对，而不是「主文本过了就行」。
- **设计规范与参数定义**：`ContrastReport` 扩为八对（既有三 + 五个降 α 对；降 α 用 `with_alpha` 展平后按 `color.rs:149-158` 既有算式测量）；阈值统一 `CONTRAST_MINIMUM = 4.5`（注音/序号属辅助文本，3.2 未给独立阈值——维持 4.5 一刀切，若五档设计值在亚克力档确实不达，则按既有退化阶梯自动升不透明，这正是门禁存在的意义）。
- **工程实现方案与代码级细节**：五对的 α 来源冻结为 3.1.1/v1.4 裁决值（0.55/0.50/0.40/0.62），与 `candidate_grid.slint`/`candidate.slint` 的绑定值同源（常量表共享，防止漂移）；`theme/tests.rs` 增「八对比率 ≥ 阈值（不透明档）」与「降级触发（亚克力档）」两组断言。
- **交互矩阵与全状态覆盖**：两 scheme × 两档（不透明/亚克力）四象限矩阵；accent 自定义色下的最坏对扫描（随机 50 色 fuzz，复用 `scheme.rs` 测试基建）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 八对扩展 + 常量同源化。
  2. `[步骤 2]` 四象限测试 + fuzz。
  3. `[步骤 3]` 若亚克力档不达：确认退化阶梯自动兜底路径并留档。
- **验收标准 (DoD)**：
  - [ ] 不透明档八对全过；亚克力档五对中不达项触发 `contrast-fallback`（断言降级而非断言达标）。[自动]
  - [ ] `just ci` 全绿。[自动]

---

#### [REFACTOR-P1.02.04] REFACTOR-P1.02.04：亚克力协商落地（BlurSurface → X11 blur 属性 + 请求时机）

- **基本属性**：
  - 绑定缺陷编号：`DEF-26`
  - 优先级与难度预估：`P1` | 中复杂度 | 预估工时: 2.0 人天
  - 前置依赖：`REFACTOR-P0.01.05`（`ThemeSpec.acrylic` 经主题通道到达）、`REFACTOR-P0.02.01`（半透明档的对比度兜底已门禁化）
  - 关键路径：`CP: 否`
  - 并行通道：`Track B`
  - 代码落地锚点：`crates/ime-ui/src/surface.rs`（`apply_theme` 真协商）、`crates/ime-ui/src/platform/x11.rs`（KDE blur 属性）、`crates/ime-ui/src/theme.rs`（`BlurSurface` 消费方）
  - 外部承接：Wayland 档的 blur 请求随四档后端启用（外部验证边界，`ASM-B-06`）。
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`apply_theme` 对 `spec.acrylic` 硬编码 `BlurNegotiation::Refused`（`surface.rs:361-377`），`base_alpha` 永远被顶到 255——设计的三档材质（0.5.2 退化阶梯）只余不透明档；`BlurSurface` 能力 trait 与 `request_blur`（`theme.rs:179-216`）零调用方；X11 后端无 `_KDE_NET_WM_BLUR_BEHIND_REGION`。
  - **商业标杆对标**：KWin/Hyprland 上的毛玻璃候选窗（Squirrel/macOS 质感的关键一档）。
- **设计规范与参数定义**：
  - **请求时机**：窗口映射后、首帧前，一次性请求容器矩形（含圆角内缩 1px——blur 区域大于圆角会在角上露噪点）；place 变更时随摆位更新区域。区域为空（窗口隐藏）不请求。
  - **协商语义**：X11 档——存在 `_KDE_NET_WM_BLUR_BEHIND_REGION` 原子且窗口管理器为 KWin 时设属性（`Applied`），否则 `Refused`（记 `ui/theme/blur-unavailable`，P0.01.06 通知位联动）；Hyprland 的 namespace 规则列实验室项。
- **工程实现方案与代码级细节**：`x11.rs` 增 `impl BlurSurface for X11Backend`（原子 intern + 属性写入，复用既有 `xcb` 连接封装）；`CandidateSurface` 增字段 `blur: Option<&mut dyn BlurSurface>` 的装配（平台探针时判定能力）；`apply_theme` 改为真协商。
- **交互矩阵与全状态覆盖**：KWin（Applied）/ 其他 WM（Refused）/ 用户关 acrylic（Disabled）三态；协商失败 → 不透明 + 通知位 + 日志一行；区域随摆位/缩放更新。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `BlurSurface for X11Backend` + 单测（mock X server 连接层）。
  2. `[步骤 2]` 协商接线 + 三态测试。
  3. `[步骤 3]` KWin 真机走查（实验室项登记）。
- **验收标准 (DoD)**：
  - [ ] mock 协商三态各得正确 `base_alpha` 与诊断码。[自动]
  - [ ] 非 KWin 环境零行为回归（恒不透明，与现状一致）。[自动]
  - [ ] `just ci` 全绿。[自动]

---

#### [REFACTOR-P1.03.01] REFACTOR-P1.03.01：交叉淡变接线（状态图标 120ms + 主题切换 120ms）

- **基本属性**：
  - 绑定缺陷编号：`DEF-27`
  - 优先级与难度预估：`P1` | 中复杂度 | 预估工时: 2.0 人天
  - 前置依赖：`REFACTOR-P0.02.01`（α 生效是颜色淡变的前提）
  - 关键路径：`CP: 否`
  - 并行通道：`Track B`
  - 代码落地锚点：`crates/ime-ui/src/adapter.rs`、`crates/ime-ui/src/adapter/frame.rs`（`write_status`）、`crates/ime-ui/src/spring/transition.rs`、`crates/ime-ui/ui/candidate.slint`（`StatusCluster`）
  - 外部承接：无
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`CubicBezier::EASE_IN_OUT`、`CROSSFADE_S` 与整套可中断过渡建成且测试齐全（`transition.rs:24-31,563-599`），但状态图标按颜色门硬切（`candidate.slint:261-306`）、主题切换是纯属性写（`adapter.rs:285-292`）——3.3.2 规定的两处 120ms crossfade 缺席，硬切在视觉上是最直接的「模板味」。
  - **商业标杆对标**：Linear/Raycast：一切状态变化有过渡；输入法模式点的硬切在余光里特别扎眼。
- **设计规范与参数定义**：
  - **状态点淡变**：每个 marker 的颜色由 `Transition<Color>` 驱动（active/idle 两端色，`EASE_IN_OUT`/`CROSSFADE_S`）；全宽/标点环的 `border-color` 同机制。transition 由 adapter 每帧 `step(dt)` 推进（复用动效期的既有 deadline 机制，不新增定时器；静止即收敛，空闲零唤醒）。
  - **主题切换淡变**：`Theme` 全局的颜色属性切换改为「旧值 → 新值」的 120ms 插值（在 adapter 侧持有 `Transition<ThemeTokens>`，逐 token 插值后写属性；八 bit 色通道整数插值保持确定性——`theme.rs` 的整数算术纪律延伸到时间维）。
- **工程实现方案与代码级细节**：`DrawState` 增 `marker_colors: [Transition<Rgba8>; 4]`；`apply_theme` 写入过渡而非直写；`advance` 推进所有未收敛 transition（`is_animating` 并入，复用既有收敛判定）；`check-ui-spec.sh` 的色值门禁改为「终值一致」断言（过渡只是时间维，终值逐字节不变）。
- **交互矩阵与全状态覆盖**：模式快切（50ms 内两次切换）→ 可中断续接（`transition.rs:157-172` 既有）；主题暗↔亮往返；`enabled=false` → 直切（快照确定性）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` marker 过渡 + adapter 推进。
  2. `[步骤 2]` 主题过渡 + 终值门禁改造。
  3. `[步骤 3]` 快切/往返/关闭三态回归。
- **验收标准 (DoD)**：
  - [ ] 切全角后 120ms 内 border-color 单调插值、终值逐字节等于 token。[自动]
  - [ ] 主题切换中帧颜色为两端合法插值、无闪烁帧。[自动]
  - [ ] 静止 60s 零唤醒；`just ci` 全绿。[自动]

---

#### [REFACTOR-P1.03.02] REFACTOR-P1.03.02：指针光标形状与 Active 按压微动

- **基本属性**：
  - 绑定缺陷编号：`DEF-28`
  - 优先级与难度预估：`P1` | 低复杂度 | 预估工时: 1.5 人天
  - 前置依赖：`REFACTOR-P0.03.01`（按压反馈依赖指针事件即时性才有意义）
  - 关键路径：`CP: 否`
  - 并行通道：`Track B`
  - 代码落地锚点：`crates/ime-ui/src/platform/x11.rs`（cursor 定义）、`crates/ime-ui/src/adapter.rs`（按压 spring）、`crates/ime-ui/ui/candidate_grid.slint`（按压缩放绑定）、`crates/ime-types/src/surface.rs`（若需指针形状事件）
  - 外部承接：无
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：候选窗内指针无光标形状管理（X11 默认 X 光标落在可点单元格上）；3.4 Active 行的「3% 下沉（60ms）」在 `candidate_grid.slint:136-139` 被显式让位给 3.3.2，而 3.3.2 也未实现——按压的唯一反馈是背景变色。
  - **商业标杆对标**：Things 3/Raycast 的按压缩放是「物理触感」的最小实现；悬停可点区域必有手型/箭头语义。
- **设计规范与参数定义**：
  - **光标形状**：命中 `Candidate`/`Container` → 系统箭头（可点击语义用 arrow，不夺焦点不受影响）；`Outside`（阴影预留）→ 透明穿透已有 `set_input_region` 保证，光标自然落回下层应用。实现走 X11 `cursordefine` + `XDefineCursor`（`x11.rs` 增一调用），Mock 后端记录调用。
  - **按压微动**：`pressed` 单元格套 `scale 0.985`（即 3% 中的可视部分，60ms `EASE_OUT` 单弹簧，`Spring1D` 既有）；变换实现用**几何缩放**（同 appear 动效的画法，Slint 无 transform）作用于格子背景与边框半径，文本随内容自然居中——超出 3% 视觉阈值的文本缩放不做（避免字形亚像素抖动）。
- **工程实现方案与代码级细节**：按压 spring 挂进 `AnimationSet`（第五个 motion，`is_animating` 收敛判定并入）；`candidate_grid.slint` 的 cell 增 `press-scale` 输入属性；快照测试锁帧（screenshot 接线后）。
- **交互矩阵与全状态覆盖**：按下（下沉）→ 移出取消 → 释放（回弹）三段；按压中翻页（press 失效回弹）；`enabled=false` 直切。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 光标形状 + mock 断言。
  2. `[步骤 2]` 按压 spring + cell 绑定。
  3. `[步骤 3]` 手感真机走查（60ms/3% 参数微调记录进卡）。
- **验收标准 (DoD)**：
  - [ ] 候选区光标为箭头、预留区为穿透（XTEST + 截图）。[实验室]
  - [ ] 按压 60ms 内 scale 单调至 0.985、释放回弹不振铃（spring 收敛判据）。[自动]
  - [ ] `just ci` 全绿。[自动]

---

#### [REFACTOR-P1.03.03] REFACTOR-P1.03.03：预编辑估宽校准与保尾修正

- **基本属性**：
  - 绑定缺陷编号：`DEF-24`
  - 优先级与难度预估：`P1` | 中复杂度 | 预估工时: 2.0 人天
  - 前置依赖：无
  - 关键路径：`CP: 否`
  - 并行通道：`Track B`
  - 代码落地锚点：`crates/ime-ui/src/adapter/cell.rs`（`character_em`）、`crates/ime-ui/src/adapter/preedit.rs`（`kept_start` 与 run 布局）、`crates/ime-ui/ui/candidate.slint`（裁切与渐隐位置）
  - 外部承接：无
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`ASCII_EM = 0.5` 对 `W/M/@/%`（≈0.8–0.95em）与数字（≈0.55em）系统性低估 → `kept_start` 保留过多 → 每个超宽 run 各自 `…` + `clip: true` 从右侧（最新输入端）裁切（`preedit.rs:302-333`、`candidate.slint:394-399`），与 3.1.3「保尾弃头」相反；左缘渐隐 overlay（`candidate.slint:438-443`）指着不存在的裁切。高估方向：希腊/西里尔按 1.0em 过早省略。
  - **商业标杆对标**：微软拼音长句预编辑永远右侧最新可见——「用户正在打的那一半」神圣不可裁。
- **设计规范与参数定义**：
  - **估宽分级**：`character_em` 从两档扩为四档——窄 ASCII（`i l j t f r . , ' : ; ! |` ≈0.30）、普通 ASCII（0.50）、宽 ASCII（`W M @ % m w` + 数字 0.55 → 统一 0.60 上界）、非 ASCII（1.0）。保持「无字体度量」边界（ASM-09），仅查表。
  - **保尾机制**：整体预算不足时只对**最左**的 run 起裁（run 序列从右往左保留至预算尽），被裁 run 以 `…` 收头而非收尾；`clip` 容器与渐隐 overlay 统一到左缘（现状 overlay 已在左缘，右侧不再可能出现裁切）。
- **工程实现方案与代码级细节**：`preedit.rs` 的布局算法改为「右对齐装箱」：从尾 run 起累计 `measure.width`，预算耗尽处的 run 记 `head_cut`（字符级，同既有 elide 步进）；`PreeditLayout` 增 `head_cut_run: Option<usize>` 字段，**被裁 run 的 `…` 前缀由 adapter 在产出 `PreeditRun.text` 时烘焙**（`.slint` 端零分支、渲染路径保持纯函数），`.slint` 侧无需任何改动。表驱动测试：混排（CJK+拉丁+数字+全角标点）20 例黄金集。
- **交互矩阵与全状态覆盖**：纯 CJK / 纯拉丁 / 混排 / 光标居中 / 光标随尾；预算从满到零的连续扫描（无离散跳变）；caret 不可见边界（截断吞掉 caret 的既有规则保持）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 估宽分级 + 黄金集基线。
  2. `[步骤 2]` 右对齐装箱重写 + `.slint` 渲染适配。
  3. `[步骤 3]` 长句真机走查（多 run 混排截断视觉）。
- **验收标准 (DoD)**：
  - [ ] 任何输入下：可见预编辑的**尾部**与输入串尾部一致（除非 caret 不可见）。[自动]
  - [ ] 全画面至多一个省略号。[自动]
  - [ ] `just ci` 全绿。[自动]

---

#### [REFACTOR-P1.04.01] REFACTOR-P1.04.01：配置面第二波接线（[data] / [diagnostics] / [ui.animation]）

- **基本属性**：
  - 绑定缺陷编号：`DEF-33`（其余 10 键：`[data]` 3 + `[diagnostics]` 3（`log_input_content` 刻意惰性除外）+ `[ui.animation]` 5 − 1 重复 = `durability`/`backup_enabled`/`backup_keep`/`level`/`log_rotation_mb`/`log_keep_files`/`enabled`/`zeta`/`appear_ms`/`disappear_ms`）
  - 优先级与难度预估：`P1` | 中复杂度 | 预估工时: 2.5 人天
  - 前置依赖：`REFACTOR-P0.01.05`（UI addon 配置步骤真身）；`REFACTOR-P0.04.01`（取证装配点）
  - 关键路径：`CP: 否`
  - 并行通道：`Track B`
  - 代码落地锚点：`crates/ime-fcitx5/src/addon/user_store.rs`、`crates/ime-fcitx5/src/addon/diagnostics.rs`、`crates/ime-fcitx5/src/addon/config.rs`、`crates/ime-ui/src/adapter.rs`（`set_motion_enabled`/弹簧参数）、`crates/ime-ui/src/spring.rs`、`crates/ime-ui-addon/src/addon.rs`
  - 外部承接：`engine.abbrev`（解码能力）→ `ADD-FEAT-P0.02.04`；`scheme.custom` → `ADD-FEAT-P0.02.01`。
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`[data]` 三键未应用（`user_store.rs:34-37` 自认：备份恒 enabled/keep 3/1 天，`Durability` 无读者）；`[diagnostics]` 三键被「订户先于配置安装」的时序卡死（`diagnostics.rs:25-31`：日志初始化在 config 步骤之前，级别/轮转/保留恒默认）；`[ui.animation]` 五键中 `enabled` 无生产调用方（`adapter.rs:443` 仅测试调用）、弹簧参数硬编码（`spring.rs:82-97`）。
  - **商业标杆对标**：配置文件的每个键都必须是真的——「写了不生效」比「没有这个键」更伤信任。
- **设计规范与参数定义**：
  - **时序解法（diagnostics 三键）**：两阶段初始化——订户仍先装（保 `BUDGET-LAT-05`），但 sink 的级别/轮转参数做成可热更（`DiagHandle` 增 `reconfigure(level, rotation, keep)`，写路径原子换参——**换参走原子指针/槽位交换，日志写线程无锁读当前参数**，绝不持锁写；`tracing` 全局订户只装一次的约束不变，换的是自有 sink 的策略，不是订户）；config 步骤读得后调 `reconfigure`。
  - **data 三键**：`user_store.rs` 的 `BackupConfig::new(directory)` 改为从 `config.data` 组装（`backup_enabled=false` → `BackupConfig::disabled()` 既有变体；`backup_keep` 直传；`durability` → flush 线程的批量窗口参数，`ime-dict` 消费点在 `user_db/flush.rs`）。
  - **animation 五键**：`enabled` → `Adapter::set_motion_enabled` 生产调用（UI addon 启动时一次）；`zeta`/`appear_ms`/`disappear_ms` → `AnimationSet::new` 的 `MotionConfig` 从配置组装（`spring.rs` 的 `SpringParams::PAGE_SLIDE` 高亮弹簧常数维持内置——`zeta` 只作用于 appear/disappear，语义边界写进 schema doc）；窗口重建才生效（弹簧参数不可热更，登记限制）。
- **工程实现方案与代码级细节**：每键一条「配置 → 行为」单元测试（表驱动）；`xtask` 的 `config --validate` 若有则补键级断言；README 模板注释与实际行为终检（与 P1.05.01 联检）。
- **交互矩阵与全状态覆盖**：`enabled=false` 快照确定性（回归既有 `snap_all` 测试）；`backup_enabled=false` 下备份目录零写入；`durability=immediate` 时 flush 窗口为零（逐笔落盘，性能回归断言 `BUDGET-MEM/CPU` 不破）；`level=debug` 下日志行数变化可观测。
- **逐步落地实施步骤**：
  1. `[步骤 1]` DiagHandle::reconfigure + 两阶段接线。
  2. `[步骤 2]` data 三键接线 + 备份关闭路径测试。
  3. `[步骤 3]` animation 五键接线 + 启动注入。
  4. `[步骤 4]` 键级测试矩阵 + README 联检。
- **验收标准 (DoD)**：
  - [ ] 10 键各有行为断言；`schema.rs` doc 与行为一一对齐。[自动]
  - [ ] `just ci` 全绿；预算键不回归。[自动]

---

#### [REFACTOR-P1.05.01] REFACTOR-P1.05.01：文档与配置诚实化（README / 模板 / reload 声明 / 模块注释）

- **基本属性**：
  - 绑定缺陷编号：`DEF-32`、`DEF-34`（诚实化部分）、`DEF-40`
  - 优先级与难度预估：`P1` | 低复杂度 | 预估工时: 1.0 人天
  - 前置依赖：`REFACTOR-P0.01.06`（诊断码语义定型后再写文案）
  - 关键路径：`CP: 否`
  - 并行通道：`Track B`
  - 代码落地锚点：`README.md`、`README.zh.md`、`crates/ime-config/src/reload/load.rs`（`DEFAULT_CONFIG_TOML`）、`crates/ime-fcitx5/src/addon.rs`（`lifecycle/pending` 行）、`crates/ime-fcitx5/src/session_host.rs`（模块注释）、`crates/ime-types/src/version.rs`（只读引用）
  - 外部承接：reload 宿主槽位接线 → `REFACTOR-P2.05.05`；键位文档的快捷键速查表 → `dev-doc-readme` skill 产出与 `KEY-P2.03.02`。
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：① 两份 README 宣称「配置改动即下一键生效」（`README.md:136-138`、`README.zh.md:119-120`），与 ABI 现实相反；② 样例 `schema_version = 1`（实际 2，`version.rs:41`）且约 15 键缺席；③ `session_host.rs:31-39` 模块注释仍说「宿主未安装」，与 `addon/session.rs:144-150` 相反（运维会误判整个引擎未激活）；④ `lifecycle/pending` 行只报 ABI 洞，不告诉用户「所以你现在要重启」。
  - **商业标杆对标**：README 与行为逐字一致是底线；「需要重启」必须被说出来。
- **设计规范与参数定义**：
  - README：热重载句改为「当前版本修改配置后需重启 fcitx5 生效；重启前修改以日志 `lifecycle/pending` 提示」；样例以 `DEFAULT_CONFIG_TOML` 生成物为准（构建时校验两者一致——新增 `check-self-tests` 条目：README 代码块中的键集合 ⊆ schema 键集合，版本号相等）。
  - 首启模板：键注释补「生效方式」列（即时/重启/未接线）三值。
  - 模块注释：`session_host.rs` 头注释重写为现状态（已安装、安装点、二次安装拒绝语义）。
  - 诊断行：`lifecycle/pending` 文案补「修改将在重启 fcitx5 后生效」。
- **工程实现方案与代码级细节**：一致性门禁脚本化（防再漂移）：从 `schema.rs` 抽键名集合、从 README 抽代码块键集合做差集断言——放 `check-self-tests`。
- **交互矩阵与全状态覆盖**：不涉交互；中英双 README 同步改。
- **逐步落地实施步骤**：
  1. `[步骤 1]` README 双语 + 模板 + 版本。
  2. `[步骤 2]` session_host 注释重写 + pending 行文案。
  3. `[步骤 3]` 一致性门禁入 `check-self-tests`。
- **验收标准 (DoD)**：
  - [ ] README 键集合 == schema 键集合（门禁绿）。[自动]
  - [ ] grep 全库无「无需重启」类失实声明。[自动]
  - [ ] `just ci` 全绿。[自动]
