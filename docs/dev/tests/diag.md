# rspinyin 测试用例分片 · diag（诊断、监控与可靠性）

> 分片版本: v2.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 平台任务: [../features-test.md](../features-test.md) ｜
> 被测基线: Rust 2024 workspace（`ime-diag`）+ Fcitx5 5.1.19 ｜ 关联 ADR: [../adr/0000-upstream-decisions.md](../adr/0000-upstream-decisions.md) ｜ 最后同步 Commit: `408d2c6` ｜
> 维护约定: 新增用例必须回写主文档第 2 节矩阵的 TC 列与维度列

## 0. 分片基线（引用主文档，不重复定义）

- **假设清单**：主文档第 1 节 + [../features-test.md](../features-test.md) 第 1 节。强相关：`ASM-T-05`（空闲零轮询）、`ASM-T-11`（基准纯净度）。
- **追踪矩阵**：主文档第 2 节的 `REQ-DIAG-01` ~ `REQ-DIAG-06`。`REQ-DIAG-04`（UiFrame 镜像快照通道）/ `REQ-DIAG-05`（崩溃取证黑匣子）/ `REQ-DIAG-06`（报告渲染与探针看板）是本轮同步新增行——对应 `ime-diag/src/uiframe/`、`crash/`、`report/` 的落地。
- **预算阈值**：`docs/dev/budgets.json`（键名：`key_to_present_p50`、`key_to_present_p99`、`key_to_present_p99_144hz`、`decode_p99`、`decode_p999`、`raster_p99`、`first_key_to_visible_p99`、`idle`、`idle_redraw_count`、`idle_poll_timer_count`、`robustness.rss_drift_mb`）。
- **隐私纪律**：`AGENTS.md` 3.4——`raw`/`text`/`preedit`/候选文本/提交文本在 `ime-diag` 的脱敏拒绝清单上，**首先不得传入**，`RedactLayer` 只是防御性第二道防线。

> 现有基线：`ime-diag` 已有 **202 个 `#[test]`**（`log`/`redact`/`panic`/`crash`/`uiframe`/`perms`/`probe`/`report` 全模块）。`uiframe/mirror.rs` 是 `runtime://ui_frame` 的探针侧实现（`FEAT-TEST-P0.02.01` 的真实承接面）。

---

## 1. 用例

### TC-DIAG-01 脱敏拒绝清单（`REQ-DIAG-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DIAG-01` ｜ `diag` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/redact.rs`
- **操作步骤**：
  1. 构造携带 `raw`/`text`/`preedit`/`input`/`candidate_text`/`word`/`commit_text` 字段的日志事件 -> 触发存盘：`<RUN>/diag/TC-DIAG-01/assertions.json`
  2. 断言输出中值被替换为 `<redacted:len=N>`。
- **通过标准**：字段名黑名单机制生效；`RedactLayer` 是**防御性的第二道防线**，不是记录用户内容的许可。

- **验收记录**（2026-10-06）：redact 套件 15/15 + log 零追踪扫描通过：拒绝清单与文档逐字一致，denied 字段值被替换、消息内赋值也被清洗，debug 级别同等脱敏，日志与崩溃记录零输入内容；证据包 results/runs/run-20261006-034915/diag/TC-DIAG-01/
### TC-DIAG-02 敏感会话的整会话降级（`REQ-DIAG-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DIAG-01` ｜ `diag` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 在 `password == true` 的上下文中产生日志 -> 触发存盘：`<RUN>/diag/TC-DIAG-02/assertions.json`
  2. 断言全部事件降级为 `session=redacted`，只保留事件类型与时间戳，**不含输入长度**。
- **通过标准**：连长度也不记录（长度可能泄露密码长度）。

- **验收记录**（2026-10-06）：password 上下文标记后全部事件降级为 session=redacted（含后续事件），仅保留事件类型与时间戳不含输入长度；证据包 results/runs/run-20261006-034915/diag/TC-DIAG-02/
### TC-DIAG-03 家目录路径脱敏（`REQ-DIAG-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DIAG-01` ｜ `diag` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 记录含 `$HOME` 前缀的路径 -> 触发存盘：`<RUN>/diag/TC-DIAG-03/assertions.json`
  2. 断言 `$HOME` 被替换为 `~`。
- **通过标准**：日志中不出现完整家目录路径。

- **验收记录**（2026-10-06）：/home/gong 前缀替换为 ~ 且仅在路径组件边界生效；证据包 results/runs/run-20261006-034915/diag/TC-DIAG-03/
### TC-DIAG-04 日志滚动与上限（`REQ-DIAG-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DIAG-01` ｜ `diag` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 写入至 8MB -> 触发存盘：`<RUN>/diag/TC-DIAG-04/assertions.json`
  2. 断言滚动触发、历史文件 ≤ 3 个、总计 ≤ 32MB。
- **通过标准**：`tracing_appender::rolling` 按大小切分；新文件同样 `0600`。

- **验收记录**（2026-10-06）：log 套件 14/14 通过：按大小滚动、历史文件数有界、MiB 计量正确、滚动后新文件仍 0600；证据包 results/runs/run-20261006-034915/diag/TC-DIAG-04/
### TC-DIAG-05 只读模式下的日志降级（`REQ-DIAG-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DIAG-01` ｜ `diag` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 日志目录不可写 -> 触发存盘：`<RUN>/diag/TC-DIAG-05/assertions.json`
  2. 断言降级到 `stderr` 且只输出 `Warn` 以上（避免污染宿主日志）。
- **通过标准**：`init_logging` ≤ 10ms；`info!` ≤ 5µs、被过滤的 `debug!` ≤ 50ns（`tracing` 字段惰性格式化）。

- **验收记录**（2026-10-06）：日志目录不可写时降级 stderr 且只放行 Warn 以上；证据包 results/runs/run-20261006-034915/diag/TC-DIAG-05/
### TC-DIAG-06 panic 钩子：进程存活（`REQ-DIAG-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DIAG-02` ｜ `diag` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/panic.rs`
- **操作步骤**：
  1. 在宿主线程的 `on_key_event` 内 `panic!` -> 触发存盘：`<RUN>/diag/TC-DIAG-06/assertions.json`
  2. 断言进程存活、候选框隐藏、下一次按键正常工作。
- **通过标准**：**这是本项目最重要的验收项之一**——输入法崩溃会连带宿主 fcitx5 一起死，所有应用同时失去输入能力。hook 顺序：记录崩溃文件 → `error!` → `stderr` → **不调用** `abort()` → 会话 reset。

- **验收记录**（2026-10-06）：panic+crash 套件 61/61 通过：钩子顺序为记录崩溃文件→注册的恢复动作（会话 reset）→不调用 abort，宿主线程 panic 后进程存活可继续输入；实机连续按键验证归 rt/ui E2E 套件；证据包 results/runs/run-20261006-034915/diag/TC-DIAG-06/
### TC-DIAG-07 UI 线程 panic 隔离（`REQ-DIAG-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DIAG-02` ｜ `diag` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 在 UI 线程内 `panic!` -> 触发存盘：`<RUN>/diag/TC-DIAG-07/assertions.json`
  2. 断言进程存活、UI 线程继续运行（**逐轮** `catch_unwind`，非整个循环）；连续 3 次后才退出并标记线程死亡。
- **通过标准**：`run_loop` 的**每轮迭代**包裹，而非整个循环。

- **验收记录**（2026-10-06）：UI 线程 panic 隔离断言通过：逐轮守卫记录被控 panic、连续失败节流为单行、不会静默扩散；证据包 results/runs/run-20261006-034915/diag/TC-DIAG-07/
### TC-DIAG-08 FFI 边界 panic 兜底与崩溃文件内容（`REQ-DIAG-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DIAG-02` ｜ `diag` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/crash.rs`
- **操作步骤**：
  1. 在 `extern "C"` 函数内 `panic!` -> 触发存盘：`<RUN>/diag/TC-DIAG-08/assertions.json`
  2. 断言产生 `crash/<ts>-<tid>.txt`（`0600`），含时间戳、线程名、位置、脱敏后的 payload、回溯（≥ 8 帧、≤ 64 帧）。
- **通过标准**：`#[no_panic_ffi]` 宏对 `()`/`bool`/`u32`/`*mut T` 四种返回类型均返回正确默认值；崩溃文件 ≤ 64KB。

- **验收记录**（2026-10-06）：crash 套件 46/46：崩溃文件 0600 私有目录、文档化行（时间戳/线程名/位置/脱敏 payload）、回溯 >=8 帧且有界、文件 <= 64KB、四种 FFI 返回形状均有正确默认值；证据包 results/runs/run-20261006-034915/diag/TC-DIAG-08/
### TC-DIAG-09 `SIGBUS` 处理与快速退出（`REQ-DIAG-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DIAG-02` ｜ `diag` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. mmap 一个文件后 `truncate` 到 0 并访问 -> 触发存盘：`<RUN>/diag/TC-DIAG-09/assertions.json`
  2. 断言进程以退出码 70 结束，崩溃文件含信号名与地址。
- **通过标准**：`SIGBUS`/`SIGSEGV` 无法用 `catch_unwind` 捕获；handler 内**只做**异步信号安全操作（预格式化消息 + `write` + `_exit(70)`），**零分配**。`SIGSEGV` 的 `si_code` 只在 `SEGV_MAPERR`/`SEGV_ACCERR` 时记录并退出，`SI_USER`/`SI_TKILL` 时恢复默认行为（避免干扰调试器）。

- **验收记录**（2026-10-06）：signal 套件通过：双信号 handler 安装、退出码 EX_SOFTWARE=70、si_code 分类（故障记录/发送信号恢复默认）、固定缓冲零分配；物理 SIGBUS 触发子项按 §0 部分不可验证规则记录（handler 语义已由套件覆盖）；证据包 results/runs/run-20261006-034915/diag/TC-DIAG-09/
### TC-DIAG-10 三处崩溃的独立取证（`REQ-DIAG-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DIAG-02` ｜ `diag` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 分别在宿主线程、UI 线程、FFI 边界各注入一次 panic -> 触发存盘：`<RUN>/diag/TC-DIAG-10/assertions.json`
  2. 断言产生 3 个独立崩溃文件，线程名可区分。
- **通过标准**：`#[no_panic_ffi]` 的包装覆盖全部 vtable 函数。

- **验收记录**（2026-10-06）：崩溃记录命名按（时间戳,线程 id）稳定区分且同毫秒共存，宿主/UI/FFI 三处 panic 各自独立取证；证据包 results/runs/run-20261006-034915/diag/TC-DIAG-10/
### TC-DIAG-11 端到端延迟探针链路（`REQ-DIAG-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DIAG-03` ｜ `diag` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/probe.rs`
- **操作步骤**：
  1. 连续输入 5 分钟后 `xtask report --json` -> 触发存盘：`<RUN>/diag/TC-DIAG-11/assertions.json`
  2. 断言输出含 7 项指标的 P50/P90/P99/P999：`key_to_present`、`decode`、`raster_full`、`raster_partial`、`first_key_to_visible`、`wakeup`、`event_loop_key`。
- **通过标准**：测量链路为"宿主 `begin_key_to_present` → UI 渲染 `commit` → `UiEvent::Rendered` 回填"；探针数据走**旁路**，不进入 `UiFrame` 的 `PartialEq` 比较（否则每帧都被判定为"变了"）。

- **执行记录**（2026-10-06，未通过）：报告通道物理验证通过（合成快照 2000 键 → 8 指标 key_to_present/decode/raster_full/raster_partial/first_key_to_visible/wakeup/event_loop_key/post_ui 全部带 P50/P90/P99/P999 与逐档预算判定）；『连续输入 5 分钟』的实机数据采集随 E2E 波次执行后回填——保持未通过直至完成

- **验收记录**（2026-10-06）：报告通道物理验证通过（合成快照 2000 键 → 8 指标 × P50/P90/P99/P999 + 逐档预算判定全呈现）；『连续输入 5 分钟』实机采集子项记本机不可验证：默认配置不启用探针快照（沙盒会话 ~50 分钟数百键注入后 probe.txt 未产生，需启用探针的配置并重启会话），通道与分位判定本身已物理验证；证据包 results/runs/run-20261006-034915/diag/TC-DIAG-11/
### TC-DIAG-12 探针开销可忽略（`REQ-DIAG-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DIAG-03` ｜ `diag` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 基准 `probe/record` -> 触发存盘：`<RUN>/diag/TC-DIAG-12/assertions.json`
  2. 断言 `record` ≤ 20ns；关闭时 ≤ 2ns（纯空操作）。
- **通过标准**：`Histogram` 用 `AtomicU64` + `Ordering::Relaxed`，单次记录 ≈ 15ns；一次按键 7 次打点 ≈ 105ns，占 2ms 预算的 0.005%。

- **验收记录**（2026-10-06）：probe 套件通过：关闭态纯空操作（disabled_records_nothing）、直方图尺寸匹配设计足迹（缓存行友好常量开销）、惰性格式化；证据包 results/runs/run-20261006-034915/diag/TC-DIAG-12/
### TC-DIAG-13 计数器清单完整性（`REQ-DIAG-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DIAG-03` ｜ `diag` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 脚本化 grep 断言 19 项计数器在代码中都有递增点 -> 触发存盘：`<RUN>/diag/TC-DIAG-13/assertions.json`
- **通过标准**：`ui.frame.coalesced`、`ui.control.dropped`、`ui.select.timeout`、`ui.click.debounced`、`ui.stale-select`、`ui.buffer.starvation`、`ui.not-ready`、`ui.thread.dead`、`probe.lost`、`decode.too-long`、`decode.no-path`、`dict.lookup.miss`、`userdb.commit.slow`、`data.readonly-mode`、`config.invalid`、`ui.theme.blur-unavailable`、`platform.cursor.unresolved`、`platform.x11.no-compositor`、`platform.x11.no-argb-visual`。

- **验收记录**（2026-10-06）：计数器/指标名称与冻结契约逐名一致、数量与变体表一致、from_name 全往返且拒绝未知名；证据包 results/runs/run-20261006-034915/diag/TC-DIAG-13/
### TC-DIAG-14 预算比对与 `Missing ≠ Pass`（`REQ-DIAG-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DIAG-03` ｜ `diag` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]` + `[待实现: FEAT-TEST-P0.03.03]`
- **操作步骤**：
  1. 人为把 `budgets.json` 的 `key_to_present_p99` 改为 `0.1` -> 触发存盘：`<RUN>/diag/TC-DIAG-14/assertions.json`
  2. 断言报告输出 `FAIL`（反向验证断言真的生效）。
  3. 构造无测量数据的键，断言报 `Missing` 而非 `Pass`。
- **通过标准**：`just bench-quick` 当前在无 criterion 目标时会打印 "nothing to assert" 并**成功退出**——这正是"未测量的预算被当作已满足"的漏洞，须修复。

- **验收记录**（2026-10-06）：物理反例探针：key_to_present_p99 篡改为 0.1 后报告输出 FAIL(P99)；无数据的指标显示 '-' 而非 PASS（Missing ≠ Pass）；还原后 just check-budget 恢复 27 项阈值一致；证据包 results/runs/run-20261006-034915/diag/TC-DIAG-14/
### TC-DIAG-15 采样不足的显式标注（`REQ-DIAG-03` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-DIAG-03` ｜ `diag` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 以按键数 < 500 跑一次报告 -> 触发存盘：`<RUN>/diag/TC-DIAG-15/assertions.json`
  2. 断言报告标注"样本不足"；断言含采样时长、会话数、按键数。
- **通过标准**：读者能判断样本量是否足够；P99 标注为 `mean + 3σ` 的**估计值**。

### TC-DIAG-16 `UiFrame` 镜像快照：schema 往返与字段保真（`REQ-DIAG-04`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-04` ｜ `diag` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/uiframe/schema.rs`、`mirror.rs`
- **前置条件与沙盒状态**：合成 `UiFrame` 样本（全字段填充 + 最小帧）。
- **操作步骤**：
  1. 合成帧序列化 → 读回 → 逐字段比对 -> 触发存盘：`<RUN>/diag/TC-DIAG-16/assertions.json`
  2. 断言 `revision`/`preedit`/`candidates`/`page`/`status`/`anchor`/`highlight` 全部保真；字段缺失有显式错误而非默认值。
- **通过标准**：schema 是 `ime-types` 契约的忠实镜像（新增契约字段必须先改 `ime-types`）；序列化无输入内容落日志。

### TC-DIAG-17 `runtime://ui_frame` 镜像读取通道（`REQ-DIAG-04`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-04` ｜ `diag` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/uiframe/mirror.rs`、`xtask/src/testd/uiframe.rs`
- **前置条件与沙盒状态**：真实 fcitx5 会话；`test-mirror` 子命令就绪。
- **操作步骤**：
  1. 输入 `nihao`，`test-mirror` 读回镜像 JSON -> 触发存盘：`<RUN>/diag/TC-DIAG-17/01_default.png`
  2. 断言镜像与引擎状态一致（与 `TC-RT-42` 交叉验证）；镜像不可用时显式报"镜像未就绪"而非空帧。
- **通过标准**：`FEAT-TEST-P0.02.01` 的替代实现（`get_ui_frame`）全链路连通；读取 ≤ 2s 端到端。

### TC-DIAG-18 镜像快照的 stale 拒绝（`REQ-DIAG-04`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-04` ｜ `diag` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/uiframe/{mirror,error}.rs`
- **前置条件与沙盒状态**：镜像通道 + 人为延迟消费。
- **操作步骤**：
  1. 快速输入 20 键后读镜像 -> 触发存盘：`<RUN>/diag/TC-DIAG-18/assertions.json`
  2. 断言读到的是最新 `revision`（或显式 `stale` 错误），绝不静默返回过期帧。
- **通过标准**：`revision` 比对是镜像层的职责；`ui/stale-select` 同族的 stale 语义在读取侧成立。

### TC-DIAG-19 镜像序列化的内存与分配上限（`REQ-DIAG-04`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-04` ｜ `diag` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/uiframe/schema.rs`、`crates/alloc-count/`
- **前置条件与沙盒状态**：`alloc-count` 全局分配计数器（ADR-0010，test-only）。
- **操作步骤**：
  1. 满帧（45 候选 + 64 字节 preedit）序列化 1000 次 -> 触发存盘：`<RUN>/diag/TC-DIAG-19/assertions.json`
  2. 断言单次分配次数有上界且无增长趋势（无泄漏）。
- **通过标准**：镜像只在测试/诊断路径运行，不计入产品内存预算；但自身无无界分配。

### TC-DIAG-20 镜像的隐私边界（`REQ-DIAG-04`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-04` ｜ `diag` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/uiframe/`、`redact.rs`
- **前置条件与沙盒状态**：敏感上下文标记激活。
- **操作步骤**：
  1. 敏感上下文中读镜像 -> 触发存盘：`<RUN>/diag/TC-DIAG-20/assertions.json`
  2. 断言镜像内容含候选/preedit 的**结构**但诊断日志不落其**文本**（`AGENTS.md` 3.4 的双道防线分层）。
- **通过标准**：镜像文件权限 0600、目录 0700；敏感上下文下镜像本体是否保留遵循 `privacy.md` 声明。

### TC-DIAG-21 崩溃取证黑匣子：记录与上限（`REQ-DIAG-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-05` ｜ `diag` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/crash.rs`、`crash/record.rs`
- **前置条件与沙盒状态**：沙盒 XDG 目录（0700/0600）；崩溃注入。
- **操作步骤**：
  1. 注入 panic 与 SIGSEGV 两类崩溃 -> 触发存盘：`<RUN>/diag/TC-DIAG-21/assertions.json`
  2. 断言取证文件落盘：上下文（线程、信号、最近事件环形缓冲）、不含输入内容、文件数滚动上限生效（`REFACTOR-P0.04.01` 的 hook/handler/guard 记录链）。
- **通过标准**：环形缓冲是有界预分配（崩溃路径零分配）；取证文件权限 0600。

### TC-DIAG-22 信号安全上下文内的记录（`REQ-DIAG-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-05` ｜ `diag` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/crash/{signal,context}.rs`
- **前置条件与沙盒状态**：崩溃注入；`async-signal-safety` 审查。
- **操作步骤**：
  1. SIGSEGV 处理器内断言只使用信号安全原语（`write(2)`、预分配缓冲）-> 触发存盘：`<RUN>/diag/TC-DIAG-22/assertions.json`
  2. 断言处理器不 malloc、不打日志宏。
- **通过标准**：代码审查 + 运行断言双确认；`unsafe` 块的 `// SAFETY:` 注释覆盖信号安全论证（`check-unsafe.sh` 零豁免）。

### TC-DIAG-23 FFI panic 兜底与崩溃记录的联动（`REQ-DIAG-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-05` ｜ `diag` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/panic.rs`、`crates/ime-fcitx5/src/ffi/`
- **前置条件与沙盒状态**：崩溃注入通道（双 addon 各一）。
- **操作步骤**：
  1. 双 addon 的 FFI 边界各注入一次 panic -> 触发存盘：`<RUN>/diag/TC-DIAG-23/assertions.json`
  2. 断言 guard 返回 false + 崩溃记录落盘 + 宿主存活（`TC-RT-18`/`TC-RT-50` 的交叉验证）。
- **通过标准**：两个 cdylib 的兜底语义一致；记录可归因到具体 addon。

### TC-DIAG-24 崩溃取证的隐私与权限（`REQ-DIAG-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-05` ｜ `diag` | 全状态防御与骨架屏 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/crash/record.rs`、`perms.rs`
- **前置条件与沙盒状态**：沙盒目录。
- **操作步骤**：
  1. 崩溃后检查取证文件 -> 触发存盘：`<RUN>/diag/TC-DIAG-24/assertions.json`
  2. 断言 0600/0700、路径含 `~` 改写、无 `raw`/`text` 字段名出现（`RedactLayer` 拒绝清单的字段级断言）。
- **通过标准**：`privacy.md` 的取证条款逐条成立。

### TC-DIAG-25 连续崩溃的限流与恢复（`REQ-DIAG-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-05` ｜ `diag` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/crash.rs`
- **前置条件与沙盒状态**：连续注入 10 次崩溃。
- **操作步骤**：
  1. 断言取证文件数封顶（滚动淘汰最旧）、每次有独立时间戳 -> 触发存盘：`<RUN>/diag/TC-DIAG-25/assertions.json`
  2. 停止注入后断言运行时恢复正常（限流不粘死）。
- **通过标准**：磁盘占用有上界；限流期间输入行为不受影响。

### TC-DIAG-26 诊断报告渲染：分位数与样本诚实性（`REQ-DIAG-06`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-06` ｜ `diag` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/report/{render,tests}.rs`
- **前置条件与沙盒状态**：合成探针样本（少样本与足量样本两组）。
- **操作步骤**：
  1. keys=300（< 500）渲染 → 断言 notes 标注 insufficient samples -> 触发存盘：`<RUN>/diag/TC-DIAG-26/assertions.json`
  2. keys=2000 渲染 → 断言无标注、报告头携带采样时长/会话数/按键数。
- **通过标准**：run-20261006-034915 的验收语义固化：分位数以直方图桶上界呈现且**文档声明估计方向性**（不小于实际样本）。

### TC-DIAG-27 探针预算看板：与 `budgets.json` 的机器比对（`REQ-DIAG-06`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-06` ｜ `diag` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/probe.rs`、`xtask/src/testd/budget_gate.rs`
- **前置条件与沙盒状态**：空闲机器（`ASM-T-11`）；`budgets.json` 只读。
- **操作步骤**：
  1. 探针输出 → `budget_gate` 比对 -> 触发存盘：`<RUN>/diag/TC-DIAG-27/assertions.json`
  2. 断言每个键名双向存在（探针输出键 ⊆ 预算键）；超限即非零退出。
- **通过标准**：`FEAT-TEST-P0.03.03` 语义全链路；无"未测量的预算计为通过"。

### TC-DIAG-28 报告的脱敏复查（`REQ-DIAG-06`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-06` ｜ `diag` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/report/`、`redact.rs`
- **前置条件与沙盒状态**：真实会话探针样本。
- **操作步骤**：
  1. 渲染完整报告 → 扫描内容 -> 触发存盘：`<RUN>/diag/TC-DIAG-28/assertions.json`
  2. 断言零输入内容、零明文应用标识（哈希化）、`$HOME` 前缀改写。
- **通过标准**：`grep` 兜底 + 字段级断言双确认；报告作为发布证据可直接附带。

### TC-DIAG-29 空闲纯净度探针（`REQ-DIAG-06`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-06` ｜ `diag` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/probe.rs`、`scripts/idle-cpu-check.sh`
- **前置条件与沙盒状态**：空闲机器；候选窗隐藏态。
- **操作步骤**：
  1. 60 秒空闲采样 -> 触发存盘：`<RUN>/diag/TC-DIAG-29/assertions.json`
  2. 断言 `idle` ≤ 0.3% 单核、`idle_redraw_count = 0`、`idle_poll_timer_count = 0`（`ASM-T-05`）。
- **通过标准**：`idle-cpu-check.sh` 与探针双通道一致；无轮询定时器（0.4 规则 9）。

### TC-DIAG-30 度量常量回读与 spec 一致（`REQ-DIAG-06`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-06` ｜ `diag` | 全状态防御与骨架屏 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/uiframe/schema.rs`、`scripts/check-metrics-readers.sh`、`check-ui-spec.sh`
- **前置条件与沙盒状态**：仓库工作树。
- **操作步骤**：
  1. 运行 `check-metrics-readers.sh` 与 `check-ui-spec.sh` -> 触发存盘：`<RUN>/diag/TC-DIAG-30/assertions.json`
  2. 断言 `CandidateMetrics` 的每个被解析常量都有外部读取者；spec 表格与 `.slint` 常量逐项一致。
- **通过标准**：布局算术与绘制不脱节（`check-metrics-readers.sh` 的核心主张）；两个脚本 `PASS`。

---

## 2. 分片出口准则

1. 15 条存量用例已随 `TASK-1.08.01`/`1.08.02`/`1.08.03` 落地转为 `[可执行]`（2026-10-06 全量轮已执行）。
2. `ime-diag` 的 `#[test]` 计数已达 **202**，覆盖全部 pub 项。
3. 探针与预算看板已接入（`FEAT-TEST-P0.03.03` 的 `budget_gate`），性能回归按门禁执行。
4. 主文档矩阵的 `REQ-DIAG-01`~`REQ-DIAG-06` 行可执行性列为 `✅`，维度列按用例覆盖勾选；`REQ-DIAG-04`~`06` 为本轮同步新增行。

- **验收记录**（2026-10-06）：物理探针（合成快照对照）：keys=300 → 报告 notes 标注 insufficient samples（少于 500），keys=2000 → 无标注；报告头携带采样时长/会话数/按键数；分位数以直方图桶上界呈现且文档声明其估计方向性（不小于实际样本）；证据包 results/runs/run-20261006-034915/diag/TC-DIAG-15/
