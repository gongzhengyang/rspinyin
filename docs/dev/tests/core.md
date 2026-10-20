# rspinyin 测试用例分片 · core（解码引擎）

> 分片版本: v3.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 执行器必读: [../test-architecture.md](../test-architecture.md) ｜ 模块形态: Headless ｜ 逻辑自检: 已通过 ｜ 维护约定: 用例状态变更须同步主文档矩阵
>
> 基线引用：`ASM-T-04/05/07/10/11`；阈值只引 `docs/dev/budgets.json` 键名。模块面判定 = `xtask testd-engine`（场景 TOML）+ `xtask testd-smoke`（211 例 TSV）+ `cargo nextest -p ime-core`（556 测）。**吸收** 字段登记本卡取代的 v2.0 用例号（其验收记录与证据包保留于 `results/runs/`）。

---

#### TC-CORE-001 音节表全表识别与输入规范化

- **基本属性**：`REQ-CORE-01` ｜ `ASM-T-10` ｜ core ｜ 维度 ①②③ ｜ 执行驱动：`testd-engine` + `cargo nextest -p ime-core` ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `requires_screenshots: false` ｜ `screenshots_expected: []` ｜ 吸收：TC-CORE-01~05 ｜ 锚点：`crates/ime-core/src/segment/syllable.rs`、`segment/dag.rs`（规范化）｜ 背书：syllable/normalize 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：进程内重放；`dict_inject::Synthetic` 词库；无时钟/文件/环境依赖（0.4 规则 4）。
- **操作步骤与物理存盘动作**（`<EVIDENCE_DIR>` = `<RUN>/core/TC-CORE-001`）：
  1. 对音节表全量 411 键逐一构造 raw 并解码 → 追加 `<EVIDENCE_DIR>/execution_log.txt`（逐键命中摘要）。
  2. `ü` 的四种输入形态（v/ü/u:）分别解码 → 记录规范化结果。
  3. 注入非法字符与超长（>64B）raw → 记录 `DroppedChars` 诊断与拒绝行为。
  4. 生成 `assertions.json`（账本 v3）。
- **通过标准与多模态断言契约**：
  - **[功能与状态机断言]**：全部合法音节可达；`ü` 四形态等价；非法字符丢弃且有诊断；超长在规范化前拒绝并命中冻结错误码（`ASM-T-10`）。
  - **[Headless 断言]**：全键盘流可遍历输入音节表（无鼠标依赖）。
  - **[性能与资源预算断言]**：规范化在 decode 预算内（不单列阈值，由 `TC-CORE-004` 的 `decode_p99` 总量覆盖）。

#### TC-CORE-002 切分 DAG：多路径保留与无路径拒止

- **基本属性**：`REQ-CORE-02` ｜ `ASM-T-10` ｜ core ｜ ①② ｜ `testd-engine` + nextest ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-CORE-06~10 ｜ 锚点：`crates/ime-core/src/segment/dag.rs:80-157` ｜ 背书：dag/fuzz 套件 + 场景 `engine-input-without-a-path` 等
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：合成词库；场景 TOML 驱动。
- **操作步骤与物理存盘动作**：
  1. 歧义输入（如 `xian`）解码 → 记录两条切分路径的候选并存 → `execution_log.txt`。
  2. 无路径输入（连续声母）→ 记录冻结错误码与降级行为。
  3. `'` 强制分隔符场景 → 记录边界行为。
  4. `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：歧义完整保留交由 Viterbi 排序；无路径返回冻结码不 panic；`'` 为 `EdgeKind::Forced`。
  - **[Headless 断言]**：fuzz 目标 60s 无 panic（背书套件内含）。
  - **[性能断言]**：DAG 构建在合成大输入下不超过 `decode_p99` 份额（场景内计时）。

#### TC-CORE-003 输入缓冲与按音节删除

- **基本属性**：`REQ-CORE-03` ｜ `ASM-T-10` ｜ core ｜ ①②⑤ ｜ nextest + 场景 `engine-backspace-removes-a-syllable` 等 ｜ `[Acceptance 验收套件]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-CORE-11~15 ｜ 锚点：`crates/ime-core/src/input/buffer.rs` ｜ 背书：buffer 套件 + proptest
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：进程内；caret 全位置矩阵。
- **操作步骤与物理存盘动作**：
  1. `nihaoa` 序列的逐次 Backspace → `execution_log.txt`（每步 raw）。
  2. caret=0 时 Backspace → 记录行为。
  3. 64B 上限后 `push_char` → 记录拒绝。
  4. `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：删除按音节边界（`nihaoa`→`nihao`→`ni`→空，逐段落 BufferEmpty→Idle）；caret=0 不 panic；上限拒绝有诊断。
  - **[Headless 断言]**：缓冲不变量 proptest 通过。
  - **[性能断言]**：删除路径无分配热点（alloc-count 预算不回退）。

#### TC-CORE-004 K-best 解码：质量、边界与确定性

- **基本属性**：`REQ-CORE-04` ｜ `ASM-T-04/10` ｜ core ｜ ①②④ ｜ `testd-engine` + `testd-smoke` + criterion ｜ `[Acceptance 验收套件]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-CORE-16~20、42~47 ｜ 锚点：`crates/ime-core/src/viterbi/{decoder,kbest,lattice}.rs`、`mod.rs:193` 整句首选测试 ｜ 背书：viterbi 套件（字节指纹 100 次）+ `bench/decode`
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：合成词库 + 211 例 TSV 基线挂载。
- **操作步骤与物理存盘动作**：
  1. 同输入 100 次解码 → 逐字节一致断言 → `execution_log.txt`。
  2. 12 音节长输入计时 → 记录 P99 → `assertions.json`（budget_keys: `decode_p99`）。
  3. 空词库降级（单字兜底仍出候选）→ 记录。
  4. `K`/候选上限三退化（`beam_k`/`max_candidates` 配置校验）→ 记录。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：整句首选契约（`test_decode_ranks_the_whole_sentence_first`）保持；双字词优先/少切分偏好成立；候选 ≤45 分页 5×9。
  - **[Headless 断言]**：场景 TOML 全部可加载；引擎直驱零文件/时钟触碰。
  - **[性能断言]**：12 音节 P99 ≤ `decode_p99`（24ms）；`just smoke` 211 例全绿。

#### TC-CORE-005 打分：定点整数与用户词频融合

- **基本属性**：`REQ-CORE-05` ｜ `ASM-T-04` ｜ core ｜ ①④ ｜ nextest + `xtask quality` ｜ `[Acceptance 验收套件]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-CORE-21~25 ｜ 锚点：`crates/ime-core/src/lm/score.rs:173-177,250-257` ｜ 背书：score 套件 + `bench/decode`
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：打分纯函数直驱；留出集 `lm_holdout.tsv`。
- **操作步骤与物理存盘动作**：
  1. 五权重定点断言（Q8.8，无浮点排序）→ `execution_log.txt`。
  2. 用户频次上限钳制（`min(log2(1+freq)/8,2)`）→ 记录。
  3. bigram miss 退化确定性（`unigram + BIGRAM_MISS_PENALTY`）→ 记录。
  4. `assertions.json`（budget_keys: 无直引，质量口径）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：非法权重配置被拒；排序翻转对照（有/无用户频次）成立；留出集可达性门槛（top1/top9 不低于基线）。
  - **[Headless 断言]**：打分对相同输入在任意运行同序（确定性契约）。
  - **[性能断言]**：打分单边耗时在 criterion 阈值内（`budgets.json bench` 段对应键）。

#### TC-CORE-006 直通与临时英文

- **基本属性**：`REQ-CORE-06` ｜ `ASM-T-10` ｜ core ｜ ①② ｜ nextest + 场景 ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-CORE-26~30、48~50 ｜ 锚点：`crates/ime-core/src/passthrough.rs`（classify 500ns 预算注释）｜ 背书：passthrough 套件 + `bench/passthrough`
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：`PassthroughFlags` 矩阵注入。
- **操作步骤与物理存盘动作**：
  1. 大写开头 → 英文直通 → `execution_log.txt`。
  2. URL/邮箱启发式 → 直通 → 记录。
  3. 中文标点表 + 全角 94 字符覆盖 → 记录。
  4. 临时英文模式全键透传 + Escape/Enter 退出 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：直通判定纯函数；双引号配对依赖上下文（有状态项显式）；标点/全角表常量完备。
  - **[Headless 断言]**：`classify` 判定与路由行为一致（无第二份规则）。
  - **[性能断言]**：`classify` 在 criterion 阈值内（既有 500ns 量级，具体以 `budgets.json bench` 键为准）。

#### TC-CORE-007 Preedit 与切分高亮段

- **基本属性**：`REQ-CORE-07` ｜ `ASM-T-10` ｜ core ｜ ①③④ ｜ nextest + 场景 ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-CORE-31~35 ｜ 锚点：`crates/ime-core/src/preedit.rs` ｜ 背书：preedit 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：进程内。
- **操作步骤与物理存盘动作**：
  1. 多音节输入的 span 全覆盖 → `execution_log.txt`。
  2. `SpanKind` 分布（Syllable/Separator/Passthrough/Cursor）→ 记录。
  3. 无路径输入退化为单一 Passthrough；空输入 → 记录。
  4. `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：span 边界与音节边界一致；退化路径合法。
  - **[Headless 断言]**：长度上限行为与冻结契约一致。
  - **[性能断言]**：preedit 生成在 decode 预算内（场景计时）。

#### TC-CORE-008 会话状态机：跃迁/翻页/revision/焦点

- **基本属性**：`REQ-CORE-08`（`REQ-RT-06` 同域）｜ `ASM-T-10` ｜ core ｜ ①②⑤ ｜ nextest + 场景 ｜ `[Acceptance 验收套件]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-CORE-36~41 ｜ 锚点：`crates/ime-core/src/state/{machine,transitions,paging}.rs` ｜ 背书：state 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：会话机直驱；效果序列断言器。
- **操作步骤与物理存盘动作**：
  1. 跃迁表逐行驱动（Idle/Composing/Cancelling/Committing）→ `execution_log.txt`。
  2. 翻页边界（首页上翻/末页下翻不环绕）+ `reconcile` 高亮保持 → 记录。
  3. `revision` 单调与过期帧丢弃；焦点丢失不提交 → 记录。
  4. 配置热重载不重置会话 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：Effect 有序（Show 先于 SendFrame、Commit 单次）；`PinHighlighted` 的转正状态以 features-add `TASK-1.04.01` 落地进度为准（工作树已出现 `toggle_pin` 实现，2026-10-20 观察；验收归 `TC-ADD-006`）。
  - **[Headless 断言]**：翻页 9/页 ×5 页语义精确。
  - **[性能断言]**：状态机步进无可测开销（背书覆盖）。

#### TC-CORE-009 自定义短语引擎

- **基本属性**：`REQ-CORE-09` ｜ `ASM-T-07` ｜ core ｜ ①② ｜ nextest ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-CORE-51~55 ｜ 锚点：`crates/ime-core/src/phrase.rs`、`phrase/writer.rs` ｜ 背书：phrase 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：短语文档 fixture；写路径临时目录。
- **操作步骤与物理存盘动作**：
  1. 导入/命中/置顶（`rq`→日期，最高优先插队）→ `execution_log.txt`。
  2. 与用户频次、词库权重的三方排序 → 记录。
  3. 边界：超长词、26 键上限、重复缩写 → 记录。
  4. 写入器原子性（fsync-rename）与开关零开销 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：短语不进词格不打分、按定义优先插队；`$Y/$M/$D/$T` 由宿主展开（表不读时钟）。
  - **[Headless 断言]**：关闭档零开销路径成立。
  - **[性能断言]**：短语表构建/查询在管理路径预算内。

#### TC-CORE-010 模糊音引擎层

- **基本属性**：`REQ-CORE-10` ｜ `ASM-T-04` ｜ core ｜ ①②④ ｜ nextest + 场景 v2（flags 注入）｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-CORE-56~60 ｜ 锚点：`crates/ime-core/src/fuzzy.rs`、`crates/ime-types/src/decode.rs:56-72` ｜ 背书：fuzzy 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：场景 schema v2 的 `[decode] flags` 注入（P0.02.01 交付后启用；未交付前本卡按 `pending-impl` 记录）。
- **操作步骤与物理存盘动作**：
  1. 8 类等价类逐类开启 → 双向命中与确定性排序 → `execution_log.txt`。
  2. 全开档计时 → `assertions.json`（budget_keys: `decode_p99`）。
  3. × 词库注入畸形兼容 → 记录。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：替代拼写上限成立；关闭档与主线逐字节一致。
  - **[Headless 断言]**：`[fuzzy]` 配置节已在工作树出现（未提交，2026-10-20 观察，对应 `TASK-2.01.01`）——本卡只测扩边行为与预算，配置→会话端到端判定归 `TC-ADD-007`。
  - **[性能断言]**：全开 ≤ 独占预算 0.60ms（features-add §3.4 口径，场景内计时断言）。

#### TC-CORE-011 简拼引擎层

- **基本属性**：`REQ-CORE-11` ｜ `ASM-T-04` ｜ core ｜ ①② ｜ nextest + 场景 v2 ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-CORE-61~65 ｜ 锚点：`crates/ime-core/src/segment/abbrev.rs`、`viterbi/lattice/abbrev.rs:133-153` ｜ 背书：abbrev 套件 + proptest
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：mock 词库（prefix 可用）+ 场景 flags 注入。
- **操作步骤与物理存盘动作**：
  1. `nh`/`nih` 展开命中 → `execution_log.txt`。
  2. 四轴有界（边数/查询数/词长/总边）→ 记录。
  3. × 用户频次交互；关闭态逐字节一致 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`ABBREV_PENALTY_Q8` 保证简拼不压过全拼；真实词库 `prefix` 已在工作树出现流式实装（未提交，2026-10-20 观察）——本卡只测引擎层 walk 行为，真实词库端到端判定归 `TC-ADD-007`。
  - **[Headless 断言]**：proptest 不变量通过。
  - **[性能断言]**：≤ 独占预算 0.50ms（features-add §3.4 口径）。

#### TC-CORE-012 解码压力：fuzz/长输入/分配预算

- **基本属性**：`REQ-CORE-04` ｜ `ASM-T-04/10` ｜ core ｜ ②④ ｜ fuzz + criterion + alloc-count ｜ `[Stress 深度压测]` ｜ P2 ｜ `false` ｜ `[]` ｜ 吸收：TC-CORE-09/10/14/20 ｜ 锚点：`crates/ime-core/src/viterbi/`、`crates/alloc-count/` ｜ 背书：fuzz 60s + `budget --alloc`
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：空闲机器（`ASM-T-11`，`purity` 门禁通过）。
- **操作步骤与物理存盘动作**：
  1. fuzz 目标 60s 无 panic → `execution_log.txt`。
  2. 稳态解码分配计数 = `alloc_count.decode_steady`（13）→ `assertions.json`（budget_keys: `alloc_count.decode_steady`）。
  3. 最长合法输入（64B）连续解码 1,000 次延迟分布 → 记录（budget_keys: `decode_p99`, `decode_p999`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：任何 fuzz 输入不 panic、不 OOB。
  - **[Headless 断言]**：分配预算超限即红（不是告警）。
  - **[性能断言]**：长输入 P99 ≤ `decode_p99`；P999 ≤ `decode_p999`；采样在空闲机器并留机器状态记录。
