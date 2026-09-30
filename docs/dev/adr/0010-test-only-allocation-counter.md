# ADR-0010：测试专用分配计数器（第四条 `unsafe` 白名单路径）

> 状态：**已接受（Accepted）** ｜ 决策日期：**2026-09-30** ｜ 决策人：**用户**（经主 Agent 提出并取得确认）｜
> 关联文档：[../../AGENTS.md](../../AGENTS.md) §3.3、§8.2 ｜ [../features.md](../features.md) 0.4 规则 3、0.5.3（`BUDGET-MEM-04`）｜ [../tests.md](../tests.md) 的 `TC-CORE-09` ｜
> 关联 ADR：[0001-frozen-boundary-contracts.md](0001-frozen-boundary-contracts.md)、[0002-rust-exports-addon-factory.md](0002-rust-exports-addon-factory.md) ｜
> 实测证据：`just check-unsafe`（含 `--self-test`）、`just check-deps`、`cargo nextest run -p alloc-count`

---

## 背景

`AGENTS.md` §3.3 与 §8.2 把 `unsafe` 限制在三处：两个 addon 的 `ffi/**`（两套 C ABI，各自编译自己的胶水）与 `crates/ime-dict/src/mmap.rs`（词典的零拷贝映射）。这条边界是 0.4 规则 3，由 `scripts/check-unsafe.sh` 按**文件路径**强制执行，且每块 `unsafe` 必须有 `// SAFETY:` 注释。

现在有两条**已冻结、已写进文档**的承诺落不到地上：

| 承诺 | 出处 | 需要的测量 |
|---|---|---|
| 单次解码**零堆分配** | `docs/dev/tests.md` 的 `TC-CORE-09`；`BUDGET-LAT-02` 的前提 | 分配次数 |
| 解码的**瞬时工作集峰值** ≤ 12 KiB | `docs/dev/opt-perf.md` 的 `PERF-P0.01.01`；`ASM-P09` 的 40% 压减以此为基线 | 分配字节数 |

两者都只能由 `#[global_allocator]` 观测，而安装全局分配器**必须**写 `unsafe impl GlobalAlloc` —— Rust 没有安全替代品。这不是"可以用更安全的写法绕开"的问题：`GlobalAlloc` 的每个方法都在原始指针上操作，语言层面不存在不需要 `unsafe` 的实现方式。

本仓库此前已经两次登记过这个缺口（`PERF-P0.04.01` 与 `PERF-P1.04.01` 的验收记录都写了"分配器探针无法在本仓库被武装"），但都止步于登记：**不解决它，那两条承诺就永远只是注释**。

## 决策

**新增 `crates/alloc-count`，并把 `crates/alloc-count/src/**` 加为第四条 `unsafe` 白名单路径。该 crate 是纯测试件：任何发布产物都不得依赖它。**

### 决策 1：为什么是独立 crate，而不是往既有文件里塞

三个候选位置都被否决：

| 位置 | 否决理由 |
|---|---|
| `crates/ime-core/src/**` 的测试模块 | `#[global_allocator]` 只在**二进制 crate 根**生效。库 crate 里装不了，`#[cfg(test)]` 也不行 —— 库的测试是一个独立编译的二进制，但它的根在 `lib.rs`，不在测试模块里。写进去就是一段永远不生效的代码 |
| `crates/ime-fcitx5/src/ffi/**` | 那条路径的许可是给 **C ABI 胶水**的，语义完全不同。把一个测试用的分配器放进去，会让"`ffi/**` 里的 `unsafe` 都是 FFI 边界"这条可读的不变量失效 |
| 直接放宽 §3.3 的措辞为"允许任何测试代码" | 那等于删掉一条边界，而不是开一个口子。`check-unsafe.sh` 是按路径白名单工作的，措辞放宽之后它无从执行 |

独立 crate 让这条许可有**可执行的边界**：路径唯一、依赖方向唯一、发布路径可达性可断言。

### 决策 2：隔离靠断言，不靠约定

"纯测试件"如果不被机器强制，就只是一句注释。因此加了两条断言，都在 `scripts/check-unsafe.sh` 里：

1. **任何 crate 的 `Cargo.toml` 若提到 `alloc-count`，只允许出现在 `[dev-dependencies]` 段**。出现在 `[dependencies]` 即失败。
2. **`crates/ime-fcitx5` 与 `crates/ime-ui-addon` 的 `Cargo.toml` 一律不得提到它** —— 那两个是唯二产出发布产物的 crate，`dev-dependencies` 也不行，因为它们连测试都不该把分配器带进链接图。

`scripts/check-deps.sh` 另把 `alloc-count` 登记进 `LEAF_ONLY`，允许的内部依赖集是**空集**：它连 `ime-types` 都不许依赖，所以它不可能成为任何类型系统的中间环节。

`scripts/check-unsafe.sh --self-test` 的 scratch 树里加了它的正例文件（一个带 `// SAFETY:` 的 `unsafe impl GlobalAlloc`），因此"这条路径确实被允许"是被测出来的，不是被写下来的。

### 决策 3：分配器只计数，不改行为

`Counting` 的每个方法都原样转发给 `System`：不池化、不改对齐、不做指针运算。一个会改变行为的分配器，会让**被测量**的对象与**发布**的对象不是同一个东西 —— 那正好毁掉这项测量的全部意义。

`alloc` / `alloc_zeroed` / `realloc` 各记一次调用与请求字节数；`dealloc` 不记（它不产生分配）。计数用 `Relaxed` 原子：它们只在单线程的测量窗口里被读，且计数器本身绝不能分配。

### 决策 4：计数是进程级的，所以测试必须独占进程

计数器是进程全局的。`cargo nextest` 给每个测试**一个进程**，因此 `allocations()` 的前后差就是被测代码的差；`cargo test` 让同 crate 的测试共享进程，读到的数会被旁边并行跑的测试污染。这是本 crate 的文档里写明的**使用前提**，也是本仓库统一用 `cargo nextest run` 的又一条理由。

## 影响面

| 位置 | 变更 |
|---|---|
| `crates/alloc-count/` | 新增：`Cargo.toml` + `src/lib.rs`（`Counting` / `allocations()` / `bytes()`），3 个自测 |
| `Cargo.toml`（workspace） | `[workspace.dependencies]` 新增 `alloc-count = { path = "crates/alloc-count" }`；`members = ["crates/*", "xtask"]` 已覆盖它，无需改 |
| `AGENTS.md` §3.3 | `unsafe` 允许位置由三处改为四处，并写明第四条的理由与"发布产物不得依赖它" |
| `AGENTS.md` §8.2 | 禁止事项 2 的路径列表同步 |
| `scripts/check-unsafe.sh` | `ALLOWED_DIRS` 新增该路径；新增两条发布依赖断言；`--self-test` 新增正例 |
| `scripts/check-deps.sh` | `LEAF_ONLY` 新增 `"alloc-count": set()`；leaf 的失败文案改为通用措辞 |

**没有破坏性变更**：`unsafe` 的边界是被**扩大**了一条有断言保护的路径，既有三处的许可与断言一条未动。`Cargo.lock` 新增一个 workspace 内部包，不引入任何外部依赖。

## 这条决策的风险与它的护栏

- **风险**：一条"仅测试用"的路径，如果哪天被人从生产代码里引用，`unsafe` 就通过一个已被审计放行的文件重新进入了发布产物。
  **护栏**：决策 2 的两条断言。它们检查的是**依赖方向**（谁能提到这个 crate），而不是"这个 crate 有没有被用到"，因此不会因为新的测试用例而误报。
- **风险**：`#[global_allocator]` 会作用于整个进程，包括测量代码自己。
  **护栏**：`Counting` 不分配；计数用原子而非锁；文档写明测试必须独占进程。此外本 crate 的用途是**计数**，不是限制 —— 它不会让任何东西失败，只有调用方读出的差值才会。
- **残余风险**：`Relaxed` 原子在多线程下不构成 happens-before 关系，因此一个跨线程的测量窗口读到的计数可能滞后。解码路径是单线程串行的（`ASM-11`），这不是问题；但若将来有人用它测一条并发的路径，这个前提必须重新检查。已写进 `allocations()` 的文档。

## 如何变更

`AGENTS.md` 的修改需要用户确认（该文件末尾的维护约定）。要再增加一条 `unsafe` 路径，或要把 `alloc-count` 从测试件变成发布依赖：

1. 在 `docs/dev/adr/` 新增一份 ADR，编号从 [README.md](README.md) 的索引取下一个空闲号（**不得复用、不得重排**）；
2. 经**用户**确认后改 `AGENTS.md`；
3. 再改 `scripts/check-unsafe.sh` 与 `scripts/check-deps.sh`。先改脚本后改文档会让门禁短暂地与文档不符，而 `AGENTS.md` §2 要求文档是权威。
