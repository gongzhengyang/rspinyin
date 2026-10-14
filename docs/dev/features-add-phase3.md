# dev-features-add — Phase 3 增量原子任务分片

> 所属主控文档: [./features-add.md](features-add.md) ｜ 关联对比报告: [./compare-and-improve.md](compare-and-improve.md) ｜ 阶段定位: Phase 3 (P2 结构升级与功能面：纠错 / 上文重排 / 索引治理 / 热替换 / 造词 / 混输 / 错音提示 / 符号 / 桥接工艺) ｜ 包含任务区间: TASK-3.01.01 ~ TASK-3.06.02（10 卡，预估 24.5 人天）
>
> 阶段目标：打开三个结构性能力面（纠错容差、上文上下文、用户造词），治理两个数据瓶颈（重码截断、热替换），补齐三个功能面（混输/错音提示/符号），收拢桥接工艺五项。本阶段多张卡涉及冻结契约的**只增不改**扩展，各卡已显式标注 ADR 义务。

---

#### TASK-3.01.01 拼写纠错（邻位替换 + 键序颠倒）

- **基本属性**：
  - 绑定差距与改进项：`GAP-11` / `IMP-10` ｜ 关联设计假设：`ASM-A-02`（独占预算 ≤0.30ms）、`ASM-A-10`（**契约增项：`DecodeFlags::CORRECTION = 1 << 14`，需 ADR-0014**）
  - 优先级与复杂度：`P2 生态扩展` ｜ 高 ｜ 预估工时： 3 人天
  - 前置依赖：`无`（复用 `crates/ime-core/src/fuzzy.rs` 的等价类扩边基建；建议在 `TASK-2.01.01` 之后实施以便共享接线模式，但无逻辑依赖）
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine`
  - 对标参考锚点 (Benchmark Reference)：librime `NearSearchCorrector`（键盘邻位，edit-distance 版被上游 `#if 0` 禁用——`dict/corrector.cc:302-322`）+ correction 的对数重罚 −log0.01（`calculus.cc:16`）；rime-ice 约 30 条 `derive` 纠错规则及其「对简拼混输有副作用」的自注（[rime-ice.md](../compare/rime-ice.md) §3.3、[librime.md](../compare/librime.md) §2.2）
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-types/src/decode.rs`（`DecodeFlags` 增 `CORRECTION` 位，ADR-0014 先行）；新增 `crates/ime-core/src/segment/correct.rs`（邻位表与纠错扩边）；修改 `crates/ime-config/src/schema.rs` + `reload.rs`（`[engine] correction = false`）+ `crates/ime-core/src/state/machine.rs`（位注入）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：`zhegn` → `zheng`、`wo` 按成 `eo` 这类击键失误目前直接无候选。librime 的经验有二：①键盘邻位纠错是收益/成本比最高的一档（edit-distance 版连上游自己都禁用了）；②纠错必须带**重罚**（−log0.01 ≈ 比模糊音重 70 倍的对数罚），否则纠错候选会污染正常排序——rime-ice 的 derive 方案正因无分级罚而自注有副作用。rspinyin 的扩边基建与 Q8.8 定点罚值体系可以精确复刻「分级容差」这一关键设计。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：

    ```rust
    /// QWERTY adjacency, row-major: the neighbours a finger can slip to. The
    /// table is data, not control flow -- the same discipline as the key
    /// router's translation tables.
    pub(crate) static NEIGHBORS: &[&[u8]] = &[
        b"qwa", b"wqes", b"erd", b"dsf", b"rfg", b"fgh", b"ght", b"hyu",
        // ... 26 rows, one per letter; 'a' row ends with 's', etc.
    ];

    /// The correction penalty in Q8.8. One correction step costs about 3.9
    /// (librime's -log(0.01) rounded to the fixed-point grid): far below any
    /// fuzzy match, so a corrected candidate only surfaces when nothing
    /// spelled-correct competes.
    pub const CORRECTION_PENALTY_Q8: i32 = -998;

    /// Expands one syllable's spelling into its correction variants (single
    /// neighbour substitution and adjacent-key transposition, at most one
    /// step per syllable). Pure; deterministic order.
    pub fn corrections_of(syllable: &str) -> SmallVec<[String; 4]>;
    ```

    扩边挂载与 fuzzy 同一位置（DAG 边展开阶段），每 span 纠错变体数上限 4（对齐 librime `max_corrections`）；开启档仅当全码精确查询 miss 后才展开纠错边（对齐 librime「先记精确命中再 ToleranceSearch」的次序，`syllabifier.cc:89-106`）。
  - **边界契约**：`DecodeFlags` 增一位 `CORRECTION = 1 << 14`（ADR-0014；位域前向兼容已被 `decode.rs` 模块注释论证为「无需版本号」的扩展形态，ADR-0005 有先例）。纠错候选在候选来源标签中标注（既有来源标签机制，无 wire 变更）。
  - **交互与工艺级规范**：无新 UI；候选注释可显示纠错原拼写（`TASK-3.05.02` 的错音提示消费此标注，本卡只标注不显示）。
  - **与现有代码的无缝衔接方案**：`fuzzy.rs` 的等价类遍历器泛化为「拼写变体源」（fuzzy 等价类 / 纠错邻位表两个实现），DAG 扩边代码不动——注释说明两者罚值分档正是 K-4 的分级容差在定点域的落法。
- **底层与非功能约束 (NFR)**：
  - 独占预算 ≤0.30ms：纠错边只在「精确 miss」分支展开，精确命中时零成本；criterion `decode/correction_off/on_hit/on_miss` 三档基准。
  - 默认关；开启时正常输入（无击键错误）的候选序与关闭档一致（纠错边永不进入最优路径的断言——重罚保证，测试钉死）。
  - 边缘条件：双拼模式下纠错作用于双拼键（对齐 rime-ice「全拼双拼同表」诉求）；简拼组合（ABBREV+Correction 同时开）→ 每音节只允许一种容差叠加（纠错优先级低于简拼），防边数爆炸（lattice 四轴限界兜底 + 单测）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` ADR-0014 → `CORRECTION` 位 + 邻位表 + `corrections_of`（性质单测：一步可达、无重复、确定性序）。
  2. `[步骤 2]` DAG 扩边挂载（miss 分支）+ 罚值接入打分 + 配置键注入。
  3. `[步骤 3]` 三档基准 + 排序无污染断言 + `testd-engine` 纠错用例（`zhegn`→`真诚` 类）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-types -p ime-core` 全绿；ADR-0014 合入。
  - [ ] 基准证据：三档基准数字写入验收记录（off 档与主线逐字节一致）。
  - [ ] 回归走查：默认关档全量测试零回退；`xtask quality` 不降。

---

#### TASK-3.02.01 上文重排（上一 commit 词作 bigram 上下文）

- **基本属性**：
  - 绑定差距与改进项：`GAP-12` / `IMP-11` ｜ 关联设计假设：`ASM-A-02`（独占预算 ≤0.05ms）、`ASM-A-10`（**零契约变更**：`DecodeRequest` 不动，上下文活在会话机内）
  - 优先级与复杂度：`P2 生态扩展` ｜ 中 ｜ 预估工时： 2 人天
  - 前置依赖：`TASK-1.03.02`（bigram 真查表是重排的打分来源）
  - 关键路径：`是`（CP-1 收口）
  - 并行通道：`Track A Core·Engine`
  - 对标参考锚点 (Benchmark Reference)：librime `contextual_translation.cc:35-52`（对已生成候选按上文 `Grammar::Evaluate` 重打分、组内重排，每轮 ≤32 个）与 `script_translator.cc:325-329`（preceding_text 跨句上下文）
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-core/src/state/machine.rs`（会话机记录 last committed word + 候选终排 post-pass）、`crates/ime-core/src/viterbi/mod.rs`（终排函数，紧邻既有同文本合并逻辑 `mod.rs:323-373`）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：bigram 落地后，句内上下文已生效（词格内 prev→word）；缺的是**跨 commit** 的上下文：上一句以「银行」结尾时，本句 `yh` 的首选应偏向「银行」而非「预算」。librime 用 `contextual_translation` 在候选生成后按上文重打分组内排序——本卡同构实现，且**不改 `DecodeRequest`**（冻结结构体，改它要版本号迁移；上下文属于会话机状态，本来就该活在会话里）。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：

    ```rust
    /// The near-tie band, in Q16.16 path-score units: only candidates whose
    /// base scores differ by less than this are eligible for a contextual
    /// swap. Context breaks ties; it never overrides a real score gap -- the
    /// whole-sentence-first contract (`test_decode_ranks_the_whole_sentence_
    /// first`) stays intact for anything outside the band.
    pub const CONTEXT_TIE_BAND_Q16: i64 = 1 << 12; // 1/16 of a Q8.8 unit

    /// Re-ranks `candidates` by the bigram bonus of `context` (the last
    /// committed word, if any): a stable sort on (bonus desc, base rank asc),
    /// evaluated only inside the tie band and only over the first page
    /// (bounded like librime's 32-candidate contextual search limit).
    /// Deterministic: same inputs, same order, every run.
    pub fn rerank_with_context(
        candidates: &mut [Candidate],
        context: Option<&str>,
        lm: &dyn LanguageModel,
    );
    ```

    会话机：`Committing → Committed` 跃迁时把提交文本按词格回溯出**末词**存入会话（纯内存，不落盘——隐私纪律：候选与提交文本本就不落日志）；下一次 `decode` 完成后、帧发布前调用 `rerank_with_context`。上下文词在 Escape/焦点丢失/超时（>10s 或 >5 次按键未用）后清除。
  - **边界契约**：`UiFrame`/`DecodeRequest` 零变更；重排是对候选列表的会话内变换，`revision` 幂等语义不变。隐私：上下文词只存会话内存，随会话销毁（红字登记进本卡与主控 §3.3）。
  - **交互与工艺级规范**：无新 UI；重排效果即候选顺序变化本身。
  - **与现有代码的无缝衔接方案**：post-pass 插在 `mod.rs` 的候选装配末端（同文本合并之后、`Paging::reconcile` 之前）；`lm` 引用取 `env.lm`，上下文词取会话机新字段——两处都是既有结构的小增量。
- **底层与非功能约束 (NFR)**：
  - ≤0.05ms：首页 9 候选 × 一次 bigram 查表；band 外零计算。criterion `decode/context_rerank` 基准。
  - 确定性：稳定排序 + 无浮点；band 内并列按原序（测试：同输入 100 次同序，复用字节指纹模式）。
  - 边缘条件：`context = None`（会话首句）→ 恒等；上下文词不在词表 → bigram miss 路径，bonus 零；整句候选（多词）取末词作被查对象。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `rerank_with_context` 纯函数 + band/稳定序单测。
  2. `[步骤 2]` 会话机末词记录与清除策略 + post-pass 接线；对照测试（「银行」上文 → `yh` 首选翻转；band 外不翻转）。
  3. `[步骤 3]` `testd-engine` 跨句用例（两段式：先 commit 上文，再断言下文首选）+ 基准。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-core` 全绿；整句首选契约测试保持绿（band 外不被推翻）。
  - [ ] 基准证据：`decode/context_rerank` ≤0.05ms 写入验收记录。
  - [ ] 回归走查：无上文时逐字节一致；隐私走查（上下文不落任何日志/文件，`grep` 抽查诊断路径）。

---

#### TASK-3.03.01 重码治理：截断审计与自适应上限

- **基本属性**：
  - 绑定差距与改进项：`GAP-13` / `IMP-05` ｜ 关联设计假设：`ASM-A-03`（词库红线）
  - 优先级与复杂度：`P2 生态扩展` ｜ 中 ｜ 预估工时： 2.5 人天
  - 前置依赖：`TASK-1.01.01`（归一化后的权重才是截断决策的可靠依据）
  - 关键路径：`否`
  - 并行通道：`Track C Infra·DevOps`
  - 对标参考锚点 (Benchmark Reference)：librime 四级索引的 `TailIndex`/`extra_code` 思想（[librime.md](../compare/librime.md) §4.2——本卡取其「按需扩容」的语义，不搬索引结构）；rspinyin 自身的 FST payload 结构（`wordlist_start<<24|count`，count 域 24 位，32 是策略值非格式值）
  - 本项目代码落地锚点 (Code Anchor)：新增 `xtask/src/dictc/truncation.rs`（审计报告）；修改 `xtask/src/dictc/build.rs`（`MAX_WORDS_PER_KEY` 策略化）与 `crates/ime-dict/src/fst_index.rs`（`MAX_WORDS_PER_KEY` 消费点常量调整）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：`MAX_WORDS_PER_KEY=32` 把每个键的候选硬截到 32。平均 1.19 词/键掩盖了热点键的真相：高频两字组合键（重码大户）可能被截掉真词。librime 用四级索引让长尾不占主索引——本卡先**量化**（审计到底丢了什么），再按数据决定上限策略，避免盲目改格式。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：审计先行：

    ```rust
    /// One key that hit the cap: how many words wanted in, how many got in,
    /// and the weight of the best word left behind. A cap is only a problem
    /// when the word it dropped would have been a reasonable candidate.
    pub struct TruncatedKey {
        pub key: String,
        pub wanted: u32,
        pub kept: u32,
        pub best_dropped_weight: u32,
    }

    /// Audits the compiled word map: every key at the cap, ranked by the
    /// quality of what the cap cost. The report drives the cap decision --
    /// if `best_dropped_weight` sits under the tail band, the cap is fine.
    pub fn audit(entries: &WordMap, cap: u32) -> Vec<TruncatedKey>;
    ```

    决策分支（按审计结果二选一，卡内都给实现路径）：
    - **A（预期主路径，零格式变更）**：权重降序写 WORDLIST 的前提下，32→64/128 的自适应上限只改策略常量（payload 的 count 域 24 位，容量无碍）；配合 `ASM-A-03` 的体积红线实测（每键多 32 词 ≈ WORDLIST 段最多 +N×4B，实测量化）。
    - **B（若 A 超红线）**：保留 32 + 超出词转「溢出表」（新容器段，仅 `prefix` 查询与整码深页消费）——格式 v2 增段，复用 `TASK-1.03.01` 的段扩展机制。
  - **边界契约**：方案 A 零契约/格式变更；方案 B 走 v2 段扩展（同 1.03.01 的读端兼容矩阵）。
  - **交互与工艺级规范**：Headless——审计报告进 `dictc quality` 输出（`truncated_keys: N, worst_dropped_weight: W`）。
  - **与现有代码的无缝衔接方案**：写端 `build.rs` 与读端 `fst_index.rs` 的两个 `MAX_WORDS_PER_KEY` 消费点同步改；`lookup` 的 `read_words_into(key, MAX_WORDS_PER_KEY, …)` 调用参数即常量来源。
- **底层与非功能约束 (NFR)**：
  - 体积：方案 A 下 `base_dict` 红线（20MB）实测把关，超限自动回退方案 B；budget scope 断言。
  - 质量：`xtask quality` 两字带 top1 相对基线不降（截断放宽只可能不减信息）。
  - 解码预算：`lookup` 返回更多词 → 词格边数受四轴限界保护（断言 decode p99 不回退）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 审计工具 + 全量报告（写验收记录：多少键触帽、最重被丢词的权重分布）。
  2. `[步骤 2]` 按审计结果落 A 或 B；体积/质量/预算三实测。
  3. `[步骤 3]` 门禁同步（budget/quality）+ 回归。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p xtask -p ime-dict` 全绿。
  - [ ] 实测证据：审计报告 + 上限决策依据 + 三项实测写入验收记录。
  - [ ] 回归走查：`budget --validate`、`xtask quality`、双编译一致性。

---

#### TASK-3.03.02 词库热替换（staged 文件 + 配置重载触发）

- **基本属性**：
  - 绑定差距与改进项：`GAP-14` / `IMP-20` ｜ 关联设计假设：`ASM-A-06`（不新增线程）、`ASM-A-09`、`ASM-A-10`（**新诊断码 `dict/swap/rejected`**）
  - 优先级复杂度：`P2 生态扩展` ｜ 中 ｜ 预估工时： 2.5 人天
  - 前置依赖：`无`（features.md `TASK-2.03.04` 原规划位）
  - 关键路径：`否`
  - 并行通道：`Track C Infra·DevOps`
  - 对标参考锚点 (Benchmark Reference)：librime 的部署期 checksum 增量重建（`dict_compiler.cc:104-118`——「校验过了才换」的语义）；本项目自有规划（features.md 索引的 `ArcSwap` 形态）
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-dict/src/lib.rs` 或新增 `crates/ime-dict/src/hot_swap.rs`（`LexiconSlot`）、`crates/ime-fcitx5/src/addon.rs`（`on_config_reload` 触发点，`addon.rs:147-162` 邻位）、`crates/ime-config/src/schema.rs` + `reload.rs`（`data.dict_staged_path`）
  - 引入依赖白名单：`arc-swap`（版本经 workspace `[workspace.dependencies]` 收敛；必要性：无锁读 + 写侧整指针交换是热替换的生态标准解，`RwLock` 读侧有缓存行争用、手写原子指针等于重造 arc-swap；无网络、维护活跃、纯 Rust——3.5 基线过审项）
  - 当前状态：`[ ] 待开始`
- **目标与价值**：换词库要重启 fcitx5——对「调词库就是日常」的用户（自建领域词、错误词修正）是硬痛点。本卡把「换库」做成：暂存文件 → 配置重载触发 → 全量校验 → 原子切换 → 在途会话保旧库。librime 的「checksum 不符才重建」语义对应到运行时就是「校验不过不换」。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：

    ```rust
    /// The live lexicon handle. Readers (`decode_into` callers) clone the
    /// `Arc` out and keep the old container alive for as long as their decode
    /// runs; a swap therefore never tears an in-flight session. The slot
    /// itself never blocks a reader.
    pub struct LexiconSlot(ArcSwap<FstLexicon>);

    impl LexiconSlot {
        /// Full validation before the swap: header, section table, per-section
        /// CRC, budget share. Any failure keeps the old container and returns
        /// the reason (`dict/swap/rejected` travels with it).
        pub fn try_swap(&self, candidate: PathBuf) -> Result<Arc<FstLexicon>, DictError>;
    }
    ```

    触发路径（无文件 watcher 是既定设计）：配置 `[data] dict_staged_path = ""`；`on_config_reload` 读到非空 staged 路径 → `try_swap` → 成功后**回写清空该键**（`writeback.rs` 既有回写通道）+ info 诊断；失败 → warn `dict/swap/rejected` + 保留键值供用户修正。
  - **边界契约**：新诊断码 `dict/swap/rejected`（主控 §3.3-4）；会话语义「组合中的会话保旧配置/旧库」对齐 0.4 规则 10 的精神——`Arc` 克隆天然保证。
  - **交互与工艺级规范**：Headless——`xtask dict stage <file>` 帮助命令（拷入 staged 位置 + 打印提示），或用户手动放置 + 手动触发 fcitx5 重载。
  - **与现有代码的无缝衔接方案**：`addon.rs` 的 lexicon 装配步产物从「裸 `FstLexicon`」变为 `LexiconSlot`（会话宿主 env 持 slot，`decode_into` 前沿 `load()` 取 `Arc`——一次原子读）；`recover.rs` 的 quarantine 机制复用为 staged 校验失败的隔离路径。
- **底层与非功能约束 (NFR)**：
  - 切换延迟：校验（全量 CRC，15.62MiB）在**宿主线程的重载回调内**执行会阻塞主循环——**必须**在重载回调里只做「mmap + 头校验」快路径，全量 CRC 交给既有 sweep 线程槽位完成后二次确认（快校验失败立即拒；慢校验失败回滚并诊断）。宿主阻塞 ≤ 10ms（`BUDGET-LAT-05` 的 120ms 初始化预算内单步）。
  - 内存：切换窗口内双 mmap 并存（峰值 +16MiB，短暂）——`dict_mmap_rss` 25MB 红线按峰值期豁免登记，切换完成旧库 drop 后回落。
  - 失败回滚：任何一步失败保旧库继续服务（降级阶梯原则）；staged 文件损坏走 quarantine 不污染正式路径。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `arc-swap` 引入（workspace 依赖 + 审计过审）+ `LexiconSlot` + 装配点改造（先零行为差异：永不 swap）。
  2. `[步骤 2]` staged 配置键 + 重载触发 + 快/慢两段校验与回滚 + 诊断。
  3. `[步骤 3]` `xtask dict stage` 辅助命令 + 沙盒会话实测（换库后新 decode 用新库、在途组合不中断）+ X11 档真机走查。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-dict -p ime-fcitx5`（swap 成功/损坏拒/回滚三组）；`cargo audit`/`cargo deny` 对新依赖零 finding。
  - [ ] 走查：X11 档真机——输入中换库：当前词继续、下一键用新库；`dict/swap/rejected` 场景人工触发一次。
  - [ ] 回归走查：`keymap_matrix`、`soak` 短程（1h）不回退；宿主阻塞实测 ≤10ms。

---

#### TASK-3.04.01 用户造词写路径（链式词库 + 句内成分回写）

- **基本属性**：
  - 绑定差距与改进项：`GAP-15` / `IMP-09` ｜ 关联设计假设：`ASM-A-04`、`ASM-A-06`、`ASM-A-10`（**零契约变更**：链式组合在装配点完成）
  - 优先级与复杂度：`P2 生态扩展` ｜ 高 ｜ 预估工时： 3 人天
  - 前置依赖：`TASK-1.02.01`（v2 schema 承载造词频次；置顶/衰减共用用户库基建）
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine`
  - 对标参考锚点 (Benchmark Reference)：librime `Memory::OnCommit`（`gear/memory.cc:106-111`）的自动造词与 `user_dictionary.cc:331-349` 的 `UpdateElements`（词组内单字回写）；`is_user_word` 的语义终点（`user_db.rs:694` 注释自述「Phase 2 用户造词」）
  - 本项目代码落地锚点 (Code Anchor)：新增 `crates/ime-dict/src/user_lexicon.rs`（内存用户词表，实现 `Lexicon`）；修改 `crates/ime-fcitx5/src/addon.rs`（装配点链式组合）、`crates/ime-dict/src/user_db.rs`（造词持久表 + `is_user_word` 真实现）、`crates/ime-core/src/state/`（commit 成分回写触发）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：整句提交后，其中被用户「选词路径」确认过的二字组合（如反复打「诸暨」）应成为下次的一等候选——librime 十年好用的核心一环。现状 `is_user_word` 恒 false、无造词写路径。设计关键是**零契约变更**：用户词表自己实现 `Lexicon` trait，装配点把它与系统词库链成 `ChainedLexicon`——decoder 无感知。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：

    ```rust
    /// The user's own words: entries the commit path enrolled, keyed by their
    /// syllable string like any dictionary entry. Implements `Lexicon` over an
    /// in-memory map (a few hundred entries at most), so the host composes it
    /// AHEAD of the system lexicon -- user words outrank system words of the
    /// same key by construction of the chain, and the decoder is unchanged.
    pub struct UserLexicon {
        words: HashMap<Box<str>, UserWord>,
    }

    struct UserWord {
        text: Box<str>,
        weight: u32,
        flags: WordFlags, // FLAG_USER set; the compiler never sets this bit,
                          // which is exactly how a consumer tells them apart
    }

    /// Delegating `Lexicon`: queries the front lexicon first, falls through to
    /// the back. Assembled once at addon init; `Send + Sync` by composition.
    pub struct ChainedLexicon {
        front: UserLexicon,
        back: Arc<FstLexicon>,
    }
    ```

    造词写入：commit 闭合时，会话机把本次选词路径上的**相邻词对**（用户连续选择的二字组合）报给宿主；宿主写 `user_lexicon`（内存即时生效）+ 持久表（redb 新表 `user_lexicon: &str key -> (word, commits)`，批量落盘同 user_words 纪律）；`is_user_word(key)` 对造词键返回 true（含 frequency 侧信息）。淘汰：与词频库共用 50 万上限与 pinned 保护。
  - **边界契约**：`Lexicon`/`UserFreqSource` trait 零变更（组合模式）；`WordFlags::USER` 位语义从「编译器从不设置」变为「只有运行期造词设置」——`format` 段注释同步（格式位不新增，语义激活）。
  - **交互与工艺级规范**：造词候选来源标签（既有机制）标注 `用户词`；`TASK-1.04.02` 的 export 增造词列（或独立导出段，实现择简并登记）。
  - **与现有代码的无缝衔接方案**：链式装配在 `addon.rs` lexicon 步（系统词库构造处包一层）；回写触发挂 commit 执行器既有回调（`router.rs` 的 `CommitDone` 闭合点邻位）。
- **底层与非功能约束 (NFR)**：
  - 造词查询代价：前置表为空时链式查询短路（一次 `is_empty` 判断），decode 零增量；非空时前置表命中优先（HashMap O(1)）。
  - 隐私：造词内容是用户输入派生物——落盘走 user.redb 同纪律（0600/0700、批量、readonly 降级），日志永不触达（脱敏 denylist 已覆盖 word 字段）。
  - 边缘条件：造词与系统词同键同文 → 合并频次不重复出候选；造词超过上限 → LRU 淘汰（pinned 例外）；`forget` 对造词 = 删除词条。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `UserLexicon`（`Lexicon` 三方法的内存实现 + 单测，mock 对拍）与 `ChainedLexicon`。
  2. `[步骤 2]` 装配链 + 持久表 + commit 成分回写 + `is_user_word` 真实现。
  3. `[步骤 3]` 行为测试（重复选词两轮后首选翻转、readonly 不造词、forget/export 闭环）+ `testd-engine` 用例。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-dict -p ime-fcitx5` 全绿。
  - [ ] 走查：沙盒会话连续两次选「诸暨」→ 第三次 `zhuji` 首选「诸暨」；导出包含造词。
  - [ ] 回归走查：空造词表时全量测试零回退；`keymap_matrix` 不回退。

---

#### TASK-3.05.01 中英混输（英文词段 + 融合档位）

- **基本属性**：
  - 绑定差距与改进项：`GAP-16` / `IMP-16` ｜ 关联设计假设：`ASM-A-03`、`ASM-A-08`（英文词源许可）
  - 优先级复杂度：`P2 生态扩展` ｜ 高 ｜ 预估工时： 3 人天
  - 前置依赖：`TASK-3.04.01`（`ChainedLexicon` 是英文词库的挂载形态）
  - 关键路径：`否`
  - 并行通道：`Track C Infra·DevOps`（数据）+ Track A（词格英文边）
  - 对标参考锚点 (Benchmark Reference)：rime-ice melt_eng 的融合排序——`initial_quality` 档位（自定义短语 99 ≫ 拼音 1.2 > 英文 1.1 > 中英混合 0.5）与拼写派生（[rime-ice.md](../compare/rime-ice.md) §5）；英文词源取 `remarks` 盘点的合法上游（rime-melt 英文词表 Apache-2.0 ~1,000 条起步；扩量用 dwyl/english-words（Unlicense）或 SCOWL，`ASM-A-08` 白名单登记）
  - 本项目代码落地锚点 (Code Anchor)：新增 `data/raw/melt-eng.tsv` + `data/sources.toml` 条目；修改 `xtask/src/dictc/build.rs`（英文小容器编译，或独立 `eng.dict`）；新增 `crates/ime-core/src/segment/english.rs`（拉丁 span 边）；修改 `crates/ime-core/src/viterbi/lattice.rs`（英文边接入）与 `crates/ime-config`（`engine.mixed_english = false`）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：输入法里打 `github`、`VIP` 目前只能切英文直通。rime-ice 的 melt_eng 证明了「同一码段多翻译器并行 + 质量档位融合」就够——rspinyin 的等价物是：英文小词库（复用容器格式，键=拉丁拼写）+ 词格拉丁 span 边 + 固定档位罚（英文永不压过拼音候选）。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：
    - 英文词库：独立小容器 `eng.dict`（复用 6 段格式，FST 键 = 小写拉丁串，词条 flags 带 `TERM` 位语义复用或新登记 `ENG` 语义注释）；dictc 从 `melt-eng.tsv` 编译（`kind="derived"` 或 upstream Apache-2.0 逐源登记）。
    - 词格边：

      ```rust
      /// A maximal latin span inside the composing buffer (`vip`, `xcode`)
      /// becomes one candidate edge through the english lexicon, priced by
      /// MIXED_ENGLISH_PENALTY_Q8 so a pinyin reading of the same keys always
      /// outranks it -- the Q8.8 counterpart of melt_eng's `initial_quality`
      /// 1.1-vs-1.2 ratio.
      pub const MIXED_ENGLISH_PENALTY_Q8: i32 = -46; // ~ -0.18
      ```

    - 融合档位（对齐 melt_eng 心智、本项目常数域）：短语 > 拼音 > 英文；英文候选不参与造句（不进 Viterbi 路径延伸，只作单边候选——`enable_sentence: false` 的对应物）。
  - **边界契约**：容器无格式变更（英文进独立文件，链式装配）；候选来源标签 `英文`。
  - **交互与工艺级规范**：大小写保真：词条按原始大小写展示（`GitHub`），查询按小写折叠；`auto_english_on_uppercase` 直通行为优先级高于混输（大写开头仍直通英文，配置可关）。
  - **与现有代码的无缝衔接方案**：词格接入点 = `build_lattice` 的 span 遍历中「音节匹配失败且 span 为纯拉丁」分支；ChainedLexicon 的英文链成员由配置档装配（关档时链条不含英文库，零成本）。
- **底层与非功能约束 (NFR)**：
  - decode 预算：英文边仅当 span 音节匹配失败时产生；关档零成本；开档 ≤0.20ms（计入未分配余量，实测入验收记录）。
  - 已知限制（卡内显式登记）：v1 覆盖「整 span 英文候选 + 单一拉丁 span 边」；`X光` 类跨中英混合词条（需词条内双语段）留待后续卡。
  - 边缘条件：`www.baidu.com` 仍走 passthrough URL 规则（优先级高于混输）；纯数字 span 不产生英文边。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 词源登记与 `eng.dict` 编译（dictc 小步骤 + 白名单/审计）。
  2. `[步骤 2]` `segment/english.rs` + 词格英文边 + 融合档位 + 配置键。
  3. `[步骤 3]` 行为测试（`github` 进候选、拼音优先序不变、大小写保真）+ `testd-engine` 用例 + 预算基准。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p xtask -p ime-core` 全绿；`check-dict` 过。
  - [ ] 走查：沙盒会话 `vip`/`github` 出英文候选且排拼音之后；关档零回退。
  - [ ] 回归走查：默认关档全量零回退；`budget --validate` 过。

---

#### TASK-3.05.02 错音错字提示（annotation 通道）

- **基本属性**：
  - 绑定差距与改进项：`GAP-17` / `IMP-17` ｜ 关联设计假设：`ASM-A-10`（annotation 走既有字段，无 wire 变更）
  - 优先级复杂度：`P2 生态扩展` ｜ 低 ｜ 预估工时： 2 人天
  - 前置依赖：`TASK-3.01.01`（纠错边的来源标注是「错音」判定的事实来源；fuzzy 接线 `TASK-2.01.01` 建议先行）
  - 关键路径：`否`
  - 并行通道：`Track C Infra·DevOps`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `corrector.lua:1-291`（~250 行错音映射 + `spelling_hints` 把全拼塞进 comment 的「注释通道传信息」技巧——[rime-ice.md](../compare/rime-ice.md) §4）
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-core/src/viterbi/mod.rs`（候选装配时按边来源附 annotation）；新增 `data/raw/confusable.tsv`（自建错字映射，`kind="derived"`）；`crates/ime-dict` 装配读取（并入 `ScriptIndex` 式的小索引）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：模糊音/纠错帮用户打出了字，但用户往往不知道「为什么这个字」——rime-ice 用注释通道做错音提示（「您是不是要找：zheng」类）。rspinyin 的候选 annotation 位现成，缺的只是「边来源 → 提示文本」这一跳；错字映射表则按其纪律**自建**（GPL 的 corrector.lua 表不可抄，`ASM-A-08`）。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：
    - 错音（引擎侧，零数据表）：候选经 `FUZZY_*` 或 `CORRECTION` 边进入时，annotation 前缀附「音:〈标准读法〉」（标准读法 = 词条被查键的原始音节串，扩边前值已在边构造处可得）。
    - 错字（数据侧）：`confusable.tsv` 两列 `错字⇥正字`（自建起步 ~100 行：的地得/在再/帐账类）；候选文本命中错字列且上下文位匹配时 annotation 附「字:→正字」。装配为与 `ScriptIndex` 同形态的小索引。
  - **边界契约**：annotation 是候选既有字段，`UiFrame` 零变更；提示不改变排序（纯注释）。
  - **交互与工艺级规范**：`show_annotation = true`（默认）时显示；文本全中文（用户面文案）；提示与既有词典注释的优先级：错音/错字提示在前、词典注释在后（同一 annotation 字段内以「｜」拼接，超宽截断规则沿用既有省略逻辑）。
  - **与现有代码的无缝衔接方案**：边来源信息已在词格边构造处（fuzzy/correct 扩边时已知），装配点读取；错字索引查询在候选装配循环内 O(词长)。
- **底层与非功能约束 (NFR)**：
  - 预算：annotation 附着 ≤0.05ms（字符串拼接仅发生在有提示的候选上；无提示候选零分配——Cow/借用路径）。
  - 边缘条件：一候选同时命中错音与错字 → 两提示并列；annotation 关闭时全链路短路。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 边来源透传 + 错音 annotation（单测：fuzzy 候选带提示、精确候选无提示）。
  2. `[步骤 2]` `confusable.tsv` 自建（起步 100 行，来源登记）+ 错字索引 + annotation 拼接。
  3. `[步骤 3]` `testd-engine` 注释断言 + 预算基准 + 帧镜像走查。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-core -p ime-dict` 全绿。
  - [ ] 走查：帧镜像中模糊音候选的 annotation 含标准读法；关 `show_annotation` 无泄漏。
  - [ ] 回归走查：排序零变化（纯注释断言）；`xtask quality` 不回退。

---

#### TASK-3.05.03 符号面板 v 模式（触发前缀 + 候选通道）

- **基本属性**：
  - 绑定差距与改进项：`GAP-18` / `IMP-18` ｜ 关联设计假设：`ASM-A-08`（符号数据自建/自由许可）、`ASM-A-10`（候选通道承载，零 wire 变更）
  - 优先级复杂度：`P2 生态扩展` ｜ 中 ｜ 预估工时： 2.5 人天
  - 前置依赖：`无`
  - 关键路径：`否`
  - 并行通道：`Track B UI·Client·CLI`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `symbols_v.yaml` 的 v 模式（71 类，**GPL 不可抄**——自建起步集：数字全家桶/序号/箭头/拼音声调/常用单位 5 类 ~200 符号，均为 Unicode 码位清单类事实数据）与 `recognizer` 前缀路由思想（[rime-ice.md](../compare/rime-ice.md) §6）；emoji 扩充用 Unicode CLDR annotations（Unicode License V3，与已接入的 Unihan 同族许可）
  - 本项目代码落地锚点 (Code Anchor)：新增 `data/raw/symbols.tsv`（`kind="derived"`：分类⇥符号清单）；新增 `crates/ime-core/src/symbols.rs`（表加载与查询，纯函数）；修改 `crates/ime-core/src/state/machine.rs`（`v` 前缀路由分支）与 `crates/ime-fcitx5/src/router`（分类注释注入）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：`v1` → １ ⁴ ½ Ⅰ 一 类符号列表现已无处可打。v 是拼音无声母字母（天然空闲前缀，rime-ice 同款选择），候选网格 + 分类注释就是「面板」——无需新窗口（自绘面板工艺归 opt-ui.md 轨道，本卡显式不做新 UI 面）。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：

    ```rust
    /// Symbol categories keyed by the code after `v` (`v1` digits, `vx`
    /// arrows, `vym` pinyin tones ...). The table is derived data, compiled
    /// next to the phrase table; queries answer with (symbol, category) pairs
    /// in file order, which is the display order.
    pub struct SymbolTable { /* categories: Vec<(prefix, Vec<char>)> */ }

    impl SymbolTable {
        /// Candidates for a `v`-prefixed raw. Pure; empty when the prefix is
        /// unknown (the router then falls through to normal decoding).
        pub fn lookup(&self, code: &str) -> &[SymbolCandidate];
    }
    ```

    路由：`machine.rs` 在进入解码前识别 `v` 开头且后续为表内前缀的 raw → 符号模式（preedit 显示 `v+code`，候选 = 表行，注释 = 分类名）；`v` 后接正常拼音（如 `vzhishi`）→ 不匹配表前缀，走既有解码（`v` 经规范化丢弃的既有行为保持）。
  - **边界契约**：`UiFrame` 零变更（符号即普通候选，annotation = 分类）；不新增 `OverlayKind` 变体（候选通道优先，面板化留待 opt-ui 轨道评估）。
  - **交互与工艺级规范**：翻页/选词复用既有候选交互全链路；数字/序号类符号单行可达 40+ 项，分页上限沿用 5×9。
  - **与现有代码的无缝衔接方案**：符号模式是会话机的一个互斥输入态（类似 `temp_english` 的现有先例），Escape/退格到空退出；数据装载走 `phrases.tsv` 同款宿主文件装配点。
- **底层与非功能约束 (NFR)**：
  - 查询 ≤0.05ms（前缀 → 二分/线性小表）；表体 ≤ 50KB（起步 5 类）。
  - 边缘条件：`v` 单独输入 → 显示符号模式空候选 + 提示注释（可发现性）；`v` 后接超长串 → 表前缀最长匹配后余文忽略（注释声明）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `symbols.tsv` 自建（5 类 ~200 符号，码位逐一验证）+ 加载器 + 单测。
  2. `[步骤 2]` 会话机符号模式 + 路由分支 + 候选装配。
  3. `[步骤 3]` `testd-engine` 用例 + 帧镜像走查 + cheatsheet 增 `v` 模式说明。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-core` 全绿；`check-dict` 对 derived 源过审。
  - [ ] 走查：帧镜像 `v1`/`vx` 候选与分页；`vzhishi` 仍走拼音解码。
  - [ ] 回归走查：非 `v` 输入零回退；`budget --validate` 过。

---

#### TASK-3.06.01 切输入法收尾策略与 preedit capability 协商

- **基本属性**：
  - 绑定差距与改进项：`GAP-19` / `IMP-13` ｜ 关联设计假设：`ASM-A-07`、`ASM-A-10`
  - 优先级复杂度：`P2 生态扩展` ｜ 中 ｜ 预估工时： 2 人天
  - 前置依赖：`无`
  - 关键路径：`否`
  - 并行通道：`Track B UI·Client·CLI`
  - 对标参考锚点 (Benchmark Reference)：fcitx5-rime `SwitchInputMethodBehavior` 四档（Clear / 提交原文 / 提交 composition preedit / 提交预览，`rimeengine.cpp:464-484`）与 preedit capability 检查（`rimestate.cpp:349-352`：IC 无 `CapabilityFlag::Preedit` 自动落回 aux 显示）——[fcitx5-rime.md](../compare/fcitx5-rime.md) §3.2 表 #2/#4
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-fcitx5/src/addon.rs`（宿主 Reset/切走回调）与 `crates/ime-fcitx5/src/engine/context.rs`（capability 探测）；`crates/ime-config`（`engine.on_im_switch = "clear" | "commit_raw"`）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：两个真实场景：①用户 composing 中切走到别的输入法——在途编码被静默丢弃（或行为不可预期），fcitx5-rime 给了四档收尾语义；②不支持客户端 preedit 的应用（部分终端/游戏）在 `client_preedit = true` 全局配置下显示异常——capability 协商把「全局手动配置」升级为「按 IC 自动降级」。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：
    - 收尾：宿主 Reset/IM 切换回调中按策略执行：`clear`（现状，丢在途）/ `commit_raw`（提交原始编码再清）。四档取二：`commit_preedit`（已转换文本）与 `commit_raw` 语义在 rspinyin 的实现里收敛为同一动作（commit 前变换链已含标点/简繁），登记取舍理由。
    - 协商：

      ```rust
      /// The client preedit decision for one input context: the user's
      /// config is an upper bound, the application's capability is the
      /// floor. A client that cannot render preedit gets aux-display
      /// semantics (the candidate window's preedit row), never a broken
      /// inline string.
      pub fn resolve_client_preedit(cfg: bool, ic_has_preedit: bool) -> bool {
          cfg && ic_has_preedit
      }
      ```

  - **边界契约**：行为契约：无 capability 的 IC 永远不见客户端 preedit（即便配置开）；收尾 commit 走既有 `Effect::Commit` 通道（无新 effect）。
  - **交互与工艺级规范**：无自绘窗改动；配置模板注释更新（`client_preedit` 注释补「不支持的应用自动降级」）。
  - **与现有代码的无缝衔接方案**：capability 位从宿主 IC 既有查询通道取得（`ffi/abi` 层已透传 capability 集合或需一步补透传——卡内步骤 1 先勘察）；收尾点挂 `addon.rs` 宿主 reset 分派处（`addon.rs:147-162` 邻位）。
- **底层与非功能约束 (NFR)**：决策均为 O(1) 布尔逻辑；收尾 commit 不超 commit 预算（复用既有路径）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 勘察 capability 位透传现状 → 补 FFI 透传（若缺）。
  2. `[步骤 2]` `resolve_client_preedit` 接线 + 收尾策略配置与执行。
  3. `[步骤 3]` 行为测试（mock 无 preedit IC、composing 中切走两档行为）+ 真机走查。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-fcitx5` 全绿。
  - [ ] 走查：X11 档——不支持 preedit 的窗口（xterm 类）无内联串；composing 中切输入法按配置提交/清除。
  - [ ] 回归走查：默认档（clear + 配置 preedit）行为与主线一致。

---

#### TASK-3.06.02 compose/dead key、诊断节流与状态区 Action

- **基本属性**：
  - 绑定差距与改进项：`GAP-19` / `IMP-14` ｜ 关联设计假设：`ASM-A-06`、`ASM-A-10`
  - 优先级复杂度：`P2 生态扩展` ｜ 中高 ｜ 预估工时： 2.5 人天
  - 前置依赖：`TASK-2.03.01`（StatusArea 的简繁状态展示消费 script 模式位）
  - 关键路径：`否`
  - 并行通道：`Track B UI·Client·CLI`
  - 对标参考锚点 (Benchmark Reference)：fcitx5-rime 三件套——compose 委托 `instance()->processComposeString`（`rimestate.cpp:170-180`）、通知节流（60s 放行窗 + 部署后 30s 静默，`rimeengine.cpp:62-63,594-607`）、StatusArea Action/菜单（option 联动 Toggle/Select，标签取自方案声明，`rimeaction.*`）——[fcitx5-rime.md](../compare/fcitx5-rime.md) §3.2 表 #3/#5/#9
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-fcitx5/src/engine/router.rs`（compose 前置处理）、`crates/ime-diag/src/log.rs`（重复诊断节流）、`crates/ime-fcitx5/src/ffi/cpp/addon_glue.cpp` + `engine/binding_audit.rs`（StatusArea Action 的 C++ 胶水扩展——**含可行性 spike 步骤**）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：三件小事各自都小，合起来是「桌面原生感」：①欧洲键盘用户需要 dead key（´+e→é）；②`lifecycle/pending` 类重复诊断在日志里刷屏（fcitx5-rime 的 60s 放行窗是现成答案）；③中英/简繁状态只有 cheatsheet 可查，StatusArea 图标是输入法的标准可见性。③的风险最高（需要扩 C++ 胶水），卡内设 spike 与降级出口。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：
    - compose：`key_event` 路由前置步——非 release 且无 Ctrl/Super 的键先问宿主 compose（`processComposeString` 经 C ABI 透传或胶水直调）；未完成组合 → 吞键；完成 → 结果字符按「长度 1 的直通 commit」处理（对齐 fcitx5-rime 语义）。可配置关（`engine.compose = false` 默认，避免与现有用户键位冲突）。
    - 节流：ime-diag 的诊断行发布器加「同码 60s 放行一次」的节流表（内存小 LRU；属诊断层，非业务层）。
    - StatusArea（spike 门控）：胶水层注册 Action（图标 + 短标签：中/英、简/繁）；**spike 结论不可行（胶水面过大）则降级交付**：compose + 节流 + cheatsheet 的状态行增强，StatusArea 登记 features.md 后续评估。
  - **边界契约**：compose 吞键语义进 `binding_audit` 矩阵（新增行为行）；节流是诊断层内部行为，不改错误码集合。
  - **交互与工艺级规范**：StatusArea 标签中文（用户面文案）；图标复用宿主图标主题（对齐 fcitx5-rime 的 hicolor 纪律，不自带贴图）。
  - **与现有代码的无缝衔接方案**：compose 前置步插在 `translate_key` 之后、分层总线之前（只在「无修饰、非 composing」时询问，零热路径成本——composing 中不问 compose）；节流表挂在 `ime-diag` 的既有行发布函数。
- **底层与非功能约束 (NFR)**：
  - compose 查询仅 idle 态触发（composing 中零调用）；节流表 ≤ 32 项 LRU，O(1)。
  - spike 产出：`docs/dev/spikes/status-area.md`（可行/不可行 + 依据）；不可行时的降级路径已在本卡范围内（不产生新卡）。
  - 边缘条件：compose 结果字符与 rspinyin 键位冲突（如 `´` 后的 `e`）——compose 吞键优先且可配关；节流不吞首条（首条永远发布）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` compose 透传（FFI/胶水勘察 → 实现）+ 配置键 + audit 矩阵扩展。
  2. `[步骤 2]` 诊断节流表 + 单测（同码 61s 两行、异码不受限）。
  3. `[步骤 3]` StatusArea spike → 可行则实现 Action 与状态联动；不可行则落降级交付与 spike 文档。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-fcitx5 -p ime-diag` 全绿；`keymap_matrix` 含 compose 行。
  - [ ] 走查：X11 档 dead key 组合实测（若宿主键盘布局支持）；日志抽查节流生效。
  - [ ] 回归走查：`engine.compose = false`（默认）时按键路由逐字节一致；spike 文档落盘。

---

## Phase 3 汇总

| 任务 | GAP/IMP | 通道 | 工时 | 关键路径 | 契约义务 |
|---|---|---|---|---|---|
| TASK-3.01.01 拼写纠错 | GAP-11/IMP-10 | A | 3 | 否 | ADR-0014（`CORRECTION` 位） |
| TASK-3.02.01 上文重排 | GAP-12/IMP-11 | A | 2 | 是（CP-1 收口） | 零契约变更 |
| TASK-3.03.01 重码治理 | GAP-13/IMP-05 | C | 2.5 | 否 | 方案 B 时 v2 增段 |
| TASK-3.03.02 词库热替换 | GAP-14/IMP-20 | C | 2.5 | 否 | 新码 `dict/swap/rejected`；新依赖 `arc-swap` |
| TASK-3.04.01 用户造词 | GAP-15/IMP-09 | A | 3 | 否 | 零契约变更（链式组合） |
| TASK-3.05.01 中英混输 | GAP-16/IMP-16 | A+C | 3 | 否 | 独立 eng.dict |
| TASK-3.05.02 错音错字提示 | GAP-17/IMP-17 | C | 2 | 否 | 零 wire 变更 |
| TASK-3.05.03 符号面板 v 模式 | GAP-18/IMP-18 | B | 2.5 | 否 | 零 wire 变更 |
| TASK-3.06.01 切走收尾 + capability | GAP-19/IMP-13 | B | 2 | 否 | 可能补 capability 透传 |
| TASK-3.06.02 compose/节流/StatusArea | GAP-19/IMP-14 | B | 2.5 | 否 | StatusArea 需胶水 spike |

**波次建议**：W1 = {3.01.01, 3.03.01, 3.04.01, 3.05.03, 3.06.01}；W2 = {3.02.01(需 1.03.02 先行), 3.03.02, 3.05.01(需 3.04.01 先行), 3.05.02(需 3.01.01 先行), 3.06.02(需 2.03.01 先行)}。Phase 3 与 Phase 1/2 的跨 Phase 依赖均已在各卡「前置依赖」标注，全部满足编号字典序约束。
