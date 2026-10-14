`librime`、`fcitx5-rime` 与 `rime-ice` 分别处于 Linux 下 Rime 输入法技术栈的**核心算法层**、**系统前端适配层**与**用户配置与数据层**。三者协同构成了目前 Linux 桌面环境下极具竞争力的中文输入体验。

## 架构关系与核心定位

在 Linux 系统中，这三个项目的调用与数据流向遵循“系统交互 -> 引擎计算 -> 数据驱动”的分层架构：

```
[ 应用程序 (Wayland / X11) ]
           ▲
           │ (IM 协议通信 / 候选框 UI 渲染)
           ▼
[ fcitx5 / fcitx5-rime ]  <── 1. 系统前端适配层：捕获按键、管理输入状态、绘制界面
           ▲
           │ (调用 C API 接口)
           ▼
[ librime (及 librime-lua) ] <── 2. 核心算法引擎：拼音切分、语法模型、词频调度、Lua 脚本执行
           ▲
           │ (读取并编译为二进制 Prism/Table)
           ▼
[ rime-ice (雾凇拼音) ]   <── 3. 配置与词库层：提供 YAML 规则、百万级词库、动态 Lua 扩展

```

| 仓库名称 | 所属层级 | 开发语言 | 维护主体 | 核心职责 |
| --- | --- | --- | --- | --- |
| **librime** | 核心算法引擎 | C++ (C++17) | Rime 官方 (式恕堂) | 负责输入法状态机、编码反查、形码/音码转换、词频动态调优及插件扩展机制 |
| **fcitx5-rime** | 前端桥接插件 | C++ | Fcitx 团队 (CSSlayer 等) | 将 `librime` 接入 Fcitx5 输入法框架，处理 Wayland/X11 焦点、快捷键与皮肤渲染 |
| **rime-ice** | 方案与词库集 | YAML / Lua | 社区开发者 (iDvel) | 替代官方老旧词库，提供高准确度现代简体词库、双拼方案、中英混输与 Lua 增强滤镜 |

---

## 各仓库技术与功能详解

### 1. librime：跨平台核心算法引擎

* **仓库地址**：[https://github.com/rime/librime](https://github.com/rime/librime)
* **核心机制**：Rime 跨平台（Windows 小狼毫、macOS 鼠须管、Linux 中州韵）的共有底座。它本身不包含任何 UI 代码或特定系统的输入法框架协议，纯粹以动态库（`librime.so`）的形式向外暴露 C/C++ 接口。
* **核心流水线架构（Pipeline）**：`librime` 将一次击键到上屏的过程拆解为高度解耦的四大组件：
1. **Processor（处理器）**：处理按键事件（如拦截快捷键、空格上屏、符号映射、分词符等）。
2. **Segmentor（分词器）**：将输入的连续 ASCII 字符串切分为逻辑片段（如识别出拼音段落、英文段落或特定前缀引导的特殊模式）。
3. **Translator（翻译器）**：将切分好的编码片段查询底层字典，转化为候选词列表（支持音码 `script_translator`、形码 `table_translator` 及动态脚本翻译）。
4. **Filter（滤镜）**：对生成的候选词列表进行后处理（如简繁转换基于 OpenCC、字符集过滤、候选词重排序、Emoji 映射等）。


* **存储与编译引擎**：当你点击“部署（Deploy）”时，`librime` 会将纯文本的 `.dict.yaml` 词库编译为磁盘上的内存映射（mmap）二进制结构：**Prism（棱镜，负责拼写纠错与模糊音推导）**、**Table（词表，基于 Marsha/Trie 树的高效前缀索引）**，以及管理个人输入习惯的 **UserDB（基于 LevelDB 实现的动态用户词频库）**。
* **插件生态**：支持在编译期或运行期挂载外部插件，其中最关键的是 `librime-lua`（允许使用 Lua 脚本编写自定义的 Processor/Translator/Filter）和 `librime-octagram`（引入 N-gram 局部语言模型，防止长句输入时出现生硬拼凑）。

### 2. fcitx5-rime：Linux 现代桌面桥接层

* **仓库地址**：[https://github.com/fcitx/fcitx5-rime](https://github.com/fcitx/fcitx5-rime)
* **核心定位**：Fcitx5 输入法框架的官方 Rime 包装器。相比于早期的 `ibus-rime` 或旧版 `fcitx-rime`，`fcitx5-rime` 在 Wayland 合成器（如 KDE KWin、Sway、Hyprland、GNOME）下的协议兼容性和光标跟随表现最为稳定。
* **主要职责**：
* **生命周期与会话管理**：为每个应用窗口维护独立的 `RimeSessionId`，确保在不同窗口间切换时，中英文状态和未上屏编码互不干扰。
* **UI 与配置解耦**：Rime 引擎本身支持部分外观参数配置，但在 Linux 下，候选词窗口的字体、配色、圆角、横向/纵向排布完全交由 Fcitx5 的主题系统（如 `fcitx5-material-color` 或经典的 ClassicUI）接管，从而与 Linux 桌面环境保持高度视觉统一。
* **D-Bus 与状态通信**：支持通过命令行或桌面状态栏小组件触发 Rime 的“重新部署（Deploy）”与“同步用户数据（Sync）”。


* **默认数据目录**：`fcitx5-rime` 遵循 XDG 规范，其用户配置路径为 `~/.local/share/fcitx5/rime/`（注意与 `ibus-rime` 的 `~/.config/ibus/rime/` 不同）。所有自定义的词库和方案文件均需放置在此目录下。

### 3. rime-ice（雾凇拼音）：现代开箱即用配置集

* **仓库地址**：[https://github.com/iDvel/rime-ice](https://github.com/iDvel/rime-ice)
* **诞生背景**：`librime` 官方自带的默认词库（如 `luna_pinyin` 明月拼音）年代久远，词频分布严重滞后于现代互联网语境，且默认为繁体字优先（通过 OpenCC 转简），导致许多现代词汇、流行语无法打出或排序极低。`rime-ice` 彻底重构了简体中文的词库与配置体系。
* **词库构成**：
* **基础字表**：基于《通用规范汉字表》（8105 字），修正了大量多音字与罕见字读音。
* **核心词库**：融合了华宇野风、清华开源词库、腾讯词向量等多个高质量语料库，经过严格的词频清洗与重新归一化，极大提升了长句整句输入的准确率。
* **英文与混输词库**：内置常见英文单词与 IT 专有名词词库（如 `Linux`、`GitHub`、`Docker`、`Rust` 等），支持无感的中英混合输入，无需频繁按 Shift 切换。


* **内置方案支持**：
* **全拼**：主方案 `rime_ice`，支持模糊音、简拼、拼写纠错（如将 `zhegn` 自动纠正为 `zheng`）。
* **双拼**：原生内置小鹤双拼、自然码、微软双拼、搜狗双拼、紫光双拼、智能 ABC 等全套配置，修改一行代码即可切换。


* **深度集成的 Lua 扩展功能**：`rime-ice` 深度依赖 `librime-lua` 插件，实现了大量纯 YAML 无法做到的交互增强：
* **日期与时间快捷输入**：输入 `rq`、`sj`、`xq` 直接生成当前日期、时间、星期。
* **Unicode 与数字大写**：特定前缀快速输入金额大写或 Unicode 编码字符。
* **长词优先与置顶词滤镜**：防止简拼时生僻单字挤占常用词组的位置，支持用户自定义固顶词（永远排在第一位的候选词）。
* **错音/错字提示与辅码支持**：支持部件拆字反查（不知读音时通过 `uU` 前缀拼写偏旁查字）以及小鹤音形等辅码筛字。



---

## Linux 环境下的部署与日常维护要点

在 Ubuntu / Debian / Arch 等主流发行版中将三者结合使用时，需要注意以下关键技术细节：

1. **确保安装 `librime-lua` 依赖**：
`rime-ice` 的许多高阶功能（日期、错音提示、长词置顶）依赖 Lua 引擎。
* **Arch Linux**：安装 `fcitx5-rime` 时所依赖的 `librime` 已默认开启 Lua 支持。
* **Ubuntu / Debian**：除了安装 `fcitx5-rime` 外，务必检查是否安装了 `librime-plugin-lua`（部分老版本发行版若未分包或版本过低，可能导致 `rime-ice` 的 Lua 滤镜失效）。


2. **文件部署方式**：
直接将 `rime-ice` 仓库克隆或覆盖到 `~/.local/share/fcitx5/rime/` 目录下，然后运行 `fcitx5-remote -r` 或在状态栏右键点击“重新部署”。首次编译百万级词库会消耗数秒 CPU 时间，并在 `build/` 子目录下生成约数十 MB 的 `.bin` 二进制文件。
3. **无痛升级策略（Custom 补丁机制）**：
如果直接使用 `git pull` 更新 `rime-ice` 词库，直接修改原文件会导致 Git 冲突。Rime 官方设计了 `*.custom.yaml` 补丁机制——例如要修改候选词数量或开启双拼，只需新建 `default.custom.yaml` 或 `rime_ice.custom.yaml` 并写入 `patch:` 节点。这样既能随时拉取 `rime-ice` 上游的最新词库，又能永久保留个人定制配置。
