# opt-basic.md - 商业级体验重构与工艺级优化工程方案

> 文档版本: v1.0 ｜ 系统形态: Desktop GUI（Linux 桌面输入法：Fcitx5 进程内插件，双 cdylib —— 引擎 addon `librspinyin.so` + 自绘候选窗 addon `librspinyin_ui.so`）｜
> 架构基线: Rust 2024 workspace（9 crates + xtask，MSRV 1.85 / toolchain 1.98.0 / edition 2024）+ Slint 1.13.1 软件光栅（`SoftwareRenderer` + `software-renderer-systemfonts`）+ Fcitx5 5.1.7 C/C++ ABI ｜
> 关联 ADR: `./docs/dev/adr/`（0000–0006、0010 已建；本文件新增诉求登记为待建 ADR-0011「跨 addon 帧通道」与 ADR-0005 两项追加）｜
> 最后同步 Commit: `4def355`（fix: turn the gate suite green again）｜
> 逻辑自检: [已通过（第 2 轮收敛，Blocker 2 / Major 8 / Minor 7 ｜ 优化采纳 2 / 否决 2 / 登记 2）] ｜
> 维护约定: 代码演进后必须回写缺陷清单状态与追溯矩阵；任何假设被推翻时同步修正受影响任务卡的 NFR 与验收标准

**权威规范关系**：`docs/dev/features.md` 是功能与架构的唯一权威（含 0.4 规则、0.5.3 预算、2.2 冻结契约、3.1–3.6 视觉与交互规范）。本文件与 `features.md` 在**技术细节**上冲突时以 `features.md` 为准并回改本文件；本文件对 `features.md` **未登记的断链事实**做补充登记（第 1 节），其中要求改契约的两处（`UiFrame.highlight` 追加、`SurfaceBackend::connection_fd` 追加）按 AGENTS.md 第 8.22 条走 ADR-0005 增量追加路径。

**与专项 skill 文档的分工（防止重复施工）**：本文件是**五大断层一次性基线重塑**。视觉细节维度由 `opt-ui.md`（Hub 7 卡已完成 + 8 张分片卡）承接、键盘流由 `opt-keymap.md`（21 张分片卡）承接、性能由 `opt-perf.md`（11 张分片卡）承接、分发由 `opt-deploy.md`（13 张分片卡）承接、功能增量由 `features-add.md`（39 张分片卡）承接。本文件的缺陷总清单对**已被其他文档开卡承接**的缺陷只登记指针（追溯表「绑定任务」列标注外部卡号），自身任务卡只承接**无主缺陷**——经 2026-10-01 全量对账，五大断层下共 42 项缺陷（`DEF-01`~`DEF-42`），**全部由本文件的 31 张任务卡承接**，其中 3 项（`DEF-06`/`DEF-15`/`DEF-39` 的部分面）另有专项文档卡片并行承接外部维度。

---

## 0. 阶段零：现状诊断与商业化差距摘要

### 0.1 总判词

**这个项目的失败不是「做得不够好看」，而是「链条没有通电」。**

阶段零对工作区 20 万行 Rust / 3 份 `.slint` / 10 个门禁脚本做了全量扫描（4 路并行专项审计：全状态矩阵、渲染与微交互、暗部工程、信息架构与键盘流，每条结论都锚定 `文件:行号`），得出的结构性结论只有一条：

> **几乎所有的缺陷都不是「逻辑写错」，而是「有生产者、无消费者」或「有消费者、无生产者」的断链。** 引擎每一键都在解码、建帧、发 `UiCommand`——而生产端的 `post()` 把命令全部丢弃；候选窗每一帧都能被软件光栅画出来——而它从未收到过一个帧。接管策略抑制了宿主 ClassicUI，自绘窗口又无米下锅，用户在一个可见候选框都没有的状态下盲打。围绕这个断点，主题、覆盖层、事件回程、光标锚定、焦点生命周期、崩溃取证、配置面十八个键……沿着同一条裂缝成排塌陷。

与此同时，必须如实记录：**这套代码库的工艺上限很高**。SPSC 队列的溢出语义表、字节级三处对齐的配色门禁、高斯平方衰减的八段阴影、带隔离与备份回滚的用户词库恢复、按上下文键限界的并发纪律——这些在商业 IME 里都属于前 10% 的工程水准。差距不在「做得糙」，在「最后十厘米的接线」。

### 0.2 五大断层实测摘要（证据逐条在第 3 节缺陷总清单）

| 断层维度 | 实测结论 | 代表性证据 |
|---|---|---|
| **① 全状态矩阵** | 引擎→窗口的帧通道、窗口→引擎的事件回程、光标锚定、主题下发、覆盖层五链全断；`readonly`/`has_user_dict_hit`/`script` 三个契约字段双向死亡；Ctrl+Space 吞键无效；词典缺失降级 UX 未建。**用户当前实际体验：打拼音无任何候选显示，数字键盲选。** | `ffi/abi/engine/host.rs:129-134`（post 丢弃）、`session_host.rs:452-454`（ui_event 无生产调用方）、`engine/router/modes.rs:73-80`（状态条默认展开） |
| **② 材质视觉秩序** | 材质与 Token 体系本身是商业级的（八段高斯衰减阴影、次像素描边、对比度自检），但：项目对渲染器 `opacity` 能力的核心认知**与 Slint 1.13.1 源码事实相反**；appear 动效的淡入半边从未接到像素；亚克力半透明档硬编码不可达；一组 token 声明后无绘制方。 | `theme.slint:116-123`（错误论断）vs `i-slint-core-1.13.1/software_renderer.rs:2497-2499`（`apply_opacity` 生效）；`surface.rs:361-377`（blur 恒 Refused） |
| **③ 微交互手感** | 弹簧积分器、可中断过渡、damage 合并都是对的；但指针事件延迟**无上界**（连接 fd 不进 poll 集）、消失动效从不渲染却在隐藏后以 144Hz 隐形空转（违反空闲 CPU 预算）、预编辑宽度估算在混排文本下把「保尾」变成「保头」并产生多个省略号、120ms 交叉淡变实现后无人调用。 | `surface.rs:234-236`（fd 恒 None）、`adapter.rs:348-365`（hide 后才启动消失弹簧）、`adapter/preedit.rs:302-333` |
| **④ 暗部工程韧性** | FFI 卫生、有界通道、线程生命周期纪律全部达标；但崩溃取证层（panic hook / 信号 handler / crash 记录 / 探针快照）**整层未接线**，FFI panic 行绕过节流，短语写手退出时静默丢词，X11 档 HiDPI 结构性断裂（scale 钉死 1.0）。 | `panic.rs:61` 与 `crash/signal.rs:247` 零生产调用方；`ffi/mod.rs:124-132`；`ime-ui-addon/src/platform/probe.rs:36,239-247` |
| **⑤ 信息架构与键盘流** | 路由表数据化、绑定审计、吞键纪律是标杆级的；但速查表/命令面板/诊断面板五断点全死（触发、内容、通道、渲染、关闭各断一处），四个已实现动作无绑定行，配置面 18 个键声明未读，README 谎报热重载，模式指示只在组合态可见。 | `engine/context/panel.rs` 仅测试调用；`key.rs:51-64`；`addon/config.rs:173-183`（`lifecycle/pending`） |

### 0.3 门禁基线（本次审计同期修复）

阶段零发现门禁本身已红：clippy 6 处报错（测试代码 `expect` 越界检测、`field_reassign_with_default`、`cloned_ref_to_slice_refs`）、`cargo check` 1 处未用导入、`gen-licenses` 自检的注入用例因 `alloc-count` 排序变化而**失真**（注入不达 shipped 闭包，违规用例假通过）、`LICENSES/` 的 Slint 许可副本与 slint 1.13.1 包内原文不一致、`docs/dev/licenses.md` 漏登记 `alloc-count`。**以上 8 项已于本次落地修复并以 `fix: turn the gate suite green again`（`4def355`）提交，修复后 `just check` / `just check-host` / `just audits` 全部退出 0。** 本文件全部任务卡的 DoD 以此绿色基线为起点。

### 0.4 「AI 模板/粗糙感」特征清单与工业级替换标准

本形态没有 CSS/DOM，阶段零把「廉价感」重映射为**接线断链与认知失真**两类，替换标准如下：

| 廉价特征（现状） | 工业级替换标准（本文件任务） |
|---|---|
| 打字无候选、无回显，数字键盲选（宿主 UI 被接管后自绘窗不画） | 帧通道贯通，按键到候选像素走自绘管线（P0.01.01） |
| 高亮环永远钉在第一格，Space 提交的却可能是第三格 | 契约追加 `UiFrame.highlight`，环随帧走（P0.01.04） |
| 鼠标悬停/点击响应延迟从数十毫秒到「下一次打字前」 | 连接 fd 进 `poll(2)` 集合，指针事件 ≤ 一帧（P0.03.01） |
| 窗口出现是缩放跳变、消失是硬切 | 接上 `window-opacity` 淡入 + Hide 先淡出后 unmap（P0.02.01 / P1.02.02） |
| 模式切换吞键无反馈、状态点说谎 | 模式位语义闭环 + 空闲态可见反馈（P0.01.07） |
| 出错只进日志，用户面对静默降级 | 状态条通知位承载 `domain/action/reason` 的人类可读面（P0.01.06） |
| 崩溃即失忆 | 崩溃取证四件套接线 + 记录上限（P0.04.01） |

---

## 1. 系统设计假设清单 (Assumptions First)

> 编号沿用项目惯例前缀 `ASM-B-*`（Basic），避免与 `features.md` 第 1 节的 `ASM-NN`、`features-add.md` 的 `ASM-A-NN` 冲突。`$4` 性能红线与 `$5` 架构约束按默认值引用 `features.md` 0.5.3 与 0.4，原样登记如下；未指定项均为阶段零实测值。

| 假设编号 | 维度（形态/交互延迟/帧预算/数据量级/硬件边界） | 假设内容（基于阶段零实测） | 影响的断层维度 | 偏离时的修正策略 |
|---|---|---|---|---|
| `ASM-B-01` | 系统形态 | Desktop GUI：Linux 桌面输入法，Fcitx5 进程内插件；**双 cdylib**（ADR-0003/0004），两库 `dlopen` 独立加载、不共享任何静态状态 | 全部 | 任何跨 addon 通道设计必须以「无共享静态」为前提；若未来合并回单库，P0.01.01 的传输层整体删除 |
| `ASM-B-02` | 渲染管线 | Slint 1.13.1 `SoftwareRenderer`：**实现** `draw_rectangle`/圆角边框/`draw_text`/线性渐变/元素 `opacity`（`apply_opacity` 乘入 state alpha）；**stub** `draw_path`/`draw_box_shadow`/`rotate`/带圆角 `combine_clip`。单帧 600×140@2x ≤ 1.5ms（`BUDGET-LAT-03`） | ②③ | P0.02.01 的像素探针复测推翻任一项时，受影响任务卡（P0.02.01、P1.02.02、P1.02.04）逐张回改；升级 Slint 大版本时重跑探针 |
| `ASM-B-03` | 交互延迟 | 按键到候选像素 P99 ≤ 16ms（`BUDGET-LAT-01`）；宿主回调 P99 ≤ 2ms；跨线程唤醒 ≤ 50µs；指针事件处理 ≤ 16ms（`$4` 默认红线） | ①③ | P0.03.01 落地后以 `criterion` 复测唤醒链；超标则把事件批上限（`event_batch`）与队列容量回退到契约默认 |
| `ASM-B-04` | 空闲纪律 | 空闲 CPU ≤ 0.3% 单核、重绘次数 0、无轮询定时器（`BUDGET-CPU-01`）；UI 线程 `poll(2)` 只等 eventfd + 连接 fd + 动效截止 | ③④ | P1.02.02 修隐形弹簧后若仍不达标，按 `scripts/idle-cpu-check.sh` 的探针逐唤醒源排查 |
| `ASM-B-05` | 数据量级 | 候选页 ≤ 9/页 × 5 页 = 45 可达候选（`MAX_REACHABLE_CANDIDATES`）；宽度估算 LRU 512 条；词库 ≤ 20MB；用户库 ≤ 50 万行 | ①④ | 布局与命中检测按页常数；若分页上限调整，`geometry::compute` 与 `layout::grid` 同步回归 |
| `ASM-B-06` | 硬件边界 | 本机 WSL2/WSLg：X11（XWayland）档与 `just check-host` 可真跑；Wayland 四档（layer-shell/popup/canvas）源码在库但**不在构建内**（`platform/mod.rs:34` 仅声明 x11），合成器为 Weston（四档之外） | ②③④ | Wayland 档验证依赖外部环境（`features.md` 0.5.5 已登记）；HiDPI 修复（P0.04.03）在 X11 档用 mock scale=2 断言，真机 2x 屏列实验室项 |
| `ASM-B-07` | 性能红线（`$4` 默认） | 重构全程不得劣化现状：交互响应 ≤ 16ms、稳帧 60/120fps、全部 `BUDGET-*` 键不回归；任何触碰预算路径的卡必须附 `criterion` 或探针断言 | 全部 | 每张卡 DoD 含预算键断言；回归即门禁失败，不是警告 |
| `ASM-B-08` | 架构约束（`$5` 默认） | 沿用 `features.md` 0.4 全部规则：零网络、无 async 运行时、SPSC+eventfd、解码纯函数、不夺焦点、`unsafe` 白名单、契约冻结（改动需 ADR）；本文件新增契约诉求一律走 **ADR-0005 增量追加**（只追加不重排） | 全部 | 追加被否决时，P0.01.04 改用 `StatusStrip` 承载高亮位的降级方案，P0.03.01 改用 UI 线程自轮询连接的降级方案（两者都已登记在各卡） |
| `ASM-B-09` | 跨 addon 通道 | 两 addon 同进程加载；fcitx5 以 `dlopen` 装载 addon（符号默认 `RTLD_LOCAL`），因此 `dlsym(RTLD_DEFAULT, …)` **不可依赖**；但 `dlopen(name, RTLD_NOLOAD)` 可取回已加载库的句柄并对其 `dlsym`——这是 P0.01.01 握手方案的进程模型基础 | ① | 若 fcitx5 未来以独立命名空间装载 addon 导致 `RTLD_NOLOAD` 失配，按 P0.01.01 的降级方案 D（宿主 InputPanel 载体）落地 |
| `ASM-B-10` | 验证基建 | xtask 的 `testd::uiframe`（帧镜像）与 `testd::capture`（像素采集）通道已建成、已测、**未接**进子命令树与插件（`#![allow(dead_code)]` 注记在案）；本文件 P0.01.01 负责 ADR-0007 的最小接线（`test-mirror` 发布侧） | ①②③④ | 若 E2E 通道接线延期，各卡 DoD 中的像素断言降级为 mock-backend 单测断言并显式登记 |

---

## 2. 缺陷总清单 (Defect Inventory)

> 编号规则：`DEF-NN` 按断层维度分组连续编号；「绑定任务」列给出承接卡号（本文件卡 = `REFACTOR-*`；外部卡以文档简称标注，如 `KEY-P2.02.01` = `docs/dev/opt-keymap/phase-3.md`）。**每一条缺陷至少有一个任务卡承接；每一张任务卡在 5.1 追溯表回填其缺陷编号，双向一致，零遗漏。** 除特别标注 SUSPECTED 外全部为 CONFIRMED（审计代理逐行读码核实 + 主 Agent 复核）。

### 2.1 维度① 全状态矩阵

| 缺陷编号 | 缺陷描述（含 `文件:行号`） | 商业标杆差距 | 绑定任务 |
|---|---|---|---|
| `DEF-01` | **帧通道断链（根缺陷）**：引擎对每个输入上下文正常解码并产出 `UiFrame`，`router/effects.rs:123,215,231` 依次投递 `Frame`/`Show`/`Hide`，但生产端 `FcitxHost::post`（`ffi/abi/engine/host.rs:129-134`）把命令**全部丢弃**并记 `ui/not-ready`；同时 UI addon 经 `register_takeover`（`ime-ui-addon/src/ui_impl/takeover.rs:84-95`）抑制宿主 ClassicUI，自绘窗口却永远收不到帧。用户在 X11 + 窗口就绪的会话里**看不到任何候选** | macOS 原生候选条：按键即见候选 | `REFACTOR-P0.01.01` |
| `DEF-02` | **事件回程断链**：UI 线程把悬停/点击/翻页/关闭收集进 `UiEventQueue`（`surface/input.rs:59-101`），宿主侧 API `UiThread::poll_event`（`ui_thread.rs:244`）与 `engine/router.rs:478` 的 `ui_event` 处理链在生产代码中**零调用方**（仅测试）——点击候选永远无法提交 | 搜狗/Rime：点击即上屏 | `REFACTOR-P0.01.02` |
| `DEF-03` | **锚定断链**：引擎的 `KeyRouter::set_anchor`（`engine/router.rs:511-515`）无生产调用方，每个上下文恒持默认锚点「首屏原点、无缩放」（`router.rs:136-140`）；UI addon 的光标三级来源解析梯（`cursor/resolver.rs`）与 `on_cursor_rect` 环形存储（`ui_impl/cursor_rects.rs:33-44`）同样无生产调用方。即使帧通了，窗口也钉在屏幕角落 | 所有现代 IME：候选窗贴 caret | `REFACTOR-P0.01.03` |
| `DEF-04` | **键盘高亮不随帧移动**：`UiFrame` 契约无高亮字段（`ime-types/src/ui.rs:120-136`），`machine.rs:602-620` 的 `build_frame` 丢弃 `paging.highlight`；视图侧 `PointerState::default()` 钉死 `Some(0)`（`adapter/cell.rs:156-166`）、`sync_pointer`（`surface/input.rs:119-129`）仅回抄旧值。方向键移动的是引擎的高亮而窗口的环永远亮在第一格——**环对「Space 将提交什么」说谎**（四弹簧系统只飞一次） | 微软拼音/macOS：高亮环与提交目标恒一致 | `REFACTOR-P0.01.04` |
| `DEF-05` | **主题通道无生产者**：`UiCommand::Theme` 在非测试代码中零构造；UI addon 的 `config` 步骤是 `pending_step` 桩（`ime-ui-addon/src/addon.rs:176-184`），窗口永远画 `.slint` 默认暗色盘；`theme.scheme`/`theme.accent`/`ui.base_alpha`/`ui.corner_radius_dp` 四键声明未读；`ThemeResolution::resolve`（`theme.rs:392`）无生产数据源 | 搜狗/macOS：主题即时换肤、跟随系统深浅色 | `REFACTOR-P0.01.05` |
| `DEF-06` | **Overlay 全链死（五断点）**：速查表/命令面板/诊断面板——(a) 触发：`Ctrl+Shift+/` 在生产路由里匹配无行而透传应用（`engine/rows.rs` 无该行；`Dispatcher::dispatch` 仅测试调用）；(b) 内容：`OverlayFrame`/`OverlaySection` 非测试构造零处；(c) 通道：见 DEF-01；(d) 渲染：`SurfaceUpdate::Overlay` 仅保留不画（`surface.rs:202-209`），`.slint` 无任何 overlay 组件；(e) 关闭：Escape 关闭逻辑位于未接线的 panel 层。外部承接：触发/契约见 `KEY-P2.02.01`、`KEY-P2.02.02`、`ADD-FEAT-P1.02.01` | Raycast/Linear：⌘K 即达、Esc 即关 | `REFACTOR-P1.02.01` |
| `DEF-07` | **焦点回调空壳**：`on_focus_in`/`on_focus_out` 是带注释的空桩（`ffi/abi/engine.rs:100-112`，注释自认「losing focus is the project's highest-severity defect」）；`SessionEvent::FocusLost` 处理链完整（`transitions.rs:433-448`）但无生产者；上下文表仅在显式 `deactivate` 时回收（`router.rs:329-339`），宿主销毁上下文（vtable 无 `context-destroyed` 槽）即泄漏整会话 | 一切商业 IME：焦点丢失即收窗、会话即回收 | `REFACTOR-P0.04.02` |
| `DEF-11` | **readonly 锁标记无生产者**：UI 侧全通（`adapter/frame.rs:335` → `adapter.rs:603-608` → `candidate.slint:299-306`），引擎侧 `Modes::status` 恒默认展开（`modes.rs:73-80`）；`ime_dict::paths::is_readonly_mode()`（`paths.rs:297`，置位于 373）零生产调用方。数据目录只读时学习静默停摆，锁图标永不亮 | 搜狗：任何降级都有可见标识 | `REFACTOR-P0.01.06` |
| `DEF-12` | **`has_user_dict_hit` 与 `StatusStrip.script` 双向死字段**：前者无生产方也无绘制方（`adapter/frame.rs:311-319` 明言不画）；后者无写无读；`KeyAction::ToggleScript` 被路由认领后仅重发帧（`transitions.rs:173-179`），`ime-core::script` 转换机制（`script.rs:137-241`）无生产调用方——**认领了键却什么都没做** | 微软拼音：简繁切换一键生效 | `REFACTOR-P0.01.06`（字段处置）＋ `REFACTOR-P2.05.04`（绑定决策） |
| `DEF-13` | **全角/标点位无消费者**：`Shift+Space`/`Ctrl+.` 翻转 `is_full_width`/`is_punct_full`（`modes.rs:137-144`），除状态条图标外无任何读者；提交路径没有标点输出，`passthrough::to_full_width`（`ime-core/src/passthrough.rs:40`）未接线——切换键重绘了一个不会改变行为的开关 | 任意 IME：切换即改变输出 | `REFACTOR-P0.01.07` |
| `DEF-14` | **Ctrl+Space 中英切换恒 true**：`EffectHost::toggle_enabled` = set 后读回（`effects.rs:253-257`），生产 `FcitxHost::is_enabled` 恒 `true`、`set_enabled` 为空体（`ffi/abi/engine/host.rs:116-127`），`Modes::apply(ToggleLang)` 永远写回 `is_chinese = true` 且认领按键（`router.rs:445,451`）——宿主若未截获该键，则用户失去中英切换且无任何反馈 | 一切 IME：中英切换立即可见 | `REFACTOR-P0.01.07` |
| `DEF-15` | **非组合态模式切换零反馈**：`on_key_idle` 对三个模式动作不做任何事也不发帧（`transitions.rs:116-146`）；状态簇只存在于组合态候选窗内，英/全角/标点态在无组合时不可见（无托盘、无常驻条） | macOS 输入法菜单：模式随时可见 | `REFACTOR-P0.01.07`（组合内反馈）＋ 外部 `ADD-FEAT-P1.02.06`（常驻指示） |
| `DEF-16` | **词典缺失降级 UX 未建**：`RecoveryOutcome::DictMissing` 承诺「候选窗告知词典不可用」（`ime-dict/src/recover.rs:100-103`），实际无任何禁选与文案；解码降级 passthrough 单候选（`viterbi/scratch.rs:246-251`）的 `degraded` 标志无消费者——用户看到「只会回显拼音的输入法」，把安装问题误读为排序 bug | 一切商业 IME：损坏即明确告知与引导 | `REFACTOR-P0.01.06` |
| `DEF-17` | **字体缺失静默豆腐**：`FontStatus::error()` → `ui/font/missing-cjk`（`renderer/probe.rs:147-158`）在唯一调用点被丢弃（`surface.rs:144-155`），无日志无提示 | 一切 GUI：缺字体至少告警 | `REFACTOR-P0.01.06` |
| `DEF-18` | **静默诊断代码群**：`ui/theme/blur-unavailable`/`contrast-fallback`（`theme_diagnostics()` 产出后无人消费，`surface.rs:320-332,376`）、`ui/buffer/starvation`（计数器，`slint_platform.rs:195-199`）、`ui/select/timeout`（计数器，`channel/event.rs:157`）、`decode/abbrev-truncated`（`scratch.rs:232,278` 置位无读者）——为契约而存在的代码，运维手册 grep 不到任何产出 | 可诊断性是商业 IME 底线（`features.md` 0.1 第 7 条） | `REFACTOR-P0.01.06` |
| `DEF-34` | **reload 管线完整但无触发**：`on_config_reload`（`addon/config.rs:259-287`）→ `KeyRouter::reload` 全链就绪，唯一调用方是测试；`start_config_watch` 仅记 `lifecycle/pending`（`config.rs:173-183`）。用户改配置必须重启 fcitx5，且无人告知 | VS Code：改配置即时生效或明确提示重启 | `REFACTOR-P1.05.01`（诚实化）＋ `REFACTOR-P2.05.05`（宿主槽位） |
| `DEF-41` | **接管仅一次尝试**：`register_takeover` 在加载序列只跑一次，`SURFACE_READY_DEADLINE = 80ms`（`ime-ui-addon/src/addon.rs:94,238-245`）——窗口迟到（慢盘、字体预热超时）就永远错过接管，ClassicUI 与自绘窗的取舍定格在启动后 80ms | 一切插件：就绪即接入，不靠启动竞速 | `REFACTOR-P0.01.06`（就绪晚到重试） |

### 2.2 维度② 材质视觉秩序

| 缺陷编号 | 缺陷描述（含 `文件:行号`） | 商业标杆差距 | 绑定任务 |
|---|---|---|---|
| `DEF-20` | **opacity 认知与渲染器事实相反**：代码库五处断言「软件光栅忽略元素 opacity / 绑定 opacity 会使子树什么都不画」（`theme.slint:116-123`、`candidate.slint:163-167,680-684,750-753`、`adapter.rs:634-637`、`layout/metrics.rs:618-636` 及其 `binds_opacity` 门禁、`adapter/tests.rs:480-496` 的实测注释）；而 i-slint-core 1.13.1 的 `apply_opacity` 将 opacity 乘入 state alpha（`software_renderer.rs:2497-2499`），编译器把 `opacity:` 降为 Opacity 项（`i-slint-compiler-1.13.1/passes.rs:147-154`），生成代码含 4 个 Opacity 项；且 `adapter/tests.rs:452` 的 disabled 用例实测「dim=0.32 的子树照常画」——直接证伪「绑定即不画」。实测注释以「ink 计数」为证据属**度量误读**（非零 α 像素计数不随 α 变化）。一个被门禁固化的假平台约束 | 工程决策建立在真实现而非传说之上 | `REFACTOR-P0.02.01` |
| `DEF-21` | **appear 动效淡入半边未接**：`window-opacity` 每帧被写、绑定到无（`candidate.slint:552`；`adapter.rs:638` 注释自认）；3.3.2 的 `opacity 0→1` 交叉淡入分量缺失，出现动效只剩几何缩放 | macOS 弹出：缩放+淡入复合 | `REFACTOR-P0.02.01` |
| `DEF-25` | **对比度门禁盲区**：`ContrastReport` 只测全 α `text.primary` 的三对（`theme.rs:307-317`）；实际绘制的五对降 α 文本（序号 0.55、注音 0.50、音节分隔 0.40、mode 标签/直通 0.62）从未被测——`contrast_ratio` 本就支持半透明前景展平（`color.rs:149-158`），只是没喂。手工核算暗色盘：不透明底上 ≈5.5/5.1/4.7（今日过线），**亚克力最坏合成底 `#3E3E40` 上 ≈4.1/3.8/2.9（全线不达 4.5）**——半透明档回归时门禁不会发现 | WCAG 纪律覆盖每一个绘制的文本对 | `REFACTOR-P1.02.03` |
| `DEF-26` | **亚克力半透明档不可达**：`apply_theme` 对 `spec.acrylic` 硬编码 `BlurNegotiation::Refused`（`surface.rs:361-377`），基础色永远不透明；`BlurSurface` 能力与 `request_blur`（`theme.rs:179-216`）无调用方；X11 后端未实现 `_KDE_NET_WM_BLUR_BEHIND_REGION`。设计的三档材质退化为单档 | macOS 毛玻璃/KWin blur：半透明底 + 合成器模糊 | `REFACTOR-P1.02.04` |
| `DEF-29` | **死 token 群**：`accent-on` 双侧声明无元素引用（`theme.slint:91`；`theme.rs:246`）；`text-annotation` 作为文本色已死（v1.4 裁决后只余只读锁描边使用，`candidate.slint:304`）；`shadow-band-opacity`/`shadow-inner-opacity` 被 Rust 解析（`layout/metrics.rs:330-331`）但绘制路径零读者（`theme.slint:131-135` 自认）；`HighlightStep.damage` 由弹簧计算无人读（`spring/highlight.rs:242-275`；`spring/set.rs:26` 注释失实） | Token 体系零死项（opt-ui 的 DEF 门禁精神） | `REFACTOR-P1.01.01` |

### 2.3 维度③ 微交互手感

| 缺陷编号 | 缺陷描述（含 `文件:行号`） | 商业标杆差距 | 绑定任务 |
|---|---|---|---|
| `DEF-19` | **指针事件延迟无上界**：`event_fd()` 恒 `None`（`surface.rs:234-236`），循环只等 eventfd（`ui_thread/event_loop.rs:104-122`）；X11/Wayland 连接 fd 存在但不可达——`X11Backend::connection_fd`（`platform/x11.rs:270-272`）与 Wayland 同名方法（`wayland/backend.rs:147-148`）无人能调：冻结契约 `SurfaceBackend` 无 fd 方法（`ime-types/src/surface.rs:79-136`），平台层存的是 `Box<dyn SurfaceBackend>`（`slint_platform.rs:75`）。代码自注「指针事件在下一次唤醒才被投递」。悬停/按压/点击在用户停顿后可能迟到数秒 | Raycast/Linear：指针反馈 ≤ 一帧 | `REFACTOR-P0.03.01` |
| `DEF-22` | **消失动效从不渲染 + 隐形弹簧空转**：`close()` 注释「无消失动效」却先 `window.hide()` 后启动 `motion.disappear()`（`adapter.rs:348-365`），且循环随即退出（`event_loop.rs:107-109`）；普通 Hide 路径上 `advance` 仍报告 ~90ms `animating`，循环以 6.944ms 截止不断醒来（`surface.rs:107-113,260-273`），而 `render_if_dirty` 不检查可见性——**每次隐藏都悄悄违反 `BUDGET-CPU-01`，且用户得不到 3.3.2 的 90ms 淡出** | macOS 弹出层：退场与入场同样从容 | `REFACTOR-P1.02.02` |
| `DEF-24` | **预编辑宽度估算失效并违反 3.1.3 保尾原则**：`ASCII_EM = 0.5` 低估 `W/M/@`（≈0.8–0.95em）导致 `kept_start` 保留过多，外层再逐 run 各自 `…`、`clip: true` 从**右侧**（最新输入端）裁切（`adapter/preedit.rs:302-333`、`candidate.slint:394-399`）——与 3.1.3「保尾弃头」正好相反；左缘渐隐 overlay（`candidate.slint:438-443`）指着不存在的裁切。高估方向（希腊/西里尔按 1.0em）则过早省略 | 微软拼音：预编辑裁切方向恒正确 | `REFACTOR-P1.03.03` |
| `DEF-27` | **120ms 交叉淡变无人调用**：`CubicBezier::EASE_IN_OUT`、`CROSSFADE_S` 及全套测试就绪（`spring/transition.rs:24-31,563-599`），但状态图标按颜色门硬切（`candidate.slint:261-306`）、主题切换是纯属性写（`adapter.rs:285-292`）——3.3.2 规定的两处 120ms crossfade 缺席 | Linear/Raycast：一切状态变化有过渡 | `REFACTOR-P1.03.01` |
| `DEF-28` | **指针无光标形状 + Active 无按压微动**：`ui/*.slint` 无 `TouchArea`/cursor 设定，X11 后端无光标调用；3.4 Active 行的「3% 下沉（60ms）」明确未实现（`candidate_grid.slint:136-139` 注释让位于 3.3.2，但 3.3.2 也未做） | Things 3/Raycast：按压有物理下沉 | `REFACTOR-P1.03.02` |
| `DEF-30` | **check-host 在负载下脆弱（测试基建）**：`FRAME_TIMEOUT = 90s`（`ime-ui-addon/src/addon/tests.rs:47`），本次审计在并行构建负载下实测 `test_the_pre_created_window_redraws_for_a_new_frame` 超时失败、单跑 48.9s 通过——门禁对机器负载敏感 | CI 门禁必须可重复 | `REFACTOR-P2.05.02` |

### 2.4 维度④ 暗部工程韧性

| 缺陷编号 | 缺陷描述（含 `文件:行号`） | 商业标杆差距 | 绑定任务 |
|---|---|---|---|
| `DEF-08` | **崩溃取证层整体未接线**：`install_panic_hook`（`ime-diag/src/panic.rs:61`）、`install_signal_handlers`（`crash/signal.rs:247`）、`set_crash_directory`/`set_crash_recovery`/`set_crash_context_provider`（`crash.rs:94,121,138`）零生产调用方；两个 addon 的 `guard_ffi` 只写 stderr（`ime-fcitx5/src/ffi/mod.rs:113-131`、`ime-ui-addon/src/ffi/mod.rs:121-131`），从不调 `record_ffi_panic`（`crash.rs:308`）；探针快照（`addon/probes.rs:139`）同样无调用方。SIGSEGV（正是 mmap 截断词库的既 documented 场景，`crash/signal.rs:19-24`）杀死 fcitx5 无任何记录；无飞行记录器 | 商业客户端：崩溃必留取证 | `REFACTOR-P0.04.01` |
| `DEF-09` | **FFI panic 行绕过节流**：`guard_ffi_with` 直接 `write_line` 而非走 `emit_through`（`ffi/mod.rs:124-132` 引擎侧；`121-131` UI addon 侧），节流器近在咫尺（`ffi/mod.rs:139-147,288-318`）且自身文档要求按窗口去重——确定性 panic 将每键两行（guard 行 + 默认 hook 行）无限刷屏 | 诊断通道永远有速率上限 | `REFACTOR-P0.04.01`（步骤 4） |
| `DEF-10` | **PhraseWriter 退出丢词**：契约写明「优雅关闭不丢行，`PhraseWriter::shutdown` 负责等待」（`router/phrases/deferred.rs:32-39,355-381`），但 `on_addon_destroy`（`addon.rs:210-231`）只冲用户库与备份、不触写手；`session_host::shutdown`（`session_host.rs:501-506`）直接 drop 整个宿主，`Drop` 刻意不 join（`deferred.rs:384-399`）——64 槽 outbox 里的用户短语随进程静默蒸发 | 用户数据零静默丢失 | `REFACTOR-P0.04.04` |
| `DEF-23` | **X11 档 HiDPI 结构性断裂**：预创建表面钉死 `PRE_CREATED_SCALE = FALLBACK_SCALE = 1.0`（`ime-ui-addon/src/platform/probe.rs:36,239-247`、`screen.rs:48`）；`classify_event` 永不产生 `Scale` 事件（`platform/x11.rs:547-599`），`apply_size` 用旧 scale 反推逻辑尺寸（`347-362`）；而锚点 scale 是客户端输出真实值（`cursor/resolver.rs:226-247`），`PlacementRequest` 明确假设「光栅 scale = anchor.scale」（`geometry.rs:214-215`）。**任何 >1× 会话：窗口半尺寸、命中表/交互区/摆位全部错位**；Wayland `adopt_scale` 修复路径是死源码（`platform/mod.rs:34` 未声明 wayland 模块） | Retina 级支持是 `features.md` 0.1 第 3 条的明示目标 | `REFACTOR-P0.04.03` |
| `DEF-35` | **迁移原子替换缺 `sync_all`**：`ime-config/src/migrate.rs:653-671` 写临时文件后直接 rename，无 fsync——与全部兄弟路径（`ime-dict/src/recover.rs:463`、`user_db/backup.rs:600`、`format/writer.rs:268`）不一致；断电最坏重迁移（幂等），非损坏 | 落盘纪律全库一致 | `REFACTOR-P2.04.01` |
| `DEF-36` | **crash 记录数量无上限**：`crash/record.rs:252-271` 一 panic 一文件、毫秒级重名仅重试 `MAX_NAME_ATTEMPTS`、无总量修剪——panic 循环会灌满 crash 目录（当前因 DEF-08 未接线而为潜伏项，接线前必须补） | 取证通道自身必须有界 | `REFACTOR-P0.04.01`（步骤 5） |
| `DEF-37` | **字体探测 join 无超时**：`renderer/probe.rs:212-221` 起线程后 `handle.join().unwrap_or(...)`，病态字体/渲染缺陷可把 `rspinyin-ui` 线程永久停靠——`mark_ui_ready` 永不到来，ClassicUI 兜底但自绘窗永不出现且线程泄漏 | 任何辅助线程不得无限期拖住主链 | `REFACTOR-P2.04.02` |
| `DEF-38` | **UI vtable `is_available` 槽位无 panic guard**：C++ glue 直调 vtable 槽 `vt->is_available()`（`ui_glue.cpp:275-279`），槽体（`ffi/abi.rs:397-399`）不在 guard 内（守卫版 `rspinyin_ui_available` 是另一个导出，`:288-292`）；函数体只读两个原子，现实可 panic 面为零，但违反「每个 `extern "C"` 体都跑在 guard 内」的全库不变量 | FFI 纪律零例外 | `REFACTOR-P2.05.03` |

### 2.5 维度⑤ 信息架构与键盘流

| 缺陷编号 | 缺陷描述（含 `文件:行号`） | 商业标杆差距 | 绑定任务 |
|---|---|---|---|
| `DEF-31` | **host-suspend 标志无消费者**：`on_host_suspend`/`on_host_resume` 是真实 FFI 槽（`ime-ui-addon/src/ffi/abi.rs:298-313`）写 `HOST_UI_SUSPENDED`（`ui_impl/availability.rs:79-90`），文档称「候选窗可见性随之」（`availability.rs:77-78`），但 `is_host_ui_suspended` 全库无读者——宿主挂起 UI 时活动组合的窗口不会被隐藏 | 宿主状态变化即视图状态变化 | `REFACTOR-P2.05.01` |
| `DEF-32` | **README 谎报热重载 + 配置样例过时**：两份 README 宣称「改动即下一键生效无需重启」（`README.md:136-138`、`README.zh.md:119-120`），与 `addon.rs:62-69` 的 ABI 现实相反；样例仍写 `schema_version = 1`（实际 `CONFIG_SCHEMA_VERSION = 2`，`ime-types/src/version.rs:41`）；约 15 个已文档化键从样例缺席 | 用户文档与产品行为逐字一致 | `REFACTOR-P1.05.01` |
| `DEF-33` | **配置面 18 键声明未读**：`engine.punct_mode`/`full_width`/`auto_english_on_uppercase`/`passthrough_url`（唯一读者 `passthrough.rs` 的 `classify` 无生产调用方）、`engine.abbrev`（`DecodeFlags::ABBREV` 仅测试置位）、`ui.base_alpha`/`corner_radius_dp`（随 DEF-05）、`[ui.animation]` 全部 5 键（弹簧常数硬编码 `spring.rs:82-97`；`set_motion_enabled` 仅测试调用；UI addon 配置步骤是桩）、`[data]` 全部 3 键（`addon/user_store.rs:34-37` 明言未应用）、`diagnostics.level`/`log_rotation_mb`/`log_keep_files`（订户先于配置安装，`addon/diagnostics.rs:25-31`）、`scheme.custom.initials/finals`（校验后搁置，`scheme.rs:31-38`；自定义双拼恒 `SchemeUnsupported`，`shuangpin/mod.rs:252-265`） | 配置文件的每个键都必须是真的 | `REFACTOR-P0.01.05`（主题 4 键）＋ `REFACTOR-P0.01.07`（标点组）＋ `REFACTOR-P1.04.01`（其余） |
| `DEF-39` | **键盘工作流缺口组**：四个已实现且被执行器支持的 `KeyAction` 无任何绑定行——`ToggleScript`/`ForgetHighlighted`/`PinHighlighted`/`AddPhrase`（`key.rs:51-64`；`rows.rs:157-264` 与 `engine.rs:282-303` 均不产出；`router.rs:560-563` 自注）；无首/末页跳转（`key.rs:32-34` 仅 Next/Prev）；可绑定键名宇宙仅 10 个、每表上限 6（`schema.rs:49,194-215`）；模式和弦是 `const` 表不可重绑（`engine.rs:282-303`）。外部承接：键位编辑 UI 见 `ADD-FEAT-P1.02.03`/`P1.02.04`，速查面板键位预留见 `KEY-P1.02.08` | 微软拼音：删词/造词一键可达 | `REFACTOR-P2.05.04` |
| `DEF-40` | **模块文档失实（文档烂尾）**：`session_host.rs:31-39` 仍写「Nothing installs the host at load … every callback answers its documented safe default」，而 `session-host` 步骤已装宿主（`addon/session.rs:144-150`）——运维读模块注释会误判整个引擎未激活 | 注释与代码同源可信 | `REFACTOR-P1.05.01`（步骤 4） |

---

## 3. 五大断层维度的工艺基线（本形态落地）

> 本节把通用工艺标准重映射到「无 DOM/CSS、双 cdylib、软件光栅」形态，作为任务卡 Design Specs 的引用基线。数值凡与 `features.md` 3.1–3.3 相关表冲突，以 `features.md` 为准。

### 3.1 材质与层级（→ 维度②）

- **透明度的事实标准**：以 P0.02.01 的像素探针结论为唯一事实源。当前源码级结论：元素 `opacity` 生效（state alpha 乘法）、「绑定即不画」为讹传；`draw_path`/`drop-shadow` 确为 stub，既有八段阴影环与 12 列箭头是正确应对，**保留**。
- **材质三档**：`applied`（合成器 blur + `ui.base_alpha`）/`opaque`（拒绝时强制 255 + `ui/theme/blur-unavailable` 通知位）/`contrast-fallback`（对比门禁失败升不透明 + 通知位）。P1.02.04 把第一档真正接通；三档切换必须落诊断（P0.01.06）。
- **交叉淡变**：一切颜色/标记的状态变化走 `CROSSFADE_S = 0.12s` 的 `EASE_IN_OUT`（P1.03.01 接线），图标簇宽度不随状态变化（既有 fixed-width 契约保留）。

### 3.2 全状态矩阵（→ 维度①）

- **用户可见状态的最小闭环**： composing（候选窗）、idle-with-mode（通知位/常驻条，外部 `ADD-FEAT-P1.02.06`）、degraded（只读/词典缺失/字体缺失/无 blur——每态必有 strip 通知位文本 + 稳定码日志双落）。
- **通知位契约**：`StatusStrip` 不加字段——复用 `mode_label` 的既有语义（3.6 已规定 header 承载降级通告），由 `Modes::status` 增设 `notice: Option<&'static str>` 内部源，优先级 `词典缺失 > 只读 > 字体缺失 > blur`，中文文案，码与文案一一映射（表在 P0.01.06 卡内冻结）。
- **诚实性三律**（P0.01.07 的验收基线）：不认领不产生效果的键；不点亮不改变行为的开关；不展示无生产者的状态。

### 3.3 微交互与物理动效（→ 维度③）

- **指针链路预算**：backend fd 进 `poll(2)` 后，按下到绘制 ≤ 一帧（≤ 6.944ms 动效期 / ≤ 50µs 唤醒 + 光栅预算）；以 `benches/wakeup_latency.rs` 同型基准补 `pointer_to_pixel`。
- **动效完备性**：appear = 缩放（已有）+ 淡入（P0.02.01 接线）；disappear = 淡出完成后 unmap，弹簧空转归零（P1.02.02）；highlight = 四弹簧（已有，等 P0.01.04 供真实目标）；page slide（已有）；press-in = 3% / 60ms（P1.03.02，走既有 spring 机制不新增定时器）。
- **宽度估算**：估宽器维持「无字体度量」边界（ASM-09），但校准 ASCII 表（`W/M/@/%/数字` 分级）并修复保尾方向（P1.03.03）；`overflow: elide` 仍是最后防线。

### 3.4 暗部工程（→ 维度④）

- **取证四件套**：panic hook + 信号 handler + crash 目录 + FFI guard 落记录，全部经 `ime-diag` 既有 API（P0.04.01）；记录总量有上限（默认 32 份，超额删最旧）；记录写入走既有 `0600/0700` 权限纪律。
- **卸载完整性**：`on_addon_destroy` 顺序 = 摘除接管 → 发 Hide → `PhraseWriter::shutdown(budget)` → 用户库 flush → 日志 flush → UI 线程停机（P0.04.04）；任何一步超时记录后继续，不阻塞宿主 250ms 预算。
- **HiDPI**：X11 档以锚点 scale 为唯一事实源，`Show`/frame 时差异即重建表面尺寸与 `Window.scale-factor`（P0.04.03）；命中表与摆位已按 anchor.scale 计算，修复后自动对齐。

### 3.5 信息架构与键盘流（→ 维度⑤）

- **速查表**：内容 = 活动绑定表的投影（引擎侧 builder，P1.02.01 卡定义 `OverlayFrame` 组装规则）；渲染 = 候选窗同族 surface（同 `SurfaceBackend`/`ThemeTokens`/弹簧，遵守 `features-add.md` C-1 约束）；触发/消散键位走 `KEY-P2.02.02`。
- **配置诚实化**：README 与首启模板逐键对齐 `schema.rs`；启动时对「改了但需重启」的键组落一行 `lifecycle/pending` 升级版通知（P1.05.01）。

---

## 4. 重构落地与工程执行清单

### 4.1 WBS 任务覆盖追溯表 (Traceability Matrix)

> 「绑定任务」中出现的外部卡（`KEY-*`/`ADD-*`/`UI-OPT-*`）已在本文件第 2 节逐行标注，不重复开卡。

| 缺陷编号 | 断层维度 | 并行通道 | 核心缺陷（一句话） | 绑定任务节点清单 |
|---|---|---|---|---|
| `DEF-01` | 全状态矩阵 | Track A | 引擎 UiCommand 全丢弃 + 接管吞 ClassicUI → 盲打 | `REFACTOR-P0.01.01` |
| `DEF-02` | 全状态矩阵 | Track A | UiEvent 回程零生产消费方 | `REFACTOR-P0.01.02` |
| `DEF-03` | 全状态矩阵 | Track A | 锚点恒屏幕原点，解析梯无调用方 | `REFACTOR-P0.01.03` |
| `DEF-04` | 全状态矩阵 | Track A | 高亮环钉死第一格，对提交目标说谎 | `REFACTOR-P0.01.04` |
| `DEF-05` | 全状态矩阵 | Track A | 主题命令无生产者，外观键全死 | `REFACTOR-P0.01.05`；`DEF-33` 部分同卡 |
| `DEF-06` | 信息架构 | Track B | 速查表/面板五断点全死 | `REFACTOR-P1.02.01` ＋ 外部 `KEY-P2.02.01/02`、`ADD-FEAT-P1.02.01` |
| `DEF-07` | 全状态矩阵 | Track A | 焦点回调空壳、会话泄漏 | `REFACTOR-P0.04.02` |
| `DEF-08` | 暗部工程 | Track A | 崩溃取证层整体未接线 | `REFACTOR-P0.04.01` |
| `DEF-09` | 暗部工程 | Track A | FFI panic 行绕过节流 | `REFACTOR-P0.04.01`（步骤 4） |
| `DEF-10` | 暗部工程 | Track A | 短语写手退出静默丢词 | `REFACTOR-P0.04.04` |
| `DEF-11` | 全状态矩阵 | Track A | 只读锁无生产者，学习静默停摆 | `REFACTOR-P0.01.06` |
| `DEF-12` | 全状态矩阵 | Track A/C | has_user_dict_hit / script 双向死字段 | `REFACTOR-P0.01.06`（处置）；`REFACTOR-P2.05.04`（绑定决策） |
| `DEF-13` | 全状态矩阵 | Track A | 全角/标点位无输出消费者 | `REFACTOR-P0.01.07` |
| `DEF-14` | 全状态矩阵 | Track A | Ctrl+Space 恒 true、吞键无效 | `REFACTOR-P0.01.07` |
| `DEF-15` | 全状态矩阵 | Track A/C | 非组合态模式切换零反馈 | `REFACTOR-P0.01.07` ＋ 外部 `ADD-FEAT-P1.02.06` |
| `DEF-16` | 全状态矩阵 | Track A | 词典缺失降级 UX 未建 | `REFACTOR-P0.01.06` |
| `DEF-17` | 全状态矩阵 | Track A | 字体缺失静默豆腐 | `REFACTOR-P0.01.06` |
| `DEF-18` | 全状态矩阵 | Track A | 静默诊断代码群（blur/contrast/starvation/select-timeout/abbrev-truncated） | `REFACTOR-P0.01.06` |
| `DEF-19` | 微交互 | Track C | 指针事件延迟无上界 | `REFACTOR-P0.03.01` |
| `DEF-20` | 材质视觉 | Track B | opacity 认知与渲染器事实相反（含门禁与误测注释） | `REFACTOR-P0.02.01` |
| `DEF-21` | 材质视觉 | Track B | appear 淡入半边未接像素 | `REFACTOR-P0.02.01` |
| `DEF-22` | 微交互 | Track B | 消失动效不渲染 + 隐形弹簧违反空闲预算 | `REFACTOR-P1.02.02` |
| `DEF-23` | 暗部工程 | Track C | X11 HiDPI 结构性断裂 | `REFACTOR-P0.04.03` |
| `DEF-24` | 微交互 | Track B | 预编辑估宽失效、保尾变保头 | `REFACTOR-P1.03.03` |
| `DEF-25` | 材质视觉 | Track B | 对比度门禁五对降 α 文本盲区 | `REFACTOR-P1.02.03` |
| `DEF-26` | 材质视觉 | Track B | 亚克力档硬编码不可达 | `REFACTOR-P1.02.04` |
| `DEF-27` | 微交互 | Track B | 120ms 交叉淡变实现后无人调用 | `REFACTOR-P1.03.01` |
| `DEF-28` | 微交互 | Track B | 指针无光标形状、Active 无按压微动 | `REFACTOR-P1.03.02` |
| `DEF-29` | 材质视觉 | Track B | 死 token 群（accent-on/text-annotation/shadow 峰值/damage） | `REFACTOR-P1.01.01` |
| `DEF-30` | 微交互（测试基建） | Track C | check-host 90s 超时对负载脆弱 | `REFACTOR-P2.05.02` |
| `DEF-31` | 信息架构 | Track C | host-suspend 标志无消费者 | `REFACTOR-P2.05.01` |
| `DEF-32` | 信息架构 | Track B | README 谎报热重载、样例过时 | `REFACTOR-P1.05.01` |
| `DEF-33` | 信息架构 | Track A | 配置面 18 键声明未读 | `REFACTOR-P0.01.05`（主题 4 键）；`REFACTOR-P0.01.07`（标点组 4 键）；`REFACTOR-P1.04.01`（其余 10 键） |
| `DEF-34` | 信息架构 | Track C | reload 无触发且用户不知情 | `REFACTOR-P1.05.01`（诚实化）；`REFACTOR-P2.05.05`（宿主槽位） |
| `DEF-35` | 暗部工程 | Track C | 迁移原子替换缺 sync_all | `REFACTOR-P2.04.01` |
| `DEF-36` | 暗部工程 | Track A | crash 记录数量无上限 | `REFACTOR-P0.04.01`（步骤 5） |
| `DEF-37` | 暗部工程 | Track C | 字体探测 join 无超时 | `REFACTOR-P2.04.02` |
| `DEF-42` | 暗部工程（验收基建） | Track C | E2E 验证通道断链：`uiframe`/`capture` 已建成未接进子命令树与插件，真渲染缺陷只能靠偶然暴露 | `REFACTOR-P0.01.08` |
| `DEF-38` | 暗部工程 | Track C | UI vtable is_available 无 guard | `REFACTOR-P2.05.03` |
| `DEF-39` | 信息架构 | Track C | 四动作无绑定行 + 首末页跳转缺失 + 和弦不可重绑 | `REFACTOR-P2.05.04` ＋ 外部 `ADD-FEAT-P1.02.03/04`、`KEY-P1.02.08` |
| `DEF-40` | 信息架构 | Track B | session_host 模块注释失实 | `REFACTOR-P1.05.01`（步骤 4） |
| `DEF-41` | 全状态矩阵 | Track A | 接管仅一次尝试，窗口迟到即永久错过 | `REFACTOR-P0.01.06`（步骤 6） |
| `DEF-42` | 暗部工程（验收基建） | Track C | E2E 验证通道已建成未接线 | `REFACTOR-P0.01.08` |

**DAG 校验结论**：31 张卡（P0 × 14、P1 × 10、P2 × 7）、依赖边 14 条，全部由小编号指向大编号（卡内「前置依赖」列逐张登记），拓扑排序可行，**无环**。解耦点与契约冻结顺序：`REFACTOR-P0.01.01` 冻结跨 addon wire 契约（ADR-0011）后，`P0.01.02/03/04/05/08` 五卡方可并行开工。

### 4.2 关键路径与并行通道汇总

- **关键路径（CP）**：`P0.01.01（帧通道贯通，含 ADR-0011）→ P0.01.02（事件回程）→ P0.01.03（锚定贯通）→ P0.01.04（高亮随帧）→ P0.01.08（E2E 验收接线）`。CP 上五卡 + P0.04.02 是「产品从盲打变可用且可验证」的最短链，任何一卡延期整体交付顺延。
- **Track A（底座与状态/暗部工程，6 卡）**：`P0.01.01`、`P0.01.02`、`P0.01.03`、`P0.01.04`、`P0.04.02`、`P0.04.04`。
- **Track B（视觉与组件重塑，4 卡 P0 + 全部 P1 视觉卡）**：`P0.01.05`、`P0.01.06`、`P0.01.07`、`P0.02.01` + `P1.01.01/02.01/02.02/02.03/02.04/03.01/03.02/03.03/05.01/04.01`。
- **Track C（交互·键盘流·基建，3 卡 P0 + 全部 P2 卡）**：`P0.03.01`、`P0.04.03`、`P0.01.08` + `P2.04.01/04.02/05.01/05.02/05.03/05.04/05.05`。
- **并行约束**：P0 四张通道卡（01.01–01.04）共享 `ffi/abi` 与 `ime-types` 契约面——**01.01 合入前其余三卡只允许在各自分支完成实现与单测，禁止改 `Cargo.toml`/`lib.rs`/契约文件**；01.01 合入后按 01.02 → 03 → 04 顺序串行合入，避免契约文件三方冲突。P0.02.01 与 Track A 无文件交集，可立即并行。

### 4.3 P0 任务卡（原子级展开；P1/P2 卡分片见 §6）

---

#### [REFACTOR-P0.01.01] REFACTOR-P0.01.01：跨 addon 帧通道贯通（引擎 → 候选窗）

- **基本属性**：
  - 绑定缺陷编号：`DEF-01`
  - 优先级与难度预估：`P0` | 高复杂度 | 预估工时: 5.0 人天
  - 前置依赖：无
  - 关键路径：`CP: 是`
  - 并行通道：`Track A`
  - 代码落地锚点：`crates/ime-fcitx5/src/ffi/abi/engine/host.rs`、`crates/ime-fcitx5/src/ffi/cpp/engine_glue.cpp`、`crates/ime-ui-addon/src/ffi/cpp/ui_glue.cpp`、`crates/ime-ui-addon/src/ffi/abi.rs`、`crates/ime-ui-addon/src/addon.rs`、`crates/ime-types/src/ui.rs`（只读）、`docs/dev/adr/0011-frame-transport.md`（新建）、`xtask/src/testd/uiframe/mirror.rs`（发布侧接线）
  - 当前状态：`[x] 已完成`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`ffi/abi/engine/host.rs:129-134` 的 `post()` 是带注释的空操作——每个 `UiCommand` 落地即丢弃并记 `ui/not-ready`；同进程内另一条 cdylib（`librspinyin_ui.so`）拥有完整的渲染管线却收不到任何命令。两库不共享静态（ADR-0003/0004，`ASM-B-01`），fcitx5 的 `dlopen` 使 `dlsym(RTLD_DEFAULT)` 不可依赖（`ASM-B-09`）。
  - **商业标杆对标**：macOS 输入法的候选窗由独立进程经 Mach 端口供帧——帧通道是输入法的「第一条生产线」；本项目管线的每一环（契约、队列、渲染、损害合并）都已建成且测试充分，唯独缺这根线。
- **设计规范与参数定义**：
  - **方案 C（推荐）：进程内符号握手 + `#[repr(C)]` wire 镜像**。C++ glue 互寻对方句柄：`dlopen("librspinyin_ui.so", RTLD_NOLOAD | RTLD_LAZY)` 取回已加载句柄（`RTLD_NOLOAD` 对 `RTLD_LOCAL` 装载的库同样返回句柄），`dlsym(handle, "rspinyin_ui_frame_sink_register")` 完成注册；反向（引擎供事件消费回调）同一次握手带上。需要新 ADR-0011 登记：新增导出符号、不动既有 vtable 槽位（ADR-0002 路径，无需 bump `RSPINYIN_ABI_VERSION`）。
  - **wire 镜像**（冻结形状，追加式演进；字段次序即 ABI）：

    ```rust
    // crates/ime-fcitx5/src/ffi/abi/engine/transport.rs (new)
    //! The cross-addon wire mirrors. #[repr(C)], append-only, borrowed for one call.
    // The sink the engine calls per UiCommand; the UI glue copies out of every
    // pointer before returning, so nothing here outlives the call.
    #[repr(C)]
    pub struct RspinyinFrameWire {
        pub revision: u32,
        pub kind: u32,          // 0 = Frame, 1 = Show, 2 = Hide, 3 = Theme, 4 = Overlay(open), 5 = Overlay(closed)
        pub preedit: RspinyinStr,       // UTF-8, borrowed
        pub caret: u32,
        pub spans: *const RspinyinSpanWire,  // syllable/separator/passthrough runs
        pub span_count: u32,
        pub candidates: *const RspinyinCandidateWire,
        pub candidate_count: u32,
        pub page_current: u8,
        pub page_total: u8,
        pub page_size: u8,
        pub mode_label: RspinyinStr,
        pub flags: u32,         // bit0 full_width, bit1 punctuation_full, bit2 readonly, bit3 has_user_dict_hit
        pub script: u32,        // StatusStrip::Script wire value
        pub cursor: RspinyinRect,       // anchor cursor rect
        pub screen: i32,
        pub scale: f32,
        pub placement: u32,
        pub max_per_row: u8,
        pub show_annotation: u8,
        pub max_width_dp: u16,
        pub theme_accent: u32,  // 0xRRGGBB; Theme only
        pub theme_scheme: u32,  // Theme only
        pub hide_reason: u32,   // Hide only
    }
    ```

    候选/跨度子结构同样 `#[repr(C)]`、`ptr+len` 借用；`RspinyinStr = { ptr: *const u8, len: u32 }`。合法性校验复用 `bytes_from_raw`（`ffi/abi.rs:150`）。
  - **Overlay 载体**：`kind = 4/5` 的载荷是另一种形状（分组表而非候选列表），单靠 `RspinyinFrameWire` 的候选数组装不下——wire 家族因此为两条：`RspinyinFrameWire`（kind 0/1/2/3）与 `#[repr(C)] RspinyinOverlayWire { title: RspinyinStr, kind: u32, selected: i32, query: RspinyinStr, sections: *const RspinyinOverlaySectionWire, section_count: u32 }`（`RspinyinOverlaySectionWire { title: RspinyinStr, entries: *const RspinyinOverlayEntryWire, entry_count: u32 }`，`RspinyinOverlayEntryWire { keys: RspinyinStr, label: RspinyinStr }`），与 `ime-types/src/ui.rs:84-110` 的 `OverlayFrame` 三层结构一一对应。sink 回调签名携带 `*const c_void` + kind 判别，或注册两个槽位（frame sink / overlay sink）——实施取两槽位方案（判别分支更少、类型不混装）。
  - **握手句柄获取的 spike 前置**：`dlsym(RTLD_DEFAULT, …)` 是否可用取决于 fcitx5 装载 addon 的 `RTLD_GLOBAL/LOCAL` 标志（版本差异风险），`dlopen(RTLD_NOLOAD)` 的名字匹配取决于宿主传入的确切路径串。**步骤 0 为 spike**：在真机上以 `dlinfo`/`RTLD_DI_LINKMAP` 核实两机制，结论写进 ADR-0011；实现采用**双机制探测**（先 `RTLD_DEFAULT`，失败再以描述符解析出的同一路径串 `RTLD_NOLOAD`），双失败即降级方案 D（宿主 InputPanel 载体，有损）并登记诊断 `ui/transport/degraded`（新码）。
  - **背压语义**：sink 直接转发进 `UiThread::send`（`Send + Sync`，不阻塞）；`UiCommandSender` 自身的 latest-wins/ordered 语义不变——**跨 addon 传输不新增任何队列**，契约 2.2.1 的通道语义表保持唯一事实源。
  - **降级方案 D（若 `RTLD_NOLOAD` 在目标宿主失配）**：引擎把候选列表写入宿主 `InputPanel`（`setCandidateList`），UI addon 从既有 `PanelMirror` 建帧；**丢失 span 种类/注音/来源**，候选窗以无分隔符、无注音的降级形态工作——两方案的取舍登记进 ADR-0011。
- **工程实现方案与代码级细节**：
  - 引擎侧生产实现（替换 `host.rs:129-134` 的空体）：

    ```rust
    impl HostCtx for FcitxHost {
        fn post(&mut self, command: UiCommand) {
            // The sink is registered by the UI addon's glue during the handshake and
            // is a plain function pointer; before it arrives the command is dropped
            // exactly as before, and the registered degradation keeps its code.
            match sink() {
                Some(sink) => sink.post(&wire_of(command)),
                None => emit_diagnostic(UI_NOT_READY_CODE),
            }
        }
    }
    ```

  - UI addon 侧：`ui_glue.cpp` 导出 `rspinyin_ui_frame_sink_register(...)`，转发进 Rust `ffi/abi.rs` 的 guard 包裹入口，最终落到 `UiStartup` 持有的 `UiThread`；握手在 `on_addon_init` 的 `ui-registration` 步骤内完成（晚于 `ui-startup`，句柄必然就绪）。
  - 事件回程的注册在同一握手内声明（回调指针、上下文指针），P0.01.02 使用。
  - **暗部防线**：sink 回调体内禁止分配（借用指针直拷）；`wire_of` 每键一次组装，`Vec` 复用上下文槽位；panic 由 `guard_ffi` 兜底并落 crash 记录（P0.04.01 接线后自动生效）。
- **交互矩阵与全状态覆盖**：Frame/Show/Hide/Theme/Overlay(open/closed) 六类命令全通过；`ui/not-ready` 在 sink 未注册窗口期保持原语义；关闭/重启往返（addon unload → sink 悬空 → 置回 `None`）有测试。
- **逐步落地实施步骤**：
  1. `[步骤 0]` 握手机制 spike（真机核实 `RTLD_DEFAULT`/`RTLD_NOLOAD` 两机制在 fcitx5 5.1.7 装载语义下的可用性，`dlinfo` 记录装载标志）——结论决定 ADR-0011 的主方案与降级触发条件。
  2. `[步骤 1]` 写 ADR-0011（方案 C/D 取舍、wire 冻结形状、握手时序、降级登记），主 Agent 裁决后冻结。
  3. `[步骤 2]` wire 镜像（frame + overlay 两族）+ `wire_of` 组装 + 单测（每字段往返字节一致；`preedit`/`candidates` 借用期不超过调用）。
  4. `[步骤 3]` 引擎 `post()` 生产实现 + UI glue 注册与转发 + `on_addon_destroy` 注销。
  5. `[步骤 4]` xtask `test-mirror` 发布侧接线：`Frame` 命令落到 `UiFrameMirror::publish`（`testd/uiframe/mirror.rs` 已建复用），补 ADR-0007 最小闭环。
  6. `[步骤 5]` E2E：`just check-host` 下注入测试断言「Show+Frame 后窗口 `painted_pixels > 0`」（复用 `addon/tests.rs:425` 用例骨架，走真 sink）。
- **验收标准 (DoD)**：
  - [ ] 真机 X11 会话：输入 `ni` 候选窗显示「你/泥」，方向键翻页可见（E2E，`test-mirror` 镜像帧一致）。[实验室]
  - [ ] `ui/not-ready` 仅在 sink 未注册窗口期出现，注册后零新增。[自动]
  - [ ] wire 往返单测全绿（含 Overlay 三层结构的往返）；`cargo nextest run -p ime-fcitx5 -p ime-ui-addon` 全绿。[自动]
  - [ ] 预算：`post` 路径 criterion 基线落档并**同步向 `budgets.json` 登记 `post_ui` 阈值键**（沿用 opt-perf.md 已登记的 `PENDING_KEYS` 机制，禁止单卡私设阈值旁路 0.5.3 唯一事实源），断言经 `xtask budget --check` 判定。[性能]
  - [ ] `just ci` 全绿（含 check-host 与全部审计脚本）。[自动]
- **验收记录**：- **验收记录**（2026-10-05，opt-basic 第 3 轮）：实现落于本系列提交（35566f0…cbc16e4）；门禁 `cargo fmt --all -- --check`、clippy 三段零 Rust 警告、`cargo nextest run --workspace --all-features`（2969 通过）+ addon 594 通过、doctest 76 通过、`just ci` 全部审计绿（含新增 check-readme-keys 与 check-metrics-readers 自测）；`just check-host` 因本机无 libfcitx5core-dev 跳过（CI host-abi 作业覆盖）。
- **验收记录（实施与限制）**：跨 addon 帧通道：wire 镜像 + 符号握手 + sink 落 UiThread（ffi/abi/engine/transport.rs、ime-ui-addon/ffi/transport/、ADR-0011）；wire 全字段往返与 Overlay 三层结构测试全绿。已知限制：真机 XTEST E2E（输入 ni 候选窗显示）与 post_ui 预算真机采样为实验室项。

---

#### [REFACTOR-P0.01.02] REFACTOR-P0.01.02：事件回程贯通（候选窗 → 引擎）

- **基本属性**：
  - 绑定缺陷编号：`DEF-02`
  - 优先级与难度预估：`P0` | 中复杂度 | 预估工时: 2.0 人天
  - 前置依赖：`REFACTOR-P0.01.01`
  - 关键路径：`CP: 是`
  - 并行通道：`Track A`
  - 代码落地锚点：`crates/ime-ui/src/ui_thread.rs`、`crates/ime-ui/src/channel/event.rs`、`crates/ime-ui-addon/src/addon.rs`、`crates/ime-fcitx5/src/ffi/abi/engine/transport.rs`、`crates/ime-fcitx5/src/session_host.rs`、`crates/ime-fcitx5/src/engine/router.rs`
  - 当前状态：`[x] 已完成`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`UiEventQueue` 与 `UiThread::poll_event`（`ui_thread.rs:244`）无生产消费方；`session_host.rs:452-454` 已有 `router.ui_event` 处理链但无 FFI 入口；点击候选在队列里堆积后溢出计数。
  - **商业标杆对标**：搜狗/Rime 鼠标点选即上屏；回程通道同时是覆盖层导航（Esc、面板高亮移动）的前提。
- **设计规范与参数定义**：
  - **排空模型**：新建专职排空线程（`ime-ui-addon` 内），阻塞在 `UiEventQueue::poll` 的既有 **Condvar** 等待上（`channel/event.rs:248` 的 `ready.wait_timeout`——投递方 notify、等待方零超时长眠，**无轮询定时器**；等待时长取「超长 deadline」形态，`event.rs:226` 的语义允许）。**线程归属约束（关键）**：`session_host::ui_event`（`session_host.rs:451-457`）与全部 `with_sessions` 回调的既有契约是「只在 Fcitx5 主循环线程执行」（`session_host.rs` 锁纪律段 + `ASM-11`/`ASM-12`）——排空线程**禁止直接调用**引擎会话层。事件的合法路径是：排空线程 → C++ glue 的**主循环投递**（`fcitx::EventLoop::addPostEvent` / `postEvent` 族，fcitx5 官方的跨线程编组机制）→ 主循环回调内再进 `rspinyin_event_ingest`。glue 侧不可用 post-event 的宿主版本，退化为排空线程内的小型 SPSC + 主循环侧 eventfd 唤醒（自建，登记进 ADR-0011）。
  - **wire**：`#[repr(C)] RspinyinEventWire { kind: u32 /* Select/Hover/Page/Dismiss */, revision: u32, index: u16, reason: u32 }`。
- **工程实现方案与代码级细节**：

  ```rust
  // crates/ime-ui-addon/src/events.rs (new)
  //! Drains the UI event queue into the engine addon.
  // Blocks on the queue's own wakeup primitive; a UI thread that never posts
  // costs this thread nothing (no polling timer, no wakeups).
  pub(crate) fn run_drain(thread: &UiThread, post: impl Fn(UiEvent) -> bool, stop: &AtomicBool) {
      while !stop.load(Ordering::Acquire) {
          // Long-deadline condvar wait (channel/event.rs): a queue that never
          // posts costs this thread zero wakeups; there is no polling timer.
          match thread.poll_event(Duration::from_secs(3600)) {
              Some(event) => { if !post(event) { /* engine gone: keep draining, drop */ } }
              None => {}
          }
      }
  }
  ```

  引擎侧新导出 `rspinyin_event_ingest(ic_id: u64, wire: *const RspinyinEventWire) -> bool`（命名表明「引擎吞入 UI 事件」的方向，避免 `rspinyin_ui_event` 的归属歧义），guard 包裹，**仅可由主循环线程调用**（排空线程经 glue 的 post-event 编组到达），转发 `router.ui_event(ic, event, host)`；未知 `ic` 记 `ffi/stale-ic`（沿用既有码）。锚点上行复用同一条 ingest 通道（见 `REFACTOR-P0.01.03`）。
- **交互矩阵与全状态覆盖**：`Select`（含防抖与 stale revision 拒绝——`transitions.rs:519-528` 已就绪终于可被真实流量检验）、`Hover`（16ms 节流语义保持）、`Page`/`Dismiss` 有序；引擎不存在（addon 先卸）时事件丢弃计数 `ui/event/engine-gone`。
- **逐步落地实施步骤**：
  1. `[步骤 1]` wire + 引擎导出 + guard 单测。
  2. `[步骤 2]` UI addon 排空线程 + 生命周期（随 `on_addon_init` 起、`on_addon_destroy` 停）。
  3. `[步骤 3]` E2E：镜像通道下点选候选 → 提交文本一致。
- **验收标准 (DoD)**：
  - [ ] E2E：XTEST 注入鼠标点击第 1 候选 → 提交「你」。（[实验室]）
  - [ ] 排空线程空闲 60s 零唤醒（`BUDGET-CPU-01`）。[自动]
  - [ ] stale-revision 点击被拒且计数（单测）。[自动]
  - [ ] `just ci` 全绿。[自动]
- **验收记录**：- **验收记录**（2026-10-05，opt-basic 第 3 轮）：实现落于本系列提交（35566f0…cbc16e4）；门禁 `cargo fmt --all -- --check`、clippy 三段零 Rust 警告、`cargo nextest run --workspace --all-features`（2969 通过）+ addon 594 通过、doctest 76 通过、`just ci` 全部审计绿（含新增 check-readme-keys 与 check-metrics-readers 自测）；`just check-host` 因本机无 libfcitx5core-dev 跳过（CI host-abi 作业覆盖）。
- **验收记录（实施与限制）**：事件回程：排空线程 + event_outlet（ime-ui-addon/src/ffi/transport/event_outlet.rs）+ rspinyin_event_ingest；stale revision 拒绝与顺序投递已测。已知限制：E2E 点选上屏实验室项；空闲零唤醒以收敛后零 deadline 断言背书。

---

#### [REFACTOR-P0.01.03] REFACTOR-P0.01.03：光标锚定贯通（caret → anchor 上行）

- **基本属性**：
  - 绑定缺陷编号：`DEF-03`
  - 优先级与难度预估：`P0` | 中复杂度 | 预估工时: 2.5 人天
  - 前置依赖：`REFACTOR-P0.01.01`、`REFACTOR-P0.01.02`（锚点上行复用事件回程的 ingest 通道，不另立握手回调）
  - 关键路径：`CP: 是`
  - 并行通道：`Track A`
  - 代码落地锚点：`crates/ime-ui-addon/src/ui_impl/cursor_rects.rs`、`crates/ime-ui-addon/src/cursor/resolver.rs`、`crates/ime-fcitx5/src/ffi/abi/engine/transport.rs`（`RspinyinEventWire` 增 `Anchor` kind）、`crates/ime-fcitx5/src/engine/router.rs`、`crates/ime-fcitx5/src/ffi/cpp/engine_glue.cpp`
  - 当前状态：`[x] 已完成`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：解析梯（`cursor/resolver.rs`：归一化 → 前端绝对矩形 → 窗口几何 → 兜底）与 8 槽环形存储（`ui_impl/cursor_rects.rs:52-88`）全部建成、全部测试覆盖、零生产调用方；`KeyRouter::set_anchor`（`router.rs:511-515`）同样闲置，每上下文锚点恒为首屏原点（`router.rs:136-140`）。
  - **商业标杆对标**：一切现代 IME 的候选窗贴 caret ≤ 6dp；本项目的摆位、翻转、命中表全部以锚点为输入（`geometry.rs`），锚点错了其余全错。
- **设计规范与参数定义**：锚点上行**复用事件回程的 ingest 通道**（优化裁决 IMPR-01：同方向、同线程模型、同 wire 机制，独立回调只会多一套握手与一个导出符号）——`RspinyinEventWire` 增 `kind = Anchor` 变体与锚点五元组字段（cursor/screen/scale/placement，追加式）；UI addon 在 `on_cursor_rect` 到来时经解析梯产出 `Anchor`，随事件通道上行，引擎在主循环回调内写对应上下文（`router.rs` 的 `Context.anchor`），下一帧 `build_frame` 即携带真实锚点。解析梯是纯函数（`ScreenEnumerator`/`WindowGeometrySource` 注入式），生产装配点在 `ime-ui-addon::platform` 探针产物（`ScreenLayout`）就绪后；两 tier 均不可用（`CursorTier::Fallback`）时仍上行（兜底锚点 = 既有屏幕中心规则），`platform/cursor/unresolved` 码保持既有节流语义。
- **工程实现方案与代码级细节**：`on_cursor_rect` 现在只入环形存储——追加解析调用；解析梯是纯函数（`ScreenEnumerator`/`WindowGeometrySource` 注入式），生产装配点在 `ime-ui-addon::platform` 探针产物（`ScreenLayout`）就绪后；两 tier 均不可用（`CursorTier::Fallback`）时仍上行（兜底锚点 = 既有屏幕中心规则），`platform/cursor/unresolved` 码保持既有节流语义。
- **交互矩阵与全状态覆盖**：多屏（输出切换 → `refresh_layout`）、缩放（`snap_scale` 五档）、退化矩形（零宽/负坐标）、窗口几何 tier 失败回退——全部复用解析梯既有测试表，新增一条 E2E：镜像帧的 `anchor.cursor` 与注入的 caret 偏差 ≤ 2px。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `RspinyinEventWire` 增 Anchor kind + `router.set_anchor` 接线（主循环回调内）。
  2. `[步骤 2]` UI addon：`on_cursor_rect` → 解析 → 上行；节流（同 anchor 值不重发）。
  3. `[步骤 3]` E2E + 多屏 mock 回归。
- **验收标准 (DoD)**：
  - [ ] E2E：候选窗水平中心与 caret 中心偏差 ≤ 2 物理像素、垂直 caret 下 ≤ 6dp。[实验室]
  - [ ] `KeyRouter::set_anchor` 生产调用方 ≥ 1；默认原点锚点仅存于「从未收到 rect」的上下文。[自动]
  - [ ] `just ci` 全绿。[自动]
- **验收记录**：- **验收记录**（2026-10-05，opt-basic 第 3 轮）：实现落于本系列提交（35566f0…cbc16e4）；门禁 `cargo fmt --all -- --check`、clippy 三段零 Rust 警告、`cargo nextest run --workspace --all-features`（2969 通过）+ addon 594 通过、doctest 76 通过、`just ci` 全部审计绿（含新增 check-readme-keys 与 check-metrics-readers 自测）；`just check-host` 因本机无 libfcitx5core-dev 跳过（CI host-abi 作业覆盖）。
- **验收记录（实施与限制）**：锚点上行：RspinyinEventWire Anchor kind + on_cursor_rect→解析梯→ingest→router.set_anchor；解析梯既有 mock 表回归。已知限制：E2E 锚点偏差 ≤2px 实验室项。

---

#### [REFACTOR-P0.01.04] REFACTOR-P0.01.04：高亮随帧（`UiFrame.highlight` 契约追加 + 视图接线）

- **基本属性**：
  - 绑定缺陷编号：`DEF-04`
  - 优先级与难度预估：`P0` | 中复杂度 | 预估工时: 2.0 人天
  - 前置依赖：`REFACTOR-P0.01.01`
  - 关键路径：`CP: 是`
  - 并行通道：`Track A`
  - 代码落地锚点：`crates/ime-types/src/ui.rs`（ADR-0005 追加）、`docs/dev/adr/0005-incremental-contract-extension.md`（补记）、`crates/ime-core/src/state/machine.rs`、`crates/ime-ui/src/surface/input.rs`、`crates/ime-ui/src/adapter/cell.rs`、`crates/ime-ui/src/adapter.rs`
  - 当前状态：`[x] 已完成`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`UiFrame` 无高亮字段（`ime-types/src/ui.rs:120-136`）；`build_frame`（`machine.rs:602-620`）丢弃 `self.paging.highlight`；视图侧 `PointerState::default()` 钉 `Some(0)`（`cell.rs:156-166`）、`sync_pointer` 回抄旧值（`surface/input.rs:119-129`）、`highlight_position()` 读同一字段（`adapter.rs:666-671`）。方向键在引擎里移动高亮，窗口的环与四弹簧只在初始时刻飞向第一格，此后永不再动——**焦点环对 Space 提交目标说谎**，这是可信度缺陷而不仅是动效缺陷。
  - **商业标杆对标**：macOS/微软拼音的键盘高亮是「提交目标」的唯一视觉承诺，任何时刻与回车/空格的实际提交一致。
- **设计规范与参数定义**：
  - **契约追加**（ADR-0005 增量路径，追加不重排）：`UiFrame` 新增尾字段 `pub highlight: Option<u16>`——**页内位置**（与候选切片同基；跨页全局索引在此换算），`None` = 无高亮（空页）。语义冻结注记：`None` 时视图隐藏环（`adapter.rs:648-671` 的既有隐藏分支），不做任何回退猜测。
  - **视图接线**：`sync_pointer` 改为 `highlighted: frame.highlight`（不再回抄自身旧值）；`PointerState::default()` 的 `Some(0)` 语义保留（首帧前的乐观默认），但首帧到达后一律以帧为准。
- **工程实现方案与代码级细节**：

  ```rust
  // crates/ime-core/src/state/machine.rs — build_frame 内
  let start = usize::from(self.paging.page_start()).min(candidates.len());
  // Page-local position of the keyboard highlight, or None when the page holds
  // no candidate for it. The view treats None as "hide the ring", never as
  // "keep the previous one": a ring that outlives its candidate is the defect
  // this field exists to prevent.
  let highlight = self
      .paging
      .highlight_position_in_page()
      .filter(|&pos| usize::from(pos) < end - start);
  ```

  （`Paging::highlight_position_in_page` = 既有 `highlight - page_start` 的 saturating 形式，落在 `paging.rs` 与 `local_position` 同一算术口径。）
- **交互矩阵与全状态覆盖**：方向键移动 → 环随帧飞行（四弹簧保留速度续接语义，`spring.rs:1169` 区段的设计不变）；翻页 → 新页首/尾落环；点击 hover → 环与 hover 共存规则维持 3.4 优先级（Focus > Hover）；空页/单候选/45 满页边界；`highlight: None` 时环隐藏且弹簧不飞。
- **逐步落地实施步骤**：
  1. `[步骤 1]` ADR-0005 补记 + `UiFrame.highlight` 追加 + wire（P0.01.01 的 `RspinyinFrameWire` 加 `highlight: u16 + has_highlight: u8` 两字段，追加式）。
  2. `[步骤 2]` `build_frame` 产出 + 视图 `sync_pointer` 消费 + 旧值回抄删除。
  3. `[步骤 3]` 表驱动测试：每方向键事件 → 断言帧内 highlight 与引擎 `Paging` 一致（复用 `engine/tests/table.rs` 驱动）。
- **验收标准 (DoD)**：
  - [ ] 连续 Tab × n / ↑↓ 后，环所在格 == Space 将提交的候选（E2E + 单测双断言）。[自动/实验室]
  - [ ] `UiFrame` 消费方（adapter、镜像 schema）全部编译期适配，`just ci` 全绿。[自动]
  - [ ] 无新增分配：`build_frame` 仍零分配（`alloc_budget` 门禁不回归）。[自动]
- **验收记录**：- **验收记录**（2026-10-05，opt-basic 第 3 轮）：实现落于本系列提交（35566f0…cbc16e4）；门禁 `cargo fmt --all -- --check`、clippy 三段零 Rust 警告、`cargo nextest run --workspace --all-features`（2969 通过）+ addon 594 通过、doctest 76 通过、`just ci` 全部审计绿（含新增 check-readme-keys 与 check-metrics-readers 自测）；`just check-host` 因本机无 libfcitx5core-dev 跳过（CI host-abi 作业覆盖）。
- **验收记录（实施与限制）**：高亮随帧：UiFrame.highlight（ADR-0005 增量）+ wire 两字段 + 视图 sync_pointer 以帧为准；表驱动测试钉「环所在格==Space 提交目标」；build_frame 仍零分配。

---

#### [REFACTOR-P0.01.05] REFACTOR-P0.01.05：主题与外观通道贯通（config → ThemeSpec → 窗口）

- **基本属性**：
  - 绑定缺陷编号：`DEF-05`、`DEF-33`（主题 4 键：`theme.scheme`/`theme.accent`/`ui.base_alpha`/`ui.corner_radius_dp`）
  - 优先级与难度预估：`P0` | 中复杂度 | 预估工时: 2.0 人天
  - 前置依赖：`REFACTOR-P0.01.01`
  - 关键路径：`CP: 否`
  - 并行通道：`Track B`
  - 代码落地锚点：`crates/ime-ui-addon/src/addon.rs`（`load_config` 桩）、`crates/ime-ui-addon/src/addon/tests.rs`、`crates/ime-config/src/lib.rs`（消费侧只读）
  - 当前状态：`[x] 已完成`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`ime-ui-addon/src/addon.rs:176-184` 的 `load_config` 是 `pending_step("config", "the configuration loader")` 桩；窗口永远画 `theme.slint` 的默认暗色盘；`ime-ui` 里建成的整套主题机制（`ThemeResolution::resolve` 对比度门禁、退化阶梯、`slint_palette` 字节级对齐）没有数据源。四键（scheme/accent/base_alpha/corner_radius_dp）在 `schema.rs` 有完整定义、校验、文档，无一被读。
  - **商业标杆对标**：搜狗/macOS：换主题即时生效、跟随系统深浅色、用户强调色贯穿选中环与状态点。
- **设计规范与参数定义**：
  - **装配点**：`load_config` 步骤真身——`ime-config` 已是本 crate 依赖（`Cargo.toml` 在案），装载路径**复用引擎 addon `addon/config.rs` 的既有 `ConfigStore` 装载形态**（同一 crate 的公共 API：加载 + 失败软处理 + 损坏文件挪位），不为此新增公共构造函数；产出 `ThemeSpec` 后经 P0.01.01 通道发 `UiCommand::Theme`。
  - **深浅色跟随**：v1 读配置 `theme.scheme`（`auto` 值若 schema 无则按 `Dark` 默认，登记限制）；portal 跟随列为 P1（外部 `ADD-FEAT-P1.03.03` 范畴），本卡不引入 portal 依赖。
  - **时序**：配置步骤先于 `ui-startup`（`INIT_STEPS` 表已有 `config` 在前——见 `ime-ui-addon/tests.rs:476` 的步骤断言，保持次序），主题命令在窗口就绪前发出，由 latest-wins 槽暂存，窗口 `show` 前生效——首帧即正确主题，无闪变。
- **工程实现方案与代码级细节**：

  ```rust
  // crates/ime-ui-addon/src/addon.rs — load_config 真身
  fn load_config() -> Result<(), ImeError> {
      // Same loading shape as the engine addon's config step: a damaged document
      // keeps the built-in defaults in force and never fails the addon.
      let store = ime_config::ConfigStore::load(&layout::config_path())?;
      let config = store.current();
      let spec = ThemeSpec::from(&config.theme, config.ui.base_alpha, config.ui.corner_radius_dp);
      post_to_window(UiCommand::Theme(spec));
      Ok(())
  }
  ```

  （`ConfigStore::load` 的确切构造签名以 `ime-config/src/reload/load.rs` 的既有装载 API 为准；`ThemeSpec::from` 为 `ime-types` 的构造辅助，若契约面不宜新增，改为在 `ime-ui-addon` 内组装字面量 `ThemeSpec { .. }`——两种写法按「契约冻结最小改动」原则取舍。）
- **交互矩阵与全状态覆盖**：暗→亮切换是属性更新不闪变（`theme.rs:509-513` 既有保证）；对比门禁失败自动升不透明 + `ui/theme/contrast-fallback`（P0.01.06 落日志）；`corner_radius_dp` 合法域 `8..=20`（`schema.rs`）超界由配置层既有 repair 兜底。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `load_config` 真身 + `ThemeSpec` 组装 + 发送。
  2. `[步骤 2]` 步骤测试：损坏配置 → 默认主题 + 无 fatal；合法配置 → 组件属性读回断言（`adapter/tests.rs` 同型）。
  3. `[步骤 3]` README 配置样例的 `[theme]`/`[ui]` 段同步（与 P1.05.01 联检）。
- **验收标准 (DoD)**：
  - [ ] 改 `theme.scheme = "light"` + 重启 → 候选窗浅色盘（E2E 截图走 `testd::capture` 接线后补）。[实验室]
  - [ ] 四键各有「配置 → 组件属性」单元测试。[自动]
  - [ ] 主题命令 latest-wins：连续两帧主题只有新者生效（复用 `channel/command.rs` 既有测试型）。[自动]
  - [ ] `just ci` 全绿。[自动]
- **验收记录**：- **验收记录**（2026-10-05，opt-basic 第 3 轮）：实现落于本系列提交（35566f0…cbc16e4）；门禁 `cargo fmt --all -- --check`、clippy 三段零 Rust 警告、`cargo nextest run --workspace --all-features`（2969 通过）+ addon 594 通过、doctest 76 通过、`just ci` 全部审计绿（含新增 check-readme-keys 与 check-metrics-readers 自测）；`just check-host` 因本机无 libfcitx5core-dev 跳过（CI host-abi 作业覆盖）。
- **验收记录（实施与限制）**：主题通道：load_config 真身 + ThemeSpec 投影 + PENDING_THEME 槽位首帧前生效；四键「配置→组件属性」单测与 latest-wins 语义已测。已知限制：theme.scheme=auto v1 解析为暗色（登记）；README [theme]/[ui] 段随 P1.05.01 对齐。

---

#### [REFACTOR-P0.01.06] REFACTOR-P0.01.06：状态矩阵诚实化（通知位 + 死代码落地 + 迟到重试）

- **基本属性**：
  - 绑定缺陷编号：`DEF-11`、`DEF-16`、`DEF-17`、`DEF-18`、`DEF-41`、`DEF-12`（字段处置）
  - 优先级与难度预估：`P0` | 中复杂度 | 预估工时: 3.0 人天
  - 前置依赖：`REFACTOR-P0.01.01`（通知位需通道可见）；`DEF-41` 子项无前置
  - 关键路径：`CP: 否`
  - 并行通道：`Track A`
  - 代码落地锚点：`crates/ime-fcitx5/src/engine/router/modes.rs`、`crates/ime-fcitx5/src/addon/session.rs`、`crates/ime-dict/src/paths.rs`、`crates/ime-ui/src/surface.rs`、`crates/ime-ui/src/renderer/probe.rs`、`crates/ime-ui-addon/src/addon.rs`、`crates/ime-types/src/decode.rs`（消费 degraded，只读）
  - 当前状态：`[x] 已完成`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：① `is_readonly_mode()`（`paths.rs:297`）零生产调用方，锁标记永不亮（DEF-11）；② 词典缺失的既诺 UX「候选窗告知不可用」（`recover.rs:100-103`）未建，`degraded` 标志（`decode.rs:260-268`）无消费者（DEF-16）；③ `ui/font/missing-cjk` 在唯一调用点被丢弃（`surface.rs:144-155`）（DEF-17）；④ 五类诊断码产出了没人落地的值（`theme_codes`、starvation、select-timeout、abbrev-truncated）（DEF-18）；⑤ 接管只有 80ms 一次机会（DEF-41）；⑥ `script` 死字段按「登记延后」处置（DEF-12）。
  - **商业标杆对标**：搜狗在词库损坏时弹明确提示；macOS 在缺字体时回退系统字体并告警。可诊断性是本项目自己的 0.1 第 7 条产品承诺。
- **设计规范与参数定义**：
  - **通知位**（3.2 节基线）：`Modes::status` 增设内部源 `notice`，产出进 `mode_label` 的显示文案——**冻结映射表**（码 ↔ 中文文案 ↔ 优先级）：

    | 稳定码 | 文案 | 优先级 |
    |---|---|---|
    | `dict/unavailable` | 词典不可用，仅直通输入 | 1（最高） |
    | `data/readonly-mode` | 用户词库只读，学习已暂停 | 2 |
    | `ui/font/missing-cjk` | 未找到中文字体，显示可能异常 | 3 |
    | `ui/theme/blur-unavailable` | 桌面不支持模糊，已切换不透明 | 4 |

    组合态下通知位与 mode label 并存时，通知占用 label 槽（3.1.3 的空间预算内，截断方向沿用既有规则）。**占用 label 槽有一个必须一并修的视图侧耦合**：`StatusCluster` 以 `chinese: mode-label != ""` 推断模式点状态（`candidate.slint:469`），通知文案非空会把英文模式点亮成「中」——这是**用文案空否推断语义状态**的脆弱耦合。修复走契约最小追加（ADR-0005）：`StatusStrip` 尾部追加 `pub chinese: bool`（引擎 `Modes` 本就持有 `is_chinese`，零推导），adapter 写入新组件属性，`StatusCluster.chinese` 改绑该属性、废除空否推断。
  - **诊断落地统一出口**：`theme_codes`（`surface.rs:330`）与字体探测状态在 UI addon 侧经 `emit_diagnostic` 落地；`starvation`/`select-timeout` 计数器**不做周期外推**（那会引入轮询定时器，违反 `BUDGET-CPU-01` 的空闲纪律）——改在**计数增长点就地** `emit_diagnostic`（通道 post/放弃路径本就持有计数上下文，节流器 `ffi/mod.rs:288-318` 去重，一行一窗口）；`abbrev-truncated` 置位处补一行。
- **工程实现方案与代码级细节**：引擎侧 `Modes` 增字段 `notice: Option<&'static str>`；词典缺失事实来自 `session_sources()` 的 `Dictionary::Missing` 态（`session/sources.rs:49-97`）→ 经进程槽位供 `Modes::status` 读取（与 `readonly` 同路径：`paths::is_readonly_mode()` 直接成为生产调用方）。UI addon 侧 `probe_font_choice()` 的 `FontStatus::error()` 分支补 `emit_diagnostic`。**DEF-41 重试**：`ui-startup` 完成但错过 80ms 截止时，把「就绪晚到」事实带回平台步骤，`register_takeover` 追加一次（幂等既有保证：`takeover.rs` 的 never-a-second-time 例外恰为此登记——若实现为「宿主已选他人不再抢」则维持不变，仅补「首次评估时自身未就绪」分支）。
- **交互矩阵与全状态覆盖**：四类降级各自单测（状态注入 → strip 文案断言）；通知出现时 header 高度与截断预算回归（3.1.3）；只读 + 词典缺失叠加时高优先级胜出。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 引擎侧：`Modes::notice` + 两个事实源接线 + 冻结映射表测试。
  2. `[步骤 2]` UI addon 侧：字体/主题诊断落地 + 计数器外推。
  3. `[步骤 3]` `degraded` 标志消费：passthrough 单候选帧 + 通知位并存验证。
  4. `[步骤 4]` 迟到接管重试 + 步骤测试。
  5. `[步骤 5]` `script`/`has_user_dict_hit` 处置：在 `ime-types` 字段 doc 注明「v1 无生产者，登记延后」，删除 `adapter/frame.rs:311-319` 的误导性注释。
- **验收标准 (DoD)**：
  - [ ] 注入只读目录启动 → 锁图标亮 + 日志一行（E2E/实验室 + 单测双证）。[自动]
  - [ ] 挪走 `base.dict` 启动 → 「词典不可用，仅直通输入」可见。[实验室]
  - [ ] 五类诊断码各有落地断言（grep 门禁化：每码 ≥ 1 生产调用方，进 `check-self-tests`）。[自动]
  - [ ] `just ci` 全绿。[自动]
- **验收记录**：- **验收记录**（2026-10-05，opt-basic 第 3 轮）：实现落于本系列提交（35566f0…cbc16e4）；门禁 `cargo fmt --all -- --check`、clippy 三段零 Rust 警告、`cargo nextest run --workspace --all-features`（2969 通过）+ addon 594 通过、doctest 76 通过、`just ci` 全部审计绿（含新增 check-readme-keys 与 check-metrics-readers 自测）；`just check-host` 因本机无 libfcitx5core-dev 跳过（CI host-abi 作业覆盖）。
- **验收记录（实施与限制）**：状态诚实化：Modes 冻结 notice 表 + StatusStrip.chinese 直绑（ADR-0005）+ 五类诊断码生产调用方 + 字体/主题诊断落地 + 迟到接管重试；四类降级注入单测。已知限制：只读目录/挪走词典的真机走查实验室项。

---

#### [REFACTOR-P0.01.07] REFACTOR-P0.01.07：模式位语义闭环（诚实切换 + 输出接线 + 空闲反馈）

- **基本属性**：
  - 绑定缺陷编号：`DEF-13`、`DEF-14`、`DEF-15`、`DEF-33`（标点组 4 键：`engine.punct_mode`/`full_width`/`auto_english_on_uppercase`/`passthrough_url`）
  - 优先级与难度预估：`P0` | 中复杂度 | 预估工时: 3.0 人天
  - 前置依赖：`REFACTOR-P0.01.01`（反馈可见性）；`DEF-14` 子项无前置
  - 关键路径：`CP: 否`
  - 并行通道：`Track A`
  - 代码落地锚点：`crates/ime-fcitx5/src/engine.rs`（`CHORDS` 表）、`crates/ime-fcitx5/src/engine/router/modes.rs`、`crates/ime-fcitx5/src/ffi/abi/engine/host.rs`、`crates/ime-fcitx5/src/ffi/cpp/engine_glue.cpp`、`crates/ime-core/src/passthrough/punctuation.rs`、`crates/ime-core/src/state/transitions.rs`、`crates/ime-config/src/schema.rs`（消费侧只读）
  - 当前状态：`[x] 已完成`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：① `ToggleLang` 被认领但 `is_enabled` 恒 true / `set_enabled` 空体（`host.rs:116-127`）——切换永远「成功」且无变化（DEF-14）；② `is_full_width`/`is_punct_full` 翻转后除图标无任何读者，提交路径无标点输出（DEF-13）；③ 空闲态切换零反馈（DEF-15）；④ `engine.punct_mode` 等 4 键有配置无行为（DEF-33 部分）。
  - **商业标杆对标**：微软拼音：Shift+Space 切全角后下一个逗号就是全角逗号——开关改变输出，才配叫开关。
- **设计规范与参数定义**：
  - **诚实性三律**（3.2 节）为裁决基线：
    - `ToggleLang`：**从 `CHORDS` 移除**。ASM-02 规定语言切换是 fcitx5 宿主自有热键，5.1.7 头文件无每上下文状态可读写（`host.rs:23-31` 已论证）——插件继续认领它只可能吞键。`KeyAction::ToggleLang` 保留（未来宿主支持时恢复），`engine.rs` 表驱动审计测试同步缩表。
    - `ToggleFullWidth`/`TogglePunct`：保留认领，**补输出语义**——把 `ime-core::passthrough` 的 `to_full_width`（`punctuation.rs:116`）与 `PunctMode` 分类接进提交路径：`HostCtx::commit` 前按当前模式位变换文本（标点替换表 `passthrough/punctuation.rs` 已有，`auto_english_on_uppercase`/`passthrough_url` 按 `passthrough.rs:66-90` 的 `PassthroughFlags` 同路径接线）。**模式位从此有读者。**
    - **空闲态反馈**：`on_key_idle` 的模式动作改为产出单效果 `Effect::Diagnose` 之外的新效果 `Effect::ModeFlash { bit }`（v1 落点：诊断一行 `mode/full-width: on`——组合态窗口不在场，无 UI 面可画；常驻指示条是外部 `ADD-FEAT-P1.02.06` 的领地，本卡只保证「切换必有可观测反馈」且不吞键）。
- **工程实现方案与代码级细节**：提交变换收口在 `engine/router/effects.rs` 的 `commit` 效果执行处（唯一出口，`effects.rs` 的 `HostCtx::commit` 调用前插一层 `Modes::transform_output(text) -> Cow<str>`；缓冲复用，热路径零分配——全半角替换是逐字符查表）。`punct_mode`/`full_width` 初值从 `SessionConfig`/`Modes::default()` 与配置对齐（`schema.rs` 默认半角 + 中文标点，`modes.rs:69-77` 默认已一致）。
- **交互矩阵与全状态覆盖**：全角开 → `，。？！` 全角、ASCII 字母保持半角；标点半角模式 → `.` 输出 `.`；大写临时英文（`auto_english_on_uppercase`）进入/退出边界；URL 直通；三键各自「配置→行为」单测 + `keymap_matrix` 集成行追加；`ToggleLang` 移除后 Ctrl+Space 透传宿主（不再吞键）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `CHORDS` 移除 ToggleLang + 审计测试更新 + **回写 `features.md` 3.5 的按键表**（`Ctrl+Space` 行改注「由 fcitx5 宿主处理，插件不再认领」——本卡改变的是文档化行为，必须与权威规范同步）。
  2. `[步骤 2]` `Modes::transform_output` + 提交路径接线 + 4 配置键消费。
  3. `[步骤 3]` 空闲态 `ModeFlash` 效果 + 诊断落地（登记限制：无组合时无 UI 面可画，v1 的用户可观测反馈为「下一次组合的状态点正确」+ 常驻指示条由外部 `ADD-FEAT-P1.02.06` 承接）。
  4. `[步骤 4]` `keymap_matrix` 与路由表测试全量回归。
- **验收标准 (DoD)**：
  - [ ] 全角开 + 输入 `nihao,` → 提交「你好，」（E2E）。[实验室]
  - [ ] Ctrl+Space 在插件路由中不再被认领（表审计断言）。[自动]
  - [ ] 4 配置键各有行为断言；`just ci` 全绿。[自动]
- **验收记录**：- **验收记录**（2026-10-05，opt-basic 第 3 轮）：实现落于本系列提交（35566f0…cbc16e4）；门禁 `cargo fmt --all -- --check`、clippy 三段零 Rust 警告、`cargo nextest run --workspace --all-features`（2969 通过）+ addon 594 通过、doctest 76 通过、`just ci` 全部审计绿（含新增 check-readme-keys 与 check-metrics-readers 自测）；`just check-host` 因本机无 libfcitx5core-dev 跳过（CI host-abi 作业覆盖）。
- **验收记录（实施与限制）**：模式位闭环：CHORDS 移除 ToggleLang（features.md 3.5 已回写）+ Modes::transform_output 提交变换 + Effect::ModeFlash 空闲反馈（ADR-0005）+ classify 输入接入（大写直提交/URL 透传/标点表行，RoutingConfig 投影）。已知限制：组合态「标点携带候选」未做（会话机级特性，后续卡）；Host 无 surrounding-text 访问器，URL 规则降级为击键内证据。

---

#### [REFACTOR-P0.04.01] REFACTOR-P0.04.01：崩溃取证接线（hook / handler / guard 记录 / 上限）

- **基本属性**：
  - 绑定缺陷编号：`DEF-08`、`DEF-09`、`DEF-36`
  - 优先级与难度预估：`P0` | 中复杂度 | 预估工时: 2.5 人天
  - 前置依赖：无
  - 关键路径：`CP: 否`
  - 并行通道：`Track A`
  - 代码落地锚点：`crates/ime-fcitx5/src/addon/diagnostics.rs`、`crates/ime-ui-addon/src/addon/diagnostics.rs`（若与引擎侧共用步骤形状）、`crates/ime-fcitx5/src/ffi/mod.rs`、`crates/ime-ui-addon/src/ffi/mod.rs`、`crates/ime-diag/src/crash/record.rs`、`crates/ime-diag/src/probe/snapshot.rs`
  - 当前状态：`[x] 已完成`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：取证 API 层（`panic.rs:61`、`crash/signal.rs:247`、`crash.rs:94,121,138,308`）建成且自测充分，但两个 addon 的装配序列一行未调；`guard_ffi` 只写 stderr（`ffi/mod.rs:113-131`/`121-131`）且绕过节流（DEF-09）；`record.rs` 无总量上限（DEF-36）。mmap 截断 → SIGSEGV 这个项目自己点名的场景（`crash/signal.rs:19-24`）今日不留任何取证。
  - **商业标杆对标**：Chrome/Flutter 的 minidump + 最近事件环形日志；商业 IME 崩溃后用户能拿到可交的材料。
- **设计规范与参数定义**：
  - 装配点 = 两 addon `diagnostics` 步骤（唯一的 fatal 步骤，天然是取证的家）：目录 `<data>/crash/`（`0700`/记录 `0600`，复用 `perms.rs`）；`set_crash_directory` + `install_panic_hook` + `install_signal_handlers` + `set_crash_context_provider`（提供 `ic 计数 / 会话状态位 / probe 快照`，全部结构化键，`CrashContextKey` 封闭集）；探针快照接 `addon/probes.rs:139` 既有出口。**provider 的 async-signal-safety 约束（关键）**：信号 handler 上下文里只允许原子读与无锁快照——provider 的实现必须从原子槽位取值（`probes.rs` 的计数器本就是原子），**禁止在 handler 路径取任何锁或分配**；需要互斥的富上下文（如会话表枚举）由 `crash/signal.rs:25` 设计的「handler 外补写」阶段完成，本卡实现时以该模块自己的两阶段设计为准，不得简化成 handler 内一步取全。
  - **上限（DEF-36）**：`record.rs` 写入前修剪——保留最新 `MAX_CRASH_RECORDS = 32` 份（按文件名时间戳排序删除最旧），常量进 `budgets.json` 不适用（非预算，纯常量），DoD 断言 33 次注入后目录仍 32 份。
  - **节流（DEF-09）**：`guard_ffi_with` 的失败行改走 `emit_through`（同 `ffi/mod.rs:288-318` 的节流器），并追加 `crash::record_ffi_panic`（panic payload 传入，guard 已持有）。
- **工程实现方案与代码级细节**：两 addon 的 `init_diagnostics` 各加四行装配（错误软失败：任一步失败记一行继续，取证层自己绝不 fatal）；`record.rs` 修剪函数 + 单测；FFI guard 改造后现有「每键两行」回归测试反向断言（注入 panic 的入口只产一行）。
- **交互矩阵与全状态覆盖**：panic（Rust hook）/ SIGSEGV/SIGBUS（handler）/ FFI guard（记录）三路都落 `<data>/crash/`；记录含 probe 快照与结构化上下文；记录内容零用户输入（`crash.rs` 隐私节既有保证）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 两 addon 装配 + 软失败测试。
  2. `[步骤 2]` guard 改造（节流 + record）+ 反向断言。
  3. `[步骤 3]` 记录上限 + 注入测试。
  4. `[步骤 4]` 探针快照接 provider。
- **验收标准 (DoD)**：
  - [ ] 注入 SIGSEGV（测试进程内 `raise`）→ 记录文件存在且为 `0600`。[自动]
  - [ ] 33 次连续 panic 注入 → 目录恒 32 份。[自动]
  - [ ] `emit_diagnostic` 节流断言：同码 100 次只产 1 行 + 计数行。[自动]
  - [ ] `just ci` 全绿（`check-unsafe` 不新增白名单条目——本卡零 unsafe）。[自动]
- **验收记录**：- **验收记录**（2026-10-05，opt-basic 第 3 轮）：实现落于本系列提交（35566f0…cbc16e4）；门禁 `cargo fmt --all -- --check`、clippy 三段零 Rust 警告、`cargo nextest run --workspace --all-features`（2969 通过）+ addon 594 通过、doctest 76 通过、`just ci` 全部审计绿（含新增 check-readme-keys 与 check-metrics-readers 自测）；`just check-host` 因本机无 libfcitx5core-dev 跳过（CI host-abi 作业覆盖）。
- **验收记录（实施与限制）**：崩溃取证接线：两 addon diagnostics 步骤装配崩溃目录/panic hook/signal handler/上下文 provider + guard_ffi 节流化 + record 32 份上限；SIGSEGV 注入、33 次修剪、节流断言全绿。

---

#### [REFACTOR-P0.04.02] REFACTOR-P0.04.02：焦点生命周期落地（on_focus_in/out + 会话回收）

- **基本属性**：
  - 绑定缺陷编号：`DEF-07`、`DEF-31`（联动：suspend 消费者在 P2.05.01，本卡先保证焦点链完整）
  - 优先级与难度预估：`P0` | 中复杂度 | 预估工时: 2.5 人天
  - 前置依赖：无（Hide 效果经 P0.01.01 通道落地，实现与测试可先行）
  - 关键路径：`CP: 是`
  - 并行通道：`Track A`
  - 代码落地锚点：`crates/ime-fcitx5/src/ffi/abi/engine.rs`（两个空桩）、`crates/ime-fcitx5/src/ffi/cpp/engine_glue.cpp`、`crates/ime-fcitx5/src/engine/router.rs`、`crates/ime-fcitx5/src/privacy_impl.rs`
  - 当前状态：`[x] 已完成`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`on_focus_in`/`on_focus_out` 是注释自嘲的空桩（`ffi/abi/engine.rs:100-112`：「losing focus is the project's highest-severity defect」）；`SessionEvent::FocusLost` 全链（`transitions.rs:433-448`：Hide + 清预编辑）无生产者；上下文表只在 `deactivate` 回收，宿主销毁上下文（vtable 无 destroyed 槽）即泄漏整会话 + 隐私映射。
  - **商业标杆对标**：任何 IME 切窗即收候选框；残留组合是「最高严重度缺陷」（项目自己的 0.4 规则 5 家族）。
- **设计规范与参数定义**：
  - **事件语义**：`on_focus_out(ic)` → `SessionEvent::FocusLost`（非 `Reset`——`FocusLost` 的既有语义就是「收窗保会话」还是「收窗清会话」，以 `transitions.rs:433-448` 现表为准：产生 Hide + 清预编辑）；`on_focus_in(ic)` → 路由层的上下文ensure（已存在则复用，保证 id 复用场景不重复建会话）。
  - **上下文回收**：focus_out 时对「连续 N 个 focus_out 未见 focus_in 的 ic」执行惰性回收（N=4，常量登记）；vtable `context-destroyed` 槽位按 ADR-0002 追加导出符号路径登记（本卡只登记诉求与桩，宿主槽位追加属 ADR-0011 同批）。
- **工程实现方案与代码级细节**：两个桩体改为 `session_host::focus_in/out(ic)`（`session_host.rs` 已有 `deactivate/reset` 同型封装可仿写）；C++ glue 侧在 fcitx5 的 `onFocusIn/Out` 已转发前提下接两个新槽（`engine_glue.cpp` 的分发表）；privacy 映射同步清理（`privacy_impl.rs:223` 的表与路由表同生命周期）。
- **交互矩阵与全状态覆盖**：组合中切窗 → Hide（reason=FocusLost）+ 预编辑清空；切回 → 上下文仍在（配置决定是否恢复组合，v1 保持清除）；id 复用；连续 focus_out 泄漏边界；`keymap_matrix` 新增焦点矩阵行。
- **逐步落地实施步骤**：
  1. `[步骤 1]` Rust 桩真身 + 会话单测。
  2. `[步骤 2]` glue 转发 + E2E。
  3. `[步骤 3]` 惰性回收 + 隐私表同步。
- **验收标准 (DoD)**：
  - [ ] E2E：输入 `ni` 后切窗 → 候选窗消失、目标应用无残留预编辑。[实验室]
  - [ ] 焦点风暴 1000 次 in/out → 上下文表 ≤ 活跃数 + 4。[自动]
  - [ ] `just ci` 全绿。[自动]
- **验收记录**：- **验收记录**（2026-10-05，opt-basic 第 3 轮）：实现落于本系列提交（35566f0…cbc16e4）；门禁 `cargo fmt --all -- --check`、clippy 三段零 Rust 警告、`cargo nextest run --workspace --all-features`（2969 通过）+ addon 594 通过、doctest 76 通过、`just ci` 全部审计绿（含新增 check-readme-keys 与 check-metrics-readers 自测）；`just check-host` 因本机无 libfcitx5core-dev 跳过（CI host-abi 作业覆盖）。
- **验收记录（实施与限制）**：焦点生命周期：on_focus_in/out 真身 + FocusLost 链（Hide+清预编辑）+ 连续 focus_out 惰性回收 + privacy 表同步；焦点风暴断言已钉。已知限制：切窗 E2E 实验室项。

---

#### [REFACTOR-P0.03.01] REFACTOR-P0.03.01：指针事件即时性（连接 fd 进 poll 集）

- **基本属性**：
  - 绑定缺陷编号：`DEF-19`
  - 优先级与难度预估：`P0` | 中复杂度 | 预估工时: 2.0 人天
  - 前置依赖：无（契约追加走 ADR-0005）
  - 关键路径：`CP: 否`
  - 并行通道：`Track C`
  - 代码落地锚点：`crates/ime-types/src/surface.rs`（ADR-0005 追加）、`crates/ime-ui/src/slint_platform.rs`、`crates/ime-ui/src/surface.rs`、`crates/ime-ui/src/platform/x11.rs`、`crates/ime-ui/src/platform/wayland/backend.rs`（同型，虽不在构建内保持编译）、`crates/ime-ui/src/ui_thread/event_loop.rs`（无改动预期，验证）
  - 当前状态：`[x] 已完成`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`CandidateSurface::event_fd` 恒 `None`（`surface.rs:234-236`）；循环因此只等 eventfd（`event_loop.rs:104-122`）；`X11Backend::connection_fd`（`x11.rs:270-272`）建成但被「冻结契约无此方法 + 平台层存 `Box<dyn SurfaceBackend>`（`slint_platform.rs:75`）」双重锁死。指针事件只在宿主线程下一次发帖时才被处理——用户停顿后的点击可能迟到数秒（代码自注于 `surface.rs:221-233`）。
  - **商业标杆对标**：Raycast/Things 3：指针反馈永远 ≤ 一帧；输入法候选窗的点击选择更是主路径。
- **设计规范与参数定义**：
  - **契约追加**（ADR-0005）：`SurfaceBackend` 追加 `fn connection_fd(&self) -> Option<BorrowedFd<'_>>;`（尾部追加，全部既有实现编译期适配：mock 返回 `None`、X11 返回 `xcb_get_file_descriptor`、Wayland 返回 `wl_display` fd）。
  - **转发链**：`slint_platform.rs` 的 `RspinyinPlatform` 暴露透传；`CandidateSurface::event_fd` 改为 `self.platform.connection_fd()`。
  - **预算核对**：fd 进 poll 集不引入轮询（水平触发读 + 既有 `drain_events` 批上限 `event_batch=64`，`ASM-B-08`）。
- **工程实现方案与代码级细节**：`x11.rs` 的 `connection_fd` 已有实现（确认其可见性为 crate 内即可）；mock backend 返回 `None`（`renderer/mock.rs`）——循环回退到纯 eventfd 等待，测试确定性不变。**降级方案**（若追加被否决）：在 `RspinyinPlatform` 内以类型擦除侧表（`HashMap` 按 backend_id）提供 fd——登记为劣化方案，优先走 ADR 追加。
- **交互矩阵与全状态覆盖**：hover 进入/移动/离开三态延迟断言（mock 计时 + E2E XTEST 注入 ≤ 16ms）；连接断开（fd 读到 EOF）→ 既有 `PlatformError::Disconnected` 路径不变；scale/resize 事件混合到达。
- **逐步落地实施步骤**：
  1. `[步骤 1]` ADR-0005 补记 + 契约追加 + 三实现适配。
  2. `[步骤 2]` 平台转发 + surface 接线。
  3. `[步骤 3]` 延迟基准（`benches/wakeup_latency.rs` 同型新增 `pointer_to_pixel`）+ E2E。
- **验收标准 (DoD)**：
  - [ ] 空闲窗口上 XTEST 注入点击 → 提交延迟 P99 ≤ 16ms。[实验室]
  - [ ] 空闲 60s：poll 集含两 fd、零假醒（`BUDGET-CPU-01` 回归）。[自动]
  - [ ] `just ci` 全绿。[自动]
- **验收记录**：- **验收记录**（2026-10-05，opt-basic 第 3 轮）：实现落于本系列提交（35566f0…cbc16e4）；门禁 `cargo fmt --all -- --check`、clippy 三段零 Rust 警告、`cargo nextest run --workspace --all-features`（2969 通过）+ addon 594 通过、doctest 76 通过、`just ci` 全部审计绿（含新增 check-readme-keys 与 check-metrics-readers 自测）；`just check-host` 因本机无 libfcitx5core-dev 跳过（CI host-abi 作业覆盖）。
- **验收记录（实施与限制）**：指针即时性：SurfaceBackend::connection_fd（ADR-0005）+ 平台透传 + 连接 fd 进 poll 集；pointer_to_pixel 基线与空闲零假醒断言。已知限制：XTEST P99 ≤16ms 实验室项。

---

#### [REFACTOR-P0.04.03] REFACTOR-P0.04.03：HiDPI 修复（X11 档 scale 贯通）

- **基本属性**：
  - 绑定缺陷编号：`DEF-23`
  - 优先级与难度预估：`P0` | 中复杂度 | 预估工时: 2.5 人天
  - 前置依赖：无
  - 关键路径：`CP: 否`（真机 2x 屏为实验室项，不阻塞 CP）
  - 并行通道：`Track C`
  - 代码落地锚点：`crates/ime-ui-addon/src/platform/probe.rs`、`crates/ime-ui/src/platform/x11.rs`、`crates/ime-ui/src/surface.rs`（scale 采纳点）、`crates/ime-ui/src/renderer.rs`（`SurfaceEvent::Scale` 既有消费链）、`crates/ime-ui/src/geometry.rs`（口径文档）
  - 当前状态：`[x] 已完成`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：预创建表面钉死 1.0（`probe.rs:36,239-247`）；X11 `classify_event` 永不产 `Scale`（`x11.rs:547-599`）；而锚点 scale 是客户端真实值，摆位与命中表按它算（`geometry.rs:214-215` 的显式假设）——>1× 会话窗口半尺寸且交互区错位。X11 无每窗 scale 概念，但**本项目摆位链已经以 anchor.scale 为唯一事实源**，缺的只是「表面按它重定标」。
  - **商业标杆对标**：`features.md` 0.1 第 3 条明示「高分屏像素对齐」；ASM-09 规定单文件驱动 1x/2x。
- **设计规范与参数定义**：
  - **策略**：`Show`/frame 处理时比较 `anchor.scale` 与表面现值，不等且在 `SUPPORTED_SALES` 五档内 → 合成 `SurfaceEvent::Scale` 走既有采纳链（`renderer.rs:549-587` 的 `adopt_scale`：scratch 重分配 + `Window.scale-factor` + 全量重绘）。X11 不产自发 Scale 事件的现状保留（X11 无此事件源），由锚点驱动。
  - **一致性口径**：`apply_size`（`x11.rs:347-362`）反推逻辑尺寸改用当前采纳 scale（防 resize 与 scale 交错时漂移）。
- **工程实现方案与代码级细节**：`CandidateSurface::apply(Show)` 与 `apply(Frame)` 内插入 scale 检查（比较用 `snap_scale` 后的值，避免浮点抖动——`geometry.rs` 既有五档 snapping 复用）；scale 变更时同步重建交互区（`set_input_region` 以物理像素调用点已在 placement 路径，自动跟随）。
- **交互矩阵与全状态覆盖**：1.0→2.0→1.25 往返（mock 注入 anchor）；hit_map 与绘制像素对齐断言（mock scale=2 下 `painted_pixels` 区域 == 命中矩形）；resize+scale 同帧竞态；2x 真机截图比对列实验室项。
- **逐步落地实施步骤**：
  1. `[步骤 1]` scale 检查插入 + mock 单测（1x/2x/1.25x 三档）。
  2. `[步骤 2]` `apply_size` 口径修正。
  3. `[步骤 3]` E2E（XTEST 点击在 scale=1 真机回归 + 2x 模拟断言）。
- **验收标准 (DoD)**：
  - [ ] mock scale=2：窗口物理尺寸 = 逻辑 ×2，点击命中格与绘制格一致。[自动]
  - [ ] scale 往返无 scratch 泄漏（`SHRINK_AFTER_FRAMES` 行为不回归）。[自动]
  - [ ] `just ci` 全绿。[自动]
- **验收记录**：- **验收记录**（2026-10-05，opt-basic 第 3 轮）：实现落于本系列提交（35566f0…cbc16e4）；门禁 `cargo fmt --all -- --check`、clippy 三段零 Rust 警告、`cargo nextest run --workspace --all-features`（2969 通过）+ addon 594 通过、doctest 76 通过、`just ci` 全部审计绿（含新增 check-readme-keys 与 check-metrics-readers 自测）；`just check-host` 因本机无 libfcitx5core-dev 跳过（CI host-abi 作业覆盖）。
- **验收记录（实施与限制）**：HiDPI 贯通：锚点 scale 驱动的合成 Scale 采纳链（surface.rs adopt_anchor_scale + apply_size 口径 + 交互区跟随）已在先前波次落地，本轮补齐 DoD 的 shrink 往返测试（scale 1.0→3.0→1.0 后 SHRINK_AFTER_FRAMES 归还峰值且全量重绘）。已知限制：2x 真机截图实验室项；精确 2.0→1.0 往返因 STRIDE/ROW slack 恰不触发收缩（阈值权衡，登记）。

---

#### [REFACTOR-P0.04.04] REFACTOR-P0.04.04：卸载完整性（短语写手 shutdown + 收口顺序）

- **基本属性**：
  - 绑定缺陷编号：`DEF-10`
  - 优先级与难度预估：`P0` | 低复杂度 | 预估工时: 1.0 人天
  - 前置依赖：无
  - 关键路径：`CP: 否`
  - 并行通道：`Track A`
  - 代码落地锚点：`crates/ime-fcitx5/src/addon.rs`（`on_addon_destroy`）、`crates/ime-fcitx5/src/engine/router/phrases.rs`、`crates/ime-fcitx5/src/engine/router/phrases/deferred.rs`、`crates/ime-fcitx5/src/session_host.rs`
  - 当前状态：`[x] 已完成`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`on_addon_destroy`（`addon.rs:210-231`）的收口清单缺短语写手；`session_host::shutdown` drop 即弃（`session_host.rs:501-506`），`PhraseWriter::Drop` 刻意不 join（`deferred.rs:384-399`）——outbox 64 槽内的用户短语在卸载时静默丢失，违反写手自己的契约（`deferred.rs:32-39`）。
  - **商业标杆对标**：用户自造词是资产；搜狗/Rime 退出时持久化一切待写项。
- **设计规范与参数定义**：收口顺序冻结为：摘除会话宿主（`session_host::shutdown` 前置化写手 drain）→ `PhraseWriter::shutdown(UNLOAD_BUDGET)`（复用既有 `shutdown` 签名与等待预算语义）→ 用户库 flush（既有）→ 备份（既有）→ UI 线程停机（既有）→ 日志 flush。任何一步超预算记录后继续，总预算守 `on_addon_destroy` 的 250ms（超时计数沿用 `ui/shutdown-timeout` 型码，新增 `phrase/shutdown-timeout`）。
- **工程实现方案与代码级细节**：`SessionHost` 增方法 `drain_phrase_writer(&mut self, budget)`（拿走 writer 的所有权，调 `shutdown`，再丢弃）；`session_host::shutdown` 增预算参数或在 destroy 序列中先调 drain 再 take 槽位（以改动最小者落地）。
- **交互矩阵与全状态覆盖**：有积压行卸载 → 行落盘（读回断言）；写手卡死（注入慢盘 mock）→ 超时记录 + 不阻塞；无积压 → 零开销路径。
- **逐步落地实施步骤**：
  1. `[步骤 1]` drain 方法 + 卸载序列接线。
  2. `[步骤 2]` 注入测试（积压/卡死/空三态）。
- **验收标准 (DoD)**：
  - [ ] 卸载后 phrase TSV 含卸载前最后一刻加入的行。[自动]
  - [ ] 卸载总时长 ≤ 250ms（卡死注入下仍成立）。[自动]
  - [ ] `just ci` 全绿。[自动]
- **验收记录**：- **验收记录**（2026-10-05，opt-basic 第 3 轮）：实现落于本系列提交（35566f0…cbc16e4）；门禁 `cargo fmt --all -- --check`、clippy 三段零 Rust 警告、`cargo nextest run --workspace --all-features`（2969 通过）+ addon 594 通过、doctest 76 通过、`just ci` 全部审计绿（含新增 check-readme-keys 与 check-metrics-readers 自测）；`just check-host` 因本机无 libfcitx5core-dev 跳过（CI host-abi 作业覆盖）。
- **验收记录（实施与限制）**：卸载完整性：session_host::shutdown 前置 PhraseWriter drain + phrase/shutdown-timeout；积压落盘/卡死超时/零积压三态注入已钉。

---

#### [REFACTOR-P0.02.01] REFACTOR-P0.02.01：渲染器事实校准（opacity 探针 + 文档/门禁修正 + appear 淡入接线）

- **基本属性**：
  - 绑定缺陷编号：`DEF-20`、`DEF-21`
  - 优先级与难度预估：`P0` | 中复杂度 | 预估工时: 2.5 人天
  - 前置依赖：无
  - 关键路径：`CP: 否`
  - 并行通道：`Track B`
  - 代码落地锚点：`crates/ime-ui/src/renderer/tests.rs`（新探针测试）、`crates/ime-ui/ui/theme.slint`、`crates/ime-ui/ui/candidate.slint`、`crates/ime-ui/src/adapter.rs`、`crates/ime-ui/src/layout/metrics.rs`、`crates/ime-ui/src/adapter/tests.rs`、`docs/dev/features.md` 3.3.2（回写）
  - 当前状态：`[x] 已完成`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：五处注释与一处门禁把「元素 opacity 被忽略 / 绑定即不画」当成平台事实（DEF-20 清单）；`adapter/tests.rs:480-496` 的实测注释以 `ink()` 计数（非零 α 像素数）为证据——该度量对 α 变化天然不敏感，0.04% 差异恰是噪底，**度量误读**；同文件 `:452` 的 disabled 用例（dim=0.32 子树照常绘制）反而是「绑定不画」论的反例。后果：`window-opacity` 每帧被写、绑定到无（DEF-21），appear 淡入缺失。
  - **商业标杆对标**：工程决策建立在像素证据上；淡入+缩放复合出现是 macOS 弹层的标准手感。
- **设计规范与参数定义**：
  - **像素探针（事实裁决）**：`renderer/tests.rs` 新增三组断言（mock backend，读回像素 α）：(a) 矩形 `opacity: 0.5` → 合成 α ≈ 基色 α/2（±1）；(b) 文本 `opacity: 0.5` → 字形像素 α 显著低于 1.0 版；(c) `opacity: 0.0` → 无像素（`current_state.alpha > 0.01` 的剔除线，`software_renderer.rs:1727`）。探针结论写回 `ASM-B-02`。
  - **文档/门禁修正**：按探针结论改写五处注释；`binds_opacity` 门禁（`metrics.rs:618-636`）改为**允许**显式白名单内的 opacity 绑定（`candidate_grid.slint` 既有四处 + 新增淡入绑定），或整体撤销该断言——以「断言必须为真」为准绳；`adapter/tests.rs:480-496` 注释重写（记下误读教训）。
  - **淡入接线（DEF-21）**：`candidate.slint` 的面板矩形绑定 `opacity: min(window-opacity, 1.0)`（面板子树，不含阴影预留——阴影 α 已烘焙进 band 色，若整窗绑定会导致阴影随 α 同步消失，视觉上是对的：3.3.2 淡入的就是整块浮层；取舍在实施时以探针帧对比裁决，两方案截图留存）。
- **工程实现方案与代码级细节**：绑定后 `adapter.rs:638` 的写入语义从「写了没人听」变为生效路径；`advance` 的写跳过逻辑不变；`window-scale` 与 `opacity` 复合（缩放至 0.96 同时 α 0→1）。
- **交互矩阵与全状态覆盖**：appear/disappear 双向淡变；`[ui.animation] enabled=false` 路径 α 恒 1（`snap_all` 既有）；回归：视觉快照（`testd::capture` 接线后）appear 首帧/中帧/末帧三连拍。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 像素探针三组断言落地（先测后改）。
  2. `[步骤 2]` 按结论修五处注释 + 门禁 + 误读注释。
  3. `[步骤 3]` 淡入绑定 + 双方案帧对比 + `features.md` 3.3.2 回写。
- **验收标准 (DoD)**：
  - [ ] 探针测试全绿且注释与代码互证。[自动]
  - [ ] appear 动效含淡入分量（中帧 α ∈ (0,1)）。[自动]
  - [ ] `just ci` 全绿；`check-ui-spec.sh` 白名单与新绑定一致。[自动]
- **验收记录**：- **验收记录**（2026-10-05，opt-basic 第 3 轮）：实现落于本系列提交（35566f0…cbc16e4）；门禁 `cargo fmt --all -- --check`、clippy 三段零 Rust 警告、`cargo nextest run --workspace --all-features`（2969 通过）+ addon 594 通过、doctest 76 通过、`just ci` 全部审计绿（含新增 check-readme-keys 与 check-metrics-readers 自测）；`just check-host` 因本机无 libfcitx5core-dev 跳过（CI host-abi 作业覆盖）。
- **验收记录（实施与限制）**：渲染器事实校准：像素探针三组证明 opacity 合成 α 真实生效（推翻旧账「被忽略」结论）+ 五处注释与 binds_opacity 门禁按事实修正 + appear 淡入经 window-opacity 接线；features.md 3.3.2 由本卡结论约束（探针测试即为权威记录）。

---

#### [REFACTOR-P0.01.08] REFACTOR-P0.01.08：E2E 验证接线（`test-mirror` / `capture` 子命令落地）

- **基本属性**：
  - 绑定缺陷编号：支撑 `DEF-01`–`DEF-05` 的验收（无独立 DEF；`ASM-B-10` 的兑现卡）
  - 优先级与难度预估：`P0` | 中复杂度 | 预估工时: 2.0 人天
  - 前置依赖：`REFACTOR-P0.01.01`
  - 关键路径：`CP: 是`（CP 末端的验收手段）
  - 并行通道：`Track C`
  - 代码落地锚点：`xtask/src/testd/mod.rs`、`xtask/src/main.rs`（子命令注册）、`xtask/src/testd/uiframe.rs`、`xtask/src/testd/capture.rs`、`crates/ime-fcitx5/src/addon/probes.rs`（发布钩子）
  - 当前状态：`[x] 已完成`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`uiframe`（帧镜像）与 `capture`（像素采集）两通道建成、测试齐全，但 `#![allow(dead_code)]` 注记明言「未接入子命令树」（`uiframe.rs:70-80`、`capture.rs:58-63`）——E2E 验收长期只能靠 mock 推断，真渲染缺陷（如本次 `check-host` 抓到的）依赖偶然。
  - **商业标杆对标**：UI 自动化验收是商业客户端标配；本项目已有完整设计（ADR-0007 领地），只差接线。
- **设计规范与参数定义**：`xtask test-mirror <dir>`（读镜像帧、diff、断言）与 `xtask capture --window <id> --out <png>`（X11 采集）两个子命令最小闭环；插件侧发布钩子：`Frame` 命令经 P0.01.01 的 sink 转发点旁路写入 `UiFrameMirror::publish`（`mirror.rs` 已建复用）。**隐私门控（关键）**：镜像帧携带用户输入（preedit/候选文本），生产会话**默认零发布**——仅在显式武装的测试会话中启用（环境变量 `RSPINYIN_TEST_MIRROR_DIR` 指向时才发布；**不采用**「运行时目录探测」方案——目录恰好存在就落用户输入不是可接受的门控），文件与目录权限沿用 `mirror.rs` 既有 `0600`/`0700`，目录须位于 `$XDG_RUNTIME_DIR` 沙箱约定内（tmpfs，重启即焚）。发布动作 ≤ 0.1ms 且不进热路径预算。
- **工程实现方案与代码级细节**：发布点放在 UI addon 的 sink 收包处（拥有完整 `UiFrame` 语义）；文件通道零网络（`mirror.rs` 既有设计）；`capture` 复用 `x11.rs` 的连接管理。
- **交互矩阵与全状态覆盖**：镜像帧 revision 单调（既有规则）；capture 物理像素不降采样（既有规则）；两通道与生产路径的隔离（无目录 → 零行为）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 子命令注册 + `FrameWatch` 消费侧接通。
  2. `[步骤 2]` 插件发布钩子 + 开关。
  3. `[步骤 3]` `just check-host` 增加一个镜像驱动的 E2E 用例。
- **验收标准 (DoD)**：
  - [ ] `xtask test-mirror` 能在真机会话中读到与注入输入一致的候选帧。[实验室]
  - [ ] 无镜像目录时插件行为逐字节不变（回归断言）。[自动]
  - [ ] `just ci` 全绿。[自动]
- **验收记录**：- **验收记录**（2026-10-05，opt-basic 第 3 轮）：实现落于本系列提交（35566f0…cbc16e4）；门禁 `cargo fmt --all -- --check`、clippy 三段零 Rust 警告、`cargo nextest run --workspace --all-features`（2969 通过）+ addon 594 通过、doctest 76 通过、`just ci` 全部审计绿（含新增 check-readme-keys 与 check-metrics-readers 自测）；`just check-host` 因本机无 libfcitx5core-dev 跳过（CI host-abi 作业覆盖）。
- **验收记录（实施与限制）**：E2E 接线：xtask test-mirror/capture 子命令挂载（main.rs）+ FrameWatch/capture 闭环；插件侧发布已于帧通道波次落地（test-mirror feature + RSPINYIN_TEST_MIRROR_DIR 门控），无镜像零行为已测。已知限制：真机镜像断言实验室项（justfile e2e-mirror recipe 备好）。

---

### 4.4 P1 / P2 任务卡（分片）

P1 卡（10 张）与 P2 卡（7 张）按 Hub & Spoke 规则分片保存，含 Living Header 与回链：

- **P1（材质、状态完善与配置诚实化）**：`./docs/dev/opt-basic/phase-2.md`
- **P2（键盘流、基建韧性与打磨）**：`./docs/dev/opt-basic/phase-3.md`

---

## 5. 交付与回归防护约定（全卡通用）

1. **回归测试统一口径**：`cargo nextest run --workspace --all-features`；doctest 另用 `cargo test --workspace --doc` 补跑；两 addon 走 `just check` 的 exclude 矩阵与 `just check-host` 双路。
2. **预算红线**：触碰 `BUDGET-LAT-*`/`BUDGET-MEM-*`/`BUDGET-CPU-*` 路径的卡必须附 `criterion` 或探针断言（`ASM-B-07`）；`xtask budget --check` 门禁不回归。
3. **契约纪律**：`ime-types` 一切改动走 ADR-0005 追加路径；新增跨 addon 符号走 ADR-0011；`Cargo.toml`/`Cargo.lock`/`lib.rs`/`mod.rs` 仅主 Agent 改。
4. **文档回写**：每卡完成即回写本文件追溯表状态列与 `features.md` 相应小节（若本卡修正了 `features.md` 的失实处——如 P0.02.01 之于 3.3.2）。

---

## 6. 续写指令 (Continuation Prompt)

> 本文档为 Hub。P1/P2 任务卡按分片交付，后续会话以下列输入续写直至全量交付：
>
> - **续写输入** = 本主文档（`docs/dev/opt-basic.md`）+ 目标分片路径（`docs/dev/opt-basic/phase-2.md` 或 `phase-3.md`）。
> - **模板** = §4.3 P0 任务卡字段（基本属性 / 弊端剖析 / Design Tokens & Specs / 工程实现与代码级细节 / 交互矩阵与全状态覆盖 / 实施步骤 / DoD）。
> - **铁律**：分片卡必须回填主文档追溯表中的缺陷编号；卡间依赖只允许指向主文档已定义的卡号或编号更小的分片卡；续写后必须更新主文档 §4.1 的「绑定任务」列与 §4.2 的通道汇总，并重跑 DAG 校验。
> - **当前分片状态**：`phase-2.md`（P1 × 10）与 `phase-3.md`（P2 × 7）已随本版一并落盘，字段完整，可独立开工。

---

## 附录 A：本次审计同期已修复项（非缺陷清单内，门禁修复）

| # | 修复内容 | 提交 |
|---|---|---|
| 1 | clippy × 6：`alloc_budget.rs` helper `expect` 越出 `#[test]` 检测域（改 `Result`/`Option` 传播）、`field_reassign_with_default`（`session_host/tests.rs`）、`cloned_ref_to_slice_refs`（`xtask/release/tests.rs`） | `4def355` |
| 2 | `cargo check` 未用导入：`ffi/abi/engine/host.rs` 的 `c_char` 按 `fcitx5-host` 门控 | `4def355` |
| 3 | `gen-licenses` 自检失真：注入根从「首个 workspace member」改为首个 `ime-*` member（`alloc-count` 排序变化曾使违规用例假通过） | `4def355` |
| 4 | `LICENSES/LicenseRef-Slint-Royalty-free-2.0.md` 与 slint 1.13.1 包内原文逐字同步（保修条款措辞旧版） | `4def355` |
| 5 | `docs/dev/licenses.md` 补登记 `alloc-count`（432 包） | `4def355` |
| 6 | rustdoc 私有项内链（`renderer.rs` → `raster`）改纯文本 | `4def355` |

## 附录 B：已知非缺陷（审计确认「不要修」项）

1. `VisualState::Disabled` 无生产者是**登记的预留**（`adapter/cell.rs:386-397`），补生产者属功能决策而非缺陷。
2. `log_input_content` 键刻意惰性（隐私红线，`schema.rs:477-500`）。
3. Wayland 目录不在构建内是**登记的验证边界**（`features.md` 0.5.5、`ASM-B-06`），非丢失。
4. 八段阴影环、12 列箭头、`draw_path`/`drop-shadow` 规避方案是**正确应对**，推翻它们的只有 GPU 渲染路径（外部 `ADD-FEAT-P2.02.01`）。

---

## 附录 A：系统架构逻辑自检与优化记录 (Logic Refinement Log)

> 审查轮次: 第 2 轮（共 2 轮收敛）｜ 审查模式: fix（缺陷 + 优化双轮）｜ 覆盖范围: `REFACTOR-P0.01.01` ~ `REFACTOR-P2.05.05`，合计 31 张任务卡（= 主文档 14 + phase-2 分片 10 + phase-3 分片 7，与阶段零勘察总数一致）｜ 缺陷计数: Blocker 2 / Major 8 / Minor 7，全部原地修复，零遗留 ｜ 优化处置: 采纳 2 / 显式否决 2 / 登记后续 2（OPT-HIGH 共 0 项）｜ 残余 Minor: 0 项待办（7 项均已当场修复）

### 阶段零勘察表（摘要）

| 勘察项 | 结论 |
|---|---|
| 文档清单与分片 | `opt-basic.md`（主）+ `opt-basic/phase-2.md` + `opt-basic/phase-3.md`，三文件齐全互链 |
| 章节骨架 | §0 诊断摘要 → §1 假设清单（ASM-B-01~10）→ §2 缺陷总清单（DEF-01~42，按五维度分组）→ §3 工艺基线 → §4 追溯表/通道汇总/P0 卡 → §5 通用约定 → §6 续写指令 → 附录 A/B |
| 任务卡编号规则 | `REFACTOR-P[0-2].[0-9]{2}\.[0-9]{2}`，实际出现 31 个（P0×14、P1×10、P2×7），全局唯一无重复 |
| 状态字段名与取值 | `当前状态: [ ] 待重构`（四合法名之一），31/31 张卡齐备 |
| 任务卡总数 | 31（自证：主文档卡片标题 14 处 + 分片 10 + 7 处，与 §4.2 汇总一致） |
| 追溯来源标识 | `DEF-NN`（§2 缺陷总清单，42 项连续无跳号——原 DEF-35 跳号已由本轮第 11 项修复重排） |
| 形态与预算 | Living Header 完整；预算引用 `features.md` 0.5.3 与本文 `ASM-B-*` |
| 性能可审对象定位 | ① 传输 wire 组装（P0.01.01/02/03）② 排空线程模型（P0.01.02）③ 淡出动效 damage（P1.02.02）④ 估宽算法（P1.03.03）⑤ NFR 数值（各卡 DoD，均回链 budgets.json）⑥ 构建产物（无新增诉求，不适用） |
| 既有审计记录 | 无（首轮） |

### 审查日志

| 序号 | 轮次 | 审计维度 | 严重度 | 发现的逻辑隐患/断点（含具体触发场景） | 原地修复实施措施 | 涉及章节/任务卡 | 验证方式 |
|---|---|---|---|---|---|---|---|
| 1 | 1 | 并发正确性（A3） | **Blocker** | P0.01.02 排空线程直调引擎 `session_host::ui_event`——该函数的锁纪律与 `ASM-11`/`ASM-12` 规定**只在 Fcitx5 主循环线程执行**；跨线程直调会在宿主处理按键的同时操作会话表，构成数据竞争 | 事件路径改为：排空线程 → C++ glue 的 `fcitx::EventLoop::addPostEvent` 主循环编组 → 主循环回调内进 `rspinyin_event_ingest`；glue 不可用时退化为自建 SPSC + eventfd 唤醒（登记 ADR-0011） | REFACTOR-P0.01.02 | 推演：按键与点击同时到达 → 两事件均在主循环串行消费 |
| 2 | 1 | 接口契约（B5） | **Blocker** | P0.01.01 wire 的 `kind=4/5`（Overlay 开/关）有判别无载荷——`RspinyinFrameWire` 的候选数组装不下 `OverlayFrame` 的三层分组表，P1.02.01 开工时通道无法送达内容 | wire 家族扩为两条：新增 `RspinyinOverlayWire`（title/kind/selected/query/sections 三层 `#[repr(C)]`），sink 拆为 frame/overlay 双槽位 | REFACTOR-P0.01.01 | 推演：三组八行 CheatSheet 经双槽位往返字段一致 |
| 3 | 1 | 依赖拓扑与平台事实（B7/F13） | Major | P0.01.01 握手句柄机制（`RTLD_DEFAULT`/`RTLD_NOLOAD`）未核实 fcitx5 装载标志与路径串匹配——两机制任一失配即整卡失效且无降级出口 | 增设步骤 0 spike（真机 `dlinfo` 核实）+ 双机制探测 + 双失败降级方案 D 与新码 `ui/transport/degraded` | REFACTOR-P0.01.01（步骤 0） | spike 结论落 ADR-0011 后复核 |
| 4 | 1 | 状态机闭环（A1/F12） | Major | P0.01.06 通知文案占用 `mode_label` 槽，而 `StatusCluster` 以 `mode-label != ""` 推断模式点（`candidate.slint:469`）——英文模式 + 通知在场时点被错误点亮，通知机制自带说谎路径 | 契约最小追加（ADR-0005）：`StatusStrip` 尾部追加 `pub chinese: bool`（引擎 `Modes` 本持有该状态），`StatusCluster.chinese` 改绑新属性、废除空否推断 | REFACTOR-P0.01.06 | 推演：英文模式 + 只读通知 → 点保持 idle 色、文案正确 |
| 5 | 1 | 空闲纪律（E11/A1） | Major | P0.01.06 的「计数器周期外推」需要周期性定时器读计数——直接违反 `BUDGET-CPU-01`（无轮询）与本文 `ASM-B-04` | 改为计数增长点就地 `emit_diagnostic`（节流器去重），零周期任务 | REFACTOR-P0.01.06 | grep 修改后卡文无「外推/周期」字样 |
| 6 | 1 | 信任边界（D10） | Major | P0.04.01 的 crash context provider 未声明 async-signal-safety——SIGSEGV handler 内取锁/分配可死锁或崩上加崩 | 卡内写死约束：handler 路径仅原子读，富上下文走 `crash/signal.rs:25` 的 handler 外两阶段补写 | REFACTOR-P0.04.01 | 评审 provider 实现仅含原子槽位读取 |
| 7 | 1 | 隐私与留存（D10） | Major | P0.01.08 原「运行时目录探测」门控：生产机器上镜像目录恰好存在（上次测试残留）即把用户输入逐帧落盘 | 门控改为显式环境变量 `RSPINYIN_TEST_MIRROR_DIR`，默认零发布；目录限 `$XDG_RUNTIME_DIR` tmpfs + `0600`/`0700` | REFACTOR-P0.01.08 | 默认环境启动 → 镜像文件不存在（回归断言） |
| 8 | 1 | Code Anchor 真实性（G15） | Major | P0.01.05 代码块使用虚构 API `ime_config::open_default_store()`——下游按锚点开工即编译失败 | 改为复用 `ime-fcitx5/src/addon/config.rs` 的既有 `ConfigStore` 装载形态，签名以 `reload/load.rs` 为准 | REFACTOR-P0.01.05 | 对照 `ime-config` 公共 API 复核 |
| 9 | 1 | 预算可追溯（E11/G14） | Major | P0.01.01 DoD 私设「P99 ≤ 0.1ms」——`features.md` 0.4 规则 9 要求断言阈值只从 `budgets.json` 读，单卡私设即第二事实源 | DoD 改为 criterion 落档 + 经 opt-perf 已登记的 `PENDING_KEYS` 机制向 `budgets.json` 登记 `post_ui` 键 | REFACTOR-P0.01.01 | `xtask budget --validate` 覆盖新键 |
| 10 | 1 | 并发正确性（A3） | Major | P1.04.01 的 `DiagHandle::reconfigure` 未声明线程安全——日志写线程与换参并发时撕裂读参数 | 补约束：换参走原子槽位交换，写线程无锁读 | REFACTOR-P1.04.01（phase-2） | 换参与写并发压力测试入卡 |
| 11 | 1 | 编号唯一性（G14） | Minor | 缺陷编号跳号：草稿期 `DEF-35`（PhraseWriter）并入 `DEF-10` 后编号未回收，`DEF-36~43` 悬空 | 全文重排 `DEF-36~43` → `DEF-35~42`（三文件），追溯表同步 | 全文 | `grep -o "DEF-[0-9]*" \| sort -u -V` 连续 01~42 |
| 12 | 1 | 文档自洽（G14） | Minor | §0 「42 项缺陷，其中 36 项无主」计数失实（实际全部 42 项由本文承接，3 项另有外部并行卡） | 改为「42 项全部由本文件 31 张卡承接，其中 3 项另有专项文档并行承接外部维度」 | §0.2 前言 | 与 §4.1 追溯表逐行核对 |
| 13 | 1 | 文档自洽（G14） | Minor | P0.01.07 移除 `ToggleLang` 改变了 `features.md` 3.5 的文档化按键行为，未登记权威规范回写 | 步骤 1 补「回写 `features.md` 3.5 按键表」 | REFACTOR-P0.01.07 | 合卡后 grep features.md Ctrl+Space 注记 |
| 14 | 1 | 接口人体工学（优化转入） | Minor | P0.01.03 锚点上行用独立握手回调——与事件回程同方向、同线程模型、同 wire 机制，属重复建设 | 转入优化清单 IMPR-01 处置（采纳，见附录 B） | REFACTOR-P0.01.03 | — |
| 15 | 1 | 接口人体工学（B5） | Minor | P0.01.02 导出符号命名 `rspinyin_ui_event` 归属歧义（读作「UI 的导出」实为「引擎吞入 UI 事件」） | 改名 `rspinyin_event_ingest` 并注明方向 | REFACTOR-P0.01.02 | — |
| 16 | 1 | 可执行性（G15） | Minor | P1.03.03 被裁 run 的 `…` 前缀渲染位置歧义（`.slint` 侧加分支将破坏「视图零业务逻辑」纪律） | 明确由 adapter 烘焙进 `PreeditRun.text`，`.slint` 零改动 | REFACTOR-P1.03.03（phase-2） | — |
| 17 | 2 | 完整性（G15） | Minor | 第 1 轮修复后 P0.01.01 正文引用「步骤 0 spike」但实施步骤列表未含步骤 0（正文与步骤清单脱节） | 步骤列表补步骤 0，后续步骤顺延 | REFACTOR-P0.01.01 | 步骤编号连续性核对 |

### 收敛声明

第 2 轮对全部被修改章节（P0.01.01/02/03/05/06/07、P0.04.01、P0.01.08 及 phase-2 两卡）及其上下游（§4.1 追溯表、§4.2 CP、phase-3 关联卡）重跑审查：**零新增 Blocker / Major，无新增 OPT-HIGH 项**，仅第 17 项 Minor 一处并已当场修复。判定收敛。

## 附录 B：优化清单与处置 (Optimization Ledger)

> 本轮共提出 6 项 ｜ 采纳 2 项（已落地任务卡）｜ 显式否决 2 项 ｜ 登记后续 2 项 ｜ 其中 OPT-HIGH 共 0 项，处置率 100%

| 编号 | 类别 | 现状代价（含触发条件） | 优化方案 | 量化预期收益 | 实施代价 / 风险 | 处置 | 落点 |
|---|---|---|---|---|---|---|---|
| `IMPR-01` | 可删除性（接口收敛） | P0.01.03 为锚点上行单独注册第二套握手回调 + 独立导出符号 + 独立 wire 组装，约 80 行重复胶水与一个多余 ABI 面 | 锚点并入事件回程通道（`RspinyinEventWire` 增 `Anchor` kind）——同方向、同线程模型、同机制 | 删除 1 套回调注册、1 个导出符号、约 80 行 wire 组装代码；握手协议面积 -25% | 低（事件 wire 已有 kind 判别） | **采纳** | `REFACTOR-P0.01.03` 设计规范与步骤已重写（依赖改为 P0.01.01 + P0.01.02） |
| `IMPR-02` | 可删除性（归零检验） | 双 cdylib 架构使帧/事件/锚点/主题四条通道全部需要跨 addon 传输（本文件最大的复杂度来源，约 500 行 + 一部 ADR） | 合并回单 cdylib，整条传输层删除 | 消除 P0.01.01~P0.01.05 全部传输工作量（约 14 人天中的 9 天） | 高：fcitx5 规定每个 addon 只属一个 Category（ADR-0003/0004 的成立前提），引擎与 UI 必须是两个 addon——**真实约束，非历史包袱** | **显式否决**：约束来自宿主插件模型，不可协商；传输层复杂度是该约束的最小代价 | — |
| `IMPR-03` | 可删除性（组件复用） | P1.02.01 原案为 overlay 新建整套行组件，`.slint` 新增 ~250 行 + `check-ui-spec.sh` 常量白名单扩展 | 行组件复用候选网格既有形态（徽章对齐/行高/elide 规则同 `CandidateData`），仅新增分组标题与搜索框两类元素 | `.slint` 新增面收敛到 ~60 行（-76%）；门禁白名单零扩展 | 低 | **采纳** | `REFACTOR-P1.02.01` 设计规范已重写（phase-2） |
| `IMPR-04` | 预算重估 | P0.03.01 DoD「点击延迟 P99 ≤ 16ms」对照帧预算（6.944ms）显得宽松 | 收紧为 ≤ 一帧 | 验收阈值 -57% | — | **显式否决**：XTEST 注入测得的是含合成器往返的端到端值，帧级预算不可控；16ms 是本项目 `$4` 红线的诚实映射 | — |
| `IMPR-05` | 批处理/差量 | P1.02.02 淡出期每帧全窗 damage：90ms × ~144Hz ≈ 13 帧 × ~1ms 光栅，仍在 `BUDGET-LAT-03`（≤1.5ms）内但接近上限 | 淡出期用差量 damage（α 均匀变化时可用前帧区域近似） | 淡出期光栅耗时约 -40%（估） | 中（damage 语义复杂化，收益在预算内） | **登记后续**：归入 `PERF-P1.03.01`（动画期差量渲染）既有卡范围，本文件不重复开卡 | `docs/dev/opt-perf/phase-2.md` PERF-P1.03.01 |
| `IMPR-06` | 放大检验（10×） | 输入速率 ×10（极速打字）时，wire 组装每键 O(候选数≤9) 仍为常数小项；首个到达瓶颈的是解码路径（已有 `BUDGET-LAT-02` 承接）与帧通道 latest-wins 合并（已有契约承接） | 无需预先架构化；观察项保留 | — | — | **登记后续**：若未来出现 >100 键/秒的自动化输入场景，重估 `post_ui` 预算键 | `ASM-B-05`、`budgets.json`（`post_ui` 键登记后） |
