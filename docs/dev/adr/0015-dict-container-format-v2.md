# ADR-0015：字典容器格式 v2 与 BIGRAM 段

> 状态：**已接受（Accepted）** ｜ 决策日期：2026-10-15 ｜
> 关联文档：[0012-corpus-and-word-sources.md](0012-corpus-and-word-sources.md)、[features-add-phase1.md](../features-add-phase1.md) `TASK-1.03.01`、[../../contracts.md](../../contracts.md)

## 背景

容器 v1 已保留 `SectionKind::Bigram`，但禁止非空段，导致 `DictLm::bigram` 只能恒定降级。`TASK-1.03.01` 需要在不破坏既有 v1 词库的前提下交付有序 BIGRAM 记录段。

## 决策

1. `ime_types::DICT_FORMAT_VERSION` 从 1 升到 2；这是冻结契约变更，由本 ADR 授权。
2. 读端永久接受 v1：v1 必须无 BIGRAM flag 且 BIGRAM 段为空。
3. 新编译容器写 v2：BIGRAM 段可为空（语料不可得的 W-8 退出路径）或由 12 字节定长记录组成；记录必须按 `pair_key` 升序、保留字段为零。
4. `FLAG_BIGRAM_PRESENT` 语义在 v1/v2 下统一为“BIGRAM 段非空”。

## 后果

- 旧 v1 `base.dict` 无需迁移，继续原样读取。
- 新读端在打开时校验 BIGRAM 长度、保留字段、排序与越界访问，任何异常返回 `dict/corrupt` 或长度错误。
- 后续 `DictLm` 装配（`TASK-1.03.02`）只在段非空时启用真查，否则保持一次 `dict/bigram/absent` 诊断与 miss 降级。
