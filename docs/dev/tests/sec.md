# rspinyin 测试用例分片 · sec（权限、隐私与合规）

> 分片版本: v1.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 平台任务: [../features-test.md](../features-test.md) ｜
> 被测基线: Rust 2024 workspace + Fcitx5 5.1.7 ｜ 关联 ADR: [../adr/0000-upstream-decisions.md](../adr/0000-upstream-decisions.md) ｜ 最后同步 Commit: `ee0dbfb` ｜
> 维护约定: 新增用例必须回写主文档第 2 节矩阵的 TC 列与维度列

## 0. 分片基线（引用主文档，不重复定义）

- **假设清单**：主文档第 1 节 + [features-test.md](../features-test.md) 第 1 节。强相关：`ASM-T-10`。
- **追踪矩阵**：主文档第 2 节的 `REQ-SEC-01` ~ `REQ-SEC-06`（`REQ-SEC-03` ~ `06` 的 P0 用例已在主文档第 3 节）。
- **用例格式与证据存盘**：主文档第 3 节 + `FEAT-TEST-P0.05.04`。
- **不可逆性纪律**：`data/sources.toml` 的注释明确——一旦 copyleft 数据混入 `base.dict`，事后剥离需重建全部词频与排序，**成本极高**。故本分片的断言全部在**构建期**强制，不依赖事后审计。

> 阶段零实测：`crates/ime-dict/src/paths.rs` **不存在**（`TASK-1.06.01` 为 `IN_PROGRESS`）；`crates/ime-core/src/privacy.rs` **不存在**（`TASK-1.06.02` 为 `PENDING`）。

---

## 1. 用例

### TC-SEC-31 XDG 目录布局与权限基线（`REQ-SEC-01`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-01` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.06.01]` ｜ `crates/ime-dict/src/paths.rs`
- **操作步骤**：
  1. 在沙盒中执行 `ensure_dirs` -> 触发存盘：`<RUN>/sec/TC-SEC-31/assertions.json`
  2. 断言 `$XDG_CONFIG_HOME/rspinyin` 与 `$XDG_DATA_HOME/rspinyin` 为 `0700`；`config.toml`/`user.redb`/`ui_takeover.json` 为 `0600`。
- **通过标准**：目录解析优先级为环境变量 → XDG 规范回退（`$HOME/.config`、`$HOME/.local/share`）；`$HOME` 也未设置时返回 `DataReadonly` 而非 panic。

### TC-SEC-32 预先存在的宽松权限被修正（`REQ-SEC-01`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-01` ｜ `sec` | 边界与容错 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.06.01]`
- **操作步骤**：
  1. 预先创建 `0644` 的 `user.redb` -> 触发存盘：`<RUN>/sec/TC-SEC-32/assertions.json`
  2. 断言被修正为 `0600` 并记 `data/perms/fixed`。
- **通过标准**：创建时即传 `mode(0o600)`（避免"先创建再 chmod"的竞态窗口）；已存在的文件强制 `chmod` 并告警。

### TC-SEC-33 目录不可写时进入只读模式（`REQ-SEC-01`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-01` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.06.01]`
- **操作步骤**：
  1. 把数据目录设为不可写 -> 触发存盘：`<RUN>/sec/TC-SEC-33/assertions.json`
  2. 断言进入只读模式、输入完整可用、`StatusStrip.readonly == true`。
- **通过标准**：`ASM-15`——绝不因写失败而阻断输入。

### TC-SEC-34 符号链接逃逸防护（`REQ-SEC-01`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-01` ｜ `sec` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.06.01]`
- **操作步骤**：
  1. 把 `~/.local/share/rspinyin` 软链到 `/tmp` -> 触发存盘：`<RUN>/sec/TC-SEC-34/assertions.json`
  2. 断言拒绝写入该路径并记 `data/path/symlink`，进入只读模式。
- **通过标准**：`ensure_dirs` 检查目标路径的**每一段**不是符号链接。

### TC-SEC-35 不递归修改用户目录权限（`REQ-SEC-01`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-01` ｜ `sec` | 边界与容错 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.06.01]`
- **操作步骤**：
  1. 在 `$HOME` 下预置若干文件并记录权限 -> 执行 `ensure_dirs` -> 触发存盘：`<RUN>/sec/TC-SEC-35/assertions.json`
  2. 断言只对 `rspinyin` 下的**直接子项**操作，`$HOME` 其他文件权限不变。
- **通过标准**：**绝不**递归 `chmod` 用户已有目录；**绝不**删除或覆盖用户文件。

### TC-SEC-36 敏感输入上下文抑制学习（`REQ-SEC-02`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-02` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.06.02]` ｜ `crates/ime-core/src/privacy.rs`
- **操作步骤**：
  1. 在 `CapabilityFlag::Password` 置位的上下文中输入 20 个拼音串并上屏 -> 触发存盘：`<RUN>/sec/TC-SEC-36/assertions.json`
  2. 断言 `user.redb` 的记录数增量 = **0**。
- **通过标准**：这是三层抑制中**最重要的一条**——用户输入密码时打出的拼音组合不进入词频库。`should_learn` ≤ 200ns。

### TC-SEC-37 敏感会话的日志零痕迹（`REQ-SEC-02`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-02` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.06.02]` + `[待实现: TASK-1.08.01]`
- **操作步骤**：
  1. 同上场景，扫描全部日志文件 -> 触发存盘：`<RUN>/sec/TC-SEC-37/assertions.json`
  2. 断言不出现输入串的任何**长度 ≥ 3 的子串**；断言敏感会话的事件降级为 `session=redacted`（**不含输入长度**）。
- **通过标准**：敏感上下文下连输入长度也不记录（长度本身可能泄露密码长度）。

### TC-SEC-38 敏感会话的崩溃文件零痕迹（`REQ-SEC-02`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-02` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.06.02]` + `[待实现: TASK-1.08.02]`
- **操作步骤**：
  1. 在密码框中输入后人为触发一次 panic -> 触发存盘：`<RUN>/sec/TC-SEC-38/assertions.json`
  2. 断言崩溃文件中不出现输入串的任何长度 ≥ 3 的子串。
- **通过标准**：`CrashRecord` 的 `context` 字段走**白名单构造**（只允许 `session_state`/`revision`/`backend_id`/`raw_len`/`candidate_count`）。

### TC-SEC-39 应用黑名单（`REQ-SEC-02`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-02` ｜ `sec` | 边界与容错 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.06.02]`
- **操作步骤**：
  1. 把某应用加入黑名单（大小写不敏感子串匹配）-> 触发存盘：`<RUN>/sec/TC-SEC-39/assertions.json`
  2. 断言该应用下同样抑制学习与日志。
- **通过标准**：默认黑名单**为空**——依赖 `CapabilityFlag::Password` 而非猜测密码管理器。`app_id` 在进入 `ime-core` 前哈希为 `u64`。

### TC-SEC-40 `CapabilityFlag` 局限的如实声明（`REQ-SEC-02`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-02` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.06.02]` ｜ `docs/dev/privacy.md`
- **操作步骤**：
  1. 检查 `privacy.md` -> 触发存盘：`<RUN>/sec/TC-SEC-40/assertions.json`
  2. 断言含数据流向图、存储位置、权限、以及**`CapabilityFlag::Password` 是"尽力而为"信号的明确声明**（未设置该标志的密码框我们无法识别）。
- **通过标准**：不给用户"绝对安全"的错觉。

### TC-SEC-41 敏感模式下候选框仍显示（`REQ-SEC-02` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-02` ｜ `sec` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.06.02]`
- **操作步骤**：
  1. 在密码框中输入 -> 触发存盘：`<RUN>/sec/TC-SEC-41/01_default.png`
  2. 断言候选框**显示**（用户需要看到自己打的是什么）。
- **通过标准**：`should_show_ui` 默认 `true`；企业环境可通过 `[privacy] disable_ui_on_password = true` 改为完全透传。

### TC-SEC-42 隐私配置变更立即生效（`REQ-SEC-02` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-02` ｜ `sec` | 边界与容错 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.06.02]`
- **操作步骤**：
  1. 运行中修改黑名单 -> 触发存盘：`<RUN>/sec/TC-SEC-42/assertions.json`
  2. 断言下一个 `KeyEvent` 即生效（无需重启）。
- **通过标准**：隐私配置的变更不要求重启。

### TC-SEC-43 词源白名单的排除清单强制（`REQ-SEC-04` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-04` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 检查 `data/sources.toml` 的注释与内容 -> 触发存盘：`<RUN>/sec/TC-SEC-43/assertions.json`
  2. 断言排除清单被显式登记：`luna-pinyin`(LGPL-3.0)、CC-CEDICT(CC BY-SA 4.0)、商业词库、研究用途词表、THUOCL（许可未核实）。
- **通过标准**：注释中明确"Excluded by ADR-0000; do not add"。

### TC-SEC-44 许可归属徽章与 `OB-1`~`OB-6` 登记（`REQ-SEC-05` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-05` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.06.03]`
- **操作步骤**：
  1. 检查 README 双语徽章 + `licenses.md` 的六项义务登记 -> 触发存盘：`<RUN>/sec/TC-SEC-44/assertions.json`
  2. 断言徽章与 `https://slint.dev` 链接可达；断言 `OB-1`~`OB-6` 逐条含条款依据、核对方式、复核结论。
- **通过标准**：**必须直接读取 Slint 发行包内的许可原文**（`LICENSES/LicenseRef-Slint-Royalty-free-2.0.txt`），不得只引用二手解读（ADR-0000 的免责声明）。

### TC-SEC-45 `strip` 与 `panic = "abort"` 的禁止（`REQ-SEC-06` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-06` ｜ `sec` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 检查 `Cargo.toml` 的 `[profile.release]` -> 触发存盘：`<RUN>/sec/TC-SEC-45/assertions.json`
  2. 断言**不含** `strip`（任何形式）与 `panic = "abort"`。
- **通过标准**：`strip = true` 与 `strip = "symbols"` 都会让 rustc 传 `--strip-all`，丢弃 `fcitx_addon_factory_instance`（`ADR-0002`）；`panic = "abort"` 会破坏 FFI 边界的 `catch_unwind`（`features.md` 2.2.3）。剥离改在**打包时**进行，并在 `nm -D` 校验后交付。

---

## 2. 分片出口准则

1. `REQ-SEC-01` 与 `REQ-SEC-02` 的 10 条用例在 `TASK-1.06.01` / `TASK-1.06.02` 落地后转为 `[可执行]`。
2. `TC-SEC-37`/`TC-SEC-38` 的零痕迹断言同时依赖 `TASK-1.08.01`（日志）与 `TASK-1.08.02`（崩溃），三者需协同验收。
3. `TC-SEC-44` 的 `OB-1` 徽章必须在 Phase 1 出口前落地（否则带病发布）。
4. 主文档矩阵的 `REQ-SEC-01`、`REQ-SEC-02` 行可执行性列更新为 `✅`。
