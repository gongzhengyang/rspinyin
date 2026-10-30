# 首轮参数整定（holdout top1 网格 sweep）— 模板

> 状态：**模板**。本文档不含任何实测数字；所有「待回填」栏在空闲机首轮 sweep 落盘后由人工回填。
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

（待回填）

## 7. 参数敏感性摘要（待回填）

（待回填。关注点：某参数各取值的 best top1 拉不开（spread ≈ 0）说明该参数在此网格密度下目标函数分辨不出——这本身是结论，登记「本轮不动该参数」即可，不视为失败。）

## 8. 裁决记录（骨架，待回填）

```
日期：            （____）
机器状态：        uptime ____；1m 负载 ____；核数 ____（sweep 启动时门禁放行的读数）
采用工作点：      （逐参数列出；或「维持现状，不回写」）
否决的备选点：    （表内次优点及否决理由，如：总 top1 持平但 2字带下降）
回写位置：        dictc 默认值（xtask/src/dictc.rs）与/或 ime-config 默认值
                  （crates/ime-config/src/schema.rs）——由人工编辑，工具不自动回写
门禁值变化：      总体 top1 门禁 ____ → ____（必须 ≥）；2字带门禁 ____ → ____（必须 ≥，只升不降）
回写前后对照：    回写前 holdout top1 ____；回写后 holdout top1 ____（必须 ≥ 回写前）
回写后验证：      `cargo run -p xtask -- quality` 全绿（门禁值按上两行更新后）
裁决理由：
裁决人：
```

## 9. 纪律与红线

- 工具只推荐，不回写；门禁值只升不降。
- 负载机显式拒绝、不误报：拒绝文本带可行动指引（关负载 / 等负载回落 / 降网格密度）。
- 推荐表只承载参数名与指标，不含词典内容与用户输入。
- 空闲机首轮的实测数字只进本文档 §6–§8 与 `docs/dev/features.md` 的验收记录，不进代码注释。
