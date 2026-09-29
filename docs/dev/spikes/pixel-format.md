# Spike 结论：软件光栅的像素格式与缓冲布局

> 状态：**代码侧结论已完成；`MockBackend` 上的像素断言可跑，真机四档截图未做**（原因见 §4）。
> 本文件按 `TASK-1.05.01` 的验收标准 7 交付。
> 关联模块：`MOD-UI` ｜ 风险编号：R-05（本 spike 即该风险落点）
> 代码落点：`crates/ime-ui/src/renderer/raster.rs`、`crates/ime-ui/src/renderer.rs`、
> `crates/ime-ui/src/platform/wayland/shm.rs`、`crates/ime-ui/src/platform/x11.rs`、
> `crates/ime-ui/src/slint_platform.rs`

## 0. 环境

| 项 | 值 | 来源 |
|---|---|---|
| Slint | 1.13（软件渲染器 `SoftwareRenderer`） | `[workspace.dependencies]` / `Cargo.lock` |
| 目标字节序 | **小端**（`x86_64` / `aarch64`） | 项目交付平台 |
| 渲染器 | Slint `software_renderer`，**非** femtovg / skia | `slint_platform.rs` |
| Rust | edition 2024 / MSRV 1.85 / toolchain 1.98.0 | `rust-toolchain.toml` |
| 无显示服务器测试 | 全部通过 | `cargo nextest run -p ime-ui` |

## 1. 三个验证问题的结论

### (a) 一个像素类型能否同时服务 Wayland 与 X11 —— **可以，且不需要转换**

- `wl_shm` 缓冲以 `WL_SHM_FORMAT_ARGB8888` 创建，每个像素是一个 `0xAARRGGBB` 字；
- 32 位 X11 `TrueColor` visual 的每像素同样是一个 `0xAARRGGBB` 字；
- 在小端主机上，这两者的**字节序都是 `B, G, R, A`**。

因此两个后端共用一个像素类型：`raster::Argb8888Pixel`，按 surface 的字节序存放通道。

### (b) Slint 的 `TargetPixel` 能否直接实现 —— **可以，这正是选它的理由**

Slint 1.13 只为 `Rgb8Pixel`、`Rgb565Pixel`、`PremultipliedRgbaColor` 三个类型实现了 `TargetPixel`。
其中只有 `PremultipliedRgbaColor` 带 alpha，而它按 `R, G, B, A` 存放通道 —— **与 surface 的顺序相反**。

若直接用它，拷进 surface 缓冲时就必须逐像素重排通道。本项目改为**在 surface 的字节序上实现
`TargetPixel`**（`raster.rs:70` 的 `impl TargetPixel for Argb8888Pixel`），于是：

- 拷进 surface 缓冲是**一次纯字节拷贝**，没有任何通道被重排；
- "格式一致"成了**类型的性质**，而不是一件每次都要对着 spike 重新核对的事。

这是本 spike 唯一改变了实现的结论，其余两条都只是确认。

### (c) 为什么需要一个 scratch —— **因为 `ime-ui` 不在 `unsafe` 白名单里**

surface 缓冲到手时是 `&mut [u8]`。把字节切片视作四字节像素切片，需要为对齐与长度做论证，
而那意味着 `unsafe`。`unsafe` 只允许出现在 `crates/ime-fcitx5/src/ffi/**`、
`crates/ime-ui-addon/src/ffi/**` 和 `crates/ime-dict/src/mmap.rs`（`AGENTS.md` 0.4 规则 3）。

因此帧先光栅化进本 crate 自己拥有的 scratch（`raster::FrameScratch`），事后再整体拷过去。
代价是一次全帧拷贝；收益是这个 crate 保持零 `unsafe`，而这由门禁断言：
`grep -rn unsafe crates/ime-ui/src/` 无输出。

**scratch 的两处富余**（`STRIDE_SLACK = 2` 像素、`ROW_SLACK = 1` 行）不是随手加的：
Slint 断言它拿到的缓冲覆盖由根元素几何推出的窗口尺寸，而那个尺寸经过一次
逻辑 → 物理 → 逻辑的往返（`width_px` → `width_px / scale` → `(width_px / scale) * scale`）。
对本项目定位的整数缩放因子该往返是精确的，但**一次向上取整的舍入就会在渲染器内部 abort**，
所以 scratch 带一点余量。这是防 abort 的余量，不是布局的一部分。

## 2. 布局结论

| 量 | 结论 | 代码位置 |
|---|---|---|
| 每像素字节数 | 4 | `raster::BYTES_PER_PIXEL`（引用 `platform::BYTES_PER_PIXEL`，**只有一处定义**） |
| 通道顺序（内存中） | `B, G, R, A` | `Argb8888Pixel::to_bytes` |
| 通道顺序（字中） | `0xAARRGGBB` | `Argb8888Pixel::pack` |
| 预乘 | **是**，全部通道预乘 alpha | `Argb8888Pixel::pack` 的文档与用例 |
| 全透明像素 | `[0, 0, 0, 0]` | `Argb8888Pixel::TRANSPARENT` |
| 行距 | `width_px * 4 + STRIDE_SLACK * 4` | `FrameScratch` |

## 3. 验证方法

### (a) 单元层（可跑，无显示服务器）

- `raster.rs` 的 `#[cfg(test)]` 用例断言：`TRANSPARENT.to_bytes() == [0,0,0,0]`、
  `from_rgb` 产出不透明像素、`pack` 的预乘算术、以及**逐字节**的往返；
- `MockBackend` 上的像素断言（`TASK-1.05.01` DoD 1）：圆角外的 alpha = 0；
  矩形中心的颜色与 `.slint` 声明的值一致（±1/255）；
- 一致性校验（`TASK-1.05.07` DoD 4）：`hit_map` 与 `.slint` 实际元素坐标的偏差 ≤ 1 物理像素。

### (b) 真机层（本机不可执行）

`.slint` 测试组件在真实 X11 与 Wayland(wlroots) 下的截图与预期图比对（DoD 2）需要一台真机；
Wayland 侧另见 [`wayland-tiers.md`](./wayland-tiers.md) §2 —— 本机没有可用合成器。

## 4. 本机实测记录

| 命令 | 结果 |
|---|---|
| `cargo nextest run -p ime-ui` | 全绿（含 `raster.rs` 的逐字节断言） |
| `grep -rn unsafe crates/ime-ui/src/` | **无输出**（DoD 6） |
| 真机 X11/Wayland 截图比对 | **未执行 —— 需要真机** |

## 5. 已知限制

1. **结论只在小端主机上成立**。`0xAARRGGBB` 与字节序 `B, G, R, A` 的对应关系是本文件
   §1a 推导的，大端主机上不成立；项目交付平台全是小端，故不构成缺陷，但若将来移植需重做本 spike。
2. **DoD 2 的真机截图比对未做**（本机无可用 Wayland 合成器；X11 侧需要一次真实的候选框会话）。
3. **DoD 4 的内存上限（≤ 18MB RSS）与 DoD 5 的空闲零重绘未在本机取数**：
   两者都需要跑满 1000 帧 / 静止 10 秒的进程级测量，而基准数字在 20 个并发 agent 的环境下不可信
   （见 `.dev-progress.json` 的阻塞项）。
4. `STRIDE_SLACK` / `ROW_SLACK` 是**防御性**的：它们掩盖了"Slint 的尺寸往返出现一次舍入误差"
   这一情形，使它在渲染器里表现为静默使用富余区，而不是 abort。这是有意的取舍（abort 会让整个
   输入法失去候选框），但代价是这类舍入误差不会自己冒出来，需要别的手段发现。
