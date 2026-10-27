# 勘察摘要：rime-ice（雾凇拼音）[SCOUT-ICE]

> 对象: `/home/gong/news/rspinyin/remarks/rime-ice`（GPL-3.0，配置/词库 + lua 脚本仓，非传统源码仓）
> 仓库形态: 21 个 yaml（default + 9 拼音方案 + melt_eng/radical/t9 + symbols*2 + squirrel/weasel）+ `lua/` 22 个 .lua（2,705 行，含 `cold_word_drop/` 8 文件）+ `cn_dicts/` 6 个词库（约 45MB）+ `en_dicts/` 10 个 + Go 词库维护工具（`others/script/`）+ shell 冒烟测试框架（`others/script/smoke/`）。
> 己方映射基准: rspinyin——Rust 编译型引擎，lua 机制须转写为引擎原生 trait/构建期展开；GPL 数据与代码一律只做机制级语义转写。

---

## 一、Lua 扩展功能面清单（lua/ 逐文件精读）

引擎挂载点分三类：`lua_processor`（按键拦截）、`lua_translator`（输入段翻译/造候选）、`lua_filter`（候选流重排/改写）。主方案挂载表见 `rime_ice.schema.yaml:42-85`。

### 1.1 filters（候选流后处理）

**`long_word_filter.lua`（52 行）— 长词优先**
- 触发: 无前缀，常驻 filter。配置键 `long_word_filter/count`（默认 2）、`/idx`（默认 4）。
- 机制: 以**第一个候选项的词长**为基准，遍历候选流，把比基准长的纯汉字词提前插到第 idx 位；含英文/数字/假名的候选不提升；扫到提升满 count 个或缓冲 >50 个即停（`lua/long_word_filter.lua:16-50`）。early-exit 上界是硬编码的，不扫全流。
- 映射: 己方引擎在候选排序阶段原生实现「目标位次插桩」——排序后取前 N 做一次长度分桶重排，O(前50)。

**`pin_cand_filter.lua`（380 行）— 置顶/固顶词**
- 触发: 无前缀常驻。配置为 schema 下的列表 `pin_cand_filter:`，条目格式 `编码<Tab>字词1 字词2`（空格分隔=顺序；` > ` 亦可）。
- 机制（init）: 启动时把每条解析成 `map[去空格编码] = [词...]`；**关键增量**: 若编码含空格（如 `ni hao`），自动派生简码键——末音节首字母（`nih`）、末音节为 zh/ch/sh 时再派生双字母（`zhich`）；多条规则同简码时按书写顺序合并，**显式定义的简码键优先于自动派生**（用 set 预判，`lua/pin_cand_filter.lua:71-150`）。
- 机制（func）: 用 `context:get_preedit()` 的纯字母部分匹配，把命中的候选按配置顺序前置（占位数组保证顺序），其余缓存后回放；查找上限 100 个候选（`lua/pin_cand_filter.lua:153-236`）。
- 映射: 置顶表构建期编译进词典索引（编码→强制 rank），简码派生在 FST/dictc 构建期展开；引擎查表 O(1) 前置，无需运行期扫流。

**`corrector.lua`（291 行）— 错音错字提示**
- 触发: 常驻 filter。配置键 `corrector`（comment 模板，默认 `{comment}`）；依赖 `translator/spelling_hints: 8`（让 comment 携带全拼）+ `comment_format` 打上 `［...］` 标记（`rime_ice.schema.yaml:170-176`）。
- 机制: **硬编码约 260 条纠错表**（init 内字面量 lua table，`lua/corrector.lua:25-264`），键=错误全拼（`"gei yu"`），值={正确词, 正确读音}。func 中解析候选 comment 的 `［拼音］`，查表且 `cand.text == c.text` 才改写 comment 为正确读音（`func` 在 266-288 行）。错音（给予 jǐ yǔ）与错字（按耐→按捺）混在一表。
- 映射: 纠错表应数据化（进 dictc 构建输入），引擎在候选产出时按「候选词+用户实际键入音节」比对注音差，comment 提示；比 rime 依赖 comment 协议更直接。

**`autocap_filter.lua`（62 行）— 英文自动大写**
- 触发: 常驻 filter，按输入码形态分派。
- 机制: 输入码首字母大写→候选转首字母大写；前 2~n 位大写→全大写；转换对象限定 `cand.type == "completion"`（即英文词典补全词），纯输入码匹配或 `PS↔Photoshop` 类缩写词不转（`lua/autocap_filter.lua:63-112`）。转换用 `Candidate(...)` 重建而非改写。
- 映射: 己方英文域候选在匹配阶段做大小写折叠匹配、输出阶段按输入码形态套壳；注意其自述「大写时无法动态调频」——早转晚转有权重取舍。

**`cn_en_spacer.lua`（32 行）— 中英混输词条自动空格**
- 触发: 常驻 filter。机制: 候选文本同时含汉字与字母/数字时，用 UTF-8 字节区间正则 `[\228-\233][\128-\191]` 识别汉字边界，在中英交界处插空格（`VIP中P`→`VIP 中 P`），经 `to_shadow_candidate` 生成影子候选（`lua/cn_en_spacer.lua:9-28`）。
- 映射: 纯文本后处理，己方在候选文本定稿处做一次 Unicode 类别扫描插空格，成本忽略。

**`en_spacer.lua`（18 行）— 英文词条上屏自动前加空格**
- 触发: 常驻 filter（要求挂在 filters 倒数第二）。机制: 读 `commit_history:latest_text()`，若上屏内容与当前候选均为纯英文单词，给候选加前导空格（`lua/en_spacer.lua:7-14`）。**这是少数读取跨按键会话状态（commit 历史）的 filter**。
- 映射: 己方会话状态已有 commit 历史（用户词频批写路径），同样可查。

**`reduce_english_filter.lua`（115 行）— 英文单词降权**
- 触发: 常驻 filter。配置 `reduce_english_filter/mode`（all|custom|none）、`/idx`（降到第几位）、`/words`（自定义表）。
- 机制: 内置约 400 个「3~4 位、前 2~3 位是完整拼音、尾字母是声母」的英文单词表（如 `ranch`、`bail`——会与拼音简拼撞车的词），命中时把该候选缓存、挪到第 idx 位输出（`lua/reduce_english_filter.lua:85-115`）。
- 映射: 己方在**词典构建期**给此类英文词条打降权标记/编码层标记即可，无需运行期名单。

**`v_filter.lua`（32 行）— v 模式单字符优先**
- 触发: 输入码恰为 `v` + 1 字符（len==2）。机制: 把单字符候选（含 `symbols_v.yaml` 的 ā á ǎ à 等）提到多字符候选（van vain）之前（`lua/v_filter.lua:14-32`）。成因是英文翻译器 `initial_quality` 高于符号。
- 映射: 己方符号域候选与英文域候选的排序权重在引擎内直接编排，不需要事后滤镜。

**`is_in_user_dict.lua`（29 行）— 用户词标记**
- 触发: 常驻 filter。配置 `is_in_user_dict: true/false` 决定给「输入过的/没输入过的」候选 comment 加 `*`。机制: 按 `cand.type ∈ {user_table, user_phrase}` 判定（`lua/is_in_user_dict.lua:17-29`）。这是**用户词库命中可视化**，学习反馈面的轻量功能。
- 映射: 己方候选结构带 source 字段，UI 层直接渲染标记。

**`t9_preedit.lua`（18 行）— 九宫格 preedit 转写**：iRime 专用，把数字 preedit 显示还原为拼音/英文（`lua/t9_preedit.lua:6-18`）。己方无九宫格域，可忽略。

**`debuger.lua`（12 行）— 调试探针**：把每个候选包成 ShadowCandidate，comment 塞入 `input - preedit - cand.preedit` 三元组（`lua/debuger.lua:3-11`）。**机制价值: 无侵入候选管线观测**——己方可在 debug 构建做等价的候选链路 trace 层。

**`cold_word_drop/`（8 文件 580 行）— 删词/隐藏/降频**
- `processor.lua`（132 行）: 拦截快捷键 `Control+d`（删词）/`Control+x`（隐藏）/`Control+j`（降频，键位在 `key_binder/drop_cand` 等），把当前高亮候选写入 `lua/cold_word_drop/{drop,hide,reduce_freq}_words.lua` 文件（**用 lua 源文件当持久化存储**，`io.open` 直写+`table.serialize`，`lua/cold_word_drop/processor.lua:101-117`）。
- `filter.lua`（72 行）: 读回三张表，删词/隐藏的候选直接吞掉，降频的压到第 idx 位（`cold_word_drop/filter.lua:24-70`）。
- 辅助: `metatable.lua`（168 行，table.serialize 元表魔法）、`string.lua`、`logger.lua`。
- 机制价值: **用户级负反馈（删词）是最缺失的产品能力之一**——交互（快捷键+当前候选）与存储（词→编码映射表）都值得转写；但存储应进己方用户词库（redb），而非源码文件。
- 映射: 删词=用户词库 tombstone；降频=用户词权重惩罚；引擎已有 UserFreqSource 通道，补「负向写入」语义即可。

### 1.2 translators（输入段→候选）

**`date_translator.lua`（146 行）— 日期时间族**
- 触发前缀: 完整匹配配置键（`rq`/`sj`/`xq`/`dt`/`ts`/`rqzh`/`rqen`，`rime_ice.schema.yaml:89-96`）。
- 输出契约: `rq`→4 种日期格式；`sj`→`HH:MM`/含时段中文（按小时分 早上/中午/下午/晚上/凌晨）；`xq`→星期X/礼拜X/周X；`dt`→ISO 8601 含时区规整（`+0800`→`+08:00`，零区→`Z`）；`ts`→秒级时间戳；`rqzh`→中文数字年月日（〇/零两式）；`rqen`→英式/美式英文日期（`lua/date_translator.lua:32-132`）。候选 `quality = 100` 强制置顶。
- 映射: 己方引擎时间域 translator——注意 **`os.time()` 是时钟读取**，己方约束「decoder 为纯函数」，时间须由 host 侧注入 trait（与 rime 的 lua 直读时钟不同），且解析要防崩溃。

**`lunar.lua`（85 行）+ `lunar.db`（722KB 二进制）— 农历/节气**
- 触发: `nl`（今日农历）或 `N + YYYYMMDD`（指定日期，recognizer tag `gregorian_to_lunar`）。
- 机制: **农历换算数据预编译进 ReverseDb 二进制**（`lua/lunar.db`，Rime 反查库格式），lua 只做查库+模板渲染（`{干支年}{生肖}年{俗称农历月}{农历日}` 六字段模板，`lua/lunar.lua:20-58`）。支持 1900-2100 年、二十四节气。
- 映射: 己方若做，农历数据进构建期编译表（或代码生成），运行期查表——数据与逻辑分离的做法与 rime 同构。

**`uuid.lua`（46 行）**：输入 `uuid`（可配）产出 v4 随机 UUID，`math.random` 按位拼（`lua/uuid.lua:15-27`）。映射: 引擎侧 trivial；注意随机源与「纯函数解码」约束的兼容（host 注入）。

**`unicode.lua`（36 行）— 码点直输**
- 触发: `U + 十六进制`（前缀取自 `recognizer/patterns/unicode` 第 2 字符）。
- 输出: 该码点字符 + 其后 16 个码点（前缀补全式联想，`code < 0x10000` 时），comment 显示 `U4f60~0` 式（`lua/unicode.lua:12-33`）；超 0x10FFFF 输出「数值超限！」防崩。
- 映射: 己方反查/符号域原生支持码点输入，天然无 lua。

**`number_translator.lua`（162 行）— 数字/金额大写**
- 触发: `R + 数字`（前缀取 `recognizer/patterns/number` 第 2 字符）。
- 输出契约: 一次产出 4 个候选——〔数字小写〕〔数字大写〕〔金额小写〕〔金额大写〕，金额走 角分厘毫 位映射，整数位 万/亿 进位，会计规范「壹拾X」特判（`lua/number_translator.lua:114-161`）。
- 映射: 纯算法，无状态无时钟，己方引擎数字域直译；与 `convert_ar_num_to_zh.lua` 共享算法。

**`convert_ar_num_to_zh.lua`（120 行）— 数字→中文算法库**（被 date_translator require）：`convert`（十/百/千/万/亿/兆 进位读法，≤9 位）与 `digits`（逐位读法 〇/零，`lua/convert_ar_num_to_zh.lua:38-115`）。

**`calc_translator.lua`（224 行）— 计算器**
- 触发: `cC + 表达式`（tag `calculator`；前缀键 `calculator/prefix`）。
- 机制: **`load()` 动态执行 lua 表达式**（sandbox: 环境 `calcPlugin` 只暴露白名单函数 sin/cos/log/fact/avg/var 等），`N!`→`fact(N)`、`50%`→`(50/100)` 预处理，成功产出「结果」与「表达式=结果」两个候选，失败回显表达式+「解析失败」（`lua/calc_translator.lua:208-222`）。
- 映射: 己方绝不可 eval——用表达式解析 crate（如 `meval` 类）或自写 PR 解析器；候选契约（值 + `expr=value` 双候选）可照搬。

**`search.lua`（306 行）— 反查/辅码（以词定音的反向：以辅码筛字）**
- 触发: 主输入 + 辅码引导符 `` ` `` + 辅码（如 `ni`ren`），tag 过滤（默认 `abc`）。
- 双通道: ① schema 查询——用 `Memory:dict_lookup` 在指定 schema（默认 `radical_pinyin`）查「辅码→字表」，与候选取交集（慢但能吃拼写运算）；② reverse db——`ReverseLookup:lookup(字)` 得码再正则比对辅码（快但只匹配原始码），支持 `*` 通配（`lua/search.lua:60-101`）。
- 配置键族: `search/schema`、`/schema_search_limit`（默认 2000）、`/db`（反查库列表）、`/wildcard`、`/fuma_format`（辅码转写 Projection，如 xlit/ABCD/1234/）、`/show_other_cands`、`/code_pattern`、`/tags`、`key_binder/search`；另有 `select_notifier` 接管选词、`commit_notifier` 把「辅码上屏词」手动写回用户词库（`lua/search.lua:114-227`）。
- 机制价值: **辅码筛候选 = 拼音输入的精确定位能力**（雾凇用拆字音做辅码域）。己方映射: 引擎内建辅码过滤 trait——候选产出后按第二编码域（拆字/笔画）过滤重排，词典构建期为每条词条预存辅码列，比 rime 运行期 reverse lookup 快一个量级。

**`select_character.lua`（60 行）— 以词定字**
- 触发: 有候选时按 `[`（取首字）/ `]`（取尾字），键位 `key_binder/select_first_character`、`/select_last_character`。
- 机制: lua_processor 拦截按键，取 `get_selected_candidate().text` 的首/尾单字直接 `commit_text`；支持长句、光标不在末尾时保留右侧未处理输入（`lua/select_character.lua:20-60`）。
- 映射: 己方按键处理层（host 侧）对候选文本做字级切分上屏，无引擎改动。

### 1.3 其他

**`force_gc.lua`（9 行）**：挂 translator 位，每按键周期 `collectgarbage("step")` 防 lua 堆膨胀（librime-lua#307）。**负面教材价值: 己方无 GC，此痛点不存在**——但「常驻空闲期内存巡检」的思路对应己方 buffer 池收缩策略。

---

## 二、Schema 配置组织

**`default.yaml`（448 行）— 全局层**: `schema_list`（9 方案注册）；`menu/page_size`；`switcher`（F4/Ctrl+\` 选单 + `save_options` 开关记忆清单）；`ascii_composer/switch_key`（**每个修饰键独立策略**: Caps_Lock=clear、Shift_L=commit_code、Shift_R=noop——左右 Shift 不同策略是中文 IME 关键手感）；`navigator`（按音节移动光标: `left_by_syllable`，`default.yaml:90-96`）；`punctuator`（单映射=直接上屏、数组=复选、`pair`=配对引号回切，`digit_separators` 数字后标点直通）；`recognizer/patterns`（email/url/underscore 不上屏正则）；`key_binder`（含小键盘全映射到主键盘、` emacs_editing ` 注释模板）。

**`rime_ice.schema.yaml`（448 行）— 主方案**:
- `engine` 三段管线显式排序（processors/segmentors/translators/filters），**filters 顺序即语义**（注释明示「置顶 > Emoji > 简繁」「长词优先 > Emoji」，`rime_ice.schema.yaml:74-85`）。
- `switches`: 每开关 `{name, states[中文标签], reset, abbrev}`——`reset` 存在=切窗口重置，注释=跨窗口记忆（`rime_ice.schema.yaml:21-38`）。开关与 filter/simplifier 的 `option_name` 绑定。
- `translator`（主翻译器）关键参数: `enable_word_completion`（>4 音节补全）、`spelling_hints: 8`（comment 注音——为 corrector 服务）、`initial_quality: 1.2`（拼音>英文 1.1>中英混 0.5，custom_phrase 99——**四域权重阶梯**）、`comment_format` xform 打标记、`preedit_format`（显示层转换 ju/qu、消除大写间空格）。
- `recognizer/patterns` 方案级: `punct: ^v([0-9]|10|[A-Za-z]+)$`（v 符号域）、`radical_lookup: ^uU[a-z]+$`、`unicode: ^U[a-f0-9]+`、`number: ^R...`、`calculator: ^cC.+`、`gregorian_to_lunar: ^N...`——**反查/快捷域前缀全部集中于此声明，lua 脚本运行期反读该键**（如 `unicode.lua:15`）。
- `speller/algebra`（拼写运算，见 §2.1）。
- `editor/bindings`: 空格=confirm、Return=**上屏原始输入**、Ctrl+Return=上屏转换后、Ctrl+Shift+Return=上屏 comment（corrector 冒烟测试靠它验证）、Ctrl+BackSpace=**back_syllable 删除一个音节**、Ctrl+Delete=删候选降权（`rime_ice.schema.yaml:290-300`）。

**2.1 speller/algebra 拼写运算（rime 的核心正交机制）**
- 四种算子: `derive`（派生: 原码保留+新码并存，模糊音/纠错全靠它）、`xform`（变形: 替换原码，双拼映射用）、`abbrev`（派生简码: `abbrev/^([a-z]).+$/$1/` 超级简拼）、`erase`（剔除: `erase/^hm$/` 让「噷」不独占简拼位）。
- **模糊音 = derive 行**，注释态存于 `rime_ice.schema.yaml:313-349`（zcs/h、n/l、an/ang、in/ing… 约 30 条，用户按需解开）；**自动纠错 = 有副作用的 derive 族**（`hzi→zhi`、`zih→zhi`、`wia→wai`、`nag/agn→ang` 等键序颠倒容忍，`:372-448`，头部注释明示对全拼简拼混输的副作用边界）。
- **双拼变体 = xform 链**: `double_pinyin*.schema.yaml` 与主方案 diff 仅在 speller/algebra（用 `<q>` 占位符两段式替换再剥壳）、`preedit_format`、触发关键字（rq→date）、去掉 v_filter/long_word_filter（`diff rime_ice.schema.yaml double_pinyin.schema.yaml` 确认，8 个变体文件 16KB/个，差异面 <5%）。
- 映射: **derive/abbrev = 构建期 FST 展开的派生编码**（一条 derive 行 = dictc 构建时对每个音节生成别名边）；xform = 键位映射函数。己方把「模糊音/纠错/简拼」做成构建配置而非运行规则，索引膨胀一次性付清。

**`melt_eng.schema.yaml`（英文方案）**: `algebra_common`（`:76-140`）用 derive 表达**数字读音派生**（`2→two/to`、`0→o/O/zero`）与**符号读音派生**（`+→plus`、`#→hash/number/sharp`、`@→at`）+ 大小写全排列派生（\U/\L + 2~10 位前缀大写逐条 derive）——这就是 `P2P`、`X光`、`HELLO` 的实现层，纯构建期展开。`algebra_rime_ice` 再 __append 阿拉伯数字读法（`10000→1wan`，`:141-165`）。

**`t9.schema.yaml`**（九宫格）: 依赖 rime_ice 方案（`schema/dependencies`），用 `xlit` 算子把字母表转写为数字键位。

**`symbols_v.yaml`（31KB）/ `symbols_caps_v.yaml`**: 66 个 `v + 双键` 符号子域（vfh 符号/vsx 数学/vpy 拼音/vjm 假名…），每键一个数组 + `vhelp`/`vlist` 自描述索引（`symbols_v.yaml:12-13`）； recognizer `punct: ^v...` 使 v 前缀进入符号分段。大写版 `V` 为 caps 变体。**机制价值: 符号系统的「域前缀+索引页+翻页」三件套**，己方符号域可直接吸收其分类学与索引页交互。

---

## 三、词库分层

**`rime_ice.dict.yaml`（主词库入口，77 行）**: 本体只放**大写字母与数字的特殊词条**（`A A`、`3D`、`M1`——让大写/数字参与造句，如 `zuogeDNAjiance→做个DNA检测`），正文用 `import_tables` 引入 6 个分表（`rime_ice.dict.yaml:7-13`）；**重复词条时最上面的表权重生效**（分层覆盖规则，行 15-17 注释）。

**`cn_dicts/` 六表分层**:
| 表 | 大小 | 定位 |
|---|---|---|
| `8105.dict.yaml` | 116KB | 单字表：《通用规范汉字表》8105 字，来源 wiktionary+25亿字语料字频；多音字处理策略写在头注（权重比 5:100 参与自动注音，`8105.dict.yaml:11-15`） |
| `41448.dict.yaml` | 387KB | 大字表（按需启用，含扩展区汉字） |
| `base.dict.yaml` | 16.6MB | 基础词库：十余部词典合并去重，修订记录极详（头注 80+ 行） |
| `ext.dict.yaml` | 11.9MB | 扩展词库：只收 ≥3 字词，日常增量 |
| `tencent.dict.yaml` | 17.3MB | 腾讯词向量大词库：**全部无注音**，靠字表自动注音 |
| `others.dict.yaml` | 17KB | 杂项：容错词（口语读音多注音并存，如 阿房宫 4 种读法全收） |
- 条目格式: `词\t拼音(空格分隔)\t权重`，三列可缺省（无拼音→自动注音；无权重→构建工具补 100）。
- **多音字解法**: tencent 表无注音 → 字表按权重比自动注音 → 歧义词手工注音（如「的 di」全量手注使 de 权重比成立）；待注音清单外置 `others/script/rime/需要注音.txt`（20KB）。

**`en_dicts/`**: `en.dict.yaml`（370KB 英文词库）+ `en_ext.dict.yaml`；**`cn_en.txt` 中英混输表**（`X光\tXguang`）+ **7 个双拼变体自动生成副本**（`cn_en_flypy.txt` 等——由 Go 工具从主表转写，生成物入库）；格式同 tabledb（`#@/db_type tabledb` 头）。

**`custom_phrase.txt`（自定义短语）**: `词汇\t编码\t权重` 三列，编码任意（不限于拼音，如 `噷\thm`、`有\tu\t3`）；挂 `table_translator@custom_phrase`，`initial_quality: 99` 独占最高权重；`db_class: stabledb` 只读不可调频（改 tabledb 可调频）；头注说明**其与造词系统的互斥性**（完整拼音编码会阻断该词条参与造词）（`custom_phrase.txt:1-20`）。双拼用户另建 `custom_phrase_double.txt`。

**opencc/**: `emoji.json/emoji.txt`（Emoji 映射）、`others.txt`——simplifier 双用途（简繁 + Emoji 注入 filter）。

**映射**: 己方 dictc 已合并多源词表；值得吸收的是**分层覆盖语义**（import 顺序=优先级）与「大写字母/数字入词参与造句」的编码域扩展（己方 base.dict 的 348,972 条亦可加注音权重分域）。

---

## 四、配置补丁机制

- **`*.custom.yaml` patch 协议**: librime 原生机制——用户目录放 `<file>.custom.yaml`，`patch: {路径: 值}` 深合并覆盖原配置；本仓自身不带 custom 文件（`ls *.custom.yaml` → 无，仅示例在 `others/patch_examples/`），**发布升级与用户定制通过「原文件可整体替换 + custom 文件永不触碰」共存**。
- `others/patch_examples/` 4 文件: 给出双拼适配跨文件补丁范例——`speller/algebra` 用 `__include: radical_pinyin.schema.yaml:/algebra_double_pinyin_flypy` **跨文件引用子树**（`others/patch_examples/radical_pinyin.custom.yaml`），即「一份拼写运算多方案共享」的组装术。
- **`__include` / `import_preset` / `__append`**: yaml 级组合原语。`rime_ice.schema.yaml:241-247` 从 default.yaml / symbols_v.yaml 拉子树；`melt_eng.schema.yaml:141` `algebra_rime_ice: __include: algebra_common + __append: [...]`——**公共层+方案层叠加**。`__include` 可带 `?`（缺文件不报错，`radical_pinyin.schema.yaml:31`）。
- **`recipe.yaml`**: plum（东风破）安装清单 `install_files:` 枚举全部交付文件——发布面=文件清单制。
- **版本字段即兼容协议**: `config_version`（default）与 `schema/version` 均为日期串；`default.yaml:5` 注释「要比共享目录的同名文件的 config_version 大才生效」——**版本比较决定覆盖**。
- 映射: 己方配置是编译期 TOML，无等价补丁协议；可吸收的是「用户覆盖层与发布层物理分离 + 跨文件子树引用」的思想（如用户自定义词/置顶表独立文件，dictc 合并时用户层优先）。

---

## 五、测试基建（负向预期被推翻——有真实测试）

**任务书预期「无传统测试」不成立，实测三层**:

1. **CI 冒烟测试框架**（`others/script/smoke/`，bash + 数据驱动）:
   - 用例表 `cases/rime_ice/input_cases.tsv`（216 行，**≈140 条用例覆盖 9 个方案**），格式 `case_id\tschema_id\tkey_sequence\texpected_text`，键序列写 `{space}` `{2}` `{bracketleft}` `{Control+Shift+Return}`；期望值支持 `@today:%Y-%m-%d`（时间相对断言）与 `@regex:^...$`（UUID v4 版本位断言）（`smoke/cases/rime_ice/input_cases.tsv:17-18`）。
   - 执行链: `rime_deployer --build` 部署 → `rime_api_console` 逐条回放按键 → 断言 commit 输出；stderr 出现 `E...` 级日志即失败（`smoke/README.md:36-42`）。本地跑需 `SMOKE_ALLOW_DESTRUCTIVE=1`（会删 build/ 与 userdb）。
   - **覆盖面**: 每个插件至少 1 条冒烟（置顶、Unicode、日期、UUID、降权、数字、计算器、农历、以词定字、辅码、错音提示——最后者用 Ctrl+Shift+Return 上屏 comment 验证 corrector）。
2. **lint**（`others/script/lint/`）: yamllint（`.yamllint.yml`）+ luacheck（`.luacheckrc`）+ lua 格式检查，`make lint` 聚合。
3. **CI**（`.github/workflows/test.yml`）: PR 触发，ubuntu 跑 lint + smoke，rime-cli 二进制缓存复用。

**`others/script/` Go 词库维护工具**（非测试，构建工具链）: `main.go` 流水线 = Emoji 检查再生（`emoji.go`）→ 中英表再生（`cn_en.go`，18KB，**生成 7 个双拼副本**）→ 半自动注音（`pinyin.go`，缺注音词与 `需要注音.txt` 比对）→ 补权重 100 → 四表格式检查（`check.go`，`_type` 1-4 对应列数模式）→ 同义多音字检查（`polyphone.go`，数据 `错别字.txt` 60KB）→ 排序去重（`sort.go`，「拼音顺序、权重逆序」）。**机制价值: 词库作为代码维护——检查器+生成器+格式化器三件套**，与己方 dictc 定位同构，其「注音校验/多音字审计」是己方 dictc 可补的质控维度。

---

## 六、候选吸收点清单

| 编号 | 源码锚点 | 一句话机制价值 | 移植难度 |
|---|---|---|---|
| SCOUT-ICE-01 | `lua/pin_cand_filter.lua:71-150` + `rime_ice.schema.yaml:158-162` | 置顶词表+简码自动派生（末音节首字母/zh-ch-sh 双字母），构建期展开为己方词典索引的强制 rank | 低 |
| SCOUT-ICE-02 | `lua/search.lua:114-284` + `key_binder/search: "\`"` | 辅码（`` ` `` 引导）筛选候选：候选流按第二编码域过滤，词库预存辅码列可 O(1) 化 | 高 |
| SCOUT-ICE-03 | `lua/corrector.lua:25-264` + `translator/spelling_hints` | 错音错字提示：纠错对表（词+错拼→正确读音）comment 注入，表数据化进 dictc | 中 |
| SCOUT-ICE-04 | `lua/cold_word_drop/processor.lua:169-202` + `filter.lua:24-70` | 用户负反馈三态（删词/隐藏/降频）快捷键交互，存储换己方用户词库 tombstone/惩罚权重 | 中 |
| SCOUT-ICE-05 | `lua/date_translator.lua:32-132` + `recognizer/patterns` | 快捷翻译器族（rq/sj/xq/dt/ts/rqzh/rqen 七前缀多格式候选），时钟须 host 注入保纯函数 | 低 |
| SCOUT-ICE-06 | `lua/number_translator.lua:114-161` + `convert_ar_num_to_zh.lua` | 数字/金额四式大写候选（小写/大写/金额小写/金额大写+壹拾会计规范），纯算法直译 | 低 |
| SCOUT-ICE-07 | `rime_ice.schema.yaml:304-448` speller/algebra | derive/abbrev/erase 拼写运算=构建期 FST 派生边：模糊音、超级简拼（zh/ch/sh 整体）、键序纠错 | 中 |
| SCOUT-ICE-08 | `melt_eng.schema.yaml:76-165` algebra_common | 英文编码派生体系：数字/符号读音（P2P/X光/HELLO）、大小写全排列、`X光\tXguang` 混输表 | 中 |
| SCOUT-ICE-09 | `rime_ice.schema.yaml:165-217` 四翻译器权重阶梯 | 拼音 1.2 > 英文 1.1 > 混输 0.5 > custom_phrase 99 的跨域排序契约 | 低 |
| SCOUT-ICE-10 | `lua/autocap_filter.lua:63-112` | 英文候选按输入码形态自动大写（首字母/全大写），匹配折叠+输出套壳 | 低 |
| SCOUT-ICE-11 | `lua/long_word_filter.lua:16-50` | 长词优先：以首候选词长为基准提前 ≤count 个长词到第 idx 位，扫描上限 50 | 低 |
| SCOUT-ICE-12 | `lua/reduce_english_filter.lua:17-56` | 与拼音简拼撞车的英文短词降权名单——构建期给词条打降权标记 | 低 |
| SCOUT-ICE-13 | `symbols_v.yaml:12-13` + `recognizer/patterns/punct` | v 前缀 66 个符号子域+自描述索引页（vhelp/vlist），分类学与翻页交互可整体吸收 | 中 |
| SCOUT-ICE-14 | `lua/select_character.lua:20-60` | 以词定字：`[`/`]` 取当前候选首/尾字上屏，支持长句与光标位置保留 | 低 |
| SCOUT-ICE-15 | `lua/is_in_user_dict.lua:17-29` | 候选来源可视化（用户词加 * 标记），己方候选 source 字段直通 UI | 低 |
| SCOUT-ICE-16 | `lua/en_spacer.lua:7-14` + `commit_history` | 跨按键 commit 历史驱动的上屏后处理（英文连排自动补空格） | 低 |
| SCOUT-ICE-17 | `others/script/smoke/cases/rime_ice/input_cases.tsv` | 数据驱动 IME 冒烟测试：TSV 键序列→期望文本（@today/@regex 动态断言），可直接改造成己方 headless 协议用例 | 低 |
| SCOUT-ICE-18 | `others/script/main.go:37-93` + `polyphone.go` | 词库质控三件套（注音校验/多音字审计/排序去重），dictc 可补同维子命令 | 中 |
| SCOUT-ICE-19 | `others/patch_examples/radical_pinyin.custom.yaml` + `__include` 体系 | 用户覆盖层与发布层物理分离、跨文件子树引用（含 `?` 容错）——己方配置合并可借鉴 | 中 |
| SCOUT-ICE-20 | `rime_ice.dict.yaml:22-76` | 大写字母/数字入词典参与造句（zuogeDNAjiance→做个DNA检测），编码域扩展 | 中 |
| SCOUT-ICE-21 | `lua/lunar.lua:20-58` + `lunar.db` | 农历/节气：换算数据预编译二进制、lua 只做查表+模板渲染——数据逻辑分离同构于 dictc | 高 |
| SCOUT-ICE-22 | `lua/unicode.lua:12-33` | 码点直输（U+hex→字符+后 16 码点联想+超限防崩），反查域原生实现 | 低 |
| SCOUT-ICE-23 | `rime_ice.schema.yaml:290-300` editor/bindings | 编辑键语义表：Ctrl+BackSpace=按音节回删、Ctrl+Delete=删词降权、三档 Return 上屏 | 低 |
| SCOUT-ICE-24 | `lua/calc_translator.lua:208-222` | 表达式计算候选（双候选契约：值 + expr=value），须换安全解析器禁 eval | 中 |
| SCOUT-ICE-25 | `default.yaml:72-79` ascii_composer/switch_key | 左右修饰键独立中英切换策略（Shift_L=commit_code / Shift_R=noop）+ Caps 三态 | 低 |

（GPL-3.0 合规: 以上全部为机制级语义转写；纠错表、降权词表、农历数据、lua 代码本体均不可拷贝。）
