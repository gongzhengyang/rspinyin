# rspinyin 测试用例分片 · dict（词库与用户数据）

> 分片版本: v3.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 执行器必读: [../test-architecture.md](../test-architecture.md) ｜ 模块形态: Headless ｜ 逻辑自检: 已通过 ｜ 维护约定: 用例状态变更须同步主文档矩阵
>
> 基线引用：`ASM-T-06/07/08`；阈值只引 `budgets.json` 键名。模块面判定 = `cargo nextest -p ime-dict`（257 测）+ `cargo nextest -p xtask`（dictc 侧）+ `testd/dict_inject` fixture。**吸收** 字段登记取代的 v2.0 用例号。

---

#### TC-DICT-001 容器格式：往返、畸形拒止与原子替换

- **基本属性**：`REQ-DICT-01` ｜ `ASM-T-07` ｜ dict ｜ ①② ｜ nextest ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `requires_screenshots: false` ｜ `[]` ｜ 吸收：TC-DICT-01~05 ｜ 锚点：`crates/ime-dict/src/format/{mod,reader,writer}.rs` ｜ 背书：format 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：临时目录；`dict_inject::Synthetic`。
- **操作步骤与物理存盘动作**：
  1. 写→读往返逐字节一致（6 段 + 头 CRC）→ `execution_log.txt`。
  2. 六种畸形输入（坏 magic/坏版本/坏段表/坏 CRC/越界 offset/截断）→ 类型化错误 → 记录。
  3. 原子替换中断模拟（任何时刻目标完整）与磁盘满（清理 `.tmp`）→ `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`v1 无 BIGRAM 段` 不变量；损坏走 quarantine 不删除（`recover`）。
  - **[Headless 断言]**：全部畸形返回 `DictError` 类型化变体，零 panic（0.4 规则 8）。
  - **[性能断言]**：全量 CRC 校验在加载路径预算内（`dict_mmap_rss` 口径记录）。

#### TC-DICT-002 FST 查询、WORDLIST 间接层与 mmap 生命周期

- **基本属性**：`REQ-DICT-02` ｜ `ASM-T-06/07` ｜ dict ｜ ①② ｜ nextest ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-DICT-06~10 ｜ 锚点：`crates/ime-dict/src/fst_index.rs:14-18,157-183`、`mmap.rs` ｜ 背书：fst_index 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：fixture 容器；只读挂载。
- **操作步骤与物理存盘动作**：
  1. 已知 key 查询与源数据逐字段一致 → `execution_log.txt`。
  2. 多音字多键命中同一 `word_id`；`start+count` 越界拒绝 → 记录。
  3. mmap RSS 口径记录；文件删除后既有视图仍可用 → `assertions.json`（budget_keys: `dict_mmap_rss`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：零拷贝 `WordRef` 借 mmap；编译期 weight 降序 = 查询即排名。
  - **[Headless 断言]**：词库不可用时解码降级不 panic。
  - **[性能断言]**：RSS 差分 ≤ `dict_mmap_rss`。

#### TC-DICT-003 条目表零拷贝与逐访问校验

- **基本属性**：`REQ-DICT-03` ｜ `ASM-T-07` ｜ dict ｜ ①② ｜ nextest ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-DICT-26~30 ｜ 锚点：`crates/ime-dict/src/entry.rs` ｜ 背书：entry 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：畸形条目 fixture。
- **操作步骤与物理存盘动作**：
  1. 四项边界校验（offset/len/count/flags）逐项注入 → `execution_log.txt`。
  2. UTF-8 构建期验证 / 读期信任的分工 → 记录。
  3. `WordFlags` 前向兼容（未知位丢弃）；畸形条目诊断去重 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：每次访问 bounds check；诊断不重复轰炸。
  - **[Headless 断言]**：未知 flags 位不致拒绝合法词。
  - **[性能断言]**：逐访问校验在查询热路径预算内（criterion `dict/*` 键）。

#### TC-DICT-004 用户词频库：记录/降级/淘汰

- **基本属性**：`REQ-DICT-04` ｜ `ASM-T-06` ｜ dict ｜ ①②④ ｜ nextest + `bench/userdb` ｜ `[Acceptance 验收套件]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-DICT-11~15、39~41 ｜ 锚点：`crates/ime-dict/src/user_db.rs:30-70,176-184,538` ｜ 背书：userdb 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：临时 XDG；`user.redb` 从零。
- **操作步骤与物理存盘动作**：
  1. record→commit→freq 闭环与排序翻转对照 → `execution_log.txt`。
  2. 崩溃模拟（窗口内丢增量 ≤ 窗口语义）；只读降级写失败不阻断 → 记录。
  3. 批量触发（32 键/2000ms/4096）与 slow-disk 自适应；50 万上限淘汰 → `assertions.json`（budget_keys: 无直引，行为口径）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`record` ≤ 5µs 契约（criterion `userdb/*` 键）；优雅退出零丢失（`final_commit`）；文件 0600。
  - **[Headless 断言]**：readonly 进入不中断输入。
  - **[性能断言]**：`freq` 命中不开读事务、不持写锁（水合态）。

#### TC-DICT-005 损坏自愈与只读模式

- **基本属性**：`REQ-DICT-05` ｜ `ASM-T-06` ｜ dict ｜ ②③ ｜ nextest ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-DICT-31~35 ｜ 锚点：`crates/ime-dict/src/recover.rs:75` ｜ 背书：recover 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：预注入损坏文件；只读目录两档（目录/文件）。
- **操作步骤与物理存盘动作**：
  1. 词库损坏 → quarantine 隔离 + 原子替换 → `execution_log.txt`。
  2. 自愈幂等（二次打开不再触发）；隔离命名冲突消解 → 记录。
  3. 只读模式：输入完整、不学习、`data/readonly-mode` 诊断 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：隔离而非删除；权限不递归修改用户目录。
  - **[Headless 断言]**：只读降级报告码 `data/readonly-mode` 恰一次。
  - **[性能断言]**：自愈路径不进入 decode 热路径。

#### TC-DICT-006 dictc 白名单与编译管线

- **基本属性**：`REQ-DICT-06` ｜ `ASM-T-07` ｜ dict ｜ ①② ｜ nextest(-p xtask) + `just smoke` ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-DICT-16~20、43 ｜ 锚点：`xtask/src/dictc/{source,manifest,build}.rs`、`scripts/check-dict-sources.sh` ｜ 背书：dictc 套件 + `check-dict` 自测
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：fixture TSV 与 allowlist 副本（ADR-0012 后含 `thuocl`/`huayu` 条目）。
- **操作步骤与物理存盘动作**：
  1. 未登记来源拒绝编译 → `execution_log.txt`。
  2. sha256 不匹配拒绝；非法音节行跳过并统计；声调等价归一 → 记录。
  3. TSV 解析边界（BOM/空行/超长列）；同 key 词条上限 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：白名单未完成条目（空 sha256 且文件在）必红；`CC0-1.0` 等价声明通过校验。
  - **[Headless 断言]**：退出码语义明确（非零即红）。
  - **[性能断言]**：全量编译时长 ≤ 既有门禁（9.3s 基线档，具体由 quality 门把守）。

#### TC-DICT-007 L3b 多音字定向展开

- **基本属性**：`REQ-DICT-07` ｜ `ASM-T-07` ｜ dict ｜ ①② ｜ nextest(-p xtask) ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-DICT-21~25、44 ｜ 锚点：`xtask/src/dictc/build.rs`（`expand_keys:277`）｜ 背书：L3b 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：polyphone fixture（去重后 2,587 行口径）。
- **操作步骤与物理存盘动作**：
  1. L3b 展开生效且受控（top 50k / cap 4 / 增幅 30%）→ `execution_log.txt`。
  2. 双编译逐字节一致 → 记录。
  3. `CAP` 可配置生效；误命中由权重压制；超限非零退出 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：键数增幅 ∈ [15%,30%]（既有 DoD 口径）。
  - **[Headless 断言]**：展开确定性与顺序无关。
  - **[性能断言]**：编译时长增量 ≤5%（归一化卡落地后同口径复核）。

#### TC-DICT-008 简繁词表与运行时

- **基本属性**：`REQ-DICT-08` ｜ `ASM-T-07` ｜ dict ｜ ①② ｜ nextest ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-DICT-46~49、56 ｜ 锚点：`crates/ime-dict/src/script.rs`、`crates/ime-core/src/script.rs` ｜ 背书：script 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：script.dict fixture（Unihan 种子变体表）。
- **操作步骤与物理存盘动作**：
  1. 词表编译往返 → `execution_log.txt`。
  2. 运行时最长匹配与一简对多繁消歧 → 记录。
  3. 畸形输入越界防护；`ToggleScript` 会话态与词频写回互斥（现状：仅重发帧）→ `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`ScriptIndex::unavailable()` 降级值语义正确（上报方接线属 `TASK-2.03.01`，落地前如实标注 pending-impl 子项）。
  - **[Headless 断言]**：commit 路径现状只走 `transform_committed`（标点/全宽），script 变换缺席是记录在案的事实。
  - **[性能断言]**：转换 ≤ 独占预算 0.30ms（features-add §3.4 口径，接线后生效）。

#### TC-DICT-009 用户词条管理原语

- **基本属性**：`REQ-DICT-09` ｜ `ASM-T-06` ｜ dict ｜ ①②③ ｜ nextest ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-DICT-50~52、57~58 ｜ 锚点：`crates/ime-dict/src/user_db/{manage,export}.rs` ｜ 背书：manage/export 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：临时用户库；导出目录 0600。
- **操作步骤与物理存盘动作**：
  1. forget/list/export/import 闭环 → `execution_log.txt`。
  2. 导出格式与规模边界（TSV 列契约）→ 记录。
  3. 写路径原子性；并发写与空闲调度；导出隐私复查（无输入内容混入）→ `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`is_user_word` 现状恒 false（造词卡 `TASK-3.04.01` 落地前的事实契约）。
  - **[Headless 断言]**：管理路径预算 ≤ 0.05ms 量级（非输入热路径）。
  - **[性能断言]**：导出 50k 条在可交互时长内（背书计时）。

#### TC-DICT-010 自动备份与回滚

- **基本属性**：`REQ-DICT-10` ｜ `ASM-T-06` ｜ dict ｜ ②③ ｜ nextest ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-DICT-53~55、59 ｜ 锚点：`crates/ime-dict/src/user_db/backup.rs` ｜ 背书：backup 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：三代备份目录 fixture；磁盘满注入。
- **操作步骤与物理存盘动作**：
  1. 备份策略与滚动（keep 3）→ `execution_log.txt`。
  2. 损坏库恢复路径；权限与并发写边界 → 记录。
  3. 备份↔恢复与用户频次一致性 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：恢复触发迁移兼容（v1 备份在新代码下恢复可用）。
  - **[Headless 断言]**：磁盘满时备份失败不阻断输入。
  - **[性能断言]**：备份 IO 在 flush 线程，宿主零阻塞。

#### TC-DICT-011 规模与长稳：体积/容量/RSS

- **基本属性**：`REQ-DICT-04/01/10` ｜ `ASM-T-06/07/08` ｜ dict ｜ ④ ｜ `xtask budget` + soak 通道 ｜ `[Stress 深度压测]` ｜ P2 ｜ `false` ｜ `[]` ｜ 吸收：TC-DICT-36~38、42、60 ｜ 锚点：`xtask/src/dictc/budget.rs`、`testd/memory.rs` ｜ 背书：`budget --validate` + 段份额
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：合成大库（50 万条上限口径）；8h 子项按 `ASM-T-08` 标注。
- **操作步骤与物理存盘动作**：
  1. 段份额与载荷红线（17.5MB 双红线）→ `execution_log.txt`。
  2. 编译耗时门；词频权重默认值 → 记录。
  3. 长稳 RSS 漂移 ≤ `robustness.rss_drift_mb`（短程实测 + 8h 子项记「本机不可验证」）→ `assertions.json`（budget_keys: `base_dict`, `plugin_rss`, `robustness.rss_drift_mb`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：规模断言以合成库为基准（`ASM-T-07`）。
  - **[Headless 断言]**：`Missing ≠ Pass`（预算看板纪律）。
  - **[性能断言]**：短程漂移实测过；8h 完整档需外部环境（`ASM-T-08`）。
