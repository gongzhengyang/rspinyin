# rspinyin 测试架构与 AI 执行器说明书（test-architecture.md · 执行前必读宪章）

> 文档版本: v3.0 ｜ 系统形态: **Hybrid**（Desktop GUI 自绘候选窗 + Headless 引擎/CLI + fcitx5 进程内插件）｜ 选定测试框架: **自研 `xtask testd` 平台（X11 XTEST 注入 + XGetImage 截图 + `UiFrame` 镜像 + 进程内引擎重放）+ `cargo nextest` / `criterion` / `alloc-count`** ｜ 关联架构规范: [./features-test.md](features-test.md)（平台任务）、[./tests.md](tests.md)（用例套件）｜ 逻辑自检: 已通过（七维基线与正则对账闭环）｜ 维护约定: 被测代码或平台演进后必须回写本文档的选型结论与 SOP；阈值只引用 `docs/dev/budgets.json` 键名，禁止出现第二数值源

**⚠️ 强制门禁**：任何 AI 测试执行器（`dev-check` / `dev-rust` 的验证阶段 / 人类工程师）在执行 [`./tests.md`](tests.md) 与 [`./tests/`](tests/) 套件中的任何用例之前，必须完整阅读本文档，并严格按第 4 节的 SOP 启动被测系统、初始化沙盒、调度分级、清理残留。

---

## 1. 被测工程技术栈全息画像与系统形态裁定

### 1.1 实测提取的技术栈（来源：`Cargo.toml`、`justfile`、源码扫描，2026-10-20）

| 维度 | 实测值 | 测试含义 |
|---|---|---|
| 语言与工具链 | Rust 2024，MSRV 1.85，toolchain 1.98.0，`resolver = "3"` | 原生测试命令 = `cargo nextest run` + `cargo test --workspace --doc`（nextest 不跑 doctest） |
| Workspace | 9 crate（ime-types/core/dict/config/diag/ui/ui-addon/fcitx5 + alloc-count）+ xtask | 被测面天然分层：契约/引擎/数据/配置/诊断/UI/宿主 |
| 渲染 | Slint 1.13 `renderer-software` + 自定义 `Platform`/`WindowAdapter`（`crates/ime-ui/src/slint_platform.rs`），UI 线程 `poll(2)` 自驱 | 无 GPU、无系统控件；像素断言走内存表面探针 + `XGetImage` |
| 窗口 | X11 override-redirect ARGB 窗口（`platform/x11.rs`）；Wayland 四档阶梯**代码在、本构建未编译** | GUI 测试面 = X11 档；Wayland 档 = `[不可验证]`（`ASM-T-08`） |
| 宿主 | fcitx5 5.1.x 进程内**双 cdylib**（引擎 `librspinyin.so` + UI `librspinyin_ui.so`，ADR-0003/0004），C ABI wire + 双 probe 握手（ADR-0011） | 端到端必须驱动**真实 fcitx5 会话**，被测对象不是独立进程 |
| 数据 | mmap 6 段容器 `base.dict`（15.62MiB，v1 无 BIGRAM 段）+ redb 用户库 + TOML 配置 + phrases.tsv | 词库类断言用**合成词库 fixture**（`dict_inject`），不依赖编译产物 |
| 网络 | 零（`BUDGET-NET-01`，`check-no-network.sh` 强制） | 任何测试不得引入外联；Mock = 进程内 fixture，非网络 Mock |
| 并发 | 双线程物理隔离 + SPSC/eventfd，**无异步运行时**（ASM-T-09） | 并发用例 = 队列语义 + 唤醒延迟，非协程调度 |
| 基线 | 3,701 `#[test]`（各 crate 分布见 tests.md §0）、criterion 基准 9、alloc-count 分配预算 `decode_steady=13` | 单元面是第零防线，E2E 卡必须引用其背书 |

### 1.2 系统形态裁定：Hybrid（三测试面）

| 测试面 | 覆盖模块 | 形态 | `requires_screenshots` | 判定面 |
|---|---|---|---|---|
| **P1 单元/集成面** | 全部 crate | Headless（进程内） | `false` | `cargo nextest` / `cargo test --doc` 退出码 |
| **P2 引擎重放面** | ime-core / ime-dict / xtask | Headless | `false` | `xtask testd-engine`（场景 TOML）、`xtask testd-smoke`（211 例 TSV）、`xtask quality` / `budget` |
| **P3 屏上端到端面** | ime-fcitx5 / ime-ui / ime-ui-addon | Desktop GUI（X11 档） | `true` | `xtask testd`（XTEST 注入 + FocusGuard）、`xtask capture`（PNG）、`xtask test-mirror`（UiFrame JSON）、`xtask testd-client`（上屏读回）、`xtask ui_metrics`（几何） |

---

## 2. 测试框架深度选型论证（ADR）

### 2.1 候选对比（按本工程真实特征裁剪，通用候选逐一裁决）

| 候选 | 适用前提 | 本工程下的裁决 | 理由（源码级证据） |
|---|---|---|---|
| Playwright / Chrome CDP | WebView/浏览器渲染 | **不适用** | 无任何 WebView/浏览器栈；Slint 软件光栅 + X11 ARGB，无 DOM 可驱动 |
| `tauri-driver` / WebDriver | Tauri/Electron 混合 | **不适用** | 非 Tauri/Electron；fcitx5 插件无 HTTP/WebDriver 端点，也不该有（零网络） |
| `slint-testing`（`slint::testing::ElementHandle`） | 测试进程内拥有 Slint app 入口 | **部分适用，已等价覆盖** | 候选窗运行在 fcitx5 进程内的 UI addon 中，测试进程无法附加其 Slint 事件循环；进程内等价物已存在——`MockBackend` + 内存表面探针（ime-ui 696 个 `#[test]` 中的像素/几何断言）与 `xtask ui_metrics`。引入它需为测试目的重排生产入口，收益为零 |
| `egui_kittest` / `iced_test` | 对应框架的组件模型 | **不适用** | 非 egui/iced |
| `expectrl` / PTY + vt100 | TUI 终端程序 | **不适用** | 无 TUI 形态 |
| `assert_cmd` + `tempfile` | CLI 黑盒 | **等价覆盖** | xtask 子命令已由 xtask 自身 1,209 个 `#[test]` + `just ci` 审计档覆盖；`dictc`/`quality`/`budget` 均为纯函数 + 退出码契约，无需再包一层进程壳 |
| **自研 `xtask testd` 平台（现状）** | X11 档 + fcitx5 沙盒 | **✅ 选定（保留并扩展）** | 唯一能驱动「真实 fcitx5 会话 → X 服务器 → 插件 → 自绘窗口」全链路的方案；20 张平台卡已交付（`74afea9` 前的 features-test v1，git 可考），实证 20/20 轮沙盒会话首词上屏 |

### 2.2 裁决结论

1. **主驱动框架**：`xtask testd` 平台（已交付，模块清单：`input`/`keys` XTEST 注入、`guard` FocusGuard 焦点护栏、`capture` XGetImage 截图、`mirror_cli` UiFrame 镜像读取、`commit_readback` GTK 探针客户端上屏读回、`engine` 进程内场景重放、`smoke` TSV 冒烟基线（211 例）、`suite` 场景套件编排、`sandbox` fcitx5 沙盒会话、`evidence` 证据包、`env_gate` 环境能力门禁、`purity` 基准纯净度、`heal` 自愈、`attribution` 缺陷归属、`framerate`/`memory` 采样、`budget_gate` 预算裁决、`dict_inject` 词库 fixture、`ui_metrics` Slint 规格度量、`logs`/`uiframe`/`review`/`coords`/`x11` 支撑层）。
2. **辅助探针**：`cargo nextest`（单元面）、`cargo test --doc`（doctest）、`criterion`（9 基准）、`alloc-count`（分配预算）、`just audits`（11 个审计脚本族）。
3. **明确不引入**：MCP server 进程、任何网络测试栈、截图 OCR、Wayland 测试档（`ASM-T-08`）。
   - **反膨胀记录（MCP server）**：skill 通用方案中的「标准 MCP server」在本工程是负资产——执行器（`dev-check`）经 Bash 调用 `xtask` 子命令即可获得全部原语，中间加一层 MCP 进程只增加序列化与生命周期成本，且违背「无网络能力」的精神。本文件第 3 节的**映射表即协议契约**，双方（工具建设方与执行方）按表对齐，不建进程。

---

## 3. 标准原语拓扑与底层驱动绑定契约（MCP 语义 → testd 驱动）

> 本表是协议契约：上层原语名是稳定词汇，右列是唯一合法实现。执行器不得绕过本表自造驱动。

### 3.1 Tools（操作与度量）

| 原语 | 底层驱动（唯一实现） | 关键契约 |
|---|---|---|
| `action_keyboard` | `xtask testd`（`input.rs` + `keys.rs`，XTEST `fake_input`） | 键事件走 X 服务器→fcitx5→插件同路径；注入前后 FocusGuard 校验焦点，`FocusStolen{by_candidate_window}` 即最高严重级缺陷 |
| `action_interact` | `xtask testd`（指针注入到 `geometry()`/`hit_map` 坐标） | 坐标来自被测窗口自身上报，禁止测试侧自行推算像素位置 |
| `capture_viewport_screenshot` | `xtask capture`（XGetImage → PNG）；GUI 不可达时降级 `capture_execution_dump`（追加 `execution_log.txt`） | 文件名 `[step-seq]_[state-tag].png`；非空即有效 |
| `get_accessibility_tree` | `xtask test-mirror`（`RSPINYIN_TEST_MIRROR_DIR` 下的 `UiFrame` JSON） | **`ASM-T-02`：无 DOM/无 A11y 树，UiFrame 镜像就是语义树**——`revision`/`preedit`/`candidates`/`page`/`status`/`anchor`/`layout` 是稳定断言面；镜像刻意不含候选文本（隐私），文本断言走 `testd-client` 上屏读回 |
| `capture_execution_dump` | Headless 面：场景 TOML 的重放 transcript / `testd-smoke` 的逐案输出 / `xtask quality` 报告 | 落 `<EVIDENCE_DIR>/execution_log.txt` |
| `measure_system_metrics` | `xtask testd`（`framerate` 帧率/`memory` RSS 采样）+ `xtask ui_metrics`（几何/规格）+ `xtask budget`（`budget_gate` 按 `budgets.json` 裁决） | 阈值只读 `budgets.json` 键名；性能数值只在**空闲机器**采信（`ASM-T-11`，`purity` 门禁拒绝受污染采样） |
| `reset_sandbox` | `xtask testd`（`sandbox.rs`：隔离 fcitx5 会话 + XDG 覆写 + 用户库复位） | 每用例前调用；禁止跨用例复用会话状态 |

### 3.2 Resources（实时上下文订阅）

| 原语 | 底层实现 | 内容 |
|---|---|---|
| `runtime://logs` | 沙盒会话的 `rspinyin.log`（0600，RedactLayer 已脱敏）+ 诊断快照行 | 执行器**只读日志目录**，禁止读入输入内容字段（脱敏层是第二道防线，第一道是数据根本不在） |
| `runtime://ui_frame` | `RSPINYIN_TEST_MIRROR_DIR` 的镜像 JSON | 稳定键集见 `ime-diag/src/uiframe/` schema；`revision` 幂等 + 旧帧 rewind 计数是过期帧契约的断言点 |
| `runtime://env` | `xtask testd`（`env.rs`/`env_gate.rs`） | `DISPLAY`/`WAYLAND_DISPLAY`/合成器/字体/词典存在性 → 用例可执行性三态判定的唯一输入 |
| `runtime://memory_profile` | `xtask testd`（`memory.rs` RSS 曲线）+ `alloc-count` 报告 | 内存断言必须是**基线差分**（`ASM-T-06`），绝对值断言错误 |

### 3.3 Prompts（审查与自愈模板）

| 原语 | 底层实现 | 用途 |
|---|---|---|
| `prompt://commercial_grade_visual_audit` | 视觉类用例的通过标准（各 ui 卡的视觉断言行）+ `xtask ui_metrics` 实测 | 视觉终审按用例卡写死的判据执行，不由执行器现场发挥 |
| `prompt://flaky_and_self_heal` | `xtask testd`（`heal.rs` 3 轮自愈 + `attribution.rs` 缺陷归属 + `purity.rs` 纯净度） | 失败先归因（被测缺陷 vs 测试污染 vs 环境缺格），自愈限 3 轮并留 `fix_patch.diff`；超限出 `trace.json` 转人工 |

---

## 4. AI 测试执行器标准作业程序（SOP Runbook）

### 4.1 SUT 构建与测试态启动

```bash
# 1) 全量构建（含测试面）
cargo build --workspace --all-targets
# 2) 引擎面词典（smoke/quality 需要；缺失时 just smoke 会报告 skip，CI 中 RSPINYIN_REQUIRE_SMOKE_DICT=1 时为红）
cargo run -p xtask -- dictc
# 3) 沙盒 fcitx5 会话拉起（testd-suite 自动完成；单独调试时）
cargo run -p xtask -- testd --help   # 注入/护栏/采样子命令族
```

**Readiness 探活契约（禁止盲目 `sleep`）**：沙盒就绪 = ①fcitx5 进程持有 display 连接且 addon 加载完成（`addon_load` 预算内，诊断流出现初始化十步完成标记）；②焦点位于探针客户端窗口（FocusGuard 首查通过）；③`RSPINYIN_TEST_MIRROR_DIR` 首帧镜像可读。三者由 `testd-suite`/`sandbox` 机械判定，执行器直接消费其就绪信号，不得自设时延。

### 4.2 沙盒初始化与用例间幂等重置

1. 每个用例独立证据目录：`<RUN>/<模块代码>/<TC-ID>/`（`<RUN>` = `results/runs/run-[YYYYMMDD-HHMMSS]`）。
2. 用例前重置：`XDG_CONFIG_HOME`/`XDG_DATA_HOME` 指向用例临时目录（mktemp -d）；用户库从零（`user.redb` 不存在 = 合法初始态）；镜像目录清空后重挂。
3. 词库类用例：一律走 `dict_inject` 合成 fixture（`ASM-T-07`：规模断言以合成词库为基准，不读 `data/compiled/`）；引擎面重放只挂只读词典。
4. 残留清理：会话级沙盒随 suite 生命周期销毁；异常退出后按进程组（PGID）清理 fcitx5 与探针客户端，并删除临时 XDG 树。

### 4.3 测试分级调度

| 层级 | 内容 | 命令 | 时机 |
|---|---|---|---|
| **Smoke**（秒级~分钟级） | 单元面增量 + 211 例 TSV 冒烟 + 引擎场景 smoke 集 | `cargo nextest run`（改动 crate）+ `just smoke` | 每次代码变更、每次自愈修复后必跑 |
| **Acceptance** | 全量单元面 + 全场景 TOML + 沙盒端到端套件 + 屏上视觉卡 | `just ci` + `cargo run -p xtask -- testd-suite` | 发版前 / 批次终审 |
| **Stress** | 8h 长稳、10k 帧投递、分配预算、大词库规模 | `xtask soak` + 对应 Stress 卡 | 周期性 / 性能改动后 |

并发约束：屏幕面用例**全局串行**（单一 X 显示、焦点竞争）；引擎面/单元面可并行；性能采样用例必须独占机器（`purity` 门禁）。

### 4.4 异常熔断与防僵尸契约

1. 单用例超时（卡内未给超时值时默认 120s）→ 取 `trace.json` + 镜像 + 日志切片 → `heal` 归因 → 最多 3 轮源码自愈（留 `fix_patch.diff`）→ 仍红则失败隔离，继续后续用例。
2. 被测进程崩溃：`crash/` 记录 + panic 钩子输出为第一证据；恢复 = 全新沙盒会话，**禁止**在污染会话上续跑。
3. 焦点被夺（`FocusStolen`）：立即熔断本用例并按 P0 缺陷上报——这是项目最高严重级缺陷类别。

### 4.5 执行器纪律红线

- **禁止**把 `[不可验证]`（Wayland 档、真实亚克力、多屏热插拔、8h 长稳子项）当作通过或失败——按 `env_gate` 三态出「本机不可验证」记录。
- **禁止**在断言中出现 `budgets.json` 之外的数值。
- **禁止**读取/落盘任何用户输入内容；候选文本断言只经 `testd-client` 读回通道（transcript 按 0600 用户数据对待）。
- **禁止**修改 `budgets.json` 来让用例通过；预算回归是门禁失败，不是告警。

---

## 5. 结构化物理证据存盘基线与形态自适应 5 维模型

### 5.1 证据契约（100% 对齐下游 `dev-check` 账本）

设 `<RUN>` = `./results/runs/run-[YYYYMMDD-HHMMSS]`：

- **用例证据包**：`<RUN>/<模块代码>/<TC-ID>/`，内容：
  - GUI（P3 面）用例：`[step-seq]_[state-tag].png`（非空，如 `01_default.png`、`02_composing.png`、`03_committed.png`）；
  - Headless（P1/P2 面）用例：`execution_log.txt`（非空：命令、输入 fixture 摘要、逐项断言结果）；
  - 通用：`assertions.json`（逐项 `{expect, actual, pass}`；字段含 `tier`、`dims`、`requires_screenshots`、`screenshots_expected`、`budget_keys`）；失败附 `trace.json`；自愈附 `fix_patch.diff`；`[不可验证]` 子项在 assertions 中记 `本机不可验证`（附 `env_gate` remedy 原文）。
- **批次索引**：`<RUN>/checklist.json`（用例清单 × 状态 × 证据存在性）、`<RUN>/index.md`（人读汇总）、`<RUN>/final-report.md`（终审结论）；产物根 `./results/index.json`（批次注册表）。
- **账本对账**：`dev-check` 以 `grep -rnE '(任务|用例)状态：`\[ \]'` 命中行驱动调度；用例卡状态行是唯一账本入口。

### 5.2 Hybrid 形态自适应 5 维模型

| 覆盖维度 | GUI 面（rt/ui 模块屏上用例） | Headless 面（core/dict/cfg/diag/sec/infra/add） |
|---|---|---|
| ① 核心业务通路 | 真实会话：键入→候选窗呈现→选择→上屏读回全闭环 | 解码/词库/配置/诊断管道输入输出闭环（场景 TOML + TSV 冒烟） |
| ② 边界与异常容错 | 超长输入、非法键序、焦点竞争、合成器异常 | 畸形容器拒止、超长 raw `decode/too-long`、坏配置修默认、损坏自愈 |
| ③ 全状态防御矩阵 | 加载中占位、空候选、readonly 降级、接管失败回退 ClassicUI | 空词库合法空结果、降级三态、大库 on-demand 路径 |
| ④ 工艺质感/韧性 | 像素格式、圆角/描边、深浅色对比度、Spring 动效帧预算、脏区渲染 | 延迟/内存/分配预算（budgets.json 全键）、确定性字节指纹 |
| ⑤ 人机工学/控制流 | 全键盘翻页/高亮/选择、数字键 0 语义、Escape 语义 | 退出码契约、信号安全停机、配置三态标注一致性 |

### 5.3 5 维到套件分级的映射规则

每张用例卡标注 `[Smoke]` / `[Acceptance]` / `[Stress]`；一个 REQ 的 5 维覆盖可由 3~5 张高内聚卡合成（维度允许合并标注，如「维度 1+3」），但**每个维度至少被该 REQ 的一张卡覆盖**——由 tests.md §2 矩阵的维度列机械核验。

---

## 6. 对既有测试资产的处置决定（v3.0 重建记录）

| 资产 | 处置 | 依据 |
|---|---|---|
| `xtask testd` 平台（20 卡交付物） | **保留，扩展 8 张新平台卡**（见 features-test v3） | 唯一可行的端到端驱动；§2.1 逐项否决了替代品 |
| 3,701 `#[test]` / criterion 9 / alloc-count | **保留**（基线不得下降，由平台卡守） | 门禁既成事实 |
| 18 场景 TOML + 211 例 TSV 冒烟 | **保留**；场景 schema 按需扩展（平台卡 P0.02.01） | 已入 `just ci` 链 |
| `docs/dev/tests.md` v2.0 + 8 旧分片 | **废弃重建为 v3.0**（9 分片：原 8 模块 + `add.md`） | 状态行/分级/证据字段不合 v3 合同；Hub 内嵌 125 条用例正文违反 Hub&Spoke；0 条覆盖 features-add 增量面 |
| `docs/dev/features-test.md`（已被 `74afea9` 删除，v2.0 套件悬空引用它） | **重建**（v3.0：只承载剩余平台工作 8 卡 + 平台台账） | 断链修复；已交付的 20 卡转入台账不重写 |
| `docs/dev/test-architecture.md` | **新建**（本文件） | 执行器宪章此前不存在 |
| 旧 382 条用例的断言内容与验收记录 | **吸收**进 v3.0 卡（每卡列「吸收」映射），证据包 `results/runs/` 原样保留 | 内容优质；粒度按 5 维高内聚原则合并（382 → 94），逐行为断言项而非独立卡 |
