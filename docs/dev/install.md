# 安装指南

> 文档版本: v1.0 ｜ 系统形态: Desktop GUI（Fcitx5 进程内双 addon）｜ 架构基线: Rust 2024 workspace，双 cdylib（`librspinyin.so` + `librspinyin_ui.so`）｜ 关联文档: [opt-deploy.md](opt-deploy.md)（权威交付矩阵）｜ 维护约定: 本文档的平台矩阵从 opt-deploy.md 3.1 派生；**两处不一致时以交付矩阵为准**，发现漂移请修正本文档。

用户侧快速路径见 [README](../../README.zh.md)。本文档面向需要更多细节的安装者：每条命令的出处、每个平台切片的实测状态、以及安装涉及的全部落盘位置。

## 1. 校验（所有方式的第一步）

安装任何发布产物之前，先校验再安装。两道独立检查：

```bash
# 1) 校验和清单的签名（密钥随发布集附带，不访问 keyserver）
gpg --import packaging/keys/rspinyin-signing-key.asc
gpg --verify SHA256SUMS.asc SHA256SUMS

# 2) 发布集自带校验器：逐文件对照清单（九个稳定 dist/* 错误码，只读不写）
xtask verify --manifest rspinyin-release.json --artifacts .
```

`xtask verify` 的拒绝路径与状态机见 `xtask/src/verify.rs`；任何 `dist/*` 拒绝都意味着产物与清单不一致，此时**不得安装**。

## 2. 四种安装方式

### 2.1 源码 tarball（`L-08`，全架构离线路径）

```bash
tar xf rspinyin-<version>-x86_64.tar.gz && cd rspinyin-<version>
bash packaging/install.sh
```

压缩包内带已编译的 `base.dict`；`install.sh` 以当前用户构建，只对文件拷贝提权（`--no-sudo` 可完全禁用提权，`--destdir`/`--prefix` 可重定位，`--dry-run` 只打印计划）。

### 2.2 Debian / Ubuntu（`L-01` 支持、`L-02` 受阻）

```bash
sudo apt install ./rspinyin_<version>_amd64.deb
sudo apt remove rspinyin   # 移除
```

**`L-02`（Ubuntu 22.04）受阻的实测结论**：jammy 的 fcitx5 是 5.0.x，低于两个 addon 描述符声明的 `core:5.1.0` 门槛（`packaging/fcitx5/*.conf`），宿主会静默不加载。只能改其一：降低依赖门槛，或把 jammy 移出基线（归属 `BUILD-P0.03.04` 的定案）。

### 2.3 Fedora（`L-03` 未实测）

```bash
sudo dnf install ./rspinyin-<version>-1.x86_64.rpm
sudo dnf remove rspinyin   # 移除
```

**已知工具链阻塞**：Fedora 40/41 归档 rustc 为 1.82，低于 workspace MSRV 1.85，`BuildRequires: rust >= 1.85` 在依赖解析阶段即失败，容器里需先安装更新的工具链（详见 opt-deploy.md `L-03` 行）。

### 2.4 Arch Linux / AUR（`L-04` 未实测）

```bash
paru -S rspinyin
# 或手动：
git clone https://aur.archlinux.org/rspinyin.git && cd rspinyin && makepkg -si
sudo pacman -R rspinyin   # 移除
```

向 AUR 的首次提交需经维护者显式确认；`sha256sums` 在首个 tarball 产出前为 `SKIP`（摘要无从计算），首次发布后由 release 流水线回填真实值。

## 3. 平台矩阵（派生自 opt-deploy.md 3.1，截至 2026-10-01）

| 切片 | 发行版 | 架构 | 状态 | 结论出处 |
|---|---|---|---|---|
| `L-01` | Ubuntu 24.04 LTS | x86_64 | 支持 | 包定义与安装可逆性已实测（本机安装面） |
| `L-02` | Ubuntu 22.04 LTS | x86_64 | **受阻**（fcitx5 5.0.x < `core:5.1.0`） | `BUILD-P0.03.04` |
| `L-03` | Fedora 40+ | x86_64 | 未实测（rustc 1.82 < MSRV 1.85） | opt-deploy.md `L-03` |
| `L-04` | Arch Linux | x86_64 | 未实测（tarball 未产出，`sha256sums` 为 `SKIP`） | opt-deploy.md `L-04` |
| `L-05` | Ubuntu 24.04 | aarch64 | 未实测（本机 x86_64，无 arm64 容器） | opt-deploy.md `L-05` |
| `L-06` | Fedora 40+ | aarch64 | 未实测（同 `L-03` + `L-05` 叠加） | opt-deploy.md `L-06` |
| `L-07` | Arch Linux | aarch64 | 未实测（同 `L-04` + `L-05` 叠加） | opt-deploy.md `L-07` |
| `L-08` | 源码 tarball | any | 支持 | 离线分发主路径 |

"未实测"不是"不支持"：定义与 CI 作业已落盘，结论将在对应切片首次真实构建/安装后回写本表与交付矩阵。

## 4. 安装了什么、装到哪里

`packaging/install.sh` 与 `xtask install` 用 `pkg-config` 从目标系统自身的 Fcitx5 安装解析目的地，不硬编码路径。三个发行版的 addon 库目录差异是刻意保持并被测试固定的（`xtask/src/install/layout.rs`）：

| 文件 | 目的地 |
|---|---|
| `librspinyin.so` | `<libdir>/fcitx5/`（Debian: `/usr/lib/x86_64-linux-gnu/fcitx5`，Fedora: `/usr/lib64/fcitx5`，Arch: `/usr/lib/fcitx5`） |
| `librspinyin_ui.so` | 同上（UI addon，第二个 cdylib） |
| `rspinyin.conf`、`rspinyin-ui.conf` | `<datadir>/fcitx5/addon/` |
| `rspinyin-im.conf` | `<datadir>/fcitx5/inputmethod/` |
| `base.dict` | `<datadir>/rspinyin/` |
| `org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml` | `<datadir>/metainfo/` |
| `fcitx-rspinyin.svg` / `.png` | `<datadir>/icons/hicolor/` |

卸载：`bash packaging/uninstall.sh`（或各发行版的包管理器移除命令）按安装清单逆序恢复，不删除共享目录内的他人文件；可逆性由 `xtask/src/install/reversible/` 的 diff 断言测试固定。

## 5. 排障（四类高频问题）

| 现象 | 先查什么 | 处置 |
|---|---|---|
| **插件没被加载** | `fcitx5 -v 2>&1 | grep rspinyin` 是否列出两个 addon；`fcitx5-diagnose` 的 addon 段 | 描述符门槛：两个 `*.conf` 都声明 `core:5.1.0`，宿主 fcitx5 低于它会**静默不加载**（Ubuntu 22.04 即此情形，见 §2.2）；确认 `.so` 确实落在本机 fcitx5 的 addon 目录（§4 的三发行版差异） |
| **候选框定位不对** | 窗口是否在多显示器/缩放环境；`ui/candidate.slint` 的锚点来自宿主 input panel 的光标矩形 | X11 档实测口径见 opt-ui/opt-perf 的验收记录；Wayland 三档（wlr-layer-shell/KWin/Mutter）在开发机不可验证，定位异常请带合成器与版本信息报 issue |
| **没有模糊** | 合成器是否支持/开启了背景模糊 | 面板在无模糊合成器上自动退化为不透明同色，属设计内降级（三层景深参数见 opt-ui.md §2.1），不是缺陷 |
| **卸载残留** | `packaging/uninstall.sh`（或包管理器移除）后比对安装清单 | 安装清单即 §4 的表；`xtask install/uninstall` 的可逆性由 diff 断言测试固定；共享目录按 rmdir 语义处理，不删他人文件 |

## 6. 安装后

1. 重启 Fcitx5（`fcitx5 -r` 或重新登录）。
2. 在 Fcitx5 配置的输入法列表中加入"Rust Pinyin"。
3. 两个 addon 都应被加载：引擎与候选窗；若候选窗未接管，按键仍走 ClassicUI，`ui/not-ready` 诊断会记录降级（见 `features.md` 2.2.4）。

配置文件在 `$XDG_CONFIG_HOME/rspinyin/config.toml`（默认 `~/.config/rspinyin/config.toml`），文件 `0600`、目录 `0700`；目录不可写时插件降级为只读模式而不是失败（`ASM-15`）。
