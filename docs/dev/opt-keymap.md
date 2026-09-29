# rspinyin 键盘优先交互体系与按键派发总线

> 文档版本: v1.0 ｜ 系统形态: `Desktop GUI`（**无焦点覆盖层**变体，见 `ASM-01`） ｜ 架构基线: Fcitx5 5.1.7 进程内 addon（C++ `InputMethodEngine` 胶水 ↔ Rust `KeyAction` 契约 ↔ `ime-core` 会话状态机） ｜ 关联 ADR: `./docs/dev/adr/0000-upstream-decisions.md`、`0001-frozen-boundary-contracts.md`、`0002-rust-exports-addon-factory.md`、`0003-ui-role-separate-addon.md`（本方案新增的 ADR 需求登记于 `KEY-P0.01.05`、`KEY-P2.02.01`） ｜ 最后同步 Commit: `ee0dbfb` ｜ 维护约定: 代码演进后必须回写《按键与焦点问题总清单》状态与《全量命令-键位映射矩阵》

---

## 0. 输入参数与本次执行的取值

| 参数 | 占位符 | 本次取值 | 说明 |
|---|---|---|---|
| 参考产品 | `$1` | Vim、Raycast、Linear、Things 3、VS Code（技能默认集） | 未提供，取默认值。**领域手感另取** Rime / Fcitx5-pinyin / 微软拼音，见 `ASM-09` |
| 输出文件路径 | `$2` | `./docs/dev/opt-keymap.md` | 未提供，取默认值 |
| 系统形态 | `$3` | 自动推断 = `Desktop GUI`（无焦点覆盖层变体） | 未提供。推断依据见 `ASM-01`；修饰键映射据此改为 Linux/XKB 口径（`ASM-03`） |
| 特殊架构约束 | `$4` | 沿用现有架构约束 | 未提供。约束来源为 `AGENTS.md` 与 `docs/dev/features.md`，逐条登记于 `ASM-02`、`ASM-05`、`ASM-06`、`ASM-08` |

**与技能默认口径的两处强制偏离**（已登记为假设，非疏漏）：

1. 技能默认的 `⌘ / ⌥` ↔ `Ctrl / Alt` 跨平台映射**不适用**——本项目只发布 Linux 原生包（features.md 0.3 明确排除 Flatpak/Snap），全部键位以 Linux/XKB 口径给出（`ASM-03`）。
2. 技能默认的"应用内命令面板"模型**不适用**于 Phase 1——本产品无常驻主窗口、无菜单栏、无工具栏，`Ctrl+Shift+/` 命令面板是 Phase 2 的 `TASK-2.03.03`（`ASM-01`、`ASM-07`）。

---

## 1. 被测项目快捷键与事件监听现状诊断摘要（阶段零产出）

### 1.1 按键链路实况

扫描 `crates/`、`xtask/`、`packaging/` 后，本项目的按键通路是**四段式**，且**第三段与第四段之间没有接线**：

| 段 | 位置 | 现状 |
|---|---|---|
| ① 宿主入口 | `crates/ime-fcitx5/src/ffi/cpp/engine_glue.cpp:164` `RspinyinEngine::keyEvent` | **已实现**。把 `fcitx::KeyEvent` 折成 `FcitxKeyEvent{sym,state,is_release,time_ms}`，回调返回 `true` 时调 `event.filterAndAccept()` |
| ② FFI 边界 | `crates/ime-fcitx5/src/ffi/abi/engine.rs:50` `on_key_event` | **Stub**。注释写明"Stub: ... Until then the key must not be swallowed"，**对每一个按键无条件返回 `false`** |
| ③ 语义翻译 | `crates/ime-fcitx5/src/engine.rs:228` `translate_key` | **已实现且有完整测试**（702 行，含 200 个随机 keysym 的"绝不吞键"扫描）。**零调用者**——`grep -rn "translate_key" crates/ xtask/` 除本文件与其测试外无命中 |
| ④ 会话状态机 | `crates/ime-core/src/state/transitions.rs:46` `Session::on_key` | **已实现且有完整测试**。消费 `KeyAction`，产出 `Effects` |

链路断点在 ②→③：`ffi/abi/engine.rs:62` 把 `*event` 读进 `let _key`（下划线绑定，立即丢弃），随后 `false`。

### 1.2 散落事件监听与硬编码审查

- **无全局键盘抓取**：全仓库无 `XGrabKey`、`RegisterHotKey`、`zwp_keyboard_shortcuts_inhibit`。这是 `ASM-02` 的正确实现（features.md 3.5「快捷键实现边界」）。
- **UI 线程不接键盘**：`crates/ime-ui/src/platform/x11.rs:661-668` 的事件掩码为 `EXPOSURE | BUTTON_PRESS | BUTTON_RELEASE | POINTER_MOTION | ENTER_WINDOW | LEAVE_WINDOW | VISIBILITY_CHANGE | STRUCTURE_NOTIFY`，**不含 `KEY_PRESS`/`KEY_RELEASE`**。这与 features.md 0.4 规则 5（永不夺取键盘焦点）一致，但意味着候选窗**没有任何键盘通路**——见 `KEY-DEF-13`。
- **硬编码键位**：`crates/ime-fcitx5/src/engine.rs:284-296` 的 `Tab` / `Shift+Tab` → `MoveHighlight(±1)` 是**写死的**，不读配置；`crates/ime-fcitx5/src/engine.rs:259-278` 的 6 条和弦（`Shift_L/R`、`Ctrl+Space`、`Shift+Space`、`Ctrl+.`、`Ctrl+Shift+E`）同样写死，无配置开关。
- **修饰键掩码硬编码**：`crates/ime-fcitx5/src/engine.rs:49-70` 把 Fcitx5 5.1.7 `fcitx-utils/keysym.h` 的 `SimpleMask`（`0x1400_006d`）抄成 7 个常量，由 `engine.rs:436` 的测试钉死。无运行时校验。

### 1.3 焦点体系与上下文作用域审查

- **焦点事件是 Stub**：`crates/ime-fcitx5/src/ffi/abi/engine.rs:138-150` 的 `on_focus_in` / `on_focus_out` 均为空 Stub，`on_activate` / `on_deactivate` / `on_reset`（`engine.rs:23-43`）同样为空。`SessionEvent::FocusLost` / `Reset` 在状态机侧**已实现**（`transitions.rs:324-339`），但**无人投递**。见 `KEY-DEF-12`。
- **无上下文栈**：`translate_key` 是 `fn(&FcitxKeyEvent, &KeyBindings) -> KeyAction` 的**纯函数**，签名里没有任何状态维度。它无法区分"Idle 下的空格"与"Composing 下的空格"，而这个区分恰恰是正确性的全部。见 `KEY-DEF-18`。
- **状态机侧有状态，但答案传不回来**：`Session::handle_key`（`machine.rs:373`）返回 `Effects`（`machine.rs:228`），不是"是否消费"。而 FFI 需要的是一个 `bool`。**没有任何代码把 `Effects` 折成"这个键我处理了没有"**。见 `KEY-DEF-01`。
- **窗口自身不夺焦点**：`crates/ime-ui/src/platform/mod.rs:695-720` 有 `test_x11_backend_never_takes_input_focus`（`#[ignore]`，需真实 X 会话）。该不变量已有测试位，但被 ignore 标记，CI 不跑。

### 1.4 快捷键提示与可配置性审查

- **发现机制：不存在**。`grep -rniE "cheat|sheet|badge|tooltip|速查|提示" crates/ xtask/ packaging/` 在 `crates/` 与 `ui/` 下**零命中**（命中项全部是 `HINT_INLINE_BOUNDARIES` / `weight_hint` 等无关标识符）。features.md 3.5 的 19 行快捷键表只存在于规格文档中，产品内无任何入口能让用户看到它。见 `KEY-DEF-16`。
- **可配置性：只有 4 个键**。`crates/ime-config/src/schema.rs:337-351` 的 `KeysConfig` 有 `digit_zero` / `enter_commit_raw` / `flip_keys` / `highlight_keys`。`crates/ime-config/src/schema.rs:148-169` 的 `KeyName` 白名单有 10 项。**没有"用户自定义任意键位"的接口**，也没有回写（`Config` 是只读加载的）。
- **配置与路由不一致**：`highlight_keys` 被解析（`reload.rs:415-417`）、被校验（`schema.rs:624-629`）、被存进 `Config`（`schema.rs:461`），然后**被路由表忽略**——`KeyBindings`（`engine.rs:172-179`）根本没有这个字段。见 `KEY-DEF-03`。
- **白名单大于路由能力**：`KeyName` 的 10 项中，`left` / `right` / `page_up` / `page_down` **在路由表里没有对应行**（`engine.rs:300-317` 的 `bare_action` 只有 `minus`/`equal`/`up`/`down`）。用户写 `flip_keys = ["page_up"]` 会被配置层接受，然后静默无效。见 `KEY-DEF-05`。

### 1.5 交互路径完整性

- **鼠标路径未实现**：`crates/ime-ui/src/renderer.rs:322-334` 的 `apply_geometry_event` 只处理 `Resize` / `Scale`，注释写明"Pointer events belong to the UI thread and are left untouched"；而 `crates/ime-ui/src/` 下**不存在 `interaction.rs`**（`TASK-1.05.06` 的 Code Anchor 所指文件）。`SurfaceEvent::PointerButton` / `PointerMotion` / `Axis` 的消费者数量为 **0**。`UiEvent::Select` / `Hover` / `Page` 在全仓库**从未被构造**（仅测试与队列定义中出现）。见 `KEY-DEF-14`。
- **候选网格是占位符**：`crates/ime-ui/ui/candidate.slint:200-230` 的 `CandidateGrid` 只画空 `Rectangle`，注释自陈"the cells, the number labels and the five states land with the grid task"。用户看不到 `1`~`9` 的数字标签，也看不到键盘高亮落在哪一项。见 `KEY-DEF-15`。

---

## 2. 系统设计假设清单 (Assumptions First)

| 假设编号 | 维度 | 假设内容（基于阶段零实测） | 影响的键位域 | 偏离时的修正策略 |
|---|---|---|---|---|
| `ASM-01` | 形态 | 本产品是 **Linux 输入法插件**：Fcitx5 进程内 addon + 自绘候选窗。无常驻主窗口、无标题栏、无菜单栏、无工具栏、无启动画面。所谓"界面"仅指候选框（`candidate.slint`）与其状态条。判定为 `Desktop GUI` 的**无焦点覆盖层变体** | 全部（不存在"应用级菜单快捷键"这一键位域） | 若未来出现独立配置 GUI，必须为其单列一套键位域并复用本表的 L0–L3 上下文总线，不得另起一套派发 |
| `ASM-02` | 宿主环境 | 宿主为 Fcitx5 5.1.7（`libfcitx5core-dev`）。插件**不注册任何 X11 grab 或 Wayland 全局快捷键**；`Ctrl+Space` 等全局键由 fcitx5 的 `[Hotkey]` 段承载（features.md 3.5、`ASM-02`），我们只消费 `KeyEvent` | 全局模式键（中英、全角、标点） | 宿主不提供某能力时按 features.md 0.5.2 分档降级；**不得**自行实现协议客户端补位 |
| `ASM-03` | 修饰键基线 | **Linux/XKB 口径，无 `⌘`/`⌥`**。修饰键为 `Ctrl` / `Alt` / `Shift` / `Super`；位掩码取自 Fcitx5 5.1.7 `fcitx-utils/keysym.h` 的 `SimpleMask` = `0x1400_006d`（`CTRL\|ALT\|SHIFT\|SUPER\|SUPER2\|HYPER\|META`） | 全部和弦行 | 宿主位掩码变更时 `test_modifier_mask_matches_the_installed_key_state_header`（`engine.rs:436`）失败；按新头文件重钉常量并重跑 `TC-RT-06` 全表 |
| `ASM-04` | 响应预算 | `on_key_event` 全程（含解码、投递）**P99 ≤ 2ms**（`BUDGET-LAT-01` 第一段）；宿主回调单次 ≤ 100µs（`ffi/abi/engine.rs:76-79`） | 分发总线的每一层 | 超预算时先削减上下文栈的匹配开销（预编译为单次 `match`），**不得**削减仲裁检查（`KEY-DEF-01` 的修复不可被性能优化掉） |
| `ASM-05` | 焦点模型 | 候选窗**永不夺取键盘焦点**（features.md 0.4 规则 5、`AGENTS.md` 禁止事项 20）。X11 后端事件掩码不含 `KEY_PRESS`/`KEY_RELEASE`（`platform/x11.rs:661`），Wayland 下 `keyboard_interactivity` 必须为 `none` | 速查面板、窗口内快捷键 | 若某后端被迫加入键盘事件，必须同时加入"绝不 `XSetInputFocus`"的断言测试；否则速查面板只能由宿主线程投递 `UiFrame` 驱动，不得在窗口内自处理按键 |
| `ASM-06` | 键盘事件唯一来源 | 键盘事件只从 `fcitx::InputMethodEngine::keyEvent` 进入（`engine_glue.cpp:164`）。UI 线程**不接收**任何键盘事件 | 分发总线的输入端 | 若新增第二来源，必须在总线上加"来源上下文"字段并按 `ASM-05` 重新评估焦点模型 |
| `ASM-07` | 配置承载 | 键位配置只有 `[keys]` 段的 4 个键；键名白名单 10 项（`schema.rs:148-169`）；单列表上限 8 项（`schema.rs:32`）；文档总键数上限 120（`ASM-19`） | 全部可配置键位 | 白名单扩展时 `KeyName`、路由表、本文档三方必须同时更新，由 `KEY-P1.03.01` 的一致性断言强制 |
| `ASM-08` | 契约冻结 | `ime-types` 的 `KeyAction`（15 变体）、`UiFrame`、`UiEvent` 为**冻结契约**，变更需 ADR 与主 agent 决策（`AGENTS.md` 禁止事项 22） | 新增 `KeyAction` 变体、`UiFrame` 加字段、新增 `RequestId` | 若 ADR 未获批，退化为"引擎侧影子状态"实现（不新增变体），代价是引擎与会话的状态不一致风险，需在 `KEY-P0.01.02` 的仲裁器里显式记录该风险 |
| `ASM-09` | 参考产品集 | 交互**模型**参考 Vim / Raycast / Linear / Things 3 / VS Code（技能默认集）；交互**手感**参考 Rime / Fcitx5-pinyin / 微软拼音的中文输入约定 | 序列按键、速查面板、发现机制 | 两者冲突时，以中文输入法用户的肌肉记忆优先（例：`1`~`9` 是选词而非 VS Code 的数字键语义；`Space` 是上屏而非翻页） |
| `ASM-10` | 无障碍可达 | 候选窗是覆盖层，**无 a11y 树**；features.md 的 `FEAT-TEST-P0.02.01` 用 `UiFrame` 快照作为 a11y 树的显式替代 | 速查面板的信息呈现 | 若后续引入 a11y 树，速查面板必须同时提供可朗读文本，不得仅靠视觉呈现 |
| `ASM-11` | 测试通道 | 键位注入由 `xtask/src/testd`（XTEST）承载；但测试平台当前**直接注入 `KeyAction`**（`xtask/src/testd/engine/builtin.rs:318` 的 `Step{action: KeyAction::InputChar(ch)}`），**完全绕过路由表** | 端到端走查的可信度 | 路由表接入后必须补一层 keysym 级注入用例（`KEY-P1.03.05`），否则"表驱动测试通过"只证明了状态机，没证明按键能被翻译成那些动作 |
| `ASM-12` | 多会话模型 | v1 为单用户单会话（features.md 0.3 明确排除多 Profile 并行会话）；`ic_id` 是唯一的会话键，但 `ffi/abi/engine.rs` 的 `_ic_id` 参数当前被丢弃 | 上下文栈的作用域键 | 若引入多会话，L1/L2 上下文必须按 `ic_id` 分槽，`KEY-P0.01.01` 的栈结构需改为 `HashMap<IcId, ContextStack>` |

---

## 3. 按键与焦点问题总清单 (Keymap Defect Inventory)

**问题类别**取值：`硬编码` / `穿透` / `吞键` / `冲突` / `焦点丢失` / `无提示` / `配置失效`。
（`吞键` 与 `配置失效` 是技能默认五类的必要扩展：前者是"穿透"的反方向——该交还的键被吃掉了，本项目最大的缺陷正在这个方向；后者覆盖"配置被接受但从不生效"，它既不是硬编码也不是无提示，而是两者之间的第三种失效。）

| 问题编号 | 场景/位置（`文件:行号`） | 问题类别 | 影响描述 |
|---|---|---|---|
| `KEY-DEF-01` | `crates/ime-fcitx5/src/engine.rs:344`（`claims_key`）+ `crates/ime-core/src/state/transitions.rs:78-100`（`on_key_idle`）+ `crates/ime-fcitx5/src/ffi/abi/engine.rs:50` | 吞键 | **最高危**。`claims_key` 只看"这张表给这个键派了动作吗"，不看"有会话能执行吗"。`on_key_idle` 对 `Backspace`/`CommitHighlighted`/`CommitRaw`/`SelectIndex`/`PageNext`/`PagePrev`/`MoveHighlight`/`MoveCaret`/`ToggleLang`/`ToggleFullWidth`/`TogglePunct`/`Escape` 全部落入 `=> {}` 空臂。两者相乘的结果是：**一旦接线，Idle 态下的空格、`1`~`9`、回车、退格、Esc、方向键、Tab 会被全部 `filterAndAccept` 吃掉，且什么也不发生**——用户在浏览器里打不出空格。`engine.rs:331-339` 的文档已经写明"caller 必须同时要求一个能执行该 action 的会话"，但没有任何代码实现这个要求 |
| `KEY-DEF-02` | `crates/ime-fcitx5/src/ffi/abi/engine.rs:50-68` | 穿透 | `on_key_event` 是 Stub，对每个按键返回 `false`。第 62 行 `let _key = unsafe { *event };` 把事件读出来后立即丢弃。整个产品当前**不消费任何按键**：装好插件、选中"Rust Pinyin"、敲 `nihao`，应用收到的是字面量 `nihao` |
| `KEY-DEF-03` | `crates/ime-fcitx5/src/engine.rs:284-296` + `:172-179` | 配置失效 | `keys.highlight_keys` 被 `ime-config` 完整解析、校验、持久化（`schema.rs:350`、`reload.rs:415-417`、`schema.rs:624-629`），但 `KeyBindings` 结构体里**没有这个字段**，`shift_tolerant_action` 把 `Tab`/`Shift+Tab` 写死为 `MoveHighlight(±1)`。用户把 `highlight_keys` 改成 `["shift_tab"]` 或不改，行为完全一样，且无任何诊断 |
| `KEY-DEF-04` | `crates/ime-fcitx5/src/engine.rs:172-189` | 配置失效 | `KeyBindings` 是宿主层类型，**没有从 `ime_config::KeysConfig` 的转换**。`grep -rn "KeyBindings" crates/ xtask/` 的全部命中都在 `engine.rs` 自身与其测试内。即使 `KEY-DEF-02` 修好，配置也到不了路由表，永远跑 `KeyBindings::default()` |
| `KEY-DEF-05` | `crates/ime-config/src/schema.rs:148-169` vs `crates/ime-fcitx5/src/engine.rs:300-317` | 配置失效 | 白名单 10 项（`minus`/`equal`/`up`/`down`/`left`/`right`/`tab`/`shift_tab`/`page_up`/`page_down`）中，`left`/`right`/`page_up`/`page_down` **在路由表里没有行**。`flip_keys = ["page_up"]` 被接受、被存进 `Config`、然后被 `FlipKeys{minus,equal,up,down}` 的固定四字段结构丢弃。`page_up`/`page_down` 的 keysym 常量（`0xff55`/`0xff56`）在 `engine.rs:75-122` 中根本没有声明 |
| `KEY-DEF-06` | `crates/ime-fcitx5/src/engine.rs:231-233` + `:259-262` + `crates/ime-types/src/key.rs:19-51` | 硬编码 | features.md 3.5 要求「`Shift`（按住）全局：临时切换中/英；松开恢复」。但 `translate_key` 第 231 行对**所有** release 一律返回 `Ignore`，第 259 行对 `Shift_L`/`Shift_R` 的 press 一律返回 `ToggleLang`。`KeyAction` 的 15 个变体里**没有任何一个表达"按下修饰键"或"松开修饰键"**。结果是：按住 Shift 会**永久**切换一次中英（`ToggleLang` 是持久语义），松开什么也不做。这条规格在当前契约下**不可实现** |
| `KEY-DEF-07` | `crates/ime-fcitx5/src/engine.rs:259-269` | 冲突 | `chord_action` 在**第一行**无条件拦截 `Shift_L`/`Shift_R` 并返回 `ToggleLang`。由于该函数在 `translate_key:237` 被最先调用，`Shift` 自身的按下事件**永远**先于任何组合键被翻译。后果：(a) 每次打大写字母都会切一次中英；(b) `Shift+Space` 先产生 `ToggleLang`（Shift 按下）再产生 `ToggleFullWidth`（Space 按下）——用户要全角，得到的是中英切换 + 全角切换两个动作。`engine.rs:625` 的测试 `test_translate_key_maps_the_global_mode_chords` 把这个错误行为**钉死为期望值** |
| `KEY-DEF-08` | `crates/ime-fcitx5/src/engine.rs:285-288` + `:647-653` | 硬编码 | 字母行只匹配 `0x61..=0x7a`（小写 keysym）。但 `engine.rs:647-649` 的注释自陈"宿主会把大小写折进 symbol"——`Ctrl+Shift+E` 那一行因此专门同时匹配 `KEY_E`(0x65) 与 `KEY_E_UPPER`(0x45)。同样的折叠发生在普通字母上时，`Shift+A` 会以 `0x41` 到达，落在 `0x61..=0x7a` 之外，最终被 `:248` 的 `MODIFIER_MASK != 0` 判为 `Ignore` 交还应用。`engine.rs:653` 的 `(press(KEY_E_UPPER, SHIFT), KeyAction::Ignore)` 把这一分支钉为期望值。**路由行为取决于宿主是否折叠大小写，而这个行为未被规格定义、未被测试固定** |
| `KEY-DEF-09` | `crates/ime-core/src/state/transitions.rs:67-75` | 冲突 | `on_key_temp_english` 把 `CommitHighlighted` 列为退出临时英文模式的三个动作之一。但 `translate_key:267` 把**裸空格**翻译为 `CommitHighlighted`。于是临时英文模式下按空格会退出该模式——而 features.md 3.5 明写退出条件是 `Enter` 或 `Escape`，空格不在其中。规格与实现直接矛盾 |
| `KEY-DEF-10` | `crates/ime-fcitx5/src/engine.rs:259-278` | 无提示 | features.md 3.5 的 19 行表中，`Ctrl+Shift+/`（命令面板，`TASK-2.03.03`）与 `Ctrl+Shift+P`（诊断面板，`TASK-2.08.01`）**在路由表里没有任何行**。Phase 2 的两个入口当前无处安放，且 `chord_action` 的写法（逐条 `if`）没有为"多一条和弦"留出结构 |
| `KEY-DEF-11` | `crates/ime-core/src/state/transitions.rs:171-180` | 吞键 | `select_digit` 在 `digit_target` 返回 `None`（该数字指名的候选不在当前页）时正确地"不消费"。但消费与否由引擎决定，而引擎拿到的信息只有 `translate_key` 的 `SelectIndex(n)` 与 `claims_key` 的 `true`。`Session::handle_key` 返回的是 `Effects`，**没有通道把"我其实没处理"传回引擎**。第 4 页只有 2 个候选时按 `5`，会吃掉这个 `5` 而什么也不发生 |
| `KEY-DEF-12` | `crates/ime-fcitx5/src/ffi/abi/engine.rs:138-150` + `:23-43` | 焦点丢失 | `on_focus_in` / `on_focus_out` / `on_activate` / `on_deactivate` / `on_reset` **全部是空 Stub**。状态机侧的 `SessionEvent::FocusLost` / `Reset` 已实现（`transitions.rs:324-339`），`HideReason::FocusLost` 已在契约中（`ui.rs:188`），但无人投递。后果：焦点移走后 composition 仍活着，候选窗挂在旧位置，用户看不见却仍在"输入"；features.md 3.6 要求的"焦点丢失 → 候选框 90ms 内消失"无法满足 |
| `KEY-DEF-13` | `crates/ime-ui/src/platform/x11.rs:661-668` | 硬编码（设计约束） | 候选窗 X11 事件掩码不含 `KEY_PRESS`/`KEY_RELEASE`。**这是正确设计**（`ASM-05`），但必须登记为约束：候选窗**永远不能自己处理按键**，任何窗口内的键盘交互（速查面板、命令面板）只能由宿主线程构造 `UiFrame` 投递。若后续有人"顺手"加上键盘掩码，就是最高级别缺陷 |
| `KEY-DEF-14` | `crates/ime-ui/src/renderer.rs:322-334`；`crates/ime-ui/src/interaction.rs`（**不存在**） | 穿透 | 鼠标路径整体缺失。`apply_geometry_event` 显式忽略指针事件，`crates/ime-ui/src/` 下没有 `interaction.rs`（`TASK-1.05.06` 的 Code Anchor）。`SurfaceEvent::PointerButton`/`PointerMotion`/`Axis` 无消费者；`UiEvent::Select`/`Hover`/`Page` 从未被构造。`Session::on_ui`（`transitions.rs:238-271`）、`UiEventQueue`（`channel/event.rs`）都已就绪，缺的只是生产者 |
| `KEY-DEF-15` | `crates/ime-ui/ui/candidate.slint:200-230` | 无提示 | `CandidateGrid` 只画占位 `Rectangle`，无文本、无数字标签、无五态、无高亮。features.md 3.4 要求候选单元的 `Default`/`Hover`/`Active`/`Focus Ring`/`Disabled` 五态，3.5 要求 `1`~`9` 数字快捷键可见。用户无法知道哪个候选是"高亮项"（`Space`/`Enter` 会提交它）、哪个数字对应哪个候选 |
| `KEY-DEF-16` | 全仓库（`crates/`、`crates/ime-ui/ui/`、`packaging/`） | 无提示 | **不存在任何快捷键发现机制**。无速查面板、无状态条徽章、无配置文件注释引导、无 `--help`。features.md 3.5 的 19 行表只活在规格文档里。`Ctrl+.`（切换标点）、`Ctrl+Shift+E`（临时英文）、`0` 的 `digit_zero` 分支这些非直觉键位，用户只能靠读源码或读 features.md 得知 |
| `KEY-DEF-17` | `crates/ime-config/src/schema.rs:640-653` | 冲突 | `repair_bindings` 只在**单个列表内**去重（`unique.contains(&name)`），从不做**跨列表**检查。`flip_keys = ["up"]` 与 `highlight_keys = ["up"]` 会被同时接受。`KeyName` 白名单在两个列表间有 6 项重叠（`up`/`down`/`left`/`right`/`tab`/`shift_tab`），冲突是可达的。没有任何诊断 |
| `KEY-DEF-18` | `crates/ime-fcitx5/src/engine.rs:228-252` | 硬编码 | `translate_key` 是纯函数，签名 `fn(&FcitxKeyEvent, &KeyBindings) -> KeyAction` 里**没有状态维度**。它无法表达：分层优先级（浮层 > 组字 > 会话 > 宿主）、序列按键（`Ctrl+K` 之后等第二段）、长按修饰键。技能要求的"分层式按键上下文总线"与"Leader Key 状态机"在当前结构下无处安放 |
| `KEY-DEF-19` | `crates/ime-config/src/schema.rs:32` | 硬编码 | `MAX_KEY_BINDINGS = 8` 被声明为契约（"at most 8 of them"），但路由表的实际容量是 `flip_keys` 4 个 + `highlight_keys` 2 个（写死）。上限 8 既不是约束也不是能力，是一个既不会被触发也不描述现实的数字 |
| `KEY-DEF-20` | `crates/ime-fcitx5/src/engine.rs:49-73` | 硬编码 | 修饰键掩码 7 个常量抄自 Fcitx5 5.1.7 头文件，仅由 `engine.rs:436` 的单元测试钉死（`assert_eq!(MODIFIER_MASK, 0x1400_006d)`）。这是**构建期绊线而非运行期校验**：宿主若变更位定义，插件不会有任何诊断，只会静默地把每个和弦读错 |
| `KEY-DEF-21` | `crates/ime-fcitx5/src/addon.rs:104-113` | 配置失效 | `INIT_STEPS` 的 8 个步骤（`diagnostics`/`data-dirs`/`config`/`store-recovery`/`lexicon`/`ui-startup`/`platform`/`ui-registration`）中**没有"构建键位绑定"这一步**，也没有订阅 `ConfigWatcher` 的热重载步骤。即使路由表接好，它读到的配置在插件整个生命周期内不会更新 |
| `KEY-DEF-22` | `crates/ime-fcitx5/src/engine.rs:263-269` | 冲突 | `KEY_SPACE` 行在 `state & MODIFIER_MASK == 0` 时返回 `CommitHighlighted`。由于 `chord_action` 在最前面被调用，**裸空格在任何上下文都会走到这一行**，包括 Idle。与 `KEY-DEF-01` 相乘即为"空格被吃掉"。此外 `Shift+Space` 与 `Ctrl+Space` 的判定是 `state & MODIFIER_MASK` 的**全等比较**，多按一个修饰键（如 `Ctrl+Shift+Space`）直接落到 `_ => Ignore`，这是正确的，但没有测试覆盖该组合 |
| `KEY-DEF-23` | `crates/ime-fcitx5/src/engine.rs:289-294` | 硬编码 | `Tab` 与 `Shift+Tab` 的分支写法是"先判 `(state & SHIFT) == 0` 返回 `+1`，再无条件返回 `-1`"。由于该函数只在 `(state & NON_SHIFT_MODIFIERS) == 0` 时被调用（`engine.rs:242`），逻辑上成立，但**结构上无法表达"再按一个修饰键是什么"**。features.md 未定义 `Ctrl+Tab`，而当前实现会把它交给宿主——这个选择没有测试固定 |
| `KEY-DEF-24` | `crates/ime-ui/src/platform/mod.rs:693-720` | 焦点丢失 | `test_x11_backend_never_takes_input_focus` 存在但标了 `#[ignore = "needs a live X server"]`。这个测试守的是本项目**最高级别缺陷**（`AGENTS.md` 禁止事项 20），却不在默认 `cargo nextest run` 中执行，也不在 `just ci` 的任何步骤里 |

---

## 4. 全量命令-键位映射矩阵 (Command-Keymap Matrix)

**口径说明**：技能默认的「macOS / Windows」两列在本项目不适用（`ASM-03`）。改为 **Linux/XKB 默认键位** 一列，另加「承载方」一列说明该键由宿主还是插件消费。全部 28 条命令穷尽自 `KeyAction`（15 变体，`crates/ime-types/src/key.rs:19-51`）、`UiEvent`（5 变体，`crates/ime-types/src/ui.rs:224-249`）、features.md 3.5 的 19 行表、以及 Phase 2 登记的两个入口。

| 命令编号 | 命令/动作 | 所属上下文作用域 | 默认键位（Linux/XKB） | 承载方 | 冲突检测结果 |
|---|---|---|---|---|---|
| `CMD-01` | 输入拼音字母 `a`~`z` | L1 组字 / L2 会话 | `a`~`z` | 插件 | 无冲突（`KEY-DEF-08` 使大写形态行为未定义） |
| `CMD-02` | 输入音节分隔符 `'` | L1 组字 | `'` | 插件 | **未接线**：`translate_key` 无 `0x0027` 行，`is_input_char` 却接受它（`transitions.rs:431-433`） |
| `CMD-03` | 删除末尾音节（Backspace） | L1 组字 | `BackSpace` | 插件 | `KEY-DEF-01`：Idle 态被吞 |
| `CMD-04` | 上屏高亮候选（Space / Enter） | L1 组字 | `Space`、`Return` | 插件 | `KEY-DEF-01`、`KEY-DEF-22` |
| `CMD-05` | 上屏原始拼音串 | L1 组字 | `Return`（`keys.enter_commit_raw = true` 时） | 插件 | 无冲突 |
| `CMD-06` | 上屏第 N 个候选 | L1 组字 | `1`~`9` | 插件 | `KEY-DEF-11`：指名不存在时仍被消费 |
| `CMD-07` | 下一页候选 | L1 组字 | `=`、`Down`（`keys.flip_keys`） | 插件 | `KEY-DEF-05`：白名单里的 `page_down` 无效 |
| `CMD-08` | 上一页候选 | L1 组字 | `-`、`Up`（`keys.flip_keys`） | 插件 | `KEY-DEF-05`：白名单里的 `page_up` 无效 |
| `CMD-09` | `0` 直通字符 | L1 组字 | `0`（`keys.digit_zero = "passthrough"`） | 插件→宿主 | 无冲突 |
| `CMD-10` | `0` 翻页 | L1 组字 | `0`（`keys.digit_zero = "flip"`） | 插件 | 无冲突 |
| `CMD-11` | 高亮移到下一候选 | L1 组字 | `Tab`（`keys.highlight_keys`） | 插件 | `KEY-DEF-03`：`highlight_keys` 不生效 |
| `CMD-12` | 高亮移到上一候选 | L1 组字 | `Shift+Tab`（`keys.highlight_keys`） | 插件 | `KEY-DEF-03` |
| `CMD-13` | preedit 光标左移一音节 | L1 组字 | `Left` | 插件 | **未接线**：`Left` 只在 `bare_action` 里出现，但 `highlight_keys` 白名单里的 `left` 若被写进 `flip_keys` 会静默无效（`KEY-DEF-05`） |
| `CMD-14` | preedit 光标右移一音节 | L1 组字 | `Right` | 插件 | 同上 |
| `CMD-15` | 取消本次输入 | L1 组字 | `Escape` | 插件 | `KEY-DEF-01`：Idle 态被吞 |
| `CMD-16` | 持久切换中/英 | L3 全局 | `Ctrl+Space`（宿主 `[Hotkey]` 亦绑定） | 宿主 + 插件 | `KEY-DEF-06`、`KEY-DEF-07` |
| `CMD-17` | 按住 Shift 临时切换中/英 | L3 全局 | `Shift`（按住） | 插件 | `KEY-DEF-06`：契约无法表达；`KEY-DEF-07`：变成持久切换 |
| `CMD-18` | 切换全角/半角 | L3 全局 | `Shift+Space` | 插件 | `KEY-DEF-07`：Shift 自身行抢先产生 `ToggleLang` |
| `CMD-19` | 切换中/英标点 | L3 全局 | `Ctrl+.` | 插件 | 无冲突（`state` 全等比较，`Ctrl+Shift+.` 正确落 `Ignore`） |
| `CMD-20` | 进入临时英文模式 | L3 全局 | `Ctrl+Shift+E` | 插件 | 无冲突（`KEY_E`/`KEY_E_UPPER` 双形态已覆盖） |
| `CMD-21` | 退出临时英文模式 | L2 会话 | `Return`、`Escape` | 插件 | `KEY-DEF-09`：`Space` 也退出，与规格矛盾 |
| `CMD-22` | 交还按键给宿主（不消费） | 全部 | 未命名的任何键 | 宿主 | `KEY-DEF-01`：该交还的被吞 |
| `CMD-23` | 鼠标点击选词 | L1 组字 | `Button1` 于候选单元 | 插件 | `KEY-DEF-14`：无生产者 |
| `CMD-24` | 鼠标悬停改高亮 | L1 组字 | `PointerMotion` 于候选网格 | 插件 | `KEY-DEF-14`；且 features.md 3.4 规定「键盘高亮优先于鼠标悬停」，当前无实现 |
| `CMD-25` | 滚轮翻页 / 首页上滚关闭 | L1 组字 | `Button4`/`Button5` | 插件 | `KEY-DEF-14` |
| `CMD-26` | 打开命令面板（Phase 2） | L0 浮层 | `Ctrl+Shift+/` | 插件 | `KEY-DEF-10`：路由表无行 |
| `CMD-27` | 打开诊断面板（Phase 2） | L0 浮层 | `Ctrl+Shift+P` | 插件 | `KEY-DEF-10`：路由表无行 |
| `CMD-28` | 唤出快捷键速查面板 | L0 浮层 | `Ctrl+Shift+/`（与 `CMD-26` 复用同一浮层上下文，见 `KEY-P2.02.02`） | 插件 | 新增；`KEY-DEF-16` |

**"核心操作 100% 键盘可达"的判定**：`CMD-01`~`CMD-22` 是 Phase 1 的全部业务命令，全部有键盘键位，其中 `CMD-23`~`CMD-25` 是鼠标补充路径（features.md 3.5 明写"鼠标只是补充"）。**可达性当前不成立**，原因不是缺键位，而是 `KEY-DEF-02`（无接线）与 `KEY-DEF-01`（吞键）——见第 6 节的追溯表。

---

## 5. WBS 任务覆盖追溯表 (Traceability Matrix)

### 5.1 追溯表

| 来源编号 | 类别 | 并行通道 | 核心内容 | 绑定任务节点清单 |
|---|---|---|---|---|
| `KEY-DEF-01` | 吞键 | Track A | Idle 态全键位被消费 | `KEY-P0.01.02`、`KEY-P1.03.05` |
| `KEY-DEF-02` | 穿透 | Track B | `on_key_event` Stub，路由表零调用者 | `KEY-P1.02.05`、`KEY-P1.02.06` |
| `KEY-DEF-03` | 配置失效 | Track B | `highlight_keys` 被解析却从不生效 | `KEY-P1.02.01` |
| `KEY-DEF-04` | 配置失效 | Track B | `KeyBindings` 无 `Config` 投影 | `KEY-P0.01.05` |
| `KEY-DEF-05` | 配置失效 | Track B | 白名单与路由能力不一致（`left`/`right`/`page_up`/`page_down`） | `KEY-P1.02.02`、`KEY-P1.03.01` |
| `KEY-DEF-06` | 硬编码 | Track A | Shift 按住临时切换中/英不可实现 | `KEY-P0.01.03`、`KEY-P0.01.05` |
| `KEY-DEF-07` | 冲突 | Track B | Shift 自身行抢先消费组合键 | `KEY-P1.02.04` |
| `KEY-DEF-08` | 硬编码 | Track B | 大写字母形态无行，行为随宿主而变 | `KEY-P1.02.03` |
| `KEY-DEF-09` | 冲突 | Track B | 临时英文模式下 `Space` 退出模式 | `KEY-P1.02.07` |
| `KEY-DEF-10` | 无提示 | Track B | `Ctrl+Shift+/`、`Ctrl+Shift+P` 无行 | `KEY-P1.02.08` |
| `KEY-DEF-11` | 吞键 | Track A | 数字键指名不存在时仍被消费 | `KEY-P0.01.02` |
| `KEY-DEF-12` | 焦点丢失 | Track A | 焦点/激活/重置事件全为 Stub | `KEY-P2.01.01` |
| `KEY-DEF-13` | 硬编码 | Track A | 候选窗事件掩码不含键盘（正确设计，需固化） | `KEY-P2.01.02` |
| `KEY-DEF-14` | 穿透 | Track C | 鼠标路径无生产者 | `KEY-P1.03.04` |
| `KEY-DEF-15` | 无提示 | Track C | 候选网格无数字标签与高亮态 | `KEY-P1.03.03` |
| `KEY-DEF-16` | 无提示 | Track C | 无快捷键发现机制 | `KEY-P2.02.01`、`KEY-P2.02.02`、`KEY-P2.02.03` |
| `KEY-DEF-17` | 冲突 | Track C | 无跨列表键位冲突校验 | `KEY-P1.03.02` |
| `KEY-DEF-18` | 硬编码 | Track A | 路由表纯函数、无上下文栈、无序列状态 | `KEY-P0.01.01`、`KEY-P0.01.04` |
| `KEY-DEF-19` | 硬编码 | Track C | `MAX_KEY_BINDINGS = 8` 与路由容量不一致 | `KEY-P1.03.01` |
| `KEY-DEF-20` | 硬编码 | Track A | 修饰键掩码硬编码，仅构建期绊线 | `KEY-P0.01.01` |
| `KEY-DEF-21` | 配置失效 | Track B | `INIT_STEPS` 无键位构建与热重载 | `KEY-P0.01.05`、`KEY-P1.02.06` |
| `KEY-DEF-22` | 冲突 | Track B | 裸空格在任何上下文都返回 `CommitHighlighted` | `KEY-P1.02.04` |
| `KEY-DEF-23` | 硬编码 | Track B | `Tab` 分支结构无法表达额外修饰键 | `KEY-P1.02.01` |
| `KEY-DEF-24` | 焦点丢失 | Track A | 不夺焦点测试被 `#[ignore]` 排除在 CI 外 | `KEY-P2.01.02` |
| `CMD-01` | 命令键位 | Track B | 输入拼音字母 | `KEY-P1.02.03`、`KEY-P1.03.05` |
| `CMD-02` | 命令键位 | Track B | 输入音节分隔符 `'` | `KEY-P1.02.02` |
| `CMD-03` | 命令键位 | Track A | Backspace 删除末尾音节 | `KEY-P0.01.02` |
| `CMD-04` | 命令键位 | Track B | Space/Enter 上屏高亮候选 | `KEY-P1.02.04` |
| `CMD-05` | 命令键位 | Track B | Enter 上屏原始拼音串 | `KEY-P1.02.04` |
| `CMD-06` | 命令键位 | Track A | `1`~`9` 上屏第 N 个候选 | `KEY-P0.01.02` |
| `CMD-07` | 命令键位 | Track B | 下一页候选 | `KEY-P1.02.02` |
| `CMD-08` | 命令键位 | Track B | 上一页候选 | `KEY-P1.02.02` |
| `CMD-09` | 命令键位 | Track B | `0` 直通字符 | `KEY-P1.02.02` |
| `CMD-10` | 命令键位 | Track B | `0` 翻页 | `KEY-P1.02.02` |
| `CMD-11` | 命令键位 | Track B | 高亮移到下一候选 | `KEY-P1.02.01` |
| `CMD-12` | 命令键位 | Track B | 高亮移到上一候选 | `KEY-P1.02.01` |
| `CMD-13` | 命令键位 | Track B | preedit 光标左移 | `KEY-P1.02.02` |
| `CMD-14` | 命令键位 | Track B | preedit 光标右移 | `KEY-P1.02.02` |
| `CMD-15` | 命令键位 | Track A | Escape 取消输入 | `KEY-P0.01.02` |
| `CMD-16` | 命令键位 | Track B | 持久切换中/英 | `KEY-P1.02.04` |
| `CMD-17` | 命令键位 | Track A | Shift 按住临时切换中/英 | `KEY-P0.01.03` |
| `CMD-18` | 命令键位 | Track B | 切换全角/半角 | `KEY-P1.02.04` |
| `CMD-19` | 命令键位 | Track B | 切换中/英标点 | `KEY-P1.02.04` |
| `CMD-20` | 命令键位 | Track B | 进入临时英文模式 | `KEY-P1.02.04` |
| `CMD-21` | 命令键位 | Track B | 退出临时英文模式 | `KEY-P1.02.07` |
| `CMD-22` | 命令键位 | Track A | 交还按键给宿主 | `KEY-P0.01.02` |
| `CMD-23` | 命令键位 | Track C | 鼠标点击选词 | `KEY-P1.03.04` |
| `CMD-24` | 命令键位 | Track C | 鼠标悬停改高亮 | `KEY-P1.03.04` |
| `CMD-25` | 命令键位 | Track C | 滚轮翻页 / 首页上滚关闭 | `KEY-P1.03.04` |
| `CMD-26` | 命令键位 | Track B | 打开命令面板（Phase 2） | `KEY-P1.02.08` |
| `CMD-27` | 命令键位 | Track B | 打开诊断面板（Phase 2） | `KEY-P1.02.08` |
| `CMD-28` | 命令键位 | Track C | 唤出快捷键速查面板 | `KEY-P2.02.02` |

**双向一致性校验**：上表 24 条 `KEY-DEF` + 28 条 `CMD` 共 52 行，每行至少挂载一张任务卡；第 7 节（P0）、`./docs/dev/opt-keymap/phase-2.md`（P1）、`./docs/dev/opt-keymap/phase-3.md`（P2）的每一张卡都在"绑定来源编号"字段回填了上表的编号。**无无主任务，无无任务覆盖的遗留项。**

### 5.2 DAG 校验

- 全部 26 张卡的编号形如 `KEY-P<优先级>.<模块序列>.<任务序号>`，依赖只指向 `(优先级, 模块序列, 任务序号)` 三元组严格更小的编号，**单向流动成立**。
- 环检测：对 26 个节点做拓扑排序，入度全部可归零，**无环**。
- 三处显式解耦点（编号相同的优先级内）：
  1. `KEY-P0.01.01`（总线）与 `KEY-P0.01.03`（修饰键状态机）都只依赖"无"，可并行开工，通过 `KeyContext` 与 `ModifierHold` 两个独立结构体解耦。
  2. `KEY-P1.02.0x`（路由表各行）之间**无依赖**，共享 `KEY-P0.01.05` 的 `KeyBindings` 投影作为冻结接口，六张卡可完全并行。
  3. `KEY-P1.03.03`（候选网格渲染）与 `KEY-P1.03.04`（鼠标路径）之间以 `crates/ime-ui/src/layout.rs` 的 `GridLayout` 为契约：渲染按 `(row, col)` 定位，命中测试按同一 `GridLayout` 反查，两者只共享这一个结构体。
- 契约冻结顺序：`KEY-P0.01.01` 冻结 `KeyContext` 与 `Consumed`；`KEY-P0.01.05` 冻结 `KeyBindings`（含 `highlight_keys`）；`KEY-P2.02.01` 需要 ADR 冻结 `UiFrame` 的速查面板扩展。**在这三个点之前，下游不得开工。**

### 5.3 关键路径与并行通道汇总

**最长依赖链（10 节点，决定整体交付时间）**：

```
KEY-P0.01.01 ──> KEY-P0.01.02 ──> KEY-P0.01.05 ──> KEY-P1.02.05 ──> KEY-P1.02.06
                                                                          │
KEY-P2.03.02 <── KEY-P2.02.02 <── KEY-P2.02.01 <── KEY-P2.01.02 <── KEY-P2.01.01
```

| 通道 | 职责 | 任务卡 | 可并行度 |
|---|---|---|---|
| **Track A**（按键总线与焦点引擎） | 分层上下文总线、声明-可执行仲裁、修饰键持有状态机、序列按键状态机、焦点生命周期 | `KEY-P0.01.01`、`KEY-P0.01.02`、`KEY-P0.01.03`、`KEY-P0.01.04`、`KEY-P2.01.01`、`KEY-P2.01.02`、`KEY-P2.01.03` | `KEY-P0.01.01` 与 `KEY-P0.01.03` 可同时开工；`KEY-P2.01.x` 串行于 `KEY-P1.02.06` |
| **Track B**（命令接入与键位矩阵） | `Config` → `KeyBindings` 投影、路由表补齐、Effect 执行、会话接线、Phase 2 入口预留 | `KEY-P0.01.05`、`KEY-P1.02.01`~`KEY-P1.02.08` | `KEY-P1.02.01`~`04`、`07`、`08` 六张卡可完全并行；`05` → `06` 串行 |
| **Track C**（速查面板·持久化·基建） | 白名单一致性断言、跨列表冲突校验、候选网格标签、鼠标路径、速查面板、持久化、端到端走查 | `KEY-P1.03.01`~`KEY-P1.03.05`、`KEY-P2.02.01`~`KEY-P2.02.03`、`KEY-P2.03.01`、`KEY-P2.03.02` | `KEY-P1.03.03` 与 `KEY-P1.03.04` 可并行；`KEY-P2.02.x` 串行于 `KEY-P2.01.02` |

**关键路径节点（CP: 是，共 10 张）**：`KEY-P0.01.01`、`KEY-P0.01.02`、`KEY-P0.01.05`、`KEY-P1.02.05`、`KEY-P1.02.06`、`KEY-P2.01.01`、`KEY-P2.01.02`、`KEY-P2.02.01`、`KEY-P2.02.02`、`KEY-P2.03.02`。

**关键路径之外的 16 张卡**可以任意顺序认领，只要不违反 5.2 的依赖。

**最早可交付里程碑**：`KEY-P0.01.01` → `KEY-P0.01.02` → `KEY-P1.02.05` 三张卡完成即可让插件**第一次真正消费按键**（`KEY-DEF-02` 关闭）。这是本方案的最小可用集，也是唯一能解除 `KEY-DEF-01` 的高危状态的路径。

---

## 6. P0 任务卡（原子级展开）

> P0 定义：**按键上下文分发引擎**。P0 全部完成前，`KEY-DEF-01`（吞键）与 `KEY-DEF-02`（未接线）两个最高危缺陷无法关闭，产品的任何按键行为都不可信。
>
> P1 卡片见 `./docs/dev/opt-keymap/phase-2.md`，P2 卡片见 `./docs/dev/opt-keymap/phase-3.md`。

---

### 任务 ID：KEY-P0.01.01 分层按键上下文总线与优先级拦截树

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-18`、`KEY-DEF-20`
  - 优先级与复杂度：`P0 | 高 | 预估工时: 4.0 人天`
  - 前置依赖：无
  - 关键路径：`CP: 是`
  - 并行通道：`Track A 按键总线与焦点引擎`
  - 代码落地锚点：`crates/ime-fcitx5/src/engine.rs`（改造 `translate_key` 的调用方）、`crates/ime-fcitx5/src/engine/context.rs`（新建）、`crates/ime-fcitx5/src/engine/modifier.rs`（新建）
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  建立严格的事件优先级拦截树。**匹配顺序即优先级，一旦命中并消费即刻阻断传播**：

  | 层 | 上下文名 | 进入条件（实测自 `SessionState` 与引擎侧标志） | 消费的键位域 |
  |---|---|---|---|
  | L0 | `ModalOverlay` 模态浮层 | 速查面板 / 命令面板 / 诊断面板处于打开态（`KEY-P2.02.02` 引入，P0 阶段恒为"未打开"） | `Escape` 关闭浮层；`Up`/`Down` 在面板内移动；`Return` 确认；其余透传至 L1 |
  | L1 | `Composition` 组字上下文 | `session.state() == Composing` | `a`~`z`、`'`、`1`~`9`、`0`、`Space`、`Return`、`BackSpace`、`Escape`、`Tab`/`Shift+Tab`、`Left`/`Right`、`-`/`=`/`Up`/`Down` |
  | L2 | `Session` 会话上下文 | 存在活跃会话但未组字：`Idle`（有 ic）、`TempEnglish`、`Cancelling`、`Committing` | 仅 `Ctrl+Shift+E`（进入临时英文）、`Shift` 按住、模式和弦；**其余一律不消费** |
  | L3 | `Host` 全局底座 | 无会话，或 L0–L2 均未消费 | 无（插件在此层不消费任何键，全部交还宿主） |

  这一层结构**直接修掉 `KEY-DEF-01`**：`CommitHighlighted`、`SelectIndex`、`Backspace`、`Escape` 等键在 L1 之外根本不会被派发，因此 `claims_key` 不会再对它们返回 `true`。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **① 强类型按键事件与消费结果**（`crates/ime-fcitx5/src/engine/context.rs`）：

  ```rust
  /// Which context a key was matched in. Ordered by priority: a lower variant
  /// wins over a higher one, and `Ord` is derived so the tree can be sorted.
  #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
  pub enum KeyContext {
      /// A modal overlay owns the keyboard: the cheat sheet or the command palette.
      ModalOverlay,
      /// A composition is live: the full composing keymap applies.
      Composition,
      /// A session exists but nothing is composing.
      Session,
      /// Nothing of ours is live; every key belongs to the host.
      Host,
  }

  /// The result of dispatching one key.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum Consumed {
      /// The plugin acted on the key: the caller calls `filterAndAccept`.
      Consumed,
      /// The key belongs to the host: the caller leaves the event alone.
      Ignored,
      /// The key opened a sequence and the plugin is waiting for the next one.
      /// The key itself is consumed, but nothing is committed yet.
      ChainPending,
  }

  /// One key event as the dispatcher sees it.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct KeyEvent {
      /// XKB keysym, passed through from `fcitx::Key::sym()`.
      pub sym: u32,
      /// Fcitx5 modifier bit mask.
      pub state: u32,
      /// `true` for the release edge.
      pub is_release: bool,
      /// Host timestamp, in milliseconds. Diagnostics only.
      pub time_ms: u32,
  }

  /// The dispatcher's per-input-context state.
  pub struct Dispatcher {
      /// The modifier the user is holding down, if any (`KEY-P0.01.03`).
      hold: ModifierHold,
      /// The sequence in flight, if any (`KEY-P0.01.04`).
      sequence: KeySequence,
      /// The overlay currently open, if any (`KEY-P2.02.02`).
      overlay: Option<Overlay>,
      /// The bindings projected from `[keys]` (`KEY-P0.01.05`).
      bindings: KeyBindings,
  }
  ```

  **② 优先级拦截树的状态机转移条件**：

  ```rust
  impl Dispatcher {
      /// Dispatches one key through the context tree, highest priority first.
      ///
      /// The first context that consumes the key stops the walk. A context that
      /// answers `Ignored` passes the key down; `ChainPending` stops the walk
      /// exactly like `Consumed` because the key is already ours.
      pub fn dispatch(&mut self, event: &KeyEvent, session: &SessionView<'_>) -> Consumed {
          for context in self.active_contexts(session) {
              match self.dispatch_in(context, event, session) {
                  Consumed::Ignored => continue,
                  decided => return decided,
              }
          }
          Consumed::Ignored
      }

      /// The contexts to try, highest priority first.
      ///
      /// The walk is over a fixed three-element array rather than a heap: the
      /// tree has four levels and the first one is a boolean, so a `Vec` would
      /// allocate on a path that has 100us to spend.
      fn active_contexts(&self, session: &SessionView<'_>) -> [KeyContext; 3] {
          // ...
      }
  }
  ```

  转移条件表（`dispatch_in` 的分派依据）：

  | 当前层 | 进入下一层的条件 | 终止条件 |
  |---|---|---|
  | `ModalOverlay` | `overlay.is_none()` | 浮层打开且该键是浮层的键 → `Consumed` |
  | `Composition` | `session.state() != Composing` 或 `session.temp_english` | 该键在组字键位域内且 `translate_key` 有行 → `Consumed` |
  | `Session` | `session.is_absent()` | 该键是会话级和弦（`Ctrl+Shift+E`、修饰键持有） → `Consumed` |
  | `Host` | — | 恒为 `Ignored` |

  **③ 修饰键掩码的运行期校验**（关闭 `KEY-DEF-20` 的"仅构建期绊线"）：

  ```rust
  /// The modifier bits this build was compiled against, plus the host's answer.
  ///
  /// The unit test at the bottom of `engine.rs` pins the constant against the
  /// header it was copied from; this function is the run-time half: the addon
  /// asks the host once at load and records `platform/modifier-mask-mismatch`
  /// when the two disagree, so a renumbered host is visible in the log rather
  /// than silent.
  fn check_modifier_mask(host_mask: u32) -> Result<(), ImeError> { /* ... */ }
  ```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在 `crates/ime-fcitx5/src/engine/context.rs` 写 `KeyContext` / `Consumed` / `KeyEvent` / `Dispatcher` 四个类型与 `dispatch` 的骨架，`active_contexts` 先返回常量数组。
  2. 把 `engine.rs` 的 `translate_key` 改为 `Dispatcher::dispatch_in` 的 L1/L2 分支实现，保留原有的纯函数 `translate_key` 作为 L1 的实现细节（不删，它是 `TC-RT-06` 的被测单元）。
  3. 在 `crates/ime-fcitx5/src/ffi/cpp/engine_glue.cpp` 增加一次性修饰键掩码上报调用（`KEY-P0.01.01` 只做校验与诊断，不改 `keyEvent` 的消费逻辑）。
  4. 写 `check_modifier_mask` 与 `platform/modifier-mask-mismatch` 诊断。
  5. 写表驱动测试：四层上下文 × 每层的键位域 × 三种消费结果，断言"低优先级的层不会看到已被消费的键"。
- **验收标准 (DoD)**：
  - [ ] 纯键盘端到端走查：`Dispatcher::dispatch` 对 `(KeyContext, KeyEvent)` 的每一种组合都有确定答案，无 `unreachable!` 分支被触发；
  - [ ] `Consumed::Ignored` 的传播路径有测试证明"每一层都试过之后才交还宿主"；
  - [ ] `ModalOverlay` 关闭时 `dispatch` 的行为与未引入该层时逐键一致（用 `KEY-P0.01.01` 前后的 `translate_key` 全表对拍）；
  - [ ] `check_modifier_mask` 在宿主掩码不匹配时返回 `Err` 并记 `platform/modifier-mask-mismatch`，匹配时不记任何日志。

---

### 任务 ID：KEY-P0.01.02 声明-可执行仲裁器（Claim Arbitrator）

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-01`、`KEY-DEF-11`、`CMD-03`、`CMD-06`、`CMD-15`、`CMD-22`
  - 优先级与复杂度：`P0 | 高 | 预估工时: 3.5 人天`
  - 前置依赖：`KEY-P0.01.01`
  - 关键路径：`CP: 是`
  - 并行通道：`Track A 按键总线与焦点引擎`
  - 代码落地锚点：`crates/ime-fcitx5/src/engine/arbiter.rs`（新建）、`crates/ime-core/src/state/machine.rs`（新增只读查询方法）、`crates/ime-core/src/state/mod.rs`（导出）
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  本卡关闭本项目**最高危**的缺陷 `KEY-DEF-01`。规则只有一条，但必须由代码强制而不是由注释声明：

  > **一个按键只有在"路由表给它派了动作"**且**"当前会话能执行该动作"**两个条件同时成立时，才可以被 `filterAndAccept`。**

  | 键位 | 路由表答案 | 会话可执行？ | 最终结果 | 修复前 |
  |---|---|---|---|---|
  | Idle 下 `Space` | `CommitHighlighted` | 否（`Idle` 无候选可提交） | `Ignored` | `Consumed`（空格被吃） |
  | Idle 下 `3` | `SelectIndex(3)` | 否 | `Ignored` | `Consumed`（数字被吃） |
  | Idle 下 `Return` | `CommitHighlighted` | 否 | `Ignored` | `Consumed`（回车被吃） |
  | Idle 下 `BackSpace` | `Backspace` | 否 | `Ignored` | `Consumed`（退格被吃） |
  | Idle 下 `Escape` | `Escape` | 否 | `Ignored` | `Consumed`（Esc 被吃） |
  | Idle 下 `Up` | `PagePrev` | 否 | `Ignored` | `Consumed` |
  | Composing 下 `Space` | `CommitHighlighted` | 是（有高亮候选） | `Consumed` | `Consumed` |
  | Composing 下 `5`（本页仅 3 项） | `SelectIndex(5)` | 否（`digit_target` 返回 `None`） | `Ignored` | `Consumed` |
  | Composing 下 `Ctrl+Shift+E` | `EnterTempEnglish` | 是 | `Consumed` | `Consumed` |
  | 任何状态下 `Ctrl+Q` | `Ignore` | — | `Ignored` | `Ignored` |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **① 会话侧新增只读查询**（`crates/ime-core/src/state/machine.rs`，**不新增 `KeyAction` 变体，不触碰冻结契约**）：

  ```rust
  /// What the session can do with an action, without performing it.
  ///
  /// The dispatcher needs this to answer "may this key be taken from the
  /// application?", and the answer must be exactly the one the step would give:
  /// a key consumed but not acted on is the defect this whole module exists to
  /// prevent. The method therefore mirrors the guards in `transitions.rs`
  /// one for one, and a test walks every `KeyAction` variant against both to
  /// prove the mirror is exact.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum Executability {
      /// The step would act on this action.
      Executable,
      /// The step would do nothing: the key must travel on.
      Inert,
  }

  impl Session {
      /// Reports whether a step with this action would change anything.
      pub fn executability(&self, action: KeyAction, cfg: &SessionConfig) -> Executability { /* ... */ }
  }
  ```

  **② 仲裁器**（`crates/ime-fcitx5/src/engine/arbiter.rs`）：

  ```rust
  /// Decides whether the plugin may take a key from the application.
  ///
  /// Two conditions, and both must hold: the routing table has to name the key,
  /// and a live session has to be able to act on the resulting action. The first
  /// is `translate_key`'s answer, the second is `Session::executability`'s. The
  /// pair is what `on_key_event` returns, and it is the only place in the crate
  /// that decides it -- a second call site would be a second answer.
  pub fn arbitrate(action: KeyAction, session: Option<&SessionView<'_>>, cfg: &SessionConfig) -> Consumed {
      if !claims_key(action) {
          return Consumed::Ignored;
      }
      match session {
          Some(view) if view.executability(action, cfg) == Executability::Executable => {
              Consumed::Consumed
          }
          // The table names the key but nothing can act on it. This is the
          // branch `KEY-DEF-01` was: without it, every routed key is eaten
          // while the session is idle.
          _ => Consumed::Ignored,
      }
  }
  ```

  **③ 镜像性证明测试**（防止两个 `match` 漂移）：

  ```rust
  /// Walks every `KeyAction` variant in every `SessionState` and asserts that
  /// `executability` agrees with what `step` actually produced.
  ///
  /// The two must agree exactly: `Executable` with an empty effect list is a
  /// swallowed key, and `Inert` with a non-empty one is a key we acted on but
  /// handed back. Either is the defect the arbiter exists to prevent, so a
  /// drift is caught here rather than in the user's typing.
  #[test]
  fn test_executability_mirrors_what_step_actually_does() { /* ... */ }
  ```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在 `machine.rs` 写 `Executability` 与 `Session::executability`，逐条镜像 `transitions.rs` 的 `on_key_idle` / `on_key_composing` / `on_key_temp_english` 守卫。
  2. 写镜像性证明测试（15 个 `KeyAction` 变体 × 4 个 `SessionState` + `temp_english` 两态）。
  3. 在 `engine/arbiter.rs` 写 `arbitrate`，接上 `claims_key` 与 `executability`。
  4. 写"Idle 态全键位交还"的表驱动测试：对 features.md 3.5 的 19 行逐行断言 Idle 下的最终结果。
  5. 在 `SessionView` 上加 `state()` / `temp_english()` / `candidate_count()` 三个只读访问器（若尚无）。
- **验收标准 (DoD)**：
  - [ ] Idle 态下按 `Space`/`1`~`9`/`Return`/`BackSpace`/`Escape`/方向键/Tab，`arbitrate` 全部返回 `Ignored`；
  - [ ] Composing 态下同一批键全部返回 `Consumed`；
  - [ ] `executability` 与 `step` 的镜像性测试覆盖全部 15 个 `KeyAction` 变体，零漂移；
  - [ ] `TC-RT-07`（200 个随机 keysym 的"绝不吞键"）在接入 `arbitrate` 后仍然通过——即返回 `Consumed` 的那些必须产生了至少一个非 `Diagnose` 的 `Effect`。

---

### 任务 ID：KEY-P0.01.03 修饰键持有状态机（Modifier Hold）

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-06`、`CMD-17`
  - 优先级与复杂度：`P0 | 高 | 预估工时: 3.0 人天`
  - 前置依赖：`KEY-P0.01.01`
  - 关键路径：`CP: 否`
  - 并行通道：`Track A 按键总线与焦点引擎`
  - 代码落地锚点：`crates/ime-fcitx5/src/engine/modifier.rs`（新建）、`crates/ime-fcitx5/src/engine/context.rs`
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  features.md 3.5 要求「`Shift`（按住）全局：临时切换中/英；松开恢复」。当前契约下这**不可实现**——`translate_key:231` 对所有 release 返回 `Ignore`，`KeyAction` 的 15 个变体里没有一个表达"修饰键按下/松开"。

  **本卡采用 `ASM-08` 的降级路径：不新增 `KeyAction` 变体，把"持有"做成引擎侧状态。** 理由：`KeyAction` 是冻结契约，新增变体需 ADR 与主 agent 决策；而"按住 Shift 临时切英文"本质是**宿主层模式**（与 `ToggleLang` 修改 fcitx5 的 `ic->setEnabled()` 同层），不属于会话语义，放在引擎侧是正确的分层而非权宜之计。

  | 事件 | 引擎动作 | 是否消费 | 备注 |
  |---|---|---|---|
  | `Shift_L`/`Shift_R` **press** | `hold.arm(SHIFT)`；记下当前 `ic->isEnabled()` | `Ignored` | **不再返回 `ToggleLang`**（修 `KEY-DEF-07` 的一半） |
  | `Shift_L`/`Shift_R` **release** | 若 `hold` 已 armed 且期间无其他键被消费 → 恢复 arm 时记下的使能态；`hold.clear()` | `Ignored` | 必须消费 release 才能观察到它——见下方 ③ |
  | 持有 Shift 期间的**字母键** | 正常走 L1/L2 派发；**`hold.mark_used()`** | 按路由表 | `mark_used` 是"这次 Shift 是打字用的，不是切模式"的判据 |
  | 持有 Shift 期间的**其他消费键** | 正常走派发；`hold.mark_used()` | 按路由表 | 同上 |
  | 持有 Shift 超过 `HOLD_THRESHOLD`（250ms）且**无任何键被消费** | 判定为"长按修饰键"，触发 `KEY-P2.02.02` 的速查面板唤出 | `Ignored` | 与 `ASM-04` 的 2ms 预算无关：判定发生在**下一个**按键或 release 到达时，不引入定时器（`features.md` 2.1 禁止轮询定时器） |

- **工程实现方案与代码级实现 (Diff / Implementation)**：

  **① 持有状态**（`crates/ime-fcitx5/src/engine/modifier.rs`）：

  ```rust
  /// How long a modifier must be held, with nothing typed, to count as a long press.
  ///
  /// The threshold is read when the release arrives rather than watched by a
  /// timer: the architecture forbids polling loops, and a modifier that is held
  /// and then released is exactly one event pair, so the elapsed time is known
  /// without waking anything up.
  pub const HOLD_THRESHOLD_MS: u32 = 250;

  /// The modifier the user is holding, and what happened while it was down.
  #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
  pub struct ModifierHold {
      /// The modifier currently held, if any.
      armed: Option<HeldModifier>,
  }

  /// One held modifier and the state it interrupted.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  struct HeldModifier {
      /// Which modifier key started the hold.
      key: ModifierKey,
      /// The host timestamp of the press, for the long-press test.
      pressed_at_ms: u32,
      /// Whether the host's input method was enabled when the hold started.
      was_enabled: bool,
      /// Whether any key was consumed while the modifier was down.
      used: bool,
  }

  impl ModifierHold {
      /// Records a modifier press and the state it interrupted.
      pub fn arm(&mut self, key: ModifierKey, at_ms: u32, was_enabled: bool) { /* ... */ }

      /// Records that a key was acted on while the modifier was down.
      pub fn mark_used(&mut self) { /* ... */ }

      /// Ends the hold and answers what the release should do.
      ///
      /// `Restore` means the modifier was a mode switch and the previous state
      /// comes back; `Nothing` means it was part of a chord or a plain
      /// keystroke and the release is a no-op.
      pub fn release(&mut self, key: ModifierKey, at_ms: u32) -> HoldOutcome { /* ... */ }
  }

  /// What a modifier release means.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum HoldOutcome {
      /// The hold changed nothing; the release is ignored.
      Nothing,
      /// The hold switched the input mode and the release restores it.
      Restore { was_enabled: bool },
      /// The modifier was held with nothing typed: a long press.
      LongPress { held_ms: u32 },
  }
  ```

  **② 释放边沿必须被观察到**（关闭 `KEY-DEF-06` 的机制障碍）：

  `translate_key:231` 现在对**所有** release 返回 `Ignore`，因此引擎永远看不到 Shift 的抬起。修法是：`Dispatcher::dispatch` 在进入上下文树**之前**先问 `ModifierHold`——只有"我正在追踪的那个修饰键的 release"才被 `ModifierHold` 消费，其余 release 仍然一律 `Ignored`。

  ```rust
  // In `Dispatcher::dispatch`, before the context walk.
  if event.is_release {
      // The one release the plugin watches: the modifier it armed. Every other
      // release belongs to the application's key-up and must travel on.
      return match self.hold.release(ModifierKey::from_sym(event.sym), event.time_ms) {
          HoldOutcome::Nothing => Consumed::Ignored,
          HoldOutcome::Restore { was_enabled } => {
              self.pending_restore = Some(was_enabled);
              Consumed::Consumed
          }
          HoldOutcome::LongPress { held_ms } => {
              self.pending_long_press = Some(held_ms);
              Consumed::Consumed
          }
      };
  }
  ```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 写 `modifier.rs` 的 `ModifierHold` / `HeldModifier` / `HoldOutcome` 与四个方法。
  2. 在 `Dispatcher::dispatch` 的最前面插入 release 分支，把"追踪中的修饰键 release"从"一律 Ignore"里分出来。
  3. 在 `translate_key` 中**删除** `Shift_L`/`Shift_R` 的 `ToggleLang` 行（`engine.rs:259-262`），改为由 `ModifierHold` 处理；同步修改 `engine.rs:625` 的 `test_translate_key_maps_the_global_mode_chords`，把 `Shift` 的两行改为断言 `Ignored`。
  4. 在 `SessionView` 之外增加 `HostMode` 抽象（读写 `ic->isEnabled()` 的接口），使 `ModifierHold` 的 `Restore` 可被单测（用 mock 实现）。
  5. 写测试：按住 Shift + 打字母（不得切换模式）、按住 Shift 不按键再松开（不得切换模式，触发长按）、Shift 与任何组合键共存（`Shift+Space` 不得产生 `ToggleLang`）。
- **验收标准 (DoD)**：
  - [ ] 按住 `Shift` 期间输入 `nihao`，中英模式**不发生任何变化**（`KEY-DEF-07` 关闭）；
  - [ ] 按住 `Shift` 250ms 以上且期间无按键，松开时产出 `LongPress`（供 `KEY-P2.02.02` 使用）；
  - [ ] 除被追踪的修饰键外，**任何** release 仍返回 `Ignored`（`TC-RT-08` 不回归）；
  - [ ] `Shift+Space` 只产生 `ToggleFullWidth`，不产生 `ToggleLang`；
  - [ ] 全部测试不依赖真实时钟：`at_ms` 由调用方传入（`engine.rs:353` 的 `press`/`release` 辅助函数已按此模式提供 `time_ms`）。

---

### 任务 ID：KEY-P0.01.04 序列按键与 Leader Key 状态机

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-18`、`CMD-28`
  - 优先级与复杂度：`P0 | 中 | 预估工时: 2.5 人天`
  - 前置依赖：`KEY-P0.01.03`
  - 关键路径：`CP: 否`
  - 并行通道：`Track A 按键总线与焦点引擎`
  - 代码落地锚点：`crates/ime-fcitx5/src/engine/sequence.rs`（新建）、`crates/ime-fcitx5/src/engine/context.rs`
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  技能要求的"序列按键（Chords）与 Leader Key 状态机"。`translate_key` 是纯函数，一次只看一个事件，无法表达 `Ctrl+K` 之后等第二段。

  **本卡只建状态机，不接入任何业务序列**——Phase 1 的键位表里没有多段序列，Phase 2 的命令面板（`KEY-P1.02.08`）与速查面板（`KEY-P2.02.02`）是第一批使用者。这样切分是为了让 `KEY-P0.01.04` 成为可独立验证的纯逻辑单元。

  | 状态 | 事件 | 次态 | 消费结果 |
  |---|---|---|---|
  | `Idle` | 非前缀键 | `Idle` | 交上下文树决定 |
  | `Idle` | 已注册前缀键（如 `Ctrl+Shift+/`） | `Pending{prefix, deadline}` | `ChainPending` |
  | `Pending` | 第二段匹配 | `Idle` | `Consumed`（执行序列动作） |
  | `Pending` | 第二段不匹配 | `Idle` | `Ignored`（**整条序列作废，第二段交还宿主**） |
  | `Pending` | `Escape` | `Idle` | `Consumed`（取消序列） |
  | `Pending` | 超过 `SEQUENCE_TIMEOUT_MS` | `Idle` | — （超时在**下一个事件到达时**判定，不使用定时器） |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  ```rust
  /// How long a half-typed sequence stays open.
  ///
  /// Checked when the next key arrives rather than by a timer: the UI thread's
  /// `poll(2)` loop has no timer of its own and the architecture forbids adding
  /// one, so a sequence that nobody continues simply fails to match and the
  /// pending state is discarded on the next event.
  pub const SEQUENCE_TIMEOUT_MS: u32 = 1_500;

  /// The prefix half of a sequence, matched against the routing table.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct SequencePrefix {
      /// XKB keysym of the first stroke.
      pub sym: u32,
      /// The modifier mask the first stroke must carry exactly.
      pub state: u32,
  }

  /// A sequence in flight.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  struct Pending {
      /// The prefix that opened the sequence.
      prefix: SequencePrefix,
      /// The host timestamp of the prefix stroke.
      started_at_ms: u32,
  }

  /// The chord state machine.
  #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
  pub struct KeySequence {
      /// The sequence in flight, if any.
      pending: Option<Pending>,
  }

  impl KeySequence {
      /// Offers one key to the machine and answers what it decided.
      ///
      /// A prefix answers `ChainPending`; the key is ours from that moment, so
      /// the caller must not hand it back even though nothing has happened yet.
      pub fn offer(&mut self, event: &KeyEvent, prefixes: &[SequencePrefix]) -> SequenceDecision { /* ... */ }
  }

  /// What the sequence machine decided about one key.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum SequenceDecision {
      /// No sequence is involved; the context tree decides.
      Pass,
      /// The key opened a sequence.
      Opened,
      /// The key completed a sequence.
      Completed,
      /// The key cancelled a sequence.
      Cancelled,
  }
  ```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 写 `sequence.rs` 的 `SequencePrefix` / `KeySequence` / `SequenceDecision` 与 `offer`。
  2. 在 `Dispatcher` 里持有 `KeySequence`，在上下文树**之前**调用 `offer`（序列优先级高于除浮层外的所有上下文）。
  3. 把 `Consumed::ChainPending` 从 `arbiter::arbitrate` 一路透传到 `on_key_event` 的返回值（`ChainPending` → `true`，因为键已经被我们接下了）。
  4. 写测试：前缀 → 完成、前缀 → 不匹配（第二段交还）、前缀 → Escape（取消）、前缀 → 超时（下一事件作废）。
  5. 写一条"空前缀表时状态机恒返回 `Pass`"的测试，证明本卡不改变任何既有行为。
- **验收标准 (DoD)**：
  - [ ] 前缀表为空时，`KeySequence::offer` 对任何事件返回 `Pass`，且 `Dispatcher` 的行为与引入本卡前逐键一致；
  - [ ] 序列作废时第二段键**必须**交还宿主（有测试断言）；
  - [ ] 状态机不引入任何定时器、不读时钟（`at_ms` 由事件携带）；
  - [ ] `Consumed::ChainPending` 在 `on_key_event` 中映射为 `true`，且有测试固定该映射。

---

### 任务 ID：KEY-P0.01.05 `Config` → `KeyBindings` 投影与热重载

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-04`、`KEY-DEF-06`、`KEY-DEF-21`、`CMD-17`
  - 优先级与复杂度：`P0 | 中 | 预估工时: 3.0 人天`
  - 前置依赖：`KEY-P0.01.02`
  - 关键路径：`CP: 是`
  - 并行通道：`Track B 命令接入与键位矩阵`
  - 代码落地锚点：`crates/ime-fcitx5/src/engine.rs`（`KeyBindings` 与其投影）、`crates/ime-fcitx5/src/addon.rs`（`INIT_STEPS` 与热重载订阅）、`crates/ime-fcitx5/src/config_bridge.rs`（新建）
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  关闭 `KEY-DEF-04`（配置到不了路由表）与 `KEY-DEF-21`（无构建步骤、无热重载）。本卡冻结 `KeyBindings` 作为 Track B 六张路由表卡片的共享接口。

  | `[keys]` 配置键 | `KeyBindings` 字段 | 投影规则 |
  |---|---|---|
  | `digit_zero` | `digit_zero: DigitZero` | 一一映射（`Passthrough` / `Flip`） |
  | `enter_commit_raw` | `enter_commit_raw: bool` | 直接复制 |
  | `flip_keys: Vec<KeyName>` | `flip_keys: FlipSet` | **从固定四字段改为位标志集合**，覆盖白名单全部 6 个可翻页键（`minus`/`equal`/`up`/`down`/`page_up`/`page_down`），关闭 `KEY-DEF-05` 的一半 |
  | `highlight_keys: Vec<KeyName>` | `highlight_keys: HighlightSet` | **新增字段**，覆盖 `tab`/`shift_tab`/`up`/`down`/`left`/`right`，关闭 `KEY-DEF-03` |
  | `ui.client_preedit` | （不进入 `KeyBindings`，由 `apply_effects` 读取） | — |

  热重载语义（与 `SessionEvent::ConfigReloaded` 一致，features.md 0.4 规则 10 禁止重置进行中的会话）：

  | 重载前后 | 行为 |
  |---|---|
  | 键位改变、无活跃 composition | 下一个按键即按新键位路由 |
  | 键位改变、**有活跃 composition** | 输入与候选**完全保留**；新键位对**后续**按键生效；`highlight_keys` 的变更不移动当前高亮 |
  | 配置非法 | `ime-config` 的 `Config::repaired` 已替换为默认并产出 `config/invalid` 诊断；引擎只接收修好的值 |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **① 投影**（`crates/ime-fcitx5/src/config_bridge.rs`）：

  ```rust
  /// Projects the `[keys]` section onto the host layer's routing table.
  ///
  /// The configuration crate owns the file format, the key-name whitelist and
  /// the validation; this function is the one place those values become the
  /// router's own type. A name the whitelist accepts but the router cannot route
  /// is a defect rather than a user error, so it is reported as
  /// `keys/unroutable-binding` instead of being dropped silently -- that silence
  /// is what `KEY-DEF-05` was.
  pub fn project_keys(keys: &ime_config::KeysConfig) -> (KeyBindings, Vec<ImeError>) { /* ... */ }
  ```

  **② 位标志集合**（`engine.rs`，替换 `FlipKeys` 与新增 `HighlightSet`）：

  ```rust
  bitflags::bitflags! {
      /// The keys that page the candidate list (`[keys] flip_keys`).
      ///
      /// A flag set rather than the four-field struct this used to be: the
      /// configuration's whitelist offers six pageable keys and the struct could
      /// only carry four, so two of them were accepted and then silently
      /// dropped.
      #[derive(Clone, Copy, Debug, PartialEq, Eq)]
      pub struct FlipSet: u8 {
          /// `-`.
          const MINUS = 1 << 0;
          /// `=`.
          const EQUAL = 1 << 1;
          /// `Up`.
          const UP = 1 << 2;
          /// `Down`.
          const DOWN = 1 << 3;
          /// `Page_Up`.
          const PAGE_UP = 1 << 4;
          /// `Page_Down`.
          const PAGE_DOWN = 1 << 5;
      }

      /// The keys that move the candidate highlight (`[keys] highlight_keys`).
      #[derive(Clone, Copy, Debug, PartialEq, Eq)]
      pub struct HighlightSet: u8 {
          /// `Tab`.
          const TAB = 1 << 0;
          /// `Shift+Tab`.
          const SHIFT_TAB = 1 << 1;
          /// `Up`.
          const UP = 1 << 2;
          /// `Down`.
          const DOWN = 1 << 3;
          /// `Left`.
          const LEFT = 1 << 4;
          /// `Right`.
          const RIGHT = 1 << 5;
      }
  }
  ```

  **③ 热重载订阅**（`crates/ime-fcitx5/src/addon.rs`）：

  `INIT_STEPS` 增加两步：

  ```rust
  InitStep::new("key-bindings", false, init_key_bindings),
  // ... after `config`
  InitStep::new("config-watch", false, start_config_watch),
  ```

  `init_key_bindings` 调 `config_bridge::project_keys` 并把结果存进 `ArcSwap<KeyBindings>`（无锁读，宿主线程每次按键只做一次 `load()`）；`start_config_watch` 订阅 `ime_config::reload::ConfigWatcher`，回调里重新投影并 `store`。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 写 `config_bridge.rs` 的 `project_keys`，含 `keys/unroutable-binding` 诊断。
  2. 把 `engine.rs` 的 `FlipKeys` 换成 `FlipSet` 位标志，新增 `HighlightSet`，同步修改 `KeyBindings`、`Default`、以及全部引用点。
  3. 在 `addon.rs` 的 `INIT_STEPS` 插入 `key-bindings`（依赖 `config` 步骤之后）与 `config-watch` 两步，两者均 `is_fatal = false`。
  4. 引入 `ArcSwap<KeyBindings>`（新依赖，需在 `[workspace.dependencies]` 登记并说明用途；若审查不通过，退化为 `Mutex<KeyBindings>` 并在宿主线程只做一次 `try_lock` 失败即用旧值）。
  5. 写测试：投影的双向一致性（`KeyName` ↔ 位标志）、非法键名的诊断、热重载不打断进行中的 composition（复用 `transitions.rs:310-321` 的 `on_config_reloaded` 测试模式）。
- **验收标准 (DoD)**：
  - [ ] `keys.highlight_keys = ["shift_tab"]` 后，`Tab` 不再移动高亮、`Shift+Tab` 移动——配置生效（`KEY-DEF-03` 关闭）；
  - [ ] `keys.flip_keys = ["page_up", "page_down"]` 后，`Page_Up`/`Page_Down` 翻页（`KEY-DEF-05` 关闭）；
  - [ ] 白名单中的 10 个键名**全部**能被投影成至少一个可路由的键位，超出的键名产出 `keys/unroutable-binding`；
  - [ ] 热重载期间进行中的 composition 的 `raw`、候选列表与高亮**逐字段不变**（`AGENTS.md` 禁止事项 23）；
  - [ ] `INIT_STEPS` 的步骤名表在 `addon.rs:502-517` 的测试中同步更新，且新步骤均非 fatal。

---

## 7. 续写指令

本主文档已全量承载《系统设计假设清单》《按键与焦点问题总清单》《全量命令-键位映射矩阵》《WBS 任务覆盖追溯表》《关键路径与并行通道汇总》与 **P0 全部 5 张任务卡**。

P1 与 P2 的详细任务卡按「主干索引 + 领域分片（Hub & Spoke）」拆分为两份分片文档，避免单次输出截断：

| 分片 | 路径 | 承载内容 |
|---|---|---|
| Phase 2 分片 | `./docs/dev/opt-keymap/phase-2.md` | P1 全部 13 张卡：`KEY-P1.02.01`~`KEY-P1.02.08`、`KEY-P1.03.01`~`KEY-P1.03.05` |
| Phase 3 分片 | `./docs/dev/opt-keymap/phase-3.md` | P2 全部 8 张卡：`KEY-P2.01.01`~`KEY-P2.01.03`、`KEY-P2.02.01`~`KEY-P2.02.03`、`KEY-P2.03.01`~`KEY-P2.03.02` |

**续写输入** = 本主文档 + 目标分片路径。
**续写模板** = 本文档第 6 节的「标准化原子任务卡」字段全集：
`任务 ID` / `基本属性`（绑定来源编号、优先级与复杂度、前置依赖、关键路径、并行通道、代码落地锚点、当前状态）/ `交互目标与按键映射矩阵` / `工程实现方案与代码级细节 (Diff / Implementation)` / `逐步落地实施步骤 (Implementation Steps)` / `验收标准 (DoD)`。

**续写约束**：
1. 分片头部必须包含 Living Header 并回链本主文档（`> 上级文档: ./docs/dev/opt-keymap.md`）。
2. 每张卡的「绑定来源编号」必须能在第 5.1 节追溯表中找到，且与主文档的编号一一对应；**不得新增或重编号**。
3. 分片不得复制主文档的表格，只引用。
4. 严禁「等等」「略」「依此类推」；每个字段必须写实。

**与 `docs/dev/features.md` 的同步义务**（`AGENTS.md` 第 7 节）：
本方案与既有任务卡的对应关系如下，落地时必须在 `features.md` 中同步卡片状态、5.1 的 WBS 追溯行与 0.7 的任务总览行：

| 本方案任务卡 | 对应的既有 `features.md` 任务卡 | 关系 |
|---|---|---|
| `KEY-P0.01.01`、`KEY-P0.01.02`、`KEY-P0.01.04`、`KEY-P1.02.01`~`KEY-P1.02.04`、`KEY-P1.02.07`、`KEY-P1.02.08` | `TASK-1.04.04`（按键事件路由与 Fcitx5 状态机协作，当前 `PARTIAL`） | 本方案是 `TASK-1.04.04` 的**缺陷修复分解**，关闭该卡的 DoD#1/#2/#4 后即可标记完成 |
| `KEY-P1.02.05`、`KEY-P1.02.06` | `TASK-1.04.04` 的步骤 3/4（`apply_effects`、`HostCtx`） | 同一张卡的实施步骤，独立成卡以便并行 |
| `KEY-P0.01.05` | `TASK-1.03.06`（配置模型，当前 `READY_FOR_FINAL_GATE`）的下游消费方 | 本卡是消费方，不改 `ime-config` 的契约 |
| `KEY-P1.03.01`、`KEY-P1.03.02` | `TASK-1.03.06` 的 `keys.*` 校验规则 | 本方案**扩展**该校验（跨列表冲突），需在 `features.md` 的 `TASK-1.03.06` 卡片中补一行 |
| `KEY-P1.03.03` | `TASK-1.05.05`（候选网格、数字快捷键标签与首选项高亮，当前 `PENDING`） | 直接对应 |
| `KEY-P1.03.04` | `TASK-1.05.06`（鼠标交互，当前 `PENDING`） | 直接对应；本卡额外要求与键盘路径同源 |
| `KEY-P2.01.01`、`KEY-P2.01.02`、`KEY-P2.01.03` | 无既有卡片 | **新增**。features.md 0.4 规则 10 与 3.6 的"焦点丢失"行有要求但无任务卡，需补建 |
| `KEY-P2.02.01`~`KEY-P2.02.03` | `TASK-2.03.03`（命令面板，P1） | 本方案把速查面板与命令面板合并为同一浮层上下文；`KEY-P2.02.01` 需 ADR 扩展 `UiFrame` |
| `KEY-P2.03.01` | 无既有卡片 | **新增**。用户自定义键位的回写当前无归属 |
| `KEY-P2.03.02` | `TC-RT-06`~`TC-RT-10`（`docs/dev/tests.md`） | 本卡是这些用例的落地执行卡 |

**待建 ADR**（`ASM-08`，两处契约变更，均须在对应卡片开工前完成）：

| ADR | 触发卡 | 内容 |
|---|---|---|
| `ADR-0004` | `KEY-P2.02.01` | `UiFrame` 增加速查面板的承载方式（新增 `overlay: Option<OverlayFrame>` 字段，或新增一个 `UiCommand::Overlay` 通道）。**倾向后者**：`UiFrame` 是候选框的完整快照，把浮层塞进去会让每一帧都背着它 |
| `ADR-0005` | 仅在 `ASM-08` 降级路径被否决时 | `KeyAction` 新增 `HoldModifier(ModifierKey, bool)` 变体。**本方案默认不采用**——`KEY-P0.01.03` 已给出不触碰契约的实现 |
