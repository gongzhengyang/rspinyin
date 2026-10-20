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
  - 任务状态：`[ ] 待开始`
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
  - 任务状态：`[ ] 待开始`
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
