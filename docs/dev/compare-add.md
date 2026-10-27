# 与对标项目深度对比分析及改进报告 · 第二轮（rspinyin vs librime / WindInput / fcitx5-rime / rime-ice）

> 报告版本: v1.0 ｜ 本项目基线: `/home/gong/news/rspinyin` main@`0ef7d8b`（stage 22 全量落地后，审计日期 2026-10-27）｜ 对标输入源: `remarks/*` 四仓库**源码级**静态精读（librime @`7bc3fb00`、WindInput、fcitx5-rime @`f504861`、rime-ice）｜ 系统形态: **Desktop GUI**（Linux 桌面输入法：进程内 Fcitx5 插件 + 独立 UI addon + 全自绘 Slint 候选窗）｜ 勘察摘要: `./research/digest-*.md`（8 份，锚点抽验 100% 通过）｜ 关联增量规划: [./features-add.md](features-add.md) **第二轮增量（IMP-21~40）** ｜ 上一轮报告: [./compare-and-improve.md](compare-and-improve.md)（IMP-01~20，已全部交付）
>
> 本报告与上一轮的关系：**不重复**、只做三件事——① Round-1 改进项的闭环确认；② 以本轮 8 份勘察摘要为事实基准，覆盖上一轮未触及的深水区（感知性能/防呆自愈/测试基建/展示层体系）与**全新对标 WindInput**；③ 收敛第二轮改进项 `IMP-21`~`IMP-40`。合规基线：WindInput 与 rime-ice 为 **GPL-3.0**，本轮全部产出为机制级语义转写，任何代码/词表/数据表拷贝均被禁止（`ASM-A-11`）。

---

## 1. 输入源勘察与双边工程基线总览

### 1.1 输入源清单与勘察分组

| 仓库 | 层级定位 | 规模 | 语言 | 许可 | 勘察簇（摘要） |
|---|---|---|---|---|---|
| `remarks/librime` | 核心算法引擎 | ~157k 行（src+deps 视角）/ 37k 业务行 | C++17 | BSD 系 | SCOUT-A（架构/性能/规模）、SCOUT-C（防呆/测试/平台） |
| `remarks/WindInput`（清风输入法） | **同栈全平台 IME**（本轮新增对象） | ~409k 行 Rust / 656 文件 / 27 crate + 三平台壳 | **Rust** | **GPL-3.0** | SCOUT-D（引擎/词库/背压）、SCOUT-E（UI/主题/旅程）、SCOUT-F（平台/测试/自愈） |
| `remarks/fcitx5-rime` | Fcitx5 桥接层 | 2,731 行（全量精读） | C++ | GPL 系 | SCOUT-FR（五维全覆盖） |
| `remarks/rime-ice` | 配置/词库/Lua 生态 | 22 lua（2,705 行）+ 21 yaml + 6 分表词库（~45MB） | YAML/Lua/Go | **GPL-3.0** | SCOUT-ICE（功能面/数据工程/测试） |
| 本仓库（基线勘察） | — | 247k 行 Rust（crates+xtask） | Rust | MIT OR Apache-2.0 | Baseline Scout → [digest-self-baseline.md](research/digest-self-baseline.md)（短板 B-01~B-15） |

### 1.2 Round-1 改进项闭环确认（IMP-01~20）

上一轮 20 项改进已全部随 stage 22（`0ef7d8b`）交付；基线勘察对 14 项核心能力逐一核实了真实挂载点（[digest-self-baseline.md](research/digest-self-baseline.md) §2，无幽灵卡）。闭环状态：

| 改进项组 | 落地确认（锚点摘录） |
|---|---|
| IMP-01/02/03（归一化/衰减/BIGRAM） | `xtask/src/dictc/normalize.rs`、`user_db/decay.rs:16-26`、`format/mod.rs:172`（v2 容器 Bigram 段已填充）——全部已完成 |
| IMP-04（置顶词） | `key.rs:71` → `engine/router.rs:311` → `user_db.rs:382`——已完成 |
| IMP-06/07/08（模糊音/简拼/简繁接线） | `fuzzy.rs`、`segment/abbrev.rs`、`script.rs` + 配置键齐备——已完成 |
| IMP-09/10/11（造词/纠错/上文重排） | `user_lexicon.rs`、`segment/correct.rs`（`DecodeFlags::CORRECTION`）、`viterbi/context.rs`——已完成 |
| IMP-12/13/14（会话共享/桥接工艺） | `session_host.rs`、compose 委托、compose/诊断节流/状态区 Action——已完成 |
| IMP-15/16/17/18（词源/英文/错音/符号） | `sources.toml`（thuocl/huayu 已钉哈希）、`segment/english.rs`、错音 annotation、`symbols.rs`——已完成 |
| IMP-19/20（冒烟 TSV/热替换） | `tests/smoke/cases.tsv` 202 例、`hot_swap.rs` ArcSwap——已完成 |
| IMP-05（重码溢出） | 上限策略与审计报告落地（TASK-3.03.01 验收记录）——已完成 |

**结论**：Round-1 的「哑铃形」短板（维 1 排序深度 / 维 4 个性化）已抬升；本轮从基线勘察 B-01~B-15 与四仓新证据出发开启第二轮。

### 1.3 双边工程基线对照表

| 维度 | librime | WindInput | fcitx5-rime | rime-ice | rspinyin（当前） |
|---|---|---|---|---|---|
| 语言/形态 | C++17 引擎库 | Rust 服务进程 + C++/Swift 薄壳 | C++ 桥接插件 | YAML/Lua 配置集 | Rust workspace（9 crate + xtask，双 cdylib） |
| 进程拓扑 | 进程内库（前端驱动） | **外置服务**（UDS+push+SHM） | 进程内插件 | —（数据） | 进程内插件（两 cdylib 无 IPC） |
| UI | 无（headless） | 服务端光栅 + SHM 贴帧 | 全委托 fcitx5 | —（主题归 Squirrel/Weasel） | UI addon 内 Slint 软光栅 + wire 结构化帧（ADR-0011） |
| 词库存储 | marisa + 4 级索引 mmap + LevelDB | 双数组 Trie `.wdat` mmap + 指纹缓存 | — | 6 分表 yaml（~192 万条） | FST+6 段 mmap 容器 v2（348,972 条 / 17.6MiB）+ redb |
| 语言模型 | Grammar 槽 + octagram 插件 | 词条权重 ln 域 + last-word 束搜（无独立 LM） | — | — | Q8.8 五权重 + BIGRAM 段真查（v2） |
| 测试 | gtest 真引擎沙箱（`test/`） | 双向协议解析守卫 + dbus-run-session e2e + 43 行自造骨架 | **零测试**（负向证明） | **三层**（TSV 冒烟 216 行 + lint + CI） | 4,000 `#[test]` + testd 三档 + 202 例冒烟 TSV |
| 许可 | BSD 系 | **GPL-3.0** | GPL 系 | **GPL-3.0** | MIT OR Apache-2.0 + 白名单 CI 审计 |

结构性结论：与 **WindInput 的同构度最高**（同为 Rust、同为 IME、候选窗同为服务端光栅思路），其机制级吸收难度整体最低；librime 提供算法深水区参照；rime-ice 提供**产品功能面清单**与数据工程纪律；fcitx5-rime 提供桥接工艺的标准答案（Round-1 已吸收大半）。

---

## 2. 顶层架构范式与核心机制源码级对比（六维）

| 维度 | 对标最佳实践（锚点） | rspinyin 现状（锚点） | 判定 |
|---|---|---|---|
| 核心定位 | librime 一切可插拔（`component.h:20` Class+Registry）；WindInput 平台能力经 `HostServices` 注入、纯逻辑 crate 零平台依赖（`AGENTS.md` 分层表） | 9 crate 单向 DAG + dictc 编译期物化（`scripts/check-deps.sh` 强制） | 取向一致；WindInput 的「编译门钉死分层」（check-headless + `cargo tree` 断言依赖边）值得作为纪律记录（已有 check-deps，维持） |
| 底层数据模型/存储 | librime `OffsetPtr` int32 自相对偏移 = 位置无关零重定位（`mapped_file.h:21-51`）；WindInput `.wdat` 内容指纹 sidecar 缓存 + single-flight（`cached.rs:296`） | 6 段 mmap + 逐段 CRC + quarantine（`format/mod.rs:162-178`） | **己方领先**（CRC 自愈）；指纹增量重建（SCOUT-A-12/C-05）小价值，不立项（见 §9） |
| 并发与事件调度 | WindInput 推送管道满=断开+重连全量同步（`push.rs:57-70`）；librime 无内部事件队列（负向证明） | 双线程物理隔离 + SPSC 槽位语义契约（Frame latest-wins / Show-Hide 有序 / Select 环 64） | **己方更严格**；「断开→全量对齐」是进程内 SPSC 溢出语义之外的第三种答案，当前溢出语义已契约化且从未触顶，不引入（§9） |
| 渲染/协议管线与状态同步 | WindInput SHM 帧防撕裂（先像素后 header 按 sequence 判新，`shared_render_frame.rs:22-52`）、`UiCommand` 纯数据契约（`wind-ui-types`） | wire 镜像结构化 UiFrame + 双方静态断言尺寸 + 握手（`ffi/transport.rs:16-22`） | 同题异解；己方结构化帧比像素帧可测性更强，维持；offset 钉死测试作为 wire 演进纪律记录（§9） |
| 扩展与插件机制 | librime 字符串键注册表 + dlopen 插件（`dict_module.cc:21-40`）；rime-ice 23 个 lua 滤镜 | dictc 数据管线（编译期）+ KeyAction/DecodeFlags 契约内开关 | **维持 Round-1 拒绝**（插件/lua 不引入）；rime-ice 的拼写运算=「构建期 FST 派生边」与己方 dictc 路线天然同构（SCOUT-ICE-07），本轮用于自定义双拼布局（IMP-25） |
| 可观测性 | WindInput `Engine::memory_parts()` 自报内存（`engine.rs:295-304`）+ startup_trace（`startup_trace.rs:1-30`）；librime `[ComponentTiming]`（`engine.cc:329-342`） | tracing 双层脱敏 + budgets.json 断言 + UiFrame 镜像 | 平手；memory_parts 式「组件自报内存」可作为预算审计增强记录，暂不立项（§9） |

---

## 3. 展示层/交互接口深度对比（Desktop GUI 路由）

> 四个对标中只有 WindInput 有自绘候选窗与主题系统（fcitx5-rime 全委托 classic-ui、librime/rime-ice 无 UI——负向证明见各自摘要）。故本矩阵以 WindInput 为对标基准；**凡属视觉工艺的改进一律路由 `opt-ui.md` 轨道**，本报告只收数据模型/通道/工程约束侧的改进。

| 视觉与展示对比维度 | 本项目现状诊断（锚点） | WindInput 实现标准（锚点） | 代差与痛点 | 改进方向 |
|---|---|---|---|---|
| 1. 色彩体系与材质层次 | 主题单文件扁平 token（`ime-ui/src/theme.rs`，恰 800 行贴顶）；亮暗两套手写 | 主题 v3 TOML + `_base` 单链继承（派生主题 16 行）+ 求值期 light/dark 坍缩单值 + 标准色契约 `fill_missing`（`wind-theme/src/resolve.rs:50-65`） | 模型表达力与维护性代差 | **数据模型部分**（继承/求值坍缩/契约色）登记为 opt-ui 轨道候选；本轮只做行限拆分（IMP-38） |
| 2. 字体排版与信息层级 | 主题内字号/字重由 Slint 模板消费 | Dim 三态 dp/px/%、相对字号偏移（`index.font_size = -2`） | 表达力差距 | opt-ui 轨道（N/A 本轮） |
| 3. 空间布局、网格与密度 | 候选窗横/纵布局 + 徽标/速查面板（`engine/badge.rs`、`cheatsheet.rs`） | 盒模型 View 树 + 三级宽度钳制 + 位置三分支（`view.rs`、`candidate_window.rs:1788-1960`） | 布局能力差距 | opt-ui 轨道；位置持久化三原则（右下锚点/抬起才发/fixed 不落盘，SCOUT-E-13）登记 opt-ui |
| 4. 复杂数据展示与视觉降噪 | 符号域仅静态类别（`symbols.rs:1-5`，v1/vx/vym）；无 emoji | v 前缀 66 符号子域 + 自描述索引页（vhelp/vlist）+ emoji 映射（`symbols_v.yaml:12-13`、opencc/emoji） | **符号系统分类学与索引页缺失（B-07）** | IMP-29（符号域扩展：emoji + 索引页 + 热重载）；分类学语义转写、不拷表 |
| 5. 交互状态、微动效与反馈完备度 | 模式徽标 + 状态条右槽；无候选时窗体行为未契约化 | 五态完备：空态=模式标记回显、用户禁用盲打、宿主压制、主题两级降级、诊断 HUD（`coordinator.rs:6832-6865`）；**零动画是刻意取向**（负向证明） | 空态语义与禁用态呈现差距 | 空态回显/禁用态呈现归 opt-ui 轨道；`GHOST_ALPHA`/工具栏 auto-hide 为产品形态差异不吸收 |

---

## 4. 深水区体验、工程韧性与测试基建七维对比矩阵

| 深水区对比维度 | 本项目现状诊断（锚点+短板） | 对标实现机制（源码路径/负向证明） | 代差分析 | 改进方案 |
|---|---|---|---|---|
| 1. 核心旅程摩擦度与开箱体验 | 高级能力默认全关（`schema/config.rs:36,52,73,136`）；冒烟只锚首选（B-09/B-10） | rime-ice 左右 Shift 独立策略 + 编辑键语义表（`default.yaml:72-79`、`schema.yaml:290-300`）；WindInput 四源联想即打即出 | 开箱「键感」无数据支撑；缺联想这一高频旅程件 | IMP-23（联想）、IMP-26（整定）、编辑键语义扩展记录为 IMP-30/31 附带评估 |
| 2. 乐观更新、流式反馈与取消令牌 | 解码同步有界（decode_p99=24ms 预算断言），无异步面 | WindInput IPC 失败 11 分类 + 读超时永不重发（`Bridge.h:69-115`） | N/A——进程内无 IPC/异步等待面 | **N/A（无需引入，理由：同步解码 + 预算断言已覆盖；无网络无 IPC）** |
| 3. 异常人体工学、防呆与自愈 | quarantine + readonly 降级 + 备份三代领先；**compose 完全依赖宿主**（`engine/host.rs:51-62`，B-12）；配置无退役键机制 | librime 用户库三级自愈 + trash 永不直删（`user_db_recovery_task.cc:23-58`、`deployment_tasks.cc:287-326`）；WindInput RETIRED_KEYS 物理清理 + 值迁移先落盘再清键（`config.rs:142-193`）；rime-ice 删词/隐藏/降频负反馈（`cold_word_drop/`） | 配置演进与宿主缺位兜底是真实缺口；负反馈只有 Forget 一态 | IMP-37（退役键）、IMP-39（compose 兜底）、IMP-31（负反馈三态）、IMP-36（用户词典 CLI） |
| 4. 会话状态记忆与恢复 | 重载不扰动进行中组合（0.4 规则 10）；per-program 共享已落地 | fcitx5-rime 快照恰好一次（`rimestate.cpp:475-527`）+ per-app 初值 `app_options`（`rimesession.cpp:41-48`） | 重载后用户开关态的保留策略未契约化；无按程序预设初值 | IMP-40（per-app 初值预设，含重载模式保留策略评估） |
| 5. 极限规模退化与背压控制 | 预算全量断言（BUDGET-*）+ 槽位语义契约；候选上限 64 硬截 | librime 截断常量组 + LazyTable ×10 扩容 + k-best 名次衰减 cutoff（`poet.cc:325-341`、`table_translator.cc:117-207`）；WindInput 候选阶梯 100/300/1000→×2 至 CAP 5000（`handle_candidate.rs:1106`） | 深翻页供给阶梯缺失；bigram 截断后 miss 回退吞掉长尾（B-02） | IMP-21（名次衰减嫁接 + 候选供给阶梯 + bigram 预算重估） |
| 6. 自动化测试基建与可测试性 | 4,000 `#[test]` + testd 三档 + 202 例冒烟（**领先**）；但 E2E 依赖真 X（B-08）、冒烟只锚 @first:（B-09） | rime-ice **三层测试**（TSV 冒烟 ≈140 例含 `@today/@regex` 动态断言 + lint + CI，预期被推翻）；WindInput 双向协议解析守卫 + dbus-run-session e2e + SIGSTOP 验熔断（`e2e.sh`） | 渲染回归无头化缺口；冒烟断言维度单薄 | IMP-27（Xvfb 无头渲染回归层）、IMP-28（冒烟深化：top3/top5 + 开档矩阵 + 动态断言）；协议解析守卫作为 wire 演进纪律记录（§9） |
| 7. 多平台原生契合度、A11y 与 i18n | Linux 专用是产品边界；compose 委托宿主 | WindInput 一核三壳（`wind-host` HostSession 薄宿主 trait）；librime 平台差异压缩到 path 一个类型点 | N/A——单平台是特性不是缺陷 | **N/A（产品边界）；薄宿主理念已在 `engine/host.rs` 实践，维持** |

---

## 5. 六维能力成熟度综合评估（L1~L5）

| 维度 | rspinyin（stage 22 后） | librime | fcitx5-rime | rime-ice | WindInput | 评级依据 |
|---|---|---|---|---|---|---|
| 1. 核心业务深度（切分/成句/词库） | **L4**（↑自 L3） | L4 | L2 | L3 | L3 | bigram 真查+衰减+分层已落地（Round-1 成果）；bigram 截断与单词上文仍封顶（B-01/B-02） |
| 2. 操作效率与零摩擦旅程 | L5 | L3 | L3 | L3 | L4 | 预算断言/确定性/零网络保持满分；联想/快捷输入缺失被 WindInput/rime-ice 反超 |
| 3. 展示层质感 | L3 | — | L2 | L2 | **L5** | 自绘工艺归 opt-ui 轨道评估；主题模型与五态完备度代差明确（§3 矩阵） |
| 4. 感知性能与工程韧性（防呆/恢复/背压） | L4 | L3 | L3 | L2 | **L5** | 自愈链完整度持平 librime；配置演进防呆与宿主缺位兜底缺位 |
| 5. 可测试性与架构解耦 | **L5** | L3 | L1 | L3 | L4 | 4,000 测试 + testd 领先；无头渲染回归与冒烟断言维度是仅存缺口 |
| 6. 交付与跨平台暗线 | L4 | L4 | L2 | L3 | L5 | CI/审计/预算门禁强；单平台交付面窄（产品边界所致） |

**第二轮目标曲线**：把维 1 的残留封顶（bigram 覆盖/上文）与维 4 的防呆缺口（退役键/compose 兜底/负反馈）补齐，把维 2 的功能面（联想/快捷输入/自定义双拼）拉到 L4，维持维 5 领先并补上无头渲染回归。

---

## 6. 对标项目杀手锏机制与跨栈工程源码拆解（K-7 ~ K-12，续接 Round-1 K-1~K-6）

### K-7 WindInput 联想四源契约（SCOUT-D-09 / SCOUT-E-15）

- **① 原理**：`AssocSource{History, Bigram, Prefix, Punct}` 固定优先级逐源取配额、跨源不比分数（量纲不同）；`AssocHit{text, commit: Option<String>}` 展示与上屏文本分离；`boundary_broken`（失焦/点击/方向键/超时）断链即不出——「跨应用全局队列 ≠ 屏幕相邻」；`ALL` 数组 + 穷尽 match 编译器守门防漏源。
- **② 现状缺陷**：rspinyin 无联想（B-07）；但 bigram 段（v2）与 commit 历史恰好是 Bigram/History 两源的现成底座，前缀延伸可由 FST `prefix` 走查供给。
- **③ 移植路径**：引擎侧新增 `AssocSource` trait（旁路新增，不动冻结契约）；候选经既有通道产出，`CandidateSource` 增 `Assoc` 变体走 **ADR-0017**；呈现复用候选列表（WindInput 同款做法，无新 UI 面）。绞杀者式：先引擎后接线，配置默认关。

### K-8 rime-ice 快捷翻译器族与构建期拼写运算（SCOUT-ICE-05/06/07/22/24）

- **① 原理**：日期/数字大写/金额/计算器/Unicode 码点七类前缀翻译器（`rq/sj/xq/R/U/cC`…），候选 `quality=100` 强制置顶；拼写运算 derive/abbrev 与己方 dictc「构建期展开」路线同构；`melt_eng` 用同一机制表达数字/符号读音派生（P2P/X光）。
- **② 现状缺陷**：v 模式仅静态符号类别（`symbols.rs:1-5`）；无日期/计算/大写数字/码点直输（B-07）。
- **③ 移植路径**：快捷域候选走既有符号通道形态；**时钟/随机源必须 host 注入**（decoder 纯函数红线 0.4 规则 4，rime 直读 `os.time()` 不可照搬）；计算器禁 eval、用安全解析器（ASM-A-16）；数字大写/金额为纯算法直译。

### K-9 librime Poet k-best 名次衰减 + WindInput k=1 等价约束（SCOUT-A-09 / SCOUT-D-08）

- **① 原理**：k-best 组句带 `cutoff_threshold` 随名次衰减（×(1−1/N)，`poet.cc:325-341`）；WindInput 钉死「k=1 与单路径逐位相同」作为零回归依据、部分解 NEG_INFINITY 保留、去重留调用方（`pinyin/viterbi.rs:353`）。
- **② 现状缺陷**：己方 K-best Viterbi（beam 16）无名次衰减；上文重排仅单词且近平局带 1<<8（`viterbi/context.rs:56`，B-02）；bigram pair 双截断参数为手工常数（B-03）。
- **③ 移植路径**：衰减公式嫁接进 k-best 候选排序（整数 Q8.8 化）；tie band 与 pair 预算参数交 IMP-26 数据驱动整定；候选供给阶梯（初始少、翻页×2 至 CAP）吸收 WindInput 阶梯语义（SCOUT-D-11）作为同卡步骤。

### K-10 WindInput 主题求值坍缩与首显闸门（SCOUT-E-01/04/11/13）

- **① 原理**：light/dark 求值期坍缩单值、`refresh_effective_theme` 唯一合并点（热路径只读结果）；首显闸门 + 四逃生口治「陈旧坐标先显后跳」（实测 85-95ms 基线）；位置持久化三原则。
- **② 现状缺陷**：`theme.rs`/`spring.rs`/`adapter.rs` 恰 800 行贴顶（B-06）；光标定位三级源梯已有（`cursor.rs:6-14`）但无首显授权语义。
- **③ 移植路径**：**主题模型与首显工艺归 opt-ui 轨道**（本报告 §3 已路由）；本轮仅执行行限贴顶文件的预防性拆分（IMP-38，纯结构治理零行为变更），拆分时预留下沉接缝。

### K-11 rime-ice 三层测试基建 + WindInput 协议双向守卫（SCOUT-ICE-17 / SCOUT-F-01）

- **① 原理**：TSV 冒烟 `case_id\tschema\tkey_sequence\texpected_text` + `@today/@regex` 动态断言 + stderr E 级日志即失败；WindInput 测试内用正则**解析对端语言源文件**逐常量比对，把「手抄镜像漂移」变成 CI 红灯。
- **② 现状缺陷**：冒烟 202 例全部 `@first:`（B-09）；E2E 渲染回归需真 X（B-08）；wire 镜像已有静态尺寸断言但无字段 offset 钉死。
- **③ 移植路径**：冒烟 TSV v2（top3/top5 + 开档矩阵 + 动态断言）= IMP-28；Xvfb 无头层 = IMP-27；offset 钉死测试作为 wire 下一次演进时的强制伴生纪律（§9 记录，不立卡）。

### K-12 WindInput 布局数据化（双拼三表 TOML + 击键覆盖测试）（SCOUT-D-14）

- **① 原理**：双拼布局是数据不是代码——`[initials]/[finals]/[zero_initials]` 三表 TOML，内置 8 种布局零代码分叉；`official_zero_initial_strokes_work` 正向击键覆盖测试守「官方击键打不打得出」；零声母两大流派不可互抄有注释级规范。
- **② 现状缺陷**：己方双拼仅 6 个硬编码方案（`decode.rs:149-161`，B-11），新增布局=改代码+发版。
- **③ 移植路径**：布局装载进 ime-config（TOML 数据 + 校验 + 冲突检测），硬编码表转为内置数据文件的默认档；覆盖测试以官方击键表为断言源（自建，不拷 WindInput 数据）。

---

## 7. 系统改进项汇总表（IMP-21 ~ IMP-40，续编）

> 优先级：P0 = 排序质量残留封顶 + 体积红线 + 旗舰功能；P1 = 高价值功能面与测试暗线；P2 = 防呆细则与结构治理。
> 映射链 `IMP → GAP → TASK` 在 [features-add.md](features-add.md) 第二轮章节闭合；任务卡见 `features-add-phase4/5/6.md`。

| 编号 | 改进域 | 具体痛点（现状锚点） | 借鉴对标机制 | 改进方案摘要 | 优先级 |
|---|---|---|---|---|---|
| `IMP-21` | 架构·排序深度 | k-best 无名次衰减；上文单词+窄带（`viterbi/context.rs:56`）；候选 64 硬截无翻页阶梯（B-01/B-02） | K-9（poet cutoff + WindInput 阶梯） | k-best 名次衰减整数化嫁接 + 上文带宽度/pair 预算重估（依赖 IMP-22 腾位）+ 候选供给阶梯 | **P0** |
| `IMP-22` | 架构·数据底座 | base.dict 17.6MiB / 20MiB 预算 84%（B-04），扩源即顶格 | rime-ice 分表分层纪律（`cn_dicts/` 六表职责） | 词条价值审计 + 裁剪管线（低价值长尾按分位裁剪）+ 容器段份额重估 | **P0** |
| `IMP-23` | 核心功能·联想 | 无下一词预测（B-07）；bigram 后继与 commit 历史闲置 | K-7（WindInput 四源契约） | `AssocSource` 三源（Bigram/History/Prefix）逐源配额 + `CandidateSource::Assoc`（ADR-0017）+ 候选通道复用 | **P0** |
| `IMP-24` | 核心功能·快捷输入 | 无日期/时间/大写数字/金额/计算/码点直输（B-07） | K-8（rime-ice 翻译器族） | 前缀域翻译器族 + ClockSource/RngSource host 注入 + 安全表达式解析 | **P1** |
| `IMP-25` | 核心功能·双拼 | 双拼 6 方案硬编码（`decode.rs:149-161`，B-11） | K-12（WindInput 布局数据化） | 布局 TOML 数据化 + 校验/覆盖测试 + 硬编码表转默认数据 | **P1** |
| `IMP-26` | 数据工程 | 归一化/衰减/bigram 参数全手工常数（`dictc.rs:432-433`，B-03） | rime-ice polyphone.go 数据把关思想 | `xtask tune` sweep 扩展：以 holdout top1 为目标函数整定参数 + 采集环境纯净度 precheck | **P1** |
| `IMP-27` | 工程暗线·测试 | E2E 渲染回归需真 X server（B-08） | WindInput dbus-run-session + Xvfb e2e（`e2e.sh`） | testd 新增 Xvfb 无头通道：虚拟屏跑渲染/截图/golden，CI 可跑 | **P1** |
| `IMP-28` | 工程暗线·测试 | 冒烟 202 例全部 `@first:`（B-09），开档行为无回归 pin | K-11（rime-ice 动态断言） | TSV v2：top3/top5 锚 + fuzzy/abbrev/correction/english 开档矩阵 + `@date/@regex` 动态断言 | **P1** |
| `IMP-29` | 核心功能·符号 | 符号域静态类别、无 emoji、表一次性加载不随重载刷新（B-05/B-07） | rime-ice 66 子域 + 索引页（`symbols_v.yaml`） | 符号表分类学扩展（emoji 域）+ 自描述索引页 + ArcSwap 热重载 | P2 |
| `IMP-30` | 核心功能·按键 | 无以词定字（`[`/`]` 取候选首/尾字） | rime-ice `select_character.lua:20-60` | 按键层字级切分上屏（光标位置保留语义一并实现） | P2 |
| `IMP-31` | 核心功能·负反馈 | 用户负反馈仅 Forget 一态 | rime-ice 删词/隐藏/降频三态（`cold_word_drop/`） | 降频（权重惩罚）+ 隐藏（会话级抑制）补齐，存储走用户词库 tombstone/惩罚位 | P2 |
| `IMP-32` | 核心功能·英文域 | 英文候选无大小形变与连排空格（ENGLISH v1 已落地直通） | rime-ice autocap/en_spacer（`autocap_filter.lua`） | 按输入码形态大小写套壳 + 中英交界/英文连排空格（纯文本后处理） | P2 |
| `IMP-33` | 数据工程 | 撞车英文短词与长词位次无数据化调控 | rime-ice reduce_english/long_word_filter | dictc 构建期标记：降权名单 + 长词优先位次 + 大写字母/数字入词 | P2 |
| `IMP-34` | 核心功能·辅码 | 无辅码筛选（精确重码定位） | rime-ice `search.lua:114-284` + WindInput aux-code 两形态 | 词典预存辅码列（dictc）+ 引导键会话快照重筛（候选子序列不变量） | P2 |
| `IMP-35` | 核心功能·反查 | 无拆字/笔画反查（不知读音打不出字） | rime-ice `uU` 域 + WindInput wind-reverse 多源注释库 | 反查域前缀 + 拆字表数据（自建）+ 反查候选通道 | P2 |
| `IMP-36` | 工程暗线·运维 | 用户词典导出/导入/备份无 CLI 入口（B-13） | librime `rime_dict_manager` 工具定位 | `xtask user-dict` 子命令（list/export/import/forget/pin 管理） | P2 |
| `IMP-37` | 工程暗线·配置 | 配置演进无退役键机制；值迁移无「先落盘再清键」纪律 | WindInput RETIRED_KEYS（`config.rs:142-193`） | ime-config 增退役键清单 + 物理清理 + 迁移顺序铁律 + 防漂移守护测试 | P2 |
| `IMP-38` | 架构·维护 | theme/spring/adapter 恰 800 行贴顶、router.rs 749（B-06） | K-10（WindInput 求值坍缩的下沉接缝） | 预防性拆分四文件（零行为变更），为 opt-ui 轨道预留下沉接缝 | P2 |
| `IMP-39` | 工程暗线·桥接 | compose 完全依赖宿主，缺位即静默退化（B-12） | fcitx5-rime compose 协议（`rimestate.cpp:169-222`） | 插件内置最小 compose 表兜底（宿主可用时仍委托宿主） | P2 |
| `IMP-40` | 工程暗线·个性化 | 无按程序预设初值；重载后开关态保留策略未契约化 | fcitx5-rime `app_options`（`rimesession.cpp:41-48`） | per-app 模式初值预设（app 标识复用 hash 通道）+ 重载模式保留策略评估 | P2 |

---

## 8. 战略取舍与反膨胀剪裁边界（Anti-Bloat · Round-2）

以下能力在本轮对标中存在，但**坚决不予照搬**（Round-1 §6 剪裁继续有效，此处只登记新增裁决）：

| 对标能力 | 拒绝/降级理由 |
|---|---|
| WindInput 外置服务进程架构（UDS + push + SHM 像素通道） | 架构分歧非缺陷：进程内双 cdylib + 结构化 UiFrame（ADR-0011）可测性与预算性更强；像素通道与 opt-ui 工艺轨冲突。只吸收其**语义**（溢出全量对齐思想记录备查） |
| WindInput IPC 失败 11 分类/熔断/SO_PEERCRED/SHM fstat 防呆 | 进程内形态无 IPC 面，全部 N/A（§4 维 2/3 已注明） |
| WindInput cmdbar/softkeyboard/quick-input 全套浮层与工具栏 | 产品形态超出输入法插件边界；交互工艺归 opt-ui/产品决策 |
| WindInput GHOST_ALPHA 假隐藏、1s 工具栏淡出 | 视觉工艺，归 opt-ui 轨道 |
| librime Registry/Module/dlopen 插件、octagram、librime-lua | Round-1 §6 已拒绝，维持 |
| librime Table 4 级索引/extra_code 重构、OffsetPtr 容器层 | 己方 FST+WORDLIST+CRC 已满足且更强（自愈）；重建索引结构收益不足 |
| librime LazyTable 惰性物化/Menu 惰性归并 | Round-1 已拒绝（负优化），维持；仅吸收「候选供给阶梯」的调参语义进 IMP-21 |
| librime UserDbMerger 多机 CRDT 同步 | features.md 3.02.05 已结论默认不实现，维持 |
| librime `__patch/__include` 配置 DSL 与 rime-ice custom patch 协议 | 配置管线已定向（TOML 编译期）；仅吸收「用户层与发布层物理分离」思想进 IMP-25/37 |
| rime-ice 词库/tencent 大表直接取数 | **GPL-3.0** + 腾讯词向量许可存疑（Round-1 裁决维持，B-15 天花板记录在案）；所有词表/纠错表/降权名单/农历数据必须自建 |
| rime-ice 农历/UUID/T9/长句动词库 | 长尾低频域，列为 IMP-24 非目标（扩展点保留），防首批面铺得过宽 |
| rime-ice `load()` 动态执行计算器 / lua 直读 `os.time()` | 安全与纯函数红线（0.4 规则 4/6）：表达式求值用安全解析器（ASM-A-16），时钟/随机源一律 host 注入（ASM-A-13） |
| WindInput build_guard 死亡记录 | dictc 为离线工具 + quarantine/readonly 自愈已覆盖崩溃语义，收益不足 |
| WindInput 日志 `.1` 恒最新滚动命名 | tracing-appender 既有滚动策略满足，运维习惯差异不立项 |
| WindInput `Engine::memory_parts()` 自报内存 / startup_trace | 预算审计已有 budgets.json 断言通道；登记为未来预算增强候选，本轮不立项 |
| Wayland 协议 XML 集合扩展（input-method-v2 等） | features.md R-02 决策点管辖，本轮增量不新增 Wayland 行为（ASM-A-01 延续） |

---

## 9. 与既有规划文档的边界（Round-2 维持）

- **`docs/dev/features.md`**：权威规范，编号空间不共用（Round-1 X-5 裁决延续）。
- **`docs/dev/opt-ui.md`**：候选窗视觉/主题模型/首显工艺/空态呈现全部归它（§3 矩阵已路由）；本轮 IMP-38 只做零行为变更的行限拆分。
- **`docs/dev/features-test.md` + `tests.md` v3.0**：IMP-27/28 是对其 Xvfb 通道与冒烟 TSV 的**数据与通道扩展**，用例卡由 `tests/add.md` 的 `可执行性：[待实现]` 机制自动转可执行，不重复其结构。
- **ADR 纪律**：本轮契约增量 `CandidateSource::Assoc` 认领 **ADR-0017**（`docs/dev/adr/README.md` 取号）；时钟/随机源 host trait 走 `ime-fcitx5/src/engine/host.rs` 既有内部 trait 扩展，不动冻结契约。
