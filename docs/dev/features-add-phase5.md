# dev-features-add — Round-2 Phase 5 增量原子任务分片（P1 功能面与测试暗线）

> 所属主控文档: [./features-add.md](features-add.md) ｜ 关联对比报告: [./compare-add.md](compare-add.md) ｜ 阶段定位: Round-2 Phase 5（P1）｜ 包含任务区间: TASK-5.01.01 ~ TASK-5.05.01（6 卡，预估 ≈13.5 人天）｜ 基线: main@`0ef7d8b`
>
> 阶段目标：补齐用户可感知的「高频小功能面」（快捷输入族、自定义双拼布局），并把测试暗线的两个已知缺口（无头渲染回归、冒烟断言维度）闭环。全阶段遵守 `ASM-A-13`（时钟/随机源注入）、`ASM-A-15`（Xvfb 档定位）、`ASM-A-16`（求值安全）。

---

#### TASK-5.01.01 快捷域底座（ClockSource/RngSource 注入 + 域骨架 + 解析器依赖）

- **基本属性**：
  - 绑定差距与改进项：`GAP-23 / IMP-24`（底座）｜ 关联设计假设：`ASM-A-13`、`ASM-A-16`、`ASM-A-12`
  - 差距类型：`架构与测试基建`
  - 优先级与复杂度：`P1 效率进阶` ｜ 中 ｜ 预估工时: 2 人天
  - 前置依赖：`无`（与 Phase 4 无文件交集；`engine/host.rs` 为 ime-fcitx5 内部 trait，非冻结契约）
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine·Resilience`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `date_translator.lua:32-132`（触发键/输出契约，SCOUT-ICE-05）、`unicode.lua:12-33`（码点域，SCOUT-ICE-22）；其 `os.time()` 直读被 `ASM-A-13` 否决——时钟必须注入
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-fcitx5/src/engine/host.rs`（ClockSource/RngSource 关联 trait）、`crates/ime-core/src/quick.rs`（新建：快捷域判定与候选骨架）、`crates/ime-config/src/schema/config.rs`（`[quick_input]` 节）、根 `Cargo.toml`（`[workspace.dependencies]` 预注册表达式解析器）
  - 引入依赖白名单：`meval <pin 最新稳定>`（数学表达式解析，纯求值无 IO/无 exec；预审结论：无网络、无 proc 依赖、维护活跃——AGENTS §3.5 审计后登记）。`TASK-5.01.02` 消费
  - 当前状态：`[ ] 待开始`
- **目标与价值**：快捷输入族（日期/时间/大写数字/金额/计算器/码点）的公共底座：触发判定（前缀域识别 O(1)）、时钟与随机源的注入通道、候选产出契约、以及表达式解析器的依赖注册。rime-ice 的七类翻译器机制级转写为引擎原生能力，但**纯函数红线不破**：一切时间/随机读取经宿主注入。
- **技术设计与代码级接入细节**：
  - **宿主注入 trait（ime-fcitx5 内部边界，随 crate 发布）**：

```rust
// crates/ime-fcitx5/src/engine/host.rs —— 追加到既有内部 host trait 家族
/// Wall-clock access for the quick-input domain. The decoder stays pure:
/// it receives `ClockSnapshot` values, never reads the clock itself.
pub trait ClockSource {
    /// Current civil time broken into fields the translators consume.
    fn now(&self) -> ClockSnapshot;
}

/// Randomness for the (optional) uuid-style generator; injectable so tests
/// pin the sequence. Default host impl uses an OS-seeded PRNG.
pub trait RngSource {
    fn fill_bytes(&self, dest: &mut [u8]);
}

pub struct ClockSnapshot {
    pub year: i32, pub month: u8, pub day: u8,
    pub hour: u8, pub minute: u8, pub second: u8,
    pub weekday: u8, // 0=Monday .. 6=Sunday
}
```

  - **快捷域判定（ime-core 侧，纯函数）**：输入缓冲前缀匹配 `rq/sj/xq/dt/ts/R/U/cC` 域引导符（可配置），未命中 O(1) 返回 `None`（零开销红线）；命中后进入域解析，产出 `QuickCandidate { text, annotation }` 列表，quality 域内置置顶权（对齐 rime-ice `quality=100` 语义，映射为 Q8.8 权重通道）。
  - **配置节**：`[quick_input] enabled=false` + `date_prefixes = ["rq","dt"]` 等前缀表 + `calc_enabled=false`；默认全关（`ASM` 开箱即现状）。
  - **解析器依赖注册**：`[workspace.dependencies]` 登记 `meval`，带用途尾注（AGENTS §3.1 纪律）；本卡不消费，`TASK-5.01.02` 消费。
- **底层与非功能约束 (NFR)**：未命中路径零分配、零查表（前缀 trie 或首字节分派）；命中路径独占预算 ≤0.30ms（§8.6）；时钟缺席（宿主未提供 trait 实现）→ 时间域整体不产出候选（回退现状，不报错刷屏）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 注入通道]`：host trait 两件 + 宿主侧实现（fcitx5 线程安全快照）+ 固定 Clock 测试 fixture。
  2. `[步骤 2 - 域骨架]`：`quick.rs` 前缀判定 + QuickCandidate 契约 + `[quick_input]` 节 + 防漂移测试。
  3. `[步骤 3 - 依赖注册]`：workspace 依赖登记 + `cargo check` 确认解析闭包无新增网络/异步能力（check-no-network 门禁自证）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：前缀判定 O(1) 路径与命中路径单测；时钟缺席降级测试；配置防漂移测试。
  - [ ] 物理产物验证：`just check-net`/`check-deps` 确认新依赖无网络能力；审计脚本全绿。
  - [ ] 编译与旧功能回归：默认关下冒烟 202 例全绿；四门禁零警告。

#### TASK-5.01.02 七类快捷翻译器实现

- **基本属性**：
  - 绑定差距与改进项：`GAP-23 / IMP-24`（主体）｜ 关联设计假设：`ASM-A-13`、`ASM-A-16`
  - 差距类型：`新增功能`
  - 优先级与复杂度：`P1 效率进阶` ｜ 中 ｜ 预估工时: 3 人天
  - 前置依赖：`TASK-5.01.01`（域骨架与注入通道）
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine·Resilience`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `date_translator.lua:32-132`（rq/sj/xq/dt/ts 七前缀多格式）、`number_translator.lua:114-161` + `convert_ar_num_to_zh.lua`（四式大写 + 壹拾会计特判，SCOUT-ICE-06）、`calc_translator.lua:208-222`（双候选契约：值 + expr=value；其 `load()` 动态执行被否决）、`unicode.lua:12-33`（码点直输 + 后 16 码点联想 + 超限防崩，SCOUT-ICE-22/24）
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-core/src/quick/`（`date.rs`/`number.rs`/`calc.rs`/`codepoint.rs` 四模块）、`crates/ime-fcitx5/src/engine/router/`（快捷域按键消费与选择上屏）
  - 引入依赖白名单：`meval`（已在 TASK-5.01.01 注册，本卡消费）
  - 当前状态：`[ ] 待开始`
- **目标与价值**：交付六类高频快捷能力（农历/UUID 列为**非目标**，扩展点保留——compare-add.md §8 反膨胀裁决）：`rq` 日期四格式、`sj` 时间（含时段中文）、`xq` 星期、`dt` ISO 8601、`R+数字` 小写/大写/金额小写/金额大写四式（含「壹拾X」会计规范）、`U+hex` 码点直输（附后 16 码点联想 + 超 `0x10FFFF` 报 `quick/expr/invalid` 防崩）、`cC+表达式` 计算器（值与 `expr=value` 双候选）。所有算法自实现（rime-ice GPL 代码零拷贝，仅机制与输出格式契约转写）。
- **技术设计与代码级接入细节**：
  - **数字大写核心（纯算法直译，含会计特判）**：

```rust
// crates/ime-core/src/quick/number.rs
/// Four renderings of an integer amount: plain, upper (大写), currency
/// plain, currency upper. `upper` follows 会计规范 incl. the 壹拾X rule
/// (10..19 start with 壹拾, never 拾X). Input length capped at 16 digits.
pub enum AmountStyle { Plain, Upper, CurrencyPlain, CurrencyUpper }
pub fn render_amount(digits: &str, style: AmountStyle) -> Option<String>;
```

  - **计算器安全边界**：`meval` 求值前做长度（≤64 字符）与字符白名单检查；解析失败/除零/超时 → 单条候选回显原式 + annotation `quick/expr/invalid`，**绝不 panic、绝不阻断会话**（`ASM-A-16`）。
  - **码点域**：`U` + 1..=6 位 hex → 码点字符候选 + 注释 `U4F60`；合法码点后附最多 16 个后继码点联想；越界（>0x10FFFF、代理区）回 invalid 诊断。
  - **时间域**：全部字段来自注入的 `ClockSnapshot`；测试以固定快照钉死全部格式输出（`rq` 四式、时段中文五段、`xq` 三式、`dt` 时区规整）。
- **底层与非功能约束 (NFR)**：命中路径合计 ≤0.30ms（域内一次解析+渲染）；候选数 ≤8/域；选择上屏走既有 commit 通道；annotation 走既有注释位（无新 UI 面，`ASM-A-12`）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 纯算法族]`：`number.rs` + `codepoint.rs`（无时钟依赖）+ 全量格式钉死测试。
  2. `[步骤 2 - 时钟族]`：`date.rs`（rq/sj/xq/dt）注入 ClockSnapshot + 固定快照测试。
  3. `[步骤 3 - 计算器与接线]`：`calc.rs`（meval 消费 + 安全边界）+ router 域内按键（选择/退格/取消）+ X11 档手验。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：六域格式输出钉死（含边界：0、负数拒绝、16 位上限、代理区、除零、超长表达式）。
  - [ ] 物理产物验证：X11 档真实会话逐域手验（触发→候选→选择上屏）；截图证据入 `results/runs/`。
  - [ ] 编译与旧功能回归：默认关冒烟全绿；`check-unsafe`/`check-net` 全绿（meval 无 unsafe 引入）。

#### TASK-5.02.01 自定义双拼布局数据化

- **基本属性**：
  - 绑定差距与改进项：`GAP-24 / IMP-25` ｜ 关联设计假设：`ASM-A-11`（布局格式机制转写，表数据自建）
  - 差距类型：`新增功能`
  - 优先级与复杂度：`P1 效率进阶` ｜ 中 ｜ 预估工时: 2.5 人天
  - 前置依赖：`无`
  - 关键路径：`否`
  - 并行通道：`Track A Core·Engine·Resilience`
  - 对标参考锚点 (Benchmark Reference)：WindInput `data/schemas/shuangpin/*.toml` 三表模型 + `official_zero_initial_strokes_work` 击键覆盖测试 + 「零声母两大流派不可互抄」注释规范（SCOUT-D-14）
  - 本项目代码落地锚点 (Code Anchor)：`crates/ime-config/src/schema/config.rs`（`[shuangpin.custom]` 节：布局文件路径或内联表）、`crates/ime-core/src/shuangpin/`（布局装载与校验，新 `layout.rs`）、`xtask/`（布局校验子命令挂 quality 旁，可选）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：双拼布局从「6 个硬编码方案（`decode.rs:149-161`，B-11）」升级为「数据定义 + 校验装载」：用户提供 TOML 三表（声母/韵母/零声母）即可新增布局，无需改代码发版。既有 6 方案转为内置默认数据（行为逐字节不变），自定义布局走显式 opt-in。
- **技术设计与代码级接入细节**：
  - **布局数据格式（机制转写，字段自定）**：

```toml
# 自定义双拼布局示例：三表 + 可选零声母风格声明
[layout]
name = "my-flypy"
zero_initial_style = "o-led"   # "o-led" | "first-letter"（两大流派，不可混用）

[initials]  # 声母 -> 单键
zh = "v"; ch = "i"; sh = "u"

[finals]    # 韵母 -> 单键（多映射时分号相隔，校验器拒绝冲突）
ang = "h"; eng = "g"; ...
```

  - **校验器（装载期 fail-fast + 修默认哲学的例外面）**：布局是数据不是键值，错误布局**拒绝装载并回退所选内置方案 + 每条错误一条 `config/invalid` 诊断**（冲突映射、缺韵母、非法键名、两流派混用均显式报错）——「布局宁可不生效也不能错着生效」。
  - **覆盖测试**：内置 6 方案各自生成「全音节可打出性」正向击键表测试（官方击键逐个断言，参照 WindInput 覆盖测试思路，表自建）；自定义布局以同一测试骨架跑用户提供表的 fixture。
- **底层与非功能约束 (NFR)**：装载在配置 reload 时一次完成（O(音节表)），解码热路径仍为查表（零新增开销）；布局表不进日志（无隐私面，但错误诊断只含键名不含用户值全文）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 数据结构]`：`layout.rs` 三表模型 + 校验器 + 错误诊断码。
  2. `[步骤 2 - 内置转默认]`：6 硬编码方案转内置数据文件 + 逐字节等价对照测试（迁移零行为变更）。
  3. `[步骤 3 - 自定义装载]`：`[shuangpin.custom]` 节 + 用户布局 fixture 测试 + 配置文档示例。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：6 内置方案全音节击键覆盖表全绿；校验器错误类逐类测试；迁移等价对照。
  - [ ] 物理产物验证：以小鹤布局为 fixture 走通「用户 TOML → 生效 → 错值回退」全链路（X11 档手验）。
  - [ ] 编译与旧功能回归：默认档冒烟 202 例全绿；四门禁零警告。

#### TASK-5.03.01 数据驱动参数整定与采集纯净度 precheck

- **基本属性**：
  - 绑定差距与改进项：`GAP-25 / IMP-26` ｜ 关联设计假设：`ASM-A-14`、`ASM-A-02`
  - 差距类型：`架构与测试基建`
  - 优先级与复杂度：`P1 效率进阶` ｜ 中 ｜ 预估工时: 2 人天
  - 前置依赖：`TASK-4.03.01`（sweep 参数面包含新暴露的 `context_tie_band_q16` 与 pair 工作点）
  - 关键路径：`否`
  - 并行通道：`Track C Infra·Testability·DevOps`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `others/script/rime/polyphone.go` 数据把关思想（阈值由数据定，不拍脑袋，SCOUT-ICE-18）；WindInput 常量注释「实测依据」纪律（SCOUT-D-08）
  - 本项目代码落地锚点 (Code Anchor)：`xtask/src/tune/`（sweep 扩展）、`xtask/src/dictc/quality.rs`（目标函数复用）、`xtask/src/main.rs`（tune 子命令参数扩展——本 Phase 唯一允许动 Command 枚举的卡）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：归一化（`core_min_freq`/`tail_flat_weight`）、衰减（半衰期）、bigram（`max_pairs`/`min_score`）、上文带（`context_tie_band_q16`）目前全部手工常数（B-03/B-14）。本卡把 `xtask tune` 扩展为「以 holdout top1（分带）为目标函数的网格 sweep」，产出参数推荐表并回写默认值；同时把「空闲机采集」从纪律升级为 precheck 门禁（load/干扰进程探测，B-14）。
- **技术设计与代码级接入细节**：
  - **sweep 骨架**：参数空间声明式（TOML：参数名 → 网格点），每点跑 `dictc`（如涉及编译参数）+ `quality` 评估，输出按 `top1_overall`（tie-break：`top1_2char` → 编译时长）排序的推荐表 + 参数敏感性摘要；确认后的推荐值由人工裁决回写默认（工具只推荐不自动改默认——门禁值只升不降的裁决权在人）。
  - **采集纯净度 precheck**（bench/tune/soak 共用）：

```rust
/// Gates every latency-sensitive collection path. Fails closed: a dirty
/// machine must produce an explicit refusal, never a misleading number.
pub struct PurityCheck { pub load_1m: f32, pub cpu_count: u32 }
impl PurityCheck {
    /// Refuse when 1m load exceeds 25% of cores (ASM-T-11 discipline,
    /// now enforced at the tool boundary instead of by convention).
    pub fn verify(&self) -> Result<(), PurityRefusal> { /* ... */ }
}
```

  - 预算敏感路径（`just bench`/`tune sweep`/`soak`）统一先过 `verify()`，拒绝时给可行动指引（关负载/降低网格密度）。
- **底层与非功能约束 (NFR)**：sweep 单点评估复用既有 9.8s 编译门与 quality 评估（总时长网格规模线性）；precheck 为 O(1) 系统调用。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - precheck]`：PurityCheck + 接入 bench/tune/soak 三入口 + 拒绝路径测试。
  2. `[步骤 2 - sweep 引擎]`：参数空间 TOML + 网格执行 + 推荐表产出。
  3. `[步骤 3 - 首轮整定]`：对四组参数跑首轮 sweep，推荐表与裁决记录进 `docs/dev/research/`；回写经裁决的默认值 + 门禁值更新。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：sweep 网格解析/排序确定性测试；precheck 拒绝路径测试。
  - [ ] 物理产物验证：空闲机上首轮推荐表落盘；默认值回写后 holdout top1 ≥ 回写前（门禁只升不降）。
  - [ ] 编译与旧功能回归：负载机上 `just bench` 显式拒绝（不再误报）；四门禁零警告。

#### TASK-5.04.01 Xvfb 无头渲染回归层

- **基本属性**：
  - 绑定差距与改进项：`GAP-26 / IMP-27` ｜ 关联设计假设：`ASM-A-15`
  - 差距类型：`架构与测试基建`
  - 优先级：`P1 效率进阶` ｜ 复杂度：中 ｜ 预估工时: 2 人天
  - 前置依赖：`无`（xtask 内增量）
  - 关键路径：`否`
  - 并行通道：`Track C Infra·Testability·DevOps`
  - 对标参考锚点 (Benchmark Reference)：WindInput `scripts/linux/e2e.sh`（Xvfb + dbus-run-session 全隔离、探活走真实通路不 sleep、xcompmgr 下合成截图不可信的坑位记录，SCOUT-F-12）
  - 本项目代码落地锚点 (Code Anchor)：`xtask/src/testd/`（新增 `xvfb` 通道模块：会话编排 + DISPLAY 注入）、`justfile`（`check-e2e-xvfb` 菜谱）
  - 引入依赖白名单：`无`（Xvfb/xvfb-run 为系统包，属环境依赖不属 Cargo 依赖；CI 缺包时按 `NeedsEnvironment` 三态记录）
  - 当前状态：`[ ] 待开始`
- **目标与价值**：E2E 渲染回归（XTEST 注入/截图/golden 对比）目前依赖真 X server（B-08），CI 环境跑不了。本卡为 testd 增加第三执行通道：在 Xvfb 虚拟显示里拉起完整会话跑既有 smoke/acceptance 用例，让渲染回归进 CI。**定位边界**（`ASM-A-15`）：Xvfb 档只挡「渲染不崩、几何稳定、截图可采」，视觉工艺终审仍在真机 X11 档。
- **技术设计与代码级接入细节**：
  - **通道编排**：`xvfb-run`（或自管 `Xvfb :N -screen 0 1280x800x24`）→ 注入 `DISPLAY` → 拉起沙盒会话（复用既有 testd sandbox 编排）→ 既有 suite/screenshot/golden 流程零改动跑在虚拟屏上。
  - **探活纪律（WindInput 教训转写）**：等待就绪用「真实按键通路探活」（注入探测键直到被消费）而非 sleep 定时；虚拟屏分辨率/色深固定为已知值，golden 对比仅在**同分辨率**间进行。
  - **三态门禁对接**：`env_gate` 探测 Xvfb 可用性（`which Xvfb`），不可用 → 用例记 `NeedsEnvironment { remedy: "install xvfb …" }`，不算失败（`CaseEligibility` 既有机制消费）。
- **底层与非功能约束 (NFR)**：Xvfb 通道对用例零改造（既有 TSV/suite 定义原样复用）；单用例执行耗时增量 ≤1.5×（虚拟屏 vs 真屏基线，超出记环境问题不记回归）。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 通道编排]`：`testd/xvfb.rs` 会话编排 + DISPLAY 注入 + 探活。
  2. `[步骤 2 - 门禁对接]`：env_gate 探测与三态记录 + `just check-e2e-xvfb` 菜谱。
  3. `[步骤 3 - CI 首跑]`：smoke 层全量在 Xvfb 下跑通；flaky 排查（虚拟屏时序）与已知坑位注释。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：通道编排单测（DISPLAY 注入/探活超时/清理路径）。
  - [ ] 物理产物验证：Xvfb 下 smoke 层全绿且 `results/runs/` 产出含截图证据的 v3 账本；无 Xvfb 环境三态记录正确。
  - [ ] 编译与旧功能回归：真 X 通道行为零变化；四门禁零警告。

#### TASK-5.05.01 冒烟 TSV v2（多维断言 + 开档矩阵）

- **基本属性**：
  - 绑定差距与改进项：`GAP-27 / IMP-28` ｜ 关联设计假设：`ASM-A-11`（断言语法机制转写）、`ASM-A-12`
  - 差距类型：`架构与测试基建`
  - 优先级：`P1 效率进阶` ｜ 复杂度：中 ｜ 预估工时: 2 人天
  - 前置依赖：`TASK-4.05.01`（开档矩阵需联想等 Round-2 行为就绪后才能钉）
  - 关键路径：`否`
  - 并行通道：`Track C Infra·Testability·DevOps`
  - 对标参考锚点 (Benchmark Reference)：rime-ice `others/script/smoke/cases/*.tsv`（`@today:%Y-%m-%d` 时间相对断言 + `@regex:` 版本位断言 + stderr E 级即失败的纪律，SCOUT-ICE-17）
  - 本项目代码落地锚点 (Code Anchor)：`tests/smoke/cases.tsv`（v2 列扩展）、`xtask/src/testd/smoke.rs`（断言引擎扩展）
  - 引入依赖白名单：`无`
  - 当前状态：`[ ] 待开始`
- **目标与价值**：202 例冒烟全部只锚首选（`@first:`，B-09）：top3/top5 命中、页序稳定性、以及 fuzzy/abbrev/correction/english/assoc **开档行为**无任何回归 pin——排序类改动（如 TASK-4.02/4.03）只能靠人工对照。本卡把冒烟 TSV 升级为 v2：多维断言 + 开档矩阵 + 动态断言，使排序与功能改动有自动回归网。
- **技术设计与代码级接入细节**：
  - **TSV v2 列契约（向后兼容：v1 行原样合法）**：

```text
case_id	scenario	flags	expect
tc-rank-001	default	@top3:你好|拟好	ni hao
tc-date-001	quick	@regex:^\d{4}-\d{2}-\d{2}$	rq
tc-date-002	quick	@today:%Y-%m-%d	rq
tc-assoc-001	assoc-on	@first:国家	zhongguo <commit> <assoc-select-1>
```

  - **断言引擎扩展**：`@top3:`（候选前 3 含序敏感集合）、`@today:<fmt>`（按注入 Clock 的当前时间渲染后精确比对——时钟来自 testd 可控 fixture，测试确定性不破）、`@regex:`（锚定文本模式）；`flags` 列声明场景开档（复用场景 schema v2 的 `[decode] flags`/`[assoc]` 机制，features-test P0.02.01 交付面）。
  - **开档矩阵**：为 fuzzy（8 类抽 3）、abbrev、correction、english、assoc 各写 ≥3 条「开档行为」用例 + 对应「默认关 = 现状」对照用例，共约 30 条新增；总量 ≤250 例维持冒烟时长门。
- **底层与非功能约束 (NFR)**：smoke 层总时长增量 ≤30%；动态断言不引入真时钟（全部经注入 Clock）；E 级日志即失败的既有纪律沿用。
- **渐进可编译实施步骤**：
  1. `[步骤 1 - 断言引擎]`：`@top3/@today/@regex` 三断言器 + v1 兼容回归（202 例零改动通过）。
  2. `[步骤 2 - 开档矩阵]`：场景 fixture + 30 条新用例编写（期望值全部从实测行为钉出，不臆造）。
  3. `[步骤 3 - 对照确认]`：对 TASK-4.02/4.03 的改动跑新基线确认能捕获行为差异（变异测试式抽查：人为反转一处排序，断言必须变红）。
- **验收标准 (DoD)**：
  - [ ] 自动化测试：三断言器单测（含 `@today` 的注入时钟确定性）。
  - [ ] 物理产物验证：≥232 例冒烟全绿；变异抽查（≥2 处人为变异被捕获）记录进验收记录。
  - [ ] 编译与旧功能回归：smoke 层时长在门内；四门禁零警告。

---

## Phase 5 汇总

| 任务 | 能力 | 通道 | 工时 | CP |
|---|---|---|---|---|
| TASK-5.01.01 快捷域底座 | GAP-23 | A | 2 | 否 |
| TASK-5.01.02 七类快捷翻译器 | GAP-23 | A | 3 | 否 |
| TASK-5.02.01 自定义双拼布局 | GAP-24 | A | 2.5 | 否 |
| TASK-5.03.01 数据驱动整定 | GAP-25 | C | 2 | 否 |
| TASK-5.04.01 Xvfb 无头层 | GAP-26 | C | 2 | 否 |
| TASK-5.05.01 冒烟 TSV v2 | GAP-27 | C | 2 | 否 |

**波次建议**：W1 = {5.01.01, 5.02.01, 5.04.01}（文件互不相交）；W2 = {5.01.02, 5.03.01}（分别依赖底座与 4.03.01）；W3 = {5.05.01}（依赖 4.05.01 与 W2 的新行为面）。出口准则：六域快捷输入在 X11 档可演示；Xvfb 层 CI 可跑；冒烟 ≥232 例含开档矩阵；四门禁全绿。
