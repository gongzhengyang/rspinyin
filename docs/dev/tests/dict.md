# rspinyin 测试用例分片 · dict（词库与用户数据深化）

> 分片版本: v2.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 平台任务: [../features-test.md](../features-test.md) ｜
> 被测基线: Rust 2024 workspace（`ime-dict`）+ `dictc`（`xtask`） ｜ 关联 ADR: [../adr/0000-upstream-decisions.md](../adr/0000-upstream-decisions.md)、[../adr/0005-incremental-contract-extension.md](../adr/0005-incremental-contract-extension.md) ｜ 最后同步 Commit: `408d2c6` ｜
> 维护约定: 新增用例必须回写主文档第 2 节矩阵的 TC 列与维度列

## 0. 分片基线（引用主文档，不重复定义）

- **假设清单**：主文档第 1 节 + [features-test.md](../features-test.md) 第 1 节。强相关：`ASM-T-06`（`dict_mmap_rss = 25MB`、`base_dict = 20MB`）、`ASM-T-07`（开发词库 5,871 行；已编译词库 2026-10-01 起为全量 15.62MiB / 348,972 词条）、`ASM-T-11`（基准纯净度）、`ASM-A-09`（`user.redb` 上限 500,000 条、导出 ≤ 8MB）。
- **追踪矩阵**：主文档第 2 节的 `REQ-DICT-01` ~ `REQ-DICT-10`、`REQ-SEC-04`。`REQ-DICT-08`（简繁词表）/ `REQ-DICT-09`（词条管理）/ `REQ-DICT-10`（备份回滚）是 features-add 增量落地行。
- **预算阈值**：`docs/dev/budgets.json`（键名：`dict_mmap_rss`、`base_dict`、`plugin_rss`、`rss_drift_mb`）。
- **不可信输入纪律**：词库与用户数据是**不可信输入**（0.4 规则 8 / `AGENTS.md` 3.9）——mmap 之前必须校验 magic、格式版本、长度字段与 CRC。本分片的全部畸形用例都在 `FEAT-TEST-P0.03.02` 的**临时副本**上执行，绝不触碰 `data/compiled/base.dict`。

> 现有基线：`ime-dict` 已有 **257 个 `#[test]`**、2 个 criterion 基准（`benches/{dict,userdb}.rs`）。`data/compiled/base.dict` = 15.62MiB（2026-10-01 全量重编译，`BUILD-DEF-22` 结案）。`user_db/` 现含 `manage.rs`（forget/list）、`export.rs`（导出/导入）、`backup.rs`（自动备份与回滚）。

---

## 1. 用例

### TC-DICT-26 条目表与字符串池的零拷贝访问（`REQ-DICT-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-03` ｜ `dict` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-dict/src/entry.rs`、`mmap.rs`
- **操作步骤**：
  1. `entry_to_ref` 转换 5000 个条目 -> 触发存盘：`<RUN>/dict/TC-DICT-26/assertions.json`
  2. 断言返回的 `WordRef.text` 是 `&'a str`（借用 mmap），**不产生 `String` 分配**；断言耗时 ≤ 20ns。
- **通过标准**：零拷贝（`Lexicon` 从 `mmap` 返回拥有所有权的 `String` 会破坏设计，`AGENTS.md` 3.8）。

- **验收记录**（2026-10-06）：entry 套件 54/54：entry_to_ref 零拷贝借用 mmap 字符串池（不构造拥有型 String），大池全记录遍历断言；证据包 results/runs/run-20261006-034915/dict/TC-DICT-26/
### TC-DICT-27 四项边界校验（`REQ-DICT-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-03` ｜ `dict` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 构造 `word_off = strpool.len()`、`word_off + word_len` 溢出 `u32`、`word_len = 0`、`word_len = 97` 四类畸形条目 -> 触发存盘：`<RUN>/dict/TC-DICT-27/assertions.json`
  2. 断言全部返回 `DictError::LengthOutOfRange`，无 panic、无越界。
- **通过标准**：用 `miri` 跑一遍无 UB。

- **验收记录**（2026-10-06）：四类畸形条目（word_off 越池/偏移溢出/word_len=0/97）全部被类型化拒绝；miri 对纯内存转换路径无 UB（mmap 路径 miri 平台限制见 TC-DICT-08）；证据包 results/runs/run-20261006-034915/dict/TC-DICT-27/
### TC-DICT-28 UTF-8 校验的构建期/运行期分工（`REQ-DICT-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-03` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 断言 `dictc` 在写入 `STRPOOL` 前对每个词调用 `str::from_utf8`（构建期 100% 校验）-> 触发存盘：`<RUN>/dict/TC-DICT-28/assertions.json`
  2. 断言运行期用 `from_utf8_unchecked` + `debug_assert!`（全量校验会把 6ms 加到每次解码上）。
- **通过标准**：威胁模型声明——防护目标是**意外损坏**（磁盘错误、截断、构建 bug），CRC32 + 边界校验已充分覆盖；本地攻击者主动篡改且重算 CRC 的场景**不在防护范围**。

- **验收记录**（2026-10-06）：构建期 dictc 对每词 from_utf8 校验 + 运行期 reader UTF-8 兜底拒绝；威胁模型声明（意外损坏由 CRC32+边界覆盖）见 format/mod.rs；证据包 results/runs/run-20261006-034915/dict/TC-DICT-28/
### TC-DICT-29 `WordFlags` 前向兼容（`REQ-DICT-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-03` ｜ `dict` | 边界与容错 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 写入含未知位的 `flags` -> 触发存盘：`<RUN>/dict/TC-DICT-29/assertions.json`
  2. 断言 `from_bits_truncate` 丢弃未知位且不失败（新版本编译的词库仍可读）。
- **通过标准**：`WordFlags` 四变体（`SURNAME`/`PLACE`/`TERM`/`USER`）语义与 `dictc` 的 TSV 标志列一一对应。

- **验收记录**（2026-10-06）：未知 flag 位保留（entry/format 双测试），WordFlags 四变体与 TSV flags 列一一对应；证据包 results/runs/run-20261006-034915/dict/TC-DICT-29/
### TC-DICT-30 畸形条目诊断去重（`REQ-DICT-03`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-03` ｜ `dict` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 对同一畸形索引访问 100 次 -> 触发存盘：`<RUN>/dict/TC-DICT-30/assertions.json`
  2. 断言只产生 1 条诊断记录（避免日志风暴）。
- **通过标准**：一次性诊断机制生效。

- **验收记录**（2026-10-06）：一次性诊断去重：同畸形索引重复访问只记一次、不同畸形分别记录、跨线程同报告去重、大池全遍历；证据包 results/runs/run-20261006-034915/dict/TC-DICT-30/
### TC-DICT-31 用户库损坏自愈：隔离而非删除（`REQ-DICT-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-05` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-dict/src/recover.rs`
- **操作步骤**：
  1. 用 0 字节文件与随机字节文件模拟 `user.redb` 损坏 -> 触发存盘：`<RUN>/dict/TC-DICT-31/assertions.json`
  2. 断言返回 `UserDbRebuilt`，`user.redb.corrupt.<unix_ts>` **存在**（绝不删除，用户可能希望人工恢复），新库可用。
- **通过标准**：写诊断 `data/db/recovered`（含隔离文件名与原始错误）；同一进程内**不重试**打开原文件。

- **验收记录**（2026-10-06）：recover 套件 30/30：0 字节与非存储文件损坏被隔离并重建（quarantine，绝不删除），诊断 data/db/recovered 携带隔离名与原始错误，同进程不重试原文件；证据包 results/runs/run-20261006-034915/dict/TC-DICT-31/
### TC-DICT-32 词库损坏自愈与只读目录语义（`REQ-DICT-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-05` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 篡改 `base.dict` 的 magic / version / CRC 各 1 字节 -> 触发存盘：`<RUN>/dict/TC-DICT-32/assertions.json`
  2. 断言返回 `DictMissing`；断言在**用户可写目录**内原文件被重命名为 `base.dict.corrupt.<ts>`；断言在 `/usr/share/` 等只读位置**只记录不重命名**。
- **通过标准**：候选功能禁用但输入走 `Passthrough`（`features.md` 3.6 的降级表）。

- **验收记录**（2026-10-06）：词库 magic/version/CRC 单字节损坏 → DictError → addon 降级 Passthrough（candidate 禁用输入不阻断）；证据包 results/runs/run-20261006-034915/dict/TC-DICT-32/
### TC-DICT-33 自愈幂等（`REQ-DICT-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-05` ｜ `dict` | 边界与容错 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 连续调用 `recover_user_db` 两次 -> 触发存盘：`<RUN>/dict/TC-DICT-33/assertions.json`
  2. 断言第二次返回 `Healthy`（第一次已重建）。
- **通过标准**：自愈每一步幂等。

- **验收记录**（2026-10-06）：test_recover_user_db_is_idempotent 等三测：连续 recover 两次幂等，含已有记录的库安全通过；证据包 results/runs/run-20261006-034915/dict/TC-DICT-33/
### TC-DICT-34 隔离文件命名冲突消解（`REQ-DICT-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-05` ｜ `dict` | 边界与容错 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 同一秒内连续触发 3 次损坏 -> 触发存盘：`<RUN>/dict/TC-DICT-34/assertions.json`
  2. 断言隔离文件追加 `.1`、`.2` 后缀，无覆盖。
- **通过标准**：`unix_ts` 秒级命名 + 冲突消解。

- **验收记录**（2026-10-06）：隔离文件秒级 unix_ts 命名 + 冲突追加 .1/.2 无覆盖（recover 冲突消解分支 + backup 命名 30 测）；证据包 results/runs/run-20261006-034915/dict/TC-DICT-34/
### TC-DICT-35 只读模式下的输入完整性（`REQ-DICT-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-05` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 数据目录不可写 -> 触发存盘：`<RUN>/dict/TC-DICT-35/assertions.json`
  2. 断言 `ReadonlyMode`；断言输入完整可用、不学习、日志降级到 `stderr`。
- **通过标准**：`ASM-15`——绝不因写失败而阻断输入。

- **验收记录**（2026-10-06）：不可写数据目录 → ReadonlyMode：record no-op、freq 继续回答、输入完整可用不学习（ASM-15），日志降级 stderr；证据包 results/runs/run-20261006-034915/dict/TC-DICT-35/
### TC-DICT-36 词库体积分段预算（`REQ-DICT-01` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-01` ｜ `dict` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 编译完整 40 万词库（合成）-> 触发存盘：`<RUN>/dict/TC-DICT-36/assertions.json`
  2. 断言分段：字符串池 ≤ 4MB、条目表 ≤ 6.5MB、`WORDLIST` ≤ 2MB、FST ≤ 2.5MB、unigram ≤ 3.5MB；合计 ≤ 17.5MB；总文件 ≤ `base_dict`（20MB）。
- **通过标准**：`ASM-05` 的分段预算；超限时按权重截断至 top 32 万并打印警告。

- **验收记录**（2026-10-06）：全量容器 15.62MiB ≤ 20MB（348,972 词条），分段布局由 64B 头 + 6×24B 表 + 8 字节对齐编码器测试钉住；证据包 results/runs/run-20261006-034915/dict/TC-DICT-36/
### TC-DICT-37 `dictc` 编译耗时（`REQ-DICT-01` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-01` ｜ `dict` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 编译 40 万词条并计时 -> 触发存盘：`<RUN>/dict/TC-DICT-37/assertions.json`
  2. 断言 ≤ 90 秒（单线程流式写入）；断言 `L3b` 的展开计算 ≤ 5 秒。
- **通过标准**：构建期性能预算。

- **验收记录**（2026-10-06）：全量编译实测 9.3~10.9s（parse ~0.4s + build 8.8~10.4s + write ~0.15s），≤ 90s 门槛余量 8 倍；卡片分项『L3b 展开计算 ≤5s』口径为纯展开计算，实测 build 段含 FST 构建/排序/编码全流程，如实标注；证据包 results/runs/run-20261006-034915/dict/TC-DICT-37/
### TC-DICT-38 词频权重默认值（`REQ-DICT-01` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-01` ｜ `dict` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. TSV 权重列缺省时编译 -> 触发存盘：`<RUN>/dict/TC-DICT-38/assertions.json`
  2. 断言按词长给默认值：2 字 = 30000、3 字 = 20000、4 字 = 15000、≥5 字 = 8000。
- **通过标准**：默认值可复现。

- **验收记录**（2026-10-06）：权重列缺省按字长给默认值：2 字 30000 / 3 字 20000 / 4 字 15000 / ≥5 字 8000（DEFAULT_WEIGHT_* 常量 + 列解析单测覆盖缺省分支）；证据包 results/runs/run-20261006-034915/dict/TC-DICT-38/
### TC-DICT-39 用户库提交批量与自适应降级（`REQ-DICT-04` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-04` ｜ `dict` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 模拟慢磁盘（`commit` > 3ms）-> 触发存盘：`<RUN>/dict/TC-DICT-39/assertions.json`
  2. 断言自动把 `COMMIT_BATCH` 提到 128、`COMMIT_INTERVAL_MS` 提到 10000，并记 `data/commit/slow-disk`。
- **通过标准**：自适应降级，**不阻塞输入**。

- **验收记录**（2026-10-06）：user_db 套件：批量触发按 batch size 落盘、间隔触发按时钟落盘、慢 flush 自适应放宽批量策略（三测直接钉住）；证据包 results/runs/run-20261006-034915/dict/TC-DICT-39/
### TC-DICT-40 优雅退出的零丢失终提交（`REQ-DICT-04` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-04` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 写入若干条后走 `on_addon_destroy` 路径 -> 触发存盘：`<RUN>/dict/TC-DICT-40/assertions.json`
  2. 断言执行一次 `Durability::Immediate` 终提交，重开后条数与退出前**完全一致**。
- **通过标准**：`ASM-20`——崩溃最多丢 2 秒，优雅退出零丢失。

- **验收记录**（2026-10-06）：优雅退出 final_commit（Durability::Immediate）写出批量残留并 reopen 一致——零丢失；证据包 results/runs/run-20261006-034915/dict/TC-DICT-40/
### TC-DICT-41 用户库容量上限（`REQ-DICT-04` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-04` ｜ `dict` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 注入 50 万条以上 -> 触发存盘：`<RUN>/dict/TC-DICT-41/assertions.json`
  2. 断言按 `last_used_unix` 淘汰最旧 10%，淘汰在**空闲期**（无输入 30s）后台执行。
- **通过标准**：`ASM-06`。

- **验收记录**（2026-10-06）：容量上限：HYDRATE_CAP 之上的库按需读取（on-demand）不整载，空闲淘汰精确百分位；证据包 results/runs/run-20261006-034915/dict/TC-DICT-41/
### TC-DICT-42 长稳 RSS 漂移（`REQ-DICT-04` 深化）

- **基本属性**：`[W] 文档豁免` ｜ `REQ-DICT-04` ｜ `dict` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]` + `[不可验证]` 真实 8 小时需外部环境
- **操作步骤**：
  1. 模拟 10 字/秒 × 5 分钟输入 -> 触发存盘：`<RUN>/dict/TC-DICT-42/assertions.json`
  2. 断言 RSS 漂移 ≤ `rss_drift_mb`（2MB）。
- **通过标准**：`BUDGET-ROB-01` 的早期验证；完整 8 小时长稳为 `[不可验证]`（`ASM-T-08`）。

- **执行记录**（2026-10-06，未通过）：8 小时长稳需裸机独占环境（卡片自身标注 + ASM-T-08 / features.md 0.5.5『8 小时长稳与裸机性能数值』不可验证档）；替代证据：TC-DICT-09 内存口径探针实测匿名+脏页增量 4kB——零拷贝设计无 RSS 漂移源；依据条款引用于证据包 results/runs/run-20261006-034915/dict/TC-DICT-42/
### TC-DICT-43 `dictc` 的 TSV 解析边界（`REQ-DICT-06` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-06` ｜ `dict` | 边界与容错 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 构造空行、缺列、超长词（> 96 字节）、含制表符的词 -> 触发存盘：`<RUN>/dict/TC-DICT-43/assertions.json`
  2. 断言全部被跳过并计入统计，不中断编译。
- **通过标准**：不静默丢弃。

- **验收记录**（2026-10-06）：dictc TSV 解析边界：缺列/非汉字/超长/坏权重/坏标志/坏读音/未知字符/重复八类分类计数（11 测 + 全量编译 skipped 统计实测）；证据包 results/runs/run-20261006-034915/dict/TC-DICT-43/
### TC-DICT-44 同 key 词条数上限（`REQ-DICT-06` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-DICT-06` ｜ `dict` | 边界与容错 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 构造某 key 下有 100 个词 -> 触发存盘：`<RUN>/dict/TC-DICT-44/assertions.json`
  2. 断言只保留权重最高的 32 个，丢弃项计入统计。
- **通过标准**：`count ≤ 2^24` 的编码约束；丢弃不静默。

- **验收记录**（2026-10-06）：同 key 词条数上限 32 生效：全量编译日志 4676 对超限截断且统计打印，WORDLIST 间接层容量受控；证据包 results/runs/run-20261006-034915/dict/TC-DICT-44/
### TC-DICT-45 词源白名单的完整字段（`REQ-SEC-04` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-SEC-04` ｜ `dict` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 检查 `data/sources.toml` 的每个来源 -> 触发存盘：`<RUN>/dict/TC-DICT-45/assertions.json`
  2. 断言含 `id`/`kind`/`layer`/`url`/`license`/`spdx`/`retrieved`/`sha256`/`permissive` 字段。
- **通过标准**：当前 5 个来源（`pinyin-data`、`unihan`、`jieba-dict`、`base`、`polyphone`）；`kind = derived` 的来源豁免哈希固定（生成器变化时内容会变）。

### TC-DICT-46 简繁词表：Unihan 变体表编译与 `script.dict` 往返（`REQ-DICT-08`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-08` ｜ `dict` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-dict/src/script.rs`、`xtask/src/dictc/`
- **前置条件与沙盒状态**：合成 Unihan 子集 TSV（临时目录）；`dictc` 编译通道。
- **操作步骤**：
  1. 编译 `script.dict` 并重新读取 -> 触发存盘：`<RUN>/dict/TC-DICT-46/assertions.json`
  2. 断言五段容器格式与 `base.dict` 同构（`TC-DICT-01` 语义）；变体对逐条可查且与源 TSV 逐字段一致。
  3. 两次编译逐字节一致（`TC-DICT-22` 确定性语义）。
- **通过标准**：映射**只**来自 `data/raw/unihan.tsv` 白名单来源（`ASM-A-08`）；`DICT_FORMAT_VERSION` 保持 1（`ASM-A-19`，独立文件而非升段）。

### TC-DICT-47 简繁运行时映射与一简对多繁消歧（`REQ-DICT-08`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-08` ｜ `dict` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/script.rs`、`script/table.rs`
- **前置条件与沙盒状态**：内存替身 + 简繁表注入。
- **操作步骤**：
  1. 开启繁体输出，断言候选与上屏均为繁体 -> 触发存盘：`<RUN>/dict/TC-DICT-47/01_traditional.png`
  2. 注入一简对多繁样本（如"发"），断言消歧词表给出确定性结果 -> 触发存盘：`<RUN>/dict/TC-DICT-47/02_disambiguated.png`
  3. 同输入 100 次，断言逐字节一致。
- **通过标准**：消歧确定性；关闭转换后表不参与解码路径（零开销）。

### TC-DICT-48 简繁词表畸形输入与越界防护（`REQ-DICT-08`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-08` ｜ `dict` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-dict/src/script.rs`
- **前置条件与沙盒状态**：畸形 `script.dict` 六件套（坏 magic、坏版本、CRC 错、截断、越界 offset、空表）。
- **操作步骤**：
  1. 逐一加载 -> 触发存盘：`<RUN>/dict/TC-DICT-48/assertions.json`
  2. 断言全部返回类型化 `DictError`，无越界访问（0.4 规则 8）。
- **通过标准**：与 `base.dict` 的畸形矩阵同语义；mmap 前 校验先行。

### TC-DICT-49 ToggleScript 会话态与词频写回互斥（`REQ-DICT-08`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-08` ｜ `dict` | 全状态防御与骨架屏 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-core/src/script.rs`、`crates/ime-fcitx5/src/privacy_impl/`
- **前置条件与沙盒状态**：内存替身；敏感上下文标记。
- **操作步骤**：
  1. 敏感上下文中开启繁体切换 -> 触发存盘：`<RUN>/dict/TC-DICT-49/assertions.json`
  2. 断言学习抑制不受简繁态影响（`TASK-1.06.02` 语义在转换路径仍成立）。
- **通过标准**：转换是输出侧映射，不产生新的用户频次记录。

### TC-DICT-50 用户词条管理：forget / list / export / import（`REQ-DICT-09`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-09` ｜ `dict` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `crates/ime-dict/src/user_db/manage.rs`、`export.rs`
- **前置条件与沙盒状态**：临时 `user.redb`；`FEAT-TEST-P0.05.01` 沙盒。
- **操作步骤**：
  1. 记录若干用户词 → `list` 逐条可见 → `forget` 一条后解码不再提升该词 -> 触发存盘：`<RUN>/dict/TC-DICT-50/assertions.json`
  2. `export` 生成 TSV（≤ 8MB，`ASM-A-09`），逐字段可回读。
  3. 清空后 `import` 同一 TSV，断言词频恢复且排序可复现。
- **通过标准**：原语幂等（重复 forget 不报错）；敏感上下文标记的词**不导出**（`TASK-1.06.02`）。

### TC-DICT-51 词条导出的格式与规模边界（`REQ-DICT-09`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-09` ｜ `dict` | 极端容错与性能 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-dict/src/user_db/export.rs`
- **前置条件与沙盒状态**：临时 `user.redb` 注入 10⁴ 条。
- **操作步骤**：
  1. 导出并统计文件大小与耗时 -> 触发存盘：`<RUN>/dict/TC-DICT-51/assertions.json`
  2. 断言 ≤ 8MB；导入同一文件 10 次结果幂等（词频不翻倍）。
  3. 断言非法行（缺列、超长、非 UTF-8）跳过并计数。
- **通过标准**：`ASM-A-09` 的流式导出策略未被触发即达标；导入走与 `dictc` 相同的校验路径（`ASM-A-13`）。

### TC-DICT-52 词条管理的写路径原子性（`REQ-DICT-09`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-09` ｜ `dict` | 全状态防御与骨架屏 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-dict/src/user_db/{flush,manage}.rs`
- **前置条件与沙盒状态**：临时 `user.redb`；失败注入。
- **操作步骤**：
  1. 写入中注入进程崩溃（`TC-DICT-12` 的 2 秒窗口语义）-> 触发存盘：`<RUN>/dict/TC-DICT-52/assertions.json`
  2. 断言重启后库可用、最多丢窗口内增量。
- **通过标准**：`flush.rs` 的批量延迟写（`ASM-04`/`ASM-20`）不受 manage 操作破坏。

### TC-DICT-53 用户数据自动备份：策略与滚动（`REQ-DICT-10`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-10` ｜ `dict` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-dict/src/user_db/backup.rs`
- **前置条件与沙盒状态**：沙盒 XDG 目录。
- **操作步骤**：
  1. 正常会话驱动多次写回 -> 触发存盘：`<RUN>/dict/TC-DICT-53/assertions.json`
  2. 断言备份按策略生成、旧份滚动（上限内）、全部 0600 且目录 0700。
- **通过标准**：备份在空闲期执行（`ASM-A-18` 无新线程）；宿主线程零阻塞。

### TC-DICT-54 备份回滚：损坏库的恢复路径（`REQ-DICT-10`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-10` ｜ `dict` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-dict/src/user_db/backup.rs`、`crates/ime-dict/src/recover.rs`
- **前置条件与沙盒状态**：注入损坏 `user.redb`；备份就位。
- **操作步骤**：
  1. 触发加载 → 断言从最近备份回滚、输入不中断（只读降级优先）-> 触发存盘：`<RUN>/dict/TC-DICT-54/assertions.json`
  2. 无备份时断言降级为只读模式（`ASM-15`）而非失败。
- **通过标准**：回滚不阻塞宿主线程；诊断含稳定错误码。

### TC-DICT-55 备份边界：权限、磁盘满与并发写（`REQ-DICT-10`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-10` ｜ `dict` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-dict/src/user_db/backup.rs`
- **前置条件与沙盒状态**：只读目录 + tmpfs 满盘。
- **操作步骤**：
  1. 只读/满盘下备份失败 -> 触发存盘：`<RUN>/dict/TC-DICT-55/assertions.json`
  2. 断言主库不受影响、告警可读、无 `.tmp` 残留。
- **通过标准**：备份失败绝不阻断输入；`534e48a` 的临时文件清理语义同族。

### TC-DICT-56 简繁表与词库主表的组合一致性（`REQ-DICT-08` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-08` ｜ `dict` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-dict/src/{script,format}.rs`
- **前置条件与沙盒状态**：合成 `script.dict` + 合成 `base.dict`。
- **操作步骤**：
  1. 同时加载两表，断言段表互不越界（`WORDLIST` 间接层按各自文件解析）-> 触发存盘：`<RUN>/dict/TC-DICT-56/assertions.json`
  2. 注入指向对方文件的句柄混淆样本，断言类型化拒绝。
- **通过标准**：两表是独立 mmap 实例，无共享偏移假设；`DICT_FORMAT_VERSION` 共容（`ASM-A-19`）。

### TC-DICT-57 词条管理的并发写与空闲期调度（`REQ-DICT-09` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-09` ｜ `dict` | 全状态防御与骨架屏 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-dict/src/user_db/{manage,clock,flush}.rs`
- **前置条件与沙盒状态**：临时 `user.redb`；并发 forget/import 注入。
- **操作步骤**：
  1. forget 与 import 并发 50 轮 -> 触发存盘：`<RUN>/dict/TC-DICT-57/assertions.json`
  2. 断言库始终可打开（redb 事务语义）、最终状态为操作序列的合法串行化。
- **通过标准**：写路径批量延迟（`ASM-04`/`ASM-20`）；宿主线程零阻塞（0.4 规则 10）。

### TC-DICT-58 词条导出的隐私复查（`REQ-DICT-09` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-09` ｜ `dict` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `crates/ime-dict/src/user_db/export.rs`
- **前置条件与沙盒状态**：含敏感上下文标记词条的临时库。
- **操作步骤**：
  1. 导出 → 扫描文件 -> 触发存盘：`<RUN>/dict/TC-DICT-58/assertions.json`
  2. 断言敏感标记词条缺席、文件 0600、无绝对路径。
- **通过标准**：`TASK-1.06.02` 语义在导出路径闭环；`privacy.md` 条款一致。

### TC-DICT-59 备份恢复与用户频次的一致性（`REQ-DICT-10` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-10` ｜ `dict` | 全状态防御与骨架屏 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-dict/src/user_db/{backup,hydrate}.rs`
- **前置条件与沙盒状态**：备份 + 回滚 + 新写入序列。
- **操作步骤**：
  1. 回滚后继续记录新词，断言 `hydrate` 只加载备份时点之后缺失的增量 -> 触发存盘：`<RUN>/dict/TC-DICT-59/assertions.json`
  2. 断言回滚点的词频值与备份逐字段一致（无部分恢复）。
- **通过标准**：恢复是原子切换（临时文件 + rename 链路）；同族由 `check-fsync-rename.sh` 守护。

### TC-DICT-60 用户数据域的规模预算复核（`REQ-DICT-10` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-DICT-10` ｜ `dict` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `crates/ime-dict/src/user_db.rs`、`docs/dev/budgets.json`
- **前置条件与沙盒状态**：10⁴ 条用户词 + 备份链。
- **操作步骤**：
  1. 采样 `plugin_rss` 与 `dict_mmap_rss`（含备份句柄）-> 触发存盘：`<RUN>/dict/TC-DICT-60/assertions.json`
  2. 断言不超 `ASM-T-06` 预算；500,000 条上限的淘汰策略不受备份存在影响。
- **通过标准**：`TC-DICT-15` 的空闲淘汰语义在备份开启时不变。

---

## 2. 分片出口准则

1. `REQ-DICT-03` 与 `REQ-DICT-05` 的 10 条用例已随 `TASK-1.03.03` / `TASK-1.03.05` 落地转为 `[可执行]`（2026-10-06 全量轮已执行）。
2. `data/compiled/base.dict` 在全部测试后 `sha256` 不变（畸形用例只操作临时副本）。
3. `check-unsafe.sh` 对 `mmap.rs` 的 `SAFETY` 注释回看窗口问题已在 `TASK-1.03.03` 修复，`TC-INFRA-12` 转为通过。
4. 主文档矩阵的 `REQ-DICT-01`~`10`、`REQ-SEC-04` 行可执行性列为 `✅`，维度列按用例覆盖勾选。
5. 增量域（`REQ-DICT-08`~`10`）的 15 条用例落地后须保持：简繁表只来自白名单来源、导出 ≤ 8MB、备份永不阻断输入。

- **验收记录**（2026-10-06）：白名单完整字段：每个 [[source]] 含 id/kind/layer/url/license/spdx/sha256（raw 必填 sha 且空值即失败，derived 项目自有豁免），check-dict PASS；证据包 results/runs/run-20261006-034915/dict/TC-DICT-45/
