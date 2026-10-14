# fcitx5-rime 深度阅读报告(面向 rspinyin 自绘 UI 插件的借鉴)

> 来源: 2026-10-14 四路并行调研之子代理原始报告,全文存档未删改 ｜ 调研对象: `remarks/fcitx5-rime`(版本 5.1.17) ｜ 交叉对比结论见 [rime-comparison.md](rime-comparison.md) ｜ 行号引用以调研当日工作区为准

我已经通读了 fcitx5-rime 的全部源码(2672 行,11 个源文件 + 4 个构建/配置文件),并核对了本地 librime 头文件中的 RIME_STRUCT 宏与 fcitx5 5.1.7 源码中 SharedLibraryLoader 的 `export:` 语义。以下是结构化报告。

# fcitx5-rime 深度阅读报告(面向 rspinyin 自绘 UI 插件的借鉴)

仓库:/home/gong/news/rspinyin/remarks/fcitx5-rime(版本 5.1.17,要求 fcitx5 >= 5.1.22,librime >= 1.9.0)
源码:/home/gong/news/rspinyin/remarks/fcitx5-rime/src/ 下共 6 个 cpp + 7 个 h,总计约 2700 行——一个"现代桌面桥接层"的全部复杂度就这么多,非常值得作为架构参照。

**总体架构一句话**:fcitx5-rime 完全不自绘任何 UI。它把 librime 的状态(commit/context/status/option)翻译成 fcitx5 的平台抽象(InputPanel / CandidateList 接口对象 / Action / StatusArea / 通知),渲染 100% 委托 classic-ui。文件职责:rimeengine(引擎+生命周期+菜单+通知)、rimestate(每 IC 状态+按键+preedit+候选刷新)、rimesession(librime session 池)、rimecandidate(候选列表适配器)、rimeaction(option 联动的 Action)、rimeservice(DBus 接口)、rimefactory(插件入口)。

---

## 1. 插件生命周期与引擎管理

**插件入口与加载方式**
- `src/rimefactory.cpp:13-20`:`RimeEngineFactory::create()` 里先 `registerDomain("fcitx5-rime", FCITX_INSTALL_LOCALEDIR)` 注册 gettext 域,再 `new RimeEngine(manager->instance())`;`FCITX_ADDON_FACTORY_V2(rime, ...)` 导出工厂符号。
- `src/rime-addon.conf.in.in:8`:`OnDemand=True`——插件按需加载,只有用户切到 rime 输入法时才 dlopen。`src/rime-addon.conf.in.in:11-12` 用 `[Addon/Dependencies] 0=core:@REQUIRED_FCITX_VERSION@` 声明对 fcitx5 core 的**带版本约束依赖**(fcitx5 会用 SemanticVersion 解析,CMakeLists.txt:5 定为 5.1.22),这是对宿主 ABI 兼容的防线。
- `Library=export:librime`(`rime-addon.conf.in.in:6`)是个关键细节:fcitx5 的 SharedLibraryLoader 见到 `export:` 前缀时会剥掉前缀得到 `librime.so`,并以 `LibraryLoadHint::ExportExternalSymbolsHint`(RTLD_GLOBAL 语义)dlopen——**把 librime 的符号导出为全局可见**,否则 librime 自己再加载的 rime 插件(lua 等外部模块)解析 librime API 符号时会失败。这是所有"包装第三方引擎库"的 fcitx5 插件都必须处理的坑。

**RimeApi 获取与初始化**
- `src/rimeengine.cpp:116-122`:`EnsureRimeApi()` = `rime_get_api()` + 失败即 `throw std::runtime_error`,异常由 fcitx5 addon loader 捕获并记日志,插件加载失败。
- 构造函数 `src/rimeengine.cpp:183-276`:成员初始化列表里完成 api 获取、`factory_`(`FactoryFor<RimeState>`,lambda 直接 `new RimeState`)、`sessionPool_` 构造;shared data dir 在桌面端用编译期宏 `RIME_DATA_DIR`,Android/Apple 上改为 StandardPaths 定位 `rime-data/default.yaml` 后取其目录(`rimeengine.cpp:187-197`)。
- `rimeStart(bool fullcheck)`(`rimeengine.cpp:287-344`)是唯一初始化路径:
  - 确保用户目录存在:`StandardPaths::global().userDirectory(StandardPathsType::PkgData) / "rime"`(即 `$XDG_DATA_HOME/fcitx5/rime`,`rimeengine.cpp:290-298`);
  - 填 `RimeTraits`(用 `RIME_STRUCT` 宏零初始化并写 data_size,见第 8 节):shared_data_dir、`app_name="rime.fcitx-rime"`、user_data_dir、发行版信息、`log_dir=""`(强制 librime 只写 stderr,`rimeengine.cpp:308-309`),并把 fcitx5 日志级别映射到 librime 的 min_log_level(`rimeengine.cpp:310-327`,Debug 时给 0 还注明"Rime info is too noisy");
  - `firstRun_` 是 **static**(`rimeengine.cpp:181`):只有进程内第一次调用 `api_->setup()`,之后 `initialize` + `set_notification_handler` + `start_maintenance(fullcheck)`(`rimeengine.cpp:331-341`);
  - 启动后若不在 maintenance 模式立即 `updateAppOptions()`(读 `fcitx5` 配置里的 app_options),否则置 `needRefreshAppOption_` 延迟到部署成功通知后补做(`rimeengine.cpp:339-343`、`rimeengine.cpp:571-577`)。

**deploy 流程与"需要重新部署"的检测**
- 插件自己**不做任何时间戳/版本扫描**——增量检测完全委托 librime 的 maintenance 机制:正常启动走 `rimeStart(false)`(librime 内部判断部署产物是否过期),手动部署走 `deploy()`(`rimeengine.cpp:668-674`)= `releaseAllSession(true)`(先快照)→ `api_->finalize()` → `rimeStart(true)`(fullcheck 全量)。
- deploy 的四个触发入口:① StatusArea 菜单按钮 `deployAction_`(`rimeengine.cpp:206-216`);② 可配置热键,在 keyEvent 顶部拦截且仅 press 生效(`rimeengine.cpp:491-495`);③ 配置工具走 `setSubConfig("deploy")` 虚路径(`rimeengine.cpp:362-369`);④ sync 类似(`rimeengine.cpp:676-683`,`save()` 时也调 `sync(false)`,`rimeengine.cpp:525`)。
- 插件自己的 rime 侧配置 `fcitx5.yaml` 有独立的版本联动:maintenance 成功通知里调 `api_->deploy_config_file("fcitx5.yaml", "config_version")`——librime 比较 config_version 决定是否重新部署这份文件,然后重读 app_options(`rimeengine.cpp:571-577`;yaml 里 `config_version: "0.22"`,`data/fcitx5.yaml:4`)。
- 配置变更路径 `updateConfig()`(`rimeengine.cpp:371-393`):释放全部会话(带快照)→ `finalize` → `rimeStart(false)` → `registerProperty("rimeState")` → 重建 schema 菜单 → 刷新会话共享策略 → 更新热键 → 刷新 StatusArea。析构则 `factory_.unregister()` + try/catch 包住的 `api_->finalize()`(`rimeengine.cpp:278-285`)。

## 2. 会话管理

这是我认为对 rspinyin 最有参考价值的一块,它用一个**三层结构**解决了"每个 InputContext 映射到 librime session,且共享粒度可配置"的问题:

- **每 IC 状态对象** `RimeState`:`InputContextProperty` 子类(`src/rimestate.h:28`),通过 `FactoryFor<RimeState>` 注册为名为 `"rimeState"` 的属性(`rimeengine.cpp:383`),取用即 `ic->propertyFor(&factory_)`(`rimeengine.cpp:609-614`)。IC 的创建/销毁由 fcitx5 的 InputContextManager 托管,插件零生命周期代码。
- **会话持有者** `RimeSessionHolder`(`src/rimesession.cpp:25-58`):构造时 `api->create_session()`(失败抛异常),`set_property(id_, "client_app", program)` 把程序名同步给 rime,并按 `fcitx5.yaml` 的 `app_options` 给该程序注入初始 option(如某些程序默认英文,`rimesession.cpp:40-48`);析构时 `destroy_session` 并从池中注销。
- **会话池** `RimeSessionPool`(`src/rimesession.cpp:104-134`):键由 `PropertyPropagatePolicy` 决定——`No`→`"u:"+IC的uuid`(每 IC 独立)、`Program`→`"p:"+程序名`(同程序共享)、`All`→`"g:"`(全局一个),存 `weak_ptr`,**没有 IC 引用时会话自动销毁**。策略来源:配置项 `sharedStatePolicy`(All/Program/No/FollowGlobalConfig,`rimeengine.h:85-87`),FollowGlobalConfig 时直接读 fcitx5 全局配置的 `shareInputState`(`rimeengine.cpp:770-782`);全局配置重载事件 `EventType::GlobalConfigReloaded` 会触发策略刷新,策略变化时释放全部会话重建(`rimeengine.cpp:233-235, 762-768`)。
- **懒创建与快照恢复**:`RimeState::session(bool requestNewSession)`(`rimestate.cpp:50-67`)在首次需要时才向池请求;池返回 `{holder, isNewSession}`——**复用了旧会话**(同程序共享)就清掉本 state 保存的快照,拿到全新会话才 `restore()`。`snapshot()/restore()`(`rimestate.cpp:475-527`)保存/恢复 schema_id + 当前 option 集合(option 以 `"name"`=true / `"!name"`=false 字符串编码),用于 deploy、配置切换、策略切换时保持用户选的简繁/方案状态。程序名同步是惰性的:只有策略为 All 时才需要在每次按键前 `maybeSyncProgramNameToSession()`(因为 All 策略下同一个全局会话会被不同程序轮流用,`rimestate.cpp:529-540`)。
- **候选词/预编辑不落地存储**:它们只存在于 librime 的 session 里,每次 `updateUI` 现取现用(`get_context` 后立刻 `free_context`,`rimestate.cpp:409-423`);fcitx5 侧只保留 InputPanel 里的 Text 与 CandidateList 快照对象。插件本地 state 只存 `lastMode_`、`changedOptions_`、快照字段(`rimestate.h:64-71`)。
- 批量释放:`releaseAllSession(snapshot)` 用 `inputContextManager().foreach` 遍历所有 IC(`rimeengine.cpp:656-666`);焦点切换本身不销毁会话(会话跟随 IC 属性),reset 时只 `clear_composition`。

## 3. 按键处理流程

入口 `RimeEngine::keyEvent`(`rimeengine.cpp:486-507`):
- **引擎级预拦截只有两个**:deploy / synchronize 热键,且 `!event.isRelease()` 才检查,命中即 `event.filterAndAccept()`。除此之外**没有任何键在插件层被白名单拦截**。
- 其余全部转给 `RimeState::keyEvent`(`rimestate.cpp:164-244`),核心流程:

1. **Compose/死键处理**:非 release 且无 Ctrl/Super 时调 `instance()->processComposeString(&ic_, sym)`(`rimestate.cpp:170-180`)——返回 nullopt 表示组合序列未完成,直接 `filterAndAccept()` 吞掉返回;返回非空字符串则得到组合结果字符(如 `´+e→é`)。
2. **维护模式与无会话直通**:`api->is_maintenance_mode()` 或 session 为 0 时直接 return——**不调用 filterAndAccept,事件自然变成 PassThrough** 落回应用(`rimestate.cpp:183-189`)。"rime 不消费 = 穿透"的语义就是"不 accept",没有显式 KeyState::PassThrough 分支。
3. **修饰键掩码裁剪**(`rimestate.cpp:195-206`):`rawKey().states()` 与 `{Mod1(Alt), CapsLock, Shift, Ctrl, Super}` 求交——Hyper/NumLock/Mod5 等全部丢弃;若含 Super 再补上 `Super2`(注释:IBus 用 virtual super mask);`isRelease()` 时置 **bit 30(IBUS_RELEASE_MASK)** 传给 `process_key`。
4. **keysym 直传**:`fcitx5` 的 Key 就是 XKB keysym 的封装,**无需转换**,直接 `api->process_key(session, event.rawKey().sym(), intStates)`(`rimestate.cpp:224-225`);compose 结果字符在长度为 1 时用 `Key::keySymFromUnicode(c)` 反查 keysym 再喂(`rimestate.cpp:211-216`),喂不进去则把字符直接 commit + `clear()`。
5. **取回提交**:`RIME_STRUCT(RimeCommit, commit)` + `get_commit` → `ic->commitString(commit.text)` + `resetCompose`(`rimestate.cpp:231-236`)。**每个按键后都拉一次 commit**,不缓存。
6. **option 变化提示**:`updateUI` 后若非 release、schema 未变、inputPanel 为空且本次按键期间 rime 发了 option 变更通知,则把变化以"半角|简体"样式显示为输入法提示信息(`rimestate.cpp:239-243`,实现 `showChangedOptions` `rimestate.cpp:545-616`,跳过 `_` 开头的内部 option,ascii_mode 走 StatusArea 刷新)。
- 数字选词/翻页**不经过插件**:数字键、Page_Up/Down、Shift 等全部作为普通 key event 交给 rime 自己解释;插件只在 UI 需要翻页时反向利用这一点——候选列表的 `prev()/next()` 直接合成 `Key(FcitxKey_Page_Up/Page_Down)` 的 KeyEvent 再喂回 `state->keyEvent()`(`rimecandidate.cpp:75-86`),复用 rime 的按键语义,零翻页逻辑。
- CapsLock 有专门通路:监听 `Instance::XkbStateMaskChanged` 信号,比较前后 locked mask 中的 CapsLock,变化时对对应 display 的焦点 IC 刷新提示与 StatusArea(`rimeengine.cpp:240-275`);`isCapsLockOn` 读 `instance_->xkbStateMask()` 的 locked 段(`rimeengine.cpp:784-790`),用于 `fcitx_rime_latin_upper` 图标切换(`rimeengine.cpp:645-647`)。

## 4. 预编辑与候选词上屏

**preedit 三种模式**(`rimeengine.h:63-66` 枚举 No/ComposingText/CommitPreview;默认桌面 ComposingText,Android No,`rimeengine.h:82-84`):
- 模式选择前提是 IC 能力位:`ic->capabilityFlags().test(CapabilityFlag::Preedit)`,不支持客户端 preedit 的应用自动落回 `PreeditMode::No`(`rimestate.cpp:349-352`)——**"组合窗口 vs 客户端 preedit"的选择就是这一个 capability 检查 + 用户配置**。
- `preeditFromRimeContext`(`rimestate.cpp:302-347`)把 rime composition 三段化:sel_start 之前(已转换)、sel_start..sel_end(高亮段,flag|highlightFlag)、sel_end 之后(待转换),并 `setCursor(cursor_pos)`,先做 sel_start<=sel_end<=length 的合法性校验。
- `updatePreedit`(`rimestate.cpp:349-390`):No 模式全放 `inputPanel().setPreedit()`(aux 区,由候选窗口绘制)、clientPreedit 置空;CommitPreview 模式 client 侧显示 `commit_text_preview`(下划线);ComposingText 模式 client 侧显示完整 composition(下划线+可高亮)。`preeditCursorPositionAtBeginning` 配置(默认桌面开)把 client preedit 光标钉在开头(`rimeengine.h:88-94` 注释解释了原因:Linux 只有光标位置可用)。

**候选列表填充**(rimestate.cpp:416-421 → src/rimecandidate.*):
- `context.menu.num_candidates` 非零就 `inputPanel().setCandidateList(std::make_unique<RimeCandidateList>(...))`,否则 setCandidateList(nullptr)。每个 key event 都**新建整个 CandidateList 对象**,旧的自动析构——无差量更新,实现极简。
- `RimeCandidateList` 同时实现四个接口 mixin(`rimecandidate.h:45-52`):`PageableCandidateList`(hasPrev=`page_no!=0`、hasNext=`!is_last_page`)、`BulkCandidateList`(candidateFromAll/totalSize)、`ActionableCandidateList`、`BulkCursorCandidateList`——**是否用 bulk cursor 由编译期决定**:librime < 1.10 时 CMake 定义 `FCITX_RIME_NO_HIGHLIGHT_CANDIDATE` 禁用(CMakeLists.txt:25-27)。
- 标签(label)三级回退(`rimecandidate.cpp:70-85`):优先 rime 的 `select_labels`(用 `RIME_STRUCT_HAS_MEMBER` 运行时探测该成员是否存在)、其次 `select_keys` 字符、最后 `(i+1)%10` 纯数字;光标取 `menu.highlighted_candidate_index`。
- **数字选词的映射**:选中动作 `RimeCandidateWord::select` → `state->selectCandidate(idx, global=false)` → `select_candidate_on_current_page`(`rimestate.cpp:246-267`)。鼠标点击/触屏走这条路;数字键选词则完全由 rime 自己处理(见第 3 节)。label 只是显示。
- bulk 部分很讲究:`totalSize()` 返回 -1(未知总数),`candidateFromAll(idx)` 用 librime 的迭代器 API `candidate_list_from_index/next/end` **惰性拉取全局第 idx 个候选并缓存**,首次越界时把 `maxSize_` 收缩为已见最大索引作为学习到的上界(`rimecandidate.cpp:96-134`)。这支撑 classic-ui 的"整页滚动+长列表"渲染。
- `globalCursorIndex()` 返回 -1(注释 "No API available"),`setGlobalCursorIndex` 调 `api->highlight_candidate`(`rimecandidate.cpp:168-181`)——高亮跟随(UI 高亮移动反向写回 rime)。
- **候选附加动作**:ActionableCandidateList 提供"删词"(Forget word)动作(`rimecandidate.cpp:138-166`),经 `delete_candidate(_on_current_page)` 实现——这也是 classic-ui 右键菜单的数据来源。

**切输入法时的收尾**(`rimeengine.cpp:464-484`):`SwitchInputMethodBehavior` 四档——Clear / 提交原始输入(`get_input`)/ 提交 composition.preedit / 提交 `commit_text_preview`,然后统一 `reset()`(clear_composition + resetCompose + inputPanel().reset + updatePreedit/updateUserInterface,`rimeengine.cpp:509-518`)。

## 5. UI 与外观

- **完全不自绘**。全代码没有一行 X11/Wayland/Skia/canvas;它只做两件事:往 `InputPanel` 塞数据模型(preedit Text + CandidateList 接口对象),然后 `ic->updateUserInterface(UserInterfaceComponent::InputPanel / StatusArea)` 通知 UI 刷新(`rimestate.cpp:426-433`)。谁来渲染(全新窗口、kimpanel、虚拟键盘)它一概不知。
- 没有任何"candidate window 位置/虚拟键盘"配置透传给 rime——librime 的 `RimeContext` 只被读 composition/menu;位置、主题、字体全由 fcitx5 classic-ui 负责。唯二与外观沾边的:preedit 高亮 `TextFormatFlag::HighLight/Underline`(`rimestate.cpp:378-383`)与 `CandidateLayoutHint::NotSet`(布局建议留空,`rimecandidate.h:115`)。
- 图标只提供名字(`fcitx_rime_im/disabled/deploy/sync/latin/latin_upper`、`fcitx-rime`),实际 SVG/PNG 装进 hicolor 图标主题(`data/CMakeLists.txt:1-3`)——遵循系统图标规范而非自带窗口贴图。

## 6. 配置系统

**fcitx5 侧配置**(`rimeengine.h:80-119`,`FCITX_CONFIGURATION` 宏):
- `preeditMode`、`sharedStatePolicy`(InputState)、`preeditCursorPositionAtBeginning`、`switchInputMethodBehavior`、`userDataDir`(**ExternalOption**:值是 `xdg-open "..."` 命令,配置工具里点击即打开用户目录,`rimeengine.h:101-110`)、`deploy`/`synchronize` 热键(KeyList)、`latinModeNameFromSchema`。持久化:`readAsIni(config_, "conf/rime.conf")` / `safeSaveAsIni(config_, "conf/rime.conf")`(`rimeengine.cpp:357-360`、`rimeengine.h:138-142`),即 `~/.config/fcitx5/conf/rime.conf`;`setConfig` 里 load 后立即 `updateConfig()` 热生效(整 librime 重启)。

**与 rime option 机制的绑定**(简繁/中英/方案切换,`src/rimeaction.*`):
- 读取每个 schema 的 `switches/` 列表(`rimeengine.cpp:685-722`):两态且有 `name` 的 switch(如简繁 `simplification`)→ `ToggleAction`(排除 `ascii_mode`,因为 imAction_ 已覆盖中英切换);`options` 列表型 switch(如多方案的内嵌选项)→ `SelectAction`(一个 Action 挂 Menu,激活时把目标 option 置 true 其余置 false,`rimeaction.cpp:96-120`)。**标签直接来自 rime schema 的 `states/`,插件不硬编码任何"简体/繁体"文案**。
- 所有操作最终都是 `api->get_option/set_option(session, name)`;AsciiMode 特化为 `RIME_ASCII_MODE` 宏(`rimestate.h:22`)+ `toggleLatinMode/setLatinMode`(`rimestate.cpp:137-153`),维护模式下全部拒绝操作。
- **展示位置全靠 Action/StatusArea 平台机制**:
  - 主 Action `IMAction`(注册名 `fcitx-rime-im`,`rimeengine.cpp:198-201`)实现 `shortText/longText/icon(ic)` 三个虚函数按 IC 现查 RimeStatus(`rimeengine.cpp:126-179`):disabled 显示 "☿"、ascii 显示 "A"/"ABC"/"abc"、否则 schema 名(短标签取 schema_name 首个 UTF-8 字符,`rimestate.cpp:114-122`);它挂 `schemaMenu_`(每个 schema 一个 SimpleAction + Latin Mode + separator + Deploy + Sync,`rimeengine.cpp:724-760`)。
  - schema 相关的 option Actions 按**当前 schema** 动态加入 StatusArea 的 InputMethod 组(`refreshStatusArea(InputContext&)` `rimeengine.cpp:395-423`),入口处有防串扰守卫:非 rime 的 IC(如打密码时的 keyboard-us)直接 return,避免清掉别的输入法的状态区(`rimeengine.cpp:397-400`)。
  - 刷新时机由 rime 通知驱动:`notify()` 里 messageType=="option" → `updateStatusArea`(只 `updateUserInterface(StatusArea)` 让 UI 重新拉值)、=="schema" → `refreshStatusArea`(`rimeengine.cpp:587-592`)。
  - option 变化的气泡提示见第 3 节第 6 步;UI 值的拉取是**拉模型**:Action 的 shortText/icon 在 UI 每次重绘时被调用,插件不主动推送具体值。

## 7. 数据目录与部署

- **插件自身配置**:`~/.config/fcitx5/conf/rime.conf`,结构即 `RimeEngineConfig`(第 6 节);模板 `src/rime.conf.in` 是输入法条目声明(Name=Rime/Icon/Label=ㄓ/Addon=rime/Configurable=True),`rime-addon.conf.in.in` 是 addon 条目,两个都经 `fcitx5_translate_desktop_file` 做翻译后安装到 `inputmethod/` 与 `addon/`(`src/CMakeLists.txt:29-33`)。
- **用户数据目录**:`$XDG_DATA_HOME/fcitx5/rime`(`StandardPathsType::PkgData / "rime"`,`rimeengine.cpp:290-292`),不存在则 `fs::makePath`;共享数据目录用 `RIME_DATA_DIR` 宏(桌面)/StandardPaths 探测(移动端)。注意 `rime.conf`(fcitx5 插件配置)与 rime 用户目录(librime 的)是两个世界,前者归 fcitx-config,后者归 librime。
- **firstRun 逻辑**:static `firstRun_`(`rimeengine.cpp:181`)保证 `api_->setup` 每进程一次;真正的"要不要部署"判断在 librime 内部(`start_maintenance(fullcheck)` + `is_maintenance_mode()`)。
- **app_options**(`data/fcitx5.yaml` + `rimeengine.cpp:65-87, 346-355`):`fcitx5.yaml` 装进 RIME_DATA_DIR(`data/CMakeLists.txt:5`),从而能被 `config_open("fcitx5")` 打开;`app_options/<程序名>/<option>: bool` 在会话创建时按程序名应用(`rimesession.cpp:40-48`),等效于"per-app 简繁/中英默认值"。yaml 的更新靠 `deploy_config_file("fcitx5.yaml", "config_version")` 版本比较(`rimeengine.cpp:573`)。
- **DBus**:`org.fcitx.Fcitx.Rime1` @ `/rime`(`rimeservice.cpp:20`),仅 5 个方法:`SetAsciiMode(b)`、`IsAsciiMode→b`、`SetSchema(s)`、`GetCurrentSchema→s`、`ListAllSchemas→as`(`rimeservice.h:30-34`)。目标 IC 用 `instance()->mostRecentInputContext()`(`rimeservice.cpp:23-29`)。**没有 Deploy/Sync 的 DBus 入口**——外部触发部署走的是 fcitx5 configtool 的 `setSubConfig("deploy"/"sync")` 机制(第 1 节)。DBus 支持是可选的:找不到 Fcitx5ModuleDBus 就 `-DFCITX_RIME_NO_DBUS` 裁掉(`src/CMakeLists.txt:18-24`);notifications 模块同理是 OptionalDependency(`rime-addon.conf.in.in:14-16`,`notifications()` 为空时静默跳过提示,`rimeengine.cpp:594-599`)。

## 8. 值得注意的工程细节(rspinyin 最该抄的部分)

1. **跨线程通知编组**:librime 的 notification 回调可能在任意线程;`rimeNotificationHandler` 用 `eventDispatcher_.schedule(lambda)` 把处理投回主循环,并用 `mainThreadId_` 判断"已在主线程"时走 `notifyImmediately` 同步路径——后者专门用于在 `process_key` 调用栈内**同步记录本次按键导致的 option 变化**(否则 option 提示会晚一个事件)(`rimeengine.cpp:527-554`)。rspinyin 的 Slint 渲染线程与 fcitx5 事件循环之间的编组可直接套这个 EventDispatcher 模式。
2. **ABI 版本兼容三板斧**:① 构建期 pkg-config 版本比较生成 `-DFCITX_RIME_NO_HIGHLIGHT_CANDIDATE`(CMakeLists.txt:25-27);② `RIME_STRUCT` 宏按 librime 的 `data_size` 机制初始化结构体(本地 librime `src/rime_api.h:60-71`:`RIME_STRUCT_INIT` 写 `sizeof(Type)-sizeof(data_size)`,`RIME_STRUCT_HAS_MEMBER` 用指针偏移运行时探测新成员,代码里用于 `select_labels`,`rimecandidate.cpp:71`);③ 对 fcitx5 core 本身用带版本依赖 `core:5.1.22` + `find_package(Fcitx5Core 5.1.22 REQUIRED)`。
3. **日志**:自有 category `FCITX_DEFINE_LOG_CATEGORY(rime_log, "rime")` + `RIME_DEBUG/RIME_ERROR` 宏(`rimeengine.cpp:56`、`rimeengine.h:236-239`);并且把 fcitx5 日志级别映射为 librime min_log_level、`log_dir=""` 逼 librime 写 stderr,使 `fcitx5 -d *rime=5` 一套开关控制两层日志(`rimeengine.cpp:307-327`)。
4. **错误处理风格**:启动失败抛异常让 addon loader 统一善后;`create_session` 失败抛异常但被池的 `catch(...)` 吞掉转为"无会话降级"(按键全部穿透,`rimesession.cpp:126-133`);`finalize` 在析构/重配时都包 try/catch;候选索引越界抛 `std::invalid_argument`(fcitx CandidateList 的接口契约)。
5. **通知节流**:`allowNotification(type)` 开一个 60 秒窗口只放行指定类型;deploy 成功/失败后 `silenceNotificationUntil_` 静默 30 秒防轰炸(`rimeengine.cpp:62-63, 520-523, 594-607`)。
6. **updateUI 的细节防御**:key release 时若 inputPanel 非空则跳过 `inputPanel().reset()`,避免把 aux 消息(如 option 提示)闪没(`rimestate.cpp:393-397`);`find_session` 先验会话仍存活;`get_context` 失败即整块跳过。
7. **对"自绘 UI 插件"的核心启示(平台换来的免费能力清单)**:fcitx5-rime 之所以只有 2700 行,是因为它把这些全部外包给了平台——IC 生命周期与多路复用(InputContextProperty/Factory)、会话共享粒度(PropertyPropagatePolicy + 全局配置联动)、compose/dead key(instance()->processComposeString/resetCompose)、preedit 能力协商(CapabilityFlag::Preedit)、渲染与定位(InputPanel + updateUserInterface)、菜单/状态区/托盘(Action/Menu/StatusArea/registerAction 按名注册)、桌面通知(notifications 模块)、DBus(FCITX_OBJECT_VTABLE_METHOD 一行一个方法)、i18n(registerDomain + _())、图标主题(hicolor 安装)、配置持久化(readAsIni/safeSaveAsIni)。**rspinyin 自绘候选窗口意味着你把 InputPanel 的"渲染消费者"换成自己,但第 1-6 项平台服务仍应照单全收**;尤其 rimecandidate.cpp 里 CandidateList 接口 mixin 的分层(Pageable/Bulk/BulkCursor/Actionable 各自独立声明能力)是值得在你的 Rust 候选数据模型中映射的接口切分方式,即便 UI 完全自绘——它使同一份数据模型未来可以同时喂给自己的 Slint 窗口和 kimpanel/虚拟键盘。
8. **一个 rspinyin 必然不同、但应三思的差异点**:fcitx5-rime 每个 key event 都完整重建 CandidateList 对象并现拉 context("无状态视图")。Slint 自绘窗口如果做差量刷新(只更新变化的候选),需要自己维护 dirty 判定——这是放弃"每帧全量重建"这一平台惯用法后要自己偿还的复杂度。
