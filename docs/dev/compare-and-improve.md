# 与对标项目深度对比分析及系统改进报告（rspinyin vs Rime 技术栈）

> 报告版本: v1.0 ｜ 本项目基线: `/home/gong/news/rspinyin`（commit `74afea9` + 工作区未提交增量，审计日期 2026-10-14）｜ 对标输入源: `./docs/dev/compare/*.md`（5 份存量报告，四路并行源码级调研产物；调研对象为 `remarks/librime`、`remarks/fcitx5-rime`(5.1.17)、`remarks/rime-ice`(≈2026-03, GPL-3.0) 三个本地参考仓库）｜ 系统形态: Desktop GUI（Linux 桌面输入法：进程内 Fcitx5 插件 + 全自绘候选窗）｜ 关联增量规划: [./features-add.md](features-add.md)
>
> 本报告不重复五份存量报告的正文，只做三件事：**归并纠偏**（以双边真实源码为唯一事实基准）、**综合评估**（六维源码对比 + 五维成熟度）、**收敛改进项**（`IMP-01`~`IMP-20` 注册表）。所有行号引用均经 2026-10-14 工作树复核。

---

## 1. 输入源勘察与存量报告归并纠偏

### 1.1 存量报告清单

| 报告 | 性质 | 对象 | 交叉结论去向 |
|---|---|---|---|
| [rspinyin-current.md](compare/rspinyin-current.md) | 本项目现状盘点（9 crate + xtask，行数实数统计） | 本仓库 | [rime-comparison.md](compare/rime-comparison.md) §2 |
| [librime.md](compare/librime.md) | 对标源码深读（pipeline/syllabifier/poet/table/prism/userdb/algebra） | `remarks/librime` | 同上 §2 |
| [fcitx5-rime.md](compare/fcitx5-rime.md) | 对标源码深读（桥接层 2700 行全量通读，平台服务清单） | `remarks/fcitx5-rime` | 同上 §3 |
| [rime-ice.md](compare/rime-ice.md) | 对标数据层深读（词库构成/词频策略/Lua/数据工程） | `remarks/rime-ice` | 同上 §4 |
| [rime-comparison.md](compare/rime-comparison.md) | **综合对比报告 v1.1**（含 P0~P3 改进清单与 §4.7 词源许可盘点） | 三者 | 本文件 §3~§6 的直接输入 |

### 1.2 源码纠偏表（本报告复核新增，Planning 前必须生效）

以当前工作树真实源码为唯一事实基准，对存量报告逐条复核，得到以下纠偏与精确化。**凡与本表冲突的旧表述，一律以本表为准**：

| # | 旧表述 | 复核结论（含证据锚点） | 对规划的影响 |
|---|---|---|---|
| X-1 | [librime.md](compare/librime.md) 总结表 #4/#6 称 rspinyin「每键全表过滤」「单词频排序等价于贪心」 | 该报告**存档备注已自我纠偏**：rspinyin 实为 FST 整码查询 + K-best Viterbi（beam 16）整句首选，解码骨架不落后 | 解码骨架不再立项，改进集中于**打分输入**（bigram/衰减/分层）而非骨架 |
| X-2 | [rspinyin-current.md](compare/rspinyin-current.md) §6 称 `engine.punct_mode`/`full_width` 「接受但未接线」 | **精确化**：运行期切换已接线——`crates/ime-fcitx5/src/engine/router/modes.rs:331` 和弦切换、`modes.rs:370` `transform_output` → `transform_committed` 均生效；缺的只是 `[engine]` 配置默认值到会话初始位的投影（`modes.rs:221-230` 使用硬编码默认而非配置投影） | 降级为 0.5 人天量级的小任务（`TASK-2.03.02`），而非 P1 整卡 |
| X-3 | [rime-comparison.md](compare/rime-comparison.md) §4.7 将 Unihan 列为「建议接入」的词源 | **已落地**：`data/sources.toml:41-50` 已登记 `unihan`（L1，SPDX `Unicode-3.0`），且 `xtask/src/dictc/quality.rs:400-487` 已按来源（含 `Origin::Unihan`）做逐源审计 | 不再为 Unihan 立项；词源扩充只覆盖华宇野风 + THUOCL + melt 英文词表 |
| X-4 | [rime-comparison.md](compare/rime-comparison.md) §2.2 称 bigram 退化点在 `dict_lm.rs:28-36` | 精确锚点为 `crates/ime-dict/src/fst_index/dict_lm.rs:207-214`：`fn bigram` 返回 `self.unigram(word).saturating_add(BIGRAM_MISS_PENALTY)`（miss 路径必须与内存模型逐字节同序，注释有等价交换契约） | 任务卡锚点按新行号；替换实现时必须保持「两个模型可互换、同序」契约 |
| X-5 | 旧版 `docs/dev/features-add.md`（52 张卡，`74afea9` 随本报告之前的提交删除） | 三处登记基线缺陷已闭环；旧卡编号沿用 features.md 的 `TASK-2.xx/3.xx` 空间，**全部作废不得引用** | 本次规划在 features-add 文档簇内重新编号（`TASK-[1-3].[簇].[序]`），与 features.md 的索引表建立映射而非共用编号 |
| X-6 | `Lexicon::prefix` 的签名状态 | `crates/ime-types/src/lexicon.rs:49` 已冻结真实语义签名 `prefix(&self, prefix: &str, limit: usize) -> Result<WordIter<'_>, ImeError>`；`fst_index.rs:399-403` 默认实现返回 `Unsupported`，注释明言「实现是对同一 FST 前缀区间的一次流式走查」 | 简拼接线是**纯实现工作**，零契约变更（`ime-types` 不动） |
| X-7 | 用户词频打分公式 | `crates/ime-core/src/lm/score.rs:250-257` 边分五项中 user 项为 `user * min(log2(1+freq)/8, 2) * 256`；`ScoreWeights` 默认 `uni=256, bi=154, len=90, seg=128, user=205`（`score.rs:173-177`） | 衰减改造替换的是**这一项的输入**（freq → 衰减后概率），权重通道复用 |
| X-8 | `PinHighlighted` 现状 | `crates/ime-core/src/state/transitions.rs:203-204`：`KeyAction::PinHighlighted => ctx.push(Effect::Diagnose(ImeError::Unsupported))`；`crates/ime-dict/src/user_db.rs:177,336` 已有 `pinned: Mutex<HashSet<Box<str>>>`（淘汰保护 + `user_meta` 的 u64 位） | 置顶词是「引擎侧转正」，存储字段已就绪，无 schema 变更 |
| X-9 | 冻结契约对新能力的预留 | `crates/ime-dict/src/format/mod.rs:84-88` 保留 `FLAG_BIGRAM_PRESENT` 与 `SectionKind::Bigram`（v1 恒空）；`user_words` 表 `(u32 commits, u64 last_used_ms)`（`user_db.rs:176`） | bigram 启用 = 容器 v1→v2 段填充 + 读端兼容，无需动 `ime-types` |
| X-10 | `ui/script/unavailable` 错误码 | `docs/dev/features.md` 验收记录明确：该码**已登记但没有上报方**（`ScriptIndex::unavailable()` 提供降级值，缺引擎侧上报点） | 简繁接线卡（`TASK-2.03.01`）的 DoD 就是把这个既有码接上上报方，**禁止新造码** |

### 1.3 双边工程基线对照表

| 维度 | librime / fcitx5-rime / rime-ice | rspinyin（当前） | 判定 |
|---|---|---|---|
| 语言与形态 | C++17 引擎 + C++ 桥 + YAML/Lua 数据 | Rust 2024 workspace（9 crate + xtask），双 cdylib | 结构性差异，取向已定（features.md 权威） |
| 代码规模 | librime ≈ 5 万行 + lua 2.7k + fcitx5-rime 2.7k | 非测试 Rust ≈ 14.3 万行（rspinyin-current.md §1 实数） | — |
| 切分 | `SyllableGraph` 顶点驱动 + prism 前缀搜索（`algo/syllabifier.cc:31-275`） | `SyllableDag` 字节偏移 + SmallVec 边表 + 反向可达剪枝（`segment/dag.rs:80-157`） | 平手（同为全切分 DAG） |
| 成句 | poet 双策略：无 LM 一元 Viterbi / 有 LM 按前词分桶束搜索（`poet.cc:134-253`） | 单次 K-best Viterbi beam 16，整句首选（`viterbi/mod.rs:193` 钉死测试） | 骨架平手，**LM 输入是差距** |
| 语言模型 | Grammar 插槽 + octagram 插件（本 checkout 未含） | `DictLm`：unigram 真查表（Q12→Q8.8 预映射 16KiB）+ bigram 恒 miss（X-4） | **落后（槽位已预留，渐进可补）** |
| 词库 | rime-ice 五文件分层 ≈192 万条；librime `.table.bin` 四级索引 + `.prism.bin` | 单容器 6 段 mmap：348,972 词条 / 292,315 键 / 15.62MiB；`MAX_WORDS_PER_KEY=32` 截断 | **规模与分层落后；零拷贝与校验领先** |
| 拼写运算 | 部署期 `Projection.Apply` 物化进 Prism（运行时零运算） | 运行期 fuzzy 扩边（8 类全实现）+ 双拼五套表 + dictc L3b 定向展开；**纠错缺失** | 引擎能力基本平手；**生产接线与纠错是差距** |
| 用户数据 | LevelDB `c/d/t` + 指数衰减 + 逻辑 tick（`algo/dynamics.h`） | redb `(commits, last_used_ms)`，批量落盘、readonly 降级、备份三代；**无衰减、无造词** | 衰减/造词落后；**落盘纪律领先** |
| UI 路线 | fcitx5-rime 零自绘，全委托 classic-ui | 全自绘（X11 override-redirect + Wayland 四档阶梯 + UIPriority 接管 + 失败回退） | 不可比（产品定位差异）；自绘的结构性收益（焦点零抢占）与代价（工艺自担，由 opt-ui.md 承接）并存 |
| 会话管理 | 三档共享粒度 + 快照/恢复（`rimesession.cpp:104-134`） | 每 IC 独立 `Context`，session id 分域 | **共享粒度缺失**（真实体验痛点） |
| 工程暗线 | checksum 增量部署；rime-ice 有 Go 校验链 + `rime_api_console` 冒烟 | budgets.json 预算断言、零网络断言、双层脱敏、SIGBUS/SIGSEGV 自愈、XTEST 三通道真窗口测试、3,701 个 `#[test]` | **rspinyin 显著领先** |
| 许可 | rime-ice 整仓 GPL-3.0；librime BSD；上游词源各有许可（§4.7 三档盘点） | MIT OR Apache-2.0；`sources.toml` 白名单 + sha256 pin + `check-dict` CI 审计 | rspinyin 分发合规更强；**取数必须从许可干净的原始上游** |

---

## 2. 顶层架构范式对比（源码级）

### 2.1 核心定位与边界

- **librime**：引擎库，把一切做成可插拔组件（Registry/Module/动态插件，`engine.cc:309-400` 配置驱动实例化）。收益是 octagram 可以独立成插件，代价是运行期间接层与攻击面。
- **rspinyin**：静态组合的 9 crate 单向 DAG（`ime-types ← core ← dict ← …`），能力在 dictc **编译期物化**（L3b 定向多键展开、多音字纠音、分段 CRC），运行期零脚本、零网络、可预算。这是「借策略、不借机制」的取向对比：**librime 的拼写层物化思想 rspinyin 已经在同构地实践**（dictc L3b ≈ Prism SpellingMap 的选择性版本），差距不在范式而在数据与打分输入。

### 2.2 数据模型与存储

| 机制 | librime | rspinyin | 差距结论 |
|---|---|---|---|
| 词条文本 | marisa StringTable，每次查询拷出 `std::string`（`table.cc:228-230`） | STRPOOL 零拷贝，`WordRef` 借 mmap（`fst_index.rs:157-183`） | rspinyin 领先，不学 |
| 索引 | Head/Trunk/Tail 四级 + `extra_code` 尾匹配（`table.h:54-77`） | FST 整码 + WORDLIST 间接层，`MAX_WORDS_PER_KEY=32` 硬截断 | 长尾重码键丢词（IMP-05）；整句路径不受影响（每 span 独立查询） |
| 词频存储 | log 域 float（编译期 `log(词频)`，`dict_compiler.cc:257`） | 线性 u32 weight，运行期 Q8.8 映射 | 大词频差被压缩（Q8.8 只有 8 个整数倍 step），**分层归一化（IMP-02）+ log 域映射（IMP-01）共同治理** |
| 用户词典 | 有序 KV + `SyllableGraph::Transpose` 顺序扫描协同 | redb 两表 + 内存水合，decode 永不开读事务 | rspinyin 落盘/恢复纪律更强；**衰减与造词缺失**（IMP-01/IMP-09） |

### 2.3 并发与事件调度

- librime：每 session 单线程无锁，后台仅 Deployer 线程；组件间 boost::signals2。
- fcitx5-rime：`EventDispatcher` 投回主循环 + `mainThreadId` 同步快路径（`rimeengine.cpp:527-554`）。
- rspinyin：宿主线程（key→decode→post）与 UI 线程（poll(2) 多路复用）物理隔离，SPSC+eventfd 通道语义表契约化（Frame latest-wins / Show/Hide 有序塌缩 / Select 环 64）；引擎↔UI addon 走 C ABI wire（ADR-0011）。**rspinyin 方案更严格，全部保持，不从对标引入任何并发机制。**

### 2.4 交互与人体工学

自绘 vs 委托不可比（X-2 表外），但 fcitx5-rime 的**平台服务借用的清单**（会话共享、切输入法收尾四档、compose、capability 协商、StatusArea Action、通知节流）是白拿的能力清单，rspinyin 自绘只替换了「渲染消费者」，1~6 项应照单全收（→ IMP-12/13/14）。

### 2.5 扩展机制

librime 的组件注册/动态插件/Lua 三件套全部**不予引入**（§7 反膨胀）；rspinyin 的扩展点是 dictc 数据管线（编译期）+ KeyAction/DecodeFlags（契约内开关）+ Overlay/短语表（既有通道）。

### 2.6 错误处理与可观测性

rspinyin 领先且保持：`domain/action/reason` 稳定错误码 + 双层脱敏 + panic/SIGBUS 兜底 + quarantine 自愈 + 探针看板。对标中唯一值得吸收的是 rime-ice 的**数据驱动冒烟**（TSV `key_sequence→expected_text`，`rime_api_console` 驱动）——rspinyin 有更强的 `xtask testd-engine` 基建，缺的只是用例集（→ IMP-19）。

---

## 3. 五维能力成熟度评估（Desktop GUI 形态）

| 维度 | rspinyin | Rime 栈 | 评级依据 |
|---|---|---|---|
| 1. 核心业务深度（切分/排序/成句/词库） | **L3** | **L4** | 骨架同级；bigram 恒 miss、无衰减、无纠错、无分层把排序上限钉死在一元词频（top1 73.8%） |
| 2. 操作与调用效率（延迟/资源/确定性） | **L5** | L3 | decode ≤3ms 预算断言、字节指纹确定性、mmap 零拷贝、零网络 |
| 3. 数据组织与可靠度（校验/自愈/规模） | **L4** | **L4** | 逐段 CRC + quarantine + 备份三代领先；规模 35 万 vs 192 万、分层缺失落后 |
| 4. 个性化与生态开放（用户调频/造词/词源/可定制） | **L2** | **L5** | 词频无衰减、造词未实现、置顶 Unsupported、词源单源；Rime 有 schema/custom 生态 |
| 5. 交付与工程暗线（测试/隐私/门禁/可观测） | **L5** | L3 | XTEST 三通道 + 3,701 测试 + 预算门禁 + 脱敏 + 崩溃自愈，超三个参考仓库平均 |

**短板诊断**：成熟度曲线呈「哑铃形」——工程暗线（维 2/5）满分而个性化与排序深度（维 1/4）塌陷。**本轮增量的全部意义就是把维 1/维 4 抬到 L4**：四件 P0（分层/衰减/bigram/置顶）+ 一组接线（模糊音/简拼/简繁/会话共享）+ 数据生态（词源/冒烟）。UI 工艺维（opt-ui.md 轨道）不在本报告域内。

---

## 4. 对标项目杀手锏机制拆解与借鉴方案（6 项）

### K-1 rime-ice 三层词频归一化与词长梯度（数据工程）

- **① 原理**：base（两字词主力）保留真实语料词频（1 ~ 1,926 万，跨 7 个数量级，`base.dict.yaml` 头注释实测分位数见 rime-ice.md §2.2）；ext/tencent（3+ 字词，约 132 万条）**全部压平为 100**，只靠「是否收录」不靠词频区分；字表用真实字频，且刻意保持词长梯度（两字均值 25,488 > 三字 7,058 > 四字+ 3,928），「同音先短词」由数据保证而非引擎逻辑。多音字纪律：字表字频比 5:100 阈值作为自动注音开关，`others/script/rime/polyphone.go` 机器把关。
- **② 本项目缺陷**：jieba 词频是单一真实分布直接进排序，长尾噪声与头部真频同权竞争——73.8% top1 的重要损耗源。
- **③ 改造路径**：在 dictc 权重层加一段**归一化管线**（层带划分 + 压平 + 词长梯度校验），容器格式零变更（IMP-02 → GAP-01）；随后以华宇野风（PD）替换/增强两字词层（IMP-15 → GAP-08），把「分词统计频」升级为「输入法场景真频」。

### K-2 librime UserDict 动态调频数学（`algo/dynamics.h`）

- **① 原理**：全局逻辑时钟 tick（每次上屏 +1，meta `/tick`）；衰减 `dee += da·exp((ta−t)/200)`（≈139 次上屏半衰期）；概率映射 `formula_p` 后取 `log` 进候选权重，与系统词统一比较；负 commits 标记删除；`dee ≤ 1e-200` 直接丢弃。
- **② 本项目缺陷**：`(commits, last_used_ms)` 单调增，多年前的词与本周的词同权（`user_db.rs:17`），是用户最先感知的排序劣化；user 项 `min(log2(1+freq)/8,2)` 无时间维。
- **③ 改造路径**：redb schema v2 增 `dee/tick`（新表 + 迁移，X-9 的表定义模式），衰减数学在**水合/落盘侧**离线批算，decode 侧只做 O(1) 整数近似映射（IMP-01 → GAP-02）；半衰期等参数进配置并给默认值。librime 的 LevelDB 每 commit 写库**不学**，rspinyin 的批量延迟落盘保留。

### K-3 librime Grammar 槽与 poet 双策略成句

- **① 原理**：`Grammar::Query(context, word, is_rear)` 唯一虚方法（`gear/grammar.h:11-28`）；有 LM 走按「最后一个词」分桶的束搜索（宽 7，`poet.cc:134-168`），无 LM 退化一元 Viterbi + cutoff 多句。`contextual_translation` 对已生成候选按上文重打分、组内重排（每轮 ≤32 个）。
- **② 本项目缺陷**：K-best Viterbi 骨架已在（更强：全整数、beam 16），但 `DictLm::bigram` 恒 miss（X-4）——**打分上限被钉死在一元**；且无「上一 commit 词作上下文」的跨会话重排。
- **③ 改造路径**：填 BIGRAM 段 + 真查表（IMP-03 → GAP-03，保持「两模型可互换同序」契约 X-4）；随后把「上一 commit 词」接入解码器初始 `prev_word` 与候选重排（IMP-11 → GAP-12）。octagram 式 500MB 模型**不做**（§7）。

### K-4 librime Prism 拼写层物化与 credibility 对数信用分

- **① 原理**：部署期 `Projection.Apply` 把 fuzz/abbrev/correction 全部物化成 trie 边 + `(音节id, type, credibility)` 属性表，运行时零运算；信用分对数可加：fuzzy/abbrev −log0.5、completion −log0.05、correction −log0.01（`calculus.cc:14-16`），随边进 log 域求和。
- **② 本项目缺陷**：fuzzy 8 类扩边已在但**生产无配置键、会话不设位**（`state/machine.rs:454` 只设 SHUANGPIN）；简拼代数 + walk 完整但 `prefix` 未实现、配置键标注 Not wired（`ime-config/src/reload.rs:125-127`）；纠错完全没有；容差只有简拼固定罚 + 每分段 seg 罚，无分级。
- **③ 改造路径**：接线走契约内开关（DecodeFlags 已有 8+1 位 fuzzy 位、`engine.abbrev` 键已存在）——IMP-06/IMP-07 是**纯接线**；纠错复用同一扩边机制 + 重罚（correction 级 ≈ −log0.01 的 Q8.8 当量），先做键盘邻位与键序颠倒（IMP-10）；分级容差则通过给扩边罚值分档实现，不引入 Prism。

### K-5 fcitx5-rime 会话池三档共享粒度

- **① 原理**：`RimeSessionPool` 键由 `PropertyPropagatePolicy` 决定——每 IC（`u:<uuid>`）/ 同程序（`p:<program>`）/ 全局（`g:`），weak_ptr 持有、无引用自动销毁；策略切换时快照 + 重建；与 fcitx5 全局 `shareInputState` 联动（`rimesession.cpp:104-134`、`rimeengine.cpp:770-782`）。
- **② 本项目缺陷**：每输入上下文独立 `Context{session, modes, anchor, phrases}`，同程序的两个窗口输入状态互不相通——真实体验痛点。
- **③ 改造路径**：host 层 Context 池键改为可配置（`per-context` 默认 / `per-program`），program 键取 `privacy_impl` 已有的 app 标识（hash 后），模式位随共享上下文迁移；不学其 Lua/schema 菜单部分（IMP-12 → GAP-09）。

### K-6 rime-ice 数据工程纪律

- **① 原理**：Go 工具链（注音校验/多音字校验/排序去重/emoji 生成校验）+ `make lint` + **基于 `rime_api_console` 的端到端冒烟**（TSV 数据驱动 `key_sequence→expected_text`，支持 `@today/@regex`）+ AGENTS.md 写死的词库维护约束（新词默认词频 100、两字词进 base、tencent 禁多音字）。
- **② 本项目缺陷**：dictc 已有 sha256 pin、双编译一致性、6,000 条留出集 quality 门，但缺「多音字自动校验」与「数据驱动的解码冒烟用例集」；排序质量改动（分层/bigram）尤其需要可回归的期望文本基线。
- **③ 改造路径**：`xtask testd-engine` 基建现成，补 TSV 用例集与断言通道（IMP-19 → GAP-10）；多音字校验吸收「字频比阈值」规则进 dictc（IMP-15 的 `TASK-2.04.02`）。

---

## 5. 系统改进项汇总表（IMP-01 ~ IMP-20）

> 优先级：P0 = 直击 top1 73.8% 与用户可感知排序；P1 = 已有能力接线 + 数据生态，性价比最高；P2 = 结构性升级与功能面扩充。
> 每项的落地任务卡见 `features-add-phase*.md`；映射链 `IMP → GAP → TASK` 在 [features-add.md](features-add.md) §3/§5 闭合。

| 编号 | 改进域 | 具体痛点（现状锚点） | 借鉴对标机制 | 改进方案摘要 | 优先级 |
|---|---|---|---|---|---|
| `IMP-01` | 架构·数据底座 | 用户词频单调增无衰减（`user_db.rs:17`），user 项无时间维（`score.rs:250-257`） | librime `formula_d/p` + 逻辑 tick（K-2） | redb schema v2（dee/tick）+ 迁移；衰减在落盘/水合侧批算，decode 侧 O(1) 整数映射进 user 项 | **P0** |
| `IMP-02` | 架构·数据底座 | jieba 单一真实词频直接排序，长尾噪声同权（top1 损耗源） | rime-ice 三层归一化 + 词长梯度（K-1） | dictc 权重层加归一化管线：两字真频带 / 3+ 字压平带 / 字表真频；容器零变更 | **P0** |
| `IMP-03` | 架构·数据底座 | `DictLm::bigram` 恒 miss（`dict_lm.rs:207-214`），排序上限一元 | librime Grammar 槽形态（K-3） | 容器 v1→v2 填 BIGRAM 段（段格式/槽位已预留，X-9）+ dictc 共现统计生成 + `DictLm` 真查（保两模型同序契约） | **P0** |
| `IMP-04` | 架构·数据底座 | `PinHighlighted → Unsupported`（`transitions.rs:203`），pinned 字段闲置 | rime-ice pin_cand_filter 语义 | 引擎侧转正：置顶 = 打分极限提升 + 状态机闭合 + 管理闭环（forget/列表/提示） | **P0** |
| `IMP-05` | 架构·数据底座 | `MAX_WORDS_PER_KEY=32` 硬截断，重码键丢词 | librime TailIndex/extra_code（简化版） | 按权重智能截断 + 超出词转入溢出表（长词补表），整码查询路径不变 | P2 |
| `IMP-06` | 核心功能接线 | fuzzy 8 类全实现但无配置键、会话不设位（`machine.rs:454`） | librime fuzz 物化语义（能力已有） | `[fuzzy]` 配置节（8 类三态）→ reload 投影 → 会话设 `DecodeFlags::FUZZY_*` | **P1** |
| `IMP-07` | 核心功能接线 | 简拼 walk 完整但 `prefix → Unsupported`（`fst_index.rs:401`）、配置键 Not wired（`reload.rs:125`） | librime abbrev/completion 边 | `FstLexicon::prefix` 流式前缀走查实装 + `engine.abbrev` 接线 + 候选合并 | **P1** |
| `IMP-08` | 核心功能接线 | 简繁算法与 `ScriptIndex` 齐备但 commit 未走、`ui/script/unavailable` 无上报方（X-2/X-10） | rime-ice traditionalize 开关语义 | commit 路径挂 script 变换 + `ToggleScript` 状态闭合 + 接上既有错误码 + `punct_mode/full_width` 配置默认值投影（X-2） | **P1** |
| `IMP-09` | 核心功能补齐 | `is_user_word` 恒 false（`user_db.rs:694`），无造词写路径 | librime `Memory::OnCommit` 自动造词 | 句内成分回写用户造词表 + 词格融合 + 与衰减 schema v2 同期 | P2 |
| `IMP-10` | 核心功能补齐 | 拼写纠错完全缺失（无 corrector 文件） | librime NearSearchCorrector + credibility −log0.01（K-4） | 复用 fuzzy 扩边基建：邻位替换 + 键序颠倒 + correction 重罚 + 数量上限 | P2 |
| `IMP-11` | 上下文排序 | 无上文重排；跨 commit 无上下文 | librime `contextual_translation`（K-3） | 上一 commit 词作 bigram 上下文：解码初始 `prev_word` + 同分候选组内重排（依赖 IMP-03） | P2 |
| `IMP-12` | 桌面桥接工艺 | 每 IC 独立 Context，同程序窗口状态不通（K-5） | fcitx5-rime 三档共享粒度 | Context 池键可配置（per-context/per-program），app 标识复用 hash 通道 | **P1** |
| `IMP-13` | 桌面桥接工艺 | 切输入法无收尾策略；preedit 无 capability 协商 | fcitx5-rime 四档收尾 + `CapabilityFlag::Preedit` 检查 | 切走时可配「提交/清除未上屏编码」；client_preedit 按 IC capability 自动降级 | P2 |
| `IMP-14` | 桌面桥接工艺 | 无 compose/dead key；诊断通知无节流；状态区无 Action | fcitx5-rime `processComposeString` / 60s 放行窗 / StatusArea | compose 委托宿主；诊断类通知节流；中英/简繁状态进 StatusArea（复用宿主机制） | P2 |
| `IMP-15` | 数据与生态 | 词源单一（jieba），两字词是「分词统计频」非输入法真频；THUOCL 被 sources.toml 预排除 | rime-ice 词库上游盘点（其 §4.7 三档许可） | sources.toml 增华宇野风（PD）/THUOCL（MIT，ADR 解除预排除）/melt 英文词表（Apache-2.0）；多音字自动校验进 dictc | **P1** |
| `IMP-16` | 数据与生态 | 无英文词段（仅临时英文直通） | rime-ice melt_eng 融合机制 | dictc 英文词段 + 词格合并一路英文 translation + `initial_quality` 式档位 | P2 |
| `IMP-17` | 数据与生态 | 无错音错字提示 | rime-ice corrector.lua 思路（表自建） | 输入读音 vs 词条标准读音比对 → annotation 通道提示；错字映射表自建（pypinyin 机器校验） | P2 |
| `IMP-18` | 数据与生态 | 无符号/emoji 面板（features.md 2.05.05 规划位） | rime-ice v 模式（71 类 symbols） | `v` 前缀触发 + 符号表数据 + 既有候选列表通道产出（不做新面板 UI，工艺归 opt-ui.md 轨道） | P2 |
| `IMP-19` | 工程暗线 | 排序质量改动无可回归的期望文本基线；多音字无自动校验 | rime-ice smoke TSV + polyphone.go（K-6） | `xtask testd-engine` 补 TSV 数据驱动冒烟（key_sequence→expected_text）+ 多音字校验脚本 | **P1** |
| `IMP-20` | 工程暗线 | 词库替换需重启（features.md 3.02.04 规划位） | —（本项目自有规划） | 校验通过后 `ArcSwap` 原子切换 + 在途会话保旧库 + 失败保旧报 `dict/swap/rejected` | P2 |

---

## 6. 战略取舍与反膨胀剪裁边界（Anti-Bloat）

以下能力在对标项目中存在，但**坚决不予照搬**。每项给出拒绝理由，防止规划执行期的范围蠕变：

| 对标能力 | 拒绝理由 |
|---|---|
| librime Registry/Module/动态插件机制（`engine.cc:309-400`、`plugins/`） | IME 场景能力集固定，9 crate 静态组合已覆盖；动态管线只增加攻击面、破坏可预算性（0.4 规则 9） |
| octagram / 万象 n-gram 模型 | 零网络约束下需用户自备 500MB 级模型文件，违背嵌入式部署行（0.5.2）；bigram 段（IMP-03）是同方向的第一步 |
| librime-lua 及 rime-ice 的 23 个 Lua 滤镜 | 全部可用「dictc 编译期数据 + Rust 静态滤镜」表达，更安全可预算；引入脚本引擎违背零脚本攻击面取向（features.md §1 结构性差异） |
| LevelDB / 每 commit 写库 + 3 秒事务回滚 | redb 已满足且纯 Rust；rspinyin 的批量延迟落盘（32 键/2000ms/4096 条）+ 崩溃窗口语义比 librime 更轻更明确 |
| schema 可组合管线（多方案共存/切换） | rspinyin 无多方案需求；配置已覆盖双拼/模糊音开关。等真实需求出现再评估（rime-comparison §5 同结论） |
| rime-ice 词库/Lua/映射表直接取数 | **整仓 GPL-3.0**，直接复制引入 GPL 义务；正确路径是 `sources.toml` 白名单从许可干净的原始上游取数（华宇 PD / THUOCL MIT / Unihan 已接入） |
| 腾讯词向量词表（约 98 万 3+ 字词） | 许可存疑（Credits 称 CC BY 3.0，聚合站标 CC BY-NC 4.0，官方页已下线）；核实原始条款（archive.org 存档）之前不引入，其覆盖可由华宇 + THUOCL 补足 |
| 多端同步（`UserDbMerger` max-tick 合并） | features.md 3.02.05 已结论「默认不实现」；tick 逻辑时钟设计会被 IMP-01 吸收，但同步协议不做 |
| librime Menu 惰性归并 / 每 keyevent 重建 CandidateList | rspinyin 一次解码全量 45 候选在预算内（decode ≤3ms），惰性化是负优化；差量刷新的复杂度由 opt-ui.md 轨道单独评估 |
| GPU 渲染路径 | opt-ui.md / features.md 3.04.01 域，且软件光栅实测未超预算；不在本增量域 |

---

## 7. 与既有规划文档的边界

- **`docs/dev/features.md`**（权威规范）：其 §5.3/§5.4 的 Phase 2/3 索引（34 + 16 条）与本报告 IMP 有大量对应关系（模糊音=2.02.01、简拼=2.02.02、纠错=2.02.03、衰减=2.03.01、热替换=2.03.04、bigram=3.02.01、上下文排序=3.02.03）。features-add 簇**不共用其编号**（X-5），映射表见 features-add.md §6.2。
- **`docs/dev/opt-ui.md`**（UI 工艺轨道，28 项缺陷）：候选窗视觉/材质/微交互全部归它。本规划涉及 UI 的任务（置顶高亮、符号候选、annotation 提示）**只走既有通道与主题 token，不做工艺改造**；两文档冲突时以 features.md 为准（opt-ui.md 头部同约定）。
- **`docs/dev/tests.md` + `tests/`**：端到端测试套件；IMP-19 的冒烟用例集是对它的数据补充，不重复其结构。
