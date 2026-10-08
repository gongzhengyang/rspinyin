# rspinyin 测试用例分片 · core（解码引擎深化）

> 分片版本: v2.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 平台任务: [../features-test.md](../features-test.md) ｜
> 被测基线: Rust 2024 workspace（`ime-core` 纯函数、无 IO）+ Fcitx5 5.1.19 ｜ 关联 ADR: [../adr/0001-frozen-boundary-contracts.md](../adr/0001-frozen-boundary-contracts.md)、[../adr/0005-incremental-contract-extension.md](../adr/0005-incremental-contract-extension.md) ｜ 最后同步 Commit: `408d2c6` ｜
> 维护约定: 新增用例必须回写主文档第 2 节矩阵的 TC 列与维度列

## 0. 分片基线（引用主文档，不重复定义）

- **假设清单**：主文档第 1 节 + [features-test.md](../features-test.md) 第 1 节。强相关：`ASM-T-10`（`raw ≤ 64`、候选 ≤ 45、单候选 ≤ 32 字符）。
- **追踪矩阵**：主文档第 2 节的 `REQ-CORE-01` ~ `REQ-CORE-10`、`REQ-RT-06`。`REQ-CORE-08`（自定义短语）/ `REQ-CORE-09`（模糊音）/ `REQ-CORE-10`（简拼）是 features-add 增量落地行。
- **用例格式与证据存盘**：主文档第 3 节 + `FEAT-TEST-P0.05.04`。
- **预算阈值**：`docs/dev/budgets.json`（键名：`decode_p99`、`decode_p999`、`raster_p99`）。
- **确定性纪律**：`ime-core` 是纯函数（0.4 规则 4）——不碰文件系统、时钟、环境变量。全部用例通过 `FEAT-TEST-P0.03.01` 的内存替身（`MockLexicon`/`MockUserFreq`/`MockLm`）驱动，**无显示服务器、无词库文件**。

> 现有基线：`ime-core` 已有 **556 个 `#[test]`**、4 个 criterion 基准（`benches/{decode,input,passthrough}.rs`）、`proptest` 用于输入缓冲。本分片在其之上补充**场景化、跨模块、可数据化**的用例与增量解码域。

---

## 1. 用例

### TC-CORE-31 Preedit 文本与 span 全覆盖（`REQ-CORE-07`）

- **基本属性**：`[x] 已通过` ｜ `REQ-CORE-07` ｜ `core` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/preedit.rs`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 对 `ni`/`nihao`/`ni'hao'a`/`zhongguo` 四个输入生成 `Preedit` -> 触发存盘：`<RUN>/core/TC-CORE-31/assertions.json`
  2. 断言四条不变量：`spans` 按 `start` 严格升序、两两不重叠、**完整覆盖** `[0, text.len())`、`caret` 落在 UTF-8 字符边界。
- **通过标准**：四条不变量成立；分隔符只在**最优切分**的边界插入（`nihao` 显示为 `ni'hao`）。

- **验收记录**（2026-10-06）：preedit 套件 23/23 通过：四条不变量（严格升序/不重叠/完整覆盖/caret 边界）与最优切分边界分隔符（nihao→ni'hao）全断言；证据包 results/runs/run-20261006-034915/core/TC-CORE-31/
### TC-CORE-32 Preedit 的 `SpanKind` 分布（`REQ-CORE-07`）

- **基本属性**：`[x] 已通过` ｜ `REQ-CORE-07` ｜ `core` | 商业化 5 态微交互与材质 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 对 `ni'hao'a` 生成 `Preedit` -> 触发存盘：`<RUN>/core/TC-CORE-32/assertions.json`
  2. 断言每个音节一个 `SpanKind::Syllable`、每个 `'` 一个 `SpanKind::Separator`、caret 处一个零宽 `SpanKind::Cursor`。
- **通过标准**：`SpanKind` 四变体（`Syllable`/`Separator`/`Passthrough`/`Cursor`）各有明确出现条件；`Cursor` 在末尾时 `start == end == text.len()`。

- **验收记录**（2026-10-06）：SpanKind 四变体的出现条件由固定输入集快照测试与 proptest（任意会话不变量）双重钉住（preedit 23/23）；证据包 results/runs/run-20261006-034915/core/TC-CORE-32/
### TC-CORE-33 Preedit 无路径时退化为单一 Passthrough（`REQ-CORE-07`）

- **基本属性**：`[x] 已通过` ｜ `REQ-CORE-07` ｜ `core` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 对无路径输入（`zzz`）生成 `Preedit` -> 触发存盘：`<RUN>/core/TC-CORE-33/assertions.json`
  2. 断言整串为单个 `SpanKind::Passthrough`，四条不变量仍成立。
- **通过标准**：切分失败时**不返回 `None`**，退化为整串 span。

- **验收记录**（2026-10-06）：切分失败退化为整串 Passthrough 而非 None，四条不变量在 proptest 任意会话下仍成立；证据包 results/runs/run-20261006-034915/core/TC-CORE-33/
### TC-CORE-34 空输入的 Preedit（`REQ-CORE-07`）

- **基本属性**：`[x] 已通过` ｜ `REQ-CORE-07` ｜ `core` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 生成空输入的 `Preedit` -> 触发存盘：`<RUN>/core/TC-CORE-34/assertions.json`
  2. 断言 `text == ""`、`caret == 0`、`spans` 为空（**不返回 `None`**，简化 UI 侧逻辑）。
- **通过标准**：空态明确，UI 无需判空分支。

- **验收记录**（2026-10-06）：空输入返回空 preedit（text=''/caret=0/spans 空，非 None），UI 无需判空分支；证据包 results/runs/run-20261006-034915/core/TC-CORE-34/
### TC-CORE-35 Preedit 长度上限与耗时（`REQ-CORE-07`）

- **基本属性**：`[x] 已通过` ｜ `REQ-CORE-07` ｜ `core` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 以 64 字节 `raw` 生成 `Preedit`（最多插入 32 个分隔符）-> 触发存盘：`<RUN>/core/TC-CORE-35/assertions.json`
  2. 断言 `text.len() <= 96`；断言耗时 ≤ 200µs。
- **通过标准**：属性测试（`proptest`）10000 次随机状态下四条不变量无一违反。

- **验收记录**（2026-10-06）：proptest 覆盖最大长度状态下的长度不变量，build_preedit_into 复用调用方缓冲（热路径零重分配）；证据包 results/runs/run-20261006-034915/core/TC-CORE-35/
### TC-CORE-36 会话状态机跃迁表逐行覆盖（`REQ-RT-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-06` ｜ `core` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/state/{mod,machine,paging}.rs`
- **前置条件与沙盒状态**：`FEAT-TEST-P0.03.01` 的引擎直驱。
- **操作步骤**：
  1. 把 `features.md` 2.3 的状态机表**逐行**转成 `(初态, 事件) → (次态, 期望 Effect 变体序列)` 用例 -> 触发存盘：`<RUN>/core/TC-CORE-36/assertions.json`
  2. 断言 24 类 `(SessionState × SessionEvent)` 组合全部有明确次态，无 `unreachable!` 被触发。
- **通过标准**：`SessionState` 四态（`Idle`/`Composing`/`Cancelling`/`Committing`）全部可达；`step` 为**纯函数**（不执行 IO、不访问时钟、只产出 `Effect`）。

- **验收记录**（2026-10-06）：state 套件 134/134：test_step_covers_every_state_and_event_pair 全组合逐行断言且与设计跃迁表一致、按键动作全状态覆盖、effect 数量有预算；证据包 results/runs/run-20261006-034915/core/TC-CORE-36/
### TC-CORE-37 翻页边界不环绕（`REQ-RT-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-06` ｜ `core` | 边界与容错 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 首页注入 `PageDir::Prev`、末页注入 `PageDir::Next` -> 触发存盘：`<RUN>/core/TC-CORE-37/assertions.json`
  2. 断言 `flip` 返回 `false` 且状态不变（**不环绕**）。
- **通过标准**：避免用户误触回到首页；首页向上滚由 `DismissReason::ScrollUpEmpty` 承接（见 `TC-UI-33`）。

- **验收记录**（2026-10-06）：首页 Prev 与末页 Next 均原地不动且 flip 返回 false（不环绕）；证据包 results/runs/run-20261006-034915/core/TC-CORE-37/
### TC-CORE-38 `reconcile` 按文本保持高亮（`REQ-RT-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-06` ｜ `core` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 高亮第 3 项后重新解码（该词仍在列表中）-> 触发存盘：`<RUN>/core/TC-CORE-38/assertions.json`
  2. 断言高亮**跟随同一文本**；该词不在列表中时归零。
- **通过标准**：`features.md` 6.2.2 的"重新解码后高亮重置"陷阱；`page` 与 `highlight` 同步调整。

- **验收记录**（2026-10-06）：重新解码后高亮跟随同一文本（词可见时保持，不可见时归零），page/highlight 同步调整；证据包 results/runs/run-20261006-034915/core/TC-CORE-38/
### TC-CORE-39 `revision` 单调递增与过期帧丢弃（`REQ-RT-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-06` ｜ `core` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 连续产出多个 `SendFrame` -> 触发存盘：`<RUN>/core/TC-CORE-39/assertions.json`
  2. 断言 `revision` 单调递增（`Revision::next()` 回绕到 1 并跳过 0）。
  3. 注入 `revision` 不匹配的 `UiEvent::Select`，断言被丢弃并产生 `Effect::Diagnose(ImeError::UiStaleSelect)`。
- **通过标准**：`revision` 是防竞态的唯一手段。

- **验收记录**（2026-10-06）：revision 语义三断言：匹配即提交、同 revision 重复选择只提交一次、过期 UI 事件带 UiStaleSelect 诊断丢弃；证据包 results/runs/run-20261006-034915/core/TC-CORE-39/
### TC-CORE-40 焦点丢失不提交候选（`REQ-RT-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-06` ｜ `core` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. `Composing` 态注入 `FocusLost` -> 触发存盘：`<RUN>/core/TC-CORE-40/assertions.json`
  2. 断言次态为 `Idle`、产生 `Effect::Hide(HideReason::FocusLost)`、**不产生任何 `Effect::Commit`**。
- **通过标准**：焦点丢失是最严重的缺陷场景，绝不能顺带上屏。

- **验收记录**（2026-10-06）：Composing 态 FocusLost → Idle + Hide(FocusLost) 且零 Commit；Committing 态 FocusLost 保留已提交并回 Idle；证据包 results/runs/run-20261006-034915/core/TC-CORE-40/
### TC-CORE-41 配置热重载不重置会话（`REQ-RT-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-06` ｜ `core` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. `Composing` 态注入 `ConfigReloaded` -> 触发存盘：`<RUN>/core/TC-CORE-41/assertions.json`
  2. 断言 `raw` 与候选列表**不变**，仅行为派生值更新。
- **通过标准**：0.4 规则 10（`AGENTS.md` 第 8 条第 23 项）；`max_raw_len` 变小时保留 `raw`，只在下次 `push_char` 生效。

- **验收记录**（2026-10-06）：Composing 态注入 ConfigReloaded 后 raw 与候选列表不变、高亮保持同词、仅行为派生值更新；无变化重载幂等；证据包 results/runs/run-20261006-034915/core/TC-CORE-41/
### TC-CORE-42 双字词优先于单字（`REQ-CORE-04` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-CORE-04` ｜ `core` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/viterbi/decoder.rs`
- **操作步骤**：
  1. 构造单字与双字权重接近的词库 -> 触发存盘：`<RUN>/core/TC-CORE-42/assertions.json`
  2. 断言双字词因 `λ_len` 长度奖励胜出（默认 `λ_len = 0.35`）。
- **通过标准**：长度奖励按 `(char_count − 1) × 256` 计算，Q8.8 定点。

- **验收记录**（2026-10-06）：viterbi+lm 套件 125/125：长度奖励 (char_count−1)×256 Q8.8 定点使双字词在权重接近时胜出；证据包 results/runs/run-20261006-034915/core/TC-CORE-42/
### TC-CORE-43 段数惩罚偏好少切分（`REQ-CORE-04` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-CORE-04` ｜ `core` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 构造"整词"与"两段"两条路径且 LM 分数接近 -> 触发存盘：`<RUN>/core/TC-CORE-43/assertions.json`
  2. 断言整词因段数惩罚（默认 `λ_seg = 0.5`）胜出。
- **通过标准**：段数惩罚为 `−λ_seg × seg_count × 256`。

- **验收记录**（2026-10-06）：段数惩罚 −λ_seg×seg_count×256（默认 0.5）使整词路径在 LM 分数接近时胜过两段；证据包 results/runs/run-20261006-034915/core/TC-CORE-43/
### TC-CORE-44 单字兜底保证任意输入有候选（`REQ-CORE-04` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-CORE-04` ｜ `core` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 对词库中**不存在**的合法音节组合解码 -> 触发存盘：`<RUN>/core/TC-CORE-44/assertions.json`
  2. 断言走 `fallback_single` 且候选非空。
- **通过标准**：`Lexicon::fallback_single` 的契约——每个音节至少给出 1 个候选。

- **验收记录**（2026-10-06）：fallback_single 契约（每音节至少 1 候选）+ 空词库降级非空候选双断言通过；证据包 results/runs/run-20261006-034915/core/TC-CORE-44/
### TC-CORE-45 `beam_k` 与 `max_candidates` 的配置校验（`REQ-CORE-04` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-CORE-04` ｜ `core` | 边界与容错 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 构造 `beam_k = 33` 与 `max_candidates = 65` -> 触发存盘：`<RUN>/core/TC-CORE-45/assertions.json`
  2. 断言 `DecodeConfig::validate()` 返回 `config/invalid`（上限 32 / 64）。
- **通过标准**：配置校验在构造期完成，不留到运行期。

- **验收记录**（2026-10-06）：DecodeConfig::validate 在构造期拒绝 beam_k=33 / max_candidates=65（上限 32/64），错误码 config/invalid；证据包 results/runs/run-20261006-034915/core/TC-CORE-45/
### TC-CORE-46 场景数据化：TOML 场景文件加载（`REQ-CORE-04` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-CORE-04` ｜ `core` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]`（引擎直驱通道已随 FEAT-TEST 平台交付）
- **操作步骤**：
  1. 加载 `tests/fixtures/scenarios/*.toml` -> 触发存盘：`<RUN>/core/TC-CORE-46/assertions.json`
  2. 断言每个场景可被 `run_scenario` 执行，失败时 `Divergence` 指出步骤序号、期望值、实际值。
- **通过标准**：用例以**数据**而非代码表达，便于非 Rust 背景评审。

- **验收记录**（2026-10-06）：场景数据化：testd-engine --dump 物化为 tests/fixtures/scenarios/ 的 18 个 TOML 场景文件，--scenarios 加载后 18 场景 × 2 次重放 0 diverged（含五类冻结错误码）；证据包 results/runs/run-20261006-034915/core/TC-CORE-46/
### TC-CORE-47 引擎直驱不触碰文件系统与时钟（`REQ-CORE-04` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-CORE-04` ｜ `core` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 用 `strace -f -e trace=openat,clock_gettime` 跑全部场景 -> 触发存盘：`<RUN>/core/TC-CORE-47/assertions.json`
  2. 断言无相关系统调用（`ime-core` 是纯函数）。
- **通过标准**：`InputBuffer` 的时间戳经 `mark_session_start(at_unix_ms)` 由调用方注入（`ime-core` 不读时钟）。

- **验收记录**（2026-10-06）：strace 动态探针：引擎直驱在 DISPLAY/WAYLAND_DISPLAY 移除下运行，追踪证明既未打开仓库文件也未读时钟（0.4 规则 4 的机器化验证）；证据包 results/runs/run-20261006-034915/core/TC-CORE-47/
### TC-CORE-48 直通判定的纯函数性（`REQ-CORE-06` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-CORE-06` ｜ `core` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 对同一 `(raw, flags, surrounding)` 三元组重复调用 `classify` 100 次 -> 触发存盘：`<RUN>/core/TC-CORE-48/assertions.json`
  2. 断言结果全等；断言 `surrounding = None` 时行为与"最保守"分支一致。
- **通过标准**：判定为纯函数，无隐藏状态。

- **验收记录**（2026-10-06）：classify 为纯函数：同输入重复调用结果全等，surrounding=None 时行为与最保守分支一致，无隐藏状态；证据包 results/runs/run-20261006-034915/core/TC-CORE-48/
### TC-CORE-49 双引号配对依赖上下文（`REQ-CORE-06` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-CORE-06` ｜ `core` | 边界与容错 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 分别以"未闭合引号数为奇数/偶数/宿主未报告"三种上下文调用 -> 触发存盘：`<RUN>/core/TC-CORE-49/assertions.json`
  2. 断言配对为 `“`/`”`；无上下文时退化为开引号。
- **通过标准**：无击键记忆时用未闭合引号计数配对（`.dev-progress.json` 记录的设计决策）。

- **验收记录**（2026-10-06）：双引号配对：未闭合引号奇偶计数决定 “/” 配对，宿主未报告上下文时退化为开引号；证据包 results/runs/run-20261006-034915/core/TC-CORE-49/
### TC-CORE-50 标点表与全角表的常量性（`REQ-CORE-06` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-CORE-06` ｜ `core` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 检查标点替换表与全角映射的实现 -> 触发存盘：`<RUN>/core/TC-CORE-50/assertions.json`
  2. 断言为 `const` 数组 + `match`，**不含 `HashMap`**（避免首键延迟抖动）。
- **通过标准**：`grep -n 'HashMap' crates/ime-core/src/passthrough.rs` 无输出；`classify` ≤ 500ns。

### TC-CORE-51 自定义短语引擎：导入、命中与置顶（`REQ-CORE-08`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-08` ｜ `core` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/phrase.rs`、`phrase/writer.rs`
- **前置条件与沙盒状态**：内存替身 `MockLexicon` + 短语表注入（`FEAT-TEST-P0.03.02` 同族）。
- **操作步骤**：
  1. 装载短语表（缩写 → 词 + 权重），输入缩写串解码 -> 触发存盘：`<RUN>/core/TC-CORE-51/assertions.json`
  2. 断言短语出现在候选且排序高于词库默认权重、低于用户频次调权。
  3. 删除一条短语（`writer.rs` 写回），断言候选即时更新且无残留。
- **通过标准**：短语解析是纯函数；非法行（缺列、超长、非 UTF-8）跳过并计数，不阻断装载；候选序 100 次逐字节一致。

### TC-CORE-52 短语与用户频次、词库权重的三方排序（`REQ-CORE-08`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-08` ｜ `core` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/phrase.rs`、`lm/score.rs`
- **前置条件与沙盒状态**：内存替身三方注入（短语 / 用户频次 / 词库权重）。
- **操作步骤**：
  1. 构造同一缩写同时命中三来源的场景 -> 触发存盘：`<RUN>/core/TC-CORE-52/assertions.json`
  2. 断言排序确定为：用户高频 > 短语 > 词库默认；权重相等时以词库序稳定裁决（无浮点比较）。
- **通过标准**：排序全部走 Q8.8 定点路径（0.4 规则 4 / 3.8）；无 `f32` 参与顺序决策。

### TC-CORE-53 短语边界：超长词、26 键上限与重复缩写（`REQ-CORE-08`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-08` ｜ `core` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/phrase.rs`、`crates/ime-config/src/schema.rs`（`CUSTOM_TABLE_KEYS`）
- **前置条件与沙盒状态**：内存替身。
- **操作步骤**：
  1. 注入 32+ 字符短语、第 27 个自定义缩写、重复缩写不同词三组边界 -> 触发存盘：`<RUN>/core/TC-CORE-53/assertions.json`
  2. 断言：超长词被拒或截断告警；超上限返回类型化错误；重复缩写由权重高者胜且确定性。
- **通过标准**：与 `CUSTOM_TABLE_KEYS = 26` 的 schema 上限一致；全部边界命中类型化错误而非 panic。

### TC-CORE-54 短语写入器的原子性与崩溃安全（`REQ-CORE-08`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-08` ｜ `core` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/phrase/writer.rs`
- **前置条件与沙盒状态**：临时目录；失败注入（rename 目标为目录）。
- **操作步骤**：
  1. 正常写入 → 断言临时文件 + fsync + rename 链路 -> 触发存盘：`<RUN>/core/TC-CORE-54/assertions.json`
  2. 失败注入：断言 `.tmp` 被清理（`534e48a` 的 DictWriter 语义同族）、原文件完好。
- **通过标准**：任何失败路径不留下半写文件；`check-fsync-rename.sh` 对该路径零命中。

### TC-CORE-55 短语开启与关闭的零开销路径（`REQ-CORE-08`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-08` ｜ `core` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/phrase.rs`、`decode.rs`
- **前置条件与沙盒状态**：空闲机器（`ASM-T-11`）；criterion。
- **操作步骤**：
  1. 关闭短语功能解码同一输入 10⁴ 次 -> 触发存盘：`<RUN>/core/TC-CORE-55/assertions.json`
  2. 断言与未装载短语表的基线无可测量差异（±噪声带内）。
- **通过标准**：关闭态零查表（分支在解码路径外）；开启态增量计入 `ASM-A-02` 的 2.10ms 增量独占预算裁决。

### TC-CORE-56 模糊音八类别全表与替代拼写上限（`REQ-CORE-09`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-09` ｜ `core` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/fuzzy.rs`
- **前置条件与沙盒状态**：内存替身；`ASM-A-06`（每音节 ≤ 8 替代拼写）。
- **操作步骤**：
  1. 逐类别开启（zh/z、ch/c、sh/s、n/l、f/h、an/ang、in/ing、en/eng），对全音节表生成替代拼写 -> 触发存盘：`<RUN>/core/TC-CORE-56/assertions.json`
  2. 断言每音节替代数 ≤ 8 且生成是纯函数（两次运行逐字节一致）。
- **通过标准**：类别可独立开关；关闭类别零残留（替代表不进词格）。

### TC-CORE-57 模糊音命中：双向候选与确定性排序（`REQ-CORE-09`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-09` ｜ `core` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/fuzzy.rs`、`viterbi/decoder.rs`
- **前置条件与沙盒状态**：内存替身；词库含模糊对两侧词汇。
- **操作步骤**：
  1. 输入 `nan`（n/l 模糊开启）断言 `男`/`蓝` 两侧词均可达 -> 触发存盘：`<RUN>/core/TC-CORE-57/assertions.json`
  2. 同输入 100 次，断言候选序逐字节一致。
- **通过标准**：模糊边在词格中有成本惩罚（精确拼写压过模糊拼写）；无浮点。

### TC-CORE-58 模糊音全开的解码预算（`REQ-CORE-09`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-09` ｜ `core` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/viterbi/`、`budgets.json`
- **前置条件与沙盒状态**：空闲机器（`ASM-T-11`）；12 音节长输入。
- **操作步骤**：
  1. 全类别开启，跑 `decode` 基准 -> 触发存盘：`<RUN>/core/TC-CORE-58/assertions.json`
  2. 断言 P99 ≤ `decode_p99`（3.0ms）；词格路径数上限（`MAX_SYL_COUNT = 16`、`MAX_WORDS_PER_KEY = 32`）未被突破。
- **通过标准**：`ASM-A-02`/`ASM-A-06` 的降级顺序（先降类别数 → 再收 beam → 最后关默认值）未被触发即达标。

### TC-CORE-59 模糊音 × 词库注入的畸形兼容（`REQ-CORE-09`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-09` ｜ `core` | 全状态防御与骨架屏 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/fuzzy.rs`、`segment/dag.rs`
- **前置条件与沙盒状态**：内存替身；畸形音节串。
- **操作步骤**：
  1. 模糊开启下注入非法串（无路径、超长、非法字符）-> 触发存盘：`<RUN>/core/TC-CORE-59/assertions.json`
  2. 断言错误码不变（`decode/no-path`、`decode/too-long`、`decode/invalid-char`）——模糊层不吞错误也不新增错误码。
- **通过标准**：模糊层是词格边的扩充，不改切分 DAG 的错误语义。

### TC-CORE-60 模糊音的配置接线与诊断（`REQ-CORE-09`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-09` ｜ `core` | 全状态防御与骨架屏 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/schema.rs`、`crates/ime-core/src/fuzzy.rs`
- **前置条件与沙盒状态**：沙盒配置 + 日志落盘。
- **操作步骤**：
  1. 配置非法模糊类别名 -> 触发存盘：`<RUN>/core/TC-CORE-60/assertions.json`
  2. 断言 schema 拒绝且日志含稳定错误码、无输入内容。
- **通过标准**：类别名是封闭枚举；未知名不可静默开启。

### TC-CORE-61 简拼缩写展开：词格遍历与命中（`REQ-CORE-10`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-10` ｜ `core` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/viterbi/lattice/abbrev.rs`、`lattice.rs`
- **前置条件与沙盒状态**：内存替身；`FEAT-TEST-P0.03.02` 词库注入。
- **操作步骤**：
  1. 输入首字母串 `nh`，断言"你好"在前 9 候选内 -> 触发存盘：`<RUN>/core/TC-CORE-61/01_default.png`
  2. 混输 `n'hao`（缩写 + 全拼），断言仍命中 -> 触发存盘：`<RUN>/core/TC-CORE-61/02_mixed.png`
- **通过标准**：缩写边有成本惩罚（`lattice.rs` 的 equal-dictionary-weight 语义：等词频时全拼压过缩写）；展开只发生在词格遍历内，不改切分。

### TC-CORE-62 简拼成本上限：四轴有界（`REQ-CORE-10`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-10` ｜ `core` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/viterbi/lattice/abbrev.rs`
- **前置条件与沙盒状态**：空闲机器（`ASM-T-11`）。
- **操作步骤**：
  1. 全缩写输入（8 个单字母）解码 -> 触发存盘：`<RUN>/core/TC-CORE-62/assertions.json`
  2. 断言 P99 ≤ `decode_p99`，且缩写遍历的四轴成本上限（`lattice.rs` 文档声明）逐一成立。
- **通过标准**：无路径爆炸；长串缩写退化为确定性截断而非失控遍历。

### TC-CORE-63 简拼与用户频次的交互（`REQ-CORE-10`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-10` ｜ `core` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/viterbi/lattice/abbrev.rs`、`lm/score.rs`
- **前置条件与沙盒状态**：内存替身 `MockUserFreq`。
- **操作步骤**：
  1. 对同一缩写注入用户高频词 -> 触发存盘：`<RUN>/core/TC-CORE-63/assertions.json`
  2. 断言用户词提升到缩写候选首位，且移除频次后回到词库序。
- **通过标准**：频次调权作用于缩写边代价而非绕过它；两次状态切换后候选序完全可复现。

### TC-CORE-64 简拼关闭态与开关配置（`REQ-CORE-10`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-10` ｜ `core` | 全状态防御与骨架屏 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/schema.rs`、`crates/ime-core/src/viterbi/lattice.rs`
- **前置条件与沙盒状态**：内存替身；配置开关。
- **操作步骤**：
  1. 关闭简拼后输入 `nh` -> 触发存盘：`<RUN>/core/TC-CORE-64/assertions.json`
  2. 断言按普通音节处理（n + h 声母行为），不产生缩写候选；开关即时生效（经 `REQ-CFG-03` 重载链路）。
- **通过标准**：关闭态零缩写遍历（分支在词格构建外）。

### TC-CORE-65 简拼增量属性的 proptest（`REQ-CORE-10`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CORE-10` ｜ `core` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/viterbi/lattice/abbrev.rs`
- **前置条件与沙盒状态**：proptest；内存替身。
- **操作步骤**：
  1. 随机缩写串 10⁴ 例 -> 触发存盘：`<RUN>/core/TC-CORE-65/assertions.json`
  2. 断言不变量：候选 ≤ 45、单候选 ≤ 32 字符、无 panic、解码确定（同输入同输出）。
- **通过标准**：`ASM-T-10` 的三条边界在缩写路径同样成立；失败样本可最小化。

---

## 2. 分片出口准则

1. `REQ-CORE-07` 与 `REQ-RT-06` 的 10 条用例已随 `TASK-1.02.05` / `TASK-1.03.07` 落地转为 `[可执行]`（2026-10-06 全量轮已执行，见各用例验收记录）。
2. `TC-CORE-47` 断言引擎直驱无文件系统与时钟调用（0.4 规则 4 的机器化验证）。
3. 主文档矩阵的 `REQ-CORE-01`~`10`、`REQ-RT-06` 行可执行性列为 `✅`，维度列按用例覆盖勾选。
4. 场景文件 `tests/fixtures/scenarios/` 覆盖五个冻结错误码（`decode/empty-input`、`decode/too-long`、`decode/invalid-char`、`decode/no-path`、`config/invalid`）——18 个场景 TOML 已于全量轮实体化。
5. 增量域（`REQ-CORE-08`~`10`）的 15 条用例落地后须保持：短语/模糊音/简拼的关闭态零开销、开启态 `decode_p99` 达预算、全部路径无浮点排序。

- **验收记录**（2026-10-06）：标点替换表与全角映射为 const 数组 + match（passthrough.rs 无 HashMap），classify 纯查表无首键延迟；证据包 results/runs/run-20261006-034915/core/TC-CORE-50/
