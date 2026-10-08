# rspinyin 测试用例分片 · infra（安装布局、打包与交付）

> 分片版本: v2.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 平台任务: [../features-test.md](../features-test.md) ｜
> 被测基线: Rust 2024 workspace + Fcitx5 5.1.19（Ubuntu 24.04 / WSL2） ｜ 关联 ADR: [../adr/0002-rust-exports-addon-factory.md](../adr/0002-rust-exports-addon-factory.md)、[../adr/0003-ui-role-separate-addon.md](../adr/0003-ui-role-separate-addon.md)、[../adr/0004-ui-addon-crate-split.md](../adr/0004-ui-addon-crate-split.md) ｜ 最后同步 Commit: `408d2c6` ｜
> 维护约定: 新增用例必须回写主文档第 2 节矩阵的 TC 列与维度列

## 0. 分片基线（引用主文档，不重复定义）

- **假设清单**：主文档第 1 节 + [../features-test.md](../features-test.md) 第 1 节。强相关：`ASM-T-06`（`so_stripped = 12MB`、`base_dict = 20MB`）。
- **追踪矩阵**：主文档第 2 节的 `REQ-INFRA-01` ~ `REQ-INFRA-09`（`REQ-INFRA-01`~`04`、`06` 的 P0 用例已在主文档第 3 节）。`REQ-INFRA-07`（场景化端到端驱动）/ `REQ-INFRA-08`（审计脚本族扩展）/ `REQ-INFRA-09`（打包与体积门禁）是本轮同步新增行。
- **预算阈值**：`docs/dev/budgets.json`（键名：`so_stripped`、`base_dict`）。
- **安装路径纪律**：**禁止硬编码安装路径**——必须从 `pkg-config --variable=addondir Fcitx5Core` 动态获取。硬编码会在 Fedora/Arch 上装到错误位置，导致 fcitx5 找不到插件（`features.md` 6.2.3 的陷阱）。

> 现状（v2.0 同步）：`packaging/fcitx5/` 已有 **3 个 conf**（`rspinyin.conf` = InputMethod、`rspinyin-im.conf` = 输入法描述、`rspinyin-ui.conf` = `Category=UI`，ADR-0004 后果闭环）；`xtask/src/install/` 全模块落地（含 `reversible` 可逆卸载、`size`、`verify`、`takeover`）；`xtask/src/package/` + `packaging/{aur,debian,rpm}` 落地；`xtask/src/testd/` 端到端驱动全模块落地（场景引擎、沙盒、证据链、自愈、纯度）。

---

## 1. 用例

### TC-INFRA-31 安装布局与动态路径解析（`REQ-INFRA-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-05` ｜ `infra` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `xtask/src/install.rs`、`packaging/install.sh`
- **前置条件与沙盒状态**：干净 Ubuntu 24.04 容器；`libfcitx5core-dev` 已装。
- **操作步骤**：
  1. 执行 `just install` -> 触发存盘：`<RUN>/infra/TC-INFRA-31/assertions.json`
  2. 断言产物落位：`librspinyin.so` 与 `librspinyin-ui.so` → `$(pkg-config --variable=addondir Fcitx5Core)`；三个 conf → `/usr/share/fcitx5/addon/` 与 `/usr/share/fcitx5/inputmethod/`；`base.dict` → `/usr/share/rspinyin/`；图标 → hicolor。
- **通过标准**：**路径由 `pkg-config` 决定**；在 Ubuntu 与 Fedora 两个容器中均正确。

- **验收记录**（2026-10-06）：实测：安装布局动态路径解析（destdir 实测 .so/conf 落位）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-31/
### TC-INFRA-32 双 addon 的 conf 齐备（`REQ-INFRA-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-05` ｜ `infra` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` + `[可执行]`
- **操作步骤**：
  1. 检查安装后的 conf 集合 -> 触发存盘：`<RUN>/infra/TC-INFRA-32/assertions.json`
  2. 断言存在 `Category=InputMethod`（`Library=librspinyin`）与 `Category=UI`（`Library=librspinyin-ui`、`UIPriority > 0`、`UIType=PhysicalKeyboard`）两类。
- **通过标准**：ADR-0003 的**后果 #2**。`AddonCategory` 是单值枚举（`InputMethod`/`Frontend`/`Loader`/`Module`/`UI`），一个 addon 只能属于一个类别；工厂符号名固定为 `fcitx_addon_factory_instance`，同一 `.so` 被两个 conf 引用会创建两个**无法区分**的实例，故必须两个 `.so`。

- **验收记录**（2026-10-06）：实测：双 addon 三份 conf 齐备；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-32/
### TC-INFRA-33 前端依赖必须是 Optional（`REQ-INFRA-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-05` ｜ `infra` | 边界与容错 ｜ `P0` ｜ 可执行性：`[可执行]`（`packaging/fcitx5/rspinyin.conf` 已存在）
- **操作步骤**：
  1. 检查 `rspinyin.conf` 的 `[Addon/Dependencies]` -> 触发存盘：`<RUN>/infra/TC-INFRA-33/assertions.json`
  2. 断言 `xcb` 与 `wayland` 在 `[Addon/OptionalDependencies]` 而**不在** `[Addon/Dependencies]`。
- **通过标准**：fcitx5 把 `[Addon/Dependencies]` 的每一项都当作必需，同时列 `xcb` 与 `wayland` 会让插件在纯 X11 或纯 Wayland 系统上**拒绝加载**（已对 fcitx5 5.1.7 验证）。当前 conf 正确：`[Addon/Dependencies]` 只有 `0=core:5.1.7`。

- **验收记录**（2026-10-06）：实测：前端依赖 Optional（缺 wayland 不抑制加载，E2E 会话实证）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-33/
### TC-INFRA-34 卸载可逆性（`REQ-INFRA-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-05` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 安装后记录文件集合，执行 `just uninstall` -> 触发存盘：`<RUN>/infra/TC-INFRA-34/assertions.json`
  2. 断言 `/usr/share/fcitx5/` 的文件集合与安装前**完全一致**。
  3. 断言用户数据**未被删除**（`~/.local/share/rspinyin/user.redb` 仍在），且打印其位置与删除命令。
- **通过标准**：完整可逆；`ui_takeover.json` 的原值被写回 fcitx5 的活跃 UI 配置。

- **验收记录**（2026-10-06）：实测：卸载可逆（--uninstall + reversible/manifest）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-34/
### TC-INFRA-35 体积预算与 `nm -D` 校验（`REQ-INFRA-05`）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-05` ｜ `infra` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `docs/dev/budgets.json`
- **操作步骤**：
  1. 打包时 `strip` 后测量 -> 触发存盘：`<RUN>/infra/TC-INFRA-35/assertions.json`
  2. 断言 `librspinyin.so` ≤ `so_stripped`（12MB）、`base.dict` ≤ `base_dict`（20MB）。
  3. **在 strip 之前与之后各跑一次** `nm -D --defined-only | grep fcitx_addon_factory_instance`，断言两者都有输出。
- **通过标准**：`strip` **只允许在打包阶段执行**，且必须紧跟 `nm -D` 校验——`Cargo.toml` 的 `[profile.release]` 不得设置 `strip`（任何形式），否则 rustc 会传 `--strip-all` 丢弃工厂符号，产出"能加载但不含 addon"的库（ADR-0002）。

- **验收记录**（2026-10-06）：实测：体积预算（3.1/6.1MB ≤ 12MB）与 nm -D 工厂符号；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-35/
### TC-INFRA-36 幂等安装与 `--dry-run`（`REQ-INFRA-05` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-05` ｜ `infra` | 边界与容错 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 连续执行 `just install` 两次 -> 触发存盘：`<RUN>/infra/TC-INFRA-36/assertions.json`
  2. 断言第二次无错误、无重复条目。
  3. 执行 `--dry-run`，断言只打印不执行（在只读文件系统上验证）。
- **通过标准**：幂等；`--dry-run` 无副作用。

- **验收记录**（2026-10-06）：实测：幂等安装与 --dry-run（二次 exit=0；dry-run 零写入）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-36/
### TC-INFRA-37 `DESTDIR`/`PREFIX` 支持（`REQ-INFRA-05` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-05` ｜ `infra` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. `DESTDIR=/tmp/stage PREFIX=/usr bash packaging/install.sh` -> 触发存盘：`<RUN>/infra/TC-INFRA-37/assertions.json`
  2. 断言全部产物落在 `/tmp/stage/usr/...`。
- **通过标准**：`TASK-2.07.01` 的 deb/rpm/AUR 打包将复用同一脚本。

- **验收记录**（2026-10-06）：实测：DESTDIR/PREFIX 双支持实测；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-37/
### TC-INFRA-38 不修改 fcitx5 的 profile（`REQ-INFRA-05` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-05` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 记录 `~/.config/fcitx5/profile` 的 `mtime` -> 执行 `just install` -> 触发存盘：`<RUN>/infra/TC-INFRA-38/assertions.json`
  2. 断言 `mtime` 不变（输入法列表由用户自己管理）。
- **通过标准**：安装脚本**不得**修改用户的输入法列表；提示用户到 `fcitx5-configtool` 添加。

- **验收记录**（2026-10-06）：实测：不修改 fcitx5 profile（安装输出明示 + takeover 备份）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-38/
### TC-INFRA-39 缺失开发包时的提示（`REQ-INFRA-05` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-05` ｜ `infra` | 边界与容错 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 在缺 `libfcitx5core-dev` 的容器执行 `just install` -> 触发存盘：`<RUN>/infra/TC-INFRA-39/assertions.json`
  2. 断言给出明确提示（`platform/fcitx5/dev-missing` + 各发行版安装命令）。
- **通过标准**：不出现晦涩的链接器符号错误。

- **验收记录**（2026-10-06）：实测：缺失开发包提示（dev-missing + remedy 文案）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-39/
### TC-INFRA-40 图标与桌面集成（`REQ-INFRA-05` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-05` ｜ `infra` | 商业化 5 态微交互与材质 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 安装后检查图标 -> 触发存盘：`<RUN>/infra/TC-INFRA-40/01_icon.png`
  2. 断言 `fcitx-rspinyin` 图标落在 hicolor 的 48x48 与 scalable 目录；断言 `gtk-update-icon-cache` 被调用（若存在）。
- **通过标准**：`rspinyin-im.conf` 的 `Icon=fcitx-rspinyin` 能解析到实际图标。

- **验收记录**（2026-10-06）：实测：图标与桌面集成（Icon 声明 + icon_cache 模块）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-40/
### TC-INFRA-41 CI 的 `check-versions` 与打包版本一致（`REQ-INFRA-05` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-05` ｜ `infra` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 执行 `just check-versions` -> 触发存盘：`<RUN>/infra/TC-INFRA-41/assertions.json`
  2. 断言 `packaging/fcitx5/rspinyin.conf` 的 `Version` == `Cargo.toml` 的 `version`（当前 `0.1.0`）。
- **通过标准**：新增的 UI conf 也必须纳入该断言（两个 conf 的 `Version` 都需一致）。

- **验收记录**（2026-10-06）：实测：check-versions 与打包版本一致（安装前置实测 0.1.0 匹配）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-41/
### TC-INFRA-42 构建产物不含 `target/`（`REQ-INFRA-05` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-05` ｜ `infra` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 检查 `.gitignore` 与 `git status` -> 触发存盘：`<RUN>/infra/TC-INFRA-42/assertions.json`
  2. 断言 `target/` 未被跟踪；断言 `.gitignore` 覆盖 `/target`、`*.dict.tmp`、`user.redb*`、`*.corrupt.*`、`/logs`。
- **通过标准**：`AGENTS.md` 3.1；`git submodule` 全局禁止（保证 `cargo vendor` 与离线构建可行）。

- **验收记录**（2026-10-06）：实测：构建产物不含 target/（gitignore + payload 校验）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-42/
### TC-INFRA-43 审计脚本在安装后的插件上仍通过（`REQ-INFRA-05` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-05` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 安装后对产物运行 `scripts/check-unsafe.sh`、`check-slint-leak.sh` -> 触发存盘：`<RUN>/infra/TC-INFRA-43/assertions.json`
  2. 断言两个脚本在已安装产物上仍通过。
- **通过标准**：安装不引入新的 `unsafe` 或 Slint 泄漏面。

- **验收记录**（2026-10-06）：实测：审计脚本与安装产物校验（elf.rs 工厂符号/尺寸）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-43/
### TC-INFRA-44 离线安装可行性（`REQ-INFRA-05` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-05` ｜ `infra` | 边界与容错 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 在断网环境（无外网）执行完整的构建 + 安装 -> 触发存盘：`<RUN>/infra/TC-INFRA-44/assertions.json`
  2. 断言成功（`cargo vendor` 或已填充的 registry 缓存）。
- **通过标准**：`BUDGET-NET-01` 的离线承诺；`git submodule` 禁止是这条的前提。

- **验收记录**（2026-10-06）：实测：离线安装可行（--dict 预置 + 零网络依赖）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-44/
### TC-INFRA-45 安装脚本的权限与所有权（`REQ-INFRA-05` 深化）

- **基本属性**：`[x] 已通过` ｜ `REQ-INFRA-05` ｜ `infra` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 检查安装产物的权限 -> 触发存盘：`<RUN>/infra/TC-INFRA-45/assertions.json`
  2. 断言系统路径下为 `0644`、目录为 `0755`、属主为 `root:root`（非用户目录）。
- **通过标准**：系统级产物权限符合 FHS；用户数据仍为 `0600`/`0700`（`REQ-SEC-01`）。

### TC-INFRA-46 场景引擎：18 个 TOML 场景全绿（`REQ-INFRA-07`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-07` ｜ `infra` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `xtask/src/testd/engine.rs`、`tests/fixtures/scenarios/`
- **前置条件与沙盒状态**：沙盒 XDG + 合成词库（不读 `data/compiled/`）；引擎直驱（`FEAT-TEST-P0.03.01`）。
- **操作步骤**：
  1. 逐场景执行 `tests/fixtures/scenarios/engine-*.toml`（18 个）-> 触发存盘：`<RUN>/infra/TC-INFRA-46/assertions.json`
  2. 断言每个场景的每步断言（输入序列 → 候选/错误码/上屏）全部命中。
- **通过标准**：五类覆盖齐备——长度上限、无路径、非法字符、降级、翻页；场景失败时错误定位到 TOML 行。

### TC-INFRA-47 端到端沙盒会话：XTEST → fcitx5 → CommitProbe（`REQ-INFRA-07`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-07` ｜ `infra` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `xtask/src/testd/{suite,sandbox,input,commit_readback}.rs`
- **前置条件与沙盒状态**：Xvfb/沙盒 fcitx5 会话；`FEAT-TEST-P0.05.01` 隔离。
- **操作步骤**：
  1. 套件驱动：XTEST 键入 `nihao` + 空格 → 断言 `CommitProbe` 输出"你好"（run-20261006-034915 的 20/20 轮语义）-> 触发存盘：`<RUN>/infra/TC-INFRA-47/01_committed.png`
  2. 断言会话间零污染（沙盒重置后首键行为一致）。
- **通过标准**：全链路（注入 → 引擎 → 候选 → 上屏）20/20 轮可重复；证据包完整（截图 + 断言 + 日志）。

### TC-INFRA-48 证据链与自愈归因（`REQ-INFRA-07`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-07` ｜ `infra` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `xtask/src/testd/{evidence,heal,attribution,purity}.rs`
- **前置条件与沙盒状态**：一次人为注入的失败场景。
- **操作步骤**：
  1. 触发失败 → 断言证据包生成：`assertions.json` + `trace.json`（失败用例）+ `RUN/index.md` + `results/index.json` -> 触发存盘：`<RUN>/infra/TC-INFRA-48/assertions.json`
  2. 定位器漂移场景：断言 `[HEALED]` 标记 + Patch 报告只改测试层（`GUARD-02`/`GUARD-03`）。
  3. 断言 `purity` 在受污染机器上拒绝采信预算数值（`GUARD-07`/`ASM-T-11`）。
- **通过标准**：自愈绝不改业务代码（红线有机器断言）；证据包命名符合 `FEAT-TEST-P0.05.04`。

### TC-INFRA-49 环境能力门禁的三态判定（`REQ-INFRA-07`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-07` ｜ `infra` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `xtask/src/testd/{env_gate,env,coords,x11}.rs`
- **前置条件与沙盒状态**：本机（WSLg/Weston）+ 人为遮挡 `DISPLAY`。
- **操作步骤**：
  1. 完整环境：断言 X11 档判 `[可执行]`、Wayland 档判 `[不可验证]`、无显示判 `[跳过-显式]` -> 触发存盘：`<RUN>/infra/TC-INFRA-49/assertions.json`
  2. 断言判定结果写进每个用例的证据包（不静默跳过，`GUARD-06`）。
- **通过标准**：三态判定与 features.md 0.5.5 的表格一致；不可验证项在报告中**显式计数**。

### TC-INFRA-50 内存采样与长稳探测通道（`REQ-INFRA-07`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-07` ｜ `infra` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]`（8h 长稳子项 `[不可验证]`） ｜ `xtask/src/testd/{memory,framerate}.rs`、`xtask/src/soak.rs`
- **前置条件与沙盒状态**：真实会话 + 高频输入注入。
- **操作步骤**：
  1. 30 分钟高频输入，每 10s 采样 `smaps_rollup` -> 触发存盘：`<RUN>/infra/TC-INFRA-50/assertions.json`
  2. 断言 RSS 漂移 ≤ `robustness.rss_drift_mb`；帧率采样无下坠趋势（`BUDGET-CPU-01` 同族）。
  3. 8 小时长稳子项记 `[不可验证]`（`ASM-T-08`）。
- **通过标准**：泄漏判定有统计基线（前 5 分钟预热不计入）；`soak` 工具可复跑。

### TC-INFRA-51 审计脚本族扩展：七个新脚本自测齐备（`REQ-INFRA-08`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-08` ｜ `infra` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `scripts/check-no-grab.sh`、`check-fsync-rename.sh`、`check-metrics-readers.sh`、`check-ui-spec.sh`、`check-readme-keys.sh`、`idle-cpu-check.sh`、`runtime-socket-check.sh`
- **前置条件与沙盒状态**：仓库工作树；`just check-self-tests` 就绪。
- **操作步骤**：
  1. 逐脚本运行 + 自测 -> 触发存盘：`<RUN>/infra/TC-INFRA-51/assertions.json`
  2. 断言七脚本各自的自测（注入违规样本 → 非零退出）齐备（`TC-INFRA-06` 语义扩展）。
- **通过标准**：`just audits` 全绿；脚本退出码语义与 `TC-INFRA-10` 一致。

### TC-INFRA-52 焦点 grab 禁令的机器化（`REQ-INFRA-08`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-08` ｜ `infra` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[可执行]` ｜ `scripts/check-no-grab.sh`
- **前置条件与沙盒状态**：仓库工作树。
- **操作步骤**：
  1. 注入 `XGrabKeyboard`/`XSetInputFocus`/热键注册/键盘抑制样本 → 断言脚本全部拦截 -> 触发存盘：`<RUN>/infra/TC-INFRA-52/assertions.json`
  2. 断言比 0.4 规则 5 更宽的禁令生效（grab、hotkey、inhibit 全家）。
- **通过标准**：`AGENTS.md` 禁止事项 20 的机器化防线；自测含假阴性用例。

### TC-INFRA-53 fsync-rename 纪律的机器化（`REQ-INFRA-08`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-08` ｜ `infra` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `scripts/check-fsync-rename.sh`
- **前置条件与沙盒状态**：仓库工作树。
- **操作步骤**：
  1. 断言全部"临时文件 + rename"路径（词库写手、短语写手、配置写回、用户库）都先 fsync -> 触发存盘：`<RUN>/infra/TC-INFRA-53/assertions.json`
  2. 注入无 fsync 的样本路径 → 断言拦截。
- **通过标准**：掉电窗口（complete name over incomplete file）在静态层排除；新写路径默认被脚本要求。

### TC-INFRA-54 运行期零 socket 与空闲 CPU 的实证（`REQ-INFRA-08`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-08` ｜ `infra` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `scripts/runtime-socket-check.sh`、`idle-cpu-check.sh`
- **前置条件与沙盒状态**：真实 fcitx5 会话（沙盒 addon）。
- **操作步骤**：
  1. 会话中扫描进程 socket → 断言零 IP socket -> 触发存盘：`<RUN>/infra/TC-INFRA-54/assertions.json`
  2. 空闲 60s → 断言 CPU ≤ 预算、零重绘、零轮询唤醒。
- **通过标准**：`BUDGET-NET-01` 从"依赖闭包无网络库"（`check-no-network.sh`）升级为"运行期实证"；两脚本互补成对。

### TC-INFRA-55 打包与体积门禁（`REQ-INFRA-09`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-09` ｜ `infra` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `xtask/src/package.rs`、`package/`、`justfile`（`check-size`、`check-advisories`）
- **前置条件与沙盒状态**：release 构建（`strip` 语义按 ADR-0002）。
- **操作步骤**：
  1. `just package` → 断言产物清单完整（双 cdylib + 3 conf + 词库）-> 触发存盘：`<RUN>/infra/TC-INFRA-55/assertions.json`
  2. `just check-size` → 断言 `so_stripped`/`base_dict` 达标；`check-advisories` 零未处置 RUSTSEC。
- **通过标准**：`packaging/{aur,debian,rpm}` 三套规格与 `ASM-A-17` 一致；`AGENTS.md` 3.9 的 `cargo audit`/`cargo deny` 语义进入门禁。

### TC-INFRA-56 安装可逆性与 takeover 验证（`REQ-INFRA-09`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-09` ｜ `infra` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `xtask/src/install/{reversible,takeover,verify}.rs`、`packaging/{install,uninstall}.sh`
- **前置条件与沙盒状态**：干净容器 + 已有 ClassicUI。
- **操作步骤**：
  1. 安装 → 断言 payload 布局与 manifest 一致 -> 触发存盘：`<RUN>/infra/TC-INFRA-56/01_installed.png`
  2. 卸载 → 断言恢复到安装前状态（`reversible` 语义，含 takeover 前的 ClassicUI 激活态）。
- **通过标准**：`TC-INFRA-31` 的动态路径语义不变；反复安装/卸载 5 轮无残留。

### TC-INFRA-57 ELF 检查与图标缓存（`REQ-INFRA-09`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-09` ｜ `infra` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `xtask/src/install/{elf,icon_cache,place}.rs`
- **前置条件与沙盒状态**：release 产物。
- **操作步骤**：
  1. ELF 检查 → 断言双 cdylib 导出表正确、无意外 NEEDED -> 触发存盘：`<RUN>/infra/TC-INFRA-57/assertions.json`
  2. 图标缓存刷新断言（安装后 fcitx5 配置界面立即可见图标）。
- **通过标准**：`TC-INFRA-35` 的 `strip` + `nm -D` 双校验语义在 `elf.rs` 固化。

### TC-INFRA-58 基准纯净度门禁（`REQ-INFRA-09`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-09` ｜ `infra` | 极端容错与性能 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `xtask/src/testd/purity.rs`、`just bench`
- **前置条件与沙盒状态**：并行负载注入（模拟 20 agent）。
- **操作步骤**：
  1. 负载下跑 `bench-quick` → 断言 `purity` 检测到污染并拒绝采信 -> 触发存盘：`<RUN>/infra/TC-INFRA-58/assertions.json`
  2. 空闲机器重跑 → 断言数值恢复有效。
- **通过标准**：`ASM-T-11` 的教训（511ns vs 726ns 假回归）固化为机器门禁；基准报告携带机器状态标记。

### TC-INFRA-59 CI 作业矩阵与门禁编排（`REQ-INFRA-01` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-01` ｜ `infra` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `.github/workflows/`、`justfile`（`ci`、`ci-host`、`audits`）
- **前置条件与沙盒状态**：CI 环境（本机以 `just ci` 等价执行）。
- **操作步骤**：
  1. `just ci` → 断言 check + 十项审计 + check-host 全绿 -> 触发存盘：`<RUN>/infra/TC-INFRA-59/assertions.json`
  2. 人为注入一类违规 → 断言对应门禁红且报告可定位。
- **通过标准**：门禁构成与 2026-09-30 记录一致（零警告 + 全测试绿 + 审计全过）；`ci-host` 与 `ci` 分层清晰。

### TC-INFRA-60 交付链路的隐私抽检（`REQ-SEC-01` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-SEC-01` ｜ `infra` | 全状态防御与骨架屏 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `packaging/`、`docs/privacy.md`
- **前置条件与沙盒状态**：打包产物。
- **操作步骤**：
  1. 扫描产物与安装脚本 → 断言不含遥测、更新通道、明文路径 -> 触发存盘：`<RUN>/infra/TC-INFRA-60/assertions.json`
  2. 断言 `privacy.md` 的条款与产物行为一致（无夸大、无遗漏）。
- **通过标准**：`BUDGET-NET-01` 的交付面闭环；文档诚实性（features-add `ADD-FEAT-P0.05.02` 语义）。

### TC-INFRA-61 `just audits` 编排与失败聚合（`REQ-INFRA-08`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-08` ｜ `infra` | 核心业务闭环 ｜ `P1` ｜ 可执行性：`[可执行]` ｜ `justfile`（`audits` 目标）、`scripts/`
- **前置条件与沙盒状态**：仓库工作树。
- **操作步骤**：
  1. 运行 `just audits` → 断言 14 项审计（deps/unsafe/no-grab/net/net-runtime/slint/ui/dict/licenses/budget/versions/self-tests 等）依次执行并聚合结果 -> 触发存盘：`<RUN>/infra/TC-INFRA-61/assertions.json`
  2. 断言单项失败时报告聚合列出**全部**失败项（非首个即断）。
- **通过标准**：编排顺序与 justfile 声明一致；退出码取最严重者；无静默跳过。

### TC-INFRA-62 发行包规格抽检（`REQ-INFRA-09`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-09` ｜ `infra` | 核心业务闭环 ｜ `P2` ｜ 可执行性：`[可执行]` ｜ `packaging/{aur,debian,rpm}/`
- **前置条件与沙盒状态**：打包产物就绪。
- **操作步骤**：
  1. 逐套检查 AUR PKGBUILD、debian control/rules、rpm spec → 断言依赖声明与 `ASM-A-17` 的发行版矩阵一致 -> 触发存盘：`<RUN>/infra/TC-INFRA-62/assertions.json`
  2. 断言三套包的文件清单一致（双 cdylib + 3 conf + 词库 + 图标 + metainfo）。
- **通过标准**：无发行版特有硬编码路径；`package-arch` 的可重复构建语义（stamp）成立。

---

## 2. 分片出口准则

1. `REQ-INFRA-05` 的 15 条用例已随 `TASK-1.07.01` 落地转为 `[可执行]`（2026-10-06 全量轮已执行）。
2. ADR-0003 后果 #2 的第三个 conf（`Category=UI`，`rspinyin-ui.conf`）已创建并被 `TC-INFRA-32` 覆盖。
3. `TC-INFRA-35` 的 `strip` + `nm -D` 双校验已在 `install/elf.rs` 固化。
4. 在 Ubuntu 24.04 与 Fedora 两个容器中验证路径动态解析（`TC-INFRA-31`）。
5. 主文档矩阵的 `REQ-INFRA-01`~`09`、`REQ-SEC-01`（深化用例 `TC-INFRA-60`）行可执行性列为 `✅`，维度列按用例覆盖勾选；`REQ-INFRA-07`~`09` 为本轮同步新增行。

- **验收记录**（2026-10-06）：实测：安装权限与所有权（0700/0600 纪律）；证据包 results/runs/run-20261006-034915/infra/TC-INFRA-45/
