# 勘察摘要 · WindInput（清风输入法）— 簇 [SCOUT-E]

> 对象: `remarks/WindInput`（GPL-3.0，Rust workspace 28 crate + 三平台原生壳；本摘要覆盖
> wind-ui / wind-theme / wind-ui-types / wind-cmdbar / wind-quick-input / wind-softkeyboard /
> wind-ipc / wind-rpc / wind-host / data/themes / docs/design·redesign / wind_linux）。
> 勘察方式: 纯静态源码精读，未编译运行。摘录仅作锚点指认（GPL-3.0），吸收只允许机制级语义转写。

## 0. 形态速写（与己方同构性判断）

- `wind_input/`：Rust 服务进程（edition 2024，**无 tokio**，纯同步线程模型），承载引擎/协调器/主题/**全部浮层的光栅化**。
- 平台壳三份，均"薄壳"：`wind_tsf/`（C++17 TSF DLL）、`wind_macos/`（IMKit .app）、`wind_linux/`（**C++17 Fcitx5 addon**，对位 rspinyin 的双 cdylib 形态）。
- 渲染拓扑：服务进程内 tiny_skia + 自研 View 盒模型产出 BGRA 像素 → Windows 本地 UpdateLayeredWindow 贴窗；**受限宿主/Linux/macOS 走 SHM wire mirror**（64B header + 像素 + 命中矩形表），壳只负责落位与鼠标回传。与 rspinyin「UI addon + wire 镜像帧」同一问题域，契约形状高度可借鉴。

## 1. 展示层体系

### 1.1 主题 / Design Token 模型（v3 TOML + base 继承 + 求值坍缩）

- 文件格式: `data/themes/<name>/theme.toml`；`_base/`（217 行）为抽象基座（`_` 前缀不进主题列表），派生主题 amber/jade/violet 各仅 16 行（`base = "_base"` + 覆盖色）。加载链: 扁平人写 TOML → `normalize` → base 单链深合并（防环，`theme_chain_dirs` 最多 9 层，`wind-theme/src/theme.rs:64-90`）→ 类型化 `Theme`（`schema.rs`）→ 求值 `resolve.rs` → `Resolved`。
- `Resolved` = 渲染层唯一来源（`wind-theme/src/resolve.rs:50-65`）：`is_dark` + `palette: HashMap<String,Rgba>` + `views: RvViews`（盒模型树）+ `behavior` + `resources`（图路径/data:URI，按 is_dark 选变体）。**LightDark 在求值期坍缩为单值，RVNode 只存终值**。
- 几何 `Dim` 三态 dp/px/%（`schema.rs:25-58`，`Dim::resolve(scale, host)`）；px 表达"发丝边框不随 DPI"。
- token 引用 `${name}` 递归展开 + 环检测（`palette.rs::resolve_palette`，visiting 集合）；hex 解析防 `#+F+F+F`/非 ASCII panic（`palette.rs:22-46`）。
- **标准色契约**: `contract.rs::fill_missing` 给没写 `accent_text`/`success` 等标准名的主题补契约值，保证模板内联色 `$[…]` 在任何第三方主题可解析（消费点 `theme_query.rs:219`）。
- 亮暗档语言栏字色例外：`resolve()` 对 colors 按 false/true **各解析一次**（任务栏明暗 ≠ 候选窗明暗，`resolve.rs:66-82`）。

```toml
# data/themes/_base/theme.toml（节选）
[colors]
primary = "#4285F4"
bg = { light = "#FFFFFF", dark = "#2D2D2D" }
accent = "${primary}"
on_accent = "#FFFFFF"
accent_text = "${accent}"          # 契约名：_base 恒等于 accent，只为让名字存在
[window]
padding = [6, 8]; radius = 8
border = { width = "1px", color = "${border}" }
[item.selected]
background = "${selection}"
[index]; color = "${accent}"; shape = "none"; font_size = -2   # 相对主字体偏移
```

- **热切换链**: 菜单选主题 `select_theme`（`handle_mode.rs:1296`）→ `push_theme`（`handle_mode.rs:1435-1462`）→ `load_resolved_dirs` → `ui_tx.send(UiCommand::SetTheme(Box<Resolved>))`（:1461）→ UI 线程 `set_theme`（`candidate_window.rs:1292`）→ `refresh_effective_theme`（:1301，**唯一**的主题⊕用户字体⊕方案字体合并点，四个低频入口共用，渲染热路径只读结果）。
- 降级: `load_theme_with_fallback`（`handle_mode.rs:1395`）两级回落 `default`，**不写盘不改 theme_name**——"把一次读盘失败固化成配置变更，比显示不对更难查"。
- 教训锚点: `Resolved` 单个 13,440 B，debug 构建按值逐层返回在 2 MiB 栈线程实测 `STATUS_STACK_OVERFLOW`（`theme_query.rs:76-82`）→ 移动端拉取面只抽字段不传整树。

### 1.2 候选窗布局与绘制管线（每平台一份文本后端、一份光栅）

- 盒模型: `wind-ui/src/view.rs`（4,015 行）——measure 自底向上 / arrange 自顶向下 / paint 递归；row/column、padding/margin、圆角/边框(dashed/dotted 数据通路就绪光栅未补)、九宫格图、渐变(linear/radial)、阴影模糊、交叉轴对齐；arrange 后带 tag 节点收集绝对矩形供鼠标命中（`collect_hits`），像素与 hit-rect 同帧下发。
- **阴影蒙版缓存**（`view.rs:30-75`）：缓存 alpha 蒙版而非着色像素（换色不失效、内存 1/4）；键 = (宽,高,圆角,模糊,亚像素相位x/y) 各量化 1/4 px 吸收浮点噪音；32 条上限 + 闲置回收。
- 帧渲染 `render_frame`（`candidate_window.rs:1788-1960`）：DPI 探测（**固定位置模式按固定点取缩放**，跨屏 DPI 不再错）→ build_tree → layout → 宽度三级钳制（自然宽 → 主题 vertical_max_width → 屏幕安全上限，钳制真生效时记 debug 日志视作预算漏算异常）→ **定位三分支 drag_pin > fixed_pos > place_window(跟随光标+4px 抗抖)** → 上翻判定后必要时重建树重排 → paint 进 BGRA buf。
- 文本后端三份，对外契约逐方法对齐（颜色 `[R,G,B,A]`、预乘 BGRA 原地 source-over）：`text/dwrite.rs`（DirectWrite）、`text/coretext.rs`、`text/linux/`（fontconfig dlopen + ttf-parser + ab_glyph_rasterizer，**不链 HarfBuzz/FreeType**，无 hinting 灰度 AA + 4 档亚像素定位；彩色 emoji 支持 CBDT/COLRv0/v1/SVGinOT 四格式，`text/linux/mod.rs:1-30`）。
- 字体治理: 主题声明的字族仅低频入口查存在性、缺失 warn 去重（`candidate_window.rs:1330-1350`）；用户字重与主题强调字重取 max 的合成规则有注释级规范。

### 1.3 亮暗随系统切换

- `ThemeStyle { System, Light, Dark }`（`wind-coordinator/src/theme_style.rs:17`）；`resolve_dark` 是**唯一明暗出口**（:70-89，注释写明"裸 u8 + match _ 让跟随系统静默退化成亮色"的历史）。系统探测: Windows 读注册表 `AppsUseLightTheme`、macOS `CFPreferencesCopyAppValue`（**先 CFGetTypeID 验型再当 CFString**，防脏偏好 abort 全进程，:166-172）、其余平台恒 false；**缺失/失败一律按浅色**（"深色是更重的假设"）。
- 系统切换事件由 UI 线程截获 `WM_SETTINGCHANGE` 回送 `UiEvent::SystemThemeChanged`（`event.rs:150`），协调器仅 `style == System` 时重解析重推（`handle_mode.rs:1336-1347`）；`System` 每次渲染实时探测不缓存（探测是一次注册表读，量级低于重绘）。
- Android 形态由宿主注入 `resolve_dark_with(system_dark)`（`theme_style.rs:74-89`）——移动端没有桌面探测源，恒 false 会让"跟随系统"永不生效。

### 1.4 动画 / 过渡

- [经源码检索确认：候选窗/状态气泡/Tooltip/Toast 均无动画过渡机制，检索词：`grep -rn "animat|fade|ease|过渡" wind-ui/src/*.rs` 仅命中工具栏 auto_hide]。全 UI 唯一动画 = 工具栏自动隐藏 **1s 线性 alpha 淡出**（`auto_hide.rs:17-27`，`FADE_DURATION=1000ms`，`FADE_FRAME=16ms`——只在淡出这 1 秒要 60fps 唤醒，`next_deadline` 驱动）+ **GHOST_ALPHA=1 假隐藏**（淡出完不真隐藏，留 ≈0.4% 不透明度保留命中区域，光标移入即唤回；alpha 取 0 会穿透，:36-40）。

### 1.5 五态完备度（空/错/禁用/宿主压制/降级）

- 空: 候选空+缓冲空+无模式标记 → `HideCandidates`（`coordinator.rs:6841-6846`）；**mode_label（拼/双/快/英/符）在候选为空时随候选窗持久显示、有候选即隐藏**（:6832-6838）——"无候选"不是空白而是模式回显。
- 用户禁用: `hide_candidate_window`（candwin 隐藏）时状态照常演进、盲打照常上屏，仅不弹窗（:6848-6856）。
- 宿主压制: TSF UI-less 或 D3D 独占全屏 → `ui_suppressed_by_host` 同上（:6857-6865）；独占全屏另有显示前最后一道闸 + 300ms TTL 缓存（`manager.rs:23-33`，游戏被盖一下会被踢出独占态卡死）。
- 错误/提示四件套: `ShowToast`（错误/方案切换，accent 色可被调用方压过主题）、`ShowStatusTip`（中英/全半角 ~1s 自动隐藏，0=常驻）、悬停 `TooltipDoc`（结构化段/行/指纹）、`InputDiagHud` + `CaretOverlay`（诊断/定位调试浮窗）。
- 主题坏: 两级降级保底；字体缺: warn 不 fail；`FALLBACK_THEME` 也被 hide 时照旧加载（"有个能看的界面好过整个主题系统空转"）。

### 1.6 命令面板 / 快捷输入 / 软键盘 / 联想的交互形态

- **cmdbar**（`wind-cmdbar`，纯逻辑库零 wind-* 依赖）: 短语 → 手写 lexer/parser → AST → `EvalContext` 求值 → `display 文本 + 动作链`。8 个副作用 trait（Clipboard/KeyInject/Url/Proc/Dict/Ime/Config/Search）聚合成 `Services`，字段均可 None（缺服务返回 `ServiceUnavailable` 不中断）；`assert_pure_display` 守卫 display 表达式不得含副作用函数；`run_actions` **先执行全部 Effect 再拼 Text 上屏**；动作经独立线程执行（coordinator 释放 state 锁后回调自身安全，`handle_cmdbar.rs:11-13`）。宿主集成入口仅 3 个函数。
- **quick-input**（`wind-quick-input`）: 日期/算式/金额三源都是 `mix_modes.members` 列表的成员 id，**开关即增删、优先级即排序**，无双真相源 bool；格式外部化到 `data/system.quick.toml`（"解析归代码、渲染归配置"）；表达式模板经 `ExprEval` 回调交宿主求值（本 crate 不依赖 wind-cmdbar，靠协调器同时持有两者解耦）；内置表 `OnceLock` 进程级单例。
- **softkeyboard**（`wind-softkeyboard`）: "不是方案，是配置文件"——三层模型 `rows 画布 → Page.slots 唯一运行时真相 → 稀疏补丁/用户覆盖`；**写侧只允许写覆盖层**（"本仓所有覆盖类事故都是同一根因：两个写入者对同一份整表各写各的"）。UI 侧 `soft_keyboard.rs` 键帽点击回送**键位名而非下标**（错位会当场报警而非静默），Ctrl 粘滞态一律合成系统按键。
- **assoc**（`wind-assoc`）: 四源（History/Bigram/Prefix/Punct）**跨源不比分数、按固定优先级取配额**（量纲不同，归一化混排是条被否决的老路）；`boundary_broken` 断链不联想（跨焦点全局队列≠屏幕相邻）；签名按"完整上文"设计防二期返工。呈现侧**复用 candidates 通道**，无独立 UI（`coordinator.rs:6836-6838` 注释）。

## 2. 核心旅程

### 2.1 冷启动时序

- `startup_trace`（`wind-config/src/startup_trace.rs:1-30`）：**绕开 tracing 的同步逐条落盘**（每次 open→write→flush→close，带 pid），理由是主日志 non_blocking worker 死掉后 lossy 丢弃会让人误读"卡在某一行"；仅限启动/故障路径，禁入按键热路径；1 MB 上限防崩溃重启循环。stage 序列: begin→singleton→logger-ready→config-ready→bridge-ready→coordinator-begin/done→service-ready→ui-thread-begin→ui-candidate-window-ok。
- UI 线程创建窗口失败**不拖垮服务**：输入照常，GUI 全消失，靠 error + 启动轨迹留痕（`manager.rs:96-120`）。

### 2.2 按键→首候选→上屏延迟点

- 链路: C++ TSF `OnTestKeyDown` **提前吃键判定**（早于 IPC 往返；硬约定"吃键集 ⊆ Rust 出字集"，违者丢键）→ Named Pipe → `handle_key_event`（`message_handler.rs:979`，优先级链: key_up 切换键→菜单→热键→候选操作→加词→英文透传→夺取回退→`state.active` 单点 match→空缓冲激活→URL 夺取→以词定字→`apply_nav_key` 统一导航→小键盘归一→兜底臂）→ `notify_ui_update`（`coordinator.rs:6826`）→ `UiSender::send`（投递+唤醒绑死）→ UI 线程渲染。
- **延迟治理实测数据**: 无逃生口时按键→候选窗恒 85-95 ms（C++ `OnLayoutChange` 50ms debounce 占大头）（`coordinator.rs:6912` 附近注释）；四个首显逃生口：compat 标记"光标稳定宿主"直显 / 坐标已就绪 / 空闲上报锚点（仅 probe 不可信宿主）/ 25ms 短兜底。
- **首显闸门**（`first_show.rs`）: 新组合首帧若非经授权（reflow 后权威坐标或兜底 timer）不显示，防"陈旧坐标先显、reflow 后跳位"。死结分析值得精读（`first_show.rs:70-102`）: 微信宿主权威坐标 190ms 才到而长按组合寿命仅 128ms，"等对坐标"与"组合内显示"数学互斥，取 20px 一次性抖动。
- 双锁治理位置漂移: `composition_start` + `locked_rect_anchor` 两把独立锁；联想接龙场景候选窗不隐藏时用 `unpin_composition_anchor_if_shown` 解锁重锚。

### 2.3 preedit 策略

- `ui.candidate.preedit_display` 三态（`wind-config/src/config.rs:6406-6416`）: `app_inline`（默认，编码嵌宿主组合区）/ `candidate_top`（候选窗独立编码栏）/ `candidate_inline`（并入候选行首）。
- **数据与显示解耦**: `preedit` 恒下发，`preedit_host_owned` 只回答"谁来画"——曾是"不画就不发"，导致自绘宿主想拿数据只能改显示模式配置（`command.rs:44-51` 原文注释）。`preedit_caret` 为字节偏移恒在字符边界。

### 2.4 会话状态持久化

- 用户词/词频/Shadow/联想历史/补全学习全在 redb `user_data.db`（`wind-store/src/store.rs`，19 个模块分表；用户短语全局不分方案）；本机 UI 状态（工具栏/软键盘/气泡位置）在 `state.toml`（`config.rs:9947`）。
- 位置持久化三原则: 报**右下角**锚点（工具栏尺寸会变，锚右下朝屏内展开）；**抬起才发一次**（拖动全程不发，落盘频次天然收敛）；固定位置模式的候选窗拖动是"组合内临时挪开"不落盘（`event.rs:33-46`）。
- 配置四层深合并（默认<data<data_custom<user）**只增不删**的坑: 出厂预置落进逐键 map 字段后用户永远关不掉——三条出路判据见根 AGENTS.md"跨组件硬约定"。

## 3. 跨层边界签名（与 rspinyin「wire 镜像」同构问题的答案）

- **进程内契约** `wind-ui-types`（`lib.rs:1-11`）: "进程内契约，非线协议：值经 mpsc 通道流转，从不编码上线"；仅纯数据与纯映射逻辑，零 IO/零日志/零 serde；唯一平台载荷 `SetHostRender` 走 `#[cfg(windows)]` target-specific 依赖；新增依赖必须过 `aarch64-linux-android` cargo check（Android 复用同一契约）。

```rust
// wind-ui-types/src/command.rs:17-56（节选）
pub enum UiCommand {
    UpdateCandidates {
        preedit: String,
        preedit_caret: usize,        // 字节偏移，恒在字符边界
        preedit_host_owned: bool,    // 只回答"谁来画"，不影响下发
        mode_label: String,          // 拼/双/快/英/符；空=不显示
        candidates: Vec<CandidateItem>,
        selected: usize, hover: i32, // 键盘选中与鼠标悬停独立
        page: usize, total_pages: usize,
        caret_x: i32, caret_y: i32, caret_height: i32, caret_valid: bool,
        fixed: bool, fixed_x: i32, fixed_y: i32,   // 固定位置模式
    },
    HideCandidates,
    SetTheme(Box<wind_theme::Resolved>),   // 求值完的树整棵下发
    SetCandidateLayout { vertical: bool, rotated: bool, upright: bool }, // 蒙古文/对联式
    ...
}
```

- **反向事件** `UiEvent`（`event.rs:12-105`）: 鼠标交互全部通道化回协调器——UI 报**事实**（"点在哪一格"、拖动落点、"排列当前是否上翻"），裁决留协调器（"判据只有 UI 侧算得出的才由 UI 报"，如 `CandidateFlipped(bool)`）。含指纹校验协议: 右键气泡带 `doc_fingerprint`，协调器核对"UI 画的"与"缓存里的"是否同份，防段下标错位取错行（`candidate.rs:66-80`）。
- **线协议** `wind-ipc`: 二进制 CMD 常量表（`protocol.rs:23-60`）与 C++/Swift 字节级对齐（三方各有协议同步单测）；8B `IpcHeader`（version 高位 ASYNC_FLAG + 版本掩码只校主版本）；`FocusLostReason` 四值枚举把"失焦"拆成四种语义不同的后果（`Thread` 清激活、`CtxLost` **绝不清输入态**——噪声层 6ms 内掉了又回，`protocol.rs:62-105`）。JSON-RPC 走 4B 大端长度前缀帧（`rpc.rs:44-58`），ctrl 请求-响应 + events 单向广播双通道，本地授权靠 OS ACL。
- **SHM wire mirror**（`wind-bridge/src/shared_render_frame.rs:22-52`）: `64B SharedRenderHeader + BGRA 像素 + hit-rect 表`；**先写像素与矩形、最后写 header**——读端按 `header.sequence` 判新帧，消除"新头旧像素"撕裂窗口（单写者）。帧字段含 `rendered_hover_index`（渲染时悬停项）与 `target_instance_id`（多宿主路由）、`FLAG_SOFTWARE_SHADOW`。
- **Linux 分体**（`wind_linux/AGENTS.md` + `wind-ui/src/overlay_linux.rs:1-19`）: 服务进程光栅化 → SHM 段 → `CMD_OVERLAY_FRAME` 通知 → addon `X11Panel` override-redirect 窗口贴帧（`X11Panel.cpp:232`，不归 WM 管不抢焦点）。**落位分工**: 服务端拿不到屏幕几何，只给"首选点+备选点"或"锚点+留白"（`Place { Absolute, Flip, FollowCandidate, Anchor, Exact }` 五态，`protocol.rs` overlay 模块:270-310），addon 按工作区翻转/钳回；**计时分工**: 自动隐藏时长随帧下发、addon 计时（forwarder 线程无到期唤醒机制）。UDS 客户端 2s 超时+熔断 3s；连不上服务由 addon 拉起（节流 20s，双 fork 防 defunct）。
- **设置端 RPC**（`wind-rpc/AGENTS.md`）: `CoreRpc` trait 是唯一扩展点（is_chinese_mode/apply_config/data_rpc…），dispatch.rs 加 match 臂即新增方法；`system.capabilities` 从配置注册表动态生成；写方法成功自动广播 `config.changed`，只读方法必须显式进"只读名单"否则设置页刷新循环。

## 4. 可直接平移的 Rust 惯用法

1. `UiSender` 把「mpsc 投递 + 唤醒」绑成一次 `send`（50+ 发送点靠类型守门不靠纪律，`ui_sender.rs`）。
2. wake 原语: `MsgWaitForMultipleObjectsEx(消息队列 ∪ auto-reset Event ∪ 超时)` 三事件源合一睡到有事发生；建不出事件降级 8ms 轮询保功能（`wake.rs`）。
3. `thread_local!` + `RefCell` 双缓存（图像解码/阴影蒙版）+ 闲置回收 `IDLE_EVICT_AFTER` + 条数上限整体清。
4. `OnceLock` 内置表进程级单例（每次调用重建会变成每按键十几次 String 分配，`quick-input/lib.rs:44-49`）。
5. Push-on-drop 守卫（`SoftKeyboardPushOnDrop`）保证"状态变了必推给宿主"覆盖所有 return 路径（`message_handler.rs:982`）。
6. `RwLock<Arc<ConfigBundle>>` 原子快照热重载：读统一 `self.rt()`，整体替换 bundle，轻量项即时生效、重型项重启生效。
7. 表驱动 `func_specs!` 宏 + "named_params 与 eval_named 必须成对声明"的配对自检测试。
8. "枚举替裸 u8 + match 无通配臂"（ThemeStyle 教训）：唯一明暗出口，漏分支编译失败而非静默按亮色跑。
9. 注释级 ADR 纪律：几乎每个非显然决策都带"为什么不是另一条路"（如"白造一棵移动端不消费的视图树""深色是更重的假设"），文件头 `//!` 写责任与边界。

## 5. 候选吸收点清单

| 编号 | 源码锚点 | 一句话机制价值 | 移植难度 |
|---|---|---|---|
| SCOUT-E-01 | `wind-theme/src/resolve.rs:50-65` + `schema.rs:25-58` | 主题求值期坍缩：Ld{light,dark}→单值、Dim{dp,px,%} 符号态留到 paint 期解析，RVNode 只存终值，渲染热路径零分支 | 低 |
| SCOUT-E-02 | `data/themes/_base/theme.toml` + `contract.rs` | base 单链继承 + 标准色契约 fill_missing：派生主题 16 行即可成 Theme，第三方主题缺契约色自动补齐 | 低 |
| SCOUT-E-03 | `handle_mode.rs:1395-1430` | 主题两级降级不动配置不持久化（"固化读盘失败成配置变更比显示不对更难查"） | 低 |
| SCOUT-E-04 | `candidate_window.rs:1292-1355` | `refresh_effective_theme` 唯一合并点：主题⊕用户字体⊕方案字体⊕字号在四个低频入口算一次，热路径只读结果 | 低 |
| SCOUT-E-05 | `view.rs:30-75` | 阴影 alpha 蒙版缓存：1/4px 量化键吸收浮点噪音、换色不失效、内存 1/4、32 条上限+闲置回收 | 中 |
| SCOUT-E-06 | `wind-ui-types/src/lib.rs:1-11` + `command.rs`/`event.rs` | 表现层纯数据进程内契约：零 IO/零 serde/平台载荷 target-specific，UI 报事实协调器裁决 + fingerprint 防错位 | 低 |
| SCOUT-E-07 | `wind-bridge/src/shared_render_frame.rs:22-52` | SHM 帧防撕裂协议：先写像素后写 header，读端按 sequence 判新帧；hit-rect 与像素同帧 | 中 |
| SCOUT-E-08 | `wind-ui/src/overlay_linux.rs:1-19` + `protocol.rs:270-310` | 无屏几何侧的落位分工：服务端发 Place{Absolute/Flip/FollowCandidate/Anchor/Exact}，addon 按工作区钳回并计时 | 中 |
| SCOUT-E-09 | `wind-ui/src/text/linux/mod.rs:1-30` | 纯 Rust 文本栈（fontconfig dlopen + ttf-parser + ab_glyph + rustybuzz 仅 emoji 簇）：不链 HarfBuzz/FreeType，四格式彩色 emoji | 高 |
| SCOUT-E-10 | `coordinator/src/theme_style.rs:17-99` | ThemeStyle 枚举唯一明暗出口 + 宿主注入 system_dark（headless 无探测源不静默退化）+ 缺失按浅色兜底 | 低 |
| SCOUT-E-11 | `coordinator.rs:6826-6935` + `first_show.rs` | 首显闸门：授权坐标/兜底 timer 双路释放 + 四逃生口，治"陈旧坐标先显后跳"；实测 85-95ms 基线与 190ms vs 128ms 死结分析 | 高 |
| SCOUT-E-12 | `command.rs:36-51` | preedit 数据与显示解耦：preedit 恒下发、host_owned 只回答"谁来画"，渲染策略不焊死在数据通道上 | 低 |
| SCOUT-E-13 | `event.rs:33-46` | 窗口位置持久化三原则：右下角锚点（尺寸变化朝屏内展开）、抬起才发一次、fixed 模式拖动不落盘 | 低 |
| SCOUT-E-14 | `wind-config/src/startup_trace.rs:1-30` | 绕开 tracing 的启动轨迹同步落盘（带 pid、1MB 上限）：主日志 worker 死后仍可判"卡在哪" | 低 |
| SCOUT-E-15 | `wind-assoc/src/lib.rs:1-44` | 联想四源固定优先级取配额不比分数（量纲不可归一）+ boundary_broken 断链守卫 | 中 |
| SCOUT-E-16 | `auto_hide.rs:1-40` | GHOST_ALPHA=1 假隐藏悬停唤回：0.4% 不透明度保留命中区，alpha=0 即真隐藏鼠标穿透 | 低 |
| SCOUT-E-17 | `wind-cmdbar/AGENTS.md` + `handle_cmdbar.rs:11-13` | 命令面板侧效应注入：Services trait 字段可 None、display 纯度守卫、Effect 先于 Text、独立线程执行 | 中 |

## 6. 与 rspinyin 的形态差异提示（吸收时需转写）

- WindInput 候选窗渲染在**服务进程**、Linux 壳是 C++ addon 贴 SHM 帧；rspinyin 是 **UI addon 内 Slint 软件光栅**、wire 镜像的是结构化 UiFrame 而非像素——E-07/E-08 的防撕裂/落位分工可语义转写，像素通道不可照搬。
- 其亮暗/主题/首显闸门逻辑都在协调器（含 wind-ui 可选 feature 的 headless 编译门 `check-headless`/`check-android`），与 rspinyin "UI addon 与 IM addon 分 cdylib、握手过 UiFrame" 的边界切法不同层，吸收时以 rspinyin 的 crate 依赖白名单为准。
- 该仓 GPL-3.0 且注释密度极高，任何代码级摘录都不得进入 rspinyin 产物；以上锚点仅供阅读定位。
