# ADR-0011：跨 addon 帧通道（wire 镜像 + 进程内符号握手）

> 状态：**已接受（Accepted）** ｜ 决策日期：**2026-10-01** ｜ 决策人：主 Agent ｜
> 关联文档：`docs/dev/opt-basic.md` 的 `REFACTOR-P0.01.01`（本 ADR 是该卡步骤 1 的交付物）、`REFACTOR-P0.01.02`/`P0.01.03`（复用同一握手与 ingest 通道）｜
> 关联 ADR：[0003-ui-role-separate-addon.md](0003-ui-role-separate-addon.md)（双 addon 架构）、[0004-ui-addon-crate-split.md](0004-ui-addon-crate-split.md)（两库独立 `dlopen`、不共享静态）、[0002-rust-exports-addon-factory.md](0002-rust-exports-addon-factory.md)（"只增不改"的符号演进路径）、[0005-incremental-contract-extension.md](0005-incremental-contract-extension.md)（`ime-types` 追加路径，本 ADR 不触碰冻结契约）｜
> 实测证据：wire 往返单测（两库各自字段保真）；真机 X11 会话 E2E 见 `REFACTOR-P0.01.08` 的验收记录（本机唯一可验证档，`features.md` 0.5.5）

---

## 背景

ADR-0003/0004 把插件拆成两个独立 `dlopen` 的 cdylib：引擎（`librspinyin.so`）与候选窗（`librspinyin-ui.so`）。两库不共享任何静态变量、不互相链接（`ASM-B-01`），fcitx5 的装载方式使 `dlsym(RTLD_DEFAULT)` 不可依赖（`ASM-B-09`）。结果是：引擎每键都在建帧并通过 `HostCtx::post` 发出，而生产端实现（`ffi/abi/engine/host.rs` 的 `post()`）把命令**全部丢弃**并记 `ui/not-ready`——渲染管线的其余各环（契约、通道、渲染、损害合并）均已建成且测试充分，唯独这根线没接。

fcitx5 的 `InputPanel` 载体只承载 preedit 与宿主候选列表：引擎今日只写 preedit（`engine_glue.cpp:402`），写不进 span 种类、注音、来源、页态等 `UiFrame` 语义——它不是帧通道，只是降级载体。

## 决策

### 决策 1：方案 C——进程内符号握手 + `#[repr(C)]` wire 镜像

引擎直接调用 UI 侧提供的 sink 函数指针，把每个 `UiCommand` 以借用 wire 形态跨库传递。不引入新队列、不新增 IPC：通道语义（latest-wins / ordered、2.2.1/2.2.2 的溢出表）的唯一事实源仍是 `ime-types` 契约与 `UiCommandSender`，跨 addon 传输是**一次函数调用**，不是一条新通道。

**握手方向（对卡文的落地裁决）：双侧发起、后到者连接。** fcitx5 对两个 addon 的装载与初始化顺序没有契约保证（输入法引擎尤其可能懒加载），单侧发起的握手在错误顺序下会永久错过。因此两侧都在自己的初始化序列里各探测一次，**后初始化的一方必然能找到先初始化的一方**，连接由它完成：

1. **UI 侧**（`ui-registration` 步骤，晚于自身 `ui-startup`）：C++ glue（`ui_addon_glue.cpp` 的 `rspinyin_ui_transport_register`）以三机制探测寻找引擎库——`dlsym(RTLD_DEFAULT)`、`dladdr` 自身装载路径推出的同目录 `librspinyin.so` + `RTLD_NOLOAD`、裸 soname + `RTLD_NOLOAD`——找到即调用 `rspinyin_engine_register_ui_sinks`（传入 Rust 导出 `rspinyin_ui_frame_sink()` 的指针）。
2. **引擎侧**（init 序列末尾的 `transport-probe` 步骤）：镜像的三机制探测（`addon_glue.cpp` 的 `rspinyin_engine_transport_probe`）寻找 `librspinyin_ui.so`，找到即调用 `rspinyin_ui_transport_register`——注册动作仍由 UI 侧的同一符号执行，两侧只是"按门铃"。

任一侧探测落空（自己是先初始化者，或对端未装载）只记一行诊断、**不重试、不轮询**；两库的初始化都在 fcitx5 主线程串行执行，后到者的探测不会与先到者的注册竞争。卡文的 spike（步骤 0）由此在实现上消解：三机制探测覆盖装载标志与路径名两种不确定性，真机 E2E 以 `ui/transport/registered: probe=*` 诊断码落点记录实际生效的机制。

**注销**：UI addon 的 `on_addon_destroy` 调用 glue 的 `rspinyin_ui_transport_unregister()` → 引擎的 `rspinyin_engine_clear_ui_sinks()`，sink 指针置回空，`post()` 回到 `ui/not-ready` 语义。addon 卸载/重装的关闭-重启往返有单测。

### 决策 2：wire 家族（两条，`#[repr(C)]`，追加式演进；字段次序即 ABI）

Wire 镜像在**两库各自转写一份**（`ime-fcitx5/src/ffi/abi/engine/transport.rs` 与 `ime-ui-addon/src/ffi/transport.rs`），与三个 `.cpp` 各自转写 `#[repr(C)]` 结构体的既有先例一致（"deliberately no shared header"）：两库独立 `dlopen`，经共享 crate 产生链接期耦合会破坏这份独立性。本 ADR 的表即冻结的形状权威；**字段只增不改、不改序**，新增字段两库同步且各带版本断言。

借用期规则：wire 中所有指针只在**一次 sink 调用内**有效；sink 实现必须在返回前把需要的字节复制为自有数据（"借用指针直拷"——不外借、不缓存、不逃逸）。引擎侧组装复用线程本地 scratch（`Vec` 清空重填），每键零新分配路径。

| wire 结构 | 字段（按 ABI 序） | 承载 |
|---|---|---|
| `RspinyinStr` | `ptr: *const u8, len: u32`（UTF-8，借用） | 一切文本 |
| `RspinyinSpanWire` | `start: u16, end: u16, kind: u32` | preedit 片段（SpanKind 判别值） |
| `RspinyinCandidateWire` | `index: u16, text: RspinyinStr, annotation: RspinyinStr（len=0 即 None）, source: u32, score: f32, consumed_syllables: u16` | 候选 |
| `RspinyinRectWire` | `x: i32, y: i32, w: u32, h: u32` | 锚点矩形 |
| `RspinyinFrameWire` | `kind: u32`（0 Frame / 1 Show / 2 Hide / 3 Theme / 6 Shutdown）＋ `revision: u32` ＋ `preedit: RspinyinStr` ＋ `caret: u32` ＋ `spans: *const RspinyinSpanWire, span_count: u32` ＋ `candidates: *const RspinyinCandidateWire, candidate_count: u32` ＋ `page_current/page_total/page_size: u8` ＋ `mode_label: RspinyinStr` ＋ `flags: u32`（bit0 full_width、bit1 punctuation_full、bit2 readonly、bit3 has_user_dict_hit）＋ `script: u32` ＋ `cursor: RspinyinRectWire, screen: i32, scale: f32, placement: u32` ＋ `max_per_row: u8, show_annotation: u8, max_width_dp: u16` ＋ **Theme 载荷（kind=3 时有效）**：`theme_accent: u32`（0xRRGGBBAA）`theme_scheme: u32` `theme_acrylic: u8` `theme_base_alpha: u8` `theme_corner_radius_dp: u16` `theme_scale: f32` ＋ `hide_reason: u32` | Frame/Show/Hide/Theme/Shutdown 五类命令 |
| `RspinyinOverlayEntryWire` | `keys: RspinyinStr, label: RspinyinStr` | 浮层行 |
| `RspinyinOverlaySectionWire` | `title: RspinyinStr, entries: *const RspinyinOverlayEntryWire, entry_count: u32` | 浮层分组 |
| `RspinyinOverlayWire` | `kind: u32`（4 Overlay(open) / 5 Overlay(closed)）＋ `panel_kind: u32` ＋ `title: RspinyinStr` ＋ `selected: i32`（-1 即 None）＋ `query: RspinyinStr` ＋ `sections: *const RspinyinOverlaySectionWire, section_count: u32` | Overlay(open/closed) 两类 |

对卡文草图的两处**补全**（登记为决策而非偏离）：① Theme 载荷补齐 `ThemeSpec` 全部六字段——草图只有 accent/scheme，会让 `DEF-33` 的外观四键静默丢失；② `Shutdown` 以 kind 6 补全 `UiCommand` 全集，避免"通道里唯一的不可序列化命令是停机"。

### 决策 3：sink 结构与引擎侧注册符号

```c
/* 由 librspinyin-ui.so 的 Rust 导出 rspinyin_ui_frame_sink() 提供，
   进程生命周期内地址稳定。 */
typedef struct {
    void *ctx;                                   /* UI 侧上下文令牌 */
    void (*frame)(void *ctx, const RspinyinFrameWire *wire);
    void (*overlay)(void *ctx, const RspinyinOverlayWire *wire);
} RspinyinUiSink;

/* 引擎导出；UI 的 glue 在双机制探测成功后调用一次。
   返回 1 表示槽位已登记。 */
int rspinyin_engine_register_ui_sinks(const RspinyinUiSink *ui);
void rspinyin_engine_clear_ui_sinks(void);
```

- 引擎侧存储为进程级原子槽位；**未注册窗口期 `post()` 保持既有 `ui/not-ready` 语义**（命令照旧丢弃、诊断照旧节流）——注册成功后该码零新增，正是 P0.01.01 的 DoD 2。
- sink 回调在**引擎主循环线程**执行；实现（UI 侧 Rust）把 wire 解析回 `UiCommand` 后送入 `UiThread::send`（`Send`、非阻塞、既有溢出语义）。**回程事件的注册不在本 ADR 的符号里**：`REFACTOR-P0.01.02` 按同一"只增不改"路径追加自己的导出符号（`rspinyin_engine_register_event_ingest`），并在同一次 `rspinyin_ui_transport_register` 调用中一并探测——一次握手流程、两个独立符号，各自可单独演进。
- 新增导出符号走 ADR-0002 的"只增不改"路径：不动既有 vtable 槽位，**不 bump** `RSPINYIN_ABI_VERSION`。
- 命名沿用卡文的 `register` 语义但方向反转（UI 发起）：引擎导出 `*_engine_register_ui_sinks`，UI 导出 `*_ui_frame_sink`。卡文里的 `rspinyin_ui_frame_sink_register` 名称不再使用，以本表为准。

### 决策 4：降级阶梯

1. **握手失败**（双探测都不中）：`ui/transport/handshake-unavailable`（新码）一行；引擎侧维持 `ui/not-ready`。不重试、不轮询。
2. **方案 D（宿主 InputPanel 有损载体，本 ADR 仅设计、不实现）**：引擎把候选列表写入宿主 `InputPanel::setCandidateList`，UI 从既有 `PanelMirror` 建帧；丢失 span 种类/注音/来源，窗口以无分隔符、无注音形态工作。仅当真机 E2E 证明握手在目标宿主不可用且不可修复时，才另开 ADR 落地方案 D。在本机可验证档（X11，fcitx5 5.1.7）上握手被 E2E 证实可用前，方案 D 不动工。

## 后果

- `ime-types` 冻结契约**零变更**：wire 是 ABI 层的镜像家族，不是契约类型（`UiCommand` 仍是唯一语义事实源，wire 是它的传输形状）。
- 两库各自新增约二百行转写代码 + 单测（字段保真往返）；`check-unsafe` 白名单不变（wire 组装是安全代码；引擎侧原子槽位是安全代码）。
- `post_ui` 预算键按 P0.01.01 DoD 4 经 opt-perf 的 `PENDING_KEYS` 机制登记进 `budgets.json`，criterion 基线落档。
- E2E（真机 X11 注入 → 候选可见 → `test-mirror` 帧一致）是 `REFACTOR-P0.01.08` 的领地；本 ADR 的真机证据项在其验收记录中落档。
