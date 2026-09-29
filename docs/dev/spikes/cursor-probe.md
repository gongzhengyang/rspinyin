# Spike 结论：光标坐标提取、多屏与缩放归一化

> 状态：**代码侧结论已完成**；三级来源在真实前端下的语义、5 应用 × 2 显示服务器的原始值与偏差为**占位**，
> 由主 Agent 在具备多屏 / wlroots 环境的机器上回填（§4）。
> 关联模块：`MOD-RT` ｜ 风险编号：**R-03（本 spike 即该风险落点）**
> 代码落点：`crates/ime-fcitx5/src/cursor.rs`、`crates/ime-fcitx5/src/screen.rs`
> 规格来源：`docs/dev/features.md` 2.5.3（策略）、`TASK-1.04.05` 任务卡（三级来源与归一化规则）

## 0. 环境

| 项 | 值 | 来源 |
|---|---|---|
| 主机 | WSL2 + WSLg | 本机 |
| X11 显示 | 单个虚拟显示 `:0`（WSLg 提供） | 实测见 §4.1 |
| 显示器数 | **1**；无第二屏、无热插拔 | 本机 |
| Wayland 合成器 | **无**（WSLg 不提供 `zwlr_layer_shell_v1` 档合成器） | 本机 |
| Fcitx5 | 5.1.7（`xcb` / `wayland` 前端 addon 均可选依赖） | `abi-spike.md` §4.3 |
| Rust | edition 2024 / MSRV 1.85 / toolchain 1.98.0 | workspace 配置 |

### 0.1 本机可验证与不可验证的边界（重要）

| 验收项 | 本机 | 说明 |
|---|---|---|
| DoD#1（5 应用偏差 ≤ 2/4 物理像素） | ❌ | 需要真实应用矩阵与人工量测 |
| DoD#2（双屏 + 混合 DPI） | ❌ | 单显示、单一 scale |
| DoD#3（退化矩形、非法 scale 不 panic） | ✅ | 纯逻辑，单元测试覆盖 |
| DoD#4（三级耗时预算） | ⚠️ 部分 | 单元测试只做**宽松冒烟**；P99 由探针实测 |
| DoD#5（拔屏后 200ms 内 refresh） | ❌ | 无第二屏可拔 |
| DoD#6（5 应用 × 2 显示服务器的记录） | ❌ | 见 §4 占位表 |

**结论**：本任务的全部**逻辑**已在纯内存下验证（注入式接缝，无需显示服务器），
但 R-03 的核心未知量——**每个前端到底给的是客户端坐标还是屏幕绝对坐标**——**只能**在真实环境测得。
这是本 spike 唯一未闭合的结论，也是 §3 探测方案存在的原因。

## 1. 三级来源：代码侧结论

### 1.1 三级来源（`CursorResolver::plan`）

| 级 | 来源 | 何时命中 | 代价 | 落地位置 |
|---|---|---|---|---|
| 1 | 前端给的矩形**已经是屏幕绝对坐标** | 判定启发式成立（见 1.2） | 无 | `cursor.rs::looks_absolute` |
| 2 | 客户端矩形 + 焦点窗口原点 | 第 1 级不成立且后端能给出窗口原点 | 一次平台往返（X11：`_NET_ACTIVE_WINDOW` + `xcb_translate_coordinates`） | `cursor.rs::WindowGeometrySource`（后端实现） |
| 3 | 兜底：光标所在屏水平居中、垂直 60% 处 | 恒可用 | 无 | `screen.rs::ScreenInfo::fallback_anchor` |

- 第 2 级是**注入式接缝**（`trait WindowGeometrySource`），X11 与 Wayland 后端各自实现；
  在它们落地前由 `UnknownWindowGeometry`（恒返回 `None`）占位，因此今天所有客户端坐标都走第 3 级。
- 第 3 级**不阻断输入**：它只决定窗口位置，`resolve` 仍然返回 `Ok(anchor)` 并记录
  `platform/cursor/unresolved`。
- Wayland 三档（T1/T2/T3）如何映射到窗口原点，见 `features.md` 2.5.2：
  T1 档 layer surface 位置由我们自己设定（原点即 `(0,0)`），T3 档用全屏父 surface 的原点 +
  客户端窗口在其中的偏移；T2 档（`xdg_popup`）由合成器定位，第 1/2 级的语义需实测确认。

### 1.2 第 1 级判定是启发式（R-03 的核心未知量）

规则：**光标矩形的中心点落在任一输出范围内 ⇒ 视为绝对坐标**；否则视为客户端坐标。

- 该规则**不可能精确**：客户端坐标通常很小（如 `(50, 50)`），当客户端窗口靠近桌面原点时，
  它同样会落在某块屏幕内，于是被误判为绝对坐标。此时窗口会偏到客户端窗口的左上角附近。
- 正因如此，规则只写在**一个函数**里（`cursor.rs::looks_absolute`，内部即
  `layout.screen_at(centre_of(caret))`）。§3 的探测结果决定它是否要改，以及怎么改。
- 可能的修订方向（**待 §4 数据决定，当前不实施**）：
  1. 若实测发现 `xcb` 前端给的是**绝对坐标**、`wayland` 前端给的是**客户端坐标**，
     则按前端类型（而非坐标范围）分流，启发式退化为一张能力表；
  2. 若两端都返回客户端坐标，则第 1 级应**整体删除**，直接进入第 2 级（我们的窗口几何查询），
     这样反而更精确——启发式只是省一次往返的优化；
  3. 若两端都返回绝对坐标，则第 1 级保留，第 2 级降为"第 1 级失败时"的补充。

### 1.3 归一化规则（与任务卡逐条对应）

| 规则 | 实现 | 单元测试 |
|---|---|---|
| 全部坐标统一为物理像素：`logical × scale`，`round()` 取整 | `cursor.rs::physical_caret`（`to_i32`/`to_u32` 为饱和转换） | `test_resolve_scales_client_coordinates_and_replaces_an_unusable_scale` |
| 命中测试用**矩形中心点**判断所在屏 | `screen.rs::centre_of` + `ScreenInfo::contains`（半开区间 `[left, right)`） | `test_resolve_hit_tests_the_caret_centre_and_keeps_negative_coordinates`、`test_screen_contains_uses_half_open_bounds` |
| 退化矩形（`w == 0 \|\| h == 0`）→ `(x, y, 1, 20 × scale)` | `physical_caret`（实现取 `w <= 0 \|\| h <= 0`，见坑 2） | `test_resolve_falls_back_for_unusable_coordinates_and_repairs_degenerate_sizes` |
| `x`/`y` 为 `INT_MIN` 或 `\|x\| > 100000` → 直接兜底 | `screen.rs::is_plausible`（判定在**缩放前**的原始值上做，与任务卡一致） | 同上 |
| `scale` 为 0 或不在 `[1.0, 3.0]` → 取 `1.0` + 记 `platform/scale/invalid` | `screen.rs::check_scale` | 同上 + `test_check_scale_accepts_the_range_and_replaces_the_rest` |
| 负坐标不做夹取，原样保留 | 全程无 clamp；`i64` 中间量 + 饱和转换只用于防溢出 | `test_resolve_hit_tests_the_caret_centre_and_keeps_negative_coordinates` |
| 候选框必须用**光标所在屏**的 scale | `Anchor.scale` 取自命中输出的 `ScreenInfo::scale` | `test_resolve_hit_tests_the_caret_centre_and_uses_that_outputs_scale` |

**对任务卡的两处补充**（均已在代码注释中说明）：

1. **输出自身的 scale 也走同一校验**。任务卡只要求校验 `FcitxCursorRect.scale`；
   但后端若把某块屏的 `scale` 报成 `0.0`，`Anchor.scale = 0.0` 会让 UI 除零。
   实现用 `check_scale` 一并校验并记录同一个诊断码。
2. **`w`/`h` 为负**同样按退化矩形处理（任务卡只写了 `== 0`）。负宽高若直接转 `u32` 会回绕成
   天文数字，属于必须挡住的输入。

### 1.4 抖动抑制与缓存

| 行为 | 规则 | 测试 |
|---|---|---|
| 抖动抑制 | 连续两次 `resolve` 的锚点**位置**在两个轴上偏差均 ≤ 1 物理像素、且屏与 scale 相同 ⇒ 复用上一次锚点（尺寸不参与比较） | `test_resolve_cache_suppresses_jitter_and_survives_an_unresolvable_caret` |
| 兜底稳定 | 光标无法定位时，**若缓存属于同一 `IcId` 则整份复用**（不受 1px 限制），避免候选框在屏上跳 | 同上 |
| 缓存键 | `IcId`：另一个输入上下文不会继承上一个客户端的锚点 | 同上 |
| 失效 | `on_focus_out(ic)` 只清除**该 ic** 的缓存；`refresh_layout`/`adopt_layout` 在缓存屏消失时清除 | `test_layout_adoption_keeps_both_paths_in_step_and_drops_a_dead_screen` |
| 诊断去重 | `platform/cursor/unresolved` 与 `platform/scale/invalid` **每次状态跃迁只记一次**（持续不可解析不会每帧刷屏） | 见坑 5 |

### 1.5 为什么没有 X11/Wayland 依赖

`screen.rs` 把**枚举**做成 `trait ScreenEnumerator`（`Send`），`cursor.rs` 把**窗口几何**做成
`trait WindowGeometrySource`（`Send`）。两者都由后端注入，纯 Rust 构建下分别由
`ScriptedScreens` / `UnknownWindowGeometry` 占位，因此：

- 本任务的全部逻辑在**没有任何显示服务器**的机器上可测（本机即如此）；
- `TASK-1.04.06` / `1.04.07` 实现这两个 trait 即可接入，`cursor.rs` 与 `screen.rs` 不需要改动；
- `Send`（非 `Sync`）与冻结契约 `SurfaceBackend` 的约定一致，也让 glue 可以把 resolver 放进
  进程级 `Mutex`。

**Wayland 的线程约束已在 API 上留了口子**：T1/T3 档的 `wl_display` 属于 UI 线程
（Wayland 连接只能在创建它的线程使用），host 线程无法主动枚举，因此 `CursorResolver::adopt_layout`
允许"在别的线程枚举好、把结果交过来"。X11 档走 `refresh_layout` 即可。

## 2. 数据流

```
fcitx5 前端 (xcb / wayland)
      │  InputContext::cursorRect() + scaleFactor
      ▼
on_cursor_rect(context, ic_id: u64, FcitxCursorRect)          ffi/abi.rs（主 Agent 挂接）
      │  IcId::new(ic_id)
      ▼
CursorResolver::resolve(ic, rect, &layout)                    cursor.rs
      ├─ 归一化：check_scale → physical_caret（物理像素 / 退化修复 / 哨兵值）
      ├─ 第 1 级 looks_absolute(layout, caret)          → FrontendAbsolute
      ├─ 第 2 级 translated(caret, window_origin(ic))   → WindowGeometry
      └─ 第 3 级 ScreenInfo::fallback_anchor()          → Fallback（记 platform/cursor/unresolved）
      │  抖动抑制 / 缓存复用
      ▼
Anchor { cursor: RectI, screen: ScreenId, scale: f32, placement: Placement::Auto }
      │
      ▼
UiCommand::Show { revision, anchor }  ──> UI 线程（TASK-1.05.07 的避让算法消费）
```

**布局来源**：`refresh_layout()`（RandR `SCREEN_CHANGE_NOTIFY` / `wl_output` 事件驱动）
或 `adopt_layout()`（UI 线程枚举后交过来）。`resolve` **绝不**触发枚举（有测试断言：
`test_resolve_never_enumerates_the_screens`）。

## 3. 探测方案（主 Agent 执行，本机无法完成）

### 3.1 需要回答的问题

| 编号 | 问题 | 为什么重要 |
|---|---|---|
| Q1 | `xcb` 前端的 `cursorRect` 是客户端坐标还是屏幕绝对坐标？ | 决定第 1 级启发式是否成立 |
| Q2 | `wayland` 前端（T1/T3 档）呢？ | 同上 |
| Q3 | `cursorRect.x/y` 是逻辑像素还是已经是物理像素？`scale` 是否**已经乘进** `x/y`？ | 决定 `physical_caret` 是否该乘 scale（当前实现按任务卡：乘） |
| Q4 | 终端与 Electron 类应用是否真的返回 `w=0`/`h=0` 的退化矩形？ | 验证 1.3 的退化修复是否必要且够用 |
| Q5 | 客户端 `scale` 与光标所在屏的 `scale` 在混合 DPI 下是否一致？ | 决定 `Anchor.scale` 取谁（当前取屏的） |
| Q6 | 拔掉显示器后，RandR / `wl_output` 事件到 `refresh_layout` 的延迟？ | DoD#5 的 200ms 预算 |

### 3.2 打点方式（建议）

在 C++ 胶水的 `on_cursor_rect` 或 Rust 侧 `resolve` 之后临时加一行诊断（**仅探测期开启**）：

```
platform/cursor/probe: raw=(x,y,w,h,scale) tier=T1|T2|T3 anchor=(x,y,w,h,screen,scale)
```

- 光标坐标**不属于**用户输入内容，不在 `ime-diag` 的脱敏黑名单内；但仍建议只在探测时打开，
  探测完成后删除该行（`AGENTS.md` 3.4：禁止把动态内容拼进消息串，此处用结构化字段）。
- 采集时同步记录 `xdotool getactivewindow getwindowgeometry` / `wtype` 的目标窗口几何，
  以便判断"客户端坐标 vs 绝对坐标"（若 `raw.x + window_x == anchor.x` 则为客户端坐标）。
- 偏差量测：用截图工具（`grim` / `import`）量候选框左边缘与光标左边缘的像素差，
  换算成物理像素后填入 §4.3。

### 3.3 采集矩阵（5 应用 × 2 显示服务器）

| 应用 | 类型 | X11 原始值 | X11 解析值 | X11 偏差 | Wayland 档位 | Wayland 原始值 | Wayland 解析值 | Wayland 偏差 |
|---|---|---|---|---|---|---|---|---|
| 终端（如 `foot`/`kitty`） | 终端 | 待填 | 待填 | 待填 | 待填 | 待填 | 待填 | 待填 |
| GTK（如 `gedit`） | GTK3/4 | 待填 | 待填 | 待填 | 待填 | 待填 | 待填 | 待填 |
| Qt（如 `kate`） | Qt | 待填 | 待填 | 待填 | 待填 | 待填 | 待填 | 待填 |
| Electron（如 VS Code） | Electron | 待填 | 待填 | 待填 | 待填 | 待填 | 待填 | 待填 |
| Firefox | Gecko | 待填 | 待填 | 待填 | 待填 | 待填 | 待填 | 待填 |

## 4. 实测证据（主 Agent 回填）

> 以下小节全部为**占位**。本机（WSL2 + WSLg）只有一个虚拟显示、无第二屏、无热插拔、
> 无 wlroots 档合成器，**这些数据在本机无法取得**；不得用推测值填充。

### 4.1 环境确认（DoD 前置）

```bash
echo "$DISPLAY $WAYLAND_DISPLAY"      # 预期：:0 与空
xrandr --query                        # 预期：单输出，无 scale 变化
fcitx5 --version                      # 预期：5.1.7
```

实测输出（2026-__, __:__）：

```text
（待主 Agent 回填）
```

### 4.2 每个应用的原始值与解析值（DoD#6）

见 §3.3 矩阵。每个格子需记录：`(x, y, w, h, scale)` 原始值、解析后的
`Anchor{cursor, screen, scale}`、以及用截图量出的水平/垂直偏差（物理像素）。

```text
（待主 Agent 回填）
```

### 4.3 偏差汇总（DoD#1：水平 ≤ 2px、垂直 ≤ 4px）

```text
（待主 Agent 回填：每应用一行，给出最大偏差与是否达标）
```

### 4.4 双屏 + 混合 DPI（DoD#2）

需要：主屏 `scale=1.0` + 副屏 `scale=2.0` 并排，且副屏位于主屏右侧/左侧各测一次
（后者覆盖负坐标排列）。断言：候选框出现在**光标所在屏**，且 `Anchor.scale` 等于该屏的 scale。

```text
（待主 Agent 回填）
```

### 4.5 热插拔（DoD#5）

```bash
# 拔掉副屏，观察日志中 refresh_layout 被触发的时刻与候选框位置
```

```text
（待主 Agent 回填：事件 → refresh_layout 的延迟 ms，以及缓存是否已丢弃消失的屏）
```

### 4.6 性能（DoD#4）

第 1/3 级 ≤ 200µs、第 2 级 ≤ 500µs（P99）。代码侧现状：第 1/3 级**零分配、无锁、无系统调用**；
第 2 级的时间完全取决于后端实现（一次 X11/Wayland 往返），因此它的预算由
`TASK-1.04.06`/`1.04.07` 的实测负责。单元测试 `test_resolve_stays_within_the_latency_budget`
是**宽松冒烟**（5k 次调用的平均值远低于预算），用于捕捉"有人在 `resolve` 里重新引入枚举"这类回归，
不替代探针的 P99 量测。

```text
（待主 Agent 回填：三级各自的 P50/P99，以及探针命令）
```

## 5. 坑与对策

1. **启发式无法自证**（§1.2）。今天的实现按任务卡"中心点落在任一屏内即视为绝对坐标"，
   客户端窗口靠近桌面原点时会误判。对策：规则收敛在一个函数，§3 的数据到手后一次性修订；
   `platform/cursor/unresolved` 的诊断率（第 3 级占比）可以作为"启发式是否普遍失效"的观测指标。
2. **负宽高会回绕**。`FcitxCursorRect.w/h` 是 `i32`，`RectI.w/h` 是 `u32`：负值直接转换会得到
   天文数字。实现先判 `<= 0` 再转，退化矩形统一修成 `1 × 20×scale`。
3. **`INT_MIN` 上取绝对值会 panic**（debug 下 `i32::MIN.abs()` 溢出）。
   实现用 `i64::from(x).abs() <= 100_000` 判定，绝不在 `i32` 上做取绝对值。
4. **溢出面**：`x + w/2` 与 `origin + size` 都可能越界。实现全程用 `i64` 中间量 + 显式 clamp，
   `translated` 用 `saturating_add`；浮点转整数依赖 Rust 的饱和语义（`as` 不会 panic）。
   这三条都有测试（`test_screen_contains_survives_a_size_that_overflows_i32`、
   `test_centre_of_survives_a_rectangle_wider_than_i32`）。
5. **诊断刷屏**：兜底路径可能每帧命中。实现只在**状态跃迁**时记录
   （`record` 里比较上一次的状态），持续不可解析不会持续写日志。
6. **`resolve` 内的枚举**是最容易引入的性能回归（每帧枚举输出 = 每帧一次系统调用）。
   实现把枚举隔离在 `refresh_layout`/`adopt_layout`，并用 `ScriptedScreens::handle()`
   的计数句柄写了断言测试。
7. **Wayland 的线程约束**：`wl_display` 只能由创建它的线程使用，而 `resolve` 在 host 线程。
   T1/T3 档因此不能在 host 线程枚举输出，必须由 UI 线程枚举后经 `adopt_layout` 交过来
   （见 §1.5）。若后端实现忽略这一点，会得到"未定义行为"而不是编译错误——`TASK-1.04.07`
   落地时必须检查。
8. **两个新诊断码需要登记**：`platform/cursor/unresolved`、`platform/scale/invalid` 不在
   `ime-types` 的冻结错误码表内（跨 FFI 的失败无法用 Rust 错误类型表达），按 `2.2.4` 的既有惯例
   应在该节新增登记表（与 `ffi/*`、`lifecycle/*` 两张表并列）。
9. **`IcId` 的定义位置**：任务卡的 `resolve(ic: IcId, …)` 未指定 `IcId` 的归属。实现把它定义在
   `cursor.rs`（`u64` newtype，含 `new`/`value`/`From<IcId> for u64`），与 `ime-types::ids` 的
   风格一致。若 `TASK-1.04.02` 的会话注册表也定义了同名 newtype，两处需要合并（唯一改动点）。
10. **文件行数**：`cursor.rs` 与 `screen.rs` 都在 800 行上限内，但 `cursor.rs` 已接近上限。
    为守住 `AGENTS.md` 第 5 节，屏幕空间算术（中心点、取整、锚点构造、抖动比较）被放在
    `screen.rs`——它们本就是"给定一块输出，锚点落在哪里"的问题。若后续要加回更细的测试，
    建议主 Agent 按 `AGENTS.md` 3.6 把测试模块移到 `crates/ime-fcitx5/tests/`。

## 6. 遗留决策（交给主 Agent）

1. **§3 的探测必须执行**：R-03 的闭合条件是 Q1–Q6 有答案，尤其是 Q1/Q2/Q3（启发式与
   scale 语义）。在本机无法取得，需要在具备双屏 + wlroots 的机器上跑。
2. **修订点唯一**：探测结果若要求改启发式，只改 `cursor.rs::looks_absolute`；
   若要求改 scale 语义，只改 `cursor.rs::physical_caret`。两处都有指向本文件的注释。
3. **RandR / `wl_output` 事件接线**属于 `TASK-1.04.06`/`1.04.07`：它们负责在事件到达时调用
   `refresh_layout`（X11）或 `adopt_layout`（Wayland）。本任务只提供入口与失效逻辑。
4. **`features.md` 2.2.4 登记**两个新诊断码（坑 8）。
5. **模块挂载**：`lib.rs` 需要 `pub mod cursor;` 与 `pub mod screen;`（由主 Agent 添加）。
6. **`ime-diag` 落地后**，`emit_diagnostic`（stderr）应换成 `tracing::warn!` 并带结构化字段；
   本任务用 stderr 是因为 `ime-fcitx5` 尚未依赖 `tracing`，且诊断层仍是占位。
