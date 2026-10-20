# rspinyin 增量功能扩充与排序质量跃升规划（features-add.md · 主控文档）

> 文档版本: v1.0 ｜ 对标输入源: `./docs/dev/compare/*.md`（5 份源码级调研报告，对象 librime / fcitx5-rime / rime-ice）｜ 系统形态: **Desktop GUI**（Linux 桌面输入法：进程内 Fcitx5 插件 + 全自绘候选窗）｜ 架构基线: Rust 2024 workspace（9 crate + xtask，edition 2024 / MSRV 1.85 / toolchain 1.98.0）+ Slint 1.13 软件光栅 + Fcitx5 5.1.7 ｜ 前置对比报告: [./compare-and-improve.md](compare-and-improve.md) ｜ 关联 ADR: [./adr/](adr/)（0000 上游决策 / 0001 契约冻结 / 0003 双 addon 拆分）｜ 逻辑自检: 内置基线已通过（§7）｜ 维护约定: 代码演进后必须回写 §3 差距矩阵、§5 追溯表与任务卡状态；假设被推翻时必须同步回写受影响任务卡

---

## 0. 文档集群分片索引

| 文件 | 职责 | 任务编号区间 |
|---|---|---|
| [./compare-and-improve.md](compare-and-improve.md) | 对比与改进报告（双边审计 / 纠偏表 X-1~X-10 / 五维成熟度 / 杀手锏拆解 K-1~K-6 / `IMP-01`~`IMP-20` / 反膨胀边界） | — |
| [./features-add.md](features-add.md)（本文件） | 主控：ASM 假设 / GAP 矩阵 / 无侵入架构与边界契约 / 全局 WBS 追溯 / DAG 关键路径 / 既有文档映射 | — |
| [./features-add-phase1.md](features-add-phase1.md) | Phase 1（**P0 排序质量跃升**：分层归一化 / 用户词频衰减 / bigram 启用 / 置顶词） | `TASK-1.01.01` ~ `TASK-1.04.02`（9 卡） |
| [./features-add-phase2.md](features-add-phase2.md) | Phase 2（**P1 接线与数据生态**：模糊音 / 简拼 / 简繁 / 词源扩充 / 会话共享 / 冒烟基建） | `TASK-2.01.01` ~ `TASK-2.06.01`（9 卡） |
| [./features-add-phase3.md](features-add-phase3.md) | Phase 3（**P2 结构升级与功能面**：纠错 / 上文重排 / 索引治理 / 热替换 / 造词 / 混输 / 错音提示 / 符号 / 桥接工艺） | `TASK-3.01.01` ~ `TASK-3.06.02`（10 卡） |

**编号空间声明（重要）**：本簇的 `TASK-[阶段].[拓扑簇].[任务]` 编号独立于 `docs/dev/features.md` 的任务编号空间（后者 Phase 1 已 103 卡全部完成、Phase 2/3 仅为索引表）。两套编号的对应关系见 §6.2，下游 `dev-rust` / `dev-rust-single` 扫描 `./docs/dev/features-*.md` 时只会命中本簇三个分片文件。

---

## 1. 当前项目能力基线摘要与《系统设计假设清单》

### 1.1 能力基线（增量挂载的既有稳定边界）

- **解码引擎（ime-core）**：全切分 DAG（`segment/dag.rs`）→ 词格（`viterbi/lattice.rs`）→ K-best Viterbi beam 16 整句首选（`viterbi/mod.rs`）→ Q8.8 五权重定点打分（`lm/score.rs:173-177`）。纯函数契约（0.4 规则 4），确定性字节指纹钉死。
- **冻结契约（ime-types）**：`Lexicon`（`lookup`/`prefix`/`fallback_single`）、`UserFreqSource`（`freq`/`record`/`is_user_word`/`forget`/`list`/`export_tsv`）、`LanguageModel`（`unigram`/`bigram -> i32`）。**本规划全部任务不改动这三个 trait 的签名**——`prefix` 已带真实语义签名（纠偏表 X-6），`bigram` 槽位已在。
- **词库（ime-dict + xtask/dictc）**：6 段 mmap 容器 v1（`format/mod.rs`，BIGRAM 段与 `FLAG_BIGRAM_PRESENT` 已预留）、FST + WORDLIST 间接层零拷贝、逐段 CRC + quarantine 自愈、redb 用户库（批量落盘/readonly/备份三代）、dictc 五层管线 + sha256 白名单。
- **宿主桥（ime-fcitx5）**：10 步 fail-soft 初始化、分层按键总线 + 仲裁、有序 Effect 执行、每 IC 独立 Context、C ABI wire 双 probe。
- **既有规划位**：features.md Phase 1（103 卡）全部 `[x]`；Phase 2/3 仅有索引表；opt-ui.md 承接 UI 工艺域（本簇不做任何候选窗工艺改造）。

### 1.2 《系统设计假设清单（Assumptions First）》

> 编号沿用 `ASM-A-xx` 前缀，避免与 features.md 的 `ASM-01`~`ASM-20` 冲突。假设被推翻时，按「偏离时的修正策略」列处置并回写受影响任务卡。

| 假设编号 | 维度 | 假设内容（基于静态源码推演） | 影响的增量域 | 偏离时的修正策略 |
|---|---|---|---|---|
| `ASM-A-01` | 形态 | Desktop GUI（fcitx5 进程内插件，X11 档可真机验证，Wayland 四档未真机验证但增量不新增 Wayland 行为） | 全部 | 增量任务全部落在宿主/引擎/数据层，不触碰平台层；若 Wayland 档启用需先过 features.md R-02 决策点 |
| `ASM-A-02` | 性能预算 | `docs/dev/budgets.json` 全量阈值逐字继承为增量红线（见 §4.2）；debug 构建不得断言 release 预算（历史验收教训） | 全部 NFR | 超预算即门禁失败；先优化偿还，不得放宽阈值 |
| `ASM-A-03` | 数据量级 | 词库 348,972 词条 / 15.62MiB（红线 20MB）；词源扩充后 ≤65 万条、≤19MiB；BIGRAM 段增量 ≤2.5MiB（裁剪后 pair 数 ≤25 万 × 10B） | GAP-01/03/08 | 段份额由 `dictc/budget.rs` per-mille 强制；超限先裁剪 pair（按 PMI 排序取头），再考虑升红线（需 ADR） |
| `ASM-A-04` | 用户库规模 | 水合上限 `HYDRATE_CAP=50k` 不变；衰减参数默认半衰期 ≈139 次上屏（librime 常数 200 对应当量），可配置；tick 为逻辑时钟非墙钟 | GAP-02/15 | 超限库维持既有 on-demand + miss cache 路径，衰减计算随水合批算，不进 decode 热路径 |
| `ASM-A-05` | bigram 语料源 | **已裁决（ADR-0012，2026-10-14）**：共现语料采用 zh-Wikipedia（CC BY-SA 4.0），走 ADR 治理而非词源白名单——语料永不入库/分发，仅 `kind="derived"` 的共现统计进仓库；`NOTICE`/`licenses.md` 署名；退出路径 = 空 BIGRAM 段 + flag 不置位 | GAP-03 | 语料不可得时 BIGRAM 段保持为空、flag 不置位，`DictLm` 保持 miss 路径——所有下游任务按「段可能为空」设计 |
| `ASM-A-06` | 并发模型 | 双线程物理隔离 + SPSC/eventfd + C ABI wire 不变；衰减批算复用既有 flush/sweep 线程槽位，**不新增线程** | GAP-02/14 | 若需新后台工作，必须走 weak handle + 既有触发器模式（`user_db.rs` 模块注释的纪律） |
| `ASM-A-07` | 会话兼容 | 会话共享默认档保持 `per-context`（现行为），`per-program` 为显式 opt-in 配置；切换策略时在途会话快照语义对齐 fcitx5-rime（K-5） | GAP-09 | 策略实现有缺陷时回退编译期固定 per-context（配置键保留但忽略，报 `config/invalid`） |
| `ASM-A-08` | 许可边界 | 只从许可干净上游取数：华宇野风（Public Domain）、THUOCL（MIT）——**两者均已由 ADR-0012（2026-10-14）登记接入**，`sources.toml` 条目与校验器已同步；melt 英文词表（Apache-2.0）；rime-ice 仓库本体（GPL-3.0）与腾讯词向量（NC 存疑，ADR-0012 W-4 维持排除）绝对排除 | GAP-08/16 | 任一源许可复核失败即从白名单移除并回退 jieba 单源；`check-dict` CI 是执行点 |
| `ASM-A-09` | 数据兼容 | 用户可升级不可降级：redb schema v1→v2 单向迁移（原值语义保留），容器 v1 读取路径**永久保留**（用户手中的旧 base.dict 必须可加载） | GAP-02/03 | 迁移失败不阻断启动：保 v1 表只读降级 + `data/user-db/schema-migrated` 失败诊断，走 readonly 纪律 |
| `ASM-A-10` | 契约稳定性 | `ime-types` 三 trait、`UiFrame`/`UiCommand`/`UiEvent`、C ABI wire 结构体、既有错误码全部冻结不动；新增诊断码只增不改（`data/user-db/schema-migrated`、`dict/bigram/absent`、`dict/swap/rejected`） | 全部 | 若实现中发现必须改契约，先停任务、走 ADR + 主代理裁决（AGENTS.md §8-22） |

---

## 2. 功能差距矩阵总览（`GAP-01` ~ `GAP-19`）

> 映射链：`compare-and-improve.md` §5 的 `IMP-XX` → 本表 `GAP-XX` → §5 WBS 的 `TASK-X.XX.XX` → 分片文件任务卡。链条 100% 闭合（§7 自检项）。

| 差距编号 | 关联改进项 | 所属维度 | 细分功能/架构改进点 | 对标参考实现 | 本项目现状锚点 | 差距等级 |
|---|---|---|---|---|---|---|
| `GAP-01` | `IMP-02` | 核心业务深度 | 词库权重分层归一化（两字真频带 / 3+ 字压平带 / 词长梯度） | rime-ice 三层归一化（`base/ext/tencent` 词频策略，K-1） | jieba 单源真实分布直接进排序；`xtask/src/dictc/`（build 管线）无权重归一层；top1 73.8% | P0 |
| `GAP-02` | `IMP-01` | 核心业务深度 | 用户词频指数衰减 + 逻辑 tick + log 域进打分 | librime `algo/dynamics.h` `formula_d/p`（K-2） | `user_db.rs:17` 表值 `(commits, last_used_ms)` 单调增；`score.rs:250-257` user 项无时间维 | P0 |
| `GAP-03` | `IMP-03` | 核心业务深度 | BIGRAM 段填充 + `DictLm::bigram` 真查 | librime Grammar 槽 + poet 有 LM 束搜索（K-3） | `fst_index/dict_lm.rs:207-214` 恒 miss（`unigram + BIGRAM_MISS_PENALTY`）；`format/mod.rs:84` flag 预留 | P0 |
| `GAP-04` | `IMP-04` | 核心业务深度 | 置顶词引擎级转正 + 管理闭环 | rime-ice `pin_cand_filter.lua` 语义（纯重排不造词） | `state/transitions.rs:203-204` `PinHighlighted → Unsupported`；`user_db.rs:177,336` pinned 字段闲置 | P0 |
| `GAP-05` | `IMP-06` | 操作效率 | 模糊音生产接线（配置 → 解码位） | librime fuzz 物化语义（能力已有，只差开关） | `fuzzy.rs` 8 类全实现；`state/machine.rs:454` 只设 SHUANGPIN 位；无 `[fuzzy]` 配置节 | P1 |
| `GAP-06` | `IMP-07` | 操作效率 | `Lexicon::prefix` 实装 + 简拼生产接线 | librime abbrev/completion 边 + `enable_word_completion` | `fst_index.rs:399-403` 返回 `Unsupported`；`ime-config/src/reload.rs:125-127` 标注 Not wired | P1 |
| `GAP-07` | `IMP-08` | 操作效率 | 简繁 commit 接线 + 输出模式默认值投影 | rime-ice traditionalize 开关 + 状态区联动 | `script.rs`/`ScriptIndex` 齐备未接；`router/modes.rs:221-230` 硬编码默认（纠偏 X-2）；`ui/script/unavailable` 无上报方（X-10） | P1 |
| `GAP-08` | `IMP-15` | 数据组织 | 词源扩充：华宇野风（PD）+ THUOCL（MIT）+ 多音字自动校验 | rime-ice 词库上游 + `polyphone.go` 纪律（K-6） | `data/sources.toml` 仅 6 源；两字词为分词统计频；L3c 依赖手工 polyphone.tsv | P1 |
| `GAP-09` | `IMP-12` | 操作效率 | 会话共享粒度（per-program opt-in） | fcitx5-rime `RimeSessionPool` 三档 + 快照/重建（K-5） | 每 IC 独立 `Context`（`ime-fcitx5/src/engine/`）；无程序级池 | P1 |
| `GAP-10` | `IMP-19` | 工程暗线 | ~~数据驱动解码冒烟~~ **已交付（纠偏 X-11）**：`xtask testd-smoke` + `tests/smoke/cases.tsv`（211 例）+ `just ci` `smoke` 层均已落地 | rime-ice smoke（K-6） | `xtask/src/testd/smoke.rs`、`tests/smoke/cases.tsv`、`justfile:322` | 已闭环 |
| `GAP-11` | `IMP-10` | 核心业务深度 | 拼写纠错（邻位替换 + 键序颠倒 + correction 重罚） | librime `NearSearchCorrector` + credibility −log0.01（K-4） | 无 corrector 文件；fuzzy 扩边基建可复用 | P2 |
| `GAP-12` | `IMP-11` | 核心业务深度 | 上文重排（上一 commit 词作 bigram 上下文） | librime `contextual_translation`（每轮 ≤32 重打分） | 解码无跨 commit 上下文；依赖 GAP-03 | P2 |
| `GAP-13` | `IMP-05` | 数据组织 | 重码治理（智能截断 + 溢出表） | librime TailIndex/`extra_code`（简化版） | `MAX_WORDS_PER_KEY=32` 硬截断丢词 | P2 |
| `GAP-14` | `IMP-20` | 交付工程 | 词库热替换（校验后 ArcSwap 原子切换） | features.md 3.02.04 自有规划位 | 替换需重启；无 swap 路径 | P2 |
| `GAP-15` | `IMP-09` | 个性化 | 用户造词写路径（句内成分回写 + 词格融合） | librime `Memory::OnCommit` 自动造词 | `user_db.rs:694` `is_user_word` 恒 false；无造词表 | P2 |
| `GAP-16` | `IMP-16` | 个性化 | 中英混输（英文词段 + 融合档位） | rime-ice melt_eng `initial_quality` 融合 | 无英文词段；仅临时英文直通 | P2 |
| `GAP-17` | `IMP-17` | 个性化 | 错音错字提示（annotation 通道） | rime-ice corrector.lua 思路（表自建） | annotation 显示位在（`show_annotation`），无比对逻辑 | P2 |
| `GAP-18` | `IMP-18` | 个性化 | 符号面板 v 模式（触发前缀 + 符号表数据） | rime-ice `symbols_v.yaml` 71 类 | 无触发路由与符号数据；overlay/候选通道现成 | P2 |
| `GAP-19` | `IMP-13`/`IMP-14` | 交付工程 | 桥接工艺簇：切输入法收尾 + capability 协商 + compose + 通知节流 + StatusArea | fcitx5-rime 平台服务清单 2/3/4/5/9 项 | 均未实现（fcitx5-rime.md §3.2 表） | P2 |

---

## 3. 无侵入接入架构、工艺基线与边界交互契约

### 3.1 无侵入接入架构（Add-on Architecture）

**总原则：所有 P0 增量都发生在既有 decode 边界之内，`ime-types` 契约、`UiFrame`/`UiCommand`、C ABI wire 零变更**（`ASM-A-10`）。挂载点全部是既有扩展位：

```text
[dictc 编译期]                      [运行期 decode 域]                    [宿主域]
sources.toml ──► L1..L5 管线        SyllableDag ──► build_lattice ──► sweep ──► kbest
   │                │                                  │            │        │
   │  GAP-01: 权重归一层（新增 pass，段份额由 budget.rs 强制）             │        │
   │  GAP-08: 新源接入（sources.toml 白名单 + L3b/L3c 规则增强）           │        │
   │  GAP-03: BIGRAM 段生成（新增段，flag 置位）                          │        │
   ▼                ▼                                  ▼            ▼        ▼
 容器 v2 ──────► FstLexicon / DictLm ◄── LanguageModel trait ── score.rs 五权重
                    │                                   ▲        ▲
                    │  GAP-02: UserDb schema v2 ── freq() 返回衰减后概率（O(1) 整数映射）
                    │  GAP-06: prefix() 流式走查（abbrev walk 消费）
                    │  GAP-13: 溢出表（lookup 尾部追加消费）
                    │  GAP-14: ArcSwap 切换点（会话持有 Lexicon 的工厂处）
                    │  GAP-15: 造词表（lattice 单音节 fallback 前插入用户词边）
                    ▼
              transitions.rs（Effect 有序 emitted）
                    │  GAP-04: PinHighlighted 转正（新 Effect::PinCandidate）
                    │  GAP-07: commit 前挂 script transform；modes.rs 默认值接配置
                    │  GAP-11: 纠错扩边（lattice 构建期，与 fuzzy 同机制）
                    │  GAP-12: 初始 prev_word = 上一 commit 词
                    ▼
              router/effects.rs ──► C ABI wire ──► UiFrame（零变更）
                        GAP-09: Context 池键策略（ime-fcitx5 内部）
                        GAP-18/17: 候选列表/annotation 既有通道承载
```

- **数据与配置向后兼容**：容器 v1 永久可读（读端按 header version/flag 分派）；redb v1→v2 迁移在打开时进行、失败不阻断（降级只读 + 诊断）；配置新增键全部走既有「坏值修默认 + 每键一条 `config/invalid` 诊断」哲学，模板 `DEFAULT_CONFIG_TOML` 与 `Config::default` 的防漂移测试同步扩展。
- **降级阶梯**：BIGRAM 段缺失 → miss 路径（现状行为）；衰减参数缺失 → 无衰减（v1 语义）；per-program 池异常 → 回落 per-context；热替换失败 → 保旧库。**任何增量失败都必须收敛到「现状行为」而不是不可用。**

### 3.2 形态自适应工艺级体验基线（Desktop GUI）

本簇的 UI 原则：**只消费既有主题 token 与组件，不做工艺改造**（工艺域归 opt-ui.md）。涉及 UI 表面的增量任务遵守：

| 项 | 基线 |
|---|---|
| 置顶视觉反馈 | 高亮候选维持现有 Spring 滑动高亮；置顶词按普通第一候选呈现（不新增视觉状态，避免与 opt-ui.md 的 token 规划冲突） |
| 符号候选 | 走既有候选网格（`candidate_grid.slint`）与分页；注释位显示符号分类名（`show_annotation` 通道） |
| 错音提示 | annotation 位文本追加（现有 `Candidate` 注释字段），不新增窗体 |
| 状态区/通知 | GAP-19 复用 fcitx5 宿主 StatusArea/通知机制（自绘窗零改动） |
| 5 态交互 | 不涉及新控件；候选窗 Default/Hover/Active/Focus/Disabled 语义维持现状 |

### 3.3 边界交互契约增量规范（Boundary Protocol Contract）

1. **序列化 Payload**：C ABI wire（`RspinyinFrameWire` 等 `#[repr(C)]`）**零变更**。引擎内部跨 crate 数据（`Effect` 枚举新增 `PinCandidate`、`ScriptUnavailable` 变体）属于 ime-core→ime-fcitx5 内部边界，随 crate 版本同发布，不属于冻结契约。
2. **RequestId/TraceId 贯穿**：沿用 `Revision` 幂等与 `FrameWatch` 旧帧丢弃契约；置顶/造词等用户操作以「会话 revision + 键」幂等，UI 侧无新增贯穿需求。
3. **跨边界状态机跃迁**：
   - `Composing --PinHighlighted--> Composing`（新增）：发出 `Effect::PinCandidate { key, word }` → 宿主写 `user_meta.pinned` → 会话内 re-decode → 帧重发。失序防护：同一高亮位重复 Pin 幂等（第二次为取消，emit `Effect::UnpinCandidate`）。
   - `Composing --ToggleScript--> Composing`（既有跃迁补全）：由「仅重发帧」补全为「置位 script 模式 + commit 路径生效 + 降级时 emit `Effect::Diagnose(ui/script/unavailable)`」。
   - `Committing --script/punct/full_width transform--> Committed`：变换挂在既有 `transform_output` 管线末尾，失败原样透传（简繁缺字时保简体 + 诊断，**不阻断提交**）。
4. **新增错误码枚举段**（只增不改，对齐 features.md 2.2.4 的 `domain/action/reason` 形态）：
   - `data/user-db/schema-migrated`：v1→v2 迁移在打开时发生（info）；迁移失败降级只读时为 warn。
   - `dict/bigram/absent`：容器无 BIGRAM 段或 flag 未置位，`DictLm` 保持 miss 降级（warn，每进程一次）。
   - `dict/swap/rejected`：热替换候选库校验失败（CRC/段表/预算），保旧库继续服务（warn）。

### 3.4 增量预算裁决（对齐 `ASM-A-02`）

`decode_p99 = 3.0ms`（budgets.json）为总量红线。增量独占分配如下（**总和 ≤ 1.80ms，为既有实现保留 ≥ 1.20ms**；任一任务落地前其独占项必须在 criterion 基准中有对应断言）：

| 增量特性 | 独占预算 | 说明 |
|---|---|---|
| bigram 查表（GAP-03） | ≤ 0.25ms | 开地址双 hash 探测，每边一次；miss 与 hit 等价 |
| 用户衰减映射（GAP-02） | ≤ 0.10ms | 水合期预计算 + 查表近似，decode 侧 O(1) 整数运算 |
| 模糊音扩边（GAP-05） | ≤ 0.60ms | 既有实现路径的预算位（当前生产未触发，接线后生效） |
| 简拼 prefix walk（GAP-06） | ≤ 0.50ms | 流式枚举 + `limit` 截断 |
| 纠错扩边（GAP-11） | ≤ 0.30ms | 邻位表有限展开 + 候选上限 |
| 上文重排（GAP-12） | ≤ 0.05ms | 45 候选 × 一次 bigram 查表复用 |

常驻内存：BIGRAM 段计入 `dict_mmap_rss`（25MB）与 `base_dict`（20MB）红线（`ASM-A-03` 裁剪规则）；衰减字段 per-record +12B，50k 条 ≈ 0.6MB，计入 `plugin_rss`（45MB）。

---

## 4. 全局 WBS 任务覆盖追溯表与 DAG 关键路径

### 4.1 全局追溯矩阵

| 差距编号 | 关联改进项 | 并行通道 | 核心增量功能点 | 绑定任务节点清单 | 所在分片文件 |
|---|---|---|---|---|---|
| `GAP-01` | `IMP-02` | Track C | 词库权重分层归一化 + 质量门刷新 | `TASK-1.01.01`, `TASK-1.01.02` | `./features-add-phase1.md` |
| `GAP-02` | `IMP-01` | Track A | 用户词频衰减（schema v2 + 打分集成 + 空闲批算） | `TASK-1.02.01`, `TASK-1.02.02`, `TASK-1.02.03` | `./features-add-phase1.md` |
| `GAP-03` | `IMP-03` | Track A | BIGRAM 段启用与真查 | `TASK-1.03.01`, `TASK-1.03.02` | `./features-add-phase1.md` |
| `GAP-04` | `IMP-04` | Track A | 置顶词引擎转正与管理闭环 | `TASK-1.04.01`, `TASK-1.04.02` | `./features-add-phase1.md` |
| `GAP-05` | `IMP-06` | Track A | 模糊音配置接线 | `TASK-2.01.01` | `./features-add-phase2.md` |
| `GAP-06` | `IMP-07` | Track A | prefix 实装与简拼接线 | `TASK-2.02.01`, `TASK-2.02.02` | `./features-add-phase2.md` |
| `GAP-07` | `IMP-08` | Track A | 简繁 commit 接线 + 输出模式默认值投影 | `TASK-2.03.01`, `TASK-2.03.02` | `./features-add-phase2.md` |
| `GAP-08` | `IMP-15` | Track C | 词源扩充（华宇/THUOCL）+ 多音字自动校验 | `TASK-2.04.01`, `TASK-2.04.02` | `./features-add-phase2.md` |
| `GAP-09` | `IMP-12` | Track B | 会话共享粒度（per-program opt-in） | `TASK-2.05.01` | `./features-add-phase2.md` |
| `GAP-10` | `IMP-19` | Track C | ~~数据驱动解码冒烟基线~~ 已交付（纠偏 X-11） | `TASK-2.06.01`（`[x]`） | `./features-add-phase2.md` |
| `GAP-11` | `IMP-10` | Track A | 拼写纠错（邻位 + 键序颠倒） | `TASK-3.01.01` | `./features-add-phase3.md` |
| `GAP-12` | `IMP-11` | Track A | 上文重排 | `TASK-3.02.01` | `./features-add-phase3.md` |
| `GAP-13` | `IMP-05` | Track C | 重码治理（智能截断 + 溢出表） | `TASK-3.03.01` | `./features-add-phase3.md` |
| `GAP-14` | `IMP-20` | Track C | 词库热替换 | `TASK-3.03.02` | `./features-add-phase3.md` |
| `GAP-15` | `IMP-09` | Track A | 用户造词写路径 | `TASK-3.04.01` | `./features-add-phase3.md` |
| `GAP-16` | `IMP-16` | Track C | 中英混输 | `TASK-3.05.01` | `./features-add-phase3.md` |
| `GAP-17` | `IMP-17` | Track C | 错音错字提示 | `TASK-3.05.02` | `./features-add-phase3.md` |
| `GAP-18` | `IMP-18` | Track B | 符号面板 v 模式 | `TASK-3.05.03` | `./features-add-phase3.md` |
| `GAP-19` | `IMP-13`/`IMP-14` | Track B | 桥接工艺簇 | `TASK-3.06.01`, `TASK-3.06.02` | `./features-add-phase3.md` |

### 4.2 DAG 拓扑约束与关键路径

- **编号即拓扑**：簇号按依赖层级递增；任何前置依赖的编号字典序严格小于自身（跨 Phase 依赖仅允许指向更小编号，已核验：`TASK-3.02.01 ← TASK-1.03.02`、`TASK-3.04.01 ← TASK-1.02.01`、`TASK-2.04.02 ← TASK-1.01.01`）。无环。
- **关键路径（CP）**：
  - **CP-1（排序质量主干）**：`TASK-1.03.01`（容器+生成）→ `TASK-1.03.02`（真查集成）→ `TASK-3.02.01`（上文重排）。bigram 是长句 top1 的最大单杠杆，数据生成是其中唯一的重活。
  - **CP-2（数据杠杆）**：`TASK-1.01.01`（归一化管线）→ `TASK-2.04.01`（源接入）→ `TASK-2.04.02`（两字层替换）。两字词真频是 top1 的第二大杠杆。
- **并行通道**：Track A（Core/Engine，ime-core/ime-dict）、Track B（宿主/桥接，ime-fcitx5 + UI 表面）、Track C（Infra/数据，xtask/dictc/data/脚本）。Phase 1 内部 4 簇两两无文件交集，可 3~4 子代理并行；Phase 2 中 `2.01/2.02/2.03`（Track A）与 `2.04/2.06`（Track C）、`2.05`（Track B）无文件交集。

### 4.3 阶段里程碑与工时汇总

| Phase | 定位 | 任务数 | 预估工时 | 出口准则（里程碑） |
|---|---|---|---|---|
| Phase 1 | P0 排序质量跃升 | 9 | ≈ 21.5 人天 | `dictc quality` 留出集 top1 ≥ 78%（分层+bigram 落地后重测）；衰减/置顶行为有对照测试；四门禁全绿 |
| Phase 2 | P1 接线与数据生态 | 9 | ≈ 18 人天 | 模糊音/简拼/简繁生产可用（配置键到端到端）；两字词层升级；testd-engine 冒烟基线 ≥ 200 用例 |
| Phase 3 | P2 结构升级与功能面 | 10 | ≈ 24.5 人天 | 纠错/上文重排/造词/混输可用；热替换在 X11 档实测；桥接工艺五项补齐 |

> top1 目标值说明：73.8% 基线由「分层归一化（去掉长尾噪声竞争）+ bigram（上下文消歧）+ 两字真频（数据替换）」三力叠加，≥78% 为保守目标；实际以 `TASK-1.01.02` 重测后的门禁值为准，**门禁值只升不降**。

---

## 5. 与既有文档的映射与冲突裁决

### 5.1 冲突裁决顺序

`docs/dev/features.md`（权威）> 本簇 > `docs/dev/opt-ui.md`。本簇与 features.md 技术冲突时修本簇；与 opt-ui.md 冲突时双方都修到与 features.md 一致。

### 5.2 features.md Phase 2/3 索引 ↔ 本簇任务映射

| features.md 索引项 | 本簇承接 | 说明 |
|---|---|---|
| `TASK-2.02.01` 模糊音解码 | `TASK-2.01.01` | 引擎侧已实现（features.md 索引写于实现前），本簇只做接线 |
| `TASK-2.02.02` 简拼 | `TASK-2.02.01`/`TASK-2.02.02` | 同上：代数已实现，补 prefix 实装 + 接线 |
| `TASK-2.02.03` 纠错解码 | `TASK-3.01.01` | 范围收窄为邻位纠错（不做 A* 编辑距离，反膨胀 §6） |
| `TASK-2.02.04` 双拼 | **已完成**（Phase 1 交付） | 索引滞后，无需立项 |
| `TASK-2.02.06` 自定义短语 | **已完成**（phrases.tsv） | 同上 |
| `TASK-2.02.07` 简繁转换 | `TASK-2.03.01`/`TASK-2.03.02` | 算法已实现，接线 + 默认值投影 |
| `TASK-2.03.01` 词频衰减 | `TASK-1.02.01`~`1.02.03` | 数学模型升级为 librime formula_d/p（K-2） |
| `TASK-2.03.04` 词库热替换 | `TASK-3.03.02` | 原规划位，本簇细化 NFR |
| `TASK-2.05.05` 表情/符号面板 | `TASK-3.05.03` | 本簇只做数据+触发+候选通道；面板工艺归 opt-ui.md |
| `TASK-3.02.01` Bigram | `TASK-1.03.01`/`TASK-1.03.02` | 提前到 P0（排序主杠杆） |
| `TASK-3.02.03` 上下文感知排序 | `TASK-3.02.01` | 范围收窄为「上一 commit 词」bigram 上下文（不做 surroundingText 应用类型感知） |
| 其余索引项（剪贴板/皮肤/打包/加密/多 Profile 等） | 不承接 | 与对标输入源无关，维持 features.md 原规划节奏 |

### 5.3 已删除的旧 features-add（52 卡）

编号空间与内容**全部作废**（对比报告纠偏表 X-5）；其登记的三处基线缺陷已闭环，本簇不再引用任何旧卡号。

---

## 6. 下游执行交接

- 本簇三个分片文件的任务状态行统一为「反引号包裹的 `[ ] 待开始`」，前缀 `当前状态：`（完整形态见各任务卡基本属性末行），命中下游对账正则：`grep -rnE '当前状态：`\[ \]|^\s*- \[ \]' ./docs/dev/features-*.md`。主控文档自身不承载任务卡，不应被对账命中。
- 执行入口：`/dev-rust`（多子代理并行，推荐按 §4.2 通道分派）或 `/dev-rust-single`（单代理串行拓扑序）。
- 执行纪律继承 AGENTS.md：子代理只写代码不跑 cargo；门禁由主代理在全部波次完成后统一跑一次（§2 四门禁 + `just ci` + 800 行上限）。
- 落盘后建议执行独立审查：`/dev-logic-audit ./docs/dev/features-add.md Desktop GUI fix`。

---

## 7. 内置自检记录（逻辑自检：内置基线已通过）

- [x] 映射链闭合：20 项 IMP → 19 项 GAP → 28 张任务卡，双向无孤儿（IMP-12/13/14 多对一映射 GAP-09/19 已显式登记）。
- [x] DAG 无环：全部前置依赖编号字典序小于自身；跨 Phase 依赖 3 处均合法。
- [x] 预算闭环：§3.4 独占预算总和 1.80ms ≤ decode_p99 余量规则；内存增量均有红线归属。
- [x] 契约冻结：`ime-types` 三 trait / wire 结构体 / 既有错误码零变更；新增诊断码 3 个均为只增。
- [x] 形态匹配：无 Web/TUI 专属要求；UI 表面全部复用既有通道，工艺域让位 opt-ui.md。
- [x] 向后兼容：容器 v1 永久可读、redb 单向迁移失败降级、配置坏值修默认、共享策略默认行为不变。
- [x] 反膨胀：compare-and-improve.md §6 十项拒绝边界在本簇任务卡中无一被绕过。
