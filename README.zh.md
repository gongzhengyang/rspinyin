# rspinyin

<!-- OB-1：Slint 归属徽章，由 Slint Royalty-free 2.0 许可（ADR-0000 决策 1）要求。
     输入法既无"关于"对话框也无启动画面，本公开页面上的徽章是该许可留下的唯一路径。
     链接必须保持可达：`scripts/gen-licenses.sh --check` 断言徽章与链接存在，
     `--check-links` 探测 https://slint.dev 的可达性。 -->
[![使用 Slint 构建](https://img.shields.io/badge/built%20with-Slint-4C9AFF)](https://slint.dev)
[![许可证：MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#许可)

离线优先的 Linux 拼音输入法：进程内 Fcitx5 附加组件，候选框完全自绘。

[English](README.md)

## 这是什么

rspinyin 是面向 Linux 桌面的中文拼音输入法。它是一个 Fcitx5 附加组件而不是一个独立
程序：Fcitx5 把它加载进自己的进程，因此没有常驻守护进程、没有 socket，也不需要额外安装
任何客户端。

候选框由 rspinyin 自己绘制，而不是交给 Fcitx5 的 ClassicUI。布局、主题、弹簧动画与光栅
化都在本项目内完成，这也是它在 X11 与 Wayland 上表现一致的原因。

全部功能离线运行。词库是编译后的容器，通过零拷贝 `mmap` 读取；你上屏的词记录在本地的
键值库中；产品内没有任何代码路径能建立网络连接。最后一条是构建期断言出来的承诺，而不是
对当前代码的描述：`scripts/check-no-network.sh` 拒绝依赖闭包中的任何网络 crate，
`scripts/runtime-socket-check.sh` 断言运行中的 Fcitx5 会话持有本插件时不持有任何 IP socket。

## 环境要求

- **Linux** 与 **Fcitx5 5.1.0 或更高版本**。两个 addon 描述符都声明 `core:5.1.0`，
  且两者必须始终一致：UI addon 能加载而引擎不能加载，用户得到的是"半个输入法"。
- **显示服务器：X11 或 Wayland**。两个前端都登记为可选依赖，因此两者皆可，构建期不要求
  任何一个。候选框通过当前存在的 X11 或 Wayland 后端完成定位。
- **从源码构建还需要：**
  - Rust。工具链由 `rust-toolchain.toml` 固定为 1.98.0，最低支持版本为 1.85。
  - C++ 编译器，以及带 Fcitx5 开发包的 `pkg-config`（Debian/Ubuntu：
    `libfcitx5core-dev`），用于 `fcitx5-host` feature——它负责链接真实的 Fcitx5 C ABI。
    不开启该 feature 时，Rust 侧在没有安装 Fcitx5 的机器上依然可以构建与测试。
  - `python3` 3.11 或更高版本，供 `scripts/` 下的审计脚本使用。

## 安装

四种安装方式，各自成节。无论选哪一种，都请在安装**之前**先校验下载内容——
[校验发布产物](#校验发布产物)是每条路径共同的第一步。

1. [从发布压缩包安装](#从发布压缩包安装)——无需 Rust 工具链。
2. [从发行版包安装](#从发行版包安装)——`.deb` / `.rpm`。
3. [从 AUR 安装](#从-aur-安装)——Arch Linux。
4. [从源码安装](#从源码安装)——开发路径。

### 校验发布产物

一份发布集是一个目录，内含产物本身、描述它们的清单 `rspinyin-release.json`、
校验和清单 `SHA256SUMS` 及其分离签名 `SHA256SUMS.asc`。两道相互独立的检查，
先校验签名：

```bash
# 导入签名密钥（随发布集附带；不访问任何 keyserver）。
gpg --import packaging/keys/rspinyin-signing-key.asc
gpg --verify SHA256SUMS.asc SHA256SUMS

# 再让发布集自带的校验器逐个对照清单检查产物。
xtask verify --manifest rspinyin-release.json --artifacts .
```

`xtask verify` 在文件缺失、被改动、未签名或不匹配时以非零码退出，并点名九个稳定
`dist/*` 错误码之一；它不写任何文件，在只读挂载上运行也是安全的。校验失败的产物
不得安装。

### 从发布压缩包安装

压缩包内带已编译的 `base.dict`，因此只需要 C 编译器与 Fcitx5 开发包：

```bash
tar xf rspinyin-<version>-x86_64.tar.gz && cd rspinyin-<version>
bash packaging/install.sh
```

### 从发行版包安装

各包携带相同的载荷，并安装到各发行版自己的 Fcitx5 查找 addon 的位置：

```bash
sudo apt install ./rspinyin_<version>_amd64.deb     # Debian / Ubuntu
sudo dnf install ./rspinyin-<version>-1.x86_64.rpm  # Fedora
```

### 从 AUR 安装

```bash
# 使用 AUR 助手：
paru -S rspinyin
# 或手动：
git clone https://aur.archlinux.org/rspinyin.git && cd rspinyin && makepkg -si
```

### 从源码安装

```bash
# 1. 获取词库源并编译（产物写入 data/compiled/base.dict）。
data/fetch.sh
cargo run -p xtask -- dictc

# 2. 构建并安装。脚本以当前用户构建，只对文件拷贝提权。
bash packaging/install.sh
```

`packaging/install.sh` 通过 `pkg-config` 从目标系统自身的 Fcitx5 安装中解析每一个目标
路径，而不是硬编码：Fcitx5 的 addon 目录在 Debian 与 Ubuntu 上是
`/usr/lib/x86_64-linux-gnu/fcitx5`，在 Fedora 上是 `/usr/lib64/fcitx5`，在 Arch 上是
`/usr/lib/fcitx5`；硬编码路径会把插件装到 Fcitx5 根本不会查找的位置。

常用选项：`--dry-run` 只打印计划不做改动，`--skip-build` 直接安装 `target/` 中已有的
产物，`--dict PATH` 安装别处编译好的 `base.dict` 而不自行编译，`--prefix PATH` 与
`--destdir PATH` 改变安装位置，`--no-sudo` 从不提权。`just install` 等价。

`--dict` 是发行包在没有 Rust 工具链的机器上安装的路径：发行包里带一份编译好的
`base.dict`，而安装器会先读一遍它再复制，而不是信任调用者给的路径。

安装的文件：

| 文件 | 目标位置 |
|---|---|
| `librspinyin.so` | `<libdir>/fcitx5/` —— 引擎 addon |
| `librspinyin_ui.so` | `<libdir>/fcitx5/` —— 候选框 addon |
| `rspinyin.conf`、`rspinyin-ui.conf` | `<datadir>/fcitx5/addon/` |
| `rspinyin.conf`（输入法） | `<datadir>/fcitx5/inputmethod/` |
| `base.dict` | `<datadir>/rspinyin/` |
| `fcitx-rspinyin.svg`、`fcitx-rspinyin.png` | `<datadir>/icons/hicolor/` |

随后重启 Fcitx5，把"Rust Pinyin"加入输入法列表。卸载执行
`bash packaging/uninstall.sh`，它会恢复安装时被替换掉的文件。

## 配置

配置文件为 `$XDG_CONFIG_HOME/rspinyin/config.toml`（默认
`~/.config/rspinyin/config.toml`），以 `0600` 创建在 `0700` 的目录中。插件首次在没有
该文件的情况下启动时会写出全部键并在每个键上方附注释，因此文件本身就是它的文档。

```toml
schema_version = 2

[engine]
punct_mode = "chinese"          # "chinese" 或 "english"
full_width = false
auto_english_on_uppercase = true
passthrough_url = true
max_raw_len = 64                # 1..=64
verify_dict_on_load = "full"    # "full" 或 "header"
abbrev = false                  # 展开首字母缩写，`nh` 可达 `你好`

[ui]
client_preedit = false          # 把编码串显示在应用内而不是候选框中
max_per_row = 5                 # 3..=9
show_annotation = true          # 在候选词旁显示注释
max_width_dp = 720              # 220..=1200
corner_radius_dp = 12           # 8..=20
base_alpha = 217                # 0..=255；217 即 0.85

[ui.animation]
enabled = true
omega0 = 26.0                   # 4.0..=80.0 rad/s
zeta = 0.85                     # 0.3..=2.0
appear_ms = 110                 # 0..=600 ms
disappear_ms = 90               # 0..=600 ms

[theme]
scheme = "auto"                 # "auto"、"light" 或 "dark"
accent = "#4C9AFF"              # #RRGGBB

[keys]
digit_zero = "passthrough"      # "passthrough" 或 "flip"
enter_commit_raw = false        # Enter 上屏原始输入而非高亮候选
flip_keys = ["minus", "equal", "up", "down", "home", "end"]
highlight_keys = ["tab", "shift_tab"]

[scheme]
scheme = "full"                 # "full" 或某个双拼布局
show_hint = true                # 在候选框头部显示当前布局名
keep_full_pinyin = true

[phrases]
enabled = true
file = ""                       # 留空：使用默认位置
max_entries = 5000              # 1..=50000

[data]
durability = "eventual"         # "eventual" 批量写盘，"immediate" 每次立即写盘
backup_enabled = true           # 自动备份学习成果
backup_keep = 3                 # 保留 1..=32 代

[diagnostics]
level = "info"                  # "error" .. "trace"
log_rotation_mb = 8
log_keep_files = 3
log_input_content = false       # 接受但无效果：输入内容绝不会被记录
probes = true
```

每个键都可省略：省略的键保持上表所示的内置默认值。文件在插件启动时读取，并在 Fcitx5
触发配置重载时重新读取——Fcitx5 自身的重载动作一执行，改动即落地。生成文件中每个键的
注释都写明其生效方式：重载即生效；启动时只读一次的设置需待下次重启 Fcitx5；本版本接受
但尚未读取的少数键也会如实标注。若所安装版本的宿主胶水层未接线重载回调，文件便无法被
重新读取：改动要等下次重启 Fcitx5 才生效，日志会以 `lifecycle/pending` 行报告该缺口。
无法读取或无法解析的文件绝不会让输入法停摆：沿用当前生效的配置，把原因写入日志，并且
绝不重置进行中的输入。

## 架构

rspinyin 以**两个共享库**交付，由 Fcitx5 各自独立加载：

```text
librspinyin.so      Category=InputMethod   解码引擎
librspinyin_ui.so   Category=UI            候选框
```

两者只在 Fcitx5 内部相遇。引擎解码一次按键，把 preedit 与候选列表写入宿主的
`InputContext`；UI addon 读取该上下文的 `inputPanel()` 并绘制。两个库互不 `dlopen`，
不共享任何静态状态，彼此之间也没有 IPC——这正是 Fcitx5 自带 ClassicUI 的工作方式。

之所以是两个 addon 而不是一个：Fcitx5 从它以 `Category=UI` 发现的 addon 中选出当前
生效的用户界面，而一个 addon 只能属于一个类别——注册为输入法的 addon 无法同时充当用户
界面。描述符中写的是 `Library=librspinyin` 与 `Library=librspinyin_ui`，Fcitx5 会在该
值后原样拼接 `.so`。

工作区内部是一组小型 crate，依赖单向
（`ime-types ← ime-core ← ime-dict ← ime-config ← ime-ui ← ime-fcitx5`，
`ime-ui-addon` 与 `ime-fcitx5` 同层），由 `scripts/check-deps.sh` 强制：

- `ime-types` 是冻结契约：错误类型、ABI 版本，以及各层相会所用的 trait。
- `ime-core` 是解码器——切分、Viterbi k-best、会话状态——以纯函数形式实现，不碰文件
  系统、时钟与环境。
- `ime-dict` 负责词库格式、`mmap` 零拷贝词表与用户词频库。
- `ime-config` 是 TOML 模型、加载器与由宿主触发的重载。
- `ime-ui` 是视图层：几何、主题、弹簧动画、软件光栅，以及 X11 与 Wayland 后端。
- `ime-fcitx5` 与 `ime-ui-addon` 是两个 C ABI 层，各自对应一个共享库。
- `ime-diag` 是诊断叶子 crate：日志、脱敏、崩溃记录。

两个线程，物理隔离。Fcitx5 主循环只负责把一次按键变成一个已解码的帧并投递出去；UI 线程
持有显示连接、光栅缓冲与渲染器，并通过 `eventfd` 而不是轮询定时器被唤醒。渲染绝不在
宿主线程上执行，宿主的 `InputContext` 也绝不从 UI 线程触碰。

## 隐私

rspinyin 在设计上就离线，留在你磁盘上的东西很少且都有记录。完整说明——数据流向、它写入
的每一个文件及其路径与权限、脱敏规则，以及我们保护能力的边界——见
[`docs/dev/privacy.md`](docs/dev/privacy.md)。

简述：

- **没有网络。** 没有对外连接、没有更新检查、没有遥测。构建期断言依赖闭包中不存在网络
  crate，运行期断言活动会话不持有任何 IP socket。
- **不记录你输入的内容。** 输入文本、preedit、候选文本与上屏文本从第一步起就不会被传给
  任何日志事件；万一被误传，脱敏层会作为第二道防线拦下。
- **学习是本地且有限的。** 上屏一个词会更新 `$XDG_DATA_HOME/rspinyin/` 下 `0600` 的
  本地库。它属于你：删除该文件只会丢掉学习成果，别无其他。
- **密码框是"尽力而为"的识别。** 我们依赖应用主动告诉 Fcitx5 "当前是密码输入框"。没有
  设置该标志的应用，我们无法识别为密码框，因此请不要把 rspinyin 当作密码安全的最后一道
  防线。

## 许可

rspinyin 以 **MIT OR Apache-2.0** 双许可发布，你可任选其一。完整文本见
[`LICENSE-MIT`](LICENSE-MIT) 与 [`LICENSE-APACHE`](LICENSE-APACHE)。

候选框使用 Slint 构建，依据 `LicenseRef-Slint-Royalty-free-2.0` 许可。该许可对本项目
提出的义务逐条登记在 [`docs/dev/licenses.md`](docs/dev/licenses.md) 中；摘要如下：

- **`OB-1` —— 归属展示。** 本页面顶部放置 Slint 归属徽章并链接到
  <https://slint.dev>。输入法既无"关于"对话框也无启动画面，公开页面上的徽章是该许可
  留下的可行路径，且该链接必须保持可达。
- **`OB-2` —— 不得单独分发。** Slint 从不单独分发：它以静态链接进入
  `librspinyin_ui.so`，本项目不构建、不打包任何属于自己分发的 Slint 库。
- **`OB-3` —— 不得用于嵌入式系统。** **本许可不覆盖嵌入式系统。** 把 rspinyin 部署到
  家电显示屏、POS 机与自助终端或车载系统上，必须另行取得 **GPL-3.0** 或 SixtyFPS GmbH
  的**商业许可**。Linux 桌面输入法属于桌面应用，在授权范围之内；上述场景不在。
- **`OB-4` —— 不得暴露 Slint API。** 候选框不是可供第三方编程的 Slint 界面：
  `ime-ui` 的公共 API 中不出现任何 Slint 类型。该约束由
  `scripts/check-slint-leak.sh` 强制。
- **`OB-5` —— 不得移除许可声明。** 不改动、不删除 Slint 的许可声明，也不改动 Slint
  发行包内的许可原文；原文摘要已登记在 `docs/dev/licenses.md`。
- **`OB-6` —— 按现状提供。** Slint 按 **"现状"提供、无任何担保**，无论明示或默示，
  包括但不限于可销售性与特定用途适用性的担保。rspinyin 自身在其两项许可下同样如此。

完整审计见 [`docs/dev/licenses.md`](docs/dev/licenses.md)，决策记录见
[`docs/dev/adr/0000-upstream-decisions.md`](docs/dev/adr/0000-upstream-decisions.md)。

## 致谢

- **[Slint](https://slint.dev)**（SixtyFPS GmbH）—— 候选框的 UI 工具包，依据
  Slint Royalty-free 2.0 许可使用。
- **[Fcitx5](https://github.com/fcitx/fcitx5)**（Fcitx 开发团队）—— 本插件所依附的
  输入法框架，依据 LGPL-2.1-or-later 发布。它以动态链接方式使用且不随本包分发：由你的
  发行版提供。
- **词库来源** —— [`mozillazg/pinyin-data`](https://github.com/mozillazg/pinyin-data)
  （MIT）提供单字读音，Unicode
  [Unihan 数据库](https://www.unicode.org/Public/UCD/latest/ucd/Unihan.zip)（Unicode
  License）用于交叉校验，[`fxsjy/jieba`](https://github.com/fxsjy/jieba)（MIT）提供
  词条与词频。
- 全部 Rust 依赖、其声明许可证与本项目主张的分支见
  [`docs/dev/licenses.md`](docs/dev/licenses.md)；随发布产物提供的第三方声明见
  [`docs/dev/NOTICE`](docs/dev/NOTICE)。
