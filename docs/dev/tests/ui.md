# rspinyin 测试用例分片 · ui（候选窗渲染与交互）

> 分片版本: v3.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 执行器必读: [../test-architecture.md](../test-architecture.md) ｜ 模块形态: Desktop GUI（X11 档）｜ 逻辑自检: 已通过 ｜ 维护约定: 用例状态变更须同步主文档矩阵
>
> 基线引用：`ASM-T-02/03/04/05/06/08`。像素/几何断言以 `MockBackend` + 内存表面探针为判定面（`ASM-T-02`），屏上观测走 `capture`/`ui_metrics`/`test-mirror`；真实亚克力子项 `[不可验证]`。模块背书 = `cargo nextest -p ime-ui`（696 测）+ `bench/{frame,wakeup_latency}`。**吸收** 字段登记取代的 v2.0 用例号。

---

#### TC-UI-001 X11 后端与焦点零抢占（最高严重级红线）

- **基本属性**：`REQ-UI-01` ｜ `ASM-T-02/03` ｜ ui ｜ ①②④ ｜ `testd`（FocusGuard）+ `capture` + nextest ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `requires_screenshots: true` ｜ `["01_transparent.png", "02_focus_guard.png"]` ｜ 吸收：TC-UI-01~05、TC-RT-33/34 ｜ 锚点：`crates/ime-ui/src/platform/x11.rs:13-18`、`testd/guard.rs` ｜ 背书：x11 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：沙盒会话；ARG visual 可用（缺失时断言回退诊断 `platform/x11/no-argb-visual`）。
- **操作步骤与物理存盘动作**：
  1. 窗口呈现：ARGB 透明 + 描边 → 存 `01_transparent.png`。
  2. FocusGuard：注入前后焦点查询，`FocusStolen{by_candidate_window}` 即红 → 存 `02_focus_guard.png` + `execution_log.txt`。
  3. SHAPE 输入区域穿透（阴影预留区）；SHM 段无泄漏；单帧提交 ≤ `raster_p99` → `assertions.json`（budget_keys: `raster_p99`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`WM_HINTS input=False`；永不 `SetInputFocus`/grab（结构性，0.4 规则 5）。
  - **[视觉断言]**：透明区 alpha=0；描边无毛刺。
  - **[性能断言]**：单帧提交 ≤ `raster_p99`（1.5ms）；SHM 段计数稳定。

#### TC-UI-002 Slint Platform 接入与像素格式

- **基本属性**：`REQ-UI-02` ｜ `ASM-T-02` ｜ ui ｜ ①④ ｜ nextest（内存表面探针）｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-UI-06 ｜ 锚点：`crates/ime-ui/src/slint_platform.rs`、`renderer.rs` ｜ 背书：slint_platform 9 测 + 像素探针
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：`MockBackend`（无显示服务器）。
- **操作步骤与物理存盘动作**：
  1. 渲染测试组件；圆角外 alpha=0、中心色一致（±1/255）→ `execution_log.txt`。
  2. `Argb8888` 预乘 alpha 字节序断言 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`set_platform` 首行单次；冲突记 `ui/slint/conflict` 落 T4。
  - **[Headless 断言]**：像素格式与 `WL_SHM_FORMAT_ARGB8888`/X11 32 位 visual 一致。
  - **[性能断言]**：全量渲染 ≤ `raster_p99`；`grep -rn unsafe crates/ime-ui/src/` 无输出。

#### TC-UI-003 UI 线程与通道语义（背压/保序/Select）

- **基本属性**：`REQ-UI-03` ｜ `ASM-T-09` ｜ ui ｜ ②④ ｜ nextest + `bench/wakeup_latency` ｜ `[Acceptance 验收套件]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-UI-11~15 ｜ 锚点：`crates/ime-ui/src/{channel,ui_thread}.rs`（channel.rs:31-43 契约表）｜ 背书：channel/ui_thread 套件（49+19 测）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：进程内直驱。
- **操作步骤与物理存盘动作**：
  1. 10,000 `Frame` 投递：唤醒延迟 P99 ≤ 50µs、单槽覆盖生效 → `execution_log.txt`。
  2. `Show,Hide,Show` 保序；`Select` 队满自旋 ≤500µs、超时弃选留诊断 → `assertions.json`（budget_keys: `idle_poll_timer_count`）。
  3. 优雅关闭与线程死亡隔离 → 记录。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：Frame/Theme latest-wins 单槽；Show/Hide 有序不可丢；Select 丢单元预算语义；eventfd 累加语义循环 read 至 EAGAIN。
  - **[Headless 断言]**：不得「简化」为无界通道（契约红线）。
  - **[性能断言]**：唤醒延迟 P99 ≤ 50µs；空闲零轮询。

#### TC-UI-004 渲染性能与内存预算

- **基本属性**：`REQ-UI-02` ｜ `ASM-T-04/05/06` ｜ ui ｜ ④ ｜ nextest + `bench/frame` + `testd/memory` ｜ `[Acceptance 验收套件]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-UI-07/09/10 ｜ 锚点：`crates/ime-ui/src/renderer.rs` ｜ 背书：renderer 计时套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：空闲机器（`ASM-T-11`）；基线在窗口创建前采集。
- **操作步骤与物理存盘动作**：
  1. 1,000 次全量渲染（1200×280 @2x）P99 → `execution_log.txt`。
  2. 脏区渲染 ≤ 全量份额；静态阴影位块拷贝 → 记录。
  3. 渲染 1,000 帧 RSS 差分 → `assertions.json`（budget_keys: `raster_p99`, `ui_rss`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：静止零重绘（`idle_redraw_count=0`）；超限降级顺序 = 先减行数、再降阴影、最后 GPU（R-05）。
  - **[Headless 断言]**：内存必须是基线差分（绝对值断言错误）。
  - **[性能断言]**：P99 ≤ `raster_p99`；差分 ≤ `ui_rss`。

#### TC-UI-005 字体降级与加载占位

- **基本属性**：`REQ-UI-02/04` ｜ `ASM-T-02` ｜ ui ｜ ③ ｜ nextest + `capture` ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `true` ｜ `["01_missing_cjk.png", "02_loading.png"]` ｜ 吸收：TC-UI-08、19 ｜ 锚点：`crates/ime-ui/src/renderer.rs`（probe）、`candidate.slint` 骨架 ｜ 背书：renderer::probe 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：字体配置指向无 CJK 目录的 fixture。
- **操作步骤与物理存盘动作**：
  1. CJK 缺失渲染 → 存 `01_missing_cjk.png`；`ui/font/missing-cjk` 诊断 → `execution_log.txt`。
  2. 词库加载期「词库加载中…」占位（34dp/0.6 不可交互）→ 存 `02_loading.png`。
  3. `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：降级后候选框仍可用（序号/英文正常）。
  - **[视觉断言]**：占位高度/透明度符合规格；Header 拉丁占位可读。
  - **[性能断言]**：探测不进入稳态渲染路径。

#### TC-UI-006 布局、网格与五态

- **基本属性**：`REQ-UI-04` ｜ `ASM-T-02/10` ｜ ui ｜ ①③④ ｜ nextest + `ui_metrics` + golden diff ｜ `[Acceptance 验收套件]` ｜ P0 ｜ `true` ｜ `["01_grid_dark.png", "02_grid_hover.png", "03_grid_selected.png"]` ｜ 吸收：TC-UI-16~18、20、26~30 ｜ 锚点：`crates/ime-ui/ui/candidate_grid.slint`、`candidate.slint`、`layout.rs:33-52` ｜ 背书：layout/adapter 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：深浅双主题 fixture；`P1.03.01` 交付后挂 golden diff。
- **操作步骤与物理存盘动作**：
  1. 4dp 网格与容器几何（`ui_metrics` 度量）→ 存 `01_grid_dark.png` + `execution_log.txt`。
  2. 五态（Default/Hover/Active/Focus/Disabled）表现与优先级 → 存 `02/03.png`。
  3. 极端小屏降级、超长候选截断（全文上屏保留）、数字标签第 10 项后语义、注音/来源标注 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：cols ≤ `max_per_row`（3..=9）、≤5 页；组件可复用性（跨主题实例化）。
  - **[视觉断言]**：网格等宽、换行规则、双层阴影圆角外透明；golden diff（基线就绪后）零差异。
  - **[性能断言]**：布局计算在帧预算内（背书计时）。

#### TC-UI-007 主题、深浅色与亚克力

- **基本属性**：`REQ-UI-05` ｜ `ASM-T-02/08` ｜ ui ｜ ④ ｜ nextest + `capture` ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `true` ｜ `["01_light.png", "02_dark.png"]` ｜ 吸收：TC-UI-21~25、48、49 ｜ 锚点：`crates/ime-ui/src/theme.rs`、`ui/theme.slint`、`platform/x11/blur.rs` ｜ 背书：theme 对比度门套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：Token 表 fixture；合成器 blur 能力探测注入。
- **操作步骤与物理存盘动作**：
  1. 18 Token 逐项一致 + 对比度硬约束 → 存 `01_light.png`/`02_dark.png` + `execution_log.txt`。
  2. 深浅自动跟随与切换不重建组件 → 记录。
  3. 亚克力三层降级（真实模糊子项 → `本机不可验证`）；config→ThemeSpec→窗口贯通 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`theme.scheme=auto/light/dark`；降级阶梯（纯色底+双层阴影+描边为默认预期）。
  - **[视觉断言]**：文本对比度达标；切换无闪烁残帧。
  - **[性能断言]**：主题切换不重建组件（对象计数稳定）。

#### TC-UI-008 鼠标交互

- **基本属性**：`REQ-UI-06` ｜ `ASM-T-02` ｜ ui ｜ ①⑤ ｜ `testd`（指针注入）+ nextest ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `true` ｜ `["01_hover.png", "02_click_commit.png"]` ｜ 吸收：TC-UI-31~35 ｜ 锚点：`crates/ime-ui/src/interaction.rs`、`testd/input.rs` ｜ 背书：interaction 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：沙盒会话 + 镜像目录。
- **操作步骤与物理存盘动作**：
  1. 悬停高亮（节流）→ 存 `01_hover.png` + `execution_log.txt`。
  2. 点击选词上屏（读回断言）→ 存 `02_click_commit.png`；拖拽防误触 → 记录。
  3. 滚轮翻页/首页上滚关闭；`hit_map` 与 `.slint` 布局一致性；右键关闭 + revision 校验 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：命中测试精确到候选单元；阴影区穿透。
  - **[视觉断言]**：悬停态与镜像高亮索引一致。
  - **[性能断言]**：指针事件经连接 fd 进 poll 集（即时唤醒，无轮询）。

#### TC-UI-009 几何与屏幕避让

- **基本属性**：`REQ-UI-07` ｜ `ASM-T-03/08` ｜ ui ｜ ①② ｜ nextest + `ui_metrics` ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-UI-36~40 ｜ 锚点：`crates/ime-ui/src/geometry.rs:296` ｜ 背书：geometry 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：屏幕几何 fixture（边缘/多屏矩阵）。
- **操作步骤与物理存盘动作**：
  1. 底部翻转与边缘夹取 → `execution_log.txt`。
  2. `Placement` 三取值（Below/Above/Auto）语义；光标箭头条件 → 记录。
  3. 多屏不跨屏；`hit_map` 一致性 → `assertions.json`（多屏热插拔子项 🚫）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：候选窗永远完整可见（兜底位置保证）。
  - **[Headless 断言]**：几何纯函数、确定性。
  - **[性能断言]**：重算 ≤ 200ms 量级（既有规格口径）。

#### TC-UI-010 Spring 动效

- **基本属性**：`REQ-UI-08` ｜ `ASM-T-02/04/05` ｜ ui ｜ ④ ｜ nextest + `bench/frame` ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-UI-41~45 ｜ 锚点：`crates/ime-ui/src/spring.rs`、`spring/`（ω₀=26、ζ=0.85）｜ 背书：spring 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：固定步长积分 fixture。
- **操作步骤与物理存盘动作**：
  1. 稳定时间与过冲（≈0.63% 理论值）→ `execution_log.txt`。
  2. 重定向保留速度；出现/消失参数；`Show` 反向续接 → 记录。
  3. 收敛后无残留定时器（`idle_poll_timer_count=0`）→ `assertions.json`（budget_keys: `raster_p99`, `idle_poll_timer_count`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：积分确定性（同输入同轨迹）。
  - **[Headless 断言]**：动效期间才用 `next_frame_deadline`。
  - **[性能断言]**：动效帧 ≤ `raster_p99`；收敛即停。

#### TC-UI-011 过渡、crossfade 与状态通知位

- **基本属性**：`REQ-UI-09` ｜ `ASM-T-02/08` ｜ ui ｜ ③④ ｜ nextest + `capture` ｜ `[Acceptance 验收套件]` ｜ P2 ｜ `true` ｜ `["01_fade_in.png", "02_crossfade.png"]` ｜ 吸收：TC-UI-46/47/50 ｜ 锚点：`crates/ime-ui/src/spring/transition.rs`、`adapter/frame.rs` ｜ 背书：transition 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：帧序列 fixture。
- **操作步骤与物理存盘动作**：
  1. 淡入/淡出参数 → 存 `01_fade_in.png` + `execution_log.txt`。
  2. crossfade 新旧帧无缝 → 存 `02_crossfade.png`。
  3. 状态通知位（StatusStrip）诚实显示 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：过渡中断可反向；状态位与真实模式一致（不撒谎的 UI）。
  - **[视觉断言]**：crossfade 无黑帧/跳变。
  - **[性能断言]**：过渡帧预算同 `raster_p99`。

#### TC-UI-012 指针微动效与 preedit 尾部保护

- **基本属性**：`REQ-UI-10` ｜ `ASM-T-02/04` ｜ ui ｜ ④⑤ ｜ nextest + `capture` ｜ `[Acceptance 验收套件]` ｜ P2 ｜ `true` ｜ `["01_press.png", "02_release.png", "03_preedit_tail.png"]` ｜ 吸收：TC-UI-51~55 ｜ 锚点：`crates/ime-ui/src/spring/press.rs`、`adapter/preedit/`、`ui_thread.rs` ｜ 背书：press/preedit 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：按压序列 fixture。
- **操作步骤与物理存盘动作**：
  1. 按压阻尼/释放回弹 → 存 `01_press.png`/`02_release.png` + `execution_log.txt`。
  2. 指针事件即时性（连接 fd 进 poll 集）→ 记录。
  3. preedit 光标段不被截断 → 存 `03_preedit_tail.png`；降级链失效安全 → `assertions.json`（budget_keys: `raster_p99`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：微动效失效不阻塞交互（失效安全）。
  - **[视觉断言]**：preedit 尾部完整可见。
  - **[性能断言]**：微动效与渲染预算联合断言不超 `raster_p99`。
