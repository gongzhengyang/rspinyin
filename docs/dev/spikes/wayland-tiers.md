# Spike 结论：Wayland 四档窗口后端的可验证性与结论

> 状态：**代码侧结论已完成；四档的真机验证在本机不可能完成**，原因见 §2。本文件按 `TASK-1.04.07` 的验收标准 6 交付。
> 关联模块：`MOD-WL` ｜ 风险编号：R-03（本 spike 即该风险落点）
> 代码落点：`crates/ime-ui/src/platform/wayland/`（`mod.rs`、`probe.rs`、`layer_shell.rs`、
> `popup.rs`、`canvas_popup.rs`、`shm.rs`、`events.rs`、`client.rs`、`backend.rs`）
> 关联假设：`ASM-13`（Wayland 四档降级梯）｜ 关联 ADR：[ADR-0000](../adr/0000-upstream-decisions.md)

## 0. 环境

| 项 | 值 | 来源 |
|---|---|---|
| 本机显示服务器 | **WSLg（XWayland + Weston）** | `$WAYLAND_DISPLAY` / `$XDG_SESSION_TYPE` 实测 |
| 本机合成器 | **Weston** | WSLg 自带 |
| `wlr-protocols` | **未安装** | 包管理器查询无结果 |
| `wayland-client` | workspace 依赖（版本见 `Cargo.lock`） | `[workspace.dependencies]` |
| Rust | edition 2024 / MSRV 1.85 / toolchain 1.98.0 | `rust-toolchain.toml` |
| 无显示服务器测试 | 全部通过 | `cargo nextest run -p ime-ui` |

## 1. 四个验证问题的结论

### (a) 四档梯子是否成立 —— **协议侧成立，合成器侧未验证**

梯子的定义与选档逻辑在 `crates/ime-ui/src/platform/wayland/mod.rs` 的模块文档中逐档写明：

| 档 | 协议 | 定位方式 | 键盘焦点 |
|---|---|---|---|
| **T1** | `zwlr_layer_shell_v1` | `OVERLAY` 层 + `TOP \| LEFT` 锚点，margin 携带坐标 | `keyboard_interactivity = none`，合成器无法覆盖 |
| **T2** | `xdg_popup` | positioner 锚在光标上，合成器负责翻转与滑移 | 从不调用 `xdg_popup.grab` |
| **T3** | `xdg_popup` | 关掉 constraint adjustment，由 `canvas_popup` 自己算矩形 | 同 T2 |
| **T4** | 无 | 不注册 `UserInterface`，宿主自己的候选列表接管（`ASM-13`） | 不适用 |

`probe::TierLadder` 从 registry 的全局对象选起始档，再用一次 configure 确认；`CONFIGURE_TIMEOUT`
内没有 configure 就降到下一档。降级链在 `probe.rs:372-374` 是单向的、有界的、可终止的。

**协议常量核对**：anchor 与 gravity 的取值、layer-shell 的枚举、constraint adjustment 的取值
都对着公开的协议描述逐个核过（`layer_shell.rs`、`popup.rs`、`canvas_popup.rs` 的常量定义处有英文注释），
**但不是对着合成器核的**。协议描述与实现之间是否有偏差，只有真机能回答。

### (b) 候选框是否会夺走键盘焦点 —— **结构上不可能，真机未验证**

这是项目最高等级的缺陷（`AGENTS.md` 禁止项 20），因此本模块把它做成结构性约束而不是承诺：

1. T1 请求 `keyboard_interactivity = none`，合成器无法覆盖该请求；
2. 后端**从不调用** `xdg_popup.grab` —— 那是唯一能把键盘交给 popup 的请求；
3. 后端**从不绑定** `wl_keyboard`；
4. `client::ProtocolClient` **没有任何方法**能取得焦点，因此这个性质不可能被后续改动无意破坏；
5. 梯子**主动监视**合成器自作主张的行为：`events::WireEvent::KeyboardEnter` 一旦到达，当前档立即判失败并降级，而不是把一个偷焦点的窗口留在屏幕上。

第 1~4 条是代码可证的（有测试），第 5 条是运行时可证的但**需要真合成器**。

### (c) popup 档的父表面从哪来 —— **开放问题，未解决**

positioner 的锚矩形必须落在父表面的窗口几何内，而 xdg-shell **没有**任何请求能让客户端指定一个
toplevel 的位置，因此父表面必须是一个覆盖整个输出的表面。合成器是否愿意映射这样一个父表面而
**不给它键盘**，正是梯子的焦点检查与 §3 要回答的问题。若某个合成器的答案是"不愿意"，该合成器的
诚实结论就是 **T4**。

本机无法回答这个问题：Weston 在 `ASM-13` 里被明确列在四档之外。

### (d) 单位换算是否只有一处 —— **成立**

契约数物理像素，协议数 surface-local 单位（= 逻辑尺寸 = 缓冲尺寸 ÷ buffer scale）。
两个坐标系之间的每次穿越都只走 `surface_offset` / `physical_offset` / `SurfaceRect` 三者之一，
模块文档把这条写成硬约束。buffer scale 设为输出的 device pixel ratio，因此缓冲按输出物理分辨率
绘制，合成器不会把任何逻辑像素上采样。

## 2. 为什么本机四档全不可验证

- **Weston 不在四档之内**：`ASM-13` 明确排除了 Weston 这类合成器；本机 WSLg 跑的正是它。
- **`wlr-protocols` 未安装**：`zwlr_layer_shell_v1` 既不存在也不可测，T1 在本机连协议对象都拿不到。
- **没有真实的 Sway / Hyprland / KWin / Mutter 会话**：本机是 WSL2 内的 X11/XWayland 环境，
  没有可切换的合成器。

这不是"没跑"，而是**跑不了**。把它记下来而不是粉饰过去，是本文件存在的理由。

## 3. 真机会话必须确认的四件事

四档各自的验收项，每一项都需要一台真机：

| 项 | 需要的环境 | 判据 |
|---|---|---|
| 逐合成器的选档结果 | Sway、Hyprland、KWin、Mutter 各一 | 实际停在哪一档，以及为什么 |
| 定位偏差 | 同上 | 候选框与光标水平中心偏差 ≤ 2 物理像素（`TASK-1.04.05` DoD 1） |
| 焦点不变 | 同上 | 首次 commit 之后**没有** `wl_keyboard.enter`（DoD 4） |
| popup 档是否可行 | Sway、KWin | 父表面能否在不取键盘的前提下被映射（§1c） |

`TASK-1.07.02` 建立测试矩阵后，这四项的回填位置在本表。

## 4. 已实现且不依赖合成器的部分

以下部分在**没有任何显示服务器**的机器上被测试覆盖，它们不因本机不可验证而缺少证据：

- **梯子的选档与降级**：`probe_tests.rs`（`#[path]` 挂在 `probe.rs` 下）覆盖全局对象缺失、
  configure 超时、降级链终止、以及 `Tier::Fallback` 的终态；
- **几何换算**：`surface_offset` / `physical_offset` / `SurfaceRect` 在 `scale ∈ {1.0, 1.25, 1.5, 2.0, 3.0}`
  下的往返一致性；
- **缓冲纪律**：`wl_buffer.release` 未到时 `acquire_buffer` 返回 `NoFreeBuffer` 而**不是**重用缓冲
  （DoD 5，用延迟 release 的 mock 覆盖）；连续 100 次显示/隐藏后 `wl_shm` pool 大小不增长（DoD 7）；
- **事件解码**：`events.rs` 对 wire 词汇的编解码；
- **调用序列**：`backend_tests.rs`（`#[path]` 挂在 `backend.rs` 下）用 mock 客户端断言请求顺序。

## 5. 本机实测记录

| 命令 | 结果 |
|---|---|
| `cargo nextest run -p ime-ui` | 全绿（含上述无显示服务器的用例） |
| `cargo check -p ime-ui` | 无告警 |
| 真机四档冒烟 | **未执行 —— 本机无可用合成器** |

## 6. 已知限制

1. **四档中没有任何一档在本机被真实合成器确认过**。协议常量是按协议描述核对的，不是按合成器核对的。
2. **T3 的可行性本身是开放的**：它依赖 §1c 的父表面问题，而这个问题只有真机能回答。
3. **`ASM-13` 的降级链是设计意图，不是实测结论**：哪个合成器落在哪一档，需要 §3 的矩阵回填。
4. 本文件不构成"Wayland 已支持"的声明。在 §3 的四项被真机确认之前，正确的表述是
   **"Wayland 后端已实现、已推理、未在真机验证；验证不可行时按 T4 降级到宿主候选列表"**。
