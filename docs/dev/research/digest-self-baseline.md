# 当前项目能力与短板基线摘要（dev-features-add 基线勘察）

> 勘察范围: 本仓库 main@0ef7d8b（stage 22，features-add/features-test 增量已全落地）｜ 方法: 纯静态源码精读（只读工具），未运行任何 cargo 命令 ｜ 测试计数方法: `grep -rn '#\[test\]' crates/ xtask/src/` 属性计数 = 4000
> 独立性声明: 本轮未读 docs/dev/compare*.md、compare-and-improve.md、adr/ 正文；features-add/test-phase*.md 仅读状态行与验收记录。

## 1. Crate 拓扑（9 crate + xtask）

依赖单向链 `ime-types ← ime-core ← ime-dict ← ime-config ← ime-ui ← ime-fcitx5`（AGENTS §4.4，`scripts/check-deps.sh` 强制）。

| crate | 职责 | 关键文件 |
|---|---|---|
| ime-types | 冻结契约：DecodeRequest/Result、DecodeFlags、Lexicon/UserFreqSource/LanguageModel/SurfaceBackend trait、UiCommand/UiEvent、KeyAction | `crates/ime-types/src/lib.rs:1-11`（冻结声明）、`decode.rs`、`lexicon.rs`、`ui.rs` |
| ime-core | 纯函数解码引擎：segment→lattice→k-best Viterbi→preedit；无文件/时钟/全局态 | `crates/ime-core/src/lib.rs:1-7`、`segment/dag.rs`、`viterbi/decoder.rs`、`state/mod.rs` |
| ime-dict | FST+mmap 容器（v2 六段）、用户库（redb）、热替换槽、隔离区 | `crates/ime-dict/src/fst_index.rs:1-18`、`format/mod.rs:162-178`、`user_db.rs` |
| ime-config | 配置 schema/迁移/热重载/回写/键表投影 | `crates/ime-config/src/schema/config.rs:12-252`、`reload/`、`keymap.rs` |
| ime-ui | 自定义 Slint Platform + 软光栅 + 弹簧动画 + 跨线程通道；pub API 禁导出 Slint 类型 | `crates/ime-ui/src/lib.rs:1-10`、`renderer.rs`、`channel.rs`、`spring.rs` |
| ime-ui-addon | 第二个 cdylib（Category=UI）：读 InputPanel→光标定位→自绘候选窗 | `crates/ime-ui-addon/src/lib.rs:1-15`、`ffi/transport.rs:1-14`、`cursor.rs:1-14` |
| ime-fcitx5 | 引擎 addon（第一个 cdylib）：按键路由/仲裁、会话宿主、C ABI | `crates/ime-fcitx5/src/lib.rs:1-15`、`engine.rs`、`addon/session.rs` |
| ime-diag | 唯一 subscriber 安装点：脱敏层、崩溃记录、UiFrame 镜像编解码 | `crates/ime-diag/src/redact.rs`（777 行）、`crash/record.rs`、`uiframe.rs` |
| alloc-count | 测试专用分配计数 `#[global_allocator]`（发布闭包禁依赖） | `crates/alloc-count/lib.rs`（单文件）；`Cargo.toml:26-29` |
| xtask | dictc 编译器/tune/quality/budget/testd 测试平台/soak/package/release/install/verify | `xtask/src/main.rs:34`（Command 枚举）、`dictc/`、`testd/` |

## 2. 解码引擎现状（crates/ime-core）

管线：`SyllableDag::build_for` 切分（含强制 `'` 边界与 dropped_chars 诊断，`segment/dag.rs:138-159`）→ `build_lattice[_into]` 读词典建词格（`viterbi/lattice.rs:138,160`）→ k-best Viterbi（beam 默认 16/上限 32，候选上限 64，`viterbi/decoder.rs:24-30`；TopK 槽 `viterbi/kbest.rs:49-126`）→ Q8.8 定点打分（f32 不参与排序决策）。降级答案：无任何切分时返回带 `degraded` 的透传候选（`viterbi/decoder.rs:13-17`）。`Decoder::decode_into` 复用 `DecodeScratch` 缓冲做稳态零分配（`viterbi/decoder.rs:9-11`）。全部 `#[test]` 走内存 mock，无真实词典依赖。

stage 22 增量逐项核实（均存在）：

- **词库分层归一化**：`xtask/src/dictc/normalize.rs:1-30`——原始词频按 (字数,权重) 映射为三频带，banded 权重只进 UNIGRAM 通道，WORDLIST 保持原始序；配置即编译参数 `tail_flat_weight`/`core_min_freq`（`xtask/src/dictc.rs:432-433`）。
- **用户词频衰减**：`crates/ime-dict/src/user_db/decay.rs:16-26`——`DecayParams` 半衰期 139 commits（librime 200·ln2 等价），`fold`(:64)/`effective_freq`(:93) 全整数饱和运算；存储 f32 bits Q16.16（`user_db.rs:19,216`）。
- **BIGRAM 段 + DictLm（容器 v2）**：`format/mod.rs:172`（`Bigram=5` 段，v1 恒空）；`fst_index/dict_lm.rs:26-30`——v2 容器携带排序 BIGRAM，命中与 unigram 同 Q12→Q8.8 表映射，miss 回退 unigram+`BIGRAM_MISS_PENALTY`（`dict_lm.rs:33,50`，与内存模型 `lm/ngram.rs:24-27` 逐字节一致）；离线统计 `xtask/src/dictc/bigram.rs:1-24`（自有词典 MFM 分词 + count·PMI 截断，`dictc.rs:308-309` 传参）。已编译产物 format_version=2（`data/compiled/base.dict.manifest.json`）。
- **置顶词**：`KeyAction::PinHighlighted`（`ime-types/src/key.rs:71`）→ 路由 `engine/router.rs:311` `toggle_pin` → `UserDb::set_pinned`（`user_db.rs:382`）→ `effects.rs:63-106` `pin_applied` 回执触发重排。
- **模糊音**：`fuzzy.rs:1-5` 八混淆类（zh/z, ch/c, sh/s, n/l, an/ang, en/eng, in/ing, f/h）；配置 `[fuzzy]` 每类独立开关（`schema/config.rs:84-102`），`class_flags()` 投影为 DecodeFlags 掩码（:115），默认全关（:136）。
- **简拼**：`segment/abbrev.rs:1-5`（`nh`→`ni'hao` 全前缀组合代数 + 罚分）；配置键 `engine.abbrev`（`schema/config.rs:36`，默认关）。
- **简繁**：`script.rs:1-5` 提交时最长匹配改写（VariantSource 表，词典侧实现于 `ime-dict/src/script.rs`）；`KeyAction::ToggleScript`（`key.rs:66`）→ 状态机 `state/transitions.rs:166,215`。
- **ENGLISH 混输**：`segment/english.rs:1-9`——仅当 exact/fuzzy/correction 三路全部无完整切分才产英文边（价格正偏移保证拼音优先）；配置 `engine.mixed_english`（`schema/config.rs:73`，默认关）；候选来源标签英文（`ime-types/src/ui.rs:242`）。已登记限制：v1 只覆盖整 span 英文候选，`X光` 类跨中英词条留待后续（`docs/dev/features-add-phase3.md:323`）。
- **符号**：`symbols.rs:1-5` v 前缀符号表（v1 数字/vx 箭头/vym 注音），宿主装载 `addon/session.rs:334-368`。
- **拼写纠错**：`DecodeFlags::CORRECTION = 1<<14`（`ime-types/src/decode.rs:92`）；`segment/correct.rs:1-27` 键盘邻位替换/换位一步纠错 + 罚分，结构上门禁保证正确输入与开关闭关逐字节同序（:24-27，DAG 无完整路径才调 `SyllableDag::build_for` `dag.rs:159`）；配置 `engine.correction`（`schema/config.rs:52`，默认关）。
- **上文重排**：`viterbi/context.rs:1-8,56`——提交后仅记住上一词，近平局带 `CONTEXT_TIE_BAND_Q16 = 1<<8`（Q16.16）内允许 bigram 连续性换位；调用点 `state/machine/rebuild.rs:113`。
- **词库热替换**：`ime-dict/src/hot_swap.rs:1-14`——`LexiconSlot` 单原子指针整束换（mapping+index+model 同生命周期）；进程槽 `addon/session.rs:104`（OnceLock），逐步重读。
- **造词**：commit 时 coin 出词对（`engine/router.rs:153-156,287-305`）→ `user_lexicon.rs:134,530` 内存表 + store 批量写；用户短语文档 `AddPhrase`（`key.rs:73`）与短语表 `phrase.rs:1-14,470`（inject_phrase_candidates 排在解码结果之前）。
- **compose/死键**：引擎侧无自有 compose 表，宿主经 trait 询问死键状态（`engine/host.rs:51-79`，`processComposeString` 归 Fcitx5）；配置键 `engine.compose`（`schema/config.rs:63`）。

会话机：`SessionConfig` 持 max_raw_len/scheme/fuzzy 掩码/abbrev/correction/english 等投影（`state/mod.rs:83-150`），重载不扰动进行中组合（scheme 于组合开始时锁定，`state/mod.rs:101-108`）。

## 3. 宿主桥与 UI addon 现状

- **按键总线/仲裁**：`translate_key` 纯表翻译（`engine.rs:256`）→ `arbitrate` 双条件（表有名 + 会话可执行）决定是否扣键（`engine/arbiter.rs:1-16`）；层叠上下文 `engine/router/`（config/context/effects/lifecycle/modes/phrases 子模块）。序列和弦：`engine/sequence.rs:1-17`（`Ctrl+K` 型 trie，Opened/Completed/Pass 三态）。
- **compose 处理**：见上节；仲裁镜像 step（`arbiter.rs:54,181`）。
- **会话宿主**：`session_host.rs:1-7` 进程级会话槽 + unload sweep；`KeyRouter` 每 InputContext 一会话（`session_host.rs` / `engine.rs:30-33`）。
- **状态区 Action**：`KeyAction` 全集（`key.rs:24-73`：翻页/高亮/ToggleLang/FullWidth/Punct/Script/TempEnglish/Forget/Pin/AddPhrase…）；模式徽标 `engine/badge.rs`，状态条右槽在帧解析时重写（`router/effects.rs:15,294`）；速查面板由实时键表生成（`cheatsheet.rs:1-10,142`，OverlayStage :80）。
- **跨 addon 帧通道（ADR-0011 wire）**：引擎侧写 `ffi/abi/engine/transport.rs`（795 行），UI 侧 `ffi/transport.rs:1-14` 逐字段镜像、双方静态断言尺寸防漂移；握手 `rspinyin_engine_register_ui_sinks`（RTLD_DEFAULT→NOLOAD 探测序，`ffi/transport.rs:16-22`）。两 cdylib 无共享静态态、无自有 IPC，面板数据经 Fcitx5 InputPanel 走（`ime-ui-addon/src/lib.rs:11-15`）。
- **帧镜像（测试通道）**：`mirror.rs:1-14`——仅 `test-mirror` feature，环境变量 `RSPINYIN_TEST_MIRROR_DIR` 显式武装且必须在 `$XDG_RUNTIME_DIR` 下（:26-34），格式权威在 `ime_diag::uiframe`。
- **光标定位**：`cursor.rs:6-14` 三级源梯（前端矩形→客户端+窗口原点→屏幕 60% 兜底）。
- **Slint 光栅管线**：`renderer.rs:1-10`——UI 线程 `poll(2)` 后唯一调用 `render_if_dirty`，双缓冲 + damage 记账保 idle 零合成器流量（BUDGET-CPU-01）；Slint 1.13 软件光栅 + systemfonts（`Cargo.toml:56-62`），dev profile 对 i-slint-core/fontdue/rustybuzz 单独 opt-level=2（`Cargo.toml:109-127`）。
- **焦点安全**：Wayland `keyboard_interactivity = NONE` 硬编码（`platform/wayland/layer_shell.rs:24,95,183`）；X11 后端无 `XSetInputFocus` 调用（`platform/x11/` 全目录 grep 无命中）。

## 4. 数据层现状（ime-dict + dictc）

- **容器布局**：六段定序 Fst/Entries/StrPool/Unigram/Bigram/WordList（`format/mod.rs:162-178`）；段表+文件级 CRC32（`Cargo.toml:41` crc32fast）；读写全偏移边界检查，越界报 `DictError::LengthOutOfRange`（`fst_index.rs:14-18`）。读路径一次 FST 查 + 一次 WORDLIST 读，编译期降权写入即排序、零排序（`fst_index.rs:8-12`）。
- **损坏隔离**：`recover.rs:51-61` CRC 失败改名 `*.corrupt.*` 隔离（`recover/quarantine.rs`），降级只读重新开始学。
- **用户库**：redb，批量写 `user_db/flush.rs`（慢盘自适应放宽窗口，`flush.rs:5,51`）；v1 库降级只读（`open.rs:77-118` AtomicBool）；TSV 导出/导入双向 8MiB 上限（`export.rs:1-10`）；备份 `user_db/backup.rs`（772 行）；逐出 `evict.rs`。
- **dictc 管线**（`xtask/src/dictc/`）：source 白名单解析 → normalize 三频带 → build（L3b 多音字展开/频带/六段组装，`build.rs:1-9`）→ bigram 邻接统计 → quality 留出集 top1 评估与 L3c 纠错表审计（`quality.rs:1-9`）；辅以 truncation/manifest/eng/stage。确定性：双编译 sha256 一致为门禁（`normalize.rs:28-30`）。
- **词源状态**（`data/sources.toml`）：12 个登记源。哈希钉扎上游 5：pinyin-data/unihan/jieba-dict（2026-09-29）、thuocl/huayu（2026-10-19，ADR-0012 接纳，:29-31 注明腾讯词向量许可存疑排除、ADR-0012 W-4）；derived 免钉 7：base/polyphone/phrase/melt-eng/confusable/symbols（`retrieved=""` 即未完态不可过 CI，:22-24 规则）。产物：base.dict 17,600,784B + eng.dict 129,976B（`data/compiled/`，2026-10-20/21）。

## 5. 测试平台现状（xtask/src/testd/）

- **子命令**（`xtask/src/main.rs:34` Command 枚举，21 变体）：dict{eng,stage}/dictc/quality/tune/budget/check-versions/report/soak/install/package/verify/release/testd/testd-client/testd-engine/testd-suite/testd-smoke/test-mirror/capture/golden。
- **testd 通道**：XTEST 真键鼠注入 + FocusGuard 前后验焦（`testd/mod.rs:1-17`）；frame mirror（`mirror_cli`）与截图（`capture_cli`）分立；三大件 `engine`（无 fcitx5 回放）、`sandbox`、`suite`。
- **套件分层**：`suite.rs:64-81` 三档——`--tier smoke`（默认，CI 可跑）、acceptance（先跑 smoke 作门禁再串行）、stress（soak/内存/规模卡，须空闲机，`main.rs` just `check-e2e-stress`）。
- **冒烟基线**：`tests/smoke/cases.tsv` 211 行 = 202 例（170 core 派生自 holdout + 19 edge + 11 ambiguity），每例 `@first:` 锚首选（`cases.tsv:1-9,42`）；`smoke.rs:77-96` `--derive-from-holdout` 重导出 core 带。
- **测试总量**：`grep -rn '#\[test\]' crates/ xtask/src/` = 4000 属性（含单测/集成/benches 内测试；方法标注如左）。xtask 子树 nextest 实测 1225/1225 绿（`docs/dev/features-test-phase1.md:21` 验收记录）。根 `tests/` 仅 fixtures+smoke。
- **justfile 菜谱**：`check:34`、`check-host:62`、`test:114`（nextest 分两段 + doc 全量）、`audits:306`（12 项审计合集）、`smoke:328`（缺 base.dict 时可跳/可强制）、`check-e2e-acceptance:351`、`check-e2e-stress:357`、`ci:360 = check+audits+check-host+smoke`、`ci-host:364`（RSPINYIN_REQUIRE_HOST_ABI=1）、arm64 交叉门禁 :370 起。
- **证据契约**：`results/runs/` 8 个 run 目录 + `index.json`；批次级 `checklist.json` format=3（用例×状态×证据布尔，`run-20261021-104600/checklist.json:1-5`）+ `index.md`；账本 v3 协议 `xtask/src/testd/evidence/v3.rs`（AssertionV3/Claim/Tier/Dim，`features-test-phase1.md:21`）。

## 6. 预算现状（docs/dev/budgets.json，键名原样）

- `latency_ms`：key_to_present_p50=4.0, key_to_present_p99=16.0, key_to_present_p99_144hz=12.0, decode_p99=24.0, decode_p999=64.0, raster_p99=1.5, first_key_to_visible_p99=8.0, addon_load=120.0
- `memory_mb`：ui_rss=18.0, plugin_rss=45.0, dict_mmap_rss=25.0
- `cpu_pct`：idle=0.3, typing_10cps=6.0, idle_redraw_count=0, idle_poll_timer_count=0
- `size_mb`：so_stripped=12.0, base_dict=20.0（现值 16.8MiB，占用 84%）
- `alloc_count`：decode_steady=13
- `robustness`：soak_hours=8.0, rss_drift_mb=2.0, pass_rate_pct=100.0
- `bench`：decode_user_decay_cold_us=100.0, passthrough_classify_ns=500.0, input_buffer_ops_us=1.0, decode_holdout_s=3.0, ui_wakeup_latency_us=512.0, post_ui_ns=100.0
- `net_sockets`：0（`xtask budget --validate` 交叉校验 0.5.3 表与 criterion 断言）

## 7. 已知偏差与 tech-debt

- **代码 TODO**：`grep -rn "TODO" crates/ xtask/src/` = **0 命中**——无未闭合 TODO 债务。
- **router.rs 拆分欠账**：`crates/ime-fcitx5/src/router.rs` 已不存在；现为 `crates/ime-fcitx5/src/engine/router.rs`（749 行）+ 6 个职责子模块（router/{config,context,effects,lifecycle,modes,phrases}）——**历史拆分欠账已清**，但 749/800 逼近上限。
- **行限现状**：非测试文件最大恰 800 行三件（`ime-ui/src/theme.rs`、`spring.rs`、`adapter.rs`，均贴顶）；测试模块 ≤1200 内（最大 `state/tests/transitions.rs` 1194、`addon/tests/steps.rs` 1189、`tests/keymap_matrix/main.rs` 1128）。
- **卡面登记的已知限制**：中英混输 v1 不覆盖跨中英混合词条（`features-add-phase3.md:323`）；基准/验收证据多处标注「须空闲机采集」，负载机上 bench 判红属测量纯净度前提（`features-test-phase1.md:21` TC-CORE-20 记录）。
- **验收记录回写**：features-add 28 卡 + features-test 8 卡全部 `当前状态/任务状态：[x] 已完成`（phase1:9+phase2:9+phase3:10；features-test 4+2+2），与 memory 台账 36 卡一致。

## 8. git log --oneline -15

```text
0ef7d8b feat: stage 22 lands the features-add and features-test increments
be32d19 stage 21
8181199 stage 20
cbdbab7 stage 19
b4417e3 stage 17
eb4a197 stage 16
74afea9 finish stage
d8e1883 stage 9
cadc1ab docs: sync the test suite with the landed codebase and audit it
408d2c6 docs: record the post-fix retest of the candidate-window blocker
56acbc4 fix: name the UI addon correctly and check the X11 window creation
534e48a fix: heal four defects the full-suite round found in product code
fc85793 docs: close the opt-basic round with acceptance records on all 31 cards
cbc16e4 feat: land the config, docs honesty, reload and token-hygiene waves
fc606a5 feat: take the window down on host suspend and make the log policy hot
```

## 现状短板与待改进候选清单

- **B-01 排序质量上限**：top1 由 `dictc quality` 留出集度量（`xtask/src/dictc/quality.rs:1-9`），冒烟 core 带仅钉当前序（`tests/smoke/cases.tsv:42`）；长尾扁平化在跨长竞争通道已收益半点 top1（`normalize.rs:20-24`），说明序仍有可挖掘空间——缺 third-bigram/三元与更细频带。
- **B-02 BIGRAM 覆盖受截断**：`max_pairs`/`min_score` 硬截（`dictc/bigram.rs:18-24`），且用自有词典 MFM 分词生成，语料外搭配全 miss 回退（`dict_lm.rs:26-30`）；上文重排仅一词且限定近平局带 1<<8（`viterbi/context.rs:56,69`），跨句/多词上文未建模。
- **B-03 L4 频率单源**：频带原料仅 thuocl 一个上游（`data/sources.toml:111-118`），归一化阈值为手工常数（`dictc.rs:432-433`），无数据驱动的阈值整定。
- **B-04 base.dict 体积余量小**：17.6MiB / 预算 20MiB（`data/compiled/` vs `budgets.json size_mb.base_dict`），继续扩词源将顶格，需先做裁剪或分段加载。
- **B-05 符号表进程级一次性加载**：`addon/session.rs:334-368` `OnceLock` 装载后不随配置重载/文件更换重读（短语表是 Arc 可换，符号表不是）。
- **B-06 行限贴顶文件**：theme/spring/adapter 恰 800 行、router.rs 749 行（§7），任何增改立刻触发强制拆分，宜预先拆。
- **B-07 扩展输入能力缺失**：无 emoji 表情面板、无笔画辅助、无快捷计算（grep emoji/calc/stroke 于 crates/ 无命中）；v 模式仅静态符号类别（`symbols.rs:1-5`）。
- **B-08 E2E 依赖真实 X server**：注入/截图/golden 全走 XTEST+X11（`testd/mod.rs:3-9`），headless 只覆盖 testd-engine decode 半边，渲染回归在无显示环境不可跑。
- **B-09 冒烟基线只锚首选**：202 例全部 `@first:`（`cases.tsv:3`），top3/top5 命中、页序稳定性与纠错/模糊开档行为无回归 pin。
- **B-10 高级能力默认全关且无实测键感数据**：abbrev/correction/mixed_english/fuzzy 出厂即 off（`schema/config.rs:36,52,73,136`），行为正确性有单测，但开档后的真实输入体验无遥测（离线产品也无采集通道）。
- **B-11 双拼仅内置 6 方案**：XIAOHE/ZIRANMA/MICROSOFT/SOGOU/ZIGUANG/FULL（`decode.rs:149-161`），无用户自定义布局定义（`shuangpin/` 目录均为硬编码表）。
- **B-12 compose 依赖宿主**：死键表完全委托 Fcitx5 `processComposeString`（`engine/host.rs:51-62`），宿主不提供时该能力静默退化（:58-60），插件自身无兜底 compose 表。
- **B-13 用户词典运维无 CLI**：导出/导入/备份在库层完备（`user_db/export.rs:1-10`、`backup.rs`），但 xtask 21 个子命令无 user-dict 管理入口（`main.rs:34` Command 枚举），运维动作无文档化路径。
- **B-14 预算采集可重复性弱**：bench 阈值判红依赖空闲机前提（`features-test-phase1.md:21`），共享/负载机上 `just bench` 误报，无环境纯净度自检门禁内建于采集路径。
- **B-15 词源扩展天花板**：腾讯词向量因许可存疑被排除（`data/sources.toml:29-31`，ADR-0012 W-4），L5 域词层仅 phrase/confusable/symbols/melt-eng 四个自产 derived 表，缺大颗粒领域词源补充路径。
