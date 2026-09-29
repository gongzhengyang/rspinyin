# ADR-0004：UI 角色拆分为 `ime-ui-addon` crate（第二个 cdylib 落地）

> 状态：**已接受（Accepted）** ｜ 决策日期：**2026-09-29** ｜ 决策人：主 Agent（宿主集成）｜
> 关联文档：[../features.md](../features.md) 的 2.1（分层拓扑与运行时隔离）、2.2.3（插件 ↔ 宿主 C ABI）、`TASK-1.04.03` 与 `TASK-1.05.01` 任务卡 ｜
> 关联 ADR：[0000-upstream-decisions.md](0000-upstream-decisions.md)、[0001-frozen-boundary-contracts.md](0001-frozen-boundary-contracts.md)、[0002-rust-exports-addon-factory.md](0002-rust-exports-addon-factory.md)、[0003-ui-role-separate-addon.md](0003-ui-role-separate-addon.md) ｜
> 实测证据：本机 Fcitx5 5.1.7 上的构建与符号检查（见下文「后果」末段）

---

## 背景

[ADR-0003](0003-ui-role-separate-addon.md) 裁定插件必须以**两个 cdylib** 交付，其后果 #3 要求"新增一个 crate 承载 UI addon 的 C++ 胶水与 Rust 侧"。该后果长期未落地，且代码里存在一处更严重的状态：

- `crates/ime-fcitx5/src/ffi/cpp/ui_glue.cpp` 定义了 `createUiInstance()`，但**全仓库没有任何调用点**——`addon_glue.cpp` 的工厂只调用 `createAddonInstance(manager)`，而 `engine_glue.cpp` 的 `RspinyinAddon final : public RspinyinEngine` 只派生自 `InputMethodEngineV2`。
- 于是 `uiSlot()` 恒为 null，`rspinyin_ui_activate()` 永远返回 `kUiActivationNotRegistered`，候选框始终留在 ClassicUI。**UI 角色处于"有代码、无 addon"的状态。**
- `crates/ime-ui`（完整的视图层：X11 / Wayland 四档后端、Slint 平台、软件光栅）被 `ime-fcitx5` 声明为依赖却从未被引用，被 LTO 与死代码消除整体丢弃——实测 `nm -D target/release/librspinyin.so | grep -ci slint` = 0。

此外，`ui_impl/` 下的两个模块**互相耦合于进程级静态变量**，使拆分不能只是"搬文件"：

- `ui_impl/takeover.rs` 的 `register_takeover()` 读 `availability::window_backend_available()`
- `ui_impl/availability.rs` 读 `takeover::TAKEOVER_DECLINED`，并读 `addon::candidate_window_ready()`

拆成两个 `.so` 后每个库各持一份静态变量副本，引擎读到的 `WINDOW_BACKEND_AVAILABLE` 永远是初始值 `false`，`register_takeover()` 会永远报 `Unsupported` 而**静默不接管**——一个不会报错、只会"候选框没出现"的故障。

## 决策

**新增 crate `crates/ime-ui-addon`，产出第二个 cdylib `librspinyin_ui.so`，承载 UI 角色的全部 C++ 胶水与 Rust 侧。**

| | `librspinyin.so`（`ime-fcitx5`） | `librspinyin_ui.so`（`ime-ui-addon`，新） |
|---|---|---|
| addon 类别 | `InputMethod` | `UI`（`UIPriority=10`、`UIType=PhysicalKeyboard`） |
| addon 实例 | `RspinyinAddon : fcitx::InputMethodEngineV2` | `RspinyinUiAddon : fcitx::RspinyinUi : fcitx::UserInterface` |
| 职责 | 按键路由、解码、把 preedit/候选写进 `InputContext` | 读 `inputPanel()`、解析光标、软件光栅自绘候选框 |
| 依赖 | `ime-types` `ime-core` `ime-dict` `ime-config` `ime-diag` | `ime-types` `ime-config` `ime-ui` `ime-diag` |

**移入新 crate 的内容**：`ffi/cpp/ui_glue.cpp`（新增配套的 `ui_addon_glue.cpp` 承载工厂、握手与生命周期）、`ui_impl/` 全部子模块、`cursor/`（光标阶梯）、`screen.rs`、`addon.rs` 中的 UI 线程启动脚手架、UI 侧 C ABI（`RspinyinUiVtable`）。

**引擎侧移除**：`ui_impl`、`cursor`、`screen`、`ui_glue.cpp`、UI 侧的三个导出符号与两个 vtable 槽位，以及 **`ime-ui` 依赖**。

### 决策 1：接管决策迁到 UI 侧，引擎不再决策

`register_takeover()` 的两个输入——窗口是否就绪、平台后端是否可用——**都是 UI 侧的本地事实**。拆库后引擎无法读取它们，因此决策整体搬进 UI 库：UI addon 的 `available()` 以自身状态作答，Fcitx5 的 `UserInterfaceManager::updateAvailability()` 按 `UIPriority` 遍历 `Category=UI` 的 addon 选出第一个 `available()` 为真者；`rspinyin_ui_activate()` 作为"状态已变，请重新评估"的催促，由 UI 库在自己的就绪状态变化时调用。

引擎的 `register_ui()` 步骤随之删除，`INIT_STEPS` 由 8 步降为 5 步（diagnostics / data-dirs / config / store-recovery / lexicon）。

### 决策 2：`RSPINYIN_ABI_VERSION` 由 1 升到 2

引擎的 `RspinyinVtable` 中 `on_input_panel_update` 与 `on_cursor_rect` 两个槽位的**唯一调用者是 `ui_glue.cpp`**（UI 侧），拆出后成为死槽位。删除它们会改变结构体布局，而 C 结构体没有其他兼容机制，因此必须升版本。

UI 库得到自己的 `RspinyinUiVtable`（两个回调 + 生命周期 + 悬停/恢复 + 可用性，共 7 项）、自己的 `rspinyin_ui_plugin_init` 握手与自己的 `rspinyin_ui_addon_factory`。这是**新符号、新表**，不是对冻结契约的原地修改；两个库各带自己的 C++ 胶水一起编译，运行期不可能出现版本错配，`check_abi` 的语义（"本 build 与自己的胶水是否同版本"）不变。

### 决策 3：崩溃护栏复制而非抽公共 crate

`catch_ffi` / `guard_ffi` / `write_stderr_line` / `emit_diagnostic` 在新 crate 里复制一份。两个库被独立 `dlopen`，让它们通过一个共享 crate 产生链接期耦合会破坏这份独立性——这正是拆分要消除的东西。C++ 侧早已接受同一取舍：`#[repr(C)]` 结构体定义在每个胶水翻译单元里各写一份，`addon_glue.cpp` 明确写了"There is deliberately no shared header"。

### 决策 4：产物名用下划线 `librspinyin_ui.so`

ADR-0003 的正文写作 `librspinyin-ui.so`，但 **Cargo 拒绝 `[lib] name` 含连字符**（实测：`error: library target names cannot contain hyphens`）。而 Fcitx5 把 `Library=` 的值**原样加 `.so`** 解析（实测：已安装的 `libclassicui.so` 对应 `Library=libclassicui`），所以描述符必须写 `Library=librspinyin_ui`。

结论：产物名为 `librspinyin_ui.so`，ADR-0003 正文中的连字符拼写按本条更正。

## 理由

- ADR-0003 已论证 `Category` 单值使"一个 addon 两角色"不可行；本条只是把它落地。
- 接管决策的输入全是 UI 侧事实，**拆库后引擎在物理上无法做出该决策**，所以迁移是唯一自洽的方案，而非偏好。
- 依赖方向保持不变：`ime-ui-addon` 与 `ime-fcitx5` 在 `check-deps.sh` 的 `LAYERS` 中**同为 rank 5**，互不依赖——等秩即"只允许依赖严格更低的层"，这正是禁止两库互相链接的机制。
- 引擎移除 `ime-ui` 依赖后，`librspinyin.so` 由 610KB 降到 510KB 且不再含任何视图层代码；视图层整体进入 `librspinyin_ui.so`（1.04MB，含 Slint 静态链接）。

## 后果

1. **门禁与规范同步更新**：`scripts/check-unsafe.sh` 的 `ALLOWED_DIRS` 增加 `crates/ime-ui-addon/src/ffi/`（自检同步）；`scripts/check-deps.sh` 的 `LAYERS` 增加 `ime-ui-addon: 5`（错误信息里的链式说明同步）；`AGENTS.md` §3.3 与 §8.2 的 `unsafe` 允许位置由两处改为三处。
2. **打包布局**：新增 `packaging/fcitx5/rspinyin-ui.conf`；`xtask/src/install.rs` 的 `PAYLOADS` 增加第二个 `AddonLibrary` 与第二个 `AddonDescriptor`。`stage_library` / `elf.rs` 无需改动——载荷表的注释早已预告"第二个库走完全相同的步骤"。
3. **版本门禁扩展**：`xtask check-versions` 由校验单一描述符改为遍历 `packaging/fcitx5/*.conf` 中的 addon 描述符，并断言**两个描述符的 `core:` 门槛一致**。门槛不一致会让 Fcitx5 静默抑制其中一个 addon，用户得到"半个输入法"。
4. **`features.md` 需同步修正**（ADR-0003 后果 #1 遗留项）：2.1 的运行时拓扑图、0.7 模块总览、5.1 追溯表需登记第二个 cdylib 与 `crates/ime-ui-addon`。
5. **`docs/dev/opt-deploy.md` 的 `BUILD-P0.03.01` 已按本条落地**，其"新增 crate 需改 `unsafe` 白名单"的开工前置已由后果 #1 满足。
6. **遗留观察（未在本条内解决）**：`librspinyin_ui.so` 的动态符号表导出了 6 个 `slint_*` 符号（`slint_send_keyboard_char`、`slint_mock_elapsed_time` 等），来源是 `slint` 的 `std` / `compat-1-2` feature 拉入的 `i-slint-backend-selector`。`OB-4` 约束的是"不得分发暴露 Slint API 供第三方编程使用的应用"，一个导出若干 Slint 测试辅助符号的候选框插件是否落入该范围需要单独判断；`scripts/check-slint-leak.sh` 目前只审计 `ime-ui` 的 Rust 公共 API（`cargo public-api`），不覆盖 cdylib 的动态符号表。**该问题记入待办，不在本条内裁决。**

**实测证据（2026-09-29，本机 Fcitx5 5.1.7）**：

```
target/release/librspinyin.so     509,736 B   fcitx_addon_factory_instance ✓   slint 符号 0
target/release/librspinyin_ui.so 1,091,880 B  fcitx_addon_factory_instance ✓   slint 符号 6
```

两个 `.so` 均导出工厂符号，且 `librspinyin_ui.so` 的 `NEEDED` 含 `libFcitx5Core.so.7` / `libFcitx5Utils.so.2`。
