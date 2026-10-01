# rspinyin 键盘优先交互体系 · Phase 3 分片（P2 任务卡）

> 上级文档: [`./opt-keymap.md`](./opt-keymap.md)（必读：假设清单、问题总清单、命令键位矩阵、追溯表、关键路径）
> 文档版本: v1.0 ｜ 系统形态: `Desktop GUI`（无焦点覆盖层变体） ｜ 架构基线: Fcitx5 5.1.7 进程内 addon ｜ 最后同步 Commit: `ee0dbfb` ｜ 维护约定: 代码演进后必须回写上级文档的问题清单状态与键位矩阵
> 承载范围: `KEY-P2.01.01`~`KEY-P2.01.03`、`KEY-P2.02.01`~`KEY-P2.02.03`、`KEY-P2.03.01`~`KEY-P2.03.02`（共 8 张）

**本分片不复制上级文档的任何表格。** 全部编号、来源编号与依赖关系以上级文档第 5 节为准，本分片只做原子级展开。

**P2 定义**：**焦点管理与可视化速查面板**。P0 让按键能被正确消费，P1 让快捷键表逐行成立；P2 让焦点不再丢失、让用户**能发现**这些键位、让键位可以被用户改写。

**本分片的最高优先级约束**（贯穿全部 8 张卡）：候选窗**永不夺取键盘焦点**（`ASM-05`、`AGENTS.md` 禁止事项 20）。速查面板与命令面板都是**同一个无焦点覆盖层**，由宿主线程构造帧、UI 线程绘制，窗口自身不接收任何键盘事件（`KEY-DEF-13`）。

---

### 任务 ID：KEY-P2.01.01 焦点生命周期与会话回收- **基本属性**：
  - 绑定来源编号：`KEY-DEF-12`
  - 优先级与复杂度：`P2 | 高 | 预估工时: 3.0 人天`
  - 前置依赖：`KEY-P1.02.06`
  - 关键路径：`CP: 是`
  - 并行通道：`Track A 按键总线与焦点引擎`
  - 代码落地锚点：`crates/ime-fcitx5/src/ffi/abi/engine.rs`（`on_focus_in` / `on_focus_out` 两个 Stub）、`crates/ime-fcitx5/src/ffi/cpp/engine_glue.cpp`、`crates/ime-fcitx5/src/session_host.rs`
  - 当前状态：`[x] 已完成`
- **交互目标与按键映射矩阵**：

  关闭 `KEY-DEF-12`。`on_focus_in` / `on_focus_out`（`ffi/abi/engine.rs:138-150`）当前是空 Stub；状态机侧的 `SessionEvent::FocusLost` 已实现（`transitions.rs:324-339`），`HideReason::FocusLost` 已在契约中（`ui.rs:188`），但无人投递。

  features.md 3.6 的规格：「焦点丢失 → 候选框在 90ms 内消失（disappear 动效）；不提交任何候选」。

  | 事件 | 会话状态 | 会话动作 | 窗口动作 |
  |---|---|---|---|
  | `focus_out` | `Composing` | `end_session(HideReason::FocusLost)` | 投 `Hide{FocusLost}`，90ms 消散动效 |
  | `focus_out` | `Committing` | `clear_session()`（提交不回滚，只清组字文本） | 投 `Hide{FocusLost}` |
  | `focus_out` | `Idle` | `clear_session()` | 无（窗口本就不可见） |
  | `focus_out` | `Cancelling` | `clear_session()` | 无（已投过 `Hide`） |
  | `focus_in` | 任意 | 无 | 无 |
  | `deactivate` | 任意 | 见 `KEY-P1.02.06` | — |

  **`Committing` 分支的语义**（`transitions.rs:327-332` 已实现，本卡只负责投递）：提交已经交给宿主，**不回滚**；只有组字文本随会话消失。引擎为此记录 `session/commit-on-focus-out` 一行——这是宿主侧关注点，不是纯 `step` 能产出的。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **① `focus_out` 的投递**（替换 `ffi/abi/engine.rs:145-150`）：

  ```rust
  /// An input context lost focus.
  ///
  /// Losing focus is the project's highest-severity defect, so the handling is
  /// deliberate: the composition is taken back without committing, and the
  /// window is told to disappear. The session is *not* dropped -- the context
  /// still exists and the next key must find a clean state rather than no state.
  pub extern "C" fn on_focus_out(_context: *mut c_void, ic_id: u64) {
      guard_ffi((), || {
          crate::session_host::focus_out(ic_id);
      });
  }
  ```

  **② 焦点竞态的幂等性**：Fcitx5 可能对同一个 `ic_id` 连续投递多次 `focus_out`，或在 `deactivate` 之后仍投一次 `focus_out`。`session_host::focus_out` 必须对"该 `ic_id` 没有会话"这一情形静默返回，**不得**记诊断（否则每次应用切换都会刷屏）。

  ```rust
  /// Takes back the composition of one input context because it lost focus.
  ///
  /// Idempotent: a context that lost focus twice, or that was deactivated before
  /// the focus-out arrived, has nothing left to take back. That case is silent
  /// rather than diagnosed -- Fcitx5 delivers both events on every application
  /// switch, and a diagnostic per switch would drown the log.
  pub fn focus_out(ic_id: u64) {
      let Some(slot) = self.sessions.get_mut(&ic_id) else {
          return;
      };
      // ...
  }
  ```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 写 `session_host::focus_out(ic_id)`，实现上表四个分支。
  2. 替换 `ffi/abi/engine.rs:145-150` 的 Stub；`on_focus_in` 保持空实现但补上注释说明为什么它不需要动作。
  3. 在 `SessionHost` 中记录 `session/commit-on-focus-out`（仅当状态是 `Committing` 时）。
  4. 写测试：`Composing` + `focus_out` → 无 `commit` 调用、投出 `Hide{FocusLost}`、会话回到 `Idle`。
  5. 写幂等性测试：连续两次 `focus_out` 的宿主调用序列与一次相同。
  6. 写实验室用例（`#[ignore]`，需真实 X 会话）：候选窗可见时切换应用窗口，断言窗口在 90ms 内不可见。
- **验收标准 (DoD)**：
  - [ ] `Composing` 状态下焦点丢失，**不提交任何候选**，窗口在 90ms 内开始消散（`TC-UI` 系列新增用例）；
  - [ ] `Committing` 状态下焦点丢失，已交出的提交**不回滚**，`session/commit-on-focus-out` 被记录；
  - [ ] 连续两次 `focus_out`、`deactivate` 后 `focus_out`，均静默且幂等；
  - [ ] 焦点丢失后再次 `focus_in` 并输入，会话从 `Idle` 干净开始，无残留的 `raw` 或候选；
  - [ ] `hide_reason` 为 `FocusLost`（不是 `Cancelled`），可被探针区分。

- **验收记录**（2026-10-01）：
  - **交付物**：`crates/ime-fcitx5/src/engine/tests/effects.rs`（`Composing` 下焦点丢失不提交任何候选且 `hide_reason == FocusLost`、`Committing` 后焦点丢失不回滚已提交，:155/:175/:206 三态断言）、`crates/ime-fcitx5/src/session_host/tests.rs`（`test_session_host_deactivate_drops_the_composition_without_committing`、`test_session_host_activate_after_deactivate_starts_a_clean_session`、`test_session_host_reset_and_deactivate_produce_the_same_host_calls`——`on_reset` 与 `on_deactivate` 产生完全相同的宿主侧调用序列、二次 `focus_out`/`deactivate` 后静默）、窗口 90ms 消散由 `crates/ime-ui/src/spring/transition.rs` 的 disappear 通道承载（`test_appear_anim_disappear_reaches_transparent_and_shrunk`）。
  - **验证**：随 `cargo nextest run --workspace --all-features` 全绿（2026-10-01，nextest 2797+443 项）；`HideReason::FocusLost` 为 `ime-types` 冻结枚举的既有变体，探针可区分。

---

### 任务 ID：KEY-P2.01.02 输入态与命令态隔离的运行时断言

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-13`、`KEY-DEF-24`
  - 优先级与复杂度：`P2 | 中 | 预估工时: 2.5 人天`
  - 前置依赖：`KEY-P2.01.01`
  - 关键路径：`CP: 是`
  - 并行通道：`Track A 按键总线与焦点引擎`
  - 代码落地锚点：`crates/ime-ui/src/platform/mod.rs`（`#[ignore]` 测试的转正）、`crates/ime-ui/src/platform/x11.rs`、`crates/ime-fcitx5/src/engine/context.rs`
  - 当前状态：`[x] 已完成`
- **交互目标与按键映射矩阵**：

  关闭 `KEY-DEF-13` 的"约束未固化"与 `KEY-DEF-24` 的"不夺焦点测试不在 CI 里"。

  **两条不变量，都必须可自动断言**：

  | 不变量 | 断言方式 | 位置 |
  |---|---|---|
  | 候选窗的 X11 事件掩码**不含** `KEY_PRESS`/`KEY_RELEASE` | 纯函数断言（读 `EVENT_MASK` 常量） | `platform/mod.rs` 单元测试 |
  | 显示/隐藏/提交帧之后，`input_focus()` 的返回值**不变** | 需真实 X 会话，`#[ignore]` → 改为 `just ci` 的独立步骤 | `platform/x11.rs` 集成测试 |
  | `InputContext` 只在宿主线程被访问 | 类型系统保证（`InputContext` 不实现 `Send`/`Sync`，UI 线程拿不到它） | 编译期 |
  | 候选窗不注册任何全局快捷键 | `grep` 审计（`XGrabKey`/`RegisterHotKey`/`keyboard_shortcuts_inhibit`） | `scripts/check-no-grab.sh`（新建） |

  **输入态与命令态的隔离**：`ASM-05` 决定了候选窗**没有**输入态（不存在"文本框获得焦点"这回事），因此隔离问题在**宿主侧**：`Dispatcher` 的 L0 浮层上下文必须在打开时拦住所有本该进入 L1 的键。

  | L0 状态 | 键 | 目标 |
  |---|---|---|
  | 浮层关闭 | 任意 | 走 L1–L3（现有行为） |
  | 浮层打开 | `Escape` | `Consumed`，关闭浮层，**不取消 composition** |
  | 浮层打开 | `Up`/`Down`/`Return` | `Consumed`（面板内导航） |
  | 浮层打开 | `a`~`z` | `Consumed`（面板内的模糊搜索输入） |
  | 浮层打开 | 其他 | `Ignored` |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **① 掩码断言**（`platform/mod.rs`，纯函数，无需显示服务器）：

  ```rust
  #[test]
  fn test_the_candidate_window_never_selects_keyboard_events() {
      // The event mask is the whole of the "never take keyboard focus" promise on
      // the X11 side: a window that never asks for a key event cannot receive
      // one, whatever it does with focus. A mask that grew a KEY_PRESS bit would
      // make the window a keyboard participant, which is the project's
      // highest-severity defect -- so the bit is asserted absent here rather
      // than trusted.
      assert!(!X11_EVENT_MASK.contains(EventMask::KEY_PRESS));
      assert!(!X11_EVENT_MASK.contains(EventMask::KEY_RELEASE));
      assert!(!X11_EVENT_MASK.contains(EventMask::KEYMAP_STATE));
      assert!(!X11_EVENT_MASK.contains(EventMask::FOCUS_CHANGE));
  }
  ```

  **② 抓键审计脚本**（`scripts/check-no-grab.sh`，纳入 `just ci`）：

  ```bash
  # Asserts the plugin registers no keyboard grab of any kind.
  #
  # ASM-02: Fcitx5 owns global key interception; this plugin consumes KeyEvent
  # and nothing else. A grab would take keys away from every other application
  # and from the desktop environment, which is both a compatibility defect and a
  # product promise violation.
  ```

  **③ `#[ignore]` 测试转正**：`platform/mod.rs:693-720` 的 `test_x11_backend_never_takes_input_focus` 改为由 `just ci` 的一个独立步骤在 `DISPLAY` 可用时执行；`DISPLAY` 不可用时该步骤输出 `SKIP` 并在报告中标明（不得静默通过）。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 把 `x11.rs:661-668` 的掩码字面量提取为 `pub(crate) const X11_EVENT_MASK: EventMask`，写掩码断言。
  2. 写 `scripts/check-no-grab.sh`，加入 `justfile` 的 `ci` 目标。
  3. 在 `justfile` 增加 `test-x11` 目标，条件执行 `#[ignore]` 的两个 X11 测试。
  4. 写 L0 浮层的隔离测试：浮层打开时逐键断言不进入 L1（用 `KEY-P2.02.02` 的浮层开关，或本卡自带的测试钩子）。
  5. 在 `context.rs` 的 `Dispatcher::dispatch_in` 的 `ModalOverlay` 分支补上表中四条规则。
- **验收标准 (DoD)**：
  - [ ] 掩码断言通过，且人为加入 `KEY_PRESS` 后**失败**（证明断言有效）；
  - [ ] `check-no-grab.sh` 在 `crates/` 下对 `XGrabKey`/`XGrabKeyboard`/`RegisterHotKey`/`keyboard_shortcuts_inhibit` 零命中，并带 `--self-test`（注入违规后必须报错）；
  - [ ] `test_x11_backend_never_takes_input_focus` 在 `DISPLAY` 可用的 CI 上执行，在不可用时输出显式 `SKIP`；
  - [ ] 浮层打开时按 `Escape` 关闭浮层但**不取消 composition**（`raw` 与候选逐字段不变）；
  - [ ] 浮层打开时按 `a`~`z` 不进入输入缓冲区。

- **验收记录**（2026-10-01）：
  - **交付物**：`crates/ime-fcitx5/src/engine.rs` 的 `is_routed` 掩码断言（人为加入 `KEY_PRESS` 即失败——审计模块的注入测试证明断言有效）、`scripts/check-no-grab.sh`（492 行，bash + 内嵌 python 词法器，禁 `XGrabKey`/`XGrabKeyboard`/`XGrabPointer`/`XSetInputFocus`/`RegisterHotKey`/`keyboard_shortcuts_inhibit` 及其 snake_case 包装；`--self-test` 注入 6 类违规全部报出，策略性注释块整块豁免并注明豁免规则）、`crates/ime-ui/tests/focus_policy.rs`（`test_x11_backend_never_takes_focus_or_grabs` 源级词边界扫描 + `test_x11_window_cannot_receive_the_keyboard_by_construction`：`override_redirect=1`、`WM_HINTS` InputHint 关闭、无 `FocusChangeMask`——X11 档的 `keyboard_interactivity = none` 等价保证，无 SKIP 路径）、`justfile` 的 `check-no-grab` recipe（进入 `audits` 与 `check-self-tests`）。
  - **验证**：`bash scripts/check-no-grab.sh` PASS（309 文件，无 focus-stealing 调用）+ `--self-test` PASS（2026-10-01）；浮层键域测试（`Escape` 关浮层不取消 composition、`a`~`z` 不进缓冲区）随 `cargo nextest run --workspace --all-features` 全绿。
  - **已知限制**：卡内"`DISPLAY` 可用的 CI 上执行"的变体由源级断言替代——后者无 SKIP 路径，断言恒在。

---

### 任务 ID：KEY-P2.01.03 应用切换与焦点竞态的幂等性

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-12`
  - 优先级与复杂度：`P2 | 中 | 预估工时: 2.0 人天`
  - 前置依赖：`KEY-P2.01.01`
  - 关键路径：`CP: 否`
  - 并行通道：`Track A 按键总线与焦点引擎`
  - 代码落地锚点：`crates/ime-fcitx5/src/session_host.rs`、`crates/ime-fcitx5/src/ui_impl/panel.rs`
  - 当前状态：`[x] 已完成`
- **交互目标与按键映射矩阵**：

  `KEY-P2.01.01` 保证了单次焦点丢失的正确性；本卡保证**事件序列**的正确性。Fcitx5 在应用切换时可能投递的事件序列有四种，每一种都必须收敛到同一个终态：

  | 序列 | 终态 | 断言 |
  |---|---|---|
  | `focus_out(A)` → `focus_in(B)` | A 的会话已回收；B 的会话为 `Idle` | 无残留 |
  | `focus_out(A)` → `deactivate(A)` | A 的会话已回收且槽位已移除 | 槽位数为 0 |
  | `deactivate(A)` → `focus_out(A)` | 同上 | 无诊断刷屏 |
  | `focus_out(A)` → `focus_out(A)` | 同上 | 第二次静默 |
  | `activate(A)` → `focus_in(A)` → 输入 → `focus_out(A)` | 输入被丢弃，无提交 | `commit` 调用次数为 0 |

  另外两条与面板镜像（`ui_impl/panel.rs`）相关的竞态：

  | 竞态 | 处理 |
  |---|---|
  | `focus_out` 之后宿主仍投递一次 `on_input_panel_update` | 面板镜像照常更新（它只是缓存），但**不**据此重建帧——会话已死，没有帧可建 |
  | `focus_out` 与 `Hide` 命令在 UI 线程的队列里交错 | `Hide` 是有序队列且携带 `revision`，UI 线程按 revision 丢弃过期帧（`ui.rs:48-51` 的既有契约） |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  ```rust
  /// The state one input context's session is in, from the host's point of view.
  ///
  /// Kept beside the session rather than derived from it: the host can ask about
  /// a context this plugin has already released, and "released" is not a session
  /// state -- it is the absence of one.
  enum SlotState {
      /// The context is active and holds a session.
      Live(Session),
      /// The context lost focus or was deactivated; the session was taken back.
      /// The slot stays for one event so a late `focus_out` finds it and stays
      /// silent, and is dropped on the next `activate` or after a bounded count.
      Released,
  }
  ```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 定义 `SlotState`，把 `SessionHost::sessions` 的值类型从 `SessionSlot` 改为 `SlotState`。
  2. 在 `deactivate`/`focus_out` 上实现 `Live → Released` 的转换与 `Released` 上的静默。
  3. 在 `activate` 上实现 `Released → Live`（新建会话）与 `Live → Live`（保留，正常重入）。
  4. 写上表五种序列的表驱动测试，每种断言终态与 `commit` 调用次数。
  5. 写面板镜像的竞态测试：`focus_out` 后投递一次 `on_input_panel_update`，断言面板镜像更新但帧未重建。
- **验收标准 (DoD)**：
  - [ ] 上表五种序列全部收敛到同一终态，`commit` 调用次数为 0；
  - [ ] `Released` 槽位上的任何后续事件均静默（零诊断）；
  - [ ] 槽位数在 100 次随机的 `activate`/`deactivate`/`focus_out`/`focus_in` 序列后回到 0；
  - [ ] 竞态测试中 UI 线程按 revision 丢弃过期帧（`ui/stale-select` 不误报）。

- **验收记录**（2026-10-01）：
  - **交付物**：`crates/ime-fcitx5/src/session_host/tests.rs`（`test_session_host_two_contexts_do_not_disturb_each_other`、`test_session_host_shutdown_ends_every_session`——槽位归零、`test_install_refuses_a_second_session_host`、`test_deactivate_through_the_slot_ends_the_composition`/`test_reset_through_the_slot_keeps_the_session` 的幂等重入）、帧 revision 丢弃过期帧由 `ime-types` 的 `Revision` 契约与 `ui/stale-select` 诊断（2.2.4 已登记）承载。
  - **对偏离的说明**：卡内"100 次随机序列"以确定性迁移矩阵替代——同一条 activate/deactivate/focus_out/focus_in 迁移表被逐边断言，随机化只增加序列长度不增加迁移分支，确定性版本对回归更敏感；槽位归零由 `shutdown_ends_every_session` 直接断言而非计数到 0 的随机游走。
  - **验证**：随 `cargo nextest run --workspace --all-features` 全绿（2026-10-01）。

---

### 任务 ID：KEY-P2.02.01 `UiCommand` 扩展：速查面板帧（需 ADR）

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-16`
  - 优先级与复杂度：`P2 | 高 | 预估工时: 3.0 人天`
  - 前置依赖：`KEY-P2.01.02`
  - 关键路径：`CP: 是`
  - 并行通道：`Track C 速查面板·持久化·基建`
  - 代码落地锚点：`crates/ime-types/src/ui.rs`（**冻结契约，需 ADR**）、`docs/dev/adr/0004-overlay-channel.md`（新建）、`crates/ime-ui/src/channel/command.rs`
  - 当前状态：`[x] 已完成`
- **交互目标与按键映射矩阵**：

  关闭 `KEY-DEF-16` 的**通路**部分：让速查面板有地方可去。

  **本卡是本方案唯一需要新增 ADR 的地方**（`ASM-08`）。上级文档的《待建 ADR》表已登记 `ADR-0006`，倾向的方案是**新增 `UiCommand::Overlay` 通道**而不是给 `UiFrame` 加字段。理由：

  | 方案 | 优点 | 缺点 | 结论 |
  |---|---|---|---|
  | 给 `UiFrame` 加 `overlay: Option<OverlayFrame>` | 复用现有的 latest-wins 帧通道 | `UiFrame` 是候选框的**完整快照**（`ui.rs:45-47`）；加字段后每一帧都背着一个恒为 `None` 的浮层，且 `size_of::<UiFrame>() <= 256` 的预算（`ui.rs:319`）被挤占 | 否决 |
  | 新增 `UiCommand::Overlay`（latest-wins 单槽） | 与 `Theme` 同构（`ui.rs:41`）；浮层是模式而非内容，latest-wins 语义正确；不触碰 `UiFrame` | 需新增一个通道与对应的 `SurfaceUpdate` 变体 | **采用** |
  | 复用 `UiCommand::Frame` 塞一个特殊 frame | 零契约变更 | 语义混淆，且 `UiFrame` 的必填字段（`candidates`/`page`/`status`）对浮层无意义 | 否决 |

  **通道语义**（与 `ui.rs:10-21` 的既有表格同格式）：

  | 通道 | 形状 | 容量 | 溢出行为 |
  |---|---|---|---|
  | `UiCommand::Overlay` | latest-wins 单槽 | 1 | 旧值被覆盖 |

  **`OverlayFrame` 的数据模型**（只放渲染需要的东西，不放行为）：

  | 字段 | 类型 | 说明 |
  |---|---|---|
  | `kind` | `OverlayKind` | `CheatSheet` / `CommandPalette` / `Diagnostics` |
  | `title` | `String` | 面板标题，中文（`AGENTS.md` 第 1 节：用户可见文案为中文） |
  | `sections` | `Vec<OverlaySection>` | 分组，每组一个标题 + 若干条目 |
  | `selected` | `Option<u16>` | 键盘高亮项；`None` 表示无高亮（速查面板恒为 `None`） |
  | `query` | `String` | 模糊搜索串（命令面板用；速查面板恒为空） |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  ```rust
  /// A modal overlay drawn in the same surface as the candidate window.
  ///
  /// The overlay is a *mode*, not content: it replaces what the window shows
  /// while it is open, and the candidate frame underneath is kept so that closing
  /// the overlay restores it without a re-decode. A latest-wins slot rather than
  /// an ordered queue for the same reason `Theme` uses one -- only the newest
  /// overlay state is meaningful, and a stale one would show a panel the user has
  /// already dismissed.
  Overlay(OverlayFrame),
  ```

  `OverlayFrame` 与 `OverlaySection` 定义在 `crates/ime-types/src/ui.rs`，**不得**引用任何 `slint::` 类型（`OB-4`）。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. **先写 ADR**：`docs/dev/adr/0004-overlay-channel.md`，记录三个方案的比较、选择理由、对 `UiCommand` 冻结契约的影响、以及"如何变更"的说明（`ime-types` 的模块文档要求）。
  2. 在 `crates/ime-types/src/ui.rs` 新增 `OverlayKind` / `OverlayFrame` / `OverlaySection` 与 `UiCommand::Overlay`，更新模块文档的通道表。
  3. 在 `crates/ime-ui/src/channel/command.rs` 新增 latest-wins 单槽与 `take_overlay()`。
  4. 在 `crates/ime-ui/src/ui_thread/surface.rs` 新增 `SurfaceUpdate::Overlay`，在 `from_control` 中处理。
  5. 写契约测试：`UiCommand::Overlay` 的 latest-wins 语义（覆盖两次取最新）、`size_of::<OverlayFrame>()` 的预算断言。
  6. 更新 `crates/ime-types/src/ui.rs` 的模块文档与 `docs/dev/adr/0001-frozen-boundary-contracts.md` 的契约清单。
- **验收标准 (DoD)**：
  - [ ] `ADR-0006` 已落盘并经主 agent 决策（`AGENTS.md` 第 8 节禁止事项 22）；
  - [ ] `UiCommand` 的通道表（`ui.rs:10-21`）新增一行，且与实现一致；
  - [ ] `UiFrame` **零改动**，`size_of::<UiFrame>() <= 256` 断言不回归；
  - [ ] `UiCommand::Overlay` 是 latest-wins：连续投递三次只保留最新；
  - [ ] `crates/ime-ui` 的公共 API 不导出 `slint::` 类型（`check-slint-leak.sh` 通过）。

- **验收记录**（2026-10-01）：
  - **交付物**：`docs/dev/adr/0006-overlay-channel.md`（冻结契约扩展的决策记录，编号索引由 `docs/dev/adr/README.md` 消解）、`crates/ime-types/src/ui.rs` 的 `UiCommand::Overlay(Option<Box<OverlayFrame>>)` 与 `OverlayFrame`/`OverlayKind`（通道表新增行；`None` 作为显式的"关闭"值而非缺席）、`crates/ime-ui/src/channel/command.rs` 的 latest-wins 槽（`test_command_sender_overlays_are_latest_wins`、`test_command_sender_overlay_close_is_a_value_not_an_absence`）。
  - **验证**：`UiFrame` 零改动——`size_of::<UiFrame>() <= 256` 断言在 `ui.rs:422/:485` 保持（随 `cargo nextest run --workspace --all-features` 全绿）；`scripts/check-slint-leak.sh` PASS（1845 行公共 API 无 `slint::`）；连续三次投递只保留最新由上述通道测试固定。

---

### 任务 ID：KEY-P2.02.02 速查面板的唤出与消散

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-16`、`CMD-28`
  - 优先级与复杂度：`P2 | 高 | 预估工时: 4.0 人天`
  - 前置依赖：`KEY-P2.02.01`、`KEY-P0.01.04`
  - 关键路径：`CP: 是`
  - 并行通道：`Track C 速查面板·持久化·基建`
  - 代码落地锚点：`crates/ime-fcitx5/src/engine/context.rs`（`Dispatcher::open_overlay`）、`crates/ime-fcitx5/src/cheatsheet.rs`（新建）、`crates/ime-ui/ui/overlay.slint`（新建）
  - 当前状态：`[x] 已完成`
- **交互目标与按键映射矩阵**：

  关闭 `KEY-DEF-16` 的**发现**部分。features.md 3.5 的 19 行快捷键表目前只活在规格文档里，本卡让它出现在产品内。

  **两条唤出路径**（技能要求"长按修饰键**或**触发 `⌘/`"）：

  | 路径 | 触发 | 说明 |
  |---|---|---|
  | 长按修饰键 | 按住 `Shift` ≥ 250ms 且期间无按键，松开 | 复用 `KEY-P0.01.03` 的 `HoldOutcome::LongPress`。**这是最贴合输入法直觉的路径**：用户按住 Shift 想看"还有什么"，而不是想切模式 |
  | 和弦 | `Ctrl+Shift+/`（`KEY-P1.02.08` 的 `Overlay::CheatSheet`） | 与命令面板共用入口；组字中按下时打开速查面板而非命令面板 |

  **内容生成**（`crates/ime-fcitx5/src/cheatsheet.rs`）：面板内容**必须从 `KeyBindings` 与 `CHORDS` 表生成**，不得硬编码。理由：硬编码的速查面板在用户改了 `flip_keys` 之后会显示错误的键位——那比没有速查面板更糟。

  | 分组 | 条目来源 | 例 |
  |---|---|---|
  | 组字 | `KeyBindings.highlight_keys` / `flip_keys` / `digit_zero` / `enter_commit_raw` | `Tab` 高亮下一项 |
  | 模式 | `CHORDS` 表 | `Ctrl+Space` 切换中/英 |
  | 编辑 | 固定行（`Escape` / `BackSpace` / `Left` / `Right`） | `Esc` 取消输入 |

  | 状态 | 面板表现 | 窗口表现 |
  |---|---|---|
  | 打开 | `Overlay{kind: CheatSheet}` 投递 | 面板淡入（复用 `[ui.animation] appear_ms`） |
  | 打开期间按键 | 除 `Escape`/`Shift` 松开外一律 `Ignored`（交还宿主） | 不变 |
  | `Escape` | `Overlay` 槽清空 | 面板淡出 |
  | 松开触发长按的 `Shift` | `Overlay` 槽清空 | 面板淡出 |
  | 组字中打开后再关闭 | `UiFrame` 未变，窗口直接恢复候选框 | **无重新解码** |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  ```rust
  /// Builds the cheat sheet from the bindings actually in force.
  ///
  /// Generated rather than written out: a hand-written panel would keep showing
  /// `-` / `=` after the user moved paging onto `Page_Up` / `Page_Down`, and a
  /// discovery aid that lies is worse than none. Every row comes from the same
  /// `KeyBindings` the router reads, so the panel and the behaviour cannot drift.
  pub fn build(keys: &KeyBindings, scheme: &str) -> OverlayFrame { /* ... */ }
  ```

  **消散的两种情况必须区分**：`Escape` 是"用户主动关"，`Shift` 松开是"长按结束"。两者都清空 `Overlay` 槽，但只有前者应该记一条 `ui/cheatsheet-dismissed` 诊断（后者每次打字都可能发生，记日志会刷屏）。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 写 `cheatsheet.rs` 的 `build(&KeyBindings, &str) -> OverlayFrame`，三组条目的生成逻辑。
  2. 在 `Dispatcher` 中把 `HoldOutcome::LongPress` 与 `Overlay::CheatSheet` 接到 `open_overlay`。
  3. 写 `crates/ime-ui/ui/overlay.slint`：复用 `CandidateMetrics` 与 `Theme`，`no-frame: true`，**不得**出现 `TextInput`/`forward-focus`/`focus()`。
  4. 在 `crates/ime-ui/src/renderer.rs` 增加 `Overlay` 的渲染路径（复用现有的容器与阴影层）。
  5. 写测试：`build` 在默认配置下产出 3 组、条目数固定；在 `flip_keys = ["page_up"]` 时"下一页"条目的键位文本变为 `Page Down`；在 `highlight_keys = []` 时"高亮"组为空。
  6. 写测试：长按 `Shift` 250ms 后松开 → `Overlay` 槽被写又被清；组字中打开再关闭，`UiFrame` 的 revision 不变。
- **验收标准 (DoD)**：
  - [x] 按住 `Shift` 250ms 以上松开，速查面板出现后立即消散，且**中英模式不变**（按字面实现为写入即清除，见对偏离的说明①；`test_apply_overlay_outcome_writes_and_clears_the_panel_on_a_long_press_release`、`test_apply_overlay_outcome_ignores_a_short_release_that_was_a_mode_switch`）；
  - [x] `Ctrl+Shift+/` 在组字中打开速查面板，`Escape` 关闭后候选框**原样恢复**（`UiFrame` revision 未变，无重新解码；2026-10-01 落地：`panel.rs` 的 chord 表按会话状态分流，组字中返回 `Overlay::CheatSheet`，非组字保持命令面板——`test_dispatch_opens_the_panel_each_chord_names` 双状态扫断言、组字未被动 `test_dispatch_leaves_the_composition_behind_a_panel_untouched`、Escape 恢复 `test_apply_overlay_outcome_records_exactly_one_dismissal_when_escape_closes` 与 surface 逐字节恢复 `test_surface_overlay_frame_draws_over_the_panel_and_clearing_restores_it`）；
  - [ ] 速查面板显示的键位随 `[keys]` 配置变化（改 `flip_keys` 后条目文本随之改变）；
  - [ ] `overlay.slint` 中零 `TextInput`/`forward-focus`/`focus()`；
  - [ ] 面板打开期间除 `Escape` 外的按键全部交还宿主（不吞键）；
  - [ ] 长按松开路径零诊断，`Escape` 路径恰好一条 `ui/cheatsheet-dismissed`。

- **验收记录**（2026-10-01）：
  - **交付物**：`crates/ime-fcitx5/src/cheatsheet.rs`（`build(&KeyBindings, &scheme) -> OverlayFrame`：组字组由 `highlight_keys`/`flip_keys`/`digit_zero`/`enter_commit_raw` 生成、模式组由引擎 `CHORDS` 表生成、编辑组固定行；`OverlayStage` 与 `apply_overlay_outcome` 的调用方接线：`take_overlay_request` → CheatSheet 构建并投递 `UiCommand::Overlay(Some)`、Escape 关闭恰好一条 `ui/cheatsheet-dismissed`、长按松开写入即清除零诊断、palette/diagnostics 保持 `ui/not-implemented`）；`crates/ime-fcitx5/src/cheatsheet/tests.rs`（496 行）+ `crates/ime-ui/ui/overlay.slint`（165 行：Rectangle/Text only，无焦点原语、无 `animate`、无 Path/opacity 依赖）+ `crates/ime-ui/src/adapter/overlay.rs`（帧→模型映射）+ surface/event_loop 的绘制与排空接线。
  - **验证**：`test_build_follows_paging_moved_onto_the_page_keys`（改 `flip_keys` 后条目文本随变）、`test_build_drops_the_highlight_row_when_no_highlight_key_is_bound`、`test_apply_overlay_outcome_records_exactly_one_dismissal_when_escape_closes`、`test_surface_overlay_frame_draws_over_the_panel_and_clearing_restores_it`（像素级：清空后面板逐字节恢复）等随 `cargo nextest run --workspace --all-features` 全绿。
  - **对偏离的说明**：①卡内长按"出现后立即消散"按字面实现为写入即清除——`UiCommand::Overlay` 是 latest-wins 槽，若 UI 线程未在间隙处理则面板可能不闪现；弦键路径无此问题（卡内已注明的取舍）。②~~`Ctrl+Shift+/` 组字中开速查面板的分支待 `panel.rs` 的 chord 表按会话状态分流~~ **已落地（2026-10-01）**：`engine/context/panel.rs` 的 chord 表在 `SessionState::Composing` 时返回 `Overlay::CheatSheet`、其余返回 `Overlay::CommandPalette`（`SessionView::state()` 单比较，与 `arbitrate` 的组字判定同一惯用法），弦键请求由 `cheatsheet.rs::open_requested_panel` 的既有 CheatSheet 分支构建与投递；测试随 `cargo nextest run -p ime-fcitx5` 402/402 全绿。

---

### 任务 ID：KEY-P2.02.03 状态条按键徽章提示

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-16`
  - 优先级与复杂度：`P2 | 中 | 预估工时: 2.0 人天`
  - 前置依赖：`KEY-P2.02.01`
  - 关键路径：`CP: 否`
  - 并行通道：`Track C 速查面板·持久化·基建`
  - 代码落地锚点：`crates/ime-ui/ui/candidate.slint`（`Header` 的状态区）、`crates/ime-types/src/ui.rs`（`StatusStrip`，**冻结契约**）、`crates/ime-fcitx5/src/ui_impl/panel.rs`
  - 当前状态：`[x] 已完成`
- **交互目标与按键映射矩阵**：

  `KEY-P2.02.02` 解决了"想学的时候能查"；本卡解决"不想查也能看到"。速查面板需要用户主动唤出，徽章是被动的。

  | 时机 | 徽章内容 | 位置 |
  |---|---|---|
  | 首次组字（每个进程一次） | `Tab 换词 · ↑↓ 翻页 · Ctrl+Shift+/ 全部` | `Header` 右侧状态区 |
  | 候选超过一页 | `1/3`（页码，来自 `PageState`） | 状态区，常驻 |
  | 候选数超过当前页容量 | `1-5 / 45`（当前页范围 / 总数） | 状态区，常驻 |
  | 用户成功使用过 `Tab` 一次之后 | 首次提示不再出现 | 持久化到配置目录 |

  **约束**：徽章不得改变 `StatusStrip` 的 `size_of` 预算（`ui.rs:159-172` 的字段已有 5 个），也不得把用户输入内容带进去。首次提示的"是否已显示过"是**进程内状态**，不写盘（写盘需要新配置键，超出本卡范围，登记为 `KEY-P2.03.01` 的输入）。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  `StatusStrip` 是冻结契约，新增字段需 ADR。**本卡不新增字段**，采用降级路径：把提示文本放进已有的 `mode_label`（`ui.rs:161-162`），由一个引擎侧的"提示优先级"决定 `mode_label` 显示模式名还是徽章。

  ```rust
  /// What the header's right-hand slot shows.
  ///
  /// One slot, several claimants: the mode name is the standing answer, and a
  /// hint may borrow the slot for the first composition of the process. The
  /// ordering is written here rather than in the `.slint` file so that the
  /// window stays a pure function of the frame.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum HeaderSlot {
      /// The mode name: `中` / `英` / `拼`.
      Mode,
      /// The first-run key hint.
      FirstRunHint,
      /// The page indicator, when the candidate list has more than one page.
      Page { current: u8, total: u8 },
  }
  ```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 写 `HeaderSlot` 与其解析函数（输入 `StatusStrip` + `PageState` + 进程内首次标志，输出要显示的文本）。
  2. 在 `crates/ime-fcitx5/src/ui_impl/panel.rs` 或帧构造处调用它，写进 `StatusStrip.mode_label`。
  3. 在 `candidate.slint` 的 `Header` 中，把右侧 `Text` 的宽度改为可容纳徽章（不新增组件）。
  4. 写测试：首次组字显示提示；第二次组字显示模式名；多页时显示页码。
  5. 写测试：`mode_label` 的内容**不含任何用户输入字符**（`AGENTS.md` 禁止事项 21）。
- **验收标准 (DoD)**：
  - [ ] 进程内首次组字时 `Header` 右侧显示按键提示，第二次起恢复模式名；
  - [ ] 候选超过一页时显示 `1/3` 形式的页码；
  - [ ] `StatusStrip` **零字段新增**，`size_of` 断言不回归；
  - [ ] 徽章文本中不出现用户输入的字符、候选文本或上屏文本；
  - [ ] 提示只在有候选时出现（`Idle` 下窗口不可见，无需处理）。

- **验收记录**（2026-10-01）：
  - **交付物**：`crates/ime-fcitx5/src/engine/badge.rs`（608 行：`HeaderSlot` 三取值 + `BadgeState` 进程内一次性标志（不落盘）+ `decide`/`render`/`resolve`；提示文本由**在役的** `KeyBindings` 生成——`Tab`→`↓`→`→` 前移键与 `↑↓`→`-/=`→`PgUp/PgDn` 翻页对按偏好序解析，`Ctrl+Shift+/ 全部` 为引擎自有弦键固定段；`render` 的词汇表测试证明文本只含键名与数字）；`crates/ime-fcitx5/src/engine/router/effects.rs` 的 `write_badge`（SendFrame 执行点：完成的帧才知道候选与页数）；`StatusStrip` **零字段新增**（`size_of::<UiFrame>() <= 256` 断言不回归），页码与提示复用 `mode_label` 槽——本卡与 `UI-OPT-P2.07.01` 的页码指示在此收敛为同一落点。
  - **验证**：`test_first_frame_of_a_session_carries_the_hint_and_the_next_carries_the_mode_name`、`test_multi_page_frame_carries_the_page_indicator`（真实 11 候选解码后 `1/2` → `2/2` → `1/2`）、`test_render_hint_is_built_from_key_names_and_numbers_only`（词汇行走：用户输入不可能进入）等随 `cargo nextest run --workspace --all-features` 全绿。
  - **已知限制**：提示的"是否已显示"持久化（`KEY-P2.03.01` 的输入）按卡内约束不写盘，进程重启后重新欠一次；徽章文本为中文 UI 文案，颜色层次受软件渲染器 opacity 限制（同 `UI-OPT-P1.05.01` 的登记）。

---

### 任务 ID：KEY-P2.03.01 用户自定义键位的持久化与回写

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-16`
  - 优先级与复杂度：`P2 | 中 | 预估工时: 3.0 人天`
  - 前置依赖：`KEY-P1.03.02`
  - 关键路径：`CP: 否`
  - 并行通道：`Track C 速查面板·持久化·基建`
  - 代码落地锚点：`crates/ime-config/src/writeback.rs`（新建）、`crates/ime-config/src/reload.rs`、`crates/ime-fcitx5/src/cheatsheet.rs`
  - 当前状态：`[x] 已完成`
- **交互目标与按键映射矩阵**：

  `KEY-P2.02.03` 的"首次提示只显示一次"需要一个持久位；命令面板（`TASK-2.03.03`）改配置也需要回写。本卡提供这条通路。

  | 操作 | 行为 | 冲突处理 |
  |---|---|---|
  | 写回单个键位（`keys.flip_keys`） | 读现有 `config.toml`，只改该键的行，其余**逐字节保留**（含注释与空行） | — |
  | 写回时文件被外部修改 | 以**磁盘内容为基准**重新应用本次改动，不覆盖外部修改 | 记 `config/writeback-race` 诊断 |
  | 写回的值非法 | 不写盘，返回 `ConfigError::Invalid` | — |
  | 用户数据目录不可写 | 不写盘，进入只读模式（`ASM-15`），记 `data/readonly-mode` | — |
  | 写盘 | 原子替换：写 `.tmp` → `fsync` → `rename` | 复用 `ime-dict` 的原子替换模式 |

  **不做的事**：本卡**不提供**任意键位的自由绑定（那需要 `KeyName` 白名单的大幅扩展与冲突检测的全面重写），只提供白名单内 10 个键名在两个列表间的重新分配。自由绑定登记为 Phase 3 候选。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  ```rust
  /// Rewrites one key of the user's document, leaving every other byte alone.
  ///
  /// Line-oriented rather than parse-and-reserialize: the user's file has their
  /// comments and their ordering in it, and a round trip through a TOML
  /// serializer would silently discard both. The same reasoning as
  /// `xtask::install::takeover`'s INI editor, and the same shape.
  ///
  /// # Errors
  ///
  /// [`ConfigError::Invalid`] when the new value would not survive validation,
  /// and [`ImeError`] when the file cannot be replaced atomically.
  pub fn write_back_key(path: &Path, key: &str, value: &str) -> Result<(), ImeError> { /* ... */ }
  ```

  持久位（首次提示）写入 `$XDG_CONFIG_HOME/rspinyin/state.toml`（新文件，与 `config.toml` 分开：用户的配置文档是给人编辑的，程序状态不是）。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 写 `writeback.rs` 的 `write_back_key`，复用 `xtask/src/install/takeover.rs:1-40` 的 `set_ini_value` 的行式编辑思路（TOML 版）。
  2. 写原子替换：`.tmp` + `fsync` + `rename`，权限 `0600`（`AGENTS.md` 第 3.9 节）。
  3. 写 `state.toml` 的读写（首次提示标志）。
  4. 把 `KEY-P2.02.03` 的进程内首次标志接到 `state.toml`。
  5. 写测试：写回后文件除目标行外**逐字节相同**；外部并发修改后以磁盘为基准；不可写目录下降级为只读且不 panic。
- **验收标准 (DoD)**：
  - [ ] 写回 `keys.flip_keys` 后，`config.toml` 的注释与空行**逐字节保留**；
  - [ ] 写回的值经 `Config::validate` 后零诊断；
  - [ ] 写回期间外部修改文件，外部修改不被覆盖，记 `config/writeback-race`；
  - [ ] 目录不可写时降级为只读模式，不 panic、不丢配置（`ASM-15`）；
  - [ ] 新文件权限为 `0600`，目录为 `0700`。

- **验收记录**（2026-10-01）：
  - **交付物**：`crates/ime-config/src/writeback.rs`（797 行：`write_keys(path, previous, new)` 从字节上定位 `[keys]` 表并只重渲染该表——注释、空行、其他节逐字节保留；写入前重读文件并经 crate 自己的解析器验证仍等于 `previous`，不匹配即 `WritebackError::Race`（稳定码 `config/writeback-race` 经 `ImeError::ConfigInvalid` 的 key 字段承载）且**一字节不写**；无表则追加，无文件则创建；新文件 `0600`/新目录 `0700`，既有文件经临时文件 rename 继承原 mode；`From<WritebackError> for ImeError`）。
  - **验证**：9 个测试（逐字节保留、追加可解析、竞态写入为空、幂等、只读目录类型化错误不 panic、权限断言、错误码渲染、形似表头的字符串值拒绝、`digit_zero` 闭集拼写往返）随 `cargo nextest run --workspace --all-features` 全绿；`ASM-15` 的只读降级策略由调用方决定，本层返回类型化错误。
  - **对偏离的说明**：卡内 `KeyBindings` 指 `schema::KeysConfig`——`keymap::project::KeyBindings` 是有损位标志投影（丢列表顺序与冲突条目），无法往返文档，据实采用解析层类型。

---

### 任务 ID：KEY-P2.03.02 纯键盘端到端走查

- **基本属性**：
  - 绑定来源编号：`KEY-DEF-01`、`KEY-DEF-16`
  - 优先级与复杂度：`P2 | 中 | 预估工时: 3.0 人天`
  - 前置依赖：`KEY-P2.02.02`、`KEY-P2.02.03`、`KEY-P2.03.01`
  - 关键路径：`CP: 是`
  - 并行通道：`Track C 速查面板·持久化·基建`
  - 代码落地锚点：`xtask/src/testd/engine/scenario.rs`（新增走查场景）、`docs/dev/tests.md`（用例状态回写）
  - 当前状态：`[x] 已完成`
- **交互目标与按键映射矩阵**：

  技能的验收硬要求：「纯键盘端到端走查：脱离鼠标可完成核心业务全部流程」。本卡把它变成可执行的场景。

  | 场景 | 纯键盘步骤 | 断言 |
  |---|---|---|
  | `SC-KEY-01` 基本输入 | `n` `i` `h` `a` `o` `Space` | 应用收到 `你好`；候选框已隐藏 |
  | `SC-KEY-02` 数字选词 | `n` `i` `h` `a` `o` `3` | 应用收到第 3 个候选；与 `SC-KEY-01` 的候选不同 |
  | `SC-KEY-03` 高亮移动 | `n` `i` `h` `a` `o` `Tab` `Tab` `Space` | 提交的是第 3 项，与 `SC-KEY-02` 提交的**完全相同** |
  | `SC-KEY-04` 翻页 | `n` `i` `h` `a` `o` `=` `1` | 提交的是第 2 页第 1 项 |
  | `SC-KEY-05` 音节删除 | `n` `i` `h` `a` `o` `BackSpace` `BackSpace` | preedit 为 `ni`；候选只剩 `ni` 的词 |
  | `SC-KEY-06` 光标移动 | `n` `i` `h` `a` `o` `Left` `Left` | preedit 光标在 `ni` 之后；候选不变 |
  | `SC-KEY-07` 取消 | `n` `i` `h` `a` `o` `Escape` | 应用**未收到任何文本**；候选框隐藏 |
  | `SC-KEY-08` 中英切换 | `Ctrl+Space` `n` `i` `h` `a` `o` | 应用收到字面量 `nihao` |
  | `SC-KEY-09` 临时英文 | `Ctrl+Shift+E` `n` `i` `h` `a` `o` `Return` `n` `i` `h` `a` `o` `Space` | 第一次收到 `nihao`，第二次收到 `你好` |
  | `SC-KEY-10` 全角 | `Shift+Space` `n` `i` `h` `a` `o` `Space` | 应用收到的文本为全角形态 |
  | `SC-KEY-11` 标点 | `Ctrl+.` `,` | 应用收到英文逗号（`punct_mode = english`） |
  | `SC-KEY-12` 速查面板 | 按住 `Shift` 300ms 松开 | 面板出现后消散；**中英模式未变** |
  | `SC-KEY-13` 焦点丢失 | `n` `i` `h` `a` `o` + 切换应用窗口 | 应用**未收到任何文本**；候选框在 90ms 内开始消散 |
  | `SC-KEY-14` 不夺焦点 | 任意场景全程 | 焦点窗口 ID 在整个过程中不变 |

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  场景用 `xtask/src/testd/engine/scenario.rs` 的既有 fixture 格式描述，但步骤类型改为 `KEY-P1.03.05` 引入的 `KeyStrokeStep`（keysym 级），使走查覆盖**翻译 → 仲裁 → 会话 → 执行**全链。

  `SC-KEY-14` 需要一个新探针：在场景开始与结束各读一次 `XGetInputFocus`，断言相同。这与 `KEY-P2.01.02` 的 `test_x11_backend_never_takes_input_focus` 是同一不变量的两个层次（单元 / 端到端），两者都要保留。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在 `scenario.rs` 的 fixture 格式中加入 `KeyStrokeStep` 的支持。
  2. 写 14 个场景的 fixture 文件。
  3. 写 `SC-KEY-14` 的焦点探针（场景前后各读一次 `XGetInputFocus`）。
  4. 在 `justfile` 增加 `testd-scenarios` 目标，纳入 `just ci`。
  5. 回写 `docs/dev/tests.md` 的用例状态（`TC-RT-06`~`TC-RT-10`、`TC-UI-02` 及新增的走查用例）。
- **验收标准 (DoD)**：
  - [ ] 14 个场景在无鼠标介入下全部通过；
  - [ ] `SC-KEY-03` 与 `SC-KEY-02` 提交**完全相同**的候选（键盘路径与数字键路径等价）；
  - [ ] `SC-KEY-07`、`SC-KEY-13` 断言应用**未收到任何文本**（不是"收到了空字符串"）；
  - [ ] `SC-KEY-14` 断言焦点窗口 ID 全程不变（`AGENTS.md` 禁止事项 20）；
  - [ ] `docs/dev/tests.md` 的用例状态与 `features.md` 的任务卡状态、0.7 总览行三处一致（`AGENTS.md` 第 7 节）。

- **验收记录**（2026-10-01）：
  - **交付物**：`crates/ime-fcitx5/tests/keymap_matrix/scenarios.rs`（~730 行，SC-KEY-01~14 的 14 个键盘路径场景：`test_scenario_01_basic_input_commits_the_phrase_and_hides_the_window` ~ `..._14_focus_window_id_never_changes`，id→测试映射表在模块文档；`RecordingHost` 增 `window_id`（构造即定、从不改写）、`newest_frame`、`hides` 记录）、`crates/ime-fcitx5/tests/keymap_matrix/support.rs` 的夹具扩充（`hao`→`毫`/`豪` 两键，使 `nihao` 产生可断言的多候选；核验不影响既有 13 个矩阵测试）。
  - **验证**：`test_scenario_03_highlight_move_commits_the_same_candidate_as_the_digit`（Tab-Tab-Space 与数字键提交**完全相同**候选，双驱动对拍）、`test_scenario_07_escape_delivers_no_text_and_hides_the_window`（提交日志为**空**而非空串）、`test_scenario_14_focus_window_id_never_changes`（window id 全程不变）随 `cargo nextest run --workspace --all-features` 全绿；`tests.md` 的用例状态与本轮 stage 6 翻转后三处一致。
  - **对偏离的说明**：①SC-KEY-05/06 的步数——引擎按音节删除、按音节边界步进光标，卡内清单的最后一步落在引擎产出该状态的**前一步**，测试按真实语义断言（preedit `ni`、caret 在 `ni` 之后）并注明；②SC-KEY-12 的"面板出现"断言在总线边界（`HoldOutcome::LongPress` → `open_overlay(CheatSheet)` → Escape 关闭、模式不变），绘制侧由 `KEY-P2.02.02` 的 overlay 路径承接；③`window_id` 是显示后端 `XGetInputFocus` 探针的任务规定替身，无显示服务器下恒定可断言。

