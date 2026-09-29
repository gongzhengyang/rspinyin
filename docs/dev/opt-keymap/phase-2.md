# rspinyin 键盘优先交互体系 · Phase 2 分片（P1 任务卡）

> 上级文档: [`./opt-keymap.md`](./opt-keymap.md)（必读：假设清单、问题总清单、命令键位矩阵、追溯表、关键路径）
> 文档版本: v1.0 ｜ 系统形态: `Desktop GUI`（无焦点覆盖层变体） ｜ 架构基线: Fcitx5 5.1.7 进程内 addon ｜ 最后同步 Commit: `ee0dbfb` ｜ 维护约定: 代码演进后必须回写上级文档的问题清单状态与键位矩阵
> 承载范围: `KEY-P1.02.01`~`KEY-P1.02.08`、`KEY-P1.03.01`~`KEY-P1.03.05`（共 13 张）

**本分片不复制上级文档的任何表格。** 全部编号、来源编号与依赖关系以上级文档第 5 节为准，本分片只做原子级展开。

**P1 定义**：**全局核心命令与序列按键接入**。P0 让插件第一次能正确地消费按键；P1 让 features.md 3.5 的 19 行快捷键表**逐行成立**，并让"鼠标只是补充"这句规格（features.md 3.5）成为事实。

---

### 任务 ID：KEY-P1.02.01 路由表补齐：`highlight_keys` 生效

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-03`、`KEY-DEF-23`、`CMD-11`、`CMD-12`
  - 优先级与复杂度：`P1 | 中 | 预估工时: 1.5 人天`
  - 前置依赖：`KEY-P0.01.05`
  - 关键路径：`CP: 否`
  - 并行通道：`Track B 命令接入与键位矩阵`
  - 代码落地锚点：`crates/ime-fcitx5/src/engine.rs`（`shift_tolerant_action` 与 `bare_action` 的重构）
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  | 配置 | `Tab` | `Shift+Tab` | `Up` | `Down` | `Left` | `Right` |
  |---|---|---|---|---|---|---|
  | 默认 `["tab", "shift_tab"]` | `MoveHighlight(+1)` | `MoveHighlight(-1)` | `PagePrev` | `PageNext` | `MoveCaret(-1)` | `MoveCaret(+1)` |
  | `["up", "down"]` | 交还宿主 | 交还宿主 | `MoveHighlight(+1)` | `MoveHighlight(-1)` | `MoveCaret(-1)` | `MoveCaret(+1)` |
  | `["left", "right"]` | 交还宿主 | 交还宿主 | `PagePrev` | `PageNext` | `MoveHighlight(-1)` | `MoveHighlight(+1)` |
  | `[]` | 交还宿主 | 交还宿主 | `PagePrev` | `PageNext` | `MoveCaret(-1)` | `MoveCaret(+1)` |

  **冲突消解规则**（`flip_keys` 与 `highlight_keys` 命中同一键时）：**`highlight_keys` 优先**。理由：高亮移动是组字过程中的高频微操作，翻页是低频操作；且 `KEY-P1.03.02` 会在配置加载期就把这种重叠报为 `config/invalid`，运行期的优先级只是兜底。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  当前结构（`engine.rs:284-296`、`:300-317`）把"容忍 Shift 的行"与"要求裸按的行"分成两个函数，导致 `Tab` 只能在第一个函数里、`Up`/`Down` 只能在第二个函数里，两者无法统一。改为**单表 + 谓词**：

  ```rust
  /// One row of the routing table.
  ///
  /// A row is a keysym plus the predicate that decides which modifier sets it
  /// accepts, so a key can appear in both the shift-tolerant group and the bare
  /// group without being written twice. The previous split into two functions is
  /// what made `highlight_keys` impossible to honour: `Tab` lived in one and the
  /// arrows in the other, and neither could see the configuration.
  struct Row {
      /// XKB keysym this row matches.
      sym: u32,
      /// Which modifier sets the row accepts.
      accepts: Accepts,
      /// What the row means, given the bindings.
      action: fn(&KeyBindings) -> Option<KeyAction>,
  }

  /// The modifier sets a row tolerates.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  enum Accepts {
      /// Only the bare press; any modifier makes it somebody else's key.
      BareOnly,
      /// A bare press, or one with Shift held (Shift is how the uppercase form
      /// of a letter is typed, and how `Shift+Tab` reverses a direction).
      BareOrShift,
  }
  ```

  行表的求值顺序固定为：`highlight_keys` 命中的行 → `flip_keys` 命中的行 → `digit_zero` → `enter_commit_raw` → 固定行（`Left`/`Right`/`Escape`/`BackSpace`）。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 定义 `Row` 与 `Accepts`，把 `shift_tolerant_action` 与 `bare_action` 的 15 行合并成一张 `const ROWS: &[Row]`。
  2. 在行表求值前插入 `highlight_keys` 与 `flip_keys` 两次查表（两者都是位标志集合，命中即返回）。
  3. 保留 `translate_key` 的公开签名不变（`TC-RT-06` 依赖它）。
  4. 写表驱动测试：上表 4 种配置 × 6 个键 = 24 个断言。
  5. 写一条"`highlight_keys` 与 `flip_keys` 同时命中时 `highlight_keys` 胜出"的测试。
- **验收标准 (DoD)**：
  - [ ] 上表 4 种配置下的 24 个键位行为逐项通过；
  - [ ] `highlight_keys = []` 时 `Tab`/`Shift+Tab` 交还宿主，且 `Up`/`Down` 仍按 `flip_keys` 翻页；
  - [ ] 默认配置下 `translate_key` 的输出与改动前**逐键一致**（用 `engine.rs:373` 的 `assert_rows` 对拍全表）；
  - [ ] `engine.rs:625` 的 `test_translate_key_maps_the_global_mode_chords` 与 `:572` 的 `test_translate_key_moves_the_highlight_and_the_caret` 更新后仍全绿。

---

### 任务 ID：KEY-P1.02.02 路由表补齐：翻页键全集与音节分隔符

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-05`、`CMD-02`、`CMD-07`、`CMD-08`、`CMD-09`、`CMD-10`、`CMD-13`、`CMD-14`
  - 优先级与复杂度：`P1 | 中 | 预估工时: 1.5 人天`
  - 前置依赖：`KEY-P0.01.05`
  - 关键路径：`CP: 否`
  - 并行通道：`Track B 命令接入与键位矩阵`
  - 代码落地锚点：`crates/ime-fcitx5/src/engine.rs`（keysym 常量区与行表）
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  | 键 | keysym | 默认行为 | 可配置 | 本次改动 |
  |---|---|---|---|---|
  | `Page_Up` | `0xff55` | — | `flip_keys` | **新增行**（白名单已接受但无行） |
  | `Page_Down` | `0xff56` | — | `flip_keys` | **新增行** |
  | `'`（单引号） | `0x0027` | 输入音节分隔符 | 否 | **新增行**（`is_input_char` 已接受它，路由表却无行） |
  | `0` | `0x0030` | 直通 / 翻页 | `digit_zero` | 已有 |
  | `-` / `=` | `0x002d` / `0x003d` | 上一页 / 下一页 | `flip_keys` | 已有 |
  | `Up` / `Down` | `0xff52` / `0xff54` | 上一页 / 下一页 | `flip_keys` | 已有 |
  | `Left` / `Right` | `0xff51` / `0xff53` | preedit 光标左移 / 右移 | 否（Phase 1） | 已有，无改动 |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  新增 keysym 常量（`engine.rs` 的常量区，沿用现有的注释格式）：

  ```rust
  /// `FcitxKey_apostrophe`, the syllable separator the input buffer accepts.
  const KEY_APOSTROPHE: u32 = 0x0027;
  /// `FcitxKey_Page_Up`, the first of the two pageable keys the whitelist names
  /// but the routing table had no row for.
  const KEY_PAGE_UP: u32 = 0xff55;
  /// `FcitxKey_Page_Down`.
  const KEY_PAGE_DOWN: u32 = 0xff56;
  ```

  `'` 的行必须走 `KeyAction::InputChar('\'')`，且**只在组字上下文有效**（`KEY-P0.01.01` 的 L1）。Idle 下按 `'` 必须交还宿主——`on_key_idle` 会把 `InputChar` 送进 `start_composing`，而 `is_input_char`（`transitions.rs:431-433`）接受 `'`，这会让用户在 Idle 下按单引号就打开候选框。因此该行的 `action` 闭包需要读会话状态，或由 `arbitrate`（`KEY-P0.01.02`）在 Idle 下判为 `Inert`。**采用后者**：`executability` 对 `InputChar('\'')` 在 Idle 返回 `Executable`（它确实会开启组字），所以正确的修法是**在 `on_key_idle` 中把 `InputChar('\'')` 与字母分开处理**——单引号在 Idle 下不开启组字。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 新增三个 keysym 常量。
  2. 在行表加入 `Page_Up`/`Page_Down` 两行，绑定 `FlipSet::PAGE_UP` / `FlipSet::PAGE_DOWN`。
  3. 在行表加入 `'` 行，`action` 为 `|_| Some(KeyAction::InputChar('\''))`。
  4. 在 `crates/ime-core/src/state/transitions.rs` 的 `on_key_idle` 中，把 `KeyAction::InputChar(ch)` 分支改为"仅 `ch.is_ascii_alphabetic()` 时 `start_composing`，否则交还"。
  5. 写测试：`Page_Up`/`Page_Down` 在两种配置下的行为；`'` 在 Composing 下进入缓冲区、在 Idle 下交还宿主。
- **验收标准 (DoD)**：
  - [ ] `flip_keys = ["page_up", "page_down"]` 后两个键翻页（`KEY-DEF-05` 完全关闭）；
  - [ ] `flip_keys` 包含全部 6 个白名单键名时，6 个键**全部**可翻页；
  - [ ] Composing 下按 `'` 后 `InputBuffer::raw()` 尾部出现 `'`，且 preedit 的 `SpanKind::Separator` 正确分段；
  - [ ] Idle 下按 `'` 交还宿主，应用收到单引号字符，候选框不出现；
  - [ ] `engine.rs:391-410` 的 `is_routed` 集合更新为包含 `KEY_APOSTROPHE`/`KEY_PAGE_UP`/`KEY_PAGE_DOWN`，`:675` 的"绝不吞键"扫描仍通过。

---

### 任务 ID：KEY-P1.02.03 大写字母形态归一

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-08`、`CMD-01`
  - 优先级与复杂度：`P1 | 中 | 预估工时: 1.0 人天`
  - 前置依赖：`KEY-P0.01.05`
  - 关键路径：`CP: 否`
  - 并行通道：`Track B 命令接入与键位矩阵`
  - 代码落地锚点：`crates/ime-fcitx5/src/engine.rs`（字母行）
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  关闭 `KEY-DEF-08`：路由行为不得取决于宿主是否把大小写折进 keysym。

  | 事件 | 当前行为 | 目标行为 |
  |---|---|---|
  | `sym=0x61`（`a`），无修饰 | `InputChar('a')` | 不变 |
  | `sym=0x61`，`Shift` | `InputChar('a')` | 不变 |
  | `sym=0x41`（`A`），`Shift` | `Ignore`（落到宿主） | `InputChar('a')` |
  | `sym=0x41`，无修饰 | `Ignore` | `Ignore`（无修饰的大写 keysym 是异常，交还宿主） |
  | `sym=0x45`（`E`），`Ctrl+Shift` | `EnterTempEnglish` | 不变 |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  归一化发生在**行表匹配之前**，作为一次纯函数折叠：

  ```rust
  /// Folds a keysym that a host may have case-folded back to its lowercase form.
  ///
  /// Fcitx5 folds the case into the symbol when Shift is held, so `Shift+a` can
  /// arrive as either `0x61` or `0x41` depending on the frontend. The routing
  /// table must not see the difference: a key that means "type the letter a" is
  /// the same key whichever shape it arrives in. The fold is applied only when
  /// Shift is actually held, so a bare uppercase symbol -- which no keyboard
  /// produces -- still falls through to the host.
  fn fold_shifted_letter(sym: u32, state: u32) -> u32 {
      if (state & SHIFT) != 0 && (KEY_A_UPPER..=KEY_Z_UPPER).contains(&sym) {
          sym + (KEY_A - KEY_A_UPPER)
      } else {
          sym
      }
  }
  ```

  新增常量：

  ```rust
  /// `FcitxKey_A`, the uppercase shape a chord containing Shift may deliver.
  const KEY_A_UPPER: u32 = 0x0041;
  /// `FcitxKey_Z`, the high end of the uppercase shape.
  const KEY_Z_UPPER: u32 = 0x005a;
  ```

  折叠在 `Dispatcher::dispatch` 中调用一次，`translate_key` 内部不再需要 `KEY_E_UPPER` 的特判（`engine.rs:274`），该行可简化为只看 `KEY_E`。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 新增 `KEY_A_UPPER` / `KEY_Z_UPPER` 常量。
  2. 写 `fold_shifted_letter`，在 `Dispatcher::dispatch` 的 release 分支之后、上下文树之前调用。
  3. 简化 `engine.rs:274` 的 `Ctrl+Shift+E` 行（去掉 `KEY_E_UPPER`），因为折叠后它不可能再以大写到达。
  4. 更新 `engine.rs:653` 的 `(press(KEY_E_UPPER, SHIFT), KeyAction::Ignore)` 断言为 `InputChar('e')`，并在注释中写明这条改动的原因。
  5. 写测试：26 个字母 × 2 种形态 × 有无 Shift = 104 个断言，全部等价。
- **验收标准 (DoD)**：
  - [ ] 26 个字母在 `sym=小写` 与 `sym=大写+Shift` 两种到达形态下产生**完全相同**的 `KeyAction`；
  - [ ] 无修饰的大写 keysym（如 `sym=0x41, state=0`）交还宿主；
  - [ ] `Ctrl+Shift+E` 仍产生 `EnterTempEnglish`（两种形态各一条用例）；
  - [ ] `fold_shifted_letter` 是纯函数，有独立的边界测试（`0x40`、`0x5b` 不在范围内）。

---

### 任务 ID：KEY-P1.02.04 修饰键自身行与和弦消歧

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-07`、`KEY-DEF-22`、`CMD-04`、`CMD-05`、`CMD-16`、`CMD-18`、`CMD-19`、`CMD-20`
  - 优先级与复杂度：`P1 | 高 | 预估工时: 2.5 人天`
  - 前置依赖：`KEY-P0.01.03`、`KEY-P0.01.05`
  - 关键路径：`CP: 否`
  - 并行通道：`Track B 命令接入与键位矩阵`
  - 代码落地锚点：`crates/ime-fcitx5/src/engine.rs`（`chord_action` 的重构）
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  关闭 `KEY-DEF-07`（修饰键自身行抢先）与 `KEY-DEF-22`（裸空格在任何上下文都返回 `CommitHighlighted`）。

  **和弦判定的唯一规则**：`state & MODIFIER_MASK` 必须与和弦声明的掩码**全等**。

  | 键 | `state & MODIFIER_MASK` | 动作 | 当前 | 目标 |
  |---|---|---|---|---|
  | `Shift_L` / `Shift_R` | 任意 | — | `ToggleLang` | **删除此行**，交由 `ModifierHold`（`KEY-P0.01.03`） |
  | `Space` | `0` | `CommitHighlighted` | 已有 | **仅在 L1 组字上下文**（由 `KEY-P0.01.01` 保证） |
  | `Space` | `CTRL` | `ToggleLang` | 已有 | 不变 |
  | `Space` | `SHIFT` | `ToggleFullWidth` | 已有（但被 `Shift` 自身行污染） | 不变（`Shift` 自身行删除后即正确） |
  | `Space` | `CTRL\|SHIFT` | `Ignore` | 已有 | 不变 |
  | `Space` | `ALT` | `Ignore` | 已有 | 不变 |
  | `period` | `CTRL` | `TogglePunct` | 已有 | 不变 |
  | `period` | `CTRL\|SHIFT` | `Ignore` | 已有 | 不变 |
  | `E` | `CTRL\|SHIFT` | `EnterTempEnglish` | 已有 | 不变（形态归一后只需匹配 `KEY_E`） |
  | 任意键 | 含 `SUPER`/`HYPER`/`META`/`SUPER2` | `Ignore` | 已有 | 不变（宿主保留给桌面环境） |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  把 `chord_action` 的逐条 `if` 改为**声明式和弦表**，让"多一条和弦"不再需要改控制流（同时为 `KEY-P1.02.08` 留出入口）：

  ```rust
  /// One global mode chord: a key plus the exact modifier set it requires.
  struct Chord {
      /// XKB keysym of the chord's key.
      sym: u32,
      /// The modifier set, compared for equality against `state & MODIFIER_MASK`.
      ///
      /// Equality rather than a subset test: `Ctrl+Space` and `Ctrl+Shift+Space`
      /// are different keys, and a chord that accepted a superset would take
      /// keys the desktop environment owns.
      mask: u32,
      /// What the chord means.
      action: KeyAction,
  }

  /// The global mode chords, in match order.
  ///
  /// `Shift_L` and `Shift_R` are deliberately absent: a modifier's own press is
  /// not a chord, and treating it as one made every capital letter toggle the
  /// input mode. The hold semantics that replaced it live in
  /// [`crate::engine::modifier`].
  const CHORDS: &[Chord] = &[
      Chord { sym: KEY_SPACE, mask: CTRL, action: KeyAction::ToggleLang },
      Chord { sym: KEY_SPACE, mask: SHIFT, action: KeyAction::ToggleFullWidth },
      Chord { sym: KEY_PERIOD, mask: CTRL, action: KeyAction::TogglePunct },
      Chord { sym: KEY_E, mask: CTRL | SHIFT, action: KeyAction::EnterTempEnglish },
  ];
  ```

  裸空格的处理：`Chord` 表只承载**带修饰键**的和弦，裸空格（`mask == 0`）留在 `bare_action` 中，由 `KEY-P0.01.01` 的 L1 上下文保证它只在组字时生效。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 定义 `Chord` 与 `CHORDS` 常量表。
  2. 删除 `chord_action` 的 `Shift_L`/`Shift_R` 分支。
  3. 把 `KEY_SPACE` 的 `0` 分支从 `chord_action` 移到 `bare_action`（它是裸按）。
  4. 把 `chord_action` 的其余分支改为遍历 `CHORDS`。
  5. 更新 `engine.rs:625` 的 `test_translate_key_maps_the_global_mode_chords`：`Shift` 的两行改为断言 `Ignored`；新增 `Shift+Space` 只产生 `ToggleFullWidth` 的用例。
  6. 写"和弦全等比较"的边界测试：每个和弦 ± 一个额外修饰键。
- **验收标准 (DoD)**：
  - [ ] 按住 `Shift` 打字**不产生任何模式切换**（`KEY-DEF-07` 关闭）；
  - [ ] `Shift+Space` 只产生 `ToggleFullWidth`，不产生 `ToggleLang`；
  - [ ] 每个和弦在"多按一个修饰键"时全部落 `Ignore`（`Ctrl+Shift+Space`、`Ctrl+Alt+Space`、`Ctrl+Shift+.` 等）；
  - [ ] `CHORDS` 表的每一条都有对应测试用例（表驱动，新增一条和弦必须同时新增一条用例）；
  - [ ] `translate_key` 对 `Shift_L`/`Shift_R` 的 press 返回 `Ignore`。

---

### 任务 ID：KEY-P1.02.05 `apply_effects`：Effect → 宿主调用

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-02`、`CMD-22`
  - 优先级与复杂度：`P1 | 高 | 预估工时: 4.0 人天`
  - 前置依赖：`KEY-P0.01.02`、`KEY-P0.01.05`
  - 关键路径：`CP: 是`
  - 并行通道：`Track B 命令接入与键位矩阵`
  - 代码落地锚点：`crates/ime-fcitx5/src/effects.rs`（新建）、`crates/ime-fcitx5/src/ffi/abi/engine.rs`（`on_key_event` 接线）、`crates/ime-fcitx5/src/ffi/cpp/engine_glue.cpp`（新增宿主调用导出）
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  关闭 `KEY-DEF-02`：让 `on_key_event` 第一次真正消费按键。本卡是**最早可交付里程碑**的最后一张（前两张是 `KEY-P0.01.01`、`KEY-P0.01.02`）。

  | `Effect` 变体（`machine.rs:228-256`） | 宿主动作 | 备注 |
  |---|---|---|
  | `UpdatePreedit(Preedit)` | `client_preedit == true` 时 `ic->setPreedit(text, caret)`；否则 `ic->clearPreedit()` | 策略读 `[ui] client_preedit` |
  | `SendFrame(Box<UiFrame>)` | `UiCommand::Frame` 投递到 UI 线程（latest-wins 单槽） | 不阻塞 |
  | `Show(AnchorHint)` | 解析 `Anchor` 后投 `UiCommand::Show{revision, anchor}`（有序队列） | `Anchor` 解析走 `crate::cursor` |
  | `Hide(HideReason)` | `UiCommand::Hide{revision, reason}`（有序队列） | |
  | `Commit(String)` | `ic->commitString(text)`；随后投 `Hide{reason: Committed}` | 文本**不得**进日志 |
  | `RecordUserFreq{key, weight_hint}` | `UserFreqSource::record`（受 `privacy_impl` 门禁） | 敏感上下文抑制 |
  | `Diagnose(ImeError)` | `tracing::warn!` + 探针计数 | 稳定错误码，无用户输入内容 |
  | `SetClientPreedit(Option<Preedit>)` | 见 `UpdatePreedit` | |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **① `HostCtx`**（持有宿主侧的一切）：

  ```rust
  /// Everything the effect executor needs from the host.
  ///
  /// A trait rather than a struct of raw pointers so that the executor is
  /// testable without Fcitx5: the real implementation lives behind
  /// `#[cfg(fcitx5_host)]` and forwards into the C++ glue, and the tests drive a
  /// recording double. Nothing here blocks: the host thread has 100us per
  /// callback and this runs inside one.
  pub trait HostCtx {
      /// Inserts text into the client at the caret.
      fn commit(&mut self, text: &str) -> Result<(), ImeError>;
      /// Sets or clears the client's preedit area.
      fn set_client_preedit(&mut self, preedit: Option<&Preedit>) -> Result<(), ImeError>;
      /// Reads whether the host's input method is enabled.
      fn is_enabled(&self) -> bool;
      /// Enables or disables the host's input method (`ToggleLang`).
      fn set_enabled(&mut self, enabled: bool) -> Result<(), ImeError>;
      /// Posts a command to the UI thread.
      fn post(&mut self, command: UiCommand);
  }
  ```

  **② 执行器**（`crates/ime-fcitx5/src/effects.rs`）：

  ```rust
  /// Applies the effects one step produced, in order.
  ///
  /// Order matters and is the step's: a commit followed by a hide must reach the
  /// UI thread in that order, and a preedit update followed by a commit must
  /// clear the preedit before the text lands. An effect that fails is recorded
  /// and the rest still run -- a commit the host refused must not stop the
  /// window from being hidden.
  pub fn apply_effects(
      effects: Effects,
      host: &mut dyn HostCtx,
      ui: &UiHandles<'_>,
      cfg: &Config,
  ) -> Result<(), ImeError> { /* ... */ }
  ```

  **③ `on_key_event` 接线**（`ffi/abi/engine.rs`，替换现有 Stub）：

  ```rust
  pub extern "C" fn on_key_event(
      context: *mut c_void,
      ic_id: u64,
      event: *const FcitxKeyEvent,
  ) -> bool {
      guard_ffi(false, || {
          if event.is_null() {
              emit_diagnostic("ffi/null-key-event");
              return false;
          }
          // SAFETY: non-null and owned by the caller for the duration of this
          // call; `FcitxKeyEvent` is `repr(C)` and every bit pattern is valid.
          let key = unsafe { *event };
          crate::session_host::handle_key(context, ic_id, &key)
      })
  }
  ```

  `handle_key` 在 `crates/ime-fcitx5/src/session_host.rs`（新建）中：取当前 `ic_id` 的会话 → 构造 `KeyEvent` → `Dispatcher::dispatch` → 若 `Consumed`/`ChainPending` 则 `step` + `apply_effects` → 返回 `bool`。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 写 `HostCtx` trait 与 `#[cfg(fcitx5_host)]` 下的真实实现（在 `ffi/cpp/engine_glue.cpp` 新增 `commit_string`/`set_preedit`/`set_enabled` 三个导出函数，签名沿用现有 vtable 的回调风格）。
  2. 写 `effects.rs` 的 `apply_effects`，逐条实现上表。
  3. 写 `session_host.rs` 的 `handle_key`，串起 `Dispatcher` → `Session::handle_key` → `apply_effects`。
  4. 替换 `ffi/abi/engine.rs:50` 的 Stub。
  5. 写 `RecordingHost` 测试替身（记录 `commit`/`set_client_preedit`/`post` 的调用序列），写表驱动测试：每个 `Effect` 变体一条。
  6. 写 `TC-RT-07` 的正式用例：200 个随机 keysym 走完整的 `handle_key`，断言返回 `true` 的那些至少产生一个非 `Diagnose` 的 `Effect`。
- **验收标准 (DoD)**：
  - [ ] 装上插件后敲 `nihao` 再按 `Space`，应用收到 `你好`（首次真正可用）；
  - [ ] Idle 态下敲空格、数字、回车、退格，应用**原样收到**这些字符（`KEY-DEF-01` 在真实宿主下验证）；
  - [ ] `TC-RT-07` 通过：`handle_key` 返回 `true` 的按键必有非 `Diagnose` 的 Effect；
  - [ ] `TC-RT-08` 通过：所有 `is_release == true` 的事件返回 `false`（`KEY-P0.01.03` 追踪的修饰键 release 除外，该例外有独立用例）；
  - [ ] `apply_effects` 不读文件、不取锁、不格式化日志字符串（`ASM-04` 的 100µs 预算）；
  - [ ] 日志中不出现任何用户输入内容（`AGENTS.md` 禁止事项 21）。

---

### 任务 ID：KEY-P1.02.06 会话生命周期接线

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-02`、`KEY-DEF-21`
  - 优先级与复杂度：`P1 | 中 | 预估工时: 2.5 人天`
  - 前置依赖：`KEY-P1.02.05`
  - 关键路径：`CP: 是`
  - 并行通道：`Track B 命令接入与键位矩阵`
  - 代码落地锚点：`crates/ime-fcitx5/src/ffi/abi/engine.rs`（`on_activate`/`on_deactivate`/`on_reset` 三个 Stub）、`crates/ime-fcitx5/src/session_host.rs`、`crates/ime-fcitx5/src/addon.rs`
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  `on_activate` / `on_deactivate` / `on_reset` 三个回调当前是空 Stub（`ffi/abi/engine.rs:23-43`），而状态机侧的 `SessionEvent::Reset` 已实现。本卡把会话的生命周期接到宿主事件上。

  | 宿主回调 | 会话事件 | 效果 |
  |---|---|---|
  | `activate(ic)` | 无（只登记 `ic_id`） | 建立该 `ic_id` 的会话槽；不改变任何状态 |
  | `deactivate(ic)` | `SessionEvent::Reset` | 丢弃 composition，不提交；投 `UiCommand::Hide{reason: Shutdown}` |
  | `reset(ic)` | `SessionEvent::Reset` | 同上 |
  | `focus_in(ic)` | 无 | 由 `KEY-P2.01.01` 处理 |
  | `focus_out(ic)` | 由 `KEY-P2.01.01` 处理 | 本卡只建立投递通道 |
  | 配置热重载 | `SessionEvent::ConfigReloaded(SessionConfig)` | 由 `KEY-P0.01.05` 的 watcher 投递 |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  ```rust
  /// The sessions this plugin holds, keyed by the host's input-context id.
  ///
  /// One entry per input context: v1 is single-user single-session (`ASM-12`),
  /// but Fcitx5 hands out a fresh `ic_id` per application window, and a session
  /// that outlived its context would keep a window on screen with no application
  /// behind it. The map is bounded by the host's context count and entries are
  /// dropped on deactivate.
  pub struct SessionHost {
      /// The live sessions.
      sessions: HashMap<u64, SessionSlot>,
      /// The bindings every session routes through (`KEY-P0.01.05`).
      bindings: ArcSwap<KeyBindings>,
      /// The session configuration every session reads.
      config: ArcSwap<SessionConfig>,
  }
  ```

  投递 `Reset` 时的顺序（与 `transitions.rs:39` 的注释一致）：先跑 `step` 拿到 `Effects`，再 `apply_effects`，最后从 map 中移除槽位。**`UiCommand::Shutdown` 由引擎自己投**，不是会话的关注点。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 写 `SessionHost` 与 `SessionSlot`，实现 `activate`/`deactivate`/`reset` 三个入口。
  2. 替换 `ffi/abi/engine.rs:23-43` 的三个 Stub，改为转发到 `SessionHost`。
  3. 在 `addon.rs` 的 `on_addon_destroy` 中，对每个存活会话投递 `Reset` 并清空 map（在 `stop_ui()` 之前）。
  4. 写测试：activate → 输入 → deactivate，断言 `commitString` 未被调用且投出了 `Hide`；reset 与 deactivate 行为一致。
  5. 写测试：两个不同 `ic_id` 的会话互不干扰（为 `ASM-12` 的多会话降级路径留验证）。
- **验收标准 (DoD)**：
  - [ ] 切换输入法（`Ctrl+Space` 关掉本插件）时，进行中的 composition 被丢弃且候选窗消失，**不提交任何候选**；
  - [ ] 同一 `ic_id` 的 `deactivate` 后再 `activate`，会话从干净状态开始；
  - [ ] 插件卸载时 `SessionHost` 为空，无残留会话（`ps -T` 无 `rspinyin-ui` 线程，`TC-RT-05` 不回归）；
  - [ ] `on_reset` 与 `on_deactivate` 产生完全相同的宿主侧调用序列。

---

### 任务 ID：KEY-P1.02.07 临时英文模式的键位语义修正

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-09`、`CMD-21`
  - 优先级与复杂度：`P1 | 中 | 预估工时: 1.5 人天`
  - 前置依赖：`KEY-P1.02.04`
  - 关键路径：`CP: 否`
  - 并行通道：`Track B 命令接入与键位矩阵`
  - 代码落地锚点：`crates/ime-core/src/state/transitions.rs`（`on_key_temp_english`）、`crates/ime-fcitx5/src/engine.rs`
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  关闭 `KEY-DEF-09`。features.md 3.5 的规格是「`Ctrl+Shift+E` 进入临时英文模式（所有键透传，直到 `Enter` 或 `Escape`）」。

  | 键 | 当前行为 | 目标行为 |
  |---|---|---|
  | `Return` | 退出模式 + 交还宿主 | 退出模式 + 交还宿主（不变） |
  | `Escape` | 退出模式 + 交还宿主 | 退出模式 + 交还宿主（不变） |
  | `Space` | **退出模式** + 交还宿主 | **留在模式内** + 交还宿主 |
  | `a`~`z` | 透传 | 透传（不变） |
  | `1`~`9` | 透传 | 透传（不变） |
  | `Ctrl+Shift+E` | 透传（已在模式内） | 透传（不变） |
  | `BackSpace` | 透传 | 透传（不变） |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  `on_key_temp_english`（`transitions.rs:67-75`）当前用 `CommitHighlighted | CommitRaw` 表达"回车"，但 `CommitHighlighted` 同时也是**空格**的动作（`engine.rs:267`）。两个不同的键共用一个 `KeyAction`，是这条缺陷的根因。

  修法**不新增 `KeyAction` 变体**（`ASM-08`）：把退出条件改为按 `KeyAction` 的**语义**判定，而不是按它当前被哪个键产生：

  ```rust
  /// Temporary English mode: every key reaches the application.
  ///
  /// `Return` and `Escape` are the two keys that leave the mode, and both are
  /// handed back to the application as well -- which is what "every key passes
  /// through" means. `Space` is deliberately not among them: it is translated to
  /// `CommitHighlighted` like the composing case, and leaving the mode on it
  /// would make a space bar press end a mode the user asked to stay in.
  ///
  /// The distinction is made on the event rather than on the action, because the
  /// two keys share an action: only the dispatcher knows which keysym produced
  /// it, and this handler is given the action alone.
  fn on_key_temp_english(&mut self, action: KeyAction, leaves: bool) {
      if leaves {
          self.temp_english = false;
      }
  }
  ```

  `leaves` 由调用方（`on_key`）传入，来源是 `SessionEvent::Key` 携带的**退出意图**。为把该意图从 dispatcher 传到状态机而不改 `KeyAction`，采用 `KEY-P0.01.03` 的同一手法：`SessionEvent::Key(KeyAction)` 保持不变，引擎在进入临时英文模式时**自行记录**该模式（`Dispatcher` 已有 `temp_english` 影子标志），并对模式内的 `Return`/`Escape` 直接调用 `Session::leave_temp_english()`（新增的公开方法），不经过 `KeyAction` 翻译。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在 `Session` 上新增 `pub fn leave_temp_english(&mut self)`，只清 `temp_english` 标志。
  2. 把 `on_key_temp_english` 的 `leaves` 判定从 `CommitHighlighted | CommitRaw` 收窄为 `CommitRaw`（`enter_commit_raw = true` 时的 `Return`）。
  3. 在 `Dispatcher` 的 L2 会话上下文中：当 `temp_english` 为真且键为 `Return`/`Escape` 时，调 `leave_temp_english` 并返回 `Ignored`（键交还宿主）。
  4. 更新 `table_tests.rs:425-455` 的临时英文测试组，把 `Space` 从退出用例中移除并新增"`Space` 不退出模式"的用例。
  5. 写测试：模式内按空格后 `temp_english` 仍为真，且后续字母仍透传。
- **验收标准 (DoD)**：
  - [ ] `Ctrl+Shift+E` 后按空格，模式**不退出**，应用收到空格字符；
  - [ ] `Ctrl+Shift+E` 后按 `Return`，模式退出且应用收到回车；
  - [ ] `Ctrl+Shift+E` 后按 `Escape`，模式退出且应用收到 Esc；
  - [ ] `enter_commit_raw = true` 时 `Return` 的退出行为与默认配置一致；
  - [ ] `transitions.rs` 的 `on_key_temp_english` 不再引用 `CommitHighlighted`。

---

### 任务 ID：KEY-P1.02.08 命令面板与诊断面板的键位预留

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-10`、`CMD-26`、`CMD-27`
  - 优先级与复杂度：`P1 | 中 | 预估工时: 2.0 人天`
  - 前置依赖：`KEY-P0.01.04`
  - 关键路径：`CP: 否`
  - 并行通道：`Track B 命令接入与键位矩阵`
  - 代码落地锚点：`crates/ime-fcitx5/src/engine/sequence.rs`、`crates/ime-fcitx5/src/engine/context.rs`
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  features.md 3.5 登记了两个 Phase 2 入口（`TASK-2.03.03` 命令面板、`TASK-2.08.01` 诊断面板），路由表里没有行。本卡**只做键位预留与上下文占位**，不实现面板本体（面板是 `KEY-P2.02.01` 的速查面板之外的另一条产品线）。

  | 键 | `state & MODIFIER_MASK` | 目标行为（Phase 1） | 目标行为（Phase 2） |
  |---|---|---|---|
  | `/` | `CTRL\|SHIFT` | `Consumed`，打开一个空的 `ModalOverlay` 上下文，投 `ui/not-implemented` 诊断 | 打开命令面板 |
  | `P` | `CTRL\|SHIFT` | 同上 | 打开诊断面板 |
  | 浮层打开时的 `Escape` | 任意 | 关闭浮层 | 不变 |
  | 浮层打开时的其他键 | 任意 | 交还宿主（浮层为空） | 面板内导航 |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  `KEY-DEF-10` 的根因是 `chord_action` 的写法没有为"多一条和弦"留结构。`KEY-P1.02.04` 已经把它改成 `CHORDS` 表，本卡只需追加两条：

  ```rust
  Chord { sym: KEY_SLASH, mask: CTRL | SHIFT, action: KeyAction::OpenOverlay(Overlay::CommandPalette) },
  Chord { sym: KEY_P,     mask: CTRL | SHIFT, action: KeyAction::OpenOverlay(Overlay::Diagnostics) },
  ```

  **但这需要 `KeyAction` 新变体**（`ASM-08`）。降级路径（默认采用，Phase 1 不触碰契约）：两条和弦**不进 `KeyAction`**，而由 `Dispatcher` 在 L0/L2 层直接处理，产生 `Consumed` 并投递诊断：

  ```rust
  /// The overlay a chord asks for.
  ///
  /// Kept out of `KeyAction` on purpose: the action enum is the session's
  /// vocabulary, and an overlay is not something a session can execute -- it is
  /// a host-layer mode. Putting it in the enum would make every session state
  /// answer a question that is not its own.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum Overlay {
      /// `Ctrl+Shift+/`: the command palette (`TASK-2.03.03`).
      CommandPalette,
      /// `Ctrl+Shift+P`: the diagnostics panel (`TASK-2.08.01`).
      Diagnostics,
      /// `Ctrl+Shift+/` while a composition is live: the cheat sheet
      /// (`KEY-P2.02.02`).
      CheatSheet,
  }
  ```

  Phase 1 的 `Dispatcher::open_overlay` 只做三件事：记 `self.overlay = Some(kind)`、投 `ui/not-implemented: <kind>` 诊断、返回 `Consumed`。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 新增 `KEY_SLASH`（`0x002f`）与 `KEY_P`（`0x0070`）/`KEY_P_UPPER`（`0x0050`）常量。
  2. 定义 `Overlay` 枚举与 `Dispatcher::overlay` 字段。
  3. 在 `Dispatcher` 的 L2 层加入两条和弦的判定（在 `CHORDS` 表之外，因为它们产生的是 `Overlay` 而非 `KeyAction`）。
  4. 写 `open_overlay` 与 `close_overlay`，Phase 1 的 `open` 只投诊断。
  5. 写测试：两条和弦返回 `Consumed` 且投出 `ui/not-implemented`；浮层打开时 `Escape` 关闭且返回 `Consumed`；浮层打开时其他键返回 `Ignored`。
- **验收标准 (DoD)**：
  - [ ] `Ctrl+Shift+/` 与 `Ctrl+Shift+P` 被消费且记录 `ui/not-implemented`（不再静默交给应用）；
  - [ ] 两条和弦在 `Ctrl+Shift+Alt+/` 等额外修饰键下交还宿主；
  - [ ] `Overlay` 的三种取值都有对应测试；
  - [ ] Phase 1 不新增任何 `KeyAction` 变体（`ime-types` 零改动，`scripts/check-*` 全绿）。

---

### 任务 ID：KEY-P1.03.01 白名单与路由表一致性断言

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-05`、`KEY-DEF-19`、`CMD-07`、`CMD-08`
  - 优先级与复杂度：`P1 | 中 | 预估工时: 1.5 人天`
  - 前置依赖：`KEY-P0.01.05`
  - 关键路径：`CP: 否`
  - 并行通道：`Track C 速查面板·持久化·基建`
  - 代码落地锚点：`crates/ime-fcitx5/src/engine/binding_audit.rs`（新建）、`crates/ime-config/src/schema.rs`
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  关闭 `KEY-DEF-05`（白名单大于路由能力）与 `KEY-DEF-19`（`MAX_KEY_BINDINGS = 8` 与路由容量不一致）。

  | 不变量 | 断言位置 | 失败时的行为 |
  |---|---|---|
  | `KeyName` 的每一个取值都能被投影成至少一个可路由键位 | `binding_audit::every_name_is_routable` | 单元测试失败（构建期） |
  | `MAX_KEY_BINDINGS` ≤ 任一列表的可路由键位数 | `binding_audit::the_bound_is_reachable` | 单元测试失败 |
  | `flip_keys` 的每个条目都落在 `FlipSet` 的位域内 | `project_keys`（`KEY-P0.01.05`） | 运行期 `keys/unroutable-binding` 诊断 |
  | `highlight_keys` 的每个条目都落在 `HighlightSet` 的位域内 | 同上 | 同上 |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  ```rust
  /// Proves that the configuration's key-name whitelist and this layer's routing
  /// table describe the same set of keys.
  ///
  /// The two live in different crates and drift silently: `page_up` was in the
  /// whitelist and had no routing row, so a user who wrote
  /// `flip_keys = ["page_up"]` got a configuration that was accepted, stored and
  /// then ignored. A unit test is the only place that can hold both halves at
  /// once, so the audit lives here and names every unrouteable key.
  #[test]
  fn test_every_whitelisted_key_name_is_routable() {
      // The whitelist is enumerated by hand rather than derived from `KeyName`:
      // a new variant that nobody adds here is the drift this test exists to
      // catch, so deriving the list would defeat it.
      let names = [
          "minus", "equal", "up", "down", "left", "right",
          "tab", "shift_tab", "page_up", "page_down",
      ];
      for name in names {
          let parsed = KeyName::parse(name, "keys.flip_keys").expect("whitelisted");
          let keys = KeyBindings {
              flip_keys: FlipSet::all(),
              highlight_keys: HighlightSet::all(),
              ..KeyBindings::default()
          };
          assert!(
              routes_any_binding(parsed, &keys),
              "{name} is accepted by the configuration and routed by nothing"
          );
      }
  }
  ```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 写 `binding_audit.rs` 的两个测试函数。
  2. 在 `engine.rs` 暴露 `routes_any_binding(name: KeyName, keys: &KeyBindings) -> bool`（`pub(crate)`），实现方式是对该键名对应的 keysym 走一次 `translate_key` 并看是否非 `Ignore`。
  3. 复核 `MAX_KEY_BINDINGS`：若 `HighlightSet` 有 6 位、`FlipSet` 有 6 位，则 8 是可达上限；若不等，改 `schema.rs:32` 的常量并在 `schema.rs:764-767` 的注释中说明。
  4. 在 `ime-config` 的 `KeyName` 上补一行文档注释，指明"新增变体必须同时在 `ime-fcitx5` 的 `binding_audit` 中登记"。
- **验收标准 (DoD)**：
  - [ ] 白名单 10 项全部可路由，测试通过；
  - [ ] 人为删除 `KEY_PAGE_UP` 行后测试**失败**并报出 `page_up`（证明断言有效）；
  - [ ] `MAX_KEY_BINDINGS` 与两个位标志集合的容量关系有明确断言；
  - [ ] `KeyName` 的文档注释包含与 `binding_audit` 的联动说明。

---

### 任务 ID：KEY-P1.03.02 跨列表键位冲突校验与诊断

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-17`、`CMD-07`、`CMD-08`、`CMD-11`、`CMD-12`
  - 优先级与复杂度：`P1 | 中 | 预估工时: 2.0 人天`
  - 前置依赖：`KEY-P1.03.01`
  - 关键路径：`CP: 否`
  - 并行通道：`Track C 速查面板·持久化·基建`
  - 代码落地锚点：`crates/ime-config/src/schema.rs`（`Config::repaired` 与 `repair_bindings`）
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  关闭 `KEY-DEF-17`：`repair_bindings`（`schema.rs:640-653`）只在单列表内去重，跨列表冲突无人检查。

  | 冲突 | 当前 | 目标 |
  |---|---|---|
  | `flip_keys` 内重复 | 报 `config/invalid`，保留首次出现 | 不变 |
  | `highlight_keys` 内重复 | 报 `config/invalid` | 不变 |
  | `flip_keys` 与 `highlight_keys` 重叠 | **静默接受** | 报 `config/invalid`，键名 `keys.highlight_keys`，**保留 `highlight_keys` 的条目、从 `flip_keys` 中移除** |
  | 列表超长（> `MAX_KEY_BINDINGS`） | 报 `config/limit-exceeded`，截断 | 不变 |

  冲突消解方向（保留 `highlight_keys`）与 `KEY-P1.02.01` 的运行期优先级一致，配置层与路由层给出同一个答案。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  ```rust
  /// Drops from `flip_keys` every key `highlight_keys` already claims.
  ///
  /// The two lists are separate settings but one keymap: a key that pages and a
  /// key that moves the highlight cannot be the same key. The overlap used to be
  /// accepted silently, and the routing table then resolved it by evaluation
  /// order -- which is a decision the user never sees. Reporting it and keeping
  /// the highlight entry makes the configuration and the router agree by
  /// construction.
  fn repair_cross_list_conflicts(keys: &mut KeysConfig, warnings: &mut Warnings) {
      let claimed: Vec<KeyName> = keys.highlight_keys.clone();
      keys.flip_keys.retain(|name| {
          if claimed.contains(name) {
              warnings.report(
                  KEY_FLIP_KEYS,
                  format!("{} also moves the highlight; the highlight binding wins", name.as_str()),
              );
              false
          } else {
              true
          }
      });
  }
  ```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 写 `repair_cross_list_conflicts`，在 `Config::repaired` 的 `repair_bindings` 两次调用**之后**调用（顺序重要：先各自去重，再消解重叠）。
  2. 在 `schema.rs` 的 `scalar_breakers` 风格测试表中加入一条跨列表冲突用例。
  3. 写测试：`flip_keys = ["up"]` + `highlight_keys = ["up"]` → 诊断 + `flip_keys` 中无 `up` + `highlight_keys` 保留 `up`。
  4. 写测试：不重叠时**零诊断**（避免过度报错）。
- **验收标准 (DoD)**：
  - [ ] 跨列表重叠产出恰好一条 `config/invalid`，键名为 `keys.flip_keys`；
  - [ ] 消解后 `flip_keys ∩ highlight_keys == ∅`；
  - [ ] 消解结果经 `Config::validate` 后为零诊断（修复是幂等的）；
  - [ ] 配置层保留的键（`highlight_keys`）与路由层优先的键（`KEY-P1.02.01`）一致，有测试交叉验证；
  - [ ] 不重叠的默认配置零诊断（`schema.rs:697` 的 `test_repaired_restores_the_default_of_every_scalar_key` 不回归）。

---

### 任务 ID：KEY-P1.03.03 候选网格数字标签与高亮态渲染

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-15`、`CMD-06`、`CMD-11`、`CMD-12`
  - 优先级与复杂度：`P1 | 高 | 预估工时: 4.0 人天`
  - 前置依赖：无（UI 内部）
  - 关键路径：`CP: 否`
  - 并行通道：`Track C 速查面板·持久化·基建`
  - 代码落地锚点：`crates/ime-ui/ui/candidate.slint`（`CandidateGrid` 与新增 `CandidateCell`）、`crates/ime-ui/src/adapter.rs`（新建）
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  关闭 `KEY-DEF-15`。这是 `CMD-06`（`1`~`9` 选词）与 `CMD-11`/`CMD-12`（高亮移动）的**视觉半边**——键位已存在，用户看不见它对应哪个候选。

  features.md 3.4 的五态与优先级（`Disabled` > `Active` > `Focus Ring` > `Hover` > `Default`），以及"键盘高亮与鼠标悬停同时存在时以键盘高亮为准"：

  | 状态 | 背景 | 文本 | 触发条件 |
  |---|---|---|---|
  | `Default` | 透明 | `text.primary`，序号 `text.annotation` | 无 |
  | `Hover` | `state.hover`，`radius 8dp` | 不变 | 指针在该单元上 |
  | `Active` | `state.pressed`，`scale 0.97`（60ms） | 不变 | 指针按下 |
  | `Focus Ring` | `state.selected.bg` + `1dp state.selected.stroke` | `15sp / 500` | `index == page.highlight` |
  | `Disabled` | 文本 `opacity 0.32`，无 hover | 指针 `default` | 候选不可选（当前无此情形，为 Phase 2 预留） |

  数字标签：`1`~`9`，1-based，取自 `Candidate.index`（`ui.rs:125-140`，契约已保证"matches the number keys 1..=9"）。**标签必须在布局中被算入 `cell-chrome-width`**（`candidate.slint:75-78` 已预留 36dp = 2×10dp padding + 序号 + 6dp gap）。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **① `CandidateCell` 组件**（`candidate.slint`）：

  ```slint
  // One candidate: its number label, its text and its five states.
  //
  // The component is exported because the Phase 2 command palette reuses it
  // (TASK-2.03.03); the state priority is written here once rather than at each
  // call site, because a second copy is a second answer to "which state wins".
  export component CandidateCell inherits Rectangle {
      in property <int> number: 0;
      in property <string> text: "";
      in property <string> annotation: "";
      in property <bool> show-annotation: true;
      in property <bool> is-highlighted: false;
      in property <bool> is-hovered: false;
      in property <bool> is-pressed: false;
      in property <bool> is-disabled: false;

      // Priority, highest first: Disabled > Active > Focus Ring > Hover > Default.
      private property <bool> effective-hover: root.is-hovered && !root.is-disabled;
      private property <bool> ring: root.is-highlighted && !root.is-disabled && !root.is-pressed;
      // ...
  }
  ```

  **② 适配器**（`crates/ime-ui/src/adapter.rs`）：

  `UiFrame` 是只读快照，`.slint` 是声明式的，两者之间需要一层把 `PageState` + `Paging::local_index` 折算成"哪一项是第几个、哪一项高亮"的纯映射。**这一层必须是纯函数且不导出 Slint 类型**（`OB-4`）。

  ```rust
  /// One cell as the window draws it.
  ///
  /// A plain value type with no Slint in it: the adapter produces these and the
  /// UI thread pushes them into the generated model. Keeping the conversion here
  /// rather than in the `.slint` file is what lets the mapping be unit-tested
  /// without a display server.
  #[derive(Clone, Debug, PartialEq)]
  pub struct CellView {
      /// 1-based number shown to the left of the text.
      pub number: u16,
      /// Candidate text, already truncated to `max-text-width` by the caller.
      pub text: String,
      /// Annotation, empty when `ui.show_annotation` is off.
      pub annotation: String,
      /// Whether this cell carries the keyboard highlight.
      pub is_highlighted: bool,
  }
  ```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在 `candidate.slint` 新增 `export component CandidateCell`，实现五态与数字标签，全部尺寸引用 `CandidateMetrics`。
  2. 把 `CandidateGrid` 的占位 `Rectangle` 换成 `CandidateCell`，`for` 循环的索引改为 `local_index` 基址 + 列偏移。
  3. 写 `adapter.rs` 的 `CellView` 与 `cells_for_frame(&UiFrame) -> Vec<CellView>`。
  4. 在 UI 线程的 `SurfaceUpdate::Frame` 应用路径上调用 `cells_for_frame` 并写入 Slint 模型。
  5. 写测试：45 个候选、5 页、第 3 页时 `cells_for_frame` 的 `number` 序列为 `11..=15`，`is_highlighted` 恰好一项为真。
  6. 更新 `crates/ime-ui/src/layout.rs` 的 `Metrics` 解析测试（若新增了尺寸常量）。
- **验收标准 (DoD)**：
  - [ ] 候选单元显示 1-based 数字标签，与 `1`~`9` 选词键一一对应；
  - [ ] `Tab`/`Shift+Tab` 移动高亮时，`Focus Ring` 状态在候选间移动且同时只有一个；
  - [ ] 鼠标悬停与键盘高亮同时存在时，`Focus Ring` 胜出（features.md 3.4 的优先级规则）；
  - [ ] `ui.show_annotation = false` 时注解不占位、单元宽度随之收缩；
  - [ ] `.slint` 中不出现任何 `TextInput`、`forward-focus` 或 `focus()` 调用（`AGENTS.md` 禁止事项 20 的静态检查）；
  - [ ] `crates/ime-ui` 的公共 API 不导出任何 `slint::` 类型（`scripts/check-slint-leak.sh` 通过）。

---

### 任务 ID：KEY-P1.03.04 鼠标路径与键盘路径同源

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-14`、`CMD-23`、`CMD-24`、`CMD-25`
  - 优先级与复杂度：`P1 | 高 | 预估工时: 3.5 人天`
  - 前置依赖：`KEY-P1.03.03`、`KEY-P1.02.05`
  - 关键路径：`CP: 否`
  - 并行通道：`Track C 速查面板·持久化·基建`
  - 代码落地锚点：`crates/ime-ui/src/interaction.rs`（新建，即 `TASK-1.05.06` 的 Code Anchor）、`crates/ime-ui/src/ui_thread/surface.rs`
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  关闭 `KEY-DEF-14`。features.md 3.5 明写"输入法的一切操作必须可纯键盘完成，鼠标只是补充"——**补充路径也必须存在**，否则 `CMD-23`~`CMD-25` 三条命令永远不可达。

  | `SurfaceEvent` | 命中测试 | 产出 `UiEvent` | 备注 |
  |---|---|---|---|
  | `PointerMotion{x,y}` | 在候选网格内 → 该单元索引；在网格外 → `None` | `Hover{revision, index}` | 经 `UiEventQueue::post_hover` 的 16ms 节流 |
  | `PointerButton{button:1, pressed:true}` | 在候选单元内 | `Select{revision, index, trigger: Mouse}` | 经 `post_select`（**永不丢弃**） |
  | `PointerButton{button:1, pressed:true}` | 在容器外 | `Dismiss{revision, reason: OutsideClick}` | |
  | `Axis{delta, horizontal:false}` | — | `Page{revision, dir}` | `delta < 0` → `Prev`，`delta > 0` → `Next` |
  | `Axis{delta < 0}` | 已在首页 | `Dismiss{revision, reason: ScrollUpEmpty}` | features.md 2.3 的"首页向上滚关掉候选框" |
  | `PointerLeave` | — | `Hover{revision, index: None}` | 不改高亮 |

  **同源要求**：命中测试用的网格必须与 `KEY-P1.03.03` 渲染用的网格是**同一个 `GridLayout`**（`crates/ime-ui/src/layout.rs` 的 `GridLayout{cols, rows, pages, overflow}`），否则点击的单元与看到的单元会错位。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  ```rust
  /// Turns a pointer position into the candidate it is over.
  ///
  /// The geometry comes from the same `GridLayout` the renderer draws from, so a
  /// click can never land on a cell other than the one under the pointer. The
  /// shadow reserve is subtracted first: the surface is larger than the panel,
  /// and a pointer in the shadow band is outside the grid rather than on
  /// candidate zero.
  ///
  /// # Returns
  ///
  /// The **global** candidate index -- the same numbering `Paging::highlight`
  /// and `UiEvent::Select` use -- or `None` outside the grid.
  pub fn hit_test(
      point: (i32, i32),
      layout: &GridLayout,
      metrics: &Metrics,
      page: &PageState,
      scale: f32,
  ) -> Option<u16> { /* ... */ }
  ```

  **与键盘路径的等价性**：`Select{index}` 与 `SelectIndex(digit)` 必须命中**同一个候选**。两者的索引来源不同（一个是命中测试，一个是 `Paging::digit_target`），因此需要一条交叉验证测试。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 写 `interaction.rs` 的 `hit_test`，输入 `(x, y)` 与 `GridLayout`，输出全局候选索引。
  2. 写 `surface.rs` 侧的事件翻译：把 `SurfaceEvent` 序列折算成 `UiEvent` 并投进 `UiEventQueue`（区分 `post_select` / `post_ordered` / `post_hover`）。
  3. 在 `crates/ime-ui/src/renderer.rs:322` 的 `apply_geometry_event` 之外，新增一条指针事件的消费路径（**不改 `apply_geometry_event`**，它正确地只处理几何事件）。
  4. 写测试：命中测试在 5 页 × 9 列的网格上逐单元往返（`hit_test(cell_rect(i)) == Some(i)`）。
  5. 写交叉验证测试：`hit_test` 命中的索引与 `Paging::digit_target(n, total)` 在 `n = local_index + 1` 时相等。
- **验收标准 (DoD)**：
  - [ ] 点击任一候选单元提交的候选，与按对应数字键提交的候选**完全相同**（交叉验证测试）；
  - [ ] 指针在阴影带（`shadow-margin` 内、容器外）时 `hit_test` 返回 `None`；
  - [ ] 滚轮在首页向上滚产出 `Dismiss{ScrollUpEmpty}`，在非首页向上滚产出 `Page{Prev}`；
  - [ ] `Select` 走 `post_select`（永不丢弃），`Hover` 走 `post_hover`（16ms 节流），`Page`/`Dismiss` 走 `post_ordered`（有序合并）；
  - [ ] 全部测试在**无显示服务器**下通过（`MockBackend`）。

---

### 任务 ID：KEY-P1.03.05 键位映射的端到端表驱动测试

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-01`、`CMD-01`、`CMD-02`
  - 优先级与复杂度：`P1 | 中 | 预估工时: 3.0 人天`
  - 前置依赖：`KEY-P1.02.01`~`KEY-P1.02.08`
  - 关键路径：`CP: 否`
  - 并行通道：`Track C 速查面板·持久化·基建`
  - 代码落地锚点：`crates/ime-fcitx5/tests/keymap_matrix.rs`（新建）、`xtask/src/testd/engine/builtin.rs`（`ASM-11` 的缺口）
  - 当前状态：`[ ] 待开始`
- **交互目标与按键映射矩阵**：

  关闭 `ASM-11` 登记的测试缺口：测试平台当前**直接注入 `KeyAction`**（`xtask/src/testd/engine/builtin.rs:318`），绕过路由表。于是"表驱动测试通过"只证明了状态机，没证明按键能被翻译成那些动作。

  | 测试 | 输入 | 断言 | 对应 `docs/dev/tests.md` |
  |---|---|---|---|
  | 3.5 全表 | features.md 3.5 的 19 行，每行构造 `FcitxKeyEvent` | 翻译为期望的 `KeyAction`；15 个变体全部可达 | `TC-RT-06` |
  | 绝不吞键 | 200 个伪随机 keysym × 4 种修饰键状态 | 返回 `true` 的必有非 `Diagnose` 的 `Effect` | `TC-RT-07` |
  | 释放边沿 | 全部按键的 `is_release = true` | 返回 `false` | `TC-RT-08` |
  | 键位矩阵往返 | 每个键位 × 每种配置分支 | `translate_key` 的输出与 `Dispatcher::dispatch` 在单会话下一致 | 新增 |
  | keysym 级注入 | `xtask/src/testd` 通过 XTEST 注入真实 keysym | 端到端产出期望的候选与上屏文本 | `TC-RT-10` |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  测试放在 `crates/ime-fcitx5/tests/`（集成测试，不占 `src/` 的行数预算；`AGENTS.md` 第 5 节允许 `tests/` 放宽到 1200 行）。

  `xtask` 侧新增一条 keysym 注入路径：

  ```rust
  /// One step expressed as the keys a user would press.
  ///
  /// The existing steps inject `KeyAction` directly, which proves the session
  /// state machine but says nothing about whether a keystroke reaches it. A step
  /// that carries the keysym and the modifier mask goes through the whole chain:
  /// translation, arbitration, session, effects.
  pub struct KeyStrokeStep {
      /// XKB keysym to press.
      pub keysym: u32,
      /// Modifier mask held while it is pressed.
      pub state: u16,
      /// What the step expects.
      pub expect: Expectation,
  }
  ```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 写 `crates/ime-fcitx5/tests/keymap_matrix.rs`，把 features.md 3.5 的 19 行逐行转成 `FcitxKeyEvent` 用例。
  2. 把 `engine.rs` 现有的 `test_translate_key_never_claims_a_key_the_table_does_not_name`（`:675`）扩展为完整的"绝不吞键"用例，走 `handle_key` 而非 `translate_key`。
  3. 写释放边沿用例，覆盖全部被路由的 keysym。
  4. 写"`translate_key` 与 `Dispatcher::dispatch` 一致"的往返测试。
  5. 在 `xtask/src/testd/engine/` 新增 `KeyStrokeStep` 与对应的执行路径。
- **验收标准 (DoD)**：
  - [ ] `TC-RT-06`：3.5 表中每一行都有对应用例，`KeyAction` 的 15 个变体全部可达；
  - [ ] `TC-RT-07`：200 个随机 keysym 的"绝不吞键"通过；
  - [ ] `TC-RT-08`：全部 `is_release` 返回 `false`（`KEY-P0.01.03` 的修饰键例外有独立用例并计数）；
  - [ ] 键位矩阵往返测试通过，`translate_key` 与 `dispatch` 零分歧；
  - [ ] `xtask` 的 keysym 级注入至少跑通一条端到端场景（`nihao` + `Space` → `你好`）。
