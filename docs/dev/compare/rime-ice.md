# rime-ice(雾凇拼音)深度调研报告

> 来源: 2026-10-14 四路并行调研之子代理原始报告,全文存档未删改 ｜ 调研对象: `remarks/rime-ice`(版本约 2026-03,GPL-3.0) ｜ 交叉对比结论见 [rime-comparison.md](rime-comparison.md) ｜ 除特别标注外,所有数字均来自实际命令测量

调研对象:`/home/gong/news/rspinyin/remarks/rime-ice`(版本约 2026-03,GPL-3.0)。除特别标注外,所有数字均来自实际命令测量。

---

## 1. 词库文件清单与规模

### 1.1 主词典索引 `rime_ice.dict.yaml`(77 行)
仅是 `import_tables` 清单:8105(字表,默认启用)、base、ext、tencent、others;41448 默认注释掉(按需启用,须放在 8105 之后)。另有巧思:直接在主词典里挂 **26 个大写字母条目**(`A A`、`B B`…)使 Shift+字母 可直接参与中文造句(`zuogeDNAjiance`→做个DNA检测),以及 `0~9` 的多读音条目(`1 yi / 1 yao`)和 `M1`、`3G`、`3D打印` 等示例条目——用**纯数据**实现了"大写字母+数字参与造词"。

### 1.2 cn_dicts/(wc -l 实测)

| 文件 | 行数 | 实际词条数(估) | 权重 | 说明 |
|---|---|---|---|---|
| `8105.dict.yaml` | 8,828 | 8,758(带权重) | 北语 25 亿字语料字频 | 《通用规范汉字表》8105 字+扩充;数据来自 wiktionary,注音参照《现汉》手动补校 |
| `41448.dict.yaml` | 46,055 | 46,023 | **全部无权重**(部署时默认值) | Unihan 16.0 `kMandarin` + mozillazg/pinyin-data(汉典)大字表,生僻字殿后 |
| `base.dict.yaml` | 558,177 | ~543,118 | 真实调频 | 核心词库:**无单字**;2 字词 139,260 / 3 字词 157,339 / 4+ 字词 246,519 |
| `ext.dict.yaml` | 339,211 | ~339,186 | **全部=100** | 3 字及以上;**所有含多音字的词条必须放这里**(手动注音) |
| `tencent.dict.yaml` | 981,574 | ~981,543 | **全部=100** | 腾讯词向量,只保留 3 字及以上,**无注音**(Rime 按字表自动注音),**禁止含多音字** |
| `others.dict.yaml` | 942 | ~900 | 少量 | 容错词/口语读音双注音(空落落 luo/lao、阿房宫 e/a pang、坐骑 qi/ji、铁骑 qi/ji 等争议读音并列) |

总计约 **197 万行中文词条**(8105+41448+base+ext+tencent ≈ 191.9 万条,不含注释行)。

### 1.3 en_dicts/
- `en.dict.yaml` 23,177 行(~2 万常用词):来源 google-10000-english 的 20k 词表 + 融合拼音方案 ~1000 词。
- `en_ext.dict.yaml` 2,639 行:缩写、按键名(Fn/Cmd/CapsLock)、`.DS_Store`、`README.md` 等非词典词。
- `cn_en*.txt` 8 个文件各 1,011 行:中英混合词库,全拼版 + 7 种双拼版,由 `others/cn_en.txt`(纯词表)用 Go 脚本自动生成。
- `melt_eng.dict.yaml` 仅 13 行:import en_ext(放前,权重优先生效)+ en。

### 1.4 其他
`radical_pinyin.dict.yaml` 132,486 行(Mirtle 部件拆字码表,含特殊部件注音如`一 heng2`、`丿 pie3`)。

### 1.5 8105 字表与多音字标注
- 8105 中 **535 个字有多个读音条目**,同字不同音、权重不同。实测样例:
  - `的 de 76938354 / di 106917`(比值 ~719:1)
  - `地 de 8804040 / di 8804040`(1:1)
  - `长 chang 3100060 / zhang 3100060`;`行 xing 2756706 / hang 2756706 / heng 1`;`了 le 3481243 / liao 999`
- **关键机制**(8105/tencent 头注释原文):对无注音词库(tencent),Rime 按方案字表自动注音,**只有字表中权重比 > 5:100 的读音才参与自动注音**。因此"的"把 `di` 的词全部手动注音,让 `de` 独占自动注音;难区分的(长 chang/zhang)全部手动注音。构建脚本 `others/script/rime/polyphone.go` + `需要注音.txt` 做机器校验。
- 845 个生僻字频率为 0 或 1,自然沉底。

---

## 2. 词频来源与归一化

### 2.1 来源(base.dict.yaml 头注释 + others/docs/Credits.md 实证)
1. **华宇野风系统词库**(华宇拼音论坛,Public Domain)
2. **清华大学开源词库 THUOCL**(MIT)
3. **现代汉语常用词表**(国家语委)
4. **腾讯词向量**(Tencent AI Lab embedding 词表,@Huandeep 整理,CC BY 3.0)——补充大量常用两字词
5. 参校:《现代汉语词典》《通用规范汉字字典》《现代汉语规范词典》《同义词词林》《新华成语大词典》;ext 另参考简化字八股文(rime-essay-simp)

### 2.2 数值范围(实测分位数)

| 词库 | min | p25 | median | p75 | p90 | p99 | max |
|---|---|---|---|---|---|---|---|
| base | 1 | 21 | 550 | 4,315 | 16,125 | 207,430 | **19,260,817** |
| 8105 | 0 | — | 3,825 | — | 358,239 | 3,608,341 | **76,938,354**(的) |
| ext / tencent | 100 | 100 | 100 | 100 | 100 | 100 | 100 |

### 2.3 归一化策略——这是最值得注意的工程决策
- **三层策略**:base(2 字词为主)继承来源语料的真实词频、跨度 1~1900 万;ext/tencent(3+ 字词,共 ~132 万条)**全部压平为 100**,只靠"是否收录"不靠"词频"区分;字表用真实字频,并刻意控制多音字读音间的比值作为自动注音开关。
- **词长协调**:base 内 2 字词平均权重 25,488、3 字词 7,058、4+ 字 3,928——短词系统性高于长词,即"同音候选里先短词后长词"不完全靠词频自然涌现,而是调频时人为保持梯度(再用 long_word_filter 滤镜反向微调)。
- **顶置词条**:base 里存在极端手工权重(江泽民 19,260,817、版权 13,204,281、三国群英传 12,345,678),显然是"硬顶置"手段。
- **维护纪律**(AGENTS.md):新词条默认词频 100,同编码重码时才按日常习惯调;两字词必须进 base;词库构建脚本自动"按拼音顺序、权重逆序"排序去重。
- base 头注释记录了大量人工校对流程:opencc 修繁体、pypinyin 修错音、删大写数字、删除几乎不会用到的两字词、降频不常见人名/地名/小品牌;还保留了《现汉》vs《现规》异形词争议清单(滴答/嘀嗒、启航/起航等数十条)作为"校对日志"。

---

## 3. schema 管线(`rime_ice.schema.yaml`,449 行)

### 3.1 engine 全清单
```yaml
processors: [lua_processor@*select_character(以词定字), ascii_composer, recognizer,
             key_binder, speller, punctuator, selector, navigator, express_editor]
segmentors: [ascii_segmentor, matcher, abc_segmentor,
             affix_segmentor@radical_lookup(拆字tag), punct_segmentor, fallback_segmentor]
translators: [punct_translator, script_translator(主拼音),
              lua: date_translator / lunar / uuid / unicode / number_translator /
                   calc_translator / force_gc,
              table: custom_phrase / melt_eng(英文) / cn_en(中英混合) / radical_lookup(拆字反查)]
filters:     [corrector(错音错字), radical_reverse_lookup(拆字注音), autocap_filter(英文大写),
              v_filter(v模式单字符优先), pin_cand_filter(置顶), long_word_filter(长词优先),
              reduce_english_filter(降英文), simplifier@emoji, simplifier@traditionalize,
              search@radical_pinyin(辅码), uniquifier]
```
注释中明确了**滤镜顺序契约**:"置顶候选项 > Emoji > 简繁切换"、"长词优先 > Emoji"——因为 simplifier 产出的 emoji 候选不该被后续滤镜移动/标记。

switches:ascii_mode、ascii_punct(¥/$)、traditionalization(简/繁)、emoji(reset:1)、full_shape、search_single_char(辅码单字优先,带 abbrev 显示"词/单")。

### 3.2 translator 参数(主翻译器)
```yaml
translator:
  dictionary: rime_ice
  enable_word_completion: true   # ≥4音节词条自动补全 (librime>1.11.2)
  # max_sentences: 1             # 造句数 (librime≥1.17.0)
  spelling_hints: 8              # 把全拼写进 comment —— 纯粹为 corrector.lua 服务
  always_show_comments: true     # 强制显示 comment,供 corrector 判断
  initial_quality: 1.2           # 拼音 > 英文(1.1) > 中英混合(0.5)
  comment_format: [xform/^/［/, xform/$/］/]   # 用全角括号"打标记"
  preedit_format:                # 显示层转换:jv→ju、nv→nv、消除大写字母间空格
  # dictionary_exclude: [...]    # NSFW 屏蔽 (librime≥1.14.0)
```
`spelling_hints + always_show_comments + comment_format` 三件套是"用引擎的注释通道给 Lua 传全拼"的典型技巧,使同一个 corrector 能同时适配全拼和双拼。

### 3.3 speller/algebra(拼写运算,全拼版)
- `alphabet` 含全部大小写字母 + `` ` ``(辅码引导符);`delimiter: " '"`
- **模糊音模板**:20+ 条全部注释待启用,单向/成对 derive(zh↔z、n↔l、f↔h、ang↔an、in↔ing、ian↔iang…甚至 fei↔hui 整音节)
- **超级简拼**:`erase/^hm$|^m$|^n$|^ng$/` 清除 4 个独占音节 + `abbrev/^([a-z]).+$/$1/`(每音节取首字母)+ `abbrev/^([zcs]h).+$/$1/`(zh/ch/sh 整体化,`ch'sh`→城市 而非 `c'h's'h`)
- **v/u 容错**:`nl ve↔ue`、`jqxy u↔v` 双向 derive,兼容错误注音习惯
- **自动纠错**(约 30 条 derive,注释承认对简拼混输有副作用):
  - 声母错位:`hzi→zhi`(h 与 zcs 交换)、`zih→zhi`
  - 韵母键序颠倒:`wia→wai`、`wie→wei`、`jei→jie`、`oa↔ao`、`ou→uo`
  - ng 拆分:`ang→nag/agn`、`eng→neg/egn`、`ing→nig/ign`、`ong→nog/ogn`、`uang→aung/uagn/unag/augn`、`iong→inog/oing/iogn/oign`
  - 缩写:`do→dou|dong`、`lon→long`、`ten→teng`、`lng→lang|leng|ling|long`
  - 部分被注释掉因为有常用词冲突(`uan→una` 撞"去哪、露娜")

### 3.4 recognizer / editor / key_binder
recognizer patterns 把**方案级功能全部前缀化**:`^v...$`(symbols)、`^uU[a-z]+$`(拆字反查,双字母前缀避免吃掉英文)、`^U[a-f0-9]+`(Unicode)、`^R[0-9.]+`(金额大写)、`^cC.+`(计算器)、`^N[0-9]{1,8}`(农历);default 级的 email/url/underscore 保证输入网址不上屏。editor 定义 space=confirm、Return=上屏原文、Ctrl+BackSpace=删一个音节(back_syllable)等。

---

## 4. Lua 扩展全清单(lua/,共 2,705 行,23 个脚本)

新版 librime-lua 模块方式引入(`lua_translator@*xxx`,无 rime.lua)。**候选变换手法**基本三类:重排序(yield 顺序)、改写 comment/text(构造新 Candidate / to_shadow_candidate / get_genuine)、quality 提升。

| 文件 | 行数 | 功能与实现思路 |
|---|---|---|
| `select_character.lua` | 60 | **以词定字**(processor):`[`/`]` 取当前候选首字/末字直接上屏;支持长句、光标不在末尾等边界 |
| `corrector.lua` | 291 | **错音错字提示**(filter):内置 ~250 条映射(错拼音→正确带调拼音,错字→正字);靠 spelling_hints 写进 comment 的全拼做 key,命中且 cand.text 相同则改写 comment |
| `pin_cand_filter.lua` | 380 | **置顶候选**(filter):配置 `编码<Tab>词1 词2`;纯重排序不造词;自动派生"末音节首字母"简码键(如 `da zhuang` 自动生成 `daz`);文件头是大篇幅使用说明书 |
| `long_word_filter.lua` | 52 | **长词优先**(filter):以第一个候选长度为基准,把更长的纯中文词提前 count 个到第 idx 位;只扫前 50 个候选,防性能退化 |
| `reduce_english_filter.lua` | 115 | **降英文**(filter):mode all(内置 ~500 个"长得像拼音"的 3~4 位英文词)/custom/none;把英文候选挪到第 idx 位之后 |
| `search.lua` | 306 | **辅码/部件反查**(filter,来自 mirtlecn):拼音+`` ` ``+部件拼音过滤候选;双通道查询——`dict_lookup`(经拼写投影,慢但支持双拼)与 reverse db(字→码反查,快);`search_single_char` 开关控制单字优先 |
| `date_translator.lua` | 146 | **日期时间**(translator):rq/sj/xq/dt/ts/rqzh/rqen 七个关键字,输出多格式日期、时段问候语、中英文日期;`cand.quality=100` 保证置顶 |
| `lunar.lua` + `lunar.db` | 85 | **农历+节气**(translator):1900-2100,`nl` 出今天,`N20240101` 出任意日;数据为香港天文台表(修正过 2051 偏移),符合 GB/T 33661-2017;template 可配置输出字段 |
| `unicode.lua` | 36 | **Unicode**(translator):`U62fc`→拼;限 0x10FFFF;还附赠后续 16 个码位候选 |
| `number_translator.lua` | 162 | **数字/金额大写**(translator):`R1234.56`→壹仟贰佰…/人民币大写(角分厘毫);源自 98 五笔 |
| `convert_ar_num_to_zh.lua` | 120 | 阿拉伯数字→中文(被 date_translator 复用) |
| `calc_translator.lua` | 224 | **计算器**(translator):`cC1+2` 出结果,暴露 math 函数(sin/cos/rdm/π/e),quality=99999 |
| `uuid.lua` | 46 | UUIDv4 生成 |
| `autocap_filter.lua` | 62 | **英文自动大写**(filter):输入 Hello→候选 Hello;HEllo→HELLO;构造新 Candidate 替换 text(注释:大写时无法动态调频) |
| `v_filter.lua` | 32 | v 开头两码时把单字符候选(ā á ǎ à)提到英文词前面 |
| `cn_en_spacer.lua` | 32 | 中英混合候选自动加空格(`VIP中P`→`VIP 中 P`),用 UTF-8 字节区间判断汉字 |
| `en_spacer.lua` | 18 | 英文后接英文自动补前导空格(to_shadow_candidate) |
| `is_in_user_dict.lua` | 29 | 按是否来自用户词典给 comment 加星号(可选挂载) |
| `force_gc.lua` | 9 | 每次调 `collectgarbage("step")` 防内存膨胀(librime-lua issue #307) |
| `t9_preedit.lua` | 18 | 九宫格把数字 preedit 还原成拼音显示 |
| `debuger.lua` | 12 | 显示 input/preedit 的调试滤镜 |
| `cold_word_drop/`(7 文件) | ~350 | 冷词降频套件(processor 记录选择时间→filter 降频/隐藏),rime_ice 默认未挂载 |

---

## 5. 中英混输(melt_eng)

**机制**:`melt_eng.schema.yaml` 本可独立使用(源自 tumuyan/rime-melt),在主方案里被拆解为 `table_translator@melt_eng` 次翻译器:同一输入码段由**多个 translator 并行查询**,融合排序靠 `initial_quality` 数值比例:

```
custom_phrase 99 ≫ 拼音 1.2 > 英文 1.1 > 中英混合词 0.5
```

- 英文库 ~2.5 万词,`enable_sentence: false`(英文不造句)、`enable_user_dict: false`(不调频)、comment 清空。
- **拼写派生**(`algebra_common`,这套规则是混输的灵魂):英文词条派生出——全大写/首字母大写/前 2~10 位大写变体;数字→英文读音(`0→o/zero/O`)、数字→**中文拼音读音**(`2→er/liang`、`10000→1wan`)、符号→读音(`+→plus/jia`、`#→hash/number/sharp/jing`);抹掉 `._-@` 等符号;erase 单字母和非常规开头。
- **双拼适配**:数字读音再派生为对应双拼码(`algebra_double_pinyin_flypy` 里 `0→lk`、`3→sj`),配合 8 份预生成的 `cn_en_<方案>.txt`(如 `X光 Xgl`)。切双拼时用户只需打 plum 配方自动写 `melt_eng.custom.yaml` 补丁切换 `__include`。
- **排序后处理**:输入 3~4 位"像拼音"的编码时英文天然靠前(reduce_english_filter 手工维护 ~500 词降位表 + 博文说明 dvel.me/posts/make-rime-en-better);v_filter 处理 `va` 这类 v 前缀两码的单字符优先。

---

## 6. 符号与特殊输入

- **symbols_v.yaml**(31KB):把 Rime 传统的 `/` 模式改成 **v 模式**(全拼无 v 声母,不冲突),共 **71 个类别**:字母变体(`va`→`ā á ǎ à ȁ â…`,**韵母类按一二三四声+其他排序**)、数字全家桶(大写汉字、上下标、圈/弧/点数字、罗马数字、苏州码)、符号/键盘/点号/夹注、象棋麻将色子扑克、天气音乐两性、八卦/64卦/太玄经、星座、几何箭头数学分数、希腊俄语字母、月份日期时间、天干地支节气、单位货币、偏旁部首笔画、拼音/注音/声调、假名/韩文等。`vhelp`/`vlist` 是自描述索引。`symbols_caps_v.yaml` 是同内容的 **V 大写版**(双拼中 `v+字母` 是正常拼写所以换大写前缀)。
- **数字/金额大写**:`R` 前缀(number_translator),小键盘数字经 key_binder 映射也可用。
- **农历**:`nl` 今日 + `N+YYYYMMDD` 任意日,含 24 节气,数据来自 lunar.db(香港天文台表,2 万+ 记录)。
- **Emoji**:opencc/emoji.txt 4,857 行**手工维护**的"词条→emoji"映射(如`芭蕾→🧑‍🩰 🕺`),经 emoji.json(mmseg 分词 + conversion_chain)挂到 `simplifier@emoji`;构建脚本从 others/emoji-map.txt 生成并校验。
- **词汇别名**:opencc/others.txt 1,498 行,如`一月→Jan. January`、偏旁部首俗称(`区字框→匚`)——同一 simplifier 通道输出。
- **标点**:default.yaml punctuator 全套映射 + `digit_separators: ",.:"`(数字后标点直通,librime≥1.13)。

---

## 7. 双拼方案(7 个 schema)

- 文件:自然码/ABC/微软/搜狗/**小鹤**/紫光/加加,各 ~16KB,**90% 内容逐字相同**(同一 engine/translators/filters、同一批 Lua、同一 rime_ice 词库)。实测 diff(自然码 vs 小鹤)差异仅 5 处:schema_id/name、`prism:` 名字、speller/algebra 映射规则、`user_dict: cn_en_<方案>`、preedit_format。
- **prism 机制**:7 个方案共用一个词库但各自编译 `prism: double_pinyin_<x>`,避免拼写派生结果互相覆盖——这是"一份词库多套编码"的关键配置。
- **双拼映射实现**(以小鹤为例,speller/algebra):用**带圈字母做占位符**完成两段式改写——先把韵母 xform 成 `ⓆⓌⒺ…`(如 `iu→Ⓠ`、`(. )ei→$1Ⓦ`、`^sh→Ⓤ`、`ing|uai→Ⓚ`),最后一条 `xlit/ⓆⓌ…Ⓜ/qwrtyuiopsdfghjklzxcvbnm/` 一次性还原为普通字母。零声母处理:`^([aoe])([ioun])→$1$1$2`(a→aa、an→an 双写)。**整个双拼映射就是十来条 xform + 一条 xlit,不含词库变化**。
- 双拼版差异细节:双拼默认**不开简拼**(pin_cand 注释);`v_filter` 与 `long_word_filter` 在双拼中未挂载(实测 flypy filters 列表无此两项);Lua 触发码改为英文词(date/lunar/uuid 而非 rq/nl);`others/no_lua_schema/` 提供去除全部 Lua 的版本供不支持 librime-lua 的环境使用。

---

## 8. 配置工程实践

- **default.yaml**(14.8KB):config_version、schema_list(9 方案)、menu/page_size、switcher(F4/Ctrl+grave;`save_options` 记忆开关)、ascii_composer(Caps=clear、左Shift=commit_code、右Shift=noop——每个选项 7 种行为全部注释)、navigator(Shift+方向按音节跳转)、punctuator 全套、recognizer 通用 patterns、key_binder(以词定字 `[ ]`、Tab/Alt+方向 音节移动、`- =` 翻页、Ctrl+Shift+3/4 切中英标点/简繁、**小键盘全映射**)。注释密度极高,相当于配置教程。
- **custom.yaml 补丁机制**:标准 Rime patch 体系(`patch:`、`__include`、`__append`、`@n` 下标替换、`+` 追加)。仓库自带范例 `others/patch_examples/`:双拼用户写 `melt_eng.custom.yaml` 把 `speller/algebra` 用 `__include: melt_eng.schema.yaml:/algebra_double_pinyin_flypy` 指向方案内置的具名 algebra 块——**把"双拼适配"做成方案内锚点,补丁只做指针切换**。
- **recipe.yaml(plum/东风引配方)**:根目录一份 `Rx: all` + install_files 清单,供 `bash rime-install iDvel/rime-ice` 一键安装;`others/recipes/` 另有 9 份细分配方:`config`(参数化 `schema=${schema}`,自动写三个 custom.yaml 切双拼)、`grammar`(下载万象语法模型 wanxiang-lts-zh-hans.gram 并 patch `grammar:` 段——collocation_max_length 6、collocation_penalty -14、max_homophones 8,即八股文/octagram n-gram 语言模型接入)、`reverse_tone`(下载 kMandarin.reverse.bin 给拆字反查加声调)、按需安装词库子集的 4 份。
- **opencc 目录**:emoji.txt + others.txt + emoji.json(见第 6 节)。
- **构建与测试管线**(超出一般 Rime 仓库之处):`others/script/` 是 Go 工具链(main.go 编排:Emoji 生成校验、pypinyin 风格注音校验 pinyin.go、多音字校验 polyphone.go、cn_en.go 生成 8 份混输词库、排序去重 sort.go;辅助数据 `汉字拼音映射.txt`/`错别字.txt`/`需要注音.txt`);`make build`(≥90s,自动排序去重+校验)、`make lint`(yamllint+luacheck);`others/script/smoke/` 是基于 `rime_deployer`+`rime_api_console` 的**端到端冒烟测试**——TSV 数据驱动用例(`case_id/schema/key_sequence/expected_text`),支持 `@today:<格式>` 与 `@regex:` 断言。
- **AGENTS.md**:为 AI 协作维护写的硬约束(词频默认 100;两字词必须进 base;多音词条必须进 ext;tencent 禁止多音字;Conventional Commits 带 `dict(cn)`/`lua` 作用域)。

---

## 总结回答两个核心问题

### Q1:2024-2026 现代词库比传统词库多了什么?

1. **词表覆盖**:不再是一份大杂烩码表,而是**分层字表+分层词库**(8105 精校字频字表 / 41448 Unihan 大字表可选拉满 / base 真频两字词 / ext+tencent 海量 3+ 字词压平频),~190 万条;外加 20k 英文、1k 中英混合词、拆字码表 13 万条。字形规范(《通用规范汉字表》)、注音规范(《现汉》逐条校)、异形词校对(有争议清单存档)。
2. **词频质量**:三层混合策略——核心短词用真实语料频(含腾讯词向量/清华语料/华宇),长尾词统一 100,字表多音字频比被**征用为自动注音的开关阈值(5:100)**。词频不再只是排序数字,还是注音正确性的控制变量。
3. **排序策略**:引擎层(initial_quality 四级比例、enable_word_completion、max_sentences、可选 octagram 语法模型 + contextual_suggestions)+ 数据层(手工顶置权重、v/u 派生、超级简拼)+ 滤镜层(pin_cand 置顶、long_word 长词优先、reduce_english 降英文、autocap)。同音候选顺序是三层合力而非单纯词频。
4. **辅助功能**:错音错字提示、辅码筛选、拆字反查、以词定字、日期/农历/金额大写/计算器/UUID/Unicode、emoji、简繁、中英混输、自动大写、v 模式符号 71 类、九宫格、7 种双拼共用一份词库。

### Q2:哪些必须引擎支持,哪些只是数据+滤镜?

**必须引擎/librime 支持(滤镜做不了)**:
- 拼写派生 algebra(derive/abbrev/xform/xlit)与 prism 编译——模糊音、简拼、自动纠错、双拼映射全靠 librime 的 spelling 层;
- script_translator 的组句(条件熵/用户词调频)、enable_word_completion、max_sentences;
- 八股文语法模型(octagram 插件)——长句 top1 的真正大杀器;
- table_translator 的 stabledb/user_dict 机制(自定义短语、cn_en);
- simplifier(OpenCC 通道)——emoji/简繁是引擎组件读数据;
- recognizer patterns(前缀 tag 路由)、affix_segmentor(反查 tag)、spelling_hints(comment 携带全拼)。

**纯数据或后处理滤镜即可(rspinyin 可低成本借鉴)**:
- v 模式符号、金额大写、农历、日期、计算器、Unicode、UUID:全是**触发前缀+查表/计算**;
- 置顶词(pin_cand)、长词优先、降英文、英文自动大写、中英自动空格、错音错字提示:全部是**对已生成候选序列的重排/改 comment**,rspinyin 完全可以在输出管线后挂同样的 pass;
- 中英混输候选融合:本质上只是多查询源 + 质量系数排序;
- 多音字自动注音(5:100 规则)虽然是 librime 行为,但规则本身简单,rspinyin 的 mmap 词库编译期可以直接实现同样逻辑(甚至做得更好——编译期决定每词条读音而非部署期)。

**对 rspinyin 的直接启示**:当前 35 万词/jieba 词表 vs 雾凇 190 万条分层词库,最划算的升级路径是——(a) 引入分层词库结构(核心真频 + 长尾平权),(b) 字表字频比驱动的多音字消歧,(c) 把"置顶/长词优先/纠错 derive"做成编译期或后处理滤镜而非引擎特性,(d) 错音错字提示只需一张 ~250 行的映射表 + 候选 comment 通道。
