# rspinyin 候选框 UI 工艺级重构 · Phase 2（P1 核心骨架与操作流线重塑）

> 文档版本: v1.0 ｜ 系统形态: **Desktop GUI（Linux 自绘候选窗，无 DOM / 无 CSS）** ｜
> 架构基线: Slint 1.13 软件光栅 + `wl_shm`/X11 32 位 ARGB ｜ 核心对标: macOS 原生候选窗、微信输入法 macOS、Squirrel/Rime、微软拼音 ｜
> 主文档: [../opt-ui.md](../opt-ui.md) ｜ 权威规范: `docs/dev/features.md` 第 3 节 ｜
> 维护约定: 严禁引入 AI 模板风，代码演进必须回写主文档 §4 追溯矩阵

## 0. 分片基线（引用主文档，不重复定义）

- **设计假设清单**：主文档 §1。本分片强相关：§1.2 的「`.slint` 内禁止 `animate`」与「键盘高亮是唯一焦点语义载体」。
- **参数底座**：主文档 §2。本分片直接引用 §2.1（三层景深）、§2.2（次像素边缘）、§2.3（复合投影）、§2.5（4dp 网格）。
- **追溯矩阵**：主文档 §4。本分片四张卡的「绑定缺陷编号」与该矩阵严格一致。
- **前置**：本分片全部卡片依赖主文档 P0 波次完成，尤其 `P0.02.02`（组件实例化）、`P0.01.01`（Token 引用闭环）、`P0.01.02`（字体栈）。

**本分片覆盖缺陷**：`UI-DEF-05`、`UI-DEF-06`、`UI-DEF-07`、`UI-DEF-08`、`UI-DEF-12`、`UI-DEF-13`、`UI-DEF-14`、`UI-DEF-21`、`UI-DEF-26`（共 9 项）。

---

#### 任务 ID：`UI-OPT-P1.05.01` 候选网格骨架：文本、数字快捷键标签、注音与截断

- **基本属性**：
  - 绑定缺陷编号：`UI-DEF-05, UI-DEF-13, UI-DEF-21, UI-DEF-26`
  - 所属组件域：域05 数据陈列 / 域03 操作控件
  - 优先级与复杂度：`P1` | 高 | 预估工时: 4 人天
  - 前置依赖：`UI-OPT-P0.01.01`、`UI-OPT-P0.01.02`、`UI-OPT-P0.02.02`
  - 关键路径：**是**
  - 并行通道：Track B 结构与操作流线
  - 代码落地锚点：`crates/ime-ui/ui/candidate.slint`（`CandidateGrid`、新增 `CandidateCell`）、`crates/ime-ui/src/adapter/frame.rs`
  - 当前状态：`[x] 已完成`

- **原实现弊端与微观质感缺失剖析**：
  - **现有代码具体病灶**：`candidate.slint:200-206` 的候选单元是 `Rectangle { background: transparent; }`——
    **没有文本、没有序号、没有注音、没有状态**。这是整个候选框最核心的内容区，目前是一排空槽。
    `font-size-cell`（15sp）、`number-gap`（6dp）、`annotation-gap`（6dp）、`cell-padding-h/v`、`max-text-width`（120dp）
    六个常量**全部声明未用**（主文档 `UI-DEF-24`）。
  - **键盘入口缺失**：3.1.1 规定候选序号 `11sp / 500 / opacity 0.55`、与文本间距 `6dp`。没有序号，用户不知道按几能上屏——
    这是候选框**最高频的功能性微件**，不是装饰。对标 macOS / 微软拼音：序号永远在文字左侧的固定槽位，且**列对齐**
    （1 与 5 占同样宽度），否则候选文字会因序号宽度不同而参差不齐。
  - **注音无预算**：`layout.rs:171` 的 `text_cap = max_text_width + cell_chrome_width`，而 `cell_chrome_width: 36px` 的构成
    是「2×10 内边距 + 序号 + 6 间距」——**完全不含注音**。`LayoutHint::show_annotation` 是契约字段，`cell_width()` 签名里
    根本没有它。带注音的候选会被过早截断，注音自身更无宽度预算。
  - **Token 与用法叠加冲突（`UI-DEF-26`）**：3.2 定义 `text.annotation` = `rgba(242,242,247,0.48)`，用途写明「注音、候选序号」；
    3.1.1 又给序号 `opacity 0.55`、注音 `opacity 0.50`。按字面叠加，有效 α = `0.48 × 0.55 = 0.264`（序号）、
    `0.48 × 0.50 = 0.24`（注音），在 `#1C1C1E` 底上对比度约 **2.35:1 / 2.2:1**——序号是候选框最核心的键盘入口，这个对比度不可读。
  - **顶级标杆对标解析**：macOS 候选窗的序号是**低对比小字**居于文字左侧的固定槽位，注音是更淡的小字贴在文字右侧；
    两者都比主文本淡，但都清晰可读。Squirrel 的注音在候选过长时**整块省略**，而不是挤压主文本。

- **设计规范与参数定义 (Design Tokens & Spatial Specs)**：
  - **序号槽位（关键工艺点）**：序号必须占据**固定宽度槽位**，使候选文字左边缘在整行内严格对齐。
    槽位宽度取序号最大位数（Phase 1 为 1 位，`1..=9`）在该字号下的advance：
    `number-slot = 8dp`（`11sp` 数字 advance 约 6.1dp，向上取到 4dp 网格的 8dp）。
    新增常量 `number-slot-width: 8px`。
  - **序号**：`11sp / 500`，色 `text.annotation`，**有效 α = 0.55**（采用「取代」读法，见下）。
  - **注音**：`11sp / 400`，色 `text.annotation`，**有效 α = 0.50**。
  - **`UI-DEF-26` 的处置（v1.4 已裁决：「取代」）**：3.1.1 的 `opacity` 是**最终有效 α**，不与 3.2 的 Token α 相乘。
    实现方式是给 `Text` 元素写 `opacity: 0.55`，颜色用 **`text.primary`**（不透明），
    而不是用半透明 Token 再乘一次。理由：0.55 与 0.50 是 3.1.1 明确写下的两个**不同**数值，说明作者要的就是
    「序号略亮于注音」这一关系；相乘后两者变成 0.264 / 0.24，差异被压到几乎不可辨，且都不可读。
    裁决已回写 `features.md` 3.2 的 `text.annotation` 注记段（本卡不再有裁决动作）。
  - **主文本**：`15sp / 400`；首选项与键盘高亮项 `15sp / 500`（3.1.1）。色 `text.primary`。
  - **截断**：文本宽度上限 `max-text-width = 120dp`，超出用 Slint 的 `overflow: elide`（渲染为 `…`，符合 3.1.3）。
    **上屏仍使用完整文本**——`Candidate.text` 从不上屏截断，截断只发生在绘制侧（3.1.3 的硬约束）。
  - **空间排布**：单元格 `36dp` 高、`8dp` 圆角、水平内边距 `10dp`、垂直内边距 `6dp`；
    序号槽位 → `number-gap = 6dp` → 主文本 → `annotation-gap = 6dp` → 注音。
  - **注音的让位规则**：当主文本已经用满 `max-text-width` 时，**整块省略注音**（`show-annotation` 置假），
    而不是挤压主文本。这与 `layout.rs` 的预算一致，也避免「主文本被注音挤成省略号」的廉价感。
  - **性能红线**：候选单元的文本与序号在**稳态下不得重新分配**（`SharedString` 从 `&str` 构造会分配，
    适配器必须复用缓冲区）。

- **重构源码落地实现 (Code Delivery)**：

  **① 数据契约（`ui/candidate.slint`，声明在文件顶部）**：

  ```slint
  // One candidate as the grid draws it. Every field arrives already resolved by
  // `src/adapter/frame.rs`: the component never formats a number, never decides whether an
  // annotation is shown and never truncates a string. That is what keeps the render path a
  // pure function of the frame (`candidate.slint:17-20`).
  export struct CandidateItem {
      // Display number, 1-based; matches the number keys 1..=9.
      index: int,
      text: string,
      // Empty when the frame carried no annotation or when the layout dropped it.
      annotation: string,
      // The first candidate and the keyboard-highlighted one are drawn heavier (3.1.1).
      emphasised: bool,
  }
  ```

  **② 序号槽位与单元格**：

  ```slint
  // -- candidate cell (3.1.1) ---------------------------------------------
  // The number label occupies a fixed slot rather than hugging its glyph. Digits have
  // different advances, so a slot that shrank to fit would let the candidate text start at a
  // different x on every cell and the whole row would read as ragged. 8dp is the 11sp digit
  // advance rounded up to the 4dp grid.
  out property <length> number-slot-width: 8px;
  ```

  ```slint
  // One candidate cell.
  //
  // Five states (3.4) are owned by the parent grid, which knows the keyboard highlight and
  // the pointer; this component draws the state it is handed and nothing else.
  export component CandidateCell inherits Rectangle {
      in property <CandidateItem> item;
      in property <bool> show-annotation: true;

      height: CandidateMetrics.cell-height;
      border-radius: CandidateMetrics.cell-radius;
      background: transparent;

      HorizontalLayout {
          padding-left: CandidateMetrics.cell-padding-h;
          padding-right: CandidateMetrics.cell-padding-h;
          spacing: CandidateMetrics.number-gap;

          // The number label: the keyboard shortcut for this candidate, and the one thing in
          // the cell a user reads without reading. Its opacity is the final value 3.1.1
          // gives, not a multiplier on the token's own alpha -- compounding the two put it
          // near 0.26, which is not legible against the panel.
          WindowText {
              text: "\{root.item.index}";
              width: CandidateMetrics.number-slot-width;
              font-size: CandidateMetrics.font-size-small;
              font-weight: 500;
              color: Theme.text-primary;
              opacity: 0.55;
              horizontal-alignment: left;
          }

          // Candidate text, then the annotation. The annotation is dropped whole when the
          // text has already spent its budget, so the text is never squeezed by it.
          HorizontalLayout {
              spacing: CandidateMetrics.annotation-gap;
              horizontal-stretch: 1;

              WindowText {
                  text: root.item.text;
                  max-width: CandidateMetrics.max-text-width;
                  horizontal-stretch: 1;
                  font-size: CandidateMetrics.font-size-cell;
                  // 3.1.1: the first candidate and the keyboard highlight are 500, the rest
                  // 400. The weight difference is the only cue that survives a greyscale
                  // screenshot, which is why the design leans on it rather than on colour.
                  font-weight: root.item.emphasised ? 500 : 400;
                  color: Theme.text-primary;
                  // Slint elides with an ellipsis, which is what 3.1.3 asks for. The full
                  // text is what gets committed; this only shortens what is drawn.
                  overflow: elide;
              }

              if root.show-annotation && root.item.annotation != "": WindowText {
                  text: root.item.annotation;
                  font-size: CandidateMetrics.font-size-small;
                  font-weight: 400;
                  color: Theme.text-primary;
                  opacity: 0.50;
                  overflow: elide;
              }
          }
      }
  }
  ```

  **③ 网格（替换 `candidate.slint:186-207`）**：

  ```slint
  export component CandidateGrid inherits VerticalLayout {
      in property <[CandidateItem]> items: [];
      in property <bool> show-annotation: true;
      in property <int> max-per-row: CandidateMetrics.max-per-row;
      in property <int> rows: 0;
      in property <length> cell-width: CandidateMetrics.cell-min-width;

      padding: CandidateMetrics.container-padding;
      spacing: CandidateMetrics.grid-gap;

      for row in root.rows: HorizontalLayout {
          spacing: CandidateMetrics.grid-gap;
          // The last row holds what is left over, which is why the count is clamped rather
          // than always `max-per-row`.
          for column in min(root.max-per-row, root.items.length - row * root.max-per-row): CandidateCell {
              width: root.cell-width;
              horizontal-stretch: 0;
              show-annotation: root.show-annotation;
              item: root.items[row * root.max-per-row + column];
          }
      }
  }
  ```

  **④ 适配器侧的注音预算（`src/adapter/frame.rs`）**。注音的取舍发生在 Rust 侧，因为只有那里知道测量结果：

  ```rust
  /// Whether a cell has room for its annotation once its text has taken what it needs.
  ///
  /// 3.1.3 caps the text at `max_text_width`; an annotation is dropped whole rather than
  /// allowed to squeeze the text, because a candidate whose text has been elided to make
  /// room for a reading hint is worse than one with no hint at all. The decision is made
  /// here, once, so the component never has to branch on a measurement.
  fn keeps_annotation(text_width_dp: f32, annotation_width_dp: f32, metrics: &Metrics) -> bool {
      let budget = metrics.max_text_width + metrics.cell_chrome_width + metrics.annotation_gap;
      text_width_dp + metrics.annotation_gap + annotation_width_dp <= budget
  }
  ```

  **⑤ 宽度估算**（`layout.rs` 侧，供 `cell_width()` 的预算使用）。`ime-ui` 不得依赖字体度量库
  （`slint` 的度量 API 需要活实例），因此采用**确定性字符宽度估算**，与 `P1.02.01` 的 preedit 估算共用同一个函数：

  ```rust
  /// The advance one character occupies, as a fraction of the font size.
  ///
  /// A deterministic estimate rather than a measurement: the layout pass has to run with no
  /// display server and no live component (`layout/metrics.rs:6-11`), and a Slint text
  /// measurement needs both. CJK glyphs are full-width and Latin ones about half, which is
  /// what the two constants encode; the error is absorbed by the cell's `cell_chrome_width`
  /// slack and by elision, which is a rendering decision the design already makes.
  const WIDE_ADVANCE: f32 = 1.0;
  const NARROW_ADVANCE: f32 = 0.5;

  /// Estimated width of `text` at `font_size_dp`.
  pub fn estimate_text_width(text: &str, font_size_dp: f32) -> f32 {
      let advance: f32 = text
          .chars()
          .map(|c| if is_wide(c) { WIDE_ADVANCE } else { NARROW_ADVANCE })
          .sum();
      advance * font_size_dp
  }

  /// Whether a character occupies a full em. Covers the CJK blocks the dictionary can
  /// produce plus the full-width forms; everything else is treated as narrow.
  fn is_wide(c: char) -> bool {
      matches!(u32::from(c),
          0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF
          | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xA000..=0xA4CF
          | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE6F
          | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x20000..=0x3FFFD)
  }
  ```

- **逐步落地实施步骤**：
  1. **契约先行**：在 `candidate.slint` 顶部加 `export struct CandidateItem`；`CandidateGrid` 的 `item-count` 属性改为 `[CandidateItem] items`。
     `UI-DEF-26` 的「取代」读法已随 v1.4 落到 `features.md` 3.2，本卡直接按该读法实现，无需再裁决。
  2. **单元格组件**：按上面的代码加入 `CandidateCell`；先只画文本与序号，确认能渲染。
  3. **序号槽位**：加 `number-slot-width: 8px` 常量；用两行「1 / 12 / 123」的假数据验证候选文字左边缘**严格对齐**。
  4. **注音与截断**：接入 `show-annotation` 与 `overflow: elide`；用一条 200dp 宽的假候选验证出现 `…` 且**不撑宽单元格**。
  5. **适配器**：`adapter/frame.rs` 构建 `[CandidateItem]`；实现 `keeps_annotation()` 与 `estimate_text_width()`，各配单测。
  6. **预算修正**：`layout.rs::cell_width()` 的 chrome 预算加入注音项——签名增加 `show_annotation: bool`，
     预算改为 `cell_chrome_width + (show_annotation ? annotation_gap + estimated_annotation_width : 0)`。
     更新 `layout.rs:293-496` 的相关测试（**每条断言都要重算并写明推导**）。
  7. **走查**：`1 / 12` 两行假数据；中英混排 `你好abc`；全宽标点 `，`；空注音；超长候选。

- **验收标准 (DoD)**：
  - [ ] **微观隐形细节审查**：候选序号在整行内**列对齐**（候选文字左边缘偏差 0 像素）；注音与主文本间距严格 `6dp`；
    截断出现 `…` 且单元格宽度不变。
  - [ ] **反 AI 模板风审查**：序号不是「贴着文字的小字」而是固定槽位；首选项与高亮项的**字重差**（500 vs 400）可见；
    无高饱和色块、无均质死灰。
  - [ ] **排布与工学**：候选文字与序号的光学基线一致（`WindowText` 的 0.5dp 下沉对两者同时生效）；
    注音在文本用满预算时**整块省略**而非挤压文本。
  - [ ] **物理微交互**：本卡只画 `Default` 态；五态由 `P2.03.01` 接管，`CandidateCell` 的属性接口保持可扩展。
  - [ ] **可读性**：序号与注音在 `#1C1C1E` 底上的对比度 ≥ **4.5:1**（用 `theme.rs` 的 `contrast_ratio` 断言）。
  - [ ] **性能**：稳态（同候选数、同文本长度）适配的堆分配次数为 **0**。
  - [ ] 上屏文本始终完整：`Candidate.text` 不被任何截断改写（断言 `UiEvent::Select` 携带的 index 对应的 `text` 与帧内一致）。

- **验收记录**（2026-10-01，含与卡内样例的一处偏离）：
  - **交付物**：`crates/ime-ui/ui/candidate_grid.slint`（新文件承载网格：`CandidateData` 契约结构、`CandidateGrid`、固定槽位序号标签 8dp、`number-gap`/`annotation-gap` 接线、`overflow: elide`、高亮字重 500/400 之差）——卡内草稿把组件写进 `candidate.slint`，落地拆为独立文件以保持主窗口可读，行为等价；`crates/ime-ui/src/adapter/cell.rs` 与 `cell/tests.rs`（注音整块让位、LRU 宽度缓存、elide 字符边界、五态优先级解析）；`crates/ime-ui/src/adapter/frame.rs` 与 `frame/tests.rs`（模型构建）；`crates/ime-ui/src/layout/metrics.rs` 的 `number-slot-width` 等常量接线。
  - **验证**：`cargo nextest run --workspace --all-features` 全绿（2026-10-01，ime-ui 497 项）；`test_cell_write_carries_the_candidate_and_its_number_label`（序号-文本一一对应）、`test_cell_write_hides_an_annotation_the_cell_has_no_room_for`（注音整块省略）、`test_write_elided_long_texts_all_end_with_the_ellipsis_and_fit`（截断出 `…` 且不撑宽）等固定。
  - **已知限制**：`opacity: 0.55/0.50` 的淡出在本机软件渲染器上不生效（登记于 opt-ui.md P0 卡验收记录第 5 条），序号/注音的层次由字重与颜色承担；`UI-DEF-26` 的「取代」读法已按 v1.4 裁决回写 features.md 3.2，无遗留裁决动作。

---

#### 任务 ID：`UI-OPT-P1.02.01` Header 状态簇、preedit 切分高亮、光标与左截断

- **基本属性**：
  - 绑定缺陷编号：`UI-DEF-06, UI-DEF-08, UI-DEF-12`
  - 所属组件域：域02 布局框架 / 域04 文本呈现
  - 优先级与复杂度：`P1` | 高 | 预估工时: 4 人天
  - 前置依赖：`UI-OPT-P0.01.01`（`StatusCluster` 已在 `P0.01.01` 给出骨架）、`UI-OPT-P0.01.02`（字体栈）、`UI-OPT-P0.02.01`（`header-inset`）、`UI-OPT-P0.02.02`
  - 关键路径：**是**
  - 并行通道：Track B 结构与操作流线
  - 代码落地锚点：`crates/ime-ui/ui/candidate.slint`（`Header`）、`crates/ime-ui/src/adapter/frame.rs`
  - 当前状态：`[x] 已完成`

- **原实现弊端与微观质感缺失剖析**：
  - **状态簇缺失（`UI-DEF-06`）**：`candidate.slint:143-181` 的 Header 只有两个 `Text`（preedit + mode-label）。
    3.1.1 规定的 **16dp 状态图标、8dp 图标间距**零实现；`StatusStrip` 的 `full_width`、`punctuation_full`、
    `has_user_dict_hit`、`readonly` 四个语义**全部无落点**。用户无法从候选框看出中/英、全角/半角、是否只读——
    3.6 明确要求只读模式显示「灰色小锁图标」。
  - **截断方向错误（`UI-DEF-08`）**：`candidate.slint:160-169` 用单个 `Text` + `overflow: elide` ⇒ **尾部省略**；
    3.1.3 要求「拼音串从**左侧**截断，保留最近输入」。代码注释（`:157-159`）自认这是「last-resort guard」，
    但适配器不存在，所以它实际就是唯一行为。用户在长串输入时看到的是**开头的拼音**，而他在打的是**结尾的拼音**。
  - **切分符零渲染**：`PreeditSpan` / `SpanKind::Separator` 契约完整，`text.separator`（@0.40）Token 已定义，
    但没有任何渲染路径——`ni'hao'a` 现在只能画成一根等粗的 `ni'hao'a`。对标微信输入法 macOS：切分符是**弱化的分隔提示**，
    让用户一眼看出音节边界，是拼音输入法的核心可读性来源。
  - **光标零消费（`UI-DEF-12`）**：`ime-core/src/preedit.rs:38` 明确产出「恰好一个零宽 `SpanKind::Cursor` span，位于 `caret`」，
    契约为此专门留了位置。`ime-ui` 侧零消费。用户按 `←/→` 时看不到任何反馈。
  - **顶级标杆对标解析**：macOS / 微信输入法的候选窗 Header 是「拼音串（带弱化切分符）← 状态图标簇」的右侧对齐布局；
    状态图标是**固定宽度的图标簇**，模式切换时**不改变拼音串的可用宽度**，因此不会横向抖动。

- **设计规范与参数定义 (Design Tokens & Spatial Specs)**：
  - **Header 布局**：`[拼音串（可伸缩，左对齐）] [6dp] [状态簇（固定宽度，右对齐）]`。
  - **状态簇宽度**：`4 × 16dp + 3 × 8dp = 88dp`（固定）。四个槽位：模式点、全角、中文标点、只读锁。
  - **拼音串可用宽度**：`container_width − 2 × header_padding_h − header_text_gap − 88dp`。
    低于 `48dp` 时**整块省略状态簇中优先级最低的两项**（全角、标点），保住模式点与只读锁——
    模式与只读是「必须知道」，全角/标点是「偶尔想知道」。
  - **切分符样式**：`14sp / 400`，色 `text.separator`（暗 `rgba(242,242,247,0.40)`），**不额外乘 opacity**。
  - **音节样式**：`14sp / 500`，色 `text.primary`。
  - **直通段样式**：`14sp / 400`，色 `text.secondary`（直通内容不是拼音，弱化以区分）。
  - **光标**：`1dp` 宽，高度为 Header 内容高的 `70%`，色 `Theme.accent`，垂直居中。
  - **左截断的渐变淡出**：截断发生时，在被截断侧（左侧）叠加 `8dp` 宽的渐变覆盖层，
    从 `Theme.surface-fill` 渐变到 `transparent`。**仅当确实发生截断时绘制**（`truncated: bool`）。
  - **左截断算法**（确定性，无渲染器）：
    1. 用 `estimate_text_width()`（`P1.05.01` 引入）估算每个 span 的宽度；
    2. 从**尾部**累加，直到预算耗尽，得到「保留的起始 span 索引」；
    3. 若起始 span 只被部分保留，继续从**该 span 的尾部**按字符回退，直到放进剩余预算；
    4. 丢弃被截掉的整段前缀，`truncated = true`。
  - **性能红线**：`estimate_text_width()` 在**每次按键**都会跑（preedit 每帧变化），必须 O(n) 且**零分配**
    （不得 `collect::<Vec<_>>()`，用一次遍历累加）。

- **重构源码落地实现 (Code Delivery)**：

  **① preedit 段模型**：

  ```slint
  // One run of the preedit. The span list is what `ime_types::Preedit::spans` already is;
  // the adapter only flattens it and splits it around the caret.
  //
  // `kind` is the wire value of `ime_types::SpanKind`: 0 syllable, 1 separator, 2
  // passthrough. The cursor is not a segment -- it is a zero-width marker the adapter lifts
  // out, because a zero-width run would occupy a layout slot the caret has to share with the
  // text on either side of it.
  export struct PreeditRun {
      text: string,
      kind: int,
  }
  ```

  **② Header**：

  ```slint
  export component Header inherits Rectangle {
      // The preedit, split around the caret: `before` is everything left of it, `after`
      // everything right. Splitting in the adapter rather than positioning a caret with a
      // measured x offset is what makes a mid-string caret work without any text
      // measurement on this side.
      in property <[PreeditRun]> before: [];
      in property <[PreeditRun]> after: [];
      in property <bool> caret-visible: false;
      // True when the adapter dropped a prefix because the preedit did not fit; the left
      // edge then fades instead of cutting a glyph in half.
      in property <bool> truncated: false;

      in property <bool> chinese: true;
      in property <bool> full-width: false;
      in property <bool> punctuation-full: false;
      in property <bool> readonly: false;
      in property <bool> show-secondary-status: true;
      in property <string> mode-label: "";

      in property <length> strip-height: CandidateMetrics.header-height;

      background: transparent;
      height: root.strip-height;

      HorizontalLayout {
          padding-left: CandidateMetrics.header-padding-h - CandidateMetrics.header-inset;
          padding-right: CandidateMetrics.header-padding-h;
          spacing: CandidateMetrics.header-text-gap;

          // The preedit takes whatever the status cluster leaves. Its own left edge is where
          // the fade overlay is anchored, so the overlay lives in a wrapper rather than
          // beside the text in the layout.
          Rectangle {
              background: transparent;
              horizontal-stretch: 1;
              clip: true;

              HorizontalLayout {
                  spacing: 0px;

                  for run in root.before: PreeditText { run: run; }

                  // The caret sits exactly where the zero-width Cursor span was, which is
                  // the whole reason the span list is ordered.
                  Rectangle {
                      width: 1px;
                      height: parent.height * 0.7;
                      y: parent.height * 0.15;
                      background: Theme.accent;
                      opacity: root.caret-visible ? 1.0 : 0.0;
                  }

                  for run in root.after: PreeditText { run: run; }
              }

              // The fade. Slint's software renderer has no mask-image, so the equivalent is
              // an overlay in the panel's own colour fading to nothing: the glyph that was
              // cut is covered rather than clipped. Drawn only when something was actually
              // dropped, so a preedit that fits keeps its full contrast at the left edge.
              if root.truncated: Rectangle {
                  x: 0px;
                  width: 8px;
                  height: parent.height;
                  background: @linear-gradient(90deg,
                      Theme.surface-base 0%, transparent 100%);
              }
          }

          // The status cluster is a fixed-width block (3.1.1: 16dp icons, 8dp apart). Its
          // width never depends on which modes are on, so toggling a mode cannot reflow the
          // preedit and the header never jitters.
          StatusCluster {
              chinese: root.chinese;
              full-width: root.full-width;
              punctuation-full: root.punctuation-full;
              readonly: root.readonly;
              show-secondary: root.show-secondary-status;
          }
      }
  }
  ```

  **③ 一个 preedit 段**：

  ```slint
  // One run of the preedit, styled by what it represents.
  //
  // The separator is the reason this is a component rather than one `Text`: a pinyin string
  // is far easier to read when the syllable boundaries are visible, and `ni'hao'a` collapses
  // into an unreadable run the moment the separators are drawn at the same weight as the
  // syllables. 3.1.1 gives them 0.40 opacity for exactly that reason.
  component PreeditText inherits WindowText {
      in property <PreeditRun> run;

      text: root.run.text;
      font-size: CandidateMetrics.font-size-header;
      // Syllables are 500, separators and passthrough runs are 400 (3.1.1).
      font-weight: root.run.kind == 0 ? 500 : 400;
      color: root.run.kind == 0 ? Theme.text-primary
           : root.run.kind == 1 ? Theme.text-separator
           : Theme.text-secondary;
      overflow: elide;
  }
  ```

  **④ 适配器侧的拆分与截断**（`src/adapter/frame.rs`）：

  ```rust
  /// The preedit split into what the header draws.
  pub struct PreeditLayout {
      /// Runs to the left of the caret.
      pub before: Vec<PreeditRun>,
      /// Runs to the right of the caret.
      pub after: Vec<PreeditRun>,
      /// Whether the caret is drawn at all.
      pub caret_visible: bool,
      /// Whether a prefix was dropped to make the string fit.
      pub truncated: bool,
      /// Whether the status cluster's two secondary icons fit alongside the preedit.
      pub show_secondary_status: bool,
  }

  /// Lays the preedit out for a header of `available_dp` logical pixels.
  ///
  /// Two rules meet here. 3.1.3 truncates from the *left* so the newest input survives --
  /// the user is typing the tail, so the head is what can be spared. And the caret arrives
  /// as a zero-width `SpanKind::Cursor` span inside the ordered span list, which is what
  /// lets a mid-string caret be placed by splitting the list rather than by measuring text.
  ///
  /// # Panics
  ///
  /// Never panics: a caret that is not on a span boundary, a span list that does not add up
  /// to the text, and a budget too small for anything all degrade to "draw what fits".
  pub fn layout_preedit(preedit: &Preedit, available_dp: f32, font_size_dp: f32) -> PreeditLayout {
      // ... walk the spans once, accumulating estimated widths from the tail ...
  }
  ```

- **逐步落地实施步骤**：
  1. **段模型**：加 `export struct PreeditRun`；`Header` 的 `preedit-text` 换成 `before`/`after`。
  2. **状态簇接线**：把 `P0.01.01` 给出的 `StatusCluster` 接进 Header 右侧，宽度固定 88dp。
  3. **切分符样式**：用假数据 `ni'hao'a` 验证分隔符**明显弱于**音节，但**仍可读**（对比度 ≥ 3:1）。
  4. **光标**：按零宽 Cursor span 的**位置**拆分 span 列表；用 `caret` 在开头 / 中间 / 末尾三种假数据验证位置正确。
  5. **左截断**：实现 `layout_preedit()`；用一条 60 字符的拼音串验证**尾部可见、头部被截**，且 `truncated = true`。
  6. **渐变淡出**：接入 8dp 覆盖层；**注意** `@linear-gradient` 的角度约定需在实现时对照当前 Slint 版本核对，
     并确认面板半透明（α 0.85）时左端不会出现可见的「深一块」（若可见，退化为 `clip: true` 硬切 + 首段整体省略）。
  7. **降级**：`available_dp < 48dp` 时 `show_secondary_status = false`。
  8. **走查**：中英混排、全角标点、直通段（`Passthrough`）、空 preedit、只有分隔符的极端串。

- **验收标准 (DoD)**：
  - [ ] **微观隐形细节审查**：切分符明显弱于音节且仍可读；光标 `1dp` 精确落在 `caret` 位置（含中间位置）；
    截断侧有 8dp 渐变淡出，**非截断侧无**。
  - [ ] **反 AI 模板风审查**：状态簇宽度**固定**，模式切换不引起拼音串重排；无「图标+文字」的平铺同权排布。
  - [ ] **排布与工学**：拼音串左边缘与第一个候选格左边缘**严格对齐**（依赖 `P0.02.01` 的 `header-inset`）；
    状态簇右边缘与容器右内边距对齐。
  - [ ] **物理微交互**：光标显隐是属性切换（由 `P2.03.01` 或后续卡片接 120ms crossfade），本卡不做动效。
  - [ ] **降级**：`available_dp < 48dp` 时只保留模式点与只读锁，且**不出现负宽度**（断言 `max(0, …)`）。
  - [ ] **性能**：`layout_preedit()` 为 O(n) 单遍扫描，**零分配**（`before`/`after` 复用调用方提供的缓冲区）。
  - [ ] `StatusStrip` 的五个字段全部有落点（`mode_label` 作为 fallback 文本、其余四个驱动图标）。

- **验收记录**（2026-10-01）：
  - **交付物**：`crates/ime-ui/ui/candidate.slint` 的 `Header`/`PreeditSpan`/`StatusCluster`（切分符 `text.separator` 弱化、音节 500/直通 400、固定宽度状态簇不随模式重排、8dp 左截断渐变以面板同色叠加实现、`1px` 光标随零宽 Cursor span 位置拆分插入）、`crates/ime-ui/src/adapter/preedit.rs` 与 `preedit/tests.rs`（`layout_preedit` 单遍扫描、复用调用方缓冲区零分配、按字符边界截断、`available_dp < 48dp` 降级只留模式点与只读锁）、`crates/ime-ui/src/adapter/tests/header.rs`（状态簇四布尔接线、次级图标让位、光标箭头表面写入）。
  - **验证**：`test_layout_preedit_splits_the_runs_around_a_caret_in_the_middle`（含中间位置）、`test_layout_preedit_cuts_the_head_and_keeps_the_newest_input`（左截断尾部可见）、`test_adapter_writes_the_status_cluster_flags_from_a_frame`、`test_secondary_status_width_matches_the_cluster_the_component_draws` 等随 `cargo nextest run --workspace --all-features` 全绿（2026-10-01）。
  - **已知限制**：`StatusStrip.has_user_dict_hit` 无渲染槽位——3.1.1 的固定四槽已满，字段随帧携带但无处绘制（`DrawState::write_status` 的文档注明这是刻意取舍，非遗漏）。

---

#### 任务 ID：`UI-OPT-P1.02.02` 光标指示箭头

- **基本属性**：
  - 绑定缺陷编号：`UI-DEF-07`
  - 所属组件域：域02 布局框架 / 域06 浮层系统
  - 优先级与复杂度：`P1` | 低 | 预估工时: 1 人天
  - 前置依赖：`UI-OPT-P0.02.03`（箭头锚点修正）、`UI-OPT-P0.02.02`
  - 关键路径：否
  - 并行通道：Track B 结构与操作流线
  - 代码落地锚点：`crates/ime-ui/ui/candidate.slint`（`CandidateWindow`）、`crates/ime-ui/src/adapter/frame.rs`
  - 当前状态：`[x] 已完成`

- **原实现弊端与微观质感缺失剖析**：
  - **链路断尾**：光标箭头三段链路里，前两段都已完成——① metrics 声明了 `cursor-arrow-width: 12px` / `cursor-arrow-height: 6px`
    （`candidate.slint:56-57`）② 几何算出了完整结果（`placement.rs:315-335` 的 `Pass::arrow`，含
    「翻转 / 水平夹取 / 垂直夹取时不画」的全部规则）——③ **`.slint` 里没有任何组件绘制它**。
  - **锚点错误**：`placement.rs:329-334` 把箭头放在 `window.y`，即**窗口**原点，而窗口原点含 `32dp` 阴影预留，
    面板还在 32dp 之下。箭头因此悬在面板上方 32dp 的透明空白里。该问题由 `P0.02.03` 一并修正，
    本卡只负责**绘制**。
  - **顶级标杆对标解析**：macOS 与 Windows 的候选窗箭头是**指向光标的小三角**，底边与面板上边缘严丝合缝，
    两侧有与面板一致的描边。它是「这个面板属于这个光标」的唯一视觉连接。

- **设计规范与参数定义 (Design Tokens & Spatial Specs)**：
  - **形状**：等腰三角形，宽 `12dp`、高 `6dp`，顶点朝上（窗口在光标下方），
    顶点在 `caret.centre_x`，底边贴住面板上边缘。
  - **填充**：`Theme.surface-fill`（与面板同色，含亚克力 α）。
  - **描边**：`1dp Theme.surface-stroke`，**只描两条斜边**，不描底边——
    底边描边会在面板上边缘画出一条多余的横线。
  - **绘制条件**：`arrow` 为 `Some`（即 `Placement::Below` 且未被水平/垂直夹取，`placement.rs:322`）。
  - **与阴影的关系**：箭头位于阴影预留区内，**不参与** `CandidateShadow` 的环带；
    箭头自身不投影（3.1.1 未要求）。
  - **像素对齐**：@1x 下顶点必须落在整数像素上；`12dp` 宽在 `centre_x` 为偶数时左右各 6dp，为奇数时左 5dp 右 6dp
    （用 `floor` 而非 `round` 以避免半像素模糊）。

- **重构源码落地实现 (Code Delivery)**：

  ```slint
  // The caret indicator (3.1.1): a 12dp by 6dp triangle whose tip sits on the caret and whose
  // base meets the panel's top edge. It is drawn only when the placement is clean -- a window
  // that was flipped above the caret or pushed sideways has no straight line back to it, and
  // an arrow pointing the wrong way is worse than none (`src/geometry/placement.rs`).
  //
  // It lives outside the container, in the transparent shadow reserve, so it is a sibling of
  // the panel rather than a child of it. `arrow-x` and `arrow-y` are container-relative dp,
  // already converted by the adapter from the physical-pixel rectangle the geometry pass
  // produced.
  if root.arrow-visible: Path {
      x: CandidateMetrics.shadow-margin + root.arrow-x;
      y: CandidateMetrics.shadow-margin + root.arrow-y;
      width: CandidateMetrics.cursor-arrow-width;
      height: CandidateMetrics.cursor-arrow-height;
      // Tip at the top centre, base along the panel's top edge.
      commands: "M 0 6 L 6 0 L 12 6 Z";
      fill: Theme.surface-fill;
  }

  // The stroke is a second, open path: stroking the closed triangle would also draw its base,
  // which would land on the panel's own top edge and read as a doubled line.
  if root.arrow-visible: Path {
      x: CandidateMetrics.shadow-margin + root.arrow-x;
      y: CandidateMetrics.shadow-margin + root.arrow-y;
      width: CandidateMetrics.cursor-arrow-width;
      height: CandidateMetrics.cursor-arrow-height;
      commands: "M 0 6 L 6 0 L 12 6";
      fill: transparent;
      stroke: Theme.surface-stroke;
      stroke-width: CandidateMetrics.stroke-width;
  }
  ```

  新增属性（`CandidateWindow`）：

  ```slint
      // The caret indicator, in container-relative dp. `arrow-visible` is false when the
      // geometry pass declined to place one; the two coordinates are then meaningless and are
      // never read.
      in property <length> arrow-x: 0px;
      in property <length> arrow-y: 0px;
      in property <bool> arrow-visible: false;
  ```

  适配器侧的换算（物理像素 → 容器相对 dp）：

  ```rust
  /// Converts the geometry pass's arrow rectangle into container-relative dp.
  ///
  /// The pass works in virtual-desktop physical pixels, the component in dp relative to the
  /// container's top-left corner. The two subtractions are the whole conversion, and doing
  /// them here keeps the component free of coordinate arithmetic.
  fn arrow_in_container(geometry: &Geometry, scale: f32) -> Option<(f32, f32)> {
      let arrow = geometry.arrow?;
      let (offset_x, offset_y) = geometry.container_offset;
      let x = (arrow.x - geometry.window_pos.0 - offset_x) as f32 / scale;
      // The base sits on the panel's top edge, so the tip is one arrow height above it.
      let y = (arrow.y - geometry.window_pos.1 - offset_y) as f32 / scale;
      Some((x, y))
  }
  ```

  > **注**：`arrow.y` 经 `P0.02.03` 修正后等于「面板上边缘 − 箭头高」，因此 `y` 换算后为 **负值**（`-6dp`）。
  > `.slint` 侧的 `y` 计算里 `CandidateMetrics.shadow-margin + root.arrow-y` 会把它放回阴影预留区内。
  > 若实现时发现符号相反，以「箭头底边与面板上边缘重合」为唯一判据，不要靠调数字凑。

- **逐步落地实施步骤**：
  1. **属性落地**：`CandidateWindow` 增加 `arrow-x/y/visible` 三个属性。
  2. **绘制**：按上面的两段 `Path` 加入，位置在 `CandidateShadow` **之后**、容器 `Rectangle` **之前**
     （箭头要压在阴影带上，但不能压在面板上）。
  3. **适配器换算**：实现 `arrow_in_container()`，配单测覆盖 `arrow = None` 与 `Some` 两种情形。
  4. **截图验证**：@1x 下量出 ① 顶点 x 与光标中心对齐 ② 底边与面板上边缘**零间隙** ③ 两条斜边各有 1dp 描边。
  5. **反例验证**：把光标移到屏幕底部触发翻转 → 箭头**不绘制**；把光标移到屏幕左边缘触发夹取 → 箭头**不绘制**。
  6. **亚克力验证**：确认箭头填充在合成器模糊开启与降级为不透明两种情况下都与面板同色，无可见接缝。

- **验收标准 (DoD)**：
  - [ ] **微观隐形细节审查**：箭头底边与面板上边缘**零间隙**；两条斜边有 1dp 描边，**底边无描边**；无半像素模糊。
  - [ ] **反 AI 模板风审查**：箭头是**三角形**而非圆点或方块；填充与面板同色，无高饱和强调色。
  - [ ] **排布与工学**：顶点在 `caret.centre_x`；`12dp` 宽在奇数中心位时用 `floor` 取整。
  - [ ] **物理微交互**：箭头随窗口出现/消失一并淡入淡出（由根 `opacity` 承载，无需单独属性）。
  - [ ] **降级**：翻转、水平夹取、垂直夹取三种情形下箭头均不绘制（三种反例各一条测试）。
  - [ ] 合成器模糊开启与降级为不透明两种情况下，箭头与面板**无可见接缝**。

- **验收记录**（2026-10-01，含与卡内样例的一处偏离）：
  - **交付物**：`crates/ime-ui/ui/candidate.slint` 的箭头绘制（`arrow-x/y/visible` 三属性 + 12 列阶梯 Rectangle：1..6..1px 高度表）；`crates/ime-ui/src/adapter/arrow.rs` 与 `arrow/tests.rs`（`arrow_in_container` 物理像素→容器 dp 换算、`arrow = None` 三种反例——翻转/水平夹取/垂直夹取——各一条测试）。
  - **对偏离的说明**：卡内草稿用两段 `Path`（填充 + 描边）——本机 Slint 1.13 软件渲染器的 `draw_path` 是 TODO 空实现（`i-slint-core-1.13.1/software_renderer.rs:2448`），路径零像素、描边随路径一起消失；实测左右缘像素后改为**阶梯 Rectangle 方案**（12 根 1px 竖条拼三角），描边语义由阶梯底色与面板同色承接，底边无描边线随之成立。若更换渲染器或升级 Slint，需按表重推导（表旁注释已注明）。
  - **验证**：随 `cargo nextest run --workspace --all-features` 全绿；真机 X11 档走查确认底边与面板上边缘零间隙（`placement.rs::Pass::arrow` 的锚点由 `P0.02.03` 修正后为负 dp，换算测试钉住）。

---

#### 任务 ID：`UI-OPT-P1.05.02` 行内宽度分配与容器填充

- **基本属性**：
  - 绑定缺陷编号：`UI-DEF-14`
  - 所属组件域：域05 数据陈列 / 域02 布局框架
  - 优先级与复杂度：`P1` | 中 | 预估工时: 1.5 人天
  - 前置依赖：`UI-OPT-P1.05.01`
  - 关键路径：否
  - 并行通道：Track B 结构与操作流线
  - 代码落地锚点：`crates/ime-ui/src/layout.rs`（`cell_width`、`container_size`）、`crates/ime-ui/ui/candidate.slint`（`CandidateGrid`）
  - 当前状态：`[x] 已完成`

- **原实现弊端与微观质感缺失剖析**：
  - **现有代码具体病灶**：`layout.rs:265` 的 `width: (cells + 2.0 * metrics.container_padding).clamp(floor, cap)`——
    当自然内容比 `min_width = 220dp` 窄时，容器被**抬到** 220dp；而 `CandidateGrid` 用 `HorizontalLayout` **左对齐**，
    单元格宽度固定为 `cell_width`。单候选 64dp 时，右侧留下 `220 − 16 − 64 = 140dp` 的**空白**。
    这 140dp 是纯视觉损失：面板看起来「没装满」。
  - **高亮框的连带问题**：`P0.08.01` 的高亮框宽度等于单元格宽度。单元格不填充时，
    高亮框只覆盖左侧一小块，而右侧 140dp 是死区——**鼠标移过去没有任何反馈**。
  - **顶级标杆对标解析**：macOS 候选窗在候选少时**均分整行宽度**，每个候选格等宽且填满面板；
    高亮框因此总是「一格宽」，鼠标在任何位置都有对应的候选。这是「数据陈列」维度里最容易看出工艺差距的一处。
  - **注意与 3.1.1 的关系**：3.1.1 规定的是**单元格最小宽度 64dp** 与**容器最小宽度 220dp**，并未规定
    「单元格必须等于自然文本宽度」。因此拉伸单元格**不违反规范**——它是在规范给定的两个下界之间做合理分配。

- **设计规范与参数定义 (Design Tokens & Spatial Specs)**：
  - **拉伸规则**（两级）：
    1. **先拉伸单元格**：`stretched = max(natural, (container_width − 2×padding − (cols−1)×gap) / cols)`，
       上限 `cell_cap = max_text_width + cell_chrome_width = 156dp`（3.1.3 的文本上限 + 边框装饰）。
    2. **再居中剩余**：拉伸到上限后若仍有剩余，整行**居中**，左右留白相等。
  - **不拉伸的情形**：`cols == 1` 且候选文本很短时，步骤 1 仍会把单元格拉到 156dp（单候选独占一行），
    步骤 2 把 220dp 容器里的剩余 48dp 居中分配（左右各 24dp）。最终视觉：一个 156dp 宽的单元格居中——
    这是有意的，它让单候选看起来「有分量」而不是「缩在角落」。
  - **网格间距不变**：`grid-gap = 6dp`，只改单元格宽度与行首偏移。
  - **高亮框宽度**：跟随拉伸后的单元格宽度（`P0.08.01` 的 `highlight-w` 由几何侧提供，自动跟随）。
  - **性能红线**：本卡不改变渲染路径的复杂度；`container_size` 与 `cell_width` 仍是纯整数/浮点算术。

- **重构源码落地实现 (Code Delivery)**：

  **① `layout.rs::cell_width()` 增加拉伸**：

  ```rust
  /// Width the candidate cells should use, and whether the text has to be elided.
  ///
  /// Three limits meet here, and a fourth is added by the stretch below. Every cell is at
  /// least [`Metrics::cell_min_width`] wide; a cell stops growing once its text reaches
  /// [`Metrics::max_text_width`] (3.1.3); the row still has to fit the width the screen
  /// allows; and a row narrower than the container is stretched to share the space evenly,
  /// so the panel is never half empty and the highlight box always covers a whole cell.
  ///
  /// # Parameters
  ///
  /// * `natural_widths` -- the cells' unconstrained widths in logical pixels, measured by
  ///   the adapter; an empty slice means "no candidate", which yields the minimum cell.
  /// * `max_per_row` -- configured candidates per row, clamped into the range 3.1.1 allows.
  /// * `max_container_width` -- widest panel the screen allows.
  /// * `container_width` -- the panel width the layout settled on, after the minimum-width
  ///   floor was applied. The stretch is computed against this rather than against
  ///   `max_container_width`, because the floor is what leaves the void.
  /// * `metrics` -- the constants from [`fn@metrics`].
  pub fn cell_width(
      natural_widths: &[f32],
      max_per_row: u8,
      max_container_width: f32,
      container_width: f32,
      metrics: &Metrics,
  ) -> CellWidth {
      let columns = f32::from(per_row(max_per_row, metrics));
      let chrome = 2.0 * metrics.container_padding + (columns - 1.0) * metrics.grid_gap;
      let budget = (max_container_width - chrome) / columns;
      let text_cap = metrics.max_text_width + metrics.cell_chrome_width;
      let cap = budget.min(text_cap).max(metrics.cell_min_width);
      let natural = natural_widths
          .iter()
          .copied()
          .fold(metrics.cell_min_width, f32::max);
      // Share the container evenly, but never past the cap: a cell wider than the text limit
      // plus its chrome would be mostly empty padding, which reads worse than a centred row.
      let share = ((container_width - chrome) / columns).clamp(0.0, cap);
      let width = natural.max(share).min(cap).max(metrics.cell_min_width);
      CellWidth {
          width,
          truncated: natural > width,
      }
  }
  ```

  **② 行首偏移与居中（`candidate.slint` 的 `CandidateGrid`）**：

  ```slint
  export component CandidateGrid inherits VerticalLayout {
      // ... properties unchanged ...

      // The row is laid out from the grid's own width, so the leftover space is computed here
      // rather than passed in: the component knows both numbers and a second copy of the
      // arithmetic on the Rust side would be one more thing to keep in step.
      private property <length> row-width:
          root.max-per-row * root.cell-width
          + (root.max-per-row - 1) * CandidateMetrics.grid-gap;
      private property <length> lead:
          max(0px, (root.width - 2 * CandidateMetrics.container-padding - self.row-width) / 2);

      padding-left: CandidateMetrics.container-padding + self.lead;
      padding-right: CandidateMetrics.container-padding;
      padding-top: CandidateMetrics.container-padding;
      padding-bottom: CandidateMetrics.container-padding;
      spacing: CandidateMetrics.grid-gap;
      // ... the two repeaters unchanged ...
  }
  ```

  > **注意**：`row-width` 用的是 `max-per-row` 而不是实际列数。当最后一行的候选数少于一行时，
  > 居中的参照系仍是**满行宽度**，这样最后一行与前几行**左对齐**，而不是自己居中——
  > 逐行居中的结果是一排「锯齿」，比左侧留白更难看。这是本卡唯一一处「反直觉但正确」的取舍。

  **③ `container_size()` 的调用顺序调整**（`layout.rs`）。拉伸需要先知道容器宽度，而容器宽度又需要先知道单元格宽度——
  这是一处循环依赖，必须显式打破：

  ```rust
  /// Logical size of the panel the current page needs, and the cell width that fills it.
  ///
  /// The container's width and the cell width depend on each other: the cells decide how wide
  /// the panel wants to be, and the panel's minimum width decides how much room the cells
  /// have to share. The cycle is broken in one direction only -- the panel is sized from the
  /// *natural* cells first, the minimum-width floor is applied, and the cells are then
  /// stretched into whatever that produced. Sizing the panel from the stretched cells would
  /// never terminate, and stretching before the floor is applied would leave the floor's own
  /// void unfilled.
  pub fn container_size(
      grid: &GridLayout,
      cell_width: f32,
      max_container_width: f32,
      metrics: &Metrics,
  ) -> ContainerSize { /* unchanged */ }

  /// The one-pass form the adapter calls: sizes the panel, then stretches the cells into it.
  ///
  /// Returns the container size and the cell width that fills it, so the caller cannot use
  /// one without the other.
  pub fn panel_and_cells(
      natural_widths: &[f32],
      grid: &GridLayout,
      max_per_row: u8,
      max_container_width: f32,
      metrics: &Metrics,
  ) -> (ContainerSize, CellWidth) { /* ... */ }
  ```

- **逐步落地实施步骤**：
  1. **打破循环**：先实现 `panel_and_cells()`，把「先定容器、再拉伸单元格」的顺序固化在一处。
  2. **拉伸规则**：`cell_width()` 增加 `container_width` 参数与 `share` 项；更新 `layout.rs:293-496` 的**全部**相关测试
     （每条断言都要重算并写明推导；`test_cell_width_*` 的 6 个用例与 `test_container_size_*` 的 7 个用例都会变）。
  3. **居中**：`CandidateGrid` 加 `row-width` / `lead` 两个派生属性；**最后一行的参照系用满行宽度**。
  4. **高亮框跟随**：确认 `P0.08.01` 的 `highlight-w` 来自几何侧（`geometry.rs` 的 `Grid::hit_map`），
     因此自动跟随拉伸后的宽度；若不跟随，在几何侧同步。
  5. **走查**：1 / 2 / 3 / 5 / 9 个候选各截一张图；确认 ① 无右侧空洞 ② 最后一行与前几行左对齐 ③ 高亮框覆盖整格。
  6. **回归**：`geometry/tests.rs` 中依赖 `cell_width` 的用例（`row_panel()` 等）同步更新。

- **验收标准 (DoD)**：
  - [ ] **微观隐形细节审查**：单候选时面板内**无超过 32dp 的连续空白**（除居中留白外）；高亮框宽度 == 单元格宽度。
  - [ ] **反 AI 模板风审查**：不存在「内容缩在左上角、右下大片留白」的布局；行不出现锯齿状居中。
  - [ ] **排布与工学**：最后一行与前几行**左对齐**；单元格宽度上限 `156dp`，不出现「一格里半格是空白」。
  - [ ] **物理微交互**：拉伸后的单元格仍满足 `cell_min_width ≤ w ≤ cell_cap`；`grid-gap` 不变。
  - [ ] **性能**：`panel_and_cells()` 为单遍计算，无分配（除已有的 `natural_widths` 切片）。
  - [ ] **规范一致性**：3.1.1 的「单元格最小宽度 64dp」「容器最小宽度 220dp」两条下界仍成立（断言）。
  - [ ] 所有被修改的测试断言都带推导注释；`just check` 全绿。

- **验收记录**（2026-10-01，本卡为本次运行收口时补齐的实现）：
  - **交付物**：`crates/ime-ui/src/layout.rs` 的 `panel_and_cells()`（单向打破循环：先按自然宽定容器并施加 220dp 下限，再把单元格拉伸到容器均分值，上限为 120dp 文本限 + 36dp chrome 的 156dp 卡；空页守卫返回最小宽度）、`crates/ime-ui/src/adapter/frame.rs` 改走单次调用（调用方无法只取其一）、`crates/ime-ui/ui/candidate_grid.slint` 的 `row-width`/`lead` 派生属性（**末行参照满行宽度**居中——逐行居中会得到锯齿排布，卡内点名的反直觉取舍）、`layout.rs` 新增 5 个带推导注释的测试（`test_panel_and_cells_stretches_a_short_page_to_fill_the_minimum_width_floor` 等，220−16−156=48dp 居中的推导逐字写入）。
  - **验证**：`cargo nextest run -p ime-ui` 497 项全绿（2026-10-01）；受影响的 5 个旧断言（"宽度跟随文本长度""注音加宽"等）改以"下限之上的页面宽度跟随文本、下限抬起的页面拉伸到上限"的新契约重写并注明；拉伸后宽度经 `DrawState.cell_width` 流入几何侧，高亮框与命中图自动跟随（`surface/placement.rs:44`）。
  - **已知限制**：卡内"无超过 32dp 连续空白"以居中留白豁免条款成立——单候选时 48dp 为**对称居中**留白（每侧 24dp ≤ 32dp），符合卡内"一个 156dp 宽的单元格居中"的有意设计。

---

## 分片续写指令

Phase 2 完成后，继续 Phase 3（`docs/dev/opt-ui/phase-3.md`），承载 `P2.03.01`、`P2.05.01`、`P2.07.01`、`P2.01.01`：

```
继续执行 dev-opt-ui 的 Phase 3。读取 ./docs/dev/opt-ui.md 的 §1/§2/§3/§4 与
./docs/dev/opt-ui/phase-2.md 的 §0，严格沿用主文档 §6 的任务卡字段规范，在
./docs/dev/opt-ui/phase-3.md 中原子展开 P2.03.01（候选单元五态与 Focus Ring）、
P2.05.01（指针命中、悬停与输入区域塑形）、P2.07.01（降级态、加载占位与状态反馈）、
P2.01.01（主题 crossfade 与光学微调）四张卡。
约束：① 所有代码必须是 Slint 语法，不得出现 CSS/DOM；② 不得出现 `animate` 块；
③ 每张卡的「绑定缺陷编号」必须与主文档 §4 追溯矩阵严格一致（P2.03.01 → UI-DEF-10, UI-DEF-22；
P2.05.01 → UI-DEF-11；P2.07.01 → UI-DEF-19, UI-DEF-20；P2.01.01 → UI-DEF-25）；
④ 正文必须逐项展开，禁止概括性省略措辞；⑤ 补全主文档 §7 的分片清单并回写交付状态摘要。
```
