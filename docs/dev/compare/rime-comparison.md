# rspinyin 与 Rime 技术栈对比分析(librime / fcitx5-rime / rime-ice)

> 文档版本: v1.1(v1.1 增补 §4.7 上游词库源盘点,并按核实结果修正腾讯词向量的许可表述) ｜ 调研日期: 2026-10-14 ｜ 调研方式: 四路并行代码级深读(子代理逐文件核对,引用均带文件与行号) ｜ 参考对象: `remarks/librime`、`remarks/fcitx5-rime`(5.1.17)、`remarks/rime-ice`(约 2026-03, GPL-3.0)
>
> 结论摘要: rspinyin 在工程面(预算门禁、零网络、脱敏、崩溃自愈、XTEST 测试平台、mmap 零拷贝)显著领先于三个参考仓库;解码骨架(DAG 切分 + K-best Viterbi 整句)与 librime 无语言模型形态同构,并不落后。真正的差距集中在四处——**词库规模与分层策略**(35 万单源 vs 190 万分层多源)、**语言模型**(bigram 空段 vs n-gram 可插拔)、**拼写容差体系**(无纠错、credibility 分级不完整)、**用户词频衰减与置顶**。桥接层最大的可借鉴点是 fcitx5-rime 用 2700 行借到的平台服务清单(会话共享粒度、切输入法收尾、compose、capability 协商、状态区 Action)。

---

## 1. 三层参照系与 rspinyin 的对应关系

| 参考仓库 | 层级 | rspinyin 对应物 |
|---|---|---|
| librime(C++17) | 核心算法引擎 | `ime-core`(切分/打分/会话)+ `ime-dict`(容器)+ `xtask/dictc`(编译器) |
| fcitx5-rime(C++, 约 2,700 行) | 桌面桥接层 | `ime-fcitx5`(引擎 cdylib)+ `ime-ui`/`ime-ui-addon`(自绘候选窗 cdylib) |
| rime-ice(YAML/Lua) | 词库与方案数据层 | `data/sources.toml` 白名单 + jieba 词表 + dictc 管线;`phrases.tsv`;无 schema 概念 |

三层之间 rspinyin 有一条结构性差异贯穿始终:**librime/rime-ice 把大量智能放在"部署期编译 + 运行期解释"(YAML algebra、Lua 滤镜、schema 管线),rspinyin 把等价逻辑放在"dictc 编译期物化 + Rust 静态滤镜"**。这个取向本身是对的(零脚本攻击面、无 GC、可预算),借鉴时应当"借策略、不借机制"。

---

## 2. 核心算法引擎:ime-core / ime-dict vs librime

### 2.1 切分与候选生成

| 维度 | librime | rspinyin 现状 | 差距评估 |
|---|---|---|---|
| 切分结构 | `SyllableGraph`:顶点驱动 + 优先队列,每确认顶点重新前缀搜索;`SyllableGraph::Transpose` 生成反查索引(`algo/syllabifier.cc`) | `SyllableDag`:字节偏移节点 + SmallVec 边表 + 反向可达剪枝,切分歧义完整保留(`segment/dag.rs:80-157`) | 同为 DAG,平手 |
| 切分容差 | 每条边带 `credibility` 对数信用分:fuzzy/abbrev −log0.5、completion −log0.05、correction −log0.01(`calculus.cc:14-16`) | 简拼有 `ABBREV_PENALTY_Q8`,每分段有 `seg` 罚(`lm/score.rs:171-257`);无 completion/correction 级 | **缺纠错与补全的分级容差** |
| 尾部补全 | `ExpandSearch` BFS(512 上限)生成 kCompletion 边,未打完的输入也能出补全候选 | 无(必须整码命中) | 缺"边打边上屏"能力,依赖 `prefix` 查询 |
| 查词路径 | 音节图与 Table 联合走查(`Table::Query`,`table.cc:571-630`) | 每 span 拼整码查 `Lexicon::lookup`,DAG 全切分 + Viterbi 排序(`viterbi/lattice.rs`) | 平手,但受 2.3 索引限制 |

### 2.2 排序与成句(语言模型)

- **librime**:`Poet` 双策略——有 Grammar 时按"最后一个词"分桶的束搜索(宽 7,`poet.cc:134-168`);无 Grammar 时退化为**一元 Viterbi**(`poet.cc:170-189`)+ `max_sentences` 多句输出 + cutoff 阈值收敛(`poet.cc:258-353`)。Grammar 是可插拔组件(`gear/grammar.h`),octagram 插件注入 n-gram。另有 `contextual_translation` 对已生成候选按上文重排(每轮 ≤32 个)。
- **rspinyin**:单次 K-best Viterbi,beam 默认 16,**整句即首选**(`viterbi/mod.rs:193` 有钉死测试);5 权重整数 Q8.8 打分(uni/bi/len/seg/user,`score.rs:171-179`);同文本候选合并加分(`mod.rs:323-373`)。
- **差距**:rspinyin 的 Viterbi 骨架与 librime 无 LM 形态等价甚至更强(beam 更大、全整数可复现)。真正的差距是 **`DictLm::bigram` 退化为 `unigram-64`**(`dict_lm.rs:28-36`,BIGRAM 段 v1 为空)——即打分上限被钉死在一元;以及没有"上文重排"(选完一个词后对剩余候选按上下文重打分)。

### 2.3 词库存储与索引

- **librime `.table.bin`**:四级索引——`HeadIndex`(首音节直接寻址)+ `TrunkIndex`(≤3 音节二分)+ `TailIndex`(`extra_code` 存第 4 音节起的剩余码,运行时在音节图上递归匹配);词频以 **log 域 float** 存储(`dict_compiler.cc:257`);marisa StringTable 文本去重。`.prism.bin`:darts 双数组 trie,key=拼写串,value=spelling id,`SpellingMap` 把 spelling id 展开为 `(音节id, 类型, credibility)` 列表(`prism.h:25-34`)——**拼写层与音节层分离**是多键展开的核心机制。
- **rspinyin**:FST 键=音节串(`'` 连接),值=WORDLIST `(start, count)` 间接层;`MAX_WORDS_PER_KEY=32` 截断;词条 16B 定长 + STRPOOL 零拷贝借 mmap(`fst_index.rs:157-183`);编译期按 weight 降序写 WORDLIST,查询即排名。
- **差距**:①rspinyin **`Lexicon::prefix` 返回 `Unsupported`**(`fst_index.rs:401-403`)——真实词库不支持前缀枚举,直接堵死简拼、词补全、尾部补全三条路;②同键 32 词截断是硬上限,重码多的键(如两字高频音组合)会丢词;③无 log 域词频(线性 u32,大词频差被 Q8.8 压缩,长尾区分度弱)。rspinyin 的零拷贝与加载校验(逐段 CRC)比 librime(marisa 每次拷出 string)更好,这点不用学。

### 2.4 用户词频

- **librime UserDict**:LevelDB 有序 KV,key=`音节串\t词`,value=`c=commits d=dee t=tick`;**指数衰减** `dee += da·exp((ta−t)/200)`(半衰期约 139 次上屏,`algo/dynamics.h:6-8`),概率映射 `formula_p` 后进 log 域与系统词统一比较;负 commits 标记删除;事务 + 3 秒回滚;多端同步取 max tick。
- **rspinyin UserDb**:redb,`(commits, last_used_ms)`,打分 `min(log2(1+freq)/8, 2)×0.8`(`score.rs:250-256`);内存 delta 批量落盘(32 键/2000ms/4096 条触发)比 librime 每次 commit 写库更轻;50 万上限淘汰已有。
- **差距**:**无时间衰减**——freq 单调增,多年前的老词与本周的词同权,这是真实使用中最先被感知的排序劣化;`is_user_word` 恒 false、无造词写路径(`user_db.rs:694-698`)。批量落盘、readonly 降级、备份三代这些 rspinyin 更好,保留。

### 2.5 拼写代数(模糊音 / 简拼 / 双拼 / 纠错)

- **librime**:部署期 `Projection.Apply` 把 algebra(erase/derive/xform/xlit/fuzz/abbrev/correction/reorder)物化进 Prism,**运行时零运算**;correction 由 `NearSearchCorrector`(键盘邻位)运行期补充,correction 候选上限 4。
- **rspinyin**:模糊音 8 类运行期扩边**全部已实现**(`fuzzy.rs`);简拼有独立代数 + lattice walk(`segment/abbrev.rs` + `lattice/abbrev.rs`);双拼五套表 + 自定义表完整落地;**纠错完全没有**(无 corrector 文件)。dictc 的 L3b"频率定向多键展开"(top 50k、cap 4、增幅 30%)本质是选择性的部署期物化,与 librime 同思路。
- **差距**:引擎侧能力大多在,但**生产链路够不着**——模糊音没有任何配置键、会话只设 SHUANGPIN 位(`state/machine.rs:454`);简拼 `engine.abbrev` 配置键存在但标注 Not wired(`ime-config/src/reload.rs:125-127`),且真实词库 prefix 未实现使 walk 静默跳过(`lattice/abbrev.rs:145`)。纠错是唯一需要新写的引擎能力,而 rspinyin 已有 fuzzy 扩边基建,邻位键纠错=同类机制加重罚,成本低。

### 2.6 反查与辅助查询

librime 的反查是"tag 路由(recognizer)+ affix_segmentor + reverse_lookup_dictionary(marisa 词→码反向索引)+ reverse_lookup_filter"的组件拼装,`.reverse.bin` 与用户词典无关。rspinyin 完全没有反向索引;若要做辅码/拆字反查,dictc 编译期从 jieba 词表导出 REVERSE 段即可,不需要 librime 那套 tag 管线。

### 2.7 差异总表

| 能力 | librime | rspinyin | 结论 |
|---|---|---|---|
| 切分歧义 | DAG 保留 | DAG 保留 | 平手 |
| 整句成句 | 一元 Viterbi(无 LM)/束搜索(有 LM) | K-best Viterbi beam16,整句首选 | 骨架平手,LM 是差距 |
| 语言模型 | Grammar 插槽 + octagram n-gram | unigram 真查表 + bigram 空段 | **落后**(可渐进) |
| 上下文重排 | contextual_translation | 无 | 落后 |
| 拼写纠错 | 键盘邻位 + credibility −log0.01 | 无 | **落后**(基建已有) |
| 尾部补全/词补全 | completion 边 + enable_word_completion | 无(prefix 未实现) | 落后 |
| 模糊音/简拼/双拼 | 部署期物化 | 运行期扩边/双拼完整,但模糊音简拼**未接线** | 引擎平手,接线落后 |
| 用户词频衰减 | 指数衰减 + 逻辑时钟 | 无衰减 | **落后** |
| 用户造词 | Memory::OnCommit 自动 | is_user_word=false,Pin Unsupported | 落后 |
| 词库索引 | 音节级四级索引 | FST 整码 + 32 词截断 + 无 prefix | **结构性落后** |
| mmap 零拷贝 | marisa 拷出 string | WordRef 借 mmap | **rspinyin 领先** |
| 加载校验 | checksum 比对 | 逐段 CRC + quarantine 自愈 | **rspinyin 领先** |
| 确定性 | 未作为契约 | 字节指纹测试钉死 | **rspinyin 领先** |

---

## 3. 桌面桥接层:ime-fcitx5 / ime-ui vs fcitx5-rime

### 3.1 架构差异

fcitx5-rime **完全不自绘 UI**(全库无一行 X11/Wayland/canvas):往 `InputPanel` 塞数据模型(preedit Text + CandidateList 接口对象)后 `updateUserInterface` 通知 classic-ui 渲染;每个 key event 完整重建 CandidateList("无状态视图")。rspinyin 走的是自绘路线(Override-redirect + 四档 Wayland 阶梯 + UIPriority=10 接管 + 失败回退 ClassicUI),这是产品定位差异而非优劣——自绘换来了主题/动效/网格布局与焦点零抢占的结构保证,代价是把 InputPanel 的渲染消费者换成自己。**两条路线不可比,但 fcitx5-rime 借用的平台服务清单值得逐项核对。**

### 3.2 fcitx5-rime 借用的平台服务清单 vs rspinyin 覆盖情况

| # | 平台服务(fcitx5-rime 用法) | rspinyin 现状 | 建议 |
|---|---|---|---|
| 1 | **会话共享粒度**:`RimeSessionPool` 按 `PropertyPropagatePolicy` 三档(每 IC/同程序/全局),并联动 fcitx5 全局 `shareInputState`,策略切换时快照+重建(`rimesession.cpp:104-134`) | 每输入上下文独立 `Context{session,...}`,session id 分域;未实现程序级/全局共享 | **强烈建议补**:同程序窗口间保持输入状态是真实体验痛点;可用 `program` 级 Context 池实现 |
| 2 | **切输入法收尾策略四档**:Clear / 提交原文 / 提交 preedit / 提交预览,再统一 reset(`rimeengine.cpp:464-484`) | 未见等价机制 | 建议:切换走时至少可配"提交未上屏编码" |
| 3 | **compose/dead key**:委托 `processComposeString`,组合字符回喂或直接 commit(`rimestate.cpp:170-180`) | 无 | 欧洲键盘布局用户需要;成本低 |
| 4 | **preedit capability 协商**:不支持 Preedit 的应用自动落回 aux 显示(`rimestate.cpp:349-352`) | 有 `client_preedit` 配置,未见 capability 检查 | 建议:按 IC capability 自动降级,而非只靠用户配置 |
| 5 | **状态区 Action/菜单**:schema 菜单 + option 联动的 Toggle/Select Action,标签取自 rime schema `states/`,插件零硬编码文案;option 变化以气泡提示(`rimeaction.*`) | 有 CheatSheet overlay;未见 StatusArea Action/图标 | 建议:中英/简繁/双拼状态进 StatusArea,切换有提示 |
| 6 | **部署/同步入口**:菜单按钮 + 热键 + setSubConfig 虚路径;`deploy_config_file` 按 `config_version` 版本联动增量部署 | 宿主 reloadConfig 驱动;词库热替换未实现(features 3.02.04 规划中) | 方向一致,按原规划推进 |
| 7 | **跨线程通知编组**:EventDispatcher 投回主循环 + mainThreadId 同步快路径(`rimeengine.cpp:527-554`) | 引擎→UI 走 C ABI wire + SPSC/eventfd,语义表完整 | rspinyin 方案更严格,不学 |
| 8 | **ABI 兼容三板斧**:RIME_STRUCT data_size 运行时探测、pkg-config 版本宏、core 带版本依赖 | `#[repr(C)]` 逐位钉死 + `core:5.1.0` 地板 + OptionalDependencies | 已覆盖,平手 |
| 9 | **通知节流**:60s 放行窗 + 部署后 30s 静默(`rimeengine.cpp:62-63, 594-607`) | 未见 | 建议:诊断通知类加节流 |
| 10 | **惰性候选**:BulkCandidateList 全局第 idx 个惰性拉取 + maxSize 学习(`rimecandidate.cpp:96-134`) | 一次解码全量 45 候选,预算内 | 不需要;但"选中动作=合成翻页键回喂引擎复用语义"(`rimecandidate.cpp:75-86`)的思路对翻页/高亮统一键语义有参考 |
| 11 | **key release 位 30 / 修饰掩码裁剪 / CapsLock 监听** | 修饰位逐位钉死 + auto_english_on_uppercase | 已覆盖 |

### 3.3 rspinyin 独有且领先的(保持)

焦点零抢占(WM_HINTS input=False + 永不 SetInputFocus,结构性保证)、XTEST FocusGuard 焦点窃取检测、帧镜像/上屏读回/截图三通道真窗口测试、沙盒会话 20/20 实证、wayland 四档降级阶梯(代码在、待真机验证)、主题对比度门与 Spring 动效。这些在三个参考仓库中均无对应物。

---

## 4. 现代词库:ime-dict / dictc vs rime-ice

### 4.1 词库构成与规模

| | rime-ice | rspinyin |
|---|---|---|
| 总量 | ~192 万条中文(8105 字表 8,758 + 41448 大字表 46,023 + base ~543k + ext ~339k + tencent ~982k + others ~900) | 348,972 条 / 292,315 键 / 15.62MiB(jieba 词表单源) |
| 字表 | 《通用规范汉字表》8105 精校字频(北语 25 亿字语料)+ Unihan 16.0 大字表可选 | pinyin-data.tsv 单字读音 |
| 结构 | **五文件分层,按用途与质量分级** | 单一容器,层仅用于多音字展开(L1/L3a/L3b/L3c) |
| 英文 | ~2.5 万词 + 8 份中英混合词库(1,011 词×8) | 无(仅临时英文直通) |
| 反查 | radical_pinyin 13.2 万条部件码表 | 无 |

### 4.2 词频策略——最值得抄的工程决策

rime-ice 是**三层归一化**:base(两字词为主)保留真实语料词频(1 ~ 1,926 万,跨 7 个数量级);ext/tencent(3+ 字词,~132 万条)**全部压平为 100**,只靠"是否收录"不靠词频区分;字表用真实字频。词长刻意保持梯度(两字词均值 25,488 > 三字 7,058 > 四字+ 3,928),"短词优先"由数据保证而非引擎逻辑。

rspinyin 的 jieba 词频是单一真实分布,没有"核心真频 / 长尾平权"的分层——长尾词频噪声直接进入排序,这正是 73.8% top1 的重要损耗源之一。

### 4.3 多音字

- rime-ice:**字表字频比 5:100 阈值**作为自动注音开关(`的 de 76,938,354 vs di 106,917`,只有高频读音参与无注音词条的自动注音;难区分的 `长 chang/zhang` 全部手动注音),多音字词条集中放 ext 手动注音,tencent 词库禁止含多音字,配 Go 校验脚本(`others/script/rime/polyphone.go`)机器把关。
- rspinyin:L3c polyphone.tsv(2,587 行)纠音 + L3b 定向多键展开,方向相同。rime-ice 的"高频比阈值自动决定展开范围"可以直接吸收进 dictc 的 L3b 规则,把"cap 4 / 增幅 30%"的手工参数变成数据驱动的阈值。

### 4.4 中英混输

melt_eng 机制 = 同一码段多翻译器并行 + `initial_quality` 融合(自定义短语 99 ≫ 拼音 1.2 > 英文 1.1 > 中英混合 0.5)+ 拼写派生(英文词派生大小写变体、`2→er/liang`、`+→plus/jia`)。rspinyin 的等价路径:dictc 增加英文词段 + 打分权重,或在词格合并一路英文 translation;临时英文直通保留。

### 4.5 辅助功能面(rime-ice 有 / rspinyin 无)

v 模式符号 71 类、emoji 4,857 行映射、错音错字提示(~250 行映射 + comment 通道)、辅码(`` 拼音+`+部件)、以词定字、日期/农历/金额大写/计算器/Unicode/UUID、7 套双拼共用一份词库(prism 分编译)、英文自动大写、长词优先、置顶词。**其中大部分是"触发前缀 + 查表/计算"或"候选序列后处理",不需要引擎改造**——rspinyin 的 phrases.tsv($Y/$M/$D 变量)、annotation 显示位、CheatSheet overlay 通道都是现成挂载点。

### 4.6 数据工程实践

rime-ice 有超出一般词库仓库的构建纪律:Go 工具链(注音校验/多音字校验/排序去重/emoji 生成校验)、`make lint`(yamllint+luacheck)、**基于 `rime_api_console` 的端到端冒烟测试**(TSV 数据驱动:key_sequence→expected_text,支持 @today/@regex)、AGENTS.md 里写死的词库维护约束(新词默认词频 100、两字词必须进 base、tencent 禁多音字)。rspinyin 的 dictc 已有 sha256 pin、双编译一致性、6,000 条留出集 quality 门,可补的是"多音字自动校验"和 testd-engine 数据驱动的 key_sequence→expected_text 冒烟(基建现成)。

### 4.7 上游词库源盘点(词库扩充可从 rime-ice 回溯到的原始数据源)

前提:rime-ice 整仓 GPL-3.0,**不能直接搬运它的任何 dict.yaml/Lua/映射表**;但它的 `others/docs/Credits.md` 与各词库文件头注释完整登记了上游来源——正确路径是**从这些原始上游取数**,经 dictc 的 `data/sources.toml` 白名单(id/spdx/sha256 pin/layer)逐源登记。以下按许可状态分三档(2026-10-14 核实):

**A. 许可干净,可直接接入 sources.toml**

| 来源 | 许可 | 规模/内容 | 建议 rspinyin 用法 |
|---|---|---|---|
| 华宇野风系统词库(thunisoft 论坛帖) | **Public Domain**(Credits.md 确认) | 真实语料词频,两字词主力(rime-ice base 的第一来源) | **最高优先**:替换/增强 jieba 的两字词层,两字词从"分词统计频"升级为"输入法场景真频"——top1 提升的主力 |
| THUOCL 清华开源词库(github.com/thunlp/THUOCL) | **MIT** | 按领域分文件:IT/财经/饮食/法律/汽车/医学/地名/历史名词等,合计 ~20-30 万条,新闻语料统计频 | 3+ 字长尾层扩充 + 专业领域词库叠加(features.md 3.02.02 规划项正好用得上) |
| Unihan `kMandarin`(unicode.org/Public) | **Unicode License V3** | 全字库读音(4 万+ 字) | 生僻字层读音;补 pinyin-data 未覆盖的字,对齐 rime-ice 的 41448 大字表思路 |
| mozillazg/pinyin-data(github.com/mozillazg/pinyin-data) | **MIT** | 单字读音+声调 | **已在用**(L1 单字读音),保持 |
| jieba 词表(github.com/fxsjy/jieba) | **MIT** | 34.9 万条 | 已在用,保留作长尾底座 |
| rime-melt 英文词 ~1,000 条(github.com/tumuyan/rime-melt) | **Apache-2.0**(Credits.md 确认) | 混输场景精选英文词 | 中英混输(P3-16)起步词表;只取词表数据 |

**B. 需核实许可,暂缓引入**

| 来源 | 状态 | 说明 |
|---|---|---|
| **腾讯词向量词表**(~1200 万词,取 3+ 字子集约 98 万条) | **许可存疑** | rime-ice Credits.md 称 **CC BY 3.0**,但 OpenDataLab 等数据聚合站标注 **CC BY-NC 4.0(禁商用)**,且官方下载页(ai.tencent.com/ailab/nlp/embedding.html)2025 年起已下线(重定向元宝),社区镜像不携带原始条款。rspinyin 词库随 IME 面向公众分发,若 NC 属实则不可用。**结论:核实原始条款(archive.org 存档)之前不引入**;其两字词覆盖大部分可由华宇+THUOCL 补足 |
| 现代汉语常用词表(国家语委,indiejoseph gist 转录) | 许可不明 | 出版物转录;~5.6 万词与 jieba/THUOCL 高度重叠,收益低,不建议 |
| 北语 25 亿字语料字频表(邢红兵,BLCU) | 学术发布,许可不明 | 8105 字表的字频来源;若未来要精校字频需先核实条款;当前 jieba 单字频够用,低优先 |
| 通用规范汉字表(教育部) | 政府标准文本,转录仓库无明确许可 | 只用它定义"8105 字符集范围"(事实性数据),字频仍取自其他源 |
| google-10000-english 20k 词表(rime-ice 英文库主来源) | 仓库无 LICENSE 文件,数据源自 Google Trillion Word Corpus | 英文库若启动需先核实;替代:dwyl/english-words(Unlicense)、SCOWL |

**C. GPL/LGPL——不可直接取数,自建替代**

| 来源 | 许可 | 自建替代路径 |
|---|---|---|
| 简化字八股文 rime-essay-simp | LGPL-3.0 | 数据静态编译进二进制的 LGPL 合规成本高;rspinyin 简繁走 `ime-core/script.rs` 内置种子表,无需它 |
| rime-radical-pinyin 拆字码表(13.2 万条) | GPL-3.0 | 辅码/拆字反查若做,自建部件拆字表:Unicode IDS(表意文字序列,Unicode License V3)+ CHISE IDS;工程量中等,优先级低 |
| rime-ice 自建 corrector 错音表 / emoji.txt / lunar.db | GPL-3.0(随仓) | 错音表自建(用 pypinyin(MIT)做机器校验);emoji 用 Unicode CLDR annotations(Unicode License V3);农历用公农历换算算法(MIT crate)生成 |

**接入路径小结**(映射到 dictc 现有管线):
1. `sources.toml` 增条目并标 layer:L1 = pinyin-data(已有)+ Unihan;L3a 两字词 = 华宇(真频)∪ jieba;L4 长尾 = THUOCL ∪ jieba(3+ 字);`fetch.sh` 必须可复现下载 + sha256 pin + SPDX 登记,`check-dict` 白名单审计同步更新。
2. 与 P0-① 分层归一化配合:两字词保真频、3+ 字长尾压平,归一化规则进 dictc 权重层。
3. 预期收益:词表 35 万 → 约 55-65 万条,且两字词质量显著提升——这比单纯堆量更直接作用于 73.8% 的 top1。
4. 多音字纪律(吸收 rime-ice 做法):从上游引入的新词条过 L3c 纠音 + "字表字频比阈值"自动校验脚本,不达标词条降层或压平。

---

## 5. 可借鉴改进清单

### P0 —— 直击 top1 73.8% 的排序质量(核心瓶颈)

| # | 改进 | 来源 | 关键点 |
|---|---|---|---|
| 1 | **词库分层:核心真频 + 长尾平权** | rime-ice 三层归一化 | 两字词保真频,3+ 字长尾压平到常数带;词长梯度由数据保证;在 dictc 加一层归一化即可,容器格式不变 |
| 2 | **用户词频时间衰减** | librime `formula_d` | redb value 加 dee(float)/tick 字段;指数衰减半衰期 ~139 次上屏;log 域进打分。防"老词永久占位" |
| 3 | **bigram 段启用** | librime poet(无 LM=一元 Viterbi 的下限已达到) | BIGRAM 段槽已保留;数据可从 jieba 语料或分词统计离线生成;Q12 概率 + 现有 5 权重中的 `bi=0.6` 直接生效 |
| 4 | **置顶词接线** | rime-ice pin_cand_filter | `user_meta.pinned` 字段已存在,`PinHighlighted` 引擎侧转正:置顶=权重极限提升,成本最低的"用户可感知"改进 |

### P1 —— 已有能力接线(引擎在,只差主机链路,性价比最高)

| # | 改进 | 现状 | 所需工作 |
|---|---|---|---|
| 5 | **模糊音配置接线** | `fuzzy.rs` 8 类全实现,无配置键、会话不设位 | ime-config 加键 + machine.rs 设 FUZZY 位,~1 天量级 |
| 6 | **简拼接线** | 代数+walk 完整;`engine.abbrev` 键在;卡在 `Lexicon::prefix` Unsupported | 先实装 prefix(FST range query)或 walk 改整码枚举 |
| 7 | **简繁转换接线** | `ime-core/script.rs` + `ScriptIndex` 都在,commit 路径未走 | commit 前挂 transform,配置键已接受 |
| 8 | **标点/全角接线** | `punct_mode`/`full_width` 配置键"接受但未接线" | 补 host 侧 transform |
| 9 | **会话共享粒度** | 每 IC 独立 Context | program 级 Context 池(参考 fcitx5-rime 三档策略),联动宿主 shareInputState |

### P2 —— 结构性升级(中等成本,打开新能力面)

| # | 改进 | 来源 | 关键点 |
|---|---|---|---|
| 10 | **`Lexicon::prefix` 实装**(解锁:简拼、词补全、尾部补全) | librime completion 边 + enable_word_completion | fst::Map 改 fst::Set/Map range + WORDLIST 分层;这是 6/11 的前置 |
| 11 | **拼写纠错(键盘邻位)** | librime NearSearchCorrector + rime-ice ~30 条 derive | 复用 fuzzy 扩边机制 + correction 重罚(等价 credibility −log0.01);先做邻位替换/键序颠倒,不做 edit distance |
| 12 | **音节级索引改造**(重码截断) | librime 四级索引 | 短期:MAX_WORDS_PER_KEY 按权重智能截断 + 超出转长词补表;长期:前 3 音节直达 + extra_code |
| 13 | **上文重排** | librime contextual_translation | 先做"上一 commit 词作 bigram 上下文"的重打分;配合 #3 |
| 14 | **词库热替换** | — | 已在 features 3.02.04 规划,ArcSwap |

### P3 —— 功能面扩充(数据为主,引擎挂载点现成)

| # | 改进 | 来源 | 挂载点 |
|---|---|---|---|
| 15 | 词源扩充:THUOCL(MIT)、华宇野风(PD)、Unihan(Unicode V3) | rime-ice 词库上游 | `sources.toml` 白名单直接加,逐源 SPDX+sha256 pin;**腾讯词向量许可存疑暂缓**;**从源头取数,不从 rime-ice 派生**——逐源清单见 §4.7 |
| 16 | 中英混输 | melt_eng | dictc 英文段 + initial_quality 式融合权重 |
| 17 | 错音错字提示 | corrector.lua(自建表) | annotation 显示位已有 |
| 18 | 符号面板(v 模式)/ emoji | symbols_v.yaml / OpenCC emoji(自建,可用 Unicode CLDR) | phrases/overlay 通道;emoji 面板已在 2.05.05 规划 |
| 19 | 切输入法收尾策略、compose、capability 协商、通知节流 | fcitx5-rime | ime-fcitx5 host 层,逐项小改 |
| 20 | 数据驱动端到端冒烟(key_sequence→expected_text) | rime-ice smoke | `xtask testd-engine` 基建现成,补 TSV 用例与多音字校验脚本 |

### 不建议照搬

- **librime 组件注册/动态插件机制**:rspinyin 9 crate 静态组合即可,IME 场景固定,动态管线只增加攻击面。
- **octagram/万象模型**:网络禁令下需用户自备 500MB 级模型文件;先走 #3 bigram 段。
- **Lua 脚本引擎**:rime-ice 的 23 个 Lua 滤镜全部可以用 Rust 静态滤镜 + 编译期数据表达,更安全可预算。
- **LevelDB**:redb 已满足且纯 Rust;librime 的"每 commit 写库 + 事务"反而比 rspinyin 的批量延迟落盘重。
- **schema 可组合管线**:rspinyin 无多方案共存需求,配置已覆盖双拼/模糊音等开关;等真有多方案需求再评估。

### 许可合规警示

rime-ice 整仓 GPL-3.0。其词库各来源许可不同(华宇 PD、THUOCL MIT、腾讯词向量**许可存疑**——Credits 称 CC BY 3.0 但聚合站标 CC BY-NC 且官方页已下线,见 §4.7),但**直接复制 rime-ice 的 dict.yaml/lua/corrector 映射表会给 rspinyin 引入 GPL 义务**。正确路径是"从许可干净的原始上游取数"(dictc 的 sources.toml 白名单机制就是为此设计的),错音错字表与 emoji 映射自建或采用 Unicode CLDR 等自由许可数据。

---

## 6. rspinyin 已领先、保持不动的方面

1. **工程门禁**:预算断言(budgets.json)、零网络断言、`check-unsafe` 白名单、alloc-count 分配预算——三个参考仓库均无。
2. **隐私与鲁棒**:双层脱敏(第一道防线是代码不传内容)、panic/SIGBUS/SIGSEGV 自愈、quarantine 损坏隔离、readonly 降级。
3. **解码确定性**:无 HashMap、generation 计数器决胜、100 次字节指纹一致测试;librime 未将确定性作为契约。
4. **mmap 零拷贝**:WordRef 借 mmap vs librime/marisa 每次拷出 std::string。
5. **真窗口测试**:XTEST + FocusGuard + 帧镜像 + 上屏读回三通道,远超 rime_api_console 冒烟的覆盖面。
6. **配置工程**:修复式校验、v1→v2 迁移、模板与默认值防漂移测试;rime 的 custom.yaml 是纯手工约定。
