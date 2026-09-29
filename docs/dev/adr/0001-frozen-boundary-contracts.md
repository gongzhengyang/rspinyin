# ADR-0001：冻结跨边界契约（`crates/ime-types`）

> 状态：**已接受（Accepted）** ｜ 冻结日期：**2026-09-29** ｜ 决策人：主 Agent（契约冻结点）｜
> 关联文档：[../features.md](../features.md) 的 2.2 节（边界交互契约）、5.1.1（契约冻结纪律）、`TASK-1.01.03` 任务卡 ｜
> 关联 ADR：[0000-upstream-decisions.md](0000-upstream-decisions.md) ｜
> 冻结效力：本 ADR 冻结 `crates/ime-types` 的全部跨边界类型。**此后任何修改必须先新开 ADR，并经主 Agent 决策后同步回写 features.md 2.2 与 5.1.1。**

---

## 背景

`TASK-1.01.03` 是本项目**唯一**的全局契约冻结点。三条并行轨道（Track A 引擎与数据、Track B 渲染与宿主集成、Track C 基建）从 W1 起完全并行开发，彼此的编译与语义耦合**全部**经过 `crates/ime-types`：

- 若该 crate 的类型不完整，下游任务会在自己的 crate 内私自定义跨边界类型，直接违反 features.md 5.1.1 的冻结纪律，并在合并时产生编译冲突（风险 `R-12`）。
- 若该 crate 的类型与 features.md 2.2 的代码块不一致，DoD #5 的脚本化字段比对会失败，且下游会按错误的字段名编码。

因此本 ADR 的首要目标是**完整性**与**逐字段保真**，其次才是设计品味。凡本 ADR 对 2.2.x 代码块做出偏离，均在下文"偏差登记"中逐条给出理由与规格出处。

---

## 决策

`crates/ime-types` 冻结为 8 个叶子模块，只依赖 `thiserror`、`bitflags` 与（可选 feature）`serde`；无内部依赖、无 `build.rs`、无 `unsafe`、无 C 依赖、无网络能力。

| 模块文件 | 承载的契约 |
|---|---|
| `src/error.rs` | 跨边界错误模型：`ImeError` / `DictError` + 四个领域错误枚举与映射 |
| `src/ids.rs` | 标识符 newtype：`SessionId` / `ScreenId` / `WordId` / `Revision` |
| `src/version.rs` | 三个冻结版本常量与 ABI 握手 `check_abi` |
| `src/ui.rs` | 引擎 ↔ UI 边界的全部协议类型（2.2.1 / 2.2.2 全文） |
| `src/decode.rs` | 解码请求 / 结果契约与 `DecodeFlags` |
| `src/lexicon.rs` | 三个数据源 trait 与零拷贝 `WordRef` / `WordIter` / `WordFlags` |
| `src/surface.rs` | 平台边界 `SurfaceBackend` 与像素 / 事件词汇 |
| `src/key.rs` | 按键语义 `KeyAction` |

### 冻结类型清单（DoD #4 要求）

**`error.rs`（6 个类型）**
`ImeError`（16 个变体，其中 15 个逐字来自 2.2.4，1 个为追加）、`DictError`（6 个变体，逐字来自 2.2.4）、`UiError`（6）、`DecodeError`（4）、`ConfigError`（2）、`PlatformError`（3）。

**`ids.rs`（4 个类型）**
`SessionId`、`ScreenId`、`WordId`、`Revision`。

**`version.rs`（4 个条目）**
`RSPINYIN_ABI_VERSION`、`DICT_FORMAT_VERSION`、`CONFIG_SCHEMA_VERSION`、`check_abi`。

**`ui.rs`（21 个类型）**
`UiCommand`、`UiFrame`、`Anchor`、`Placement`、`RectI`、`Preedit`、`PreeditSpan`、`SpanKind`、`Candidate`、`CandidateSource`、`PageState`、`StatusStrip`、`LayoutHint`、`HideReason`、`ThemeSpec`、`ColorScheme`、`Rgba8`、`UiEvent`、`SelectTrigger`、`PageDir`、`DismissReason`。

**`decode.rs`（5 个类型）**
`DecodeRequest`、`DecodeResult`、`Segment`、`SyllableId`、`DecodeFlags`。

**`lexicon.rs`（6 个类型）**
`Lexicon`、`UserFreqSource`、`LanguageModel`、`WordRef`、`WordIter`、`WordFlags`。

**`surface.rs`（4 个类型）**
`SurfaceBackend`、`PixelBufferMut`、`FrameToken`、`SurfaceEvent`。

**`key.rs`（1 个类型）**
`KeyAction`（15 个变体，与 2.2 及 5.1.1 重构 #3 一致）。

合计 **51 个公开条目**。

### 明确不在本 crate 内的契约

`features.md` 2.2.3 的 C ABI 类型（`FcitxCursorRect`、`FcitxKeyEvent`、`RspinyinVtable`、`UiPanelSnapshot`、`rspinyin_register_vtable`）**不属于** `ime-types`：它们的落地锚点是 `crates/ime-fcitx5/src/ffi/abi.rs`（见 2.2.3 的代码块标题与 `TASK-1.04.01` 的实施步骤 2），且含裸指针与 `extern "C"`，与"`ime-types` 无 `unsafe`、无 C 依赖"的 NFR 冲突。本 crate 只冻结它们共享的**版本常量** `RSPINYIN_ABI_VERSION`（在 `version.rs`），`abi.rs` 引用之而不重复定义。

---

## 冻结的版本常量

| 常量 | 值 | 语义 |
|---|---|---|
| `RSPINYIN_ABI_VERSION` | `1` | Rust ↔ C++ vtable 契约版本。**追加 vtable 字段必须递增**（C 结构体无其他兼容机制） |
| `DICT_FORMAT_VERSION` | `1` | 词库容器格式版本 |
| `CONFIG_SCHEMA_VERSION` | `1` | `config.toml` 的 schema 版本 |

`check_abi(host) -> Result<(), ImeError>`：`host != RSPINYIN_ABI_VERSION` 时返回错误。ABI 不匹配**复用** `ImeError::Fcitx5VersionMismatch`（`host` 承载宿主上报的 ABI 版本、`required` 承载本构建要求的版本），以免为一次性场景在冻结错误表里新增一个码。2.2.3 规定的日志原文 `rspinyin: ABI mismatch (host=1, plugin=N)` 由调用方（`TASK-1.04.01`）在写诊断时输出。

---

## 规格未给出代码块的部分（本 ADR 一并冻结）

2.2.1 / 2.2.2 / 2.2.4 给出了逐字代码块，`decode.rs` 与四个领域错误枚举**只有类型名清单**。以下定义由本任务首次落地并就此冻结：

### `decode.rs`

- `SyllableId(u16)`：411 音节表的下标 newtype，字段私有，`new` / `value` / `From<SyllableId> for u16`。
- `DecodeFlags: u16`（`bitflags`）：`FUZZY`（总开关）、`FUZZY_ZH_Z`、`FUZZY_CH_C`、`FUZZY_SH_S`、`FUZZY_N_L`、`FUZZY_AN_ANG`、`FUZZY_EN_ENG`、`FUZZY_IN_ING`、`FUZZY_F_H`、`ABBREV`、`USER_DICT`。模糊音等价类取自 0.5.2 与 `TASK-2.02.01` 的权威列表；`Default = USER_DICT`（Phase 1 只用该位）。
- `DecodeRequest { raw: String, flags: DecodeFlags }`：`raw` 为**未规范化**的 ASCII 输入（规范化在切分层内完成，见 `TASK-1.02.01`）。
- `DecodeResult { candidates: Vec<Candidate>, segments: Vec<Segment>, degraded: bool }`：`candidates` 复用 2.2.1 的 `Candidate`（引擎产出的候选即 `UiFrame.candidates`，无中间转换层）；`segments` 为最优路径的切分；`degraded` 对应"无路径 / 词库不可用"的降级态。
- `Segment { start: u16, end: u16, text: String, source: CandidateSource }`：`start`/`end` 为音节下标区间（左闭右开）。

### 四个领域错误枚举

| 枚举 | 变体 | 稳定码 |
|---|---|---|
| `UiError` | `ChannelClosed` / `StaleSelect` / `FontMissingCjk` / `SelectTimeout` / `NotReady` / `ThreadDead` | `ui/channel-closed`、`ui/stale-select`、`ui/font/missing-cjk`、`ui/select/timeout`、`ui/not-ready`、`ui/thread/dead` |
| `DecodeError` | `EmptyInput` / `TooLong` / `InvalidChar` / `NoPath` | `decode/empty-input`、`decode/too-long`、`decode/invalid-char`、`decode/no-path` |
| `ConfigError` | `Invalid` / `LimitExceeded` | `config/invalid`、`config/limit-exceeded` |
| `PlatformError` | `Unavailable` / `NoFreeBuffer` / `Disconnected` | `platform/backend/unavailable`、`platform/backend/no-free-buffer`、`platform/backend/disconnected` |

全部码都取自 features.md 中已有的诊断码（`ui/select/timeout`、`ui/not-ready`、`ui/thread/dead`、`config/limit-exceeded`、`decode/*`），**未新造任何码**。

**到 `ImeError` 的映射**（`impl From<X> for ImeError`，满足 AGENTS.md 3.2 的"领域错误必须可 `#[from]` 式转换"）：

| 领域变体 | 映射目标 | 说明 |
|---|---|---|
| `DecodeError::{TooLong, InvalidChar, NoPath}` | `ImeError::{DecodeTooLong, DecodeInvalidChar, DecodeNoPath}` | 逐字段无损 |
| `DecodeError::EmptyInput` | `ImeError::DecodeNoPath { raw: "" }` | 空串是"无路径"的退化情形 |
| `ConfigError::Invalid` | `ImeError::ConfigInvalid` | 逐字段无损 |
| `ConfigError::LimitExceeded` | `ImeError::ConfigInvalid { key: section, reason: "limit exceeded: N" }` | 冻结表中无对应码，压到 `config/invalid` 并在 reason 中保留限额 |
| `UiError::{ChannelClosed, StaleSelect, FontMissingCjk}` | `ImeError::{UiChannelClosed, UiStaleSelect, UiFontMissingCjk}` | 逐字段无损 |
| `UiError::{SelectTimeout, NotReady, ThreadDead}` | `ImeError::UiChannelClosed` | 三者从调用方视角同为"UI 通道不可用" |
| `PlatformError::*` | `ImeError::CompositorUnsupported { detail }` | 冻结表只有单一平台域变体；三个 detail 字符串保持可区分 |

**纪律**：映射是"传播用"的，不是"记录用"的。领域错误必须在**失败现场**以精确码写入诊断（`tracing` / 探针计数），再做转换——转换后的码可能更粗。

### `WordIter` 的实现选择

`WordIter<'a>` 内部为 `std::vec::IntoIter<WordRef<'a>>`（`from_vec` 构造），实现 `Iterator` + `ExactSizeIterator` + `FusedIterator`。理由：三次 `Lexicon` 查询的结果规模都有界（单键词表上限 32、`limit` 参数），物化成本与一次 `Box<dyn Iterator>` 分配相当，但**无动态派发、顺序确定、可 `ExactSizeIterator`**。结构体字段私有，后续阶段可在不改任何签名的情况下替换内部实现。

### serde 策略

`serde` 为可选 feature（默认关闭），只对**数据型**契约类型派生（ids / ui / decode / key 与 `WordFlags`）；两个 `bitflags` 类型用手写 `Serialize` / `Deserialize`（序列化为裸位集，反序列化时丢弃未知位，保证"新版本写、旧版本读"不炸）。错误类型、`WordRef` / `WordIter`（借用型）、`SurfaceBackend` / `PixelBufferMut`（独占借用）不参与序列化。

---

## 偏差登记（对 features.md 2.2.x 代码块的偏离）

以下 5 处偏离均为**必要**或**规格内部冲突的裁决**，除此之外 2.2.1 / 2.2.2 / 2.2.4 的每个类型、字段、变体、方法签名与 `#[error]` 字符串都逐字落地（注释按 AGENTS.md §1 译为英文）。

| # | 位置 | 规格原文 | 实际落地 | 理由 |
|---|---|---|---|---|
| 1 | `Anchor` 的 derive | `#[derive(Clone, Copy, Debug, PartialEq, Eq)]` | `#[derive(Clone, Copy, Debug, PartialEq)]`（去掉 `Eq`） | **规格原文无法编译**：`Anchor.scale` 是 `f32`，而 `f32` 未实现 `Eq`，派生 `Eq` 会报 E0277。字段与类型一字未改 |
| 2 | `UiCommand` 的 derive | `#[derive(Clone, Debug)]` | `#[derive(Clone, Debug, PartialEq)]`（追加 `PartialEq`） | 本任务卡的 NFR 明文要求"全部跨边界类型必须 `#[derive(Clone, Debug, PartialEq)]`"，且 `TASK-1.05.02` 的 DoD #4 需要比较投递命令的顺序。追加 `PartialEq` 是纯增量，不破坏任何下游 |
| 3 | `ImeError` 变体表 | 15 个变体（2.2.4 全文） | 追加第 16 个变体 `Unsupported`（`dict/unsupported`），**位置在末尾** | `TASK-1.03.02` 的 `Lexicon::prefix` 实现与实施步骤 4 明文要求 `Phase 1 返回 Err(ImeError::Unsupported)`；冻结表若缺此变体，`ime-dict` 只能私造错误类型（违反 5.1.1）。追加在末尾以保持既有变体顺序不变 |
| 4 | `StatusStrip` 字段 | 4 个字段 | 追加 `readonly: bool`，**位置在末尾** | `TASK-1.03.04` 明文要求"`StatusStrip` 增加 `readonly: bool` 字段——**这是对 2.2.1 契约的追加，须在 `TASK-1.01.03` 中补入**"；`TASK-1.05.03` 的 `candidate.slint` 与 3.6 的降级表（只读模式显示灰色小锁）同样依赖它 |
| 5 | `WordRef` 字段 | `TASK-1.01.03` 卡：`{ text: &'a str, weight: u16, flags: WordFlags }`；`TASK-1.03.03` 卡：`{ text: &'a str, weight: u32, syl_count: u8, flags: WordFlags }` | 采用 **`TASK-1.03.03`** 的定义（`weight: u32` + `syl_count: u8`） | 两处规格**互相冲突**，且 `WordRef` 不在 2.2.x 代码块内（DoD #5 不覆盖它）。采用消费方定义的三个理由：(a) `weight: u32` 与词库容器的 `DictEntry.weight`（Q16.16）类型一致，`u16` 会强制有损截断；(b) `syl_count` 是 `TASK-1.02.04` 计算 `Candidate.consumed_syllables` 的唯一来源，且 `TASK-1.03.03` 的 DoD #3 明文断言该字段；(c) 反向选择会让两个下游任务的验收标准不可达，而正向选择不破坏任何东西 |

**连带约束（写给下游任务，不是偏离）**：`WordFlags` 因被 `WordRef` 内嵌，定义在 `ime-types/src/lexicon.rs`；`TASK-1.03.03` 的 `crates/ime-dict/src/entry.rs` **不得重复定义** `WordFlags`，只能 `pub use ime_types::lexicon::WordFlags;`（否则两个 crate 会出现两个不同的类型）。

---

## 变更流程

1. **任何**对 `crates/ime-types` 的公开 API 修改（含增删字段、改类型、改 derive、改 `#[error]` 字符串）都必须先在本目录新开 ADR，写明动机、影响面与迁移路径。
2. 由**主 Agent** 决策；被批准后同步回写 features.md 的 2.2 代码块、5.1.1 的"契约冻结纪律"段，以及本文件的"冻结类型清单"。
3. 子 Agent **禁止**修改本 crate（AGENTS.md §8 第 8 条、6.4 的纪律）；业务 crate 内**禁止**私自定义跨边界类型（5.1.1）。
4. 错误码字符串一经发布不得改写：诊断与测试按码匹配。
5. 追加而非重排：新增枚举变体 / 结构体字段一律追加到**末尾**，以保持既有顺序与（对 `RspinyinVtable` 而言的）C ABI 布局稳定。

---

## 后果与待办

- **下游解锁**：Track A / Track B 从 W1 起可完全并行——`Decoder::decode` 只依赖 `DecodeRequest` / `DecodeResult` 与三个 trait；UI 侧只依赖 `UiCommand` / `UiFrame` / `UiEvent` / `SurfaceBackend`；按键路由只依赖 `KeyAction`。
- **`UiFrame` 体积**：实测 `size_of::<UiFrame>()` ≤ 256 字节（单元测试 `test_ui_frame_size_stays_within_budget` 断言），满足本任务卡的 NFR。
- **待办（主 Agent 决策项）**：
  1. 若主 Agent 更希望 ABI 不匹配拥有独立错误码，可新增 `ImeError::AbiMismatch`（本 ADR 选择了复用 `Fcitx5VersionMismatch`，理由见上）。
  2. 若主 Agent 判定 `WordRef` 应以 `TASK-1.01.03` 卡的 `weight: u16` 为准，需同时修改 `TASK-1.03.03` 与 `TASK-1.02.04` 的验收标准（本 ADR 已论证不应如此）。
  3. 若 DoD #5 的脚本化比对按"变体名集合完全相等"实现，则偏差 3 与 4 需要脚本放行（两项追加都有规格明文依据）。

---

## 参考

- `docs/dev/features.md` 2.2（边界交互契约）、2.3（跨边界状态机）、5.1.1（契约冻结纪律与三处重构）
- `docs/dev/features.md` 0.4（不可违反的架构规则）、0.5.3（性能预算）
- [0000-upstream-decisions.md](0000-upstream-decisions.md)（Slint 许可与词源冻结）
- `AGENTS.md` §1（注释语言）、§3.2（错误处理）、§4.3（类型与函数设计）、§8（禁止事项）
