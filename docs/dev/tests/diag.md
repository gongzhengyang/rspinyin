# rspinyin 测试用例分片 · diag（诊断、监控与可靠性）

> 分片版本: v1.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 平台任务: [../features-test.md](../features-test.md) ｜
> 被测基线: Rust 2024 workspace（`ime-diag`）+ Fcitx5 5.1.7 ｜ 关联 ADR: [../adr/0000-upstream-decisions.md](../adr/0000-upstream-decisions.md) ｜ 最后同步 Commit: `ee0dbfb` ｜
> 维护约定: 新增用例必须回写主文档第 2 节矩阵的 TC 列与维度列

## 0. 分片基线（引用主文档，不重复定义）

- **假设清单**：主文档第 1 节 + [features-test.md](../features-test.md) 第 1 节。强相关：`ASM-T-05`（空闲零轮询）、`ASM-T-11`（基准纯净度）。
- **追踪矩阵**：主文档第 2 节的 `REQ-DIAG-01` ~ `REQ-DIAG-03`。
- **预算阈值**：`docs/dev/budgets.json`（键名：`key_to_present_p50`、`key_to_present_p99`、`key_to_present_p99_144hz`、`decode_p99`、`decode_p999`、`raster_p99`、`first_key_to_visible_p99`、`idle`、`idle_redraw_count`、`idle_poll_timer_count`、`robustness.rss_drift_mb`）。
- **隐私纪律**：`AGENTS.md` 3.4——`raw`/`text`/`preedit`/候选文本/提交文本在 `ime-diag` 的脱敏拒绝清单上，**首先不得传入**，`RedactLayer` 只是防御性第二道防线。

> 阶段零实测：`crates/ime-diag/src/lib.rs` **只有文档注释，零代码、零测试**（`TASK-1.08.01` 为 `IN_PROGRESS`，`1.08.02`/`1.08.03` 为 `PENDING`）。本分片全部 15 条用例标注 `[待实现]`。

---

## 1. 用例

### TC-DIAG-01 脱敏拒绝清单（`REQ-DIAG-01`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-01` ｜ `diag` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.08.01]` ｜ `crates/ime-diag/src/redact.rs`
- **操作步骤**：
  1. 构造携带 `raw`/`text`/`preedit`/`input`/`candidate_text`/`word`/`commit_text` 字段的日志事件 -> 触发存盘：`<RUN>/diag/TC-DIAG-01/assertions.json`
  2. 断言输出中值被替换为 `<redacted:len=N>`。
- **通过标准**：字段名黑名单机制生效；`RedactLayer` 是**防御性的第二道防线**，不是记录用户内容的许可。

### TC-DIAG-02 敏感会话的整会话降级（`REQ-DIAG-01`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-01` ｜ `diag` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.08.01]`
- **操作步骤**：
  1. 在 `password == true` 的上下文中产生日志 -> 触发存盘：`<RUN>/diag/TC-DIAG-02/assertions.json`
  2. 断言全部事件降级为 `session=redacted`，只保留事件类型与时间戳，**不含输入长度**。
- **通过标准**：连长度也不记录（长度可能泄露密码长度）。

### TC-DIAG-03 家目录路径脱敏（`REQ-DIAG-01`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-01` ｜ `diag` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.08.01]`
- **操作步骤**：
  1. 记录含 `$HOME` 前缀的路径 -> 触发存盘：`<RUN>/diag/TC-DIAG-03/assertions.json`
  2. 断言 `$HOME` 被替换为 `~`。
- **通过标准**：日志中不出现完整家目录路径。

### TC-DIAG-04 日志滚动与上限（`REQ-DIAG-01`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-01` ｜ `diag` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.08.01]`
- **操作步骤**：
  1. 写入至 8MB -> 触发存盘：`<RUN>/diag/TC-DIAG-04/assertions.json`
  2. 断言滚动触发、历史文件 ≤ 3 个、总计 ≤ 32MB。
- **通过标准**：`tracing_appender::rolling` 按大小切分；新文件同样 `0600`。

### TC-DIAG-05 只读模式下的日志降级（`REQ-DIAG-01`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-01` ｜ `diag` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.08.01]`
- **操作步骤**：
  1. 日志目录不可写 -> 触发存盘：`<RUN>/diag/TC-DIAG-05/assertions.json`
  2. 断言降级到 `stderr` 且只输出 `Warn` 以上（避免污染宿主日志）。
- **通过标准**：`init_logging` ≤ 10ms；`info!` ≤ 5µs、被过滤的 `debug!` ≤ 50ns（`tracing` 字段惰性格式化）。

### TC-DIAG-06 panic 钩子：进程存活（`REQ-DIAG-02`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-02` ｜ `diag` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.08.02]` ｜ `crates/ime-diag/src/panic.rs`
- **操作步骤**：
  1. 在宿主线程的 `on_key_event` 内 `panic!` -> 触发存盘：`<RUN>/diag/TC-DIAG-06/assertions.json`
  2. 断言进程存活、候选框隐藏、下一次按键正常工作。
- **通过标准**：**这是本项目最重要的验收项之一**——输入法崩溃会连带宿主 fcitx5 一起死，所有应用同时失去输入能力。hook 顺序：记录崩溃文件 → `error!` → `stderr` → **不调用** `abort()` → 会话 reset。

### TC-DIAG-07 UI 线程 panic 隔离（`REQ-DIAG-02`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-02` ｜ `diag` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.08.02]`
- **操作步骤**：
  1. 在 UI 线程内 `panic!` -> 触发存盘：`<RUN>/diag/TC-DIAG-07/assertions.json`
  2. 断言进程存活、UI 线程继续运行（**逐轮** `catch_unwind`，非整个循环）；连续 3 次后才退出并标记线程死亡。
- **通过标准**：`run_loop` 的**每轮迭代**包裹，而非整个循环。

### TC-DIAG-08 FFI 边界 panic 兜底与崩溃文件内容（`REQ-DIAG-02`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-02` ｜ `diag` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.08.02]` ｜ `crates/ime-diag/src/crash.rs`
- **操作步骤**：
  1. 在 `extern "C"` 函数内 `panic!` -> 触发存盘：`<RUN>/diag/TC-DIAG-08/assertions.json`
  2. 断言产生 `crash/<ts>-<tid>.txt`（`0600`），含时间戳、线程名、位置、脱敏后的 payload、回溯（≥ 8 帧、≤ 64 帧）。
- **通过标准**：`#[no_panic_ffi]` 宏对 `()`/`bool`/`u32`/`*mut T` 四种返回类型均返回正确默认值；崩溃文件 ≤ 64KB。

### TC-DIAG-09 `SIGBUS` 处理与快速退出（`REQ-DIAG-02`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-02` ｜ `diag` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.08.02]`
- **操作步骤**：
  1. mmap 一个文件后 `truncate` 到 0 并访问 -> 触发存盘：`<RUN>/diag/TC-DIAG-09/assertions.json`
  2. 断言进程以退出码 70 结束，崩溃文件含信号名与地址。
- **通过标准**：`SIGBUS`/`SIGSEGV` 无法用 `catch_unwind` 捕获；handler 内**只做**异步信号安全操作（预格式化消息 + `write` + `_exit(70)`），**零分配**。`SIGSEGV` 的 `si_code` 只在 `SEGV_MAPERR`/`SEGV_ACCERR` 时记录并退出，`SI_USER`/`SI_TKILL` 时恢复默认行为（避免干扰调试器）。

### TC-DIAG-10 三处崩溃的独立取证（`REQ-DIAG-02`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-02` ｜ `diag` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.08.02]`
- **操作步骤**：
  1. 分别在宿主线程、UI 线程、FFI 边界各注入一次 panic -> 触发存盘：`<RUN>/diag/TC-DIAG-10/assertions.json`
  2. 断言产生 3 个独立崩溃文件，线程名可区分。
- **通过标准**：`#[no_panic_ffi]` 的包装覆盖全部 vtable 函数。

### TC-DIAG-11 端到端延迟探针链路（`REQ-DIAG-03`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-03` ｜ `diag` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.08.03]` ｜ `crates/ime-diag/src/probe.rs`
- **操作步骤**：
  1. 连续输入 5 分钟后 `xtask report --json` -> 触发存盘：`<RUN>/diag/TC-DIAG-11/assertions.json`
  2. 断言输出含 7 项指标的 P50/P90/P99/P999：`key_to_present`、`decode`、`raster_full`、`raster_partial`、`first_key_to_visible`、`wakeup`、`event_loop_key`。
- **通过标准**：测量链路为"宿主 `begin_key_to_present` → UI 渲染 `commit` → `UiEvent::Rendered` 回填"；探针数据走**旁路**，不进入 `UiFrame` 的 `PartialEq` 比较（否则每帧都被判定为"变了"）。

### TC-DIAG-12 探针开销可忽略（`REQ-DIAG-03`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-03` ｜ `diag` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.08.03]`
- **操作步骤**：
  1. 基准 `probe/record` -> 触发存盘：`<RUN>/diag/TC-DIAG-12/assertions.json`
  2. 断言 `record` ≤ 20ns；关闭时 ≤ 2ns（纯空操作）。
- **通过标准**：`Histogram` 用 `AtomicU64` + `Ordering::Relaxed`，单次记录 ≈ 15ns；一次按键 7 次打点 ≈ 105ns，占 2ms 预算的 0.005%。

### TC-DIAG-13 计数器清单完整性（`REQ-DIAG-03`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-03` ｜ `diag` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.08.03]`
- **操作步骤**：
  1. 脚本化 grep 断言 19 项计数器在代码中都有递增点 -> 触发存盘：`<RUN>/diag/TC-DIAG-13/assertions.json`
- **通过标准**：`ui.frame.coalesced`、`ui.control.dropped`、`ui.select.timeout`、`ui.click.debounced`、`ui.stale-select`、`ui.buffer.starvation`、`ui.not-ready`、`ui.thread.dead`、`probe.lost`、`decode.too-long`、`decode.no-path`、`dict.lookup.miss`、`userdb.commit.slow`、`data.readonly-mode`、`config.invalid`、`ui.theme.blur-unavailable`、`platform.cursor.unresolved`、`platform.x11.no-compositor`、`platform.x11.no-argb-visual`。

### TC-DIAG-14 预算比对与 `Missing ≠ Pass`（`REQ-DIAG-03`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-03` ｜ `diag` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.08.03]` + `[待实现: FEAT-TEST-P0.03.03]`
- **操作步骤**：
  1. 人为把 `budgets.json` 的 `key_to_present_p99` 改为 `0.1` -> 触发存盘：`<RUN>/diag/TC-DIAG-14/assertions.json`
  2. 断言报告输出 `FAIL`（反向验证断言真的生效）。
  3. 构造无测量数据的键，断言报 `Missing` 而非 `Pass`。
- **通过标准**：`just bench-quick` 当前在无 criterion 目标时会打印 "nothing to assert" 并**成功退出**——这正是"未测量的预算被当作已满足"的漏洞，须修复。

### TC-DIAG-15 采样不足的显式标注（`REQ-DIAG-03` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DIAG-03` ｜ `diag` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.08.03]`
- **操作步骤**：
  1. 以按键数 < 500 跑一次报告 -> 触发存盘：`<RUN>/diag/TC-DIAG-15/assertions.json`
  2. 断言报告标注"样本不足"；断言含采样时长、会话数、按键数。
- **通过标准**：读者能判断样本量是否足够；P99 标注为 `mean + 3σ` 的**估计值**。

---

## 2. 分片出口准则

1. 15 条用例在 `TASK-1.08.01`/`1.08.02`/`1.08.03` 落地后转为 `[可执行]`。
2. `ime-diag` 的 `#[test]` 计数从 **0** 上升到覆盖全部 pub 项（当前 0 是本套件最大的空白区）。
3. 探针与预算看板接入 CI（`FEAT-TEST-P0.03.03`），性能回归成为门禁。
4. 主文档矩阵的 `REQ-DIAG-01`~`REQ-DIAG-03` 行可执行性列更新为 `✅`，维度列全部勾选。
