//! The suite's case table: one row per P0 baseline case, transcribed from the case document.
//!
//! Responsibility: hold the thirty P0 baseline cases of `docs/dev/tests.md` as data, so that the
//! runner has something to execute and a reviewer has something to compare the document against.
//! Boundaries: this module decides nothing. It does not judge a case, run anything, or read the
//! case document at run time -- the document is read by whoever edits this table, and the table
//! is what the runner executes.
//!
//! # Everything here is copied, and nothing here is invented
//!
//! Each row's [`commands`](CaseLine::commands), [`precondition`](CaseLine::precondition) and
//! [`criteria`](CaseLine::criteria) come out of the case document word for word. A command line
//! keeps the spelling the document gives it -- the same package, the same filter, the same
//! feature -- because the whole value of running the suite is that the command which ran is the
//! command the document states, and a row that had been tidied up would be evidence for a
//! judgement nobody wrote down.
//!
//! # Why some rows declare no command
//!
//! Twelve of the thirty cases state a precondition and no command: `MockLm`'s `bigram` table is
//! empty, `MockUserFreq` can be handed any count, the case is expressed against a double rather
//! than against a test filter. There is no command to run for one of them, and this table does
//! not invent one -- a filter guessed from the case's subject would run *some* tests and report
//! a pass for a case whose own criterion was never checked. Such a row carries an empty
//! [`commands`](CaseLine::commands) slice, and the runner records it as `flawed` with the
//! document's own precondition as the reason.
//!
//! # The split between a precondition and a command
//!
//! The document writes both on one line: the precondition first, then the command in backticks.
//! This table keeps them apart, which is why a row's precondition ends at the full stop the
//! document writes before the command, and the command keeps the document's own text without
//! its backticks. A case that states two commands -- a benchmark run and the budget gate that
//! judges what it measured -- keeps both, in the document's order.

use super::error::SuiteError;

/// The module code of every case this table holds.
///
/// It is also what the runner's `--module` defaults to, so the module a bare invocation runs is
/// the module the table is written for; a test reads the default back out of the argument parser
/// and compares it with this constant, which is what keeps the two from drifting apart.
pub const CORE_MODULE: &str = "core";

/// One case of the P0 baseline, as `docs/dev/tests.md` states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaseLine {
    /// The case's identifier, e.g. `TC-CORE-01`.
    pub id: &'static str,
    /// The module code the case belongs to, e.g. `core`.
    pub module: &'static str,
    /// The command lines the case document states, in the order it states them.
    ///
    /// Empty when the document states a precondition and no command; see the module
    /// documentation.
    pub commands: &'static [&'static str],
    /// The precondition the case document states, with its trailing command removed.
    pub precondition: &'static str,
    /// The pass criteria the case document states, its bullets joined in the order it writes
    /// them. This is what the case is judged against, and it is recorded in the case's own
    /// evidence so that a bundle read on its own still says what it was evidence for.
    pub criteria: &'static str,
}

/// The P0 baseline cases, in the order the case document states them.
///
/// A `static` rather than a `const` because the selection hands out references into it and a
/// `static` has one address for the whole program: a `const` is materialised afresh at every use,
/// so a caller that kept a reference to it would be keeping a reference to a copy.
pub static CORE_CASES: [CaseLine; 30] = [
    CaseLine {
        id: "TC-CORE-01",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core segment::syllable"],
        precondition: r#"无（纯 Rust 单测，无显示服务器、无词库文件）。"#,
        criteria: r#"**功能逻辑**：411 项全部 `lookup` 命中；表严格升序无重复；无 panic、无 `unwrap` 触发的崩溃。 **边界**：单字母 `x` 不命中（不是合法音节）；空串返回 `None`。"#,
    },
    CaseLine {
        id: "TC-CORE-02",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core segment::syllable"],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：四种 `ü` 写法归一一致；`j/q/x/y` 后的 `u` 正确转为 `ü`；大小写归一。 **边界**：`v` 出现在非 `ü` 位置的输入不 panic（如 `vvv`）。"#,
    },
    CaseLine {
        id: "TC-CORE-03",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core segment::syllable"],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：非法字符被丢弃而非 panic；诊断码为冻结的 `decode/invalid-char`（不得改写，`AGENTS.md` 第 1 节）。 **边界**：全非法输入（如 `!!!`）归一为 `""` 且不 panic。"#,
    },
    CaseLine {
        id: "TC-CORE-04",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core"],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：长度判定发生在**规范化之前**（防止规范化放大长度）；64 字节恰好通过。 **边界**：65 与 64 的边界两侧行为明确，无 off-by-one。"#,
    },
    CaseLine {
        id: "TC-CORE-05",
        module: CORE_MODULE,
        commands: &[
            "cargo nextest run -p ime-core segment::dag::tests::test_build_finds_a_path_for_every_table_syllable segment::syllable::tests::test_lookup_finds_every_table_entry_by_its_index",
        ],
        precondition: r#"`FEAT-TEST-P0.03.01` 的引擎直驱通道就绪。场景文件 `tests/fixtures/scenarios/syllable_traverse.toml`"#,
        criteria: r#"**功能逻辑**：411 个音节全部可纯键盘输入并切分成功。 **人机工学**：不依赖任何修饰键或鼠标操作。"#,
    },
    CaseLine {
        id: "TC-CORE-06",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core segment::dag"],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：多路径不被提前剪枝，全部交给 Viterbi 打分。 **边界**：路径数为 1 的输入（如 `ni`）同样正确。"#,
    },
    CaseLine {
        id: "TC-CORE-07",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core segment::dag"],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：无路径是**可预期的正常分支**，返回类型化错误而非崩溃。 **边界**：单字符非法输入同样走此分支。"#,
    },
    CaseLine {
        id: "TC-CORE-08",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core segment::dag"],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：强制分隔符语义正确；异常分隔符被折叠并记录诊断。 **边界**：开头/结尾/连续 `'` 三种异常均不崩溃。"#,
    },
    CaseLine {
        id: "TC-CORE-09",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core --test alloc_budget"],
        precondition: r#"无；计数由 `alloc-count` 计数分配器（`crates/alloc-count`）在独立测试二进制里安装，经 nextest 每用例一进程运行。"#,
        criteria: r#"**功能逻辑**：稳态解码的堆分配次数不超过分配预算（`BUDGET-ALLOC-01`，`docs/dev/budgets.json` 的 `alloc_count.decode_steady`）；直通降级不比真实解码更贵；首次解码必然更多。 **预算**：`target/alloc-report.txt` 由分配预算门（`budget --alloc`）判定通过。"#,
    },
    CaseLine {
        id: "TC-CORE-10",
        module: CORE_MODULE,
        commands: &["just fuzz 60"],
        precondition: r#"`cargo +nightly fuzz`（`just fuzz 60`）。"#,
        criteria: r#"**功能逻辑**：60 秒 fuzz 无崩溃，`fuzz/artifacts/dag_build/` 无新增产物。 **性能**：单次 `build_dag` 耗时上界成立。"#,
    },
    CaseLine {
        id: "TC-CORE-11",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core input::buffer"],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：一次 Backspace 删除**整个末尾音节**而非一个字母；`BufferEmpty` 的语义是"本次按键后缓冲为空"，**不是**"删除了一个音节"（`.dev-progress.json` 的 cross_task_note 已明确）。 **边界**：第三次返回 `BufferEmpty` 时不得期望 `RemovedSyllable`。"#,
    },
    CaseLine {
        id: "TC-CORE-12",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core input::buffer"],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：caret 为 0 且缓冲非空时不得返回 `BufferEmpty`——否则会话会被误判为结束，preedit 丢失（`.dev-progress.json` 已记录该边界）。 **边界**：删除后缓冲非空。"#,
    },
    CaseLine {
        id: "TC-CORE-13",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core input::buffer"],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：超限时缓冲不变，错误可被上层转为"已达上限"提示。 **边界**：恰好 64 字节时 `push_char` 成功。"#,
    },
    CaseLine {
        id: "TC-CORE-14",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core input::buffer"],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：10000 组随机序列无一违反两条不变量。 **边界**：序列中包含空操作与重复 backspace。"#,
    },
    CaseLine {
        id: "TC-CORE-15",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core test_move_caret"],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：caret 移动不破坏音节边界不变量；越界被拒绝。 **人机工学**：全部操作可由 `KeyAction` 表达，无需鼠标。"#,
    },
    CaseLine {
        id: "TC-CORE-16",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core viterbi"],
        precondition: r#"`FEAT-TEST-P0.03.01` 的内存替身（`MockLexicon`/`MockUserFreq`/`MockLm`）。"#,
        criteria: r#"**功能逻辑**：排序由 Q8.8 定点驱动，**不得**由 `f32` 决定顺序（`AGENTS.md` 3.8）。这是拦截"浮点漂移导致候选抖动"的唯一手段。 **性能**：解码 P99 ≤ 3ms、P999 ≤ 8ms（12 音节输入，`budget` 键 `decode_p99`/`decode_p999`）。"#,
    },
    CaseLine {
        id: "TC-CORE-17",
        module: CORE_MODULE,
        commands: &[
            "cargo nextest run -p ime-core viterbi::tests::test_decode_without_a_usable_word_degrades_to_passthrough",
        ],
        precondition: r#"`MockLexicon` 的所有 `lookup` 返回空迭代器。"#,
        criteria: r#"**功能逻辑**：**绝不返回空候选**——空候选在用户侧表现为"打字无反应"，是最差的降级。 **状态矩阵**：降级状态被显式标记（`degraded`），UI 可据此提示。"#,
    },
    CaseLine {
        id: "TC-CORE-18",
        module: CORE_MODULE,
        commands: &[
            "cargo nextest run -p ime-core viterbi::tests::test_decode_keeps_the_list_inside_the_page_budget state::paging::tests::test_page_count_rounds_up_and_caps_at_five_pages",
        ],
        precondition: r#"`MockLexicon` 对某个 key 返回 200 个词。"#,
        criteria: r#"**功能逻辑**：候选数上限与分页约束成立。 **边界**：恰好 45 与 46 个词的边界行为明确。"#,
    },
    CaseLine {
        id: "TC-CORE-19",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core viterbi::kbest"],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：三种退化均正确；`K > 元素数` 时返回全部元素。 **边界**：含重复值的输入不产生重复输出（或按契约保留）。"#,
    },
    CaseLine {
        id: "TC-CORE-20",
        module: CORE_MODULE,
        commands: &["just bench"],
        precondition: r#"**必须在空闲机器上采集**（`ASM-T-11`；`FEAT-TEST-P0.05.06` 的 `is_clean()` 为真）。"#,
        criteria: r#"**性能**：P99 估计 ≤ 3.0ms。超限即失败（0.4 规则 9）。 **测量纯净度**：报告含 CPU 型号与 governor；不洁净时拒采而非降级为警告。"#,
    },
    CaseLine {
        id: "TC-CORE-21",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core lm::score"],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：打分全程 Q8.8 定点，候选顺序可复现。 **边界**：`prob_num == 0` 时返回下限 `-2048` 而非 panic。"#,
    },
    CaseLine {
        id: "TC-CORE-22",
        module: CORE_MODULE,
        commands: &[
            "cargo nextest run -p ime-core lm::score::tests::test_edge_score_clamps_the_user_term_at_a_million_hits lm::score::tests::test_user_term_starts_at_zero_and_scales_with_the_count",
        ],
        precondition: r#"`MockUserFreq` 可注入任意频次。"#,
        criteria: r#"**功能逻辑**：防止某个词被打 10 万次后永久霸榜。 **边界**：`freq = 0` 时用户项为 0。"#,
    },
    CaseLine {
        id: "TC-CORE-23",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core lm::score::tests::test_scorer_new"],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：非法配置在构造期被拒绝，不留到运行期。 **边界**：全零权重合法（退化为等分）。"#,
    },
    CaseLine {
        id: "TC-CORE-24",
        module: CORE_MODULE,
        commands: &[
            "cargo nextest run -p ime-core lm::ngram::tests::test_bigram_falls_back_to_the_unigram_plus_the_fixed_penalty",
        ],
        precondition: r#"`MockLm` 的 `bigram` 表为空。"#,
        criteria: r#"**功能逻辑**：退化惩罚是固定常数，保证确定性。 **边界**：`prev` 为空串时同样退化。"#,
    },
    CaseLine {
        id: "TC-CORE-25",
        module: CORE_MODULE,
        commands: &[
            "cargo nextest run -p xtask tune::tests::test_rank_and_evaluate_count_the_first_choice_and_the_reachable_rows tune::tests::test_generate_holdout_writes_rows_that_avoid_the_evaluation_set",
        ],
        precondition: r#"`xtask tune` 的求值器（`xtask/src/tune/eval.rs`）。"#,
        criteria: r#"**功能逻辑**：两个指标分别记录（前者衡量排序、后者衡量可达性——`L3b` 多键展开修的是后者）。 **性能**：全量解码耗时 < 3s（可参与 CI）。 **边界**：`lm_holdout.tsv` 与 `lm_golden.tsv` 无重叠。"#,
    },
    CaseLine {
        id: "TC-CORE-26",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core passthrough"],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：打大写英文不触发输入法。 **边界**：`auto_english_on_uppercase = false` 时走 `Decode`。"#,
    },
    CaseLine {
        id: "TC-CORE-27",
        module: CORE_MODULE,
        commands: &[
            "cargo nextest run -p ime-core passthrough::tests::test_classify_rule_table_returns_the_expected_decision",
        ],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：URL/邮箱场景不打断用户。 **边界**：纯字母输入**不**被误判为 URL。"#,
    },
    CaseLine {
        id: "TC-CORE-28",
        module: CORE_MODULE,
        commands: &[
            "cargo nextest run -p ime-core passthrough::tests::test_classify_rule_table_returns_the_expected_decision passthrough::tests::test_classify_punctuation_table_maps_the_twelve_ascii_marks",
        ],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：12 个标点映射正确；`'` 保留为分隔符语义。 **边界**：`punct_mode = english` 时返回 `HostHandles`（**不是** `Decode`——`Decode` 会把标点当非法字符丢弃，导致按逗号"什么都没发生"；`.dev-progress.json` 已记录该修正）。"#,
    },
    CaseLine {
        id: "TC-CORE-29",
        module: CORE_MODULE,
        commands: &["cargo nextest run -p ime-core passthrough::tests::test_to_full_width"],
        precondition: r#"无。"#,
        criteria: r#"**功能逻辑**：94 个字符逐一断言正确。 **边界**：非 ASCII 字符原样返回。"#,
    },
    CaseLine {
        id: "TC-CORE-30",
        module: CORE_MODULE,
        commands: &[
            "cargo nextest run -p ime-core temp_english passthrough::tests::test_classify_rule_table_returns_the_expected_decision",
        ],
        precondition: r#"`PassthroughFlags.temp_english = true`。"#,
        criteria: r#"**功能逻辑**：临时英文模式的判定顺序**早于**大写提交规则与 URL 规则（`.dev-progress.json` 记录了该顺序修正：按任务卡的原始顺序，临时英文模式下打一个大写字母会被大写规则提交并退出会话——正是该模式要防止的）。 **边界**：`EnterTempEnglish` 由路由层产出，`classify` 不返回它（纯文本分类器观察不到键和弦）。"#,
    },
];

/// The cases of `module` that `only` names, in the table's own order.
///
/// An empty `only` selects the module's whole table. A named identifier that the module does not
/// hold refuses the whole selection rather than being dropped, and a module the table does not
/// cover at all refuses too: a run of zero cases has no verdict to give, and reporting one green
/// would be a claim about work nobody did.
///
/// The order is the table's and not the caller's, so that two invocations that name the same
/// cases in a different order publish the same run -- the index of a run is read as a document,
/// and a document whose rows move with the order of a command line is harder to compare with the
/// one before it.
///
/// # Errors
///
/// Returns [`SuiteError::NoCases`] when the module has no case in the table, and
/// [`SuiteError::UnknownCase`] when `only` names one the module does not hold.
///
/// # Panics
///
/// Never.
pub fn selected(module: &str, only: &[String]) -> Result<Vec<&'static CaseLine>, SuiteError> {
    let held: Vec<&'static CaseLine> = CORE_CASES
        .iter()
        .filter(|case| case.module == module)
        .collect();
    if held.is_empty() {
        return Err(SuiteError::NoCases {
            module: module.to_owned(),
        });
    }
    if only.is_empty() {
        return Ok(held);
    }
    for id in only {
        if !held.iter().any(|case| case.id == id.as_str()) {
            return Err(SuiteError::UnknownCase {
                tc: id.clone(),
                module: module.to_owned(),
            });
        }
    }
    Ok(held
        .into_iter()
        .filter(|case| only.iter().any(|id| id.as_str() == case.id))
        .collect())
}
