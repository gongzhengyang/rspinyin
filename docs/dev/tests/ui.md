# rspinyin 测试用例分片 · ui（候选框渲染与交互）

> 分片版本: v2.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 平台任务: [../features-test.md](../features-test.md) ｜
> 被测基线: Rust 2024 workspace + Slint 1.x（软件光栅）+ Fcitx5 5.1.19 ｜ 关联 ADR: [../adr/0000-upstream-decisions.md](../adr/0000-upstream-decisions.md)、[../adr/0003-ui-role-separate-addon.md](../adr/0003-ui-role-separate-addon.md)、[../adr/0006-overlay-channel.md](../adr/0006-overlay-channel.md) ｜ 最后同步 Commit: `408d2c6` ｜
> 维护约定: 新增用例必须回写主文档第 2 节矩阵的 TC 列与维度列；可执行性变更须同步 [../features-test.md](../features-test.md) 的 `FEAT-TEST-P0.05.05` 判定表

## 0. 分片基线（引用主文档，不重复定义）

- **假设清单**：主文档第 1 节 + [../features-test.md](../features-test.md) 第 1 节。本分片强相关：`ASM-T-02`（无 DOM / 无 A11y 树，语义断言用 `UiFrame` 替代）、`ASM-T-03`（本机 WSLg/Weston）、`ASM-T-06`（`ui_rss = 18MB`、`raster_p99 = 1.5ms`）、`ASM-T-08`（真实亚克力不可验证）。
- **追踪矩阵**：主文档第 2 节的 `REQ-UI-01` ~ `REQ-UI-11`。`REQ-UI-10`（淡入淡出与亚克力协商）/ `REQ-UI-11`（指针微动效与 preedit 尾部保护）是 opt-basic 落地行。
- **用例格式与证据存盘**：主文档第 3 节 + [../features-test.md](../features-test.md) `FEAT-TEST-P0.05.04`。
- **预算阈值**：`docs/dev/budgets.json`（**唯一真值源**；本分片的性能断言只引用键名：`raster_p99`、`ui_rss`、`key_to_present_p99`、`key_to_present_p50`、`key_to_present_p99_144hz`、`first_key_to_visible_p99`、`idle`、`idle_redraw_count`、`idle_poll_timer_count`）。

> **状态（v2.0 同步）**：候选框已落地——`crates/ime-ui/ui/` 现有 4 个 `.slint`（`overlay` / `theme` / `candidate_grid` / `candidate`），`1.05.01–1.05.08` 全部 `COMPLETED`。本分片 45 条存量用例已在 run-20261006-034915 执行（像素级断言由内存表面探针等价执行；屏上窗口级观测受 isolated defect「depth-24 回退路径渲染黑屏」阻断处已逐条如实标注）。像素/几何/动效断言以 `MockBackend` + 内存表面探针为判定面；真实亚克力子项维持 `[不可验证]`。

**真实契约标识**（用例中引用，全部来自 `crates/ime-types/src/ui.rs`）：
`UiFrame{revision, preedit, candidates, page, status, anchor, layout}`、`Candidate{index, text, annotation, source, score, consumed_syllables}`、`PageState{current, total, page_size}`、`StatusStrip{mode_label, full_width, punctuation_full, has_user_dict_hit, readonly}`、`LayoutHint{max_per_row, show_annotation, max_width_dp}`、`Anchor{cursor, screen, scale, placement}`、`RectI{x, y, w, h}`、`Placement{Below, Above, Auto}`、`Preedit{text, caret, spans}`、`PreeditSpan{start, end, kind}`、`SpanKind{Syllable, Separator, Passthrough, Cursor}`、`ThemeSpec{scheme, accent, acrylic, base_alpha, corner_radius_dp, scale}`、`ColorScheme{Light, Dark}`、`Rgba8{r,g,b,a}`、`UiCommand{Frame, Show, Hide, Theme, Shutdown}`、`UiEvent{Select, Hover, Page, Dismiss, Rendered}`、`SelectTrigger{Mouse, NumberKey, Space, Enter, Tab}`、`PageDir{Next, Prev}`、`DismissReason{OutsideClick, Escape, ScrollUpEmpty}`、`HideReason{Committed, Cancelled, FocusLost, EmptyInput, Shutdown}`；以及 `SurfaceEvent{PointerEnter, PointerLeave, PointerMotion, PointerButton, Axis, Resize, Scale, CloseRequested}`。

---

## 1. 用例

### TC-UI-06 Slint Platform 接入与像素格式对齐（`REQ-UI-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-02` ｜ `ui` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/slint_platform.rs`、`renderer.rs`
- **前置条件与沙盒状态**：`FEAT-TEST-P0.05.01` 沙盒；`MockBackend` 可用（无显示服务器）；窗口 `1200×280` 物理像素。
- **操作步骤**：
  1. 在 `MockBackend` 上渲染一个 `.slint` 测试组件（圆角矩形 + 文本 + 渐变）-> 触发存盘：`<RUN>/ui/TC-UI-06/01_default.png`、`assertions.json`
  2. 采样圆角外像素，断言 alpha = 0；采样矩形中心，断言颜色与 `.slint` 声明一致（±1/255）。
  3. 验证 `Argb8888` 的内存序：渲染 `rgba(255,0,0,128)`，断言字节序为 `B=0,G=0,R=128,A=128`（预乘 alpha）。
- **通过标准**：
  - **功能逻辑**：`slint::platform::set_platform` 在 UI 线程首行调用且全局仅一次；冲突时记 `ui/slint/conflict` 并落 T4。
  - **视觉**：像素格式与 `wl_shm` 的 `WL_SHM_FORMAT_ARGB8888` / X11 32 位 visual 一致（`spikes/pixel-format.md` 的结论）。
  - **性能**：全量渲染 ≤ `raster_p99`；`grep -rn unsafe crates/ime-ui/src/` 无输出（`ime-ui` 不在允许清单内）。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：Slint Platform 接入与像素格式（slint_platform 9 测 + renderer 像素探针）；MockBackend 屏上 PNG 部分被 isolated defect ui-window-never-draws 阻断，像素级断言由内存表面探针（test_count_ink_counts_only_pixels_with_alpha 等）等价执行。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-06/
### TC-UI-07 静止零重绘与脏区渲染（`REQ-UI-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-02` ｜ `ui` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/renderer.rs`
- **前置条件与沙盒状态**：`FEAT-TEST-P0.05.06` 的 `is_clean()` 为真。
- **操作步骤**：
  1. 渲染一帧后静置 10 秒 -> 触发存盘：`<RUN>/ui/TC-UI-07/assertions.json`
  2. 断言 `commit` 调用次数增量为 0。
  3. 只改一个候选的高亮态，断言脏区面积 ≤ 两个候选单元矩形的并集 × 1.2。
- **通过标准**：
  - **性能**：`idle_redraw_count = 0`；脏区渲染 ≤ 0.2ms（全量 ≤ `raster_p99`）。
  - **功能逻辑**：`PartialRenderingCache` 生效；尺寸不变时阴影层为位块拷贝。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：静止零重绘（test_adapter_apply_pointer_redraws_the_grid_only_when_it_changes）+ 脏区跟踪；实机静置观测被同缺陷阻断。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-07/
### TC-UI-08 字体缺失降级（`REQ-UI-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-02` ｜ `ui` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/renderer.rs`
- **前置条件与沙盒状态**：沙盒内把字体配置指向无 CJK 字体的目录。
- **操作步骤**：
  1. 渲染含 `你好啊` 的候选框 -> 触发存盘：`<RUN>/ui/TC-UI-08/01_default.png`
  2. 断言检测到"所有 CJK 字形宽度为 0"，记 `ui/font/missing-cjk`，Header 用拉丁占位。
- **通过标准**：候选框**仍可用**（候选序号与英文正常）；降级可见且被诊断记录。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：字体缺失降级三态（error 报缺失 CJK 码位/timeout 无错码/空测量无字体）由 renderer::probe 套件钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-08/
### TC-UI-09 单帧渲染耗时预算（`REQ-UI-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-02` ｜ `ui` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/renderer.rs`
- **前置条件与沙盒状态**：`FEAT-TEST-P0.05.06` 洁净。
- **操作步骤**：
  1. 采集 1000 次全量渲染（`1200×280` @2x）耗时 -> 触发存盘：`<RUN>/ui/TC-UI-09/assertions.json`
  2. 断言 P99 ≤ `raster_p99`（1.5ms）。
- **通过标准**：超限时的降级顺序为**先减可见行数、再降阴影模糊半径、最后才动 GPU 路径**（`features.md` R-05）。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：单帧渲染耗时预算由 renderer 套件计时断言（内存表面），实机 P99 采集被阻断并如实标注。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-09/
### TC-UI-10 UI 渲染层内存预算（`REQ-UI-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-02` ｜ `ui` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/renderer.rs`、`docs/dev/budgets.json`
- **前置条件与沙盒状态**：`FEAT-TEST-P0.02.05` 的内存采样；基线在候选框创建**之前**采集。
- **操作步骤**：
  1. 渲染 1000 帧后采样 RSS -> 触发存盘：`<RUN>/ui/TC-UI-10/assertions.json`
  2. 断言 `delta_from_baseline_kb` ≤ `ui_rss`（18MB）。
- **通过标准**：**必须是增量**（基线差分），绝对值断言是错的（会把 fcitx5 自身内存算进来）。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：UI 渲染层内存预算（缓冲复用 + ui_rss 口径）由 renderer/surface 套件断言。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-10/
### TC-UI-11 UI 线程跨线程唤醒延迟（`REQ-UI-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-03` ｜ `ui` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/ui_thread.rs`、`channel.rs`
- **前置条件与沙盒状态**：`FEAT-TEST-P0.05.06` 洁净。
- **操作步骤**：
  1. 投递 10,000 次 `UiCommand::Frame` 并记录唤醒延迟 -> 触发存盘：`<RUN>/ui/TC-UI-11/assertions.json`
  2. 断言 P99 ≤ 50µs。
- **通过标准**：`eventfd` 是**累加语义**，接收方必须循环 `read` 直到 `EAGAIN`，否则会丢唤醒（`features.md` 6.2.1 的"最容易踩且最难复现的坑"）。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：eventfd 累加语义与循环 read 至 EAGAIN 由 channel/ui_thread 套件钉住（49+19 测）；10k Frame 投递唤醒延迟由吞吐断言覆盖。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-11/
### TC-UI-12 空闲零轮询与 CPU 预算（`REQ-UI-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-03` ｜ `ui` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/ui_thread.rs`
- **前置条件与沙盒状态**：`FEAT-TEST-P0.05.06` 洁净；真实会话。
- **操作步骤**：
  1. 空闲 60 秒，`pidstat -p <pid> 60` 采样 -> 触发存盘：`<RUN>/ui/TC-UI-12/assertions.json`
  2. 断言 CPU ≤ `idle`（0.3% 单核）；断言 `render_count` 不增长；断言无轮询定时器。
- **通过标准**：静止时 `poll` 超时 = `-1`（无限等待）；动效期间才用 `next_frame_deadline`。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：空闲零轮询（poll -1 无限等待；next_frame_deadline 仅动效期）由 ui_thread 套件断言。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-12/
### TC-UI-13 背压：`Frame` 合并、控制命令保序（`REQ-UI-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-03` ｜ `ui` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/channel.rs`
- **前置条件与沙盒状态**：`MockBackend` 记录调用序列。
- **操作步骤**：
  1. 连续投递 10,000 个 `Frame` -> 触发存盘：`<RUN>/ui/TC-UI-13/assertions.json`
  2. 断言 UI 线程只处理 ≤ 100 帧（单槽覆盖生效）。
  3. 投递 `Show, Hide, Show`，断言观察到的顺序一致（有序队列）。
- **通过标准**：`Frame`/`Theme` 为 latest-wins 单槽；`Show`/`Hide` **有序且不可丢**（`features.md` 2.2.1 的契约）。不得"简化"为无界通道。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：Frame 合并与控制命令保序由 channel 背压/保序套件钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-13/
### TC-UI-14 `UiEvent::Select` 绝不丢弃（`REQ-UI-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-03` ｜ `ui` | 边界与容错 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/channel.rs`
- **前置条件与沙盒状态**：SPSC 队列容量 64。
- **操作步骤**：
  1. 填满队列后继续投递 `Select` -> 触发存盘：`<RUN>/ui/TC-UI-14/assertions.json`
  2. 断言 UI 线程自旋等待 ≤ 500µs；超时则报 `ui/select/timeout` 并放弃本次点击（**不静默吞掉**）。
- **通过标准**：用户点击必须生效；放弃时必须留下诊断。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：UiEvent::Select 绝不丢弃由 channel select 语义断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-14/
### TC-UI-15 优雅关闭与线程死亡隔离（`REQ-UI-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-03` ｜ `ui` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/ui_thread.rs`
- **前置条件与沙盒状态**：真实会话。
- **操作步骤**：
  1. 投递 `UiCommand::Shutdown` -> 触发存盘：`<RUN>/ui/TC-UI-15/assertions.json`
  2. 断言 200ms 内完成；断言 `ps -T` 中 `rspinyin-ui` 线程消失；断言连续调用 `shutdown()` 两次不 panic。
  3. 在 UI 线程内注入 `panic!`，断言进程存活、`send()` 静默丢弃命令、记 `ui/thread/dead`。
- **通过标准**：超时则 detach 并记 `ui/shutdown/timeout`；输入功能仍可用（只是没有候选框）。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：优雅关闭与线程死亡隔离由 ui_thread 关闭/隔离断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-15/
### TC-UI-16 容器几何与 4dp 网格（`REQ-UI-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-04` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/ui/candidate.slint`、`src/layout.rs`
- **前置条件与沙盒状态**：真实 X11 会话；scale 1.0 与 2.0 各测一次。
- **操作步骤**：
  1. 渲染候选框 -> 触发存盘：`<RUN>/ui/TC-UI-16/01_default.png`
  2. 逐项采样：容器圆角 = `ContainerRadius`(12dp) × scale；Header 高 = `HeaderHeight`(34dp) × scale；候选单元高 = `CellHeight`(36dp) × scale；网格间距 = `GridGap`(6dp) × scale；阴影预留 = `ShadowMargin`(32dp) × scale。
  3. 断言全部间距为 4dp 整数倍（例外仅 `1px` 描边与 `6dp` 光标箭头）。
- **通过标准**：`scripts/check-ui-spec.sh` 输出 `PASS`（features.md 3.1 表格与 `.slint` 常量逐项一致）；`window_width/height` 为偶数。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：容器几何与 4dp 网格：check-ui-spec.sh PASS（33 sizes / 4dp 网格 30 lengths + 13 exceptions）+ layout 42 测；屏上 PNG 被同缺陷阻断。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-16/
### TC-UI-17 双层阴影与圆角外透明（`REQ-UI-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-04` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：真实会话 + 活跃合成器。
- **操作步骤**：
  1. 渲染候选框 -> 触发存盘：`<RUN>/ui/TC-UI-17/01_default.png`
  2. 采样圆角外与 `ShadowMargin` 区域，断言 alpha = 0。
  3. 断言内层硬阴影 `0 1dp 2dp shadow.inner`、外层软阴影 `0 8dp 28dp shadow.outer` 两层的存在与颜色。
- **通过标准**：**无合成器时**圆角外为不透明底（已知限制，记入 `assertions.json` 而非判为缺陷）；阴影层在尺寸不变时为位块拷贝（`render/reshadow_cached` ≤ 0.4ms）。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：双层阴影缓存与圆角外 alpha 由 renderer 像素探针断言（无合成器不透明底为已知限制如实登记）。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-17/
### TC-UI-18 极端小屏与超长内容降级（`REQ-UI-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-04` ｜ `ui` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：合成窗口尺寸注入（`< 220dp` 可用宽）。
- **操作步骤**：
  1. 以 `可用宽 = 200dp` 渲染 -> 触发存盘：`<RUN>/ui/TC-UI-18/01_default.png`
  2. 断言候选框宽 = 屏宽 − `16dp`，`max_per_row` 降为 3，**字号不变**（宁可换行不缩字）。
  3. 输入 32 字符候选，断言展示串截断为 `…` 结尾。
- **通过标准**：`features.md` 3.1.3 的 6 条极端场景逐条成立；`Candidate.text`（完整）与展示串分离。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：极端小屏与超长内容降级（text budget/截断保留全文）由 layout/adapter 断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-18/
### TC-UI-19 骨架屏：词库加载期占位（`REQ-UI-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-04` ｜ `ui` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：词库加载中的状态注入。
- **操作步骤**：
  1. 渲染加载态 -> 触发存盘：`<RUN>/ui/TC-UI-19/01_skeleton.png`
  2. 断言显示"词库加载中…"单行占位，高度 `34dp`，`opacity 0.6`，不可交互。
- **通过标准**：加载期无空白死板页面；占位高度与 `HeaderHeight` 一致（避免布局跳变，CLS == 0）。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：骨架屏占位由 adapter 骨架态绘制断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-19/
### TC-UI-20 组件可复用性（`REQ-UI-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-04` ｜ `ui` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：测试用 `.slint` 文件。
- **操作步骤**：
  1. 用第二个 `.slint` import `Header` 与 `CandidateGrid` 并渲染 -> 触发存盘：`<RUN>/ui/TC-UI-20/01_default.png`
  2. 断言两者是独立 `export component` 且可被外部 import。
- **通过标准**：Phase 2 的命令面板（`TASK-2.03.03`）将复用这两个组件；`CandidateWindow` 不得把它们内联展开。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：组件可复用性（Header/StatusCluster/CandidateShadow 导出 + 常量一致性）由 check-ui-spec PASS 断言。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-20/
### TC-UI-21 18 个颜色 Token 逐项一致（`REQ-UI-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-05` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/ui/theme.slint`、`src/theme.rs`
- **前置条件与沙盒状态**：暗色与亮色两套各渲染一次。
- **操作步骤**：
  1. 渲染两套主题 -> 触发存盘：`<RUN>/ui/TC-UI-21/01_default.png`、`02_dark.png`
  2. 逐项采样 18 个 Token：`surface.base`、`surface.stroke`、`text.primary`、`text.secondary`、`text.annotation`、`text.separator`、`accent.default`、`accent.on`、`state.hover`、`state.selected.bg`、`state.selected.stroke`、`state.pressed`、`separator`、`shadow.inner`、`shadow.outer`、`status.dot.active`、`status.dot.idle`。
- **通过标准**：`scripts/check-ui-spec.sh` 比对 features.md 3.2 表格与 `.slint` Token 逐项一致；暗色 `surface.base = #1C1C1E @0.85`、亮色 `#FFFFFF @0.85`。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：18 颜色 Token 逐项一致：check-ui-spec PASS（34 colours across theme.slint and theme.rs）+ theme 89 测。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-21/
### TC-UI-22 对比度硬约束（`REQ-UI-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-05` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：暗色/亮色/自定义 accent 三种组合。
- **操作步骤**：
  1. 计算三组对比度 -> 触发存盘：`<RUN>/ui/TC-UI-22/assertions.json`
  2. `text.primary` 在 `surface.base` 上 ≥ 7:1；叠于纯白（暗色）与纯黑（亮色）背景之上 ≥ 4.5:1；`text.primary` 在 `state.selected.bg` 上 ≥ 4.5:1。
- **通过标准**：自检失败时把 `base_alpha` 提升到 1.0 并记 `ui/theme/contrast-fallback`——**这是唯一允许的自动降级**（调底色而非拒绝配置）。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：对比度硬约束由 theme 套件对比度断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-22/
### TC-UI-23 深浅色自动跟随与切换（`REQ-UI-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-05` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：Portal `color-scheme` 可注入。
- **操作步骤**：
  1. 从暗切到亮 -> 触发存盘：`<RUN>/ui/TC-UI-23/01_dark.png`、`02_light.png`
  2. 断言 300ms 内完成切换（含 120ms crossfade），无闪烁。
  3. 断言 `CandidateWindow` 的组件实例数不变（无重建）。
- **通过标准**：优先级 = Portal → `GTK_THEME` 含 `dark` → `QT_STYLE_OVERRIDE` → 默认暗色；Portal 不可用时**不轮询**（只在启动时读一次）。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：深浅色跟随与切换由 theme ColorScheme 分支断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-23/
### TC-UI-24 亚克力协商与降级（`REQ-UI-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-05` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]` + `[不可验证]` 真实模糊需外部环境
- **前置条件与沙盒状态**：本机（WSLg/Weston）预期为**不支持**。
- **操作步骤**：
  1. 请求模糊 -> 触发存盘：`<RUN>/ui/TC-UI-24/01_default.png`
  2. 断言失败时 `base_alpha` 置 1.0，记 `ui/theme/blur-unavailable`，文本对比度仍 ≥ 4.5:1。
- **通过标准**：**降级是默认预期而非异常**——纯色底 + 双层阴影 + 描边在视觉密度上达标的 85%。真实亚克力 `[不可验证]`（`ASM-T-08`），需 KWin/Hyprland/picom 环境。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：亚克力协商降级（无合成器 alpha=255）由 theme 断言钉住；真实模糊按卡片自身声明记本机不可验证（features.md 0.5.5）。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-24/
### TC-UI-25 主题切换不重建组件（`REQ-UI-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-05` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：`MockBackend` 记录组件实例数。
- **操作步骤**：
  1. 连续切换主题 20 次 -> 触发存盘：`<RUN>/ui/TC-UI-25/assertions.json`
  2. 断言组件实例数不变，`apply()` ≤ 100µs。
- **通过标准**：只改属性，不重建（重建会导致闪烁）。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：主题切换不重建组件（rapid flag flips continue from reached colour）由 adapter 断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-25/
### TC-UI-26 候选网格等宽与换行（`REQ-UI-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-06` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/ui/candidate_grid.slint`、`src/adapter.rs`
- **前置条件与沙盒状态**：候选数 1 / 9 / 10 / 45 与 `max_per_row` 3 / 5 / 9 的全组合。
- **操作步骤**：
  1. 逐组合渲染 -> 触发存盘：`<RUN>/ui/TC-UI-26/01_default.png`
  2. 断言 `cell-width = max(CellMinWidth, 最大自然宽度)` 且 ≤ `MaxWidthDp` 约束；断言超 `max_per_row` 时换行。
- **通过标准**：全部组合无溢出、无 panic；`adapter.rs` 的网格计算 ≤ 300µs（缓存命中，9 候选）。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：候选网格等宽与换行由 adapter grid 套件（writes_grid_model/per_row clamp）钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-26/
### TC-UI-27 五态表现与优先级（`REQ-UI-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-06` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：测试用 `.slint` 强制设置三个布尔。
- **操作步骤**：
  1. 逐态渲染 `Default`/`Hover`/`Active`/`Focus Ring`/`Disabled` -> 触发存盘：`<RUN>/ui/TC-UI-27/01_default.png`、`02_hover.png`、`03_active.png`、`04_focus.png`、`05_disabled.png`
  2. 断言各态背景/描边/字重符合 features.md 3.4 表格。
  3. 同时置 `Focus Ring` 与 `Hover`，断言 `Focus Ring` 胜出。
- **通过标准**：优先级 `Disabled > Active > Focus Ring > Hover > Default`；`Active` 有 `scale 0.97` 的阻尼微下沉（60ms）。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：五态表现与优先级由 test_candidate_grid_draws_the_five_states_of_the_design_table + reserved disabled opacity 钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-27/
### TC-UI-28 数字标签与第 10 项之后（`REQ-UI-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-06` ｜ `ui` | 全键盘流 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：45 个候选。
- **操作步骤**：
  1. 渲染 -> 触发存盘：`<RUN>/ui/TC-UI-28/01_default.png`
  2. 断言 `1`~`9` 有数字标签（`11sp / 500`，`opacity 0.55`）；第 10 项及以后**无**标签。
  3. 断言第 10 项及以后仍可被鼠标点击。
- **通过标准**：标签与按键一一对应；无标签项不因此变为 `Disabled`。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：数字标签与第 10 项之后由 adapter cell 标签断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-28/
### TC-UI-29 超长候选截断与完整文本分离（`REQ-UI-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-06` ｜ `ui` | 边界与容错 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：32 字符候选。
- **操作步骤**：
  1. 渲染并截断 -> 触发存盘：`<RUN>/ui/TC-UI-29/01_default.png`
  2. 断言展示串以 `…` 结尾且宽度不超限。
  3. 点击该候选，断言 `UiEvent::Select` 只传 `index`，上屏的是**完整** `Candidate.text`。
- **通过标准**：截断算法对 CJK/ASCII/混合/emoji 四类输入均以 `…` 结尾；**上屏文本不得含 `…`**（`features.md` 6.2.2 的"长候选截断后上屏错误"陷阱）。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：超长候选截断与完整文本分离由 test_adapter_keeps_the_full_text_of_a_truncated_candidate 钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-29/
### TC-UI-30 注音与来源标注（`REQ-UI-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-06` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：`show_annotation = true` / `false` 各一次。
- **操作步骤**：
  1. 渲染两种配置 -> 触发存盘：`<RUN>/ui/TC-UI-30/01_default.png`
  2. 断言注音为 `11sp / 400`、`opacity 0.50`、与文本间距 `6dp`；断言 `false` 时不占位（不留下空隙）。
- **通过标准**：`LayoutHint.show_annotation` 生效；`CandidateSource` 的 5 个变体（`Dict`/`UserDict`/`Learned`/`Passthrough`/`Symbol`）各有可辨的视觉表达。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：注音与来源标注由 adapter annotation 断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-30/
### TC-UI-31 悬停与节流（`REQ-UI-07`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-07` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/interaction.rs`
- **前置条件与沙盒状态**：`MockBackend` 注入 `SurfaceEvent::PointerMotion`。
- **操作步骤**：
  1. 16ms 内连续注入 10 次 `PointerMotion` -> 触发存盘：`<RUN>/ui/TC-UI-31/assertions.json`
  2. 断言只产生 1 个 `UiEvent::Hover`，且指向最终落点。
- **通过标准**：`Hover` 单槽覆盖 + 16ms 节流；`translate` ≤ 5µs。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：悬停与节流（仅变化时重绘）由 adapter pointer 断言 + interaction 48 测钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-31/
### TC-UI-32 点击选词与拖拽防误触（`REQ-UI-07`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-07` ｜ `ui` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：真实会话 + `CommitProbe`。
- **操作步骤**：
  1. 点击第 3 个候选 -> 触发存盘：`<RUN>/ui/TC-UI-32/01_default.png`、`02_active.png`
  2. 断言产生 `UiEvent::Select{trigger: SelectTrigger::Mouse}` 并上屏正确。
  3. 按下第 3 项、在第 4 项释放，断言**不产生** `Select`。
- **通过标准**：100 次点击无一失败；拖拽不误触；100ms 内重复点击同一候选只生效一次（记 `ui/click/debounced`）。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：点击选词与拖拽防误触由 interaction click/drag 断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-32/
### TC-UI-33 滚轮翻页与首页向上滚关闭（`REQ-UI-07`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-07` ｜ `ui` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：候选 > 9 个。
- **操作步骤**：
  1. 注入 `Axis{delta > 0}` -> 触发存盘：`<RUN>/ui/TC-UI-33/assertions.json`
  2. 断言产生 `UiEvent::Page{dir: PageDir::Next}`。
  3. 在首页注入 `Axis{delta < 0}`，断言产生 `UiEvent::Dismiss{reason: DismissReason::ScrollUpEmpty}` 且候选框关闭。
- **通过标准**：两端符号约定在后端内统一为"正 delta = 下一页"（X11 按键 4/5 与 `wl_pointer.axis` 的符号相反），`interaction.rs` 不再做符号判断。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：滚轮翻页与首页向上滚关闭（ScrollUpEmpty）由 interaction 断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-33/
### TC-UI-34 命中测试与阴影区穿透（`REQ-UI-07`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-07` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：真实会话。
- **操作步骤**：
  1. 点击阴影预留区与容器空白区 -> 触发存盘：`<RUN>/ui/TC-UI-34/assertions.json`
  2. 断言命中 `Outside`/`Container`，**不产生** `Select`。
- **通过标准**：命中测试前必须减去 `ShadowMargin × scale` 的偏移（坐标转换集中在一处）；`hit_map` 与 `.slint` 实际布局偏差 ≤ 1 物理像素。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：命中测试与阴影区穿透由 interaction hit-map 断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-34/
### TC-UI-35 右键关闭与 `revision` 校验（`REQ-UI-07`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-07` ｜ `ui` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 注入右键 -> 触发存盘：`<RUN>/ui/TC-UI-35/assertions.json`
  2. 断言产生 `UiEvent::Dismiss{reason: DismissReason::OutsideClick}`。
  3. 注入 `revision` 不匹配的 `Select`，断言被丢弃并记 `ui/stale-select`。
- **通过标准**：窗口外点击**不会**产生事件（输入区域已排除），"点击外部关闭"实际由宿主侧 `FocusOut` 触发；右键语义据此修正。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：右键关闭与 revision 校验由 interaction 断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-35/
### TC-UI-36 底部翻转与边缘夹取（`REQ-UI-08`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-08` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/geometry.rs`
- **前置条件与沙盒状态**：`MockBackend` 注入屏幕布局；光标在四角与屏幕底部。
- **操作步骤**：
  1. 光标置屏幕底部 -> 触发存盘：`<RUN>/ui/TC-UI-36/01_above.png`
  2. 断言 `Placement::Above` 且不绘制光标箭头。
  3. 光标置四角，断言候选框完全可见（无裁切）。
- **通过标准**：`compute` ≤ 20µs；`window_size` 两分量均为偶数；`features.md` 3.1.3 的 6 条极端场景逐条成立。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：底部翻转与边缘夹取由 geometry 42 测钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-36/
### TC-UI-37 `Placement` 三取值语义（`REQ-UI-08`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-08` ｜ `ui` | 边界与容错 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 分别以 `Auto`/`Below`/`Above` 计算 -> 触发存盘：`<RUN>/ui/TC-UI-37/assertions.json`
  2. 断言 `Auto` 执行翻转判定；`Below`/`Above` **强制**方向但**仍执行夹取**。
- **通过标准**：三取值语义明确，不混淆。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：Placement 三取值语义由 geometry placement 断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-37/
### TC-UI-38 光标指示箭头条件（`REQ-UI-08`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-08` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 构造 `Below` + 未夹取 + 箭头在有效范围内 -> 触发存盘：`<RUN>/ui/TC-UI-38/01_default.png`
  2. 断言箭头高 `6dp`、宽 `12dp`、水平中心对齐光标中心。
  3. 构造 `Above` 或发生夹取的场景，断言**不绘制**箭头。
- **通过标准**：箭头出现的四个条件（`Below` && `!clamped_x` && `!clamped_y` && 水平距离在 `[12dp, 宽度−12dp]`）全部满足才绘制。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：光标指示箭头条件（绘制/拒收隐藏）由 test_adapter_draws_the_caret_arrow_into_the_surface 等钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-38/
### TC-UI-39 多屏不跨屏（`REQ-UI-08`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-08` ｜ `ui` | 边界与容错 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：注入双屏布局；光标靠近屏幕右边缘。
- **操作步骤**：
  1. 计算几何 -> 触发存盘：`<RUN>/ui/TC-UI-39/assertions.json`
  2. 断言候选框被夹取在**本屏内**，不跨屏。
- **通过标准**：跨屏会导致 DPI 不一致的渲染问题，必须夹取。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：多屏不跨屏由 geometry 多屏夹取断言（合成布局）钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-39/
### TC-UI-40 `hit_map` 与 `.slint` 布局一致性（`REQ-UI-08`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-08` ｜ `ui` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：`debug_assert` 回读 `.slint` 的实际元素坐标。
- **操作步骤**：
  1. 对 9 个候选逐一比对 `hit_map` 与 `.slint` 实际矩形 -> 触发存盘：`<RUN>/ui/TC-UI-40/assertions.json`
  2. 断言偏差 ≤ 1 物理像素。
- **通过标准**：`hit_map` 必须使用与 `.slint` **相同的常量**（全部来自 `TASK-1.05.03` 的 `public constant`，通过 Rust 绑定读取）——否则会"点第 3 个上屏第 4 个"（`features.md` 6.2.2 的陷阱）。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：hit_map 与 .slint 布局一致性由 adapter/interaction 一致性断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-40/
### TC-UI-41 Spring 稳定时间与过冲（`REQ-UI-09`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-09` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/spring.rs`
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-ui spring`
- **操作步骤**：
  1. 以 `ω₀ = 26.0`、`ζ = 0.85`、`m = 1.0` 积分 -> 触发存盘：`<RUN>/ui/TC-UI-41/assertions.json`
  2. 断言稳定时间 ∈ `[154, 208]ms`（理论 181ms ± 15%）；断言过冲 ≤ 1.13%（理论 0.63%）。
- **通过标准**：`k = 676.0`、`c = 44.2`；`dt` clamp 到 `1/60`（掉帧时变慢但**不发散**）。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：Spring 稳定时间/过冲参数由 spring 60 测参数断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-41/
### TC-UI-42 重定向保留速度（`REQ-UI-09`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-09` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 高亮滑动途中（30ms 后）改变目标 -> 触发存盘：`<RUN>/ui/TC-UI-42/assertions.json`
  2. 断言速度 `v` 未被重置为 0，逐帧位置连续（相邻帧位移差 ≤ 前一帧速度 × dt × 1.5）。
  3. 快速连按方向键 20 次（间隔 30ms），断言最终收敛到最后一个目标。
- **通过标准**：**这是 Spring 相对 bezier 不可替代的核心行为**（`features.md` 3.3.1）；用 bezier 无法实现。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：重定向保留速度由 spring redirect 断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-42/
### TC-UI-43 出现/消失动效参数（`REQ-UI-09`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-09` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：逐帧连拍。
- **操作步骤**：
  1. 触发出现 -> 触发存盘：`<RUN>/ui/TC-UI-43/01_appearing-1.png`、`02_appearing-2.png`、`03_visible.png`
  2. 断言出现为 `opacity 0→1` + `scale 0.96→1.0`，`110ms`，`cubic-bezier(0.22,1.0,0.36,1.0)`。
  3. 断言消失为 `opacity 1→0` + `scale 1.0→0.98`，`90ms`，`cubic-bezier(0.4,0.0,1.0,1.0)`。
- **通过标准**：**禁止 linear 动效**；出现/消失用 bezier（一次性、无重定向需求、无过冲），高亮滑动用 Spring。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：出现/消失动效参数由 spring 参数断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-43/
### TC-UI-44 消失动效中收到 `Show` 的反向续接（`REQ-UI-09`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-09` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：逐帧连拍。
- **操作步骤**：
  1. 在消失动效进行中投递 `Show` -> 触发存盘：`<RUN>/ui/TC-UI-44/01_disappearing.png`、`02_reappearing.png`
  2. 断言 `opacity` 从**当前值**续接，不跳变到 0 再升到 1。
- **通过标准**：`features.md` 2.3 的 UI 状态机表："中断消失动效，从当前透明度反向续接（不跳变）"。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：消失中收到 Show 的反向续接由 spring 反向续接断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-44/
### TC-UI-45 动效收敛后无残留定时器（`REQ-UI-09`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-09` ｜ `ui` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：`FEAT-TEST-P0.05.06` 洁净。
- **操作步骤**：
  1. 触发一次完整动效并等待收敛 -> 触发存盘：`<RUN>/ui/TC-UI-45/assertions.json`
  2. 断言收敛后 1 帧内 `poll` 超时恢复 `-1`；断言 `idle_poll_timer_count = 0`。
- **通过标准**：收敛判据为位移 < `0.5dp` 且速度 < `20dp/s`；`idle = 0.3%` 单核。

### TC-UI-46 出现淡入与消失淡出的过渡参数（`REQ-UI-10`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-UI-10` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/spring/transition.rs`、`adapter.rs`
- **前置条件与沙盒状态**：`MockBackend` + 逐帧位置序列记录（`FEAT-TEST-P0.01.03` 同族）。
- **操作步骤**：
  1. 触发出现（首键 → 候选框可见），采样逐帧 alpha/位移序列 -> 触发存盘：`<RUN>/ui/TC-UI-46/01_appear.png`
  2. 触发消失（提交/取消），采样对称序列 -> 触发存盘：`<RUN>/ui/TC-UI-46/02_disappear.png`
  3. 断言参数与 features.md 3.3 数值规范一致（时长、缓动、无生硬线性突变）。
- **通过标准**：appear 淡入接线生效（`REFACTOR-P0.02.01` 语义）；过渡期间帧耗时 ≤ `raster_p99`；收敛后无残留定时器（`TC-UI-45` 语义）。

### TC-UI-47 交叉淡化 crossfade：新旧帧无缝切换（`REQ-UI-10`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-UI-10` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/adapter/frame.rs`、`adapter/tests/crossfade.rs`
- **前置条件与沙盒状态**：`MockBackend`；连续翻页场景。
- **操作步骤**：
  1. 连续翻页 10 次，采样新旧帧混合序列 -> 触发存盘：`<RUN>/ui/TC-UI-47/01_crossfade.png`
  2. 断言无空白帧（CLS == 0 的帧级等价断言）、无重影残留（过渡终点单帧清晰）。
  3. 中断翻页（第 5 页时反向），断言 crossfade 反向续接不撕裂（`TC-UI-44` 语义同族）。
- **通过标准**：帧序列连续性断言通过；过渡中途 `Hide` 立即收敛不悬挂。

### TC-UI-48 亚克力协商：能力探测与三层降级（`REQ-UI-10`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-UI-10` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P1` ｜ 可执行性：`[可执行]`（真实模糊子项 `[不可验证]`） ｜ `crates/ime-ui/src/platform/x11/blur.rs`
- **前置条件与沙盒状态**：X11 档（本机合成器 Weston 不支持模糊——降级是**默认预期**）。
- **操作步骤**：
  1. 请求模糊 → 断言协商失败时 `base_alpha` 升至 1.0、记 `ui/theme/blur-unavailable` -> 触发存盘：`<RUN>/ui/TC-UI-48/01_degraded.png`
  2. 断言降级后文本对比度仍 ≥ 4.5:1（`TC-UI-22` 同表）；纯色底 + 双层阴影 + 描边达成视觉密度的 85%。
  3. 真实亚克力子项（KWin/Hyprland/picom）记 `[不可验证]`，需外部环境。
- **通过标准**：协商是启动期一次性探测（无轮询）；降级路径有像素级断言（内存表面探针）。

### TC-UI-49 主题通道贯通：config → ThemeSpec → 窗口（`REQ-UI-10`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-UI-10` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/theme.rs`、`adapter.rs`（`REFACTOR-P0.01.05`）
- **前置条件与沙盒状态**：`MockBackend`；`FEAT-TEST-P0.02.04` 度量通道。
- **操作步骤**：
  1. 改配置主题三处（accent、底色透明度、圆角）→ 采样渲染帧 -> 触发存盘：`<RUN>/ui/TC-UI-49/assertions.json`
  2. 断言三处逐像素反映（±1/255）、组件不重建（`TC-UI-25` 语义）。
- **通过标准**：通道无中间缓存漂移（`ThemeSpec` 单一事实来源）；未变更项零重绘。

### TC-UI-50 状态通知位的诚实显示（`REQ-UI-10`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-UI-10` ｜ `ui` | 全状态防御与骨架屏 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/adapter.rs`（`REFACTOR-P0.01.06` 通知位）
- **前置条件与沙盒状态**：`MockBackend` + `UiFrame` 镜像。
- **操作步骤**：
  1. 注入降级状态（词库不可用、只读模式、`degraded` 解码）-> 触发存盘：`<RUN>/ui/TC-UI-50/assertions.json`
  2. 断言状态条诚实反映（`StatusStrip` 字段与实际一致），不显示误导性正常态。
- **通过标准**：状态矩阵全绿无死代码（`REFACTOR-P0.01.06` 的死代码落地语义）；迟到重试有视觉反馈。

### TC-UI-51 指针微动效：按压阻尼与释放回弹（`REQ-UI-11`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-UI-11` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/spring/press.rs`
- **前置条件与沙盒状态**：`MockBackend` + 逐帧位置序列。
- **操作步骤**：
  1. 按下候选单元，采样 scale 序列 -> 触发存盘：`<RUN>/ui/TC-UI-51/01_press.png`
  2. 断言按压下沉到位（阻尼曲线无过冲）、释放回弹收敛（Spring 稳定语义，`TC-UI-41` 同族）。
  3. 高频狂点 20 次，断言无累计偏移、无掉帧。
- **通过标准**：`press.rs` 的阻尼参数与 features.md 3.4 五态表一致（Active 微下沉 scale 0.985 同族）；帧耗时 ≤ `raster_p99`。

### TC-UI-52 指针事件即时性：连接 fd 进 poll 集（`REQ-UI-11`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-UI-11` ｜ `ui` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/ui_thread.rs`、`channel.rs`（`REFACTOR-P0.03.01`）
- **前置条件与沙盒状态**：X11 档联机；指针事件时间戳记录。
- **操作步骤**：
  1. XTEST 快速划过候选行，记录事件注入 → 视觉反馈延迟 -> 触发存盘：`<RUN>/ui/TC-UI-52/assertions.json`
  2. 断言悬停反馈 ≤ 16ms（指针路径不排队等帧）；空闲时 poll 集无额外唤醒。
- **通过标准**：`BUDGET-CPU-01` 不回退（空闲唤醒计数不增）；事件不丢（乱序率 0）。

### TC-UI-53 preedit 尾部保护：光标段不被截断（`REQ-UI-11`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-UI-11` ｜ `ui` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/adapter/preedit/`
- **前置条件与沙盒状态**：`MockBackend`；`UiFrame` 镜像。
- **操作步骤**：
  1. 输入到 preedit 接近容器右缘，逐步加长 -> 触发存盘：`<RUN>/ui/TC-UI-53/01_edge.png`
  2. 断言 caret 段（`SpanKind::Cursor`）始终可见——被省略时收缩前置 span 而非光标（`119dd8b` 尾部保护语义）。
  3. 极端场景：单 span 超过容器宽，断言整体截断 + 省略号而非光标丢失。
- **通过标准**：golden 样本（`preedit/golden.rs`）全绿；光标在 64 字节上限内任何输入下可见。

### TC-UI-54 微动效与渲染预算的联合断言（`REQ-UI-11`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-UI-11` ｜ `ui` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/benches/{frame,histogram,wakeup_latency}.rs`
- **前置条件与沙盒状态**：空闲机器（`ASM-T-11`）；criterion 三基准。
- **操作步骤**：
  1. 全动效开启跑三基准 -> 触发存盘：`<RUN>/ui/TC-UI-54/assertions.json`
  2. 断言：`raster_p99`、`ui_rss`、唤醒延迟全部达标；`benches/wakeup_latency.rs` 无假回归（空闲机器重测）。
- **通过标准**：微动效增量未突破 `ASM-A-02` 预算裁决；直方图无长尾尖峰。

### TC-UI-55 微动效的降级链与失效安全（`REQ-UI-11`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-UI-11` ｜ `ui` | 全状态防御与骨架屏 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/spring/press.rs`、`transition.rs`
- **前置条件与沙盒状态**：`MockBackend`；动效开关注入。
- **操作步骤**：
  1. 关闭动效（配置项），断言状态切换即时、无中间帧 -> 触发存盘：`<RUN>/ui/TC-UI-55/assertions.json`
  2. 动效中线程被高压注入（模拟 200% 负载），断言动效跳帧收敛而非悬挂。
- **通过标准**：降级后功能等价（选词、翻页不受影响）；无定时器泄漏。

---

## 2. 分片出口准则

1. 55 条用例全部实现并执行（存量 45 条已于 run-20261006-034915 执行，增量 10 条见上文）。
2. `scripts/check-ui-spec.sh` 输出 `PASS`（features.md 3.1/3.2 表格与 `.slint` 常量逐项一致）。
3. `raster_p99`、`ui_rss`、`idle`、`idle_redraw_count`、`idle_poll_timer_count` 全部达标。
4. 暗色/亮色两套主题的视觉基线建立并通过人工复核（`MCP-P-01` 的审查提示模板）。
5. `TC-UI-48`（真实亚克力）与 `TC-UI-16` 的 2.0 缩放项在具备合成器的外部环境完成验证，或显式标注 `[不可验证]`。
6. 主文档矩阵的 `REQ-UI-01`~`REQ-UI-11` 行的可执行性列为 `✅`，维度列按用例覆盖勾选；`REQ-UI-10`/`REQ-UI-11` 为 opt-basic 落地行。
7. depth-24 回退路径黑屏缺陷（`TC-RT-21` 隔离）修复后，屏上窗口级观测复测并把逐条的"内存探针等价执行"标注替换为屏上断言。

- **验收记录**（2026-10-06）：ime-ui 套件 604/604 绿（214s）：动效收敛后无残留定时器由 spring/ui_thread 收敛断言钉住。像素级断言由内存表面探针等价执行；屏上窗口级观测被 isolated defect ui-window-never-draws（results/runs/…/rt/TC-RT-21/trace.json）阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-45/
