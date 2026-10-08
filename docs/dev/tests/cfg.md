# rspinyin 测试用例分片 · cfg（配置、迁移、热重载、键位与双拼方案）

> 分片版本: v2.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 平台任务: [../features-test.md](../features-test.md) ｜
> 被测基线: Rust 2024 workspace（`ime-config` 全量 + `ime-core` 的 `shuangpin` 接线）+ Fcitx5 5.1.19 ｜ 关联 ADR: [../adr/0005-incremental-contract-extension.md](../adr/0005-incremental-contract-extension.md) ｜ 最后同步 Commit: `408d2c6` ｜
> 维护约定: 新增用例必须回写主文档第 2 节矩阵的 TC 列与维度列

## 0. 分片基线（引用主文档，不重复定义）

- **假设清单**：主文档第 1 节 + [features-test.md](../features-test.md) 第 1 节。强相关：`ASM-A-10`（配置 ≤ 120 键，增量新增 ≤ 45 键）、`ASM-A-11`（迁移单向不可逆，旧文件重命名为 `config.toml.v<N>` 保留）、`ASM-A-14`（键位表上限 64 条）、`ASM-A-19`（`CONFIG_SCHEMA_VERSION` 升为 2，`RSPINYIN_ABI_VERSION` 保持 1）。
- **追踪矩阵**：主文档第 2 节的 `REQ-CFG-01` ~ `REQ-CFG-06`。跨域解码增量（短语/模糊音/简拼/简繁/词条管理/备份）见主文档 `REQ-CORE-08`~`10`、`REQ-DICT-08`~`10` 行，用例在 [`core.md`](core.md) 与 [`dict.md`](dict.md)。
- **迁移语义**（features-add `ADD-FEAT-P0.03.02` 冻结）：迁移**单向且不可逆**；`schema_version` 从 N 升到 N+1 时旧文件重命名保留、新文件写入当前版本；**不提供降级迁移**。
- **热重载语义**（features.md 0.4 规则 10）：重载期间正在进行的 `Composing` 会话**必须原样保留**，不得 reset。
- **写回竞态码**：`config/writeback-race`（`crates/ime-config/src/writeback.rs` 的 `WRITEBACK_RACE_CODE`）是跨边界稳定错误码，**不得改写**。

## 1. 用例

### TC-CFG-01 配置文档解析与逐键 schema 校验（`REQ-CFG-01`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-01` ｜ `cfg` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/schema.rs`、`schema/data.rs`
- **前置条件与沙盒状态**：`FEAT-TEST-P0.05.01` 沙盒 XDG 目录；`config.toml` 不存在（首启状态）。
- **操作步骤**：
  1. 断言首启从 `DEFAULT_CONFIG_TOML`（`reload.rs`）生成模板并成功经 `from_document()` 解析 -> 触发存盘：`<RUN>/cfg/TC-CFG-01/assertions.json`
  2. 写入合法的全键文档（≤ 120 键），断言解析产物的每个键值与文档一致。
  3. 逐键注入 8 类非法值（未知键、越界数值、错误枚举、类型错配、重复键、超长字符串、非法路径、超键数上限），断言每次返回类型化 `ConfigError` 且**修复建议可读**。
- **通过标准**：
  - **功能逻辑**：合法文档零告警；非法键收集为 `Vec<ImeError>` 而非首个即断（`SchemeConfig::validate()` 语义）；键数上限 `MAX_DOCUMENT_KEYS` 命中时报错键名与上限值。
  - **健壮性**：任何非法输入不 panic、不写盘、不阻断加载（降级到默认值 + 诊断）。

### TC-CFG-02 损坏配置的就地修复与 `repaired()` 语义（`REQ-CFG-01`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-01` ｜ `cfg` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/schema/repair.rs`、`schema.rs`（`repaired`）
- **前置条件与沙盒状态**：沙盒目录中预置 5 份半损坏文档（截断、坏枚举、缺段、坏数值、混入注释乱码）。
- **操作步骤**：
  1. 逐份加载并调用 `repaired()` -> 触发存盘：`<RUN>/cfg/TC-CFG-02/assertions.json`
  2. 断言返回 `(Self, Vec<ImeError>)`：修复后的值落在合法域内，告警逐条指明"哪个键、从什么、改成了什么"。
  3. 断言修复结果**确定性**：同一损坏文档修复 100 次逐字节一致。
- **通过标准**：修复永不静默（每处改动都有告警）；修复后的文档回写前先经 `TC-CFG-01` 的全量校验。

### TC-CFG-03 schema 上限与常量一致性（`REQ-CFG-01`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-01` ｜ `cfg` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/schema.rs`（`MAX_DOCUMENT_KEYS`、`CUSTOM_TABLE_KEYS`）
- **前置条件与沙盒状态**：无外部依赖，纯内存。
- **操作步骤**：
  1. 构造 120 键合法文档（含 26 键自定义短语表）通过；121 键被拒 -> 触发存盘：`<RUN>/cfg/TC-CFG-03/assertions.json`
  2. 断言 `CUSTOM_TABLE_KEYS = 26` 与自定义短语表键数一致。
  3. 断言 `features-add.md` `ASM-A-10` 的分段总和（`[scheme]` 8 + `[phrases]` 3 + `[script]` 3 + `[ui]` 6 + `[keys]` 3 + `[data]` 5 + `[diagnostics]` 2 + `[profile]` 15 = 45）不超过增量上限。
- **通过标准**：上限常量只有一个事实来源（schema.rs）；漂移即 `check-versions` 同族的门禁失败。

### TC-CFG-04 配置域的模糊输入属性测试（`REQ-CFG-01`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-01` ｜ `cfg` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/schema.rs`、`migrate.rs`
- **前置条件与沙盒状态**：proptest；无外部依赖。
- **操作步骤**：
  1. 随机 TOML 文档 10⁴ 例（含嵌套、深缩进、Unicode 键、超大数值）-> 触发存盘：`<RUN>/cfg/TC-CFG-04/assertions.json`
  2. 断言三类结局完备（接受 / 类型化错误 / 修复）且无 panic、无 `unwrap` 路径。
- **通过标准**：`check-unsafe` 对 `ime-config` 零命中；失败样本可最小化复现（proptest 收敛）。

### TC-CFG-05 schema 错误的诊断与脱敏（`REQ-CFG-01`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-01` ｜ `cfg` | 全状态防御与骨架屏 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-diag/src/log.rs`、`redact.rs`
- **前置条件与沙盒状态**：日志落盘至沙盒 XDG 目录（0600）。
- **操作步骤**：
  1. 触发一次含非法键的加载与一次失败写回 -> 触发存盘：`<RUN>/cfg/TC-CFG-05/assertions.json`
  2. 断言日志含稳定错误码与键名，**不含**用户输入内容、绝对路径前缀（`$HOME` 改写为 `~`）。
- **通过标准**：`RedactLayer` 兜底零命中（第一道防线是根本不传入）；日志权限 0600。

### TC-CFG-06 v1→v2 迁移：升版本、保旧档、语义不变（`REQ-CFG-02`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-02` ｜ `cfg` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/migrate.rs`（`V1_TO_V2`、`migrate()`、`MigrationReport`）
- **前置条件与沙盒状态**：沙盒目录预置 v1 文档（`schema_version = 1`，含 `[keys]`/`[ui]` 旧段全部合法键）。
- **操作步骤**：
  1. 调用 `migrate()` -> 触发存盘：`<RUN>/cfg/TC-CFG-06/assertions.json`
  2. 断言：旧文件被重命名为 `config.toml.v1` 且**逐字节等于原文件**；新文件 `schema_version = 2`；返回 `Some(MigrationReport)`。
  3. 断言语义保持：v1 文档中的每个用户可观察设置（开关、键位、主题、翻页）在 v2 产物中解析出**相同**的运行时值。
- **通过标准**：`MigrationReport` 逐条记录搬运的键；迁移后文档通过 `TC-CFG-01` 的全量校验；`STEPS` 表只含 `V1_TO_V2`。

### TC-CFG-07 迁移边界：已是 v2、缺版本、损坏与只读目录（`REQ-CFG-02`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-02` ｜ `cfg` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/migrate.rs`
- **前置条件与沙盒状态**：4 份边界文档：`schema_version = 2`、缺失版本键、`schema_version = 99`、损坏 TOML；外加一个只读目录。
- **操作步骤**：
  1. 逐份调用 `migrate()` -> 触发存盘：`<RUN>/cfg/TC-CFG-07/assertions.json`
  2. 断言：v2 原样返回 `None`（无迁移、无重命名）；缺版本按最新版本处理并告警；未知高版本返回类型化错误（**绝不**降级解析）；损坏文档不迁移、不动原文件。
  3. 只读目录下迁移失败时原文件完好且错误可读。
- **通过标准**：迁移是纯文档操作，**不触发**热重载回调（`REQ-CFG-03` 的会话不受影响）；任何失败路径都保留用户原文件（`ASM-A-11` 单向不可逆）。

### TC-CFG-08 迁移确定性：同输入 100 次逐字节一致（`REQ-CFG-02`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-02` ｜ `cfg` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/migrate.rs`
- **前置条件与沙盒状态**：纯单测；固定 v1 样本集 20 份。
- **操作步骤**：
  1. 每份样本迁移 100 次，断言 `MigrationReport` 与产物文档逐字节一致 -> 触发存盘：`<RUN>/cfg/TC-CFG-08/assertions.json`
  2. 断言迁移不依赖时钟、环境变量、迭代顺序（`HashMap` 遍历处有排序）。
- **通过标准**：迁移是纯函数（0.4 规则 4 对数据路径同样成立）。

### TC-CFG-09 迁移不 reset 进行中的输入会话（`REQ-CFG-02`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-02` ｜ `cfg` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/migrate.rs`、`crates/ime-fcitx5/src/engine.rs`
- **前置条件与沙盒状态**：`CommitProbe` 客户端联机；v1 配置就位。
- **操作步骤**：
  1. 输入 `ni` 进入 Composing -> 触发存盘：`<RUN>/cfg/TC-CFG-09/01_composing.png`
  2. 触发迁移（首次加载 v1 文档）-> 触发存盘：`<RUN>/cfg/TC-CFG-09/02_migrated.png`
  3. 继续输入 `hao` + 空格，断言上屏"你好"、会话未断。
- **通过标准**：features.md 0.4 规则 10 同时约束迁移与热重载；`UiFrame.revision` 连续。

### TC-CFG-10 迁移报告的审计可读性（`REQ-CFG-02`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-02` ｜ `cfg` | 全状态防御与骨架屏 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/migrate.rs`（`MigrationReport`）
- **前置条件与沙盒状态**：纯单测。
- **操作步骤**：
  1. 迁移一份含全部可迁移键的 v1 样本 -> 触发存盘：`<RUN>/cfg/TC-CFG-10/assertions.json`
  2. 断言报告逐条含：原键路径、新键路径、原值、新值；丢弃键有显式理由。
- **通过标准**：报告是结构化数据（可被 `xtask` 消化），不是拼接字符串；无"等等"式省略。

### TC-CFG-11 配置热重载全链路（写盘 → 重载 → 生效）（`REQ-CFG-03`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-03` ｜ `cfg` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/reload.rs`、`reload/load.rs`
- **前置条件与沙盒状态**：沙盒 XDG 目录 + 真实 `config.toml`；插件经 `CommitProbe` 客户端联机（`FEAT-TEST-P0.02.02`）。
- **操作步骤**：
  1. 记录初始生效值（如主题色、翻页键、候选数）-> 触发存盘：`<RUN>/cfg/TC-CFG-11/01_default.png`
  2. 编辑 `config.toml`（改主题与翻页键）保存 -> 触发存盘：`<RUN>/cfg/TC-CFG-11/02_reloaded.png`
  3. 输入 `nihao` 触发候选框，断言新主题与新翻页键**已生效**且无需重启 fcitx5 -> 触发存盘：`<RUN>/cfg/TC-CFG-11/03_after.png`
- **通过标准**：
  - **功能逻辑**：重载原子生效（无半旧半新状态）；重载失败（写坏文档）时沿用旧配置并记 `warn` 级诊断，**绝不**落入空配置。
  - **诊断**：日志记录重载版本号与变更键计数，不含任何输入内容。

### TC-CFG-12 热重载不 reset 正在进行的输入会话（`REQ-CFG-03`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-03` ｜ `cfg` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/reload.rs`、`crates/ime-fcitx5/src/engine.rs`（会话保持）
- **前置条件与沙盒状态**：同 `TC-CFG-11`；`CommitProbe` 客户端焦点就绪。
- **操作步骤**：
  1. 输入 `ni`（Composing 中，候选框可见）-> 触发存盘：`<RUN>/cfg/TC-CFG-12/01_composing.png`
  2. **在 Composing 期间**改写 `config.toml` 触发热重载 -> 触发存盘：`<RUN>/cfg/TC-CFG-12/02_reload-mid-compose.png`
  3. 继续输入 `hao` + 空格 -> 触发存盘：`<RUN>/cfg/TC-CFG-12/03_committed.png`
  4. 断言上屏"你好"，preedit 与候选全程未闪断（`UiFrame` 快照 `revision` 连续）。
- **通过标准**：features.md 0.4 规则 10——重载**不得** reset `Composing` 会话；断言缓冲内容、caret 位置、候选页码在重载前后逐项不变。

### TC-CFG-13 重载风暴与竞态：连续 20 次快速写盘（`REQ-CFG-03`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-03` ｜ `cfg` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/reload/store.rs`
- **前置条件与沙盒状态**：沙盒目录；无显示服务器依赖（store 层单测 + 实机各一轮）。
- **操作步骤**：
  1. 200ms 内连续 20 次写盘（内容交替合法/损坏）-> 触发存盘：`<RUN>/cfg/TC-CFG-13/assertions.json`
  2. 断言最终状态等于最后一次合法文档；中间的损坏文档各自产生一条告警但**不污染**最终配置。
  3. 断言重载回调不重入（同一次文件事件只触发一次重载）。
- **通过标准**：合并语义是"最后一次合法写入获胜"；全程无死锁、无泄漏的 watch 句柄（`/proc/<pid>/fd` 计数回到基线）。

### TC-CFG-14 重载期间的版本一致性（conf ↔ Cargo.toml）（`REQ-CFG-03`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-03` ｜ `cfg` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `xtask/src/versions.rs`、`crates/ime-config/src/reload.rs`
- **前置条件与沙盒状态**：纯单测。
- **操作步骤**：
  1. 断言配置 schema 版本常量与 `Cargo.toml` 版本声明一致（`just check-versions` 等价断言）-> 触发存盘：`<RUN>/cfg/TC-CFG-14/assertions.json`
  2. 人为漂移版本号，断言 `xtask check-versions` 非零退出且报文可定位。
- **通过标准**：版本一致性是门禁而非运行时行为；热重载路径不读取版本号以外的元数据。

### TC-CFG-15 配置加载与重载的耗时上限（`REQ-CFG-03`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-03` ｜ `cfg` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/reload/load.rs`
- **前置条件与沙盒状态**：空闲机器（`ASM-T-11`）；120 键满配文档。
- **操作步骤**：
  1. 冷加载 100 次取 P99 -> 触发存盘：`<RUN>/cfg/TC-CFG-15/assertions.json`
  2. 断言 P99 不落入任何解码路径预算（加载发生在会话建立期，不计入 `key_to_present`）；重载回调本身 ≤ 200µs 单次。
- **通过标准**：重载期间宿主线程零阻塞（0.4 规则 10 的可测量等价断言）；数值纳入 `probe` 看板（`REQ-DIAG-06`）。

### TC-CFG-16 键位全量自定义：64 条上限内逐条生效（`REQ-CFG-04`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-04` ｜ `cfg` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/keymap.rs`、`crates/ime-fcitx5/tests/keymap_matrix/`
- **前置条件与沙盒状态**：`keymap_matrix` 场景驱动（`scenarios.rs`）；沙盒配置。
- **操作步骤**：
  1. 在配置中重定义全部可自定义动作（含翻页、候选选择、中英切换、临时英文、简繁切换）-> 触发存盘：`<RUN>/cfg/TC-CFG-16/01_default.png`
  2. 经 `keymap_matrix` 逐动作注入改后键位，断言每个动作触发对应 `KeyAction` -> 触发存盘：`<RUN>/cfg/TC-CFG-16/02_rebound.png`
  3. 恢复默认键位，断言默认行为不变。
- **通过标准**：动作集 = `KeyAction` 全部变体；键位上限 64 条（`ASM-A-14`）内逐条生效；改绑**不破坏**被 fcitx5 占用的修饰键语义（`TC-RT-07` 绝不吞键仍绿）。

### TC-CFG-17 键位冲突与上限校验（`REQ-CFG-04`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-04` ｜ `cfg` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/keymap.rs`
- **前置条件与沙盒状态**：纯单测 + 沙盒配置。
- **操作步骤**：
  1. 配置两条同键不同动作 -> 触发存盘：`<RUN>/cfg/TC-CFG-17/assertions.json`
  2. 断言加载时返回类型化错误且指明冲突的两条绑定；第 65 条绑定被拒。
  3. 断言非法键名（不存在的 keysym 字符串）被拒并回显键名（不含输入内容）。
- **通过标准**：冲突键位**绝不**以"后写胜"静默生效；错误信息给出建议（保留其一或删除）。

### TC-CFG-18 分层按键投影与斜杠弦的会话态路由（`REQ-CFG-04`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-04` ｜ `cfg` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/keymap/project.rs`、`crates/ime-fcitx5/src/engine/sequence.rs`
- **前置条件与沙盒状态**：`keymap_matrix` 场景驱动；会话态覆盖 `Empty`/`Composing`/`TempEnglish`。
- **操作步骤**：
  1. 三个会话态下各注入 `/` 弦 -> 触发存盘：`<RUN>/cfg/TC-CFG-18/assertions.json`
  2. 断言路由随会话态变化（Composing 中触发候选页行为/标点行为，空会话态走直通），与 `ed5ca60` 落地的会话态路由一致。
  3. 断言投影层不改写 `KeyAction` 语义，只改触发键（0.4 规则 2 的 UI 侧无按键语义）。
- **通过标准**：分层派发顺序确定（会话态 → 投影 → `KeyAction`）；无按键被两处同时消费。

### TC-CFG-19 键位速查表窗口与配置一致（`REQ-CFG-04`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-04` ｜ `cfg` | 商业化 5 态微交互与材质 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-fcitx5/src/cheatsheet.rs`
- **前置条件与沙盒状态**：X11 档联机；`FEAT-TEST-P0.01.02` 截图通道。
- **操作步骤**：
  1. 触发速查表 -> 触发存盘：`<RUN>/cfg/TC-CFG-19/01_default.png`
  2. 改绑两个键位后重新触发 -> 触发存盘：`<RUN>/cfg/TC-CFG-19/02_rebound.png`
  3. 断言速查表渲染的是**改后**键位；窗口永不夺焦（`check-no-grab` 语义 + `xcb_get_input_focus` 不变断言）。
- **通过标准**：速查表内容与 `DEFAULT_CONFIG_TOML` 的键位段逐键一致（`check-readme-keys.sh` 同族断言）；关闭后无残留定时器。

### TC-CFG-20 键位改绑的全键盘矩阵闭环（`REQ-CFG-04`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-04` ｜ `cfg` | 人机工学与全键盘流 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-fcitx5/tests/keymap_matrix/main.rs`
- **前置条件与沙盒状态**：`keymap_matrix` 矩阵场景；XTEST 注入通道。
- **操作步骤**：
  1. 矩阵遍历：每个 `KeyAction` × 改绑键位 × 三个会话态 -> 触发存盘：`<RUN>/cfg/TC-CFG-20/assertions.json`
  2. 断言全程无鼠标可达：改绑、翻页、选词、中英切换、简繁切换均可纯键盘完成。
- **通过标准**：矩阵无死格（每个动作至少一条键位在全部会话态可达）；`is_release` 事件一律不消费。

### TC-CFG-21 双拼方案：5 方案音节映射全表（`REQ-CFG-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-05` ｜ `cfg` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/scheme.rs`（`SchemeChoice`）、`crates/ime-core/src/shuangpin/`
- **前置条件与沙盒状态**：纯单测；音节表来自 `ime-core` 冻结数据。
- **操作步骤**：
  1. 对 `xiaohe`/`ziranma`/`microsoft`/`sogou`/`ziguang` 五方案逐一生成全音节映射 -> 触发存盘：`<RUN>/cfg/TC-CFG-21/assertions.json`
  2. 每方案断言：合法音节全表可编码为 ≤ 2 键并可逆解码；零声母/standalone 音节行为与方案官方表一致。
  3. 断言全表确定性：两次生成逐字节一致。
- **通过标准**：五方案覆盖 `SchemeChoice::is_double_pinyin()` 的全部取值；映射表是纯函数（无时钟/环境依赖，0.4 规则 4）。

### TC-CFG-22 双拼切换与混输兜底（`REQ-CFG-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-05` ｜ `cfg` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/shuangpin/fallback.rs`、`crates/ime-config/src/scheme.rs`（`decode_settings`）
- **前置条件与沙盒状态**：沙盒配置，方案设为小鹤双拼。
- **操作步骤**：
  1. 双拼模式下输入全拼串 `nihao` -> 触发存盘：`<RUN>/cfg/TC-CFG-22/01_default.png`
  2. 断言兜底层产出合理候选（`fallback` 语义）而非拒绝输入 -> 触发存盘：`<RUN>/cfg/TC-CFG-22/02_fallback.png`
  3. 运行中切换方案（全拼 ↔ 双拼），断言切换即时生效且不 reset 进行中会话。
- **通过标准**：`decode_settings()` 的方案位与实际解码路径一致；兜底候选排序确定性可复现（Q8.8 定点）。

### TC-CFG-23 自定义双拼方案表的校验（`REQ-CFG-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-05` ｜ `cfg` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/scheme.rs`（`CustomSchemeConfig::validate`）
- **前置条件与沙盒状态**：纯单测。
- **操作步骤**：
  1. 构造自定义方案：合法表通过；重复声母映射、缺韵母覆盖、非法键名三类错误各注入一次 -> 触发存盘：`<RUN>/cfg/TC-CFG-23/assertions.json`
  2. 断言 `validate()` 返回的 `Vec<ImeError>` 逐条指明行列与原因。
- **通过标准**：自定义表与内置方案走同一解码路径；校验失败时方案回退到全拼并告警。

### TC-CFG-24 双拼 × 简拼 × 模糊音的组合边界（`REQ-CFG-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-05` ｜ `cfg` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/shuangpin/`、`fuzzy.rs`、`viterbi/lattice/abbrev.rs`
- **前置条件与沙盒状态**：纯单测；三能力同时开启。
- **操作步骤**：
  1. 双拼键位下注入简拼缩写与模糊对输入 -> 触发存盘：`<RUN>/cfg/TC-CFG-24/assertions.json`
  2. 断言三者组合不产生歧义爆炸（`ASM-A-06` 每音节 ≤ 8 替代拼写仍成立）且 `decode_p99` 达预算。
- **通过标准**：组合确定性可复现；`ASM-A-02` 的增量预算裁决未被突破。

### TC-CFG-25 双拼方案的诊断与降级（`REQ-CFG-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-05` ｜ `cfg` | 全状态防御与骨架屏 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/scheme.rs`、`crates/ime-diag/src/redact.rs`
- **前置条件与沙盒状态**：沙盒配置；日志落盘。
- **操作步骤**：
  1. 配置指向不存在的方案名 -> 触发存盘：`<RUN>/cfg/TC-CFG-25/assertions.json`
  2. 断言加载降级为全拼、记 `warn`（含稳定错误码）、用户可继续输入不受阻。
- **通过标准**：降级不静默；日志无输入内容。

### TC-CFG-26 配置写回：字段级原子写与竞态码（`REQ-CFG-06`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-06` ｜ `cfg` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/writeback.rs`（`write_keys`、`WRITEBACK_RACE_CODE`）
- **前置条件与沙盒状态**：沙盒目录 + 现存 `config.toml`。
- **操作步骤**：
  1. 调用 `write_keys()` 更新两个键 -> 触发存盘：`<RUN>/cfg/TC-CFG-26/assertions.json`
  2. 断言：文件经临时文件 + `fsync` + 原子 rename 落盘（`check-fsync-rename.sh` 语义）；注释与未触碰键**逐字节保留**。
  3. 注入外部并发改动后写回，断言命中 `config/writeback-race` 稳定错误码而非覆盖他人改动。
- **通过标准**：错误码字符串与冻结清单一致；写回路径不重排用户文档键序。

### TC-CFG-27 写回边界：只读目录与磁盘满（`REQ-CFG-06`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-06` ｜ `cfg` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/writeback.rs`（`WritebackError`）
- **前置条件与沙盒状态**：`chmod 0500` 的沙盒目录 + tmpfs 满盘注入。
- **操作步骤**：
  1. 只读目录下写回 -> 触发存盘：`<RUN>/cfg/TC-CFG-27/assertions.json`
  2. 断言返回 `WritebackError` 且 `.tmp` 残留被清理（`534e48a` 的 DictWriter 语义同族）；原文件完好。
  3. 满盘注入同断言。
- **通过标准**：写回失败**绝不**阻断输入（降级为内存态 + 告警）；不留部分写入的文件。

### TC-CFG-28 首启模板与 README 键位样例一致（`REQ-CFG-06`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-06` ｜ `cfg` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/reload.rs`（`DEFAULT_CONFIG_TOML`）、`scripts/check-readme-keys.sh`
- **前置条件与沙盒状态**：仓库工作树干净。
- **操作步骤**：
  1. 断言 `DEFAULT_CONFIG_TOML` 可解析且通过全量校验 -> 触发存盘：`<RUN>/cfg/TC-CFG-28/assertions.json`
  2. 运行 `scripts/check-readme-keys.sh`，断言 README 的 ```toml 样例与首启模板键对键相等。
- **通过标准**：模板是唯一事实来源；漂移即门禁失败（脚本非零退出）。

### TC-CFG-29 配置到渲染的 ThemeSpec 通道（`REQ-CFG-06`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-06` ｜ `cfg` | 商业化 5 态微交互与材质 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-ui/src/theme.rs`、`ui/theme.slint`
- **前置条件与沙盒状态**：`MockBackend`；`FEAT-TEST-P0.02.04` 度量通道。
- **操作步骤**：
  1. 改配置主题段 → 断言 `ThemeSpec` 更新 → 组件只改属性不重建（20 次连续切换）-> 触发存盘：`<RUN>/cfg/TC-CFG-29/assertions.json`
  2. 断言 18 颜色 Token 全量透传（与 `TC-UI-21` 同表）。
- **通过标准**：`apply()` ≤ 100µs；配置→渲染链路无中间缓存漂移（单一事实来源）。

### TC-CFG-30 写回与热重载的联动闭环（`REQ-CFG-06`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-CFG-06` ｜ `cfg` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-config/src/writeback.rs`、`reload.rs`
- **前置条件与沙盒状态**：沙盒 XDG 目录；watch 就绪。
- **操作步骤**：
  1. 程序自身 `write_keys()` 写回一个键 -> 触发存盘：`<RUN>/cfg/TC-CFG-30/assertions.json`
  2. 断言自身的 watch 收到事件后重载，且不会把**自己**的写回误判为外部竞态（`config/writeback-race` 不触发）。
  3. 外部编辑器并发写回时竞态码正常触发。
- **通过标准**：写回→重载闭环幂等（重复 10 次状态收敛）；重载计数与写回计数 1:1（无重入放大）。

## 2. 分片出口准则

- 全部 30 条用例的"基本属性"均已回填主文档矩阵 `REQ-CFG-01`~`06` 行。
- `P0` 用例（`TC-CFG-01`、`TC-CFG-06`、`TC-CFG-11`、`TC-CFG-12`、`TC-CFG-16`、`TC-CFG-21`、`TC-CFG-26`）在每次发布前必须全绿；任何一条红即阻塞发布。
- 本分片不含本机不可验证项（配置域全部可离线/沙盒判定）；若后续新增依赖显示服务器的能力，必须按 `FEAT-TEST-P0.05.05` 显式标注。
