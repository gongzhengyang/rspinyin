# ADR-0012：词源扩展与 bigram 共现语料决策

> 状态：**已接受（Accepted）** ｜ 决策日期：2026-10-14 ｜ 决策人：项目所有者（对 [compare-and-improve.md](../compare-and-improve.md) 规划提出的两个决策点的显式批准）｜
> 关联文档：[0000-upstream-decisions.md](0000-upstream-decisions.md)（决策 2 的延伸）、[../compare-and-improve.md](../compare-and-improve.md) §4.7/§6、[../features-add.md](../features-add.md) `ASM-A-05`/`ASM-A-08`、承担任务 `TASK-1.03.01`（features-add-phase1.md）、`TASK-2.04.01`/`TASK-2.04.02`（features-add-phase2.md）｜
> 冻结效力：本 ADR 一经接受，`data/sources.toml` 与承担任务卡的「待裁决」状态即告解除；后续对这些数据源的任何变更（换源、扩许可档、改派生方式）需新开 ADR。

---

## 背景

[ADR-0000](0000-upstream-decisions.md) 决策 2 冻结了「词库只用开放许可证词源、以宽松许可优先」的基线，并把 THUOCL 列为 L5 领域词的**待核实**预定位源（「未确认前不得引入」）。[compare-and-improve.md](../compare-and-improve.md) 的增量规划提出两个超出 ADR-0000 现有授权范围的取数需求：

1. **两字词层换血与 3+ 字长尾扩充**需要 THUOCL（清华开源词库）——ADR-0000 的「待核实」状态需要正式解除。
2. **bigram 共现统计**（`TASK-1.03.01`）需要一个中文文本语料来统计词对共现——现有词源白名单里没有任何语料形态的源。

两个决策点于 2026-10-14 经项目所有者批准。本 ADR 记录裁决内容、合规义务与被否决的替代方案。

---

## 决策 1：THUOCL（MIT）与华宇野风（Public Domain）接入词源白名单

### 决策内容

1. **THUOCL**（`thunlp/THUOCL`，**MIT** 许可，仓库 LICENSE 文件已核实——`docs/dev/compare/rime-comparison.md` §4.7 A 档）解除 ADR-0000 的「待核实」预排除，登记为正式词源：
   - `data/sources.toml` 中以 `id = "thuocl"` 登记，`layer = "L4"`（3+ 字长尾频率带）；其按领域分文件（IT/医学/法律等）若引入，以 `id = "thuocl-<domain>"` 逐文件登记为 `layer = "L5"`（对应 ADR-0000 许可分层表的 L5 行，该行自此从「待核实」转为「已接受」）。
   - 服务于 `TASK-2.04.01`（源接入）与 `TASK-2.04.02`（两字层与多音字校验）。
2. **华宇野风系统词库**（华宇拼音论坛发布，**Public Domain**——rime-ice `Credits.md` 确认，对比报告 §4.7 A 档）登记为两字词主力源，`layer = "L2"`。PD 数据没有 SPDX 标识符，以 **`spdx = "CC0-1.0"` 等价登记**（`license` 人类可读字段写明 "Public Domain" 原始事实，`CC0-1.0` 仅作为机器可读的等价声明；来源说明写入 `docs/dev/licenses.md`）。

### 义务清单

| # | 义务 | 落地方式 | 承担任务 |
|---|---|---|---|
| W-1 | 从**原始上游**取数，不经 rime-ice 派生（rime-ice 整仓 GPL-3.0） | `fetch.sh` 的 URL 固化为原始发布地址；清洗/格式转换脚本入库并登记 `kind = "derived"` 中间产物 | `TASK-2.04.01` |
| W-2 | sha256 pin + 获取日期 | `sources.toml` 字段规则不变：文件落盘前 `sha256` 留空，落盘后 CI 强制比对 | `TASK-2.04.01` |
| W-3 | 许可登记 | `docs/dev/licenses.md` 逐源登记（来源 URL、SPDX、获取日期、哈希、用途层） | `TASK-2.04.01` |
| W-4 | 腾讯词向量维持排除 | 许可存疑（CC BY 3.0 vs CC BY-NC 4.0 冲突、官方页下线）——**本 ADR 不解除**，核实原始条款（archive.org 存档）前不引入，且需新开 ADR | — |

### 被否决的选项

| 选项 | 否决理由 |
|---|---|
| 直接取用 rime-ice 的 base/ext/tencent 词库文件 | 整仓 GPL-3.0，引入传染义务（ADR-0000 已排除，维持） |
| 腾讯词向量（约 98 万 3+ 字词） | 许可存疑（见 W-4）；其覆盖可由华宇 + THUOCL 补足 |
| 现代汉语常用词表（国家语委）、北语字频表 | 许可不明（对比报告 §4.7 B 档），收益与 jieba/THUOCL 高度重叠 |

---

## 决策 2：zh-Wikipedia（CC BY-SA 4.0）作为 bigram 共现语料

### 决策内容

`TASK-1.03.01` 的 bigram 共现统计采用 **zh-Wikipedia 语料**（Wikimedia enterprise HTML dumps 或 zh.wikipedia 文章转储，CC BY-SA 4.0；署名对象 Wikimedia Commons 的各贡献者）。

**许可分析与约束**（本决策的根据与边界）：

- CC BY-SA 4.0 的 ShareAlike 义务作用于「改编物（Adaptation）」的分发。本决策**不分发语料本身，也不分发语料的表达性衍生品**：进入 `base.dict` 的只有词对共现**统计事实**（计数与量化概率）。词频/共现统计属于事实数据，通常不构成版权意义上的改编——这与 ADR-0000 已接受的先例同构（jieba 词频、Unihan 读音等事实数据的编译）。
- 该分析存在残余法律不确定性（不同法域对事实提取与数据库特殊权利的认定不一致）。因此本决策以**可随时退出的工程纪律**落地：BIGRAM 段是容器中一个可缺席的段，退出时清空段并保持 flag 不置位即可，词库的分发许可不受影响。
- **白名单边界不变**：`scripts/check-dict-sources.sh` 的宽松 SPDX 白名单（MIT/Apache-2.0/BSD/ISC/Unicode/CC0/CC0 等价 PD 登记）**不扩展** CC BY-SA。语料不是词源，不进 `data/sources.toml` 的 `[[source]]` 表；其治理由本 ADR 承担（见 W-5）。

### 义务清单（硬约束，逐条 CI 或流程强制）

| # | 义务 | 落地方式 | 承担任务 |
|---|---|---|---|
| W-5 | 语料**永不入库、永不随包分发** | 获取走 `xtask/fetch.sh`（URL 固化 + sha256 记录于本 ADR 下方登记表 + 获取日期）；语料文件只存在于构建机的 `data/raw/corpus/`（gitignore 覆盖），仓库中只允许存在 `kind = "derived"` 的**共现计数表**（`bigram-pairs.tsv`，由本项目生成器从语料产出，随生成器演进再生成） | `TASK-1.03.01` |
| W-6 | 署名（Attribution 4.0） | `NOTICE` 与 `docs/dev/licenses.md` 登记：「bigram 统计派生自 zh-Wikipedia（CC BY-SA 4.0），© Wikipedia 贡献者」；README 许可段同步提及 | `TASK-1.03.01` |
| W-7 | 派生管线可复现 | 生成器版本 + 语料 sha256 + 参数（`BigramConfig`）三元组写入编译产物审计输出；同语料同参数双编译 sha256 一致（既有门禁） | `TASK-1.03.01` |
| W-8 | 退出路径常备 | 语料不可得或许可立场需重估时：`dictc` 以空 BIGRAM 段编译（flag 不置位），`DictLm` 走 miss 降级，`dict/bigram/absent` 诊断——词库其余部分与分发许可完全不受影响 | `TASK-1.03.01`（读端与装配侧已按此设计） |

### 语料获取登记表（fetch 时回填，格式对齐 sources.toml 字段纪律）

| 字段 | 值 |
|---|---|
| 语料 | zh-Wikipedia 文章转储 |
| 许可 | CC BY-SA 4.0（© Wikipedia 贡献者） |
| URL | *（`TASK-1.03.01` 执行时固化为具体 dump 版本地址）* |
| sha256 | *（fetch 时回填至本表与 `fetch.sh`）* |
| retrieved | *（fetch 日期）* |
| 入库 | 否（仅构建机本地，gitignore） |

### 被否决的选项

| 选项 | 否决理由 |
|---|---|
| LGPL 的八股文语料（rime-essay-simp） | 数据静态编译进二进制的 LGPL 合规成本高（ADR-0000/对比报告同结论） |
| 微软/腾讯发布的大规模共现表 | 许可不明或 NC 存疑 |
| 用 jieba 词表自身近似（bigram ≡ unigram 外推） | 无真实共现信息，收益存疑——保留为 W-8 之外的另一降级档（`DictLm` 的 v1 行为本身即是该形态） |
| 扩展 `check-dict-sources.sh` 白名单收编 CC BY-SA | 会稀释词源白名单「宽松许可」的语义（ADR-0000 决策 2 的核心）；语料治理走 ADR 更准确 |

---

## 后果（连带修改）

1. `data/sources.toml`：头注释的排除清单更新（THUOCL 移出、指向本 ADR）；`kind` 字段说明补充「derived 亦可产生自本 ADR 治理的语料」；新增 `thuocl`（及后续 `huayu`）条目。
2. `docs/dev/features-add.md`：`ASM-A-05`（语料源已裁决）、`ASM-A-08`（THUOCL 解除）状态更新。
3. `docs/dev/features-add-phase1.md`：`TASK-1.03.01` 步骤 1 由「ADR 草案 → 裁决」改为「按本 ADR 执行 fetch 与登记」。
4. `docs/dev/features-add-phase2.md`：`TASK-2.04.01` 步骤 1 同上。
5. `docs/dev/adr/README.md` 索引：登记本 ADR（已接受），并登记 0013（`set_pinned` 契约增项，`TASK-1.04.01` 认领）与 0014（`DecodeFlags::CORRECTION` 位，`TASK-3.01.01` 认领）为「已分配，未落盘」。
6. `docs/dev/licenses.md` / `NOTICE`：由承担任务在文件实际获取时回填（W-3/W-6）。
