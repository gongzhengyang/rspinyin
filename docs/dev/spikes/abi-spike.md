# Spike 结论：Rust `cdylib` 与 Fcitx5 C++ 宿主的 C ABI 可行性

> 状态：代码侧结论已完成；`nm -D` 与 `fcitx5 -r` 的实测输出为占位，由主 Agent 回填。
> 关联模块：`MOD-RT` ｜ 风险编号：R-01（本 spike 即该风险落点）
> 代码落点：`crates/ime-fcitx5/build.rs`、`crates/ime-fcitx5/src/ffi/{mod,abi}.rs`、
> `crates/ime-fcitx5/src/ffi/cpp/{addon,engine,ui}_glue.cpp`

## 0. 环境

| 项 | 值 | 来源 |
|---|---|---|
| Fcitx5 版本 | **5.1.7** | 头文件证据：`addonmanager.h` 中 `setAddonOptions`/`addonOptions` 标注 `@since 5.1.7`，`AddonInstance::canRestart` 标注 `@since 5.1.6`；实测 `fcitx5 -v` 输出见 §4 占位 |
| 头文件路径 | `/usr/include/Fcitx5/Core/fcitx/`、`/usr/include/Fcitx5/Utils/fcitx-utils/` | 本机安装 |
| C++ 标准 | `-std=c++17` | 与 Fcitx5 5.1 自身构建标准对齐，避免 ABI 漂移 |
| Rust | edition 2024 / MSRV 1.85 / toolchain 1.98.0 | workspace 配置 |

## 1. 三个验证问题的结论

### (a) Rust `cdylib` 能否导出 fcitx5 可识别的 addon 工厂符号 —— **可行**

`FCITX_ADDON_FACTORY(ClassName)` 在 5.1.7 中的展开（`fcitx/addoninstance.h:188`）：

```cpp
extern "C" {
FCITXCORE_EXPORT
::fcitx::AddonFactory *fcitx_addon_factory_instance() {
    static ClassName factory;
    return &factory;
}
}
```

- `FCITXCORE_EXPORT` = `__attribute__((visibility("default")))`（`fcitxcore_export.h`），因此符号默认可见。
- 该符号**必须由 C++ 生成**：宏依赖 C++ 模板与 `fcitx::AddonManager` 的完整定义，Rust 侧无法构造。Rust 侧只导出一个符号 `rspinyin_plugin_init`，与 2.2.3「Rust 侧只暴露一个导出符号与一个 vtable 结构体」一致。
- 两条独立保证它不会被裁掉：
  1. `build.rs` 的 `cc::Build` 显式加 `-fvisibility=default`（默认值，但写出来是因为一旦被隐藏，产物会静默地无法加载）；
  2. 胶水编进静态归档 `librspinyin_glue.a`，而工厂符号只被 fcitx5 的 `dlsym` 使用、Rust 侧不引用 —— 归档成员可能被判为无用而丢弃。`build.rs` 因此加 `-Wl,--undefined=fcitx_addon_factory_instance`；同时 Rust 调用 `rspinyin_register_vtable` 也会把同一个目标文件拉进链接（双保险）。
- `[profile.release]` 的 `strip = "symbols"` 只清 `.symtab`，`nm -D` 读的 `.dynsym` 不受影响 —— 这一条属于推断，实测见 §4。

### (b) C++ 虚基类能否通过 C ABI vtable 跨语言安全调用 —— **可行**

- 对象生命周期完全留在 C++ 侧：`RspinyinAddon`/`RspinyinEngine`/`RspinyinUi` 由 fcitx5 创建与销毁，Rust 侧只看到 `*mut c_void`（不透明上下文）与 POD 结构。
- 跨边界只传 `#[repr(C)]` 结构（`FcitxCursorRect` / `FcitxKeyEvent` / `UiPanelSnapshot`）与函数指针表；字符串一律 `ptr + len`，由 C++ 用 `std::string` 的 `data()/size()` 保证长度。
- panic 不跨边界：Rust 侧每个 `extern "C"` 函数体都跑在 `ffi::guard_ffi` 的 `catch_unwind` 兜底里，panic 转成 fallback 返回值并写崩溃日志（`ffi/panic`）。workspace 的 `[profile.release]` 注释明确禁止 `panic = "abort"`，否则兜底失效。
- 编译期证明：两个派生类各带一条 `static_assert(!std::is_abstract_v<...>)`，少实现一个纯虚函数即编译失败。
- **代价**：`RspinyinVtable` 的字段顺序就是 ABI，只能追加；新增字段必须同时改 `abi.rs` 与三份 C++ 镜像定义并递增 `RSPINYIN_ABI_VERSION`。

### (c) `InputMethodEngineV2` / `UserInterface` 虚函数签名在 5.1.x 内是否稳定 —— **稳定，但必须用编译期断言守住**

从 5.1.7 头文件抄录的实际签名：

| 类 | 虚函数 | 说明 |
|---|---|---|
| `fcitx::AddonInstance` | `reloadConfig()` / `save()` / `getConfig()` / `setConfig()` / `getSubConfig()` / `setSubConfig()` | 全部有默认实现 |
| `fcitx::InputMethodEngine` | `keyEvent(const InputMethodEntry &, KeyEvent &)` | **唯一纯虚** |
| `fcitx::InputMethodEngine` | `activate` / `deactivate` / `reset` / `filterKey` / `listInputMethods` … | 有默认实现 |
| `fcitx::InputMethodEngineV2` | `subModeIconImpl` / `subModeLabelImpl` | 有默认实现 |
| `fcitx::UserInterface` | `update(UserInterfaceComponent, InputContext *)` / `available()` / `suspend()` / `resume()` | **四个纯虚** |

- 5.1.x 内新增能力走的是**新增派生类**（`InputMethodEngineV3`/`V4`）与**新增非虚函数**，没有改动既有虚函数签名；唯一被标记 `FCITXCORE_DEPRECATED` 的是 `InputMethodEngine::updateSurroundingText`，本插件不使用。
- 对策：所有覆盖都写 `override`，再加 `static_assert(!is_abstract)`，签名漂移会在编译期而非运行期暴露；构建按 pkg-config 探测到的 5.1.x 链接，不锁死小版本。
- 与 2.2.3 契约无冲突：契约里 `on_*` 回调的参数是自定的 POD 形状，与 fcitx5 的 C++ 类型解耦，所以 fcitx5 小版本升级不会波及 Rust 侧。

## 2. 落地后的调用关系

```
fcitx5 ──dlopen──> librspinyin.so
       ──dlsym───> fcitx_addon_factory_instance()        (C++ 宏，唯一被 dlsym 的符号)
       ──create(AddonManager*)──> new RspinyinAddon
                                      │
                                      ├─ rspinyin_plugin_init()          (Rust 唯一导出符号)
                                      │     ├─ 校验自身 vtable 的 abi_version
                                      │     └─ rspinyin_register_vtable(&RSPINYIN_VTABLE)   (C++ 提供)
                                      │            └─ 再校验一次版本；不匹配则拒绝注册并写诊断
                                      ├─ 把返回的不透明 context 交给宿主
                                      └─ on_addon_init(context)          (Rust 回调)
```

ABI 版本校验**两侧各做一次**：Rust 侧用 `ime_types::version::check_abi`（诊断文案沿用冻结的
`platform/fcitx5/version-mismatch: host=… required=…`），C++ 侧用编译期常量比较（日志
`rspinyin: ABI mismatch (host=1, plugin=N)`）。任一侧拒绝的可见效果相同：不注册、不调用
`on_addon_init`、插件停在纯引擎模式。

## 3. 坑与对策（按踩到顺序）

1. **任务卡与头文件不一致**：任务卡写 `fcitx::AddonInstance *fcitx_addon_factory_instance(fcitx::AddonManager *)`，5.1.7 实际是**无参数、返回 `fcitx::AddonFactory *`**（旧版 API 的残留描述）。以头文件为准；符号名不变，DoD#1 的 `nm` 断言不受影响。
2. **静态归档裁剪**：见 §1(a)。若后续把 `rspinyin_register_vtable` 的调用挪到别的编译单元，工厂符号会从 `.so` 消失，表现为「能链接但 fcitx5 加载不到 addon」。
3. **`--as-needed` 链接顺序**：pkg-config 探测必须先于 `cc` 编译（胶水需要 include 目录），于是 `-lFcitx5*` 会排在静态归档之前，Debian/Ubuntu 工具链下会被丢弃，最终报一堆 `fcitx::…` 未定义符号。`build.rs` 在编译之后**重复发射**一次 link-lib 指令；重复的 `-l` 无害。
4. **edition 2024 语法**：必须写 `#[unsafe(no_mangle)]` 与 `unsafe extern "C" { … }`，否则直接编译失败。
5. **`unsafe_code = "deny"`**（workspace lints）：`ffi/mod.rs` 顶部加 `#![allow(unsafe_code)]` 并写明理由；`build.rs` 内不含 `unsafe`。
6. **没有 `clippy.toml`**：`unwrap_used`/`expect_used`/`panic` 是 workspace 级 `warn` + 门禁 `-D warnings`，因此**测试代码也不能用 `unwrap`/`expect`/`panic!`**。测试用 `std::panic::panic_any` 制造 panic（不受 `clippy::panic` 约束），用 `matches!`/`if let` 断言。
7. **`build.rs` 的 `rustc-check-cfg` 必须无条件发射**：原实现只在 `fcitx5-host` 打开时打印，导致纯 Rust 构建下 `#[cfg(fcitx5_host)]` 被判为未知 cfg 而告警 → 门禁失败。已移到特性判断之前。
8. **`InputContext` 没有数字 id**：只有 16 字节 `ICUUID`，而 ABI 需要 `u64`。实现取 uuid 的 64 位 FNV-1a 摘要（无锁、无分配、上下文生命周期内稳定）；若后续改为会话注册表，`rspinyin::ic_id` 是唯一改动点。
9. **一个 `.so` 只能有一个工厂符号**：`UserInterfaceManager::load()` / `InputMethodManager` 都是「按 addon 名取实例再转型」，而本库只有一个 addon 实例。因此 `engine_glue.cpp` / `ui_glue.cpp` 的两个类**当前没有被实例化**，它们的存在价值是「对着真实头文件编译通过」这一签名验证；引擎与 UI 两个角色如何分发（多继承 / 两个 addon 名共用 `Library=` / 其它）留给后续卡片决策。
10. **空 preedit / 无候选是合法状态**：面板快照的 `(null, 0)` 表示空缓冲而非非法，故 `on_input_panel_update` 用空容忍的读取器；而 `on_commit_string` / `on_set_preedit` 的 `len == 0` 按 2.2.3 表判定为 `ffi/invalid-*` 并忽略。
11. **缺少 addon `.conf` 时 fcitx5 不会尝试加载**：`packaging/fcitx5/rspinyin.conf`（含 `Library=rspinyin`）属于后续卡片。DoD#2 的实验室验证需要先放好该文件或临时 conf。
12. **日志文案两种写法**：任务卡「目标与职责」写 `rspinyin addon loaded`（无冒号），DoD#2 写 `rspinyin: addon loaded`（有冒号）。实现取 DoD 版本（带冒号）。
13. **C++ 无共享头文件**：文件白名单只允许三个 `.cpp`，因此契约结构在三份文件中各镜像一份（内容完全一致）。`build.rs` 的 `rerun-if-changed=src/ffi/cpp` 覆盖改动重编。
14. **新增错误码需回填文档**：`ffi/panic`、`ffi/null-key-event`、`ffi/null-panel-snapshot`、`ffi/invalid-panel-snapshot`、`ffi/invalid-preedit`、`ffi/host-not-linked`（`ffi/invalid-commit` 已在冻结表内）。`docs/dev/features.md` 2.2.4 由主 Agent 维护。
15. **C++ 告警面**：胶水按 `-Wall` 编译，但没有 `-Werror`；C++ 告警不属于任何门禁，需人工过一眼。

## 4. 实测证据（主 Agent 回填）

### 4.1 工厂符号导出（DoD#1）

```bash
nm -D --defined-only target/release/librspinyin.so | grep fcitx_addon_factory_instance
```

预期：一行 `… T fcitx_addon_factory_instance`。

实测输出（2026-09-29，rustc 1.98.0，mold 与 GNU ld 结果一致）：

```text
0000000000015ae0 T fcitx_addon_factory_instance
```

**坑 3 的推断被推翻**：`strip = "symbols"` **不是**原因，去掉它符号依然缺失；`--export-dynamic`、
`--export-dynamic-symbol`、`--dynamic-list` 三个 flag **全部无效**。

真实原因由 linker 包装脚本抓取实际 argv 得到 —— rustc 为 cdylib 生成的 version script 是：

```text
{
  global:
    rspinyin_plugin_init;

  local:
    *;
};
```

`local: *` 是**权威**的，任何 link flag 都无法把已被它置为 local 的符号重新暴露。rustc 的 global
列表只含 Rust 侧标记导出的符号，因此 C++ 定义的工厂**必然**不可见。

**解法**（已落地）：`addon_glue.cpp` 手工展开宏为私有名 `rspinyin_addon_factory`，由 Rust 侧
`#[unsafe(no_mangle)] pub extern "C" fn fcitx_addon_factory_instance()` 转发导出。工厂对象仍在
C++ 构造（完整类型可用），Rust 只转指针。`--undefined=` 仍需保留：它负责把归档成员拉进镜像，
与「导出」是两件事。

### 4.2 fcitx5 加载与日志（DoD#2）

```bash
sudo cp target/release/librspinyin.so /usr/lib/x86_64-linux-gnu/fcitx5/
sudo cp packaging/fcitx5/rspinyin.conf /usr/share/fcitx5/addon/
fcitx5 -r
```

实测输出（fcitx5 5.1.7，WSLg X11）：

```text
I2026-09-29 13:14:43.342845 addon_glue.cpp:209] rspinyin: addon loaded
I2026-09-29 13:14:43.342880 addonmanager.cpp:195] Loaded addon rspinyin
```

**坑 11 补充**：addon conf 的 `Library=` 是 `<值>.so` 的**文件名主干**，不含 `lib` 前缀的补全。
其他 addon 写 `Library=libquickphrase` 是因为它们的产物就叫 `libquickphrase.so`。本插件产物是
`librspinyin.so`，因此必须写 `Library=librspinyin`；写 `Library=rspinyin` 会报
`Could not locate library rspinyin.so`。

### 4.3 版本确认

```bash
fcitx5 --version
```

实测输出：

```text
5.1.7
```

`pkg-config --modversion Fcitx5Core Fcitx5Utils Fcitx5Config` 同为 `5.1.7`，与 `ASM-14` 的
「Fcitx5 5.1.x」一致。

## 5. 遗留决策（交给主 Agent）

1. 引擎 / UI 角色的实例化方式（坑 9）：三选一需要主 Agent 决策并同步到后续卡片。
2. 新增错误码合入 `features.md` 2.2.4（坑 14）。
3. `packaging/fcitx5/rspinyin.conf` 的就位时间点（坑 11）决定 DoD#2 何时可实测。
4. ~~若 `nm -D` 断言失败，下一步是去掉 `strip` 或改用 `--export-dynamic-symbol`~~ —— **已解决**，
   见 §4.1：两个方向都无效，真实原因是 rustc 的 version script，解法是改由 Rust 导出工厂符号。
   这条**偏离了 `features.md` 2.2.3 的「Rust 侧不直接导出工厂符号」**，需要回写该节并记入 ADR。
5. addon conf 的 `Library=librspinyin`（含 `lib` 前缀）需在 `TASK-1.04.02` 落地时保持一致，
   否则 DoD#2 会退化为 `Could not locate library rspinyin.so`。
