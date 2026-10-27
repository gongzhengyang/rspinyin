# 勘察摘要：librime（对标簇 SCOUT-C）

> 对标仓库: `remarks/librime`（HEAD `7bc3fb00` "ci: add ubuntu arm runner"，版本 1.17.0，CMakeLists.txt:8）
> 规模: src/ + tools/ + test/ + plugins/ 共 37,125 行 C++17（另有 deps/ 内置 boost/leveldb/marisa 等达 ~157k）；本次精读 deployer、lever/、dict/（db 层）、config/（错误路径）、setup/module/registry/component、rime_api.h、plugins/、test/、tools/、CMake/CI。
> 定位: 输入法核心引擎库（非前端），与 rspinyin 的对应物是 ime-core + ime-dict + 部署器；其「部署任务总线、用户库自愈、产物校验、C ABI 稳定化」四套机制对本项目最具移植价值。
> 边界说明: librime 是库不是 IME 前端，无按键/候选 UI 层；本摘要只覆盖指定三维度。

---

## 维度④ 错误防呆（Poka-Yoke）与自愈

**D1 部署任务总线：逐任务异常隔离 + 结果广播**。`Deployer`（deployer.h）持有互斥锁保护的 `pending_tasks_` 队列；`Run()`（deployer.cc:79-104）逐个弹出执行，单任务异常被 catch 后仅记 failure、不中断批次；结束时经 `message_sink_("deploy", "success"|"failure")` 向宿主广播。任务经组件注册表按名字解析：`RunTask(task_name, std::any arg)` → `DeploymentTask::Require(task_name)->Create(arg)`（deployer.cc:28-40），参数用 `std::any` 传递、`std::bad_any_cast` 被捕获降级为 LOG(ERROR)（user_db_recovery_task.cc:85-92）。

```cpp
// src/rime/deployer.cc:84-99
do {
  while (auto task = NextTask()) {
    try {
      if (task->Run(this)) ++success;
      else ++failure;
    } catch (const std::exception& ex) {
      ++failure;
      LOG(ERROR) << "Error deploying: " << ex.what();
    }
  }
  LOG(INFO) << success + failure << " tasks ran: " << success << " success, "
            << failure << " failure.";
  message_sink_("deploy", !failure ? "success" : "failure");
  // new tasks could have been enqueued while we were sending the message.
  // before quitting, double check if there is nothing left to do.
} while (HasPendingTasks());
```
不变量：**单任务失败只累加计数器并广播汇总结果，批次内其余任务照常执行**。工作线程经 `std::async`（deployer.cc:121），`RIME_NO_THREADING` 编译期退化为同线程（:115-118）。

**D2 用户库三级自愈：disable → Repair → rename .old 保留 → snapshot 恢复**。`UserDbRecoveryTask`（user_db_recovery_task.cc）构造时即 `db_->disable()`（写入路径全局失效），`Run()` 用 BOOST_SCOPE_EXIT 保证任何出口重新 `enable()`；恢复顺序：① `Recoverable::Recover()`（LevelDb 实现为 `leveldb::RepairDB`，level_db.cc:218-227）；② 失败则把损坏库 `rename` 为 `.old` **保留**（仅当 rename 也失败才 `Remove()`，:42-50）；③ 重建空库后 `RestoreUserDataFromSnapshot`——按 `<dict>.userdb.txt` → 旧式 `<dict>.userdb.*.snapshot` 顺序在 `sync/<user_id>/` 找快照恢复（:60-83）。
不变量：**恢复全程库处于 disabled 态；损坏文件永不直接删除（先 .old）；重建后第一动作是从最近已知好快照回填**。`Db` 基类（db.h:64-65）的 `disable()/enable()` 标志让所有 Update/Erase 在恢复期静默 no-op（level_db.cc:179-191 检查 `readonly()||disabled` 由 UserDb 层实现）。

**D3 词典产物 fail-closed 校验链**。`Table::Load()`（table.cc:269-313）逐级验证并即时 `Close()+return false`：mmap 打开 → 偏移 0 处 `Find<table::Metadata>` → `strncmp(metadata_->format, kTableFormatPrefix)` 魔数 → `format_version < kTableFormatLowestCompatible` 拒载旧版本 → `syllabary_`/`index_` 结构指针非空。
不变量：**任何一级校验失败即整体弃用产物，绝不带病运行；格式版本声明"最低兼容下限"而非"精确匹配"，向前兼容靠下限判定**。

**D4 内容指纹驱动的增量编译**。`DictCompiler::Compile()`（dict_compiler.cc:97-137）对全部源 .dict.yaml + 预设词表算 `ChecksumComputer` 校验和，与已存在 .table.bin 的 metadata 内嵌 `dict_file_checksum`（table.cc:326-328）比较决定是否重建；prism 额外比较 `schema_file_checksum`；reverse_db 校验和不同也强制重建表。
不变量：**重建决策 = 源内容指纹与产物内嵌指纹比较，不依赖 mtime；指纹由编译时写入产物 metadata，形成自描述产物**。

**D5 配置 patch 合并的「版本 × 定制」双轴状态机**。`Customizer::UpdateConfigFile()` 头部注释（customizer.cc:18-31）以 9 行真值表穷举 source 版本 / custom 补丁 / dest 版本三方的组合；applied 状态编码进版本串 `X.Y.custom.<checksum>` 与顶层 `customization:` 键（:125-132）。逐 patch 项 `SetItem` 失败即整体 return false（:114-119），`patch` 键缺失仅 WARNING 不硬失败（:122）。

```yaml
# customizer.cc:19-29（原文注释，状态表即规格）
# source custom dest
# 1.0    -      1.0 (up-to-date)
# 2.0    -      1.0  --> Update: 2.0
# 1.0    X      1.0  --> Update: 1.0.custom.X
# 2.0    X      1.0  --> Update: 2.0.custom.X
# 1.0    X      1.0.custom.X (up-to-date)
# 2.0    X      1.0.custom.X  --> Update: 2.0.custom.X
# 1.0    X      2.0  --> Update: 2.0.custom.X
# 1.0    X      2.0.custom.X (up-to-date)
# 1.0    Y      1.0.custom.X  --> Update: 1.0.custom.Y
```
不变量：**「该不该重放 patch」由 (版本比较, 定制校验和是否变化) 二元组唯一决定，状态完全外化到文件内容，无隐藏状态文件**。新版由 config_compiler 的 `__patch` 机制 + `AutoPatchConfigPlugin`（auto_patch_config_plugin.cc:19-36）在编译期自动挂接 `<id>.custom:/patch?`，老机制标 DEPRECATED 但状态表仍是权威语义。

**D6 废弃副本 → trash，永不直接删除**。`TrashDeprecatedUserCopy`（deployment_tasks.cc:287-326）在共享副本版本 > 用户副本、或同版本但用户副本已定制时，把用户副本 `rename` 进 `user_data/trash/`；`.minimal` 后缀视为等于原版本以防误删完整版（:300-302）。配套 `CleanupTrash`（:588-622）把历史遗留垃圾（`.bin`、`.userdb.kct.old`、`.snapshot`）也移入 trash 而非删除。

**D7 备份的防呆细节**。`BackupConfigFiles`（deployment_tasks.cc:541-586）：目标备份已存在且 `Checksum()` 相同 → 跳过；`.yaml` 且含 `customization` 键（`IsCustomizedCopy`，:527-539，即 rime 自己生成过的定制副本）→ 跳过不覆盖；逐文件成功/失败/up-to-date/skipped 四计数汇总日志。`DetectModifications`（:44-80）源目录 mtime（排除 user.yaml）与 user.yaml `var/last_build_time` 比较，`fs` 异常时 return true（**宁可多部署一次也不漏部署**）。`ConfigNeedsUpdate`（:393-429）在编译产物内嵌 `__build_info/timestamps` 逐源文件时间戳做第二层脏检查。`SymlinkingPrebuiltDictionaries`（:472-503）清理悬空符号链接（`fs::canonical` 带 `std::error_code` 检测坏链）。

**D8 资源解析 fallback 链**。`FallbackResourceResolver::ResolvePath`（resource.cc:34-44）先查用户目录，不存在则回退 `fallback_root_path_`（service.cc:170 设为 shared_data_dir、:185 设为 prebuilt_data_dir）。
不变量：**「用户覆盖系统默认」是路径解析层的通用行为，业务代码不写 if-exists 分支**。

**D9 会话边界异常防火墙**。`Service::CreateSession`（service.cc:91-107）用五连 catch（std::exception/string/const char*/int/...）保证 FFI 边界内异常永不逃逸，最坏返回 `kInvalidSessionId`。

**D10 [负向证明] 无原子写、无部署回滚**。`ConfigData::SaveToFile`（config_data.cc:84-95）直接 `std::ofstream` 覆写目标，无 temp+rename 两阶段提交 [经源码检索确认：未实现该机制，检索词：`rename|tmp|tmpfile`（config_data.cc）→ 零命中]；部署失败亦无 staging 回滚，仅靠 D2 的 `.old` 与 D6 的 trash 保留旧物。UserDB 快照文本格式同样无 checksum 尾部（BackupConfigFiles 的校验和只用于跳过相同文件）。

**D11 用户库合并的逻辑时钟**。`UserDbMerger`（user_db.cc:187-244）以 `/tick` 元数据为逻辑时钟：双端各自 `formula_d` 衰减旧值后按 commits 绝对值大者胜、dee 取 max、tick 收敛为 max——多设备同步的 CRDT 雏形；`UserDbImporter`（:246-268）用负 commits 表示墓碑删除。`UserDictManager::Restore`（user_dict_manager.cc:74-102）先恢复进 `.temp` 临时库、`IsUserDb()` 元数据校验通过才 merge 进真身，scope-exit 保证 temp 清理。
不变量：**外部快照先落隔离区并验明正身（db_type 元数据），再以合并语义（非覆盖）入主库**。

## 维度⑤ 测试基建

**T1 真引擎 + 目录沙箱**。test/rime_test_main.cc:9-20 定义 `GlobalEnvironment`：gtest Environment 启动完整 rime（setup+initialize），四个数据目录全部指向 `.`（构建目录 $build/test），用例在纯沙箱内驱动真实部署/词典链路；`app_name="rime.test"` 隔离日志。

**T2 fixture 组织**。`data/test/*.yaml` 共 8 个（config_merge_test.yaml、config_circular_dependency_test.yaml、dictionary_test.dict.yaml、starcraft.yaml 等）；test/CMakeLists.txt 用 `file(GLOB ... data/test/*.yaml)` + `file(COPY ... DESTINATION ${EXECUTABLE_OUTPUT_PATH})` 把 fixture 复制到运行目录，`add_test` 接入 CTest，WORKING_DIRECTORY 即产物目录。词典类用例（dictionary_test.cc、user_db_test.cc）每例开头 `if (db.Exists()) db.Remove();` 自建自清，无全局 fixture 文件依赖。断言风格：标准 gtest `EXPECT_/ASSERT_` 宏，无自定义断言层。

**T3 CI 与构建入口**。.github/workflows/ 五个文件（linux/macos/windows/commit/release）拆分复用（`workflow_call`）；linux-build.yml 矩阵 {ubuntu-latest, ubuntu-26.04-arm} × {gcc, clang}，arm+clang 因 boost 引导失败显式 `exclude`；构建入口 `make test` → Makefile:116-117 `(cd $(build); ctest --output-on-failure)`。依赖清单集中在 action-install-linux.sh（glog/leveldb/marisa/yaml-cpp/opencc/gtest 七个 apt 包）。[负向证明：无 benchmark/perf 工具，检索词：`benchmark|google/benchmark`（CMakeLists+src+tools）→ 零命中]

**T4 tools/ 的运维 CLI**。五个独立小工具复用同一引擎库：rime_deployer（部署器 CLI，tools/rime_deployer.cc 直接操作 default.custom.yaml 的 patch/schema_list）、rime_dict_manager（用户库导入导出/同步）、rime_patch（独立打补丁）、rime_table_decompiler（.table.bin 反编译回文本——**产物可逆向，天然的可调试性设计**）、rime_console（API 调用 REPL）。

## 维度⑤ 跨平台原生适配

**P1 平台隔离层极薄（4 处）**。全部业务源码仅：① common.h:91-105 `path` 类 Windows 下 `u8path` UTF-8→原生宽字符转换、POSIX 下显式禁隐式转换；② algo/fs.h `to_time_t` 跨时钟统一；③ lever/deployment_tasks.cc:28-30 唯一引 windows.h；④ tools/codepage.h Windows 控制台代码页。其余系统交互全走 boost::interprocess（mmap）、boost::dll（dlopen）、std::filesystem。[检索词：`ifdef _WIN32|_MSC_VER|__linux|__APPLE__`（src 全量）→ 仅 common.h 与 deployment_tasks.cc 两文件命中]
不变量：**平台差异被压缩到「路径编码」一个类型点，其余靠第三方库吸收；新平台适配 = 实现一个 path 构造分支**。

**P2 模块注册机制**。模块是 C 结构 `RimeModule{module_name, initialize, finalize, data_size}`；`RIME_REGISTER_MODULE(name)` 宏（rime_api.h:555-567）定义空函数 `rime_require_module_<name>` 供 dladdr 定位，并用 `__attribute__((constructor))`/MSVC `.CRT$XCU`（rime_api.h:540-549 RIME_MODULE_INITIALIZER）在库加载时自动 `RimeRegisterModule`。`ModuleManager::LoadModule`（module.cc:25-37）幂等（loaded_ 集合去重）、initialize 缺失仅 WARNING。模块组（`RIME_REGISTER_MODULE_GROUP(default, "core","dict","gears")`，setup.cc:45-46）实现按需级联加载；`RIME_EXTRA_MODULES` 经 Boost.Preprocessor 把构建期插件清单编进默认模块表（setup.cc:30-43）。

**P3 组件注册机制**。`Registry`（registry.cc）全局单例持 `map<string, ComponentBase*>`；`Class<T,Arg>::Component`（component.h:22-32）为每个类型生成工厂接口，`Require(name)` 内部 `dynamic_cast` 回具体 Component 类型。每个模块的 initialize 函数集中注册本域组件（dict_module.cc:21-40：`r.Register("userdb", new UserDbComponent<LevelDb>)` 等 9 项），字符串键即稳定 ABI（"userdb"/"dictionary"/"userdb_recovery_task"）。
不变量：**跨模块服务发现 = (字符串键 → 工厂) 注册表，新增实现不改动既有模块，legacy 接口以注释指引用插件补注册（dict_module.cc:33-35 legacy_userdb）**。

**P4 插件动态加载**。PluginManager（plugins/plugins_module.cc:35-70）扫描 `rime-<name>.so/.dll` 目录，boost::dll 加载失败 catch 后 `continue`（坏插件不拖垮主程序）；命名约定 `librime-<name>` 剥前缀得模块名再去 `mm.Find(name)`；插件目录靠 `dladdr(current_module_path)` 对已知符号定位自身 so 所在目录 + `RIME_PLUGINS_DIR`（:104-124）；Windows 分支为空实现 TODO（:96-100）。BUILD_MERGED_PLUGINS=ON 时插件静态合并进主库（CMakeLists.txt:19）。

**P5 C ABI 稳定化：自版本化结构体**。核心手法是 `data_size` 头字段 + 指针算术成员探测（rime_api.h:60-75）：

```c
// src/rime_api.h:60-75
#define RIME_STRUCT_INIT(Type, var) \
  ((var).data_size = sizeof(Type) - sizeof((var).data_size))
#define RIME_STRUCT_HAS_MEMBER(var, member)           \
  ((int)(sizeof((var).data_size) + (var).data_size) > \
   (char*)&member - (char*)&var)
#define RIME_STRUCT_CLEAR(var) \
  memset((char*)&(var) + sizeof((var).data_size), 0, (var).data_size)
// tests that member is a non-null pointer in self-versioned struct *p.
#define RIME_PROVIDED(p, member) \
  ((p) && RIME_STRUCT_HAS_MEMBER(*(p), (p)->member) && (p)->member)
```
调用方用自己的编译期 `sizeof` 填 `data_size`，库端据此判断调用方结构体是否包含某字段——新旧任意组合可互操作；结构体字段按版本注释分组追加（RimeTraits :84-117 标 v0.9/v1.0/v1.6）。函数指针 vtable 同理：`RimeApi` 结构 + `RIME_API_AVAILABLE(api, func)`（:534-535）；废弃函数走 `RIME_DEPRECATED` 宏层保留（rime_api_impl.h:285 RimeFreeContext）。RimeModule/RimeTraits 全部走同一机制。
不变量：**结构布局演进只追加不改动，成员可见性由调用方声明的 data_size 逐成员运行期判定——ABI 兼容从「约定」变为「可测试的算术」**。

**P6 日志设施**。glog 可选：`ENABLE_LOGGING=OFF` 时整体替换为 no_logging.h 空实现（setup.cc:10-14），业务代码零改动。`SetupLogging`（setup.cc:83-111）：`FLAGS_logfile_mode = 0600`（与 rspinyin 现行 3.4 策略一致）、app_name 作日志文件前缀符号链接、重复初始化检测（:105-109）。`CleanOldLogFiles`（deployment_tasks.cc:624-696）清理非当日日志但跳过正被符号链接使用的文件，无权限目录（Android）catch 后跳过。app_name 约定 `"rime.<app>"` 前缀保证老日志可识别清理（rime_api.h:93-97 注释）。

**P7 i18n**。[经源码检索确认：未实现该机制，检索词：`find -name "*.po"` + `grep gettext|i18n`（src/rime）→ 零命中；`translation.cc/h` 为候选词迭代器，与 i18n 无关] librime 核心无任何 UI 文案，i18n 属于前端发行版（如 Weasel/Squirrel）职责。

**P8 CMake 平台组织**。顶层 CMakeLists.txt 以 16 个 option 声明能力开关（BUILD_SHARED_LIBS/BUILD_MERGED_PLUGINS/BUILD_STATIC/ENABLE_LOGGING/ENABLE_ASAN/ENABLE_THREADING/ENABLE_TIMESTAMP/ENABLE_EXTERNAL_PLUGINS 等，:18-31）；平台分支仅 3 处（WIN32:36、MSVC:61、Linux 专属 Boost find:65-69）；Windows 子目录 cmake/ 承载 MSVC 细节；Makefile:24 甚至特判 Termux（ANDROID）路径。库拆分：rime（总库）/rime-dict/rime-gears/rime-levers 可分可合（BUILD_SEPARATE_LIBS）。

---

## 《候选吸收点清单》

| 编号 | 源码锚点 | 一句话机制价值 | 跨栈移植难度 |
|---|---|---|---|
| SCOUT-C-01 | src/rime/deployer.cc:79-104 | 部署任务总线：逐任务 try/catch 隔离 + success/failure 计数 + message_sink 广播，单任务坏不拖垮批次 | 低 |
| SCOUT-C-02 | src/rime/dict/user_db_recovery_task.cc:23-58 | 用户库三级自愈：disable→Recover→损坏文件 rename .old 保留→从 sync 快照回填 | 中 |
| SCOUT-C-03 | src/rime/dict/db.h:88-92, level_db.cc:218-227 | 单方法 `Recoverable::Recover()` trait 使修复策略可按后端插拔 | 低 |
| SCOUT-C-04 | src/rime/dict/table.cc:269-313 | 产物加载 fail-closed 校验链：metadata→魔数前缀→版本下限→结构指针逐级即时 Close | 低 |
| SCOUT-C-05 | src/rime/dict/dict_compiler.cc:97-137 | 增量编译=源内容 checksum 与产物内嵌指纹比较（非 mtime），产物自描述 | 低 |
| SCOUT-C-06 | src/rime/lever/customizer.cc:18-31 | patch 重放状态机：版本×定制校验和双轴真值表，状态外化进文件版本串 `.custom.<sum>` | 中 |
| SCOUT-C-07 | src/rime/lever/deployment_tasks.cc:287-326 | 废弃用户副本永不删除，版本比较后 rename 进 trash/ | 低 |
| SCOUT-C-08 | src/rime/lever/deployment_tasks.cc:527-586 | 备份防呆：checksum 相同跳过 + `customization` 键识别用户定制副本免覆盖 + 四计数日志 | 低 |
| SCOUT-C-09 | src/rime/lever/user_dict_manager.cc:74-102 | 外部快照恢复经临时库隔离：restore→temp→db_type 元数据验明→merge→scope-exit 清理 | 中 |
| SCOUT-C-10 | src/rime/dict/user_db.cc:187-268 | 用户数据合并用逻辑时钟 tick + 衰减公式 + 负 commits 墓碑，多端同步免冲突 | 高 |
| SCOUT-C-11 | src/rime/resource.cc:34-44 | 用户目录优先→系统目录 fallback 的资源解析链，业务零 if-exists | 低 |
| SCOUT-C-12 | src/rime/lever/deployment_tasks.cc:44-80, 393-429 | 双层脏检查：源 mtime vs `var/last_build_time`（异常时宁做勿漏）+ 产物内嵌逐文件时间戳 | 中 |
| SCOUT-C-13 | src/rime_api.h:60-75, 534-535 | data_size 自版本化结构体 + 指针算术成员探测，C ABI 兼容成为可测试算术（可平移到 Rust repr(C) FFI 头） | 低 |
| SCOUT-C-14 | src/rime_api.h:555-567, module.cc:25-37, dict_module.cc:19-40 | 模块= C 结构 + constructor 自动注册 + 幂等惰性加载 + 字符串键组件注册表 | 中 |
| SCOUT-C-15 | plugins/plugins_module.cc:35-84 | 插件目录扫描 + `librime-<name>` 命名约定→模块名映射 + dladdr 自定位 + 坏插件 continue | 中 |
| SCOUT-C-16 | src/rime/setup.cc:83-111, deployment_tasks.cc:624-696 | 日志 0600 + app_name 前缀隔离 + 只清非当日且不在使用的日志文件 | 低 |
| SCOUT-C-17 | src/rime/service.cc:91-107 | FFI/会话边界五连 catch 防火墙，异常永不逃逸、最坏返回无效句柄 | 低 |
| SCOUT-C-18 | test/rime_test_main.cc:9-20, test/CMakeLists.txt | 测试=真引擎启动 + 四数据目录全指向构建目录沙箱 + data/test fixture 复制 + CTest 接入 | 低 |
| SCOUT-C-19 | tools/rime_table_decompiler.cc | 产物反编译 CLI：二进制词典可逆向回文本，交付物自带可调试性 | 中 |
| SCOUT-C-20 | src/rime/dict/dict_module.cc:33-35 | 遗留格式升级钩子：注册表预留 `legacy_userdb` 插槽，新版本可后装升级器 | 低 |

**跨栈警示（负向，勿吸收）**：config_data.cc:84-95 保存无 temp+rename 原子写（rspinyin 已有 atomic-write 计划，勿倒退）；部署失败无整体回滚（仅文件级 trash/.old）；无 benchmark 基建（rspinyin 有 criterion 契约，保持）；userdb 快照无 checksum 尾部校验。
