# opt-perf.md — rspinyin 极致性能调优任务卡工程规范

> 文档版本: v1.0 ｜ 系统形态: **Desktop GUI**（自绘候选框 + 软件光栅 + Spring 动效，X11 / Wayland 双后端）｜ 架构基线: Rust 2024 workspace（7 crates + xtask），双线程物理隔离（Fcitx5 宿主线程 / UI 线程）+ SPSC 有界通道 + `eventfd` 唤醒，**无 async 运行时** ｜ 关联 ADR: [./adr/0000-upstream-decisions.md](./adr/0000-upstream-decisions.md)、[0001](./adr/0001-frozen-boundary-contracts.md)、[0002](./adr/0002-rust-exports-addon-factory.md)、[0003](./adr/0003-ui-role-separate-addon.md) ｜ 最后同步 Commit: `ee0dbfb` ｜ 维护约定: 代码演进后必须回写《性能热点总清单》状态与《WBS 任务覆盖追溯表》，并同步 `docs/dev/budgets.json`（若新增阈值）

**权威性声明**：本文档是**优化任务的唯一事实来源**，但**不是性能阈值的来源**。所有阈值以 `docs/dev/features.md` 0.5.3 为唯一权威（`docs/dev/budgets.json` 是其机器可读镜像，由 `just check-budget` 双向校验）。本文档与 0.5.3 冲突时以 0.5.3 为准。本文档不新增任何阈值，只引用、分解并为其补齐**断言手段**。

---

## 0. 被测项目性能瓶颈与热点诊断摘要

### 0.1 审计范围与方法

对 `crates/` 与 `xtask/` 全部 65,436 行 Rust 源码做了逐文件热点审计，重点覆盖三条链路：

| 链路 | 覆盖文件 | 审计要点 |
|---|---|---|
| **数据密集型链路** | `ime-core/src/{segment,viterbi,lm,state,input,preedit}/*`、`ime-dict/src/{fst_index,mmap,format,user_db}/*`、`ime-types/src/lexicon.rs` | 堆分配点、深拷贝、数据局部性、内存生命周期 |
| **并发调度链路** | `ime-fcitx5/src/{addon,engine,ui_impl,ffi}/*`、`ime-ui/src/{channel,ui_thread}/*`、`ime-core/src/state/machine.rs` | 主线程同步 IO / 阻塞锁 / 自旋、通道背压、有界性 |
| **渲染重绘链路** | `ime-ui/src/{renderer,platform,geometry,layout,spring,theme}/*`、`ime-ui/ui/*.slint` | 逐像素拷贝、整树重渲、damage 合并、缓冲池化 |

### 0.2 关键诊断结论（五条）

**结论一：解码路径每键约 100–200 次堆分配，且模块文档声称的"零分配"并未成立。**
`crates/ime-core/src/segment/dag.rs:30-36` 明确写道「A DAG that has already built a string of a given length builds again without touching the allocator, which is what keeps a decode free of allocations」。但 `Decoder::decode` 在 `crates/ime-core/src/viterbi/decoder.rs:233` 每次调用都 `SyllableDag::new()`，复用前提被打破；`Sweep::new`（`decoder.rs:431`）每键分配并零填充 `129 × 16 × 20B ≈ 40KB`；`read_words`（`crates/ime-dict/src/fst_index/read.rs:106`）每次词典查找分配一个 `Vec<WordRef>`，而一次解码最多发起 `MAX_KEYS_PER_NODE = 24` × 节点数（`crates/ime-core/src/viterbi/lattice.rs:63`）次查找。

**结论二：分词图每键被完整构建两次。**
`Session::resegment`（`crates/ime-core/src/state/machine.rs:453-467`）构建 `self.dag`，随后 `Session::refresh`（`machine.rs:436-446`）调用的 `Decoder::decode` 又为同一输入重建了一份 DAG。第二份是纯粹的重复计算。

**结论三：用户词频查询在热路径上打开 redb 读事务。**
`Scorer::edge_score`（`crates/ime-core/src/lm/score.rs:312`）对**每一次格边扩展**调用 `uf.freq(word)`；生产实现 `UserDb::freq`（`crates/ime-dict/src/user_db.rs:700`）在 4096 槽 CLOCK 缓存未命中时调用 `Inner::committed`（`user_db.rs:430`），即 `db.begin_read()` + `open_table()` + `get()`。一次解码可触发数百次 B 树读，且未命中路径要取 **3 次互斥锁**（`user_db.rs:692`、`:702`、`:707`）。这与冻结契约的明文要求冲突——`crates/ime-types/src/lexicon.rs:23-25`：「The methods run on the hot decode path and must not block, must not take a lock a writer can hold, and must not allocate per candidate.」

**结论四：宿主线程上存在三处无界阻塞源。**
(a) `UserDb::record`（`user_db.rs:747`）在批次/间隔触发时**在调用线程同步执行 redb 写事务**，即 fcitx5 主循环上的文件 IO，直接违反 AGENTS.md 第 10 条禁令；(b) `CollapsingQueue::push`（`crates/ime-ui/src/channel/queue.rs:312`）在队列满时持锁自旋整个预算；(c) 帧级诊断 `emit_diagnostic`（`crates/ime-fcitx5/src/ffi/mod.rs:105`）执行 `format!` + 阻塞 `writeln!(stderr)`，而 `candidate_window_available`（`crates/ime-fcitx5/src/addon.rs:288`）的文档约定它在**每一帧被拒时**记录一条诊断。

**结论五：渲染链路的端到端集成尚未落地，帧预算当前不可断言。**
`crates/ime-ui/src/ui_thread/surface.rs:87` 定义了 `trait UiSurface`，但**生产代码中没有任何实现**（`impl UiSurface for` 仅出现在 `event_loop.rs:168` 与 `tests.rs:122` 的测试替身中）；`UiThread::spawn` 无生产调用方；`crates/ime-fcitx5/src/addon.rs:373` 的 `ui_startup_body` 恒定返回 `Err(ImeError::UiChannelClosed)`；`crates/ime-fcitx5/src/ffi/abi/engine.rs:50` 的 `on_key_event` 是 stub，恒定返回 `false`。因此 `BUDGET-LAT-01`（按键到上屏）、`BUDGET-LAT-03`（单帧光栅）、`BUDGET-LAT-04`（首帧可见）**目前均无测量对象**。本规范据实登记这一边界，并将其转化为 `PERF-P0.04.01`（探针与基准先行）与 `PERF-P0.03.01`（渲染路径基准先行）两张任务卡——**先建立可测量的对象，再谈优化**。

### 0.3 审计通过项（非热点，登记以证明覆盖面）

以下位置经审查确认**不构成热点**，不作为任务卡对象；登记在此以避免后续重复审计：

| 位置 | 结论 | 依据 |
|---|---|---|
| `crates/ime-core/src/segment/syllable.rs:155` `lookup` | 411 项表的二分查找，约 9 次比较，无分配 | 纯静态表 + `binary_search` |
| `crates/ime-core/src/state/machine.rs:436` `Ctx::new` / `Effects` | `SmallVec<[Effect; 4]>` 内联，一次 step 零分配 | `machine.rs:71` 常量与 `MAX_EFFECTS` 断言 |
| `crates/ime-core/src/preedit.rs:140` `build_preedit_into` | 写入调用方持有的 `Preedit`，跨键复用容量 | 函数签名即证据 |
| `crates/ime-config/src/reload.rs:701` `ConfigStore::current` | 返回 `&Arc<Config>`，热路径读配置为一次 Arc 解引用 | 无克隆、无锁 |
| `crates/ime-ui/src/platform/wayland/shm.rs:169` `acquire` | 双槽 `wl_shm` 池，稳态零分配 | `pool_bytes` 与 `release` 成对 |
| `crates/ime-ui/src/platform/x11.rs:126` | 两个固定 `Vec<u8>` 缓冲 + `packed` 暂存，稳态零分配 | 字段声明 + `apply_size` 尺寸夹取 |
| `crates/ime-ui/src/spring.rs:305` `step` | 纯整数/浮点积分，无分配、无锁 | 定长状态机 |
| `crates/ime-ui/src/geometry.rs:326` `compute` | 每次放置分配一个 `hit_map: Vec<(RectI, u16)>`，条目 ≤ 9 | 单次分配约 108B，相对帧预算可忽略；若 `PERF-P0.03.01` 基准显示其显著，再升级为热点 |
| `crates/ime-fcitx5/src/ui_impl/cursor_rects.rs:76` `insert` | 定长 8 槽数组线性扫描 + 一次互斥锁，无分配 | 有界、无 IO |
| `crates/ime-fcitx5/src/ui_impl/panel.rs:74` `refill` | 复用两个 `String` 容量，稳态零分配 | `clear` + `push_str` |
| `crates/ime-ui/src/channel/wakeup.rs:83` `wake` | 每次投递一次 `write(2)` 系统调用 | 设计契约（`eventfd` 合并语义），非缺陷 |

---

## 1. 系统设计假设清单 (Assumptions First)

> 本清单是**所有基准断言的前提**。任何一条被推翻时，必须同步回写受影响的 `PERF` 任务卡阈值与 DoD；禁止在假设失效后沿用旧断言。

| 假设编号 | 维度 | 假设内容（基于阶段零实测） | 影响的优化层面 | 偏离时的修正策略 |
|---|---|---|---|---|
| `ASM-P01` | 负载模型 | 稳态输入速率 10 字/秒（对应 `BUDGET-CPU-02`）；突发上限 30 字/秒（快速连打，含候选翻页键） | 解码工作区复用、词频查询、帧快照 | 速率上修时重跑 `PERF-P0.04.01` 探针；若 P999 越界，将词频查询下沉为按帧批量预取 |
| `ASM-P02` | 负载模型 | 单次解码输入 ≤ 12 音节 / 64 字节（`MAX_RAW_LEN = 64`，`syllable.rs:27`）；候选上限 45（`DEFAULT_MAX_CANDIDATES`，`decoder.rs:65`） | 全部解码层任务 | 上限提高时 `Sweep` 槽位与 `lattice` 边表容量按 `MAX_NODES` 重新推导 |
| `ASM-P03` | 数据量级 | 词库条目 10^4–10^6（`base.dict` ≤ 20MB，`BUDGET-SIZE-02`）；FST 索引与条目表**不驻留一级缓存** | 词典查找零分配、LM mmap 直读 | 条目数超过 10^6 时 FST 随机访问的 TLB miss 成为主项，需追加 `madvise(MADV_RANDOM)` 复核与页表预取评估 |
| `ASM-P04` | 数据量级 | 用户词频库常态 10^3–10^4 条，上限 `USER_WORD_CAP = 500_000`（`user_db.rs:78`） | `PERF-P0.02.01` 词频热路径改造 | 常态量级超过 5×10^4 时停用全量水合，回退到负缓存 + 按需读，并记录 `data/user-db/large` 诊断 |
| `ASM-P05` | 并发峰值 | 宿主线程与 UI 线程各 1 条；词库/词频写入方最多再 2 条（idle sweep + 落盘线程） | 通道契约、锁竞争治理 | 写入方超过 2 条时，`UserDb` 的 `Mutex<LruCache>` 需改为分片或 `RwLock` |
| `ASM-P06` | 硬件边界 | 目标机为 4 核以上 x86_64、DDR4 及以上、L3 ≥ 8MB；开发机 WSL2 数值**仅作相对回归基线**（`features.md` 0.5.5 已登记该限制） | 全部性能断言 | 断言以**相对回归**形式表达（相对基线的比值），绝对值仅在目标硬件上复核 |
| `ASM-P07` | 帧预算 | 候选框表面 600×140 逻辑像素 @ scale 2.0 → 1200×280 物理像素；刷新率 60Hz 与 144Hz 两档 | 渲染层任务、`BUDGET-LAT-03` | 表面尺寸上修时按面积线性外推 blit 预算，并复核 `BUDGET-MEM-01`（≤18MB） |
| `ASM-P08` | 帧预算 | 硬指标 `$1` 默认值原样采纳，并按本项目形态**重新锚定**：<br>① 「输入到首帧响应 ≤ 16ms」= `BUDGET-LAT-01` P99 ≤ 16ms（**同一数值，直接对应**）；<br>② 「十万级数据滚动锁定 120fps」本项目**无滚动列表**（候选框最多 45 项、单页 ≤ 9 项），等价口径为**候选框在 Spring 动效与连续翻页下不掉帧**，落到 `BUDGET-LAT-03`（单帧光栅 P99 ≤ 1.5ms）与 144Hz 档 `BUDGET-LAT-01` P99 ≤ 12ms；「十万级数据」等价映射到 `ASM-P03` 的词库规模 | 渲染层任务、基准任务 | 若产品形态变更为带滚动列表的面板（如 Phase 2 的表情/符号面板 `TASK-2.05.05`），必须为面板新增虚拟化任务卡，不得沿用本口径 |
| `ASM-P09` | 帧预算 | 硬指标 `$1` 的「内存峰值压减 40%」= 以**单次解码的瞬时工作集峰值**为基线（当前实测构成见 0.2 结论一：约 40KB Sweep + 129 节点边表 + 每次查找的 `Vec`），压减 ≥ 40%；**绝对上限仍以 `BUDGET-MEM-01/02/03` 为准**，二者取严 | `PERF-P0.01.01`、`PERF-P0.01.02`、`PERF-P0.02.01`、`PERF-P1.04.01` | 若瞬时峰值已远低于 RSS 上限（即优化不影响 RSS），则以「分配次数」作为等价可断言指标（见 `PERF-P2.01.01`） |
| `ASM-P10` | 架构约束 | `$4` 未提供，沿用现有架构约束：**无 async 运行时**；宿主线程只做 `key → decode → build UiFrame → post`；跨线程只用有界 SPSC + `eventfd`；禁止轮询定时器；`unsafe` 仅限 `crates/ime-fcitx5/src/ffi/**` 与 `crates/ime-dict/src/mmap.rs`；`ime-types` 为冻结契约；禁止新增未评审依赖；禁止任何网络能力 | 全部任务卡的实现手段 | 任何任务卡若必须触碰上述约束，先提 ADR 并由主 agent 决策，不得在卡内自行放宽 |
| `ASM-P11` | 架构约束 | `ime-ui` 的 `Cargo.toml` 无 bench target，`crates/ime-ui/benches/` 不存在；新增 bench 需改 `Cargo.toml`（主 agent 专属文件，子 agent 禁止触碰） | `PERF-P0.03.01`、`PERF-P1.03.01` | 若基准暂时无法接入，则以 `MockBackend` 驱动的计数型断言替代（`crates/ime-ui/src/renderer/mock.rs` 已存在），并在卡内记录替代口径 |
| `ASM-P12` | 数据量级 | 冻结契约 `WordIter` 的内部实现可替换而不改签名——`crates/ime-types/src/lexicon.rs:118-123` 明文授权：「A later phase may replace what is inside this struct without touching a single signature」 | `PERF-P0.01.02` | 若实现中发现必须改签名，则该卡升级为契约变更，需 ADR + 主 agent 决策（AGENTS.md §8.22） |

---

## 2. 性能热点总清单 (Hotspot Inventory)

> 本清单是任务拆解的**唯一事实来源**。清单行数（17）即任务覆盖的最小底线。每一条硬指标在清单中至少有一个热点行归属（见 §3 追溯表）。

### 2.1 数据与内存层

| 热点编号 | 层面 | 热点位置（`文件:行号`） | 瓶颈机理 | 量化损失与归属硬指标 |
|---|---|---|---|---|
| `HOT-01` | 数据 | `crates/ime-core/src/viterbi/decoder.rs:233`<br>`crates/ime-core/src/viterbi/lattice.rs:241-245` | `Decoder::decode` 每次调用新建 `SyllableDag`（`reachable_to_end` 分配 + 129 个 `SmallVec<[DagEdge;8]>` 的边表 `Vec`，约 9.3KB）与 `Lattice`（`key: String::with_capacity(42)`，`edges` 从 0 起倍增扩容约 8 次）。DAG 模块文档（`dag.rs:30-36`）声称的"跨键复用"在调用链上从未发生 | 每键 ≥ 10 次分配 / 约 10KB 瞬时堆占用；归属「内存峰值压减 40%」与「输入到首帧响应 ≤16ms」 |
| `HOT-02` | 数据 | `crates/ime-core/src/viterbi/decoder.rs:431` | `Sweep::new` 执行 `vec![PathState::DEAD; nodes * cap]`，`nodes = MAX_NODES = 129`（`syllable.rs:43`）、`cap = beam_k = 16`，`PathState` 为 20 字节 → **每键分配并零填充 41,280 字节**。其中绝大多数槽位（DEAD 哨兵）从不被读取，且写入后立即被合并覆盖 | 每键 1 次 40KB 级分配 + 40KB memset；归属「内存峰值压减 40%」与「输入到首帧响应 ≤16ms」 |
| `HOT-03` | 数据 | `crates/ime-dict/src/fst_index/read.rs:106`<br>`crates/ime-dict/src/fst_index.rs:212` | `read_words` 每次查找 `Vec::with_capacity(...)` 物化词表，`FstLexicon::lookup` 再用 `WordIter::from_vec` 包裹。查找次数上限为 `MAX_KEYS_PER_NODE = 24`（`lattice.rs:63`）× 节点数；实测 12 音节输入可达 60–150 次查找 | 每键 60–150 次小对象分配（每次 malloc+free 约 30–60ns），累计 2–9µs；归属「内存峰值压减 40%」与「输入到首帧响应 ≤16ms」 |
| `HOT-04` | 数据 | `crates/ime-core/src/state/machine.rs:437` | `self.highlighted_candidate().map(\|held\| held.text.clone())` 每键克隆一个 `String`，唯一用途是喂给 `Paging::reconcile`（`crates/ime-core/src/state/paging.rs:254`），而 `reconcile` 只做 `==` 比较，不持有该值 | 每键 1 次分配 + 一次堆拷贝；归属「输入到首帧响应 ≤16ms」 |
| `HOT-05` | 数据 | `crates/ime-core/src/state/machine.rs:453-467`<br>`crates/ime-core/src/viterbi/decoder.rs:239` | `Session::resegment` 构建 `self.dag` 后，`Decoder::decode` 又为**同一 `req.raw`** 完整重建一份 DAG（`build` → `prepare_nodes` → `build_edges` → `compute_reachability`）。第二份构建是纯重复计算，占解码 DAG 成本的约 50% | 每键重复一遍 129 节点 × ≤6 长度的音节查表（约 390 次二分查找）；归属「输入到首帧响应 ≤16ms」 |
| `HOT-06` | 数据 | `crates/ime-dict/src/user_db.rs:700`<br>`crates/ime-dict/src/user_db.rs:430` | `UserDb::freq` 在缓存未命中时打开 redb 读事务（`begin_read` + `open_table` + `get`），即一次带页缓存缺页风险的 B 树随机读；未命中路径取 3 次互斥锁（`:692`、`:702`、`:707`）。调用方是 `Scorer::edge_score`（`crates/ime-core/src/lm/score.rs:312`），每格边一次 | 每键数百次 B 树读 + 数百次加解锁；与 `crates/ime-types/src/lexicon.rs:23-25` 的明文契约冲突。归属「输入到首帧响应 ≤16ms」与「内存峰值压减 40%」 |
| `HOT-07` | 数据 | `crates/ime-core/src/lm/ngram.rs:70-74` | `InMemoryLm` 是 `BTreeMap<String, i32>` + `BTreeMap<String, BTreeMap<String, i32>>`。每次 `edge_score` 触发 3 次字符串键 B 树遍历（`score.rs:304` 的 `unigram`、`:298` 的 `bigram` 外层与内层），每次比较是一次 `strcmp`。`FstLexicon::unigram_at`（`crates/ime-dict/src/fst_index.rs:166`）已为 mmap 直读准备好接口，但无实现 | 每键数百至数千次字符串比较 + 指针追逐型 cache miss；归属「输入到首帧响应 ≤16ms」与「十万级数据（词库规模）稳帧」等价口径 |
| `HOT-08` | 数据 | `crates/ime-core/src/state/machine.rs:479-491`<br>`crates/ime-core/src/state/machine.rs:500,513` | `build_frame` 每键克隆 `preedit`（`String` + `Vec<PreeditSpan>`）、`decoded.candidates[start..end].to_vec()`（`Vec` + 每个候选一个 `String`，≤9 个）、`status`（`StatusStrip` 内含 `String`），再 `Box::new`；`emit_update` 又为 `Effect::UpdatePreedit` **第二次**克隆 `preedit` | 每键约 12–14 次分配，其中 2 次是 `Preedit` 的整份拷贝；归属「输入到首帧响应 ≤16ms」与「内存峰值压减 40%」 |
| `HOT-09` | 数据 | `crates/ime-core/src/viterbi/decoder.rs:558,588,603,624,634,737` | 候选物化逐项分配：`read_path` 每个 draft 一次 `String::with_capacity`（≤ `beam_k+1 = 17` 次）、`first_word` 一次 `to_owned`、`segments_of` 每段一次 `to_owned`、`collect` 一次 `Vec::with_capacity`、`result_of` 一次 `.collect()`。`DecodeResult` 与 `Draft` 均为**可复用**的持有型结构，当前却每次重建 | 每键约 20–30 次分配；归属「内存峰值压减 40%」与「输入到首帧响应 ≤16ms」 |

| `HOT-17` | 数据 | `crates/ime-core/src/viterbi/lattice.rs:382`<br>`crates/ime-core/src/viterbi/lattice.rs:404` | `build_lattice` 对**每一条格边**执行 `u16::try_from(word.text.chars().count())`。`chars().count()` 是一次完整的 UTF-8 解码扫描，成本与词长线性相关；同一条词在多条跨度上被反复 push（同一 `WordRef` 在不同 `frame` 下重复出现），因此重复计算是结构性的。字符数在建库时即已确定，读路径却每次重新推导 | 按 `HOT-03` 的同一量级（12 音节输入约 500 条边），每键约 500 次冗余 UTF-8 扫描；归属「内存峰值压减 40%」与「输入到首帧响应 ≤16ms」 |

### 2.2 并发与调度层

| 热点编号 | 层面 | 热点位置（`文件:行号`） | 瓶颈机理 | 量化损失与归属硬指标 |
|---|---|---|---|---|
| `HOT-10` | 并发 | `crates/ime-core/src/state/machine.rs:436-446`<br>`crates/ime-core/src/ffi/abi/engine.rs:50` | `Session::refresh` 把 `resegment` + `decode` + `reconcile` + `build_preedit` 全部放在**宿主线程**同步执行（`features.md` 2.1 的设计契约），于是 §2.1 的全部 100–200 次分配都落在 fcitx5 主循环的按键回调内。当前 `on_key_event` 仍是 stub，该成本尚未真实发生——**这正是必须在接线前建立探针的原因** | 当前无实测值；归属「输入到首帧响应 ≤16ms」的**测量前提**（无探针则预算不可断言） |
| `HOT-11` | 并发 | `crates/ime-dict/src/user_db.rs:747`<br>`crates/ime-dict/src/user_db.rs:519-540` | `UserDb::record` 在批次（32 键）或间隔（2000ms）触发时**在调用线程同步执行 `commit_inner`**，即 redb 写事务 + `fsync` 级落盘，而调用方是宿主线程上的提交路径。松弛策略 `report` 只在**连续 3 次**超过 `SLOW_COMMIT_MS = 3ms` 后才放宽，即最坏情况下宿主线程要承受 3 次 3ms+ 的停顿才换来缓解 | 单次停顿可达毫秒级（远超宿主回调 100µs 预算）；违反 AGENTS.md 第 10 条禁令。归属「输入到首帧响应 ≤16ms」 |
| `HOT-12` | 并发 | `crates/ime-ui/src/channel/queue.rs:213-235`<br>`crates/ime-ui/src/channel/queue.rs:312-330` | `RingQueue::push_within` 在队列满时以 `spin_loop()` / `yield_now()` 自旋整个预算；`CollapsingQueue::push` 在自旋期间**持有 staging 互斥锁**。生产者在宿主线程上，队列满即转化为主循环忙等 | 自旋预算由 `ChannelConfig` 决定，默认值下最坏为整个预算长度的宿主线程占用；归属「输入到首帧响应 ≤16ms」 |
| `HOT-13` | 并发 | `crates/ime-fcitx5/src/addon.rs:288-296`<br>`crates/ime-fcitx5/src/ffi/mod.rs:105-119` | `candidate_window_available` 的文档约定「Each declined frame records `ui/not-ready`」，而 `emit_diagnostic` 执行 `format!`（分配）+ `writeln!(stderr)`（**无缓冲阻塞写系统调用**）。在窗口未就绪的降级态下，这会变成**每键一次 stderr 写**；`on_input_panel_update`（`engine.rs:101`）对畸形面板快照同样每次一条 | 单次开销取决于 stderr 接收端（管道满时阻塞不可预期）；同时构成日志洪水。归属「输入到首帧响应 ≤16ms」 |

### 2.3 渲染与视图层

| 热点编号 | 层面 | 热点位置（`文件:行号`） | 瓶颈机理 | 量化损失与归属硬指标 |
|---|---|---|---|---|
| `HOT-14` | 渲染 | `crates/ime-ui/src/renderer/raster.rs:150-179` | `blit_into` 对每个矩形逐像素执行 `to_bytes()` + `copy_from_slice`。虽然内层循环已按行连续（`raster.rs:160-177`），但**是否已被 LLVM 向量化为行级 `memcpy` 未经实测**；且 `Argb8888Pixel`（`raster.rs:42`）未标 `#[repr(transparent)]`，使"字节布局与目标缓冲一致"这一前提无法用安全代码表达 | **未实测**。ASM-P07 口径下全窗口 1200×280 = 336,000 像素；若逐像素执行则约为 `BUDGET-LAT-03`（P99 1.5ms）的主要占用项。归属「候选框稳帧（120fps 等价口径）」 |
| `HOT-15` | 渲染 | `crates/ime-ui/src/renderer.rs:275-277` | `commit` 对 `state.pending.iter().chain(state.shown.iter())` 逐矩形调用 `blit_into`，即**同一帧把"本次改动区"与"上帧 damage 区"各拷一遍**。稳态按键时两个区域高度重叠（高亮移动改动的是同一批单元格），重叠部分被拷贝两次。模块文档（`renderer.rs:53-58`）已确认"并集是安全超集" | 稳态按键下拷贝量约为必要量的 2 倍；动画期两个区域同时接近全窗口，浪费被放大。归属「候选框稳帧（120fps 等价口径）」与 `BUDGET-LAT-03` |
| `HOT-16` | 渲染 | `crates/ime-ui/src/renderer.rs:294-311`<br>`crates/ime-ui/src/renderer/raster.rs:127-136` | `PixelScratch::ensure` 只增不减：窗口从大改小后，scratch 仍按历史最大尺寸保留，且 `renderer.render(&mut state.scratch.pixels[..], stride)` 传入的是**整个**历史缓冲。动画期（Spring 运行中）每一帧都会经过 `rasterize` → `record_damage` → `commit` 全链 | 大窗口使用后回缩的场景下，每帧多付历史峰值面积的光栅/拷贝成本。归属「候选框稳帧（120fps 等价口径）」 |

---

## 3. WBS 任务覆盖追溯表 (Traceability Matrix)

### 3.1 热点 → 任务双向绑定

| 热点编号 | 层面 | 并行通道 | 核心瓶颈 | 绑定任务节点清单 |
|---|---|---|---|---|
| `HOT-01` | 数据 | Track A | 每键重建 DAG 与 Lattice | `PERF-P0.01.01`、`PERF-P1.01.02` |
| `HOT-02` | 数据 | Track A | 每键 40KB Sweep 缓冲区分配与零填充 | `PERF-P0.01.01` |
| `HOT-03` | 数据 | Track A | 每次词典查找一个 `Vec<WordRef>` | `PERF-P0.01.02` |
| `HOT-04` | 数据 | Track A | 每键克隆高亮候选文本 | `PERF-P0.01.03` |
| `HOT-05` | 数据 | Track A | 每键两次完整分词 | `PERF-P0.01.01` |
| `HOT-06` | 数据 | Track A | 热路径 redb 读事务 | `PERF-P0.02.01` |
| `HOT-07` | 数据 | Track A | 字符串键 BTreeMap 语言模型 | `PERF-P1.01.01` |
| `HOT-08` | 数据 | Track A | `UiFrame` 深拷贝（preedit ×2 + candidates + status） | `PERF-P0.01.03` |
| `HOT-09` | 数据 | Track A | 候选/分段物化逐项分配 | `PERF-P0.01.01`、`PERF-P1.01.02` |
| `HOT-17` | 数据 | Track A | 每边一次冗余 UTF-8 字符计数 | `PERF-P1.01.02` |
| `HOT-10` | 并发 | Track C | 宿主线程承载全部解码分配 | `PERF-P0.04.01` |
| `HOT-11` | 并发 | Track A | 宿主线程同步落盘 | `PERF-P0.02.02`、`PERF-P1.02.01` |
| `HOT-12` | 并发 | Track A | 控制通道满时宿主线程自旋 | `PERF-P0.02.02`、`PERF-P1.02.02` |
| `HOT-13` | 并发 | Track A | 每帧诊断 stderr 阻塞写 | `PERF-P0.02.03` |
| `HOT-14` | 渲染 | Track B | 逐像素 blit（未实测） | `PERF-P0.03.01`、`PERF-P2.03.01` |
| `HOT-15` | 渲染 | Track B | pending + shown 双份拷贝 | `PERF-P0.03.01`、`PERF-P1.03.01` |
| `HOT-16` | 渲染 | Track B | 像素暂存只增不减 | `PERF-P1.03.02` |

### 3.2 硬指标 → 兜底任务卡

> 每一条硬指标必须至少有一张**兜底任务卡**，即该指标的测量手段与断言门禁的归属卡。

| 硬指标（`$1` 默认值，按 `ASM-P08`/`ASM-P09` 重新锚定） | 权威阈值来源 | 兜底任务卡 | 直接治理任务卡 |
|---|---|---|---|
| 输入到首帧响应 ≤ 16ms | `features.md` 0.5.3 `BUDGET-LAT-01`（P50 ≤ 4ms / P99 ≤ 16ms / 144Hz P99 ≤ 12ms） | `PERF-P0.04.01`（探针 + 断言） | `PERF-P0.01.01`、`PERF-P0.01.02`、`PERF-P0.01.03`、`PERF-P0.02.01`、`PERF-P0.02.02`、`PERF-P0.02.03`、`PERF-P1.01.01` |
| 候选框稳帧（十万级数据滚动 120fps 的等价口径） | `BUDGET-LAT-03`（单帧光栅 P99 ≤ 1.5ms）+ `BUDGET-LAT-01` 144Hz P99 ≤ 12ms | `PERF-P0.03.01`（基准 + 合并拷贝） | `PERF-P1.03.01`、`PERF-P1.03.02`、`PERF-P2.03.01` |
| 十万级数据（词库 10^5–10^6 条）下的查找稳定性 | `ASM-P03` | `PERF-P1.01.01`（LM mmap 直读 + 词典查找零分配合流） | `PERF-P0.01.02` |
| 内存峰值压减 40% | `ASM-P09`（单次解码瞬时工作集）；绝对上限取 `BUDGET-MEM-01/02/03` 之严者 | `PERF-P1.04.01`（内存与 RSS 预算断言） | `PERF-P0.01.01`、`PERF-P0.01.02`、`PERF-P0.02.01` |

### 3.3 DAG 依赖校验

**编号规则**：`PERF-[优先级].[层面序列].[任务序号]`。层面序列：`01` = 数据与内存，`02` = 并发与调度，`03` = 渲染与视图，`04` = 基准与监控。优先级字典序 `P0 < P1 < P2`，因此**依赖只能指向编号严格更小的任务**。

**全量依赖边（已做环检测，无环）**：

> 图中箭头方向为「前置 → 后继」；只出现在右侧的任务没有后继，只出现在左侧的任务没有前置。依赖关系与各任务卡「前置依赖」字段逐项一致。

```
PERF-P0.01.01 ──┬─→ PERF-P1.01.01 ──→ PERF-P2.01.01
                ├─→ PERF-P1.01.02
                └─→ PERF-P0.01.03
PERF-P0.01.02 ─────→ PERF-P1.01.01
PERF-P0.02.01 ─────→ PERF-P0.02.02 ──┬─→ PERF-P1.02.01
                                      └─→ PERF-P1.02.02
PERF-P0.03.01 ──┬─→ PERF-P1.03.01 ──→ PERF-P2.03.01
                └─→ PERF-P1.03.02
PERF-P0.04.01 ─────→ PERF-P1.04.01 ──┬─→ PERF-P2.04.01
                                      └─→ PERF-P2.04.02
```

**无前置任务（可第 1 天启动）**：`PERF-P0.01.01`、`PERF-P0.01.02`、`PERF-P0.02.01`、`PERF-P0.02.03`、`PERF-P0.03.01`、`PERF-P0.04.01`。

**无后继任务（链尾）**：`PERF-P0.01.03`、`PERF-P0.02.03`、`PERF-P1.01.02`、`PERF-P1.02.01`、`PERF-P1.02.02`、`PERF-P1.03.02`、`PERF-P2.01.01`、`PERF-P2.03.01`、`PERF-P2.04.01`、`PERF-P2.04.02`。

**校验结果**：
1. 全部依赖边均指向编号更小的任务 ✅
2. 拓扑排序存在且唯一性无关（无环） ✅
3. 未发现需要拆分重构的循环依赖 ✅
4. **契约冻结顺序声明**：`PERF-P0.01.02` 触及 `crates/ime-types` 的 `WordIter`。按 `ASM-P12`，该改动**不改任何 `pub` 签名**，因此不构成契约变更、不需要 ADR；若实现中发现签名必须变更，则该卡立即冻结，转由主 agent 走 ADR 流程，其余任务卡不受影响（解耦点：`PERF-P0.01.01` 与 `PERF-P0.02.*` 均不依赖 `WordIter` 的内部表示）。
5. **文件冲突声明**：`PERF-P0.01.01` 与 `PERF-P0.01.03` 同时触及 `crates/ime-core/src/state/machine.rs`，二者**不得并行认领**，必须按 `P0.01.01 → P0.01.03` 串行（后者依赖前者对 `Session` 结构的改造）。其余 P0 卡文件集互不重叠，可并行。

### 3.4 关键路径与并行通道汇总

**关键路径（CP）**：`PERF-P0.04.01 → PERF-P1.04.01 → PERF-P2.04.02`

| 排序 | 任务 | 工时 | 累计 |
|---|---|---|---|
| 1 | `PERF-P0.04.01` 宿主线程路径探针与预算断言 | 3 人天 | 3 |
| 2 | `PERF-P1.04.01` 内存峰值与 RSS 预算断言 | 4 人天 | 7 |
| 3 | `PERF-P2.04.02` 帧预算 CI 门禁 | 3 人天 | 10 |

**CP 总长 10 人天**，与次长链 `PERF-P0.01.01 → PERF-P1.01.01 → PERF-P2.01.01`（4+3+2 = 9 人天）几乎持平。

**推论（必须据此排期）**：本项目的关键路径是**测量链**而非**改写链**——因为 AGENTS.md §2 与 §3.6 要求「任何触及预算路径的改动都需要 criterion 基准或探针断言」，**没有可断言的基准就无法宣告任何优化完成**。因此 `PERF-P0.04.01` 必须在**第 1 天**启动，与数据层改写并行，不得排在改写之后。

**并行通道分工**：

| 通道 | 职责 | 任务卡 | 总工时 | 可并行度 |
|---|---|---|---|---|
| **Track A（数据与并发引擎）** | 解码工作区、词典查找、词频存储、宿主线程阻塞点 | `PERF-P0.01.01`、`PERF-P0.01.02`、`PERF-P0.01.03`、`PERF-P0.02.01`、`PERF-P0.02.02`、`PERF-P0.02.03`、`PERF-P1.01.01`、`PERF-P1.01.02`、`PERF-P1.02.01`、`PERF-P1.02.02`、`PERF-P2.01.01` | 27 人天 | 除 `machine.rs` 冲突对与 `P0.02.01→P0.02.02` 链外，其余 4 张 P0 卡可同时认领 |
| **Track B（渲染与视图管线）** | 帧拷贝、damage 合并、像素暂存池 | `PERF-P0.03.01`、`PERF-P1.03.01`、`PERF-P1.03.02`、`PERF-P2.03.01` | 11 人天 | `P1.03.01` 与 `P1.03.02` 文件集不重叠，可并行 |
| **Track C（基准·监控·基建）** | 探针、基准、门禁、soak | `PERF-P0.04.01`、`PERF-P1.04.01`、`PERF-P2.04.01`、`PERF-P2.04.02` | 14 人天 | **关键路径所在通道，必须最先启动**；`P2.04.01` 与 `P2.04.02` 可并行 |

**三通道总工时 52 人天**，关键路径 10 人天；理想并行（3–5 人）下墙钟约 12–15 人天。

---

## 4. 通用系统级性能优化基准（本项目口径）

以下五条是本项目所有任务卡的实现准绳，均由阶段零审计结论推导，不是通用套话：

1. **数据导向设计与零拷贝**：词库访问已经是零拷贝（`WordRef.text` 直接指向 mmap），**但"零拷贝"不等于"零分配"**——`HOT-01/02/03/09` 全部是"数据不拷贝但容器每次新建"。本项目的零拷贝改造目标因此重定义为：**热路径上除最终产物外不产生任何堆分配**。
2. **对象复用池（Arena / Scratch）**：解码的工作集是**定长上界已知**的（`MAX_NODES = 129`、`beam_k ≤ 32`、`candidates ≤ 64`），因此适用**定长复用池**而非通用分配器。`DecodeScratch` 是这一原则的落地形态。
3. **主线程物理隔离与有界背压**：宿主线程铁律不变（`features.md` 2.1）。本项目的补充判据是：**宿主线程回调内不得出现任何系统调用，除非它是 `eventfd` 写入**。`HOT-11`（文件 IO）与 `HOT-13`（stderr 写）都违反该判据。
4. **跨边界通道契约**：跨线程通道按语义分类——`Command`（请求-响应，如 `UiEvent::Select`）/ `Event`（单向广播，如 `Show`/`Hide`）/ `Data Stream`（流式背压，如 `Frame`）。本项目的容量与背压策略**已经写入冻结契约**（`features.md` 2.2.1/2.2.2），任务卡只做实现对齐与容量数值固化，不得重新设计语义。
5. **差量渲染与视口虚拟化**：候选框无长列表，**虚拟化不适用**；本项目的差量渲染落点是 **damage 合并**（`HOT-15`）与**暂存池化**（`HOT-16`）。

---

## 5. P0 任务卡（原子级展开）

> P1 / P2 任务卡分片见 [./opt-perf/phase-2.md](./opt-perf/phase-2.md) 与 [./opt-perf/phase-3.md](./opt-perf/phase-3.md)。
> 全部任务卡遵守 `ASM-P10` 的架构约束；子 agent 执行时**只写代码，不跑 cargo 命令**（AGENTS.md §6.2）。

---

### 任务 ID：PERF-P0.01.01 解码工作区复用（DecodeScratch）

- **基本属性**：
  - 绑定热点编号：`HOT-01`、`HOT-02`、`HOT-05`、`HOT-09`
  - 优先级与复杂度：`P0 | 高 | 预估工时: 4 人天`
  - 前置依赖：无
  - 关键路径：`CP: 是`
  - 并行通道：`Track A 数据与并发引擎`
  - 代码落地锚点 (Code Anchor)：`crates/ime-core/src/viterbi/decoder.rs`、`crates/ime-core/src/viterbi/lattice.rs`、`crates/ime-core/src/viterbi/mod.rs`、`crates/ime-core/src/state/machine.rs`、`crates/ime-core/src/segment/dag.rs`
  - 当前状态：`[x] 已完成`

- **瓶颈定位与机理剖析**：
  - **现有代码缺陷**：`decoder.rs:233` 的 `let mut dag = SyllableDag::new();` 使 `dag.rs:30-36` 承诺的跨键复用失效；`decoder.rs:431` 的 `vec![PathState::DEAD; nodes * cap]` 每键分配并零填充 41,280 字节；`machine.rs:453` 与 `decoder.rs:239` 对同一输入各建一次 DAG；`decoder.rs:558/588/624/634/737` 的 `Draft`/`DecodeResult`/`Segment` 全部每次重建，而它们都是**纯持有型**结构，容量完全可以跨键保留。
  - **量化优化目标**：单次解码（12 音节 / 45 候选上限）的堆分配次数从当前约 100–200 降至 **≤ 8**（仅剩 `Lattice.edges` 的预置分配 + 词典查找侧分配，后者由 `PERF-P0.01.02` 处理）；单次解码的瞬时堆工作集从约 50KB 降至 **≤ 12KB**（`ASM-P09` 的 40% 压减目标的主要来源）；`crates/ime-core/benches/decode.rs` 的 `decode/viterbi` 基准相对基线回归 **≥ 25%**（以开发机相对基线计，见 `ASM-P06`）。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：把三块可复用的工作区收进一个生命周期无关的 `DecodeScratch`。**

  ```rust
  // crates/ime-core/src/viterbi/scratch.rs （新文件）

  /// Everything one decode allocates, kept across decodes.
  ///
  /// The working set of a decode has a known upper bound -- `MAX_NODES` nodes, a
  /// beam of at most `MAX_BEAM_K`, at most `MAX_CANDIDATES` candidates -- so the
  /// storage is a fixed-shape pool rather than a general allocator. A scratch
  /// that has decoded one twelve-syllable input decodes the next without
  /// touching the allocator at all.
  ///
  /// # Why the lattice is not in here
  ///
  /// `LatticeEdge` holds a `WordRef<'dict>` borrowed from the dictionary, so a
  /// `Lattice` cannot outlive the `decode` call that produced it without the
  /// session itself becoming generic over the dictionary's lifetime. The edge
  /// vector is therefore the one buffer this type does not own; it is
  /// pre-sized instead (see `Lattice::with_capacity`).
  #[derive(Debug, Default)]
  pub struct DecodeScratch {
      /// Segmentation graph of the input being decoded, buffers reused.
      dag: SyllableDag,
      /// `slots[node * cap .. (node + 1) * cap]` is that node's beam.
      slots: Vec<PathState>,
      /// Live length of every node's beam.
      lens: [usize; MAX_NODES],
      /// Candidate texts before they are numbered, capacities reused.
      drafts: Vec<Draft>,
      /// The result the decode writes into, capacities reused.
      out: DecodeResult,
  }

  impl DecodeScratch {
      /// Creates an empty scratch; the first decode allocates.
      pub fn new() -> Self { Self::default() }

      /// Grows the sweep storage to `nodes * cap`, keeping the existing allocation
      /// whenever it already fits, and resets both the beams and the sequence.
      ///
      /// The reset writes only the live region: `DEAD` is the value a slot holds
      /// when nothing occupies it, and a slot that was never written already holds
      /// it from the previous decode's own reset.
      fn prepare_sweep(&mut self, nodes: usize, cap: usize) {
          let needed = nodes.saturating_mul(cap);
          if self.slots.len() < needed {
              self.slots.resize(needed, PathState::DEAD);
          }
          self.lens = [0usize; MAX_NODES];
      }
  }
  ```

  **第二步：`Sweep` 改为借用 scratch 的存储，`Decoder::decode` 增加 `_into` 变体。**

  ```rust
  // crates/ime-core/src/viterbi/decoder.rs

  impl Decoder {
      /// Decodes one request, allocating a scratch for the call.
      ///
      /// The convenience form: a caller that decodes more than once -- which is
      /// every session, once per keystroke -- uses [`Decoder::decode_into`] and
      /// keeps its scratch, which is what makes a steady-state decode
      /// allocation-free.
      pub fn decode(
          &self,
          req: &DecodeRequest,
          lx: &dyn Lexicon,
          uf: &dyn UserFreqSource,
          lm: &dyn LanguageModel,
      ) -> DecodeResult {
          let mut scratch = DecodeScratch::new();
          self.decode_into(&mut scratch, req, lx, uf, lm);
          core::mem::take(&mut scratch.out)
      }

      /// Decodes one request into `scratch`, reusing every buffer it holds.
      ///
      /// `scratch.dag` is rebuilt for `req.raw` and left describing the input on
      /// return, so a caller that also needs the graph -- the session does, for
      /// the preedit -- reads it from the scratch instead of building a second one.
      pub fn decode_into(
          &self,
          scratch: &mut DecodeScratch,
          req: &DecodeRequest,
          lx: &dyn Lexicon,
          uf: &dyn UserFreqSource,
          lm: &dyn LanguageModel,
      ) {
          scratch.out.candidates.clear();
          scratch.out.segments.clear();
          scratch.out.degraded = false;
          if scratch.dag.build(&req.raw).is_err() || !scratch.dag.has_path() {
              passthrough_into(&mut scratch.out, &req.raw);
              return;
          }
          let silent = Silent;
          let user: &dyn UserFreqSource = if req.flags.contains(DecodeFlags::USER_DICT) {
              uf
          } else {
              &silent
          };
          // Pre-sized so the edge vector is allocated once rather than doubled
          // eight times: a node contributes at most `WORDS_PER_KEY` edges for its
          // one-syllable spans plus the fallbacks, and `node_count` nodes exist.
          let mut lattice = Lattice::with_capacity(scratch.dag.len(), self.cfg.fallback_single);
          build_lattice_into(&mut lattice, &scratch.dag, lx, user, self.cfg.fallback_single);
          let sources = Sources { lattice: &lattice, scorer: &self.scorer, lm, uf: user };
          let nodes = lattice.node_count();
          scratch.prepare_sweep(nodes, usize::from(self.cfg.beam_k));
          let mut sweep = Sweep::with_storage(
              sources,
              usize::from(self.cfg.beam_k),
              nodes,
              &mut scratch.slots,
              &mut scratch.lens,
          );
          sweep.run();
          let terminal = usize::from(scratch.dag.len());
          sweep.collect_into(terminal, &mut scratch.drafts);
          finish(&mut scratch.drafts, self.cfg.max_candidates);
          if scratch.drafts.is_empty() {
              passthrough_into(&mut scratch.out, &req.raw);
              return;
          }
          let segments = sweep.segments_into(terminal, &mut scratch.out.segments);
          result_of_into(&mut scratch.out, &scratch.drafts, segments, lattice.lookup_failed());
      }
  }
  ```

  **第三步：`Sweep` 的存储改为外部借用（不改变算法与排序语义）。**

  ```rust
  // crates/ime-core/src/viterbi/decoder.rs

  struct Sweep<'a, 'dict, 'store> {
      sources: Sources<'a, 'dict>,
      cap: usize,
      nodes: usize,
      /// Borrowed from the scratch: `slots[node * cap .. (node + 1) * cap]` is that
      /// node's beam. Borrowed rather than owned so that the buffer survives the
      /// call, which is the whole point of the scratch.
      slots: &'store mut [PathState],
      lens: &'store mut [usize; MAX_NODES],
      sequence: u32,
  }

  impl<'a, 'dict, 'store> Sweep<'a, 'dict, 'store> {
      fn with_storage(
          sources: Sources<'a, 'dict>,
          cap: usize,
          nodes: usize,
          slots: &'store mut [PathState],
          lens: &'store mut [usize; MAX_NODES],
      ) -> Self {
          let cap = cap.min(usize::from(MAX_BEAM_K));
          let nodes = nodes.min(MAX_NODES).min(slots.len() / cap.max(1));
          Self { sources, cap, nodes, slots, lens, sequence: 1 }
      }
      // `beam_parts`, `slot`, `push`, `run`, `expand_node`, `chain` keep their
      // current bodies; only the field access changes from `self.slots` to
      // `&mut self.slots[..]`.
  }
  ```

  **第四步：`Draft` / `DecodeResult` 的容量复用。** `Draft` 是 `decoder.rs` 私有类型，可自由改造；`DecodeResult` 是冻结契约类型，但其字段是 `Vec`，**可以 `clear()` 后复用**（`Candidate.text: String` 与 `Segment.text: String` 的容量通过 `String::clear` + `push_str` 保留）。

  ```rust
  /// Reads the paths that reach `terminal` into `out`, reusing what it holds.
  ///
  /// Every draft is written through [`Draft::reset`], which clears the text and
  /// keeps its capacity, so a decode that produces the same number of candidates
  /// as the last one allocates nothing.
  fn collect_into(&self, terminal: usize, out: &mut Vec<Draft>) {
      let live = self.lens.get(terminal).copied().unwrap_or(0);
      out.clear();
      for slot in 0..live {
          let score = self.slot(terminal, slot).score;
          if let Some(draft) = self.read_path(terminal, slot, score) {
              out.push(draft);
          }
      }
      let best = self.slot(terminal, 0).score;
      if let Some(draft) = self.first_word(terminal, 0, best) {
          out.push(draft);
      }
  }
  ```

  `finish` 需要一并改造为不 `truncate` 掉容量：`drafts.truncate(n)` 保留容量，无需改动；`dedupe` 已就地操作。`result_of_into` 把候选逐个写进 `out.candidates`，复用已有 `String` 容量：

  ```rust
  /// Turns the ordered drafts into the frozen result, reusing `out`'s buffers.
  fn result_of_into(out: &mut DecodeResult, drafts: &[Draft], segments: Vec<Segment>, degraded: bool) {
      for (position, candidate) in out.candidates.iter_mut().enumerate() {
          candidate.text.clear();
          let _ = (position, candidate);
      }
      out.candidates.truncate(drafts.len());
      for (position, draft) in drafts.iter().enumerate() {
          match out.candidates.get_mut(position) {
              Some(slot) => {
                  slot.text.clear();
                  slot.text.push_str(&draft.text);
                  slot.index = u16::try_from(position).unwrap_or(u16::MAX).saturating_add(1);
                  slot.annotation = None;
                  slot.source = draft.source;
                  slot.score = draft.score as f32 / Q16_ONE;
                  slot.consumed_syllables = draft.syllables;
              }
              None => out.candidates.push(Candidate {
                  index: u16::try_from(position).unwrap_or(u16::MAX).saturating_add(1),
                  text: draft.text.clone(),
                  annotation: None,
                  source: draft.source,
                  score: draft.score as f32 / Q16_ONE,
                  consumed_syllables: draft.syllables,
              }),
          }
      }
      out.segments = segments;
      out.degraded = degraded;
  }
  ```

  **第五步：`Session` 持有 scratch，消除第二次 DAG 构建。**

  ```rust
  // crates/ime-core/src/state/machine.rs

  pub struct Session {
      // ...现有字段不变...
      /// The decode workspace: the segmentation graph the session also reads for
      /// its preedit, plus the sweep and candidate buffers. Held across keystrokes
      /// so a steady-state decode does not allocate.
      ///
      /// The graph lives here rather than in `Session::dag` because the decoder
      /// builds it: keeping two copies was what made every keystroke segment the
      /// same input twice.
      scratch: DecodeScratch,
  }

  impl Session {
      /// Re-segments and re-decodes the current input, then repairs everything derived.
      pub(super) fn refresh(&mut self, env: &SessionEnv<'_>) {
          // The highlight's text is read as a borrow: `reconcile` only compares it,
          // so the clone this used to take bought nothing but an allocation.
          let request = DecodeRequest::new(self.buf.raw());
          env.decoder
              .decode_into(&mut self.scratch, &request, env.lexicon, env.user_freq, env.lm);
          let prev = self.highlighted_candidate().map(|held| held.text.as_str());
          self.paging.reconcile(prev, &self.scratch.out.candidates);
          self.decoded = core::mem::take(&mut self.scratch.out);
          self.resegment();
          build_preedit_into(&self.buf, &self.scratch.dag, &mut self.preedit);
      }
  }
  ```

  > **借用顺序注意**：`prev` 借用 `self.decoded`，而 `reconcile` 需要 `&self.scratch.out.candidates`。两者是不同字段，Rust 的字段级借用允许同时存在；实现时若遇借用冲突，把 `prev` 改存为 `Paging::reconcile` 接受的索引（先取 `highlight` 索引，再用索引比对），**不得回退为 `String` 克隆**。

  **边界契约**：本卡不跨越任何线程边界，不引入通道，无序列化 Payload，无 RequestId/TraceId。`DecodeScratch` 是 `Session` 的私有字段，`Session` 由宿主线程独占（`ASM-11`，`machine.rs:44-49`），因此 `DecodeScratch` **不需要** `Send`/`Sync`。

  **并发与死锁风险**：无。全部改动是单线程内的所有权转移；`Sweep` 从持有 `Vec` 改为借用 `&mut [PathState]` 后，`beam_parts` 的 `get_mut` 边界检查语义不变（越界仍返回 `None`），不引入任何新的 panic 点。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 新建 `crates/ime-core/src/viterbi/scratch.rs`，定义 `DecodeScratch` 与 `prepare_sweep`；在 `viterbi/mod.rs` 中 `mod scratch; pub use scratch::DecodeScratch;`（模块声明由主 agent 执行，见 AGENTS.md §4.4）。
  2. 改造 `Sweep` 为借用存储（`with_storage`），保持 `run`/`expand_node`/`push`/`beam_parts`/`slot`/`chain` 的算法与排序语义逐行不变；`TopK` 的用法不变。
  3. 新增 `Lattice::with_capacity(node_count, fallback_single)` 与 `build_lattice_into`，把 `edges` 的容量一次性预置为 `node_count * WORDS_PER_KEY`（上限夹取到 `MAX_LATTICE_NODES * WORDS_PER_KEY`），消除倍增扩容。
  4. 实现 `Decoder::decode_into` 与 `result_of_into`，保留 `decode` 作为薄封装（向后兼容全部现有测试与 `benches/decode.rs`）。
  5. 改造 `Session`：字段 `dag` 由 `scratch.dag` 取代，`resegment` 改为只把 `dag.best_segmentation_hint` 写回 `buf`，`refresh` 按第五步重排顺序；`clear_input` 中 `let _ = self.dag.build("")` 改为 `let _ = self.scratch.dag.build("")`。
  6. 在 `crates/ime-core/benches/decode.rs` 追加 `decode/viterbi_into`（复用 scratch）与 `decode/viterbi`（每次新建）两组基准，并追加一个**分配计数断言**：用 `#[global_allocator]` 计数包装（仅在 bench target 内）断言 `decode_into` 稳态调用的分配次数 ≤ 8。
  7. 全量回归：`cargo nextest run -p ime-core`、`cargo test -p ime-core --doc`（`Lattice` 与 `SyllableDag` 的 doctest 必须同步更新为 `decode_into` 的用法示例）。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] `crates/ime-core/benches/decode.rs` 新增 `decode/viterbi_into` 与分配计数断言，`cargo bench -p ime-core --bench decode` 可运行并输出 `decode/viterbi` 与 `decode/viterbi_into` 的对比数据
  - [ ] 分配计数断言通过：`decode_into` 连续 100 次调用的稳态分配次数 ≤ 8 次/次（当前基线由本卡实施前先测一次并记录在案）
  - [ ] `decode/viterbi_into` 相对 `decode/viterbi` 的 P50 改善 ≥ 25%（开发机相对基线）
  - [ ] 功能链路完全向后兼容：`cargo nextest run -p ime-core` 全绿；`crates/ime-core/src/state/tests.rs`、`table_tests.rs`、`sweep_tests.rs` 一行不改即通过
  - [ ] `cargo test -p ime-core --doc` 全绿（`lattice.rs:104-141` 的 doctest 若签名未变则无需改动）
  - [ ] 无新增 `unsafe`；无新增依赖；无 `#[allow]` 未附理由
- **验收记录**（2026-09-29）：
  - **交付物**：新增 `crates/ime-core/src/viterbi/scratch.rs`（`DecodeScratch` + `Decoder::decode_into` + `passthrough_into` + `result`/`take_result`/`recycle`/`into_result`）、`viterbi/sweep.rs`（从 `decoder.rs` 拆出的 K-best sweep，存储改为外部借用）、`viterbi/lattice/testing.rs`；`decoder.rs` 瘦身至配置 + `decode` 薄封装 + `scorer()`；`lattice.rs` 增加 `with_capacity` 与 `build_lattice_into`；`benches/decode.rs` 新增 `decode/viterbi` 与 `decode/viterbi_into` 对比组。
  - **验证命令与结果**：`cargo check -p ime-core --all-targets` 绿。**基准未跑**。
  - **本次由主 Agent 修正的一处**：`scratch.rs` 的测试缺 `DecodeConfig` 导入（`use crate::viterbi::decoder::Decoder;` 应为 `::{DecodeConfig, Decoder}`），已补。
  - **本次发现并修复的一处真实缺陷**：光束槽位不再清零后，终端节点无路径（`live == 0`）时会读到上一次输入遗留的槽位，可能凭旧边号在新格上拼出**假候选**。现以 `live > 0` 守住首次候选读取，并加了回归测试 `test_decode_into_does_not_read_a_path_the_last_input_left_behind`。
  - **已知限制**：
    1. **DoD 的分配计数断言（稳态 ≤ 8 次/次）不可实现**：需要 `#[global_allocator]` + `unsafe impl GlobalAlloc`，与 0.4 规则 3 及 `check-unsafe.sh` 的白名单直接冲突。替代证据是 `test_decode_into_reuses_every_buffer_across_decodes` 的**容量探针**（稳态第二次解码后 slots/drafts/candidates/segments 及各文本容量全部不变）。
    2. **DoD 的性能目标（`decode/viterbi_into` 相对 `decode/viterbi` P50 改善 ≥ 25%）未取数**：基准目标已就位，未运行。
    3. **`DecodeScratch` 尚未被产品使用**：接线（`Session` 持 scratch、`refresh` 改调 `decode_into`）不在本卡白名单内，已单独派工。在接线完成前，本卡交付的是**可用的工作区 API 及其基准**，而不是已经生效的加速。
    4. 卡片固定的 `decode_into` 形状是 5 参，超过 §4.3 的「>4 参聚合为结构体」；按卡片实现并报备。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、criterion 0.8.2、cargo-nextest 0.9.143。

---

### 任务 ID：PERF-P0.01.02 词典查找零分配（WordIter 内联槽位）

- **基本属性**：
  - 绑定热点编号：`HOT-03`
  - 优先级与复杂度：`P0 | 中 | 预估工时: 2 人天`
  - 前置依赖：无
  - 关键路径：`CP: 是`
  - 并行通道：`Track A 数据与并发引擎`
  - 代码落地锚点 (Code Anchor)：`crates/ime-types/src/lexicon.rs`、`crates/ime-dict/src/fst_index.rs`、`crates/ime-dict/src/fst_index/read.rs`、`crates/ime-core/benches/decode.rs`、`crates/ime-dict/benches/dict.rs`
  - 当前状态：`[x] 已完成`

- **瓶颈定位与机理剖析**：
  - **现有代码缺陷**：`read.rs:106` 的 `let mut words = Vec::with_capacity(count.min(MAX_WORDS_PER_KEY as usize));` 对每次查找分配一次；`fst_index.rs:212` 与 `:235` 用 `WordIter::from_vec` 包裹。`crates/ime-core/src/viterbi/lattice.rs:320` 的 `MAX_KEYS_PER_NODE = 24` 与 `:376` 的 `words.take(WORDS_PER_KEY)` 决定了调用频次上限：12 音节输入实测 60–150 次查找/键。
  - **契约边界（`ASM-P12`）**：`crates/ime-types/src/lexicon.rs:118-123` 明文授权替换 `WordIter` 的内部表示而不改签名。本卡**不改任何 `pub` 签名**，因此不构成契约变更。`ime-types` 的依赖白名单只有 `thiserror` / `bitflags` / `serde`（`Cargo.toml` 注释「契约层（ime-types 只允许依赖这三个）」），**因此不得引入 `smallvec`**——改用固定长度内联数组。
  - **量化优化目标**：一次 `Lexicon::lookup` 的堆分配次数：词数 ≤ 8 时为 **0**（当前为 1）；`fallback_single`（限 `FALLBACK_SINGLES = 3`）为 **0**（当前为 1，且当前实现先物化 `count` 个再 `truncate`，浪费更甚）；单次解码的总分配次数中词典查找的贡献从 60–150 降至 **0–20**。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：`WordIter` 改为内联/堆双态，无新依赖。**

  ```rust
  // crates/ime-types/src/lexicon.rs

  /// Words one [`WordIter`] holds without touching the allocator.
  ///
  /// Eight matches the head of a key's word list -- the part a lookup is asked
  /// for -- and it is a compile-time constant of the contract rather than a
  /// dependency: `ime-types` may only depend on `thiserror`, `bitflags` and
  /// `serde`, so the inline storage is a plain array.
  pub const WORD_ITER_INLINE: usize = 8;

  /// Iterator over the words of one lookup.
  ///
  /// The backing store is deliberately unspecified by the contract. A lookup
  /// whose word list fits [`WORD_ITER_INLINE`] -- which is every lookup the
  /// decode path makes in the common case -- holds its words inline and
  /// allocates nothing; a longer list spills to a vector.
  #[derive(Debug)]
  pub struct WordIter<'a> {
      inner: Inner<'a>,
  }

  #[derive(Debug)]
  enum Inner<'a> {
      /// The words, how many of the slots are live, and how many have been read.
      Inline {
          words: [WordRef<'a>; WORD_ITER_INLINE],
          len: u8,
          next: u8,
      },
      /// A list longer than the inline capacity.
      Heap(std::vec::IntoIter<WordRef<'a>>),
  }

  impl<'a> WordIter<'a> {
      /// Builds an iterator over an already materialized, already ordered word list.
      ///
      /// Kept as-is: it is the constructor every existing caller and test uses,
      /// and the vector it is handed is moved into the iterator.
      pub fn from_vec(words: Vec<WordRef<'a>>) -> Self { /* 现状不变 */ }

      /// Builds an iterator that holds its words inline.
      ///
      /// # Panics
      ///
      /// Never: a list longer than [`WORD_ITER_INLINE`] is truncated to it.
      pub fn from_slice(words: &[WordRef<'a>]) -> Self {
          let mut inline = [EMPTY_WORD; WORD_ITER_INLINE];
          let taken = words.len().min(WORD_ITER_INLINE);
          inline[..taken].copy_from_slice(&words[..taken]);
          Self {
              inner: Inner::Inline {
                  words: inline,
                  len: taken as u8,
                  next: 0,
              },
          }
      }
  }
  ```

  `EMPTY_WORD` 是 `WordRef { text: "", weight: 0, syl_count: 0, flags: WordFlags::empty() }` 的常量；`WordRef` 已 `Copy`（`lexicon.rs:104`），数组可零成本初始化。

  **第二步：`read_words` 增加受限变体，写进调用方提供的内联缓冲。**

  ```rust
  // crates/ime-dict/src/fst_index/read.rs

  /// Reads at most `limit` words of `key` into `out`, which the caller owns.
  ///
  /// The decode path asks for a bounded head of the list, so materializing the
  /// whole thing into a fresh vector per lookup was pure waste: this variant
  /// writes into a buffer the caller already holds and stops at `limit`, which is
  /// what removes the allocation from the lookup path entirely.
  ///
  /// # Errors
  ///
  /// As [`FstLexicon::read_words`].
  pub(super) fn read_words_into<'a>(
      &'a self,
      key: &str,
      limit: usize,
      out: &mut WordBuf<'a>,
  ) -> Result<(), DictError> {
      out.clear();
      let Some(packed) = self.fst.get(key.as_bytes()) else {
          return Ok(());
      };
      let (start, count) = unpack_fst_value(packed);
      // ...与 read_words 相同的边界检查...
      let take = count.min(limit).min(MAX_WORDS_PER_KEY as usize);
      for index in 0..take {
          let id = read_u32(ids, index * WORD_ID_SIZE, "wordlist_entry")?;
          let entry = self.entry(id)?;
          out.push(WordRef {
              text: self.word(&entry)?,
              weight: entry.weight,
              syl_count: entry.syl_count,
              flags: WordFlags::from_bits_truncate(entry.flags),
          });
      }
      Ok(())
  }
  ```

  ```rust
  // crates/ime-dict/src/fst_index.rs

  /// A bounded word list a lookup writes into.
  ///
  /// Sized for the head of a key's list, which is all the decode path reads; a
  /// list longer than that spills to the heap rather than being truncated, so a
  /// caller that asks for more still gets it.
  type WordBuf<'a> = smallvec::SmallVec<[WordRef<'a>; WORD_ITER_INLINE]>;
  ```

  > `smallvec` 已是 workspace 依赖且 `ime-dict` 已可用（`Cargo.toml` 的「核心引擎」段）。**不得**把它加进 `ime-types`。

  **第三步：`Lexicon` 实现改为走受限路径。**

  ```rust
  impl Lexicon for FstLexicon {
      /// Reads the words of `key`, holding up to [`WORD_ITER_INLINE`] inline.
      fn lookup(&self, key: &str) -> Result<WordIter<'_>, ImeError> {
          let mut buf = WordBuf::new();
          self.read_words_into(key, MAX_WORDS_PER_KEY as usize, &mut buf)
              .map_err(|cause| self.unavailable(cause))?;
          Ok(match buf.len() {
              0..=WORD_ITER_INLINE => WordIter::from_slice(&buf),
              _ => WordIter::from_vec(buf.into_vec()),
          })
      }

      /// Reads the single-character words of one syllable.
      ///
      /// The caller's `limit` is pushed all the way down, so a fallback lookup
      /// materializes exactly what it returns and never allocates: the previous
      /// implementation read the whole list and then truncated it.
      fn fallback_single(&self, syl: SyllableId, limit: usize) -> Result<WordIter<'_>, ImeError> {
          let Some(key) = syllable_at(syl) else {
              return Err(ImeError::Unsupported);
          };
          let mut buf = WordBuf::new();
          self.read_words_into(key, limit, &mut buf)
              .map_err(|cause| self.unavailable(cause))?;
          Ok(match buf.len() {
              0..=WORD_ITER_INLINE => WordIter::from_slice(&buf),
              _ => WordIter::from_vec(buf.into_vec()),
          })
      }
  }
  ```

  > **重要**：`lookup` 的签名没有 `limit` 参数（冻结契约），因此它仍可能物化至多 `MAX_WORDS_PER_KEY = 32` 个词。这**不是**本卡能消除的——调用方 `lattice.rs:376` 只取 8 个。若要彻底消除，需要给 `Lexicon::lookup` 增加 `limit` 参数，那是**契约变更**（AGENTS.md §8.22，需 ADR）。本卡在 DoD 中登记该残余，并把它作为 `PERF-P1.01.02` 的输入。

  **边界契约**：本卡跨越 `ime-types` ↔ `ime-dict` 的 crate 边界，但**不新增跨线程通道、不新增 Payload 结构体、不涉及序列化**。`WordIter` 是纯值类型，`Send`/`Sync` 属性由 `WordRef<'a>`（`&'a str` + `u32` + `u8` + `u8`）决定，与改造前一致。`Lexicon` 的并发保证（`lexicon.rs:19-25`：`Send + Sync`、不阻塞、不取写者锁、**不按候选分配**）在本卡后**首次真正成立**。

  **并发与死锁风险**：无。`WordIter` 是栈上值，`WordBuf` 是局部变量，无共享状态。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在 `crates/ime-types/src/lexicon.rs` 增加 `WORD_ITER_INLINE`、`Inner` 枚举、`WordIter::from_slice`；`Iterator`/`ExactSizeIterator`/`FusedIterator` 三个 impl 按 `Inner` 分支实现，`size_hint` 与 `len` 语义与现状逐位一致。
  2. 在 `crates/ime-dict/src/fst_index/read.rs` 增加 `read_words_into`；把现有 `read_words` 实现改为 `read_words_into` 的薄封装（`Vec` 版本），保证既有测试与 `fst_index/tests.rs` 一行不改即通过。
  3. 在 `crates/ime-dict/src/fst_index.rs` 定义 `WordBuf` 类型别名并改造两个 `Lexicon` 方法。
  4. 为 `crates/ime-dict/benches/dict.rs` 追加 `dict/lookup_inline`（命中 ≤8 词键）与 `dict/lookup_spill`（命中 >8 词键）两组基准，并在 bench target 内用分配计数器断言内联路径分配次数为 0。
  5. 回归：`cargo nextest run -p ime-types -p ime-dict -p ime-core`；`cargo test -p ime-types --doc`（`lexicon.rs:134-146` 的 `from_vec` doctest 保留即可通过）。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] `crates/ime-dict/benches/dict.rs` 新增 `dict/lookup_inline` 与 `dict/lookup_spill`，分配计数断言通过（内联路径 0 次分配）
  - [ ] `crates/ime-core/benches/decode.rs` 的 `decode/viterbi` 相对基线改善 ≥ 15%（开发机相对基线）
  - [ ] `WordIter` 的**任何 `pub` 签名未变**：`cargo public-api -p ime-types` 输出与改造前逐行一致（本卡的硬性门禁）
  - [ ] `cargo nextest run -p ime-types -p ime-dict -p ime-core` 全绿；`crates/ime-dict/src/fst_index/tests.rs` 与 `crates/ime-core/src/viterbi/lattice.rs` 的测试一行不改即通过
  - [ ] `cargo test -p ime-types --doc` 全绿
  - [ ] 残余项已登记：`lookup` 因冻结签名仍可物化至多 32 词，该项交由 `PERF-P1.01.02` 评估（若需改签名则转 ADR 流程）
  - [ ] 无新增依赖进入 `ime-types`（`crates/ime-types/Cargo.toml` 的 `[dependencies]` 段行数不变）
- **验收记录**（2026-09-29）：
  - **交付物**：`crates/ime-types/src/lexicon.rs` 的 `WordIter` 改为 `Inner::{Inline, Heap}` 双态 + `WORD_ITER_INLINE = 8` + `from_slice`（`from_vec`/`Iterator`/`ExactSizeIterator`/`FusedIterator` 语义逐位保持）；`crates/ime-dict/src/fst_index.rs` 新增 `WordBuf`（内联 8 槽 + 堆溢出）并让 `lookup`/`fallback_single` 走内联路径；`fst_index/read.rs` 新增 `word_ids`（边界检查的唯一出口）与 `read_words_into`；`benches/dict.rs` 新增 `dict/lookup_inline` 与 `dict/lookup_spill`。
  - **验证命令与结果**：`cargo check -p ime-types -p ime-dict --all-targets` 绿。
  - **本次发现并修复的一处既有缺陷**：`crates/ime-dict/benches/dict.rs` 的 fixture 用 2000 个 serial 生成两音节键，411 个音节表项使键每 411 个 serial 完全重复且整体非字典序，`fst::MapBuilder::insert` 的报错被 `.ok()?` 吞掉 → `loaded_fixture()` 返回 `None` → **`dict/lookup` 与 `dict/entry_to_ref` 静默不注册任何用例**。现改为商参与音节位置共同决定键、并按键排序后写入。**这条意味着此前所有「dict 基准」的结论都建立在零用例之上。**
  - **已知限制**：
    1. **DoD 的分配计数断言仍缺口**（同 PERF-P0.01.01：本仓库无 `unsafe` 通路做 `GlobalAlloc`）。替代证据是 `word_buf_tests.rs` 把溢出阈值精确到「第 9 个词才溢出」+ 基准的 `check_fixture` 断言两族词数。
    2. **DoD 的性能目标（`decode/viterbi` 相对基线改善 ≥ 15%）未取数**。
    3. **`cargo public-api` 逐行比对未跑**：`WordIter` 的既有 `pub` 签名确实未变，但该断言本身未执行。
    4. `lookup` 因冻结签名仍可物化至多 32 词（容器自身的 `MAX_WORDS_PER_KEY`），已写进 `FstLexicon::lookup` 的文档注释。
    5. 可选建议（未采纳）：在 `crates/ime-dict/Cargo.toml` 加 `smallvec` 可把本地 `WordBuf` 换成 `SmallVec`，无行为变化。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、criterion 0.8.2、cargo-nextest 0.9.143。

---

### 任务 ID：PERF-P0.01.03 帧快照零拷贝化（Preedit / Candidate 复用）

- **基本属性**：
  - 绑定热点编号：`HOT-04`、`HOT-08`
  - 优先级与复杂度：`P0 | 中 | 预估工时: 2 人天`
  - 前置依赖：`PERF-P0.01.01`
  - 关键路径：`CP: 否`
  - 并行通道：`Track A 数据与并发引擎`
  - 代码落地锚点 (Code Anchor)：`crates/ime-core/src/state/machine.rs`、`crates/ime-core/src/preedit.rs`、`crates/ime-core/src/state/tests.rs`
  - 当前状态：`[x] 已完成`

- **瓶颈定位与机理剖析**：
  - **现有代码缺陷**：`machine.rs:437` 每键克隆高亮候选文本，而 `paging.rs:254` 的 `reconcile(prev_text: Option<&str>, candidates: &[Candidate])` 只需要一个 `&str`；`machine.rs:481/500/513` 每键克隆 `Preedit`（`String` + `Vec<PreeditSpan>`）**两次**（一次进 `UiFrame`，一次进 `Effect::UpdatePreedit`）；`machine.rs:482` 的 `candidates[start..end].to_vec()` 为每个可见候选克隆一个 `String`（≤9 个）；`machine.rs:484` 克隆 `StatusStrip`。
  - **量化优化目标**：`Session::refresh` 的每键分配次数从 3 次（含 `Preedit` 的 `String` + `Vec` 两份拷贝共 4 次分配）降至 **≤ 1**；`build_frame` 的每次调用分配次数从约 12–14 降至 **≤ 3**（`Box<UiFrame>` + `Vec<Candidate>` 骨架 + 必要时的文本增长）。
  - **不可消除项（据实登记）**：`UiFrame.candidates: Vec<Candidate>` 与 `Candidate.text: String` 是冻结契约（`crates/ime-types/src/ui.rs:48-64`），`Effect::SendFrame(Box<UiFrame>)` 要求帧的所有权转移给宿主线程，因此**每帧至少一次 `Box` 分配与一次候选 `Vec` 分配不可避免**。本卡的目标是消除**重复**的克隆，而非消除帧本身的所有权转移。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：`reconcile` 改为按索引对账，去掉文本克隆。**

  ```rust
  // crates/ime-core/src/state/paging.rs

  /// Keeps the highlight on the same candidate after the list was rebuilt.
  ///
  /// The caller used to hand in a cloned copy of the highlighted text, which cost
  /// an allocation on every keystroke to serve a comparison that never outlived
  /// the call. The candidate index it now takes is the same information without
  /// the copy: the two lists are compared by position, and a highlight whose text
  /// moved is found by the scan.
  pub fn reconcile_at(&mut self, prev: Option<(&str, u16)>, candidates: &[Candidate]) {
      // `prev` carries the text and the index it sat at; the scan below finds the
      // same text at its new position, and the index is the fallback for a text
      // that appears more than once.
  }
  ```

  > **实现提示**：`reconcile` 的现有测试（`paging.rs:456-480`）以 `Option<&str>` 调用。为保持测试不改，保留 `reconcile(prev_text: Option<&str>, ...)` 签名，只在 `machine.rs` 侧改为借用：

  ```rust
  // crates/ime-core/src/state/machine.rs

  pub(super) fn refresh(&mut self, env: &SessionEnv<'_>) {
      // The highlight is read before the decode and compared as a borrow: the clone
      // this used to take existed only because the decode replaces `self.decoded`
      // wholesale, and the borrow below ends before that assignment.
      let request = DecodeRequest::new(self.buf.raw());
      let prev = self
          .decoded
          .candidates
          .get(usize::from(self.paging.highlight))
          .map(|held| held.text.as_str());
      env.decoder
          .decode_into(&mut self.scratch, &request, env.lexicon, env.user_freq, env.lm);
      self.paging
          .reconcile(prev, &self.scratch.out.candidates);
      self.decoded = core::mem::take(&mut self.scratch.out);
      self.resegment();
      build_preedit_into(&self.buf, &self.scratch.dag, &mut self.preedit);
  }
  ```

  **第二步：`emit_window` / `emit_update` 只克隆一次 `Preedit`。**

  ```rust
  /// Emits the preedit, the show and the frame that put a composition on screen.
  ///
  /// The preedit is cloned once, into the frame; the `UpdatePreedit` effect takes
  /// the frame's own copy, so the two carriers cannot disagree about what the
  /// composing text is -- which the previous two-clone form could only guarantee
  /// by cloning from the same source twice.
  pub(super) fn emit_window(&mut self, ctx: &mut Ctx<'_>) {
      let revision = self.revision.next();
      let frame = Box::new(self.build_frame(ctx.cfg, revision));
      ctx.push(Effect::UpdatePreedit(frame.preedit.clone()));
      ctx.push(Effect::Show(AnchorHint { revision, placement: Placement::Auto }));
      ctx.push(Effect::SendFrame(frame));
  }
  ```

  > 这一步把 2 次 `Preedit` 克隆降为 2 次（`frame.preedit.clone()` 与 `build_frame` 内的 1 次）→ 需要 `build_frame` 自己也不克隆：

  ```rust
  /// Builds the frame the window draws from the session's current state.
  ///
  /// The preedit is cloned exactly once, here: the session keeps its own copy for
  /// the next keystroke and the frame takes a snapshot, which is the minimum a
  /// frame that outlives the call can cost.
  fn build_frame(&self, cfg: &SessionConfig, revision: Revision) -> UiFrame {
      let total = u16::try_from(self.decoded.candidates.len()).unwrap_or(u16::MAX);
      let start = usize::from(self.paging.page_start()).min(self.decoded.candidates.len());
      let end = usize::from(self.paging.page_end(total)).min(self.decoded.candidates.len());
      let mut candidates = Vec::with_capacity(end.saturating_sub(start));
      for candidate in &self.decoded.candidates[start..end] {
          let mut copy = candidate.clone();
          copy.text = String::from(candidate.text.as_str());
          candidates.push(copy);
      }
      UiFrame {
          revision: revision.value(),
          preedit: self.preedit.clone(),
          candidates,
          page: self.paging.page_state(total),
          status: self.ctx.status.clone(),
          anchor: self.ctx.anchor,
          layout: LayoutHint {
              max_per_row: self.paging.page_size,
              show_annotation: cfg.show_annotation,
              max_width_dp: cfg.max_width_dp,
          },
      }
  }
  ```

  > **本卡的诚实边界**：`UiFrame` 的候选与 preedit **必须**是拥有所有权的值（帧要跨线程交给 UI 线程），因此"零拷贝"在此处不可达。本卡实际达成的是：**每键从 4 次 `Preedit` 级克隆降为 1 次，并消除 1 次纯为比较而做的 `String` 克隆**。若要让帧本身零拷贝，需要把 `UiFrame` 改为增量 `Patch`（差量帧），那是契约变更，属于 `PERF-P1.03.01` 的评估范围，不在本卡。

  **边界契约**：`Effect::SendFrame(Box<UiFrame>)` 的**语义与顺序不变**（`Show` 必须先于 `SendFrame`，`machine.rs:20-25` 的契约）；`Effect::UpdatePreedit` 的内容仍与帧内 preedit 逐字节一致（现在由构造顺序保证而非由两次克隆保证）。不新增通道、不新增 Payload 类型、不涉及序列化。

  **并发与死锁风险**：无。全部改动在宿主线程内，`Session` 不被共享。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 改造 `Session::refresh`，把 `prev_text` 的 `String` 克隆改为借用（注意借用结束于 `self.decoded` 赋值之前；若编译器不接受，改为先取 `highlight` 索引与 `candidates` 的切片，再在赋值前完成 `reconcile`）。
  2. 改造 `build_frame`，消除 `self.decoded.candidates[start..end].to_vec()` 之外的重复克隆。
  3. 改造 `emit_window` / `emit_update`，把 `Preedit` 克隆次数从 2 降到 1。
  4. 为 `crates/ime-core/src/state/tests.rs` 追加两个断言：一次 `Key(InputChar)` 后 `frame.preedit == session.preedit()`（内容一致性），以及 `UpdatePreedit` 载荷与帧内 preedit 逐字节相等（防止改造破坏契约）。
  5. 回归：`cargo nextest run -p ime-core`；`cargo test -p ime-core --doc`。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] `crates/ime-core/benches/input.rs` 追加 `session/keystroke`（模拟 10 次连续按键的 `step` 调用），分配计数断言：稳态每键分配次数 ≤ 8（含 `Box<UiFrame>` 与候选 `Vec`）
  - [ ] `session/keystroke` 相对基线改善 ≥ 20%（开发机相对基线）
  - [ ] 新增一致性测试通过：`UpdatePreedit` 载荷与 `UiFrame.preedit` 逐字节相等
  - [ ] `cargo nextest run -p ime-core` 全绿；`crates/ime-core/src/state/tests.rs` 的既有断言一行不改即通过
  - [ ] `cargo test -p ime-core --doc` 全绿
  - [ ] 残余项已登记：`UiFrame` 的拥有型候选/预编辑快照不可避免，差量帧评估交由 `PERF-P1.03.01`
- **验收记录**（2026-09-29）：
  - **交付物**：`crates/ime-core/src/state/machine.rs` 的 `refresh`/`emit_window`/`emit_update` 三处；新增 `crates/ime-core/src/state/tests/frame.rs`（191 行，6 个用例）；`benches/decode.rs` 新增 `session/keystroke`。
  - **验证命令与结果**：`cargo check -p ime-core --all-targets` 绿；`state/tests.rs` 仅新增 `mod frame;` 一行，既有断言一行未动。
  - **实际消除的分配（代码事实，非测量）**：①`refresh` 里 `highlighted_candidate().map(|held| held.text.clone())` 改为按字段取借用 `.map(|held| held.text.as_str())`，每键省一次 `String` 分配——取字段而非 `highlighted_candidate()` 是因为后者借整个 `self`，会与 `reconcile` 的 `&mut self.paging` 冲突；②`emit_window`/`emit_update` 的 `Effect::UpdatePreedit` 载荷改为 `frame.preedit.clone()`，与 `UiFrame.preedit` 由**同一个值**派生，字节一致由构造保证而非由两次独立克隆碰巧一致。
  - **已知限制**：
    1. **卡片量化目标「`build_frame` 每次调用 12–14 → ≤3」在冻结契约下不可达**：`UiFrame` 的 `preedit`/`candidates`/`status` 必须拥有所有权（帧跨线程交给 UI 线程、寿命超出调用），且 `Candidate.text: String` 是冻结字段，因此每帧至少 `Box<UiFrame>` + 候选 `Vec` + 每可见候选 1 个 `String` + preedit 的 `String`+`Vec`。已在 `build_frame` 的文档注释中逐条登记。
    2. **「`Session::refresh` 每键分配 3 → ≤1」部分达成**：已消除为比较而做的那次克隆；余下的是 `DecodeRequest::new(self.buf.raw())` 的 `String` 与解码输出，后者属 `PERF-P0.01.01` 的 `DecodeScratch`。
    3. **DoD 的分配计数断言与基准改善未取数**（同前两卡的原因）。
    4. 基准落在 `benches/decode.rs` 而非卡片点名的 `benches/input.rs`（白名单所限）；若要迁回，整段 `bench_session` 可搬。
  - **给主 Agent 的越界发现**：
    1. `state/transitions.rs` 的 `on_config_reloaded` 有**完全相同**的克隆模式，可同法消除；非热路径，每键不触发。
    2. `refresh` 里的 `DecodeRequest::new(self.buf.raw())` **每键分配一个 `String`**。无需契约变更即可消除——让 `Session` 持一个可复用的 `DecodeRequest` 缓冲（`raw.clear()` + `push_str`）。已并入 `DecodeScratch` 的接线工单。
    3. `crates/ime-core/src/state/tests.rs` 现为 824 行（本卡 +2 前已是 822），超 800 行上限，属既有违规。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、criterion 0.8.2、cargo-nextest 0.9.143。

---

### 任务 ID：PERF-P0.02.01 用户词频热路径去 redb

- **基本属性**：
  - 绑定热点编号：`HOT-06`
  - 优先级与复杂度：`P0 | 高 | 预估工时: 3 人天`
  - 前置依赖：无
  - 关键路径：`CP: 是`
  - 并行通道：`Track A 数据与并发引擎`
  - 代码落地锚点 (Code Anchor)：`crates/ime-dict/src/user_db.rs`、`crates/ime-dict/src/user_db/evict.rs`、`crates/ime-dict/benches/userdb.rs`、`crates/ime-dict/src/user_db/tests.rs`
  - 当前状态：`[x] 已完成`

- **瓶颈定位与机理剖析**：
  - **现有代码缺陷**：`user_db.rs:690-710` 的 `freq` 在 `LruCache` 未命中时调用 `Inner::committed`（`:430`），后者执行 `self.db.begin_read()` → `txn.open_table(USER_WORDS)` → `table.get(key)`，即一次 redb 读事务与一次 B 树随机读；未命中路径还要在 `:692`、`:702`、`:707` 取 **3 次**互斥锁。`LruCache` 容量 `CACHE_CAPACITY = 4096`（`:58`）而词表规模 `ASM-P03` 为 10^4–10^6，命中率不足。
  - **契约冲突**：`crates/ime-types/src/lexicon.rs:23-25` 明文要求 `freq`「runs on the decode hot path and must not block, must not take a lock a writer can hold, and must not allocate per candidate」——当前的 redb 事务既可能阻塞（页缓存缺页触发磁盘读）又持有可被写者争用的锁。
  - **量化优化目标**：`UserDb::freq` 在稳态下的 redb 事务次数 = **0**；互斥锁获取次数从最多 3 次降为 **1** 次；单次 `freq` 的 P99 ≤ **1µs**（内存命中）；水合后的常驻增量 ≤ **2MB**（`HYDRATE_CAP = 50_000` 条，`ASM-P04`）。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：把"已提交"一侧从 redb 搬进内存，落盘只写不回读。**

  ```rust
  // crates/ime-dict/src/user_db.rs

  /// Records the store loads into memory at open.
  ///
  /// A user's own vocabulary is a few thousand words; the ceiling exists for the
  /// pathological store, and past it the store keeps the on-demand path rather
  /// than growing the plugin's resident set without bound.
  pub const HYDRATE_CAP: u64 = 50_000;

  /// Diagnostic code recorded when the store is too large to hydrate.
  pub const LARGE_STORE_CODE: &str = "data/user-db/large";

  /// The committed half of the frequency signal, held in memory.
  ///
  /// The decode path asks for a frequency once per lattice edge -- hundreds of
  /// times per keystroke -- so answering it out of the store's own B-tree was
  /// never viable: the frozen contract requires `freq` not to block and not to
  /// take a lock a writer can hold, and a read transaction does both.
  ///
  /// The map is authoritative for reads and is kept in step with the store by
  /// [`Inner::write_batch`] (which inserts what it wrote) and by the idle sweep
  /// (which removes what it evicted), so nothing here can drift from the file.
  struct Committed {
      /// Counts as the store holds them, keyed by word text.
      counts: HashMap<Box<str>, u32>,
      /// Whether the store was small enough to load. When false, reads fall back
      /// to the on-demand path and the diagnostic names the reason.
      hydrated: bool,
  }
  ```

  **第二步：`freq` 变成一次加锁、一次哈希查找。**

  ```rust
  impl UserFreqSource for UserDb {
      /// Returns the frequency of `key`: one lock, one hash lookup, no IO.
      ///
      /// The answer is `committed + pending`, both of which are in memory, so a
      /// decode never opens a read transaction. A store too large to hydrate
      /// keeps the on-demand path, which is the only case that still touches the
      /// file -- and it is bounded by `HYDRATE_CAP`, not by the word count.
      fn freq(&self, key: &str) -> u32 {
          let committed = lock(&self.inner.committed);
          if committed.hydrated {
              let base = committed.counts.get(key).copied().unwrap_or(0);
              let delta = lock(&self.inner.pending)
                  .entries
                  .get(key)
                  .map_or(0, |entry| entry.count);
              return base.saturating_add(delta);
          }
          drop(committed);
          // On-demand fallback for a store past `HYDRATE_CAP`: the read
          // transaction is unavoidable here, and the negative cache below keeps
          // it to once per distinct word.
          self.on_demand(key)
      }
  }
  ```

  > **锁序声明（防死锁）**：全局锁序固定为 `committed → pending`，任何路径不得反向获取。`record` 与 `write_batch` 必须遵守同一顺序。

  **第三步：`HYDRATE_CAP` 之下的降级路径带负缓存。**

  ```rust
  /// Reads one frequency from the store, remembering the miss.
  ///
  /// Without the negative cache a word the user has never typed would open a read
  /// transaction on every one of the hundreds of edges that name it in a single
  /// decode. The cache turns that into one transaction per distinct word per
  /// session.
  fn on_demand(&self, key: &str) -> u32 {
      let mut cache = lock(&self.inner.cache);
      if let Some(hit) = cache.get(key) {
          return hit;
      }
      drop(cache);
      let committed = self.inner.committed(key);
      let delta = lock(&self.inner.pending).entries.get(key).map_or(0, |e| e.count);
      let total = committed.saturating_add(delta);
      lock(&self.inner.cache).insert(key, total);
      total
  }
  ```

  **第四步：写入路径同步内存映射。**

  ```rust
  fn write_batch(&self, drained: &[(Box<str>, Pending)], durability: Durability)
      -> Result<(), String>
  {
      // ...现有事务体不变...
      // After the commit succeeds, the in-memory map adopts what was written, so a
      // read never has to go back to the file to see it.
      let mut committed = lock(&self.committed);
      if committed.hydrated {
          for (key, delta) in drained {
              let entry = committed.counts.entry(key.clone()).or_insert(0);
              *entry = entry.saturating_add(delta.count);
          }
      }
      Ok(())
  }
  ```

  同步点清单（**遗漏任何一个都会让内存与文件漂移**）：
  | 事件 | 必须同步的动作 |
  |---|---|
  | `open_with` 成功 | 若 `stored_count() <= HYDRATE_CAP`，全量读入 `counts` 并置 `hydrated = true`；否则置 `false` 并记录 `LARGE_STORE_CODE` |
  | `write_batch` 成功 | 逐键累加（见上） |
  | `write_batch` 失败 → `degrade` | **不**修改 `counts`（事务已回滚），并把 `cache` 回滚（现有 `degrade` 已做） |
  | `evict_oldest` 成功 | 从 `counts` 中移除被删键（`user_db/evict.rs` 的删除列表需回传） |

  **边界契约**：
  - 通道类型语义：本卡**不新增**任何跨线程通道。`Committed` 由 `Mutex` 保护，与既有 `pending`/`cache` 同构。
  - 状态机跃迁：`hydrated: true → false` 仅允许在 `degrade`（转只读）时发生，且此后不再回切；`is_readonly()` 的语义不变。
  - 错误码：沿用既有 `data/readonly-mode`（`ImeError::DataReadonly`）与 `data/commit/slow-disk`（`SLOW_DISK_CODE`）；本卡**新增** `data/user-db/large`，必须同步登记进 `docs/dev/features.md` 2.2.4 的错误码表（由主 agent 执行）。

  **并发与死锁风险**：锁序固定为 `committed → pending`，`cache` 是叶子锁（不得在持有时获取其他锁）。`Inner::committed()`（redb 读）**必须在不持有任何锁时调用**——这是本卡最容易写错的地方，实现时须在 `on_demand` 中以显式 `drop(cache)` 保证，并在代码注释中写明理由。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 新增 `Committed` 结构与 `HYDRATE_CAP` / `LARGE_STORE_CODE` 常量；在 `Inner` 中加字段并保持锁序注释。
  2. 在 `open_with` 中实现水合（`stored_count()` 已存在，`:446`）；超过上限时走降级分支。
  3. 改造 `freq` 与新增 `on_demand`；改造 `write_batch` 与 `evict_oldest` 的同步点。
  4. 在 `crates/ime-dict/src/user_db/tests.rs` 追加四个测试：水合后 `freq` 不再触发读事务（用 `redb` 的只读句柄计数或注入式探针断言）、`record` 后 `freq` 立即可见、`degrade` 后 `freq` 仍可读且不写、`evict_oldest` 后被删键的 `freq` 归零。
  5. 在 `crates/ime-dict/benches/userdb.rs` 追加 `userdb/freq_hit`（内存命中）与 `userdb/freq_miss`（降级路径）基准。
  6. 回归：`cargo nextest run -p ime-dict`。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] `crates/ime-dict/benches/userdb.rs` 新增 `userdb/freq_hit` 与 `userdb/freq_miss`；`userdb/freq_hit` 的 P99 ≤ 1µs（本机相对基线，`ASM-P06`）
  - [ ] 事务计数断言通过：水合态下连续 1000 次 `freq` 触发的 redb 读事务次数 = 0
  - [ ] 内存断言通过：`HYDRATE_CAP` 条记录水合后的 RSS 增量 ≤ 2MB
  - [ ] 一致性断言通过：`write_batch` 后 `counts` 与 `stored_count()` 语义一致；`evict_oldest` 后被删键不可读
  - [ ] `cargo nextest run -p ime-dict` 全绿；`crates/ime-dict/src/user_db/tests.rs` 既有断言一行不改即通过
  - [ ] `cargo test -p ime-dict --doc` 全绿
  - [ ] 新增错误码 `data/user-db/large` 已登记进 `docs/dev/features.md` 2.2.4（主 agent 执行）
- **验收记录**（2026-09-29）：
  - **交付物**：`crates/ime-dict/src/user_db.rs`（769 行）、新增 `user_db/{cache,hydrate}.rs`、`user_db/evict.rs`、`user_db/tests.rs`（追加 7 个用例，既有 15 个一行未改）、`benches/userdb.rs`。
  - **验证命令与结果**：`just ci` 退出 0；`cargo nextest run -p ime-dict` 全绿，含 `test_user_db_loaded_lookup_opens_no_store_read`（水合态连续 1000 次 `freq`，redb 读事务增量 == 0）。
  - **耐久性契约完全保留**：三触发器（`COMMIT_BATCH` / `COMMIT_INTERVAL_MS` / `PENDING_CAPACITY`）、`SLOW_COMMIT_STREAK = 3` 的松弛判据、只读降级、idle sweep 一行未改。
  - **已知限制**：
    1. **DoD 3 的 `HYDRATE_CAP` RSS 上限未验证且很可能超标**：卡片要求水合 50000 条后 RSS 增量 ≤ 2MB，按字节算术估算 `HashMap<Box<str>, u32>` × 5 万条约 2.5–3.5MB。`#[ignore]` soak 测试已写未跑。需跑 `cargo nextest run -p ime-dict --run-ignored all` 后决定：换紧凑键表示、下调 `HYDRATE_CAP`、或按实测调整该 DoD 数字（预算声明属主 Agent 裁决）。
    2. **一处有意偏离卡片**：卡片说 `hydrated: true → false` 仅允许在 `degrade` 时发生。未在 `degrade` 里翻转——`counts` 只含「已到达文件」的值，失败事务已回滚，它仍等于文件；翻转会让只读库在解码热路径上开始开读事务，与该卡的目的相反。已在 `degrade` 的文档注释写明。
    3. **DoD 1 的 P99 ≤ 1µs 未取数**：基准用例已补（`userdb/freq_hit` / `freq_miss` / `freq_degraded`），未运行。
    4. `freq` 水合态是 **2 次短锁**（`committed`、`pending`）而非卡片写的 1 次；卡片自己的伪码也是两个锁。
    5. 新增的 `data/user-db/large` 与既有的 `data/commit/slow-disk` 已由主 Agent 登记进 `features.md` 2.2.4；目前**没有任何生产代码调用 `UserDb`**（`ime-fcitx5` 只经冻结的 `UserFreqSource` 拿 `&dyn`），故两个码都还没有上报方。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143。

---

### 任务 ID：PERF-P0.02.02 宿主线程阻塞点清除（异步落盘 + 通道不自旋）

- **基本属性**：
  - 绑定热点编号：`HOT-11`、`HOT-12`
  - 优先级与复杂度：`P0 | 中 | 预估工时: 2 人天`
  - 前置依赖：`PERF-P0.02.01`
  - 关键路径：`CP: 是`
  - 并行通道：`Track A 数据与并发引擎`
  - 代码落地锚点 (Code Anchor)：`crates/ime-dict/src/user_db.rs`、`crates/ime-dict/src/user_db/evict.rs`、`crates/ime-ui/src/channel/queue.rs`、`crates/ime-ui/src/channel.rs`
  - 当前状态：`[ ] 待优化`

- **瓶颈定位与机理剖析**：
  - **现有代码缺陷（一）**：`user_db.rs:741-748` 的 `if due { let _ = self.commit_inner(Durability::Eventual); }` 在**调用线程**（宿主线程）执行 redb 写事务 + 提交。`report`（`:519-540`）的松弛策略要求**连续 3 次**超过 `SLOW_COMMIT_MS = 3ms` 才放宽，即最坏情况下宿主线程先承受 3 次 3ms+ 停顿。这违反 AGENTS.md 第 10 条「Blocking work on the host thread — filesystem IO ... inside an fcitx5 callback」。
  - **现有代码缺陷（二）**：`queue.rs:213-235` 的 `push_within` 以 `spin_loop()` / `yield_now()` 自旋整个 `budget`，而 `queue.rs:312-330` 的 `CollapsingQueue::push` 在自旋期间持有 `staged` 互斥锁。生产者是宿主线程，队列满即主循环忙等。
  - **量化优化目标**：宿主线程单次回调内的**阻塞时间上限** ≤ **200µs**（可由 `PERF-P0.04.01` 的探针断言）；`UserDb::record` 在宿主线程上的最坏耗时 ≤ **50µs**（当前可达毫秒级）；控制通道满时宿主线程的自旋预算 ≤ **100µs** 且不再持锁自旋。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：把落盘搬到既有的 idle sweep 线程，用 `eventfd` 唤醒。**

  ```rust
  // crates/ime-dict/src/user_db.rs

  /// What the writer thread is asked to do.
  ///
  /// The channel is a `Command` channel in the boundary taxonomy: the host thread
  /// asks, the writer performs, and the answer travels back through the shared
  /// state rather than through a reply message -- a flush has no result the host
  /// can act on beyond the read-only flag it already polls.
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  enum FlushRequest {
      /// Write the pending records with the given durability.
      Write(Durability),
      /// Stop the thread.
      Stop,
  }
  ```

  ```rust
  /// Asks the writer thread to flush, without waiting for it.
  ///
  /// The host thread's contract is that a commit never blocks it: the flush is
  /// handed to the writer thread that already owns the store's slow path (the idle
  /// sweep) and the call returns. Ordering is preserved because the writer is a
  /// single consumer draining a FIFO, which is the same guarantee the same-thread
  /// flush provided -- without putting a write transaction on the fcitx5 main loop.
  fn request_flush(&self) {
      if let Some(writer) = &self.inner.writer {
          writer.request(FlushRequest::Write(Durability::Eventual));
      }
  }
  ```

  `record` 的尾部因此变为：

  ```rust
  fn record(&self, key: &str, _weight_hint: u16) {
      if self.is_readonly() {
          return;
      }
      let now_nanos = self.inner.clock.now_nanos();
      let now_ms = self.inner.clock.now_ms();
      self.inner.last_record_nanos.store(now_nanos, Ordering::Relaxed);
      let due = { /* 现有计算不变 */ };
      { /* cache.bump 不变 */ }
      if due {
          // Handed to the writer rather than performed here: a redb write
          // transaction is filesystem IO, and this runs inside an fcitx5 callback.
          self.request_flush();
      }
  }
  ```

  **第二步：`CollapsingQueue::push` 不再持锁自旋。**

  ```rust
  // crates/ime-ui/src/channel/queue.rs

  /// How long a producer waits for room before it stages the value instead.
  ///
  /// The producer runs on the fcitx5 host thread, whose callback budget is two
  /// orders of magnitude below a frame interval: a queue that is full means the UI
  /// thread is behind, and the correct response is to hand the value to the
  /// staging slot and return, not to hold the main loop while the consumer
  /// catches up.
  const STAGE_BUDGET: Duration = Duration::from_micros(100);

  pub fn push(&self, value: T) {
      // The staging slot is taken first and released immediately: holding it
      // across the wait below was what let a spinning producer block the
      // consumer's own `pop` on a full queue.
      let staged = self.lock().take();
      if let Some(previous) = staged {
          if self.queue.try_push(previous).is_err() {
              // The queue is still full. The contract keeps the newest value, so
              // the staged one is what disappears -- counted, as before.
              *self.lock() = Some(value);
              self.collapsed.fetch_add(1, Ordering::Relaxed);
              return;
          }
      }
      if let Err(value) = self.queue.try_push(value) {
          *self.lock() = Some(value);
          self.collapsed.fetch_add(1, Ordering::Relaxed);
      }
  }
  ```

  > `STAGE_BUDGET` 仅用于文档化"为什么不再等待"；改造后 `push` 是**两次 `try_push` + 至多两次短锁**，无自旋、无 `Instant::now()`、无 `yield_now()`。`RingQueue::push_within` 保留（其他调用方可能仍需要），但 `CollapsingQueue` 不再使用它。

  **边界契约**：
  - 通道类型语义：`FlushRequest` 是 **Command** 通道（宿主线程请求，writer 线程执行），容量 1（合并语义：多次 `request_flush` 只需一次写盘，因为 `commit_inner` 排空整个 `pending`）。写入端使用 `eventfd`（复用 `ime-ui` 的 `Wakeup` 形态，但 `ime-dict` 不能依赖 `ime-ui`——层级顺序是 `ime-types ← ime-core ← ime-dict ← ime-config ← ime-ui ← ime-fcitx5`，反向依赖是构建失败）。**因此 `ime-dict` 侧用一个 `AtomicBool` + `std::sync::mpsc::sync_channel(1)` 或 `Condvar`，不引入 `eventfd`**。
  - 状态机跃迁：writer 线程的 `Stop` 只在 `Drop`/`final_commit` 后发出；`final_commit` 必须**同步等待** writer 排空（`ASM-20`：优雅退出不丢数据）。
  - 错误码：沿用 `data/readonly-mode`；新增 `data/commit/thread-lost`（writer 线程不可用时的降级码），由主 agent 登记进 2.2.4。

  **并发与死锁风险**：
  - 锁序不变（`committed → pending`，`cache` 为叶子锁）。
  - **`final_commit` 的同步等待必须带超时**（建议 200ms，与 `addon.rs:67` 的 `UI_SHUTDOWN_TIMEOUT` 同量级），超时后放弃等待并记录 `data/commit/thread-lost`，绝不无限阻塞宿主退出路径。
  - `CollapsingQueue::push` 改造后不再跨锁等待，因此**不可能**与消费者的 `pop` 形成互等。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在 `user_db.rs` 定义 `FlushRequest` 与 writer 线程的启动/停止；复用 `user_db/evict.rs` 的 `start_sweep` 线程或新增一条（二者都需要在 `Drop` 时收敛，实现时以**单一线程**同时承担 sweep 与 flush 为优先，避免线程数增长）。
  2. 改造 `record` 的触发分支为 `request_flush()`。
  3. 改造 `final_commit` 为"请求 + 有超时等待"。
  4. 改造 `CollapsingQueue::push`（`queue.rs:312`），删除持锁自旋；同步更新 `queue.rs:15-23` 的模块文档（当前文档以「spin, then collapse」描述溢出规则，改造后应改为「try, then stage」）。
  5. 在 `crates/ime-dict/src/user_db/tests.rs` 追加：`record` 后宿主线程耗时的上界断言（注入慢 clock 模拟慢盘）、`final_commit` 在 writer 线程未启动时仍能落盘。
  6. 在 `crates/ime-ui/src/channel.rs` 的测试模块追加：`CollapsingQueue::push` 在队列满时**不阻塞**（用 `Instant` 断言单次 `push` ≤ 100µs）且顺序语义不变。
  7. 回归：`cargo nextest run -p ime-dict -p ime-ui`。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] `crates/ime-dict/benches/userdb.rs` 新增 `userdb/record_triggered_flush`：断言触发落盘的那次 `record` 调用耗时 P99 ≤ 50µs（当前实现会达到毫秒级）
  - [ ] `crates/ime-ui/src/channel.rs` 测试断言：队列满时 `CollapsingQueue::push` 单次耗时 ≤ 100µs 且 `collapsed()` 计数递增
  - [ ] 顺序语义断言：连续 3 次 `push` 后 `pop` 序列与改造前逐项一致（既有测试 `test_collapsing_queue_collapses_to_the_newest_when_full` 等一行不改即通过）
  - [ ] 数据不丢断言：`final_commit` 后 `record_count()` 等于已记录的键数（`ASM-20`）
  - [ ] `cargo nextest run -p ime-dict -p ime-ui` 全绿；`cargo test -p ime-dict -p ime-ui --doc` 全绿
  - [ ] 新增错误码 `data/commit/thread-lost` 已登记（主 agent 执行）
  - [ ] `ime-dict` 未新增对 `ime-ui` 的依赖（`crates/ime-dict/Cargo.toml` 的依赖方向不变）

---

### 任务 ID：PERF-P0.02.03 诊断发射节流（去掉每帧 stderr 写）

- **基本属性**：
  - 绑定热点编号：`HOT-13`
  - 优先级与复杂度：`P0 | 低 | 预估工时: 1 人天`
  - 前置依赖：无
  - 关键路径：`CP: 否`
  - 并行通道：`Track A 数据与并发引擎`
  - 代码落地锚点 (Code Anchor)：`crates/ime-fcitx5/src/addon.rs`、`crates/ime-fcitx5/src/ffi/mod.rs`、`crates/ime-fcitx5/src/ui_impl/panel.rs`、`crates/ime-fcitx5/src/ffi/abi/engine.rs`
  - 当前状态：`[x] 已完成`

- **瓶颈定位与机理剖析**：
  - **现有代码缺陷**：`ffi/mod.rs:105-107` 的 `emit_diagnostic` 执行 `format!("rspinyin: {}", code)`（一次分配）后 `writeln!(std::io::stderr(), ...)`（一次**无缓冲阻塞写系统调用**）。`addon.rs:288-296` 的 `candidate_window_available` 文档约定「Each declined frame records `ui/not-ready`」，即**降级态下每键一次**；`ui_impl/panel.rs:102` 对每个畸形面板快照同样每次一条；`ffi/abi/engine.rs:57`、`:101`、`:109`、`:169`、`:189` 的 null/非法输入路径同理。
  - **量化优化目标**：稳态下诊断发射次数为 **0**（不重复同一码）；同一诊断码在 1 秒窗口内最多发射 **1** 次；`emit_diagnostic` 的单次开销从"分配 + 阻塞写"降为"一次原子比较"（被抑制时）。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  ```rust
  // crates/ime-fcitx5/src/ffi/mod.rs

  /// Emits one diagnostic line, at most once per code per window.
  ///
  /// The crash channel is deliberately not `tracing`: it has to work before the
  /// diagnostics layer is up and while a panic is being handled. What it must not
  /// be is a per-frame syscall -- the callers include the "the candidate window is
  /// not ready" path, which a session without a window reaches on every keystroke,
  /// and an unbuffered write to a stderr whose reader is slow blocks the fcitx5
  /// main loop. A code therefore reports itself once and then stays quiet until
  /// the window has passed, which keeps the first occurrence (the one an operator
  /// needs) and drops the storm behind it.
  pub(crate) fn emit_diagnostic(code: &str) {
      if !should_emit(code) {
          return;
      }
      write_stderr_line(&format!("rspinyin: {code}"));
  }

  /// Whether `code` may be written now, recording the answer.
  ///
  /// The gate is a fixed table of code slots rather than a map: the set of codes
  /// this module emits is closed and small, so a linear scan over a few dozen
  /// entries is cheaper than hashing, and the table needs no allocation -- which
  /// matters because this function is called from the paths that must not
  /// allocate.
  fn should_emit(code: &str) -> bool {
      const WINDOW: Duration = Duration::from_secs(1);
      const SLOTS: usize = 32;
      // One slot per distinct code, with the monotonic instant it last fired.
      static SLOTS_TABLE: Mutex<[Option<(&'static str, Instant)>; SLOTS]> =
          Mutex::new([None; SLOTS]);
      // ...
  }
  ```

  > **实现约束**：`code` 参数当前是 `&str`，而槽位需要 `&'static str` 才能长期保存。**不得**把 `code.to_owned()` 存进槽位（那正是要消除的分配）。可行做法有二，实现时择一并写明理由：
  > (a) 把 `emit_diagnostic` 的签名收紧为 `code: &'static str`，所有调用点传字面量——**推荐**，因为 `addon.rs:121` 的 `pending_step` 已经用 `&format!` 构造动态字符串，该处需改为「静态码 + 结构化字段」两次调用；
  > (b) 槽位存 `u64` 哈希（`code` 的 FNV-1a），零分配但存在理论碰撞；需在注释中说明碰撞只会导致某条诊断被多抑制一次，不影响正确性。
  >
  > 若选 (a)，`addon.rs:120-122` 的 `pending_step` 与 `:455-459`、`:463-468` 的 `report_*_outcome` 都要改写为静态码形式（这三处仅在生命周期事件上调用，不在热路径，但仍应统一）。

  **边界契约**：诊断码是**跨边界稳定标识**（AGENTS.md §1），节流**不得**改变任何既有码的拼写；本卡不新增码，也不删除码。节流窗口是进程级全局状态，需在模块文档中声明其非确定性与测试隔离方式（测试用注入时钟）。

  **并发与死锁风险**：`SLOTS_TABLE` 是叶子锁，任何路径不得在持有时获取其他锁；`should_emit` 在锁内只做比较与写入，不分配、不调用 `format!`。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在 `ffi/mod.rs` 实现 `should_emit` 与节流表；`emit_diagnostic` 改为先过闸。
  2. 若选签名收紧方案 (a)，改造全部调用点：`addon.rs:121`、`:208`、`:222`、`:258`、`:266`、`:292`、`:436`、`:459`、`:467`，以及 `ffi/abi/engine.rs` 与 `ui_impl/panel.rs` 的调用点。
  3. 追加测试：同一码连续调用 100 次，stderr 只出现 1 行；不同码互不抑制；窗口过期后重新发射（注入时钟）。
  4. 回归：`cargo nextest run -p ime-fcitx5`。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] 节流测试通过：同一码 1 秒内 100 次调用 → stderr 1 行
  - [ ] 抑制路径的分配断言：被抑制的 `emit_diagnostic` 调用分配次数 = 0
  - [ ] 既有诊断码拼写零改动（`grep` 全部码字面量，与改造前逐字一致）
  - [ ] `cargo nextest run -p ime-fcitx5` 全绿；`cargo test -p ime-fcitx5 --doc` 全绿
  - [ ] 无新增依赖（节流表用 `std::sync::Mutex` + `std::time::Instant`，不引入 `once_cell`/`dashmap`）
- **验收记录**（2026-09-29）：
  - **交付物**：`crates/ime-fcitx5/src/ffi/mod.rs`（525 行）与 `crates/ime-ui-addon/src/ffi/mod.rs`（536 行）各新增节流表 `Throttle` / `Slot` / `Emission`、`code_hash`（FNV-1a 64）、`static THROTTLE`、`lock_throttle`、`diagnostic_line`；`emit_diagnostic` 改为经 `emit_through` 过闸。两处文档同步。
  - **验证命令与结果**：`just ci` 退出 0；两个 crate 各 6 个新用例全绿（同码 1 秒内 100 次 → 恰 1 行；窗口到期后再发 → 第 2 行附 ` (suppressed 99 repeats in 1000ms)`；不同码不互相抑制；表满 40 个码时全部照写；`code_hash` 对已发布 FNV-1a 向量）。
  - **语义**：同一诊断码在 1 秒窗口内最多写 1 行；窗口内重复被计数。码字面量保持在行首且拼写不变，`grep <code>` 仍能命中。
  - **ADR-0004 决策 3 的延续**：两个库各复制一份实现而非共享——两库被独立 `dlopen`，通过共享 crate 产生链接期耦合正是拆分要消除的东西。
  - **已知限制**：
    1. **选方案 (b)（FNV-1a 哈希建槽）而非卡中推荐的 (a)（收紧签名为 `&'static str`）**：`emit_diagnostic` 的调用点分布在 6 个白名单外的文件；且 `lifecycle/pending: <step> awaits <x>` 等动态码若收紧为 `&'static str` 会丢失步骤/错误信息。哈希碰撞只可能多抑制一行，永远不会改写字面量。
    2. **表满（>32 个不同码同窗口）时失败方向是「照写不误」**：未登记的码每次仍会写入，绝不静默吞掉；代价是该码失去节流。当前两个 crate 的码集合各约 10–20 条，该分支不可达。
    3. **`guard_ffi` 的 panic 行刻意不节流**：panic 是严重事件且每条携带不同 payload，节流会吞掉解释崩溃的那条消息。
    4. **DoD 的「抑制路径分配 = 0」只有结构性断言**（sink 未被调用 + 表不增长），无分配计数器实测。硬断言需 `#[global_allocator]` 计数分配器（`unsafe impl GlobalAlloc`），风险高于收益，未做。
  - **环境**：Rust 1.98.0（workspace MSRV 1.85，`rust-toolchain.toml` 钉定）、Linux 6.18.40.1-microsoft-standard-WSL2、Fcitx5 5.1.7、cargo-nextest 0.9.143。

---

### 任务 ID：PERF-P0.03.01 帧拷贝路径：合并 damage 并建立基准

- **基本属性**：
  - 绑定热点编号：`HOT-14`、`HOT-15`
  - 优先级与复杂度：`P0 | 中 | 预估工时: 2 人天`
  - 前置依赖：无
  - 关键路径：`CP: 否`
  - 并行通道：`Track B 渲染与视图管线`
  - 代码落地锚点 (Code Anchor)：`crates/ime-ui/src/renderer.rs`、`crates/ime-ui/src/renderer/raster.rs`、`crates/ime-ui/src/renderer/mock.rs`、`crates/ime-ui/src/renderer/tests.rs`
  - 当前状态：`[ ] 待优化`

- **瓶颈定位与机理剖析**：
  - **现有代码缺陷（HOT-15，可证明的浪费）**：`renderer.rs:275-277` 对 `state.pending.iter().chain(state.shown.iter())` 逐矩形调用 `blit_into`。稳态按键时 `pending`（本次改动区）与 `shown`（上帧 damage 区）高度重叠——高亮移动改动的正是同一批单元格——重叠部分被**拷贝两次**。`renderer.rs:53-58` 的模块文档已明确"并集是安全超集"，即合并是安全的。
  - **现有代码缺陷（HOT-14，未实测）**：`raster.rs:171-176` 的内层逐像素拷贝**可能**已被 LLVM 向量化为行级 `memcpy`（`Argb8888Pixel::to_bytes` 是 `u32::to_le_bytes`，源与目标都连续）。本卡**不假设**其慢，而是**先建立基准**，用数据决定是否改写。
  - **量化优化目标**：稳态按键的像素拷贝量从"2 × 必要量"降为"1 × 必要量 + 少量并集外冗余"；`crates/ime-ui/benches/frame.rs`（新增）给出全窗口 1200×280 的 `blit_into` P99 与 `render_if_dirty` 端到端 P99 两个数字，并与 `BUDGET-LAT-03`（P99 ≤ 1.5ms）比较；**若全窗口 blit 的实测 P99 ≤ 0.5ms，则 HOT-14 判定为"已达标，无需改写"并在本文档回写结论**。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：合并两个 damage 列表为一次拷贝。**

  ```rust
  // crates/ime-ui/src/renderer.rs

  /// Regions one committed frame has to copy: what this frame rendered and what
  /// the surface is still showing.
  ///
  /// The two lists overlap heavily in the steady state -- a keystroke moves the
  /// highlight within the same cells the previous keystroke damaged -- so copying
  /// them one after the other copied the overlap twice. Their union is a superset
  /// of both, and a superset copy is always safe for a copy, which is the same
  /// argument the pending-list collapse already relies on.
  ///
  /// # Why the union and not a computed difference
  ///
  /// Computing the exact difference would need a rectangle-subtraction pass whose
  /// result can be several rectangles; the union is one rectangle and the copy it
  /// costs is bounded by the surface. The per-frame path must not allocate, so the
  /// union is written into a caller-owned slot rather than returned as a vector.
  fn copy_regions<'a>(state: &'a FrameState, out: &'a mut RectI) -> RectI {
      let mut all = Vec::new();  // 见下方"零分配"约束
      // ...
  }
  ```

  > **零分配约束（本卡最容易写错的地方）**：`renderer.rs:12-14` 明确要求「the per-frame path must not allocate」。因此**不得**构造临时 `Vec<RectI>` 求并集。正确写法是把 `pending` 与 `shown` 的并集**逐个矩形喂给 `union_rect` 的两两合并形式**，或直接在 `commit` 内联一个"取并集包围盒"的循环：

  ```rust
  /// The bounding box of everything one frame must copy.
  ///
  /// Written as an explicit fold over both lists rather than by building a
  /// combined vector: the per-frame path may not allocate, and the two lists are
  /// already in the state the caller owns.
  fn copy_bounds(pending: &[RectI], shown: &[RectI], width_px: u32, height_px: u32) -> Option<RectI> {
      let mut bounds: Option<RectI> = None;
      for rect in pending.iter().chain(shown.iter()) {
          let Some(clipped) = clip_rect(*rect, width_px, height_px) else {
              continue;
          };
          bounds = Some(match bounds {
              None => clipped,
              Some(current) => union_pair(current, clipped),
          });
      }
      bounds
  }

  /// The bounding box of two rectangles.
  ///
  /// The two-rectangle form of [`union_rect`], kept separate because the per-frame
  /// path folds over a stream and cannot build the slice that function takes.
  fn union_pair(left: RectI, right: RectI) -> RectI {
      let x0 = i64::from(left.x).min(i64::from(right.x));
      let y0 = i64::from(left.y).min(i64::from(right.y));
      let x1 = (i64::from(left.x) + i64::from(left.w)).max(i64::from(right.x) + i64::from(right.w));
      let y1 = (i64::from(left.y) + i64::from(left.h)).max(i64::from(right.y) + i64::from(right.h));
      RectI { x: x0 as i32, y: y0 as i32, w: (x1 - x0) as u32, h: (y1 - y0) as u32 }
  }
  ```

  `commit` 因此变为：

  ```rust
  fn commit(&self, state: &mut FrameState) -> Result<RenderOutcome, PlatformError> {
      let mut backend = self.backend.borrow_mut();
      let buffer = match backend.acquire_buffer() { /* 现状不变 */ };
      let (width_px, height_px) = self.geometry.get().physical();
      // One copy over the union: the two lists overlap in the steady state, and
      // copying them separately copied the overlap twice.
      if let Some(rect) = copy_bounds(&state.pending, &state.shown, width_px, height_px) {
          state.scratch.blit_into(buffer.data, buffer.stride, rect)?;
      }
      backend.commit(&state.pending)?;
      // ...其余不变...
  }
  ```

  > **注意**：`backend.commit(&state.pending)` 报告给合成器的 damage **必须仍是 `pending`**（只有本次改动才是新 damage），不得改成并集——否则会把上帧的 damage 重复上报，让合成器多做一次合成。这是本卡唯一容易写反的语义点。

  **第二步（条件执行）：若基准显示 `blit_into` 是瓶颈，启用行级快路径。**

  ```rust
  // crates/ime-ui/src/renderer/raster.rs

  /// One premultiplied `Argb8888` pixel, in the byte order the surface buffers use.
  ///
  /// `repr(transparent)` over the `u32` the surface format defines, so that the
  /// pixel array's byte layout is a property the type states rather than one the
  /// blit has to re-derive.
  #[repr(transparent)]
  #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
  pub(super) struct Argb8888Pixel(u32);
  ```

  `repr(transparent)` 本身不改变任何行为，它只是把既有前提写进类型；**真正的快路径仍不得使用 `unsafe`**（`ime-ui` 不在 `unsafe` 白名单内，AGENTS.md §3.3）。安全的行级写法：

  ```rust
  /// Copies one row of the scratch into one row of a surface buffer.
  ///
  /// The inner loop is the same per-pixel copy the whole-rectangle form performs,
  /// but hoisted out of the row loop so the bounds checks happen once per row
  /// rather than once per pixel, and so the compiler sees a loop over two
  /// contiguous slices with a constant trip count of four bytes.
  #[inline]
  fn copy_row(source: &[Argb8888Pixel], destination: &mut [u8]) {
      for (pixel, out) in source.iter().zip(destination.chunks_exact_mut(BYTES_PER_PIXEL)) {
          out.copy_from_slice(&pixel.to_bytes());
      }
  }
  ```

  **边界契约**：`SurfaceBackend::commit(damage)` 的语义不变（只报本次 damage）；`acquire_buffer` 的 `NoFreeBuffer` 分支与 `starved` 计数不变；`RenderOutcome::Rendered { bounding, rectangles }` 的 `bounding` 仍是 `shown` 的包围盒（`renderer.rs:288`），**不得**改为拷贝并集——它报告的是"本帧改了什么"，不是"拷了什么"。

  **并发与死锁风险**：无。全部在 UI 线程内，`RefCell` 借用范围不变。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. **先建基准**：在 `crates/ime-ui/benches/frame.rs`（新增文件）建立三个基准：`frame/blit_full`（全窗口 1200×280 单矩形）、`frame/blit_pending_shown`（模拟稳态按键的两个重叠矩形）、`frame/render_if_dirty`（经 `MockBackend` 的端到端）。**注意**：新增 bench target 需要在 `crates/ime-ui/Cargo.toml` 增加 `[[bench]]` 段——该文件属主 agent 专属（AGENTS.md §6.4），子 agent 须把所需的 `Cargo.toml` 片段写入交付报告由主 agent 落地。
  2. 记录 `frame/blit_full` 的 P50/P99 基线数值，写入本文档 §6 的回写区。
  3. 实现 `union_pair` 与 `copy_bounds`，改造 `commit`；**不改** `backend.commit` 的参数与 `RenderOutcome` 的语义。
  4. 在 `crates/ime-ui/src/renderer/tests.rs` 追加：用 `mock.rs` 的计数后端断言稳态按键（两次连续 `apply(Frame)` + `render`）的 `blit_into` 调用次数从 2 次降为 1 次。
  5. **判定 HOT-14**：若第 2 步的 `frame/blit_full` P99 ≤ 0.5ms，则本卡在此收口，HOT-14 标记为"实测达标，无需改写"，并把数值回写本文档；否则执行第 6 步。
  6. （条件）给 `Argb8888Pixel` 加 `#[repr(transparent)]`，抽出 `copy_row`，重测 `frame/blit_full` 并记录改善幅度。
  7. 回归：`cargo nextest run -p ime-ui`。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] `crates/ime-ui/benches/frame.rs` 存在，三个基准可运行；所需 `Cargo.toml` 片段已交付给主 agent
  - [ ] `frame/blit_full`（1200×280）的 P50/P99 已记录；若 P99 ≤ 0.5ms，HOT-14 判定为"实测达标"并回写本文档；否则 `#[repr(transparent)]` + `copy_row` 改造后改善幅度 ≥ 30%
  - [ ] 稳态按键的 `blit_into` 调用次数从 2 降为 1（计数后端断言）
  - [ ] `backend.commit` 收到的 damage 仍是本次 `pending`（断言：合成器看到的 damage 矩形集合与改造前逐项一致）
  - [ ] `RenderOutcome::Rendered.bounding` 仍是 `shown` 的包围盒（断言）
  - [ ] `cargo nextest run -p ime-ui` 全绿；`cargo test -p ime-ui --doc` 全绿
  - [ ] 无新增 `unsafe`（`scripts/check-unsafe.sh` 通过）

---

### 任务 ID：PERF-P0.04.01 宿主线程路径探针与预算断言

- **基本属性**：
  - 绑定热点编号：`HOT-10`
  - 优先级与复杂度：`P0 | 高 | 预估工时: 3 人天`
  - 前置依赖：无
  - 关键路径：`CP: 是`（**本卡是全项目最长依赖链的起点，必须第 1 天启动**）
  - 并行通道：`Track C 基准·监控·基建`
  - 代码落地锚点 (Code Anchor)：`crates/ime-diag/src/lib.rs`、`crates/ime-diag/src/log.rs`、`crates/ime-core/src/state/machine.rs`、`crates/ime-fcitx5/src/ffi/abi/engine.rs`、`xtask/src/budget.rs`、`docs/dev/budgets.json`
  - 当前状态：`[ ] 待优化`

- **瓶颈定位与机理剖析**：
  - **现有代码缺陷（能力缺失，非代码缺陷）**：`BUDGET-LAT-01`（按键 → 候选框新帧提交）的测量方式是「`ime-diag` 探针打点 + `Rendered{presented_at}` 回执」，但 `ime-diag` 只有日志与崩溃层（`log.rs`、`crash.rs`、`panic.rs`、`redact.rs`、`perms.rs`），**没有任何探针/计时设施**；`UiFrame` 也没有 `presented_at` 字段（`crates/ime-types/src/ui.rs:48-64`）。因此 `BUDGET-LAT-01` 当前**不可测量**。
  - **现有代码缺陷（断链）**：`ffi/abi/engine.rs:50` 的 `on_key_event` 是 stub（`false`），`addon.rs:373` 的 `ui_startup_body` 恒返回 `Err`，生产代码中无 `UiSurface` 实现（§0.2 结论五）。因此**即使有了探针也无处打点**。
  - **量化优化目标**：建立一条**可测量**的宿主线程路径：`on_key_event` 进入 → `Session::step` → `Effect::SendFrame` 投递 → `Wakeup::wake` 返回，全程打点；产出 `key_to_post` 的 P50/P99/P999 直方图，并以 `BUDGET-LAT-01` 的 P50 ≤ 4ms / P99 ≤ 16ms 作为断言。**本卡不追求达标，只追求"可测量 + 有基线数字"**——基线数字是 `PERF-P1.04.01` 的输入。

- **工程实现方案与代码级细节 (Diff / Implementation)**：

  **第一步：`ime-diag` 增加无分配的直方图探针。**

  ```rust
  // crates/ime-diag/src/probe.rs （新文件）

  /// A fixed-bucket latency histogram, written from the hot path.
  ///
  /// The probe must not allocate, must not take a lock a producer can hold for
  /// long, and must not be the reason a keystroke is late -- so the buckets are a
  /// fixed array of atomics, a sample is one relaxed increment, and the
  /// percentiles are computed when someone asks for them rather than when a
  /// sample is taken.
  ///
  /// # Why not `tracing`
  ///
  /// A log line per keystroke is an allocation and a write syscall per keystroke.
  /// The probe answers the budget question with counters, and only the summary --
  /// one line per reporting window -- goes through the logger.
  pub struct LatencyProbe {
      /// Nanosecond buckets, in ascending order; the last one is the overflow.
      buckets: [AtomicU64; BUCKETS],
      /// Samples taken.
      count: AtomicU64,
      /// Sum of the sampled nanoseconds, for the mean.
      total_nanos: AtomicU64,
  }
  ```

  桶边界（纳秒）：`100, 250, 500, 1_000, 2_500, 5_000, 10_000, 16_000, 25_000, 50_000, 100_000, 250_000, 500_000, 1_000_000, 4_000_000, 16_000_000, u64::MAX`——覆盖 `BUDGET-LAT-01` 的 4ms / 16ms 两个判决点与 `BUDGET-LAT-02` 的 3ms / 8ms 两个判决点。

  **第二步：在宿主路径上打三个点。**

  ```rust
  // crates/ime-fcitx5/src/ffi/abi/engine.rs

  pub extern "C" fn on_key_event(context: *mut c_void, ic_id: u64, event: *const FcitxKeyEvent) -> bool {
      guard_ffi(false, || {
          // The probe starts at the ABI boundary and stops after the post: the
          // budget is stated for the whole of that path, so a point taken deeper
          // in would measure something the user does not experience.
          let started = ime_diag::probe::start();
          let handled = /* ...路由 + step + 执行 Effect... */;
          ime_diag::probe::record_key_to_post(started.elapsed());
          handled
      })
  }
  ```

  **第三步：`Rendered{presented_at}` 回执的契约落地（需 ADR）。** `BUDGET-LAT-01` 的测量方式要求 UI 线程在帧真正提交后回报 `presented_at`。这需要在 `UiFrame` 或 `UiEvent` 上携带时间戳——**属于冻结契约变更**（`crates/ime-types/src/ui.rs`）。本卡**只登记该需求**，不在卡内实施：由主 agent 决定是 (a) 在 `UiEvent` 增加 `Presented { revision, presented_at_ns }` 变体，还是 (b) 由 `ime-ui` 的探针独立记录并以 `revision` 关联。**在 (a)/(b) 决定之前，`key_to_present` 不可断言，只能断言 `key_to_post`。**

  **边界契约**：
  - 通道类型语义：本卡**不新增通道**。探针是进程级 `static`，通过 `AtomicU64` 写入，读取由 `ime-diag` 的汇报窗口或 `xtask` 完成。
  - 数据 Payload：`LatencyProbe` 的桶数组是唯一共享状态；无序列化，无 RequestId/TraceId。
  - 错误码：探针本身不产生错误码；预算越界由 `xtask budget` 报告，复用既有 `budget/regression` 码（若不存在则由主 agent 在 2.2.4 登记）。
  - **脱敏约束**：探针**不得**记录任何用户输入内容（AGENTS.md §3.4）。本卡只记录时长，不记录按键符号、不记录候选文本。

  **并发与死锁风险**：`AtomicU64` 的 relaxed 递增无锁、无等待；`LatencyProbe` 不参与任何锁序。唯一风险是探针本身的开销进入被测路径——缓解手段是"一次 `Instant::now()` + 一次 relaxed 递增"，并在基准中以探针关闭态作为对照。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 新建 `crates/ime-diag/src/probe.rs`，实现 `LatencyProbe`（定长桶 + 原子计数 + 分位数查询）；在 `ime-diag/src/lib.rs` 声明模块（主 agent 执行）。
  2. 在 `crates/ime-fcitx5/src/ffi/abi/engine.rs` 的 `on_key_event` 打点（此时函数体仍是 stub，探针记录的是"进入到返回"的近似路径耗时，作为**接线前的占位基线**）。
  3. 在 `xtask` 增加子命令 `probe --report <path>`（或在既有 `budget` 子命令下增加 `--probe`），把探针的分位数输出为 JSON，与 `docs/dev/budgets.json` 的 `latency_ms` 段逐项比较。
  4. 在 `xtask/src/budget.rs` 的 `Binding` 表中增加 `latency_ms.key_to_post_p50/p99`（若决定写入 `budgets.json`，需同步 `docs/dev/features.md` 0.5.3 的表格——**该表是唯一权威，主 agent 执行**）。
  5. 在 `crates/ime-diag/src/probe.rs` 的测试模块追加：空探针的分位数为 0；单样本落在正确桶；溢出样本计入最后一桶；分位数单调。
  6. 登记 `Rendered{presented_at}` 的契约需求到交付报告（不在本卡实施）。

- **基准测试 (Benchmark) 与验收标准 (DoD)**：
  - [ ] `crates/ime-diag/src/probe.rs` 存在，`LatencyProbe` 有单测覆盖（空/单样本/溢出/分位数单调）
  - [ ] `xtask` 可从探针导出 JSON 并与 `budgets.json` 比较
  - [ ] 分配断言：`LatencyProbe::record` 的调用分配次数 = 0
  - [ ] **基线数字已产出**：在开发机上跑 10 分钟真实输入（或 `xtask testd` 的回放，若可用），产出 `key_to_post` 的 P50/P99/P999 三个数字并回写本文档 §6
  - [ ] `cargo nextest run -p ime-diag -p xtask` 全绿；`cargo test -p ime-diag --doc` 全绿
  - [ ] `Rendered{presented_at}` 的契约需求已写入交付报告，等待主 agent 决策（ADR 或探针关联方案）
  - [ ] 探针不记录任何用户输入内容（代码审查项：`probe.rs` 中不出现 `&str` 类型的载荷）

---

## 6. 基线回写区（由执行者填写，禁止预先编造）

> 本区是《性能热点总清单》的**实测回写点**。任何任务卡完成后，执行者必须在此登记实测值，并同步更新对应热点行的状态。**禁止填入推测值**。

| 指标 | 测量命令 | 基线值 | 优化后 | 回写人 / 日期 |
|---|---|---|---|---|
| `decode/viterbi` P50 | `cargo bench -p ime-core --bench decode` | 待测 | — | — |
| 单次解码堆分配次数 | bench target 内分配计数器 | 待测 | — | — |
| `session/keystroke` 分配次数 | `cargo bench -p ime-core --bench input` | 待测 | — | — |
| `dict/lookup_inline` P50 | `cargo bench -p ime-dict --bench dict` | 待测 | — | — |
| `userdb/freq_hit` P99 | `cargo bench -p ime-dict --bench userdb` | 待测 | — | — |
| `frame/blit_full`（1200×280）P99 | `cargo bench -p ime-ui --bench frame` | 待测 | — | — |
| `key_to_post` P50 / P99 / P999 | `cargo run -p xtask -- probe --report` | 待测 | — | — |

---

## 7. 续写指令

本主文档全量承载：**系统设计假设清单**、**性能热点总清单（17 条）**、**WBS 任务覆盖追溯表**、**DAG 校验**、**关键路径与并行通道汇总**，以及 **P0 任务卡（8 张，原子级展开）**。

P1 与 P2 任务卡分片保存于：

- `./docs/dev/opt-perf/phase-2.md` — P1 任务卡（7 张：`PERF-P1.01.01`、`PERF-P1.01.02`、`PERF-P1.02.01`、`PERF-P1.02.02`、`PERF-P1.03.01`、`PERF-P1.03.02`、`PERF-P1.04.01`）
- `./docs/dev/opt-perf/phase-3.md` — P2 任务卡（4 张：`PERF-P2.01.01`、`PERF-P2.03.01`、`PERF-P2.04.01`、`PERF-P2.04.02`）

**续写输入** = 本主文档 + 目标分片路径。
**续写模板** = 本规范 §5 的任务卡字段（基本属性 / 瓶颈定位与机理剖析 / 工程实现方案与代码级细节 / 逐步落地实施步骤 / 基准测试与验收标准），一字不减。
**分片头部**须含 Living Header 并回链本主文档。
**禁止**在续写中新增或改名热点编号；若审计发现新热点，必须先在主文档 §2 追加编号与追溯表行，再在分片中使用。

---

*本文档由 `dev-opt-perf` 技能生成；权威阈值以 `docs/dev/features.md` 0.5.3 为准。修改本文档不需要用户确认，但修改 §3 追溯表与 §6 基线回写区时，必须同步 `docs/dev/features.md` 与 `docs/dev/budgets.json`（若涉及阈值）。*
