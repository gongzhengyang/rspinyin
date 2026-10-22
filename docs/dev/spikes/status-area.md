# Spike 结论：StatusArea Action 不可行（本卡降级交付）

> 状态：**结论已闭合**——判定为「本卡范围内不可行」，按任务卡预设的降级出口交付；复评入口见 §5。
> 关联任务：`TASK-3.06.02`（compose/dead key、诊断节流与状态区 Action）｜ 风险锚点：卡内自设 spike 门控
> 规格来源：`docs/dev/features-add-phase3.md` TASK-3.06.02 ｜ 对标：[fcitx5-rime.md](../compare/fcitx5-rime.md) §3.2 表 #9（`rimeaction.*`）

## 1. 问题定义

fcitx5-rime 的中英/简繁状态有三个展示位：候选窗头部、StatusArea（输入法组图标 + 短标签）、托盘。
其机制（`rimeaction.cpp:96-120`、`rimeengine.cpp:395-423`）：

- 插件在引擎层创建 **C++ `Action` 子类对象**（`ToggleAction`/`SelectAction`），重写
  `shortLabel(InputContext*)`、`icon`、`activate(InputContext*)` 等虚函数——标签随每次取值实时计算；
- 用 `instance->registerAction` 按名注册到全局，再在 `refreshStatusArea(InputContext&)` 里把
  schema 相关的 Action 加进当前 IC 的 StatusArea InputMethod 组，入口有守卫：**非本插件 IC 直接
  return**（避免清掉别的输入法的状态区，`rimeengine.cpp:397-400`）；
- 状态变化后 `updateUserInterface(UserInterfaceComponent::StatusArea)` 通知 classic-ui 重新拉值；
  图标走宿主 hicolor 主题，插件不带贴图。

rspinyin 想要的最小等价物：两个 Action（中/英、简/繁），短标签中文，图标用主题名，
激活时翻转对应模式位并回报 Rust。

## 2. 现状勘察（代码事实）

| 勘察项 | 现状 | 结论 |
|---|---|---|
| 胶水引擎类 | `RspinyinEngine : public fcitx::InputMethodEngineV2`（`ffi/cpp/engine_glue.cpp:198`） | 无 Action 相关基类设施 |
| 头文件 | 仅 `inputpanel.h`/`inputcontext.h`/`instance.h` 等 | **无 `fcitx/action.h`、无 `fcitx/statusarea.h`** |
| 注册路径 | 无 `registerAction`、无 `statusArea().addAction`、无 `updateUserInterface(StatusArea)` | 整条链路缺席 |
| C ABI | 键入 / 效果出（commit、preedit、post_ui、toggle、diagnose）单向各一 | **无「模式位推送」与「Action 激活回报」两个通道** |
| C++ 测试设施 | 无（`just check-host` 只编译 + 断言工厂符号） | C++ 新增面全部**不可自动验证** |
| 本机渲染验证 | WSLg + X11 单档（[abi-spike.md](abi-spike.md) §4 的环境结论） | StatusArea 是 classic-ui 的绘制物，本机只能走查，不能断言 |

## 3. 判定：不可行（本卡范围）

依据四条，按权重排序：

1. **胶水面是「类 + 生命周期 + 双向 ABI」三件事，不是一件事。**
   动态短标签排除 `SimpleAction`（标签静态），必须自写 `fcitx::Action` 的 C++ 子类并实现虚函数——
   虚函数无法从 Rust 侧提供，C++ 类不可避免。在此基础上还需要：引擎析构前 `unregisterAction`、
   每 IC 的 add/remove 与 foreign-IC 守卫（rime 为此专门写了 `refreshStatusArea` 的防串扰分支——
   状态区是**跨插件共享的 UI 地产**，刷新错了会清掉别的输入法的图标，是本项目 0.4 表里
   「焦点丢失」级别的可观测回归类别）、以及 C ABI 两个新通道 + 回调生命周期护栏
   （TASK-1.08.02 的 panic-guard 纪律要求每个入口包一层）。合计新增 C++ 约 200~300 行、
   ABI 函数 2~4 个、跨语言回调 1 组——远超卡内为 ③ 预留的份额（整卡 2.5 人天，已含 compose 与节流）。
2. **零可断言面。** 上述全部逻辑落在 C++ 或「C++ 虚函数 + classic-ui 渲染」的交界上：
   纯 Rust 门禁（fmt/clippy/nextest/doctest）一个都够不着，`check-host` 只保证能编译。
   换言之交付物处于本仓库测试纪律的盲区，只能靠真桌面走查——而 StatusArea 的正确性恰恰
   依赖多输入法共存场景（foreign-IC 守卫），单输入法环境走查不可能暴露串扰回归。
   不能被任何门禁约束的 C++ 代码，按 AGENTS.md 3.2/3.6 的标准不构成可交付面。
3. **回调生命周期是新增风险类。** C++ → Rust 的「Action 激活」回调要求 Rust 侧保证
   「引擎存续期 > 回调可达期」，这是目前 ABI 上不存在的时序契约；做错的下场是悬垂调用
   （UB），做对的验证手段（压力测试 teardown 边沿）本机同样没有。
4. **对标物的复杂度不来自我们缺的部分。** rime 的 Action 面之所以值得写，是它要表达
   N 个 schema × M 类 switch 的动态菜单；我们的需求只有两个布尔位的展示位。为两个布尔
   引入整条 Action/Menu/StatusArea 机制，投入产出比不支持在一张 P2 卡的份额内完成。

**结论**：走卡内预设的降级出口。这不是「永远不做」的判定——机制本身没有技术障碍，
缺的是测试设施与工时（见 §5 复评条件）。

## 4. 降级交付（已随本卡落地）

| 项 | 落点 | 说明 |
|---|---|---|
| compose/dead key（Rust 半） | `engine/router.rs` 前置步 + `Host::process_compose` 接缝 + `engine.compose` 配置键（默认关）+ `keymap_matrix` SC-KEY-17 行 | 语义对齐 rimestate.cpp:170-180：未完成组合吞键、完成字符按长度 1 直通 commit 处理 |
| 诊断节流 | `ime-fcitx5` 与 `ime-ui-addon` 两处发布器：60s 放行窗、32 槽、首条永放 | 对齐 fcitx5-rime 的 60s 窗；不改错误码集合 |
| 状态可见性 | cheatsheet「模式」区新增「状态条」行：窗顶状态条显示当前中英 · 简繁 | 用户能在 cheatsheet 里查到**哪里**看状态；StatusArea 本体登记后续评估 |
| compose 宿主半（FFI 集成项） | 见 §5 | `Host::process_compose` 已是接缝，缺 C++ 侧答问 |

### 4.1 登记为 FFI 集成项：compose 的 C++ 答问槽

与 StatusArea 不同，compose 的 Rust 半已完整（前置步、配置键、audit 行、16 个矩阵场景），
缺的只是宿主半——把 `instance()->processComposeString(&ic, sym)` 接进 C ABI。规划形态
（复评时按此实施，无需重新勘察）：

- C ABI 新增一个问答槽：入参 `(ic_id, keysym, state)`，出参镜像 `ComposeOutcome` 三态
  （Unsupported / Swallowed / Committed(char)）；compose 缓冲由 C++ 侧按 IC 持有，
  语义与 `processComposeString`/`resetCompose` 一一对应；
- Rust 侧零改动：`Host::process_compose` 的默认实现（`Unsupported`）换成转发即可；
  `engine.compose` 默认关的开关语义不变，未接入时前置步天然惰性；
- 验证沿用 SC-KEY-17 的矩阵行 + X11 档 dead key 组合走查（´+e→é、¨+a→ä，含与 rspinyin
  键位的冲突样例）。

## 5. 复评条件（何时重启 StatusArea）

同时满足以下三条即可重启，实现按 §2 勘察的缺口直接补：

1. **C++ 侧获得最小测试设施**——至少是一条「glue 层行为回放」通道（记录/重放 C++ 侧调用），
   使 foreign-IC 守卫与回调生命周期可被自动断言；
2. **一个可复现的多输入法桌面环境**（非本机 WSLg），能走查 classic-ui 状态区在
   keyboard-us ↔ rspinyin 切换下的表现；
3. **独立的工作量预算**（估 1.5~2 人天，纯 C++ + ABI + 走查，不含 Rust 改动）。

在此之前，`features.md` 的状态可见性结论以 cheatsheet「状态条」行为准。
