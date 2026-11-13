# third-bigram（二阶上文）评估报告 — TASK-4.03.01 验收附属结论

> 评估日期：2026-11-11 ｜ 裁决：**不立项** ｜ 复核前置条件见 §4

## 1. 卡面判据

TASK-4.03.01（`docs/dev/features-add-phase4.md`）要求：以现有 pair 表统计「二阶上文可消歧的近平局对」占比，产出评估报告；**若潜在收益 <0.5pp top1 或段预算不足，明确登记「不立项」结论**（防范围蠕变）。

## 2. 数据事实（2026-11-11 实测）

- 在产容器 `data/compiled/base.dict`（2026-10-27 构建，15.49MiB，sha256 `916239b8…`）manifest 显示 **BIGRAM 段为空**：`entries=0, bytes=0`。
- 根因是设计行为而非缺陷：`xtask/src/dictc.rs:83` 明确「Optional corpus used to generate the bigram section; **absence yields an empty v2**」——该次编译未提供 corpus 输入，`fill_bigram` 空转。生成机制本身（`bigram_counts` → score ≥ `min_score`(1.0) → `max_pairs`(200k) 截断 → v2 BIGRAM 段）已完整落地并有双编译一致性测试覆盖。
- 运行时侧 `ContextBand`（`crates/ime-core/src/viterbi/context.rs`，`[data] context_tie_band_q16` 默认 `1<<8`）与 miss 路径 `unigram + BIGRAM_MISS_PENALTY` 契约均按 Round-1 裁决就位；pair 缺席时上文重排自动退化为恒等（`without_context_is_the_identity` 测试钉死）。

## 3. 裁决理由

1. **统计对象缺席**：一阶 pair 表在产为空，「二阶上文可消歧的近平局对占比」统计空转——没有一阶近平局对，就不存在二阶可消歧对象，任何收益估计（含 0.5pp 阈值检验）都无从谈起。
2. **收益链路依赖一阶先行**：二阶上文的价值只能在一阶 pair 已投产且 holdout 分带仍显示近平局残余 ≥0.5pp 时才可能成立；当前连一阶都未投产（语料源未裁决）。
3. **预算事实**：BIGRAM 段预算（per-mille 上限）目前整体空闲，但为一阶 pair 预留；二阶（pair-of-pairs）体积增长为一阶的乘数量级，在既有 per-mille 纪律下无预算位。
4. 结论：**不立项**。卡面的两个豁免判据（<0.5pp / 段预算不足）在「一阶缺席」前提下被更强的前提覆盖。

## 4. 复核前置条件（立项门槛，全部满足方可另开新卡）

1. 语料源经 ADR-0000 字典源允许清单裁决入库（含许可审计），`dictc` 编译出含一阶 pair 的容器；
2. `just smoke` 全绿且 holdout top1 分带对照显示**一阶** pair 带来实际收益（门禁值只升不降）；
3. 一阶投产后 holdout 仍存在 ≥0.5pp 的近平局残余（`context_tie_band_q16` 带内对子统计，复用 `bigram_scan` 工作点通道）；
4. 段预算复算确认二阶体积增量仍满足 per-mille 纪律。

## 5. 相关锚点

- 编译侧：`xtask/src/dictc/bigram.rs`（`scan_records` / `parse_working_points` / `report_working_point`）、`dictc.rs` `--bigram-scan` / `--corpus`
- 运行侧：`crates/ime-core/src/viterbi/context.rs`（ContextBand）、`crates/ime-dict/src/fst_index/dict_lm.rs`（DictLm bigram 查表与 miss 契约）
- 既有整定计划：`docs/dev/research/tune-round-1.md`
