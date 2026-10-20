# dev-features-test — Phase 3 平台任务分片（P2 账本与 CI 基建）

> 所属主控文档: [./features-test.md](features-test.md) ｜ 关联架构规范: [./test-architecture.md](test-architecture.md) ｜ 阶段定位: Phase 3（P2 报告生成与分级门禁）｜ 包含任务区间: FEAT-TEST-P2.05.01 ~ FEAT-TEST-P2.06.01（2 卡，预估 3.5 人天）
>
> 逻辑自检: 已通过（七维基线与正则对账闭环）

---

#### FEAT-TEST-P2.05.01 批次报告生成器（final-report.md 与 results/index.json）

- **基本属性**：
  - 绑定平台能力：`MCP-P-10` ｜ 底层驱动框架：`xtask testd`（`suite.rs` + `evidence.rs` 消费端）
  - 优先级与复杂度：`P2 账本与CI基建` ｜ 低 ｜ 预估工时: 1.5 人天
  - 前置依赖：`FEAT-TEST-P0.01.01`（消费账本 v3 的批次 checklist）
  - 关键路径：`CP: 否`
  - 并行通道：`Track C 基建·CI`
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/evidence.rs`（渲染函数）、`xtask/src/testd/suite.rs`（收尾调用）
  - 任务状态：`[ ] 待开始`
- **MCP 能力映射与协议报文契约**：批次收尾产物补全：

```rust
/// The batch-level report the executor and humans read first. It renders
/// from the checklist the ledger already wrote -- the generator owns no
/// state of its own, so a report can always be regenerated from the run
/// directory.
pub struct FinalReport {
    /// case id -> { tier, dims, verdict: Pass|Fail|PendingImpl|Unverifiable }
    pub verdicts: BTreeMap<String, CaseVerdict>,
    /// Names-only list of budget keys any failing case touched.
    pub budget_violations: Vec<String>,
    /// Honest blockers: isolated defects with their trace paths.
    pub blockers: Vec<Blocker>,
}

/// Registers the run in `./results/index.json` (append; the file is the
/// cross-batch registry `dev-check` resumes from).
pub fn register_run(index_path: &Path, entry: RunEntry) -> Result<()>;
```

- **实现细节**：渲染纯函数（checklist JSON 进 → markdown 出）；`blockers` 沿用旧套件的隔离缺陷表述纪律（如 v2.0 的 depth-24 黑屏记录格式）；`results/index.json` 追加写 + 损坏容忍（坏行跳过并告警，不整体重写）。报告语言中文（项目文档规范）。
- **逐步落地实施步骤**：
  1. `[步骤 1]`：`FinalReport` 渲染器 + 单测（三种 verdict、budget 违例归并、空批次）。
  2. `[步骤 2]`：`results/index.json` 追加注册 + 容错读取。
  3. `[步骤 3]`：`testd-suite` 收尾接线；对既有 run 目录（run-20261006-034915）做一次只读重渲染冒烟。
- **验收标准 (DoD)**：
  - **[单元测试闭环]**：`cargo nextest run -p xtask` 全绿。
  - **[物理产物验证]**：跑冒烟集后 `<RUN>/final-report.md` 与 `./results/index.json` 真实生成、非空、格式稳定。
  - **[性能开销基线]**：渲染 ≤ 100ms/批次；只读既有证据，无额外采样开销。

---

#### FEAT-TEST-P2.06.01 分级门禁调度（just check-e2e-smoke / acceptance / stress）

- **基本属性**：
  - 绑定平台能力：`MCP-P-11` ｜ 底层驱动框架：`justfile` + `xtask testd-suite`（现有编排面）
  - 优先级与复杂度：`P2 账本与CI基建` ｜ 中 ｜ 预估工时: 2 人天
  - 前置依赖：`FEAT-TEST-P0.01.01`、`FEAT-TEST-P1.03.01`、`FEAT-TEST-P2.05.01`（三依赖编号均小于自身）
  - 关键路径：`CP: 是`（收口）
  - 并行通道：`Track C 基建·CI`
  - 代码落地锚点 (Code Anchor)：`justfile`（新档位）、`xtask/src/testd/suite.rs`（`--tier` 过滤参数）
  - 任务状态：`[ ] 待开始`
- **MCP 能力映射与协议报文契约**：三级调度入口（对齐 test-architecture §4.3）：

```bash
# Smoke: unit plane delta + 211-case TSV + engine scenario smoke set.
just smoke                      # already wired (justfile:328) -- unchanged
# Acceptance: full suites + on-screen visual cards, serial on the X server.
just check-e2e-acceptance       # testd-suite --tier acceptance (X11 档; 无显示时 env_gate 判不可执行并出报告)
# Stress: soak + allocation + scale cards; never in CI, machine-exclusive.
just check-e2e-stress           # testd-suite --tier stress + soak
```

```rust
/// Suite tier filter. Selection is by the case cards' tier field; an
/// unknown tier is a usage error. `Acceptance` implies `Smoke` first.
pub enum TierFilter { Smoke, Acceptance, Stress }
```

- **实现细节**：`suite.rs` 增 `--tier` 参数（按账本 tier 过滤用例）；Acceptance 档强制串行（X 焦点竞争）并先跑 Smoke 作为门闸；Stress 档拒绝在有其他负载时启动（复用 `purity` 探测）；CI 只挂 Smoke（现状）+ Acceptance 的 headless 子集（屏幕面在 CI 无显示环境由 env_gate 判不可执行、出报告不阻塞——与 v1 门禁语义一致）。
- **逐步落地实施步骤**：
  1. `[步骤 1]`：`--tier` 过滤 + 未知档报错；Smoke 先行门闸逻辑。
  2. `[步骤 2]`：justfile 三档位 + CI 编排（headless 子集判定）。
  3. `[步骤 3]`：三档各实跑一次并出 final-report；purity 拒绝路径演练。
- **验收标准 (DoD)**：
  - **[单元测试闭环]**：`cargo nextest run -p xtask` 全绿；tier 过滤单测覆盖。
  - **[物理产物验证]**：`just check-e2e-acceptance` 在本机真实执行并产出带 tier 标注的批次报告；CI 编排在无显示环境产出「屏幕面不可执行」报告而非误红。
  - **[性能开销基线]**：调度器自身开销 ≤ 1s；Smoke 档总时长不高于现状 `just smoke` + 10%。

---

## Phase 3 汇总

| 任务 | 能力 | 通道 | 工时 | CP |
|---|---|---|---|---|
| FEAT-TEST-P2.05.01 批次报告生成器 | MCP-P-10 | C | 1.5 | 否 |
| FEAT-TEST-P2.06.01 分级门禁调度 | MCP-P-11 | C | 2 | 是（收口） |

**波次建议**：W1 = {P2.05.01}；W2 = {P2.06.01}。平台集群总工期 ≈ 15 人天（8 卡），CP ≈ 9.5 人天。
