# ADR-0003：UI 角色拆分为独立 addon（双 cdylib）

> 状态：**已接受（Accepted）** ｜ 决策日期：**2026-09-29** ｜ 决策人：主 Agent（宿主集成）｜
> 关联文档：[../features.md](../features.md) 的 2.1（分层拓扑与运行时隔离）、2.2.3（插件 ↔ 宿主 C ABI）、`TASK-1.04.03` 与 `TASK-1.05.01` 任务卡 ｜
> 关联 ADR：[0000-upstream-decisions.md](0000-upstream-decisions.md)、[0001-frozen-boundary-contracts.md](0001-frozen-boundary-contracts.md)、[0002-rust-exports-addon-factory.md](0002-rust-exports-addon-factory.md) ｜
> 实测证据：本机 Fcitx5 5.1.7 上的崩溃复现（见下文「背景」末段）

---

## 背景

`TASK-1.04.03` 的目标是让插件接管 `fcitx::UserInterface`，使候选框完全自绘并抑制 ClassicUI。
该任务卡假定存在一个注册接口（原文写作 `registerUserInterface`），实现时按该假定推进，随后
对照本机安装的 Fcitx5 5.1.7 头文件逐条核对，发现**假定不成立**：

1. **没有注册接口。** `/usr/include/Fcitx5/Core/fcitx/userinterfacemanager.h` 中的
   `UserInterfaceManager` 只有一个析构函数与若干虚拟键盘相关方法，**没有任何注册
   `UserInterface` 的方法**。活跃 UI 不是被注册进去的，而是由 Fcitx5 从
   `Category=UI` 的 addon 中按 `available()` / `UIPriority` 选出。
2. **`Category` 是单值枚举。**
   `/usr/include/Fcitx5/Core/fcitx/addoninfo.h:22` 定义
   `FCITX_CONFIG_ENUM(AddonCategory, InputMethod, Frontend, Loader, Module, UI)`，
   且 `AddonManager::addonNames(AddonCategory category)` 按单一类别返回 addon 名集合。
   一个 addon 只能属于一个类别。
3. **输入法只从 `Category=InputMethod` 的 addon 中发现**，而 `InputMethodManager` 取引擎的
   方式是**把 addon 实例当作 `InputMethodEngine` 使用**——`InputMethodEngine` 本身继承自
   `AddonInstance`（`inputmethodengine.h:17`），因此这是对 addon 实例向下转型。

结论：**同一个 addon 实例无法同时充当输入法引擎与 `UserInterface`。** 若以
`Category=InputMethod` 注册却未提供引擎，Fcitx5 会在输入法枚举阶段崩溃。

该崩溃已在本机复现（Fcitx5 5.1.7，`Category=InputMethod` 的 addon 未提供引擎）：

```
I ... addon_glue.cpp:208] rspinyin: addon loaded
I ... addonmanager.cpp:195] Loaded addon rspinyin
I ... inputmethodmanager.cpp:190] Found 737 input method(s) in addon keyboard
=========================
Fcit 5.1.7 -- Get Signal No.: 11
```

同一环境下 `fcitx5 --disable rspinyin` 全程干净退出，3/3 次对照一致。

## 决策

**插件以两个 addon、两个 cdylib 交付：**

| 库 | addon 类别 | addon 实例的角色 | 职责 |
|---|---|---|---|
| `librspinyin.so` | `InputMethod` | `fcitx::InputMethodEngineV2` | 按键路由、解码、向输入上下文写 preedit/候选 |
| `librspinyin-ui.so` | `UI` | `fcitx::UserInterface` | 读取输入上下文面板、软件光栅自绘候选框 |

两个库**不共享任何静态状态，也不需要 IPC**：数据经 Fcitx5 自身的接口流转——引擎侧调用
`InputContext` 的 preedit/候选更新接口，UI 侧的 `UserInterface::update(InputContext *)`
读取该上下文的 `inputPanel()`。这正是 Fcitx5 自带 classicui 的工作方式。

## 理由

- `Category` 单值（上文证据 2）直接排除了「一个 addon 两用」。
- 工厂符号名固定为 `fcitx_addon_factory_instance`（见 ADR-0002），由 `Library=` 指定的库
  `dlsym` 得到；`AddonFactory::create(AddonManager *)` 拿不到 addon 名，因此**同一个 .so 被
  两个 conf 引用时会创建两个无法区分的实例**。一个 .so 只能服务一种 addon 类别。
- 数据经宿主接口流转是 Fcitx5 的惯用做法，避免了跨 .so 的静态状态与自建 IPC——后者会引入
  与 `ASM-10`/`ASM-11` 的并发模型冲突的风险。
- 新增导出符号而非新增 vtable 槽位，`RSPINYIN_ABI_VERSION` **不变**（见 ADR-0002 的既有约束）。

## 后果

1. **features.md 需同步修正**：2.1 的运行时隔离模型、0.7 与 5.1 的模块/任务锚点需登记第二个
   cdylib；`TASK-1.04.03` 的验收标准 1 与 5 依赖本决策落地后方可判定。
2. **打包布局增加一个 conf**：`packaging/fcitx5/` 需同时提供 `Category=InputMethod` 的
   `rspinyin.conf` 与 `Category=UI`（`UIPriority > 0`、`UIType=PhysicalKeyboard`）的 UI conf。
   `TASK-1.07.01` 的安装脚本须安装两者。
3. **新增一个 crate** 承载 UI addon 的 C++ 胶水与 Rust 侧；`TASK-1.05.01` 的 Slint 平台与
   光栅渲染器归入该 crate 使用。
4. `TASK-1.04.03` 中已完成的 Rust 半（面板快照、光标矩形、接管策略与诊断码）仍然有效，但其
   宿主侧接线需要迁到 UI addon 的胶水中。
5. 本条决策推翻了任务卡对宿主 API 的假定，按 AGENTS.md「文档与代码不一致时以代码为准并修正
   文档」，`features.md` 的相关表述须一并改写。
