# rspinyin 测试用例分片 · core（解码引擎深化）

> 分片版本: v1.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 平台任务: [../features-test.md](../features-test.md) ｜
> 被测基线: Rust 2024 workspace（`ime-core` 纯函数、无 IO）+ Fcitx5 5.1.7 ｜ 关联 ADR: [../adr/0001-frozen-boundary-contracts.md](../adr/0001-frozen-boundary-contracts.md) ｜ 最后同步 Commit: `ee0dbfb` ｜
> 维护约定: 新增用例必须回写主文档第 2 节矩阵的 TC 列与维度列

## 0. 分片基线（引用主文档，不重复定义）

- **假设清单**：主文档第 1 节 + [features-test.md](../features-test.md) 第 1 节。强相关：`ASM-T-10`（`raw ≤ 64`、候选 ≤ 45、单候选 ≤ 32 字符）。
- **追踪矩阵**：主文档第 2 节的 `REQ-CORE-01` ~ `REQ-CORE-07`、`REQ-RT-06`。
- **用例格式与证据存盘**：主文档第 3 节 + `FEAT-TEST-P0.05.04`。
- **预算阈值**：`docs/dev/budgets.json`（键名：`decode_p99`、`decode_p999`、`raster_p99`）。
- **确定性纪律**：`ime-core` 是纯函数（0.4 规则 4）——不碰文件系统、时钟、环境变量。全部用例通过 `FEAT-TEST-P0.03.01` 的内存替身（`MockLexicon`/`MockUserFreq`/`MockLm`）驱动，**无显示服务器、无词库文件**。

> 现有基线：`ime-core` 已有 **112 个 `#[test]`**、2 个 criterion 基准（`benches/{input,passthrough}.rs`）、`proptest` 用于输入缓冲。本分片在其之上补充**场景化、跨模块、可数据化**的用例。

---

## 1. 用例

### TC-CORE-31 Preedit 文本与 span 全覆盖（`REQ-CORE-07`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-07` ｜ `core` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.02.05]` ｜ `crates/ime-core/src/preedit.rs`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 对 `ni`/`nihao`/`ni'hao'a`/`zhongguo` 四个输入生成 `Preedit` -> 触发存盘：`<RUN>/core/TC-CORE-31/assertions.json`
  2. 断言四条不变量：`spans` 按 `start` 严格升序、两两不重叠、**完整覆盖** `[0, text.len())`、`caret` 落在 UTF-8 字符边界。
- **通过标准**：四条不变量成立；分隔符只在**最优切分**的边界插入（`nihao` 显示为 `ni'hao`）。

### TC-CORE-32 Preedit 的 `SpanKind` 分布（`REQ-CORE-07`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-07` ｜ `core` | 商业化 5 态微交互与材质 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.02.05]`
- **操作步骤**：
  1. 对 `ni'hao'a` 生成 `Preedit` -> 触发存盘：`<RUN>/core/TC-CORE-32/assertions.json`
  2. 断言每个音节一个 `SpanKind::Syllable`、每个 `'` 一个 `SpanKind::Separator`、caret 处一个零宽 `SpanKind::Cursor`。
- **通过标准**：`SpanKind` 四变体（`Syllable`/`Separator`/`Passthrough`/`Cursor`）各有明确出现条件；`Cursor` 在末尾时 `start == end == text.len()`。

### TC-CORE-33 Preedit 无路径时退化为单一 Passthrough（`REQ-CORE-07`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-07` ｜ `core` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.02.05]`
- **操作步骤**：
  1. 对无路径输入（`zzz`）生成 `Preedit` -> 触发存盘：`<RUN>/core/TC-CORE-33/assertions.json`
  2. 断言整串为单个 `SpanKind::Passthrough`，四条不变量仍成立。
- **通过标准**：切分失败时**不返回 `None`**，退化为整串 span。

### TC-CORE-34 空输入的 Preedit（`REQ-CORE-07`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-07` ｜ `core` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.02.05]`
- **操作步骤**：
  1. 生成空输入的 `Preedit` -> 触发存盘：`<RUN>/core/TC-CORE-34/assertions.json`
  2. 断言 `text == ""`、`caret == 0`、`spans` 为空（**不返回 `None`**，简化 UI 侧逻辑）。
- **通过标准**：空态明确，UI 无需判空分支。

### TC-CORE-35 Preedit 长度上限与耗时（`REQ-CORE-07`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-07` ｜ `core` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.02.05]`
- **操作步骤**：
  1. 以 64 字节 `raw` 生成 `Preedit`（最多插入 32 个分隔符）-> 触发存盘：`<RUN>/core/TC-CORE-35/assertions.json`
  2. 断言 `text.len() <= 96`；断言耗时 ≤ 200µs。
- **通过标准**：属性测试（`proptest`）10000 次随机状态下四条不变量无一违反。

### TC-CORE-36 会话状态机跃迁表逐行覆盖（`REQ-RT-06`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-RT-06` ｜ `core` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.03.07]` ｜ `crates/ime-core/src/state/{mod,machine,paging}.rs`
- **前置条件与沙盒状态**：`FEAT-TEST-P0.03.01` 的引擎直驱。
- **操作步骤**：
  1. 把 `features.md` 2.3 的状态机表**逐行**转成 `(初态, 事件) → (次态, 期望 Effect 变体序列)` 用例 -> 触发存盘：`<RUN>/core/TC-CORE-36/assertions.json`
  2. 断言 24 类 `(SessionState × SessionEvent)` 组合全部有明确次态，无 `unreachable!` 被触发。
- **通过标准**：`SessionState` 四态（`Idle`/`Composing`/`Cancelling`/`Committing`）全部可达；`step` 为**纯函数**（不执行 IO、不访问时钟、只产出 `Effect`）。

### TC-CORE-37 翻页边界不环绕（`REQ-RT-06`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-RT-06` ｜ `core` | 边界与容错 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.03.07]`
- **操作步骤**：
  1. 首页注入 `PageDir::Prev`、末页注入 `PageDir::Next` -> 触发存盘：`<RUN>/core/TC-CORE-37/assertions.json`
  2. 断言 `flip` 返回 `false` 且状态不变（**不环绕**）。
- **通过标准**：避免用户误触回到首页；首页向上滚由 `DismissReason::ScrollUpEmpty` 承接（见 `TC-UI-33`）。

### TC-CORE-38 `reconcile` 按文本保持高亮（`REQ-RT-06`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-RT-06` ｜ `core` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.03.07]`
- **操作步骤**：
  1. 高亮第 3 项后重新解码（该词仍在列表中）-> 触发存盘：`<RUN>/core/TC-CORE-38/assertions.json`
  2. 断言高亮**跟随同一文本**；该词不在列表中时归零。
- **通过标准**：`features.md` 6.2.2 的"重新解码后高亮重置"陷阱；`page` 与 `highlight` 同步调整。

### TC-CORE-39 `revision` 单调递增与过期帧丢弃（`REQ-RT-06`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-RT-06` ｜ `core` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.03.07]`
- **操作步骤**：
  1. 连续产出多个 `SendFrame` -> 触发存盘：`<RUN>/core/TC-CORE-39/assertions.json`
  2. 断言 `revision` 单调递增（`Revision::next()` 回绕到 1 并跳过 0）。
  3. 注入 `revision` 不匹配的 `UiEvent::Select`，断言被丢弃并产生 `Effect::Diagnose(ImeError::UiStaleSelect)`。
- **通过标准**：`revision` 是防竞态的唯一手段。

### TC-CORE-40 焦点丢失不提交候选（`REQ-RT-06`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-RT-06` ｜ `core` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.03.07]`
- **操作步骤**：
  1. `Composing` 态注入 `FocusLost` -> 触发存盘：`<RUN>/core/TC-CORE-40/assertions.json`
  2. 断言次态为 `Idle`、产生 `Effect::Hide(HideReason::FocusLost)`、**不产生任何 `Effect::Commit`**。
- **通过标准**：焦点丢失是最严重的缺陷场景，绝不能顺带上屏。

### TC-CORE-41 配置热重载不重置会话（`REQ-RT-06`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-RT-06` ｜ `core` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.03.07]`
- **操作步骤**：
  1. `Composing` 态注入 `ConfigReloaded` -> 触发存盘：`<RUN>/core/TC-CORE-41/assertions.json`
  2. 断言 `raw` 与候选列表**不变**，仅行为派生值更新。
- **通过标准**：0.4 规则 10（`AGENTS.md` 第 8 条第 23 项）；`max_raw_len` 变小时保留 `raw`，只在下次 `push_char` 生效。

### TC-CORE-42 双字词优先于单字（`REQ-CORE-04` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-04` ｜ `core` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/viterbi/decoder.rs`
- **操作步骤**：
  1. 构造单字与双字权重接近的词库 -> 触发存盘：`<RUN>/core/TC-CORE-42/assertions.json`
  2. 断言双字词因 `λ_len` 长度奖励胜出（默认 `λ_len = 0.35`）。
- **通过标准**：长度奖励按 `(char_count − 1) × 256` 计算，Q8.8 定点。

### TC-CORE-43 段数惩罚偏好少切分（`REQ-CORE-04` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-04` ｜ `core` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 构造"整词"与"两段"两条路径且 LM 分数接近 -> 触发存盘：`<RUN>/core/TC-CORE-43/assertions.json`
  2. 断言整词因段数惩罚（默认 `λ_seg = 0.5`）胜出。
- **通过标准**：段数惩罚为 `−λ_seg × seg_count × 256`。

### TC-CORE-44 单字兜底保证任意输入有候选（`REQ-CORE-04` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-04` ｜ `core` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 对词库中**不存在**的合法音节组合解码 -> 触发存盘：`<RUN>/core/TC-CORE-44/assertions.json`
  2. 断言走 `fallback_single` 且候选非空。
- **通过标准**：`Lexicon::fallback_single` 的契约——每个音节至少给出 1 个候选。

### TC-CORE-45 `beam_k` 与 `max_candidates` 的配置校验（`REQ-CORE-04` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-04` ｜ `core` | 边界与容错 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 构造 `beam_k = 33` 与 `max_candidates = 65` -> 触发存盘：`<RUN>/core/TC-CORE-45/assertions.json`
  2. 断言 `DecodeConfig::validate()` 返回 `config/invalid`（上限 32 / 64）。
- **通过标准**：配置校验在构造期完成，不留到运行期。

### TC-CORE-46 场景数据化：TOML 场景文件加载（`REQ-CORE-04` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-04` ｜ `core` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[待实现: FEAT-TEST-P0.03.01]`
- **操作步骤**：
  1. 加载 `tests/fixtures/scenarios/*.toml` -> 触发存盘：`<RUN>/core/TC-CORE-46/assertions.json`
  2. 断言每个场景可被 `run_scenario` 执行，失败时 `Divergence` 指出步骤序号、期望值、实际值。
- **通过标准**：用例以**数据**而非代码表达，便于非 Rust 背景评审。

### TC-CORE-47 引擎直驱不触碰文件系统与时钟（`REQ-CORE-04` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-04` ｜ `core` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 用 `strace -f -e trace=openat,clock_gettime` 跑全部场景 -> 触发存盘：`<RUN>/core/TC-CORE-47/assertions.json`
  2. 断言无相关系统调用（`ime-core` 是纯函数）。
- **通过标准**：`InputBuffer` 的时间戳经 `mark_session_start(at_unix_ms)` 由调用方注入（`ime-core` 不读时钟）。

### TC-CORE-48 直通判定的纯函数性（`REQ-CORE-06` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-06` ｜ `core` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 对同一 `(raw, flags, surrounding)` 三元组重复调用 `classify` 100 次 -> 触发存盘：`<RUN>/core/TC-CORE-48/assertions.json`
  2. 断言结果全等；断言 `surrounding = None` 时行为与"最保守"分支一致。
- **通过标准**：判定为纯函数，无隐藏状态。

### TC-CORE-49 双引号配对依赖上下文（`REQ-CORE-06` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-06` ｜ `core` | 边界与容错 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 分别以"未闭合引号数为奇数/偶数/宿主未报告"三种上下文调用 -> 触发存盘：`<RUN>/core/TC-CORE-49/assertions.json`
  2. 断言配对为 `“`/`”`；无上下文时退化为开引号。
- **通过标准**：无击键记忆时用未闭合引号计数配对（`.dev-progress.json` 记录的设计决策）。

### TC-CORE-50 标点表与全角表的常量性（`REQ-CORE-06` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-06` ｜ `core` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 检查标点替换表与全角映射的实现 -> 触发存盘：`<RUN>/core/TC-CORE-50/assertions.json`
  2. 断言为 `const` 数组 + `match`，**不含 `HashMap`**（避免首键延迟抖动）。
- **通过标准**：`grep -n 'HashMap' crates/ime-core/src/passthrough.rs` 无输出；`classify` ≤ 500ns。

---

## 2. 分片出口准则

1. `REQ-CORE-07` 与 `REQ-RT-06` 的 10 条用例在 `TASK-1.02.05` / `TASK-1.03.07` 落地后转为 `[可执行]` 并通过。
2. `TC-CORE-47` 断言引擎直驱无文件系统与时钟调用（0.4 规则 4 的机器化验证）。
3. 主文档矩阵的 `REQ-CORE-07`、`REQ-RT-06` 行可执行性列更新为 `✅`，维度列全部勾选。
4. 场景文件 `tests/fixtures/scenarios/` 覆盖五个冻结错误码（`decode/empty-input`、`decode/too-long`、`decode/invalid-char`、`decode/no-path`、`config/invalid`）。
