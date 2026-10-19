# ADR-0013：`UserFreqSource::set_pinned` 默认方法

## 状态

已接受。

## 背景

置顶词需要把用户选择固定到指定编码的首位。冻结的 `UserFreqSource` 目前只能学习、读取和删除频率；`PinHighlighted` 只能报告 `dict/unsupported`。存储侧已经预留 `user_meta.pinned` 和内存 pinned 集合，因此剩余问题是契约扩展方式。

## 决策

1. `UserFreqSource` 追加 `set_pinned(&self, key: &str, word: Option<&str>) -> Result<(), ImeError>` 默认方法。
2. `word = Some(text)` 表示“切换该 key 对 `text` 的置顶”：未置顶时置顶，已置顶同词时取消；已置顶其他词时改为新词。`word = None` 表示显式取消该 key 的置顶。
3. 默认实现返回 `ImeError::Unsupported`，与 ADR-0005 的 `list`、`export_tsv`、`forget` 扩展一致，避免破坏现有 mock 和下游实现。
4. 置顶只提升用户频率项到饱和上限，不新增造词，不改候选视觉状态，也不阻塞宿主线程；实际写入由用户库批量通道完成。

## 后果

现有 trait 实现无需修改即可继续编译；真实用户库可以逐步接入置顶状态。后续如需枚举某个 key 的当前置顶词，必须另立 ADR，不得通过日志或输入内容间接推断。
