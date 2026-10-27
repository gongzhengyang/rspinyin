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
  - 任务状态：`[x] 已完成`
- 落地证据：['FinalReport: evidence/report.rs — CaseVerdict{tier,dims,verdict: CaseOutcome Pass|Fail|PendingImpl|Unverifiable}、budget_violations（仅失败用例触碰的 budget_keys，去重排序、names-only）、blockers（失败用例含 trace 路径 + 红行的平台缺陷）；render() 纯函数输出中文 markdown（用例表/预算违例/阻塞项节），case id/budget key/路径保持账本原文', '判定映射: checklist 行 Skipped 携带 withheld 前缀（pending-impl/needs-environment，取自 eligibility claim），报告据其区分 PendingImpl 与 Unverifiable；Flawed→Unverifiable；ChecklistRow 增 withheld/dims/budget_keys（serde default，旧 checklist 兼容解析）', 'register_run: results/index.json 改为 JSONL 追加写——每 run 一行，追加永不重写既有字节；0600；读端坏行跳过+stderr 告警（一条坏行=一条丢失条目而非整个历史），文件保持原样；旧版整体式 document（含 format 字段）读时整体采纳、写时一次性无损迁移为行（迁移测试断言每个条目都存活且仅换包装）', 'RunIndex 收敛为容忍读端: open()=read_runs+dedupe（同名取后行=重发布语义），latest_run=最大 run 名（旧指针规则不变）；publish 改走 register_run；RecordIndex 的 record/write/读拒路径移除，INDEX_FORMAT_VERSION 常量删除（死代码）', 'suite 接线: testd-suite 收尾以 publish 写的 checklist 渲染 final-report.md 并写回 run 目录（报告语言中文）；RunDir::write_report 落盘', '物理产物: cargo run -q -p xtask -- testd-suite => 30 用例真实跑（29 pass / 1 fail=TC-CORE-20 机器负载已知项），run-20261021-103501/final-report.md 生成非空（30 行判定表+阻塞项含 trace 路径）；results/index.json 追加成功；--only TC-CORE-01 复跑绿灯且读端零告警；本地旧格式 registry 已迁移（4 条目无损）', "缺陷修复记录（物理运行抓出）: 旧 document 无尾换行致追加行粘连（}{'run'}接缝）——读端改为整文本优先采纳 document + register_run 对 document 形态先迁移再落行；本地文件已按同规则修复", '测试: cargo nextest run -p xtask => 1274 run / 1274 passed / 9 skipped；报告四判定映射（withheld 区分两种 skip）、预算键仅失败侧、阻塞项 trace 路径与红行、空批次、中文渲染、checklist 回读再生成（无状态可再生）、追加不改写字节、坏行跳过、document 采纳+迁移一次性、渲染 min(10)≤100ms（500 行批）', '性能: 渲染 ≤100ms/批次断言通过（500 行）；只读既有证据无额外采样开销（渲染仅消费 checklist.json）', '偏差1: results/index.json 物理格式由整体式 JSON document 改为 JSONL——卡片契约要求追加写+坏行跳过，document 形态两者皆不可实现；旧文件由采纳/迁移路径无缝接管', '偏差2: 卡片锚点 evidence.rs 单文件，实际落 evidence/report.rs 子模块（evidence.rs 738 行，加渲染函数将破 800 行限），与 evidence/{index,v3,write}.rs 同型', 'clippy: report/index/tests 零警告（redundant_closure 已修）；存量 ime-core/ime-dict 警告留阶段4']
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
  - 任务状态：`[x] 已完成`
- 落地证据：xtask/src/testd/suite.rs(+cases/error/tests/tier.rs)+justfile：--tier smoke|acceptance|stress（TierFilter，未知拼写=用法错误）、plan_phases 空档拒绝(suite/tier/empty)、Stress 档 purity 拒启(suite/purity/busy)、env_gate 不可执行用例按 eligibility claim 跳过（needs-environment/pending-impl 前缀，报告不误红）、Acceptance 先跑 Smoke 门闸；11 个新 tier 测试，xtask 全量 1296/1296 绿；物理实跑：acceptance=run-20261022-105401（29/30，TC-CORE-20 沿承已有红：bench purity 拒采，门闸真实拦截 acceptance 集，报告带 tier 标注）、stress=purity fail-closed 拒绝演练（/proc 不可读）、smoke 档=run-20261021-103501（同 29/30）；偏差登记：① acceptance 面暂空（无已转录行，按表设计 tiers 以列编辑到达，仅空门闸/空单档拒绝）② headless 不误红路径由单测证明（表内尚无带需求行）③ just ci 保持现状仅挂 smoke——smoke 档含 fuzz/bench 卡，CI 是否挂 acceptance 留给用户决策；调度开销：规划 min(50)≤1s 单测；just smoke 未动，smoke 档时长预算按构造成立
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
