# rspinyin 测试用例分片 · add（features-add 增量面 · 全部待实现）

> 分片版本: v3.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 执行器必读: [../test-architecture.md](../test-architecture.md) ｜ 模块形态: Hybrid（锚定 [../features-add.md](../features-add.md) 的 19 个 GAP）｜ 逻辑自检: 已通过 ｜ 维护约定: 对应 TASK 落地后，本分片用例由 `env_gate` 判定转为可执行并纳入对应层级调度
>
> **可执行性纪律**：本分片全部用例的基本属性带 `` 可执行性：`[待实现]` `` 标注与 `锚定` TASK id——被测面未合入前，`env_gate` 记 `pending-impl`（不算通过也不算失败）；TASK 合入后自动转可执行，无需改写用例卡。锚点均为 features-add 登记的**规划锚点**，落地时若实际路径有出入，以实现为准并回写本卡。
>
> **工作树观察（2026-10-20）**：锚定任务中至少 `TASK-1.04.01`（toggle_pin）、`TASK-2.01.01`（[fuzzy] 配置节）、`TASK-2.02.01`（prefix 流式实装）已在工作树出现**未提交**实现——本分片状态判定以 `features-add.md` 的任务状态回写为准：落地验收时先回写任务文档（含验收记录），再由 `env_gate` 翻转本分片对应卡。

---

#### TC-ADD-001 词库权重分层归一化与质量门

- **基本属性**：`REQ-ADD-01` ｜ `ASM-T-07` ｜ add ｜ ①④ ｜ `xtask dictc` + `xtask quality` ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `requires_screenshots: false` ｜ `[]` ｜ 可执行性：`[待实现]` ｜ 锚定：`TASK-1.01.01`/`TASK-1.01.02` ｜ 锚点：`xtask/src/dictc/normalize.rs`（规划）｜ 背书：normalize 套件（待落地）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：合成两带 fixture（`dict_inject::Layered`，`P0.02.02` 交付后可用）。
- **操作步骤与物理存盘动作**：
  1. 归一化编译：两字真频保留 / 3+ 字压平 / 词长梯度校验 → `execution_log.txt`。
  2. 分带 top1 审计报告（1字/2字/3+字三行）→ `assertions.json`（门禁值：总 top1 ≥ 78%，features-add §4.3）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：双编译 sha256 一致；词条/键数变化 ≤1%；`NormalizeReport` 三带计数正确。
  - **[Headless 断言]**：分带报告三行齐全且两字带不低于前基线。
  - **[性能断言]**：编译时长 ≤ 9.8s（+5% 门）。

#### TC-ADD-002 用户词频衰减行为与排序

- **基本属性**：`REQ-ADD-02` ｜ `ASM-T-04` ｜ add ｜ ①④ ｜ 场景 v2（`[decay]` 时钟注入）+ nextest ｜ `[Acceptance 验收套件]` ｜ P0 ｜ `false` ｜ `[]` ｜ 可执行性：`[待实现]` ｜ 锚定：`TASK-1.02.01`/`TASK-1.02.02` ｜ 锚点：`crates/ime-dict/src/user_db/decay.rs`（规划）｜ 背书：decay 套件（待落地）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：v2 用户库 fixture + 已知 tick 序列。
- **操作步骤与物理存盘动作**：
  1. v1 文件打开 → 迁移幂等 + 行数守恒 → `execution_log.txt`。
  2. 注入 tick 序列：久未用词沉底、新词上浮的排序对照 → `assertions.json`。
  3. `effective_freq` 冷/热基准（budget_keys: `decode_p99` 份额内）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：半衰点准确性（elapsed=139 → 衰减 ≈1/2，误差 <1/256）；readonly 下折叠只在内存。
  - **[Headless 断言]**：无衰减参数时与 v1 排序逐字节一致。
  - **[性能断言]**：折叠冷路径 ≤ 0.10ms 独占预算、热路径 ≈0。

#### TC-ADD-003 衰减配置与空闲批算

- **基本属性**：`REQ-ADD-02` ｜ `ASM-T-05` ｜ add ｜ ② ｜ nextest(-p ime-config/-p ime-dict) ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 可执行性：`[待实现]` ｜ 锚定：`TASK-1.02.03` ｜ 锚点：`crates/ime-config/src/reload.rs`（`[data] decay_*` 规划）｜ 背书：配置防漂移测试（待落地）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：坏值 fixture（`decay_half_life_ticks = 0`）。
- **操作步骤与物理存盘动作**：
  1. 坏值修默认 139 + 单条 `config/invalid` → `execution_log.txt`。
  2. sweep 批量折叠后磁盘 tick 距 now ≤ 1 轮步长；淘汰排序按 tick → `assertions.json`（budget_keys: `cpu_pct.idle`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：模板防漂移测试更新；宿主线程零 involvement。
  - **[Headless 断言]**：sweep 单轮增量 ≤20%。
  - **[性能断言]**：空闲预算不回退。

#### TC-ADD-004 BIGRAM 段与真查排序

- **基本属性**：`REQ-ADD-03` ｜ `ASM-T-04/07` ｜ add ｜ ①④ ｜ `dict_inject::BigramPresent` + 场景 v2 + criterion ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 可执行性：`[待实现]` ｜ 锚定：`TASK-1.03.01`/`TASK-1.03.02` ｜ 锚点：`crates/ime-dict/src/fst_index/dict_lm.rs:207-214`、`format/mod.rs`（规划）｜ 背书：bigram 套件（待落地）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：`BigramPresent` fixture（corpus-snippet 自有文本，禁 ADR-0012 语料）。
- **操作步骤与物理存盘动作**：
  1. v2 容器往返（flag 置位、记录表升序、CRC）→ `execution_log.txt`。
  2. 「中国/银行」类接续：bigram 命中改变首选的对照 → `assertions.json`（budget_keys: `decode_p99`）。
  3. hit/miss 双基准。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：两模型同序契约（`DictLm` vs `InMemoryLm` 注入同 pair 逐字节同序）；miss 路径 = `unigram + BIGRAM_MISS_PENALTY` 保留。
  - **[Headless 断言]**：`xtask quality` top1 对照 ≥ +1pp。
  - **[性能断言]**：查表 ≤ 0.25ms 独占预算。

#### TC-ADD-005 BIGRAM 缺席降级与诊断

- **基本属性**：`REQ-ADD-03` ｜ `ASM-T-10` ｜ add ｜ ② ｜ nextest + 场景（`bigram="absent"`）｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 可执行性：`[待实现]` ｜ 锚定：`TASK-1.03.02`（W-8 退出路径）｜ 锚点：`dict_lm.rs` 装配点（规划）｜ 背书：装配降级测试（待落地）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：v1 容器（无 BIGRAM 段）。
- **操作步骤与物理存盘动作**：
  1. v1 容器加载 → 行为与现状逐字节一致 → `execution_log.txt`。
  2. `dict/bigram/absent` 诊断每进程恰一次 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`just smoke` 211 例在无段容器下全绿（回归不变）。
  - **[Headless 断言]**：flag 不置位时读端拒绝非空段（`dict/corrupt`）。
  - **[性能断言]**：降级零开销。

#### TC-ADD-006 置顶词全链路

- **基本属性**：`REQ-ADD-04` ｜ `ASM-T-01/02` ｜ add ｜ ①③⑤ ｜ 场景 + `test-mirror` + `testd-client` ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `true` ｜ `["01_before_pin.png", "02_after_pin.png", "03_unpin.png"]` ｜ 可执行性：`[待实现]` ｜ 锚定：`TASK-1.04.01`/`TASK-1.04.02` ｜ 锚点：`crates/ime-core/src/state/transitions.rs:203`（现状 Unsupported）｜ 背书：pin 套件（待落地）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：沙盒会话；`set_pinned` 契约（ADR-0013）合入。
- **操作步骤与物理存盘动作**：
  1. Pin 高亮候选 → 目标词升至首位 → 存 `01/02.png` + `execution_log.txt`。
  2. 再按 = 取消（幂等）→ 存 `03_unpin.png`。
  3. forget 清 pin；导出含 pinned 标记；readonly 下 Pin 报 `data/readonly-mode` → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：镜像断言候选序列变化；`PINNED_FREQ` 封顶不破坏确定性。
  - **[视觉断言]**：置顶词按普通第一候选呈现（无新视觉状态，features-add §3.2）。
  - **[性能断言]**：Pin 动作延迟 ≤ 一个解码周期。

#### TC-ADD-007 模糊音/简拼/简繁生产接线端到端

- **基本属性**：`REQ-ADD-05` ｜ `ASM-T-01` ｜ add ｜ ①⑤ ｜ 沙盒会话（XTEST）+ `testd-client` ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `true` ｜ `["01_fuzzy_commit.png", "02_abbrev_commit.png", "03_script_commit.png"]` ｜ 可执行性：`[待实现]` ｜ 锚定：`TASK-2.01.01`/`TASK-2.02.01`/`TASK-2.02.02`/`TASK-2.03.01`/`TASK-2.03.02` ｜ 锚点：`state/machine.rs:454`、`router/modes.rs` ｜ 背书：接线套件（待落地）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：配置 fixture（fuzzy 开 zh_z / abbrev 开 / script 开）三组会话。
- **操作步骤与物理存盘动作**：
  1. 模糊音会话：`zhong` 命中两族 → 读回断言 → 存 `01_fuzzy_commit.png`。
  2. 简拼会话：`nh` 首选 `你好` → 存 `02_abbrev_commit.png`。
  3. 简繁会话：`nihao`+空格提交繁体形态；无变体表时报 `ui/script/unavailable` 且提交不阻断 → 存 `03_script_commit.png` + `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：默认关档与主线逐字节一致；`punct_mode`/`full_width` 配置默认值对新会话生效。
  - **[视觉断言]**：接线不改变候选窗呈现（仅候选集变化）。
  - **[性能断言]**：各接线档不破 `decode_p99`（独立预算口径见 features-add §3.4）。

#### TC-ADD-008 词源扩充与多音字自动校验

- **基本属性**：`REQ-ADD-06` ｜ `ASM-T-07` ｜ add ｜ ①② ｜ `dictc` + `quality` ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 可执行性：`[待实现]` ｜ 锚定：`TASK-2.04.01`/`TASK-2.04.02` ｜ 锚点：`data/sources.toml`、`xtask/src/dictc/quality.rs` ｜ 背书：merge/polyphone 套件（待落地）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：thuocl/huayu 取数完成（sha256 回填）；字频比 fixture。
- **操作步骤与物理存盘动作**：
  1. 两字层合并（huayu 赢、jieba 补缺、顺序无关）→ `execution_log.txt`。
  2. 多音字自动校验三计数（assigned/flattened/manual）齐全且守恒 → `assertions.json`（budget_keys: `base_dict`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：合并后 ≤65 万条 / ≤19MiB（`ASM-A-03` 口径）；词库红线不放宽。
  - **[Headless 断言]**：`xtask quality` 两字带 top1 不降（预期升）。
  - **[性能断言]**：全量编译时长 ≤ 既有门禁。

#### TC-ADD-009 会话共享粒度

- **基本属性**：`REQ-ADD-07` ｜ `ASM-T-01` ｜ add ｜ ①⑤ ｜ 沙盒双 IC + nextest ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 可执行性：`[待实现]` ｜ 锚定：`TASK-2.05.01` ｜ 锚点：`crates/ime-fcitx5/src/engine/context.rs`（规划）｜ 背书：pool 套件（待落地）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：同程序双 IC fixture；碰撞注入用例。
- **操作步骤与物理存盘动作**：
  1. per-program 档：窗口 A 切英文 → 窗口 B 保持英文 → `execution_log.txt`。
  2. 跨程序隔离（hash 碰撞 fallback 二次比较）；策略热切换快照 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：默认 per-context 行为与主线逐项一致（`keymap_matrix` 全绿）。
  - **[Headless 断言]**：敏感上下文抑制学习不回退。
  - **[性能断言]**：池查找 O(1)，宿主无新锁竞争。

#### TC-ADD-010 拼写纠错扩边

- **基本属性**：`REQ-ADD-08` ｜ `ASM-T-04/10` ｜ add ｜ ①② ｜ 场景 v2（`CORRECTION` 位）+ criterion ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 可执行性：`[待实现]` ｜ 锚定：`TASK-3.01.01`（ADR-0014）｜ 锚点：`crates/ime-core/src/segment/correct.rs`（规划）｜ 背书：correct 套件（待落地）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：`CORRECTION` 位 fixture（ADR-0014 合入后）。
- **操作步骤与物理存盘动作**：
  1. `zhegn` → `zheng` 类命中 → `execution_log.txt`。
  2. off 档与主线逐字节一致；正常输入（无错）纠错边不进最优路径 → `assertions.json`（budget_keys: `decode_p99`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：每 span 纠错变体 ≤4；精确命中时零展开。
  - **[Headless 断言]**：双拼/简拼叠加的组合边界（每音节一种容差）。
  - **[性能断言]**：on_miss ≤ 0.30ms 独占预算。

#### TC-ADD-011 上文重排

- **基本属性**：`REQ-ADD-08` ｜ `ASM-T-04` ｜ add ｜ ①④ ｜ 场景两段式（先 commit 上文再断言）｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 可执行性：`[待实现]` ｜ 锚定：`TASK-3.02.01` ｜ 锚点：`crates/ime-core/src/state/machine.rs`（规划）｜ 背书：rerank 套件（待落地）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：`TASK-1.03.02` 已落地（bigram 真查）。
- **操作步骤与物理存盘动作**：
  1. 上句以「银行」结尾 → 下句 `yh` 首选翻转 → `execution_log.txt`。
  2. band 外不翻转（整句首选契约保持）；`context=None` 恒等 → `assertions.json`（budget_keys: `decode_p99`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：上下文词不落任何日志/文件（隐私红线，grep 抽查）。
  - **[Headless 断言]**：100 次同序（确定性）。
  - **[性能断言]**：重排 ≤ 0.05ms。

#### TC-ADD-012 结构升级与功能面合集（重码/热替换/造词/混输/错音/符号/桥接）

- **基本属性**：`REQ-ADD-09`/`REQ-ADD-10` ｜ `ASM-T-01/03/07` ｜ add ｜ ①②③⑤ ｜ 分功能驱动（fixture/沙盒）｜ `[Acceptance 验收套件]` ｜ P2 ｜ `true` ｜ `["01_swap_live.png", "02_symbols_v1.png"]` ｜ 可执行性：`[待实现]` ｜ 锚定：`TASK-3.03.01`/`3.03.02`/`3.04.01`/`3.05.01`/`3.05.02`/`3.05.03`/`3.06.01`/`3.06.02` ｜ 锚点：各 TASK 的规划锚点 ｜ 背书：各功能套件（待落地）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：按子项分别准备（`StagedVariant` fixture / 造词序列 / 符号表 / compose 键）。
- **操作步骤与物理存盘动作**：
  1. 重码审计报告产出 + 上限策略生效 → `execution_log.txt`。
  2. 热替换：输入中换库（当前词继续、下一键新库）；损坏候选库报 `dict/swap/rejected` 保旧 → 存 `01_swap_live.png`。
  3. 造词：两次选择后第三词首选；混输：`github` 进英文档；错音提示：annotation 含标准读法；符号：`v1`/`vx` 候选 → 存 `02_symbols_v1.png`。
  4. 桥接：无 preedit capability 的 IC 无内联串；切走收尾按配置执行 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：每个子项各自 TASK 卡的 DoD 断言在本卡逐项复核（失败隔离，子项独立记账）。
  - **[视觉断言]**：符号候选走既有网格分页；annotation 提示不影响布局稳定性。
  - **[性能断言]**：各自独占预算（features-add §3.4 汇总 ≤1.80ms 增量）。
