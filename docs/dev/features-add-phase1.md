# dev-features-add — Phase 1 增量原子任务分片

> 所属主控文档: [./features-add.md](features-add.md) ｜ 关联对比报告: [./compare-and-improve.md](compare-and-improve.md) ｜ 阶段定位: Phase 1 (P0 核心增强：排序质量跃升) ｜ 包含任务区间: TASK-1.01.01 ~ TASK-1.04.02（9 卡，预估 21.5 人天）
>
> 阶段目标：把 top1 73.8% 的三大损耗源（长尾噪声、一元排序上限、用户无衰减）与最低成本的置顶能力一次性补齐。全部任务落在 decode 边界之内，`UiFrame`/C ABI wire 零变更（主控 §3.3）。

---

#### TASK-1.01.01 dictc 权重分层归一化管线

- **基本属性**：
  - 绑定差距与改进项：`GAP-01` / `IMP-02`（另叠加 `IMP-15` 的规则前提）｜ 关联设计假设：`ASM-A-02`、`ASM-A-03`、`ASM-A-08`
  - 优先级与复杂度：`P0 核心增强` ｜ 高 ｜ 预估工时： 3 人天
  - 前置依赖：`无`
  - 关键路径：`是`（CP-2 起点）
  - 并行通道：`Track C Infra·DevOps`
  - 对标参考锚点 (Benchmark Reference)：rime-ice 三层归一化——`remarks/rime-ice/cn_dicts/base.dict.yaml`（两字词真实语料词频 1~1,926 万）与 `ext.dict.yaml`/`tencent.dict.yaml`（3+ 字词全部压平为 100）；词长梯度数据见 [rime-ice.md](../compare/rime-ice.md) §2.2~§2.3
  - 本项目代码落地锚点 (Code Anchor)：新增 `xtask/src/dictc/normalize.rs`；修改 `xtask/src/dictc/build.rs:115`（`compile` 管线内、L3c 纠音之后、五段写出之前插入归一化 pass）；`xtask/src/main.rs` 注册 `mod normalize`（模块根文件归主代理）
  - 引入依赖白名单：`无`
  - 当前状态：`[x] 已完成`
- 落地证据：xtask/src/dictc/normalize.rs (test_normalize_two_char_true_freq_is_kept, test_normalize_core_clamp_caps_the_head, test_normalize_long_words_are_flattened, test_normalize_low_freq_two_char_drops_into_the_tail)；build.rs 归一化 pass 接线于纠音后、五段写出前
- **目标与价值**：jieba 词频是单一真实分布（1 ~ 千万级）直接进 Q8.8 排序，3+ 字长尾词的频次噪声与两字头部真频同权竞争，是 top1 73.8% 的重要损耗源。本卡在 dictc 编译期把权重划分成「两字真频带 / 3+ 字压平带 / 字表真频带」三层，并校验词长梯度——**容器格式零变更**，排序输入质量直接提升。rime-ice 用同样的数据策略把「同音先短词」做成了数据保证而非引擎逻辑。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：`compile()` 的词条收集完成后、`prob_q12` 映射（`build.rs:308`）之前，按词长与来源对权重归一化：

    ```rust
    /// The frequency band a word lands in after normalization. The band decides
    /// whether the source frequency is kept verbatim or replaced by a constant,
    /// mirroring rime-ice's base/ext split (true corpus frequency for short
    /// words, presence-only for the long tail).
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum WeightBand {
        /// Single characters: real character frequency from the L1 source.
        CharFreq,
        /// Two-character words from a real-corpus source: keep the frequency.
        CoreTrueFreq,
        /// Three or more characters: flattened to the band constant.
        TailFlat,
    }

    /// Normalization rules; the defaults follow rime-ice's published practice
    /// (tail flat = 100) and are overridable per run for tuning sweeps.
    #[derive(Clone, Copy, Debug)]
    pub struct NormalizeRules {
        /// The constant every tail-band word receives.
        pub tail_flat_weight: u32,
        /// Two-character words at or below this source frequency drop into the
        /// tail band: below it the frequency is noise, not signal.
        pub core_min_freq: u32,
        /// Clamp that keeps any surviving core frequency from dominating the
        /// Q8.8 mapping (the compile-time counterpart of `prob_q12`'s ceiling).
        pub core_max_weight: u32,
    }

    impl Default for NormalizeRules {
        fn default() -> Self {
            Self { tail_flat_weight: 100, core_min_freq: 20, core_max_weight: 2_000_000 }
        }
    }

    /// The per-band audit the quality report prints; deterministic order.
    #[derive(Default)]
    pub struct NormalizeReport {
        pub char_freq: u32,
        pub core_true_freq: u32,
        pub tail_flat: u32,
    }

    /// Normalizes `entries` in place. Pure and order-stable: equal inputs give
    /// byte-identical containers, which the double-compile sha256 gate asserts.
    pub fn normalize_weights(
        entries: &mut [(u32, crate::dictc::build::RawWord)],
        rules: NormalizeRules,
        report: &mut NormalizeReport,
    );
    ```

  - **边界契约**：无跨边界变更（编译期 pass，产物仍是 v1/v2 容器）。`NormalizeReport` 并入 `dictc quality` 的既有 `Origin` 审计通道输出。
  - **交互与工艺级规范**：`xtask dictc` 新增 CLI 覆盖项 `--tail-flat-weight <u32>`、`--core-min-freq <u32>`（clap 既有参数风格）；不指定时用 `Default`。归一化统计行进 stdout 结果区（xtask CLI 输出豁免区）。
  - **与现有代码的无缝衔接方案**：`build.rs::compile` 在 L3c 纠音（`polyphone` 应用）之后调用 `normalize_weights`，再进入既有的五段写出；`band_threshold`（`build.rs:322`）的频率带逻辑保持不变——归一化发生在带划分**之前**，两者正交。
- **底层与非功能约束 (NFR)**：
  - 编译时长增量 ≤ 5%（基线 9.3s，门禁 ≤ 9.8s；pass 为单次 O(n) 遍历）；双编译 sha256 一致性门禁不得回退（确定性硬契约）。
  - `dictc/budget.rs` 段份额红线不放宽；归一化不得改变词条总数与键数 ±1%（只改 weight 与个别低频词的层归属，不改集合）。
  - 容错：`core_max_weight` < `core_min_freq` 等非法组合在参数解析期报错退出（xtask CLI 豁免区允许 stderr 用法错误），不产生半成品容器（沿用原子写）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 新建 `normalize.rs`：`WeightBand`/`NormalizeRules`/`NormalizeReport` + `normalize_weights`（含单测：两字真频保留、3+ 字压平、低频两字下沉、参数非法报错；命名 `test_normalize_<scenario>_<expectation>`）。
  2. `[步骤 2]` `build.rs::compile` 插桩调用 + `main.rs` 注册模块 + clap 参数接线；跑全量编译，确认 15.62MiB 容器各段份额与 sha256 双编译一致性。
  3. `[步骤 3]` 用 6,000 条留出集跑 `xtask quality` 对比归一化前后 top1（预期 ≥ +1.5pp），把对照数字写进卡片验收记录。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p xtask`（normalize 单测 + 既有 dictc 测试全绿）；`cargo test --workspace --doc` 不回退。
  - [ ] 全量编译实测：词条/键数变化 ≤1%、时长 ≤9.8s、双编译 sha256 一致、`budget.rs` 段份额全过。
  - [ ] 质量对照：`xtask quality` 输出归一化前后 top1 对照（写进验收记录）；与旧功能回归走查（`--input data/raw/base.tsv` 开发子集路径仍可用）。

---

#### TASK-1.01.02 质量门升级：分层 top1 审计与权重重调

- **基本属性**：
  - 绑定差距与改进项：`GAP-01` / `IMP-02`（与 `IMP-19` 的质量基线衔接）｜ 关联设计假设：`ASM-A-02`、`ASM-A-03`
  - 优先级与复杂度：`P0 核心增强` ｜ 中 ｜ 预估工时： 2 人天
  - 前置依赖：`TASK-1.01.01`（归一化管线先落地，审计才有分层对象）
  - 关键路径：`是`（CP-2：质量门是数据杠杆链的收口）
  - 并行通道：`Track C Infra·DevOps`
  - 对标参考锚点 (Benchmark Reference)：rime-ice 的数据工程纪律——`others/script/` Go 校验链 + 基于 `rime_api_console` 的端到端冒烟（[rime-ice.md](../compare/rime-ice.md) §8）；本项目既有 `dictc/quality.rs:349` `measure<L: Lexicon>` 留出集测量
  - 本项目代码落地锚点 (Code Anchor)：修改 `xtask/src/dictc/quality.rs`（`QualityReport` 增词长分带 top1）、`xtask/src/tune.rs:119`（`run` 的搜索空间接入新默认权重）、留出集再生成脚本 `xtask/src/bin/`（若拆分为 holdout 子命令则改 `main.rs` 分派）
  - 引入依赖白名单：`无`
  - 当前状态：`[x] 已完成`
- 落地证据：xtask/src/dictc/quality.rs report 模块 Top1Band/LengthBandAccuracy 词长分带 top1 审计；xtask/src/tune/holdout.rs + tune/tests.rs (test_generate_holdout_is_stratified_attributed_and_reproducible, test_generate_holdout_writes_rows_that_avoid_the_evaluation_set)
- **目标与价值**：归一化与后续 bigram/词源改动都需要「可回归的排序质量基线」。现状 `quality.rs` 只报总 top1/top9，无法定位「两字词层 vs 长尾层」各自的损耗，调参（`xtask tune`）也缺少分带反馈。本卡把质量门升级为分带审计 + 门禁值上移，是 CP-2 的收口动作。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：

    ```rust
    /// Top-1 accuracy attributed by word length. The bands mirror the
    /// normalization bands so a regression can name its layer instead of
    /// surfacing as an anonymous drop in the total.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct LengthBandAccuracy {
        /// Single-character cases.
        pub single: Band,
        /// Two-character cases (the core band).
        pub two: Band,
        /// Three-or-more-character cases (the flattened tail).
        pub long: Band,
    }

    #[derive(Clone, Copy, Debug, Default)]
    pub struct Band {
        pub cases: u32,
        pub first: u32,
    }

    impl Band {
        /// Top-1 rate within the band; `None` when the band is empty so the
        /// report never prints a misleading `0%` for an unpopulated layer.
        pub fn top1(self) -> Option<f64> {
            (self.cases > 0).then(|| f64::from(self.first) / f64::from(self.cases))
        }
    }
    ```

    `QualityReport` 增字段 `length_bands: LengthBandAccuracy`；`measure` 按 `HoldoutCase` 期望文本的 `char_count` 分带累计（`DictEntry::char_count` 已在词条里，零成本）。
  - **边界契约**：无。`xtask quality` 的 stdout 报告与 `--json` 输出为 CLI 豁免区。
  - **交互与工艺级规范**：`xtask quality` 报告新增 `top1 by length: 1字 xx.x% | 2字 xx.x% | 3+字 xx.x%` 行；JSON schema 增 `length_bands` 对象（向后兼容：新增键，不改既有键）。
  - **与现有代码的无缝衔接方案**：分带逻辑挂在 `measure` 的既有累计循环里；`xtask tune` 的打分函数复用 `measure`，因此分带天然进入调优器反馈。新默认权重仍走既有落地机制（`lm/score.rs:169-171` 注释所述「tuning run replaces」通道），确定性测试（`viterbi/mod.rs:212-227` 字节指纹）必须保持绿。
- **底层与非功能约束 (NFR)**：
  - 门禁值上移：`xtask quality` 退出码判据改为「总 top1 ≥ 78% 且两字带 top1 不低于归一化前实测值」（门禁值只升不降，主控 §4.3）。
  - `measure` 仍为纯函数：不读时钟/环境，holdout 全内存；分带不增加分配（Band 为 Copy 结构）。
  - 调优运行的随机性（若有）必须种子化，保证两轮 tune 结果可复现。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `quality.rs` 增加 `LengthBandAccuracy` 与分带累计；补单测（空带返回 `None`、分带计数守恒等于总 cases）。
  2. `[步骤 2]` 在 `TASK-1.01.02` 前置产物（归一化容器）上重测：记录总/分带 top1 基线；运行 `xtask tune` 产出新权重并用既有机制落地；重测确认提升后把新门禁值写进本卡验收记录。
  3. `[步骤 3]` 回归防护：`lm_holdout.tsv` 留出集再生成（排除已进入词库的泄漏项，沿用既有生成器规则），`budget --validate` 与四门禁全绿。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p xtask`（分带审计单测 + tune 回归）全绿；四门禁（fmt/clippy/nextest/doc）零警告通过。
  - [ ] 质量门禁：`xtask quality` 总 top1 ≥ 78%，分带报告三行齐全，数字写入验收记录。
  - [ ] 回归走查：确定性字节指纹测试、`budget --validate`、既有 holdout 用例无回归。

---

#### TASK-1.02.01 用户词频库 schema v2（dee/tick）与打开时迁移

- **基本属性**：
  - 绑定差距与改进项：`GAP-02` / `IMP-01` ｜ 关联设计假设：`ASM-A-04`、`ASM-A-06`、`ASM-A-09`、`ASM-A-10`
  - 优先级与复杂度：`P0 核心增强` ｜ 高 ｜ 预估工时： 2.5 人天
  - 前置依赖：`无`（纯 ime-dict 内部；不依赖 1.01.x）
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine`
  - 对标参考锚点 (Benchmark Reference)：librime `algo/dynamics.h:6-15`（`formula_d` 指数衰减 + `formula_p` 概率映射）与 `user_db.cc:22-26` 的 `c=<commits> d=<dee> t=<tick>` 记录形态；逻辑时钟见 [librime.md](../compare/librime.md) §5.2
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-dict/src/user_db.rs`（表定义 :176-184、水合结构、`record`/`freq`/`commit` 路径、打开时迁移）；`user_db/backup.rs` 备份兼容确认（只读整文件拷贝，无需改）
  - 引入依赖白名单：`无`
  - 当前状态：`[x] 已完成`
- 落地证据：crates/ime-dict/src/user_db.rs schema v2 (dee/tick) + user_db/open.rs 打开时迁移；user_db/tests/migration.rs (test_user_db_migrates_v1_rows_once_and_preserves_their_values, test_user_db_migrates_an_empty_v1_store_without_writes, test_a_migrated_store_hydrates_and_lists_its_rows, test_user_db_failed_v1_migration_degrades_readonly_and_can_retry)
- **目标与价值**：现状 `user_words: &str -> (u32 commits, u64 last_used_ms)`（`user_db.rs:17,176`）单调递增，多年前的词与本周的词同权。本卡把存储升级为带**衰减累加器 dee 与逻辑时钟 tick** 的 v2 记录，为 `TASK-1.02.02` 的衰减数学提供数据基座。librime 用同一套 `c/d/t` 记录支撑了十年可用的动态调频，其「按音节逻辑时钟衰减」的数学是直接可移植的成熟设计。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：新表与迁移（redb 表宽度固定，加字段必须换表；旧文件迁移放在打开路径的一个写事务里，失败则原文件不动）：

    ```rust
    /// V2 record: the v1 pair plus the decay accumulator and the logical tick
    /// it was last touched at. `dee` is persisted as `f32` bits -- a storage
    /// encoding only; all scoring arithmetic happens in integer fixed point
    /// (0.4 rule: f32 must not drive ordering decisions).
    ///
    /// Migration folds no history: a v1 row has no tick trace, so it enters
    /// v2 with `dee = commits as f32` and `tick = 0`, which the fold treats as
    /// "aged by the whole clock" -- the same answer v1 ranking gave, modulo
    /// the decay that v1 never applied.
    const USER_WORDS_V2: TableDefinition<&str, (u32, u64, u32)> =
        TableDefinition::new("user_words_v2");

    /// Meta row holding the global logical clock. It advances once per
    /// `record` batch commit, not per wall-clock second: like librime's
    /// `/tick`, it measures usage, not time.
    const META_TICK: TableDefinition<&str, u64> = TableDefinition::new("tick");
    ```

    水合结构：

    ```rust
    /// One hydrated user record. `dee_q16` is the decay accumulator in Q16.16
    /// fixed point; `tick` is the record's own logical stamp.
    #[derive(Clone, Copy, Debug)]
    pub(crate) struct UserRecord {
        pub commits: u32,
        pub dee_q16: i64,
        pub tick: u64,
    }
    ```

  - **边界契约**：`UserFreqSource` trait **零变更**（`freq/record` 签名不动；`is_user_word` 仍为 false——造词是 `TASK-3.04.01` 的域）。新增诊断码 `data/user-db/schema-migrated`（info：迁移发生；warn：迁移失败降级只读），登记进主控 §3.3-4。
  - **交互与工艺级规范**：Headless 契约——迁移日志一行、`0600/0700` 权限不变、`readonly` 降级路径复用 `recover.rs:75` 既有码 `data/readonly-mode`。
  - **与现有代码的无缝衔接方案**：`UserDb::open` 在既有校验（schema_version meta 行）后插入迁移步：`meta.schema_version == 1 → 单写事务{建 v2 表 + 逐行拷贝 + 写 META_TICK=0 + 置 version=2}`；`record` 改为推进 META_TICK（批次级别 +1，维持 ≤5µs 契约——原子加 + 内存 map 更新，无 IO）；`commit`/`final_commit` 的 delta 序列化改为 v2 三元组；`evict`（:538 起）的「最老排序」从 `last_used_ms` 改为 `tick`。
- **底层与非功能约束 (NFR)**：
  - `record` ≤ 5µs 契约不放宽（criterion 基准 `userdb/record` 断言保持）；`freq` 命中路径仍不开读事务、不持写锁（水合态内存读）。
  - 兼容矩阵：v1 文件（含 50 万行满库）打开 → 迁移成功且行数守恒；损坏文件 → 既有 quarantine 路径优先于迁移；`user.redb` 权限/目录位不变。
  - 失败回滚：迁移事务失败 → 报 `data/user-db/schema-migrated`(warn) + 以 v1 只读降级继续服务（`ASM-A-09`），绝不 panic、绝不丢文件。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 表定义与 `UserRecord` 水合结构改造；迁移函数（含 v1 满库、空库、重复打开幂等三组单测）。
  2. `[步骤 2]` `record`/`commit`/`final_commit`/`evict`/`list`/`export_tsv` 全链路切换 v2（export 格式增 `dee/tick` 列并在头注释登记版本）。
  3. `[步骤 3]` 异常分支：迁移中断（模拟事务失败）→ 只读降级断言；备份恢复（`backup.rs` 的 v1 备份在 v2 代码下恢复 → 触发迁移而非报错）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-dict`（迁移幂等、行数守恒、record ≤5µs 基准、readonly 降级、备份↔迁移互操作）；`cargo bench -p ime-dict` 的 `userdb/*` 用例不回退。
  - [ ] 兼容走查：手造 v1 文件（ fixtures）→ 打开 → v2 迁移 → 关闭重开不再迁移（幂等诊断只报一次）。
  - [ ] 回归走查：`DecodeFlags::USER_DICT` 排序翻转既有对照测试（`viterbi/mod.rs:426-440`）在 v2 下语义不变（无衰减时等价旧行为，由 `TASK-1.02.02` 的门禁再次确认）。

---

#### TASK-1.02.02 衰减数学与打分集成（formula_d/p 定点移植）

- **基本属性**：
  - 绑定差距与改进项：`GAP-02` / `IMP-01` ｜ 关联设计假设：`ASM-A-04`、`ASM-A-06`、`ASM-A-02`（独占预算 ≤0.10ms）
  - 优先级与复杂度：`P0 核心增强` ｜ 高 ｜ 预估工时： 3 人天
  - 前置依赖：`TASK-1.02.01`（v2 记录是输入）
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine`
  - 对标参考锚点 (Benchmark Reference)：librime `algo/dynamics.h` 全文（`formula_d(d,t,da,ta)=d+da·exp((ta−t)/200)`；`formula_p` 概率映射；`dee ≤ 1e-200` 丢弃）与 `user_dictionary.cc:535-574` 的读侧折叠（[librime.md](../compare/librime.md) §5.2）
  - 本项目代码落地锚点 (Code Anchor)：新增 `crates/ime-dict/src/user_db/decay.rs`；修改 `crates/ime-dict/src/user_db.rs`（`freq` 惰性折叠）、`crates/ime-core/src/lm/score.rs:250-257`（user 项输入说明与饱和注释）
  - 引入依赖白名单：`无`（exp 用分段查表近似，与既有 `log2_q8` 同手法，不引入 `libm`）
  - 当前状态：`[x] 已完成`
- 落地证据：crates/ime-dict/src/user_db/decay.rs (test_fold_half_life_is_exact_to_one_half, test_fold_is_monotonic_and_bounded_by_the_floor, test_fold_treats_a_rewound_tick_as_unaged, test_fold_saturates_an_extreme_tick_gap_at_the_floor)；user_db/tests.rs (test_user_db_decay_sinks_an_aged_word_below_a_fresh_one, test_user_db_idle_fold_advances_a_flushed_record_to_the_current_tick)
- **目标与价值**：给用户词频加上「按使用量衰减」的时间维：久未上屏的词权重自然回落，新词能浮上来——这是真实使用中最先被感知的排序劣化的根治。librime 的衰减数学（半衰期 ≈139 次上屏）经过十年验证，本项目把它移植为**整数定点**实现，遵守 3.8 的「f32 不得驱动排序」约束。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：

    ```rust
    /// Decay parameters. `half_life_ticks` defaults to 139: the fixed-point
    /// equivalent of librime's `exp((ta-t)/200)` e-folding constant, whose
    /// half-life is 200*ln(2). Tunable through `data.decay.*` (see 1.02.03).
    #[derive(Clone, Copy, Debug)]
    pub struct DecayParams {
        pub half_life_ticks: u32,
        /// Accumulator floor in Q16.16: below it the record stops
        /// contributing (librime drops at 1e-200; the floor exists so the
        /// integer domain saturates instead of flushing to zero silently).
        pub floor_q16: i64,
    }

    impl Default for DecayParams {
        fn default() -> Self {
            Self { half_life_ticks: 139, floor_q16: 1 << 16 } // dee == 1.0
        }
    }

    impl DecayParams {
        /// Folds a record forward to `now`: returns the decayed accumulator
        /// in Q16.16. `decay = dee * 2^(-(now-tick) / half_life)` evaluated
        /// with a 256-entry table of `2^(-k/4096)` plus an exponent shift --
        /// the mirror image of `lm/score.rs`'s `log2_q8`, error < 1/256.
        pub fn fold(&self, dee_q16: i64, tick: u64, now: u64) -> i64;

        /// Maps a decayed accumulator and the commit count to the integer
        /// frequency the existing user term consumes (`lm/score.rs:250-257`
        /// keeps its shape; only the input's meaning changes from "raw
        /// commits" to "decay-weighted usage"). Saturating, never panics.
        pub fn effective_freq(&self, commits: u32, dee_q16: i64) -> u32;
    }
    ```

    `UserDb::freq` 的水合态读路径：`elapsed == record.tick 对齐` 时直接返回缓存；否则 `fold` 一次并把 `(dee_q16, tick)` 物化回内存记录（**惰性一次性折叠**：重复解码 O(1) 查缓存，写盘仍走既有批量 delta）。首次折叠后与「从没衰减过」的行为差异由对照测试钉死。
  - **边界契约**：`UserFreqSource::freq` 返回值语义从「commits」变为「衰减加权使用量」——同一 trait、同一类型（`u32`），语义变化登记在本卡与主控 §3.3；`ime-core` 侧 `score.rs` 不改公式只改注释。
  - **交互与工艺级规范**：无 UI 面。诊断：`xtask report` 的用户库小节增显 `decay: half_life=139, floored=N` 计数（N = 触底停用词数）。
  - **与现有代码的无缝衔接方案**：`freq` 折叠逻辑内联在既有水合读路径（`user_db.rs` 模块注释承诺的「decode 永不开读事务」保持）；`ime-core` 的 mock `UserFreqSource`（测试 double）补一个可注 tick 的变体供确定性测试。
- **底层与非功能约束 (NFR)**：
  - decode 独占预算 ≤ 0.10ms（主控 §3.4）：折叠是首次访问一次性成本，稳态为零新增；criterion 基准 `decode/user_decay_cold`（首访折叠）与 `decode/user_decay_warm`（缓存命中）双断言。
  - 确定性：同一 `(dee, tick, now)` 序列在任何运行下产出同一排序（定点 + 无 HashMap 迭代依赖）；`f32` 只存在于存储位型。
  - 边缘条件：`tick` 回绕/倒退（外部改库）→ `elapsed` 取饱和 0，不 panic；`half_life_ticks = 0` 非法配置 → 修默认 + `config/invalid` 诊断（`TASK-1.02.03` 的配置键接线）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `decay.rs`：表驱动的 `fold`（与 `log2_q8` 同构的 `exp2` 逆运算表 + 指数移位）与 `effective_freq`；性质测试：单调性（elapsed↑ → freq↓）、半衰点准确性（elapsed = half_life → 衰减 ≈ 1/2，误差 < 1/256）、饱和与触底。
  2. `[步骤 2]` `UserDb::freq` 惰性折叠接入 + `record` 批次推进 META_TICK；对照测试：同一 commits 序列在「有衰减/无衰减（half_life 超大）」两档下的排序差异符合预期。
  3. `[步骤 3]` 与 `ime-core` 集成：mock double 增 tick 注入口；`viterbi` 排序翻转测试扩展出「衰减后翻转回退」用例（老词沉底）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-dict -p ime-core`（fold 性质组、freq 折叠幂等、排序对照）；criterion `decode/user_decay_*` 预算断言进 `budgets.json` 读取路径。
  - [ ] 基准证据：`cargo bench -p ime-dict` 冷/热两组数字写入验收记录（冷 ≤ 0.10ms 独占预算内，热 ≈ 0 增量）。
  - [ ] 回归走查：无用户词时排序与现状逐字节一致；readonly 模式下折叠只在内存、不产生写事务。

---

#### TASK-1.02.03 衰减配置键与空闲批量重算

- **基本属性**：
  - 绑定差距与改进项：`GAP-02` / `IMP-01` ｜ 关联设计假设：`ASM-A-04`、`ASM-A-06`
  - 优先级与复杂度：`P0 核心增强` ｜ 低 ｜ 预估工时： 1.5 人天
  - 前置依赖：`TASK-1.02.02`
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine`
  - 对标参考锚点 (Benchmark Reference)：librime 的衰减参数内置于公式；本项目把参数外化为配置（对齐 ime-config 的「坏值修默认」哲学）——无直接对标代码，属本地化适配
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-config/src/schema.rs` + `reload.rs`（`[data]` 节新增 `decay_half_life_ticks`/`decay_floor`）+ `DEFAULT_CONFIG_TOML` 模板；修改 `crates/ime-dict/src/user_db.rs`（`evict` sweep 内批量前向折叠）
  - 引入依赖白名单：`无`
  - 当前状态：`[x] 已完成`
- 落地证据：crates/ime-config/src/schema/tests.rs (test_data_section_reads_the_decay_keys_from_a_document, test_data_section_repairs_decay_values_a_document_writes_out_of_range)；user_db/tests.rs evict 批量前向折叠 (test_user_db_evicts_the_oldest_records, test_user_db_eviction_forgets_the_evicted_counts)
- **目标与价值**：让衰减行为可调（重度用户可加快遗忘、轻量用户可加长记忆），并保证 on-demand 懒折叠的代价有界：空闲期 sweep 把磁盘上的 dee/tick 前向折叠到接近当前 tick，单次懒折叠的 `elapsed` 永远有限。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：`[data]` 节新增键（三态标注「reload 生效」）：

    ```toml
    [data]
    # Logical-clock half-life of user-word decay, in commits. 139 matches the
    # librime e-folding constant; 0 or negative-valued input is repaired to the
    # default with one `config/invalid` diagnostic.
    decay_half_life_ticks = 139
    ```

    sweep 扩展：既有空闲 sweep（30s 一轮、每轮 10%、批 4096）在淘汰扫描前对当批记录执行 `fold(now)` 并入写 delta——复用既有 flush 线程与触发器，不新增线程（`ASM-A-06`）。
  - **边界契约**：配置投影走既有 `ConfigStore` Arc 替换；会话保旧配置的组合语义不变（0.4 规则 10：配置重载不重置进行中会话——衰减参数只影响后续折叠）。
  - **交互与工艺级规范**：Headless——模板注释英文、键语义与 `reload.rs` 三态标注体系一致。
  - **与现有代码的无缝衔接方案**：`UserDb` 持有 `DecayParams`（open 时从投影后的 config 注入；`TASK-1.02.02` 的默认构造改为「参数从外部传入」）；sweep 改动点在既有 `evict` 函数体内，weak-handle 纪律不变。
- **底层与非功能约束 (NFR)**：
  - sweep 单轮时长增量 ≤ 20%（基准 `userdb/sweep` 有既有断言）；宿主线程零 involvement。
  - 坏值修复：`decay_half_life_ticks = 0` → 默认 139 + 一条 `config/invalid`；模板与 `Config::default` 防漂移测试同步。
  - 边缘条件：折叠批与用户 `record` 并发 → 既有 delta 单消费者顺序保证（ASM-11）不受影响。
- **逐步落地实施步骤**：
  1. `[步骤 1]` config schema/reload/模板三处 + 防漂移测试扩展。
  2. `[步骤 2]` sweep 批量折叠接入 + 单测（折叠后磁盘 tick 距 now ≤ 1 轮步长；淘汰排序仍按 tick）。
  3. `[步骤 3]` `xtask report` 看板补 decay 摘要行；回归既有 readonly/slow-disk 路径。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-config -p ime-dict` 全绿；模板防漂移测试更新。
  - [ ] 基准证据：`userdb/sweep` 前后对照写入验收记录。
  - [ ] 回归走查：`config/invalid` 坏值修复、readonly、slow-disk 三个既有行为用例不回退。

---

#### TASK-1.03.01 容器 v2：BIGRAM 段生成管线与读端兼容

- **基本属性**：
  - 绑定差距与改进项：`GAP-03` / `IMP-03`（CP-1 起点）｜ 关联设计假设：`ASM-A-03`、`ASM-A-05`、`ASM-A-09`、`ASM-A-10`
  - 优先级与复杂度：`P0 核心增强` ｜ 高 ｜ 预估工时： 4 人天
  - 前置依赖：`TASK-1.01.01`（**文件串行声明**：两者都编辑 `xtask/src/dictc/build.rs`，无逻辑依赖，仅按 AGENTS §6.3 同文件须串行）
  - 关键路径：`是`（CP-1）
  - 并行通道：`Track A Core·Engine`（容器/读端）+ `Track C`（dictc 生成）
  - 对标参考锚点 (Benchmark Reference)：librime `.table.bin` 的 log 域词频存储与四级索引思想（[librime.md](../compare/librime.md) §4.2）；librime-octagram 的「Grammar 槽 + 插件注入」形态（本卡只取「bigram 概率表」这一最小数据形态，不引模型文件）
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-dict/src/format/mod.rs`（v2 header/flag 语义、`BigramRecord`）、`crates/ime-dict/src/format/reader.rs`（v2 验收 + v1 永久兼容）；新增 `xtask/src/dictc/bigram.rs`（共现统计与段生成）；修改 `xtask/src/dictc/build.rs`（接线一行）、`xtask/src/dictc/budget.rs`（Bigram scope 份额启用，`budget.rs:196` 的 scope 已存在）、`data/sources.toml` + `xtask/fetch.sh`（语料源登记）
  - 引入依赖白名单：`无`（语料切分用自编译词表做最大正向匹配，不引入分词 crate）
  - 当前状态：`[x] 已完成`
- 落地证据：crates/ime-dict/src/format/reader/ v2 BIGRAM 段验收 (test_parse_rejects_a_bigram_section_in_a_version_one_container, test_round_trip_reads_back_every_section_byte_for_byte)；xtask/src/dictc/bigram.rs (test_count_cooccurrence_segments_known_adjacent_words_and_skips_unknown_text, test_build_records_cuts_quantizes_and_sorts_deterministically, test_build_records_honours_the_score_floor_and_empty_input)
- **目标与价值**：`DictLm::bigram` 恒 miss（`fst_index/dict_lm.rs:207-214`）把排序上限钉死在一元。本卡交付 BIGRAM 段的完整生产链：语料 → 自词表切分 → 相邻共现计数 → 按 PMI·count 裁剪 top-N → `(pair, prob_q12)` 有序段 → v2 容器。这是 CP-1 的重活，也是长句 top1 的最大单杠杆。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：段记录与读取视图：

    ```rust
    /// One bigram observation: a (previous, next) word pair and the
    /// conditional probability of the next word given the previous one, in
    /// Q12 (`PROB_Q12_MAX = 4096`, the unigram table's own scale). Records are
    /// sorted by `pair_key` so the reader answers a lookup with one binary
    /// search and no allocation.
    #[derive(Clone, Copy, Debug)]
    #[repr(C)]
    pub struct BigramRecord {
        /// `hash(prev) << 32 | hash(word)`; the 64-bit key keeps the sort
        /// total and the search branch-predictable.
        pub pair_key: u64,
        pub prob_q12: u16,
        pub _pad: u16,
    }
    ```

    容器版本语义（`format/mod.rs`）：header `version = 2`；`FLAG_BIGRAM_PRESENT`（:84-85）置位当且仅当 BIGRAM 段 `len > 0`；**段表布局不变**（BIGRAM 是六个固定 kind 之一，v1 恒空段）。读端规则：`version == 1` → BIGRAM 段必须为空（违反即 `dict/corrupt`）；`version >= 2` → 段可为空（flag 不置位，降级路径）或为合法记录表（升序、8 字节对齐、CRC 过）。

    dictc 生成（`bigram.rs`）：

    ```rust
    /// Bigram generation knobs; the budget module already carries a Bigram
    /// section scope, and `max_pairs` is what keeps the section inside its
    /// 2.5 MiB share (about 200k records at 12 bytes).
    pub struct BigramConfig {
        pub max_pairs: u32,
        /// Minimum count·PMI a pair needs to survive the cut. Pairs below it
        /// are the long tail the unigram already models.
        pub min_score: f32,
    }

    /// Counts adjacent word co-occurrences over the corpus, segmenting each
    /// line with a maximal-forward match against the very word list being
    /// compiled -- the compiler is its own segmenter, so no tokenizer
    /// dependency enters the build. Unknown characters are skipped, never
    /// guessed.
    pub fn count_cooccurrence(corpus: &str, words: &WordIndex) -> BTreeMap<u64, u32>;

    /// Cuts, scores (count * PMI) and quantizes the surviving pairs into the
    /// sorted record table the section writer emits.
    pub fn build_records(counts: BTreeMap<u64, u32>, cfg: BigramConfig) -> Vec<BigramRecord>;
    ```

  - **边界契约**：新诊断码 `dict/bigram/absent`（`DictLm` 装配时段缺失/flag 未置位，warn 每进程一次，miss 降级）。`ime-types` 零变更。
  - **交互与工艺级规范**：语料治理按 **ADR-0012 决策 2（已裁决，2026-10-14）**执行——语料**不进** `sources.toml` 词源白名单（该校验器的宽松 SPDX 语义不扩展），走 ADR 登记 + `fetch.sh` 下载 + sha256 回填 ADR 登记表 + 语料目录 gitignore + `NOTICE`/`licenses.md` 署名；仓库只允许存在 `kind="derived"` 的共现计数表。语料不可得时整卡交付「段格式 + 读端 + 生成器 + 空段降级」，flag 不置位（W-8 退出路径）。
  - **与现有代码的无缝衔接方案**：`build.rs::compile` 在五段写出前调 `bigram::build_records`（若语料文件存在）；`prob_q12`（`build.rs:308`）复用为条件概率量化器；`Reader::open` 的逐段 CRC 校验天然覆盖新段；`dictc/budget.rs` 的 Bigram scope 启用份额断言。
- **底层与非功能约束 (NFR)**：
  - 段体 ≤ 2.5MiB（`ASM-A-03`），由 budget scope 强制；超限先降 `max_pairs`/升 `min_score`，不动红线。
  - 编译时长：语料切分为编译期离线步骤，目标全量编译（含 bigram）≤ 90s 既有门禁不变；内存峰值 ≤ 512MB（计数器用 `BTreeMap<u64,u32>`，可预估条目数）。
  - 确定性：同一语料 + 同一词表 → 段字节级一致（双编译 sha256 门禁覆盖 BIGRAM 段）。
  - 读端安全：`BigramRecord` 逐字段 bounds check（对齐 `entry.rs` 的逐访问校验纪律）；offset 越界返回 `DictError` 绝不 OOB（0.4 规则 8）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 按 **ADR-0012 决策 2（已裁决）**执行语料接入：固化 zh-Wikipedia dump 的具体版本 URL → `fetch.sh` 增下载步骤（语料落 `data/raw/corpus/`，gitignore 覆盖，永不入库）→ sha256 与获取日期回填 ADR-0012 登记表 → `NOTICE`/`docs/dev/licenses.md` 署名（W-6）。
  2. `[步骤 2]` `format/mod.rs` + `reader.rs` 的 v2 语义与 `BigramRecord` 读取视图（单测：v1 空/非法拒绝、v2 空段降级、合法段往返、越界拒绝）。
  3. `[步骤 3]` `dictc/bigram.rs` 生成器（切分、计数、PMI 裁剪、量化、写段）+ budget scope 接线；全量编译实测段体与编译时长；`DictLm` 装配侧留到 `TASK-1.03.02`（本卡只交付容器与生成）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-dict -p xtask`（段格式往返、v1/v2 兼容矩阵、生成器确定性、PMI 裁剪边界）；双编译 sha256 含 BIGRAM 段一致。
  - [ ] 实测证据：全量编译时长、段体字节数、`budget --validate` 输出写入验收记录；若语料 fetch 未完成，明确记录「空段交付」状态与后续解锁条件（ADR-0012 W-8）。
  - [ ] 回归走查：v1 `base.dict` 在新读端下逐字节可用（既有加载测试不回退）；`dict/Bigram/absent` 诊断只报一次。

---

#### TASK-1.03.02 DictLm bigram 真查与两模型同序契约

- **基本属性**：
  - 绑定差距与改进项：`GAP-03` / `IMP-03`（CP-1 收口）｜ 关联设计假设：`ASM-A-02`（独占预算 ≤0.25ms）、`ASM-A-05`、`ASM-A-10`
  - 优先级与复杂度：`P0 核心增强` ｜ 中 ｜ 预估工时： 2 人天
  - 前置依赖：`TASK-1.03.01`
  - 关键路径：`是`（CP-1）
  - 并行通道：`Track A Core·Engine`
  - 对标参考锚点 (Benchmark Reference)：librime poet 的有 LM 形态（按前词分桶束搜索，`poet.cc:134-168`）——本项目解码骨架已是 K-best Viterbi，只把 `Grammar::Query` 的等价物（bigram 查表）接进既有打分；librime.md §7 的消费点清单
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-dict/src/fst_index/dict_lm.rs`（段视图 + `bigram` 真查）、`crates/ime-dict/src/fst_index.rs`（`DictLm` 装配传段）；`crates/ime-core/src/lm/mod.rs` 的 `InMemoryLm`（测试 double 支持真 pair，保同序契约可测）
  - 引入依赖白名单：`无`
  - 当前状态：`[x] 已完成`
- 落地证据：crates/ime-dict/src/fst_index/tests.rs (test_dict_lm_bigram_hit_matches_the_in_memory_model_and_ranking, test_dict_lm_bigram_falls_back_to_the_unigram_minus_the_miss_penalty, test_dict_lm_v1_has_no_bigram_section_and_preserves_the_miss_path)；ime-core lm/ngram.rs + lm/score.rs (test_edge_score_uses_the_bigram_when_a_predecessor_is_given)
- **目标与价值**：把 `fn bigram` 从「恒 miss」换成真查表，使 5 权重中沉睡的 `bi = 154(0.6)` 通道真正生效——两字词序列获得上下文消歧（「中国/银行」类接续），长句 top1 直接受益。`LanguageModel` trait 冻结不动（`ASM-A-10`），本卡是纯实现替换。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：

    ```rust
    /// The section view: the sorted record slice plus the same Q12 -> Q8.8
    /// precomputed mapping the unigram table already uses, so both tables
    /// score on one scale and one code path.
    pub(crate) struct BigramTable {
        records: &'static [BigramRecord],
        q12_to_q8: &'static [i32; PROB_Q12_STEPS],
    }

    impl BigramTable {
        /// One binary search, no allocation. A miss returns `None`; the
        /// caller applies the miss path (below).
        pub fn prob_q8(&self, prev: &str, word: &str) -> Option<i32>;
    }
    ```

    `DictLm::bigram` 替换（miss 路径逐字保留——纠偏表 X-4 的同序契约）：

    ```rust
    fn bigram(&self, prev: &str, word: &str) -> i32 {
        match self.bigram_table.and_then(|t| t.prob_q8(prev, word)) {
            Some(q8) => q8,
            // The documented miss fallback: interchangeable with the
            // in-memory model, byte-for-byte, so a caller that swaps one for
            // the other gets the same candidate order.
            None => self.unigram(word).saturating_add(BIGRAM_MISS_PENALTY),
        }
    }
    ```

  - **边界契约**：`LanguageModel` trait 与 `bigram(prev, word) -> i32` 语义不变；装配点在 `ime-fcitx5` 会话工厂把 `DictLm` 递给解码器的既有路径（`addon.rs` INIT_STEPS 的 lexicon 步），零改动。
  - **交互与工艺级规范**：无 UI 面。`DictLm` 的 `Debug`（`dict_lm.rs:222-229`）补 `bigram_entries` 字段计数（不记录词文本，遵守脱敏第一道防线）。
  - **与现有代码的无缝衔接方案**：`sweep.rs` 已经把 `prev_word` 传入打分（`score.rs:253` 的 `bi * bigram(prev, word)` 项在调），本卡不改词格与扫描器；`InMemoryLm` 增 `with_bigrams(pairs)` 构造器，双模型同序测试对拍。
- **底层与非功能约束 (NFR)**：
  - decode 独占预算 ≤ 0.25ms：每边一次二分（`records` 升序，典型 20 万条 ≈ 18 次比较）；criterion `decode/bigram_hit` + `decode/bigram_miss` 双基准断言。
  - 同序契约：同输入下 `DictLm` 与 `InMemoryLm`（注入同一 pair 集）产出**逐字节相同**的候选序列（测试钉死，注释引用 miss 路径文档）。
  - 段缺失降级：`bigram_table == None` 时行为与 v1 完全一致（回归断言）；诊断 `dict/bigram/absent` 由本卡装配点发出。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `BigramTable` 读取视图 + 二分查找（越界/乱序输入在 `Reader` 校验期已拒绝，这里信任段校验——注释声明该前提）。
  2. `[步骤 2]` `DictLm` 装配与 `bigram` 替换；`InMemoryLm` 扩展；同序对拍测试 + hit/miss 基准。
  3. `[步骤 3]` 端到端对照：`TASK-2.06.01` 的冒烟 TSV（若已落地）或临时 `testd-engine` 用例验证「中国/银行」类接续排序改善；`xtask quality` 重测总 top1 与分带。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-dict -p ime-core`（同序对拍、miss 等价、段缺失降级）；`cargo test --workspace --doc` 不回退。
  - [ ] 基准证据：`decode/bigram_hit/miss` 数字写入验收记录（≤0.25ms 独占预算内）；`xtask quality` top1 对照（预期 ≥ +1pp，写入验收记录）。
  - [ ] 回归走查：v1 容器（无段）行为逐字节等同现状；确定性字节指纹测试保持绿。

---

#### TASK-1.04.01 置顶词引擎侧转正（PinHighlighted 闭合）

- **基本属性**：
  - 绑定差距与改进项：`GAP-04` / `IMP-04` ｜ 关联设计假设：`ASM-A-04`、`ASM-A-10`（**契约增项：需 ADR-0013 + 主代理裁决**）
  - 优先级与复杂度：`P0 核心增强` ｜ 中 ｜ 预估工时： 2 人天
  - 前置依赖：`无`（与 1.01/1.02/1.03 全并行；文件交集见白名单声明）
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `pin_cand_filter.lua:1-380`（「编码⇥词」置顶、纯重排不造词、自动派生简码键——[rime-ice.md](../compare/rime-ice.md) §4）；librime 用户词「同码长用户词优先」的排序语义（`script_translator.cc:626-633`）
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-types/src/lexicon.rs`（`UserFreqSource` 增 `set_pinned` 默认方法——**冻结契约增项**）、`crates/ime-core/src/state/transitions.rs:203-204`（`PinHighlighted` 转正）与 `crates/ime-core/src/state/effects.rs`（`Effect` 增变体）、`crates/ime-dict/src/user_db.rs`（`set_pinned` 真实现 + `freq` 置顶封顶）、`crates/ime-fcitx5/src/effects.rs`（执行器分支）
  - 引入依赖白名单：`无`
  - 当前状态：`[x] 已完成`
- 落地证据：crates/ime-types/src/lexicon.rs UserFreqSource::set_pinned 默认方法；state/tests/transitions.rs (test_pin_highlighted_arm_toggles_the_highlighted_word_for_the_input, test_pin_highlighted_arm_does_nothing_without_a_composition, test_pin_applied_event_refreshes_a_composing_session)；user_db set_pinned 真实现 + ime-fcitx5 effects 执行器分支
- **目标与价值**：置顶是「成本最低的用户可感知改进」（对比报告 P0-4）：用户对高亮候选按 Pin 键，该词在其编码下从此排第一。存储侧 `user_meta.pinned` 与内存 `pinned: Mutex<HashSet<Box<str>>>`（`user_db.rs:177,336`）已就绪，状态机与 trait 只差最后一跳。rime-ice 用 380 行 Lua 实现的语义，本项目在引擎内只需一个 effect + 一个打分封顶。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：契约增项（默认实现模式与 `list`/`export_tsv` 的 ADR-0005 先例一致——测试 double 与其他实现零负担）：

    ```rust
    /// Pins `word` as the top candidate of `key`, or unpins when `word` is
    /// `None`. Pinning is per-key and replaces any previous pin of the same
    /// key; the pinned word still participates in sentence scoring, it only
    /// gains a ceiling on its user term. The default returns
    /// [`ImeError::Unsupported`] so existing implementations (tests, future
    /// backends) keep compiling unchanged.
    fn set_pinned(&self, key: &str, word: Option<&str>) -> Result<(), ImeError> {
        let _ = (key, word);
        Err(ImeError::Unsupported)
    }
    ```

    打分封顶（`user_db.rs` 的 `freq` 读路径，零 `score.rs` 改动）：

    ```rust
    /// The frequency a pinned word reports. `u32::MAX` saturates the user
    /// term's `min(log2(1+freq)/8, 2)` at its ceiling, which lifts the word
    /// above every unpinned rival of the same key without inventing an
    /// infinite weight -- path scores stay finite and the determinism
    /// contract stays intact.
    pub(crate) const PINNED_FREQ: u32 = u32::MAX;
    ```

    状态机与效果（`transitions.rs`）：

    ```rust
    // KeyAction::PinHighlighted (Composing -> Composing): replace the
    // `Unsupported` diagnostic with a pin toggle on the highlighted
    // candidate; a second pin of the same word unpins it.
    KeyAction::PinHighlighted => self.toggle_pin(ctx),
    // effects.rs:
    //   Effect::PinCandidate { key: String, word: String }
    //   Effect::UnpinCandidate { key: String }
    ```

  - **边界契约**：跃迁 `Composing --PinHighlighted--> Composing`（幂等切换）；effect 变体属 ime-core 内部边界（非冻结契约），随 crate 同版本发布；**本卡含 ADR-0013（增量契约扩展：`set_pinned` 默认方法）的前置产出义务，未经主代理裁决不得合入**。
  - **交互与工艺级规范**：置顶词在候选列表按普通第一候选呈现（主控 §3.2，不新增视觉状态）；Pin 键位已在 `KeyName` 白名单与 cheatsheet 键位表（`cheatsheet.rs`）中登记，无需新键。
  - **与现有代码的无缝衔接方案**：`toggle_pin` 从当前高亮候选取 `(key, word)`（会话状态已有高亮索引与当前键）；宿主 `effects.rs` 把 effect 映射到 `UserDb::set_pinned`（经既有 `UserFreqSource` trait 对象）后触发会话内 re-decode（复用既有「record 后重排」路径）；`viterbi/mod.rs:426-440` 的排序翻转测试模式复用为置顶翻转测试。
- **底层与非功能约束 (NFR)**：
  - Pin 动作延迟 ≤ 一个解码周期（无新增 IO：pinned 集合在内存，落盘走 `user_meta` 既有批量通道）；宿主线程零阻塞。
  - 幂等与并发：双击 Pin → 第二次为 unpin（状态机单步语义天然保证）；`forget` 命中被置顶词的行为由 `TASK-1.04.02` 定义（本卡先落「forget 清 pin」的最小一致性：pinned 集合随删除清理）。
  - 边缘条件：对单字/整句候选 Pin 均合法（key 取当前输入键）；pinned 词不在本次词格中时 freq 封顶自然不生效（无害）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` ADR-0013 草案（`set_pinned` 契约增项）→ 主代理裁决 → `ime-types` 默认方法 + mock double 实现。
  2. `[步骤 2]` 状态机 `toggle_pin` + effect 变体 + 宿主执行器 + `UserDb` 真实现（内存集合 + `user_meta.pinned` 写路径）+ `freq` 封顶。
  3. `[步骤 3]` 测试：置顶翻转排序（复用 ：426 模式）、双击幂等、forget 清 pin、`Unsupported` 默认实现的 trait 兼容（全部现有 double 不改即编译）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-types -p ime-core -p ime-dict -p ime-fcitx5` 全绿；ADR-0013 已合入 `docs/dev/adr/`。
  - [ ] 端到端走查：`xtask testd-engine`（或沙盒会话）注入 Pin 键 → 帧镜像断言目标词升至首位；再按 → 复位。
  - [ ] 回归走查：`binding_audit` 键位审计、既有排序翻转测试、readonly 模式下 Pin 报 `data/readonly-mode` 且输入不中断。

---

#### TASK-1.04.02 置顶管理闭环（list/export/提示与一致性）

- **基本属性**：
  - 绑定差距与改进项：`GAP-04` / `IMP-04` ｜ 关联设计假设：`ASM-A-04`
  - 优先级与复杂度：`P0 核心增强` ｜ 低 ｜ 预估工时： 1.5 人天
  - 前置依赖：`TASK-1.04.01`
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `pin_cand_filter.lua` 的配置文件「编码⇥词」可读格式与使用说明传统（文件头大篇幅说明书）；librime `UserDbMerger` 之外的管理面（forget/list）语义
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-dict/src/user_db.rs`（`list`/`export_tsv` 增置顶标记）、`crates/ime-fcitx5/src/cheatsheet.rs`（键位说明补 Pin 行为）、`crates/ime-diag`（probe 计数器 `user_pins` 若探针注册表需扩展）
  - 引入依赖白名单：`无`
  - 当前状态：`[x] 已完成`
- 落地证据：crates/ime-dict/src/user_db/manage_tests.rs (test_forget_of_a_pinned_word_clears_the_pin_and_deletes_the_record, test_list_words_reads_an_old_record_as_unpinned_and_unstamped)；ime-fcitx5 addon/session/sources.rs (test_store_source_receives_the_pin_toggle)；export/list 置顶标记与 cheatsheet 键位说明
- **目标与价值**：置顶若无管理面就是黑洞：用户不知道自己置顶过什么、导出的词库丢失置顶信息、诊断无法回答「为什么这个词总在第一」。本卡把置顶接入既有管理出口，形成闭环。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：`export_tsv` 的 TSV 增列（头注释登记 v2 格式）：

    ```rust
    // Row format v2: `<key>\t<word>\t<commits>\t<pinned>` -- `pinned` is 1 for
    // the pinned word of the key and 0 otherwise. The extra column is
    // append-only: v1 parsers that stop at column 3 keep working.
    ```

    `list` 返回的条目带 pinned 标志；`forget` 对被置顶词先清 pin 再删（`TASK-1.04.01` 的最小一致性在本卡补测试与文档）；cheatsheet 的 Pin 键行说明补「再次按下取消」语义。
  - **边界契约**：`export_tsv` 输出格式向后兼容（追加列）；`UserFreqSource` 不再变更。
  - **交互与工艺级规范**：cheatsheet 文案中文（用户面 UI 文案规范），键位表结构不变。
  - **与现有代码的无缝衔接方案**：全部挂既有方法签名内部；诊断计数走 ime-diag 既有 counter 注册模式。
- **底层与非功能约束 (NFR)**：`list`/`export` 非输入热路径（既有 ≤0.05ms 管理路径预算位）；导出文件按用户数据纪律 0600。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `list`/`export_tsv` 扩展 + 兼容单测（v1 前三列逐字节不变）。
  2. `[步骤 2]` forget 清 pin 一致性 + cheatsheet 文案 + probe 计数。
  3. `[步骤 3]` `testd-engine` 用例：置顶 → forget → 频率回落原位。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-dict -p ime-fcitx5` 全绿（导出兼容、forget 一致性）。
  - [ ] 走查：cheatsheet 覆盖层实际渲染 Pin 行说明；导出文件人工抽查一例置顶标记。
  - [ ] 回归走查：`export_tsv` 既有消费方（若有测试 fixtures）不回退。

---

## Phase 1 汇总

| 任务 | GAP/IMP | 通道 | 工时 | 关键路径 |
|---|---|---|---|---|
| TASK-1.01.01 dictc 归一化管线 | GAP-01/IMP-02 | C | 3 | 是（CP-2） |
| TASK-1.01.02 质量门升级 | GAP-01/IMP-02 | C | 2 | 是（CP-2） |
| TASK-1.02.01 schema v2 迁移 | GAP-02/IMP-01 | A | 2.5 | 否 |
| TASK-1.02.02 衰减数学与打分 | GAP-02/IMP-01 | A | 3 | 否 |
| TASK-1.02.03 配置与空闲重算 | GAP-02/IMP-01 | A | 1.5 | 否 |
| TASK-1.03.01 容器 v2 + bigram 生成 | GAP-03/IMP-03 | A+C | 4 | 是（CP-1） |
| TASK-1.03.02 DictLm 真查 | GAP-03/IMP-03 | A | 2 | 是（CP-1） |
| TASK-1.04.01 置顶转正 | GAP-04/IMP-04 | A | 2 | 否 |
| TASK-1.04.02 置顶管理闭环 | GAP-04/IMP-04 | A | 1.5 | 否 |

**波次建议（文件不相交原则，AGENTS §6.3）**：W1 = {1.01.01, 1.02.01, 1.03.01*(*与 1.01.01 同文件须串行，排 W1 末或 W2 初), 1.04.01}；W2 = {1.01.02, 1.02.02, 1.03.02, 1.04.02}；W3 = {1.02.03}。1.03.01 与 1.01.01 的 `build.rs` 串行是本 Phase 唯一的同文件约束。
