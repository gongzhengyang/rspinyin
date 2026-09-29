# ADR-0002：由 Rust 侧导出 Fcitx5 addon 工厂符号

> 状态：**已接受（Accepted）** ｜ 决策日期：**2026-09-29** ｜ 决策人：主 Agent（宿主集成）｜
> 关联文档：[../features.md](../features.md) 的 2.2.3（插件 ↔ 宿主 C ABI）、`TASK-1.04.01` 任务卡 ｜
> 关联 ADR：[0000-upstream-decisions.md](0000-upstream-decisions.md)、[0001-frozen-boundary-contracts.md](0001-frozen-boundary-contracts.md) ｜
> 实测证据：[../spikes/abi-spike.md](../spikes/abi-spike.md) 的 §4.1

---

## 背景

Fcitx5 加载插件的方式是 `dlopen("librspinyin.so")` 之后 `dlsym("fcitx_addon_factory_instance")`。
这个符号的常规产生方式是 C++ 侧的 `FCITX_ADDON_FACTORY(ClassName)` 宏（定义在
`/usr/include/Fcitx5/Core/fcitx/addoninstance.h`），它展开成一个 `extern "C"` 的函数：

```cpp
extern "C" FCITXCORE_EXPORT ::fcitx::AddonFactory *fcitx_addon_factory_instance() {
    static ClassName factory;
    return &factory;
}
```

`features.md` 2.2.3 据此规定：**「Rust 侧不直接导出工厂符号」**，理由是宏依赖 C++ 模板与
`fcitx::AddonManager` 的完整定义，Rust 侧无法生成。

`TASK-1.04.01` 按此实现后，`nm -D` 断言失败：`.dynsym` 里只有 `rspinyin_plugin_init`
一个已定义符号，工厂符号不存在。

## 决策

**工厂符号改由 Rust 侧导出**：`addon_glue.cpp` 手工展开宏为私有名
`rspinyin_addon_factory`，Rust 侧以 `#[unsafe(no_mangle)] pub extern "C" fn
fcitx_addon_factory_instance()` 转发导出。工厂对象**仍在 C++ 构造**（完整类型可用），
Rust 只转指针。

## 理由

根因经 linker 包装脚本抓取实际 argv 定位，**不是** `strip`、也**不是**链接器选择：

rustc 为 `cdylib` 生成的 version script 是

```text
{
  global:
    rspinyin_plugin_init;

  local:
    *;
};
```

`local: *` 是**权威**的。以下四者全部实测无效：

| 尝试 | 结果 |
|---|---|
| 去掉 `[profile.release] strip = "symbols"` | 符号仍缺失 |
| `-Wl,--export-dynamic` | 符号仍缺失 |
| `-Wl,--export-dynamic-symbol=fcitx_addon_factory_instance` | 符号仍缺失 |
| `-Wl,--dynamic-list=<含该符号的文件>` | 符号仍缺失 |
| 换 `mold` → GNU ld (`bfd`) | 结果一致，与链接器无关 |

rustc 的 global 列表**只含 Rust 侧标记导出的符号**，因此 C++ 定义的工厂**必然**不可见。
这条约束无法从 C++ 侧绕过，只能把导出动作移到 Rust 侧。

## 偏差登记（对 features.md 2.2.3）

| # | 规格原文 | 实际落地 | 理由 |
|---|---|---|---|
| 1 | 「Rust 侧不直接导出工厂符号」 | Rust 侧导出 `fcitx_addon_factory_instance`，C++ 侧改为导出私有名 `rspinyin_addon_factory` | 规格的前提（C++ 可导出该符号）在 rustc cdylib 下不成立，见上表 |
| 2 | 「Rust 侧只提供 `RspinyinVtable` 与 `rspinyin_register_vtable`」 | 另需一个 Rust 导出符号。`rspinyin_register_vtable` 仍由 C++ 提供、Rust 声明（与规格一致） | 规格未规定 C++ 如何取得 vtable；本 ADR 一并冻结为：C++ 工厂构造时调用 Rust 的 `rspinyin_plugin_init`，返回不透明 context（null 表示拒绝/纯引擎模式） |

## 后果

- `TASK-1.04.02` 起的 addon 生命周期代码不受影响：握手、ABI 校验与 panic 兜底仍在原位置。
- `packaging/fcitx5/rspinyin.conf` 的 `Library=` 必须是 `librspinyin`（带 `lib` 前缀）。
  fcitx5 把该值直接拼成 `<值>.so`；写 `rspinyin` 会去找 `rspinyin.so` 并失败。已实测。
- `build.rs` 的 `-Wl,--undefined=fcitx_addon_factory_instance` **仍需保留**：它负责把归档成员
  拉进镜像，与「导出」是两件事，缺了它会得到一个「能加载但没有 addon」的库。
- 任何未来新增的、需要被宿主 `dlsym` 的 C++ 符号，都必须走同一条路（Rust 转发导出），
  不能指望链接器 flag。

## 待办

- 回写 `features.md` 2.2.3 的「Rust 侧不直接导出工厂符号」一句，并登记本 ADR。

## 参考

- `/usr/include/Fcitx5/Core/fcitx/addoninstance.h`（`FCITX_ADDON_FACTORY` 宏定义）
- [../spikes/abi-spike.md](../spikes/abi-spike.md) §4.1（`nm -D` 实测输出与 linker argv 抓取过程）
