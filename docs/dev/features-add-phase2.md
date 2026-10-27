# dev-features-add — Phase 2 增量原子任务分片

> 所属主控文档: [./features-add.md](features-add.md) ｜ 关联对比报告: [./compare-and-improve.md](compare-and-improve.md) ｜ 阶段定位: Phase 2 (P1 效率跃升：已有能力接线 + 数据生态 + 会话共享) ｜ 包含任务区间: TASK-2.01.01 ~ TASK-2.06.01（9 卡，预估 18 人天）
>
> 阶段目标：把「引擎能力在、生产链路够不着」的三件事（模糊音/简拼/简繁）接通到端到端可用，把两字词层的数据质量换血（华宇真频），补上会话共享与数据驱动冒烟两块体验/工程短板。除 `TASK-2.02.01`（prefix 实装）外全部是接线与数据工作。

---

#### TASK-2.01.01 模糊音配置节与会话接线

- **基本属性**：
  - 绑定差距与改进项：`GAP-05` / `IMP-06` ｜ 关联设计假设：`ASM-A-02`（独占预算 ≤0.60ms）、`ASM-A-10`
  - 优先级与复杂度：`P1 效率进阶` ｜ 低 ｜ 预估工时： 1.5 人天
  - 前置依赖：`无`（fuzzy 扩边引擎已在 `crates/ime-core/src/fuzzy.rs` 全量实现）
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine`
  - 对标参考锚点 (Benchmark Reference)：librime `fuzz` 运算的物化语义（[librime.md](../compare/librime.md) §6，`calculus.cc:214-221`）；rime-ice `rime_ice.schema.yaml` 的 20+ 条模糊音模板（单向/成对 derive，注释待启用形态）
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-config/src/schema.rs` + `reload.rs`（新增 `[fuzzy]` 节）+ `DEFAULT_CONFIG_TOML`；修改 `crates/ime-core/src/state/machine.rs:454` 附近（把投影后的模糊音位并入 `self.request.flags`）
  - 引入依赖白名单：`无`
  - 当前状态：`[x] 已完成`
- 落地证据：crates/ime-config [fuzzy] 节：reload/tests.rs (test_fuzzy_class_flags_maps_the_section_onto_the_frozen_bits, test_fuzzy_section_merges_key_by_key_over_the_defaults)；state/tests/transitions.rs (test_fuzzy_classes_latched_from_the_config_widen_the_lattice, test_fuzzy_reload_latches_at_the_next_composition)
- **目标与价值**：8 类模糊音扩边（`DecodeFlags::FUZZY_ZH_Z` … `FUZZY_F_H`，`ime-types/src/decode.rs:56-72`）引擎侧全部实现，但生产会话只设 `SHUANGPIN` 位（`machine.rs:454`），用户没有任何开关能打开它。本卡是典型「最后一公里接线」：配置节 → 投影 → 解码位，~1 天量级换来方言口音用户的可用性格变。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：配置节（模板片段）：

    ```toml
    [fuzzy]
    # Each entry expands one equivalence class at lattice-build time (the
    # engine capability is `crates/ime-core/src/fuzzy.rs`; these keys only set
    # the decode flags). `enabled = true` turns every class on at once; an
    # individual `true` wins over the master switch, so a user can enable all
    # and then subtract one.
    enabled = false
    zh_z = false
    ch_c = false
    sh_s = false
    n_l = false
    f_h = false
    an_ang = false
    en_eng = false
    in_ing = false
    ```

    投影与会话接线：

    ```rust
    // ime-core, state/machine.rs -- next to the SHUANGPIN set:
    // The fuzzy projection lands once per config reload, so the decode flags
    // assemble here instead of per keystroke: `fuzzy_flags` is the precomputed
    // `DecodeFlags` mask the config store hands to the session host.
    self.request
        .flags
        .insert(env.fuzzy_flags & DecodeFlags::FUZZY);
    ```

  - **边界契约**：`DecodeFlags` 位定义冻结不动；`FUZZY` 主位（1<<0）语义为「任一类开启」——投影层负责维护主位与各位的一致性（开任一类即置主位）。
  - **交互与工艺级规范**：Headless——坏值走既有「修默认 + `config/invalid`」；模板与 `Config::default` 防漂移测试同步。
  - **与现有代码的无缝衔接方案**：投影产物（`DecodeFlags` 掩码）挂在会话宿主既有配置注入点（`addon.rs` INIT_STEPS 的 config 步产物）；`machine.rs` 在组装 `request` 处并入，与 `SHUANGPIN` 位同一位置，无新路径。
- **底层与非功能约束 (NFR)**：
  - 扩边只在 flag 开启时发生，关闭位时 decode 路径零新增（预算 ≤0.60ms 仅在开启档生效，criterion 开/关双档基准）。
  - 边缘条件：全开 8 类 + 双拼叠加 → 边数上限受 lattice 既有四轴限界约束（`MAX_TOTAL_EDGES` 等），断言不回退。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `[fuzzy]` schema/reload/模板三处 + 防漂移测试 + `config/invalid` 坏值用例。
  2. `[步骤 2]` 会话宿主注入 `fuzzy_flags` + `machine.rs` 并入；单测（开一类 → 对应等价类扩边生效；全关 → 与现状逐字节一致）。
  3. `[步骤 3]` `xtask testd-engine` 用例：`zhong` 开 zh_z 后能命中「总/宗」两族；budget 双档基准。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-config -p ime-core -p ime-fcitx5` 全绿。
  - [ ] 走查：`testd-engine` 模糊音用例过；cheatsheet 无需变更（模糊音非键位）。
  - [ ] 回归走查：默认全关时解码结果与主线逐字节一致（确定性指纹测试不回退）。

---

#### TASK-2.02.01 `FstLexicon::prefix` 实装（FST 前缀区间流式走查）

- **基本属性**：
  - 绑定差距与改进项：`GAP-06` / `IMP-07` ｜ 关联设计假设：`ASM-A-02`（独占预算 ≤0.50ms）、`ASM-A-10`（**契约签名已冻结，零变更**）
  - 优先级与复杂度：`P1 效率进阶` ｜ 中 ｜ 预估工时： 2.5 人天
  - 前置依赖：`无`
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine`
  - 对标参考锚点 (Benchmark Reference)：librime Prism 的 `CommonPrefixSearch`/`ExpandSearch`（[librime.md](../compare/librime.md) §2.2、§4.3——darts 双数组的前缀枚举）；features.md `TASK-2.02.02` 索引预写的「`fst::map::Streamer` 前缀枚举」思路
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-dict/src/fst_index.rs:399-403`（`FstLexicon` 覆写 `prefix` 默认实现）；`crates/ime-types/src/lexicon.rs` **零改动**（`lexicon.rs:49` 的签名与文档已是最终语义）
  - 引入依赖白名单：`无`（`fst` crate 已在依赖树，stream/range API 内置）
  - 当前状态：`[x] 已完成`
- 落地证据：crates/ime-dict/src/fst_index.rs FstLexicon 覆写 prefix 流式前缀枚举；fst_index/tests.rs (test_prefix_streams_the_words_of_every_key_in_the_range, test_prefix_of_an_exact_key_answers_that_key_alone, test_prefix_stops_at_the_first_key_outside_the_range, test_prefix_pushes_the_limit_down_into_the_walk)
- **目标与价值**：`prefix → Unsupported` 一行堵死了三条路：简拼生产化（`lattice/abbrev.rs:145` 静默跳过）、词补全、尾部补全。容器注释早已预告实现形态——「对同一 FST 的前缀区间做一次流式走查」（`fst_index.rs:399` 注释）。本卡兑现它：`fst::Map` 的 range stream（`ge(prefix)` 起步、逐键比对前缀、越过区间即停），WORDLIST 间接层照常解引用，零拷贝语义不变。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：

    ```rust
    fn prefix(&self, prefix: &str, limit: usize) -> Result<WordIter<'_>, ImeError> {
        // Keys are the `'`-joined syllable strings, so an abbreviated query
        // like `n'h` is itself a literal key prefix: the FST orders keys
        // lexicographically, and every key that starts with `prefix` forms a
        // contiguous range. The walk stops at the first key that diverges,
        // which is what keeps a hot prefix cheap and a miss O(log n).
        let mut buf = WordBuf::new();
        let mut stream = self.map.stream().ge(prefix);
        while let Some((key, payload)) = stream.next() {
            let Some(key) = core::str::from_utf8(key).ok() else { break };
            if !key.starts_with(prefix) {
                break; // past the prefix range: lexicographic order says so
            }
            self.push_payload(payload, &mut buf);
            if buf.len() >= limit {
                break; // the caller's limit is pushed all the way down
            }
        }
        Ok(buf.into_word_iter())
    }
    ```

    *（骨架示意：`push_payload` 复用 `lookup` 已有的 `(start<<24|count)` 解码与 `read_words_into` 尾部追加；实际实现沿用 `WordBuf` 8 词内联、溢出转堆的既有纪律。）*
  - **边界契约**：`Lexicon::prefix` 签名与文档零改动；语义补全进方法的既有 doc comment（返回词的排序遵循 WORDLIST 的 weight 降序——与 `lookup` 一致，跨键之间按 FST 字典序）。`ImeError::Unsupported` 语义从「FstLexicon 不可服务」收窄为「词库未加载/非法键」（ syllable 表外标识符的行为对齐 `fallback_single` 的既有注释）。
  - **交互与工艺级规范**：无 UI 面。
  - **与现有代码的无缝衔接方案**：本卡落地后 `lattice/abbrev.rs:145` 的 `else` 静默跳过分支自动消失（对真实词库不再触发），简拼 walk 无需改动即被激活——这正是该 walk 从第一天就按「prefix 有实现」写的原因。
- **底层与非功能约束 (NFR)**：
  - 独占预算 ≤0.50ms（简拼路径含本查询）：`PREFIX_LIMIT` 截断下推；criterion 基准 `dict/prefix_hit`（热前缀）/`dict/prefix_miss`（无命中区间）。
  - 零拷贝：返回词仍借 mmap（`WordIter<'_>` 生命周期不变）；UTF-8 键边界：FST 键为编译期验证的 UTF-8，运行期 `from_utf8` 失败按 corrupt 段处理（防御性，不可达时注释声明）。
  - 边缘条件：`limit = 0` → 空迭代；`prefix` 含 `'` 或超长 → 与 `lookup` 同一校验路径；空串前缀 → 全库枚举必须被 `limit` 截断（防退化）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` range walk 实现 + 单测：前缀命中、恰好整键、字典序越界停止、limit 截断、空串防退化（`test_prefix_<scenario>_<expectation>`）。
  2. `[步骤 2]` 与 mock lexicon 对拍：`ime-core` 既有简拼测试（mock 前缀实现）与真实词库行为一致；`lattice/abbrev.rs` 静默跳过分支的测试更新。
  3. `[步骤 3]` criterion 双基准 + `xtask quality` 回归（prefix 不进默认解码路径，top1 不变断言）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-dict -p ime-core` 全绿；`cargo test --workspace --doc` 不回退。
  - [ ] 基准证据：`dict/prefix_hit/miss` 数字写入验收记录；简拼端到端行为留待 `TASK-2.02.02` 验收。
  - [ ] 回归走查：`lookup`/`fallback_single` 既有测试零回退；`check-dict` 与四门禁全绿。

---

#### TASK-2.02.02 简拼生产接线（engine.abbrev 转正）

- **基本属性**：
  - 绑定差距与改进项：`GAP-06` / `IMP-07` ｜ 关联设计假设：`ASM-A-02`（≤0.50ms，与 2.02.01 共享预算位）、`ASM-A-10`
  - 优先级与复杂度：`P1 效率进阶` ｜ 低 ｜ 预估工时： 1.5 人天
  - 前置依赖：`TASK-2.02.01`
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine`
  - 对标参考锚点 (Benchmark Reference)：rime-ice 超级简拼——`erase/^hm$|^m$|^n$|^ng$/` + `abbrev/^([a-z]).+$/$1/` + `abbrev/^([zcs]h).+$/$1/`（`ch'sh`→城市，[rime-ice.md](../compare/rime-ice.md) §3.3）；librime abbrev 的 credibility −log0.5 对数罚（`calculus.cc:238-245`）
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-config/src/reload.rs:125-127`（删去 "Not wired yet" 注释并转正）、`crates/ime-core/src/state/machine.rs`（abbrev 位并入 flags，与 2.01.01 同一注入点）
  - 引入依赖白名单：`无`
  - 当前状态：`[x] 已完成`
- 落地证据：crates/ime-config/src/reload/merge.rs abbrev 配置转正接线；state/tests/transitions.rs (test_abbrev_switch_expands_the_initial_reading)
- **目标与价值**：`engine.abbrev` 配置键存在（默认 false）但注释明言「Not wired yet」；`segment/abbrev.rs` 代数与 `lattice/abbrev.rs` walk 完整、简拼罚 `ABBREV_PENALTY_Q8` 已带。prefix 实装后唯一缺口就是这根线。rime-ice 的「超级简拼」（zh/ch/sh 整体取首两母）语义已在 rspinyin 的简拼代数中有对应物。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：与 2.01.01 完全同构的一根线：

    ```rust
    // state/machine.rs, decode-flag assembly (one line beside the fuzzy set):
    self.request.flags.set(DecodeFlags::ABBREV, env.abbrev_enabled);
    ```

    配置注释转正（`reload.rs`）：`Off by default: an abbreviation is ambiguous by nature.` 保留，删 `Not wired yet.`，补一句指向 `decode/abbrev` 的诊断码（若有）与预算说明。
  - **边界契约**：`DecodeFlags::ABBREV` 位语义不变；简拼罚常量 `ABBREV_PENALTY_Q8` 不动（等价 librime 的 −log0.5，K-4 的容差分级已在）。
  - **交互与工艺级规范**：无 UI 面；候选注释位可标注简拼来源（若 `Candidate` 已带来源标签则透传，无则不加——不扩 UiFrame）。
  - **与现有代码的无缝衔接方案**：会话宿主配置注入点与 2.01.01 相同（一个 `env.abbrev_enabled` 布尔）；开/关双态的确定性对拍测试模式复用。
- **底层与非功能约束 (NFR)**：
  - 默认关：关闭时 decode 与主线逐字节一致；开启时简拼候选与全拼同列、简拼罚保证不压过全拼（对拍断言：同输入下全拼候选序保持）。
  - 边缘条件：单字母输入（`n`）→ 大量前缀命中，由 `PREFIX_LIMIT` 与 lattice 四轴限界双重截断；断言 decode 预算不破。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 配置转正 + 注入线 + 双态单测。
  2. `[步骤 2]` `testd-engine` 简拼用例：`nh` → `你好` 进前 3；`bjdx` → `北京大学`（视词库覆盖）。
  3. `[步骤 3]` budget 开档基准 + 文档（模板注释、cheatsheet 若登记该开关）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-config -p ime-core` 全绿。
  - [ ] 走查：真实词库下 `nh`/`nih` 简拼命中（对比报告 K-4②的验收场景）；预算内。
  - [ ] 回归走查：默认关档逐字节一致；`xtask quality` 不回退。

---

#### TASK-2.03.01 简繁转换 commit 接线与降级上报

- **基本属性**：
  - 绑定差距与改进项：`GAP-07` / `IMP-08` ｜ 关联设计假设：`ASM-A-10`（`ui/script/unavailable` 既有码接上报方，禁新造码）
  - 优先级与复杂度：`P1 效率进阶` ｜ 中 ｜ 预估工时： 2.5 人天
  - 前置依赖：`无`（script 算法与 `ScriptIndex` 已实现：`crates/ime-core/src/script.rs` + `crates/ime-dict/src/script.rs`）
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `traditionalize` 开关（switches 声明 + `states/` 标签联动，[rime-ice.md](../compare/rime-ice.md) §3.1）；librime OpenCC 通道的「引擎组件读数据」形态
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-core/src/state/transitions.rs:192-198`（`ToggleScript` 从「仅重发帧」补全为置位 + 效果）、`crates/ime-core/src/state/effects.rs`（`Effect::ScriptUnavailable` 若需新效果变体）、`crates/ime-fcitx5/src/engine/router/modes.rs:370` 附近（`transform_output` 挂 script 变换）、`crates/ime-fcitx5/src/engine/binding_audit.rs:288-297`（解除「v1 不启用」注释与审计豁免）
  - 引入依赖白名单：`无`
  - 当前状态：`[x] 已完成`
- 落地证据：crates/ime-fcitx5/src/engine/router/tests.rs (test_script_toggle_converts_the_commit_and_toggles_back)；state transitions ToggleScript 置位+效果；binding_audit v1 不启用豁免解除
- **目标与价值**：简繁算法（最长匹配 + 内置种子变体表 + `ScriptIndex` 降级值）与 `ToggleScript` 键位全在，commit 路径却只走 `passthrough::transform_committed`（标点/全宽）；`ui/script/unavailable` 错误码已登记但没有上报方（features.md 验收记录明确）。本卡一次补齐三件事：模式置位、commit 变换、降级上报——繁体用户从「能切但没效果」到真实可用。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：commit 变换挂在输出管线末尾（`modes.rs` 的 `transform_output` 邻位）：

    ```rust
    /// The commit transform pipeline: passthrough first (it may pass the text
    /// through untouched, borrowing), then the script mapping when the
    /// session is in traditional mode. A character the variant table does not
    /// cover stays simplified -- the commit never fails and never blocks.
    pub(super) fn transform_output<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let text = transform_committed(text, self.is_punct_full, self.is_full_width);
        if self.is_traditional {
            self.script.convert(text) // Cow-in, Cow-out; miss = identity
        } else {
            text
        }
    }
    ```

    状态机（`transitions.rs`）：`ToggleScript` 置 `is_traditional` 位 + 重发帧（preedit/候选是否即时转繁由 `ScriptIndex::unavailable()` 降级语义决定——**词库未带变体表时只报不切**：emit `Effect::Diagnose(ImeError::...)` 携既有码 `ui/script/unavailable`，模式位不置）。
  - **边界契约**：跃迁 `Composing --ToggleScript--> Composing`（有变体表：置位/清位 + 帧重发；无表：`ui/script/unavailable` 诊断 + 状态不变）。错误码零新增（X-10：接上唯一缺的**上报方**）。
  - **交互与工艺级规范**：切换无气泡（自绘窗无通知区）；状态可见性由 `TASK-3.06.02` 的 StatusArea 承接（本卡在 cheatsheet 键位说明补「简↔繁」行）。
  - **与现有代码的无缝衔接方案**：`ScriptIndex` 由词库装配点（`addon.rs` lexicon 步）注入会话宿主，随 `Context` 持有；`script.rs` 的 `convert` 已是纯函数（Cow 进出、未命中恒等），管线挂接零分配（非繁模式完全借用路径）。
- **底层与非功能约束 (NFR)**：
  - commit 预算：繁模式仅对最终提交文本做一次最长匹配（不进 Viterbi），增量 ≤0.30ms 计入 commit 路径（非 decode_p99，criterion `commit/script_on` 基准）；简模式零增量（借用透传）。
  - 失败语义：变体表缺失/降级 → 提交原文 + 诊断，**绝不阻断提交**（主控 §3.3-3）；病态输入（超长提交）由既有 commit 上限保护。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 会话宿主持有 `ScriptIndex` + `Context` 增 `is_traditional` 位；`ToggleScript` 状态机补全 + 降级上报（`ui/script/unavailable` 上报方落地，features.md 验收缺口闭环）。
  2. `[步骤 2]` `transform_output` 管线挂接 + `binding_audit` 审计豁免解除（`KeyAction::ToggleScript` 进入实际执行矩阵）。
  3. `[步骤 3]` 测试：简↔繁往返、缺字保简、无表降级、简模式零分配断言；`testd-engine` 繁体提交用例。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-core -p ime-fcitx5` 全绿；`ui/script/unavailable` 的上报方存在性断言（诊断抽取测试扩展）。
  - [ ] 走查：`testd-engine` 用例 `nihao`+繁模式+空格 → `你好` 的繁体形态上屏（读回通道断言）。
  - [ ] 回归走查：简模式（默认）提交路径逐字节一致；`binding_audit` 双向穷尽断言恢复。

---

#### TASK-2.03.02 标点/全宽配置默认值投影

- **基本属性**：
  - 绑定差距与改进项：`GAP-07` / `IMP-08` ｜ 关联设计假设：`ASM-A-10`、`ASM-A-07`（会话初值来源变更需登记）
  - 优先级与复杂度：`P1 效率进阶` ｜ 低 ｜ 预估工时： 0.5 人天
  - 前置依赖：`无`（与 2.03.01 无依赖；同碰 `modes.rs`，**同文件串行声明**）
  - 关键路径：`否`
  - 并行通道：`Track B UI·Client·CLI`
  - 对标参考锚点 (Benchmark Reference)：fcitx5-rime 的 preedit/模式初值协商（[fcitx5-rime.md](../compare/fcitx5-rime.md) §4/§6——配置初值进会话创建路径）；纠偏表 X-2
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-fcitx5/src/engine/router/modes.rs:221-230`（`Modes::new` 接投影默认值）
  - 引入依赖白名单：`无`
  - 当前状态：`[x] 已完成`
- 落地证据：crates/ime-fcitx5/src/engine/router/modes.rs Modes::new 接投影默认值；router/modes/tests.rs (test_transform_output_default_modes_leave_ascii_and_cjk_alike_untouched)；router/config/tests.rs (test_from_config_projects_the_shipped_document_onto_the_default_view)
- **目标与价值**：纠偏表 X-2 的结论：`punct_mode`/`full_width` 的运行期切换已接线（和弦 + `transform_committed`），缺的只是 `[engine]` 配置默认值到会话初始位的投影——`Modes::new` 用硬编码默认（`modes.rs:221-230` 注释自述）。用户改配置文件不生效，这是配置工程一致性缺口。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：

    ```rust
    // The two output bits stop being constants: the config projection hands
    // them in at session creation, the chord still flips them at runtime, and
    // a reloaded document reaches the next session (per-context semantics are
    // unchanged; an in-flight session keeps its own bits -- 0.4 rule 10).
    impl Modes {
        pub(super) fn from_projection(p: OutputProjection) -> Self;
    }
    ```

  - **边界契约**：会话初值来源变更登记（配置 → 会话）；重载语义「新会话生效」与既有 `data.durability` 等 restart/reload 三态标注对齐（本键属 reload 生效档——`reload.rs` 已投影该键，只差消费）。
  - **交互与工艺级规范**：无新 UI。
  - **与现有代码的无缝衔接方案**：`Modes::new` 调用点（会话创建）从投影后的 config 取值；和弦翻转路径（`modes.rs:331`）不动。
- **底层与非功能约束 (NFR)**：零热路径变化（初值赋值一次）；坏值已由 config 层修复，此处信任投影。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `OutputProjection` 结构与 `Modes` 改造 + 单测（配置 `full_width = true` → 新会话首字符即全宽；在途会话不受重载影响）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-fcitx5` 全绿。
  - [ ] 走查：改配置 → 重启/新会话生效；运行期和弦切换仍即时。
  - [ ] 回归走查：默认值行为与现状一致。

---

#### TASK-2.04.01 词源接入：华宇野风（PD）与 THUOCL（MIT）

- **基本属性**：
  - 绑定差距与改进项：`GAP-08` / `IMP-15`（CP-2 中段）｜ 关联设计假设：`ASM-A-08`、`ASM-A-03`
  - 优先级与复杂度：`P1 效率进阶` ｜ 中 ｜ 预估工时： 2 人天
  - 前置依赖：`无`（与 1.01.01 的归一化管线解耦：本卡只管「源进来」，2.04.02 管「用起来」）
  - 关键路径：`是`（CP-2）
  - 并行通道：`Track C Infra·DevOps`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `others/docs/Credits.md` 的上游登记与 `data/sources.toml` 白名单机制的对接设计（[rime-comparison.md](../compare/rime-comparison.md) §4.7 A 档清单）
  - 本项目代码落地锚点 (Code Anchor)：修改 `xtask/fetch.sh`（两源下载步骤）；`docs/dev/licenses.md`（逐源登记）；~~`data/sources.toml` 条目与 `scripts/check-dict-sources.sh` 排除名单~~ **已由 ADR-0012 落地完成（2026-10-14）**：`thuocl`（MIT, L4）与 `huayu`（PD→CC0 等价, L2）条目已登记，校验器 PASS（6 raw / 8 declared）——本卡只需回填 sha256/retrieved 与下载文件
  - 引入依赖白名单：`无`
  - 当前状态：`[x] 已完成`
- 落地证据：data/fetch.sh thuocl/huayu 下载步骤；data/sources.toml thuocl(L4)/huayu(L2) 条目含 sha256+retrieved；docs/dev/licenses.md 逐源登记；scripts/check-dict-sources.sh PASS (11 raw / 11 declared)
- **目标与价值**：两字词「输入法场景真频」的主力源（华宇野风，rime-ice base 的第一来源，Public Domain 许可最干净）与 3+ 字领域长尾源（THUOCL，MIT，按领域分文件）接进白名单。**许可裁决已落盘：ADR-0012（2026-10-14 用户批准）解除了 THUOCL 的预排除并登记了 PD→CC0 等价声明**，`sources.toml` 条目与校验器排除名单已同步。本卡的步骤 1 因此是执行性工作（fetch、hash 回填、登记），不再是裁决流程。从原始上游取数、不经 rime-ice 派生，仍是许可合规的硬边界（`ASM-A-08`）。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：`sources.toml` 新条目（对齐既有字段规则）：

    ```toml
    [[source]]
    id = "huayu"
    kind = "upstream"
    layer = "L2"
    url = "<华宇野风原始发布地址，fetch 步骤固化>"
    license = "Public Domain (huayu system wordlist, per rime-ice Credits)"
    spdx = "CC0-1.0"       # PD 没有 SPDX 码，以 CC0 等价登记并注释声明
    retrieved = "<落地日>"
    sha256 = "<下载后回填，回填前 CI 必须红>"
    permissive = true
    ```

    `dictc/source.rs` 的 SPDX 校验集合扩 `CC0-1.0`（PD 的等价登记），`check-dict-sources.sh` 同步；`layer = "L2"` 供 `TASK-2.04.02` 的两字层替换消费。
  - **边界契约**：白名单是 ADR-0000 决策 2 的执行面——条目的 ADR 依据已落盘（**ADR-0012**：THUOCL 解除预排除 + PD→CC0 等价登记），`sources.toml` 条目与 `check-dict-sources.sh` 排除名单的同步已完成；本卡只做数据面（fetch / 清洗 / hash 回填 / `licenses.md` 登记）。
  - **交互与工艺级规范**：Headless——`fetch.sh` 可复现（URL 固化 + sha256 pin + 日期）；失败即红，无静默降级。
  - **与现有代码的无缝衔接方案**：dictc 管线零改动（多源已在 `manifest.rs`/`source.rs` 的白名单读取里正交合并）；本卡交付后词库内容暂不变（`TASK-2.04.02` 才接进 L3a/L4 规则）。
- **底层与非功能约束 (NFR)**：
  - 数据纪律：sha256 未回填则 CI 红（既有机制：`sources.toml` 头注释声明的「unfinished entry cannot pass」）；词库红线由 `TASK-2.04.02` 的 budget 实测把关。
  - 复现性：两次 fetch（不同时间/网络）sha256 一致才允许 pin 固化。
- **逐步落地实施步骤**：
  1. `[步骤 1]` 按 ADR-0012 执行：固化两源的原始发布 URL → `fetch.sh` 下载 → sha256/retrieved 回填 `sources.toml` → 格式勘察与清洗为 dictc 可读 TSV（若原始格式非 TSV，转换脚本进 `xtask/` 或 `scripts/`，转换器登记为 `kind = "derived"`）→ `docs/dev/licenses.md` 逐源登记（W-3）。
  2. `[步骤 2]` 下载、格式勘察（列结构/词频列/编码）、清洗为 dictc 可读 TSV（若原始格式非 TSV，转换脚本进 `xtask/` 或 `scripts/`，转换器登记为 `kind = "derived"`）。
  3. `[步骤 3]` 双编译一致性冒烟（含新源）+ `check-dict` 审计确认（登记面已由 ADR-0012 完成，此处验证数据面）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p xtask`（source 解析新 SPDX）+ `just ci` 的 `check-dict` 过。
  - [ ] 数据证据：两源的 sha256/规模/字段样例写入验收记录；许可复核链接（THUOCL 仓库 LICENSE 文件、华宇 Credits 出处）写入 ADR。
  - [ ] 回归走查：不接新源时（`--input` 老路径）编译产物与主线一致。

---

#### TASK-2.04.02 两字词层换血与多音字自动校验

- **基本属性**：
  - 绑定差距与改进项：`GAP-08` / `IMP-15`（CP-2 收口）｜ 关联设计假设：`ASM-A-03`、`ASM-A-08`
  - 优先级与复杂度：`P1 效率进阶` ｜ 中 ｜ 预估工时： 2.5 人天
  - 前置依赖：`TASK-2.04.01`（源就绪）、`TASK-1.01.01`（归一化规则决定权重落带）
  - 关键路径：`是`（CP-2 收口）
  - 并行通道：`Track C Infra·DevOps`
  - 对标参考锚点 (Benchmark Reference)：rime-ice 的多音字纪律——「字表字频比 5:100 阈值作为自动注音开关」+ `others/script/rime/polyphone.go` 机器校验（[rime-ice.md](../compare/rime-ice.md) §1.5、§8）；其 base「两字词必须进 base」的分层维护约束
  - 本项目代码落地锚点 (Code Anchor)：修改 `xtask/src/dictc/build.rs`（L2 合并规则：华宇两字层与 jieba 的频率择优/覆盖策略）与 `xtask/src/dictc/quality.rs`（新校验 pass）；新增 `scripts/polyphone-check.py`（或 xtask 子命令，机器校验多音字词条）
  - 引入依赖白名单：`无`（校验逻辑自包含；若用 Python 脚本则 `-I` 隔离运行，不进 cargo 依赖）
  - 当前状态：`[x] 已完成`
- 落地证据：xtask/src/dictc/build.rs 华宇两字层合并 + PolyphoneRule 自动多音字检查；build/tests.rs (test_merge_two_char_layer_overrides_fills_and_skips_other_lengths, test_merge_two_char_layer_floor_and_order_free, test_compile_applies_polyphone_corrections_without_duplicating_a_word)；dictc/source/tests.rs (test_load_polyphone_refuses_an_illegal_row)
- **目标与价值**：把两字词从「jieba 分词统计频」升级为「输入法场景真频」——这是 top1 的第二大杠杆（CP-2 收口）。同时吸收 rime-ice 的多音字纪律：新源词条过「字频比阈值」自动校验，不达标的词条降层或压平，把 L3c 手工 polyphone.tsv 的维护压力变成机器把关。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：L2 合并规则（build.rs 的源合并段）：

    ```rust
    /// Two-character layer merge: the huayu layer carries IME-scene true
    /// frequencies and wins where both sources have the word; jieba fills the
    /// gaps. The rule is deterministic and order-free -- `huayu` presence is
    /// decided by source id, never by file order.
    pub(crate) fn merge_two_char_layer(jieba: &mut WordMap, huayu: &WordMap);
    ```

    多音字自动校验（quality 新 pass）：

    ```rust
    /// A reading survives automatic assignment only when its character
    /// frequency is at least `freq_ratio` percent of the character's top
    /// reading (rime-ice uses 5:100). Below the line, the word must come
    /// through the explicit polyphone table or it is flattened to its top
    /// reading -- the data-driven counterpart of the manual L3c curation.
    pub struct PolyphoneRule {
        pub freq_ratio_pct: u8,
    }
    ```

  - **边界契约**：无跨边界；`dictc quality` 报告增 `polyphone_auto: {assigned, flattened, manual}` 计数。
  - **交互与工艺级规范**：Headless——校验不过的处置是「降层/压平 + 报告行」，不是编译失败（对齐「绝不阻断」哲学）；但 `quality` 门禁退出码检查 `manual+auto == total`（无静默丢失）。
  - **与现有代码的无缝衔接方案**：合并发生在 L1/L3a 之间既有的源装配区；校验复用 `quality.rs` 的 `Origin` 审计框架（`huayu` 来源自动获得归属统计，`quality.rs:400-487` 的 `Origin` 枚举扩一项）。
- **底层与非功能约束 (NFR)**：
  - 词库红线（`ASM-A-03`）：合并后 ≤65 万条 / ≤19MiB；budget scope 实测把关，超限先收紧 huayu 的收录下限（频率 ≥ N 才收录）。
  - 确定性：合并与校验均为纯函数、顺序无关；双编译 sha256 一致。
  - 质量门：`xtask quality` 的两字带 top1 相对 `TASK-1.01.02` 基线不降（预期升，写验收记录）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `merge_two_char_layer` + Origin 扩展 + 单测（覆盖/择优/顺序无关）。
  2. `[步骤 2]` `PolyphoneRule` 校验 pass + 字频比数据来源（L1 字表已有真频）+ 报告三计数。
  3. `[步骤 3]` 全量编译实测（条数/体积/时长/质量四对照），门禁值评估与验收记录。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p xtask` 全绿；`budget --validate` 过。
  - [ ] 实测证据：词库规模/体积/top1（总分带）四项前后对照写入验收记录。
  - [ ] 回归走查：不接 huayu 的编译路径（老 `--input`）行为不变；`check-dict` 全绿。

---

#### TASK-2.05.01 会话共享粒度（per-program Context 池）

- **基本属性**：
  - 绑定差距与改进项：`GAP-09` / `IMP-12` ｜ 关联设计假设：`ASM-A-07`、`ASM-A-06`、`ASM-A-10`
  - 优先级与复杂度：`P1 效率进阶` ｜ 中 ｜ 预估工时： 3 人天
  - 前置依赖：`无`
  - 关键路径：`否`
  - 并行通道：`Track B UI·Client·CLI`
  - 对标参考锚点 (Benchmark Reference)：fcitx5-rime `RimeSessionPool`（`rimesession.cpp:104-134`：`No/Program/All` 三档键、weak 持有、无引用自动销毁）+ 策略切换快照/重建 + fcitx5 全局 `shareInputState` 联动（`rimeengine.cpp:770-782`）——[fcitx5-rime.md](../compare/fcitx5-rime.md) §2
  - 本项目代码落地锚点 (Code Anchor)：修改 `crates/ime-fcitx5/src/engine/context.rs`（Context 池键策略）、`crates/ime-fcitx5/src/addon.rs`（配置注入与策略）、`crates/ime-config/src/schema.rs` + `reload.rs`（`[engine] share_state = "context" | "program"`）
  - 引入依赖白名单：`无`
  - 当前状态：`[x] 已完成`
- 落地证据：schema.rs ShareState closed_set + EngineConfig.share_state; reload.rs PartialEngine/merge_engine/DEFAULT_CONFIG_TOML + 2 repair tests; router/config.rs RoutingConfig.share; router.rs PoolKey/pool_key/PoolSlot/Context{fresh,adopted}/park + create-or-adopt activate_reported + last-live park on deactivate + pool.clear() on tier switch (snapshot semantics); tests: 7 pool tests (script bit across ICs, temp english, two-program isolation, unknown-app never pools, last-live park, policy switch, context-tier zero-diff) + test_load_repairs_an_unknown_share_state_to_the_context_default + test_reload_adopts_the_program_share_state; cargo nextest run -p ime-fcitx5 = 492 passed (keymap_matrix 2568 green)
- **目标与价值**：现状每个输入上下文独立 `Context{session, modes, anchor, phrases}`，同一程序的两个窗口（编辑器主窗 + 查找框）输入状态互不相通——切窗口丢模式是真实高频痛点。fcitx5-rime 用三档会话池 + 属性传播策略解决；本项目取其 `Program` 档（`All` 全局档不做：跨程序共享模式弊大于利），app 标识复用 `privacy_impl` 已有的 hash 通道（明文程序名不过界）。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：

    ```rust
    /// The key a Context is pooled by. `Context` keeps today's behaviour
    /// (one context per input context); `Program` shares one context across
    /// every input context of the same application, so mode bits, anchor and
    /// phrases survive window switches inside one app.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum PoolKey {
        Context(SessionId),
        /// The app hash from the privacy layer's identifier channel -- never
        /// the clear program name.
        Program(u64),
    }

    /// Pool policy resolved at config time; invalid values repair to
    /// `Context` with one `config/invalid` diagnostic.
    pub enum SharePolicy { Context, Program }
    ```

    池语义：`per-program` 档下 Context 由池按 `Program(u64)` 复用；最后一个 IC 释放时 Context 归还池并延迟销毁（对齐 fcitx5-rime 的 weak 语义——本项目用引用计数或宿主生命周期回调）；焦点切换不清空组合（现行为保持：`session/commit-on-focus-out` 既有语义优先于共享档）。
  - **边界契约**：默认档 `Context`（现行为，`ASM-A-07` 兼容承诺）；`UiCommand`/`UiEvent` 零变更（共享的是宿主侧会话对象，UI 帧仍按 IC 投递）；`session id 分域`（2^32 步长）语义在 per-program 档下由池保证一 program 一 session。
  - **交互与工艺级规范**：无自绘窗改动；行为可见性 = 同程序窗口间模式位保持一致。
  - **与现有代码的无缝衔接方案**：`context.rs` 的 `Context` 查找入口从「per-IC map」改为「策略分派」；`addon.rs` 的 config 步把 `SharePolicy` 注入；程序名→hash 复用 `privacy_impl` 既有管道（该层是唯一见明文名的层，纪律不变）。
- **底层与非功能约束 (NFR)**：
  - 查找开销：池查找 O(1)（HashMap by key），宿主线程无锁竞争窗口（Context 访问本就单线程于宿主循环——ASM-10/11 纪律不变）。
  - 隔离底线：per-program 档下两个程序的会话绝不串线（hash 碰撞概率 2^-64，注释声明；碰撞时按 key 二次比较 fallback——测试注入碰撞用例验证 fallback）。
  - 边缘条件：策略切换（配置重载）→ 在途会话按现快照语义收尾后重建（对齐 fcitx5-rime 策略切换的快照/重建，K-5）；密码框等敏感上下文的抑制学习与共享无关（privacy fail-closed 在会话层之下）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` `SharePolicy` 配置键 + 投影 + 坏值修复测试。
  2. `[步骤 2]` 池分派改造（默认档走原路径的等价重构，先保证零行为差异）+ per-program 复用/回收。
  3. `[步骤 3]` 行为测试：同程序双窗口模式保持、跨程序隔离、策略热切换快照、敏感上下文抑制不回退；`testd` 沙盒双 IC 场景（若 testd 支持多 IC 注入，否则 testd-engine 模拟层断言）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`cargo nextest run -p ime-fcitx5` 全绿（含碰撞 fallback、策略切换快照）。
  - [ ] 走查：X11 档真机会话——同程序两窗口切换后中英模式保持（人工走查 + 探针样本）。
  - [ ] 回归走查：默认 `Context` 档行为与主线逐项一致（`keymap_matrix` 2,568 行路由矩阵全绿）。

---

#### TASK-2.06.01 数据驱动解码冒烟基线（TSV key_sequence→expected_text）

> **纠偏（2026-10-20）**：阶段零复核发现本卡交付物**已随 `74afea9` 落地**——`xtask/src/testd/smoke.rs`（TSV 驱动器，@first/@contains/@regex 三形态）+ `tests/smoke/cases.tsv`（211 例，core 带从留出集派生）+ `justfile` 的 `smoke:` 档（`just ci` 已含）。本卡转 `[x]`；后续对冒烟基线的扩充随各功能卡（1.01.02/1.03.02/2.04.02）的 DoD 一并交付，不再单列。

- **基本属性**：
  - 绑定差距与改进项：`GAP-10` / `IMP-19` ｜ 关联设计假设：`ASM-A-02`
  - 优先级与复杂度：`P1 效率进阶` ｜ 中 ｜ 预估工时： 2 人天
  - 前置依赖：`无`
  - 关键路径：`否`
  - 并行通道：`Track C Infra·DevOps`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `others/script/smoke/` TSV 数据驱动冒烟（[rime-ice.md](../compare/rime-ice.md) §8）
  - 本项目代码落地锚点 (Code Anchor)：`xtask/src/testd/smoke.rs`、`tests/smoke/cases.tsv`（211 行）、`justfile` `smoke:` 档（:322-342）
  - 引入依赖白名单：`无`
  - 当前状态：`[x] 已完成`
- 落地证据：xtask/src/testd/smoke.rs (TSV 驱动器: parse_cases/expectation(@first|@contains|@regex|literal)/evaluate/run_cases/mount(FstLexicon::load+DictLm::new+FreshUser)/derive(留出集派生 core 带, 手写带保留), 11 单测) + main.rs TestdSmoke 子命令 + tests/smoke/cases.tsv (200 例: 30 手写 ambiguity/edge + 170 core 派生) + tests/smoke/README.md 指南 + justfile smoke 可选档(ci 链尾, 缺词典报跳过); regex 依赖入 workspace/contracts §6; 实测: 200/200 绿 1.7s(≤2s), 破坏用例→红且 FAIL 行可定位, cargo nextest run -p xtask 1214 绿
  - 验收记录：驱动器 + 211 例 TSV + `just ci` 的 `smoke` 层均已入线（`74afea9`，`RSPINYIN_REQUIRE_SMOKE_DICT=1` 时缺词典即红）；本记录由 2026-10-20 规划期复核补登。
- **目标与价值**：分层（1.01）、衰减（1.02）、bigram（1.03）、词源（2.04）都在改排序——没有「期望文本」基线，每次改动只能靠 top1 百分比间接感知。rime-ice 的 TSV 冒烟把「这批键必须出这批字」变成一行一个用例的可读资产；`xtask testd-engine` 纯内存重放基建已在，缺的只是驱动器与首批用例集（目标 ≥200 例）。
- **技术设计与代码级接入细节**：
  - **底层模型与数据流改动**：用例格式（对齐 rime-ice 心智模型、贴本项目词库纪律）：

    ```text
    # tests/smoke/cases.tsv -- one case per line, TAB separated.
    # case_id  key_sequence  expected_text  [tags]
    # `key_sequence` uses raw keys; `'` forces a syllable boundary.
    # `expected_text` may be:
    #   literal text            -- exact match of the first commit
    #   @first:<text>           -- text must be the top-1 candidate
    #   @contains:<text>        -- text appears in the top-9 page
    #   @regex:<pattern>        -- for date/variable commits
    base_001  nihao           @first:你好     core
    seg_014   xian            @contains:西安  ambiguity
    ```

    驱动器：

    ```rust
    /// Runs every TSV case through the in-memory replay (`testd-engine`):
    /// keys -> decode -> first commit, compared against the expectation
    /// column. Pure, deterministic, no display server, no dictionary file on
    /// disk beyond the fixture the run mounts.
    pub fn run(cases: &Path, dict: &Path) -> Result<SmokeReport>;
    ```

  - **边界契约**：无运行时契约变更；`SmokeReport` 输出为 CLI 豁免区；用例文件按用户数据纪律对待（内容是词库派生物，非隐私）。
  - **交互与工艺级规范**：Headless——失败输出 `case_id + key_sequence + got vs expected`（不打印整页候选，除非 `--verbose`）；退出码非零即门禁红。
  - **与现有代码的无缝衔接方案**：驱动器是 `testd-engine` 的薄壳（重放入口与 fixtures 装载复用既有代码）；`xtask testd-suite` 的证据包目录约定沿用（`results/runs/`）。
- **底层与非功能约束 (NFR)**：
  - 200 例全跑 ≤ 2s（纯内存重放；允许与 `cargo nextest` 并存的独立命令，进 `just ci` 的可选档）。
  - 用例分带（core/ambiguity/user/fuzzy/abbrev/bigram 占位 tag），后续质量卡按 tag 引用子集做回归。
  - 边缘条件：空词库/损坏词库 fixture → 驱动器报错退出而非 panic（`budget --alloc` 纪律不涉）。
- **逐步落地实施步骤**：
  1. `[步骤 1]` TSV 解析器 + 驱动器 + 三种断言形态 + 单测。
  2. `[步骤 2]` 首批 200 用例（从留出集 `lm_holdout.tsv` 派生 core 带 + 手写 ambiguity/边界 30 例）。
  3. `[步骤 3]` 接入 `just ci` 可选档 + 文档（用例编写指南进 `tests/smoke/README`）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：`xtask testd-smoke`（或子命令）200 例全绿；驱动器单测全绿。
  - [ ] 走查：人为破坏一个用例（改 expected）→ 门禁红且输出可定位。
  - [ ] 回归走查：`testd-suite`/`soak` 既有通道不受影响。

---

## Phase 2 汇总

| 任务 | GAP/IMP | 通道 | 工时 | 关键路径 |
|---|---|---|---|---|
| TASK-2.01.01 模糊音接线 | GAP-05/IMP-06 | A | 1.5 | 否 |
| TASK-2.02.01 prefix 实装 | GAP-06/IMP-07 | A | 2.5 | 否 |
| TASK-2.02.02 简拼接线 | GAP-06/IMP-07 | A | 1.5 | 否 |
| TASK-2.03.01 简繁 commit 接线 | GAP-07/IMP-08 | A | 2.5 | 否 |
| TASK-2.03.02 标点/全宽默认值投影 | GAP-07/IMP-08 | B | 0.5 | 否 |
| TASK-2.04.01 词源接入 | GAP-08/IMP-15 | C | 2 | 是（CP-2） |
| TASK-2.04.02 两字层换血+多音字校验 | GAP-08/IMP-15 | C | 2.5 | 是（CP-2） |
| TASK-2.05.01 会话共享粒度 | GAP-09/IMP-12 | B | 3 | 否 |
| TASK-2.06.01 冒烟基线 | GAP-10/IMP-19 | C | 2 | 否 |

**波次建议**：W1 = {2.01.01, 2.02.01, 2.03.01, 2.04.01, 2.06.01}（文件不相交）；W2 = {2.02.02, 2.03.02*, 2.04.02, 2.05.01}（*2.03.02 与 2.03.01 同文件 `modes.rs`，排 W2 串行）。Phase 2 与 Phase 1 可交叠推进：Phase 2 全部任务不与 Phase 1 任务共文件（唯 `2.04.02 ← 1.01.01` 逻辑依赖先行）。
