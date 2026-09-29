# 语言模型权重与评测基线

本文档记录 `TASK-1.02.03` 的打分权重、两个评测集上的实测命中率，以及为什么出厂权重**没有**采用网格搜索的最优解。

- 生成工具：`cargo run -p xtask -- tune [--grid] [--eval <tsv>] [--gen-holdout]`
- 打分实现：`crates/ime-core/src/lm/score.rs`（`ScoreWeights` / `Scorer`，全程 Q8.8 定点）
- 测量日期：2026-09-29 ｜ 机器：Linux 6.18.40.1-microsoft-standard-WSL2 ｜ Rust 1.98.0

## 1. 出厂权重

`ScoreWeights::default()`，与 `TASK-1.02.04` 的 `DecodeConfig` 默认值一致（Q8.8，1.0 == 256）：

| 权重 | Q8.8 | 实数 | 作用 |
|---|---|---|---|
| `uni` | 256 | 1.0 | unigram 对数概率 |
| `bi` | 154 | 0.6 | bigram 条件对数概率 |
| `len` | 90 | 0.35 | 长度奖励，`(char_count - 1) · 256` |
| `seg` | 128 | 0.5 | 段数惩罚，`-seg_count · 256` |
| `user` | 205 | 0.8 | 用户频次融合，上限 `λ_user · 2` |

## 2. 评测集

| 集合 | 规模 | 来源 | 用途 | 参与 CI |
|---|---|---|---|---|
| `crates/ime-core/tests/fixtures/lm_golden.tsv` | 232 行 | 手工编写，覆盖单字/双字/三字/四字词与常见短句 | 权重调优与回归 | 否（调优时手动跑） |
| `crates/ime-core/tests/fixtures/lm_holdout.tsv` | 6000 行 | 由 `tune --gen-holdout` 从 `data/raw/jieba-dict.tsv` 高频词派生，键取 L1 首读音，与 golden 按**词**与**键**双重去重 | 基线错误率、词源增删与展开阈值的影响度量 | 是 |

`lm_holdout.tsv` 的生成日志：6000 行写入，跳过 6173 行（non-han 73、too-long 1841、与 golden 重叠 4259）。

## 3. 实测结果（出厂权重）

| 指标 | golden（232 行） | holdout（6000 行） |
|---|---|---|
| 首选词命中率 | **95.3%**（221/232） | **13.4%**（807/6000） |
| 加权命中率 | 99.8% | 13.4% |
| 目标词出现在前 9 候选内（可达性） | 99.1%（230/232） | **24.8%**（1487/6000） |
| 键不在词表中的行（`empty`） | 0 | 205 |

判据对照：`TASK-1.02.03` 验收标准 5 要求 golden 首选命中率 ≥ 85%，实测 **95.3%**，达标。

### holdout 命中率低的解释（重要，勿误读为打分缺陷）

holdout 的 13.4% / 24.8% **主要度量的是开发子集的词表覆盖度，而不是排序质量**。原因：

- holdout 的词取自 jieba 的 34.9 万高频词，而当前词表是 `data/raw/base.tsv` 的 **5441 词开发子集**（4338 个键）。
- 键能命中（仅 205 行为 `empty`），但该键下**期望词本身往往不在子集里**，于是另一个词胜出，可达性掉到 24.8%。

因此这两个数字是**开发子集的基线**，其用途是：当真实 40 万词词库落地、或 `L3b` 展开阈值与词源增删发生变化时，用同一命令重测并比较差值。**它不能与 golden 的 95.3% 横向对比**——两者的分母不是同一件事（golden 的期望词都在词表内，holdout 的大多不在）。

## 4. 网格搜索结果（未采用）

`tune --grid` 在 golden 上搜索 48 组权重（`bi` 与 `user` 两维在当前离线词表上不可辨识，保留出厂值；原因见 `xtask/src/tune/grid.rs` 中 `grid_search` 的文档注释）：

- 最优解：`uni=128, bi=154, len=180, seg=256, user=205` → 首选命中率 **97.8%**（227/232）
- 出厂解：`uni=256, bi=154, len=90, seg=128, user=205` → 首选命中率 95.3%（221/232）

**决定：不采用网格最优解，保持出厂权重。** 理由：

1. 调优集只有 232 行，两组权重的差距是 **5 行**。`TASK-1.02.03` 的卡片本身即指出该规模"太小，无法检出 1.9% 量级的差异"（ADR-0000 的实测结论），因此这个差异在统计上不可区分，采用它属于对 232 行的过拟合。
2. 网格在 `bi` 与 `user` 不可辨识时仍给出 `uni`/`len`/`seg` 的解，其可信度进一步下降。
3. 出厂权重同时是 `TASK-1.02.04` 的 `DecodeConfig` 默认值，改动会同时影响候选顺序的确定性基线。

待真实 bigram 表与 40 万词词库落地后，应重跑网格并在此处更新本节；届时的调优集规模应显著大于 232 行。

## 5. 复现命令

```bash
cargo run -p xtask -- tune --gen-holdout      # 重新生成 lm_holdout.tsv
cargo run -p xtask -- tune                    # 出厂权重在 golden 上的命中率
cargo run -p xtask -- tune --eval crates/ime-core/tests/fixtures/lm_holdout.tsv
cargo run -p xtask -- tune --grid             # 网格搜索
```
