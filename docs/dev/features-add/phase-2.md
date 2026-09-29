# rspinyin 增量功能扩充 · Phase 2（效率跃升与全键盘工作流）

> 分片版本: v1.0 ｜ 主文档: [../features-add.md](../features-add.md) ｜
> 系统形态: Desktop GUI（Linux 桌面输入法） ｜ 架构基线: Rust 2024 + Slint 1.x + Fcitx5 5.1 ｜
> 关联 ADR: [../adr/0000-upstream-decisions.md](../adr/0000-upstream-decisions.md)、[../adr/0001-frozen-boundary-contracts.md](../adr/0001-frozen-boundary-contracts.md)、ADR-0004（待决策）｜
> 最后同步 Commit: `ee0dbfb` ｜
> 维护约定: 任务状态变更必须回写主文档 5.1 追溯表；假设变更必须回写主文档第 2 节

## 0. 分片基线（引用主文档，不重复定义）

- **任务卡字段规范**：主文档 7.2
- **不可违反的增量约束**：主文档 0.4（继承 `features.md` 0.4 的 11 条 + 追加 A-1~A-4）
- **独占性能预算**：主文档 0.3（P1 合计 ≤ 0.40ms）
- **假设清单**：主文档第 2 节（22 条，强相关：`ASM-A-10` 配置键上限、`ASM-A-14` 键位表上限 64、`ASM-A-15` 竖排形态、`ASM-A-16` D-Bus 只读优先、`ASM-A-17` 发行版覆盖、`ASM-A-18` 不引入新线程）
- **边界契约增量**：主文档 4.2（本分片**不新增** `crates/ime-types` 变更，全部 P1 卡复用 `M0` 已冻结的类型）
- **验收标签**：`features.md` 0.2 的 `[自动]` / `[文档]` / `[实验室]` / `[视觉]` / `[性能]`

**分片范围**：27 张 P1 卡 = 25 张绑定主文档 5.1 的 `GAP-04/06/07/08/11/14/17/18/19/20/21/23/24/25/28/30/31/39/40/42/43/44` + 2 张承接 `features.md` 已排期项（`TASK-2.04.02`、`TASK-2.08.02`）。

**前置**：本分片**全部**卡片依赖主文档 5.0 的 `M0`（契约冻结）。开工前必须确认 `M0` 已决策。

---

## 1. 任务卡

### 1.1 解码能力深化（Track A-解码）

#### 任务 ID：ADD-FEAT-P1.01.01 智能纠错（错键 / 漏键 / 多键 / 乱序）

- **基本属性**：绑定差距条目：`GAP-04` ｜ `P1 效率进阶 | 高 | 4 人天` ｜ 前置 `ADD-FEAT-P0.02.03` ｜ `CP: 否` ｜ `Track A-解码` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-core/src/segment/correct.rs`（新增）、`crates/ime-core/src/viterbi/lattice.rs`（改：纠错边）、`crates/ime-config/src/schema.rs`（改：`[engine] correction`）
- **目标与价值**：对标搜狗的"错字纠正"、微软拼音的"自动纠错"。用户在键盘上打错一个键（`nihap` 应为 `nihao`）、漏一个键（`nihao` 打成 `niho`）、多一个键、或相邻两键颠倒时仍能给出正确候选。这是**最容易被感知为"输入法聪明"**的特性。
- **技术设计**：
  - **复用模糊音的路径扩张机制**（`P0.02.03` 的 `push_fuzzy_edges` 同族）。纠错不是新机制，是**同一机制的不同触发条件**：

    ```rust
    /// The edit operations correction may apply to one syllable.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum EditKind {
        /// One key was typed instead of another: `nihap` for `nihao`.
        Substituted,
        /// One key is missing: `niho` for `nihao`.
        Missing,
        /// One key was typed twice: `niihaoo` for `nihao`.
        Doubled,
        /// Two adjacent keys are the wrong way round: `nial` for `nail`.
        Transposed,
    }

    /// The largest edit distance correction will consider.
    ///
    /// One edit, not two: at distance two the corrected set grows fast enough
    /// to swamp the lattice with candidates the user did not mean, and the
    /// measured benefit flattens. A user who needs two corrections is better
    /// served by typing the syllable again than by a candidate list of
    /// guesses.
    pub const MAX_EDIT_DISTANCE: u8 = 1;

    /// Every syllable within `MAX_EDIT_DISTANCE` of `syllable`.
    ///
    /// # Errors
    /// This function is infallible: it returns no `Result`. A syllable with no
    /// neighbour inside the distance yields a set containing only itself.
    pub fn neighbours(syllable: SyllableId) -> ArrayVec<SyllableId, MAX_CORRECTIONS>;

    /// The most corrections one syllable may produce, bounding the lattice.
    pub const MAX_CORRECTIONS: usize = 6;
    ```
  - **纠错边必须带惩罚，且惩罚大于模糊音**（纠错是"打错了"，模糊音是"读不准"——前者更可能不是用户本意）：

    ```rust
    /// The score penalty one corrected edge carries, in Q8.8.
    ///
    /// Larger than the fuzzy penalty: a fuzzy match means the user's dialect
    /// merges two sounds, so the variant is a real reading; a correction means
    /// the user's finger slipped, so the edge is a guess. A guess must lose to
    /// every real reading at equal weight, and only win over a word that is
    /// markedly less frequent.
    pub const CORRECTION_PENALTY_Q8: i32 = 20 << 8;
    ```
  - **开关默认关闭**：纠错会引入用户没打的候选，默认开启会让老用户觉得"输入法在乱猜"。配置 `[engine] correction = false`，并在诊断中提示可开启。
  - **与模糊音的联合封顶**：复用 `P0.02.04` 建立的 `MAX_TOTAL_EDGES`，优先级为 **精确边 → 简拼边 → 模糊边 → 纠错边**。
- **NFR**：独占预算 ≤ 0.15ms（主文档 0.3 的 P1 池内）；确定性（`neighbours` 的顺序由音节表下标决定）；单音节纠错上限 `MAX_CORRECTIONS = 6`；与模糊音+简拼同时开启时的组合 `decode_p99` ≤ 3.0ms。
- **实施步骤**：① 实现 `EditKind` 与 `neighbours`（纯函数，按音节表下标枚举四类编辑）；② 在 `lattice.rs` 接入 `push_correction_edges`，带 `CORRECTION_PENALTY_Q8`；③ 加 `[engine] correction` 配置（默认 false）；④ 纳入 `MAX_TOTAL_EDGES` 联合封顶；⑤ 四类编辑各一条命中测试 + 惩罚优先测试 + 组合基准。
- **DoD**：① `cargo nextest run -p ime-core` 全绿，新增 ≥ 10 条（四类编辑各 2 条 + 上限截断 + 惩罚优先 + 默认关闭时零开销 + 确定性）。[自动] ② 手工验证：开启后输入 `nihap` 得到「你好」。[实验室] ③ 三特性同时开启的 `decode_p99` ≤ 3.0ms。[性能]

---

#### 任务 ID：ADD-FEAT-P1.01.02 日期 / 时间 / 数字大写等计算类输入

- **基本属性**：绑定差距条目：`GAP-06` ｜ `P1 效率进阶 | 中 | 3 人天` ｜ 前置 `ADD-FEAT-P0.01.03` ｜ `CP: 否` ｜ `Track A-数据` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-core/src/phrase/computed.rs`（新增）、`crates/ime-fcitx5/src/engine.rs`（改：注入时钟与数字上下文）、`data/raw/phrase.tsv`（改：内置计算类短语）
- **目标与价值**：对标搜狗的日期时间输入与数字大写、微软拼音的 V 模式。`rq`→`2026年9月29日`、输入 `1234` 时给出「壹仟贰佰叁拾肆」。**这是"每天都用得上"的实用特性**。
- **技术设计**：
  - **复用 `P0.01.03` 的短语注入点**，但文本由**计算**而非查表得出：

    ```rust
    /// A phrase whose body is computed rather than looked up.
    ///
    /// The computation needs the clock and the current raw input, neither of
    /// which `ime-core` may read (0.4 rule 4), so the host layer resolves the
    /// value and hands the finished string to the phrase injector. What lives
    /// here is only the *shape* of the request and the number formatting,
    /// which is pure.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum ComputedPhrase {
        /// Current date in the configured format.
        Date,
        /// Current time.
        Time,
        /// Current weekday.
        Weekday,
        /// A number spelled in Chinese uppercase: `1234` to `壹仟贰佰叁拾肆`.
        UpperCaseNumber,
        /// A number spelled in Chinese lowercase: `1234` to `一千二百三十四`.
        LowerCaseNumber,
    }

    /// The key that triggers each computed phrase, as written in `phrase.tsv`.
    ///
    /// The keys are project-chosen and stable: a user who learned `rq` on
    /// another input method expects `rq` here too.
    pub const TRIGGERS: [(ComputedPhrase, &str); 5] = [
        (ComputedPhrase::Date, "rq"),
        (ComputedPhrase::Time, "sj"),
        (ComputedPhrase::Weekday, "xq"),
        (ComputedPhrase::UpperCaseNumber, "dx"),
        (ComputedPhrase::LowerCaseNumber, "xx"),
    ];

    /// Spells an integer in Chinese uppercase.
    ///
    /// Pure: the digits come from the raw input, not from the clock.
    ///
    /// # Errors
    /// This function is infallible: it returns no `Result`. A value outside
    /// `i64` yields an empty string, which the caller treats as "no
    /// candidate".
    pub fn to_upper_case_number(value: i64) -> String;

    /// Spells an integer in Chinese lowercase.
    ///
    /// # Errors
    /// This function is infallible: it returns no `Result`.
    pub fn to_lower_case_number(value: i64) -> String;
    ```
  - **数字大写的触发条件**：raw 输入**全部为数字**且长度 ≥ 2 时，在候选列表尾部追加大小写两种写法（不占用首候选位——用户打数字时多半就是要数字）。
  - **金额格式**：`1234.56` → `壹仟贰佰叁拾肆元伍角陆分`，小数点后最多两位。
- **NFR**：独占预算 ≤ 0.05ms；`ime-core` **不得**读时钟（0.4 规则 4）——时钟由 `ime-fcitx5` 注入，`computed.rs` 只做纯格式化；数字超过 `i64` 范围时返回空串而非 panic。
- **实施步骤**：① 实现 `to_upper_case_number` / `to_lower_case_number`（纯函数，含零的读法规则）；② 实现 `TRIGGERS` 与 `ComputedPhrase`；③ 在 `engine.rs` 注入时钟并把解析结果交给短语注入器；④ 数字上下文的候选追加；⑤ 金额格式与边界（0、负数、超范围、小数点后超两位）。
- **DoD**：① `cargo nextest run -p ime-core` 全绿，新增 ≥ 12 条（数字大写含零/进位/边界、金额、日期时间格式、超范围空串）。[自动] ② 手工验证：`rq` → 今日日期；输入 `1234` → 候选含「壹仟贰佰叁拾肆」。[实验室] ③ 实测 ≤ 0.05ms。[性能]

---

#### 任务 ID：ADD-FEAT-P1.01.03 中英混输（英文词候选）

- **基本属性**：绑定差距条目：`GAP-08` ｜ `P1 效率进阶 | 高 | 5 人天` ｜ 前置 `ADD-FEAT-P0.01.01` ｜ `CP: 否` ｜ `Track A-数据` ｜ `[ ] 待开始`
- **代码落地锚点**：`data/raw/english.tsv`（新增，**新词源，需登记白名单**）、`xtask/src/dictc/builders/english.rs`（新增）、`crates/ime-core/src/passthrough.rs`（改：混输判定）、`crates/ime-core/src/viterbi/decoder.rs`（改：英文候选注入）、`data/sources.toml`（改）
- **目标与价值**：对标搜狗/微软拼音的中英混输。当前 rspinyin 只有"整串直通"——用户打 `hello` 时得到的是一串拼音候选，要切到英文模式才能打出 `hello`。中英混输让英文单词**直接出现在候选里**。
- **技术设计**：
  - **词源决策（必须走 ADR-0000 的流程）**：英文词表是**新词源**，必须过 `check-dict-sources.sh` 白名单。候选来源：**项目自建的高频英文词表**（从公开的、宽松许可的词频表派生，或由项目统计生成）。**不得**直接抓取商业输入法的英文词库。
  - **独立的 `english.dict`**（与 `P0.02.05` 的 `script.dict` 同族设计——复用容器格式，`DICT_FORMAT_VERSION` 保持 1）：

    ```rust
    /// An English word candidate offered alongside the pinyin candidates.
    ///
    /// The word is matched against the raw input directly, not through the
    /// syllable table: an English word is not a sequence of pinyin syllables,
    /// and forcing it through the DAG would mean inventing readings for it.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct EnglishHit {
        /// Byte range of the matched input.
        pub start: u16,
        pub end: u16,
        /// Rank within the English list, 0 being the most frequent.
        pub rank: u16,
    }

    /// The most English candidates injected for one input.
    ///
    /// Three. The user is typing pinyin, so an English word is a detour; a
    /// list of ten English words would push the pinyin candidates off the
    /// first page, which is the opposite of helping.
    pub const MAX_ENGLISH_CANDIDATES: usize = 3;

    /// The shortest input that may produce an English candidate.
    ///
    /// Three letters. Below that the English and pinyin candidate sets overlap
    /// so heavily that every short input would show English noise.
    pub const MIN_ENGLISH_LEN: usize = 3;
    ```
  - **注入位置**：候选列表**尾部**（不是首位）。`CandidateSource::Dict` 的拼音候选保持在前——用户在用拼音输入法。
  - **判定条件**：raw 长度 ≥ `MIN_ENGLISH_LEN`、raw 全为 ASCII 字母、且**当前无高置信度拼音候选**（首候选的 `score` 低于阈值）时才注入。
- **NFR**：独占预算 ≤ 0.15ms；`english.dict` 体积 ≤ 2MB；词源必须登记白名单且 `permissive = true`；**不得**因混输而改变拼音候选的顺序。
- **实施步骤**：① **先开 ADR 或走 ADR-0000 的补充登记**：确认英文词源的许可，登记 `data/sources.toml`；② 实现 `builders/english.rs` 生成 `english.dict`；③ 实现 `EnglishHit` 与匹配；④ 在 `decoder.rs` 的候选汇总期注入（尾部，≤ 3 条）；⑤ 判定条件与边界（短输入、纯拼音命中时的抑制）。
- **DoD**：① `data/compiled/english.dict` 生成成功且 ≤ 2MB。[自动] ② `bash scripts/check-dict-sources.sh` 通过，新词源登记且 `permissive = true`。[自动] ③ `cargo nextest run -p ime-core` 全绿，新增 ≥ 8 条。[自动] ④ 手工验证：输入 `hello` 时候选尾部出现 `hello`。[实验室] ⑤ 拼音候选顺序不受影响（`test_english_injection_preserves_pinyin_order`）。[自动]

---

#### 任务 ID：ADD-FEAT-P1.01.04 符号与 Emoji 面板

- **基本属性**：绑定差距条目：`GAP-07` ｜ `P1 效率进阶 | 高 | 5 人天` ｜ 前置 `ADD-FEAT-P0.01.03` ｜ `CP: 否` ｜ `Track A-数据 ｜ Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`data/raw/symbols.tsv`（新增）、`xtask/src/dictc/builders/symbols.rs`（新增）、`crates/ime-core/src/symbol.rs`（新增）、`crates/ime-ui/ui/panel.slint`（新增）、`crates/ime-ui/src/ui_thread/surface.rs`（改：面板 surface）、`crates/ime-types/src/ui.rs`（复用 `M0` 的 `CandidateSource::Symbol`）、`crates/ime-config/src/schema.rs`（改：`[symbols]` 段）
- **目标与价值**：对标搜狗的 13 类符号面板、微软拼音的 Emoji 面板、macOS 的字符检视器、RIME 的 `symbols` 段。`CandidateSource::Symbol` **已冻结但从未被构造**——这是它的第一个消费者。
- **技术设计**：
  - **面板是候选框的同族 surface**（主文档 `C-1`）：同一 `SurfaceBackend`、同一 `ThemeTokens`、同一 `SpringIntegrator`。**禁止**另起渲染路径。
  - **面板不夺焦**（0.4 规则 5）：`override_redirect`（X11）/ `keyboard_interactivity = none`（Wayland），与候选框同族。
  - **面板尺寸变化不触发候选框重新定位**（主文档 `C-5`）：面板从候选框边缘生长，锚点不动。
  - **符号表按分类组织**：

    ```rust
    /// One category of symbols the panel shows.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct SymbolCategory {
        /// Stable identifier, used in configuration and diagnostics.
        pub id: &'static str,
        /// Label shown on the panel's category rail.
        pub label: &'static str,
        /// Trigger typed in the preedit to jump straight to this category.
        pub trigger: &'static str,
        /// Index into the symbol table where this category's entries start.
        pub start: u16,
        pub len: u16,
    }

    /// The categories the panel ships with.
    ///
    /// Ordered by how often a Chinese user reaches for them, which is why
    /// punctuation comes before arrows rather than after.
    pub const CATEGORIES: &[SymbolCategory] = &[
        SymbolCategory { id: "punct",  label: "标点", trigger: "bd",  start: 0,    len: 0 },
        SymbolCategory { id: "math",   label: "数学", trigger: "sx",  start: 0,    len: 0 },
        SymbolCategory { id: "arrow",  label: "箭头", trigger: "jt",  start: 0,    len: 0 },
        SymbolCategory { id: "shape",  label: "图形", trigger: "tx",  start: 0,    len: 0 },
        SymbolCategory { id: "emoji",  label: "表情", trigger: "bq",  start: 0,    len: 0 },
    ];
    ```
  - **`symbols.dict` 复用容器格式**（与 `script.dict`、`english.dict` 同族），键为分类触发词 + 序号，值为符号本身。
  - **快捷键**：`Ctrl+Shift+4` 打开面板（进 `[keys.bindings]`）；面板内 `Tab`/`Shift+Tab` 切分类，方向键选符号，`Enter` 提交，`Esc` 关闭。
- **NFR**：面板打开/关闭的动效沿用 Spring 参数（`ω₀=26.0` / `ζ=0.85`），**不得**引入 ease 曲线（主文档 `C-4`）；`symbols.dict` ≤ 1MB；面板 surface 的常驻内存 ≤ 2MB（计入 `ui_rss = 18MB`）；面板**不得**夺取键盘焦点。
- **实施步骤**：① 整理 `symbols.tsv`（分类 + 符号，来源为 Unicode 字符表，无许可负担）；② 生成 `symbols.dict`；③ 实现 `SymbolCategory` 与 `symbol.rs` 的查找；④ `panel.slint` 骨架 + `surface.rs` 的面板 surface（复用候选框的 `SurfaceBackend`）；⑤ 按键路由（打开/切分类/选择/关闭）与 `[keys.bindings]` 绑定。
- **DoD**：① 面板在 X11 与 Wayland/wlroots 两档上可打开、可切分类、可提交符号。[实验室] ② 焦点断言：面板打开时**当前应用的输入焦点不丢失**（键盘输入仍进入应用）。[实验室] ③ 面板动效的帧率 ≥ 55fps，无肉眼可见掉帧。[视觉] ④ 面板 surface 的常驻内存 ≤ 2MB。[性能] ⑤ `cargo nextest run -p ime-core` 全绿（符号查找与分类切换的纯逻辑）。[自动]

---

#### 任务 ID：ADD-FEAT-P1.01.05 词频时间衰减与场景化权重

- **基本属性**：绑定差距条目：`GAP-11` ｜ `P1 效率进阶 | 中 | 3 人天` ｜ 前置 `ADD-FEAT-P0.01.04` ｜ `CP: 否` ｜ `Track A-数据` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-dict/src/user_db/decay.rs`（新增）、`crates/ime-dict/src/user_db.rs`（改：`freq` 的读取路径）、`crates/ime-config/src/schema.rs`（改：`[data] decay_halflife_days`）
- **目标与价值**：对标搜狗的"词频随时间调整"、微软拼音的自学习权重动态调整。**问题**：当前 `UserDb` 的权重是**单调累加**的——用户三年前学的一个词，权重与昨天学的词一样高，只要它被累计提交的次数多。结果是"输入法记得太久以前的事"。
- **技术设计**：
  - **衰减在读取时计算，不修改存储**（这是关键设计决策——写入时衰减需要定期全表扫描，会阻塞）：

    ```rust
    /// The half-life of a learned word's weight, in days.
    ///
    /// 90 days. Learning should fade slowly enough that a word used every few
    /// weeks keeps its advantage, and fast enough that a word the user
    /// abandoned last year stops outranking one they use daily. A half-life
    /// shorter than a month makes the input method feel like it forgot.
    pub const DECAY_HALFLIFE_DAYS_DEFAULT: u16 = 90;

    /// The decayed weight of a record, in the same units as the stored weight.
    ///
    /// Computed on read so that no background pass has to rewrite the store:
    /// a decay pass would have to touch every row, and the store is on the
    /// user's disk, not in a database with a scheduler. The stored weight is
    /// the historical total; this is what ranking actually uses.
    ///
    /// # Errors
    /// This function is infallible: it returns no `Result`. A clock that moved
    /// backwards yields the undecayed weight rather than a value above it.
    pub fn decayed_weight(
        stored: u32,
        last_used_unix: u64,
        now_unix: u64,
        halflife_days: u16,
    ) -> u32;

    /// The floor a decayed weight never falls below.
    ///
    /// A word the user learned is still a word they typed, so it keeps a
    /// minimum standing instead of decaying to zero and becoming invisible.
    pub const DECAY_FLOOR: u32 = 8;
    ```
  - **定点运算**：衰减用 Q8.8 定点（`0.4` 规则 9 同族——排序必须可复现，不得由 `f32` 驱动）。半衰期计算用整数近似（`stored >> (elapsed_days / halflife_days)`），避免浮点。
  - **场景化权重**：本卡**只做时间衰减**。按应用/场景分离的权重需要 `per-app profile`（`ADD-FEAT-P1.02.07`）的键空间支持，本卡在其上预留接口（`decayed_weight` 的签名已可接受不同的 `now_unix`）。
- **NFR**：`decayed_weight` 在 `freq` 的读取路径上，独占预算 ≤ 0.05ms（纯整数运算，实测应在纳秒级）；**不得**引入后台重写任务（`ASM-A-18` 不引入新线程）；时钟回拨时返回未衰减值（不返回放大值）。
- **实施步骤**：① 实现 `decayed_weight`（纯函数 + 整数半衰期近似）；② 在 `user_db.rs` 的 `freq` 读取路径接入；③ 加 `[data] decay_halflife_days` 配置（默认 90，范围 7..=365）；④ 边界：时钟回拨、`halflife = 0`（禁用衰减）、极旧记录、`DECAY_FLOOR`。
- **DoD**：① `cargo nextest run -p ime-dict` 全绿，新增 ≥ 8 条（半衰期计算、时钟回拨、floor、禁用、极旧记录、确定性）。[自动] ② 实测 `freq` 路径耗时 ≤ 0.05ms。[性能] ③ 手工验证：构造一个 `last_used_unix` 为一年前的记录，断言其排序权重低于一条昨天的同权重记录。[实验室]

---

#### 任务 ID：ADD-FEAT-P1.01.06 用户自建词表导入

- **基本属性**：绑定差距条目：`GAP-14` ｜ `P1 效率进阶 | 中 | 3 人天` ｜ 前置 `ADD-FEAT-P0.01.01` ｜ `CP: 否` ｜ `Track A-数据` ｜ `[ ] 待开始`
- **代码落地锚点**：`xtask/src/dictc/source/user_table.rs`（新增）、`crates/ime-dict/src/user_db/import.rs`（改：支持用户词表）、`crates/ime-config/src/schema.rs`（改：`[data] user_tables`）、`data/sources.toml`（改：登记用户词表机制）
- **目标与价值**：对标 RIME 的 `*.dict.yaml` 自定义词表、搜狗的 txt 词库导入。让专业用户（医生、律师、程序员）把自己的术语表导进来。**`ASM-A-13` 明确只接受 TSV**——不接受 `.scel`/`.bcd`（其格式无公开规范，解析即构成对商业词库的间接使用）。
- **技术设计**：
  - **复用 `dictc` 的 TSV 校验路径**：用户词表与 `data/raw/base.tsv` **同一格式**（`词<TAB>读音<TAB>权重<TAB>标志`），走同一个解析器，因此同一套边界行为（超长词跳过、缺列跳过、畸形计入统计）。
  - **用户词表编译进用户库而非 `base.dict`**：`base.dict` 是系统级只读文件，用户词表是用户级可写数据。用户词表在 `dictc` 校验后写入 `user.redb`（`WordFlags::USER`），与学习词条同一存储。

    ```rust
    /// One user-supplied word table.
    ///
    /// Kept separate from the learned words: a table is a declaration the user
    /// made, and it must survive "forget everything I learned" and be
    /// re-importable. Learned words are observations; a table is a statement.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct UserTable {
        /// Absolute path of the source TSV.
        pub path: PathBuf,
        /// Rows that were imported.
        pub imported: u64,
        /// Rows skipped, with the reason, for the diagnostics surface.
        pub skipped: Vec<SkippedRow>,
    }

    /// Why one row of a user table was skipped.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct SkippedRow {
        pub line: u64,
        pub reason: SkipReason,
    }

    /// The reasons a row is skipped.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum SkipReason {
        /// Fewer than two columns.
        MissingColumn,
        /// The word is longer than `MAX_WORD_LEN` bytes.
        WordTooLong,
        /// The reading does not reduce to a syllable sequence.
        UnreadableReading,
        /// The weight is not a decimal integer.
        BadWeight,
        /// A tab or a newline inside the word.
        IllegalCharacter,
    }
    ```
  - **导入是显式动作**：`xtask` 提供 `import-user-table` 子命令；**不**自动扫描目录（自动扫描会让用户忘记自己导入过什么）。
- **NFR**：导入是**一次性动作**，不在输入热路径；1 万行导入 ≤ 2 秒；畸形行**跳过并计数**，不中断（既有 `dictc` 的同族行为）；导入的词条**不参与** `evict_oldest` 淘汰（它们是声明，不是观察）。
- **实施步骤**：① 复用 `dictc` 的 TSV 解析器，抽出可被 `ime-dict` 调用的入口；② 实现 `UserTable` 与 `SkippedRow`；③ 导入写 `user.redb` 并标 `WordFlags::USER`；④ `evict_oldest` 跳过用户表词条；⑤ `xtask import-user-table` 子命令 + 配置 `[data] user_tables`。
- **DoD**：① `cargo nextest run -p ime-dict` 全绿，新增 ≥ 10 条（五类跳过原因各 1 条 + 导入计数 + 淘汰豁免 + 重复导入幂等）。[自动] ② 手工验证：导入一个含 100 个专业术语的 TSV，输入其拼音得到该术语。[实验室] ③ 1 万行导入 ≤ 2 秒。[性能] ④ 导入的词条在 `evict_oldest` 后仍存在。[自动]

---

### 1.2 交互与工作流（Track B）

#### 任务 ID：ADD-FEAT-P1.02.01 命令面板（`Ctrl+Shift+/`）

- **基本属性**：绑定差距条目：`GAP-17` ｜ `P1 效率进阶 | 高 | 6 人天` ｜ 前置 `ADD-FEAT-P0.02.02` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-ui/ui/command_panel.slint`（新增）、`crates/ime-ui/src/ui_thread/panel.rs`（新增）、`crates/ime-ui/src/command/mod.rs`（新增）、`crates/ime-types/src/ui.rs`（**需 ADR-0004 追加**：`UiCommand::Panel` / `UiEvent::PanelAction`）、`crates/ime-fcitx5/src/engine.rs`（改：`Ctrl+Shift+/` 路由）
- **目标与价值**：对标 Raycast 的命令面板、Linear 的 `⌘K`、VS Code 的 `Ctrl+Shift+P`。**这是"输入法也能有现代交互"的标志性特性**，同时是 `features.md` 的 `TASK-2.03.03` 的落地，并承载 ADR-0000 的 `OB-1` **补充路径**（面板内提供"关于"入口并渲染 `AboutSlint`）。
- **技术设计**：
  - **面板是候选框的同族 surface**（`C-1`），复用 `SurfaceBackend` / `ThemeTokens` / `SpringIntegrator`。
  - **命令注册表**：

    ```rust
    /// One command the panel can run.
    ///
    /// Commands are registered as data, not as match arms: the panel, the
    /// `[keys.bindings]` table and the D-Bus surface all address commands by
    /// the same identifier, and a new command must not need edits in three
    /// places.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct CommandSpec {
        /// Stable identifier, used in `[keys.bindings]` and in diagnostics.
        pub id: &'static str,
        /// Label shown in the panel.
        pub label: &'static str,
        /// Extra words the fuzzy filter matches on, so a user can find
        /// "切换简繁" by typing "jiantifanti" or "script".
        pub keywords: &'static [&'static str],
        /// The configuration key this command writes, when it toggles one.
        pub config_key: Option<&'static str>,
    }

    /// The commands this build registers.
    pub const COMMANDS: &[CommandSpec] = &[
        CommandSpec { id: "toggle-script",      label: "切换简繁",   keywords: &["script", "jiantifanti"], config_key: Some("script.traditional") },
        CommandSpec { id: "toggle-full-width",  label: "切换全角",   keywords: &["fullwidth", "quanjiao"], config_key: Some("engine.full_width") },
        CommandSpec { id: "toggle-punct",       label: "切换标点",   keywords: &["punct", "biaodian"],     config_key: Some("engine.punct_mode") },
        CommandSpec { id: "switch-scheme",      label: "切换方案",   keywords: &["scheme", "shuangpin"],   config_key: Some("scheme.scheme") },
        CommandSpec { id: "edit-phrases",       label: "编辑短语",   keywords: &["phrase", "duanyu"],      config_key: None },
        CommandSpec { id: "edit-bindings",      label: "编辑键位",   keywords: &["keys", "jianwei"],       config_key: None },
        CommandSpec { id: "manage-user-words",  label: "管理词库",   keywords: &["userdict", "ciku"],      config_key: None },
        CommandSpec { id: "show-capabilities",  label: "能力状态",   keywords: &["doctor", "status"],      config_key: None },
        CommandSpec { id: "about",              label: "关于",       keywords: &["about", "guanyu", "slint"], config_key: None },
    ];
    ```
  - **`about` 命令渲染 `AboutSlint`**（ADR-0000 的 `OB-1` 补充路径）。**注意**：渲染 Slint 组件不违反 `OB-4`——`OB-4` 禁止的是**导出 Slint 类型到公共 API**，`AboutSlint` 是内部使用。
  - **契约增量**（**必须走 ADR-0004**）：`UiCommand::Panel { open: bool }` 与 `UiEvent::PanelAction { command_id: u16, arg: u32 }`。`command_id` 是 `COMMANDS` 的下标（`u16`），避免字符串跨边界。
- **NFR**：面板打开动效沿用 Spring（`C-4`）；面板**绝不夺焦**（0.4 规则 5）；面板尺寸变化**不触发**候选框重新定位（`C-5`）；命令注册表 ≤ 32 条；面板的模糊过滤 ≤ 16ms（用户每敲一个字母都要重算）。
- **实施步骤**：① **先落 ADR-0004 的 `UiCommand::Panel` / `UiEvent::PanelAction`**；② 实现 `CommandSpec` / `COMMANDS`；③ `command_panel.slint` 骨架（分类列表 + 模糊过滤 + 快捷键提示）；④ `panel.rs` 的面板 surface 与生命周期；⑤ `Ctrl+Shift+/` 路由；⑥ 每条命令的执行体（前四条改配置，后五条打开对应子面板）。
- **DoD**：① 面板可打开、可模糊过滤、可执行 9 条命令。[实验室] ② 焦点断言：面板打开时不夺焦。[实验室] ③ `about` 命令渲染 `AboutSlint` 且 `cargo public-api -p ime-ui` **不含** `slint::`（`OB-4` 不被违反）。[自动] ④ 模糊过滤 ≤ 16ms。[性能] ⑤ `cargo nextest run --workspace --all-features` 全绿。[自动]

---

#### 任务 ID：ADD-FEAT-P1.02.02 命令面板内的设置项与全键盘可达

- **基本属性**：绑定差距条目：`GAP-17`、`GAP-25` ｜ `P1 效率进阶 | 中 | 4 人天` ｜ 前置 `ADD-FEAT-P1.02.01` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-ui/src/command/settings.rs`（新增）、`crates/ime-ui/ui/settings_panel.slint`（新增）、`crates/ime-config/src/schema.rs`（改：设置项的元数据）
- **目标与价值**：对标 VS Code 的设置 UI。让用户**不编辑 TOML** 就能改配置。**全键盘可达**是硬要求（`GAP-25`）：`Tab` 在设置项间移动、方向键改值、`Enter` 确认、`Esc` 取消，**无鼠标闭环**。
- **技术设计**：
  - **设置项由 schema 派生，不手工维护**——避免"配置加了一个键，设置 UI 忘了加"：

    ```rust
    /// One configurable setting, derived from the schema rather than listed.
    ///
    /// The list is generated from `Config`'s key whitelist so a new
    /// configuration key cannot be added without a place to change it. The
    /// derivation is what keeps the panel and the file from drifting.
    #[derive(Clone, Debug, PartialEq)]
    pub struct SettingSpec {
        /// The dotted configuration key, e.g. `ui.max_per_row`.
        pub key: &'static str,
        /// Label shown in the panel.
        pub label: &'static str,
        /// The control that edits it.
        pub control: SettingControl,
        /// The help line shown under the control.
        pub help: &'static str,
    }

    /// The control a setting is edited with.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum SettingControl {
        /// A boolean switch.
        Toggle,
        /// A bounded integer stepper.
        Stepper { min: i64, max: i64, step: i64 },
        /// A choice among a fixed set.
        Choice { count: u8 },
        /// A colour picker over the accent palette.
        Accent,
        /// Read-only, for values the panel reports but does not edit.
        ReadOnly,
    }
    ```
  - **5 态覆盖**（主文档 `C-3`）：设置项是新增控件，**必须**有 Disabled 态。这依赖 `ADD-FEAT-P1.03.03` 补齐 `ThemeTokens.state_disabled` 与 `theme.slint` 的 `state-disabled`。
  - **键盘流**：`Tab`/`Shift+Tab` 移动、`↑`/`↓` 在同组内移动、`←`/`→` 改值、`Enter` 提交、`Esc` 取消全部更改。**必须**有可见的 Focus Ring（既有 `state-selected-stroke` 同族）。
- **NFR**：设置面板**绝不夺焦**；改值**立即生效但不重置进行中的会话**（0.4 规则 10）；`Esc` 取消必须真正回滚（不能只关闭面板）。
- **实施步骤**：① 实现 `SettingSpec` / `SettingControl` 与从 schema 的派生；② `settings_panel.slint`（分组 + 控件 + 帮助行）；③ 键盘流实现；④ 值写入 `Config` 并触发热重载；⑤ `Esc` 的回滚（保存打开面板时的配置快照）。
- **DoD**：① **无鼠标闭环**：从打开面板到改完所有设置再到关闭，全程只用键盘。[实验室] ② Focus Ring 完整无截断。[视觉] ③ 改值后**进行中的输入不中断**。[自动] ④ `Esc` 取消后配置与打开前逐键一致。[自动]

---

#### 任务 ID：ADD-FEAT-P1.02.03 键位表编辑界面

- **基本属性**：绑定差距条目：`GAP-25` ｜ `P1 效率进阶 | 中 | 4 人天` ｜ 前置 `ADD-FEAT-P1.02.01` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-ui/ui/bindings_panel.slint`（新增）、`crates/ime-ui/src/command/bindings.rs`（新增）、`crates/ime-config/src/schema.rs`（改：`[keys.bindings]` 表）
- **目标与价值**：为 `ADD-FEAT-P1.02.04`（全量键位自定义）提供编辑界面。没有界面，键位自定义只能手改 TOML——那对普通用户等于不存在。
- **技术设计**：
  - **键位冲突检测**：录入一个键时立即检查是否与既有绑定冲突，冲突项高亮并给出"替换/取消"两个选项。**这是编辑界面的核心价值**，手改 TOML 时用户不知道自己撞了哪个键。
  - **键名解析复用既有 `KeyName`**（`crates/ime-config/src/schema.rs:149`）——`KeyName::parse` 已存在，直接复用，不新造解析器。
  - **录入方式**：按下组合键即录入（`Ctrl+Shift+F` → `"ctrl+shift+f"`），而不是从下拉框里选——下拉框无法表达组合键。
  - **键位表上限** `MAX_BINDINGS = 64`（`ASM-A-14`）；达到上限时"添加"按钮进 Disabled 态（依赖 `P1.03.03` 的 `state-disabled`）。
- **NFR**：键位表 ≤ 64 条；冲突检测在录入时同步完成（≤ 5ms）；删除一条绑定后**其动作回到默认键位**（不是消失）。
- **实施步骤**：① `[keys.bindings]` 的 schema（`Vec<Binding>`，每项 `key` + `action`）；② 冲突检测；③ `bindings_panel.slint`（列表 + 录入 + 冲突提示 + 删除）；④ 复用 `KeyName::parse` 的键名规范化；⑤ 保存与热重载。
- **DoD**：① 可添加、删除、修改绑定；冲突时给出提示。[实验室] ② 删除绑定后动作回到默认键位。[自动] ③ `cargo nextest run -p ime-config` 全绿（键名解析、冲突检测、上限）。[自动] ④ 全键盘可操作。[实验室]

---

#### 任务 ID：ADD-FEAT-P1.02.04 全量键位自定义（`[keys.bindings]`）

- **基本属性**：绑定差距条目：`GAP-18` ｜ `P1 效率进阶 | 中 | 4 人天` ｜ 前置 `ADD-FEAT-P1.02.03` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-config/src/schema.rs`（改：`KeysConfig.bindings`）、`crates/ime-fcitx5/src/engine.rs`（改：按键路由查表而非硬编码）、`crates/ime-core/src/state/transitions.rs`（改：`KeyAction` 的解析）
- **目标与价值**：对标 RIME 的 `key_binder/bindings`、VS Code 的 `keybindings.json`。当前 `KeysConfig` 只有 4 个字段（`digit_zero` / `enter_commit_raw` / `flip_keys` / `highlight_keys`），**无法把任意动作绑定到任意键**。本卡把按键路由从"硬编码分支"改为"查表"。
- **技术设计**：
  - **路由表的构造**：启动时把 `[keys.bindings]` + 既有的 4 个字段**合并**成一张 `BindingTable`，按键路径只查表：

    ```rust
    /// The resolved key-to-action table.
    ///
    /// Built once per configuration load by merging the four legacy key fields
    /// with `[keys.bindings]`. The legacy fields keep working — a user who
    /// configured `flip_keys` five releases ago must not have to rewrite it —
    /// and `[keys.bindings]` wins where the two disagree, because it is the
    /// one the user wrote deliberately.
    pub struct BindingTable {
        /// Sorted by key, so a lookup is a binary search on the hot path.
        entries: Vec<(KeyCombo, KeyAction)>,
    }

    /// One key combination.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    pub struct KeyCombo {
        /// Modifier mask, matching the frozen `KeyAction` translation layer.
        pub mods: u8,
        /// The keysym, normalised through `KeyName`.
        pub key: u16,
    }
    ```
  - **动作名到 `KeyAction` 的映射**：字符串 → `KeyAction` 的解析器（`"toggle-script"` → `KeyAction::ToggleScript`），复用 `P1.02.01` 的 `COMMANDS` 标识符，保证"命令面板里能做的"和"能绑键的"是同一套。
  - **优先级**：`[keys.bindings]` > 既有 4 个字段 > 内置默认。冲突时**后者不生效但不报错**（用户可能故意覆盖）。
- **NFR**：按键路径的查表 ≤ 1µs（二分查找，`entries` ≤ 64 + 既有 4 项的展开）；`BindingTable` 的构造在配置加载时完成，不在按键路径上；**不得**让键位表影响 `key_to_present_p99 = 16ms`。
- **实施步骤**：① `[keys.bindings]` 的 schema 与 `BindingTable` 的构造；② 动作名 → `KeyAction` 的解析器；③ `engine.rs` 的按键路由改为查表；④ 合并优先级（`bindings` > 旧字段 > 默认）；⑤ 回归：既有 4 个字段的行为**逐条不变**。
- **DoD**：① 任意 `KeyAction` 可绑定到任意键，绑定后生效。[实验室] ② 既有 4 个字段的行为不变（既有测试全绿）。[自动] ③ 按键路径查表 ≤ 1µs。[性能] ④ `key_to_present_p99 ≤ 16ms` 不劣化。[性能]

---

#### 任务 ID：ADD-FEAT-P1.02.05 快速造词（选中即学 + 手动加词）

- **基本属性**：绑定差距条目：`GAP-24` ｜ `P1 效率进阶 | 低 | 2 人天` ｜ 前置 `ADD-FEAT-P0.01.04` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-core/src/state/transitions.rs`（改：`AddPhrase` 跃迁体）、`crates/ime-fcitx5/src/engine.rs`（改：按键路由）、`crates/ime-ui/src/command/user_words.rs`（新增）
- **目标与价值**：对标搜狗的"选中文本后按键加词"、微软拼音的自造词、RIME 的用户词表。当前只有**自动学习**——用户无法主动"把这两个字记成一个词"。
- **技术设计**：
  - **两个入口**：
    1. **选中即学**：在候选列表中高亮某个候选时按绑定键（`KeyAction::PinHighlighted`，`M0` 已落地）→ 该词被**固定**（`pinned = true`），豁免淘汰（`P0.01.04` 的 `UserRecord.pinned`）。
    2. **手动加词**：在命令面板的"管理词库"里输入 `词 + 拼音` 手动添加。
  - **与短语的区别**（必须在 UI 上说清楚）：加词是"让这个词在拼音候选里排前面"；短语是"打这几个字母直接出这段文本"。前者进入候选竞争，后者直接替换。
  - **`pinned` 的语义**：固定词豁免 `evict_oldest`，且权重有下限（`DECAY_FLOOR` 之上）。用户固定的词**不会因为久不用而消失**——这是"固定"的意思。
- **NFR**：固定操作在按键路径上，≤ 0.05ms（复用 `P0.01.04` 的 `record` 路径）；固定词不计入 `USER_WORD_CAP` 的淘汰候选；取消固定需要显式操作（不自动过期）。
- **实施步骤**：① `AddPhrase` / `PinHighlighted` 的跃迁体；② 按键路由；③ 命令面板的"管理词库"界面（列表 + 添加 + 固定/取消固定 + 删除）；④ 固定词的淘汰豁免与权重下限。
- **DoD**：① 高亮候选后按键即固定，重启后仍存在。[实验室] ② 固定的词在 `evict_oldest` 后仍存在。[自动] ③ 命令面板可手动加词。[实验室] ④ `cargo nextest run -p ime-core` 全绿。[自动]

---

#### 任务 ID：ADD-FEAT-P1.02.06 常驻输入状态指示

- **基本属性**：绑定差距条目：`GAP-23` ｜ `P1 效率进阶 | 中 | 3 人天` ｜ 前置 `ADD-FEAT-P0.02.05` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-ui/ui/status_indicator.slint`（新增）、`crates/ime-ui/src/ui_thread/indicator.rs`（新增）、`crates/ime-fcitx5/src/ui_impl/indicator.rs`（新增）、`crates/ime-config/src/schema.rs`（改：`[ui] status_indicator`）
- **目标与价值**：对标搜狗的悬浮状态条、微软拼音的任务栏指示、macOS 的菜单栏图标。**当前的问题**：`StatusStrip` 只存在于候选框的 header 内——**候选框隐藏时用户完全不知道**当前是中文还是英文、全角还是半角、是否处于只读模式、简繁状态。**注意**：ADR-0000 的 `OB-1` 说输入法"无常驻界面"——那指的是**关于对话框与启动画面**，不禁止一个输入状态指示器。本卡需在 `licenses.md` 中确认这一点。
- **技术设计**：
  - **独立的轻量 surface**（不是候选框的一部分）：尺寸固定（约 `120×24` dp），只在状态**变化时**重绘（`idle_redraw_count = 0` 的预算约束）。
  - **显示内容**：`中/EN`、`全/半`、`简/繁`、`只读锁`。**不显示**任何用户输入内容。
  - **位置**：默认右下角，可配置为 `off` / `bottom-right` / `bottom-left` / `top-right` / `top-left`；可拖动并记忆位置。
  - **可完全关闭**：`[ui] status_indicator = "off"` 是默认值——**常驻 UI 是侵入性的**，不该默认开启。
  - **`idle_redraw_count = 0` 的兼容**：指示器在**状态未变化时零重绘**；状态变化时才重绘一次。`poll` 超时仍为 `-1`（无轮询定时器）。
- **NFR**：`idle_redraw_count = 0`、`idle_poll_timer_count = 0`（既有预算，**不得违反**）；指示器 surface 常驻内存 ≤ 1.5MB（计入 `ui_rss = 18MB`）；**绝不夺焦**（0.4 规则 5）；默认关闭。
- **实施步骤**：① `status_indicator.slint`（四个状态徽标 + 紧凑布局）；② `indicator.rs` 的 surface 生命周期（预创建、状态变化时重绘）；③ 位置配置与拖动记忆；④ 只读模式与简繁状态的绑定；⑤ 默认关闭 + 开启时的预算验证。
- **DoD**：① 开启后状态变化即时反映；关闭后零常驻 surface。[实验室] ② `idle_redraw_count = 0`、`idle_poll_timer_count = 0` 仍成立。[性能] ③ 指示器不夺焦。[实验室] ④ 常驻内存 ≤ 1.5MB。[性能]

---

#### 任务 ID：ADD-FEAT-P1.02.07 per-app Profile（按应用切换行为）

- **基本属性**：绑定差距条目：`features.md` 的 `TASK-2.04.02`（**不在主文档 5.1 的 44 行绑定范围内**）｜ `P1 效率进阶 | 中 | 3 人天` ｜ 前置 `ADD-FEAT-P1.02.02` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-config/src/schema.rs`（改：`[profile]` 段）、`crates/ime-fcitx5/src/engine.rs`（改：按 `AppIdHash` 选配置）、`crates/ime-core/src/privacy.rs`（只读复用：`AppIdHash`）
- **目标与价值**：对标 `features.md` 的 `R-10`（特定应用兼容问题）与搜狗/微软的"按应用设置"。三个真实场景：(1) 在终端里关闭自绘 UI（`vim` 的 insert 模式位置错乱）；(2) 在 IDE 里强制英文（写代码时不想触发中文）；(3) 在游戏里完全透传。
- **技术设计**：
  - **按应用哈希索引**（复用既有 `AppIdHash`，**不存明文应用名**——`AGENTS.md` 3.4 的隐私要求）：

    ```toml
    [[profile]]
    # The application identifier is matched case-insensitively as a substring
    # and stored only as a hash. The profile is selected at session start and
    # never changes mid-session, because a mid-session switch would silently
    # change what the keys do.
    app = "code"
    scheme = "full"
    script = false
    self_drawn_ui = true

    [[profile]]
    app = "terminal"
    self_drawn_ui = false    # fall back to fcitx5's ClassicUI
    ```
  - **配置继承**：`[profile]` 只覆盖它显式写出的键，未写的键从全局配置继承。这避免了"每个 profile 都要写全量配置"。
  - **per-app 关闭自绘 UI** 是 `features.md` 的 `R-10` 对策：关闭后候选框由 fcitx5 的 ClassicUI 绘制，功能完整、外观不同——**这是降级不是失败**。
- **NFR**：profile 选择在**会话开始时**完成，**不在**每次按键时（避免热路径查表）；`app` 的匹配用 `AppIdHash`，**日志中不出现明文应用名**；profile 数量 ≤ 16。
- **实施步骤**：① `[profile]` 的 schema 与继承规则；② `AppIdHash` 的匹配；③ 会话开始时选 profile；④ `self_drawn_ui = false` 时通知 fcitx5 使用 ClassicUI（复用 ADR-0003 的双 cdylib 架构——UI cdylib 不注册即可）；⑤ 隐私复核（日志无明文应用名）。
- **DoD**：① 配置 `app = "code"` 的 profile 后，在 VS Code 中生效、在别处不生效。[实验室] ② `self_drawn_ui = false` 时候选框由 ClassicUI 绘制且输入功能完整。[实验室] ③ 日志中不含明文应用名。[自动] ④ profile 切换**不重置**进行中的会话。[自动]

---

### 1.3 候选框外观与形态（Track B）

#### 任务 ID：ADD-FEAT-P1.03.01 候选框外观深度自定义

- **基本属性**：绑定差距条目：`GAP-19` ｜ `P1 效率进阶 | 中 | 5 人天` ｜ 前置：无 ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-ui/ui/candidate.slint`（改：`CandidateMetrics` 从编译期常量改为运行期绑定）、`crates/ime-ui/src/layout/metrics.rs`（改：度量从配置派生）、`crates/ime-config/src/schema.rs`（改：`[ui]` 追加 6 键）、`crates/ime-ui/src/theme.rs`（改：Token 随密度变化）
- **目标与价值**：对标搜狗的皮肤字号设置、微软拼音的主题/字号/密度设置、macOS 的候选条形态。当前 `UiConfig` 只有 7 个键、`ThemeConfig` 只有 2 个键，**`.slint` 的 `CandidateMetrics` 全部是编译期常量**——用户无法调整字号、密度、间距。
- **技术设计**：
  - **`CandidateMetrics` 从常量改为派生值**。这是本卡的核心改动：`.slint` 的 `out property <length>` 改为 `in-out property`，由 Rust 侧在配置加载时写入：

    ```rust
    /// The geometry the candidate window is laid out with.
    ///
    /// Derived from the configuration rather than declared in `.slint`: the
    /// design document's constants are the *defaults* of this struct, and a
    /// user who changes the font size changes every measurement that depends
    /// on it. Keeping the derivation in Rust is what lets the metrics be
    /// tested without rendering.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct CandidateMetrics {
        /// Cell font size, in dp.
        pub font_size_cell: u16,
        /// Header font size, in dp.
        pub font_size_header: u16,
        /// Annotation and hint font size, in dp.
        pub font_size_small: u16,
        /// Cell height, derived from the font size and the density.
        pub cell_height: u16,
        /// Horizontal padding inside one cell.
        pub cell_padding_h: u16,
        /// Vertical padding inside one cell.
        pub cell_padding_v: u16,
        /// Gap between cells.
        pub cell_gap: u16,
        /// Container corner radius.
        pub container_radius: u16,
        /// Container padding.
        pub container_padding: u16,
        /// Header height.
        pub header_height: u16,
        /// Shadow blur radius, in dp.
        pub shadow_blur: u16,
        /// Shadow vertical offset, in dp.
        pub shadow_offset_y: u16,
        /// Shadow reserved margin, in dp.
        pub shadow_margin: u16,
    }

    /// How tightly the cells are packed.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
    pub enum Density {
        /// 0.875x cell height, 4dp gap.
        Compact,
        /// 1.0x cell height, 6dp gap.
        #[default]
        Normal,
        /// 1.125x cell height, 8dp gap.
        Relaxed,
    }

    impl CandidateMetrics {
        /// Derives every measurement from the configurable subset.
        ///
        /// Only four values are configurable — the three font sizes and the
        /// density — and everything else follows from them. Exposing every
        /// field would let a user produce a candidate window whose parts do
        /// not fit together.
        ///
        /// # Errors
        /// Returns `ConfigError::Invalid` when a font size falls outside
        /// `11..=22`; the caller repairs rather than refusing to start.
        pub fn derive(cfg: &UiConfig) -> Result<Self, ConfigError>;
    }
    ```
  - **可配置项**（`ASM-A-10` 的键预算内）：`font_size_dp`（11..=22，默认 15）、`font_size_header_dp`（默认 14）、`density`（compact/normal/relaxed）、`cell_gap_dp`（0..=16）、`corner_radius_dp`（既有，0..=24）、`max_width_dp`（既有）。
  - **4dp 网格约束**（主文档 `C-2`）：`derive` 的输出必须全部落在 4dp 网格上（向上取整），**不得**产生 13px 这类非标值。
  - **字阶约束**：三个字号必须满足 `font_size_header ≤ font_size_cell` 且 `font_size_small ≤ font_size_header`，违反时 `derive` 报 `ConfigError::Invalid`。
- **NFR**：`derive` 在配置加载时执行，不在渲染路径；度量变化**不得**破坏 `hit_map` 与实际布局的一致性（`features.md` 6.2.2 的"点错候选"陷阱）——`hit_map` 必须用同一份 `CandidateMetrics`；**绝不夺焦**；**不得**引入新颜色。
- **实施步骤**：① 实现 `CandidateMetrics::derive` 与 `Density`；② `.slint` 的 `CandidateMetrics` 改为 `in-out property`，由 Rust 侧注入；③ `hit_map` 改为消费同一份度量；④ 配置项与校验（范围 + 字阶序关系）；⑤ 视觉走查三档密度 × 四档字号的组合。
- **DoD**：① 三档密度、四档字号的候选框视觉走查通过。[视觉] ② 点击第 N 个候选上屏第 N 个（`hit_map` 一致性），在三档密度下各验证一次。[自动] ③ `cargo nextest run -p ime-ui` 全绿，新增 ≥ 10 条（`derive` 的范围、网格对齐、字阶序、非法值修复）。[自动] ④ 渲染耗时 ≤ `raster_p99 = 1.5ms` 不劣化。[性能]

---

#### 任务 ID：ADD-FEAT-P1.03.02 竖排单列候选

- **基本属性**：绑定差距条目：`GAP-20` ｜ `P1 效率进阶 | 中 | 3 人天` ｜ 前置 `ADD-FEAT-P1.03.01` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-ui/ui/candidate.slint`（改：排布方向）、`crates/ime-ui/src/layout.rs`（改：竖排布局）、`crates/ime-ui/src/geometry/placement.rs`（改：竖排的避让计算）、`crates/ime-config/src/schema.rs`（改：`[ui] orientation`）
- **目标与价值**：对标搜狗的横排/竖排切换、微软拼音的横竖排、macOS 的竖排候选条、RIME 的 `vertical` 选项。当前 `max_per_row` 的合法区间是 **`3..=9`**（`schema.rs:314` 的注释；测试 `ui.max_per_row = 2` 被 `validate()` 拒绝）——**用户无法配置为 1 列**。竖排在终端、窄窗口、以及习惯竖排的用户那里是刚需。
- **技术设计**：
  - **`max_per_row` 的下限从 3 放宽到 1**，并新增 `orientation` 键作为**语义化入口**（`grid` / `vertical`）：

    ```rust
    /// How candidates are arranged.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
    pub enum Orientation {
        /// A grid, `max_per_row` wide.
        #[default]
        Grid,
        /// A single column, one candidate per row.
        Vertical,
    }

    impl Orientation {
        /// The effective candidates per row for this orientation.
        ///
        /// `Vertical` always answers 1 regardless of `max_per_row`, so a user
        /// who switches to vertical and back finds their grid width intact.
        ///
        /// # Errors
        /// This function is infallible: it returns no `Result`.
        pub const fn per_row(self, configured: u8) -> u8 {
            match self {
                Orientation::Grid => configured,
                Orientation::Vertical => 1,
            }
        }
    }
    ```
  - **`max_per_row` 下限改为 1**（`schema.rs` 的 `validate`/`repaired` 边界同步），但**语义上**竖排由 `orientation` 控制，`max_per_row` 只在 `grid` 下生效。
  - **避让计算必须改**：竖排时候选框又高又窄，`placement.rs` 的底部翻转判定必须用竖排后的高度——这是本卡最容易出错的地方。
  - **宽度约束**：竖排时 `max_width_dp` 不再有意义，改用 `min_width = 220dp` 作为唯一宽度；超过最长候选宽度时按 `max_width_dp` 截断（沿用既有的截断规则，`text` 与 `display-text` 分离）。
- **NFR**：竖排下的 `raster_p99 ≤ 1.5ms`（竖排的单元格数更多，是渲染的最坏情况）；`hit_map` 与竖排布局一致；避让计算在竖排下不得把候选框推出屏幕；**绝不夺焦**。
- **实施步骤**：① `Orientation` 与 `per_row`；② `.slint` 的排布改为按 `orientation` 分支（`GridLayout` 的列数动态）；③ `layout.rs` 的竖排尺寸计算；④ `placement.rs` 的避让用竖排高度；⑤ `max_per_row` 下限改为 1 并同步既有测试。
- **DoD**：① 竖排候选在 X11 与 Wayland/wlroots 两档上可用。[实验室] ② 屏幕底部时竖排候选框正确翻转到光标上方。[实验室] ③ `hit_map` 一致性在竖排下验证通过。[自动] ④ 竖排的 `raster_p99 ≤ 1.5ms`。[性能] ⑤ 切回 `grid` 后 `max_per_row` 的配置值未被破坏。[自动]

---

#### 任务 ID：ADD-FEAT-P1.03.03 主题 Token 扩展与 `state-disabled` 补齐

- **基本属性**：绑定差距条目：`GAP-19` ｜ `P1 效率进阶 | 中 | 3 人天` ｜ 前置 `ADD-FEAT-P1.03.01` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-ui/src/theme.rs`（改：`ThemeTokens` 追加 `state_disabled`）、`crates/ime-ui/src/theme/scheme.rs`（改：深浅两套的值）、`crates/ime-ui/ui/theme.slint`（改：追加 `state-disabled`）、`crates/ime-ui/src/theme/color.rs`（只读复用：`contrast_ratio`）
- **目标与价值**：**补齐一个已存在的缺口**。既有 `Theme` global 有 `state-hover` / `state-selected-bg` / `state-selected-stroke` / `state-pressed` **四个状态**，**但没有 `state-disabled`**——而主文档 `C-3` 要求凡新增控件必须补齐 5 态。`P1.02.02`（设置项）、`P1.02.03`（键位表）、`P1.02.05`（词库管理）三张卡**全部依赖本卡**。
- **技术设计**：
  - **追加 `state_disabled`**（`ThemeTokens` 的字段 + `.slint` 的 `state-disabled`）：

    ```rust
    /// The colour a disabled control's content is drawn in.
    ///
    /// Deliberately *not* a plain alpha reduction of `text_primary`: a
    /// disabled control must still be legible enough to tell what it would do
    /// if it were enabled, while being unmistakably not interactive. The value
    /// is the one the contrast self-check accepts as the floor, which keeps a
    /// disabled label readable rather than decorative.
    pub state_disabled: Rgba8,
    ```
  - **对比度自检必须覆盖 `state_disabled`**：既有的 `CONTRAST_MINIMUM = 4.5` 是对"文本/背景对"的要求。禁用态文本**不适用** 4.5（它会不达标，这是设计意图），但**必须** ≥ 3.0（可辨识下限）。新增一个常量：

    ```rust
    /// The contrast a disabled label must still reach against the surface.
    ///
    /// Lower than `CONTRAST_MINIMUM` because a disabled label is meant to read
    /// as unavailable, not as body text. It is not zero: a control the user
    /// cannot read is a control they cannot reason about.
    pub const CONTRAST_DISABLED_MINIMUM: f32 = 3.0;
    ```
  - **深浅两套的值**必须都过 `CONTRAST_DISABLED_MINIMUM`，由既有的 `contrast_ratio` 在测试中断言（**不新增对比度实现**）。
  - **不引入新颜色族**：`state_disabled` 从既有 `text_primary` 与 `surface_base` 的混合派生，保持与既有 token 的色相一致。
- **NFR**：对比度 ≥ 3.0（深浅两套）；`ThemeTokens` 新增字段后 `UiFrame` 的尺寸预算不受影响（`ThemeTokens` 不在 `UiFrame` 内，走 `UiCommand::Theme`）；**不得**改变既有 22 个 token 的值（会破坏既有视觉基线）。
- **实施步骤**：① `ThemeTokens` 追加 `state_disabled`；② 深浅两套的取值（从 `text_primary` × `surface_base` 派生）；③ `CONTRAST_DISABLED_MINIMUM` 与断言测试；④ `.slint` 的 `Theme` global 追加 `state-disabled`；⑤ 在 `P1.02.02` 的设置项上验证 5 态完整。
- **DoD**：① 深浅两套的 `state_disabled` 对比度均 ≥ 3.0。[自动] ② 既有 22 个 token 的值未变（逐 token 断言）。[自动] ③ 设置项控件的 5 态在深浅两套下视觉走查通过。[视觉] ④ `cargo nextest run -p ime-ui` 全绿。[自动]

---

#### 任务 ID：ADD-FEAT-P1.03.04 候选框位置策略（固定 / 记忆 / 跟随）

- **基本属性**：绑定差距条目：`GAP-21` ｜ `P1 效率进阶 | 中 | 3 人天` ｜ 前置 `ADD-FEAT-P1.03.01` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-ui/src/geometry/placement.rs`（改：位置策略）、`crates/ime-fcitx5/src/cursor/mod.rs`（改：位置记忆的持久化）、`crates/ime-config/src/schema.rs`（改：`[ui] placement`）、`crates/ime-types/src/ui.rs`（**需 ADR-0004 扩展**：`Placement` 追加 `Fixed` / `Remember`）
- **目标与价值**：对标搜狗的"跟随光标 / 固定位置 / 记忆位置"、微软拼音的跟随/固定。当前 `Placement` 只有 `Below` / `Above` / `Auto` **三个值**。三个真实场景：(1) 某些应用的光标坐标不可靠（`features.md` 的 `R-03`），固定位置是兜底；(2) 用户习惯候选框总在同一个地方；(3) 终端里跟随光标会遮挡。
- **技术设计**：
  - **`Placement` 追加两个变体**（**必须走 ADR-0004**，`Placement` 是 `ADR-0001` 冻结的类型）：

    ```rust
    pub enum Placement {
        /// Below the cursor.
        Below,
        /// Above the cursor.
        Above,
        /// Below unless there is no room, then above.
        Auto,
        /// A fixed screen position, ignoring the cursor.
        ///
        /// The escape hatch for the applications whose cursor rectangle is
        /// unreliable (`features.md` R-03): a window that is always in the
        /// same place is worse than one that follows the cursor, and much
        /// better than one in the wrong place.
        Fixed,
        /// The position this application was last seen at.
        ///
        /// Remembered per application, keyed by `AppIdHash`, so the window
        /// does not jump around when the user switches between an editor and
        /// a chat client.
        Remember,
    }
    ```
  - **位置记忆的存储**：`$XDG_DATA_HOME/rspinyin/ui_positions.json`（`0600`），键为 `AppIdHash` 的十六进制，值为 `(x, y)`。**不存明文应用名**（隐私）。
  - **`Fixed` 的位置来源**：`[ui] fixed_x` / `[ui] fixed_y`（dp，相对屏幕左上角）；未配置时用"屏幕下 1/3 居中"（`features.md` 的 `R-03` 兜底位置）。
  - **记忆的写入时机**：用户拖动候选框时（若 `P1.02.06` 的状态指示器有拖动能力，候选框本身也可拖动）→ 记一次；**不在**每次显示时写（避免频繁 IO）。
- **NFR**：位置计算在 UI 线程，**不在**宿主线程（`features.md` 2.1）；`ui_positions.json` 的写入在空闲期；位置计算 ≤ 0.1ms；**绝不夺焦**；**不得**把候选框推出屏幕（既有避让逻辑在 `Fixed`/`Remember` 下仍须生效——记忆的位置若已超出当前屏幕分辨率，回退到 `Auto`）。
- **实施步骤**：① **先落 ADR-0004 的 `Placement` 扩展**；② `placement.rs` 的五种策略实现；③ `ui_positions.json` 的读写（`0600` + 原子写）；④ `[ui] placement` / `fixed_x` / `fixed_y` 配置；⑤ 边界：屏幕分辨率变化、记忆位置超界、多屏切换。
- **DoD**：① 五种策略全部生效。[实验室] ② 记忆位置按应用分离且**不含明文应用名**。[自动] ③ 记忆位置超界时回退到 `Auto`。[自动] ④ 多屏切换后候选框仍在屏幕内。[实验室] ⑤ `cargo nextest run -p ime-ui` 全绿，新增 ≥ 8 条。[自动]

---

### 1.4 数据主权与外部接口（Track C）

#### 任务 ID：ADD-FEAT-P1.04.01 配置导入 / 导出 / 一键重置

- **基本属性**：绑定差距条目：`GAP-28` ｜ `P1 效率进阶 | 低 | 3 人天` ｜ 前置 `ADD-FEAT-P0.03.02` ｜ `CP: 否` ｜ `Track C` ｜ `[ ] 待开始`
- **代码落地锚点**：`xtask/src/config_io.rs`（新增）、`crates/ime-config/src/reload.rs`（改：导出/导入入口）、`crates/ime-ui/src/command/settings.rs`（改：面板内的导出/导入/重置按钮）
- **目标与价值**：对标 VS Code 的设置同步与导出。用户换机器、重装系统、或把配置分享给同事时**当前无路可走**——只能手工复制 `config.toml`，而不知道 `phrases.tsv` / `ui_positions.json` 也要带。
- **技术设计**：
  - **导出的是一个目录/归档，不是单个文件**：配置的完整状态分布在 4 个文件（`config.toml`、`phrases.tsv`、`ui_positions.json`、可选的 `[data] user_tables` 指向的文件）。**只导出 `config.toml` 是错的**——用户会以为带全了。
  - **导出格式**：`rspinyin-config-<日期>.tar.gz`（含 `manifest.json` 记录版本与文件清单）。**需评审新增依赖**：`tar` + `flate2`（`AGENTS.md` 3.5 要求新依赖先评审；两者均为宽松许可且无网络能力，但**必须**在 `licenses.md` 登记）。若评审不通过，退化为"复制到一个目录"（无压缩，零新依赖）。
  - **导入的版本校验**：`manifest.json` 的 `schema_version` 低于当前时**先迁移再导入**（复用 `P0.03.02`）；高于当前时**拒绝并说明**（不猜测未来格式）。
  - **一键重置**：把 `config.toml` 重命名为 `config.toml.bak.<ts>` 后写默认值。**绝不删除**用户的配置。
- **NFR**：导出/导入是显式动作，不在热路径；导入前**先备份现有配置**；导入失败时**原配置不变**（原子替换）；**不引入网络能力**（`BUDGET-NET-01 = 0`）；新依赖（若采用压缩）必须过 `cargo deny` 与 `licenses.md` 登记。
- **实施步骤**：① 决定导出格式（压缩 vs 目录）并评审依赖；② `manifest.json` 的结构与版本字段；③ 导出/导入实现（原子替换 + 先备份）；④ 面板内的三个按钮；⑤ 版本校验与迁移衔接。
- **DoD**：① 导出后在新环境导入，配置逐键一致。[自动] ② 导入低版本配置时自动迁移。[自动] ③ 导入失败时原配置不变。[自动] ④ 一键重置后原配置以 `.bak` 保留。[自动] ⑤ 若新增压缩依赖，已在 `licenses.md` 登记且 `cargo deny` 通过。[自动]

---

#### 任务 ID：ADD-FEAT-P1.04.02 用户数据的完全可删除

- **基本属性**：绑定差距条目：`GAP-30` ｜ `P1 效率进阶 | 低 | 2 人天` ｜ 前置 `ADD-FEAT-P0.03.01` ｜ `CP: 否` ｜ `Track A-数据 ｜ Track C` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-dict/src/paths.rs`（改：`purge` 入口）、`crates/ime-ui/src/command/user_words.rs`（改：删除入口）、`docs/dev/privacy.md`（改：说明删除语义）
- **目标与价值**：对标 Obsidian 的"数据在用户目录，可直接删"与 RIME 的"词表即文件"。用户当前**可以直接删除 `user.redb`**，但**没有程序内入口**，且删除后无提示、无确认、无"删除后行为如何"的说明。**这是用户对自己数据的控制权**。
- **技术设计**：
  - **三档删除**（粒度是关键——"全删"太粗暴，"删一条"已经有了）：
    | 档位 | 删除对象 | 保留 |
    |---|---|---|
    | **清空学习** | `user.redb` 的全部学习词条 | 用户词表（`WordFlags::USER`）、固定词、短语 |
    | **清空用户数据** | `user.redb` + `backups/` | `config.toml`、`phrases.tsv` |
    | **完全重置** | 全部用户数据（配置保留为默认值） | 无 |
  - **删除前必须确认**（UI 层）：显示将被删除的条目数与文件大小，要求二次确认。
  - **删除是隔离不是抹除**（复用 `recover.rs` 的同族语义）：文件重命名为 `<name>.deleted.<ts>` 保留 7 天，之后由空闲期清理。**理由**：误删的代价是全部学习成果，而"隔离 7 天"的成本只是几十 KB。
- **NFR**：删除是显式动作；**绝不**删除 `config.toml`（用户的配置是他们的劳动）；隔离文件 7 天后清理（复用既有的空闲期窗口）；删除后**不重启即可生效**（重新打开用户库）。
- **实施步骤**：① 三档删除的语义与实现；② 隔离重命名 + 7 天清理；③ 面板内的确认对话框（含条目数与大小）；④ `privacy.md` 的删除语义说明；⑤ 边界：只读模式下的删除（返回 `Readonly`，不 panic）。
- **DoD**：① 三档删除各自生效，未列入的对象保留。[自动] ② 删除前显示条目数与大小。[实验室] ③ 只读模式下删除返回 `Readonly` 且不 panic。[自动] ④ `privacy.md` 说明三档语义与隔离期。[文档]

---

#### 任务 ID：ADD-FEAT-P1.04.03 D-Bus 只读控制接口（`Status` / `Capabilities`）

- **基本属性**：绑定差距条目：`GAP-31` ｜ `P1 效率进阶 | 中 | 4 人天` ｜ 前置 `ADD-FEAT-P1.05.02` ｜ `CP: 否` ｜ `Track C` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-fcitx5/src/dbus/mod.rs`（新增）、`crates/ime-fcitx5/src/dbus/introspect.rs`（新增）、`crates/ime-fcitx5/Cargo.toml`（改：**新增 `zbus` 依赖，需评审**）、`docs/dev/architecture.md`（新增：接口文档）
- **目标与价值**：对标搜狗的 D-Bus 接口、RIME 的 `rime_api`、Fcitx5 自身的 D-Bus。让外部程序能**查询** rspinyin 的状态：IDE 插件想知道"现在是不是中文模式"、脚本想在自动化测试里断言输入法状态、桌面环境想在切换工作区时知道输入法状态。**`ASM-A-16` 明确只读优先**——写方法（切换模式）在第二批，因为"外部程序能否改变用户输入状态"有隐私含义。
- **技术设计**：
  - **接口定义**（只读）：
    ```
    org.rspinyin.Control
      method  Status() -> a{sv}
          schema_version: u16
          mode:           s   ("zh" | "en")
          script:         s   ("simplified" | "traditional")
          full_width:     b
          punct_full:     b
          scheme:         s   ("full" | "xiaohe" | ...)
          readonly:       b
          self_drawn_ui:  b

      method  Capabilities() -> a{sv}
          wayland_tier:   s   ("layer-shell" | "popup" | "canvas" | "fallback" | "x11" | "unknown")
          blur:           b
          dict_entries:   u
          dict_version:   u
          script_table:   b
          symbol_table:   b
          engine_glossary: b
    ```
  - **接口不含任何用户数据**：`Status` 与 `Capabilities` **不暴露**候选内容、输入内容、用户词条数（那会泄露用户的学习规模）。**这是硬约束**，必须在接口文档里写明。
  - **依赖评审**：`zbus` 是新增依赖，**必须**过 `AGENTS.md` 3.5 的评审。**关键检查**：`zbus` 的默认 feature 是否拉入网络能力（D-Bus 是本地 IPC，不是网络，但必须确认 `zbus` 不启用 `tokio`/TCP 传输）。若 `zbus` 无法在不引入网络能力的前提下使用，**退化方案**是直接用 `libdbus` 的 C 绑定（但那会引入 `unsafe`，违反 `ASM-A-03` 的 A-1）——因此**首选 `zbus` 并显式关闭其网络 feature**。
  - **`crates/ime-fcitx5` 的 `unsafe` 白名单不变**：D-Bus 走 `zbus` 的安全 API，**不新增 `unsafe`**（`A-1`）。
- **NFR**：D-Bus 调用 ≤ 5ms；**接口不暴露任何用户数据**；`BUDGET-NET-01 = 0` 不受影响（D-Bus 是本地 socket，`scripts/runtime-socket-check.sh` 需确认其判定口径覆盖 D-Bus 时**不误报**——**这是本卡的一个必须处理的点**）；D-Bus 线程**不阻塞宿主线程**（`ASM-A-18` 的"不引入新线程"在此处需要一次 ADR 级评审，因为 D-Bus 需要事件循环——**建议复用 fcitx5 自身的事件循环而非新起线程**）。
- **实施步骤**：① **先评审 `zbus`**（许可、网络 feature、`cargo deny`）；② 确认 D-Bus 的接入方式（复用 fcitx5 事件循环 vs 新线程）并**必要时开 ADR**；③ 接口实现与 introspection XML；④ `scripts/runtime-socket-check.sh` 的口径复核（D-Bus 是否被误判为网络 socket）；⑤ 接口文档。
- **DoD**：① `busctl --user introspect org.rspinyin / org.rspinyin.Control` 可见两个方法。[实验室] ② `Status()` 的返回值与实际状态一致。[实验室] ③ 接口**不含任何用户数据**（逐字段复核）。[文档] ④ `bash scripts/check-no-network.sh` 与 `runtime-socket-check.sh` 均通过。[自动] ⑤ `zbus` 已在 `licenses.md` 登记且 `cargo deny` 通过。[自动] ⑥ 新增依赖**未引入网络能力**。[自动]

---

### 1.5 交付与诊断（Track C）

#### 任务 ID：ADD-FEAT-P1.05.01 配置校验反馈的用户可达性

- **基本属性**：绑定差距条目：`GAP-43` ｜ `P1 效率进阶 | 低 | 2 人天` ｜ 前置 `ADD-FEAT-P0.03.02` ｜ `CP: 否` ｜ `Track C` ｜ `[ ] 待开始`
- **代码落地锚点**：`xtask/src/main.rs`（改：新增 `check-config` 子命令）、`crates/ime-config/src/reload.rs`（改：报告的结构化输出）、`crates/ime-ui/src/command/settings.rs`（改：面板内展示校验报告）
- **目标与价值**：对标 VS Code 的设置内联报错、RIME 的 `rime_deployer` 输出。当前 `Config::validate()` 返回 `Vec<ImeError>`、`Config::repaired()` 返回修复报告——**但只进日志**。用户手改 TOML 打错一个键，只能翻日志文件才知道被忽略了。**这是"改了没反应"的经典体验问题。**
- **技术设计**：
  - **`rspinyin --check-config` 命令**：输出人类可读的校验报告，含每一处问题、所在键、原因、以及**修复后的值**（`repaired()` 已经算出）。
  - **报告格式**（既是 CLI 输出也是面板的数据源）：
    ```
    config.toml — 3 issues

      [ui] max_per_row = 2
        invalid: value 2 is below the minimum 3
        repaired to: 3

      [keys] flip_keys = ["comma", "comma"]
        invalid: "comma" appears twice
        repaired to: ["comma"]

      [theme] accent = "#GGGGGG"
        invalid: not a hex colour
        repaired to: "#0A6CFF"

    Result: 0 fatal, 3 repaired. The plugin starts with the repaired values.
    ```
  - **面板内展示**：命令面板的"设置"页顶部显示一行提示（"3 处配置问题，点击查看"），点开显示同一份报告。**这是"改了没反应"问题的正解**——用户不需要知道日志文件在哪。
- **NFR**：校验是启动期动作，不在热路径；`--check-config` **不修改**配置文件（只报告）；报告**不含**任何用户数据。
- **实施步骤**：① 把 `validate()`/`repaired()` 的 `Vec<ImeError>` 结构化为 `ConfigIssue`（含键、原因、修复值）；② `xtask check-config` 子命令；③ 面板内的报告展示；④ 启动时若有问题则记一条 `warn` 并在面板内可见。
- **DoD**：① `xtask check-config` 对含 3 处问题的配置输出上述格式。[自动] ② `--check-config` 不修改配置文件（`sha256` 前后一致）。[自动] ③ 面板内可见校验报告。[实验室] ④ `cargo nextest run -p ime-config` 全绿。[自动]

---

#### 任务 ID：ADD-FEAT-P1.05.02 运行时能力状态的可读输出

- **基本属性**：绑定差距条目：`GAP-44` ｜ `P1 效率进阶 | 低 | 2 人天` ｜ 前置：无 ｜ `CP: 否` ｜ `Track C` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-fcitx5/src/capability.rs`（新增）、`crates/ime-ui/src/platform/wayland/probe.rs`（只读复用：档位探测）、`xtask/src/main.rs`（改：`capabilities` 子命令）、`docs/dev/capabilities.md`（新增）
- **目标与价值**：`features.md` 0.5.2 有 4 档平台能力矩阵，但**用户看不到当前环境支持什么**。用户遇到"候选框没出现"时，无法自助判断是 Wayland 档位降级了、还是模糊不可用、还是只读模式。**这是把 `features.md` 0.5.2 的矩阵变成用户可见的输出。**
- **技术设计**：
  - **能力探测复用既有的探测代码**：`crates/ime-ui/src/platform/wayland/probe.rs` 已有档位探测（`layer_shell` / `popup` / `canvas` / 兜底）；`theme.rs` 已有 `BlurNegotiation`。本卡把它们聚合成一个可输出的结构：

    ```rust
    /// What this environment actually supports, as measured at runtime.
    ///
    /// Every field is the result of a probe that already exists; this type
    /// only collects them so a user can see the same matrix the design
    /// document describes, filled in for their machine.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Capabilities {
        /// Which window backend the candidate window got.
        pub backend: BackendKind,
        /// Whether the compositor accepted the blur request.
        pub blur: bool,
        /// Whether the user data directory is writable.
        pub writable: bool,
        /// Whether the dictionary loaded, and how many entries it holds.
        pub dict: Option<DictInfo>,
        /// Whether the script table loaded.
        pub script_table: bool,
        /// Whether the symbol table loaded.
        pub symbol_table: bool,
        /// The Fcitx5 version the plugin is running against.
        pub fcitx5_version: String,
    }

    /// The window backend the candidate window is using.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum BackendKind {
        /// Wayland with `wlr-layer-shell`.
        LayerShell,
        /// Wayland with `xdg-popup`.
        Popup,
        /// Wayland with the canvas-popup fallback.
        Canvas,
        /// Wayland with no positioning protocol; the window is placed by the
        /// compositor.
        WaylandFallback,
        /// X11 with an ARGB visual.
        X11,
        /// No window backend: fcitx5's ClassicUI draws the candidates.
        ClassicUi,
    }
    ```
  - **输出形式**：`rspinyin --capabilities`（CLI）+ 命令面板的"能力状态"页 + 状态指示器的 tooltip。
- **NFR**：探测**不阻塞**宿主线程（复用既有的探测路径，它们在初始化期已完成）；`Capabilities` **不含**任何用户数据（`dict_entries` 是词库规模，不是用户词条数）；输出人类可读且可机器解析（`--json`）。
- **实施步骤**：① 聚合既有探测结果为 `Capabilities`；② `BackendKind` 的映射（从 `probe.rs` 的档位）；③ CLI 输出（人类可读 + `--json`）；④ 面板内的展示；⑤ `docs/dev/capabilities.md` 说明每一档的含义与降级影响。
- **DoD**：① `rspinyin --capabilities` 输出与实际环境一致（在 X11 与 Wayland 两档各验证一次）。[实验室] ② `--json` 输出可被 `jq` 解析。[自动] ③ `docs/dev/capabilities.md` 逐档说明含义。[文档] ④ 输出不含用户数据。[文档]

---

#### 任务 ID：ADD-FEAT-P1.05.03 无障碍语义暴露评估与实现

- **基本属性**：绑定差距条目：`GAP-39` ｜ `P1 效率进阶 | 高 | 5 人天` ｜ 前置 `ADD-FEAT-P0.02.05` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`docs/dev/a11y.md`（新增）、`crates/ime-fcitx5/src/a11y/mod.rs`（新增）、`crates/ime-ui/src/ui_thread/a11y.rs`（新增）
- **目标与价值**：对标 macOS 原生拼音（VoiceOver 可读候选）、微软拼音（Narrator 可读）。**当前候选框是全自绘 surface，对 AT-SPI 完全不可见**——屏幕阅读器用户看不到候选列表。`features.md` 的 `TASK-2.05.06` 只覆盖"高对比主题可用，对比度 ≥ 7:1"——那是**视觉**可达性，**不是语义**可达性。
- **技术设计**：
  - **关键约束（`ASM-A-12`）**：候选框**绝不能夺取键盘焦点**（`features.md` 0.4 规则 5，项目的最高严重级缺陷）。因此**候选框本身不能成为 AT-SPI 的可聚焦对象**。这决定了可行的技术路径：
    | 路径 | 机制 | 可行性 | 局限 |
    |---|---|---|---|
    | **`client_preedit`** | 把 preedit 交给应用自身的输入框（`UiConfig.client_preedit` **已存在但未见消费方**），应用的无障碍树自然包含它 | **高** | 只暴露 preedit，不暴露候选列表 |
    | **只读 AT-SPI 旁路** | 候选框注册为**不可聚焦**的 AT-SPI 对象，屏幕阅读器可读但不接收焦点 | 中 | 需验证不夺焦；部分屏幕阅读器只读可聚焦对象 |
    | **应用侧反馈** | 通过 fcitx5 把候选数/首候选写进应用的 `InputContext` 状态 | 中 | 依赖应用暴露该状态 |
  - **本卡先做评估**：在真实的 Orca（GNOME 屏幕阅读器）环境下验证三条路径，产出 `docs/dev/a11y.md` 的结论与选型。**评估结论决定 `P1.05.04` 的实现范围**。
  - **无论选哪条路径，`client_preedit` 都应默认开启**——它是零风险的、不依赖任何无障碍基础设施的改进。
- **NFR**：**绝不夺焦**（这是硬约束，任何方案违反即否决）；`client_preedit` 开启后 `key_to_present_p99` 不劣化（preedit 通过 fcitx5 的既有通道，无新增延迟）；评估结论必须**如实记录局限**，不承诺做不到的事。
- **实施步骤**：① 搭建 Orca 测试环境；② 逐条验证三条路径（能否读到 preedit、能否读到候选、是否夺焦）；③ 产出 `docs/dev/a11y.md`（含验证方法与结论）；④ 实现 `client_preedit` 路径（若既有代码未消费该配置项，本卡补上）；⑤ 按评估结论决定是否实现 AT-SPI 旁路。
- **DoD**：① `docs/dev/a11y.md` 存在，含三条路径的实测结论与局限声明。[文档] ② `client_preedit` 开启后，应用的无障碍树包含 preedit 文本。[实验室] ③ **焦点断言**：任何无障碍路径下候选框都不夺取键盘焦点。[实验室] ④ `key_to_present_p99` 不劣化。[性能] ⑤ 局限在 `docs/dev/privacy.md` 同族文档中如实声明，不给用户"完全无障碍"的错觉。[文档]

---

#### 任务 ID：ADD-FEAT-P1.05.04 `client_preedit` 路径与 AT-SPI 旁路

- **基本属性**：绑定差距条目：`GAP-39` ｜ `P1 效率进阶 | 中 | 3 人天` ｜ 前置 `ADD-FEAT-P1.05.03` ｜ `CP: 否` ｜ `Track B` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-fcitx5/src/engine.rs`（改：`client_preedit` 的消费）、`crates/ime-ui/src/ui_thread/a11y.rs`（改：按评估结论实现旁路）、`crates/ime-config/src/schema.rs`（改：`client_preedit` 的默认值与文档）
- **目标与价值**：落地 `P1.05.03` 的评估结论。**`UiConfig.client_preedit` 已在 `schema.rs:313` 存在但未见消费方**——这是一个"配置项写了但没接线"的缺口，本卡把它接上。
- **技术设计**：
  - **`client_preedit = true` 的语义**：preedit 由**应用**绘制在它自己的输入框里（而非候选框的 header）。这带来两个收益：(1) 应用的无障碍树天然包含 preedit；(2) 某些应用（终端、IDE）对光标位置的处理更准确。
  - **代价**：候选框的 header 不再显示 preedit，视觉上少了一层信息。**因此默认值需要决策**——建议默认 `false`（保持现有的自绘 header），由无障碍需求驱动开启。
  - **AT-SPI 旁路**（若 `P1.05.03` 的评估结论支持）：注册为**不可聚焦**的 AT-SPI 对象，暴露候选列表的只读文本。
    ```rust
    /// The read-only accessibility view of the candidate window.
    ///
    /// Registered as a non-focusable object: the candidate window must never
    /// take keyboard focus (`features.md` 0.4 rule 5), so the accessibility
    /// object exists to be *read*, not to be tabbed into. A screen reader that
    /// only announces focusable objects will therefore still not see it, which
    /// is a limitation the documentation states rather than hides.
    pub trait A11ySurface {
        /// Publishes the current frame's candidate texts.
        ///
        /// # Errors
        /// Returns `UiError::NotReady` when the accessibility bus is not
        /// available; the caller degrades silently, because a desktop without
        /// a screen reader must not pay for the accessibility path.
        fn publish(&mut self, frame: &UiFrame) -> Result<(), UiError>;
    }
    ```
- **NFR**：**绝不夺焦**；无障碍总线不可用时静默降级（无屏幕阅读器的桌面不为该路径付出代价）；`publish` ≤ 0.2ms；`client_preedit` 开启时 `key_to_present_p99` 不劣化。
- **实施步骤**：① 接线 `client_preedit`（既有配置项的第一个消费方）；② 按 `P1.05.03` 的结论实现 `A11ySurface`；③ 无障碍总线探测与静默降级；④ 默认值与文档；⑤ 焦点断言测试。
- **DoD**：① `client_preedit = true` 时 preedit 由应用绘制且进入其无障碍树。[实验室] ② **焦点断言**：无障碍路径不夺焦。[实验室] ③ 无屏幕阅读器环境下零额外开销（`publish` 不被调用）。[自动] ④ `cargo nextest run -p ime-fcitx5` 全绿。[自动]

---

#### 任务 ID：ADD-FEAT-P1.05.05 `ime-doctor` 用户级自助诊断

- **基本属性**：绑定差距条目：`GAP-40` ｜ `P1 效率进阶 | 中 | 4 人天` ｜ 前置 `ADD-FEAT-P1.05.02` ｜ `CP: 否` ｜ `Track C` ｜ `[ ] 待开始`
- **代码落地锚点**：`crates/ime-diag/src/doctor.rs`（新增）、`xtask/src/main.rs`（改：`doctor` 子命令）、`crates/ime-ui/src/command/settings.rs`（改：面板入口）、`docs/dev/doctor.md`（新增）
- **目标与价值**：`features.md` 的 `TASK-2.08.01` 的落地。用户遇到问题时**只能**看 `tracing` 日志，且不知道日志在哪。`ime-doctor` 给出一个自解释的检查清单：环境、依赖、权限、词库、平台后端、预算，每项 `OK` / `WARN` / `FAIL` 并附**可执行的修复建议**。
- **技术设计**：
  - **检查项**（每项独立，一项失败不影响其余）：

    ```rust
    /// The outcome of one diagnostic check.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct CheckResult {
        /// Stable identifier, so the output can be diffed across runs.
        pub id: &'static str,
        /// What was checked, in one line.
        pub summary: &'static str,
        /// The outcome.
        pub level: CheckLevel,
        /// What was found, with the numbers that led to the conclusion.
        pub detail: String,
        /// What the user can do about it; empty when there is nothing to do.
        pub remedy: &'static str,
    }

    /// How serious a check's outcome is.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    pub enum CheckLevel {
        /// Everything is as expected.
        Ok,
        /// Working, but in a degraded mode the user should know about.
        Warn,
        /// Not working; the user must act.
        Fail,
    }
    ```
  - **检查项清单**（逐项，无省略）：`fcitx5-version`、`addon-installed`、`addon-loaded`、`dict-present`、`dict-version`、`dict-entries`、`script-table`、`data-dir-writable`、`data-dir-perms`、`backup-recent`、`config-valid`、`config-schema-version`、`wayland-tier`、`blur-available`、`font-cjk-present`、`no-network`、`rss-budget`、`decode-latency`。
  - **不引入网络**：`no-network` 检查项**验证**零网络（复用 `runtime-socket-check.sh` 的判定），**不发起**任何连接。
- **NFR**：`ime-doctor` 全程 ≤ 2 秒；**不含**任何用户数据（词条数、输入内容）；每项检查独立超时（一项卡住不拖垮整体）；输出人类可读 + `--json`。
- **实施步骤**：① 实现 `CheckResult` / `CheckLevel`；② 18 个检查项逐个实现；③ CLI 输出（人类可读 + `--json`）；④ 面板入口；⑤ `docs/dev/doctor.md` 逐项说明含义与修复。
- **DoD**：① `rspinyin --doctor` 输出 18 项检查，每项含级别、详情、修复建议。[自动] ② 在健康环境上全部 `OK`。[实验室] ③ 在故意破坏的环境（词库缺失、目录只读、无 CJK 字体）上给出正确的 `FAIL`/`WARN` 与修复建议。[实验室] ④ 输出不含用户数据。[文档] ⑤ 全程 ≤ 2 秒。[性能]

---

#### 任务 ID：ADD-FEAT-P1.05.06 发行版打包（deb / rpm / AUR）

- **基本属性**：绑定差距条目：`GAP-42` ｜ `P1 效率进阶 | 高 | 6 人天` ｜ 前置 `ADD-FEAT-P0.05.01` ｜ `CP: 否` ｜ `Track C` ｜ `[ ] 待开始`
- **代码落地锚点**：`packaging/deb/`（新增）、`packaging/rpm/`（新增）、`packaging/aur/`（新增）、`packaging/install.sh`（改：改为打包脚本的公共部分）、`justfile`（改：`package-*` recipe）、`docs/dev/packaging.md`（新增）
- **目标与价值**：`features.md` 的 `TASK-2.07.01` 的落地。当前只有 `install.sh`——用户必须克隆仓库、装开发包、编译。**发行版包是"能被普通用户安装"的前提。** `ASM-A-17` 定标 deb（Ubuntu 22.04/24.04）、rpm（Fedora 40/41）、AUR（Arch）。
- **技术设计**：
  - **安装布局**（`features.md` 4.6 的 `MOD-SHIP`）：`librspinyin.so` + `librspinyin-ui.so` → `/usr/lib/fcitx5/`；`rspinyin.conf` / `rspinyin-im.conf` → `/usr/share/fcitx5/addon/` 与 `/usr/share/fcitx5/inputmethod/`；`base.dict` / `script.dict` / `symbols.dict` / `english.dict` → `/usr/share/rspinyin/`。
  - **`OnDemand=False`**（`features.md` 6.2.3 的陷阱）：`rspinyin.conf` 必须常驻，否则首键延迟 120ms（`BUDGET-LAT-05`）。
  - **`strip` 的处理**（`ADR-0002` + `TC-SEC-45`）：`Cargo.toml` 的 release profile **不得**设 `strip`；剥离在**打包时**进行，且**必须在 `nm -D` 校验 `fcitx_addon_factory_instance` 仍存在之后**才能交付。
    ```bash
    # packaging/deb/build.sh — the strip step, with the guard that makes it safe.
    strip --strip-unneeded "$STAGE/usr/lib/fcitx5/librspinyin.so"
    # ADR-0002: the addon factory symbol is what fcitx5's dlopen looks for. A
    # strip that removed it would produce a package that installs cleanly and
    # then does nothing, which is the worst possible failure mode.
    nm -D "$STAGE/usr/lib/fcitx5/librspinyin.so" | grep -q fcitx_addon_factory_instance \
      || { echo "strip removed the addon factory symbol"; exit 1; }
    ```
  - **依赖声明**：`fcitx5 (>= 5.1)`、`libc6`、字体依赖（CJK 字体是必需的吗？——`UiFontMissingCjk` 是既有的错误码，说明字体缺失是可诊断的降级，**不应**硬依赖，应 `Recommends`）。
  - **可卸载且可逆**（`features.md` 6.3 的 Phase 2 出口准则 #3）：卸载后 `fcitx5` 的配置必须恢复——这依赖 `TASK-1.04.03` 的 `ui_takeover.json` 备份机制。
- **NFR**：三个包的安装/卸载在对应发行版上可逆；`.so` 的剥离后体积 ≤ `so_stripped = 12MB`；`nm -D` 校验必须在打包脚本中**强制**（不是文档要求）；**不引入网络**（打包脚本不下载任何东西，`base.dict` 从仓库构建）。
- **实施步骤**：① 整理安装布局（复用 `install.sh` 的既有逻辑）；② deb 的 `control` / `postinst` / `prerm`；③ rpm 的 `.spec`；④ AUR 的 `PKGBUILD`；⑤ **`nm -D` 校验**接入三个打包脚本；⑥ 三套包的安装/卸载可逆性验证。
- **DoD**：① deb 在 Ubuntu 22.04 与 24.04 上可安装、可卸载、可逆。[实验室] ② rpm 在 Fedora 40/41 上同上。[实验室] ③ AUR 包在 Arch 上同上。[实验室] ④ `nm -D` 校验在三个打包脚本中生效（人为去掉校验后打包会失败）。[自动] ⑤ 剥离后 `.so` ≤ 12MB。[性能] ⑥ 卸载后 fcitx5 配置恢复。[实验室]

---

#### 任务 ID：ADD-FEAT-P1.05.07 预算回归门禁

- **基本属性**：绑定差距条目：`features.md` 的 `TASK-2.08.02`（**不在主文档 5.1 的 44 行绑定范围内**）｜ `P1 效率进阶 | 中 | 3 人天` ｜ 前置 `ADD-FEAT-P1.05.05` ｜ `CP: 否` ｜ `Track C` ｜ `[ ] 待开始`
- **代码落地锚点**：`.github/workflows/ci.yml`（改：预算 job）、`xtask/src/budget.rs`（改：从"校验阈值一致"扩展为"校验实测值"）、`scripts/budget-regression.sh`（新增）、`docs/dev/budgets.json`（只读）
- **目标与价值**：`features.md` 6.3 的 Phase 2 出口准则 #4："`TASK-2.08.02` 的预算回归门禁在 CI 中生效（连续 10 次 PR 无预算退化）"。`features.md` 0.5.3 把每个阈值都变成**可 CI 断言的声明**，`xtask budget --validate` 目前只校验 `budgets.json` 与文档表格一致——**不校验实测值**。本卡补上后半截。
- **技术设计**：
  - **两层门禁**：
    1. **阈值一致性**（既有）：`budgets.json` ↔ `features.md` 0.5.3 的表格逐项一致。
    2. **实测回归**（本卡新增）：在 CI 上跑基准，与**基线快照**比较，超出容差即失败。
  - **基线快照的存储**：`docs/dev/budget-baseline.json`（提交入库），记录上一次认可的实测值。PR 若使某项劣化超过容差，CI 失败并要求显式更新快照（**更新快照的 PR 必须附带理由**）。
  - **容差**（必须显式，否则 CI 会因噪声频繁失败）：
    ```json
    {
      "tolerance_pct": { "latency_ms": 10.0, "memory_mb": 5.0, "size_mb": 2.0, "cpu_pct": 20.0 },
      "note": "Latency tolerates 10% because CI machines are noisy; memory and size tolerate less because they are deterministic."
    }
    ```
  - **关键约束（`.dev-progress.json` 的阻塞项 3）**：**所有基准必须在空闲机器上跑**。当前"今日所有数字取自 20 个并发 agent 环境，不可信"。因此本卡的门禁**必须**跑在专用 runner 上，或在 CI 中**串行执行**且标注 `runner: self-hosted` 或等效。
- **NFR**：CI 的预算 job ≤ 10 分钟；容差显式；**不得**用放宽阈值来解决回归（`AGENTS.md` 第 8 节禁止"删除或绕过测试使构建通过"）；基线快照的更新必须在 PR 描述中说明理由。
- **实施步骤**：① 定义 `budget-baseline.json` 的结构与容差；② 在**空闲环境**上采集初始基线；③ `xtask budget` 扩展为"跑基准 + 比对 + 输出 diff"；④ CI job（串行、专用 runner）；⑤ 连续 10 次 PR 的稳定性验证。
- **DoD**：① `xtask budget --regression` 能检出人为制造的 20% 劣化。[自动] ② CI job 在连续 10 次 PR 上无假阳性。[自动] ③ 容差在 `budget-baseline.json` 中显式。[文档] ④ 基线快照的更新在 PR 中需说明理由（CI 检查 PR 描述非空）。[自动]

---

## 2. 分片出口准则

1. **27 张 P1 卡全部验收通过**，每条验收标准有非空验收记录。[文档]
2. **独占预算合计 ≤ 0.40ms**（主文档 0.3 的 P1 池），且 `decode_p99 ≤ 3.0ms` 在**全部解码类 P1 特性同时开启**时仍成立。[性能]
3. **`idle_redraw_count = 0`、`idle_poll_timer_count = 0`** 在常驻状态指示器（`P1.02.06`）开启时仍成立。[性能]
4. **焦点断言**：`P1.01.04`（符号面板）、`P1.02.01`（命令面板）、`P1.02.06`（状态指示器）、`P1.05.03`/`P1.05.04`（无障碍）五处新增 surface **全部不夺取键盘焦点**。[实验室]
5. **`cargo public-api -p ime-ui` 不含 `slint::`**（`OB-4` 在新增 4 个面板后仍成立）。[自动]
6. **无新增网络能力**：`zbus`（`P1.04.03`）与任何压缩依赖（`P1.04.01`）均在 `licenses.md` 登记且 `cargo deny` 通过。[自动]
7. **主文档 5.1 追溯表的 P1 行**（22 个差距编号）全部有已完成的绑定卡。[文档]
8. **配置键总数 ≤ 192**（`ASM-A-10` 的 `MAX_DOCUMENT_KEYS`）。[自动]
9. **无障碍结论如实落地**：`docs/dev/a11y.md` 存在且含局限声明。[文档]

