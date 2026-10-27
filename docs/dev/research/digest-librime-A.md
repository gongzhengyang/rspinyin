# librime 勘察摘要（SCOUT-A · 引擎簇）

> 对标仓库: `/home/gong/news/rspinyin/remarks/librime`（checkout @ 7bc3fb00, 2025 上游主线）
> 形态: C++17 跨平台输入法核心库，无 UI、无主循环、同步调用驱动；管线组件全部插件化。
> 方法: 静态源码精读（read + grep + wc），未编译未运行。己方对照: rspinyin = FST+mmap 词库 + K-best Viterbi + Q8.8 定点。

## ① 目录拓扑与核心架构

### 1.1 四类管线组件的虚接口契约

四类组件均继承 `Class<T, const Ticket&>`（`src/rime/component.h:20-38`），经全局 `Registry`（字符串名 → 工厂）用 schema 配置装配。`Ticket = {engine, klass, name_space}`（`ticket.h`）携带引擎指针与配置命名空间。

| 组件 | 头文件 | 纯虚签名 | 语义 |
|---|---|---|---|
| Processor | `src/rime/processor.h:30` | `ProcessResult ProcessKeyEvent(const KeyEvent&)` | 键处理；返回 `kRejected`(交还OS)/`kAccepted`(吞掉)/`kNoop`(传给下一个)（18-22 行枚举） |
| Segmentor | `src/rime/segmentor.h:24` | `bool Proceed(Segmentation*)` | 向 Segment 标注区间+tag；返回 false 截断链 |
| Translator | `src/rime/translator.h:28` | `an<Translation> Query(const string& input, const Segment&)` | 输入串→候选流（惰性生成器，可为 null） |
| Filter | `src/rime/filter.h:28-31` | `an<Translation> Apply(an<Translation>, CandidateList*)` + `bool AppliesToSegment(Segment*)` | 包裹/改写候选流（装饰器叠加） |

跨层边界：`Engine::TranslateSegments`（`engine.cc:213-243`）把每个 translator 的 `Translation` 塞进 `Menu::AddTranslation`，再把 filters 依次 `menu->AddFilter(filter)` —— **Filter 的输出即下一个 Filter 的输入，最终流被 Menu 消费**。Translator 候选进入 Filter 的唯一通道就是 `Translation` 迭代器接口，无具体候选数组跨层传递。

跨栈不变量：管线组件是「同构迭代器上的装饰器栈」，顺序由配置而非代码决定；新增能力=注册新组件名+改 schema 配置，零改引擎。

### 1.2 Engine/Context/Composition/Segment 状态流转

- `Context`（`context.h:19-118`）持有 `input_`、`caret_pos_`、`composition_`（= `Segmentation`，`vector<Segment>`）、`commit_history_`、options/properties 两张 map；对外暴露 7 个 signals：commit/select/update/delete/abort/option_update/property_update/unhandled_key。
- `Segment`（`segmentation.h:18-58`）状态机：`kVoid → kGuess → kSelected → kConfirmed`；带 `tags`(set<string>)、`menu`（候选菜单）、`selected_index`。
- 主循环（单线程同步，`engine.cc`）：`ProcessKey` 顺序过 processors → 未处理键推入 `commit_history` → post_processors → `unhandled_key_notifier`。任何 processor 改动 input 都会触发 `update_notifier` → `Engine::Compose(ctx)`（164-179 行）：
  1. `comp.Reset(active_input)`——**只重算 caret 之前的部分，已确认段保留在 comp 里**（增量失效边界）；
  2. `CalculateSegmentation`（181-211 行）：while 未完成 → 依次调 segmentors `Proceed` → 无进展则停；caret 之后的输入**只允许一个 segment**（200 行注释）；
  3. `TranslateSegments`（213-243 行）：跳过 `status >= kGuess` 的段（**已翻译段不重译**），为每个新段建 `Menu` 聚合各 translator。
- `OnSelect`（269-292 行）：选词后 `seg.Close()`；末段到输入尾则 `kConfirmed` + `_auto_commit` 选项决定直接上屏或前进空段；否则 `Forward` + `Compose`。
- `Segment::Reopen`（`segmentation.cc:26-40`）：caret 未越过段尾时 `kSelected→kGuess` **复用旧 menu（候选缓存）**，否则 `kVoid` 全失效。

跨栈不变量： composition 是「已确认前缀 + 单个活动段」的增量失效模型；重算粒度 = 段，缓存粒度 = 段内 menu。

### 1.3 Prism（拼写运算/模糊音索引）磁盘格式与内存布局

`src/rime/dict/prism.h:21-102`。本质：**darts 双数组 trie**（vendored `deps/darts-clone`）+ 自定义 `SpellingMap`。

- Metadata（36-50 行）：`format[32]`、`dict_file_checksum`、`schema_file_checksum`、`num_syllables/num_spellings`、`double_array_size`、`OffsetPtr<char> double_array`、`OffsetPtr<SpellingMap> spelling_map`、`alphabet[256]`、v5 增 `max_key_length`。
- 关键设计：trie 的 value 是 **spelling_id**；`SpellingMap` 是 `Array<List<SpellingDescriptor>>`，`SpellingDescriptor = {syllable_id, type(bit30=is_correction), credibility, tips}`（25-31 行）。**一个键串（拼写串）→ N 个音节 ID 的多对多展开**：模糊音/简拼/纠错全部编码为同一 trie 里的不同 key，用 `type` 区分（kNormalSpelling < kAbbreviation < kFuzzySpelling < kInvalidSpelling，`algo/spelling.h`）。
- 查询面：`CommonPrefixSearch`（前缀下行）/`ExpandSearch`（带 limit 的补全搜索）/`QuerySpelling(id)`（拿该 spelling 展开出的音节集）。

跨栈不变量：拼写层与词库层解耦的点是 `SyllableId`；模糊音不是查询时规则运算，而是**建库时展开成独立键的多值索引**，查询零规则开销。

### 1.4 Table（词表）磁盘格式与内存布局

`src/rime/dict/table.h:26-94` + `mapped_file.h`。**不是 trie，是「4 级音节 ID 索引 + 崩溃安全的自定义 arena mmap」**：

- 底座 `MappedFile`（`mapped_file.h:84-169`）：文件即 arena，`Allocate<T>` 线性 bump 分配、容量不足倍增 `Resize`；**所有结构间引用用 `OffsetPtr<T>`（int32 自相对偏移，0=NULL）**（21-51 行）——文件内容位置无关，`mmap` 后零重定位零拷贝直接读。`Array<T>={size,T at[1]}`、`List<T>={size,OffsetPtr<T>}`、`String={OffsetPtr<char>}` 是三大容器原语。
- 索引四级（`table.h:54-77`）：`HeadIndex`（按 syllable_id 直接寻址的数组，第 1 键）→ `TrunkIndex`（{key,entries,next} 有序数组，第 2、3 键）→ `TailIndex`（`LongEntry{extra_code,entry}` 数组，≥4 键时前 3 键入 trunk、余下编码存 `extra_code` 查询时逐位验证）。**`Code::kIndexCodeMaxLength = 3`**（`vocabulary.h:27`）。
- `Entry = {StringType(text:str|str_id), Weight(float)}`；v2 起 `Metadata` 挂 `string_table`（**marisa-trie**，`string_table.h:23-56`）：长词条文本去重后以 `StringId` 引用，短文本仍内联 `str`（`table.cc` AddString 按权重决定）。
- 查询：`TableQuery::Access/Advance/Backdate`（`table.h:137-181`）在四级间游走；`Table::Query(syll_graph,...)`（`table.cc:571-630`）从 SyllableGraph 出发 BFS，沿边拿 `TableAccessor`（指向 mmap 内 entries 数组的游标）。

跨栈不变量：词条物理存放于其编码的最浅索引层，长码存 `extra_code` 尾验证——索引深度封顶使深码查询退化为「浅索引 + 尾码线性验证」，mmap 永不拷贝。

### 1.5 UserDB 存储引擎与 schema

- 引擎：**LevelDB**（`dict/level_db.cc`，注册名 `"userdb"`）或纯文本 TextDb（`"plain_userdb"`，快照/同步用）`dict/dict_module.cc:28-31`。读迭代器显式 `options.fill_cache=false`（level_db.cc:24）避免污染 block cache。
- 记录：key = `编码串 + '\t' + 词条文本`；value = `"c=<commits> d=<dee> t=<tick>"`（`user_db.cc:22-25` UserDbValue::Pack）。`tick` 为全局逻辑时钟（每次提交学习 +1，meta `/tick`），`dee` 是衰减积累量。
- 衰减动力学（`algo/dynamics.h:6-15`）：`formula_d: d' = d + da·exp((ta−t)/200)`（半衰期≈200 tick 的指数记忆）；`formula_p` 用 commits/tick/dee 拟合出 0~1 概率再取 `log` 为词重。查询时惰性衰减（`user_dictionary.cc:551`：读到旧 tick 先折旧），**写入只在新学习时发生**——读路径承担衰减计算，存储永不回写全表。
- 删除=软删：`commits` 置负（`user_dictionary.cc:443-456`），查询跳过；BackSpace 触发 `DiscardSession` 回滚未提交事务（`memory.cc:151-160`）。
- 多机合并 `UserDbMerger::Put`（`user_db.cc`）：双方 dee 各自折算到 `max_tick` 后 `dee=max(o.dee,v.dee)`、`commits=|max|`——**按 tick 归一后的 CRDT 式 max 合并**，无冲突解决逻辑。
- 事务：`Transactional`（`db.h:75-86`）+ `Memory::StartSession/FinishSession/DiscardSession` 包住一次上屏的学习。

跨栈不变量：用户词频状态压缩为 (commits, dee, tick) 三元组，衰减延迟到读时折算，写放大恒定 O(1)/次上屏。

### 1.6 deploy 编译产物与校验

- 产物族：`<dict>.table.bin`、`<dict>.prism.bin`、`<dict>.reverse.bin`、`<pack>.table.bin`（packs 附加词库）；编译入口 `DictCompiler::Compile`（`dict/dict_compiler.cc:90-210`），输出到 `deployer->staging_dir`（`build/`）。
- 校验与增量：源 dict.yaml/text 的 **CRC32**（`ChecksumComputer`，`algo/utilities.h:18-32`，64KB 分块 `boost::crc_32_type`）写进 .bin Metadata；再次部署时重算源文件校验和与 metadata 比对，仅重建变化件（`kRebuildPrism/kRebuildTable` 选项位，`dict_compiler.h:27-32`；比对逻辑 `dict_compiler.cc:97-135`）。schema 文件校验和独立驱动 prism 重建（拼写规则随 schema 变）。
- 编译期管线 `EntryCollector`（`entry_collector.cc`）：多源 yaml 合并 → `Vocabulary`（按 code 长度分层）→ 建 4 级索引；遇到 syllabary 之外的非法音节直接**丢弃该词条**并记 ERROR（198-201 行）——建库期质量门。
- 任务化：`Deployer`（`deployer.h:32-75`）= 任务队列 + `std::future` 工作线程；任务名注册于 lever/dict 模块（`deployment_tasks.h`：`workspace_update`、`schema_update`、`prebuild_all_schemas`、`user_dict_sync`、`cleanup_trash`…）。

跨栈不变量：部署产物的「身份」= 源文件 CRC32 对，缓存失效判定 O(1)，重部署代价 = 差集。

### 1.7 配置系统（补充）

`config/config_compiler.h:37-40`：`__include / __patch / __append / __merge` 四指令在部署期经 `ConfigDependencyGraph` 拓扑解析，把 distribution/schema/用户 custom 三层 yaml 编译为单个 `.schema.yaml` 快照——运行期零 patch 开销。

## ③ 引擎侧感知性能机制

按「按键→首候选」调用链列出：

1. **mmap 句柄共享池**（`dictionary.cc:453-478`）：`DictionaryComponent` 持 `map<string, weak<Table>> table_map_` / `prism_map_`——同词库跨 session/跨 schema 复用同一映射，首次 Load 后常驻进程，后续按键零 IO。
2. **SyllableGraph 构建即剪枝**（`syllabifier.cc:31-222`）：优先队列按「最佳拼写类型」扩展顶点（同位置更差类型直接 continue，52 行）；构建完从 `farthest` 反向删除不可达顶点/低类型边（174-222 行）—— translators 拿到的 DAG 已最小化。
3. **Translation 全线惰性生成器**：`Translation::{Next,Peek,Compare,exhausted}`（`translation.h:17-39`）是候选的 pull 模型；`ScriptTranslation::Evaluate` 只做首屏必需的 Lookup+组句，逐候选物化（`script_translator.cc:519-583`）。`CacheTranslation`（1 元缓存）、`DistinctTranslation`（文本去重集）、`PrefetchTranslation`（队列缓冲，`translation.h:104-144`）为三种包装器。
4. **指数扩容预取**（`table_translator.cc:117-207`）：`LazyTableTranslation` 初始 `kInitialSearchLimit=10` 条，下游拉空后 `kExpandingFactor=10` 翻倍重查（`FetchMoreTableEntries/FetchMoreUserPhrases`，Replenish 钩子）——短输入最多物化 10 条，翻页才扩查。
5. **Menu 按需分页**（`menu.cc:26-58`）：`Prepare(requested)` 只生成到请求下标；`CreatePage(page_size, page_no)` 缺页时补拉。首候选代价与词库命中数无关。
6. **多路合并选举**（`translation.cc:126-152`）：`MergedTranslation::Elect` 对 N 个 translator 流做锦标赛（每流经 `Compare(other, previous_candidates)` 定序），取当前最小者为全局下一条——归并 O(N) 每候选，无需物化全队列。
7. **组件创建计时**（`engine.cc:329-342`，上游 PR #1216）：每个组件工厂 Create 计微秒日志 `[ComponentTiming]`——装配期性能回归可见。
8. **无结果级 LRU/memoization**：跨按键不做查询结果缓存（每次 Compose 重建 DAG 与 lookup），仅靠段级 menu 复用 + mmap 常驻。负向确认：全库 grep `memo|lru` 无查询缓存实现（仅 `ascii_composer` 的 `cached_input_` 单键状态，`ascii_composer.h:69-70`）。

跨栈不变量：首候选延迟 = O(段数 × (DAG 构建 + 首屏 limit 内 lookup))，与词库总量无关；所有「大词库」成本都被 limit/生成器挡在首屏之外。

## ④ 极限规模背压与截断

**截断常量组**（跨栈可迁移的调参清单）：

| 常量 | 值 | 锚点 | 作用 |
|---|---|---|---|
| `kExpandSearchLimit` | 512 | `syllabifier.cc:226` | 简拼补全的 ExpandSearch 上限 |
| `ToleranceSearch` 限 | 5 | `syllabifier.cc:95` | 纠错候选上限 |
| `max_corrections_` | 4 | `script_translator.cc:173` | 单次翻译纠错候选最多 4 条 |
| `max_homophones_` | 默认 1，schema 可调 | `script_translator.h:58`; 组句时 `EnrollEntries`（`script_translator.cc:688-703`）按它截每边 homophones | WordGraph 每边每起点词条上限 |
| `kMaxSyllablesForUserPhraseQuery` | 5 | `script_translator.cc:708` | 组句时用户词最大查深 |
| Beam 宽 `kMaxLineCandidates` | 7 | `poet.cc:137` | 组句束宽 |
| `beam_width`（k-best） | `max_sentences*3` | `poet.cc:263-264` | k-best 搜索宽度 |
| `cutoff_threshold` 衰减 | ×(1−1/N) | `poet.cc:325-341` | 第 k 句与首句权重差超过衰减阈值即截断 |
| `menu/page_size` | 默认 5 | `schema.cc:32-34` | 翻页大小 |
| `LazyTable` limit | 10→×10 | `table_translator.cc:119-120` | 增量扩查 |
| `Grammar::Evaluate` 无语法惩罚 | −13.8155 | `grammar.h:23-26` | 无 grammar 时统一 log(1e-6)，使 DP 结果与含语法束搜一致性成立 |

- **百万级词库 deploy**：无分块压缩；规模控制靠 ①建库期非法词条丢弃（`entry_collector.cc:198-201`）②文本经 marisa-trie 去重（短串内联、长串 StringId）③索引深度封顶 3。包体拆分走 `packs`（附加 table，`dictionary.cc:396-401` 可选加载、缺失不致命）。
- **事件洪峰**：[经源码检索确认：未实现该机制，检索词：`grep -rn -i -E "event.?loop|main.?loop|xevent|drop|coalesce|debounce" src/rime`（仅命中 entry_collector 的建库丢词日志）]。librime 是同步库——按键驱动、调用即处理完返回，**不存在内部事件队列**；背压责任完全在上游前端（fcitx5/thewe）。rspinyin 的 SPSC 槽位语义在此无对标物。
- **查询截断**：`Menu::Prepare` 即分页截断点；`DictEntryIterator::Skip`（`dictionary.h:40`）支持跳过不物化。

## ② 展示层：负向证明

[经源码检索确认：未实现该机制，检索词：`grep -rn -i -E "render|draw|paint|window|X11|wayland|cairo|opengl|bitmap" src/rime --include=*.cc --include=*.h -l`。命中仅为：`candidate.cc:47` 注释 "// draw"、`deployment_tasks.cc:29` 的 `<windows.h>`（文件操作）、`ascii_composer.cc:298` 注释、`key_table.*` 的 XK_ 键名常量]。librime 无任何绘制/窗口/布局代码；对 UI 的全部输出 = `Context` 的 preedit/composition/menu 数据 + `message_sink_` 事件（"schema"/"option" 字符串对）。它是纯 headless 引擎，展示层 100% 由前端实现——与 rspinyin「引擎可测、窗口自绘」的分层完全同构。

## 关键源码摘录（8 段）

**摘录 1 — OffsetPtr：mmap 内自相对偏移指针（位置无关零拷贝的地基）** `src/rime/dict/mapped_file.h:40-50`

```cpp
T* get() const {
  if (!offset_)
    return NULL;
  return reinterpret_cast<T*>((char*)&offset_ + offset_);
}
private:
Offset to_offset(const T* ptr) const {
  return ptr ? (char*)ptr - (char*)(&offset_) : 0;
}
Offset offset_ = 0;
```
跨栈提炼：文件内指针 = int32 自偏移 ⇒ 映射基址无关，加载零重定位。

**摘录 2 — Engine::Compose 的增量失效边界** `src/rime/engine.cc:164-178`

```cpp
Composition& comp = ctx->composition();
const string active_input = ctx->input().substr(0, ctx->caret_pos());
comp.Reset(active_input);
if (ctx->caret_pos() < ctx->input().length() &&
    ctx->caret_pos() == comp.GetConfirmedPosition()) {
  // translate one segment past caret pos.
  comp.Reset(ctx->input());
}
CalculateSegmentation(&comp);
TranslateSegments(&comp);
```
跨栈提炼：重算范围被裁到 caret 前，已确认段整体免重译。

**摘录 3 — Poet 双策略组句（DP / BeamSearch 可插拔）** `src/rime/gear/poet.cc:134-151`

```cpp
struct BeamSearch {
  using State = LineCandidates;          // key = last_word
  static constexpr int kMaxLineCandidates = 7;
  static void ForEachCandidate(const State& state,
                               Poet::Compare compare,
                               UpdateLineCandidate update) {
    auto top_candidates =
        find_top_candidates<kMaxLineCandidates>(state, compare);
    for (const auto* candidate : top_candidates) {
      update(*candidate);
    }
  }
```
跨栈提炼：同一 Viterbi 骨架，束宽与状态键（末词分桶）策略化，k-best 另带权重衰减截断。

**摘录 4 — LazyTableTranslation 指数扩容预取** `src/rime/gear/table_translator.cc:189-207`

```cpp
bool LazyTableTranslation::FetchMoreTableEntries() {
  if (!dict_ || limit_ == 0) return false;
  size_t previous_entry_count = iter_.entry_count();
  DictEntryIterator more;
  if (dict_->LookupWords(&more, input_, true, limit_, blacklist_) < limit_) {
    limit_ = 0;  // no more try
  } else {
    limit_ *= kExpandingFactor;   // 10 → 100 → 1000 ...
  }
  if (more.entry_count() > previous_entry_count) {
    more.Skip(previous_entry_count);
    iter_ = std::move(more);
  }
  return true;
}
```
跨栈提炼：候选物化量自适应下游消费速度，初始常数小、翻页几何扩。

**摘录 5 — Menu 按需分页** `src/rime/menu.cc:26-34`

```cpp
size_t Menu::Prepare(size_t requested) {
  while (candidates_.size() < requested && !result_->exhausted()) {
    if (auto cand = result_->Peek()) {
      candidates_.push_back(cand);
    }
    result_->Next();
  }
  return candidates_.size();
}
```
跨栈提炼：UI 只要第 k 条就只算到第 k 条，生成器 pull 天然截断。

**摘录 6 — Table::Query 沿 SyllableGraph 的 BFS 查询** `src/rime/dict/table.cc:595-625`

```cpp
for (const auto& spellings : index->second) {
  SyllableId syll_id = spellings.first;
  for (auto props : spellings.second) {
    size_t end_pos = props->end_pos;
    double next_credibility = props->credibility + penalty;
    double delta_quality_len =
        (is_normal_spelling ? 1.0 : 0.0) * (end_pos - current_pos);
    TableAccessor accessor =
        query.Access(syll_id, next_credibility, delta_quality_len);
    if (!accessor.exhausted()) {
      (*result)[end_pos].push_back(accessor);
    }
    if (end_pos < syll_graph.interpreted_length &&
        query.Advance(syll_id, next_credibility, delta_quality_len, current_pos)) {
      q.push({end_pos, query});
      query.Backdate();
    }
  }
}
```
跨栈提炼：可信度与「全码质量长度」随 DAG 边累加，直通词条排序——排序信息在查询期一次算全。

**摘录 7 — 用户词软删/复活/衰减（读时折旧）** `src/rime/dict/user_dictionary.cc:425-458`

```cpp
if (db_->Fetch(key, &value)) {
  v.Unpack(value);
  if (v.tick > tick_) { v.tick = tick_; }   // fix abnormal timestamp
}
if (commits > 0) {
  if (v.commits < 0) v.commits = -v.commits;  // revive a deleted item
  v.commits += commits;
  UpdateTickCount(1);
  v.dee = algo::formula_d(commits, (double)tick_, v.dee, (double)v.tick);
} else if (commits < 0) {  // mark as deleted
  v.commits = (std::min)(-1, -v.commits);
  ...
```
跨栈不变量：删除是符号翻转不是行删；衰减在读取时按 tick 差折算，存储态永不批量回写。

**摘录 8 — Prism 拼写多值展开进 SyllableGraph 边** `src/rime/algo/syllabifier.cc:126-146`

```cpp
SpellingAccessor accessor(prism.QuerySpelling(m.value));
while (!accessor.exhausted()) {
  SyllableId syllable_id = accessor.syllable_id();
  EdgeProperties props(accessor.properties());
  ...
    props.end_pos = end_pos;
    if (corrector_ && exact_match_syllables.find(m.value) ==
                      exact_match_syllables.end()) {
      props.is_correction = true;
      props.credibility = kCorrectionCredibility;
    }
    auto it = spellings.find(syllable_id);
    if (it == spellings.end()) {
      spellings.insert({syllable_id, props});
    } else {
      it->second.type = (std::min)(it->second.type, props.type);
    }
```
跨栈提炼：一个输入串经拼写索引一次展开出全部（音节,类型,可信度），DAG 边携带排序权重。

## 《候选吸收点清单》

| 编号 | 源码锚点 | 一句话机制价值 | 跨栈移植难度 |
|---|---|---|---|
| SCOUT-A-01 | `dict/mapped_file.h:21-51` OffsetPtr | 文件内 int32 自相对偏移指针，mmap 零重定位零拷贝、产物位置无关 | 低（己方 mmap.rs 可加同构容器层） |
| SCOUT-A-02 | `dict/table.h:54-77` + `table.cc:392-490` 四级索引 | 索引深度封顶 3 键，长码存 extra_code 尾验证，深码查询不炸深度 | 中 |
| SCOUT-A-03 | `dict/prism.h:21-52` + `syllabifier.cc:126-155` | 模糊音/简拼/纠错建库期展开为多值拼写索引，查询零规则开销且带类型+可信度 | 中（己方 FST 需加 value=音节集多值映射） |
| SCOUT-A-04 | `algo/syllabifier.cc:31-222` | DAG 构建即 Dijkstra 剪枝（同点位劣质拼写丢弃）+ 反向不可达清理，缩小下游规模 | 中 |
| SCOUT-A-05 | `rime/translation.h:17-39` Translation 接口 | 候选全线惰性生成器（Next/Peek/Compare），首候选延迟与词库规模解耦 | 低 |
| SCOUT-A-06 | `gear/table_translator.cc:117-207` LazyTableTranslation | 初始 10 条、×10 指数扩容预取，候选物化量自适应翻页需求 | 低 |
| SCOUT-A-07 | `rime/menu.cc:26-58` Menu::Prepare/CreatePage | 按需分页物化候选，page_size 默认 5，天然截断点 | 低 |
| SCOUT-A-08 | `rime/translation.cc:126-152` MergedTranslation::Elect | 多 translator 流锦标赛归并，每候选 O(N) 无需全队列 | 中 |
| SCOUT-A-09 | `gear/poet.cc:258-353` MakeSentences | k-best 组句：束宽 3N、text-hash 去重、cutoff_threshold 随名次衰减(×(1−1/N)) | 低（嫁接己方 K-best Viterbi） |
| SCOUT-A-10 | `algo/dynamics.h:6-15` + `dict/user_db.cc` UserDbMerger::Put | (commits,dee,tick) 三元组 + 读时指数折旧 + max-dee CRDT 合并，写放大 O(1) | 低（公式与 schema 可直译） |
| SCOUT-A-11 | `dict/user_dictionary.cc:443-456` 软删/复活 | 删除=commits 符号翻转，BackSpace 回滚学习事务（DiscardSession） | 低 |
| SCOUT-A-12 | `dict/dict_compiler.cc:97-135` + `algo/utilities.h:18-32` | 源文件 CRC32 写入 .bin metadata，部署按差集增量重建 | 低 |
| SCOUT-A-13 | `engine.cc:181-211,354-400` + `component.h:20-38` | 配置驱动的组件装配（Ticket+Registry）+ 段级增量重算 + [ComponentTiming] 装配耗时回归 | 低 |
| SCOUT-A-14 | `segmentation.cc:26-40` Segment::Reopen | caret 未越段尾时 kSelected→kGuess 复用旧 menu，编辑前缀不重查词 | 低 |
| SCOUT-A-15 | `script_translator.cc:688-703` + 常量组（512/5/4/homophones/束宽7/page_size5） | 全链路截断常量组：补全/纠错/组句/翻页各层独立上限，防大词库长尾 | 低 |
| SCOUT-A-16 | `gear/memory.cc:99-136` CommitEntry 学习管线 | 上屏按 core_word_length 串联切分学习、超 max_word_length 降级为元素级学习 | 中 |
| SCOUT-A-17 | `dict/dictionary.cc:453-478` weak 句柄池 | 同词库 mmap 句柄跨 session 共享，进程内唯一映射常驻 | 低 |
| SCOUT-A-18 | `config/config_compiler.h:37-40` 四指令 DSL | __include/__patch/__append/__merge 部署期编译为单快照，运行期零 patch | 中 |
| SCOUT-A-19 | `gear/grammar.h:18-27` Grammar::Evaluate | 无语法模型时统一 −13.8 惩罚，保证纯 DP 与带语法束搜结果兼容 | 低 |
| SCOUT-A-20 | 负向证明（②④节检索词） | 引擎零 UI、零事件队列——背压全在上游；佐证 rspinyin 分层正确，无需对标 | — |
