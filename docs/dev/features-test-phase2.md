# dev-features-test — Phase 2 平台任务分片（P1 断言引擎）

> 所属主控文档: [./features-test.md](features-test.md) ｜ 关联架构规范: [./test-architecture.md](test-architecture.md) ｜ 阶段定位: Phase 2（P1 视觉断言与镜像对齐）｜ 包含任务区间: FEAT-TEST-P1.03.01 ~ FEAT-TEST-P1.04.01（2 卡，预估 3.5 人天）
>
> 逻辑自检: 已通过（七维基线与正则对账闭环）

---

#### FEAT-TEST-P1.03.01 视觉基线差分（golden PNG diff）

- **基本属性**：
  - 绑定平台能力：`MCP-T-13` ｜ 底层驱动框架：`xtask testd`（`capture.rs` + `evidence.rs`）
  - 优先级与复杂度：`P1 断言引擎` ｜ 中 ｜ 预估工时: 2 人天
  - 前置依赖：`FEAT-TEST-P0.01.01`（差分结果写入账本 v3 的 `Claim`）
  - 关键路径：`CP: 否`
  - 并行通道：`Track B 断言引擎`
  - 代码落地锚点 (Code Anchor)：`xtask/src/testd/capture.rs`、`tests/fixtures/golden/`（基线库，新增）
  - 任务状态：`[x] 已完成`
- 落地证据：['契约: VisualJudgement{Structural|GoldenDiff{baseline,max_diff_pixels}} + DiffReport{baseline,diff_pixels,first_diff_rect} 落在 xtask/src/testd/golden.rs；judge(): Structural 不比较，GoldenDiff 载入基线→尺寸不符先拒绝（DimensionMismatch）→逐像素 RGBA 比较（计数+首差异行主序 bbox）；read_rgba 补齐 capture/encode.rs 的 PNG 解码半边', '基线库: tests/fixtures/golden/ 首批 12 张 = default/hover/selected × dark/light × @1x/@2x，由 fixture raster（golden/raster.rs，closed-world 整数运算、无文字无模糊、同参同字节）经 `xtask golden --bless-raster` 生成入库；名称文法 <subject>_<W>x<H>_<theme>[@Sx].png 右锚解析，尺寸/主题/缩放三参数锁进文件名（状态入 subject：grid_hover_…）', '--bless 纪律: 三入口 --bless-raster（fixture 批量）/ --bless-from PNG --as NAME（采集帧）/ --judge PNG --against NAME（含 --max-diff）；bless 拒绝名称与帧尺寸不符（尺寸变更=新名新基线，NameFrameMismatch）；同尺寸重 bless=有意更新，人工执行+验收记录，CI 无 bless 权限', '账本对接: Claim{name:"golden-diff", expect:"diff_pixels <= N", actual:"M differing pixels", pass}；DiffReport 经 AssertionV3.diffs（serde default，旧记录兼容解析）随 assertions.json 结构化落盘；golden tests 含 v3 往返与无 diffs 键兼容测试', '环境相关帧: 模糊/字体回退帧不入库（库内只收构造可复现帧）；此类用例声明 Structural 并在卡注记——跳过是声明不是静默通过', '测试: cargo nextest run -p xtask => 1264 run / 1264 passed / 9 skipped（fmt 后重建热窗实测稳定）；diff 对确定性输入零差异、注入单像素必红（diff=1, box=(x,y,1,1)）、双区域 bbox 跨度正确、缺基线/尺寸不符为两种不同拒绝、名称文法 12 例拒绝、首批 12 名互异', '物理产物: `xtask golden --judge` 对入库基线判 0 diff PASS（实测 grid_hover_1200x280_light@2x.png）；基线 12 文件已入库待提交', '性能: 1200×280@2x 单帧 diff min(20) ≤ 30ms 通过（纯比较计时，PNG IO 不在预算量内）；不进生产路径（xtask 工具链）', '修复记录: 基线名曾不含状态致三状态同名互覆（批次 distinct 断言抓出）→ 状态入 subject；read_rgba 曾用含 filter 字节的 raw_bytes 计量致读回帧多 140 零字节（PNG 往返测试抓出）→ 改用 output_buffer_size+truncate', '偏差1: 首批基线由 fixture raster 而非生产渲染器生成——xtask 不链 slint/ime-ui（与 dict_inject 的 DictFixture 同型 closed-world 折中）；生产渲染器基线经 --bless-from 采集入库，门已开', '偏差2: golden.rs 按 capture.rs 先例带窄化 #![allow(dead_code, unused_imports)]——CLI 未命名的用例侧 API（claim 映射、Structural 变体）由 suite 通道任务落地时消费', 'clippy: xtask 零新警告（manual_repeat_n 已修）；ime-core/ime-dict 存量警告属本次运行前序任务，留待阶段4统一清算']
- **MCP 能力映射与协议报文契约**：`capture_viewport_screenshot` 增判定模式：

```rust
/// How a captured frame is judged. `Structural` keeps today's behaviour
/// (measurements asserted by the case). `GoldenDiff` compares against a
/// committed baseline: the software rasterizer is deterministic, so any
/// pixel delta is a real regression -- no fuzzy thresholds, no perceptual
/// hashing. Scale-dependent baselines live under a scale-suffixed name.
pub enum VisualJudgement {
    Structural,
    GoldenDiff {
        /// Baseline file under tests/fixtures/golden/, e.g. "grid_dark@2x.png".
        baseline: String,
        /// Maximum allowed differing pixels; 0 for the deterministic raster.
        max_diff_pixels: u32,
    },
}

/// The diff record that lands in the case's evidence pack.
pub struct DiffReport {
    pub baseline: String,
    pub diff_pixels: u32,
    /// Bounding box of the first differing region, for one-look triage.
    pub first_diff_rect: (u32, u32, u32, u32),
}
```

- **实现细节**：基线库只收**内存表面探针**与 `capture` 可稳定复现的帧（尺寸/主题/缩放三参数锁定进文件名）；亚克力模糊、字体回退等环境相关帧**不入**基线（沿用旧套件 `[不可验证]` 子项纪律）。基线更新 = 显式 `--bless`（人工执行并留验收记录），CI 中无 bless 权限。深浅色双基线分别落库。
- **逐步落地实施步骤**：
  1. `[步骤 1]`：diff 计算（逐像素 XOR 计数 + 首差异区）与 `DiffReport` 落盘。
  2. `[步骤 2]`：golden 库首批（候选网格 default/hover/selected × 深浅 × @1x/@2x，≤12 张）；`--bless` 流程。
  3. `[步骤 3]`：账本对接（`Claim{name:"golden-diff", pass: diff<=max}`）；环境相关帧的跳过标注。
- **验收标准 (DoD)**：
  - **[单元测试闭环]**：`cargo nextest run -p xtask` 全绿；diff 对确定性输入零差异、对注入的单像素改动必红。
  - **[物理产物验证]**：一次包含 `GoldenDiff` 判定的用例产生 `DiffReport`（assertions.json 内），基线文件入库且 `--bless` 有验收记录。
  - **[性能开销基线]**：单帧 diff ≤ 30ms（1200×280@2x）；不进入生产路径。

---

#### FEAT-TEST-P1.04.01 UiFrame 镜像 schema 与增量面对齐

- **基本属性**：
  - 绑定平台能力：`MCP-R-11` ｜ 底层驱动框架：`xtask testd`（`mirror_cli.rs`）+ `ime-diag/src/uiframe/`
  - 优先级与复杂度：`P1 断言引擎` ｜ 中 ｜ 预估工时: 1.5 人天
  - 前置依赖：`FEAT-TEST-P0.02.01`（消费其场景面产出的新帧字段）
  - 关键路径：`CP: 否`
  - 并行通道：`Track B 断言引擎`
  - 代码落地锚点 (Code Anchor)：`crates/ime-diag/src/uiframe/schema.rs`、`crates/ime-types/src/ui.rs`（契约侧，**只读**）、`xtask/src/testd/mirror_cli.rs`（断言面扩展）
  - 任务状态：`[x] 已完成`
- 落地证据：xtask/src/testd/mirror_cli.rs 与 mirror_cli/tests.rs：新增 --assert-annotation/--assert-full-list/--assert-status 断言面（Expectations::from_args/verdict/ok_lines），11 个新测试；task_check xtask mirror 19/19 绿、ime-diag --features uiframe uiframe 22/22 绿；偏差登记：卡前提 schema v2 过时——uiframe 快照格式已是 format 3，annotation/score/全量候选/status 逐字落盘、写端零丢失，真实缺口是断言 CLI 而非 schema；镜像 JSON 写增量仅 annotation 字符串约 ≤1KB；置顶可见性端到端=add-user-pin 重放 + --assert-full-list 顺序断言
- **MCP 能力映射与协议报文契约**：`runtime://ui_frame` 的镜像 schema 为 features-add 增量字段预留判定面（**镜像 schema 是诊断面，冻结契约 `ime-types` 不动**——新增字段来自既有 `Candidate.annotation`/`StatusStrip`/`score` 的透传，不新增契约类型）：

```rust
/// Mirror-side assertion surface additions. Each maps onto a field the
/// frozen contract already carries; the mirror only stops dropping them.
pub enum AddedMirrorField {
    /// Candidate.annotation now carries correction/fuzzy hints (TASK-3.05.02)
    /// and symbol categories (TASK-3.05.03); the mirror keeps it verbatim.
    Annotation,
    /// A pinned candidate is visible through source/score ordering (TASK-1.04.x);
    /// the mirror records the full candidate list, not just the first page.
    FullCandidateList,
    /// StatusStrip mode label reflects script toggle (TASK-2.03.01).
    StatusStrip,
}
```

- **实现细节**：镜像写入端（ui-addon `mirror.rs`）当前可能裁剪字段以最小化落盘——本卡把上述三类字段改为完整透传并补 schema 版本号（`schema_version: 2`，读端兼容 v1）；`mirror_cli` 增对应断言子命令（`--assert-annotation` / `--assert-full-list` / `--assert-status`）。隐私边界不变：镜像仍不含候选文本（annotation 中的提示短语除外，其为引擎生成、非用户输入）。
- **逐步落地实施步骤**：
  1. `[步骤 1]`：schema v2 字段定义与 ui-addon 写入端扩展；v1 镜像读端兼容。
  2. `[步骤 2]`：`mirror_cli` 三个断言子命令 + 与账本 v3 对接。
  3. `[步骤 3]`：随 P0.02.01 的示例场景做端到端验证（置顶后镜像可见排序变化）。
- **验收标准 (DoD)**：
  - **[单元测试闭环]**：`cargo nextest run -p ime-diag -p xtask` 全绿；schema v1/v2 双读兼容。
  - **[物理产物验证]**：镜像 JSON 出现 `schema_version: 2` 且三类字段可断言；旧镜像文件仍可读。
  - **[性能开销基线]**：镜像写入增量 ≤ 1KB/帧；对帧发布路径无可测延迟增量（`key_to_present_p50` 预算不回退）。

---

## Phase 2 汇总

| 任务 | 能力 | 通道 | 工时 | CP |
|---|---|---|---|---|
| FEAT-TEST-P1.03.01 视觉基线差分 | MCP-T-13 | B | 2 | 否 |
| FEAT-TEST-P1.04.01 镜像 schema 对齐 | MCP-R-11 | B | 1.5 | 否 |

**波次建议**：W1 = {P1.03.01}（仅依赖账本 v3）；W2 = {P1.04.01}（依赖场景 v2 产出增量字段）。两卡文件不相交，可并行。
