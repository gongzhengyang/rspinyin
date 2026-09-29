# ADR 索引

> 维护约定：**编号一经使用永不复用、永不重排**。新增 ADR 前必须先读本表取下一个空闲编号；`docs/dev/` 下的任何规划文档在提出"待决策 ADR"时，必须从本表取号，不得自行假设编号。

本表登记两类条目：**已决策**（文件已落盘）与**已分配但未落盘**（某个规划文档已认领该编号，决策尚未做出）。

| 编号 | 标题 | 状态 | 认领方 | 关联 |
|---|---|---|---|---|
| `0000` | 上游决策记录（Slint 许可、词源、`OB-1`~`OB-6`） | 已接受 | 项目基线 | [0000-upstream-decisions.md](0000-upstream-decisions.md) |
| `0001` | 冻结边界契约（`crates/ime-types` 的全部跨边界类型） | 已接受 | 项目基线 | [0001-frozen-boundary-contracts.md](0001-frozen-boundary-contracts.md) |
| `0002` | Rust 侧导出 addon 工厂符号 | 已接受 | 项目基线 | [0002-rust-exports-addon-factory.md](0002-rust-exports-addon-factory.md) |
| `0003` | UI 角色必须独立成 addon | 已接受 | 项目基线 | [0003-ui-role-separate-addon.md](0003-ui-role-separate-addon.md) |
| `0004` | UI 角色拆分为 `ime-ui-addon` crate（第二个 cdylib 落地） | 已接受 | 本仓库实现 | [0004-ui-addon-crate-split.md](0004-ui-addon-crate-split.md) |
| `0005` | 增量功能的契约扩展（`crates/ime-types`） | 已接受 | `docs/dev/features-add.md` 的里程碑 `M0` | [0005-incremental-contract-extension.md](0005-incremental-contract-extension.md) |
| `0006` | 按键速查浮层的承载方式（`UiFrame.overlay` 字段 vs `UiCommand::Overlay` 通道） | **已分配，未落盘** | `docs/dev/opt-keymap.md` 的 `KEY-P2.02.01` | 见 `docs/dev/opt-keymap.md` 与 `docs/dev/opt-keymap/phase-3.md` |
| `0007` | `UiFrame` 的 `serde` 派生（冻结契约的追加） | **已分配，未落盘** | `docs/dev/features-test.md` 的 `FEAT-TEST-P0.02.01` | 见 `docs/dev/features-test.md` |
| `0008` | 命令面板的契约增量（`UiCommand::Panel` / `UiEvent::PanelAction`） | **已分配，未落盘** | `docs/dev/features-add/phase-2.md` 的 `ADD-FEAT-P1.02.02` | 见 `docs/dev/features-add/phase-2.md` |
| `0009` | `Placement` 追加 `Fixed` / `Remember` 两个变体 | **已分配，未落盘** | `docs/dev/features-add/phase-2.md` 的 `ADD-FEAT-P1.03.04` | 见 `docs/dev/features-add/phase-2.md` |

## 编号冲突的由来

`0004` 曾被三个不同的规划流程各自独立认领，起草时互不知情：

- `docs/dev/features-add.md` 的 `M0` 里程碑认领 `0004` 用于**增量契约扩展**；
- `docs/dev/opt-keymap.md` 的 `KEY-P2.02.01` 认领 `0004` 用于**按键浮层承载**；
- `docs/dev/features-test.md` 的 `FEAT-TEST-P0.02.01` 认领 `0004` 用于**`UiFrame` 的 `serde` 派生**。

而 `0004` 实际被 UI addon 拆分占用（该拆分先落地）。冲突已按本表裁决：契约扩展改 `0005`，其余两项改 `0006` 与 `0007`，`features-add/phase-2.md` 里两处新提出的契约增量分配 `0008` 与 `0009`。**三份规划文档中的引用已同步改写**。

## 未落盘条目的含义

标记为"已分配，未落盘"的编号**不表示该决策已做出**，只表示该编号已被认领、不得他用。这些 ADR 的正文尚未撰写，其提案内容散落在对应的规划文档里。按 `AGENTS.md` §8.22，在这些 ADR 落盘并经主 Agent 决策之前，**任何改动冻结契约的实现都不得开工**。
