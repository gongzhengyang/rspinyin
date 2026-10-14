# rspinyin 候选框 UI 工艺级重构 · 主文档（P0 底座）

> 文档版本: v1.0 ｜ 系统形态: **Desktop GUI（Linux 自绘候选窗，无 DOM / 无 CSS / 无 Web 引擎）** ｜
> 架构基线: Slint 1.13 软件光栅（`SoftwareRenderer` + `software-renderer-systemfonts`）+ `wl_shm`/X11 32 位 ARGB ｜
> 核心对标: macOS 原生输入法候选窗、微信输入法 macOS、Squirrel/Rime、fcitx5 自带候选窗、微软拼音（Windows 11）；交互工艺借鉴 Linear / Raycast / Craft ｜
> 细节维度: 12 大隐形质感全覆盖（按本形态逐条重映射，见 §1.3）｜
> 权威规范: `docs/dev/features.md` 第 3 节（本文件与它冲突时**以 features.md 为准**，本文件必须修正）｜
> 维护约定: 严禁引入 AI 模板风，代码演进必须回写 §4 追溯矩阵

---

## 0. 阶段零结论（先读这一段）

**本项目的 UI 层目前不是「做得不好看」，而是「骨架期」：`.slint` 组件从未被实例化。**

阶段零实测（`grep`/`Read` 全量扫描，证据逐条落在 §3）：

| 事实 | 证据 |
|---|---|
| Slint 组件 `CandidateWindow` 在 Rust 侧**零引用** | `grep -rn "CandidateWindow\|include_modules" crates/` 仅命中 `build.rs` 注释 |
| 生产代码**没有任何 `impl UiSurface`** | 仅 `ui_thread/event_loop.rs:168` 与 `ui_thread/tests.rs:122` 两个测试替身 |
| 候选区 `CandidateGrid` 只画**空透明矩形** | `candidate.slint:200-206`，无文本、无序号、无注音、无状态 |
| 9 个 Theme Token、13 个尺寸常量**零引用** | §3 UI-DEF-03 / UI-DEF-24 |
| `hit_map` 已算出但**无消费方**；`set_input_region` 生产路径**零调用** | `geometry.rs:296`、`grep -rn set_input_region crates/` |
| `scripts/check-ui-spec.sh` **不存在**，却被 6 处引用（含 2 处生产代码注释） | §3 UI-DEF-04 |

因此本文件的定位是：**把「已完成的任务卡留下的占位」补齐到工艺级，并把已经做错的材质/Token 决策改对**。
它不是在既有成品上抛光，而是在一份结构正确、注释严谨、但视觉尚未落地的骨架上做真正的工艺落地。

**缺陷总量：28 项**（域01 全局基线 6 项 ｜ 域02 布局框架 7 项 ｜ 域03 操作控件 2 项 ｜ 域04 文本呈现 2 项 ｜
域05 数据陈列 4 项 ｜ 域06 浮层系统 3 项 ｜ 域07 状态反馈 2 项 ｜ 域08 键盘交互 2 项）。
**任务卡：15 张**（P0 7 张 / P1 4 张 / P2 4 张），双向一致，无遗漏、无占位。

> ⚠️ **阶段零另外发现一处高影响几何缺陷（`UI-DEF-27`）**：候选框实际显示在光标下方 **38dp** 处而非规范要求的 6dp，
> 光标箭头悬在面板上方 32dp 的空白里。根因是 `geometry/placement.rs` 把**含阴影预留的窗口原点**当成了光标间隙的落点，
> 而面板还要再向内缩进 `shadow-margin = 32dp`。该缺陷已被现有测试 `geometry/tests.rs:611-626` **断言为正确行为**，
> 因此不会被任何现有门禁捕获。详见 `UI-OPT-P0.02.03`。

### 0.1 裁决记录（2026-09-29，已生效）

四项待裁决事项已由用户裁决并落地。**规范与代码均已同步，本文件的相应卡片不再有"二选一"分支。**

| # | 事项 | 裁决 | 落地位置 |
|---|---|---|---|
| 1 | `features.md` 3.1.4 的 4dp 规则冲突 | **改规范**：保留 4dp 规则，把 8 个非 4 倍数尺寸列为**显式例外清单** | `features.md` v1.4 变更摘要、3.1.4（新增例外表）、`UI-OPT-P0.01.02` |
| 2 | 单元格圆角同心性 | **改为 `4dp`**（`容器 12dp − 内边距 8dp`） | `features.md` 3.1.1、`ui/candidate.slint` 的 `cell-radius`、`layout/metrics.rs` 的断言、`UI-OPT-P0.02.01` |
| 3 | `text.annotation` 的 α 读法 | **「取代」**：3.1.1 的 `opacity` 是最终有效 α，不与 Token α 相乘 | `features.md` 3.2（新增注记段）、`UI-OPT-P1.05.01` |
| 4 | Slint 颜色插值 API | **`P2.01.01` 走方案 A**（单属性 `darkness` 驱动 18 个 Token） | 见下方 API 核实结论、`UI-OPT-P2.01.01` |

**`UI-DEF-04`（Slint 颜色插值）的 API 核实结论**（对锁定版本 `slint 1.13.1` 的源码核实，非推测）：

- `BuiltinFunction::ColorMix` 存在于 `i-slint-compiler-1.13.1`（`expression_tree.rs:79,235`），
  签名为 `(Color, Color, Float32) -> Color`，在 `lookup.rs:1029` 注册为**颜色类型的成员方法** `mix`，
  **无 feature 门控**。
- 调用形式是 **`<颜色>.mix(<另一颜色>, <factor>)`**，**不是自由函数** `mix(a, b, f)`。
- **`factor` 作用于接收者**：`a.mix(b, f)` = `f × a + (1 − f) × b`（`i-slint-core-1.13.1/graphics/color.rs:271`）。
  因此要让 `darkness = 1.0` 得到暗色，**暗色必须是接收者**：`#1C1C1E.mix(#FFFFFF, darkness)`。
- 实现是 Sass 的 `mix()` 算法，**按 alpha 加权**（`color.rs:277-300`），不是朴素逐通道插值。
  对本项目影响可忽略（同一 Token 的明暗两列 alpha 差异 ≤ 0.05），但跨 alpha 的 token（如 `surface-stroke`
  的 `0.06` 与 `0.10`）在过渡中段的混合比例会略偏向低 alpha 一侧——120ms 内不可感知。

> 结论：**方案 A 可行，无需退回方案 B。** 但 `UI-OPT-P2.01.01` 的代码必须按成员方法形式书写，
> 且参数顺序为「暗色在前、亮色在后」，与本文件初稿中写的自由函数形式不同。

---

## 1. 系统设计假设清单（Assumptions First）

### 1.1 形态与像素基线

| 项 | 取值 | 依据 |
|---|---|---|
| 渲染路径 | CPU 软件光栅，`Argb8888` 预乘，`wl_shm` / X11 `PutImage` | `renderer/raster.rs:5-20`、`platform/x11.rs:19-28` |
| 逻辑像素单位 | `dp`（= Slint `px`），物理像素 = `dp × scale` | `candidate.slint:11-13` |
| 支持的 scale | `1.0 / 1.25 / 1.5 / 2.0 / 3.0`，其余就近吸附 | `geometry.rs:66`、`geometry.rs:154-166` |
| 典型窗口尺寸 | 单候选 `284×151` 物理像素 @1x（含 32dp 阴影预留）；满页 `568×302` @2x | `layout.rs:453`、`layout.rs:486` |
| 单帧光栅预算 | P99 ≤ **1.5 ms**（`raster_p99`） | `docs/dev/budgets.json` |
| UI 线程内存增量预算 | ≤ **18 MB**（`ui_rss`） | 同上 |
| 空闲 CPU / 重绘 | ≤ **0.3%** 单核；`idle_redraw_count = 0`；**无轮询定时器** | 同上、`ui_thread/event_loop.rs:1-14` |
| 动效帧率 | 目标 144Hz；`dt` 硬钳制 ≤ 1/60s | `spring.rs:73` |
| 无障碍树 | **无**。语义断言一律走 `UiFrame`（`ASM-T-02`） | `docs/dev/tests/ui.md` §0 |

### 1.2 交互基线（决定「隐形细节」怎么落地）

| 项 | 取值 | 依据 |
|---|---|---|
| 键盘焦点 | **永不获取**。`candidate.slint` 内禁止 `TextInput` / `forward-focus` / `focus()` | `candidate.slint:15-16`、`AGENTS.md` 禁止事项 20 |
| 「焦点」语义载体 | 键盘高亮项（Focus Ring）——它是本形态唯一的焦点表达 | `features.md:1065` |
| 指针命中 | 由 `Geometry::hit_map`（容器坐标 + 全局候选索引）驱动 | `geometry.rs:296` |
| 输入区域 | 塑形到容器矩形，阴影预留必须点击穿透 | `platform/x11.rs:459`、`slint_platform.rs:163-172` |
| 滚动 | **不存在**。翻页是唯一的「长内容」机制（最多 5 页，溢出显示 `5/5+`） | `features.md:976` |
| 悬停节流 | 16 ms，且仅当索引变化才投递 | `channel.rs:71-84` |
| 动效时钟 | **由 Rust 侧 `AnimationSet` 积分后写属性**；`.slint` 内禁止 `animate` 块 | `spring/set.rs:1-16`、`spring.rs:9-14` |

> ⚠️ **最后一条是本形态最重要的工艺约束**。Slint 自带 `animate <prop> { duration: …; }` 会启动 Slint 内部时钟，
> 与「时间由调用方交给积分器、静止时 `poll` 无限等待」的 `BUDGET-CPU-01` 前提直接冲突，也会让截图回归不确定。
> 本文所有任务卡的代码**一律不出现 `animate`**，动画属性一律是普通 `in property`，由 Rust 逐帧写入。

### 1.3 12 大隐形质感维度 · 本形态重映射

技能基线的 12 维面向 Web/DOM。本形态无 DOM、无 CSS、无滚动条、无 Tooltip 组件，必须**逐条诚实重映射**，
不得为了凑数虚构不存在的组件：

| # | 基线维度 | 本形态对应物 | 落点 |
|---|---|---|---|
| 1 | 自定义微型滚动条 | **翻页指示器**（`5/5+` 溢出标 + 页码点），同样要求「静止隐退、操作时渐入」 | P2.07.01 |
| 2 | 文本选区与光标质感 | **preedit 光标**（`Preedit.caret`）+ **切分符**（`text.separator` @0.40） | P1.02.01 |
| 3 | 模态遮罩与背景虚化 | **亚克力底 + 合成器模糊协商**（`BlurNegotiation` 四级降级阶梯） | P0.06.01 |
| 4 | 悬浮工具提示 | **注音/来源标签**（`Candidate.annotation`，`text.annotation` @0.48） | P1.05.01 |
| 5 | 右键与下拉菜单 | **状态图标簇**（中/英点、全角、标点、只读锁）的悬浮胶囊热区 | P1.02.01、P2.03.01 |
| 6 | 表单校验防跳变 | **无表单**。映射为「窗口尺寸变化 140ms Spring 不抖动」+「行数降级不跳字」 | P1.05.02、P2.07.01 |
| 7 | 分割条工艺 | **无分割条**。映射为「单元格与容器的同心圆角 + 内边距台阶」 | P0.02.01 |
| 8 | 图标光学中线 | 状态图标与文本基线校准（`translateY` 等价物 = 显式 y 偏移） | P1.02.01 |
| 9 | 渐变遮罩截断 | **preedit 左截断的渐变淡出**（Slint 无 `mask-image`，用 `@linear-gradient` 覆盖层等价实现） | P1.02.01 |
| 10 | 空状态与骨架屏 | **词库加载中占位 / 词库不可用 / 只读锁 / 已达上限** | P2.07.01 |
| 11 | 操作排布空间语法 | 候选单元的 **Default / Hover / Active / Focus Ring / Disabled** 五态优先级 | P2.03.01 |
| 12 | 三层景深体系 | Canvas（桌面）→ **Surface**（面板）→ **Elevated**（选中单元 / 光标箭头） | P0.06.01、P2.03.01 |

### 1.4 顶级标杆对标解析

| 标杆 | 本形态要抄的具体决策 |
|---|---|
| **macOS 原生候选窗** | 亚克力底 + 极细内描边；候选序号以**低对比小字**居于文字左侧而非独立色块；首选项用**淡色底 + 细描边**而非实心强调色 |
| **微信输入法 macOS** | 拼音串的**音节分隔符**（`'`）用 40% 不透明度弱化；从左截断保留最近输入 |
| **Squirrel/Rime** | 键盘高亮框**滑动**而非跳变——正是 `spring/highlight.rs` 已经算好但没接上的东西 |
| **Linear / Raycast** | 键盘高亮与鼠标悬停**同时存在时键盘优先**；悬停只改指针与其他项，不抢高亮 |
| **Craft / Bear** | 外层软阴影必须是**真衰减**；等宽色带会立刻暴露「廉价感」——这是本项目当前最刺眼的材质缺陷（UI-DEF-16） |

---

## 2. 设计规范与参数底座（Design Tokens & Spatial Specs）

本节是全部 14 张任务卡共用的参数底座。**任何卡片不得自行定义不同数值**；与 `features.md` 3.1/3.2 冲突的，
一律以 `features.md` 为准并回写本表。

### 2.1 三层景深明度基准（本形态实测值）

| 层 | 暗色 | 亮色 | 本形态实现 |
|---|---|---|---|
| Canvas | 桌面本体（不可控） | 桌面本体（不可控） | 透明表面，`background: transparent` |
| Surface | `#1C1C1E @ α 0.85` | `#FFFFFF @ α 0.85` | `Theme.surface-fill`，`candidate.slint:256` |
| Elevated | 强调色 @ 0.18（暗）/ 0.14（亮） | 同左 | `Theme.state-selected-bg`，**当前零引用** |

### 2.2 次像素边缘与环境内光（Sub-pixel Inner Border）

本形态**不使用** `inset box-shadow`（无 CSS）。等价物是 Slint 的 `border-width` + `border-color`，
Slint 的 `Rectangle` 边框**绘制在矩形内侧**，天然满足 3.1.2 的「inset 描边，不改变外部尺寸」：

| 场景 | 暗色 | 亮色 |
|---|---|---|
| 容器描边（1dp） | `rgba(255,255,255,0.10)` | `rgba(0,0,0,0.06)` |
| 选中单元描边（1dp） | `accent @ 0.55` | `accent @ 0.50` |
| 悬停单元描边 | 无（仅底色 `state.hover`） | 无 |

### 2.3 物理三层复合投影（Physical Lighting Elevation）

3.1.2 规定的两层材质，本形态的**正确构造方式**（UI-DEF-16/17 的修复目标）：

```
L3 外层软阴影  0 8dp 28dp  shadow.outer
   → 8 条同心圆角环带，向外铺满 28dp，带宽 3.5dp
   → 每条环带的 opacity 沿二次衰减：(i+1)/8 的平方 × 峰值不透明度
   → 环带互不重叠，因此复合 α 就等于单条环带的 α；衰减必须做在 opacity 上

L2 内层硬阴影  0 1dp 2dp   shadow.inner
   → 2 条 1dp 环带，外圈 α = 0.35 × 峰值，内圈 α = 0.75 × 峰值
   → 整体下移 1dp

L4 容器描边    1dp         surface.stroke（内侧绘制）
```

### 2.4 弹簧阻尼微动效（Spring Constants，来自 3.3，已实现）

| 动效 | 参数 | 稳定时间 | 落点属性（P0.08.01 建立） |
|---|---|---|---|
| 高亮滑动 | `ω₀=26.0, ζ=0.85, m=1.0` | 181 ms | `highlight-x/y/w/h` |
| 翻页内容位移 | `ω₀=32.0, ζ=0.90`，行程 `±12dp` | 139 ms | `page-offset-dp` |
| 窗口尺寸变化 | `ω₀=30.0, ζ=0.92` | ~145 ms | `container-width/height` |
| 出现 | `110ms` `cubic-bezier(0.22,1.0,0.36,1.0)`，`scale 0.96→1.0` | — | `window-opacity` / `window-scale` |
| 消失 | `90ms` `cubic-bezier(0.4,0.0,1.0,1.0)`，`scale 1.0→0.98` | — | 同上 |
| 状态图标 / 主题 crossfade | `120ms` `ease-in-out` | — | `theme-blend` |

### 2.5 空间微网格（4dp）

`features.md:982` 规定「所有间距与尺寸必须是 `4dp` 的整数倍，例外仅描边 `1dp` 与光标箭头 `6dp` 高」。
**当前 3.1.1 表本身违反该规则 8 处**（UI-DEF-02），已在 v1.4 裁决为**保留规则 + 显式例外清单**，处理方式见 P0.01.02。

---

## 3. UI 质感与隐形细节缺陷总清单（Defect Inventory）

> 编号规则 `UI-DEF-NN`；「所属域」为 §1.3 的八域；每条均可被 §4 追溯矩阵与任务卡双向验证。

### 域01 全局基线

| 编号 | 目标文件与行号 | 坏味道类别 | 现状具体病灶剖析与工业级对标差距 |
|---|---|---|---|
| `UI-DEF-01` | `crates/ime-ui/ui/candidate.slint:160-179`、`renderer/probe.rs:119-127` | 字体栈缺失 | 全仓库 **0 处 `font-family` 声明**。Slint 软件渲染器经 `fontdb` 回退到系统默认无衬线；在只装了拉丁字体的环境下，`你好啊` 直接变豆腐块。`probe_fonts()` 只**检测**不**修复**——它把 `ui/font/missing-cjk` 记成诊断就结束了，用户看到的是方框。对标 macOS 候选窗：字体栈是平台级保证，不存在「可能画不出」这一状态。 |
| `UI-DEF-02` | `candidate.slint:35,37,40,46,47,49,51`、`docs/dev/features.md:982` | 规范自相矛盾 | `header-height: 34px`、`header-padding-h: 10px`、`header-text-gap: 6px`、`cell-padding-h: 10px`、`cell-padding-v: 6px`、`number-gap: 6px`、`annotation-gap: 6px`、`grid-gap: 6px` —— **8 个值不是 4 的倍数**，而 3.1.4 只给了「1px 描边」「6dp 箭头」两个例外。任何按 3.1.4 写的断言脚本都会失败——这正是 `check-ui-spec.sh` 迟迟写不出来的根因。 |
| `UI-DEF-03` | `crates/ime-ui/ui/theme.slint:48,49,52,55,56,57,58,68,69` | 设计系统半落地 | 9 个 Token 定义了却**零引用**：`text-annotation`、`text-separator`、`accent-on`、`state-hover`、`state-selected-bg`、`state-selected-stroke`、`state-pressed`、`status-dot-active`、`status-dot-idle`。设计系统只有一半接上了线，另一半是装饰。 |
| `UI-DEF-04` | `crates/ime-ui/src/theme.rs:22`、`ui/theme.slint:12`、`docs/dev/features.md:3309` | 门禁缺失 | `scripts/check-ui-spec.sh` **不存在**（`ls scripts/` 无此文件），却被 6 处引用，其中 2 处是生产代码注释、2 处是任务卡验收标准。后果：`theme.slint:39-69` 与 `theme.rs:501-698` 两份调色板**没有任何自动化比对**，漂移随时可能发生且无人发现。 |
| `UI-DEF-24` | `candidate.slint:32,38,39,44,45,46,47,49,50,56,57,73,77,78,82,83,87` | 常量引用断裂 | 13 个尺寸常量在 `.slint` 侧零引用：`max-width`、`header-icon-size`、`header-icon-gap`、`cell-padding-h`、`cell-padding-v`、`number-gap`、`annotation-gap`、`cursor-arrow-width/height`、`max-text-width`、`cell-chrome-width`、`max-pages`、`min-per-row`、`max-per-row-limit`、`font-size-cell`。这些常量被 Rust 侧 `layout.rs`/`placement.rs` 消费，但**绘制侧不消费**——即「算的是一套，画的是另一套」。 |
| `UI-DEF-25` | `crates/ime-ui/src/theme.rs:467-471` | 切换硬闪 | `apply()` 直写 `set_dark` / `set_accent` / `set_base_alpha` 三个属性，`Theme` global 的 `out property` 立即重算 → 深浅色切换是**瞬时硬闪**。3.2 明确要求「切换延迟 ≤ 300ms（含 120ms crossfade 过渡）」。 |

### 域02 布局框架

| 编号 | 目标文件与行号 | 坏味道类别 | 现状具体病灶剖析与工业级对标差距 |
|---|---|---|---|
| `UI-DEF-05` | `crates/ime-ui/ui/candidate.slint:200-206` | 占位未落地 | 候选单元是 `Rectangle { background: transparent; }` —— **没有文本、没有序号、没有注音、没有状态**。整个候选区是「预留空间的占位」（文件注释 `candidate.slint:183-185` 自认）。这是全项目最大的视觉空洞：候选框的核心内容不存在。 |
| `UI-DEF-06` | `candidate.slint:143-181`、`theme.slint:68-69` | 状态簇缺失 | Header 只有两个 `Text`（preedit + mode-label）。3.1.1 规定的 **16dp 状态图标、8dp 图标间距**没有任何实现；`status.dot.active/idle`、`full_width`、`punctuation_full`、`readonly` 五个语义**全部无落点**。用户无法从候选框看出中/英、全角/半角、是否只读。 |
| `UI-DEF-07` | `candidate.slint:56-57`、`geometry/placement.rs:315-335` | 链路断尾 | 光标箭头三段链路：① metrics 已声明（`cursor-arrow-width/height`）② 几何已算出（`Pass::arrow` 返回 `Option<RectI>`，含「翻转/夹取时不画」的完整规则）③ **`.slint` 里没有任何组件绘制它**。前两段都做完了，最后一环没接。 |
| `UI-DEF-08` | `candidate.slint:160-169`、`ime-types/src/ui.rs:99-122` | 截断方向错误 | preedit 用单个 `Text` + `overflow: elide` → **尾部省略**；而 3.1.3 要求「拼音串从**左侧**截断，保留最近输入」。代码注释（`candidate.slint:157-159`）承认这是「last-resort guard」，但适配器并不存在，所以它实际就是唯一行为。同时 `PreeditSpan`/`SpanKind::{Syllable,Separator,Passthrough,Cursor}` 契约完整，`text.separator` @0.40 的切分符**零渲染路径**——`ni'hao'a` 现在只能画成一根等粗的 `ni'hao'a`。 |
| `UI-DEF-09` | `candidate.slint:152-155` vs `193` | 光学错位 | Header 水平内边距 `10dp`，候选区 `8dp`。preedit 首字与第一个候选格左边缘**错位 2dp**。规范 3.1.1 确实分别规定了 10dp 与 8dp，但视觉上这是「文本没有对齐」——对标 macOS 候选窗，拼音串与候选文字共享同一条左基线。 |
| `UI-DEF-24b` | `crates/ime-ui/src/lib.rs:34-37` | 组件未实例化 | `CandidateWindow` 组件**从未被构造**。`container-width/height/cell-width/grid-rows/item-count/max-per-row` 六个 `in` 属性没有任何写入方；生产代码没有任何 `impl UiSurface`。整个 `.slint` 目前只是一份**能被编译、不能被显示**的声明。 |
| `UI-DEF-27` | `crates/ime-ui/src/geometry/placement.rs:269-306`、`geometry/tests.rs:611-626` | **定位偏移 32dp** | `vertical_pos` 把 `caret.bottom + caret_gap(6dp)` 直接当作**窗口**原点，而窗口原点含四周 `32dp` 阴影预留 ⇒ 面板可见上边缘落在 `caret.bottom + 38dp`。`Pass::arrow`（`placement.rs:329-334`）把箭头放在同一个窗口原点上，于是箭头悬在面板上方 **32dp 的透明空白**里，与 3.1.1「箭头填充光标与候选框之间的 6dp 间隙」完全脱节。**现有测试把错误行为断言为正确**（`tests.rs:620,623`），所以门禁全绿也发现不了。 |

### 域03 操作控件

| 编号 | 目标文件与行号 | 坏味道类别 | 现状具体病灶剖析与工业级对标差距 |
|---|---|---|---|
| `UI-DEF-10` | `theme.slint:55-58`、`candidate.slint:186-207` | 五态零实现 | 3.4 的整张五态表（`Default`/`Hover`/`Active`/`Focus Ring`/`Disabled`）**零落点**：四个 `state.*` Token 定义了不用，候选单元没有 `MouseArea` 等价物，没有按下 `scale 0.97`，没有 `Disabled` 的 `opacity 0.32`。 |
| `UI-DEF-11` | `geometry.rs:296`、`slint_platform.rs:171` | 交互链路缺失 | `hit_map`（容器坐标 + 全局候选索引）已经算出，**没有任何函数消费它**；`set_input_region` 在生产路径**零调用**（只有测试与 `SlintPlatform` 的转发方法）。后果：阴影预留区域会吞掉本该穿透到应用的点击，且点击候选无任何反应。 |

### 域04 文本呈现（本形态无表单控件，重映射至此）

| 编号 | 目标文件与行号 | 坏味道类别 | 现状具体病灶剖析与工业级对标差距 |
|---|---|---|---|
| `UI-DEF-12` | `ime-types/src/ui.rs:100-105`、`ime-core/src/preedit.rs:143-157` | 契约字段零消费 | `Preedit.caret` 由 `ime-core` 精确产出（含 UTF-8 边界钳制），在 `ime-ui` 侧**零消费**。用户在 `←/→` 移动光标时**看不到任何光标**。`SpanKind::Cursor` 同样零消费。 |
| `UI-DEF-13` | `crates/ime-ui/src/layout.rs:171` | 预算漏项 | `text_cap = max_text_width + cell_chrome_width`，而 `cell_chrome_width: 36px` 的构成是「2×10 内边距 + 序号 + 6 间距」——**完全不含注音**。`LayoutHint::show_annotation` 是契约字段，但 `cell_width()` 签名里根本没有它。带注音的候选会被过早截断，注音自身更无宽度预算。 |

### 域05 数据陈列

| 编号 | 目标文件与行号 | 坏味道类别 | 现状具体病灶剖析与工业级对标差距 |
|---|---|---|---|
| `UI-DEF-14` | `crates/ime-ui/src/layout.rs:265`、`candidate.slint:196` | 排布留白 | 容器宽度 `(cells + 2×padding).clamp(floor, cap)` 会被抬到 `min_width = 220dp`；而 `CandidateGrid` 用 `HorizontalLayout` **左对齐**。单候选 64dp 时，右侧留下 `220 - 16 - 64 = 140dp` 空洞。对标 macOS：候选格会**吸收剩余宽度**或整行居中，绝不出现半屏空白。 |
| `UI-DEF-15` | `candidate.slint:48` vs `29,30` | 圆角不同心 | 容器圆角 `12dp` + 内边距 `8dp` → 同心内圆角应为 `12 - 8 = 4dp`；单元格实际是 `8dp`。嵌套圆角不同心，在网格四角会露出「外方内圆」的破绽。**v1.4 已裁决**：`features.md` 3.1.1 与 `.slint` 常量均改为 `4dp`（见 §0.1）。 |
| `UI-DEF-21` | `candidate.slint:49,186-207` | 快捷键徽标缺失 | 候选**序号零实现**：`number-gap` 声明未用，3.1.1 的 `11sp / 500 / opacity 0.55` 无落点。对标 macOS / 微软拼音：数字键提示是候选框最高频的功能性微件，没有它用户不知道按几。 |
| `UI-DEF-26` | `docs/dev/features.md:939,941,992` | Token 与用法叠加冲突 | 3.2 定义 `text.annotation` = `rgba(242,242,247,0.48)`（暗）并注明用途为「注音、候选序号」；3.1.1 又给序号 `opacity 0.55`、注音 `opacity 0.50`。若按字面叠加，有效 α = `0.48 × 0.55 = 0.264` / `0.48 × 0.50 = 0.24`，在 `#1C1C1E` 底上对比度仅约 **2.35:1 / 2.2:1**——数字键提示是候选框最核心的键盘入口，这个对比度不可读。**v1.4 已裁决为「取代」**：3.1.1 的 `opacity` 即最终有效 α，不与 Token α 相乘（见 §0.1）。 |

### 域06 浮层系统

| 编号 | 目标文件与行号 | 坏味道类别 | 现状具体病灶剖析与工业级对标差距 |
|---|---|---|---|
| `UI-DEF-16` | `candidate.slint:112-124` | 景深缺失（**最刺眼**） | 外层软阴影是 4 条**等透明度**环带：`spread` 从 28 递减到 7、`border-width` 恒为 `band-step = 7px`、`opacity` 恒为 `0.18`。四条环带**首尾相接铺满 28dp 且互不重叠** ⇒ 复合 α 处处等于 0.18 ⇒ 结果是**一整片 28dp、18% 黑的均匀色晕，外缘硬切**。既无高斯衰减也无层次。注释（`candidate.slint:66-67`）声称「falloff accumulates towards the panel edge」——**与几何事实不符**。对标 Craft/Bear 的浮层：外阴影必须是可见的衰减梯度，否则一眼廉价。 |
| `UI-DEF-17` | `candidate.slint:64,70,127-137` | 材质参数超标 | 内层阴影 `shadow-inner-spread: 4px` + `opacity: 1.0` + 色值 `rgba(0,0,0,0.35)`（暗色） ⇒ 容器外一圈 **4dp、35% 黑的不透明硬环**。3.1.2 规定的是 `0 1dp 2dp`——**扩散量超标 4 倍**。读起来是一条粗黑描边，不是内阴影。 |
| `UI-DEF-18` | `candidate.slint:96-99` | 替代方案选错 | 注释正确指出 `drop-shadow-*` 在软件渲染器下是空实现，但替代方案（等 α 同心环带）没有实现衰减语义。这是「知道不能用 A，随手用了一个不对的 B」的典型。 |

### 域07 状态反馈

| 编号 | 目标文件与行号 | 坏味道类别 | 现状具体病灶剖析与工业级对标差距 |
|---|---|---|---|
| `UI-DEF-19` | `ime-types/src/ui.rs:159-172`、`candidate.slint:171-179` | 状态反馈缺失 | `StatusStrip` 五个字段中只有 `mode_label` 有落点，且是**一根等粗的纯文本**。`full_width`（全角）、`punctuation_full`（中文标点）、`has_user_dict_hit`（用户词命中）、`readonly`（只读锁）四个状态**用户完全不可见**。3.6 明确要求只读模式显示「灰色小锁图标」、输入达上限显示「已达上限」。 |
| `UI-DEF-20` | `docs/dev/features.md:1070` | 骨架屏缺失 | 词库加载期要求「候选框显示『词库加载中…』单行占位，高度 34dp，`opacity 0.6`，不可交互」——**零实现**。3.6 的「词库不可用」也只是一行文案，同样零实现。当前行为：加载期窗口要么不出现，要么出现一个空壳。 |

### 域08 键盘交互

| 编号 | 目标文件与行号 | 坏味道类别 | 现状具体病灶剖析与工业级对标差距 |
|---|---|---|---|
| `UI-DEF-22` | `candidate.slint:87,186-207`、`theme.slint:56-57` | 键盘反馈缺失 | Focus Ring（键盘高亮）零实现：`state-selected-bg` / `state-selected-stroke` 定义了不用；首选项与键盘高亮项的**字重区分**（`15sp / 400` vs `15sp / 500`，3.1.1）也没有——`font-size-cell` 声明未用。3.4 规定的状态优先级 `Disabled > Active > Focus Ring > Hover > Default`、以及「键盘高亮与鼠标悬停同时存在时以键盘为准」**均无实现载体**。 |
| `UI-DEF-23` | `spring/highlight.rs:132-277`、`spring/set.rs:29-40` | 动效无落点 | `AnimationSet` 已完整实现高亮弹簧（四自由度、速度续接、损伤区域）、窗口淡入淡出、翻页位移——`FrameMotion` 的四个字段 `highlight.rect` / `opacity` / `scale` / `page_offset_dp` **没有任何 `.slint` 属性可以写入**。`CandidateWindow` 只声明了 preedit/mode-label/几何六项。整个动效子系统悬空。 |

---

## 4. 覆盖追溯矩阵（严格双向一致）

| 缺陷编号 | 所属域 | 任务卡 | 优先级 | 关键路径 |
|---|---|---|---|---|
| `UI-DEF-01` | 域01 | `UI-OPT-P0.01.02` | P0 | 是 |
| `UI-DEF-02` | 域01 | `UI-OPT-P0.01.02` | P0 | 否 |
| `UI-DEF-03` | 域01 | `UI-OPT-P0.01.01` | P0 | 是 |
| `UI-DEF-04` | 域01 | `UI-OPT-P0.01.01` | P0 | 否 |
| `UI-DEF-05` | 域02 | `UI-OPT-P1.05.01` | P1 | 是 |
| `UI-DEF-06` | 域02 | `UI-OPT-P1.02.01` | P1 | 是 |
| `UI-DEF-07` | 域02 | `UI-OPT-P1.02.02` | P1 | 否 |
| `UI-DEF-08` | 域02 | `UI-OPT-P1.02.01` | P1 | 是 |
| `UI-DEF-09` | 域02 | `UI-OPT-P0.02.01` | P0 | 否 |
| `UI-DEF-10` | 域03 | `UI-OPT-P2.03.01` | P2 | 是 |
| `UI-DEF-11` | 域03 | `UI-OPT-P2.05.01` | P2 | 是 |
| `UI-DEF-12` | 域04 | `UI-OPT-P1.02.01` | P1 | 否 |
| `UI-DEF-13` | 域04 | `UI-OPT-P1.05.01` | P1 | 否 |
| `UI-DEF-14` | 域05 | `UI-OPT-P1.05.02` | P1 | 否 |
| `UI-DEF-15` | 域05 | `UI-OPT-P0.02.01` | P0 | 否 |
| `UI-DEF-16` | 域06 | `UI-OPT-P0.06.01` | P0 | 是 |
| `UI-DEF-17` | 域06 | `UI-OPT-P0.06.01` | P0 | 否 |
| `UI-DEF-18` | 域06 | `UI-OPT-P0.06.01` | P0 | 否 |
| `UI-DEF-19` | 域07 | `UI-OPT-P2.07.01` | P2 | 否 |
| `UI-DEF-20` | 域07 | `UI-OPT-P2.07.01` | P2 | 否 |
| `UI-DEF-21` | 域05 | `UI-OPT-P1.05.01` | P1 | 是 |
| `UI-DEF-22` | 域08 | `UI-OPT-P2.03.01` | P2 | 是 |
| `UI-DEF-23` | 域08 | `UI-OPT-P0.08.01` | P0 | 是 |
| `UI-DEF-24` | 域01 | `UI-OPT-P0.01.01` | P0 | 是 |
| `UI-DEF-24b` | 域02 | `UI-OPT-P0.02.02` | P0 | **是（头号阻塞）** |
| `UI-DEF-25` | 域01 | `UI-OPT-P2.01.01` | P2 | 否 |
| `UI-DEF-26` | 域05 | `UI-OPT-P1.05.01` | P1 | 否 |
| `UI-DEF-27` | 域02 | `UI-OPT-P0.02.03` | P0 | **是** |

**反向校验**：15 张任务卡每张都在「绑定缺陷编号」字段列出上表对应的 `UI-DEF-*`，无孤立卡片、无未归属缺陷。

---

## 5. 关键路径与并行通道

### 5.1 并行通道

| 通道 | 主题 | 卡片 |
|---|---|---|
| **Track A** | 底座与微观质感 | `P0.01.01`、`P0.01.02`、`P0.02.01`、`P0.06.01` |
| **Track B** | 结构与操作流线 | `P0.02.02`、`P0.02.03`、`P1.05.01`、`P1.02.01`、`P1.02.02`、`P1.05.02` |
| **Track C** | 控件与动态反馈 | `P0.08.01`、`P2.03.01`、`P2.05.01`、`P2.07.01`、`P2.01.01` |

### 5.2 关键路径

```
P0.02.02（组件实例化 + UiFrame 绑定）
   ├─→ P0.02.03（定位偏移修正）──→ P1.02.02（光标箭头）
   └─→ P1.05.02（行内宽度分配）──→ P2.05.01（命中与悬停）──→ P2.03.01（五态 + Focus Ring）
            └─→ P1.05.01（候选网格骨架）────────────────────┘
P0.01.01（Token 引用闭环）──→ P1.02.01（Header 状态簇）──→ P2.07.01（降级态与状态反馈）
P0.01.02（字体栈）──────────→ P1.02.01
P0.08.01（动效属性落点）────────────────────────────────→ P2.03.01
P0.06.01（阴影材质）  ── 独立，无下游
P0.02.01（圆角与内边距）──→ P1.05.01
P0.01.01 ──→ P2.01.01（主题 crossfade 与光学微调）
```

**CP 总长**：`P0.02.02 → P1.05.02 → P2.05.01 → P2.03.01`，共 **4 张卡**（最长链）。
**无跨轨道硬阻塞**：Track A 的四张卡不依赖 Track B；Track C 的 `P0.08.01` 只依赖 `P0.02.02` 与已完成的 `spring/`；
`P2.05.01` 与 `P2.03.01` 之间是**串行**（五态的 Hover/Active 需要命中结果），已在 Phase 3 的前置依赖字段中标注。

### 5.3 并行化约束

- `P0.01.01` 与 `P0.01.02` 都改 `ui/theme.slint` 与 `ui/candidate.slint` 的**声明块** → **必须串行**，或由同一执行者一次完成。
- `P0.06.01` 改 `candidate.slint:100-138`，`P0.02.01` 改 `candidate.slint:25-89` 与 `:250-282`，`P0.08.01` 改 `candidate.slint:211-232` → 三者在**同一文件不同区段**，可并行但需约定「只改自己声明的行区间」。
- 任何卡片都**不得**改 `crates/ime-types/`（冻结契约）与 `crates/ime-ui/src/lib.rs`（模块根）。

---

## 6. P0 任务卡（原子展开）

---

#### 任务 ID：`UI-OPT-P0.01.01` 材质与尺寸 Token 引用闭环 + `check-ui-spec` 门禁落地

- **基本属性**：
  - 绑定缺陷编号：`UI-DEF-03, UI-DEF-04, UI-DEF-24`
  - 所属组件域：域01 全局基线
  - 优先级与复杂度：`P0` | 中 | 预估工时: 2 人天
  - 前置依赖：无
  - 关键路径：**是**
  - 并行通道：Track A 底座与微观质感
  - 代码落地锚点：`crates/ime-ui/ui/theme.slint`、`crates/ime-ui/ui/candidate.slint`、`scripts/check-ui-spec.sh`、`justfile`、`crates/ime-ui/src/theme.rs`
  - 当前状态：`[x] 已完成`

- **原实现弊端与微观质感缺失剖析**：
  - **现有代码具体病灶**：`theme.slint:48-69` 定义了 18 个 Token，`candidate.slint` 只引用了 7 个（`shadow-outer`/`shadow-inner`/`surface-fill`/`surface-stroke`/`text-primary`/`text-secondary`/`separator`）。剩下 9 个是**死 Token**。同时 `candidate.slint` 的 `CandidateMetrics` 里 13 个常量在绘制侧零引用。这不是「预留」，而是**设计系统与绘制层的接线断了一半**——它让任何后续视觉工作都无法判断「这个 Token 到底有没有生效」。
  - **门禁缺失的后果**：`theme.rs:501-698` 手工重复了整份调色板（因为对比度自检必须脱离渲染器运行）。文件注释（`theme.rs:19-26`）声明两份副本「由本文件旁的单元测试与 `scripts/check-ui-spec.sh` 钉住」。**该脚本不存在**，所以「钉住」这句话目前是空的——两份 8 位 α 值（如 `0.62 × 255 = 158.1 → 158`）只要有一处手滑就永久漂移。
  - **顶级标杆对标解析**：Linear / Raycast 的设计系统里不存在「定义了但没用的 Token」——CI 会直接失败。本卡的目标是把「引用闭环」变成**可执行断言**，而不是靠人眼审阅。

- **设计规范与参数定义 (Design Tokens & Spatial Specs)**：
  - **色彩与材质**：不新增任何色值。本卡只做**引用接线**与**门禁**，色值全部沿用 `features.md` 3.2 表。
  - **空间排布与微网格**：不新增任何尺寸。
  - **微观尺寸**：门禁脚本需覆盖的比对项 = `features.md` 3.1.1 表的全部尺寸行 + 3.2 表的全部颜色行。
  - **门禁容差**：颜色 α 值按 `round(α × 255)` 比对，**零容差**；尺寸按整数 dp 比对，**零容差**。

- **重构源码落地实现 (Code Delivery)**：

  **步骤 A —— 接线（`ui/candidate.slint`）**。把死 Token 接到**有意义的宿主**上，而不是为了「用掉」而滥用：

  ```slint
  // The header's status cluster. Each icon is a 16dp disc; the ones that are off are drawn
  // at the idle token rather than hidden, so the cluster's width never changes and the
  // header text cannot shift when a mode toggles (3.4: no layout jump).
  export component StatusCluster inherits HorizontalLayout {
      in property <bool> chinese: true;
      in property <bool> full-width: false;
      in property <bool> punctuation-full: false;
      in property <bool> readonly: false;

      spacing: CandidateMetrics.header-icon-gap;
      // The cluster is a fixed-size block: its width is icon-size * 4 + gap * 3, declared
      // once so the header text can be budgeted against it (3.1.3).
      width: 4 * CandidateMetrics.header-icon-size + 3 * CandidateMetrics.header-icon-gap;
      height: CandidateMetrics.header-icon-size;
      vertical-alignment: center;

      // Mode dot: solid accent in Chinese mode, the idle token in English mode.
      Rectangle {
          width: CandidateMetrics.header-icon-size;
          height: CandidateMetrics.header-icon-size;
          border-radius: self.width / 2;
          background: root.chinese ? Theme.status-dot-active : Theme.status-dot-idle;
      }
      // Full-width marker: the same disc, filled only while full-width punctuation is on.
      Rectangle {
          width: CandidateMetrics.header-icon-size;
          height: CandidateMetrics.header-icon-size;
          border-radius: self.width / 2;
          border-width: 2px;
          border-color: root.full-width ? Theme.status-dot-active : Theme.status-dot-idle;
          background: transparent;
      }
      // Chinese-punctuation marker.
      Rectangle {
          width: CandidateMetrics.header-icon-size;
          height: CandidateMetrics.header-icon-size;
          border-radius: self.width / 2;
          border-width: 2px;
          border-color: root.punctuation-full ? Theme.status-dot-active : Theme.status-dot-idle;
          background: transparent;
      }
      // Read-only lock: drawn only in the degraded mode 3.6 describes, and drawn in the
      // annotation token because it is information, not an alert.
      Rectangle {
          width: CandidateMetrics.header-icon-size;
          height: CandidateMetrics.header-icon-size;
          border-radius: 3px;
          border-width: 1px;
          border-color: Theme.text-annotation;
          background: transparent;
          opacity: root.readonly ? 1.0 : 0.0;
      }
  }
  ```

  `text-annotation` / `text-separator` / `state-*` / `accent-on` 的接线分别落在 `P1.05.01`（序号、注音）与
  `P2.03.01`（五态）。本卡负责**建立门禁**，使这些接线一旦漏掉就会在 CI 失败。

  **步骤 B —— `scripts/check-ui-spec.sh`（新建）**。脚本骨架：

  ```bash
  #!/usr/bin/env bash
  # Compares docs/dev/features.md sections 3.1.1 and 3.2 against the .slint declarations.
  #
  # The two sources are the spec table and the component that draws from it. They are kept
  # in step by this script rather than by review, because a size that drifts between them
  # is invisible until someone measures the window on screen.
  #
  # Exits non-zero and prints every mismatch; prints PASS on success.
  set -euo pipefail

  ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  exec python3 "$ROOT/scripts/check-ui-spec.py" "$ROOT"
  ```

  比对实现（`scripts/check-ui-spec.py`）必须覆盖三类断言：
  1. **3.1.1 尺寸 ↔ `CandidateMetrics`**：把表里的中文元素名映射到 kebab-case 属性名（映射表写在脚本里，是**唯一**允许出现中文注释的地方——脚本不是 `.rs`，不受 `AGENTS.md` §1 的英文注释约束，但映射表仍需逐行注释）。
  2. **3.2 颜色 ↔ `theme.slint` 默认值 ↔ `theme.rs` 的 `DARK_PALETTE`/`LIGHT_PALETTE`**：三向比对，α 按 `round(α×255)`。
  3. **3.1.4 的 4dp 规则**：对 `CandidateMetrics` 的每个 `length` 常量断言 `value % 4 == 0`，
     白名单取自 `features.md` 3.1.4 的**显式例外清单表**（v1.4 已建立，含 12 个常量名：
     `stroke-width`、`separator-height`、`shadow-inner-offset-y`、`cursor-arrow-height`、`header-height`、
     `header-padding-h`、`header-text-gap`、`cell-padding-h`、`cell-padding-v`、`number-gap`、`annotation-gap`、`grid-gap`）。
     **脚本必须解析该表**而不是把白名单硬编码在脚本里，否则规范新增例外时脚本会静默失准。
     字号、计数、比例三类常量不参与该断言（见规范 3.1.4 的「不适用本规则的量」）。

  **步骤 C —— `justfile` 接线**：

  ```make
  # Candidate-window spec conformance: features.md 3.1/3.2 against the .slint sources.
  check-ui:
      bash scripts/check-ui-spec.sh
  ```

  并把 `check-ui` 加入 `ci` 目标（`justfile` 的 `ci` 配方），与 `check-deps` / `check-unsafe` / `check-net` / `check-slint` / `check-dict` 并列。

- **逐步落地实施步骤**：
  1. **门禁先行**：写 `check-ui-spec.py`，先让它**红**——记录当前实际失败项（预期：颜色三向比对若有 α 不一致则 N 项；
     4dp 断言在解析 v1.4 的例外清单后应为**零失败**，若非零则说明清单与 `.slint` 已经漂移）。把失败清单写进本卡的验收记录。
  2. **接线**：在 `candidate.slint` 落地 `StatusCluster`（步骤 A 的代码），把 `header-icon-size` / `header-icon-gap` / `status-dot-*` 从死常量变成活常量。
  3. **收敛**：修复 `theme.rs` 与 `theme.slint` 的 α 差异（若有），使颜色三向比对转绿。
  4. **门禁转绿**：4dp 断言自本卡起即为 `FAIL` 级（例外清单已在 `features.md` 3.1.4 建立，无需等待后续卡片）。
  5. **走查**：`just check-ui` 输出 `PASS`；`grep -c "Theme\." candidate.slint` 的引用集合与 `theme.slint` 的声明集合之差 ≤ 3（仅允许 `accent-on` 与两个 crossfade 用 Token 处于待用状态，且在 `P2.01.01` 后归零）。

- **验收标准 (DoD)**：
  - [ ] **微观隐形细节审查**：`scripts/check-ui-spec.sh` 存在、可执行、输出 `PASS`；`just ci` 包含 `check-ui`。
  - [ ] **反 AI 模板风审查**：`candidate.slint` 中零引用的 `Theme.*` Token 数 ≤ 3；零引用的 `CandidateMetrics.*` 常量数 ≤ 2（仅允许 `max-width`、`max-per-row-limit`，它们由 Rust 侧布局消费）。
  - [ ] **排布与工学**：`StatusCluster` 宽度是**固定值**，模式切换不改变 Header 文本可用宽度 ⇒ 切换时无横向抖动。
  - [ ] **物理微交互**：本卡不引入动效；`candidate.slint` 内 `grep -c "animate"` 结果为 0。
- **验收记录**（2026-09-29）：
  - **交付物**：`scripts/check-ui-spec.sh`（1085 行，bash + 内嵌 python3，含 9 例 `--self-test`）、`crates/ime-ui/ui/candidate.slint`（新增 `StatusCluster` 组件）、`justfile`（新增 `check-ui`，接入 `ci` 与 `check-self-tests`）、`.github/workflows/ci.yml`。`theme.slint` / `theme.rs` 未改动——三向比对本来就已一致（上一张卡的 7 个 α 字节修正生效），本卡把它变成可执行断言。
  - **验证命令与结果**：
    - `bash scripts/check-ui-spec.sh` → `PASS (33 sizes from 3.1.1, 34 colours from 3.2 across theme.slint and theme.rs, 29 lengths on the 4dp grid of 3.1.4 with 12 exceptions, 5 type-scale sizes, 18 tokens and 39 constants declared, 2 tokens and 0 constants deferred)`
    - `bash scripts/check-ui-spec.sh --self-test` → `PASS (9 injected violations, all reported)`
    - `cargo check -p ime-ui --all-targets` → 绿（确认新增的 `StatusCluster` 能被 `slint-build` 编译）
  - **门禁的四类断言（全部解析规范表，绝不另抄白名单）**：①3.1.1 尺寸 ↔ `CandidateMetrics`（覆盖该表全部 23 行、33 个常量取值，并把 `layout/metrics.rs` 的 `take*` 调用名反向断言为其子集，改名即红）；②3.2 颜色**三向**比对（规范表 ↔ `theme.slint` 默认表达式按 Slint 规则求值 ↔ `theme.rs` 的两个调色板），颜色按 Slint 自己的换算求值（`rgba()` 截断、`with-alpha()` 四舍五入），与 `src/theme/slint_palette.rs` **逐字节一致**，不存在两套判据；③3.1.4 的 4dp 网格与 12 项例外清单（表内常量必须存在且确实不在网格上，清单不会腐烂）；④引用闭环（死 token 必须登记在带规范归属的延迟清单里，**且延迟名一旦被引用即红**——清单只减不增）。
  - **已知限制**：
    1. **DoD「反 AI 模板风审查」在本卡内不可达**，且与卡内步骤 A 自相矛盾：接线后零引用的 `Theme.*` 为 **6** 个（`accent-on`、`text-separator`、`state-hover`、`state-selected-bg`、`state-selected-stroke`、`state-pressed`），零引用且无 Rust 消费者的 `CandidateMetrics` 常量为 **4** 个（`cell-padding-h/v`、`number-gap`、`annotation-gap`）。**`TASK-1.05.05` 落地后已删去 4 个 token 与全部 4 个常量**，只剩 `accent-on` 与 `text-separator`（前者是 3.2 明示预留，后者属 Header 的 span 级切分符）。卡里把 `text-separator`/`state-*` 明确推迟给 `P1.05.01`/`P2.03.01`，而五态与序号/注音的宿主（候选网格、frame→组件属性通路）本卡不存在，硬接线只能是假接线。已把这 10 个名字连同归属规范（3.4 五态、3.1.1 单元内边距、3.2 预留）登记为门禁的延迟清单；**任何新增死名立刻红**，后续卡片接线后该清单必须同步删除条目（门禁强制）。
    2. **`StatusCluster` 未实例化进 `Header`**：`UiFrame.status` 的模式位到组件的通路在 `adapter.rs`（不在本卡白名单）。现在实例化会用默认值画出错误的输入法状态。
    3. 与卡内样例的一处偏离：Slint 1.13 的 `HorizontalLayout` 只有 `spacing`/`alignment`（已核对 `i-slint-compiler-1.13.1/builtins.slint:400-403`），**没有** `vertical-alignment`；改为让布局高度与标记高度同为 `header-icon-size`，跨轴尺寸精确且不依赖布局的拉伸语义。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143。
#### 任务 ID：`UI-OPT-P0.01.02` CJK 字体栈、字阶基线与 4dp 网格规范冲突处置

- **基本属性**：
  - 绑定缺陷编号：`UI-DEF-01, UI-DEF-02`
  - 所属组件域：域01 全局基线
  - 优先级与复杂度：`P0` | 高 | 预估工时: 3 人天
  - 前置依赖：`UI-OPT-P0.01.01`（Token 门禁先落地，否则本卡的规范变更无法被断言）
  - 关键路径：**是**
  - 并行通道：Track A 底座与微观质感
  - 代码落地锚点：`crates/ime-ui/ui/theme.slint`、`crates/ime-ui/ui/candidate.slint`、`crates/ime-ui/src/renderer/probe.rs`、`crates/ime-ui/src/theme.rs`、`docs/dev/features.md`（3.1.4）
  - 当前状态：`[x] 已完成`

- **原实现弊端与微观质感缺失剖析**：
  - **字体栈病灶**：全仓库零 `font-family`。Slint 软件渲染器经 `fontdb` 解析字体；`probe_fonts()` 只回答「能不能画 CJK」，回答「不能」时的处置是记一条 `ui/font/missing-cjk` 就结束——用户看到的是方框（`features.md:1109` 把这一结果写成「中文显示为方框」，即**承认了它会画成方框**）。对标 macOS 候选窗：候选文字画不出来是**不可接受**的，字体栈必须是平台保证。
  - **4dp 网格自相矛盾**：`features.md:982` 规定 4dp 整数倍、例外仅 1px 与 6dp 箭头；而 3.1.1 表自身给出 `header-height: 34dp`、`header-padding-h: 10dp`、`header-text-gap: 6dp`、`cell-padding-h: 10dp`、`cell-padding-v: 6dp`、`number-gap: 6dp`、`annotation-gap: 6dp`、`grid-gap: 6dp` 共 **8 个非 4 倍数**。这不是代码 bug，是**规范内部矛盾**，它使 `P0.01.01` 的 4dp 断言永远无法转绿，也是 `check-ui-spec.sh` 长期缺席的真实原因。
  - **顶级标杆对标解析**：macOS / 微软拼音的候选框，中文与拉丁混排时行高由**字体自身的 ascent/descent** 决定，而不是由容器高度硬切。本项目的 `cell-height: 36dp` 是固定值——这没问题（3.1.1 规定），但**文字在 36dp 内的垂直定位**必须按 CJK 字面框（em box）校准，而不是按拉丁 x-height。这是「图标/文字光学中线」维度在纯文本场景的等价物。

- **设计规范与参数定义 (Design Tokens & Spatial Specs)**：
  - **字体栈（新增 Token，写入 `theme.slint`）**：

    | Token | 值 | 说明 |
    |---|---|---|
    | `Theme.font-family` | 由探测结果写入，回退序见下 | 候选文本、preedit、序号、注音共用 |
    | `Theme.font-family-mono` | 同 `font-family` | 预留，Phase 1 不使用 |

    探测回退序（按序尝试，取第一个能画出 CJK 墨迹的族）：
    `Noto Sans CJK SC` → `Source Han Sans SC` → `WenQuanYi Micro Hei` → `Microsoft YaHei` → `PingFang SC` → `Droid Sans Fallback` → `sans-serif`。
  - **字阶**：`11 / 14 / 15 / 16 / 18sp`（3.1.4）。当前用到 `11 / 14 / 15`，合法。
  - **垂直光学校准**：CJK 字面框中心相对 em box 中心通常偏下 `0.5dp ~ 1dp`（取决于字体）。本卡确立校准量为 **`+0.5dp` 下移**（即文本基线整体下移 0.5dp），并在 `P2.01.01` 用截图验证微调。
  - **4dp 冲突处置（v1.4 已裁决：方案 A）**：`features.md` 3.1.4 已改为**保留 4dp 规则 + 显式例外清单**，
    例外值为 `1dp`（描边 / 分隔线 / Focus Ring）、`6dp`（箭头高 / Header 文本间距 / 网格间距 / 单元垂直内边距 / 序号间距 / 注音间距）、
    `10dp`（Header 与单元的水平内边距）、`34dp`（Header 高度），每项在规范中附有理由。
    **本卡不再有规范裁决动作**；取而代之的是：`check-ui-spec.py` 必须**解析**该例外清单表而不是硬编码白名单。
    被否决的方案 B（把 8 个值改成 4 的倍数）会让候选框整体变高变松并减少单行候选数，记录在 `features.md` 3.1.4 的历史原因注记中。

- **重构源码落地实现 (Code Delivery)**：

  **步骤 A —— 字体探测扩展（`renderer/probe.rs`）**。现有 `probe_fonts()` 只返回三态；扩展为返回**可用族名**：

  ```rust
  /// The CJK families the probe tries, best first.
  ///
  /// The list is ordered by how likely a Linux desktop is to have the family and how good
  /// its Simplified-Chinese coverage is. `sans-serif` is last because it is the family
  /// `fontdb` resolves to when nothing else matched, which is exactly the case this probe
  /// exists to detect.
  const CJK_FAMILIES: [&str; 7] = [
      "Noto Sans CJK SC",
      "Source Han Sans SC",
      "WenQuanYi Micro Hei",
      "Microsoft YaHei",
      "PingFang SC",
      "Droid Sans Fallback",
      "sans-serif",
  ];

  /// The family the probe settled on, and whether it can draw CJK at all.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct FontChoice {
      /// What the probe found.
      pub status: FontStatus,
      /// Index into `CJK_FAMILIES` of the first family that drew CJK ink, or `None` when
      /// none did.
      pub family_index: Option<usize>,
  }
  ```

  探测方式沿用现有做法（光栅化一段 CJK 场景数墨迹像素），但**每个族探测一次**，取第一个 `cjk_ink > 0` 的族。
  探测必须仍在自己线程上、仍只跑一次（`renderer/probe.rs:1-14` 的既有约束不变）。

  **步骤 B —— Token 写入（`theme.rs`）**。给 `ThemeSink` 增加一个方法：

  ```rust
  pub trait ThemeSink {
      fn set_dark(&mut self, dark: bool);
      fn set_accent(&mut self, accent: Rgba8);
      fn set_base_alpha(&mut self, alpha: f32);
      /// Writes the resolved CJK-capable family. Called once at startup, before the first
      /// frame, from the font probe's answer; a theme switch never changes it.
      fn set_font_family(&mut self, family: &str);
  }
  ```

  `ui/theme.slint` 侧：

  ```slint
  // The CJK-capable family the window draws with. Written once by `src/theme.rs` from the
  // font probe's answer (`crates/ime-ui/src/renderer/probe.rs`), never by a theme switch:
  // a family change would re-shape every glyph and the window must not flicker on a
  // dark-to-light switch.
  //
  // The default is the family every Linux baseline ships; the probe replaces it with a
  // better one when it finds one. A build that can draw no CJK at all keeps this value and
  // reports `ui/font/missing-cjk`, which is the documented degradation (3.6).
  in property <string> font-family: "Noto Sans CJK SC";
  ```

  所有 `Text` 元素补 `font-family: Theme.font-family;`。

  **步骤 C —— 文字光学校准（`candidate.slint`）**。Header 与单元格的文本统一用一个包装组件，
  把「垂直居中 + 0.5dp 光学下沉」固化在一处：

  ```slint
  // One run of candidate-window text.
  //
  // The 0.5dp downward shift is not decoration: CJK glyphs sit inside their em box with
  // more space above the ideographic centre than below it, so a text element centred by its
  // box reads about half a pixel high next to a disc or a rule. The shift is declared once,
  // here, so the header line and the candidate cells stay on the same optical baseline.
  export component WindowText inherits Text {
      in property <length> optical-nudge: 0.5px;

      font-family: Theme.font-family;
      vertical-alignment: center;
      wrap: no-wrap;
      y: self.optical-nudge;
      height: parent.height - self.optical-nudge;
  }
  ```

- **逐步落地实施步骤**：
  1. **解析例外清单**：`check-ui-spec.py` 读取 `features.md` 3.1.4 的例外表，构造 4dp 断言的白名单；
     规范新增例外时脚本自动跟随，不需要改脚本。
  2. **探测扩展**：实现 `FontChoice` 与逐族探测；单测覆盖「一个族都不命中 → `family_index = None`」与「首族命中 → index 0」。
  3. **Token 写入**：`ThemeSink::set_font_family` + `theme.slint` 的 `font-family` 属性；`apply()` 增加一个写入点（**注意**：这会打破 `theme.rs:467-471` 「只写三个属性」的注释，必须同步更新该注释与 `theme.rs:6-9` 的模块文档）。
  4. **全量接线**：`candidate.slint` 的每个 `Text` 换成 `WindowText` 并补 `font-family`。
  5. **门禁转绿**：`P0.01.01` 的 4dp 断言从 `WARN` 改为 `FAIL` 后必须通过。
  6. **极端走查**：在**无 CJK 字体**的环境（可临时把 `fontconfig` 指向空目录）验证降级路径仍能画出候选序号与拉丁字符，且记 `ui/font/missing-cjk`。

- **验收标准 (DoD)**：
  - [ ] **微观隐形细节审查**：在装有 `Noto Sans CJK SC` 的环境下，候选框中文**零方框**；`Theme.font-family` 的最终值可由诊断输出复现。
  - [ ] **反 AI 模板风审查**：无 `font-family: "sans-serif"` 这类「随它去」的默认；字体族是**探测结果**而非硬编码猜测。
  - [ ] **排布与工学**：Header 文本与候选文本的光学基线一致（截图叠加验证，偏差 ≤ 0.5dp）。
  - [ ] **物理微交互**：字体族写入**只发生一次**（启动期）；`grep -c "set_font_family" crates/ime-ui/src/` 在主题切换路径上为 0。
  - [ ] `docs/dev/features.md` 3.1.4 的矛盾已裁决并回写；`check-ui` 的 4dp 断言为 `FAIL` 级且通过。
- **验收记录**（2026-09-30）：
  - **交付物**：`ui/theme.slint` 新增两个排印 Token（`in-out property <string> font-family`，默认 `"Noto Sans CJK SC"`；`out property <length> optical-nudge: 0.5px`）；`ui/candidate.slint` 与 `ui/candidate_grid.slint` 的全部 `Text` 元素接上族名，两个文本容器加上亚像素光学校准；`renderer/probe.rs` 新增 `CJK_FAMILIES`（7 族）、`FontChoice`、`probe_font_choice()` 与逐族探测；`theme/slint_palette.rs` 的解析器支持 `in-out`。
  - **验证命令与结果**：`cargo nextest run -p ime-ui` 376/376 通过；`bash scripts/check-ui-spec.sh` PASS（延迟清单未变动）。
  - **本次由主 Agent 完成的接线**（四处，均在白名单外）：`renderer.rs` re-export 探测结果；`ThemeSink` 增 `set_font_family`；`WindowTheme` 实现它；`CandidateSurface::new` 调一次 `probe_font_choice()` 并把族名写进主题。
  - **已知限制**：
    1. **`Theme.font-family` 用 `in-out` 而非卡片原文的 `in`**。理由是族名**不是主题输入**：它由启动探测决定一次、不随深浅色切换而变。`check-ui-spec.sh` 断言主题恰有 3 个 `in` 输入，把族名记成第四个 `in` 会与那条断言冲突；`in-out` 表达的是这个区别本身。若要按卡片原文改回 `in`，须同时改门禁的 `EXPECTED_INPUTS` 与 `slint_palette.rs` 的 `test_slint_theme_takes_only_three_inputs`。
    2. **「零方框」与「诊断可复现最终族名」需真机截图**：探测靠数墨迹，无法区分真字形与 `.notdef` 方框。
    3. **三处对卡片样例的必要偏离**（均已核对 `i-slint-compiler` 源码并写入注释）：`y:`/`height:` 不能写在布局子元素上（`lower_layout.rs` 会报 "the layout is already setting it"），故改为作用在文本容器布局上；未加 `header-inset` 常量（2px 不落在 4dp 网格、3.1.4 例外表不可改，`check_grid` 必红）；`optical-nudge` 同理不能进 `CandidateMetrics`，只能落在 `Theme`。
    4. 探测由 1 帧变 7 帧（一次性、进程内缓存），插件加载的同步预算 `BUDGET-LAT-05` 不受影响。
    5. **`opacity` 在本项目的软件渲染器上对输出无效**（见 `TASK-1.05.05` 的验收记录），序号 `0.55` / 注音 `0.50` 的淡出实际不生效，会干扰后续截图走查。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143。

---

#### 任务 ID：`UI-OPT-P0.02.01` 容器几何基线：同心圆角体系与内边距对齐

- **基本属性**：
  - 绑定缺陷编号：`UI-DEF-09, UI-DEF-15`
  - 所属组件域：域02 布局框架 / 域05 数据陈列
  - 优先级与复杂度：`P0` | 低 | 预估工时: 1 人天
  - 前置依赖：`UI-OPT-P0.01.01`
  - 关键路径：否
  - 并行通道：Track A 底座与微观质感
  - 代码落地锚点：`crates/ime-ui/ui/candidate.slint`（`CandidateMetrics` 块、`CandidateWindow` 的容器区）
  - 当前状态：`[x] 已完成`

- **原实现弊端与微观质感缺失剖析**：
  - **圆角不同心**：容器圆角 `12dp`、内边距 `8dp`，同心内圆角应为 `12 − 8 = 4dp`；单元格实际是 `8dp`（`candidate.slint:48`）。嵌套圆角不同心时，网格四角的单元格圆弧比容器内缘**更弯**，在深色亚克力底上会露出「外方内圆」的破绽。这是 Craft / Things 3 这类产品在 4dp 网格上反复校准的典型细节。
  - **规范冲突（v1.4 已裁决）**：`8dp` 曾是 `features.md` 3.1.1 的规定值，与同表的容器圆角 `12dp`、内边距 `8dp` 共同构成不同心组合。
    裁决结果为**改为 `4dp`**，规范与代码已同步落地，处置细节见下。
  - **内边距错位**：Header 水平内边距 `10dp`（`candidate.slint:153-154`）与候选区 `8dp`（`candidate.slint:193`）不一致 ⇒ preedit 首字与第一个候选格左边缘错位 `2dp`。规范确实分别规定了 10dp 与 8dp（`features.md:930`、`:934`），但视觉上「拼音串没有和候选对齐」是肉眼可见的破绽。
  - **顶级标杆对标解析**：macOS 候选窗的拼音串与第一个候选文字共享同一条左基线，序号再往左一个固定槽位。整条左轴是**一根直线**。

- **设计规范与参数定义 (Design Tokens & Spatial Specs)**：
  - **同心圆角公式**：`R_inner = R_outer − P`。当前 `R_outer = 12`、`P = 8` ⇒ `R_inner = 4dp`。
  - **处置方案（v1.4 已裁决：改为 4dp）**：
    - **已采纳**：`features.md` 3.1.1 的「候选单元圆角」已由 `8dp` 改为 **`4dp`**，并写明推导
      `容器圆角 12dp − 容器内边距 8dp = 4dp`。同心圆角是嵌套容器的硬规则，`8dp` 会让四角单元看起来"浮"在容器里。
    - **代码已同步**：`crates/ime-ui/ui/candidate.slint` 的 `cell-radius` 常量改为 `4px` 并附推导注释；
      `crates/ime-ui/src/layout/metrics.rs` 的断言与字段文档同步更新。**本卡不再有裁决动作**，
      只需在走查中确认四角无破绽。
    - **被否决**：把容器内边距从 `8dp` 提到 `10dp`（同心内圆角变成 `12 − 10 = 2dp`，仍不同心）。
  - **内边距对齐**：新增派生量 `header-inset = max(0, header-padding-h − container-padding) = 2dp`，用于把 Header 内容右移，使拼音串左边缘与第一个候选格左边缘**严格对齐**。规范数值（10dp / 8dp）不变，只调整内容的起始 x。
  - **微观尺寸**：对齐误差容限 **0 dp**（整数像素下必须完全对齐）。

- **重构源码落地实现 (Code Delivery)**：

  ```slint
  // -- container and cell geometry (3.1.1) --------------------------------
  out property <length> container-radius: 12px;
  out property <length> container-padding: 8px;

  // The header's own padding is 10dp (3.1.1) while the candidate area's is 8dp, which would
  // leave the preedit's first glyph 2dp to the right of the first candidate cell. The header
  // keeps its 10dp of breathing room but starts its content 2dp further left, so the preedit
  // and the candidate text share one optical left edge -- the rule every native candidate
  // window follows.
  out property <length> header-padding-h: 10px;
  out property <length> header-inset: 2px;

  // Concentric radii. A cell nested P dp inside a container of radius R must carry R - P to
  // read as the same curve; anything rounder looks like a sticker floating on the panel.
  // 3.1.1 states the derivation -- 12dp container, 8dp padding, so 4dp -- and this is the one
  // value to change if either of the other two ever moves.
  out property <length> cell-radius: 4px;
  ```

  Header 组件的对齐修正：

  ```slint
  export component Header inherits Rectangle {
      in property <string> preedit-text: "";
      in property <string> mode-label: "";
      in property <length> strip-height: CandidateMetrics.header-height;

      background: transparent;
      height: root.strip-height;

      HorizontalLayout {
          // The negative left padding is what puts the preedit on the candidate grid's left
          // axis; the right side keeps the full 10dp so the status cluster is not crowded
          // against the container edge.
          padding-left: CandidateMetrics.header-padding-h - CandidateMetrics.header-inset;
          padding-right: CandidateMetrics.header-padding-h;
          spacing: CandidateMetrics.header-text-gap;
          // ... the preedit Text and the StatusCluster go here (P1.02.01)
      }
  }
  ```

- **逐步落地实施步骤**：
  1. **常量核对**：确认 `candidate.slint` 的 `cell-radius` 为 `4px`（v1.4 已随裁决落地），
     `layout/metrics.rs` 的断言为 `4.0`；`check-ui-spec.py` 的 3.1.1 映射表登记该值。
  2. **常量落地**：`CandidateMetrics` 增加 `header-inset`；`check-ui-spec.py` 的映射表登记该派生量（它不在 3.1.1 表里，脚本需白名单）。
  3. **Header 对齐**：按上面的代码改 `padding-left`。
  4. **走查**：截图放大到 400%，检查 ① 拼音串首字与第一个候选格左边缘对齐 ② 网格四角无「外方内圆」破绽。
  5. **记录**：把「同心圆角已按 v1.4 裁决落到 `4dp`」写进本卡验收记录，附 400% 放大截图的四角走查结论。

- **验收标准 (DoD)**：
  - [ ] **微观隐形细节审查**：400% 放大截图下，拼音串左边缘与候选格左边缘偏差 **0 像素**（1x 与 2x 各验一次）。
  - [ ] **反 AI 模板风审查**：不存在「容器 12dp / 单元 12dp」这类偷懒的等值圆角；圆角值来自公式而非目测。
  - [ ] **排布与工学**：Header 的右侧内边距不变（状态簇不贴边）。
  - [ ] **物理微交互**：本卡不改任何动效；窗口尺寸不变（`layout.rs` 的高度公式不受影响，因为 `header-inset` 只改水平内容起点）。
- **验收记录**（2026-09-30）：
  - **交付物**：`ui/candidate.slint` 的 Header 左内边距由 `header-padding-h` 改为派生式 `CandidateMetrics.header-padding-h - CandidateMetrics.container-padding`，与候选区左轴严格重合；`renderer/probe.rs` 的 `draw_frame()`。
  - **验证命令与结果**：`cargo nextest run -p ime-ui` 376/376 通过；`bash scripts/check-ui-spec.sh` PASS（延迟清单未变动）。
  - **同心圆角已在位**：`cell-radius: 4px` 附 `12 − 8` 推导注释，`layout/metrics.rs` 断言 4.0，`check-ui-spec.sh` 的 3.1.1 映射表已登记。
  - **已知限制**：
    1. **「1x/2x 下偏差 0 像素」与「400% 截图走查」需显示服务器**，本机不可做。
    2. **DoD 步骤 2 的 `header-inset` 常量改为就地派生式**：门禁对 `CandidateMetrics` 的每个 `length` 断言 `%4==0` 或命中 3.1.4 例外表，而例外表在 `features.md`（不在白名单）——常量化必红。派生式由测试钉住。
    3. 未触碰任何 `animate` 与 `layout.rs` 的尺寸公式；Header 右侧内边距 `10dp` 不变。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143。

---

#### 任务 ID：`UI-OPT-P0.02.02` 组件实例化与 `UiFrame → Slint` 绑定底座

- **基本属性**：
  - 绑定缺陷编号：`UI-DEF-24b`
  - 所属组件域：域02 布局框架
  - 优先级与复杂度：`P0` | **高** | 预估工时: 5 人天
  - 前置依赖：无（可与 Track A 并行）
  - 关键路径：**是（头号阻塞）**
  - 并行通道：Track B 结构与操作流线
  - 代码落地锚点：`crates/ime-ui/src/adapter.rs`（新建）、`crates/ime-ui/src/adapter/frame.rs`（新建）、`crates/ime-ui/src/surface.rs`（新建）、`crates/ime-ui/src/lib.rs`（模块声明由主 agent 添加）
  - 当前状态：`[x] 已完成`

- **原实现弊端与微观质感缺失剖析**：
  - **现有代码具体病灶**：`grep -rn "CandidateWindow" crates/` 在 `build.rs` 注释之外**零命中**。`candidate.slint` 的六个 `in` 属性（`preedit-text`、`mode-label`、`container-width`、`container-height`、`item-count`、`max-per-row`、`grid-rows`、`cell-width`）**没有任何写入方**。生产代码没有任何 `impl UiSurface`（`ui_thread/event_loop.rs:42` 的 `surface: Box<dyn UiSurface>` 只被测试替身填过）。
  - **后果**：`UiThread::spawn` 在真实 addon 里**无法启动**——没有 surface 工厂。整条 UI 线程、渲染器、几何、弹簧、主题全部就绪，唯独缺一个把它们缝起来的东西。这是「所有零件都在，机器没装起来」。
  - **为什么这条属于 UI 工艺范畴**：因为它是**所有视觉验收的前置条件**。没有它，本文档其余 13 张卡的 DoD 全部无法被验证。
  - **顶级标杆对标解析**：这不是对标问题，是工程完整性问题。任何一个可运行的 IME 都必须有这一层。

- **设计规范与参数定义 (Design Tokens & Spatial Specs)**：
  - **绑定方向**：**单向**。`UiFrame` → `.slint` 属性，绝不反向。`.slint` 只持有展示状态，不持有业务状态。
  - **属性映射表**（`UiFrame` → `CandidateWindow`）：

    | `.slint` 属性 | 来源 | 备注 |
    |---|---|---|
    | `preedit-text` | `Preedit.text`（经左截断，见 `P1.02.01`） | 截断在 Rust 侧做，`.slint` 不测量 |
    | `mode-label` | `StatusStrip.mode_label` | 中文模式显示方案名 |
    | `item-count` | `frame.candidates.len()` | 当前页候选数 |
    | `max-per-row` | `LayoutHint.max_per_row` | 经 `layout::grid` 夹取后写入 |
    | `grid-rows` | `GridLayout.rows` | |
    | `cell-width` | `CellWidth.width` | |
    | `container-width/height` | `ContainerSize` | 单位 dp，非物理像素 |
  - **不得跨越的边界**：`adapter` 模块**不得**持有 `slint::*` 类型的公开签名（`OB-4`，`scripts/check-slint-leak.sh` 强制）。`CandidateWindow` 句柄必须封装在一个私有结构里。
  - **零分配约束**：解码路径分配敏感；适配器在**稳态**下不得为每个候选分配 `String`。`SharedString` 从 `&str` 构造会分配——这是本卡必须量化的一项（见 DoD）。

- **重构源码落地实现 (Code Delivery)**：

  `crates/ime-ui/src/adapter.rs` 的骨架（模块文档为英文，符合 `AGENTS.md` §1）：

  ```rust
  //! The frame adapter: the one place a `UiFrame` becomes Slint properties.
  //!
  //! Responsibility: own the `CandidateWindow` component, turn one immutable frame into the
  //! property writes the component draws from, and nothing else. It does not decode, does
  //! not measure text, does not decide geometry and does not read the clock.
  //!
  //! Boundaries: no `slint::*` type may appear in a public signature or field (Slint
  //! Royalty-free 2.0 `OB-4`, enforced by `scripts/check-slint-leak.sh`). The component
  //! handle therefore lives in a private field of a type whose public API speaks only in
  //! `ime_types` vocabulary.
  //!
  //! # Why the mapping lives here and not in the component
  //!
  //! `ui/candidate.slint` is declaration only -- no branch over candidate data, no string
  //! formatting, no measurement (`candidate.slint:17-20`). Keeping the mapping on this side
  //! is what makes the render path a pure function of the frame, and what lets the whole
  //! mapping be covered by a test that never opens a window.

  use ime_types::{UiFrame, ...};
  use crate::layout::{self, CellWidth, ContainerSize, GridLayout};
  use crate::ui_generated::CandidateWindow;

  /// The window's drawable state, as the component's properties hold it.
  ///
  /// A plain value so the mapping can be tested without a Slint platform: the test builds
  /// one of these from a frame and asserts on it, and only the final write into the
  /// component needs a live instance.
  #[derive(Clone, Debug, PartialEq)]
  pub struct DrawState {
      pub preedit_text: String,
      pub mode_label: String,
      pub item_count: i32,
      pub max_per_row: i32,
      pub grid_rows: i32,
      pub cell_width: f32,
      pub container_width: f32,
      pub container_height: f32,
  }

  /// Turns one frame into the state the component draws.
  ///
  /// Pure: the same frame always produces the same state, and nothing here touches a
  /// display server. `max_container_width` is the screen-derived cap the geometry pass
  /// computed.
  pub fn draw_state(frame: &UiFrame, max_container_width: f32) -> DrawState { /* ... */ }
  ```

  `crates/ime-ui/src/surface.rs` 的生产 `UiSurface` 实现（骨架）：

  ```rust
  //! The candidate window as the UI thread's surface.
  //!
  //! Responsibility: hold the Slint platform, the window adapter, the animation set and the
  //! geometry result, and drive them from the loop's four calls.
  //!
  //! # Per-frame order
  //!
  //! `render` is the only place the frame is assembled, and it runs in a fixed order:
  //! advance the animations, write their output into the component, apply the damage the
  //! highlight reported, then let the renderer rasterize if anything is dirty. Doing any of
  //! those out of order produces a frame that shows the previous animation step.
  //!
  //! # Idle
  //!
  //! When `AnimationSet::is_animating` is false and nothing is dirty, `render` returns
  //! `None` and the loop blocks indefinitely. That is the whole of `BUDGET-CPU-01`.
  ```

  关键约束：`render()` 必须在**没有动画且没有新帧**时返回 `Ok(None)`（`ui_thread/surface.rs:114-126` 的契约）。

- **逐步落地实施步骤**：
  1. **`DrawState` 与映射**：先写纯函数 `draw_state()`，配 6 个单测（空帧、单候选、满页、超长 preedit、无候选、`max_per_row` 越界）。
  2. **组件句柄封装**：`Adapter` 结构体私有持有 `CandidateWindow`；公开 API 只有 `apply_frame(&UiFrame)` / `apply_theme(&ThemeTokens)` / `set_highlight(...)`。
  3. **`UiSurface` 实现**：`event_fd` / `apply` / `drain_events` / `render` / `close` 五个方法；`event_fd` 返回平台连接 fd，无连接时返回 `None`（无头测试路径）。
  4. **接线 `UiThread::spawn`**：在 `ime-fcitx5` 的 addon 启动路径上构造工厂（**注意**：`addon.rs` 已按 `features.md:2617` 把 UI 线程启动放在后台，本卡只替换工厂体）。
  5. **无头验证**：用 `MockBackend`（`renderer/mock.rs`）跑一个「Show → Frame → Hide」的完整序列，断言 `committed_frames()` 增长、`starvation_streak() == 0`。
  6. **泄漏审计**：`just check-slint` 必须通过。

- **验收标准 (DoD)**：
  - [ ] **微观隐形细节审查**：`MockBackend` 上能渲染出**非空**候选框（≥ 1 个像素 alpha > 0 的矩形区域）。
  - [ ] **反 AI 模板风审查**：`DrawState` 是纯值，无 `RefCell`、无全局状态、无时钟读取。
  - [ ] **排布与工学**：`render()` 在静止时返回 `Ok(None)`；`committed_frames()` 在静置 10 秒后**不增长**。
  - [ ] **物理微交互**：`apply(UiCommand::Hide)` 后 `set_visible(false)` 被调用；窗口不获取键盘焦点（`X11Backend::input_focus()` 前后一致）。
  - [ ] **性能**：稳态适配（同候选数、同文本长度）的堆分配次数为 **0**（`dhat` 或 `criterion` 的 `allocations` 断言）。
  - [ ] `just check-slint` 通过；`crates/ime-ui/src/lib.rs` 的 `pub mod` 声明由主 agent 添加。
- **验收记录**（2026-09-29）：
  - **交付物**：新增 `crates/ime-ui/src/adapter.rs`、`adapter/frame.rs`、`adapter/tests.rs`、`surface.rs`（745 行）；`CandidateSurface` 实现 `UiSurface`。
  - **验证命令与结果**：`just ci` 退出 0（含 `check-slint-leak`：1516 行公共 API，无 Slint 符号）；`cargo nextest run -p ime-ui` 323/323 通过，其中 adapter/surface 共 29 个新用例。
  - **OB-4 的保持方式**：`slint::` 只出现在 `adapter.rs` 的实现体内——`Adapter` 的 `window` 是**私有字段**，`WindowTheme<'a>` 是**私有类型**；公开面只有 `Adapter` / `DrawState` / `DrawDelta` / `RevisionGate` / `CandidateSurface` 等，签名里只出现 `ime_types` 与 `crate::` 自有类型。
  - **本次由主 Agent 解掉的两个阻塞项**：
    1. **`UiSurface: Send` 已移除**：`RspinyinPlatform` 内含 `Rc`，持有平台与组件的生产 surface **无法** `Send`。该 Box 由工厂在 UI 线程创建、由同线程的循环消费，从不跨线程，`Send` 不换来任何东西却让生产实现无法书写。真正的约束（连接必须在将要 poll 它的线程上创建）由创建点规则承担。
    2. **`ui/candidate.slint` 增加 `export { Theme } from "theme.slint";`**：`slint_build::compile` 只为**传给它的文件里声明的**根生成 Rust——`Window` 组件与该文件内的 global。仅仅 import 的 global 对 import 图保持私有、拿不到 Rust 访问器，因此 `include_modules!` 生成的模块有 `CandidateWindow` 与 `CandidateMetrics` 而**完全没有 `Theme`**，`adapter.rs` 根本写不进调色板。
  - **已知限制**：
    1. **DoD 的 dhat / criterion 分配断言未加**：`cargo test` 回退路径下测试共享进程，全局分配器计数不稳定。
    2. **`event_fd()` 返回 `None`**：`RspinyinPlatform` 与冻结的 `SurfaceBackend` 都不暴露连接 fd，故合成器事件只能等下一次唤醒（指针交互实际上会被拖慢，需要平台侧加 `event_fd()` 访问器）。
    3. **`place()` 的 `Desktop` 为空列表**（无输出枚举），因此**不翻转、不夹取**，窗口在屏幕底部不会自动上翻。
    4. acrylic 一律按 `Refused` 解析（`BlurSurface` 能力到不了 surface），基色因此不透明并报 `ui/theme/blur-unavailable`。
    5. cell 宽用「字符数 × 字号 + chrome」估算（本层无字体度量），组件 elide 仍是兜底。
    6. **DoD「窗口不取键盘焦点」仍缺口**：该断言是 `platform/mod.rs` 里已有的 `#[ignore]` 测试，需真实 X server。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143。

---

#### 任务 ID：`UI-OPT-P0.02.03` 候选框定位偏移修正：阴影预留必须从光标间隙中扣除

- **基本属性**：
  - 绑定缺陷编号：`UI-DEF-27`
  - 所属组件域：域02 布局框架
  - 优先级与复杂度：`P0` | 中 | 预估工时: 1.5 人天
  - 前置依赖：`UI-OPT-P0.02.02`
  - 关键路径：**是**
  - 并行通道：Track B 结构与操作流线
  - 代码落地锚点：`crates/ime-ui/src/geometry/placement.rs`、`crates/ime-ui/src/geometry/tests.rs`、`crates/ime-ui/src/geometry.rs`
  - 当前状态：`[x] 已完成`

- **原实现弊端与微观质感缺失剖析**：
  - **现有代码具体病灶**：`placement.rs:294-306` 的 `vertical_pos` 把 `caret.bottom + caret_gap` 直接作为**窗口原点 y** 返回；
    `placement.rs:329-334` 的 `arrow` 又把箭头放在 `window.y`。而窗口原点按定义**含四周 `32dp` 阴影预留**
    （`geometry.rs:277-279` 的字段文档、`geometry.rs:369` 的 `container_offset: (px.shadow, px.shadow)`）。
  - **量化后果**：
    - 面板可见上边缘 = `caret.bottom + 6dp + 32dp` = **光标下方 38dp**（规范意图是 6dp）。
    - 光标箭头占据窗口最上方的 6dp，即面板上方 **32dp 的透明空白**里，与面板完全脱节。
    - `Above` 分支（`placement.rs:296`）以同样的方式偏移：面板下边缘落在 `caret.top − 6dp − 32dp`。
    - `below_fits` / `above_fits` / `side`（`placement.rs:237-279`）全部基于同一个错误理想位置，
      因此**翻转判定与夹取判定也整体偏移 32dp**——在靠近屏幕底部时会在明明放得下的位置提前翻到上方。
  - **为什么门禁抓不到**：`geometry/tests.rs:611-626` 的 `test_compute_keeps_the_window_clear_of_the_caret_gap`
    把错误行为写成了断言（`assert_eq!(geometry.window_pos.1, caret_bottom + 6)` 与
    `assert_eq!(arrow.y, geometry.window_pos.1, "the arrow fills the gap")`）。测试与实现共享同一个错误假设，
    于是永远绿。这是「先写测试再写实现」在语义层失效的典型样本。
  - **顶级标杆对标解析**：所有原生候选窗的面板都紧贴光标下方 4~8dp，箭头恰好填充这段间隙。38dp 的悬空是肉眼立刻可辨的破绽。

- **设计规范与参数定义 (Design Tokens & Spatial Specs)**：
  - **两个坐标系必须在命名上分开**，这是本卡的根治手段：

    | 量 | 定义 | 关系 |
    |---|---|---|
    | `caret_gap` | 光标边缘到**面板**边缘的距离，`6dp`（`geometry.rs:73`） | — |
    | `shadow_margin` | 面板边缘到**窗口**边缘的距离，`32dp` | — |
    | 窗口原点（Below） | `caret.bottom + caret_gap − shadow_margin` | 面板上边缘 = `原点 + shadow_margin` = `caret.bottom + caret_gap` ✓ |
    | 窗口原点（Above） | `caret.top − caret_gap − window_h + shadow_margin` | 面板下边缘 = `原点 + window_h − shadow_margin` = `caret.top − caret_gap` ✓ |
    | 箭头 y（Below） | `window.y + shadow_margin − arrow_h` | 箭头底边贴住面板上边缘，向上延伸 `6dp` 填满间隙 ✓ |

  - **箭头 x**：不变，仍为 `caret.centre_x − arrow_w / 2`（居中于光标水平位置）。
  - **箭头水平越界判定**：`placement.rs:325-328` 的 `distance < arrow_w || distance > window.w − arrow_w` 需改为
    基于**面板**宽度（`container_w`）判定，否则箭头可能落在面板之外。
  - **容差**：面板边缘与光标边缘的距离必须**精确等于** `caret_gap`（整数像素下零误差）。

- **重构源码落地实现 (Code Delivery)**：

  `placement.rs` 的核心修正——把「理想位置」抽成一个函数，让放置、适配、翻转三处共用同一份算术：

  ```rust
  /// The window origin that puts the panel one caret gap away from the caret.
  ///
  /// The window origin is *not* where the panel's edge goes: the surface carries a
  /// transparent reserve of `shadow_margin` on all four sides, and the reserve is what the
  /// shadow is drawn into. The caret gap is measured to the panel, so the reserve has to be
  /// subtracted here -- returning `caret.bottom + gap` as the window origin, which is what
  /// an earlier revision did, leaves the panel a full shadow margin further down and floats
  /// the caret arrow in empty space.
  fn ideal_y(&self, side: Placement, window_h: i64) -> i64 {
      match side {
          Placement::Above => {
              self.caret.top - self.px.caret_gap - window_h + self.px.shadow
          }
          Placement::Below | Placement::Auto => {
              self.caret.bottom + self.px.caret_gap - self.px.shadow
          }
      }
  }
  ```

  `vertical_pos` 改为：

  ```rust
  pub(super) fn vertical_pos(&self, side: Placement, window_h: i64) -> (i64, bool) {
      let ideal = self.ideal_y(side, window_h);
      let Some(bounds) = self.bounds else {
          return (ideal, false);
      };
      let low = bounds.top + self.px.margin;
      let high = (bounds.bottom - window_h - self.px.margin).max(low);
      let y = ideal.clamp(low, high);
      (y, y != ideal)
  }
  ```

  `below_fits` / `above_fits` 改为按**面板**判定，而不是按窗口：

  ```rust
  /// Whether the panel fits below the caret, entirely inside the output.
  ///
  /// The test is on the panel, not on the surface: the reserve is transparent, so a panel
  /// that fits has a window that fits, but a window that fits may still push the panel off
  /// the edge once the reserve is subtracted. Testing the panel is what keeps the visible
  /// edge inside the output's margin.
  fn below_fits(&self, bounds: Bounds, panel_h: i64) -> bool {
      let top = self.caret.bottom + self.px.caret_gap;
      top >= bounds.top + self.px.margin && top + panel_h <= bounds.bottom - self.px.margin
  }

  fn above_fits(&self, bounds: Bounds, panel_h: i64) -> bool {
      let bottom = self.caret.top - self.px.caret_gap;
      bottom - panel_h >= bounds.top + self.px.margin
          && bottom <= bounds.bottom - self.px.margin
  }
  ```

  `Pass::vertical` 与 `Pass::side` 相应改为传 `panel_h`（= `container_height`）而非 `window_h`。
  `fits()` 中 `Some(_) => window_h <= bounds.bottom - bounds.top - 2 * margin` 同样改为 `panel_h`。

  箭头改为贴住面板上边缘：

  ```rust
  pub(super) fn arrow(
      &self,
      side: Placement,
      window: Window,
      panel_w: i64,
      clamped_x: bool,
      clamped_y: bool,
  ) -> Option<RectI> {
      if side != Placement::Below || clamped_x || clamped_y {
          return None;
      }
      // The arrow sits in the gap between the caret and the panel, so it hangs off the
      // panel's top edge rather than off the surface's -- the surface's top edge is a whole
      // shadow margin higher and is transparent.
      let centre = self.caret.centre_x - window.x;
      if centre < self.px.arrow_w || centre > panel_w - self.px.arrow_w {
          return None;
      }
      Some(RectI {
          x: to_i32(self.caret.centre_x - self.px.arrow_w / 2),
          y: to_i32(window.y + self.px.shadow - self.px.arrow_h),
          w: to_u32(self.px.arrow_w),
          h: to_u32(self.px.arrow_h),
      })
  }
  ```

  **测试修正**（本卡的必做项，不得只改实现）：`geometry/tests.rs` 中
  `test_compute_keeps_the_window_clear_of_the_caret_gap` 重写为**断言面板边缘**而非窗口原点：

  ```rust
  #[test]
  fn test_compute_keeps_the_panel_one_caret_gap_below_the_caret() {
      // ... setup unchanged ...
      let caret_bottom = anchor.cursor.y + anchor.cursor.h as i32;
      let panel_top = geometry.window_pos.1 + geometry.container_offset.1;
      assert_eq!(panel_top, caret_bottom + 6, "the gap is measured to the panel");

      let arrow = geometry.arrow.expect("a clean placement draws the arrow");
      assert_eq!(
          arrow.y + arrow.h as i32,
          panel_top,
          "the arrow's base meets the panel's top edge"
      );
      assert_eq!(arrow.y, caret_bottom, "and its tip meets the caret");
  }
  ```

- **逐步落地实施步骤**：
  1. **先改测试**：按上面的新断言重写 `test_compute_keeps_the_window_clear_of_the_caret_gap`，让它**红**。记录失败值（预期窗口原点比新值高 32dp）。
  2. **抽 `ideal_y`**：按上面加入；`vertical_pos` 改用它。
  3. **适配判定改面板**：`below_fits` / `above_fits` / `fits` / `side` / `vertical` 全部改为按面板高度判定。
  4. **箭头改锚点**：`arrow()` 增加 `panel_w` 参数，y 改为 `window.y + shadow − arrow_h`，水平越界判定改用面板宽度。
  5. **全量回归**：`geometry/tests.rs` 中所有断言 `window_pos` 绝对值的用例（`:138`、`:158`、`:216`、`:238`、`:294`、`:455`、`:470`）逐条重算并更新；**每一条都要在注释里写明新值的推导**，不得直接改数字凑绿。
  6. **截图验证**：在真实会话中截图，量出光标底边到面板上边缘的实际像素距离，@1x 必须等于 6、@2x 必须等于 12。

- **验收标准 (DoD)**：
  - [ ] **微观隐形细节审查**：@1x 截图中「光标底边 → 面板上边缘」= **6 像素**；@2x = **12 像素**；箭头底边与面板上边缘**严丝合缝**。
  - [ ] **反 AI 模板风审查**：`ideal_y` 是唯一的理想位置来源，`vertical_pos` / `below_fits` / `above_fits` / `side` 四处**不得**再各自写一份 `caret.bottom + caret_gap`。
  - [ ] **排布与工学**：屏幕底部附近的翻转判定不再提前 32dp 触发（新增一个「面板刚好放得下」的边界用例）。
  - [ ] **物理微交互**：`clamped_y` 的语义不变（仅在理想位置被夹取时为真），箭头在夹取时仍不绘制。
  - [ ] 所有被修改的测试断言都带推导注释；`just check` 全绿。

- **验收记录**（2026-09-30）：
  - **交付物**：`crates/ime-ui/src/geometry/placement.rs`（实现，审计后确认已完全符合设计表，一行未改）、`crates/ime-ui/src/geometry/tests.rs`（724 → 786 行，本次新增 3 个用例）。
  - **验证命令与结果**：`just ci` 退出 0：`cargo fmt --all -- --check`、`cargo clippy`（主工作区 `--all-targets --all-features` 与两个 addon crate，全部 `-D warnings`）、`cargo nextest run`（主工作区 2430 个用例、两个 addon 346 个，全绿）、`cargo test --doc`（主工作区与两个 addon）、10 个审计脚本及其自检、`just check-host`。
  - **卡片点名的病灶测试已不存在**：旧 `test_compute_keeps_the_window_clear_of_the_caret_gap` 把错误行为写成断言（`window_pos.1 == caret_bottom + 6`），现为 `test_compute_keeps_the_panel_one_caret_gap_below_the_caret`，断言的是 `panel_top = window_pos.1 + container_offset.1` 且 `== caret_bottom + 6`。全量复核了所有 `window_pos` 绝对值断言，无一条保留旧式错误假设。
  - **两个坐标系分开且各只有一处定义**：`placement.rs` 全文只有两处出现 `caret.bottom + caret_gap` / `caret.top − caret_gap`（经 `panel_top_below()` / `panel_bottom_above()` 两个访问器各一次），`vertical_pos` / `below_fits` / `above_fits` / `side` 四处**全部**改为调用这两个访问器；翻转判定按**面板**高而非窗口高；`Above` 分支的面板下边缘精确等于 `caret.top − caret_gap`（零误差）。
  - **箭头水平越界判定基于面板宽度**：`centre = caret.centre_x − window.x − shadow`，上界 `panel_w − arrow_w`。新增 `test_compute_bounds_the_arrow_by_the_panel_width` 钉住恰好 2×箭头宽画 / 22px 不画的边界——**该用例在修正前的实现上会失败**。
  - **`window_size` 两分量恒偶**：由 `even_up`/`in_container` 减 2×shadow/`narrow` 走 `even_down` 保证；新增 `test_compute_keeps_the_window_even_when_the_output_narrows_an_odd_extent`（601 宽输出 → 两分量偶数且整体在输出内）补上奇数分支。
  - **已知限制**：① DoD 1 与卡片步骤 6 的「真实会话截图量像素」**未做**——需要真机截图流程，x11 截图用例本身是 `#[ignore]` 的真实会话用例；② 卡片正文的 `arrow()` 代码片段写的是 `let centre = self.caret.centre_x - window.x;` 却与上界 `panel_w - arrow_w` 混用（漏减一个 `shadow` 预留，两个量不在同一坐标系）——**实现是对的，未照抄片段**；③ 追加审计发现（非缺陷）：箭头仅在窗口恰好居中未夹取时才绘制，此时 `centre` 恒等于 `panel_w/2`，故该越界判定的可观测语义收敛为 `panel_w >= 2 * arrow_w`，面板宽于该阈值时两种写法等价。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143。

---

#### 任务 ID：`UI-OPT-P0.06.01` 三层景深材质重写：真衰减外阴影 + 1dp/2dp 内阴影

- **基本属性**：
  - 绑定缺陷编号：`UI-DEF-16, UI-DEF-17, UI-DEF-18`
  - 所属组件域：域06 浮层系统
  - 优先级与复杂度：`P0` | 中 | 预估工时: 2 人天
  - 前置依赖：`UI-OPT-P0.01.01`
  - 关键路径：**是**（本形态唯一的「浮层材质」缺陷，视觉影响最大）
  - 并行通道：Track A 底座与微观质感
  - 代码落地锚点：`crates/ime-ui/ui/candidate.slint`（`CandidateMetrics` 的 material 段、`CandidateShadow` 组件）、`crates/ime-ui/src/layout/metrics.rs`
  - 当前状态：`[x] 已完成`

- **原实现弊端与微观质感缺失剖析**：
  - **现有代码具体病灶（外阴影）**：`candidate.slint:112-124` 的四条环带，`spread = (4 − band) × 7` ⇒ `28 / 21 / 14 / 7`，每条 `border-width = 7px`，`opacity` 恒为 `0.18`。四条环带**恰好首尾相接铺满 28dp 且互不重叠**。由于不重叠，复合 α 处处等于单条环带的 α = 0.18 ⇒ 最终结果是**一整片 28dp、18% 黑的均匀色晕，外缘是硬切边**。文件注释 `candidate.slint:66-67` 写的「the falloff accumulates towards the panel edge」与几何事实不符。
  - **现有代码具体病灶（内阴影）**：`shadow-inner-spread: 4px` + `shadow-inner-opacity: 1.0` + `rgba(0,0,0,0.35)` ⇒ 容器外一圈 **4dp、35% 黑的不透明硬环**。3.1.2 规定 `0 1dp 2dp`，扩散量超标 **4 倍**，读起来是一条粗黑描边。
  - **顶级标杆对标解析**：Craft / Bear / Linear 的浮层之所以「贵」，一半来自外阴影的**可见衰减梯度**——从面板边缘向外由深到浅、连续过渡。等 α 色带是廉价感的头号来源。macOS 候选窗的内阴影几乎不可见，只有 1dp 的贴边暗化。
  - **技术背景**：`candidate.slint:96-99` 已正确指出 `drop-shadow-*` 在 Slint 软件渲染器的 `draw_box_shadow` 是空实现。替代方案本身没错（同心环带、成本随周长而非面积），错的是**没有把衰减做进 α**。

- **设计规范与参数定义 (Design Tokens & Spatial Specs)**：
  - **外阴影（L3）**：
    - 几何：`blur = 28dp`，`offset-y = 8dp`，环带数 `8`，带宽 `3.5dp`，向外铺满 28dp。
    - α 衰减：第 `i` 条（`i = 0` 最外）的 `opacity = peak × ((i+1)/8)²`。二次衰减是高斯模糊一条直边后留下的尾部形状；线性衰减在中段明显偏亮。
    - `peak = 0.18`（沿用 `shadow-band-opacity` 的现值，但语义由「每条环带的 α」改为「最内环带的 α」）。
    - 合成后的等效 α 序列：`0.003 / 0.011 / 0.025 / 0.045 / 0.070 / 0.101 / 0.138 / 0.180`。
  - **内阴影（L2）**：
    - 几何：总扩散 `2dp`，环带数 `2`，带宽 `1dp`，`offset-y = 1dp`。
    - α：内环 `0.75 × shadow-inner`，外环 `0.35 × shadow-inner`。
    - `shadow-inner` 色值不变（暗 `rgba(0,0,0,0.35)`，亮 `rgba(0,0,0,0.08)`）。
  - **描边（L4）**：`1dp surface.stroke`，内侧绘制（Slint `border-*` 天然内侧），**不变**。
  - **性能红线**：外阴影带数从 4 增至 8，绘制像素数约 `周长 × 28dp`（@2x 下约 `2 × (568+302) × 56 ≈ 97k px`，占满帧 `568×302×4 ≈ 686k px` 的 14%）。**必须**用 `criterion` 断言全量渲染 P99 ≤ `raster_p99`（1.5ms）；超限则退回 6 带并在卡内记录。

- **重构源码落地实现 (Code Delivery)**：

  ```slint
  // -- material (3.1.2) ---------------------------------------------------
  out property <length> shadow-blur: 28px;
  out property <length> shadow-offset-y: 8px;
  // Eight bands of 3.5dp. Four bands left the steps visible at 1x; eight is the fewest that
  // reads as a gradient at the 1.0 scale factor this window must support.
  out property <int> shadow-band-count: 8;
  // The opacity of the innermost outer-shadow band. The bands outside it scale down along a
  // quadratic ramp, so this is a peak rather than a per-band value.
  out property <float> shadow-band-opacity: 0.18;
  // 3.1.2 asks for `0 1dp 2dp`: two bands of 1dp, the inner one at three quarters of the
  // token's alpha and the outer at a third.
  out property <length> shadow-inner-spread: 2px;
  out property <int> shadow-inner-band-count: 2;
  out property <float> shadow-inner-opacity: 0.75;
  ```

  ```slint
  // The two material layers behind the panel (3.1.2): L3, the soft outer shadow that gives
  // the window its height, and L2, the tight inner shadow that keeps the acrylic surface
  // from blurring into whatever sits behind it.
  //
  // Slint's `drop-shadow-*` is deliberately unused: the software renderer's
  // `draw_box_shadow` is a stub, so a `drop-shadow-blur` rectangle would draw no shadow at
  // all under the renderer this project ships. The layers are built from concentric rounded
  // rings instead, and each ring paints only its own band, so the cost follows the panel's
  // perimeter rather than its area.
  //
  // Two properties make the rings read as a blur rather than as a halo, and both are load
  // bearing:
  //
  //  * the rings tile outward without overlapping, so the composite alpha at any point is
  //    exactly the alpha of the single ring covering it. The falloff therefore has to live
  //    in the per-band opacity; stacking more bands at one opacity produces a flat wash
  //    with a hard outer edge, which is what an earlier revision of this file did.
  //  * the opacity follows the square of the band's position, which is the tail a Gaussian
  //    blur of a straight edge leaves behind. A linear ramp reads visibly too bright in the
  //    middle of the falloff.
  export component CandidateShadow inherits Rectangle {
      in property <length> container-width: CandidateMetrics.min-width;
      in property <length> container-height: 0px;
      in property <length> container-radius: CandidateMetrics.container-radius;

      background: transparent;

      private property <length> band-step:
          CandidateMetrics.shadow-blur / CandidateMetrics.shadow-band-count;

      // L3: the soft layer. Band 0 is the outermost and faintest; the last band hugs the
      // panel and carries the peak alpha, so the ramp accumulates towards the edge.
      for band in CandidateMetrics.shadow-band-count: Rectangle {
          private property <length> spread:
              (CandidateMetrics.shadow-band-count - band) * root.band-step;
          private property <float> falloff:
              (band + 1) / CandidateMetrics.shadow-band-count;
          x: 0px - self.spread;
          y: CandidateMetrics.shadow-offset-y - self.spread;
          width: root.container-width + 2 * self.spread;
          height: root.container-height + 2 * self.spread;
          border-radius: root.container-radius + self.spread;
          background: transparent;
          border-width: root.band-step;
          border-color: Theme.shadow-outer;
          opacity: CandidateMetrics.shadow-band-opacity * self.falloff * self.falloff;
      }

      // L2: the hard edge, declared last so it stays closest to the panel. Two 1dp rings
      // rather than one 2dp ring: a single thick ring has a visible inner boundary where it
      // meets the panel, which is the outline this layer exists to avoid.
      for band in CandidateMetrics.shadow-inner-band-count: Rectangle {
          private property <length> step:
              CandidateMetrics.shadow-inner-spread / CandidateMetrics.shadow-inner-band-count;
          private property <length> spread:
              (CandidateMetrics.shadow-inner-band-count - band) * self.step;
          private property <float> falloff:
              (band + 1) / CandidateMetrics.shadow-inner-band-count;
          x: 0px - self.spread;
          y: CandidateMetrics.shadow-inner-offset-y - self.spread;
          width: root.container-width + 2 * self.spread;
          height: root.container-height + 2 * self.spread;
          border-radius: root.container-radius + self.spread;
          background: transparent;
          border-width: self.step;
          border-color: Theme.shadow-inner;
          opacity: CandidateMetrics.shadow-inner-opacity * self.falloff * self.falloff
              + CandidateMetrics.shadow-inner-opacity * 0.45 * (1 - self.falloff);
      }
  }
  ```

  > **注**：内阴影的 opacity 表达式刻意写得比外阴影复杂，因为只有 2 条带，二次衰减会把外圈压到几乎不可见。该表达式在 `band=1`（内圈）给出 `0.75`，在 `band=0`（外圈）给出 `0.75×0.25 + 0.34 = 0.53`。实现时应简化为一张显式的 2 元素查找表，可读性优先：

  ```slint
  // The two inner bands' opacities, innermost first. Written out rather than derived: with
  // only two bands a formula is less readable than the two numbers it produces.
  private property <[float]> inner-band-opacity: [0.75, 0.30];
  // ...
  opacity: CandidateMetrics.shadow-inner-opacity * root.inner-band-opacity[band];
  ```

  `crates/ime-ui/src/layout/metrics.rs` 需同步：新增 `shadow_inner_band_count` 字段、`take_count(found, "shadow-inner-band-count")` 一行、
  并更新 `shadow_band_opacity` 与 `shadow_inner_opacity` 的文档注释（语义从「每条」变为「峰值/最内条」）。
  `metrics.rs:486-493` 的测试常量列表需加入 `shadow-inner-band-count`。

- **逐步落地实施步骤**：
  1. **常量改造**：`CandidateMetrics` 的 material 段按上面重写；`layout/metrics.rs` 同步新增字段与解析。
  2. **阴影组件重写**：按上面替换 `CandidateShadow` 的两个 `for` 块。
  3. **目视校验**：在暗色 + 浅色桌面各截一张图，放大到 400%，确认 ① 外阴影是连续梯度、无 3.5dp 台阶 ② 外缘无硬切边 ③ 内阴影读起来是「贴边暗化」而非「粗描边」。
  4. **性能断言**：新增 `criterion` 基准，全量渲染 `568×302 @2x`，断言 P99 ≤ `raster_p99`。超限则把 `shadow-band-count` 降到 6 并记录实测值。
  5. **回归**：`layout.rs` 的 `window_size` / `container_rect` 不受影响（阴影预留仍是 `shadow-margin: 32dp`），但必须跑一遍 `layout` 与 `geometry` 的全部测试确认。
  6. **门禁**：`just check-ui` 通过（`shadow-blur`/`shadow-offset-y` 与 3.1.2 表一致）。

- **验收标准 (DoD)**：
  - [ ] **微观隐形细节审查**：400% 放大下外阴影为连续梯度，**无可见台阶**；外缘无硬切边；内阴影宽度 ≤ 2dp。
  - [ ] **反 AI 模板风审查**：不存在等 α 色带；不存在「粗黑描边」式内阴影。
  - [ ] **排布与工学**：窗口外框尺寸不变（`shadow-margin` 仍为 32dp），几何与命中区域零回归。
  - [ ] **物理微交互**：本卡不引入动效。
  - [ ] **性能**：全量渲染 P99 ≤ `raster_p99`（1.5ms）；若退回 6 带，实测值与理由写入验收记录。

- **验收记录**（2026-09-30）：
  - **交付物**：`crates/ime-ui/ui/candidate.slint`（516 行）、`ui/theme.slint`、`crates/ime-ui/src/theme.rs`、`theme/slint_palette.rs`、`theme/tests.rs`（791 行，本次新增 6 个用例）。
  - **验证命令与结果**：`just ci` 退出 0：`cargo fmt --all -- --check`、`cargo clippy`（主工作区 `--all-targets --all-features` 与两个 addon crate，全部 `-D warnings`）、`cargo nextest run`（主工作区 2430 个用例、两个 addon 346 个，全绿）、`cargo test --doc`（主工作区与两个 addon）、10 个审计脚本及其自检、`just check-host`。
  - **外阴影：真衰减，8 环带无缝平铺**：`band-step = shadow-blur / shadow-band-count = 3.5dp`，环带 `i` 的 `spread = (count − i) × step`、`border-width = step`（Slint 描边内侧绘制），因此环带恰好平铺 `[0, 28dp]`——最内环贴面板、最外环外缘落在模糊半径上，**不重叠、不留缝**；α 由 `Theme.shadow-outer-bands[i]`（`0.18 × ((i+1)/8)²`，最外 1/255、最内 46/255）给出，二次衰减而非等 α 色带。
  - **内阴影：2×1dp 贴边暗化**：`inner-band-step = 2dp / 2 = 1dp`，两条 1dp 环带覆盖 `[0,2dp]`，offset-y 1dp——由原来的「4dp/α=1.0 粗黑环」变为贴边暗化。
  - **渲染红线遵守**：**没有重新引入任何 `opacity` 绑定**（阴影透明度一律烘进逐带颜色），也未使用 `drop-shadow-*`。有测试在剥离注释后断言块内无 opacity、全文件无 drop-shadow。
  - **排布与命中零回归**：`shadow-margin: 32px` 未动、`window-width/height` 公式未动、`geometry.rs`/`layout.rs` 未触碰；新增 `test_view_shadow_material_matches_the_material_table` 钉住 32dp。
  - **环带数的越界保护**：环带数 0 会让 `band-step` 除零——由「count 必须为 8」与「内阴影 count ≥ 2」两条断言钉住；数组越界由「声明计数必须等于表长（两种配色都验）」钉住。
  - **已知限制**：① **DoD 1 的 400% 目视校验未做**（需真实会话截图）；② **DoD 5 的性能数字未取**——新增的 `shadow-band-count` 从 4 变 8 使绘制调用 +4 而覆盖像素数不变（都是 28dp 环带包络），内阴影由 4dp 收窄到 2dp 使填充面积**减少**约一半，按卡片自己的估算 @2x 约 97k px ≈ 满帧的 14%（外）+ ≈1%（内）；criterion 基准未建，若实测超限的处置是「把 band 数降到 6 并记录」；③ 外阴影 `offset-y 8dp + blur 28dp = 36dp` 比 32dp 阴影预留多 4dp，面板下方最外环带（α=1/255）会被窗口边界裁掉——这是 3.1.2 几何与 3.1.1 预留的固有差额，本卡前即存在，本卡按 DoD 要求未改预留。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85）、Linux 6.18.40.1-microsoft-standard-WSL2、Slint 1.13 软件渲染器、cargo-nextest 0.9.143。

---

#### 任务 ID：`UI-OPT-P0.08.01` 动效属性落点：把 `AnimationSet` 接到 `.slint` 上

- **基本属性**：
  - 绑定缺陷编号：`UI-DEF-23`
  - 所属组件域：域08 键盘交互 / 域02 布局框架
  - 优先级与复杂度：`P0` | 中 | 预估工时: 2 人天
  - 前置依赖：`UI-OPT-P0.02.02`（需要组件实例化才能写属性）
  - 关键路径：**是**
  - 并行通道：Track C 控件与动态反馈
  - 代码落地锚点：`crates/ime-ui/ui/candidate.slint`（`CandidateWindow` 的属性区）、`crates/ime-ui/src/surface.rs`、`crates/ime-ui/src/spring/set.rs`
  - 当前状态：`[x] 已完成`

- **原实现弊端与微观质感缺失剖析**：
  - **现有代码具体病灶**：`spring/set.rs:29-40` 的 `FrameMotion` 已经算好了四个量——`highlight.rect`（四自由度、速度续接、损伤区域）、`opacity`、`scale`、`page_offset_dp`。而 `candidate.slint:211-232` 的 `CandidateWindow` 只声明了 `preedit-text`、`mode-label` 与六个几何属性。**没有任何属性可以承载这四个量**。`spring/highlight.rs` 那套「四弹簧独立、飞行中改向保留速度、损伤区取新旧并集」的精细设计，目前是**悬空的**。
  - **规范要求**：3.3.1 明确「快速连按方向键时，高亮框在飞行途中收到新 target，必须保留当前速度续接，不得重置为 0。这是『消除生硬跳跃感』的核心」。Squirrel/Rime 与 macOS 原生候选窗都有这个滑动。
  - **为什么必须由 Rust 驱动而不是 Slint 的 `animate`**：`spring.rs:9-14` 的纪律是「时间由调用方交给积分器」；Slint 的 `animate` 会启动 Slint 内部时钟，使「静止时 `poll` 无限等待」不成立，直接违反 `BUDGET-CPU-01`（`idle_poll_timer_count = 0`），并让截图回归不确定。
  - **顶级标杆对标解析**：高亮框的物理滑动是「消除生硬跳跃感」的核心手段。Raycast / Linear 的列表高亮全部是弹簧驱动的连续运动，不是状态切换。

- **设计规范与参数定义 (Design Tokens & Spatial Specs)**：
  - **新增属性（`CandidateWindow`）**，全部为普通 `in property`，**禁止 `animate`**：

    | 属性 | 类型 | 单位 | 驱动源 | 范围 |
    |---|---|---|---|---|
    | `highlight-x` | `length` | dp（容器坐标） | `HighlightAnim::rect().x` | `0 .. container-width` |
    | `highlight-y` | `length` | dp | `.y` | `0 .. container-height` |
    | `highlight-w` | `length` | dp | `.w` | `≥ 0` |
    | `highlight-h` | `length` | dp | `.h` | `≥ 0` |
    | `highlight-visible` | `bool` | — | `HighlightAnim::is_visible()` | — |
    | `window-opacity` | `float` | — | `AnimationSet::opacity()` | `0.0 .. 1.0` |
    | `window-scale` | `float` | — | `AnimationSet::scale()` | `0.96 .. 1.0` |
    | `page-offset-dp` | `length` | dp | `AnimationSet::page_offset_dp()` | `-12 .. +12` |

  - **`window-scale` 的锚点**：3.3.2 规定「锚点为**光标侧边缘**」。出现时窗口从 `0.96` 长到 `1.0`，锚点必须是靠光标的那条边（`Below` 时为**上边缘**，`Above` 时为**下边缘**），否则窗口会「从中心胀开」而不是「从光标处长出来」。
  - **`page-offset-dp` 的作用域**：只作用于**候选网格**，不作用于 Header（Header 的 preedit 在翻页时不应位移）。
  - **高亮框与单元格的几何一致性**：`highlight-*` 是**容器坐标**，与 `geometry.rs:296` 的 `hit_map` 同一坐标系，因此命中测试与绘制天然对齐。
  - **收敛即停**：`AnimationSet::is_animating() == false` 时 `render()` 必须返回 `Ok(None)`。

- **重构源码落地实现 (Code Delivery)**：

  ```slint
  export component CandidateWindow inherits Window {
      // ... frame data and geometry (unchanged) ...

      // -- motion (3.3) -------------------------------------------------------
      // Every animated property below is written by `src/surface.rs` once per frame from the
      // spring integrator in `src/spring/`. There is deliberately no `animate` block in this
      // file: Slint's own animation clock would keep a timer alive while the window is at
      // rest, which breaks the idle-CPU budget (`BUDGET-CPU-01`), and it would make a
      // rendered frame depend on how many frames came before it, which breaks the visual
      // regression screenshots.
      in property <length> highlight-x: 0px;
      in property <length> highlight-y: 0px;
      in property <length> highlight-w: 0px;
      in property <length> highlight-h: 0px;
      in property <bool> highlight-visible: false;

      in property <float> window-opacity: 1.0;
      // 3.3.2 anchors the appear animation on the cursor-side edge, so the scale is applied
      // about that edge rather than about the centre: a window that grew from its centre
      // would visibly slide away from the caret as it faded in.
      in property <float> window-scale: 1.0;

      // The page-content slide (3.3.2): the candidate grid enters displaced by 12dp and
      // springs home. The header does not move -- the preedit did not change page.
      in property <length> page-offset-dp: 0px;

      // The whole surface fades as one. `opacity` on the root is what the renderer blends
      // against the transparent reserve, so the shadow fades with the panel.
      opacity: root.window-opacity;
      // ... rest unchanged ...
  }
  ```

  高亮框与网格的挂载（放在容器 `Rectangle` 内、`VerticalLayout` **之后**，保证绘制在最上层）：

  ```slint
      // The highlight floats above the grid rather than being a cell's background: the four
      // springs move x, y, w and h independently, so a cross-row move flies and reshapes
      // instead of sliding, and mid-flight the box overlaps two cells -- which is what the
      // motion is meant to look like (`src/spring/highlight.rs`).
      //
      // Declared after the layout so it paints on top; a cell that painted over it would
      // clip the box the moment it left its own cell.
      Rectangle {
          x: CandidateMetrics.container-padding + root.highlight-x;
          y: root.header-height + CandidateMetrics.separator-height
              + CandidateMetrics.container-padding + root.highlight-y;
          width: root.highlight-w;
          height: root.highlight-h;
          border-radius: CandidateMetrics.cell-radius;
          background: Theme.state-selected-bg;
          border-width: 1px;
          border-color: Theme.state-selected-stroke;
          opacity: root.highlight-visible ? 1.0 : 0.0;
      }
  ```

  翻页位移的挂载：

  ```slint
      CandidateGrid {
          x: root.page-offset-dp;   // the grid slides, the header does not
          // ... properties unchanged ...
      }
  ```

  出现/消失的 scale 锚点（`src/surface.rs` 侧，不是 `.slint`）：

  ```rust
  /// The point the appear animation scales about, in dp relative to the window.
  ///
  /// 3.3.2 anchors the growth on the cursor-side edge: a window placed below the caret grows
  /// downward from its top edge, one placed above grows upward from its bottom edge. Scaling
  /// about the centre instead would make the panel slide away from the caret while it faded
  /// in, which reads as the window arriving from the wrong place.
  fn scale_origin(placement: Placement, window_height_dp: f32) -> (f32, f32) {
      match placement {
          Placement::Above => (0.5, 1.0),
          Placement::Below | Placement::Auto => (0.5, 0.0),
      }
  }
  ```

- **逐步落地实施步骤**：
  1. **属性落地**：按上表在 `CandidateWindow` 声明 8 个属性；确认 `grep -c "animate" candidate.slint` 为 0。
  2. **挂载高亮框与翻页位移**：按上面的代码放置；确认高亮框在 `VerticalLayout` 之后（z 序）。
  3. **接线（`src/surface.rs`）**：`render()` 内按固定顺序——① `AnimationSet::step(dt)` ② 写 8 个属性 ③ 若 `highlight.damage` 非空则调用 `set_input_region`/损伤提交 ④ `render_if_dirty()`。
  4. **时钟**：`dt` 由 `render(now: Instant)` 的相邻两次调用差值给出，**首次调用 `dt = 0`**；`dt` 传给 `Spring1D::step` 时由 `spring.rs:389-395` 自行钳制。
  5. **静止断言**：`AnimationSet::is_animating() == false` ⇒ `render` 返回 `Ok(None)`，`committed_frames()` 不增长。
  6. **极端场景**：`[ui.animation] enabled = false`（`MotionConfig::instant()`）时所有属性必须直接落在终值（`opacity = 1.0`、`scale = 1.0`、`page-offset = 0`、高亮 `snap`）。

- **验收标准 (DoD)**：
  - [ ] **微观隐形细节审查**：连续按 `Tab` 时高亮框**滑动**而非跳变；飞行途中反向按键，轨迹**弯折**而非重启（`spring/highlight.rs:326-344` 的既有测试在接线后仍通过）。
  - [ ] **反 AI 模板风审查**：`candidate.slint` 内 `grep -c "animate"` 结果为 **0**；不存在 Slint 内部时钟驱动的动效。
  - [ ] **排布与工学**：出现动效的缩放锚点在光标侧边缘（`Below` 锚上边缘、`Above` 锚下边缘）；翻页时 Header **不位移**。
  - [ ] **物理微交互**：`enabled = false` 时全部动效瞬时到位；静止时 `committed_frames()` 不增长，`idle_poll_timer_count = 0`。
  - [ ] **性能**：高亮滑动的单帧损伤区 ≤ 两格并集 × 1.2（`spring/highlight.rs:359-394` 的既有断言在接线后仍通过）。

- **验收记录**（2026-09-30）：
  - **交付物**：`crates/ime-ui/ui/candidate.slint`（523 行）、`crates/ime-ui/src/adapter.rs`（759 行）、`adapter/tests.rs`（797 → 1035 行，本次新增 5 个用例）。
  - **验证命令与结果**：`just ci` 退出 0：`cargo fmt --all -- --check`、`cargo clippy`（主工作区 `--all-targets --all-features` 与两个 addon crate，全部 `-D warnings`）、`cargo nextest run`（主工作区 2430 个用例、两个 addon 346 个，全绿）、`cargo test --doc`（主工作区与两个 addon）、10 个审计脚本及其自检、`just check-host`。
  - **本次修掉的一处真实接线缺陷**：`highlight-visible` 不在 `FrameMotion` 里，原先只靠 `drawn_motion != Some(motion)` 决定是否写属性，导致「高亮框已静止时页面变空」这一步产生的 `FrameMotion` 与上一帧完全相同 → 属性不写 → 组件继续画一个**没有候选的高亮框**。现同时比对组件自身的 `get_highlight_visible()`。新增 `test_adapter_highlight_hides_when_the_page_empties` 是该缺陷的回归测试（**修复前必失败**）。
  - **DoD「反 AI 模板风」的 grep 现在有意义**：`grep -c "animate" crates/ime-ui/ui/candidate.slint` 从 2 → **0**。改动前那两处都在解释「为什么这里不写 Slint 属性过渡」的注释里，使该 grep 无法区分「注释提到关键字」与「代码里声明了关键字」；理由（Slint 内部时钟会让静止窗口挂着 timer，违反 `BUDGET-CPU-01`；会让一帧依赖此前画了多少帧，破坏截图回归）全部保留。
  - **动效关闭时一步到位**：新增 `test_adapter_motion_disabled_mid_flight_writes_the_end_values_at_once` 覆盖**飞行途中**关闭（先制造翻页位移 + 高亮飞行，再 `set_motion_enabled(false)`，下一次 `advance` 后断言全部到位且 `animating=false`）。
  - **翻页像素回归**：新增 `test_adapter_page_turn_slides_the_grid_and_leaves_the_header_still` 把 Header 行带与网格行带按原始字节比对，断言 Header 带**逐字节不变**、网格带改变。
  - **已知限制**：① **卡片示例里的 `opacity: root.window-opacity;` 在本项目不可实现**——本渲染器上绑定的 opacity 会让整棵子树**完全不绘制**（不是 no-op）。替代方案已落地：出现动效画成**几何生长**（面板 0.96 → 1.0，锚在光标侧边缘），理由已写成英文注释。② **消失动效在当前接线下完全没有可见效果**——`Adapter::set_visible(false)` 立即 `window.hide()`，`CandidateSurface::close()` 也不跑动效，所以 `AppearAnim` 算出的 1.0→0 / 1.0→0.98 永远不会被光栅化；要让它可见必须让 `surface.rs` 推迟 unmap 到动效收敛。③ **`[ui.animation]` 配置通路是死的**——`ime-config` 已完整解析并校验 `AnimationConfig`，但全工作区无任何消费者：`UiCommand` 与 `SurfaceUpdate` 都没有动效载荷，`Adapter::new()` 硬编码 `MotionConfig::default()`，`Adapter::set_motion_enabled` 目前只有测试在调。因此卡片第 6 步目前只在适配器层被证明，配置改 `enabled=false` 不会改变行为。④ `FrameMotion::highlight.damage` 在生产代码里**没有消费者**（`renderer.rs` 自己做拷贝损伤跟踪，且交互区是整个面板矩形、高亮框不会越出）；⑤ 出现动效期间交互区仍是完整面板矩形，缩小的面板外沿约 4dp 仍可命中；⑥ `crates/ime-ui/src/surface.rs` 的模块注释「Nothing animates yet」已过期。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85）、Linux 6.18.40.1-microsoft-standard-WSL2、Slint 1.13 软件渲染器、cargo-nextest 0.9.143。

---

## 7. 分片续写指令

本文件（`docs/dev/opt-ui.md`）只承载 **P0 底座**。其余两波次分别落在：

| 分片 | 路径 | 内容 | 卡片 | 状态 |
|---|---|---|---|---|
| Phase 2 | `docs/dev/opt-ui/phase-2.md` | P1 核心骨架与操作流线重塑 | `P1.05.01`、`P1.02.01`、`P1.02.02`、`P1.05.02` | **已产出** |
| Phase 3 | `docs/dev/opt-ui/phase-3.md` | P2 控件五态、状态反馈与极度打磨 | `P2.03.01`、`P2.05.01`、`P2.07.01`、`P2.01.01` | **已产出** |

**续写指令（如需追加第四波次，可直接作为下一轮 prompt）**：

```
继续执行 dev-opt-ui 的扩展波次。读取 ./docs/dev/opt-ui.md 的 §1（设计假设）、§2（参数底座）、
§3（缺陷总清单）、§4（追溯矩阵），以及 ./docs/dev/opt-ui/phase-2.md 与 phase-3.md 的 §0，
严格沿用主文档 §6 的任务卡字段规范，新增 ./docs/dev/opt-ui/phase-4.md。
本波次只处理 Phase 2 之后的**新增**缺陷，不得与既有 15 张卡重复。
约束：① 所有代码必须是 Slint 语法，不得出现 CSS/DOM；② 不得出现 `animate` 块；
③ 新缺陷必须先登记进主文档 §3 与 §4，再分配卡片，保持双向一致；
④ 正文必须逐项展开，禁止概括性省略措辞；⑤ 文件超过 800 行时继续分片。
```

---

## 8. 交付状态摘要

| 状态 | 内容 |
|---|---|
| **已完成** | 全景代码审计（8 域 × 28 缺陷，逐条 file:line）；设计假设清单；12 维隐形质感重映射；参数底座（三层景深 / 次像素内光 / 复合投影 / 弹簧常量 / 4dp 网格）；覆盖追溯矩阵（双向一致）；关键路径与并行通道；**P0 七张任务卡原子展开**；**Phase 2（P1 × 4）与 Phase 3（P2 × 4）分片已产出**；**四项裁决已落地到规范与代码**（见 §0.1） |
| **进行中** | 无 |
| **待办** | 无（15 张卡全部原子展开；`UI-DEF-01` ~ `UI-DEF-27` 全部归位；四项裁决全部关闭） |
| **已随裁决落地的代码改动** | `crates/ime-ui/ui/candidate.slint` 的 `cell-radius` `8px → 4px`（含推导注释）；`crates/ime-ui/src/layout/metrics.rs` 的 `cell_radius` 字段文档与断言 `8.0 → 4.0` |
| **已随裁决落地的规范改动** | `features.md` 升至 v1.4：新增 v1.4 变更摘要；3.1.1 候选单元圆角改 `4dp` 并写明同心推导；3.1.4 新增**显式例外清单表**（`1/6/10/34dp` 四类，逐项附理由）与历史原因注记；3.2 新增 `text.annotation` 的 α 不叠加注记段 |
