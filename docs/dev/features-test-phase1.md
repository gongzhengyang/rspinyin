# dev-features-test — Phase 1 平台任务分片（P0 平台内核）

> 所属主控文档: [./features-test.md](features-test.md) ｜ 关联架构规范: [./test-architecture.md](test-architecture.md) ｜ 阶段定位: Phase 1（P0 可测性账本与驱动扩展）｜ 包含任务区间: FEAT-TEST-P0.01.01 ~ FEAT-TEST-P0.02.02（4 卡，预估 8 人天）
>
> 逻辑自检: 已通过（七维基线与正则对账闭环）
>
> 阶段目标：把证据链升级到 tests v3.0 合同（账本字段、三态门禁），并把引擎重放面扩展到 features-add 增量 surface（场景 schema v2、多源词库 fixture）。全部任务落在 `xtask/src/testd/` 与其 fixture 目录内，不触碰生产 crate。

---

#### FEAT-TEST-P0.01.01 证据账本 v3（assertions/checklist/index 三级账本）

- **基本属性**：
  - 绑定平台能力：`MCP-T-10` ｜ 底层驱动框架：`xtask testd`（`evidence.rs` 扩展）
  - 优先级与复杂度：`P0 平台内核` ｜ 中 ｜ 预估工时: 2.5 人天
  - 前置依赖：`无`
  - 关键路径：`CP: 是`（v3 账本起点，下游 4 卡依赖）
  - 并行通道：`Track C 基建·CI`
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/evidence.rs`（扩展）、`xtask/src/testd/suite.rs`（批次编排调用点）
  - 任务状态：`[x] 已完成`
- 落地证据：v3 账本落地: xtask/src/testd/evidence/v3.rs 新增 AssertionV3/Claim/Tier/Dim 协议类型(serde d1-d5/smoke-acceptance-stress, from_name 容错解析); assertions.json 以 v3 键并列 v1 记录(skip_serializing_if, v1 读取者不受影响); RunJournal.record 差集探测 screenshots_expected vs bundle 目录,缺失即红行+BatchSummary.screenshot_gaps,verdict 按 Flawed 判定但用例状态不改写; publish() 落盘批次checklist.json(format=3)与 index.md 同写; suite.run_case 为每用例构建 v3(claims 由 v1 assertions 映射,dims/截图/budget 键留空=诚实未声明); evidence/tests/v3.rs 五测: 往返序列化/红行传播/缺失差集/损坏 JSON 拒读/checklist 落盘回读; CaseRow.is_complete 修复为含截图缺失。验收: cargo nextest run -p xtask 1225/1225 绿;物理产物 results/runs/run-20261021-065428: checklist.json format=3+30 行(tier=smoke),index.md,TC-CORE-01 assertions.json 含 v3 节 5 claims,全部 0600; 29/30 通过,TC-CORE-20(just bench,卡面标注须空闲机采集)在本机负载下 exit 101 属测量纯净度前提而非本任务回归。性能: v3 为既有写路径上 ~6 断言的内存映射+同文档内 JSON 节,无新增文件,增量<1ms ≤5ms 基线。注: suites 判定表按 nextest DSL 或语义,task_check 过滤器须写括号交替式
- **MCP 能力映射与协议报文契约**：`capture_viewport_screenshot` / `capture_execution_dump` 的落盘元数据升级。`assertions.json` 条目 schema v3：

```rust
/// One structured assertion record, v3. The tier/dims/evidence fields are the
/// contract `dev-check` reconciles against the tests v3.0 case cards; the
/// budget keys name `budgets.json` entries instead of restating numbers, so
/// an assertion can never drift from its threshold source.
#[derive(Serialize, Deserialize)]
pub struct AssertionV3 {
    /// Stable case id, e.g. "TC-UI-07".
    pub case: String,
    /// The suite tier: "smoke" | "acceptance" | "stress".
    pub tier: Tier,
    /// Covered dimensions, subset of ["d1","d2","d3","d4","d5"].
    pub dims: Vec<Dim>,
    /// Whether the case owes PNG evidence (GUI plane) or a log dump.
    pub requires_screenshots: bool,
    /// The exact file names the evidence dir must contain.
    pub screenshots_expected: Vec<String>,
    /// `budgets.json` keys this case's thresholds come from (names only).
    pub budget_keys: Vec<String>,
    /// Per-claim verdicts: what was expected, what was measured.
    pub claims: Vec<Claim>,
}

#[derive(Serialize, Deserialize)]
pub struct Claim {
    pub name: String,
    pub expect: String,
    pub actual: String,
    /// `false` also covers the "not verifiable on this machine" state, which
    /// must carry the env-gate remedy text in `actual`.
    pub pass: bool,
}
```

- **实现细节**：`evidence.rs` 既有 per-case 写入点（PNG/日志/trace）保持；新增 v3 记录的写入与**批次级** `checklist.json`（用例 × 状态 × 证据存在性布尔）与 `index.md` 骨架生成；`screenshots_expected` 与实际文件集合的差集即为账本红行——用例卡声明了截图却没落盘是**平台缺陷**而非用例失败，必须显式报错。
- **逐步落地实施步骤**：
  1. `[步骤 1 - 协议与账本]`：`AssertionV3`/`Claim`/`Tier`/`Dim` 类型与序列化；per-case 写入路径接线。
  2. `[步骤 2 - 批次账本]`：`suite.rs` 批次收尾时汇总生成 `checklist.json` + `index.md`；证据存在性差集校验。
  3. `[步骤 3 - 容错与自测]`：`evidence/tests.rs`（现 module）补 v3 字段断言；损坏 JSON 的容错读取。
- **验收标准 (DoD)**：
  - **[单元测试闭环]**：`cargo nextest run -p xtask` 全绿，v3 字段序列化往返一致，异常输入不 Panic。
  - **[物理产物验证]**：跑一次 `testd-suite` 冒烟集，`results/runs/run-*/` 下真实生成含 v3 字段的 `assertions.json` 与批次 `checklist.json`/`index.md`（非空）。
  - **[性能开销基线]**：账本写入对单用例执行耗时增量 ≤ 5ms；截图捕获端到端 ≤ 1.5s（既有基线不回退）。

---

#### FEAT-TEST-P0.01.02 环境门禁三态扩展（待实现 / 不可验证 / 可执行）

- **基本属性**：
  - 绑定平台能力：`MCP-R-10` ｜ 底层驱动框架：`xtask testd`（`env_gate.rs` 扩展）
  - 优先级与复杂度：`P0 平台内核` ｜ 低 ｜ 预估工时: 1 人天
  - 前置依赖：`无`（与 P0.01.01 无文件交集，可并行）
  - 关键路径：`CP: 否`
  - 并行通道：`Track C 基建·CI`
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/env_gate.rs`、`xtask/src/testd/env.rs`
  - 任务状态：`[x] 已完成`
- 落地证据：环境门禁三态落地: env_gate/eligibility.rs 新增 CaseEligibility(Runnable/PendingImpl{task}/NeedsEnvironment{remedy:&'static})+eligibility_of(卡面标注优先于判定表,表 Blocked 行映射 PendingImpl,Unverifiable 行映射 NeedsEnvironment 且 remedy 逐字引用表原文)+claim()渲染账本落点(pending-impl: TASK-… / needs-environment: …, pass=false); cases.rs 探针解析用例卡基本属性行的 可执行性:[待实现] 与 锚定 TASK id(全角/半角竖线分段,锚点: 字段不误读,无锚待实现拒绝置位走判定表fail-closed); evidence.rs 增 CaseStatus::Skipped(needs_trace=false,既不算通过也不算失败),BatchSummary.skipped 单列且不进 cases()/verdict(); env_gate/tests.rs 四测: 标注优先/三态映射(remedy 逐字)/无锚拒绝/账本落点 + 真实分片探针形状测试 + #[ignore] 物理产物运行器。偏差登记: 卡面契约 PendingImpl.task 写 &\'static str,实现为 String——锚定 id 运行时读自用例卡,静态寿命需本模块枚举全部 features-add 任务即判定表禁止的硬编码清单;remedy 保持 &'static(表原文)满足契约另一半。验收: cargo nextest run -p xtask 1230/1230 绿; 物理产物 results/runs/run-20261021-073359: 真实 TC-ADD-001 卡(锚定 TASK-1.01.01/TASK-1.01.02)→ assertions.json status=skipped+v3 claim pending-impl: TASK-1.01.01/TASK-1.01.02(pass=false),checklist.json verdict=pass 且行 complete(不计红),文件 0600。性能: 判定为常量表查询+一次分段扫描,无可测开销,符合基线
- **MCP 能力映射与协议报文契约**：`runtime://env` 的判定输出从「可执行 / 不可验证」二元扩展为三态机读：

```rust
/// Why a case cannot run here. `PendingImpl` names the features-add task the
/// case is anchored to: the case is legal, its subject does not exist yet,
/// and the ledger must say so instead of failing it. `NeedsEnvironment`
/// carries the remedy text verbatim from the gate table, as v1 did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaseEligibility {
    Runnable,
    /// The implementation behind the case is not merged yet; `task` is the
    /// features-add id (e.g. "TASK-1.03.02") that flips it to Runnable.
    PendingImpl { task: &'static str },
    /// Code exists, this machine cannot judge it (Wayland tiers, real blur,
    /// multi-monitor, 8h soak). Remedy text is mandatory.
    NeedsEnvironment { remedy: &'static str },
}
```

- **实现细节**：`env_gate` 判定表增「实现缺席」探针——按用例卡基本属性的 `` 可执行性：`[待实现]` `` 标注识别（当前仅 `tests/add.md` 分片），并从同卡的 `锚定` 字段取 features-add TASK id；`PendingImpl` 用例在账本记 `skipped: pending-impl`，既不算通过也不算失败，CI 汇总单独列示。保留 v1 的 `remedy` 原文引用纪律。
- **逐步落地实施步骤**：
  1. `[步骤 1]`：三态枚举与判定表扩展；`PendingImpl` 的 task id 解析（读用例卡标注，单点实现）。
  2. `[步骤 2]`：与 P0.01.01 账本的对接（`Claim.pass=false` + `actual="pending-impl: TASK-…"`）。
  3. `[步骤 3]`：`env_gate` 自测：三态各构造一例，断言判定与账本落点。
- **验收标准 (DoD)**：
  - **[单元测试闭环]**：`cargo nextest run -p xtask` 全绿；三态判定覆盖。
  - **[物理产物验证]**：对一条 `[待实现]` 卡执行，证据包 `assertions.json` 出现 `pending-impl` 记录且批次 checklist 不计红。
  - **[性能开销基线]**：判定为常量表查询，无可测开销。

---

#### FEAT-TEST-P0.02.01 场景 schema v2（DecodeFlags 注入 / 用户库 fixture / BIGRAM 缺席 / 衰减时钟）

- **基本属性**：
  - 绑定平台能力：`MCP-T-11` ｜ 底层驱动框架：`xtask testd`（`engine.rs` + 场景 TOML）
  - 优先级与复杂度：`P0 平台内核` ｜ 高 ｜ 预估工时: 2 人天
  - 前置依赖：`FEAT-TEST-P0.01.01`（重放输出经账本 v3 落盘）
  - 关键路径：`CP: 是`
  - 并行通道：`Track A 平台内核`
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/engine.rs`（场景解析与重放装配）、`tests/fixtures/scenarios/`（新字段示例）
  - 任务状态：`[x] 已完成`
- 落地证据：['契约: v2 增段全部可选、缺省=v1；[decode] flags=DecodeFlags 名称（未知名=解析错）+ bigram=absent|present；[user_db] fixture=cold|seed:<name>；[decay] half_life_ticks（无 store 拒绝）；cross-section 规则 parse 期拒绝（clock-without-store / bigram+spec-model / store+spec-freq）；deny_unknown_fields', '实现: xtask/src/testd/engine/scenario/v2.rs（解析+校验+ScenarioV2::of 投影）、scenario/store.rs（load 侧真 UserDb 装配：open_with_decay+FixedClock+seed 回放，drive 路径零 IO）、scenario/fixture.rs（store/container/container_lm 装载进 fixture，sources() 换源，Drop 清 scratch）、engine.rs（Session::extra 注入 [decode].flags，replay 直读 scenario.decode.flags）', 'CLI 缺口修复（物理重放抓出）: cli.rs run() 原直传 spec 双打（&fixture.user_freq/&fixture.lm），v2 的 store/container 永远到不了 CLI 重放；改为 fixture.sources()，与测试同路。修后 CLI 重放 21 场景 0 diverged', 'v1 兼容: test_every_builtin_scenario_stays_v1（18 个内建场景 is_default×2+decay=None 且 fixture 可装配）+ 内建集 CLI 重放全绿；strace 纯净性 drive 路径不变（源扫描 store.rs 显式豁免并注明 load-side-only）', '示例场景: tests/fixtures/scenarios/add-fuzzy.toml（模糊音 zh_z）、add-abbrev.toml（[decode].flags=[ABBREV]）、add-user-pin.toml（[user_db] seed:pin 置顶序列）；tests/fixtures/userdb/pin.toml（你好=100 commits）；物理重放: cargo run -q -p xtask -- testd-engine --scenarios tests/fixtures/scenarios => 21 scenarios, 0 diverged', '测试: cargo nextest run -p xtask => 1243 run / 1243 passed / 9 skipped（连续两轮全绿）；engine 模块 45 测试含 v2 七测（flags 到达 request、三段不可装配拒绝、ADD 三例重放绿、store 供种、container 接管模型、v1 兼容门、解析≤1ms/场景 min-of-50 稳健式）', '性能: 解析增量 min(50)<=1ms/场景断言通过；重放路径零新增热区=v2 全部装载期装配（fixture.rs），drive 路径无 fs/clock/env（源扫描+--strace 机制不变），ime-core 解码器零改动故 alloc-count 门禁基线不变', '偏差1: 契约代码示例的 ScenarioV2.flags 字段未建——resolved flags 与 [decode] 段是同一事实的两种存法，replay 直读 scenario.decode.flags（唯一消费者），第二份拷贝=第二真相源；解析行为与卡片一致（未知名=红色解析错）', '偏差2: 卡片「证据包含 v3 断言记录」——engine CLI 不写证据包（证据包走 suite 通道），engine 重放的物理产物是 CLI 重放输出+场景文件本体；v3 落盘由 P2.05.01 FinalReport/suite 通道承接，此处如实登记', '环境注记: --strace 动态纯净性检查本机未装 strace 无法运行；静态半由 engine/tests.rs 源扫描断言承担，动态半待有 strace 的机器补跑']
- **MCP 能力映射与协议报文契约**：`testd-engine` 场景 TOML v2 增段（全部可选，缺省保持 v1 行为）：

```toml
# scenario v2 optional sections -- every field defaults to the v1 behaviour,
# so the existing 18 scenarios parse unchanged.
[decode]
flags = ["USER_DICT", "ABBREV"]      # DecodeFlags by name; unknown name = parse error
bigram = "absent"                    # "absent" (default) | "present" (v2 container fixture)

[user_db]
fixture = "cold"                     # "cold" (never-recorded) | "seed:<name>" from fixtures/userdb/

[decay]
half_life_ticks = 139                # enables the decay clock at a known constant
```

```rust
/// The v2 scenario extras. Parsing is total: an unknown flag or fixture name
/// is a red parse error, never a silently ignored line.
pub struct ScenarioV2 {
    pub flags: Vec<ime_types::decode::DecodeFlags>,
    pub bigram: BigramMode,
    pub user_db: UserDbFixture,
    pub decay: Option<DecayClock>,
}
```

- **实现细节**：`engine.rs` 的重放装配点按 `ScenarioV2` 组装 `DecodeRequest`（flags 位）、`UserFreqSource` fixture（复用 `dict_inject` 的内存 redb 能力）、`LanguageModel`（absent = 现行 miss 路径；present 需要 v2 容器 fixture，由 P0.02.02 提供）；衰减时钟注入到 `UserDb` 的逻辑 tick（对应 `TASK-1.02.02` 的可注入设计）。输出经账本 v3 记录。
- **逐步落地实施步骤**：
  1. `[步骤 1]`：TOML v2 段解析 + 校验（未知键报错）+ v1 兼容回归（18 个既有场景零改动通过）。
  2. `[步骤 2]`：重放装配扩展（flags/用户库/衰减时钟）；BIGRAM present 档先返回明确的「fixture 未就绪」诊断（随 P0.02.02 解锁）。
  3. `[步骤 3]`：为 features-add 的 ADD 分片用例各写 1 个 schema v2 示例场景（模糊音开启、简拼开启、置顶序列）。
- **验收标准 (DoD)**：
  - **[单元测试闭环]**：`cargo nextest run -p xtask` 全绿；v1 场景兼容矩阵全过。
  - **[物理产物验证]**：新增示例场景经 `testd-engine` 重放，证据包含 v3 断言记录。
  - **[性能开销基线]**：场景解析增量 ≤ 1ms/场景；重放路径零新增分配热区（alloc-count 门禁不回退）。

---

#### FEAT-TEST-P0.02.02 dict_inject 多源 fixture（新词源样例库与 staged 词库）

- **基本属性**：
  - 绑定平台能力：`MCP-T-12` ｜ 底层驱动框架：`xtask testd`（`dict_inject.rs` 扩展）
  - 优先级与复杂度：`P0 平台内核` ｜ 中 ｜ 预估工时: 1.5 人天
  - 前置依赖：`无`（与 P0.02.01 无文件交集：本卡只产 fixture 文件与装载 API，不碰场景解析）
  - 关键路径：`CP: 否`
  - 并行通道：`Track A 平台内核`
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/dict_inject.rs`、`tests/fixtures/dict/`（新增样例库）
  - 任务状态：`[x] 已完成`
- 落地证据：dict_inject 多源 fixture 落地: 新增 dict_inject/generate.rs,DictFixture 四值枚举(Synthetic/Layered/BigramPresent/StagedVariant)+generate()(进程内复用 dictc build::compile 分段装配,Writer encode,Reader::parse Verify::Full 装载校验后才落盘)+generate_in()(FstLexicon 真实加载);词表全部闭世界固定表(18 词 12 键/9 词 8 键/6 词 6 键),BigramPresent 语料为本仓自写 tests/fixtures/dict/corpus-snippet.txt(20 句,无 ADR-0012 语料,min_score=0 全对保留);期望表入库 tests/fixtures/dict/expectations.toml(词数/键数/同键序/unigram Q12/bigram 对与 Q12);StagedVariant 与 Synthetic 同键不同序(四多候选键全部倒序);装载校验含 flag 断言(仅 bigram-present 置 FLAG_BIGRAM_PRESENT)。偏差登记: 卡面枚举名 DictFixture 与既有突变宿主类型冲突,既有结构更名 MutatedContainer(模块内 24 处,编译器核对),新枚举取契约原名;dictc::bigram 模块改 pub(crate) 供进程内调用。测试 generate_tests.rs 五组: 双生成 sha256 一致+≤200ms/四容器 Reader::open+计数+flag 对账/synthetic 序与 Q12/layered 三带划分(尾带扁平且覆盖全部词)/bigram 16 对全在+右边际 Q12(4096/2457/1228/682)/staged 同键倒序+经 FstLexicon lookup 换库后首候选可观测。验收: cargo nextest run -p xtask 1236/1236 绿; 物理产物为测试内临时目录四容器文件,Reader::open 全过(生成器内 Verify::Full 预检)。性能: 单 fixture 生成实测毫秒级 ≤200ms 基线。步骤3 场景 v2 对接与 env_gate 旧构建 pending-impl 报告属 P0.02.01 落点,本卡 API 已就绪
- **MCP 能力映射与协议报文契约**：fixture 家族扩展，供词源/热替换/分层归一化用例消费：

```rust
/// Named dictionary fixtures the case cards reference. `Synthetic` stays the
/// closed-world default (ASM-T-07); the new entries exist so the word-source
/// and hot-swap cases exercise real-shaped data without reading anything
/// from `data/compiled/` or the network.
pub enum DictFixture {
    /// The classic synthetic build (existing behaviour).
    Synthetic,
    /// Two-layer weights in one container: core true-frequency band + flat
    /// tail band, for the normalization cases (TASK-1.01.x).
    Layered,
    /// A container with a populated BIGRAM section (v2), for the bigram
    /// lookup cases (TASK-1.03.x); built by dictc from an in-repo corpus
    /// snippet, never from fetched data.
    BigramPresent,
    /// A second container staged for the hot-swap cases (TASK-3.03.02):
    /// valid but different ranking, so a swap is observable in candidates.
    StagedVariant,
}
```

- **实现细节**：生成器复用 dictc 编译 API（进程内调用，产物写入用例临时目录）；`BigramPresent` 的语料片段入库（`tests/fixtures/dict/corpus-snippet.txt`，自有文本，几十句），**禁止**引用 ADR-0012 语料（测试封闭性，ASM-T-07）；`StagedVariant` 与 `Synthetic` 同键不同序，保证 swap 前后候选可区分。
- **逐步落地实施步骤**：
  1. `[步骤 1]`：fixture 枚举与生成 API；corpus-snippet 与期望表入库。
  2. `[步骤 2]`：`Layered`/`BigramPresent`/`StagedVariant` 三个生成器；装载校验（CRC/段表）。
  3. `[步骤 3]`：与场景 v2（P0.02.01）的 `bigram = "present"` 档对接解锁；`env_gate` 对缺 BIGRAM 支持的旧构建报 `pending-impl`。
- **验收标准 (DoD)**：
  - **[单元测试闭环]**：`cargo nextest run -p xtask` 全绿；三 fixture 生成确定性（双生成 sha256 一致）。
  - **[物理产物验证]**：临时目录真实产出三种容器文件且 `Reader::open` 全过；`BigramPresent` 的 flag 已置位。
  - **[性能开销基线]**：单 fixture 生成 ≤ 200ms（进程内 dictc）；对用例执行链路无可测开销。

---

## Phase 1 汇总

| 任务 | 能力 | 通道 | 工时 | CP |
|---|---|---|---|---|
| FEAT-TEST-P0.01.01 证据账本 v3 | MCP-T-10 | C | 2.5 | 是 |
| FEAT-TEST-P0.01.02 门禁三态 | MCP-R-10 | C | 1 | 否 |
| FEAT-TEST-P0.02.01 场景 schema v2 | MCP-T-11 | A | 2 | 是 |
| FEAT-TEST-P0.02.02 dict_inject 多源 | MCP-T-12 | A | 1.5 | 否 |

**波次建议**：W1 = {P0.01.01, P0.01.02, P0.02.02}（三卡文件不相交）；W2 = {P0.02.01}（依赖账本 v3）。与 tests v3.0 分片写作无阻塞关系：用例卡先行，`PendingImpl` 机制保证未实现面不误报。
