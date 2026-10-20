# rspinyin 测试用例分片 · infra（门禁、打包与测试平台）

> 分片版本: v3.0 ｜ 主文档: [../tests.md](../tests.md) ｜ 执行器必读: [../test-architecture.md](../test-architecture.md) ｜ 模块形态: Headless（E2E 卡为 GUI 面入口）｜ 逻辑自检: 已通过 ｜ 维护约定: 用例状态变更须同步主文档矩阵
>
> 基线引用：`ASM-T-04/06/07/08/11`。模块背书 = `just ci` + `cargo nextest -p xtask`（1,209 测）+ `testd-suite`。**吸收** 字段登记取代的 v2.0 用例号。

---

#### TC-INFRA-001 质量门禁四命令

- **基本属性**：`REQ-INFRA-01` ｜ `ASM-T-11` ｜ infra ｜ ① ｜ `just ci` ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `requires_screenshots: false` ｜ `[]` ｜ 吸收：TC-INFRA-01~05、59 ｜ 锚点：`justfile`（`ci: check audits check-host smoke`）、`.github/workflows/ci.yml` ｜ 背书：CI 矩阵
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：干净工作树；空闲机器（性能基准档）。
- **操作步骤与物理存盘动作**：
  1. `cargo fmt --all -- --check` → `execution_log.txt`。
  2. `cargo clippy --workspace --all-targets --all-features -- -D warnings` → 记录。
  3. `cargo nextest run --workspace --all-features` + `cargo test --workspace --doc` → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：零警告、全绿；`check-host` 缺依赖时给出可读指引（`just check-host`）。
  - **[Headless 断言]**：CI 作业矩阵覆盖。
  - **[性能断言]**：基准档在空闲机器采信（`ASM-T-11`）。

#### TC-INFRA-002 审计脚本族自测

- **基本属性**：`REQ-INFRA-02/08` ｜ `ASM-T-07` ｜ infra ｜ ①② ｜ `just audits` ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-INFRA-06~10、51~55、61 ｜ 锚点：`scripts/check-*.sh`（11 脚本族）、`justfile audits` ｜ 背书：各脚本 `--self-test`
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：干净工作树 + 各脚本 fixture 自测。
- **操作步骤与物理存盘动作**：
  1. 七个新脚本（no-grab/fsync-rename 等）自测齐备 → `execution_log.txt`。
  2. 自测无副作用（fixture 树临时目录）；不依赖 `Cargo.lock` 文本匹配 → 记录。
  3. 输出可操作（给出修复命令）；退出码语义一致；`just audits` 失败聚合 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：焦点 grab 禁令机器化；fsync-rename 纪律机器化。
  - **[Headless 断言]**：违规 fixture 必红、干净树必绿。
  - **[性能断言]**：全族 ≤ 60s。

#### TC-INFRA-003 unsafe 边界

- **基本属性**：`REQ-INFRA-03` ｜ `ASM-T-09` ｜ infra ｜ ① ｜ `check-unsafe.sh` ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-INFRA-11~15 ｜ 锚点：`scripts/check-unsafe.sh`、workspace lints（`unsafe_code = "deny"`）｜ 背书：脚本自测
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：干净工作树。
- **操作步骤与物理存盘动作**：
  1. `unsafe` 仅在四处允许目录（`ime-fcitx5/ffi`、`ime-ui-addon/ffi`、`ime-dict/mmap.rs`、`alloc-count/`）→ `execution_log.txt`。
  2. 每个 `unsafe` 块有 `// SAFETY:`；allowlist 例外有据 → 记录。
  3. release 产物不依赖 `alloc-count` → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：白名单外出现 `unsafe` 即红。
  - **[Headless 断言]**：`unsafe_code = "deny"` 生效（FFI/mmap 局部豁免）。
  - **[性能断言]**：不适用。

#### TC-INFRA-004 版本一致性

- **基本属性**：`REQ-INFRA-04` ｜ `ASM-T-01` ｜ infra ｜ ① ｜ `xtask check-versions` ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-INFRA-16~20 ｜ 锚点：`xtask/src/versions.rs`、`packaging/*.conf` ｜ 背书：versions 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：干净工作树。
- **操作步骤与物理存盘动作**：
  1. conf ↔ `Cargo.toml` 版本一致；漂移被检出 → `execution_log.txt`。
  2. `RSPINYIN_ABI_VERSION` 冻结；词库格式版本兼容语义（v1 永久可读）→ 记录。
  3. `rust-toolchain.toml` 固定 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`check-versions` 进 CI（TC-INFRA-41 口径）。
  - **[Headless 断言]**：格式 v2 读端兼容 v1（`TC-ADD-005` 联动）。
  - **[性能断言]**：不适用。

#### TC-INFRA-005 预算表与 budgets.json 一致性

- **基本属性**：`REQ-INFRA-05` ｜ `ASM-T-04/06` ｜ infra ｜ ①④ ｜ `xtask budget --validate` ｜ `[Smoke 冒烟门禁]` ｜ P0 ｜ `false` ｜ `[]` ｜ 吸收：TC-INFRA-21~25 ｜ 锚点：`xtask/src/budget.rs`、`budget/{schema,spec}.rs`、`docs/dev/budgets.json` ｜ 背书：budget 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：`features.md` 0.5.3 与 `budgets.json` 双源在位。
- **操作步骤与物理存盘动作**：
  1. 结构校验（schema v1，21 thresholds）→ `execution_log.txt`。
  2. 与 0.5.3 表格逐项一致（脚本比对）；`Cargo.lock` 已提交 → 记录。
  3. 超限即失败（非告警）；`Missing ≠ Pass`；源码无第二份硬编码阈值 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：`xtask budget --measure` 对 `dist/` 产物断言两个 `size_mb` 阈值。
  - **[Headless 断言]**：未测量预算不得计为通过。
  - **[性能断言]**：不适用。

#### TC-INFRA-006 安装布局与打包

- **基本属性**：`REQ-INFRA-06` ｜ `ASM-T-06` ｜ infra ｜ ①② ｜ `xtask install` + `just package` ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-INFRA-31~45、55~57、62 ｜ 锚点：`packaging/{aur,debian,rpm}`、`xtask/src/{install,package}` ｜ 背书：install/package 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：`DESTDIR`/`PREFIX` 临时安装树。
- **操作步骤与物理存盘动作**：
  1. 安装布局与动态路径解析；双 addon conf 齐备 → `execution_log.txt`。
  2. 前端依赖 Optional；卸载可逆；幂等安装与 `--dry-run` → 记录。
  3. 体积预算（`so_stripped`）与 `nm -D` 导出校验；ELF 检查与图标缓存；发行包规格抽检 → `assertions.json`（budget_keys: `so_stripped`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：不修改 fcitx5 profile；缺失开发包提示可读；离线安装可行；审计脚本在安装后插件上仍过。
  - **[Headless 断言]**：构建产物不含 `target/`。
  - **[性能断言]**：`so_stripped` ≤ 12MB；`base_dict` ≤ 20MB。

#### TC-INFRA-007 场景引擎与沙盒端到端

- **基本属性**：`REQ-INFRA-07` ｜ `ASM-T-01/02` ｜ infra ｜ ①⑤ ｜ `testd-suite` ｜ `[Acceptance 验收套件]` ｜ P0 ｜ `true` ｜ `["01_sandbox_ready.png", "02_first_commit_readback.png"]` ｜ 吸收：TC-INFRA-46~50、48 ｜ 锚点：`xtask/src/testd/{engine,suite,sandbox,evidence,heal,env_gate}.rs`、`tests/fixtures/scenarios/*.toml`（18）｜ 背书：suite 套件
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：X11 会话在位；镜像/读回通道就绪。
- **操作步骤与物理存盘动作**：
  1. 18 场景 TOML 全绿 → 存 `01_sandbox_ready.png` + `execution_log.txt`。
  2. 沙盒会话 XTEST→fcitx5→CommitProbe 闭环（首词上屏读回）→ 存 `02_first_commit_readback.png`。
  3. 证据链与自愈归因演练；三态门禁判定；内存/帧率采样通道 → `assertions.json`。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：证据包结构符合 v3 账本（`P0.01.01` 落地后字段齐备）。
  - **[视觉断言]**：读回 transcript 与镜像一致。
  - **[性能断言]**：单场景执行 ≤ 30s（沙盒复用口径）。

#### TC-INFRA-008 运行期零 socket 与基准纯净度

- **基本属性**：`REQ-INFRA-08` ｜ `ASM-T-09/11` ｜ infra ｜ ④ ｜ `runtime-socket-check` + `testd/purity` ｜ `[Acceptance 验收套件]` ｜ P1 ｜ `false` ｜ `[]` ｜ 吸收：TC-INFRA-54、58 ｜ 锚点：`scripts/runtime-socket-check.sh`、`testd/purity.rs` ｜ 背书：脚本自测
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：运行中沙盒会话；空闲机器。
- **操作步骤与物理存盘动作**：
  1. 运行期零外部 socket（/proc fd 扫描）→ `execution_log.txt`。
  2. 空闲 CPU 预算；并行负载下拒绝采信预算数值 → `assertions.json`（budget_keys: `net_sockets`, `cpu_pct.idle`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**： purity 门禁有拒绝记录输出（remedy 文案）。
  - **[Headless 断言]**：socket 扫描对自身 fixture 检出违规。
  - **[性能断言]**：`cpu_pct.idle` ≤ 0.3%。

#### TC-INFRA-009 长稳压测通道

- **基本属性**：`REQ-INFRA-09` ｜ `ASM-T-06/08` ｜ infra ｜ ④ ｜ `xtask soak` ｜ `[Stress 深度压测]` ｜ P2 ｜ `false` ｜ `[]` ｜ 吸收：TC-INFRA-50（长稳子项）、TC-DICT-42（长稳部分）｜ 锚点：`xtask/src/soak.rs`、`testd/memory.rs` ｜ 背书：soak 报告裁决（`soak --report`）
- 用例状态：`[ ] 未通过`
- **前置条件与沙盒重置契约**：独占机器；模式切换/翻页/点击脚本注入。
- **操作步骤与物理存盘动作**：
  1. 短程（≥1h）连续输入 → `execution_log.txt`（RSS 曲线 + 延迟分布）。
  2. `robustness` 三阈值裁决（无崩溃/RSS 漂移/通过率）→ `assertions.json`（budget_keys: `robustness.*`）。
  3. 8h 完整档 → 子项记「本机不可验证」（`ASM-T-08`）。
- **通过标准与多模态断言契约**：
  - **[功能断言]**：含模式切换、翻页、鼠标点击的混合负载。
  - **[Headless 断言]**：soak 报告由 `xtask soak --report` 机械裁决。
  - **[性能断言]**：RSS 漂移 ≤ `robustness.rss_drift_mb`；通过率 100%（短程口径）。
