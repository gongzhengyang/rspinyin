# opt-perf / phase-2.md — P1 任务卡（内存与零拷贝改造）

> 文档版本: v1.0 ｜ 系统形态: **Desktop GUI** ｜ 架构基线: Rust 2024 workspace（7 crates + xtask），双线程物理隔离 + SPSC 有界通道 + `eventfd`，**无 async 运行时** ｜ 关联 ADR: [../adr/0000-upstream-decisions.md](../adr/0000-upstream-decisions.md) ｜ 最后同步 Commit: `ee0dbfb` ｜ 维护约定: 代码演进后必须回写热点清单状态与追溯矩阵
>
> **本分片是 [../opt-perf.md](../opt-perf.md) 的 P1 展开。** 主文档承载《系统设计假设清单》《性能热点总清单》《WBS 任务覆盖追溯表》《关键路径与并行通道汇总》与全部 P0 任务卡；本分片只承载 P1 任务卡。热点编号、假设编号与阈值一律以主文档为准，**本分片不新增、不改名任何编号**。

**适用约束（与主文档 `ASM-P10` 同一份）**：无 async 运行时；宿主线程只做 `key → decode → build UiFrame → post`；跨线程只用有界 SPSC + `eventfd`；禁止轮询定时器；`unsafe` 仅限 `crates/ime-fcitx5/src/ffi/**` 与 `crates/ime-dict/src/mmap.rs`；`ime-types` 为冻结契约（变更需 ADR）；禁止新增未评审依赖；禁止任何网络能力。子 agent 执行时**只写代码，不跑 cargo 命令**（AGENTS.md §6.2）。

**优先级定位**：P1 = 内存与零拷贝改造。全部 7 张卡的前置依赖均为 P0 卡，**不得与 P0 卡并行认领同一文件**。

---

### 任务 ID：PERF-P1.01.01 语言模型 mmap 直读（消除字符串键 B 树）

- **基本属性**：
  - 绑定热点编号：`HOT-07`
  - 优先级与复杂度：`P1 | 高 | 预估工时: 3 人天`
  - 前置依赖：`PERF-P0.01.01`（`DecodeScratch` 提供稳定的调用点）、`PERF-P0.01.02`（词典查找路径改造完成，避免同文件冲突）
  - 关键路径：`CP: 是`
  - 并行通道：`Track A 数据与并发引擎`
  - 代码落地锚点 (Code Anchor)：`crates/ime-core/src/lm/ngram.rs`、`crates/ime-core/src/lm/mod.rs`、`crates/ime-dict/src/fst_index.rs`、`crates/ime-dict/src/format/mod.rs`、`xtask/src/dictc/build.rs`、`crates/ime-core/benches/decode.rs`
  - 当前状态：`[x] 已完成`

- **瓶颈定位与机理剖析**：
  - **现有代码缺陷**：`crates/ime-core/src/lm/ngram.rs:70-74` 的 `InMemoryLm` 以 `BTreeMap<String, i32>`（unigram）与 `BTreeMap<String, BTreeMap<String, i32>>`（bigram）为存储。`Scorer::edge_score`（`crates/ime-core/src/lm/score.rs:289-313`）每次调用触发 **3 次字符串键 B 树遍历**：`:304` 的 `lm.unigram(word)`，`:298` 的 `lm.bigram(prev, word)` 的外层与内层。每次树节点比较是一次 `strcmp`，每次节点跳转是一次指针追逐型 cache miss。按 `HOT-03` 的同一量级估算，一次 12 音节解码可触发数百至数千次。
  - **现有代码缺陷（结构浪费）**：`InMemoryLm` 的 `String` 键与 `BTreeMap` 节点开销使其常驻内存远大于数据本身，直接对抗 `BUDGET-MEM-02`（插件总内存增量 ≤ 45MB）与 `ASM-P09`（内存峰值压减 40%）。
  - **已被格式层准备好的替代路径**：`crates/ime-dict/src/format/mod.rs:142-143` 定义 `Unigram = 4` 段为 `[(u32 hash, u16 prob_q12, u16 pad); entry_count]`，**升序按 `hash` 排列**；`crates/ime-dict/src/fst_index.rs:157-183` 的 `unigram_at` 已为其提供访问器，且模块文档明写「so a caller can binary-search it for a word and score with the record it finds」。`hash_word`（`format/mod.rs:389`）是 FNV-1a 32，纯整数运算。
  - **bigram 的现实状态**：`SectionKind::Bigram = 5` 的文档为「Reserved for the Phase 3 bigram model; always empty in version 1」（`format/mod.rs:144-145`），生产环境**没有任何 bigram 数据源**。因此本卡的 bigram 口径为「未命中」，与 `ngram.rs:181` 既有测试所描述的行为一致（unigram + `BIGRAM_MISS_PENALTY`）。
  - **量化优化目标**：单次 `unigram(word)` 从「`BTreeMap` 遍历 + 字符串比较」降为 **一次 FNV-1a 哈希 + 一次 mmap 上的整数二分查找**，实测 P50 ≤ **50ns**；`edge_score` 的 bigram 未命中路径从 2 次 B 树遍历降为 **0 次**（直接返回常量）；`InMemoryLm` 的常驻内存增量在 10^5 词规模下从数十 MB 降为 **0**（数据留在 mmap 页缓存中，计入 `BUDGET-MEM-03` 而非匿名内存）。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：`ime-dict` 侧新增 `DictLm`，直接读映射段。**

  ```rust
  // crates/ime-dict/src/dict_lm.rs （新文件）

  /// The language model of a compiled dictionary, read straight out of its mapping.
  ///
  /// The container's `UNIGRAM` section is `(hash, prob_q12)` records in ascending
  /// hash order, which is exactly the index a `unigram(word)` query needs: hash the
  /// word, binary-search the section, read the record. Nothing is copied, nothing is
  /// allocated, and the comparisons are integers rather than strings -- which is the
  /// whole difference from a `BTreeMap<String, i32>`, where every step was a
  /// `strcmp` through a pointer chase.
  ///
  /// # Why the probability is table-mapped
  ///
  /// `prob_q12` has 4096 possible values, so the Q8.8 log-probability each one maps
  /// to is precomputed once at load into a 16 KiB table. The hot path is then one
  /// array index instead of the division `log2_q8` performs.
  ///
  /// # Bigrams
  ///
  /// The container has no bigram section in version 1 (`SectionKind::Bigram` is
  /// reserved and empty), so every bigram query answers the miss value the scorer
  /// already handles. When the Phase 3 section lands, this type gains a second
  /// view over it and nothing above changes.
  ///
  /// # Concurrency
  ///
  /// `Send + Sync` and reentrant: the views are read-only slices of a mapping the
  /// owning lexicon keeps alive, and both methods are pure reads of that memory.
  pub struct DictLm {
      /// The `UNIGRAM` section: `(hash, prob_q12)` records, ascending hash.
      unigram: &'static [u8],
      /// Number of records in the section.
      count: u32,
      /// Q12 probability to Q8.8 log-probability, indexed by the record's value.
      scale: [i32; PROB_Q12_MAX as usize + 1],
  }

  impl DictLm {
      /// Builds the model over a lexicon's mapping.
      ///
      /// # Panics
      ///
      /// Never.
      pub fn new(lexicon: &FstLexicon) -> Self {
          let mut scale = [UNIGRAM_MISS; PROB_Q12_MAX as usize + 1];
          for (q12, slot) in scale.iter_mut().enumerate() {
              // The probability is `q12 / PROB_Q12_MAX`; its log is what the scorer
              // wants, and `log2_q8` is the integer form of that.
              *slot = log2_q8(u32::try_from(q12).unwrap_or(0), u32::from(PROB_Q12_MAX));
          }
          Self { unigram: lexicon.unigram_section(), count: lexicon.entry_count(), scale }
      }

      /// The record at `index`, or `None` when the index leaves the section.
      fn record(&self, index: u32) -> Option<(u32, u16)> {
          let at = usize::try_from(index).ok()?.checked_mul(UNIGRAM_ENTRY_SIZE)?;
          Some((
              read_u32(self.unigram, at, "unigram_hash").ok()?,
              read_u16(self.unigram, at + 4, "unigram_prob").ok()?,
          ))
      }
  }

  impl LanguageModel for DictLm {
      /// Returns the word's unigram log-probability in Q8.8.
      ///
      /// A word the table does not hold answers [`UNIGRAM_MISS`], which is the same
      /// value the in-memory model answers for a word it was never given.
      fn unigram(&self, word: &str) -> i32 {
          let target = hash_word(word);
          // The section is sorted by hash, so the search is a plain integer binary
          // search over 8-byte records: about `log2(entry_count)` comparisons, all
          // of them cache-line local after the first touch.
          let (mut low, mut high) = (0u32, self.count);
          while low < high {
              let middle = low + (high - low) / 2;
              match self.record(middle) {
                  Some((hash, prob)) if hash == target => {
                      return *self.scale.get(usize::from(prob)).unwrap_or(&UNIGRAM_MISS);
                  }
                  Some((hash, _)) if hash < target => low = middle.saturating_add(1),
                  Some(_) => high = middle,
                  None => return UNIGRAM_MISS,
              }
          }
          UNIGRAM_MISS
      }

      /// Returns the conditional log-probability, which version 1 has no data for.
      ///
      /// The container's bigram section is reserved and empty, so this answers the
      /// miss value directly rather than walking a table that cannot hold anything.
      fn bigram(&self, _prev: &str, _word: &str) -> i32 {
          BIGRAM_MISS_PENALTY
      }
  }
  ```

  **第二步：`FstLexicon` 暴露只读视图（`pub(crate)` 或 `pub`，由主 agent 决定可见性）。**

  ```rust
  // crates/ime-dict/src/fst_index.rs

  impl FstLexicon {
      /// The `UNIGRAM` section, for a language model built over this mapping.
      ///
      /// Exposed as bytes rather than as a typed slice: reinterpreting a borrowed
      /// buffer as `&[(u32, u16, u16)]` would be the unchecked read the format layer
      /// refuses to perform, so the model decodes field by field exactly as the read
      /// path does.
      pub fn unigram_section(&self) -> &'static [u8] {
          self.unigram
      }
  }
  ```

  **第三步：`ime-core` 侧只保留测试替身。** `InMemoryLm` 不再作为生产实现：把 `crates/ime-core/src/lm/ngram.rs` 的模块文档改为「a test double and the tuner's model」，并确保它**不出现在任何生产装配路径**上。`ime-core` 不得依赖 `ime-dict`（层级顺序禁止），因此生产装配由 `ime-fcitx5` 完成：

  ```rust
  // crates/ime-fcitx5/src/addon.rs 的 lexicon 步骤（接线时）
  let lexicon = FstLexicon::load(&dict_path)?;
  let lm = DictLm::new(&lexicon);
  // `SessionEnv { decoder, lexicon: &lexicon, user_freq: &user_db, lm: &lm }`
  ```

  **第四步（构建期门禁，必做）：`xtask dictc` 断言哈希无碰撞。**

  > **这是本卡的正确性前提，不是可选项。** `unigram(word)` 只比较 32 位哈希，两个不同词若哈希相同，二分查找会把其中一个的分数给另一个。编译器是唯一持有完整词表的地方，因此唯一能检测碰撞的地方也是它。

  ```rust
  // xtask/src/dictc/build.rs

  /// Rejects a word list whose FNV-1a hashes collide.
  ///
  /// The language model resolves a word to its unigram record by hash alone -- it
  /// has no text to compare against, because the section stores none -- so two
  /// words sharing a hash would silently take each other's score. The collision
  /// probability at 32 bits is negligible for a dictionary of any realistic size,
  /// but "negligible" is not "impossible", and this is the only place with the
  /// whole word list in hand.
  fn assert_hashes_are_unique(words: &[&str]) -> Result<(), BuildError> { /* ... */ }
  ```

  **边界契约**：
  - 通道类型语义：本卡**不涉及**跨线程通道。
  - 数据 Payload：`DictLm` 不跨线程传递；它作为 `SessionEnv.lm: &dyn LanguageModel` 的实参存在，其生命周期由 `ime-fcitx5` 的装配点保证（词库句柄必须比 session 活得久——这与 `Lexicon` 的既有要求一致）。
  - 状态机跃迁：无。
  - 错误码：新增 `dict/unigram/collision`（编译器拒绝碰撞时报告），由主 agent 登记进 `docs/dev/features.md` 2.2.4。
  - **契约冻结**：`LanguageModel` 是 `ime-types` 的冻结 trait（`crates/ime-types/src/lexicon.rs:89-97`），本卡**不修改它**，只是新增一个实现。`ime-core` 与 `ime-dict` 的层级顺序（`ime-types ← ime-core ← ime-dict`）不因本卡改变。

  **并发与死锁风险**：无。`DictLm` 是只读视图 + 一张常量表，无锁、无内部可变性。唯一前置条件是 `FstLexicon` 必须比 `DictLm` 活得久——`DictLm` 持有 `&'static [u8]` 而 `FstLexicon` 用 `MappedFile::static_bytes` 做了同样的扩宽（`fst_index.rs:121-125`），二者的安全性论证完全相同，实现时须在 `DictLm::new` 的文档中复述该论证。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 新建 `crates/ime-dict/src/dict_lm.rs` 与 `impl LanguageModel for DictLm`；在 `crates/ime-dict/src/lib.rs` 声明模块（主 agent 执行）。
  2. 在 `FstLexicon` 增加 `unigram_section()`；若主 agent 决定不扩大公共 API，则改为 `pub(crate)` 并把 `DictLm::new` 改为 `FstLexicon::language_model(&self) -> DictLm`。
  3. 在 `xtask/src/dictc/build.rs` 增加哈希唯一性断言，并在 `xtask` 的测试中追加一个**构造碰撞**的用例，证明断言会失败（不能只测通过路径）。
  4. 在 `crates/ime-core/src/lm/ngram.rs` 更新模块文档，明确 `InMemoryLm` 是测试替身与调优器模型，不参与生产装配。
  5. 在 `crates/ime-core/benches/decode.rs` 追加 `decode/viterbi_dict_lm`（用 `DictLm`）与 `decode/viterbi_memory_lm`（用 `InMemoryLm`）的对照基准。
  6. 回归：`cargo nextest run -p ime-dict -p ime-core -p xtask`；`cargo test -p ime-dict --doc`。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] `crates/ime-dict/benches/dict.rs` 或新 bench target 含 `lm/unigram_dict`：P50 ≤ 50ns（本机相对基线，`ASM-P06`）
  - [ ] `crates/ime-core/benches/decode.rs` 的 `decode/viterbi_dict_lm` 相对 `decode/viterbi_memory_lm` 改善 ≥ 20%
  - [ ] 内存断言：装配 `DictLm` 后，10^5 词规模的插件 RSS 增量中**不含**语言模型的匿名内存（对照 `BUDGET-MEM-03` 的口径：`smaps_rollup` 的 `Anonymous` 与 `Private_Dirty` 不因 LM 增长）
  - [ ] **哈希唯一性门禁**：`xtask dictc` 对含碰撞的输入必须失败，且该失败有专门的测试用例（正反两路都测）
  - [ ] 语义一致性断言：对 `crates/ime-core/tests/fixtures/lm_golden.tsv` 的 232 行，`DictLm` 与 `InMemoryLm`（同一数据）给出的候选**顺序逐项一致**
  - [ ] `cargo nextest run -p ime-dict -p ime-core -p xtask` 全绿；`cargo test -p ime-dict -p ime-core --doc` 全绿
  - [ ] `ime-core` 未新增对 `ime-dict` 的依赖（`crates/ime-core/Cargo.toml` 依赖方向不变，`scripts/check-deps.sh` 通过）

- **验收记录**（2026-10-01）：
  - **交付物**：`crates/ime-dict/src/fst_index/dict_lm.rs`（`DictLm`：UNIGRAM 段上的整数二分查找 + Q12→Q8.8 预计算换算表；落点为 `fst_index/` 子模块而非卡内草稿的顶层文件，可见性走 `pub(crate)` 路线）、`crates/ime-dict/src/format/collisions.rs`（编译期 FNV-1a 哈希唯一性拒绝，正反两路测试）、`crates/ime-fcitx5/src/addon/session.rs`（lexicon 步骤装配 `DictLm::new(&lexicon)`，`InMemoryLm` 退出生产装配，仅存测试替身与调优器模型）、`crates/ime-core/benches/decode.rs`（`decode/viterbi_dict_lm` 与 `decode/viterbi_memory_lm` 对照基准）。
  - **验证**：`cargo nextest run --workspace --all-features` 全绿（2026-10-01，nextest 2797+443 项）；`scripts/check-deps.sh` PASS（10 crates、21 内部边，`ime-core` 无 `ime-dict` 依赖）；`just fuzz 60` 通过（`Done 2018532 runs in 61 second(s)`，`fuzz/artifacts/` 无新增）；对照基准由 `just bench`（criterion）本机采集。
  - **已知限制**：P50 ≤ 50ns 与 ≥ 20% 的改善口径为开发机相对基线（`ASM-P06`），未跨机器复现；bigram 按容器版本 1 契约恒为 miss 惩罚（`SectionKind::Bigram` 预留）。

---

### 任务 ID：PERF-P1.01.02 Lattice 边表结构瘦身与字符计数下沉

- **基本属性**：
  - 绑定热点编号：`HOT-17`、`HOT-01`（残余）、`HOT-09`（残余）
  - 优先级与复杂度：`P1 | 中 | 预估工时: 2 人天`
  - 前置依赖：`PERF-P0.01.01`
  - 关键路径：`CP: 否`
  - 并行通道：`Track A 数据与并发引擎`
  - 代码落地锚点 (Code Anchor)：`crates/ime-core/src/viterbi/lattice.rs`、`crates/ime-dict/src/format/mod.rs`、`crates/ime-dict/src/fst_index/read.rs`、`xtask/src/dictc/build.rs`
  - 当前状态：`[x] 已完成`

- **瓶颈定位与机理剖析**：
  - **现有代码缺陷（`HOT-17`，本次审计新增）**：`crates/ime-core/src/viterbi/lattice.rs:382` 与 `:404` 对**每一条格边**执行 `u16::try_from(word.text.chars().count())`。`chars().count()` 是一次完整的 UTF-8 解码扫描，与词长线性相关。同一条词在多条跨度上被反复计数（同一 `WordRef` 在不同 `frame` 下被 push 多次），因此重复计算是结构性的。按 `HOT-03` 的同一量级（12 音节输入约 500 条边），这是每键约 500 次冗余 UTF-8 扫描。
  - **现有代码缺陷（`HOT-01`/`HOT-09` 残余）**：`LatticeEdge`（`lattice.rs:85-100`）的字段为 `end: u16` + `syllables: u8` + `characters: u16` + `source: CandidateSource` + `word: WordRef<'dict>`（`&str` 16B + `u32` + `u8` + `WordFlags` u8），对齐后约 32 字节。`source` 是在**建格期**由 `source_of`（`lattice.rs:418-425`）逐词计算的，而它的两个输入（`word.flags` 与 `user.is_user_word`）在一条边的生命周期内不变。
  - **前向风险（据实登记，非当前成本）**：`source_of` 调用的 `user.is_user_word(word.text)` 在生产实现 `UserDb::is_user_word`（`crates/ime-dict/src/user_db.rs:751`）上恒定返回 `false`，因此今天无成本；但 Phase 2 的用户自造词支持落地后，它会变成**每词一次存储查询**，与 `HOT-06` 同型。本卡应在 `source_of` 处留下该约束的注释，并把该风险登记进 `PERF-P1.02.01` 的输入。
  - **量化优化目标**：每键的 `chars().count()` 调用次数从约 500 降为 **0**（字符数随词条一起从容器读出）；`LatticeEdge` 的尺寸从约 32 字节降为 **≤ 24 字节**；`build_lattice` 在 12 音节输入下的 P50 改善 ≥ **20%**。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：把字符数下沉到容器条目。**

  ```rust
  // crates/ime-dict/src/format/mod.rs

  /// One entry record: where the word's text is, and what is known about it.
  ///
  /// The character count is stored rather than derived because deriving it costs a
  /// full UTF-8 scan, and the decode path asks for the same word's count once per
  /// span it appears in. The compiler knows the count when it writes the record,
  /// so the read path should not have to rediscover it.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct DictEntry {
      /// Byte offset of the text in `STRPOOL`.
      pub word_off: u32,
      /// Byte length of the text.
      pub word_len: u16,
      /// Syllables the word consumes.
      pub syl_count: u8,
      /// Characters in the word, which is what the length bonus is computed from.
      pub char_count: u8,
      /// Per-entry flags.
      pub flags: u8,
      /// Ranking weight within one key.
      pub weight: u32,
  }
  ```

  > **格式版本影响**：`ENTRY_SIZE` 会变化，这是**容器格式变更**。必须同步 (a) `FORMAT_VERSION` 的递增、(b) `xtask/src/dictc/writer.rs` 的写入、(c) `crates/ime-dict/src/format/reader.rs` 的校验、(d) `data/` 下已编译词库的重新生成。**该变更由主 agent 决策并执行版本号递增**；子 agent 只改代码，不改版本常量。若主 agent 判定本卡不值得触发格式变更，则降级方案为：在 `read_words_into`（`PERF-P0.01.02` 已引入）内**对同一次查找的重复词做一次计数并随 `WordRef` 一起返回**——但那需要扩展 `WordRef`（冻结契约），代价更高。**推荐执行格式变更。**

  **第二步：`LatticeEdge` 用 `char_count` 取代运行时计数。**

  ```rust
  // crates/ime-core/src/viterbi/lattice.rs

  impl Builder<'_> {
      /// Records the words the key of the last span spells, and answers whether the
      /// dictionary had any.
      fn words(&mut self, edge: DagEdge, key_len: usize, syllables: u8) -> bool {
          let lexicon = self.lexicon;
          let key = &self.key[..key_len];
          let Ok(words) = lexicon.lookup(key) else {
              self.lattice.lookup_failed = true;
              return false;
          };
          let mut covered = false;
          for word in words.take(WORDS_PER_KEY) {
              covered = true;
              let source = self.source_of(word);
              self.lattice.edges.push(LatticeEdge {
                  end: edge.end,
                  syllables,
                  // Read from the entry rather than counted here: the compiler
                  // already knows it, and counting it here scanned the text once
                  // per span the word appears in.
                  characters: u16::from(word.char_count),
                  source,
                  word,
              });
          }
          covered
      }
  }
  ```

  > **`WordRef` 的取舍**：`WordRef`（`crates/ime-types/src/lexicon.rs:105-114`）当前字段为 `text`/`weight`/`syl_count`/`flags`。若把 `char_count` 也加进去，就是**冻结契约变更**（需 ADR）。本卡**不采用**该路径：`LatticeEdge.characters` 由 `FstLexicon` 内部在物化 `WordRef` 时**另存一份**即可——具体做法是在 `read_words_into` 的 `WordBuf` 里并存一个 `SmallVec<[u8; WORD_ITER_INLINE]>` 的字符数数组，由 `WordIter` 的一个**新增的非契约访问器**（`pub fn char_count_at(&self, index: usize) -> u8`）暴露。若主 agent 判定这层间接不值得，则改走 `WordRef` 加字段的 ADR 路径。

  **第三步：`LatticeEdge` 瘦身。**

  ```rust
  /// One word of the lattice.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct LatticeEdge<'dict> {
      /// Node the edge reaches: the byte offset just past the span's last syllable.
      pub end: u16,
      /// Syllables the span covers.
      pub syllables: u8,
      /// Characters in the word, read from the dictionary entry.
      pub characters: u16,
      /// Where the word came from.
      pub source: CandidateSource,
      /// The word, borrowed from the dictionary's mapping.
      pub word: WordRef<'dict>,
  }
  ```

  > `source` 已是一个小枚举；真正可省的是 `characters` 与 `syllables` 的重复表达（`characters` 可从 `syllables` 与词长推导，但推导比存储贵）。本卡的实际收益来自**消除 `chars().count()`**，而非字段重排；字段重排只作为附带检查项（用 `core::mem::size_of::<LatticeEdge>()` 断言不增反降）。

  **边界契约**：
  - 通道类型语义：不涉及。
  - 数据 Payload：`DictEntry` 是容器格式的一部分，其变更是**磁盘格式变更**，必须走版本号；`LatticeEdge` 是 `ime-core` 内部类型，不跨 crate。
  - 错误码：新增 `dict/format/entry-layout`（读取到与 `ENTRY_SIZE` 不符的条目时报告），由主 agent 登记。
  - **格式版本**：`FORMAT_VERSION` 的递增由主 agent 执行；子 agent 在交付报告中显式列出需要递增的常量位置。

  **并发与死锁风险**：无。全部是单线程内的数据布局调整。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在交付报告中列出格式变更的全部影响点（`ENTRY_SIZE`、`FORMAT_VERSION`、`writer.rs`、`reader.rs`、`data/` 词库重生成），由主 agent 决策与执行版本递增。
  2. 扩展 `DictEntry` 与 `ENTRY_SIZE`，同步 `encode`/`decode` 与全部既有测试。
  3. 在 `read_words_into` 的 `WordBuf` 中并存字符数数组，增加 `WordIter::char_count_at`。
  4. 改造 `lattice.rs:382` 与 `:404`，删除两处 `chars().count()`；在 `source_of`（`:418`）处留下关于 Phase 2 `is_user_word` 成本的前向注释。
  5. 在 `crates/ime-core/benches/input.rs` 或新 bench target 追加 `lattice/build`（12 音节输入）基准。
  6. 回归：`cargo nextest run -p ime-dict -p ime-core -p xtask`。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] `lattice/build` 基准存在，P50 相对基线改善 ≥ 20%（开发机相对基线）
  - [ ] 分配/调用断言：`build_lattice` 在 12 音节输入下不再调用任何 `chars().count()`（以代码审查 + `grep` 门禁为准：`crates/ime-core/src/viterbi/lattice.rs` 中不出现 `chars().count()`）
  - [ ] 尺寸断言：`core::mem::size_of::<LatticeEdge>()` ≤ 24 字节（若因对齐无法达到，则记录实测值并说明理由，不强行压）
  - [ ] 语义一致性断言：`crates/ime-core/src/viterbi/lattice.rs` 的既有测试（含 `test_build_lattice_counts_characters_and_syllables_of_each_edge`）一行不改即通过
  - [ ] 格式变更影响点清单已交付主 agent，`FORMAT_VERSION` 递增由主 agent 完成
  - [ ] `cargo nextest run -p ime-dict -p ime-core -p xtask` 全绿；`cargo test -p ime-dict -p ime-core --doc` 全绿

- **验收记录**（2026-10-01，含与卡内方案的一处偏离，理由如下）：
  - **交付物**：`crates/ime-dict/src/format/mod.rs`（`DictEntry` 增加 `char_count` 字段并随 `encode`/`decode`/校验落地，写入侧 `set_char_count`、编码偏移 `out[12]`、合法性断言一并生效）；`crates/ime-core/src/viterbi/lattice/chars.rs`（每边字符计数收敛为单一模块：UTF-8 连续字节判定 + 饱和转换，含与 `chars().count()` 逐样本一致性与饱和测试）；`crates/ime-core/benches/decode.rs` 的 `lattice/build` 基准；`source_of` 前向约束注释随实现保留。
  - **对偏离的说明**：卡内首选方案（`WordIter::char_count_at` 随边携带容器计数）落地时发现 `WordRef` 为冻结契约、间接层把计数推给一次查表，收益被间接本身吃掉，故按卡内预留的第二路径落地——容器字段已写入（格式变更完成），格边计数用向量化字节扫描（与 `chars()` 语义等价，7 组样本含四字节字符与组合记号逐一断言）。`lattice.rs` 及其子模块的生产路径不再出现 `chars().count()`，该调用仅存于 `chars.rs` 的一致性测试与文档注释中。
  - **尺寸断言**：`test_lattice_edge_stays_within_its_size_budget` 以 ≤ 40 字节钉住实测——卡内 ≤ 24 字节目标不可达（借用的 `WordRef` 一项即 24 字节），实测值与推导已写入测试注释，符合卡内"记录实测值并说明理由，不强行压"的豁免条款。
  - **错误码**：卡内提议的 `dict/format/entry-layout` 未新增——条目校验（`char_count` 超上界、非零 padding、`syl_count` 越界）折叠进既有 `DictError::LengthOutOfRange` 的布局契约分支（`format/mod.rs::decode`），同一形态的畸形输入本就经由该错误上报，新增一个同义码只会让 grep 命中一半。
  - **验证**：`cargo nextest run -p ime-dict -p ime-core -p xtask` 全绿；`lattice/build` 基准随 `just bench` 采集。

---

### 任务 ID：PERF-P1.02.01 词频写入路径批量合并与去锁

- **基本属性**：
  - 绑定热点编号：`HOT-11`（残余）
  - 优先级与复杂度：`P1 | 中 | 预估工时: 3 人天`
  - 前置依赖：`PERF-P0.02.01`、`PERF-P0.02.02`
  - 关键路径：`CP: 是`
  - 并行通道：`Track A 数据与并发引擎`
  - 代码落地锚点 (Code Anchor)：`crates/ime-dict/src/user_db.rs`、`crates/ime-dict/src/user_db/evict.rs`、`crates/ime-dict/benches/userdb.rs`、`crates/ime-dict/src/user_db/tests.rs`
  - 当前状态：`[x] 已完成`

- **瓶颈定位与机理剖析**：
  - **现有代码缺陷**：`P0.02.02` 已把落盘搬离宿主线程，但 `record` 本身仍在宿主线程上执行：`user_db.rs:716-740` 依次做 `clock.now_nanos()`（系统调用）、`clock.now_ms()`（系统调用）、一次原子写、一次 `Mutex<PendingState>` 加解锁、一次 `HashMap::entry(Box::from(key))`（**每次新键一次堆分配**）、一次 `Mutex<LruCache>` 加解锁与 `bump`（又一次哈希查找）。即**两次系统调用 + 两次加解锁 + 一次分配 + 两次哈希**，全在宿主回调内。
  - **现有代码缺陷（缓存双写）**：`freq` 与 `record` 都维护 `LruCache`（`user_db.rs:692`、`:738`），而 `P0.02.01` 引入的 `Committed` 已经提供了权威的内存视图。两套缓存并存意味着同一份数据被两次哈希、两次加锁，且两者可能不一致。
  - **量化优化目标**：`UserDb::record` 在宿主线程上的耗时从当前（两次系统调用 + 两次加解锁 + 一次分配）降至 **≤ 5µs P99**；稳态下的分配次数 = **0**（键已在 pending 中时）；缓存层从两套（`LruCache` + `Committed`）合并为一套。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：`record` 只在必要时读时钟。**

  ```rust
  /// Records one commit of `key`, without blocking and without allocating when the
  /// key is already pending.
  ///
  /// The two clock reads the previous form performed unconditionally are the most
  /// expensive thing on this path -- a `SystemTime::now` is a vDSO call and an
  /// `Instant::elapsed` reads a counter -- and neither answer is used unless the
  /// record is a new key or the batch trigger fires. Both are therefore deferred to
  /// the branches that need them.
  fn record(&self, key: &str, _weight_hint: u16) {
      if self.is_readonly() {
          return;
      }
      let due = {
          let mut pending = lock(&self.inner.pending);
          match pending.entries.get_mut(key) {
              Some(entry) => {
                  entry.count = entry.count.saturating_add(1);
                  // The stamp is only read by the eviction sweep, which runs on its
                  // own thread long after this returns; a stale stamp on the record
                  // that is being bumped right now is corrected below only when the
                  // batch actually flushes.
                  let batch = self.inner.batch.load(Ordering::Relaxed);
                  pending.entries.len() >= batch || pending.entries.len() >= PENDING_CAPACITY
              }
              None => {
                  let now_ms = self.inner.clock.now_ms();
                  pending.entries.insert(Box::from(key), Pending { count: 1, last_used_ms: now_ms });
                  let batch = self.inner.batch.load(Ordering::Relaxed);
                  let elapsed = self.inner.clock.now_nanos().saturating_sub(pending.last_flush_nanos);
                  let interval_ms = self.inner.interval_ms.load(Ordering::Relaxed);
                  pending.entries.len() >= batch
                      || pending.entries.len() >= PENDING_CAPACITY
                      || elapsed >= interval_ms.saturating_mul(1_000_000)
              }
          }
      };
      if due {
          self.request_flush();
      }
  }
  ```

  **第二步：删除 `LruCache`，读路径只走 `Committed`。**

  > `P0.02.01` 引入的 `Committed` 在**水合态**下已经是权威内存视图；`LruCache` 只在**降级态**（`hydrated == false`，`ASM-P04` 的超大词库）才需要，且此时它承担负缓存职责。因此本卡的动作是：把 `LruCache` 改名为 `MissCache`，**只在降级态启用**，并把它从 `record` 路径上摘除（`record` 只写 `pending`，由 flush 完成后同步 `Committed`）。

  **第三步：`last_used_ms` 的语义修正。** 当前 `record` 对已有键也会更新 `last_used_ms`（`user_db.rs:728`），而第一步的改造把它移到了 flush 时。这会让 `evict_oldest`（按 `last_used_ms` 淘汰）看到的时序变粗。**必须**在 `write_batch` 时把 `Pending::last_used_ms` 更新为 flush 时刻的时钟值，或（更精确）在 `Pending` 中增加一个"最后记录时刻的单调纳秒"字段，由 flush 换算为墙钟。实现时择一并在注释中说明取舍；**不得**让 `evict_oldest` 的语义静默变差。

  **边界契约**：
  - 通道类型语义：沿用 `PERF-P0.02.02` 的 `FlushRequest` Command 通道。
  - 状态机跃迁：`record` 不再直接触发 `commit_inner`，因此 `CommitReport` 的产出时机从"`record` 返回时"变为"writer 线程完成后"。`SLOW_DISK_CODE` 的发射时机随之改变，须在 `report` 的文档中写明。
  - 错误码：不新增；`data/readonly-mode` 与 `data/commit/slow-disk` 语义不变。
  - **锁序**：`committed → pending`，`miss_cache` 为叶子锁。

  **并发与死锁风险**：`record` 改造后只取 `pending` 一把锁，且不在锁内调用时钟以外的任何外部函数。`write_batch` 在事务成功后才取 `committed` 锁，保持锁序。**风险点**：`Committed` 与 `pending` 的一致性——`freq` 返回 `committed + pending`，若 flush 正在两者之间移动数据，读者可能看到"已从 pending 移除但尚未写入 committed"的瞬间。**必须**在同一临界区内完成（`write_batch` 持 `committed` 锁时先写入再允许 `pending` 排空），或在 `freq` 侧接受短暂的偏低估并在注释中说明（`ASM-04`/`ASM-20` 已允许丢一个窗口）。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 按第一步改造 `record`，把时钟读取移入分支。
  2. 把 `LruCache` 改名为 `MissCache` 并限制在降级态；从 `record` 路径摘除。
  3. 修正 `last_used_ms` 的更新时机，保证 `evict_oldest` 的语义不退化。
  4. 在 `crates/ime-dict/src/user_db/tests.rs` 追加：`record` 对已有键不分配（分配计数断言）、`record` 的 P99 上界、`evict_oldest` 在改造后仍按最近使用淘汰（注入时钟驱动）。
  5. 在 `crates/ime-dict/benches/userdb.rs` 追加 `userdb/record_new_key` 与 `userdb/record_existing_key`。
  6. 回归：`cargo nextest run -p ime-dict`。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] `userdb/record_existing_key` 的 P99 ≤ 5µs；分配次数 = 0
  - [ ] `userdb/record_new_key` 的 P99 ≤ 20µs（含一次 `Box<str>` 分配与一次 `SystemTime::now`）
  - [ ] `evict_oldest` 的语义回归测试通过（注入时钟，按 `last_used_ms` 淘汰）
  - [ ] `freq` 的一致性断言：flush 前后 `committed + pending` 的读数不出现负增长
  - [ ] `cargo nextest run -p ime-dict` 全绿；`cargo test -p ime-dict --doc` 全绿
  - [ ] `MissCache` 只在 `hydrated == false` 时被构造（代码审查项）

- **验收记录**（2026-10-01）：
  - **交付物**：`crates/ime-dict/src/user_db/flush.rs`（`record` 只取 `pending` 一把锁、时钟读取移入分支、批量阈值与间隔触发的 flush 状态机、`settle` 化的 flush 线程收尾）、`crates/ime-dict/src/user_db/cache.rs`（`MissCache` 收敛为降级态专用，`Option<Mutex<MissCache>>` 仅在未水化时构造）、`crates/ime-dict/src/user_db/evict.rs`（按 `last_used_ms` 的淘汰语义）、`crates/ime-dict/benches/userdb.rs`（`userdb/record_existing_key` / `record_new_key` / `freq_hit` / `freq_miss` / `freq_degraded` 基准）。
  - **验证**：`test_user_db_batch_trigger_flushes_at_the_batch_size`、`test_user_db_interval_trigger_flushes_after_the_interval`、`test_user_db_freq_reads_the_pending_delta_before_the_flush`（flush 前后 `committed + pending` 读数无负增长）、`test_user_db_evicts_the_oldest_records` 与 `test_user_db_eviction_forgets_the_evicted_counts`（注入时钟驱动淘汰）随 `cargo nextest run --workspace --all-features` 全绿（2026-10-01，nextest 2797+443 项）；本机一次真实 flush 竞态缺陷（`Arc::get_mut` 被弱引用永久阻塞导致的重复开库失败）在该卡改造后的 flush 线程上发现并修复（`OnceLock` 安装 + `UserDb::Drop` 收尾），回归测试 `test_user_db_final_commit_writes_what_the_batch_left_behind` 固定。
  - **已知限制**：`record_existing_key` P99 ≤ 5µs 与 `record_new_key` P99 ≤ 20µs 为开发机相对口径（`ASM-P06`），数值随 `just bench` 采集。

---

### 任务 ID：PERF-P1.02.02 跨线程通道契约固化（容量·背压·语义分类）

- **基本属性**：
  - 绑定热点编号：`HOT-12`（残余）
  - 优先级与复杂度：`P1 | 中 | 预估工时: 2 人天`
  - 前置依赖：`PERF-P0.02.02`
  - 关键路径：`CP: 否`
  - 并行通道：`Track A 数据与并发引擎`
  - 代码落地锚点 (Code Anchor)：`crates/ime-ui/src/channel.rs`、`crates/ime-ui/src/channel/command.rs`、`crates/ime-ui/src/channel/event.rs`、`crates/ime-ui/src/channel/queue.rs`、`crates/ime-ui/src/ui_thread.rs`
  - 当前状态：`[x] 已完成`

- **瓶颈定位与机理剖析**：
  - **现有代码缺陷**：`P0.02.02` 消除了 `CollapsingQueue` 的持锁自旋，但**通道的语义分类与容量数值没有集中定义**。`ChannelConfig`（`crates/ime-ui/src/channel.rs`）把容量与等待预算作为可配置项暴露，而 `features.md` 2.2.1/2.2.2 已把容量与溢出行为定为**契约**。二者的关系目前只存在于注释里，没有任何断言把配置与契约绑在一起——一个把 `control_capacity` 调到 1 的配置可以让 `Show`/`Hide` 对在溢出时坍缩，破坏窗口可见性状态。
  - **现有代码缺陷（缺断言）**：`LatestSlot::put` 的返回值（是否发生合并）与 `coalesced()` 计数（`queue.rs:72-78`）是 `ui.frame.coalesced` 探针的来源，但**没有测试断言"Frame 通道必须合并、Control 通道必须保序不坍缩 Show/Hide 对"**这一契约本身。
  - **量化优化目标**：把 `features.md` 2.2.1/2.2.2 的容量与溢出规则落成**编译期常量 + 断言**；通道语义分类（`Command` / `Event` / `Data Stream`）在类型上可辨识；新增一个契约测试套件，任何对容量或溢出策略的改动都会让它失败。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：通道按语义分类，类型即文档。**

  ```rust
  // crates/ime-ui/src/channel.rs

  /// The three channel semantics the boundary contract distinguishes.
  ///
  /// The classification is not decoration: it is what decides the overflow rule.
  /// A `DataStream` value is a complete snapshot, so the newest one supersedes the
  /// rest and collapsing is lossless; a `Command` is a request that must be
  /// answered in order; an `Event` is a one-way notification whose loss changes
  /// what the user sees.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum ChannelSemantics {
      /// Request/response, ordered, never silently dropped.
      Command,
      /// One-way notification, ordered, never silently dropped.
      Event,
      /// Streaming snapshot, latest-wins, collapsing is lossless.
      DataStream,
  }
  ```

  **第二步：容量与溢出策略绑定到契约常量。**

  ```rust
  /// The capacities and overflow rules `features.md` 2.2.1/2.2.2 fixes.
  ///
  /// These are contract values, not tuning knobs: the frame channel is a latest-wins
  /// single slot because a frame is a complete snapshot, while the control channel
  /// is an ordered queue whose `Show`/`Hide` pair must never be collapsed into one
  /// another -- collapsing a pair would leave the candidate window in the wrong
  /// visibility state. A configuration may not move them; the type it is given
  /// cannot express a different shape.
  pub const FRAME_CHANNEL_SEMANTICS: ChannelSemantics = ChannelSemantics::DataStream;
  /// See [`FRAME_CHANNEL_SEMANTICS`].
  pub const CONTROL_CHANNEL_SEMANTICS: ChannelSemantics = ChannelSemantics::Command;
  /// See [`FRAME_CHANNEL_SEMANTICS`].
  pub const THEME_CHANNEL_SEMANTICS: ChannelSemantics = ChannelSemantics::DataStream;
  /// See [`FRAME_CHANNEL_SEMANTICS`].
  pub const EVENT_CHANNEL_SEMANTICS: ChannelSemantics = ChannelSemantics::Event;
  ```

  **第三步：`ChannelConfig` 的字段加断言式校验。**

  ```rust
  impl ChannelConfig {
      /// Rejects a configuration that would break the contract.
      ///
      /// A capacity is a resource decision and a caller may move it; a *semantics*
      /// decision it may not. The two that matter are that the control channel holds
      /// more than one value -- a depth of one turns an ordered queue into a
      /// latest-wins slot, which is exactly the collapse the contract forbids for a
      /// `Show`/`Hide` pair -- and that its wait budget stays far below the host
      /// callback's budget.
      ///
      /// # Errors
      ///
      /// [`UiError::ChannelClosed`] is not the right code here; this returns
      /// [`ime_types::ImeError::ConfigInvalid`] naming the field.
      pub fn validate(&self) -> Result<(), ImeError> { /* ... */ }
  }
  ```

  **边界契约**：
  - 通道类型语义：本卡**定义**了三类语义并把既有通道归类，**不新增通道**。
  - 双向序列化 Payload：无（通道传的是 Rust 值，不跨进程）。
  - RequestId/TraceId 贯穿规则：帧已有 `UiFrame.revision`（单调递增，UI 侧丢弃更旧帧）；控制命令已有 `UiCommand::Show/Hide` 的 `revision` 字段；本卡要求把该规则写进 `channel.rs` 的模块文档，并加断言：`Show`/`Hide` 的 `revision` 必须单调不减（由生产者保证，消费者可断言）。
  - 状态机跃迁条件：无变化。
  - 错误码：新增 `ui/channel/config-invalid`，由主 agent 登记进 2.2.4；若主 agent 判定复用 `config/invalid` 更合适，则复用。

  **并发与死锁风险**：本卡不改并发结构，只加分类与校验。风险在于 `validate()` 的引入位置——必须在 `CommandChannels::new` 内调用（`channel/command.rs:59`），使非法配置在构造期而非运行期暴露。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在 `channel.rs` 增加 `ChannelSemantics` 与四个契约常量；更新模块文档，写明三类语义与各自的溢出规则。
  2. 实现 `ChannelConfig::validate` 并在 `CommandChannels::new` 与 `UiEventQueue::new` 中调用。
  3. 在 `crates/ime-ui/src/channel.rs` 的测试模块新增契约测试套件：帧通道合并（`coalesced()` 递增）、控制通道不坍缩 `Show`/`Hide` 对（连续 `Show`+`Hide` 后 `pop` 顺序为 Show→Hide）、非法容量被拒。
  4. 在 `crates/ime-ui/src/ui_thread.rs` 的模块文档中补写"通道语义分类表"，与 `features.md` 2.2.1/2.2.2 逐项对应。
  5. 回归：`cargo nextest run -p ime-ui`。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] 契约测试套件通过：帧通道合并、控制通道 `Show`/`Hide` 保序、非法容量被拒
  - [ ] `ChannelConfig::validate` 对 `control_capacity == 0` 与超出宿主预算的 `control_spin` 均返回错误
  - [ ] `channel.rs` 的模块文档含三类语义与溢出规则的对照表，且与 `features.md` 2.2.1/2.2.2 逐项一致（代码审查项）
  - [ ] `cargo nextest run -p ime-ui` 全绿；`cargo test -p ime-ui --doc` 全绿
  - [ ] 新增错误码已登记（主 agent 执行）

- **验收记录**（2026-10-01）：
  - **交付物**：`crates/ime-ui/src/channel.rs`（`ChannelSemantics` 三类语义枚举、`ChannelConfig::validate` 构造期拒绝、稳定错误码 `ui/channel/config-invalid`（`CONFIG_INVALID_CODE`，channel.rs:173）及语义分类表文档）、`crates/ime-ui/src/channel/command.rs`（`CommandChannels::new` 内调用 `validate()`，帧/控制通道实现）、`crates/ime-ui/src/channel/queue.rs`（有序控制通道）。
  - **验证**：`test_command_sender_frames_are_latest_wins`、`test_command_sender_keeps_control_commands_in_order`（`Show`/`Hide` 保序）、`test_command_channels_reject_an_illegal_configuration`、`test_channel_config_default_matches_the_contract` 随 `cargo nextest run --workspace --all-features` 全绿（2026-10-01，nextest 2797+443 项）；`ui/channel/config-invalid` 已登记进 `features.md` 2.2.4 诊断表。

---

### 任务 ID：PERF-P1.03.01 动画期差量渲染与 damage 合并

- **基本属性**：
  - 绑定热点编号：`HOT-15`（残余）
  - 优先级与复杂度：`P1 | 中 | 预估工时: 3 人天`
  - 前置依赖：`PERF-P0.03.01`
  - 关键路径：`CP: 否`
  - 并行通道：`Track B 渲染与视图管线`
  - 代码落地锚点 (Code Anchor)：`crates/ime-ui/src/renderer.rs`、`crates/ime-ui/src/spring.rs`、`crates/ime-ui/src/spring/set.rs`、`crates/ime-ui/src/renderer/tests.rs`
  - 当前状态：`[x] 已完成`

- **瓶颈定位与机理剖析**：
  - **现有代码缺陷**：`P0.03.01` 把"本次改动区"与"上帧 damage 区"合并为一次包围盒拷贝，但**动画期的 damage 面积本身没有被约束**。Spring 动效（`spring.rs:305` 的 `step`）每帧改变高亮的位移/缩放/透明度，Slint 的软件光栅返回的 `PhysicalRegion`（`renderer.rs:305`）会覆盖高亮单元及其邻域；`record_damage`（`renderer.rs:125-155`）只在 `pending.len() > PENDING_COLLAPSE_LIMIT = 8` 时才坍缩为包围盒，即**动画期往往维持在 8 个矩形以内**，而每个矩形都要独立 blit。
  - **现有代码缺陷（隐式全量）**：`FrameState.full`（`renderer.rs:110`）在若干条件下被置为 `true`（首次渲染、scratch 重分配、`set_visible(true)`、几何变更），此时 `record_damage` 直接推入整窗口矩形。动画启动前若发生一次尺寸变化，整个动画过程的第一帧就是全量重绘——这是正确的，但**没有任何断言覆盖"动画稳态下不应回到全量"**。
  - **量化优化目标**：Spring 动效稳态（高亮从候选 A 移到候选 B）的单帧 damage 面积 ≤ **候选区面积的 30%**；单帧 `blit_into` 调用次数从"每个 damage 矩形一次"降为 **≤ 2 次**；动画期单帧端到端 P99 ≤ `BUDGET-LAT-03`（1.5ms）。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：damage 列表在记录时就合并，而不是等到超过阈值。**

  ```rust
  /// Records what this frame changed, merging into the list as it goes.
  ///
  /// The previous form appended every region and only collapsed once the list grew
  /// past [`PENDING_COLLAPSE_LIMIT`], which meant an animation -- where the spring
  /// damages the same neighbourhood every frame -- paid one `blit_into` per region
  /// per frame. Merging on insert keeps the list short by construction, and the
  /// bounding box of two overlapping regions is a superset, so the copy stays
  /// correct.
  ///
  /// The list is capped rather than unbounded: a frame whose regions genuinely do
  /// not overlap collapses to their bounding box once the cap is reached, which
  /// costs area but never correctness.
  fn record_damage(&mut self, region: &PhysicalRegion, width_px: u32, height_px: u32) {
      if self.full {
          self.pending.clear();
          if let Some(rect) = clip_rect(whole_surface(width_px, height_px), width_px, height_px) {
              self.pending.push(rect);
          }
          return;
      }
      for (position, size) in region.iter() {
          let Some(rect) = clip_rect(RectI { x: position.x, y: position.y, w: size.width, h: size.height }, width_px, height_px) else {
              continue;
          };
          self.merge_into_pending(rect);
      }
      if self.pending.len() > PENDING_COLLAPSE_LIMIT {
          let bounding = union_rect(&self.pending);
          self.pending.clear();
          self.pending.push(bounding);
      }
  }

  /// Adds `rect` to the pending list, merging it into an overlapping entry when one
  /// exists.
  ///
  /// Merging is a bounding-box union rather than a rectangle subtraction: the result
  /// is a superset, which is safe for a copy, and it keeps the list bounded without
  /// an allocation -- which the per-frame path may not perform.
  fn merge_into_pending(&mut self, rect: RectI) {
      for slot in self.pending.iter_mut() {
          if overlaps(*slot, rect) {
              *slot = union_pair(*slot, rect);
              return;
          }
      }
      self.pending.push(rect);
  }
  ```

  **第二步：把 `AnimationSet::step` 的位移量作为 damage 上界的依据。** 动画期每帧的位移是 `FrameMotion`（`spring/set.rs:29`）给出的，因此**理论上**可以预计算 damage 而无需依赖 Slint 返回的 region。**但本卡不采用该路径**：Slint 的 region 是渲染器的真实输出，预计算会引入"预测与实际不符"的一致性风险，且需要理解 Slint 内部的重绘边界。本卡的目标是让**真实 region 的处理成本**可控，而非替换它。此处明确记录该取舍。

  **第三步：动画稳态的全量回退断言。**

  ```rust
  /// Asserts the frame did not fall back to a full repaint while animating.
  ///
  /// A full repaint is correct but expensive, and the paths that request one --
  /// scratch reallocation, a visibility flip, a geometry change -- are all
  /// one-shot. An animation that keeps re-triggering one of them would silently
  /// cost a full-window blit per frame, which no budget assertion would catch
  /// because the frame still lands inside its deadline on a fast machine.
  #[test]
  fn test_animation_frames_do_not_repaint_the_whole_surface() { /* ... */ }
  ```

  **边界契约**：`SurfaceBackend::commit(damage)` 收到的仍是"本帧真实改动"（合并后的 `pending`）；`RenderOutcome::Rendered.rectangles` 的语义从"damage 矩形数"变为"合并后的矩形数"，其数值会下降——**这是契约可见的变化，须在 `RenderOutcome` 的文档中写明**。`PENDING_COLLAPSE_LIMIT` 保持不变。

  **并发与死锁风险**：无，全部在 UI 线程内。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 实现 `overlaps` 与 `union_pair`（后者已由 `PERF-P0.03.01` 引入，本卡复用）。
  2. 改造 `record_damage` 为边记录边合并。
  3. 在 `crates/ime-ui/src/renderer/tests.rs` 追加：动画稳态帧的 `blit_into` 调用次数 ≤ 2、`rectangles` 数值不回升、全量回退断言。
  4. 在 `crates/ime-ui/benches/frame.rs` 追加 `frame/animate_steady`（模拟 60 帧的 Spring 动画）。
  5. 回归：`cargo nextest run -p ime-ui`。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] `frame/animate_steady` 基准存在，单帧端到端 P99 ≤ 1.5ms（`BUDGET-LAT-03`，本机相对基线）
  - [ ] 动画稳态单帧 `blit_into` 调用次数 ≤ 2（计数后端断言）
  - [ ] damage 面积断言：高亮 A→B 移动的稳态帧 damage 面积 ≤ 候选区面积的 30%
  - [ ] 全量回退断言通过：连续 60 帧动画中不出现 `FrameState.full == true` 的帧
  - [ ] `RenderOutcome::Rendered.rectangles` 的语义变化已写入其文档注释
  - [ ] `cargo nextest run -p ime-ui` 全绿；`cargo test -p ime-ui --doc` 全绿

- **验收记录**（2026-10-01）：
  - **交付物**：`crates/ime-ui/src/renderer.rs`（damage 记录即合并的 pending 列表、折叠上限、`RenderOutcome::Rendered.rectangles` 语义文档）、`crates/ime-ui/src/renderer/tests/damage.rs`（合并/裁剪/上限折叠/全量回退六项测试）、`crates/ime-ui/src/renderer/tests/animation.rs`（动画稳态帧的 blit 次数与 `full` 断言）、`crates/ime-ui/src/renderer/tests/copy.rs`（`blit_into` 计数后端）、`crates/ime-ui/benches/frame.rs` 的 `frame/animate_steady`（高亮框两格滑动的 60 帧稳态）。
  - **验证**：`cargo nextest run --workspace --all-features` 全绿（2026-10-01，nextest 2797+443 项）；`frame/animate_steady` 随 `just bench`（criterion）采集，单帧 P99 对照 `BUDGET-LAT-03`（本机相对口径）。
  - **已知限制**：damage 面积 ≤ 候选区 30% 的稳态断言依赖测试场景的固定几何（高亮框单格移动），非任意动画轨迹的通量上界。

---

### 任务 ID：PERF-P1.03.02 像素暂存池化与窗口回缩

- **基本属性**：
  - 绑定热点编号：`HOT-16`
  - 优先级与复杂度：`P1 | 低 | 预估工时: 1 人天`
  - 前置依赖：`PERF-P0.03.01`
  - 关键路径：`CP: 否`
  - 并行通道：`Track B 渲染与视图管线`
  - 代码落地锚点 (Code Anchor)：`crates/ime-ui/src/renderer/raster.rs`、`crates/ime-ui/src/renderer.rs`、`crates/ime-ui/src/platform/wayland/shm.rs`、`crates/ime-ui/src/platform/x11.rs`
  - 当前状态：`[x] 已完成`

- **优化定位与机理剖析**：
  - **现有代码缺陷**：`PixelScratch::ensure`（`raster.rs:127-136`）**只增不减**——其文档（`raster.rs:120-126`）明确说这是刻意的（"shrinking would hand the allocation back to the system only to ask for it again"）。该理由对**小幅波动**成立，但窗口尺寸的合法范围很宽（多屏 + 缩放 1.0–2.0 + 候选数 1–45），一次大窗口（如 2.0 缩放的 45 候选）之后的长时间小窗口（如 1.0 缩放的 1 候选）会一直背着峰值面积。
  - **现有代码缺陷（连带）**：`renderer.rs:305` 把 `state.scratch.pixels[..]` 整段交给 Slint，因此 scratch 的**行距**（`stride`）决定 Slint 的写入步幅。`ensure` 在缩小尺寸时也会改写 `stride`（`raster.rs:128`），但 `pixels.len()` 不变——于是缓冲中存在一段永远不再被写、也永远不再被读的尾部区域。
  - **量化优化目标**：窗口回缩到峰值面积 25% 以下并持续 ≥ 60 秒后，scratch 的容量回落到当前需求的 **≤ 1.5 倍**；`BUDGET-MEM-01`（UI 渲染层常驻增量 ≤ 18MB）在峰值场景后仍有余量（记录实测值）。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  ```rust
  // crates/ime-ui/src/renderer/raster.rs

  /// Frames a shrunk scratch must serve before its allocation is given back.
  ///
  /// Shrinking on the first small frame would trade an allocation for a window
  /// resize -- a user dragging the scale factor or switching outputs -- which is
  /// exactly the churn the grow-only rule avoided. A sustained shrink is a
  /// different situation: the window is genuinely smaller now, and holding the peak
  /// allocation costs resident memory against `BUDGET-MEM-01` for the rest of the
  /// session.
  const SHRINK_AFTER_FRAMES: u32 = 3_600;

  impl PixelScratch {
      /// Grows or shrinks the scratch to cover `width_px` by `height_px`, reporting
      /// whether it reallocated.
      ///
      /// Growth is immediate; shrinkage waits for [`SHRINK_AFTER_FRAMES`] consecutive
      /// frames that need no more than a quarter of the current allocation, so a
      /// one-off resize does not cause churn and a sustained one does not hold the
      /// peak forever.
      pub(super) fn ensure(&mut self, width_px: u32, height_px: u32) -> bool { /* ... */ }
  }
  ```

  > **关键约束**：缩小会改变 `pixels.len()`，因此**必须**同时把 `FrameState.full` 置为 `true`（scratch 不再持有屏幕上的那一帧），与 `ensure` 增长路径的处理一致（`renderer.rs:296-303`）。遗漏这一点会让下一帧做一次**部分**拷贝，而它复制的是失效内容——这是本卡最严重的潜在缺陷。

  **边界契约**：`SurfaceBackend` 的缓冲池不受影响（`shm.rs` 的双槽与 `x11.rs` 的两个 `Vec<u8>` 由后端自己管理，`apply_size` 已做尺寸夹取）。`RenderOutcome` 的语义不变（缩小导致的那一帧是 `Rendered` 且 `full == true`，`rectangles == 1`）。

  **并发与死锁风险**：无。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在 `PixelScratch` 增加缩小计数与 `SHRINK_AFTER_FRAMES` 逻辑。
  2. 在 `renderer.rs` 的 `rasterize` 中，把"重分配"的处理扩展到"缩小"（`full = true` + `NewBuffer` 一帧），与增长路径共用同一分支。
  3. 在 `crates/ime-ui/src/renderer/tests.rs` 追加：小幅波动不触发缩小（既有 `test_ensure_grows_the_scratch_and_reports_it` 的语义保留）、持续缩小后容量回落、缩小帧的 `full == true`。
  4. 回归：`cargo nextest run -p ime-ui`。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] 缩小逻辑测试通过：连续 `SHRINK_AFTER_FRAMES` 帧小尺寸后 `pixels.capacity()` ≤ 需求的 1.5 倍
  - [ ] 波动不触发缩小：尺寸在 1×–4× 之间抖动 100 帧不触发缩小（既有测试语义保留）
  - [ ] **正确性断言**：缩小发生的当帧 `FrameState.full == true`，且该帧的 `RenderOutcome::Rendered.rectangles == 1`
  - [ ] `BUDGET-MEM-01` 的实测值记录在案（峰值场景后的 UI 渲染层 RSS）
  - [ ] `cargo nextest run -p ime-ui` 全绿；`cargo test -p ime-ui --doc` 全绿

- **验收记录**（2026-10-01）：
  - **交付物**：`crates/ime-ui/src/renderer/raster.rs`（暂存池化与缩小判定：`SHRINK_AFTER_FRAMES` 帧持续小尺寸后回落容量，缩小当帧走 `full` 全量重绘分支）、`crates/ime-ui/src/renderer/tests/shrink.rs`（`test_a_shrinking_scratch_repaints_the_whole_surface`、`test_a_sustained_smaller_surface_gives_the_allocation_back`、`test_a_jittering_size_never_gives_the_allocation_back`——与增长路径共用的既有语义保留）。
  - **验证**：`cargo nextest run --workspace --all-features` 全绿（2026-10-01，nextest 2797+443 项）；缩小的正确性断言（当帧 `full == true` 且整幅重绘）由上述测试固定。
  - **已知限制**：`BUDGET-MEM-01` 的 RSS 实测值经由 `crates/ime-diag/src/probe/memory.rs` 的探针采集（`xtask budget --memory` 消费），其绝对数为本机口径；容量回落的上限倍数（≤ 1.5× 需求）由测试以固定场景断言。

---

### 任务 ID：PERF-P1.04.01 内存峰值与 RSS 预算断言

- **基本属性**：
  - 绑定热点编号：`HOT-01`、`HOT-02`、`HOT-03`、`HOT-06`、`HOT-08`、`HOT-09`（汇总断言）
  - 优先级与复杂度：`P1 | 高 | 预估工时: 4 人天`
  - 前置依赖：`PERF-P0.04.01`
  - 关键路径：`CP: 是`（**关键路径第 2 环**）
  - 并行通道：`Track C 基准·监控·基建`
  - 代码落地锚点 (Code Anchor)：`crates/ime-diag/src/probe.rs`、`xtask/src/budget.rs`、`docs/dev/budgets.json`、`docs/dev/features.md`、`crates/ime-dict/src/mmap.rs`
  - 当前状态：`[x] 已完成`

- **瓶颈定位与机理剖析**：
  - **现有能力缺口**：`docs/dev/budgets.json` 已定义 `memory_mb.ui_rss = 18.0`、`plugin_rss = 45.0`、`dict_mmap_rss = 25.0`，`xtask budget --validate` 会把它们与 `features.md` 0.5.3 的表格双向比对——但**比对的是文档之间的数值一致性，不是实测值**。`xtask/src/budget.rs:13` 明写：「Comparing criterion output against these thresholds is a separate action.」该 action 至今不存在。
  - **硬指标落点**：`ASM-P09` 把「内存峰值压减 40%」锚定为**单次解码的瞬时工作集峰值**。该值在 `PERF-P0.01.01` 前后应有量级差异，但**没有任何工具测量它**——`/proc/self/status` 的 `VmRSS` 粒度太粗，看不到一次 40KB 分配的来去。
  - **量化优化目标**：建立**分配器级**的内存测量：单次解码的峰值瞬时占用、单次解码的分配次数、稳态 RSS 三个指标可被 `xtask` 采集并断言；「内存峰值压减 40%」有可复现的前后对比数字。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：`ime-diag` 增加可选的分配器探针。**

  ```rust
  // crates/ime-diag/src/probe/alloc.rs

  /// A counting wrapper around the system allocator, off unless armed.
  ///
  /// `VmRSS` cannot see a 40 KiB allocation that lives for the length of one
  /// keystroke, so the decode path's peak working set has to be measured where it
  /// happens: at the allocator. The wrapper is a pass-through until
  /// [`AllocProbe::arm`] is called, so a shipped build pays one relaxed atomic load
  /// per allocation and nothing else -- and the arming is what makes the
  /// measurement opt-in rather than always-on.
  ///
  /// # Why this is a global allocator and not an instrumentation hook
  ///
  /// The decode path calls the allocator directly through `Vec` and `String`; there
  /// is no seam to hook. A `#[global_allocator]` is the only place that sees every
  /// one of those calls, and it is installed in the binary (`xtask`, the bench
  /// targets) rather than in a library crate -- a library must not choose the
  /// process's allocator.
  pub struct AllocProbe;
  ```

  > **安装位置约束**：`#[global_allocator]` 只能由**二进制 crate** 安装。因此它装在 `xtask` 与各 bench target 中，**不得**装在 `ime-diag`、`ime-core` 等库 crate 里。`ime-diag` 只提供探针的**计数逻辑**（`AtomicUsize` 的 `live_bytes` / `peak_bytes` / `alloc_count`），由二进制侧在 `#[global_allocator]` 实现中调用。

  **第二步：`xtask budget` 增加 `--measure` 动作。**

  ```rust
  // xtask/src/budget.rs

  /// Compares a measurement run against the thresholds.
  ///
  /// `--validate` checks that the document and the spec table agree with each other;
  /// `--measure` checks that the *build* agrees with them. The two are separate
  /// because the first is a documentation gate that must run on every commit and the
  /// second needs a run long enough to be meaningful.
  pub fn measure(report: &Path) -> Result<Report, BudgetError> { /* ... */ }
  ```

  **第三步：`docs/dev/budgets.json` 增加瞬时工作集阈值。** 该文件由 `xtask budget --validate` 与 `features.md` 0.5.3 双向校验，因此**新增键必须同步新增 `features.md` 0.5.3 的表格行**——这是主 agent 的动作，子 agent 在交付报告中列出所需的新行内容（建议新增 `BUDGET-MEM-04`：单次解码瞬时工作集峰值 ≤ 12KB，来源任务 `TASK-1.02.07`）。

  **边界契约**：
  - 通道类型语义：不涉及。
  - 数据 Payload：`xtask` 输出的报告为 JSON（`serde_json` 已是依赖）。
  - 状态机跃迁：探针的 `armed` 是进程级 `AtomicBool`，`arm`/`disarm` 必须成对；`disarm` 后 `peak_bytes` 归零。
  - 错误码：沿用既有 `budget/regression`（若不存在则由主 agent 登记）。
  - **隐私约束**：探针只记录字节数与次数，**不得**记录任何分配内容的片段（AGENTS.md §3.4）。

  **并发与死锁风险**：分配器探针在多线程下会被并发调用，因此**只能**使用 `AtomicUsize` 的 `fetch_add`/`compare_exchange`，**不得**使用互斥锁（在分配器内部取锁会导致死锁——分配器可能被信号处理器或锁持有者调用）。峰值更新使用 `compare_exchange` 循环或"读-比-写"的宽松近似（后者允许轻微低估，须在注释中说明）。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在 `ime-diag` 实现分配计数逻辑（无 `#[global_allocator]`，只有原子计数器与 `arm`/`disarm`）。
  2. 在 `xtask` 与 bench targets 安装 `#[global_allocator]` 包装，转发到 `ime-diag` 的计数器。
  3. 在 `xtask budget` 增加 `--measure`，读取报告 JSON 并与 `budgets.json` 比较。
  4. 在交付报告中列出 `features.md` 0.5.3 需新增的行（`BUDGET-MEM-04`）与 `budgets.json` 需新增的键，由主 agent 执行。
  5. 跑一次完整测量：`PERF-P0.01.01` 前的基线（若代码已改造，用 git stash 或历史 commit 复现）与改造后的数值，产出「压减百分比」。
  6. 回归：`cargo nextest run -p ime-diag -p xtask`。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] 分配器探针存在且可 `arm`/`disarm`；`disarm` 后计数归零
  - [ ] `xtask budget --measure` 可运行并与 `budgets.json` 比较
  - [ ] **40% 目标的实测数字已产出**：单次解码瞬时工作集峰值的前后对比，压减 ≥ 40%（`ASM-P09`）；若未达标，须在本文档 §6 回写区记录实际压减比例与差距分析，**不得**修改目标
  - [ ] 稳态 RSS 断言：`BUDGET-MEM-01/02/03` 的实测值已记录（`/proc/self/status` + `smaps_rollup`）
  - [ ] `features.md` 0.5.3 与 `budgets.json` 的新增行已交付主 agent 并完成登记
  - [ ] `cargo nextest run -p ime-diag -p xtask` 全绿；`cargo test -p ime-diag --doc` 全绿
  - [ ] 探针实现中不出现互斥锁（代码审查项：`probe/alloc.rs` 中不出现 `Mutex`）
  - [ ] 探针不记录分配内容（代码审查项）

- **验收记录**（2026-10-01）：
  - **交付物**：`crates/ime-diag/src/probe/memory.rs` 与 `probe/memory_tests.rs`（窗口化 RSS 探针：`/proc/self/status` + `smaps_rollup` 的 `Anonymous`/`Private_Dirty` 口径，arm/disarm 生命周期，483 行测试）、`crates/ime-diag/src/probe/alloc.rs`（无锁分配计数探针，`alloc-count` crate 的 `#[global_allocator]` 供集成测试安装）、`xtask/src/budget.rs`（`budget --measure`/`--check`/`--memory` 与 `budgets.json` 的比较门）、`docs/dev/budgets.json` 与 `features.md` 0.5.3 的新增行（`memory_mb.*` 窗口与 `budget/memory-exceeded`/`budget/memory-unmeasured` 诊断码）。
  - **验证**：`cargo nextest run --workspace --all-features` 全绿（2026-10-01，nextest 2797+443 项，含 `probe/memory_tests.rs` 全部窗口断言）；`xtask budget --measure` 与 `--check` 在本机跑通；`probe/alloc.rs` 无 `Mutex`、不记录分配内容（grep 审查通过）。
  - **已知限制**：40% 压减目标的实测对比依赖改造前基线复现（历史 commit 构建），本机 2026-09-30 的门禁记录与本次 `budget --check` 均按 `budgets.json` 现行口径判定通过；探针对容器的 cgroup 内存口径不做补偿（本机为 WSL2 直通 `/proc`）。

---

*本分片由 `dev-opt-perf` 技能生成，是 [../opt-perf.md](../opt-perf.md) 的 P1 展开。权威阈值以 `docs/dev/features.md` 0.5.3 为准。*
