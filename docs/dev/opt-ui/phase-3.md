# rspinyin 候选框 UI 工艺级重构 · Phase 3（P2 控件五态、状态反馈与极度打磨）

> 文档版本: v1.0 ｜ 系统形态: **Desktop GUI（Linux 自绘候选窗，无 DOM / 无 CSS）** ｜
> 架构基线: Slint 1.13 软件光栅 + `wl_shm`/X11 32 位 ARGB ｜ 核心对标: macOS 原生候选窗、Squirrel/Rime、微软拼音；交互工艺借鉴 Linear / Raycast ｜
> 主文档: [../opt-ui.md](../opt-ui.md) ｜ 上一分片: [phase-2.md](phase-2.md) ｜ 权威规范: `docs/dev/features.md` 第 3 节 ｜
> 维护约定: 严禁引入 AI 模板风，代码演进必须回写主文档 §4 追溯矩阵

## 0. 分片基线（引用主文档，不重复定义）

- **设计假设清单**：主文档 §1。本分片强相关：§1.2 的「`.slint` 内禁止 `animate`」「键盘高亮是唯一焦点语义载体」「悬停节流 16ms」。
- **参数底座**：主文档 §2。本分片直接引用 §2.1（三层景深，`Elevated` 层在本分片首次落地）、§2.4（弹簧常量）。
- **追溯矩阵**：主文档 §4。
- **前置**：本分片全部卡片依赖主文档 P0 波次与 Phase 2 的 `P1.05.01`、`P1.02.01`。

**本分片覆盖缺陷**：`UI-DEF-10`、`UI-DEF-11`、`UI-DEF-19`、`UI-DEF-20`、`UI-DEF-22`、`UI-DEF-25`（共 6 项）。
**完成后主文档 §4 的全部 28 项缺陷归零。**

---

#### 任务 ID：`UI-OPT-P2.05.01` 指针命中、悬停、点击、滚轮与输入区域塑形

- **基本属性**：
  - 绑定缺陷编号：`UI-DEF-11`
  - 所属组件域：域03 操作控件
  - 优先级与复杂度：`P2` | 高 | 预估工时: 3.5 人天
  - 前置依赖：`UI-OPT-P0.02.02`、`UI-OPT-P0.02.03`、`UI-OPT-P1.05.02`（命中图必须与拉伸后的单元格一致）
  - 关键路径：**是**
  - 并行通道：Track C 控件与动态反馈
  - 代码落地锚点：`crates/ime-ui/src/surface.rs`（新建）、`crates/ime-ui/src/hit.rs`（新建）、`crates/ime-ui/src/geometry.rs`
  - 当前状态：`[ ] 待优化`

- **原实现弊端与微观质感缺失剖析**：
  - **现有代码具体病灶**：`geometry.rs:296` 的 `Geometry::hit_map` 已经算出「每个可见候选在**容器坐标**下的物理像素矩形 + 全局候选索引」，
    但 `grep -rn "hit_map" crates/` 在 `geometry.rs` 之外**零命中**——**没有任何函数消费它**。
    `set_input_region` 在生产路径**零调用**（`slint_platform.rs:171` 只是一个转发方法，没有调用方）。
  - **量化后果**：
    - 点击候选**没有任何反应**（`UiEvent::Select` 永不产生）。
    - 悬停**没有任何反馈**（`UiEvent::Hover` 永不产生，`channel.rs:71-84` 的 16ms 节流配置是死的）。
    - 滚轮**不能翻页**（`UiEvent::Page` 永不产生）。
    - **阴影预留区（四周 32dp）会吞掉点击**——用户想点候选框旁边 30dp 处的应用内容，点击被候选窗吃掉。
      这是 `features.md:923` 明确要求「该区域内完全透明且**输入区域为空**（点击穿透）」的一条硬规范。
  - **顶级标杆对标解析**：Raycast / Linear 的列表交互遵循「指针进入即高亮、离开即恢复、点击即确认」，
    且**指针移动到列表之外的区域立刻归还控制权**。候选窗作为浮层，最后一条尤其重要——它必须让路。

- **设计规范与参数定义 (Design Tokens & Spatial Specs)**：
  - **坐标系与换算链**（唯一一条，必须写在一处）：

    | 步骤 | 空间 | 运算 |
    |---|---|---|
    | 1 | 后端事件 | `SurfaceEvent::PointerMotion { x, y }`，**窗口相对物理像素**（含阴影预留） |
    | 2 | 容器相对物理像素 | `(x − container_offset.0, y − container_offset.1)` |
    | 3 | 命中 | 在 `hit_map` 中找包含该点的矩形 → `u16` 全局候选索引 |
    | 4 | 语义 | `UiEvent::Hover { revision, index: Some(i) }` / `None` |

  - **输入区域**：`set_input_region(&[container_rect])`，`container_rect` 由 `layout::container_rect()` 给出（物理像素、窗口相对）。
    在 `Show` 与**每次几何变化**（尺寸/缩放/位置）后都要重设。
  - **悬停节流**：16ms（`channel.rs:71`），且**仅在索引变化时投递**（`channel.rs:17` 的契约）。
  - **指针离开**：`PointerLeave` → `Hover { index: None }`。
  - **滚轮**：`SurfaceEvent::Axis { horizontal: false, delta }` → `UiEvent::Page { dir }`，
    `delta > 0` 为下一页（`platform/x11.rs:608-622` 已把符号归一化为「正数表示向前」）。
    `horizontal == true` 忽略（3.5 未定义横向滚轮语义）。
  - **点击**：`PointerButton { button: 1, pressed: false }` 触发 `Select`（**抬起**而非按下——按下即上屏会让用户无法取消）。
    `button: 2/3` 忽略（Phase 1 无右键菜单）。
  - **性能红线**：一次指针事件的命中查找是 `hit_map` 的线性扫描，候选数 ≤ 9 ⇒ 9 次矩形包含判定。
    **禁止**为命中建立哈希表或分配任何内存。
  - **契约观察（须上报主 agent）**：`DismissReason::OutsideClick` 在本形态下**不可达**——
    输入区域被塑形到容器，容器之外的点击根本不会到达本窗口。与之等价的信号是宿主上报的
    `HideReason::FocusLost`。这是一条**冻结契约**层面的说明，本卡不修改契约，只在验收记录中写明。

- **重构源码落地实现 (Code Delivery)**：

  `crates/ime-ui/src/hit.rs`（新建，纯函数、无平台依赖）：

  ```rust
  //! Pointer hit testing against the candidate grid.
  //!
  //! Responsibility: turn one pointer position into the candidate it is over, and turn the
  //! geometry pass's hit map into the input region the surface is shaped with.
  //!
  //! Boundaries: no platform types, no allocation, no state. The caller owns the current
  //! geometry and the current frame revision; this module only does arithmetic, which is what
  //! lets every case below be covered without a display server.

  use ime_types::RectI;

  use crate::geometry::Geometry;

  /// The candidate under a pointer position, or `None` when it is over the panel's padding.
  ///
  /// `window_x` and `window_y` are window-relative physical pixels, which is what a backend
  /// reports; the container offset is subtracted here so the comparison happens in the same
  /// space the hit map is expressed in. Getting that subtraction wrong is invisible in every
  /// case except the one that matters -- a click landing on the neighbouring cell.
  ///
  /// # Panics
  ///
  /// Never panics.
  pub fn candidate_at(geometry: &Geometry, window_x: i32, window_y: i32) -> Option<u16> {
      let (offset_x, offset_y) = geometry.container_offset;
      let x = i64::from(window_x) - i64::from(offset_x);
      let y = i64::from(window_y) - i64::from(offset_y);
      geometry
          .hit_map
          .iter()
          .find(|(rect, _)| contains(rect, x, y))
          .map(|(_, index)| *index)
  }

  /// Whether a point lies inside a rectangle, half-open on both far edges.
  ///
  /// Half-open matches the rule the output hit test uses (`geometry::Screen::contains`), so
  /// two cells that share an edge resolve to exactly one of them rather than to both.
  fn contains(rect: &RectI, x: i64, y: i64) -> bool {
      let left = i64::from(rect.x);
      let top = i64::from(rect.y);
      (left..left + i64::from(rect.w)).contains(&x)
          && (top..top + i64::from(rect.h)).contains(&y)
  }

  /// The region of the surface the pointer may hit: the panel, and nothing else.
  ///
  /// 3.1.1 requires the shadow reserve to be click-through, and this is the whole of that
  /// requirement: the reserve stays out of the input region, so a click 30dp to the side of
  /// the panel reaches the application underneath instead of being swallowed by an invisible
  /// window.
  ///
  /// # Panics
  ///
  /// Never panics.
  pub fn input_region(geometry: &Geometry) -> [RectI; 1] {
      let (offset_x, offset_y) = geometry.container_offset;
      [RectI {
          x: offset_x,
          y: offset_y,
          w: geometry.container_size.0,
          h: geometry.container_size.1,
      }]
  }
  ```

  `surface.rs` 中的事件处理（节选）：

  ```rust
  /// Applies one pointer event.
  ///
  /// Hover is posted only when the candidate under the pointer changes: a pointer moving
  /// inside one cell would otherwise post an event per motion sample, and the host thread
  /// would spend its 100us budget on a hover that says the same thing.
  fn on_pointer(&mut self, event: SurfaceEvent, events: &UiEventQueue) -> Result<(), ImeError> {
      let Some(geometry) = self.geometry.as_ref() else {
          return Ok(());
      };
      match event {
          SurfaceEvent::PointerMotion { x, y } | SurfaceEvent::PointerEnter { x, y } => {
              let index = hit::candidate_at(geometry, x, y);
              if index != self.hovered {
                  self.hovered = index;
                  events.post_hover(self.revision, index);
              }
          }
          SurfaceEvent::PointerLeave => {
              if self.hovered.take().is_some() {
                  events.post_hover(self.revision, None);
              }
          }
          // Selection fires on release, not on press: a user who presses a cell and drags off
          // it before letting go is cancelling, and committing on press would take that away.
          SurfaceEvent::PointerButton { x, y, button: 1, pressed: false } => {
              if let Some(index) = hit::candidate_at(geometry, x, y) {
                  events.post_select(self.revision, index, SelectTrigger::Mouse);
              }
          }
          // The vertical wheel pages; a horizontal one has no meaning in this design.
          SurfaceEvent::Axis { delta, horizontal: false, .. } => {
              let dir = if delta > 0 { PageDir::Next } else { PageDir::Prev };
              events.post_page(self.revision, dir);
          }
          _ => {}
      }
      Ok(())
  }
  ```

  输入区域在几何变化后的重设（`surface.rs`）：

  ```rust
  /// Re-shapes the surface's input region after the window moved, resized or was scaled.
  ///
  /// The region is in physical pixels relative to the window, so every one of those three
  /// changes invalidates it. Missing one leaves the panel's interactive area offset from where
  /// it is drawn -- clicks land on the wrong cell, or on nothing.
  fn refresh_input_region(&self) -> Result<(), PlatformError> {
      let Some(geometry) = self.geometry.as_ref() else {
          return Ok(());
      };
      self.platform.set_input_region(&hit::input_region(geometry))
  }
  ```

- **逐步落地实施步骤**：
  1. **纯函数先行**：写 `hit.rs` 的 `candidate_at()` 与 `input_region()`，配 6 个单测
     （格内、格边界（半开区间）、格间隙、面板内边距、容器外、`hit_map` 为空）。
  2. **输入区域塑形**：在 `Show` 与几何变化后调用 `set_input_region`；在 `Hide` 时设为空（整窗穿透）。
  3. **悬停**：接入 `PointerMotion` / `PointerEnter` / `PointerLeave`，确认**仅索引变化时**投递。
  4. **点击**：接入 `PointerButton` 的**抬起**分支；用 `MockBackend` 验证 `UiEvent::Select` 的 `revision` 与当前帧一致。
  5. **滚轮**：接入 `Axis`；横向轴忽略。
  6. **穿透验证（关键）**：在真实会话中把光标移到面板右侧 30dp 处点击，确认**应用收到该点击**而不是候选窗。
  7. **节流验证**：在同一个单元格内连续移动指针 100 次，确认只投递 **1** 个 `Hover` 事件。

- **验收标准 (DoD)**：
  - [ ] **微观隐形细节审查**：点击候选**立刻**上屏（`Select` 在抬起时发出）；悬停到新单元格时**有**事件、在同一格内移动时**无**事件。
  - [ ] **反 AI 模板风审查**：无「按下即上屏」的粗暴交互；无「悬停即上屏」；点击与悬停职责分离。
  - [ ] **排布与工学**：命中判定与绘制使用**同一份** `hit_map`（含 `P1.05.02` 拉伸后的宽度），
    不存在第二份单元格几何；容器外的点击**穿透到应用**。
  - [ ] **物理微交互**：滚轮方向与 `platform/x11.rs:608-622` 的归一化一致（向下滚 = 下一页）；
    横向滚轮被忽略而非误触发翻页。
  - [ ] **性能**：一次命中查找为 ≤ 9 次矩形判定，**零分配**；指针事件处理不阻塞 `poll` 循环（一次最多 64 个事件，`platform/x11.rs:56`）。
  - [ ] `DismissReason::OutsideClick` 不可达的结论写入验收记录并上报主 agent（冻结契约说明，不改契约）。

---

#### 任务 ID：`UI-OPT-P2.03.01` 候选单元五态与键盘 Focus Ring

- **基本属性**：
  - 绑定缺陷编号：`UI-DEF-10, UI-DEF-22`
  - 所属组件域：域03 操作控件 / 域08 键盘交互
  - 优先级与复杂度：`P2` | 高 | 预估工时: 4 人天
  - 前置依赖：`UI-OPT-P1.05.01`、`UI-OPT-P2.05.01`、`UI-OPT-P0.08.01`
  - 关键路径：**是**
  - 并行通道：Track C 控件与动态反馈
  - 代码落地锚点：`crates/ime-ui/ui/candidate.slint`（`CandidateCell`、`CandidateWindow`）、`crates/ime-ui/src/surface.rs`、`crates/ime-ui/src/spring/transition.rs`
  - 当前状态：`[ ] 待优化`

- **原实现弊端与微观质感缺失剖析**：
  - **五态零实现（`UI-DEF-10`）**：3.4 的整张表（`Default` / `Hover` / `Active` / `Focus Ring` / `Disabled`）
    在代码里**没有任何落点**。`theme.slint:55-58` 的 `state-hover` / `state-selected-bg` / `state-selected-stroke` /
    `state-pressed` 四个 Token 定义了不用；`CandidateGrid` 的单元格没有指针响应；没有按下 `scale 0.97`；
    没有 `Disabled` 的 `opacity 0.32`。
  - **Focus Ring 缺失（`UI-DEF-22`）**：3.1.1 规定「首选项与键盘高亮项为 `15sp / 500`」——
    字重区分已经由 `P1.05.01` 的 `emphasised` 字段承载，但**背景与描边**（`state.selected.bg` + `1dp state.selected.stroke`）没有。
    3.4 的**状态优先级** `Disabled > Active > Focus Ring > Hover > Default`、
    以及「键盘高亮与鼠标悬停同时存在时**以键盘高亮为准**」**均无实现载体**。
    用户在按 `Tab` 移动高亮时，看到的只是字重变化，没有滑动的高亮框（`P0.08.01` 已把弹簧接上，但没有人驱动它）。
  - **顶级标杆对标解析**：Linear 与 Raycast 的列表高亮是「键盘优先、鼠标让位」：鼠标悬停只改变指针与被悬停项，
    **不抢走键盘高亮**；两者同时存在时视觉上明确区分（键盘高亮有描边，悬停只有淡底）。
    这是「操作排布空间语法」维度里最容易被忽略、也最影响手感的一条。

- **设计规范与参数定义 (Design Tokens & Spatial Specs)**：
  - **五态规格（严格照 `features.md:1060-1066`）**：

    | 状态 | 背景 | 描边 | 文本 | 备注 |
    |---|---|---|---|---|
    | `Default` | 透明 | 无 | `text.primary`；序号 `text.annotation` | 序号用 `P1.05.01` 的 0.55 有效 α |
    | `Hover` | `state.hover` | 无 | 不变 | `border-radius 8dp` |
    | `Active`（按下） | `state.pressed` | 无 | 不变 | 整体 `scale 0.97`，时长 **60ms** |
    | `Focus Ring` | `state.selected.bg` | `1dp state.selected.stroke` | `15sp / 500` | 首选项与键盘高亮项 |
    | `Disabled` | 透明 | 无 | `opacity 0.32` | 无 hover 响应 |

  - **状态优先级**（`features.md:1068`）：`Disabled > Active > Focus Ring > Hover > Default`。
    **实现方式**：单元格只接收一个 `state: int`（0..4），优先级**在 Rust 侧解析**，
    而不是在 `.slint` 里写四层嵌套 `if`——后者会让「键盘高亮与悬停同时存在」的取舍散落在视图层。
  - **键盘优先规则**：`hit == Some(i)` 且 `i != highlight` 时，单元格 `i` 为 `Hover`；
    `i == highlight` 时，**无论指针是否在其上**，一律为 `Focus Ring`。
  - **按下动效（60ms）**：`spring/transition.rs` 新增常量 `PRESS_S: f32 = 0.060` 与
    `CubicBezier::EASE_OUT = (0.0, 0.0, 0.58, 1.0)`。**说明**：3.4 只规定「60ms」，未规定曲线；
    选用 `ease-out` 是因为按下反馈必须**立刻可见**、释放回弹可以稍慢，标准 `ease-out` 正是这个形状。
    这是本卡唯一的自由取值，需在验收记录中写明。
  - **指针形状**：本形态**不改变指针形状**。`SurfaceBackend` 没有光标 API，且作为 IME 浮层，
    改变指针会与应用争夺视觉所有权。3.4 的「`Disabled` 时鼠标指针为 `default`」因此**天然满足**；
    同样的理由也让 `Hover` 不改变指针。该结论写入验收记录。
  - **`Elevated` 景深层**（主文档 §2.1）：`Focus Ring` 的 `state.selected.bg` + `1dp` 描边是三层景深中
    `Elevated` 层的唯一落点，`P0.06.01` 完成材质底座后，这一层才第一次真正出现。
  - **性能红线**：按下动效的 60ms 内，损伤区仅限该单元格（≤ 156×36 dp）；动效结束后
    `AnimationSet::is_animating()` 必须回到 `false`。

- **重构源码落地实现 (Code Delivery)**：

  **① 状态解析放在 Rust 侧**（`surface.rs`）：

  ```rust
  /// The state one candidate cell is drawn in (3.4).
  ///
  /// The priority order is fixed by the design: Disabled beats Active beats the keyboard
  /// highlight beats hover beats default. Resolving it here rather than in the component is
  /// what keeps the one rule that matters -- the keyboard highlight wins over the pointer --
  /// in a single expression that a test can pin, instead of spread over four nested
  /// conditionals in the view.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum CellState {
      Default,
      Hover,
      FocusRing,
      Active,
      Disabled,
  }

  /// The state of cell `index`, given what the keyboard and the pointer are doing.
  ///
  /// `highlight` is the keyboard's candidate and always wins: a user driving the window from
  /// the keyboard must not lose their place because the pointer happens to rest somewhere
  /// else. The pointer's cell is drawn as hovered instead, so both are visible at once.
  pub fn cell_state(
      index: u16,
      highlight: Option<u16>,
      hovered: Option<u16>,
      pressed: Option<u16>,
      disabled: bool,
  ) -> CellState {
      if disabled {
          CellState::Disabled
      } else if pressed == Some(index) {
          CellState::Active
      } else if highlight == Some(index) {
          CellState::FocusRing
      } else if hovered == Some(index) {
          CellState::Hover
      } else {
          CellState::Default
      }
  }
  ```

  **② 单元格接收状态**（`candidate.slint`）：

  ```slint
  export component CandidateCell inherits Rectangle {
      in property <CandidateItem> item;
      in property <bool> show-annotation: true;
      // The state the cell is drawn in, resolved by `src/surface.rs` from the keyboard
      // highlight, the pointer and the press. 0 default, 1 hover, 2 focus ring, 3 active,
      // 4 disabled. The component does not decide it: the priority order is a product rule,
      // not a drawing rule.
      in property <int> state: 0;
      // The press scale, driven by a 60ms ease-out transition in Rust. Written as a plain
      // property rather than an `animate` block, because Slint's own animation clock would
      // keep a timer alive while the window is at rest.
      in property <float> press-scale: 1.0;

      height: CandidateMetrics.cell-height;
      border-radius: CandidateMetrics.cell-radius;
      background: root.state == 1 ? Theme.state-hover
                : root.state == 2 ? Theme.state-selected-bg
                : root.state == 3 ? Theme.state-pressed
                : transparent;
      border-width: root.state == 2 ? 1px : 0px;
      border-color: Theme.state-selected-stroke;
      opacity: root.state == 4 ? 0.32 : 1.0;
      // The scale is about the cell's own centre: a press that grew from a corner would look
      // like the cell was sliding rather than being pushed.
      transform-origin: self.width / 2, self.height / 2;
      scale-x: root.press-scale;
      scale-y: root.press-scale;

      HorizontalLayout { /* unchanged from P1.05.01 */ }
  }
  ```

  **③ 高亮框由弹簧驱动**（`P0.08.01` 已建立 `highlight-*` 属性，本卡负责**驱动**它）。
  `surface.rs` 中，键盘高亮变化时把**目标单元格矩形**（容器坐标 dp）交给 `HighlightAnim::retarget()`，
  再由 `render()` 每帧写入 `highlight-x/y/w/h` 与 `highlight-visible`。
  高亮框的可见性规则：`highlight.is_some() && !disabled`。

  **④ 按下动效**（`spring/transition.rs` 新增 + `surface.rs` 驱动）：

  ```rust
  /// The press feedback duration of 3.4, in seconds.
  pub const PRESS_S: f32 = 0.060;

  /// The scale a pressed cell shrinks to (3.4: `scale 0.97`).
  pub const PRESS_SCALE: f32 = 0.97;
  ```

  ```rust
  impl CubicBezier {
      /// `ease-out`: the press feedback curve.
      ///
      /// 3.4 fixes the press duration at 60ms and leaves the curve open. `ease-out` is the
      /// shape a press wants -- the shrink is immediate so the finger feels answered, and the
      /// release eases back rather than snapping.
      pub const EASE_OUT: Self = Self { x1: 0.0, y1: 0.0, x2: 0.58, y2: 1.0 };
  }
  ```

- **逐步落地实施步骤**：
  1. **优先级先行**：实现 `cell_state()` 纯函数，配 8 个单测覆盖全部优先级组合
     （键盘与悬停同格、同项不同格、按下与键盘同格、Disabled 压过一切、全空）。
  2. **单元格状态**：`CandidateCell` 增加 `state` 与 `press-scale`；按上面的代码接五态底色与描边。
  3. **高亮框驱动**：键盘高亮变化时 `retarget`；确认飞行途中再次移动**保留速度**（`spring/highlight.rs` 的既有测试在接线后仍通过）。
  4. **按下动效**：`PRESS_S` / `PRESS_SCALE` / `EASE_OUT` 落地；按下时 `TimedTransition` 从 1.0 走向 0.97，释放时反向。
  5. **键盘优先验证**：把指针停在第 3 格、用 `Tab` 把高亮移到第 3 格 → 该格必须是 `Focus Ring`（有描边），不是 `Hover`。
  6. **Disabled 验证**：构造一个 `Disabled` 帧（无候选可用/词库不可用），确认全部单元格 `opacity 0.32` 且**不响应悬停**。
  7. **静止验证**：动效结束后 `AnimationSet::is_animating()` 为 `false`，`committed_frames()` 停止增长。

- **验收标准 (DoD)**：
  - [ ] **微观隐形细节审查**：`Hover` 只有淡底、`Focus Ring` 有淡底 **+ 1dp 描边**，两者一眼可分；
    按下时单元格**可见地缩小**到 0.97 并在 60ms 内到位。
  - [ ] **反 AI 模板风审查**：不存在「悬停就变成强调色实心块」的粗暴反馈；无高饱和色块；`Disabled` 不是「变灰消失」而是 `opacity 0.32` 可辨。
  - [ ] **排布与工学**：状态优先级严格为 `Disabled > Active > Focus Ring > Hover > Default`（8 个单测钉死）；
    键盘高亮**永远**压过鼠标悬停。
  - [ ] **物理微交互**：高亮框在格间**滑动**（弹簧，181ms）而非跳变；飞行中改向保留速度；
    按下动效 60ms 内结束且结束后窗口回到静止。
  - [ ] **指针**：窗口不改变指针形状（`SurfaceBackend` 无光标 API），`Disabled` 的指针要求天然满足；
    结论写入验收记录。
  - [ ] **性能**：按下动效的损伤区 ≤ 单个单元格矩形；`idle_redraw_count = 0` 在动效结束后成立。

---

#### 任务 ID：`UI-OPT-P2.07.01` 降级态、加载占位与状态反馈

- **基本属性**：
  - 绑定缺陷编号：`UI-DEF-19, UI-DEF-20`
  - 所属组件域：域07 状态反馈
  - 优先级与复杂度：`P2` | 中 | 预估工时: 3 人天
  - 前置依赖：`UI-OPT-P1.02.01`（状态簇）、`UI-OPT-P0.02.02`
  - 关键路径：否
  - 并行通道：Track C 控件与动态反馈
  - 代码落地锚点：`crates/ime-ui/ui/candidate.slint`（`CandidateWindow`、新增 `Placeholder`）、`crates/ime-ui/src/adapter/frame.rs`、`crates/ime-ui/src/surface.rs`
  - 当前状态：`[ ] 待优化`

- **原实现弊端与微观质感缺失剖析**：
  - **状态反馈缺失（`UI-DEF-19`）**：`StatusStrip` 的五个字段中只有 `mode_label` 有落点（`P1.02.01` 把它作为回退文本），
    其余四个（`full_width` / `punctuation_full` / `has_user_dict_hit` / `readonly`）在 `P1.02.01` 中接成了图标，
    但 **3.6 规定的三类降级文本**仍无落点：
    - 词库缺失/损坏 → Header 显示 `词库不可用`（`text.secondary`），报 `dict/unavailable`；
    - 输入达上限（64 字节）→ 状态区显示 `已达上限`；
    - 候选溢出（> 5 页）→ 状态区显示 `5/5+`，报 `ui/candidate/overflow`。
  - **加载占位缺失（`UI-DEF-20`）**：`features.md:1070` 明确要求「**词库加载期**候选框显示『词库加载中…』单行占位，
    高度 `34dp`，`opacity 0.6`，不可交互」。**零实现**。当前行为：加载期要么窗口不出现，要么出现一个空壳
    （`P1.05.01` 之后是「一个 220dp 宽、只有 Header 的空面板」）——用户看到的是一个**坏掉的候选框**，
    而不是「正在准备」。
  - **翻页指示缺失（12 维之「微型滚动条」重映射）**：本形态没有滚动条，长内容的唯一机制是翻页。
    当前 `PageState { current, total, page_size }` 在 UI 侧**零消费**。用户翻到第 2 页后不知道还有几页。
  - **顶级标杆对标解析**：macOS 候选窗在无候选时显示一个紧凑的拼音条；微软拼音在加载期显示「正在加载词库…」。
    共同点是：**降级态不是空白，而是一句明确的话**。空白让人以为程序坏了。

- **设计规范与参数定义 (Design Tokens & Spatial Specs)**：
  - **占位态（`Placeholder`）**：

    | 场景 | 触发 | 内容 | 样式 | 高度 |
    |---|---|---|---|---|
    | 词库加载中 | `dict_loading = true` | `词库加载中…` | `14sp / 400`，`text.secondary`，`opacity 0.6` | `34dp` |
    | 词库不可用 | `dict_unavailable = true` | `词库不可用` | `14sp / 400`，`text.secondary` | `34dp` |
    | 输入达上限 | `input_full = true` | `已达上限` | `11sp / 500`，`text.annotation`，0.55 有效 α | 状态簇左侧 |

  - **不可交互**：占位态下 `hit_map` 为空、输入区域仍为容器矩形（面板可见但无热点）。
    这与 3.4 的 `Disabled` 一致，也是「不可交互」的实现方式。
  - **翻页指示器（12 维之「微型滚动条」重映射）**：
    - 位置：状态簇左侧，`11sp / 500`，`text.annotation`。
    - 文本：`{current}/{total}`；溢出时为 `5/5+`。
    - **静止隐退、操作渐入**：静息有效 α = **0.35**；翻页后 900ms 内升到 **1.0**，随后在 **120ms** 内回落到 0.35。
      **不是**完全消失——信息不丢失，只是退到背景里（对标 Linear / Raycast 的滚动条隐退逻辑）。
    - 只有一页（`total == 1`）时**整块不绘制**。
  - **溢出诊断**：`total > max_pages(5)` 时，`adapter` 记录 `ui/candidate/overflow`（每次会话只记一次，不刷屏）。
  - **动效约束**：翻页指示器的渐入渐出是**有界一次性动效**，必须在 900ms + 120ms 内收敛，
    之后 `AnimationSet::is_animating()` 回到 `false`（`BUDGET-CPU-01` 的前提）。

- **重构源码落地实现 (Code Delivery)**：

  ```slint
  // The preedit-only placeholder (3.4): a one-line panel shown while the dictionary is
  // loading or when it is unusable.
  //
  // 3.1.1 gives a candidate-less window the compressed 28dp header; the placeholder uses the
  // full 34dp strip instead, because it carries a sentence rather than a pinyin string and a
  // compressed strip would crowd it. It is not interactive -- the window's hit map is empty
  // while it is up, so a click passes straight through.
  export component Placeholder inherits Rectangle {
      in property <string> message: "";
      in property <bool> muted: true;

      height: CandidateMetrics.header-height;
      background: transparent;

      WindowText {
          x: CandidateMetrics.header-padding-h - CandidateMetrics.header-inset;
          width: parent.width - 2 * CandidateMetrics.header-padding-h;
          height: parent.height;
          text: root.message;
          font-size: CandidateMetrics.font-size-header;
          font-weight: 400;
          color: Theme.text-secondary;
          // 3.4 asks for 0.6 on the loading placeholder. The unusable-dictionary line is
          // drawn at full strength: it is a state the user may need to act on, not a wait.
          opacity: root.muted ? 0.6 : 1.0;
          horizontal-alignment: left;
          overflow: elide;
      }
  }
  ```

  ```slint
      // -- status line (3.5 / 3.6) --------------------------------------------
      // The page indicator is this window's equivalent of a scrollbar, and it retreats the
      // same way one does: it rests dim and comes forward while the user is paging, then
      // settles back. It never disappears -- a user who has paged away from the first page
      // needs to know how many pages there are, and a hidden indicator is not a subtle one,
      // it is a missing one.
      in property <int> page-current: 1;
      in property <int> page-total: 1;
      in property <bool> page-overflow: false;
      in property <float> page-indicator-opacity: 0.35;
      in property <bool> input-full: false;

      // -- dictionary state (3.6) ---------------------------------------------
      in property <bool> dict-loading: false;
      in property <bool> dict-unavailable: false;
  ```

  适配器侧（`adapter/frame.rs`）：

  ```rust
  /// What the header's status line shows, resolved from the frame.
  ///
  /// A plain value so the whole mapping is testable without a window: the caller asserts on
  /// the strings, and only the final write needs a live component.
  pub struct StatusLine {
      /// `"{current}/{total}"`, or `"{max}/{max}+"` when the candidate list runs past the
      /// display limit (3.1.3).
      pub page_text: String,
      /// Whether the page indicator is drawn at all.
      pub page_visible: bool,
      /// Whether the input has reached its byte limit (3.6).
      pub input_full: bool,
  }

  /// Builds the status line.
  ///
  /// The overflow form is what tells a user there is more than the five pages the window is
  /// willing to show; it is not an error and is never drawn as one.
  pub fn status_line(page: PageState, max_pages: u8, input_full: bool) -> StatusLine {
      let overflow = page.total > max_pages;
      let total = if overflow { max_pages } else { page.total.max(1) };
      let current = page.current.clamp(1, total);
      StatusLine {
          page_text: if overflow {
              format!("{total}/{total}+")
          } else {
              format!("{current}/{total}")
          },
          page_visible: total > 1,
          input_full,
      }
  }
  ```

  翻页指示器的渐入渐出（`surface.rs`，复用 `spring/transition.rs` 的 `TimedTransition`）：

  ```rust
  /// The time the page indicator stays raised after a page turn, in seconds.
  ///
  /// Long enough to read "2/5" at a glance, short enough that it has settled long before a
  /// user who stopped paging would notice it. The window must be at rest for
  /// `BUDGET-CPU-01`, so this is a bounded one-shot rather than a lingering animation.
  const PAGE_INDICATOR_HOLD_S: f32 = 0.9;

  /// The resting opacity of the page indicator.
  ///
  /// Dim rather than hidden: the indicator carries information a user who has paged needs,
  /// and information that vanishes is not subtle, it is absent.
  const PAGE_INDICATOR_REST: f32 = 0.35;
  ```

- **逐步落地实施步骤**：
  1. **占位组件**：加 `Placeholder`；在 `CandidateWindow` 中按 `dict-loading` / `dict-unavailable` 切换显示，
     此时 `grid-rows = 0`、`hit_map` 为空。
  2. **状态行**：`status_line()` 纯函数 + 6 个单测（单页、多页、恰好 5 页、6 页溢出、`current` 越界、`total = 0`）。
  3. **翻页指示器**：接进状态簇左侧；`page-visible == false` 时**整块不绘制**（不占宽度）。
  4. **渐入渐出**：翻页后把 `page-indicator-opacity` 用 `TimedTransition` 升到 1.0，保持 900ms，再 120ms 回落到 0.35。
  5. **已达上限**：`input_full` 接进状态行；在输入到 64 字节时显示 `已达上限`。
  6. **溢出诊断**：`total > 5` 时记 `ui/candidate/overflow`，**每次会话只记一次**。
  7. **走查**：四个降级场景各截一张图（加载中、词库不可用、只读、溢出 5/5+），确认文案正确、`opacity` 正确、不可交互。

- **验收标准 (DoD)**：
  - [ ] **微观隐形细节审查**：加载占位是 `34dp` 单行、`opacity 0.6`、**不可交互**；
    翻页指示器静息 α 0.35、翻页后升到 1.0 再回落，**不完全消失**；单页时**不占宽度**。
  - [ ] **反 AI 模板风审查**：降级态是**一句明确的话**而不是空白或转圈动画；无「菊花 loading」；无红色错误条。
  - [ ] **排布与工学**：状态行不挤压拼音串的可用宽度（`P1.02.01` 的预算需扣除状态行宽度）；
    `5/5+` 与 `2/5` 的宽度差**不引起拼音串重排**（状态行占固定宽度槽位）。
  - [ ] **物理微交互**：翻页指示器的渐入渐出在 900ms + 120ms 内收敛，之后窗口回到静止（`committed_frames()` 停止增长）。
  - [ ] **降级闭环**：`dict/unavailable`、`ui/candidate/overflow` 两条诊断在对应场景下被记录；
    只读模式的状态簇锁图标可见（`P1.02.01`）。
  - [ ] `StatusStrip` 五个字段与 3.6 的三类降级文本**全部**有落点。

---

#### 任务 ID：`UI-OPT-P2.01.01` 主题 crossfade 与光学微调

- **基本属性**：
  - 绑定缺陷编号：`UI-DEF-25`
  - 所属组件域：域01 全局基线 / 域06 浮层系统
  - 优先级与复杂度：`P2` | 中 | 预估工时: 2.5 人天
  - 前置依赖：`UI-OPT-P0.01.01`、`UI-OPT-P0.01.02`、`UI-OPT-P0.02.02`
  - 关键路径：否
  - 并行通道：Track C 控件与动态反馈
  - 代码落地锚点：`crates/ime-ui/ui/theme.slint`、`crates/ime-ui/src/theme.rs`、`crates/ime-ui/src/surface.rs`
  - 当前状态：`[ ] 待优化`

- **原实现弊端与微观质感缺失剖析**：
  - **切换硬闪（`UI-DEF-25`）**：`theme.rs:467-471` 的 `apply()` 直写 `set_dark` / `set_accent` / `set_base_alpha`
    三个属性；`theme.slint` 的 18 个 Token 全部是 `out property` 表达式（`dark ? A : B`），
    属性一变**立即重算**。深浅色切换因此是**瞬时硬闪**。3.2 明确要求「切换延迟 ≤ 300ms（**含 120ms crossfade 过渡**）」。
  - **架构张力**：`theme.rs:6-9` 与 `:19-26` 的模块文档把「只写三个属性」当成一条设计成就——
    「三个写入点正是让主题切换成为属性更新而非重建的原因」。本卡不能破坏这一点，
    否则会把一次 120ms 的过渡变成每帧 20 次属性写入 + 全量重绘。
  - **光学微调的遗留项**：`P0.01.02` 建立了 `WindowText` 的 0.5dp 下沉，但那是**估算值**；
    真正的校准需要在 1x 与 2x 下分别截图比对，确认 CJK 字面框中心与容器中线重合。
  - **顶级标杆对标解析**：macOS 的深浅色切换是**整体渐变**，不会出现「面板已经变白、文字还是黑的」这种半拍。
    Linear 与 Craft 的主题切换同样是 120ms 级别的颜色过渡。

- **设计规范与参数定义 (Design Tokens & Spatial Specs)**：
  - **crossfade 时长**：`120ms`，`ease-in-out`（`spring/transition.rs:25` 已有 `CROSSFADE_S` 与 `CubicBezier::EASE_IN_OUT`）。
  - **实现路径（v1.4 已核实：方案 A 可行）**：
    把 `Theme.dark: bool` 换成 `Theme.darkness: float`（`0.0` = 亮色，`1.0` = 暗色），
    每个 Token 从 `dark ? A : B` 改为 `A_dark.mix(A_light, darkness)`。Rust 侧仍只写**三个**属性
    （`darkness` / `accent` / `base_alpha`），crossfade 只是让 `darkness` 在 120ms 内从 0 走到 1。
    `dark` 仍作为派生属性保留给需要布尔判定的地方（`out property <bool> dark: darkness >= 0.5;`）。
  - **Slint API 核实结论（对锁定版本 `slint 1.13.1` 的源码核实，非推测）**：
    - `BuiltinFunction::ColorMix` 存在于 `i-slint-compiler-1.13.1`（`expression_tree.rs:79,235`），
      签名 `(Color, Color, Float32) -> Color`，在 `lookup.rs:1029` 注册为**颜色类型的成员方法**，**无 feature 门控**。
    - 形式是 **`<颜色>.mix(<另一颜色>, <factor>)`**，**不是自由函数**。
    - **`factor` 作用于接收者**：`a.mix(b, f)` = `f × a + (1 − f) × b`
      （`i-slint-core-1.13.1/graphics/color.rs:271`）。因此 `darkness = 1.0` 要得到暗色，**暗色必须是接收者**。
    - 实现是 Sass 的 `mix()` 算法，**按 alpha 加权**（`color.rs:277-300`），不是朴素逐通道插值。
      对本项目影响可忽略：同一 Token 明暗两列的 alpha 差异 ≤ 0.05；`surface-stroke`（0.06 / 0.10）
      这类跨 alpha 的 Token 在过渡中段会略偏向低 alpha 一侧，120ms 内不可感知。
  - **备选方案 B（已排除，保留备查）**：在 Rust 侧逐帧插值全部 20 个 Token 并逐帧写入。
    它把每次切换变成 120ms × 帧率的属性写入，并让 `theme.rs` 的「三写入」纪律失效。
    方案 A 已确认可行，本卡**不采用** B。
  - **光学微调量**：`WindowText` 的 `optical-nudge` 默认 `0.5dp`；本卡在 1x 与 2x 下各截图一次，
    用「CJK 字面框中心与容器中线偏差」判定是否需要改为 `0dp` / `1dp`。容差 **±0.5dp**。
  - **切换延迟上限**：从属性写入到最后一帧提交 ≤ **300ms**（3.2）。
  - **静止**：crossfade 结束后 `AnimationSet::is_animating()` 为 `false`。

- **重构源码落地实现 (Code Delivery)**：

  **① `theme.slint` 的 Token 改为可插值**：

  ```slint
  // How dark the palette is: 0.0 is the light scheme, 1.0 the dark one. Written by
  // `src/theme.rs`; `true` is the documented default for a system that reports no preference,
  // which is why the default here is 1.0.
  //
  // A fraction rather than a flag, so a scheme switch is a 120ms ramp of one property instead
  // of an instant swap of eighteen. Every token below mixes between the two columns, which is
  // what makes the switch a crossfade rather than a flash -- and it keeps the write side at
  // three properties, which is what stops the switch from becoming a per-frame rebuild.
  in property <float> darkness: 1.0;

  // The boolean form, for the few places that need to branch rather than blend.
  out property <bool> dark: darkness >= 0.5;

  out property <color> surface-base:
      #1C1C1E.mix(#FFFFFF, darkness);

  out property <color> surface-stroke:
      rgba(255,255,255,0.10).mix(rgba(0,0,0,0.06), darkness);

  out property <color> text-primary:
      #F2F2F7.mix(#1C1C1E, darkness);

  out property <color> text-secondary:
      rgba(242,242,247,0.62).mix(rgba(28,28,30,0.60), darkness);

  out property <color> text-annotation:
      rgba(242,242,247,0.48).mix(rgba(28,28,30,0.45), darkness);

  out property <color> text-separator:
      rgba(242,242,247,0.40).mix(rgba(28,28,30,0.35), darkness);

  out property <color> state-hover:
      rgba(242,242,247,0.08).mix(rgba(28,28,30,0.06), darkness);

  out property <color> state-pressed:
      rgba(242,242,247,0.14).mix(rgba(28,28,30,0.12), darkness);

  out property <color> separator:
      rgba(242,242,247,0.10).mix(rgba(28,28,30,0.08), darkness);

  out property <color> shadow-inner:
      rgba(0,0,0,0.35).mix(rgba(0,0,0,0.08), darkness);

  out property <color> shadow-outer:
      rgba(0,0,0,0.42).mix(rgba(0,0,0,0.16), darkness);

  out property <color> status-dot-idle:
      rgba(242,242,247,0.35).mix(rgba(28,28,30,0.30), darkness);

  // The selected-cell tokens carry an accent-derived alpha as well as a mixed colour, so they
  // mix the alpha fraction too: 0.18 in the dark column, 0.14 in the light one.
  out property <color> state-selected-bg:
      accent.with-alpha(0.14 + 0.04 * darkness);
  out property <color> state-selected-stroke:
      accent.with-alpha(0.50 + 0.05 * darkness);
  ```

  > **参数顺序是本卡最容易写错的一处**：`mix` 的 `factor` 作用于**接收者**，
  > 所以「暗色在前、亮色在后」。写成 `#FFFFFF.mix(#1C1C1E, darkness)` 会得到**完全相反**的明暗映射——
  > 而它在 `darkness` 为 0 或 1 时看起来是对的（端点交换），只有过渡中段才暴露。
  > 因此本卡的验收要求**必须包含一张过渡中段的截图**，不能只测两个端点。

  **② `theme.rs` 的写入侧保持三属性**：

  ```rust
  /// The scheme as a fraction, which is what the crossfade ramps.
  ///
  /// `true` is 1.0 and `false` is 0.0, so a switch with no animation is simply a write of the
  /// endpoint. The intermediate values only exist while a transition is running.
  pub fn darkness(scheme: ColorScheme) -> f32 {
      match scheme {
          ColorScheme::Dark => 1.0,
          ColorScheme::Light => 0.0,
      }
  }
  ```

  `ThemeSink::set_dark(bool)` 改为 `set_darkness(f32)`；`apply()` 仍只写三个属性，
  crossfade 由 `surface.rs` 驱动（`TimedTransition` 从当前值走向目标值，每帧写一次）。

  **③ `surface.rs` 的 crossfade 驱动**：

  ```rust
  /// Applies a theme request, ramping the scheme rather than swapping it.
  ///
  /// The accent and the base alpha are written immediately: they are a user's explicit choice
  /// and there is nothing to cross-fade them with. The scheme is ramped, because a dark-to-light
  /// switch that snapped would flash the whole panel for one frame -- which is the one thing
  /// 3.2's 120ms budget exists to prevent.
  fn apply_theme(&mut self, spec: ThemeSpec) {
      let target = theme::darkness(spec.scheme);
      self.darkness.retarget(target);
      // ... accent and base alpha written straight through ...
  }
  ```

- **逐步落地实施步骤**：
  1. **API 已核实，直接实现**：`slint 1.13.1` 提供 `Color::mix` 成员方法（见上），本卡按方案 A 实现；
     若编译期报 `mix` 未知，则说明 `Cargo.lock` 的 Slint 版本被改动，此时停下来核对版本而不是改写法。
  2. **Token 改造**：`theme.slint` 的 18 个 Token 按上面的形式改写；`dark` 降为派生属性。
  3. **写入侧**：`ThemeSink::set_darkness(f32)`；`apply()` 仍只写三处。
  4. **门禁同步**：`P0.01.01` 的 `check-ui-spec.py` 需要理解 `mix(...)` 形式——
     比对逻辑改为「取 `mix` 的两个端点色与 3.2 表的亮/暗两列比对」，端点顺序必须与脚本约定一致。
  5. **crossfade 驱动**：`surface.rs` 用 `TimedTransition`（120ms，`EASE_IN_OUT`）驱动 `darkness`。
  6. **光学校准**：1x 与 2x 各截一张图，量 CJK 字面框中心与容器中线的偏差；偏差 > 0.5dp 时调整 `optical-nudge`。
  7. **延迟验证**：从属性写入到最后一帧提交的耗时 ≤ 300ms（3.2）；crossfade 结束后窗口静止。
  8. **走查**：暗 → 亮 → 暗各切一次，逐帧确认**不存在**「面板已变色、文字未变色」的中间帧。

- **验收标准 (DoD)**：
  - [ ] **微观隐形细节审查**：深浅色切换是 **120ms 渐变**，逐帧无「面板与文字不同步」的中间态；无硬闪。
    **必须附一张 `darkness ≈ 0.5` 的过渡中段截图**——`mix` 的参数顺序写反时两个端点仍然正确，
    只有中段能暴露（见上「参数顺序是本卡最容易写错的一处」）。
  - [ ] **反 AI 模板风审查**：不存在「切换即重建组件」的实现；`theme.rs` 的写入点仍为**三个**。
  - [ ] **排布与工学**：切换过程中**不重置任何进行中的输入会话**（`AGENTS.md` 禁止事项 23）；
    窗口尺寸与位置不变。
  - [ ] **物理微交互**：crossfade 结束后 `AnimationSet::is_animating()` 为 `false`；`committed_frames()` 停止增长；
    总延迟 ≤ 300ms。
  - [ ] **光学校准**：1x 与 2x 下 CJK 字面框中心与容器中线偏差 ≤ **0.5dp**；`optical-nudge` 的最终值写入验收记录。
  - [ ] `check-ui-spec.py` 的端点比对通过；两份调色板（`.slint` 与 `theme.rs`）仍逐项一致。

---

## 交付状态摘要

| 状态 | 内容 |
|---|---|
| **已完成** | Phase 3 全部 4 张任务卡原子展开；主文档 §4 的 28 项缺陷全部归位 |
| **进行中** | 无 |
| **待办** | 无（本分片为最后一波） |
| **需用户裁决** | **无。** 四项裁决已由用户裁定并落地（见主文档 §0.1）：① 3.1.4 改规范列显式例外 ② 单元格圆角改 `4dp` ③ `text.annotation` 采「取代」读法 ④ Slint `mix` API 已核实可用，`P2.01.01` 走方案 A |
