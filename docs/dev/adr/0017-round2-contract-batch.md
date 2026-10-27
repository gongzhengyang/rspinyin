# ADR-0017：Round-2 契约增量批次——`CandidateSource::Assoc` 与 `KeyAction::{SelectFirstChar, SelectLastChar, ReduceFrequency, HideCandidate}`

## 状态

已接受。

## 背景

Round-2 的三块能力——联想（`GAP-22`）、以词定字（`GAP-29`）、用户负反馈三态补齐（`GAP-30`）——各自都要向冻结契约（`crates/ime-types`）追加枚举变体。按 `AGENTS.md` §8.22，每一处冻结契约变更都需要 ADR 与主 Agent 决策；若三块能力各自走一次冻结程序，会带来三份 ADR、三次同文件（`ui.rs`/`key.rs`）的串行改写，以及消费方卡之间的契约等待与热点冲突（§8.4）。本卡（`docs/dev/features-add-phase4.md` 的 `TASK-4.01.01`）把三块能力的契约增量收敛为一次裁决、一次枚举扩展，使 `TASK-4.04.01`（联想引擎侧）、`TASK-6.02.01`（以词定字）、`TASK-6.03.01`（降频与隐藏）在契约落盘后完全并行。

变体在定义与消费之间需要一段"先有类型、后有线路"的过渡期。先例是 `Symbol` 与 ADR-0016 的 `English`：契约先落盘、实现随后接入，期间由编译器的穷举性检查列出全部必须决策的位置。

## 决策

1. `CandidateSource` 追加 `Assoc` 变体，序位 8（`source_code` 数值线：`Dict=0`…`English=7`，`Assoc=8`；`ime-ui`、两个 FFI transport 的数值映射随本 ADR 补臂）。语义：commit 之后由联想源产出的"下一词"候选。候选复用既有 `Candidate` wire 结构体与候选窗控件；`annotation` 携带「联想」标签（既有字段，无 wire 变更），UI 侧按来源序号标注，不新增帧字段。
2. `KeyAction` 追加四个变体（序位接在 `AddPhrase` 之后，既有变体零改动）：
   - `SelectFirstChar`：提交高亮候选的首字（以词定字，`[`）。
   - `SelectLastChar`：提交高亮候选的尾字（以词定字，`]`）。
   - `ReduceFrequency`：降低高亮候选的用户词权重（降频，可被后续正常使用恢复，区别于 `ForgetHighlighted` 的删除）。
   - `HideCandidate`：本会话内不再产出高亮候选（隐藏，不落盘；跨会话隐藏是 `ForgetHighlighted` 的职责）。
3. `ime-config` 新增 `[assoc]` 配置节骨架：`enabled: bool = false`、`max_candidates: u8 = 5`、`history_depth: u8 = 8`，默认全关。`[assoc]` 是 `ime-config` 的内部增项（非冻结契约），随 crate 同版本发布；文档模板 `DEFAULT_CONFIG_TOML`、坏值修默认范围与配置投影（`SessionConfig`）由联想接线卡在其可读时落地——本卡只建立带默认值的类型面，保证消费方卡有稳定的投影目标。
4. 语义冻结（与 ADR-0014/0016 同构）：过渡期内每个新变体在全部既有 `match` 点的默认臂等于现状行为——`Assoc` 候选无产出方（无任何解码路径构造它）；四个 `KeyAction` 无键位路由（`translate_key` 不可能产出它们），会话收到时以 `Effect::Diagnose(ImeError::Unsupported)`（`dict/unsupported`）作答并消费按键，与 `PinHighlighted` 在置顶实现落地前的过渡臂同例。因此本 ADR 独立合入后，默认配置下的解码行为与合入前逐字节一致。

## 兼容性论证

- **wire 零变更**：`Candidate`/`UiFrame` 结构体不加字段、不改布局；来源以既有数值线传递，`Assoc=8` 只增不改；`KeyAction` 是进程内动作枚举，不跨 wire。
- **穷举性守卫**：追加变体让每一个无通配臂的 `match` 编译失败，编译器列出全部决策点。本次被迫补臂的清单（全部为默认臂=现状行为）：`ime-types::key` 的测试 `label`；`ime-core` 状态机的 idle/composing 两处；`ime-fcitx5` 的 arbiter 两处、cheatsheet 的 `chord_label`、binding_audit 的 `canonical`（及其 `EVERY_ACTION`/`UNBOUND_WHITELIST` 登记与键位矩阵测试的 `action_name`）；`ime-ui` 与 `ime-ui-addon`/`ime-fcitx5` 两侧 transport 的数值线；`ime-diag` 快照的 `SourceName` 双向映射；`xtask` 场景驱动的 `action_name`。这与 ADR-0016 追加 `English` 时的守卫路径一致。
- **位域先例**：既有枚举追加（ADR-0005 的 `Phrase`/`Script`/`PageFirst`/`PageLast`、ADR-0016 的 `English`）均未引入版本号；枚举的穷举性检查本身是回归网。

## 后果与退出路径

- 消费方卡可立即并行开工：以词定字接 `SelectFirstChar`/`SelectLastChar` 的路由与字级提交 Effect，负反馈接 `ReduceFrequency`/`HideCandidate` 的用户库 penalty 位与会话隐藏集，联想接 `Assoc` 候选产出与 `[assoc]` 投影；各卡替换各自的过渡臂，互不阻塞。
- 变体一经落盘即保留：某张消费卡延期或回退，不影响其余两张；过渡臂（诊断 + 无行为）本身就是合法的长期状态，`binding_audit` 的 unbound 白名单以书面理由维持其可达性纪律。
- `[assoc]` 的三个键在配置合并层可读之前，文档中出现该节时按未知节处理：解析不报错、值取内置默认（与既有未知键行为一致）；接线卡落地 `PartialAssoc`/投影与坏值修默认范围时，同步把该节写进 `DEFAULT_CONFIG_TOML` 的键文档。
