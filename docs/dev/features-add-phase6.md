# dev-features-add — Round-2 Phase 6 增量原子任务分片（P2 防呆细则与结构治理）

> 所属主控文档: [./features-add.md](features-add.md) ｜ 关联对比报告: [./compare-add.md](compare-add.md) ｜ 阶段定位: Round-2 Phase 6（P2）｜ 包含任务区间: TASK-6.01.01 ~ TASK-6.12.01（12 卡，预估 ≈21.5 人天）｜ 基线: main@`0ef7d8b`
>
> 阶段目标：补齐防呆与个性化细则（负反馈/退役键/per-app/compose 兜底）、扩展字级辅助能力（符号域/以词定字/辅码/反查）、交付运维 CLI，并完成行限贴顶文件的预防性拆分。全阶段遵守 §8.4 热点收敛（`TASK-6.09.01` → `TASK-6.12.01` 配置键串行；`router.rs` 只出不进；`ime-types` 零改动——全部契约变体已由 `TASK-4.01.01` 落齐）。

---

#### TASK-6.01.01 符号域扩展（emoji 域 + 自描述索引页 + ArcSwap 热重载）

- **基本属性**：
  - 绑定差距与改进项：`GAP-28 / IMP-29` ｜ 关联设计假设：`ASM-A-11`（分类学机制转写，表自建）、`ASM-A-12`
  - 差距类型：`新增功能`
  - 优先级：`P2 生态扩展` ｜ 复杂度：中 ｜ 预估工时: 2.5 人天
  - 前置依赖：`无`
  - 关键路径：`否`
  - 并行通道：`Track B UI·Visual·Protocol`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `symbols_v.yaml:12-13` 66 子域 + vhelp/vlist 自描述索引页（SCOUT-ICE-13）；WindInput `OnceLock` 内置表进程单例教训反例（SCOUT-E-14 相邻）
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-dict/src/symbols.rs`（域扩展 + 索引页）、`crates/ime-fcitx5/src/addon/session.rs:334-368`（OnceLock → 可换槽）、`data/`（emoji 域符号表 derived 条目 + `sources.toml` 登记）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：符号域从「v1/vx/vym 静态三类（B-07）」扩展为「分域 + 自描述索引页」：新增 emoji 域（自建表，GPL emoji.json 不拷贝）、`vhelp` 式「输入即列出全部域及其触发键」的索引页，并把符号表装载从 OnceLock 改为与短语表同款的 ArcSwap 可换槽（B-05：重载后符号表刷新）。
- **技术设计与代码级接入细节**：
  - **热重载改造**：

```rust
// crates/ime-fcitx5/src/addon/session.rs —— 符号表与短语表对齐为可换槽
static SYMBOL_TABLE: ArcSwap<SymbolTable> = /* … */;
/// Reload swaps the slot atomically; in-flight frames keep the old table
/// alive via Arc, so no reader ever sees a half-loaded table.
fn swap_symbol_table(next: Arc<SymbolTable>) { SYMBOL_TABLE.store(next); }
```

  - **索引页契约**：触发键 `vhelp`（可配）产出「域名 → 触发键」候选列表；选择某域后进入该域翻页浏览——全部走既有候选网格与分页通道（`ASM-A-12`，无新 UI 面）。
  - **emoji 域**：`ve` 触发，条目格式 `词\t别名(拼音)`，别名参与输入匹配；表为 `kind="derived"` 自建（首批常用 ~200 条，来源=项目自维护），sha256 钉扎纪律沿用。
- **底层与非功能约束 (NFR)**：符号域命中路径走既有符号通道预算；表体积计入 `base_dict` 预算前置约束（`ASM-A-14`，扩表前先过 `TASK-4.01.02`）；热重载原子性（Arc 读端无撕裂）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 热重载]`：OnceLock → ArcSwap 槽 + reload 接线 + 撕裂防护测试。
  2. `[步骤 2 - 索引页]`：vhelp 索引域 + 域间跳转按键流。
  3. `[步骤 3 - emoji 域]`：自建表 + sources.toml 登记 + 匹配/候选/annotation 测试。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：热重载后新表生效（旧 Arc 存活性）；索引页候选序列钉死；emoji 匹配测试。
  - [ ] 物理产物验证：X11 档手验 vhelp→emoji 全链路；重载配置后符号域即时刷新。
  - [ ] 编译与旧功能回归：默认行为零变化；`just smoke` 全绿。

#### TASK-6.02.01 以词定字

- **基本属性**：
  - 绑定差距与改进项：`GAP-29 / IMP-30` ｜ 关联设计假设：`ASM-A-12`
  - 差距类型：`新增功能`
  - 优先级：`P2 生态扩展` ｜ 复杂度：低 ｜ 预估工时: 1 人天
  - 前置依赖：`TASK-4.01.01`（消费 `KeyAction::SelectFirstChar/SelectLastChar`）
  - 关键路径：`否`
  - 并行通道：`Track B UI·Visual·Protocol`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `select_character.lua:20-60`（`[`/`]` 取候选首/尾字 + 光标不在末尾时保留右侧未处理输入，SCOUT-ICE-14）
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-fcitx5/src/engine/router.rs`（KeyAction 路由臂，≤20 行）+ `crates/ime-fcitx5/src/engine/router/effects.rs`（字级提交 Effect）、`crates/ime-core/src/state/transitions.rs`（跃迁）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：打出「中国」后按 `[` 只上屏「中」、按 `]` 只上屏「国」——单字候选排序不佳时的精确定位手段。键位默认 `[`/`]`（可配），消费 Round-1 已预留给宿主的按键通道。
- **技术设计与代码级接入细节**：

```rust
// router/effects.rs —— 字级提交：UTF-8 字符边界切分，失败原样拒绝不 panic
fn char_at_boundaries(text: &str, which: Which) -> Option<&str> {
    match which {
        Which::First => text.chars().next().map(|c| {
            &text[..c.len_utf8()]
        }),
        Which::Last => text.chars().next_back().map(|c| {
            &text[text.len() - c.len_utf8()..]
        }),
    }
}
```

  - **光标语义**：光标不在缓冲末尾时，仅取所选字的字符提交、右侧未处理输入原位保留（rime-ice 语义转写）；提交后回到常规 Composing 态。
- **底层与非功能约束 (NFR)**：O(1) 字符切分；空候选/纯符号候选按键直接穿透（不消费）；与联想态互斥（联想候选上同样可用，取其 `commit` 文本的字）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 路由臂]`：两 KeyAction 消费 + 字符切分工具 + 穿透语义。
  2. `[步骤 2 - 状态闭合]`：跃迁（提交后 Composing）+ 键位配置（`[keys]` 既有键表机制）。
  3. `[步骤 3 - 测试]`：多字节/单字/空候选/光标偏移四类钉死测试。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：切分边界（ASCII/多字节/emoji 尾）与跃迁矩阵测试。
  - [ ] 物理产物验证：X11 档手验 `nihao`→选「你好」→`[` 上屏「你」且缓冲余「hao」继续解码。
  - [ ] 编译与旧功能回归：`just smoke` 全绿。

#### TASK-6.03.01 用户负反馈三态补齐（降频 + 隐藏）

- **基本属性**：
  - 绑定差距与改进项：`GAP-30 / IMP-31` ｜ 关联设计假设：`ASM-A-11`、`ASM-A-12`
  - 差距类型：`新增功能`
  - 优先级：`P2 生态扩展` ｜ 复杂度：中 ｜ 预估工时: 2 人天
  - 前置依赖：`TASK-4.01.01`（消费 `KeyAction::ReduceFrequency/HideCandidate`）
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine·Resilience`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `cold_word_drop/processor.lua:169-202`（快捷键负反馈三态）+ `filter.lua:24-70`（降频压位语义，SCOUT-ICE-04）；其 lua 文件当存储的做法被否决——存储进用户词库
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-dict/src/user_db.rs`（`penalty` 位与 `hidden` 会话集）、`crates/ime-core/src/state/transitions.rs`（两跃迁）、`crates/ime-fcitx5/src/engine/router/effects.rs`（回执 Effect）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：用户负反馈目前只有 Forget 一态（删词）。补齐 **降频**（用户词权重乘惩罚因子，可被后续正常使用恢复——区别于 forget 的删除）与 **隐藏**（仅本会话不再产出该候选，不落盘），对齐 rime-ice「删词/隐藏/降频」三态语义；交互沿用「对高亮候选按键」。
- **技术设计与代码级接入细节**：
  - **存储语义**：降频 = 用户词记录增 `penalty: u8` 位（`effective_freq` 计算时右移），正常 commit 使 penalty 衰减一档；隐藏 = 会话内存 `HashSet<(key, text)>`，不落盘（跨会话隐藏是 Forget 的职责，不重复建设）。
  - **跃迁**：`Composing --ReduceFrequency--> Composing`（Effect 回执 + re-decode，候选位次下移）；`Composing --HideCandidate--> Composing`（本会话过滤）。
- **底层与非功能约束 (NFR)**：penalty 参与打分路径 O(1)；与衰减（Round-1）正交（衰减管时间、惩罚管信任）；写路径走既有批量落盘批（不新增落盘频次）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 存储位]`：schema 增 penalty 位（v2 库内追加字段，向后兼容旧库读取）+ 批算接线。
  2. `[步骤 2 - 跃迁与回执]`：两 KeyAction 路由 + Effect 回执 + 会话隐藏集。
  3. `[步骤 3 - 测试]`：降频后位次变化钉死；惩罚衰减恢复；隐藏会话性（重启消失）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：三态与 Forget 的语义边界矩阵；旧库兼容（无 penalty 字段读回默认 0）。
  - [ ] 物理产物验证：X11 档手验降频/隐藏即时生效与恢复路径。
  - [ ] 编译与旧功能回归：默认行为零变化；`just smoke` 全绿。

#### TASK-6.04.01 英文域细则（大小写套壳 + 连排空格）

- **基本属性**：
  - 绑定差距与改进项：`GAP-31 / IMP-32` ｜ 关联设计假设：`ASM-A-11`
  - 差距类型：`体验与韧性打磨`
  - 优先级：`P2 生态扩展` ｜ 复杂度：低 ｜ 预估工时: 1.5 人天
  - 前置依赖：`无`
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine·Resilience`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `autocap_filter.lua:63-112`（按输入码形态套壳；「大写时无法动态调频」取舍注释，SCOUT-ICE-10）、`cn_en_spacer.lua:9-28` + `en_spacer.lua:7-14`（交界/连排空格，SCOUT-ICE-16）
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-core/src/segment/english.rs`（大小形变）、`crates/ime-core/src/state/`（commit 前空格后处理，挂在 transform_output 管线）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：ENGLISH 混输 v1 直通输出的打磨：输入 `github`→`GitHub` 式首字母套壳、`GITHUB`→全大写匹配；中文后接英文候选自动补空格、连续英文候选间自动补空格（`en_spacer` 语义）。全部为 commit 前的确定性文本后处理。
- **技术设计与代码级接入细节**：

```rust
// english.rs —— casing is a display transform over the matched entry; the
// underlying user-frequency record stays cased as authored (rime-ice's
// "uppercase breaks frequency learning" trade-off is avoided by transforming
// at output, not at match time beyond case-folded comparison).
pub enum InputShape { Lower, Capitalized, Upper }
pub fn apply_shape(entry: &str, shape: InputShape) -> String;
```

  - **空格规则**：commit 侧检测「上一上屏文本」与「本次候选」的字符类别（汉字/字母数字），中→英或英→英(词边界)插单空格；规则表显式可配 `[english] spacing = "auto" | "off"`。
- **底层与非功能约束 (NFR)**：O(文本长度)；空格规则失败安全（未知类别不插）；不改变词典匹配语义（大小写折叠匹配，套壳仅在输出）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 形变]`：InputShape 判定与套壳 + 折叠匹配一致性测试。
  2. `[步骤 2 - 空格]`：commit 后处理 + 规则表配置。
  3. `[步骤 3 - 测试]`：中英混排样例矩阵钉死。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：套壳/空格矩阵（中→英、英→英、数字边界、`X光` 型词条不误插）。
  - [ ] 物理产物验证：X11 档手验混输句子排版。
  - [ ] 编译与旧功能回归：默认（spacing=auto 但 ENGLISH 域默认关）行为零变化。

#### TASK-6.05.01 构建期排序标记（降权名单 + 长词优先位次 + 大写数字入词）

- **基本属性**：
  - 绑定差距与改进项：`GAP-32 / IMP-33` ｜ 关联设计假设：`ASM-A-11`、`ASM-A-14`
  - 差距类型：`新增功能`
  - 优先级：`P2 生态扩展` ｜ 复杂度：中 ｜ 预估工时: 2.5 人天
  - 前置依赖：`TASK-4.01.02`（扩表面受治理前置）
  - 关键路径：`否`
  - 并行通道：`Track C Infra·Testability·DevOps`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `reduce_english_filter.lua:17-56`（撞车英文降权名单机制，SCOUT-ICE-12）、`long_word_filter.lua:16-50`（长词提前 ≤count 到第 idx 位，SCOUT-ICE-11）、`rime_ice.dict.yaml:22-76`（大写/数字入词参与造句，SCOUT-ICE-20）
  - 本项目代码落地锚点 (Code Anchor)：`xtask/src/dictc/`（标记 pass）、`data/`（降权名单 derived 表）、`crates/ime-dict/src/format/`（排序已编译期定序——标记作用于权重而非新增段）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：把 rime-ice 三个「运行期滤镜」转写为 **dictc 构建期标记**（己方路线：运行期零滤镜）：① 与拼音简拼撞车的英文短词（`ranch` 类）权重下调；② 长词优先位次表（同键候选按词长分桶的显示梯度，编译进权重序）；③ 大写字母/数字特殊词条入词（`3D`、`DNA` 参与整句）。
- **技术设计与代码级接入细节**：
  - **标记通道**：dictc 新 `marks` pass——降权名单条目命中 → 权重乘 `REDUCE_FACTOR`；长词梯度 → 同键内按词长分桶乘梯度因子（ Round-1 归一化的词长梯度延续，粒度细化到键内）；大写/数字词条 → 允许进入整句词格（现仅候选域）。
  - **数据表**：降权名单自建（`kind="derived"`，依据=「前 2 位是完整音节、尾音节是声母」的机器判定 + 人工复核）；名单与生成脚本入库可复现。
- **底层与非功能约束 (NFR)**：全部发生在编译期（运行期零开销零分配）；holdout top1 门禁只升不降（标记后重测）；`base_dict` 体积在治理后余量内。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 名单生成]`：撞车判定脚本 + 人工复核流程 + derived 表登记。
  2. `[步骤 2 - 标记 pass]`：dictc 三标记实现 + 确定性测试。
  3. `[步骤 3 - 质量对照]`：top1/top3 分带对照 + 冒烟全绿。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：标记幂等（重复编译同序）；名单机器判定复现测试。
  - [ ] 物理产物验证：holdout top1 ≥ 门禁值；撞车样例（`na`/`ranch` 类）排序对照报告。
  - [ ] 编译与旧功能回归：`just smoke` 全绿。

#### TASK-6.06.01 辅码筛选（词典辅码列 + 引导键会话）

- **基本属性**：
  - 绑定差距与改进项：`GAP-33 / IMP-34` ｜ 关联设计假设：`ASM-A-11`、`ASM-A-14`（辅码列扩表面受治理前置）
  - 差距类型：`新增功能`
  - 优先级：`P2 生态扩展` ｜ 复杂度：高 ｜ 预估工时: 4 人天
  - 前置依赖：`TASK-4.01.02`
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine·Resilience`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `search.lua:114-284`（`` ` `` 引导 + 候选流筛选 + wildcard + fuma 转写，SCOUT-ICE-02）；WindInput `wind-aux-code/session.rs:25` 快照 + 子序列重筛不变量 + 门卫五道不吞键（SCOUT-D-10）
  - 本项目代码落地锚点 (Code Anchor)：`xtask/src/dictc/`（辅码列生成：拆字码 → 词条列）、`crates/ime-dict/src/format/`（AUX 可选段或 STRPOOL 扩展列）、`crates/ime-core/src/aux/`（筛选会话，新建）、`crates/ime-fcitx5/src/engine/router/`（引导键路由）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：重码精确定位：输入拼音后按 `` ` `` 进入辅码会话，续输拆字/笔画码对当前候选集做**子序列重筛**（候选集合只缩不排——WindInput 子序列不变量转写），快速定位「会打不会选」的重码字。词典构建期预存辅码列使筛选 O(候选数)，避免 rime-ice 运行期 reverse lookup 的量级开销。
- **技术设计与代码级接入细节**：
  - **会话模型（WindInput 两形态取快照式）**：

```rust
// crates/ime-core/src/aux/session.rs
/// Aux-code refinement session. `kept` is always a subsequence of the
/// snapshot (order never changes — only membership shrinks); Esc/backspace
/// to the entry point restores the snapshot verbatim.
pub struct AuxSession {
    snapshot: Vec<Candidate>,
    typed: String,
}
impl AuxSession {
    pub fn refine(&mut self, code: &str) { /* predicate over aux column */ }
    pub fn kept(&self) -> &[Candidate] { /* subsequence of snapshot */ }
}
```

  - **辅码列生成**：dictc 从自建拆字表（形码数据自建，GPL 拆字库不拷贝）为每词条生成首字辅码列；列为可选段（v2 容器 flag 位追加，旧容器无列 → `aux/absent` 降级为普通解码，每进程一次）。
  - **门卫纪律（五道转写）**：已在其他域/未启用/引导键被占用（告警）/无辅码列/无候选——任一命中引导键**不消费直接穿透**。
- **底层与非功能约束 (NFR)**：筛选独占预算 ≤0.20ms（§8.6，仅会话内）；列体积计入 `ASM-A-14` 治理余量（超限则首版仅覆盖单字表）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 列生成]`：拆字表自建 + dictc 辅码列 pass + 容器可选段读写。
  2. `[步骤 2 - 会话]`：AuxSession 快照重筛 + 门卫 + 引导键路由。
  3. `[步骤 3 - 测试与降级]`：子序列不变量钉死 + `aux/absent` 降级 + X11 档手验。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：子序列不变量（任意码序列 kept ⊆ snapshot 且保序）；门卫五道；旧容器降级。
  - [ ] 物理产物验证：X11 档手验重码字定位全链路；列体积报告在预算内。
  - [ ] 编译与旧功能回归：默认关冒烟全绿。

#### TASK-6.07.01 拆字反查域

- **基本属性**：
  - 绑定差距与改进项：`GAP-34 / IMP-35` ｜ 关联设计假设：`ASM-A-11`（拆字数据自建）、`ASM-A-12`
  - 差距类型：`新增功能`
  - 优先级：`P2 生态扩展` ｜ 复杂度：中 ｜ 预估工时: 3 人天
  - 前置依赖：`TASK-6.06.01`（复用拆字表数据与可选段基建）
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine·Resilience`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `uU` 部件拆字反查域（recognizer/patterns 声明，SCOUT-ICE §2）；WindInput `wind-reverse/lib.rs:856-1040` 多源注释库「路径优先级先到先得 + 每源独立验鲜」（SCOUT-D-16）
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-dict/src/reverse.rs`（新建：拆字反查索引）、`crates/ime-core/src/segment/`（反查域判定）、`crates/ime-fcitx5/src/engine/router/`（反查域路由）、`data/`（拆字反查表 derived）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：不知读音的字打得出：`u` 前缀进入拆字域，按「字→部件分解序列」反查候选（如 `u`+`fang` 出「房」类部件组合）。与辅码共享拆字数据但查询方向相反（辅码=已知拼音筛字，反查=不知拼音寻字）。
- **技术设计与代码级接入细节**：
  - **索引形态**：拆字表（字\t部件码）构建期物化为「部件码前缀 → 字集」有序索引（FST 或排序区间，复用既有索引原语）；候选带 annotation 显示完整拆字与读音（错音提示通道复用）。
  - **域判定**：`u`+合法部件字符序列 → 反查域（与 `U` 码点域（TASK-5.01.02）互斥不冲突：大小写分域）。
- **底层与非功能约束 (NFR)**：反查查询独占预算计入快捷域 0.30ms 档同口径（域内一次性）；拆字表体积独立 derived 文件计入 `dict_mmap_rss`（25MB）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 数据]`：拆字反查表自建（部件分解覆盖常用 8105 字首期）+ 登记。
  2. `[步骤 2 - 索引与域]`：反查索引 + 域判定 + 候选产出（annotation 带读音）。
  3. `[步骤 3 - 测试]`：域互斥、越界、空结果钉死 + X11 档手验。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：反查命中/未命中/多音字 annotation 测试；与码点域互斥测试。
  - [ ] 物理产物验证：X11 档手验「不知读音寻字」全链路。
  - [ ] 编译与旧功能回归：默认关冒烟全绿。

#### TASK-6.08.01 用户词典运维 CLI

- **基本属性**：
  - 绑定差距与改进项：`GAP-35 / IMP-36` ｜ 关联设计假设：`ASM-A-13`
  - 差距类型：`架构与测试基建`
  - 优先级：`P2 生态扩展` ｜ 复杂度：低 ｜ 预估工时: 1.5 人天
  - 前置依赖：`无`
  - 关键路径：`否`
  - 并行通道：`Track C Infra·Testability·DevOps`
  - 对标参考锚点 (Benchmark Reference)：librime `tools/rime_dict_manager`（用户库导入导出/同步的独立运维 CLI 定位，SCOUT-C T4）；`rime_table_decompiler`「产物可逆向」的可调试性思想
  - 本项目代码落地锚点 (Code Anchor)：`xtask/src/userdict/`（新建模块）、`xtask/src/main.rs`（Command 枚举增 `UserDict` 变体——本 Phase 唯一允许动枚举的卡）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：用户词典的库层能力（导出/导入/备份/forget/pin）早已完备（`user_db/export.rs`、`backup.rs`），但没有运维入口（B-13）。本卡提供 `xtask user-dict <list|export|import|forget|pin|stats>` 子命令，让备份/迁移/问题排查有文档化路径；输出全部走结构化 stdout（xtask CLI 豁免区）。
- **技术设计与代码级接入细节**：
  - **子命令面**：`list [--limit N]`（按频次序列出）、`export [--out FILE]`（复用 TSV 导出，8MiB 上限沿用）、`import FILE`（幂等合并）、`forget WORD`、`pin/unpin WORD`、`stats`（条数/体积/最近上屏分布——只出计数不出词文，隐私红线对 CLI 同样生效，`--full` 显式解锁词文输出）。
  - **安全边界**：操作目标库来自 `--db` 参数（默认用户库路径），打开走与运行时相同的 readonly/迁移纪律；误操作防护：`forget`/`import` 需 `--yes` 或交互确认。
- **底层与非功能约束 (NFR)**：复用库层 API 零新增存储逻辑；导入幂等（重复导入结果不变）；诊断走既有错误码形态。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 骨架]`：Command 变体 + 参数解析 + 库打开/关闭纪律。
  2. `[步骤 2 - 六子命令]`：逐个实现 + 幂等测试。
  3. `[步骤 3 - 文档]`：`docs/dev/` 或 install.md 增运维手册节。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：六子命令对临时库的全流程测试（export→forget→import 往返一致）。
  - [ ] 物理产物验证：对真实用户库副本跑 `stats`/`export` 出正确产物。
  - [ ] 编译与旧功能回归：运行时 crate 零改动（纯 xtask）；四门禁零警告。

#### TASK-6.09.01 配置退役键机制（RETIRED_KEYS）

- **基本属性**：
  - 绑定差距与改进项：`GAP-36 / IMP-37` ｜ 关联设计假设：`ASM-A-12`
  - 差距类型：`架构与测试基建`
  - 优先级：`P2 生态扩展` ｜ 复杂度：低 ｜ 预估工时: 1 人天
  - 前置依赖：`无`
  - 关键路径：`否`
  - 并行通道：`Track C Infra·Testability·DevOps`
  - 对标参考锚点 (Benchmark Reference)：WindInput `wind-config/config.rs:142-193`（RETIRED_KEYS 显式清单 + prune_retired 物理清理 + 「清理前后 load() 逐键相同」不变量 + 值迁移先落盘再清键铁律，SCOUT-F-05）
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-config/src/reload/`（退役清单与清理 pass）、`crates/ime-config/src/schema/`（防漂移测试扩展）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：Round-2 新增了 `[assoc]`/`[quick_input]` 等配置面，未来键改名/合并不可避免。当前机制只有「未知键忽略」——旧键值会在用户层**永久残留**且无人清理。本卡建立退役键显式清单机制：清单内键在加载时从用户层物理删除（值先迁移则先迁移），并有「清理不改变语义」的守护测试。
- **技术设计与代码级接入细节**：

```rust
// crates/ime-config/src/reload/retired.rs
/// Keys this version no longer reads. Pruning runs after value migration
/// and BEFORE persistence: the iron rule is "migrate-and-persist first,
/// prune after", so an interrupted run can never lose the value.
pub const RETIRED_KEYS: &[&[&str]] = &[
    // &["engine", "old_key_name"], — filled when a key actually retires.
];
/// Invariant under test: pruning is idempotent and
/// load(prune(doc)) == load(doc) key-by-key.
pub fn prune_retired(doc: &mut DocumentMut) -> usize;
```

  - **守护测试三件**（WindInput 防漂移测试转写）：清单覆盖性（文档中不出现未登记退役的孤儿键）、清理幂等、迁移先于清理的顺序断言。
- **底层与非功能约束 (NFR)**：清理 O(键数)；本卡交付时清单为空（机制先行），首个退役键出现时只改数据不改机制。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 机制]`：清单 + prune pass + 顺序铁律接线。
  2. `[步骤 2 - 守护测试]`：三件测试 + fixture。
  3. `[步骤 3 - 文档]`：config 维护约定增「退役键流程」节。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：prune 幂等/顺序/覆盖三件全绿。
  - [ ] 物理产物验证：带退役键 fixture 的真实往返（写入→加载→清理→写回）无残留。
  - [ ] 编译与旧功能回归：现配置文件零变化；四门禁零警告。

#### TASK-6.10.01 行限贴顶预防性拆分

- **基本属性**：
  - 绑定差距与改进项：`GAP-37 / IMP-38` ｜ 关联设计假设：`ASM-A-12`
  - 差距类型：`架构与测试基建`
  - 优先级：`P2 生态扩展` ｜ 复杂度：中 ｜ 预估工时: 2 人天
  - 前置依赖：`无`
  - 关键路径：`否`
  - 并行通道：`Track B UI·Visual·Protocol`
  - 对标参考锚点 (Benchmark Reference)：WindInput 求值坍缩「渲染热路径只读结果」的下沉接缝思想（SCOUT-E-01/04，K-10 路由——只吸收结构原则，不吸收主题模型）
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-ui/src/theme.rs`（恰 800 行）、`crates/ime-ui/src/spring.rs`（恰 800）、`crates/ime-ui/src/adapter.rs`（恰 800）、`crates/ime-fcitx5/src/engine/router.rs`（749）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：四个文件行限贴顶/逼近（B-06）——Round-2 任何涉及它们的增改都会当场触发「先拆分再干活」的强制路径，打断任务原子性。本卡做**零行为变更**的预防性拆分：按职责切子模块（theme.rs → `theme/{tokens,palette,resolve}.rs`；spring.rs → 参数/积分器分离；adapter.rs → 平台适配分档；router.rs → 把剩余职责下沉 `router/` 子模块），为 opt-ui 轨道（主题模型改造）与 Round-2 各卡恢复余量。
- **技术设计与代码级接入细节**：
  - **拆分纪律（AGENTS §5）**：不删注释、不挤空行、不并长行；每文件拆出后 ≤600 行（预留余量）；`pub use` 收敛公共接口保持外部路径不变（零调用方改动）；测试随归属移动不改断言。
  - **行为等价证明**：拆分前后 `cargo nextest run` 全绿 + 目标 crate 的确定性指纹测试（既有字节指纹钉死）逐字节一致。
- **底层与非功能约束 (NFR)**：纯移动零逻辑改动；任何 diff 中出现逻辑修改即本卡失败（review 判据）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - ime-ui 三件]`：theme/spring/adapter 拆分 + 接口收敛。
  2. `[步骤 2 - router.rs]`：剩余职责下沉 `router/`（Round-1 已拆六子模块的延续）。
  3. `[步骤 3 - 等价验证]`：全量测试 + 行数核查（`wc -l` 自检命令）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：全量测试绿；`find . -name "*.rs" -not -path "./target/*" | xargs wc -l | sort -rn | head` 无 800 行贴顶非测试文件。
  - [ ] 物理产物验证：拆分后文件均有明确单一职责头注释（`//!`）。
  - [ ] 编译与旧功能回归：diff 审查确认零逻辑变更；四门禁零警告。

#### TASK-6.11.01 compose 宿主缺位兜底

- **基本属性**：
  - 绑定差距与改进项：`GAP-38 / IMP-39` ｜ 关联设计假设：`ASM-A-12`
  - 差距类型：`体验与韧性打磨`
  - 优先级：`P2 生态扩展` ｜ 复杂度：低 ｜ 预估工时: 1.5 人天
  - 前置依赖：`无`
  - 关键路径：`否`
  - 并行通道：`Track B UI·Visual·Protocol`
  - 对标参考锚点 (Benchmark Reference)：fcitx5-rime `rimestate.cpp:169-222` compose 协议（单字符结果改写 keysym 再入引擎、多字符直通上屏，SCOUT-FR-09）
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-fcitx5/src/engine/host.rs`（缺位探测已有）、`crates/ime-fcitx5/src/engine/compose.rs`（新建：最小兜底表）、`crates/ime-fcitx5/src/engine/router/`（兜底分派）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：compose/死键目前完全委托宿主 `processComposeString`（B-12），宿主不提供时静默退化（按键直穿）。本卡内置一张**最小兜底表**（常用重音死键：`´e`→`é` 等 ~30 组，数据自建）：宿主可用时仍委托宿主（优先），缺位时兜底表接管——能力不再因宿主差异静默消失。
- **技术设计与代码级接入细节**：
  - **分派顺序**：`host.compose_available() ? host.process_compose(key) : builtin_table.lookup(key)`——与既有 trait 询问流程兼容，缺位探测点已在（`engine/host.rs:58-60`）。
  - **兜底表**：静态数组（键序列 → 产出字符），匹配语义与宿主协议一致（单字符产出改写 keysym 再入管线；无匹配返回 None 穿透）。
- **底层与非功能约束 (NFR)**：表查询 O(1)（首字符分派）；宿主可用路径零开销（先探测后兜底）；表数据可配置扩展（数据文件 opt-in，首版内置即可）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 表与分派]`：内置表 + 分派逻辑。
  2. `[步骤 2 - 接线]`：router 消费点 + 缺位探测联动。
  3. `[步骤 3 - 测试]`：两路径（宿主在/不在）行为矩阵 + 死键序列钉死。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：兜底表命中/穿透/多字符产出测试；宿主优先路径 mock 测试。
  - [ ] 物理产物验证：X11 档手验死键输入（宿主与兜底两路径）。
  - [ ] 编译与旧功能回归：宿主可用环境行为零变化；`just smoke` 全绿。

#### TASK-6.12.01 per-app 模式初值预设

- **基本属性**：
  - 绑定差距与改进项：`GAP-39 / IMP-40` ｜ 关联设计假设：`ASM-A-12`、`ASM-A-07`（app 标识复用 hash 通道）
  - 差距类型：`体验与韧性打磨`
  - 优先级：`P2 生态扩展` ｜ 复杂度：中 ｜ 预估工时: 2 人天
  - 前置依赖：`TASK-6.09.01`（同文件 ime-config 键操作，串行防冲突——§8.4 热点裁决）
  - 关键路径：`否`
  - 并行通道：`Track B UI·Visual·Protocol`
  - 对标参考锚点 (Benchmark Reference)：fcitx5-rime `rimesession.cpp:41-48` + `rimeengine.cpp:65-87`（app_options：会话创建时按程序名 set_option，SCOUT-FR-13）；`rimestate.cpp:475-527` 快照恰好一次（SCOUT-FR-02）
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-config/src/schema/config.rs`（`[app_profiles]` 节）、`crates/ime-fcitx5/src/engine/session_host.rs`（会话创建时投影）、`crates/ime-core/src/state/mod.rs`（初值注入）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：不同应用需要不同初态（终端里默认英文、编辑器里默认中文简繁）：按程序名（复用既有 app 标识 hash 通道，**不落明文程序名**——隐私纪律）预设会话初值（中英/简繁/全半角）。同时评估「配置重载后用户开关态保留策略」（fcitx5-rime 快照恰好一次的语义是否引入——评估结论落验收记录，立项与否显式裁决）。
- **技术设计与代码级接入细节**：

```toml
# [app_profiles] —— key = 程序标识哈希（写入工具另出；此处示例为占位符）
[app_profiles.profiles]
# "<sha256-16hex>" = { ascii = true, script = "simplified" }
```

  - **投影时机**：会话创建时（含 per-program 池复用新建会话时）查表投影初值；无命中=现行为。与 Round-1 会话共享策略正交（共享的是会话，初值是创建参数）。
  - **重载保留评估**：对比「重载即重置」与「快照恢复」（后者需在每 IC 记 pre-reload 开关态）的成本收益，产出评估报告；若立项则追加任务卡（不在本卡内偷做）。
- **底层与非功能约束 (NFR)**：查表 O(1)（hash 键）；程序标识 hash 通道复用（明文程序名不出现在任何日志/配置）；配置键带坏值修复。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 配置面]`：`[app_profiles]` 节 + hash 键校验。
  2. `[步骤 2 - 投影]`：会话创建注入 + 与共享池交互测试。
  3. `[步骤 3 - 评估与文档]`：重载保留评估报告 + 用户手册节（含 hash 生成工具说明，复用 `xtask` 既有 hash 工具或 `user-dict` 卡的 CLI 面旁路）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：命中/未命中/坏值修复测试；与 per-program 共享组合矩阵。
  - [ ] 物理产物验证：X11 档两应用不同初值实测；配置与日志中无明文程序名（grep 抽查）。
  - [ ] 编译与旧功能回归：无 profile 配置时行为零变化；四门禁零警告。

---

## Phase 6 汇总

| 任务 | 能力 | 通道 | 工时 | CP |
|---|---|---|---|---|
| TASK-6.01.01 符号域扩展 | GAP-28 | B | 2.5 | 否 |
| TASK-6.02.01 以词定字 | GAP-29 | B | 1 | 否 |
| TASK-6.03.01 负反馈三态 | GAP-30 | A | 2 | 否 |
| TASK-6.04.01 英文域细则 | GAP-31 | A | 1.5 | 否 |
| TASK-6.05.01 构建期排序标记 | GAP-32 | C | 2.5 | 否 |
| TASK-6.06.01 辅码筛选 | GAP-33 | A | 4 | 否 |
| TASK-6.07.01 拆字反查域 | GAP-34 | A | 3 | 否 |
| TASK-6.08.01 用户词典 CLI | GAP-35 | C | 1.5 | 否 |
| TASK-6.09.01 配置退役键 | GAP-36 | C | 1 | 否 |
| TASK-6.10.01 行限拆分 | GAP-37 | B | 2 | 否 |
| TASK-6.11.01 compose 兜底 | GAP-38 | B | 1.5 | 否 |
| TASK-6.12.01 per-app 初值 | GAP-39 | B | 2 | 否 |

**波次建议**：W1 = {6.01.01, 6.02.01, 6.04.01, 6.08.01, 6.10.01, 6.11.01}（六卡文件互不相交）；W2 = {6.03.01, 6.05.01, 6.06.01, 6.09.01}；W3 = {6.07.01（依赖 6.06.01 拆字数据）, 6.12.01（依赖 6.09.01 配置键）}。出口准则：12 卡全部落地且默认行为零变化（各卡 opt-in）；行限贴顶清零；`just ci` 全绿。
