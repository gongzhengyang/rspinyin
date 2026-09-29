# 隐私说明（privacy.md）

> 本文件是「敏感输入上下文检测与学习抑制」的交付物，也是 [README](../../README.md) 与
> [README.zh.md](../../README.zh.md)「隐私」段所链接的完整说明。
>
> **这是一份文档契约，不是宣传材料。** 本文件描述的每一条机制都必须能在代码中定位到
> 具体文件与行为；当文档与代码不一致时，**以代码为准并修正文档**（`AGENTS.md` 第 7 节）。
> 核对范围：`crates/ime-core/src/privacy.rs`（策略）、
> `crates/ime-fcitx5/src/privacy_impl.rs`（宿主侧接入与黑名单）、
> `crates/ime-diag/src/redact.rs`（脱敏）、
> `crates/ime-diag/src/log.rs` 与 `crates/ime-diag/src/crash/record.rs`（日志与崩溃文件的落地）、
> `crates/ime-dict/src/paths.rs`（路径与权限）、
> `crates/ime-dict/src/user_db.rs`（学习库）、
> `crates/ime-dict/src/recover/quarantine.rs`（损坏隔离命名）。
> 第 6 节如实登记当前**尚未接线**的部分。

---

## 1. 数据流向

```text
按键 ──► fcitx5 主循环（宿主线程）
             │
             ▼
        librspinyin.so（引擎 addon，进程内）
             │
             ├─► 解码：纯函数，无文件、无时钟、无环境变量（ime-core）
             │
             ├─► 用户词频（ime-dict::user_db）
             │     └─► $XDG_DATA_HOME/rspinyin/user.redb   0600（目录 0700）
             │           仅当该上下文允许学习时写入（第 3 节）；
             │           批量落盘：32 个不同词 / 2 秒 / 4096 条上限，三者先到先触发
             │
             └─► UiFrame ──► librspinyin_ui.so（UI addon，UI 线程）
                                  └─► 候选框 surface
                                      Wayland：wl_surface + wl_shm 缓冲
                                      X11：覆盖窗口，帧以 PutImage 上传

诊断 ──► $XDG_DATA_HOME/rspinyin/logs/rspinyin.log   0600（目录 0700）
          滚动兄弟：rspinyin.log.1、rspinyin.log.2 …（默认 8MB 一个、保留 3 个）
崩溃 ──► $XDG_DATA_HOME/rspinyin/crash/<毫秒时间戳>-<线程id>.txt   0600（目录 0700）
网络 ──► 无。
          · 依赖闭包中不存在任何网络 crate：BUDGET-NET-01 = 0，
            由 `scripts/check-no-network.sh` 对 `cargo metadata` 的解析图断言；
          · 运行期不持有 IP socket：由 `scripts/runtime-socket-check.sh` 在真实会话中断言；
          · 没有更新检查、没有遥测、没有崩溃上报——崩溃记录只写在本地。
```

两条与"数据流向"直接相关的实现事实：

- 解码路径（`ime-core`）是**纯函数**：不碰文件系统、不读时钟、不读环境变量、没有全局
  可变状态（`features.md` 0.4 规则 4）。用户词频通过 `trait UserFreqSource` 注入，因此
  解码本身没有任何"能写出去"的出口。
- 引擎与 UI 两个共享库**互不 `dlopen`**、不共享静态状态，数据只经 Fcitx5 自己的
  `InputContext` 流转（ADR-0003）。UI 库拿到的是一次渲染所需的帧，不是按键历史。

## 2. 存储位置与权限

路径由 `crates/ime-dict/src/paths.rs` 从 XDG 基目录解析：`$XDG_CONFIG_HOME`
（缺省 `~/.config`）与 `$XDG_DATA_HOME`（缺省 `~/.local/share`），其下各加一层
`rspinyin/`。**目录一律 `0700`、文件一律 `0600`**（`DIR_MODE` / `FILE_MODE`），模式在
`mkdir(2)` / `open(2)` 的调用里就带上，而不是先按 umask 建成 `0755`/`0644` 再 `chmod`
——中间不存在"另一个账号能读到"的那一个系统调用窗口。

| 文件 | 路径 | 权限 | 内容 | 可删除 | 落地状态 |
|---|---|---|---|---|---|
| `config.toml` | `$XDG_CONFIG_HOME/rspinyin/config.toml` | `0600`（目录 `0700`） | 你的配置 | 是（回到内置默认值） | 已落地 |
| `user.redb` | `$XDG_DATA_HOME/rspinyin/user.redb` | `0600`（目录 `0700`） | **学习到的词与词频**：`user_words` 表存「词 → 上屏次数 + 最近使用时间」，`meta` 表存 schema 版本 | 是（丢失全部学习成果） | 已落地 |
| `user.redb.corrupt.<unix秒>` | `$XDG_DATA_HOME/rspinyin/` | 继承原文件模式（重命名不改模式，原文件本就是 `0600`） | 损坏时隔离的原文件，从不删除 | 是（**建议先人工检查**） | 已落地 |
| `ui_takeover.json` | `$XDG_DATA_HOME/rspinyin/ui_takeover.json` | `0600`（目录 `0700`） | 保存的宿主配置快照（`paths.rs` 的注释：saved host configuration） | 是 | 路径已预留，**当前版本无写入点** |
| `logs/rspinyin.log`（及 `.1`、`.2` …） | `$XDG_DATA_HOME/rspinyin/logs/` | `0600`（目录 `0700`） | 诊断日志（脱敏规则见第 4 节） | 是 | 已落地 |
| `crash/<毫秒时间戳>-<线程id>.txt` | `$XDG_DATA_HOME/rspinyin/crash/` | `0600`（目录 `0700`） | 崩溃记录（结构字段的封闭集合，见第 4 节） | 是 | 已落地 |
| `config.toml.v1` | `$XDG_CONFIG_HOME/rspinyin/` | `0600` | 配置迁移前的原件 | 是 | 规划中（配置迁移任务） |
| `phrases.tsv` | `$XDG_CONFIG_HOME/rspinyin/` | `0600` | 用户自定义短语 | 是 | 规划中（自定义短语任务） |
| `backups/user-YYYYMMDD-HHMMSS.tsv` | `$XDG_DATA_HOME/rspinyin/backups/` | `0600`（目录 `0700`） | **用户词条的完整备份**，与 `user.redb` 同一份数据（词 + 词频），按时间戳命名的世代文件；默认保留 3 份，最旧的先删 | 是（丢失回滚能力，不影响 `user.redb`） | 已落地（写入在插件卸载时于工作线程完成；`user.redb` 损坏时启动即从最新可导入的世代回滚） |

权限上的三条边界行为（`paths.rs`，均为代码可查的实际行为）：

- **基目录不动。** `$XDG_CONFIG_HOME` 与 `$XDG_DATA_HOME` 本身属于你所有的应用，插件
  只创建、从不收紧它们的模式。
- **子树内的符号链接一律拒绝。** 布局自己的路径若是指向别处的链接（例如
  `~/.local/share/rspinyin` 被换成链接），插件拒绝沿链接写入，并转入只读模式，报告
  `data/path/symlink`。基目录本身是链接则接受（把家目录放在另一块盘是正常配置）。
- **降级而不是失败。** 目录建不出来、模式收不紧、路径过长——任何一种都让插件进入
  **只读模式**（`data/readonly-mode`）：输入照常工作，学习停止，只读状态由
  `ime_dict::paths::is_readonly_mode()` 以进程级标志上报给上层。它绝不因为写不进去而
  拒绝启动，也绝不"带病继续写"。

## 3. 能力边界：`CapabilityFlag::Password` 是"尽力而为"的信号

> **我们的能力边界**：rspinyin 依赖宿主应用通过 Fcitx5 的 `CapabilityFlag::Password`
> 告诉我们"当前输入框是密码框"。**如果应用没有设置这个标志，我们无法识别它是密码框**，
> 此时在该框中输入的内容会被当作普通输入处理。我们**不会**尝试通过窗口标题、应用名或
> 其他启发式规则猜测密码框——猜测会带来误判，而误判的代价是双向的（误判为密码框会让
> 用户无法学习，误判为非密码框会泄露输入）。
>
> 因此：**不要把 rspinyin 当作密码安全的最后一道防线。** 它是一道有意义的补充，不是
> 保证。

代码落点：`crates/ime-core/src/privacy.rs` 的 `InputContextKind::password` 字段文档逐字
写明"best-effort signal"，`InputContextKind::unreported` 与
`impl Default for InputContextKind` 则把"宿主什么都没报告"这一情况**失败关闭**：它不是
普通上下文，而是被当作敏感上下文处理（`forbids_learning()` 为真）。理由是同一个：未知
可能是读不到标志的密码框。

同一判定链上的其余输入：

- `CapabilityFlag::Sensitive`：宿主为"比密码更宽"的场景提供的第二个位，同样抑制学习。
- 应用黑名单：在 `crates/ime-fcitx5/src/privacy_impl/blacklist.rs` 中以**大小写不敏感的
  子串**匹配应用标识（`keepassxc`、`keepassxc-bin`、`org.keepassxc.KeePassXC` 是同一个
  程序）；命中即抑制学习。**默认黑名单为空**——我们不假设你用哪个密码管理器。
  应用标识在进入 `ime-core` 之前被哈希为 `AppIdHash`，明文不跨越 crate 边界。
- 敏感上下文**仍然显示候选框**（`should_show_ui` 默认 `true`）：你必须能看见自己正在
  打什么。隐藏候选框并不比"不学习"多保护任何东西。企业环境若要求完全透传，策略结构里
  已有 `disable_ui_on_password` 开关；**该开关的配置段目前尚未落地**（第 6 节）。

抑制的语义是**只针对学习，不针对打字**：敏感上下文中照样组字、照样排序、照样上屏，
上屏的文本与其他任何地方逐字节相同；唯一不同的是这些按键永远不进入 `user.redb`。

## 4. 日志与崩溃记录的脱敏机制

### 4.1 第一道防线：不传

`raw`、`text`、`preedit`、`input`、`candidate_text`、`word`、`commit_text` **从不**被
传给任何日志事件（`AGENTS.md` 3.4）。诊断用替代物是**结构事实**：候选个数、切分边数、
来源分布、耗时、错误码。字段名与内容无关时才有意义，因此 `raw_len`、`raw_bytes` 这类
"关于输入的长度"仍然可读，而 `raw` 本身不可读（`redact.rs` 的字段名按**精确匹配**而非
子串匹配）。

### 4.2 第二道防线：`RedactLayer`

`crates/ime-diag/src/redact.rs` 的 `RedactLayer` 与 `RedactFormat` 是同一策略的两半，
共享一个 `RedactState`。它是"万一有人误传"的兜底，**不是**可以放心传内容的许可证。

| 情况 | 写入日志的内容 |
|---|---|
| 字段名在拒绝清单内（`raw`、`text`、`preedit`、`input`、`candidate_text`、`word`、`commit_text`） | `<redacted:len=N>`，`N` 是被扣下值的**字符**数（不是字节数） |
| 其他字段 | 值本身；开头匹配家目录前缀的部分改写为 `~` |
| 敏感会话的任何事件 | 只有 `session=redacted` 与 `app=<哈希>`；**消息、字段、连输入长度都不写** |

- **敏感会话如何被标记**：某条事件同时带 `session = <id>` 与 `password = true`（或
  `sensitive = true`）时，该会话被永久标记；此后该会话的所有事件都降级。宿主在激活输入
  上下文时就知道标志，可以直接调 `DiagHandle::mark_sensitive_session`，这条路不依赖某条
  事件是否被级别过滤掉。
- **为什么连长度也不写**：密码的长度本身就是秘密。
- **应用标识只以哈希出现**：`AppIdHash` 渲染为 `0x` 加 16 位十六进制，绝不明文；哈希的
  用途是让诊断能按应用分组，而不是把应用名藏进日志。
- **换行被转义**：值里的 `\n`/`\r` 写为 `\\n`/`\\r`，一个字段无法伪造出第二条日志行。
- **`diagnostics.log_input_content` 不会打开内容**：该开关只把级别下限抬到 `debug`
  （`log.rs` 的 `effective_level`），它**不能**让被拒字段重新出现——`redact.rs` 的
  拒绝清单在 `debug` 下与在 `info` 下完全一致（该行为有测试断言）。打开它只意味着多写
  一些结构字段。
- **落盘位置与权限**：`logs/rspinyin.log`，`0600`，目录 `0700`。目录不可写时降级为
  `stderr` 并把级别收到 `warn` 以上（与宿主其他 addon 共用一条流，不该被逐键细节淹没）。

### 4.3 崩溃记录

`crates/ime-diag/src/crash/record.rs`：记录只接受**封闭的、受检的结构键集合**
（`CrashContextKey`），其中**没有**输入缓冲、preedit、候选或上屏文本的位置——不是"我们
不写"，而是"没有可写的字段"。两处自由文本是 panic 消息与回溯：消息按项目规则是静态模板
（绝不携带值），回溯只有符号名与地址，两者都做长度截断与控制字符清洗，记录本身上限
64KB。文件 `0600`、目录 `0700`，模式同样在 `open(2)` 里带上。

### 4.4 零痕迹的验收口径

上述机制由该任务的零痕迹断言验收：在密码框中输入并上屏 20 个拼音串后，
`user.redb` 记录数增量必须为 0，日志文件中不得出现这 20 个串的任何长度 ≥ 3 的子串，
人为触发崩溃后崩溃文件中同样不得出现。

## 5. 导出与备份的隐私含义

> `user.redb` 与 `backups/*.tsv` 包含你的**全部学习词条**——即你打过的词与拼音键。
> 导出或分享这些文件等同于分享你的输入历史。它们默认 `0600` 且只存在于你的用户目录，
> **rspinyin 从不把它们发送到任何地方**。

- 这两个文件是**普通文件**：你可以随时删除、复制或备份，不需要经过本程序。
- 反过来，一旦你把它们放到同步目录、网盘或聊天窗口里，泄露就不再由本程序控制——
  这是你在导出时作出的选择，而不是本程序的默认行为。
- `logs/` 与 `crash/` 同样只写在本地，且按第 4 节脱敏；把它们附在 issue 里之前，
  仍建议先自己看一遍。
- 备份与导出功能落地后（规划中），导出的文件不会获得比原文件更宽的权限。

## 6. 实现状态与已知缺口（如实登记）

**已落地并有测试**：策略与判定链（`ime-core/src/privacy.rs`）、宿主侧接入与黑名单
（`ime-fcitx5/src/privacy_impl.rs`）、脱敏层与格式化器（`ime-diag/src/redact.rs`）、
日志落盘与权限（`ime-diag/src/log.rs`）、崩溃记录的结构约束（`ime-diag/src/crash/`）、
XDG 布局与权限（`ime-dict/src/paths.rs`）、学习库与隔离命名
（`ime-dict/src/user_db.rs`、`ime-dict/src/recover/quarantine.rs`）。

**尚未接线**（本节存在的意义就是不让读者高估现状）：

1. **C ABI 还没有把能力标志传进 Rust。** `privacy_impl.rs` 的模块文档明确写着：vtable
   目前只携带输入上下文的 id，能力标志与程序名尚未随之上行；因此当前唯一可能的报告是
   `ContextReport::Unreported`（连该调用点本身也尚未出现），而**未报告的上下文是失败
   关闭的**——插件不学习，而不是冒险从一个可能是密码框的上下文学习。
2. **`apply_effects` 尚不存在**，因此 `LearningGate::record_commit` 目前没有调用点
   （`crates/ime-fcitx5/src/engine.rs` 的模块文档同样记载了这一点）。换言之：学习这条
   路现在整体未通，隐私门禁的接线点也已经备好。
3. **`DiagHandle::mark_sensitive_session` 目前没有调用点**；会话级降级仍可由带
   `password = true` 的事件触发，但"宿主主动标记"这条路要等第 1 条接线完成。
4. **`[privacy]` 配置段尚未进入 `ime-config` 的 schema**：`DefaultPolicy` 里已有
   `disable_ui_on_password` 字段，但配置文件目前没有对应的段与键。
5. `ui_takeover.json` 的路径已在布局中预留，当前版本没有写入点。

以上五条都是**能力缺口，不是数据泄露**：缺口 1 与 2 使当前版本比设计**更保守**（不学习
而非误学习），缺口 3 使会话降级少一条触发路径，缺口 4 与 5 只影响可配置性与便利性。
一旦发现与本节不符的行为（例如某个字段实际未被脱敏），应当**报告**而不是在文档里掩饰：
报告路径是主 Agent（`AGENTS.md` 6.4）。

## 7. 许可相关的两处落点

- **`OB-3`（不得用于嵌入式系统）**：本项目的 Slint 授权不覆盖嵌入式系统，声明落在
  `docs/dev/licenses.md` 第 6 节，并在两份 README 的「许可」段转述。
- **`OB-6`（按现状提供、无担保）**：Slint 的免责声明转述落在 `docs/dev/licenses.md`
  第 7 节，并在两份 README 的「许可」段转述。

本文件不重复这两节的内容；许可问题以 `docs/dev/licenses.md` 与
[ADR-0000](adr/0000-upstream-decisions.md) 为准。

## 8. 强制这些结论的门禁

| 门禁 | 覆盖 |
|---|---|
| `scripts/check-no-network.sh` | 依赖闭包中不存在网络能力（`BUDGET-NET-01`） |
| `scripts/runtime-socket-check.sh` | 活动会话不持有任何 IP socket |
| `bash scripts/gen-licenses.sh --check`（`just check-licenses`） | 依赖许可与 `OB-1`~`OB-6` 的复核 |
| `cargo nextest run -p ime-diag -p ime-core -p ime-dict` | 脱敏、策略判定链、路径权限与学习库的单元测试 |
