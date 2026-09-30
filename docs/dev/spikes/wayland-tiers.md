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

---

## 7. 自动化验证方法学（CI 侧）

> 本节记录**怎么验**，不改变 §1 的四档定义与 §2 的"本机不可验证"结论。
> 落地位置：`.github/workflows/compositor.yml` 与 `packaging/ci/compositor/*.sh`。

### 7.1 一档检查分成两半，两半的可验证性不同

| 半 | 断言内容 | 谁决定成败 |
|---|---|---|
| **environment** | 合成器起得来；它真的发布了该档代码路径需要的协议；两个 addon 都加载进去；会话运行的是构建树里的那两个 `.so` | **环境**。插件做不了任何事让它通过或失败 |
| **window** | 候选框存在、是 override-redirect（X11）/ 落在正确的档上（Wayland）、并且**从不持有键盘** | **插件**。这一半承载项目最高等级缺陷（`AGENTS.md` 禁止项 20） |

`--scope environment|window|all` 选择要跑哪一半，**退出码只描述被请求的那一半**：

| 退出码 | 含义 | 诊断码 |
|---|---|---|
| 0 | 请求范围内的每一条断言都跑了且成立 | — |
| 1 | 断言失败（插件或该档代码路径的缺陷） | — |
| 2 | 检查脚本自身的使用/环境错误 | — |
| 3 | 合成器起不来，一条断言都没跑 | `dist/verify/compositor-unavailable` |
| 4 | 请求了 window 半，但插件根本没有窗口可查 | `platform/compositor/unsupported` |

3 与 4 必须分开：3 是**机器的问题**（给一台有合成器的机器即可），4 是**插件的问题**。
合成器不可用时以 3 失败、且与代码缺陷可区分，正是本表存在的理由。

### 7.2 每一档的可观测面（"能断言什么"来自哪里）

| 档 | 环境半的第一手证据 | 窗口半的第一手证据 | 窗口半**读不到**什么 |
|---|---|---|---|
| X11 | `xdpyinfo` 的扩展列表（SHAPE / XTEST / RANDR / Composite）；根窗口 `_NET_WM_CM_S0` | X 服务器自己的答复：`xwininfo` 的 override-redirect 位、`xprop` 的 `WM_HINTS` `input = False`、`xdotool getwindowfocus`。另附项目自己的注入器 `xtask testd --probe` 的输出 | 真透明（Xvfb 无合成器，走的是不透明降级路径） |
| wlroots | `wayland-info` 的全局列表：`zwlr_layer_shell_v1` 存在 ⇒ 这一档真的是 T1 那一档 | 插件自身的诊断 | Wayland **没有任何请求可以枚举别的客户端的 surface**，所以窗口的几何与光标距离在会话外不可观测 |
| KWin | `wayland-info`：`xdg_wm_base` 存在 **且** `zwlr_layer_shell_v1` 不存在 | 同上 | 同上；另外 positioner 夹取在虚拟输出几何下未必被触发 |
| Mutter | 同上 | 同上 | 同上；Mutter 的夹取更激进，贴边路径需要真实输出 |

`zwlr_layer_shell_v1` 在 KWin / Mutter 档**必须断言不存在**，不是对称性洁癖：该协议一旦存在，
梯子会停在 T1，跑的是 layer-shell 路径，而这两个档存在的意义恰恰是 popup 路径。若不断言，
作业会在 popup 路径从未被执行的情况下报告"该档已覆盖"。

### 7.3 覆盖度状态机与"只报绿不报范围"的结构性排除

每次运行结束时写出一段覆盖度块（人读）与一行机器可读行：

```
coverage tier=<id> session=<kind> scope=<scope> environment=<state> window=<state> exit=<status>
```

`<state>` ∈ `verified` / `failed` / `skipped`，**只有 `verified` 计为覆盖**。未覆盖的部分写进
"NOT verified here" 列表并附原因，读者不必从"缺了什么"去反推边界。

`coverage` 作业（`if: always()`）汇总四档并执行两条硬约束：

1. 列在仓库变量 `RSPINYIN_COMPOSITOR_REQUIRED`（默认 `x11,wayland-wlroots`）里的档位**必须成功**，
   否则 `coverage` 作业失败——这样"把某档删掉/改名/关掉"都不能把它变成已覆盖；
2. 请求的 `scope` 包含 window 半时，任何一档报 `window=skipped` 即失败——这样某一档悄悄少验一半
   会被抓住。

`RSPINYIN_COMPOSITOR_SCOPE` 是工作流里的一行常量，不是隐藏默认值：**放宽覆盖声明必须是可评审的改动**。

读出上述一切的解析器本身由 `packaging/ci/compositor/self-test.sh` 自测——不需要合成器、不需要 fcitx5、
不需要显示服务器，`coverage` 作业在汇总之前先跑它。理由与上面两条硬约束同源：解析器悄悄匹配不到东西，
正是"报出并不存在的覆盖"那条路径。自测成对给样例：`Loaded addon rspinyin` 与
`Loaded addon rspinyin-ui` 互为前缀，必须能分辨；`Could not load addon` 不得被读成成功。

### 7.4 窗口半当前为什么是关的：插件侧的前置条件

四档的 window 半今天都跑不起来，原因**不在合成器，在插件**：

- UI addon 的初始化序列里，`platform` 与 `ui-startup` 两步仍是占位（`crates/ime-ui-addon/src/addon.rs`
  的 `pending_step`），插件自己在日志里写出
  `lifecycle/pending: platform awaits the X11 / Wayland backend probe`；
- `ui_impl::availability::window_backend_available()` 因此恒为 `false`，`available()` 返回 `false`，
  宿主继续用 ClassicUI——**这是设计内的正确降级**（宁可用宿主候选框，也不要抑制了 ClassicUI 却没东西画）；
- 因此 `WINDOW_BACKEND_AVAILABLE` 没有被任何运行时路径置位，`ProtocolClient` 目前只有测试实现
  （`backend_tests.rs` 的 `ScriptedClient`），真实协议绑定尚未接入。

检查脚本**不猜**这件事：它读插件自己的诊断行。该行消失之日，window 半自动开始运行，脚本无需改动。
在此之前，任何声称"四档已验证"的说法都超出证据范围。

### 7.5 会话隔离：被测的必须是构建树里的那两个 `.so`

检查把两个 cdylib 与三个 `.conf` 复制进一个私有目录，并用 `FCITX_ADDON_DIRS` / `FCITX_DATA_DIRS`
指过去（与 `xtask/src/testd/sandbox.rs` 同一套机制），随后读 `/proc/<pid>/maps` 断言会话映射的
正是私有目录里那两份。理由是实测过的：本机装有系统级副本，指向系统目录的会话会为**从未运行的代码**通过。

---

## 8. 实测结论：四档的当前覆盖度

> 本节是 §2 结论的**延伸**而非推翻：本机依旧四档全不可验证。变化在于"有没有外部环境可挂"。

### 8.1 每档的环境前提与本机/CI 状态

| 档 | 需要什么才能跑 environment 半 | 本机（WSL2 + WSLg） | GitHub 托管 runner | 本卡落地后 |
|---|---|---|---|---|
| X11 | 任意 X 服务器（`Xvfb` 即可，无需合成器） | **可验证**（`DISPLAY=:0`，或本脚本自起 `Xvfb`） | **可**（`xvfb`） | **CI 可验证** |
| Wayland/wlroots | Sway / Hyprland / labwc，`WLR_BACKENDS=headless` 亦可 | **否**：Weston 不实现 `zwlr_layer_shell_v1`，且 `wlr-protocols` 未安装 | **可**（`sway`，headless） | **CI 可验证** |
| Wayland/KWin | KWin 的虚拟后端可用的图形栈，或真实 KDE 会话 | **否**：无 KDE 会话 | **否**：无 DRM 设备，虚拟后端起不来 | **仍否**：作业默认 `skipped`，脚本已就绪，待有对应 runner 或 `--attach` 真会话 |
| Wayland/Mutter | 能渲染嵌套 Mutter 的机器，或真实 GNOME 会话 | **否**：无 GNOME 会话 | **否**：同上 | **仍否**：同上 |

"CI 可验证"仅指 **environment 半**。四档的 window 半今天全不可验证（§7.4）。

### 8.2 与既有验收的关系（不替代）

本卡为 `TASK-1.04.07` 与 `TASK-1.05.07` **提供环境**，不替代它们的验收：

- `TASK-1.04.07` DoD#1/#2/#3（逐档定位结果）需要 **window 半**，而 window 半依赖 §7.4 的插件前置条件；
- `TASK-1.04.07` DoD#4（四档下不夺焦点）在 X11 档已可**直接观测**（override-redirect +
  `WM_HINTS input=False` + `xdotool getwindowfocus`），在 Wayland 三档只能读插件自身的
  `platform/wayland/focus-taken` 诊断；
- `TASK-1.05.07` 的 `[视觉]` 项（光标四角可见性）需要**几何测量**，而 Wayland 在会话外不可枚举
  surface，因此本卡不覆盖它——见 §7.2 的"窗口半读不到什么"列。

### 8.3 已知限制（本节新增）

1. 四档的 window 半**全未运行**，原因是插件侧前置条件未就位，不是合成器不可用；
2. Wayland 三档的窗口几何与放置精度在会话外不可观测，需要会话内的测量手段（截图 / 合成器侧查询），
   本卡未提供；
3. KWin / Mutter 两档的脚本**从未在任何机器上执行过**——它们被有意放在默认关闭的闸门后。
   首次启用时必须先跑一次并回填本表，不得直接按"已实现"计；
4. X11 档的 `_NET_WM_CM_S0` 读的是根窗口属性，插件自己读的是同名 atom 的 selection owner；
   两者不一致时会**低估**覆盖（报"无合成器"），不会高估。
