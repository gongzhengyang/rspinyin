# rspinyin 测试用例分片 · sec（隐私、许可与依赖纪律）

> 分片版本: v3.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 执行器必读: [../test-architecture.md](../test-architecture.md) ｜ 模块形态: Headless ｜ 逻辑自检: 已通过 ｜ 维护约定: 用例状态变更须同步主文档矩阵
>
> 基线引用：`ASM-T-06/07`。模块背书 = `cargo nextest -p ime-dict/-p ime-fcitx5/-p ime-core` 相关套件 + `just audits`（11 脚本族）。**吸收** 字段登记取代的 v2.0 用例号。本分片含 ADR-0012 后的新增卡 `TC-SEC-008`。

---

#### TC-SEC-001 XDG 数据目录权限基线

- **基本属性**：`REQ-SEC-01` ｜ `ASM-T-06` ｜ sec ｜ ②③ ｜ nextest(-p ime-dict paths) ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `requires_screenshots: false` ｜ `[]` ｜ 吸收：TC-SEC-31~35 ｜ 锚点：`crates/ime-dict/src/paths.rs`、`paths/` ｜ 背书：paths 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：临时 XDG 树；预置宽松权限 fixture。
- **操作步骤与物理存盘动作**：
  1. 目录 0700 / 文件 0600 基线 → `execution_log.txt`。
  2. 预先存在的宽松权限被修正；符号链接逃逸防护 → 记录。
  3. 不递归修改用户目录权限；目录不可写 → readonly → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：权限修正不越界（只动自己的文件）。
  - **[Headless 断言]**：readonly 降级报告 `data/readonly-mode`。
  - **[性能断言]**：权限检查在启动路径预算内。

#### TC-SEC-002 敏感输入上下文抑制学习

- **基本属性**：`REQ-SEC-02` ｜ `ASM-T-01` ｜ sec ｜ ①②③ ｜ nextest + 沙盒 ｜ `[Acceptance 验收套件]` ｜ P0 ｜ `true` ｜ `["01_sensitive_composing.png"]` ｜ 吸收：TC-SEC-36~42 ｜ 锚点：`crates/ime-fcitx5/src/privacy_impl/`、`crates/ime-core/src/privacy.rs` ｜ 背书：privacy 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：沙盒会话 + 黑名单 fixture；用户库从零。
- **操作步骤与物理存盘动作**：
  1. 敏感上下文（密码框类）输入 → 学习完全抑制 → 存 `01_sensitive_composing.png`（候选窗仍显示）+ `execution_log.txt`。
  2. 日志/崩溃文件零痕迹（整会话降级，连长度都删）→ 记录。
  3. 应用黑名单生效；隐私配置变更立即生效 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`InputContextKind` fail-closed（宿主未报一律不学习）；显示 fail-open（密码框照常出候选）。
  - **[视觉断言]**：敏感模式候选窗行为与普通模式一致（无降级闪烁）。
  - **[性能断言]**：hash 化 app 标识在键路径预算内。

#### TC-SEC-003 零网络断言

- **基本属性**：`REQ-SEC-03` ｜ `ASM-T-09` ｜ sec ｜ ①② ｜ `just audits` + `runtime-socket-check` ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-SEC-11~15 ｜ 锚点：`scripts/check-no-network.sh`、`Cargo.lock` ｜ 背书：脚本自测
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：干净 resolve graph。
- **操作步骤与物理存盘动作**：
  1. 依赖闭包无网络库（cargo metadata 断言）→ `execution_log.txt`。
  2. 网络 crate 的 feature 门控也被拦截；运行期零外部 socket → 记录。
  3. 无自动更新/遥测通道 → `assertions.json`（budget_keys: `net_sockets`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`BUDGET-NET-01` 语义全量成立。
  - **[Headless 断言]**：脚本自测通过（含违规 fixture 检出）。
  - **[性能断言]**：不适用。

#### TC-SEC-004 词源白名单与许可纪律（ADR-0000/0012 执行面）

- **基本属性**：`REQ-SEC-04` ｜ `ASM-T-07` ｜ sec ｜ ①② ｜ `check-dict-sources.sh --self-test` + nextest(-p xtask) ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-SEC-16~20、43、TC-DICT-45 ｜ 锚点：`scripts/check-dict-sources.sh`、`data/sources.toml` ｜ 背书：脚本自测（5 violation classes）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：allowlist fixture 副本。
- **操作步骤与物理存盘动作**：
  1. 全部来源宽松许可（SPDX ∈ MIT/Apache-2.0/BSD/ISC/Unicode/CC0-1.0）→ `execution_log.txt`。
  2. 排除清单强制（luna-pinyin/CC-CEDICT/商业词库/腾讯 NC 存疑）；sha256 pin 与未完成条目必红 → 记录。
  3. 无未登记 TSV；ADR-0012 语料治理边界（语料不入 allowlist、`data/raw/corpus/` gitignore）→ `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`thuocl`（MIT）/`huayu`（PD→CC0-1.0）已登记且校验 PASS（6 raw / 8 declared 口径随取数演进）。
  - **[Headless 断言]**：自测 5 类违规全检出。
  - **[性能断言]**：不适用。

#### TC-SEC-005 Slint API 不泄漏（OB-4）与许可归属

- **基本属性**：`REQ-SEC-05` ｜ `ASM-T-06` ｜ sec ｜ ① ｜ `check-slint-leak.sh` + 文档走查 ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-SEC-21~25、44 ｜ 锚点：`scripts/check-slint-leak.sh`、`README.md` 徽章、`docs/dev/licenses.md` ｜ 背书：脚本自测
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：干净工作树。
- **操作步骤与物理存盘动作**：
  1. `ime-ui` 公共 API 无 `slint::*` → `execution_log.txt`。
  2. 徽章可达（`repository` 与 README 同指）；`OB-3` 嵌入式排除声明存在 → 记录。
  3. 许可原文摘录（非二手解读）→ `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：OB-1~OB-6 义务登记齐备（ADR-0000 表）。
  - **[Headless 断言]**：泄漏脚本自测通过。
  - **[性能断言]**：不适用。

#### TC-SEC-006 依赖单向性与分层

- **基本属性**：`REQ-SEC-06` ｜ `ASM-T-09` ｜ sec ｜ ① ｜ `check-deps.sh` ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-SEC-26~30 ｜ 锚点：`scripts/check-deps.sh`、workspace `Cargo.toml:8-45` ｜ 背书：脚本自测
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：干净工作树。
- **操作步骤与物理存盘动作**：
  1. 层序 `ime-types ← core ← dict ← config ← ui ← fcitx5` 无反向边 → `execution_log.txt`。
  2. `ime-ui` 不依赖解码器/FST 句柄；`ime-types` 叶子化 → 记录。
  3. 新增依赖走基线评审（`Cargo.toml` 注释齐备）→ `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：任何反向边 = 构建失败。
  - **[Headless 断言]**：自测含违规 fixture 检出。
  - **[性能断言]**：不适用。

#### TC-SEC-007 交付链路隐私抽检

- **基本属性**：`REQ-SEC-07` ｜ `ASM-T-06` ｜ sec ｜ ② ｜ 安装后审计 ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-INFRA-60 ｜ 锚点：`xtask/src/install/`、`packaging/` ｜ 背书：install 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：`DESTDIR` 安装树。
- **操作步骤与物理存盘动作**：
  1. 包内无用户数据/输入内容残留（strings 抽查）→ `execution_log.txt`。
  2. 日志/镜像/读回 transcript 的 0600 纪律在安装文档中登记 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：交付物 = 代码 + 词库，无隐私面。
  - **[Headless 断言]**：抽查脚本退出码 0。
  - **[性能断言]**：不适用。

#### TC-SEC-008 词源扩展执行面（ADR-0012 新增）

- **基本属性**：`REQ-SEC-04` ｜ `ASM-T-07` ｜ sec ｜ ①② ｜ `check-dict-sources.sh` + `git check-ignore` ｜ `[Smoke 冒烟门禁]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：无（新增，覆盖 2026-10-14 ADR-0012 裁决面）｜ 锚点：`data/sources.toml:105-137`、`docs/dev/adr/0012-corpus-and-word-sources.md` ｜ 背书：校验器 PASS（6 raw / 8 declared）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：当前 allowlist。
- **操作步骤与物理存盘动作**：
  1. `thuocl`/`huayu` 条目字段齐备且 sha256 空（未取数态合法）→ `execution_log.txt`。
  2. 文件落盘后 CI 必红直到 hash 回填（fixture 演练）→ 记录。
  3. 语料治理：`data/raw/corpus/` 被 gitignore；语料 URL/sha256 回填 ADR-0012 登记表（W-5）→ `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`NOTICE`/`licenses.md` 的署名条目随取数落地（W-3/W-6，未取数时为登记在案的待办而非缺陷）。
  - **[Headless 断言]**：腾讯词向量维持排除（ADR-0012 W-4）。
  - **[性能断言]**：不适用。
