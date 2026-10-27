# rspinyin 增量功能扩充与排序质量跃升规划（features-add.md · 主控文档）

> 文档版本: v2.0（Round-2 续编）｜ 对标输入源: Round-1 `./docs/dev/compare/*.md`（5 份报告，已交付）；Round-2 `remarks/*` 四仓源码级精读（librime / WindInput / fcitx5-rime / rime-ice，2026-10-27）｜ 系统形态: **Desktop GUI**（Linux 桌面输入法：进程内 Fcitx5 插件 + 全自绘候选窗）｜ 架构基线: Rust 2024 workspace（9 crate + xtask，edition 2024 / MSRV 1.85 / toolchain 1.98.0）+ Slint 1.13 软件光栅 + Fcitx5 5.1.7 ｜ 前置对比报告: Round-1 [./compare-and-improve.md](compare-and-improve.md)（IMP-01~20，stage 22 全部交付）；Round-2 [./compare-add.md](compare-add.md)（IMP-21~40）｜ 关联 ADR: [./adr/](adr/)（0000/0001/0003 基线；0005/0013~0016 Round-1 增量；0017 Round-2 批次，已分配）｜ 逻辑自检: 内置基线已通过（§7 / §8.9）｜ 维护约定: 代码演进后必须回写差距矩阵、追溯表与任务卡状态；假设被推翻时必须同步回写受影响任务卡

---

## 0. 文档集群分片索引

| 文件 | 职责 | 任务编号区间 |
|---|---|---|
| [./compare-and-improve.md](compare-and-improve.md) | Round-1 对比与改进报告（双边审计 / 纠偏表 X-1~X-10 / 杀手锏 K-1~K-6 / `IMP-01`~`IMP-20` / 反膨胀边界）——**已交付档案** | — |
| [./compare-add.md](compare-add.md) | Round-2 对比与改进报告（8 份勘察摘要综合 / Round-1 闭环确认 / K-7~K-12 / `IMP-21`~`IMP-40` / 反膨胀续裁） | — |
| [./features-add.md](features-add.md)（本文件） | 主控：ASM 假设 / GAP 矩阵（§2 Round-1 / §8.3 Round-2）/ 无侵入架构与边界契约 / 全局 WBS 追溯 / DAG 关键路径 / 既有文档映射 | — |
| [./features-add-phase1.md](features-add-phase1.md) | Round-1 Phase 1（P0 排序质量跃升）——**已交付** | `TASK-1.01.01` ~ `TASK-1.04.02`（9 卡，全部已完成） |
| [./features-add-phase2.md](features-add-phase2.md) | Round-1 Phase 2（P1 接线与数据生态）——**已交付** | `TASK-2.01.01` ~ `TASK-2.06.01`（9 卡，全部已完成） |
| [./features-add-phase3.md](features-add-phase3.md) | Round-1 Phase 3（P2 结构升级与功能面）——**已交付** | `TASK-3.01.01` ~ `TASK-3.06.02`（10 卡，全部已完成） |
| [./features-add-phase4.md](features-add-phase4.md) | Round-2 Phase 4（**P0 排序第二跃升与联想**：契约底座 / 体积治理 / 名次衰减 / 上文与 bigram 预算 / 联想三源） | `TASK-4.01.01` ~ `TASK-4.05.01`（6 卡） |
| [./features-add-phase5.md](features-add-phase5.md) | Round-2 Phase 5（**P1 功能面与测试暗线**：快捷输入族 / 自定义双拼 / 数据驱动整定 / Xvfb 无头层 / 冒烟 v2） | `TASK-5.01.01` ~ `TASK-5.05.01`（6 卡） |
| [./features-add-phase6.md](features-add-phase6.md) | Round-2 Phase 6（**P2 防呆细则与结构治理**：符号域 / 以词定字 / 负反馈 / 英文域 / 构建期标记 / 辅码 / 反查 / CLI / 退役键 / 行限拆分 / compose 兜底 / per-app 初值） | `TASK-6.01.01` ~ `TASK-6.12.01`（12 卡） |

**编号空间声明（重要）**：本簇的 `TASK-[阶段].[拓扑簇].[任务]` 编号独立于 `docs/dev/features.md` 的任务编号空间（后者 Phase 1 已 103 卡全部完成、Phase 2/3 仅为索引表）。两套编号的对应关系见 §5.2，下游 `dev-rust` / `dev-rust-single` 扫描 `./docs/dev/features-*.md` 时会命中本簇六个分片文件——Round-1 三个分片（phase1/2/3）全部已完成状态，Round-2 三个分片（phase4/5/6）为待执行面。

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

- ✅ 映射链闭合：20 项 IMP → 19 项 GAP → 28 张任务卡，双向无孤儿（IMP-12/13/14 多对一映射 GAP-09/19 已显式登记）。
  - 落地证据：xtask/src/dictc/normalize.rs (test_normalize_two_char_true_freq_is_kept, test_normalize_core_clamp_caps_the_head, test_normalize_long_words_are_flattened, test_normalize_low_freq_two_char_drops_into_the_tail)；build.rs 归一化 pass 接线于纠音后、五段写出前
- ✅ DAG 无环：全部前置依赖编号字典序小于自身；跨 Phase 依赖 3 处均合法。
  - 落地证据：xtask/src/dictc/quality.rs report 模块 Top1Band/LengthBandAccuracy 词长分带 top1 审计；xtask/src/tune/holdout.rs + tune/tests.rs (test_generate_holdout_is_stratified_attributed_and_reproducible, test_generate_holdout_writes_rows_that_avoid_the_evaluation_set)
- ✅ 预算闭环：§3.4 独占预算总和 1.80ms ≤ decode_p99 余量规则；内存增量均有红线归属。
  - 落地证据：crates/ime-dict/src/user_db.rs schema v2 (dee/tick) + user_db/open.rs 打开时迁移；user_db/tests/migration.rs (test_user_db_migrates_v1_rows_once_and_preserves_their_values, test_user_db_migrates_an_empty_v1_store_without_writes, test_a_migrated_store_hydrates_and_lists_its_rows, test_user_db_failed_v1_migration_degrades_readonly_and_can_retry)
- ✅ 契约冻结：`ime-types` 三 trait / wire 结构体 / 既有错误码零变更；新增诊断码 3 个均为只增。
  - 落地证据：crates/ime-dict/src/user_db/decay.rs (test_fold_half_life_is_exact_to_one_half, test_fold_is_monotonic_and_bounded_by_the_floor, test_fold_treats_a_rewound_tick_as_unaged, test_fold_saturates_an_extreme_tick_gap_at_the_floor)；user_db/tests.rs (test_user_db_decay_sinks_an_aged_word_below_a_fresh_one, test_user_db_idle_fold_advances_a_flushed_record_to_the_current_tick)
- ✅ 形态匹配：无 Web/TUI 专属要求；UI 表面全部复用既有通道，工艺域让位 opt-ui.md。
  - 落地证据：crates/ime-config/src/schema/tests.rs (test_data_section_reads_the_decay_keys_from_a_document, test_data_section_repairs_decay_values_a_document_writes_out_of_range)；user_db/tests.rs evict 批量前向折叠 (test_user_db_evicts_the_oldest_records, test_user_db_eviction_forgets_the_evicted_counts)
- ✅ 向后兼容：容器 v1 永久可读、redb 单向迁移失败降级、配置坏值修默认、共享策略默认行为不变。
  - 落地证据：crates/ime-dict/src/format/reader/ v2 BIGRAM 段验收 (test_parse_rejects_a_bigram_section_in_a_version_one_container, test_round_trip_reads_back_every_section_byte_for_byte)；xtask/src/dictc/bigram.rs (test_count_cooccurrence_segments_known_adjacent_words_and_skips_unknown_text, test_build_records_cuts_quantizes_and_sorts_deterministically, test_build_records_honours_the_score_floor_and_empty_input)
- ✅ 反膨胀：compare-and-improve.md §6 十项拒绝边界在本簇任务卡中无一被绕过。
  - 落地证据：crates/ime-dict/src/fst_index/tests.rs (test_dict_lm_bigram_hit_matches_the_in_memory_model_and_ranking, test_dict_lm_bigram_falls_back_to_the_unigram_minus_the_miss_penalty, test_dict_lm_v1_has_no_bigram_section_and_preserves_the_miss_path)；ime-core lm/ngram.rs + lm/score.rs (test_edge_score_uses_the_bigram_when_a_predecessor_is_given)

---

## 8. 第二轮增量（Round 2 · `IMP-21`~`IMP-40` · 2026-10-27 勘察波次）

> 输入：[./compare-add.md](compare-add.md)（8 份勘察摘要，`./research/digest-*.md`）+ 基线勘察短板清单 B-01~B-15。Round-1 的 §0~§7 为已交付档案原样保留；本章为其增量续编，结构与 §1~§4 同构。

### 8.1 Round-1 闭环与 Round-2 出发点

Round-1 全部 28 卡已完成（基线勘察逐项核实挂载点，见 compare-add.md §1.2）。Round-2 的三个着力点：**① 排序深度残留封顶**（B-01/B-02：k-best 无名次衰减、上文单词窄带、bigram 双截断吞长尾）；**② 用户可感知功能面**（B-07/B-11：联想/快捷输入/自定义双拼——WindInput 与 rime-ice 提供完整契约样板）；**③ 工程暗线**（B-04/B-05/B-08/B-09/B-12/B-13：体积红线、无头渲染回归、冒烟断言维度、配置演进防呆）。

### 8.2 《系统设计假设清单 · Round-2 增补（`ASM-A-11` ~ `ASM-A-16`）》

| 假设编号 | 维度 | 假设内容（基于静态源码推演） | 影响的增量域 | 偏离时的修正策略 |
|---|---|---|---|---|
| `ASM-A-11` | 许可边界 | WindInput 与 rime-ice 整仓 **GPL-3.0**：全部吸收仅限机制级语义转写；其代码/词表/纠错表/降权名单/拆字表/农历数据一律不得拷贝进本仓库，相关数据表必须自建并在 `sources.toml` 登记 `kind="derived"` | GAP-22~34 | 任一产出被发现与 GPL 仓库文本实质相似即删除重写；`check-dict` 与人工复核是执行点 |
| `ASM-A-12` | 呈现边界 | 联想/符号/辅码/反查候选全部复用既有候选通道与主题 token，**不新增 UI 契约面**；契约增量收敛为 ADR-0017 批次（`CandidateSource::Assoc` + `KeyAction` 四变体） | GAP-22/28/29/30/33/34 | 若实现中需要更多契约面，先停任务走 ADR + 主代理裁决（§8.5） |
| `ASM-A-13` | 纯函数红线 | 快捷输入族的时钟/随机源读取一律经 `ime-fcitx5/src/engine/host.rs` 既有内部 trait 注入（`ClockSource`/`RngSource` 扩展）；decoder 保持纯函数（0.4 规则 4）；测试用固定 Clock fixture | GAP-23 | 宿主不提供时钟源时快捷时间域整体禁用（回退现状），不得引入直读 |
| `ASM-A-14` | 体积红线 | base.dict 17.6MiB / 预算 20MiB（占用 84%）：**任何词表/标记/辅码列扩充前必须先过体积治理卡**（`TASK-4.01.02`）；红线不放宽（`ASM-A-02` 延续） | GAP-21/32/33 | 裁剪后仍超限则削减该增量范围，不得放宽 `size_mb.base_dict` |
| `ASM-A-15` | 无头环境 | Xvfb 可作为 CI 渲染回归的虚拟显示；Xvfb 档只挡**回归**（渲染不崩/几何稳定），视觉工艺终审仍以真机 X11 档为准 | GAP-26 | Xvfb 不可用的环境按 `NeedsEnvironment` 三态记录，不算失败 |
| `ASM-A-16` | 求值安全 | 计算器等表达式求值禁止 eval/动态执行：采用白名单预审的第三方解析器或自写 PR 解析器；输入长度与嵌套深度设上限 | GAP-23 | 解析器依赖过安全预审（AGENTS §3.5）失败则计算域降级为纯四则运算自写实现 |

### 8.3 Round-2 差距矩阵（`GAP-20` ~ `GAP-39`）

> 映射链：compare-add.md §7 的 `IMP-21`~`IMP-40` → 本表 `GAP-20`~`GAP-39` → §8.7 WBS 的 `TASK-4/5/6.x.x` → phase4/5/6 任务卡。链条 100% 闭合（§8.9 自检项）。

| 差距编号 | 关联改进项 | 所属维度 | 细分功能/架构改进点 | 对标参考实现（SCOUT 锚点） | 本项目现状锚点 | 差距等级 |
|---|---|---|---|---|---|---|
| `GAP-20` | `IMP-21` | 核心业务深度 | 成句器第二阶段：k-best 名次衰减 + 上文带/pair 预算重估 + 候选供给阶梯 | `poet.cc:325-341` cutoff、`handle_candidate.rs:1106` 阶梯（K-9） | `viterbi/context.rs:56` 单词+窄带；kbest 无衰减；候选 64 硬截 | P0 |
| `GAP-21` | `IMP-22` | 数据组织 | 词条价值审计与体积治理（为一切扩表腾位） | rime-ice `cn_dicts/` 分表职责纪律 | `data/compiled/` base.dict 17.6MiB / 20MiB = 84%（B-04） | P0 |
| `GAP-22` | `IMP-23` | 个性化 | 联想（下一词预测，Bigram/History/Prefix 三源逐源配额） | `wind-assoc/lib.rs:50-110,329`（K-7） | 无联想面；bigram 段与 commit 历史闲置 | P0 |
| `GAP-23` | `IMP-24` | 个性化 | 快捷输入族（日期/时间/星期/大写数字/金额/计算器/码点） | `date/number/calc/unicode_translator.lua`（K-8） | `symbols.rs:1-5` 仅静态符号类别（B-07） | P1 |
| `GAP-24` | `IMP-25` | 个性化 | 自定义双拼布局数据化（TOML 三表 + 击键覆盖测试） | `data/schemas/shuangpin/*.toml`（K-12） | `decode.rs:149-161` 六方案硬编码（B-11） | P1 |
| `GAP-25` | `IMP-26` | 数据组织 | 数据驱动参数整定（holdout top1 为目标函数）+ 采集纯净度 precheck | rime-ice `polyphone.go` 数据把关思想 | `dictc.rs:432-433` 归一化常数手工（B-03/B-14） | P1 |
| `GAP-26` | `IMP-27` | 工程暗线 | Xvfb 无头渲染回归层（testd 第四通道） | WindInput `scripts/linux/e2e.sh`（Xvfb + dbus-run-session） | testd 注入/截图依赖真 X（B-08） | P1 |
| `GAP-27` | `IMP-28` | 工程暗线 | 冒烟 TSV v2：top3/top5 锚 + 开档矩阵 + `@date/@regex` 动态断言 | `others/script/smoke/cases/*.tsv`（K-11） | `tests/smoke/cases.tsv:3` 全部 `@first:`（B-09） | P1 |
| `GAP-28` | `IMP-29` | 个性化 | 符号域扩展：emoji 域 + 自描述索引页 + ArcSwap 热重载 | `symbols_v.yaml:12-13` 66 子域 + vhelp/vlist | `symbols.rs:1-5`；`addon/session.rs:334-368` OnceLock 不随重载刷新（B-05） | P2 |
| `GAP-29` | `IMP-30` | 个性化 | 以词定字（取候选首/尾字上屏） | `select_character.lua:20-60` | 无对应 KeyAction | P2 |
| `GAP-30` | `IMP-31` | 个性化 | 用户负反馈三态补齐（降频 + 隐藏；删词已有 Forget） | `cold_word_drop/{processor,filter}.lua` | `user_db.rs` 仅 forget/pin 语义 | P2 |
| `GAP-31` | `IMP-32` | 个性化 | 英文域细则：按输入码形态大小写套壳 + 中英/英文连排空格 | `autocap_filter.lua:63-112`、`en_spacer.lua:7-14` | `segment/english.rs` v1 直通无后处理 | P2 |
| `GAP-32` | `IMP-33` | 数据组织 | 构建期排序标记：撞车英文降权名单 + 长词优先位次 + 大写字母/数字入词 | `reduce_english_filter.lua`、`long_word_filter.lua`、`rime_ice.dict.yaml:22-76` | dictc 无标记通道；重码审计已有上限策略（Round-1） | P2 |
| `GAP-33` | `IMP-34` | 个性化 | 辅码筛选（词典预存辅码列 + 引导键会话快照重筛） | `search.lua:114-284` + `wind-aux-code/session.rs:25` | 无辅码面 | P2 |
| `GAP-34` | `IMP-35` | 个性化 | 拆字反查域（不知读音的字按字形反查） | rime-ice `uU` 域 + `wind-reverse/lib.rs:856-1040` | 无反查面 | P2 |
| `GAP-35` | `IMP-36` | 工程暗线 | 用户词典运维 CLI（list/export/import/forget/pin） | librime `tools/rime_dict_manager` 定位 | `user_db/export.rs` 库层完备但 `main.rs:34` 无入口（B-13） | P2 |
| `GAP-36` | `IMP-37` | 工程暗线 | 配置退役键机制（显式清单 + 物理清理 + 先落盘再清键） | `wind-config/config.rs:142-193` RETIRED_KEYS | ime-config 迁移有、退役清理无 | P2 |
| `GAP-37` | `IMP-38` | 架构维护 | 行限贴顶预防性拆分（theme/spring/adapter 恰 800、router.rs 749） | WindInput 求值坍缩的下沉接缝（K-10 路由，零行为变更） | B-06（任何增改即触发强制拆分） | P2 |
| `GAP-38` | `IMP-39` | 工程暗线 | compose 宿主缺位兜底（插件内置最小死键表） | `rimestate.cpp:169-222` compose 协议 | `engine/host.rs:51-62` 纯委托，缺位静默退化（B-12） | P2 |
| `GAP-39` | `IMP-40` | 个性化 | per-app 模式初值预设 + 重载开关态保留策略 | `rimesession.cpp:41-48` app_options、`rimestate.cpp:475-527` 快照 | 无按程序预设；重载保留策略未契约化 | P2 |

### 8.4 无侵入接入架构与热点收敛（Round-2）

- **绞杀者路径**：全部新增能力走「旁路新增 → 配置默认关 → 显式 opt-in」三级路径。联想/快捷输入/辅码/反查都是在既有 decode 产线**之后**追加的候选供给源或**过滤器**，不改 `Lexicon`/`UserFreqSource`/`LanguageModel` 三 trait 签名（`ASM-A-10` 延续）；唯一契约增量收敛在 ADR-0017 批次（§8.5）。
- **热点文件并发隔离（Round-2 版）**：
  - `crates/ime-types/src/{ui.rs,key.rs}`（契约枚举）：**仅 `TASK-4.01.01` 可修改**（ADR-0017 批次一次性落齐 5 个契约变体），其余所有卡只消费。
  - `crates/ime-config/src/schema/config.rs`：骨架键节由 `TASK-4.01.01`（`[assoc]`）与 `TASK-5.01.01`（`[quick_input]`）、`TASK-5.02.01`（自定义布局节）分 Phase 前置卡落齐；Phase 6 中 `TASK-6.09.01`（退役键）先于 `TASK-6.12.01`（per-app 键）串行（依赖已声明）。其余卡触及配置一律只读消费，防漂移测试同步扩展由**当卡**负责。
  - `xtask/src/main.rs`（Command 枚举）：每 Phase 至多一张卡新增变体——Phase 5 归 `TASK-5.03.01`（tune 扩展），Phase 6 归 `TASK-6.08.01`（user-dict 子命令）；`TASK-5.04.01` 的 Xvfb 通道走 `testd` 既有子命令参数扩展，不动枚举。
  - `crates/ime-fcitx5/src/engine/router.rs`（749 行）：任何卡不得向其追加超过 20 行；新路由逻辑进 `router/` 子模块（`TASK-6.10.01` 完成拆分后余量恢复）。
- **降级阶梯（Round-2 新增项）**：联想源任一缺席 → 该源静默跳过（配额归并其余源）；时钟源缺席 → 时间域候选整体不产出；辅码列缺席 → 引导键回退普通解码；Xvfb 不可用 → `NeedsEnvironment` 记录不判红；compose 兜底表缺失 → 维持现状委托宿主。**任何增量失败都收敛到现状行为。**

### 8.5 边界契约增量规范（Round-2 · ADR-0017 批次）

1. **ADR-0017（已分配，落盘于 `TASK-4.01.01`）**一次性认领五个契约变体：
   - `CandidateSource::Assoc`（联想来源标签，`ui.rs` 枚举只增）；
   - `KeyAction::SelectFirstChar` / `KeyAction::SelectLastChar`（以词定字，`key.rs` 只增）；
   - `KeyAction::ReduceFrequency` / `KeyAction::HideCandidate`（负反馈降频/隐藏，`key.rs` 只增）。
   全部为枚举追加变体，不改既有变体与 trait 签名；`ime-types` 冻结声明的「变更需 ADR」程序由此满足。
2. **宿主内部 trait 扩展（非冻结契约）**：`engine/host.rs` 增 `ClockSource`/`RngSource` 关联访问（`ASM-A-13`），属 ime-fcitx5 内部边界随 crate 同发布。
3. **新增错误码（只增不改）**：`quick/expr/invalid`（表达式解析失败，回显原式不 panic）、`aux/absent`（辅码列缺席降级，每进程一次）、`assoc/unavailable`（联想源装配失败降级，warn）。既有码零改动。
4. **跨边界状态机跃迁**：`Committed --(assoc trigger)--> Composing(联想候选态)`——联想候选仅在「上一 commit 后未输入」的会话态产出，任意按键即回到常规解码；`Composing --ReduceFrequency/HideCandidate--> Composing`（对高亮候选生效，emit Effect 回执后 re-decode）。

### 8.6 Round-2 增量预算裁决

预算真源是 `docs/dev/budgets.json` 现值（`latency_ms.decode_p99 = 24.0`；§3.4 的 3.0ms 为 Round-1 时点口径，已由 features-test v3.0 纠正）。Round-2 增量独占分配（合计 ≤ 0.70ms，任一卡落地前在 criterion 基准补对应断言）：

| 增量特性 | 独占预算 | 说明 |
|---|---|---|
| k-best 名次衰减（GAP-20） | ≤ 0.05ms | 纯整数 Q8.8 乘加，随 kbest 遍历一次完成 |
| 联想候选供给（GAP-22） | ≤ 0.15ms | 仅 commit 后会话态触发；常规按键路径零进入 |
| 快捷输入域（GAP-23） | ≤ 0.30ms | 前缀未命中时 O(1) 判定零开销；命中才进入域解析 |
| 辅码筛选（GAP-33） | ≤ 0.20ms | 仅引导键会话内生效；快照重筛为 O(候选数) 单次 |

内存：联想三源缓存与快捷域表计入 `plugin_rss`（45MB）份额；辅码列计入 `base_dict`（20MB，受 `ASM-A-14` 治理前置约束）；拆字表为独立 derived 词典文件计入 `dict_mmap_rss`（25MB）。

### 8.7 Round-2 全局 WBS 追溯表

| 差距编号 | 关联改进项 | 并行通道 | 核心增量功能点 | 绑定任务节点清单 | 所在分片文件 |
|---|---|---|---|---|---|
| `GAP-22` | `IMP-23` | Track A | Round-2 契约底座（ADR-0017 批次 + 配置骨架） | `TASK-4.01.01` | `./features-add-phase4.md` |
| `GAP-21` | `IMP-22` | Track C | 词条价值审计与体积治理 | `TASK-4.01.02` | `./features-add-phase4.md` |
| `GAP-20` | `IMP-21` | Track A | k-best 名次衰减与候选供给阶梯 | `TASK-4.02.01` | `./features-add-phase4.md` |
| `GAP-20` | `IMP-21` | Track A | 上文带宽度与 bigram pair 预算重估 | `TASK-4.03.01` | `./features-add-phase4.md` |
| `GAP-22` | `IMP-23` | Track A | 联想三源引擎侧（AssocSource trait + 后继查询） | `TASK-4.04.01` | `./features-add-phase4.md` |
| `GAP-22` | `IMP-23` | Track B | 联想会话接线与配置投影 | `TASK-4.05.01` | `./features-add-phase4.md` |
| `GAP-23` | `IMP-24` | Track A | 快捷域底座（ClockSource/RngSource + 域骨架 + 解析器依赖） | `TASK-5.01.01` | `./features-add-phase5.md` |
| `GAP-23` | `IMP-24` | Track A | 七类快捷翻译器实现 | `TASK-5.01.02` | `./features-add-phase5.md` |
| `GAP-24` | `IMP-25` | Track A | 自定义双拼布局数据化 | `TASK-5.02.01` | `./features-add-phase5.md` |
| `GAP-25` | `IMP-26` | Track C | 数据驱动参数整定 + 采集纯净度 precheck | `TASK-5.03.01` | `./features-add-phase5.md` |
| `GAP-26` | `IMP-27` | Track C | Xvfb 无头渲染回归层 | `TASK-5.04.01` | `./features-add-phase5.md` |
| `GAP-27` | `IMP-28` | Track C | 冒烟 TSV v2（多维断言 + 开档矩阵） | `TASK-5.05.01` | `./features-add-phase5.md` |
| `GAP-28` | `IMP-29` | Track B | 符号域扩展（emoji + 索引页 + 热重载） | `TASK-6.01.01` | `./features-add-phase6.md` |
| `GAP-29` | `IMP-30` | Track B | 以词定字 | `TASK-6.02.01` | `./features-add-phase6.md` |
| `GAP-30` | `IMP-31` | Track A | 用户负反馈三态补齐 | `TASK-6.03.01` | `./features-add-phase6.md` |
| `GAP-31` | `IMP-32` | Track A | 英文域细则（大小写套壳 + 连排空格） | `TASK-6.04.01` | `./features-add-phase6.md` |
| `GAP-32` | `IMP-33` | Track C | 构建期排序标记（降权/长词/大写数字入词） | `TASK-6.05.01` | `./features-add-phase6.md` |
| `GAP-33` | `IMP-34` | Track A | 辅码筛选 | `TASK-6.06.01` | `./features-add-phase6.md` |
| `GAP-34` | `IMP-35` | Track A | 拆字反查域 | `TASK-6.07.01` | `./features-add-phase6.md` |
| `GAP-35` | `IMP-36` | Track C | 用户词典运维 CLI | `TASK-6.08.01` | `./features-add-phase6.md` |
| `GAP-36` | `IMP-37` | Track C | 配置退役键机制 | `TASK-6.09.01` | `./features-add-phase6.md` |
| `GAP-37` | `IMP-38` | Track B | 行限贴顶预防性拆分 | `TASK-6.10.01` | `./features-add-phase6.md` |
| `GAP-38` | `IMP-39` | Track B | compose 宿主缺位兜底 | `TASK-6.11.01` | `./features-add-phase6.md` |
| `GAP-39` | `IMP-40` | Track B | per-app 初值预设 | `TASK-6.12.01` | `./features-add-phase6.md` |

### 8.8 Round-2 DAG 拓扑约束与关键路径

- **跨 Phase 前置（已核验编号字典序合法）**：`TASK-4.03.01 ← TASK-4.01.02`（pair 预算重估需治理腾位）；`TASK-4.04.01 ← TASK-4.01.01`（联想源消费 Assoc 契约）；`TASK-4.05.01 ← TASK-4.04.01`；`TASK-5.01.02 ← TASK-5.01.01`；`TASK-5.03.01 ← TASK-4.03.01`（整定 sweep 覆盖新参数）；`TASK-5.05.01 ← TASK-4.05.01`（开档矩阵钉联想行为）；`TASK-6.02.01/6.03.01 ← TASK-4.01.01`（消费 KeyAction 变体）；`TASK-6.12.01 ← TASK-6.09.01`（配置键防串行冲突）。无环。
- **关键路径（CP-R2）**：`TASK-4.01.01`（契约底座）→ `TASK-4.04.01`（联想源）→ `TASK-4.05.01`（联想接线）→ `TASK-5.05.01`（冒烟钉行为）。联想是 Round-2 唯一贯穿引擎-宿主-测试三层的旗舰能力。
- **并行通道**：Track A（ime-core/ime-dict/ime-config）、Track B（ime-fcitx5 + UI 表面）、Track C（xtask/data/脚本）。Phase 4 六卡中 `4.01.01/4.01.02`（底座+治理，文件不相交）与 `4.02.01`（viterbi）可三路并行；Phase 6 十二卡按簇号串行分波（同簇文件不相交即可并行）。

### 8.9 Round-2 内置自检声明

- ✅ 映射链闭合：Round-2 共 20 项 IMP（21~40）→ 20 项 GAP（20~39）→ 24 张任务卡（6+6+12），双向无孤儿（IMP-21 两卡、IMP-23 三卡、IMP-24 两卡拆分已显式登记）。
- ✅ DAG 无环：§8.8 全部前置依赖编号字典序严格小于自身。
- ✅ 预算闭环：§8.6 新增独占预算合计 0.70ms，均有 budgets.json 键归属与内存红线归属；`ASM-A-14` 使扩表面受治理卡前置约束。
- ✅ 契约冻结：三 trait 与 wire 结构零变更；唯一契约增量收敛 ADR-0017 批次（枚举只增），ADR 已在 `docs/dev/adr/README.md` 取号登记。
- ✅ 反膨胀：compare-add.md §8 的 16 项 Round-2 裁决在本轮任务卡中无一被绕过（农历/UUID/T9 列为 IMP-24 非目标）。
- ✅ 形态匹配：无 Web/TUI 专属要求；UI 表面全部复用既有通道；视觉工艺域让位 opt-ui.md（compare-add.md §3 路由）。
