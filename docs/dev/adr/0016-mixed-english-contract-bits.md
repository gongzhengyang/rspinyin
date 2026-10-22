# ADR-0016：混输契约扩展——`DecodeFlags::ENGLISH` 位与 `CandidateSource::English` 变体

## 状态

已接受。

## 背景

中英混输（`docs/dev/features-add-phase3.md` 的 `TASK-3.05.01`）需要两处冻结契约扩展（`AGENTS.md` §8.22）：

1. 一个解码开关：开启后，**精确、模糊、纠错三段行走全部未能切分的输入**（如 `github`、`vip`）由英文词库给出整段英文候选，而不是落到逐字直通（passthrough）。
2. 一个候选来源：英文候选的来源标签是「英文」，与 `Dict`/`UserDict`/`Passthrough` 均不同——直通候选是「原样回显」，英文候选是词库词条（大小写保真：键 `github`、词条 `GitHub`），语义必须可区分。

位域的前向兼容性由既有论证覆盖（`from_bits_truncate` 丢弃未知位）；`CandidateSource` 的 `match` 穷举性由 `ui.rs` 自己的文档声明为刻意设计——「追加变体让编译器列出所有必须决策的位置」。`source_code` 映射表（`ime-ui`、两个 FFI transport、`ime-diag` schema）正是这个守卫的消费方，随本 ADR 同步补臂。

## 决策

1. `DecodeFlags` 追加 `ENGLISH = 1 << 15`。命名空间从 `0x7FFF` 扩到 `0xFFFF`（`u16` 位域的最后一位）；`decode.rs` 中钉住 `all().bits()` 的测试随行更新。默认关。
2. 配置面为 `[engine] mixed_english = false`（`ime-config`，非冻结契约），沿 `abbrev`/`correction` 的既有通路：宿主层把配置键折算成该位，解码器不读配置。
3. `CandidateSource` 追加 `English` 变体，序位 7（`source_code` 数值线：`Dict=0`…`Script=6`，`English=7`）。候选的 `annotation` 携带「英文」标签（既有 wire 字段，无变更；`短语` 标签的 `PHRASE_ANNOTATION` 同例）。
4. 语义冻结（与 ADR-0014 的纠错同构）：**英文边只在无路可切时产生**——完整拼出的输入零英文成本、零候选扰动；英文候选天然排在一切拼音路径之后（它只存在于拼音路径不存在的输入上）。
5. v1 范围显式登记：整段单边（`单一拉丁 span 边`）——整段输入作为一个英文键查询，成功则产出至多 `WORDS_PER_KEY` 个候选边；跨中英混合词条（`X光` 类）、段内多英文边、英文词的 unigram 语言模型通道（v1 中英文词条经系统 LM 查询命中 miss 地板，恰与「英文不压拼音」的优先级一致）均留待后续卡。

## 实现注记（ime-core 内部，非契约）

- **罚值符号域**：卡面草写的 `-46` 是质量档位的书写习惯（melt_eng 的 `initial_quality` 1.1 对 1.2 的差）。本工作区罚值约定为正整数、由 sweep 以 `saturating_sub` 减入分数（`CORRECTION 998 << 8`、`ABBREV 6 << 8` 同例，ADR-0014 先例），故落地为 `MIXED_ENGLISH_PENALTY_Q8 = 46`（≈0.18 个对数单位）。
- **英文库不进链**：卡面「ChainedLexicon 的英文链成员」按独立的 `LatticeOptions::english` 查询落地。链式挂载会让英文词条泄漏进拼音 span 的 `lookup` 与 `fallback_single` 通道（如音节 `a` 命中英文词 `a`）；独立查询把英文边严格限制在「无路可切」分支，与卡面自己的 NFR（「英文边仅当 span 音节匹配失败时产生」）一致，配置关闭时查询单格为 `None`、零成本。
- **English 边的 `syllables` 记 0**：英文候选不消费拼音音节（`consumed_syllables` 为 0，与直通候选同形），preedit 不为其标注读音。

## 后果

旧构建读到新位静默忽略；`CandidateSource` 的新变体按既有守卫由编译器枚举所有 match 点。英文词源（`melt-eng.tsv`）与编译命令、装配点由任务卡落地；对冻结类型的两处追加均沿 ADR-0005/0014 先例，不需要版本号。
