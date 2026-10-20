# rspinyin 测试用例分片 · rt（宿主桥与真实会话）

> 分片版本: v3.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 执行器必读: [../test-architecture.md](../test-architecture.md) ｜ 模块形态: Hybrid（GUI 面为主）｜ 逻辑自检: 已通过 ｜ 维护约定: 用例状态变更须同步主文档矩阵
>
> 基线引用：`ASM-T-01/02/03/04/08`。模块面判定 = `xtask testd`（XTEST + FocusGuard）+ `testd-client`（上屏读回）+ `test-mirror` + `cargo nextest -p ime-fcitx5`（475 测）+ `tests/keymap_matrix/`。**吸收** 字段登记取代的 v2.0 用例号。

---

#### TC-RT-001 Addon 生命周期与双 addon 注册

- **基本属性**：`REQ-RT-01` ｜ `ASM-T-01/04` ｜ rt ｜ ①② ｜ `testd` 沙盒 + nextest ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `requires_screenshots: false` ｜ `[]` ｜ 吸收：TC-RT-01~05、35 ｜ 锚点：`crates/ime-fcitx5/src/addon.rs:117-131`（INIT_STEPS）｜ 背书：addon 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：沙盒 fcitx5 会话；`runtime://env` 就绪三条件（test-architecture §4.1）。
- **操作步骤与物理存盘动作**：
  1. 双 addon 加载断言（`librspinyin.so` + `librspinyin_ui.so`）→ `execution_log.txt`。
  2. 加载耗时 ≤ `addon_load`；初始化失败不阻止 fcitx5 启动（fail-soft）→ 记录。
  3. 工厂符号动态可见；卸载线程清理 → `assertions.json`（budget_keys: `addon_load`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：10 步 INIT_STEPS 除 diagnostics 全部 fail-soft；`lifecycle/pending` 行为符合（宿主不回调时改动下次重启生效）。
  - **[Headless 断言]**：诊断流出现初始化完成标记作为 readiness。
  - **[性能断言]**：`addon_load` ≤ 120ms。

#### TC-RT-002 按键路由：翻译表、不吞键与 release

- **基本属性**：`REQ-RT-02` ｜ `ASM-T-10` ｜ rt ｜ ①②⑤ ｜ nextest + `keymap_matrix` ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-RT-06~10 ｜ 锚点：`crates/ime-fcitx5/src/engine/{router,arbiter,sequence}.rs` ｜ 背书：`keymap_matrix/`（2,568 行矩阵）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：进程内路由直驱 + 沙盒抽查。
- **操作步骤与物理存盘动作**：
  1. `KeyAction` 翻译表逐行 → `execution_log.txt`。
  2. 200 随机 keysym 不吞键（无效果即放行）→ 记录。
  3. release 一律不消费；序列和弦放弃时补交 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：认领规则（有效果或模式切换才 filterAndAccept）；临时英文全放行。
  - **[Headless 断言]**：数据表非控制流（翻译表为数据）。
  - **[性能断言]**：`on_key_event` 延迟在 criterion 阈值内（`bench/transport` 键）。

#### TC-RT-003 光标坐标三级来源与归一化

- **基本属性**：`REQ-RT-03` ｜ `ASM-T-03/08` ｜ rt ｜ ①② ｜ nextest + 沙盒 ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-RT-11~15 ｜ 锚点：`crates/ime-fcitx5/src/cursor/{resolver,sources}.rs`、`screen.rs` ｜ 背书：cursor 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：坐标 fixture（三级来源注入）。
- **操作步骤与物理存盘动作**：
  1. 三级来源降级与归一化 → `execution_log.txt`。
  2. 退化矩形/非法 scale/负坐标（多屏左侧）→ 记录。
  3. 位置抖动抑制；混合 DPI 子项 → `assertions.json`（子项记「本机不可验证」，`ASM-T-08`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：启发式判定 + 三级降级 + 兜底位置（屏幕下 1/3 居中）。
  - **[Headless 断言]**：负坐标不被错误夹取。
  - **[性能断言]**：解析在 key→present 预算内（不单列，总链覆盖）。

#### TC-RT-004 C ABI 契约：协商、兜底与布局

- **基本属性**：`REQ-RT-04` ｜ `ASM-T-01` ｜ rt ｜ ①② ｜ nextest ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-RT-16~20 ｜ 锚点：`crates/ime-fcitx5/src/ffi/abi/{types,engine,lifecycle,ui}.rs` ｜ 背书：ffi 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：FFI 直驱。
- **操作步骤与物理存盘动作**：
  1. 版本协商与拒绝路径 → `execution_log.txt`。
  2. 空指针/零长度参数不 panic；FFI panic 被 `catch_unwind` 兜底 → 记录。
  3. `UiPanelSnapshot` 序列化不泄漏；结构体布局编译期断言 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`extern "C"` 不越界 unwind（UB 红线）；崩溃文件落 `crash/`。
  - **[Headless 断言]**：keysym 位与 fcitx5 5.1.7 逐位钉死（`engine.rs:99-133`）。
  - **[性能断言]**：FFI 往返在 `bench/transport` 阈值内。

#### TC-RT-005 UserInterface 接管与可用性降级

- **基本属性**：`REQ-RT-05` ｜ `ASM-T-01/03` ｜ rt ｜ ①③④ ｜ 沙盒 + `test-mirror` + `capture` ｜ `[Acceptance 验收套件]` ｜ P0 ｜ `requires_screenshots: true` ｜ `["01_classicui.png", "02_takeover.png", "03_fallback.png"]` ｜ 吸收：TC-RT-21~25（v2 唯一红灯延续）｜ 锚点：`crates/ime-fcitx5/src/ui_impl/{takeover,panel,availability}.rs:42-59` ｜ 背书：ui_impl 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：沙盒会话；ClassicUI 在场（UIPriority=10 压制路径可测）。
- **操作步骤与物理存盘动作**：
  1. 接管生效：`updateAvailability()` 请求后自绘窗顶上 → 存 `01_classicui.png`、`02_takeover.png`。
  2. `available()` 判据与面板快照零分配复用 → `execution_log.txt`。
  3. 被拒一次永不再抢（`TAKEOVER_DECLINED`）；任何失败回退 ClassicUI → 存 `03_fallback.png`。
  4. `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：结构判据（接管状态机/回退语义）全过；光标矩形通道正确。
  - **[视觉断言]**：**沿用 v2.0 红灯记录**——depth-24 回退路径渲染黑屏（isolated defect `ui-window-never-draws`，trace 见 `results/runs/…/rt/TC-RT-21/`）；修复后按本卡复测，未修复前视觉判据保持失败隔离、不视为通过。
  - **[性能断言]**：接管切换不引入首帧延迟回退（`first_key_to_visible_p99` 键）。

#### TC-RT-006 真实会话端到端主链路

- **基本属性**：`REQ-RT-06` ｜ `ASM-T-01/02` ｜ rt ｜ ①⑤ ｜ `testd`（XTEST）+ `testd-client`（读回）｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `true` ｜ `["01_idle.png", "02_composing.png", "03_committed.png"]` ｜ 吸收：TC-RT-26~31 ｜ 锚点：`crates/ime-fcitx5/src/engine/router`、`testd/commit_readback.rs` ｜ 背书：沙盒 20/20 实证（v1 台账）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：全新沙盒会话；焦点在探针客户端；镜像目录清空。
- **操作步骤与物理存盘动作**：
  1. XTEST 注入 `nihao` → 候选窗呈现 → 存 `01_idle.png`、`02_composing.png`；FocusGuard 前后校验。
  2. 空格上屏 → 读回通道断言 commit = `你好` → 存 `03_committed.png`。
  3. 数字选词/翻页/Backspace/Escape/焦点切换/中英切换六个子序列 → `execution_log.txt`。
  4. `assertions.json`（budget_keys: `key_to_present_p50`, `key_to_present_p99`, `first_key_to_visible_p99`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：首词上屏逐字节正确；翻页/选词语义精确；焦点零被夺。
  - **[视觉断言]**：三态截图非空且与候选数/页态镜像一致。
  - **[性能断言]**：键→呈现 P50 ≤ `key_to_present_p50`、P99 ≤ `key_to_present_p99`（空闲机器采样，`ASM-T-11`）。

#### TC-RT-007 真实会话窗口行为与多应用兼容

- **基本属性**：`REQ-RT-07` ｜ `ASM-T-01/03` ｜ rt ｜ ②④ ｜ 沙盒 + `capture` ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `true` ｜ `["01_no_compositor.png", "02_input_region.png"]` ｜ 吸收：TC-RT-32~35 ｜ 锚点：`crates/ime-ui/src/platform/x11.rs:13-18` ｜ 背书：x11 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：多应用矩阵（≥3 类客户端）；无合成器档可选。
- **操作步骤与物理存盘动作**：
  1. 多应用（终端/编辑器/浏览器类）注入兼容性 → 存证 + `execution_log.txt`。
  2. X11 无合成器降级；输入区域（SHAPE）点击穿透 → 存 `01/02.png`。
  3. 长会话（≥30min）无泄漏抽样 → `assertions.json`（budget_keys: `plugin_rss`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：候选窗跟随、无跨屏、无残影。
  - **[视觉断言]**：输入区域外点击穿透生效（FocusGuard 无异常）。
  - **[性能断言]**：长会话 RSS 差分 ≤ `plugin_rss` 约束内。

#### TC-RT-008 Wayland 四档后端（本机不可验证）

- **基本属性**：`REQ-RT-08` ｜ `ASM-T-03/08` ｜ rt ｜ ①②③④ ｜ （需外部 Wayland 环境：layer-shell 合成器）｜ `[Acceptance 验收套件]` ｜ P2 ｜ `true` ｜ `["01_t1.png", "02_t2.png", "03_t3.png", "04_t4.png"]`（外部环境执行时）｜ 吸收：TC-RT-36~40 ｜ 锚点：`crates/ime-ui/src/platform/wayland/`（`mod.rs:56-81` 诚实登记）｜ 背书：代码级测试（未编译入本构建）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：`env_gate` 判 `NeedsEnvironment`（remedy：需 wlr-layer-shell 真机/嵌套合成器，features.md R-02）。
- **操作步骤与物理存盘动作**：
  1. 本机执行 → `execution_log.txt` 记录 `本机不可验证`（env_gate remedy 原文）。
  2. 外部环境执行时：T1 layer-shell 定位 → T2 xdg_popup → T3 canvas popup → T4 兜底回退，各存一图。
  3. `assertions.json`（本机：全部子项 `本机不可验证`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`keyboard_interactivity=none`（0.4 规则 5，焦点红线）代码级断言已由背书覆盖。
  - **[视觉断言]**：外部环境执行时各档截图非空且定位正确。
  - **[性能断言]**：不适用（本机）；外部执行时按 `key_to_present` 键采样。

#### TC-RT-009 跨 addon 帧通道（ADR-0011）

- **基本属性**：`REQ-RT-09` ｜ `ASM-T-01/09` ｜ rt ｜ ①② ｜ nextest + 沙盒 ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-RT-41~45 ｜ 锚点：`crates/ime-ui-addon/src/ffi/`、`crates/ime-fcitx5/src/ffi/abi/engine/transport.rs` ｜ 背书：`bench/transport` + ui-addon 125 测
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：双 addon 沙盒；probe 握手日志开。
- **操作步骤与物理存盘动作**：
  1. 工厂符号与双 probe 握手（加载顺序无关）→ `execution_log.txt`。
  2. 引擎→候选窗帧贯通；事件回程（Select/Page）→ 记录。
  3. caret→anchor 上行；UI addon 卸载资源回收 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：wire 为 repr(C) 序列化、无 SPSC 跨 addon（ADR-0011 语义）；混合 DPI 子项 🚫。
  - **[Headless 断言]**：dlsym 探测顺序（RTLD_DEFAULT→兄弟路径→裸 soname）可复现。
  - **[性能断言]**：帧通道延迟在 `bench/transport` 阈值内。

#### TC-RT-010 行为诚实化：模式位/焦点/挂起/日志热切换

- **基本属性**：`REQ-RT-10` ｜ `ASM-T-01` ｜ rt ｜ ①② ｜ 沙盒 + nextest ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-RT-46~50 ｜ 锚点：`crates/ime-core/src/state/effects.rs`、`crates/ime-fcitx5/src/{session_host,engine/context}.rs`、`crates/ime-diag/src/log.rs` ｜ 背书：session_host 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：沙盒会话；诊断级别热切开关。
- **操作步骤与物理存盘动作**：
  1. 中英模式位语义闭环（诚实切换，无幽灵状态）→ `execution_log.txt`。
  2. `on_focus_in/out` 会话回收；宿主挂起时窗口收起/恢复 → 记录。
  3. 日志策略热切换与运行期脱敏升级；UI 侧 FFI panic 兜底 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：焦点丢失不提交、不丢 preedit 状态语义。
  - **[Headless 断言]**：脱敏升级即时生效（新级别下旧字段也拒写）。
  - **[性能断言]**：模式切换在单解码周期内完成。
