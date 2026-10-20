# rspinyin 测试平台与 MCP 基建研发规划（features-test.md · 主控）

> 文档版本: v3.0 ｜ 系统形态: Hybrid（Desktop GUI 自绘候选窗 + Headless 引擎/CLI）｜ 选定测试框架: 自研 `xtask testd` 平台 + `cargo nextest`/`criterion`/`alloc-count`（选型论证见 [./test-architecture.md](test-architecture.md) §2）｜ 关联架构规范: [./test-architecture.md](test-architecture.md) ｜ 关联 ADR: [./adr/0012-corpus-and-word-sources.md](./adr/0012-corpus-and-word-sources.md)（词源/语料治理，测试 fixture 相关）｜ 前代版本: v1（20 卡全交付，随 `74afea9` 删除，台账见 §4）｜ 逻辑自检: 已通过（七维基线与正则对账闭环）｜ 维护约定: 平台能力演进后必须回写 §2 追溯表与任务卡状态

---

## 0. 文档集群分片索引

| 文件 | 职责 | 任务区间 |
|---|---|---|
| [./test-architecture.md](test-architecture.md) | 执行器宪章：技术栈画像、框架选型 ADR、原语→驱动映射、SOP、证据契约 | — |
| [./features-test.md](features-test.md)（本文件） | 平台能力追溯表、DAG/CP、已交付平台台账、增量平台工作分片路由 | — |
| [./features-test-phase1.md](features-test-phase1.md) | P0 平台内核：证据账本 v3、环境门禁三态扩展、场景 schema v2、词库 fixture 扩展 | `FEAT-TEST-P0.01.01` ~ `P0.02.02`（4 卡） |
| [./features-test-phase2.md](features-test-phase2.md) | P1 断言引擎：视觉基线差分、增量面镜像对齐 | `FEAT-TEST-P1.03.01` ~ `P1.04.01`（2 卡） |
| [./features-test-phase3.md](features-test-phase3.md) | P2 账本与 CI：批次报告生成器、分级门禁调度 | `FEAT-TEST-P2.05.01` ~ `P2.06.01`（2 卡） |
| [./tests.md](tests.md) + [./tests/](tests/) | 业务用例集群（与平台集群解耦，单独对账） | — |

---

## 1. 系统设计假设清单（ASM-T，自 v1 台账恢复并按现状校正）

> 全部量化阈值来自 `docs/dev/budgets.json`（唯一真值源）。与 v1 的差异：`decode_p99` 3.0→**24.0**、`decode_p999` →**64.0**（budgets.json 现行值）；其余不变。

| 假设编号 | 维度 | 假设内容 | 影响的用例域 | 偏离时的修正策略 |
|---|---|---|---|---|
| `ASM-T-01` | 形态 | 被测对象是 fcitx5 进程内的**两个 addon**（ADR-0003/0004），端到端必须驱动真实 fcitx5 会话 | 全部屏上用例 | 若改 headless 引擎测试，上屏/UiFrame 断言退化为单元级，端到端卡转 `[不可执行]` |
| `ASM-T-02` | 形态 | 候选框完全自绘：**无 DOM / 无 A11y 树**；语义断言面 = `UiFrame` 镜像（`runtime://ui_frame`） | 视觉与交互用例 | 若未来引入 A11y 暴露（features.md `TASK-3.04.02`），可增语义树断言 |
| `ASM-T-03` | 硬件边界 | 本机 WSL2 + WSLg（`DISPLAY=:0`、Weston）：X11 档可测，Wayland 档不可测 | 平台相关用例 | X11 结论可移植，Wayland 结论不可 |
| `ASM-T-04` | 性能预算 | `key_to_present_p99=16ms`、`key_to_present_p50=4ms`、`key_to_present_p99_144hz=12ms`、`decode_p99=24ms`、`decode_p999=64ms`、`raster_p99=1.5ms`、`first_key_to_visible_p99=8ms`、`addon_load=120ms` | 延迟与帧率用例 | 阈值变更只改 `budgets.json`（+features.md 0.5.3 同步），用例只引键名 |
| `ASM-T-05` | 性能预算 | `idle=0.3%` 单核、`idle_redraw_count=0`、`idle_poll_timer_count=0` | 空闲占用用例 | 超限先查是否引入轮询定时器（0.4 规则 9） |
| `ASM-T-06` | 资源预算 | `ui_rss=18MB`、`plugin_rss=45MB`、`dict_mmap_rss=25MB`、`so_stripped=12MB`、`base_dict=20MB` | 内存与体积用例 | 内存断言必须是基线差分；体积用例在 `just package` 后测（release 禁 strip，ADR-0002） |
| `ASM-T-07` | 数据量级 | 仓库内测试以**合成词库**（`dict_inject`）为基准；`data/compiled/base.dict`（15.62MiB/348,972 词条）仅 smoke/quality 档使用 | 词库规模用例 | 测试保持封闭，不依赖未入库的大词库 |
| `ASM-T-08` | 硬件边界 | 本机不可验证：`wlr-layer-shell`/KWin/Mutter 档、真实亚克力模糊、多屏热插拔、8h 长稳 | Wayland 与长稳用例 | 必须标注「本机不可验证」并给出所需环境，不得当作已通过 |
| `ASM-T-09` | 并发模型 | 双线程物理隔离 + SPSC/eventfd，无异步运行时 | 并发与背压用例 | 引入异步运行时会直接违反 `BUDGET-NET-01` 族审计 |
| `ASM-T-10` | 数据量级 | 单会话 `raw ≤ 64` 字节；候选 ≤ 45（5 页 × 9）；`DEFAULT_BEAM_K=16`（≤32） | 边界与容错用例 | 超限行为必须命中冻结错误码（`decode/too-long` 等） |
| `ASM-T-11` | 测量纪律 | 预算数值只在**空闲机器**上采信（`purity` 门禁拒绝并行负载下的采样） | 全部性能用例 | 受污染采样作废重测，验收记录注明机器状态 |

---

## 2. 平台能力-任务覆盖追溯表（v3 增量平台工作）

> 平台能力分两类：**已交付能力**（v1 的 20 卡产出，§4 台账，不重复立项）与 **v3 增量能力**（下表 8 项，驱动 tests v3.0 套件执行）。

| 能力编号 | 所属分层 | 核心职责描述（绑定 test-architecture.md 选定框架） | 绑定任务节点 | 所在分片文件 | 关键路径 (CP) | 并行通道 |
|---|---|---|---|---|---|---|
| `MCP-T-10` | Tools/账本 | 证据账本 v3：`assertions.json` 增 `tier/dims/requires_screenshots/screenshots_expected/budget_keys` 字段；批次级 `checklist.json`/`index.md` 骨架 | `FEAT-TEST-P0.01.01` | `./features-test-phase1.md` | 是 | Track C 账本基建 |
| `MCP-R-10` | Resources | 环境门禁三态扩展：`待实现`（被测代码缺席，锚定 features-add TASK）与 `不可验证` 并列机读 | `FEAT-TEST-P0.01.02` | `./features-test-phase1.md` | 否 | Track C |
| `MCP-T-11` | Tools | 场景 schema v2：TOML 支持 `DecodeFlags` 注入、用户库 fixture、BIGRAM 缺席档、衰减时钟注入 | `FEAT-TEST-P0.02.01` | `./features-test-phase1.md` | 是 | Track A 平台内核 |
| `MCP-T-12` | Tools | `dict_inject` 多源 fixture：新词源（thuocl/huayu/合成）样例库 + staged 词库文件（热替换用例） | `FEAT-TEST-P0.02.02` | `./features-test-phase1.md` | 否 | Track A |
| `MCP-T-13` | Tools | 视觉基线差分：capture PNG 与 golden 基线的像素 diff 工具（光栅确定性回归） | `FEAT-TEST-P1.03.01` | `./features-test-phase2.md` | 否 | Track B 断言引擎 |
| `MCP-R-11` | Resources | `UiFrame` 镜像 schema 与 features-add 增量字段对齐（置顶标注、script 状态、纠错 annotation） | `FEAT-TEST-P1.04.01` | `./features-test-phase2.md` | 否 | Track B |
| `MCP-P-10` | Prompts/报告 | 批次报告生成器：`final-report.md` 渲染 + `results/index.json` 注册表 | `FEAT-TEST-P2.05.01` | `./features-test-phase3.md` | 否 | Track C |
| `MCP-P-11` | Prompts/CI | 分级门禁调度：`just check-e2e-smoke` / Acceptance / Stress 三档编排脚本 | `FEAT-TEST-P2.06.01` | `./features-test-phase3.md` | 是 | Track C |

### 2.1 编号规则与 DAG 校验

- 编号：`FEAT-TEST-[优先级].[拓扑簇序号].[任务序号]`；拓扑簇全局严格递增（`01` 账本与门禁、`02` 引擎与词库驱动、`03` 视觉断言、`04` 镜像对齐、`05` 报告、`06` CI 调度）；同簇任务完全解耦可并行。
- 依赖边（全部满足 dep < self，无环）：`P0.02.01 ← P0.01.01`（场景输出走账本 v3 schema）；`P1.03.01 ← P0.01.01`；`P1.04.01 ← P0.02.01`；`P2.05.01 ← P0.01.01`；`P2.06.01 ← P0.01.01, P1.03.01, P2.05.01`。
- **与 features-add 的边界**：`TASK-2.06.01`（冒烟 TSV 基线）已交付（features-add 纠偏 X-11），本表不再立项；用例集扩充随各功能卡 DoD 走。

### 2.2 关键路径与工期

```text
CP: P0.01.01（账本 v3，2.5d）→ P0.02.01（场景 v2，2d）→ P1.04.01（镜像对齐，1.5d）
    P0.01.01 → P2.05.01（报告，1.5d）→ P2.06.01（CI 调度，2d）
CP 总工期 ≈ 9.5 人天；三轨并行（A: P0.02.02; B: P1.03.01; C: P0.01.02）约 6 工作日
```

---

## 3. MCP 能力映射总纲（与 test-architecture.md §3 一致，执行面唯一）

| 原语族 | v1 已交付实现（保留） | v3 增量（本集群 8 卡） |
|---|---|---|
| Tools | `action_keyboard`/`action_interact`（XTEST）、`capture_viewport_screenshot`（XGetImage）、`reset_sandbox` | 场景 schema v2、多源 fixture、视觉基线差分 |
| Resources | `runtime://ui_frame`（镜像）、`runtime://env`（门禁）、`runtime://logs`、`runtime://memory_profile` | 门禁三态、镜像增量字段 |
| Prompts | 视觉判据（用例卡内）+ `heal`/`attribution`/`purity` | 批次报告、分级调度 |
| 明确不建 | — | MCP server 进程（反膨胀裁决，test-architecture §2.2-3） |

---

## 4. 已交付平台台账（v1 遗产，git `74afea9^` 可考）

v1 features-test 的 20 张平台任务卡（`FEAT-TEST-P0.01.01` ~ `FEAT-TEST-P1.05.07`）全部 `[x]`，产出即现今 `xtask/src/testd/` 全模块 + 18 场景 TOML + 211 例 TSV + 证据链 + 门禁族。v3.0 不重写其卡面，仅登记映射：

| v1 能力 | 交付物（Code Anchor） | GUARD |
|---|---|---|
| XTEST 键鼠注入 + FocusGuard | `testd/{input,keys,guard,x11}.rs` | GUARD-01 焦点零抢占 |
| XGetImage 截图 | `testd/{capture,capture_cli}.rs` | GUARD-05 |
| UiFrame 镜像 | `testd/{mirror_cli,uiframe}.rs`、`ime-diag/src/uiframe/` | GUARD-05 |
| 上屏读回探针客户端 | `testd/commit_readback.rs`、`xtask/testd/client/rspinyin_probe_client.c` | GUARD-02 隐私（transcript 0600） |
| 进程内引擎重放 + 场景套件 | `testd/{engine,suite,sandbox}.rs`、`tests/fixtures/scenarios/*.toml`（18） | GUARD-03 |
| TSV 冒烟基线 | `testd/smoke.rs`、`tests/smoke/cases.tsv`（211）、`justfile smoke:` | `just ci` 层 |
| 证据链 + 自愈 + 归因 | `testd/{evidence,heal,attribution,review}.rs` | GUARD-04 |
| 环境门禁 + 纯净度 | `testd/{env,env_gate,purity}.rs` | GUARD-06/07 |
| 采样与预算裁决 | `testd/{framerate,memory,budget_gate}.rs`、`xtask/src/budget.rs` | GUARD-07 |
| 词库注入 | `testd/dict_inject.rs` | — |
| Slint 规格度量 | `testd/ui_metrics.rs` | — |
| 矩阵机械校验 | 旧 `P1.05.07`（并入 v3 `P0.01.02` 的三态判定与 tests.md 自检） | GUARD-08 |

> 台账与 tests v2.0 的引用关系已随 v3.0 重建切断；v2.0 套件（382 条）的处置记录见 [test-architecture.md](test-architecture.md) §6。
