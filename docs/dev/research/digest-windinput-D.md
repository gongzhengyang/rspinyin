# 勘察摘要：WindInput（清风输入法）——簇 SCOUT-D

> 勘察对象：`remarks/WindInput`（GPL-3.0）。核心引擎 `wind_input/` 为 Rust workspace（edition 2024），
> 27 个 `wind-*` crate + 4 个 apps，共 ~409k 行 Rust。本摘要全部锚点经源码精读核实（2026-10-27）。
> 核心管线权威文档：`WindInput/docs/architecture/engine-candidate-pipeline.md`（1180 行，与代码同步维护）。
> ⚠️ GPL-3.0：以下摘录仅作锚点指认，产品侧吸收只能是机制级语义转写。

## 1. 总体形态：外置服务，非进程内

与 rspinyin 的「Fcitx5 进程内插件」根本不同：WindInput 是 **Windows TSF DLL（C++17）↔ Named Pipe ↔ Rust 核心服务**三段架构（macOS 用 IMKit .app 对位，`wind_macos/`）。按键在 C++ 侧 `OnTestKeyDown` 就地决定吃/不吃，早于 IPC 往返——由此产生一条立约级不变量：「C++ 吃键集 ⊆ Rust 出字集」，违反即在严格宿主上丢键（`AGENTS.md` 跨组件硬约定，历史案例：全角模式丢键）。Rust 服务内部**单进程多线程、无 async 运行时**（`Cargo.lock` 354 个包无 tokio/async-std/smol；检索词 `grep 'name = "tokio"'` 等均无命中）。

## 2. 维度①：目录拓扑与核心架构

### 2.1 27 crate 分层与单向依赖

按职责天然分层（`wind_input/Cargo.toml` members 顺序即依赖序）：`wind-utils`（零依赖底座）→ `wind-ipc`（线协议）→ `wind-dict`/`wind-store`/`wind-config` → `wind-engine` → `wind-candidate` → `wind-coordinator`（136k 行大脑）→ `wind-ui-types`（表现层纯数据协议）→ `wind-ui`（渲染）与各平台桥（`wind-bridge`）。平台分层立约在 `AGENTS.md`「平台分层策略」表：**纯逻辑 crate 零平台依赖**，平台能力经 `HostServices` trait 注入（`wind-coordinator/src/host_services.rs`）或事件推送通路，不造第二个 pull 真相源。两个编译门兜底防走样：`cargo check-headless`（无 wind-ui 形态，Android 复用同一 coordinator）+ CI `cargo tree` 断言 coordinator 无 wind-ui 依赖边。**模式分派不用 trait 对象**：三散装 bool 收敛为单槽枚举 `ModeKind`（`wind-coordinator/src/pipeline.rs:52`，TempPinyin/TempEnglish/Url/Email/Unicode/Special(u8)…），注释明言「三 bool 无法表达『同一时刻至多一个独占模式』这一不变式」；键事件分派是对 `state.active` 的单次 match。

### 2.2 按键→候选完整管线（跨 crate 边界签名）

```
KeyEventData{key_code:u32, scan_code, modifiers, event_type:u8, toggles, event_seq:u16, prev_char:u16}
  （wind-bridge/src/handler.rs:19，From<&KeyPayload>）
→ Coordinator::handle_key_event (coordinator/src/coordinator/message_handler.rs:979)
→ 按 State.active: Option<ModeKind> 单点分派（overlay 模式各有 handle_*_key）
→ update_candidates (handle_candidate.rs:2294) → build_candidates (handle_candidate.rs:1218)
→ EngineManager::convert (wind-engine/src/manager.rs:991) → dyn Engine::convert(input, max) -> ConvertResult
→ 引擎后统一后处理：短语注入→层级排序→去重(merged_codes)→过滤→词频重排→shadow→自动上屏复评
→ UiCommand 推送 (wind-ui-types/src/command.rs:13)
```

**`Engine` trait**（`wind-engine/src/engine.rs:302`）：`convert/reset/engine_type/max_code_length/handle_top_code/recheck_auto_commit/set_dict_enabled` + 拼音探测 `is_whole_syllable_pinyin` 等。`EngineType::{Pinyin, CodeTable, Mixed, English}`。四引擎实现：拼音（双拼是其**前置转换层**，非独立引擎）、码表、混输（主码表+次拼音+英文三子引擎）、英文（码表薄包装）。

**`ConvertResult`**（engine.rs:18+）设计亮点：五个 preedit 形态字段（`preedit_display/preedit_pinyin/preedit_fullpinyin/preedit_abbrev/preedit_codetable`）供「组合区跟随高亮候选类型」——每个字段的注释都写明「为何另立字段而不复用旧字段」（挤进老字段会连带改掉无关路径）；`completion_hints: Vec<Candidate>` 是**池不是单条**（补全采纳与否由看到最终列表的协调器在过滤后收口）；`shadow_code` 由**引擎**给出归一码（只有引擎握着双拼转换结果，协调器拿击键串重猜必然出错）；`sentence_pool` 承载整句 N-best。

**`Candidate`**（`wind-candidate/src/candidate.rs`）：`text/pinyin/code/weight: i32/base_order: i32/natural_order: i32/comment/consumed_length` + `source: CandidateSource{None,CodeTable,Pinyin,English,Phrase,Assoc}` + 分类标志（`is_phrase/is_command/is_group/is_fuzzy/is_prefix/is_partial/is_common/is_exact_code/is_wildcard`…）。统一比较器 `better()`（candidate.rs:1172）：`weight desc → base_order asc → natural_order asc → code asc → consumed_length desc → text asc`。`CandidateSource::Assoc` 的注释点明来源枚举的语义本质：「不在『哪本词库』而在『有没有输入』——凡以输入码为 key 的加工都必须跳过它」。

### 2.3 词库：双数组 Trie mmap + 派生缓存 + 多层合并

三格式（`wind-dict/`）：YAML 源（RIME 风格 TSV，列序按文件头声明/投票探测）、`.wdb`（Header+KeyIndex+Data+StringPool）、**`.wdat` 双数组 Trie**（`datformat.rs:2698 行`：Header+Base/Check/MaxW 三段+LeafTable+EntryRecords+StringPool+**独立简拼段 AbbrevSection**（自描述 24B 头，不污染全拼前缀查询）+CharMap `[i32;256]`）。

- **零拷贝 mmap**：`WdatReader`（datformat.rs:1008）打开即校验 magic+version+全段偏移越界，失败 bail 回退重建；精确查询 walk 编码验终止符，前缀查询 walk 后按权重降序截断，`for_each_entry` 流式遍历供离线索引构建。
- **内容指纹缓存**（`cached.rs:296 load_at_with`）：`.wdat` 存在且 sidecar `.fp` **内容指纹**（非 mtime）匹配 → 直接 mmap；否则加载 yaml→写 `.wdat`→**先删旧 .fp 再写新 wdat**（写失败留「无指纹」而非错指纹）→mmap 重开。每缓存文件 single-flight `file_lock`（`reader_pool.rs:199`，HashMap<PathBuf, Arc<Mutex<()>>>）——同一 wdat 被多方案引用（独立引擎与混输子引擎）时防并发重建。缓存写失败**显式降级**为 `CachedDict::Memory` 并 warn「本次退化为内存模式——词库常驻堆而非 mmap」。wdat-only 分发模式（无 yaml 源）抢在 cache 校验之前，失败明确报错不静默。
- **reader_pool**（reader_pool.rs:171-178）：`OnceLock<Mutex<HashMap<PathBuf, Entry<Arc<T>>>>>` 三张池（wdat/comment/emoji）共享 mmap reader；`Replacing` RAII 管理换文件时旧 reader 的 Weak 降级。
- **多层合并**（`composite.rs:163 merge_search`）：层优先级 `Logic(0)>User(1)>Temp(2)>Cell(3)>System(4)`（`layer.rs:13`），每层 natural_order 叠 `layer_idx × 10M`；按 text 去重时**仅 code 相同才继承更高权重**——「权重属于 `(code,text)` 词条，不属于字」，跨码位取 max 等于让候选带着别的码位的权重参与排序。
- **`DictLayer` trait**（layer.rs:33）契约设计：`search_abbrev` 默认**返回空 Vec 而非回退全表扫**——「静默的全表扫正是简拼卡顿的根因，与其让新层不知不觉继承那个代价，不如召不回、在测试里立刻暴露」；`for_each_entry` 注释钉死「O(全表)，绝不能出现在按键链路上」；`entries_digest()` 返回 None 的层其派生索引**不落盘**（慢但正确，宁可没缓存不骗新鲜）。

### 2.4 方案文件驱动（解释器架构）

`data/schemas/*.schema.toml` 定义全部行为：`[schema] id/name/icon_label` + `[engine] type = pinyin|codetable|mixed|english` + 引擎固定参数（`[engine.codetable] max_code_length=4, base_sort="weight"`）+ `[[dictionaries]]`（path/type/`default`/`base_order`/`default_enabled`）。引擎构建按 type 分流（manager.rs build_engine）。**双拼布局是数据不是代码**：`data/schemas/shuangpin/xiaohe.toml` 三表 `[initials]/[finals]/[zero_initials]`，内置 8 种布局；零声母两大流派（O 引导 vs 首字母引导）不可互抄，靠 `tests/shuangpin_coverage.rs::official_zero_initial_strokes_work` 正向击键表把守「官方击键打不打得出」（覆盖率门禁问不到这一层）。用户覆盖两层：`schema_overrides/{id}.toml` 与方案段**同段同名**深合并（设置页写入点）；配套一条硬约定——出厂预置必须落在用户能清空的载体（逐键深合并表达不了删除，map 字段里删掉的键会被合并回来，见 AGENTS.md 三条出路）。

## 3. 维度④：背压与资源纪律

### 3.1 通道容量与溢出语义（唯一的有界队列）

推送管道 `wind-bridge/src/push.rs`：

```rust
/// 每个客户端发送队列的帧数上限。
/// 正常情况下队列几乎是空的（写线程即收即写），连接握手时的一批配置推送约 14 帧，
/// 64 帧足够吸收突发；真满了说明对端不读了，见 offer 的处置。
/// 不取更大：sync_channel 创建时就按容量分配槽位（实测 256 槽 ≈ 8.8 KB/客户端，而
/// 每个加载了 TSF 的进程都有客户端），上限本身不该变成新的常驻开销。
pub(crate) const PUSH_QUEUE_CAP: usize = 64;
```

溢出语义是「**断开，不是丢帧**」（push.rs:57-70）：`offer` 用 `try_send`，`TrySendError::Full` 判该客户端卡死 → `drop_stalled` 移出客户端表 → 写线程解除阻塞写完余帧、recv 出错、断开管道 → DLL 读线程感知断开重连 → **重连握手做全量状态同步**。对端恢复后拿到的是当前状态而非半截过期帧。按键主路是同步请求-响应（TSF 等待），无队列积压问题；`StatusUpdateData`（handler.rs:48）立约「每条携带状态的响应必须带当前值，DLL 无条件镜像——level-triggered，漏带即清掉正活着的会话」。

### 3.2 内存策略

- **mmap 优先 + 派生索引按大小自动选驻留**：`.wridx`（反查索引，`wind-dict/src/reverseidx.rs`）小于阈值整份读入内存、超过则 mmap，**两条路同一份字节布局、同一套查找代码**（反查低频，两套实现分叉会长期无人发现）。`.wridx` 头注释记录了一次**净亏否决**：曾试把反查做成 text 为键的 DAT——264MB 磁盘+15s 写入换省 95MB 内存，方案作废；反查只需要二分点查+按 text 序顺扫，DAT 是为「按前缀逐键检索百万条」付的钱。
- **池化 arena 手工搓**：ChaiziTable/注释库用「排序数组 + 共享 String arena + 二分」，每条 `text|comment|code` 三段连续存、各自长度记条目（`wind-reverse/src/lib.rs:102-161`）；`.wridx` 编码用 1 字节长度前缀替代全局结束偏移数组（每码省 3B，代价是只能顺序走——每词编码数个位数，可接受）。[经源码检索确认：引擎热路径**未用** bumpalo/arena 分配器 crate，检索词 `grep 'bumpalo|arena|simd|SmallVec' crates/**/Cargo.toml` → 仅 wind-ui-types 有 smallvec；Cargo.lock 中 bumpalo 为 wasm-bindgen-shared 传递依赖，不触及引擎]
- **空闲归还堆**（`wind-coordinator/src/heap_trim.rs:12`）：进程分配器释放后不主动还 OS（实测 23.6MiB 释放后不落，malloc_trim 压回 5.8MiB）；三平台实现（HeapOptimizeResources+HeapCompact / malloc_trim(0) / malloc_zone_pressure_relief），**只在空闲时调**（接线 redb 空闲回收：长档连续 60s 无人碰库），「放在按键路径上就是白白抖动」。
- **引擎自报内存**：`Engine` trait 带 `memory_parts() -> Vec<MemPart{name, bytes, allocs}>`（engine.rs:295-304），只读、不触发构建，bytes 与 allocs 一起看（逐条 String 在真实分配器上的占用明显高于 bytes）。
- **redb 池**（`wind-store/src/store.rs`）：`pause/resume/drop_page_cache/spawn_idle_cache_reclaimer`——页缓存主动回收 + 空闲回收线程，配 `mark_scan_pending`（全表扫待回收时短档 3s 触发）。

### 3.3 分配纪律与失效纪律（按键热路径）

- 候选元数据用 `Arc<str>` 而非 `String`（`CandidateMeta.weight_layer/store_code`）：「每条候选上都要填，候选在按键热路径上成百上千地造；层名是固定几个短串，克隆一次原子计数远比一次堆分配便宜」。
- 参数钳制防方案作者放大分配：`SPLIT_FRONT_LIMIT=8` 钳制 `split_front_candidates`——「方案作者写 50 就会在按键热路径上构造 50×8=400 条候选（每条两次 String 分配），紧接着 truncate 又把绝大多数丢掉」（codetable/engine.rs:64-71）。
- **「记住失败」双 Option 缓存**：`shared_english_engine: Mutex<Option<Option<Arc<dyn Engine>>>>`（manager.rs）——`Some(None)`=试过且不可用不再重试，少了这层「每次按键都会重走构建、失败、warn!，热路径刷日志且白做功」。
- **代次防旧值回填**：`aux_settings_gen: AtomicU64`——未命中时先记代次再算，回填前代次变了就不回填，否则「算到一半被清」的旧值插回去一直留到下次失效（manager.rs）。
- 缓存含 None：`schema_type_cache: Mutex<HashMap<String, Option<String>>>`——缓存「文件不存在」，避免反复读盘+刷成片警告。
- 候选扩容阶梯：首批 `initial_candidate_limit_of`（handle_candidate.rs:1106）码表按输入长 100/300/1000、拼音混输 300；翻页到边界 ×2 至 `CANDIDATE_LIMIT_CAP=5000`（engine.rs:38，注释明言「两处各写一个 5000 的话，扩容要 8000、引擎只回 5000 ⇒ has_more 恒假」）。

## 4. 维度③：引擎侧感知性能

- **懒加载 + 双层 single-flight**：`HashMap<方案ID, Arc<dyn Engine>>` 懒构建；`build_locks`（引擎/缓存，快）与 `index_build_locks`（反查索引，秒级上百 MB）**刻意分开**（manager.rs:604-610），互不阻塞。
- **单字母前缀预聚合 hotcache**（`wind-dict/src/hotcache.rs`）：'z' 子树 ~47k 候选，`HotCache` 按文件键+首字节 256 个 `OnceLock` 槽位缓存 top-K，`get_or_build` 后台建、首查即中。
- **派生索引懒建+后台预热**：英文词组 `LazyPhraseIndex` 用 `RwLock<Option<Arc<..>>>` 而非 OnceLock——「词库热摘（set_dict_enabled）不重建引擎，索引必须能单独作废」；反查索引 `prewarm_indexes` 切方案后台预热。
- **K-best Viterbi**（`wind-engine/src/pinyin/viterbi.rs:353 decode_nbest`）：每位置保留 top-K 前驱（`prev_idx` 回溯），**k=1 与单路径 DP 逐位相同**（零回归依据）；部分可达路径 `log_prob=NEG_INFINITY` 原样保留（首版取 dp[end]，部分解「不知道哈」抢了首选）；**不按文本去重**留给调用方连同凑数一起做；有语法模型走 beam（`BEAM_WIDTH=7` 按末词分组）时只返回 1 条（beam 的前 k 条只差末词，当 N-best 是误导）。评分以词条自身词典权重为基（`ln(weight/DICT_TOTAL)`，DICT_TOTAL=242,154,693），无独立语言模型文件——早先的 unigram.txt 因双源冗余+多音字污染（虚高 6.7 万倍）整体移除。
- **切分与查询预算**：`Dag::maximum_match` DP 求覆盖最多字符的切分（非贪心）；模糊音笛卡尔积上限 64，「超预算时逐级降低可同时模糊的音节数，不是整体跳过」。
- **截断保护精确匹配**（codetable convert）：短输入前缀候选数百条，纯按权重 truncate 会把精确全码挤出配额——超额时改「精确优先」稳定分区截断再恢复显示序。
- **负向证明**：[经源码检索确认：拼音引擎无按键级解码结果缓存/增量解码，检索词 `grep 'cache|memo' pinyin/mod.rs dag.rs` → 仅命中注释「移除死状态 code_hint_cache」（mod.rs:803）；每次按键全量重算，速度靠 mmap+预聚合索引支撑]。无 SIMD（同 3.2 检索）。配置解析全缓存化：`read_schema` 每次都是 `fs::read_to_string`+TOML 解析，「每次按键调 5+ 次」的判据必须进内存镜像，漏刷新的症状是「设置页改了不生效、重启后才生效」。

## 5. 附加能力面（己方未实现/部分实现）

### 5.1 wind-assoc（联想，1.0k 行）

四级来源 `AssocSource{History, Bigram, Prefix, Punct}` 顺序即优先级，逐源配额+HashSet 去重（`lib.rs:329 associate`）。**Bigram = 离线从 n-gram 模型蒸馏的词→后继表**；Prefix = 码表词文本前缀延伸。三个契约点：① `AssocHit{text, commit: Option<String>}` 展示文本与上屏文本分离（词语联想显示「中国」只补「国」；存字段而非下游 strip_prefix——Smart 档候选不以上文开头，strip 失败的兜底恰好看起来是对的）；② `AssocContext.boundary_broken`（失焦/切宿主/点击/方向键/超时）断链即不出——「recent_commits 是跨应用跨焦点全局队列，历史相邻≠屏幕相邻」；③ 编译器守门防静默漏源：`ALL` 数组与 `priority()` 两处独立维护，`priority` 的 match 穷尽新增变体即编译失败。

### 5.2 wind-aux-code（辅码，2.5k 行，出厂关）

两种形态：**overlay 会话**（引导键进入，`AuxCodeSession` 持 `CandidateStore` 原始快照+筛选视图，每次从快照重筛，`kept` 是输入的**子序列**不变量绝不重排，退格/退出从快照恢复）；**直接辅码**（双拼无引导键，无状态，`direct.rs:59 split_direct` 按奇偶把输入拆前缀+末 1~2 位，前缀候选里字形对上并入）。表格式 `字=码` 一行一条（多码空白分隔），相邻文件 merge 成层、`schema:<id>` 方案来源复用码表方案词库。进入门卫五道（已在别的 overlay/未启用/键被音节分隔符占用要告警/方案未配 files/无候选）一律不吞键；支持共键写法 `"aux_code:page_next"`（能进就进、进不去翻页）。

### 5.3 wind-reverse（反查，2.7k 行）

`ReverseLookup`（lib.rs:17）三表：拆字库（方案 `[engine.chaizi].db_path`，字\t字根\t编码，可随方案热重载 `swap_chaizi`）、拼音表（pinyin-data 格式 `U+4E00: yī`，多音字逗号分隔）、注释库多源挂载（路径按优先级升序先到先得，各源独立 `.wcmt` mmap 缓存独立校验新鲜度，「增删一个库不牵动其他库」）。查询面：`comment_of/codes_of/readings_of/toned_pinyin_of/radicals_of`，按 (text, code, schema) 消歧。

### 5.4 wind-keys / wind-phrase / wind-punct / wind-transform / wind-transfer

`wind-keys/src/keymap.rs` 是 VK 常量 SSOT（`VK_*` pub const，禁止裸十六进制）+ `KEY_TABLE` 键名↔VK 单一真相源 + `NavKeys` 从配置组名编译翻页/高亮键 + 二三候选键分流（可打印键组走 keydown、修饰键组走 **keyup**——修饰键 keydown 不能吃且在 keydown 判定会误选候选）。`wind-phrase` 静态/模板短语（`$CC` 命令/`$SS` 组二级展开）+ cmdbar 短语解析 AST；`wind-punct` 纯函数标点转换（`convert_punct` 四列查表 chinese_punct×full_width）；`wind-transform` 全角/自动配对（pair_tracker）/简繁（s2t 一对多变体展开）；`wind-transfer` Bundle（manifest+zip，`validate()`）聚合打包 + Merge 策略 + `.wpkg`/`.wtheme` 格式定义。`wind-utils`（583 行）立约：**零外部依赖**（否则全仓编译排在它后面）+ 只收「判据只此一份」的东西（utils 变垃圾堆都是这么开始的）。

## 6. 可平移 Rust 惯用法与语言无关不变量

**Rust 惯用法（可直接平移）**：单槽 `Option<ModeKind>` 替代多 bool 模式态；`Option<Option<Arc<T>>>` 记忆失败；`RwLock<Option<Arc<T>>>` 懒建可单独作废索引；OnceLock 槽位数组预聚合缓存；`AtomicU64` 代次防缓存旧值回填；`Arc<str>` 热路径元数据；枚举 `ALL` 数组+穷尽 match 守门配对登记；`From<&KeyPayload> for KeyEventData` 边界收敛 + re-export 进签名类型（不复刻等价结构）；`#[serde(rename)]` 线协议枚举；RAII `Replacing` 管理 mmap 换代。
**语言无关不变量**：候选排序必须落 weight（上层重排会冲掉引擎内改序）；去重必须并码位（`merged_codes`/`absorb_codes_from`，四处归并点清单写在类型注释里）；「默认实现返回空/None 好过默认静默全表扫」；派生缓存的指纹校验与先删指纹后写正文；断开重连全量同步优于丢帧；引擎判意向、协调器按显示态复核放行（上屏即所见）。

## 《候选吸收点清单》

| 编号 | 源码锚点 | 一句话机制价值 | 跨栈移植难度 |
|---|---|---|---|
| SCOUT-D-01 | `wind-bridge/src/push.rs:42,57-70` | 有界队列满=断开客户端待重连+握手全量状态同步，而非丢帧——对 rspinyin 的 Show/Hide/Select 通道溢出语义是现成第三种答案（断链后全量对齐） | 中 |
| SCOUT-D-02 | `wind-dict/src/layer.rs:33-120` | DictLayer trait 契约：search_abbrev 默认空实现（宁召回失败测试暴露，不做静默全表扫）+ for_each_entry 钉死离线路径 + entries_digest 派生索引新鲜度 | 低 |
| SCOUT-D-03 | `wind-dict/src/datformat.rs:1008`；`cached.rs:296`；`reader_pool.rs:199` | wdat 内容指纹 sidecar 缓存 + 每文件 single-flight 锁 + mmap reader 池共享 + 写缓存失败显式降级 Memory 并告警 | 中 |
| SCOUT-D-04 | `wind-dict/src/reverseidx.rs:1-50` | 派生索引按大小自动选驻留/mmap（同一字节布局同一套查找代码）；并把「DAT 方案净亏」的否决记录写进头注释供后人复用 | 中 |
| SCOUT-D-05 | `wind-coordinator/src/heap_trim.rs:12` | 空闲时三平台归还堆内存（malloc_trim/HeapCompact/zone_pressure_relief），只接空闲钩子不上按键路径——rspinyin 驻留内存预算的补充手段 | 低 |
| SCOUT-D-06 | `wind-engine/src/engine.rs:295-304` | Engine trait 自带 memory_parts() 自报 MemPart{name,bytes,allocs}，只读不触发构建——内存预算可审计化 | 低 |
| SCOUT-D-07 | `wind-engine/src/manager.rs:604-610`（另 `Option<Option<…>>`、`AtomicU64` 代次） | 引擎懒加载双层 single-flight（快构建与秒级索引分锁）+「记住失败」缓存 + 代次防旧值回填——按键热路径失效纪律三件套 | 低 |
| SCOUT-D-08 | `wind-engine/src/pinyin/viterbi.rs:353`；`lattice.rs:119-317` | K-best Viterbi 的工程约束：k=1 与单路径逐位相同（零回归）、部分解 NEG_INFINITY 保留、去重留调用方；节点打分常量全部具名（RETROFLEX_SPLIT_PENALTY=8.0 等）并注明实测依据 | 中 |
| SCOUT-D-09 | `wind-assoc/src/lib.rs:50-110,329` | 联想四级来源逐源配额 + text/commit 分离字段 + boundary_broken 断链闸 + ALL 数组与穷尽 match 编译器守门——rspinyin 联想能力的完整契约样板 | 中 |
| SCOUT-D-10 | `wind-aux-code/src/session.rs:25`；`direct.rs:59` | 辅码两形态：快照+谓词子序列重筛（CandidateStore）与无状态奇偶拆分并入（direct）；门卫五道不吞键 + 共键 `aux_code:page_next` | 中 |
| SCOUT-D-11 | `wind-coordinator/src/handle_candidate.rs:1106,2478`；`wind-engine/src/engine.rs:21` | 候选首批按引擎类型/码长阶梯（100/300/1000）+ 翻页 ×2 至全局唯一 CANDIDATE_LIMIT_CAP=5000（常量注释写明双写失配的后果） | 低 |
| SCOUT-D-12 | `wind-candidate/src/candidate.rs`（merged_codes 注释）；`wind-dict/src/composite.rs:163` | 去重不丢码位：absorb_codes_from + 「权重属 (code,text) 词条不属字」的同码才取 max——过滤单调性不因去重破坏 | 低 |
| SCOUT-D-13 | `wind-engine/src/codetable/engine.rs:64-71` | 方案作者可配参数在热路径入口钳制（SPLIT_FRONT_LIMIT=8），防止配置放大候选构造分配 | 低 |
| SCOUT-D-14 | `data/schemas/shuangpin/xiaomi…xiaohe.toml`；`wind-engine/src/pinyin/shuangpin.rs` | 布局是数据不是代码（三表 TOML）+ 位置映射回双拼键空间 + 正向击键覆盖测试守「官方击键」而非仅「打不打得出」 | 中 |
| SCOUT-D-15 | `wind-coordinator/src/pipeline.rs:52` | 模式态收敛为单槽 Option<ModeKind> 单点 match 分派，结构上保证互斥不变式；前缀夺取/后缀触发两类模式共用同一闸门与 Rewind 回退骨架 | 低 |
| SCOUT-D-16 | `wind-reverse/src/lib.rs:856-1040` | 反查多源注释库：路径优先级先到先得、每源独立 mmap 缓存独立验鲜、swap_chaizi 热重载——候选悬浮注释的工程化底座 | 中 |
| SCOUT-D-17 | `wind-utils/src/lib.rs` | 底座 crate 双约定：零外部依赖（保编译并行度）+ 只收「判据只此一份」的工具（防垃圾堆化） | 低 |
| SCOUT-D-18 | `WindInput/AGENTS.md`（平台分层策略表） | 「Android 上编得过吗」门 + check-headless + cargo tree 断言依赖边——用编译门把架构分层钉死而非靠评审 | 低 |
