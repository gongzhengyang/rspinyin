# contracts.md — features-add 簇跨模块契约增项

> 本文件是 `docs/dev/features-add*.md` 28 张任务卡的跨模块 API/契约预定义（dev-rust-single 阶段 1.3）。
> 冻结契约（`crates/ime-types` 三 trait / wire 结构体 / 既有错误码）的每一处增项都有对应 ADR；实现与本文件冲突时先改本文件再改实现。

## 1. 契约增项（需 ADR 的 ime-types 变更）

### 1.1 `UserFreqSource::set_pinned`（TASK-1.04.01 → ADR-0013）

```rust
/// Pins `word` as the top candidate of `key`, or unpins when `word` is `None`.
/// Per-key; a second pin of the same word unpins. Default returns
/// [`ImeError::Unsupported`] so existing implementations compile unchanged.
fn set_pinned(&self, key: &str, word: Option<&str>) -> Result<(), ImeError> {
    let _ = (key, word);
    Err(ImeError::Unsupported)
}
```

先例：ADR-0005（`list`/`export_tsv` 默认方法）。测试 double 与既有实现零改动。

### 1.2 `DecodeFlags::CORRECTION = 1 << 14`（TASK-3.01.01 → ADR-0014）

位域只增不改；模块注释已论证前向兼容无需版本号（ADR-0005 先例）。

## 2. 内部边界增项（非冻结，随 crate 同版本发布）

- 两字层合并（TASK-2.04.02）：`dictc` 新增 `--l2 <path>`（默认 `data/raw/huayu.tsv`，文件缺失时降级为不合并并打印一行说明）与 `--l2-min-freq <u32>`（默认 0，超预算时收紧的收录下限）。合并规则：`build::merge_two_char_layer` 对两字词条——双源共有的词**保留 jieba 权重**（huayu 的 u16 频次在头部饱和：最重的约两千条全部落在 62,000–64,288 且大量同值，作主序会扰乱候选排序——实测 2字带 top1 88.3%→80.9%），只采纳 huayu 的**读法**（读法经多音字自动校验把关）；huayu 独有的词以 jieba 两字最小权重**入尾**（coverage 增益，不入头部，排序交给用户学习通道）。确定性、与文件顺序无关。
- 多音字自动校验（TASK-2.04.02）：`build::PolyphoneRule { freq_ratio_pct }`（默认 5，即 rime-ice 的 5:100）在构建期运行——逐词条比对显式读法与 L1 首选读法，分歧字的字频比（词表自身按权重聚合）达标的保留（assigned）、不达标的压平到 L1 首选（flattened）、polyphone.tsv 显式行豁免（manual）。pass 落在 `build.rs`（词表变换与 `expand_keys`/纠错应用同层；quality.rs 的模块边界声明其不解析词表），计数进入构建报告并由 manifest 的 `polyphone_auto: {assigned, flattened, manual, total}` 携带；`xtask quality` 读取 manifest 并校验 `assigned + flattened + manual == total`（无静默丢失），计入 quality 报告。
- `quality::Origin` 扩 `Huayu` 变体（TASK-2.04.02）：L3c 纠错表 origin 列新增 `huayu` 取值，`OriginMix` 同步计数。

- `ime-core` `Effect` 新变体（TASK-1.04.01 / TASK-2.03.01）：
  - 置顶：实现取「单变体切换」形态 `Effect::ToggleUserPin { key: String, word: String }`（key = 当前输入编码，word = 高亮候选词），取代原计划的 `PinCandidate`/`UnpinCandidate` 成对变体——状态机单步语义天然幂等，宿主执行后回发 `SessionEvent::PinApplied` 触发会话内 re-decode。
  - 简繁降级上报：实现取「既有诊断通道直报」形态——`ToggleScript` 在引擎层（`KeyRouter` 的模式分派）处理，无表时直接 `emit_diagnostic("ui/script/unavailable")` 并释放按键，不经会话 Effect 列表；`Effect::ScriptUnavailable` 变体不再需要（`SCRIPT_UNAVAILABLE_CODE` 常量即上报方）。
- 用户库 v2 schema 增表（TASK-1.04.01）：`user_key_pins: &str(输入编码) -> &str(置顶词)`，与 `user_meta.pinned`（词级 flag，freq 封顶与防驱逐读它）同事务写；每编码至多一个置顶词（「换词即移动置顶」），双击同词 = 取消，`word = None` = 显式取消该编码置顶（ADR-0013 决策 2）。
- 容器 v2（TASK-1.03.01）：header `version = 2`；`FLAG_BIGRAM_PRESENT` 置位 ⟺ BIGRAM 段 `len > 0`；段表布局不变；`BigramRecord { pair_key: u64, prob_q12: u16, _pad: u16 }` 按 `pair_key` 升序。

## 3. 新增诊断码（只增不改，`domain/action/reason`）

| 码 | 语义 | 级别 | 任务 |
|---|---|---|---|
| `data/user-db/schema-migrated` | v1→v2 打开时迁移发生；失败降级只读 | info / warn | 1.02.01 |
| `dict/bigram/absent` | 容器无 BIGRAM 段或 flag 未置位，`DictLm` miss 降级 | warn（每进程一次） | 1.03.01/1.03.02 |
| `dict/swap/rejected` | 热替换候选库校验失败，保旧库 | warn | 3.03.02 |

## 4. 新增配置键（坏值修默认 + 每键一条 `config/invalid`）

| 键 | 默认 | 任务 |
|---|---|---|
| `[data] decay_half_life_ticks` | 139（0 → 修默认） | 1.02.03 |
| `[fuzzy] enabled` + 8 类（zh_z/ch_c/sh_s/n_l/f_h/an_ang/en_eng/in_ing） | 全 false；个体 true 胜过主开关 | 2.01.01 |
| `[engine] abbrev` | false（既有键转正，删 "Not wired yet"） | 2.02.02 |
| `[engine] share_state` | `"context"`（`"program"` opt-in；坏值修 `"context"`） | 2.05.01 |
| `[engine] correction` | false | 3.01.01 |
| `[data] dict_staged_path` | `""` | 3.03.02 |
| `[engine] on_im_switch` | `"clear"`（`"commit_raw"` 档） | 3.06.01 |
| `[engine] compose` | false | 3.06.02 |
| `[engine] mixed_english` | false | 3.05.01 |

## 5. `UserFreqSource::freq` 语义升级（TASK-1.02.02）

签名/类型不变（`freq(&self, key: &str, word: &str) -> u32`），返回值语义从「原始 commits」变为「衰减加权使用量」（`effective_freq`）。`ime-core` 侧打分公式零改动。

## 6. 新增依赖

| crate | 版本 | 理由 | 任务 |
|---|---|---|---|
| `arc-swap` | 1（workspace 收敛） | 热替换无锁读 + 写侧整指针交换的生态标准解 | 3.03.02 |
| `regex` | 1（workspace 收敛） | TSV 冒烟用例 `@regex:` 断言形态（rime-ice 冒烟对齐）；仅 xtask 使用，不进发布闭包 | 2.06.01 |

## 7. xtask 新增面

- `dictc` CLI：`--tail-flat-weight <u32>`、`--core-min-freq <u32>`（1.01.01）
- `xtask quality`：`QualityReport.length_bands` 分带审计 + JSON 新键（1.01.02）
- `xtask testd-smoke`：TSV 驱动器 + `tests/smoke/cases.tsv`（2.06.01）
- `xtask dict stage <file>`：热替换辅助命令（3.03.02）
