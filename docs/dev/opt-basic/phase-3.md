# opt-basic phase-3 - P2 任务卡：键盘流、基建韧性与打磨（分片）

> 文档版本: v1.0 ｜ 系统形态: Desktop GUI（Linux 桌面输入法，双 cdylib）｜
> 架构基线: Rust 2024 workspace + Slint 1.13.1 软件光栅 + Fcitx5 5.1.7 C/C++ ABI ｜
> 关联 ADR: ./docs/dev/adr/（ADR-0011 由主文档 P0.01.01 建立；本分片无新增 ADR 诉求）｜
> 最后同步 Commit: `4def355` ｜
> Hub 回链: [../opt-basic.md](../opt-basic.md)（假设清单 ASM-B-*、缺陷总清单、追溯表、关键路径均在 Hub）｜
> 逻辑自检: [已通过（随 Hub 第 2 轮收敛；本分片卡全覆盖，无独立新增发现）] ｜
> 维护约定: 代码演进后必须回写 Hub §4.1 追溯表的本分片卡状态；卡间依赖只允许指向 Hub 已定义卡号或本文件编号更小的卡

**本分片定位**：P2 = 物理微交互、弹簧动效与全键盘流打磨 + 基建韧性收尾。全部 7 张卡体量小、相互独立（除「前置依赖」列登记者），适合收尾批次或单人认领。

---

#### [REFACTOR-P2.04.01] REFACTOR-P2.04.01：迁移原子替换补 `sync_all`

- **基本属性**：
  - 绑定缺陷编号：`DEF-35`
  - 优先级与难度预估：`P2` | 低复杂度 | 预估工时: 0.5 人天
  - 前置依赖：无
  - 关键路径：`CP: 否`
  - 并行通道：`Track C`
  - 代码落地锚点：`crates/ime-config/src/migrate.rs`（`replace`，约 653-671 行）
  - 外部承接：无
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`migrate.rs` 的原子替换写临时文件后直接 rename，无 `sync_all`——与库内全部兄弟路径（`ime-dict/src/recover.rs:463`、`user_db/backup.rs:600`、`format/writer.rs:268`）不一致。迁移幂等使最坏后果限于重迁移，但落盘纪律应当全库一致。
  - **商业标杆对标**：断电安全是配置写入的默认期望。
- **设计规范与参数定义**：`replace` 在 rename 前对临时文件 `sync_all()`；与兄弟路径同一注释口径（说明 rename 原子性与 fsync 的分工）。
- **工程实现方案与代码级细节**：三行改动 + 一条断电语义注释；不引入新错误分支（`sync_all` 失败按现有 `io::Error` 传播路径）。
- **交互矩阵与全状态覆盖**：迁移成功/失败/中断三态不变（仅持久性增强）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` fsync 插入 + 注释。
  2. `[步骤 2]` 迁移回归测试全绿。
- **验收标准 (DoD)**：
  - [ ] `migrate` 测试全绿；grep 全库「写临时文件 + rename」路径的 fsync 覆盖一致（新门禁条目入 `check-self-tests`）。[自动]
  - [ ] `just ci` 全绿。[自动]

---

#### [REFACTOR-P2.04.02] REFACTOR-P2.04.02：字体探测 join 超时

- **基本属性**：
  - 绑定缺陷编号：`DEF-37`
  - 优先级与难度预估：`P2` | 低复杂度 | 预估工时: 0.5 人天
  - 前置依赖：无
  - 关键路径：`CP: 否`
  - 并行通道：`Track C`
  - 代码落地锚点：`crates/ime-ui/src/renderer/probe.rs`（约 212-221 行）
  - 外部承接：无
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`probe_font_choice` 起工作线程后 `handle.join()` 无超时（`probe.rs:212-221`）——病态字体/渲染缺陷可把 `rspinyin-ui` 线程永久停靠：`mark_ui_ready` 不到来、自绘窗永不出现、线程泄漏。
  - **商业标杆对标**：辅助线程不得无限期拖住主链（项目自己的 `join_within` 模式，`ui_thread.rs:378-398`）。
- **设计规范与参数定义**：复用 `join_within` 同型助手（helper 线程 join + channel 超时）；超时值 = `PROBE_BUDGET: Duration = 500ms`（探测是每进程一次、字体预热之外的独立步骤）；超时时采用 `CJK_FAMILIES` 首个候选（既有 fallback 顺序）并记 `ui/font/probe-timeout`（新码，进 2.2.4 清单）。
- **工程实现方案与代码级细节**：超时后 helper 线程 detach（与 `join_within` 同一哲学：宁留 helper 不拖宿主）；`FontStatus` 增 `TimedOut` 变体或复用 `Missing` + 诊断行（实现时取改动最小者）。
- **交互矩阵与全状态覆盖**：正常探测（毫秒级）/ 病态探测（注入 sleep mock）/ 无字体三态。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 超时 join + fallback + 诊断码。
  2. `[步骤 2]` 三态测试。
- **验收标准 (DoD)**：
  - [ ] 注入阻塞探测 → 500ms 后窗口照常出现（默认字体）。[自动]
  - [ ] `just ci` 全绿。[自动]

---

#### [REFACTOR-P2.05.01] REFACTOR-P2.05.01：host-suspend 消费者（挂起即收窗）

- **基本属性**：
  - 绑定缺陷编号：`DEF-31`
  - 优先级与难度预估：`P2` | 低复杂度 | 预估工时: 1.0 人天
  - 前置依赖：`REFACTOR-P0.04.02`（焦点链先完整，挂起语义才有锚点）
  - 关键路径：`CP: 否`
  - 并行通道：`Track C`
  - 代码落地锚点：`crates/ime-ui-addon/src/ui_impl/availability.rs`、`crates/ime-ui-addon/src/addon.rs`（`suspend`/`resume` 装配点）、`crates/ime-ui/src/ui_thread.rs`（命令侧）
  - 外部承接：无
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`on_host_suspend`/`on_host_resume` 是真实 FFI 槽（`ffi/abi.rs:298-313`）写 `HOST_UI_SUSPENDED`（`availability.rs:79-90`），doc 称「候选窗可见性随之」（`availability.rs:77-78`），但全库无读者——宿主挂起 UI 时活动组合的窗口不会被隐藏。
  - **商业标杆对标**：宿主状态变化即视图状态变化；被挂起的输入法残留窗口是低级缺陷。
- **设计规范与参数定义**：`suspend` 装配点追加 `post_to_window(UiCommand::Hide { reason: FocusLost, .. })`（`HideReason` 语义最接近值；若需精确语义，ADR-0005 追加 `HideReason::Suspended`——实施时按契约最小改动取舍）；`resume` 不自动重现窗口（等下一个组合），与焦点模型一致。
- **工程实现方案与代码级细节**：两行命令投递 + 一条状态测试（挂起 → 镜像收到 Hide）；`availability.rs` 的 doc 修正为真实现描述。
- **交互矩阵与全状态覆盖**：组合中挂起/空闲挂起/挂起后 resume 三态；挂起期间新 Frame 到达 → 正常暂存，unmap 状态保持到 resume 后的下一个 Show。
- **逐步落地实施步骤**：
  1. `[步骤 1]` suspend 投递 + 测试。
  2. `[步骤 2]` doc 修正。
- **验收标准 (DoD)**：
  - [ ] 组合中注入宿主 suspend → 候选窗 Hide。[自动]
  - [ ] `just ci` 全绿。[自动]

---

#### [REFACTOR-P2.05.02] REFACTOR-P2.05.02：check-host 稳健化（FRAME_TIMEOUT 负载自适应）

- **基本属性**：
  - 绑定缺陷编号：`DEF-30`
  - 优先级与难度预估：`P2` | 低复杂度 | 预估工时: 0.5 人天
  - 前置依赖：无
  - 关键路径：`CP: 否`
  - 并行通道：`Track C`
  - 代码落地锚点：`crates/ime-ui-addon/src/addon/tests.rs`（`FRAME_TIMEOUT`/`SETTLE_POLL` 及三处使用）
  - 外部承接：无
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`FRAME_TIMEOUT = 90s` 是常量（`addon/tests.rs:47`）；本次审计实测：并行构建负载下 `test_the_pre_created_window_redraws_for_a_new_frame` 在 90.5s 超时失败，单独重跑 48.9s 通过——门禁对机器负载敏感，误报会侵蚀对红绿灯的信任。
  - **商业标杆对标**：CI 门禁必须可重复。
- **设计规范与参数定义**：`FRAME_TIMEOUT` 从常量改为环境变量覆盖（`RSPINYIN_TEST_FRAME_TIMEOUT_SECS`，默认 90，`just check-host` 显式传 240）；settled 判据的「3 次连续一致」在超时分支改为「最后观测值 + painted 断言」软化（超时不再是失败条件，`painted_pixels > 0` 才是——该测试的本意是画没画，不是多快画完）。
- **工程实现方案与代码级细节**：三个用例的 `settled_checksum` 超时路径返回前先做 painted 断言；`justfile` 的 `check-host` recipe 传超时变量。
- **交互矩阵与全状态覆盖**：无交互；负载模拟（后台编译压力）下连续 3 次 `check-host` 全绿。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 环境变量 + 超时软化。
  2. `[步骤 2]` justfile 传参 + 负载复测。
- **验收标准 (DoD)**：
  - [ ] 并行负载下 `just check-host` 连续 3 次绿。[自动]
  - [ ] 默认（无变量）行为对既有 CI 不变。[自动]

---

#### [REFACTOR-P2.05.03] REFACTOR-P2.05.03：UI vtable `is_available` 槽位补 panic guard

- **基本属性**：
  - 绑定缺陷编号：`DEF-38`
  - 优先级与难度预估：`P2` | 低复杂度 | 预估工时: 0.5 人天
  - 前置依赖：无
  - 关键路径：`CP: 否`
  - 并行通道：`Track C`
  - 代码落地锚点：`crates/ime-ui-addon/src/ffi/abi.rs`（vtable 槽体约 397-399 行）、`crates/ime-ui-addon/src/ffi/abi/tests.rs`
  - 外部承接：无
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：C++ glue 直调 vtable 槽 `vt->is_available()`（`ui_glue.cpp:275-279`），槽体（`ffi/abi.rs:397-399`）不在 `guard_ffi` 内——守卫版 `rspinyin_ui_available`（`:288-292`）是另一个导出、不是槽体。函数体只读两个原子、现实可 panic 面为零，但违反「每个 `extern "C"` 体都跑在 guard 内」的全库不变量（`ffi/abi.rs:34-38` 自述）。
  - **商业标杆对标**：FFI 纪律零例外——不变量的价值正是它在零成本时也被遵守。
- **设计规范与参数定义**：槽体改为调 `guard_ffi` 包裹的内部函数（fallback 值 `false`——不可用时宿主走 ClassicUI 兜底，安全方向正确）；既有守卫导出 `rspinyin_ui_available` 复用同一内部函数，两入口一实现。
- **工程实现方案与代码级细节**：十行内改动；`ffi/abi/tests.rs` 增「槽体经 guard」结构断言（或以注入 panic 的测试佐证——两个原子读不可注 panic，则结构断言即可）。
- **交互矩阵与全状态覆盖**：可用/不可用两态行为逐字节不变。
- **逐步落地实施步骤**：
  1. `[步骤 1]` guard 包裹 + 内部函数合一。
  2. `[步骤 2]` 结构断言 + 回归。
- **验收标准 (DoD)**：
  - [ ] grep 断言：UI vtable 每个槽体都在 guard 内（入 `check-unsafe.sh` 或 crate 测试）。[自动]
  - [ ] `just ci` 全绿。[自动]

---

#### [REFACTOR-P2.05.04] REFACTOR-P2.05.04：未绑定动作的默认绑定与路由审计扩展

- **基本属性**：
  - 绑定缺陷编号：`DEF-39`、`DEF-12`（`ToggleScript` 的绑定决策）
  - 优先级与难度预估：`P2` | 中复杂度 | 预估工时: 1.5 人天
  - 前置依赖：`REFACTOR-P0.01.06`（`script` 死字段的处置先定型）
  - 关键路径：`CP: 否`
  - 并行通道：`Track C`
  - 代码落地锚点：`crates/ime-fcitx5/src/engine/rows.rs`、`crates/ime-fcitx5/src/engine.rs`（`CHORDS`）、`crates/ime-fcitx5/src/engine/binding_audit.rs`、`crates/ime-config/src/schema.rs`、`crates/ime-config/src/keymap/project.rs`
  - 外部承接：键位编辑 UI → `ADD-FEAT-P1.02.03`/`P1.02.04`；速查面板键位预留 → `KEY-P1.02.08`；本卡交付「默认绑定行 + 审计断言 + 首末页动作」。
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：四个已实现且被执行器支持的 `KeyAction` 无任何绑定行——`ToggleScript`/`ForgetHighlighted`/`PinHighlighted`/`AddPhrase`（`key.rs:51-64`；`rows.rs`/`CHORDS` 均不产出；`router.rs:560-563` 自注）；无首/末页跳转（`key.rs:32-34` 仅 Next/Prev）；`KeyAction::ToggleScript` 当前被认领后零效果（`transitions.rs:173-179`）。用户无法从键盘删错词、钉词、造词、切简繁。
  - **商业标杆对标**：微软拼音：Ctrl+Shift+数字删词、简繁一键；搜狗的快速造词。
- **设计规范与参数定义**：
  - **默认绑定**（写进 `rows.rs` 默认表 + `DEFAULT_CONFIG_TOML` 注释 + README 键表）：`Ctrl+Shift+S` → `ToggleScript`（前置：P0.01.06 已把 script 处置定型；若 v1 判定简繁转换不启用，则本行不绑，改在审计里登记「已实现未绑定」白名单）；`Ctrl+Delete` → `ForgetHighlighted`；`Ctrl+Shift+Space`? 避让既有和弦——`PinHighlighted` 无默认绑定（登记白名单），`AddPhrase` 无默认绑定（外部 `ADD-FEAT-P1.02.05` 交付时绑）。**原则：动作要么有绑定要么进白名单，不允许「认领了键却没效果」与「实现了动作却永远不可达」之外的第三态。**
  - **首/末页跳转**：`KeyAction` 追加 `PageFirst`/`PageLast`（ADR-0005 增量），默认绑定 `Home`/`End`（`KeyName` 白名单同步追加两键名）；会话侧 `Paging::flip_to_first/last`（`paging.rs` 追加，纯函数）。
  - **审计扩展**：`binding_audit.rs` 增「每个 `KeyAction` 变体必须 ≥1 绑定行或在 `UNBOUND_WHITELIST`」断言——把「无主动作」变成门禁错误而非注释。
- **工程实现方案与代码级细节**：会话侧 transitions 补两个动作的分支（复用 `page_event` 通路）；`keymap_matrix` 集成行追加；`KeyName` 白名单追加受 `MAX_KEY_BINDINGS` 影响评估（不变）。
- **交互矩阵与全状态覆盖**：Home/End 在首末页的边界（不动 + 交还按键语义按既有 paging 规则）；Forget 后高亮候选从列表消失且用户库同步（既有 executor 路径）；白名单断言测试。
- **逐步落地实施步骤**：
  1. `[步骤 1]` KeyAction 追加 + Paging 分支 + 绑定行。
  2. `[步骤 2]` 审计断言 + 白名单。
  3. `[步骤 3]` README 键表同步（与 P1.05.01 门禁联动）。
- **验收标准 (DoD)**：
  - [ ] `binding_audit` 断言：零「无绑定且不在白名单」的动作。[自动]
  - [ ] End 键直达末页候选（集成测试）。[自动]
  - [ ] `just ci` 全绿。[自动]

---

#### [REFACTOR-P2.05.05] REFACTOR-P2.05.05：reloadConfig 宿主槽位接线（配置热重载触发）

- **基本属性**：
  - 绑定缺陷编号：`DEF-34`（触发部分；诚实化部分在 `REFACTOR-P1.05.01`）
  - 优先级与难度预估：`P2` | 中复杂度 | 预估工时: 2.0 人天
  - 前置依赖：`REFACTOR-P0.01.01`（ADR-0011 的导出符号路径先建立——本卡沿用「新增导出符号、不动 vtable 槽位」模式）
  - 关键路径：`CP: 否`
  - 并行通道：`Track C`
  - 代码落地锚点：`crates/ime-fcitx5/src/addon/config.rs`（`on_config_reload` 的生产调用方）、`crates/ime-fcitx5/src/ffi/abi/lifecycle.rs`、`crates/ime-fcitx5/src/ffi/cpp/addon_glue.cpp`、`crates/ime-fcitx5/src/engine/router.rs`（`reload`）
  - 外部承接：无
  - 当前状态：`[ ] 待重构`
- **重构目标与 Demo 感弊端剖析**：
  - **现有代码具体缺陷**：`on_config_reload`（`addon/config.rs:259-287`）→ `KeyRouter::reload` 全链建成、全测，唯一调用方是测试；`start_config_watch` 只能记 `lifecycle/pending`（`config.rs:173-183`）——「this build's ABI carries no slot to reach it」（`addon.rs:62-69`）。宿主 fcitx5 的 `reloadConfig()` addon 回调无人接。
  - **商业标杆对标**：VS Code/Raycast：配置改动即时生效；最差也要一键重载。
- **设计规范与参数定义**：
  - **槽位**：`fcitx5::AddonInstance` 的 `reloadConfig()` 虚函数在 C++ glue 侧 override（`addon_glue.cpp` 的 addon 子类追加），转发到新导出 `rspinyin_config_reload() -> bool`（guard 包裹，内部调 `on_config_reload()` → 对每个活跃上下文 `router.reload(routing, host)`——`session_host` 广播与 `KeyRouter::reload` 的既有签名对齐）。
  - **语义**：reload 不重置组合（`AGENTS.md` 禁止项 23 / 0.4 规则 10，既有保证）；reload 结果（Updated/Kept/Unchanged）落诊断行（既有 `report_reload`）；P1.05.01 的「需重启」README 句在本卡合入后改为「配置重载由 fcitx5 的重载动作触发；未接槽位的宿主版本需重启」。
- **工程实现方案与代码级细节**：vtable 不动（ADR-0002 路径）；glue 的 `reloadConfig` override 与 fcitx5 5.1.7 的 addon 接口签名核对（实验室项：真机 `fcitx5-remote --reload` 或配置工具触发）。
- **交互矩阵与全状态覆盖**：组合中 reload（`[keys]` 改动 → 下一键即新表，组合不清）；非法文档 reload（Kept + 诊断行）；连续快速 reload 幂等。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 导出符号 + glue override。
  2. `[步骤 2]` 广播接线 + 语义测试。
  3. `[步骤 3]` 真机验证 + README 句更新。
- **验收标准 (DoD)**：
  - [ ] 真机触发重载后改 `[ui] max_per_row` → 下一帧页宽变化，组合不中断。[实验室]
  - [ ] `just ci` 全绿。[自动]
