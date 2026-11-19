# 首轮参数整定（holdout top1 网格 sweep）— 模板

> 状态：**已闭环（2026-11-17）**。§6/§7 为两批 sweep 的工具输出原样存档（12 + 9 点，全部 spread 0.00pp）；§8 裁决已签署——维持现状、不回写，门禁值不变。原模板说明保留于下。
> 工具面：`xtask tune --sweep <spec.toml>`（推荐表 + 参数敏感性摘要）；采集纯净度门禁 `PurityCheck` 在 bench / tune sweep / soak 三入口统一前置。

## 1. 目的与边界

四组手工常数——归一化（`core_min_freq` / `tail_flat_weight`）、衰减（半衰期）、bigram（`max_pairs` / `min_score`）、上文带（`context_tie_band_q16`）——改为由 holdout top1（分带）目标函数的数据重估：sweep 引擎按声明式 TOML 参数空间逐点编译、逐点评估、按确定性排序输出推荐表；**工具只推荐**。回写默认值是人工裁决（本文档 §8 骨架），门禁值只升不降。

## 2. 参数组与真实读取点

| 组 | 参数名 | 现行值 | 真实读取点 | sweep 通道 |
|---|---|---|---|---|
| 归一化 | `core_min_freq` | 20 | `dictc --core-min-freq`（构建期，`xtask/src/dictc.rs`） | 重编译 + holdout top1 ✅ |
| 归一化 | `tail_flat_weight` | 100 | `dictc --tail-flat-weight`（构建期，同上） | 重编译 + holdout top1 ✅ |
| bigram | `max_pairs` | 200000 | `dictc --max-bigram-pairs`（构建期，同上） | 重编译 + holdout top1 ✅ |
| bigram | `min_score` | 1.0 | `dictc --min-bigram-score`（构建期，同上） | 重编译 + holdout top1 ✅ |
| 衰减 | `decay_half_life_ticks` | 139 | `ime-config` `[data] decay_half_life_ticks`（运行期；`crates/ime-config/src/schema.rs`） | 当前目标函数不可观测（§3） |
| 上文带 | `context_tie_band_q16` | 256（`1<<8`） | `ime-config` `[data] context_tie_band_q16` → 解码器 `ContextBand`（运行期；`crates/ime-core/src/viterbi/context.rs`） | 当前目标函数不可观测（§3） |

## 3. 当前目标函数的可观测边界

sweep 目标 = holdout top1（分带），测的是**编译期烤入容器的 per-key 排序**。因此：

- 衰减参数作用于用户库（用户词频随提交次数衰减），而 holdout 集不含任何用户历史——每个网格点都会测到同一个数。
- 上文带只在「上一个 commit 词存在」的近平局重排时生效，而 holdout 评估无上文——同样，每个网格点都会测到同一个数。

两类参数在 TOML 参数空间里是**真实键名**（sweep 的注册表认识它们），但 sweep 会显式拒绝并说明原因，而不是跑完输出误导性的「无效果」——「无通道」与「无效果」是两回事。要整定这两组，需要先给目标函数扩展可观测通道（句级解码评估 / 用户衰减仿真），属于后续独立工作；届时在 §2 表中把通道改为「可观测」，再补网格。

## 4. 本轮网格（拟定，待跑）

> 拟定网格以现行默认值居首：现行工作点出现在表内，才可直接对比「改动 vs 现状」。跑前可按机器预算调整密度；拒绝指引里「降低网格密度」即指这里。

```toml
[grid]
core_min_freq    = [20, 10, 40, 80]
tail_flat_weight = [100, 50, 200]
max_pairs        = [200000, 120000, 400000]
min_score        = [1.0, 0.5, 2.0]
```

单点成本 ≈ 一次完整 `dictc` 编译 + 一次 holdout 评估；总时长随网格规模线性（3×3×3×4 = 108 点为上限预案，首轮可先按参数分批：先归一化两参数联合，再 bigram 两参数联合，避免一轮跑过夜）。

## 5. 如何运行

1. **空闲机纪律（已升级为门禁，不再靠自觉）**：1 分钟负载 > 25% 核数时 sweep 直接拒绝。并行 agent / 编译 / 索引器必须停掉；刚结束的重负载会让 1m 平均值滞后偏高，等它回落再跑。运行中保持安静是操作者纪律——门禁只把守入口。
2. 把 §4 的 TOML 存为临时 spec（建议 `target/tune-sweep-spec.toml`，不入库），然后：

   ```
   cargo run -p xtask -- tune --sweep target/tune-sweep-spec.toml
   ```

   holdout 集默认 `crates/ime-core/tests/fixtures/lm_holdout.tsv`，与 `xtask quality` 同源；可用 `--holdout` 换。
3. 每个网格点的编译日志直接继承 stdout（`dictc:` 行），点与点之间以 `sweep: [i/N] <参数>` 分隔；最终表按 `top1_overall` 降序（tie-break：2字带 → 编译时长 → 参数字典序）。
4. scratch 容器写在 `target/tune-sweep/`，测完即删；全程不触碰 `data/compiled/base.dict` 与任何配置文件。

## 6. 推荐表（待回填）

跑完把 stdout 的排名表与敏感性摘要原样粘贴到此处（工具输出即存档格式，不另行造表）。列含义：

| 列 | 含义 |
|---|---|
| `#` | 排名（1 = 最优） |
| `top1` | holdout 总体 top1（目标函数，降序） |
| `2字` / `1字` / `3+字` | 分带 top1（2字为核带，tie-break 第一优先） |
| `top9` | 首页可达率 |
| `unreach` | 容器完全不服务的用例数 |
| `compile` | 该点编译墙钟时长（tie-break 第二优先） |
| `parameters` | 该点的参数赋值 |

分两批按 §4 拟定网格执行（先归一化两参数联合，再 bigram 两参数联合）。以下为工具输出原样存档。

**Batch 1：归一化两参数联合（`core_min_freq` × `tail_flat_weight`，12 点）**

```text
sweep: machine clean: load 1.35 on 16 cores (ceiling 4.00)
sweep: 12 point(s) over 2 parameter(s); holdout 6000 cases from crates/ime-core/tests/fixtures/lm_holdout.tsv
sweep: #    parameters                                           top1       2字       1字      3+字     top9  unreach   compile
sweep: 1    core_min_freq=80 tail_flat_weight=50               73.75%   88.32%   15.78%  100.00%   96.35%        0   15.36s
sweep: 2    core_min_freq=40 tail_flat_weight=200              73.75%   88.32%   15.78%  100.00%   96.35%        0   15.50s
sweep: 3    core_min_freq=40 tail_flat_weight=50               73.75%   88.32%   15.78%  100.00%   96.35%        0   15.80s
sweep: 4    core_min_freq=80 tail_flat_weight=100              73.75%   88.32%   15.78%  100.00%   96.35%        0   15.88s
sweep: 5    core_min_freq=80 tail_flat_weight=200              73.75%   88.32%   15.78%  100.00%   96.35%        0   16.23s
sweep: 6    core_min_freq=20 tail_flat_weight=50               73.75%   88.32%   15.78%  100.00%   96.35%        0   16.37s
sweep: 7    core_min_freq=20 tail_flat_weight=200              73.75%   88.32%   15.78%  100.00%   96.35%        0   16.72s
sweep: 8    core_min_freq=40 tail_flat_weight=100              73.75%   88.32%   15.78%  100.00%   96.35%        0   18.57s
sweep: 9    core_min_freq=10 tail_flat_weight=50               73.75%   88.32%   15.78%  100.00%   96.35%        0   20.99s
sweep: 10   core_min_freq=10 tail_flat_weight=200              73.75%   88.32%   15.78%  100.00%   96.35%        0   21.29s
sweep: 11   core_min_freq=10 tail_flat_weight=100              73.75%   88.32%   15.78%  100.00%   96.35%        0   21.32s
sweep: 12   core_min_freq=20 tail_flat_weight=100              73.75%   88.32%   15.78%  100.00%   96.35%        0   21.94s
sweep: sensitivity (best top1 over the points holding each declared value):
sweep:   core_min_freq: 20 -> 73.75% (3 pt) | 10 -> 73.75% (3 pt) | 40 -> 73.75% (3 pt) | 80 -> 73.75% (3 pt)
sweep:   core_min_freq: spread 0.00pp across the declared values
sweep:   tail_flat_weight: 100 -> 73.75% (4 pt) | 50 -> 73.75% (4 pt) | 200 -> 73.75% (4 pt)
sweep:   tail_flat_weight: spread 0.00pp across the declared values
sweep: recommendation: core_min_freq=80 tail_flat_weight=50
```

**Batch 2：bigram 两参数联合（`max_pairs` × `min_score`，9 点）**

```text
sweep: machine clean: load 1.68 on 16 cores (ceiling 4.00)
sweep: 9 point(s) over 2 parameter(s); holdout 6000 cases from crates/ime-core/tests/fixtures/lm_holdout.tsv
sweep: #    parameters                                           top1       2字       1字      3+字     top9  unreach   compile
sweep: 1    max_pairs=120000 min_score=1                       73.75%   88.32%   15.78%  100.00%   96.35%        0   14.88s
sweep: 2    max_pairs=200000 min_score=0.5                     73.75%   88.32%   15.78%  100.00%   96.35%        0   14.90s
sweep: 3    max_pairs=400000 min_score=1                       73.75%   88.32%   15.78%  100.00%   96.35%        0   15.14s
sweep: 4    max_pairs=400000 min_score=2                       73.75%   88.32%   15.78%  100.00%   96.35%        0   15.27s
sweep: 5    max_pairs=400000 min_score=0.5                     73.75%   88.32%   15.78%  100.00%   96.35%        0   15.27s
sweep: 6    max_pairs=120000 min_score=2                       73.75%   88.32%   15.78%  100.00%   96.35%        0   15.32s
sweep: 7    max_pairs=120000 min_score=0.5                     73.75%   88.32%   15.78%  100.00%   96.35%        0   15.56s
sweep: 8    max_pairs=200000 min_score=2                       73.75%   88.32%   15.78%  100.00%   96.35%        0   16.20s
sweep: 9    max_pairs=200000 min_score=1                       73.75%   88.32%   15.78%  100.00%   96.35%        0   17.91s
sweep: sensitivity (best top1 over the points holding each declared value):
sweep:   max_pairs: 200000 -> 73.75% (3 pt) | 120000 -> 73.75% (3 pt) | 400000 -> 73.75% (3 pt)
sweep:   max_pairs: spread 0.00pp across the declared values
sweep:   min_score: 1 -> 73.75% (3 pt) | 0.5 -> 73.75% (3 pt) | 2 -> 73.75% (3 pt)
sweep:   min_score: spread 0.00pp across the declared values
sweep: recommendation: max_pairs=120000 min_score=1
```

两批的工具推荐行均为**同分下的编译时长 tie-break**（21 点总分与全部分带完全一致），不是质量信号；见 §7 结论与 §8 裁决。现行工作点即 Batch 1 第 12 行 / Batch 2 第 9 行，复测 73.75% 与既往基线一致。

## 7. 参数敏感性摘要（待回填）

（待回填。关注点：某参数各取值的 best top1 拉不开（spread ≈ 0）说明该参数在此网格密度下目标函数分辨不出——这本身是结论，登记「本轮不动该参数」即可，不视为失败。）

**首轮实测结论（2026-11-17）**：四个参数 spread 全部 0.00pp——

- `core_min_freq`（20/10/40/80）、`tail_flat_weight`（100/50/200）、`max_pairs`（200k/120k/400k）、`min_score`（1.0/0.5/2.0）在受测范围内对 holdout top1 及全部分带**零分辨力**（21 点总分、分带、top9、unreach 完全一致）。
- 按 §7 协议登记：**本轮四个参数全部不动**，不视为失败。
- 附带观察：`unreach` 全程为 0、`top9` 恒 96.35%，说明 top1 的 26.25% 缺口不在「容器服务不到」也不在这四个构建期旋钮的烤入排序上；改进抓手在解码/权重侧（`tune --grid` 权重通道，TASK-4.03.01 已覆盖）与 §3 列出的尚不可观测通道（衰减、上文带）。

## 8. 裁决记录（骨架，待回填）

```
日期：            2026-11-17（首轮实跑）；裁决签署：2026-11-17
机器状态：        uptime 4:34；1m 负载 1.35（batch1）/ 1.68（batch2）；核数 16
                  （sweep 启动时门禁放行读数，ceiling 4.00 = 25% × 16）
采用工作点：      （建议）维持现状，不回写——core_min_freq=20 tail_flat_weight=100
                  max_pairs=200000 min_score=1.0
否决的备选点：    batch1 机械推荐 core_min_freq=80 tail_flat_weight=50；
                  batch2 机械推荐 max_pairs=120000 min_score=1。
                  否决理由：与现行点总分及全部分带完全相同（73.75%），「推荐」仅为
                  编译时长 tie-break，采纳无质量增益且扩大 diff；按 §7 协议
                  spread≈0 → 登记本轮不动
回写位置：        无回写（xtask/src/dictc.rs 与 crates/ime-config/src/schema.rs 均不动）
门禁值变化：      无（总体 top1 门禁维持 73.75%；2字带门禁维持 88.32%）
回写前后对照：    无回写；现行点在两批 sweep 中复测均为 73.75%（batch1 第12行 /
                  batch2 第9行），与既有基线一致，「回写后 ≥ 回写前」平凡满足
回写后验证：      不适用（无回写、门禁值未变）
裁决理由：        两批 sweep 21 点总分与全部分带完全一致（73.75% / 2字带 88.32% /
                  1字带 15.78% / top9 96.35% / unreach 0），四个构建期旋钮在受测
                  范围内对 holdout 目标零分辨力；两行工具「recommendation」仅为
                  同分编译时长 tie-break，采纳无质量增益且扩大 diff，按 §7 协议
                  登记本轮不动。门禁值只升不降：总体 top1 维持 73.75%、2字带
                  维持 88.32%。top1 缺口的后续抓手在解码/权重侧（tune --grid
                  权重通道）与 §3 尚不可观测通道（衰减、上文带，需先扩展目标函数）。
裁决人：          gongzhengyang（用户于 2026-11-17 会话中确认「维持现状，不回写」）
```

## 9. 纪律与红线

- 工具只推荐，不回写；门禁值只升不降。
- 负载机显式拒绝、不误报：拒绝文本带可行动指引（关负载 / 等负载回落 / 降网格密度）。
- 推荐表只承载参数名与指标，不含词典内容与用户输入。
- 空闲机首轮的实测数字只进本文档 §6–§8 与 `docs/dev/features.md` 的验收记录，不进代码注释。
