# rspinyin 测试用例分片 · infra（安装布局、打包与交付）

> 分片版本: v1.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 平台任务: [../features-test.md](../features-test.md) ｜
> 被测基线: Rust 2024 workspace + Fcitx5 5.1.7（Ubuntu 24.04 / WSL2） ｜ 关联 ADR: [../adr/0002-rust-exports-addon-factory.md](../adr/0002-rust-exports-addon-factory.md)、[../adr/0003-ui-role-separate-addon.md](../adr/0003-ui-role-separate-addon.md) ｜ 最后同步 Commit: `ee0dbfb` ｜
> 维护约定: 新增用例必须回写主文档第 2 节矩阵的 TC 列与维度列

## 0. 分片基线（引用主文档，不重复定义）

- **假设清单**：主文档第 1 节 + [features-test.md](../features-test.md) 第 1 节。强相关：`ASM-T-06`（`so_stripped = 12MB`、`base_dict = 20MB`）。
- **追踪矩阵**：主文档第 2 节的 `REQ-INFRA-01` ~ `REQ-INFRA-06`（`REQ-INFRA-01`~`04`、`06` 的 P0 用例已在主文档第 3 节）。
- **预算阈值**：`docs/dev/budgets.json`（键名：`so_stripped`、`base_dict`）。
- **安装路径纪律**：**禁止硬编码安装路径**——必须从 `pkg-config --variable=addondir Fcitx5Core` 动态获取。硬编码会在 Fedora/Arch 上装到错误位置，导致 fcitx5 找不到插件（`features.md` 6.2.3 的陷阱）。

> 阶段零实测：`packaging/fcitx5/` 当前有 **2 个 conf**（`rspinyin.conf` = `Category=InputMethod`、`rspinyin-im.conf` = 输入法描述）。ADR-0003 后果 #2 要求**增加第三个** `Category=UI` 的 conf（`librspinyin-ui.so`），当前**尚未创建**。`xtask/src/install.rs` **不存在**（`TASK-1.07.01` 为 `PENDING`）。

---

## 1. 用例

### TC-INFRA-31 安装布局与动态路径解析（`REQ-INFRA-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-05` ｜ `infra` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.07.01]` ｜ `xtask/src/install.rs`、`packaging/install.sh`
- **前置条件与沙盒状态**：干净 Ubuntu 24.04 容器；`libfcitx5core-dev` 已装。
- **操作步骤**：
  1. 执行 `just install` -> 触发存盘：`<RUN>/infra/TC-INFRA-31/assertions.json`
  2. 断言产物落位：`librspinyin.so` 与 `librspinyin-ui.so` → `$(pkg-config --variable=addondir Fcitx5Core)`；三个 conf → `/usr/share/fcitx5/addon/` 与 `/usr/share/fcitx5/inputmethod/`；`base.dict` → `/usr/share/rspinyin/`；图标 → hicolor。
- **通过标准**：**路径由 `pkg-config` 决定**；在 Ubuntu 与 Fedora 两个容器中均正确。

### TC-INFRA-32 双 addon 的 conf 齐备（`REQ-INFRA-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-05` ｜ `infra` | 核心业务闭环 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.07.01]` + `[待实现: TASK-1.04.03]`
- **操作步骤**：
  1. 检查安装后的 conf 集合 -> 触发存盘：`<RUN>/infra/TC-INFRA-32/assertions.json`
  2. 断言存在 `Category=InputMethod`（`Library=librspinyin`）与 `Category=UI`（`Library=librspinyin-ui`、`UIPriority > 0`、`UIType=PhysicalKeyboard`）两类。
- **通过标准**：ADR-0003 的**后果 #2**。`AddonCategory` 是单值枚举（`InputMethod`/`Frontend`/`Loader`/`Module`/`UI`），一个 addon 只能属于一个类别；工厂符号名固定为 `fcitx_addon_factory_instance`，同一 `.so` 被两个 conf 引用会创建两个**无法区分**的实例，故必须两个 `.so`。

### TC-INFRA-33 前端依赖必须是 Optional（`REQ-INFRA-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-05` ｜ `infra` | 边界与容错 ｜ `P0` ｜ 可执行性：`[可执行]`（`packaging/fcitx5/rspinyin.conf` 已存在）
- **操作步骤**：
  1. 检查 `rspinyin.conf` 的 `[Addon/Dependencies]` -> 触发存盘：`<RUN>/infra/TC-INFRA-33/assertions.json`
  2. 断言 `xcb` 与 `wayland` 在 `[Addon/OptionalDependencies]` 而**不在** `[Addon/Dependencies]`。
- **通过标准**：fcitx5 把 `[Addon/Dependencies]` 的每一项都当作必需，同时列 `xcb` 与 `wayland` 会让插件在纯 X11 或纯 Wayland 系统上**拒绝加载**（已对 fcitx5 5.1.7 验证）。当前 conf 正确：`[Addon/Dependencies]` 只有 `0=core:5.1.7`。

### TC-INFRA-34 卸载可逆性（`REQ-INFRA-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-05` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.07.01]`
- **操作步骤**：
  1. 安装后记录文件集合，执行 `just uninstall` -> 触发存盘：`<RUN>/infra/TC-INFRA-34/assertions.json`
  2. 断言 `/usr/share/fcitx5/` 的文件集合与安装前**完全一致**。
  3. 断言用户数据**未被删除**（`~/.local/share/rspinyin/user.redb` 仍在），且打印其位置与删除命令。
- **通过标准**：完整可逆；`ui_takeover.json` 的原值被写回 fcitx5 的活跃 UI 配置。

### TC-INFRA-35 体积预算与 `nm -D` 校验（`REQ-INFRA-05`）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-05` ｜ `infra` | 极端容错与性能 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.07.01]` ｜ `docs/dev/budgets.json`
- **操作步骤**：
  1. 打包时 `strip` 后测量 -> 触发存盘：`<RUN>/infra/TC-INFRA-35/assertions.json`
  2. 断言 `librspinyin.so` ≤ `so_stripped`（12MB）、`base.dict` ≤ `base_dict`（20MB）。
  3. **在 strip 之前与之后各跑一次** `nm -D --defined-only | grep fcitx_addon_factory_instance`，断言两者都有输出。
- **通过标准**：`strip` **只允许在打包阶段执行**，且必须紧跟 `nm -D` 校验——`Cargo.toml` 的 `[profile.release]` 不得设置 `strip`（任何形式），否则 rustc 会传 `--strip-all` 丢弃工厂符号，产出"能加载但不含 addon"的库（ADR-0002）。

### TC-INFRA-36 幂等安装与 `--dry-run`（`REQ-INFRA-05` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-05` ｜ `infra` | 边界与容错 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.07.01]`
- **操作步骤**：
  1. 连续执行 `just install` 两次 -> 触发存盘：`<RUN>/infra/TC-INFRA-36/assertions.json`
  2. 断言第二次无错误、无重复条目。
  3. 执行 `--dry-run`，断言只打印不执行（在只读文件系统上验证）。
- **通过标准**：幂等；`--dry-run` 无副作用。

### TC-INFRA-37 `DESTDIR`/`PREFIX` 支持（`REQ-INFRA-05` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-05` ｜ `infra` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.07.01]`
- **操作步骤**：
  1. `DESTDIR=/tmp/stage PREFIX=/usr bash packaging/install.sh` -> 触发存盘：`<RUN>/infra/TC-INFRA-37/assertions.json`
  2. 断言全部产物落在 `/tmp/stage/usr/...`。
- **通过标准**：`TASK-2.07.01` 的 deb/rpm/AUR 打包将复用同一脚本。

### TC-INFRA-38 不修改 fcitx5 的 profile（`REQ-INFRA-05` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-05` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.07.01]`
- **操作步骤**：
  1. 记录 `~/.config/fcitx5/profile` 的 `mtime` -> 执行 `just install` -> 触发存盘：`<RUN>/infra/TC-INFRA-38/assertions.json`
  2. 断言 `mtime` 不变（输入法列表由用户自己管理）。
- **通过标准**：安装脚本**不得**修改用户的输入法列表；提示用户到 `fcitx5-configtool` 添加。

### TC-INFRA-39 缺失开发包时的提示（`REQ-INFRA-05` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-05` ｜ `infra` | 边界与容错 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.07.01]`
- **操作步骤**：
  1. 在缺 `libfcitx5core-dev` 的容器执行 `just install` -> 触发存盘：`<RUN>/infra/TC-INFRA-39/assertions.json`
  2. 断言给出明确提示（`platform/fcitx5/dev-missing` + 各发行版安装命令）。
- **通过标准**：不出现晦涩的链接器符号错误。

### TC-INFRA-40 图标与桌面集成（`REQ-INFRA-05` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-05` ｜ `infra` | 商业化 5 态微交互与材质 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.07.01]`
- **操作步骤**：
  1. 安装后检查图标 -> 触发存盘：`<RUN>/infra/TC-INFRA-40/01_icon.png`
  2. 断言 `fcitx-rspinyin` 图标落在 hicolor 的 48x48 与 scalable 目录；断言 `gtk-update-icon-cache` 被调用（若存在）。
- **通过标准**：`rspinyin-im.conf` 的 `Icon=fcitx-rspinyin` 能解析到实际图标。

### TC-INFRA-41 CI 的 `check-versions` 与打包版本一致（`REQ-INFRA-05` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-05` ｜ `infra` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 执行 `just check-versions` -> 触发存盘：`<RUN>/infra/TC-INFRA-41/assertions.json`
  2. 断言 `packaging/fcitx5/rspinyin.conf` 的 `Version` == `Cargo.toml` 的 `version`（当前 `0.1.0`）。
- **通过标准**：新增的 UI conf 也必须纳入该断言（两个 conf 的 `Version` 都需一致）。

### TC-INFRA-42 构建产物不含 `target/`（`REQ-INFRA-05` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-05` ｜ `infra` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[可执行]`
- **操作步骤**：
  1. 检查 `.gitignore` 与 `git status` -> 触发存盘：`<RUN>/infra/TC-INFRA-42/assertions.json`
  2. 断言 `target/` 未被跟踪；断言 `.gitignore` 覆盖 `/target`、`*.dict.tmp`、`user.redb*`、`*.corrupt.*`、`/logs`。
- **通过标准**：`AGENTS.md` 3.1；`git submodule` 全局禁止（保证 `cargo vendor` 与离线构建可行）。

### TC-INFRA-43 审计脚本在安装后的插件上仍通过（`REQ-INFRA-05` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-05` ｜ `infra` | 全状态防御与骨架屏 ｜ `P0` ｜ 可执行性：`[待实现: TASK-1.07.01]`
- **操作步骤**：
  1. 安装后对产物运行 `scripts/check-unsafe.sh`、`check-slint-leak.sh` -> 触发存盘：`<RUN>/infra/TC-INFRA-43/assertions.json`
  2. 断言两个脚本在已安装产物上仍通过。
- **通过标准**：安装不引入新的 `unsafe` 或 Slint 泄漏面。

### TC-INFRA-44 离线安装可行性（`REQ-INFRA-05` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-05` ｜ `infra` | 边界与容错 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.07.01]`
- **操作步骤**：
  1. 在断网环境（无外网）执行完整的构建 + 安装 -> 触发存盘：`<RUN>/infra/TC-INFRA-44/assertions.json`
  2. 断言成功（`cargo vendor` 或已填充的 registry 缓存）。
- **通过标准**：`BUDGET-NET-01` 的离线承诺；`git submodule` 禁止是这条的前提。

### TC-INFRA-45 安装脚本的权限与所有权（`REQ-INFRA-05` 深化）

- **基本属性**：`[ ] 未通过` ｜ `REQ-INFRA-05` ｜ `infra` | 全状态防御与骨架屏 ｜ `P1` ｜ 可执行性：`[待实现: TASK-1.07.01]`
- **操作步骤**：
  1. 检查安装产物的权限 -> 触发存盘：`<RUN>/infra/TC-INFRA-45/assertions.json`
  2. 断言系统路径下为 `0644`、目录为 `0755`、属主为 `root:root`（非用户目录）。
- **通过标准**：系统级产物权限符合 FHS；用户数据仍为 `0600`/`0700`（`REQ-SEC-01`）。

---

## 2. 分片出口准则

1. `REQ-INFRA-05` 的 15 条用例在 `TASK-1.07.01` 落地后转为 `[可执行]`。
2. **ADR-0003 后果 #2 的第三个 conf**（`Category=UI`）创建并被 `TC-INFRA-32` 覆盖。
3. `TC-INFRA-35` 的 `strip` + `nm -D` 双校验进入打包流程（`TASK-2.07.01` 复用）。
4. 在 Ubuntu 24.04 与 Fedora 两个容器中验证路径动态解析（`TC-INFRA-31`）。
5. 主文档矩阵的 `REQ-INFRA-05` 行可执行性列更新为 `✅`，维度列全部勾选。
