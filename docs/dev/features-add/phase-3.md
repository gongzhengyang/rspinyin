# rspinyin 增量功能扩充 · Phase 3（生态扩充与极限调优）

> 分片版本: v1.0 ｜ 主文档: [../features-add.md](../features-add.md) ｜
> 系统形态: Desktop GUI（Linux 桌面输入法） ｜ 架构基线: Rust 2024 + Slint 1.x + Fcitx5 5.1 ｜
> 关联 ADR: [../adr/0000-upstream-decisions.md](../adr/0000-upstream-decisions.md)、[../adr/0001-frozen-boundary-contracts.md](../adr/0001-frozen-boundary-contracts.md)、[../adr/0003-ui-role-separate-addon.md](../adr/0003-ui-role-separate-addon.md)、ADR-0005（待决策）｜
> 最后同步 Commit: `ee0dbfb` ｜
> 维护约定: 任务状态变更必须回写主文档 5.1 追溯表；假设变更必须回写主文档第 2 节

## 0. 分片基线（引用主文档，不重复定义）

- **任务卡字段规范**：主文档 7.2
- **不可违反的增量约束**：主文档 0.4（继承 `features.md` 0.4 的 11 条 + 追加 A-1~A-4）
- **独占性能预算**：主文档 0.3（P2 特性多数不在解码热路径；`P2.01.01` 的 bigram 是唯一显著占用者）
- **假设清单**：主文档第 2 节（强相关：`ASM-A-03` 的 A-3 不引入 copyleft 数据、`ASM-A-05` 词库 20MB 上限、`ASM-A-18` 不引入新线程）
- **边界契约增量**：主文档 4.2（`P2.01.01` 会触及 `DICT_FORMAT_VERSION`，见该卡；其余卡复用 `M0` 已冻结的类型）
- **验收标签**：`features.md` 0.2 的 `[自动]` / `[文档]` / `[实验室]` / `[视觉]` / `[性能]`

**分片范围**：12 张 P2 卡 = 9 张绑定主文档 5.1 的 `GAP-12/15/29/32/33/34/35/36/41` + 3 张承接 `features.md` 已排期项（`TASK-3.04.01`、`TASK-3.07.01`/`3.07.03`、`TASK-3.08.01`）。

**前置**：本分片**全部**卡片依赖主文档 5.0 的 `M0`，且多数依赖 P0/P1 的产物（见各卡的前置依赖）。

---

## 1. 任务卡

### 1.1 词库与语言模型深化（Track A-数据）

#### 任务 ID：ADD-FEAT-P2.01.01 bigram 长句语言模型

- **基本属性**：绑定差距条目：`GAP-12` ｜ `P2 生态扩展 | 高 | 10 人天` ｜ 前置 `ADD-FEAT-P0.01.01` ｜ `CP: 否` ｜ `Track A-数据` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-dict/src/format/mod.rs`（改：`SectionKind::Bigram` 的解禁）、`crates/ime-dict/src/format/reader.rs`（改：v2 头的接受）、`xtask/src/dictc/builders/bigram.rs`（新增）、`crates/ime-dict/src/bigram.rs`（新增）、`crates/ime-core/src/lm/bigram.rs`（新增）、`crates/ime-types/src/version.rs`（改：`DICT_FORMAT_VERSION` 1 → 2）、`data/raw/corpus.txt`（新增：训练语料）
- **目标与价值**：对标搜狗的云端+本地 n-gram 整句、微软拼音的神经网络整句、RIME 的 `grammar` 语言模型、libpinyin 的 bi-gram 整句。**契约与格式均已预留、零实现**：`LanguageModel::bigram(prev, word)` 已冻结但 `InMemoryLm` 恒返回 0；`SectionKind::Bigram` 与 `FLAG_BIGRAM_PRESENT` 的写入侧已实现，但 v1 格式**拒绝** bigram 段（有测试断言 `test_add_section_rejects_a_bigram_payload_in_version_one`）。
- **技术设计**：
  - **格式版本升级**：`DICT_FORMAT_VERSION` 1 → 2。**注意**：这与主文档 `ASM-A-19` 的"保持 1"矛盾——`ASM-A-19` 的前提是"简繁表走独立 `script.dict`"，而 bigram **无法**走独立文件（它必须与 `ENTRIES` 的 `word_id` 对齐）。因此本卡**必须**：
    1. 新开 ADR 说明为何 bigram 不能外置；
    2. `reader.rs` 接受 v1 与 v2 两种头（v1 无 bigram 段，`bigram()` 返回 0，与今天行为一致）；
    3. `SECTION_COUNT` 从 6 保持为 6（`Bigram` 段**已在 v1 的段表中占位**，只是长度恒为 0）——**这是关键的幸运之处**：v1 已经预留了段表槽位，v2 只是允许它有内容，**段表布局不变**。
  - **bigram 的存储**：`[(u32 prev_hash, u32 word_id, u16 prob_q12); n]`，按 `(prev_hash, word_id)` 排序。用 `prev_hash` 而非 `prev_word_id` 是为了让表可以按哈希排序后二分，避免为每个前词维护变长列表。
    ```rust
    /// One bigram entry: the conditional probability of `word_id` after the
    /// word whose hash is `prev_hash`.
    ///
    /// Keyed by hash rather than by `word_id` so the table is a single sorted
    /// array: a per-predecessor list would need an offset table and a second
    /// indirection, and the hash is already computed on the decode path.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct BigramEntry {
        pub prev_hash: u32,
        pub word_id: u32,
        /// `P(word | prev)` in Q12, matching `UNIGRAM`'s encoding.
        pub prob_q12: u16,
    }

    /// The largest number of bigram entries a dictionary may carry.
    ///
    /// 8 million entries is 96 MB at 12 bytes each, which is far past the
    /// `base_dict` budget. The bound exists to reject a malformed length
    /// field before it is used to size an allocation, not to describe a
    /// realistic table.
    pub const MAX_BIGRAM_ENTRIES: u32 = 8_000_000;
    ```
  - **训练管线**：`data/raw/corpus.txt`（**新词源，必须过白名单**）。语料来源必须宽松许可——候选：项目自建语料、或公开的 CC0/公有领域文本。**`ASM-A-03` 的 A-3 明确禁止引入 copyleft 语料。**
  - **打分接入**：`crates/ime-core/src/lm/bigram.rs` 实现 `LanguageModel::bigram` 的真实查表；`viterbi/decoder.rs` **已有** bigram 项的调用点（`decoder.rs:307` 的注释提到"the next edge's bigram term conditions on"），本卡只需让数据源不再恒返回 0。
  - **体积预算**：bigram 表计入 `base_dict` 的 20MB 上限（`BUDGET-SIZE-02`）。8 万条 bigram 约 1MB；若需要更大，必须重新分配分段预算。
- **NFR**：bigram 查表在解码热路径上，独占预算 ≤ 0.40ms（主文档 0.3 的 P1/P2 池）；`base_dict` 总量 ≤ 20MB；`decode_p99 ≤ 3.0ms`；v1 词库仍可读（向后兼容）。
- **实施步骤**：① **先开 ADR** 说明 bigram 不能外置与 v1→v2 的兼容策略；② `reader.rs` 接受两种头；③ 解除 v1 的 bigram 段拒绝（改 `test_add_section_rejects_a_bigram_payload_in_version_one` 为 v2 的接受测试）；④ 语料获取与白名单登记；⑤ 训练管线（`builders/bigram.rs`）；⑥ `BigramEntry` 的查表与 `LanguageModel::bigram` 的接入；⑦ 留出集质量度量（复用 `P0.01.02` 的 `QualityReport`）。
- **DoD**：① ADR 已写，说明 v1→v2 的兼容策略。[文档] ② v1 词库仍可读且 `bigram()` 返回 0（既有行为不变）。[自动] ③ `data/compiled/base.dict` 为 v2 且 `total_len ≤ 20MB`。[自动] ④ `lm_holdout.tsv` 的 `top1_rate` 相比 bigram 前**有提升**，数值记入验收记录。[性能] ⑤ bigram 查表 ≤ 0.40ms，`decode_p99 ≤ 3.0ms`。[性能] ⑥ 语料来源已登记白名单且 `permissive = true`。[自动]

---

#### 任务 ID：ADD-FEAT-P2.01.02 生僻字与 CJK 扩展区覆盖 + 领域词库

- **基本属性**：绑定差距条目：`GAP-15` ｜ `P2 生态扩展 | 中 | 6 人天` ｜ 前置 `ADD-FEAT-P0.01.01` ｜ `CP: 否` ｜ `Track A-数据` ｜ `[ ] 待开始`
- **代码落地锚点**：`data/raw/domain/`（新增：领域词表目录）、`xtask/src/dictc/source/domain.rs`（新增）、`data/sources.toml`（改：登记领域词源）、`crates/ime-dict/src/format/mod.rs`（只读：`WordFlags::TERM` 的首次使用）、`docs/dev/dict-sources.md`（新增）
- **目标与价值**：对标搜狗的"生僻字模式"与专业词库、微软拼音的生僻字。两个子目标：
  1. **生僻字覆盖**：L1 单字表有 44,435 字（`pinyin-data`）+ Unihan 44,355 行交叉校验，但**词库仅 5,441 条**（`P0.01.01` 扩到 32 万后仍是通用词），扩展区字符（CJK Ext-B/C/D）**几乎无词组覆盖**。格式上 `MAX_WORD_LEN = 96` 字节（Ext-B 字符 4 字节 → 24 字）无碍。
  2. **领域词库**：`features.md` 的 `TASK-3.02.02` 排了 THUOCL（**许可待核实**）。`ASM-A-03` 的 A-3 明确：**未确认前不得引入**。
- **技术设计**：
  - **`WordFlags::TERM` 的首次使用**：`WordFlags` 的四个变体（`SURNAME`/`PLACE`/`TERM`/`USER`）**已冻结但 `TERM` 从未被任何词源使用**。领域词库的每个词条标 `TERM`，用于：(1) 诊断中区分词条来源；(2) 可选的"只在特定 profile 下启用领域词"（复用 `P1.02.07` 的 per-app profile）。
  - **领域词库的组织**：`data/raw/domain/<name>.tsv`（`词<TAB>读音<TAB>权重`），每个领域一个文件；`data/sources.toml` 逐文件登记。
  - **许可纪律（硬约束）**：`THUOCL` **必须**先核实许可并写入 `data/sources.toml` 的 `license`/`spdx`/`permissive` 三个字段，**否则不得引入**。若许可无法核实，替代路径是**项目自建**领域词表（从公开的、宽松许可的语料统计生成），或**不提供**领域词库——**"没有领域词库"好过"许可不清的领域词库"**（ADR-0000 的不可逆性说明）。
  - **生僻字的补充路径**：Unihan 的 `kMandarin` / `kHanyuPinyin` / `kXHC1983` 三个字段对扩展区字符有读音覆盖。从 `data/raw/unihan.tsv`（已在白名单）补齐 L1 表中缺失的扩展区单字读音——**零新增许可负担**。
  - **无笔画/手写输入**：搜狗的生僻字模式含笔画输入。本卡**不做**——笔画输入是独立的手写/字形引擎，与拼音解码器的架构不兼容（需要字形数据库与笔画序列匹配），且 `features.md` 未规划。**如实登记为不做**，并在 `docs/dev/dict-sources.md` 中说明"生僻字只做读音覆盖，不做笔画检索"。
- **NFR**：领域词库的体积计入 `base_dict` 的 20MB 上限；生僻字补充**不得**让 L1 表与 Unihan 的交叉校验失败（既有校验逻辑必须仍通过）；每个领域词源**必须**有 `sha256`（`kind = "upstream"`）或登记为 `derived`。
- **实施步骤**：① **先核实 THUOCL 的许可**；无法核实则改走项目自建路径；② `data/raw/domain/` 的组织与 `sources.toml` 登记；③ `builders/domain.rs`；④ 从 Unihan 补齐扩展区单字读音（`data/raw/pinyin-data.tsv` 的补充）；⑤ `WordFlags::TERM` 的标注；⑥ `docs/dev/dict-sources.md` 的逐源说明与"不做笔画"的声明。
- **DoD**：① `data/raw/domain/` 的每个词源在 `sources.toml` 中登记且 `permissive = true`。[自动] ② **若 THUOCL 许可未核实，则它未被引入**（`grep -i thuocl data/` 无命中，或命中处有许可核实记录）。[文档] ③ 扩展区单字的读音覆盖率相比本卡前**有提升**，数值记入验收记录。[性能] ④ `WordFlags::TERM` 被领域词条使用（`test_domain_words_carry_term_flag`）。[自动] ⑤ `docs/dev/dict-sources.md` 含"不做笔画检索"的明确声明。[文档] ⑥ `base_dict ≤ 20MB`。[性能]

---

### 1.2 渲染与主题（Track B）

#### 任务 ID：ADD-FEAT-P2.02.01 GPU 渲染路径评估与实现

- **基本属性**：绑定差距条目：`features.md` 的 `TASK-3.04.01`（**不在主文档 5.1 的 44 行绑定范围内**）｜ `P2 生态扩展 | 高 | 8 人天` ｜ 前置 `ADD-FEAT-P1.03.01` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-ui/src/renderer/gpu.rs`（新增）、`crates/ime-ui/src/renderer.rs`（改：后端选择）、`crates/ime-ui/src/slint_platform.rs`（改：Slint 的渲染后端切换）、`docs/dev/spikes/gpu-render.md`（新增：评估结论）
- **目标与价值**：`features.md` 0.1 把 GPU 渲染列为**首版非目标**并排入 `TASK-3.04.01`，理由是"首版用 `wl_shm`/MIT-SHM 软件光栅即可满足 1.5ms 预算，且规避驱动差异"。**本卡先评估后实现**：若软件光栅在 `R-05` 的最坏情况（720px 宽 × 5 行 × scale 2.0 = 1440×560 物理像素，含 28px 模糊阴影）下确实超 `BUDGET-LAT-03 = 1.5ms`，才实现 GPU 路径。
- **技术设计**：
  - **评估优先，实现其次**：本卡的**第一个交付物是 `docs/dev/spikes/gpu-render.md`**，给出在真实硬件上的软件光栅实测值。**若实测达标，本卡以"评估结论：不需要 GPU 路径"结项**，实现部分不做——这是合法的完成状态（`features.md` 6.3 的 Phase 3 出口准则 #4 要求评估类任务"产出明确的 Go/No-Go 结论文档"）。
  - **Slint 的后端**：Slint 支持 `femtovg`（OpenGL ES）/ `skia` / `software` 三种渲染器。切到 GPU 只需改 `slint_platform.rs` 的 `Renderer` 选择——**但候选框走的是自定义 `Platform`**，因此需要实现 Slint 的 GPU 渲染器接口。
  - **关键的架构约束**：GPU 路径**不得**改变 `SurfaceBackend` 的契约（`ADR-0001` 冻结）。缓冲管理从 `wl_shm` 变为 EGL/GBM buffer，但 `trait SurfaceBackend` 的 `acquire_buffer` / `present` 语义不变。
  - **降级路径必须保留**：软件光栅是**默认**，GPU 是**可选**。驱动不兼容、EGL 初始化失败、`LIBGL_ALWAYS_SOFTWARE=1` 等场景必须回退到软件光栅并记一条 `info` 级诊断。**不得**因为 GPU 初始化失败而让候选框不出现。
- **NFR**：若实现，`raster_p99 ≤ 1.5ms` 且在 `R-05` 的最坏情况下达标；GPU 路径的常驻内存增量 ≤ 8MB（计入 `ui_rss = 18MB` —— **这可能是 GPU 路径的主要代价**，必须实测）；**软件光栅路径必须完整保留**（不能删）；GPU 不可用时静默降级。
- **实施步骤**：① 在真实硬件上实测软件光栅的 `R-05` 最坏情况耗时；② 产出 `docs/dev/spikes/gpu-render.md` 的 Go/No-Go 结论；③ **若 No-Go：本卡结项**；④ 若 Go：实现 `renderer/gpu.rs`，保持 `SurfaceBackend` 契约不变；⑤ 降级路径与诊断。
- **DoD**：① `docs/dev/spikes/gpu-render.md` 存在，含真实硬件上的软件光栅实测值与 Go/No-Go 结论。[性能] ② 若结论为 No-Go，本卡以评估结项，`raster_p99 ≤ 1.5ms` 的实测数据记入验收记录。[性能] ③ 若实现 GPU：`raster_p99 ≤ 1.5ms` 在 `R-05` 最坏情况下达标。[性能] ④ 若实现 GPU：软件光栅路径仍完整可用（可通过配置强制）。[自动] ⑤ 若实现 GPU：EGL 初始化失败时静默回退到软件光栅，候选框仍出现。[实验室]

---

#### 任务 ID：ADD-FEAT-P2.03.02 皮肤 / 主题包导入导出

- **基本属性**：绑定差距条目：`GAP-35` ｜ `P2 生态扩展 | 中 | 5 人天` ｜ 前置 `ADD-FEAT-P1.03.01` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-ui/src/theme/pack.rs`（新增）、`crates/ime-ui/src/theme/scheme.rs`（改：从主题包加载）、`crates/ime-config/src/schema.rs`（改：`[theme] pack`）、`docs/dev/theme-format.md`（新增）
- **目标与价值**：对标搜狗的皮肤体系、RIME 的 `preset_color_schemes`。`features.md` 排了 `TASK-3.05.02`（皮肤市场，需网络），但**用户自定义皮肤包**（离线、本地文件）与"市场"是两件事——前者不需要网络，可以现在就做。
- **技术设计**：
  - **主题包是一个 TOML 文件**（不是二进制、不是压缩包——TOML 用户可读可改，符合本项目的"用户可检查一切"取向）：
    ```toml
    # rspinyin-theme-<name>.toml
    schema_version = 1
    name = "Nord"
    author = "…"
    licence = "MIT"

    [light]
    surface_base    = "#FFFFFF"
    surface_stroke  = "#0F000000"
    text_primary    = "#1C1C1E"
    text_secondary  = "#9A1C1C1E"
    text_annotation = "#731C1C1E"
    text_separator  = "#591C1C1E"
    accent          = "#0A6CFF"
    accent_on       = "#FFFFFF"
    state_hover     = "#0F1C1C1E"
    state_pressed   = "#1F1C1C1E"
    state_disabled  = "#611C1C1E"
    separator       = "#141C1C1E"
    shadow_inner    = "#14000000"
    shadow_outer    = "#29000000"

    [dark]
    # …same keys
    ```
  - **对比度自检是导入的强制关卡**（复用既有的 `contrast_ratio`）：主题包若使 `text_primary` 对 `surface_base` 的对比度低于 `CONTRAST_MINIMUM = 4.5`，**拒绝导入并说明原因**。这不是"可选的警告"——一个对比度不足的候选框是**不可用**的，而用户导入时不知道自己在做什么。
    ```rust
    /// Why a theme pack was rejected.
    #[derive(Clone, Debug, PartialEq)]
    pub enum PackRejection {
        /// The file is not a theme pack.
        NotAPack,
        /// The schema version is newer than this build understands.
        FutureVersion { found: u16 },
        /// A required token is missing.
        MissingToken { token: &'static str },
        /// A value is not a colour this build can parse.
        BadColour { token: &'static str, value: String },
        /// The pack would produce an unreadable candidate window.
        ContrastTooLow { token: &'static str, ratio: f32, minimum: f32 },
    }
    ```
  - **导出**：把当前生效的 token 集合导出为同一格式的主题包，用户可分享或修改。
  - **导入的主题包存在用户配置目录**：`$XDG_CONFIG_HOME/rspinyin/themes/<name>.toml`（`0600`），`[theme] pack = "nord"` 启用。
- **NFR**：对比度自检**必须**拒绝低对比主题（不是警告）；主题包**不得**包含可执行内容（纯数据）；主题包**不得**改变几何度量（尺寸由 `P1.03.01` 的 `CandidateMetrics` 控制，主题只管颜色——**这是刻意的分工**，避免"换个皮肤候选框就点不准"）；主题包加载失败时回退到内置主题并记诊断。
- **实施步骤**：① 定义主题包的 TOML 格式与 `docs/dev/theme-format.md`；② `PackRejection` 与校验（含对比度）；③ 从主题包加载 token；④ 导出当前 token；⑤ 内置主题的迁移（把 `scheme.rs` 的常量表达为同一个结构，**保证内置主题与主题包走同一条代码路径**）。
- **DoD**：① 导入一个合法的 Nord 主题包后候选框配色变化。[实验室] ② 导入一个对比度不足的主题包被**拒绝**并说明原因。[自动] ③ 导出后再导入，token 逐项一致。[自动] ④ 内置主题与主题包走同一路径（`test_builtin_theme_uses_pack_loader`）。[自动] ⑤ 主题包不影响几何度量（`CandidateMetrics` 不变）。[自动]

---

#### 任务 ID：ADD-FEAT-P2.03.03 候选框 UI 文案 i18n

- **基本属性**：绑定差距条目：`GAP-33` ｜ `P2 生态扩展 | 中 | 4 人天` ｜ 前置 `ADD-FEAT-P1.02.01` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-ui/src/i18n.rs`（新增）、`crates/ime-ui/i18n/{en,zh-CN,zh-TW}.toml`（新增）、`crates/ime-config/src/schema.rs`（改：`[ui] language`）、`crates/ime-core/src/state/machine.rs`（改：`StatusStrip.mode_label` 的填充）
- **目标与价值**：对标 VS Code 的多语言包、RIME 的方案自带文案。当前候选框内的所有文案硬编码中文，`grep -ri i18n\|locale crates/` **零命中**。**非中文用户在英文模式下仍看到中文提示**——这不只是美观问题，而是"这个软件不是给我用的"的信号。
- **技术设计**：
  - **语言选择**：默认跟随 `LANG` / `LC_MESSAGES`；可被 `[ui] language` 覆盖（`auto` / `en` / `zh-CN` / `zh-TW`）。
  - **需要本地化的文案清单**（**逐条，无省略**）：
    | 位置 | 中文 | 说明 |
    |---|---|---|
    | `StatusStrip.mode_label` | 中 / EN | 中英模式 |
    | header 的简繁指示 | 简 / 繁 | 简繁状态 |
    | header 的全角指示 | 全 / 半 | 全角状态 |
    | header 的只读指示 | 只读 | `StatusStrip.readonly` |
    | 候选来源标注 | 短语 / 符号 / 用户 / 学习 | `CandidateSource` 的标签 |
    | 命令面板的 9 条命令 | 切换简繁 / 切换全角 / … | `COMMANDS` 的 `label` |
    | 设置面板的分组名 | 外观 / 输入 / 数据 / 诊断 | 设置分组 |
    | 符号面板的分类名 | 标点 / 数学 / 箭头 / 图形 / 表情 | `CATEGORIES` 的 `label` |
    | 状态指示器的 tooltip | 中文 / 英文 / 全角 / 只读 | 指示器 |
    | 诊断消息 | `ime-doctor` 的 `summary` 与 `remedy` | 18 项检查 |
  - **`StatusStrip.mode_label` 的处理**：它是 `String`（`ADR-0001` 冻结），由引擎填。i18n 后仍由引擎填，但值从语言表取——**不改变契约**。
  - **`COMMANDS` 与 `CATEGORIES` 的 `label` 字段**：它们是 `&'static str` 常量。i18n 后改为**键**（`&'static str` 的 key），由 `i18n.rs` 在运行时解析为语言表的值。**这改变了 `CommandSpec.label` 的语义**（从"显示文本"变为"文案键"）——必须在文档中写明，并在实现时把常量值改为键名。
  - **不引入 gettext / fluent 等 i18n 框架**：文案量小（约 40 条），一个 TOML 表 + `HashMap<&str, &str>` 足够。引入框架会带来构建期复杂度（`msgfmt` 工具链）与依赖，**违反 `AGENTS.md` 3.5 的"不重复造轮子"的反面——这里是"轮子比车还大"**。
- **NFR**：语言表在编译期嵌入（`include_str!`），**不在运行期读文件**（避免文件缺失导致文案空白）；语言切换**不重置**进行中的会话（0.4 规则 10）；缺失的键**回退到中文**并记 `warn`（不是空白）；语言表 ≤ 8KB/语言。
- **实施步骤**：① 抽出全部待本地化文案为键（逐条对照上表）；② 三份语言表 TOML（`en` / `zh-CN` / `zh-TW`）；③ `i18n.rs` 的加载与回退；④ `LANG` 探测与 `[ui] language` 配置；⑤ `COMMANDS`/`CATEGORIES` 的 `label` 语义变更与全部消费点同步。
- **DoD**：① `LANG=en_US.UTF-8` 启动时，候选框、命令面板、设置面板、符号面板、`ime-doctor` 的文案全部为英文。[实验室] ② 缺失键回退到中文并记 `warn`。[自动] ③ 语言切换不重置进行中的会话。[自动] ④ **无新增 i18n 框架依赖**（`Cargo.lock` 无 `fluent`/`gettext`/`unic-langid`）。[自动] ⑤ 三份语言表的键集合一致（`test_language_tables_have_identical_keys`）。[自动]

---

### 1.3 数据组织与生态（Track B / Track C）

#### 任务 ID：ADD-FEAT-P2.03.01 插件 / 脚本扩展系统决策（**明确不做**）

- **基本属性**：绑定差距条目：`GAP-34` ｜ `P2 生态扩展 | 低 | 2 人天` ｜ 前置：无 ｜ `CP: 否` ｜ 通道：`—`（决策文档，无代码）｜ `[ ] 待开始`
- **代码落地锚点**：`docs/dev/adr/0005-no-plugin-system.md`（新增）、`docs/dev/features.md` 0.1 的非目标表（改：登记本决策）
- **目标与价值**：**把"不做"变成一个显式的、有理由的决策，而不是一个遗漏。** 对标 RIME 的 `librime` 插件与 Lua 脚本、Obsidian 的插件市场、Raycast 的扩展商店、VS Code 的扩展市场。**本卡的结论是"不做"，理由必须写下来**，否则每个新会话都会重新提出这个问题。
- **技术设计**：**不做插件的三条理由**（逐条，需在 ADR 中展开）：
  1. **同进程 = 任意代码执行面。** 输入法插件与宿主 fcitx5 **同进程**运行（`describe.md` 的架构前提）。任何脚本引擎（Lua、WASM、JS）都意味着**第三方代码运行在用户输入路径上**——它能看到用户的每一次击键。这与项目的第 6 条产品目标（"绝对隐私：v1 零网络外联、零遥测、零云同步"）在**信任模型**上直接冲突：我们不能一边承诺"不记录你的输入"，一边提供一个能记录你输入的扩展点。
  2. **破坏"所有数据来源可审计"的不变量。** ADR-0000 的词源体系建立在"`data/sources.toml` 白名单 + `check-dict-sources.sh` 强制"之上。插件可以加载任意词表，绕过白名单——**而 copyleft 数据一旦混入是不可逆的**（ADR-0000 的不可逆性说明）。
  3. **维护成本与安全责任的转移。** 一旦开放扩展点，本项目就对"第三方插件的安全性"负有说明责任，而这是个小团队无法承担的。
  - **替代路径**（必须给出，否则"不做"等于"没有"）：
    | 用户想要 | 本项目提供的替代 |
    |---|---|
    | 自定义词表 | `ADD-FEAT-P1.01.06` 的用户自建词表导入 |
    | 自定义短语 | `ADD-FEAT-P0.01.03` 的自定义短语 |
    | 自定义键位 | `ADD-FEAT-P1.02.04` 的全量键位自定义 |
    | 自定义主题 | `ADD-FEAT-P2.03.02` 的皮肤包导入导出 |
    | 外部程序控制 | `ADD-FEAT-P1.04.03` 的 D-Bus 只读接口 |
    | 新输入方案 | `ADD-FEAT-P0.02.01`/`P0.02.02` 的双拼方案（含 `[scheme.custom]` 自定义方案） |
- **NFR**：无代码，无性能影响。ADR 必须**逐条给出理由与替代路径**，不得只写"暂不支持"。
- **实施步骤**：① 写 `docs/dev/adr/0005-no-plugin-system.md`；② 三条理由逐条展开，附与本项目产品目标的冲突分析；③ 替代路径表逐行给出对应的 `ADD-FEAT` 编号；④ 在 `features.md` 0.1 的非目标表中登记。
- **DoD**：① `docs/dev/adr/0005-no-plugin-system.md` 存在，含三条理由与替代路径表。[文档] ② 替代路径表的每一行指向一个真实的 `ADD-FEAT` 编号。[文档] ③ `features.md` 0.1 的非目标表已登记。[文档]

---

#### 任务 ID：ADD-FEAT-P2.04.01 多用户 / 多 Profile 并行会话

- **基本属性**：绑定差距条目：`GAP-36` ｜ `P2 生态扩展 | 中 | 5 人天` ｜ 前置 `ADD-FEAT-P0.03.02` ｜ `CP: 否` ｜ `Track C` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-dict/src/paths.rs`（改：profile 化的 XDG 路径）、`crates/ime-config/src/schema.rs`（改：`[profile]` 段的扩展）、`crates/ime-fcitx5/src/addon.rs`（改：按 profile 加载用户库）、`docs/dev/profiles.md`（新增）
- **目标与价值**：`features.md` 的 `TASK-2.03.02`。**注意与 `P1.02.07`（per-app profile）的区别**：`P1.02.07` 是**同一个用户**在不同应用下用不同配置；本卡是**同一个系统**下的不同**使用场景/身份**各有独立的配置与用户词库。典型用例：工作与个人两套词库（工作词库不学私人词汇）、共享机器上的多用户、以及"一个干净的词库用来测试"。
- **技术设计**：
  - **profile 是一个目录**，不是配置里的一个键：
    ```
    $XDG_DATA_HOME/rspinyin/
    ├── default/            # 默认 profile
    │   ├── user.redb
    │   └── backups/
    ├── work/
    │   ├── user.redb
    │   └── backups/
    └── test/
        └── user.redb
    ```
  - **profile 的切换方式**：`rspinyin --profile work`（CLI）+ 命令面板的切换项 + 环境变量 `RSPINYIN_PROFILE`。**`fcitx5` 是单例进程**，因此 profile 切换需要重启插件——**这是本卡的主要限制**，必须如实说明。
  - **配置的 profile 化**：`config.toml` 可全局共享，也可每 profile 一份（`<profile>/config.toml` 存在时优先）。
  - **迁移**：既有用户的 `user.redb` 位于 `$XDG_DATA_HOME/rspinyin/` 根下（无 profile 子目录）。首次启动新版时**自动迁移**到 `default/`（复用 `P0.03.02` 的迁移框架的同族做法：移动前先备份）。
- **NFR**：profile 切换需重启插件（**如实记录，不承诺热切换**）；迁移必须**先备份再移动**（`user.redb` 的丢失代价是全部学习成果）；每个 profile 的 `user.redb` 独立遵守 `USER_WORD_CAP`；**不得**在日志中记录 profile 名以外的用户数据。
- **实施步骤**：① profile 化的路径解析（`paths.rs`）；② 既有数据的自动迁移（先备份）；③ `--profile` / `RSPINYIN_PROFILE` / 面板切换；④ 每 profile 的独立用户库；⑤ `docs/dev/profiles.md` 说明限制（需重启）。
- **DoD**：① 两个 profile 的用户词库互相隔离（在一个里学的词不出现在另一个）。[自动] ② 既有用户的 `user.redb` 自动迁移到 `default/` 且**迁移前有备份**。[自动] ③ `--profile work` 启动后加载对应 profile。[实验室] ④ `docs/dev/profiles.md` 如实说明"切换需重启"。[文档]

---

#### 任务 ID：ADD-FEAT-P2.04.02 剪贴板历史

- **基本属性**：绑定差距条目：`GAP-32` ｜ `P2 生态扩展 | 中 | 4 人天` ｜ 前置 `ADD-FEAT-P1.01.04` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-ui/src/ui_thread/clipboard.rs`（新增）、`crates/ime-ui/ui/clipboard_panel.slint`（新增）、`crates/ime-fcitx5/src/clipboard/mod.rs`（新增）、`crates/ime-config/src/schema.rs`（改：`[clipboard]` 段）
- **目标与价值**：`features.md` 的 `TASK-2.04.03`。对标搜狗的剪贴板历史（`Ctrl+;`）。**本卡需要一次隐私评审**：剪贴板内容可能含密码、令牌、私密文本，而输入法读剪贴板是**权限敏感的**。
- **技术设计**：
  - **隐私设计（本卡的核心，必须先于功能设计）**：
    | 决策 | 取值 | 理由 |
    |---|---|---|
    | 默认开关 | **关闭** | 剪贴板历史是侵入性的，不该默认开启 |
    | 敏感内容过滤 | **开启且不可关闭** | 匹配常见密钥/令牌模式（`ghp_`、`sk-`、`AKIA`、长 base64）的条目**不进入历史** |
    | 密码管理器来源 | **跳过** | 若剪贴板所有者是已知的密码管理器（按 `AppIdHash` 匹配），跳过 |
    | 存储 | **仅内存，不落盘** | 剪贴板历史落盘 = 一个明文的"用户复制过的一切"文件。**内存上限 32 条，退出即失** |
    | 过期 | 5 分钟 | 超时的条目自动从历史移除 |
    | 最大条目 | 32 | 内存占用与可浏览性的折中 |
  - **读取方式**：X11 的 `CLIPBOARD` selection / Wayland 的 `wl_data_device`。**注意**：`features.md` 0.4 规则 5 要求候选框**绝不夺焦**——读剪贴板**不需要焦点**，但需要处理 selection owner 的所有权协商，这是本卡的技术难点。
  - **面板**：复用 `P1.01.04` 的符号面板机制（同族 surface、`C-1`）。
- **NFR**：**仅内存，绝不落盘**（这是硬约束——落盘会让"退出即失"的承诺失效）；敏感模式过滤**不可关闭**；剪贴板读取**不阻塞**宿主线程；**绝不夺焦**；`idle_poll_timer_count = 0`（不轮询剪贴板——只在面板打开时读取）。
- **实施步骤**：① **先做隐私评审**并把上表写入 `docs/dev/privacy.md`；② 敏感模式过滤器；③ X11 / Wayland 的剪贴板读取（只在面板打开时）；④ `clipboard_panel.slint`（复用符号面板的机制）；⑤ 内存上限、过期、退出清空。
- **DoD**：① 复制一段文本后，面板内可见该条目。[实验室] ② 复制一个形如 `ghp_…` 的令牌后，**面板内不可见**。[自动] ③ 剪贴板历史**不落盘**（`strace` 或等效手段验证无文件写入）。[自动] ④ 退出后历史清空。[自动] ⑤ 面板不夺焦。[实验室] ⑥ `privacy.md` 含剪贴板历史的隐私设计表。[文档]

---

#### 任务 ID：ADD-FEAT-P2.04.03 输入统计与自学习透明度

- **基本属性**：绑定差距条目：`GAP-29` ｜ `P2 生态扩展 | 低 | 3 人天` ｜ 前置 `ADD-FEAT-P0.01.04` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-dict/src/user_db/stats.rs`（新增）、`crates/ime-ui/src/command/user_words.rs`（改：统计页）、`docs/dev/privacy.md`（改：统计的隐私含义）
- **目标与价值**：对标搜狗的输入统计、微信输入法的打字统计。**但本卡的定位与商业输入法不同**：它们的统计是"炫耀你的打字速度"，本项目的统计是**自学习透明度**——让用户看到"输入法学了什么、学了多少、哪些词是我固定的"。这是隐私承诺的一部分：**如果用户能看到，就说明数据确实只在本地**。
- **技术设计**：
  - **统计口径（全部是聚合量，不含内容）**：
    | 指标 | 来源 | 说明 |
    |---|---|---|
    | 学习词条数 | `UserDb::record_count()` | 已有方法，**首次有消费者** |
    | 固定词条数 | `pinned` 的计数 | |
    | 用户词表词条数 | `WordFlags::USER` 的计数 | |
    | 库文件大小 | 文件元数据 | |
    | 最近学习时间 | `max(created_unix)` | |
    | 最常用的 10 个词 | 按权重排序 | **这是唯一显示内容的指标** |
  - **"最常用的 10 个词"是隐私权衡点**：显示它意味着界面上出现用户输入过的词。**决策：显示，但需用户显式点开**（默认折叠），且**不进入日志**。理由是"管理词库"界面本来就要显示词条（`P1.02.05`），统计页不显示反而不一致。
  - **打字速度统计不做**：需要记录每次按键的时间戳，那是在输入路径上采集行为数据——**与"不记录用户输入"的承诺的边界太近**。如实登记为不做。
- **NFR**：统计计算 ≤ 100ms（在面板打开时算，不在输入路径）；**不进入日志**；打字速度统计**不做**（登记理由）；统计页的"最常用词"默认折叠。
- **实施步骤**：① `stats.rs` 的聚合查询；② 统计页 UI（表格 + 默认折叠的最常用词）；③ `privacy.md` 的统计隐私含义说明；④ "打字速度统计不做"的登记。
- **DoD**：① 统计页显示 6 个指标。[实验室] ② 统计数字与实际库内容一致。[自动] ③ "最常用词"默认折叠。[视觉] ④ 统计不进入日志。[自动] ⑤ `privacy.md` 含统计的隐私含义与"不做打字速度"的说明。[文档]

---

### 1.4 交付与运维（Track C）

#### 任务 ID：ADD-FEAT-P2.05.01 发布流水线与跨发行版 CI 矩阵

- **基本属性**：绑定差距条目：`features.md` 的 `TASK-3.07.01` / `TASK-3.07.03`（**不在主文档 5.1 的 44 行绑定范围内**）｜ `P2 生态扩展 | 高 | 6 人天` ｜ 前置 `ADD-FEAT-P1.05.06` ｜ `CP: 否` ｜ `Track C` ｜ `[ ] 待开始`
- **代码落地锚点**：`.github/workflows/release.yml`（新增）、`.github/workflows/ci.yml`（改：跨发行版矩阵）、`scripts/release.sh`（新增）、`docs/dev/release.md`（新增）
- **目标与价值**：`features.md` 6.3 的 Phase 3 出口准则 #2、#3。当前只有 `.github/workflows/ci.yml`（单发行版）与 `packaging/install.sh`。**没有发布流水线就没有可复现的发布**。
- **技术设计**：
  - **跨发行版矩阵**：Ubuntu 22.04 / 24.04、Fedora 40 / 41、Arch（容器）。每个组合跑：`cargo check -p ime-fcitx5 --features fcitx5-host`（宿主 ABI 门禁）+ 打包 + 安装/卸载可逆性验证。
  - **发布产物**：deb / rpm / AUR 三套包 + `SHA256SUMS` + 源码归档。
  - **签名的现实约束**：`features.md` 6.3 的出口准则 #3 要求"带签名的包与 `SHA256SUMS`"。**GPG 签名需要一个长期密钥**——本卡必须明确密钥的持有与轮换方式，且**密钥绝不入库**（`AGENTS.md` 第 8 节禁止硬编码凭据）。签名在 CI 中用 GitHub Secrets 注入的密钥完成。
  - **`nm -D` 校验在发布流水线中强制**（复用 `P1.05.06` 的检查）：发布前的最后一道关卡。
  - **可复现构建**：`Cargo.lock` 已入库，`--locked` 构建；记录构建环境的工具链版本（`rust-toolchain.toml` 已固定 `1.98.0`）。
- **NFR**：CI 全矩阵 ≤ 30 分钟；**签名密钥不入库**（`AGENTS.md` 第 8 节）；每个产物附 `SHA256SUMS`；发布前 `nm -D` 校验强制；**无网络能力**（打包脚本不下载，`base.dict` 从仓库构建——**注意**：`data/fetch.sh` 会下载词源，但那是**构建期**的一次性动作，产物中不含网络能力）。
- **实施步骤**：① 跨发行版 CI 矩阵；② `release.yml` 的构建、打包、签名、产物上传；③ `SHA256SUMS` 生成；④ `nm -D` 校验接入；⑤ `docs/dev/release.md` 的发布流程与密钥管理说明。
- **DoD**：① CI 矩阵在 5 个发行版组合上全绿。[自动] ② 发布产物含 deb / rpm / AUR 三套包 + `SHA256SUMS`。[自动] ③ **签名密钥不入库**（`grep` 复核仓库无密钥材料）。[自动] ④ 发布前的 `nm -D` 校验强制生效。[自动] ⑤ `docs/dev/release.md` 说明密钥持有与轮换。[文档] ⑥ CI 全矩阵 ≤ 30 分钟。[性能]

---

#### 任务 ID：ADD-FEAT-P2.05.02 8 小时长稳压测

- **基本属性**：绑定差距条目：`features.md` 的 `TASK-3.08.01`（**不在主文档 5.1 的 44 行绑定范围内**）｜ `P2 生态扩展 | 中 | 4 人天` ｜ 前置 `ADD-FEAT-P1.05.07` ｜ `CP: 否` ｜ `Track C` ｜ `[ ] 待开始`
- **代码落地锚点**：`xtask/src/soak.rs`（新增）、`scripts/soak.sh`（新增）、`docs/dev/soak-report.md`（新增）
- **目标与价值**：`features.md` 6.3 的 Phase 3 出口准则 #1：`BUDGET-ROB-01`（`soak_hours = 8.0`、`rss_drift_mb = 2.0`、`pass_rate_pct = 100.0`）。**`.dev-progress.json` 的阻塞项 3 明确："没有任何基准是在空闲机器上跑的"**——长稳压测必须在**空闲机器**上跑，否则数据无意义。
- **技术设计**：
  - **压测的驱动方式**：复用 `crates/ime-fcitx5/src/` 的 XTEST 注入通道（`features-test.md` 的 `TASK-040`，`IN_PROGRESS`）或 `xtask` 的引擎直驱通道（`TASK-049`，`IN_PROGRESS`）。**引擎直驱优先**——它不依赖显示服务器，可在 CI 上跑。
  - **压测场景**（8 小时的构成）：
    | 阶段 | 时长 | 内容 |
    |---|---|---|
    | 稳态输入 | 4h | 10 字/秒的连续输入，模拟真实打字 |
    | 爆发输入 | 1h | 30 秒爆发 + 30 秒空闲的循环 |
    | 空闲 | 2h | 完全无输入，验证 `idle_redraw_count = 0`、`idle_poll_timer_count = 0` |
    | 边界冲击 | 1h | 词库重载、配置重载、用户库损坏、只读切换的循环 |
  - **监测指标**：RSS（每 10 秒采样）、`decode_p99`（每 5 分钟滚动窗口）、`raster_p99`、崩溃计数、日志大小。
  - **通过判据**（`BUDGET-ROB-01`）：无崩溃、`rss_drift_mb ≤ 2.0`、`decode_p99 ≤ 3.0ms` 全程成立、`idle_redraw_count = 0`。
- **NFR**：**必须在空闲机器上跑**（这是本卡的前置条件，不是建议）；8 小时不间断；RSS 采样不干扰被测进程；报告含完整的时间序列数据（不是只给结论）。
- **实施步骤**：① 准备空闲的测试机器（物理机或专用 VM，**无并发负载**）；② `soak.rs` 的四阶段场景；③ RSS / 延迟的采样与时间序列记录；④ 跑满 8 小时；⑤ `docs/dev/soak-report.md` 的完整报告（含时间序列与结论）。
- **DoD**：① 8 小时压测完成，无崩溃。[性能] ② `rss_drift_mb ≤ 2.0`。[性能] ③ `decode_p99 ≤ 3.0ms` 全程成立。[性能] ④ `idle_redraw_count = 0`、`idle_poll_timer_count = 0` 在空闲阶段成立。[性能] ⑤ `docs/dev/soak-report.md` 含完整时间序列，**并注明测试机器的空闲状态**。[文档]

---

#### 任务 ID：ADD-FEAT-P2.05.03 崩溃与诊断包一键导出

- **基本属性**：绑定差距条目：`GAP-41` ｜ `P2 生态扩展 | 低 | 3 人天` ｜ 前置 `ADD-FEAT-P1.05.05` ｜ `CP: 否` ｜ `Track C` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-diag/src/bundle.rs`（新增）、`xtask/src/main.rs`（改：`bundle` 子命令）、`crates/ime-ui/src/command/settings.rs`（改：面板入口）、`docs/dev/diagnostics.md`（新增）
- **目标与价值**：对标 VS Code 的问题报告生成、Obsidian 的调试信息复制。`crash.rs` 会落盘崩溃文件，但**用户不知道路径**、**无导出命令**、**无脱敏后的可分享格式**。用户报 bug 时唯一能提供的是"它崩了"。
- **技术设计**：
  - **诊断包的内容**（**每一项都必须过脱敏复核**）：
    | 内容 | 来源 | 脱敏 |
    |---|---|---|
    | `capabilities.json` | `P1.05.02` 的 `Capabilities` | 无用户数据 |
    | `doctor.json` | `P1.05.05` 的 18 项检查 | 无用户数据 |
    | `config.toml` | 用户配置 | **去掉可能含个人信息的键**（`[data] export_dir`、`[profile] app` 的应用名） |
    | `logs/*.log`（最近 1 个） | `ime-diag` 的日志 | 既有 `RedactLayer` 已处理；**导出前再过一遍** |
    | `crash/*.log`（最近 1 个） | `crash.rs` | 同上 |
    | `versions.json` | 工具链 / fcitx5 / 内核 / 合成器版本 | 无用户数据 |
    | `MANIFEST.txt` | 文件清单 + 生成时间 | 无用户数据 |
  - **导出前必须自检**：包内**不得**含用户词条、输入内容、明文应用名。这个自检是**代码强制**的，不是文档要求：
    ```rust
    /// The patterns a diagnostic bundle must never contain.
    ///
    /// Checked against every file before the bundle is written, because the
    /// cost of shipping a user's typing history in a bug report is not
    /// something a code review catches. A hit aborts the export and names the
    /// file, which is the only outcome that lets us find the leak.
    pub const FORBIDDEN_PATTERNS: &[&str] = &[
        // A user word's pinyin key, which is what `user.redb` stores.
        r"(?m)^[a-z']{2,}\t\d+$",
        // A home directory path with a real user name.
        r"/home/(?!user\b)[a-z0-9_-]+/",
        // The redaction layer's own marker for content it removed.
        r"REDACTED-",
    ];
    ```
  - **`REDACTED-` 的处理**：`RedactLayer` 脱敏后的标记**不应**出现在导出包里——它说明"这里本来有内容被删了"，对排查无用且暗示了内容的存在。导出时**移除**含该标记的行。
- **NFR**：诊断包 ≤ 5MB；导出前自检**强制**（命中即中止）；导出是显式动作；**不含**用户词条、输入内容、明文应用名；**不引入压缩依赖**（若 `P1.04.01` 已引入 `tar`+`flate2` 则复用，否则用目录）。
- **实施步骤**：① `bundle.rs` 的收集与脱敏；② `FORBIDDEN_PATTERNS` 的自检；③ `xtask bundle` 子命令；④ 面板入口；⑤ `docs/dev/diagnostics.md` 说明包内容与如何提交。
- **DoD**：① `rspinyin --bundle` 生成诊断包。[自动] ② 人为在日志中插入一个用户词条，导出被**中止**并指出文件名。[自动] ③ 包内不含 `user.redb`、不含明文应用名、不含 `REDACTED-` 标记。[自动] ④ 诊断包 ≤ 5MB。[性能] ⑤ `docs/dev/diagnostics.md` 说明内容与提交方式。[文档]

---

## 2. 分片出口准则

1. **12 张 P2 卡全部验收通过**，每条验收标准有非空验收记录。[文档]
2. **`P2.02.01` 与 `P2.01.01` 产出明确的 Go/No-Go 结论文档**（`features.md` 6.3 的 Phase 3 出口准则 #3）。[文档]
3. **`P2.03.01` 的"不做插件"决策已写入 ADR 且替代路径完整**。[文档]
4. **8 小时长稳压测通过**：无崩溃、`rss_drift_mb ≤ 2.0`、延迟无退化（`BUDGET-ROB-01`），**且测试在空闲机器上完成**。[性能]
5. **跨发行版 CI 全矩阵**（Ubuntu 22.04/24.04、Fedora 40/41、Arch）全绿。[自动]
6. **发布流水线产出带签名的包与 `SHA256SUMS`**，且**签名密钥未入库**。[自动]
7. **`base_dict` 在 bigram 与领域词库加入后仍 ≤ 20MB**（`BUDGET-SIZE-02`）。[性能]
8. **隐私纪律**：`P2.04.02`（剪贴板）**仅内存不落盘**；`P2.05.03`（诊断包）的强制自检生效；两者均无用户内容泄漏。[自动]
9. **主文档 5.1 追溯表的 P2 行**（8 个差距编号 + `GAP-34` 的"不做"决策）全部有已完成的绑定卡。[文档]
10. **`cargo public-api -p ime-ui` 仍不含 `slint::`**（`P2.02.01` 的 GPU 路径与 `P2.03.02` 的主题包均不得泄漏 Slint 类型）。[自动]
