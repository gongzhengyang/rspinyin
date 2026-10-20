# rspinyin 测试用例分片 · cfg（配置系统）

> 分片版本: v3.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 执行器必读: [../test-architecture.md](../test-architecture.md) ｜ 模块形态: Headless ｜ 逻辑自检: 已通过 ｜ 维护约定: 用例状态变更须同步主文档矩阵
>
> 基线引用：`ASM-T-01/04/05`。模块背书 = `cargo nextest -p ime-config`（117 测）+ `tests/keymap_matrix/`。**吸收** 字段登记取代的 v2.0 用例号。

---

#### TC-CFG-001 配置 schema 校验与修复

- **基本属性**：`REQ-CFG-01` ｜ `ASM-T-07` ｜ cfg ｜ ①② ｜ nextest(-p ime-config) ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `requires_screenshots: false` ｜ `[]` ｜ 吸收：TC-CFG-01~05 ｜ 锚点：`crates/ime-config/src/schema.rs`、`schema/{data,repair}.rs:9-16` ｜ 背书：schema/repair 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：坏值矩阵 fixture（逐键）。
- **操作步骤与物理存盘动作**：
  1. 逐键 schema 校验与 `repaired()` 语义 → `execution_log.txt`。
  2. 上限与常量一致性；模糊输入属性测试 → 记录。
  3. 每键一条 `config/invalid` 诊断且脱敏 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：绝不阻断启动；闭合枚举 `closed_set!` 宏覆盖。
  - **[Headless 断言]**：诊断不带文件路径（用户面只给错误码）。
  - **[性能断言]**：全量修复在启动预算内（`addon_load` 份额）。

#### TC-CFG-002 v1→v2 单向迁移

- **基本属性**：`REQ-CFG-02` ｜ `ASM-T-07` ｜ cfg ｜ ①② ｜ nextest ｜ `[Acceptance 验收套件]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-CFG-06~10 ｜ 锚点：`crates/ime-config/src/migrate.rs`（749 行）｜ 背书：migrate 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：v1 配置 fixture；只读目录档。
- **操作步骤与物理存盘动作**：
  1. 升版本、保旧档（原文件保留）、语义不变 → `execution_log.txt`。
  2. 边界：已是 v2 / 缺版本 / 损坏 / 只读目录 → 记录。
  3. 确定性（同输入 100 次逐字节一致）；迁移报告可审计 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：迁移不 reset 进行中的输入会话（0.4 规则 10）。
  - **[Headless 断言]**：报告人类可读且含逐键处置。
  - **[性能断言]**：迁移一次性，不进热路径。

#### TC-CFG-003 配置热重载

- **基本属性**：`REQ-CFG-03` ｜ `ASM-T-01` ｜ cfg ｜ ①② ｜ nextest + 沙盒 ｜ `[Acceptance 验收套件]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-CFG-11~15 ｜ 锚点：`crates/ime-config/src/reload.rs`、`reload/{load,store}.rs` ｜ 背书：reload 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：沙盒会话；写盘触发器。
- **操作步骤与物理存盘动作**：
  1. 写盘→重载→生效全链路（`ConfigStore` Arc 替换）→ `execution_log.txt`。
  2. 热重载不 reset 会话；重载风暴 20 次快速写盘竞态 → 记录。
  3. reload 生效 vs restart 生效键的三态标注一致性；耗时上限 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：组合中的会话保旧配置；无文件 watcher 是既定设计（reload 驱动）。
  - **[Headless 断言]**：版本一致性在重载路径保持（`check-versions` 口径）。
  - **[性能断言]**：单次重载 ≤ 既定上限（背书计时）。

#### TC-CFG-004 键位自定义与速查表

- **基本属性**：`REQ-CFG-04` ｜ `ASM-T-01/10` ｜ cfg ｜ ①④⑤ ｜ nextest + `keymap_matrix` + 沙盒 ｜ `[Acceptance 验收套件]` ｜ P0 ｜ `true` ｜ `["01_cheatsheet.png"]` ｜ 吸收：TC-CFG-16~20 ｜ 锚点：`crates/ime-config/src/keymap.rs`、`keymap/project.rs`、`crates/ime-fcitx5/src/cheatsheet.rs` ｜ 背书：keymap 套件 + 2,568 行矩阵
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：64 条改绑 fixture；冲突矩阵。
- **操作步骤与物理存盘动作**：
  1. 64 条上限内逐条生效 → `execution_log.txt`。
  2. 冲突仲裁（高亮赢/翻页让）与上限校验 → 记录。
  3. 分层投影（context/dispatcher/overlay/session）与斜杠弦路由；速查表与配置一致 → 存 `01_cheatsheet.png` + `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`KeyName` 白名单与 `binding_audit` 双向穷尽。
  - **[视觉断言]**：CheatSheet overlay 渲染与键位表逐行一致。
  - **[性能断言]**：投影在会话创建期一次完成。

#### TC-CFG-005 双拼方案引擎

- **基本属性**：`REQ-CFG-05` ｜ `ASM-T-10` ｜ cfg ｜ ①② ｜ nextest + 场景 ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-CFG-21~25 ｜ 锚点：`crates/ime-config/src/scheme.rs`、`crates/ime-core/src/shuangpin/`（五套表 + custom）｜ 背书：scheme/shuangpin 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：五方案 fixture + custom 表矩阵。
- **操作步骤与物理存盘动作**：
  1. 5 方案音节映射全表 → `execution_log.txt`。
  2. 切换与 `keep_full_pinyin` 混输兜底；custom 表校验 → 记录。
  3. 双拼 × 简拼 × 模糊音组合边界；非法方案诊断降级 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`;` 列不可达为登记在案的已知限制（`shuangpin/mod.rs:36-39`）。
  - **[Headless 断言]**：`DecodeFlags::SHUANGPIN` 位语义（machine.rs:454 注入点）。
  - **[性能断言]**：映射 ≤ 独占预算 0.15ms（features-add §3.4 口径）。

#### TC-CFG-006 配置写回与首启模板

- **基本属性**：`REQ-CFG-06` ｜ `ASM-T-06` ｜ cfg ｜ ①②③ ｜ nextest ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-CFG-26~30 ｜ 锚点：`crates/ime-config/src/writeback.rs`、`reload.rs`（`DEFAULT_CONFIG_TOML:87-222`）｜ 背书：writeback 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：临时 XDG；磁盘满注入。
- **操作步骤与物理存盘动作**：
  1. 字段级原子写与竞态码 → `execution_log.txt`。
  2. 只读目录与磁盘满边界 → 记录。
  3. 首启模板与 `Config::default` 防漂移（测试保证）；配置→`ThemeSpec` 渲染通道；写回×热重载联动 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：模板含逐键注释与三态标注；与 README 键位样例一致。
  - **[Headless 断言]**：`ThemeSpec` 通道零 Slint 类型泄漏（OB-4 联动）。
  - **[性能断言]**：写回原子（fsync-rename 纪律，`check` 审计联动）。
