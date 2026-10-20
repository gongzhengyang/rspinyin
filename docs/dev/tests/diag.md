# rspinyin 测试用例分片 · diag（诊断、崩溃与可观测）

> 分片版本: v3.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 执行器必读: [../test-architecture.md](../test-architecture.md) ｜ 模块形态: Headless ｜ 逻辑自检: 已通过 ｜ 维护约定: 用例状态变更须同步主文档矩阵
>
> 基线引用：`ASM-T-04/05/06/11`。模块背书 = `cargo nextest -p ime-diag`（202 测）+ `testd/{purity,memory,framerate}`。**吸收** 字段登记取代的 v2.0 用例号。

---

#### TC-DIAG-001 日志、滚动与字段脱敏

- **基本属性**：`REQ-DIAG-01` ｜ `ASM-T-06` ｜ diag ｜ ①② ｜ nextest(-p ime-diag) ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `requires_screenshots: false` ｜ `[]` ｜ 吸收：TC-DIAG-01~05 ｜ 锚点：`crates/ime-diag/src/{log,redact}.rs:13-21,62-84` ｜ 背书：log/redact 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：临时日志目录（0700）；只读目录档。
- **操作步骤与物理存盘动作**：
  1. 7 个拒写字段（raw/text/preedit/input/candidate_text/word/commit_text）→ `<redacted:len=N>` → `execution_log.txt`。
  2. 敏感会话整会话降级（消息都删，只留 app hash）；家目录 `~` 改写 → 记录。
  3. 按大小滚动与上限；只读目录 → stderr 降级 + 级别抬 warn → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：0600/0700 权限；`scrub_denied_values` 二防线对自由文本生效。
  - **[Headless 断言]**：第一道防线（代码不传内容）由背书 grep 断言。
  - **[性能断言]**：脱敏层开销在日志路径预算内（背书计时）。

#### TC-DIAG-002 panic 钩子与 FFI 兜底

- **基本属性**：`REQ-DIAG-02` ｜ `ASM-T-01` ｜ diag ｜ ② ｜ nextest ｜ `[Acceptance 验收套件]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-DIAG-06~10、23 ｜ 锚点：`crates/ime-diag/src/{panic,crash}.rs`、`ffi` guard ｜ 背书：panic/crash 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：注入 panic 的测试进程；FFI guard 直驱。
- **操作步骤与物理存盘动作**：
  1. panic 后进程存活（不 abort、不恢复默认钩子）→ `execution_log.txt`。
  2. UI 线程 panic 隔离（不拖垮宿主）；`extern "C"` 兜底返回 false + 崩溃文件 → 记录。
  3. `SIGBUS` 处理与 async-signal-safe 快速退出；三处崩溃独立取证 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：崩溃记录字段为闭合键集、无输入字段；回溯先 bound+scrub。
  - **[Headless 断言]**：消息限静态模板。
  - **[性能断言]**：钩子安装一次性，热路径零开销。

#### TC-DIAG-003 探针、预算看板与报告诚实性

- **基本属性**：`REQ-DIAG-03/06` ｜ `ASM-T-04/05/11` ｜ diag ｜ ④ ｜ nextest + `xtask budget` ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-DIAG-11~15、26~30 ｜ 锚点：`crates/ime-diag/src/probe.rs`、`report/`、`xtask/src/testd/budget_gate.rs` ｜ 背书：probe/report 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：探针快照 fixture；空闲机器（`ASM-T-11`）。
- **操作步骤与物理存盘动作**：
  1. 端到端延迟探针链路（Fibonacci 桶直方图 536B）→ `execution_log.txt`。
  2. 探针开销可忽略；计数器清单完整 → 记录。
  3. 预算比对 `Missing ≠ Pass`；采样不足显式标注；报告脱敏复查 → `assertions.json`（budget_keys: 全量键名覆盖度）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：看板只从 `budgets.json` 读阈值（无第二份硬编码）。
  - **[Headless 断言]**：文本+JSON 报告全 ASCII 断言。
  - **[性能断言]**：探针采样额外开销 ≤5%。

#### TC-DIAG-004 UiFrame 镜像通道

- **基本属性**：`REQ-DIAG-04` ｜ `ASM-T-02` ｜ diag ｜ ①② ｜ nextest + `test-mirror` ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-DIAG-16~20 ｜ 锚点：`crates/ime-diag/src/uiframe/{schema,mirror,error}.rs` ｜ 背书：uiframe 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：镜像目录 fixture。
- **操作步骤与物理存盘动作**：
  1. schema 往返与字段保真 → `execution_log.txt`。
  2. `runtime://ui_frame` 读取通道；stale 拒绝（revision/rewind 计数）→ 记录。
  3. 序列化内存/分配上限；隐私边界（镜像无候选文本）→ `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：v2 schema 兼容 v1 读（`P1.04.01` 落地后）；`FrameWatch` 契约可断言。
  - **[Headless 断言]**：镜像写入不落用户输入。
  - **[性能断言]**：写入增量 ≤1KB/帧（对齐平台卡 P1.04.01 基线）。

#### TC-DIAG-005 崩溃取证黑匣子

- **基本属性**：`REQ-DIAG-05` ｜ `ASM-T-01` ｜ diag ｜ ②③ ｜ nextest ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-DIAG-21~22、24~25 ｜ 锚点：`crates/ime-diag/src/crash/{record,signal,context}.rs` ｜ 背书：crash 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：崩溃注入 fixture。
- **操作步骤与物理存盘动作**：
  1. 记录与上限（文件大小/条目）→ `execution_log.txt`。
  2. 信号安全上下文内的记录（SIGBUS/SIGSEGV si_code 分类）→ 记录。
  3. 隐私与权限（0600）；连续崩溃限流与恢复 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`crash/` 目录聚合可被看板消费（自愈卡 `TASK-3.08.02` 的数据源）。
  - **[Headless 断言]**：async-signal-safe 纪律（无分配/锁于 handler）。
  - **[性能断言]**：取证写入不延长退出路径。

#### TC-DIAG-006 空闲纯净度探针

- **基本属性**：`REQ-DIAG-06` ｜ `ASM-T-05/11` ｜ diag ｜ ④ ｜ `testd/purity` + `pidstat` ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-DIAG-29/30 ｜ 锚点：`testd/purity.rs`、`crates/ime-diag/src/probe.rs` ｜ 背书：purity 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：60s 空闲会话；无并行负载（`ASM-T-11`）。
- **操作步骤与物理存盘动作**：
  1. 空闲 60s CPU 采样 → `execution_log.txt`。
  2. `render_count` 不增长；无轮询定时器 → `assertions.json`（budget_keys: `cpu_pct.idle`, `idle_redraw_count`, `idle_poll_timer_count`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`poll` 超时 -1（无限等待）为稳态。
  - **[Headless 断言]**：度量常量回读与 spec 一致。
  - **[性能断言]**：CPU ≤ `cpu_pct.idle`（0.3% 单核）。
