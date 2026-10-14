# rspinyin 工作区现状盘点报告

> 来源: 2026-10-14 四路并行调研之子代理原始报告,全文存档未删改 ｜ 调研对象: 本仓库 `crates/`、`xtask/`、`docs/dev/features.md`(remarks/ 下外部参考仓库不在范围内) ｜ 交叉对比结论见 [rime-comparison.md](rime-comparison.md) ｜ 全部行数为实数统计(`#[test]` 内联测试单独给出"非测试文件"口径)

盘点范围:`crates/`(9 个)、`xtask/`、`docs/dev/features.md`;`remarks/` 下三个外部参考仓库不在盘点范围内。全部行数为实数统计(`#[test]` 内联测试单独给出"非测试文件"口径)。

---

## 1. Crate 拓扑

| crate | 职责 | .rs 总行数 | 非测试文件行数 |
|---|---|---|---|
| `ime-types` | 全局冻结契约层:全部跨边界类型 + 4 个 trait(`Lexicon`/`UserFreqSource`/`LanguageModel`/`SurfaceBackend`)+ 错误模型 + `UiCommand`/`UiEvent`/`UiFrame`。只许依赖 thiserror/bitflags/serde | 2,714 | 2,714 |
| `ime-core` | 解码引擎:切分 DAG、词格、K-best Viterbi、定点打分、会话状态机、双拼、模糊音、简拼代数、直通/标点、preedit、简繁算法、隐私策略。纯函数,无 IO | 25,905 | 16,307 |
| `ime-dict` | mmap 词库容器读写、FST 索引 lexicon、`DictLm`、redb 用户词频库、XDG 路径、损坏自愈/备份 | 14,537 | 9,367 |
| `ime-config` | TOML 配置模型 + 修复 + 迁移(v1→v2)+ 热重载 + 键位白名单投影 + 回写 | 7,447 | 4,337 |
| `ime-diag` | 唯一 tracing subscriber + 字段级脱敏、panic 钩子、SIGBUS/SIGSEGV 崩溃记录、探针(直方图/计数器/内存)、预算看板、测试帧镜像 | 11,908 | 8,442 |
| `ime-ui` | 自绘候选窗:X11/Wayland 平台层、Slint 自定义 Platform、软件光栅、几何、主题、Spring 动效、跨线程通道(SPCS+eventfd) | 40,812 | 26,187 |
| `ime-ui-addon` | UI cdylib(`librspinyin_ui.so`):fcitx5 UI addon 生命周期、UserInterface 接管、frame wire 解析、光标/多屏、测试镜像发布 | 9,566 | 6,964 |
| `ime-fcitx5` | 引擎 cdylib(`librspinyin.so`):addon 生命周期、按键路由、会话执行器、C ABI + C++ 胶水、短语文件、隐私落地 | 30,842 | 15,323 |
| `alloc-count` | 测试专用 `#[global_allocator]`(ADR-0010),任何发行物不得依赖,`scripts/check-unsafe.sh` 强制 | 245 | 245 |
| `xtask` | dictc 编译器、质量预算门、安装/打包/验证/发布、soak、权重调优、**testd 测试平台** | 78,654 | 52,384 |

- 依赖方向(单向 DAG):`ime-types` ← core/dict/config;dict → core(仅用 `segment::syllable_at`);fcitx5 → core+dict+config+diag+types;ui → types(+slint/x11rb/rustix);ui-addon → ui+diag+types。`Cargo.toml:8-45` 有逐依赖中文注释,`unsafe_code = "deny"` 在 workspace lints(FFI/mmap 局部豁免)。
- 全仓 **3,701 个 `#[test]`**(与 `docs/dev/tests.md` 第 0 节登记的基线一致),criterion 基准 9 个。
- 版本 0.1.0,edition 2024,MSRV 1.85;release profile 注释明确禁 strip(保 `fcitx_addon_factory_instance` 导出)与禁 `panic="abort"`(保 FFI catch_unwind)。

## 2. 解码引擎 ime-core

**音节切分 —— 是 DAG,不是 greedy**。`segment/dag.rs:80-157` `SyllableDag`:节点=字节偏移,每节点边表 SmallVec 内联 8(`EDGE_INLINE`),`reachable_to_end` 反向可达剪枝,`'` 为强制边界(`EdgeKind::Forced`),规范化(v/u→ü、丢字符上报 `DroppedChars`),上限 `MAX_RAW_LEN=64`。音节表 411 条(`segment/syllable.rs`)。**切分歧义被完整保留**:`viterbi/lattice.rs:14-24` 明确"不是一次切分,而是按图走 span"(`xian` 同时是 `xian` 与 `xi'an` 两种 span 各查一次典),排序交给 Viterbi。

**候选生成流程**:`SyllableDag::build` → `build_lattice`(`viterbi/lattice.rs`,每 span 拼 `'` 连接键查 `Lexicon::lookup`,字典无词的单音节 span 上单字 fallback,`MAX_WORD_SYLLABLES`/每节点查询数/`MAX_TOTAL_EDGES` 四轴限界)→ `sweep.rs` 按字节序扫格 → `kbest.rs` 有界最小堆 `TopK` 保留每节点前 K 路径 → 读回末节点路径为候选。简拼走 `lattice/abbrev.rs` 的独立枚举 walk(带 `ABBREV_PENALTY_Q8` 惩罚),模糊音走 `fuzzy.rs` 等价类扩边。

**排序打分公式 —— 全整数 Q8.8 定点**(`lm/score.rs`):
- `Q=256`,整数 `log2_q8`(8 段线性插值表 `LOG2_SEGMENTS`,误差 <2/256);`LOG2_FLOOR=-2048`。
- 5 权重 `ScoreWeights`(score.rs:171-179):`uni=256(1.0), bi=154(0.6), len=90(0.35), seg=128(0.5), user=205(0.8)`,离线调优器 `xtask tune` 的起点,负值拒绝。
- 边分(score.rs:250-257):`uni*unigram + bi*bigram(prev,word) + len*(char_count-1)*256 + user*min(log2(1+freq)/8, 2)*256 + 每分段 seg 罚`;路径分 Q16.16。
- 确定性是硬契约:sweep 用单调 generation 计数器决胜、无 HashMap、100 次运行字节指纹一致(`viterbi/mod.rs:212-227` 测试)。

**k-best / 成句**:beam 默认 16(`DEFAULT_BEAM_K`,上限 32),候选默认 45 = 5 页 × 9(`DEFAULT_MAX_CANDIDATES`,恰等于 `MAX_REACHABLE_CANDIDATES`,mod.rs:309 有测试钉死)。**整句能力有**:首选即整句最优路径(`test_decode_ranks_the_whole_sentence_first`,mod.rs:193),并附最优路径首词作为第二候选(继承父分,mod.rs:376-393);同文本候选合并、分数相加、来源标签并成 `UserDict`(mod.rs:323-373)。

**三个 trait 的形状与实现状态**(`ime-types/src/lexicon.rs`):
- `Lexicon{lookup, prefix, fallback_single}`:真实现 `ime_dict::fst_index::FstLexicon`。`lookup`/`fallback_single` 实装;**`prefix` 至今返回 `Err(ImeError::Unsupported)`**(`fst_index.rs:401-403`)——即真实词库不支持前缀枚举。
- `UserFreqSource{freq, record, is_user_word, forget, list, export_tsv}`(后三个 ADR-0005 追加、带默认实现):真实现 `ime_dict::user_db::UserDb`,前四个中 **`is_user_word` 硬编码 `false`**(`user_db.rs:694-698`,注释:"v1 只存词频…Phase 2 用户造词")。**用户词频生效**:默认 `DecodeFlags::USER_DICT` 打开,`sweep` 读 `freq` 进打分,`record` 在 commit 后调用(`viterbi/mod.rs:426-440` 有排序翻转测试)。
- `LanguageModel{unigram, bigram} -> i32`:**不是空实现**。生产模型 `ime_dict::fst_index::DictLm` 直接读容器 `UNIGRAM` 段(`(hash,prob_q12)` 升序二分 + 预计算 Q8.8 映射表 16KiB);**bigram 在 v1 退化为 `unigram-64`**(`dict_lm.rs:28-36`,BIGRAM 段保留为空,Phase 3 启用)。`ime_core::lm::InMemoryLm` 是测试/调优 double,生产装配路径不用。

**简拼/模糊音/纠错的现状(关键,易误判)**:
- 模糊音:`fuzzy.rs` 8 类全部实现(`DecodeFlags::FUZZY_*` 8+1 位),但 `ime-config` **没有任何模糊音配置键**,生产会话只设 `SHUANGPIN` 位(`state/machine.rs:454`)——引擎能力在,主机接线无。
- 简拼:`segment/abbrev.rs` 代数 + lattice walk 完整,配置键 `engine.abbrev` 存在但默认 false 且注释 "Not wired yet"(`ime-config/src/reload.rs:125-127`);加之真实词库 `prefix` 未实现(`lattice/abbrev.rs:145` 对 prefix 错误静默跳过)——**生产不可用**,只有 mock lexicon 测试可驱动。
- 双拼:**已完整落地**——`shuangpin/` 五套表(小鹤/自然码/微软/搜狗/紫光)+ `[scheme.custom]` 自定义表 + `keep_full_pinyin` 回退;已知限制:输入缓冲不收 `;`,三张表的 `;` 列当前不可达(`shuangpin/mod.rs:36-39`)。
- 拼写纠错:**完全没有**(TASK-2.02.03 的相邻键位替换/A* 容错无对应文件,无 `viterbi/correct.rs`)。
- 会话状态机 `state/`:`Idle/Composing/Cancelling/Committing` 四态 + 有序 `Effect` 列表(Show 先于 SendFrame、Commit 单次、hide 由执行器补发),`Paging`(9/页×5 页)、revision 防过期帧、`temp_english` 临时英文(Enter/Escape 退出)、AddPhrase/Forget 实装、**PinHighlighted 回 `ImeError::Unsupported`**(`state/transitions.rs:199-205`)。

## 3. 词库 ime-dict 与编译器 xtask/dictc

**mmap 二进制格式 v1**(`ime-dict/src/format/mod.rs:13-48`):
- 布局:64B 头(magic `b"RSPD"`(:60)、version=1、flags bit0=BIGRAM 存在(v1 恒 0)、total_len、file_crc32(覆盖 [64,len),:31)、entry_count、32B 保留)+ 144B 段表(6×24B:kind/crc32/offset/len)+ 各段 8 字节对齐。
- 六段:`FST`(fst::Map,键=音节串,值=payload `(wordlist_start<<24|count)`,`MAX_WORDS_PER_KEY=32`)、`ENTRIES`(16B 定长 `DictEntry`:word_off u32/word_len u16/syl_count u8/flags u8/weight u32/char_count u8/pad[3],`repr(C,align(8))`,`entry.rs` 逐字段解码+每次访问 bounds check)、`STRPOOL`(无长度前缀 UTF-8 连接)、`UNIGRAM`(`(u32 hash,u16 prob_q12,u16 pad)` 按 hash 升序,`PROB_Q12_MAX=4096`)、`BIGRAM`(v1 空,Phase 3 保留)、`WORDLIST`(`[u32 word_id]` 按键分组)。
- **多音字编码 = WORDLIST 间接层**(mod.rs:44-48):一词多键只扩 WORDLIST 的 4 字节对,ENTRIES 每词一条;词 flag `SURNAME/PLACE/TERM/USER`(USER 编译器从不设置,mod.rs:122)。

**dictc**(`xtask/src/dictc.rs`):源 = `data/sources.toml` 白名单(id/spdx/sha256 pin/layer,fetch.sh 下载)+ **jieba-dict.tsv**(词+词频,349,046 行,同时作 L4 频率带)+ pinyin-data.tsv(L1 单字读音)+ polyphone.tsv(L3c 多音字权重校正,2,587 行去重后)。管线 `L1 单字 → L3a 基线键 → L3b 频率定向多键展开(top 50k、cap 4、key 增幅上限 30%)→ L3c 纠音 → 五段 → 原子写`。实测全量:348,972 词条 / 292,315 键 / 15.62MiB、编译 9.3s、双编译 sha256 一致(features.md:2278);`xtask quality` 在 6,000 条留出集:top1 73.8% / top9 96.4%;`dictc/budget.rs` 段份额(per-mille)+ 载荷 17.5MB 双红线。

**查词路径**(`fst_index.rs:14-18`):`key → fst::Map::get → WORDLIST[start..start+count] → ENTRIES[word_id] → STRPOOL → &str`,零拷贝(WordRef 借 mmap,'static 视图靠字段声明序保证 drop 序,:157-183);编译期按 weight 降序写 WORDLIST → 查询即排名不排序;8 词内联 `WordBuf`,溢出转堆不截断。**整码查询**(键即 `'` 连接串)+ 单字 fallback;**无前缀查询**(见上)。

**校验**:加载即全量验证(header+段表+每段 CRC32,`format/reader.rs`);配置 `engine.verify_dict_on_load = full|header`;每访问 bounds check(entry.rs,UTF-8 建期验证/读期信任,威胁模型明确只防意外损坏,:46-52);SIGBUS 由 metadata 长度建映射 + ime-diag 信号 handler 兜底;`recover.rs` 损坏隔离 quarantine + 原子替换,`user_db/backup.rs` 备份/恢复(keep 3 代)。

## 4. fcitx5 桥 ime-fcitx5

- **注册**:描述符 `packaging/fcitx5/rspinyin.conf`(`Category=InputMethod`,`Library=librspinyin`,`OnDemand=False`,`Configurable=True`;硬依赖仅 `core:5.1.0` 地板,xcb/wayland 列 OptionalDependencies——注释解释了为何不能列成硬依赖)。工厂符号 `fcitx_addon_factory_instance` 由 build.rs 用 `cc` 编译 C++ 胶水(`src/ffi/cpp/addon_glue.cpp`)导出、宿主 dlsym 解析;`fcitx5-host` feature 控制真实链接,缺依赖时纯 Rust 门禁照常跑(justfile 头注)。
- **生命周期**:`addon.rs:117-131` 固定 10 步 `INIT_STEPS`(diagnostics[唯一 fatal]→data-dirs→config→key-bindings→config-watch→phrases→store-recovery→lexicon→session-host→transport-probe),除 diagnostics 全部 fail-soft,同步段预算 120ms(`BUDGET-LAT-05`)。
- **keyEvent 流程**:`ffi/abi`(冻结 `#[repr(C)] FcitxKeyEvent` = keysym+state,修饰位与 fcitx5 5.1.7 keysym.h 逐位钉死,`engine.rs:99-133`)→ `translate_key` 双表(`CHORDS` 精确修饰集 + `rows` 优先级行,数据非控制流)→ `context.rs` 分层总线(context/dispatcher/overlay/session)→ `arbiter.rs` 可执行性仲裁 → `KeyRouter::key_event`(`router.rs`):步进 `ime-core` 会话 → 有序 `Effect` 经 `Host` trait 执行(commit、client preedit、`post_ui` 投递命令、诊断)。**认领规则**:有效果或引擎模式切换才 `filterAndAccept`;key release 不认领;音节分隔符 idle 态不认领;临时英文模式全放行;序列按键(chord trie,`sequence.rs`,Open/Complete/Abandoned,放弃时补交所占键)。
- **拦截键**:a-z(大写按 `auto_english_on_uppercase` 直通英文)、1-9 选候选(0 由 `keys.digit_zero` 配置 passthrough/flip)、space、Enter(`enter_commit_raw`)、Backspace、Escape、`'`(仅 composing)、`-`/`=`/方向/Home/End 翻页 + Tab/Shift+Tab 高亮(白名单 `KeyName` 八个,可配 ≤6 个)、全角/标点/临时英文的 Ctrl 和弦、AddPhrase/Forget/Script 切换。
- **preedit/commit**:`Effect::UpdatePreedit/SetClientPreedit/Commit`,提交与清空由执行器回 `CommitDone/PreeditCleared` 事件闭合(router.rs:44-46);每输入上下文一个 `Context{session, modes, anchor, phrases}`,session id 按 2^32 步长分域防串线。
- **与 UI addon 的关系(两个 cdylib 分工)**:`addon.rs:16-21` 声明"这个 addon 解码与上屏,那个画窗口;互不等待"。**通信 = C ABI wire,不是 SPSC+eventfd**(ADR-0011):引擎在宿主线程序列化 `UiCommand` 进 `RspinyinFrameWire` 等 repr(C) 结构调 UI sink(`ffi/abi/engine/transport.rs`);握手反向——UI addon 的 C++ 胶水 dlsym 找引擎 `rspinyin_engine_register_ui_sinks`(探测顺序 RTLD_DEFAULT→NOLOAD 兄弟路径→裸 soname,`ui-addon/src/ffi/transport.rs:19-25`);双端 probe 使加载顺序无关(`addon.rs:127-131`)。**SPSC+eventfd 是 ime-ui 内部"宿主线程→UI 线程"通道**(`channel.rs:31-43` 通道契约表:Frame latest-wins/1、Show/Hide 有序塌缩/8、Select 环 64 丢单元预算、eventfd 计数器合并唤醒,`channel/wakeup.rs`)。
- **配置重载**:宿主 `reloadConfig()` → 导出函数 `rspinyin_config_reload` → `on_config_reload` 重读文档 + 重投影路由表(`[keys]` 与文档同刻替换,`ime-config/reload/store.rs`);**无文件 watcher**(reload.rs:15-22 有意为之);宿主不回调时打 `lifecycle/pending` 行,改动下次重启生效(`addon.rs:147-162`)。

## 5. 自绘 UI ime-ui + ime-ui-addon

- **X11**(`platform/x11.rs`):单连接 + override-redirect ARGB 32 窗口,`WM_HINTS input=False`、永不 `SetInputFocus`/grab(:13-18,焦点零抢占是结构性的);**定位策略 = 普通窗口 + ConfigureWindow 移动**(非 XIM、非嵌入),EWMH `WINDOW_TYPE_DOCK`+ABOVE+SKIP_TASKBAR;帧用 `PutImage`(刻意不用 MIT-SHM——需要 libc/unsafe,workspace 禁),ARG visual 缺失回退 root visual 并报 `platform/x11/no-argb-visual`;SHAPE 扩展整型交互区;`blur.rs` 亚克力协商、`cursor.rs` 指针态。
- **Wayland**(`platform/wayland/`):**四档阶梯** T1 `zwlr_layer_shell_v1`(OVERLAY、TOP|LEFT、margin 定位、`keyboard_interactivity=none`)→ T2 `xdg_popup`(positioner 由合成器摆)→ T3 同 popup 关闭约束自摆(canvas_popup)→ T4 兜底(不注册 UserInterface,ClassicUI 顶上,ASM-13)。**关键事实:本构建未编译 Wayland 层**——`platform/mod.rs` 只 `pub mod x11`,依赖闭包里没有 wayland-client;`ui-addon/src/platform.rs:21-29` 明确 Wayland 会话答 T4 + `platform/compositor/unsupported`。代码与测试齐全但"无法在开发机验证"(WSLg=Weston 在四档之外,mod.rs:56-81 逐条诚实登记了哪些是"实现+推理未验证")。
- **Slint 角色**:`slint_platform.rs` 自定义 `Platform`/`WindowAdapter`——Slint 被驱动而不跑自己的事件循环,UI 线程持 `poll(2)` 循环、唤醒后 `render_if_dirty`;`renderer-software + software-renderer-systemfonts` 软件光栅(无 GPU 依赖,缺 systemfonts 会 panic,Cargo.toml:39-44);OB-4 许可隔离:公共 API 零 Slint 类型(`ThemeSink` trait 反转写入,theme.rs:24-27)。UI 文件 4 个:`candidate.slint`(888 行骨架+header)、`candidate_grid.slint`、`theme.slint`(token 默认值)、`overlay.slint`。
- **布局能力**:**横排网格**——`GridLayout{cols,rows,pages}`,cols ≤ `max_per_row`(3..=9),最多 5 页,超长省略但全文上屏(layout.rs:33-52);**无竖排**。主题:`theme.rs` 深浅色双 palette + 对比度门 + acrylic 降级阶梯(纯色底+双层阴影+描边为默认预期)、`theme.scheme=auto/light/dark`;动效:Spring 高亮滑动/press/appear-disappear(ω₀=26、ζ=0.85 可配,restart 生效)。Overlay 三种 `OverlayKind` CheatSheet/CommandPalette/Diagnostics 中**只有 CheatSheet 实装**(`ime-fcitx5/src/cheatsheet.rs`);CommandPalette 有路由与 wire 编码、无产生方。
- **与 classic-ui 的关系**:`rspinyin-ui.conf` 以 `UIPriority=10` 压过 ClassicUI(0)、`UIType=PhysicalKeyboard`;`ui_impl/takeover.rs` 主动调 `UserInterfaceManager::updateAvailability()` 请求接管,被拒一次后永不再抢(`TAKEOVER_DECLINED`,:42-59),任何失败回退 ClassicUI——输入永远可用,只丢自绘外观。
- **UI 线程**:单线程拥有显示连接/缓冲/Slint platform(`ui_thread.rs`),通道语义三类(命令/事件/数据流)决定溢出行为,`Rendered` 事件回传探针样本;`UiEvent::Select` 带 revision 幂等。

## 6. 配置 ime-config

- 格式:TOML,`$XDG_CONFIG_HOME/rspinyin/config.toml`,`schema_version=2`,`migrate.rs`(749 行)做 v1→v2 迁移且原文件保留;无文件时写出内嵌的带逐键注释模板 `DEFAULT_CONFIG_TOML`(`reload.rs:87-222`,测试保证它与 `Config::default` 不漂移)。
- 用户可调项(按每键注释的三态标注):**reload 生效**——`auto_english_on_uppercase`、`passthrough_url`、`max_raw_len(1..=64)`、`client_preedit`、`max_per_row/show_annotation/max_width_dp`、`digit_zero`、`enter_commit_raw`、`flip_keys/highlight_keys`、`scheme.*`(含 custom 双拼表)、`diagnostics.level/rotation/keep`;**restart 生效**——`verify_dict_on_load`、`ui.corner_radius_dp/base_alpha`、`ui.animation.*`、`theme.scheme/accent`、`phrases.*`、`data.durability/backup_*`、`diagnostics.probes`;**接受但未接线**——`engine.punct_mode`、`engine.full_width`、`engine.abbrev`、`diagnostics.log_input_content`(注释:无论真假永不记录输入)。
- 校验哲学:坏值修默认 + 每键一条 `config/invalid` 诊断、绝不阻断启动(`schema.rs:9-16`);闭合枚举用 `closed_set!` 宏;`flip_keys`/`highlight_keys` 冲突按"高亮赢、翻页让"仲裁;键名白名单 `KeyName` 与路由层 `binding_audit` 双向穷尽断言。`writeback.rs` 支持配置回写。
- 热重载:`ConfigStore` 以 `Arc` 整体替换(组合中的会话保旧配置);reload 只改进不写盘、失败保现值(reload.rs:23-30);引擎侧步骤 `config-watch` 在宿主不回调时记 pending。
- 敏感上下文:`ime-core/privacy.rs`——`InputContextKind` **fail-closed**(宿主没报的一律不学习),显示 fail-open(密码框照常出候选窗,只是不进 user.redb);`ime-fcitx5/privacy_impl.rs` 是唯一见明文 app 名的层,hash 后跨界,黑名单匹配后抑制学习。

## 7. 用户数据

- **存储 = redb 2**(`user.redb`,user_db.rs:15-20):`user_words: &str → (u32 commits, u64 last_used_ms)`、`user_meta: &str → (u64 created_ms, u64 pinned)`(pin 存 u64 防 redb bool 解码 panic)、`meta.schema_version=1`;文件 0600/目录 0700。
- **学习机制**:commit → `record` 仅入内存 delta(≤5µs 契约)→ 三个触发之一落盘(32 个不同键 / 2000ms / 4096 上限),写在自己的 flush 线程;慢盘自适应(连续 3 次 >3ms → batch 128/10s,`data/commit/slow-disk`);**自动调频**:打分 user 项 `min(log2(1+freq)/8, 2)×0.8`(score.rs:250-256),无历史时字典序决定(`viterbi/mod.rs:426` 有对照测试);读路径 ≤50k 条(HYDRATE_CAP)全量内存化、decode 永不开读事务,超限走 on-demand+4096 miss cache 并报 `data/user-db/large`。
- **用户造词:未实现**——`is_user_word` 恒 false(user_db.rs:694)、无造词写路径;淘汰:50 万上限、空闲 30s sweep 每轮 10%(批 4096);`forget/list/export_tsv(TSV)/备份恢复` 实装;**readonly 模式**:目录/文件不可写时进入只读——输入不中断、不学习,报 `data/readonly-mode`(recover.rs:75)。
- **用户短语(独立于词频库)**:`phrases.tsv` 两列文档(`rq→2026-09-29`),内置表+用户文件合并(后者赢),`AddPhrase` 由 writer 线程追加(drain 预算 100ms,shutdown 超时脱离);命中短语不进词格不打分,**作为最高优先级候选插在解码结果之前**(`ime-core/src/phrase.rs:6-9`);`$Y/$M/$D/$T` 时间变量由 host 层展开(表本身不读时钟)。

## 8. 诊断 ime-diag

- **日志**(`log.rs`):全 workspace 唯一 subscriber(Registry + RedactLayer + RedactFormat);`rspinyin.log` 0600/0700,**自实现按大小滚动**(tracing-appender 只用于预留的 non-blocking writer);目录建不出→降级 stderr 且级别抬到 warn,绝不 fail 启动。
- **脱敏**(`redact.rs:13-21,62-84`):7 个拒写字段(raw/text/preedit/input/candidate_text/word/commit_text)→ `<redacted:len=N>`;app 值回家目录 `~` 改写;带 `password/sensitive` 标志的会话自标记起全字段+消息丢弃(连长度都删),只留 app hash;`scrub_denied_values` 对自由文本消息做第二防线;第一道防线是"代码根本不传内容"。
- **崩溃**:panic hook(panic.rs)= 崩溃记录→tracing 结构事件→stderr→**不 abort 不恢复默认钩子**→注册的会话清除动作;FFI guard `catch_unwind` 兜 `extern "C"`(消息限静态模板);`crash/record.rs` 记录文件字段为闭合键集、无输入字段、消息回溯先 bound+scrub;`crash/signal.rs` 处理 **SIGBUS+SIGSEGV**(signal-hook + 经 FFI registrar 的 `sigaction` SA_SIGINFO),si_code 分类、async-signal-safe 退出。
- **探针**:Fibonacci 桶直方图(恰好 536B)/计数器/内存采样/行式 snapshot(不可信输入解析);`report.rs` 按 `budgets.json` 阈值渲染看板(文本+JSON,全 ASCII 断言);`alloc-count` crate 支撑分配预算门(`xtask budget --alloc`:steady decode 13 次分配);`uiframe/` 是测试帧镜像 schema。
- 文档登记的历史缺口(features.md:4808-4813,2026-09-30 验收记录):打点接线与 SIGUSR1 落盘路径当时未闭环;其后 commit(testd/opt 系列)显示 `xtask testd` 帧率/内存采样与镜像通道已交付(tests.md 第 0 节)。

## 9. 明确缺失 / 占位(对比基线最关键的一节)

**文档层面**:
- **`docs/dev/features-add.md` 不存在**(全仓 find 无果)——所谓 52 张 ADD-FEAT 卡无处可查。`docs/dev/` 现存规划类文档只有 `features.md`(5,108 行,Phase 1 103 卡全 COMPLETED)、`opt-ui.md`(1,414 行)、`tests.md`+`tests/` 8 个分片;`tests.md` 头部引用的 `features-test.md` 同样**不存在**(悬空引用)。
- `features.md` §5.3(34 张 Phase 2 卡)/§5.4(15 张 Phase 3 卡)只是索引表,指向的 `features/phase-2.md`、`features/phase-3.md` 文件也不存在。

**能力层面(相对 features.md 规划与同类输入法)**:
- **已实现但未接线**(引擎能力在、生产路径够不着):模糊音(无配置键、会话不设 FUZZY 位)、简拼(词库 `prefix` 未实现 + `engine.abbrev` 标记 Not wired)、简繁转换(`ime-core/script.rs` 最长匹配 + `ime-dict/script.rs` `ScriptIndex`/内置种子变体表都有,但 `ToggleScript` 在状态机里只重发帧(transitions.rs:192-198),commit 路径只走 `passthrough::transform_committed`)、标点/全角配置键。
- **完全缺失**:拼写纠错(TASK-2.02.03)、中英混输英文词补全(2.02.05;现有只是"临时英文直通"模式)、撤销上屏(2.02.10)、词频时间衰减(2.03.01)、多 Profile(2.03.02)、命令面板面板本体(2.03.03,通道/骨架在)、词库热替换 ArcSwap(2.03.04)、emoji/符号面板(2.05.05)、剪贴板集成(2.04.03)、用户造词写入(Pin 亦是 Unsupported)、Bigram 真查表(3.02.01,段槽保留)、专业领域词库叠加(3.02.02)、上下文感知排序(3.02.03)、GPU 渲染路径(3.04.01)、`ime-doctor` 自检 CLI(2.08.01;部分被 `xtask report/verify` 覆盖)。
- **契约层的占位**:`Lexicon::prefix → Unsupported`、`UserFreqSource::is_user_word → false`、`UserFreqSource.list/export_tsv` 有真实现、`PinHighlighted → Unsupported`、BIGRAM 段空。
- **刻意不做**(非缺失):云输入(3.02.04 结论"不实现")、词库同步(3.02.05)、自动更新、XIM/IBus、皮肤市场。

## 10. 候选窗口测试基建 xtask/testd

- **`xtask testd`(注入通道)**(`testd/mod.rs`):经 **XTEST FakeInput** 把"键入 nihao/点击候选格"变成真实 X 事件(走 X server→fcitx5→addon 全链路,"进程内驱动证明不了这条路由");核心纪律是 **FocusGuard**——每次注入前后向服务器查焦点,`FocusStolen` 错误带 `by_candidate_window`(按候选窗 WM_CLASS 判定),直指项目最高严重级缺陷"候选窗抢焦点"(mod.rs:14-19);只产输入与焦点裁决,不读应用侧、不断言像素。
- **真窗口怎么测(三通道)**:
  1. **上屏读回**:`xtask testd-client`(`testd/commit_readback.rs`)启动 `xtask/testd/client/rspinyin_probe_client.c`——GTK3 窗口 + GtkEntry,把 IM context 的 `commit`/`preedit-changed` 信号逐行 JSON 打到父进程管道(C 文件头注:不 OCR 截图,断言宿主真实提交文本;transcript 按 0600 用户数据对待)。
  2. **帧镜像**:`RSPINYIN_TEST_MIRROR_DIR` 武装被测会话后,UI addon 的 `mirror.rs` 把引擎发布的最新 `UiFrame` 写成 JSON;`xtask test-mirror`(`mirror_cli.rs`)读回并断言 counts/revision/页态/几何——**报告刻意不回显文本**(镜像里就是用户输入),`--json-out` 显式才拷贝进证据包(0600);`FrameWatch` 执行"旧帧丢弃计 rewind"契约。
  3. **截图**:`xtask capture`(XGetImage→PNG)与 `ui_metrics`(slint 规格度量)。
- **套件层**:`xtask testd-suite` 按 `tests/fixtures/scenarios/*.toml`(18 个场景)+ `sandbox.rs`(隔离的 fcitx5 会话)跑 P0 用例并落证据包 `results/runs/run-*/...`(heal 自愈、purity 基准纯净门、env_gate 环境门禁配套);`xtask testd-engine` 无 fcitx5 纯内存重放;`xtask soak` 长稳。另有两个 dlopen 级集成测试树:`crates/ime-fcitx5/tests/keymap_matrix/`(2,568 行按键路由矩阵)。tests.md 登记的端到端实证:沙盒 fcitx5 会话 20/20 轮完成 `nihao`+空格 → 你好 首词上屏。

---

### 与 librime / fcitx5-rime / rime-ice 对比时建议抓的锚点

- 引擎形态:单次全输入 K-best Viterbi(beam 16/整句首选)vs librime 的 syllabify+translation+candidate 分层;切分歧义用"全切分图+排序"而非 prism 音节图。
- 词库:单一自研 mmap 容器(349k 词条/292k 键/15.62MiB,无 prism、无 reverse db、无多词典叠加)vs rime 的 `.table.bin+.prism+dict.yaml` 生态。
- 语言模型:仅 unigram(hash 表)+ bigram 常量退化,无八股文式 n-gram/无 opencc 接线。
- 用户数据:redb 词频(无造词、无 decay)vs rime userdb。
- schema 概念:无;`[scheme]` 只是双拼布局选择,不是 rime 的可组合 schema。
- UI:完全自绘(Override-redirect/四档 Wayland 阶梯)+ UIPriority 接管,vs fcitx5-rime 依赖 classic-ui;Wayland 档代码在但未编译、未真机验证。
- 工程面:预算门禁(budgets.json)、零网络断言、脱敏双层、崩溃自愈、XTEST 测试平台——这一面显著超出三个参考仓库的平均水准。
