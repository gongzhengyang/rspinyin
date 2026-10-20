# rspinyin 端到端测试用例套件（tests.md · Hub v3.0）

> 文档版本: v3.0 ｜ 系统形态: Hybrid（Desktop GUI 自绘候选窗 + Headless 引擎/CLI）｜ 选定测试框架: 自研 `xtask testd` 平台 + `cargo nextest`/`criterion`/`alloc-count` ｜ 关联架构规范: [./test-architecture.md](test-architecture.md) ｜ 平台任务: [./features-test.md](features-test.md) ｜ 逻辑自检: 已通过（七维基线与正则对账闭环）｜ 维护约定: 被测代码演进后必须回写 §2 矩阵的 TC 列与维度列；用例状态变更须同步本矩阵与对应分片

**⚠️ AI 测试执行器（`dev-check`）强制前置阅读门禁：执行本套件任何用例前，必须首先完整阅读 [./test-architecture.md](test-architecture.md)，严格按其 §4 SOP 启动 SUT、初始化沙盒、执行分级调度与残留清理；其 §3 原语映射表是与平台驱动的唯一协议契约。**

**Hub & Spoke**：本文档只承载矩阵、假设与索引，不含用例卡正文。全部用例卡按模块分片：[`tests/core.md`](tests/core.md)、[`tests/dict.md`](tests/dict.md)、[`tests/rt.md`](tests/rt.md)、[`tests/ui.md`](tests/ui.md)、[`tests/sec.md`](tests/sec.md)、[`tests/diag.md`](tests/diag.md)、[`tests/infra.md`](tests/infra.md)、[`tests/cfg.md`](tests/cfg.md)、[`tests/add.md`](tests/add.md)（features-add 增量面，全部 `[待实现]`）。

**证据存盘**：`<RUN>` = `results/runs/run-[YYYYMMDD-HHMMSS]`；用例证据包 `<RUN>/<模块代码>/<TC-ID>/`；字段契约见 [test-architecture.md](test-architecture.md) §5.1。

---

## 0. 覆盖状态与可执行性声明

| 类别 | 含义 | 用例数（v3.0） |
|---|---|---|
| `[可执行]` | 被测代码存在，本机可判定 | 73 |
| `[待实现: TASK-x.xx.xx]` | 锚定 features-add 任务，被测面未合入 | 12 |
| `[不可验证]` | 代码存在但本机无判定环境（Wayland 四档） | 1（`TC-RT-008`；另有若干**子项**级标注） |

**v3.0 重建声明**：本套件由 v2.0（382 条，`74afea9` 基线）按 5 维高内聚原则**合并重建**为 86 张卡；每张卡头部「吸收」字段登记其取代的 v2.0 用例号，v2.0 的验收记录与证据包（`results/runs/`）原样保留作为历史基线。v2.0 的唯一红灯 `TC-RT-21`（depth-24 回退路径黑屏）在 `TC-RT-005` 中以失败隔离延续。单元面基线 **3,701 `#[test]`** 不变，由各卡「背书」字段引用。

---

## 1. 系统设计假设清单（ASM-T-01 ~ ASM-T-11）

全文见 [features-test.md](features-test.md) §1。摘要（阈值一律只引 `budgets.json` 键名）：`ASM-T-01` 双 addon 进程内形态 ｜ `ASM-T-02` 无 DOM/A11y，语义面 = `UiFrame` 镜像 ｜ `ASM-T-03` WSLg/Weston，X11 可测 Wayland 不可 ｜ `ASM-T-04` 延迟预算键（`key_to_present_p99` 等）｜ `ASM-T-05` 空闲预算键 ｜ `ASM-T-06` 内存/体积预算键 ｜ `ASM-T-07` 合成词库封闭测试 ｜ `ASM-T-08` 本机不可验证档清单 ｜ `ASM-T-09` 无异步运行时 ｜ `ASM-T-10` raw≤64/候选≤45 边界 ｜ `ASM-T-11` 空闲机器采信纪律。

---

## 2. 功能-代码-用例全景追踪矩阵

**维度图例**：① 核心业务通路 ② 边界与容错 ③ 全状态防御 ④ 工艺质感/性能韧性 ⑤ 人机工学/控制流。分级：S=Smoke / A=Acceptance / T=Stress。

### 2.1 引擎与数据（Headless 面）

| 需求编号 | 功能清单条目 | 关联真实源码路径 | 承接平台任务 | 承接用例 (TC) | 维度 | 分级 | 可执行性 |
|---|---|---|---|---|---|---|---|
| `REQ-CORE-01` | 音节表与输入规范化 | `crates/ime-core/src/segment/syllable.rs` | `P0.02.01` | `TC-CORE-001` | ①②③ | S | ✅ |
| `REQ-CORE-02` | 切分 DAG 与非法串保护 | `crates/ime-core/src/segment/dag.rs` | `P0.02.01` | `TC-CORE-002` | ①② | S | ✅ |
| `REQ-CORE-03` | 输入缓冲与按音节删除 | `crates/ime-core/src/input/buffer.rs` | `P0.02.01` | `TC-CORE-003` | ①②⑤ | A | ✅ |
| `REQ-CORE-04` | K-best Viterbi 解码与候选生成 | `crates/ime-core/src/viterbi/` | `P0.02.01` | `TC-CORE-004`、`TC-CORE-012` | ①②④ | A/T | ✅ |
| `REQ-CORE-05` | 语言模型评分与用户词频融合 | `crates/ime-core/src/lm/score.rs` | `P0.02.01` | `TC-CORE-005` | ①④ | A | ✅ |
| `REQ-CORE-06` | 直通与临时英文 | `crates/ime-core/src/passthrough.rs` | `P0.02.01` | `TC-CORE-006` | ①② | A | ✅ |
| `REQ-CORE-07` | Preedit 与切分高亮段 | `crates/ime-core/src/preedit.rs` | `P0.02.01` | `TC-CORE-007` | ①③④ | A | ✅ |
| `REQ-CORE-08` | 会话状态机/翻页/revision | `crates/ime-core/src/state/`（`REQ-RT-06` 同域） | `P0.02.01` | `TC-CORE-008` | ①②⑤ | A | ✅ |
| `REQ-CORE-09` | 自定义短语引擎 | `crates/ime-core/src/phrase.rs` | `P0.02.01` | `TC-CORE-009` | ①② | A | ✅ |
| `REQ-CORE-10` | 模糊音引擎层 | `crates/ime-core/src/fuzzy.rs` | `P0.02.01` | `TC-CORE-010` | ①②④ | A | ✅ |
| `REQ-CORE-11` | 简拼引擎层 | `crates/ime-core/src/viterbi/lattice/abbrev.rs` | `P0.02.01` | `TC-CORE-011` | ①② | A | ✅ |
| `REQ-DICT-01` | 容器格式读写与原子替换 | `crates/ime-dict/src/format/` | `P0.02.02` | `TC-DICT-001` | ①② | S | ✅ |
| `REQ-DICT-02` | FST 索引与 mmap 加载 | `crates/ime-dict/src/fst_index.rs`、`mmap.rs` | `P0.02.02` | `TC-DICT-002` | ①② | S | ✅ |
| `REQ-DICT-03` | 条目表零拷贝与逐访问校验 | `crates/ime-dict/src/entry.rs` | `P0.02.02` | `TC-DICT-003` | ①② | A | ✅ |
| `REQ-DICT-04` | 用户词频库与落盘纪律 | `crates/ime-dict/src/user_db.rs` | `P0.02.02` | `TC-DICT-004`、`TC-DICT-011` | ①②④ | A/T | ✅ |
| `REQ-DICT-05` | 损坏自愈与只读降级 | `crates/ime-dict/src/recover.rs` | `P0.02.02` | `TC-DICT-005` | ②③ | A | ✅ |
| `REQ-DICT-06` | dictc 白名单与编译管线 | `xtask/src/dictc/` | `P0.02.02` | `TC-DICT-006` | ①② | S | ✅ |
| `REQ-DICT-07` | L3b 多音字定向展开 | `xtask/src/dictc/build.rs` | `P0.02.02` | `TC-DICT-007` | ①② | A | ✅ |
| `REQ-DICT-08` | 简繁词表与运行时 | `crates/ime-dict/src/script.rs`、`crates/ime-core/src/script.rs` | `P0.02.02` | `TC-DICT-008` | ①② | A | ✅ |
| `REQ-DICT-09` | 用户词条管理原语 | `crates/ime-dict/src/user_db/` | `P0.02.02` | `TC-DICT-009` | ①②③ | A | ✅ |
| `REQ-DICT-10` | 自动备份与回滚 | `crates/ime-dict/src/user_db/backup.rs` | `P0.02.02` | `TC-DICT-010` | ②③ | A | ✅ |
| `REQ-CFG-01` | 配置 schema 与修复 | `crates/ime-config/src/schema.rs` | `P0.02.01` | `TC-CFG-001` | ①② | S | ✅ |
| `REQ-CFG-02` | v1→v2 单向迁移 | `crates/ime-config/src/migrate.rs` | `P0.02.01` | `TC-CFG-002` | ①② | A | ✅ |
| `REQ-CFG-03` | 热重载与会话保持 | `crates/ime-config/src/reload.rs` | `P0.02.01` | `TC-CFG-003` | ①② | A | ✅ |
| `REQ-CFG-04` | 键位自定义与速查表 | `crates/ime-config/src/keymap.rs`、`tests/keymap_matrix/` | `P0.02.01` | `TC-CFG-004` | ①④⑤ | A | ✅ |
| `REQ-CFG-05` | 双拼方案引擎 | `crates/ime-config/src/scheme.rs`、`crates/ime-core/src/shuangpin/` | `P0.02.01` | `TC-CFG-005` | ①② | A | ✅ |
| `REQ-CFG-06` | 配置写回与首启模板 | `crates/ime-config/src/writeback.rs` | `P0.02.01` | `TC-CFG-006` | ①②③ | A | ✅ |
| `REQ-DIAG-01` | 日志与字段脱敏 | `crates/ime-diag/src/{log,redact}.rs` | `P0.02.01` | `TC-DIAG-001` | ①② | S | ✅ |
| `REQ-DIAG-02` | panic 钩子与 FFI 兜底 | `crates/ime-diag/src/{panic,crash}.rs` | `P0.02.01` | `TC-DIAG-002` | ② | A | ✅ |
| `REQ-DIAG-03` | 探针与预算看板 | `crates/ime-diag/src/probe.rs`、`report/` | `P0.02.01` | `TC-DIAG-003` | ④ | A | ✅ |
| `REQ-DIAG-04` | UiFrame 镜像通道 | `crates/ime-diag/src/uiframe/` | `P0.02.01`、`P1.04.01` | `TC-DIAG-004` | ①② | S | ✅ |
| `REQ-DIAG-05` | 崩溃取证黑匣子 | `crates/ime-diag/src/crash/` | `P0.02.01` | `TC-DIAG-005` | ②③ | A | ✅ |
| `REQ-DIAG-06` | 纯净度与空闲探针 | `crates/ime-diag/src/probe.rs`、`testd/purity.rs` | `P0.02.01` | `TC-DIAG-006` | ④ | A | ✅ |

### 2.2 宿主与窗口（GUI 面为主）

| 需求编号 | 功能清单条目 | 关联真实源码路径 | 承接平台任务 | 承接用例 (TC) | 维度 | 分级 | 可执行性 |
|---|---|---|---|---|---|---|---|
| `REQ-RT-01` | Addon 生命周期与注册 | `crates/ime-fcitx5/src/addon.rs` | `P0.02.01` | `TC-RT-001` | ①② | S | ✅ |
| `REQ-RT-02` | 按键路由与不吞键 | `crates/ime-fcitx5/src/engine.rs`、`router` | `P0.02.01` | `TC-RT-002` | ①②⑤ | S | ✅ |
| `REQ-RT-03` | 光标坐标与多屏归一化 | `crates/ime-fcitx5/src/cursor/`、`screen.rs` | `P0.02.01` | `TC-RT-003` | ①② | A | ✅（`TC-RT-003` 混合 DPI 子项 🚫） |
| `REQ-RT-04` | C ABI 契约与 panic 兜底 | `crates/ime-fcitx5/src/ffi/abi/` | `P0.02.01` | `TC-RT-004` | ①② | S | ✅ |
| `REQ-RT-05` | UserInterface 接管与降级 | `crates/ime-fcitx5/src/ui_impl/` | `P0.02.01` | `TC-RT-005` | ①③④ | A | ✅（视觉判据：v2 红灯延续，见卡内复测记录） |
| `REQ-RT-06` | 真实会话端到端主链路 | `crates/ime-fcitx5/src/engine/router` + 沙盒 | `P0.02.01` | `TC-RT-006` | ①⑤ | S | ✅ |
| `REQ-RT-07` | 真实会话窗口行为与长期运行 | `xtask/src/testd/suite.rs` | `P0.02.01` | `TC-RT-007` | ②④ | A | ✅（长稳子项 T） |
| `REQ-RT-08` | Wayland 四档后端 | `crates/ime-ui/src/platform/wayland/` | `P0.02.01` | `TC-RT-008` | ①②③④ | A | 🚫 本机不可验证 |
| `REQ-RT-09` | 跨 addon 帧通道（ADR-0011） | `crates/ime-ui-addon/src/ffi/` | `P0.02.01` | `TC-RT-009` | ①② | S | ✅ |
| `REQ-RT-10` | 模式位/焦点/挂起/日志热切换 | `crates/ime-fcitx5/src/{session_host,engine/context}.rs` | `P0.02.01` | `TC-RT-010` | ①② | A | ✅ |
| `REQ-UI-01` | X11 窗口与焦点零抢占 | `crates/ime-ui/src/platform/x11.rs` | `P0.02.01` | `TC-UI-001` | ①②④ | S | ✅ |
| `REQ-UI-02` | Slint Platform 与软件光栅 | `crates/ime-ui/src/slint_platform.rs`、`renderer.rs` | `P0.02.01` | `TC-UI-002`、`TC-UI-004` | ①④ | S/A | ✅ |
| `REQ-UI-03` | UI 线程与通道语义 | `crates/ime-ui/src/{ui_thread,channel}.rs` | `P0.02.01` | `TC-UI-003` | ②④ | A | ✅ |
| `REQ-UI-04` | 布局与候选网格 | `crates/ime-ui/ui/candidate*.slint`、`layout.rs` | `P0.02.01`、`P1.03.01` | `TC-UI-006` | ①③④ | A | ✅ |
| `REQ-UI-05` | 主题/深浅色/亚克力 | `crates/ime-ui/src/theme.rs`、`x11/blur.rs` | `P0.02.01` | `TC-UI-007` | ④ | A | ✅（真实模糊子项 🚫） |
| `REQ-UI-06` | 鼠标交互 | `crates/ime-ui/src/interaction.rs` | `P0.02.01` | `TC-UI-008` | ①⑤ | A | ✅ |
| `REQ-UI-07` | 几何与屏幕避让 | `crates/ime-ui/src/geometry.rs` | `P0.02.01` | `TC-UI-009` | ①② | A | ✅ |
| `REQ-UI-08` | Spring 动效 | `crates/ime-ui/src/spring.rs` | `P0.02.01` | `TC-UI-010` | ④ | A | ✅ |
| `REQ-UI-09` | 过渡/状态位诚实显示 | `crates/ime-ui/src/spring/transition.rs` | `P0.02.01` | `TC-UI-011` | ③④ | A | ✅ |
| `REQ-UI-10` | 指针微动效与 preedit 尾部保护 | `crates/ime-ui/src/spring/press.rs`、`adapter/preedit/` | `P0.02.01` | `TC-UI-012` | ④⑤ | A | ✅ |
| `REQ-SEC-01` | 数据目录权限基线 | `crates/ime-dict/src/paths.rs` | `P0.02.01` | `TC-SEC-001` | ②③ | S | ✅ |
| `REQ-SEC-02` | 敏感上下文抑制学习 | `crates/ime-fcitx5/src/privacy_impl/`、`crates/ime-core/src/privacy.rs` | `P0.02.01` | `TC-SEC-002` | ①②③ | A | ✅ |
| `REQ-SEC-03` | 零网络断言 | `scripts/check-no-network.sh` | `P0.02.01` | `TC-SEC-003` | ①② | S | ✅ |
| `REQ-SEC-04` | 词源白名单与许可 | `scripts/check-dict-sources.sh`、`data/sources.toml` | `P0.02.01` | `TC-SEC-004`、`TC-SEC-008` | ①② | S | ✅ |
| `REQ-SEC-05` | Slint API 不泄漏（OB-4） | `scripts/check-slint-leak.sh` | `P0.02.01` | `TC-SEC-005` | ① | S | ✅ |
| `REQ-SEC-06` | 依赖单向性 | `scripts/check-deps.sh` | `P0.02.01` | `TC-SEC-006` | ① | S | ✅ |
| `REQ-SEC-07` | 交付链路隐私抽检 | `xtask/src/install/` | `P0.02.01` | `TC-SEC-007` | ② | A | ✅ |
| `REQ-INFRA-01` | 质量门禁四命令 | `justfile`、`.github/workflows/ci.yml` | `P0.02.01` | `TC-INFRA-001` | ① | S | ✅ |
| `REQ-INFRA-02` | 审计脚本族自测 | `scripts/check-*.sh`、`just audits` | `P0.02.01` | `TC-INFRA-002` | ①② | S | ✅ |
| `REQ-INFRA-03` | unsafe 边界 | `scripts/check-unsafe.sh` | `P0.02.01` | `TC-INFRA-003` | ① | S | ✅ |
| `REQ-INFRA-04` | 版本一致性 | `xtask/src/versions.rs` | `P0.02.01` | `TC-INFRA-004` | ① | S | ✅ |
| `REQ-INFRA-05` | 预算表一致性 | `xtask/src/budget.rs`、`docs/dev/budgets.json` | `P0.02.01` | `TC-INFRA-005` | ①④ | S | ✅ |
| `REQ-INFRA-06` | 安装布局与打包 | `packaging/`、`xtask/src/{install,package}` | `P0.02.01` | `TC-INFRA-006` | ①② | A | ✅ |
| `REQ-INFRA-07` | 场景引擎与沙盒 E2E | `xtask/src/testd/{engine,suite,sandbox}.rs` | `P0.02.01` | `TC-INFRA-007` | ①⑤ | A | ✅ |
| `REQ-INFRA-08` | 运行期零 socket 与纯净度 | `scripts/runtime-socket-check.sh`、`testd/purity.rs` | `P0.02.01` | `TC-INFRA-008` | ④ | A | ✅ |
| `REQ-INFRA-09` | 长稳与体积 | `xtask/src/soak.rs`、`testd/memory.rs` | `P0.02.01` | `TC-INFRA-009` | ④ | T | ✅（8h 子项 🚫） |

### 2.3 features-add 增量面（全部 `[待实现]`，锚定 features-add TASK）

| 需求编号 | 功能清单条目 | 关联真实源码路径（落地锚点） | 承接平台任务 | 承接用例 (TC) | 维度 | 分级 | 可执行性 |
|---|---|---|---|---|---|---|---|
| `REQ-ADD-01` | 词库权重分层归一化 | `xtask/src/dictc/{normalize,build}.rs`（规划锚点） | `P0.02.01` | `TC-ADD-001` | ①④ | S | ⏳ `TASK-1.01.01` |
| `REQ-ADD-02` | 用户词频时间衰减 | `crates/ime-dict/src/user_db/decay.rs`（规划锚点） | `P0.02.01` | `TC-ADD-002`、`TC-ADD-003` | ①④ | A | ⏳ `TASK-1.02.01` |
| `REQ-ADD-03` | BIGRAM 段与真查 | `crates/ime-dict/src/fst_index/dict_lm.rs`、`format/mod.rs` | `P0.02.01`、`P0.02.02` | `TC-ADD-004`、`TC-ADD-005` | ①②④ | S/A | ⏳ `TASK-1.03.01` |
| `REQ-ADD-04` | 置顶词全链路 | `crates/ime-core/src/state/transitions.rs:203` | `P0.02.01`、`P1.04.01` | `TC-ADD-006` | ①③⑤ | S | ⏳ `TASK-1.04.01` |
| `REQ-ADD-05` | 模糊音/简拼/简繁生产接线 | `crates/ime-core/src/state/machine.rs:454` | `P0.02.01` | `TC-ADD-007` | ①⑤ | S | ⏳ `TASK-2.01.01` |
| `REQ-ADD-06` | 词源扩充与多音字校验 | `data/sources.toml`、`xtask/src/dictc/quality.rs` | `P0.02.02` | `TC-ADD-008` | ①② | A | ⏳ `TASK-2.04.01` |
| `REQ-ADD-07` | 会话共享粒度 | `crates/ime-fcitx5/src/engine/context.rs` | `P0.02.01` | `TC-ADD-009` | ①⑤ | A | ⏳ `TASK-2.05.01` |
| `REQ-ADD-08` | 纠错与上文重排 | `crates/ime-core/src/segment/`（规划锚点） | `P0.02.01` | `TC-ADD-010`、`TC-ADD-011` | ①②④ | A | ⏳ `TASK-3.01.01` |
| `REQ-ADD-09` | 重码治理/热替换/造词 | `crates/ime-dict/src/{hot_swap,user_lexicon}.rs`（规划锚点） | `P0.02.02` | `TC-ADD-012`（前半） | ②④ | A | ⏳ `TASK-3.03.01` |
| `REQ-ADD-10` | 功能面与桥接工艺 | `crates/ime-core/src/symbols.rs`（规划锚点）等 | `P0.02.01`、`P1.04.01` | `TC-ADD-012`（后半） | ①③⑤ | A | ⏳ `TASK-3.05.01` |

---

## 3. 模块索引与对账

| 分片 | 用例卡数 | REQ 覆盖 |
|---|---|---|
| [tests/core.md](tests/core.md) | 12 | REQ-CORE-01~11（含原 REQ-RT-06 状态机域，并入 `REQ-CORE-08`） |
| [tests/dict.md](tests/dict.md) | 11 | REQ-DICT-01~10 |
| [tests/rt.md](tests/rt.md) | 10 | REQ-RT-01~10 |
| [tests/ui.md](tests/ui.md) | 12 | REQ-UI-01~10 |
| [tests/sec.md](tests/sec.md) | 8 | REQ-SEC-01~07 |
| [tests/diag.md](tests/diag.md) | 6 | REQ-DIAG-01~06 |
| [tests/infra.md](tests/infra.md) | 9 | REQ-INFRA-01~09 |
| [tests/cfg.md](tests/cfg.md) | 6 | REQ-CFG-01~06 |
| [tests/add.md](tests/add.md) | 12 | REQ-ADD-01~10 |
| **合计** | **86** | 69 存量 REQ（v2.0 的 67 条经 CORE 拆分与 SEC 增列重组为 69）+ 10 增量 REQ |

机械对账命令（自检与 CI 可复跑）：

```bash
grep -rnE '用例状态：`\[ \]' ./docs/dev/tests/*.md | wc -l   # == 86
grep -cE '^\| `REQ-' ./docs/dev/tests.md                      # == 79（69 + 10）
```
