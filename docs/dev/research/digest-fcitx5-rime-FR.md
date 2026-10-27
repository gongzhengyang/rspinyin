# 勘察摘要：fcitx5-rime（对标簇 SCOUT-FR）

> 对标仓库: `remarks/fcitx5-rime`（origin https://github.com/fcitx/fcitx5-rime，HEAD `f504861` "bump version"）
> 规模: src/ 共 2,731 行 C++（rimeengine.cpp 792 / rimestate.cpp 617 / rimecandidate 308 / rimeaction 249 / rimesession 222 / rimeservice 128 / rimefactory 40），本次**全部文件逐行精读**。
> 定位: Fcitx5 官方 Rime 桥接插件——librime 引擎 + fcitx5 原生 UI（无自绘候选窗），与 rspinyin 同为 in-process InputMethodEngine addon，是"会话/生命周期/按键转发"维度的最直接同问题域参照。
> 线程模型总览: 单主线程持有全部 RimeApi 调用；librime 维护线程经 `rimeNotificationHandler`（rimeengine.cpp:527）回呼，同线程走同步路径、异线程经 `EventDispatcher::schedule`（rimeengine.cpp:202 attach / :536 schedule）序列化回主线程。零互斥锁、零后台线程自建（检索词 `mutex|lock_guard|std::thread` → 仅 `mainThreadId_` 声明，rimeengine.h:230）。

---

## 维度① 架构：factory、每 IC 会话、option 双向同步

**A1 addon 装配**。入口 `FCITX_ADDON_FACTORY_V2(rime, RimeEngineFactory)`（rimefactory.cpp:20）；`create()` 只做 i18n 域注册 + `new RimeEngine(manager->instance())`（rimefactory.cpp:13-16）。addon 元数据 `rime-addon.conf.in.in`：`OnDemand=True`、`Configurable=True`、可选依赖 notifications/dbus（:8-16），DBus 缺失时 CMake 退化为 `-DFCITX_RIME_NO_DBUS`（src/CMakeLists.txt:18-24）。构造函数（rimeengine.cpp:183-276）内完成：`EnsureRimeApi()` 失败即 throw（addon 加载失败路径，:116-122）、`FactoryFor<RimeState>` 工厂 lambda（:185）、4 个 Action 注册到 userInterfaceManager（:199-229）、`watchEvent(GlobalConfigReloaded)`（:233-235）、XkbStateMaskChanged 连接（:240-275）。析构顺序：先 `factory_.unregister()` 再 `api_->finalize()`（:278-285）——先断 IC 侧引用再拆引擎。

**A2 每 IC 会话：两层结构**。外层 `RimeState : InputContextProperty` 由 fcitx5 按 IC 自动构造/析构（rimeengine.cpp:185, :609-614 `state(ic) = ic->propertyFor(&factory_)`）；内层 `RimeSessionHolder` 持真正 `RimeSessionId`，由 `RimeSessionPool` 按**共享策略键**复用：

```
// rimesession.cpp:105-124（RimeSessionPool::requestSession）
    std::string key;
    switch (policy_) {
    case PropertyPropagatePolicy::No:
        key = uuidKey(ic);                    // "u:"+IC uuid → 每 IC 独占
        break;
    case PropertyPropagatePolicy::Program:
        if (!ic->program().empty()) {
            key = stringutils::concat("p:", ic->program());  // 同程序共享
        } else {
            key = uuidKey(ic);
        }
        break;
    case PropertyPropagatePolicy::All:
        key = "g:";                           // 全局唯一会话
        break;
    }
    auto iter = sessions_.find(key);
    if (iter != sessions_.end()) {
        return {iter->second.lock(), false};  // 池存 weak_ptr，不延长寿命
    }
```

不变量：**会话身份 = (策略, 键) → weak_ptr**；IC 属性持 shared_ptr 决定会话生死，池只做查找不做保活——最后一个 IC 释放，`~RimeSessionHolder` 才 `destroy_session` 并从池摘除（rimesession.cpp:51-58）。

**A3 懒创建 + 快照恢复**。RimeState 首次用到会话时才创建（按键/getStatus 时），且新建时回放快照、复用池中旧会话时丢弃快照——恢复恰好发生一次：

```
// rimestate.cpp:50-67（RimeState::session）
RimeSessionId RimeState::session(bool requestNewSession) {
    if (!session_ && requestNewSession) {
        auto [sessionHolder, isNewSession] =
            engine_->sessionPool().requestSession(&ic_);
        session_ = sessionHolder;
        if (isNewSession) {
            restore();                          // 新建 → 回放 schema+options
        } else {
            savedCurrentSchema_.clear();        // 复用 → 快照作废
            savedOptions_.clear();
        }
    }
    if (!session_) {
        return 0;
    }
    return session_->id();
}
```

不变量：**快照生命周期 = "会话被回收"到"下一个会话诞生"**；配置热重载（updateConfig → `releaseAllSession(true)` 先逐 IC `snapshot()` 再释放，rimeengine.cpp:371-393, :656-666）因此不丢用户当前 schema/开关。restore 前校验 schema 仍在已注册集合 `schemas_` 内（rimestate.cpp:515-517），防恢复到已删除方案。

**A4 option 双向同步（三条上行 + 两条下行）**：
- 下行（引擎→UI 状态区）：部署时从各 schema 的 `switches` 列表动态生成 ToggleAction/SelectAction（rimeengine.cpp:685-722 `updateActionsForSchema`，读 `states`/`options` 数组，2 态→Toggle、N 态→Select，`ascii_mode` 跳过因已有 Latin Mode 菜单项）。
- 下行（外部→引擎）：D-Bus `SetAsciiMode/SetSchema`（rimeservice.cpp:31-59）。
- 上行（引擎→UI 刷新）：librime 发 `option` 通知 → `notify()` → `updateStatusArea(session)`（rimeengine.cpp:587-588），只对 `foreachFocused` 且 IM==rime 的 IC 触发 `updateUserInterface(StatusArea)`（:438-453）。
- 上行（回收前）：`snapshotOption()` 把开关值编码为 `"option"` / `"!option"` 字符串（rimeaction.cpp:72-78）。
- 程序级初值：`fcitx5.yaml` 的 `app_options` 按程序名在会话创建时 `set_option` 逐项应用（rimesession.cpp:41-48；解析 rimeengine.cpp:65-87）。

## 维度② UI 交互：候选产出、preedit 三模式、主题边界、状态区动作

**B1 候选产出**：`RimeCandidateList` 一类实现 5 个接口——`CandidateList + ActionableCandidateList + PageableCandidateList + BulkCandidateList + BulkCursorCandidateList`（rimecandidate.h:45-52），每次 `updateUI` 从 `RimeContext.menu` 整体重建（rimestate.cpp:416-421）。标号优先级：`select_labels` > `select_keys` > `(i+1)%10`（rimecandidate.cpp:75-85）。高亮游标 `menu.highlighted_candidate_index → cursor_`（:90-92）。

**B2 翻页不持页态**——UI 层合成按键回流管线：

```
// rimecandidate.h:75-86（RimeCandidateList::prev/next）
    void prev() override {
        KeyEvent event(ic_, Key(FcitxKey_Page_Up));
        if (auto state = engine_->state(ic_)) {
            state->keyEvent(event);
        }
    }
    void next() override {
        KeyEvent event(ic_, Key(FcitxKey_Page_Down));
        if (auto state = engine_->state(ic_)) {
            state->keyEvent(event);
        }
    }
```

不变量：**页码事实只存在于引擎（Rime menu），UI 翻页=重新走一遍按键裁决**——零 UI 侧页状态，杜绝双份页态漂移。

**B3 preedit 三模式**（rimestate.cpp:349-390 `updatePreedit`）：模式选择先看客户端**能力位** `CapabilityFlag::Preedit`，无能力一律 `PreeditMode::No`（写 app 侧 inputPanel preedit，client preedit 置空）；`ComposingText` 把三段组字串（已转换/转换中高亮/待转换，`preeditFromRimeContext` rimestate.cpp:302-347，切分前校验 `sel_start<=sel_end<=length`，:312-315）以下划线格式进 client preedit；`CommitPreview` 把 `context.commit_text_preview`（最终上屏预览）进 client preedit。光标位置是**可配置布局决策**：`PreeditCursorPositionAtBeginning` 把光标钉在开头（注释明言：Linux 只有光标位置可用所以钉住候选窗，rimeengine.h:88-94）。摘录：

```
// rimestate.cpp:360-375（CommitPreview 分支）
    case PreeditMode::CommitPreview: {
        ic->inputPanel().setPreedit(preeditFromRimeContext(
            context, TextFormatFlag::NoFlag, TextFormatFlag::NoFlag));
        if (context.composition.length > 0 && context.commit_text_preview) {
            Text clientPreedit;
            clientPreedit.append(context.commit_text_preview,
                                 TextFormatFlag::Underline);
            if (*engine_->config().preeditCursorPositionAtBeginning) {
                clientPreedit.setCursor(0);
            } else {
                clientPreedit.setCursor(clientPreedit.textLength());
            }
            ic->inputPanel().setClientPreedit(clientPreedit);
        } else {
            ic->inputPanel().setClientPreedit(Text());
        }
    } break;
```

不变量：**app preedit 与 client preedit 是两条独立通道**，按客户端能力与用户偏好装填；光标偏移是策略不是引擎事实。

**B4 主题边界**：插件零主题参与——只产出 `Text`（带 TextFormatFlag）+ 图标名 + Action，渲染全归 fcitx5 前端。`IconTheme theme_` 成员声明于 rimeengine.h:196 但**任何 .cpp 均未使用**（检索词 `theme_` in src/*.cpp → 0 命中，上游死成员）。

**B5 候选动作**：`ActionableCandidateList` 提供"Forget word"单项——从任意候选（当前页或全局迭代器取回的）触发 `delete_candidate(_on_current_page)` 删用户词（rimecandidate.cpp:138-166, rimestate.cpp:269-284）。`BulkCandidateList::candidateFromAll` 用 `candidate_list_from_index` 迭代器**惰性物化 + memo**，越界时把 `maxSize_` 收缩为饱和上限，`totalSize()` 恒 `-1` 表达"未知全集"（rimecandidate.cpp:96-136）。

**B6 状态区动作**（status area）：常驻 `IMAction`（shortText/icon/longText 全部从 `RimeStatus` 派生：disabled→⌛图标、ascii→"A"/"abc"（CapsLock 时 "ABC"，rimestate.cpp:77-90）、否则 schema 名；菜单挂 schemaMenu）+ separator + Deploy + Sync（rimeengine.cpp:198-232）。刷新前有**归属检查**：`if (instance_->inputMethod(&ic) != "rime") return;` 防止覆盖其他 IM（如 keyboard-us 密码场景）的状态区（rimeengine.cpp:395-400）。子模式标签 = schema 名首 UTF-8 字符（rimestate.cpp:116-120）。

## 维度③ 核心旅程：keyEvent 转发链、hotkey、光标、D-Bus

**C1 转发链**（rimeengine.cpp:486-507 `RimeEngine::keyEvent`）：① release 键跳过 deploy/sync 热键检测；② 热键命中 → `event.filterAndAccept()`（消费且下传被滤）；③ `state->keyEvent(event)`，期间置 `currentKeyEventState_`（供同步 option 通知捕获，:504-506）。`RimeState::keyEvent`（rimestate.cpp:164-244）：compose 处理（`processComposeString`，单字符结果改写 keysym 再 `process_key`，多字符/失败则 `commitPreedit + commitString + clear` 直通，:169-222）→ 维护模式检查（:183-185，直接 return **不消费** → 按键穿透给应用）→ 状态掩码映射（保留 Mod1/CapsLock/Shift/Ctrl/Super，Super 补 Super2 虚位，release 置 bit30 `IBUS_RELEASE_MASK`，:195-206）：

```
// rimestate.cpp:207-226（keyEvent 主体截取）
    if (!composeResult.empty()) {
        event.filterAndAccept();
        auto length = utf8::lengthValidated(composeResult);
        bool result = false;
        if (length == 1) {
            auto c = utf8::getChar(composeResult);
            auto sym = Key::keySymFromUnicode(c);
            if (sym != FcitxKey_None) {
                result = api->process_key(session, sym, intStates);
            }
        }
        if (!result) {
            commitPreedit(ic);
            ic->commitString(composeResult);
            clear();
        }
    } else {
        auto result =
            api->process_key(session, event.rawKey().sym(), intStates);
        if (result) {
            event.filterAndAccept();
        }
```

不变量：**消费权二值裁决——`process_key` 真 → filterAndAccept（截停），假 → 完全不碰 KeyEvent（原样穿透）**；随后统一收尾：`get_commit` 有文本即 `ic->commitString`（:231-236）→ `updateUI`（:238）→ schema 未变且面板空但 option 变过 → `showChangedOptions()` HUD（:239-243）。

**C2 updateUI 单一汇合点**（rimestate.cpp:392-433）：面板 reset 带 key-release 防抖（`if (!inputPanel.empty())` 才 reset，:395-397，避免释放键清掉 overlay）；维护模式/find_session/get_context 任一失败即空面板早退；`lastMode_ != subMode()` 才 `showInputMethodInformation` + 刷 StatusArea（:427-430）——**信息条只在模式跃迁时打扰用户**。

**C3 hotkey**：插件侧仅 deploy/sync 两个可配热键（rimeengine.h:111-116，Linux 默认空、Apple 默认 `Control+Alt+grave`，经 `SimpleAction::setHotkey` 生效 rimeengine.cpp:387-388）。ascii 切换不在插件热键层，而是 librime schema 内部处理 + 状态区菜单 "Latin Mode"（rimeengine.cpp:733-739）+ D-Bus。**F4 快捷配置：不存在**（检索词 `F4` → src/ 0 命中；那是 fcitx4 时代记忆）。

**C4 光标跟随**：插件仅上报 preedit 内游标 `preedit.setCursor(context.composition.cursor_pos)`（rimestate.cpp:343）+ 可选钉头（:367,:384）；候选窗定位完全由 fcitx5 core 依 preedit 光标完成。`setCursorRect` **未实现**（检索词 `setCursorRect` → 0 命中）；不抢焦点（检索词 `XSetInputFocus|grab` → 0 命中，仅读 `hasFocus`）。

**C5 D-Bus 服务**（rimeservice.cpp:14-21）：`/rime` 路径注册 `org.fcitx.Fcitx.Rime1`，5 方法（SetAsciiMode/IsAsciiMode/SetSchema/GetCurrentSchema/ListAllSchemas，rimeservice.h:30-34）；全部操作面向 `mostRecentInputContext()`（rimeservice.cpp:23-29）——"最近活跃 IC"是外部控制面的天然目标语义；写操作后若该 IC 持焦点则补 `showInputMethodInformation`（:34-37, :54-57）。

**C6 通知线程归化**（rimeengine.cpp:527-541）：

```
// rimeengine.cpp:527-541（rimeNotificationHandler，静态成员）
    auto *that = static_cast<RimeEngine *>(context);
    if (that->mainThreadId_ == std::this_thread::get_id()) {
        that->notifyImmediately(session, messageType, messageValue);
    }
    that->eventDispatcher_.schedule(
        [that, session, messageType = std::string(messageType),
         messageValue = std::string(messageValue)]() {
            that->notify(session, messageType, messageValue);
        });
```

不变量：**回调线程归属不被假设**——同线程（librime 在 process_key 内同步回呼）走立即路径捕获 changedOptions，异线程（维护线程）只经 EventDispatcher 归化回主线程；消息体以值拷贝跨越线程边界（`std::string(messageType)`），无悬垂。

## 维度④ 错误处理：API 缺失、部署阻塞、通知呈现

**D1 加载失败路径**：`rime_get_api()` 为空 → `EnsureRimeApi` 抛 `std::runtime_error`（rimeengine.cpp:116-122）→ 构造失败 → fcitx5 addon manager 判定插件加载失败（fail-fast，无降级运行态）。会话创建失败则相反——**软失败**：`RimeSessionHolder` 构造抛错被 `requestSession` 的 `catch (...)` 吞掉返回 `{nullptr,false}`（rimesession.cpp:126-133），此后 `session()==0`，keyEvent/getStatus 全部空转早退，按键原样穿透——插件活着但透明。

**D2 首次运行部署阻塞**：`start_maintenance(fullcheck)`（rimeengine.cpp:337）在 librime 内部线程执行；期间 `is_maintenance_mode()` 为真，所有入口（keyEvent :183-185 / selectCandidate :249-252 / deleteCandidate :271-274 / toggleLatinMode :139-141 / selectSchema :156-159 / updateUI :400-403）早退——**部署期间输入引擎整体静默穿透，不挂起不丢键**。UI 依赖部署完成通知（`deploy/success`）后延迟补做：`needRefreshAppOption_` 标记 → `deploy_config_file("fcitx5.yaml")` + `updateAppOptions` + `updateSchemaMenu` + `refreshStatusArea(0)`（:571-579）——结构重建不阻塞维护，推迟到成功广播。

**D3 通知呈现与节流**（rimeengine.cpp:556-607）：`deploy start/success/failure` → 经 notifications 模块 `INotifications::showTip` 桌面通知（:600-601）；`option` → updateStatusArea；`schema` → refreshStatusArea。节流双闸：60s 类型白名单窗（`allowNotification(type)`，:63,:520-523——构造时只放行 `failure` 类型，:237）；success/failure 为终态广播后 `silenceNotificationUntil_ = now+30s` 全量静默（:604-606）——防维护期事件刷屏。

```
// rimeengine.cpp:563-582（notify() 的 deploy 分支）
    if (messageType == "deploy") {
        tipId = "fcitx-rime-deploy";
        icon = "fcitx_rime_deploy";
        if (messageValue == "start") {
            message = _("Rime is under maintenance. It may take a few "
                        "seconds. Please wait until it is finished...");
        } else if (messageValue == "success") {
            message = _("Rime is ready.");
            if (!api_->is_maintenance_mode()) {
                if (needRefreshAppOption_) {
                    api_->deploy_config_file("fcitx5.yaml", "config_version");
                    updateAppOptions();
                    needRefreshAppOption_ = false;
                }
            }
            updateSchemaMenu();
            refreshStatusArea(0);
            blockMessage = true;
        } else if (messageValue == "failure") {
```

不变量：**部署是终态广播驱动的全局结构重建**（schema 菜单、状态区、app options 全部失效重建），通知呈现（节流/静默）与状态更新两条正交路径并行推进。

**D4 其他防线**：finalize 两处 try/catch（析构 :280-284、updateConfig :377-380）；preedit 切分前 sel 范围校验（rimestate.cpp:312-315）；switch IM 时四态收尾策略防组字丢失（Clear/CommitRaw/Composing/Preview，rimeengine.cpp:464-484）。**发现上游真实 bug**：`RimeCandidateList::checkIndex` 条件 `idx < 0 && idx >= size()` 恒为假（应为 `||`，rimecandidate.h:104-108），负索引可穿透到 `labels_[idx]` 构成 UB——静态防御形同虚设，佐证"边界校验必须配测试"。

## 维度⑤ 测试基建（负向证明）

- **单元/集成测试：不存在**。[经源码检索确认：未实现，检索词：`add_test` / `enable_testing` / `gtest` / `catch2` / `QTest` / `ECMAddTests` → 全仓库 0 命中；`find -iname '*test*'` → 0 文件]
- CI（.github/workflows/check.yml）名为 "Build and test" 的 job，`Test` 步骤执行 `ctest --test-dir fcitx5-rime/build`（check.yml:71-73），但项目未注册任何测试 → ctest 空跑通过。CI 的实际功能 = clang-format 检查 + gcc/clang 双编译器构建矩阵 + CodeQL 静态分析（check.yml:62-74）。fcitx5 依赖以 `-DENABLE_X11=Off -DENABLE_WAYLAND=Off -DENABLE_DBUS=Off` 全裁剪构建（check.yml:47-50）——保证插件可脱离显示服务器编译。
- 结论：**正确性完全外置**——依赖 librime 与 fcitx5 core 的树外测试；桥接层自身 2.7k 行逻辑零回归保护。rspinyin 的 mock 驱动确定性测试纪律是对标的明确超越点，无需吸收；可吸收的只有 CI 形态认知（纯插件可无显示服务器构建验证）。

## 跨层边界签名清单

**RimeApi（librime C API）调用点**（全部经 `rime_api_t *api_` 单点持有，rimeengine.h:199）：
- 生命周期: `setup(RimeTraits*)` rimeengine.cpp:332 ｜ `initialize(RimeTraits*)` :335 ｜ `start_maintenance(bool)` :337 ｜ `finalize()` :281/:377/:671 ｜ `is_maintenance_mode()` rimestate.cpp:139,149,157,183,249,271,400
- 会话: `create_session()` rimesession.cpp:29 ｜ `destroy_session(id)` :53 ｜ `set_property(id,"client_app",prog)` :68 ｜ `find_session(id)` rimestate.cpp:405 ｜ `clear_composition(id)` :71
- 按键/输出: `process_key(id,sym,mask)→Bool` rimestate.cpp:215,225 ｜ `get_commit(id,RimeCommit*)→Bool` :232,262 ｜ `get_input(id)→const char*` :439 ｜ `get_context(id,RimeContext*)→Bool` :410,451,465 ｜ `free_commit/free_context/free_status` :234,423,457,471,298
- 状态/选项: `get_status(id,RimeStatus*)` :294 ｜ `get_option/set_option(id,name,Bool)` rimestate.cpp:143-144,152,160,522-524; rimeaction.cpp:37,57-58,108,130,148 ｜ `select_schema(id,name)` :161 ｜ `get_state_label_abbreviated(...)` :83
- 候选: `select_candidate(_on_current_page)(id,idx)` :257-259 ｜ `delete_candidate(_on_current_page)(id,idx)` :279-281 ｜ `candidate_list_from_index/next/end` rimecandidate.cpp:121-132 ｜ `highlight_candidate(id,idx)` :179
- 配置/方案: `config_open/begin_map/begin_list/next/get_bool/get_cstring/close` rimeengine.cpp:65-114,349-351,688-721 ｜ `schema_open` :688 ｜ `get_schema_list/free_schema_list` :730,758; rimeservice.cpp:77-82 ｜ `deploy_config_file` :573 ｜ `sync_user_data()` :682 ｜ `set_notification_handler(fn,ctx)` :336

**fcitx5 addon API 调用点**：
- 引擎钩子: `InputMethodEngineV2` 的 activate/deactivate/keyEvent/reset/reloadConfig/save/getConfig/setConfig/setSubConfig/subMode/subModeLabelImpl/subModeIconImpl（rimeengine.h:126-152 全 override）
- 每 IC 属性: `FactoryFor<RimeState>` + `registerProperty("rimeState",&factory_)`（rimeengine.cpp:185,383）＋ `ic->propertyFor(&factory_)`（:613）
- 按键语义: `KeyEvent{filterAndAccept/accept/filter/isRelease/key/rawKey}`；`instance->processComposeString/resetCompose`（rimestate.cpp:174,235）
- UI 产出: `ic->commitString` rimestate.cpp:220,233,263,441,455,469 ｜ `inputPanel().{reset,setPreedit,setClientPreedit,setCandidateList}` :356-421 ｜ `ic->updatePreedit/updateUserInterface(InputPanel|StatusArea)` :426-432
- 状态区/动作: `registerAction(name,Action*)` rimeengine.cpp:199-228,740,753; rimeaction.cpp:46,111,117 ｜ `statusArea.clearGroup/addAction(StatusGroup::InputMethod,…)` :402-421 ｜ `showInputMethodInformation/showCustomInputMethodInformation` :266; rimestate.cpp:428,614
- 事件/线程: `watchEvent(GlobalConfigReloaded)` rimeengine.cpp:233 ｜ `connect<Instance::XkbStateMaskChanged>` :240 ｜ `EventDispatcher{attach,schedule}` :202,536 ｜ `instance->xkbStateMask(display)` :785
- 外部服务: `dbus::ObjectVTable` + `bus->addObjectVTable("/rime","org.fcitx.Fcitx.Rime1",*this)` rimeservice.cpp:20 ｜ `INotifications::showTip` rimeengine.cpp:600

**隐私对照（红旗）**：`RIME_DEBUG() << event.rawKey()`（rimeengine.cpp:488）在 Debug 级记录用户按键、通知处理器记录 messageValue（:530-531）——与 rspinyin「永不记录输入内容」红线相悖，吸收机制时必须剥离该行为。

## 候选吸收点清单

| 编号 | 源码锚点 | 一句话机制价值 | 跨栈移植难度 |
|---|---|---|---|
| SCOUT-FR-01 | rimesession.cpp:104-134 | 会话共享策略三键制（IC-uuid/程序名/全局）一段代码三种共享粒度，共享边界显式可配且可全局跟随 | 低 |
| SCOUT-FR-02 | rimestate.cpp:475-527 | 会话回收前 snapshot schema+options（"!"前缀编码 false），新会话建好即 restore 且恰好一次——配置热重载不丢用户态 | 中 |
| SCOUT-FR-03 | rimestate.cpp:349-390 | preedit 三模式（无/组字/提交预览）× 客户端能力位（CapabilityFlag::Preedit）× 光标钉头选项，一引擎适配三类客户端 | 低 |
| SCOUT-FR-04 | rimestate.cpp:302-347 | 组字串三段切分（已转换/转换中高亮/待转换）+ sel 范围先验校验——preedit 高亮语义的标准答案 | 低 |
| SCOUT-FR-05 | rimeengine.cpp:527-541 | 引擎回调线程归属中立：同线程同步处理 + EventDispatcher 值拷贝归化主线程，不重不漏无锁 | 低 |
| SCOUT-FR-06 | rimestate.cpp:542-616 | option 变更即时 HUD：按键内同步捕获 changedOptions，收尾时 showCustomInputMethodInformation 聚合显示"简→繁"标签且去重 | 中 |
| SCOUT-FR-07 | rimeengine.cpp:63,520-523,596-606 | 通知节流双闸：60s 类型白名单窗 + 终态后 30s 全量静默——部署期事件刷屏抑制 | 低 |
| SCOUT-FR-08 | rimeengine.cpp:464-484 | 切换输入法四态收尾策略（清除/提交原文/提交组字/提交预览）可配置，杜绝切换丢组字 | 低 |
| SCOUT-FR-09 | rimestate.cpp:169-222 | compose(死键)协议：单字符结果改写 keysym 再入引擎、多字符直通上屏——与 XKB 合成器协同的标准姿势 | 中 |
| SCOUT-FR-10 | rimecandidate.cpp:96-136 | 全量候选惰性迭代物化+memo，越界即收紧 maxSize_ 饱和上限，totalSize=-1 表达未知全集 | 中 |
| SCOUT-FR-11 | rimecandidate.cpp:138-166 | ActionableCandidateList"忘掉该词"→ delete_candidate——候选负反馈一键直通用户词典删除 | 低 |
| SCOUT-FR-12 | rimeengine.cpp:685-722 | 状态区开关从 schema 配置动态生成（2 态 Toggle/N 态 Select），不硬编码任何方案专属选项 | 高 |
| SCOUT-FR-13 | rimesession.cpp:41-48 + rimeengine.cpp:65-87 | per-app 初值（app_options）：会话创建时按程序名 set_option（如终端自动 ascii） | 中 |
| SCOUT-FR-14 | rimestate.cpp:392-433 | updateUI 单一汇合点：key-release 面板重置防抖 + 仅模式跃迁才弹信息条——打扰最小化 | 低 |
| SCOUT-FR-15 | rimeengine.cpp:395-400 | 状态区写入前归属检查（当前 IM 非本插件即返回）——多 IM 共存防串写 | 低 |
| SCOUT-FR-16 | rimecandidate.h:75-86 | 翻页=合成 PageUp/Down 键事件回流按键管线，UI 层零页态、单一裁决入口 | 低 |
| SCOUT-FR-17 | rimeengine.cpp:240-275 | 平台状态（CapsLock）变化监视→仅对同 display 焦点 IC 刷新信息条——平台态接入 IM 的范式 | 中 |
| SCOUT-FR-18 | rimeservice.cpp:23-59 | D-Bus 控制面 5 方法全部面向 mostRecentInputContext——外部操控协议的最小完备集 | 中 |
| SCOUT-FR-19 | rimestate.cpp:515-517 | restore 前校验目标 schema 仍在已注册集合，防恢复到已删除方案 | 低 |
| SCOUT-FR-20 | rimecandidate.h:104-108 | 反面教材：checkIndex `&&` 恒假 bug（应为 `||`）——边界校验无测试兜底必然腐烂 | 低 |
