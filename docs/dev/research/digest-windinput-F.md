# 勘察摘要：WindInput（清风输入法）— 簇 SCOUT-F

> 勘察对象：`remarks/WindInput`（GPL-3.0）。静态源码精读，未编译未运行。
> 形态：一套 Rust 引擎服务进程（`wind_input`）+ 三平台薄壳 —— `wind_linux/`（C++17 Fcitx5 addon）、
> `wind_tsf/`（Windows TSF DLL）、`wind_macos/`（Swift IMKit .app）。
> 与 rspinyin 的最大架构分歧：**引擎不在插件进程内**。Fcitx5 addon 只做「按键转发 + 上屏写回 + 贴图」，
> 解码/词库/候选/光栅化全部在独立 Rust 服务进程里，经 UDS + SHM 与 addon 通信。

## 0. 总体架构（与己方对照的前提）

- addon 注册：`wind_linux/src/fcitx/WindEngine.h:50` `class WindEngine : public fcitx::InputMethodEngineV2`，
  `WindEngine.h:189` `WindEngineFactory : fcitx::AddonFactory`。头注释明示线程模型：主线程同步请求
  （UDS 往返 <1ms、2s 超时兜底），push 通道后台线程收帧、经 EventDispatcher 转回主线程才碰 InputContext。
- 两条通道（`wind_linux/include/Bridge.h`）：请求/响应 `bridge.sock`（同步、一问一答、非线程安全）+
  推送 `bridge_push.sock`（服务端单写、客户端只读、断线每秒重连）。协议头 8 字节：
  `wind_input/crates/wind-ipc/src/protocol.rs:656-706`（`IpcHeader{version:u16 含 ASYNC_FLAG, command:u16, length:u32}` LE）。
- 候选窗由**服务进程**光栅化（BGRA 预乘 alpha）→ 写 POSIX SHM `/WindInput{Dev}.<uid>` →
  push `CMD_HOST_RENDER_FRAME` → addon 主线程按名只读打开 SHM 拷帧呈现（`wind_linux/AGENTS.md`「候选窗（X11）」节）。
- `protocols/` 目录是 **Wayland 协议 XML**（input-method-unstable-v1/v2、wlr-layer-shell、viewporter、
  xdg-output、xdg-shell、relative-pointer），供 WaylandPanel 使用；X11 候选窗自建 xcb 连接、
  override-redirect 窗口（不归 WM 管、不抢焦点）。
- IPC 编解码层 `wind-ipc`（Rust SSOT）↔ `wind_tsf/include/BinaryProtocol.h`（C++ 镜像）↔
  Swift `ProtocolTypes.swift/BinaryCodec.swift` ↔ `wind_linux/include/{ExtProtocol.h,Codec.cpp}` 四语言字节级对齐。

## 1. 测试基建（维度⑤）

- **C++ 单测无需 Fcitx5**：`wind_linux/src/core` 与 `include` 刻意不 include 任何 Fcitx5 头
  （仅 `src/fcitx/WindEngine.cpp`、`X11Panel.cpp` 两个文件依赖），于是 `make -C wind_linux test`
  用 g++ 直接编 13 个测试可执行（`wind_linux/Makefile:17` `TESTS :=` 列表），CXXFLAGS `-Werror -pthread`。
- **测试骨架自造**：`wind_linux/tests/TestHarness.h`（43 行）——无第三方框架，`CHECK/CHECK_EQ/CASE`
  宏 + 全局失败计数作退出码；与 `wind_tsf/tests` 同风格。
- **跨语言同步守卫（最有平移价值）**：
  - `wind_linux/tests/protocol_sync_test.cpp:28-47` 用 `std::regex` **解析 Rust 源码**
    `protocol.rs`（编译期 `-DWIND_PROTOCOL_RS=<路径>` 传入），逐常量比对 `ExtProtocol.h` 的 cmd id、
    overlay 枚举、`ext_kind` 字符串、`menu_target` 负值常量。
  - 反向同款：`wind-ipc/src/protocol.rs:1943-2010` `status_bits_match_the_cpp_header` 与
    `caret_sources_match_the_cpp_header` 在 Rust 测试里解析 `BinaryProtocol.h` 的
    `constexpr uint32_t STATUS_*`，双向断言（Rust 有 C++ 无、C++ 有 Rust 无都红）。
  - `keysynth_test` 直接读 Swift `KeySynthesizer.swift` 与 Rust `key_inject.rs` 源码逐个核对键名表。
  - 理由（protocol.rs:1939 注释）：「注释挡不住漂移……症状全是静默的（状态位被读成另一个状态）」。
- **字节布局冻结测试**：`protocol.rs:1887-1920` `diag_snapshot_head_layout_is_frozen` 钉死 64B 头每个
  字段偏移；`icon_shm_layout_fits_without_overlap`（protocol.rs:2079-2093）断言变体表/slot/总大小自洽。
- **Rust 侧组织**：每 crate `tests/`（集成）+ `#[cfg(test)]`（单元）；wind-store 的 `tests/` 里
  性能/内存回归也用普通 `#[test]`（`perf_cache_size.rs`、`idle_cache_reclaim.rs`、`redb_cache_high_water.rs`），
  不依赖 criterion。全仓仅 3 处 bench 形态文件（wind-config/compat_bench.rs 等）。
- **e2e（真服务+真 fcitx5）**：`scripts/linux/e2e.sh`（306 行）——`dbus-run-session` 起私有总线，
  fcitx5 `--disable=all --enable=keyboard,dbus,dbusfrontend,windinput` 起，dbus-next 客户端经
  `org.fcitx.Fcitx.InputContext1.ProcessKeyEvent` 送键，断言 `CommitString`/`UpdateFormattedPreedit`
  信号；Xvfb 里按 `WM_CLASS` 实例名 `xdotool search --classname` 找候选窗截图断言。隔离靠
  `XDG_*_HOME` + `WIND_INPUT_RUNTIME_DIR`（socket 路径 108 字节上限→临时目录必须短）。
  探活用真实通路（按 `a` 直到被吃再 Esc）而非 sleep 猜；SIGSTOP/SIGCONT 验服务卡死熔断（stall_cases）；
  e2e 专用按键合成短语追加进隔离目录的 `system.phrases.toml`（e2e.sh:76-106）。
- 已知坑都有记录（e2e.sh 头注释 + wind_linux/AGENTS.md）：dbus 模块依赖 keyboard 模块；SDK 的 Xvfb
  写死 `/usr/bin/xkbcomp` 要原地改串；xcompmgr 下 `xwd -root` 合成结果不可信、只验 `xwd -id`。
- **CI**：`.github/workflows/ci.yml` 全程 ubuntu-latest，`cargo-xwin` 交叉编 MSVC（无需 Windows
  runner），`scripts/dev.sh ci` 一键；`wind_tsf` 纯逻辑单测单列一步（「交叉编译只保证能编出来，
  一条断言都不会执行……它们是唯一会自动执行的判据闸门」）；`cargo tree` 反向断言依赖边
  （`! grep "wind-ui v"`、`! grep -qE "zstd-sys|bzip2-sys|fontdb"`）。

## 2. 错误防呆与自愈（维度④）

### 2.1 IPC 失败分类与熔断（最成熟的一段）

`wind_linux/include/Bridge.h:57-128` `BridgeClient`：
- IO 超时 2000ms **连 connect 一起管**（超时在 connect 前设好——服务 SIGSTOP 时 AF_UNIX connect
  认 SO_SNDTIMEO）；写用 `MSG_NOSIGNAL`（防 SIGPIPE 杀 fcitx5）。
- `enum class Failure` 11 种（None/NotConnected/Suspended/Connect/ConnectTimeout/Untrusted/Send/
  SendTimeout/ReadTimeout/PeerClosed/Broken），分类直接决定行为：`Bridge.h:110`
  `static bool retryable(Failure f)` 只有 `Send | PeerClosed` 可原样重发一次——「对端确定没处理」；
  **读超时不重发**（服务可能已处理，协调器不按 event_seq 去重，重发=重复上屏）。
- 任何超时后熔断 3s（`Bridge.h:62-63` 注释：每键至多付一次 2s，而不是每键 4~6s）；
  熔断期收起组字/候选窗/浮层/菜单、补发 key.hold 抬起；重连后发 `COMPOSITION_TERMINATED` + 重报焦点
  （服务恢复会处理积在旧连接里的帧）。
- `takeStallFlag()`：上层在重连成功时据此对齐两端组字状态。
- 失败即关连接交上层 `requestWithRetry` 换新连接重试**一次**（Bridge.h:130-134）。

### 2.2 通道安全

- 只连本用户服务：两条通道连上即查 `SO_PEERCRED` 对端 uid（`wind-bridge/src/endpoint.rs:137-165`，
  C++ 侧 `Bridge.h:36-37`）；无 `$XDG_RUNTIME_DIR` 时 `/tmp/wind_input{_dev}-<uid>` 兜底目录先过
  `endpoint.rs:112` `check_private_dir`（lstat 是目录、非符号链接、属主=uid、组/他人无权限）——
  防别的用户抢先建目录收走全部按键（含密码框）。Rust 服务端建目录用同一套规则（双向闭环）。
- SHM 读端防呆（`wind_linux/AGENTS.md`）：打开先 `fstat` 属主；比帧头还小也拒绝、等下一帧——
  服务端 `shm_open(O_CREAT)` 与 `ftruncate` 之间段大小为 0，按 4MB 映射一读就 SIGBUS 带走整个
  fcitx5；映射取 `min(实际大小, 4MB)`，帧头尺寸再按映射大小校验。SHM 名带 uid（`/dev/shm` 全机共用）。

### 2.3 崩溃自愈：build_guard「死亡记录」

`wind_input/crates/wind-dict/src/build_guard.rs`（473 行，纯 Rust，无 unsafe）：
- 问题定义（:3-9）：构建期峰值是几十万次小分配，任何一次失败走 `handle_alloc_error` →
  `process::abort()`，**不 unwind、不可捕获**；release 又是 `panic="abort"`。进程内接住失败这条路不存在，
  只能「下次启动时知道上次死在了构建里」。
- 机制：构建前写 `<cache>.building` 标记（输入键 + 已登记次数 + 持有者身份 pid+启动时间），完成即删；
  下次启动同键且连续死 `MAX_ATTEMPTS=2` 次 → 跳过构建、该词库本次降级不可用、给出可读原因（含手动重试指引）。
- 持有者判活防 pid 复用（Linux `/proc/<pid>/stat` starttime，:77-82）；**判不了就当活着**（宁可少退避，
  不误杀并发的在建构建）。源键变化→计数归零；删标记→手动重试。
- 文档里明确否决了 `try_reserve`/`catch_unwind` 替代方案及理由。

### 2.4 配置键迁移：声明式「不做向后兼容」

`docs/config-key-migration.md`（131 行）+ `wind-config/src/config.rs`：
- 基调（migration.md:3）：**运行时只认现行键，旧键加载即被 serde 丢弃**；文档是 126 键的单一映射真相，
  驱动 config.rs 结构体、config_schema.rs 注册表、data/config.toml、manifest.toml、前端 config-keys.json 五处同步。
- 但「丢弃」之外有三层显式机制：
  1. `RETIRED_KEYS` 显式清单（config.rs:142-178）+ `prune_retired`（:185-193）从**用户层**物理删除退役键，
     不变量是「清理前后 `load()` 结果逐键完全相同」；
  2. 值迁移函数：`Config::migrate_show_code_hint_value` / `migrate_font_size_follow_theme_value` /
     `migrate_comment_max_chars_value`（config.rs:8441-8453 统一调度）——布尔改枚举、六键并一数组。
     顺序铁律（config.rs:154-158 注释）：**先把迁移结果落盘、再清旧键**，否则下次启动再也迁不到；
  3. `KEY_ACTIONS_MATERIALIZE_VERSION`（config.rs:140）显式版本号门控物化迁移，且注释禁止
     「看起来像迁过了」的推断（config.rs:196-199：「用户手工配过 key_actions 但没迁移过时会直接猜错」）。
- 防漂移三守护测试（wind-config/AGENTS.md）：`registry_covers_every_config_key`（struct↔registry 零漂移）、
  `data_config_toml_covers_registry`（预置文件不许少键）、`data_config_toml_has_no_orphan_keys`（不许多键）。
  `data/config.toml` 同时是出厂默认值与全键说明书。

### 2.5 日志规范与滚动

`docs/logging-convention.md`：
- 分级硬约束（:18）：**`info` 及以上禁止出现在每键/每 IPC 命令/每次坐标更新/每次连接读写的路径上**；
  隐私红线（:20）：info+ 禁止用户输入、词条、preedit 明文；「不打预期分支」（:21）；不打成对冗余日志（:22）。
- 滚动命名（`apps/service/src/log_rotate.rs`）：**`.1` 恒为最新**、序号插在扩展名之前、服务每次启动强制
  滚动一次；理由（:4-14）是旧方案「无序号=最新」不自明，用户发的日志包里常常只有 `.1.log`。
  两代旧命名启动时自动迁移。基于 `file-rotate` crate，内部序号与磁盘序号差 1 的换算只许走两个出口函数。

### 2.6 服务端 handler 防呆

- `wind-bridge/src/deferred.rs` `DeferredHandler`：初始化完成前的安全代理（键事件回 PassThrough、模式回
  中文默认），`set_ready` 后切换真 handler——服务启动期间按键不炸不挂。
- 新增命令须三处同步：handler.rs（trait）→ server.rs（dispatch）→ deferred.rs（转发），
  漏 deferred 会被启动期静默丢弃（wind-bridge/AGENTS.md 列为规约）。

## 3. 跨平台原生适配（维度⑤）

### 3.1 平台 trait 边界

- **薄宿主接缝**：`wind_input/crates/wind-host/src/lib.rs`（71 行，`#![forbid(unsafe_code)]`）
  `trait HostSession: Send+Sync { should_handle_key/proces_key/snapshot/readiness/prepare }`。
  设计原则（:24-29）：**不做能力协商**（否决了 `HostProfile{has_pre_filter,...}` 方案——会在核心长出
  组合爆炸分支），API 默认按最薄宿主设计，厚宿主忽略自己已有的部分即可；「TSF 可以不调
  should_handle_key，但这个函数必须先存在、且是唯一真相源」。
  动机（:4-13）：Android 薄宿主逼出的教训——宿主手写判据与核心漂移，一个会话里同形 bug 出现三次。
- **厚宿主命令面**：`wind-bridge/src/handler.rs` `MessageHandler` trait（Windows/Linux 全命令），
  `server.rs::dispatch_command` 分发；`KeyAction` 枚举是服务端语义的唯一出口，三平台各自的
  ResponseRouter（C++/Swift）逐条移植消费。

### 3.2 平台差异的收纳手法

- 传输层 per-platform 文件对：`wind-rpc/src/transport_unix.rs` / `transport_windows.rs`；
  `wind-bridge/src/{server_unix,push_unix,shared_memory_posix}.rs` vs `*_windows.rs`。线路帧统一
  4 字节大端长度前缀 + JSON（wind-rpc），或 8B 头二进制（wind-ipc）。
- 纯逻辑与平台 IO 分离：`shared_render_frame.rs` 跨平台纯编帧（64B `SharedRenderHeader` + BGRA +
  hit-rect 表）「POSIX 与 Windows 写端共用，Linux 可测」（wind-bridge/AGENTS.md）；
  `wind_linux/src/core/*` 全部纯 POSIX，`OverlayInput/KeySynth/placeOverlay` 等「纯逻辑」文件配同名
  `*_test.cpp`。
- 平台差异进协议而不进代码分支：Linux 专属 `CMD_MENU_POINTER`/`CMD_OVERLAY_FRAME` 走专用码位，
  浮层落位规则（ABSOLUTE/FLIP/FOLLOW_CANDIDATE/ANCHOR/EXACT）编码进帧由宿主执行——服务端拿不到
  屏幕几何，几何决策全部下沉到 addon（`protocol.rs:270-306` `overlay` 模块）。
- 单位换算边界隔离：`wind_linux/include/Utf.h` UTF-16 码元↔UTF-8 字节（服务端光标以 UTF-16 计、
  Fcitx5 要字节偏移，「落在代理对中间宁少删」）。
- 高低频两档消息判据（`protocol.rs:309-343` CMD_EXT 文档）：「一次连续输入里会不会被反复触发」——
  高频走专用码位+定长二进制，低频走 `CMD_EXT`(0x0E01) 扩展信封（kindLen+kind+bodyLen+body）；
  **未知 kind 一律安静忽略**是新旧版本互相兼容的根本。每加 kind 必须在 `ext_kind` 模块登记常量，
  禁止调用点裸字符串。

### 3.3 按键合成的平台选型记录

Linux 命令直通车选 `InputContext::forwardKey` 而非 XTest（wind_linux/AGENTS.md 对比表）：
前端覆盖面（DBus/XIM/GTK/Qt/Wayland vs 仅 X11）、不回流（合成键不再进引擎，天然防组字污染）、
可测（DBus 收 ForwardKey 信号逐个断言）、作用范围只到当前 IC 所在应用。附 `keysynth_test` 限流护栏
（单条 seq ≤64 组合、每秒 ≤1024 事件、KeyHoldTracker 同时 ≤8 个、10s 最长保持兜底）。

## 4. 附加：wind-webdata 与网络能力

- `wind_input/crates/wind-webdata/src/lib.rs:1-9`：「Web 设置数据 RPC」指**给 Web 技术设置前端的
  数据域 RPC**（schema/dict/temp/freq/shadow/stats/theme 命名空间），经 wind-rpc `CoreRpc::data_rpc`
  转发，**不是网络**。方法名与前端 `contract.ts` 1:1。
- `[经源码检索确认：无网络能力，检索词：grep -rnE "reqwest|hyper|ureq|curl|tokio|async-std|smol"
  于 wind_input/{Cargo.toml,crates/*/Cargo.toml,apps/*/Cargo.toml} 零命中]`。tokio 已于 2026-07 移除
  （wind-bridge/AGENTS.md：IPC 为同步线程模型）。
- 设计侧收益：webdata 独立成 crate 后 Android 闭包纯 Rust，CI 用 `! cargo tree | grep zstd-sys`
  把「-sys C 依赖混回闭包」变成编译期暴露（ci.yml 注释③）。

## 5. 候选吸收点清单

| 编号 | 源码锚点 | 一句话机制价值 | 移植难度 |
|---|---|---|---|
| SCOUT-F-01 | `wind_linux/tests/protocol_sync_test.cpp:28-47` + `wind-ipc/src/protocol.rs:1943-2010` | 测试内用正则解析对端语言源文件，双向断言协议常量/状态位逐值一致，把「手抄镜像漂移」变成 CI 红灯 | 低 |
| SCOUT-F-02 | `wind_linux/include/Bridge.h:69-115` `Failure` 枚举 + `retryable()` | IPC 失败 11 分类驱动重试/熔断/透传决策；只有「对端确定没处理」可重发，读超时永不重发（防重复上屏） | 低 |
| SCOUT-F-03 | `Bridge.h:57-64` + 熔断语义（wind_linux/AGENTS.md「断线自愈」节） | 2s 超时含 connect + 3s 熔断 + 熔断期收 UI 补 keyup + 重连后 COMPOSITION_TERMINATED 对齐两端状态 | 中 |
| SCOUT-F-04 | `wind-dict/src/build_guard.rs` 全文 | `.building` 死亡记录标记（输入键+次数+pid+启动时间）让 alloc-abort 类必败构建不逐次启动重演；判活保守方向为当活着 | 低 |
| SCOUT-F-05 | `wind-config/src/config.rs:142-193` RETIRED_KEYS + prune_retired + 值迁移「先落盘再清键」 | 声明式退役键清单物理清理用户层旧键，值迁移有序落盘；配 126 键单一映射真相文档 | 中 |
| SCOUT-F-06 | `docs/logging-convention.md:18-22` + `apps/service/src/log_rotate.rs` | 「info 禁入每键路径」硬约束 + `.1` 恒最新命名 + 启动强制滚动 + 旧命名自动迁移 | 低 |
| SCOUT-F-07 | `wind-host/src/lib.rs:24-70` `HostSession` trait | 「API 默认按最薄宿主设计，厚宿主忽略已有部分」的平台缝：should_handle_key 是唯一真相源，不做能力协商 | 中 |
| SCOUT-F-08 | `wind-ipc/src/protocol.rs:309-343` CMD_EXT 扩展信封 + 两档判据 | 高频专用码位二进制/低频 kind+JSON 信封分档；未知 kind 安静忽略=向前兼容根本；kind 常量强制登记 | 低 |
| SCOUT-F-09 | `wind-bridge/src/endpoint.rs:112-165` check_private_dir + SO_PEERCRED | /tmp 兜底目录私有性校验（lstat 目录/非符号链接/属主/无组他人权限）+ 对端 uid 双端闭环，防按键被劫持 | 低 |
| SCOUT-F-10 | `wind_linux/src/core/ShmFrame.cpp` + AGENTS.md「SHM 与重启」节 | SHM 读端 fstat 属主+大小校验、映射取 min(实际,4MB) 防 SIGBUS 带走 fcitx5；SERVICE_READY 关旧映射防孤儿帧 | 低 |
| SCOUT-F-11 | `wind-bridge/src/deferred.rs` + 三处同步规约 | DeferredHandler 初始化前安全默认（键 PassThrough），服务半启动态不炸不挂；trait/dispatch/deferred 三处同步列规约 | 低 |
| SCOUT-F-12 | `scripts/linux/e2e.sh` 全文 + `TestHarness.h` | dbus-run-session+真 fcitx5+真服务全隔离 e2e；探活走真实通路不 sleep；SIGSTOP 验熔断；43 行自造测试骨架免框架依赖 | 中 |
| SCOUT-F-13 | `protocol.rs:974-995` FocusGainedPayload 变长段注释 | 「变长段必须线性走、新段一律接尾」的协议演进规约 + `>=` 逐段兼容解码，旧 DLL 短包照常解析（有专门回归测试） | 低 |
| SCOUT-F-14 | `wind_linux/src/core/KeySynth.cpp` 限流护栏 | 按键合成限流：seq≤64、每秒≤1024、hold 并发≤8、10s 最长保持兜底、逆序补发抬起防卡键 | 低 |

—— 完 ——（正文约 210 行，符合 ≤300 行约束）
