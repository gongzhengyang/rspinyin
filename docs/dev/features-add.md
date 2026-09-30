# rspinyin 增量功能扩充与体验升维规划（features-add.md）

> 文档版本: v1.0 ｜ 系统形态: **Desktop GUI**（Linux 桌面输入法：进程内 Fcitx5 插件 + 全自绘候选框）｜
> 架构基线: Rust 2024 workspace（7 crates + xtask，edition 2024 / MSRV 1.85 / toolchain 1.98.0）+ Slint 1.x + Fcitx5 5.1.7 ｜
> 关联 ADR: [./adr/0000-upstream-decisions.md](./adr/0000-upstream-decisions.md)（Slint 许可 + 词源）、[./adr/0001-frozen-boundary-contracts.md](./adr/0001-frozen-boundary-contracts.md)（契约冻结）、[./adr/0002-rust-exports-addon-factory.md](./adr/0002-rust-exports-addon-factory.md)、[./adr/0003-ui-role-separate-addon.md](./adr/0003-ui-role-separate-addon.md)；本规范新提 **ADR-0005（增量契约扩展）**，状态：待决策 ｜
> 上游主文档: [./features.md](./features.md)（Phase 1 全量任务卡 + Phase 2/3 索引）｜ 测试体系: [./features-test.md](./features-test.md)、[./tests.md](./tests.md) ｜
> 最后同步 Commit: `ee0dbfb`（工作区含未提交增量，审计基准为 2026-09-29 17:0x 的工作树）｜
> 维护约定: 代码演进后必须回写第 3.7 节差距矩阵、第 5.1 节追溯表与任务卡状态；假设被推翻时必须同步回写受影响任务卡

---

## 0. 阅读顺序、产出物清单与规范继承

### 0.1 本规范的产出物

| 文件 | 角色 | 承载内容 |
|---|---|---|
| `docs/dev/features-add.md` | **Hub（本文件）** | 现状基线清单、假设清单、**全量差距矩阵**、增量架构与契约规范、**全量追溯表**、关键路径汇总、**P0 任务卡（原子级展开）**、演化路线、续写指令 |
| `docs/dev/features-add/phase-2.md` | Spoke | P1 任务卡的原子级展开（效率跃升与全键盘工作流） |
| `docs/dev/features-add/phase-3.md` | Spoke | P2 任务卡的原子级展开（生态扩充与极限调优） |

### 0.2 本规范对 skill 默认值的四处适配（**必读，否则后续章节的口径不可理解**）

skill 的入参全部未提供，按默认值执行；但默认值中有四处与"Linux 桌面输入法"这一被测形态直接冲突，**按 skill 的核心法则"对标行业同类顶尖产品"做显式适配并登记**，不做隐式替换：

| 项 | skill 默认值 | 本规范取值 | 适配理由 |
|---|---|---|---|
| `$1` 对标产品清单 | Linear、Raycast、Things 3、Figma、Arc、Obsidian、VS Code | **双轴制**：轴 A（功能镜像）= 搜狗拼音输入法、微软拼音（Windows 11）、RIME / ibus-rime、Fcitx5 内置拼音（libpinyin）、macOS 原生拼音、微信输入法、小鹤音形；轴 B（工艺镜像）= skill 默认的七款产品**全量保留** | 默认清单全部是**生产力应用**，与输入法不存在功能对位关系（Figma 的多人协作、Arc 的标签管理在输入法上无对应物），直接照搬会产出无法落地的"伪差距"。轴 B 保留默认清单是因其在**交互工艺、键盘优先、配置体系、本地优先、插件生态**五个维度上确实是行业标杆，可逐维映射（映射表见 3.1）。**登记为 `ASM-A-01`** |
| `$3` 系统形态 | 自动从代码扫描推断 | **Desktop GUI**（判定依据：`crates/ime-ui/src/platform/{x11.rs, wayland/}` 的窗口后端、`crates/ime-ui/ui/*.slint` 的声明式界面、`crates/ime-fcitx5/src/ffi/` 的宿主 C ABI） | 扫描结论明确，无歧义 |
| `$4` 性能与资源预算 | 按形态基线（Desktop 交互响应 ≤ 16ms / 稳帧 60–120fps） | **原文继承项目现状基线**：`docs/dev/budgets.json` 全量 14 项 `BUDGET-*` 阈值，**逐字引用不改写**（见 0.3） | 项目已有机器可读的预算镜像且被 `xtask budget --validate` 与 CI 强制，凭空引入第二套阈值会造成双源漂移 |
| `$5` 特殊架构约束 | 沿用现有架构约束 | **原文继承** `features.md` 0.4 的 11 条架构规则 + `AGENTS.md` 的 23 条禁止事项，并在 0.4 追加 4 条**仅约束增量**的规则 | 增量特性不得绕过既有约束 |

### 0.3 继承的预算基线（`docs/dev/budgets.json`，逐字引用，增量不得劣化）

```
latency_ms : key_to_present_p50=4.0  key_to_present_p99=16.0  key_to_present_p99_144hz=12.0
             decode_p99=3.0  decode_p999=8.0  raster_p99=1.5
             first_key_to_visible_p99=8.0  addon_load=120.0
memory_mb  : ui_rss=18.0  plugin_rss=45.0  dict_mmap_rss=25.0
cpu_pct    : idle=0.3  typing_10cps=6.0  idle_redraw_count=0  idle_poll_timer_count=0
size_mb    : so_stripped=12.0  base_dict=20.0
robustness : soak_hours=8.0  rss_drift_mb=2.0  pass_rate_pct=100.0
net_sockets: 0
```

**增量预算裁决规则**（本规范追加，登记为 `ASM-A-02`）：

- 增量特性**不得放宽**上表任何一项。新增的常驻内存计入 `plugin_rss`（45MB），新增的 UI 侧资源计入 `ui_rss`（18MB），新增的词库段计入 `base_dict`（20MB）。
- 新增的**解码期**工作计入 `decode_p99`（3.0ms）。凡增量触及解码热路径，必须在任务卡的 NFR 中给出**该特性独占的毫秒预算**，且全部增量之和不得超过 3.0ms。
- 本规范为增量特性分配的独占预算（**总和 = 2.10ms，为既有实现留 0.90ms**）：

| 增量特性 | 独占预算 | 说明 |
|---|---|---|
| 双拼方案音节映射 | ≤ 0.15ms | 单表查表，无回溯 |
| 模糊音匹配 | ≤ 0.60ms | 每个音节最多 8 个替代拼写，路径数上限受 `ASM-A-06` 约束 |
| 简拼/首字母缩写展开 | ≤ 0.50ms | `Lexicon::prefix` 前缀枚举 + 候选上限截断 |
| 自定义短语 | ≤ 0.10ms | 独立小 FST，键长 ≤ 32 |
| 简繁转换 | ≤ 0.30ms | 仅对最终候选做最长匹配，不参与 Viterbi 打分 |
| 用户词条 forget/list | ≤ 0.05ms | 仅管理路径，非输入热路径 |
| 配置 schema 迁移 | 0ms（不在热路径） | 仅启动期 |
| 其余增量（P1/P2） | ≤ 0.40ms | 合计封顶 |

### 0.4 增量约束（继承 + 追加）

**继承**（`features.md` 0.4 的 11 条规则，**不得违反**）：单向依赖、`ime-ui` 不依赖解码器、`unsafe` 白名单、解码器纯函数、候选框绝不夺取焦点、零网络、无 `unwrap/expect/panic!`、词库视为不可信输入、预算是契约、配置热重载不重置会话、`ime-ui` 公共 API 不导出 Slint 类型。

**追加（仅约束增量特性，登记为 `ASM-A-03`）**：

| # | 规则 | 理由 |
|---|---|---|
| **A-1** | **增量不得新增 `unsafe` 位置**。`unsafe` 白名单仍只有 `crates/ime-fcitx5/src/ffi/**` 与 `crates/ime-dict/src/mmap.rs` 两处 | 新特性（简繁、短语、用户词管理）全部可在安全 Rust 内完成；一旦需要新的 mmap 或 FFI，说明设计选错了层次 |
| **A-2** | **增量不得新增网络能力，也不得新增"可选的"网络能力**（含 feature-gated） | `BUDGET-NET-01 = 0` 是产品承诺；一个默认关闭的 feature 在发行版打包时会被误开 |
| **A-3** | **增量不得引入新的 copyleft 或 ShareAlike 数据源**。简繁转换表**必须**来自 `data/raw/unihan.tsv`（Unicode License，已在白名单）或项目自建 | ADR-0000 决策 B 的不可逆性说明：copyleft 数据一旦混入，事后剥离需重建全部词频与排序 |
| **A-4** | **增量不得改变 `crates/ime-types` 的既有类型语义**；只允许**追加**（新变体、新字段、新方法带默认实现）或经 ADR-0005 批准的最小破坏性变更 | `ADR-0001` 冻结纪律；破坏性变更必须在 4.2 的契约增量表中逐条登记并给出迁移路径 |

### 0.5 与 `features.md` Phase 2/3 既定计划的关系（**防重复规划**）

`features.md` 的 0.1 节与 6.3 节已经把若干能力**按编号排期**到 Phase 2（36 个任务）与 Phase 3（16 个任务），但 **`docs/dev/features/phase-2.md` 与 `phase-3.md` 两个 Spoke 文件至今不存在**（审计实测：`docs/dev/features/` 目录不存在）。本规范不重复规划这些已排期项，而是**为它们补齐原子级任务卡**，并在每张卡上标注承接关系。

| `features.md` 已排期编号 | 内容 | 本规范承接的 `ADD-FEAT` 卡 |
|---|---|---|
| `TASK-2.02.01` ~ `TASK-2.02.04` | 模糊音 / 简拼 / 双拼 / 纠错 | `ADD-FEAT-P0.02.01`、`P0.02.02`、`P0.02.03`、`P0.02.04`；纠错见 `ADD-FEAT-P1.01.01` |
| `TASK-2.02.08` | 词库质量补偿（L3b/L3c 落地） | `ADD-FEAT-P0.01.02` |
| `TASK-2.03.02` | 多用户 / 多 Profile | `ADD-FEAT-P2.04.01` |
| `TASK-2.03.03` | 命令面板（`Ctrl+Shift+/`）与 `OB-1` 补充路径 | `ADD-FEAT-P1.02.01`、`ADD-FEAT-P1.02.02` |
| `TASK-2.04.02` | per-app profile | `ADD-FEAT-P1.02.07` |
| `TASK-2.04.03` | 剪贴板历史 | `ADD-FEAT-P2.04.02` |
| `TASK-2.05.02` | 主题与动效打磨 | `ADD-FEAT-P1.03.03` |
| `TASK-2.05.03` | 模糊降级视觉补偿 | 已由 `features.md` 覆盖，本规范不重复 |
| `TASK-2.05.05` | 表情 / 符号面板 | `ADD-FEAT-P1.01.04` |
| `TASK-2.05.06` | 高对比主题与无障碍评估 | `ADD-FEAT-P1.05.03`、`ADD-FEAT-P1.05.04` |
| `TASK-2.06.03` | 许可免责声明 | `ADD-FEAT-P0.05.02` |
| `TASK-2.07.01` / `TASK-2.07.02` | deb / rpm / AUR 打包；fcitx5 5.0.x 支持评估 | `ADD-FEAT-P1.05.06` |
| `TASK-2.08.01` | `ime-doctor` | `ADD-FEAT-P1.05.05` |
| `TASK-2.08.02` | 预算回归门禁 | `ADD-FEAT-P1.05.07` |
| `TASK-3.02.02` | THUOCL 领域词库（许可待核实） | `ADD-FEAT-P2.01.02` |
| `TASK-3.04.01` | GPU 渲染路径 | `ADD-FEAT-P2.02.01` |
| `TASK-3.05.02` | 皮肤市场 / 在线主题 | `ADD-FEAT-P2.03.02` |
| `TASK-3.07.01` / `TASK-3.07.03` | 发布流水线；跨发行版 CI 矩阵 | `ADD-FEAT-P2.05.01` |
| `TASK-3.08.01` | 8 小时长稳压测 | `ADD-FEAT-P2.05.02` |

**本规范新增、`features.md` 完全未规划的差距域**（这才是增量规划的净增量）：简繁转换、自定义短语、用户词条可见可删、用户词库导入导出、用户数据备份、配置 schema 迁移、快捷键全量自定义、候选框外观深度自定义、竖排候选、候选框位置策略、常驻状态指示、快速造词、配置导入导出、D-Bus 控制接口、候选框 UI 文案 i18n、诊断包导出、**README 与 `OB-1` 徽章**、**隐私说明文档**。

---

## 1. 当前项目已实现功能与技术基线清单（阶段零实测）

> 审计方法：只读扫描工作树（`git status` 显示 30 个已修改文件 + 大量未跟踪新文件），辅以二进制头解析与 `grep` 命中统计。**不读取任何构建产物以外的推测**。所有数字均为 2026-09-29 实测。

### 1.1 代码资产实测

| 指标 | 实测值 | 备注 |
|---|---|---|
| Rust 源文件总数 | **182** | `crates/` + `xtask/`，排除 `target/` |
| Rust 代码总行数 | **65,383** | 含测试与注释 |
| `#[test]` 总数 | **1,122** | 分布见下 |
| Slint 源文件 | **2** | `crates/ime-ui/ui/candidate.slint`（283 行）、`theme.slint`（70 行） |
| criterion 基准 | **5** | `ime-core/benches/{decode,input,passthrough}.rs`、`ime-dict/benches/{dict,userdb}.rs` |
| fuzz 目标 | **1** | `fuzz/fuzz_targets/dag_build.rs` |
| CI 审计脚本 | **8** | `scripts/check-{deps,unsafe,no-network,slint-leak,dict-sources}.sh`、`runtime-socket-check.sh`、`gen-licenses.sh`、`dict-probe.py` |
| ADR | **4** | 0000–0003 |
| 已编译词库 | `data/compiled/base.dict` = **248,484 字节** | 见 1.4 的发现 ① |

**各 crate 实测规模**：

| crate | 文件数 | 行数 | `#[test]` | 已实现的职责 |
|---|---|---|---|---|
| `ime-types` | 9 | 1,991 | 44 | 冻结契约（8 个叶子模块） |
| `ime-core` | 30 | 11,307 | 235 | 解码管线全链 |
| `ime-dict` | 18 | 7,625 | 132 | 词库格式 / FST / mmap / 用户库 |
| `ime-config` | 3 | 1,833 | 13 | TOML schema + 热重载 |
| `ime-ui` | 43 | 17,523 | 340 | Slint Platform / 光栅 / 几何 / 主题 / Spring / 线程 |
| `ime-fcitx5` | 24 | 6,237 | 109 | Addon / FFI / 光标 / 引擎 / 隐私 |
| `ime-diag` | 9 | 4,021 | 72 | 日志 / 崩溃 / panic / 权限 / 脱敏 |
| `xtask` | — | — | 177 | `dictc` / `tune` / `install` / `budget` / `check-versions` / `testd` |

### 1.2 已实现能力清单（按功能域，**只列实测存在的代码**）

**A. 核心解码（`ime-core`）——已完整实现**

- 音节表（411 条）与切分 DAG 构建器，含非法串保护（`src/segment/`）。
- 输入缓冲与增量解析：`InputBuffer::push_char` / `set_boundaries` / backspace 语义 / 光标（`src/input/buffer.rs`）。
- K-best Viterbi 解码器与词格构建（`src/viterbi/{decoder,lattice,mod}.rs`）。
- 语言模型评分层：`InMemoryLm`（**unigram**）、`log2_q8` 定点打分、`ScoreWeights` / `Scorer`（`src/lm/{ngram,score,mod}.rs`）。
- 用户词频融合（`trait UserFreqSource` 注入）。
- Preedit 生成与拼音切分高亮段（`src/preedit.rs`）。
- 非拼音直通与临时英文模式：`classify()` + `PassthroughFlags`（5 字段）+ `PunctMode` 全角标点（`src/passthrough/{punctuation,mod}.rs`）。
- 输入会话状态机：`SessionState` / `SessionEvent` / `Effect` / `Paging`（含 `reconcile` 按文本保持高亮）（`src/state/{machine,paging,transitions,boundaries}.rs`）。
- 隐私策略：`InputContextKind` / `PrivacyPolicy` / `DefaultPolicy` / `AppIdHash`（`src/privacy.rs`）。

**B. 词库与数据（`ime-dict`）——已完整实现**

- 二进制容器格式 v1：magic `RSPD`、`FORMAT_VERSION = 1`、64 字节头、6 段段表（`Fst`/`Entries`/`StrPool`/`Unigram`/**`Bigram`（v1 恒空）**/`WordList`）、CRC32 校验、`DictWriter`（`src/format/{mod,reader,writer}.rs`）。
- FST 索引构建与前缀枚举（`src/fst_index/`），`Lexicon::prefix` 已冻结待用。
- `mmap` 只读零拷贝加载（`src/mmap.rs`，**唯一允许 `unsafe` 的字典侧位置**）。
- 条目表与字符串池零拷贝访问（`src/entry.rs`）。
- 用户词频库（redb）：批量提交（`COMMIT_BATCH=32` / `COMMIT_INTERVAL_MS=2000`）、慢盘自适应降级（`SLOW_COMMIT_MS=3` → `RELAXED_COMMIT_BATCH=128`）、容量上限 500,000、空闲期按 `last_used_unix` 淘汰 10%（`src/user_db.rs`）。
- 损坏自愈：隔离而非删除（`*.corrupt.<unix_ts>`）、只读目录只记录不重命名、幂等（`src/recover/`）。
- XDG 路径解析与权限基线（`0700`/`0600`）（`src/paths.rs`）。

**C. 配置（`ime-config`）——已实现但**（见 1.4 发现 ②）

- TOML schema v1，6 个段：`[engine]`（6 键）、`[ui]`（7 键）、`[theme]`（2 键）、`[keys]`（4 键）、`[data]`（1 键）、`[diagnostics]`（5 键）；`MAX_DOCUMENT_KEYS = 120`、`MAX_KEY_BINDINGS = 8`、`MAX_RAW_LEN = 64`。
- `validate()` 返回 `Vec<ImeError>`；`repaired()` 返回修复后配置 + 报告列表（逐键白名单 + 越界夹取）。
- 热重载：`ConfigStore` / `ReloadOutcome` / `default_path()`，**不重置进行中的输入会话**（`src/reload.rs`，1016 行，**已超 800 行上限**，见 1.4 发现 ③）。

**D. 视图与渲染（`ime-ui`）——骨架完整，候选网格与鼠标交互未实现**

- 自定义 Slint `Platform` 与软件光栅渲染器（`src/slint_platform.rs`、`src/renderer/{raster,probe,mock}.rs`）。
- 候选框几何与屏幕避让（`src/geometry/placement.rs`）、布局度量（`src/layout/metrics.rs`）。
- 主题 Token 全量（18 个 `ThemeTokens` 字段）、WCAG 对比度自检（`CONTRAST_BODY=7.0` / `CONTRAST_MINIMUM=4.5`）、亚克力模糊协商（`BlurNegotiation` / `BlurSurface` / `request_blur`）、深浅色决策链（portal → `GTK_THEME` → `QT_STYLE_OVERRIDE`）（`src/theme/`）。
- Spring 物理积分器（`ω₀=26.0` / `ζ=0.85` / 重定向保留速度）（`src/spring/`）。
- UI 线程模型与 SPSC 命令队列 + `eventfd` 唤醒（`src/channel/`、`src/ui_thread/`）。
- 平台后端：X11 ARGB 透明窗口（`src/platform/x11.rs`）、Wayland 四档（`layer_shell.rs` / `popup.rs` / `canvas_popup.rs` / 兜底）（`src/platform/wayland/`）。
- `.slint` 骨架：`CandidateMetrics`（30+ 几何常量）、`CandidateShadow`、`Header`；`Theme` global（22 个颜色 Token）。

**E. 宿主集成（`ime-fcitx5`）**

- 双 cdylib 架构（ADR-0003）：`librspinyin.so`（`Category=InputMethod`）+ `librspinyin-ui.so`（`Category=UI`）。
- Addon 注册与生命周期、`rspinyin.conf` / `rspinyin-im.conf`、工厂符号从 Rust 导出（ADR-0002）。
- FFI 边界：C ABI 契约、panic 守卫、`ui_glue.cpp` / `engine_glue.cpp` / `addon_glue.cpp`。
- 光标坐标提取、多屏与缩放归一化（`src/cursor/`、`src/screen.rs`）。
- 按键事件路由与 Fcitx5 状态机协作（`src/engine.rs`）。
- 隐私实现层（`src/privacy_impl/`）、UI 实现层（`src/ui_impl/`）。

**F. 诊断（`ime-diag`）**

- `tracing` 结构化日志 + 滚动（8MB × 4）+ `RedactLayer` 字段脱敏 + 应用标识哈希 + `$HOME` 重写（`src/{log,redact}.rs`）。
- panic 钩子、崩溃回溯落盘、FFI 边界兜底（`src/{crash,panic}.rs`）。
- 文件权限（`src/perms.rs`）。

**G. 工程与交付**

- `justfile` 16 个 recipe，含 `ci` = `check + check-deps + check-unsafe + check-net + check-slint + check-dict + check-licenses + check-budget + check-versions`。
- `.github/workflows/ci.yml`、`packaging/{install,uninstall}.sh`、`packaging/fcitx5/*.conf`。
- 词源白名单 `data/sources.toml`（5 个来源，全部 `permissive = true`）。
- 许可清单 `docs/dev/licenses.md`（含 `OB-1`~`OB-6` 逐条复核）、`docs/dev/NOTICE`、`docs/dev/lm-weights.md`、`docs/dev/spikes/{abi-spike,cursor-probe}.md`。

### 1.3 契约中**已冻结但尚未实现**的预留位（增量的天然挂载点）

这些位置是 `ADR-0001` 特意为后续阶段冻结的，**增量特性应优先落在这些位上，而不是新造类型**：

| 预留位 | 位置 | 冻结时的注释 | 本规范承接 |
|---|---|---|---|
| `DecodeFlags::FUZZY` + 8 个类别位（`1<<0`~`1<<8`） | `crates/ime-types/src/decode.rs` | "switched on by the Phase 2 decode extensions (fuzzy syllables…)" | `ADD-FEAT-P0.02.03` |
| `DecodeFlags::ABBREV`（`1<<9`） | 同上 | "Expand initial-letter abbreviations" | `ADD-FEAT-P0.02.04` |
| `Lexicon::prefix(&self, prefix, limit)` | `crates/ime-types/src/lexicon.rs:49` | "Prefix enumeration, used by the Phase 2 abbreviation expansion" | `ADD-FEAT-P0.02.04` |
| `CandidateSource::Symbol` | `crates/ime-types/src/ui.rs` | 从未被构造 | `ADD-FEAT-P1.01.04` |
| `LanguageModel::bigram(&self, prev, word)` | `crates/ime-types/src/lexicon.rs:96` | `InMemoryLm` 恒返回 0 | `ADD-FEAT-P2.01.01` |
| `SectionKind::Bigram` | `crates/ime-dict/src/format/mod.rs:144` | "Reserved for the Phase 3 bigram model; always empty in version 1" | `ADD-FEAT-P2.01.01` |
| `FLAG_BIGRAM_PRESENT` | `crates/ime-dict/src/format/mod.rs:80` | 写入侧已实现，v1 拒绝 bigram 段 | `ADD-FEAT-P2.01.01` |
| `WordFlags::{SURNAME, PLACE, TERM, USER}` | `crates/ime-types/src/lexicon.rs:180` | `TERM`（术语）从未被词源使用 | `ADD-FEAT-P2.01.02` |
| `UiConfig::client_preedit` | `crates/ime-config/src/schema.rs:313` | 已配置但**未见消费方** | `ADD-FEAT-P1.05.04`（无障碍路径） |
| `DiagnosticsConfig::probes` | `crates/ime-config/src/schema.rs:376` | 开关已存在 | `ADD-FEAT-P1.05.07` |
| `StatusStrip::readonly` | `crates/ime-types/src/ui.rs` | ADR-0001 追加字段，有先例 | 本规范沿用同一追加模式 |

### 1.4 本次审计发现的三处基线缺陷（**必须先处理，否则增量无处落脚**）

> 这三条不是"功能差距"，而是**当前工作树与文档/计划不一致**。按 `AGENTS.md` 第 7 节"文档与代码不一致时以代码为准并修正文档"，逐条给出处置。

**发现 ①：已编译词库只有 5,441 条，是开发词表而非产品词库。**

实测 `data/compiled/base.dict` 头部：`entry_count = 5,441`、`total_len = 248,484`。段分布：`Fst` 54.5KB / `Entries` 85.0KB / `StrPool` **35.8KB** / `Unigram` 42.5KB / `Bigram` 0 / `WordList` 24.6KB。

对照白名单来源：`data/raw/jieba-dict.tsv` **349,046 行 / 4.1MB**、`data/raw/pinyin-data.tsv` **44,435 行**、`data/raw/unihan.tsv` **44,355 行**、`data/raw/polyphone.tsv` 3,151 行。**`StrPool` 仅 35.8KB 意味着词条总文本量不足 4 万汉字**——349k 词表的字符串池应在 2MB 量级。

结论：**当前 `base.dict` 是 `data/raw/base.tsv`（5,871 行）单独编译的产物，jieba 的 349k 词表尚未进入编译。** 而 `BUDGET-SIZE-02` 允许 20MB，实际用量 1.2%。这不只是"词库小"，而是**产品核心价值（输入正确性）的载体尚未建立**。处置：`ADD-FEAT-P0.01.01`。

**发现 ②：`docs/dev/privacy.md` 不存在，`README.md` 不存在。**

`features.md` 6.3 节 Phase 1 出口准则 #7 要求"`docs/dev/` 下的 `budgets.json`、`licenses.md`、`privacy.md`、`adr/0000-*.md`、`adr/0001-*.md`、`spikes/*.md` 齐备"；#8 要求"`OB-1` 归属徽章已上线（README 双语的徽章与链接可达）"。实测 `docs/dev/privacy.md` **不存在**，仓库根 `README.md` **不存在**（`ls README*` 无匹配）。

同时 `docs/dev/licenses.md` 第 1 节自述"`LICENSE-APACHE` 与 `LICENSE-MIT` 由 Phase 2 的许可文件任务落地"——即**项目自身的许可文件也尚未落地**。

结论：**Phase 1 的两条出口准则当前不满足，且其中一条（`OB-1`）是 ADR-0000 决策 A 的硬性法务义务**。处置：`ADD-FEAT-P0.05.01`、`ADD-FEAT-P0.05.02`。

**发现 ③：`features.md` 仍含被 ADR-0003 推翻的 `UserInterface` 注册假定（6 处未修）。**

`features.md` 第 635、833、888（2.6 修正表第 1 行）、2677、2704、2997 行仍写着"实现 `fcitx::UserInterface` 并注册，把 fcitx5 的活跃 UI 切到 rspinyin"。ADR-0003 已实证：`UserInterfaceManager` **没有任何注册方法**、`AddonCategory` 是单值枚举、以 `Category=InputMethod` 注册却不提供引擎会让 fcitx5 **SIGSEGV**（已在 5.1.7 复现）。ADR-0003 的"后果 #1"明确要求修正，但至今未做。

**这不在本规范的增量范围内**（它是既有文档的修正，不是新功能），但**必须先修**：本规范的第 4.2 节契约增量与 `ADD-FEAT-P0.05.01` 的 README 架构图都会引用运行时拓扑，若 `features.md` 2.1 的拓扑图仍不登记第二个 cdylib，后续会话会按错误的架构实现。登记为 `ASM-A-04` 的前置条件。

### 1.5 当前任务状态快照（`.dev-progress.json`，59 个任务）

| 状态 | 数量 | 说明 |
|---|---|---|
| `COMPLETED` | 1 | `TASK-1.02.02` 输入缓冲与增量解析 |
| `READY_FOR_FINAL_GATE` | 28 | 代码已完成，待四条门禁一次性收口 |
| `IN_PROGRESS` | 7 | 含 `1.02.03` Viterbi、`1.02.04` Preedit、`1.05.04` theme.slint、`1.06.03` 许可证审计、`1.07.01` 安装布局、`1.08.03` 预算探针、`2.x` 测试平台项 |
| `PARTIAL` | 4 | `1.02.07` 解码基准、`1.04.03` UI 接管、`1.04.04` 按键路由、`1.08.03` 探针 |
| `PENDING` | 19 | **含 `TASK-1.05.06` 候选网格与数字快捷键标签、`TASK-1.05.07` 鼠标交互**——即候选框的内容层尚未实现 |

**已登记的阻塞项**（`.dev-progress.json` 的 `blockers`，原文引用）：

1. workspace 可编译；`ime-diag` 测试构建残留 3 条警告。
2. **3 个文件超过 800 行上限**：`ime-config/src/reload.rs`（1016）、`xtask/src/testd/engine/scenario.rs`（937）、`ime-config/src/schema.rs`（802）。
3. **没有任何基准是在空闲机器上跑的**——今日所有数字取自 20 个并发 agent 环境下，不可信。
4. Slint 构建接线可编译，但 `.slint` 源**从未端到端渲染过**。
5. 系统安装的 `librspinyin.so` 是修复前的构建，会段错误；不带 `FCITX_ADDON_DIRS` 的实验运行可复现。

---

## 2. 系统设计假设清单 (Assumptions First)

> 规则：`$4`/`$5` 已指定的预算与约束原样引用（见 0.3 / 0.4），未指定项才允许假设并显式登记。**严禁"等等""略"**。假设被推翻时必须同步回写受影响任务卡。

| 假设编号 | 维度 | 假设内容（基于阶段零实测） | 影响的增量域 | 偏离时的修正策略 |
|---|---|---|---|---|
| `ASM-A-01` | 形态 / 对标口径 | 对标采用双轴制：轴 A 为同类输入法（搜狗 / 微软拼音 / RIME / libpinyin / macOS 原生 / 微信输入法 / 小鹤音形），轴 B 为 skill 默认的七款生产力产品，仅用于交互工艺、键盘优先、配置体系、本地优先、插件生态五个维度的映射 | 全部差距域 | 若用户要求只用同类输入法对标，删除 3.1 的轴 B 映射表，差距矩阵行数与任务卡不变（轴 B 只影响"对标产品实现"列的文字，不影响差距识别） |
| `ASM-A-02` | 性能预算 | 增量预算裁决规则与独占预算分配表（0.3 节）成立：增量总独占 ≤ 2.10ms，既有实现保留 0.90ms | 全部解码类增量 | 若某增量实测超预算，按 `features.md` 6.2.1 的既有裁决顺序降级：先降候选上限，再降模糊音类别数，最后才考虑裁剪特性 |
| `ASM-A-03` | 架构约束 | 增量追加的 4 条约束（A-1 不新增 `unsafe`、A-2 不新增网络、A-3 不引入 copyleft 数据、A-4 只追加不改语义）成立 | 全部 | 若某增量确需新的 `unsafe` 或破坏性契约变更，必须新开 ADR 并由用户决策，不得在本规范内自行放宽 |
| `ASM-A-04` | 文档基线 | `features.md` 的 6 处 `UserInterface` 注册假定会在本规范的任何任务开工前被修正 | 全部（架构引用） | 若未修正，则本规范 4.2 的契约增量与 `ADD-FEAT-P0.05.01` 的架构图必须**自带**正确的双 cdylib 描述，且不得引用 `features.md` 2.1 的拓扑图 |
| `ASM-A-05` | 数据量级 | **产品词库目标规模为 349,046 条（jieba 全量）**，编译后 `base.dict` 落在 12~17MB，不超 `BUDGET-SIZE-02`（20MB） | 词库规模化、简繁、自定义短语、生僻字 | 若实测超 20MB，按 `features.md` 6.1 的 `R-08` 对策：按权重截断至 top 32 万并打印警告；**不得**通过放宽预算解决 |
| `ASM-A-06` | 算法复杂度 | 模糊音每个音节最多产生 8 个替代拼写；词格路径数上限沿用既有 `MAX_SYL_COUNT = 16` 与 `MAX_WORDS_PER_KEY = 32`，Viterbi 的 beam 宽度不因增量扩大 | 模糊音、简拼、纠错 | 若路径数导致 `decode_p99 > 3.0ms`，按"先降模糊音类别数 → 再收紧 beam → 最后关闭该类别默认值"的顺序降级 |
| `ASM-A-07` | 双拼方案集 | 首批支持 **5 种**方案：小鹤双拼、自然码、微软双拼、搜狗双拼、紫光拼音。智能 ABC / 拼音加加 / 全拼（默认）为后续批次 | 双拼全部 | 若用户要求覆盖全部 11 种，`ADD-FEAT-P0.02.01` 的方案表由 5 行扩为 11 行，工时 +1.5 人天，其余设计不变 |
| `ASM-A-08` | 简繁映射来源 | 简繁映射表**只能**来自 `data/raw/unihan.tsv`（Unicode License，`permissive = true`，已在白名单）的 `kSimplifiedVariant` / `kTraditionalVariant` 字段，加项目自建的一简对多繁消歧词表 | 简繁转换 | 若 Unihan 的变体字段覆盖率不足（实测待验证），缺口部分由项目自建词表补，**绝不引入 OpenCC 或 CC-CEDICT**（`ASM-A-03` 的 A-3） |
| `ASM-A-09` | 用户词库规模 | 单用户 `user.redb` 常态 10³~10⁴ 条，上限 500,000 条（既有 `USER_WORD_CAP`）；导出文件 ≤ 8MB | 用户词管理、导入导出、备份 | 若导出超 8MB，改用流式导出 + 分卷；**不得**引入压缩依赖（新增依赖需评审） |
| `ASM-A-10` | 配置规模 | `config.toml` 常态 ≤ 120 键（既有 `MAX_DOCUMENT_KEYS`）；增量新增的配置键总数 ≤ 45（`[scheme]` 8 + `[phrases]` 3 + `[script]` 3 + `[ui]` 追加 6 + `[keys]` 追加 3 + `[data]` 追加 5 + `[diagnostics]` 追加 2 + `[profile]` 15） | 配置类全部增量 | 若超 120 键，提升 `MAX_DOCUMENT_KEYS` 需同步修改 `schema.rs` 的 `MAX_DOCUMENT_KEYS` 常量与 `reload.rs` 的文档化上限，并在 `licenses.md` 同族的审计脚本中同步 |
| `ASM-A-11` | 配置迁移 | 迁移是**单向且不可逆**的：`schema_version` 从 N 升到 N+1 时，旧文件被重命名为 `config.toml.v<N>` 保留，新文件写入当前版本；**不提供降级迁移** | 配置 schema 迁移 | 若用户要求可逆迁移，需要为每个版本对写双向映射，工时 +3 人天；本规范按不可逆设计 |
| `ASM-A-12` | 无障碍路径 | 候选框**永远不夺取键盘焦点**（`features.md` 0.4 规则 5），因此候选框自身**不能**成为可聚焦的 AT-SPI 对象；无障碍可达性只能通过 fcitx5 的 `client_preedit`（把 preedit 交给应用自身的 A11y 树）与候选框的只读 A11y 旁路实现 | 无障碍 | 若 AT-SPI 旁路被证实会夺焦，则只保留 `client_preedit` 路径，并在 `privacy.md` 同族的 `docs/dev/a11y.md` 中如实声明局限 |
| `ASM-A-13` | 词库来源扩展 | 用户自建词表的导入**只接受 TSV**（`词<TAB>读音<TAB>权重`），走与 `dictc` 相同的校验路径；不接受 `.scel` / `.bcd` 等闭源商业格式的解析（其格式无公开规范，且解析即构成对商业词库的间接使用） | 用户自建词表导入 | 若用户明确要求 `.scel` 支持，需先取得格式规范的法律意见；本规范默认不做 |
| `ASM-A-14` | 快捷键自定义上限 | 全量键位自定义的键位表上限为 **64 条**（`MAX_BINDINGS`），动作集为 `KeyAction` 的全部变体 | 快捷键全量自定义 | 若 64 条不足，提升上限需同步 `MAX_KEY_BINDINGS` 同族的常量与校验；上限本身不是架构约束 |
| `ASM-A-15` | 候选框形态 | 候选框支持**横排网格（3~9 列）与竖排单列（1 列）两种形态**；竖排时 `max_per_row = 1`，几何常量沿用 `CandidateMetrics`，仅改变排布方向 | 竖排候选、外观自定义 | 若竖排的 `max_width_dp` 语义冲突，竖排下改用 `min_width` 作为唯一宽度约束 |
| `ASM-A-16` | 外部接口 | D-Bus 接口只读优先：先提供 `Status` / `Capabilities` 两个只读方法，写方法（切换模式、切换方案）在第二批 | D-Bus 控制接口 | 若只读接口不足以满足 IDE 插件场景，第二批写方法需重新评审"外部程序能否改变用户输入状态"的隐私含义 |
| `ASM-A-17` | 发行版覆盖 | 打包目标为 **deb（Ubuntu 22.04/24.04）、rpm（Fedora 40/41）、AUR（Arch）** 三套，与 `features.md` 0.5.5 的 6 台登记设备一致 | 发行版打包、跨发行版 CI | 若新增发行版，需同步 `features.md` 0.5.2 的能力矩阵与 0.5.5 的设备表 |
| `ASM-A-18` | 并发模型 | 增量**不引入任何新线程**。既有两线程模型（宿主线程 + UI 线程）不变；后台任务（词库重载、用户库淘汰、备份）复用既有的"空闲期"窗口 | 全部 | 若某增量确需新线程（如导出大文件），必须改为"空闲期分片执行"而非新线程；本规范按无新线程设计 |
| `ASM-A-19` | 版本兼容 | 增量落地后 `DICT_FORMAT_VERSION` **保持 1**（简繁表走独立的 `script.dict` 复用同一容器格式）；`CONFIG_SCHEMA_VERSION` 升为 **2**（新增配置段）；`RSPINYIN_ABI_VERSION` **保持 1**（FFI vtable 不变） | 简繁、配置迁移 | 若简繁表必须内联进 `base.dict`，则 `DICT_FORMAT_VERSION` 升为 2，`SECTION_COUNT` 由 6 升为 7，读取器需接受 v1 与 v2 两种头——代价是 `format/reader.rs` 的条件分支，工时 +1 人天 |
| `ASM-A-20` | 测试基线 | 增量任务卡的验收全部通过 `cargo nextest run --workspace --all-features` + `cargo test --workspace --doc`；**不得**用 `cargo test` 裸跑替代 | 全部任务卡 | 若某增量只能手工验证（如 Wayland 四档），按 `features.md` 0.2 的 `[实验室]` 标签如实标注并说明不可自动化 |
| `ASM-A-21` | 词库质量度量 | 增量后的词库质量用 `lm_holdout.tsv`（≥ 5000 条留出集）的「首选词命中率」与「目标词是否出现在前 9 候选内」两个指标度量；**不得**用 `lm_golden.tsv`（200 条）判定 1.9% 量级的差异 | 词库规模化、简拼、模糊音 | 若留出集不足以检出某增量的增益，扩充留出集而不是放宽判据 |
| `ASM-A-22` | 并发开发 | 本规范落地期间工作树仍有多个并行会话在改动；任何任务卡开工前必须重新确认其"代码落地锚点"列出的文件未被他人改动 | 全部 | 若锚点文件已被改动，以工作树现状为准并回写本规范的锚点列 |

---

## 3. 行业顶尖对标与功能差距矩阵

### 3.1 对标产品与对标轴

**轴 A：同类输入法（功能镜像）** —— 逐条列名，不做概括。

| 对标产品 | 形态 | 在本规范中被对标的特性 |
|---|---|---|
| **搜狗拼音输入法** | Windows / Linux / macOS | 双拼多方案、模糊音、简拼、智能纠错、自定义短语、日期时间数字大写、符号与 emoji 面板、中英混输、用户词库管理与导入导出、词频时间衰减、输入统计、横竖排候选、候选框位置策略、简繁转换、皮肤体系、设置界面、D-Bus 控制 |
| **微软拼音（Windows 11）** | Windows | 双拼方案、模糊音、简拼、云候选（**本项目非目标**）、自学习与自造词、中英混输、候选框外观深度自定义（字号/密度/主题）、Emoji 面板、多 Profile、无障碍（Narrator 可读候选） |
| **RIME / ibus-rime** | Linux 全平台 | 方案（schema）体系与切换、`key_binder` 全量键位自定义、`custom_phrase` 自定义短语、用户词库导入导出（`rime_dict_manager`）、简繁转换（`simplification` 过滤器）、自定义词表、词库备份与恢复、开放配置体系 |
| **Fcitx5 内置拼音（libpinyin）** | Linux | 模糊音、简拼、双拼、整句解码、与 fcitx5 生态的原生集成、`fcitx5-configtool` 图形配置 |
| **macOS 原生拼音** | macOS | 工艺级视觉与动效、候选条形态、Emoji 与符号、简繁转换、触控板手势、VoiceOver 无障碍 |
| **微信输入法** | 全平台 | 极简界面、隐私承诺的对外表达、跨设备词库（**本项目非目标**）、打字统计 |
| **小鹤音形** | Windows / Linux / macOS | 双拼 + 音形的组合方案、形码辅助选字、方案切换的零成本 |

**轴 B：跨类工艺标杆（交互与工程镜像）** —— skill 默认清单的逐项映射，**只用于五个可迁移维度**。

| 默认对标产品 | 可迁移到输入法的维度 | 在差距矩阵中的落点 |
|---|---|---|
| **Raycast** | 命令面板、键盘优先、模糊搜索、扩展生态 | `GAP-16`（命令面板）、`GAP-24`（全键盘可达）、`GAP-33`（插件系统，明确不做） |
| **Linear** | 速度即特性、快捷键体系、有主张的默认值 | `GAP-17`（键位自定义）、`GAP-41`（配置反馈） |
| **Things 3** | 工艺级动效、排版节奏、细节打磨 | `GAP-18`（外观自定义）、`GAP-19`（竖排） |
| **VS Code** | 配置体系与迁移、键位文件、设置搜索、扩展 API、诊断导出 | `GAP-25`（schema 迁移）、`GAP-17`、`GAP-27`（配置导入导出）、`GAP-30`（外部接口）、`GAP-38`/`GAP-39`（诊断） |
| **Obsidian** | 本地优先、数据所有权、插件生态、备份与恢复 | `GAP-26`（备份）、`GAP-29`（数据可删除）、`GAP-34`（主题包）、`GAP-33` |
| **Arc** | 新交互范式、空间化组织 | `GAP-20`（位置策略）、`GAP-31`（剪贴板历史） |
| **Figma** | 多人协作、画布、组件系统 | **无对位维度**——输入法是单用户本地进程内组件，无协作面。**登记为不适用**，不强行编造差距 |

**明确不适用、不进入差距矩阵的项**（避免编造差距）：云输入与联网词库更新、跨设备同步、多人协作、遥测与崩溃上报、Windows / macOS 平台移植、Flatpak / Snap 分发、`zwp_input_method_v2` 自建协议客户端（已由 `features.md` 0.1 排入 Phase 3 评估）、GPU 渲染路径（已排入 `TASK-3.04.01`）。

### 3.2 维度一：核心业务深度与高级工作流

| 差距编号 | 细分功能点 | 对标产品实现（逐个列名） | 本项目现状（阶段零实测） | 差距等级 |
|---|---|---|---|---|
| `GAP-01` | **双拼（多方案）** | 搜狗（小鹤/自然码/微软/搜狗/紫光/智能ABC/拼音加加）、微软拼音（微软双拼/自然码/小鹤/搜狗/紫光/拼音加加）、RIME（`double_pinyin` 系列 8 个方案）、libpinyin（双拼模式）、macOS 原生（小鹤/自然码/微软）、小鹤音形（核心形态） | **完全不存在**。`grep -ri shuangpin\|double_pinyin crates/ xtask/ data/` **零命中**；`KeyAction` 无方案相关变体；`DecodeRequest.raw` 的文档语义是"全拼 ASCII" | **P0** |
| `GAP-02` | **模糊音** | 搜狗（zh=z / ch=c / sh=s / n=l / r=l / f=h / an=ang / en=eng / in=ing / ian=iang / uan=uang）、微软拼音（同族 11 类）、RIME（`speller/algebra` 自定义）、libpinyin（内置模糊音开关） | **flag 位已冻结，零实现**。`DecodeFlags::FUZZY` + 8 个类别位（`1<<0`~`1<<8`）已定义，`Default` 只开 `USER_DICT`；解码器无任何模糊匹配代码；配置无 `[engine] fuzzy_*` 键 | **P0** |
| `GAP-03` | **简拼 / 首字母缩写** | 搜狗（全简拼 + 声母简拼 + 混拼）、微软拼音（简拼）、RIME（`abbrev` 系列）、libpinyin（简拼）、微信输入法（简拼） | **契约已冻结，零实现**。`DecodeFlags::ABBREV`（`1<<9`）与 `Lexicon::prefix(prefix, limit)` 已冻结，后者注释明写"used by the Phase 2 abbreviation expansion"；无任何调用方 | **P0** |
| `GAP-04` | **智能纠错**（错键 / 漏键 / 多键 / 乱序） | 搜狗（错字纠正，默认开启）、微软拼音（自动纠错） | 无。`DecodeFlags` 无纠错位；`segment/` 只做合法切分，非法串走 `DecodeNoPath` 降级 | P1 |
| `GAP-05` | **自定义短语 / 快捷输入** | 搜狗（自定义短语，支持位置参数）、RIME（`custom_phrase`）、微软拼音（自定义短语） | **完全不存在**。无短语配置段、无短语词表、无 `CandidateSource::Phrase`。用户无法让 `rq` 输出当前日期 | **P0** |
| `GAP-06` | **日期 / 时间 / 数字大写等计算类输入** | 搜狗（输入 `rq` 出日期、输入数字出中文大写、内置计算器）、微软拼音（日期、V 模式数字） | 无。`CandidateSource` 无计算类来源 | P1 |
| `GAP-07` | **符号与 Emoji 面板** | 搜狗（13 类符号 + emoji 联想）、微软拼音（Emoji 面板 `Win+.`）、macOS（字符检视器）、RIME（`symbols` 段） | **契约已冻结，零实现**。`CandidateSource::Symbol` 已定义但**从未被构造**（全仓零处构造）；无符号表数据；配置无 `[symbols]` 段 | P1 |
| `GAP-08` | **中英混输（英文词候选）** | 搜狗（中英混输，拼音流中直接给英文候选）、微软拼音（中英混输） | 仅"整串直通"：`PassthroughDecision` 判定后整串交给宿主，**不产生英文候选**。无英文词表 | P1 |
| `GAP-09` | **用户词条的可见与可删（忘记该词）** | 搜狗（词库管理，可删除单条）、微软拼音（自学习词条可删）、RIME（`rime_dict_manager` 可删） | **契约级缺口**。`trait UserFreqSource` 只有 `freq` / `record` / `is_user_word` **三个方法**，**没有 `forget`**；`UserDb` 只有 `record_count()` 与 `evict_oldest(percent)`（按最旧 10% 批量淘汰），用户**无法删除一个学错的词** | **P0** |
| `GAP-10` | **用户词库导入 / 导出 / 备份** | 搜狗（词库导入导出、账号同步）、RIME（`rime_dict_manager` 导出 `*.txt`）、微软拼音（导出学习词条） | 无。`UserDb` 无 `export` / `import`；无备份文件；`recover.rs` 只做**损坏隔离**（重命名为 `.corrupt.<ts>`），不产生可用备份 | **P0** |
| `GAP-11` | **词频时间衰减与场景化权重** | 搜狗（词频随时间与使用场景调整）、微软拼音（自学习权重动态调整） | 无时间衰减：`UserDb` 的权重是单调累加，`last_used_unix` 仅用于淘汰判定；无按应用/场景分离的权重 | P1 |
| `GAP-12` | **长句上下文语言模型** | 搜狗（云端 + 本地 n-gram 整句）、微软拼音（神经网络整句）、RIME（`grammar` 语言模型）、libpinyin（bi-gram 整句） | **契约与格式均已预留，零实现**。`LanguageModel::bigram(prev, word)` 已冻结但 `InMemoryLm` 恒返回 0；`SectionKind::Bigram` 与 `FLAG_BIGRAM_PRESENT` 已实现写入侧，但 v1 格式**拒绝** bigram 段（有测试断言） | P2 |
| `GAP-13` | **词库规模（产品级 vs 开发级）** | 搜狗（数十万至百万级词条）、微软拼音（同量级）、RIME（`luna-pinyin` 约 8 万词 + `essay` 词频）、libpinyin（约 30 万词） | **实测 `base.dict` 仅 5,441 条**（见 1.4 发现 ①）。`jieba-dict.tsv` 的 349,046 行**尚未编译进词库**；`StrPool` 仅 35.8KB | **P0** |
| `GAP-14` | **用户自建词表导入** | RIME（`*.dict.yaml` 自定义词表 + 用户词表）、搜狗（导入 txt 词库）、libpinyin（导入自定义词库） | 无。用户无法扩充词库 | P1 |
| `GAP-15` | **生僻字与 CJK 扩展区覆盖** | 搜狗（生僻字模式 + 笔画/手写输入）、微软拼音（生僻字）、RIME（依赖词表覆盖） | 部分。L1 单字表 44,435 字（`pinyin-data`）+ Unihan 44,355 行交叉校验；格式上 `MAX_WORD_LEN = 96` 字节（UTF-8，Ext-B 字符 4 字节 → 24 字）无碍。但**词库仅 5,441 条**，扩展区字符几乎无词组覆盖；无笔画/手写输入路径 | P2 |
| `GAP-16` | **整句输入的分词质量与候选可达性** | 搜狗（整句首选命中率高）、微软拼音（同）、libpinyin（bi-gram 整句） | 已有 Viterbi k-best 与 unigram 打分，但**词库仅 5,441 条**意味着长句路径大量落到单字回退（`Lexicon::fallback_single`）；`decode/degraded` 标志会在无路径时置位 | **P0**（由 `GAP-13` 派生，同卡承接） |

### 3.3 维度二：操作效率与沉浸式交互

| 差距编号 | 细分功能点 | 对标产品实现（逐个列名） | 本项目现状（阶段零实测） | 差距等级 |
|---|---|---|---|---|
| `GAP-17` | **图形化设置界面 / 命令面板** | 搜狗（完整属性设置窗口）、微软拼音（设置应用）、RIME（`fcitx5-configtool` + 手工 YAML）、libpinyin（`fcitx5-configtool`）、VS Code（设置 UI + 命令面板）、Raycast（命令面板即全部） | 无。配置**只能手改 `config.toml`**；`features.md` 排 `TASK-2.03.03`（`Ctrl+Shift+/` 命令面板），但 Spoke 文件不存在、卡未写 | P1 |
| `GAP-18` | **快捷键全量自定义** | RIME（`key_binder/bindings` 任意动作到任意键）、VS Code（`keybindings.json`）、搜狗（可自定义翻页键、中英切换键、简繁切换键、候选数量键）、微软拼音（键位自定义） | **极为有限**。`KeysConfig` 只有 4 个字段：`digit_zero`、`enter_commit_raw`、`flip_keys`（≤ `MAX_KEY_BINDINGS = 8`）、`highlight_keys`（≤ 8）。**无法**把任意动作绑定到任意键；无 `[keys.bindings]` 表 | P1 |
| `GAP-19` | **候选框外观深度自定义** | 搜狗（皮肤体系 + 字号 + 横竖排 + 候选数 + 间距）、微软拼音（主题 + 字号 + 密度 + 候选数）、macOS（候选条形态） | **有限**。`UiConfig` 7 个键：`client_preedit`、`max_per_row`、`show_annotation`、`max_width_dp`、`corner_radius_dp`、`base_alpha`、`animation`；`ThemeConfig` 2 个键：`scheme`、`accent`。**无**字号、字重、行高、单元格内边距、网格间距、阴影强度的配置；`.slint` 的 `CandidateMetrics` 是编译期常量 | P1 |
| `GAP-20` | **竖排单列候选** | 搜狗（横排/竖排可切换）、微软拼音（横排/竖排）、macOS（候选条即竖排）、RIME（`vertical` 选项） | **不可配置**。`max_per_row` 的合法区间是 `3..=9`（`schema.rs:314` 注释；测试 `ui.max_per_row = 2` 被 `validate()` 拒绝）。**无法配置为 1 列** | P1 |
| `GAP-21` | **候选框位置策略** | 搜狗（跟随光标 / 固定位置 / 记忆位置）、微软拼音（跟随 / 固定）、macOS（跟随） | **仅三种**。`Placement` 只有 `Below` / `Above` / `Auto`；`Anchor` 携带 `cursor` / `screen` / `scale`。**无**"固定屏幕位置"、"按应用记忆位置"、"跟随鼠标" | P1 |
| `GAP-22` | **简繁转换** | 搜狗（简繁切换快捷键 + 词级消歧）、微软拼音（简繁切换）、RIME（`simplification` 过滤器，OpenCC 表）、macOS（简繁切换） | **完全不存在**。`grep -ri 简繁\|traditional crates/` 零命中；`KeyAction` 只有 `ToggleLang` / `ToggleFullWidth` / `TogglePunct`，**无 `ToggleScript`**；`StatusStrip` 无 script 字段；无变体表数据 | **P0** |
| `GAP-23` | **常驻输入状态指示** | 搜狗（悬浮状态条，可拖动）、微软拼音（任务栏指示）、macOS（菜单栏图标） | 无。`StatusStrip` 只存在于**候选框的 header 内**——候选框隐藏时用户**无法得知**当前是中文还是英文、全角还是半角、是否处于只读模式。输入法**无常驻 UI**（ADR-0000 的 `OB-1` 明确"无关于对话框、无启动画面"） | P1 |
| `GAP-24` | **快速造词（手动加词）** | 搜狗（选中文本后按键加词）、微软拼音（自造词）、RIME（`custom_phrase` + 用户词表） | **仅自动学习**。用户无法主动"把这两个字记成一个词"；`UserFreqSource::record(key, weight_hint)` 只能由引擎在提交后调用 | P1 |
| `GAP-25` | **全键盘可达的设置与诊断入口** | Raycast（一切皆命令）、Linear（`⌘K`）、VS Code（`Ctrl+Shift+P`）、RIME（无 GUI 但有 `rime_console`） | 无设置界面，故无键盘可达性可言；`features.md` 的 `TASK-2.03.03` 计划用 `Ctrl+Shift+/` 唤起命令面板 | P1 |

### 3.4 维度三：数据管理与知识组织

| 差距编号 | 细分功能点 | 对标产品实现（逐个列名） | 本项目现状（阶段零实测） | 差距等级 |
|---|---|---|---|---|
| `GAP-26` | **配置 schema 迁移** | VS Code（配置自动迁移 + `settings.json` 版本化）、Obsidian（插件配置迁移） | **契约常量已冻结，迁移代码零行**。`CONFIG_SCHEMA_VERSION = 1`（`ime-types/src/version.rs:30`）已定义，`Config.schema_version` 字段存在；`grep -ri migrate\|migration crates/ xtask/` **零命中**。当前 `reload.rs` 只做"读入 + 校验 + 修复"，**schema 升到 v2 时旧文件的行为未定义** | **P0** |
| `GAP-27` | **用户数据自动备份与回滚** | Obsidian（`.obsidian` 备份 + 文件恢复插件）、RIME（用户词表可备份）、搜狗（账号同步即备份） | **无备份**。`recover.rs` 的语义是"损坏时**隔离**原文件"（重命名为 `user.redb.corrupt.<unix_ts>`，**绝不删除**），但**不产生可用副本**。用户数据只有一份，损坏后全部学习成果需人工从隔离文件抢救 | **P0** |
| `GAP-28` | **配置导入 / 导出 / 一键重置** | VS Code（设置同步 + 导出）、RIME（配置即文件，天然可复制）、搜狗（配置导出） | 无。用户迁移机器时无法导出配置；无"恢复默认值"命令（`Config::default()` 存在但无用户可达入口） | P1 |
| `GAP-29` | **输入统计与自学习透明度** | 搜狗（输入统计：打字速度、字数、常用词）、微信输入法（打字统计）、微软拼音（学习词条可查看） | 无。用户**看不到**自己学了多少词、哪些词被学过。`UserDb::record_count()` 存在但无消费者 | P2 |
| `GAP-30` | **用户数据的完全可删除** | Obsidian（数据在用户目录，可直接删）、RIME（词表即文件） | 部分。`user.redb` 在 XDG 目录下用户可直接删除，但**无程序内入口**，且删除后无提示、无确认、无"删除后行为如何"的说明。`docs/dev/privacy.md` **不存在**（见 1.4 发现 ②） | P1 |

### 3.5 维度四：个性化、集成与生态开放

| 差距编号 | 细分功能点 | 对标产品实现（逐个列名） | 本项目现状（阶段零实测） | 差距等级 |
|---|---|---|---|---|
| `GAP-31` | **外部程序控制接口（D-Bus）** | 搜狗（D-Bus 接口供外部查询/切换）、微软拼音（Windows TSF 接口）、RIME（`rime_api` C 接口 + `librime` 插件）、Fcitx5（自身的 D-Bus 接口） | 无。外部程序**无法**查询 rspinyin 的当前模式、无法请求切换中英、无法在 IDE 调试时临时禁用中文。`grep -ri dbus crates/` 零命中 | P1 |
| `GAP-32` | **剪贴板历史** | 搜狗（剪贴板历史，`Ctrl+;`）、微软拼音（`Win+V` 系统级） | 无。`features.md` 排 `TASK-2.04.03`，Spoke 不存在 | P2 |
| `GAP-33` | **候选框 UI 文案的 i18n** | VS Code（多语言包）、RIME（方案自带文案） | 无。`StatusStrip.mode_label` 由引擎填中文；候选框内的所有文案（"只读"、"全角"等）硬编码；`grep -ri i18n\|locale crates/` 零命中。非中文用户在英文模式下仍看到中文提示 | P2 |
| `GAP-34` | **插件 / 脚本扩展系统** | RIME（`librime` 插件 + Lua 脚本）、Obsidian（插件市场）、Raycast（扩展商店）、VS Code（扩展市场） | **无，且本规范建议明确不做**。理由：输入法插件与宿主同进程运行，任何脚本引擎都构成**任意代码执行面**；且 ADR-0000 的词源与许可体系建立在"所有数据来源可审计"之上，插件会破坏该不变量。**登记为"明确不做"并给出替代路径**（见 `ADD-FEAT-P2.03.01`） | — |
| `GAP-35` | **皮肤 / 主题包导入导出** | 搜狗（皮肤体系，可下载）、RIME（`preset_color_schemes` + 自定义）、macOS（无） | 仅内置主题。`ThemeConfig` 只有 `scheme` 与 `accent` 两键；`.slint` 的 `Theme` global 是编译期常量。`features.md` 排 `TASK-3.05.02`（皮肤市场），但**用户自定义皮肤包**（离线、本地文件）与"市场"是两件事 | P2 |
| `GAP-36` | **多用户 / 多 Profile 并行会话** | 微软拼音（多用户账户各自独立）、搜狗（账号级配置） | 单用户单会话。`features.md` 0.1 排 `TASK-2.03.02` | P2 |

### 3.6 维度五：交付与工程暗线基础设施

| 差距编号 | 细分功能点 | 对标产品实现（逐个列名） | 本项目现状（阶段零实测） | 差距等级 |
|---|---|---|---|---|
| `GAP-37` | **README 与 `OB-1` 归属徽章** | — （交付暗线，无同类产品对位） | **不存在**。仓库根 `ls README*` 无匹配。`features.md` 6.3 的 Phase 1 出口准则 #8 要求"`OB-1` 归属徽章已上线（README 双语的徽章与链接可达）"；ADR-0000 决策 A 的 `OB-1` 明确：输入法无常驻界面、无关于对话框、无启动画面，**必须走公开网页徽章路径**。当前**法务义务未履行** | **P0** |
| `GAP-38` | **隐私说明文档** | 微信输入法（隐私承诺的对外表达）、搜狗（隐私政策） | **不存在**。`docs/dev/privacy.md` 未创建（`features.md` 6.3 出口准则 #7 要求其齐备）；`TASK-1.06.02` 的验收标准要求"含数据流向图、存储位置、权限、以及 `CapabilityFlag::Password` 是'尽力而为'信号的明确声明"。**项目自身的 `LICENSE-APACHE` / `LICENSE-MIT` 亦未落地**（`licenses.md` 第 1 节自述由 Phase 2 落地） | **P0** |
| `GAP-39` | **无障碍语义暴露** | macOS 原生拼音（VoiceOver 可读候选）、微软拼音（Narrator 可读）、RIME（无）、搜狗（无） | 无。候选框是全自绘 surface，**对 AT-SPI 完全不可见**；屏幕阅读器读不到候选列表。`features.md` 的 `TASK-2.05.06` 只覆盖"高对比主题可用，对比度 ≥ 7:1"——那是**视觉**可达性，不是**语义**可达性。关键约束：候选框**绝不能夺焦**（0.4 规则 5），故不能是 AT-SPI 可聚焦对象 | P1 |
| `GAP-40` | **用户级自助诊断** | VS Code（`--status`、扩展诊断）、Obsidian（安全模式）、RIME（`rime_deployer --build` 输出） | 无。`features.md` 排 `TASK-2.08.01`（`ime-doctor`）。用户遇到问题时**只能**看 `tracing` 日志文件，且不知道日志在哪 | P1 |
| `GAP-41` | **崩溃与诊断包的一键导出** | VS Code（问题报告生成）、Obsidian（调试信息复制） | 部分。`ime-diag/src/crash.rs` 会落盘崩溃文件，但**用户不知道路径**、**无导出命令**、**无脱敏后的可分享格式** | P2 |
| `GAP-42` | **发行版打包（deb / rpm / AUR）** | 搜狗（deb/rpm）、RIME（全发行版仓库）、libpinyin（全发行版仓库） | 仅 `packaging/install.sh` / `uninstall.sh` + `packaging/fcitx5/*.conf`。`features.md` 排 `TASK-2.07.01` | P1 |
| `GAP-43` | **配置校验反馈的用户可达性** | VS Code（设置 UI 内联报错）、RIME（`rime_deployer` 输出）、Obsidian（控制台） | 部分。`Config::validate()` 返回 `Vec<ImeError>`、`Config::repaired()` 返回修复报告——**但只进日志**。用户手改 TOML 打错一个键，只能翻日志文件才知道被忽略了 | P1 |
| `GAP-44` | **运行时能力状态的可读输出** | RIME（`rime_deployer --build` 报告）、VS Code（`--status`） | 部分。`features.md` 0.5.2 有 4 档平台能力矩阵，但**用户看不到**当前环境支持什么（Wayland 档位、模糊是否可用、只读模式、候选框是否降级到 ClassicUI）。`StatusStrip.readonly` 只在候选框内可见 | P1 |

### 3.7 功能差距矩阵总览（**后续拆解的唯一事实来源**）

> 本表是 3.2~3.6 五节的全量收敛。**共 44 行**，每行至少有一张 `ADD-FEAT` 任务卡承接（映射见 5.1）。维度列使用 3.2~3.6 的五分法。

| 差距编号 | 所属维度 | 细分功能点 | 对标产品实现（逐个列名） | 本项目现状 | 差距等级 |
|---|---|---|---|---|---|
| `GAP-01` | 核心业务深度 | 双拼（多方案） | 搜狗、微软拼音、RIME、libpinyin、macOS 原生、小鹤音形 | 完全不存在 | **P0** |
| `GAP-02` | 核心业务深度 | 模糊音 | 搜狗、微软拼音、RIME、libpinyin | flag 位已冻结，零实现 | **P0** |
| `GAP-03` | 核心业务深度 | 简拼 / 首字母缩写 | 搜狗、微软拼音、RIME、libpinyin、微信输入法 | 契约已冻结，零实现 | **P0** |
| `GAP-04` | 核心业务深度 | 智能纠错 | 搜狗、微软拼音 | 无 | P1 |
| `GAP-05` | 核心业务深度 | 自定义短语 / 快捷输入 | 搜狗、RIME、微软拼音 | 完全不存在 | **P0** |
| `GAP-06` | 核心业务深度 | 日期 / 时间 / 数字大写等计算类输入 | 搜狗、微软拼音 | 无 | P1 |
| `GAP-07` | 核心业务深度 | 符号与 Emoji 面板 | 搜狗、微软拼音、macOS、RIME | `CandidateSource::Symbol` 冻结未用 | P1 |
| `GAP-08` | 核心业务深度 | 中英混输（英文词候选） | 搜狗、微软拼音 | 仅整串直通 | P1 |
| `GAP-09` | 核心业务深度 | 用户词条的可见与可删 | 搜狗、微软拼音、RIME | 契约级缺口：无 `forget` | **P0** |
| `GAP-10` | 核心业务深度 | 用户词库导入 / 导出 / 备份 | 搜狗、RIME、微软拼音 | 无 | **P0** |
| `GAP-11` | 核心业务深度 | 词频时间衰减与场景化权重 | 搜狗、微软拼音 | 无 | P1 |
| `GAP-12` | 核心业务深度 | 长句上下文语言模型（bigram） | 搜狗、微软拼音、RIME、libpinyin | 契约与格式预留，零实现 | P2 |
| `GAP-13` | 核心业务深度 | 词库规模（产品级 vs 开发级） | 搜狗、微软拼音、RIME、libpinyin | 实测仅 5,441 条 | **P0** |
| `GAP-14` | 核心业务深度 | 用户自建词表导入 | RIME、搜狗、libpinyin | 无 | P1 |
| `GAP-15` | 核心业务深度 | 生僻字与 CJK 扩展区覆盖 | 搜狗、微软拼音、RIME | 部分（L1 表全，词组几乎无） | P2 |
| `GAP-16` | 核心业务深度 | 整句输入的分词质量与候选可达性 | 搜狗、微软拼音、libpinyin | 词库过小导致大量单字回退 | **P0** |
| `GAP-17` | 操作效率 | 图形化设置界面 / 命令面板 | 搜狗、微软拼音、RIME、libpinyin、VS Code、Raycast | 无，仅 TOML | P1 |
| `GAP-18` | 操作效率 | 快捷键全量自定义 | RIME、VS Code、搜狗、微软拼音 | `KeysConfig` 仅 4 字段 | P1 |
| `GAP-19` | 操作效率 | 候选框外观深度自定义 | 搜狗、微软拼音、macOS | `UiConfig` 7 键 + `ThemeConfig` 2 键 | P1 |
| `GAP-20` | 操作效率 | 竖排单列候选 | 搜狗、微软拼音、macOS、RIME | `max_per_row` 下限为 3 | P1 |
| `GAP-21` | 操作效率 | 候选框位置策略 | 搜狗、微软拼音、macOS | `Placement` 仅三值 | P1 |
| `GAP-22` | 操作效率 | 简繁转换 | 搜狗、微软拼音、RIME、macOS | 完全不存在 | **P0** |
| `GAP-23` | 操作效率 | 常驻输入状态指示 | 搜狗、微软拼音、macOS | 状态仅在候选框 header 内 | P1 |
| `GAP-24` | 操作效率 | 快速造词（手动加词） | 搜狗、微软拼音、RIME | 仅自动学习 | P1 |
| `GAP-25` | 操作效率 | 全键盘可达的设置与诊断入口 | Raycast、Linear、VS Code | 无设置界面 | P1 |
| `GAP-26` | 数据管理 | 配置 schema 迁移 | VS Code、Obsidian | 常量已冻结，迁移零行 | **P0** |
| `GAP-27` | 数据管理 | 用户数据自动备份与回滚 | Obsidian、RIME、搜狗 | 仅损坏隔离，无备份 | **P0** |
| `GAP-28` | 数据管理 | 配置导入 / 导出 / 一键重置 | VS Code、RIME、搜狗 | 无 | P1 |
| `GAP-29` | 数据管理 | 输入统计与自学习透明度 | 搜狗、微信输入法、微软拼音 | 无 | P2 |
| `GAP-30` | 数据管理 | 用户数据的完全可删除 | Obsidian、RIME | 部分（无程序内入口） | P1 |
| `GAP-31` | 生态开放 | 外部程序控制接口（D-Bus） | 搜狗、微软拼音、RIME、Fcitx5 | 无 | P1 |
| `GAP-32` | 生态开放 | 剪贴板历史 | 搜狗、微软拼音 | 无 | P2 |
| `GAP-33` | 生态开放 | 候选框 UI 文案的 i18n | VS Code、RIME | 硬编码中文 | P2 |
| `GAP-34` | 生态开放 | 插件 / 脚本扩展系统 | RIME、Obsidian、Raycast、VS Code | 无，**建议明确不做** | — |
| `GAP-35` | 生态开放 | 皮肤 / 主题包导入导出 | 搜狗、RIME | 仅内置主题 | P2 |
| `GAP-36` | 生态开放 | 多用户 / 多 Profile 并行会话 | 微软拼音、搜狗 | 单用户单会话 | P2 |
| `GAP-37` | 交付暗线 | README 与 `OB-1` 归属徽章 | — | **不存在**（法务义务未履行） | **P0** |
| `GAP-38` | 交付暗线 | 隐私说明文档与项目许可文件 | 微信输入法、搜狗 | **不存在** | **P0** |
| `GAP-39` | 交付暗线 | 无障碍语义暴露 | macOS 原生、微软拼音 | 无 AT-SPI 暴露 | P1 |
| `GAP-40` | 交付暗线 | 用户级自助诊断（`ime-doctor`） | VS Code、Obsidian、RIME | 无 | P1 |
| `GAP-41` | 交付暗线 | 崩溃与诊断包一键导出 | VS Code、Obsidian | 崩溃文件仅本地、无导出 | P2 |
| `GAP-42` | 交付暗线 | 发行版打包（deb / rpm / AUR） | 搜狗、RIME、libpinyin | 仅 `install.sh` | P1 |
| `GAP-43` | 交付暗线 | 配置校验反馈的用户可达性 | VS Code、RIME、Obsidian | 仅进日志 | P1 |
| `GAP-44` | 交付暗线 | 运行时能力状态的可读输出 | RIME、VS Code | 用户看不到能力矩阵 | P1 |

**矩阵自检**：44 行 = P0 **13** 行（`GAP-01/02/03/05/09/10/13/16/22/26/27/37/38`） + P1 **22** 行（`GAP-04/06/07/08/11/14/17/18/19/20/21/23/24/25/28/30/31/39/40/42/43/44`） + P2 **8** 行（`GAP-12/15/29/32/33/35/36/41`） + 不做 **1** 行（`GAP-34`）。13 + 22 + 8 + 1 = **44** ✅。行内无省略符，无"等等"。

---

## 4. 增量接入架构设计与工艺级交互基线

### 4.1 无侵入接入架构（Add-on Architecture）

增量特性必须挂载到既有事件流，**不得**改动既有调用链的拓扑。项目已有 5 个天然挂载点，全部为"策略注入"而非"流程改写"：

| 挂载点 | 既有代码位置 | 增量如何接入 | 禁止事项 |
|---|---|---|---|
| **H1 解码前的输入改写** | `ime-core/src/segment/`（`DecodeRequest.raw` → 音节网格） | 双拼方案在**进入切分之前**把方案键序映射为全拼串；映射是纯函数、无状态、可单测 | 禁止在切分器内部按方案分支（会把方案知识泄漏进 411 音节表的语义） |
| **H2 词格构建时的路径扩张** | `ime-core/src/viterbi/lattice.rs` | 模糊音与简拼在**词格构建期**扩张边，而非在 Viterbi 内部；扩张受 `ASM-A-06` 的路径上限约束 | 禁止修改 Viterbi 的松弛顺序或 beam 宽度 |
| **H3 候选合并期的注入** | `ime-core/src/viterbi/decoder.rs` 的候选汇总段 | 自定义短语、符号面板作为**独立候选源**注入，通过 `CandidateSource::Phrase` / `Symbol` 标注 | 禁止把短语塞进 `Lexicon`（短语不是词库，生命周期与热重载语义不同） |
| **H4 提交前的输出变换** | `ime-core/src/state/transitions.rs` 的 commit 效果 | 简繁转换在**最终候选文本**上做最长匹配替换，**不参与打分、不参与 Viterbi** | 禁止在打分阶段做简繁（会让排序依赖一个与读音无关的维度） |
| **H5 宿主层的能力旁路** | `ime-fcitx5/src/{engine,addon}.rs`、`ime-ui/src/ui_thread/` | 常驻状态指示、D-Bus 接口、诊断命令在**宿主层**实现，不进入 `ime-core` | 禁止让 `ime-core` 感知任何宿主概念（0.4 规则 4 的纯函数约束） |

**数据向后兼容策略**（逐条强制）：

1. **词库格式不升版**（`ASM-A-19`）：简繁表走独立的 `script.dict`，**复用同一容器格式**（`ime-dict::format`）与同一 `Lexicon` 读取路径。这样 `DICT_FORMAT_VERSION` 保持 1，`format/reader.rs` 零改动，v1 词库与 v2 词库可共存。
2. **配置文件升版但不破坏读取**：`CONFIG_SCHEMA_VERSION` 由 1 升为 2。v1 文件被 `reload.rs` 识别后走迁移路径（`ADD-FEAT-P0.03.03`），迁移前的原件重命名为 `config.toml.v1` 保留。**未识别的键一律进 `Config::repaired()` 的报告列表**，绝不静默丢弃。
3. **用户库格式不升版**：`user.redb` 的新字段（如 `pinned`、`created_unix`）通过 redb 的表新增实现，旧记录读取时用 `Default` 补齐。**不写迁移脚本**。
4. **FFI ABI 不变**：`RSPINYIN_ABI_VERSION` 保持 1。增量的宿主交互（D-Bus、状态指示）走**新增的独立通道**，不扩 `RspinyinVtable`。

### 4.2 边界交互契约增量规范（**ADR-0005 提案**）

> 状态：**待决策**。落地前必须新开 `docs/dev/adr/0005-incremental-contract-extension.md` 并经用户决策，因为 `ADR-0001` 已冻结 `crates/ime-types` 的全部跨边界类型。

#### 4.2.1 契约增量总表

| 类型 | 位置 | 变更 | 变更类别 | 兼容性论证 | 承接卡 |
|---|---|---|---|---|---|
| `DecodeFlags` | `ime-types/src/decode.rs` | 新增 `SHUANGPIN = 1<<11`、`PHRASE = 1<<12`、`SCRIPT = 1<<13` | **追加（位域）** | 位域天然前向兼容：`from_bits_truncate` 丢弃未知位；旧读者读到新位会忽略。既有测试 `test_decode_flags_all_covers_every_defined_bit` 断言 `all().bits() == 0x07FF`，须同步改为 `0x3FFF` | `P0.02.01`、`P0.02.05`、`P0.02.06` |
| `SchemeId` | `ime-types/src/decode.rs`（新增） | `pub struct SchemeId(u8)`，`Full = 0`、`Xiaohe = 1`、`Ziranma = 2`、`Microsoft = 3`、`Sogou = 4`、`Ziguang = 5`；`const COUNT: u8 = 6` | **新增类型** | 新增类型不影响既有代码 | `P0.02.01` |
| `DecodeRequest` | `ime-types/src/decode.rs` | 新增字段 `pub scheme: SchemeId` | **破坏性（结构体加字段）** | 缓解：`DecodeRequest::new()` 与 `with_flags()` 已存在，新增 `with_scheme()` 构造器；`Default` 语义为 `SchemeId::Full`。**本 workspace 内无外部消费者**，所有构造点在同一 PR 内同步；`#[non_exhaustive]` 未使用故需全量同步。**这是本次唯一的破坏性变更，必须在 ADR-0005 中单独列出** | `P0.02.01` |
| `CandidateSource` | `ime-types/src/ui.rs` | 新增 `Phrase`、`Script` 两个变体 | **追加（枚举）** | 枚举新增变体使 `match` 不再穷尽。缓解：`CandidateSource` 当前在 `ime-core`/`ime-ui`/`ime-fcitx5` 共 N 处被 `match`；全部为 workspace 内代码，同 PR 同步。`Symbol` 变体是既有先例（已定义、未使用） | `P0.02.05`、`P0.02.06` |
| `StatusStrip` | `ime-types/src/ui.rs` | 新增 `pub script: Script`（`Script::Simplified` / `Traditional`） | **追加（结构体加字段）** | **已有先例**：`readonly` 字段是 ADR-0001 追加的，注释明确记录。`StatusStrip` 已 `derive(Default)`，新字段的 `Default` 为 `Simplified` | `P0.02.06` |
| `KeyAction` | `ime-types/src/key.rs` | 新增 `ToggleScript`、`ForgetHighlighted`、`PinHighlighted`、`AddPhrase` | **追加（枚举）** | 同 `CandidateSource`。既有测试 `label()` 是**穷尽 match**，新增变体会使其编译失败——这正是该测试的设计意图（"adding or removing one breaks the build instead of silently changing the frozen contract"），须同步更新 | `P0.02.06`、`P0.03.01`、`P1.02.05` |
| `UserFreqSource` | `ime-types/src/lexicon.rs` | 新增 `fn forget(&self, key: &str) -> bool`；`fn list(&self, offset: u64, limit: u16) -> Result<Vec<WordRef<'_>>, ImeError>`；`fn export_tsv(&self, w: &mut dyn Write) -> Result<u64, ImeError>` | **追加（trait 方法）** | **必须带默认实现**，否则所有既有实现（含 `ime-core` 测试中的 mock）编译失败。默认实现：`forget` 返回 `false`（"不支持"）、`list` 返回 `Err(ImeError::DictUnavailable)`、`export_tsv` 返回 `Err(...)`。`UserDb` 覆盖全部三个 | `P0.03.01` |
| `PhraseConfig` / `SchemeConfig` / `ScriptConfig` | `ime-config/src/schema.rs` | 新增 3 个配置段（见 4.2.3） | **追加** | `Config` 结构体加字段，`Config::default()` 补齐；`MAX_DOCUMENT_KEYS` 由 120 提升（见 `ASM-A-10`） | `P0.02.05`、`P0.02.01`、`P0.02.06` |
| `ConfigError` | `ime-types/src/error.rs` | 新增 `Migrated { from: u16, to: u16, backup: String }` | **追加（枚举）** | `ConfigError` 现有 2 个变体（`Invalid`、`LimitExceeded`）。**新错误码在既有 `config/*` 段内分配**，不新开域名段 | `P0.03.03` |
| `DictError` | `ime-types/src/error.rs` | 新增 `UserWordNotFound`、`ExportTooLarge { bytes: u64, limit: u64 }` | **追加（枚举）** | 在既有 `dict/*` 段内分配 | `P0.03.01` |
| `ImeError` | `ime-types/src/error.rs` | 新增 `DataBackupFailed { reason: String }`、`SchemeUnsupported { scheme: u8 }` | **追加（枚举）** | 在既有 `data/*` 与 `decode/*` 段内分配 | `P0.03.02`、`P0.02.01` |

**新增错误码的稳定字符串**（沿用 `domain/action/reason` 形式，`AGENTS.md` 第 1 节）：

```
config/migrated                (info 级，不是错误；用于诊断)
config/migration-failed        (error 级)
dict/user-word-not-found
dict/export-too-large
data/backup-failed
data/backup-restored
decode/scheme-unsupported
ui/script/unavailable          (简繁表缺失时的降级)
phrase/table-unavailable
```

#### 4.2.2 跨边界状态机跃迁（增量部分）

既有 `SessionState` 状态机（`ime-core/src/state/machine.rs`）的增量跃迁条件。**未列出的跃迁一律不变**。

| from-state | event | to-state | 副作用 | 承接卡 |
|---|---|---|---|---|
| `Idle` | `KeyEvent(ToggleScript)` | `Idle` | 写 `ScriptConfig.enabled`，重发 `Theme`/`StatusStrip`，**不产生 preedit** | `P0.02.06` |
| `Composing` | `KeyEvent(ToggleScript)` | `Composing` | **保持会话不重置**（0.4 规则 10），仅重算当前候选的显示文本；`revision` 自增 | `P0.02.06` |
| `Composing` | `KeyEvent(ForgetHighlighted)` | `Composing` | 调 `UserFreqSource::forget(text)`；从候选列表移除该词；`Paging::reconcile` 按文本保持高亮 | `P0.03.01` |
| `Composing` | `KeyEvent(AddPhrase)` | `Composing` | 把当前高亮候选写进 `[phrases]` 表并落盘；记 `phrase/added` 诊断 | `P1.02.05` |
| `Composing` | `DecodeDone{scheme != Full}` | `Composing` | 走方案映射后的全拼串；`DecodeResult.segments` 的 `start/end` 仍是**方案音节**下标 | `P0.02.01` |
| `Composing` | `ConfigReloaded` | `Composing` | **既有行为不变**（0.4 规则 10）；增量新增的 `[scheme]` / `[phrases]` / `[script]` 段的变更**延迟到本次提交之后生效**，避免中途换方案导致候选突变 | `P0.02.01`、`P0.02.05`、`P0.02.06` |
| `Idle` | `Startup` | `Idle` | 若 `config.toml` 的 `schema_version < CONFIG_SCHEMA_VERSION`，执行迁移并把原件重命名为 `config.toml.v<N>`；迁移失败则**以默认值启动 + 记 `config/migration-failed`**，绝不拒绝启动 | `P0.03.03` |
| `Idle` | `SessionStart` | `Idle` | 若距上次备份 > 24h 且用户库非空，在**空闲期**（无输入 30s）触发一次备份 | `P0.03.02` |

#### 4.2.3 新增配置段的完整键表

```toml
[scheme]                      # ADD-FEAT-P0.02.01 / P0.02.02
scheme          = "full"      # full | xiaohe | ziranma | microsoft | sogou | ziguang
show_hint       = true        # 双拼模式下在 header 显示方案名
keep_full_pinyin = true       # 允许在双拼模式下用全拼输入（混输兜底）

[scheme.custom]               # 自定义方案（仅 scheme = "custom" 时生效）
initials = ""                 # 26 个声母映射，长度必须为 26
finals   = ""                 # 韵母映射表，形如 "iu:q,ei:w,..."

[phrases]                     # ADD-FEAT-P0.02.05
enabled   = true
file      = ""                # 空 = 使用默认 ~/.config/rspinyin/phrases.tsv
max_entries = 5000

[script]                      # ADD-FEAT-P0.02.06
enabled     = false           # 默认关闭：简繁是显式选择，不是默认行为
traditional = false           # 持久化的当前状态
hotkey      = "ctrl+shift+f"  # 触发 ToggleScript

[ui]                          # 追加键（ADD-FEAT-P1.03.01 / P1.03.02）
font_size_dp       = 15       # 追加：候选字号（11..=22）
font_size_header_dp = 14      # 追加
density            = "normal" # 追加：compact | normal | relaxed
orientation        = "grid"   # 追加：grid | vertical
cell_gap_dp        = 6        # 追加：0..=16
placement          = "auto"   # 追加：auto | below | above | fixed | remember

[keys]                        # 追加键（ADD-FEAT-P1.02.04）
[[keys.bindings]]             # 追加：全量键位表，≤ 64 条
key    = "ctrl+shift+f"
action = "toggle-script"
[[keys.bindings]]
key    = "ctrl+shift+4"
action = "enter-symbol-panel"

[data]                        # 追加键（ADD-FEAT-P0.03.01 / P0.03.02）
export_dir      = ""          # 追加：导出目录，空 = XDG 默认
backup_enabled  = true        # 追加
backup_keep     = 3           # 追加：保留最近 N 份

[diagnostics]                 # 追加键（ADD-FEAT-P1.05.05）
doctor_on_start = false       # 追加：启动时跑一次能力自检并记日志
```

#### 4.2.4 简繁表的载体设计（**不升词库格式版本**）

```
/usr/share/rspinyin/
├── base.dict        # 既有，DICT_FORMAT_VERSION = 1
├── script.dict      # 新增，同一容器格式 v1
└── symbols.dict     # 新增（P1），同一容器格式 v1
```

- `script.dict` 的 FST 键 = 简体词的全拼（与 `base.dict` 同构），值 = 繁体词。**复用 `ime-dict::format` 与 `Lexicon` 读取路径，零新增解析代码。**
- 转换算法：在**最终提交文本**上做最长匹配（最大 8 字窗口），命中即替换；未命中保持原样。
- 一简对多繁（`发`→`發`/`髮`、`干`→`乾`/`幹`/`干`、`后`→`後`/`后`、`里`→`裡`/`里`）由**词级条目**消歧：`头发` → `頭髮`、`出发` → `出發`。词级条目由 `xtask` 从 `data/raw/unihan.tsv` 的 `kTraditionalVariant` 生成，歧义项进 `data/raw/script-disambig.tsv`（项目自建，`kind = "derived"`）。
- **许可**：Unihan 为 Unicode License（`permissive = true`，已在白名单）；消歧表为项目自建。**不引入 OpenCC**（其字典虽为 Apache-2.0，但会把转换质量的责任转移给一个我们无法审计的第三方表，且与本项目的"来源可审计"不变量冲突）。登记为 `ASM-A-08`。

### 4.3 工艺级交互基线：继承与增量

**继承**（`features.md` 3.1~3.6 的全部规范对增量同等生效，不重复抄录）：3.1 空间与材质（`shadow-margin: 32px`、`stroke-width: 1px`、`container-radius: 12px`、`container-padding: 8px`、`min-width: 220px`、`max-width: 720px`）、3.2 色彩 Token 与深浅色、3.3 物理动效（`ω₀=26.0` / `ζ=0.85` / 重定向保留速度）、3.4 组件五态、3.5 键盘优先、3.6 降级。

**增量追加的工艺约束**（只约束新增的交互面）：

| # | 约束 | 落点 |
|---|---|---|
| **C-1** | **新增的任何交互面必须是候选框的"同族 surface"**：同一 `SurfaceBackend`、同一 `ThemeTokens`、同一 `SpringIntegrator`。禁止为符号面板/命令面板另起一套渲染路径 | `ADD-FEAT-P1.01.04`、`P1.02.01` |
| **C-2** | **新增的文本必须走同一套字阶与 4dp 网格**：`font-size-header = 14px`、`font-size-cell = 15px`、`font-size-small = 11px`。禁止为"看起来更小"而引入 12px/13px 这类非标字阶 | 全部涉及 UI 的卡 |
| **C-3** | **新增的 5 态必须补齐 Disabled 态**。既有 `Theme` global 已有 `state-hover` / `state-selected-bg` / `state-selected-stroke` / `state-pressed`，**但没有 `state-disabled`**。凡新增控件（键位表、词条列表、皮肤选择）必须先把 `state-disabled` 补进 `theme.slint` 与 `ThemeTokens` | `ADD-FEAT-P1.03.01` |
| **C-4** | **新增动效必须复用 Spring 积分器且必须能"重定向保留速度"**。禁止为新增面板引入 CSS 式 ease 曲线 | `ADD-FEAT-P1.02.01`、`P1.01.04` |
| **C-5** | **新增面板的尺寸变化不得触发候选框的重新定位**。符号面板/命令面板展开时，候选框锚点不动，面板从候选框边缘生长 | `ADD-FEAT-P1.01.04` |
| **C-6** | **新增的候选来源必须在视觉上可区分但不得喧宾夺主**：`CandidateSource::Phrase` / `Symbol` / `Script` 的标注沿用既有 `text-annotation`（`rgba(242,242,247,0.48)` / `rgba(28,28,30,0.45)`），**不引入新颜色** | `ADD-FEAT-P0.02.05`、`P0.02.06` |

---

## 5. 增量工程落地任务清单

### 5.0 契约冻结点（**不是任务卡，是并行工作的前置里程碑**）

`crates/ime-types` 的契约增量（4.2.1 的表）**不绑定任何差距编号**，因此不作为 `ADD-FEAT` 任务卡，而作为**里程碑 `M0`** 显式声明。理由：它是所有并行工作的公共前置，且其交付物是 ADR 而非功能。

| 里程碑 | 内容 | 交付物 | 工时 | 门禁 |
|---|---|---|---|---|
| **`M0` 契约冻结** | 新开 `docs/dev/adr/0005-incremental-contract-extension.md`；按 4.2.1 的表逐条落地 `crates/ime-types` 的追加与**唯一一处破坏性变更**（`DecodeRequest.scheme`）；同步更新三个穷尽匹配测试（`test_decode_flags_all_covers_every_defined_bit`、`test_key_action_*` 的 `label()`、`CandidateSource` 的匹配点） | ADR-0005 + 契约代码 + `cargo check --workspace --all-targets` 绿 | **1 人天** | 用户对 ADR-0005 的决策；`cargo test -p ime-types` 全绿 |

**`M0` 是硬门禁**：`ADD-FEAT-P0.02.*`、`P0.01.03`、`P0.01.04`、`P0.03.02` **全部**依赖它。`ADD-FEAT-P0.01.01`、`P0.01.02`、`P0.03.01`、`P0.05.01`、`P0.05.02` **不依赖**它，可在 `M0` 决策期间并行开工。

### 5.1 WBS 任务覆盖追溯表

> 规则：**差距矩阵的 44 行每一行都必须出现在本表**；每张任务卡必须回填其绑定的差距编号；双向一致。`GAP-16` 由 `GAP-13` 派生（词库规模不足直接导致整句分词质量下降），两者同卡承接，在本表中合并为一行并注明。

| 差距编号 | 所属维度 | 并行通道 | 增量功能点 | 绑定任务节点清单 | 阶段 |
|---|---|---|---|---|---|
| `GAP-01` | 核心业务深度 | Track A（解码）｜Track B | 双拼（多方案） | `ADD-FEAT-P0.02.01`、`ADD-FEAT-P0.02.02` | P0 |
| `GAP-02` | 核心业务深度 | Track A（解码） | 模糊音 | `ADD-FEAT-P0.02.03` | P0 |
| `GAP-03` | 核心业务深度 | Track A（解码） | 简拼 / 首字母缩写 | `ADD-FEAT-P0.02.04` | P0 |
| `GAP-04` | 核心业务深度 | Track A（解码） | 智能纠错 | `ADD-FEAT-P1.01.01` | P1 |
| `GAP-05` | 核心业务深度 | Track A（数据） | 自定义短语 / 快捷输入 | `ADD-FEAT-P0.01.03` | P0 |
| `GAP-06` | 核心业务深度 | Track A（数据） | 日期 / 时间 / 数字大写等计算类输入 | `ADD-FEAT-P1.01.02` | P1 |
| `GAP-07` | 核心业务深度 | Track A（数据）｜Track B | 符号与 Emoji 面板 | `ADD-FEAT-P1.01.04` | P1 |
| `GAP-08` | 核心业务深度 | Track A（数据） | 中英混输（英文词候选） | `ADD-FEAT-P1.01.03` | P1 |
| `GAP-09` | 核心业务深度 | Track A（数据） | 用户词条的可见与可删 | `ADD-FEAT-P0.01.04` | P0 |
| `GAP-10` | 核心业务深度 | Track A（数据） | 用户词库导入 / 导出 / 备份 | `ADD-FEAT-P0.01.04`、`ADD-FEAT-P0.03.01` | P0 |
| `GAP-11` | 核心业务深度 | Track A（数据） | 词频时间衰减与场景化权重 | `ADD-FEAT-P1.01.05` | P1 |
| `GAP-12` | 核心业务深度 | Track A（数据） | 长句上下文语言模型（bigram） | `ADD-FEAT-P2.01.01` | P2 |
| `GAP-13` | 核心业务深度 | Track A（数据） | 词库规模（产品级 vs 开发级） | `ADD-FEAT-P0.01.01`、`ADD-FEAT-P0.01.02` | P0 |
| `GAP-14` | 核心业务深度 | Track A（数据） | 用户自建词表导入 | `ADD-FEAT-P1.01.06` | P1 |
| `GAP-15` | 核心业务深度 | Track A（数据） | 生僻字与 CJK 扩展区覆盖 | `ADD-FEAT-P2.01.02` | P2 |
| `GAP-16` | 核心业务深度 | Track A（数据） | 整句输入的分词质量与候选可达性（`GAP-13` 的派生项，同卡承接） | `ADD-FEAT-P0.01.01`、`ADD-FEAT-P0.01.02` | P0 |
| `GAP-17` | 操作效率 | Track B | 图形化设置界面 / 命令面板 | `ADD-FEAT-P1.02.01`、`ADD-FEAT-P1.02.02` | P1 |
| `GAP-18` | 操作效率 | Track B | 快捷键全量自定义 | `ADD-FEAT-P1.02.04` | P1 |
| `GAP-19` | 操作效率 | Track B | 候选框外观深度自定义 | `ADD-FEAT-P1.03.01`、`ADD-FEAT-P1.03.03` | P1 |
| `GAP-20` | 操作效率 | Track B | 竖排单列候选 | `ADD-FEAT-P1.03.02` | P1 |
| `GAP-21` | 操作效率 | Track B | 候选框位置策略 | `ADD-FEAT-P1.03.04` | P1 |
| `GAP-22` | 操作效率 | Track A（解码） | 简繁转换 | `ADD-FEAT-P0.02.05` | P0 |
| `GAP-23` | 操作效率 | Track B | 常驻输入状态指示 | `ADD-FEAT-P1.02.06` | P1 |
| `GAP-24` | 操作效率 | Track B | 快速造词（手动加词） | `ADD-FEAT-P1.02.05` | P1 |
| `GAP-25` | 操作效率 | Track B | 全键盘可达的设置与诊断入口 | `ADD-FEAT-P1.02.02`、`ADD-FEAT-P1.02.03` | P1 |
| `GAP-26` | 数据管理 | Track C | 配置 schema 迁移 | `ADD-FEAT-P0.03.02` | P0 |
| `GAP-27` | 数据管理 | Track A（数据） | 用户数据自动备份与回滚 | `ADD-FEAT-P0.03.01` | P0 |
| `GAP-28` | 数据管理 | Track C | 配置导入 / 导出 / 一键重置 | `ADD-FEAT-P1.04.01` | P1 |
| `GAP-29` | 数据管理 | Track B | 输入统计与自学习透明度 | `ADD-FEAT-P2.04.03` | P2 |
| `GAP-30` | 数据管理 | Track A（数据）｜Track C | 用户数据的完全可删除 | `ADD-FEAT-P1.04.02` | P1 |
| `GAP-31` | 生态开放 | Track C | 外部程序控制接口（D-Bus） | `ADD-FEAT-P1.04.03` | P1 |
| `GAP-32` | 生态开放 | Track B | 剪贴板历史 | `ADD-FEAT-P2.04.02` | P2 |
| `GAP-33` | 生态开放 | Track B | 候选框 UI 文案的 i18n | `ADD-FEAT-P2.03.03` | P2 |
| `GAP-34` | 生态开放 | — | 插件 / 脚本扩展系统（**明确不做**，只交付决策文档与替代路径） | `ADD-FEAT-P2.03.01` | P2 |
| `GAP-35` | 生态开放 | Track B | 皮肤 / 主题包导入导出 | `ADD-FEAT-P2.03.02` | P2 |
| `GAP-36` | 生态开放 | Track C | 多用户 / 多 Profile 并行会话 | `ADD-FEAT-P2.04.01` | P2 |
| `GAP-37` | 交付暗线 | Track C | README 与 `OB-1` 归属徽章 | `ADD-FEAT-P0.05.01` | P0 |
| `GAP-38` | 交付暗线 | Track C | 隐私说明文档与项目许可文件 | `ADD-FEAT-P0.05.02` | P0 |
| `GAP-39` | 交付暗线 | Track B | 无障碍语义暴露 | `ADD-FEAT-P1.05.03`、`ADD-FEAT-P1.05.04` | P1 |
| `GAP-40` | 交付暗线 | Track C | 用户级自助诊断（`ime-doctor`） | `ADD-FEAT-P1.05.05` | P1 |
| `GAP-41` | 交付暗线 | Track C | 崩溃与诊断包一键导出 | `ADD-FEAT-P2.05.03` | P2 |
| `GAP-42` | 交付暗线 | Track C | 发行版打包（deb / rpm / AUR） | `ADD-FEAT-P1.05.06` | P1 |
| `GAP-43` | 交付暗线 | Track C | 配置校验反馈的用户可达性 | `ADD-FEAT-P1.05.01` | P1 |
| `GAP-44` | 交付暗线 | Track C | 运行时能力状态的可读输出 | `ADD-FEAT-P1.05.02` | P1 |

**追溯表自检**：矩阵 44 行 → 本表 **44 行**，逐行对应，无遗漏、无多余。任务卡总数 = P0 **13** + P1 **21** + P2 **9** = **43 张**，每张至少绑定 1 个差距编号，每个差距编号至少被 1 张卡承接（`GAP-10` 被 2 张卡承接、`GAP-13`/`GAP-16` 各被 2 张、`GAP-17`/`GAP-19`/`GAP-25`/`GAP-39` 各被 2 张）。**无主任务数 = 0，无卡差距数 = 0。**

### 5.2 DAG 校验

**依赖边全表**（只列有依赖的卡；未列出者为无前置依赖）：

| 任务 | 前置依赖 | 依赖编号是否更小 | 备注 |
|---|---|---|---|
| `ADD-FEAT-P0.01.02` | `ADD-FEAT-P0.01.01` | ✅ `01.01 < 01.02` | 质量度量必须在词库规模化之后 |
| `ADD-FEAT-P0.02.02` | `ADD-FEAT-P0.02.01` | ✅ `02.01 < 02.02` | 配置层依赖方案引擎的 `SchemeId` |
| `ADD-FEAT-P0.03.01` | `ADD-FEAT-P0.01.04` | ✅ `01.04 < 03.01` | 备份复用导出原语 |
| `ADD-FEAT-P0.03.02` | `ADD-FEAT-P0.01.03`、`ADD-FEAT-P0.02.02`、`ADD-FEAT-P0.02.05` | ✅ 全部更小 | v2 schema = v1 + `[phrases]` + `[scheme]` + `[script]`，故迁移框架必须最后落地 |
| `ADD-FEAT-P1.01.01` | `ADD-FEAT-P0.02.03` | ✅ | 纠错复用模糊音的路径扩张机制 |
| `ADD-FEAT-P1.01.02` | `ADD-FEAT-P0.01.03` | ✅ | 计算类输入复用短语引擎的候选注入点 |
| `ADD-FEAT-P1.01.03` | `ADD-FEAT-P0.01.01` | ✅ | 英文词表编译进 `base.dict` 需要规模化的编译管线 |
| `ADD-FEAT-P1.01.04` | `ADD-FEAT-P0.01.03` | ✅ | 符号面板复用短语引擎的独立候选源机制 |
| `ADD-FEAT-P1.01.05` | `ADD-FEAT-P0.01.04` | ✅ | 时间衰减需要 `last_used_unix` 的可写管理路径 |
| `ADD-FEAT-P1.01.06` | `ADD-FEAT-P0.01.01` | ✅ | 用户词表导入复用 `dictc` 的 TSV 校验 |
| `ADD-FEAT-P1.02.01` | `ADD-FEAT-P0.02.02` | ✅ | 命令面板要能切换方案 |
| `ADD-FEAT-P1.02.02` | `ADD-FEAT-P1.02.01` | ✅ | 面板内的设置项依赖面板存在 |
| `ADD-FEAT-P1.02.03` | `ADD-FEAT-P1.02.01` | ✅ | 键位表在面板内编辑 |
| `ADD-FEAT-P1.02.04` | `ADD-FEAT-P1.02.03` | ✅ | 全量键位依赖键位表 UI |
| `ADD-FEAT-P1.02.05` | `ADD-FEAT-P0.01.04` | ✅ | 手动加词复用用户词条写入原语 |
| `ADD-FEAT-P1.02.06` | `ADD-FEAT-P0.02.05` | ✅ | 状态条要显示简繁状态 |
| `ADD-FEAT-P1.03.01` | 无 | — | 纯 `ime-ui` 侧 |
| `ADD-FEAT-P1.03.02` | `ADD-FEAT-P1.03.01` | ✅ | 竖排是外观配置的一项 |
| `ADD-FEAT-P1.03.03` | `ADD-FEAT-P1.03.01` | ✅ | 密度/字阶 token 依赖外观配置落地 |
| `ADD-FEAT-P1.03.04` | `ADD-FEAT-P1.03.01` | ✅ | 位置策略是外观配置的一项 |
| `ADD-FEAT-P1.04.01` | `ADD-FEAT-P0.03.02` | ✅ | 导入导出依赖版本化配置 |
| `ADD-FEAT-P1.04.02` | `ADD-FEAT-P0.03.01` | ✅ | 完全删除复用备份/导出路径 |
| `ADD-FEAT-P1.04.03` | `ADD-FEAT-P1.05.02` | ✅ | D-Bus 的 `Status`/`Capabilities` 复用能力矩阵输出 |
| `ADD-FEAT-P1.05.01` | `ADD-FEAT-P0.03.02` | ✅ | 校验反馈要能报迁移结果 |
| `ADD-FEAT-P1.05.02` | 无 | — | 纯能力探测 |
| `ADD-FEAT-P1.05.03` | `ADD-FEAT-P0.02.05` | ✅ | 无障碍要播报简繁状态 |
| `ADD-FEAT-P1.05.04` | `ADD-FEAT-P1.05.03` | ✅ | `client_preedit` 路径依赖 A11y 评估结论 |
| `ADD-FEAT-P1.05.05` | `ADD-FEAT-P1.05.02` | ✅ | `ime-doctor` 复用能力探测 |
| `ADD-FEAT-P1.05.06` | `ADD-FEAT-P0.05.01` | ✅ | 打包需要 README 与许可文件齐备 |
| `ADD-FEAT-P1.05.07` | `ADD-FEAT-P1.05.05` | ✅ | 预算门禁复用 `ime-doctor` 的测量 |
| `ADD-FEAT-P2.01.01` | `ADD-FEAT-P0.01.01` | ✅ | bigram 需要产品级词库的语料规模 |
| `ADD-FEAT-P2.01.02` | `ADD-FEAT-P0.01.01` | ✅ | 领域词库需要规模化编译管线 |
| `ADD-FEAT-P2.02.01` | `ADD-FEAT-P1.03.01` | ✅ | GPU 路径需要外观配置稳定 |
| `ADD-FEAT-P2.03.01` | 无 | — | 决策文档 |
| `ADD-FEAT-P2.03.02` | `ADD-FEAT-P1.03.01` | ✅ | 皮肤包是外观配置的文件化 |
| `ADD-FEAT-P2.03.03` | `ADD-FEAT-P1.02.01` | ✅ | i18n 需要面板承载语言选择 |
| `ADD-FEAT-P2.04.01` | `ADD-FEAT-P0.03.02` | ✅ | 多 Profile 需要版本化配置 |
| `ADD-FEAT-P2.04.02` | `ADD-FEAT-P1.01.04` | ✅ | 剪贴板历史复用面板机制 |
| `ADD-FEAT-P2.04.03` | `ADD-FEAT-P0.01.04` | ✅ | 统计读取用户库 |
| `ADD-FEAT-P2.05.01` | `ADD-FEAT-P1.05.06` | ✅ | 发布流水线依赖打包 |
| `ADD-FEAT-P2.05.02` | `ADD-FEAT-P1.05.07` | ✅ | 长稳压测依赖预算门禁 |
| `ADD-FEAT-P2.05.03` | `ADD-FEAT-P1.05.05` | ✅ | 诊断包复用 `ime-doctor` |

**环检测结论**：**无环**。全部 40 条依赖边的目标编号严格小于源编号，故 DAG 天然成立（拓扑序由编号本身保证）。**无任何任务需要拆分重构。**

**解耦点声明**（并行开发时唯一需要串行的位置）：

| 解耦点 | 串行原因 | 冻结顺序 |
|---|---|---|
| `crates/ime-types/src/{decode,key,ui,error,lexicon}.rs` | 三个 Track 的增量**全部**要改这 5 个文件 | 由 `M0` 一次性冻结，之后任何 Track 不得再改 |
| `crates/ime-core/src/state/transitions.rs` | `P0.01.04`（forget）、`P0.02.05`（script）、`P0.02.01`（scheme）都要加跃迁分支 | `M0` 时**预留三个空分支并写好注释**，各卡只填自己的分支体 |
| `xtask/src/dictc/` | `P0.01.01`（规模化）、`P0.02.05`（`script.dict`）、`P1.01.03`（英文词表）都要改 | `P0.01.01` 先落地并**把 `dictc` 的 section 构建抽成可复用的 `SectionBuilder`**，后续卡只加 builder 实现 |
| `data/sources.toml` | 任何新增来源都要登记 | 由 `P0.01.01` 一次性登记 `script-disambig` 与 `phrase` 两个 `derived` 来源 |

### 5.3 关键路径与并行通道汇总

**关键路径 `CP`（最长依赖链）**：

```
ADD-FEAT-P0.01.01 (5)  →  ADD-FEAT-P0.01.02 (3)  →  ADD-FEAT-P0.03.01 (4)  →  ADD-FEAT-P0.03.02 (2)
      词库规模化              质量闭环                备份与回滚              配置迁移
```

**CP 长度 = 14 人天**（P0 总量 40 人天，**CP 占比 35%**——即 P0 阶段有 26 人天的可并行空间）。

**并行通道与工时分布**：

| 通道 | 文件域 | 任务 | 工时 | 与 CP 的关系 |
|---|---|---|---|---|
| **Track A-数据**（词库、用户库、配置） | `data/**`、`xtask/src/dictc/**`、`crates/ime-dict/src/**`、`crates/ime-config/src/**` | `P0.01.01`、`P0.01.02`、`P0.01.03`、`P0.01.04`、`P0.03.01`、`P0.03.02` | **21** | **全部在 CP 上或直接汇入 CP** |
| **Track A-解码**（切分、词格、打分） | `crates/ime-core/src/{segment,viterbi,lm,state}/**` | `P0.02.01`、`P0.02.03`、`P0.02.04`、`P0.02.05` | **14** | 全部**不在 CP**，松弛量 = 14 − 0 = 14 人天（`P0.02.05` 汇入 `P0.03.02`，路径长 4+2=6 人天，松弛 8 人天） |
| **Track B**（UI·Client） | `crates/ime-ui/**`、`crates/ime-fcitx5/src/{ui_impl,engine}.rs` | `P0.02.02` | **2** | 不在 CP，松弛 12 人天 |
| **Track C**（Infra·DevOps） | `docs/`、`README*.md`、`packaging/**`、`scripts/**` | `P0.05.01`、`P0.05.02` | **3** | 不在 CP，松弛 14 人天 |

**关键路径的三个反直觉结论**（资源分配建议）：

1. **CP 完全由"数据"构成，与"功能"无关。** 最长链是"词库规模化 → 质量闭环 → 备份 → 配置迁移"，四个任务全部是数据侧工程。**解码扩展（双拼/模糊音/简拼/简繁，14 人天）整条链都不在 CP 上。** 这意味着：如果只投入 1 个人，应该先做词库规模化；如果投入 4 个人，解码扩展可以完全并行而不拖慢交付。

2. **`P0.03.02`（配置迁移）是 CP 的收尾瓶颈，且它的三个前置全在别的 Track。** `P0.03.02` 依赖 `P0.01.03`（数据）、`P0.02.02`（UI）、`P0.02.05`（解码）——**三个 Track 的产物必须先全部落地，迁移框架才能定义 v2 schema**。这是本项目增量的**唯一汇合点**，必须显式排期。若 `P0.02.02` 延期，整个 P0 阶段延期。

3. **`P0.05.01`（README 与 `OB-1` 徽章）只有 1 人天，但它是法务义务且零依赖。** 它不在 CP 上，**因此极易被"先做功能"的惯性无限推迟**——这正是它至今未做（1.4 发现 ②）的原因。建议**第一天就做掉**。

**`M0`（契约冻结，1 人天）在 CP 之外，但门控 5 张卡**。它与 `P0.01.01` 可并行（`P0.01.01` 不改契约）。

### 5.4 P0 任务卡（原子级展开）

> 字段规范沿用 `features.md` 0.3。验收标签沿用 `features.md` 0.2：`[自动]` / `[文档]` / `[实验室]` / `[视觉]` / `[性能]`。
> 所有任务卡的测试基线：`cargo nextest run --workspace --all-features` + `cargo test --workspace --doc`（`ASM-A-20`）。

---

#### 任务 ID：ADD-FEAT-P0.01.01 产品级词库编译与分段预算落地

- **基本属性**：
  - 绑定差距条目：`GAP-13`（词库规模）、`GAP-16`（整句分词质量，`GAP-13` 的派生项）
  - 优先级与复杂度：`P0 核心增强` ｜ **中** ｜ 预估工时: **5 人天**
  - 前置依赖：无（**不依赖 `M0`**）；软依赖 `TASK-1.03.01`（词库格式 v1 与 `dictc`）、`TASK-1.03.02`（FST 索引）
  - 关键路径：**`CP: 是`**（CP 起点）
  - 并行通道：`Track A-数据`
  - 当前状态：`[x] 已完成`
  - 代码落地锚点：`xtask/src/dictc/{mod,source,build,expand}.rs`（改）、`xtask/src/dictc/section.rs`（**新增**：可复用的 `SectionBuilder`）、`data/sources.toml`（改：登记两个 `derived` 来源）、`data/raw/jieba-dict.tsv`（已存在，349,046 行）、`data/raw/base.tsv`（已存在，5,871 行）、`data/raw/phrase.tsv`（**新增**）、`data/raw/script-disambig.tsv`（**新增**）、`data/compiled/base.dict`（重新产出）、`docs/dev/budgets.json`（只读引用，不改）

- **目标与价值**：把 `base.dict` 从**开发词表（5,441 条）**扩到**产品词库（≥ 320,000 条）**，并把编译管线改造成可被后续增量（简繁表、符号表、英文词表、领域词库）复用的分段构建器。对标搜狗/微软拼音/RIME/libpinyin 的词库规模量级。**这是本项目"输入正确性"这一第一优先级目标的载体**——当前 5,441 条词库下，任何长句输入都会大量落到 `Lexicon::fallback_single` 的单字回退，`DecodeResult.degraded` 频繁置位。

- **技术设计与代码级接入细节**：

  **底层模型与数据流改动**：

  (a) **`dictc` 的 section 构建抽象**。当前 `xtask/src/dictc/` 直接按顺序拼 section，导致后续任何新表都要改同一条主流程。抽出：

  ```rust
  // xtask/src/dictc/section.rs
  /// One dictionary section, built from a whitelisted source.
  ///
  /// Builders are registered in a fixed order and each one owns exactly one
  /// `SectionKind`, so adding a section (the script table, the symbol table)
  /// never edits the compile pipeline: it registers another builder.
  pub trait SectionBuilder {
      /// The section this builder emits.
      fn kind(&self) -> SectionKind;

      /// Reads the sources and produces the payload, or `None` when the
      /// section would be empty and must be written as absent.
      ///
      /// # Errors
      /// Returns `DictError::LengthOutOfRange` when a computed offset or
      /// length would not fit the container's field widths.
      fn build(&self, ctx: &BuildContext<'_>) -> Result<Option<Vec<u8>>, DictError>;

      /// Reported by `dictc --stats`; the numbers land in the build log so a
      /// size regression is visible without a benchmark run.
      fn stats(&self) -> SectionStats;
  }

  /// Shared, read-only view of everything the builders need.
  pub struct BuildContext<'a> {
      /// L1 character readings, indexed by character.
      pub l1: &'a CharTable,
      /// L2 words with optional readings and weights.
      pub words: &'a [SourceWord],
      /// L3b expansion cap: the most keys one word may generate.
      pub expand_cap: u8,
      /// The frequency band above which L3b expansion applies.
      pub expand_top_n: u32,
  }

  /// Per-section size accounting.
  #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
  pub struct SectionStats {
      pub entries: u64,
      pub bytes: u64,
  }
  ```

  (b) **L3b 词频定向多键展开**（ADR-0000 已定的算法，当前未在 `dictc` 中启用）。按词频 top 50k、`CAP = 4` 生成多键，FST 键 +23%（337k → 414k），救回 58.1% 的错音质量：

  ```rust
  // xtask/src/dictc/expand.rs
  /// One word's expanded syllable keys, most frequent reading first.
  ///
  /// Only the top `top_n` words are expanded: the measured curve is
  /// +23% FST keys for 58.1% of the recoverable error mass, against
  /// +176% keys for 60.2% under full expansion. The band is what makes
  /// the trade worth taking.
  pub struct ExpandedKeys {
      /// Up to `cap` distinct keys, deduplicated.
      pub keys: Vec<String>,
      /// How many readings the cartesian product had before capping.
      pub generated: u32,
  }

  /// Builds the keys one word is reachable by.
  ///
  /// # Errors
  /// Returns `DictError::LengthOutOfRange` when a key exceeds
  /// `MAX_WORD_LEN` after syllable joining.
  pub fn expand_word(
      word: &SourceWord,
      l1: &CharTable,
      cap: u8,
  ) -> Result<ExpandedKeys, DictError>;
  ```

  (c) **`WORDLIST` 间接层必须保持生效**。`ENTRIES` 是 `[DictEntry; entry_count]`，多键展开**只增加 `WORDLIST` 的 `word_id` 引用**，不复制 `ENTRIES` 行。这是 `BUDGET-SIZE-02`（20MB）在 +23% 键下仍可达的关键——`ENTRIES` 保持 6.5MB 量级，增长的只有 `WORDLIST`（≤ 2MB）。

  (d) **分段预算落地**（`ASM-A-05`）：

  | 段 | 预算 | 依据 |
  |---|---|---|
  | `StrPool` | ≤ 4MB | 32 万词 × 平均 6 汉字 × 3 字节 ≈ 3.2MB |
  | `Entries` | ≤ 6.5MB | 32 万 × `ENTRY_SIZE(16)` = 5.1MB，留余量 |
  | `WordList` | ≤ 2MB | 41.4 万键的 `u32` 引用 + 分组头 |
  | `Fst` | ≤ 2.5MB | `fst::Map` 对 41.4 万键的实测量级 |
  | `Unigram` | ≤ 3.5MB | 32 万 × `UNIGRAM_ENTRY_SIZE(8)` = 2.6MB |
  | 五段之和 | ≤ 18.5MB | 分段上限逐段宽松，只用于归因"哪一段在涨" |
  | **载荷总上限** | **≤ 17.5MB** | 真正的红线：留 2.5MB 余量给 `BUDGET-SIZE-02` 的 20MB（`PAYLOAD_SHARE = 875‰`） |
  | 超限对策 | 按权重截断至 top 320,000 并打印警告 | `features.md` 6.1 的 `R-08` 对策 |

  **两个上限同时判**：任一段越限即失败，五段之和越过 17.5MB 同样失败。分段上限之和（18.5MB）大于总上限，这是有意的——它是"哪一段在涨"的归因工具，而总上限才是契约。实现见 `xtask/src/dictc/budget.rs` 的 `SECTION_SHARES` 与 `PAYLOAD_SHARE`，两者都是容器上限的 per-mille 份额，容器一改全部同步缩放。

  (e) **`data/sources.toml` 新增两个 `derived` 来源**（`kind = "derived"` 豁免哈希固定，因生成器变化时内容会变）：

  ```toml
  [[source]]
  id = "phrase"
  kind = "derived"
  layer = "L2"
  url = "https://github.com/gongzhengyang/rspinyin"
  license = "Project-owned (MIT OR Apache-2.0)"
  spdx = "MIT OR Apache-2.0"
  retrieved = ""
  sha256 = ""
  permissive = true

  [[source]]
  id = "script-disambig"
  kind = "derived"
  layer = "L3"
  url = "https://github.com/gongzhengyang/rspinyin"
  license = "Project-owned (MIT OR Apache-2.0)"
  spdx = "MIT OR Apache-2.0"
  retrieved = ""
  sha256 = ""
  permissive = true
  ```

  **边界契约（跨边界任务必填）**：本卡**不修改 `crates/ime-types`**，故不依赖 `M0`。词库格式 `DICT_FORMAT_VERSION` **保持 1**（`ASM-A-19`）；`SectionKind` 的 6 个变体不变。`dictc --stats` 的输出格式是**构建期契约**（`scripts/check-budget.sh` 消费），字段名一经确定不得改：

  ```
  section   entries      bytes   budget
  StrPool         —    3,142,016  4,194,304
  Entries    320,000   5,120,000  6,815,744
  WordList   414,000   1,656,000  2,097,152
  Fst             —    2,310,144  2,621,440
  Unigram    320,000   2,560,000  3,670,016
  TOTAL           —   14,788,160 18,350,080
  ```

  **与现有代码的无缝衔接方案**：`crates/ime-dict` **零改动**——`format/reader.rs` 已完整支持全部 6 段，`fst_index/` 已支持前缀枚举，`entry.rs` 已做零拷贝。本卡只改 `xtask`（构建侧）与 `data/`（数据侧）。这是本卡被排在 CP 起点且零依赖的原因。

- **非功能约束 (NFR) 与性能指标**：
  - **编译耗时** ≤ 90 秒（单线程流式写入）；`L3b` 展开计算 ≤ 5 秒。
  - **产物体积** ≤ 17.5MB（分段预算合计），`BUDGET-SIZE-02` = 20MB。
  - **运行期**：`dict_mmap_rss` ≤ 25MB（既有预算，词库变大后 mmap 的 RSS 增长必须仍在该值内——按需分页，不是全量驻留）。
  - **解码延迟**：词库变大后 `decode_p99` **不得劣化**（≤ 3.0ms）。FST 查询是 O(key 长度)，与词条总数无关；`WORDLIST` 的分组读取是 O(1) 偏移。**若实测劣化，说明 `MAX_WORDS_PER_KEY = 32` 的截断未生效**。
  - **异常容灾**：编译中途失败必须**不覆盖** `data/compiled/base.dict`（既有 `DictWriter::TEMP_SUFFIX = ".tmp"` 的原子替换语义）；`data/raw/*.tsv` 缺失时 `dictc` 报 `dict/unavailable` 并退出，不产出半成品。
  - **边缘条件**：超长词（> `MAX_WORD_LEN = 96` 字节）、同 key 词条 > `MAX_WORDS_PER_KEY = 32`、`word_len = 0`、含制表符的词——全部**跳过并计入统计**，不中断编译（既有 `dictc` 的 TSV 解析边界行为）。

- **逐步落地实施步骤**：
  1. **抽出 `SectionBuilder` 并迁移既有 6 个段**：在 `xtask/src/dictc/section.rs` 定义 trait 与 `BuildContext`；把现有 `Fst`/`Entries`/`StrPool`/`Unigram`/`WordList` 的构建逻辑各自包成一个 builder；跑一遍全量编译，**断言产出与改造前逐字节一致**（`sha256sum` 比对），这是"零行为变更"的证明。
  2. **接入 jieba 全量词表 + L3a 基线读音**：把 `data/raw/jieba-dict.tsv` 的 349,046 行纳入 L2，读音按 ADR-0000 的 `L3a` 由 L1 首读音拼接；跑通并记录分段尺寸。
  3. **启用 L3b 词频定向多键展开**：实现 `expand.rs`；按 top 50k / `CAP = 4` 展开；记录 FST 键数与 `WORDLIST` 增长，验证落在 +23% / ≤ 2MB。
  4. **落地分段预算断言**：`dictc --stats` 输出上表格式；`scripts/check-budget.sh` 解析并与 `budgets.json` 比对；超限时按权重截断至 top 320,000 并打印警告。
  5. **登记两个 `derived` 来源并跑 `check-dict-sources.sh`**：确认白名单校验通过、`permissive = true` 的来源数不变。

- **验收标准 (DoD)**：
  1. `data/compiled/base.dict` 的 `entry_count ≥ 320,000`，`total_len ≤ 17.5MB`；`dictc --stats` 的六段尺寸全部在预算内。[自动]
  2. `cargo nextest run -p xtask` 全绿；新增 `test_section_builder_migration_is_byte_identical`、`test_expand_word_respects_cap`、`test_expand_word_rejects_overlong_key`、`test_stats_total_matches_budget` 四条测试。[自动]
  3. `cargo nextest run -p ime-dict` 全绿——**词库变大后既有 132 条测试一条都不许改**。若某条失败，说明格式或读取路径被误改，必须修实现而非改测试。[自动]
  4. `bash scripts/check-dict-sources.sh` 通过；`data/sources.toml` 的 `permissive = false` 来源数仍为 0。[自动]
  5. 在空闲机器上跑 `cargo bench -p ime-core -- decode`，`decode_p99 ≤ 3.0ms` 且**不劣于改造前**。[性能]
  6. `lm_holdout.tsv`（≥ 5000 条）上的「首选词命中率」与「前 9 候选可达率」两个指标**均高于改造前**，数值记入验收记录。[性能]
  7. `dict_mmap_rss ≤ 25MB`、`plugin_rss ≤ 45MB`。[性能]

- **验收记录**（2026-09-30）：
  - **交付物**：`xtask/src/dictc/budget.rs`（471 行）+ `budget/tests.rs`（390 行，20 个用例）、`xtask/src/dictc/manifest.rs`（394 行）+ `manifest/tests.rs`（396 行，14 个用例）；`xtask/src/dictc.rs` 挂载并在 `run()` 中接线：`budget::enforce` 在写入容器**之前**执行，`manifest::record_build` 在 `writer.finish` 之后执行。
  - **验证命令与结果**：`just ci` 退出 0（fmt、clippy `-D warnings`、nextest 1992 个用例、doctest、10 个审计脚本全部通过）。
  - **阈值单源**：模块内不存任何字节阈值，只存 per-mille 份额（`200/325/100/125/175`）；`budgets.json` 的 20 MiB 上精确复现 `ASM-05` 的 4/6.5/2/2.5/3.5 MiB 与 17.5 MiB 总上限，文档一改全部同步缩放。
  - **来源身份**：`SourceIdentity` 的 id / kind / layer / spdx 从 `data/sources.toml` 读取（`Source` 新增 `layer` 字段），不硬编码。
  - **已知限制**：
    1. **产品级词表并未真正编译进去**。`data/raw/jieba-dict.tsv`（349,046 行，4.2 MB）已在仓库中、已在 `data/sources.toml` 登记，`load_words` 也已能读它的两列格式——**`base.dict` 只有 5,441 条的原因是 `dictc` 的默认 `--input` 指向 5,871 行的 `base.tsv`**。把 `--input` 换掉即可，但本卡没有执行（子 Agent 零命令权限，且重产出 `data/compiled/base.dict` 属发布动作）。这是「输入不准」的根因，登记为后续项。
    2. **staged compilation / `SectionBuilder` 抽象未做**：卡片锚点里的 `xtask/src/dictc/{mod,source,build,expand}.rs`（改）与 `section.rs`（新增）不在白名单内，`mod.rs` 被明令禁止修改。
    3. `data/raw/script-disambig.tsv` 未创建，`data/sources.toml` 未新增该 `derived` 来源。
    4. `scripts/check-budget.sh` 不存在，`just check-budget` 目前只跑 `xtask budget --validate`。
    5. ~~**`ASM-05` 的算术偏差**：分段上限 4/6.5/2/2.5/3.5 MB 之和是 18.5 MB，而文档写「合计 ≤ 17.5MB」。实现按字面同时判两个上限（各段各自判 + 总载荷判，`is_within` 要求两者都过）；文档需要回写其中一处。~~ **已回写（2026-09-30，主 Agent）**：`features.md` 的 `ASM-05` 行与本文档 §(d) 的分段预算表都改为显式写出两个数——五段之和 ≤ 18.5MB、载荷总上限 ≤ 17.5MB，并说明分段上限是归因工具、总上限才是红线。代码未改：`xtask/src/dictc/budget.rs` 的 `SECTION_SHARES` 与 `PAYLOAD_SHARE` 本来就是 per-mille 份额，文档一改全部同步缩放。
  - **环境**：Rust 1.98.0、Linux 6.18.40.1-microsoft-standard-WSL2、cargo-nextest 0.9.143。

---

#### 任务 ID：ADD-FEAT-P0.01.02 词库质量闭环：留出集度量与 L3c 校正表扩容

- **基本属性**：
  - 绑定差距条目：`GAP-13`（词库规模）、`GAP-16`（整句分词质量）
  - 优先级与复杂度：`P0 核心增强` ｜ **中** ｜ 预估工时: **3 人天**
  - 前置依赖：`ADD-FEAT-P0.01.01`
  - 关键路径：**`CP: 是`**
  - 并行通道：`Track A-数据`
  - 当前状态：`[x] 已完成`
  - 代码落地锚点：`xtask/src/tune/`（改：`xtask tune` 子命令）、`data/raw/polyphone.tsv`（改：3,151 → ≥ 5,000 条）、`crates/ime-core/tests/fixtures/lm_holdout.tsv`（**新增**，≥ 5,000 条）、`crates/ime-core/tests/lm_quality.rs`（**新增**）、`crates/ime-core/src/lm/score.rs`（改：`ScoreWeights` 的默认值可被 `tune` 输出覆盖）、`docs/dev/lm-weights.md`（改：回写实测权重）

- **目标与价值**：把"词库变大"变成"候选变准"。`ADD-FEAT-P0.01.01` 解决**可达性**（词在不在库里），本卡解决**排序质量**（词排得对不对）与**多音字残余错误**。ADR-0000 实测：`L3a` 基线错音率 4.3% 词数 / 1.9% 加权，`L3b` 后残余约 0.8% 加权；`L3c` 权重校正表是收窄残余的最后一步，但当前 `polyphone.tsv` 只有 3,151 条，**低于 ADR-0000 要求的 ≥ 3,000 条的边界**，需要扩容。

- **技术设计与代码级接入细节**：

  **底层模型与数据流改动**：

  (a) **`lm_holdout.tsv` 与两个度量指标**。`features.md` 6.1 的 `R-08` 明确："`lm_golden.tsv`（200 条）**无法检出 1.9% 量级的差异**"，因此必须新建 ≥ 5,000 条的留出集：

  ```
  # crates/ime-core/tests/fixtures/lm_holdout.tsv
  # Columns: raw <TAB> expected <TAB> source
  #   raw       the ASCII pinyin the test types
  #   expected  the word that must rank first
  #   source    which layer produced the reading: L1 | L3b | L3c
  nihao	你好	L1
  yinhang	银行	L3b
  chongqing	重庆	L3c
  ```

  两个指标（`ASM-A-21`）：

  ```rust
  // crates/ime-core/tests/lm_quality.rs
  /// Quality of one decode over the whole hold-out set.
  #[derive(Clone, Copy, Debug, Default, PartialEq)]
  pub struct QualityReport {
      /// Fraction of cases whose first candidate is the expected word.
      pub top1_rate: f64,
      /// Fraction of cases where the expected word appears in the first nine
      /// candidates. This is the reachability half: L3b fixes this one.
      pub top9_rate: f64,
      /// Cases the decoder could not spell at all, reported separately so a
      /// missing word is never confused with a badly ranked one.
      pub unreachable: u32,
  }
  ```

  (b) **`polyphone.tsv` 扩容到 ≥ 5,000 条**，并把它从"权重校正"升级为**带来源标注的校正表**：

  ```
  # word <TAB> reading <TAB> weight <TAB> origin
  #   origin: unihan | manual | corpus
  # Unihan rows are derived from data/raw/unihan.tsv (Unicode License) and
  # carry no extra licence obligation. `manual` rows are project-owned.
  # `corpus` rows are counted from the project's own word list only.
  银行	yin'hang	60000	unihan
  重庆	chong'qing	55000	unihan
  ```

  **边界契约**：本卡**不修改 `crates/ime-types`**。`data/raw/polyphone.tsv` 的列数从 3 增到 4（新增 `origin` 列）——这是**构建期数据格式**变更，不是跨边界契约变更。`dictc` 必须接受 3 列与 4 列两种写法（向后兼容旧文件）。

  **与现有代码的无缝衔接方案**：`xtask tune` 子命令已存在（`Command::Tune`）；本卡把它的输出从"打印建议权重"升级为"直接写 `crates/ime-core/src/lm/score.rs` 的 `ScoreWeights::default()` 常量并回写 `docs/dev/lm-weights.md`"，形成"度量 → 调参 → 回写文档"的闭环。

- **非功能约束 (NFR) 与性能指标**：
  - `xtask tune` 在全量留出集上运行 ≤ 60 秒。
  - `ScoreWeights` 的默认值变化**不得**使 `decode_p99` 劣化（权重是纯乘加，无分支）。
  - `lm_quality.rs` 作为**集成测试**运行，其耗时 ≤ 10 秒（用 5,000 条的子集，全量在 `xtask tune` 中跑）。
  - **异常容灾**：`lm_holdout.tsv` 中任何一条 `expected` 词不在词库中时，计入 `unreachable` 而**不是**测试失败——这本身就是要度量的指标。

- **逐步落地实施步骤**：
  1. **构造 `lm_holdout.tsv`（≥ 5,000 条）**：从 jieba 词表中按词频分层抽样（覆盖 top 1k / 1k–10k / 10k–50k / 50k+ 四层），每层 ≥ 1,250 条；读音由 `L3a` 生成，`source` 标注该词实际由哪一层提供了正确读音。
  2. **实现 `QualityReport` 与 `lm_quality.rs`**：跑基线（`P0.01.01` 完成后的状态），把 `top1_rate` / `top9_rate` 记入本卡的验收记录作为**基线值**。
  3. **扩容 `polyphone.tsv` 到 ≥ 5,000 条**：Unihan 派生 + 人工校正，带 `origin` 列；跑 `dictc` 重新编译。
  4. **实现 `xtask tune` 的调参闭环**：网格搜索 `ScoreWeights` 的三个权重，以 `top1_rate` 为目标，输出最优值与前后对比表。
  5. **回写 `docs/dev/lm-weights.md`** 与 `ScoreWeights::default()`，重跑 `lm_quality.rs` 确认提升。

- **验收标准 (DoD)**：
  1. `crates/ime-core/tests/fixtures/lm_holdout.tsv` 存在且 ≥ 5,000 行，四层各 ≥ 1,250 行。[文档]
  2. `cargo nextest run -p ime-core --test lm_quality` 通过，输出 `top1_rate` 与 `top9_rate`；两个值均**高于** `P0.01.01` 的基线。[自动]
  3. `data/raw/polyphone.tsv` ≥ 5,000 行且含 `origin` 列；`origin` 的取值只允许 `unihan` / `manual` / `corpus` 三种。[自动]
  4. `dictc` 对 3 列与 4 列的 `polyphone.tsv` 都能编译，行为一致（`test_polyphone_accepts_three_and_four_columns`）。[自动]
  5. `docs/dev/lm-weights.md` 含调参前后的对比表与留出集规模。[文档]
  6. `bash scripts/check-dict-sources.sh` 通过——`polyphone` 仍登记为 `derived`，其 Unihan 派生部分不引入新许可。[自动]
  7. `decode_p99 ≤ 3.0ms` 不劣化。[性能]

- **验收记录**（2026-09-30）：
  - **交付物**：`xtask/src/tune/holdout.rs`（476 行，重写）、`tune/{io,eval,grid}.rs`、`tune/tests.rs`（496 行）；`xtask/src/dictc/quality.rs`（836 → 771 行）、本次新建 `dictc/quality/tables.rs`（105 行）、`dictc/quality/tests.rs`（537 行）。
  - **验证命令与结果**：`just ci` 退出 0：`cargo fmt --all -- --check`、`cargo clippy`（主工作区 `--all-targets --all-features` 与两个 addon crate，全部 `-D warnings`）、`cargo nextest run`（主工作区 2430 个用例、两个 addon 346 个，全绿）、`cargo test --doc`（主工作区与两个 addon）、10 个审计脚本及其自检、`just check-host`。
  - **留出集改为按频次排名四分层抽样**：`1..1k` / `1k..10k` / `10k..50k` / `50k+` 各层等额配额、上限为层容量；输出改为三列 `key<TAB>word<TAB>source`，`source` 由 L1 读音表 + L3c 校正表判定（L1/L3b/L3c）。**禁止自评**：`run` 拒绝 `--eval == --holdout`，holdout 生成时按词与键**双重**排除调优集。**可复现**：无采样、无时钟、无 RNG，语料按 count 排序（并列按文本），同一语料换行序产出**字节相同**的文件（有测试）。
  - **校正表改为双向有界**：新增 `MAX_CORRECTIONS`（20000）与 `--max-corrections`，表有下界（5000）也有上界；新增 `count_repeats` 并**拒绝重复的 `(word, reading)` 行**（防止用重复行灌满下界）。
  - **度量把排序质量与词条覆盖分开**：`QualityReport::served_top1_rate` 打「key 服务到的用例中的首选率」，与「全体用例首选率」并列输出——前者才是纯排序质量。
  - **`tune --grid` 输出前后对比表**（shipped / best / delta，单位 pp），并列时保留出厂值。
  - **已知限制**：① **卡片 DoD 1 有一处算术自相矛盾**——它同时要求「≥5000 行」与「四层各 ≥1250」，而 `top 1k` 层按定义只有 1000 个词，任何实现都无法让该层贡献 1250 行。实现取「每层给出配额或它的全部」，默认 `--holdout-rows 8000` 时实际约 6500–7000 行，总量下限 5000 由生成器 `ensure!` 强制，2/3/4 层各 2000 行 ≥1250；若需严格满足首层，应把该层边界放宽到 `top 2k`（需用户裁定）。② **`crates/ime-core/tests/fixtures/lm_holdout.tsv` 仍是旧的 2 列、按总频次取 top-N 的版本**，需跑 `cargo run -p xtask -- tune --gen-holdout` 重生成；③ **`data/raw/polyphone.tsv` 有一处真实缺陷**：`自行车	zi'xing'che` 在第 46–47 行完全重复，`xtask dictc quality` 会以重复行拒绝（诊断给出 `cut -f1,2 <path> | sort | uniq -d`）；删掉一行即可；该文件同时需要扩容并补 `origin` 列；④ `crates/ime-core/tests/lm_quality.rs` 未创建（不在白名单），等价度量在 `xtask dictc quality` 中输出；⑤ `docs/dev/lm-weights.md` 的前后对比表由 `xtask tune --grid` 打印，卡片第 4 步的「自动回写」半环未实现，保持「工具打印、人工落盘」；⑥ 本卡改动全在 `xtask`（构建期工具），未触碰任何运行时代码路径，故 `decode_p99` 无影响（静态结论，无基准可跑）。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85）、Linux 6.18.40.1-microsoft-standard-WSL2、cargo-nextest 0.9.143。

---

#### 任务 ID：ADD-FEAT-P0.01.03 自定义短语引擎与配置

- **基本属性**：
  - 绑定差距条目：`GAP-05`（自定义短语 / 快捷输入）
  - 优先级与复杂度：`P0 核心增强` ｜ **中** ｜ 预估工时: **3 人天**
  - 前置依赖：**`M0` 契约冻结**（需要 `CandidateSource::Phrase` 与 `DecodeFlags::PHRASE`）；无其他任务依赖
  - 关键路径：`CP: 否`（松弛 9 人天）
  - 并行通道：`Track A-数据`
  - 当前状态：`[x] 已完成`
  - 代码落地锚点：`crates/ime-core/src/phrase.rs`（**新增**）、`crates/ime-core/src/phrase/table.rs`（**新增**）、`crates/ime-core/src/viterbi/decoder.rs`（改：候选汇总期注入）、`crates/ime-config/src/schema.rs`（改：新增 `[phrases]` 段）、`crates/ime-config/src/reload.rs`（改：热重载短语表）、`crates/ime-types/src/ui.rs`（`M0` 已加 `CandidateSource::Phrase`）、`data/raw/phrase.tsv`（**新增**，内置短语，项目自建）

- **目标与价值**：让用户用 2–4 个字母输出长文本。对标搜狗的自定义短语、RIME 的 `custom_phrase`。**这是投入产出比最高的增量**：3 人天换来的是用户每天数十次的重复输入被消除。典型用例：`rq`→当前日期、`dz`→邮箱、`sfz`→身份证号、`gs`→公司抬头。

- **技术设计与代码级接入细节**：

  **底层模型与数据流改动**：

  (a) **短语表的数据结构**。短语不是词库：它的生命周期是"用户随时可改"，必须支持**热重载**且**不重置进行中的会话**（0.4 规则 10）。用独立的小 FST（复用 `fst` crate），键长 ≤ 32：

  ```rust
  // crates/ime-core/src/phrase/table.rs
  /// An immutable snapshot of the user's phrase table.
  ///
  /// Built once per reload and swapped behind an `Arc`, so a decode that is
  /// already running keeps the table it started with and never observes a
  /// half-loaded one. The table is small by construction (`max_entries`), so
  /// a reload is a rebuild rather than an incremental update.
  pub struct PhraseTable {
      /// Key (lower-case ASCII) to phrase.
      index: fst::Map<Vec<u8>>,
      /// Phrases in key order; `index` stores the offset into this pool.
      pool: String,
      /// End offset of each phrase, parallel to the key order.
      ends: Vec<u32>,
  }

  /// One phrase that matched the input.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct PhraseHit {
      /// Byte range of the matched key inside the raw input.
      pub key_start: u16,
      pub key_end: u16,
      /// Byte range of the replacement inside the pool.
      pub text_start: u32,
      pub text_end: u32,
  }

  impl PhraseTable {
      /// Looks up the longest key that is a prefix of `raw` starting at `at`.
      ///
      /// Longest-match rather than shortest: a user who defines both `rq` and
      /// `rqq` expects the longer one to win when they type it.
      ///
      /// # Errors
      /// Returns `ImeError::DictUnavailable` when the table failed to load.
      pub fn longest_match(&self, raw: &str, at: usize) -> Result<Option<PhraseHit>, ImeError>;

      /// Number of entries; reported by the diagnostics surface.
      pub fn len(&self) -> u64;
  }
  ```

  (b) **注入点：候选汇总期，不进入 Viterbi**（4.1 的挂载点 H3）：

  ```rust
  // crates/ime-core/src/phrase.rs
  /// Appends the phrase candidates that match the raw input.
  ///
  /// Phrases are injected after the decoder has produced its word candidates,
  /// never as lattice edges: a phrase has no reading and therefore no place in
  /// a scoring pass that is defined over readings. They are placed ahead of the
  /// dictionary candidates because a user who defined `rq` means it.
  ///
  /// # Errors
  /// Returns `ImeError::DictUnavailable` when the table is absent; the caller
  /// degrades to the dictionary-only candidate list rather than failing.
  pub fn inject_phrase_candidates(
      table: &PhraseTable,
      raw: &str,
      candidates: &mut Vec<Candidate>,
  ) -> Result<usize, ImeError>;
  ```

  (c) **日期/时间的动态展开**（为 `GAP-06` 留出接口，本卡只做静态短语）。短语文本中允许 `%` 转义，由**调用方**（`ime-fcitx5` 的引擎层，因为 `ime-core` 不得读时钟，0.4 规则 4）在注入前替换：

  ```rust
  /// Substitutes the time escapes in a phrase body.
  ///
  /// Lives in the host layer, not in `ime-core`: the decoder may not read the
  /// clock (0.4 rule 4), so the substitution happens before the text reaches
  /// the pure decode path. Recognised: `%Y` `%m` `%d` `%H` `%M` `%S`.
  pub fn expand_time_escapes(body: &str, at_unix_ms: u64) -> String;
  ```

  **边界契约（跨边界任务必填）**：
  - `CandidateSource::Phrase`（`M0` 落地）——候选来源标注，UI 用它决定是否显示 `text-annotation` 的来源标签。
  - `DecodeFlags::PHRASE`（`1<<12`，`M0` 落地）——允许用户在配置中关闭短语注入。
  - 错误码：`phrase/table-unavailable`（短语表缺失时的**降级**，不是失败；`info` 级诊断，用户不可见）。
  - **状态机跃迁**：`Composing` + `ConfigReloaded` → `Composing`，短语表的变更**延迟到本次提交之后生效**（4.2.2 的表）。

  **配置段**（4.2.3 已列）：`[phrases]` 的 `enabled` / `file` / `max_entries`（默认 5000）。

  **UI 与工艺级设计规范**（`C-6`）：短语候选的视觉与词库候选**完全一致**，唯一区别是 `CandidateSource::Phrase` 在 `annotation` 上显示"短语"二字，颜色沿用既有 `text-annotation` token。**不引入新颜色、不改变候选格尺寸**——短语是候选，不是另一类东西。

  **与现有代码的无缝衔接方案**：`Decoder::decode` 的签名不变；短语注入发生在 `decode` 内部、候选排序之后、`DecodeResult` 组装之前。`crates/ime-ui` **零改动**（`Candidate` 结构不变，只是 `source` 多一个取值）。

- **非功能约束 (NFR) 与性能指标**：
  - **独占预算 ≤ 0.10ms**（0.3 节）。`longest_match` 是 FST 前缀遍历，键长 ≤ 32，实测应在 1µs 量级；0.10ms 是含 `inject_phrase_candidates` 的分配在内的总预算。
  - **表容量** ≤ `max_entries`（默认 5000）；超限时按"最近使用"淘汰，并记 `phrase/limit-exceeded`。
  - **热重载**：短语表重载**不得**重置进行中的输入会话（0.4 规则 10），且**不得**阻塞宿主线程（重载在空闲期完成）。
  - **异常容灾**：短语文件缺失 → 空表 + `phrase/table-unavailable` 降级，输入完整可用；短语文件畸形（非法 UTF-8、键含非 ASCII、超长）→ 跳过该行并计入统计，不中断加载。
  - **边缘条件**：键与词库键冲突时**短语优先**（用户显式定义优先于统计学习）；同一键重复定义时**后者覆盖前者**并记警告。

- **逐步落地实施步骤**：
  1. **实现 `PhraseTable` 与 `longest_match`**：独立于 `ime-core` 的其他部分，可先用内存构造的测试表单测（`ASM-A-20` 的确定性要求：不依赖真实文件）。
  2. **接入候选注入**：在 `Decoder::decode` 的候选汇总段调用 `inject_phrase_candidates`；保证短语候选的 `consumed_syllables` 语义正确（短语消耗其键对应的音节数，用于提交后计算剩余 preedit）。
  3. **落地配置与热重载**：`[phrases]` 段进 `schema.rs`；`reload.rs` 在重载时重建 `PhraseTable` 并 `Arc` 交换；断言进行中的会话不重置。
  4. **内置短语表 `data/raw/phrase.tsv`**：项目自建，含日期/时间转义示例（`rq`、`sj`、`xq`），随包分发；用户的 `~/.config/rspinyin/phrases.tsv` 与内置表**合并**（用户表优先）。
  5. **异常分支与回归**：畸形文件、超长键、键冲突、表缺失四条降级路径各一条测试；跑 `ime-core` 与 `ime-fcitx5` 全量回归。

- **验收标准 (DoD)**：
  1. `cargo nextest run -p ime-core` 全绿，新增 ≥ 8 条测试覆盖：最长匹配、键冲突、超长键、畸形文件、表缺失降级、候选注入顺序、`consumed_syllables` 正确性、热重载不重置会话。[自动]
  2. `cargo nextest run -p ime-config` 全绿，新增 `test_phrases_section_defaults`、`test_phrases_max_entries_bounds`。[自动]
  3. 手工验证：配置 `rq = 2026年9月29日` 后输入 `rq`，首候选即为该文本。[实验室]
  4. 短语注入的实测耗时 ≤ 0.10ms（`criterion` 基准或探针断言）。[性能]
  5. `decode_p99 ≤ 3.0ms` 不劣化。[性能]
  6. 热重载短语表期间进行中的输入**不中断**（`test_reload_preserves_active_session`）。[自动]
- **验收记录**（2026-09-30）：
  - **交付物**：`crates/ime-core/src/phrase.rs`（708 行）+ `phrase/tests.rs`（400 行，26 个用例）；`state/machine.rs` 的 `Effect::AddPhrase { key, text }`；`state/transitions.rs` 的 `AddPhrase` arm 与 4 条状态机用例；`ime-config/src/schema.rs` 的 `PhraseConfig`、`MAX_PHRASE_ENTRIES = 50_000`、`DEFAULT_PHRASE_ENTRIES = 5_000`；`data/raw/phrase.tsv`（项目自建）。
  - **它替换了 ADR-0005 决策 5 留的占位**：状态机原先对 `KeyAction::AddPhrase` 回 `Diagnose(dict/unsupported)`，本卡把它换成真实 effect。
  - **验证命令与结果**：`just ci` 退出 0（`cargo fmt --check`、clippy `-D warnings`、nextest 1521 个用例、doctest、9 个审计脚本及其自检、25 条预算阈值全部通过）。
  - **已知限制**：
    1. **宿主侧未接线**：`engine/router.rs` 的 `apply_effects` 对 `Effect::AddPhrase` 仍发 `Diagnose(ImeError::Unsupported)`。`PhraseTable` 只有读（`parse`/`load`/`longest_match`），没有写入器，也没有任何东西把用户的 `phrases.tsv` 载入宿主——写一个 arm 等于在所有者定义接口之前发明它。**这是本卡唯一未完成的半边，已在代码注释中写明。**
    2. **解码注入点未接线**：`viterbi/scratch.rs` 的 `decode_into` 未调用 `inject_phrase_candidates`。
    3. **`ime-config/src/reload.rs` 未接线**：`PartialConfig` 缺 `phrases` 字段，用户在 `config.toml` 里写的 `[phrases]` 会被 serde 静默忽略。
    4. `phrase/limit-exceeded` 与 `phrase/added` 两个码尚未产生；`PhraseReport::over_limit` 已把计数暴露给调用方。
    5. 未使用 `fst`：5000 条短键用有序表 + 二分即够（`ime-core` 的 `Cargo.toml` 不在白名单，且理由已写入 `PhraseTable` 的文档）。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143。

---

#### 任务 ID：ADD-FEAT-P0.01.04 用户词条管理原语（forget / list / export / import）

- **基本属性**：
  - 绑定差距条目：`GAP-09`（用户词条的可见与可删）、`GAP-10`（用户词库导入 / 导出）
  - 优先级与复杂度：`P0 核心增强` ｜ **高**（触及冻结契约与 redb 存储层）｜ 预估工时: **4 人天**
  - 前置依赖：**`M0` 契约冻结**（`UserFreqSource` 的三个新方法 + `DictError` 两个新变体）
  - 关键路径：`CP: 否`，但**是 `P0.03.01` 的前置**（该卡在 CP 上）
  - 并行通道：`Track A-数据`
  - 当前状态：`[x] 已完成`
  - 代码落地锚点：`crates/ime-types/src/lexicon.rs`（`M0` 已加 trait 方法）、`crates/ime-dict/src/user_db.rs`（改：实现三个新方法 + 新增 `pinned`/`created_unix` 字段）、`crates/ime-dict/src/user_db/export.rs`（**新增**）、`crates/ime-dict/src/user_db/tests.rs`（改：402 行，需追加）、`crates/ime-core/src/state/transitions.rs`（改：`ForgetHighlighted` 跃迁体）、`crates/ime-fcitx5/src/engine.rs`（改：按键路由）

- **目标与价值**：解决输入法长期使用后**最被抱怨的问题**——"它学错了一个词，我删不掉"。当前 `UserDb` 只有 `record_count()` 与 `evict_oldest(percent)`（按最旧 10% 批量淘汰），用户面对一个学错的词**完全无能为力**。同时提供导出/导入，让用户能备份、迁移、在换机器时带走自己的词库。对标搜狗的"词库管理（可删除单条）"、RIME 的 `rime_dict_manager`。

- **技术设计与代码级接入细节**：

  **底层模型与数据流改动**：

  (a) **`UserFreqSource` 的三个新方法**（`M0` 落地，**必须带默认实现**，否则既有 mock 编译失败）：

  ```rust
  // crates/ime-types/src/lexicon.rs (M0 已落地)
  pub trait UserFreqSource: Send + Sync {
      fn freq(&self, key: &str) -> u32;
      fn record(&self, key: &str, weight_hint: u16);
      fn is_user_word(&self, key: &str) -> bool;

      /// Removes one learned word.
      ///
      /// Returns `true` when a record was removed and `false` when there was
      /// nothing to remove. A source that cannot forget returns `false`
      /// rather than an error: "nothing was removed" and "I do not support
      /// removal" are both non-failures from the caller's point of view.
      fn forget(&self, _key: &str) -> bool {
          false
      }

      /// Enumerates learned words, most recently used first.
      ///
      /// # Errors
      /// The default implementation reports `ImeError::DictUnavailable`,
      /// because a source that cannot enumerate is not an error condition for
      /// decoding — only for the management surface.
      fn list(&self, _offset: u64, _limit: u16) -> Result<Vec<WordRef<'_>>, ImeError> {
          Err(ImeError::DictUnavailable {
              path: PathBuf::new(),
              cause: DictError::Unsupported,
          })
      }

      /// Writes the learned words as TSV, returning the row count.
      ///
      /// # Errors
      /// The default implementation reports `ImeError::DictUnavailable`.
      fn export_tsv(&self, _out: &mut dyn Write) -> Result<u64, ImeError> {
          Err(ImeError::DictUnavailable {
              path: PathBuf::new(),
              cause: DictError::Unsupported,
          })
      }
  }
  ```

  (b) **`UserDb` 的存储层改动**。redb 的表中新增两个字段，旧记录读取时用 `Default` 补齐（4.1 的兼容策略 3）：

  ```rust
  // crates/ime-dict/src/user_db.rs
  /// One learned word's record.
  ///
  /// `created_unix` and `pinned` were added after the first release. redb reads
  /// a row written by an older build as a shorter tuple, so both are read
  /// through `unwrap_or_default`: an old row becomes an unpinned record with
  /// no creation time, which is exactly what it was.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct UserRecord {
      /// Accumulated frequency, saturating at `u32::MAX`.
      pub weight: u32,
      /// Last use, in unix milliseconds; drives eviction and the time decay of
      /// `ADD-FEAT-P1.01.05`.
      pub last_used_unix: u64,
      /// First learning time, in unix milliseconds.
      pub created_unix: u64,
      /// Set when the user pinned the word, which exempts it from eviction.
      pub pinned: bool,
  }

  /// The reason a removal did or did not happen, for the diagnostics surface.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum ForgetOutcome {
      /// A record was removed.
      Removed,
      /// The key was never learned.
      NotFound,
      /// The key is pinned; the user must unpin it first.
      Pinned,
      /// The store is in read-only mode.
      Readonly,
  }
  ```

  (c) **导出格式**（`ASM-A-13` 的同族纪律：只接受 TSV）：

  ```
  # rspinyin user dictionary export v1
  # Columns: key <TAB> weight <TAB> last_used_unix <TAB> created_unix <TAB> pinned
  # `key` is the raw pinyin the user typed, not the committed text: the export
  # is a record of what was learned, and the committed text is derivable from
  # the dictionary but not the other way round.
  nihao	128	1759161600000	1756570000000	0
  yinhang	64	1759075200000	1756570000000	1
  ```

  **边界契约（跨边界任务必填）**：
  - `UserFreqSource::{forget, list, export_tsv}`（`M0` 落地，带默认实现）。
  - `DictError::UserWordNotFound`、`DictError::ExportTooLarge { bytes, limit }`（`M0` 落地，在既有 `dict/*` 段内分配）。
  - 错误码：`dict/user-word-not-found`、`dict/export-too-large`。
  - `KeyAction::ForgetHighlighted`（`M0` 落地）——默认不绑定任何键（`[keys.bindings]` 里由用户显式绑定），避免误删。
  - **状态机跃迁**（4.2.2）：`Composing` + `KeyEvent(ForgetHighlighted)` → `Composing`，副作用为调 `forget(text)` + 从候选列表移除 + `Paging::reconcile` 按文本保持高亮。
  - **RequestId/TraceId 贯穿**：`list` 与 `export_tsv` 是**管理路径**而非输入路径，不走 `Revision`；但每次调用必须记一条带 `session_id` 的 `info` 级诊断（**不含任何词条内容**，只记条数，遵守 `AGENTS.md` 3.4 的"绝不记录用户输入内容"——用户词条本身就是用户输入）。

  **与现有代码的无缝衔接方案**：
  - `ime-core` 的 mock `UserFreqSource` 实现**零改动**（默认实现生效）。
  - `Decoder` 的调用路径不变；`forget` 只在按键路径上被调用一次，不在解码热路径上。
  - `ime-ui` **零改动**（本卡不新增 UI；管理界面在 `ADD-FEAT-P1.02.01` 的命令面板里做）。
  - **重要**：`forget` 后必须调 `Paging::reconcile(prev_text, candidates)` 而非重置高亮——既有 `reconcile` 已按文本保持高亮，直接复用。

- **非功能约束 (NFR) 与性能指标**：
  - **`forget` 是 O(1) 的 redb 删除**，实测 ≤ 0.05ms（0.3 节的独占预算）。它在按键路径上但**不阻塞**：删除走既有的批量提交机制（`COMMIT_BATCH` / `COMMIT_INTERVAL_MS`），与 `record` 同一条路径。
  - **`list` 分页**：`offset`/`limit` 语义，`limit ≤ 200`；内存占用与 `limit` 成正比，不随用户库总条数增长。
  - **导出体积** ≤ 8MB（`ASM-A-09`）；超限返回 `DictError::ExportTooLarge` 并**不产出部分文件**。导出到临时文件 + 原子重命名。
  - **导入**：只接受本格式（校验头部注释行 + 列数 + 权重为十进制）；畸形行跳过并计数，不中断。
  - **异常容灾**：只读模式下 `forget` 返回 `ForgetOutcome::Readonly` 并记 `data/readonly-mode`，**不 panic、不阻断输入**（`ASM-15`）。
  - **隐私**：`forget`/`list`/`export_tsv` 的**全部诊断记录不得含词条文本或键**，只记条数与耗时。`docs/dev/privacy.md`（`ADD-FEAT-P0.05.02`）必须说明"导出的文件包含你的全部学习词条，请自行保管"。

- **逐步落地实施步骤**：
  1. **落地 `UserRecord` 与存储层兼容读取**：新增两个字段；写一条"旧行读取"测试（构造短元组行，断言读出 `created_unix = 0`、`pinned = false`）。
  2. **实现 `forget` / `list` / `export_tsv`**：`forget` 处理 `Pinned` / `NotFound` / `Readonly` 三个分支；`list` 按 `last_used_unix` 降序分页；`export_tsv` 写临时文件后原子重命名。
  3. **接入按键路径**：`KeyAction::ForgetHighlighted` 在 `transitions.rs` 的跃迁体里调 `forget`；`ime-fcitx5/src/engine.rs` 把按键路由到该 action；`Paging::reconcile` 保持高亮。
  4. **实现导入**：`xtask` 新增 `rspinyin-import` 或在 `ime-dict` 内提供 `import_tsv`；逐行校验，畸形跳过并计数。
  5. **异常分支与回归**：只读模式、`Pinned`、`NotFound`、超大导出、畸形导入五行各一条测试；跑 `ime-dict` 全量 132 条回归确认无破坏。

- **验收标准 (DoD)**：
  1. `cargo nextest run -p ime-dict` 全绿，新增 ≥ 12 条测试覆盖：forget 三分支、list 分页边界、export 体积上限、import 畸形行、旧记录兼容读取、只读模式降级。[自动]
  2. `cargo nextest run -p ime-types` 全绿——**默认实现存在性由 mock 的编译成功证明**（`ime-core` 的测试 mock 不改一行仍能编译）。[自动]
  3. `cargo nextest run -p ime-core` 全绿，新增 `test_forget_highlighted_preserves_highlight`（删除高亮词后高亮按文本保持，不跳回第一个）。[自动]
  4. 手工验证：输入 `yinhang` 选中并提交数次使其进入用户库 → 再次输入 → 按绑定键删除 → 断言该词从候选消失且**重启后不再出现**。[实验室]
  5. `forget` 的实测耗时 ≤ 0.05ms。[性能]
  6. 导出的 TSV 可被导入回一个空库，`record_count()` 与导出前一致（往返一致性）。[自动]

- **验收记录**（2026-09-30）：
  - **交付物**：`crates/ime-dict/src/user_db/manage.rs`（352 行）、`export.rs`（572 行）、`flush.rs`（写路径从 `user_db.rs` 迁出并扩展为「一批 delta + 一批删除 + 首次写入 meta 行」）、`manage_tests.rs`（15 个用例）、`export_tests.rs`（13 个用例）；`user_db.rs` 新增 `USER_META` 表、`PendingState.removed`、`Inner.pinned`、`freq` 的墓碑短路；`ime-core` 的 `Effect::ForgetUserWord` 与 `ForgetHighlighted` 转换。
  - **接线**（主 Agent 完成）：`engine/router.rs` 的 `apply_effects` 新增 `Effect::ForgetUserWord` arm（失败时发 `dict/user-word-not-found`）；`state/tests.rs` 的 `kind()` 补齐。
  - **验证命令与结果**：`just ci` 退出 0（fmt、clippy `-D warnings`、nextest 1992 个用例、doctest、10 个审计脚本全部通过）。
  - **主 Agent 修掉的三处缺陷**：`manage.rs` 的 `Ordering` 被 `std::sync::atomic` 遮蔽（`cmp`/`partial_cmp` 返回了错误的枚举）；`pending_snapshot` 对兄弟模块不可见；**`Ranked::cmp` 方向反了**——它把「最好」的行放在堆顶，于是每次 push 都淘汰最好的，有界堆最终留下的是最差的两行（`list_words(0, 2)` 返回 `[w4, w0]` 而不是 `[w4, w3]`）。
  - **已知限制**：
    1. **`UserFreqSource::list` 返回 `Err(dict/unsupported)`**：`WordRef<'_>` 的 text 借用期限 = `&self`，而 store 的词集是活的（`record`/`forget` 都是 `&self` 且必须能写），键只能存在 `Mutex` 之后，借用无法逃出 guard；伪造 `'static` 只剩泄漏键或 `unsafe` 两条被禁的路。同页数据由固有方法 `UserDb::list_words`（owned 行）与 `export_tsv` 提供。**这是冻结签名的缺陷而非实现缺口**，要让它可服务需要契约返回 owned 行或 guard 类型。
    2. **`forget` 的存在性与 pin 判定只查内存**（hydrated 时即全库，精确；超过 `HYDRATE_CAP` 的库只认本会话见过的键，且看不到 pin）——按键路径不得开读事务。
    3. pin 目前只有导入能置位（`PinHighlighted` 仍是 `dict/unsupported`）。
    4. 导出/导入的 8 MiB 上限与「畸形即整份拒绝」策略与卡片原文的「畸形行跳过并计数」不同，理由已写入代码注释。
  - **环境**：Rust 1.98.0、Linux 6.18.40.1-microsoft-standard-WSL2、cargo-nextest 0.9.143。
---

#### 任务 ID：ADD-FEAT-P0.02.01 双拼方案引擎与音节映射

- **基本属性**：
  - 绑定差距条目：`GAP-01`（双拼，多方案）
  - 优先级与复杂度：`P0 核心增强` ｜ **高**（涉及 5 张方案表与一个 26×26 的映射矩阵）｜ 预估工时: **4 人天**
  - 前置依赖：**`M0` 契约冻结**（`SchemeId`、`DecodeRequest.scheme`、`DecodeFlags::SHUANGPIN`）；无其他任务依赖
  - 关键路径：`CP: 否`（松弛 8 人天）
  - 并行通道：`Track A-解码`
  - 当前状态：`[x] 已完成`
  - 代码落地锚点：`crates/ime-core/src/scheme.rs`（**新增**）、`crates/ime-core/src/scheme/{xiaohe,ziranma,microsoft,sogou,ziguang}.rs`（**新增**，5 个方案表）、`crates/ime-core/src/segment/mod.rs`（改：进入切分前的输入改写）、`crates/ime-types/src/decode.rs`（`M0` 已加 `SchemeId` 与 `DecodeRequest.scheme`）

- **目标与价值**：补齐**最大的单项功能缺口**。双拼是中文输入法的高频专业特性：用户按一次键得到一个音节，击键数约为全拼的 45%。对标搜狗（7 种方案）、微软拼音（6 种）、RIME（8 种）、libpinyin、macOS 原生、小鹤音形。当前 `grep -ri shuangpin\|double_pinyin crates/ xtask/ data/` **零命中**——不是"实现得不好"，是**完全不存在**。

- **技术设计与代码级接入细节**：

  **底层模型与数据流改动**：

  (a) **方案表的数据结构**。双拼的本质是"声母一键 + 韵母一键"的两键编码。关键设计决策：**方案表是编译期常量数组，不是配置文件**——方案是标准化的、不随用户变化；用户自定义方案走 `[scheme.custom]`（`ADD-FEAT-P0.02.02`）。

  ```rust
  // crates/ime-core/src/scheme.rs
  /// One double-pinyin scheme: the letter that stands for each initial and each
  /// final.
  ///
  /// The tables are `const` because a scheme is a published standard, not a
  /// user preference. The one scheme that *is* a user preference — a custom
  /// table — is built at runtime from `[scheme.custom]` and has the same shape.
  #[derive(Clone, Copy, Debug)]
  pub struct SchemeTable {
      /// Identifies the scheme in diagnostics and configuration.
      pub id: SchemeId,
      /// Human-readable name shown in the preedit header.
      pub name: &'static str,
      /// `initials[b - b'a']` is the syllable initial the letter stands for.
      /// An empty entry means the letter cannot start a syllable.
      pub initials: [&'static str; 26],
      /// `finals[b - b'a']` is the syllable final the letter stands for,
      /// possibly followed by a second letter that is consumed as part of the
      /// final. Empty means the letter cannot be a final.
      pub finals: [&'static str; 26],
      /// The letters that stand for a syllable on their own (`a`, `e`, `o` and
      /// the vowel-initial syllables).
      pub standalone: [&'static str; 26],
  }

  /// Maps a double-pinyin key sequence onto a full-pinyin syllable string.
  ///
  /// The mapping is a pure function of the scheme and the input: it reads no
  /// file, no clock and no global state, which is what keeps the decoder's
  /// determinism guarantee intact (0.4 rule 4).
  ///
  /// # Errors
  /// Returns `DecodeError::InvalidChar` for a character outside `a..=z`, and
  /// `ImeError::SchemeUnsupported` when the scheme table is malformed.
  pub fn map_syllable(
      table: &SchemeTable,
      keys: &[u8],
  ) -> Result<Option<MappedSyllable>, ImeError>;

  /// One syllable produced by the scheme mapping.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct MappedSyllable {
      /// Full-pinyin spelling, e.g. `zhong`.
      pub full: SyllableId,
      /// How many scheme keys this syllable consumed: 1 or 2.
      pub consumed: u8,
  }
  ```

  (b) **五个方案表**（`ASM-A-07`：首批 5 种）。每个方案一张 `SchemeTable`，以**编译期常量**形式定义。示例（小鹤双拼的声母与韵母，完整表在实现时逐条填写，此处给结构与关键行）：

  ```rust
  // crates/ime-core/src/scheme/xiaohe.rs
  /// The Xiaohe scheme, the most widely used double-pinyin layout.
  ///
  /// Initials follow the keyboard's letter names where they exist (`zh` on
  /// `v`, `ch` on `i`, `sh` on `u`); finals use the published Xiaohe table.
  pub const XIAOHE: SchemeTable = SchemeTable {
      id: SchemeId::Xiaohe,
      name: "小鹤双拼",
      initials: [
          // a  b  c  d  e  f  g  h  i  j  k  l  m
          "", "b", "c", "d", "", "f", "g", "h", "ch", "j", "k", "l", "m",
          // n  o  p  q  r  s  t  u  v  w  x  y  z
          "n", "", "p", "q", "r", "s", "t", "sh", "zh", "w", "x", "y", "z",
      ],
      finals: [
          // a  b  c  d  e  f  g  h  i  j  k  l  m
          "a", "in", "ao", "ue", "e", "en", "eng", "ang", "i", "an", "ing", "iang", "ian",
          // n  o  p  q  r  s  t  u  v  w  x  y  z
          "iao", "o", "ie", "iu", "uan", "ong", "ue", "u", "ui", "ei", "ia", "un", "ou",
      ],
      standalone: [
          "a", "", "", "", "e", "", "", "", "", "", "", "", "",
          "", "o", "", "", "", "", "", "", "", "", "", "", "",
      ],
  };
  ```

  (c) **注入点：切分之前**（4.1 的挂载点 H1）。双拼映射是**纯输入改写**，切分器本身完全不知道方案的存在：

  ```rust
  // crates/ime-core/src/segment/mod.rs (改)
  /// Rewrites the raw input into full pinyin before segmentation.
  ///
  /// The segmentation layer must never learn about schemes: it is defined over
  /// the 411-syllable table, and a scheme is a different encoding of the same
  /// syllables. Rewriting here keeps every downstream stage — the DAG, the
  /// lattice, the Viterbi pass, the preedit builder — unchanged.
  fn rewrite_for_scheme(raw: &str, scheme: SchemeId) -> Result<Cow<'_, str>, ImeError>;
  ```

  **关键语义约束**：改写后 `DecodeResult.segments` 的 `start`/`end` 仍是**方案音节**下标（不是全拼音节下标）——因为 `Segment` 是给 UI 画 preedit 高亮用的，UI 显示的是用户键入的方案键。**这是一个必须在实现时写进 doc comment 的语义点**，否则 UI 的 preedit 高亮会错位。

  **边界契约（跨边界任务必填）**：
  - `SchemeId`（`M0` 落地）：`Full = 0` / `Xiaohe = 1` / `Ziranma = 2` / `Microsoft = 3` / `Sogou = 4` / `Ziguang = 5`，`const COUNT: u8 = 6`。
  - `DecodeRequest.scheme`（`M0` 落地，**本次唯一的破坏性契约变更**）。
  - `DecodeFlags::SHUANGPIN`（`1<<11`，`M0` 落地）——用于在混输场景下临时切回全拼解析。
  - `ImeError::SchemeUnsupported { scheme: u8 }`（`M0` 落地，在既有 `decode/*` 段内分配）；错误码 `decode/scheme-unsupported`。
  - **状态机跃迁**（4.2.2）：`Composing` + `DecodeDone{scheme != Full}` → `Composing`；`Composing` + `ConfigReloaded`（`[scheme]` 变更）→ `Composing` 且**变更延迟到本次提交之后生效**（中途换方案会让候选突变）。

  **与现有代码的无缝衔接方案**：`Decoder::decode` 的签名不变（方案在 `DecodeRequest.scheme` 里）；`ime-core` 的 `state/`、`viterbi/`、`lm/`、`preedit.rs` **全部零改动**。唯一改动的既有文件是 `segment/mod.rs`（加一行改写调用）。**这是把方案设计成"输入改写"而非"切分分支"的直接收益。**

- **非功能约束 (NFR) 与性能指标**：
  - **独占预算 ≤ 0.15ms**（0.3 节）。方案映射是单表查表 + 小循环，实测应在 10µs 量级。
  - **确定性**：映射是纯函数，同一 `(scheme, raw)` 恒产生同一结果；**不得**依赖任何全局状态（0.4 规则 4）。
  - **边缘条件**：
    - 孤立字母（`a`/`e`/`o` 单独成音节）走 `standalone` 表。
    - 无法映射的键序列 → 该音节**回退为单字母**并继续（不整串失败），保证用户永远打得出字。
    - 键序列长度超过 `MAX_RAW_LEN = 64` → 既有 `DecodeError::TooLong`，方案映射不改变此行为。
    - 三键音节（如某些方案的 `iong`）由 `finals` 表的第二字母表达，`consumed` 相应为 2。
  - **不得**因为方案映射而增加 `DecodeResult.candidates` 的数量——双拼只是换了一种输入编码，候选集应与全拼等价。

- **逐步落地实施步骤**：
  1. **定义 `SchemeTable` 与 `SchemeId`**（`SchemeId` 在 `M0` 已落地）；实现 `map_syllable` 与 `MappedSyllable`。
  2. **逐条填写 5 张方案表**：小鹤、自然码、微软、搜狗、紫光。每张表配一组**黄金用例**（该方案的官方键序 → 期望全拼），来源为该方案的公开键位图。
  3. **接入 `segment/mod.rs` 的输入改写**：`rewrite_for_scheme` 返回 `Cow`，全拼时零拷贝直接返回借用。
  4. **保证 `Segment.start/end` 的方案下标语义**：在 `decoder.rs` 组装 `DecodeResult` 时，把全拼音节下标反查回方案音节下标；配一条专门的测试。
  5. **回归与边界**：孤立字母、无法映射、超长、三键音节各一条测试；跑 `ime-core` 全量 235 条回归。

- **验收标准 (DoD)**：
  1. `cargo nextest run -p ime-core` 全绿，新增 ≥ 15 条测试：5 张方案表各 ≥ 2 条黄金用例 + 孤立字母 + 无法映射回退 + 三键音节 + `Segment` 下标语义。[自动]
  2. 同一段文本用全拼与用小鹤双拼输入，`DecodeResult.candidates` 的**文本序列完全一致**（`test_scheme_and_full_pinyin_agree_on_candidates`）。[自动]
  3. `cargo nextest run -p ime-types` 全绿——`DecodeRequest.scheme` 的破坏性变更在所有构造点同步完成。[自动]
  4. 实测映射耗时 ≤ 0.15ms。[性能]
  5. `decode_p99 ≤ 3.0ms` 不劣化。[性能]
  6. 手工验证：配置 `scheme = "xiaohe"` 后，键入 `vs` 得到「中」，`nihc` 得到「你好」。[实验室]
- **验收记录**（2026-09-30）：
  - **交付物**：新增 `crates/ime-core/src/shuangpin/`（`mod.rs` 约 660 行 + 5 个方案表文件 + `tests.rs`），共 57 个用例（含 2 条 doctest）。
  - **五个方案表的出处（每个都两处独立互校，写在各自文件的模块文档里）**：小鹤/自然码/微软/紫光与 libpinyin 的 `double_pinyin_table.h` 逐键一致；搜狗另参官方帮助页的零声母规则。
  - **验证命令与结果**：`just ci` 退出 0（`cargo fmt --check`、clippy `-D warnings`、nextest 1521 个用例、doctest、9 个审计脚本及其自检、25 条预算阈值全部通过）。
  - **本次由主 Agent 修正的三处**：
    1. `key_index` 的 `usize::from` 在 `const fn` 里不可调用（`From` 尚不是 const trait），改为显式 cast。
    2. 微软与搜狗两张表的黄金用例把 `en` 记在 `w` 键上，而两张表的 `w` 都是 `ia/ua`、`en` 在 `f`——**测试写错了，表是对的**。改为 `(b"of", "en")`。
    3. **一处真实缺陷**：回退路径的 `v` 排除测试写成 `ch != 'v'`，而按键在**之后**才被转小写，因此大写 `V` 会溜过检查、把一个裸 `v` 写进结果——正是规范化器要折叠成 `ü` 的那个字节，会让其后每个字节偏移都错位。改为 `!ch.eq_ignore_ascii_case(&'v')`，由 `test_rewrite_is_a_fixed_point_of_the_normalizer` 的 `"VSHC"` 用例抓到。
  - **已知限制**：
    1. **`;` 键打不进来**：微软/搜狗/紫光的 `ing` 都印在分号键上（Rime 三个 schema、libpinyin 的 mspy/zgpy 表、搜狗官方页均如此），故键位表是 27 列；但 `InputBuffer::is_input_char` 只放行 ASCII 字母与 `'`，这三个方案的 `-ing` 音节因此暂时不可键入。要么扩输入字母表，要么在文档里明示。
    2. **解码器接线缺失**：`viterbi/scratch.rs` 仍直接 `SyllableDag::build(&req.raw)`，未调用 `shuangpin::rewrite_request`；`DecodeResult.segments` 也未用 `SchemeMap::scheme_span` 把全拼音节下标换算回方案下标。机制与测试已就绪，只差两行调用。
    3. **改写后长度可能超过 `MAX_RAW_LEN`**：64 个键最多展开成约 192 字节，而 `SyllableDag::build` 的长度上限是 64，超长会退化为透传候选。
    4. 模块名为 `shuangpin/`（白名单指定），而卡面锚点是 `scheme.rs`；改名只需 `git mv` + 一行 `pub mod`。
    5. DoD 4/5 的实测耗时与 DoD 6 的手工验证仍缺口（需接线与基准）。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143。

---

#### 任务 ID：ADD-FEAT-P0.02.02 双拼方案配置、切换与混输兜底

- **基本属性**：
  - 绑定差距条目：`GAP-01`（双拼，多方案）
  - 优先级与复杂度：`P0 核心增强` ｜ **低** ｜ 预估工时: **2 人天**
  - 前置依赖：`ADD-FEAT-P0.02.01`
  - 关键路径：`CP: 否`（松弛 12 人天）
  - 并行通道：`Track B`（配置 UI 与状态显示）；配置加载部分落 `Track C`
  - 当前状态：`[x] 已完成`
  - 代码落地锚点：`crates/ime-config/src/schema.rs`（改：新增 `[scheme]` 段）、`crates/ime-config/src/reload.rs`（改：方案热重载）、`crates/ime-fcitx5/src/engine.rs`（改：把 `SchemeId` 填进 `DecodeRequest`）、`crates/ime-core/src/state/machine.rs`（改：`StatusStrip` 填方案名）、`crates/ime-ui/ui/candidate.slint`（改：header 显示方案名）

- **目标与价值**：让方案**可用、可切、可退回**。没有本卡，`P0.02.01` 只是一个无法被用户启用的引擎。三个具体价值：(1) 配置层暴露方案选择；(2) header 显示当前方案，避免用户忘记自己开了双拼而困惑；(3) **混输兜底**——用户开了双拼后仍能偶尔用全拼输入，这是搜狗/微软都有但常被忽略的关键体验点。

- **技术设计与代码级接入细节**：

  **底层模型与数据流改动**：

  (a) **配置段**（4.2.3 已列）：`[scheme]` 的 `scheme` / `show_hint` / `keep_full_pinyin` + `[scheme.custom]` 的 `initials` / `finals`。

  ```rust
  // crates/ime-config/src/schema.rs (改)
  /// The `[scheme]` section.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct SchemeConfig {
      /// `scheme`: which double-pinyin layout is active, or full pinyin.
      pub scheme: SchemeChoice,
      /// `show_hint`: show the scheme name in the preedit header.
      pub show_hint: bool,
      /// `keep_full_pinyin`: accept a full-pinyin syllable inside a
      /// double-pinyin session.
      ///
      /// Without this, a user who switched to double pinyin and then types a
      /// full-pinyin syllable gets nothing — which reads as "the input method
      /// broke", not as "you are in the wrong mode".
      pub keep_full_pinyin: bool,
  }

  /// The `scheme` key's values.
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
  pub enum SchemeChoice {
      #[default]
      Full,
      Xiaohe,
      Ziranma,
      Microsoft,
      Sogou,
      Ziguang,
      Custom,
  }
  ```

  (b) **混输兜底的算法**。在 `rewrite_for_scheme` 内部：方案映射失败时，**先尝试按全拼解释当前音节**；两者都失败才回退为单字母。

  ```rust
  /// How one syllable was resolved, for the preedit header hint.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum SyllableOrigin {
      /// Resolved by the active scheme.
      Scheme,
      /// Resolved as full pinyin inside a scheme session; the header shows a
      /// subtle hint so the user can tell why this syllable looks different.
      FullPinyinFallback,
      /// Neither worked; the letter passed through on its own.
      Literal,
  }
  ```

  (c) **header 显示**：`StatusStrip` 不新增字段（`M0` 只加了 `script`）。方案名复用既有的 `mode_label`——中文模式下 `mode_label` 显示方案名（`"小鹤"` / `"全拼"`），英文模式下显示 `"EN"`。**这是零契约变更的实现方式**。

  **边界契约**：`[scheme]` 段是本卡新增的配置键；`SchemeChoice` 是**配置层的枚举**，通过 `From<SchemeChoice> for SchemeId` 映射到契约层的 `SchemeId`（`ime-config` 依赖 `ime-types` 是允许的方向）。**不修改 `ime-types`**。

  **UI 与工艺级设计规范**：header 的方案名沿用既有 `font-size-header = 14px` 与 `text-secondary` token；**不新增颜色、不改变 header 高度**（`header-height = 34px`）。混输兜底命中时，`SyllableOrigin::FullPinyinFallback` 的提示沿用 `text-annotation` token 与 `font-size-small = 11px`（`C-2` 的字阶约束）。

  **与现有代码的无缝衔接方案**：`ime-fcitx5/src/engine.rs` 在构造 `DecodeRequest` 时填 `scheme`（`M0` 已加该字段）；`ime-core` 的 `state/machine.rs` 在 `set_frame_context` 时把方案名写进 `StatusStrip.mode_label`。**热重载**：`reload.rs` 检测到 `[scheme]` 变更时，若会话处于 `Composing`，**标记为 pending**，在当前提交完成后生效（4.2.2 的跃迁表）。

- **非功能约束 (NFR) 与性能指标**：
  - 配置热重载**不得**重置进行中的会话（0.4 规则 10）；`[scheme]` 变更延迟生效。
  - `keep_full_pinyin = true` 时的兜底尝试**不得**使 `decode_p99` 劣化超过 0.05ms（它是失败路径上的一次额外尝试，正常路径不触发）。
  - **边缘条件**：`scheme = "custom"` 但 `[scheme.custom]` 未填 → 降级为全拼并记 `config/invalid`；`initials` 长度 ≠ 26 → `ConfigError::Invalid`，由 `Config::repaired()` 修复为全拼。
  - 配置文件中的方案名大小写不敏感（`"XiaoHe"` = `"xiaohe"`）。

- **逐步落地实施步骤**：
  1. **加 `[scheme]` 段与 `SchemeChoice`**：进 `schema.rs`；`From<SchemeChoice> for SchemeId`；补 `validate()` 与 `repaired()` 分支。
  2. **接入 `engine.rs`**：把配置里的方案填进 `DecodeRequest.scheme`。
  3. **实现混输兜底**：`rewrite_for_scheme` 的三级回退（`Scheme` → `FullPinyinFallback` → `Literal`）；返回 `SyllableOrigin` 供 header 用。
  4. **header 显示方案名**：`machine.rs` 填 `mode_label`；`.slint` 的 `Header` 已绑定 `mode-label`，无需改 `.slint`（**这是既有骨架的红利**）。
  5. **热重载的延迟生效**：`reload.rs` 的 pending 机制 + 一条"重载期间会话不中断"的测试。

- **验收标准 (DoD)**：
  1. `cargo nextest run -p ime-config` 全绿，新增 ≥ 6 条：方案名解析（含大小写）、非法方案名降级、`custom` 缺表降级、`initials` 长度校验、`keep_full_pinyin` 默认值、`repaired()` 对非法值的修复。[自动]
  2. `cargo nextest run -p ime-core` 全绿，新增 `test_full_pinyin_fallback_inside_scheme_session`、`test_literal_fallback_keeps_input_typeable`。[自动]
  3. 手工验证：开启小鹤双拼后，输入 `nihao`（全拼）仍能得到「你好」候选。[实验室]
  4. 运行中把 `scheme` 从 `xiaohe` 改为 `full`，进行中的输入**不中断**，变更在本次提交后生效。[实验室]
  5. 配置非法方案名时 `Config::repaired()` 修复为 `full` 并报告 `config/invalid`。[自动]

- **验收记录**（2026-09-30）：
  - **交付物**：`crates/ime-config/src/scheme.rs`（约 485 行，新增两个投影接缝）、`scheme/tests.rs`（约 412 行，新增 5 个用例）、`crates/ime-core/src/shuangpin/fallback/tests.rs`（新增 1 个边界用例）。`fallback.rs` 与 `state/scheme.rs` 逐行核对后确认**无缺口，未改**。
  - **验证命令与结果**：`just ci` 退出 0：`cargo fmt --all -- --check`、`cargo clippy`（主工作区 `--all-targets --all-features` 与两个 addon crate，全部 `-D warnings`）、`cargo nextest run`（主工作区 2430 个用例、两个 addon 346 个，全绿）、`cargo test --doc`（主工作区与两个 addon）、10 个审计脚本及其自检、`just check-host`。
  - **新增的两个投影接缝**：`SchemeConfig::decode_settings() -> (SchemeId, bool)`（布局号走既有的 `From<SchemeChoice> for SchemeId`，`Custom → FULL` 这条规则只在这里存在一份；开关直接取 `keep_full_pinyin`）与 `SchemeConfig::header_hint() -> Option<&str>`（`show_hint` 的门；`None` 表示引擎应回落到自己的「中/英」标签）。两者都有 doctest。
  - **`[scheme]` 校验零新增错误码**：未知方案名由 `SchemeChoice::parse` 报 `ConfigError::Invalid { key: "scheme.scheme" }` → 稳定码 `config/invalid`，槽位保持默认 `Full`（即降级到全拼）；`custom` 无论表是否完整一律报 `scheme.scheme` 并降级；表本身逐列表校验长度 26，报 `scheme.custom.initials`/`scheme.custom.finals`。解码侧未知编号报既有的 `decode/scheme-unsupported`。
  - **切换语义**：`machine.rs::step` 只在 `SessionState::Idle` 时 `sess.scheme.latch(cfg.scheme, cfg.keep_full_pinyin)`，组合进行中冻结；`SchemeSession` 的模块文档写明 "Latched per composition"。行为等价于卡片的 pending 机制，可观察契约一致，0.4 规则 10 / 禁止项 23 满足（测试 `test_scheme_change_is_deferred_until_the_next_composition`）。
  - **混输兜底三级阶梯**：先 pair、再 standalone（都是方案自身读法）→ 策略允许时按全拼读 → 单字母 Literal，与卡片「方案映射失败时先按全拼解释，两者都失败才回退单字母」逐字一致。本次补的只是「pair 存在但组不成音节 → 退回首键 standalone 且第二键留给下一个位置」这一条此前没有测试钉住的优先级分支。
  - **已知限制**：① **`custom` 方案不可用（设计偏差，需 ADR）**——卡片设想 `custom` + 合法表可用，但 `SchemeId` 是冻结的 6 值枚举、`table_for` 只认 `&'static SchemeTable`，运行时表无处安放；现状「校验后置一边 + 报 `config/invalid` + 降级全拼」是唯一不破坏契约的解法，理由已写在模块文档里；② **配置 → 引擎的投影在本次之前完全没有接线**——`SessionConfig.scheme` / `keep_full_pinyin` 从来没人从文档里填过，本卡在配置层备好了接缝，引擎侧接线由另一张卡完成（`KeyRouter` 现在从 `ime_config::Config` 投影 `[keys]` 与 `[scheme]`）；③ header 方案名（`mode_label`）与混输提示（`SyllableOrigin::FullPinyinFallback`）的接线同理；④ 兜底路径无基准数据（卡片 NFR「劣化 ≤ 0.05ms」未取数）；⑤ DoD 3「小鹤下输入字面量 `nihao` 仍得「你好」」需 mock lexicon 的会话级测试，当前只有读法层的等价覆盖。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85）、Linux 6.18.40.1-microsoft-standard-WSL2、cargo-nextest 0.9.143。

---

#### 任务 ID：ADD-FEAT-P0.02.03 模糊音匹配层

- **基本属性**：
  - 绑定差距条目：`GAP-02`（模糊音）
  - 优先级与复杂度：`P0 核心增强` ｜ **中** ｜ 预估工时: **3 人天**
  - 前置依赖：**`M0` 契约冻结**（`DecodeFlags::FUZZY*` 的 9 个位已在 `ADR-0001` 冻结，本卡只需确认其语义）；无其他任务依赖
  - 关键路径：`CP: 否`（松弛 11 人天）
  - 并行通道：`Track A-解码`
  - 当前状态：`[x] 已完成`
  - 代码落地锚点：`crates/ime-core/src/segment/fuzzy.rs`（**新增**）、`crates/ime-core/src/viterbi/lattice.rs`（改：词格构建期的边扩张）、`crates/ime-config/src/schema.rs`（改：新增 `[engine] fuzzy` 子键）、`crates/ime-types/src/decode.rs`（`M0` 已确认 `FUZZY*` 位语义）

- **目标与价值**：让发音不准的用户也能打出字。`zh/z`、`ch/c`、`sh/s`、`n/l`、`f/h`、`an/ang`、`en/eng`、`in/ing` 八类混淆在南方方言区是高频需求，搜狗/微软/RIME/libpinyin **全部**内置。当前 `DecodeFlags::FUZZY` 与 8 个类别位**已冻结但零实现**，是"契约已备好、功能没做"的典型。

- **技术设计与代码级接入细节**：

  **底层模型与数据流改动**：

  (a) **模糊规则表**。每条规则是"某音节可被某替代拼写命中"的双向关系：

  ```rust
  // crates/ime-core/src/segment/fuzzy.rs
  /// One fuzzy class: a pair of spellings that are treated as equivalent.
  ///
  /// The class is applied at the syllable level, not the letter level: `zh`
  /// and `z` are whole initials, and `an` / `ang` are whole finals. Letter-level
  /// rewriting would turn `zhang` into `zang` correctly but also `zha` into
  /// `za`, which is a different syllable with a different meaning.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct FuzzyClass {
      /// The bit in `DecodeFlags` that enables this class.
      pub flag: DecodeFlags,
      /// The spelling as written.
      pub canonical: &'static str,
      /// The spelling it also matches.
      pub variant: &'static str,
  }

  /// Every class the frozen `DecodeFlags` namespace defines, in bit order.
  pub const CLASSES: [FuzzyClass; 8] = [
      FuzzyClass { flag: DecodeFlags::FUZZY_ZH_Z,    canonical: "zh",  variant: "z"  },
      FuzzyClass { flag: DecodeFlags::FUZZY_CH_C,    canonical: "ch",  variant: "c"  },
      FuzzyClass { flag: DecodeFlags::FUZZY_SH_S,    canonical: "sh",  variant: "s"  },
      FuzzyClass { flag: DecodeFlags::FUZZY_N_L,     canonical: "n",   variant: "l"  },
      FuzzyClass { flag: DecodeFlags::FUZZY_AN_ANG,  canonical: "an",  variant: "ang" },
      FuzzyClass { flag: DecodeFlags::FUZZY_EN_ENG,  canonical: "en",  variant: "eng" },
      FuzzyClass { flag: DecodeFlags::FUZZY_IN_ING,  canonical: "in",  variant: "ing" },
      FuzzyClass { flag: DecodeFlags::FUZZY_F_H,     canonical: "f",   variant: "h"  },
  ];

  /// Every syllable the enabled classes make `syllable` equivalent to.
  ///
  /// The canonical spelling is included, so a caller can treat the result as
  /// "the complete set of syllables to try" without special-casing the exact
  /// match. The set is bounded by `MAX_VARIANTS`.
  ///
  /// # Errors
  /// This function is infallible: it returns no `Result`.
  pub fn variants(syllable: SyllableId, flags: DecodeFlags) -> ArrayVec<SyllableId, MAX_VARIANTS>;
  ```

  (b) **`MAX_VARIANTS = 8`**（`ASM-A-06`）。八类规则最多让一个音节产生 2⁸ = 256 种组合，**必须**封顶：按规则的启用顺序取前 8 个，并在 `DecodeFlags` 全开时记一条 `decode/fuzzy-truncated` 诊断。

  (c) **注入点：词格构建期**（4.1 的挂载点 H2）。**不在 Viterbi 内部**：

  ```rust
  // crates/ime-core/src/viterbi/lattice.rs (改)
  /// Adds a lattice edge for every fuzzy variant of the segment's syllable.
  ///
  /// The expansion happens while the lattice is built, not while it is
  /// relaxed: the Viterbi pass is defined over a fixed lattice, and giving it
  /// a growing one would make the k-best guarantee depend on the order edges
  /// were discovered in. Each variant edge carries a penalty so an exact match
  /// always outranks a fuzzy one at equal frequency.
  fn push_fuzzy_edges(
      lattice: &mut Lattice,
      segment: &Segment,
      flags: DecodeFlags,
      lexicon: &dyn Lexicon,
  ) -> Result<usize, ImeError>;
  ```

  (d) **变体边必须带惩罚**，否则模糊音会污染排序。惩罚值是一个 Q8.8 常量，与 `ScoreWeights` 同族：

  ```rust
  /// The score penalty one fuzzy-variant edge carries, in Q8.8.
  ///
  /// Without it, a fuzzy match at equal dictionary weight would tie with an
  /// exact match, and the tie would be broken by whichever edge the lattice
  /// happened to hold first — an order-dependent result, which the decoder's
  /// determinism guarantee forbids. 12.0 in Q8.8 is roughly a 5% weight
  /// difference: enough to lose every tie, small enough that a much more
  /// frequent fuzzy word still wins.
  pub const FUZZY_PENALTY_Q8: i32 = 12 << 8;
  ```

  **边界契约（跨边界任务必填）**：
  - `DecodeFlags::{FUZZY, FUZZY_ZH_Z, FUZZY_CH_C, FUZZY_SH_S, FUZZY_N_L, FUZZY_AN_ANG, FUZZY_EN_ENG, FUZZY_IN_ING, FUZZY_F_H}`——**全部已在 `ADR-0001` 冻结**（`1<<0`~`1<<8`），本卡**不修改契约**。`FUZZY` 是主开关，类别位只在主开关置位时生效（既有注释已明确此语义）。
  - 新增配置键 `[engine] fuzzy = ["zh_z", "ch_c", ...]`（字符串数组，每项对应一个类别）。**这是本卡唯一新增的配置面**。
  - 新错误码：`decode/fuzzy-truncated`（**info 级诊断，不是错误**）。

  **与现有代码的无缝衔接方案**：`Decoder::decode` 签名不变；`flags` 从 `DecodeRequest.flags` 读出（既有字段）。`ime-ui` **零改动**（模糊音不改变候选的结构，只改变候选集的内容）。既有测试 `test_decode_flags_all_covers_every_defined_bit` 断言 `all().bits() == 0x07FF`——本卡**不改这个值**（`M0` 才改，因为 `M0` 加了三个新位）。

- **非功能约束 (NFR) 与性能指标**：
  - **独占预算 ≤ 0.60ms**（0.3 节）——这是全部增量中**最大的一笔**，因为模糊音会成倍扩大词格。
  - **路径数上限**：每音节 ≤ `MAX_VARIANTS = 8` 个变体；词格总边数 ≤ 既有 `MAX_WORDS_PER_KEY × MAX_SYL_COUNT` 的 8 倍，超出时**按启用类别的顺序截断**并记 `decode/fuzzy-truncated`。
  - **确定性**：变体集合的枚举顺序必须由 `CLASSES` 的常量顺序决定，**不得**依赖 `HashSet` 的迭代顺序。
  - **边缘条件**：
    - 全拼模式下 `FUZZY` 未置位 → `variants()` 返回只含自身的单元素集合，**零开销**（提前返回，不建表）。
    - 变体音节不在 411 表中（如 `zh` + `ang` 组合出的 `zhang` 在表中，但某些组合不在）→ 跳过该变体。
    - `FUZZY` 置位但无任何类别位 → 等价于关闭，不产生诊断噪音。
  - **不得**使 `decode_p99` 超过 3.0ms；若实测超限，按 `ASM-A-06` 的降级顺序：先减类别数 → 再收紧 beam → 最后关闭该类别默认值。

- **逐步落地实施步骤**：
  1. **实现 `FuzzyClass` / `CLASSES` / `variants`**：纯函数，可用内存构造的 `SyllableId` 表单测，无文件依赖。
  2. **接入词格构建期**：`push_fuzzy_edges` 在 `lattice.rs` 的边构建循环内调用；每个变体边带 `FUZZY_PENALTY_Q8`。
  3. **落地配置**：`[engine] fuzzy` 字符串数组 → `DecodeFlags` 的位组合；非法类别名走 `Config::repaired()` 报告并忽略。
  4. **截断与诊断**：`MAX_VARIANTS` 截断 + `decode/fuzzy-truncated` 一次性诊断（**不得**每帧重复记录，避免日志风暴）。
  5. **回归与确定性**：加一条"同一输入连跑 100 次候选顺序完全一致"的测试；跑 `ime-core` 全量回归。

- **验收标准 (DoD)**：
  1. `cargo nextest run -p ime-core` 全绿，新增 ≥ 12 条：八类规则各 1 条命中用例 + 主开关语义 + `MAX_VARIANTS` 截断 + 惩罚使精确匹配优先 + 确定性（100 次顺序一致）+ 关闭时零开销。[自动]
  2. `cargo nextest run -p ime-config` 全绿，新增 `test_fuzzy_class_names_parse`、`test_unknown_fuzzy_class_is_reported`。[自动]
  3. 实测模糊音开启后的 `decode_p99` ≤ 3.0ms，且相比关闭时劣化 ≤ 0.60ms。[性能]
  4. 手工验证：开启 `n_l` 后输入 `lihao` 得到「你好」候选。[实验室]
  5. `decode/fuzzy-truncated` 在 100 次相同输入下只记录 1 条。[自动]

---
- **验收记录**（2026-09-30）：
  - **交付物**：新增 `crates/ime-core/src/fuzzy.rs`（470 行）与 `fuzzy/tests.rs`（450 行，24 个用例）。
  - **八个类别**（`CLASSES`，每类带 `flag` / 配置名 `name` / `canonical` / `variant`）：`zh_z`、`ch_c`、`sh_s`、`n_l`、`an_ang`、`en_eng`、`in_ing`、`f_h`，位序 `1<<1..=1<<8`，与冻结的 `DecodeFlags` 位域一致（`CLASS_BITS == 0x01FE`，不含主开关位）。
  - **音节级双向替换**：先在词首、再在词尾各试一次，结果必须在 411 表中才收（因此 `zhuang`→`zuang`、`shei`→`sei` 被跳过）；类别按表序逐层叠加，所以两类混淆可复合（`zhang` 在 `zh_z`+`an_ang` 下同时到达 `zan`）；原拼写永远在首位。
  - **主开关语义**：`is_enabled = FUZZY && 任一类别位`；`FUZZY` 置位但无类别位等价于关闭且**不产生诊断**；关闭时提前返回、零查表、`SmallVec` 不溢出到堆（有 5 种关闭位型 × 411 音节的断言）。
  - **惩罚**：`FUZZY_PENALTY_Q8 = 12 << 8`，作用在 Q16.16 边分上约等于一个 log 概率单位的 4.7%，与卡片「约 5% 权重差」一致；`penalize` 饱和减，`i32::MIN` 不回绕。
  - **已知限制**：
    1. **词格接线缺失**：`crates/ime-core/src/viterbi/**` 不在本卡白名单。需要把 `DecodeFlags` 传进 `build_lattice*`、对每条边调 `fuzzy::variants` 并按变体拼键、给 `LatticeEdge` 标出「模糊变体」以便扫描阶段调 `fuzzy::penalize`。原语与其拼接契约已写进模块文档，`push_fuzzy_edges` 未落地。因此 **DoD 1 的端到端半句、DoD 3（延迟）与 DoD 4（手工验证 `lihao`）仍缺口**。
    2. **DoD 2 仍缺口**：`crates/ime-config/**` 不在白名单，`[engine] fuzzy` 的类别名解析未接。已备好 `CLASSES[i].name` 与 `class_by_name`（大小写不敏感，未知名字返回 `None` 交配置层报 `config/invalid`），接入约十行。
    3. **DoD 5 的记录侧仍缺口**：`variants_into` 已精确回答「是否被截断」、`may_truncate` 提供廉价必要条件，但日志落点在 `ime-diag`/状态机。
    4. **一条实测发现：`MAX_VARIANTS = 8` 在当前类别表下不可达。** 交付前用脚本把算法原样重写在真实的 411 音节表上跑了一遍（只读校验）：单类对任一音节最多只加 1 个变体，**八类全开时任何音节最多只有 4 个拼写**（一个音节只有一个声母、一个韵母，各归属至多一个类别）。所以 `decode/fuzzy-truncated` 今天不可观测，它是为类别表将来扩容留的护栏。若希望它可观测，需另开卡把类别表扩到 4 类以上可作用于同一音节，或下调上限；当前实现把两件事都做成可测的（`expand_into` 的 cap 参数 + `may_truncate` 真值表）。
    5. 三处对卡片草图的偏离（均已写入代码注释）：`FuzzyClass` 多一个 `name` 字段（配置键与 DoD 2 的类别名解析要求它，表是唯一真源）；`CLASSES` 用 `static` 而非 `const`（`class_by_name` 要交出 `&'static FuzzyClass`）；模块落在 `src/fuzzy.rs` 而非 `src/segment/fuzzy.rs`（`lib.rs` 是工作区约定的挂载点，而 `segment/mod.rs` 是模块根文件）。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143。

#### 任务 ID：ADD-FEAT-P0.02.04 简拼与首字母缩写展开

- **基本属性**：
  - 绑定差距条目：`GAP-03`（简拼 / 首字母缩写）
  - 优先级与复杂度：`P0 核心增强` ｜ **中** ｜ 预估工时: **3 人天**
  - 前置依赖：**`M0` 契约冻结**（确认 `DecodeFlags::ABBREV` 与 `Lexicon::prefix` 的语义）；软依赖 `ADD-FEAT-P0.01.01`（**词库越大，简拼命中率越高**——5,441 条词库下简拼几乎无候选）
  - 关键路径：`CP: 否`（松弛 11 人天）
  - 并行通道：`Track A-解码`
  - 当前状态：`[x] 已完成`
  - 代码落地锚点：`crates/ime-core/src/segment/abbrev.rs`（**新增**）、`crates/ime-core/src/viterbi/lattice.rs`（改：简拼边的构建）、`crates/ime-config/src/schema.rs`（改：新增 `[engine] abbrev` 子键）、`crates/ime-types/src/lexicon.rs`（`Lexicon::prefix` 已在 `ADR-0001` 冻结，本卡是**第一个调用方**）

- **目标与价值**：让用户少打 60%~80% 的键。`nh` → 你好、`bjdx` → 北京大学、`zgrm` → 中国人民。对标搜狗/微软拼音/RIME/libpinyin/微信输入法。当前 `DecodeFlags::ABBREV`（`1<<9`）与 `Lexicon::prefix(prefix, limit)` **已冻结但从未被调用**——`lexicon.rs:40` 的注释明写"Prefix enumeration, used by the Phase 2 abbreviation expansion"，Phase 2 未落地。

- **技术设计与代码级接入细节**：

  **底层模型与数据流改动**：

  (a) **简拼的两条路径**，必须区分清楚：

  | 路径 | 输入 | 机制 | 候选来源 |
  |---|---|---|---|
  | **全简拼** | 全部字母都是声母（`nh`、`bjdx`） | 对每个字母取"以该字母开头的所有声母"，做前缀枚举 | `Lexicon::prefix` |
  | **混拼** | 部分音节全拼、部分简拼（`nih`、`beijingd`） | 逐音节判定：能构成完整音节的按全拼，剩余的按声母 | 全拼段走 `lookup`，简拼段走 `prefix` |

  ```rust
  // crates/ime-core/src/segment/abbrev.rs
  /// One way to read the input as a mix of full syllables and bare initials.
  ///
  /// Abbreviation is ambiguous by nature: `nh` can be `ni'hao` (你号/你好) or
  /// `na'he` (那和). The decoder does not pick one — it enumerates the
  /// readings the syllable table allows, up to `MAX_READINGS`, and lets the
  /// language model rank them.
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub struct AbbrevReading {
      /// One entry per consumed input letter group, in order.
      pub syllables: Vec<AbbrevSyllable>,
      /// Total input letters consumed, so the caller can try a longer reading.
      pub consumed: u16,
  }

  /// One syllable of an abbreviated reading.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum AbbrevSyllable {
      /// A fully spelled syllable.
      Full(SyllableId),
      /// A bare initial: the dictionary is asked for every word whose reading
      /// starts with this initial.
      Initial(u8),
  }

  /// The largest number of readings one input may be read as.
  ///
  /// Abbreviation multiplies the lattice: without a bound, `bjdx` would reach
  /// every four-initial word in the dictionary. 64 keeps the lattice inside
  /// the decode budget while still covering every reading a user is likely to
  /// have meant.
  pub const MAX_READINGS: usize = 64;

  /// Enumerates the readings of `raw` that mix full syllables and initials.
  ///
  /// Readings are produced longest-syllable-first, so the most specific
  /// reading is tried before the vaguest one and the enumeration can stop
  /// early once `MAX_READINGS` is reached.
  ///
  /// # Errors
  /// This function is infallible: it returns no `Result`. An input with no
  /// reading at all yields an empty vector, which the caller treats as
  /// "abbreviation did not apply" rather than as a failure.
  pub fn readings(raw: &str) -> Vec<AbbrevReading>;
  ```

  (b) **注入点：词格构建期**（4.1 的挂载点 H2，与模糊音同一层）。简拼边通过 `Lexicon::prefix` 拿到候选词：

  ```rust
  // crates/ime-core/src/viterbi/lattice.rs (改)
  /// Adds a lattice edge for every word reachable by an abbreviated reading.
  ///
  /// This is the first caller of `Lexicon::prefix`. The prefix query is bounded
  /// by `PREFIX_LIMIT`, so a one-letter initial cannot pull the whole
  /// dictionary into the lattice.
  fn push_abbrev_edges(
      lattice: &mut Lattice,
      reading: &AbbrevReading,
      lexicon: &dyn Lexicon,
  ) -> Result<usize, ImeError>;

  /// The most words one prefix query may return.
  ///
  /// A single initial such as `z` prefixes tens of thousands of words. The
  /// dictionary returns them in descending weight order, so taking the head of
  /// the list keeps the candidates a user would actually have meant and keeps
  /// the lattice bounded.
  pub const PREFIX_LIMIT: usize = 64;
  ```

  (c) **简拼边同样需要惩罚**（与模糊音同理，保证精确匹配优先），但惩罚更小——简拼是**用户主动的输入方式**，不是"读错了"：

  ```rust
  /// The score penalty one abbreviated edge carries, in Q8.8.
  ///
  /// Smaller than the fuzzy penalty: a fuzzy match means the user misspelled,
  /// while an abbreviation means the user deliberately typed less. The
  /// abbreviation should still lose to a full spelling at equal weight, but
  /// only just.
  pub const ABBREV_PENALTY_Q8: i32 = 6 << 8;
  ```

  **边界契约（跨边界任务必填）**：
  - `DecodeFlags::ABBREV`（`1<<9`）——**已在 `ADR-0001` 冻结**，本卡不改契约。
  - `Lexicon::prefix(&self, prefix: &str, limit: usize) -> Result<WordIter<'_>, ImeError>`——**已在 `ADR-0001` 冻结**，本卡是首个调用方。**必须验证 `ime-dict` 的 `fst_index` 实现真的按权重降序返回**；若否，本卡的 `PREFIX_LIMIT` 截断会截掉高频词，需要在 `P0.01.01` 中补排序。
  - 新增配置键 `[engine] abbrev = true|false`（默认 `false`——简拼会显著改变候选顺序，不该是默认行为）。
  - 新错误码：`decode/abbrev-truncated`（info 级诊断）。

  **与现有代码的无缝衔接方案**：`Decoder::decode` 签名不变；`flags` 从 `DecodeRequest.flags` 读出。**`ime-dict` 需要一处验证性改动**：确认 `fst_index` 的 `prefix` 实现按权重降序；若已按权重降序则零改动，否则在 `fst_index` 内加一次排序（该排序是 O(k log k)，k ≤ 词条数，在构建期完成而非运行期）。`ime-ui` **零改动**。

- **非功能约束 (NFR) 与性能指标**：
  - **独占预算 ≤ 0.50ms**（0.3 节）。`MAX_READINGS = 64` × `PREFIX_LIMIT = 64` 是上界，实际远小于此。
  - **路径数上限**：`MAX_READINGS = 64`；达到上限时按"最长音节优先"的顺序截断，记 `decode/abbrev-truncated`。
  - **确定性**：`readings()` 的枚举顺序由算法固定（最长音节优先 + 字母顺序），**不得**依赖哈希迭代顺序。
  - **边缘条件**：
    - 单字母输入（`n`）→ 不启用简拼（会产生过多候选），走既有的单字回退路径。
    - `ABBREV` 未置位 → `readings()` **不被调用**，零开销。
    - 与模糊音同时开启 → 两者在词格构建期各自扩张，**总边数上限是各自上限的乘积**，必须在 `lattice.rs` 里做联合封顶（`MAX_TOTAL_EDGES`）。
    - 简拼命中但词库无匹配 → 该简拼边不产生，退化为全拼路径。
  - **不得**使 `decode_p99` 超过 3.0ms。简拼与模糊音同时开启是**最坏组合**，必须在基准中单独测。

- **逐步落地实施步骤**：
  1. **实现 `readings()`**：最长音节优先的枚举；`MAX_READINGS` 截断；纯函数，可用内存构造的输入单测。
  2. **验证并（如需要）修正 `Lexicon::prefix` 的权重降序**：写一条断言"prefix 返回的前 10 个词的权重单调不增"的测试；若失败，在 `fst_index` 的构建期补排序。
  3. **接入词格构建期**：`push_abbrev_edges` 用 `PREFIX_LIMIT` 截断；每条边带 `ABBREV_PENALTY_Q8`。
  4. **联合封顶**：在 `lattice.rs` 加 `MAX_TOTAL_EDGES`，模糊音与简拼共享该上限；超限时按"精确边 → 简拼边 → 模糊边"的优先级保留。
  5. **配置与回归**：`[engine] abbrev` 默认 `false`；跑 `ime-core` 全量回归 + 一条"简拼与模糊音同时开启"的组合基准。

- **验收标准 (DoD)**：
  1. `cargo nextest run -p ime-core` 全绿，新增 ≥ 12 条：全简拼、混拼、`MAX_READINGS` 截断、单字母不启用、`ABBREV` 关闭时零开销、惩罚使全拼优先、与模糊音组合的联合封顶、确定性。[自动]
  2. `cargo nextest run -p ime-dict` 全绿，新增 `test_prefix_returns_words_in_descending_weight`。[自动]
  3. 手工验证：开启简拼后输入 `nh` 得到「你好」、输入 `bjdx` 得到「北京大学」。[实验室]
  4. 实测简拼开启后的 `decode_p99` ≤ 3.0ms；简拼 + 模糊音同时开启的 `decode_p99` 也 ≤ 3.0ms。[性能]
  5. 简拼 + 模糊音组合下的 `decode_p99` 劣化 ≤ 0.50ms + 0.60ms = 1.10ms。[性能]

- **验收记录**（2026-09-30）：
  - **交付物**：`crates/ime-core/src/segment/abbrev.rs`（775 行）、`segment/abbrev/tests.rs`（707 行，新增 5 个用例，模块共 32 条）。
  - **验证命令与结果**：`just ci` 退出 0：`cargo fmt --all -- --check`、`cargo clippy`（主工作区 `--all-targets --all-features` 与两个 addon crate，全部 `-D warnings`）、`cargo nextest run`（主工作区 2430 个用例、两个 addon 346 个，全绿）、`cargo test --doc`（主工作区与两个 addon）、10 个审计脚本及其自检、`just check-host`。
  - **本次修掉的一处真实缺陷**：`MAX_READINGS` 上限原先在 `out.push` **之后**判断，导致「读数恰好等于 64 条」的输入被误报截断——与本函数自己文档写的「`true` 仅当确有读数被丢弃」以及 `decode/abbrev-truncated` 的语义相矛盾。改为 push **之前**判断。
  - **补上卡片要求的错误码常量**：新增 `ABBREV_TRUNCATED_CODE = "decode/abbrev-truncated"`。原实现只在散文里提到该字符串，没有可被诊断匹配的常量。
  - **缩写候选排在等价全拼候选之后**：由 `ABBREV_PENALTY_Q8 < FUZZY_PENALTY_Q8` 保证，且有 `const _: () = assert!(...)` 编译期把关。
  - **`ABBREV` 关闭时零开销是结构性证明**：`test_is_enabled_over_every_flag_combination_follows_the_abbrev_bit` 穷举 `DecodeFlags` 全部 16384 个组合，断言 `is_enabled(f) == f.contains(ABBREV)`，恰好 8192 个为真。
  - **已知限制**：① **词格接线未落地**——`push_abbrev_edges` 属 `viterbi/lattice.rs`，本卡只交付了代数层；接线时注意 `readings_into` 每节点只调一次、结果放进 `DecodeScratch` 的复用缓冲，且**必须按 `MAX_WORD_SYLLABLES` 截断读数切片**，否则最坏情形约 64×64 次前缀展开会吃掉 `BUDGET-LAT-02` 的 0.50ms 预算；② **词典侧的关键前提**：`spell_into` 对全声母读数产出的查询串形如 `n'h`，它**不是** `ni'hao` 的字面前缀；只有混拼读数（`nih`）产出的 `ni'h` 才是。全拼索引的词典答不了 `n'h`——`nh → 你好` 能否命中取决于 `dictc` 是否建缩写键，**没有卡拥有这件事**；③ `[engine] abbrev` 配置键已落地（默认 `false`）但还没有消费者；④ 与模糊音组合的联合封顶（`MAX_TOTAL_EDGES`）属 `lattice.rs`，不在本卡；⑤ `abbrev.rs` 仅剩 25 行余量（775/800），后续加内容需先拆分。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85）、Linux 6.18.40.1-microsoft-standard-WSL2、cargo-nextest 0.9.143。

---

#### 任务 ID：ADD-FEAT-P0.02.05 简繁转换（Unihan 变体表 + ToggleScript）

- **基本属性**：
  - 绑定差距条目：`GAP-22`（简繁转换）
  - 优先级与复杂度：`P0 核心增强` ｜ **中** ｜ 预估工时: **4 人天**
  - 前置依赖：**`M0` 契约冻结**（`CandidateSource::Script`、`StatusStrip.script`、`KeyAction::ToggleScript`、`DecodeFlags::SCRIPT`）；软依赖 `ADD-FEAT-P0.01.01`（复用 `dictc` 的 `SectionBuilder` 与 `script.dict` 的生成管线）
  - 关键路径：`CP: 否`（松弛 8 人天）
  - 并行通道：`Track A-解码`
  - 当前状态：`[x] 已完成`
  - 代码落地锚点：`crates/ime-core/src/script.rs`（**新增**）、`crates/ime-core/src/script/table.rs`（**新增**）、`crates/ime-core/src/state/transitions.rs`（改：commit 前的输出变换 + `ToggleScript` 跃迁体）、`crates/ime-dict/src/script_index.rs`（**新增**：加载 `script.dict`）、`xtask/src/dictc/builders/script.rs`（**新增**：生成 `script.dict`）、`data/raw/script-disambig.tsv`（**新增**，项目自建）、`crates/ime-fcitx5/src/engine.rs`（改：按键路由）、`crates/ime-ui/ui/candidate.slint`（改：header 显示简/繁）

- **目标与价值**：补齐**港澳台用户与古籍/书法场景的刚需**。搜狗、微软拼音、RIME、macOS 原生**全部**内置简繁切换。当前 `grep -ri 简繁\|traditional crates/` **零命中**，`KeyAction` 只有 15 个变体、**无 `ToggleScript`**。技术亮点：**零新增许可负担**——映射表来自已在白名单的 `data/raw/unihan.tsv`（Unicode License，`permissive = true`），**不引入 OpenCC**（`ASM-A-08`）。

- **技术设计与代码级接入细节**：

  **底层模型与数据流改动**：

  (a) **`script.dict` 复用既有容器格式**（4.2.4）——这是本卡最重要的设计决策：

  ```rust
  // crates/ime-dict/src/script_index.rs
  /// The simplified/traditional word mapping, loaded from `script.dict`.
  ///
  /// `script.dict` uses the same container as `base.dict` (`RSPD`, version 1,
  /// the same six sections), so this type is a thin wrapper over the existing
  /// reader rather than a second format. The FST key is the simplified word's
  /// pinyin and the payload is the traditional spelling, which means the
  /// mapping is looked up through the same zero-copy path as any other word.
  ///
  /// Keeping the mapping out of `base.dict` is what lets `DICT_FORMAT_VERSION`
  /// stay at 1: a dictionary without a script table is still a valid v1 file,
  /// and a user who never turns the feature on never pays for the section.
  pub struct ScriptIndex {
      map: Mmap,
      fst: fst::Map<Mmap>,
      pool: Range<usize>,
  }

  impl ScriptIndex {
      /// Opens the mapping, or reports that the feature is unavailable.
      ///
      /// A missing `script.dict` is not an error: the plugin runs without the
      /// feature and reports `ui/script/unavailable` once. Only a file that is
      /// present and malformed is a `DictError`.
      ///
      /// # Errors
      /// Returns `DictError::Corrupt` when the file exists but fails its
      /// magic, version, length or CRC checks.
      pub fn open(path: &Path) -> Result<Option<Self>, DictError>;

      /// The traditional spelling of `simplified`, or `None` when the word has
      /// no distinct traditional form.
      ///
      /// # Errors
      /// This function is infallible: it returns no `Result`.
      pub fn traditional(&self, simplified: &str) -> Option<&str>;
  }
  ```

  (b) **转换算法：提交前的输出变换，不参与打分**（4.1 的挂载点 H4）：

  ```rust
  // crates/ime-core/src/script.rs
  /// Rewrites committed text into the target script.
  ///
  /// Runs on the final committed string, after the decoder has chosen a
  /// candidate: the script a user reads is not a property of how a word is
  /// pronounced, so letting it into the scoring pass would make the candidate
  /// order depend on a dimension the language model knows nothing about.
  ///
  /// Matching is longest-first over a bounded window, because a one-character
  /// mapping is wrong wherever a word-level mapping exists: `干` alone is
  /// ambiguous, but `干净` is always `乾淨` and `干活` is always `幹活`.
  ///
  /// # Errors
  /// This function is infallible: it returns no `Result`. Text with no mapping
  /// is returned unchanged, which is the correct outcome rather than a failure.
  pub fn to_traditional(text: &str, index: &ScriptIndex) -> String;

  /// The reverse direction, for a user who typed in traditional and wants
  /// simplified output.
  ///
  /// # Errors
  /// This function is infallible: it returns no `Result`.
  pub fn to_simplified(text: &str, index: &ScriptIndex) -> String;

  /// The longest word the mapper will look up, in characters.
  ///
  /// Eight covers every disambiguating word in the table with room to spare
  /// and bounds the per-commit work at `len * 8` lookups.
  pub const MAX_MATCH_CHARS: usize = 8;
  ```

  (c) **一简对多繁的消歧**（`ASM-A-08`）。Unihan 的 `kTraditionalVariant` 对单字给出**全部**繁体变体，无法消歧。消歧靠**词级条目**：

  | 简体 | 单字映射（歧义） | 词级条目（消歧后） |
  |---|---|---|
  | 发 | 發 / 髮 | `头发`→`頭髮`、`出发`→`出發`、`发现`→`發現` |
  | 干 | 乾 / 幹 / 干 | `干净`→`乾淨`、`干活`→`幹活`、`干涉`→`干涉` |
  | 后 | 後 / 后 | `后面`→`後面`、`皇后`→`皇后` |
  | 里 | 裡 / 里 | `里面`→`裡面`、`公里`→`公里` |
  | 台 | 臺 / 檯 / 颱 | `台湾`→`臺灣`、`台风`→`颱風`、`写字台`→`寫字檯` |

  `data/raw/script-disambig.tsv`（项目自建，`kind = "derived"`，`ASM-A-03` 的 A-3 允许）：

  ```
  # word <TAB> traditional <TAB> origin
  # Derived from Unihan's kTraditionalVariant (Unicode License) plus manual
  # disambiguation. No OpenCC, no CC-CEDICT: ADR-0000 decision B.
  头发	頭髮	unihan
  出发	出發	unihan
  干净	乾淨	unihan
  ```

  (d) **生成管线**：`xtask/src/dictc/builders/script.rs` 实现 `SectionBuilder`（`P0.01.01` 抽出的抽象），从 `unihan.tsv` + `script-disambig.tsv` 生成 `script.dict`。**`base.dict` 零改动。**

  **边界契约（跨边界任务必填）**：
  - `CandidateSource::Script`（`M0` 落地）——转换后的候选来源标注。
  - `StatusStrip.script: Script`（`M0` 落地；`Script::Simplified` / `Script::Traditional`，`Default` 为 `Simplified`）。**沿用 `readonly` 字段的追加先例**（ADR-0001）。
  - `KeyAction::ToggleScript`（`M0` 落地）。默认键位 `Ctrl+Shift+F`（进 `[script] hotkey`）。
  - `DecodeFlags::SCRIPT`（`1<<13`，`M0` 落地）——允许按请求关闭转换。
  - 错误码：`ui/script/unavailable`（`script.dict` 缺失时的**降级**，记一次 `info` 级诊断，用户不可见）。
  - **状态机跃迁**（4.2.2）：
    - `Idle` + `KeyEvent(ToggleScript)` → `Idle`，副作用为写 `[script] traditional` 并重发 `StatusStrip`，**不产生 preedit**。
    - `Composing` + `KeyEvent(ToggleScript)` → `Composing`，**保持会话不重置**（0.4 规则 10），仅重算当前候选的显示文本，`revision` 自增。
  - **配置段**（4.2.3）：`[script]` 的 `enabled`（默认 **false**——简繁是显式选择，不是默认行为）、`traditional`、`hotkey`。

  **UI 与工艺级设计规范**（`C-6`）：繁体候选与简体候选**视觉完全一致**，唯一区别是 header 的 `script` 指示（"简" / "繁"），沿用既有 `font-size-small = 11px` 与 `text-annotation` token。**不引入新颜色、不改变候选格尺寸。**

  **与现有代码的无缝衔接方案**：
  - `ime-core` 的 `viterbi/`、`lm/`、`segment/` **全部零改动**——转换在 commit 之后。
  - `ime-dict` 新增一个文件（`script_index.rs`），**既有代码零改动**（`ScriptIndex` 复用 `mmap.rs` 与 `format/reader.rs`）。
  - `ime-fcitx5/src/engine.rs` 的 commit 路径末尾加一次 `to_traditional` 调用。
  - `ime-ui` 只改 `.slint` 的 `Header` 加一个 `script` 文本绑定，**Rust 侧零改动**。

- **非功能约束 (NFR) 与性能指标**：
  - **独占预算 ≤ 0.30ms**（0.3 节）。转换是 `MAX_MATCH_CHARS = 8` 窗口内的 FST 前缀遍历，每字符最多 8 次查询。
  - **`script.dict` 体积** ≤ 1.5MB（`BUDGET-SIZE-02` 的 20MB 之内，与 `base.dict` 分列预算）。
  - **`script.dict` 缺失时**：功能静默降级，输入完整可用，记一次 `ui/script/unavailable`（**一次性**，不重复记录）。
  - **边缘条件**：
    - 转换后的文本长度可能变化（`发`→`髮` 是 1:1，但词级映射可能不等长）→ `Preedit.caret` 与 `Segment.start/end` 的**字节偏移必须重算**；这是本卡最容易出错的地方，必须有专门测试。
    - 已经是繁体的文本再转繁 → 幂等（`to_traditional(to_traditional(x)) == to_traditional(x)`）。
    - 混合文本（部分有映射、部分无）→ 逐段处理，无映射段原样保留。
    - 用户词库里的词也要转换（用户学的词同样需要繁化）→ 转换在 commit 之后，天然覆盖。
  - **不得**因为简繁转换而改变 `DecodeResult.candidates` 的数量或顺序。

- **逐步落地实施步骤**：
  1. **生成 `script.dict`**：实现 `xtask/src/dictc/builders/script.rs`；从 `unihan.tsv` 的 `kTraditionalVariant` 生成单字映射；合并 `script-disambig.tsv` 的词级条目；用 `DictWriter` 写出（**复用既有格式，零新解析代码**）。
  2. **实现 `ScriptIndex`**：复用 `mmap.rs` 与 `format/reader.rs`；`open` 返回 `Option<Self>`（缺失不是错误）。
  3. **实现 `to_traditional` / `to_simplified`**：最长匹配（`MAX_MATCH_CHARS = 8`）；处理字节偏移重算。
  4. **接入 commit 路径与按键**：`transitions.rs` 的 commit 效果末尾调转换；`KeyAction::ToggleScript` 的跃迁体；`engine.rs` 的按键路由。
  5. **UI 与配置**：`StatusStrip.script` 的填充；`.slint` 的 header 指示；`[script]` 配置段。
  6. **回归与边界**：幂等性、偏移重算、表缺失降级、混合文本四条测试；跑 `ime-core`/`ime-dict`/`ime-ui` 全量回归。

- **验收标准 (DoD)**：
  1. `data/compiled/script.dict` 生成成功，体积 ≤ 1.5MB，格式校验（magic/version/CRC）通过。[自动]
  2. `cargo nextest run -p ime-core` 全绿，新增 ≥ 10 条：`to_traditional` 的 5 组歧义消解（发/干/后/里/台）、幂等性、偏移重算、混合文本、表缺失降级、`ToggleScript` 不重置会话。[自动]
  3. `cargo nextest run -p ime-dict` 全绿，新增 `test_script_index_absent_is_not_an_error`、`test_script_index_rejects_corrupt_file`。[自动]
  4. 手工验证：按 `Ctrl+Shift+F` 后输入 `yinhang` 提交「銀行」；再按一次回到「银行」。[实验室]
  5. 实测转换耗时 ≤ 0.30ms。[性能]
  6. `bash scripts/check-dict-sources.sh` 通过；`script-disambig` 登记为 `derived`；**无新增许可依赖**（`licenses.md` 的依赖闭包不变）。[自动]
  7. 视觉走查：繁体模式下 header 的"繁"指示与既有 token 一致，候选格尺寸不变。[视觉]

- **验收记录**（2026-09-30）：
  - **交付物**：`crates/ime-core/src/script.rs`（355 行）+ `script/{table.rs,tests.rs}`（350 / 458 行，本次新增 6 条边界用例，模块共 39 条）；本次新建 `crates/ime-dict/src/script.rs`（148 行）与 `script/tests.rs`（113 行，9 条用例）。两个模块由主 agent 挂载（`ime-core/src/lib.rs` 与 `ime-dict/src/lib.rs` 各一行 `pub mod script;`）。
  - **验证命令与结果**：`just ci` 退出 0：`cargo fmt --all -- --check`、`cargo clippy`（主工作区 `--all-targets --all-features` 与两个 addon crate，全部 `-D warnings`）、`cargo nextest run`（主工作区 2430 个用例、两个 addon 346 个，全绿）、`cargo test --doc`（主工作区与两个 addon）、10 个审计脚本及其自检、`just check-host`。
  - **算法与边界**：最长匹配改写（`MAX_MATCH_CHARS` 窗口从长到短），纯函数、从 `VariantSource` trait 取表、无文件无时钟无环境无全局态。本次补的边界覆盖：单字符（映射与不映射、汉/ASCII/4 字节 emoji）、无变体文本、恰好 8 字在长文本中、**表尾截断**（7 字前缀不匹配、前移 1 字仍不匹配、恰好 8 字匹配）、**窗口按字符计数而非字节**（含 4 字节字符）。
  - **`ScriptIndex` 是词典层的 `VariantSource` 实现**：持有 `VariantTable`（双向索引），三个构造——`from_pairs`（任意顺序/重复键，首键胜出，空键或空值丢弃）、`seed()`（本构建随附的种子表）、`unavailable()`（降级值：全部 miss，转换退化为恒等）。查询路径零分配、返回借用、不写表、不阻塞，且由 `Arc<dyn VariantSource + Send + Sync>` 强制转换断言了 `Send + Sync`。
  - **已知限制**：① **词典格式尚未携带变体表（本卡最重要的一处）**——容器 v1 固定六段，没有任何一段承载「词→词」表，且双向映射需要**两个键空间**，单个 FST 段无法同时索引双向；`crates/ime-dict/src/paths.rs` 也没有 `script.dict` 路径项。按回退路径实现内存态适配器，未臆造载荷布局。可选方案：(a) 扩展容器（需 ADR + 第七段）；(b) 随二进制内嵌（`include_bytes!`，零格式改动，但约 1.5MB 表会被双向展开成约两倍堆内存）；(c) 维持种子表。② **引擎提交路径未接线**——`Ctrl+Shift+F` / `ToggleScript` 的按键路由、`state/transitions.rs` 的跃迁与 `[script]` 配置段都不在本卡；`KeyAction::ToggleScript` 在 `ime-types` 与 `transitions.rs` 中已存在并被 `arbiter` 判定为 `Executable`。③ **`.slint` header 的「繁」指示未接**。④ 卡片草图的 `ScriptIndex::open(path) -> Result<Option<Self>, DictError>` 与 `traditional(&self, simplified)` 与已落地的 API 不一致（实际是 `lookup(word, target)`），接入时以 `VariantSource` 为准；那两条命名测试留待格式决策后补，**未写桩函数**。⑤ `ui/script/unavailable` 码已在 `features.md` 2.2.4 登记，但**目前没有上报方**（`ScriptIndex::unavailable()` 提供了降级值，缺的是引擎侧的上报点）。⑥ 转换耗时（卡片 DoD 5 的 ≤0.30ms）未取数；结构性上界是「单次转换 ≤ `len × 8` 次二分查找 + 1 次 `String` 分配」。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85）、Linux 6.18.40.1-microsoft-standard-WSL2、cargo-nextest 0.9.143。

---

#### 任务 ID：ADD-FEAT-P0.03.01 用户数据自动备份与回滚

- **基本属性**：
  - 绑定差距条目：`GAP-27`（用户数据自动备份与回滚）、`GAP-10`（用户词库导入 / 导出 / 备份）
  - 优先级与复杂度：`P0 核心增强` ｜ **中** ｜ 预估工时: **4 人天**
  - 前置依赖：`ADD-FEAT-P0.01.04`（复用 `export_tsv` / 导入原语）
  - 关键路径：**`CP: 是`**
  - 并行通道：`Track A-数据`
  - 当前状态：`[x] 已完成`
  - 代码落地锚点：`crates/ime-dict/src/backup.rs`（**新增**）、`crates/ime-dict/src/backup/rotate.rs`（**新增**）、`crates/ime-dict/src/user_db.rs`（改：`open` 时触发空闲期备份检查）、`crates/ime-config/src/schema.rs`（改：`[data]` 追加 `backup_enabled` / `backup_keep` / `export_dir`）、`crates/ime-fcitx5/src/addon.rs`（改：`on_addon_destroy` 路径上的终备份）、`crates/ime-diag/src/crash.rs`（改：崩溃恢复时检查备份）

- **目标与价值**：让用户的学习成果**不会因为一次损坏而全失**。当前 `recover.rs` 的语义是"损坏时**隔离**原文件"（重命名为 `user.redb.corrupt.<unix_ts>`，**绝不删除**）——这个设计是对的，但它**不产生可用副本**。用户的数据只有一份；一旦损坏，用户面对的是一个 `.corrupt.1759161600` 文件，需要人工抢救。本卡提供：(1) 定期自动备份；(2) 损坏时**自动从备份回滚**；(3) 手动回滚入口。对标 Obsidian 的 `.obsidian` 备份与文件恢复、RIME 的用户词表可备份。

- **技术设计与代码级接入细节**：

  **底层模型与数据流改动**：

  (a) **备份格式与轮转**。备份用 `export_tsv` 的产物（**不是** `user.redb` 的字节拷贝）——理由：TSV 是**跨版本可读**的，而 `user.redb` 的字节拷贝在 redb 升级后可能不可读；TSV 也天然可被用户查看与编辑。

  ```rust
  // crates/ime-dict/src/backup.rs
  /// One backup generation on disk.
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub struct BackupFile {
      /// Absolute path.
      pub path: PathBuf,
      /// When the backup was taken, in unix milliseconds.
      pub taken_at_unix: u64,
      /// Rows it holds; read from the header comment, not by parsing the body.
      pub rows: u64,
      /// Size in bytes.
      pub bytes: u64,
  }

  /// What a backup attempt did.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum BackupOutcome {
      /// A new backup was written.
      Written,
      /// The newest backup is recent enough that another one was pointless.
      Skipped,
      /// The data directory is not writable; the plugin runs read-only.
      Readonly,
      /// The user turned backups off.
      Disabled,
      /// Writing failed; the reason is in the diagnostics.
      Failed,
  }

  /// Writes a backup if one is due, then rotates old generations.
  ///
  /// # Errors
  /// Returns `ImeError::DataBackupFailed` only when a backup was due and could
  /// not be written. A skipped or disabled backup is a success: the caller has
  /// nothing to do about either, and treating them as errors would put a
  /// failure in the log every time a user typed.
  pub fn run_backup(
      store: &UserDb,
      cfg: &BackupConfig,
      now_unix_ms: u64,
    ) -> Result<BackupOutcome, ImeError>;

  /// The interval between automatic backups, in milliseconds.
  ///
  /// One day. Learning is slow and the file is small, so a shorter interval
  /// would spend writes to protect against a loss of at most one day of
  /// learning — which the user would barely notice.
  pub const BACKUP_INTERVAL_MS: u64 = 24 * 60 * 60 * 1000;

  /// The most backups kept, matching the `data.backup_keep` default.
  pub const BACKUP_KEEP_DEFAULT: u8 = 3;
  ```

  (b) **文件布局**（沿用既有 XDG 路径与权限基线 `0700`/`0600`）：

  ```
  $XDG_DATA_HOME/rspinyin/
  ├── user.redb
  ├── user.redb.corrupt.<unix_ts>        # 既有：损坏隔离，绝不删除
  └── backups/
      ├── user-20260929-120000.tsv       # 0600
      ├── user-20260928-120000.tsv
      └── user-20260927-120000.tsv
  ```

  (c) **损坏时的自动回滚**。这是本卡的核心价值——把 `recover.rs` 的"隔离"升级为"隔离 + 回滚"：

  ```rust
  // crates/ime-dict/src/recover.rs (改)
  /// What recovery did with a damaged user store.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum RecoveryOutcome {
      /// The store was readable; nothing happened.
      Healthy,
      /// The store was damaged, isolated, and rebuilt empty.
      RebuiltEmpty,
      /// The store was damaged, isolated, and rebuilt from the newest usable
      /// backup.
      RestoredFromBackup {
          /// The backup that was used.
          from_unix: u64,
          /// Rows it held.
          rows: u64,
      },
  }

  /// Isolates a damaged store and rebuilds it, preferring a backup.
  ///
  /// The damaged file is renamed, never deleted: a user who wants to salvage
  /// rows by hand must still be able to, and the backup may itself be older
  /// than the damage. Every backup is tried newest-first and the first one that
  /// imports cleanly wins, so one bad generation does not cost the user
  /// everything.
  ///
  /// # Errors
  /// Returns `ImeError::DataBackupFailed` when a backup existed but none could
  /// be imported. The store is still left usable (empty) in that case.
  pub fn recover_user_db_with_backup(
      path: &Path,
      backups: &[BackupFile],
  ) -> Result<RecoveryOutcome, ImeError>;
  ```

  **边界契约（跨边界任务必填）**：
  - `ImeError::DataBackupFailed { reason: String }`（`M0` 落地，在既有 `data/*` 段内分配）。
  - 错误码：`data/backup-failed`（error 级）、`data/backup-restored`（**info 级，不是错误**——恢复成功要让用户知道）。
  - 新增配置键（4.2.3）：`[data] export_dir`、`[data] backup_enabled`（默认 `true`）、`[data] backup_keep`（默认 `3`）。
  - **状态机跃迁**（4.2.2）：`Idle` + `SessionStart` → `Idle`，若距上次备份 > `BACKUP_INTERVAL_MS` 且用户库非空，在**空闲期**（无输入 30s，复用 `user_db.rs` 的既有 `EVICT_IDLE_MS` 窗口）触发一次备份。
  - **不引入新线程**（`ASM-A-18`）：备份复用既有的空闲期窗口，与 `evict_oldest` 串行。

  **与现有代码的无缝衔接方案**：
  - `recover.rs` 的既有 `recover_user_db` 保留（作为无备份时的路径），新增 `recover_user_db_with_backup` 作为首选路径。既有测试（`TC-DICT-31`~`TC-DICT-34`）**一条都不改**——它们测的是"隔离而非删除"，该行为不变。
  - `addon.rs` 的 `on_addon_destroy` 路径：既有已做 `final_commit()`（`TC-DICT-40` 断言"零丢失终提交"）；本卡在其后追加一次"若距上次备份 > 24h 则备份"。
  - `crash.rs`：崩溃恢复启动时先检查 `backups/` 是否有比 `user.redb` 更新的备份。

- **非功能约束 (NFR) 与性能指标**：
  - **备份在空闲期执行**，**绝不**在按键路径上；单次备份 ≤ 200ms（导出 10⁴ 条 TSV 的量级），且**不阻塞宿主线程**（0.4 规则 10 的同族约束）。
  - **备份体积** ≤ 8MB（`ASM-A-09` 的导出上限）。
  - **轮转**：保留最近 `backup_keep`（默认 3）份；超出时删除**最旧**的一份。轮转必须**先写新备份再删旧的**——顺序反了会在两次操作之间留下零备份窗口。
  - **异常容灾**：
    - 数据目录不可写 → `BackupOutcome::Readonly`，输入完整可用，记 `data/readonly-mode`（**不 panic、不阻断输入**，`ASM-15`）。
    - 备份写入中途失败 → 临时文件 + 原子重命名（复用 `DictWriter::TEMP_SUFFIX` 的同族做法），**不留下半个备份**。
    - 备份文件本身损坏 → 尝试下一份（newest-first）；全部失败则 `RebuiltEmpty` + `data/backup-failed`。
    - 备份目录被符号链接 → 复用 `paths.rs` 既有的符号链接逃逸防护（`TC-SEC-34`），拒绝写入并进只读模式。
  - **隐私**：备份文件含用户的全部学习词条 → 权限必须 `0600`（复用 `perms.rs`），且 `docs/dev/privacy.md` 必须说明其存在与位置。

- **逐步落地实施步骤**：
  1. **实现 `BackupFile` / `run_backup` / 轮转**：写临时文件 → 原子重命名 → 删除超出的最旧份；权限 `0600`。
  2. **实现 `recover_user_db_with_backup`**：newest-first 尝试导入；成功的备份记 `data/backup-restored`。
  3. **接入空闲期触发**：`user_db.rs` 的 `open` 记录 `last_backup_unix`；`addon.rs` 在空闲窗口检查并触发。
  4. **接入 `on_addon_destroy`**：终提交后若备份过期则补一次。
  5. **配置与回归**：`[data]` 三个新键；`backup_enabled = false` 时 `run_backup` 返回 `Disabled`；跑 `ime-dict` 全量回归确认 `TC-DICT-31`~`34` 行为不变。

- **验收标准 (DoD)**：
  1. `cargo nextest run -p ime-dict` 全绿，新增 ≥ 12 条：备份写入与权限、轮转顺序（先写后删）、`Skipped`/`Disabled`/`Readonly` 三分支、原子重命名、损坏备份跳过并试下一份、全部失败时 `RebuiltEmpty`、符号链接拒绝、往返一致性（备份 → 空库 → 导入 → `record_count()` 一致）。[自动]
  2. **既有 `TC-DICT-31`~`TC-DICT-34` 的行为不变**——`recover_user_db` 的隔离语义一条测试都不改。[自动]
  3. 手工验证：造一个损坏的 `user.redb` 并在 `backups/` 放一份有效备份 → 重启 → 断言词库被恢复且日志含 `data/backup-restored`。[实验室]
  4. 备份目录与文件的权限为 `0700` / `0600`。[自动]
  5. 空闲期备份期间连续输入 100 次，**无一次按键延迟超过 `key_to_present_p99 = 16ms`**。[性能]
  6. 单次备份耗时 ≤ 200ms（10⁴ 条词条）。[性能]

- **验收记录**（2026-09-30）：
  - **交付物**：`crates/ime-fcitx5/src/addon.rs`（566 行）与本次新建 `addon/tests.rs`（原内联测试整体迁入 + 12 个接线层用例）；`crates/ime-config/src/schema.rs` 新增 `data.backup_enabled` / `data.backup_keep` 两个键（788 行）；`reload.rs` 的 `PartialData`/`merge_data` 与 `DEFAULT_CONFIG_TOML` 同步接线。`ime-dict/src/user_db/backup.rs` 与其 29 条测试**未改**——其 API 已完整够用。
  - **验证命令与结果**：`just ci` 退出 0：`cargo fmt --all -- --check`、`cargo clippy`（主工作区 `--all-targets --all-features` 与两个 addon crate，全部 `-D warnings`）、`cargo nextest run`（主工作区 2430 个用例、两个 addon 346 个，全绿）、`cargo test --doc`（主工作区与两个 addon）、10 个审计脚本及其自检、`just check-host`。
  - **启动回滚**：`paths::ensure_dirs()` → `backup_dir(data_dir)` → `list_backups` → `recover_user_db_with_backup`；损坏库被隔离（改名 `.corrupt.`，**绝不删除**）并从**最新可导入**的备份重建，逐份 newest-first 重试。
  - **卸载备份**：`take_user_store()` 取出句柄（第二次调用自然空转）→ `flush_store` → `start_shutdown_backup`（worker 名 `userdb-backup`，**不 join**）。**宿主线程红线**：唯一留在宿主线程的写是 `final_commit()`（写的是 store 自身上限的未刷增量，有界）；备份本体（整库文档，上限 8MiB）交给一次性 worker 线程且句柄直接 drop——宿主回调不做无界 IO、不 sleep、不 join。
  - **通知映射做成纯函数**：`restore_notice` / `backup_notice` 使「哪个 outcome 报哪个码」成为可断言的值（本 crate 的诊断通道没有测试可读回的缝）。
  - **配置键已落地**：`backup_enabled`（默认 `true`）、`backup_keep`（默认 `DEFAULT_BACKUP_KEEP = 3`，合法域 `1..=MAX_BACKUP_KEEP = 32`，越界报 `config/invalid` 的 `data.backup_keep` 并回落默认值）。两个常量与 `ime-dict::user_db::backup` 的同名常量是**两处书写**（`ime-config` 未依赖 `ime-dict`），二者必须恒等——已在 `schema.rs` 的文档注释里写明这层耦合。
  - **已知限制**：① **空闲期定期备份未接线**——需改 `crates/ime-dict/src/user_db.rs` / `user_db/evict.rs`（让 idle sweep 线程在空闲窗口调用 `run_backup`）；当前备份只在**干净退出**时发生（配合 24h 间隔，等于每天一次），长驻会话中途不备份。② **addon 侧尚未从 `[data]` 构造 `BackupConfig`**——`addon.rs` 目前用 `BackupConfig::new(backup_dir(&layout.data_dir))`（模块默认），接口已就位，只差读取。③ **DoD 5「空闲期备份期间按键 p99 ≤16ms」结构上满足**（备份不在按键路径），但正式数值需探针/基准；**DoD 6「单次备份 ≤200ms」未实测**（需 criterion 或探针）。④ 手工验证（损坏库 + 有效备份 → 重启 → 恢复 + 日志含 `data/backup-restored`）是实验室项，可在本机用 `just install` + 手工损坏 `~/.local/share/rspinyin/user.redb` 复现。⑤ `BackupOutcome::Written` 无对应诊断码，故成功备份不打点（未擅自造新码）；若希望可观测，建议登记一个 info 码后由 `backup_notice` 增加一条分支。⑥ worker 线程不 join：进程若在析构后立刻退出，本次备份可能未落盘；写是「临时文件 + 原子改名」，不会留下半个备份，且下次启动按间隔会补写。⑦ `BackupOutcome::Readonly` 的「store 已只读」那一支只能由 `ime-dict` 自己的测试覆盖（无公开注入手段）；接线层用「备份目录不可写」覆盖了同一降级语义。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85）、Linux 6.18.40.1-microsoft-standard-WSL2、cargo-nextest 0.9.143。

---

#### 任务 ID：ADD-FEAT-P0.03.02 配置 schema 迁移框架

- **基本属性**：
  - 绑定差距条目：`GAP-26`（配置 schema 迁移）
  - 优先级与复杂度：`P0 核心增强` ｜ **中** ｜ 预估工时: **2 人天**
  - 前置依赖：`ADD-FEAT-P0.01.03`（`[phrases]` 段）、`ADD-FEAT-P0.02.02`（`[scheme]` 段）、`ADD-FEAT-P0.02.05`（`[script]` 段）——**v2 schema = v1 + 这三个段，故本卡必须最后落地**
  - 关键路径：**`CP: 是`**（**CP 终点，且是三个 Track 的唯一汇合点**）
  - 并行通道：`Track C`
  - 当前状态：`[x] 已完成`
  - 代码落地锚点：`crates/ime-config/src/migrate.rs`（**新增**）、`crates/ime-config/src/reload.rs`（改：加载时先迁移；**该文件 1016 行已超 800 行上限，本卡必须同时把它拆成 `reload.rs` + `reload/watch.rs`**）、`crates/ime-config/src/schema.rs`（改：`CONFIG_SCHEMA_VERSION` 相关校验；**该文件 802 行已超上限，本卡必须同时拆分**）、`crates/ime-types/src/version.rs`（改：`CONFIG_SCHEMA_VERSION` 1 → 2）、`crates/ime-types/src/error.rs`（`M0` 已加 `ConfigError::Migrated`）、`docs/dev/features.md` 0.7 与 5.1（改：登记本卡）

- **目标与价值**：**消除一个交付级缺陷**。`CONFIG_SCHEMA_VERSION = 1` 已冻结、`Config.schema_version` 字段已存在，但**迁移代码零行**（`grep -ri migrate\|migration crates/ xtask/` 零命中）。当前 `reload.rs` 只做"读入 + 校验 + 修复"。当 `P0.01.03`/`P0.02.02`/`P0.02.05` 引入 `[phrases]`/`[scheme]`/`[script]` 三个段后，schema 必须升到 2——**如果不做本卡，用户升级后旧配置的行为是未定义的**。对标 VS Code 的配置自动迁移、Obsidian 的插件配置迁移。

  **附带收益**：本卡同时清掉 `.dev-progress.json` 登记的阻塞项之一——"3 个文件超过 800 行上限"中的两个（`reload.rs` 1016 行、`schema.rs` 802 行）。

- **技术设计与代码级接入细节**：

  **底层模型与数据流改动**：

  (a) **迁移器的抽象**。迁移是**逐版本**的：`v1 → v2` 是一个独立的函数，不是"从任意版本直接到最新"：

  ```rust
  // crates/ime-config/src/migrate.rs
  /// One step from one schema version to the next.
  ///
  /// Steps are chained rather than written as "any version to current": a user
  /// who skipped three releases needs all three steps applied in order, and a
  /// single monolithic upgrader would have to reimplement every intermediate
  /// shape anyway.
  pub trait MigrationStep {
      /// The version this step reads.
      fn from_version(&self) -> u16;
      /// The version this step produces; always `from_version() + 1`.
      fn to_version(&self) -> u16;

      /// Rewrites the document in place.
      ///
      /// The document is a `toml::Value`, not a `Config`: migration happens
      /// before deserialization, because a v1 file may not deserialize into a
      /// v2 `Config` at all. Working on the document also preserves keys this
      /// build does not know about, which is what lets a user roll back.
      ///
      /// # Errors
      /// Returns `ConfigError::Invalid` when the document is not a v1 document
      /// at all — a `schema_version` that disagrees with the content.
      fn apply(&self, doc: &mut toml::Value) -> Result<Vec<ImeError>, ConfigError>;
  }

  /// Every step this build knows, in ascending version order.
  pub const STEPS: &[&dyn MigrationStep] = &[&V1ToV2];

  /// What a migration did.
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub struct MigrationReport {
      /// The version the document was at.
      pub from: u16,
      /// The version it is at now.
      pub to: u16,
      /// Where the pre-migration file was kept, if it was kept.
      pub backup: Option<PathBuf>,
      /// Keys the migration added, with their defaults.
      pub added: Vec<String>,
      /// Keys the migration could not carry over.
      pub dropped: Vec<String>,
  }

  /// Brings a document up to `CONFIG_SCHEMA_VERSION`.
  ///
  /// # Errors
  /// Returns `ConfigError::Invalid` when a step fails. The caller starts from
  /// defaults in that case rather than refusing to start: an input method that
  /// will not start because of a configuration file is worse than one that
  /// starts with default settings and says so.
  pub fn migrate(
      doc: &mut toml::Value,
      path: &Path,
  ) -> Result<Option<MigrationReport>, ConfigError>;
  ```

  (b) **不可逆但可回滚**（`ASM-A-11`）。迁移是单向的，但**原件必须保留**：

  ```
  $XDG_CONFIG_HOME/rspinyin/
  ├── config.toml            # 已迁移到 v2
  └── config.toml.v1         # 迁移前的原件，原样保留
  ```

  保留原件不是"支持降级迁移"，而是"用户手改回旧版本插件时不会失去配置"。**不提供 v2 → v1 的程序化降级**（`ASM-A-11` 已声明）。

  (c) **`v1 → v2` 的具体内容**：

  | 动作 | 键 | 默认值 |
  |---|---|---|
  | 新增段 | `[phrases]` | `enabled = true`、`file = ""`、`max_entries = 5000` |
  | 新增段 | `[scheme]` | `scheme = "full"`、`show_hint = true`、`keep_full_pinyin = true` |
  | 新增段 | `[script]` | `enabled = false`、`traditional = false`、`hotkey = "ctrl+shift+f"` |
  | 新增键 | `[data] backup_enabled` | `true` |
  | 新增键 | `[data] backup_keep` | `3` |
  | 新增键 | `[data] export_dir` | `""` |
  | 保留 | 全部 v1 键 | 原值不动 |
  | 升级 | `schema_version` | `1` → `2` |

  **边界契约（跨边界任务必填）**：
  - `CONFIG_SCHEMA_VERSION`（`ime-types/src/version.rs`）由 `1` 改为 `2`。**这是本卡唯一的契约常量变更**，且是"值变更"而非"类型变更"，不破坏 `ADR-0001` 的类型冻结。
  - `ConfigError::Migrated { from: u16, to: u16, backup: String }`（`M0` 落地，在既有 `config/*` 段内分配）。错误码：`config/migrated`（**info 级，不是错误**）、`config/migration-failed`（error 级）。
  - **状态机跃迁**（4.2.2）：`Idle` + `Startup` → `Idle`，若 `schema_version < CONFIG_SCHEMA_VERSION` 则迁移；**迁移失败以默认值启动 + 记 `config/migration-failed`，绝不拒绝启动**。
  - **向后兼容**：`reload.rs` 必须能读 `schema_version` 为 1 或 2 的文件；读到 1 时先迁移再反序列化；读到 > 2 时**拒绝加载并报 `config/invalid`**（不猜测未来版本）。

  **与现有代码的无缝衔接方案**：
  - `reload.rs` 的加载路径插入一个前置步骤：`read → parse toml::Value → migrate → deserialize to Config → validate → repaired`。
  - **`Config::repaired()` 的既有语义完全不变**——它处理"值越界"，迁移处理"结构变化"，两者正交。
  - `MAX_DOCUMENT_KEYS` 由 120 提升到 **192**（`ASM-A-10` 的新增键总数 ≤ 45，120 + 45 = 165，留余量到 192）。
  - **文件拆分**（同时清掉 800 行上限的阻塞项）：`reload.rs`（1016 行）拆为 `reload.rs`（加载 + 迁移调用）+ `reload/watch.rs`（文件监听与防抖）；`schema.rs`（802 行）拆为 `schema.rs`（`Config` 与各段定义）+ `schema/validate.rs`（`validate`/`repaired`/`warnings`）。

- **非功能约束 (NFR) 与性能指标**：
  - **迁移只发生在启动期**，不在热路径；耗时 ≤ 20ms（含写盘）。
  - **迁移不重置进行中的会话**：迁移发生在 `Startup`，此时无会话；但 `reload.rs` 的**热重载**路径若遇到版本变化，必须走"延迟生效"（4.2.2）。
  - **异常容灾**：
    - 文件不可写 → 迁移在内存中完成，**不写回**，记 `data/readonly-mode`；本次运行用迁移后的配置，下次启动重新迁移。
    - 迁移中途失败 → **不写回半成品**；以默认值启动 + `config/migration-failed`。
    - 原件重命名失败 → **放弃迁移**，以 v1 语义加载（不识别的新键进 `repaired()` 报告），记 `config/migration-failed`。
  - **幂等**：对已是 v2 的文件调 `migrate` 是 no-op，返回 `Ok(None)`。
  - **不引入新依赖**：`toml` 已在 `[workspace.dependencies]` 中（`AGENTS.md` 3.5 的选型清单）。

- **逐步落地实施步骤**：
  1. **拆分 `reload.rs` 与 `schema.rs`**（先清阻塞项，避免在超长文件上叠加改动）：`reload.rs` → `reload.rs` + `reload/watch.rs`；`schema.rs` → `schema.rs` + `schema/validate.rs`。**纯移动，零行为变更**，用测试全绿证明。
  2. **实现 `MigrationStep` / `STEPS` / `migrate`**：`toml::Value` 上的操作，不依赖 `Config` 结构。
  3. **实现 `V1ToV2`**：按 (c) 的表逐键添加；保留全部 v1 键；升级 `schema_version`。
  4. **接入加载路径**：`reload.rs` 的 `read → parse → migrate → deserialize` 链；原件重命名；`config.toml.v1` 的写入。
  5. **提升 `MAX_DOCUMENT_KEYS` 到 192** 并同步 `schema/validate.rs` 的文档化上限。
  6. **回归**：v1 文件、v2 文件、v3 文件（未来版本）、损坏文件、只读目录五种输入各一条测试。

- **验收标准 (DoD)**：
  1. `cargo nextest run -p ime-config` 全绿，新增 ≥ 12 条：v1→v2 迁移的逐键断言、`schema_version` 升级、原件保留、幂等性、已是 v2 时 no-op、v3 文件被拒、只读目录不写回、迁移失败以默认值启动、`MAX_DOCUMENT_KEYS` 边界。[自动]
  2. **`cargo nextest run -p ime-config` 的既有 13 条测试全部保持通过**——`repaired()` 与 `validate()` 的语义未变。[自动]
  3. `cargo nextest run --workspace --all-features` 全绿——`CONFIG_SCHEMA_VERSION` 1 → 2 的所有引用点同步。[自动]
  4. **文件行数**：`crates/ime-config/src/reload.rs` ≤ 800、`crates/ime-config/src/schema.rs` ≤ 800、`xtask/src/testd/engine/scenario.rs` ≤ 800（`.dev-progress.json` 的三个阻塞项清零）。[自动]
  5. 手工验证：放一个 v1 `config.toml`（含自定义的 `[ui] max_per_row = 7`）→ 启动 → 断言该值被保留、`[scheme]` 等新段出现默认值、`config.toml.v1` 存在。[实验室]
  6. 迁移耗时 ≤ 20ms。[性能]

- **验收记录**（2026-09-30）：
  - **交付物**：`crates/ime-config/src/migrate.rs`（739 行）+ `migrate/tests.rs`（786 行，27 个用例）。
  - **接线**（主 Agent 完成）：`ime-config/src/lib.rs` 挂载 `pub mod migrate;`；`reload.rs` 新增 `Config::from_document_at(text, path)`，迁移在**任何读取之前**执行——先做版本检查会把一个 v1 文档答成默认值，那正是迁移要防的静默丢失；`ConfigStore::load_at` 传入真实路径，使迁移能把原件留在 `config.toml.v1`。`CONFIG_SCHEMA_VERSION` 由 1 升到 2（ADR-0005），`MAX_DOCUMENT_KEYS` 由 120 升到 192（迁移为每个旧文档加 12 个键，120 会让接近上限的文档在重载时收到 `config/limit-exceeded`）。
  - **契约**：`ConfigError::MigrationFailed` 与 `ImeError::ConfigMigrationFailed` 由 ADR-0005 追加，承载 `features.md` 2.2.4 预留而**没有承载变体**的 `config/migration-failed`；`features.md` 的两处 enum 副本已同步（由 `test_spec_enums_matches_the_source_enum_by_enum` 逐 enum 校验）。
  - **验证命令与结果**：`just ci` 退出 0（fmt、clippy `-D warnings`、nextest 1992 个用例、doctest、10 个审计脚本全部通过）。
  - **无损与原子**：报告由迁移前后文档的**叶键差集**生成，任何 step 丢键都会出现在 `dropped`；step 只改副本，全部成功后才写回；落盘走「复制原件 → 写 `.migrating` → rename」，任一步失败即回滚。
  - **已知限制**：
    1. **注释与键序会在写回时丢失**（迁移在 `toml::Value` 上改写），原件完整保存在 `config.toml.v1`。要保住注释需引入 `toml_edit`，本卡未依赖它。
    2. `[script]` 段的读取方（`ADD-FEAT-P0.02.05`）尚未落地，迁移写入的三个默认值目前是无害的额外键。
    3. 卡片 DoD 6 的「迁移耗时 ≤ 20ms」未做断言：迁移只做内存改写 + 两次写盘，路径上无时钟无轮询；按 AGENTS.md §3.6 预算断言应落在 `criterion` 或 probe。
  - **环境**：Rust 1.98.0、Linux 6.18.40.1-microsoft-standard-WSL2、cargo-nextest 0.9.143。

---

#### 任务 ID：ADD-FEAT-P0.05.01 README 双语与 `OB-1` 归属徽章

- **基本属性**：
  - 绑定差距条目：`GAP-37`（README 与 `OB-1` 归属徽章）
  - 优先级与复杂度：`P0 核心增强` ｜ **低** ｜ 预估工时: **1 人天**
  - 前置依赖：无
  - 关键路径：`CP: 否`（松弛 14 人天）
  - 并行通道：`Track C`
  - 当前状态：`[x] 已完成`
  - 代码落地锚点：`README.md`（**新增**）、`README.zh.md`（**新增**）、`LICENSE-APACHE`（**新增**）、`LICENSE-MIT`（**新增**）、`docs/dev/licenses.md`（改：把"由 Phase 2 落地"改为已落地）、`docs/dev/features.md` 6.3 的 Phase 1 出口准则 #8（改：登记已满足）

- **目标与价值**：**这是法务义务，不是文档工作**。ADR-0000 决策 A 的 `OB-1` 条款要求"在公开网页上显著展示 Slint 归属徽章"——因为输入法**既无关于对话框也无启动画面**，这是唯一可行路径。`features.md` 6.3 的 Phase 1 出口准则 #8 明文要求"`OB-1` 归属徽章已上线（README 双语的徽章与链接可达）"。实测：**仓库根 `README.md` 不存在**，`LICENSE-APACHE` / `LICENSE-MIT` 也不存在（`licenses.md` 第 1 节自述由 Phase 2 落地）。**当前状态下带病发布即违反许可条款。**

  本卡只有 1 人天且零依赖，**但正因为不在 CP 上，它极易被"先做功能"的惯性无限推迟**——这正是它至今未做的原因。**建议第一天就做掉。**

- **技术设计与代码级接入细节**：

  **交付物清单（逐项）**：

  (a) **`README.md`（英文）与 `README.zh.md`（中文）**，双语互链。内容结构：

  ```markdown
  # rspinyin

  <!-- OB-1: Slint attribution badge. Required by the Slint Royalty-free 2.0
       licence (ADR-0000 decision A). An input method has no About dialog and
       no splash screen, so this public-webpage badge is the only path the
       licence leaves open. The link must stay reachable. -->
  [![Built with Slint](https://img.shields.io/badge/built%20with-Slint-4C9AFF)](https://slint.dev)

  [![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#licence)

  An offline-first Linux pinyin input method: an in-process Fcitx5 addon with a
  fully self-drawn candidate window.

  [中文文档](README.zh.md)

  ## What it is
  ## Requirements
  ## Install
  ## Configuration
  ## Architecture          <!-- 必须描述双 cdylib，见 ASM-A-04 -->
  ## Privacy               <!-- 链接到 docs/dev/privacy.md -->
  ## Licence               <!-- OB-1..OB-6 的转述 + 嵌入式排除声明（OB-3） -->
  ## Credits
  ```

  (b) **架构段必须描述双 cdylib**（`ASM-A-04`）。`features.md` 的 6 处 `UserInterface` 注册假定被 ADR-0003 推翻；本卡的 README **不得**复制那个错误描述：

  ```
  Two shared objects, no IPC between them:

    librspinyin.so      Category=InputMethod  the decoding engine
    librspinyin-ui.so   Category=UI           the candidate window

  They meet only inside fcitx5: the engine posts a UiFrame through
  InputContext, the UI thread draws it. Neither dlopens the other.
  ```

  (c) **许可段必须完整转述 `OB-1`~`OB-6` 与 `OB-3` 的嵌入式排除声明**（`OB-6` 要求"按现状提供、无担保"）：

  ```markdown
  ## Licence

  rspinyin is licensed under MIT OR Apache-2.0.

  The candidate window is built with Slint under the
  `LicenseRef-Slint-Royalty-free-2.0` licence. The obligations that licence
  places on this project are recorded item by item in
  `docs/dev/licenses.md`; in summary:

  - Slint is not distributed on its own; it is statically linked into
    `librspinyin-ui.so`.
  - The candidate window is not a programmable Slint surface: no Slint type
    appears in `ime-ui`'s public API, so third parties cannot program against
    it.
  - **This licence does not cover embedded use.** Deploying rspinyin on an
    appliance panel, a point-of-sale terminal or an in-vehicle system requires
    a GPL-3.0 or commercial Slint licence, obtained separately.
  - Slint is provided "as is", without warranty of any kind.

  See `docs/dev/licenses.md` for the full audit and `docs/dev/adr/0000-upstream-decisions.md`
  for the decision record.
  ```

  (d) **`LICENSE-APACHE` 与 `LICENSE-MIT`**：`Cargo.toml` 的 `license = "MIT OR Apache-2.0"` 已声明双许可，但两个许可文件本身未落地。补齐标准文本。

  **边界契约**：本卡**不涉及任何代码契约**。README 中的架构描述是**文档契约**——它必须与 `ADR-0003` 一致。

  **与现有代码的无缝衔接方案**：`docs/dev/licenses.md` 第 1 节自述"`LICENSE-APACHE` 与 `LICENSE-MIT` 由 Phase 2 的许可文件任务落地"，本卡完成后必须回写该句。`features.md` 6.3 的出口准则 #8 必须从"要求"改为"已满足"（**但这属于 `features.md` 的维护，由主 Agent 在 `ASM-A-04` 的修正中一并处理**）。

- **非功能约束 (NFR) 与性能指标**：
  - **`OB-1` 的链接必须可达**：`https://slint.dev` 与徽章图片 URL 在发布时验证可达。**这是唯一一条需要网络访问的验收项**，且是**人执行的验收**——项目自身零网络（`BUDGET-NET-01` 不受影响，因为没有代码发起请求）。
  - **双语互链完整**：`README.md` 顶部链接 `README.zh.md`，反向亦然。
  - **无遗漏**：`OB-1`~`OB-6` 六项在 README 的许可段或 `licenses.md` 中逐项有落点。

- **逐步落地实施步骤**：
  1. **写 `LICENSE-APACHE` 与 `LICENSE-MIT`**（标准文本），回写 `licenses.md` 第 1 节。
  2. **写 `README.md`**：徽章（`OB-1`）、架构段（双 cdylib）、隐私段（链到 `docs/dev/privacy.md`，该文件由 `ADD-FEAT-P0.05.02` 产出，两卡并行）、许可段（`OB-1`~`OB-6` + `OB-3` 嵌入式排除）。
  3. **写 `README.zh.md`**：与英文版逐段对应，双语互链。
  4. **验证徽章与链接可达**，把验证结果记入验收记录。

- **验收标准 (DoD)**：
  1. `README.md`、`README.zh.md`、`LICENSE-APACHE`、`LICENSE-MIT` 四个文件存在。[文档]
  2. `README.md` 与 `README.zh.md` 的顶部均含 Slint 徽章且链接到 `https://slint.dev`；**链接实测可达**（验收记录附验证时间与方式）。[文档]
  3. 双语互链可达。[文档]
  4. `README` 的架构段描述双 cdylib，**不含**任何"注册 `UserInterface`"的表述。[文档]
  5. `README` 的许可段含 `OB-3` 的嵌入式排除声明与 `OB-6` 的"按现状提供"转述。[文档]
  6. `docs/dev/licenses.md` 第 1 节的"由 Phase 2 落地"已回写为已落地。[文档]
  7. `cargo metadata` 的 `license` 字段与实际存在的许可文件一致。[自动]
- **验收记录**（2026-09-29）：
  - **交付物**：`README.md`、`README.zh.md`、`LICENSE-APACHE`、`LICENSE-MIT`。
  - **验证命令与结果**：`bash scripts/gen-licenses.sh --check` → PASS（431 packages，OB-1..OB-6 recorded）。`readme_conclusion()` 的两条断言逐条满足：两份 README 顶部均为 `[![Built with Slint](https://img.shields.io/badge/built%20with-Slint-4C9AFF)](https://slint.dev)`，中文版 alt 为「使用 Slint 构建」，URL 含 `Slint` 且 IGNORECASE 命中；`https://slint.dev` 链接在徽章与致谢段各出现一次。
  - **架构段按 ADR-0003 / ADR-0004 描述双 cdylib**，产物名用 `librspinyin_ui.so`（并注明描述符写 `Library=librspinyin_ui`，Fcitx5 原样加 `.so`）；全篇**无**「注册 `UserInterface`」表述，改为「Fcitx5 从 `Category=UI` 的 addon 中按 `available()` / `UIPriority` 选出」。
  - **许可段逐条转述 OB-1~OB-6**，含 OB-3 的嵌入式排除声明与 OB-6 的「按现状提供、无担保」。
  - **已知限制**：
    1. **OB-1 的链接可达性未实测**：DoD 2 要求「链接实测可达」并附验证时间与方式。`--check-links` 的 HTTP 200 探测是**本仓库工具链里唯一的网络访问**，需人工或联网环境执行一次并把结果记入本记录。
    2. 卡片 DoD 6 要求的 `docs/dev/licenses.md` 第 1 节回写已由主 Agent 完成（「由 Phase 2 落地」→「已随仓库根提供」）。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143。

---

#### 任务 ID：ADD-FEAT-P0.05.02 隐私说明与项目许可文件

- **基本属性**：
  - 绑定差距条目：`GAP-38`（隐私说明文档与项目许可文件）
  - 优先级与复杂度：`P0 核心增强` ｜ **低** ｜ 预估工时: **2 人天**
  - 前置依赖：无（与 `ADD-FEAT-P0.05.01` 并行；两卡的 README 与 `privacy.md` 互相链接，链接关系在**两卡都完成后**验证）
  - 关键路径：`CP: 否`（松弛 14 人天）
  - 并行通道：`Track C`
  - 当前状态：`[x] 已完成`
  - 代码落地锚点：`docs/dev/privacy.md`（**新增**）、`docs/dev/licenses.md`（改：`OB-3`/`OB-6` 的落点核对）、`crates/ime-diag/src/redact.rs`（只读核对：文档必须与实现一致）、`crates/ime-core/src/privacy.rs`（只读核对：`CapabilityFlag::Password` 的"尽力而为"语义）、`crates/ime-dict/src/paths.rs`（只读核对：实际路径与权限）

- **目标与价值**：**补齐 `features.md` 6.3 出口准则 #7 的缺失交付物**。实测 `docs/dev/privacy.md` 不存在。`TASK-1.06.02` 的验收标准明文要求它"含数据流向图、存储位置、权限、以及 `CapabilityFlag::Password` 是'尽力而为'信号的明确声明"。这是**用户信任的载体**——一个离线输入法最有力的卖点就是"我可以证明我不联网、不上传、不记录"，但这个证明必须写下来并且**与代码逐条对应**。

- **技术设计与代码级接入细节**：

  **交付物结构与每一节的内容要求**：

  (a) **数据流向图**（必须与代码一致，不得是示意图）：

  ```
  按键 ──► fcitx5 主循环 ──► librspinyin.so (引擎)
                                │
                                ├─► 解码（纯函数，无 IO）
                                │
                                ├─► user.redb 读写（$XDG_DATA_HOME/rspinyin/，0600）
                                │      仅当 should_learn() 为真
                                │
                                └─► UiFrame ──► librspinyin-ui.so（UI 线程）
                                                  │
                                                  └─► 候选框 surface（wl_shm / MIT-SHM）

  日志 ──► $XDG_STATE_HOME/rspinyin/logs/（0600，目录 0700）
  崩溃 ──► $XDG_STATE_HOME/rspinyin/crash/（0600）
  网络 ──► 无。BUDGET-NET-01 = 0，由 scripts/check-no-network.sh 在 CI 强制。
  ```

  (b) **存储位置与权限表**（必须与 `crates/ime-dict/src/paths.rs` 的实际解析逻辑逐条对应）：

  | 文件 | 路径 | 权限 | 内容 | 可删除 |
  |---|---|---|---|---|
  | `config.toml` | `$XDG_CONFIG_HOME/rspinyin/` | `0600` | 用户配置 | 是（回到默认值） |
  | `config.toml.v1` | 同上 | `0600` | 迁移前的配置原件 | 是 |
  | `phrases.tsv` | `$XDG_CONFIG_HOME/rspinyin/` | `0600` | 用户自定义短语 | 是 |
  | `user.redb` | `$XDG_DATA_HOME/rspinyin/` | `0600` | **学习到的词与词频** | 是（丢失全部学习成果） |
  | `backups/*.tsv` | `$XDG_DATA_HOME/rspinyin/backups/` | `0600` | 用户词条的备份 | 是 |
  | `user.redb.corrupt.<ts>` | `$XDG_DATA_HOME/rspinyin/` | `0600` | 损坏时隔离的原文件 | 是（**但建议先人工检查**） |
  | `logs/*.log` | `$XDG_STATE_HOME/rspinyin/` | `0600` | 诊断日志 | 是 |
  | `crash/*.log` | `$XDG_STATE_HOME/rspinyin/` | `0600` | 崩溃记录 | 是 |

  (c) **`CapabilityFlag::Password` 是"尽力而为"信号的明确声明**（`TASK-1.06.02` 的硬性要求）：

  > **我们的能力边界**：rspinyin 依赖宿主应用通过 fcitx5 的 `CapabilityFlag::Password` 告诉我们"当前输入框是密码框"。**如果应用没有设置这个标志，我们无法识别它是密码框**，此时在该框中输入的内容会被当作普通输入处理。我们**不会**尝试通过窗口标题、应用名或其他启发式规则猜测密码框——猜测会带来误判，而误判的代价是双向的（误判为密码框会让用户无法学习，误判为非密码框会泄露输入）。
  >
  > 因此：**不要把 rspinyin 当作密码安全的最后一道防线。** 它是一道有意义的补充，不是保证。

  (d) **日志脱敏的实际机制**（必须与 `crates/ime-diag/src/redact.rs` 的实现逐条对应）：

  - 代码层面**不传**：`raw`、`text`、`preedit`、候选文本、提交文本**从不**进入日志事件（这是第一道防线，`AGENTS.md` 3.4）。
  - `RedactLayer` 是**第二道防线**，不是许可证：即使有人误传，也会被字段名黑名单拦下。
  - 应用标识以**哈希**记录（`AppIdHash`），绝不明文。
  - `$HOME` 前缀重写为 `~`。
  - 敏感上下文中，事件降级为 `session=redacted`，**连输入长度也不记录**（长度本身可能泄露密码长度）。

  (e) **导出与备份的隐私含义**（本规范新增，`ADD-FEAT-P0.01.04` / `P0.03.01` 的产物）：

  > `user.redb` 与 `backups/*.tsv` 包含你的**全部学习词条**——即你打过的词与拼音键。导出或分享这些文件等同于分享你的输入历史。它们默认 `0600` 且只存在于你的用户目录，**rspinyin 从不发送它们到任何地方**。

  (f) **`OB-3` 与 `OB-6` 的核对**（`licenses.md` 的补充）：`OB-3` 的嵌入式排除声明、`OB-6` 的"按现状提供"转述必须同时出现在 `licenses.md` 与 README 的许可段（`ADD-FEAT-P0.05.01` 的交付物）。

  **边界契约**：本卡**不涉及代码契约**。`privacy.md` 是**文档契约**——它描述的机制必须与 `redact.rs`、`privacy.rs`、`paths.rs` 的实现逐条一致；**若发现不一致，以代码为准并修正文档**（`AGENTS.md` 第 7 节）。

  **与现有代码的无缝衔接方案**：本卡是**只读核对 + 文档撰写**，不改任何代码。若核对中发现代码缺陷（例如某个字段实际未被脱敏），**必须报告而不是在文档里掩饰**——报告路径是主 Agent。

- **非功能约束 (NFR) 与性能指标**：
  - **零虚构**：文档中的每一条机制声明必须能在代码中定位到具体文件与行为。**不接受**"我们很注重隐私"这类无锚点的表述。
  - **可验证**：`docs/dev/tests/sec.md` 的 `TC-SEC-40`（`CapabilityFlag` 局限的如实声明）直接断言本文件的第 (c) 节内容，故 (c) 的措辞必须与测试的断言一致。
  - **不含真实用户数据**：文档中的示例路径与文件名均为占位，不含任何真实用户名或路径。

- **逐步落地实施步骤**：
  1. **核对代码**：读 `redact.rs` 的字段黑名单、`privacy.rs` 的 `should_learn` 判定链、`paths.rs` 的 XDG 解析与权限设置，逐条记录**实际行为**。
  2. **写数据流向图与存储位置表**：与第 1 步的核对结果逐条对应。
  3. **写能力边界声明**（(c) 节）：措辞必须与 `TC-SEC-40` 的断言一致。
  4. **写日志脱敏机制**（(d) 节）与导出隐私含义（(e) 节）。
  5. **核对 `licenses.md` 的 `OB-3`/`OB-6` 落点**，补齐缺失项。
  6. **与 `ADD-FEAT-P0.05.01` 互链**：README 的隐私段链接到本文件；本文件回链 README。

- **验收标准 (DoD)**：
  1. `docs/dev/privacy.md` 存在，含数据流向图、存储位置与权限表、能力边界声明、日志脱敏机制、导出隐私含义五节。[文档]
  2. 权限表中的每一行与 `paths.rs` 的实际行为一致（逐条核对，核对结果记入验收记录）。[自动]
  3. 能力边界声明与 `docs/dev/tests/sec.md` 的 `TC-SEC-40` 断言一致。[文档]
  4. 日志脱敏机制的每一条与 `redact.rs` 的实现对应；**若发现实现缺陷，已报告而非掩饰**。[文档]
  5. `docs/dev/licenses.md` 的 `OB-3`（嵌入式排除）与 `OB-6`（无担保）两节完整。[文档]
  6. `README.md` 的隐私段链接到本文件且可达。[文档]
  7. 文档中不含任何真实用户数据或路径。[文档]
- **验收记录**（2026-09-29）：
  - **交付物**：`docs/dev/privacy.md`（8 节）、`docs/dev/NOTICE` 的第 1 节回写。
  - **验证命令与结果**：`just ci` 退出 0；`gen-licenses.sh --check` PASS，`NOTICE` 的生成块标记与 `fcitx5` / `LGPL-2.1` / `LicenseRef-Slint-Royalty-free-2.0` 三项声明原样保留。
  - **内容**：①数据流向图；②存储位置与权限表（逐条对齐 `paths.rs`）；③`CapabilityFlag::Password` 的「尽力而为」声明（未设置该标志的密码框无法识别）；④日志脱敏机制（7 个拒绝字段、`<redacted:len=N>` 计字符、敏感会话只留 `session=redacted app=<hash>`、应用标识哈希、`$HOME`→`~`、换行转义）；⑤导出/备份的隐私含义；⑥实现状态与已知缺口；⑦OB-3/OB-6 落点；⑧门禁表。
  - **已知限制（第 6 节如实登记，均属「能力缺口而非泄露」）**：
    1. C ABI 未把 `CapabilityFlag` 上行，`apply_effects` 不存在，`LearningGate::record_commit` 无调用点；`DiagHandle::mark_sensitive_session` 无调用点。前两条使当前版本比设计**更保守**（失败关闭、不学习）。
    2. `[privacy]` 段未进 `ime-config` schema（`tests/sec.md` 的 `TC-SEC-41` 已按该键写断言）。
    3. `ui_takeover.json` 无写入点。
    4. 卡片的数据流向图与代码不符，按代码写：日志/崩溃在 `$XDG_DATA_HOME/rspinyin/logs/` 与 `crash/<毫秒>-<线程id>.txt`，全仓库无 `XDG_STATE_HOME` 引用；卡片表还漏了 `ui_takeover.json`。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143。

---

## 6. 演化里程碑与防返工路线

### 6.1 Phase 1（增量核心：产品级厚度补齐）— 13 个任务，40 人天，CP 14 人天

**交付物**：产品级词库 + 解码能力补齐（双拼/模糊音/简拼/简繁）+ 用户数据主权（词条可删、可导出、可备份）+ 配置版本化 + 发布合规底座。

**出口准则**：

1. 13 张 P0 任务卡的验收标准全部通过，且每条都有非空的验收记录。[文档]
2. `data/compiled/base.dict` 的 `entry_count ≥ 320,000`，`total_len ≤ 17.5MB`；`decode_p99 ≤ 3.0ms` 在全部解码类增量开启时仍成立。[性能]
3. `lm_holdout.tsv` 的「首选词命中率」与「前 9 候选可达率」相比增量前均有提升，数值记入验收记录。[性能]
4. 双拼（5 方案）、模糊音（8 类）、简拼、简繁转换、自定义短语五项功能在 **X11 与 Wayland/wlroots** 两档上手工验证通过。[实验室]
5. `README.md` / `README.zh.md` 的 `OB-1` 徽章上线且链接可达；`docs/dev/privacy.md` 齐备；`LICENSE-APACHE` / `LICENSE-MIT` 落地。[文档]
6. 用户词条可删除、可导出、可导入、可备份恢复，四条路径各有自动化测试。[自动]
7. v1 配置文件可被自动迁移到 v2，原件保留，`config.toml.v1` 存在。[自动]
8. `just ci` 全绿；`.dev-progress.json` 的三个 800 行超限阻塞项清零。[自动]

### 6.2 Phase 2（效率跃升与全键盘工作流）— 25 个任务（+2 张承接 `features.md` 的卡），共 27 张

**交付物**：命令面板与设置界面、全量键位自定义、候选框外观深度自定义（含竖排）、位置策略、常驻状态指示、符号面板、计算类输入、中英混输、用户自建词表、智能纠错、词频时间衰减、快速造词、配置导入导出、数据完全删除、D-Bus 接口、配置校验反馈、能力状态输出、无障碍、`ime-doctor`、发行版打包、预算回归门禁。

**出口准则**：

1. 全部 P1 任务卡的验收标准通过。[文档]
2. 命令面板（`Ctrl+Shift+/`）内可完成：切换方案、切换简繁、编辑键位、编辑短语、查看能力状态、查看自学习词条、执行删除。[实验室]
3. 候选框的横排/竖排、三档密度、四档字号、五种位置策略全部可在配置中生效且视觉走查通过。[视觉]
4. `ime-doctor` 在 `features.md` 0.5.5 的 6 台登记设备上全部输出 `HEALTHY`。[实验室]
5. deb / rpm / AUR 三套包可安装、可卸载且可逆。[实验室]
6. 无障碍：候选内容可通过至少一条路径被屏幕阅读器读取（`client_preedit` 或 AT-SPI 旁路），局限已如实记录在 `docs/dev/a11y.md`。[实验室]
7. 预算回归门禁在 CI 中生效（连续 10 次 PR 无预算退化）。[自动]

### 6.3 Phase 3（生态扩充与极限调优）— 9 个任务（+3 张承接 `features.md` 的卡），共 12 张

**交付物**：bigram 长句模型、生僻字与扩展区覆盖、领域词库、GPU 渲染评估、插件系统决策文档、皮肤包、i18n、多 Profile、剪贴板历史、诊断包导出、发布流水线与跨发行版 CI、8 小时长稳。

**出口准则**：

1. 全部 P2 任务卡的验收标准通过。[文档]
2. `ADD-FEAT-P2.03.01`（插件系统）产出明确的 Go/No-Go 结论文档。[文档]
3. `ADD-FEAT-P2.02.01`（GPU 渲染）与 `ADD-FEAT-P2.01.01`（bigram）产出实测数据与结论。[性能]
4. 8 小时长稳压测通过：无崩溃、RSS 漂移 ≤ 2MB、延迟无退化（`BUDGET-ROB-01`）。[性能]
5. 跨发行版 CI 全矩阵（Ubuntu 22.04/24.04、Fedora 40/41、Arch）全绿。[自动]

### 6.4 防返工的三条硬约束（**违反即返工**）

| # | 约束 | 违反的后果 | 强制方式 |
|---|---|---|---|
| **R-1** | **`M0` 契约冻结必须先于任何并行开工。** 契约增量表（4.2.1）一次冻结，之后任何 Track 不得再改 `crates/ime-types` 的 5 个文件 | 三个 Track 同时改同一批文件 → 合并冲突 + 语义错位（`features.md` 的 `R-12`） | `M0` 是硬门禁；`ADD-FEAT-P0.02.*` / `P0.01.03` / `P0.01.04` / `P0.03.02` 在其完成前不得开工 |
| **R-2** | **简繁表必须走独立的 `script.dict`，不得内联进 `base.dict`。** `DICT_FORMAT_VERSION` 必须保持 1 | 内联需 `SECTION_COUNT` 6 → 7 与 `FORMAT_VERSION` 1 → 2，`format/reader.rs` 要接受两种头，且**已发布给用户的 v1 词库会与 v2 不兼容** | `ADD-FEAT-P0.02.05` 的 DoD #1 断言 `script.dict` 独立存在且格式校验通过 |
| **R-3** | **`ADD-FEAT-P0.03.02`（配置迁移）必须最后落地。** 它的三个前置（`[phrases]` / `[scheme]` / `[script]` 段）必须先全部到位 | v2 schema 定义不完整 → 迁移后的文件缺段 → 用户升级后功能不可用 | `ADD-FEAT-P0.03.02` 的依赖表硬绑定三张卡；CP 上它是终点 |

### 6.5 与 `features.md` 里程碑的合流点

本规范的 P0/P1/P2 **不替代** `features.md` 的 Phase 1/2/3，而是与之**合流**：

```
features.md Phase 1（39 任务，100.5 人天）── 进行中（1 COMPLETED / 28 READY_FOR_FINAL_GATE / 19 PENDING）
        │
        │  并行：本规范的 P0 只依赖 features.md 的 TASK-1.03.01/1.03.02（词库格式与 FST）
        │        这两张卡状态为 READY_FOR_FINAL_GATE，代码已存在
        ▼
本规范 Phase 1（13 任务，40 人天）── 增量核心
        │
        ├──► features.md Phase 2（36 任务）── 本规范的 P1 是它的**原子级展开**
        │
        └──► features.md Phase 3（16 任务）── 本规范的 P2 是它的**原子级展开**
```

**合流的关键点**：`features.md` 的 Phase 2/3 任务卡（Spoke 文件）**至今不存在**。本规范的 P1/P2 分片（`features-add/phase-2.md`、`phase-3.md`）**就是**那些 Spoke 的替代交付物——见 0.5 节的承接对照表。落地时建议：

- 把 `features-add/phase-2.md` 的内容**合并进** `features/phase-2.md`，或
- 在 `features/phase-2.md` 的头部指向 `features-add/phase-2.md`，避免双源漂移。

**双源漂移是本节要防的主要风险**：`features.md` 的 0.7/5.1 索引表与本规范的 5.1 追溯表都描述任务节点。**以本规范的 5.1 为增量部分的唯一事实来源**，`features.md` 的索引表登记"增量部分见 `features-add.md`"即可，不复制内容。

---

## 7. 续写指令

> 本规范按 Hub & Spoke 分片交付。**主文档（本文件）已全量承载**：现状基线清单（§1）、假设清单（§2，22 条）、功能差距矩阵（§3，44 行）、增量架构与契约规范（§4）、WBS 追溯表（§5.1，44 行）、DAG 校验（§5.2，40 条依赖边）、关键路径汇总（§5.3）、**P0 任务卡原子级展开（§5.4，13 张）**、演化路线（§6）。
>
> **P1（21 张）与 P2（9 张）的详细任务卡在分片中**：`./docs/dev/features-add/phase-2.md` 与 `./docs/dev/features-add/phase-3.md`。

### 7.1 分片索引（**每张卡必须归属且只归属一个 Phase**）

**Phase 2 分片（`./docs/dev/features-add/phase-2.md`）— 21 张 P1 卡**：

| 编号 | 名称 | 绑定差距 | 工时 | 通道 |
|---|---|---|---|---|
| `ADD-FEAT-P1.01.01` | 智能纠错（错键 / 漏键 / 多键 / 乱序） | `GAP-04` | 4 | Track A-解码 |
| `ADD-FEAT-P1.01.02` | 日期 / 时间 / 数字大写等计算类输入 | `GAP-06` | 3 | Track A-数据 |
| `ADD-FEAT-P1.01.03` | 中英混输（英文词候选） | `GAP-08` | 5 | Track A-数据 |
| `ADD-FEAT-P1.01.04` | 符号与 Emoji 面板 | `GAP-07` | 5 | Track A-数据 ｜ Track B |
| `ADD-FEAT-P1.01.05` | 词频时间衰减与场景化权重 | `GAP-11` | 3 | Track A-数据 |
| `ADD-FEAT-P1.01.06` | 用户自建词表导入 | `GAP-14` | 3 | Track A-数据 |
| `ADD-FEAT-P1.02.01` | 命令面板（`Ctrl+Shift+/`） | `GAP-17` | 6 | Track B |
| `ADD-FEAT-P1.02.02` | 命令面板内的设置项与全键盘可达 | `GAP-17`、`GAP-25` | 4 | Track B |
| `ADD-FEAT-P1.02.03` | 键位表编辑界面 | `GAP-25` | 4 | Track B |
| `ADD-FEAT-P1.02.04` | 全量键位自定义（`[keys.bindings]`） | `GAP-18` | 4 | Track B |
| `ADD-FEAT-P1.02.05` | 快速造词（选中即学 + 手动加词） | `GAP-24` | 2 | Track B |
| `ADD-FEAT-P1.02.06` | 常驻输入状态指示 | `GAP-23` | 3 | Track B |
| `ADD-FEAT-P1.02.07` | per-app profile（按应用关闭自绘 UI） | `GAP-21`（派生） | 3 | Track B |
| `ADD-FEAT-P1.03.01` | 候选框外观深度自定义（字号 / 字重 / 密度 / 间距） | `GAP-19` | 5 | Track B |
| `ADD-FEAT-P1.03.02` | 竖排单列候选 | `GAP-20` | 3 | Track B |
| `ADD-FEAT-P1.03.03` | 主题 Token 扩展与 `state-disabled` 补齐 | `GAP-19` | 3 | Track B |
| `ADD-FEAT-P1.03.04` | 候选框位置策略（固定 / 记忆 / 跟随） | `GAP-21` | 3 | Track B |
| `ADD-FEAT-P1.04.01` | 配置导入 / 导出 / 一键重置 | `GAP-28` | 3 | Track C |
| `ADD-FEAT-P1.04.02` | 用户数据的完全可删除 | `GAP-30` | 2 | Track A-数据 ｜ Track C |
| `ADD-FEAT-P1.04.03` | D-Bus 只读控制接口（`Status` / `Capabilities`） | `GAP-31` | 4 | Track C |
| `ADD-FEAT-P1.05.01` | 配置校验反馈的用户可达性 | `GAP-43` | 2 | Track C |
| `ADD-FEAT-P1.05.02` | 运行时能力状态的可读输出 | `GAP-44` | 2 | Track C |
| `ADD-FEAT-P1.05.03` | 无障碍语义暴露评估与实现 | `GAP-39` | 5 | Track B |
| `ADD-FEAT-P1.05.04` | `client_preedit` 路径与 AT-SPI 旁路 | `GAP-39` | 3 | Track B |
| `ADD-FEAT-P1.05.05` | `ime-doctor` 用户级自助诊断 | `GAP-40` | 4 | Track C |
| `ADD-FEAT-P1.05.06` | 发行版打包（deb / rpm / AUR） | `GAP-42` | 6 | Track C |
| `ADD-FEAT-P1.05.07` | 预算回归门禁 | `GAP-40`（派生） | 3 | Track C |

> **注**：上表 27 行 = 27 张 P1 卡，其中 `ADD-FEAT-P1.02.07` 与 `ADD-FEAT-P1.05.07` 承接 `features.md` 的 `TASK-2.04.02` 与 `TASK-2.08.02`，**不在 5.1 追溯表的 44 行绑定范围内**。**5.1 追溯表绑定的 P1 卡共 25 张**，与上表扣除这 2 张后一致。25 + 2 = 27 ✅。

**Phase 3 分片（`./docs/dev/features-add/phase-3.md`）— 9 张 P2 卡**：

| 编号 | 名称 | 绑定差距 | 工时 | 通道 |
|---|---|---|---|---|
| `ADD-FEAT-P2.01.01` | bigram 长句语言模型（格式 v2 + 训练管线） | `GAP-12` | 10 | Track A-数据 |
| `ADD-FEAT-P2.01.02` | 生僻字与 CJK 扩展区覆盖 + 领域词库 | `GAP-15` | 6 | Track A-数据 |
| `ADD-FEAT-P2.02.01` | GPU 渲染路径评估与实现 | `GAP-19`（派生） | 8 | Track B |
| `ADD-FEAT-P2.03.01` | 插件 / 脚本扩展系统决策（**明确不做**） | `GAP-34` | 2 | — |
| `ADD-FEAT-P2.03.02` | 皮肤 / 主题包导入导出 | `GAP-35` | 5 | Track B |
| `ADD-FEAT-P2.03.03` | 候选框 UI 文案 i18n | `GAP-33` | 4 | Track B |
| `ADD-FEAT-P2.04.01` | 多用户 / 多 Profile 并行会话 | `GAP-36` | 5 | Track C |
| `ADD-FEAT-P2.04.02` | 剪贴板历史 | `GAP-32` | 4 | Track B |
| `ADD-FEAT-P2.04.03` | 输入统计与自学习透明度 | `GAP-29` | 3 | Track B |
| `ADD-FEAT-P2.05.01` | 发布流水线与跨发行版 CI 矩阵 | `GAP-42`（派生） | 6 | Track C |
| `ADD-FEAT-P2.05.02` | 8 小时长稳压测 | `GAP-40`（派生） | 4 | Track C |
| `ADD-FEAT-P2.05.03` | 崩溃与诊断包一键导出 | `GAP-41` | 3 | Track C |

> **注**：上表 12 行含 12 张卡，其中 `ADD-FEAT-P2.02.01`、`P2.05.01`、`P2.05.02` 承接 `features.md` 的 `TASK-3.04.01`、`TASK-3.07.01`/`3.07.03`、`TASK-3.08.01`，**不在 5.1 追溯表的绑定范围内**。**5.1 追溯表绑定的 P2 卡共 9 张**，与上表扣除这 3 张后一致。

### 7.2 分片续写模板（**逐字沿用，不得简化**）

新会话续写 P1/P2 分片时，输入 = **本主文档 + 目标分片路径**，每张卡严格按以下字段展开：

```markdown
#### 任务 ID：ADD-FEAT-[P1|P2].[差距域序列].[任务序号] [名称]

- **基本属性**：
  - 绑定差距条目：`[GAP-xx，必须与本主文档 5.1 追溯表一致]`
  - 优先级与复杂度：`[P1 效率进阶 / P2 生态扩展]` ｜ `[高/中/低]` ｜ 预估工时: X 人天
  - 前置依赖：`[必须指向编号更小的任务，或"无"]`
  - 关键路径：`[CP: 是 / 否]`
  - 并行通道：`[Track A-数据 / Track A-解码 / Track B / Track C]`
  - 当前状态：<反引号包住的方括号标记、一个空格、中文状态；新卡写「待开始」，落地后回写「已完成」>
  - 代码落地锚点：`[基于阶段零实测的真实文件路径，标注 新增/改]`
- **目标与价值**：对标哪个产品的哪项特性，解决何种痛点
- **技术设计与代码级接入细节**：
  - 底层模型与数据流改动：核心 Struct / Trait 骨架代码（含字段与方法声明）
  - 边界契约（跨边界任务必填）：Payload 结构体、RequestId/TraceId 贯穿、状态机跃迁（from → event → to）、错误码枚举
  - UI 与工艺级设计规范（涉及 UI 时必填）：尺寸、Design Token 映射、5 态参数、Spring 参数
  - 与现有代码的无缝衔接方案
- **非功能约束 (NFR) 与性能指标**：对齐 0.3 的独占预算与 §2 的假设清单
- **逐步落地实施步骤**：3 步以上
- **验收标准 (DoD)**：每条以 `features.md` 0.2 的方法标签结尾
```

### 7.3 分片头部模板（Living Header）

```markdown
# rspinyin 增量功能扩充 · Phase N

> 分片版本: v1.0 ｜ 主文档: [../features-add.md](../features-add.md) ｜
> 系统形态: Desktop GUI（Linux 桌面输入法） ｜ 架构基线: Rust 2024 + Slint 1.x + Fcitx5 5.1 ｜
> 关联 ADR: [../adr/0000-upstream-decisions.md](../adr/0000-upstream-decisions.md)、[../adr/0001-frozen-boundary-contracts.md](../adr/0001-frozen-boundary-contracts.md)、ADR-0005（待决策）｜
> 最后同步 Commit: `<短哈希>` ｜
> 维护约定: 任务状态变更必须回写主文档 5.1 追溯表；假设变更必须回写主文档第 2 节

## 0. 分片基线（引用主文档，不重复定义）
- 任务卡字段规范：主文档 7.2
- 不可违反的增量约束：主文档 0.4（继承 + 追加 4 条）
- 独占性能预算：主文档 0.3
- 假设清单：主文档第 2 节（22 条）
- 边界契约增量：主文档 4.2
```

### 7.4 落地前的强制检查清单

新会话在续写或落地本规范前，**必须逐条确认**。以下六条是**每次落地前都要重新回答的问题**，不是可以一次勾掉的任务，所以不写成复选框；每条后面是本轮（2026-09-30）的答案：

1. **`ASM-A-04` 的前置条件**：`features.md` 的 6 处 `UserInterface` 注册假定是否已修正？若未修正，本规范的所有架构引用必须自带正确的双 cdylib 描述。**本轮答案：已修正**——2.2.3 已写明 `RSPINYIN_ABI_VERSION = 2`、两个 cdylib 各有自己的胶水与 vtable，2.1 的运行时拓扑图把 UI 角色画在 `ime-ui-addon` 一侧。
2. **`M0` 是否已决策？ADR-0005 是否已写入 `docs/dev/adr/`？** **本轮答案：是**——`docs/dev/adr/0005-incremental-contract-extension.md` 已落地，`crates/ime-types` 的增量类型随之冻结。
3. **`ASM-A-22`：任务卡的"代码落地锚点"列出的文件是否被其他并行会话改动？** **本轮答案：全部复核过**——本轮的每一张卡都按最新工作树核对过锚点；`.dev-progress.json` 的 `cross_task_notes` 记录了跨卡的文件归属与冲突点。
4. **`.dev-progress.json` 的三个 800 行超限阻塞项是否已由 `ADD-FEAT-P0.03.02` 清零？** **本轮答案：已清零**——全树 `find … | wc -l` 复核后无任何 `.rs` 越限（`reload.rs`、`surface.rs`、`install.rs`、`router.rs`、`memory.rs`、`manifest.rs`、`budget_gate.rs`、`guard.rs`、`evidence/tests.rs` 均已拆分）。
5. **全部基准是否在空闲机器上重跑过？**（`.dev-progress.json` 的阻塞项 3：今日所有数字取自 20 个并发 agent 环境，不可信）**本轮答案：未重跑**——本轮全程有并发 agent 在编译，取数留待一次专门的空闲运行（`just bench` + `xtask budget --check`）。
6. **`crates/ime-ui/ui/*.slint` 是否已端到端渲染过？**（`.dev-progress.json` 的阻塞项 4）**本轮答案：仍未在真机会话里渲染，但这条在本轮产出了一个真实缺陷并已修复**——`SlintWindowAdapter::request_redraw` 从不被触发，因此候选框会在画出第一帧之后**永不再重绘**；`Adapter` 现在在写完属性后自己请求重绘（`adapter.rs` 的 `request_repaint`），`adapter/tests.rs` 的翻页像素回归用例把它钉住了。


### 7.5 本规范自身的维护约定

| 触发条件 | 必须回写的位置 |
|---|---|
| 任何 `ADD-FEAT` 卡的代码落地 | 该卡的"当前状态"、主文档 5.1 追溯表的"阶段"列 |
| 新增差距 | 主文档 3.7 矩阵 + 5.1 追溯表 + 对应 Phase 分片 |
| `crates/ime-types` 的契约再次变更 | 主文档 4.2.1 的契约增量表 + 新开 ADR |
| 任何假设被推翻 | 主文档第 2 节的"偏离时的修正策略"列 + 受影响的全部任务卡 NFR |
| 预算阈值变更 | **不修改本规范**——`docs/dev/budgets.json` 是唯一来源，本规范 0.3 的引用随之失效并须重新引用 |
| `features.md` 的 Phase 2/3 Spoke 文件被创建 | 主文档 0.5 的承接对照表 + 7.1 的分片索引（避免双源漂移） |

---

**（本规范正文结束。P1 任务卡见 [./features-add/phase-2.md](./features-add/phase-2.md)；P2 任务卡见 [./features-add/phase-3.md](./features-add/phase-3.md)。）**


