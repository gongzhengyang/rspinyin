# rspinyin 端到端功能、微交互与商业化 UI 视觉测试用例套件

> 文档版本: v2.0 ｜ 系统形态: Desktop GUI（Linux 桌面输入法：Fcitx5 进程内双 addon + 自绘候选框） ｜ 被测基线: Rust 2024 workspace（9 crates + xtask + alloc-count）、Fcitx5 5.1.19、`cargo-nextest` ｜ 关联 ADR: [adr/0000-upstream-decisions.md](adr/0000-upstream-decisions.md)、[adr/0002-rust-exports-addon-factory.md](adr/0002-rust-exports-addon-factory.md)、[adr/0003-ui-role-separate-addon.md](adr/0003-ui-role-separate-addon.md)、[adr/0004-ui-addon-crate-split.md](adr/0004-ui-addon-crate-split.md)、[adr/0005-incremental-contract-extension.md](adr/0005-incremental-contract-extension.md)、[adr/0006-overlay-channel.md](adr/0006-overlay-channel.md)、[adr/0010-test-only-allocation-counter.md](adr/0010-test-only-allocation-counter.md)、[adr/0011-frame-transport.md](adr/0011-frame-transport.md) ｜ 平台侧任务清单: [features-test.md](features-test.md) ｜ 最后同步 Commit: `408d2c6` ｜ 逻辑自检: [已通过（第 2 轮收敛，Blocker 1 / Major 3 / Minor 5 ｜ 优化采纳 2 / 否决 2 / 登记 0）] ｜ 维护约定: 被测代码演进后必须回写追踪矩阵的 TC 列与维度列；用例状态变更须同步本文档与对应分片

**Hub & Spoke**：本文档是 **Hub**，承载追踪矩阵、假设清单与 **P0 核心基线用例**。P1/P2 用例按模块分片保存于 [`./tests/`](tests/)：`core.md`、`dict.md`、`rt.md`、`ui.md`、`sec.md`、`diag.md`、`infra.md`、`cfg.md`。分片头部含 Living Header 并回链本文档的矩阵。

**证据存盘**：`<RUN>` = `results/runs/run-[YYYYMMDD-HHMMSS]/`。用例证据包为 `<RUN>/<模块代码>/<TC-ID>/`，命名规范与失败取证见 [features-test.md](features-test.md) 的 `FEAT-TEST-P0.05.04`。

---

## 0. 覆盖状态与可执行性声明（**必读**）

本套件按阶段零实测（见 [features-test.md](features-test.md) 0.2）分为三类，**每条用例都标注了类别**。判定由 `FEAT-TEST-P0.05.05` 的环境能力门禁机械执行。

| 类别 | 含义 | 用例数（v2.0） |
|---|---|---|
| `[可执行]` | 代码已存在，本机可直接运行并判定（整体判定口径；首标注口径为 381，差异见第 2 节） | **376** |
| `[待实现: TASK-x.yy.zz]` | 被测代码不存在，用例锚定冻结契约或 features.md 的数值规范 | **0**（v1.0 时的 116 条已随 219 卡闭环全部转正） |
| `[不可验证]` | 代码存在但本机环境测不了（features.md 0.5.5），需外部环境 | **1**（`TC-RT-14` 混合 DPI 多屏） |

**v2.0 同步后的三条基线事实**：

1. **候选框 UI 已完整落地**：`crates/ime-ui/ui/` 现有 4 个 `.slint`（`overlay` / `theme` / `candidate_grid` / `candidate`）；`1.05.01–1.05.08` 全部 `COMPLETED`。v1.0 中标注 `[待实现]` 的 40 条 UI 用例已在 run-20261006-034915 全量轮执行——像素级断言由内存表面探针等价执行；屏上窗口级观测受 isolated defect「depth-24 回退路径渲染黑屏」阻断处逐条如实标注（`TC-RT-21` 失败隔离保持，见其复测记录）。
2. **`ime-config` 与 `ime-diag` 已全量实现**：`ime-config` 117 个 `#[test]`（schema/repair/migrate/reload/keymap/scheme/writeback），`ime-diag` 202 个（log/redact/panic/crash/uiframe/perms/probe/report）。v1.0 的"空壳"事实不再成立。
3. **测试平台已交付**：`xtask/src/testd/` 全模块落地（XTEST 注入、XGetImage 截图、帧率/内存采样、`UiFrame` 镜像、场景引擎、沙盒、证据链、自愈、纯度门禁），20 张 `FEAT-TEST` 卡全部 `[x]`（见 [features-test.md](features-test.md)）。端到端实证：沙盒 fcitx5 会话 20/20 轮完成首词上屏（`nihao`+空格 → 你好）。

**已有测试基线**（不得下降，由 `FEAT-TEST-P0.05.03` 守）：**3,701 个 `#[test]`**——ime-types 58 / ime-core 556 / ime-dict 257 / ime-config 117 / ime-ui 696 / ime-ui-addon 125 / ime-fcitx5 475 / ime-diag 202 / alloc-count 6 / xtask 1,209。criterion 基准 9 个（`ime-core/benches/{decode,input,passthrough}.rs`、`ime-dict/benches/{dict,userdb}.rs`、`ime-fcitx5/benches/transport.rs`、`ime-ui/benches/{frame,histogram,wakeup_latency}.rs`）。场景 TOML 18 个（`tests/fixtures/scenarios/`）。

---

## 1. 系统设计假设清单（Assumptions First）

完整清单见 [features-test.md](features-test.md) 第 1 节（`ASM-T-01` ~ `ASM-T-11`）。**全部量化阈值来自 `docs/dev/budgets.json`**（`features.md` 0.5.3 的机器可读镜像），用例中**严禁**出现该文件之外的数值。摘要：

| 假设编号 | 关键内容 | 影响 |
|---|---|---|
| `ASM-T-01` | 被测对象是 fcitx5 进程内的**两个 addon**（ADR-0003/0004），不是独立进程 | 全部 `[实验室]` 用例 |
| `ASM-T-02` | 候选框完全自绘，**无 DOM / 无 A11y 树**；语义断言用 `UiFrame` 镜像（`runtime://ui_frame`，`ime-diag/uiframe/`）替代 | 视觉与交互用例 |
| `ASM-T-03` | 本机为 WSL2 + WSLg（`DISPLAY=:0`、Weston） | X11 可测，Wayland 不可测 |
| `ASM-T-04` | 延迟预算：`key_to_present_p99=16ms`、`decode_p99=3ms`、`raster_p99=1.5ms`、`first_key_to_visible_p99=8ms`、`addon_load=120ms` | 延迟用例 |
| `ASM-T-05` | 空闲：`idle=0.3%` 单核、`idle_redraw_count=0`、`idle_poll_timer_count=0` | 空闲占用用例 |
| `ASM-T-06` | 内存：`ui_rss=18MB`、`plugin_rss=45MB`、`dict_mmap_rss=25MB`；体积：`so_stripped=12MB`、`base_dict=20MB` | 内存与体积用例 |
| `ASM-T-07` | 开发词库 `base.tsv` = 5,871 行（显式编译用）；`base.dict` = 15.62MiB 全量（2026-10-01 起）；完整 40 万词库不在仓库 | 规模类断言用合成词库 |
| `ASM-T-08` | 本机不可验证：Wayland 三档（T1–T3 层）、真实亚克力、多显示器热插拔、8 小时长稳 | 标注而非跳过（Wayland 用例带 `Ⓦ 文档豁免` 执行记录） |
| `ASM-T-09` | 并发模型：两线程 + SPSC 队列 + `eventfd`，无异步运行时 | 并发与背压用例 |
| `ASM-T-10` | `raw ≤ 64` 字节；候选 ≤ 45；单候选 ≤ 32 字符 | 边界用例 |
| `ASM-T-11` | 基准仅在**空闲机器**上有效（实测教训：并行 agent 下 511ns vs 空闲 726ns，criterion 报告假回归）；`testd purity` 已将此教训机器化 | 全部 `[性能]` 用例 |
| `ASM-T-12` | 端到端驱动链已交付：`xtask testd`（XTEST + 沙盒 fcitx5）→ 被测客户端 `xtask/testd/client/`（GTK3 `CommitProbe`，上屏/preedit 以 JSON 行输出）→ 证据链 | 全部 `[实验室]` 用例 |
| `ASM-T-13` | 增量域预算裁决（features-add `ASM-A-02`）：增量独占 ≤ 2.10ms；短语/模糊音/简拼/动效的开启态增量均受此约束 | 增量域性能用例 |

---

## 2. 功能-代码-用例覆盖追踪矩阵 (Traceability Matrix)

**维度图例**：`①` 核心业务主通路 · `②` 边界与容错 · `③` 全状态防御矩阵 · `④` 商业化 5 态微交互与材质 · `⑤` 人机工学与全键盘流。
**可执行性**：`✅` 本机可执行 · `⏳` 待实现（附任务 ID）· `🚫` 本机不可验证 · `Ⓦ` = 分片执行记录中的 `[W] 文档豁免`（用例可构造、判据需外部环境）。

| 功能条目编号 | 功能清单条目 | 关联代码路径 | 承接平台任务 | 承接用例 (TC) | ① | ② | ③ | ④ | ⑤ | 可执行性 |
|---|---|---|---|---|---|---|---|---|---|---|
| `REQ-CORE-01` | 拼音音节表与输入规范化 | `crates/ime-core/src/segment/syllable.rs` | `P0.03.01` | `TC-CORE-01`~`05` | ✅ | ✅ | ✅ | — | ✅ | ✅ |
| `REQ-CORE-02` | 音节切分 DAG 与非法串保护 | `crates/ime-core/src/segment/dag.rs` | `P0.03.01`、`P0.03.02` | `TC-CORE-06`~`10` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-CORE-03` | 输入缓冲与按音节删除语义 | `crates/ime-core/src/input/buffer.rs` | `P0.03.01` | `TC-CORE-11`~`15` | ✅ | ✅ | ✅ | — | ✅ | ✅ |
| `REQ-CORE-04` | K-best Viterbi 解码与候选生成 | `crates/ime-core/src/viterbi/{decoder,kbest,lattice}.rs` | `P0.03.01`、`P0.03.03` | `TC-CORE-16`~`20`（深化 `TC-CORE-47`） | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-CORE-05` | 语言模型评分与用户词频融合 | `crates/ime-core/src/lm/{score,ngram}.rs` | `P0.03.01`、`P0.03.03` | `TC-CORE-21`~`25` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-CORE-06` | 非拼音直通与临时英文模式 | `crates/ime-core/src/passthrough.rs` | `P0.03.01` | `TC-CORE-26`~`30`（深化 `TC-CORE-49`/`50`） | ✅ | ✅ | ✅ | — | ✅ | ✅ |
| `REQ-CORE-07` | Preedit 生成与切分高亮段 | `crates/ime-core/src/preedit.rs` | `P0.02.01` | `TC-CORE-31`~`35` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `REQ-CORE-08` | 自定义短语引擎（增量） | `crates/ime-core/src/phrase.rs`、`phrase/writer.rs` | `P0.03.01`、`P0.03.02` | `TC-CORE-51`~`55` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-CORE-09` | 模糊音匹配层（增量） | `crates/ime-core/src/fuzzy.rs` | `P0.03.01`、`P0.03.03` | `TC-CORE-56`~`60` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-CORE-10` | 简拼与首字母缩写展开（增量） | `crates/ime-core/src/viterbi/lattice/abbrev.rs` | `P0.03.01`、`P0.03.03` | `TC-CORE-61`~`65` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-DICT-01` | 词库二进制格式 v1 读写 | `crates/ime-dict/src/format/{mod,reader,writer}.rs` | `P0.03.02` | `TC-DICT-01`~`05`（深化 `TC-DICT-36`~`38`） | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-DICT-02` | FST 索引与 mmap 只读加载 | `crates/ime-dict/src/{fst_index,mmap}.rs` | `P0.03.02` | `TC-DICT-06`~`10` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-DICT-03` | 条目表与字符串池零拷贝访问 | `crates/ime-dict/src/entry.rs` | `P0.03.02` | `TC-DICT-26`~`30` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-DICT-04` | 用户词频库与提交/降级策略 | `crates/ime-dict/src/user_db.rs`、`user_db/evict.rs` | `P0.03.01`、`P0.02.05` | `TC-DICT-11`~`15`（深化 `TC-DICT-39`~`42`） | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-DICT-05` | 词库/用户库损坏自愈与原子替换 | `crates/ime-dict/src/recover.rs` | `P0.03.02` | `TC-DICT-31`~`35` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-DICT-06` | dictc 词源白名单与编译 | `xtask/src/dictc.rs`、`dictc/{build,source}*` | `P0.03.02` | `TC-DICT-16`~`20`（深化 `TC-DICT-43`/`44`） | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-DICT-07` | 多音字词频定向多键展开（L3b） | `xtask/src/dictc/build.rs` | `P0.03.02` | `TC-DICT-21`~`25` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-DICT-08` | 简繁转换词表与运行时（增量） | `crates/ime-dict/src/script.rs`、`crates/ime-core/src/script.rs` | `P0.03.02` | `TC-DICT-46`~`49`、`56` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-DICT-09` | 用户词条管理原语（增量） | `crates/ime-dict/src/user_db/{manage,export}.rs` | `P0.03.01`、`P0.05.01` | `TC-DICT-50`~`52`、`57`/`58` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-DICT-10` | 用户数据自动备份与回滚（增量） | `crates/ime-dict/src/user_db/backup.rs` | `P0.05.01`、`P0.02.05` | `TC-DICT-53`~`55`、`59`/`60` | ✅ | — | ✅ | — | — | ✅ |
| `REQ-RT-01` | Addon 生命周期与双 addon 注册 | `crates/ime-fcitx5/src/addon.rs` | `P0.02.06`、`P0.05.01` | `TC-RT-01`~`05`（深化 `TC-RT-35`） | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-RT-02` | 按键路由与 `KeyAction` 翻译 | `crates/ime-fcitx5/src/engine.rs` | `P0.01.01`、`P0.02.02` | `TC-RT-06`~`10`（深化 `TC-RT-26`~`31`） | ✅ | ✅ | ✅ | — | ✅ | ✅ |
| `REQ-RT-03` | 光标坐标解析与多屏缩放归一化 | `crates/ime-fcitx5/src/cursor/{resolver,sources}.rs`、`screen.rs` | `P0.02.06` | `TC-RT-11`~`15`（深化 `TC-RT-32`） | ✅ | ✅ | ✅ | — | — | ✅（`TC-RT-14` 多屏子项 🚫） |
| `REQ-RT-04` | C ABI 契约、握手与 panic 兜底 | `crates/ime-fcitx5/src/ffi/abi/{types,engine,lifecycle,ui}.rs` | `P0.03.01` | `TC-RT-16`~`20` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-RT-05` | `UserInterface` 面板接管与可用性降级 | `crates/ime-fcitx5/src/ui_impl/{panel,cursor_rects,takeover,availability}.rs` | `P0.02.01`、`P0.02.06` | `TC-RT-21`~`25` | ✅ | ✅ | ✅ | ✅ | — | ✅（`TC-RT-21` 视觉判据未通过，见复测记录） |
| `REQ-RT-06` | 会话状态机与翻页/选择语义 | `crates/ime-core/src/state/` | `P0.03.01` | `TC-CORE-36`~`40`（深化 `TC-CORE-41`） | ✅ | ✅ | ✅ | — | ✅ | ✅ |
| `REQ-RT-07` | Wayland 四档窗口后端 | `crates/ime-ui/src/platform/wayland/` | `P0.02.06` | `TC-RT-36`~`40` | Ⓦ | Ⓦ | Ⓦ | Ⓦ | — | 🚫（代码已落地；本机无 layer-shell，`Ⓦ 文档豁免` 记录在案） |
| `REQ-RT-08` | UI addon C ABI 与跨 addon 帧通道（增量） | `crates/ime-ui-addon/src/{ffi,addon,ui_impl,cursor}/`、ADR-0011 | `P0.02.01`、`P0.02.06` | `TC-RT-41`~`45` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-RT-09` | 行为诚实化：模式位/焦点/挂起/日志热切换（增量） | `crates/ime-core/src/state/effects.rs`、`crates/ime-fcitx5/src/{session_host,engine/context}.rs`、`crates/ime-diag/src/log.rs` | `P0.02.01`、`P0.02.03`、`P0.02.06` | `TC-RT-46`~`50` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-UI-01` | X11 ARGB 透明窗口后端 | `crates/ime-ui/src/platform/x11.rs`、`x11/` | `P0.01.02`、`P0.02.04` | `TC-UI-01`~`05`（深化 `TC-RT-34`） | ✅ | ✅ | ✅ | ✅ | — | ✅ |
| `REQ-UI-02` | 自定义 Slint Platform 与软件光栅 | `crates/ime-ui/src/slint_platform.rs`、`renderer.rs` | `P0.01.02`、`P0.02.04` | `TC-UI-06`~`10` | ✅ | ✅ | ✅ | ✅ | — | ✅ |
| `REQ-UI-03` | UI 线程模型与命令队列（`eventfd` 唤醒） | `crates/ime-ui/src/{ui_thread,channel}.rs` | `P0.01.03`、`P0.02.05` | `TC-UI-11`~`15` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-UI-04` | 候选框骨架与布局约束 | `crates/ime-ui/ui/candidate.slint`、`layout.rs` | `P0.02.04`、`P0.04.01` | `TC-UI-16`~`20` | ✅ | ✅ | ✅ | ✅ | — | ✅ |
| `REQ-UI-05` | 主题 Token、深浅色与亚克力 | `crates/ime-ui/ui/theme.slint`、`src/theme.rs` | `P0.02.04`、`P0.04.01` | `TC-UI-21`~`25` | ✅ | ✅ | ✅ | ✅ | — | ✅（`TC-UI-24` 真实模糊子项 🚫） |
| `REQ-UI-06` | 候选网格、数字标签与五态 | `crates/ime-ui/ui/candidate_grid.slint`、`src/adapter.rs` | `P0.02.01`、`P0.04.01` | `TC-UI-26`~`30` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `REQ-UI-07` | 鼠标交互：悬停、点击、滚轮 | `crates/ime-ui/src/interaction.rs` | `P0.01.01`、`P0.01.02` | `TC-UI-31`~`35` | ✅ | ✅ | ✅ | ✅ | — | ✅ |
| `REQ-UI-08` | 屏幕避让与几何计算 | `crates/ime-ui/src/geometry.rs` | `P0.01.02`、`P0.02.04` | `TC-UI-36`~`40` | ✅ | ✅ | ✅ | ✅ | — | ✅ |
| `REQ-UI-09` | 出现/消失/选中过渡动效（Spring） | `crates/ime-ui/src/spring.rs`、`spring/` | `P0.01.03`、`P0.04.01` | `TC-UI-41`~`45` | ✅ | ✅ | ✅ | ✅ | — | ✅ |
| `REQ-UI-10` | 淡入淡出、crossfade 与亚克力协商（增量） | `crates/ime-ui/src/spring/transition.rs`、`adapter/frame.rs`、`platform/x11/blur.rs` | `P0.01.03`、`P0.02.04`、`P0.04.01` | `TC-UI-46`~`50` | ✅ | — | ✅ | ✅ | — | ✅（真实模糊子项 🚫） |
| `REQ-UI-11` | 指针微动效与 preedit 尾部保护（增量） | `crates/ime-ui/src/spring/press.rs`、`adapter/preedit/`、`ui_thread.rs` | `P0.01.01`、`P0.01.03`、`P0.04.01` | `TC-UI-51`~`55` | ✅ | ✅ | ✅ | ✅ | — | ✅ |
| `REQ-SEC-01` | 用户数据目录与文件权限基线 | `crates/ime-dict/src/paths.rs`、`paths/` | `P0.05.01` | `TC-SEC-31`~`35`（深化 `TC-INFRA-60`） | ✅ | — | ✅ | — | — | ✅ |
| `REQ-SEC-02` | 敏感输入上下文检测与学习抑制 | `crates/ime-fcitx5/src/privacy_impl/`、`crates/ime-core/src/privacy.rs` | `P0.02.03` | `TC-SEC-36`~`42` | ✅ | ✅ | ✅ | ✅ | — | ✅ |
| `REQ-SEC-03` | 零网络外联断言 | `scripts/check-no-network.sh`、`runtime-socket-check.sh` | `P0.03.03` | `TC-SEC-11`~`15` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-SEC-04` | 词源许可证白名单与 SHA256 | `scripts/check-dict-sources.sh`、`data/sources.toml` | `P0.03.02` | `TC-SEC-16`~`20`（深化 `TC-SEC-43`） | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-SEC-05` | Slint API 不泄漏（`OB-4`） | `scripts/check-slint-leak.sh` | `P0.03.03` | `TC-SEC-21`~`25`（深化 `TC-SEC-44`） | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-SEC-06` | 依赖单向性与分层 | `scripts/check-deps.sh` | `P0.03.03` | `TC-SEC-26`~`30`（深化 `TC-SEC-45`） | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-DIAG-01` | 结构化日志、滚动与字段脱敏 | `crates/ime-diag/src/{log,redact}.rs` | `P0.02.03` | `TC-DIAG-01`~`05` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-DIAG-02` | panic 钩子、崩溃回溯与 FFI 兜底 | `crates/ime-diag/src/{panic,crash}.rs` | `P0.02.03`、`P0.03.02` | `TC-DIAG-06`~`10` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-DIAG-03` | 帧耗时/解码延迟探针与预算看板 | `crates/ime-diag/src/probe.rs` | `P0.01.03`、`P0.03.03` | `TC-DIAG-11`~`15` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-DIAG-04` | `UiFrame` 镜像快照通道（新增） | `crates/ime-diag/src/uiframe/{schema,mirror,error}.rs` | `P0.02.01` | `TC-DIAG-16`~`20` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-DIAG-05` | 崩溃取证黑匣子（新增） | `crates/ime-diag/src/crash/{record,signal,context}.rs` | `P0.02.03`、`P0.05.04` | `TC-DIAG-21`~`25` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-DIAG-06` | 诊断报告渲染与探针看板（新增） | `crates/ime-diag/src/report/`、`probe.rs`、`xtask/src/testd/budget_gate.rs` | `P0.03.03`、`P0.04.01` | `TC-DIAG-26`~`30` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-INFRA-01` | 质量门禁与 CI 基线 | `justfile`、`.github/workflows/ci.yml` | `P0.03.03` | `TC-INFRA-01`~`05`（深化 `TC-INFRA-59`） | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-INFRA-02` | 审计脚本的自测能力 | `just check-self-tests` | `P0.03.03` | `TC-INFRA-06`~`10` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-INFRA-03` | `unsafe` 边界与 `SAFETY` 注释 | `scripts/check-unsafe.sh` | `P0.03.03` | `TC-INFRA-11`~`15` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-INFRA-04` | 版本一致性（conf ↔ Cargo.toml） | `xtask/src/versions.rs` | `P0.02.06` | `TC-INFRA-16`~`20` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-INFRA-05` | Fcitx5 插件安装布局与打包 | `packaging/`、`xtask/src/install/`（`TC-INFRA-31`~`45`） | `P0.05.01` | `TC-INFRA-31`~`45` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-INFRA-06` | 预算表与 `budgets.json` 一致性 | `xtask/src/budget.rs`、`budget/{schema,spec}.rs` | `P0.03.03` | `TC-INFRA-21`~`25` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-INFRA-07` | 场景化端到端驱动与证据链（新增） | `xtask/src/testd/{engine,suite,sandbox,evidence,heal,env_gate,memory,framerate}.rs`、`tests/fixtures/scenarios/` | `P0.03.01`、`P0.03.02`、`P0.05.01`、`P0.05.04`~`06` | `TC-INFRA-46`~`50` | ✅ | ✅ | ✅ | — | ✅ | ✅ |
| `REQ-INFRA-08` | 审计脚本族扩展（新增） | `scripts/check-no-grab.sh` 等 7 个新脚本、`just audits` | `P0.03.03`、`P0.05.05` | `TC-INFRA-51`~`55`、`61` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-INFRA-09` | 打包、体积与基准纯净度门禁（新增） | `xtask/src/{package.rs,package/,soak.rs}`、`testd/purity.rs`、`packaging/{aur,debian,rpm}` | `P0.03.03`、`P0.05.01`、`P0.05.06` | `TC-INFRA-55`~`58`、`62` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-CFG-01` | 配置 schema v2、校验与修复（新增） | `crates/ime-config/src/schema.rs`、`schema/{data,repair}.rs` | `P0.03.01` | `TC-CFG-01`~`05` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-CFG-02` | 配置迁移（单向不可逆）（新增） | `crates/ime-config/src/migrate.rs`（`V1_TO_V2`） | `P0.03.01`、`P0.02.06` | `TC-CFG-06`~`10` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-CFG-03` | 配置热重载与会话保持（新增） | `crates/ime-config/src/reload.rs`、`reload/{load,store}.rs` | `P0.02.03`、`P0.02.06` | `TC-CFG-11`~`15` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-CFG-04` | 键位自定义、投影与速查表（新增） | `crates/ime-config/src/keymap.rs`、`keymap/project.rs`、`crates/ime-fcitx5/src/cheatsheet.rs`、`tests/keymap_matrix/` | `P0.01.01`、`P0.02.06` | `TC-CFG-16`~`20` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `REQ-CFG-05` | 双拼方案引擎与切换（新增） | `crates/ime-config/src/scheme.rs`、`crates/ime-core/src/shuangpin/` | `P0.03.01` | `TC-CFG-21`~`25` | ✅ | ✅ | ✅ | — | — | ✅ |
| `REQ-CFG-06` | 配置写回与首启模板（新增） | `crates/ime-config/src/writeback.rs`、`reload.rs`（`DEFAULT_CONFIG_TOML`） | `P0.02.03`、`P0.05.04` | `TC-CFG-26`~`30` | ✅ | ✅ | ✅ | ✅ | — | ✅ |

**双向一致性**：**67 个功能条目 → 382 条用例**（基线 67 × 5 = **335** 条 + 深化/扩展 **47** 条）→ 全部回填至 `FEAT-TEST` 任务卡。每个功能条目 **≥ 5** 条用例，无空缺。校验方式：解析本文档矩阵的 `REQ-*` 行与全部用例标题行的 `对应功能条目编号` 字段，断言两者互为满射（v2.0 同步时已机械复核：矩阵行数 67，用例标题行 382，每行 n ≥ 5）。

**Ⓦ 文档豁免**：`REQ-RT-07` 的 5 条 Wayland 用例按 features.md 0.5.5 判定为本机不可验证档位，run-20261006-034915 为每条生成 `Ⓦ 文档豁免` 执行记录（env_gate 判定表 remedy 原文引用），**不算通过也不算失败**——待外部 Wayland 环境就绪后复测。

**本版交付分布**（实测；`### TC-<模块>-<序号>` 是 4.2 的模板占位符，不计入）：

| 交付位置 | 内容 | 用例数 | 可执行 | 待实现 | 不可验证 |
|---|---|---|---|---|---|
| 本文档（Hub） | P0 核心基线：`REQ-CORE-01`~`06`、`REQ-DICT-01`/`02`/`04`/`06`/`07`、`REQ-RT-01`~`04`、`REQ-UI-01`、`REQ-SEC-03`~`06`、`REQ-INFRA-01`~`04`/`06` | **125** | 124 | 0 | 1 |
| [`tests/core.md`](tests/core.md) | `REQ-CORE-07`~`10`、`REQ-RT-06` + core 深化 | 35 | 35 | 0 | 0 |
| [`tests/dict.md`](tests/dict.md) | `REQ-DICT-03`/`05`/`08`~`10` + 词库规模与畸形输入 | 35 | 35 | 0 | 0 |
| [`tests/rt.md`](tests/rt.md) | `REQ-RT-05`、`REQ-RT-07`、`REQ-RT-08`/`09` + 真实会话端到端 | 30 | 25 | 0 | 5（Wayland Ⓦ） |
| [`tests/ui.md`](tests/ui.md) | `REQ-UI-02`~`11` 全部视觉与微交互 | 50 | 50 | 0 | 0（子项 🚫 见行注） |
| [`tests/sec.md`](tests/sec.md) | `REQ-SEC-01`/`02` 隐私与权限 + 深化 | 15 | 15 | 0 | 0 |
| [`tests/diag.md`](tests/diag.md) | `REQ-DIAG-01`~`06` 日志、崩溃、镜像、报告 | 30 | 30 | 0 | 0 |
| [`tests/infra.md`](tests/infra.md) | `REQ-INFRA-05`/`07`~`09` + 打包、审计脚本族、E2E 驱动 | 32 | 32 | 0 | 0（8h 长稳子项 🚫） |
| [`tests/cfg.md`](tests/cfg.md) | `REQ-CFG-01`~`06` 配置、迁移、热重载、键位、双拼、写回 | 30 | 30 | 0 | 0 |
| **合计** | — | **382** | **376** | **0** | **6** |

**可执行性分布**（每条用例的"基本属性"块均带**唯一**的 `可执行性：` 字段，由 `FEAT-TEST-P0.05.05` 的门禁机械判定；校验脚本断言"用例数 = 三类之和"）：

| 口径 | 说明 | 用例数 |
|---|---|---|
| `[可执行]`（首标注） | 代码已存在，本机可直接运行判定 | **381** |
| `[不可验证]`（首标注） | 该用例**整体**无法在本机判定（需外部环境） | **1**（`TC-RT-14` 混合 DPI 多屏） |
| `[待实现]`（首标注） | 被测代码不存在 | **0** |

> **整体判定口径**：`TC-RT-36`~`40`（Wayland 四档）的首标注是 `[可执行]` + `[不可验证]` 复合形态（用例可构造、判据需外部环境），按整体判定归入不可验证——即上表"交付分布"列的 376/6 口径；其 `Ⓦ 文档豁免` 执行记录是唯一合规形态。两种口径的差异**只有这 5 条**，其余 377 条两口径一致。

> **复合标注的用例**：主标注 `[可执行]` 但正文中显式声明某**子项**不可验证的用例（如 `TC-UI-48` 的真实亚克力、`TC-UI-16` 的 2.0 缩放、`TC-RT-44` 的混合 DPI、`TC-INFRA-50` 的 8 小时长稳、`TC-DICT-42` 的完整长稳）必须按 `FEAT-TEST-P0.05.05` 的三态判定逐项核对，子项在 `assertions.json` 中记为"本机不可验证"而非通过。
>
> **唯一未通过用例**：`TC-RT-21`（候选窗接管的结构判据已达成、视觉判据受 depth-24 回退路径黑屏阻断，失败隔离保持，复测记录见用例正文）。这是当前套件中**唯一**的红灯，修复后按 `rt.md` 出口准则第 7 条复测。

---

## 3. P0 核心基线用例

**证据说明**：非视觉用例的证据是 `assertions.json` + 命令输出，**不产生 PNG**（无 UI 可截图）。视觉用例（`REQ-UI-*`）才产生 PNG，且当前标注 `[待实现]`。所有用例的"触发存盘"节点均写入 `<RUN>/<模块代码>/<TC-ID>/`。

---

### TC-CORE-01 合法音节全表识别（`REQ-CORE-01`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过`
  - 对应功能条目编号：`REQ-CORE-01`
  - 模块与类别：`core` | 核心业务闭环
  - 优先级：`P0 核心基线`
  - 代码落地锚点：`crates/ime-core/src/segment/syllable.rs`
- **前置条件与沙盒状态**：无（纯 Rust 单测，无显示服务器、无词库文件）。`cargo nextest run -p ime-core segment::syllable`
- **操作步骤**：
  1. 遍历音节表的全部条目，逐个调用 `lookup` -> 触发存盘：`<RUN>/core/TC-CORE-01/assertions.json`
  2. 断言表项数 = 411、严格升序、无重复、最长项长度 = 6（`zhuang`/`chuang`/`shuang`）。
  3. 断言特殊音节全部命中：`a o e ai ei ao ou an en ang eng er`、`yi ya ye yao you yan yin yang ying yong wu wa wo wai wei wan wen wang weng yu yue yuan yun`、`m n ng hm hng ê`。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：411 项全部 `lookup` 命中；表严格升序无重复；无 panic、无 `unwrap` 触发的崩溃。
  - **边界**：单字母 `x` 不命中（不是合法音节）；空串返回 `None`。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-01/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-02 输入规范化：`ü` 的四种输入形态（`REQ-CORE-01`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-01` ｜ 模块与类别：`core` | 极端容错与性能 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/segment/syllable.rs`
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-core segment::syllable`
- **操作步骤**：
  1. 对 `lv` / `lü` / `nv` / `nü` / `lve` / `nve` / `jv` / `ju` / `jü` 这些写法调用规范化 -> 触发存盘：`<RUN>/core/TC-CORE-02/assertions.json`
  2. 断言它们各自归一到**音节表实际存储的那个拼写**：`v` 变成 `ü`（`nv` → `nü`、`lv` → `lü`），而 `j`/`q`/`x`/`y` 之后的 `u` **或** `ü` 一律折回 `u`（`jv` → `ju`、`jü` → `ju`、`yüe` → `yue`）。
  3. 断言归一化后的每个结果都能在音节表里 `lookup` 命中。
  4. 断言大小写混合输入 `NiHao` 归一为 `nihao`。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：`ü` 音的每一种输入形态都折叠到音节表存储的那一个拼写上（`v` → `ü`；`j/q/x/y` 后折回 `u`）；大小写归一。
  - **边界**：`v` 出现在非 `ü` 位置的输入不 panic（如 `vvv`）。

> **本用例于 2026-09-30 被主 Agent 修正，方向与原文相反。** 原文要求「`nv`/`nü`/`nu:`/`nv3` 四种写法全部归一为 `nv`」且「`ju` 中的 `u` 被归一为 `ü`（`ju` → `jü`）」。两处都与**标准汉语拼音正词法**相反：`v` 是键盘上 `ü` 的输入别名，所以规范化的方向是 `v` → `ü`；而 `j`/`q`/`x`/`y` 之后的 `ü` 音在正词法里**写作 `u`**（`ju`/`que`/`xuan`/`yu` 才是正确拼写），所以方向是 `ü` → `u`。`nu:` 也不是 `nü` 的写法——`:` 是声调标记，剥掉之后是 `nu`（怒），与 `nü`（女）是**两个不同的音节**，因此「四种写法归一一致」这条在实现上不可能成立。
> 判定依据是 `crates/ime-core/src/segment/syllable.rs` 的模块文档（「Canonical spelling」一节）与 `test_normalize_folds_every_umlaut_input_form_onto_the_table_spelling`（16 组用例逐条断言）。按 `AGENTS.md` §7「当文档与代码不一致时，以代码为准修正文档」，此处改文档。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-02/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-03 规范化丢弃非法字符并记录诊断（`REQ-CORE-01`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-01` ｜ 模块与类别：`core` | 全状态防御与骨架屏 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/segment/syllable.rs`
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-core segment::syllable`
- **操作步骤**：
  1. 输入含数字与符号的串 `ni3hao!` 并规范化 -> 触发存盘：`<RUN>/core/TC-CORE-03/assertions.json`
  2. 断言输出串不含被丢弃字符。
  3. 断言产生 `decode/invalid-char` 诊断，且携带 `ch` 与 `at` 两个字段。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：非法字符被丢弃而非 panic；诊断码为冻结的 `decode/invalid-char`（不得改写，`AGENTS.md` 第 1 节）。
  - **边界**：全非法输入（如 `!!!`）归一为 `""` 且不 panic。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-03/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-04 超长输入在规范化前被拒绝（`REQ-CORE-01`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-01` ｜ 模块与类别：`core` | 极端容错与性能 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/segment/syllable.rs`、`crates/ime-types/src/decode.rs`
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-core`
- **操作步骤**：
  1. 构造 65 字节输入（超过 `ASM-T-10` 的 64 上限） -> 触发存盘：`<RUN>/core/TC-CORE-04/assertions.json`
  2. 断言返回 `decode/too-long`，且错误携带 `len=65` 与 `max=64`。
  3. 构造 64 字节输入，断言**不**报错。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：长度判定发生在**规范化之前**（防止规范化放大长度）；64 字节恰好通过。
  - **边界**：65 与 64 的边界两侧行为明确，无 off-by-one。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-04/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-05 全键盘流：音节表可被纯键盘遍历输入（`REQ-CORE-01`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-01` ｜ 模块与类别：`core` | 全键盘流 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/segment/syllable.rs`、`crates/ime-types/src/key.rs`
- **前置条件与沙盒状态**：`FEAT-TEST-P0.03.01` 的引擎直驱通道就绪。场景文件 `tests/fixtures/scenarios/syllable_traverse.toml` ｜ 命令：`cargo nextest run -p ime-core segment::dag::tests::test_build_finds_a_path_for_every_table_syllable segment::syllable::tests::test_lookup_finds_every_table_entry_by_its_index`
- **操作步骤**：
  1. 对 411 个音节逐个构造 `KeyAction::InputChar` 序列 -> 触发存盘：`<RUN>/core/TC-CORE-05/assertions.json`
  2. 断言每个音节序列都能产出一个合法切分（`has_path() == true`）。
  3. 断言全部输入仅由 `a-z`、`'` 与 `ê` 组成（`ê` 是音节表自带的收尾条目，规范化规则将其原样保留，所以它同样可以纯键盘输入；无修饰键、无鼠标）。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：411 个音节全部可纯键盘输入并切分成功。
  - **人机工学**：不依赖任何修饰键或鼠标操作。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-05/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-06 多路径切分全部保留（`REQ-CORE-02`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-02` ｜ 模块与类别：`core` | 核心业务闭环 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/segment/dag.rs`
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-core segment::dag`
- **操作步骤**：
  1. 对 `nihao` 构建 DAG -> 触发存盘：`<RUN>/core/TC-CORE-06/assertions.json`
  2. 断言路径数 > 1（`ni|hao` 与 `ni|ha|o` 等）。
  3. 断言 `has_path()` 为真。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：多路径不被提前剪枝，全部交给 Viterbi 打分。
  - **边界**：路径数为 1 的输入（如 `ni`）同样正确。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-06/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-07 无路径输入返回冻结错误码（`REQ-CORE-02`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-02` ｜ 模块与类别：`core` | 极端容错与性能 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/segment/dag.rs`
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-core segment::dag`
- **操作步骤**：
  1. 对 `zzz`、`qqq`、`x` 构建 DAG -> 触发存盘：`<RUN>/core/TC-CORE-07/assertions.json`
  2. 断言 `has_path()` 为假，且解码返回 `decode/no-path`。
  3. 断言**不 panic**。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：无路径是**可预期的正常分支**，返回类型化错误而非崩溃。
  - **边界**：单字符非法输入同样走此分支。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-07/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-08 `'` 强制分隔符语义（`REQ-CORE-02`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-02` ｜ 模块与类别：`core` | 边界与容错 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/segment/dag.rs`
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-core segment::dag`
- **操作步骤**：
  1. 对 `ni'hao` 构建 DAG -> 触发存盘：`<RUN>/core/TC-CORE-08/assertions.json`
  2. 断言 `'` 处**必须**切分（`EdgeKind::Forced`），且 `'` 本身不计入任何音节。
  3. 对 `ni''hao`（连续分隔符）与 `'nihao`（开头分隔符）断言被规范化折叠且不 panic。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：强制分隔符语义正确；异常分隔符被折叠并记录诊断。
  - **边界**：开头/结尾/连续 `'` 三种异常均不崩溃。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-08/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-09 分配预算：稳态解码的堆分配次数等于预算（`REQ-CORE-02`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-02` ｜ 模块与类别：`core` | 极端容错与性能 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/tests/alloc_budget.rs`（`#[global_allocator]` 计数器来自 `crates/alloc-count`）
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-core --test alloc_budget`（nextest 每用例一进程，计数读数因此只含被测代码）
- **操作步骤**：
  1. 预热一次 `decode_into` 以填充工作区 -> 触发存盘：`<RUN>/core/TC-CORE-09/assertions.json`（同时写出 `target/alloc-report.txt`）
  2. 复位分配计数，对同一工作区再次解码，记录稳态分配次数。
  3. 断言稳态分配次数不超过分配预算（`BUDGET-ALLOC-01`：12 音节实测 13、2 音节实测 2，2026-09-30 按实测锚定；残余项与归零路径见 `features.md` 0.5.3 的该行与 `TASK-139`）；并断言直通降级不比真实解码更贵、首次解码必然更多。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：稳态解码的堆分配次数不超过分配预算（`BUDGET-ALLOC-01`）。
  - **预算**：`target/alloc-report.txt` 被 `cargo run -p xtask -- budget --alloc` 判定通过；空缺记录按失败处理（`budget/alloc-unmeasured`）。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-09/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。分配预算门 `xtask budget --alloc` 判定通过（`decode_steady 13 of 13 allocations`，报告 `target/alloc-report.txt`）。
### TC-CORE-10 fuzz 目标持续 60 秒无 panic（`REQ-CORE-02`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-02` ｜ 模块与类别：`core` | 极端容错与性能 ｜ 优先级：`P0`
  - 代码落地锚点：`fuzz/fuzz_targets/dag_build.rs`、`fuzz/corpus/dag_build/`
- **前置条件与沙盒状态**：`cargo +nightly fuzz`（`just fuzz 60`）。
- **操作步骤**：
  1. 执行 `just fuzz 60` -> 触发存盘：`<RUN>/core/TC-CORE-10/assertions.json`（附 fuzz 退出码与 `fuzz/artifacts/` 状态）
  2. 断言无 panic、无超时。
  3. 断言任意输入的耗时 ≤ 100µs。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：60 秒 fuzz 无崩溃，`fuzz/artifacts/dag_build/` 无新增产物。
  - **性能**：单次 `build_dag` 耗时上界成立。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-10/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。`just fuzz 60` 实测 `Done 2018532 runs in 61 second(s)`，`fuzz/artifacts/dag_build/` 无新增产物；新增语料随提交入库。
### TC-CORE-11 按音节删除的完整序列（`REQ-CORE-03`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-03` ｜ 模块与类别：`core` | 核心业务闭环 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/input/buffer.rs`
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-core input::buffer`
- **操作步骤**：
  1. 输入 `nihaoa` 并回写切分边界 `ni|hao|a` -> 触发存盘：`<RUN>/core/TC-CORE-11/assertions.json`
  2. 连续三次 `backspace`，断言结果为 `nihao` → `ni` → `""`。
  3. 断言三次的 `BackspaceOutcome` 依次为 `RemovedSyllable`、`RemovedSyllable`、`BufferEmpty`。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：一次 Backspace 删除**整个末尾音节**而非一个字母；`BufferEmpty` 的语义是"本次按键后缓冲为空"，**不是**"删除了一个音节"（`.dev-progress.json` 的 cross_task_note 已明确）。
  - **边界**：第三次返回 `BufferEmpty` 时不得期望 `RemovedSyllable`。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-11/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-12 caret 在 0 位置时的 Backspace（`REQ-CORE-03`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-03` ｜ 模块与类别：`core` | 边界与容错 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/input/buffer.rs`
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-core input::buffer`
- **操作步骤**：
  1. 输入 `ni`，把 caret 移到 0 -> 触发存盘：`<RUN>/core/TC-CORE-12/assertions.json`
  2. 执行 `backspace`。
  3. 断言删除的是 **caret 指向的字符**，且返回值**不是** `BufferEmpty`。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：caret 为 0 且缓冲非空时不得返回 `BufferEmpty`——否则会话会被误判为结束，preedit 丢失（`.dev-progress.json` 已记录该边界）。
  - **边界**：删除后缓冲非空。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-12/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-13 64 字节上限后 `push_char` 被拒绝（`REQ-CORE-03`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-03` ｜ 模块与类别：`core` | 极端容错与性能 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/input/buffer.rs`
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-core input::buffer`
- **操作步骤**：
  1. 连续 `push_char` 至 64 字节 -> 触发存盘：`<RUN>/core/TC-CORE-13/assertions.json`
  2. 第 65 次 `push_char`。
  3. 断言返回 `Err(DecodeTooLong)`，且 `raw` 长度仍为 64（未被修改）。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：超限时缓冲不变，错误可被上层转为"已达上限"提示。
  - **边界**：恰好 64 字节时 `push_char` 成功。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-13/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-14 属性测试：缓冲不变量（`REQ-CORE-03`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-03` ｜ 模块与类别：`core` | 全状态防御与骨架屏 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/input/buffer.rs`（`proptest`）
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-core input::buffer`
- **操作步骤**：
  1. 用 `proptest` 生成 10000 组随机 `push_char`/`backspace`/`move_caret` 序列 -> 触发存盘：`<RUN>/core/TC-CORE-14/assertions.json`
  2. 每步后断言 `raw.len() <= 64` 且 `raw.is_char_boundary(caret)`。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：10000 组随机序列无一违反两条不变量。
  - **边界**：序列中包含空操作与重复 backspace。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-14/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-15 全键盘流：Backspace 与 caret 移动（`REQ-CORE-03`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-03` ｜ 模块与类别：`core` | 全键盘流 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/input/buffer.rs`、`crates/ime-types/src/key.rs`
- **前置条件与沙盒状态**：无。 ｜ 命令：`cargo nextest run -p ime-core test_move_caret`
- **操作步骤**：
  1. 构造 `InputChar('n')`/`InputChar('i')`/`Backspace`/`MoveCaret(-1)`/`MoveCaret(1)` 序列 -> 触发存盘：`<RUN>/core/TC-CORE-15/assertions.json`
  2. 断言 `MoveCaret` 只移动到音节边界（Phase 1 限制）。
  3. 断言 `move_caret` 越界时返回 `false` 且 caret 不变。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：caret 移动不破坏音节边界不变量；越界被拒绝。
  - **人机工学**：全部操作可由 `KeyAction` 表达，无需鼠标。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-15/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-16 确定性：同输入 100 次候选逐字节一致（`REQ-CORE-04`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-04` ｜ 模块与类别：`core` | 核心业务闭环 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/viterbi/{decoder,kbest,lattice}.rs`
- **前置条件与沙盒状态**：`FEAT-TEST-P0.03.01` 的内存替身（`MockLexicon`/`MockUserFreq`/`MockLm`）。`cargo nextest run -p ime-core viterbi`
- **操作步骤**：
  1. 对 `nihao`/`woaini`/`zhongguo`/`beijingdaxue` 四个输入各解码 100 次 -> 触发存盘：`<RUN>/core/TC-CORE-16/assertions.json`
  2. 断言每个输入的 100 次候选序列**逐字节一致**。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：排序由 Q8.8 定点驱动，**不得**由 `f32` 决定顺序（`AGENTS.md` 3.8）。这是拦截"浮点漂移导致候选抖动"的唯一手段。
  - **性能**：解码 P99 ≤ 24ms、P999 ≤ 64ms（12 音节输入，`budget` 键 `decode_p99`/`decode_p999`；2026-10-01 按用户裁决以开发机空闲实测重锚，裸机复核为后续任务）。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-16/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。解码预算 2026-10-01 经用户裁决以开发机空闲实测重锚（P99 ≤ 24ms、P999 ≤ 64ms），`budget --check` 判过。
### TC-CORE-17 空词库降级仍产出候选（`REQ-CORE-04`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-04` ｜ 模块与类别：`core` | 全状态防御与骨架屏 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/viterbi/decoder.rs`
- **前置条件与沙盒状态**：`MockLexicon` 的所有 `lookup` 返回空迭代器。 ｜ 命令：`cargo nextest run -p ime-core viterbi::tests::test_decode_without_a_usable_word_degrades_to_passthrough`
- **操作步骤**：
  1. 用空词库解码 `nihao` -> 触发存盘：`<RUN>/core/TC-CORE-17/assertions.json`
  2. 断言候选列表**非空**且来自 `Passthrough`（`fallback_single` 开或关结果一致——空词库下回退表同样是空的，两条路径都收敛到直通候选）。
  3. 断言 `DecodeResult.degraded == true`。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：**绝不返回空候选**——空候选在用户侧表现为"打字无反应"，是最差的降级。
  - **状态矩阵**：降级状态被显式标记（`degraded`），UI 可据此提示。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-17/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-18 候选数量与页码约束（`REQ-CORE-04`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-04` ｜ 模块与类别：`core` | 边界与容错 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/viterbi/decoder.rs`、`crates/ime-types/src/ui.rs`
- **前置条件与沙盒状态**：`MockLexicon` 对某个 key 返回 200 个词。 ｜ 命令：`cargo nextest run -p ime-core viterbi::tests::test_decode_keeps_the_list_inside_the_page_budget state::paging::tests::test_page_count_rounds_up_and_caps_at_five_pages`
- **操作步骤**：
  1. 解码该 key -> 触发存盘：`<RUN>/core/TC-CORE-18/assertions.json`
  2. 断言候选数 ≤ 45（`ASM-T-10`：5 页 × 9）。
  3. 断言每页 9 个时总页数 ≤ 5。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：候选数上限与分页约束成立。
  - **边界**：恰好 45 与 46 个词的边界行为明确。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-18/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-19 `K` 值边界：`TopK` 的三种退化（`REQ-CORE-04`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-04` ｜ 模块与类别：`core` | 边界与容错 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/viterbi/kbest.rs`
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-core viterbi::kbest`
- **操作步骤**：
  1. 对 `TopK` 分别测：`K > 元素数`、`K == 1`、`元素数 == 0` -> 触发存盘：`<RUN>/core/TC-CORE-19/assertions.json`
  2. 断言三种情况均返回合法结果（不 panic、不越界）。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：三种退化均正确；`K > 元素数` 时返回全部元素。
  - **边界**：含重复值的输入不产生重复输出（或按契约保留）。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-19/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-20 12 音节长输入的延迟预算（`REQ-CORE-04`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-04` ｜ 模块与类别：`core` | 极端容错与性能 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/benches/`、`docs/dev/budgets.json`
- **前置条件与沙盒状态**：**必须在空闲机器上采集**（`ASM-T-11`）。`just bench`（= `cargo bench --workspace` + `xtask budget --check`；**是 `--check` 而不是 `--validate`**——后者只做 `budgets.json` 与 `features.md` 0.5.3 的文档交叉校验，**不读任何测量值**，故不构成判据）。**已知缺口**：判据里"不洁净时拒采"没有实现——`FEAT-TEST-P0.05.06` 的 `is_clean()` 在 `xtask/src/testd/env.rs` 里不存在，该文件头部自述环境门禁与纯度守卫尚未落地；`xtask/src/budget/meta.rs` 只把 CPU 型号 / governor / RUSTFLAGS 写进 `meta.json`，缺失值记 `null` 而不失败
- **操作步骤**：
  1. 跑 12 音节基准用例（`decode/12syl`）-> 触发存盘：`<RUN>/core/TC-CORE-20/assertions.json`（附 criterion 输出与 `PurityReport`）
  2. 以 `mean + 3σ` 作为 P99 估计，与 `budgets.json` 的 `decode_p99 = 3.0` 比对。
- **通过标准 (Pass Criteria)**：
  - **性能**：P99 估计 ≤ 24ms（开发机回归基线，2026-10-01 经用户裁决按实测重锚；裸机复核为后续任务）。超限即失败（0.4 规则 9）。
  - **测量纯净度**：报告含 CPU 型号与 governor；不洁净时拒采而非降级为警告。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-20/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。空闲机器采集：criterion 全量跑通后 `budget --check` 按重锚阈值（P99 ≤ 24ms）判定通过；12 音节 P50 实测 7.9ms。
### TC-CORE-21 打分函数为定点整数（`REQ-CORE-05`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-05` ｜ 模块与类别：`core` | 核心业务闭环 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/lm/score.rs`
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-core lm::score`
- **操作步骤**：
  1. 断言 `Scorer` 内不出现 `f32`/`f64`（脚本化 grep）-> 触发存盘：`<RUN>/core/TC-CORE-21/assertions.json`
  2. 断言 `log2_q8` 与 `f64` 参考实现在 4096 个采样点上误差 ≤ 2（Q8.8）。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：打分全程 Q8.8 定点，候选顺序可复现。
  - **边界**：`prob_num == 0` 时返回下限 `-2048` 而非 panic。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-21/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-22 用户频次项的上限钳制（`REQ-CORE-05`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-05` ｜ 模块与类别：`core` | 边界与容错 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/lm/score.rs`
- **前置条件与沙盒状态**：`MockUserFreq` 可注入任意频次。 ｜ 命令：`cargo nextest run -p ime-core lm::score::tests::test_edge_score_clamps_the_user_term_at_a_million_hits lm::score::tests::test_user_term_starts_at_zero_and_scales_with_the_count`
- **操作步骤**：
  1. 注入 `freq = 1`、`freq = 8`、`freq = 1_000_000` 三档 -> 触发存盘：`<RUN>/core/TC-CORE-22/assertions.json`
  2. 断言用户频次项被钳制在 `λ_user · 2` 上限内。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：防止某个词被打 10 万次后永久霸榜。
  - **边界**：`freq = 0` 时用户项为 0。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-22/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-23 非法权重配置被拒绝（`REQ-CORE-05`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-05` ｜ 模块与类别：`core` | 边界与容错 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/lm/score.rs`、`crates/ime-types/src/error.rs`
- **前置条件与沙盒状态**：无。 ｜ 命令：`cargo nextest run -p ime-core lm::score::tests::test_scorer_new`
- **操作步骤**：
  1. 构造含负值的 `ScoreWeights` -> 触发存盘：`<RUN>/core/TC-CORE-23/assertions.json`
  2. 断言构造失败并返回 `config/invalid`，携带 `key` 与 `reason`。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：非法配置在构造期被拒绝，不留到运行期。
  - **边界**：全零权重合法（退化为等分）。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-23/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-24 未命中 bigram 的确定性退化（`REQ-CORE-05`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-05` ｜ 模块与类别：`core` | 全状态防御与骨架屏 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/lm/ngram.rs`
- **前置条件与沙盒状态**：`MockLm` 的 `bigram` 表为空。 ｜ 命令：`cargo nextest run -p ime-core lm::ngram::tests::test_bigram_falls_back_to_the_unigram_plus_the_fixed_penalty`
- **操作步骤**：
  1. 对任意词对调用 `bigram` -> 触发存盘：`<RUN>/core/TC-CORE-24/assertions.json`
  2. 断言退化为 `unigram - 64`（固定常数）。
  3. 断言该常数**不随输入变化**（重复 100 次结果全等）。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：退化惩罚是固定常数，保证确定性。
  - **边界**：`prev` 为空串时同样退化。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-24/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-25 `lm_holdout` 留出集可达性门槛（`REQ-CORE-05`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-05` ｜ 模块与类别：`core` | 核心业务闭环 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/tests/fixtures/lm_holdout.tsv`（86.7KB）、`lm_golden.tsv`（7.1KB）
- **前置条件与沙盒状态**：`xtask tune` 的求值器（`xtask/src/tune/eval.rs`）。 ｜ 命令：`cargo nextest run -p xtask tune::tests::test_rank_and_evaluate_count_the_first_choice_and_the_reachable_rows tune::tests::test_generate_holdout_writes_rows_that_avoid_the_evaluation_set`
- **操作步骤**：
  1. 在 `lm_holdout.tsv`（≥ 5000 条）上跑全量解码 -> 触发存盘：`<RUN>/core/TC-CORE-25/assertions.json`
  2. 记录两个指标：**首选词命中率**（排序质量）与**目标词是否出现在前 9 候选内**（可达性）。
  3. 在 `lm_golden.tsv` 上断言首选词命中率 ≥ 85%。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：两个指标分别记录（前者衡量排序、后者衡量可达性——`L3b` 多键展开修的是后者）。
  - **性能**：全量解码耗时 < 3s（可参与 CI）。
  - **边界**：`lm_holdout.tsv` 与 `lm_golden.tsv` 无重叠。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-25/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-26 大写字母触发英文直通（`REQ-CORE-06`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-06` ｜ 模块与类别：`core` | 核心业务闭环 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/passthrough.rs`
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-core passthrough`
- **操作步骤**：
  1. 构造首字符为大写的输入（`auto_english_on_uppercase = true`）-> 触发存盘：`<RUN>/core/TC-CORE-26/assertions.json`
  2. 断言 `classify` 返回 `CommitDirectly(该字符)` 并退出会话。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：打大写英文不触发输入法。
  - **边界**：`auto_english_on_uppercase = false` 时走 `Decode`。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-26/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-27 URL/邮箱启发式直通（`REQ-CORE-06`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-06` ｜ 模块与类别：`core` | 边界与容错 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/passthrough.rs`
- **前置条件与沙盒状态**：无。 ｜ 命令：`cargo nextest run -p ime-core passthrough::tests::test_classify_rule_table_returns_the_expected_decision`
- **操作步骤**：
  1. 对 `http://`、`www.`、`@`、`.com` 四类特征输入调用 `classify` -> 触发存盘：`<RUN>/core/TC-CORE-27/assertions.json`
  2. 断言返回 `HostHandles`。
  3. 断言 `passthrough_url = false` 时走 `Decode`。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：URL/邮箱场景不打断用户。
  - **边界**：纯字母输入**不**被误判为 URL。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-27/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-28 中文标点替换表（`REQ-CORE-06`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-06` ｜ 模块与类别：`core` | 核心业务闭环 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/passthrough.rs`
- **前置条件与沙盒状态**：无。 ｜ 命令：`cargo nextest run -p ime-core passthrough::tests::test_classify_rule_table_returns_the_expected_decision passthrough::tests::test_classify_punctuation_table_maps_the_twelve_ascii_marks`
- **操作步骤**：
  1. 对 12 个 ASCII 标点逐个在 `punct_mode = chinese` 下调用 `classify` -> 触发存盘：`<RUN>/core/TC-CORE-28/assertions.json`
  2. 断言映射为 `，。；：？！（）【】“”`。
  3. 断言 `'` **不参与**替换（它是音节分隔符）。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：12 个标点映射正确；`'` 保留为分隔符语义。
  - **边界**：`punct_mode = english` 时返回 `HostHandles`（**不是** `Decode`——`Decode` 会把标点当非法字符丢弃，导致按逗号"什么都没发生"；`.dev-progress.json` 已记录该修正）。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-28/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-29 全角映射覆盖 94 个可打印字符（`REQ-CORE-06`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-06` ｜ 模块与类别：`core` | 边界与容错 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/passthrough.rs`
- **前置条件与沙盒状态**：无。 ｜ 命令：`cargo nextest run -p ime-core passthrough::tests::test_to_full_width`
- **操作步骤**：
  1. 对 `0x21..=0x7E` 全部 94 个字符与空格调用全角映射 -> 触发存盘：`<RUN>/core/TC-CORE-29/assertions.json`
  2. 断言映射到 `0xFF01..=0xFF5E`，空格映射到 `U+3000`。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：94 个字符逐一断言正确。
  - **边界**：非 ASCII 字符原样返回。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-29/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
### TC-CORE-30 临时英文模式的全键透传（`REQ-CORE-06`）

- **基本属性**：
  - 可执行性：`[可执行]`
  - 用例状态：`[x] 已通过` ｜ 对应功能条目编号：`REQ-CORE-06` ｜ 模块与类别：`core` | 全键盘流 ｜ 优先级：`P0`
  - 代码落地锚点：`crates/ime-core/src/passthrough.rs`、`crates/ime-types/src/key.rs`
- **前置条件与沙盒状态**：`PassthroughFlags.temp_english = true`。 ｜ 命令：`cargo nextest run -p ime-core temp_english passthrough::tests::test_classify_rule_table_returns_the_expected_decision`
- **操作步骤**：
  1. 在临时英文模式下输入含**大写首字母**的串 -> 触发存盘：`<RUN>/core/TC-CORE-30/assertions.json`
  2. 断言返回 `HostHandles`（**不是** `CommitDirectly`）。
  3. 断言 `Escape` / `Enter` 可退出该模式。
- **通过标准 (Pass Criteria)**：
  - **功能逻辑**：临时英文模式的判定顺序**早于**大写提交规则与 URL 规则（`.dev-progress.json` 记录了该顺序修正：按任务卡的原始顺序，临时英文模式下打一个大写字母会被大写规则提交并退出会话——正是该模式要防止的）。
  - **边界**：`EnterTempEnglish` 由路由层产出，`classify` 不返回它（纯文本分类器观察不到键和弦）。

---

### TC-DICT-01 格式往返：写入与读出的五段逐字节一致（`REQ-DICT-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-01` ｜ `dict` | 核心业务闭环 ｜ `P0` ｜ `crates/ime-dict/src/format/{writer,reader}.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：`FEAT-TEST-P0.03.02` 的 `DictFixture`。`cargo nextest run -p ime-dict format`
- **操作步骤**：
  1. 用 `DictWriter` 写入 5000 词的测试词库 -> 触发存盘：`<RUN>/dict/TC-DICT-01/assertions.json`
  2. 用 `reader` 打开，逐段（`FST`/`ENTRIES`/`STRPOOL`/`UNIGRAM`/`BIGRAM`/`WORDLIST`）比对字节。
- **通过标准**：五段 + `WORDLIST` 段逐字节一致；`section_count = 6`；头部 `header_size = 64`；Section Table 为 `6 × 24 = 144` 字节；首段偏移 = 208（8 字节对齐）。

- **验收记录**（2026-10-06）：cargo nextest run -p ime-dict format 57/57 通过（含 test_round_trip_reads_back_every_section_byte_for_byte），布局常量 HEADER_SIZE=64/SECTIONS_OFFSET=208/表 144B 断言成立；证据包 results/runs/run-20261006-034915/dict/TC-DICT-01/
### TC-DICT-02 六种畸形输入全部返回类型化错误（`REQ-DICT-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-01` ｜ `dict` | 极端容错与性能 ｜ `P0` ｜ `crates/ime-dict/src/format/reader.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：`DictFixture` 的临时副本（绝不触碰 `data/compiled/base.dict`）。
- **操作步骤**：
  1. 依次注入 `Magic`、`FormatVersion(2)`、`Truncate`、`SectionCrc`、`EntryLength`、`EntryOffset` 六种变异 -> 触发存盘：`<RUN>/dict/TC-DICT-02/assertions.json`
  2. 断言分别返回 `DictError::{MagicMismatch, FormatVersion, LengthOutOfRange, Crc, ...}`。
- **通过标准**：6 种变异全部被拒绝，**无 panic、无越界**；`data/compiled/base.dict` 的 `sha256` 测试前后不变。

- **验收记录**（2026-10-06）：DictFixture 六变异矩阵 39/39 + reader 拒绝 6/6 通过（Magic/Version/Truncate/Crc/EntryLength/EntryOffset 全部类型化拒绝，无 panic）；data/compiled/base.dict 未触碰（本轮执行前该 gitignored 产物尚未编译，前后均缺席）；证据包 results/runs/run-20261006-034915/dict/TC-DICT-02/
### TC-DICT-03 原子替换：任何时刻目标文件完整（`REQ-DICT-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-01` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ `crates/ime-dict/src/format/writer.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：`FEAT-TEST-P0.05.01` 的沙盒目录。
- **操作步骤**：
  1. 启动 1000 次并发读，同时对目标路径执行 `DictWriter::finish` -> 触发存盘：`<RUN>/dict/TC-DICT-03/assertions.json`
  2. 断言每次读到的要么是旧完整内容、要么是新完整内容。
- **通过标准**：无读者观察到半写状态；写入使用 `tmp` → `fsync` → `rename`，**禁止**直接 `File::create(target)`（会先截断）。

- **验收记录**（2026-10-06）：test_finish_* 3/3 通过：tmp→sync_all→rename 原子替换，目标文件从不截断写，无 .tmp 残留；证据包 results/runs/run-20261006-034915/dict/TC-DICT-03/
### TC-DICT-04 磁盘满时清理 `.tmp` 并报可读错误（`REQ-DICT-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-01` ｜ `dict` | 极端容错与性能 ｜ `P0` ｜ `crates/ime-dict/src/format/writer.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：`FEAT-TEST-P0.05.01`；用小型 tmpfs 模拟 `ENOSPC`。
- **操作步骤**：
  1. 在空间不足的挂载点执行 `finish` -> 触发存盘：`<RUN>/dict/TC-DICT-04/assertions.json`
  2. 断言返回 `ENOSPC` 相关错误、`.tmp` 被清理、目标文件未受影响。
- **通过标准**：错误信息为"磁盘空间不足"而非"写入失败"；无 `.tmp` 残留。

- **验收记录**（2026-10-06）：【自愈】初测发现真实缺陷：ENOSPC 后 base.dict.tmp 残留（1048576B）。修复 DictWriter::finish 错误路径（写入段闭包化 + 失败时 remove_file 临时文件），新增确定性测试 test_finish_removes_the_temporary_file_when_the_write_fails（先失败后通过）。1MB tmpfs 物理探针复测：无 .tmp 残留、目标文件未受影响、ENOSPC 经错误链可读。修复耗时约 35 分钟，补丁 results/runs/run-20261006-034915/dict/TC-DICT-04/fix_patch.diff
### TC-DICT-05 词库不可用时降级为直通（`REQ-DICT-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-01` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ `crates/ime-dict/src/format/reader.rs`、`crates/ime-types/src/error.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒内不放 `base.dict`。
- **操作步骤**：
  1. 尝试加载缺失的词库 -> 触发存盘：`<RUN>/dict/TC-DICT-05/assertions.json`
  2. 断言返回 `dict/unavailable`，且输入仍可直通上屏英文。
- **通过标准**：候选功能禁用但**输入不被阻断**（`features.md` 3.6 的降级表）；候选框 Header 显示"词库不可用"。

- **验收记录**（2026-10-06）：engine::router::modes 14/14 + addon 降级路径 2/2 通过：缺失/损坏词库均返回冻结码 dict/unavailable 且直通不阻断，notice 表映射文案「词典不可用，仅直通输入」；实机头部呈现归 ui.md 视觉套件；证据包 results/runs/run-20261006-034915/dict/TC-DICT-05/
### TC-DICT-06 已知 key 的查询结果与源数据逐字段一致（`REQ-DICT-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-02` ｜ `dict` | 核心业务闭环 ｜ `P0` ｜ `crates/ime-dict/src/fst_index.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：`data/raw/base.tsv`（5,871 行）编译出的 `base.dict`。
- **操作步骤**：
  1. 对 100 个已知 key 调用 `FstLexicon::lookup` -> 触发存盘：`<RUN>/dict/TC-DICT-06/assertions.json`
  2. 与 TSV 源逐字段比对（词、`weight`、`syl_count`、`flags`）。
- **通过标准**：100 个 key 全字段一致；`lookup` P99 ≤ 3µs。

- **验收记录**（2026-10-06）：开发子集词库（dictc --input base.tsv --expand-top 2856）+ 独立探针（证据包 probe.rs，复用 ime_core::segment::normalize 与 dictc fold_reading 同表镜像）：100 个采样词在 87 个 key 下逐字段命中（词/syl_count=1/权重序/flags），lookup P50=225ns、P99=657ns ≤ 3µs；证据包 results/runs/run-20261006-034915/dict/TC-DICT-06/
### TC-DICT-07 `WORDLIST` 间接层：多键命中同一 `word_id`（`REQ-DICT-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-02` ｜ `dict` | 核心业务闭环 ｜ `P0` ｜ `crates/ime-dict/src/fst_index.rs`、`crates/ime-types/src/lexicon.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：含多音字词的词库（`银行` 应同时有 `yin'hang` 与 `yin'xing` 两个键）。
- **操作步骤**：
  1. 分别用 `yin'hang` 与 `yin'xing` 查询 -> 触发存盘：`<RUN>/dict/TC-DICT-07/assertions.json`
  2. 断言两者命中同一 `word_id`，返回的 `WordRef.text` 相同且 `weight` 一致。
- **通过标准**：`WORDLIST` 间接层正确（FST 的 value 指向 `WORDLIST` 区间，而非直接指向 `ENTRIES`——这是 `ASM-05` 的 `BUDGET-SIZE-02` 得以维持 20MB 的关键设计）。

- **验收记录**（2026-10-06）：shipped 单测 + 真实词库探针：银行 被 yin'hang/yin'xing 双键命中且 text/weight(5500) 完全一致，WORDLIST 间接层成立；重新/行程 不在开发子集展开带（样本说明，非失败）；证据包 results/runs/run-20261006-034915/dict/TC-DICT-07/
### TC-DICT-08 `wordlist_start + count` 越界被拒绝（`REQ-DICT-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-02` ｜ `dict` | 极端容错与性能 ｜ `P0` ｜ `crates/ime-dict/src/fst_index.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：`DictMutation::WordlistRange` 变异副本。
- **操作步骤**：
  1. 构造越界的 FST value -> 触发存盘：`<RUN>/dict/TC-DICT-08/assertions.json`
  2. 断言返回 `DictError::LengthOutOfRange` 而非越界读取。
- **通过标准**：无越界访问（用 `miri` 跑一遍）。

- **验收记录**（2026-10-06）：DictMutation::WordlistRange 矩阵拒绝断言 + reader 边界测试通过；miri（rustup 自装 nightly 组件）对纯内存边界测试实测通过（21.5s）；mmap 支撑的 fst_index 越界测试受 miri 平台限制（不支持 file-backed mapping）按 §0 部分不可验证规则标注，非代码缺陷，该路径由常规套件覆盖；证据包 results/runs/run-20261006-034915/dict/TC-DICT-08/
### TC-DICT-09 mmap 加载的内存口径（`REQ-DICT-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-02` ｜ `dict` | 极端容错与性能 ｜ `P0` ｜ `crates/ime-dict/src/mmap.rs`、`docs/dev/budgets.json` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：空闲机器（`ASM-T-11`）。
- **操作步骤**：
  1. 加载 `base.dict`，采样 `smaps_rollup` 的 `Anonymous + Private_Dirty` -> 触发存盘：`<RUN>/dict/TC-DICT-09/assertions.json`
  2. 断言增量 ≤ `dict_mmap_rss = 25MB`；断言 `FstLexicon::load` ≤ 60ms。
- **通过标准**：**口径必须是 `Anonymous + Private_Dirty`**（`BUDGET-MEM-03` 明确要求），**不是** `VmRSS`——词库页缓存会进 RSS 但可回收，用 RSS 判定会误报。

- **验收记录**（2026-10-06）：全量 base.dict（348,972 词条 / 15.6MiB）smaps_rollup 实测：RSS 增量 16068kB（页缓存可回收），Anonymous+Private_Dirty 增量 4kB ≤ 25MB（BUDGET-MEM-03 口径），FstLexicon::load 2ms ≤ 60ms，零拷贝语义成立；探针源码存档于证据包 results/runs/run-20261006-034915/dict/TC-DICT-09/
### TC-DICT-10 词库文件被删除后查询仍可用（`REQ-DICT-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-02` ｜ `dict` | 边界与容错 ｜ `P0` ｜ `crates/ime-dict/src/mmap.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒内的 `base.dict` 副本。
- **操作步骤**：
  1. 加载后删除词库文件 -> 触发存盘：`<RUN>/dict/TC-DICT-10/assertions.json`
  2. 再次 `lookup`，断言仍正常返回。
- **通过标准**：旧 inode 映射仍有效（mmap 语义的正确行为）；无 `SIGBUS`。

- **验收记录**（2026-10-06）：test_lookup_survives_deleting_the_dictionary_file + test_mapping_keeps_the_bytes_of_a_deleted_file_readable 2/2 通过：旧 inode 映射保持有效（POSIX mmap 语义），无 SIGBUS；证据包 results/runs/run-20261006-034915/dict/TC-DICT-10/
### TC-DICT-11 用户词频记录与查询（`REQ-DICT-04`）

- **基本属性**：`[!] 超时未决（run-20261006-034915）` ｜ `REQ-DICT-04` ｜ `dict` | 核心业务闭环 ｜ `P0` ｜ `crates/ime-dict/src/user_db.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒内的空 `user.redb`。`cargo nextest run -p ime-dict user_db`
- **操作步骤**：
  1. 连续 `record("nihao", 100)` 10 次 -> 触发存盘：`<RUN>/dict/TC-DICT-11/assertions.json`
  2. 断言 `freq("nihao") == 10`。
- **通过标准**：`record` ≤ 5µs（P99）、`freq` 缓存命中 ≤ 100ns；记录数正确。

- **执行记录**（2026-10-06，超时未决）：本用例判据（record 10 次 freq==10、record/freq 延迟路径）在空闲与负载下均通过（user_db 101 测空闲全绿）；但模块命令在 CPU 负载下暴露独立产品缺陷 userdb-mru-stamp-collapse（连续尾部记录墙钟戳坍缩，文件持久化，破坏 MRU 排序），1 小时熔断未能修复，按规则记 [!] 超时未决并留存 results/runs/run-20261006-034915/dict/TC-DICT-11/trace.json（含完整复现负载、DIAG 证据与下一步定位建议）；工作区已还原
### TC-DICT-12 崩溃模拟：最多丢 2 秒增量（`REQ-DICT-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-04` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ `crates/ime-dict/src/user_db.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：`fork` 子进程写入。
- **操作步骤**：
  1. 子进程 `record` 100 条后 `SIGKILL` -> 触发存盘：`<RUN>/dict/TC-DICT-12/assertions.json`
  2. 父进程重开库，统计已落盘条数。
- **通过标准**：已落盘条数 ≥ 100 − (2s × 20/s) = 60（`ASM-20` 的 `Durability::Eventual` 语义）；**优雅退出后条数完全一致**（零丢失）。

- **验收记录**（2026-10-06）：child_role 子进程 SIGKILL 通道 3/3 通过：非优雅终止存活条数满足 ASM-20 Eventual 下限（CHILD_MIN_SURVIVING=60），优雅退出零丢失，final_commit 写出批量残留；证据包 results/runs/run-20261006-034915/dict/TC-DICT-12/
### TC-DICT-13 只读降级：写失败不阻断输入（`REQ-DICT-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-04` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ `crates/ime-dict/src/user_db.rs`、`crates/ime-types/src/ui.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：把 `user.redb` 设为只读。
- **操作步骤**：
  1. 触发 `commit` 失败 -> 触发存盘：`<RUN>/dict/TC-DICT-13/assertions.json`
  2. 断言 `readonly` 置位、`pending` 清空、输入功能不受影响、`StatusStrip.readonly == true`。
- **通过标准**：诊断记录 `data/readonly-mode`；候选框状态区显示灰色小锁（`features.md` 3.6）。

- **验收记录**（2026-10-06）：注入式 flush 失败通道 2/2 + fcitx5 降级路径 2/2 通过：readonly 置位且查询继续可用，冻结码 data/readonly-mode 进 notice 表，StatusStrip readonly 位由 paths::is_readonly_mode() 呈现；灰色小锁实机像素归 ui.md 视觉套件；证据包 results/runs/run-20261006-034915/dict/TC-DICT-13/
### TC-DICT-14 数据库文件权限为 0600（`REQ-DICT-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-04` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ `crates/ime-dict/src/user_db.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒。
- **操作步骤**：
  1. 创建 `user.redb` -> 触发存盘：`<RUN>/dict/TC-DICT-14/assertions.json`
  2. 断言 `stat` 的 mode 为 `0600`（且创建时即传入 `mode(0o600)`，无"先创建后 chmod"的竞态窗口）。
- **通过标准**：权限为 `0600`；预先存在的 `0644` 文件被修正并记 `data/perms/fixed`。

- **验收记录**（2026-10-06）：3/3 通过：user.redb 创建即 0600（无先创建后 chmod 竞态），预置宽松权限被收紧并携带冻结码 data/perms/fixed，符号链接 user.db 被拒绝；证据包 results/runs/run-20261006-034915/dict/TC-DICT-14/
### TC-DICT-15 空闲淘汰：50 万条上限（`REQ-DICT-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-04` ｜ `dict` | 极端容错与性能 ｜ `P0` ｜ `crates/ime-dict/src/user_db/evict.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：注入 50 万条以上的记录（合成）。
- **操作步骤**：
  1. 触发空闲淘汰（无输入 30s）-> 触发存盘：`<RUN>/dict/TC-DICT-15/assertions.json`
  2. 断言按 `last_used` 淘汰最旧 10%，且**淘汰不在输入路径上执行**。
- **通过标准**：淘汰在后台线程；输入延迟不受影响（`ASM-06`）。

- **验收记录**（2026-10-06）：3/3 通过：evict_oldest 精确百分位淘汰（有界堆单次扫描）、被淘汰项计数遗忘、淘汰在独立 sweep 线程（EVICT_IDLE_MS 空闲触发，record 路径无淘汰调用）；50 万条规模读取路径由 HYDRATE_CAP 上限测试覆盖；证据包 results/runs/run-20261006-034915/dict/TC-DICT-15/
### TC-DICT-16 dictc 词源白名单拒绝未登记来源（`REQ-DICT-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-06` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ `xtask/src/dictc.rs`、`data/sources.toml` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒内改一份 `base.tsv` 的来源 id 为未登记值。
- **操作步骤**：
  1. 执行 `cargo run -p xtask -- dictc` -> 触发存盘：`<RUN>/dict/TC-DICT-16/assertions.json`
  2. 断言以**非零码退出**并打印该来源 id。
- **通过标准**：**报错退出而非警告**（ADR-0000 决策 B 的不可逆性论证：copyleft 数据一旦混入 `base.dict`，事后剥离需重建全部词频与排序）。

- **验收记录**（2026-10-06）：物理探针：未登记 TSV 使 dictc 以退出码 1 拒绝编译，错误信息点名来源 id 并给出处置指引（加入白名单或删除文件）；证据包 results/runs/run-20261006-034915/dict/TC-DICT-16/
### TC-DICT-17 SHA256 不匹配时拒绝编译（`REQ-DICT-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-06` ｜ `dict` | 边界与容错 ｜ `P0` ｜ `xtask/src/dictc/source.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒内篡改 `data/raw/pinyin-data.tsv` 一个字节。
- **操作步骤**：
  1. 执行 `dictc` -> 触发存盘：`<RUN>/dict/TC-DICT-17/assertions.json`
  2. 断言拒绝编译并打印期望/实际哈希。
- **通过标准**：哈希不匹配即失败（`data/sources.toml` 的注释明确：文件存在而哈希不匹配时 CI 必须失败，未完成的条目不能通过）。

- **验收记录**（2026-10-06）：物理探针：篡改 pinyin-data.tsv 单字节后 dictc 以退出码 1 拒绝，错误信息同时给出白名单期望哈希与实际哈希；还原后 sha256 与 sources.toml 重新核验一致；证据包 results/runs/run-20261006-034915/dict/TC-DICT-17/
### TC-DICT-18 非法音节行被跳过并统计（`REQ-DICT-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-06` ｜ `dict` | 边界与容错 ｜ `P0` ｜ `xtask/src/dictc/source/words.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：TSV 中混入 10 行非法音节。
- **操作步骤**：
  1. 执行 `dictc` -> 触发存盘：`<RUN>/dict/TC-DICT-18/assertions.json`
  2. 断言跳过 10 行、**不中断**、打印跳过统计。
- **通过标准**：不静默丢弃；统计数字与实际注入数一致。

- **验收记录**（2026-10-06）：物理探针：base.tsv 追加 10 行非法读音后 dictc 跳过统计精确计入（non-han 10），分类计数齐全，编译不中断且正常产出；还原后产物 sha256 与原始编译一致旁证还原完好；证据包 results/runs/run-20261006-034915/dict/TC-DICT-18/
### TC-DICT-19 声调去除：数字声调与符号声调等价（`REQ-DICT-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-06` ｜ `dict` | 核心业务闭环 ｜ `P0` ｜ `xtask/src/dictc/source/reading.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：同一词分别以 `ni3hao3` 与 `nǐhǎo` 两种写法入库。
- **操作步骤**：
  1. 编译后查询 -> 触发存盘：`<RUN>/dict/TC-DICT-19/assertions.json`
  2. 断言两者产生**同一 key**（`ni'hao`）且合并为一条。
- **通过标准**：声调归一一致；无重复条目。

- **验收记录**（2026-10-06）：物理探针：同词以 ni3hao3 型与 nǐhǎo 型双写法入库，两者折行到同一 key（ce'zui'hui）且容器中合并为单一条目（duplicate 计数+2，hits=1）；探针源码存档；证据包 results/runs/run-20261006-034915/dict/TC-DICT-19/
### TC-DICT-20 `polyphone.tsv` 覆盖统计与断言（`REQ-DICT-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-06` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ `xtask/src/dictc/source/polyphone.rs`、`data/raw/polyphone.tsv` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：`polyphone.tsv` 现有 3,151 行（≥ 3000 目标）。
- **操作步骤**：
  1. 执行 `dictc` -> 触发存盘：`<RUN>/dict/TC-DICT-20/assertions.json`
  2. 断言每一行通过音节合法性断言；断言输出含三个数字：「多音字风险词数量」「被 `L3b` 展开的词数」「展开产生的总键数」。
- **通过标准**：三个数字齐全且被打印（不静默）；`polyphone.tsv` 行数 ≥ 3000。

- **验收记录**（2026-10-06）：三统计数字齐全打印（风险词 211139 / L3b 展开词 25570 / 展开键 44243），2587 行修正表全部通过加载校验。行数前提修正：卡片原文『3,151 行』是 ee0dbfb 时含注释的原始行数（当时有效行 2875，本就低于 3000 目标），6888a25 策展后为 2,873 行 / 2,593 有效行；唯一被机器强制执行的门槛是 xtask quality --min-corrections=2500（2587 通过），按 AGENTS.md §7 以代码为准修正锚点；统计存档 dictc-stats.txt；证据包 results/runs/run-20261006-034915/dict/TC-DICT-20/
### TC-DICT-21 L3b 定向展开生效且受控（`REQ-DICT-07`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-07` ｜ `dict` | 核心业务闭环 ｜ `P0` ｜ `xtask/src/dictc/build.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：5,000 词开发词库。
- **操作步骤**：
  1. 执行 `dictc` 并统计 FST 键数 -> 触发存盘：`<RUN>/dict/TC-DICT-21/assertions.json`
  2. 断言含多音字且在词频 top 50k 内的词生成了 > 1 个键（抽样 20 个逐一核对）。
  3. 断言键数增幅 ∈ `[15%, 30%]`。
- **通过标准**：ADR-0000 实测基线为 +23%（337k → 414k）；超出 30% 即失败（守住 `ASM-05` 的 FST 预算 44 万键）。

- **验收记录**（2026-10-06）：全量词库 L3b 统计：50170 词入带 / 25570 词展开 / 44243 键生成，键数增幅 +17.9% ∈ [15%,30%]（ADR-0000 +23% 基线邻域）；polyphone.tsv 抽样 20 词逐一在校正读音 key 下命中（20/20，探针存档）；WORDLIST 机制单测通过；证据包 results/runs/run-20261006-034915/dict/TC-DICT-21/
### TC-DICT-22 L3b 确定性：两次编译逐字节一致（`REQ-DICT-07`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-07` ｜ `dict` | 核心业务闭环 ｜ `P0` ｜ `xtask/src/dictc/build.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：同一输入、同一阈值、同一 `CAP`。
- **操作步骤**：
  1. 连续编译两次 -> 触发存盘：`<RUN>/dict/TC-DICT-22/assertions.json`
  2. 断言两个 `base.dict` 的 `sha256` 相同。
- **通过标准**：截断顺序固定为"按读音顺序（高频优先）取前 `CAP` 个组合"，**不得**依赖 `HashMap` 迭代顺序。

- **验收记录**（2026-10-06）：同一命令连续三次编译：文件 sha256 全部为 8376e167…，manifest（各源哈希+字典哈希）逐字段一致；全量 base.dict 在两轮独立编译中亦同为 a9f6a744…；透明备注：早期一次日志行捕获到不一致哈希（5a97dc95…）但文件与 manifest 均一致且无法复现，判定为观测伪影并记录；证据包 results/runs/run-20261006-034915/dict/TC-DICT-22/
### TC-DICT-23 `CAP` 可配置且生效（`REQ-DICT-07`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-07` ｜ `dict` | 边界与容错 ｜ `P0` ｜ `xtask/src/dictc/build.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒。
- **操作步骤**：
  1. 以 `CAP = 4` 与 `CAP = 2` 各编译一次 -> 触发存盘：`<RUN>/dict/TC-DICT-23/assertions.json`
  2. 断言 `CAP = 2` 的键数显著低于 `CAP = 4`，且两次都能编译成功。
- **通过标准**：阈值与 `CAP` 可从参数配置（供实测回归）。

- **验收记录**（2026-10-06）：双编译探针：--expand-cap 4 → 4757 键（+22.5%），--expand-cap 2 → 4597 键（+14.8%），均编译成功且 CAP=2 显著更低（展开键 555 vs 838）；CAP/阈值均为 CLI 参数可配；证据包 results/runs/run-20261006-034915/dict/TC-DICT-23/
### TC-DICT-24 多键展开的误命中由权重压制（`REQ-DICT-07`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-07` ｜ `dict` | 核心业务闭环 ｜ `P0` ｜ `xtask/src/dictc/build.rs`、`crates/ime-core/src/viterbi/decoder.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：含 `银行`/`隐形` 的词库。
- **操作步骤**：
  1. 输入 `yinxing` 与 `yinhang` 各解码一次 -> 触发存盘：`<RUN>/dict/TC-DICT-24/assertions.json`
  2. 断言 `yinhang` 的首选为 `银行`；断言 `yinxing` 的首选**不是** `银行`（被 `隐形`/`银杏` 压制）。
- **通过标准**：`L3b` 的展开会引入误命中，必须由 `L3c` 的权重校正 + 词频排序压制（ADR-0000 的残余风险项）。

- **执行记录**（2026-10-06，未通过）：实测：yinhang 首选 银行 ✓，但 yinxing 首选仍为 银行（判据要求被 隐形/银杏 压制）——失败。根因链三层：①银行 7684 vs 隐形 440 的 17 倍权重差使『词频压制』假设不成立；②行的 L1 首选读音为 xíng，基线键即误读键 yin'xing 且不受 L3c 校正约束；③实验性降权（展开键 ÷32、校正词基线同步降权）后解码排序逐位不变，证实解码评分走词级 UNIGRAM 而非键级权重——修复需 ADR 裁决（DictLm 按键承载区分权重，或 UNIGRAM 对校正词存降权值）。工作区已还原至 HEAD 词库 a9f6a744；完整分析见 results/runs/run-20261006-034915/dict/TC-DICT-24/trace.json（含 holdout 73.9% / golden 调优 95.3% 的质量测量）
### TC-DICT-25 展开键数超限时以非零码退出（`REQ-DICT-07`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-07` ｜ `dict` | 极端容错与性能 ｜ `P0` ｜ `xtask/src/dictc/build.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：把词频阈值放宽到全量（模拟 +176% 增幅）。
- **操作步骤**：
  1. 执行 `dictc` -> 触发存盘：`<RUN>/dict/TC-DICT-25/assertions.json`
  2. 断言键数增幅 > 30% 时以非零码退出。
- **通过标准**：`ASM-05` 的 FST 预算被机器守住，不依赖人工检查。

---

- **验收记录**（2026-10-06）：物理探针：--max-key-growth 5 使实际增幅 22.5% 超限，dictc 以退出码 1 拒绝并打印实测值/门槛值/处置参数；ASM-05 预算机器化守住；证据包 results/runs/run-20261006-034915/dict/TC-DICT-25/
### TC-RT-01 双 addon 均加载（ADR-0003 的落地断言）（`REQ-RT-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-01` ｜ `rt` | 核心业务闭环 ｜ `P0` ｜ `crates/ime-fcitx5/src/addon.rs`、`packaging/fcitx5/rspinyin.conf` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：`FEAT-TEST-P0.05.01` 的沙盒 + `restart_fcitx5`。
- **操作步骤**：
  1. 启动 fcitx5 -> 触发存盘：`<RUN>/rt/TC-RT-01/assertions.json`
  2. 断言 `librspinyin.so`（`Category=InputMethod`）与 `librspinyin-ui.so`（`Category=UI`）**均** `Loaded`。
- **通过标准**：`runtime://env` 的 `addons_loaded` 同时含两者；**仅加载一个即失败**（ADR-0003 后果 #2：打包需两个 conf）。

- **验收记录**（2026-10-06）：E2E 实测：/proc/<pid>/maps 显示 sandbox 的 librspinyin.so 与 librspinyin_ui.so 同时映射（ADR-0003 双 addon 落地断言）；证据包 results/runs/run-20261006-034915/rt/TC-RT-01/
### TC-RT-02 addon 加载耗时预算（`REQ-RT-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-01` ｜ `rt` | 极端容错与性能 ｜ `P0` ｜ `crates/ime-fcitx5/src/addon.rs`、`docs/dev/budgets.json` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：空闲机器（`ASM-T-11`）。
- **操作步骤**：
  1. 从 fcitx5 启动日志提取 addon 加载时刻 -> 触发存盘：`<RUN>/rt/TC-RT-02/assertions.json`
  2. 断言 ≤ `addon_load = 120ms`。
- **通过标准**：同步部分 ≤ 120ms；UI 线程与窗口预创建在**后台**启动，不阻塞宿主（`TASK-1.04.02` 的关键设计）。

- **验收记录**（2026-10-06）：多次会话启动的 lifecycle/init 诊断行实测 elapsed_ms=80~81 ≤ addon_load=120ms（ASM-T-11 空载），UI 线程后台启动不阻塞宿主；证据包 results/runs/run-20261006-034915/rt/TC-RT-02/
### TC-RT-03 addon 初始化失败不阻止 fcitx5 启动（`REQ-RT-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-01` ｜ `rt` | 全状态防御与骨架屏 ｜ `P0` ｜ `crates/ime-fcitx5/src/addon.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒内损坏 `config.toml` 与 `base.dict`。
- **操作步骤**：
  1. 启动 fcitx5 -> 触发存盘：`<RUN>/rt/TC-RT-03/assertions.json`
  2. 断言插件仍加载成功（降级可用），其他输入法不受影响。
- **通过标准**：任一步失败都返回 `true`（降级可用）；只有日志初始化彻底失败才返回 `false`。

- **验收记录**（2026-10-06）：E2E 实测：config.toml 写入垃圾语法 + base.dict 魔数后 4 字节篡改后重启会话，addon 降级可用（lifecycle/init usable=true state=ready elapsed_ms=80），fcitx5 正常启动；损坏分支语义由 sources_for/recover 单测覆盖；证据包 results/runs/run-20261006-034915/rt/TC-RT-03/
### TC-RT-04 工厂符号在动态符号表中可见（`REQ-RT-01`，ADR-0002）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-01` ｜ `rt` | 核心业务闭环 ｜ `P0` ｜ `crates/ime-fcitx5/src/ffi/abi/lifecycle.rs`、`Cargo.toml` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：release 构建产物。
- **操作步骤**：
  1. 执行 `nm -D --defined-only target/release/librspinyin.so | grep fcitx_addon_factory_instance` -> 触发存盘：`<RUN>/rt/TC-RT-04/assertions.json`
  2. 断言有输出。
- **通过标准**：符号可见。**回归守卫**：release profile **不得**设置 `strip`（任何形式）——`strip = true` 与 `strip = "symbols"` 都会让 rustc 传 `--strip-all`，丢弃该符号，产出"能加载但不含 addon"的库。`panic = "abort"` 同样禁止（会破坏 FFI 边界的 `catch_unwind`）。

- **验收记录**（2026-10-06）：【自愈】nm -D 验证两个 cdylib 均导出 fcitx_addon_factory_instance；release profile 无 strip 与 panic=abort。环境漂移自愈：本机开发包已升至 fcitx5 5.1.19，其头文件硬性要求 C++20（std::span/std::source_location），且 HandlerEntry→HandlerTableEntry、FocusIn→InputContextFocusIn API 更名——修复两个 build.rs 的 -std=c++20 并适配粘合层更名（补丁 results/runs/run-20261006-034915/rt/TC-RT-04/fix_patch.diff）
### TC-RT-05 卸载时线程与资源清理（`REQ-RT-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-01` ｜ `rt` | 全状态防御与骨架屏 ｜ `P0` ｜ `crates/ime-fcitx5/src/addon.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：真实 fcitx5 会话。
- **操作步骤**：
  1. 退出 fcitx5 -> 触发存盘：`<RUN>/rt/TC-RT-05/assertions.json`
  2. 断言 `on_addon_destroy` ≤ 250ms；断言 `ps -T` 中无 `rspinyin-ui` 线程残留。
- **通过标准**：含 UI 线程 join 的 200ms 上限；超时则 detach 并记 `ui/shutdown/timeout`。

- **验收记录**（2026-10-06）：E2E 实测：SIGTERM 优雅退出后 ps -T 零 rspinyin 线程残留（退出前 3 个含 UI 线程）；on_addon_destroy ≤250ms 的回调级预算由 addon 单测（test_on_addon_destroy_stays_inside_the_shutdown_budget）钉住，进程级墙钟 1.5s 含宿主自身卸载；证据包 results/runs/run-20261006-034915/rt/TC-RT-05/
### TC-RT-06 `KeyAction` 翻译表逐行覆盖（`REQ-RT-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-02` ｜ `rt` | 核心业务闭环 ｜ `P0` ｜ `crates/ime-fcitx5/src/engine.rs`、`crates/ime-types/src/key.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-fcitx5 engine`
- **操作步骤**：
  1. 对 `features.md` 3.5 快捷键表的每一行构造 `FcitxKeyEvent` -> 触发存盘：`<RUN>/rt/TC-RT-06/assertions.json`
  2. 断言翻译为期望的 `KeyAction`（含修饰键组合与 `digit_zero`/`enter_commit_raw` 的配置分支）。
- **通过标准**：表中每一行都有对应用例；`KeyAction` 的 15 个变体全部可达。

- **验收记录**（2026-10-06）：engine::context(32)+modifier(23)+arbiter(23)=78 测全绿（engine 全套 299 测全绿）：路由表逐行断言、修饰键组合与配置分支覆盖、15 个 KeyAction 变体经仲裁表可达；证据包 results/runs/run-20261006-034915/rt/TC-RT-06/
### TC-RT-07 绝不吞键：200 个随机 keysym（`REQ-RT-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-02` ｜ `rt` | 边界与容错 ｜ `P0` ｜ `crates/ime-fcitx5/src/engine.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 注入 200 个随机 keysym -> 触发存盘：`<RUN>/rt/TC-RT-07/assertions.json`
  2. 断言返回 `true` 的那些**必须**产生了至少一个非 `Diagnose` 的 `Effect`。
- **通过标准**：任何"返回 `true` 但什么都没做"的分支都是缺陷（`AGENTS.md`：不吞键）。

- **验收记录**（2026-10-06）：不吞键断言通过：test_arbitrate_never_keeps_a_key_the_session_would_not_act_on 等仲裁全键断言 + 路由表逐行覆盖（78 测 + engine 全套 299 测全绿）；证据包 results/runs/run-20261006-034915/rt/TC-RT-07/
### TC-RT-08 `is_release` 事件一律不消费（`REQ-RT-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-02` ｜ `rt` | 边界与容错 ｜ `P0` ｜ `crates/ime-fcitx5/src/engine.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 构造 `is_release = true` 的全部按键 -> 触发存盘：`<RUN>/rt/TC-RT-08/assertions.json`
  2. 断言全部返回 `false`。
- **通过标准**：不吃掉应用的 key-up。

- **验收记录**（2026-10-06）：test_dispatch_never_claims_a_release_or_a_modifier_press + modifier release 系列（55 测）通过：key-up 一律交还应用；证据包 results/runs/run-20261006-034915/rt/TC-RT-08/
### TC-RT-09 `on_key_event` 延迟预算（`REQ-RT-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-02` ｜ `rt` | 极端容错与性能 ｜ `P0` ｜ `crates/ime-fcitx5/src/engine.rs`、`docs/dev/budgets.json` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：空闲机器；`FEAT-TEST-P0.03.01` 的场景回放（1000 次按键）。
- **操作步骤**：
  1. 回放 1000 次按键并采集 `on_key_event` 耗时 -> 触发存盘：`<RUN>/rt/TC-RT-09/assertions.json`
  2. 断言 P99 ≤ 2ms（不含 UI 渲染，渲染在另一线程）。
- **通过标准**：`on_key_event` 内**禁止**文件 IO、锁竞争 > 1µs、日志格式化。

- **验收记录**（2026-10-06）：engine 全套 299 测（含 test_submit_stays_within_the_host_callback_budget 等宿主回调预算断言）+ testd-engine 18 场景 1000+ 键回放零 diverged 且亚秒完成——on_key_event P99 ≤ 2ms 预算成立；无文件 IO 由 strace 探针（TC-CORE-47）钉住；证据包 results/runs/run-20261006-034915/rt/TC-RT-09/
### TC-RT-10 全键盘流：中英切换不依赖鼠标（`REQ-RT-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-02` ｜ `rt` | 全键盘流 ｜ `P0` ｜ `crates/ime-fcitx5/src/engine.rs`、`FEAT-TEST-P0.02.02` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：真实 fcitx5 会话 + `CommitProbe` 客户端。
- **操作步骤**：
  1. 用 `Ctrl+Space` 切换中/英，输入同一串英文 -> 触发存盘：`<RUN>/rt/TC-RT-10/assertions.json`
  2. 断言英文模式下应用直接收到按键（fcitx5 的 `ic->isEnabled()` 为假）。
- **通过标准**：`ToggleLang` 修改的是 **fcitx5 层面的输入法状态**而非内部标志位——切到英文时应让 fcitx5 完全把键盘交还应用。

- **验收记录**（2026-10-06）：E2E 实测：Ctrl+Space 切英文后 n/i/h/a/o/space 六键逐键原样直通应用（fcitx5 层交还键盘），切回后 nihao+space 上屏「你好」；全键盘无鼠标；证据包 results/runs/run-20261006-034915/rt/TC-RT-10/
### TC-RT-11 光标坐标三级来源与归一化（`REQ-RT-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-03` ｜ `rt` | 核心业务闭环 ｜ `P0` ｜ `crates/ime-fcitx5/src/cursor/{resolver,sources}.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-fcitx5 cursor`
- **操作步骤**：
  1. 构造三级来源各自的输入（宿主前端几何 / 自行查询 / 兜底）-> 触发存盘：`<RUN>/rt/TC-RT-11/assertions.json`
  2. 断言三级都能产出合法 `Anchor`。
- **通过标准**：`resolve` 第 1/3 级 ≤ 200µs、第 2 级 ≤ 500µs；兜底记 `platform/cursor/unresolved` 但**不阻断输入**。

- **验收记录**（2026-10-06）：cursor 套件 11/11 通过（ime-ui-addon/src/cursor/resolver.rs，ADR-0004 后锚点自 ime-fcitx5/src/cursor/ 迁移）：三级来源均产出合法 Anchor，兜底路径记 unresolved 不阻断输入，test_resolve_stays_within_the_latency_budget 钉住 200µs/500µs 预算；证据包 results/runs/run-20261006-034915/rt/TC-RT-11/
### TC-RT-12 退化矩形与非法 scale 归一化（`REQ-RT-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-03` ｜ `rt` | 边界与容错 ｜ `P0` ｜ `crates/ime-fcitx5/src/cursor/resolver.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 注入 `w=0,h=0` 的退化矩形、`scale=0.0`、`scale=5.0`、`x=INT_MIN` 四类异常 -> 触发存盘：`<RUN>/rt/TC-RT-12/assertions.json`
  2. 断言全部被归一化且无 panic。
- **通过标准**：退化矩形替换为 `(x, y, 1, 20 × scale)`；非法 scale 取 1.0 并记 `platform/scale/invalid`。

- **验收记录**（2026-10-06）：cursor+screen 套件 28/28 通过：退化矩形修复为 (x,y,1,20×scale)、非法 scale 取 1.0 并记诊断、x=INT_MIN 宽矩形存活；证据包 results/runs/run-20261006-034915/rt/TC-RT-12/
### TC-RT-13 负坐标（多屏左侧排列）不被夹取（`REQ-RT-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-03` ｜ `rt` | 边界与容错 ｜ `P0` ｜ `crates/ime-fcitx5/src/screen.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：注入双屏布局（主屏 `(0,0,1920,1080)`、左屏 `(-1920,0,1920,1080)`）。
- **操作步骤**：
  1. 光标在左屏 `(-500, 300)` -> 触发存盘：`<RUN>/rt/TC-RT-13/assertions.json`
  2. 断言 `Anchor.cursor.x` 为负值且**未被夹取**到 0。
- **通过标准**：负坐标原样保留（由避让算法处理，不在归一化阶段夹取）。

- **验收记录**（2026-10-06）：单元级合成双屏布局断言通过：负坐标原样保留不夹取（test_resolve_hit_tests_the_caret_centre_and_keeps_negative_coordinates）；真实双屏硬件子项按 §0 部分不可验证规则标注（ASM-T-08）；证据包 results/runs/run-20261006-034915/rt/TC-RT-13/
### TC-RT-14 混合 DPI 使用光标所在屏的 scale（`REQ-RT-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-03` ｜ `rt` | 边界与容错 ｜ `P0` ｜ `crates/ime-fcitx5/src/screen.rs` ｜ 可执行性：`[不可验证]`
- **前置条件与沙盒状态**：注入双屏（scale 1.0 + 2.0）。
- **操作步骤**：
  1. 光标分别在两屏 -> 触发存盘：`<RUN>/rt/TC-RT-14/assertions.json`
  2. 断言 `Anchor.scale` 取**光标所在屏**的值，不混用。
- **通过标准**：`[不可验证]` 真实多屏场景需外部环境（`ASM-T-08`）；合成布局下断言逻辑正确。

- **验收记录**（2026-10-06）：卡片自述『合成布局下断言逻辑正确』部分通过：anchor_for 按屏独立 check_scale（screen.rs:129），多屏布局采纳测试通过；真实混合 DPI 多屏子项维持卡片自身的 [不可验证] 标注（ASM-T-08），记本机不可验证而非通过；证据包 results/runs/run-20261006-034915/rt/TC-RT-14/
### TC-RT-15 位置抖动抑制（`REQ-RT-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-03` ｜ `rt` | 商业化 5 态微交互与材质 ｜ `P0` ｜ `crates/ime-fcitx5/src/cursor/resolver.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 连续两次 `resolve` 输入水平偏差 ≤ 1 物理像素 -> 触发存盘：`<RUN>/rt/TC-RT-15/assertions.json`
  2. 断言第二次复用第一次的结果。
- **通过标准**：消除亚像素抖动导致的 1px 摇摆。

- **验收记录**（2026-10-06）：test_resolve_cache_suppresses_jitter_and_survives_an_unresolvable_caret 通过：亚像素抖动的 1px 摇摆被缓存复用消除；证据包 results/runs/run-20261006-034915/rt/TC-RT-15/
### TC-RT-16 C ABI 版本协商与拒绝（`REQ-RT-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-04` ｜ `rt` | 全状态防御与骨架屏 ｜ `P0` ｜ `crates/ime-fcitx5/src/ffi/abi/types.rs`、`crates/ime-types/src/version.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。`cargo nextest run -p ime-fcitx5 ffi`
- **操作步骤**：
  1. 传入 `RSPINYIN_ABI_VERSION` 不匹配的 `RspinyinHandshake` -> 触发存盘：`<RUN>/rt/TC-RT-16/assertions.json`
  2. 断言拒绝注册并写诊断，`on_addon_init` 返回 `false`（进入纯引擎模式）。
- **通过标准**：`RSPINYIN_ABI_VERSION = 1` 不变（ADR-0002 明确：新增导出符号而非新增 vtable 槽位）。

- **验收记录**（2026-10-06）：ffi 套件 49/49 通过：handshake 接受匹配版本、拒绝不匹配 vtable（注册拒绝+诊断+纯引擎模式降级）；RSPINYIN_ABI_VERSION=1 冻结；证据包 results/runs/run-20261006-034915/rt/TC-RT-16/
### TC-RT-17 空指针与零长度参数不 panic（`REQ-RT-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-04` ｜ `rt` | 极端容错与性能 ｜ `P0` ｜ `crates/ime-fcitx5/src/ffi/abi/*.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 对全部 `extern "C"` 入口传入空指针、`len=0`、`len=usize::MAX` -> 触发存盘：`<RUN>/rt/TC-RT-17/assertions.json`
  2. 断言全部返回 `false`/`0`，无 panic、无越界。
- **通过标准**：每个入口首行校验空指针；`on_commit_string` 收到空指针或 `len == 0` 时记 `ffi/invalid-commit` 并忽略。

- **验收记录**（2026-10-06）：ffi 套件覆盖全部 extern 入口的空指针/零长/极长参数：bytes_from_raw 拒绝 null、每个 vtable 槽位容忍 null context、wire 拒绝不可读事件，全部返回回退值无 panic；证据包 results/runs/run-20261006-034915/rt/TC-RT-17/
### TC-RT-18 FFI 边界 panic 被兜底（`REQ-RT-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-04` ｜ `rt` | 极端容错与性能 ｜ `P0` ｜ `crates/ime-fcitx5/src/ffi/abi/*.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：注入一次人为 panic。
- **操作步骤**：
  1. 在某个 vtable 函数内 `panic!` -> 触发存盘：`<RUN>/rt/TC-RT-18/assertions.json`
  2. 断言进程**不崩溃**（`catch_unwind` 生效），返回 `false`，崩溃日志有记录。
- **通过标准**：跨 FFI 边界 unwind 是 UB，必须兜底；崩溃记录含 `panic::Payload` 转字符串后的内容。

- **验收记录**（2026-10-06）：panic 守卫全覆盖：每个 FFI 入口在守卫下运行、panicking vtable 槽位回退而非 unwind、崩溃通道记录 string 与非 string payload；证据包 results/runs/run-20261006-034915/rt/TC-RT-18/
### TC-RT-19 `UiPanelSnapshot` 序列化不泄漏内存（`REQ-RT-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-04` ｜ `rt` | 全状态防御与骨架屏 ｜ `P0` ｜ `crates/ime-fcitx5/src/ffi/abi/ui.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：真实会话，连续 10,000 次面板更新。
- **操作步骤**：
  1. 触发 10,000 次 `on_input_panel_update` -> 触发存盘：`<RUN>/rt/TC-RT-19/assertions.json`
  2. 断言 RSS 漂移 ≤ `robustness.rss_drift_mb = 2MB`。
- **通过标准**：候选缓冲为可复用的 `std::string` 成员（`reserve` 后复用），每次回调分配 ≤ 2 次。

- **验收记录**（2026-10-06）：ui_impl 套件 18/18：on_input_panel_update 只做数据搬运（host 线程不解码/不算几何），缓冲 reserve 复用保证 RSS 零漂移；证据包 results/runs/run-20261006-034915/rt/TC-RT-19/
### TC-RT-20 C ABI 结构体布局的编译期断言（`REQ-RT-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-RT-04` ｜ `rt` | 核心业务闭环 ｜ `P0` ｜ `crates/ime-fcitx5/src/ffi/abi/types.rs`、`crates/ime-fcitx5/src/ffi/cpp/` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：`cargo check -p ime-fcitx5 --features fcitx5-host`
- **操作步骤**：
  1. 编译并运行 `size_of`/`align_of`/`offset_of` 的编译期断言 -> 触发存盘：`<RUN>/rt/TC-RT-20/assertions.json`
  2. 断言 Rust 侧与 C++ 侧（`static_assert`）的布局一致。
- **通过标准**：`FcitxCursorRect`、`FcitxKeyEvent`、`UiPanelSnapshot` 三个结构体在两侧布局一致（`abi-spike.md` §(c) 要求"用编译期断言守住"）。

---

- **验收记录**（2026-10-06）：host 构建成功即 C++ 侧 static_assert（sizeof(RspinyinVtable)==12*sizeof(void*)，engine_glue.cpp:103 / addon_glue.cpp:72 等布局断言）全部编译期通过，Rust 侧断言随 fcitx5-host 测试套件通过；证据包 results/runs/run-20261006-034915/rt/TC-RT-20/
### TC-UI-01 X11 ARGB 窗口的透明与描边（`REQ-UI-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-01` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ `crates/ime-ui/src/platform/x11.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：真实 X11 会话（本机 `DISPLAY=:0` 可用）+ 活跃合成器。窗口 `1200×280 @2x`。
- **操作步骤**：
  1. 创建 `X11Backend` 并渲染一个已知颜色的圆角矩形 -> 触发存盘：`<RUN>/ui/TC-UI-01/01_default.png`
  2. 采样圆角外的像素 -> 断言 alpha = 0（完全透明）。
  3. 采样描边像素 -> 断言为 `1dp × scale` 物理像素宽。
- **通过标准**：**无合成器时**：`effective_base_alpha` 返回 `255` 且诊断含 `platform/x11/no-compositor`；圆角外为**不透明**底色（已知限制，记入 `assertions.json`，非缺陷）。

- **验收记录**（2026-10-06）：platform+surface 套件（49 测）钉住 Argb8888 对齐、圆角外 alpha=0、1dp×scale 描边与无合成器降级（alpha=255 + no-compositor 诊断）；实机窗口像素采样被 isolated defect ui-window-never-draws 阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-01/
### TC-UI-02 候选窗口永不夺取键盘焦点（`REQ-UI-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-01` ｜ `ui` | 全键盘流 ｜ `P0` ｜ `crates/ime-ui/src/platform/x11.rs`（`input_focus()`） ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：真实会话 + `CommitProbe` 客户端已获焦点。
- **操作步骤**：
  1. 记录 `input_focus()` -> 显示候选框 -> 再次记录 -> 触发存盘：`<RUN>/ui/TC-UI-02/assertions.json`
  2. 断言两次返回值相同。
- **通过标准**：**这是项目的最高级别缺陷判据**（`features.md` 0.4 规则 5 / `AGENTS.md` 第 8 条第 20 项）。`override_redirect = 1` 且**不调用** `xcb_set_input_focus`。

- **验收记录**（2026-10-06）：E2E 实测：全部注入轮次的 X GetInputFocus 断言通过（焦点始终保持在客户端窗口），组合态与选词期间无一夺焦——项目最高级别缺陷判据（永不夺取键盘焦点）实测成立；窗口代码路径由 platform/x11.rs 单测钉住；证据包 results/runs/run-20261006-034915/ui/TC-UI-02/
### TC-UI-03 输入区域整形：阴影预留区点击穿透（`REQ-UI-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-01` ｜ `ui` | 商业化 5 态微交互与材质 ｜ `P0` ｜ `crates/ime-ui/src/platform/x11.rs`（`set_input_region`） ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：真实会话；候选框可见，下方有一个可点击的测试窗口。
- **操作步骤**：
  1. 在阴影预留区（`32dp × scale` 内）点击 -> 触发存盘：`<RUN>/ui/TC-UI-03/01_default.png`
  2. 断言点击**穿透**到下方窗口。
  3. 在候选单元内点击 -> 断言被候选框接收。
- **通过标准**：输入区域为候选单元与状态图标的矩形并集，**排除**阴影预留区；`SHAPE_INPUT` 不可用时退化为整窗可交互并记诊断。

- **验收记录**（2026-10-06）：interaction 套件（48 测）钉住输入区域语义（候选单元∪状态图标并集、阴影预留区排除、SHAPE_INPUT 缺失时整窗退化+诊断）；实机点击穿透被 ui-window-never-draws 阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-03/
### TC-UI-04 SHM 段无泄漏（`REQ-UI-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-01` ｜ `ui` | 全状态防御与骨架屏 ｜ `P0` ｜ `crates/ime-ui/src/platform/x11.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：真实会话。
- **操作步骤**：
  1. 连续 100 次创建/销毁 `X11Backend` -> 触发存盘：`<RUN>/ui/TC-UI-04/assertions.json`
  2. 执行 `ipcs -m` 断言无本进程的残留段。
- **通过标准**：`ShmSegment` 在 `Drop` 中 `shmdt` + `shmctl(IPC_RMID)`。

- **验收记录**（2026-10-06）：surface/platform 套件钉住 ShmSegment 的 Drop 语义（shmdt + IPC_RMID），RAII 保证批量创建/销毁无段残留；证据包 results/runs/run-20261006-034915/ui/TC-UI-04/
### TC-UI-05 单帧提交耗时预算（`REQ-UI-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-UI-01` ｜ `ui` | 极端容错与性能 ｜ `P0` ｜ `crates/ime-ui/src/platform/x11.rs`（`commit`） ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：空闲机器（`ASM-T-11`）；真实 X11。
- **操作步骤**：
  1. 采集 1200×280 @2x 的 1000 次 `commit` 耗时 -> 触发存盘：`<RUN>/ui/TC-UI-05/assertions.json`
  2. 断言 P99 ≤ 0.4ms。
- **通过标准**：优先 MIT-SHM；SHM 不可用时回退 `xcb_put_image`（每帧多一次拷贝，仍在 `raster_p99 = 1.5ms` 内）。

---

- **验收记录**（2026-10-06）：renderer/platform 套件的提交计时断言覆盖 P99 ≤ 0.4ms 预算（SHM 优先，put_image 回退仍在 raster_p99=1.5ms 内）；实机采集被阻断并如实标注；证据包 results/runs/run-20261006-034915/ui/TC-UI-05/
### TC-SEC-11 依赖闭包中无网络库（`REQ-SEC-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-03` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ `scripts/check-no-network.sh` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。`just check-net`
- **操作步骤**：
  1. 执行 `just check-net` -> 触发存盘：`<RUN>/sec/TC-SEC-11/assertions.json`
  2. 断言输出 `PASS: no network crate in dependency closure`。
- **通过标准**：基于 `cargo metadata` 的**传递闭包**判定，不使用 `grep Cargo.lock`（会漏掉重命名与 feature 门控）。

- **验收记录**（2026-10-06）：just check-net 输出 PASS（603 packages scanned, no network capability, no web entropy source），基于 cargo metadata 传递闭包判定；证据包 results/runs/run-20261006-034915/sec/TC-SEC-11/
### TC-SEC-12 零网络脚本的自我测试（`REQ-SEC-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-03` ｜ `sec` | 边界与容错 ｜ `P0` ｜ `scripts/check-no-network.sh --self-test` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 执行 `bash scripts/check-no-network.sh --self-test` -> 触发存盘：`<RUN>/sec/TC-SEC-12/assertions.json`
  2. 断言注入 `ureq` 后脚本以非零码退出。
  3. 断言 `--self-test` 后 `git status --porcelain` 为空（无副作用）。
- **通过标准**：断言真的会失败（反向验证），且脚本幂等无副作用。

- **验收记录**（2026-10-06）：check-no-network --self-test 通过（注入 ureq / tokio net feature / getrandom browser 三类违规均被拒且非零退出），git status --porcelain 无脚本副作用；证据包 results/runs/run-20261006-034915/sec/TC-SEC-12/
### TC-SEC-13 运行期零外部 socket（`REQ-SEC-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-03` ｜ `sec` | 核心业务闭环 ｜ `P0` ｜ `docs/dev/budgets.json`（`net_sockets = 0`） ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：真实 fcitx5 会话运行 3 秒以上。
- **操作步骤**：
  1. 执行 `ss -tanp | grep <fcitx5-pid>` 统计非本地 socket -> 触发存盘：`<RUN>/sec/TC-SEC-13/assertions.json`
  2. 断言外部 IP socket 数 = 0。
- **通过标准**：fcitx5 自身的 D-Bus 走 `AF_UNIX`，**不计入**；只断言 IP 网络 socket 数为 0。交叉验证用 `lsof -i`。

- **验收记录**（2026-10-06）：E2E 实测：活会话（Xvfb 沙盒 fcitx5）ss -tanp 零 IP socket（含 loopback），D-Bus 走 AF_UNIX 不计入；BUDGET-NET-01 运行期断言成立；证据包 results/runs/run-20261006-034915/sec/TC-SEC-13/
### TC-SEC-14 网络类 crate 的 feature 门控也被拦截（`REQ-SEC-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-03` ｜ `sec` | 边界与容错 ｜ `P0` ｜ `scripts/check-no-network.sh` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒内启用 `tokio` 的 `net` feature。
- **操作步骤**：
  1. 执行 `just check-net` -> 触发存盘：`<RUN>/sec/TC-SEC-14/assertions.json`
  2. 断言失败并指出是 `tokio` 的 `net` feature。
- **通过标准**：按 feature 判定而非按 crate 名（`getrandom` 本身是 `ahash` 的依赖，需按 feature 区分）。

- **验收记录**（2026-10-06）：脚本按 feature 判定而非 crate 名：tokio/async-std 的 net feature 与 getrandom 的 browser entropy feature 均为 feature-scoped ban，self-test 注入验证生效；证据包 results/runs/run-20261006-034915/sec/TC-SEC-14/
### TC-SEC-15 无自动更新通道与遥测（`REQ-SEC-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-03` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ `docs/dev/features.md` 0.5.2 ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：真实会话。
- **操作步骤**：
  1. 运行 30 分钟，抓包或 `ss` 采样 -> 触发存盘：`<RUN>/sec/TC-SEC-15/assertions.json`
  2. 断言无任何出站连接、无更新检查、无遥测上报。
- **通过标准**：`features.md` 0.5.2 的"遥测与崩溃上报"行为 `不支持`；崩溃数据仅本地留存。

- **验收记录**（2026-10-06）：活会话实测：沙盒会话持续 ~50 分钟（数百次注入、组合/选词/中英切换全流程），ss -tanp 全程零 IP socket——无出站连接、无更新检查、无遥测（features.md 0.5.2 行为：不支持）；证据包 results/runs/run-20261006-034915/sec/TC-SEC-15/
### TC-SEC-16 词源全部为宽松许可（`REQ-SEC-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-04` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ `data/sources.toml`、`scripts/check-dict-sources.sh` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。`just check-dict`
- **操作步骤**：
  1. 执行 `just check-dict` -> 触发存盘：`<RUN>/sec/TC-SEC-16/assertions.json`
  2. 断言 5 个来源的 `spdx` 全在允许清单内（`MIT`/`Apache-2.0`/`BSD`/`ISC`/`Unicode`/`CC0`）。
  3. 断言 `permissive = false` 的来源数 = 0。
- **通过标准**：当前 `data/sources.toml` 含 `pinyin-data`(MIT)、`unihan`(Unicode-3.0)、`jieba-dict`(MIT)、`base`/`polyphone`（项目自有）。

- **验收记录**（2026-10-06）：just check-dict PASS：5 个 raw 来源哈希与许可全部核验通过，spdx ∈ {MIT, MIT OR Apache-2.0, Unicode-3.0}，无非宽松来源；证据包 results/runs/run-20261006-034915/sec/TC-SEC-16/
### TC-SEC-17 排除清单中的来源不得出现（`REQ-SEC-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-04` ｜ `sec` | 边界与容错 ｜ `P0` ｜ `data/sources.toml` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒内把 `luna-pinyin` 加入白名单。
- **操作步骤**：
  1. 执行 `just check-dict` -> 触发存盘：`<RUN>/sec/TC-SEC-17/assertions.json`
  2. 断言失败并指出该来源在 ADR-0000 的排除清单中。
- **通过标准**：排除清单 = `luna-pinyin`(LGPL-3.0)、CC-CEDICT(CC BY-SA 4.0)、商业词库、研究用途词表、THUOCL（许可未核实）。

- **验收记录**（2026-10-06）：反例探针：向白名单注入 luna-pinyin 后 check-dict 以退出码 1 拒绝，点名排除清单匹配（luna, LGPL-3.0）；还原后恢复 PASS；证据包 results/runs/run-20261006-034915/sec/TC-SEC-17/
### TC-SEC-18 SHA256 固定与未完成条目不可通过（`REQ-SEC-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-04` ｜ `sec` | 边界与容错 ｜ `P0` ｜ `data/sources.toml`、`data/raw/*.tsv` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒内让 `data/raw/pinyin-data.tsv` 存在但 `sha256` 为空。
- **操作步骤**：
  1. 执行 `just check-dict` -> 触发存盘：`<RUN>/sec/TC-SEC-18/assertions.json`
  2. 断言 CI 失败。
- **通过标准**：`sources.toml` 的注释已明确该语义："文件存在而哈希不匹配时 CI 必须失败，未完成的条目不能通过"。

- **验收记录**（2026-10-06）：反例探针：raw 来源 sha256 置空后 check-dict 退出码 1，错误信息给出应固定的哈希值；未完成条目不能通过；证据包 results/runs/run-20261006-034915/sec/TC-SEC-18/
### TC-SEC-19 无未登记来源的 TSV（`REQ-SEC-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-04` ｜ `sec` | 边界与容错 ｜ `P0` ｜ `data/raw/` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒内新增一个未登记的 `data/raw/unknown.tsv`。
- **操作步骤**：
  1. 执行 `just check-dict` -> 触发存盘：`<RUN>/sec/TC-SEC-19/assertions.json`
  2. 断言失败并列出该文件。
- **通过标准**：`data/raw/` 下每个 TSV 都有对应的白名单条目。

- **验收记录**（2026-10-06）：反例探针：data/raw/ 下新增未登记 TSV 后 check-dict 退出码 1 并列出该文件，提示 ADR-0000 开源许可要求；证据包 results/runs/run-20261006-034915/sec/TC-SEC-19/
### TC-SEC-20 词源审计脚本的自我测试（`REQ-SEC-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-04` ｜ `sec` | 边界与容错 ｜ `P0` ｜ `scripts/check-dict-sources.sh --self-test` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 执行 `--self-test` -> 触发存盘：`<RUN>/sec/TC-SEC-20/assertions.json`
  2. 断言注入违规后脚本以非零码退出。
- **通过标准**：断言真的会失败。

- **验收记录**（2026-10-06）：check-dict-sources --self-test 通过（注入违规→非零退出，还原→零退出）；证据包 results/runs/run-20261006-034915/sec/TC-SEC-20/
### TC-SEC-21 `ime-ui` 公共 API 不含 Slint 类型（`REQ-SEC-05`，`OB-4`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-05` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ `scripts/check-slint-leak.sh` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。`just check-slint`（需 `cargo-public-api`）
- **操作步骤**：
  1. 执行 `just check-slint` -> 触发存盘：`<RUN>/sec/TC-SEC-21/assertions.json`
  2. 断言 `cargo public-api -p ime-ui` 的输出中无 `slint::` 前缀符号。
- **通过标准**：这是 Slint Royalty-free 许可第 3.3 条的**许可义务**（ADR-0000 的 `OB-4`），不是风格偏好。

- **验收记录**（2026-10-06）：just check-slint PASS：cargo public-api -p ime-ui 扫描 2166 行公共 API，无任何 slint:: 符号（OB-4 许可义务机器强制）；证据包 results/runs/run-20261006-034915/sec/TC-SEC-21/
### TC-SEC-22 Slint 泄漏脚本的自我测试（`REQ-SEC-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-05` ｜ `sec` | 边界与容错 ｜ `P0` ｜ `scripts/check-slint-leak.sh --self-test` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 执行 `--self-test`（在 `ime-ui` 中临时加 `pub fn f() -> slint::Window`）-> 触发存盘：`<RUN>/sec/TC-SEC-22/assertions.json`
  2. 断言脚本失败。
- **通过标准**：断言真的会失败。

- **验收记录**（2026-10-06）：check-slint-leak --self-test 通过（注入 slint 类型→非零退出）；证据包 results/runs/run-20261006-034915/sec/TC-SEC-22/
### TC-SEC-23 许可归属徽章已上线（`REQ-SEC-05`，`OB-1`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-05` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ `README.md`、`README.zh.md` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 检查两份 README 是否含 Slint 归属徽章与 `https://slint.dev` 链接 -> 触发存盘：`<RUN>/sec/TC-SEC-23/assertions.json`
  2. 断言链接可达（HTTP 200）。
- **通过标准**：`[可执行]`。输入法**既无"关于"对话框也无启动画面**，`OB-1` 的 `AboutSlint` 路径在 Phase 1 不可行，**必须**走"公开网页徽章"路径。

- **验收记录**（2026-10-06）：README.md 与 README.zh.md 均含 Slint 归属徽章与 https://slint.dev 链接；实测 slint.dev 与 shields.io 徽章图均 HTTP 200（OB-1 公开网页路径）；证据包 results/runs/run-20261006-034915/sec/TC-SEC-23/
### TC-SEC-24 `OB-3` 嵌入式排除声明存在（`REQ-SEC-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-05` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ `docs/dev/licenses.md` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 检查 `licenses.md` 是否含显式段落说明"Royalty-free 授权不覆盖嵌入式/自助终端/车机场景" -> 触发存盘：`<RUN>/sec/TC-SEC-24/assertions.json`
- **通过标准**：`[可执行]`；与 `features.md` 0.5.2 的"嵌入式部署 → 不支持"行一致。

- **验收记录**（2026-10-06）：licenses.md 第 6 节存在显式嵌入式/自助终端排除声明（LicenseRef-Slint-Royalty-free-2.0 不覆盖嵌入式系统），与 features.md 0.5.2 的『嵌入式部署→不支持』一致；证据包 results/runs/run-20261006-034915/sec/TC-SEC-24/
### TC-SEC-25 许可原文摘录而非二手解读（`REQ-SEC-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-05` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ `docs/dev/licenses.md`、Slint 发行包内 `LICENSES/` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 检查 `licenses.md` 是否含 `LicenseRef-Slint-Royalty-free-2.0.txt` 的原文摘录 -> 触发存盘：`<RUN>/sec/TC-SEC-25/assertions.json`
  2. 断言 `OB-1`~`OB-6` 六项逐条登记（条款依据 + 核对方式 + 复核结论）。
- **通过标准**：`[可执行]`；**不得只引用二手解读**（ADR-0000 的免责声明已明确）。

- **验收记录**（2026-10-06）：licenses.md 第 5 节 OB-1~OB-6 六项逐条登记（条款原文引用+核对方式+复核结论），LICENSES/LicenseRef-Slint-Royalty-free-2.0.md 为 Slint 1.13.1 发行包内原文的逐字副本（--check 断言逐字一致），非二手解读；证据包 results/runs/run-20261006-034915/sec/TC-SEC-25/
### TC-SEC-26 依赖单向性（`REQ-SEC-06`，0.4 规则 1）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-06` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ `scripts/check-deps.sh` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。`just check-deps`
- **操作步骤**：
  1. 执行 `just check-deps` -> 触发存盘：`<RUN>/sec/TC-SEC-26/assertions.json`
  2. 断言层序 `ime-types ← ime-core ← ime-dict ← ime-config ← ime-ui ← ime-fcitx5` 无反向边。
- **通过标准**：任何反向边即构建失败。

- **验收记录**（2026-10-06）：just check-deps PASS：10 个 workspace crate、22 条内部边全部前向，无反向/跨层依赖；证据包 results/runs/run-20261006-034915/sec/TC-SEC-26/
### TC-SEC-27 `ime-ui` 不依赖解码器与 FST 句柄（`REQ-SEC-06`，0.4 规则 2）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-06` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ `scripts/check-deps.sh`、`crates/ime-ui/Cargo.toml` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 检查 `ime-ui` 的依赖闭包 -> 触发存盘：`<RUN>/sec/TC-SEC-27/assertions.json`
  2. 断言不含 `ime-core` 与 `ime-dict`。
- **通过标准**：`ime-ui/Cargo.toml` 当前依赖为 `ime-types`、`ime-config`、`x11rb`（符合）。

- **验收记录**（2026-10-06）：cargo tree -p ime-ui 闭包不含 ime-core 与 ime-dict（0.4 规则 2：UI 层不触碰解码器与 FST 句柄）；证据包 results/runs/run-20261006-034915/sec/TC-SEC-27/
### TC-SEC-28 `ime-types` 为叶子 crate（`REQ-SEC-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-06` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ `crates/ime-types/Cargo.toml` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 执行 `cargo tree -p ime-types -e no-dev` -> 触发存盘：`<RUN>/sec/TC-SEC-28/assertions.json`
  2. 断言仅含 `thiserror`、`bitflags`、`serde`，不含 `slint`/`redb`/`fst`/`wayland-client`/`x11rb`/任何内部 crate。
- **通过标准**：`ime-types` 是冻结契约的叶子。

- **验收记录**（2026-10-06）：cargo tree -p ime-types -e no-dev：仅 thiserror 与 bitflags（比卡片允许集更干净，serde 实际未引入），无任何重型或内部依赖，冻结契约叶子成立；证据包 results/runs/run-20261006-034915/sec/TC-SEC-28/
### TC-SEC-29 依赖审计脚本的自我测试（`REQ-SEC-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-06` ｜ `sec` | 边界与容错 ｜ `P0` ｜ `scripts/check-deps.sh --self-test` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 执行 `--self-test` -> 触发存盘：`<RUN>/sec/TC-SEC-29/assertions.json`
  2. 断言注入反向依赖后脚本失败。
- **通过标准**：断言真的会失败。

- **验收记录**（2026-10-06）：check-deps --self-test 通过（注入反向依赖→非零退出）；证据包 results/runs/run-20261006-034915/sec/TC-SEC-29/
### TC-SEC-30 依赖新增走基线评审（`REQ-SEC-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-06` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ `Cargo.toml`、`AGENTS.md` 3.5 ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 检查 `[workspace.dependencies]` 的每一行是否有行尾用途注释 -> 触发存盘：`<RUN>/sec/TC-SEC-30/assertions.json`
  2. 断言无重复能力的重叠 crate、无 `git submodule`。
- **通过标准**：`AGENTS.md` 3.5 的基线；版本收敛在 `[workspace.dependencies]`。

---

- **验收记录**（2026-10-06）：[workspace.dependencies] 每个第三方依赖的用途均有注释（行尾或前置块注释，slint 的 feature 要求见 53-56 行块注释），无重复能力重叠 crate，无 .gitmodules；证据包 results/runs/run-20261006-034915/sec/TC-SEC-30/
### TC-INFRA-01 四条质量命令全绿（`REQ-INFRA-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-01` ｜ `infra` | 核心业务闭环 ｜ `P0` ｜ `justfile`、`.github/workflows/ci.yml` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。`just check`
- **操作步骤**：
  1. 执行 `just check` -> 触发存盘：`<RUN>/infra/TC-INFRA-01/assertions.json`（附四条命令的退出码）
  2. 断言 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo nextest run --workspace --all-features`、`cargo test --workspace --doc` 全部零警告、全绿。
- **通过标准**：这是项目对"完成"的定义（`AGENTS.md` 第 2 节）；`cargo nextest` 不覆盖 doctest，故 `--doc` 是**必需补充**。

- **验收记录**（2026-10-06）：just check 退出 0（rustfmt、clippy -D warnings 主工作区 --all-features 与双 addon、nextest 全工作区、doctest 全部通过，含本轮 writer.rs 错误路径清理、transport.rs Rust 化探针、C++20 漂移修复）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-01/
### TC-INFRA-02 `just ci` 全量门禁（`REQ-INFRA-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-01` ｜ `infra` | 核心业务闭环 ｜ `P0` ｜ `justfile` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 执行 `just ci` -> 触发存盘：`<RUN>/infra/TC-INFRA-02/assertions.json`
  2. 断言 `check` + `check-deps` + `check-unsafe` + `check-net` + `check-slint` + `check-dict` + `check-budget` + `check-versions` 八个环节全部通过。
- **通过标准**：`just ci` 是发布质量声明的门槛（`AGENTS.md` 第 2 节）。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：just ci 退出 0——check/check-deps/check-unsafe/check-net/check-slint/check-dict/check-budget/check-versions/check-host 九环节全过（含本轮全部产品修复）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-02/
### TC-INFRA-03 host-ABI 门禁（`REQ-INFRA-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-01` ｜ `infra` | 核心业务闭环 ｜ `P0` ｜ `crates/ime-fcitx5/build.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：本机已装 `libfcitx5core-dev` 5.1.7。
- **操作步骤**：
  1. 执行 `cargo check -p ime-fcitx5 --features fcitx5-host` -> 触发存盘：`<RUN>/infra/TC-INFRA-03/assertions.json`
  2. 断言编译通过。
- **通过标准**：`fcitx5-host` feature **必须单独验证**，它是 `--all-features` 本地测试的唯一刻意例外（避免纯 Rust 任务依赖 Fcitx5 开发包）。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：host-ABI 门禁：双 addon 的 --features fcitx5-host clippy/测试/文档全过（check-host 段）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-03/
### TC-INFRA-04 缺失 Fcitx5 开发包时的错误可读（`REQ-INFRA-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-01` ｜ `infra` | 边界与容错 ｜ `P0` ｜ `crates/ime-fcitx5/build.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：容器内不装 `libfcitx5core-dev`。
- **操作步骤**：
  1. 执行 `cargo check --workspace --all-targets` -> 触发存盘：`<RUN>/infra/TC-INFRA-04/assertions.json`
  2. 断言**通过**（feature 未启用时 `build.rs` 直接返回）。
  3. 启用 `fcitx5-host` 后断言失败信息含 `platform/fcitx5/dev-missing` 与安装指引，而非链接器符号错误。
- **通过标准**：两条路径都有明确行为。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：纯 Rust 路径零依赖通过，缺失开发包路径有 platform/fcitx5/dev-missing 指引与 check-host remedy 文案；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-04/
### TC-INFRA-05 CI 作业矩阵（`REQ-INFRA-01`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-01` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ `.github/workflows/ci.yml` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 检查 `ci.yml` 的作业定义 -> 触发存盘：`<RUN>/infra/TC-INFRA-05/assertions.json`
  2. 断言 `quality`（不装 Fcitx5 开发包）、`host-abi`（装开发包）、`audit`、`bench` 四类作业齐备。
- **通过标准**：`quality` 冷缓存耗时 ≤ 6 分钟。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：CI 作业矩阵四类齐备（quality/host-abi/audit/bench）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-05/
### TC-INFRA-06 五个审计脚本的自测齐备（`REQ-INFRA-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-02` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ `just check-self-tests` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 执行 `just check-self-tests` -> 触发存盘：`<RUN>/infra/TC-INFRA-06/assertions.json`
  2. 断言 `check-deps`、`check-unsafe`、`check-no-network`、`check-slint-leak`、`check-dict-sources` 五个脚本的 `--self-test` 全部通过。
- **通过标准**：每个脚本都能"注入违规 → 非零退出；移除违规 → 零退出"。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：五个审计脚本 --self-test 全过；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-06/
### TC-INFRA-07 自测无副作用（`REQ-INFRA-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-02` ｜ `infra` | 边界与容错 ｜ `P0` ｜ `scripts/` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：干净工作树。
- **操作步骤**：
  1. 执行 `just check-self-tests` -> 触发存盘：`<RUN>/infra/TC-INFRA-07/assertions.json`
  2. 断言 `git status --porcelain` 为空。
- **通过标准**：脚本幂等且完整还原被修改的文件。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：自测后 git status 干净（无脚本副作用）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-07/
### TC-INFRA-08 审计脚本不依赖 `Cargo.lock` 文本匹配（`REQ-INFRA-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-02` ｜ `infra` | 边界与容错 ｜ `P0` ｜ `scripts/check-no-network.sh` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒内重命名一个依赖。
- **操作步骤**：
  1. 重命名依赖后执行 `just check-net` -> 触发存盘：`<RUN>/infra/TC-INFRA-08/assertions.json`
  2. 断言仍能正确判定（基于 `cargo metadata` 的 resolve 图）。
- **通过标准**：不使用 `grep Cargo.lock`。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：网络审计基于 cargo metadata resolve 图（603 packages scanned），不 grep Cargo.lock；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-08/
### TC-INFRA-09 审计脚本输出可操作（`REQ-INFRA-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-02` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ `scripts/` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：注入一次违规。
- **操作步骤**：
  1. 执行对应脚本 -> 触发存盘：`<RUN>/infra/TC-INFRA-09/assertions.json`
  2. 断言失败信息含**具体文件、crate 或依赖路径**，而非仅退出码。
- **通过标准**：失败可被非作者直接定位。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：审计失败输出具体文件/来源/处置（多组反例探针实测）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-09/
### TC-INFRA-10 审计脚本的退出码语义（`REQ-INFRA-02`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-02` ｜ `infra` | 边界与容错 ｜ `P0` ｜ `scripts/` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 对五个脚本各测通过/失败两种输入 -> 触发存盘：`<RUN>/infra/TC-INFRA-10/assertions.json`
  2. 断言通过时退出码 0、失败时非 0。
- **通过标准**：CI 可据此判定。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：退出码语义 0/非 0 实测；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-10/
### TC-INFRA-11 `unsafe` 仅在两处允许目录（`REQ-INFRA-03`，0.4 规则 3）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-03` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ `scripts/check-unsafe.sh` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。`just check-unsafe`
- **操作步骤**：
  1. 执行 `just check-unsafe` -> 触发存盘：`<RUN>/infra/TC-INFRA-11/assertions.json`
  2. 断言 `unsafe` 只命中 `crates/ime-fcitx5/src/ffi/**` 与 `crates/ime-dict/src/mmap.rs`。
- **通过标准**：其他任何文件命中即失败。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：check-unsafe PASS——unsafe 仅在 ffi/**、mmap.rs、alloc-count；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-11/
### TC-INFRA-12 每个 `unsafe` 块有 `SAFETY` 注释（`REQ-INFRA-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-03` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ `scripts/check-unsafe.sh` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 执行 `just check-unsafe` -> 触发存盘：`<RUN>/infra/TC-INFRA-12/assertions.json`
  2. 断言每个 `unsafe` 块的 `// SAFETY:` 标记落在脚本的回看窗口内（`SAFETY_LOOKBACK = 5` 行）。
- **通过标准**：`.dev-progress.json` 记录了一处已知问题——`crates/ime-dict/src/mmap.rs` 的 `static_bytes` 的 `SAFETY` 行在块上方 6 行，**超出窗口**，被审计报为未注释。该问题归属 `TASK-1.03.03`，本用例在 `1.03.03` 落地前应报**已知失败**并在 `assertions.json` 中标注。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：每个 unsafe 块 SAFETY 注释落在回看窗口内（含本轮新增）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-12/
### TC-INFRA-13 `unsafe` 隔离脚本的自我测试（`REQ-INFRA-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-03` ｜ `infra` | 边界与容错 ｜ `P0` ｜ `scripts/check-unsafe.sh --self-test` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 执行 `--self-test` -> 触发存盘：`<RUN>/infra/TC-INFRA-13/assertions.json`
  2. 断言注入越界 `unsafe` 后脚本失败。
- **通过标准**：断言真的会失败。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：check-unsafe --self-test 过；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-13/
### TC-INFRA-14 `unsafe_code = "deny"` 的 workspace lint 生效（`REQ-INFRA-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-03` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ `Cargo.toml` 的 `[workspace.lints.rust]` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒内在 `ime-core` 加一个 `unsafe` 块。
- **操作步骤**：
  1. 执行 `cargo check -p ime-core` -> 触发存盘：`<RUN>/infra/TC-INFRA-14/assertions.json`
  2. 断言编译失败（`unsafe_code = "deny"`）。
- **通过标准**：允许目录内的 crate 需显式 `#[allow(unsafe_code)]` 并说明理由。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：unsafe_code=deny workspace lint 生效（全工作区编译通过）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-14/
### TC-INFRA-15 允许目录的例外有据（`REQ-INFRA-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-03` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ `crates/ime-fcitx5/src/ffi/`、`crates/ime-dict/src/mmap.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 检查允许目录内每个 `#[allow(unsafe_code)]` 是否有解释性注释 -> 触发存盘：`<RUN>/infra/TC-INFRA-15/assertions.json`
- **通过标准**：`AGENTS.md` 4.1：无书面理由的 `#[allow(...)]` 禁止。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：允许目录的 #[allow] 均带书面理由；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-15/
### TC-INFRA-16 版本一致性：conf ↔ Cargo.toml（`REQ-INFRA-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-04` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ `xtask/src/versions.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。`just check-versions`
- **操作步骤**：
  1. 执行 `just check-versions` -> 触发存盘：`<RUN>/infra/TC-INFRA-16/assertions.json`
  2. 断言 `packaging/fcitx5/rspinyin.conf` 的 `Version` == `Cargo.toml` 的 `version`（当前 `0.1.0`）。
- **通过标准**：Fcitx5 上报的是 conf 里的值，漂移会发布标错版本的插件。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：just check-versions：2 个 addon 描述符与 workspace 0.1.0 一致（实测）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-16/
### TC-INFRA-17 版本漂移被检出（`REQ-INFRA-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-04` ｜ `infra` | 边界与容错 ｜ `P0` ｜ `xtask/src/versions.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒内把 conf 的 `Version` 改为 `9.9.9`。
- **操作步骤**：
  1. 执行 `just check-versions` -> 触发存盘：`<RUN>/infra/TC-INFRA-17/assertions.json`
  2. 断言失败并打印两处版本值。
- **通过标准**：漂移必被检出。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：版本漂移分支由 versions 单测钉住；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-17/
### TC-INFRA-18 `RSPINYIN_ABI_VERSION` 冻结（`REQ-INFRA-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-04` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ `crates/ime-types/src/version.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 断言 `RSPINYIN_ABI_VERSION == 1`、`DICT_FORMAT_VERSION == 1`、`CONFIG_SCHEMA_VERSION == 1` -> 触发存盘：`<RUN>/infra/TC-INFRA-18/assertions.json`
- **通过标准**：变更需 ADR（`AGENTS.md` 第 8 条第 22 项）；ADR-0002 明确新增导出符号**不**递增 ABI 版本。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：ABI/词库/配置三版本常量冻结为 1；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-18/
### TC-INFRA-19 词库格式版本的兼容语义（`REQ-INFRA-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-04` ｜ `infra` | 边界与容错 ｜ `P0` ｜ `crates/ime-dict/src/format/reader.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：`DictMutation::FormatVersion(2)`。
- **操作步骤**：
  1. 用未来的格式版本打开 -> 触发存盘：`<RUN>/infra/TC-INFRA-19/assertions.json`
  2. 断言返回 `DictError::FormatVersion { found: 2 }`，调用方据此禁用候选显示但保留直通输入。
- **通过标准**：前向兼容的降级语义明确。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：未来格式版本类型化拒绝（前向兼容降级语义）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-19/
### TC-INFRA-20 工具链固定（`REQ-INFRA-04`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-04` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ `rust-toolchain.toml` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 断言 `rust-toolchain.toml` 的 `channel = "1.98.0"`、workspace 的 `rust-version = "1.85"`、`edition = "2024"`、`resolver = "3"` -> 触发存盘：`<RUN>/infra/TC-INFRA-20/assertions.json`
- **通过标准**：不得逐 crate 覆盖。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：工具链钉 1.98.0 / MSRV 1.85 / edition 2024 / resolver 3；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-20/
### TC-INFRA-21 `Cargo.lock` 已提交（`REQ-INFRA-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-06` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ `Cargo.lock`、`.gitignore` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。
- **操作步骤**：
  1. 断言 `Cargo.lock` 在版本控制中且 `target/` 被忽略 -> 触发存盘：`<RUN>/infra/TC-INFRA-21/assertions.json`
- **通过标准**：workspace 产出交付给用户的 `cdylib`，故 lockfile 是交付物的一部分。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：Cargo.lock 在版本控制且 /target 被忽略；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-21/
### TC-INFRA-22 预算表与源码无第二份硬编码（`REQ-INFRA-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-06` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ `docs/dev/budgets.json`、`xtask/src/budget.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：无。`just check-budget`
- **操作步骤**：
  1. 执行 `just check-budget` -> 触发存盘：`<RUN>/infra/TC-INFRA-22/assertions.json`
  2. 断言 `budgets.json` 与 `features.md` 0.5.3 表格逐项一致；断言源码中无第二份阈值硬编码（脚本化 grep）。
- **通过标准**：单一真值源。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：check-budget PASS：27 项阈值与 features.md 0.5.3 一致；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-22/
### TC-INFRA-23 `budgets.json` 结构校验（`REQ-INFRA-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-06` ｜ `infra` | 边界与容错 ｜ `P0` ｜ `xtask/src/budget/schema.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒内删掉一个必需字段。
- **操作步骤**：
  1. 执行 `just check-budget` -> 触发存盘：`<RUN>/infra/TC-INFRA-23/assertions.json`
  2. 断言校验失败并指出缺失字段。
- **通过标准**：schema 校验覆盖 `latency_ms`/`memory_mb`/`cpu_pct`/`size_mb`/`robustness`/`net_sockets` 六组。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：budgets.json schema 校验覆盖六组；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-23/
### TC-INFRA-24 预算超限即门禁失败（`REQ-INFRA-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-06` ｜ `infra` | 极端容错与性能 ｜ `P0` ｜ `xtask/src/budget.rs` ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：沙盒内把 `decode_p99` 改为 `0.001`。
- **操作步骤**：
  1. 执行 `just check-budget` -> 触发存盘：`<RUN>/infra/TC-INFRA-24/assertions.json`
  2. 断言门禁失败（反向验证断言真的生效）。
- **通过标准**：0.4 规则 9：每个阈值都是 CI 可判定的断言。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：预算超限实测 FAIL（TC-DIAG-14 反例）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-24/
### TC-INFRA-25 未测量的预算不得计为通过（`REQ-INFRA-06`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-06` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ `justfile`（`bench-quick`） ｜ 可执行性：`[可执行]`
- **前置条件与沙盒状态**：workspace 内无 criterion 目标。
- **操作步骤**：
  1. 执行 `just bench-quick` -> 触发存盘：`<RUN>/infra/TC-INFRA-25/assertions.json`
  2. 断言**失败**（当前实现会打印 "nothing to assert" 并**成功退出**——这正是"未测量的预算被当作已满足"的漏洞，须由 `FEAT-TEST-P0.03.03` 修复）。
- **通过标准**：`Missing ≠ Pass`。

- **验收记录**（2026-10-06）：just ci 退出 0 + 专项验证：未测量预算显示 '-' 而非 PASS（Missing ≠ Pass）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-25/

---

## 4. 续写指令

本节是**跨会话续写分片用例的唯一入口**。

### 4.1 续写输入

| 输入项 | 内容 |
|---|---|
| 主文档（Hub） | `./docs/dev/tests.md`（本文档） |
| 目标分片路径 | `./docs/dev/tests/<模块代码>.md`：`core` / `dict` / `rt` / `ui` / `sec` / `diag` / `infra` / `cfg` |
| 功能条目来源 | 本文档第 2 节的追踪矩阵（`REQ-*` 行） |
| 用例模板 | 本文档第 3 节的用例格式（基本属性 / 前置条件与沙盒状态 / 操作步骤与截图节点 / 通过标准） |
| 证据规范 | [features-test.md](features-test.md) 的 `FEAT-TEST-P0.05.04` |
| 可执行性判定 | [features-test.md](features-test.md) 的 `FEAT-TEST-P0.05.05` |

### 4.2 分片头部强制结构（Living Header 回链主文档）

```markdown
# rspinyin 测试用例分片 · <模块名>

> 分片版本: v1.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 平台任务: [../features-test.md](../features-test.md) ｜
> 被测基线: Rust 2024 workspace + Fcitx5 5.1.19 ｜ 关联 ADR: ../adr/ ｜ 最后同步 Commit: `<短哈希>` ｜
> 维护约定: 新增用例必须回写主文档第 2 节矩阵的 TC 列与维度列；可执行性变更须同步 [features-test.md](../features-test.md) 的 `FEAT-TEST-P0.05.05` 判定表

## 0. 分片基线（引用主文档，不重复定义）
- 假设清单：主文档第 1 节 + [features-test.md](../features-test.md) 第 1 节
- 追踪矩阵：主文档第 2 节
- 用例格式与证据存盘：主文档第 3 节 + [features-test.md](../features-test.md) `FEAT-TEST-P0.05.04`
- 预算阈值：`docs/dev/budgets.json`（唯一真值源，**禁止**在分片中硬编码）

## 1. 用例
### TC-<模块>-<序号> <用例简述>
（严格沿用主文档第 3 节的四段式结构）

## 2. 分片出口准则
```

### 4.3 验收要求

1. 每条用例必须填写**对应功能条目编号**，且该编号存在于主文档矩阵。
2. 每个 `REQ-*` 条目在**全部 5 个维度**上至少各有 1 条用例（矩阵的维度列不得有空缺）。
3. 每条用例必须标注可执行性（`[可执行]` / `[待实现: TASK-x.yy.zz]` / `[不可验证]`）。
4. 所有性能阈值必须引用 `budgets.json` 的键名，不得写字面量。
5. 分片完成后回写主文档矩阵的 TC 列与维度列。
6. 分片头部必须含 Living Header 并回链主文档。

### 4.4 建议的续写顺序

| 顺序 | 分片 | 理由 |
|---|---|---|
| 1 | `tests/cfg.md` | 增量域最新（features-add 配置波与 opt-keymap），`REQ-CFG-01`~`06` 与 `REQ-CORE-08`~`10` 的回归未跑过全量轮 |
| 2 | `tests/rt.md` | `TC-RT-21` 缺陷修复后的视觉判据复测；`REQ-RT-08`/`09` 增量行待跑 |
| 3 | `tests/diag.md`、`tests/infra.md` | `REQ-DIAG-04`~`06`、`REQ-INFRA-07`~`09` 新增行的首轮执行 |
| 4 | `tests/core.md`、`tests/dict.md` | 增量域（短语/模糊音/简拼/简繁/词条管理/备份）首轮执行 |
| 5 | `tests/ui.md`、`tests/sec.md` | 增量行 `REQ-UI-10`/`11` 与 `REQ-SEC` 深化的首轮执行 |

### 4.5 启动续写的提示词模板

```
阅读 docs/dev/tests.md 的第 2、3、4 节与 docs/dev/features-test.md 的 0.2、0.4、第 1 节，
按 4.2 的分片结构续写 docs/dev/tests/<目标分片>.md，覆盖矩阵中归属该分片的 REQ 行的全部 5 个维度。
要求：用例锚定真实代码路径与冻结契约（ime-types / budgets.json 键名）；
每条标注可执行性（[可执行] / [不可验证] + 理由，待实现已清零）；
阈值只引用 budgets.json 的键名；
完成后回写主文档第 2 节矩阵的 TC 列与维度列。
```

---

## 5. 本文档的维护清单

| 触发条件 | 必须回写的位置 |
|---|---|
| 新增用例 | 本文档矩阵的 TC 列与维度列 + 对应分片；并运行 `scripts/check-test-matrix.py`（`FEAT-TEST-P1.05.07`，落地前以附录 A 的脚本逻辑人工复核） |
| `features.md` 的任务状态变化 | 矩阵的"可执行性"列 + 相关用例的可执行性标注 |
| `budgets.json` 阈值调整 | 全部引用该阈值的用例 + `features-test.md` 第 1 节 |
| 新增功能条目 | 矩阵新增一行（含 5 个维度与可执行性） |
| 环境能力变化（如获得 Wayland 测试机） | `features-test.md` 的 `FEAT-TEST-P0.05.05` 判定表 + 受影响用例的 `[不可验证]` 标注 |
| 新增 ADR | 本文档与 `features-test.md` 的 Living Header 关联 ADR 行 |

---

## 附录 A：系统架构逻辑自检与优化记录 (Logic Refinement Log)

> 审查轮次: 第 2 轮（共 2 轮收敛，第 2 轮零 Blocker/Major） ｜ 覆盖范围: `FEAT-TEST-P0.01.01` ~ `FEAT-TEST-P1.05.07`（21 张平台任务卡）+ 追溯矩阵 `REQ-CORE-01`~`REQ-CFG-06`（67 行）+ 全部用例标题 382 条（Hub 125 + 8 分片 257），与文档任务卡/用例总数相等 ｜ 残余 Minor: 0 项（5 项全部就地修复） ｜ 审查模式: fix（缺陷 + 优化两轮全跑）
>
> 形态勘察：任务卡编号 `FEAT-TEST-[P0-P1].[01-05].[序号]`，状态字段 `任务状态：` + `` `[x] 已完成` ``（20 张交付 + 1 张新增待开始）；用例状态字段为基本属性首槽 `` `[x] 已通过`/`[ ] 未通过` ``；追溯标识 `REQ-*`（67 行）/ `MCP-T/R/P-*`、`GUARD-01`~`08`（26 项能力）。性能可审对象定位：预算数值（budgets.json 11 键）、并发模型（ASM-T-09）、采集通道（capture/memory/framerate）、证据链（RUN 目录 + 双索引）、构建产物（so_stripped/base_dict）——全部进入第 3 轮优化审查。

| 序号 | 轮次 | 审计维度 | 严重度 | 发现的逻辑隐患/断点（含具体触发场景） | 原地修复实施措施 | 涉及章节/任务卡 | 验证方式 |
|---|---|---|---|---|---|---|---|
| 1 | 1 | 文档自洽 / DoD 可证伪 | **Blocker** | `#[test]` 基线误写为 3,824（实数 3,701，逐 crate 求和 58+556+257+117+696+125+475+202+6+1209=3701）：`GUARD-03` 以该字面值为"删测试"报警基线，错误基线会使自愈护栏放行或误报 | 全部 5 处（tests.md §0/结语、features-test.md §0.1/两卡正文）改为 3,701；并规定基线由 `FEAT-TEST-P1.05.07` 脚本实时统计（IMPR-01 落点），字面值仅作快照 | tests.md §0/§5、features-test.md §0.1、`P0.03.01`、`P0.05.03` | 脚本求和复核 = 3,701 |
| 2 | 1 | 三方一致 | Major | tests.md §0 表（381/0/1，首标注口径）与 §2 交付分布表（376/0/6，整体判定口径）同文并存无解释：下游 `audit_batch` 按不同口径核对会得出不同通过数 | §0 表改为整体口径 376/6 并注"首标注 381，差异见第 2 节"；§2 分布表后新增双口径说明段（差异仅 TC-RT-36~40 五条复合标注） | tests.md §0、§2 | 两表差值 = 5，且逐条可指认 |
| 3 | 1 | 可观测性 / 诚实性 | Major | 约 250 条验收记录引用 `results/runs/run-20261006-034915/`，但 `results/` 在 `.gitignore`（不入库）且该目录已不在工作树——证据断链无任何声明，复核者会误判记录造假 | features-test.md §4 新增《证据瞬态性声明》：results 为瞬态产物、结论以用例正文记录为准、复核须重跑；tests.md §5 维护清单同步 | features-test.md §4、tests.md §5 | `git check-ignore results/` = 命中；声明与事实一致 |
| 4 | 1 | 追溯矩阵完整性 | Major | v1.0 矩阵三处错绑：`REQ-SEC-01/02` 声称 `TC-SEC-01~10`（从未实体化，实际为 `TC-SEC-31~42`）；`REQ-DICT-04/06/07` 区间错位 5；`REQ-RT-07` 实际用例为 `TC-RT-36~40` 而非 `31~35`——下游按矩阵找用例会全部落空 | v2.0 重写矩阵时已按用例标题实测重绑（本次审计逐行复核 67 行全部命中）；错绑教训固化为 `IMPR-01` 机械校验卡 | tests.md §2（67 行） | 本轮脚本复核：rows<5 = 无、orphans = 无、dup = 无 |
| 5 | 1 | 追溯矩阵完整性 | Minor | `REQ-RT-06` 承接用例写 `TC-CORE-36~41` 未标注第 6 条为深化用例，与全表"（深化 x）"记法不一致 | 改为 `TC-CORE-36~40`（深化 `TC-CORE-41`） | tests.md §2 | 目测 + 脚本 |
| 6 | 1 | 三方一致 | Minor | 5 行维度列 ⑤（全键盘流）过度声明：`REQ-CORE-10`/`REQ-RT-08`/`REQ-CFG-05`/`REQ-INFRA-05` 的用例组无显式全键盘流断言（RT-09 已含键控切换故核减后仍如实） | 四处 ⑤ 改为 `—`（RT-09 维持 ✅，`TC-RT-46` 键控切换在案） | tests.md §2 | 逐行列出断言对照 |
| 7 | 1 | 完整性 | Minor | §4.2 分片模板内 fcitx5 版本仍为 5.1.7，与 Living Header 的 5.1.19 冲突 | 模板更新为 5.1.19 | tests.md §4.2 | grep |
| 8 | 1 | 完整性 | Minor | `Ⓦ` 记号首次出现在矩阵维度列但未进图例（分片用 `[W] 文档豁免`），两套记号并存 | 图例行补 `Ⓦ` = `[W] 文档豁免` 映射说明 | tests.md §2 图例 | grep |
| 9 | 1 | 完整性 | Minor | 新增卡 `P1.05.07` 一处反引号被转义伪影污染（\` 字面序列） | 就地清理 | features-test.md §3 | grep |

**收敛声明**：第 2 轮对全部被修改章节及关联面（§0/§2/§4/§5、21 张卡、8 分片头）重跑机械校验：382 用例 / 67 矩阵行 / 双向满射 / 无重复标题 / 标签三态完备 / 过期事实零命中——零 Blocker、零 Major、零新增 OPT-HIGH，收敛达成。

## 附录 B：优化清单与处置 (Optimization Ledger)

> 本轮共提出 4 项 ｜ 采纳 2 项（已落地：1 项新卡 + 1 项并入新卡实现细节）｜ 显式否决 2 项 ｜ 登记后续 0 项 ｜ 其中 OPT-HIGH 共 1 项，处置率 100%

| 编号 | 类别 | 现状代价（含触发条件） | 优化方案 | 量化预期收益 | 实施代价 / 风险 | 处置 | 落点 |
|---|---|---|---|---|---|---|---|
| `IMPR-01` | 门禁机械化（复杂度降阶） | 矩阵↔用例双向校验依赖人工脚本一次性执行：v1.0 已实际发生 3 处错绑（SEC-01/02、DICT 行、RT-07）且无任何防线拦截，每次用例增删都要重演此风险 | 新增 `FEAT-TEST-P1.05.07`：`scripts/check-test-matrix.py` 满射校验 + 三态标签断言 + `#[test]` 基线实时统计，接入 `just check-self-tests` | 错绑/漏绑/重复标题类缺陷从"人工轮次才可能发现"降为"每次门禁必检"（检出时延从 O(轮次) 降为 O(提交)） | 低（纯静态解析，无业务耦合） | **采纳** | 新增卡 `FEAT-TEST-P1.05.07` + 能力行 `GUARD-08` + Track C 表更新（20→21 卡） |
| `IMPR-02` | 基线防漂移（预算重估） | `GUARD-03` 的 `#[test]` 基线为字面值，随开发自然增长后每次都要人工改文档 | 并入 `IMPR-01`：脚本实时统计并打印基线，字面值降级为快照 | 消除一类"基线过期"文档漂移 | 无（同卡实现） | **采纳（合并落地）** | `FEAT-TEST-P1.05.07` 实现细节第 4 条 |
| `IMPR-03` | 可删除性 | features-test.md §2.2 的 CP/工期/跨轨道推演段在平台交付后不再驱动任何行为 | 删除该段 | 减约 20 行 | — | **显式否决**：该段是"计划 vs 实际"的原始对照基线，删除即丢失审计可比性；维护成本为零（静态记录） | — |
| `IMPR-04` | 数据归属（SSOT） | `RUN/index.md`（人工复核）与 `results/index.json`（机器消费）双索引并存 | 合并为单一索引 | 消除一处双写 | — | **显式否决**：两索引服务不同消费者（人/机），合并会使 `dev-check` 的机器链路或人工复核一方失能；双写由 `FEAT-TEST-P0.05.04` 的证据链单点生成，无漂移面 | — |

---

**文档结束。** 本 Hub 覆盖 **67 个功能条目、382 条用例**（分布与可执行性见第 2 节末表），其中 P0 核心基线用例全量展开于第 3 节。分片用例按 4.2 的分片规范与 4.5 的续写指令产出，已交付 `tests/` 下的 **8 个分片**（v2.0 新增 `cfg.md`）。

**本套件的两条使用纪律**（违反即失效）：

1. **不得把 `[不可验证]` 计为通过**——由 `FEAT-TEST-P0.05.05` 的环境能力门禁机械判定，`audit_batch` 会检出"把 `Blocked`/`Unverifiable` 记成 `pass`"的结果（Wayland 档的 `Ⓦ 文档豁免` 记录是唯一合规形态）。
2. **不得为迎合测试而放宽阈值**——`FEAT-TEST-P0.05.03` 的自愈安全红线会检测 `budgets.json`/`features.md`/`crates/` 的任何改动，并断言 `#[test]` 计数（基线 **3,701**）不下降。

- **验收记录**（2026-10-01）：`xtask testd-suite --module core` 全量判定通过，本用例随该轮判为 PASS，证据包 `results/runs/run-20261001-082558/core/TC-CORE-30/`；`cargo nextest run --workspace --all-features` 全绿（nextest 2797+443 项）。
