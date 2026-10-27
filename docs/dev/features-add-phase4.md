# dev-features-add — Round-2 Phase 4 增量原子任务分片（P0 排序第二跃升与联想）

> 所属主控文档: [./features-add.md](features-add.md) ｜ 关联对比报告: [./compare-add.md](compare-add.md) ｜ 阶段定位: Round-2 Phase 4（P0）｜ 包含任务区间: TASK-4.01.01 ~ TASK-4.05.01（6 卡，预估 ≈14.5 人天）｜ 基线: main@`0ef7d8b`（Round-1 全部已交付）
>
> 阶段目标：把排序深度的三处残留封顶（k-best 名次衰减、上文带与 bigram 预算、候选供给阶梯），解除体积红线对一切扩表的阻塞，并交付本轮旗舰能力「联想（下一词预测）」。全阶段遵守 `ASM-A-11`~`ASM-A-16` 与 §8.4/8.5/8.6 的热点收敛、契约与预算裁决。

---

#### TASK-4.01.01 Round-2 契约底座（ADR-0017 批次 + 配置骨架）

- **基本属性**：
  - 绑定差距与改进项：`GAP-22 / IMP-23`（底座服务于 GAP-22/29/30 的契约消费）｜ 关联设计假设：`ASM-A-12`
  - 差距类型：`架构与测试基建`
  - 优先级与复杂度：`P0 核心增强` ｜ 低 ｜ 预估工时: 1.5 人天
  - 前置依赖：`无`
  - 关键路径：`是`（CP-R2 首环）
  - 并行通道：`Track A Core·Engine·Resilience`
  - 对标参考锚点 (Benchmark Reference)：WindInput `wind-assoc/src/lib.rs:50-110`（来源枚举契约形态，SCOUT-D-09）；rime-ice `select_character.lua:20-60`、`cold_word_drop/processor.lua:169-202`（按键动作语义，SCOUT-ICE-14/04）
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-types/src/ui.rs`（CandidateSource 枚举）、`crates/ime-types/src/key.rs`（KeyAction 枚举）、`crates/ime-config/src/schema/config.rs`（`[assoc]` 节骨架）、`docs/dev/adr/0017-round2-contract-batch.md`（新建）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：把 Round-2 全部契约增量收敛为一次 ADR 裁决与一次枚举扩展，消除后续 10+ 张卡的契约等待与多卡同文件冲突（§8.4 热点规则）。完成本卡后，`TASK-4.04.01`、`TASK-6.02.01`、`TASK-6.03.01` 等消费方卡可完全并行。
- **技术设计与代码级接入细节**：
  - **契约变体（全部只增不改，ADR-0017 落盘后生效）**：

```rust
// crates/ime-types/src/ui.rs —— CandidateSource 追加一个变体（既有变体零改动）
pub enum CandidateSource {
    // ... 既有变体保持不动 ...
    /// Next-word association candidate produced by an assoc source after a
    /// commit (see ADR-0017). Rendered with the same candidate widgets; the
    /// marker lets the UI annotate "联想" without a new frame field.
    Assoc,
}

// crates/ime-types/src/key.rs —— KeyAction 追加四个变体（既有变体零改动）
pub enum KeyAction {
    // ... 既有变体保持不动 ...
    /// Commit the first character of the highlighted candidate (以词定字).
    SelectFirstChar,
    /// Commit the last character of the highlighted candidate (以词定字).
    SelectLastChar,
    /// Penalize the highlighted candidate's user frequency (negative feedback).
    ReduceFrequency,
    /// Suppress the highlighted candidate for the rest of this session.
    HideCandidate,
}
```

  - **配置骨架**：`schema/config.rs` 增 `[assoc]` 节（`enabled: bool = false`、`max_candidates: u8 = 5`、`history_depth: u8 = 8`），默认全关；`DEFAULT_CONFIG_TOML` 与 `Config::default` 防漂移测试同步扩展（坏值修默认 + 每键一条 `config/invalid` 诊断的既有哲学）。
  - **ADR-0017 正文**：动机（联想/以词定字/负反馈三能力共用一次冻结程序）、变体清单、兼容性论证（枚举追加不破坏 wire 与既有匹配的穷尽性处理清单——编译器强制全部 match 臂补齐即回归网）、退出路径（变体保留、消费卡可独立回退）。
- **无侵入旁路挂载方案**：枚举追加后，全部既有 `match` 臂以编译期穷尽性检查暴露；每个新增臂的默认行为=现状行为（`Assoc` 候选无消费者时永远不会产出；四个 KeyAction 无路由时维持 `Effect::Diagnose(Unsupported)` 现状模式），保证本卡独立合入后存量行为逐字节不变。
- **底层与非功能约束 (NFR)**：零运行时开销（纯类型与配置解析）；`decode_steady=13` 分配计数不回退；wire 结构体零变更。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - ADR 落盘]`：撰写 `0017-round2-contract-batch.md` 并在 `docs/dev/adr/README.md` 把 0017 置为已接受。
  2. `[步骤 2 - 契约扩展]`：五个枚举变体 + 全部 match 臂补齐（默认臂=现状行为）+ `ui.rs`/`key.rs` 模块文档更新。
  3. `[步骤 3 - 配置骨架与测试]`：`[assoc]` 节 + 防漂移测试 + 坏值修复测试；`cargo nextest run -p ime-types -p ime-config` 全绿。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：枚举穷尽性 match 全绿；配置防漂移与坏值修复测试通过（`cargo nextest run -p ime-types -p ime-config`）。
  - [ ] 行为走查：本卡合入后默认配置下解码行为与 `0ef7d8b` 逐字节一致（冒烟 202 例全绿）。
  - [ ] 编译与旧功能回归：零警告编译；ADR-0017 已登记为已接受；无任何消费方卡提前引用未落盘变体。

#### TASK-4.01.02 词条价值审计与体积治理

- **基本属性**：
  - 绑定差距与改进项：`GAP-21 / IMP-22` ｜ 关联设计假设：`ASM-A-14`（治理是扩表前置）、`ASM-A-03`
  - 差距类型：`架构与测试基建`
  - 优先级与复杂度：`P0 核心增强` ｜ 中 ｜ 预估工时: 3 人天
  - 前置依赖：`无`（与 TASK-4.01.01 文件不相交，可并行）
  - 关键路径：`是`（GAP-20/32/33 的预算前置）
  - 并行通道：`Track C Infra·Testability·DevOps`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `cn_dicts/` 六分表职责纪律与「权重属词条不属字」（SCOUT-ICE 摘要 §三）；librime `entry_collector.cc:198-201` 建库期质量门（SCOUT-A）
  - 本项目代码落地锚点 (Code Anchor)：`xtask/src/dictc/audit.rs`（新建：词条价值审计）、`xtask/src/dictc/normalize.rs`（裁剪 pass 挂载点）、`xtask/src/dictc.rs`（审计/裁剪参数）、`data/compiled/`（产物）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：base.dict 17.6MiB 已占 20MiB 预算 84%（B-04），任何词表/标记/辅码列扩充都会顶格。本卡建立「词条价值分位审计 → 确定性裁剪 → 质量门对照」闭环，目标**裁剪后体积 ≤ 16.5MiB（腾出 ≥1.0MiB）且 holdout top1 不降**，为 TASK-4.03.01（pair 预算）与 Phase 6 扩表卡（辅码列/构建期标记）解除阻塞。
- **技术设计与代码级接入细节**：
  - **价值模型**：每词条计 `value = f(频带, 词长, 来源, 冗余度)`——频带取归一化后权重分位；冗余度 = 同键同前缀近邻数（长尾重复模式）；单字与两字高频带永不在裁剪集。裁剪按确定性排序（value 升序 → 键序 → 词序）取尾，参数化 `trim_target_mib`。
  - **审计报告（新 `dictc audit` 通道，挂在既有 quality 旁）**：

```rust
/// One row of the per-band value audit written next to the manifest.
pub struct ValueBandRow {
    /// Band label, e.g. "2char-core" / "3plus-tail".
    pub band: &'static str,
    pub entries: u64,
    /// Share of total container bytes this band occupies.
    pub bytes_mib: f32,
    /// Holdout top1 contribution estimate (from quality.rs band metrics).
    pub top1_share_pp: f32,
    /// Entries the trim pass would drop under the current target.
    pub trim_candidates: u64,
}
```

  - **裁剪 pass 位置**：normalize 三频带之后、六段组装之前；`NormalizeReport` 增 trim 段（裁剪数/释放字节/分带明细），双编译 sha256 一致门禁沿用（裁剪必须确定性）。
- **底层与非功能约束 (NFR)**：编译时长 ≤ 既有 9.8s 门 +10%（审计 pass 新增）；裁剪后 `just smoke` 202 例全绿；holdout top1 门禁值**只升不降**（Round-1 裁决延续）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 审计通道]`：`audit.rs` 价值模型与 `ValueBandRow` 报告；对当前 base.dict 出首份审计报告（人工判读分位切点）。
  2. `[步骤 2 - 裁剪 pass]`：确定性裁剪实现 + `trim_target_mib` 参数 + manifest 记录裁剪参数与前后体积。
  3. `[步骤 3 - 质量对照]`：裁剪前后各跑一次 `dictc quality`，分带 top1 对照进报告；冒烟全绿确认。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：裁剪确定性（双编译 sha256 一致）测试；trim 边界（目标小于核心带体积时拒绝裁剪并报错）测试。
  - [ ] 物理产物验证：`data/compiled/` 实际产出 ≤16.5MiB 容器；审计报告三带行齐全；holdout top1 ≥ 既有门禁值。
  - [ ] 编译与旧功能回归：`just smoke` 全绿；四门禁零警告。

#### TASK-4.02.01 k-best 名次衰减与候选供给阶梯

- **基本属性**：
  - 绑定差距与改进项：`GAP-20 / IMP-21`（前半）｜ 关联设计假设：`ASM-A-02`
  - 差距类型：`新增功能`
  - 优先级与复杂度：`P0 核心增强` ｜ 中 ｜ 预估工时: 2 人天
  - 前置依赖：`无`（与 4.01.x 两卡文件不相交，可并行）
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine·Resilience`
  - 对标参考锚点 (Benchmark Reference)：librime `poet.cc:325-341` cutoff_threshold 随名次衰减 ×(1−1/N)（SCOUT-A-09）；WindInput `pinyin/viterbi.rs:353`「k=1 与单路径逐位相同」零回归约束 + `handle_candidate.rs:1106` 供给阶梯（SCOUT-D-08/D-11）
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-core/src/viterbi/kbest.rs`（TopK 槽后处理）、`crates/ime-core/src/viterbi/decoder.rs`（候选配额参数）、`crates/ime-core/src/state/mod.rs`（SessionConfig 候选数投影）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：当前 k-best 整句候选无论质量差距多大都占满候选位，低质量整句挤掉词级候选（重码/短输入场景可感知）。本卡引入名次衰减截断（质量差距超阈值的整句不再产出，腾位给词格候选）与按需供给阶梯（翻页深位才扩大物化量），提升候选列表前几屏的信息密度；top1 与首选序列零变化（WindInput 同款零回归约束）。
- **技术设计与代码级接入细节**：
  - **名次衰减截断（Q8.8 整数化）**：

```rust
/// Rank decay over the k-best sentence list, all in Q8.8.
/// `DECAY_STEP_Q88 = 24` models librime's ×(1-1/N) per-rank cutoff: the
/// r-th sentence must still beat `base - r*STEP` or it is dropped, freeing
/// its slot for lattice word candidates. Monotonic in rank, so the retained
/// prefix order (and thus the top-1) is bit-identical to the undecayed run.
const DECAY_STEP_Q88: i32 = 24;

fn decay_floor(rank: usize, base: i32) -> i32 {
    base - (rank as i32) * DECAY_STEP_Q88
}
```

  - **供给阶梯**：解码候选配额从固定 64 改为 `initial_quota → ×2 → CAP`（默认 24 → 48 → 64），翻页请求触发扩查；`SessionConfig` 增只读配额常量（不进配置文件，避免伪可调面）。
  - **跨栈不变量提炼**：截断只作用「整句候选段」，词级候选配额不受衰减影响；截断发生处记 `trace` 诊断（计数，不涉内容）。
- **底层与非功能约束 (NFR)**：衰减计算随 kbest 遍历一次完成，独占预算 ≤0.05ms（§8.6）；`decode_steady` 分配计数不回退；确定性：同输入双跑候选序列逐字节一致（既有钉死测试扩展到截断路径）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 衰减实现]`：kbest 后处理 + 单测（保留前缀逐位相同、截断点稳定、空列表安全）。
  2. `[步骤 2 - 供给阶梯]`：配额参数化 + 翻页扩查路径 + 阶梯边界（CAP 处 has_more 语义正确，参照 SCOUT-D-11 注释的双写失配教训）。
  3. `[步骤 3 - 回归对照]`：holdout top1/top3 对照报告；冒烟 202 例全绿；criterion 基准补 0.05ms 断言。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：衰减单调性/零回归（k=1 路径逐位相同）/阶梯边界三类单测全绿。
  - [ ] 物理产物验证：holdout top1 不降、前 3 屏候选的「整句:词级」配比报告改善；criterion 断言在 budgets 键内。
  - [ ] 编译与旧功能回归：`just smoke` 全绿；候选窗翻页行为在 X11 档实测正常。

#### TASK-4.03.01 上文带宽度与 bigram pair 预算重估

- **基本属性**：
  - 绑定差距与改进项：`GAP-20 / IMP-21`（后半）｜ 关联设计假设：`ASM-A-14`（受治理腾位约束）、`ASM-A-03`
  - 差距类型：`新增功能`
  - 优先级与复杂度：`P0 核心增强` ｜ 中 ｜ 预估工时: 2.5 人天
  - 前置依赖：`TASK-4.01.02`（pair 预算重估消费治理腾出的段预算）
  - 关键路径：`是`（`TASK-5.03.01` 整定 sweep 的参数面依赖本卡）
  - 并行通道：`Track A Core·Engine·Resilience`
  - 对标参考锚点 (Benchmark Reference)：librime `contextual_translation`（组内重排每轮 ≤32，SCOUT-A）；WindInput K-best 工程约束（SCOUT-D-08）；rime-ice 三层归一化的参数纪律（SCOUT-ICE §2.1）
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-core/src/viterbi/context.rs`（tie band 参数化）、`crates/ime-core/src/state/machine/rebuild.rs:113`（调用点）、`xtask/src/dictc/bigram.rs:18-24`（BigramConfig 参数重估）、`crates/ime-config/src/schema/config.rs`（`[data] context_tie_band_q16` 键）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：上文重排目前仅单词上文 + 硬编码近平局带 `1<<8`（B-02），bigram pair 双截断（`max_pairs`/`min_score`）为手工常数（B-03）——两者都是 top1 残留封顶点。本卡把带宽度参数化并由数据重估默认值，pair 预算按治理后的段份额重新裁剪（在 2.5MiB 段上限内提高 pair 存活数），并产出 third-bigram（二阶上文）的评估结论（立项或明确不做）。
- **技术设计与代码级接入细节**：
  - **带宽度参数化**：

```rust
// crates/ime-core/src/viterbi/context.rs
/// Near-tie band for context-driven reordering, Q16.16. Configurable via
/// `[data] context_tie_band_q16`; the default is re-derived by the tuning
/// sweep (TASK-5.03.01), not hand-picked. Outside the band the sentence
/// first-choice contract holds untouched (byte-identical to no-context).
pub struct ContextBand { pub q16: i64 }
```

  - **pair 预算重估流程**：治理后体积 → `dictc budget` 段份额复算 → bigram 生成以 `min_score` 扫描曲线（pair 存活数 vs 段体积）选工作点 → 双编译确定性门禁沿用；miss 路径 `unigram + BIGRAM_MISS_PENALTY` 契约不动（Round-1 X-4）。
  - **third-bigram 评估**：以现有 pair 表统计「二阶上文可消歧的近平局对」占比，产出评估报告；若潜在收益 <0.5pp top1 或段预算不足，明确登记「不立项」结论进本卡验收记录（防范围蠕变）。
- **底层与非功能约束 (NFR)**：带宽度查询 O(1)；bigram 查表预算 ≤0.25ms 不追加（Round-1 既有位）；段体积 ≤ BIGRAM 段 per-mille 上限；无衰减参数时与 v1 排序逐字节一致（Round-1 兼容契约延续）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 带参数化]`：`ContextBand` + 配置键 + 默认值=现值 `1<<8`（行为零变更合入）。
  2. `[步骤 2 - pair 重估]`：扫描曲线工具 + 工作点选择 + 重新编译容器 + 质量对照（top1/top3 分带）。
  3. `[步骤 3 - 评估与文档]`：third-bigram 评估报告 + `dictc.rs` 参数注释更新（数据来源标注）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：带参数坏值修复测试；pair 重估确定性（双编译一致）测试；无参数时 v1 等序测试。
  - [ ] 物理产物验证：新容器段体积在预算内；holdout top1 ≥ 上一门禁值（预期升）；评估报告落盘 `docs/dev/research/`。
  - [ ] 编译与旧功能回归：`just smoke` 全绿；四门禁零警告。

#### TASK-4.04.01 联想三源引擎侧（AssocSource trait + 后继查询）

- **基本属性**：
  - 绑定差距与改进项：`GAP-22 / IMP-23`（前半）｜ 关联设计假设：`ASM-A-11`（机制转写）、`ASM-A-12`
  - 差距类型：`新增功能`
  - 优先级与复杂度：`P0 核心增强` ｜ 高 ｜ 预估工时: 3.5 人天
  - 前置依赖：`TASK-4.01.01`（消费 `CandidateSource::Assoc` 契约）
  - 关键路径：`是`（CP-R2 中环）
  - 并行通道：`Track A Core·Engine·Resilience`
  - 对标参考锚点 (Benchmark Reference)：WindInput `wind-assoc/src/lib.rs:50-110,329` 四源逐源配额 + `AssocHit{text, commit}` 分离 + `boundary_broken` 断链（SCOUT-D-09/E-15）；`wind-dict/src/composite.rs:163` 层优先级纪律（SCOUT-D）
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-core/src/assoc.rs`（新建：trait + 三源 + 配额合并）、`crates/ime-dict/src/fst_index/dict_lm.rs`（后继区间查询）、`crates/ime-dict/src/user_db.rs`（最近 commit 环）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：交付联想的引擎内核：上一 commit 词之后，从 **Bigram 后继**（既有 BIGRAM 段）、**用户历史**（redb 用户词最近 commit 环）、**词表前缀延伸**（FST `prefix` 走查）三源按固定优先级取配额，产出带 `CandidateSource::Assoc` 标记的候选。三源契约照搬 WindInput 的语义转写：跨源不比分数（量纲不同）、展示与上屏文本分离、配额合并确定性。
- **技术设计与代码级接入细节**：
  - **核心抽象（引擎纯函数侧，输入全部显式注入）**：

```rust
// crates/ime-core/src/assoc.rs
/// One association candidate. `commit` is the text actually inserted when
/// selected (may differ from `text`, e.g. word→suffix completion); decoding
/// stays pure — all sources are injected as data, never queried live.
pub struct AssocHit {
    pub text: String,
    pub commit: String,
    pub source: AssocSourceKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssocSourceKind { Bigram, History, Prefix }

/// Fixed-priority, per-source quota merge. Sources never compare scores
/// across kinds (different units); within a source, provider order wins.
pub fn merge_assoc(
    quota: u8,
    bigram: &[AssocHit],
    history: &[AssocHit],
    prefix: &[AssocHit],
) -> Vec<AssocHit> { /* round-robin by source priority, dedup by text */ }
```

  - **Bigram 后继查询（ime-dict 侧）**：BIGRAM 段 pair 记录按 `(prev_id asc, score desc)` 有序存储（既有格式），`DictLm::successors(prev: WordId, limit: u8) -> SmallVec<[(WordId, i32); 8]>` 用区间二分定位 `prev` 段后顺取 limit 条——零新增段、零运行期扫描。
  - **History 源**：会话内存环形缓冲（容量 `history_depth`，默认 8），只存文本与时间序，不新增持久化表（隐私红线：不上盘、不进日志）。
  - **Prefix 源**：`Lexicon::prefix(last_word_text, limit)` 流式走查（Round-1 已实装的真实语义签名），取「last_word 为前缀的更长词」中权重头几条。
- **底层与非功能约束 (NFR)**：合并纯函数、确定性（同输入同序列）；`successors` 独占预算计入联想 0.15ms 档（§8.6）；`decode_steady` 分配计数不回退（联想路径不在常规按键热路径上）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 后继查询]`：`DictLm::successors` + 区间单测（含 prev 缺席/段尾截断）。
  2. `[步骤 2 - trait 与三源]`：`assoc.rs` 三源实现 + 配额合并 + 去重语义（同 text 不同 source 的归属规则=高优先级源）。
  3. `[步骤 3 - 确定性测试]`：三源注入 fixture 的合并序列钉死测试；空源/全缺席降级测试（回退=无联想候选）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：三源配额/去重/优先级序列钉死；后继查询区间正确性；全缺席回退。
  - [ ] 物理产物验证：对「中国」类高频词，Bigram 源在真实容器上产出合理后继（手验对照 BIGRAM 表）；criterion 记录 successors 延迟在预算内。
  - [ ] 编译与旧功能回归：默认配置下解码行为零变化（联想无接线，本卡为纯引擎能力）。

#### TASK-4.05.01 联想会话接线与配置投影

- **基本属性**：
  - 绑定差距与改进项：`GAP-22 / IMP-23`（后半）｜ 关联设计假设：`ASM-A-12`（候选通道复用）、`ASM-A-13`
  - 差距类型：`新增功能`
  - 优先级与复杂度：`P0 核心增强` ｜ 中 ｜ 预估工时: 2 人天
  - 前置依赖：`TASK-4.04.01`（引擎源就绪）
  - 关键路径：`是`（CP-R2 尾环 → `TASK-5.05.01` 冒烟钉行为）
  - 并行通道：`Track B UI·Visual·Protocol`
  - 对标参考锚点 (Benchmark Reference)：WindInput 联想呈现「复用 candidates 通道、无独立 UI」（SCOUT-E-15 呈现侧注释）；`boundary_broken` 断链语义（SCOUT-D-09）
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-core/src/state/machine.rs`（commit 后联想态）、`crates/ime-core/src/state/transitions.rs`（跃迁与 Effect）、`crates/ime-fcitx5/src/engine/router/phrases.rs`（回执与重排 Effect 消费点旁路）、`crates/ime-config/src/reload/`（`[assoc]` 投影）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：把联想内核接入会话生命周期：commit 之后无输入时，候选窗直接呈现联想候选（带「联想」annotation）；任意按键立即回到常规解码（断链语义）；数字/翻页/高亮键在联想态下工作；选择即上屏 `commit` 文本。用户体验对齐主流输入法的「打完一个词，紧接着出下一个词的候选」。
- **技术设计与代码级接入细节**：
  - **状态机跃迁**：`Committed --(assoc enabled & hits non-empty)--> Composing(assoc 视图)`。assoc 视图复用 `UiFrame` 候选列表（`CandidateSource::Assoc` 标记 + annotation「联想」）；`Reset/BackSpace/任意可打印键 --> 常规 Composing`（断链）；`数字键/Space --> 提交所选联想词`；`方向键 --> 移动高亮（不重解码）`。
  - **配置投影**：`reload` 把 `[assoc]` 三键投影进 `SessionConfig`（组合开始时锁定，与 scheme 同纪律）；默认关 = 现状行为（commit 后无候选）。
  - **断链条件（WindInput `boundary_broken` 语义转写）**：失焦、切窗口、点击移动光标、`Escape` ——全部立即清联想态且**不**保留任何跨焦点记忆（会话内存环仅服务 History 源数据，不服务候选态）。
- **底层与非功能约束 (NFR)**：联想候选供给独占预算 ≤0.15ms（§8.6，仅 commit 后触发）；候选数 ≤ `max_candidates`（默认 5）；焦点安全红线不涉及（无新窗口行为）；敏感上下文（password 类）抑制联想（复用既有的敏感上下文学抑制通道，连候选呈现一并抑制）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 状态与跃迁]`：commit 后联想态实现 + 断链条件 + 默认关行为零变更验证。
  2. `[步骤 2 - 交互闭环]`：数字/翻页/高亮/选择键在联想态的路由 + `commit` 文本上屏 + `ReduceFrequency/HideCandidate` 对联想候选的语义裁决（隐藏=本会话不再出该词）。
  3. `[步骤 3 - 配置与测试]`：`[assoc]` 投影 + 状态机测试（跃迁矩阵钉死）+ X11 档手验（真窗口呈现与选择）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：联想态跃迁矩阵全绿；默认关零变化（冒烟 202 例全绿）；断链条件逐条测试。
  - [ ] 物理产物验证：X11 档真实会话：commit「中国」后候选窗出现联想候选、数字键上屏、按键断链；截图证据入 `results/runs/`。
  - [ ] 编译与旧功能回归：四门禁零警告；`just ci` 全绿。

---

## Phase 4 汇总

| 任务 | 能力 | 通道 | 工时 | CP |
|---|---|---|---|---|
| TASK-4.01.01 契约底座（ADR-0017） | GAP-22 底座 | A | 1.5 | 是 |
| TASK-4.01.02 体积治理 | GAP-21 | C | 3 | 是 |
| TASK-4.02.01 名次衰减与供给阶梯 | GAP-20 | A | 2 | 否 |
| TASK-4.03.01 上文带与 pair 重估 | GAP-20 | A | 2.5 | 是 |
| TASK-4.04.01 联想三源引擎侧 | GAP-22 | A | 3.5 | 是 |
| TASK-4.05.01 联想会话接线 | GAP-22 | B | 2 | 是 |

**波次建议**：W1 = {4.01.01, 4.01.02, 4.02.01}（三卡文件不相交，可三路并行）；W2 = {4.03.01, 4.04.01}（分别依赖 W1 的治理与契约）；W3 = {4.05.01}。出口准则：holdout top1 门禁值只升不降；联想端到端在 X11 档可演示；`just ci` 全绿。
