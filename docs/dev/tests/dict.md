# rspinyin 测试用例分片 · dict（词库与用户数据深化）

> 分片版本: v1.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 平台任务: [../features-test.md](../features-test.md) ｜
> 被测基线: Rust 2024 workspace（`ime-dict`）+ `dictc`（`xtask`） ｜ 关联 ADR: [../adr/0000-upstream-decisions.md](../adr/0000-upstream-decisions.md) ｜ 最后同步 Commit: `ee0dbfb` ｜
> 维护约定: 新增用例必须回写主文档第 2 节矩阵的 TC 列与维度列

## 0. 分片基线（引用主文档，不重复定义）

- **假设清单**：主文档第 1 节 + [features-test.md](../features-test.md) 第 1 节。强相关：`ASM-T-06`（`dict_mmap_rss = 25MB`、`base_dict = 20MB`）、`ASM-T-07`（开发词库 5,871 行 / 248KB）、`ASM-T-11`（基准纯净度）。
- **追踪矩阵**：主文档第 2 节的 `REQ-DICT-01` ~ `REQ-DICT-07`、`REQ-SEC-04`。
- **预算阈值**：`docs/dev/budgets.json`（键名：`dict_mmap_rss`、`base_dict`、`plugin_rss`、`rss_drift_mb`）。
- **不可信输入纪律**：词库与用户数据是**不可信输入**（0.4 规则 8 / `AGENTS.md` 3.9）——mmap 之前必须校验 magic、格式版本、长度字段与 CRC。本分片的全部畸形用例都在 `FEAT-TEST-P0.03.02` 的**临时副本**上执行，绝不触碰 `data/compiled/base.dict`。

> 现有基线：`ime-dict` 已有 **89 个 `#[test]`**（含 `user_db/tests.rs` 402 行）、2 个 criterion 基准（`benches/{dict,userdb}.rs`）。`data/compiled/base.dict` = 248KB。

---

## 1. 用例

### TC-DICT-26 条目表与字符串池的零拷贝访问（`REQ-DICT-03`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-03` ｜ `dict` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.03.03]` ｜ `crates/ime-dict/src/entry.rs`、`mmap.rs`
- **操作步骤**：
  1. `entry_to_ref` 转换 5000 个条目 -> 触发存盘：`<RUN>/dict/TC-DICT-26/assertions.json`
  2. 断言返回的 `WordRef.text` 是 `&'a str`（借用 mmap），**不产生 `String` 分配**；断言耗时 ≤ 20ns。
- **通过标准**：零拷贝（`Lexicon` 从 `mmap` 返回拥有所有权的 `String` 会破坏设计，`AGENTS.md` 3.8）。

### TC-DICT-27 四项边界校验（`REQ-DICT-03`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-03` ｜ `dict` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.03.03]`
- **操作步骤**：
  1. 构造 `word_off = strpool.len()`、`word_off + word_len` 溢出 `u32`、`word_len = 0`、`word_len = 97` 四类畸形条目 -> 触发存盘：`<RUN>/dict/TC-DICT-27/assertions.json`
  2. 断言全部返回 `DictError::LengthOutOfRange`，无 panic、无越界。
- **通过标准**：用 `miri` 跑一遍无 UB。

### TC-DICT-28 UTF-8 校验的构建期/运行期分工（`REQ-DICT-03`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-03` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.03.03]`
- **操作步骤**：
  1. 断言 `dictc` 在写入 `STRPOOL` 前对每个词调用 `str::from_utf8`（构建期 100% 校验）-> 触发存盘：`<RUN>/dict/TC-DICT-28/assertions.json`
  2. 断言运行期用 `from_utf8_unchecked` + `debug_assert!`（全量校验会把 6ms 加到每次解码上）。
- **通过标准**：威胁模型声明——防护目标是**意外损坏**（磁盘错误、截断、构建 bug），CRC32 + 边界校验已充分覆盖；本地攻击者主动篡改且重算 CRC 的场景**不在防护范围**。

### TC-DICT-29 `WordFlags` 前向兼容（`REQ-DICT-03`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-03` ｜ `dict` | 边界与容错 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.03.03]`
- **操作步骤**：
  1. 写入含未知位的 `flags` -> 触发存盘：`<RUN>/dict/TC-DICT-29/assertions.json`
  2. 断言 `from_bits_truncate` 丢弃未知位且不失败（新版本编译的词库仍可读）。
- **通过标准**：`WordFlags` 四变体（`SURNAME`/`PLACE`/`TERM`/`USER`）语义与 `dictc` 的 TSV 标志列一一对应。

### TC-DICT-30 畸形条目诊断去重（`REQ-DICT-03`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-03` ｜ `dict` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.03.03]`
- **操作步骤**：
  1. 对同一畸形索引访问 100 次 -> 触发存盘：`<RUN>/dict/TC-DICT-30/assertions.json`
  2. 断言只产生 1 条诊断记录（避免日志风暴）。
- **通过标准**：一次性诊断机制生效。

### TC-DICT-31 用户库损坏自愈：隔离而非删除（`REQ-DICT-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-05` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.03.05]` ｜ `crates/ime-dict/src/recover.rs`
- **操作步骤**：
  1. 用 0 字节文件与随机字节文件模拟 `user.redb` 损坏 -> 触发存盘：`<RUN>/dict/TC-DICT-31/assertions.json`
  2. 断言返回 `UserDbRebuilt`，`user.redb.corrupt.<unix_ts>` **存在**（绝不删除，用户可能希望人工恢复），新库可用。
- **通过标准**：写诊断 `data/db/recovered`（含隔离文件名与原始错误）；同一进程内**不重试**打开原文件。

### TC-DICT-32 词库损坏自愈与只读目录语义（`REQ-DICT-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-05` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.03.05]`
- **操作步骤**：
  1. 篡改 `base.dict` 的 magic / version / CRC 各 1 字节 -> 触发存盘：`<RUN>/dict/TC-DICT-32/assertions.json`
  2. 断言返回 `DictMissing`；断言在**用户可写目录**内原文件被重命名为 `base.dict.corrupt.<ts>`；断言在 `/usr/share/` 等只读位置**只记录不重命名**。
- **通过标准**：候选功能禁用但输入走 `Passthrough`（`features.md` 3.6 的降级表）。

### TC-DICT-33 自愈幂等（`REQ-DICT-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-05` ｜ `dict` | 边界与容错 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.03.05]`
- **操作步骤**：
  1. 连续调用 `recover_user_db` 两次 -> 触发存盘：`<RUN>/dict/TC-DICT-33/assertions.json`
  2. 断言第二次返回 `Healthy`（第一次已重建）。
- **通过标准**：自愈每一步幂等。

### TC-DICT-34 隔离文件命名冲突消解（`REQ-DICT-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-05` ｜ `dict` | 边界与容错 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.03.05]`
- **操作步骤**：
  1. 同一秒内连续触发 3 次损坏 -> 触发存盘：`<RUN>/dict/TC-DICT-34/assertions.json`
  2. 断言隔离文件追加 `.1`、`.2` 后缀，无覆盖。
- **通过标准**：`unix_ts` 秒级命名 + 冲突消解。

### TC-DICT-35 只读模式下的输入完整性（`REQ-DICT-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-05` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.03.05]`
- **操作步骤**：
  1. 数据目录不可写 -> 触发存盘：`<RUN>/dict/TC-DICT-35/assertions.json`
  2. 断言 `ReadonlyMode`；断言输入完整可用、不学习、日志降级到 `stderr`。
- **通过标准**：`ASM-15`——绝不因写失败而阻断输入。

### TC-DICT-36 词库体积分段预算（`REQ-DICT-01` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-01` ｜ `dict` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 编译完整 40 万词库（合成）-> 触发存盘：`<RUN>/dict/TC-DICT-36/assertions.json`
  2. 断言分段：字符串池 ≤ 4MB、条目表 ≤ 6.5MB、`WORDLIST` ≤ 2MB、FST ≤ 2.5MB、unigram ≤ 3.5MB；合计 ≤ 17.5MB；总文件 ≤ `base_dict`（20MB）。
- **通过标准**：`ASM-05` 的分段预算；超限时按权重截断至 top 32 万并打印警告。

### TC-DICT-37 `dictc` 编译耗时（`REQ-DICT-01` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-01` ｜ `dict` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 编译 40 万词条并计时 -> 触发存盘：`<RUN>/dict/TC-DICT-37/assertions.json`
  2. 断言 ≤ 90 秒（单线程流式写入）；断言 `L3b` 的展开计算 ≤ 5 秒。
- **通过标准**：构建期性能预算。

### TC-DICT-38 词频权重默认值（`REQ-DICT-01` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-01` ｜ `dict` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. TSV 权重列缺省时编译 -> 触发存盘：`<RUN>/dict/TC-DICT-38/assertions.json`
  2. 断言按词长给默认值：2 字 = 30000、3 字 = 20000、4 字 = 15000、≥5 字 = 8000。
- **通过标准**：默认值可复现。

### TC-DICT-39 用户库提交批量与自适应降级（`REQ-DICT-04` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-04` ｜ `dict` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 模拟慢磁盘（`commit` > 3ms）-> 触发存盘：`<RUN>/dict/TC-DICT-39/assertions.json`
  2. 断言自动把 `COMMIT_BATCH` 提到 128、`COMMIT_INTERVAL_MS` 提到 10000，并记 `data/commit/slow-disk`。
- **通过标准**：自适应降级，**不阻塞输入**。

### TC-DICT-40 优雅退出的零丢失终提交（`REQ-DICT-04` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-04` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 写入若干条后走 `on_addon_destroy` 路径 -> 触发存盘：`<RUN>/dict/TC-DICT-40/assertions.json`
  2. 断言执行一次 `Durability::Immediate` 终提交，重开后条数与退出前**完全一致**。
- **通过标准**：`ASM-20`——崩溃最多丢 2 秒，优雅退出零丢失。

### TC-DICT-41 用户库容量上限（`REQ-DICT-04` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-04` ｜ `dict` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 注入 50 万条以上 -> 触发存盘：`<RUN>/dict/TC-DICT-41/assertions.json`
  2. 断言按 `last_used_unix` 淘汰最旧 10%，淘汰在**空闲期**（无输入 30s）后台执行。
- **通过标准**：`ASM-06`。

### TC-DICT-42 长稳 RSS 漂移（`REQ-DICT-04` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-04` ｜ `dict` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]` + `[不可验证]` 真实 8 小时需外部环境
- **操作步骤**：
  1. 模拟 10 字/秒 × 5 分钟输入 -> 触发存盘：`<RUN>/dict/TC-DICT-42/assertions.json`
  2. 断言 RSS 漂移 ≤ `rss_drift_mb`（2MB）。
- **通过标准**：`BUDGET-ROB-01` 的早期验证；完整 8 小时长稳为 `[不可验证]`（`ASM-T-08`）。

### TC-DICT-43 `dictc` 的 TSV 解析边界（`REQ-DICT-06` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-06` ｜ `dict` | 边界与容错 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 构造空行、缺列、超长词（> 96 字节）、含制表符的词 -> 触发存盘：`<RUN>/dict/TC-DICT-43/assertions.json`
  2. 断言全部被跳过并计入统计，不中断编译。
- **通过标准**：不静默丢弃。

### TC-DICT-44 同 key 词条数上限（`REQ-DICT-06` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-06` ｜ `dict` | 边界与容错 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 构造某 key 下有 100 个词 -> 触发存盘：`<RUN>/dict/TC-DICT-44/assertions.json`
  2. 断言只保留权重最高的 32 个，丢弃项计入统计。
- **通过标准**：`count ≤ 2^24` 的编码约束；丢弃不静默。

### TC-DICT-45 词源白名单的完整字段（`REQ-SEC-04` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-04` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 检查 `data/sources.toml` 的每个来源 -> 触发存盘：`<RUN>/dict/TC-DICT-45/assertions.json`
  2. 断言含 `id`/`kind`/`layer`/`url`/`license`/`spdx`/`retrieved`/`sha256`/`permissive` 字段。
- **通过标准**：当前 5 个来源（`pinyin-data`、`unihan`、`jieba-dict`、`base`、`polyphone`）；`kind = derived` 的来源豁免哈希固定（生成器变化时内容会变）。

---

## 2. 分片出口准则

1. `REQ-DICT-03` 与 `REQ-DICT-05` 的 10 条用例在 `TASK-1.03.03` / `TASK-1.03.05` 落地后转为 `[可执行]` 并通过。
2. `data/compiled/base.dict` 在全部测试后 `sha256` 不变（畸形用例只操作临时副本）。
3. `check-unsafe.sh` 对 `mmap.rs` 的 `SAFETY` 注释回看窗口问题（`.dev-progress.json` 记录的已知缺陷）在 `TASK-1.03.03` 中修复，`TC-INFRA-12` 转为通过。
4. 主文档矩阵的 `REQ-DICT-03`、`REQ-DICT-05` 行可执行性列更新为 `✅`，维度列全部勾选。
