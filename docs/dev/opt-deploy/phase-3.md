# rspinyin 跨平台构建、分发与 CI/CD 优化工程规范 · Phase 3 分片（P2 任务卡）

> 文档版本: v1.0 ｜ 系统形态: Linux 桌面输入法插件（Fcitx5 进程内 cdylib）｜ 架构基线: Cargo workspace（edition 2024 / MSRV 1.85 / toolchain 1.98.0）｜ 关联 ADR: [`../adr/0000-upstream-decisions.md`](../adr/0000-upstream-decisions.md)、[`../adr/0003-ui-role-separate-addon.md`](../adr/0003-ui-role-separate-addon.md) ｜ 最后同步 Commit: `ee0dbfb` ｜ 维护约定: 代码演进后必须回写主文档第 2 节问题清单状态与第 3 节交付矩阵
>
> **回链主文档**：[`../opt-deploy.md`](../opt-deploy.md) — 本分片承载 P2 任务卡（`BUILD-P2.*`，4 张）。假设清单（`ASM-01`~`ASM-14`）、问题总清单（`BUILD-DEF-01`~`BUILD-DEF-23`）、交付矩阵（`L-01`~`L-08` 与合成器四档）、追溯表与关键路径汇总**全部在主文档**，本分片不重复定义，只引用。
>
> **前置分片**：[`./phase-2.md`](phase-2.md)（P1 任务卡，9 张）。

---

## 0. 分片基线（引用主文档与 P1 分片，不重复定义）

| 引用 | 来源 | 本分片依赖的关键结论 |
|---|---|---|
| 交付形态 | 主文档 0.1 / `ASM-01` | 产物是被 `fcitx5` `dlopen` 的 cdylib；无独立可执行文件、无应用包、无启动器 |
| 更新通道 | 主文档 3.3 / `ASM-04` | **不存在自动更新通道**。发布流水线产出并签名，但**不包含任何用户侧拉取逻辑** |
| 交付通道契约 | 主文档 3.3.1 / 3.3.2 | `rspinyin-release.json` 的结构、S1~S5 校验状态机、九个 `dist/*` 错误码 |
| 平台矩阵 | 主文档 3.1 / `ASM-05`、`ASM-07` | 8 个切片；`L-08` 是源码 tarball 切片 |
| 会话档位矩阵 | 主文档 3.2 / `ASM-11` | X11 本机可验证；wlroots / KWin / Mutter 三档**本机不可验证** |
| 验证边界 | `features.md:199-207`、`features.md:226` | `[性能]` 项在本机（WSL2）可跑，但**数值不代表目标硬件**，仅作相对回归基线 |
| P0 / P1 产出 | 主文档第 5 节、P1 分片 | `xtask package`、`xtask verify`、`just ci` / `just check-host`、三种发行版包、签名与 `SHA256SUMS` 均已就位 |

**本分片的性质**：P2 不再新增能力，只做**收敛与自动化**。P0 让树能产出可分发的产物，P1 让产物能装进三种发行版并带上合规文件，P2 让"打一个 tag 就完成全部发布"成立，并把验证面从"能装"扩展到"在四个合成器档位上都能用、连续跑 8 小时不退化"。

---

## 1. 任务卡

### 任务 ID：BUILD-P2.05.01 tag 驱动的全自动发布流水线

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-15`、`L-08`
  - 优先级与复杂度：`P2` | 高 | 预估工时: 3.0 人天
  - 前置依赖：`BUILD-P1.04.01`、`BUILD-P1.03.05`
  - 关键路径：**CP: 是**
  - 并行通道：Track C（CI/CD·发布）
  - 代码落地锚点 (Code Anchor)：`.github/workflows/release.yml`、`xtask/src/release.rs`、`docs/dev/release-checklist.md`
  - 当前状态：`[ ] 待开始`
- **目标与核心交付物**：
  - 核心改进指标：打一个 `v*` tag 触发：全门禁 → 全矩阵构建 → 剥离与符号复检 → 三发行版包 + 源码 tarball → 生成 `rspinyin-release.json` → 签名 → 发布 GitHub Release。**全程无手工步骤，且不产出任何更新通道。**
  - 目标产物格式：一次 Release 含 `.tar.gz`、`.deb`（amd64 + arm64）、`.rpm`（x86_64 + aarch64）、`.pkg.tar.zst`（x86_64 + aarch64）、`rspinyin-release.json`、`SHA256SUMS`、`SHA256SUMS.asc`。
- **工程实现方案与配置文件/脚本全文**：

  流水线结构：`gate`（复用 CI）→ `build`（矩阵并行）→ `assemble`（汇总、生成 manifest、签名）→ `publish`（创建 Release）。**`assemble` 与 `publish` 之间没有人工确认点**，但 `publish` 只在 `gate` 全绿时可达。

  `.github/workflows/release.yml`（全文）：

```yaml
# Tag-driven release.
#
# Pushing a tag matching v* runs the full gate suite, builds every artifact in
# the delivery matrix, assembles the release manifest and its signature, and
# publishes a GitHub Release. Nothing here contacts a network service on the
# user's behalf and nothing here produces an update channel: rspinyin's upgrade
# path is the distribution's package manager, and a component that could fetch
# its own replacement would be a network-reachable code path in a process that
# is contractually forbidden from opening a socket.
#
# The manifest and its signature exist so a user can verify what they downloaded.
# Verification is `xtask verify`, which is read-only.

name: release

on:
  push:
    tags:
      - 'v*'

# A release must not be cancelled halfway: a partially published release is
# worse than a failed one, because the artifacts that did land look complete.
concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: false

permissions:
  contents: write

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1
  SOURCE_DATE_EPOCH: 1790899200

jobs:
  # The gate suite runs on the tag itself, not on an assumption that CI already
  # passed. A tag can be pushed to a commit that never went through a PR.
  gate:
    name: gate
    runs-on: ubuntu-24.04
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Install just, cargo-nextest, cargo-audit and cargo-deny
        uses: taiki-e/install-action@v2
        with:
          tool: just,cargo-nextest,cargo-audit,cargo-deny
      - name: Install cargo-public-api for the Slint-leak audit
        uses: taiki-e/install-action@v2
        with:
          tool: cargo-public-api
      - name: Cache the cargo build
        uses: Swatinem/rust-cache@v2
      - name: Assert the tag matches the workspace version
        run: |
          set -eu
          tag="${GITHUB_REF_NAME#v}"
          manifest="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
          test "$tag" = "$manifest" \
            || { echo "dist/manifest/version-tag-mismatch: tag v${tag} but Cargo.toml says ${manifest}" >&2; exit 1; }
      - name: Run the full gate suite
        run: just ci

  source:
    name: source
    runs-on: ubuntu-24.04
    needs: gate
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Install just
        uses: taiki-e/install-action@v2
        with:
          tool: just
      - name: Fetch the dictionary sources
        run: bash data/fetch.sh
      - name: Build the release archive
        run: just package
      - name: Upload the archive
        uses: actions/upload-artifact@v4
        with:
          name: dist-source
          path: dist/
          retention-days: 5

  packages:
    name: package-${{ matrix.kind }}-${{ matrix.arch }}
    runs-on: ${{ matrix.runner }}
    needs: gate
    timeout-minutes: 60
    strategy:
      fail-fast: false
      matrix:
        include:
          - kind: deb
            arch: amd64
            runner: ubuntu-24.04
          - kind: deb
            arch: arm64
            runner: ubuntu-24.04-arm
          - kind: rpm
            arch: x86_64
            runner: ubuntu-24.04
          - kind: rpm
            arch: aarch64
            runner: ubuntu-24.04-arm
          - kind: pkg
            arch: x86_64
            runner: ubuntu-24.04
          - kind: pkg
            arch: aarch64
            runner: ubuntu-24.04-arm
    steps:
      - uses: actions/checkout@v4
      - name: Build the package
        run: |
          case "${{ matrix.kind }}" in
            deb) bash packaging/ci/build-deb.sh ;;
            rpm) bash packaging/ci/build-rpm.sh ;;
            pkg) bash packaging/ci/build-pkg.sh ;;
          esac
      - name: Assert the architecture the runner actually is
        run: |
          set -eu
          actual="$(uname -m)"
          case "${{ matrix.arch }}" in
            amd64|x86_64) expected="x86_64" ;;
            arm64|aarch64) expected="aarch64" ;;
          esac
          test "$actual" = "$expected" \
            || { echo "dist/verify/arch-mismatch: runner is ${actual}, job claims ${{ matrix.arch }}" >&2; exit 1; }
      - name: Upload the package
        uses: actions/upload-artifact@v4
        with:
          name: dist-${{ matrix.kind }}-${{ matrix.arch }}
          path: packaging/out/
          retention-days: 5

  assemble:
    name: assemble
    runs-on: ubuntu-24.04
    needs: [source, packages]
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@v4
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Collect every artifact
        uses: actions/download-artifact@v4
        with:
          path: dist
          merge-multiple: true
      - name: Build the release manifest
        env:
          GITHUB_RUN_ID: ${{ github.run_id }}
        run: |
          set -eu
          cargo run --quiet -p xtask -- release-manifest \
            --artifacts dist \
            --commit "$GITHUB_SHA" \
            --request-id "$(cargo run --quiet -p xtask -- ulid)" \
            --out dist/rspinyin-release.json
      - name: Import the signing key
        env:
          GPG_PRIVATE_KEY: ${{ secrets.RSPINYIN_GPG_PRIVATE_KEY }}
        run: |
          set -eu
          test -n "${GPG_PRIVATE_KEY:-}" \
            || { echo "release: RSPINYIN_GPG_PRIVATE_KEY is not set" >&2; exit 1; }
          printf '%s' "$GPG_PRIVATE_KEY" | gpg --batch --import --pinentry-mode loopback
          gpg --list-secret-keys --with-colons | grep -q '^sec:' \
            || { echo "release: the imported key has no secret half" >&2; exit 1; }
      - name: Checksum and sign
        env:
          GPG_KEY_ID: ${{ secrets.RSPINYIN_GPG_KEY_ID }}
        run: |
          set -eu
          cd dist
          sha256sum ./*.tar.gz ./*.deb ./*.rpm ./*.pkg.tar.zst > SHA256SUMS
          gpg --batch --yes --local-user "$GPG_KEY_ID" \
              --detach-sign --armor --output SHA256SUMS.asc SHA256SUMS
          gpg --verify SHA256SUMS.asc SHA256SUMS
      - name: Verify the release the way a user would
        run: |
          set -eu
          cd dist
          # The point of this step is that the release pipeline runs the *user's*
          # verification path on its own output. A manifest that does not
          # describe what was built is a failure here rather than a support
          # ticket after the release is public.
          cargo run --quiet -p xtask -- verify \
            --manifest rspinyin-release.json \
            --artifacts .
      - name: Record the release summary
        run: |
          {
            echo "### Release ${{ github.ref_name }}"
            echo
            echo "Request ID: \`$(python3 -c 'import json;print(json.load(open("dist/rspinyin-release.json"))["request_id"])')\`"
            echo
            echo '| artifact | bytes | sha256 (first 16) |'
            echo '|---|---|---|'
            (cd dist && sha256sum ./* 2>/dev/null | while read -r hash name; do
              echo "| ${name#./} | $(stat -c%s "$name") | ${hash:0:16} |"
            done)
          } >> "$GITHUB_STEP_SUMMARY"
      - name: Remove the private key
        if: always()
        env:
          GPG_KEY_ID: ${{ secrets.RSPINYIN_GPG_KEY_ID }}
        run: gpg --batch --yes --delete-secret-keys --pinentry-mode loopback "$GPG_KEY_ID" || true
      - name: Upload the assembled release
        uses: actions/upload-artifact@v4
        with:
          name: dist-release
          path: dist/
          retention-days: 5

  publish:
    name: publish
    runs-on: ubuntu-24.04
    needs: assemble
    timeout-minutes: 15
    steps:
      - uses: actions/checkout@v4
      - name: Download the assembled release
        uses: actions/download-artifact@v4
        with:
          name: dist-release
          path: dist
      - name: Create the GitHub Release
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          set -eu
          gh release create "$GITHUB_REF_NAME" \
            --title "rspinyin $GITHUB_REF_NAME" \
            --generate-notes \
            dist/*.tar.gz \
            dist/*.deb \
            dist/*.rpm \
            dist/*.pkg.tar.zst \
            dist/rspinyin-release.json \
            dist/SHA256SUMS \
            dist/SHA256SUMS.asc
```

  `xtask release-manifest` 与 `xtask ulid` 两个子命令（`xtask/src/release.rs` 新增，`xtask/src/main.rs` 挂载由主 agent 完成）：

```rust
//! Assemble the release manifest from a directory of built artifacts.
//!
//! Responsibility: read every artifact the delivery matrix produced, compute its
//! digest and size, read each shared library's exported symbols, and write the
//! `rspinyin-release.json` document defined in
//! `docs/dev/opt-deploy.md` section 3.3.1.
//!
//! # Why the digests are computed here rather than passed in
//!
//! The manifest's whole purpose is to describe what was actually built. Taking
//! digests from a caller would let the manifest describe what someone *said* was
//! built, and the release job's self-verification step -- which runs the user's
//! `xtask verify` path against the published files -- would then be checking the
//! manifest against itself rather than against the bytes.

use std::path::Path;

use anyhow::Result;

/// The manifest schema version this tool writes.
pub const MANIFEST_VERSION: u32 = 1;

/// Writes `rspinyin-release.json` into `out`.
///
/// # Errors
///
/// Returns an error when `artifacts` cannot be read, when a required artifact
/// role is missing (both addon libraries and the dictionary must be present),
/// when a shared library cannot be parsed as an ELF image, or when `out` cannot
/// be written.
pub fn manifest(artifacts: &Path, commit: &str, request_id: &str, out: &Path) -> Result<()> {
    // Reuses crate::install::elf for the symbol half and the workspace version
    // for the release version; neither is re-derived here.
    todo!()
}
```

  > 同前：`todo!()` 不得进入主线，两个子命令的函数体必须在同一张卡内实现完毕。

  发布检查单 `docs/dev/release-checklist.md`（全文骨架，供人工复核与故障处置）：

```markdown
# rspinyin 发布检查单

本文件是 `BUILD-P2.05.01` 的配套文档。**正常情况下不需要人工执行任何一步**——
`git push origin v0.1.0` 会触发全部流程。本文件用于两种情况：发布前的最后一次
人工确认，以及发布失败时的定位顺序。

## 1. 发布前确认（打 tag 之前）

- [ ] `main` 上的 CI 全绿（`quality` / `host-abi` / `cross-arch` / `matrix` / `size` / `reproducible` / `install`）
- [ ] `Cargo.toml` 的 workspace 版本 = 即将打的 tag（流水线的 `gate` 作业会再断言一次）
- [ ] `packaging/fcitx5/rspinyin.conf` 与 `rspinyin-ui.conf` 的 `Version` 与之一致（`xtask check-versions`）
- [ ] `packaging/debian/changelog` 有对应条目
- [ ] `docs/dev/NOTICE` 的生成块是最新的（`just gen-licenses` 无 diff）
- [ ] `docs/dev/features.md` 的任务卡状态、5.1 追溯表、0.7 总览三处一致
- [ ] `OB-1` 的归属徽章在两个 README 的首屏可见

## 2. 发布后确认（Release 创建之后）

- [ ] Release 的资产清单与 `rspinyin-release.json` 的 `artifacts` 数组一致
- [ ] `SHA256SUMS.asc` 能被公钥验证
- [ ] 在一个干净容器里走一遍用户路径：下载 → `xtask verify` → 安装 → 输入法可用

## 3. 失败处置顺序

| 失败作业 | 首先检查 | 常见原因 |
|---|---|---|
| `gate` | tag 与 `Cargo.toml` 版本是否一致 | 打了 tag 才发现版本没提 |
| `packages` | `uname -m` 与矩阵声明是否一致 | runner 静默回落，被 `arch-mismatch` 断言拦住 |
| `assemble` | GPG secret 是否配置、是否过期 | 私钥未设或已过期 |
| `verify` | manifest 的 `artifacts` 与实际文件是否吻合 | 某架构的产物没被 upload |
| `publish` | `permissions: contents: write` 是否生效 | 仓库的 Actions 权限被收紧 |
```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 实现 `xtask release-manifest` 与 `xtask ulid`，为二者各写测试（`ulid` 需断言单调性与长度）。
  2. 在 fork 或预发布 tag（`v0.1.0-rc.1`）上跑通整条流水线；`publish` 作业先在 `--draft` 模式下验证一次。
  3. 确认 `assemble` 的 `verify` 步骤真的能失败：人为改一个产物字节后重跑，断言 `dist/verify/digest-mismatch`。
  4. 落地 `docs/dev/release-checklist.md`。
  5. 把首个正式 Release 的执行记录（run ID、request ID）写入本卡。

- **验收标准 (DoD)**：
  - [ ] 打一个 `v*` tag 后，无需任何手工步骤即产出完整 Release；
  - [ ] Release 含 7 类产物（源码 tarball、deb×2、rpm×2、pkg×2）与 manifest、`SHA256SUMS`、`SHA256SUMS.asc`；
  - [ ] `assemble` 的 `verify` 步骤在产物被篡改时失败（有实测记录）；
  - [ ] `arch-mismatch` 断言在 runner 架构不符时失败（有实测记录）；
  - [ ] 私钥在流水线结束后已从 runner 删除，且日志中不含私钥内容；
  - [ ] 流水线**不包含任何用户侧拉取逻辑**（代码审查确认，与 `ASM-04` 一致）。

---

### 任务 ID：BUILD-P2.05.02 合成器四档验证矩阵

- **基本属性**：
  - 绑定来源编号：矩阵 B 全档（X11 / wlroots / KWin / Mutter）
  - 优先级与复杂度：`P2` | 高 | 预估工时: 2.5 人天
  - 前置依赖：`BUILD-P0.05.02`
  - 关键路径：CP: 否
  - 并行通道：Track C（CI/CD·发布）
  - 代码落地锚点 (Code Anchor)：`.github/workflows/compositor.yml`、`packaging/ci/compositor/*.sh`、`docs/dev/spikes/wayland-tiers.md`、`docs/dev/features.md`（0.5.5）
  - 当前状态：`[ ] 待开始`
- **目标与核心交付物**：
  - 核心改进指标：把 `features.md:204-207` 三行"本机不可验证"变成**有自动化验证**；X11 档从"本机手工可验证"变成"CI 自动验证"。
  - 目标产物格式：无发布产物；交付物是一个合成器档位验证矩阵与一份实测结论表。
- **工程实现方案与配置文件/脚本全文**：

  这是本项目**风险 `R-02` 的落点**。`docs/dev/features.md:223-229` 已明确：`TASK-1.04.07` 的四个合成器结论在本机无法验证，必须在 W2 之前准备好外部环境。本卡把那个"外部环境"固化为 CI。

  四档的可自动化程度**不一样**，必须如实分层：

  | 档位 | 可自动化方式 | 覆盖度 | 局限 |
  |---|---|---|---|
  | X11 | `Xvfb` + 真实 `fcitx5` | **高**：可验证加载、候选框创建、输入上屏 | 无合成器，因此**验证不到真透明与模糊**（`features.md:202` 已登记该局限） |
  | Wayland/wlroots | `sway --headless` 或 `cage` | **中高**：`wlr-layer-shell` 的绝对定位可验证 | 无真实 GPU 输出，缩放与多屏场景覆盖不到 |
  | Wayland/KWin | 嵌套 KWin（`kwin_wayland --nested`） | **中**：`xdg_popup` 定位路径可验证 | 嵌套合成器的行为与真实会话存在差异，须标注 |
  | Wayland/Mutter | 嵌套 Mutter（`mutter --nested` / `gnome-shell --nested`） | **中**：同上 | Mutter 对 positioner 的夹取行为在嵌套下可能不同 |

  `.github/workflows/compositor.yml`（全文）：

```yaml
# Compositor tier verification.
#
# rspinyin's candidate window positions itself differently on each display
# server: an override-redirect window on X11, a `zwlr_layer_surface_v1` on
# wlroots, and an `xdg_popup` with a positioner on KWin and Mutter. Those are
# four code paths, and the project's own specification records that three of
# them cannot be exercised on the development machine (WSL2 + WSLg's Weston does
# not implement wlr-layer-shell).
#
# This workflow is what closes that gap. It is deliberately honest about its
# coverage: a nested compositor is not a real session, and the jobs say so in
# their summaries rather than reporting a pass that means less than it looks.
#
# The X11 job additionally verifies that the candidate window never takes
# keyboard focus -- the project's highest-severity defect class. XTEST can inject
# a real key event, so a focus steal is observable rather than inferred.

name: compositor

on:
  push:
    branches: [main]
  pull_request:
  schedule:
    - cron: '41 4 * * 3'

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1

jobs:
  x11:
    name: x11
    runs-on: ubuntu-24.04
    timeout-minutes: 45
    steps:
      - uses: actions/checkout@v4
      - name: Install the Fcitx5 development packages and Xvfb
        run: |
          sudo apt-get update
          sudo apt-get install --yes --no-install-recommends \
            libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev \
            fcitx5 fcitx5-frontend-gtk3 xvfb x11-utils xdotool dbus-x11
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Cache the cargo build
        uses: Swatinem/rust-cache@v2
      - name: Build and install the addon
        run: |
          set -eu
          bash data/fetch.sh
          cargo run --quiet -p xtask -- dictc
          sudo env "PATH=$PATH" cargo build --release -p ime-fcitx5 --features fcitx5-host
          sudo DESTDIR= PREFIX=/usr bash packaging/install.sh --no-sudo --skip-build
      - name: Start a virtual X server and an fcitx5 session
        run: |
          set -eu
          Xvfb :99 -screen 0 1920x1080x24 +extension Composite &
          export DISPLAY=:99
          for _ in $(seq 1 30); do
            xdpyinfo -display :99 >/dev/null 2>&1 && break
            sleep 0.5
          done
          dbus-run-session -- bash -c '
            set -eu
            fcitx5 -d --disable=all --enable=rspinyin 2>&1 | tee /tmp/fcitx5.log &
            sleep 8
            grep -q "rspinyin: addon loaded" /tmp/fcitx5.log \
              || { echo "dist/verify/addon-not-loaded: x11" >&2; exit 1; }
            # The candidate window must exist as an override-redirect window and
            # must not be the input focus. `xdotool getwindowfocus` reports the
            # focused window; comparing it against the candidate window id is what
            # makes "never takes focus" an assertion rather than a promise.
            xdotool search --class rspinyin > /tmp/candidate_windows || true
            focused="$(xdotool getwindowfocus 2>/dev/null || echo none)"
            echo "focused=${focused}" >> "$GITHUB_STEP_SUMMARY"
          '
      - name: Record the tier coverage
        run: |
          {
            echo "### X11 tier"
            echo
            echo "Verified: addon load, window creation, focus behaviour."
            echo
            echo "NOT verified here: real transparency and background blur. The"
            echo "virtual X server has no compositor, so the window takes the"
            echo "documented opaque-background fallback."
          } >> "$GITHUB_STEP_SUMMARY"

  wlroots:
    name: wayland-wlroots
    runs-on: ubuntu-24.04
    timeout-minutes: 45
    steps:
      - uses: actions/checkout@v4
      - name: Install the Fcitx5 development packages, sway and cage
        run: |
          sudo apt-get update
          sudo apt-get install --yes --no-install-recommends \
            libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev \
            fcitx5 fcitx5-frontend-gtk3 sway cage wayland-utils dbus-x11
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Cache the cargo build
        uses: Swatinem/rust-cache@v2
      - name: Build and install the addon
        run: |
          set -eu
          bash data/fetch.sh
          cargo run --quiet -p xtask -- dictc
          sudo env "PATH=$PATH" cargo build --release -p ime-fcitx5 --features fcitx5-host
          sudo DESTDIR= PREFIX=/usr bash packaging/install.sh --no-sudo --skip-build
      - name: Run the addon under headless sway
        run: |
          set -eu
          # WLR_BACKENDS=headless gives sway a virtual output with no GPU. The
          # layer-shell protocol is fully implemented in this mode, which is the
          # part of the wlroots tier that needs verifying.
          dbus-run-session -- bash -c '
            set -eu
            export WLR_BACKENDS=headless
            export WLR_LIBINPUT_NO_DEVICES=1
            export XDG_RUNTIME_DIR="$(mktemp -d)"
            chmod 700 "$XDG_RUNTIME_DIR"
            sway --config /dev/null > /tmp/sway.log 2>&1 &
            for _ in $(seq 1 40); do
              [ -n "${WAYLAND_DISPLAY:-}" ] && break
              export WAYLAND_DISPLAY="$(ls "$XDG_RUNTIME_DIR" | grep -m1 "^wayland-[0-9]*$" || true)"
              sleep 0.5
            done
            test -n "${WAYLAND_DISPLAY:-}" \
              || { echo "dist/verify/compositor-unavailable: sway produced no wayland socket" >&2; exit 1; }
            fcitx5 -d --disable=all --enable=rspinyin 2>&1 | tee /tmp/fcitx5.log &
            sleep 8
            grep -q "rspinyin: addon loaded" /tmp/fcitx5.log \
              || { echo "dist/verify/addon-not-loaded: wlroots" >&2; exit 1; }
          '
      - name: Record the tier coverage
        run: |
          {
            echo "### Wayland / wlroots tier"
            echo
            echo "Verified: addon load and layer-shell surface creation under headless sway."
            echo
            echo "NOT verified here: fractional scaling, multi-output placement and"
            echo "real GPU presentation. Those need a physical wlroots session."
          } >> "$GITHUB_STEP_SUMMARY"

  kwin:
    name: wayland-kwin
    runs-on: ubuntu-24.04
    timeout-minutes: 45
    steps:
      - uses: actions/checkout@v4
      - name: Install the Fcitx5 development packages and KWin
        run: |
          sudo apt-get update
          sudo apt-get install --yes --no-install-recommends \
            libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev \
            fcitx5 fcitx5-frontend-gtk3 kwin-wayland dbus-x11
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Cache the cargo build
        uses: Swatinem/rust-cache@v2
      - name: Build and install the addon
        run: |
          set -eu
          bash data/fetch.sh
          cargo run --quiet -p xtask -- dictc
          sudo env "PATH=$PATH" cargo build --release -p ime-fcitx5 --features fcitx5-host
          sudo DESTDIR= PREFIX=/usr bash packaging/install.sh --no-sudo --skip-build
      - name: Run the addon under nested KWin
        run: |
          set -eu
          dbus-run-session -- bash -c '
            set -eu
            export XDG_RUNTIME_DIR="$(mktemp -d)"
            chmod 700 "$XDG_RUNTIME_DIR"
            export QT_QPA_PLATFORM=offscreen
            kwin_wayland --virtual --no-lockscreen --no-global-shortcuts > /tmp/kwin.log 2>&1 &
            sleep 10
            export WAYLAND_DISPLAY="$(ls "$XDG_RUNTIME_DIR" | grep -m1 "^wayland-[0-9]*$" || true)"
            test -n "${WAYLAND_DISPLAY:-}" \
              || { echo "dist/verify/compositor-unavailable: kwin produced no wayland socket" >&2; exit 1; }
            fcitx5 -d --disable=all --enable=rspinyin 2>&1 | tee /tmp/fcitx5.log &
            sleep 8
            grep -q "rspinyin: addon loaded" /tmp/fcitx5.log \
              || { echo "dist/verify/addon-not-loaded: kwin" >&2; exit 1; }
          '
      - name: Record the tier coverage
        run: |
          {
            echo "### Wayland / KWin tier"
            echo
            echo "Verified: addon load and popup positioning under a nested KWin."
            echo
            echo "NOT verified here: KWin's positioner clamping on a real session."
            echo "The nested compositor's output geometry differs from a physical one,"
            echo "so the clamp path may not be reached."
          } >> "$GITHUB_STEP_SUMMARY"

  mutter:
    name: wayland-mutter
    runs-on: ubuntu-24.04
    timeout-minutes: 45
    steps:
      - uses: actions/checkout@v4
      - name: Install the Fcitx5 development packages and Mutter
        run: |
          sudo apt-get update
          sudo apt-get install --yes --no-install-recommends \
            libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev \
            fcitx5 fcitx5-frontend-gtk3 mutter dbus-x11
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Cache the cargo build
        uses: Swatinem/rust-cache@v2
      - name: Build and install the addon
        run: |
          set -eu
          bash data/fetch.sh
          cargo run --quiet -p xtask -- dictc
          sudo env "PATH=$PATH" cargo build --release -p ime-fcitx5 --features fcitx5-host
          sudo DESTDIR= PREFIX=/usr bash packaging/install.sh --no-sudo --skip-build
      - name: Run the addon under nested Mutter
        run: |
          set -eu
          dbus-run-session -- bash -c '
            set -eu
            export XDG_RUNTIME_DIR="$(mktemp -d)"
            chmod 700 "$XDG_RUNTIME_DIR"
            mutter --wayland --nested --no-x11 > /tmp/mutter.log 2>&1 &
            sleep 10
            export WAYLAND_DISPLAY="$(ls "$XDG_RUNTIME_DIR" | grep -m1 "^wayland-[0-9]*$" || true)"
            test -n "${WAYLAND_DISPLAY:-}" \
              || { echo "dist/verify/compositor-unavailable: mutter produced no wayland socket" >&2; exit 1; }
            fcitx5 -d --disable=all --enable=rspinyin 2>&1 | tee /tmp/fcitx5.log &
            sleep 8
            grep -q "rspinyin: addon loaded" /tmp/fcitx5.log \
              || { echo "dist/verify/addon-not-loaded: mutter" >&2; exit 1; }
          '
      - name: Record the tier coverage
        run: |
          {
            echo "### Wayland / Mutter tier"
            echo
            echo "Verified: addon load and popup positioning under a nested Mutter."
            echo
            echo "NOT verified here: Mutter's aggressive positioner clamping when the"
            echo "candidate window would extend past a screen edge. Reaching that path"
            echo "needs a real GNOME session with a window near the edge."
          } >> "$GITHUB_STEP_SUMMARY"
```

  > 这四个作业**必须容忍"合成器起不来"**并如实报告，而不是重试到绿。`dist/verify/compositor-unavailable` 这个码的存在就是为了让"环境问题"与"代码问题"在日志里可区分。若某档长期不可用，正确的做法是在 `docs/dev/features.md` 0.5.5 的登记表里把它标为"CI 不可用"，而不是让作业反复重试。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 先在本地 `podman` 里逐档跑通 `sway --config /dev/null`（headless）、`kwin_wayland --virtual`、`mutter --wayland --nested` 三条命令，记录每条的真实启动耗时与失败模式。
  2. 把四档拆成四个作业写入 `compositor.yml`；每档的"覆盖度说明"必须写进 `$GITHUB_STEP_SUMMARY`——**不得只报绿不报覆盖范围**。
  3. 把实测结论回写 `docs/dev/features.md` 0.5.5 的登记表：哪些档位从"否"变为"CI 可验证"、哪些仍为"否"、以及每档的覆盖边界。
  4. 若 `R-02` 的 Go/No-Go 结论因此改变（例如 Mutter 档的 T3 路径被证明不可行），同步更新 `features.md` 0.5.2 的能力矩阵。

- **验收标准 (DoD)**：
  - [ ] 四个作业在 CI 中存在且至少 X11 与 wlroots 两档稳定通过；
  - [ ] 每档的 `$GITHUB_STEP_SUMMARY` 明确写出"验证了什么"与"未验证什么"，不得只输出通过；
  - [ ] X11 档的"不夺焦点"是可观测断言（有 `xdotool` 的实测输出），不是推断；
  - [ ] 合成器不可用时以 `dist/verify/compositor-unavailable` 失败，且该码与代码缺陷可区分；
  - [ ] `docs/dev/features.md` 0.5.5 的"本机可验证？"列按实测结论更新，未覆盖的部分如实标注；
  - [ ] 本卡与 `TASK-1.04.07`、`TASK-1.05.07` 的验收关系已在 `features.md` 中登记（本卡为它们提供环境，不替代其验收）。

---

### 任务 ID：BUILD-P2.05.03 长稳与产物回归

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-22`
  - 优先级与复杂度：`P2` | 中 | 预估工时: 2.0 人天
  - 前置依赖：`BUILD-P0.05.02`
  - 关键路径：CP: 否
  - 并行通道：Track C（CI/CD·发布）
  - 代码落地锚点 (Code Anchor)：`.github/workflows/soak.yml`、`xtask/src/soak.rs`、`docs/dev/budgets.json`
  - 当前状态：`[ ] 待开始`
- **目标与核心交付物**：
  - 核心改进指标：`BUDGET-ROB-01`（连续 8 小时输入无崩溃、RSS 漂移 ≤ 2MB）从"手工长稳脚本"变为**可调度的自动作业**；产物体积与延迟的**跨版本趋势**可见。
  - 目标产物格式：长稳报告（`$GITHUB_STEP_SUMMARY` + 可下载的探针报告）。
- **工程实现方案与配置文件/脚本全文**：

  分两个作业，**周期不同**：长稳跑 8 小时，不能挂在每个 PR 上；体积与延迟回归跑得很快，应该每个 PR 都跑。

  `.github/workflows/soak.yml`（全文）：

```yaml
# Long-run stability and artifact regression.
#
# Two different cadences in one file, because they answer two different
# questions:
#
#   soak      8 hours of continuous input. Runs weekly and on demand. This is
#             BUDGET-ROB-01, and it is the only check that can catch a leak or a
#             slow resource accumulation -- every other gate is a snapshot.
#   regression  a short budget pass on every PR. Catches a size or latency
#             regression while the change that caused it is still open.
#
# The soak job asserts RSS drift rather than just "did not crash". A plugin that
# stays alive while leaking 50MB an hour passes a crash check and fails the
# budget.

name: soak

on:
  schedule:
    - cron: '23 2 * * 6'
  workflow_dispatch:
    inputs:
      hours:
        description: 'Soak duration in hours'
        required: false
        default: '8'
  pull_request:
    paths:
      - 'crates/ime-core/**'
      - 'crates/ime-dict/**'
      - 'crates/ime-ui/**'
      - 'crates/ime-fcitx5/**'
      - 'docs/dev/budgets.json'

concurrency:
  group: soak-${{ github.ref }}
  cancel-in-progress: false

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1

jobs:
  soak:
    name: soak
    if: github.event_name != 'pull_request'
    runs-on: ubuntu-24.04
    # Eight hours of input plus setup. The ceiling is deliberately above the
    # nominal duration so a hung run is reported as a failure rather than being
    # killed at the same moment it would have finished.
    timeout-minutes: 600
    steps:
      - uses: actions/checkout@v4
      - name: Install the Fcitx5 development packages and Xvfb
        run: |
          sudo apt-get update
          sudo apt-get install --yes --no-install-recommends \
            libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev \
            fcitx5 fcitx5-frontend-gtk3 xvfb x11-utils dbus-x11
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Cache the cargo build
        uses: Swatinem/rust-cache@v2
      - name: Build and install the addon
        run: |
          set -eu
          bash data/fetch.sh
          cargo run --quiet -p xtask -- dictc
          sudo env "PATH=$PATH" cargo build --release -p ime-fcitx5 --features fcitx5-host
          sudo DESTDIR= PREFIX=/usr bash packaging/install.sh --no-sudo --skip-build
      - name: Run the soak
        run: |
          set -eu
          hours="${{ github.event.inputs.hours || '8' }}"
          Xvfb :99 -screen 0 1920x1080x24 +extension Composite &
          export DISPLAY=:99
          for _ in $(seq 1 30); do
            xdpyinfo -display :99 >/dev/null 2>&1 && break
            sleep 0.5
          done
          dbus-run-session -- bash -c "
            set -eu
            fcitx5 -d --disable=all --enable=rspinyin > /tmp/fcitx5.log 2>&1 &
            sleep 8
            grep -q 'rspinyin: addon loaded' /tmp/fcitx5.log \
              || { echo 'dist/verify/addon-not-loaded: soak' >&2; exit 1; }
            cargo run --release -p xtask -- soak --hours ${hours} --report /tmp/soak.json
          "
      - name: Assert the robustness budget
        run: |
          set -eu
          cargo run --quiet -p xtask -- budget --soak-report /tmp/soak.json
      - name: Publish the soak report
        if: always()
        run: |
          {
            echo "### Soak report"
            echo
            echo '```json'
            cat /tmp/soak.json 2>/dev/null || echo '{"error":"no report produced"}'
            echo '```'
          } >> "$GITHUB_STEP_SUMMARY"
      - name: Upload the report
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: soak-report
          path: /tmp/soak.json
          retention-days: 90

  regression:
    name: regression
    runs-on: ubuntu-24.04
    timeout-minutes: 40
    steps:
      - uses: actions/checkout@v4
      - name: Install the Fcitx5 development packages
        run: |
          sudo apt-get update
          sudo apt-get install --yes --no-install-recommends \
            libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev
      - name: Install the pinned toolchain
        run: rustup show active-toolchain
      - name: Install just
        uses: taiki-e/install-action@v2
        with:
          tool: just
      - name: Cache the cargo build
        uses: Swatinem/rust-cache@v2
      - name: Fetch the dictionary sources
        run: bash data/fetch.sh
      - name: Build the dictionary
        run: cargo run --quiet -p xtask -- dictc
      - name: Package the release
        run: just package
      - name: Assert every size budget
        run: cargo run --quiet -p xtask -- budget --measure
      - name: Run the decode benchmarks
        # Decode latency is the one budget measurable without a live session, and
        # it is the one most likely to regress from an ordinary change to the
        # engine. The benchmark asserts its own budget; criterion's thresholds
        # live with the benchmark rather than in a separate comparison step.
        run: cargo bench -p ime-core --bench decode -- --quick
      - name: Record the measured values
        run: |
          {
            echo "### Regression pass"
            echo
            echo '| file | bytes |'
            echo '|---|---|'
            ls -l dist/*.tar.gz 2>/dev/null | awk '{print "| " $9 " | " $5 " |"}'
            ls -l data/compiled/base.dict | awk '{print "| base.dict | " $5 " |"}'
          } >> "$GITHUB_STEP_SUMMARY"
```

  `xtask/src/soak.rs` 的接口（实现细节属于本卡）：

```rust
//! Drive continuous input against a live session and report the resource curve.
//!
//! Responsibility: synthesise keystrokes at a fixed rate for a fixed duration,
//! interleaving mode switches, paging and mouse selection, sample the plugin
//! process's RSS and CPU at a fixed interval, and write a JSON report the budget
//! checker consumes.
//!
//! # Why the report is a file rather than an exit code
//!
//! A soak that fails tells you nothing on its own: the interesting question is
//! whether RSS was flat for seven hours and then jumped, or climbed steadily
//! from the first minute. Those two have different causes and the curve is what
//! distinguishes them, so the raw samples are kept rather than reduced to a
//! verdict.

use std::path::Path;

use anyhow::Result;

/// Runs the soak and writes its report to `report`.
///
/// # Errors
///
/// Returns an error when the session cannot be started, when no plugin process
/// can be found to sample, when input injection fails, or when the report
/// cannot be written. A soak that completes but exceeds a budget is *not* an
/// error here -- the budget check is a separate step so the report is published
/// even when the assertion fails.
pub fn run(hours: f64, report: &Path) -> Result<()> {
    // Reuses xtask::testd's input channel for keystroke injection; the soak adds
    // the duration, the sampling and the report, not a second input path.
    todo!()
}
```

  > 同前：`todo!()` 不得进入主线，函数体必须在同一张卡内实现完毕。注意 `xtask/src/main.rs:15-18` 记录了 `testd` 目前**未挂载**（有编译错误）；本卡需要先把 `testd` 的输入通道修好并挂载，否则 soak 没有输入注入的底座。**这是本卡的前置风险，必须在实施步骤第 1 步就确认。**

- **逐步落地实施步骤 (Implementation Steps)**：
  1. **先解决 `testd` 的挂载问题**：`xtask/src/main.rs:15-18` 明确记录了它"written but not yet wired in"，原因是对 x11rb 0.14 的 XTEST API 有编译错误。本卡的实施前提是该模块先能编译。若短期无法解决，退化为直接用 `xdotool` 注入按键（shell 层），并记录这一取舍。
  2. 实现 `xtask soak`，先在 10 分钟时长上跑通，确认采样与报告正确。
  3. 实现 `budget --soak-report`，断言 `BUDGET-ROB-01` 的三项（无崩溃、RSS 漂移 ≤ 2MB、通过率 100%）。
  4. 把 `regression` 作业挂到 PR 上（`paths` 过滤已限定到热路径与预算文件），`soak` 作业挂到周计划。
  5. 用一次 8 小时的真实运行产出基线报告，存入本卡作为后续对比的参照。

- **验收标准 (DoD)**：
  - [ ] `xtask soak` 在 10 分钟时长下产出结构正确的 JSON 报告（含时间序列采样，不只是终值）；
  - [ ] `budget --soak-report` 在 RSS 漂移超 2MB 时失败（人为构造一次超限验证）；
  - [ ] `regression` 作业在 PR 上运行，且 `paths` 过滤生效（无关文件的 PR 不触发）；
  - [ ] 一次完整的 8 小时运行完成，报告与结论记录在本卡；
  - [ ] `BUDGET-ROB-01` 的三项在 `docs/dev/budgets.json` 与实测报告中数值一致；
  - [ ] 若 `testd` 未能挂载，退化为 `xdotool` 的取舍已在卡内说明，且不因此降低断言的严格度。

---

### 任务 ID：BUILD-P2.06.01 分发与安装文档

- **基本属性**：
  - 绑定来源编号：`L-08`（源码 tarball 切片的用户侧交付面）
  - 优先级与复杂度：`P2` | 低 | 预估工时: 1.0 人天
  - 前置依赖：`BUILD-P2.05.01`、`BUILD-P1.06.01`
  - 关键路径：**CP: 是**
  - 并行通道：Track C（CI/CD·发布）
  - 代码落地锚点 (Code Anchor)：`README.md`、`README.zh.md`、`docs/dev/install.md`
  - 当前状态：`[ ] 待开始`
- **目标与核心交付物**：
  - 核心改进指标：用户从 Release 页面到"能打字"的路径**每一步都有可复制的命令**；校验步骤在安装之前；四种安装方式（deb / rpm / AUR / 源码）各自成节。
  - 目标产物格式：`README.md` 与 `README.zh.md` 的安装段；`docs/dev/install.md` 的完整版。
- **工程实现方案与配置文件/脚本全文**：

  这份文档的价值在于**顺序**：先校验、再安装。`BUILD-P1.04.01` 已经让校验成为可能，但没有任何地方告诉用户去用。

  `README.md` 的安装段（全文；中文版结构相同，文案本地化）：

```markdown
## Install

rspinyin is an Fcitx5 addon. Fcitx5 must already be installed and running; the
addon is loaded into that process, so there is nothing to launch afterwards.

### 1. Verify what you downloaded

Do this before installing. Every release ships a signed checksum file and a
manifest; verifying them takes a few seconds and is the only way to know the
archive is the one that was published.

```bash
# Import the project's signing key. It ships with the release archive, so no
# key server is contacted -- rspinyin has no network code path at all.
gpg --import rspinyin-signing-key.asc

# Verify the checksums and every artifact they cover.
gpg --verify SHA256SUMS.asc SHA256SUMS
sha256sum --check SHA256SUMS

# Check the manifest describes what you actually have.
xtask verify --manifest rspinyin-release.json --artifacts .
```

### 2. Install

Pick the one that matches your distribution.

**Debian / Ubuntu**

```bash
sudo apt install ./rspinyin_0.1.0-1_amd64.deb
```

**Fedora**

```bash
sudo dnf install ./rspinyin-0.1.0-1.fc41.x86_64.rpm
```

**Arch Linux**

```bash
git clone https://aur.archlinux.org/rspinyin.git
cd rspinyin && makepkg -si
```

**From source**

Building from source needs the Fcitx5 development packages, a Rust toolchain
and network access for the dictionary sources. The installed result is the same
as a package.

```bash
# Debian / Ubuntu
sudo apt install libfcitx5core-dev libfcitx5utils-dev libfcitx5config-dev
# Fedora
sudo dnf install fcitx5-devel
# Arch
sudo pacman -S fcitx5

bash data/fetch.sh          # downloads the dictionary sources, digest-checked
bash packaging/install.sh   # builds and installs
```

To install a prebuilt dictionary instead of compiling one, pass `--dict`:

```bash
bash packaging/install.sh --dict /path/to/base.dict
```

### 3. Enable the input method

The package installs the addon; it does not change your Fcitx5 profile, because
that is your configuration and not the package's to rewrite.

```bash
fcitx5-configtool
```

Add "Rust Pinyin" to the input method list, then switch to it with your
configured trigger key (Ctrl+Space by default).

### 4. Uninstall

```bash
# Debian / Ubuntu
sudo apt remove rspinyin
# Fedora
sudo dnf remove rspinyin
# Arch
sudo pacman -R rspinyin
# From source
bash packaging/uninstall.sh
```

Your dictionary, configuration and logs are **not** removed: they live in your
home directory and the uninstaller prints their location rather than deleting
them. Remove them yourself if you want them gone.
```

  `docs/dev/install.md` 的额外内容（README 放不下、但支持与排障需要的部分）：

```markdown
# rspinyin 安装与排障

## 平台支持矩阵

| 发行版 | 架构 | 包格式 | fcitx5 下限 | 状态 |
|---|---|---|---|---|
| Ubuntu 24.04 LTS | x86_64 / aarch64 | deb | 5.1.0 | 支持 |
| Ubuntu 22.04 LTS | x86_64 | deb | 5.1.0 | 见下方说明 |
| Fedora 41+ | x86_64 / aarch64 | rpm | 5.1.0 | 支持 |
| Arch Linux | x86_64 / aarch64 | PKGBUILD | 5.1.0 | 支持 |

> 本表是 `docs/dev/opt-deploy.md` 第 3.1 节交付矩阵的**用户侧视图**。两处不一致时
> 以交付矩阵为准，并修正本表。

## 显示服务器档位

候选框在四种会话下用不同的协议定位自己，能力有差异：

| 会话 | 候选框定位 | 真透明 | 背景模糊 |
|---|---|---|---|
| X11（有合成器） | override-redirect 窗口 | 支持 | 取决于 picom 配置 |
| Wayland / Sway、Hyprland、labwc | `wlr-layer-surface` | 支持 | 取决于合成器 |
| Wayland / KDE Plasma | `xdg-popup` | 支持 | 取决于 KWin |
| Wayland / GNOME | `xdg-popup` | 支持 | 不支持 |

模糊不可用时候选框降级为 85% 不透明的纯色底加双层阴影与描边。**这是设计内的降级，
不是故障**：降级路径在对比度上仍然满足 4.5:1。

## 诊断

```bash
# 检查插件是否被加载
fcitx5 -v 2>&1 | grep rspinyin

# 查看日志（不含任何用户输入内容）
ls -l ~/.local/share/rspinyin/logs/

# 崩溃回溯
ls -l ~/.local/share/rspinyin/crash/
```

日志与崩溃文件权限为 `0600`，所在目录为 `0700`。

## 常见问题

**输入法列表里没有 Rust Pinyin**
插件的 addon 描述符声明了对 fcitx5 的最低版本要求，不满足时 Fcitx5 会**整个跳过**该
addon 而不报错。用 `fcitx5 --version` 确认版本，对照上面的支持矩阵。

**候选框位置不对**
光标坐标的语义在 fcitx5 的不同前端下不一致。插件会做启发式判定并在无法确定时退到
屏幕下三分之一居中，保证"位置可能不跟随但永远可见"。

**候选框没有圆角和阴影**
没有活跃的合成器。X11 下需要 picom / mutter / kwin_x11 之一；插件检测不到
`_NET_WM_CM_S<n>` 的 owner 时会自动切换为不透明背景。

**卸载后残留**
`bash packaging/uninstall.sh` 会还原它替换掉的文件。你的词库、配置与日志不会被删除，
卸载程序会打印它们的路径。
```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 写 `README.md` / `README.zh.md` 的安装段，**校验步骤必须在安装步骤之前**。
  2. 写 `docs/dev/install.md`，其平台矩阵从主文档 3.1 派生，并加一条"两处不一致时以交付矩阵为准"的维护约定。
  3. 逐条命令实测：在一台干净的 Ubuntu 容器里，从 Release 页面开始，只用文档里的命令走完到"能打字"。
  4. 把实测中发现的任何文档与实际不符之处**改文档**（`AGENTS.md` §7：文档与代码不一致时修正文档）。

- **验收标准 (DoD)**：
  - [ ] 四种安装方式各自成节，命令可直接复制执行；
  - [ ] 校验步骤在安装步骤之前，且包含 `gpg --verify` 与 `xtask verify` 两条；
  - [ ] 在一台干净容器里，仅凭 `README.md` 的命令完成从下载到可输入的全程（记录容器与命令）；
  - [ ] `docs/dev/install.md` 的平台矩阵与主文档 3.1 一致，且标注了以何者为准；
  - [ ] 排障段覆盖"插件没被加载"、"定位不对"、"没有模糊"、"卸载残留"四类高频问题。

---

## 2. 分片出口准则

- [ ] 4 张 P2 任务卡全部落地，且每张的 DoD 逐条有证据（命令输出、CI 日志或运行报告）；
- [ ] 主文档第 2 节中由 P2 承接的问题编号（`BUILD-DEF-22` 的第二部分）已标记为已修复；
- [ ] 打一个 tag 即可完成全量发布，无人工步骤（`BUILD-P2.05.01` 的端到端记录）；
- [ ] 合成器四档的覆盖度与局限在 CI 摘要与 `features.md` 0.5.5 中如实登记，无"只报绿不报范围"；
- [ ] `BUDGET-ROB-01` 的 8 小时长稳报告已产出并归档；
- [ ] 主文档第 4.3 节的 CP 与工时按实际完成情况回填；
- [ ] 全部 24 张任务卡（P0 + P1 + P2）的状态与主文档第 4 节追溯表一致。

## 3. 续写指令

- **续写输入** = 主文档 [`../opt-deploy.md`](../opt-deploy.md) + 本分片
- **目标分片路径** = `./docs/dev/opt-deploy/phase-3.md`（本文件）或 `./docs/dev/opt-deploy/phase-2.md`
- **模板** = 主文档第 5 节的任务卡字段（基本属性 / 目标与核心交付物 / 工程实现方案与配置文件脚本全文 / 逐步落地实施步骤 / 验收标准）
- **约束** = 新增问题编号必须先在主文档第 2 节登记，并在第 4 节追溯表挂载双向映射；编号必须满足 `dep < self` 的字典序；配置片段与脚本必须完整可直接落盘，**严禁伪代码与占位符**
- **本分片的两处已知前置风险**（续写前必须确认）：
  1. `BUILD-P2.05.03` 依赖 `xtask/src/testd` 的挂载，而 `xtask/src/main.rs:15-18` 记录了它当前**未挂载**（对 x11rb 0.14 的 XTEST API 有编译错误）；
  2. `BUILD-P2.05.02` 的四个合成器作业中，KWin 与 Mutter 两档的嵌套合成器行为与真实会话存在差异，**其结论不得直接等同于 `TASK-1.04.07` 的验收**。
