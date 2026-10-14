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

- `ime-core` `Effect` 新变体（TASK-1.04.01 / TASK-2.03.01）：
  - `Effect::PinCandidate { key: String, word: String }`
  - `Effect::UnpinCandidate { key: String }`
  - `Effect::ScriptUnavailable`（或经既有 Diagnose 通道携 `ui/script/unavailable`，实现择简登记）
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

## 7. xtask 新增面

- `dictc` CLI：`--tail-flat-weight <u32>`、`--core-min-freq <u32>`（1.01.01）
- `xtask quality`：`QualityReport.length_bands` 分带审计 + JSON 新键（1.01.02）
- `xtask testd-smoke`：TSV 驱动器 + `tests/smoke/cases.tsv`（2.06.01）
- `xtask dict stage <file>`：热替换辅助命令（3.03.02）
