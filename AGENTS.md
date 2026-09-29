# AGENTS.md — rspinyin Development Standards

> 文档版本: v1.0 ｜ 基线来源: rust.AGENTS.en.md ｜ 适配技术栈: Rust 2024 workspace（7 crates + xtask，edition 2024 / MSRV 1.85 / toolchain 1.98.0）｜ 最后同步 Commit: `<初始提交后回填>` ｜ 维护约定: 工具链或协作模式变更后必须同步更新门禁与禁止事项

Scope: all agents and developers in this repository. All rules are mandatory by default; exceptions require explicit user approval.

This document is adapted from `rust.AGENTS.en.md` for rspinyin — an offline-first Linux pinyin IME delivered as an in-process Fcitx5 addon with a fully self-drawn candidate window. Where a baseline rule was replaced rather than carried over, the reason is recorded in the project's adaptation record. The authoritative product/architecture spec remains `docs/dev/features.md`; when this file and `features.md` disagree on a **technical** matter, `features.md` wins and this file must be corrected.

**Role definitions**
- **Main agent**: breaks down and dispatches tasks (possibly in multiple waves), owns the frozen contract crate `crates/ime-types`, merges each wave's output, runs the Section 2 gates once after all waves complete and then fixes issues in one concentrated pass, performs git commits, maintains `docs/dev/features.md`.
- **Sub-agent**: writes code only, within whitelisted files (implementation + tests + doc comments); runs no cargo commands, does not modify the task document, performs no git operations.

**General principles**: safety first; readability first; do not reinvent wheels (see 3.5); minimal changes — no unrelated refactors or formatting.

---

## 1. Language Conventions

| Artifact | Language requirement |
|---|---|
| User communication, project docs, task docs (`docs/`) | Chinese |
| Code comments, doc comments (`///`, `//!`) | **English** |
| Identifiers (types/functions/variables/modules) | English, follow Rust naming conventions, no pinyin |
| Commit messages | English (Conventional Commits) |
| Error codes, log messages, developer-facing diagnostics | English |
| User-facing UI copy (candidate window, status strip) | Chinese |

- Comments explain "why", not "what"; complex algorithms (segmentation DAG, Viterbi k-best, spring integrator, buffer damage) must carry explanatory English comments.
- **Comments must not contain task tags** (`TASK-1.02.01`, `1.02.01`, ticket numbers); task tags may only appear in `docs/dev/features.md` and commit messages.
- TODOs use `// TODO: specifics`; ownerless or vague TODOs are forbidden; dead code (including commented-out code) is deleted outright.
- Cross-boundary error codes follow the stable `domain/action/reason` string form defined in `features.md` 2.2.4 (e.g. `dict/corrupt`, `decode/too-long`, `ui/stale-select`). Never reword an existing code — they are matched by diagnostics and tests.

## 2. Quality Gates (main agent only)

Sub-agents must not run any command in this section. Only after **all waves of sub-agents have been dispatched and completed** and all code is merged back to the mainline does the main agent run the full gate suite **once** at the end (no intermediate per-wave gates). Pass criteria: **zero warnings, all tests green**.

These commands are fixed by `features.md` 0.3 and are the project's single source of truth for "done":

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo nextest run --workspace --all-features      # if missing: cargo install cargo-nextest --locked
cargo test --workspace --doc                      # nextest does not cover doctests; mandatory
```

- **Test execution baseline**: run tests through `cargo nextest run`. If it is not installed, run `cargo install cargo-nextest --locked` first; fall back to `cargo test --workspace --all-features` only if the install fails. **nextest does not execute doctests**, so `cargo test --workspace --doc` must be run as a supplement on either path.
- **Host-ABI gate** (requires `libfcitx5core-dev`; absent by default so the pure-Rust job stays dependency-free):
  ```bash
  cargo check -p ime-fcitx5 --features fcitx5-host
  ```
- **Architecture & license audits** — required before any release-quality claim. Established by `TASK-1.01.02`; until those scripts exist, run the equivalent check by hand and say so in the report:
  ```bash
  just ci     # = check + check-deps + check-unsafe + check-net + check-slint + check-dict
  ```
  `check-deps` (0.4 rule 1) ｜ `check-unsafe` (0.4 rule 3) ｜ `check-net` (0.4 rule 6) ｜ `check-slint` (0.4 rule 11 / `OB-4`) ｜ `check-dict` (ADR-0000 dictionary-source allowlist).
- **Budget assertions**: thresholds live in `features.md` 0.5.3 and `docs/dev/budgets.json`. A regression past a budget is a gate failure, not a warning.
- Any `#[allow(...)]` must carry a comment explaining why the warning cannot be eliminated.
- All issues found by the gates (compile errors, clippy warnings, test failures) must be **fully fixed by the main agent**; fix small issues directly instead of sending work back to sub-agents; re-dispatch only for large-scale problems.

## 3. Coding Standards

### 3.1 Project Configuration
- Toolchain is pinned by `rust-toolchain.toml` (`channel = "1.98.0"`); `rust-version = "1.85"` is the workspace MSRV; `edition = "2024"` and `resolver = "3"` are workspace-wide and must not be overridden per-crate.
- `Cargo.lock` is **committed** — the workspace produces a `cdylib` shipped to users, so the lockfile is part of the deliverable.
- Versions converge in `[workspace.dependencies]`; new crates must declare their dependency there, not inline. Every dependency line carries a trailing comment stating its purpose.
- Declare features centrally in `[features]`, one semantic each; no implicit enabling between features; gates verify with `--all-features`. `ime-fcitx5`'s `fcitx5-host` feature is the one deliberate exception to `--all-features` local testing: it must be exercised separately via the host-ABI gate.
- `.gitignore` covers `/target`, `*.dict.tmp`, `user.redb*`, `*.corrupt.*`, `/logs`; `target/` never enters the repository.
- `git submodule` is forbidden anywhere in the workspace — `cargo vendor` and offline builds must remain possible.

### 3.2 Error Handling
- Recoverable errors return `Result` and propagate via `?`; never swallow errors.
- Cross-boundary errors are **uniformly** `ImeError` / `DictError` from `crates/ime-types` (derived with `thiserror`). Hand-written `impl Display` / `impl std::error::Error` is forbidden. Crate-local errors are allowed only if they are `#[from]`-convertible into `ImeError`; `anyhow` is confined to `xtask` and tests.
- **Non-test code must not use `unwrap()` / `expect()` / `panic!`** — this is `features.md` 0.4 rule 7, enforced by the workspace clippy lints (`unwrap_used`, `expect_used`, `panic`) plus `-D warnings`. `unwrap`/`expect` are allowed only in tests and in provably infallible cases backed by static invariants, always with an explanatory comment.
- No `extern "C"` function may unwind. Every FFI entry point is wrapped by the panic guard from `TASK-1.08.02`; on panic it returns `false` and writes a crash log. Cross-FFI unwind is undefined behaviour.
- Never leak internal error details to users: user-visible text carries the stable error code, never a filesystem path.

### 3.3 Threading & Concurrency
**This project does not use an async runtime.** The baseline's `tokio`-first rule is replaced wholesale: `tokio` and every other async executor are excluded by `features.md` 0.4 rule 6, and the architecture is built on OS threads and file descriptors instead. The concurrency contract is `features.md` 2.1 and `ASM-10`–`ASM-12`.

- **Two threads, physically isolated.** The host thread is fcitx5's main loop and does only `key → decode → build UiFrame → post`. The UI thread owns the Wayland/X11 connection, the Slint platform object, and the raster buffers. Never call rendering work from the host thread, and never touch `InputContext` from the UI thread.
- **Cross-thread communication is SPSC queues plus `eventfd`.** Wake the UI thread by writing to its `eventfd`; the UI thread's `poll(2)` loop multiplexes the Wayland fd, the `eventfd`, and the timerfd. **No polling timers** — a spinning or periodically-waking loop breaks `BUDGET-CPU-01`.
- Queue capacities and overflow behaviour are contractual (2.2.1/2.2.2). `Frame`/`Theme` are latest-wins single slots; `Show`/`Hide` are ordered and non-droppable; `UiEvent::Select` is never dropped. Do not "simplify" these into an unbounded channel.
- **Never block the host thread.** No filesystem IO, no `mmap`-cold-page faults, no lock contention, no `sleep` on the fcitx5 main loop. Decoding is pure and bounded (`BUDGET-LAT-02`); user-frequency writes are batched and deferred per `ASM-04`/`ASM-20`.
- Decoding is **single-threaded and serial per session** (`ASM-11`). Do not introduce concurrency into session state; if a concurrent source appears, add a `revision` and let the UI drop stale frames.
- Prefer message passing over shared mutable state. Where shared state is unavoidable: atomics > `Mutex`/`RwLock` > `unsafe`.
- `unsafe` is permitted **only** in `crates/ime-fcitx5/src/ffi/**`, `crates/ime-ui-addon/src/ffi/**` and `crates/ime-dict/src/mmap.rs` (0.4 rule 3, enforced by `scripts/check-unsafe.sh`). There are two FFI directories because there are two cdylibs: ADR-0003 splits the plugin into an input-method addon and a user-interface addon, each with its own C ABI and its own glue, neither linking the other. Every `unsafe` block carries a `// SAFETY:` comment justifying the invariants it relies on.
- The decoder must stay a pure function (0.4 rule 4): no filesystem, clock, environment, or global mutable state. User frequency arrives through `trait UserFreqSource`. This is what makes deterministic testing possible — do not break it for convenience.

### 3.4 Logging & Debug Output
- Uniformly `tracing` + `tracing-subscriber`, initialised exactly once through `ime-diag`'s `init_logging`. Business crates use `tracing` macros but never install a subscriber.
- Level semantics: `error` needs human intervention / `warn` auto-recoverable anomalies / `info` key business milestones / `debug`, `trace` development diagnostics.
- Prefer structured fields (`field = %value`); never concatenate dynamic content into the message string.
- **Never log user input content.** `raw`, `text`, `preedit`, candidate text, and commit text are on `ime-diag`'s redaction denylist and must not be passed in the first place — the `RedactLayer` is a defensive second line, not a licence to log them. Candidate counts and source distributions are the diagnostic substitute.
- Application identifiers are logged as a hash, never in clear text (`TASK-1.06.02`). `$HOME` prefixes are rewritten to `~`.
- Log files and their rolled siblings are `0600`; the containing directory is `0700`.
- Never commit `println!`/`eprintln!`/`dbg!`; normal CLI output in `xtask` (stdout results, stderr usage errors) is exempt.

### 3.5 Dependencies: prefer third-party crates, no reinventing wheels
- Before implementing any generic capability (serialization, parsing, CLI, hashing, FST, mmap, embedded KV), first adopt the ecosystem's de-facto standard crate; implement it yourself only when nothing fits, recording the reason in comments and the commit message.
- Selection reference for this project: `thiserror`, `serde` (+`serde_json`, `toml`), `clap`, `tracing` (+`tracing-subscriber`, `tracing-appender`), `bitflags`, `fst`, `memmap2`, `redb`, `slint`, `x11rb`, `wayland-client`, `criterion`, `cargo-nextest`.
- **Network crates are prohibited outright.** No `reqwest`/`hyper`/`ureq`/`curl`/`isahc`/`surf`/`awc`/`tungstenite`/`quinn`/`zmq`, no `rustls`/`native-tls`, and no network features of `tokio`/`async-std`, anywhere in the dependency closure. This is a product promise (`BUDGET-NET-01`), not a preference — `scripts/check-no-network.sh` asserts it over the `cargo metadata` resolve graph.
- New dependency baseline: ecosystem-mainstream, no unfixed vulnerabilities (check rustsec), license-compatible with the frozen licensing decisions. Slint's Royalty-free 2.0 obligations (`OB-1`…`OB-6` in `docs/dev/adr/0000-upstream-decisions.md`) are binding — see Section 8.
- No multiple overlapping crates for the same capability; versions converge in `[workspace.dependencies]`; never hand-edit `Cargo.lock` to bypass conflicts.

### 3.6 Testing
- Every pub function: at least 1 positive test + 1 boundary/error-path test. A test without a real assertion (`assert!`, `assert_eq!`, `assert_matches!`) does not count as delivered.
- Unit tests live in `#[cfg(test)]` next to the code under test; move to `tests/` once test code exceeds 200 lines.
- **Determinism is mandatory** in `ime-core` and `ime-dict`: tests drive the decoder through in-memory mock `Lexicon` / `UserFreqSource` / `LanguageModel` implementations. No test may depend on a real dictionary file, the clock, the environment, or the display server.
- Before fixing a bug, first write a failing test that reproduces it.
- Platform and rendering code is tested against a `MockBackend` implementing `trait SurfaceBackend`; tests must pass with no display server present.
- Naming: `test_<function>_<scenario>_<expectation>`, e.g. `test_segment_empty_input_returns_empty_dag`.
- Run tests through `cargo nextest run` (see Section 2), never bare `cargo test`; doctests are the sole exception and are verified separately with `cargo test --doc`.
- Any change touching a budgeted path (`BUDGET-LAT-*`, `BUDGET-MEM-*`, `BUDGET-CPU-*`) needs a `criterion` benchmark or probe assertion, not just a functional test.

### 3.7 Doc Comments
- Every pub item must have `///`: purpose, parameters, return value, `# Errors`, `# Panics` — written in English per Section 1.
- Modules use `//!` describing responsibility and boundaries; `ime-types` additionally states that it is the frozen contract and how to change it.
- Include runnable examples (`# Examples`) where they clarify the contract, verified by `cargo test --doc` (nextest does not cover doctests; this step cannot be skipped).
- `trait Lexicon` / `UserFreqSource` / `LanguageModel` / `SurfaceBackend` implementations must document their concurrency guarantees (`Send`/`Sync`, reentrancy, blocking behaviour).

### 3.8 Performance
- **The budgets in `features.md` 0.5.3 are contracts, not aspirations.** 0.4 rule 9 makes every threshold a CI-assertable claim. Do not add work to a budgeted path without measuring it.
- No premature optimization; minimize allocations on hot paths, avoid repeated allocation inside loops. The decode path is allocation-sensitive — reuse buffers across keystrokes rather than reallocating per frame.
- Avoid unnecessary `clone()` in APIs; borrow instead of copying whenever possible. Dictionary access is **zero-copy over `mmap`** — returning owned `String`s from `Lexicon` defeats the design.
- Prefer `i32` Q8.8 fixed-point for scoring across the decode boundary; `f32` scores may not drive ordering decisions (candidate order must be reproducible).
- Performance-sensitive changes require `criterion` / `cargo bench` data.

### 3.9 Security & Privacy Practices
- **Dictionary and user-data files are untrusted input.** Validate magic, format version, length fields, and CRC *before* mapping or trusting any offset; any out-of-range offset returns `DictError`, never an out-of-bounds access (0.4 rule 8).
- Use `TryFrom`/`try_into` conversions and `checked_`/`saturating_` arithmetic on parsed lengths and offsets to prevent overflow panics.
- User data lives in XDG directories with directory mode `0700` and file mode `0600`; the plugin degrades to **read-only mode** (`data/readonly-mode`) rather than failing when a directory is unwritable (`ASM-15`).
- Passwords and other sensitive input contexts suppress learning entirely (`TASK-1.06.02`). Never persist, log, or transmit input from a sensitive context.
- Credentials, keys, tokens: never hardcode or commit them; inject via environment variables or config files; sample files contain placeholders only.
- Run `cargo audit` (vulnerabilities) and `cargo deny` (advisories/licenses/bans) before committing; findings must be fixed or exempted in writing.

## 4. Code Style

### 4.1 Formatting & Lint
- rustfmt is the final formatting standard (`rustfmt.toml` may override individual options); import grouping uses `StdExternalCrate` (std/third-party/local), consistent project-wide.
- clippy is the final style arbiter; `#[allow(clippy::...)]` without written justification is forbidden. Note that `clippy::unwrap_used`, `clippy::expect_used`, and `clippy::panic` are deliberately enabled workspace-wide and are not to be silenced.

### 4.2 Naming

| Item | Convention | Example |
|---|---|---|
| Types/traits | UpperCamelCase | `UiFrame`, `SurfaceBackend` |
| Functions/variables | lower_snake_case | `build_segmentation_dag` |
| Constants | SCREAMING_SNAKE_CASE | `RSPINYIN_ABI_VERSION` |
| Modules/crates | lower_snake_case | `ime-types`, `ime_dict::format` |
| Lifetimes | short and semantic | `'a`, `'dict` |
| Booleans | is_/has_/can_ prefixes | `is_composing` |
| Getters | no get_ prefix | `fn geometry(&self)` |

- Names express intent (`syllable_count` over `n`); never shadow standard-library type names (e.g. custom `Result`, `Box`).
- Contract type names in `crates/ime-types` are frozen — renaming one is a contract change (Section 8), not a refactor.

### 4.3 Type & Function Design
- Use `enum` for mutually exclusive states instead of multiple `bool`s; use newtypes to distinguish same-shape primitives (`SessionId(u64)` / `WordId(u32)` / `Revision(u32)`).
- Public APIs do not expose internal implementation types; `pub` fields need sufficient reason; prefer `derive` for standard traits; `RefCell` is forbidden in public APIs.
- Prefer immutable data; pass mutability explicitly via `&mut`.
- Function bodies ≤50 lines; aggregate >4 parameters into a struct; single responsibility; nesting ≤3 levels, use early returns.

### 4.4 Module Organization
- One responsibility per file, file name matching the core type or function; `mod.rs` style consistent project-wide.
- `use` paths ≤3 levels deep; use `pub use` to converge public interfaces when needed; **one-way dependencies only** — the layer order `ime-types ← ime-core ← ime-dict ← ime-config ← ime-ui ← ime-fcitx5` is enforced by `scripts/check-deps.sh` and any reverse edge is a build failure.
- Module declaration (`pub mod`) in a crate's `lib.rs` is the main agent's job; sub-agents write leaf files only.

## 5. File Line Limit ≤ 800

- Any `.rs` file ≤800 lines (`wc -l` counting, including blank lines and comments); `tests/` relaxed to 1200; generated code is exempt but must be marked `// @generated` with the generation command (`.slint`-generated Rust included).
- Over the limit it must be split: split submodules by responsibility (e.g. `platform/wayland/` into `layer_shell.rs`, `popup.rs`, `canvas_popup.rs`), move tests to `tests/`, sink shared logic down, distribute multiple `impl` blocks of one type across files.
- Never delete comments, squeeze blank lines, or cram logic into long lines to pass; over-limit files count as unfinished tasks.
- Self-check: `find . -name "*.rs" -not -path "./target/*" | xargs wc -l | sort -rn | head -20`

## 6. Multi-Sub-agent Parallel Development

### 6.1 Scheduling
- **Dispatch first, fix later**: upon receiving many tasks, break them down and dispatch immediately to get sub-agents running; the main agent does integration, fixing, and verification afterwards.
- Tasks may be dispatched in multiple waves: as each wave's sub-agents finish, collect their output, merge the code, and dispatch the next wave; keep 2–5 sub-agents running in parallel, no idling, no interruption.
- Split tasks as vertical slices (each independently verifiable), not by technical layer. Note the project's task cards are already sized as vertical slices — prefer dispatching a whole `TASK-*` card rather than sub-dividing it.
- Failure isolation: one sub-agent's failure does not affect other tasks; the main agent decides to retry (possibly smaller) or take the work back.

### 6.2 Process
1. **Plan** (main agent): produce a written task list; the cross-module API contract is **`crates/ime-types` itself** — it is frozen before W1 and sub-agents only implement against it. Contract changes require an ADR and a main-agent decision (5.1.1). Sub-agents never invent cross-boundary types in their own crate.
2. **Dispatch** (main agent): task descriptions are self-contained (goal, files involved, contract, constraints, acceptance criteria), declare the file whitelist, and state "write code only, do not run cargo commands".
3. **Execute** (sub-agent, code only): complete implementation + test code + doc comments within the whitelist; static self-check (naming, thiserror, threading discipline, line limits, comment language); report afterwards: what was done, self-assessed risks, open issues.
4. **Aggregate & final gate** (main agent): as each wave completes, merge its output back to the mainline in dependency order, but run **no intermediate gates** beyond a quick `cargo check --workspace --all-targets` to keep the tree compilable; after **all waves have been dispatched and completed**, run the full Section 2 gate suite once and fix all found issues in one concentrated pass; on conflicts, keep the side that better matches the contract and quality requirements; do not declare completion until the gates pass and all fixes are done.

### 6.3 Parallelization Criteria
- Parallelizable: no file overlap, interfaces agreeable in advance, independent bug fixes/features/research. The project's wave plan (`features.md` 0.6) and Track A/B/C split are pre-computed for this.
- Must be serial: same-file modifications, dependence on prior output (design interfaces first, then parallelize), global decisions (architecture choices, dependency additions, directory structure, contract changes).

### 6.4 Discipline
- Interface contracts, dependency lists, and directory structure: the main agent's documents are the single source of truth. `Cargo.toml`, `Cargo.lock`, and module root files (`lib.rs`, `mod.rs`) are never touched by sub-agents.
- Sub-agents never run `git commit`/`git push`; commits are made by the main agent after verification passes.
- Sub-agents report cross-scope problems to the main agent instead of handling them on their own.

## 7. Git & Task Document

- Conventional Commits, **written in English**; types: feat / fix / refactor / docs / test / chore / perf / style / ci; one commit does one thing; task tags go into commits only, never into code.
- Without explicit user request, `git push`, `git rebase`, and `git reset --hard` are forbidden; `git status` must be clean before committing.
- **The task document is `docs/dev/features.md`.** After finishing each task, the main agent immediately flips that task card's status field from `` `[ ] 待开始` `` to `` `[x] 已完成` `` and appends an **验收记录** (verification command, platform/compositor/dependency versions, known limitations) beneath the acceptance criteria — before starting the next task. Batch backfilling is forbidden; sub-agents do not edit the task document.
- A task may only be marked `[x]` when its 验收记录 is non-empty and the four Section 2 gates are green. An empty or "（未验证）" 验收记录 means the task is still `[ ]`, regardless of what the code looks like.
- Consistency is mandatory across three places: the task card status, the WBS traceability row in 5.1, and the task-overview row in 0.7. When document and code disagree, correct the document to match the code.
- Delivery reports attach a status summary (done / in progress / todo).

## 8. Prohibited

1. Committing `todo!()` / `unimplemented!()` stub code to the mainline (unless the user explicitly requests a skeleton first).
2. `unsafe` without a `// SAFETY:` comment, or `unsafe` anywhere outside `crates/ime-fcitx5/src/ffi/**`, `crates/ime-ui-addon/src/ffi/**` and `crates/ime-dict/src/mmap.rs`.
3. `unwrap()` / `expect()` / `panic!` in non-test code for recoverable errors (0.4 rule 7).
4. Deleting or bypassing tests to make the build pass.
5. Unreviewed new dependencies, or major-version upgrades without notifying the user.
6. Hand-editing `Cargo.lock` to bypass dependency conflicts.
7. Deleting comments, squeezing blank lines, or cramming long lines to pass the line limit.
8. Sub-agents modifying files outside the whitelist or changing the interface contract.
9. Sub-agents running heavy commands such as `cargo build`/`test`/`clippy`/`fmt`.
10. Blocking work on the host thread — filesystem IO, cold `mmap` faults, lock contention, `sleep`, or any unbounded computation inside an fcitx5 callback.
11. Reinventing wheels when mature third-party crates are available.
12. Task tags (`TASK-1.02.01`, `1.02.01`, ...) or code-irrelevant process information in comments.
13. Failing to update the task document promptly after finishing a task, or batch backfilling.
14. Hardcoding credentials/keys/tokens or committing them to the repository.
15. Committing `println!` / `eprintln!` / `dbg!` debug output (normal CLI output is exempt).
16. Ignoring `cargo audit` / `cargo deny` findings without fixing or a written exemption.
17. **Any network capability.** No network crate in the dependency closure, no outbound socket, no update channel, no telemetry (0.4 rule 6, `BUDGET-NET-01`).
18. **Exporting a Slint type from `ime-ui`'s public API** — `slint::*` must not appear in any `pub` signature or field (0.4 rule 11, `OB-4`). The candidate window must never be a distributable Slint-programmable surface.
19. **Reverse or cross-layer dependencies** between crates (0.4 rule 1); `ime-ui` must never import `ime-core`'s decoder or `ime-dict`'s FST handles (0.4 rule 2).
20. **Taking keyboard focus.** The candidate window must never call `XSetInputFocus`; on Wayland `keyboard_interactivity` must be `none` (0.4 rule 5). Focus loss is the project's highest-severity defect.
21. Logging user input content, candidate text, or commit text; or logging an application identifier in clear text.
22. Changing a frozen `crates/ime-types` type without an ADR in `docs/dev/adr/` and a main-agent decision.
23. Resetting an in-progress composing session on config reload (0.4 rule 10).

## 9. Delivery Checklist (main agent executes; report with evidence)

- [ ] All Section 2 gates pass (zero warnings, tests green), including `cargo test --workspace --doc`
- [ ] `just ci` (or the equivalent manual audits) passes: deps / unsafe / network / Slint-leak / dictionary-source
- [ ] No file over 800 lines (tests/ 1200)
- [ ] pub items fully doc-commented in English, with `# Errors` / `# Panics`; no task tags in comments
- [ ] New code has corresponding tests; new dependencies passed the baseline review and added no network capability
- [ ] No hardcoded credentials; no `println!` / `dbg!` debug output left; no input content in logs
- [ ] Sub-agent tasks merged back to the mainline, verified and fixed, no out-of-whitelist changes, no heavy commands run
- [ ] Task document synchronized: card status, 5.1 WBS row, and 0.7 overview row all agree
- [ ] Report includes: what was done, how it was verified, open issues and suggestions

Auxiliary grep checks (output requires human review):

```bash
# Non-English comments in code (expected: no output; comments are English per Section 1)
grep -rnP '//.*[\x{4e00}-\x{9fff}]' --include="*.rs" crates/ xtask/
# Task tags in comments
grep -rnE '//.*(TASK-[0-9]|[0-9]+\.[0-9]+\.[0-9]+)' --include="*.rs" crates/ xtask/
# Leftover debug output (build.rs is excluded: `println!("cargo::...")` is Cargo's
# only build-script directive channel, not debug output)
grep -rn -e 'dbg!' -e 'println!' -e 'eprintln!' --include="*.rs" crates/ xtask/ | grep -v 'build\.rs:'
# unwrap/expect outside tests (spot check; clippy is the real gate)
grep -rnE '\.(unwrap|expect)\(' --include="*.rs" crates/ | grep -v '#\[cfg(test)\]'
# Network crates in the resolve graph
grep -rnE '^(reqwest|hyper|ureq|curl|rustls|native-tls) ' Cargo.lock
```

---

*Modifying this file requires user confirmation.*
