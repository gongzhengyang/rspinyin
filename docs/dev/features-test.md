# rspinyin AI Agent + MCP 端到端自动化测试平台研发任务清单

> 文档版本: v1.0 ｜ 系统形态: Desktop GUI（Linux 桌面输入法：Fcitx5 进程内插件 + 自绘候选框） ｜ 被测基线: Rust 2024 workspace（7 crates + xtask）、Fcitx5 5.1.7 C++/C ABI、cargo-nextest 0.9.143 ｜ 关联 ADR: [adr/0000-upstream-decisions.md](adr/0000-upstream-decisions.md)、[adr/0002-rust-exports-addon-factory.md](adr/0002-rust-exports-addon-factory.md)、[adr/0003-ui-role-separate-addon.md](adr/0003-ui-role-separate-addon.md) ｜ 最后同步 Commit: `ee0dbfb` ｜ 维护约定: 被测代码演进后必须回写能力追溯表、任务卡状态与 `tests.md` 的追踪矩阵

本文档是 rspinyin 自动化测试**平台侧**的研发任务清单；用例侧见 [tests.md](tests.md) 与其分片。两份文档的事实来源是 [features.md](features.md) 的 0.5.2 能力矩阵、0.5.3 预算表与第 3 节交互规范。

---

## 0. 被测工程架构审查与商业化差距摘要

### 0.1 架构审查结论（阶段零实测）

| 维度 | 实测结论 |
|---|---|
| 工程形态 | Cargo workspace，`members = ["crates/*", "xtask"]`，`exclude = ["fuzz"]`（fuzz 需 nightly）。7 个 crate：`ime-types` / `ime-core` / `ime-dict` / `ime-config` / `ime-ui` / `ime-fcitx5` / `ime-diag` |
| 交付形态 | **两个 cdylib**（ADR-0003）：`librspinyin.so`（`Category=InputMethod`，引擎）+ `librspinyin-ui.so`（`Category=UI`，候选框）。二者**无 IPC**，数据经 Fcitx5 自身的 `InputContext`/`inputPanel()` 流转 |
| 渲染架构 | 候选框为**完全自绘**（Slint 软件光栅 + `wl_shm`/MIT-SHM），**无 Webview、无 DOM、无系统控件** |
| 现有测试资产 | **401 个 `#[test]`**：ime-types 44 / ime-core 112 / ime-dict 89 / ime-fcitx5 95 / **ime-ui 18** / xtask 43 / **ime-config 0** / **ime-diag 0**。criterion 基准 4 个（`ime-core/benches/{input,passthrough}.rs`、`ime-dict/benches/{dict,userdb}.rs`）。proptest 2 处。fuzz 目标 1 个（`fuzz/fuzz_targets/dag_build.rs`） |
| 质量门禁 | `just check`（fmt/clippy/nextest/doctest）、`just ci`（+5 个审计脚本 + budget + versions）、`just bench`、`just fuzz`、`just check-self-tests`（脚本自测） |
| 边界契约 | `ime-types` 为冻结契约（ADR-0001）；C ABI 为 `RspinyinVtable` + `RspinyinHandshake`（`crates/ime-fcitx5/src/ffi/abi/types.rs`） |
| 隐私约束 | 零网络（0.4 规则 6 / `BUDGET-NET-01`）；日志脱敏（`ime-diag` 的 `RedactLayer`）；`ime-ui` 公共 API 不得导出 Slint 类型（0.4 规则 11 / `OB-4`） |

### 0.2 实现进度与测试覆盖的真实边界（**决定本文档能测什么**）

来源：`.dev-progress.json`（`ledger_revision = 3`，39 个任务）。

| 状态 | 数量 | 任务 |
|---|---|---|
| `COMPLETED` | 1 | 1.02.02 |
| `READY_FOR_FINAL_GATE` | 13 | 1.01.01–03、1.02.01/03/06、1.03.01/02/04、1.04.01/02/05/06 |
| `IN_PROGRESS` | 6 | 1.02.04/05、1.03.03/06、1.06.01、1.08.01 |
| `PARTIAL` | 5 | 1.02.07、1.04.03/04、1.06.03、1.08.03 |
| `PENDING` | 14 | **1.03.05、1.03.07、1.04.07、1.05.01–1.05.08、1.06.02、1.07.01、1.08.02** |

**三条硬事实**（本文档与 `tests.md` 的全部用例都必须据此标注可执行性）：

1. **候选框 UI 完全不存在**：`crates/ime-ui/src/` 只有 `lib.rs`（`pub mod platform;`）、`platform/mod.rs`（`pub mod x11;`）、`platform/x11.rs`。**`crates/ime-ui/ui/` 是空目录，零个 `.slint` 文件**。1.05.01–1.05.08 全部 `PENDING`。
   ⇒ 一切"候选框视觉 / 微交互 / 5 态 / 动效 / 截图"类断言**今天无法执行**，只能锚定 features.md 第 3 节的**数值规范**与 `TASK-1.05.03` 定义的**`.slint` 常量名**（这些是冻结的真实名称，非臆造）。
2. **两个 crate 是空壳**：`ime-config/src/lib.rs` 与 `ime-diag/src/lib.rs` **只有文档注释，零代码、零测试**（对应 1.03.06 / 1.08.01 为 `IN_PROGRESS`）。任何针对配置热重载与日志脱敏的用例必须标注 `[待实现]`。
3. **Wayland 四档后端不存在**（1.04.07 `PENDING`），且**本机不可验证**（features.md 0.5.5：`wlr-protocols` 未安装、WSLg 合成器为 Weston）。X11 档**可验证**（1.04.06 已 `READY_FOR_FINAL_GATE`）。

### 0.3 商业化差距摘要（本测试体系要自动化拦截的目标）

| 断层 | rspinyin 的具体表现 | 自动化拦截手段 |
|---|---|---|
| Happy Path 与全状态矩阵 | 候选数为 0（仅 preedit）、词库不可用降级、只读模式、`degraded` 解码、页码越界 | `UiFrame` 快照断言 + 状态矩阵用例（`tests/ui.md`） |
| 视觉物理秩序 | 4dp 网格、`1px` 描边、双层阴影、85% 亚克力底、对比度 ≥ 4.5:1 | 像素采样 + 与 features.md 3.1/3.2 的数值逐项比对 |
| 微交互连续性 | Spring 重定向保留速度（稳定 181ms / 过冲 0.63%）、5 态优先级 | 逐帧位置序列连续性断言 |
| 暗部工程 | 帧耗时、空闲零重绘、内存漂移、崩溃回溯 | 探针直方图 + `/proc` 采样 + 崩溃文件断言 |
| 人机工学 | 全键盘可达、按键→像素 P99 ≤ 16ms、**永不夺取焦点** | 键位表驱动用例 + `xcb_get_input_focus` 不变断言 |

### 0.4 MCP 原语映射：**形态适配与显式替代**（严禁虚构）

被测系统是**进程内插件 + 自绘窗口**，不是浏览器或原生控件应用。skill 定义的部分原语在本形态下**不存在可对接的对象**，因此按下表**显式替代**，并在每条用例中标注所用原语的真实实现：

| skill 原语 | rspinyin 的真实实现 | 替代说明 |
|---|---|---|
| `action_interact` | **XTEST**（`x11rb::protocol::xtest`）注入指针事件到候选框坐标 | 真实可用；候选框坐标由 `X11Backend::geometry()` 与 `TASK-1.05.07` 的 `hit_map` 提供 |
| `action_keyboard` | **XTEST** 注入键事件 + `xdotool key --clearmodifiers` 兜底 | 真实可用；焦点必须在被测客户端窗口上 |
| `capture_viewport_screenshot` | **`XGetImage`**（`x11rb::protocol::xproto::get_image`）抓取候选窗口/全屏，导出 PNG | 真实可用；DPI 由 `X11Backend::geometry()` 的 scale 决定 |
| `get_accessibility_tree` | **⚠️ 不存在**。候选框为自绘，**无 A11y 树** | **替代**：`runtime://ui_frame`（探针导出的 `UiFrame` JSON 快照）+ 被测客户端应用的 AT-SPI 文本读取（用于校验上屏结果）。**不得声称存在 A11y 树** |
| `measure_animation_frame_rate` | 探针的帧计数器（`ProbeReport`）+ `X11Backend` 的 `commit` 调用序列 | 真实可用；X11 无 frame 回调，帧率由 UI 线程定时器驱动 |
| `runtime://logs` | tail `$XDG_DATA_HOME/rspinyin/logs/rspinyin.log`（`0600`） | 真实可用（1.08.01 `IN_PROGRESS`，日志尚未落盘 → 标注 `[待实现]`） |
| `runtime://style_computed` | **⚠️ 无 DOM/CSS** | **替代**：`UiFrame` 字段 + `.slint` `public constant`（`ShadowMargin`/`ContainerRadius`/`CellHeight` 等，来自 `TASK-1.05.03`）+ `X11Backend::geometry()`/`effective_base_alpha()` |
| `runtime://memory_profile` | `/proc/<fcitx5-pid>/smaps_rollup` 与 `/status` 的 `VmRSS` | 真实可用 |

---

## 1. 系统设计假设清单（Assumptions First）

`$4`（性能预算）与 `$5`（架构约束）未由入参指定，故按下表从 `docs/dev/budgets.json` 与 features.md 0.5.3 **原样引用**并登记。**严禁**用例中出现本表之外的阈值。

| 假设编号 | 维度 | 假设内容（基于阶段零实测） | 影响的用例域 | 偏离时的修正策略 |
|---|---|---|---|---|
| `ASM-T-01` | 形态 | 被测对象是 **fcitx5 进程内的两个 addon**，不是独立进程；测试必须驱动真实 `fcitx5` 会话 | 全部 `[实验室]` 用例 | 若改为 headless 引擎测试，则 `UiFrame`/上屏断言退化为单元级，端到端用例标记为不可执行 |
| `ASM-T-02` | 形态 | 候选框**完全自绘**，无 DOM / 无 A11y 树 / 无系统控件 | 视觉与交互用例 | 已按 0.4 显式替代；若引入 A11y 暴露（`TASK-3.04.02`），可新增语义树断言 |
| `ASM-T-03` | 硬件边界 | 本机为 **WSL2 + WSLg**：`DISPLAY=:0`（XWayland）、`WAYLAND_DISPLAY=wayland-0`、合成器 **Weston** | 环境能力边界 | 见 `ASM-T-08`；X11 档结论可移植，Wayland 档结论**不可** |
| `ASM-T-04` | 性能预算 | 引用 `budgets.json`：`key_to_present_p99 = 16.0ms`、`decode_p99 = 3.0ms`、`raster_p99 = 1.5ms`、`first_key_to_visible_p99 = 8.0ms`、`addon_load = 120.0ms` | 延迟与帧率用例 | 阈值变更须同步 `budgets.json` 与 features.md 0.5.3（三处一致） |
| `ASM-T-05` | 性能预算 | `idle = 0.3%` 单核、`idle_redraw_count = 0`、`idle_poll_timer_count = 0` | 空闲占用用例 | 若实测超限，先查是否引入了轮询定时器（0.4 规则 9） |
| `ASM-T-06` | 资源预算 | `ui_rss = 18MB`、`plugin_rss = 45MB`、`dict_mmap_rss = 25MB`、`so_stripped = 12MB`、`base_dict = 20MB` | 内存与体积用例 | 体积用例需在 packaging 后测（release profile **不得设 `strip`**，见 ADR-0002） |
| `ASM-T-07` | 数据量级 | 开发词库 `data/raw/base.tsv` = 5,871 行；`data/compiled/base.dict` = 248KB。完整 40 万词库不在仓库内 | 词库规模用例 | 规模类断言以**合成词库**为基准（`xtask dictc` 生成），不依赖真实大词库 |
| `ASM-T-08` | 硬件边界 | **本机不可验证**：`wlr-layer-shell` / KWin / Mutter 三档、真实亚克力模糊、多显示器热插拔、8 小时长稳 | Wayland 与长稳用例 | 必须标注"本机不可验证"并给出所需环境；**不得当作已通过**（features.md 0.5.5） |
| `ASM-T-09` | 并发模型 | 宿主线程与 UI 线程物理隔离；跨线程仅 SPSC 队列 + `eventfd`；**无异步运行时** | 并发与背压用例 | 若引入异步运行时，`BUDGET-NET-01` 与 `check-no-network.sh` 会直接失败 |
| `ASM-T-10` | 数据量级 | 单会话 `raw ≤ 64` 字节；候选 ≤ 45（5 页 × 9）；单候选文本 ≤ 32 字符 | 边界与容错用例 | 超限行为必须命中 `decode/too-long` 等**已冻结**错误码 |
| `ASM-T-11` | 形态 | 基准数值仅在**空闲机器**上有效（`.dev-progress.json` 明确记录：并行 agent 运行时 `passthrough/classify` 测得 511ns vs 空闲 726ns，criterion 自身报告了假回归） | 全部 `[性能]` 用例 | 性能用例必须在无其他 agent 运行时重测，并在验收记录中注明机器状态 |

---

## 2. 平台能力-任务覆盖追溯表 (Traceability Matrix)

`MCP-*` = MCP 协议原语与资源；`GUARD-*` = 自愈与隔离护栏。每项能力**至少**由一张 `FEAT-TEST` 任务卡承接。

| 能力编号 | 所属层 | 能力描述 | 绑定任务节点 |
|---|---|---|---|
| `MCP-T-01` | Tools | `action_keyboard`：XTEST 键事件注入（含修饰键组合、逐键录入、`--clearmodifiers` 兜底） | `FEAT-TEST-P0.01.01` |
| `MCP-T-02` | Tools | `action_interact`：XTEST 指针事件注入（精准位移、点击、长按、滚轮、右键） | `FEAT-TEST-P0.01.01` |
| `MCP-T-03` | Tools | `capture_viewport_screenshot`：`XGetImage` 抓取 + DPI 感知的坐标映射 + PNG 落盘 | `FEAT-TEST-P0.01.02` |
| `MCP-T-04` | Tools | `measure_animation_frame_rate`：帧计数器采样与掉帧检测 | `FEAT-TEST-P0.01.03` |
| `MCP-T-05` | Tools | `get_accessibility_tree` → **替代实现** `get_ui_frame`：探针导出的 `UiFrame` JSON 快照读取 | `FEAT-TEST-P0.02.01` |
| `MCP-T-06` | Tools | 客户端上屏文本读取（AT-SPI 到被测应用） | `FEAT-TEST-P0.02.02` |
| `MCP-T-07` | Tools | `action_engine`：进程内引擎直驱（不经 fcitx5），用于纯契约/解码断言 | `FEAT-TEST-P0.03.01` |
| `MCP-T-08` | Tools | `action_dict`：词库编译/加载/篡改注入（畸形条目、CRC 破坏、截断） | `FEAT-TEST-P0.03.02` |
| `MCP-R-01` | Resources | `runtime://logs`：`tracing` 日志与审计脚本输出抓取 | `FEAT-TEST-P0.02.03` |
| `MCP-R-02` | Resources | `runtime://style_computed` → **替代** `runtime://ui_metrics`：`UiFrame` + `.slint` 常量 + `X11Backend` 度量 | `FEAT-TEST-P0.02.04` |
| `MCP-R-03` | Resources | `runtime://memory_profile`：`smaps_rollup` / `VmRSS` 采样与漂移检测 | `FEAT-TEST-P0.02.05` |
| `MCP-R-04` | Resources | `runtime://budget`：`budgets.json` 与 criterion/probe 输出的比对 | `FEAT-TEST-P0.03.03` |
| `MCP-R-05` | Resources | `runtime://env`：fcitx5 版本、addon 加载状态、平台档位、合成器能力探测 | `FEAT-TEST-P0.02.06` |
| `MCP-P-01` | Prompts | `prompt://commercial_grade_visual_audit`：注入 features.md 3.1/3.2/3.4 数值规范的多模态审查提示 | `FEAT-TEST-P0.04.01` |
| `MCP-P-02` | Prompts | `prompt://flaky_and_self_heal`：定位漂移与异步竞态的根因归因提示 | `FEAT-TEST-P0.04.02` |
| `MCP-P-03` | Prompts | `prompt://ime_spec_audit`：把 features.md 0.5.2 能力矩阵逐行转为可判定断言的提示 | `FEAT-TEST-P0.04.01` |
| `GUARD-01` | 护栏 | 环境隔离：沙盒 XDG 目录、`base.dict` 与 `user.redb` 回滚、fcitx5 会话重置 | `FEAT-TEST-P0.05.01` |
| `GUARD-02` | 护栏 | 定位器自愈：坐标/常量漂移时按 `UiFrame` 与 `.slint` 常量重定位，标记 `[HEALED]` | `FEAT-TEST-P0.05.02` |
| `GUARD-03` | 护栏 | 自愈安全红线：禁止修改业务代码或放宽阈值；仅允许改测试脚本并产出 Patch 报告 | `FEAT-TEST-P0.05.03` |
| `GUARD-04` | 护栏 | 失败取证归档：`trace.json`、断言 Diff、图像缺陷定位、堆栈 | `FEAT-TEST-P0.05.04` |
| `GUARD-05` | 护栏 | 快照存盘规范：`RUN/<模块代码>/<TC-ID>/[step]_[state].png` + `assertions.json` + `index.md` + `results/index.json` | `FEAT-TEST-P0.05.04` |
| `GUARD-06` | 护栏 | 环境能力门禁：按 features.md 0.5.5 判定用例可执行性，不可验证项**必须显式标注**而非跳过 | `FEAT-TEST-P0.05.05` |
| `GUARD-07` | 护栏 | 性能测量纯净度：检测并行 agent 占用，拒绝在受污染机器上采信预算数值（`ASM-T-11`） | `FEAT-TEST-P0.05.06` |

**双向一致性**：上表 **25 项能力** → **20 张任务卡**（`FEAT-TEST-P0.01.01` ~ `FEAT-TEST-P0.05.06`），每张卡回填其承接的能力编号，无孤儿能力、无无主任务。

### 2.1 编号规则与 DAG 校验

编号格式 `FEAT-TEST-[优先级].[能力域序列].[任务序号]`，能力域序列：`01` 输入与视觉采集、`02` 运行时观测、`03` 引擎与词库直驱、`04` 提示模板与视觉审查、`05` 护栏与报告。

**校验结果**：**20 个任务、17 条依赖边**，全部满足 `dep < self`，无环、无自环、无重复边。设计中出现并已拆解的一处循环：

| 原始循环 | 解耦方式 | 冻结顺序 |
|---|---|---|
| `FEAT-TEST-P0.05.05`（环境能力门禁）需要 `FEAT-TEST-P0.02.06`（`runtime://env`）的探测结果；而 `runtime://env` 的实现又需要门禁来判定"哪些档位该探测" | 把**档位清单与判定规则**下沉为 `runtime://env` 的静态输入（读 features.md 0.5.5 的表格结构），门禁只消费 `runtime://env` 的输出，不反向影响探测范围 | `runtime://env` 的返回结构在 W1 首日冻结 |

### 2.2 关键路径与并行通道汇总

```
FEAT-TEST-P0.01.01 (2.5d, MCP-T-01/02)
  └─► FEAT-TEST-P0.01.02 (2.0d, MCP-T-03)
        └─► FEAT-TEST-P0.02.01 (2.0d, MCP-T-05)
              └─► FEAT-TEST-P0.02.02 (2.0d, MCP-T-06)
                    └─► FEAT-TEST-P0.05.04 (2.5d, GUARD-04/05)
                          └─► FEAT-TEST-P0.04.01 (3.0d, MCP-P-01/03)

CP 总工期 = 14.0 人天（6 个任务）
```

| 通道 | 归属 | 任务 | 任务数 | 工时 |
|---|---|---|---|---|
| **Track A**（平台内核：MCP 通道与执行引擎） | 输入/观测/引擎直驱 | `P0.01.01–03`、`P0.02.01–06`、`P0.03.01–03` | 12 | 22.0 人天 |
| **Track B**（断言与用例库） | 提示模板与视觉审查 | `P0.04.01–02` | 2 | 5.0 人天 |
| **Track C**（CI·报告·基建） | 护栏与报告 | `P0.05.01–06` | 6 | 11.5 人天 |
| **合计** | — | — | **20** | **38.5 人天** |

**跨轨道阻塞校验**：Track B 依赖 Track A 的 `P0.02.01`（`UiFrame` 快照）与 `P0.01.02`（截图），存在硬依赖但 `P0.02.01` 在 CP 上、`P0.04.01` 也在 CP 上，故不构成额外时差。Track C 的 `P0.05.01`（环境隔离）无前置，可 W0 立即启动。

**工期预估**：1 人 14 工作日（严格 CP）；3 人（A/B/C）约 8 工作日。

---

---

## 3. 原子任务卡清单

任务卡沿用项目既有格式（与 `features.md` 5.2 一致，不使用外层代码块包裹，以便文档内锚点与链接可用）。

### FEAT-TEST-P0.01.01 XTEST 输入注入通道（键鼠）

- **基本属性**：
  - 绑定平台能力：`MCP-T-01`、`MCP-T-02`
  - 任务状态：`[x] 已完成`
  - 优先级与复杂度：`P0` | 高 | 预估工时: 2.5 人天
  - 依赖关系：无
  - 关键路径：`CP: 是`（起点）
  - 并行通道：Track A 平台内核
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/input.rs`、`xtask/src/testd/x11.rs`、`crates/ime-ui/src/platform/x11.rs`（读取 `geometry()` 与 `connection_fd()`）
- **MCP 能力映射**：
  - Tools：`action_keyboard`、`action_interact`
  - Resources：`runtime://env`（获取 `DISPLAY` 与窗口几何）
- **功能目标与架构说明**：向真实 `fcitx5` 会话注入高保真键鼠事件。键盘走 **XTEST**（`x11rb::protocol::xtest::fake_input`），使事件经过 X 服务器与 fcitx5 的 `xcb` 前端，产生与真人按键**同路径**的事件流——这是端到端用例可信的前提。指针事件注入到候选框坐标，坐标来自 `X11Backend::geometry()` 与几何计算的 `hit_map`。注入前必须把输入焦点显式设到被测客户端窗口，注入后必须校验焦点未被候选框夺走。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    /// One synthetic input channel bound to a live X11 session.
    pub struct X11Injector { conn: RustConnection, root: Window, scale: f32 }

    impl X11Injector {
        /// Moves the input focus to `window` and asserts the server agrees.
        /// Fails with `TestError::FocusRefused` if `get_input_focus` disagrees.
        pub fn focus(&self, window: Window) -> Result<(), TestError>;

        /// Types one keysym with the given modifier mask, press then release.
        pub fn key(&self, keysym: u32, state: u16) -> Result<(), TestError>;

        /// Types an ASCII string one character at a time, honouring per-key delay.
        pub fn type_text(&self, text: &str, delay: Duration) -> Result<(), TestError>;

        /// Moves the pointer to absolute physical pixels and clicks.
        pub fn click(&self, x: i16, y: i16, button: u8) -> Result<(), TestError>;
        pub fn scroll(&self, x: i16, y: i16, delta: i32) -> Result<(), TestError>;
    }
    ```
  - **坐标换算**：候选框的 `hit_map` 是**窗口内物理像素**（含 `ShadowMargin × scale` 预留区）。注入指针需要**屏幕绝对坐标** = 窗口位置 + 窗口内偏移。窗口位置由 `X11Backend::geometry()` 与 `move_to` 的调用记录给出。换算集中在一个函数内，且必须断言结果落在目标屏幕范围内。
  - **容错机制**：
    - XTEST 扩展缺失（部分嵌套 X 服务器）→ 返回 `TestError::NoXtest`，**不静默降级为 xdotool**，由调用方显式选择兜底通道。
    - 逐键注入的 `delay` 默认 `8ms`（低于 fcitx5 的事件处理节拍会导致丢键）；可用例覆盖。
    - 注入前清修饰键（`--clearmodifiers` 语义）：先查询 `query_keymap`，把按下的修饰键逐个释放。
  - **焦点红线**：每次注入前后调用 `get_input_focus` 并比对；候选框显示期间焦点变化即判定为**最高级别缺陷**（features.md 0.4 规则 5），用例立即失败。
- **逐步落地实施步骤**：
  1. 写 `xtask/src/testd/x11.rs`：X11 连接、XTEST 能力探测、keysym ↔ 字符映射表（覆盖 `a-z`、`0-9`、`-`/`=`/`space`/`Tab`/`Return`/`Escape`/`BackSpace`/方向键/`Shift`/`Ctrl`/`Super`）。
  2. 写 `xtask/src/testd/input.rs` 的 `X11Injector`，实现 `focus` / `key` / `type_text` / `click` / `scroll`。
  3. 写坐标换算与屏幕边界断言；与被测窗口的 `geometry()` 联调。
  4. 写焦点不变断言与清修饰键逻辑。
  5. 在真实 `fcitx5` 会话中做冒烟：向 `xterm`/`gedit` 注入 `nihao`，断言应用收到按键（用 `MCP-T-06` 读取）。
- **验收标准 (DoD)**：
  - [ ] `focus` 后 `get_input_focus` 返回目标窗口；不匹配时返回 `FocusRefused` 而非静默继续。[自动]
  - [ ] 逐键注入 `nihao`（6 键，8ms 间隔）在真实会话中无一丢失（连续 20 次）。[实验室]
  - [ ] 候选框显示期间注入 100 次按键，`get_input_focus` 始终指向客户端窗口。[实验室]
  - [ ] XTEST 缺失的环境返回 `NoXtest` 并给出可操作提示，不崩溃。[自动]
  - [ ] 坐标换算在 scale 1.0 与 2.0 下均落在目标屏幕内（断言无负坐标、无越界）。[自动]

---
- **验收记录**（2026-09-29）：
  - **交付物**：`xtask/src/testd/{x11,input,coords}.rs` 的修正与新增 `xtask/src/testd/input/tests.rs`（约 290 行）。
  - **验证命令与结果**：`just ci` 退出 0；`cargo nextest run -p xtask` 全绿。
  - **本卡最重要的发现**：`xtask/src/main.rs` **从未声明 `mod testd;`**，因此 `testd/{mod,input,keys,x11,coords,engine,sandbox}` 约 2000 行**从未被编译过**。主 Agent 已挂载（`mod testd;` + `Testd` 子命令）并修掉暴露出的错误：`thiserror` 把名为 `source` 的字段当作错误源，故 `SandboxError::DictMismatch` 的 `PathBuf` 字段改名为 `pristine`；`TestError` 补 `PartialEq`；`coords.rs` 的 `i16`/`u16` 比较；`scenario.rs` 的 `KeyAction` 匹配补 ADR-0005 新增的四个变体。
  - **修好的编译错误（x11rb 0.14 API）**：`xtest_get_version` 的 `minor_version` 是 `u16`；`XtestVersion.minor` 随之由 `u8` 改 `u16`；`InputFocus::Parent` → **`InputFocus::PARENT`**（0.14 里是「结构体 + 关联常量」，不是枚举变体）。
  - **焦点红线做成类型系统强制**：注入 API 的每个方法都要求 `&FocusGuard`（没有 guard 就无法注入），且每次注入**前后**各查一次 `get_input_focus` 并比对，不匹配即返回 `FocusStolen` 并中止。注入方**从不调用** `set_input_focus`。判定抽成两个纯函数 `focused()` / `focus_held()`，使焦点红线**无需显示器即可被断言**。
  - **失败后不留改键状态的三重保证**：①每个 stroke 先 `query_keymap` 释放所有物理按下的修饰键；②键码与修饰键码在按下任何键之前全部解析完，`press()` 中途失败会回滚，`release()` 无论主键成败都执行；③`impl Drop for X11Injector` 兜底释放。
  - **已知限制**：
    1. **4 个 `#[ignore]` 实验室用例未在本机执行**（需真 X + XTEST）：`DISPLAY=:0 cargo nextest run -p xtask --run-ignored all`。
    2. **DoD 3 的「候选框显示期间」这一半仍缺口**：候选框尚不存在。
    3. **卡片落地步骤 5 的真 fcitx5 会话冒烟仍缺口**：需要真会话 + 客户端应用 + `FEAT-TEST-P0.02.02` 的文本读取通道；本卡只覆盖注入侧。
    4. `clear_modifiers` 只有逻辑保证，无自动化用例（需要能读回 keysym 的通道）。
    5. **xdotool 兜底通道未实现**（有意为之）：卡片 DoD 无此项；容错实现为 `NoXtest` 硬拒绝 + 可操作提示，不静默降级。
    6. 与卡片骨架的两处有意偏离：`key/type_text/click/scroll` 多一个 `&FocusGuard` 参数、`focus()` 返回 `FocusGuard`；窗口位置由 `translate_coordinates` 向服务器查询而非读 `X11Backend::move_to` 的调用记录（候选框在 fcitx5 进程内，xtask 跨进程拿不到该记录）。**下游卡（P0.01.02 / P0.02.01 / P0.03.01）需按带 guard 的签名调用。**
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2（有 X11/XWayland，无 Wayland 合成器）、Fcitx5 5.1.7、cargo-nextest 0.9.143。

### FEAT-TEST-P0.01.02 X11 截图采集与 DPI 映射

- **基本属性**：
  - 绑定平台能力：`MCP-T-03`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 2.0 人天
  - 依赖关系：`FEAT-TEST-P0.01.01`
  - 关键路径：`CP: 是`
  - 并行通道：Track A 平台内核
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/capture.rs`、`crates/ime-ui/src/platform/x11.rs`（`geometry()` 的 scale）
- **MCP 能力映射**：
  - Tools：`capture_viewport_screenshot`
  - Resources：`runtime://env`、`runtime://style_computed`（→ `runtime://ui_metrics`）
- **功能目标与架构说明**：用 `XGetImage` 抓取候选窗口或全屏，导出 PNG 到 `RUN/<模块代码>/<TC-ID>/`。**DPI 感知**是核心：候选框的几何是逻辑像素，屏幕是物理像素，`scale ∈ {1.0, 1.25, 1.5, 2.0, 3.0}`。截图必须保留**物理像素**分辨率（不做降采样），否则 `1px` 描边与次像素对齐的审查会失真。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    pub struct CaptureRequest {
        pub target: CaptureTarget,   // Window(Window) | FullScreen | Region(RectI)
        pub include_cursor: bool,
        pub out_path: PathBuf,       // RUN/<module>/<TC-ID>/[step]_[state].png
    }
    pub struct CapturedImage { pub width_px: u32, pub height_px: u32, pub scale: f32 }

    /// Captures and writes a PNG. `scale` is recorded in the image metadata so a
    /// later audit can tell physical from logical pixels without guessing.
    pub fn capture(conn: &RustConnection, req: &CaptureRequest) -> Result<CapturedImage, TestError>;
    ```
  - **大尺寸视口流控**：全屏 4K @2x = 7680×4320×4B ≈ 132MB，超过单次 `get_image` 的 `u16` 宽度上限与内存预算。策略：**按行分块抓取**（每块 ≤ 2048 行），拼接后编码 PNG；抓取前先 `get_geometry` 校验尺寸，超限则拒绝并提示改用 `Region`。
  - **容错机制**：
    - 抓取期间窗口被移动/重绘 → 校验抓取前后的 `get_geometry` 一致，不一致则重试一次，仍不一致则报 `TestError::WindowMoved`（避免把错位画面当作基线）。
    - PNG 编码用 `png` crate（新增依赖需走 `[workspace.dependencies]`，并在 `AGENTS.md` 3.5 的基线评审中登记）。
    - 无合成器的 X11 会话下，窗口外的 alpha 不可读（`get_image` 只返回已绘制像素）——**这是已知限制**，必须记入用例的已知限制而非当作缺陷。
  - **DPI 映射**：`CapturedImage.scale` 由 `X11Backend::geometry()` 提供；审查时把 features.md 3.1 的 `dp` 值乘以 `scale` 得到期望物理像素数。
- **逐步落地实施步骤**：
  1. 写 `capture.rs`：`get_geometry` → 分块 `get_image` → 拼接 → PNG 落盘（含 `scale` 元数据）。
  2. 写 `CaptureTarget` 的三种目标与窗口/全屏的坐标换算。
  3. 写尺寸上限校验与分块抓取；写 `WindowMoved` 检测。
  4. 写 `RUN/` 目录创建与命名规范（`[step-seq]_[state-tag].png`）。
  5. 在真实会话中抓取一个已知颜色的测试窗口，逐像素断言颜色正确（端到端验证采集链路无色彩空间转换）。
- **验收标准 (DoD)**：
  - [ ] 抓取一个纯 `#FF0000` 测试窗口，采样像素为 `(255,0,0)`（±1/255）。[自动]
  - [ ] 4K @2x 全屏抓取不失败（分块生效），耗时 ≤ 3s。[性能]
  - [ ] 抓取期间移动窗口 → 返回 `WindowMoved`，不产出错位基线。[自动]
  - [ ] PNG 元数据含 `scale`，且与 `X11Backend::geometry()` 一致。[自动]
  - [ ] 单步快照捕获端到端（含 PNG 编码与落盘）≤ 2s。[性能]

---

### FEAT-TEST-P0.01.03 帧率采样与掉帧检测

- **基本属性**：
  - 绑定平台能力：`MCP-T-04`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 1.5 人天
  - 依赖关系：`FEAT-TEST-P0.01.01`
  - 关键路径：`CP: 否`
  - 并行通道：Track A 平台内核
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/framerate.rs`、`crates/ime-ui/src/platform/x11.rs`（`commit` 调用序列）、`crates/ime-diag/src/probe.rs`（待实现，`TASK-1.08.03`）
- **MCP 能力映射**：
  - Tools：`measure_animation_frame_rate`
  - Resources：`runtime://ui_metrics`、`runtime://memory_profile`
- **功能目标与架构说明**：采样连续交互期间的渲染帧率。**X11 档没有合成器 frame 回调**（`SurfaceBackend::request_frame()` 返回 `None`），因此帧率的真实来源是 `X11Backend::commit()` 的调用序列——由测试用的 `SurfaceBackend` 装饰器记录时间戳，而非猜测刷新率。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    /// Wraps a backend and records every commit's timestamp and damage area.
    pub struct InstrumentedBackend<B: SurfaceBackend> {
        inner: B,
        commits: Vec<CommitRecord>,
        acquire_misses: u32,     // NoFreeBuffer occurrences
    }
    pub struct CommitRecord { pub at: Instant, pub damage_px: u64, pub frame_token: Option<FrameToken> }

    impl InstrumentedBackend<X11Backend> {
        /// Frames per second over a sliding window, plus the dropped-frame count
        /// derived from inter-commit gaps exceeding 1.5x the target period.
        pub fn frame_stats(&self, target_hz: f32) -> FrameStats;
    }
    ```
  - **掉帧判据**：相邻 `commit` 间隔 > `1.5 × (1000 / target_hz) ms` 记为一次掉帧。目标帧率由用例指定（`features.md` 3.3.2：Wayland 由 frame 回调决定；X11 下 UI 线程用 `1000/60ms` 定时器）。
  - **`acquire_misses` 的意义**：`NoFreeBuffer` 计数是 `wl_buffer.release` 饥饿的直接指标（`TASK-1.04.07` 的验收项之一）。X11 档同样记录（SHM 缓冲被占用）。
  - **容错机制**：装饰器只记录，**不改变**被测后端行为；不得为了好数值跳过 `commit` 或合并脏区。
  - **测量纯净度**：与 `GUARD-07` 联动——并行 agent 运行时采样结果无效。
- **逐步落地实施步骤**：
  1. 写 `framerate.rs` 的 `InstrumentedBackend` 与 `FrameStats`。
  2. 写掉帧判据与滑动窗口统计（P50/P95/P99 帧间隔）。
  3. 与被测 UI 线程接线（`TASK-1.05.02` 落地后启用；在此之前用 `MockBackend` 驱动自测）。
  4. 写自测：注入已知的 `commit` 序列，断言统计值正确。
- **验收标准 (DoD)**：
  - [ ] 注入 60 次等间隔 `commit`（16.67ms），`frame_stats` 报告 FPS ≈ 60 且掉帧数 = 0。[自动]
  - [ ] 注入含 3 次 50ms 间隔的序列，掉帧数 = 3。[自动]
  - [ ] `NoFreeBuffer` 被计数且不导致采样中断。[自动]
  - [ ] 装饰器不改变内层后端的任何可观测行为（同一操作序列下 `backend_id()` 与 `geometry()` 一致）。[自动]

---

### FEAT-TEST-P0.02.01 `UiFrame` 快照通道（A11y 树的显式替代）

- **基本属性**：
  - 绑定平台能力：`MCP-T-05`
  - 任务状态：`[x] 已完成`
  - 优先级与复杂度：`P0` | 高 | 预估工时: 2.0 人天
  - 依赖关系：`FEAT-TEST-P0.01.01`
  - 关键路径：`CP: 是`
  - 并行通道：Track A 平台内核
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/uiframe.rs`、`crates/ime-types/src/ui.rs`（`UiFrame` / `UiCommand` / `UiEvent`）、`crates/ime-fcitx5/src/engine.rs`（投递点）
- **MCP 能力映射**：
  - Tools：`get_accessibility_tree` → **替代** `get_ui_frame`
  - Resources：`runtime://style_computed` → **替代** `runtime://ui_metrics`
- **功能目标与架构说明**：候选框为**完全自绘**，**不存在 A11y 树**（`ASM-T-02`）。语义断言的可靠来源是引擎投递给 UI 线程的 `UiFrame` 快照——它是冻结契约，字段齐全且可序列化。本任务把投递路径上的 `UiFrame` 镜像到一个**测试可见的 JSON 落盘点**，使测试可以断言"引擎认为候选框应该显示什么"，与"截图显示实际画了什么"形成对照。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    /// Mirror of the last frame the engine posted, written for tests to read.
    /// Enabled only under the `test-mirror` feature; never active in release.
    pub struct UiFrameMirror {
        path: PathBuf,          // $XDG_RUNTIME_DIR/rspinyin-test/ui_frame.json
        last: Mutex<Option<UiFrame>>,
    }
    impl UiFrameMirror {
        pub fn publish(&self, frame: &UiFrame) -> Result<(), ImeError>;
        /// Reads the newest snapshot; `None` before the first frame.
        pub fn read_latest(path: &Path) -> Result<Option<UiFrame>, TestError>;
    }
    ```
  - **为什么不用 socket**：0.4 规则 6 / `BUDGET-NET-01` 约束的是 IP 网络；但引入 AF_UNIX 控制通道仍会增加生产代码路径与攻击面。**落盘 + 原子重命名**（写 `.tmp` → `rename`）零新增依赖、可离线读取、崩溃后仍可取证。代价是 ~0.2ms 的写盘开销，且**只在 `test-mirror` feature 下启用**，release 构建不含此路径。
  - **序列化**：`UiFrame` 需要 `serde` 派生。`ime-types` 已有 `serde` 可选依赖（`Cargo.toml` 中 `serde = { version = "1", features = ["derive"] }`）。**若 `UiFrame` 尚未派生 `Serialize`，这是对冻结契约的追加**（新增派生不改变字段语义，但按 `AGENTS.md` 第 8 条第 22 项仍需 ADR）→ 本任务负责开 ADR-0007。
  - **容错机制**：
    - 镜像写失败**不得**影响生产路径：`publish` 内部吞掉 IO 错误并计数，绝不向上传播（否则测试设施会拖垮输入法）。
    - 读取侧容忍空文件/半写文件：先读 `.tmp` 是否存在，再读主文件；解析失败重试一次。
  - **与 `revision` 的配合**：快照含 `revision`，用例据此断言"UI 收到的是最新帧"并检测乱序。
- **逐步落地实施步骤**：
  1. 开 `docs/dev/adr/0004-uiframe-test-mirror.md`：记录 `UiFrame` 派生 `Serialize` 与新增 `test-mirror` feature 的理由与影响面。
  2. 在 `ime-types` 为 `UiFrame` 及其嵌套类型（`Preedit`/`Candidate`/`PageState`/`StatusStrip`/`Anchor`/`LayoutHint`）派生 `Serialize`。
  3. 在 `ime-fcitx5` 的 `SendFrame` 效果点接入 `UiFrameMirror::publish`（`#[cfg(feature = "test-mirror")]`）。
  4. 写 `read_latest` 与原子写；写自测（含半写文件、空文件、并发写）。
  5. 与 `MCP-T-03` 联调：同一次按键后同时取 `UiFrame` 快照与截图，断言二者描述同一帧（`revision` 一致、候选数一致）。
- **验收标准 (DoD)**：
  - [ ] `UiFrame` 及其嵌套类型可 `serde_json::to_string` / `from_str` 往返，字段无丢失。[自动]
  - [ ] `test-mirror` 未启用时，release 构建的 `nm -D` 中不含 `publish` 相关符号。[自动]
  - [ ] 镜像写失败（目录只读）时 `publish` 返回 `Ok`，生产路径不受影响。[自动]
  - [ ] 读取半写文件不 panic，重试后成功。[自动]
  - [ ] 同一次按键的 `UiFrame.revision` 与截图时刻的帧一致。[实验室]

---
- **验收记录**（2026-09-29）：
  - **交付物**：新增 `xtask/src/testd/uiframe.rs`、`uiframe/{mirror,schema,error,tests}.rs`（5 个文件，最大 640 行，20 个用例）。
  - **验证命令与结果**：`just ci` 退出 0；`cargo nextest run -p xtask` 全绿。
  - **形态适配（本卡是 0.4 节列名的 A11y 替代）**：自绘窗口无 A11y 树，`FrameSnapshot` 是「引擎认为窗口该画什么」的结构化、可比较视图；文档明确写出「快照不等于屏幕像素，像素归截图通道」，杜绝两通道互相冒充。
  - **不碰冻结契约**：`ime-types` 的 serde 是可选 feature，给 `UiFrame` 派生 `Serialize` 属契约变更（需 ADR）。因此在 xtask 侧逐字段镜像契约类型，转换双向全量无损；**契约新增变体会让转换编译失败，这是刻意设计**。
  - **写侧 revision 规则**：`publish` 绝不覆盖更新的快照；失败**不抬升** floor，故下次 publish 会重试；floor 用 `revision + 1` 编码以 `0` 表示「未写过」（契约里 `0` 是合法 revision，不能当哨兵）。**读侧**复刻契约「丢弃更旧的帧」，区分 `Absent`/`New`/`Repeated`/`Rewound`。
  - **写前校验**：`serde_json` 会把 NaN/Inf 写成 `null`，那会写出一个所有读者都拒收的文件并覆盖掉上一份好快照——因此 `to_json` 先拒绝非有限浮点，且拒绝发生在打开 tmp 之前。
  - **隐私**：帧内容只落盘到沙箱私有文件，不进日志；`describe()` 刻意不转发 `serde_json` 的原始消息（它会引用出错值，也就是用户键入的文本），有专门用例断言。
  - **已知限制**：
    1. **DoD「未启用 `test-mirror` 时 release 的 `nm -D` 不含 `publish` 符号」仍缺口**：需要 `ime-types`/`ime-fcitx5` 的 `test-mirror` feature，属 `crates/**` 与 ADR 范围。xtask 本身不随产品发布，该项只对插件侧成立。
    2. **DoD「同一次按键的 `UiFrame.revision` 与截图时刻的帧一致」仍缺口**：需要插件侧投递点与截图通道 P0.01.02 联调；读侧规则（`FrameWatch`）已就位。
    3. **有意偏离字面**：`publish` 返回 `Publish` 值而不是 `Result`，结构上不存在可被 `?` 传播的错误路径，失败被计数。用例用 ENOTDIR 而非 `chmod 0500` 制造写失败，因为后者在 root 运行的 CI 上不产生 EACCES。
    4. 用例 14 含一个 20ms 线程延迟（重试路径必须由并发写入触发才能验证），预算给 1s。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2（有 X11/XWayland，无 Wayland 合成器）、Fcitx5 5.1.7、cargo-nextest 0.9.143。

### FEAT-TEST-P0.02.02 客户端上屏文本读取

- **基本属性**：
  - 绑定平台能力：`MCP-T-06`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 2.0 人天
  - 依赖关系：`FEAT-TEST-P0.01.01`、`FEAT-TEST-P0.02.01`
  - 关键路径：`CP: 是`
  - 并行通道：Track A 平台内核
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/commit_readback.rs`、`crates/ime-fcitx5/src/engine.rs`（`Effect::Commit` 的执行点）
- **MCP 能力映射**：
  - Tools：`get_accessibility_tree`（作用于**被测客户端应用**，非候选框）、`action_keyboard`
  - Resources：`runtime://logs`
- **功能目标与架构说明**：断言"用户上屏后应用真的收到了正确的文本"。这是核心业务闭环的终点，也是最容易造假的地方——只看 `UiFrame` 或日志不足以证明文本进了应用。**首选实现**：用 `xterm` 作为被测宿主，上屏文本可从终端回显读取（`xterm` 的 `-S` 或截图 OCR 均不可靠）。**选定实现**：起一个**自带回读的最小 GTK/Qt 客户端**（`xtask/testd/client/`），它把 `commit` 回调写到自己 stdout，测试直接读管道。这是唯一能**确定性**验证上屏的路径。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    /// Minimal client used as the commit sink: it prints every commit it receives
    /// to stdout, one JSON object per line, so the harness can assert on it.
    pub struct CommitProbe { child: std::process::Child, stdout: BufReader<ChildStdout> }
    impl CommitProbe {
        pub fn spawn(display: &str) -> Result<Self, TestError>;
        /// Blocks until one commit line arrives or the deadline passes.
        pub fn next_commit(&mut self, timeout: Duration) -> Result<String, TestError>;
        pub fn preedit(&mut self) -> Result<Option<String>, TestError>;
        pub fn close(self) -> Result<(), TestError>;
    }
    ```
  - **客户端的输入上下文必须声明 `CapabilityFlag`**：被测客户端要显式声明 `Preedit` / `ClientSideInputPanel` 等能力，才能覆盖 features.md 3.5 的 `client_preedit` 两种配置。客户端需支持通过参数切换能力集。
  - **为什么不 OCR 截图**：OCR 依赖字体渲染与抗锯齿，无法区分"上屏正确"与"候选框恰好显示相同文字"，且对 CJK 的识别率不可控。**明确排除 OCR 作为上屏断言手段**。
  - **容错机制**：
    - 客户端启动超时（等待窗口映射）→ 重试一次，仍失败报 `TestError::ClientSpawn`。
    - 管道读取超时 → 返回 `TestError::CommitTimeout` 并把已收到的行附在错误里（便于归因"没上屏"还是"上屏了别的字"）。
    - 客户端崩溃 → 捕获退出码与 stderr，归档到 `trace.json`。
  - **preedit 回读**：同一客户端在 `client_preedit = true` 时把 `set_preedit` 的文本也写 stdout，用于 `tests/rt.md` 的 preedit 用例。
- **逐步落地实施步骤**：
  1. 写 `xtask/testd/client/`：一个最小 GTK4 或 Qt6 客户端（选依赖更轻的一方；若两者都过重，用 **X11 + XIM** 客户端——但 XIM 路径与 Wayland 无关，需在用例中标注）。
  2. 实现 `commit` / `set_preedit` 回调 → stdout JSON 行。
  3. 写 `CommitProbe` 的 spawn/next_commit/close 与超时、崩溃处理。
  4. 与 `MCP-T-01` 联调：注入 `nihao` + 空格，断言收到 `你好`。
  5. 写能力集切换参数（`--caps=preedit,panel`）与对应自测。
- **验收标准 (DoD)**：
  - [ ] 注入 `nihao` 后按空格，`next_commit` 收到 `你好`（连续 20 次无失败）。[实验室]
  - [ ] 上屏失败时返回 `CommitTimeout` 并附带已收到的全部行。[自动]
  - [ ] 客户端崩溃时归档退出码与 stderr 到 `trace.json`。[自动]
  - [ ] `client_preedit = true` 配置下可读到 preedit 文本；`false` 时读到空。[实验室]
  - [ ] 连续 200 次上屏，管道无丢失、无乱序。[自动]

---

### FEAT-TEST-P0.02.03 `runtime://logs` 日志与审计输出抓取

- **基本属性**：
  - 绑定平台能力：`MCP-R-01`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 低 | 预估工时: 1.5 人天
  - 依赖关系：无（只读文件与进程输出）
  - 关键路径：`CP: 否`
  - 并行通道：Track A 平台内核
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/logs.rs`、`$XDG_DATA_HOME/rspinyin/logs/rspinyin.log`、`crates/ime-diag/src/log.rs`（待实现，`TASK-1.08.01`）
- **MCP 能力映射**：
  - Resources：`runtime://logs`
- **功能目标与架构说明**：抓取三路输出供断言与取证：(a) `ime-diag` 的结构化日志（`IN_PROGRESS`，尚未落盘 → 用例标注 `[待实现]`）；(b) `fcitx5` 自身的 stdout/stderr（addon 加载、崩溃信号）；(c) 五个审计脚本的输出。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    pub struct LogTap { path: PathBuf, offset: u64 }
    impl LogTap {
        /// Returns lines appended since the previous call; never re-reads history.
        pub fn drain(&mut self) -> Result<Vec<String>, TestError>;
        /// Asserts no line matches a forbidden pattern (privacy denylist).
        pub fn assert_absent(&self, patterns: &[&str]) -> Result<(), TestError>;
        /// Asserts every emitted error code is in the frozen set.
        pub fn assert_error_codes_known(&self, known: &[&str]) -> Result<(), TestError>;
    }
    ```
  - **隐私断言（核心价值）**：`assert_absent` 用 features.md 的脱敏规则做反向断言——日志中**不得出现**用户输入内容、候选文本、明文应用标识、明文家目录路径。这是 `TASK-1.06.02` 零痕迹断言的自动化载体。**注意**：`ime-diag` 未实现，本断言当前只能对 `fcitx5` 自身日志生效，用例必须标注 `[待实现: TASK-1.08.01]`。
  - **容错机制**：日志文件被滚动（`tracing-appender` 按大小切分）→ `LogTap` 检测 inode 变化后切换到新文件，并把已轮转的历史文件一并纳入 `assert_absent` 的扫描范围（否则滚出去的内容会逃过隐私断言）。
  - **错误码白名单**：从 `crates/ime-types/src/error.rs` 的 `#[error("...")]` 字符串**提取**而非手抄，避免文档漂移。
- **逐步落地实施步骤**：
  1. 写 `logs.rs` 的 `LogTap`（含 inode 变化检测与轮转文件追踪）。
  2. 写 `assert_absent` / `assert_error_codes_known`。
  3. 写错误码白名单提取器（解析 `error.rs`）。
  4. 写 `fcitx5` 子进程 stdout/stderr 的捕获与归档。
  5. 与 `GUARD-04` 联调：失败用例把日志片段写入 `trace.json`。
- **验收标准 (DoD)**：
  - [ ] `drain` 只返回增量，连续调用不重复返回历史行。[自动]
  - [ ] 日志被轮转后，`assert_absent` 仍能覆盖已轮转的历史文件。[自动]
  - [ ] 错误码白名单从 `error.rs` 提取，与源码逐条一致（无手抄漂移）。[自动]
  - [ ] 注入一条含明文应用名的日志，`assert_absent` 失败并指出该行。[自动]

---

### FEAT-TEST-P0.02.04 `runtime://ui_metrics` 度量通道（Computed Style 的显式替代）

- **基本属性**：
  - 绑定平台能力：`MCP-R-02`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 1.5 人天
  - 依赖关系：`FEAT-TEST-P0.02.01`
  - 关键路径：`CP: 否`
  - 并行通道：Track A 平台内核
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/ui_metrics.rs`、`crates/ime-ui/src/platform/x11.rs`（`geometry()` / `effective_base_alpha()` / `diagnostics()`）、`crates/ime-types/src/ui.rs`
- **MCP 能力映射**：
  - Resources：`runtime://style_computed` → **替代** `runtime://ui_metrics`
- **功能目标与架构说明**：**没有 DOM、没有 CSS**，因此"计算样式"不存在。替代方案是三源合成：(a) `X11Backend` 的运行期度量（`geometry()` 的 `(w, h, scale)`、`effective_base_alpha()`、`X11Diagnostics`）；(b) `UiFrame` 的 `layout`/`theme` 字段；(c) features.md 3.1/3.2 的**数值规范表**与 `TASK-1.05.03` 定义的 `.slint` `public constant`。三者交叉校验才能发现"代码写错了常量"或"主题降级未生效"。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    pub struct UiMetrics {
        pub window_px: (u32, u32),        // physical, from X11Backend::geometry()
        pub scale: f32,
        pub base_alpha: u8,               // effective, after compositor fallback
        pub compositor_present: bool,     // X11Diagnostics
        pub argb_visual: bool,
        pub backend_id: &'static str,     // "x11" | "wlr-layer-shell" | ...
        pub layout: LayoutHint,
        pub theme: ThemeSpec,
    }
    /// Compares a spec table parsed from features.md 3.1 against the live metrics
    /// and the .slint public constants, reporting every mismatch.
    pub fn cross_check_spec(spec: &SpecTable, live: &UiMetrics, slint: &SlintConstants)
        -> Vec<MetricMismatch>;
    ```
  - **三源交叉校验**（本任务的独有价值）：
    | 源 | 内容 | 检出什么 |
    |---|---|---|
    | features.md 3.1/3.2 表格 | `ContainerRadius = 12dp`、`CellHeight = 36dp`、`surface.base = #1C1C1E @0.85` … | 规范本身被误改 |
    | `.slint` `public constant`（`TASK-1.05.03`） | `ShadowMargin`、`ContainerRadius`、`CellHeight` … | 代码与规范漂移 |
    | 运行期 `X11Backend` 度量 | 实际窗口尺寸、`effective_base_alpha`、合成器能力 | 主题降级未生效、几何算错 |
  - **当前可执行性**：`.slint` 常量不存在（1.05.03 `PENDING`），故**第三源不可用**；本任务须支持"源缺失"的显式降级（返回 `MetricSource::Unavailable` 而非空结果），使用例能如实标注。
  - **容错机制**：规范表解析失败（Markdown 表格结构变化）→ 报 `TestError::SpecUnparsable` 并打印期望的表格形状，**不静默返回空表**（空表会让所有断言假通过）。
- **逐步落地实施步骤**：
  1. 写 `ui_metrics.rs`：从 features.md 3.1/3.2 解析 `SpecTable`（列名锚定，结构变化即失败）。
  2. 写 `.slint` 常量的提取器（解析 `public constant <Name>: <value>`），源缺失时返回 `Unavailable`。
  3. 写 `cross_check_spec` 的三源比对与 `MetricMismatch` 报告。
  4. 接入 `X11Diagnostics` 与 `effective_base_alpha`。
  5. 写自测：注入一份与规范不一致的假 `.slint` 常量表，断言 `cross_check_spec` 报告该差异。
- **验收标准 (DoD)**：
  - [ ] `SpecTable` 从 features.md 3.1 解析出全部尺寸项；表格结构被破坏时返回 `SpecUnparsable`。[自动]
  - [ ] 注入错误的 `.slint` 常量 → `cross_check_spec` 精确报告项名、规范值、代码值。[自动]
  - [ ] `.slint` 源缺失时返回 `Unavailable` 而非空表（断言不假通过）。[自动]
  - [ ] `effective_base_alpha` 在无合成器环境下返回 `255`，与 `X11Diagnostics` 一致。[自动]

---

### FEAT-TEST-P0.02.05 内存与资源采样

- **基本属性**：
  - 绑定平台能力：`MCP-R-03`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 1.5 人天
  - 依赖关系：无
  - 关键路径：`CP: 否`
  - 并行通道：Track A 平台内核
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/memory.rs`、`docs/dev/budgets.json`（`memory_mb.*` / `robustness.rss_drift_mb`）
- **MCP 能力映射**：
  - Resources：`runtime://memory_profile`
- **功能目标与架构说明**：采样 `fcitx5` 进程的 RSS 与 smaps 明细，支撑三类断言：`BUDGET-MEM-01`（UI ≤ 18MB）、`BUDGET-MEM-02`（插件 ≤ 45MB）、`BUDGET-MEM-03`（词库 mmap 匿名驻留 ≤ 25MB），以及长稳的 RSS 漂移 ≤ 2MB。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    pub struct MemorySample {
        pub vm_rss_kb: u64,
        pub anonymous_kb: u64,        // smaps_rollup Anonymous
        pub private_dirty_kb: u64,    // smaps_rollup Private_Dirty
        pub at: Instant,
    }
    pub struct MemoryMonitor { pid: i32, baseline: Option<MemorySample>, samples: Vec<MemorySample> }
    impl MemoryMonitor {
        /// Samples /proc/<pid>/status (VmRSS) and /proc/<pid>/smaps_rollup.
        pub fn sample(&mut self) -> Result<MemorySample, TestError>;
        /// RSS drift across the window; the soak assertion uses this.
        pub fn drift_kb(&self) -> Option<i64>;
        /// Delta against the baseline taken before the measured interaction.
        pub fn delta_from_baseline_kb(&self) -> Option<i64>;
    }
    ```
  - **口径纪律**：`BUDGET-MEM-03` 明确要求看 **`Anonymous` 与 `Private_Dirty`**，不是 `VmRSS`——因为词库 mmap 的页缓存会进 RSS 但可回收，用 RSS 判定会误报。**两条口径必须分开断言**，用例不得混用。
  - **基线必须差分**：`BUDGET-MEM-01` 是"UI 渲染层**增量** ≤ 18MB"，所以必须在候选框创建前后各采一次，用 `delta_from_baseline_kb`。**绝对值断言是错的**（会把 fcitx5 自身的内存算进来）。
  - **容错机制**：`/proc` 读取失败（进程已退出、权限不足）→ `TestError::ProcUnreadable` 并附 errno；`smaps_rollup` 不可用（老内核）→ 退化为 `VmRSS` 并**在报告中标明口径退化**（不静默）。
  - **采样频率**：长稳用例每 30s 一次，共 960 次（8 小时）；采样本身的开销可忽略。
- **逐步落地实施步骤**：
  1. 写 `memory.rs` 的 `MemorySample` / `MemoryMonitor`（`status` + `smaps_rollup` 解析）。
  2. 写 `drift_kb` / `delta_from_baseline_kb` 与口径退化的显式标记。
  3. 与 `docs/dev/budgets.json` 的阈值接线（**不硬编码**阈值）。
  4. 写自测：用当前测试进程自采，断言 `vm_rss_kb > 0` 且解析无 panic。
- **验收标准 (DoD)**：
  - [ ] `sample` 对自身进程返回非零 `vm_rss_kb` 且 `anonymous_kb ≤ vm_rss_kb`。[自动]
  - [ ] `smaps_rollup` 缺失时退化为 `VmRSS` 并在报告中标明口径。[自动]
  - [ ] `delta_from_baseline_kb` 在基线为 `None` 时返回 `None` 而非 `0`（防止假通过）。[自动]
  - [ ] 阈值全部来自 `budgets.json`，源码中无第二份硬编码（脚本化 grep 断言）。[自动]

---

### FEAT-TEST-P0.02.06 `runtime://env` 环境能力探测

- **基本属性**：
  - 绑定平台能力：`MCP-R-05`
  - 任务状态：`[x] 已完成`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 1.5 人天
  - 依赖关系：无
  - 关键路径：`CP: 否`
  - 并行通道：Track A 平台内核
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/env.rs`、`crates/ime-ui/src/platform/x11.rs`（`X11Diagnostics`）、`docs/dev/features.md` 0.5.5、`docs/dev/adr/0003-ui-role-separate-addon.md`
- **MCP 能力映射**：
  - Resources：`runtime://env`
- **功能目标与架构说明**：探测并发布运行环境能力，作为**用例可执行性判定**与**档位相关断言**的唯一输入。返回结构在 W1 首日冻结（见 2.1 的解耦约定），`GUARD-06` 只消费不反向影响。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    pub struct EnvCapabilities {
        pub fcitx5_version: Option<String>,     // e.g. "5.1.7"
        pub dev_packages: bool,                 // pkg-config Fcitx5Core
        pub addons_loaded: Vec<AddonState>,     // name -> loaded/failed
        pub display_server: DisplayServer,      // X11 | Wayland | Headless
        pub compositor: Option<String>,         // e.g. "weston", "kwin", "sway"
        pub tier: WaylandTier,                  // T1..T4 | NotApplicable
        pub wlr_layer_shell: bool,
        pub argb_visual: bool,
        pub compositor_present: bool,
        pub cjk_font_count: u32,
        pub writable_data_dir: bool,
        pub concurrent_agents: u32,             // for GUARD-07
    }
    pub struct AddonState { pub name: String, pub loaded: bool, pub category: String }
    ```
  - **档位判定的真实依据**：`tier` 不是猜的——按 features.md 2.5.2 的顺序探测 `wl_registry` 全局对象（`zwlr_layer_shell_v1` 是否存在）、`xdg_wm_base` 的 `configure` 行为，并把结论与 features.md 0.5.5 的本机基线对照。**本机预期为 `NotApplicable`**（Weston，四档之外）。
  - **`concurrent_agents`（供 `GUARD-07`）**：统计同名构建/测试进程数（`/proc` 扫描 `cargo`/`nextest` 的并发实例），用于判定性能采样是否受污染（`ASM-T-11` 的实测教训：并行 agent 下 `passthrough/classify` 511ns vs 空闲 726ns，criterion 报告了假回归）。
  - **双 addon 校验**（ADR-0003 的落地断言）：`addons_loaded` 必须能区分 `librspinyin.so`（`Category=InputMethod`）与 `librspinyin-ui.so`（`Category=UI`）两个 addon；**若只加载了一个，本任务必须报告缺失而非静默**——这是 ADR-0003 后果 #2（打包需两个 conf）的可观测点。
  - **容错机制**：`fcitx5` 未安装 → 返回 `fcitx5_version: None` 而非失败（使纯 Rust 用例仍可执行）。
- **逐步落地实施步骤**：
  1. 写 `env.rs` 的 `EnvCapabilities` 与各探测函数。
  2. 写 fcitx5 版本与 addon 加载状态的探测（`fcitx5-diagnose` 输出解析 + `pkg-config`）。
  3. 写显示服务器/合成器/档位探测。
  4. 写 `concurrent_agents` 统计。
  5. 与 `GUARD-06` 联调：把 `EnvCapabilities` 映射为 `tests.md` 各用例的可执行性判定。
- **验收标准 (DoD)**：
  - [ ] 本机探测结果与 features.md 0.5.5 的登记基线一致（`fcitx5_version = "5.1.7"`、`wlr_layer_shell = false`、`tier = NotApplicable`、`cjk_font_count ≥ 90`）。[自动]
  - [ ] 双 addon 状态可分别读出；仅加载一个时报告缺失。[实验室]
  - [ ] `fcitx5` 未安装时返回 `None` 而非失败。[自动]
  - [ ] `concurrent_agents > 0` 时性能用例被 `GUARD-07` 拒绝采信。[自动]

---
- **验收记录**（2026-09-29）：
  - **交付物**：新增 `xtask/src/testd/env.rs` 与 `env/{observation,compositor,process,addons}.rs` 及 5 个测试文件（10 个文件共 2294 行，最大 495 行，60 个用例）。
  - **验证命令与结果**：`just ci` 退出 0；`cargo nextest run -p xtask` 全绿。
  - **探测与解释分离**：`Observation`（原始读数）→ `EnvCapabilities::from_observation`（纯函数）。所有判定都能在无显示服务器、无合成器、无 fcitx5、无进程表的条件下断言。
  - **「读不到」与「否」严格区分**：读不到的读数一律记入 `gaps`（8 个稳定码 + `needs`，如 `env/wayland/registry-unread`、`env/proc/unreadable`），字段本身取证据支持的值；`missing_addons()` 在无会话日志时返回空而不是把「未知」报成「缺失」。
  - **双 addon**：`rspinyin`(InputMethod) 与 `rspinyin-ui`(UI) 分别读出，类别来自已安装 descriptor（读不到则回落并记 gap），装载结论只来自会话日志（`fcitx5-diagnose` 不打印逐 addon 结论，`sandbox/session.rs` 已实测记录）。
  - **`concurrent_agents` 排除自身祖先链**：否则在 nextest 下永远 >0；`0` 才表示「这台机器上没有别人在构建」。
  - **已知限制**：
    1. **DoD「`tier` 由真实 `wl_registry` 判定」仍缺口**：工作区完全没有 `wayland-client` 依赖（`ime-ui` 的 Wayland 后端也还停在 `ProtocolClient` seam 上）。按 0.4 的形态替代原则显式降级为「合成器表推断 + `env/wayland/registry-unread` gap」，不静默。`RegistryFacts` 就是为此预留的接口。
    2. **`#[ignore]` 的 0.5.5 基线断言未在登记机上跑过**（`fcitx5_version="5.1.7"`、`wlr_layer_shell=false`、`tier=NotApplicable`、`cjk_font_count≥90`）。
    3. `compositor == Some("weston")` 由 `/proc` 进程名推断；WSLg 里该进程的 `comm` 是否恰为 `weston` 未验证。
    4. `argb_visual` / `compositor_present` 在本机的真实取值未知（0.5.5 只登记「WSLg 不提供 X11 合成器」）。
    5. 卡片文字里的 `librspinyin-ui.so` 是连字符写法，与 `features.md` 0.7 及 ADR-0004 的 `librspinyin_ui.so` 不一致；实现按后者为准，且 `AddonState` 只报 addon 名（`rspinyin` / `rspinyin-ui`），库名不进报告。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2（有 X11/XWayland，无 Wayland 合成器）、Fcitx5 5.1.7、cargo-nextest 0.9.143。

### FEAT-TEST-P0.03.01 引擎直驱通道（不经 fcitx5）

- **基本属性**：
  - 绑定平台能力：`MCP-T-07`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 2.5 人天
  - 依赖关系：无
  - 关键路径：`CP: 否`
  - 并行通道：Track A 平台内核
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/engine.rs`、`crates/ime-core/src/{segment,input,viterbi,lm,passthrough}/`、`crates/ime-types/src/{decode,lexicon}.rs`
- **MCP 能力映射**：
  - Tools：`action_engine`
  - Resources：`runtime://budget`
- **功能目标与架构说明**：**这是本平台最有价值、且今天就能全量执行的一层**。`ime-core` 被设计为纯函数（0.4 规则 4：不碰文件系统、时钟、环境变量），全部数据源经 `trait Lexicon`/`UserFreqSource`/`LanguageModel` 注入。因此可以在**无显示服务器、无 fcitx5、无词库文件**的条件下，用内存替身驱动完整解码链路并做确定性断言。401 个现有 `#[test]` 中的绝大多数属于这一层。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    /// In-memory doubles over the frozen traits. Deterministic by construction:
    /// no clock, no filesystem, no randomness.
    pub struct MockLexicon { entries: BTreeMap<String, Vec<OwnedWord>> }
    pub struct MockUserFreq { map: BTreeMap<String, u32> }
    pub struct MockLm { uni: BTreeMap<String, i32>, bi: BTreeMap<(String, String), i32> }

    /// One scripted scenario: a keystroke sequence plus the expected observable
    /// outcome at each step. Kept declarative so cases live in data, not in code.
    pub struct Scenario { pub steps: Vec<Step> }
    pub struct Step { pub action: KeyAction, pub expect: Expectation }
    pub enum Expectation {
        Candidates { first: String, count: usize, contains: Vec<String> },
        Preedit { text: String, caret: u32, spans: usize },
        Degraded { code: &'static str },     // e.g. "decode/too-long"
        Page { current: u8, total: u8 },
    }
    /// Runs a scenario against the engine and returns the first divergence.
    pub fn run_scenario(s: &Scenario, lx: &dyn Lexicon, uf: &dyn UserFreqSource,
                        lm: &dyn LanguageModel) -> Result<(), Divergence>;
    ```
  - **确定性纪律**：同一输入 + 同一替身 + 同一配置 ⇒ 逐字节一致的候选序列。所有打分走 Q8.8 定点（`crates/ime-core/src/lm/score.rs`），候选排序**不得**由 `f32` 驱动（`AGENTS.md` 3.8）。本任务须提供**重复运行断言**（同一场景跑 100 次，结果全等）——这是拦截"浮点漂移导致候选顺序抖动"的唯一手段。
  - **场景数据化**：把用例步骤与期望写成 `tests/fixtures/scenarios/*.toml`（数据而非代码），使 `tests.md` 的用例能直接引用文件名，也让非 Rust 背景的评审者能读懂。
  - **容错机制**：替身查不到词时**必须**返回空而非 panic；`MockLexicon` 要能模拟 `Err(ImeError::...)` 分支以覆盖降级路径。
  - **边界覆盖**：`raw` 长度上限 64（`ASM-T-10`）、空输入（`decode/empty-input`）、无路径（`decode/no-path`）、非法字符（`decode/invalid-char`）、超长（`decode/too-long`）——五个错误码都必须有场景。
- **逐步落地实施步骤**：
  1. 写 `engine.rs` 的三个替身与 `Scenario`/`Step`/`Expectation` 类型。
  2. 写 `run_scenario` 与 `Divergence`（含"第几步、期望什么、实际什么"的可读 diff）。
  3. 写 TOML 场景加载器与 `tests/fixtures/scenarios/` 的骨架（每模块至少 1 个场景，后续由 `tests.md` 的用例填充）。
  4. 写重复运行断言（100 次全等）。
  5. 与现有 401 个单测对照，把可数据化的用例迁移为场景（**不删原测试**，两者互补）。
- **验收标准 (DoD)**：
  - [ ] `run_scenario` 对 5 个已冻结错误码各有一条场景通过。[自动]
  - [ ] 同一场景重复 100 次，候选序列逐字节一致。[自动]
  - [ ] 场景失败时 `Divergence` 指出步骤序号、期望值、实际值。[自动]
  - [ ] 引擎直驱不触碰文件系统与时钟（用 `strace -f -e trace=openat,clock_gettime` 断言无相关系统调用）。[自动]
  - [ ] 在无 `DISPLAY`、无 `WAYLAND_DISPLAY` 的环境下全量场景通过。[自动]

---

### FEAT-TEST-P0.03.02 词库注入与畸形输入构造

- **基本属性**：
  - 绑定平台能力：`MCP-T-08`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 高 | 预估工时: 2.0 人天
  - 依赖关系：`FEAT-TEST-P0.03.01`
  - 关键路径：`CP: 否`
  - 并行通道：Track A 平台内核
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/dict_inject.rs`、`crates/ime-dict/src/{format,fst_index,mmap,user_db}.rs`、`xtask/src/dictc/`、`data/compiled/base.dict`
- **MCP 能力映射**：
  - Tools：`action_dict`
  - Resources：`runtime://logs`
- **功能目标与架构说明**：词库与用户数据是**不可信输入**（0.4 规则 8 / `AGENTS.md` 3.9）：在 mmap 之前必须校验 magic、格式版本、长度字段与 CRC。本任务提供受控的畸形构造能力，使"越界偏移必须返回 `DictError` 而非越界访问"这条红线可被**自动验证**，而不是靠代码审查。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    /// Byte-level mutations over a real base.dict, applied to a scratch copy.
    pub enum DictMutation {
        Magic,                       // corrupt the 4-byte magic
        FormatVersion(u16),          // write a future version
        Truncate { at: usize },      // cut the file short
        SectionCrc { kind: u32 },    // flip one byte inside a section
        EntryLength { index: u32, len: u16 },  // word_len out of range
        EntryOffset { index: u32, off: u32 },  // word_off past the string pool
        WordlistRange { key: &str }, // wordlist_start + count past the end
        EmptyFile,
    }
    pub struct DictFixture { path: PathBuf }   // scratch copy under RUN/
    impl DictFixture {
        pub fn from_compiled(src: &Path, dest: &Path) -> Result<Self, TestError>;
        pub fn mutate(&mut self, m: DictMutation) -> Result<(), TestError>;
        /// Every mutation must produce a typed error, never a panic or a SIGSEGV.
        pub fn assert_rejected(&self, expect: DictErrorKind) -> Result<(), TestError>;
    }
    ```
  - **`SIGBUS` 的显式覆盖**：`mmap` 的文件被 `truncate` 后访问越界页会触发 `SIGBUS`（不可用 `catch_unwind` 捕获）。`Truncate` 变异必须配合 `TASK-1.08.02` 的信号处理断言——**该任务 `PENDING`，故此项用例标注 `[待实现: TASK-1.08.02]`**，在此之前只能断言"加载期检测到长度不符"。
  - **FST 值的位解包边界**：`(wordlist_start << 24) | count` 的 `wordlist_start ≤ 2^40`、`count ≤ 2^24`。`WordlistRange` 变异直接构造越界的 value，断言 `entry_to_ref` 返回 `DictError::LengthOutOfRange`。
  - **用户库变异**：`user.redb` 的损坏（0 字节、随机字节）必须走 `TASK-1.03.05` 的自愈路径（隔离为 `user.redb.corrupt.<ts>` 并新建空库）。**该任务 `PENDING`，用例标注 `[待实现: TASK-1.03.05]`**。
  - **容错机制**：所有变异在**临时副本**上进行（`RUN/<module>/<TC-ID>/` 下），绝不触碰 `data/compiled/base.dict`；`DictFixture` 的 `Drop` 清理副本。
- **逐步落地实施步骤**：
  1. 写 `dict_inject.rs` 的 `DictMutation` 八种变异与 `DictFixture`。
  2. 写 `assert_rejected` 与 `DictErrorKind` 映射（从 `crates/ime-types/src/error.rs` 的 `DictError` 变体提取）。
  3. 写临时副本管理与 `Drop` 清理。
  4. 写八种变异 × `FstLexicon::load` / `entry_to_ref` 的断言矩阵。
  5. 写 `data/compiled/base.dict` 的完整性校验（断言测试前后原文件 `sha256` 不变）。
- **验收标准 (DoD)**：
  - [ ] 8 种变异全部被拒绝，返回对应的 `DictError` 变体，无 panic、无越界。[自动]
  - [ ] `Truncate` 变异在 `TASK-1.08.02` 落地前标注 `[待实现]`，落地后断言 `SIGBUS` handler 以退出码 70 结束并留下崩溃文件。[自动]
  - [ ] `data/compiled/base.dict` 在全部测试后 `sha256` 不变。[自动]
  - [ ] `WordlistRange` 变异触发 `entry_to_ref` 的越界返回而非读取。[自动]
  - [ ] 变异在临时副本上进行，`data/compiled/` 下无残留文件。[自动]

---

### FEAT-TEST-P0.03.03 预算比对与回归门禁

- **基本属性**：
  - 绑定平台能力：`MCP-R-04`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 1.5 人天
  - 依赖关系：`FEAT-TEST-P0.03.01`
  - 关键路径：`CP: 否`
  - 并行通道：Track A 平台内核
  - 代码落地锚点 (Code Anchor)：`xtask/src/budget.rs`（**已存在**，`xtask/src/budget/{schema,spec,tests}.rs`）、`docs/dev/budgets.json`、`crates/ime-core/benches/{input,passthrough}.rs`、`crates/ime-dict/benches/{dict,userdb}.rs`
- **MCP 能力映射**：
  - Resources：`runtime://budget`
- **功能目标与架构说明**：`xtask budget --validate` 已能校验 `budgets.json` 与 features.md 0.5.3 表格的一致性。本任务把它扩展为**回归门禁**：把 criterion 与探针的实测值与该表比对，超限即失败。这样 `ASM-T-04`/`ASM-T-05`/`ASM-T-06` 的每个阈值都成为 CI 可判定的断言（0.4 规则 9）。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    /// One measured value tied to its budget key, e.g. ("decode_p99", 2.4, Millis).
    pub struct Measurement { pub key: &'static str, pub value: f64, pub unit: Unit }
    pub enum Verdict { Pass, Fail { budget: f64, measured: f64, ratio: f64 }, Missing }
    /// Compares measurements against budgets.json. A key with no measurement is
    /// `Missing`, never `Pass` -- an unmeasured budget is an unmet budget.
    pub fn compare(ms: &[Measurement], budgets: &Budgets) -> Vec<(&'static str, Verdict)>;
    /// Parses criterion's target/criterion/<bench>/<case>/new/estimates.json.
    pub fn read_criterion(target_dir: &Path) -> Result<Vec<Measurement>, TestError>;
    ```
  - **`Missing ≠ Pass`** 是本任务的核心纪律。现有 `just bench-quick` 在无 criterion 目标时会打印 "nothing to assert" 并**成功退出**——这正是"未测量的预算被当作已满足"的漏洞。本任务必须让 `check-budget` 在缺测量时报 `Missing` 并失败（或在明确的白名单下显式豁免并打印）。
  - **测量口径**：criterion 输出的是 `mean`/`std_dev`，预算表用的是 P99。比对规则必须写明：以 `mean + 3σ` 作为 P99 估计（`features.md` 5.2 的 TASK-1.02.07 已如此定义），并在报告中标注这是**估计值**。
  - **测量纯净度**：与 `GUARD-07` 联动——`concurrent_agents > 0` 时拒绝采信并提示重测（`ASM-T-11` 的实测教训）。
  - **容错机制**：criterion 目录缺失 → `Missing`；`estimates.json` 结构变化 → `TestError::CriterionFormat` 并打印实际结构。
- **逐步落地实施步骤**：
  1. 写 `Measurement` / `Verdict` / `compare`，读 `budgets.json` 的全部键。
  2. 写 `read_criterion` 解析 `estimates.json`（`mean`/`std_dev`）。
  3. 把 `mean + 3σ` 的 P99 估计与口径标注写进报告。
  4. 改 `justfile` 的 `bench-quick`：缺测量时失败而非成功退出（保留显式白名单）。
  5. 写自测：注入一份超限的假 `estimates.json`，断言 `compare` 报 `Fail` 且 ratio 正确。
- **验收标准 (DoD)**：
  - [ ] `compare` 对 `budgets.json` 的每个键返回 `Pass` 或 `Fail`；无测量时返回 `Missing`。[自动]
  - [ ] 注入超限值 → 报 `Fail` 并打印 `budget`、`measured`、`ratio`。[自动]
  - [ ] `bench-quick` 在无 criterion 目标时**失败**（不再是成功退出），除非显式白名单。[自动]
  - [ ] 报告标注 P99 为 `mean + 3σ` 估计值。[文档]
  - [ ] `concurrent_agents > 0` 时拒绝采信并提示重测。[自动]

---

### FEAT-TEST-P0.04.01 商业化视觉审查提示模板

- **基本属性**：
  - 绑定平台能力：`MCP-P-01`、`MCP-P-03`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 高 | 预估工时: 3.0 人天
  - 依赖关系：`FEAT-TEST-P0.01.02`、`FEAT-TEST-P0.02.01`、`FEAT-TEST-P0.02.04`
  - 关键路径：`CP: 是`（终点）
  - 并行通道：Track B 断言与用例库
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/prompts/visual_audit.md`、`xtask/src/testd/prompts/spec_audit.md`、`docs/dev/features.md` 第 3 节、`docs/dev/tests/ui.md`
- **MCP 能力映射**：
  - Prompts：`prompt://commercial_grade_visual_audit`、`prompt://ime_spec_audit`
  - Resources：`runtime://ui_metrics`、`runtime://logs`
- **功能目标与架构说明**：把 features.md 第 3 节的**数值规范**（3.1 尺寸表、3.2 颜色 Token、3.3 动效参数、3.4 五态表）转成多模态审查提示，使 AI 对截图的评审**有据可依**，而不是凭"感觉好看"。`prompt://ime_spec_audit` 则把 0.5.2 的能力矩阵逐行转成可判定断言，确保能力矩阵的每一行都有用例承接。
- **实现细节与防御策略**：
  - 提示模板骨架（`visual_audit.md`）：
    ```markdown
    # 视觉审查：候选框
    ## 输入
    - 截图：{screenshot}  （物理像素，scale = {scale}）
    - 期望度量：{ui_metrics}   （来自 runtime://ui_metrics）
    - 规范来源：docs/dev/features.md 3.1 / 3.2 / 3.4
    ## 判定项（逐条给出 通过/不通过/无法判定 + 像素证据）
    1. 容器圆角 = 12dp × scale 物理像素；圆角外 alpha = 0
    2. 容器描边 1dp，颜色 = surface.stroke（暗色 rgba(255,255,255,0.10)）
    3. 阴影双层：内层 0/1/2 + 外层 0/8/28；最外层 32dp 预留区完全透明
    4. Header 高 34dp、候选单元高 36dp、网格间距 6dp —— 均为 4dp 整数倍
    5. 全部间距为 4dp 整数倍（例外：1px 描边、6dp 光标箭头）
    6. 首选项背景 = accent @0.18 + 描边 accent @0.55 + 字重 500
    7. text.primary 在 surface.base 上的对比度 ≥ 7:1
    8. 亚克力不可用时 base_alpha = 1.0 且对比度仍 ≥ 4.5:1
    ## 输出格式
    每条判定必须给出：结论 + 采样坐标 + 实测像素值 + 规范值
    ```
  - **"无法判定"必须被允许**：亚克力模糊、真透明在无合成器环境下**读不到**（`get_image` 只返回已绘制像素）。模板必须显式允许"无法判定"并要求给出原因，**禁止**把无法判定算作通过——这与 `GUARD-06` 同源。
  - **反幻觉约束**：模板要求每条判定附**采样坐标与实测像素值**，使结论可被脚本复核（`cross_check_spec` 的独立通道）。纯文字结论（"看起来圆角正常"）视为无效输出。
  - **能力矩阵转断言**（`spec_audit.md`）：逐行读 0.5.2 的 4 平台 × N 能力表，为每行的非"支持"单元生成"降级策略可观测"的断言（例如"合成器不支持模糊 → `base_alpha = 1.0` 且诊断含 `ui/theme/blur-unavailable`"）。
- **逐步落地实施步骤**：
  1. 写 `visual_audit.md` 模板，判定项逐条锚定 features.md 3.x 的行号与数值。
  2. 写 `spec_audit.md` 模板与能力矩阵的解析器。
  3. 写输出校验器：拒绝无采样坐标的判定、拒绝把"无法判定"计为通过。
  4. 用一张**故意做错**的合成截图（圆角 8dp、描边 2px、间距 5dp）验证模板能报出全部违规。
  5. 与 `MCP-T-03` 联调：从截图 + `ui_metrics` 生成完整审查输入。
- **验收标准 (DoD)**：
  - [ ] 模板的判定项覆盖 features.md 3.1（全部尺寸）、3.2（全部 18 个 Token）、3.4（5 态），逐项可追溯行号。[文档]
  - [ ] 对"故意做错"的合成截图，模板报出全部注入的违规（召回率 100%）。[自动]
  - [ ] 无采样坐标的判定被输出校验器拒绝。[自动]
  - [ ] "无法判定"不被计为通过（断言统计口径）。[自动]
  - [ ] 0.5.2 能力矩阵的每个非"支持"单元都有对应的降级可观测断言。[文档]

---

### FEAT-TEST-P0.04.02 定位漂移与竞态的自愈归因提示

- **基本属性**：
  - 绑定平台能力：`MCP-P-02`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 2.0 人天
  - 依赖关系：`FEAT-TEST-P0.02.01`
  - 关键路径：`CP: 否`
  - 并行通道：Track B 断言与用例库
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/prompts/self_heal.md`、`xtask/src/testd/uiframe.rs`
- **MCP 能力映射**：
  - Prompts：`prompt://flaky_and_self_heal`
  - Resources：`runtime://ui_metrics`、`runtime://logs`
- **功能目标与架构说明**：输入法测试的 flaky 有两个特有来源，需要不同的归因策略：(a) **坐标漂移**——候选框位置依赖光标坐标，光标坐标依赖客户端窗口几何，任何一环变化都会让基于绝对坐标的断言失效；(b) **竞态**——`UiFrame` 有 `revision`，UI 侧应丢弃过期帧，测试若在错误的 `revision` 上断言会间歇失败。
- **实现细节与防御策略**：
  - 提示模板骨架（`self_heal.md`）：
    ```markdown
    # 归因与自愈
    ## 输入
    - 失败断言与 trace.json
    - 最近的 UiFrame 序列（含 revision）
    - 候选框几何（X11Backend::geometry() 与 hit_map）
    ## 分类（先分类再修复，禁止直接改断言阈值）
    A. 坐标漂移 → 改用 hit_map 的相对坐标 + 几何断言，不硬编码绝对像素
    B. revision 竞态 → 等待 revision 单调递增到期望值，而非 sleep
    C. 真实缺陷 → 停止自愈，上报主 Agent
    ## 红线
    - 禁止修改业务代码（crates/ 下任何文件）
    - 禁止放宽 budgets.json 或 features.md 的阈值
    - 修复仅限 xtask/src/testd/ 与 docs/dev/tests/ 下的测试脚本
    ```
  - **分类优先于修复**：模板强制先给出 A/B/C 分类与证据，避免"把真实缺陷当成 flaky 改断言"——这是自愈机制最危险的失效模式。
  - **`revision` 竞态的正确修法**：等待 `revision` 达到期望值（轮询快照，超时失败），**不是** `sleep(100ms)`。后者会在慢机器上间歇失败、在快机器上掩盖真实延迟问题。
  - **坐标漂移的正确修法**：断言几何关系（候选框水平中心 ≈ 光标水平中心 ± 2px、垂直紧贴 ± 4px，features.md `TASK-1.04.05` DoD#1），而非硬编码绝对坐标。
  - **容错机制**：自愈产生 Patch 报告（`RUN/<module>/<TC-ID>/heal.patch`），标注 `[HEALED]`，并在报告中列出被修改的测试文件；`GUARD-03` 校验修改范围未越界。
- **逐步落地实施步骤**：
  1. 写 `self_heal.md` 模板与 A/B/C 分类判据。
  2. 写 `revision` 等待助手（替代 sleep）。
  3. 写几何关系断言助手（替代绝对坐标）。
  4. 写 Patch 报告生成与 `[HEALED]` 标记。
  5. 用两个合成 flaky 场景验证分类正确：一个坐标漂移、一个 revision 竞态。
- **验收标准 (DoD)**：
  - [ ] 模板在给出分类前拒绝产出修复建议。[自动]
  - [ ] 合成坐标漂移场景被分类为 A 并产出基于 `hit_map` 的修复。[自动]
  - [ ] 合成 revision 竞态场景被分类为 B 并产出 `revision` 等待（非 sleep）。[自动]
  - [ ] 合成真实缺陷场景被分类为 C 且**不产出**任何修复建议。[自动]
  - [ ] 自愈修改的文件全部在 `xtask/src/testd/` 或 `docs/dev/tests/` 下（`GUARD-03` 校验）。[自动]

---

### FEAT-TEST-P0.05.01 环境隔离与沙盒重置

- **基本属性**：
  - 绑定平台能力：`GUARD-01`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 2.0 人天
  - 依赖关系：无
  - 关键路径：`CP: 否`
  - 并行通道：Track C CI·报告·基建
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/sandbox.rs`、`crates/ime-dict/src/user_db.rs`、`data/compiled/base.dict`
- **MCP 能力映射**：
  - 护栏：`GUARD-01`
  - Resources：`runtime://env`
- **功能目标与架构说明**：每个用例必须在**隔离且可重置**的环境中执行，杜绝交叉污染。输入法有三处可变状态必须隔离：`$XDG_DATA_HOME/rspinyin/user.redb`（用户词频）、`$XDG_CONFIG_HOME/rspinyin/config.toml`（配置）、`$XDG_RUNTIME_DIR/rspinyin-test/`（`UiFrame` 镜像与探针快照）。此外 fcitx5 会话本身是有状态的（addon 加载、活跃输入法）。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    pub struct Sandbox {
        root: PathBuf,            // RUN/<module>/<TC-ID>/sandbox/
        data_dir: PathBuf,
        config_dir: PathBuf,
        runtime_dir: PathBuf,
    }
    impl Sandbox {
        /// Creates a fresh XDG triple and returns the env vars the fcitx5 child
        /// must be launched with. Never touches the user's real directories.
        pub fn create(root: &Path) -> Result<Self, TestError>;
        /// Resets mutable state between cases: user.redb, config.toml, mirrors.
        pub fn reset(&mut self) -> Result<(), TestError>;
        /// Restarts fcitx5 with this sandbox's environment and waits for readiness.
        pub fn restart_fcitx5(&mut self, probe: &mut CommitProbe) -> Result<(), TestError>;
        pub fn env(&self) -> Vec<(String, String)>;
    }
    ```
  - **绝不触碰用户真实目录**（硬红线）：沙盒通过覆盖 `XDG_DATA_HOME` / `XDG_CONFIG_HOME` / `XDG_RUNTIME_DIR` 三个环境变量实现，**不移动、不删除**用户已有文件。测试启动前断言 `HOME` 下的 `~/.local/share/rspinyin` 未被创建（若已存在则记录其 `mtime` 并断言测试后不变）。
  - **`base.dict` 的回滚**：词库是只读的，但 `MCP-T-08` 的变异测试会改写副本。沙盒为每个用例提供 `data/compiled/base.dict` 的独立副本，并在 `reset` 时从原始文件重建（`sha256` 校验）。
  - **fcitx5 会话重置**：`restart_fcitx5` 必须等待**就绪信号**而非固定 sleep——就绪判据是 `fcitx5-diagnose` 中出现两个 addon 均 `Loaded`（ADR-0003 的双 addon），超时则失败并归档日志。
  - **容错机制**：`restart_fcitx5` 失败（端口/DBus 冲突）→ 重试一次并换用不同的 `DBUS_SESSION_BUS_ADDRESS`；仍失败则报错并保留沙盒目录供人工检查。
- **逐步落地实施步骤**：
  1. 写 `sandbox.rs` 的 XDG 三元组创建与 `env()`。
  2. 写 `reset`（`user.redb` 删除、`config.toml` 重建、镜像清空、`base.dict` 副本校验）。
  3. 写 `restart_fcitx5` 与就绪判据（双 addon `Loaded`）。
  4. 写"用户真实目录未被触碰"的断言（`mtime` 比对）。
  5. 写两个用例连续执行、第二个用例不受第一个影响的验证。
- **验收标准 (DoD)**：
  - [ ] 用例执行后 `~/.local/share/rspinyin` 与 `~/.config/rspinyin` 的 `mtime` 不变。[自动]
  - [ ] `reset` 后 `user.redb` 不存在、`config.toml` 为默认、`base.dict` 副本 `sha256` 与原件一致。[自动]
  - [ ] `restart_fcitx5` 在 5s 内检测到双 addon `Loaded`；超时则失败并归档日志。[实验室]
  - [ ] 连续执行两个用例，第二个的 `UiFrame.revision` 从 0/1 重新开始（无残留状态）。[自动]
  - [ ] 沙盒目录全部位于 `RUN/` 下，测试结束无 `/tmp` 残留。[自动]

---

### FEAT-TEST-P0.05.02 定位器自愈（常量与坐标漂移）

- **基本属性**：
  - 绑定平台能力：`GUARD-02`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 2.0 人天
  - 依赖关系：`FEAT-TEST-P0.02.01`、`FEAT-TEST-P0.02.04`
  - 关键路径：`CP: 否`
  - 并行通道：Track C CI·报告·基建
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/heal.rs`、`crates/ime-ui/ui/candidate.slint`（待实现，`TASK-1.05.03`）、`docs/dev/features.md` 3.1
- **MCP 能力映射**：
  - 护栏：`GUARD-02`
  - Prompts：`prompt://flaky_and_self_heal`
- **功能目标与架构说明**：候选框没有 DOM 选择器，"定位器"在这里是三类**真实标识**：`.slint` 的 `public constant` 名（如 `CellHeight`）、`UiFrame` 的字段名、以及由几何计算产出的 `hit_map` 相对坐标。当常量被重命名或几何算法调整时，测试应能按**语义**重新定位并标记 `[HEALED]`，而不是硬失败。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    /// A reference to a UI constant, resolved by name first and by spec row second.
    pub enum ConstRef {
        ByName(&'static str),          // "CellHeight" -- preferred
        BySpecRow { section: &'static str, row: &'static str },  // features.md 3.1 的行
    }
    pub struct ResolvedConst { pub name: String, pub value_dp: u32, pub healed: bool }
    /// Resolves `ConstRef` against the .slint constants; on a name miss it falls
    /// back to the spec row and marks the result as healed.
    pub fn resolve(r: ConstRef, slint: &SlintConstants, spec: &SpecTable)
        -> Result<ResolvedConst, TestError>;
    ```
  - **自愈的边界**：只允许"名称变更"级别的自愈（`CellHeight` → `CandidateCellHeight`）；**值变更不算自愈**——值变了意味着规范或实现被改动，必须由 `P0.02.04` 的 `cross_check_spec` 报为 `MetricMismatch` 并失败。这个区分是本任务的核心，否则自愈会掩盖真实的视觉回归。
  - **`[HEALED]` 标记**：每次自愈必须在 `assertions.json` 中记录原名、新名、依据（规范行号），并在批次 `index.md` 中汇总，使人工能复核自愈是否合理。
  - **容错机制**：名称与规范行都找不到 → `TestError::Unresolvable`，**不猜测**（禁止模糊匹配到"最像的常量"）。
- **逐步落地实施步骤**：
  1. 写 `heal.rs` 的 `ConstRef` / `ResolvedConst` / `resolve`。
  2. 写 `.slint` 常量提取（复用 `P0.02.04`）与规范行解析。
  3. 写名称缺失时的规范行回退与 `[HEALED]` 记录。
  4. 写"值变更不触发自愈"的断言。
  5. 写自测：重命名一个常量 → 自愈成功且标记；改一个常量值 → 报 `MetricMismatch` 失败。
- **验收标准 (DoD)**：
  - [ ] 常量被重命名后 `resolve` 通过规范行回退成功，`healed = true`。[自动]
  - [ ] 常量**值**被改动时**不**自愈，而是由 `cross_check_spec` 报 `MetricMismatch`。[自动]
  - [ ] 名称与规范行均不存在时返回 `Unresolvable`，不做模糊匹配。[自动]
  - [ ] 每次自愈在 `assertions.json` 与 `index.md` 中留下原名/新名/依据。[自动]

---

### FEAT-TEST-P0.05.03 自愈安全红线

- **基本属性**：
  - 绑定平台能力：`GUARD-03`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 低 | 预估工时: 1.0 人天
  - 依赖关系：无
  - 关键路径：`CP: 否`
  - 并行通道：Track C CI·报告·基建
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/guard.rs`、`docs/dev/budgets.json`、`docs/dev/features.md`、`crates/`
- **MCP 能力映射**：
  - 护栏：`GUARD-03`
- **功能目标与架构说明**：**严禁 AI 为迎合测试而修改业务逻辑或弱化断言阈值**。这是整个测试体系最重要的一条防线：一个会自动放宽阈值的测试框架比没有测试更危险。本任务把它做成**机器可判定**的门禁，而不是一句约定。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    /// Files a self-healing pass is allowed to touch. Anything else is a red-line
    /// violation and aborts the run.
    pub const HEAL_ALLOWED_PREFIXES: &[&str] = &["xtask/src/testd/", "docs/dev/tests/"];
    /// Files whose content is frozen for the whole run; a diff fails the gate.
    pub const FROZEN_FILES: &[&str] = &[
        "docs/dev/budgets.json",
        "docs/dev/features.md",
        "Cargo.toml", "Cargo.lock", "clippy.toml", "justfile",
    ];
    pub struct GuardReport { pub touched: Vec<PathBuf>, pub violations: Vec<Violation> }
    pub fn audit_heal_pass(before: &TreeHash, after: &TreeHash) -> GuardReport;
    ```
  - **四类红线**（每类都要有断言）：
    1. 自愈修改了 `crates/` 下的任何文件 → 违规。
    2. 自愈修改了 `budgets.json` 或 `features.md` 的阈值/规范 → 违规。
    3. 自愈修改了 `Cargo.toml` / `Cargo.lock` / `justfile` / `clippy.toml` → 违规。
    4. 自愈删除了测试用例或把断言替换为无条件通过 → 违规（用断言计数与 `#[test]` 计数比对检出）。
  - **树哈希**：自愈前后对整个工作树取哈希（排除 `target/` 与 `RUN/`），逐文件比对得出 `touched` 列表。
  - **`#[test]` 计数不下降**：从 401 这个基线出发，任何自愈都不得使计数下降。这是拦截"删测试让 CI 变绿"的直接手段。
  - **容错机制**：`audit_heal_pass` 自身失败（树哈希不可用）→ 视为违规并中止（fail-closed，不 fail-open）。
- **逐步落地实施步骤**：
  1. 写 `guard.rs` 的 `TreeHash`（排除 `target/`、`RUN/`、`.git/`）与 `audit_heal_pass`。
  2. 写四类红线的判据。
  3. 写 `#[test]` 计数提取与不下降断言（基线 401）。
  4. 写自测：模拟一次越界修改，断言报违规并中止。
  5. 接入 CI：自愈跑完后必须过 `audit_heal_pass`。
- **验收标准 (DoD)**：
  - [ ] 自愈触碰 `crates/` 下任一文件 → 报违规并中止。[自动]
  - [ ] 自愈修改 `budgets.json` 或 `features.md` → 报违规。[自动]
  - [ ] `#[test]` 计数下降 → 报违规。[自动]
  - [ ] 断言被替换为无条件通过（`assert!` 计数下降）→ 报违规。[自动]
  - [ ] `audit_heal_pass` 自身失败时 fail-closed（中止而非放行）。[自动]

---

### FEAT-TEST-P0.05.04 失败取证与快照存盘规范

- **基本属性**：
  - 绑定平台能力：`GUARD-04`、`GUARD-05`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 2.5 人天
  - 依赖关系：`FEAT-TEST-P0.01.02`、`FEAT-TEST-P0.02.01`
  - 关键路径：`CP: 是`
  - 并行通道：Track C CI·报告·基建
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/evidence.rs`、`results/index.json`、`RUN/index.md`
- **MCP 能力映射**：
  - 护栏：`GUARD-04`、`GUARD-05`
  - Resources：`runtime://logs`
- **功能目标与架构说明**：实现与 `dev-check` **严格一致**的存盘布局，禁止双源漂移。一个用例的全部证据内聚在一个目录里，人工复核零跨目录检索。
- **实现细节与防御策略**：
  - **目录与命名规范**（不可协商）：
    ```
    results/
      runs/run-[YYYYMMDD-HHMMSS]/          # 记为 <RUN>
        index.md                            # 用例 → 证据索引总表
        <模块代码>/<TC-ID>/                 # 一个用例一个目录
          01_default.png
          02_hover.png
          03_active.png
          assertions.json                   # 断言与视觉审查结论
          trace.json                        # 仅失败/瑕疵用例生成
          heal.patch                        # 仅发生自愈时生成
      index.json                            # 跨 run 机器可读索引 + latest_run 指针
    ```
  - **文件名不再重复 TC 编号**（目录名已承载定位）；同一步骤多帧连拍追加 `-1`/`-2`。
  - **禁止 legacy 平铺布局**：不得写入 `results/screenshots/` 或 `results/traces/`——`dev-check` 会把它们判定为 legacy 产物并迁移归档。本任务须提供**检测并拒绝**该写法的断言。
  - **`assertions.json` 结构**：
    ```json
    { "tc": "TC-CORE-01", "module": "core", "status": "pass|fail|flawed",
      "started_at": "...", "duration_ms": 1234,
      "assertions": [ { "name": "first_candidate", "expected": "你好", "actual": "你好", "ok": true } ],
      "visual": [ { "item": "corner_radius", "expected": "12dp", "measured": "24px", "verdict": "pass" } ],
      "healed": [ { "old": "CellHeight", "new": "CandidateCellHeight", "basis": "features.md 3.1" } ] }
    ```
  - **`trace.json` 仅失败/瑕疵生成**：杜绝空占位。内容含断言 Diff、图像缺陷定位（矩形 + 期望/实测像素）、堆栈、日志片段。
  - **`results/index.json`**：跨 run 的机器可读索引 + `latest_run` 指针，使 CI 能定位最近一次结果而不必扫描目录。
  - **容错机制**：PNG 写失败（磁盘满）→ 该用例标记为 `flawed` 并继续（不中止整批），但批次结论必须反映存在取证缺口。
- **逐步落地实施步骤**：
  1. 写 `evidence.rs` 的目录创建、命名规范与 `assertions.json` 序列化。
  2. 写 `trace.json` 生成（仅失败/瑕疵）。
  3. 写 `RUN/index.md` 与 `results/index.json`（含 `latest_run` 指针）。
  4. 写 legacy 布局检测与拒绝。
  5. 写自测：一个通过的用例、一个失败的用例，断言产出的证据包结构完全符合规范。
- **验收标准 (DoD)**：
  - [ ] 通过用例产出 `RUN/<模块>/<TC>/` + `assertions.json`，**不**产出 `trace.json`。[自动]
  - [ ] 失败用例额外产出 `trace.json` 且含断言 Diff 与图像定位。[自动]
  - [ ] 向 `results/screenshots/` 写入被检测并拒绝。[自动]
  - [ ] `results/index.json` 的 `latest_run` 指向最新批次。[自动]
  - [ ] 磁盘写失败时用例标记 `flawed` 且批次结论反映取证缺口。[自动]

---

### FEAT-TEST-P0.05.05 环境能力门禁

- **基本属性**：
  - 绑定平台能力：`GUARD-06`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 2.0 人天
  - 依赖关系：`FEAT-TEST-P0.02.06`
  - 关键路径：`CP: 否`
  - 并行通道：Track C CI·报告·基建
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/env_gate.rs`、`docs/dev/features.md` 0.5.5、`docs/dev/tests/`
- **MCP 能力映射**：
  - 护栏：`GUARD-06`
  - Resources：`runtime://env`
- **功能目标与架构说明**：features.md 0.5.5 已把本机能力边界清单化：X11 档可验证，**Wayland 四档、真实亚克力、多显示器热插拔、8 小时长稳不可验证**。本任务把这份清单变成**机器可判定的门禁**：不可执行的用例必须被显式标注（`[不可验证]` + 原因 + 所需环境），**不得静默跳过，也不得当作通过**。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    pub enum Executability {
        Runnable,
        /// The code under test does not exist yet; blocked on a features.md task.
        Blocked { task: String, reason: String },
        /// The environment cannot exercise this; verified elsewhere.
        Unverifiable { needs: String, reason: String },
    }
    /// Maps a case's declared environment requirement to a verdict for this machine.
    pub fn classify(case: &TestCaseMeta, env: &EnvCapabilities) -> Executability;
    /// Fails the batch when an Unverifiable case is reported as passing.
    pub fn audit_batch(results: &[CaseResult]) -> Vec<GateViolation>;
    ```
  - **三态而非两态**：`Blocked`（代码不存在，等 `features.md` 的任务）与 `Unverifiable`（代码存在但环境测不了）必须分开——前者会随时间自然消解，后者需要外部环境。混为一谈会让"待实现"看起来像"环境受限"。
  - **判定表**（来自 features.md 0.5.5）：
    | 用例声明的需求 | 本机判定 | 依据 |
    |---|---|---|
    | `X11 会话` | `Runnable` | `DISPLAY=:0` 可用，1.04.06 `READY_FOR_FINAL_GATE` |
    | `fcitx5 会话` | `Runnable` | 5.1.7 可运行，双 addon 可加载 |
    | `Wayland wlroots/KWin/Mutter` | `Unverifiable { needs: "真实 Sway/Hyprland/KWin/GNOME 会话" }` | 0.5.5：`wlr-protocols` 未安装，合成器为 Weston |
    | `合成器模糊` | `Unverifiable { needs: "支持应用侧模糊的合成器" }` | 0.5.5 |
    | `多显示器` | `Unverifiable { needs: "真实多显示器环境" }` | 0.5.5 |
    | `8 小时长稳` | `Unverifiable { needs: "裸机 Linux，8 小时独占" }` | 0.5.5 + `ASM-T-11` |
    | `候选框 UI` | `Blocked { task: "TASK-1.05.01–1.05.08" }` | 阶段零实测：零 `.slint` 文件 |
    | `配置热重载` | `Blocked { task: "TASK-1.03.06" }` | `ime-config/src/lib.rs` 为空壳 |
    | `日志脱敏` | `Blocked { task: "TASK-1.08.01" }` | `ime-diag/src/lib.rs` 为空壳 |
  - **反作弊断言**：`audit_batch` 必须检出"把 `Blocked`/`Unverifiable` 记成 `pass`"的结果，这是本任务最重要的断言——否则门禁形同虚设。
  - **容错机制**：`classify` 遇到未在判定表中声明的需求 → 返回 `Unverifiable { needs: "<未声明>", reason: "unknown requirement" }`，**默认不可信**（fail-closed）。
- **逐步落地实施步骤**：
  1. 写 `env_gate.rs` 的 `Executability` 与判定表（从 features.md 0.5.5 的结构化提取）。
  2. 写 `classify` 与 `audit_batch`。
  3. 写批次报告：汇总 `Runnable`/`Blocked`/`Unverifiable` 三类计数与逐条原因。
  4. 写反作弊断言（`Blocked`/`Unverifiable` 记为 pass 即违规）。
  5. 接入 `tests.md` 的用例元数据（每条用例声明其环境需求）。
- **验收标准 (DoD)**：
  - [ ] 本机对 `TASK-1.05.01–1.05.08` 相关用例返回 `Blocked`，对 Wayland 相关用例返回 `Unverifiable`。[自动]
  - [ ] 未声明的需求返回 `Unverifiable`（fail-closed）而非 `Runnable`。[自动]
  - [ ] `Blocked`/`Unverifiable` 被记为 `pass` 时 `audit_batch` 报违规。[自动]
  - [ ] 批次报告给出三类计数与逐条原因，可直接引用进交付报告。[文档]
  - [ ] 判定表与 features.md 0.5.5 逐条一致（脚本化比对）。[自动]

---

### FEAT-TEST-P0.05.06 性能测量纯净度

- **基本属性**：
  - 绑定平台能力：`GUARD-07`
  - 任务状态：`[ ] 待开始`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 2.0 人天
  - 依赖关系：无
  - 关键路径：`CP: 否`
  - 并行通道：Track C CI·报告·基建
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/purity.rs`、`crates/ime-core/benches/{input,passthrough}.rs`、`docs/dev/budgets.json`
- **MCP 能力映射**：
  - 护栏：`GUARD-07`
  - Resources：`runtime://env`、`runtime://budget`
- **功能目标与架构说明**：`.dev-progress.json` 记录了一条**实测教训**：并行 agent 运行时 `passthrough/classify` 测得 511ns，几分钟后空闲重测 726ns，且 criterion **自身报告了一个 +40% 的假回归**。本项目由多个 agent 并行开发，这条教训会反复出现。本任务把"性能数值必须在纯净机器上采集"变成自动门禁。
- **实现细节与防御策略**：
  - 核心接口骨架：
    ```rust
    pub struct PurityReport {
        pub concurrent_builds: u32,     // cargo/nextest/rustc processes
        pub cpu_load_1m: f64,           // /proc/loadavg
        pub cpu_count: u32,
        pub scaling_governor: Option<String>,
        pub idle_ok: bool,
    }
    impl PurityReport {
        pub fn sample() -> Result<Self, TestError>;
        /// True when the machine is quiet enough for a budget measurement.
        pub fn is_clean(&self) -> bool;
        /// Human-readable reason to attach to a rejected measurement.
        pub fn rejection_reason(&self) -> Option<String>;
    }
    ```
  - **洁净判据**（全部满足才算洁净）：
    1. `concurrent_builds == 0`（无 `cargo`/`nextest`/`rustc` 进程）。
    2. `cpu_load_1m < cpu_count × 0.25`。
    3. `scaling_governor == "performance"` 或该文件不可读时记入报告（不强制，但要标注）。
  - **拒采而非警告**：`is_clean() == false` 时，性能类断言必须**拒绝采信**并输出 `rejection_reason()`，而不是打个警告继续——警告会被忽略，这正是 511ns/726ns 事件的成因。
  - **基线冻结**：首次在洁净机器上采集的数值记入 `results/baselines/<budget_key>.json`（含 CPU 型号、governor、时间戳）。后续比较用**相对回归**（当前 / 基线），因为绝对数值跨机器不可比。
  - **容错机制**：`/proc/loadavg` 不可读 → 视为不洁净（fail-closed）。`concurrent_builds` 统计自身进程需排除（否则永远不洁净）。
- **逐步落地实施步骤**：
  1. 写 `purity.rs` 的 `PurityReport` 与三项判据。
  2. 写 `concurrent_builds` 统计（`/proc` 扫描，排除自身）。
  3. 写基线冻结与相对回归比较。
  4. 把拒采逻辑接入 `P0.03.03` 的 `compare`。
  5. 写自测：并行启动一个 `cargo check`，断言 `is_clean()` 为假且 `rejection_reason()` 指出原因。
- **验收标准 (DoD)**：
  - [ ] 有 `cargo`/`rustc` 进程运行时 `is_clean()` 为假，`rejection_reason()` 指出进程数。[自动]
  - [ ] `cpu_load_1m` 超阈值时 `is_clean()` 为假。[自动]
  - [ ] `/proc/loadavg` 不可读时视为不洁净（fail-closed）。[自动]
  - [ ] 不洁净时性能断言拒采并输出原因，**不**降级为警告。[自动]
  - [ ] 基线文件含 CPU 型号、governor、时间戳。[自动]

---

## 4. 交付与后续

- **平台侧交付物**：本文档（20 个任务卡）+ `xtask/src/testd/` 的实现。任务卡可直接分配给工程师或 AI agent 执行。
- **用例侧交付物**：[tests.md](tests.md) 与其分片（`docs/dev/tests/{core,dict,rt,ui,sec,diag,infra}.md`）。
- **执行纪律**：
  1. 性能类断言必须在 `PurityReport::is_clean()` 为真时采集（`GUARD-07`）。
  2. 不可执行的用例必须显式标注 `Blocked`/`Unverifiable`，**不得当作通过**（`GUARD-06`）。
  3. 自愈只能改 `xtask/src/testd/` 与 `docs/dev/tests/`（`GUARD-03`）。
  4. 所有阈值来自 `docs/dev/budgets.json`，禁止第二份硬编码。
- **与既有门禁的关系**：`just ci`（`check` + 五个审计脚本 + `check-budget` + `check-versions`）仍是"代码完成"的判定；本平台是"功能正确 + 视觉合格"的判定。二者互补，**不得互相替代**。

