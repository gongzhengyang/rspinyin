# opt-perf / phase-3.md — P2 任务卡（极致渲染与微架构调优）

> 文档版本: v1.0 ｜ 系统形态: **Desktop GUI** ｜ 架构基线: Rust 2024 workspace（7 crates + xtask），双线程物理隔离 + SPSC 有界通道 + `eventfd`，**无 async 运行时** ｜ 关联 ADR: [../adr/0000-upstream-decisions.md](../adr/0000-upstream-decisions.md) ｜ 最后同步 Commit: `ee0dbfb` ｜ 维护约定: 代码演进后必须回写热点清单状态与追溯矩阵
>
> **本分片是 [../opt-perf.md](../opt-perf.md) 的 P2 展开。** 主文档承载《系统设计假设清单》《性能热点总清单》《WBS 任务覆盖追溯表》《关键路径与并行通道汇总》与全部 P0 任务卡；P1 任务卡见 [./phase-2.md](./phase-2.md)。热点编号、假设编号与阈值一律以主文档为准，**本分片不新增、不改名任何编号**。

**适用约束（与主文档 `ASM-P10` 同一份）**：无 async 运行时；宿主线程只做 `key → decode → build UiFrame → post`；跨线程只用有界 SPSC + `eventfd`；禁止轮询定时器；`unsafe` 仅限 `crates/ime-fcitx5/src/ffi/**` 与 `crates/ime-dict/src/mmap.rs`；`ime-types` 为冻结契约（变更需 ADR）；禁止新增未评审依赖；禁止任何网络能力。子 agent 执行时**只写代码，不跑 cargo 命令**（AGENTS.md §6.2）。

**优先级定位**：P2 = 极致渲染与微架构调优。本层任务**全部是门禁与可观测性建设**，而非代码改写——因为 P0/P1 已经把可识别的结构性浪费处理完毕，剩下的工作是把"不会退化"变成 CI 可执行的断言。这是本项目与通用性能调优最大的差异：AGENTS.md §2 明确「A regression past a budget is a gate failure, not a warning」，没有门禁的优化成果会在下一次重构中无声流失。

**前置说明（据实登记）**：P2 的四张卡全部依赖 `PERF-P1.04.01`，而 `PERF-P1.04.01` 依赖 `PERF-P0.04.01`。因此 P2 的启动前提是**测量基建已就位**。在此之前认领 P2 任务卡会产生"写了门禁却无数据可断言"的空转，**不得提前启动**。

---

### 任务 ID：PERF-P2.01.01 解码分配次数常量断言（分配预算门禁）

- **基本属性**：
  - 绑定热点编号：`HOT-01`、`HOT-02`、`HOT-03`、`HOT-09`（分配计数口径的汇总断言）
  - 优先级与复杂度：`P2 | 中 | 预估工时: 2 人天`
  - 前置依赖：`PERF-P1.01.01`
  - 关键路径：`CP: 否`
  - 并行通道：`Track A 数据与并发引擎`
  - 代码落地锚点 (Code Anchor)：`crates/ime-core/benches/decode.rs`、`crates/ime-core/benches/input.rs`、`crates/ime-diag/src/probe/alloc.rs`、`xtask/src/budget.rs`
  - 当前状态：`[ ] 待优化`

- **优化定位与机理剖析**：
  - **现有能力缺口**：`PERF-P0.01.01`、`PERF-P0.01.02`、`PERF-P1.01.01` 三张卡都以"分配次数"作为量化目标，但该指标目前**只存在于各卡的 DoD 文字里**，没有一个统一的可执行门禁。一个后续重构把 `DecodeScratch` 的复用改回每次新建，会让分配次数从 8 涨回 200，而**所有既有测试仍然全绿**——因为功能行为没有变化。
  - **为什么这是 P2 而非 P0**：分配次数是**实现细节**，把它写成门禁需要先有稳定的测量基建（`PERF-P0.04.01` 的探针 + `PERF-P1.04.01` 的分配器包装）。在基建到位前写门禁只会得到一个不可靠的数字。
  - **量化优化目标**：稳态解码的分配次数成为一个**常量断言**——`decode_into` 连续 N 次调用的分配次数 ≤ 常量 K，且该常量写入 `docs/dev/budgets.json` 作为回归阈值。任一 PR 让它增长即 CI 失败。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：把分配计数做成 bench harness 的一等公民。**

  ```rust
  // crates/ime-core/benches/alloc_budget.rs （新 bench target）

  //! The allocation budget of the decode path, asserted rather than measured.
  //!
  //! Criterion answers "how long"; this target answers "how many times", which is
  //! the metric the decode path's design is stated in. A criterion regression of a
  //! few percent is noise; an allocation count going from 8 to 200 is a design
  //! regression that no timing threshold on a fast machine would catch.
  //!
  //! The assertion is a hard failure, not a report: `cargo bench` exits non-zero
  //! when the count exceeds the budget, so the gate is the same command the
  //! project already runs for its other budgets.

  /// The steady-state allocation budget of one decode.
  ///
  /// The value is what `PERF-P0.01.01` + `PERF-P0.01.02` + `PERF-P1.01.01` leave
  /// behind: the lattice's edge vector, the candidate vector's first growth, and
  /// nothing else. It is a budget, not a measurement -- raising it is a decision
  /// that has to be argued in a commit message, which is the whole point.
  const DECODE_ALLOC_BUDGET: usize = 8;

  /// The steady-state allocation budget of one keystroke through a session.
  ///
  /// Larger than the decode budget because a frame owns its candidate texts and is
  /// handed to another thread: `Box<UiFrame>` and one `String` per visible
  /// candidate are the price of the frame being a snapshot rather than a borrow.
  const KEYSTROKE_ALLOC_BUDGET: usize = 16;
  ```

  **第二步：断言点覆盖两条路径。**

  | 断言 | 输入 | 预算 | 覆盖的热点 |
  |---|---|---|---|
  | `decode_into` 稳态 | 12 音节（`decode.rs` 的 `INPUT`） | `DECODE_ALLOC_BUDGET` | `HOT-01`、`HOT-02`、`HOT-03`、`HOT-09` |
  | `Session::step(Key)` 稳态 | 连续 10 次 `InputChar` | `KEYSTROKE_ALLOC_BUDGET` | `HOT-04`、`HOT-08` |
  | `DictLm::unigram` | 10^5 词表内的命中与未命中 | 0 | `HOT-07` |
  | `UserDb::freq`（水合态） | 1000 次查询 | 0 | `HOT-06` |

  **第三步：阈值入册。** `DECODE_ALLOC_BUDGET` 与 `KEYSTROKE_ALLOC_BUDGET` 必须写入 `docs/dev/budgets.json`（新增 `alloc_count` 段），并同步 `features.md` 0.5.3 的表格行。**这是主 agent 的动作**；子 agent 在交付报告中列出所需行内容（建议新增 `BUDGET-ALLOC-01`：单次解码堆分配次数 ≤ 8，来源任务 `TASK-1.02.07`）。

  **边界契约**：
  - 通道类型语义：不涉及。
  - 数据 Payload：无新增。
  - 状态机跃迁：分配探针的 `arm`/`disarm` 必须在断言前后成对，且**断言失败时也必须 `disarm`**（否则后续 bench 的计数被污染）。用 RAII guard 实现，不靠手工配对。
  - 错误码：沿用 `budget/regression`。
  - **隐私约束**：探针只记录次数与字节数，不记录分配内容。

  **并发与死锁风险**：与 `PERF-P1.04.01` 同一套约束——探针在分配器内部被调用，**只能**用原子操作，**不得**取锁。bench harness 是单线程驱动的，但探针实现本身必须是多线程安全的。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 新建 `crates/ime-core/benches/alloc_budget.rs`，实现四个断言点；所需 `Cargo.toml` 的 `[[bench]]` 段在交付报告中列出，由主 agent 落地。
  2. 用 `PERF-P1.04.01` 的 `AllocProbe` 作为计数源；断言失败时 `panic!`（bench target 允许 `panic`，不受 §3.2 的 `clippy::panic` 约束——但需在文件头注明该豁免理由）。
  3. 在交付报告中列出 `budgets.json` 与 `features.md` 0.5.3 需新增的行，由主 agent 登记。
  4. 回归：`cargo bench -p ime-core --bench alloc_budget` 全通过。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] `crates/ime-core/benches/alloc_budget.rs` 存在，四个断言点全部可运行
  - [ ] 断言在预算内通过；把 `DECODE_ALLOC_BUDGET` 临时调低 1 会让它失败（**正反两路都验证**，证明断言真的在断言）
  - [ ] 阈值已入册 `budgets.json` 与 `features.md` 0.5.3（主 agent 执行）
  - [ ] `disarm` 用 RAII guard 实现（代码审查项：断言路径中不存在手工的 `disarm` 调用）
  - [ ] `cargo nextest run -p ime-core` 全绿；`cargo test -p ime-core --doc` 全绿
  - [ ] 探针实现中不出现互斥锁（代码审查项）

---

### 任务 ID：PERF-P2.03.01 Slint 局部重绘边界调优

- **基本属性**：
  - 绑定热点编号：`HOT-14`（残余）、`HOT-15`（残余）
  - 优先级与复杂度：`P2 | 高 | 预估工时: 3 人天`
  - 前置依赖：`PERF-P1.03.01`
  - 关键路径：`CP: 否`
  - 并行通道：`Track B 渲染与视图管线`
  - 代码落地锚点 (Code Anchor)：`crates/ime-ui/ui/candidate.slint`、`crates/ime-ui/ui/theme.slint`、`crates/ime-ui/src/renderer.rs`、`crates/ime-ui/src/renderer/raster.rs`、`crates/ime-ui/benches/frame.rs`
  - 当前状态：`[ ] 待优化`

- **优化定位与机理剖析**：
  - **现状（据实登记）**：`crates/ime-ui/ui/candidate.slint` 的 `CandidateGrid`（`:185-205`）目前**只绘制占位矩形**——`for row in root.rows` / `for column in min(...)` 循环里只有一个 `Rectangle { background: transparent; }`。模块注释（`:182-184`）明确写着「the cells, the number labels and the five states land with the grid task」。也就是说，**当前没有任何候选文本被光栅化**，`HOT-14` 的实测成本必然远低于真实产品状态。
  - **因此本卡的定位不是"优化现状"，而是"在候选网格落地后立刻建立边界"**：Slint 的软件光栅按元素边界推导重绘区域（`renderer.rs:305` 的 `PhysicalRegion` 是它的输出），而候选文本、序号标签、高亮背景是三类不同的重绘诱因。**没有实测数据就调优是臆测**，本卡的全部价值在于用 `frame/animate_steady` 与新增的 `frame/grid_full` 两个基准，给出"候选网格落地后"的真实成本，并据此判断是否需要调整元素层级或引入缓存层。
  - **量化优化目标**：在候选网格（9 候选 × 序号 + 文本 + 高亮态）落地后，`frame/grid_full` 的全窗口 P99 ≤ `BUDGET-LAT-03`（1.5ms）；高亮 A→B 移动的稳态帧 P99 ≤ 该值的 30%；若实测超标，本卡产出**具体的层级调整方案**（如把高亮背景提升为独立图层以隔离其重绘）而非笼统的"优化渲染"。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：建立"候选网格已落地"的基准场景。**

  ```rust
  // crates/ime-ui/benches/frame.rs

  /// The grid the benchmark rasterizes: nine candidates with labels and a highlight.
  ///
  /// The window is 600x140 logical pixels at scale 2.0, which is the size
  /// `BUDGET-LAT-03` is stated for. The scene is built through the same Slint
  /// component the product uses -- a benchmark against a hand-built scene would
  /// measure the benchmark rather than the window.
  fn grid_scene(candidates: usize) -> CandidateWindow { /* ... */ }
  ```

  **第二步（条件执行）：按实测结果调整元素层级。** 若 `frame/animate_steady` 的高亮移动帧超过预算的 30%，调整方向按以下优先级：

  1. **高亮背景与文本分离**：把高亮背景 `Rectangle` 与候选文本 `Text` 放在同一层级但在 Z 序上分离，使高亮移动只失效背景层。Slint 的软件光栅不支持真正的图层合成，因此该手段的收益需实测确认，**不得假设**。
  2. **`clip: true` 的边界收紧**：给候选网格加 `clip: true` 会引入一个裁剪区域，可能**增加**而非减少重绘——同样需实测确认。
  3. **禁用不必要的 `drop-shadow`/`border-radius`**：`CandidateShadow` 与容器圆角（`candidate.slint:236-250`）是每帧重绘的固定成本；若实测显示其占比高，则评估把阴影预烘焙为一张位图缓存。**注意**：这会与 `HOT-16`（像素暂存池化）的收益叠加，须在两者都完成后重测。

  > **强行排他**：本卡**不得**在没有 `frame/grid_full` 与 `frame/animate_steady` 两个基准数据的情况下提交任何 `.slint` 改动。这是本卡唯一的硬性流程约束，也是它与"凭感觉调优"的分界线。

  **边界契约**：
  - 渲染层契约：`SlintWindowAdapter`（`renderer.rs:162`）是 `pub(crate)`，不得导出；`.slint` 的改动不得引入任何 `slint::` 类型到 `ime-ui` 的公共 API（`OB-4`，由 `scripts/check-slint-leak.sh` 强制）。
  - `UiFrame` 契约：本卡不改 `UiFrame` 结构。
  - 错误码：不新增。

  **并发与死锁风险**：无。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在 `crates/ime-ui/benches/frame.rs` 追加 `frame/grid_full` 与 `frame/grid_highlight_move`（依赖候选网格已落地；若尚未落地，则先建一个等价的测试场景并在报告中标注"非产品场景"）。
  2. 采集基线数据并回写主文档 §6。
  3. 若超标，按第二步的优先级逐项试验，**每项试验单独测量并记录**，不做多项合并提交。
  4. 在 `crates/ime-ui/src/renderer/tests.rs` 追加断言：高亮移动帧的 damage 面积不超过候选区面积的 30%（与 `PERF-P1.03.01` 的断言互补——前者是"列表处理"，本卡是"元素层级"）。
  5. 回归：`cargo nextest run -p ime-ui`；`bash scripts/check-slint-leak.sh`。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] `frame/grid_full` 与 `frame/grid_highlight_move` 基准存在，基线数据已回写主文档 §6
  - [ ] `frame/grid_full` 的 P99 ≤ 1.5ms（`BUDGET-LAT-03`）；若超标，本卡产出具体的层级调整方案并逐项测量记录
  - [ ] 每一项 `.slint` 改动都有对应的前后测量数据（**无数据不得提交**）
  - [ ] `bash scripts/check-slint-leak.sh` 通过（`slint::` 未泄漏到 `ime-ui` 公共 API）
  - [ ] `cargo nextest run -p ime-ui` 全绿；`cargo test -p ime-ui --doc` 全绿

---

### 任务 ID：PERF-P2.04.01 长稳 RSS 漂移与 8 小时 soak 门禁

- **基本属性**：
  - 绑定热点编号：`HOT-02`、`HOT-06`、`HOT-11`、`HOT-16`（长稳口径的汇总断言）
  - 优先级与复杂度：`P2 | 中 | 预估工时: 3 人天`
  - 前置依赖：`PERF-P1.04.01`
  - 关键路径：`CP: 否`
  - 并行通道：`Track C 基准·监控·基建`
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/sandbox.rs`、`xtask/src/budget.rs`、`docs/dev/budgets.json`、`crates/ime-diag/src/probe.rs`
  - 当前状态：`[ ] 待优化`

- **优化定位与机理剖析**：
  - **现有能力缺口**：`BUDGET-ROB-01`（`features.md` 0.5.3）规定「连续 8 小时输入无崩溃、无内存增长（RSS 漂移 ≤ 2MB），100% 通过」，来源任务 `TASK-3.08.01`。`docs/dev/budgets.json` 的 `robustness` 段已有 `soak_hours = 8.0` 与 `rss_drift_mb = 2.0`，但**没有执行者**。`xtask/src/testd/` 已有 sandbox 与输入注入能力（`sandbox.rs`、`keys.rs`、`input.rs`），是可复用的地基。
  - **本卡治理的具体风险**：`HOT-02`（每键 40KB 分配）与 `HOT-16`（scratch 只增不减）是典型的"分配器碎片型"泄漏——它们不会表现为 RSS 单调上升，而表现为**RSS 平台期抬升**：分配器从内核拿到的高水位线在峰值后不归还。这类问题在短测中完全不可见。
  - **量化优化目标**：产出一次可复现的 8 小时 soak 运行，记录 RSS 时间序列与漂移量；漂移 ≤ 2MB 判通过。若超标，本卡产出**归因报告**（哪个热点贡献了多少漂移），而不是直接改阈值。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：soak 驱动器。**

  ```rust
  // xtask/src/soak.rs （新文件）

  //! The long-run harness behind `BUDGET-ROB-01`.
  //!
  //! Eight hours of continuous input, with the resident set sampled on a fixed
  //! cadence, is the only way to see the failure this budget exists for: not a
  //! crash, and not a leak in the textbook sense, but an allocator high-water mark
  //! that ratchets up and never comes back down. A two-minute benchmark cannot see
  //! it and a unit test cannot express it.
  //!
  //! The harness drives the same `Session` the plugin drives, through the same
  //! event stream, with an injected clock -- so a "typing session" is a loop over
  //! generated key sequences rather than a real keyboard, and the run is
  //! reproducible from its seed.
  ```

  **第二步：RSS 采样与漂移判定。**

  | 指标 | 采集方式 | 判定 |
  |---|---|---|
  | `VmRSS` | `/proc/self/status` | 末 1 小时的 P50 相对首 1 小时的 P50，漂移 ≤ `rss_drift_mb`（2MB） |
  | `Anonymous` + `Private_Dirty` | `/proc/self/smaps_rollup` | 同上，用于区分"堆高水位"与"mmap 页缓存" |
  | 崩溃/panic | 进程退出码 + `ime-diag` 的崩溃记录目录 | 必须为 0 |
  | 分配次数 | `PERF-P1.04.01` 的分配器探针 | 按小时分段，各段速率应平稳（无单调增长） |

  > **判定口径的诚实说明**：`BUDGET-ROB-01` 的原文是「无内存增长（RSS 漂移 ≤ 2MB）」，本卡把它操作化为"末段 P50 相对首段 P50"。**若主 agent 判定该操作化偏离原文，必须以 `features.md` 为准修正本卡**——`features.md` 在技术事项上优先（AGENTS.md 前言）。

  **第三步：门禁化。** soak 是 8 小时任务，**不得**放进每次提交的 CI。它作为 nightly 或 release 前置门禁，由 `just` 目标承载：

  ```make
  # justfile
  # Eight-hour robustness run (BUDGET-ROB-01). Not part of `just ci`: it takes
  # eight hours and the budget it asserts is about sustained behaviour, not about
  # a commit.
  soak:
      cargo run --release -p xtask -- soak --hours 8 --report soak.json
  ```

  **边界契约**：
  - 通道类型语义：不涉及。
  - 数据 Payload：`soak.json`（RSS 时间序列 + 判定结果），由 `serde_json` 序列化。
  - 状态机跃迁：soak 驱动器必须显式走完 `Idle → Composing → Committing → Idle` 的完整循环，**不得**只测 `Composing` 稳态——泄漏最可能出现在状态跃迁的清理路径上（`machine.rs:402` 的 `clear_input`、`:426` 的 `clear_session`）。
  - 错误码：不新增；崩溃由 `ime-diag` 的既有崩溃记录承载。
  - **隐私约束**：soak 驱动器生成的输入是**合成的**，不得使用真实用户词库或真实输入历史；报告中的分配计数不含内容。

  **并发与死锁风险**：soak 驱动 `Session` 是单线程的（`ASM-11`：解码按会话串行）。若要覆盖双线程路径，须额外驱动 `CommandChannels` + `UiLoop`（用 `MockBackend`），并在报告中区分两段的 RSS 曲线——**不得**把两个进程的曲线混在一起判定。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 新建 `xtask/src/soak.rs`，实现会话驱动（合成按键流 + 注入时钟 + 完整状态循环）。
  2. 实现 RSS 采样与判定；报告写入 JSON。
  3. 在 `justfile` 增加 `soak` 目标（**`justfile` 由主 agent 修改**，子 agent 在交付报告中列出所需片段）。
  4. 跑一次完整 8 小时运行；若超标，产出归因报告（按 `PERF-P1.04.01` 的分配探针分段定位）。
  5. 回归：`cargo nextest run -p xtask`。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] `xtask soak --hours 8 --report soak.json` 可运行并产出完整报告
  - [ ] 一次完整的 8 小时运行已完成，RSS 漂移 ≤ 2MB（`BUDGET-ROB-01`）；实测值回写主文档 §6
  - [ ] 崩溃/panic 次数 = 0
  - [ ] 状态循环覆盖断言：驱动器走完 `Idle → Composing → Committing → Idle` 至少 N 次（N 写入报告）
  - [ ] 若漂移超标，归因报告已产出，且**未**修改任何阈值
  - [ ] `justfile` 的 `soak` 目标片段已交付主 agent
  - [ ] `cargo nextest run -p xtask` 全绿；`cargo test -p xtask --doc` 全绿

---

### 任务 ID：PERF-P2.04.02 性能预算 CI 门禁（回归阈值与报告）

- **基本属性**：
  - 绑定热点编号：全部 17 条（汇总门禁）
  - 优先级与复杂度：`P2 | 高 | 预估工时: 3 人天`
  - 前置依赖：`PERF-P1.04.01`
  - 关键路径：`CP: 是`（**关键路径终点**）
  - 并行通道：`Track C 基准·监控·基建`
  - 代码落地锚点 (Code Anchor)：`xtask/src/budget.rs`、`xtask/src/main.rs`、`justfile`、`.github/workflows/`、`docs/dev/budgets.json`、`scripts/`
  - 当前状态：`[ ] 待优化`

- **优化定位与机理剖析**：
  - **现有能力缺口**：`AGENTS.md` §2 规定「**Budget assertions**: thresholds live in `features.md` 0.5.3 and `docs/dev/budgets.json`. A regression past a budget is a gate failure, not a warning.」而现状是：`xtask budget --validate` 只校验**文档之间**的一致性（`budget.rs:8-13`），criterion 的输出从未与阈值比较（`budget.rs:13` 明写该动作是 separate action 且尚不存在）。`justfile:118-121` 的 `bench` 目标只跑 `cargo bench`，`justfile:122-129` 的 CI 快速档在无 criterion target 时**刻意保持绿色**（注释明写「a bench job that cannot run is reported instead of failing for the wrong reason」）。因此**没有任何自动机制会在性能回归时失败**。
  - **本卡是整条关键路径的终点**：它把 `PERF-P0.04.01` 的探针、`PERF-P1.04.01` 的分配测量、`PERF-P2.01.01` 的分配预算、`PERF-P2.04.01` 的 soak 全部接进 CI，使"优化成果不退化"成为机器保证。
  - **量化优化目标**：`just ci` 中包含一个**可执行的**性能门禁；任一预算回归使 CI 失败并给出"哪个预算、基线值、实测值、超出比例"四元组；门禁自身的误报率可通过 3 次连续运行验证（同一 commit 三次运行结论一致）。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：criterion 输出 → 阈值比较。**

  ```rust
  // xtask/src/budget/measure.rs （新文件）

  //! Comparing a benchmark run against the thresholds.
  //!
  //! `--validate` answers "do the document and the spec table agree with each
  //! other"; this answers "does the build agree with them". They are separate
  //! commands because the first must run on every commit and takes milliseconds,
  //! while the second needs a benchmark run long enough to be meaningful.
  //!
  //! # Which number is compared
  //!
  //! Criterion reports a distribution, and a budget is stated at a percentile
  //! (`BUDGET-LAT-02` is a P99 and a P999, `BUDGET-LAT-01` a P50 and a P99). The
  //! comparison therefore reads the matching percentile out of criterion's
  //! `estimates.json` rather than its mean, because a mean hides exactly the tail
  //! the budget exists for.
  //!
  //! # Baselines
  //!
  //! A regression is a change relative to a *baseline*, not to an absolute
  //! threshold: the development machine is WSL2 with virtualization overhead, and
  //! `features.md` 0.5.5 already records that its numbers are a relative regression
  //! baseline rather than target-hardware figures. The baseline is therefore a
  //! committed file, and the gate compares against it.
  ```

  **第二步：预算 → 基准 → 百分位的绑定表。**

  | 预算 | 基准 target / 用例 | 比较的百分位 | 阈值 | 基线文件 |
  |---|---|---|---|---|
  | `BUDGET-LAT-02` | `ime-core/decode` → `decode/viterbi_into` | P99 / P999 | 3ms / 8ms | 是 |
  | `BUDGET-LAT-02` | `ime-core/alloc_budget` → `decode_allocations` | 计数 | ≤ 8 | 是 |
  | `BUDGET-LAT-02` | `ime-core/alloc_budget` → `keystroke_allocations` | 计数 | ≤ 16 | 是 |
  | `BUDGET-LAT-03` | `ime-ui/frame` → `frame/grid_full` | P99 | 1.5ms | 是 |
  | `BUDGET-LAT-03` | `ime-ui/frame` → `frame/animate_steady` | P99 | 1.5ms | 是 |
  | `BUDGET-LAT-01` | `xtask probe` 报告 | P50 / P99 | 4ms / 16ms | 是 |
  | `BUDGET-LAT-05` | `addon.rs:455` 的 `lifecycle/init` 日志 | 单次 | 120ms | 否（单次事件） |
  | `BUDGET-MEM-01/02/03` | `xtask budget --measure` 报告 | 单次 | 18/45/25MB | 否 |
  | `BUDGET-ROB-01` | `xtask soak` 报告 | 漂移 | 2MB | 否 |
  | `BUDGET-SIZE-01/02` | `ls -l` + `size` | 单次 | 12MB / 20MB | 否 |

  **第三步：CI 接线。** `justfile:137` 的 `ci` 目标增加 `check-perf`（快速档：只跑分配计数断言与 `--validate`）；完整档（跑 criterion + `--measure`）作为 nightly。

  ```make
  # Performance budgets against the authoritative table (0.5.3).
  check-budget:
      cargo run --quiet -p xtask -- budget --validate

  # Allocation budgets. Fast enough for every commit: the counts are asserted by a
  # bench target that runs in seconds, unlike the timing benchmarks below.
  check-alloc:
      cargo bench -p ime-core --bench alloc_budget

  # Timing budgets against the committed baseline. Slower, and its numbers are
  # only comparable on a quiet machine -- see features.md 0.5.5.
  check-perf:
      cargo run --quiet -p xtask -- budget --measure target/criterion
  ```

  **边界契约**：
  - 通道类型语义：不涉及。
  - 数据 Payload：基线文件（`docs/dev/perf-baseline.json`，新增）与报告（`target/perf-report.json`）。
  - 状态机跃迁：无。
  - 错误码：沿用 `budget/regression`；报告须含 `budget_id` / `baseline` / `measured` / `excess_ratio` 四个字段。
  - **`.github/workflows/` 与 `justfile` 由主 agent 修改**（AGENTS.md §6.4：子 agent 不触碰 `Cargo.toml`、`Cargo.lock` 与模块根文件；CI 配置与 justfile 同属工程骨架）。子 agent 在交付报告中列出所需片段。

  **并发与死锁风险**：无。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 新建 `xtask/src/budget/measure.rs`，实现 criterion `estimates.json` 的读取与百分位比较；在 `xtask/src/main.rs` 挂载子命令（主 agent 执行）。
  2. 产出并提交基线文件 `docs/dev/perf-baseline.json`（首次基线必须在**干净机器 + 无其他负载**下采集，并在文件中记录采集环境）。
  3. 实现报告输出与失败退出码。
  4. 在交付报告中列出 `justfile` 的 `check-alloc` / `check-perf` 片段与 `.github/workflows/` 的 nightly 接线方案。
  5. 验证门禁有效性：手工把某个基线值调低 10%，确认 CI 失败且报告四元组正确（**正反两路都验证**）。
  6. 误报率验证：同一 commit 连续跑 3 次，结论一致。
  7. 回归：`cargo nextest run -p xtask`。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] `xtask budget --measure <criterion-dir>` 可运行，报告含 `budget_id` / `baseline` / `measured` / `excess_ratio` 四字段
  - [ ] 基线文件 `docs/dev/perf-baseline.json` 已提交，且记录了采集环境（机器 / 日期 / `features.md` 0.5.5 的档位）
  - [ ] **门禁有效性正反两路验证**：基线调低 10% 后 CI 失败；恢复后 CI 通过
  - [ ] **误报率验证**：同一 commit 连续 3 次运行结论一致
  - [ ] `justfile` 的 `check-alloc` / `check-perf` 片段与 CI 接线方案已交付主 agent
  - [ ] `docs/dev/budgets.json` 的每一个 `latency_ms` 键都至少绑定一个基准用例或一个探针报告（**双向覆盖断言**：既无未绑定的预算，也无未绑定的基准）
  - [ ] `cargo nextest run -p xtask` 全绿；`cargo test -p xtask --doc` 全绿

---

## 附录 A：P2 完成后的门禁全景

| 门禁 | 命令 | 频率 | 断言对象 |
|---|---|---|---|
| 文档一致性 | `cargo run -p xtask -- budget --validate` | 每次提交 | `features.md` 0.5.3 ↔ `budgets.json` |
| 分配预算 | `cargo bench -p ime-core --bench alloc_budget` | 每次提交 | `BUDGET-ALLOC-01`、`HOT-01/02/03/04/06/07/08/09` |
| 时序预算 | `cargo run -p xtask -- budget --measure target/criterion` | nightly | `BUDGET-LAT-01/02/03`、`HOT-10`、`HOT-14/15` |
| 内存预算 | `cargo run -p xtask -- budget --measure` + `smaps_rollup` | nightly | `BUDGET-MEM-01/02/03` |
| 体积预算 | `cargo run -p xtask -- budget --measure` | release | `BUDGET-SIZE-01/02` |
| 长稳预算 | `cargo run --release -p xtask -- soak --hours 8` | release | `BUDGET-ROB-01`、`HOT-02/06/11/16` |
| 宿主线程探针 | `cargo run -p xtask -- probe --report` | 手工 / nightly | `BUDGET-LAT-01` 的 P50/P99 |

## 附录 B：P2 之后仍然存在的已知边界（据实登记，不承诺修复）

| 边界 | 依据 | 影响 |
|---|---|---|
| `Lexicon::lookup` 无 `limit` 参数，最多物化 `MAX_WORDS_PER_KEY = 32` 个词而调用方只取 8 | 冻结契约，`crates/ime-types/src/lexicon.rs:38` | 多音节键无影响；单音节高频键（如 `shi`）仍有一次堆分配。彻底修复需 ADR |
| `UiFrame` 是拥有型快照，每帧至少一次 `Box` + 一次候选 `Vec` 分配 | 冻结契约，`crates/ime-types/src/ui.rs:48-64` | 差量帧（`Patch`）可消除，但需 ADR 且会改变 UI 线程的帧应用逻辑 |
| 语言模型的 bigram 在 v1 无数据源 | 容器格式，`crates/ime-dict/src/format/mod.rs:144-145`（`Bigram` 段保留且为空） | `bigram` 恒返回未命中值；Phase 3 的 bigram 段落地后由 `PERF-P1.01.01` 的实现自动受益 |
| 渲染链路的端到端集成（`UiSurface` 生产实现、`on_key_event` 接线）尚未落地 | `crates/ime-fcitx5/src/addon.rs:373`、`crates/ime-fcitx5/src/ffi/abi/engine.rs:50` | `BUDGET-LAT-01/03/04` 在接线前只能断言分段（`key_to_post`），不能断言端到端 |
| 开发机为 WSL2，数值仅作相对回归基线 | `features.md` 0.5.5 | 全部绝对阈值需在裸机复核 |

---

*本分片由 `dev-opt-perf` 技能生成，是 [../opt-perf.md](../opt-perf.md) 的 P2 展开。权威阈值以 `docs/dev/features.md` 0.5.3 为准。*
