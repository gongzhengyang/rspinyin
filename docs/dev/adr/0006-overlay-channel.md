# ADR-0006：按键速查浮层的承载方式（`UiFrame.overlay` 字段 vs `UiCommand::Overlay` 通道）

> 状态：**已接受（Accepted）** ｜ 决策日期：**2026-09-30** ｜ 决策人：主 Agent（宿主集成）｜
> 关联文档：[../opt-keymap.md](../opt-keymap.md)（`KEY-DEF-16`）、[../opt-keymap/phase-3.md](../opt-keymap/phase-3.md) 的 `KEY-P2.02.01` / `KEY-P2.02.02` / `KEY-P2.02.03` ｜
> 关联 ADR：[0001-frozen-boundary-contracts.md](0001-frozen-boundary-contracts.md)、[0005-incremental-contract-extension.md](0005-incremental-contract-extension.md) ｜
> 实测证据：`cargo nextest run -p ime-types` 绿（含本 ADR 新增的四条断言）

---

## 背景

`docs/dev/opt-keymap.md` 的 `KEY-DEF-16` 记录了一个缺口：`features.md` 3.5 的 19 行快捷键表**只活在规格文档里**，产品内没有任何地方能把它呈现给用户。`KEY-P2.02.02` 要落地一个速查浮层（长按修饰键或 `Ctrl+Shift+/` 唤出），`KEY-P2.02.03` 要在状态条上放被动徽章。

这两张卡都需要一个新的跨边界类型：**浮层帧**。而 `crates/ime-types` 的 `UiFrame` / `UiCommand` 由 [ADR-0001](0001-frozen-boundary-contracts.md) 冻结，`AGENTS.md` §8.22 因此要求「改动冻结类型必须走 ADR 并经主 Agent 决策」。

`docs/dev/adr/README.md` 的索引已把 `0006` 分配给本决策（`KEY-P2.02.01` 认领），标题即「`UiFrame.overlay` 字段 vs `UiCommand::Overlay` 通道」。本文件是该编号的正文。

## 决策

**新增 `UiCommand::Overlay(OverlayFrame)` 通道，`UiFrame` 零改动。**

| 方案 | 优点 | 缺点 | 结论 |
|---|---|---|---|
| 给 `UiFrame` 加 `overlay: Option<OverlayFrame>` | 复用现有的 latest-wins 帧通道 | `UiFrame` 是候选框的**完整快照**（`ui.rs:45-47`）；加字段后每一帧都背着一个恒为 `None` 的浮层，且 `size_of::<UiFrame>() <= 256` 的预算（`ui.rs:349-351` 的断言）被挤占 | **否决** |
| 新增 `UiCommand::Overlay`（latest-wins 单槽） | 与 `Theme` 同构（`ui.rs:41`）；浮层是模式而非内容，latest-wins 语义正确；不触碰 `UiFrame` | 需新增一个通道与对应的 `SurfaceUpdate` 变体 | **采用** |
| 复用 `UiCommand::Frame` 塞一个特殊 frame | 零契约变更 | 语义混淆，且 `UiFrame` 的必填字段（`candidates` / `page` / `status`）对浮层无意义 | 否决 |

### 决策 1：`UiFrame` 是快照，浮层是模式 —— 这是两个不同的东西

`UiFrame` 的语义是「候选框这一帧长什么样」：它必须携带 `candidates`、`page`、`status`，缺一个 UI 线程就画不出东西。浮层不是这个形状：它没有候选、没有分页，它的 `sections` 是一张静态的表。把一张静态表塞进一个每帧重建的快照，等于让解码路径每敲一个键都为它分配一次 —— 而它只在面板开合时才变。

反过来，浮层的生命周期与候选帧也**不同步**：面板打开时输入是冻结的（`KEY-P2.02.02` 的"消散"语义），候选帧可以继续被覆盖而浮层不动。两个不同步的东西共用一个 latest-wins 槽，就会出现「候选帧的更新把浮层冲掉」或「浮层覆盖时把候选帧吞掉」这两种都不想要的合并。

### 决策 2：latest-wins 而非有序队列

浮层是**模式**：同一时刻只可能有一个浮层，且只有最新的那个有意义。用户连按两次 `Ctrl+Shift+/`（打开、关闭），一次投递一个 `None`，旧值被覆盖 —— 这正是想要的。用有序队列反而会引入"积压的浮层状态逐个重放"的问题，那会让用户看到一个他已经关掉的面板。

这与 `Theme` 的语义完全同构，因此复用同一个 `LatestSlot` 实现，不引入新机制。

### 决策 3：`OverlayFrame` 承载数据，不承载行为

| 字段 | 类型 | 说明 |
|---|---|---|
| `kind` | `OverlayKind` | `CheatSheet` / `CommandPalette` / `Diagnostics` |
| `title` | `String` | 面板标题，中文（`AGENTS.md` 第 1 节：用户可见文案为中文） |
| `sections` | `Vec<OverlaySection>` | 分组，每组一个标题 + 若干条目 |
| `selected` | `Option<u16>` | 键盘高亮项；`None` 表示无高亮（速查面板恒为 `None`） |
| `query` | `String` | 模糊搜索串（命令面板用；速查面板恒为空） |

`OverlaySection` 是 `title` + `Vec<OverlayEntry>`，`OverlayEntry` 是 `keys`（如 `Tab`）+ `label`（如 `高亮下一项`）。

**内容生成不在契约里。** `KEY-P2.02.02` 要求面板内容**从 `KeyBindings` 与 `CHORDS` 表生成**、不得硬编码 —— 但那是引擎侧的实现约束，不是边界类型的事。契约只规定"送过来的是一张已经渲染好的表"，UI 线程不做任何键位推导。这条边界与 `UiFrame` 的既有原则一致：UI 线程只读快照，不派生。

### 决策 4：`OverlayFrame` 不引用任何 `slint::` 类型

`OB-4` / `AGENTS.md` §8.18 要求 `ime-ui` 的公共 API 不得导出 `slint::*`。`OverlayFrame` 定义在 `crates/ime-types`，本身就不依赖 Slint，本条是防止实现时"顺手"把一个 `slint::SharedString` 塞进来。`scripts/check-slint-leak.sh` 是这条的执行者。

## 影响面

| 类型 | 变更 | 类别 | 落地位置 |
|---|---|---|---|
| `UiCommand` | 追加 `Overlay(Option<Box<OverlayFrame>>)` 变体 | 追加（枚举） | `crates/ime-types/src/ui.rs` |
| `OverlayKind` | 新增 `pub enum OverlayKind { CheatSheet, CommandPalette, Diagnostics }` | 新增类型 | `crates/ime-types/src/ui.rs` |
| `OverlayFrame` | 新增 `pub struct OverlayFrame` | 新增类型 | `crates/ime-types/src/ui.rs` |
| `OverlaySection` | 新增 `pub struct OverlaySection` | 新增类型 | `crates/ime-types/src/ui.rs` |
| `OverlayEntry` | 新增 `pub struct OverlayEntry` | 新增类型 | `crates/ime-types/src/ui.rs` |
| `UiFrame` | **零改动** | — | — |

### 决策 5：与 `KEY-P2.02.01` 卡片正文的两处偏离

卡片给出的变体字面量是 `Overlay(OverlayFrame)`，本 ADR 落地为 `Overlay(Option<Box<OverlayFrame>>)`。两处偏离都是主 Agent 的决定，逐条说明：

**偏离一：外面包一层 `Option`。** 卡片的通道表只定义了「latest-wins 单槽，旧值被覆盖」，但**没有回答"面板怎么关"**。`KEY-P2.02.02` 要求浮层能被"消散"，而一个 `LatestSlot<OverlayFrame>` 只能表达"换成另一个浮层"，表达不了"没有浮层" —— 那会让速查面板**关不掉**。加上 `None` 之后，关面板就是投递一个 `None`，覆盖掉旧值，语义与 latest-wins 完全一致。代价是通道的类型参数变成 `Option<OverlayFrame>`，这恰好也是 `LatestSlot` 需要的形状。

（另一条路是再加一个 `OverlayDismissed` 变体。否决：那会让"关闭"有两条路径 —— 投递 `None` 与投递 `OverlayDismissed` —— 而 latest-wins 槽必须自己决定两者谁覆盖谁，这比多一层 `Option` 更容易写错。）

**偏离二：`Box` 一层。** 理由是**同枚举内的先例**：`UiCommand::Frame` 已经是 `Box<UiFrame>`，就是为了不让一个完整快照的内联大小进入 `UiCommand`。`UiCommand` 会被放进 `Show` / `Hide` 共用的那个有序队列（`control`），而那个队列在**每次按键**都可能被写入 —— 让它从约 40 字节涨到约 88 字节，是让热路径承担一个每会话只变两次的浮层的代价。`Box` 之后 `UiCommand` 的大小基本不变。

两条偏离都已写进 `crates/ime-types/src/ui.rs` 的测试里（`test_ui_frame_size_stays_within_budget_with_the_overlay_variant_present` 断言装箱确实更小；`test_ui_command_overlay_distinguishes_open_from_closed` 断言 `None` 是一个不同的值），因此**未来有人想"简化"回卡片的字面写法时，测试会先失败**。

全部是**追加**，没有原地修改，也没有破坏性变更：

- `UiCommand` 是 `#[derive(Clone, Debug, PartialEq)]` 的枚举，加一个变体不破坏任何既有构造点，但会让**每一个 `match` 变得非穷尽** —— 这是**故意的**：编译器会列出所有必须决定"新变体怎么办"的地方，这正是我们要的清单。`CandidateSource` 的 `Phrase` / `Script` 追加（ADR-0005）是同一手法。
- `UiFrame` 一个字都没动，因此 `size_of::<UiFrame>() <= 256` 的既有断言不回归。本 ADR 另加一条 `size_of::<UiCommand>()` 的断言，把 `UiCommand` 因新变体而变大的代价写进测试，而不是让它悄悄发生。

## 被否决方案的具体代价（供后来者参考）

若走"给 `UiFrame` 加字段"：`Option<OverlayFrame>` 的内联大小约为 `1 + 24 + 24 + 4 + 24 = 77` 字节（`String` 与 `Vec` 各 24 字节，`Option<u16>` 因判别位占 4 字节）。`UiFrame` 当前实测远低于 256，加 80 字节后**仍可能**在预算内 —— 所以这个方案不是"编译不过"，而是"语义错"：它会让解码热路径为一张静态表付出每帧一次构造与一次丢弃，且把两个不同步的生命周期绑在一起。**本 ADR 否决它的理由是语义，不是体积。**

## 如何变更

`crates/ime-types` 是冻结契约（ADR-0001）。要再改 `OverlayFrame` 的字段或 `UiCommand` 的形状：

1. 在 `docs/dev/adr/` 新增一份 ADR，编号从 [README.md](README.md) 的索引取下一个空闲号（**不得复用、不得重排**）；
2. 经主 Agent 决策并在索引表里把状态改为「已接受」；
3. 再动代码。`AGENTS.md` §8.22 禁止在 ADR 落盘前开工。

追加一个 `OverlayKind` 变体同样走这条路 —— 它是契约枚举。
