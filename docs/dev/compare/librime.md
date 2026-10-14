# librime 核心算法与存储设计深度调研报告

> 来源: 2026-10-14 四路并行调研之子代理原始报告,全文存档未删改 ｜ 调研对象: `remarks/librime` ｜ 交叉对比结论见 [rime-comparison.md](rime-comparison.md) ｜ 行号引用以调研当日工作区为准

调研范围:`/home/gong/news/rspinyin/remarks/librime`(忽略 deps/ 与 build/)。该 checkout 中 plugins/ 仅有动态插件加载器,**不含 octagram**;Grammar 组件接口存在但无人注册(详见 §7)。

---

## 1. Pipeline 与组件注册

### 1.1 四级流水线的串联

Engine 是纯编排器,持有 6 条组件数组:`processors_ / segmentors_ / translators_ / filters_ / formatters_ / post_processors_`(`src/rime/engine.cc:56-61`)。

**一次按键的完整调用链**(以 script_translator 为例):

1. 前端调用 `Session::ProcessKey` → `Engine::ProcessKey`(`engine.cc:106-132`):顺序遍历 processors,`kAccepted` 立即返回,`kRejected` 中断;未被消费的键推入 `commit_history`(`engine.cc:119`)并发出 `unhandled_key_notifier`(`engine.cc:130`)。
2. 拼音键由 **speller** 处理(`gear/speller.cc:100-146`):校验字符属于 `speller/alphabet` 或 delimiter 后,`ctx->PushInput(ch)`(`context.cc:65-75`)。PushInput 末尾触发 `update_notifier_`——**这是整条链的驱动核心:processor 只改 input 字符串,翻译由通知机制异步唤起**。
3. `update_notifier_` 连接到 `ConcreteEngine::OnContextUpdate` → `Compose(ctx)`(`engine.cc:134-138, 164-179`)。
4. `Compose` → `CalculateSegmentation`(`engine.cc:181-211`):循环调用 segmentors 的 `Proceed()`;abc_segmentor 用 alphabet/initials/finals 贪心划出 `[j,k)` 段并打 `abc` tag(`gear/abc_segmentor.cc:39-69`);无进展即停;每段是 `Segment{start,end,tags,menu,status}`(`segmentation.h:18-58`)。
5. `TranslateSegments`(`engine.cc:213-243`):对每个 status<kGuess 的段,new 一个 `Menu`,逐个调 `translator->Query(input, segment)`,非 exhausted 的 translation 挂进 Menu;再把 `AppliesToSegment` 的 filter 挂到 Menu(`engine.cc:234-238`)。
6. **Menu 是惰性归并器**(`menu.cc:11-37`):`MergedTranslation` 按 quality 惰性拉取多路翻译的堆顶,`AddFilter` 把 filter 包在 translation 链上;只有翻页/选词时才 `Prepare(n)` 真正取候选。
7. 候选生成细节见 §2/§3;选词后 `OnSelect`(`engine.cc:269-292`)推进 composition;commit 时 `Memory::OnCommit` 把 CommitEntry 写入用户词典(`gear/memory.cc:54-56, 106-111`)。

### 1.2 组件注册与发现:三级机制

1. **Registry(全局组件表)**:名字 → `ComponentBase*` 的 map(`registry.h:17-32`,实现 `registry.cc:13-45`)。组件通过 CRTP `Class<T,Arg>::Component::Require(name)` 查找(`component.h:20-38`),`Create(ticket)` 实例化。
2. **Module(静态模块分组)**:`RimeModule` 由 `RIME_REGISTER_MODULE` 宏定义初始化函数;`ModuleManager` 按名加载(`module.h:18-37`)。`setup.cc:45-46` 把 `"default"` 组注册为 `core+dict+gears`。各模块的注册点:
   - core:config_builder/config/schema/user_config(`core_module.cc:19-43`)
   - dict:dictionary、user_dictionary、reverse_lookup_dictionary、userdb(LevelDb)、corrector 等(`dict/dict_module.cc:22-43`)
   - gears:全部处理器/切分器/翻译器/过滤器(`gear/gears_module.cc:40-94`,含 `r10n_translator` 等 alias,`:75`)
3. **配置驱动的实例化**:`InitializeComponents` 读 schema 的 `engine/processors|segmentors|translators|filters` 列表,构造 `Ticket{engine, type, prescription}`,`T::Require(klass)` + `Create(ticket)`(`engine.cc:309-400`;`@namespace` 语法在 Ticket 解析)。实例自带计时日志(`engine.cc:329-342`,此仓库加了 ComponentTiming)。
4. **动态插件**:`plugins/plugins_module.cc:35-70` 用 boost::dll 加载 `rime-*.so`,按文件名找同名 module 并 `LoadModule`——octagram 等即以这种方式注入 `grammar` 组件。

一个典型 schema 的管线配置见 `data/minimal/luna_pinyin.schema.yaml:39-68`。

---

## 2. 音节切分(syllabifier):Prism 支撑的「从任意中间位置开始的模糊匹配」

### 2.1 数据结构:SyllableGraph

`algo/syllabifier.h:40-46`:

```cpp
struct SyllableGraph {
  size_t input_length, interpreted_length;
  VertexMap vertices;        // 位置 -> 该位置最优 SpellingType
  EdgeMap edges;             // start -> (end -> SpellingMap: SyllableId -> EdgeProperties)
  SpellingIndices indices;   // start -> (SyllableId -> [EdgeProperties*])  反转索引
};
```

即一个**以字符位置为顶点、音节为边、边上携带「该拼写串映射到的所有音节 id + 属性」的 DAG**。`EdgeProperties = SpellingProperties + ambiguous_source_positions`(`syllabifier.h:24-29`);`SpellingProperties{type, end_pos, credibility, tips, is_correction}`(`algo/spelling.h:24-35`)。

### 2.2 走查算法(`algo/syllabifier.cc:31-275`)

- **顶点驱动 + 优先队列**(非简单 DP):`VertexQueue = priority_queue<pair<pos,SpellingType>, greater>`(`:20-22`),保证同一位置**拼写类型更优的顶点先入图,后到者丢弃**(`:47-53`)——这是「从任意中间位置开始」的关键:每个被确认的顶点都重新出发做一次前缀搜索。
- **边生成**:对每个顶点,取剩余输入:
  - 常规路径:`prism.CommonPrefixSearch` 一次拿到所有「以当前位置为起点的音节前缀命中」(`:87`;darts 双数组 `commonPrefixSearch`,`dict/prism.cc:279-286`)。
  - 有 canonicalizer(配置了 `translator/canonicalize` 重排类运算)时,前缀单调性被打破,退化为**逐长度步进 + 精确查询** `prism.GetValue`(`:71-86`)。
  - 有 corrector 时,先记录精确命中音节,再 `corrector_->ToleranceSearch(prism, input, &corrections, 5)`(`:89-106`)。
- **模糊展开**:每个 match 的 value 是 spelling id,经 `prism.QuerySpelling()` 的 `SpellingAccessor` 枚举它映射的**全部** SyllableId 及属性(`:126-155`)——模糊音/简拼一个键串对应多音节就在这里展开。`strict_spelling_` 时整串输入不允许纯 fuzzy/abbrev 匹配(`:130-132`)。
- **类型传播与容差分级**:边上的属性按 type 取最小;路径类型向下兼容(顶点类型取 max,`:148-166`)。类型序:`kNormalSpelling < kFuzzySpelling < kAbbreviation < kCompletion < kAmbiguousSpelling`(`spelling.h:15-22`)。误差代价是**对数信用分(credibility)**而非 edit distance 阈值:
  - fuzzy/abbrev:各 −log 0.5 = −0.693(`calculus.cc:14-15`)
  - completion:−log 0.05 = −2.996(`syllabifier.cc:28`)
  - correction:−log 0.01 = −4.605(`syllabifier.cc:29`、`calculus.cc:16`)
- **切分歧义处理**:
  - 多路切分天然保留(DAG 并行边);
  - `CheckOverlappedSpellings`(`:277-309`):若存在跨接边 Z=YX,则把 Y|X 中间顶点标记 `kAmbiguousSpelling`(注释例:"niju'ede"),让成句阶段可对歧义路径加罚;
  - 反向修剪(`:174-222`):从 farthest 顶点回扫,删掉「不连通到最优终点」或「类型劣于可达路径」的边/顶点(fuzzy 对失效免疫,`:178-179`);
  - **尾部补全**:`enable_completion_` 且未消费完时,`prism.ExpandSearch`(BFS 限 512 个,`prism.cc:288-329`)把剩余串当成「可能的前缀」生成 kCompletion 边(`:224-265`)。
- **Transpose**(`:311-321`):把 edges 反转成 `indices`(按 SyllableId 排序)——这是给用户词典 LevelDB **顺序扫描**用的(§5)。

### 2.3 传给 translator 的东西与查表

script_translator 在 `ScriptTranslation::Evaluate`(`gear/script_translator.cc:460-517`)里:`consumed = BuildSyllableGraph(*dict->prism())`,然后 `dict->Lookup(syllable_graph, 0, …)`、`user_dict->Lookup(...)`。`Dictionary::Lookup` → `Table::Query`(§4)按图走查;`UserDictionary::DfsLookup` 按 indices DFS。另有一个 `syllabify_dfs`(`script_translator.cc:50-74`)用于给具体词条反解字符区间(preedit/音节分隔 Spans、纠错判定 `IsCorrection` `:379-415`)。

**纠错配置**:`translator/enable_correction`(默认关)→ `Corrector::Require("corrector")`(`script_translator.cc:193, 207-211`);correction 候选最多 4 个(`max_corrections_`, `:173`)。注意当前 build 里 `CorrectorComponent::Create` 实际只返回键盘邻位的 `NearSearchCorrector`,edit-distance 版被 `#if 0` 禁用(`dict/corrector.cc:302-322`)。

---

## 3. 候选成句(composition/poet)

### 3.1 词图与权重

- `WordGraph = map<int, map<int, DictEntryList>>`(`gear/poet.h:20`):start → end → 候选词列表。由 `PrepareForMakingSentence`(`script_translator.cc:705-723`)构建:对音节图的**每个顶点**再查一次词典/用户词典,每处每码长最多保留 `translator/max_homophones` 个(`EnrollEntries`,`:688-703`)。
- 行权重 = 词权重之和 + 语法上下文分:`candidate.weight + Grammar::Evaluate(context, entry->text, entry->weight, is_rear, grammar_)`(`poet.cc:215-219`)。`Grammar::Evaluate`(`gear/grammar.h:18-28`)在 grammar 为空时返回常数 `log(1e-6) ≈ −13.81` 惩罚——**常数意味着无 LM 时所有路径同减,排序退化为纯词频(一元)求和**。

### 3.2 两种策略:有 LM 用 Beam Search,无 LM 用 DP

`Poet::MakeSentence`(`poet.cc:246-253`):`grammar_ ? BeamSearch : DynamicProgramming`。

- **DynamicProgramming**(`poet.cc:170-189`):每个 end 位置只保留一条最优 Line——这是**标准 Viterbi**(一元权重的最短路径),比较器为 Poet 构造时的 `compare_`(script_translator 用默认 `CompareWeight`,`script_translator.cc:199`;table_translator 用 `LeftAssociateCompare`:词数少优先、词长序列字典序,`table_translator.cc:228`、`poet.cc:93-109`)。
- **BeamSearch**(`poet.cc:134-168`):状态是 `hash_map<string /*last_word*/, Line>`——**按"最后一个词"分桶是因为二元文法的上下文就是前一个词**;每步从各桶取 top-`kMaxLineCandidates=7`(`:137`)条延伸。所以是「per-end-position × per-last-word 的束搜索」,不是全局 k-shortest。
- **多句输出** `MakeSentences`(`poet.cc:258-353`):`max_sentences>1` 时启用,`beam_width = max_sentences*3`,按 text_hash 去重、按权重降序截断;输出端用 **cutoff 阈值**控制:相邻句相对权重差 > `sentence_cutoff_threshold`(默认 0.1)即停,且阈值每收一句乘以 `(1−1/max_sentences)` 加速收敛(`:325-343`)。配置入口:`translator/max_sentences`、`translator/sentence_cutoff_threshold`(`gear/translator_commons.h:179-180`)。
- 附加规则:整串被单个词覆盖的候选被显式排除(`poet.cc:207-208`、`:276-277`),保证整句输入时仍给出分词结果。
- 成句的**触发条件**(`script_translator.cc:489-514`):音节 ≥2 且没有「可靠的全码精确匹配」(系统词与用户词都算)时才成句;否则只出词条。

### 3.3 无 octagram 时的行为

本仓库 `Grammar::Require("grammar")` 找不到组件(`poet.cc:74-79`),`grammar_` 为空:
1. `MakeSentence` 走 DynamicProgramming = **一元词频 Viterbi**;
2. `ContextualWeighted` 直接原样返回 translation(`poet.h:53-55`),无上下文重排;
3. 候选的最终 quality = `exp(entry->weight) + initial_quality + quality_len/full_code_length`(`script_translator.cc:653, 671`),其中 `entry->weight = log(词频) − log(1e8) + credibility`(`dict/dictionary.cc:163-164`)——**credibility 把 fuzzy/abbrev/completion/correction 的惩罚直接加进 log 域**。

---

## 4. 词库存储:Table 与 Prism

### 4.1 基础设施:MappedFile 与 OffsetPtr

所有二进制文件都走 `dict/mapped_file.h`:自研的 bump allocator(`Allocate`,`:134-152`)+ **相对指针** `OffsetPtr<T>`(相对自身地址的 int32 偏移,0=NULL,`:21-51`)+ 定长头 `Array`/`List`(`:60-78`)。运行时 `OpenReadOnly()` 整文件 mmap,**零反序列化**:所有查询都是指针在映射内存上的 chase。

### 4.2 `.table.bin`:音节索引 → 词组链

格式(`dict/table.h:79-92`):`Metadata{format "Rime::Table/4.0", dict_file_checksum, num_syllables, num_entries, syllabary, index, string_table}`。

- **Syllabary**:`Array<StringType>`,StringType 是 `String|StringId` union(`:34`),v4 起文本统一存入 **marisa-trie 的 StringTable**(`dict/string_table.h:23-56`;写入 `table.cc:244-257`,读取 `:259-263`)——词条文本去重共享。
- **Entry**(`table.h:42-45`):`{ StringType text; float weight }`。**词频以 log 域 float 存储**:编译期 `e->weight = log(原始词频)`(`dict_compiler.cc:257`);运行时取词时 `weight − log(1e8) + credibility`(`dictionary.cc:163-164`)。LongEntry 额外带 `extra_code`(超出索引深度的音节 id 列表,`:47-50`)。
- **四级索引**(核心设计,`table.h:54-77`):
  - `HeadIndex`:按下**第一个** SyllableId 直接寻址的数组;
  - `TrunkIndex`:后续层,按 SyllableId 排序的数组 + 二分查找(`find_node`,`table.cc:143-150`);
  - 每层节点:`{该音节的同音词 Entry 列表, next_level}`;
  - 码长 ≤ `kIndexCodeMaxLength = 3` 个音节的词在 trunk 层结束;**更长的词进入 TailIndex**(`Array<LongEntry>`,extra_code 存第 4 个音节起的剩余码),运行时用 `match_extra_code` 在音节图上递归匹配剩余码(`dictionary.cc:92-123`)。
  - 查询入口 `TableQuery::Advance/Access/Backdate`(`table.cc:102-217`),沿层级走并累积 credibility 与 quality_len。
- **`Table::Query`(把音节图与词表联合走查,`table.cc:571-630`)**:BFS 队列持 `(输入位置, TableQuery 状态)`;对图上每条边(音节)先 `Access` 取词,若未到 interpreted_length 且层数<3 则 `Advance` 继续。歧义罚 `kPenaltyForAmbiguousSyllable=−3.0` 已写好但被 `if (false)` 显式停用(`:601-608`)。

### 4.3 `.prism.bin`:拼写 → 音节 id 的双数组 trie + 多键展开表

格式(`dict/prism.h:36-50`):`Metadata{format "Rime::Prism/5.0", dict_file_checksum, schema_file_checksum, num_syllables, num_spellings, double_array(_size), spelling_map, alphabet[256], max_key_length}`。

- **trie 主体**:darts-clone 双数组(`include/darts.h`),key=拼写串(可含 delimiter),value=spelling id;构建后整块 image memcpy 进 mmap(`prism.cc:165, 206-213`),加载时 `trie_->set_array(array, size)` 直接引用(`:117-125`)。
- **SpellingMap(多键展开的物化)**:`Array<SpellingMapItem>`,item 为 `List<SpellingDescriptor{syllable_id, type(位30=is_correction), credibility(float), tips}>`(`prism.h:25-34`;is_correction 打包在 type 高位,`prism.cc:27-28, 241-245`)。即 **spelling id → 一组(音节 id, 属性)**——模糊音「一个键串=多个音节」、简拼、纠错全在这张表里表达;`SpellingAccessor`(`prism.cc:40-80`)惰性枚举。
- 查询 API:`GetValue`(精确)、`CommonPrefixSearch`(切分热路径)、`ExpandSearch`(BFS 补全,用 metadata 里的 alphabet 限分支,`prism.cc:288-329`)。

### 4.4 编译与缓存(dict_compiler)

`dict_compiler.cc:80-208`:
- 校验和缓存:`compute_dict_file_checksum`(`:64-78`)对全部 .dict.yaml + 预设词表算 checksum;已有 `.table.bin` 的 checksum 不符才重建表(`:104-118`);`.prism.bin` 额外比对 **schema 文件 checksum**(`:119-125`)——所以改 speller/algebra 只会重编 prism,不会重编 table;`.reverse.bin` 校验失败会连带强制重建表(`:129-138`)。
- `BuildTable`:EntryCollector 收集词→构建 Vocabulary(按 code 分层)→ `vocabulary.SortHomophones()` → `table->Build`(`:216-278`)。
- `BuildPrism`(`:296-367`):从表里取回 syllabary,加载 schema 的 `speller/algebra`,`Projection.Apply(&script)` 展开,传给 `prism_->Build(syllabary, &script, …)`。
- 产物写到 staging_dir(用户 build 目录,`setup.cc:73-80`),与 prebuilt_data_dir 分离。

---

## 5. 用户词典 UserDict

### 5.1 存储层

- `Db` 抽象(`dict/db.h:26-76`):有序 KV + `DbAccessor{Reset/Jump/GetNextRecord}`(前缀顺序扫描)+ Meta 键 + 事务/恢复接口。实现:`userdb`=**LevelDb**(`dict_module.cc:31`;`level_db.h:35`),`plain_userdb`=文本 TSV,`stabledb/tabledb` 供 custom_phrase 用。
- **key = `音节串(空格分隔尾随空格) + '\t' + 词文本`**(`user_dictionary.cc:431`;`user_db.cc:69` 注释)——按音节字典序排列,配合 `SyllableGraph::indices` 的有序性做**一次前缀顺序扫描**完成整句查词(`user_dictionary.cc:193-214` 的长注释解释了为什么需要 backdating 及其限制)。
- **value = `c=<commits> d=<dee> t=<tick>`**(`user_db.cc:22-26`)。

### 5.2 调频数学(algo/dynamics.h)

全局 tick 存于 meta `/tick`(`user_dictionary.cc:459-466`),**每次上屏一个词条 +1**(`:447`)。

- 衰减:`formula_d(d,t,da,ta) = d + da·exp((ta−t)/200)`(`dynamics.h:6-8`)——历史 commit 按 tick 常数 200 指数衰减求和(≈139 tick 半衰期);
- 概率映射:`formula_p(s,u,t,d)`(`:10-15`)把衰减和映射到 (0,1) 的使用概率, d≥20 时超线性增长;
- 读:`CreateDictEntry`(`user_dictionary.cc:535-574`):先把 dee 衰减到当前 tick(`:551`),`dee ≤ 1e-200` 直接丢弃(`:533, 554`),`weight = log(formula_p(0, commits/present_tick, present_tick, dee)) + credibility`;
- 写:`UpdateEntry`(`:425-457`):commits>0 时 `dee = formula_d(commits, tick, dee, v.tick)` 并 tick+1;commits=0 是「句内成分更新」(k=0.1);commits<0 负数标记删除(可复活)。

### 5.3 对排序的影响

`ScriptTranslation::PrepareCandidate`(`script_translator.cc:598-680`):同码长时用户词优先(纠错结果除外,`:626-633`);系统词全码精确匹配在首位之前压制用户长词联想(`:634-642`);最终 quality 统一为 `exp(weight)+initial_quality+quality_len/full_len`。词组里的单字也会被回写(`UpdateElements`,`:331-349`)。

### 5.4 事务与同步

- 每次翻译会话包一层事务:`Memory::StartSession/FinishSession`(`gear/memory.cc:94-104`)→ LevelDb WriteBatch(`level_db.cc:303-326`);3 秒内可回滚(`user_dictionary.cc:495-502`)。
- 打开失败时把 `userdb_recovery_task` 丢给 Deployer 后台线程(`:169-183`)。
- 多端同步:`UserDbMerger`(`user_db.cc:187-244`)把双方 dee 各自衰减到 max_tick 再取 max——tick 是逻辑时钟,不是墙钟。

---

## 6. 拼写运算 Algebra

### 6.1 表示

- `Script = map<string, vector<Spelling>>`(`algo/algebra.h:20-27`):「音节串 → 它展开出的所有(拼写串, 属性)」。
- `Projection::Apply(Script*)`(`algebra.cc:107-140`)按公式逐轮变换:**运算命中时,原映射保留(deletion=false 时)、新拼写追加(addition=true 时),属性按 `Compose` 累积**——「derive 产生冗余键、erase 删键、xform 替换键」的语义全靠 addition/deletion 两个虚函数区分。

### 6.2 运算集(`algo/calculus.h/cc`)

| 运算 | 类 | 属性效果 |
|---|---|---|
| `xlit/a/b/` | Transliteration(逐码字符映射,`calculus.h:41-48`) | 无 |
| `xform/x/y/` | Transformation(boost::regex 替换) | 无 |
| `erase/x/` | Erasion(regex 整体匹配删除) | 删除键 |
| `derive/x/y/` | Derivation | 追加键 |
| `derive/…/abbrev` | Abbreviation → `kAbbreviation`, −log0.5(`calculus.cc:238-245`) |
| `fuzz/x/y/` | Fuzzing → `kFuzzySpelling`, −log0.5(`:214-221`) |
| `derive/…/correction` | Correction → `is_correction`, −log0.01(`:249-256`) |
| `reorder/zyx/dedup` | Reorder(键内字符重排/去重,`:260-370`) | 无(触发 canonicalizer 的逐长查询模式) |

解析器按「第一个非小写字母字符」当分隔符切分(`calculus.cc:32-46`)。luna_pinyin 的实际用例(abbrev zh/ch/sh 简拼 + 8 条 correction 纠错)见 `data/minimal/luna_pinyin.schema.yaml:70-87`。

### 6.3 编译到 Prism

部署期 `dict_compiler.cc:311-327`:syllabary 逐个 `script.AddSyllable`(恒等映射)→ `Projection.Apply(&script)` 展开 → `prism_->Build(syllabary, &script)`。Build 时 trie 的 key 是**拼写串**(script 的第一维),value 是 spelling id;SpellingMap 把 spelling id 连到全部目标音节 id(`prism.cc:215-256`)——**运行时零运算**:所有 fuzz/abbrev/correction 都已物化成 trie 边 + 属性表。另有运行期小 Projection(canonicalize)只用于切分阶段的串重写(§2.2)。

---

## 7. 语言模型接口与 octagram

- **接口**:`gear/grammar.h:11-28`。`Grammar : Class<Grammar, Config*>`,唯一虚方法 `double Query(const string& context, const string& word, bool is_rear)`(返回 log 域加分);静态 `Evaluate` 封装「无 grammar 时 −13.8155」。它是普通组件,由 `Grammar::Require("grammar")` 发现(`poet.cc:74-79`)。
- **本仓库状态**:plugins/ 下只有加载器(`plugins/plugins_module.cc`),没有 octagram;`dict_module.cc`/`gears_module.cc` 均未注册 `"grammar"`。因此如 §3.3 所述,poet 退化为无上下文 DP。
- **消费点**(上游 octagram 挂上后自动生效的三处):
  1. 成句:`Poet::MakeSentenceWithStrategy<BeamSearch>`,bigram 上下文 = `Line::context()`(前两个词,`poet.cc:53-59`)或 preceding_text(跨句,`script_translator.cc:325-329`);
  2. 多句:同上,beam 3×max_sentences;
  3. 上下文重排:`ContextualTranslation`(`gear/contextual_translation.cc:35-52`)对已生成的候选逐个用 `Grammar::Evaluate(preceding_text, …)` 重打分、组内重排(每轮最多 32 个,`kContextualSearchLimit`)。由 `translator/contextual_suggestions` 开关(`translator_commons.h:121-122, 175`)。
- octagram 本体(上游 librime-octagram)不在此 checkout,无法核实其数据结构;从接口看它只需实现 Query(context, word, is_rear) 即可, presumed 为 blgram 三阶插值 n-gram(mmap)。

---

## 8. Reverse lookup 与反查

- **`.reverse.bin`**(`dict/reverse_lookup_dictionary.h:23-33`):`Metadata{dict_file_checksum, dict_settings(YAML), index(List<StringId>), key_trie, value_trie}`——key(词文本)与 value(音节串列表)各是一棵 **marisa StringTable** image,index[key_id]=value_id。构建:遍历 Vocabulary 得 `word → set(syllable)`,另存 `stem` 键(`"\x1fstem"` 后缀)用于 UniTE 造词(`reverse_lookup_dictionary.cc:84-124`);查询 O(marisa Lookup + 数组下标)(`:71-82`)。
- **引擎层支持是「组件拼装」而非内置功能**:
  1. `recognizer` 用正则把特定输入打上 tag(如 `reverse_lookup: ``[a-z]*'?$``,luna_pinyin.schema.yaml:153-159);
  2. `reverse_lookup_translator`(`gear/reverse_lookup_translator.cc:92-180`)监听该 tag:输入侧用一个 Dictionary(可以是另一方案的 prism+table)做 `LookupWords`(predictive,limit 100)或切分后 TableQuery;目标侧用 `reverse_lookup_dictionary`(**默认指向 translator/dictionary**,`/target` 可改,`:133-137`);候选 comment 显示反查码(`:56-74`),且在排序上压过 sentence/completion(`:76-90`)。首次使用才加载(`:147-148`)。
  3. `affix_segmentor` 提供前缀/后缀段(`P:`、`` ` `` 等),`reverse_lookup_filter` 反向地给主翻译候选**加注**反查码(gears_module.cc:89)。
- 注意:**反查只依赖两张静态文件,与用户词典无关**——这也是 table_translator(形码)与 script_translator(音码)能共用 `Memory`/`Dictionary` 基础设施的原因。

---

## 9. 性能与工程细节

- **部署期缓存**:以内容 checksum(diff .dict.yaml / schema.yaml)驱动的增量重建(§4.4);产物版本号不满足即强制重编(`prism.cc:110-115`、`table.cc:291-297`);`kDump` 选项可把 table/prism 反导出为文本供调试(`dict_compiler.cc:229-232, 351-355`)。
- **运行时热路径**:
  - 无互斥锁——引擎、词典、切分全程单线程无锁;唯一的锁在 Service 的 session 表(`service.cc:161`)与 Deployer 任务队列(`deployer.h:71-73`)。
  - 惰性求值贯穿始终:Menu 不预取(`menu.cc:26-37`),`DictEntryIterator` 用 `partial_sort` 逐个吐最优 chunk(`dictionary.cc:135-141`),`LazyTableTranslation` 分页拉取(table_translator.cc:141-190)。
  - 分配较多:每次 Query new ScriptTranslation/collector/chunk,候选是 shared_ptr(`common.h:58-79` 的 an/of/the);词条文本每次从 marisa 拷出成 std::string(`table.cc:228-230`)——mmap 换来了零启动成本与低 RSS,但牺牲了零拷贝读取。
- **mmap vs 预加载**:prism/table/reverse 全部 mmap 只读、页按需调入(Darts/marisa 均原地引用);LevelDB 自带 mmap 选项;唯一的「预加载」是 syllabary→id 的 `rev_syllabary_` 缓存(user_dictionary.cc:517-520)。
- **线程模型**:前端(ibus/fcitx 等)每 session 一个引擎,单线程驱动 ProcessKey;后台只有 Deployer 的 `std::async` 工作线程跑部署任务与 userdb 恢复(`deployer.cc:106-121`、`user_dictionary.cc:174-179`);组件间通信用 boost::signals2(`common.h:81-82`)。Table/Prism 实例在 `DictionaryComponent` 用 weak_ptr 缓存共享(dictionary.h:116-117),userdb 用 `db_pool_` 共享(user_dictionary.cc:580-593)。

---

## 总结:librime 在算法层比「DAG 切分 + 词频排序」多出的核心能力,及其数据结构前提

| # | 能力 | librime 的做法(引用) | 数据结构前提(rspinyin 若引入需具备) |
|---|---|---|---|
| 1 | **拼写运算编译期展开**(模糊音/简拼/纠错/缩写) | speller/algebra → Projection → Script → Prism SpellingMap(§6、§4.3) | 键串 trie 的 value 不直接是词条,而是 **spelling id → (音节id, type, credibility) 列表** 的间接层;rspinyin 无 Prism,需先补这个「拼写层/音节层」分离 |
| 2 | **对数信用分容差体系** | fuzzy −0.69 / abbrev −0.69 / completion −3.0 / correction −4.6,随边存储,进 log 域求和(§2.2、§3.3) | 每条切分边携带可加的 credibility;候选质量用 `exp(log词频 + credibility)` 融合,而非词频单键排序 |
| 3 | **带补全的任意位置模糊切分** | 每顶点重起 CommonPrefixSearch + ExpandSearch(512 上限)+ 反向修剪(§2.2) | 双数组(或 FFT/FST)trie 的 `commonPrefixSearch` + 前缀展开枚举;jieba 词表需先按音节串建 trie(35 万词条 → 音节数只有 ~420 个,音节 trie 极小) |
| 4 | **任意长度词组的 O(音节数) 查表** | Table 四级索引 head/trunk/tail + extra_code 尾匹配,3 音节内直达(§4.2) | 分层索引或「前 3 音节哈希 + 长词补表」;rspinyin 现在每键全表过滤的模式无法扩展到整句 |
| 5 | **用户词频 + 时间衰减调频** | commits/dee/tick + formula_d/p(§5.2);顺序扫描协同靠 SyllableGraph::Transpose | 有序 KV(或等价 mmap 排序表)+ 逻辑 tick 时钟;权重进 log 域与系统词统一比较 |
| 6 | **n-gram 成句接口(可插拔)** | Grammar::Query + Poet 双策略:有 LM 束搜索(宽 7,按 last_word 分桶)/ 无 LM 一元 Viterbi + cutoff 多句(§3) | WordGraph(start→end→词列表)与「按上下文增量打分」的抽象;**rspinyin 即使不上 octagram,「无 LM=一元 Viterbi」这一半就能把 73.8% 的 top1 显著抬升**——当前「单词频排序」等价于贪心,而 Viterbi 解决的是整句词频和最大化 |
| 7 | **成句候选与词条候选的统一队列** | ScriptTranslation 按「可靠精确匹配优先,否则句子优先」交替出候选(§2.3、script_translator.cc:502-514) | 候选层需要「码长分组 + 来源合并」的多路归并,而非单一排序列表 |
| 8 | **反查/辅助查询** | tag 驱动的 recognizer+affix_segmentor+reverse_translator/filter + marisa 反查文件(§8) | 词→码的反向索引(可直接从 jieba 词表 + 拼音标注导出) |
| 9 | **组件化 + 配置驱动管线** | Registry/Module/插件(§1.2) | 无数据结构前提,纯架构;是以上能力可插拔(如 octagram 独立成插件)的原因 |

**对 rspinyin 最具性价比的三步**(依据上述引用):
1. **补一元 Viterbi 成句**(poet.cc:170-253 的 DP 逻辑,~100 行即可移植):在无 LM 时把 `Grammar::Evaluate` 退化为常数,这正是 librime 自身的默认形态;
2. **为词表建音节级索引**(前 3 音节直达 + 尾码表),替代全量扫描,使整句路径查询可行;
3. **引入 credibility 字段**到切分边(哪怕只有 fuzzy/completion 两级),让简拼、未打完的输入有合理排序而不是被词频淹没。

> 存档备注:总结表中第 4/6 条对 rspinyin 现状的描述("每键全表过滤""单词频排序等价于贪心")是本子代理的推测,与 rspinyin 现状盘点报告不符——rspinyin 实际为 FST 整码查询 + K-best Viterbi(beam 16)整句首选,解码骨架并不落后;这两条的准确差距见 [rime-comparison.md](rime-comparison.md) §2。
