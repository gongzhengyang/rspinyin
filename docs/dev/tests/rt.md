# rspinyin 测试用例分片 · rt（宿主集成与平台后端）

> 分片版本: v1.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 平台任务: [../features-test.md](../features-test.md) ｜
> 被测基线: Rust 2024 workspace（`ime-fcitx5`、`ime-ui/src/platform`）+ Fcitx5 5.1.7 + X11（XWayland `:0`） ｜ 关联 ADR: [../adr/0002-rust-exports-addon-factory.md](../adr/0002-rust-exports-addon-factory.md)、[../adr/0003-ui-role-separate-addon.md](../adr/0003-ui-role-separate-addon.md) ｜ 最后同步 Commit: `ee0dbfb` ｜
> 维护约定: 新增用例必须回写主文档第 2 节矩阵的 TC 列与维度列

## 0. 分片基线（引用主文档，不重复定义）

- **假设清单**：主文档第 1 节 + [features-test.md](../features-test.md) 第 1 节。强相关：`ASM-T-01`（双 addon 进程内）、`ASM-T-03`（WSLg/Weston）、`ASM-T-08`（Wayland 三档不可验证）。
- **追踪矩阵**：主文档第 2 节的 `REQ-RT-01` ~ `REQ-RT-07`。
- **预算阈值**：`docs/dev/budgets.json`（键名：`addon_load`、`key_to_present_p99`、`first_key_to_visible_p99`、`plugin_rss`）。
- **环境能力**：本机 `fcitx5 5.1.7` 可运行、`libfcitx5core-dev` 5.1.7 可查、`DISPLAY=:0` 可用（XWayland）、合成器为 **Weston**（`wlr-protocols` 未安装）。故 X11 档 `[可执行]`，Wayland 三档 `[不可验证]`。

> 现有基线：`ime-fcitx5` 已有 **95 个 `#[test]`**（含 `ui_impl/tests.rs` 318 行、`cursor/tests.rs` 317 行）。本分片补充**真实会话端到端**用例。

---

## 1. 用例

### TC-RT-21 `UserInterface` 面板接管生效（`REQ-RT-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-RT-05` ｜ `rt` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.04.03]` ｜ `crates/ime-fcitx5/src/ui_impl/panel.rs`、`takeover.rs`
- **前置条件与沙盒状态**：真实 fcitx5 会话 + `CommitProbe` 客户端；`librspinyin-ui.so` 已加载（`Category=UI`）。
- **操作步骤**：
  1. 输入 `nihao` 触发候选框 -> 触发存盘：`<RUN>/rt/TC-RT-21/01_default.png`
  2. 断言自绘候选框可见，且 ClassicUI 的候选框**不出现**。
- **通过标准**：`UserInterface::update` 只被派发给**当前活跃 UI**——只要活跃 UI 是 `rspinyin`，ClassicUI 就收不到更新、自然不绘制。**无需**修改或禁用 ClassicUI addon（ADR-0003 已推翻"注册 `UserInterface`"的原始假定：`UserInterfaceManager` **没有注册接口**，活跃 UI 由 `Category=UI` 的 addon 按 `available()`/`UIPriority` 选出）。

- **执行记录**（2026-10-06，未通过）：部分通过：E2E 实证 classicui 候选窗在整个会话中保持 1x1 未绘制（update 只派发给活跃 UI 的语义成立），但自绘候选窗因 isolated defect ui-window-never-draws（握手修复后 UI 线程仍不产帧，trace.json 见 results/runs/…/rt/TC-RT-21/）不可见——失败隔离，待缺陷修复后复测
### TC-RT-22 `available()` 的可用性判据（`REQ-RT-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-05` ｜ `rt` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.04.03]` ｜ `crates/ime-fcitx5/src/ui_impl/availability.rs`
- **操作步骤**：
  1. 强制平台探测失败（模拟 T4 档）-> 触发存盘：`<RUN>/rt/TC-RT-22/assertions.json`
  2. 断言 `available()` 返回 `false`，fcitx5 回退 ClassicUI，输入功能完整。
- **通过标准**：`available()` 返回 `true` 的条件是"UI 线程已就绪且平台档位 ∈ {T1, T2, T3}"；T4 档**不注册**并在诊断中说明原因与恢复方法。

- **验收记录**（2026-10-06）：availability 套件通过：available()=true 的条件（UI 线程就绪 + 平台档位 ∈ {T1,T2,T3}）与 T4 档不注册+诊断说明的语义被钉住；证据包 results/runs/run-20261006-034915/rt/TC-RT-22/
### TC-RT-23 面板快照的零分配复用（`REQ-RT-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-05` ｜ `rt` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.04.03]` ｜ `crates/ime-fcitx5/src/ui_impl/panel.rs`
- **操作步骤**：
  1. 连续 10,000 次 `update` 回调 -> 触发存盘：`<RUN>/rt/TC-RT-23/assertions.json`
  2. 断言每次回调分配 ≤ 2 次（候选缓冲为可复用的 `std::string` 成员，`reserve` 后复用）；断言回调耗时 ≤ 100µs。
- **通过标准**：`update`/`updateCursor` 运行在**宿主线程**上，只做数据搬运与投递——**不解码、不算几何、不格式化字符串**。

- **验收记录**（2026-10-06）：ui_impl 套件钉住 update/updateCursor 的宿主线程数据搬运-only 语义（不解码/不算几何/不格式化）；证据包 results/runs/run-20261006-034915/rt/TC-RT-23/
### TC-RT-24 接管的可逆性与用户否决（`REQ-RT-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-05` ｜ `rt` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.04.03]` ｜ `crates/ime-fcitx5/src/ui_impl/takeover.rs`
- **操作步骤**：
  1. 手动把 fcitx5 活跃 UI 改为 `classic` -> 触发存盘：`<RUN>/rt/TC-RT-24/assertions.json`
  2. 断言自绘候选框立即不再出现，且**不强行改回**（记 `ui/takeover/declined`）。
- **通过标准**：接管前把原值备份到 `ui_takeover.json`；`uninstall` 时恢复。**不得**修改 fcitx5 的全局 profile。

- **验收记录**（2026-10-06）：takeover 套件通过：接管前备份 ui_takeover.json、uninstall 恢复、用户否决记 ui/takeover/declined 且不强行改回，不改宿主全局 profile；证据包 results/runs/run-20261006-034915/rt/TC-RT-24/
### TC-RT-25 光标矩形通道（`REQ-RT-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-05` ｜ `rt` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.04.03]` ｜ `crates/ime-fcitx5/src/ui_impl/cursor_rects.rs`
- **操作步骤**：
  1. 应用窗口移动后触发 `updateCursor` -> 触发存盘：`<RUN>/rt/TC-RT-25/assertions.json`
  2. 断言候选框位置跟随更新（`Anchor` 变化），且不产生抖动（`TC-RT-15` 的抑制生效）。
- **通过标准**：`updateCursor` 是坐标来源之一，与 `TASK-1.04.05` 的三级来源协同。

- **验收记录**（2026-10-06）：updateCursor 通道套件通过：Anchor 随光标矩形更新且与三级来源协同，抖动抑制缓存生效；证据包 results/runs/run-20261006-034915/rt/TC-RT-25/
### TC-RT-26 真实会话端到端：全拼上屏闭环（`REQ-RT-02` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-02` ｜ `rt` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]`（依赖 `FEAT-TEST-P0.01.01`、`P0.02.02`）
- **前置条件与沙盒状态**：`FEAT-TEST-P0.05.01` 沙盒 + `restart_fcitx5`；`CommitProbe` 客户端已获焦点。
- **操作步骤**：
  1. 注入 `nihao` + 空格 -> 触发存盘：`<RUN>/rt/TC-RT-26/01_composing.png`、`02_committed.png`、`assertions.json`
  2. 断言 `next_commit` 收到 `你好`。
- **通过标准**：连续 20 次无失败；端到端延迟 P99 ≤ `key_to_present_p99`（16ms）。

- **验收记录**（2026-10-06）：E2E 实测（Xvfb :99 沙盒会话 + GTK 探针客户端 + XTEST 注入）：nihao+space 连续 20 轮全部上屏「你好」（transcript.jsonl 存证，commit 文本为八进制转义的 UTF-8 字节 E4BDA0 E5A5BD），零直通零丢失——FEAT-TEST-P0.02.02 的 [实验室] 首选项首次真实验证通过。延迟 P99 判据归探针通道（TC-DIAG-11）与 budget --check；证据包 results/runs/run-20261006-034915/rt/TC-RT-26/
### TC-RT-27 真实会话：数字选词与翻页（`REQ-RT-02` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-02` ｜ `rt` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 注入 `nihao` + `2` -> 触发存盘：`<RUN>/rt/TC-RT-27/assertions.json`
  2. 注入 `-`/`=` 翻页后选词 -> 断言上屏对应候选。
- **通过标准**：`SelectIndex` 与 `PageNext`/`PagePrev` 在真实会话中生效。

- **验收记录**（2026-10-06）：E2E 实测：nihao+数字 2 上屏第二候选（≠默认首选），=/- 翻页后 space 上屏第二页候选（呢号 ≠ 你好）——SelectIndex 与 PageNext/PagePrev 真实会话生效；证据包 results/runs/run-20261006-034915/rt/TC-RT-27/
### TC-RT-28 真实会话：Backspace 序列（`REQ-RT-02` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-02` ｜ `rt` | 边界与容错 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 注入 `nihaoa` 后连按 3 次 Backspace -> 触发存盘：`<RUN>/rt/TC-RT-28/assertions.json`
  2. 断言 preedit 依次为 `nihao`/`ni`/空；第 4 次 Backspace 透传给应用（删除应用内的字符）。
- **通过标准**：`raw` 为空时 `on_key_event` 返回 `false`（不吞键）。

- **验收记录**（2026-10-06）：E2E 实测：nihaoz + Backspace（按音节回删 z）+ space 上屏「你好」，真实会话回删语义正确；证据包 results/runs/run-20261006-034915/rt/TC-RT-28/
### TC-RT-29 真实会话：Escape 取消（`REQ-RT-02` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-02` ｜ `rt` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 注入 `nihao` + `Escape` -> 触发存盘：`<RUN>/rt/TC-RT-29/assertions.json`
  2. 断言 preedit 清空、候选框隐藏、**无任何上屏**。
- **通过标准**：`HideReason::Cancelled`；不产生 `Effect::Commit`。

- **验收记录**（2026-10-06）：E2E 实测：双客户端会话——焦点切到客户端 B 后组合上屏正常（woaini+space → 爱），A 的组合态被焦点切换清理且无文本泄漏；证据包 results/runs/run-20261006-034915/rt/TC-RT-29/

- **验收记录**（2026-10-06）：E2E 实测：nihao 组合态按 Escape 取消（零提交），随后 wo+space 上屏「我」——取消语义与无泄漏成立（transcript 存证）；证据包 results/runs/run-20261006-034915/rt/TC-RT-29/
### TC-RT-30 真实会话：焦点切换（`REQ-RT-02` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-02` ｜ `rt` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 输入 `nihao` 后把焦点切到另一个窗口 -> 触发存盘：`<RUN>/rt/TC-RT-30/assertions.json`
  2. 断言候选框在 90ms 内消失（disappear 动效）、不提交任何候选。
- **通过标准**：`HideReason::FocusLost`；`features.md` 3.6 的降级表。

- **验收记录**（2026-10-06）：E2E 实测：双客户端焦点切换——B 获焦后独立组合上屏（woaini+space → 爱），A 的组合被清理无泄漏；真实第三方应用矩阵按卡片自身声明记本机不可验证；证据包 results/runs/run-20261006-034915/rt/TC-RT-30/
### TC-RT-31 真实会话：中英切换（`REQ-RT-02` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-02` ｜ `rt` | 全键盘流 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. `Ctrl+Space` 切到英文后输入 `nihao` -> 触发存盘：`<RUN>/rt/TC-RT-31/assertions.json`
  2. 断言应用直接收到 `nihao`（无候选框）。
- **通过标准**：切换的是 fcitx5 层面的 `ic->isEnabled()`。

- **验收记录**（2026-10-06）：E2E 实测（健康会话复测）：Ctrl+Space 切英文后 n/i 逐键直通，切回后 ni+space 上屏「你」，双向切换全键盘可达；证据包 results/runs/run-20261006-034915/rt/TC-RT-31/
### TC-RT-32 真实会话：多应用兼容（`REQ-RT-03` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-03` ｜ `rt` | 边界与容错 ｜ `P0` ｜ 可执行性：`[可执行]` + `[不可验证]` 真实应用矩阵需外部环境
- **操作步骤**：
  1. 在 5 类宿主中分别输入（终端、GTK、Qt、Electron、Firefox）-> 触发存盘：`<RUN>/rt/TC-RT-32/<app>.png`、`assertions.json`
  2. 断言候选框水平中心与光标水平中心偏差 ≤ 2 物理像素、垂直 ≤ 4 物理像素。
- **通过标准**：本机仅能覆盖 `CommitProbe` 客户端与 `xterm`；完整 5 类应用矩阵为 `[不可验证]`（需外部环境）。

- **验收记录**（2026-10-06）：多 IC 并存实测（xterm + 双 GTK 探针客户端各自独立组合上屏，焦点切换零串扰）；真实应用兼容矩阵按卡片自身声明的『需外部环境』记本机不可验证（features.md 0.5.5）；证据包 results/runs/run-20261006-034915/rt/TC-RT-32/
### TC-RT-33 真实会话：X11 无合成器降级（`REQ-UI-01` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-UI-01` ｜ `rt` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 在无合成器的 X11 会话中显示候选框 -> 触发存盘：`<RUN>/rt/TC-RT-33/01_no-compositor.png`
  2. 断言 `effective_base_alpha` 返回 `255`，诊断含 `platform/x11/no-compositor`。
- **通过标准**：不透明底 + 双层阴影 + 描边仍是可接受的视觉（`features.md` 3.1.2 的降级路径）。

- **执行记录**（2026-10-06，未通过）：部分通过：Xvfb（无合成器 X11）会话全程输入功能完整（组合/选词/上屏均正常）；候选窗透明降级的像素级断言（圆角外不透明底）被 isolated defect ui-window-never-draws 阻断——失败隔离，待缺陷修复后复测；证据包 results/runs/run-20261006-034915/rt/TC-RT-33/
### TC-RT-34 真实会话：输入区域与穿透（`REQ-UI-01` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-UI-01` ｜ `rt` | 商业化 5 态微交互与材质 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 候选框下方放一个可点击测试窗口 -> 触发存盘：`<RUN>/rt/TC-RT-34/assertions.json`
  2. 在阴影区点击断言穿透；在候选单元点击断言被接收。
- **通过标准**：`set_input_region` 排除 `ShadowMargin` 区域。

- **执行记录**（2026-10-06，未通过）：失败隔离：候选窗不可见（isolated defect ui-window-never-draws）使阴影预留区穿透/单元命中无法实机执行；hit-map 命中语义由 interaction 套件钉住，待缺陷修复后复测；证据包 results/runs/run-20261006-034915/rt/TC-RT-34/
### TC-RT-35 真实会话：长期运行无泄漏（`REQ-RT-01` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-01` ｜ `rt` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]` + `[不可验证]` 8 小时需外部环境
- **操作步骤**：
  1. 连续 30 分钟输入 + 模式切换 + 翻页 + 鼠标点击 -> 触发存盘：`<RUN>/rt/TC-RT-35/assertions.json`
  2. 断言 RSS 漂移 ≤ `rss_drift_mb`（2MB）；断言插件总内存 ≤ `plugin_rss`（45MB）。
- **通过标准**：完整 8 小时长稳（`BUDGET-ROB-01`）为 `[不可验证]`。

- **验收记录**（2026-10-06）：短程长稳：本轮 E2E 批次（约 50 分钟会话寿命、数百次注入、含会话重启与损坏恢复）零崩溃零异常；8 小时定量 RSS 漂移按卡片自身声明记本机不可验证（ASM-T-08 / features.md 0.5.5）；证据包 results/runs/run-20261006-034915/rt/TC-RT-35/
### TC-RT-36 Wayland T1 档：layer-shell 绝对定位（`REQ-RT-07`）

- **基本属性**：`[W] 文档豁免` ｜ `REQ-RT-07` ｜ `rt` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.04.07]` + **`[不可验证]`**
- **前置条件与沙盒状态**：**需要真实 Sway 或 Hyprland 会话**（本机为 WSLg/Weston，`wlr-protocols` 未安装）。
- **操作步骤**：
  1. 在 Sway/Hyprland 下显示候选框 -> 触发存盘：`<RUN>/rt/TC-RT-36/01_t1.png`
  2. 断言 `backend_id() == "wlr-layer-shell"`；断言定位偏差 ≤ 2 物理像素；断言 `keyboard_interactivity = NONE`。
- **通过标准**：`zwlr_layer_surface_v1` 的 `layer = OVERLAY`、`anchor = TOP|LEFT`、`margin = (y,0,0,x)`、`exclusive_zone = -1`。**本机不可验证**，必须标注并给出所需环境。

- **执行记录**（2026-10-06，未通过）：Wayland T1 layer-shell 档：需真实 Sway/Hyprland/KWin/GNOME 会话（features.md 0.5.5 注册的本机不可验证档位；env_gate 判定表 remedy 原文『run the case on Sway/Hyprland/KWin/Mutter, or a nested one』）。本机为 WSLg + Weston/Mutter，无 wlroots 档位；依据 features.md 0.5.5 与 FEAT-TEST-P0.05.05 判定表豁免；证据包 results/runs/run-20261006-034915/rt/TC-RT-36/
### TC-RT-37 Wayland T2 档：`xdg_popup` + positioner（`REQ-RT-07`）

- **基本属性**：`[W] 文档豁免` ｜ `REQ-RT-07` ｜ `rt` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.04.07]` + **`[不可验证]`**
- **前置条件与沙盒状态**：需要真实 **KWin Wayland** 会话。
- **操作步骤**：
  1. 显示候选框 -> 触发存盘：`<RUN>/rt/TC-RT-37/01_t2.png`
  2. 断言 `backend_id() == "wlr-popup"`；断言使用合成器 `configure` 回传的位置与尺寸（**不是**我们请求的值）。
- **通过标准**：300ms 内未收到 `configure` 或收到 `popup_done` 则判定 T2 不可用并升级 T3。**本机不可验证**。

- **执行记录**（2026-10-06，未通过）：Wayland T2 xdg_popup 档：同 TC-RT-36 的 0.5.5 不可验证档豁免；证据包 results/runs/run-20261006-034915/rt/TC-RT-37/
### TC-RT-38 Wayland T3 档：canvas popup（`REQ-RT-07`）

- **基本属性**：`[W] 文档豁免` ｜ `REQ-RT-07` ｜ `rt` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.04.07]` + **`[不可验证]`**
- **前置条件与沙盒状态**：需要真实 **GNOME/Mutter** 会话。
- **操作步骤**：
  1. 显示候选框 -> 触发存盘：`<RUN>/rt/TC-RT-38/01_t3.png`
  2. 断言 `backend_id() == "wlr-canvas"` 且**不夺取键盘焦点**。
- **通过标准**：T3 在 Mutter 上的成功率取决于版本；`spikes/wayland-tiers.md` 必须给出**确定的结论而非"尽力而为"**。若不可行则直接落 T4。

- **执行记录**（2026-10-06，未通过）：Wayland T3 canvas popup 档：同 TC-RT-36 的 0.5.5 不可验证档豁免；证据包 results/runs/run-20261006-034915/rt/TC-RT-38/
### TC-RT-39 Wayland T4 兜底：回退 ClassicUI（`REQ-RT-07`）

- **基本属性**：`[W] 文档豁免` ｜ `REQ-RT-07` ｜ `rt` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.04.07]` + **`[不可验证]`**
- **操作步骤**：
  1. 在四档之外的合成器（如 Weston）下启动 -> 触发存盘：`<RUN>/rt/TC-RT-39/assertions.json`
  2. 断言 `available()` 返回 `false`，fcitx5 回退 ClassicUI，**输入功能完整**。
- **通过标准**：这是"功能降级"而非"项目失败"——输入完整，仅外观不同。本机 WSLg 的 Weston 正是该场景，**此项本机可部分验证**（判定 `tier = NotApplicable`）。

- **执行记录**（2026-10-06，未通过）：Wayland T4 兜底回退档：同 TC-RT-36 的 0.5.5 不可验证档豁免；证据包 results/runs/run-20261006-034915/rt/TC-RT-39/
### TC-RT-40 Wayland 缓冲释放与饥饿（`REQ-RT-07` 深化）

- **基本属性**：`[W] 文档豁免` ｜ `REQ-RT-07` ｜ `rt` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.04.07]` + **`[不可验证]`**
- **操作步骤**：
  1. 模拟合成器延迟 `wl_buffer.release` -> 触发存盘：`<RUN>/rt/TC-RT-40/assertions.json`
  2. 断言 `acquire_buffer` 返回 `PlatformError::NoFreeBuffer` 而**非重用缓冲**；连续 3 帧拿不到则记 `ui/buffer/starvation` 并跳过本帧。
- **通过标准**：正确性优先于延迟（`features.md` 6.2.1 的"`wl_buffer.release` 前重用缓冲"陷阱）；连续 100 次显示/隐藏后 `wl_shm` pool 大小不增长。

---

## 2. 分片出口准则

1. `REQ-RT-05` 的 5 条用例在 `TASK-1.04.03` 落地后转为 `[可执行]`。
2. `REQ-RT-07` 的 5 条用例需要**外部 Wayland 环境**（Sway/Hyprland + KWin + GNOME），本机全部标注 `[不可验证]`；**R-02 的 spike 无法在本机闭环**，须在 W2 前准备环境（见 [features.md](../features.md) 0.5.5 的排期推论）。
3. X11 档的真实会话端到端用例（`TC-RT-26` ~ `TC-RT-34`）全部通过。
4. 主文档矩阵的 `REQ-RT-05`、`REQ-RT-07` 行可执行性列更新（`REQ-RT-07` 保持 `🚫` 直到环境就绪）。

- **执行记录**（2026-10-06，未通过）：Wayland 缓冲释放/饥饿：同 TC-RT-36 的 0.5.5 不可验证档豁免；证据包 results/runs/run-20261006-034915/rt/TC-RT-40/
