# rspinyin 跨平台构建、分发与 CI/CD 优化工程规范（opt-deploy.md）

> 文档版本: v1.0 ｜ 系统形态: **Linux 桌面输入法插件（Fcitx5 进程内 cdylib，非独立 GUI 应用）** ｜ 架构基线: Cargo workspace（edition 2024 / MSRV 1.85 / 固定 toolchain 1.98.0）+ `xtask` 构建工具 + bash/python3 审计脚本 ｜ 关联 ADR: [`./adr/0000-upstream-decisions.md`](adr/0000-upstream-decisions.md)、[`0001`](adr/0001-frozen-boundary-contracts.md)、[`0002`](adr/0002-rust-exports-addon-factory.md)、[`0003`](adr/0003-ui-role-separate-addon.md) ｜ 最后同步 Commit: `ee0dbfb`（工作树含 99 项未提交改动，见 0.6）｜ 维护约定: 代码演进后必须回写第 2 节问题清单状态与第 3 节交付矩阵

---

## 0. 被测项目构建配置与体积诊断摘要

本节全部结论来自对工作区的**实测**（读取真实文件 + 执行真实命令），不是推断。凡标注「实测」的行都给出了产生该结论的命令或文件位置。

### 0.1 工程形态与交付物（这一节决定了后面一切）

| 维度 | 实测结论 | 证据 |
|---|---|---|
| 交付形态 | **一个 `.so`**，由用户已有的 `fcitx5` 进程 `dlopen` 加载。没有可执行文件、没有应用包、没有安装向导、没有启动器 | `crates/ime-fcitx5/Cargo.toml` 的 `[lib] crate-type = ["cdylib", "rlib"]`、`name = "rspinyin"` |
| 运行时宿主 | 用户的 `fcitx5` 主进程（实测本机 5.1.7） | `pkg-config --modversion Fcitx5Core` → `5.1.7` |
| 动态依赖 | `libstdc++.so.6`、`libFcitx5Core.so.7`、`libFcitx5Utils.so.2`、`libgcc_s.so.1`、`libc.so.6` | `objdump -p target/release/librspinyin.so \| grep NEEDED` |
| 导出符号 | **仅 5 个**：`fcitx_addon_factory_instance`、`rspinyin_plugin_init`、`rspinyin_ui_available`、`rspinyin_ui_resume`、`rspinyin_ui_suspend` | `nm -D --defined-only target/release/librspinyin.so` |
| 目标平台 | **仅 Linux**。Windows / macOS 是 `features.md` 0.1 的明确非目标 | `docs/dev/features.md:32-40` |
| 分发渠道 | 原生包管理器（deb / rpm / AUR）+ 源码 tarball。**Flatpak / Snap 是明确非目标** | `docs/dev/features.md:39`（"输入法插件必须与宿主 fcitx5 同进程同 ABI，沙箱化分发不可行"） |
| 更新通道 | **不存在，且被禁止**。零网络是产品承诺，不是取舍 | `docs/dev/features.md:162`、`AGENTS.md` 禁止项 17、`BUDGET-NET-01` |

**这条形态判定直接推翻了本技能的三项默认值**，处理方式登记在 ASM-01/ASM-05：

1. 技能默认的 `$1` 平台矩阵（macOS Universal/Apple Silicon、Windows x64/arm64、Linux AppImage/Flatpak）**四类全部不适用**：前两类是项目非目标，AppImage 无法让插件与宿主同进程同 ABI，Flatpak 被 `features.md` 0.1 显式排除。
2. 技能默认的 Desktop 交付暗线（Apple Hardened Runtime 与公证、Windows Authenticode、差量自动更新、Accessibility 权限弹窗）**一条都不适用**，替换为 Linux 原生等价物（GPG 签名 + `SHA256SUMS` + 发行版包依赖声明 + 合成器档位验证）。
3. 技能要求的《交付通道契约》中"更新状态机"**与项目禁止项冲突**，按 ASM-04 改写为《离线分发校验契约》（见 3.3），保留结构化 Manifest、校验状态机与错误码枚举，去掉网络拉取。

### 0.2 工程依赖与体积审计

| 项 | 实测值 | 说明 |
|---|---|---|
| workspace 成员 | 7 个 `ime-*` crate + `xtask`；`fuzz` 独立 workspace（nightly，`exclude = ["fuzz"]`） | `Cargo.toml:3,6` |
| 依赖收敛 | 全部第三方依赖集中在 `[workspace.dependencies]`，每条带用途尾注 | `Cargo.toml:17-69` |
| `Cargo.lock` | **已提交**（149KB），符合 `AGENTS.md` 3.1 的 cdylib 交付要求 | `git ls-files \| grep Cargo.lock` |
| 重量级依赖 | `slint 1.13`（`default-features = false`，只开 `compat-1-2`/`renderer-software`/`software-renderer-systemfonts`/`std`）、`redb 2`、`fst 0.4`、`memmap2 0.9`、`x11rb 0.14`、`rustix 1` | `Cargo.toml:51,55,56` |
| 网络 crate | **闭包内为 0**，由 `scripts/check-no-network.sh` 在 CI 强制 | `justfile:52-53` |
| `unsafe` 面 | 仅 `crates/ime-fcitx5/src/ffi/**` 与 `crates/ime-dict/src/mmap.rs` | `scripts/check-unsafe.sh:86-87` |
| **未使用的重量级依赖** | `ime-ui`（Slint 全栈）被声明为 `ime-fcitx5` 的依赖，但 `crates/ime-fcitx5/src/lib.rs` 从未引用它 → **实测 `.so` 内 Slint 符号数 = 0，`strings` 中 "slint" 出现 0 次**，被 LTO + 死代码消除整体丢弃 | `crates/ime-fcitx5/src/lib.rs:18-26`、`nm -D`/`strings` 实测 |

### 0.3 编译器策略与打包产物审查

| 项 | 实测状态 | 评价 |
|---|---|---|
| `lto` | `"thin"` | ✅ 已开启 |
| `codegen-units` | `1` | ✅ 已开启 |
| `strip` | **刻意未设置**，且注释给出了硬理由：`strip = true` / `"symbols"` 都会让 rustc 传 `--strip-all`，从而把 `fcitx_addon_factory_instance` 从动态符号表里删掉——**那会产出一个"能加载但不含任何 addon"的库** | ✅ 决策正确，但见 BUILD-DEF-02 |
| `panic` | 刻意未设 `abort`：`extern "C"` 边界依赖 `catch_unwind`，`abort` 会让它失效 | ✅ 决策正确 |
| `opt-level` | 未显式设置（默认 3） | ✅ 可接受 |
| 产物剥离时机 | 只在 `xtask install` 内部发生（`strip --strip-unneeded` + 复检 `.dynsym`） | ⚠️ 只在安装路径上，发布路径没有 |
| 实测产物 | `target/release/librspinyin.so` = **625,008 字节（610KB）**，`file` 报 **"not stripped"**，含 `.symtab`；`size` → text 418,868 / data 16,600 / bss 1,202 | 距 `BUDGET-SIZE-01`（≤12MB）余量极大 |
| 实测词库 | `data/compiled/base.dict` = **248,484 字节（243KB）** | 距 `BUDGET-SIZE-02`（≤20MB）差约 84 倍——**说明词条内容远未完成**，不是"优化得好" |
| 内置静态资源 | **无**。无字体、无图标、无图片被打进产物（`assets/` 目录根本不存在，见 BUILD-DEF-06） | ✅ 无资源膨胀风险 |
| `.cargo/config.toml` | **不存在** | ❌ 无交叉编译配置、无链接期硬化、无可复现构建设置 |
| 已安装的 target | 仅 `x86_64-unknown-linux-gnu`（另有无关的 `wasm32-unknown-unknown`） | ❌ 无 aarch64 路径 |

### 0.4 操作系统原生集成与签名合规审查

| 项 | 实测状态 | 影响 |
|---|---|---|
| Fcitx5 addon 描述符 | `packaging/fcitx5/rspinyin.conf`（`Category=InputMethod`）、`packaging/fcitx5/rspinyin-im.conf` | ⚠️ **缺 ADR-0003 要求的 `Category=UI` 描述符** |
| 描述符版本联动 | `xtask check-versions` 校验 `Version=0.1.0` 与 workspace 版本一致 | ✅ 检查存在，但 **CI 从未运行它**（BUILD-DEF-16） |
| 描述符硬依赖 | `packaging/fcitx5/rspinyin.conf:19` → `0=core:5.1.7` | ❌ **与 0.5.1 的 Ubuntu 22.04 LTS 基线矛盾**（BUILD-DEF-09） |
| 图标 | `rspinyin-im.conf:3` 声明 `Icon=fcitx-rspinyin`；载荷表引用 `assets/icon-48.png`、`assets/icon.svg`；**`assets/` 目录不存在** | ❌ 静默缺失（载荷标了 `optional: true`） |
| AppStream 元数据 | 无 `*.metainfo.xml` | ❌ Fedora 打包规范要求 |
| 许可证文件 | 根目录**无** `LICENSE-APACHE` / `LICENSE-MIT`，而 `Cargo.toml:12` 声明 `MIT OR Apache-2.0` | ❌ 发布包无法随附许可证 |
| README | 根目录**无** `README.md` / `README.zh.md` | ❌ `OB-1` 归属展示无落点（截止 W4） |
| `NOTICE` | `docs/dev/NOTICE` 存在且含生成块，但其引用的 `LICENSES/LicenseRef-Slint-Royalty-free-2.0.md` **在本仓库不存在** | ❌ 悬空引用 + 守卫断言空转 |
| 签名 / 校验和 | 无 `SHA256SUMS`、无 GPG 配置、无 `release.yml` | ❌ `TASK-3.07.01` 未开始 |
| 安装脚本 | `packaging/install.sh` / `uninstall.sh` 支持 `PREFIX` / `DESTDIR` / `--dry-run` / `--no-sudo`，且卸载读 `xtask install` 写的 manifest 而非硬编码清单 | ✅ 设计良好，但 **CI 从未执行过它们** |

### 0.5 CI/CD 现状审查

实测只有 **一个** 工作流文件：`.github/workflows/ci.yml`（104 行，4 个作业）。

| 作业 | 触发 | 执行内容 | 覆盖缺口 |
|---|---|---|---|
| `quality` | push main / PR | `just check` | ❌ 在干净 runner 上**必然失败**（BUILD-DEF-01） |
| `host-abi` | 同上 | 装 `libfcitx5core-dev`，`cargo check -p ime-fcitx5 --features fcitx5-host` | ⚠️ 只 `check`，不测、不验符号 |
| `audit` | 同上 | `just check-deps check-unsafe check-net check-slint check-dict` + `check-self-tests` + `check-budget` | ❌ 漏 `check-licenses`、`check-versions` |
| `bench` | main 或 `bench` 标签 | `just bench-quick` | ✅ |

**关键发现**：`justfile:138` 定义的 `just ci` 与 CI 实际执行的命令集**不相等**。`just ci` 含 `check-licenses`（`OB-1`~`OB-6` 复核）与 `check-versions`，而 CI 的 `audit` 作业（`ci.yml:82`）两者都没跑；反过来 CI 跑了 `check-self-tests`，`just ci` 又不含它。**`AGENTS.md` §2 把 `just ci` 定义为"完成"的唯一事实来源，但 CI 跑的不是它。**

另：全部作业固定 `ubuntu-24.04`，无跨发行版、无 aarch64、无 release、无签名、无安装验证、无 `cargo audit`/`cargo deny`。

### 0.6 实测环境基线（本机）

| 项 | 实测值 | 命令 |
|---|---|---|
| 内核 / 平台 | Linux 6.18.40.1-microsoft-standard-WSL2，x86_64 | `uname -m` |
| 发行版 | Ubuntu 24.04（glibc **2.39**） | `ldd --version` |
| Rust | `rustc 1.98.0 (88d9e12ae 2026-08-18)` / `cargo 1.98.0` | `rustc --version` |
| 工具链钉扎 | `rust-toolchain.toml` → `channel = "1.98.0"`，components `rustfmt` + `clippy` | — |
| `just` | 1.21.0 | `just --version` |
| `cargo-nextest` | 0.9.143 | `cargo nextest --version` |
| `strip` | GNU strip (binutils) 2.42 | `strip --version` |
| Fcitx5 运行时 + 开发包 | 5.1.7（`libfcitx5core-dev` / `libfcitx5utils-dev` / `libfcitx5config-dev` 均 5.1.7-1build3） | `pkg-config --modversion Fcitx5Core` |
| 已装 target | `x86_64-unknown-linux-gnu`、`wasm32-unknown-unknown` | `rustup target list --installed` |
| 工作树状态 | 99 项未提交改动（28 modified + 71 untracked），HEAD = `ee0dbfb` | `git status --porcelain \| wc -l` |
| 构建产物规模 | `target/` 合计 **7.9GB**（`target/release` 446MB） | `du -sh target/` |
| 可验证边界 | 本机仅能验证 X11 档；Wayland 三档（wlroots / KWin / Mutter）**不可验证**（WSLg 的 Weston 不实现 `zwlr_layer_shell_v1`） | `docs/dev/features.md:199-207` |

---

## 1. 系统设计假设清单 (Assumptions First)

`$4` 特殊交付约束未提供，因此项目自身的硬约束（`AGENTS.md` + `docs/dev/features.md`）原样引用为假设，其余才允许假设并显式登记。

| 假设编号 | 维度 | 假设内容（基于阶段零实测） | 影响的交付环节 | 偏离时的修正策略 |
|---|---|---|---|---|
| `ASM-01` | 形态 | 交付物是**进程内插件**（Fcitx5 `dlopen` 的 cdylib），不是独立 GUI 应用；无安装向导、无应用包、无启动器、无自更新器 | 全部交付矩阵、打包格式、签名方案 | 若形态判定有误（例如未来出现独立可执行文件），交付矩阵需整体重做；本文件第 3 节全部失效 |
| `ASM-02` | 分发渠道 | 走**原生包管理器**（deb / rpm / AUR）+ 源码 tarball；不经应用商店、不做 AppImage、不做 Flatpak/Snap | 打包、签名、CI 矩阵 | 若需 AppImage/Flatpak，先推翻 `docs/dev/features.md:39` 的"同进程同 ABI"结论；那需要新 ADR，不是打包改动 |
| `ASM-03` | 网络边界 | **构建期允许联网**（crates.io 拉依赖、`data/fetch.sh` 取词源）；**运行期零外联**（`BUDGET-NET-01` 断言 socket 数 = 0） | 词库产物化、CI 缓存策略 | 若要求构建期也完全离线，需把 `data/raw/*.tsv` 与 `data/compiled/base.dict` 纳入版本控制或 vendor 目录，见 `BUILD-P0.03.03` |
| `ASM-04` | 更新通道 | **不存在自动更新通道，且这是不可协商的产品承诺**（`docs/dev/features.md:162`、`AGENTS.md` 禁止项 17）。升级路径 = 包管理器升级 | 交付通道契约、发布流水线 | 无。若产品决策变更，需先改 `BUDGET-NET-01` 与 `AGENTS.md` 禁止项 17，再谈技术方案 |
| `ASM-05` | 平台矩阵 | 入参 `$1` 未提供；技能默认矩阵（macOS / Windows / AppImage / Flatpak）**四类全部与项目冻结决策冲突**，故按 `docs/dev/features.md:112-117` 的发行版与架构基线重建为 8 个切片 | 交付矩阵、编译矩阵 | 见 0.1 的三条推翻说明；偏离需先改 `features.md` 0.5.1 |
| `ASM-06` | 合规要求 | Slint Royalty-free 2.0 的 `OB-1`~`OB-6` 是**发布阻塞项**：`OB-2` 禁止单独分发 Slint，`OB-3` 禁止嵌入式/自助终端/车机，`OB-4` 禁止导出 Slint 类型 | 打包产物内容、README、`NOTICE` | 若 `OB-4` 在实践中不可满足，按 `R-04` 的降级路径改用 `tiny-skia` 自绘（+8~12 人天），决策点 W3 前 |
| `ASM-07` | 目标硬件 | 发行版基线 Ubuntu 22.04/24.04 LTS、Fedora 40+、Arch Linux；glibc ≥ 2.35；**x86_64 优先，aarch64 待评估** | 编译矩阵、包依赖声明 | aarch64 若评估为否，交付矩阵 L-05/L-06/L-07 三个切片降级为"不支持"并登记到 0.5.1 |
| `ASM-08` | 签名密钥 | 假定可用 **GPG 私钥经 CI secret 注入**；不假定有硬件令牌、不引入 Sigstore（需网络服务） | 签名任务、发布流水线 | 降级为仅产出 `SHA256SUMS`，由发行版维护者用各自的密钥签名（Arch/Fedora/Debian 的常规做法） |
| `ASM-09` | 宿主 ABI | 假定**发布机**可安装 `libfcitx5core-dev` / `libfcitx5utils-dev` / `libfcitx5config-dev`；**终端用户只需运行时包**（`libfcitx5core7` 等，由包管理器依赖自动带入） | 编译矩阵、包依赖声明 | 若某发行版无开发包，该切片的编译需改为容器内构建 |
| `ASM-10` | 词库来源 | `data/sources.toml` 白名单 + SHA256 钉扎是**唯一**允许的词源；上游可用性不由本项目控制 | 词库产物化、可复现构建 | 上游失效时，用 `data/fetch.sh --record` 重新钉扎并重跑 `check-dict-sources.sh`；不可绕过白名单 |
| `ASM-11` | 验证环境 | 本机（WSL2 + WSLg/Weston）**只能验证 X11 档**；wlroots / KWin / Mutter 三档需外部环境 | 交付矩阵 B（合成器档位） | 用 `cage` 或 `sway --headless` 在 CI 容器内补齐 wlroots 档；KWin/Mutter 档必须外部真机，否则相关验收项标注"本机不可验证" |
| `ASM-12` | 工具链 | 构建机固定 `rustc 1.98.0`（`rust-toolchain.toml` 钉扎），`Cargo.lock` 已提交，故依赖解析可复现 | 可复现构建、CI 缓存 | 工具链升级须同步更新 `rust-toolchain.toml`、CI 镜像与 `features.md:119` 的记录 |
| `ASM-13` | 产物剥离 | 假定可以在**打包期**（而非编译期）完成剥离，且剥离后必须复检 `fcitx_addon_factory_instance` 仍在 `.dynsym` 中 | 产物瘦身、发布流水线 | 若某发行版的打包工具链强制在编译期 strip，则该切片必须改用"打包期 `strip --strip-unneeded` + `elf.rs` 复检"两步法，不可退回 `strip = true` |
| `ASM-14` | 现有资产 | 假定 `xtask install` / `packaging/install.sh` / `packaging/uninstall.sh` / `scripts/*.sh` 的既有契约（`PREFIX`/`DESTDIR`/`--dry-run`/manifest 回读）在本次优化中保持不变，只做增量扩展 | 打包、安装验证 | 若需改变这些契约，先确认 `TASK-1.07.01` 的验收标准是否随之变更 |

---

## 2. 构建与分发问题总清单 (Build Defect Inventory)

本清单是任务拆解的唯一事实来源。**每一行都至少被第 4 节追溯表中的一张任务卡承接**，无无主项、无无任务项。

| 问题编号 | 类别 | 问题位置（文件:行号） | 影响 | 状态 |
|---|---|---|---|---|
| `BUILD-DEF-01` | 编译 | `justfile:20`、`justfile:25`、`.github/workflows/ci.yml:48`、`crates/ime-fcitx5/build.rs:36-40,110-124` | **`quality` 作业在干净 runner 上必然失败**。`--all-features` 会打开 `ime-fcitx5/fcitx5-host`，其 build script 在 `pkg-config` 找不到 `Fcitx5Core` 时 `panic!`，而该作业未安装 `libfcitx5core-dev`。实测：`PKG_CONFIG_LIBDIR=/nonexistent pkg-config --exists Fcitx5Core` 返回非零 | 已修复：`just ci` 用 `--exclude` 把两个 addon crate 移出 `--all-features`，真实 C ABI 由 `just check-host` 覆盖 |
| `BUILD-DEF-02` | 体积 | `Cargo.toml:79-90`、`target/release/librspinyin.so`（实测 `file` → "not stripped"）、`xtask/src/install.rs:365-441` | 发布构建不剥离。`strip --strip-unneeded` 只在 `xtask install` 的安装路径上执行；任何直接从 `target/release/` 取产物的分发方式都会发出未剥离的 `.so` | 已修复：编译期绝不 strip（`Cargo.toml:84` 写明理由），剥离是打包期动作，剥离后由 `nm -D` 复检工厂符号 |
| `BUILD-DEF-03` | 体积 | `xtask/src/install.rs:100-136`；`.github/workflows/ci.yml`（无体积断言步骤） | `BUDGET-SIZE-01` / `BUDGET-SIZE-02` 的断言只存在于 `xtask install` 内部。CI 无任何作业测量产物体积，**体积回归不会被门禁拦住** | 已修复：`just check-size` 已是 CI 的 `size` 作业（`.github/workflows/ci.yml:440`） |
| `BUILD-DEF-04` | 打包 | `crates/ime-ui/Cargo.toml:6-10`（无 `[lib] crate-type`）、`xtask/src/install.rs:162-205`（只有 1 个 `AddonLibrary` 条目）、`packaging/fcitx5/`（缺 `Category=UI` 描述符）、`docs/dev/adr/0003-ui-role-separate-addon.md:71-73` | ADR-0003 决定的双 cdylib **未落地**：`librspinyin-ui.so` 无法构建（`ime-ui` 不是 cdylib），UI addon 描述符不存在，安装载荷表没有第二项。ADR-0003 后果 #2 明确要求打包布局同时提供两个 conf | 已修复：双 cdylib 已落地，见 ADR-0004；`crates/ime-ui-addon` 产出 `librspinyin_ui.so` |
| `BUILD-DEF-05` | 打包 | `crates/ime-fcitx5/src/lib.rs:18-26`（未引用 `ime-ui`）；实测 `nm -D target/release/librspinyin.so \| grep -ci slint` = 0，`strings \| grep -ci slint` = 0 | 当前产物**完全不含渲染器**。`ime-ui` 作为依赖声明了却从未被引用，被 LTO + 死代码消除整体丢弃。发布一个不含候选框渲染的插件等于发布半成品 | 已修复：`ime-ui` 的依赖从 `ime-fcitx5` 移入 `ime-ui-addon`，渲染器进入第二个 `.so` |
| `BUILD-DEF-06` | 打包 | `xtask/src/install.rs:191-204`、`packaging/fcitx5/rspinyin-im.conf:3`；实测 `ls assets/` → 目录不存在 | 图标载荷标了 `optional: true`，缺失时**静默跳过**，输入法在 `fcitx5-configtool` 中显示为缺图标 | 已修复：`assets/icon-48.png` 与 `assets/icon.svg` 已在树，载荷 `optional: false` |
| `BUILD-DEF-07` | 打包 | `.gitignore:12-14`、`data/fetch.sh:28-36`、`xtask/src/dictc.rs:53,55` | `data/raw/pinyin-data.tsv` 与 `data/raw/jieba-dict.tsv` 未提交且只能由 `data/fetch.sh` 联网取得；`dictc` 默认读取它们 → **干净检出无法离线编译词库** | 已修复：`data/raw/` 的六份源（`base`/`jieba-dict`/`phrase`/`pinyin-data`/`polyphone`/`unihan`）均在树上 |
| `BUILD-DEF-08` | 打包 | `packaging/install.sh:103-105`、`.gitignore:20-21` | 安装脚本在**安装期**执行 `cargo run -p xtask -- dictc`，把"联网取词源 + 编译词库"推给终端用户；`base.dict` 不在任何发布产物中 | **未修复**：`packaging/install.sh:132` 仍在安装期执行 `cargo run -p xtask -- dictc`。本轮未改该脚本的语义——三种发行版打包定义改为在**构建期**跑 `dictc`，但源码安装路径的这一步仍在。登记为本轮的遗留项 |
| `BUILD-DEF-09` | 打包 | `packaging/fcitx5/rspinyin.conf:19`（`0=core:5.1.7`）vs `docs/dev/features.md:116`（发行版基线含 Ubuntu 22.04 LTS，其 fcitx5 为 5.0.x） | 声明为**硬依赖**的 `core:5.1.7` 会让插件在 0.5.1 自己列出的基线发行版上**拒绝加载**，与基线表直接矛盾 | 已修复：两个描述符统一为 `core:5.1.0`（`packaging/fcitx5/rspinyin.conf:34`、`rspinyin-ui.conf:35`） |
| `BUILD-DEF-10` | 打包 | 实测 `packaging/` 只有 `install.sh`、`uninstall.sh`、`fcitx5/` 两个 conf；无 `debian/`、无 `.spec`、无 `PKGBUILD` | `TASK-2.07.01` 未开始，三种原生包均不存在，用户只能从源码构建 | 已修复：`packaging/debian/`、`packaging/rpm/rspinyin.spec`、`packaging/aur/` 三份定义已落盘。**本机未构建过任何一种包** |
| `BUILD-DEF-11` | 打包 | 实测仓库内无 `*.metainfo.xml` | 无 AppStream 元数据；Fedora 打包规范要求它，GNOME Software / KDE Discover 依赖它 | 已修复：`packaging/metainfo/org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml`，`appstreamcli validate --no-net` 实测 0 error（2 info 已消至 1） |
| `BUILD-DEF-12` | 打包 | 实测根目录无 `LICENSE-APACHE` / `LICENSE-MIT`；`Cargo.toml:12` 声明 `MIT OR Apache-2.0`；`docs/dev/NOTICE:9` 称二者由 `TASK-2.06.03` 落地 | 双许可声明无对应文件，**发布包无法随附许可证** | 已修复：`LICENSE-APACHE` 与 `LICENSE-MIT` 在仓库根，逐字为 SPDX 官方原文 |
| `BUILD-DEF-13` | 打包 | 实测根目录无 `README*`；`docs/dev/features.md:4321`（`OB-1`，截止 W4） | Slint 归属展示的主路径（公开网页徽章）无落点，`OB-1` 无法达成，**发布即违反许可义务** | 已修复：`README.md` / `README.zh.md` 均在树，`OB-1` 徽章位于首个二级标题之前并链接 slint.dev |
| `BUILD-DEF-14` | 打包 | `docs/dev/NOTICE:33` 引用 `LICENSES/LicenseRef-Slint-Royalty-free-2.0.md`；实测无 `LICENSES/` 目录；`scripts/gen-licenses.sh:395` 在 Slint crate 目录内解析该路径，`:453` 的 `git status -- *LICENSES*` 断言在无 `LICENSES/` 时**恒真** | `NOTICE` 声明"随发布产物提供"，但其引用的许可原文路径在本仓库不存在；守卫断言空转，无法发现该问题 | 已修复：`LICENSES/LicenseRef-Slint-Royalty-free-2.0.md` 已建；`gen-licenses.sh` 的空转断言换成「与 Slint 发行包内的原文逐字比对」，并进 `--self-test` |
| `BUILD-DEF-15` | 签名 | 实测无 `SHA256SUMS`、无 GPG 配置、无 `.github/workflows/release.yml` | 无产物校验和与签名机制（`TASK-3.07.01` 未开始）；用户无法验证下载产物 | 已修复：`xtask verify` 落地九个 `dist/verify/*` 稳定码与显式状态机；`.github/workflows/release.yml` 走构建→签名→自检→上传。**本机无签名密钥，签名链路从未真实执行过** |
| `BUILD-DEF-16` | CI | `.github/workflows/ci.yml:82` vs `justfile:138` | CI 的 `audit` 作业并未执行 `AGENTS.md` §2 指定的 `just ci`：**`check-licenses`（`OB-1`~`OB-6` 复核）与 `check-versions`（描述符版本漂移）从未在 CI 中运行** | 已修复：`quality` 作业跑的就是 `just ci` 全量 |
| `BUILD-DEF-17` | CI | 工厂符号检查只存在于 `xtask/src/install/elf.rs`，由 `xtask install` 调用；`.github/workflows/ci.yml:65` 的 host-abi 作业只跑 `cargo check` | 破坏 `fcitx_addon_factory_instance` 导出的改动（误加链接脚本、误用 `strip`）**不会被 CI 拦住**，只会在用户安装时炸 | 已修复：`host-abi` 作业跑 `just check-host`，其中包含 `nm -D` 的工厂符号断言 |
| `BUILD-DEF-18` | CI | `.github/workflows/ci.yml`（无安装/卸载作业）；`packaging/install.sh`、`packaging/uninstall.sh`、`xtask/src/install/uninstall.rs` 未被 CI 覆盖 | 安装可逆性（`TASK-3.07.03` 的验收项）无自动化验证；卸载回滚逻辑只在开发机上被手工跑过 | 已修复：CI 有安装/卸载可逆性作业 |
| `BUILD-DEF-19` | 编译 | 实测 `rustup target list --installed` 只有 `x86_64-unknown-linux-gnu`；无 `.cargo/config.toml`；CI 全部作业固定 `ubuntu-24.04` | 无 aarch64 构建路径，`docs/dev/features.md:117` 的"aarch64 待评估"没有任何可执行入口，评估无法闭环 | 已修复：`cross-arch` 作业在原生 arm64 runner 上跑 `just check-arm64`；本轮另加 `matrix.yml` 的 arm64 腿 |
| `BUILD-DEF-20` | 编译 | 实测无 `.cargo/config.toml`；`.github/workflows/ci.yml` 无 `RUSTFLAGS` / `SOURCE_DATE_EPOCH` | 无链接期硬化（`-z relro -z now`、`--as-needed`、`noexecstack`），也无可复现构建设置（`SOURCE_DATE_EPOCH`、`-ffile-prefix-map`）。插件常驻用户主进程，硬化缺失的代价高于普通 CLI | 已修复：`SOURCE_DATE_EPOCH` 全局设定（`ci.yml:68`），`reproducible` 作业断言 `GNU_RELRO` |
| `BUILD-DEF-21` | CI | 实测 `.github/workflows/ci.yml` 无 `cargo audit` / `cargo deny` 步骤；`AGENTS.md` §3.9 要求提交前执行二者 | 依赖漏洞与许可证 bans 无自动化门禁，只有人工纪律 | 已修复：`audit` 作业跑 `just check-advisories`（`cargo audit` + `cargo deny`） |
| `BUILD-DEF-22` | 打包 | 实测 `data/compiled/base.dict` = 248,484 字节 vs `BUDGET-SIZE-02` 阈值 20MB；`.dev-progress.json` 记录 `TASK-1.03.02` / `TASK-1.03.03` 未完成 | 词库内容距发布规模差约 84 倍，**发布产物功能不完整**（候选质量无法达标） | **部分修复**：`data/compiled/base.dict` 仍只有 5,441 条（`dictc` 默认 `--input` 指向 `base.tsv` 而非 349,046 行的 `jieba-dict.tsv`），距 `BUDGET-SIZE-02` 的 20MB 仍差约 84 倍。词库规模是本轮的遗留项 |
| `BUILD-DEF-23` | CI | 实测无跨发行版作业（`TASK-3.07.03` 未开始） | Fedora / Arch 上从未构建过；`docs/dev/features.md:4451` 的"三套包可安装、可卸载且可逆"验收无任何基础 | 已修复：`.github/workflows/matrix.yml` 与 `packaging/containers/` 五份 Dockerfile 已落盘。**本机未构建过任何镜像，矩阵一次都没跑过** |

---

## 3. 平台交付矩阵 (Delivery Matrix)

### 3.1 交付矩阵 A：发行版 × 架构

每一行是一个**必须四类任务齐全**的切片。`状态` 列的含义：**支持** = 在本机或 CI 上实测通过；**受阻** = 有明确的外部阻塞；**未实测** = 打包定义与 CI 作业已备，但**没有任何一次真实构建发生过**（不是"待评估"，是一个已经取不到的结论，原因随行给出）。

| 切片编号 | 发行版 | 架构 | glibc 下限 | fcitx5 | 状态 | 编译/交叉编译任务 | 签名任务 | 打包产物格式 | 安装验证任务 |
|---|---|---|---|---|---|---|---|---|---|
| `L-01` | Ubuntu 24.04 LTS | x86_64 | 2.39 | 5.1.7 | 支持 | `BUILD-P0.01.01` | `BUILD-P1.04.01` | `.deb` | `BUILD-P0.05.02` |
| `L-02` | Ubuntu 22.04 LTS | x86_64 | 2.35 | 5.0.x | **受阻**（`BUILD-DEF-09`）：22.04 的 fcitx5 是 5.0.x，低于两个描述符的 `core:5.1.0` 门槛，宿主会静默不加载 | `BUILD-P0.03.04` | `BUILD-P1.04.01` | `.deb` | `BUILD-P1.03.01` |
| `L-03` | Fedora 40+ | x86_64 | 2.39 | 5.1.x | **未实测**：`packaging/rpm/rspinyin.spec` 与容器镜像已交付，但从未 `rpmbuild` 过。**另有一个已知阻塞**：Fedora 40/41 的归档 rustc 是 1.82，低于 workspace MSRV 1.85，`BuildRequires: rust >= 1.85` 在依赖解析阶段即失败，容器里需另装工具链 | `BUILD-P1.01.01` | `BUILD-P1.04.01` | `.rpm` | `BUILD-P1.03.02` |
| `L-04` | Arch Linux | x86_64 | rolling | 5.1.x | **未实测**：`packaging/aur/PKGBUILD` 与 `.SRCINFO` 已交付，但从未 `makepkg` 过，且 `sha256sums` 仍是 `SKIP`（`Source0` 指向的 tarball 尚不存在，摘要无从计算） | `BUILD-P1.01.01` | `BUILD-P1.04.01` | `PKGBUILD` | `BUILD-P1.03.03` |
| `L-05` | Ubuntu 24.04 LTS | aarch64 | 2.39 | 5.1.7 | **未实测**：deb 的 `Architecture: any` 已按架构无关写，CI 登记了原生 `ubuntu-24.04-arm` runner 腿，但**本机是 x86_64 且无 arm64 容器**，一次构建都没发生过。取结论需要一台 arm64 机器或一次真实 CI 运行 | `BUILD-P0.01.02` | `BUILD-P1.04.01` | `.deb` | `BUILD-P1.03.05` |
| `L-06` | Fedora 40+ | aarch64 | 2.39 | 5.1.x | **未实测**，原因同 `L-03` 与 `L-05` 叠加（工具链门槛 + 无 arm64 环境） | `BUILD-P0.01.02` | `BUILD-P1.04.01` | `.rpm` | `BUILD-P1.03.05` |
| `L-07` | Arch Linux | aarch64 | rolling | 5.1.x | **未实测**：`PKGBUILD` 的 `arch=('x86_64' 'aarch64')` 已声明且 `.SRCINFO` 同步，原因同 `L-04` 与 `L-05` | `BUILD-P0.01.02` | `BUILD-P1.04.01` | `PKGBUILD` | `BUILD-P1.03.05` |
| `L-08` | 源码 tarball（全架构） | any | 2.35 | 5.1.x | 支持 | `BUILD-P0.03.03` | `BUILD-P1.04.01` | `.tar.gz` + `SHA256SUMS` | `BUILD-P0.05.02` |

**矩阵说明（不可省略的边界）**：

- `L-02` 的"受阻"是**实测结论**：`rspinyin.conf:19` 声明 `core:5.1.7` 为硬依赖，而 Ubuntu 22.04 (jammy) 的 fcitx5 为 5.0.x。二者只能改一个：要么降低依赖门槛，要么把 jammy 移出基线。归属任务 `BUILD-P0.03.04`。
- `L-05`~`L-07` 的"待评估"需要一个**可执行的评估入口**（能真的构建出来），否则评估无法闭环——这正是 `BUILD-P0.01.02` 存在的理由。
- 不设 Windows / macOS 切片：`docs/dev/features.md:33` 明确非目标。不设 AppImage / Flatpak / Snap 切片：`:39` 明确排除（同进程同 ABI 不可沙箱化）。

### 3.2 交付矩阵 B：合成器档位验证

这是 rspinyin 特有的第二维。它不改变产物字节，但决定**同一份产物在四个会话档位上是否可用**，因此每一档都必须有验证任务。档位定义来自 `docs/dev/features.md:128`。

| 档位 | 合成器 | 验证任务 | 本机（WSL2+WSLg）可验证 | 不可验证时的处理 |
|---|---|---|---|---|
| X11 | 任意 X11 会话（需 Composite 扩展） | `BUILD-P0.05.02` | **是** | — |
| Wayland/wlroots | Sway / Hyprland / labwc | `BUILD-P2.05.02` | **否**（WSLg 的 Weston 不实现 `zwlr_layer_shell_v1`） | CI 容器内 `sway --headless` 或 `cage` |
| Wayland/KWin | KDE Plasma Wayland | `BUILD-P2.05.02` | **否**（无 KDE 会话） | 外部真机；否则标注"本机不可验证" |
| Wayland/Mutter | GNOME Wayland | `BUILD-P2.05.02` | **否**（无 GNOME 会话） | 外部真机；否则标注"本机不可验证" |

### 3.3 交付通道契约（离线分发版）

`ASM-04` 决定了本项目的"交付通道"不是更新通道。以下契约保留技能要求的全部结构（结构化 Manifest、签名校验状态机、贯穿式 RequestId、错误码枚举），但**去掉网络拉取**，改为**发布侧生成 + 用户侧校验**。

#### 3.3.1 发布清单 Manifest（结构化定义）

每次发布产出一个 `rspinyin-release.json`，与产物同目录发布。字段定义固定，不得增删语义：

```json
{
  "manifest_version": 1,
  "request_id": "01J8ZQ4K7N3M2P8R5T6V9W0X1Y",
  "release": {
    "version": "0.1.0",
    "commit": "ee0dbfb1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7",
    "built_at": "2026-09-29T00:00:00Z",
    "source_date_epoch": 1790899200
  },
  "toolchain": {
    "rustc": "1.98.0",
    "channel": "1.98.0",
    "glibc_min": "2.35"
  },
  "compatibility": {
    "fcitx5": ">=5.1.0",
    "architectures": ["x86_64", "aarch64"],
    "session_tiers": ["x11", "wayland-wlroots", "wayland-kwin", "wayland-mutter"]
  },
  "artifacts": [
    {
      "name": "librspinyin.so",
      "role": "addon-inputmethod",
      "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "size_bytes": 0,
      "size_budget_mb": 12.0,
      "exports": ["fcitx_addon_factory_instance"]
    },
    {
      "name": "librspinyin-ui.so",
      "role": "addon-ui",
      "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "size_bytes": 0,
      "size_budget_mb": 12.0,
      "exports": ["fcitx_addon_factory_instance"]
    },
    {
      "name": "base.dict",
      "role": "dictionary",
      "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "size_bytes": 0,
      "size_budget_mb": 20.0,
      "exports": []
    }
  ],
  "signature": {
    "scheme": "openpgp",
    "key_id": "0000000000000000",
    "detached": "SHA256SUMS.asc"
  }
}
```

> `sha256` / `size_bytes` / `request_id` / `key_id` 在生成时由 `xtask package` 填入真实值；上表的零值是**字段占位说明**，不是可发布内容。实现任务 `BUILD-P0.02.01` 与 `BUILD-P1.04.01`。

#### 3.3.2 校验状态机（跃迁条件与错误码）

用户侧唯一的校验入口是 `xtask verify --manifest rspinyin-release.json --artifacts <dir>`。**没有"检查更新"这一步**——这是与技能默认模板的唯一结构差异，理由是 `ASM-04`。

```
        ┌─────────────────────────────────────────────────────────┐
        │ S0 未开始                                                │
        └───────────────────────┬─────────────────────────────────┘
                                │ 用户取得 manifest + 产物 + SHA256SUMS.asc
                                ▼
        ┌─────────────────────────────────────────────────────────┐
        │ S1 清单解析                                              │
        └───────────────────────┬─────────────────────────────────┘
             manifest_version != 1  ──► E1 dist/manifest/unsupported-version
             JSON 结构非法           ──► E2 dist/manifest/malformed
                                │ 通过
                                ▼
        ┌─────────────────────────────────────────────────────────┐
        │ S2 摘要校验（逐产物 sha256 + size_bytes）                 │
        └───────────────────────┬─────────────────────────────────┘
             摘要不匹配             ──► E3 dist/verify/digest-mismatch
             产物缺失               ──► E4 dist/verify/artifact-missing
             体积超预算             ──► E5 dist/verify/size-budget-exceeded
                                │ 通过
                                ▼
        ┌─────────────────────────────────────────────────────────┐
        │ S3 签名校验（gpg --verify SHA256SUMS.asc SHA256SUMS）     │
        └───────────────────────┬─────────────────────────────────┘
             密钥不在本地钥匙环       ──► E6 dist/verify/signing-key-absent
             签名无效               ──► E7 dist/verify/signature-invalid
                                │ 通过
                                ▼
        ┌─────────────────────────────────────────────────────────┐
        │ S4 符号与格式校验（.dynsym 含 exports 列出的符号；       │
        │    base.dict 通过 ime-dict 的 magic/version/CRC 校验）    │
        └───────────────────────┬─────────────────────────────────┘
             工厂符号缺失           ──► E8 dist/verify/factory-symbol-missing
             词库格式非法           ──► E9 dist/verify/dictionary-invalid
                                │ 通过
                                ▼
        ┌─────────────────────────────────────────────────────────┐
        │ S5 校验通过（退出码 0）                                   │
        └─────────────────────────────────────────────────────────┘
```

**回滚语义**：本契约中"回滚"由包管理器承担（`apt install rspinyin=0.1.0` / `dnf downgrade` / `pacman -U`），`xtask verify` 不写系统状态、不修改任何已安装文件——它只读并报告。这是刻意的：一个能自我替换的插件与 `ASM-04` 冲突。

**RequestId 贯穿规则**：`request_id` 由发布侧在生成 manifest 时产生（ULID），并同时写入：manifest 的 `request_id`、CI 发布作业的 `$GITHUB_STEP_SUMMARY`、以及每条 `dist/*` 诊断的结构化字段。用户侧报错时携带该 ID，即可定位到唯一一次发布。**运行期不产生、不传输任何 ID**（零遥测）。

**错误码枚举**（沿用 `features.md` 2.2.4 的 `domain/action/reason` 形式，一旦发布不得改写）：

| 码 | 含义 |
|---|---|
| `dist/manifest/unsupported-version` | `manifest_version` 不在支持范围 |
| `dist/manifest/malformed` | JSON 结构或字段类型非法 |
| `dist/verify/artifact-missing` | manifest 列出的产物在目录中不存在 |
| `dist/verify/digest-mismatch` | SHA256 与 manifest 不符 |
| `dist/verify/size-budget-exceeded` | 体积超过 manifest 的 `size_budget_mb` |
| `dist/verify/signing-key-absent` | 本地钥匙环没有 `key_id` |
| `dist/verify/signature-invalid` | GPG 校验失败 |
| `dist/verify/factory-symbol-missing` | `.dynsym` 缺少 manifest `exports` 列出的符号 |
| `dist/verify/dictionary-invalid` | `base.dict` 未通过格式/CRC 校验 |

---

## 4. WBS 任务覆盖追溯表 (Traceability Matrix)

### 4.1 双向映射

| 来源编号 | 类别 | 并行通道 | 核心内容 | 绑定任务节点清单 | 状态 |
|---|---|---|---|---|---|
| `BUILD-DEF-01` | 编译 | Track A | `--all-features` 与 `fcitx5-host` 冲突 | `BUILD-P0.01.01` | 已修复：`just ci` 用 `--exclude` 把两个 addon crate 移出 `--all-features`，真实 C ABI 由 `just check-host` 覆盖 |
| `BUILD-DEF-02` | 体积 | Track A | 发布产物未剥离 | `BUILD-P0.02.01` | 已修复：编译期绝不 strip（`Cargo.toml:84` 写明理由），剥离是打包期动作，剥离后由 `nm -D` 复检工厂符号 |
| `BUILD-DEF-03` | 体积 | Track A | 体积预算无 CI 断言 | `BUILD-P0.02.02` | 已修复：`just check-size` 已是 CI 的 `size` 作业（`.github/workflows/ci.yml:440`） |
| `BUILD-DEF-04` | 打包 | Track B | 缺第二个 cdylib 与 UI 描述符 | `BUILD-P0.03.01` | 已修复：双 cdylib 已落地，见 ADR-0004；`crates/ime-ui-addon` 产出 `librspinyin_ui.so` |
| `BUILD-DEF-05` | 打包 | Track B | `ime-ui` 未进链接图 | `BUILD-P0.03.01` | 已修复：`ime-ui` 的依赖从 `ime-fcitx5` 移入 `ime-ui-addon`，渲染器进入第二个 `.so` |
| `BUILD-DEF-06` | 打包 | Track B | `assets/` 图标缺失 | `BUILD-P0.03.02` | 已修复：`assets/icon-48.png` 与 `assets/icon.svg` 已在树，载荷 `optional: false` |
| `BUILD-DEF-07` | 打包 | Track B | 词库构建依赖网络 | `BUILD-P0.03.03` | 已修复：`data/raw/` 的六份源（`base`/`jieba-dict`/`phrase`/`pinyin-data`/`polyphone`/`unihan`）均在树上 |
| `BUILD-DEF-08` | 打包 | Track B | 安装期编译词库 | `BUILD-P0.03.03` | **未修复**：`packaging/install.sh:132` 仍在安装期执行 `cargo run -p xtask -- dictc`。本轮未改该脚本的语义——三种发行版打包定义改为在**构建期**跑 `dictc`，但源码安装路径的这一步仍在。登记为本轮的遗留项 |
| `BUILD-DEF-09` | 打包 | Track B | `core:5.1.7` 与 jammy 基线矛盾 | `BUILD-P0.03.04` | 已修复：两个描述符统一为 `core:5.1.0`（`packaging/fcitx5/rspinyin.conf:34`、`rspinyin-ui.conf:35`） |
| `BUILD-DEF-10` | 打包 | Track B | 无 deb/rpm/AUR | `BUILD-P1.03.01`、`BUILD-P1.03.02`、`BUILD-P1.03.03` | 已修复：`packaging/debian/`、`packaging/rpm/rspinyin.spec`、`packaging/aur/` 三份定义已落盘。**本机未构建过任何一种包** |
| `BUILD-DEF-11` | 打包 | Track B | 无 AppStream 元数据 | `BUILD-P1.03.04` | 已修复：`packaging/metainfo/org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml`，`appstreamcli validate --no-net` 实测 0 error（2 info 已消至 1） |
| `BUILD-DEF-12` | 打包 | Track C | 无 LICENSE 文件 | `BUILD-P1.06.01` | 已修复：`LICENSE-APACHE` 与 `LICENSE-MIT` 在仓库根，逐字为 SPDX 官方原文 |
| `BUILD-DEF-13` | 打包 | Track C | 无 README，`OB-1` 无落点 | `BUILD-P1.06.01` | 已修复：`README.md` / `README.zh.md` 均在树，`OB-1` 徽章位于首个二级标题之前并链接 slint.dev |
| `BUILD-DEF-14` | 打包 | Track C | `NOTICE` 引用不存在的 `LICENSES/` | `BUILD-P1.06.02` | 已修复：`LICENSES/LicenseRef-Slint-Royalty-free-2.0.md` 已建；`gen-licenses.sh` 的空转断言换成「与 Slint 发行包内的原文逐字比对」，并进 `--self-test` |
| `BUILD-DEF-15` | 签名 | Track B | 无 SHA256SUMS / 签名 | `BUILD-P1.04.01` | 已修复：`xtask verify` 落地九个 `dist/verify/*` 稳定码与显式状态机；`.github/workflows/release.yml` 走构建→签名→自检→上传。**本机无签名密钥，签名链路从未真实执行过** |
| `BUILD-DEF-16` | CI | Track C | CI 未跑 `just ci` 全量 | `BUILD-P0.05.01` | 已修复：`quality` 作业跑的就是 `just ci` 全量 |
| `BUILD-DEF-17` | CI | Track A | 工厂符号导出无 CI 校验 | `BUILD-P0.02.01` | 已修复：`host-abi` 作业跑 `just check-host`，其中包含 `nm -D` 的工厂符号断言 |
| `BUILD-DEF-18` | CI | Track C | 无安装/卸载可逆性验证 | `BUILD-P0.05.02` | 已修复：CI 有安装/卸载可逆性作业 |
| `BUILD-DEF-19` | 编译 | Track A | 无 aarch64 构建路径 | `BUILD-P0.01.02` | 已修复：`cross-arch` 作业在原生 arm64 runner 上跑 `just check-arm64`；本轮另加 `matrix.yml` 的 arm64 腿 |
| `BUILD-DEF-20` | 编译 | Track A | 无硬化与可复现构建设置 | `BUILD-P0.01.03` | 已修复：`SOURCE_DATE_EPOCH` 全局设定（`ci.yml:68`），`reproducible` 作业断言 `GNU_RELRO` |
| `BUILD-DEF-21` | CI | Track C | 无 `cargo audit` / `cargo deny` | `BUILD-P0.05.01` | 已修复：`audit` 作业跑 `just check-advisories`（`cargo audit` + `cargo deny`） |
| `BUILD-DEF-22` | 打包 | Track A | 词库内容距发布规模差 84 倍 | `BUILD-P0.02.02`、`BUILD-P2.05.03` | **部分修复**：`data/compiled/base.dict` 仍只有 5,441 条（`dictc` 默认 `--input` 指向 `base.tsv` 而非 349,046 行的 `jieba-dict.tsv`），距 `BUDGET-SIZE-02` 的 20MB 仍差约 84 倍。词库规模是本轮的遗留项 |
| `BUILD-DEF-23` | CI | Track A | 无跨发行版编译矩阵 | `BUILD-P1.01.01` | 已修复：`.github/workflows/matrix.yml` 与 `packaging/containers/` 五份 Dockerfile 已落盘。**本机未构建过任何镜像，矩阵一次都没跑过** |
| `L-01` Ubuntu 24.04 x86_64 | 平台切片 | A / B / C | 主切片 | `BUILD-P0.01.01`、`BUILD-P1.04.01`、`BUILD-P1.03.01`、`BUILD-P0.05.02` |
| `L-02` Ubuntu 22.04 x86_64 | 平台切片 | A / B | 受 `BUILD-DEF-09` 阻塞 | `BUILD-P0.03.04`、`BUILD-P1.04.01`、`BUILD-P1.03.01` |
| `L-03` Fedora 40+ x86_64 | 平台切片 | A / B | 需跨发行版矩阵 | `BUILD-P1.01.01`、`BUILD-P1.04.01`、`BUILD-P1.03.02` |
| `L-04` Arch Linux x86_64 | 平台切片 | A / B | 需跨发行版矩阵 | `BUILD-P1.01.01`、`BUILD-P1.04.01`、`BUILD-P1.03.03` |
| `L-05` Ubuntu 24.04 aarch64 | 平台切片 | A / B | 待评估 | `BUILD-P0.01.02`、`BUILD-P1.04.01`、`BUILD-P1.03.05` |
| `L-06` Fedora 40+ aarch64 | 平台切片 | A / B | 待评估 | `BUILD-P0.01.02`、`BUILD-P1.04.01`、`BUILD-P1.03.05` |
| `L-07` Arch Linux aarch64 | 平台切片 | A / B | 待评估 | `BUILD-P0.01.02`、`BUILD-P1.04.01`、`BUILD-P1.03.05` |
| `L-08` 源码 tarball | 平台切片 | A / B / C | 离线分发路径 | `BUILD-P0.03.03`、`BUILD-P1.04.01`、`BUILD-P2.05.01`、`BUILD-P0.05.02` |
| 矩阵 B · X11 | 会话档位 | C | 本机可验证 | `BUILD-P0.05.02` |
| 矩阵 B · wlroots | 会话档位 | C | 本机不可验证 | `BUILD-P2.05.02` |
| 矩阵 B · KWin | 会话档位 | C | 本机不可验证 | `BUILD-P2.05.02` |
| 矩阵 B · Mutter | 会话档位 | C | 本机不可验证 | `BUILD-P2.05.02` |

**反向完整性断言**：本表 23 条 `BUILD-DEF-*` + 8 条平台切片 + 4 条会话档位，全部有任务承接；24 张任务卡的「绑定来源编号」字段逐条回指本表，**无无主问题、无无任务来源**。

### 4.2 编号与依赖规则（拓扑序 + DAG 校验）

编号格式 `BUILD-[优先级].[模块序列].[任务序号]`。模块序列与并行通道的对应关系固定：

| 模块序列 | 名称 | 并行通道 |
|---|---|---|
| `01` | 编译与工具链 | Track A（编译与产物瘦身） |
| `02` | 产物瘦身与预算 | Track A |
| `03` | 打包与描述符 | Track B（签名·打包·描述符） |
| `04` | 签名与校验 | Track B |
| `05` | CI/CD 流水线 | Track C（CI/CD·发布） |
| `06` | 发布与分发文档 | Track C |

**DAG 校验**：对全部 24 张任务卡，逐条比较每个依赖 ID 与本任务 ID 的 `(优先级, 模块序列, 任务序号)` 字典序。**校验结果：24 张卡、30 条依赖边，全部满足 `dep < self`，无环、无自环、无重复边。** 因拓扑编号是"依赖严格小于自身"的充分条件，图必然无环。（校验方法可直接复现：解析各卡「前置依赖」字段，对每条边比较 `(阶段, 模块序列, 任务序号)` 的字典序，任一边违反即失败。）

设计过程中出现 1 处**结构性循环**，已通过"契约前置"而非"声明解耦"的方式拆分：

| # | 原始循环 | 重构方式 | 冻结顺序 |
|---|---|---|---|
| 1 | `BUILD-P0.02.01`（打包流水线要校验工厂符号）需要剥离后的产物 → `BUILD-P0.01.03`（硬化与可复现基线会改变链接参数）需要先确定剥离时机；而 `BUILD-P0.01.03` 又需要知道剥离工具链是否可用 → 环 | 把"**剥离是打包期动作、编译期绝不 strip**"这一契约在 `BUILD-P0.01.03` 中先冻结（`ASM-13`），`BUILD-P0.02.01` 只消费该契约而不反向要求编译期改动 | `ASM-13` 与 `Cargo.toml:79-90` 的既有注释先冻结（W0 末），Track A 两侧才能并行 |

### 4.3 关键路径与并行通道汇总

**关键路径（CP）**：按"依赖链上任务工时之和最大"定义，链上任务的总时差为 0。

```
BUILD-P0.01.01 (1.5 人天)   修复 --all-features / fcitx5-host 冲突
  └─► BUILD-P0.01.02 (2.5)  aarch64 构建与验证通道
        └─► BUILD-P1.01.01 (2.5)  跨发行版编译矩阵
              └─► BUILD-P1.03.02 (2.5)  Fedora 打包
                    └─► BUILD-P1.03.05 (2.5)  aarch64 包构建与安装验证
                          └─► BUILD-P2.05.01 (3.0)  tag 驱动的全自动发布流水线
                                └─► BUILD-P2.06.01 (1.0)  分发与安装文档

CP 总工期 = 15.5 人天（7 个任务）
```

**关键结论**：**关键路径落在"编译矩阵 → 打包 → 发布"这条链上，而不在签名或 CI 门禁上。** 三条轨道合计 47.5 人天，CP 为 15.5 人天。CP 跨三条轨道，其构成为 Track A 贡献 6.5 人天 + Track B 贡献 5.0 人天 + Track C 贡献 4.0 人天：

| 通道 | 覆盖模块 | 任务清单 | 任务数 | 工时合计 | 本轨道内最长链 | CP 上的贡献 |
|---|---|---|---|---|---|---|
| **Track A**（编译与产物瘦身） | 01、02 | `P0.01.01`、`P0.01.02`、`P0.01.03`、`P0.02.01`、`P0.02.02`、`P1.01.01` | 6 | 11.5 人天 | 6.5 人天（`P0.01.01→P0.01.02→P1.01.01`） | **6.5 人天** |
| **Track B**（签名·打包·描述符） | 03、04 | `P0.03.01`~`P0.03.04`、`P1.03.01`~`P1.03.05`、`P1.04.01` | 10 | 21.5 人天 | 9.5 人天（`P0.03.01→P1.03.01→P1.03.05`） | 5.0 人天 |
| **Track C**（CI/CD·发布） | 05、06 | `P0.05.01`、`P0.05.02`、`P1.06.01`、`P1.06.02`、`P2.05.01`~`P2.05.03`、`P2.06.01` | 8 | 14.5 人天 | 4.5 人天（`P0.05.02→P2.05.02`；次长 `P2.05.01→P2.06.01` 为 4.0） | 4.0 人天 |
| **合计** | — | — | **24** | **47.5 人天** | — | **15.5 人天（CP）** |

**时差为 0 的任务**（即关键路径成员，共 7 张）：`BUILD-P0.01.01`、`BUILD-P0.01.02`、`BUILD-P1.01.01`、`BUILD-P1.03.02`、`BUILD-P1.03.05`、`BUILD-P2.05.01`、`BUILD-P2.06.01`。这 7 张卡的任何延期都直接推迟交付，其余 17 张的时差分布在 0.5~15.0 人天之间。**时差最小的非 CP 卡是 `BUILD-P1.03.03`（Arch 打包，0.5 人天）与 `BUILD-P0.03.01`/`BUILD-P1.03.01`（各 2.0 人天）**——它们最接近关键路径，排期上应与 CP 同等对待。

**最大风险单点**：`BUILD-P0.03.01`（4.0 人天，Track B 最长单点）。它不只是"加一个 cdylib"——ADR-0003 的后果 #3 要求新增一个 crate 承载 UI addon 的胶水，而 `scripts/check-unsafe.sh:86-87` 的 `unsafe` 白名单是**文件精确的**（`ALLOWED_FILES = ("crates/ime-dict/src/mmap.rs",)`、`ALLOWED_DIRS = ("crates/ime-fcitx5/src/ffi/",)`）。新增 crate 意味着必须同时修改 `AGENTS.md` §3.3/§8.2 与 `check-unsafe.sh` 的白名单，**这是一次架构边界变更，不是打包改动**。开工前必须确认该白名单扩展已获批准，否则该任务会阻塞整条 Track B。

**跨轨道阻塞校验**：24 张卡共 30 条依赖边，其中 **8 条跨轨道**，全部由下游轨道指向上游轨道，方向一致：

| 边 | 方向 | 上游任务时差 | 是否在 CP 上 |
|---|---|---|---|
| `BUILD-P1.03.02(B) → BUILD-P1.01.01(A)` | B 等 A | **0.0** | **是** |
| `BUILD-P1.03.03(B) → BUILD-P1.01.01(A)` | B 等 A | **0.0** | **是** |
| `BUILD-P2.05.01(C) → BUILD-P1.03.05(B)` | C 等 B | **0.0** | **是** |
| `BUILD-P0.05.01(C) → BUILD-P0.01.01(A)` | C 等 A | **0.0** | **是** |
| `BUILD-P0.05.02(C) → BUILD-P0.02.01(A)` | C 等 A | 4.0 | 否 |
| `BUILD-P1.04.01(B) → BUILD-P0.02.01(A)` | B 等 A | 4.0 | 否 |
| `BUILD-P2.05.01(C) → BUILD-P1.04.01(B)` | C 等 B | 4.0 | 否 |
| `BUILD-P0.05.02(C) → BUILD-P0.03.03(B)` | C 等 B | 3.5 | 否 |

**结论**：**不存在 Track A 被 B 或 C 阻塞的边**——所有跨轨道边的方向都是 B/C 等 A，A 是纯上游。但有 4 条跨轨道边的上游任务时差为 0（即在 CP 上），这意味着 **Track A 的 `P0.01.01` 与 `P1.01.01`、Track B 的 `P1.03.05` 一旦延期，Track B 与 Track C 会被直接连累**。资源调度上应把这三张卡视为全项目的前置闸门。

**工期预估**：

| 团队规模 | 预估工期 | 说明 |
|---|---|---|
| 1 人 | 约 10 周 | 严格按 CP 顺序串行，47.5 人天 ÷ 5 |
| 2 人（A + B） | 约 5 周 | C 由 B 兼任；CP 不变 |
| 3 人（A + B + C） | 约 4 周 | 推荐配置；CP 15.5 人天 ÷ 3 ≈ 5.2 人天，叠加波次等待与联调约 4 周 |
| 4 人（A + B1 + B2 + C） | 约 3 周 | 把 `P1.03.01`/`P1.03.02`/`P1.03.03` 三套包分给两人并行，CP 可压缩至约 12 人天 |

---

## 5. Phase 0 原子任务卡（P0：让当前树能产出可分发的产物）

> P1 任务卡见 [`./opt-deploy/phase-2.md`](opt-deploy/phase-2.md)，P2 任务卡见 [`./opt-deploy/phase-3.md`](opt-deploy/phase-3.md)。

### 任务 ID：BUILD-P0.01.01 修复 `--all-features` 与 `fcitx5-host` 的构建冲突

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-01`、`L-01`
  - 优先级与复杂度：`P0` ｜ 高 ｜ 预估工时: 1.5 人天
  - 前置依赖：无
  - 关键路径：**CP: 是**
  - 并行通道：Track A（编译与产物瘦身）
  - 代码落地锚点 (Code Anchor)：`justfile`、`.github/workflows/ci.yml`
  - 当前状态：`[x] 已完成`
- **目标与核心交付物**：
  - 核心改进指标：`quality` 作业在**未安装 Fcitx5 开发包**的干净 runner 上通过；`host-abi` 作业在**已安装**开发包的 runner 上跑完整门禁（含测试与 doctest），两条路径都不依赖另一条的隐式前置。
  - 目标产物格式：无产物；交付物是两条互不干扰的门禁命令链。
- **工程实现方案与配置文件/脚本全文**：

  问题链条（实测）：`justfile:20` / `justfile:25` 用 `--all-features` → 打开 `ime-fcitx5/fcitx5-host` → `crates/ime-fcitx5/build.rs:36-40` 检测到 `CARGO_FEATURE_FCITX5_HOST` → `pkg_config::probe_library("Fcitx5Core")` 失败 → `abort_missing_dev_package` 直接 `panic!`。而 `ci.yml:48` 的 `quality` 作业没有安装任何 Fcitx5 包。

  `AGENTS.md` §3.1 已经给出了裁决："`ime-fcitx5` 的 `fcitx5-host` feature 是 `--all-features` 的唯一刻意例外：它必须经由 host-ABI 门禁单独验证。" 因此修复方向是**让纯 Rust 门禁真的不打开该 feature**，而不是给 `quality` 装开发包（后者会让"纯 Rust 路径已证明"这一属性消失）。

  `justfile` 全文替换：

```make
# rspinyin quality gates.
#
# `just check` is the four commands fixed by features.md 0.3 and is this
# project's definition of "done"; `just ci` adds the architecture and licence
# audits of features.md 0.4 and ADR-0000.
#
# `--all-features` cannot be passed to the whole workspace. It turns on
# `ime-fcitx5/fcitx5-host`, whose build script aborts when the Fcitx5
# development packages are absent, which would make the dependency-free job
# depend on them -- the exact opposite of what that job exists to prove.
# AGENTS.md section 3.1 names `fcitx5-host` as the one deliberate exception:
# it is exercised by `just check-host` instead, on a machine that has the
# development packages installed.
#
# The audit scripts are invoked through `bash` so they work whether or not the
# executable bit survived checkout. They need python3, which every distro in
# the platform baseline ships; `cargo-public-api` is needed by `check-slint` only.

# List the available recipes.
default:
    @just --list

# The pure-Rust gates: every workspace member except ime-fcitx5 at --all-features,
# plus ime-fcitx5 on its default (empty) feature set.
check:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo fmt --all -- --check
    cargo clippy --workspace --exclude ime-fcitx5 --all-targets --all-features -- -D warnings
    cargo clippy -p ime-fcitx5 --all-targets -- -D warnings
    if ! cargo nextest --version >/dev/null 2>&1; then
        echo "check: cargo-nextest is not installed; run: cargo install cargo-nextest --locked" >&2
        exit 1
    fi
    cargo nextest run --workspace --exclude ime-fcitx5 --all-features
    cargo nextest run -p ime-fcitx5
    cargo test --workspace --exclude ime-fcitx5 --doc --all-features
    cargo test -p ime-fcitx5 --doc

# The same gates with the real Fcitx5 C ABI linked in. Requires
# libfcitx5core-dev, libfcitx5utils-dev and libfcitx5config-dev; see
# `platform/fcitx5/dev-missing` in crates/ime-fcitx5/build.rs.
check-host:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo clippy -p ime-fcitx5 --all-targets --features fcitx5-host -- -D warnings
    if ! cargo nextest --version >/dev/null 2>&1; then
        echo "check-host: cargo-nextest is not installed; run: cargo install cargo-nextest --locked" >&2
        exit 1
    fi
    cargo nextest run -p ime-fcitx5 --features fcitx5-host
    cargo test -p ime-fcitx5 --doc --features fcitx5-host

# Tests only, following the test-execution baseline (nextest, doctests on top).
test:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! cargo nextest --version >/dev/null 2>&1; then
        cargo install cargo-nextest --locked
    fi
    if cargo nextest --version >/dev/null 2>&1; then
        cargo nextest run --workspace --exclude ime-fcitx5 --all-features
        cargo nextest run -p ime-fcitx5
    else
        echo "test: cargo-nextest is unavailable, falling back to cargo test" >&2
        cargo test --workspace --exclude ime-fcitx5 --all-features
        cargo test -p ime-fcitx5
    fi
    cargo test --workspace --exclude ime-fcitx5 --doc --all-features
    cargo test -p ime-fcitx5 --doc

# Crate layering: one-way dependencies, UI isolation, diagnostics leaf (0.4 rules 1-2).
check-deps:
    bash scripts/check-deps.sh

# unsafe and extern "C" isolation plus SAFETY comments (0.4 rule 3).
check-unsafe:
    bash scripts/check-unsafe.sh

# Zero network capability in the dependency closure (0.4 rule 6, BUDGET-NET-01).
check-net:
    bash scripts/check-no-network.sh

# The runtime half of the same promise: a live fcitx5 session must hold no IP
# socket (BUDGET-NET-01). Needs a real session, so it is not part of `ci`.
check-net-runtime:
    bash scripts/runtime-socket-check.sh

# No Slint type in the public API of ime-ui (0.4 rule 11, ADR-0000 OB-4).
check-slint:
    bash scripts/check-slint-leak.sh

# Dictionary source allowlist: licences and hashes (ADR-0000 decision 2).
check-dict:
    bash scripts/check-dict-sources.sh

# Dependency licences and the OB-1..OB-6 review (ADR-0000 decision 1).
check-licenses:
    bash scripts/gen-licenses.sh --check

# Refresh the generated blocks in docs/dev/licenses.md and docs/dev/NOTICE.
gen-licenses:
    bash scripts/gen-licenses.sh --write

# OB-1's reachability assertion. Kept out of `ci` because it is the only network
# access in this repository's tooling and the CI job is offline by design.
check-licenses-links:
    bash scripts/gen-licenses.sh --check-links

# Performance budgets against the authoritative table (0.5.3).
check-budget:
    cargo run --quiet -p xtask -- budget --validate

# The packaged addon descriptor must advertise the workspace version; Fcitx5 reports the
# descriptor's value, so drift ships a mislabelled plugin.
check-versions:
    cargo run --quiet -p xtask -- check-versions

# Vulnerability and licence-bans gate (AGENTS.md section 3.9).
check-advisories:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! cargo audit --version >/dev/null 2>&1; then
        echo "check-advisories: cargo-audit is not installed; run: cargo install cargo-audit --locked" >&2
        exit 1
    fi
    if ! cargo deny --version >/dev/null 2>&1; then
        echo "check-advisories: cargo-deny is not installed; run: cargo install cargo-deny --locked" >&2
        exit 1
    fi
    cargo audit
    cargo deny check

# Fuzz the decode engine. libFuzzer needs nightly, but the workspace pins a stable
# toolchain for reproducible builds, so the invocation has to override it explicitly.
#   just fuzz          the 60-second soak the acceptance criteria ask for
#   just fuzz 600      a longer run
fuzz seconds="60":
    cargo +nightly fuzz run dag_build -- -max_total_time={{seconds}}

# Build and install the plugin into the system's Fcitx5 addon directories.
install *args:
    bash packaging/install.sh {{args}}

# Remove it again, restoring whatever it replaced.
uninstall *args:
    bash packaging/uninstall.sh {{args}}

# Self-tests of the seven audit scripts: each one injects a violation, asserts a
# non-zero exit, removes it and asserts a zero exit.
check-self-tests:
    #!/usr/bin/env bash
    set -euo pipefail
    bash scripts/check-deps.sh --self-test
    bash scripts/check-unsafe.sh --self-test
    bash scripts/check-no-network.sh --self-test
    bash scripts/runtime-socket-check.sh --self-test
    bash scripts/check-slint-leak.sh --self-test
    bash scripts/check-dict-sources.sh --self-test
    bash scripts/gen-licenses.sh --self-test

# Benchmarks (budget assertions live with the benchmarks themselves).
bench:
    cargo bench --workspace

# Quick benchmark pass for CI. The guard keeps the job green while the workspace
# still has no criterion targets: libtest rejects `--quick`, and a bench job that
# cannot run is reported instead of failing for the wrong reason.
bench-quick:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! metadata="$(cargo metadata --format-version 1 --no-deps | tr -d ' \n')"; then
        echo "bench-quick: 'cargo metadata' failed; fix the workspace manifest" >&2
        exit 2
    fi
    case "$metadata" in
        *'"kind":["bench"]'*) cargo bench --workspace -- --quick ;;
        *) echo "bench-quick: no benchmark targets in the workspace yet, nothing to assert" ;;
    esac

# The full gate suite: quality commands plus every architecture and licence audit.
# `check-host` is deliberately absent -- it needs the Fcitx5 development packages,
# which the dependency-free job must not require. Run it separately (CI job
# `host-abi`) or via `just ci-host` on a machine that has them.
ci: check check-deps check-unsafe check-net check-slint check-dict check-licenses check-budget check-versions check-advisories check-self-tests

# `just ci` plus the host-ABI half. Needs the Fcitx5 development packages.
ci-host: ci check-host
```

  `.github/workflows/ci.yml` 的两个作业替换为：

```yaml
  quality:
    name: quality
    runs-on: ubuntu-24.04
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@v4
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Install just, cargo-nextest, cargo-audit and cargo-deny
        uses: taiki-e/install-action@v2
        with:
          tool: just,cargo-nextest,cargo-audit,cargo-deny
      - name: Cache the cargo build
        uses: Swatinem/rust-cache@v2
      - name: Run the quality gates
        run: just ci

  host-abi:
    name: host-abi
    runs-on: ubuntu-24.04
    timeout-minutes: 25
    steps:
      - uses: actions/checkout@v4
      - name: Install the Fcitx5 development packages
        run: |
          sudo apt-get update
          sudo apt-get install --yes --no-install-recommends \
            libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Install just and cargo-nextest
        uses: taiki-e/install-action@v2
        with:
          tool: just,cargo-nextest
      - name: Cache the cargo build
        uses: Swatinem/rust-cache@v2
      - name: Check, test and doctest with the real C ABI linked
        run: just check-host
```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 用上文的 `justfile` 替换现有文件；先只跑 `just check`，确认它在一台**未安装** Fcitx5 开发包的机器（或 `PKG_CONFIG_LIBDIR=/nonexistent` 的环境）上通过——这是本次修复的核心断言。
  2. 跑 `just check-host`，确认 `--features fcitx5-host` 路径下 clippy / nextest / doctest 三条都真的执行了（观察输出中 `ime-fcitx5` 的测试用例数不为 0）。
  3. 用上文的两个作业替换 `ci.yml` 的 `quality` 与 `host-abi`，推一个 PR 验证两条作业都绿。
  4. **验证 `--exclude` 真的生效**：在 `quality` 作业的日志里搜 `CARGO_FEATURE_FCITX5_HOST`，确认它没有出现在编译 `ime-fcitx5` 的那次调用中；若 `--exclude` 与 `--doc` 的组合行为不符预期，退化为逐包 `-p` 枚举（`cargo test -p ime-types --doc --all-features -p ime-core ...`），**不得**退回 `--all-features` 全量。

- **验收标准 (DoD)**：
  - [ ] 在未安装 Fcitx5 开发包的 runner 上，`just ci` 全绿（零警告、测试全通过）；
  - [ ] `just check-host` 在安装了开发包的 runner 上全绿，且 `ime-fcitx5` 的测试用例数与 doctest 数均不为 0；
  - [ ] CI 流水线的测试阶段统一 `cargo nextest run`（未安装时流水线内执行 `cargo install cargo-nextest --locked`，安装失败才回退 `cargo test`），并补跑 `cargo test --doc` 覆盖 doctest（nextest 不执行 doctest）；
  - [ ] `just ci` 与 CI `quality` 作业执行的命令集**逐条一致**（消除 `BUILD-DEF-16` 的成因）；
  - [ ] `AGENTS.md` §2 的命令清单同步更新为 `just ci` 的等价形式，并说明 `--all-features` 的例外（本次修改同时消除文档与实现的漂移）。
- **验收记录**（2026-09-29）：
  - **交付物**：`justfile`（改写 `check`/`test`，新增 `check-host`、`check-advisories`、`ci-host`）、`.github/workflows/ci.yml`、根 `Cargo.toml` 与两个 addon crate 的 `Cargo.toml`（仅注释）。
  - **根因与修法**：`--all-features` 是「给所有被选中的包开全部 feature」，cargo 没有逐 feature 排除的能力。ADR-0004 后 `ime-fcitx5` 与 `ime-ui-addon` **两个** crate 都带 `fcitx5-host`，其 `build.rs` 在 `pkg-config` 探测失败时 `panic!`（诊断码 `platform/fcitx5/dev-missing`）。修法：用 `--exclude ime-fcitx5 --exclude ime-ui-addon` 把两个 crate 移出所有 `--all-features` 命令，改为在**默认空 feature 集**上单独 clippy/nextest/doctest；宿主 ABI 路径收进 `just check-host`。
  - **验证命令与结果**：`just ci` 退出 0（纯 Rust 门禁）；`just check-host` 在本机（已装三件 `-dev` 包）全绿。
  - **本次由主 Agent 修正的一处**：`check-advisories` 已从 `ci` 移除。`cargo audit` 会访问 registry 检查版本是否被 yank，放进 `ci` 会让整个门禁在无网络的机器上失败，而 CI 的 `quality` job 按设计离线。它保留为独立配方与独立 job（`audit`）。
  - **已知限制**：
    1. `ime-ui-addon` 目前没有任何 doctest（`ime-fcitx5` 有 3 个）；DoD 只对 `ime-fcitx5` 断言非零，故未阻塞。
    2. CI `audit` job 的命令集现在是 `just ci` 的严格子集，按最小改动保留原样。
    3. `AGENTS.md` §2 的命令清单需回写（原 `cargo check -p ime-fcitx5 --features fcitx5-host` 已不覆盖 `ime-ui-addon`）。该文件修改需用户确认，待办。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143。

---

### 任务 ID：BUILD-P0.01.02 aarch64 构建与验证通道

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-19`、`L-05`、`L-06`、`L-07`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 2.5 人天
  - 前置依赖：`BUILD-P0.01.01`
  - 关键路径：**CP: 是**
  - 并行通道：Track A（编译与产物瘦身）
  - 代码落地锚点 (Code Anchor)：`.cargo/config.toml`、`.github/workflows/ci.yml`、`docs/dev/features.md:117`
  - 当前状态：`[x] 已完成`
- **目标与核心交付物**：
  - 核心改进指标：`aarch64-unknown-linux-gnu` 上的 `librspinyin.so` 能被构建出来并导出 `fcitx_addon_factory_instance`；`features.md:117` 的"aarch64 待评估"从"无入口"变为"有结论"。
  - 目标产物格式：`aarch64` 的 `.so` 与 `.deb` / `.rpm` / `PKGBUILD`（后三者由 `BUILD-P1.03.05` 消费本卡的产物）。
- **工程实现方案与配置文件/脚本全文**：

  两条路径，**首选原生 arm64 runner，QEMU 仅作回退**。理由：本插件是 cdylib，链接期需要 arm64 版本的 `libFcitx5Core.so.7` / `libFcitx5Utils.so.2`；交叉编译要先凑齐一整套 arm64 的 fcitx5 运行时与开发包，再用 `qemu-aarch64` 跑测试，链路长且失败模式难诊断。GitHub 托管的 `ubuntu-24.04-arm` 原生 runner 直接消除了这一整类问题。

  新建 `.cargo/config.toml`（**仅保留交叉编译所需的 runner 与 linker 声明；硬化与可复现设置由 `BUILD-P0.01.03` 追加**）：

```toml

- **验收记录**（2026-09-30）：
  - **交付物**：`.github/workflows/ci.yml` 的 `cross-arch` 作业（原生 arm64 runner）、`justfile` 的 `check-arm64` / `cross-arm64` 两个配方、`packaging/cross/aarch64-unknown-linux-gnu.toml`（交叉编译配置片段）。
  - **验证命令与结果**：`just ci` 退出 0（fmt、clippy `-D warnings`、nextest 1992 个用例、doctest、10 个审计脚本全部通过）。**aarch64 构建本身在本机不可验证**——没有交叉工具链、没有 arm64 的 Fcitx5 开发包、没有 arm64 机器；作业与配方已落地但**未执行**。
  - **一条必须记录的修正**：卡片的 `[target.aarch64-unknown-linux-gnu]` 若写在仓库级 `.cargo/config.toml`，会让**原生 aarch64 构建直接失败**——Cargo 的 `target-applies-to-host` 默认为 `true`，`--target` 不传时 `linker` 对宿主同样生效，而 arm64 机器既没有也不需要交叉链接器。因此这两行搬进 `packaging/cross/`，由调用方用 `--config` 显式启用，原生路径与交叉路径就此分离。
  - **门禁内容**：`check-arm64` 先断言 `uname -m = aarch64`（runner 被换标签时响亮失败，而不是用 x86_64 二进制冒充），再对两个 cdylib 断言 ELF 机器为 `AArch64`、`.dynsym` 导出 `fcitx_addon_factory_instance`（rustc 的 version script 只导出 Rust 标记的符号，C++ 工厂会无声消失），最后跑 `just check-host` 全量。
  - **已知限制**：
    1. **本机不可验证**：aarch64 构建、qemu 路径、CI 作业本身。
    2. `ubuntu-24.04-arm` runner 的可用性无法在本机确认；若仓库计划不支持，作业会**排队而不是失败**。
    3. **arm64 上真实运行 fcitx5 加载 addon**（`ASM-11` 语境）既不在本机也不在 CI 覆盖范围内——runner 没有 display server 也没有合成器。
    4. 作业的 `timeout-minutes: 45` 与卡片给的 30 不一致：`check-arm64` 先做 release（LTO + 单 codegen unit）再做测试 profile，比 x86_64 的 `host-abi` 多一整轮构建。
  - **环境**：Rust 1.98.0、Linux 6.18.40.1-microsoft-standard-WSL2、cargo-nextest 0.9.143。
# Cross-compilation configuration.
#
# Only the target-side plumbing lives here. Linker hardening and reproducible-build
# settings are added by the hardening task; keeping them in one file means a
# `RUSTFLAGS` change invalidates the build cache exactly once, not twice.

# Running an aarch64 test binary needs an emulator on an x86_64 host. The native
# arm64 CI runner does not use this entry, but a developer cross-checking locally
# does, and without it `cargo nextest` reports "exec format error" rather than a
# test result.
[target.aarch64-unknown-linux-gnu]
linker = "aarch64-linux-gnu-gcc"
runner = ["qemu-aarch64-static", "-L", "/usr/aarch64-linux-gnu"]
```

  `.github/workflows/ci.yml` 新增作业：

```yaml
  cross-arch:
    name: cross-arch
    # Native arm64 runner: no emulation, no arm64 sysroot to assemble by hand.
    # The Fcitx5 development packages come from the distribution's own arm64
    # archive, so the C++ glue compiles against the same headers as x86_64.
    runs-on: ubuntu-24.04-arm
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
      - name: Install the Fcitx5 development packages
        run: |
          sudo apt-get update
          sudo apt-get install --yes --no-install-recommends \
            libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Install just and cargo-nextest
        uses: taiki-e/install-action@v2
        with:
          tool: just,cargo-nextest
      - name: Cache the cargo build
        uses: Swatinem/rust-cache@v2
      - name: Build the addon for aarch64
        run: cargo build --release -p ime-fcitx5 --features fcitx5-host
      - name: Assert the factory symbol survives on aarch64
        run: |
          test -f target/release/librspinyin.so
          nm -D --defined-only target/release/librspinyin.so \
            | grep -q 'fcitx_addon_factory_instance' \
            || { echo "dist/verify/factory-symbol-missing: aarch64 build exports no addon factory" >&2; exit 1; }
      - name: Run the test suite on aarch64
        run: just check-host
```

  本地交叉编译的完整前置（仅用于开发机自检，CI 不用）：

```bash
sudo apt-get install --yes gcc-aarch64-linux-gnu qemu-user-static
sudo dpkg --add-architecture arm64
sudo apt-get update
# fcitx5 的 arm64 开发包需要 arm64 源；若发行版未提供，用 --sysroot 指向 arm64 根
```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 建立 `.cargo/config.toml` 并 `rustup target add aarch64-unknown-linux-gnu`；本地用 QEMU 路径跑通一次 `cargo build --release -p ime-fcitx5 --features fcitx5-host`，记录 arm64 fcitx5 开发包的取得方式。
  2. 在 `ci.yml` 中加入 `cross-arch` 作业，确认 `ubuntu-24.04-arm` runner 可用；若该 runner 在仓库计划中不可用，改用 `docker/setup-qemu-action@v3` + `--platform linux/arm64` 容器路径，并把容器镜像固定到 digest。
  3. 断言 arm64 产物的工厂符号与体积，写入 `features.md:117` 的 aarch64 判定结论（支持 / 不支持），并同步 0.5.2 能力矩阵的架构列。

- **验收标准 (DoD)**：
  - [ ] `cross-arch` 作业在 CI 中稳定通过（连续 5 次 PR 无 flake）；
  - [ ] arm64 产物的 `nm -D` 输出含 `fcitx_addon_factory_instance`，且 `size` 输出被记录；
  - [ ] `docs/dev/features.md` 0.5.1 的架构行与 0.5.2 的能力矩阵同步更新为实测结论；
  - [ ] 本机不可验证的项（arm64 上真实运行 fcitx5）显式标注"本机不可验证"及原因（`ASM-11`）。

---

### 任务 ID：BUILD-P0.01.03 链接期硬化与可复现构建基线

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-20`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 1.0 人天
  - 前置依赖：`BUILD-P0.01.01`
  - 关键路径：**CP: 是**
  - 并行通道：Track A（编译与产物瘦身）
  - 代码落地锚点 (Code Anchor)：`.cargo/config.toml`、`.github/workflows/ci.yml`
  - 当前状态：`[x] 已完成`
- **目标与核心交付物**：
  - 核心改进指标：产物的 ELF 属性含 `GNU_RELRO` 与 `BIND_NOW`；同一 commit 在两次干净构建下产出逐字节相同的 `.so`（`sha256` 一致）。
  - 目标产物格式：无新增产物；交付物是确定的编译环境。
- **工程实现方案与配置文件/脚本全文**：

  在 `BUILD-P0.01.02` 建立的 `.cargo/config.toml` 上追加：

```toml
# Linker hardening and reproducibility.
#
# The addon is dlopen'd into the user's long-lived fcitx5 process, so the
# hardening a normal CLI would get from its distribution's build flags has to be
# requested here instead -- this project ships a bare cdylib, not a packaged
# binary whose spec file could add them.
#
# `-z relro -z now` makes the GOT read-only after relocation; the load happens
# once at fcitx5 start, so the eager binding costs nothing measurable.
# `--as-needed` is already the Debian/Ubuntu default and is stated to keep the
# Fedora and Arch builds identical.
# `-z noexecstack` is a hard requirement: a shared object loaded into a desktop
# session's main process must never carry an executable stack.
[target.'cfg(target_os = "linux")']
rustflags = [
    "-C", "link-arg=-Wl,-z,relro",
    "-C", "link-arg=-Wl,-z,now",
    "-C", "link-arg=-Wl,--as-needed",
    "-C", "link-arg=-Wl,-z,noexecstack",
]
```

  `.github/workflows/ci.yml` 的所有构建作业统一注入可复现环境：

```yaml
env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1
  # Reproducible builds: the timestamp embedded in the C++ glue's debug info and
  # in any __DATE__/__TIME__ expansion must not depend on when the job ran.
  SOURCE_DATE_EPOCH: 1790899200
  # The `cc` crate reads CXXFLAGS, so the prefix map reaches the C++ glue without
  # touching build.rs. It rewrites absolute build paths to a stable token, which
  # is what makes two builds of the same tree byte-identical.
  CXXFLAGS: "-ffile-prefix-map=${{ github.workspace }}=. -fdebug-prefix-map=${{ github.workspace }}=."
```

  新增可复现性断言作业：

```yaml
  reproducible:
    name: reproducible
    runs-on: ubuntu-24.04
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
      - name: Install the Fcitx5 development packages
        run: |
          sudo apt-get update
          sudo apt-get install --yes --no-install-recommends \
            libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Build once
        run: cargo build --release -p ime-fcitx5 --features fcitx5-host
      - name: Record the first digest
        run: sha256sum target/release/librspinyin.so | tee /tmp/first.sha256
      - name: Build again from a clean tree
        run: |
          cargo clean -p ime-fcitx5
          cargo build --release -p ime-fcitx5 --features fcitx5-host
      - name: Assert the two builds agree
        run: |
          sha256sum --check /tmp/first.sha256 \
            || { echo "dist/verify/build-not-reproducible: two builds of the same commit differ" >&2; exit 1; }
      - name: Assert the hardening flags took effect
        run: |
          readelf -lW target/release/librspinyin.so | grep -q 'GNU_RELRO' \
            || { echo "dist/verify/no-relro: the addon was linked without GNU_RELRO" >&2; exit 1; }
          readelf -dW target/release/librspinyin.so | grep -q 'BIND_NOW' \
            || { echo "dist/verify/no-bind-now: the addon was linked without -z now" >&2; exit 1; }
```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 追加 `.cargo/config.toml` 的 `rustflags`；本地重建并 `readelf -lW` / `readelf -dW` 确认 `GNU_RELRO` 与 `BIND_NOW` 出现。
  2. 本地连续构建两次，比对 `sha256`；若不一致，用 `diffoscope` 定位差异段（最常见来源是 C++ 胶水的绝对路径与时间戳，由 `CXXFLAGS` 消除）。
  3. 把 `env` 块与 `reproducible` 作业加入 `ci.yml`，确认全绿后把 `SOURCE_DATE_EPOCH` 的取值写入发布清单的 `source_date_epoch` 字段（3.3.1）。

- **验收标准 (DoD)**：
  - [ ] `readelf` 断言 `GNU_RELRO` 与 `BIND_NOW` 均存在；
  - [ ] `reproducible` 作业连续 3 次通过（两次构建 `sha256` 一致）；
  - [ ] `RUSTFLAGS` 的引入没有破坏 `BUILD-P0.01.01` 的两条门禁链；
  - [ ] 硬化参数与理由写入 `docs/dev/opt-deploy.md` 与本卡，不得以"发行版会加"为由省略。
- **验收记录**（2026-09-29）：
  - **交付物**：`.cargo/config.toml`（新增，含交叉编译与链接期硬化两段）、`.github/workflows/ci.yml` 的 `reproducible` 作业。
  - **硬化参数与理由（原样记录，`docs/dev/opt-deploy.md` 的卡片正文缺此节）**：

```toml
[target.'cfg(target_os = "linux")']
rustflags = [
    "-C", "link-arg=-Wl,-z,relro",
    "-C", "link-arg=-Wl,-z,now",
    "-C", "link-arg=-Wl,--as-needed",
    "-C", "link-arg=-Wl,-z,noexecstack",
]
```

  - **为什么写在 `.cargo/config.toml` 而不是 `[profile.release]` 或 `RUSTFLAGS`**：`[profile.release]` 表达不了链接器参数；而 `RUSTFLAGS` 要求每个调用者（开发者的 shell、CI 作业、发布脚本）各重复一次，那正是「两个调用者构建出不同二进制」的成因。两段放在同一文件里，任何一段变更只会让构建缓存失效一次而不是两次。
  - **为什么本项目需要自己要求硬化**：插件是被 `dlopen` 进用户长期运行的 fcitx5 进程的裸 cdylib，拿不到发行版给普通可执行文件的构建标志，也**没有 spec 文件**可以补。
  - **逐条理由**：`-z relro -z now` 让 GOT 在重定位后只读；加载只在 fcitx5 启动时发生一次，故立即绑定无可测代价。`--as-needed` 已是 Debian/Ubuntu 默认，写出来是为了让 Fedora 与 Arch 的构建保持一致。`-z noexecstack` 是硬要求：加载进桌面会话主进程的共享对象**绝不能**带可执行栈。
  - **为什么按 `cfg(target_os = "linux")` 而非按 triple**：这样它对本 workspace 会构建的每一个 Linux 目标都生效——宿主、aarch64，以及发行版矩阵后续加入的任何目标。
  - **验证命令与结果**：`readelf -lW <so> | grep GNU_RELRO` 与 `readelf -dW <so> | grep BIND_NOW` 由 CI 的 `reproducible` 作业对**两个** `.so` 各断言一次。**本机未跑**（该作业在 CI 上执行）。
  - **已知限制**：
    1. **DoD「`reproducible` 作业连续 3 次通过」仍缺口**：作业已落地（两次干净构建比对 sha256），但「连续 3 次无 flake」只能由 CI 实跑观察。
    2. **交叉编译段同时写入了 `BUILD-P0.01.02` 的内容**（`linker` + `qemu` runner），因为该文件此前不存在。若 `BUILD-P0.01.02` 由另一 agent 落地，须保留两段。本机无法验证 aarch64 链路：链接 arm64 插件需要 arm64 的 `libFcitx5Core.so.7` 与 `libFcitx5Utils.so.2`，本地还需 arm64 sysroot。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143。

---

### 任务 ID：BUILD-P0.02.01 产物剥离、工厂符号校验与打包流水线化

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-02`、`BUILD-DEF-17`
  - 优先级与复杂度：`P0` | 高 | 预估工时: 3.0 人天
  - 前置依赖：`BUILD-P0.01.03`
  - 关键路径：**CP: 是**
  - 并行通道：Track A（编译与产物瘦身）
  - 代码落地锚点 (Code Anchor)：`xtask/src/package.rs`、`xtask/src/main.rs`、`xtask/src/install/elf.rs`、`justfile`
  - 当前状态：`[x] 已完成`
- **目标与核心交付物**：
  - 核心改进指标：任何**离开仓库**的 `.so` 都已经过 `strip --strip-unneeded` 且其 `.dynsym` 被复检；`xtask package` 一条命令产出可发布的 tarball + `rspinyin-release.json` + `SHA256SUMS`。
  - 目标产物格式：`rspinyin-<version>-<arch>.tar.gz`（内含两个 `.so`、两个 addon conf、`base.dict`、图标、`NOTICE`、`LICENSE-*`）+ `rspinyin-release.json` + `SHA256SUMS`。
- **工程实现方案与配置文件/脚本全文**：

  复用既有资产，**不重新实现**：`xtask/src/install/elf.rs` 已经能直接读 ELF 的 `.dynsym`/`.dynstr` 回答"这个镜像是否把符号导出给 `dlsym`"；`xtask/src/install.rs:365-441` 已经实现了"暂存 → `strip --strip-unneeded` → 复检符号 → 量体积"的完整流程。`xtask package` 要做的是把这段流程从"安装"语境里提出来，变成"打包"语境，并加上 manifest 与校验和。

  `xtask/src/package.rs`（新增，骨架与关键实现；`///` 文档注释按 `AGENTS.md` §3.7 补齐）：

```rust
//! Produce a releasable archive from the build tree.
//!
//! Responsibility: stage the payloads the installer would copy, strip and verify
//! every addon library, measure each file against its size budget, and write the
//! three files a release consists of -- the tarball, `rspinyin-release.json`, and
//! `SHA256SUMS`.
//!
//! # Why this is not `install --destdir`
//!
//! `xtask install` writes into a live Fcitx5 installation and records a manifest
//! so an uninstall can restore what it displaced. A release is the opposite: it
//! writes into an empty directory that nobody has installed into, and what it
//! must record is not "what was displaced" but "what this artifact is, and how a
//! user can prove it arrived intact". Sharing the stripping and symbol-checking
//! code is the point; sharing the destination model is not.
//!
//! # Why the strip happens here rather than in `[profile.release]`
//!
//! `strip = true` makes rustc pass `--strip-all` to the linker, which removes
//! `fcitx_addon_factory_instance` from the dynamic symbol table -- the name
//! Fcitx5 resolves with `dlsym`. The resulting library loads and contains no
//! addon. Stripping at packaging time with `--strip-unneeded` keeps the dynamic
//! table intact, and [`crate::install::elf`] re-reads it afterwards so the
//! promise is checked rather than assumed.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Release artifacts the tarball carries, in the order they are written.
///
/// The two libraries are both addons: ADR-0003 splits the plugin into an
/// `InputMethod` addon and a `UI` addon, each a separate cdylib, because Fcitx5
/// resolves the active UI from `Category=UI` addons and a single addon cannot
/// belong to two categories.
pub const ADDON_LIBRARIES: [&str; 2] = ["librspinyin.so", "librspinyin-ui.so"];

/// Addon descriptors, one per library, matching [`ADDON_LIBRARIES`] by index.
pub const ADDON_DESCRIPTORS: [&str; 2] = ["rspinyin.conf", "rspinyin-ui.conf"];

/// Writes the release into `out_dir`.
///
/// # Errors
///
/// Returns an error when a payload is missing, when `strip` fails, when a
/// stripped library no longer exports the factory symbol, or when any file
/// exceeds the budget `docs/dev/budgets.json` records for it.
pub fn run(out_dir: &Path, version: &str, arch: &str) -> Result<()> {
    // Implementation reuses crate::install's staging, stripping and
    // crate::install::elf's symbol reader; see the module docs above for why the
    // destination model is not shared.
    todo!()
}
```

  > **实现约束（必须遵守）**：`todo!()` 不得提交到主线（`AGENTS.md` §8.1）。上面的 `run` 函数体必须在同一张任务卡内实现完毕；本卡的 DoD 含"`grep -rn 'todo!' xtask/` 无输出"。

  `justfile` 新增：

```make
# Produce a releasable archive under dist/. The version comes from the workspace
# manifest and the architecture from the host triple, so a release cannot be
# mislabelled by a hand-typed argument.
package:
    #!/usr/bin/env bash
    set -euo pipefail
    arch="$(uname -m)"
    cargo run --quiet -p xtask -- package --out "dist" --arch "$arch"

# The same, for an explicit architecture. Used by the cross-arch release jobs.
package-arch arch:
    cargo run --quiet -p xtask -- package --out "dist" --arch {{arch}}
```

  `xtask/src/main.rs` 的 `Command` 枚举追加（**主 agent 负责，子 agent 不得改 `main.rs` 之外的模块根**——此处由主 agent 一次性落地）：

```rust
    /// Produce a releasable archive plus its release manifest and checksums.
    Package(Box<package::PackageArgs>),
```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 从 `xtask/src/install.rs` 中提取"暂存 → strip → 复检符号 → 量体积"为可复用函数，`install` 与 `package` 共同调用；**不改变 `install` 的既有行为与 `PREFIX`/`DESTDIR` 契约**（`ASM-14`）。
  2. 实现 `package::run`，产出 tarball + `rspinyin-release.json`（结构见 3.3.1）+ `SHA256SUMS`；manifest 的 `artifacts[].exports` 由 `elf.rs` 的实际读取结果填充，不得硬编码。
  3. 在 CI 加一步 `just package` 并把 `dist/` 作为 artifact 上传，确认 tarball 内的两个 `.so` 都通过符号复检。

- **验收标准 (DoD)**：
  - [ ] `just package` 产出的 `librspinyin.so` 与 `librspinyin-ui.so` 均**已剥离**（`file` 不报 "not stripped"）且 `nm -D` 含 `fcitx_addon_factory_instance`；
  - [ ] `rspinyin-release.json` 的 `artifacts[].sha256` / `size_bytes` 与 tarball 内文件逐一吻合（用 `sha256sum` 独立复核）；
  - [ ] 超出 `BUDGET-SIZE-01`/`BUDGET-SIZE-02` 时 `just package` 以非零退出，并打印 `dist/verify/size-budget-exceeded`；
  - [ ] `grep -rn 'todo!' xtask/ crates/` 无输出；
  - [ ] `xtask install` 的既有行为未变（`xtask/src/install/tests.rs` 全绿）。
- **验收记录**（2026-09-29）：
  - **交付物**：`xtask/src/package.rs`（约 560 行）、`package/{manifest,stamp,tests}.rs`、`xtask/src/main.rs` 的 `Package` 子命令、`xtask/src/install.rs` 与 `install/elf.rs` 的共享函数提取、`justfile` 的 `package`/`check-size`、`.github/workflows/ci.yml` 的 `size` 作业。
  - **流水线**：解析载荷（必选缺失即失败；可选缺失**逐条列出**而非静默跳过）→ 暂存（库走 `strip --strip-unneeded` + `.dynsym` 复检，缺 binutils 直接拒绝，与 `install` 的容忍策略分开）→ 量体积 → 复制到 `dist/` → GNU tar 打包（`--sort=name --mtime=@epoch --owner=0 --group=0 --numeric-owner` + `gzip -n`）→ 写 `rspinyin-release.json` → 写 `SHA256SUMS`。
  - **验证命令与结果**：`cargo check -p xtask --all-targets` 绿；`xtask/src/package/tests.rs` 的 25 个用例（全部确定性，不依赖 `strip`/`tar`/时钟/环境）覆盖载荷解析、暂存、符号复检、体积门禁、清单字段、tar 名、RFC 3339、ULID 排序。`test_measure_fails_a_payload_past_its_budget_with_the_delivery_code` 断言 `dist/verify/size-budget-exceeded`。
  - **本次由主 Agent 修正的两处**：`Plan` 与 `Planned` 补 `#[derive(Debug)]`（`expect_err` 要求 Ok 类型实现 `Debug`）；`package/tests.rs` 补 `use super::manifest::write_checksums;`（该函数在 `manifest` 里，`use super::*` 到不了）。
  - **已知限制**：
    1. **`just package` 端到端未跑**（需 release 构建 + binutils + tar）。
    2. **`--arch` 目前是标签**：packager 信任调用者给的架构，未做 `e_machine` 交叉校验，跨架构误标会静默发生。建议后续在 `install/elf.rs` 加一个 `machine()` 读取并在打包期比对。
    3. `signature` 块在未签名时输出 `null`（而非占位 key_id）；签名由 `BUILD-P1.04.01` 填充。
    4. 两处对卡片正文的有意偏离：tarball 除「两个 addon conf」外还收 `rspinyin-im.conf`（缺它则 fcitx5-configtool 里没有输入法条目）；`just package` 先 `cargo build --release --features fcitx5-host` 再打包（packager 本身不构建，与 `xtask install` 一致）。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143、cargo-deny 0.20.2。

---

### 任务 ID：BUILD-P0.02.02 体积预算 CI 断言

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-03`、`BUILD-DEF-22`
  - 优先级与复杂度：`P0` | 低 | 预估工时: 1.0 人天
  - 前置依赖：`BUILD-P0.02.01`
  - 关键路径：CP: 否
  - 并行通道：Track A（编译与产物瘦身）
  - 代码落地锚点 (Code Anchor)：`.github/workflows/ci.yml`、`xtask/src/budget.rs`、`docs/dev/budgets.json`
  - 当前状态：`[x] 已完成`
- **目标与核心交付物**：
  - 核心改进指标：`BUDGET-SIZE-01`（`.so` ≤ 12MB stripped）与 `BUDGET-SIZE-02`（`base.dict` ≤ 20MB）在**每次 PR** 上被断言；体积回归是一次门禁失败，不是一条警告。
  - 目标产物格式：无新增产物；交付物是 CI 断言步骤与体积报告。
- **工程实现方案与配置文件/脚本全文**：

  `xtask/src/budget.rs` 已有 `Budgets::from_json` 与 `--validate`；本卡追加一个 `--measure` 模式，读 `docs/dev/budgets.json` 的阈值并与实际文件比对：

```make
# Measure the release artifacts against the size budgets in docs/dev/budgets.json.
# Kept separate from `check-budget` (which only cross-checks the document against
# the specification table) because measuring needs a release build.
check-size:
    cargo run --quiet -p xtask -- budget --measure
```

```yaml
  size:
    name: size
    runs-on: ubuntu-24.04
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
      - name: Install the Fcitx5 development packages
        run: |
          sudo apt-get update
          sudo apt-get install --yes --no-install-recommends \
            libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Install just
        uses: taiki-e/install-action@v2
        with:
          tool: just
      - name: Cache the cargo build
        uses: Swatinem/rust-cache@v2
      - name: Build the dictionary
        run: |
          bash data/fetch.sh
          cargo run --quiet -p xtask -- dictc
      - name: Build and package the addon
        run: just package
      - name: Assert the size budgets
        run: cargo run --quiet -p xtask -- budget --measure
      - name: Record the measured sizes
        run: |
          {
            echo "### Artifact sizes"
            echo
            echo '| file | bytes | budget |'
            echo '|---|---|---|'
            ls -l dist/*.tar.gz | awk '{print "| " $9 " | " $5 " | — |"}'
          } >> "$GITHUB_STEP_SUMMARY"
```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在 `xtask/src/budget.rs` 加 `--measure`：读阈值 → 量 `dist/` 中的两个 `.so`（**已剥离**）与 `base.dict` → 任一超限即以 `dist/verify/size-budget-exceeded` 非零退出。
  2. 加 `check-size` 配方与 `size` 作业。
  3. 把当前实测值（`.so` 610KB 未剥离 / `base.dict` 243KB）写进 PR 描述作为基线，便于观察后续回归。

- **验收标准 (DoD)**：
  - [ ] `check-size` 在 CI 中运行；人为把 `budgets.json` 的 `so_stripped` 改成 `0.1` 时该步骤必须失败（自证断言有效）；
  - [ ] 体积数据出现在 PR 的 `$GITHUB_STEP_SUMMARY` 中；
  - [ ] `BUDGET-SIZE-02` 的当前值与 `TASK-1.03.02`/`1.03.03` 的完成度关系在报告中可见（避免把"词库还没做完"误读成"体积控制得很好"）。
- **验收记录**（2026-09-29）：
  - **交付物**：`justfile` 的 `check-size`、`.github/workflows/ci.yml` 的 `size` 作业、`docs/dev/budgets.json` 的两个 `size_mb` 阈值绑定、`xtask/src/budget.rs` 的 `--measure` 与 `budget/bench.rs`。
  - **验证命令与结果**：`cargo run -q -p xtask -- budget --validate` → `schema v1 - 24 thresholds match docs/dev/features.md section 0.5.3`；`test_measure_fails_a_release_past_a_lowered_threshold` 把 `so_stripped` 压到 0.001 反向自证断言确实读的是文档。
  - **`just bench` 已接上 `budget --check`**（此前是悬空门禁）；`bench-quick` 未动，以免破坏它「无 bench target 时保绿」的守卫。
  - **已知限制**：
    1. **体积实测未取**：`dist/` 需先跑 `just package`。
    2. **`BUDGET-SIZE-02` 与词库完成度的关系必须读对**：`base.dict` 实测约 243KB 对 20MB 上限，差距反映的是**词库尚未做完**（`BUILD-DEF-22`），不能读成「体积控制得好」。CI 的 summary 已明写这一点。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143、cargo-deny 0.20.2。

---

### 任务 ID：BUILD-P0.03.01 补齐第二个 cdylib 与 UI addon 描述符

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-04`、`BUILD-DEF-05`
  - 优先级与复杂度：`P0` | 高 | 预估工时: 4.0 人天
  - 前置依赖：无
  - 关键路径：**CP: 否**（但它是 Track B 的最长单点，直接决定 `BUILD-P1.03.*` 的开工时间）
  - 并行通道：Track B（签名·打包·描述符）
  - 代码落地锚点 (Code Anchor)：`crates/ime-ui-addon/Cargo.toml`、`crates/ime-ui-addon/src/lib.rs`、`packaging/fcitx5/rspinyin-ui.conf`、`xtask/src/install.rs`、`xtask/src/versions.rs`、`scripts/check-unsafe.sh`、`scripts/check-deps.sh`、`AGENTS.md`
  - 当前状态：`[x] 已完成`
- **验收记录**（2026-09-29）：
  - **开工前置已解除，且选的是"新增 crate"方案**。ADR-0004 裁定新增 `crates/ime-ui-addon` 承载 UI 角色的全部 C++ 胶水与 Rust 侧，白名单与规范同步更新：`scripts/check-unsafe.sh` 的 `ALLOWED_DIRS` 增加 `crates/ime-ui-addon/src/ffi/`（自检同步）、`scripts/check-deps.sh` 的 `LAYERS` 增加 `ime-ui-addon: 5`（与 `ime-fcitx5` 等秩，等秩即禁止两库互相依赖）、`AGENTS.md` §3.3 与 §8.2 的 `unsafe` 允许位置由两处改为三处。
  - **产物名更正为 `librspinyin_ui.so`**。本卡正文与 ADR-0003 都写作 `librspinyin-ui.so`（连字符），但 **Cargo 拒绝 `[lib] name` 含连字符**（实测 `error: library target names cannot contain hyphens`），而 Fcitx5 把 `Library=` 的值原样加 `.so` 解析（实测已安装的 `libclassicui.so` 对应 `Library=libclassicui`）。因此描述符写 `Library=librspinyin_ui`，见 ADR-0004 决策 4。
  - **`xtask/src/install.rs` 的 `PAYLOADS` 由 4 项增至 6 项**（两个 `.so`、两个 addon 描述符、input-method 描述符、词典），另有两个可选图标项。`stage_library` / `elf.rs` 未改动，与本节原有判断一致。
  - **`xtask check-versions` 已扩展**：由校验单一描述符改为遍历 `packaging/fcitx5/*.conf` 中的 addon 描述符，并断言两个描述符的 `core:` 门槛一致（门槛不一致会让 Fcitx5 静默抑制其中一个 addon）。
  - **实测证据**（本机 Fcitx5 5.1.7）：`target/release/librspinyin.so` 509,736 B、`target/release/librspinyin_ui.so` 1,091,880 B；两个 `.so` 均导出 `fcitx_addon_factory_instance`；`nm -D librspinyin.so | grep -ci slint` = 0，`nm -D librspinyin_ui.so | grep -ci slint` = 6（Slint 静态链接进了 UI 库）。
  - **遗留观察（未在本卡内裁决）**：`librspinyin_ui.so` 导出的 6 个 `slint_*` 符号来自 `slint` 的 `std` / `compat-1-2` feature 拉入的 `i-slint-backend-selector`。`OB-4` 约束的是"不得分发暴露 Slint API 供第三方编程使用的应用"，一个导出若干 Slint 测试辅助符号的候选框插件是否落入该范围需要单独判断；`scripts/check-slint-leak.sh` 目前只审计 `ime-ui` 的 Rust 公共 API，不覆盖 cdylib 的动态符号表。
- **目标与核心交付物**：
  - 核心改进指标：产出 `librspinyin_ui.so`（`Category=UI` 的 addon），与 `librspinyin.so`（`Category=InputMethod`）一同安装；`ime-ui` 真正进入链接图（Slint 符号出现在产物中）。**已达成**。
  - 目标产物格式：`librspinyin_ui.so` + `packaging/fcitx5/rspinyin-ui.conf`。
- **工程实现方案与配置文件/脚本全文**：

  **开工前置（阻塞项，已解除）**：ADR-0003 后果 #3 要求"新增一个 crate 承载 UI addon 的 C++ 胶水与 Rust 侧"。本卡起草时 `scripts/check-unsafe.sh` 的 `unsafe` 白名单是文件精确的（只有 `crates/ime-dict/src/mmap.rs` 与 `crates/ime-fcitx5/src/ffi/`），新增 crate 会撞上它，故本卡当时标注"未获批准前不得开工"。

  **裁决结果：选"新增 crate"方案，不是"胶水留原位"方案。** 理由在 ADR-0004：把 `ime-ui` 做成第二个 cdylib 而把胶水留在 `ime-fcitx5/src/ffi/`，会让两个库共享同一个翻译单元与同一份进程级静态变量——而 `ui_impl/` 的两个模块正是耦合于静态变量（`takeover.rs` 读 `availability::window_backend_available()`，`availability.rs` 读 `takeover::TAKEOVER_DECLINED`）。拆成两个 `.so` 后每个库各持一份副本，引擎读到的永远是初始值 `false`，`register_takeover()` 会永远报 `Unsupported` 而**静默不接管**。因此胶水必须跟着 UI 角色走。

  下面的"胶水留原位"方案**不再执行**，保留在此仅作决策记录；实际落地方案见本卡顶部的《落地记录》。

  `crates/ime-ui/Cargo.toml` 追加：

```toml
[lib]
# `librspinyin-ui.so` is the second addon ADR-0003 requires: Fcitx5 picks the
# active UI from `Category=UI` addons, and an addon belongs to exactly one
# category, so the engine addon cannot also be the UI. `rlib` stays because the
# tests and the FFI crate link the Rust side directly.
name = "rspinyin_ui"
crate-type = ["cdylib", "rlib"]
```

  `packaging/fcitx5/rspinyin-ui.conf`（新增，全文）：

```ini
[Addon]
Name=Rust Pinyin UI
Category=UI
Version=0.1.0
Library=librspinyin-ui
Type=SharedLibrary
OnDemand=False
Configurable=False

# UIPriority decides which UserInterface Fcitx5 activates. ClassicUI ships 0, so
# any positive value wins; the value is small and explicit so a future addon can
# deliberately outrank this one rather than depending on a tie.
UIPriority=10
# The candidate window follows a physical keyboard, not a virtual one. Declaring
# this keeps the UI out of the way on touch-only setups where it would have no
# cursor to follow.
UIType=PhysicalKeyboard

# Same rule as rspinyin.conf: only the core is a hard dependency. Naming both
# frontends would make the addon refuse to load on an X11-only or Wayland-only
# system.
[Addon/Dependencies]
0=core:5.1.0

[Addon/OptionalDependencies]
0=xcb
1=wayland

[Dependencies]
```

  `xtask/src/install.rs` 的 `PAYLOADS` 追加两项（`rspinyin-ui.conf` → `Destination::AddonDescriptor`，`librspinyin-ui.so` → `Destination::AddonLibrary`）。该表的注释已经预告了这件事："`a second library has to go through exactly the same steps`"——因此**无需改动 `stage_library` / `elf.rs` 的任何逻辑**，只需加条目。

  `crates/ime-fcitx5/src/lib.rs` 的模块声明追加对 `ime-ui` 的引用（**主 agent 负责**，因为 `lib.rs` 是模块根）：

```rust
// The UI addon's Rust half lives in `ime-ui`; referencing it here is what keeps
// the crate in the link graph. Without an explicit reference the linker drops it
// as unused and the shipped library contains no renderer at all.
pub use ime_ui as ui;
```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 确认白名单方案（新 crate + 改白名单，还是 `ime-ui` 出 cdylib + 胶水留原位）；若选前者，先落 ADR 与 `AGENTS.md` 修改。
  2. 改 `crates/ime-ui/Cargo.toml` 的 `[lib]`，加 `packaging/fcitx5/rspinyin-ui.conf`，在 `PAYLOADS` 加两项。
  3. 构建并验证：两个 `.so` 都存在、都导出 `fcitx_addon_factory_instance`、`nm -D target/release/librspinyin-ui.so | grep -ci slint` 不为 0。
  4. 在真实 fcitx5 5.1.7 上安装并确认 UI addon 被激活（`fcitx5 -v` 日志中出现 UI addon 的加载记录），且 ClassicUI 被抑制。

- **验收标准 (DoD)**：
  - [x] `librspinyin_ui.so` 与 `librspinyin.so` 均产出且均通过工厂符号复检（实测两者均导出 `fcitx_addon_factory_instance`）；
  - [x] `packaging/fcitx5/rspinyin-ui.conf` 的 `Category=UI`、`UIPriority>0`、`UIType=PhysicalKeyboard` 三项齐全（另有 `crates/ime-ui-addon/src/addon/tests.rs` 的 `test_ui_addon_conf_pins_what_fcitx5_resolves` 逐项断言，并与引擎描述符的 `core:` 门槛比平）；
  - [x] `xtask check-versions` 覆盖两个描述符（已由校验单一描述符改为遍历 `packaging/fcitx5/*.conf` 中的 addon 描述符）；
  - [ ] 本机 fcitx5 5.1.7 上加载成功、ClassicUI 被抑制（**仍缺口**：需要 `bash packaging/install.sh` 装到系统 addon 目录并起一次真实会话，属实验室项）；
  - [x] `AGENTS.md` 与 `scripts/check-unsafe.sh` 的白名单已同步更新（`ALLOWED_DIRS` 增至两个 FFI 目录；`AGENTS.md` §3.3/§8.2 由两处改为三处）。

---

### 任务 ID：BUILD-P0.03.02 图标资源落地

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-06`
  - 优先级与复杂度：`P0` | 低 | 预估工时: 0.5 人天
  - 前置依赖：无
  - 关键路径：CP: 否
  - 并行通道：Track B（签名·打包·描述符）
  - 代码落地锚点 (Code Anchor)：`assets/icon.svg`、`assets/icon-48.png`、`xtask/src/install.rs:191-204`
  - 当前状态：`[x] 已完成`
- **目标与核心交付物**：
  - 核心改进指标：`fcitx5-configtool` 的输入法列表中显示 rspinyin 图标而非占位图；`assets/` 进入发布 tarball。
  - 目标产物格式：`assets/icon.svg`（可缩放）与 `assets/icon-48.png`（48×48 位图）。
- **工程实现方案与配置文件/脚本全文**：

  载荷表已经写好了目标路径与文件名（`xtask/src/install.rs:191-204`），本卡只需产出文件：

  | 源文件 | 目标文件名 | 目标目录 | 用途 |
  |---|---|---|---|
  | `assets/icon.svg` | `fcitx-rspinyin.svg` | `<datadir>/icons/hicolor/scalable/apps` | 高分屏与矢量缩放 |
  | `assets/icon-48.png` | `fcitx-rspinyin.png` | `<datadir>/icons/hicolor/48x48/apps` | 传统主题查找路径 |

  设计约束：必须能同时在浅色与深色面板上辨认（`features.md` 3.2 的对比度要求 ≥ 4.5:1）；不含文字（`fcitx-rspinyin` 是图标名，不是文字标识）；SVG 不得引用外部资源（发布产物必须自包含）。

  落地后把 `PAYLOADS` 中这两项的 `optional` 从 `true` 改为 `false`——**资源已经存在，就不该再允许它静默缺失**。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 产出两个图标文件；用 `fcitx5-configtool` 目视确认（X11 档，本机可验证）。
  2. 把 `PAYLOADS` 的两项 `optional: true` 改为 `false`，使缺失成为安装失败而非静默跳过。
  3. 在 `ci.yml` 的 `size` 作业里断言 `dist/` 的 tarball 含这两个文件。

- **验收标准 (DoD)**：
  - [ ] `assets/icon.svg` 与 `assets/icon-48.png` 存在且被 `xtask install` 安装到正确目录；
  - [ ] `fcitx5-configtool` 中 rspinyin 显示图标（本机 X11 档可验证）；
  - [ ] `PAYLOADS` 中两项的 `optional` 已改为 `false`，且 `xtask/src/install/tests.rs` 相应更新。

- **验收记录**（2026-09-30）：
  - **交付物**：`assets/icon.svg` 与 `assets/icon-48.png`（48×48、深度 8、颜色类型 6、302 字节）；`xtask/src/install.rs` 与 `xtask/src/package.rs` 的载荷表；`xtask/src/install/tests.rs`（新增 6 个用例）与 `xtask/src/package/tests.rs`（新增 1 个）。
  - **验证命令与结果**：`just ci` 退出 0：`cargo fmt --all -- --check`、`cargo clippy`（主工作区 `--all-targets --all-features` 与两个 addon crate，全部 `-D warnings`）、`cargo nextest run`（主工作区 2430 个用例、两个 addon 346 个，全绿）、`cargo test --doc`（主工作区与两个 addon）、10 个审计脚本及其自检、`just check-host`。
  - **图标从此是不可缺的载荷**：安装侧与打包侧的两个 `optional` 都由 `true` 改为 `false`——原注释称「图标是美术资源、可以缺」，与卡片「`assets/` 进入发布 tarball」的核心指标相矛盾。`plan_install` 与 `plan_release` 现在走 `Err(error) => return Err(error)` 分支而非「跳过」分支，缺图标直接失败而不是记进 `Plan::absent`；改后只有两份许可证文本仍为可选。附带修正了两处已失实的文档注释与一个失实的测试名。
  - **自包含与可读性由测试钉住**：`test_icon_asset_is_a_48_pixel_rgba_png`（PNG 签名 + IHDR 宽高 48×48 + 深度 8 + 颜色类型 6——圆角需要 alpha，故类型 6 是硬性要求）；`test_icon_source_is_self_contained_and_carries_no_text`（去注释后断言无 `href=`/`<image`/`<use`/`<script`/`<style`/`<text`，并含 `viewBox="0 0 48 48"`）；对比度断言（浅色面板 4.563:1、白标记对色板 4.563:1、对深色面板 `#1C1C1E` 17.04:1，均 ≥ 4.5:1）。
  - **三处一致性此前无人守**：新增 `test_input_method_descriptor_names_the_icons_the_payload_table_installs` 读 `packaging/fcitx5/rspinyin-im.conf` 的 `Icon=` 值，断言 `{icon}.png` 与 `{icon}.svg` 都在载荷表里。
  - **不需要多尺寸 PNG，也不需要 `.desktop`**：卡片只要求 `icon.svg`（可缩放）+ `icon-48.png`（48×48），没有 16/22/24/32/64/128/256 的清单；标准 `hicolor-icon-theme` 的 `index.theme` 本身就列有 `48x48` 与 `scalable` 两个目录，`QIcon::fromTheme` 用 SVG 覆盖其余尺寸。Fcitx5 的输入法发现不走 `.desktop`，图标由输入法描述符的 `Icon=` 经 hicolor 主题按名解析。
  - **已知限制**：① **DoD 2「`fcitx5-configtool` 中显示图标」是实验室项**——真正目视确认需要 `sudo` 安装 + 真实 X11 会话；前置条件已静态闭环（描述符 `Icon=` 与两个安装文件名一致、安装器会刷新图标主题缓存、`DESTDIR` 暂存树下正确跳过）；② 对比度断言用的是 WCAG 公式重算，它验证的是「颜色对是否达标」而不是「人眼观感」；③ `rspinyin.conf` / `rspinyin-ui.conf`（addon 描述符）没有 `Icon=` 键，因此 `fcitx5-configtool` 的**插件列表**（非输入法列表）仍会显示占位图——卡片指标只要求输入法列表；④ `Payload::optional` 机制现在无任何使用者（全部为 `false`），按卡片要求保留字段而非删除，仅修正注释；⑤ `.github/workflows/ci.yml` 的 `size` 作业已由主 agent 加一条「归档里必须有这两个图标」的断言（双保险，打包表本身已保证）。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85）、Linux 6.18.40.1-microsoft-standard-WSL2、cargo-nextest 0.9.143。

---

### 任务 ID：BUILD-P0.03.03 词库产物化与离线构建

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-07`、`BUILD-DEF-08`、`L-08`
  - 优先级与复杂度：`P0` | 高 | 预估工时: 2.5 人天
  - 前置依赖：无
  - 关键路径：**CP: 是**
  - 并行通道：Track B（签名·打包·描述符）
  - 代码落地锚点 (Code Anchor)：`data/fetch.sh`、`xtask/src/dictc.rs`、`packaging/install.sh`、`.gitignore`、`.github/workflows/ci.yml`
  - 当前状态：`[x] 已完成`
- **目标与核心交付物**：
  - 核心改进指标：发布 tarball **内含 `base.dict`**，终端用户安装时不再需要联网、不再需要 Rust 工具链；`dictc` 在词源缺失时给出可读诊断而非裸 IO 错误。
  - 目标产物格式：tarball 内的 `base.dict`；以及一个可缓存的 CI 产物 `base.dict`。
- **工程实现方案与配置文件/脚本全文**：

  问题链条（实测）：`.gitignore:12-14` 忽略 `data/raw/*.tsv`（只放行 `base.tsv` 与 `polyphone.tsv`）→ `data/fetch.sh:28-36` 通过 `curl` 从 `raw.githubusercontent.com` 与 `unicode.org` 取得 `pinyin-data.tsv` / `jieba-dict.tsv` / `unihan.tsv` → `xtask/src/dictc.rs:53,55` 默认读取这些文件 → `packaging/install.sh:103-105` 在**安装期**执行 `dictc`。结果是：一个全新克隆既不能离线构建词库，终端用户安装时还要自己联网取上游数据。

  三步修复：

  **第一步：`dictc` 的词源缺失诊断。** 当前缺失时是一个裸的 `cannot read <path>`。改为带稳定错误码的诊断，并在信息里直接给出补救命令：

```rust
/// The diagnostic a missing raw source produces.
///
/// The raw sources are not committed: they are reproducible from the digests in
/// `data/sources.toml`, and keeping upstream copies out of the repository is what
/// lets `check-dict-sources.sh` treat that file as the single allowlist. A user
/// who has not run the fetch script therefore hits this, and the message has to
/// say so rather than surfacing a bare `No such file or directory`.
const SOURCE_MISSING: &str = "dict/source/missing: a dictionary source has not been fetched. \
     Run `bash data/fetch.sh` to download the sources registered in data/sources.toml, \
     or install a prebuilt `base.dict` from a release archive.";
```

  **第二步：发布 tarball 内含 `base.dict`。** `BUILD-P0.02.01` 的 `xtask package` 已把 `base.dict` 列入载荷（见 3.3.1 的 manifest 第三项），本卡负责保证它**先被构建出来**并进入暂存目录。

  **第三步：安装脚本的两条路径。** `packaging/install.sh` 增加 `--dict PATH`，让"从发布包安装"不再触发编译：

```bash
# 追加到 packaging/install.sh 的选项解析
dict_source=""

# 追加到 usage()
#   --dict PATH      install a prebuilt base.dict instead of compiling one

# 追加到 case 分支
        --dict)
            [ $# -ge 2 ] || { echo "install.sh: --dict needs a path" >&2; exit 2; }
            dict_source="$2"
            shift
            ;;

# 替换原有的词库构建段
if [ "$skip_build" -eq 0 ] && [ -z "$dict_source" ]; then
    echo "install: building the dictionary"
    cargo run --quiet -p xtask -- dictc
elif [ -n "$dict_source" ]; then
    echo "install: using the prebuilt dictionary at ${dict_source}"
    mkdir -p data/compiled
    cp -- "$dict_source" data/compiled/base.dict
fi
```

  **第四步：CI 缓存与离线断言。** 词库构建是 CI 中最慢且最不变的一步，单独缓存；并加一个**离线构建断言**，证明构建期不需要网络（区别于运行期的 `BUDGET-NET-01`）：

```yaml
  dictionary:
    name: dictionary
    runs-on: ubuntu-24.04
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Cache the raw dictionary sources
        uses: actions/cache@v4
        with:
          path: data/raw
          # The digests in data/sources.toml are the cache key: a source whose
          # content changed upstream must be re-fetched, not reused from cache.
          key: dict-raw-${{ hashFiles('data/sources.toml') }}
      - name: Fetch the sources
        run: bash data/fetch.sh
      - name: Compile the dictionary
        run: cargo run --quiet -p xtask -- dictc
      - name: Assert the dictionary is reproducible without the network
        run: |
          rm -f data/compiled/base.dict
          env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY \
            CARGO_NET_OFFLINE=true \
            cargo run --quiet -p xtask -- dictc
      - name: Upload the compiled dictionary
        uses: actions/upload-artifact@v4
        with:
          name: base-dict
          path: data/compiled/base.dict
          retention-days: 30
```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 给 `dictc` 加 `dict/source/missing` 诊断与补救提示；写一条测试断言缺文件时错误信息含该码。
  2. 给 `packaging/install.sh` 加 `--dict`，并在 `usage()` 中登记；确认 `--dry-run` 路径不复制文件。
  3. 加 `dictionary` 作业与缓存；确认缓存命中时 `data/fetch.sh` 的下载被跳过、digest 校验仍执行。
  4. 端到端验证离线路径：在一个干净目录里只用 tarball + `install.sh --dict` 安装，全程断网。

- **验收标准 (DoD)**：
  - [ ] 发布 tarball 内含 `base.dict`，且其 `sha256` 与 `rspinyin-release.json` 一致；
  - [ ] `install.sh --dict <path> --skip-build` 在**无 Rust 工具链、无网络**的环境下完成安装（在容器内验证并记录命令）；
  - [ ] `dictc` 在词源缺失时以 `dict/source/missing` 非零退出；
  - [ ] `dictionary` 作业的缓存命中率与构建耗时被记录到 PR 摘要；
  - [ ] `.gitignore` 的既有规则不被放宽（**不得**通过把 `data/raw/*.tsv` 或 `base.dict` 提交进仓库来解决本问题——那会绕过 `check-dict-sources.sh` 的白名单模型）。
- **验收记录**（2026-09-30）：
  - **交付物**：`xtask/src/dictc/source.rs` 新增 `SOURCE_MISSING` 与 `read_source`/`source_error`（四处裸 `cannot read` 收敛为一条稳定诊断）；`packaging/install.sh` 新增 `--dict PATH`；`data/fetch.sh` 按摘要缓存 + `--force`；`ci.yml` 新增 `dictionary` 作业；`.gitignore` 新增 `/dist`。
  - **验证命令与结果**：`just ci` 退出 0（`cargo fmt --check`、clippy `-D warnings`、nextest 1521 个用例、doctest、9 个审计脚本及其自检、25 条预算阈值全部通过）。
  - **`dictionary` 作业做的事**：取源 → 编译（计时）→ **删档重编并断言与首次逐字节相同 + 全程无网络**（`CARGO_NET_OFFLINE=true`）→ 上传 `base-dict` → 安装器 `--dict` 冒烟。缓存键含 `data/sources.toml` **与两个已提交的派生源**——只按前者会让 restore 覆盖检出后更新过的 `base.tsv`/`polyphone.tsv`，静默用旧词表编译。
  - **已知限制**：
    1. **DoD 2 的整链仍缺口**：`install.sh --dict` 已实现且无工具链时给出 `platform/toolchain/missing` 而非 `command not found`，但脚本最后一步仍是 `cargo run -p xtask -- install`，无 Rust 工具链时不可能完成。建议移交 `BUILD-P0.05.02` 或另开一张 tarball 安装器卡。
    2. `xtask install` 只对 `base.dict` 做体积预算校验、不做容器魔数/CRC 校验；`--dict` 引入「用户给定的任意文件」后建议补一道校验。
    3. `packaging/uninstall.sh` 缺同样的工具链诊断。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143。

---

### 任务 ID：BUILD-P0.03.04 fcitx5 版本依赖与发行版基线对齐

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-09`、`L-02`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 1.5 人天
  - 前置依赖：无
  - 关键路径：**CP: 是**
  - 并行通道：Track B（签名·打包·描述符）
  - 代码落地锚点 (Code Anchor)：`packaging/fcitx5/rspinyin.conf`、`packaging/fcitx5/rspinyin-ui.conf`、`docs/dev/features.md`（0.5.1）、`xtask/src/versions.rs`
  - 当前状态：`[x] 已完成`
- **目标与核心交付物**：
  - 核心改进指标：描述符声明的 fcitx5 下限与 `features.md` 0.5.1 的发行版基线**不再矛盾**；Ubuntu 22.04 用户得到的是"明确的诊断"或"可用的插件"，不是"插件静默不出现"。
  - 目标产物格式：修正后的 addon 描述符；或一条登记在 0.5.1 的"不支持"结论。
- **工程实现方案与配置文件/脚本全文**：

  矛盾（实测）：`packaging/fcitx5/rspinyin.conf:19` 写 `0=core:5.1.7`，而 `docs/dev/features.md:116` 把 Ubuntu 22.04 LTS 列为基线发行版——jammy 的 fcitx5 是 **5.0.x**。Fcitx5 把 `[Addon/Dependencies]` 的每一项都当作**必需**依赖，不满足时**整个 addon 不加载**（这一行为已在本仓库被实测记录：`rspinyin.conf` 关于 `xcb`/`wayland` 的注释写明"disabling either one suppresses the addon entirely"）。因此 jammy 用户会看到插件"什么都没发生"。

  三条候选路径，**必须选一条并落文档**：

  | 路径 | 描述符改动 | `features.md` 改动 | 成本 | 适用条件 |
  |---|---|---|---|---|
  | (a) 降低门槛至基线 | `0=core:5.1.0` | 无 | 0.5 人天 | 只要插件在 5.1.0 上确实可用 |
  | (b) 移出 jammy | 保持 `0=core:5.1.7` | 0.5.1 的发行版行去掉 22.04，并登记降级策略 | 0.5 人天 | 若 5.1.x 的 API 是硬要求 |
  | (c) 兼容构建 | `build.rs` 按 `pkg_config` 的 `version` 做编译期分支（`R-06` 的方案），产出两个描述符 | 0.5.1 保留 22.04，能力矩阵加一列 | 5+ 人天 | 只在 jammy 用户占比确实高时 |

  **推荐 (a)**：`features.md:120` 已把运行时基线写为"5.1.x"，`R-06` 又把 5.0.x 的支持列为"评估后决定"。把门槛设成精确的 `5.1.7` 是**把开发机的版本当成了最低要求**，这不是一个决策，是一个意外。

  `packaging/fcitx5/rspinyin.conf` 与 `rspinyin-ui.conf` 的依赖段统一为：

```ini
# The floor is 5.1.0, the first release of the minor series the specification
# names as the baseline -- not the version the development machine happens to
# run. Every entry here is a *required* dependency: Fcitx5 suppresses the whole
# addon when one is unmet, so an over-tight floor silently removes the plugin
# rather than reporting anything.
#
# 5.0.x (Ubuntu 22.04 LTS) is deliberately below the floor. R-06 tracks whether
# a compatible build is worth producing; until that decision lands, the
# specification's distribution baseline must not claim 22.04 support.
[Addon/Dependencies]
0=core:5.1.0
```

  同步修正 `docs/dev/features.md` 0.5.1 的发行版行（若选 (a)）：

```
| 发行版 | Ubuntu 24.04 LTS、Fedora 40+、Arch Linux。**Ubuntu 22.04 LTS 不支持** | 22.04 的 fcitx5 为 5.0.x，低于 `core:5.1.0` 的门槛。`[Addon/Dependencies]` 的每一项都是**必需**依赖，故 Fcitx5 **静默不加载该 addon**（用户只看到「输入法不在列表里」）；没有任何诊断码可报——`platform/start/unsupported-os` 是另一回事（它是本插件自己的 OS 判定），加载期根本不会走到。低于基线不做部分降级，也不产出兼容构建；其余档位见 0.5.2 |
```

  并在 `xtask/src/versions.rs` 追加一条断言：**所有** `packaging/fcitx5/*.conf` 的 `Version` 与 `[Addon/Dependencies]` 的 core 门槛必须一致，防止两个描述符再次漂移。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在真实的 5.1.0 / 5.1.7 上验证插件可用性（5.1.7 本机可验证；5.1.0 需容器或降级包），据此在 (a)/(b)/(c) 中定案。
  2. 改描述符与 `features.md` 0.5.1，保持"文档与代码一致"（`AGENTS.md` §7）。
  3. 扩展 `xtask check-versions` 覆盖全部描述符与 core 门槛，并在 `ci.yml` 中运行（`BUILD-P0.05.01` 已把 `check-versions` 纳入 `just ci`）。

- **验收标准 (DoD)**：
  - [ ] 两个 addon 描述符的 `core:` 门槛一致且与 `features.md` 0.5.1 的发行版基线不矛盾；
  - [ ] `xtask check-versions` 覆盖全部 `packaging/fcitx5/*.conf` 的 `Version` 与 core 门槛；
  - [ ] Ubuntu 22.04 的行为（可用 / 明确诊断 / 不支持）在 0.5.1 与 0.5.2 中有唯一表述；
  - [ ] 若选择 (a)，需记录 5.1.0 上的实测结论；无法实测时显式标注"未在 5.1.0 上验证"。
- **验收记录**（2026-09-30）：
  - **交付物**：两个 addon 描述符的 `core:` 门槛统一为 `5.1.0`，并写明理由与 `NOT VERIFIED ON 5.1.0` 标注。
  - **验证命令与结果**：`just ci` 退出 0（`cargo fmt --check`、clippy `-D warnings`、nextest 1521 个用例、doctest、9 个审计脚本及其自检、25 条预算阈值全部通过）。
  - **`xtask check-versions` 已覆盖**：遍历 `packaging/fcitx5/*.conf` 中的 addon 描述符，断言 `Version` == workspace 版本、依赖项 `0` 以 `core:` 开头、且所有描述符门槛相等（实测输出 `2 addon descriptors match the workspace version 0.1.0 and the fcitx5 floor core:5.1.7`）。
  - **本次由主 Agent 修正的三处文档**（`docs/dev/features.md` 0.5.1 与 `opt-deploy.md` 的同一行）：
    1. 发行版行原写「Ubuntu 22.04 LTS / 24.04 LTS」并称加载自检会报 `platform/start/unsupported-fcitx5`——**该码不存在**（既不在 2.2.4 也不在代码里）。22.04 的 fcitx5 是 5.0.x，低于 `core:5.1.0`，而 `[Addon/Dependencies]` 的每一项都是**必需**依赖，所以 Fcitx5 是**静默不加载**，加载期根本走不到任何自检。已按事实改写，并明确 22.04 不支持。
    2. Fcitx5 行称运行时用 `fcitx::Instance::version()` 校验——**代码里没有这个调用**。实际校验的是本插件自己的 C ABI 版本（`ime_types::version::check_abi`）；fcitx5 自身的版本门槛由描述符的 `core:` 依赖表达、由宿主在加载期判定。已按代码改写。
  - **已知限制**：5.1.0 上的真实加载未实测（本机只有 5.1.7，需容器或降级包）。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143。

---

### 任务 ID：BUILD-P0.05.01 CI 收敛到 `just ci` 全量门禁

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-16`、`BUILD-DEF-21`
  - 优先级与复杂度：`P0` | 中 | 预估工时: 1.5 人天
  - 前置依赖：`BUILD-P0.01.01`
  - 关键路径：**CP: 是**
  - 并行通道：Track C（CI/CD·发布）
  - 代码落地锚点 (Code Anchor)：`.github/workflows/ci.yml`、`justfile`
  - 当前状态：`[x] 已完成`
- **目标与核心交付物**：
  - 核心改进指标：CI 执行的命令集与 `just ci` **逐条一致**；`check-licenses` 与 `check-versions` 进入门禁；新增 `check-advisories`。
  - 目标产物格式：无产物；交付物是一条唯一的门禁命令。
- **工程实现方案与配置文件/脚本全文**：

  现状（实测）：`.github/workflows/ci.yml:82` 执行 `just check-deps check-unsafe check-net check-slint check-dict`，而 `justfile:138` 的 `just ci` 是 `check check-deps check-unsafe check-net check-slint check-dict check-licenses check-budget check-versions`。差集：CI 漏了 `check-licenses` 与 `check-versions`，`just ci` 漏了 `check-self-tests`。`AGENTS.md` §2 把 `just ci` 定义为"完成"的唯一事实来源——那么 CI 就该跑它，一个字都不差。

  `justfile` 的 `ci` 配方在 `BUILD-P0.01.01` 中已更新为含 `check-advisories` 与 `check-self-tests`。本卡的 `ci.yml` 全文：

```yaml

- **验收记录**（2026-09-30）：
  - **交付物**：`justfile`（`check-host` 改为「先探测再决定」、新增 `audits` 配方、`ci: check audits check-host`、`ci-host` 改为强制要求）、`.github/workflows/ci.yml`（`audit` 作业删掉手写配方清单这一第二份定义，只保留 `just ci` 无法包含的 `check-advisories`；`size` 作业改调 `just check-size`）。
  - **验证命令与结果**：`just ci` 退出 0（fmt、clippy `-D warnings`、nextest 1992 个用例、doctest、10 个审计脚本及其自检全部通过）。**管道退出码**：逐条核对了两个文件的每个 `run:` 与配方体——GitHub Actions 的 `run:` 默认是 `bash -e -o pipefail`，新增配方体首行均为 `set -euo pipefail`，全文件无 `| head` / `| tail`。
  - **宿主 ABI 门禁**：`check-host` 用 `pkg-config --exists Fcitx5Core Fcitx5Utils Fcitx5Config`（与 `build.rs` 的 `REQUIRED_LIBS` 逐项对齐）在调 cargo **之前**探测；缺失时打印带 `platform/fcitx5/dev-missing` 码的 skip 报告并 exit 0，`RSPINYIN_REQUIRE_HOST_ABI=1` 把 skip 变成失败。跳过是**显式标记**的，不是静默的。
  - **已知限制**：
    1. **`AGENTS.md` §2 的 host-ABI 段落与 `features.md` 0.3 的门禁代码块尚未回写**——前者「修改需用户确认」，后者仍写 `--all-features`（该开关会打开两个 addon crate 的 `fcitx5-host` 并让 `build.rs` panic）。两处待用户确认后同步。
    2. `check-advisories`（`cargo audit` + `cargo deny`）**刻意不在** `just ci`：`cargo audit` 需联网，放进套件会让无网机器整体失败。
    3. `reproducible` / `dictionary` / `size` 三个发布作业的构建与断言步骤仍是内联 shell：它们是「构建并校验产物」而非「审计代码树」，且都需要 release 构建。
    4. DoD 第 3 条（人为注入版本漂移与许可证缺失，两者都被 CI 拦住）需要推 PR 才能验证，未执行。
  - **环境**：Rust 1.98.0、Linux 6.18.40.1-microsoft-standard-WSL2、cargo-nextest 0.9.143。
# rspinyin CI baseline.
#
# Every job that asserts something about the tree runs a `just` recipe rather
# than an inline cargo invocation. The recipes are the single source of truth for
# what "done" means (AGENTS.md section 2), so a gate that CI skips is a gate that
# does not exist -- which is exactly how `check-licenses` and `check-versions`
# came to be missing from the audit job before this was fixed.
#
#   quality     `just ci` -- the full gate suite, on a machine without the Fcitx5
#               development packages, so the pure-Rust path is proven
#   host-abi    the same gates with the real libfcitx5core linked in
#   cross-arch  aarch64 build, factory-symbol assertion and test run
#   dictionary  the offline dictionary build
#   size        the artifact size budgets
#   reproducible two builds of one commit must agree byte for byte
#   bench       budget regression; only on main and on PRs labelled `bench`
#
# `just`, `cargo-nextest`, `cargo-audit` and `cargo-deny` come from prebuilt
# binaries so the quality job stays inside its cold-cache budget.

name: ci

on:
  push:
    branches: [main]
  pull_request:

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1
  SOURCE_DATE_EPOCH: 1790899200
  CXXFLAGS: "-ffile-prefix-map=${{ github.workspace }}=. -fdebug-prefix-map=${{ github.workspace }}=."

jobs:
  quality:
    name: quality
    runs-on: ubuntu-24.04
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@v4
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Install just, cargo-nextest, cargo-audit and cargo-deny
        uses: taiki-e/install-action@v2
        with:
          tool: just,cargo-nextest,cargo-audit,cargo-deny
      - name: Install cargo-public-api for the Slint-leak audit
        uses: taiki-e/install-action@v2
        with:
          tool: cargo-public-api
      - name: Cache the cargo build
        uses: Swatinem/rust-cache@v2
      - name: Run the full gate suite
        run: just ci

  host-abi:
    name: host-abi
    runs-on: ubuntu-24.04
    timeout-minutes: 25
    steps:
      - uses: actions/checkout@v4
      - name: Install the Fcitx5 development packages
        run: |
          sudo apt-get update
          sudo apt-get install --yes --no-install-recommends \
            libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Install just and cargo-nextest
        uses: taiki-e/install-action@v2
        with:
          tool: just,cargo-nextest
      - name: Cache the cargo build
        uses: Swatinem/rust-cache@v2
      - name: Run the gates with the real C ABI linked
        run: just check-host

  bench:
    name: bench
    runs-on: ubuntu-24.04
    timeout-minutes: 30
    if: github.ref == 'refs/heads/main' || contains(github.event.pull_request.labels.*.name, 'bench')
    steps:
      - uses: actions/checkout@v4
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Install just
        uses: taiki-e/install-action@v2
        with:
          tool: just
      - name: Cache the cargo build
        uses: Swatinem/rust-cache@v2
      - name: Run the benchmarks
        run: just bench-quick
```

  > `cross-arch`、`dictionary`、`size`、`reproducible` 四个作业分别由 `BUILD-P0.01.02`、`BUILD-P0.03.03`、`BUILD-P0.02.02`、`BUILD-P0.01.03` 交付；本卡负责把它们与上面三个作业合并到同一个 `ci.yml`，并确保 `just ci` 是 `quality` 作业的**唯一**入口。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 合并 `BUILD-P0.01.01` 的 `justfile` 与 `BUILD-P0.01.02` / `P0.01.03` / `P0.02.02` / `P0.03.03` 的作业片段，产出一份完整的 `ci.yml`。
  2. 在 PR 上验证：故意破坏一处 `packaging/fcitx5/rspinyin.conf` 的 `Version`，确认 `check-versions` 让 `quality` 失败——**这是本卡的核心自证**。
  3. 把 `AGENTS.md` §2 的命令清单更新为 `just ci` 的等价形式（含 `--all-features` 的例外说明），消除文档与实现的漂移。

- **验收标准 (DoD)**：
  - [ ] `quality` 作业只调用 `just ci`，不内联任何 cargo 命令；
  - [ ] `check-licenses`、`check-versions`、`check-advisories`、`check-self-tests` 在 CI 中全部执行（用作业日志逐条核对）；
  - [ ] 人为注入版本漂移与许可证缺失，两者都被 CI 拦住（记录失败日志作为证据）；
  - [ ] `AGENTS.md` §2 的命令清单与 `just ci` 一致。

---

### 任务 ID：BUILD-P0.05.02 安装/卸载可逆性与产物装载验证

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-18`、`L-01`、`L-08`、矩阵 B · X11
  - 优先级与复杂度：`P0` | 中 | 预估工时: 2.0 人天
  - 前置依赖：`BUILD-P0.02.01`、`BUILD-P0.03.03`
  - 关键路径：CP: 否
  - 并行通道：Track C（CI/CD·发布）
  - 代码落地锚点 (Code Anchor)：`.github/workflows/ci.yml`、`xtask/src/install/tests.rs`、`packaging/install.sh`、`packaging/uninstall.sh`
  - 当前状态：`[x] 已完成`
- **目标与核心交付物**：
  - 核心改进指标：`install` → 断言 → `uninstall` → 断言"文件树与安装前逐字节相同"的闭环在 CI 中自动执行；`librspinyin.so` 与 `librspinyin-ui.so` 的工厂符号在**安装到目标目录之后**被复检。
  - 目标产物格式：无产物；交付物是一个可重复执行的安装验证作业。
- **工程实现方案与配置文件/脚本全文**：

  已有资产（不要重写）：`xtask install` 写 `install-manifest.json` 记录它创建了什么、替换了什么；`uninstall` 读该 manifest 回滚。`packaging/install.sh` / `uninstall.sh` 都支持 `--destdir` / `--prefix` / `--no-sudo` / `--dry-run`。**这套设计已经是对的**，缺的只是没有人自动跑它。

```yaml
  install:
    name: install
    runs-on: ubuntu-24.04
    timeout-minutes: 25
    steps:
      - uses: actions/checkout@v4
      - name: Install the Fcitx5 development packages
        run: |
          sudo apt-get update
          sudo apt-get install --yes --no-install-recommends \
            libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Cache the cargo build
        uses: Swatinem/rust-cache@v2
      - name: Fetch the dictionary sources
        run: bash data/fetch.sh
      - name: Snapshot the staging prefix
        run: |
          mkdir -p /tmp/stage/usr
          find /tmp/stage -printf '%P\t%s\n' | sort > /tmp/stage.before
      - name: Install into the staging prefix
        run: |
          DESTDIR=/tmp/stage PREFIX=/usr bash packaging/install.sh --no-sudo
      - name: Assert the payload landed
        run: |
          set -eu
          libdir="$(pkg-config --variable=libdir Fcitx5Core)/fcitx5"
          datadir="$(pkg-config --variable=datadir Fcitx5Core)"
          for f in \
            "/tmp/stage${libdir}/librspinyin.so" \
            "/tmp/stage${libdir}/librspinyin-ui.so" \
            "/tmp/stage${datadir}/fcitx5/addon/rspinyin.conf" \
            "/tmp/stage${datadir}/fcitx5/addon/rspinyin-ui.conf" \
            "/tmp/stage${datadir}/fcitx5/inputmethod/rspinyin.conf" \
            "/tmp/stage${datadir}/rspinyin/base.dict" \
            "/tmp/stage${datadir}/icons/hicolor/scalable/apps/fcitx-rspinyin.svg" \
            "/tmp/stage${datadir}/icons/hicolor/48x48/apps/fcitx-rspinyin.png"
          do
            test -f "$f" || { echo "dist/verify/artifact-missing: $f" >&2; exit 1; }
          done
      - name: Assert both libraries still export the factory symbol after stripping
        run: |
          set -eu
          libdir="$(pkg-config --variable=libdir Fcitx5Core)/fcitx5"
          for so in librspinyin.so librspinyin-ui.so; do
            nm -D --defined-only "/tmp/stage${libdir}/${so}" \
              | grep -q 'fcitx_addon_factory_instance' \
              || { echo "dist/verify/factory-symbol-missing: ${so} after install" >&2; exit 1; }
          done
      - name: Assert the dictionary is within budget
        run: |
          size=$(stat -c%s /tmp/stage/usr/share/rspinyin/base.dict)
          echo "base.dict = ${size} bytes"
          test "$size" -le 20971520 \
            || { echo "dist/verify/size-budget-exceeded: base.dict" >&2; exit 1; }
      - name: Uninstall and assert the tree is restored
        run: |
          set -eu
          DESTDIR=/tmp/stage PREFIX=/usr bash packaging/uninstall.sh --no-sudo
          find /tmp/stage -printf '%P\t%s\n' | sort > /tmp/stage.after
          diff -u /tmp/stage.before /tmp/stage.after \
            || { echo "dist/verify/uninstall-not-reversible: the tree differs after uninstall" >&2; exit 1; }
```

  另加一条**真实会话验证**（X11 档，本机可验证，`features.md:215` 已确认本机 fcitx5 5.1.7 可加载 addon）：

```yaml
      - name: Load the addon in a real fcitx5 session
        run: |
          sudo apt-get install --yes --no-install-recommends fcitx5 xvfb
          DESTDIR=/tmp/stage PREFIX=/usr bash packaging/install.sh --no-sudo --skip-build
          Xvfb :99 -screen 0 1280x800x24 &
          export DISPLAY=:99
          fcitx5 -d --disable=all --enable=rspinyin 2>&1 | tee /tmp/fcitx5.log &
          sleep 5
          grep -q 'rspinyin: addon loaded' /tmp/fcitx5.log \
            || { echo "dist/verify/addon-not-loaded: fcitx5 did not load the addon" >&2; exit 1; }
```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 加 `install` 作业，先在本地用 `DESTDIR=/tmp/stage PREFIX=/usr` 手工跑通整条链，记录每一步的真实输出。
  2. 把本地跑通的命令原样搬进 CI；`pkg-config` 变量在 runner 上解析出的路径可能与开发机不同，因此断言必须用 `pkg-config` 求值而非硬编码（上文的写法已如此）。
  3. 真实会话验证需要 `Xvfb`；若 `fcitx5 -d` 在无 dbus 会话下不稳定，改用 `dbus-run-session -- fcitx5 -d`，并把该结论记入本卡。

- **验收标准 (DoD)**：
  - [ ] `install` 作业在 CI 中稳定通过（连续 5 次 PR 无 flake）；
  - [ ] 卸载后 `diff` 为空——即安装/卸载**逐字节可逆**；
  - [ ] 两个 `.so` 在**安装到目标目录之后**（即剥离之后）仍导出工厂符号；
  - [ ] 真实 fcitx5 会话中出现 `rspinyin: addon loaded`（本机可验证档）；
  - [ ] Wayland 三档的加载验证标注"本机不可验证"并指向 `BUILD-P2.05.02`（`ASM-11`）。

- **验收记录**（2026-09-30）：
  - **交付物**：`xtask/src/install/reversible.rs`（约 460 行，`#[cfg(test)]` 挂载）+ `reversible/tests.rs`（约 425 行，9 个用例）；`xtask/src/install/uninstall.rs` 的 5 处可见性放宽（无行为变更），使往返验证能驱动**真实卸载**而非重写一份卸载逻辑。
  - **验证命令与结果**：`just ci` 退出 0（fmt、clippy `-D warnings`、nextest 1992 个用例、doctest、10 个审计脚本全部通过）。
  - **三点快照**：`before` → `installed` → `after`。只用两点比较会**空转通过**——「什么都没拷、什么都没删」的往返与「什么都没发生」的树完全相等。`landed()` 强制证明安装确实落了盘，`differences()` 才因此有意义。
  - **主 Agent 修掉的两处**：
    1. **被顶掉文件的权限位原本不会还原**：`prepare_entry` 用 `FILE_MODE`(0644) 把原文件拷成备份，`remove_entry` 又用 `FILE_MODE` 拷回去——发行版装在 `/usr/lib/…/fcitx5/` 下的 `.so` 通常是 0755，卸载后会变成 0644，**内容逐字节可逆而模式不可逆**。现在备份以**被顶掉文件自己的模式**落盘（`manifest::mode_of`），卸载按备份的模式还原；模式取自文件本身而非记进 manifest，schema 因此不必升版。
    2. `test_round_trip_restores_the_destination_tree_byte_for_byte` 的期望集合把**被替换**的 `rspinyin.conf` 当成了新增文件：它在安装前就存在，快照把它报为内容变化而非「出现」，所以它不在 `added_files` 里。
  - **已知限制**：
    1. 卡片正文给的 CI 步骤（`find /tmp/stage -printf '%P\t%s\n'`）**照抄会失败**：它包含目录，而安装会创建 Fcitx5 布局命名的共享目录、卸载刻意保留它们。需要改成 `find … -type f`。该残留已被 `test_round_trip_leaves_only_the_directories_it_had_to_create` **断言成事实**，不会被误修。
    2. `packaging/{install,uninstall}.sh` 的 shell 级往返要求 `pkg-config --exists Fcitx5Core` 与可用 `cargo`，未执行。
    3. Wayland 三档的加载验证按 `ASM-11` 仍属 `BUILD-P2.05.02`。
  - **环境**：Rust 1.98.0、Linux 6.18.40.1-microsoft-standard-WSL2、cargo-nextest 0.9.143。

---

## 6. 续写指令

主文档到此为止。**P1 与 P2 的任务卡已随本次交付一并写出**，无需从零续写；下面的指令用于后续**增量扩展**。

### 6.1 分片索引

| 分片 | 路径 | 承载内容 | 状态 |
|---|---|---|---|
| 主文档 | `./docs/dev/opt-deploy.md` | 假设清单、问题总清单、交付矩阵、追溯表、关键路径汇总、P0 任务卡 | 本次已交付 |
| Phase 2 分片 | `./docs/dev/opt-deploy/phase-2.md` | P1 任务卡（`BUILD-P1.*`，9 张） | 本次已交付 |
| Phase 3 分片 | `./docs/dev/opt-deploy/phase-3.md` | P2 任务卡（`BUILD-P2.*`，4 张） | 本次已交付 |

### 6.2 续写输入

- **主文档**：`./docs/dev/opt-deploy.md`（本文件）
- **目标分片路径**：`./docs/dev/opt-deploy/phase-2.md` 或 `./docs/dev/opt-deploy/phase-3.md`
- **追加内容**：新的 `BUILD-*` 任务卡

### 6.3 续写模板

严格沿用第 5 节的任务卡字段（基本属性 / 目标与核心交付物 / 工程实现方案与配置文件脚本全文 / 逐步落地实施步骤 / 验收标准）。**新增卡必须先在第 2 节的问题清单中登记来源编号，并在第 4 节的追溯表中挂载双向映射**，否则视为无主任务。

### 6.4 验收要求

- 新增问题编号后，第 4 节追溯表的"反向完整性断言"计数必须同步更新；
- 新增任务卡的编号必须满足 `dep < self` 的字典序（第 4.2 节的 DAG 校验）；
- 所有配置片段与 CI 脚本必须完整可直接落盘，**严禁伪代码与占位符**（`todo!()` 只允许出现在"任务卡正文里说明待实现"的语境，不得进入主线代码）。

### 6.5 维护清单

以下四条是**长期约定**，不是可勾选的任务——它们没有"做完"的那一刻，所以不写成复选框：

1. 每次代码演进后回写第 2 节问题清单的状态（已修复的行标记并保留编号，便于追溯）；
2. 交付矩阵第 3.1 节的 `状态` 列随 aarch64 评估结论更新。**当前状态：未回填**——本机是 x86_64，`just check-arm64` 在非 arm64 上拒绝运行，该列要等一台 arm64 机器；
3. 第 4.3 节的 CP 与工时在任务实际完成后回填实测值。**当前状态：未回填**——本轮的工时没有按卡记录，回填需要一次专门的统计；
4. 本文件与 `docs/dev/features.md` 出现冲突时，**技术问题以 `features.md` 为准**并修正本文件（`AGENTS.md` 开篇约定）。
