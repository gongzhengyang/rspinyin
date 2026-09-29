# ADR-0005：增量功能的契约扩展（`crates/ime-types`）

> 状态：**已接受（Accepted）** ｜ 决策日期：**2026-09-29** ｜ 决策人：主 Agent（宿主集成）｜
> 关联文档：[../features-add.md](../features-add.md) 的 4.2（边界交互契约增量规范）、4.2.1（契约增量总表）、5.0（`M0` 契约冻结点）｜
> 关联 ADR：[0001-frozen-boundary-contracts.md](0001-frozen-boundary-contracts.md)、[0004-ui-addon-crate-split.md](0004-ui-addon-crate-split.md) ｜
> 实测证据：`cargo check -p ime-types --all-targets` 绿、`cargo nextest run -p ime-types` 48/48 绿

---

## 背景

`docs/dev/features-add.md` 的 44 项功能差距里，有 6 张 P0 卡（`ADD-FEAT-P0.01.03`、`P0.01.04`、`P0.02.01`、`P0.02.03`、`P0.02.04`、`P0.02.05`）以及 `P0.03.02` 都需要先扩 `crates/ime-types` 的跨边界类型：短语与简繁转换要新的 `CandidateSource` 变体，双拼要 `SchemeId` 与 `DecodeRequest` 的新字段，用户词条管理要 `UserFreqSource` 的三个新方法与两个 `DictError` 变体。

`ADR-0001` 冻结了这些类型，`AGENTS.md` §8.22 因此要求「改动冻结类型必须走 ADR 并经主 Agent 决策」。features-add.md 把这件事抽成里程碑 `M0` 而非任务卡，理由是它的交付物是 ADR 加契约代码，且是所有并行工作的公共前置。

**编号冲突**：features-add.md 起草时把这件 ADR 命名为 `0004-incremental-contract-extension.md`。`ADR-0004` 已被 [UI 角色拆分为 `ime-ui-addon` crate](0004-ui-addon-crate-split.md) 占用，因此本决策改用 **0005**。

冲突不止一处：`docs/dev/opt-keymap.md` 与 `docs/dev/features-test.md` 也各自独立认领了 `0004`。全部裁决结果登记在 [`docs/dev/adr/README.md`](README.md) 的索引里——**该索引是编号的唯一权威**，新增 ADR 前必须先查它。features-add.md 与其分片中凡引用「ADR-0004」指本契约扩展者，已一律改写为 `ADR-0005`。

## 决策

**按 features-add.md 4.2.1 的表逐条落地契约增量，其中唯一的破坏性变更是 `DecodeRequest` 增加 `scheme` 字段。**

| 类型 | 变更 | 类别 | 落地位置 |
|---|---|---|---|
| `DecodeFlags` | 追加 `SHUANGPIN = 1<<11`、`PHRASE = 1<<12`、`SCRIPT = 1<<13` | 追加（位域） | `decode.rs` |
| `SchemeId` | 新增 `pub struct SchemeId(u8)`，`FULL`/`XIAOHE`/`ZIRANMA`/`MICROSOFT`/`SOGOU`/`ZIGUANG` + `COUNT = 6` | 新增类型 | `decode.rs` |
| `DecodeRequest` | 新增字段 `pub scheme: SchemeId` 与构造器 `with_scheme()` | **破坏性** | `decode.rs` |
| `CandidateSource` | 追加 `Phrase`、`Script` | 追加（枚举） | `ui.rs` |
| `Script` | 新增 `pub enum Script { Simplified, Traditional }` | 新增类型 | `ui.rs` |
| `StatusStrip` | 新增字段 `pub script: Script` | 追加（结构体） | `ui.rs` |
| `KeyAction` | 追加 `ToggleScript`、`ForgetHighlighted`、`PinHighlighted`、`AddPhrase` | 追加（枚举） | `key.rs` |
| `UserFreqSource` | 追加 `forget`、`list`、`export_tsv`，**均带默认实现** | 追加（trait） | `lexicon.rs` |
| `ConfigError` | 追加 `Migrated { from, to, backup }` | 追加（枚举） | `error.rs` |
| `ImeError` | 追加 `DataBackupFailed { reason }`、`SchemeUnsupported { scheme }`、`ConfigMigrated { from, to, backup }`、`UserWordNotFound`、`ExportTooLarge { bytes, limit }` | 追加（枚举） | `error.rs` |

`PhraseConfig` / `SchemeConfig` / `ScriptConfig` 三个配置段落在 `ime-config`，不属于 `crates/ime-types`，由各自的卡承接。

### 决策 1：唯一的破坏性变更及其缓解

`DecodeRequest` 加字段会破坏每一个结构体字面量构造点，因为它没有 `#[non_exhaustive]`。实测本 workspace 内**没有任何字面量构造点**——所有调用者都用 `DecodeRequest::new()`，再按需 `.with_flags()`。这正是 `ADR-0001` 当初提供构造器而非让调用者拼结构体的价值：这次扩展只动了 `new()` 一个地方，加了一个 `with_scheme()`。

`Default` 语义为 `SchemeId::FULL`（全拼），因此任何不关心双拼的调用者行为完全不变。

### 决策 2：`UserFreqSource` 的三个新方法必须带默认实现

不带默认实现会让**每一个**既有实现编译失败，包括 `ime-core` 的确定性测试所驱动的内存 mock（`MockUserFreq`、`NoUser`、`RecordingUser`）。那样扩展的侵入面会远大于它旁边的结构体字段。

默认实现取「本实现不提供该能力」的诚实答案：`forget` 返回 `false`，`list` 与 `export_tsv` 返回 `Err(ImeError::Unsupported)`。

**对 features-add.md 的一处更正**：该表原本写 `list` 的默认实现返回 `ImeError::DictUnavailable`。但那个变体携带 `PathBuf` 与 `DictError`，而「这个数据源根本没有枚举能力」既没有路径也没有底层容器错误——伪造一个路径会把一个不存在的位置写进用户可见的诊断。`dict/unsupported` 才是契约里为「本构建阶段故意不提供该能力」保留的码，`Lexicon::prefix` 已经用了它，此处沿用。

### 决策 3：`ImeError` 增加 `ConfigMigrated`（超出 4.2.1 表的一条）

4.2.1 的 `ImeError` 行只列了 `DataBackupFailed` 与 `SchemeUnsupported`，但 4.2.2 的跃迁表要求配置迁移成功后记 `config/migrated`，且 4.2.1 末尾的稳定码清单里**已经列了这个码**。

如果不给 `ImeError` 一个承载它的变体，`From<ConfigError> for ImeError` 就只剩两个选择：要么把一次**成功的**迁移折到 `ConfigInvalid` 上（等于告诉用户配置校验失败，而事实是它被修好了），要么让 `config/migrated` 成为一个任何渲染都产生不出来的码——而契约规定错误码由诊断与测试逐字匹配，产生不出来的码就是匹配不了的码。

因此追加 `ImeError::ConfigMigrated { from, to, backup }`。这是一次**追加**，不是原地修改；它在调用点的级别是 `info`，不进入错误路径的噪声。本条的代价是 `ImeError` 比 4.2.1 的表多一个变体，这是主 Agent 的决策并在此登记。

### 决策 4：`dict/user-word-not-found` 与 `dict/export-too-large` 挂在 `ImeError` 而非 `DictError`

**这是对 4.2.1 表的一处更正**：该表把这两个变体列在 `DictError` 名下。落地时发现那是分类错误，理由是 `DictError` 自己的文档写着它是「validating and reading the compiled dictionary」时抛出的容器失败，而这两个描述的是「查询成功但没找到」与「导出超限」——不是容器的性质。

代价在 `crates/ime-dict/src/recover.rs` 立刻显现：该文件有两处对 `DictError` 的穷尽 `match`（`is_damaged` 与 `classify_store_failure`），它们的工作是判断**文件内容是否损坏**、从而决定要不要把用户的文件移到隔离区。若把这两个变体放在 `DictError` 上，这两处就必须为一个「与文件无关」的变体编造一个答案。

因此改挂 `ImeError`：错误码仍在既有 `dict/*` 段内（4.2.1 的原始要求是「在既有段内分配」，指的是码，不是枚举），渲染的是完整稳定码，且 `UserFreqSource::list` / `export_tsv` 本来就返回 `ImeError`，调用点无需转换。`DictError` 保持它「只描述容器完整性」的单一含义。

### 决策 5：状态机对新动作的落点

`KeyAction` 追加 4 个变体后，`ime-core` 的两处穷尽 `match` 必须给出落点：

- `ToggleScript`：引擎像拥有中英、全角、标点三个位一样拥有脚本位，所以会话只重发帧（`emit_frame`，revision 自增）。**刻意不产生 preedit**，且在 `Composing` 下**不重置会话**——这是 0.4 规则 10 的要求，也是 4.2.2 跃迁表的原文。
- `ForgetHighlighted` / `PinHighlighted` / `AddPhrase`：三者分别需要短语表、pin 集合与可写的用户库，都不存在。这两处 arm 发出 `Effect::Diagnose(ImeError::Unsupported)`（渲染为 `dict/unsupported`），由实现这些子系统的卡替换。

**为什么不写成静默 no-op**：那会让用户看到一个「按了没反应」的键，且没有任何诊断解释。`dict/unsupported` 是契约为此保留的码。

## 理由

- 位域与枚举的追加天然前向兼容：`from_bits_truncate` 丢弃未知位，旧读者读到新位会忽略。真正需要 ADR 的只有 `DecodeRequest` 加字段这一处，本 ADR 单独列出。
- `CandidateSource` 与 `KeyAction` 的追加会让穷尽 `match` 编译失败——**这正是那些测试的设计意图**。`key.rs` 的 `label()` 注释写明「adding or removing one breaks the build instead of silently changing the frozen contract」，`every_key_action()` 的定长数组同理。实测：`CandidateSource` 在 `ime-types` 之外**没有任何 `match`**，只有相等比较，因此该追加的实际影响面为零；`KeyAction` 的两处穷尽 `match` 都已同步。
- 所有新错误码都分配在既有域名段内（`config/*`、`dict/*`、`data/*`、`decode/*`），不新开域名，符合 `AGENTS.md` §1 的稳定码约定。

## 后果

1. **解锁 6 张 P0 卡**：`ADD-FEAT-P0.01.03`、`P0.01.04`、`P0.02.01`、`P0.02.03`、`P0.02.04`、`P0.02.05` 的「前置依赖：`M0` 契约冻结」已满足。
2. **三处穷尽测试同步**：`decode.rs` 的 `test_decode_flags_all_covers_every_defined_bit` 由 `0x07FF` 改为 `0x3FFF`（另有 `from_bits_truncate` 的同一断言）、`key.rs` 的 `label()` 增 4 个 arm、`state/tests.rs` 的 `every_key_action()` 由 `[KeyAction; 15]` 改为 `[KeyAction; 19]`。
3. **`features-add.md` 与 `docs/dev/opt-*.md` 中的「ADR-0004」引用需按本 ADR 开头的更正说明回写**。
4. **`docs/dev/features.md` 2.2.3 的 C ABI 版本不变**：本次全部是 Rust 侧类型，不进 vtable，`RSPINYIN_ABI_VERSION` 保持 2。
5. **遗留**：`PinHighlighted` 未被 4.2.2 的跃迁表覆盖，本 ADR 只落了它的词汇与 `label()`；它的行为由实现 pin 集合的卡定义。
