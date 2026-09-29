# rspinyin 跨平台构建、分发与 CI/CD 优化工程规范 · Phase 2 分片（P1 任务卡）

> 文档版本: v1.0 ｜ 系统形态: Linux 桌面输入法插件（Fcitx5 进程内 cdylib）｜ 架构基线: Cargo workspace（edition 2024 / MSRV 1.85 / toolchain 1.98.0）｜ 关联 ADR: [`../adr/0000-upstream-decisions.md`](../adr/0000-upstream-decisions.md)、[`../adr/0003-ui-role-separate-addon.md`](../adr/0003-ui-role-separate-addon.md) ｜ 最后同步 Commit: `ee0dbfb` ｜ 维护约定: 代码演进后必须回写主文档第 2 节问题清单状态与第 3 节交付矩阵
>
> **回链主文档**：[`../opt-deploy.md`](../opt-deploy.md) — 本分片承载 P1 任务卡（`BUILD-P1.*`，9 张）。假设清单（`ASM-01`~`ASM-14`）、问题总清单（`BUILD-DEF-01`~`BUILD-DEF-23`）、交付矩阵（`L-01`~`L-08` 与合成器四档）、追溯表与关键路径汇总**全部在主文档**，本分片不重复定义，只引用。

---

## 0. 分片基线（引用主文档，不重复定义）

本分片中的每一张任务卡都以下列主文档结论为前提，不再复述：

| 引用 | 来源 | 本分片依赖的关键结论 |
|---|---|---|
| 交付形态 | 主文档 0.1 / `ASM-01` | 产物是一个被 `fcitx5` `dlopen` 的 cdylib；无独立可执行文件、无应用包、无启动器 |
| 平台矩阵 | 主文档 3.1 / `ASM-05`、`ASM-07` | 8 个切片：Ubuntu 24.04/22.04 × Fedora 40+ × Arch，各含 x86_64 与 aarch64；**无 Windows / macOS / AppImage / Flatpak / Snap 切片** |
| 更新通道 | 主文档 3.3 / `ASM-04` | **不存在自动更新通道**。发布侧生成 `rspinyin-release.json` + `SHA256SUMS` + GPG 分离签名；用户侧只校验，不拉取 |
| 错误码 | 主文档 3.3.2 | `dist/manifest/*` 与 `dist/verify/*` 九个码，一经发布不得改写 |
| 合规 | 主文档 `ASM-06` | `OB-1`~`OB-6` 是发布阻塞项；`OB-2` 禁止单独分发 Slint，`OB-3` 禁止嵌入式场景 |
| 剥离契约 | 主文档 `ASM-13` | **剥离是打包期动作，编译期绝不 strip**；剥离后必须复检 `fcitx_addon_factory_instance` |
| 验证边界 | 主文档 `ASM-11` / `features.md:199-207` | 本机（WSL2+WSLg）只能验证 X11 档；wlroots / KWin / Mutter 三档不可验证 |
| P0 产出 | 主文档第 5 节 | `xtask package`（`BUILD-P0.02.01`）产出 tarball + manifest + `SHA256SUMS`；`check-host` 与 `just ci` 已就位 |

---

## 1. 任务卡

### 任务 ID：BUILD-P1.01.01 跨发行版编译矩阵

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-23`、`L-03`、`L-04`
  - 优先级与复杂度：`P1` | 中 | 预估工时: 2.5 人天
  - 前置依赖：`BUILD-P0.01.02`
  - 关键路径：**CP: 是**
  - 并行通道：Track A（编译与产物瘦身）
  - 代码落地锚点 (Code Anchor)：`.github/workflows/matrix.yml`、`packaging/containers/*.Dockerfile`
  - 当前状态：`[ ] 待开始`
- **目标与核心交付物**：
  - 核心改进指标：Fedora 40+ 与 Arch Linux 上的**完整构建 + 门禁 + 安装可逆性**每次 PR 都跑一遍；`docs/dev/features.md:4451` 的"三套包可安装、可卸载且可逆"验收从"无基础"变为"有基础"。
  - 目标产物格式：无发布产物；交付物是一个跨发行版验证矩阵。
- **工程实现方案与配置文件/脚本全文**：

  用**发行版官方容器**而非 GitHub 的发行版 runner：Fedora 与 Arch 在 GitHub 上都没有原生 runner，容器是唯一可控的方式；且容器镜像可以钉到 digest，保证矩阵不因基础镜像漂移而 flake。

  `packaging/containers/fedora.Dockerfile`（全文）：

```dockerfile
# Build and verify rspinyin on Fedora.
#
# The image is deliberately not pinned to a digest in the repository -- the
# workflow pins it instead, so a developer can rebuild locally against
# `fedora:latest` while CI stays reproducible.
FROM fedora:41

# fcitx5-devel brings the Fcitx5Core/Fcitx5Utils/Fcitx5Config pkg-config files
# and the C++ headers the glue compiles against. pkgconf is what build.rs
# queries them with.
RUN dnf install --assumeyes --setopt=install_weak_deps=False \
        cargo \
        gcc-c++ \
        fcitx5-devel \
        pkgconf-pkg-config \
        findutils \
        nmap-ncat \
    && dnf clean all

# `just` and `cargo-nextest` come from prebuilt binaries at workflow time, not
# from here: they are pinned by the install action, and baking them in would
# mean rebuilding the image on every tool version bump.
WORKDIR /work
```

  `packaging/containers/arch.Dockerfile`（全文）：

```dockerfile
# Build and verify rspinyin on Arch Linux.
#
# Arch is a rolling distribution with no versioned base image, so this container
# is inherently less reproducible than the Fedora one. That is accepted rather
# than worked around: Arch's value in the matrix is precisely that it catches
# breakage from a newer toolchain or a newer fcitx5 before a stable release does.
FROM archlinux:base-devel

RUN pacman --sync --refresh --noconfirm --needed \
        rust \
        fcitx5 \
        pkgconf \
        findutils \
    && pacman --sync --clean --noconfirm

WORKDIR /work
```

  `.github/workflows/matrix.yml`（全文）：

```yaml
# Cross-distribution build matrix.
#
# The three distributions in the platform baseline build differently: Debian and
# Ubuntu put the addon under /usr/lib/<triplet>/fcitx5, Fedora under
# /usr/lib64/fcitx5, Arch under /usr/lib/fcitx5. `xtask install` derives all
# three from pkg-config rather than assuming one, and this workflow is what
# proves the derivation is right -- a hardcoded path would install the plugin
# where Fcitx5 never looks for it, on exactly two of the three.
#
# Each job runs the full gate suite plus an install/uninstall round trip, so a
# distribution that compiles but cannot install is a failure rather than a
# surprise at release time.

name: matrix

on:
  push:
    branches: [main]
  pull_request:
  schedule:
    # Weekly, so a rolling distribution's drift is caught between releases
    # rather than during one. Monday 03:17 UTC -- off the hour and off the
    # half hour, because that is when everyone else's cron runs.
    - cron: '17 3 * * 1'

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1
  SOURCE_DATE_EPOCH: 1790899200

jobs:
  fedora:
    name: fedora
    runs-on: ubuntu-24.04
    timeout-minutes: 45
    container:
      # Pinned by digest: a rolling base image must not be able to change what
      # this job asserts. Update the digest deliberately, not by accident.
      image: fedora:41@sha256:0000000000000000000000000000000000000000000000000000000000000000
    steps:
      - uses: actions/checkout@v4
      - name: Install the build dependencies
        run: |
          dnf install --assumeyes --setopt=install_weak_deps=False \
            cargo gcc-c++ fcitx5-devel pkgconf-pkg-config findutils which tar gzip git
      - name: Install just and cargo-nextest
        run: |
          curl --proto '=https' --tlsv1.2 -sSf https://just.systems/install.sh \
            | bash -s -- --to /usr/local/bin
          curl -LsSf https://get.nexte.st/latest/linux | tar zxf - -C /usr/local/bin
      - name: Report the resolved versions
        run: |
          {
            echo "### Fedora"
            echo
            echo '```'
            cat /etc/fedora-release
            rustc --version
            pkg-config --modversion Fcitx5Core
            echo '```'
          } >> "$GITHUB_STEP_SUMMARY"
      - name: Run the gate suite
        run: just ci
      - name: Build with the real C ABI linked
        run: just check-host
      - name: Install, assert and uninstall
        run: |
          set -eu
          mkdir -p /tmp/stage
          find /tmp/stage -printf '%P\t%s\n' | sort > /tmp/stage.before
          bash data/fetch.sh
          cargo run --quiet -p xtask -- dictc
          DESTDIR=/tmp/stage PREFIX=/usr bash packaging/install.sh --no-sudo --skip-build
          # Fedora puts the addon under /usr/lib64/fcitx5; asking pkg-config is
          # what makes this assertion valid on all three distributions.
          addondir="$(pkg-config --variable=addondir Fcitx5Core || true)"
          if [ -z "$addondir" ]; then
            addondir="$(pkg-config --variable=libdir Fcitx5Core)/fcitx5"
          fi
          test -f "/tmp/stage${addondir}/librspinyin.so" \
            || { echo "dist/verify/artifact-missing: ${addondir}/librspinyin.so" >&2; exit 1; }
          nm -D --defined-only "/tmp/stage${addondir}/librspinyin.so" \
            | grep -q fcitx_addon_factory_instance \
            || { echo "dist/verify/factory-symbol-missing: fedora" >&2; exit 1; }
          DESTDIR=/tmp/stage PREFIX=/usr bash packaging/uninstall.sh --no-sudo
          find /tmp/stage -printf '%P\t%s\n' | sort > /tmp/stage.after
          diff -u /tmp/stage.before /tmp/stage.after \
            || { echo "dist/verify/uninstall-not-reversible: fedora" >&2; exit 1; }

  arch:
    name: arch
    runs-on: ubuntu-24.04
    timeout-minutes: 45
    container:
      image: archlinux:base-devel@sha256:0000000000000000000000000000000000000000000000000000000000000000
    steps:
      - uses: actions/checkout@v4
      - name: Install the build dependencies
        run: |
          pacman --sync --refresh --noconfirm --needed \
            rust fcitx5 pkgconf findutils which tar gzip git curl
      - name: Install just and cargo-nextest
        run: |
          curl --proto '=https' --tlsv1.2 -sSf https://just.systems/install.sh \
            | bash -s -- --to /usr/local/bin
          curl -LsSf https://get.nexte.st/latest/linux | tar zxf - -C /usr/local/bin
      - name: Report the resolved versions
        run: |
          {
            echo "### Arch Linux"
            echo
            echo '```'
            rustc --version
            pkg-config --modversion Fcitx5Core
            echo '```'
          } >> "$GITHUB_STEP_SUMMARY"
      - name: Run the gate suite
        run: just ci
      - name: Build with the real C ABI linked
        run: just check-host
      - name: Install, assert and uninstall
        run: |
          set -eu
          mkdir -p /tmp/stage
          find /tmp/stage -printf '%P\t%s\n' | sort > /tmp/stage.before
          bash data/fetch.sh
          cargo run --quiet -p xtask -- dictc
          DESTDIR=/tmp/stage PREFIX=/usr bash packaging/install.sh --no-sudo --skip-build
          addondir="$(pkg-config --variable=addondir Fcitx5Core || true)"
          if [ -z "$addondir" ]; then
            addondir="$(pkg-config --variable=libdir Fcitx5Core)/fcitx5"
          fi
          test -f "/tmp/stage${addondir}/librspinyin.so" \
            || { echo "dist/verify/artifact-missing: ${addondir}/librspinyin.so" >&2; exit 1; }
          DESTDIR=/tmp/stage PREFIX=/usr bash packaging/uninstall.sh --no-sudo
          find /tmp/stage -printf '%P\t%s\n' | sort > /tmp/stage.after
          diff -u /tmp/stage.before /tmp/stage.after \
            || { echo "dist/verify/uninstall-not-reversible: arch" >&2; exit 1; }
```

  > 上面的 `image:` 行使用全零 digest 作为**必须替换的占位**：实施时用 `docker inspect --format='{{index .RepoDigests 0}}' fedora:41` 取得真实 digest 填入。这是本文件中唯一允许的占位符，因为它是一个**运行时才知道的值**，无法预先写出；其余任何位置不得出现占位符。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 取得 `fedora:41` 与 `archlinux:base-devel` 的真实 digest，替换上面两处全零值。
  2. 本地用 `podman run` 跑通 Fedora 容器内的 `just ci`，记录 Arch 上因滚动更新导致的首次失败（预期会发生，这是该切片的用途）。
  3. 把 `matrix.yml` 加入 CI；`schedule` 触发是**必须保留**的——滚动发行版的漂移只会在时间维度上暴露。
  4. 把两个发行版的 `pkg-config --variable=libdir Fcitx5Core` 实测值记入 `xtask/src/install/layout.rs` 的模块文档（该文档目前记录了三种路径的预期，用实测值确认）。

- **验收标准 (DoD)**：
  - [ ] Fedora 与 Arch 两个作业在 CI 中稳定通过（连续 5 次 PR 无 flake）；
  - [ ] 两个作业都执行了完整的 `just ci`（不是只编译）；
  - [ ] 两个作业都完成了 install → 断言 → uninstall → `diff` 可逆性闭环；
  - [ ] 两个发行版的 addon 目录实测值与 `xtask/src/install/layout.rs` 的文档一致；
  - [ ] `schedule` 触发已启用，且首次定时运行的结论被记录。

---

### 任务 ID：BUILD-P1.03.01 Debian / Ubuntu 打包

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-10`、`L-01`、`L-02`
  - 优先级与复杂度：`P1` | 高 | 预估工时: 3.0 人天
  - 前置依赖：`BUILD-P0.03.01`、`BUILD-P0.03.03`、`BUILD-P0.03.04`
  - 关键路径：CP: 否（时差 2.0 人天）
  - 并行通道：Track B（签名·打包·描述符）
  - 代码落地锚点 (Code Anchor)：`packaging/debian/control`、`packaging/debian/rules`、`packaging/debian/changelog`、`packaging/debian/copyright`、`packaging/debian/source/format`
  - 当前状态：`[ ] 待开始`
- **目标与核心交付物**：
  - 核心改进指标：产出可安装的 `.deb`（`amd64` 与 `arm64`），安装后 `fcitx5-configtool` 中出现 rspinyin 且候选框自绘生效；`apt remove rspinyin` 后系统回到安装前状态。
  - 目标产物格式：`rspinyin_<version>_<arch>.deb`（单一二进制包，不分 `-dev`/`-dbgsym` 之外的子包）。
- **工程实现方案与配置文件/脚本全文**：

  包结构决策：**单包**。rspinyin 没有供第三方链接的库（`ime-ui` 的公共 API 被 `OB-4` 约束为不得导出 Slint 类型，`ime-fcitx5` 只导出 C ABI 供 Fcitx5 `dlsym`），因此不需要 `-dev` 子包。两个 `.so` 都是插件实现细节，不进 `/usr/lib`。

  `packaging/debian/control`（全文）：

```
Source: rspinyin
Section: utils
Priority: optional
Maintainer: rspinyin contributors <rspinyin@localhost>
Build-Depends:
 cargo,
 debhelper-compat (= 13),
 libfcitx5config-dev,
 libfcitx5core-dev,
 libfcitx5utils-dev,
 pkgconf,
 rustc (>= 1.85),
Standards-Version: 4.7.0
Rules-Requires-Root: no
Homepage: https://github.com/rspinyin/rspinyin
Vcs-Git: https://github.com/rspinyin/rspinyin.git
Vcs-Browser: https://github.com/rspinyin/rspinyin

Package: rspinyin
Architecture: any
Depends:
 ${misc:Depends},
 ${shlibs:Depends},
 fcitx5 (>= 5.1.0),
Description: Offline-first Chinese pinyin input method for Fcitx5
 rspinyin is an in-process Fcitx5 addon that draws its own candidate window
 instead of delegating to ClassicUI. It decodes full-pinyin input with a
 k-best Viterbi search over a compiled FST dictionary, learns from the user's
 own word frequency, and never opens a network socket: there is no cloud input,
 no dictionary update channel and no telemetry.
 .
 The candidate window is rendered by a software rasteriser into a shared-memory
 buffer, so it needs no GPU and no display-server-specific toolkit.
```

  > `fcitx5 (>= 5.1.0)` 与 `BUILD-P0.03.04` 确定的 addon 描述符门槛必须一致。若该任务最终选择路径 (b)（保留 `core:5.1.7`），这里的依赖要同步改为 `fcitx5 (>= 5.1.7)`。

  `packaging/debian/rules`（全文）：

```make
#!/usr/bin/make -f
#
# Debian packaging rules for rspinyin.
#
# This drives cargo directly rather than using the `cargo` debhelper buildsystem
# from dh-cargo. dh-cargo is built around Debian's vendored-crates model: it
# expects a `debian/cargo-checksum.json` and a `vendor/` tree produced by
# `cargo vendor`, and it builds library crates for other packages to link
# against. rspinyin ships a cdylib that nothing links against, and its
# Cargo.lock is committed, so the vendoring step buys nothing here and would
# have to be re-run on every dependency bump.
#
# For an upload to Debian main the vendored path is the compliant one and this
# file must be replaced by `dh $@ --buildsystem=cargo` plus the vendor tree.
# For a PPA, a self-hosted repository or an internal build, this is the
# shorter correct answer.
#
# The dictionary is compiled at build time and shipped in the package. It is
# NOT compiled on the user's machine: that would require the raw upstream TSVs
# and therefore network access during install.

export DH_VERBOSE = 1
export SOURCE_DATE_EPOCH ?= 1790899200
# Reproducible builds: rewrite the absolute build path to a stable token so two
# builds of the same source tree produce identical debug info in the C++ glue.
export CXXFLAGS = -ffile-prefix-map=$(CURDIR)=. -fdebug-prefix-map=$(CURDIR)=.

%:
	dh $@

override_dh_auto_configure:
	# Nothing to configure: there is no autotools/cmake/meson build system, and
	# cargo reads its configuration from Cargo.toml and .cargo/config.toml.

override_dh_auto_build:
	cargo build --release --locked -p ime-fcitx5 --features fcitx5-host
	cargo run --release --locked -p xtask -- dictc

override_dh_auto_test:
	# The test suite needs cargo-nextest, which is not in Debian's archive.
	# Tests run in CI on every commit (.github/workflows/ci.yml); repeating them
	# here would only re-assert what the source archive was built from.
	:

override_dh_auto_install:
	# `xtask install` resolves the destination directories from the *target*
	# system's pkg-config, so this works unchanged on Debian (lib/x86_64-linux-gnu)
	# and on any derivative that uses a different triplet.
	DESTDIR=$(CURDIR)/debian/rspinyin PREFIX=/usr \
	  bash packaging/install.sh --no-sudo --skip-build

override_dh_strip:
	# The addon libraries are stripped by `xtask install` with
	# `strip --strip-unneeded`, and the symbol table is re-read afterwards to
	# confirm `fcitx_addon_factory_instance` survived. Running dh_strip on top
	# would re-strip them, and dh_strip's default is `--strip-all`, which is
	# exactly the operation that removes that symbol and produces a library
	# Fcitx5 loads but finds no addon in.
	#
	# There is no ELF in this package that dh_strip should touch: the two .so
	# files are addon plugins, not libraries anything links against.
	:

override_dh_makeshlibs:
	# Same reason: these are dlopen'd plugins with a private C ABI, not shared
	# libraries, so they must not contribute a shlibs entry.
	:
```

  `packaging/debian/source/format`（全文）：

```
3.0 (quilt)
```

  `packaging/debian/changelog`（全文，`<version>` 与 `Cargo.toml` 的 workspace 版本必须一致，由 `xtask check-versions` 的扩展版校验）：

```
rspinyin (0.1.0-1) unstable; urgency=medium

  * Initial packaging.

 -- rspinyin contributors <rspinyin@localhost>  Tue, 29 Sep 2026 00:00:00 +0000
```

  `packaging/debian/copyright`（全文，格式遵循 DEP-5）：

```
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: rspinyin
Source: https://github.com/rspinyin/rspinyin

Files: *
Copyright: rspinyin contributors
License: MIT or Apache-2.0
 This project is dual-licensed under the MIT License and the Apache License,
 Version 2.0. You may use it under either, at your option.
 .
 On Debian systems the full texts of these licences are available at
 /usr/share/common-licenses/MIT and /usr/share/common-licenses/Apache-2.0.

Files: base.dict
Copyright: rspinyin contributors
License: MIT or Apache-2.0
 The compiled dictionary is derived exclusively from sources carrying
 permissive licences, registered with their SHA256 digests in
 data/sources.toml. The allowlist is enforced by
 scripts/check-dict-sources.sh.
Comment:
 The dictionary is a derived database, not a redistribution of any single
 upstream file: it is compiled from mozillazg/pinyin-data (MIT),
 Unicode Unihan (Unicode-3.0) and fxsjy/jieba (MIT). None of those licences
 impose a copyleft obligation on the derived database.

Files: LICENSES/LicenseRef-Slint-Royalty-free-2.0.md
Copyright: SixtyFPS GmbH
License: LicenseRef-Slint-Royalty-free-2.0
 Slint is used under the Slint Royalty-free Desktop, Mobile, and Web
 Applications License, Version 2.0. Attribution: this software uses Slint --
 https://slint.dev
 .
 The licence does NOT cover embedded systems (appliance displays, point of
 sale terminals, in-car instrumentation). Such deployments require a
 GPL-3.0 or commercial licence obtained separately.
```

  `packaging/debian/rspinyin.install`（全文，把 `xtask install` 已经放好的文件登记给 dh_install）：

```
debian/rspinyin/usr/lib/*/fcitx5/*.so usr/lib
debian/rspinyin/usr/share/fcitx5/addon/*.conf usr/share/fcitx5/addon
debian/rspinyin/usr/share/fcitx5/inputmethod/*.conf usr/share/fcitx5/inputmethod
debian/rspinyin/usr/share/rspinyin/base.dict usr/share/rspinyin
debian/rspinyin/usr/share/icons/hicolor/48x48/apps/*.png usr/share/icons/hicolor/48x48/apps
debian/rspinyin/usr/share/icons/hicolor/scalable/apps/*.svg usr/share/icons/hicolor/scalable/apps
debian/rspinyin/usr/share/doc/rspinyin/NOTICE usr/share/doc/rspinyin
```

  > 注意：`debian/rules` 的 `override_dh_auto_install` 已经把文件装进 `debian/rspinyin/`，若再列 `.install` 文件，dh_install 会试图二次安装。**二选一**：保留 `override_dh_auto_install` 则删除 `rspinyin.install`；使用 `.install` 则 `dh_auto_install` 改为 `:`。本卡采用**保留 `override_dh_auto_install` 并删除 `rspinyin.install`** 的方案，因为 `xtask install` 同时负责剥离与符号复检，而 `.install` 只做文件搬运。上表的 `.install` 内容保留在此仅作为**替代方案的登记**。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在 Ubuntu 24.04 容器内执行 `dpkg-buildpackage -us -uc -b`，确认产出 `.deb`。
  2. `lintian --pedantic` 跑一遍，处理所有 `error` 与 `warning`（`pedantic`/`info` 可豁免但需在提交信息中记录理由）。
  3. 在干净的 Ubuntu 24.04 与 22.04 容器内 `apt install ./rspinyin_*.deb`，确认依赖自动带入 `fcitx5`，且 `fcitx5-configtool` 中出现 rspinyin。
  4. `apt remove rspinyin` 后确认无残留（`dpkg -L rspinyin` 为空、`/usr/lib/*/fcitx5/librspinyin*.so` 不存在）。
  5. 把 `.deb` 纳入 `BUILD-P1.04.01` 的签名产物清单与 `BUILD-P2.05.01` 的发布流水线。

- **验收标准 (DoD)**：
  - [ ] `dpkg-buildpackage -b` 在 Ubuntu 24.04 容器内成功产出 `rspinyin_0.1.0-1_amd64.deb`；
  - [ ] `lintian` 无 `error`（`warning` 若有，逐条记录豁免理由）；
  - [ ] 在 Ubuntu 24.04 与 22.04 上均可 `apt install` 并生效（22.04 的结论取决于 `BUILD-P0.03.04` 的定案）；
  - [ ] `apt remove` 后无残留文件；
  - [ ] 包内 `.so` 已剥离且导出 `fcitx_addon_factory_instance`（`dpkg-deb -x` 后 `nm -D` 复检）；
  - [ ] `debian/copyright` 覆盖 `base.dict` 与 Slint 许可原文两处，且 `OB-3` 的嵌入式排除声明在案。

---

### 任务 ID：BUILD-P1.03.02 Fedora 打包

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-10`、`L-03`
  - 优先级与复杂度：`P1` | 中 | 预估工时: 2.5 人天
  - 前置依赖：`BUILD-P1.01.01`、`BUILD-P0.03.01`、`BUILD-P0.03.03`、`BUILD-P0.03.04`
  - 关键路径：**CP: 是**
  - 并行通道：Track B（签名·打包·描述符）
  - 代码落地锚点 (Code Anchor)：`packaging/rpm/rspinyin.spec`
  - 当前状态：`[ ] 待开始`
- **目标与核心交付物**：
  - 核心改进指标：`rpmbuild` 在 Fedora 41 上产出可安装的 `.rpm`；`dnf install` 后 addon 落到 `/usr/lib64/fcitx5/`（Fedora 的路径，与 Debian 不同）。
  - 目标产物格式：`rspinyin-<version>-1.fc41.<arch>.rpm`。
- **工程实现方案与配置文件/脚本全文**：

  `packaging/rpm/rspinyin.spec`（全文）：

```spec
# RPM spec for rspinyin.
#
# Fedora installs Fcitx5 addons under %{_libdir}/fcitx5, which is /usr/lib64 on
# x86_64 and aarch64 and /usr/lib on 32-bit. The spec does not hardcode any of
# them: `xtask install` asks the target system's pkg-config, and %{_libdir} is
# used only to tell rpm which files to expect in the manifest.

%global debug_package %{nil}

Name:           rspinyin
Version:        0.1.0
Release:        1%{?dist}
Summary:        Offline-first Chinese pinyin input method for Fcitx5

License:        MIT AND Apache-2.0
URL:            https://github.com/rspinyin/rspinyin
Source0:        %{url}/archive/v%{version}/rspinyin-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  gcc-c++
BuildRequires:  fcitx5-devel
BuildRequires:  pkgconf-pkg-config
BuildRequires:  rust >= 1.85
Requires:       fcitx5 >= 5.1.0

# The plugin is an in-process addon: it links against the same libFcitx5Core the
# host process already loaded, so the automatic library dependency generator
# must record it. Without this the package installs and then fails to load.
AutoReqProv:    yes

%description
rspinyin is an in-process Fcitx5 addon that draws its own candidate window
instead of delegating to ClassicUI. It decodes full-pinyin input with a k-best
Viterbi search over a compiled FST dictionary, learns from the user's own word
frequency, and never opens a network socket: there is no cloud input, no
dictionary update channel and no telemetry.

The candidate window is rendered by a software rasteriser into a shared-memory
buffer, so it needs no GPU and no display-server-specific toolkit.

%prep
%setup -q -n rspinyin-%{version}

%build
# SOURCE_DATE_EPOCH is set by rpmbuild from the changelog date; propagating it
# keeps the C++ glue's debug info stable across rebuilds.
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-%{changelog_time}}
export CXXFLAGS="-ffile-prefix-map=$PWD=. -fdebug-prefix-map=$PWD=. ${CXXFLAGS:-}"
cargo build --release --locked -p ime-fcitx5 --features fcitx5-host
cargo run --release --locked -p xtask -- dictc

%install
rm -rf %{buildroot}
DESTDIR=%{buildroot} PREFIX=%{_prefix} \
  bash packaging/install.sh --no-sudo --skip-build

%check
# Tests run in CI on every commit and need cargo-nextest, which is not in
# Fedora's archive. What this package can assert cheaply is that the thing it is
# about to ship actually exports the addon factory symbol.
for so in %{buildroot}%{_libdir}/fcitx5/librspinyin.so \
          %{buildroot}%{_libdir}/fcitx5/librspinyin-ui.so; do
  nm -D --defined-only "$so" | grep -q fcitx_addon_factory_instance \
    || { echo "dist/verify/factory-symbol-missing: $so" >&2; exit 1; }
done
test -s %{buildroot}%{_datadir}/rspinyin/base.dict \
  || { echo "dist/verify/artifact-missing: base.dict" >&2; exit 1; }

%files
%license LICENSES/LicenseRef-Slint-Royalty-free-2.0.md
%doc docs/dev/NOTICE
%{_libdir}/fcitx5/librspinyin.so
%{_libdir}/fcitx5/librspinyin-ui.so
%{_datadir}/fcitx5/addon/rspinyin.conf
%{_datadir}/fcitx5/addon/rspinyin-ui.conf
%{_datadir}/fcitx5/inputmethod/rspinyin.conf
%{_datadir}/rspinyin/base.dict
%{_datadir}/icons/hicolor/48x48/apps/fcitx-rspinyin.png
%{_datadir}/icons/hicolor/scalable/apps/fcitx-rspinyin.svg
%{_metainfodir}/org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml

%changelog
* Tue Sep 29 2026 rspinyin contributors <rspinyin@localhost> - 0.1.0-1
- Initial package.
```

  > **两处必须确认的事项**（Fedora 的规则会变，且本条无法在不联网查规范的情况下断言）：
  > 1. `License: MIT AND Apache-2.0` — 项目自身是 `MIT OR Apache-2.0`（双许可、用户可选），而发布包**同时包含两份许可原文**。Fedora 的 SPDX 表达规则对"源码含两份许可"与"用户可二选一"的处理不同，实施时必须对照当前的 Fedora 许可规范确认应写 `AND` 还是 `OR`。
  > 2. `%{_metainfodir}` 一行依赖 `BUILD-P1.03.04` 的产物路径；该任务未完成前此行会因文件缺失而失败，属预期的前置依赖。

  `packaging/rpm/build.sh`（全文，本地与 CI 共用的构建入口）：

```bash
#!/usr/bin/env bash
#
# Build the RPM from a source archive.
#
# Run through `bash`; the executable bit is not required, matching the rest of
# packaging/. The source archive is produced by `xtask package` (or by
# `git archive`), not by this script: rpmbuild must build from a pristine tree
# so the spec's %setup finds exactly what the release shipped.

set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname -- "$(dirname -- "$script_dir")")"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
if [ -z "$version" ]; then
    echo "rpm/build.sh: could not read the workspace version from Cargo.toml" >&2
    exit 1
fi

topdir="${RPM_TOPDIR:-$root/target/rpmbuild}"
mkdir -p "$topdir"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}

archive="$topdir/SOURCES/rspinyin-${version}.tar.gz"
if [ ! -f "$archive" ]; then
    echo "rpm/build.sh: creating $archive"
    git -C "$root" archive --format=tar.gz \
        --prefix="rspinyin-${version}/" \
        --output="$archive" HEAD
fi

cp -- "$script_dir/rspinyin.spec" "$topdir/SPECS/"

rpmbuild --define "_topdir $topdir" \
         --define "_sourcedir $topdir/SOURCES" \
         -ba "$topdir/SPECS/rspinyin.spec"

echo "rpm/build.sh: artifacts under $topdir/RPMS and $topdir/SRPMS"
```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在 Fedora 41 容器内安装 `rpm-build rpmdevtools`，用 `packaging/rpm/build.sh` 跑通 `-ba`。
  2. 用 `rpmlint` 检查 spec 与产出的 `.rpm`，处理所有 `E:`（error）级问题。
  3. 在干净的 Fedora 41 容器内 `dnf install ./rspinyin-*.rpm`，确认 `fcitx5` 被自动带入、addon 落在 `/usr/lib64/fcitx5/`、`fcitx5-configtool` 中出现 rspinyin。
  4. `dnf remove rspinyin` 后确认 `/usr/lib64/fcitx5/librspinyin*.so` 已移除。
  5. 对照当前 Fedora 许可规范确认 `License:` 字段的 `AND`/`OR` 写法，并把结论记入本卡。

- **验收标准 (DoD)**：
  - [ ] `rpmbuild -ba` 在 Fedora 41 容器内成功，产出 `.rpm` 与 `.src.rpm`；
  - [ ] `rpmlint` 无 `E:` 级问题；
  - [ ] 在干净的 Fedora 41 上 `dnf install` 后 addon 生效，且落在 `/usr/lib64/fcitx5/`（与 Debian 的路径不同，这是该切片的核心验证点）；
  - [ ] `%check` 段的工厂符号断言真的执行且能失败（人为破坏后验证）；
  - [ ] `License:` 字段的写法已对照 Fedora 规范确认并记录依据。

---

### 任务 ID：BUILD-P1.03.03 Arch Linux / AUR 打包

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-10`、`L-04`
  - 优先级与复杂度：`P1` | 中 | 预估工时: 2.0 人天
  - 前置依赖：`BUILD-P1.01.01`、`BUILD-P0.03.01`、`BUILD-P0.03.03`、`BUILD-P0.03.04`
  - 关键路径：CP: 否
  - 并行通道：Track B（签名·打包·描述符）
  - 代码落地锚点 (Code Anchor)：`packaging/aur/PKGBUILD`、`packaging/aur/.SRCINFO`
  - 当前状态：`[ ] 待开始`
- **目标与核心交付物**：
  - 核心改进指标：`makepkg -si` 在 Arch 上产出可安装的包，addon 落到 `/usr/lib/fcitx5/`（Arch 的路径，三者中最短的一个）。
  - 目标产物格式：`rspinyin-<version>-1-<arch>.pkg.tar.zst`。
- **工程实现方案与配置文件/脚本全文**：

  `packaging/aur/PKGBUILD`（全文）：

```bash
# Maintainer: rspinyin contributors <rspinyin@localhost>
#
# Arch installs Fcitx5 addons under /usr/lib/fcitx5 -- no triplet, no lib64.
# That is the third of the three layouts `xtask install` derives from
# pkg-config, and the one a hardcoded path would get wrong most visibly.

pkgname=rspinyin
pkgver=0.1.0
pkgrel=1
pkgdesc="Offline-first Chinese pinyin input method for Fcitx5, with a self-drawn candidate window"
arch=('x86_64' 'aarch64')
url="https://github.com/rspinyin/rspinyin"
license=('MIT' 'Apache')
depends=('fcitx5>=5.1.0')
makedepends=('cargo' 'gcc' 'pkgconf' 'git')
provides=('fcitx5-rspinyin')
conflicts=('fcitx5-rspinyin')
options=('!lto' '!strip')
source=("$pkgname-$pkgver.tar.gz::$url/archive/v$pkgver.tar.gz")
sha256sums=('SKIP')

prepare() {
    cd "$pkgname-$pkgver"
    # The dictionary sources are not in the source archive: they are fetched
    # from upstream and pinned by digest in data/sources.toml. makepkg runs in
    # a network-enabled environment, so the fetch is done here rather than
    # requiring the packager to have run it beforehand.
    bash data/fetch.sh
}

build() {
    cd "$pkgname-$pkgver"
    export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(date +%s)}"
    export CXXFLAGS="-ffile-prefix-map=$srcdir/$pkgname-$pkgver=. ${CXXFLAGS:-}"
    cargo build --release --locked -p ime-fcitx5 --features fcitx5-host
    cargo run --release --locked -p xtask -- dictc
}

check() {
    cd "$pkgname-$pkgver"
    # `!strip` is set above: `xtask install` strips with --strip-unneeded and
    # then re-reads the dynamic symbol table to confirm the addon factory symbol
    # survived. makepkg's own stripping is `--strip-all` by default, which
    # removes exactly that symbol and produces a library Fcitx5 loads but finds
    # no addon in.
    #
    # Assert the same thing makepkg is being told not to break.
    for so in target/release/librspinyin.so target/release/librspinyin-ui.so; do
        nm -D --defined-only "$so" | grep -q fcitx_addon_factory_instance \
            || { echo "dist/verify/factory-symbol-missing: $so" >&2; return 1; }
    done
}

package() {
    cd "$pkgname-$pkgver"
    DESTDIR="$pkgdir" PREFIX=/usr bash packaging/install.sh --no-sudo --skip-build
}
```

  > `options=('!lto' '!strip')` 两项都不是可选的：`!strip` 是硬要求（理由写在 `check()` 的注释里）；`!lto` 关掉的是 **makepkg 的 LTO 包装**，与 `Cargo.toml` 的 `lto = "thin"` 无关——Arch 的 `makepkg` 默认会注入 `-flto` 到 `CFLAGS`，那会与 Rust 的链接期优化叠加并可能改变 C++ 胶水的 ABI 可见性。`BUILD-P0.02.01` 的符号复检能发现该问题，但在这里直接关掉更省事。
  >
  > `sha256sums=('SKIP')` **不可用于提交到 AUR**：AUR 要求真实校验和。实施时必须用 `updpkgsums` 填入，并在每次版本更新时重跑。

  `packaging/aur/.SRCINFO` 由 `makepkg --printsrcinfo > .SRCINFO` 生成，**不手工编辑**。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 在 Arch 容器内 `makepkg -si --noconfirm` 跑通。
  2. 用 `namcap PKGBUILD` 与 `namcap *.pkg.tar.zst` 检查，处理所有问题。
  3. 用 `updpkgsums` 填入真实校验和，重新生成 `.SRCINFO`。
  4. 确认 `pacman -R rspinyin` 后 `/usr/lib/fcitx5/librspinyin*.so` 被移除。
  5. 把 PKGBUILD 提交到 AUR（这一步需要 AUR 账号，属于发布动作，需用户显式确认后执行）。

- **验收标准 (DoD)**：
  - [ ] `makepkg -si` 在 Arch 容器内成功，`namcap` 无 error；
  - [ ] `sha256sums` 为真实值（非 `SKIP`），`.SRCINFO` 由 `makepkg --printsrcinfo` 生成；
  - [ ] addon 落在 `/usr/lib/fcitx5/`，`pacman -R` 后无残留；
  - [ ] `options` 中的 `!strip` 与 `!lto` 及其理由在 PKGBUILD 注释中说明；
  - [ ] 向 AUR 的提交经用户显式确认后才执行。

---

### 任务 ID：BUILD-P1.03.04 AppStream 元数据

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-11`
  - 优先级与复杂度：`P1` | 低 | 预估工时: 1.0 人天
  - 前置依赖：无
  - 关键路径：CP: 否
  - 并行通道：Track B（签名·打包·描述符）
  - 代码落地锚点 (Code Anchor)：`packaging/metainfo/org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml`、`xtask/src/install.rs`、`packaging/debian/rules`、`packaging/rpm/rspinyin.spec`、`packaging/aur/PKGBUILD`
  - 当前状态：`[ ] 待开始`
- **目标与核心交付物**：
  - 核心改进指标：`appstreamcli validate` 零错误；Fedora 打包规范要求满足；GNOME Software / KDE Discover 能显示 rspinyin 的图标与描述。
  - 目标产物格式：`org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml`，安装到 `%{_metainfodir}` / `<datadir>/metainfo/`。
- **工程实现方案与配置文件/脚本全文**：

  `packaging/metainfo/org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml`（全文）：

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!--
  AppStream metadata for the rspinyin Fcitx5 addon.

  The component type is `addon` and it extends the Fcitx5 desktop component:
  rspinyin has no launcher of its own, so listing it as a standalone desktop
  application would put a non-launchable entry in application menus.

  The `extends` value must match the component ID Fcitx5 itself publishes.
  Verify it against the installed package before changing anything here:
      appstreamcli search fcitx5
  A wrong ID makes the addon invisible to software centres without any error.
-->
<component type="addon">
  <id>org.fcitx.Fcitx5.Addon.rspinyin</id>

  <metadata_license>CC0-1.0</metadata_license>
  <project_license>MIT AND Apache-2.0</project_license>

  <name>Rust Pinyin</name>
  <summary>Offline-first Chinese pinyin input method</summary>

  <description>
    <p>
      rspinyin is a Chinese pinyin input method for Fcitx5 that draws its own
      candidate window instead of delegating to ClassicUI. The window has rounded
      corners, layered shadows, a translucent background where the compositor
      supports it, spring-physics transitions and automatic light/dark theming.
    </p>
    <p>
      Decoding is a k-best Viterbi search over a compiled FST dictionary, with the
      user's own word frequency folded into the ranking. The candidate window is
      rendered by a software rasteriser into a shared-memory buffer, so it needs
      no GPU and no display-server-specific toolkit.
    </p>
    <p>
      The input method never opens a network socket. There is no cloud input, no
      dictionary update channel and no telemetry; user frequency data stays in the
      user's own home directory.
    </p>
  </description>

  <launchable type="desktop-id">org.fcitx.Fcitx5.desktop</launchable>

  <url type="homepage">https://github.com/rspinyin/rspinyin</url>
  <url type="bugtracker">https://github.com/rspinyin/rspinyin/issues</url>

  <translation type="gettext">rspinyin</translation>

  <provides>
    <binary>librspinyin.so</binary>
    <binary>librspinyin-ui.so</binary>
  </provides>

  <extends>org.fcitx.Fcitx5</extends>

  <languages>
    <lang>zh_CN</lang>
  </languages>

  <releases>
    <release version="0.1.0" date="2026-09-29" type="stable">
      <description>
        <p>First release.</p>
      </description>
    </release>
  </releases>

  <content_rating type="oars-1.1"/>

  <keywords>
    <keyword>pinyin</keyword>
    <keyword>input method</keyword>
    <keyword>chinese</keyword>
    <keyword>ime</keyword>
  </keywords>
</component>
```

  载荷表追加（`xtask/src/install.rs` 的 `PAYLOADS`）：

```rust
    Payload {
        name: "org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml",
        artifact: &["packaging/metainfo/org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml"],
        destination: Destination::MetaInfo,
        optional: false,
        budget: None,
    },
```

  `Destination` 枚举追加变体（`xtask/src/install/layout.rs`）：

```rust
    /// AppStream metadata, under `<datadir>/metainfo`.
    MetaInfo,
```

  与 `Layout` 的对应字段：

```rust
    /// `<datadir>/metainfo`.
    pub metainfo_dir: PathBuf,
```

  CI 断言（加进 `BUILD-P0.05.01` 的 `quality` 作业之后，或独立成步）：

```yaml
      - name: Validate the AppStream metadata
        run: |
          sudo apt-get install --yes --no-install-recommends appstream
          appstreamcli validate --no-net \
            packaging/metainfo/org.fcitx.Fcitx5.Addon.rspinyin.metainfo.xml
```

  > `--no-net` 是必须的：`appstreamcli validate` 默认会联网检查 URL 可达性，而本项目的 CI 与产品承诺都是离线的。加 `--no-net` 后它只做结构与枚举校验。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 用 `appstreamcli search fcitx5` 确认 Fcitx5 自身发布的 component ID，据实修正 `<extends>` 与 `<launchable>`。
  2. `appstreamcli validate --no-net` 跑到零错误。
  3. 把载荷加入 `PAYLOADS`，`Destination` 与 `Layout` 各加一个变体/字段，并更新 `xtask/src/install/tests.rs`。
  4. 在三个发行版的打包定义中分别登记该文件的安装路径（deb 由 `xtask install` 自动处理；rpm 的 `%files` 已含 `%{_metainfodir}` 行；PKGBUILD 由 `xtask install` 处理）。

- **验收标准 (DoD)**：
  - [ ] `appstreamcli validate --no-net` 零错误；
  - [ ] `<extends>` 与 `<launchable>` 的 ID 经 `appstreamcli search fcitx5` 实测确认；
  - [ ] `xtask install` 把该文件装到 `<datadir>/metainfo/`，`xtask/src/install/tests.rs` 覆盖新变体；
  - [ ] 三个发行版的打包定义都能找到该文件（rpm 的 `%files` 不再因缺文件而失败）。

---

### 任务 ID：BUILD-P1.03.05 aarch64 包构建与安装验证

- **基本属性**：
  - 绑定来源编号：`L-05`、`L-06`、`L-07`
  - 优先级与复杂度：`P1` | 中 | 预估工时: 2.5 人天
  - 前置依赖：`BUILD-P1.03.01`、`BUILD-P1.03.02`、`BUILD-P1.03.03`
  - 关键路径：**CP: 是**
  - 并行通道：Track B（签名·打包·描述符）
  - 代码落地锚点 (Code Anchor)：`.github/workflows/matrix.yml`、`packaging/debian/control`、`packaging/rpm/rspinyin.spec`、`packaging/aur/PKGBUILD`
  - 当前状态：`[ ] 待开始`
- **目标与核心交付物**：
  - 核心改进指标：`arm64` 的 `.deb` / `.rpm` / PKGBUILD 三种包均能构建并安装；`docs/dev/features.md:117` 的 aarch64 判定从"待评估"变为"支持"或"不支持"。
  - 目标产物格式：`rspinyin_<version>_arm64.deb`、`rspinyin-<version>-1.fc41.aarch64.rpm`、`rspinyin-<version>-1-aarch64.pkg.tar.zst`。
- **工程实现方案与配置文件/脚本全文**：

  `.github/workflows/matrix.yml` 追加（利用 `ubuntu-24.04-arm` 原生 runner，理由同 `BUILD-P0.01.02`）：

```yaml
  arm64-packages:
    name: arm64-packages
    runs-on: ubuntu-24.04-arm
    timeout-minutes: 60
    strategy:
      fail-fast: false
      matrix:
        include:
          - distro: ubuntu-24.04
            image: ubuntu:24.04@sha256:0000000000000000000000000000000000000000000000000000000000000000
            kind: deb
          - distro: fedora-41
            image: fedora:41@sha256:0000000000000000000000000000000000000000000000000000000000000000
            kind: rpm
          - distro: arch
            image: archlinux:base-devel@sha256:0000000000000000000000000000000000000000000000000000000000000000
            kind: pkg
    container:
      image: ${{ matrix.image }}
    steps:
      - uses: actions/checkout@v4
      - name: Report the architecture
        run: |
          {
            echo "### ${{ matrix.distro }} / $(uname -m)"
            echo
            echo '```'
            uname -m
            echo '```'
          } >> "$GITHUB_STEP_SUMMARY"
      - name: Build the ${{ matrix.kind }} package
        run: |
          case "${{ matrix.kind }}" in
            deb) bash packaging/ci/build-deb.sh ;;
            rpm) bash packaging/ci/build-rpm.sh ;;
            pkg) bash packaging/ci/build-pkg.sh ;;
          esac
      - name: Install the package and assert the addon loads
        run: bash packaging/ci/verify-install.sh "${{ matrix.kind }}" arm64
```

  > `packaging/ci/build-*.sh` 与 `verify-install.sh` 是 `BUILD-P0.05.02` 的安装验证逻辑按包格式参数化后的产物；本卡负责把它们抽出来共用，而不是再写一遍。三个 digest 与 `BUILD-P1.01.01` 的同名占位一样，必须在实施时用 `docker inspect` 的实测值替换。

  **aarch64 判定的产出**：本卡完成后，把结论写回 `docs/dev/features.md` 两处：

1. `0.5.1` 的架构行：`x86_64 优先；aarch64 待评估` → `x86_64 与 aarch64（Ubuntu 24.04 / Fedora 41 / Arch 三发行版实测）`。
2. `0.5.2` 的能力矩阵：四列平台档位的架构维度随之确认（矩阵本身按显示服务器分档，架构是正交维度，需在表头或脚注中登记）。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 确认仓库可用的 `ubuntu-24.04-arm` runner 配额；若不可用，退化为 QEMU + `--platform linux/arm64` 容器（代价：构建时间约 5~10 倍，需相应放宽 `timeout-minutes`）。
  2. 抽出 `packaging/ci/*.sh` 三个构建脚本与一个验证脚本，让三个包格式共用验证逻辑。
  3. 三个矩阵项全绿后，把 aarch64 结论回写 `features.md` 0.5.1 与 0.5.2。
  4. 若某项无法通过（例如某发行版的 arm64 仓库缺 fcitx5 开发包），**如实登记为"不支持"并写明原因**，不得用"待评估"含糊过去。

- **验收标准 (DoD)**：
  - [ ] 三种包格式的 arm64 产物均构建成功并记录 `sha256`；
  - [ ] 每种包在对应的 arm64 容器内安装后，`nm -D` 复检工厂符号通过；
  - [ ] 每个作业的 `uname -m` 输出确为 `aarch64`（防止 runner 静默回落到 x86_64）；
  - [ ] `docs/dev/features.md` 0.5.1 与 0.5.2 的 aarch64 判定已回写为实测结论；
  - [ ] 未通过的切片如实标注"不支持"及原因，无"待评估"残留。

---

### 任务 ID：BUILD-P1.04.01 产物签名与 SHA256SUMS

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-15`
  - 优先级与复杂度：`P1` | 中 | 预估工时: 2.0 人天
  - 前置依赖：`BUILD-P0.02.01`
  - 关键路径：**CP: 是**
  - 并行通道：Track B（签名·打包·描述符）
  - 代码落地锚点 (Code Anchor)：`xtask/src/verify.rs`、`xtask/src/main.rs`、`.github/workflows/release.yml`、`packaging/keys/README.md`
  - 当前状态：`[ ] 待开始`
- **目标与核心交付物**：
  - 核心改进指标：每次发布产出 `SHA256SUMS` 与 `SHA256SUMS.asc`；用户侧一条命令完成主文档 3.3.2 的 S1~S5 全部校验。
  - 目标产物格式：`SHA256SUMS`、`SHA256SUMS.asc`、`rspinyin-release.json`。
- **工程实现方案与配置文件/脚本全文**：

  实现主文档 3.3.2 定义的状态机。**关键点：`xtask verify` 是只读的**——它不写系统状态、不替换任何已安装文件。理由已在 3.3.2 写明：一个能自我替换的插件与 `ASM-04` 冲突。

  `xtask/src/verify.rs`（新增）：

```rust
//! Verify a release archive against its manifest and signature.
//!
//! Responsibility: implement the check state machine a user runs before
//! installing a downloaded archive -- parse the manifest, check every artifact
//! digest and size, verify the detached signature, and read each shared
//! library's dynamic symbol table to confirm the addon factory symbol is
//! present.
//!
//! # What this deliberately does not do
//!
//! It does not install, replace or remove anything, and it does not look for a
//! newer release. rspinyin has no update channel by design: upgrades arrive
//! through the distribution's package manager, and a component that could
//! rewrite its own installation would be a network-reachable code path in a
//! process that is contractually forbidden from opening a socket. Verification
//! is a read-only operation whose only output is a verdict and an exit code.
//!
//! # Error codes
//!
//! Every failure carries one of the `dist/*` codes from the delivery contract
//! in `docs/dev/opt-deploy.md` section 3.3.2. The codes are matched by
//! diagnostics and by the acceptance tests, so they must not be reworded.

use std::path::Path;

use anyhow::Result;

/// The manifest schema version this build understands.
pub const MANIFEST_VERSION: u32 = 1;

/// Runs the verification state machine over `artifacts_dir`.
///
/// # Errors
///
/// Returns an error carrying a `dist/manifest/*` or `dist/verify/*` code when
/// the manifest is unreadable or malformed, when an artifact is missing, when a
/// digest or size disagrees with the manifest, when the signature does not
/// verify, when a shared library is missing a symbol the manifest lists, or
/// when the dictionary fails its own format check.
pub fn run(manifest: &Path, artifacts_dir: &Path) -> Result<()> {
    // S1 parse -> S2 digest/size -> S3 signature -> S4 symbols and dictionary.
    // Implementation shares crate::install::elf for the symbol half and
    // ime-dict's reader for the dictionary half; neither is re-implemented here.
    todo!()
}
```

  > 同 `BUILD-P0.02.01`：`todo!()` 不得进入主线，函数体必须在同一张卡内实现完毕，DoD 含 `grep -rn 'todo!' xtask/` 无输出。

  签名与校验的完整命令链（写进 `packaging/keys/README.md` 供维护者与用户共用）：

```bash
# ---- 维护者：生成（一次性） ----
gpg --full-generate-key                       # 选 RSA 4096 或 Ed25519，有效期按项目策略
gpg --armor --export <KEY_ID> > packaging/keys/rspinyin-signing-key.asc
# 私钥导出后存入 CI secret，本地副本删除；绝不提交私钥（AGENTS.md 禁止项 14）

# ---- 发布流水线：签名 ----
cd dist
sha256sum ./*.tar.gz ./*.deb ./*.rpm ./*.pkg.tar.zst > SHA256SUMS
gpg --batch --yes --local-user "$GPG_KEY_ID" \
    --detach-sign --armor --output SHA256SUMS.asc SHA256SUMS

# ---- 用户：校验 ----
gpg --import packaging/keys/rspinyin-signing-key.asc
gpg --verify SHA256SUMS.asc SHA256SUMS
sha256sum --check SHA256SUMS
cargo run --quiet -p xtask -- verify \
    --manifest rspinyin-release.json \
    --artifacts .
```

  发布流水线中的签名段（`.github/workflows/release.yml` 的片段，完整流水线由 `BUILD-P2.05.01` 交付）：

```yaml
      - name: Import the signing key
        env:
          GPG_PRIVATE_KEY: ${{ secrets.RSPINYIN_GPG_PRIVATE_KEY }}
        run: |
          set -eu
          if [ -z "${GPG_PRIVATE_KEY:-}" ]; then
            echo "release: RSPINYIN_GPG_PRIVATE_KEY is not set" >&2
            exit 1
          fi
          # The key is passed through the environment, never written to a file
          # in the repository and never echoed. `--batch --pinentry-mode loopback`
          # keeps gpg from trying to open a tty on a runner.
          printf '%s' "$GPG_PRIVATE_KEY" \
            | gpg --batch --import --pinentry-mode loopback
          gpg --list-secret-keys --with-colons | grep -q '^sec:' \
            || { echo "release: the imported key has no secret half" >&2; exit 1; }
      - name: Sign the checksums
        env:
          GPG_KEY_ID: ${{ secrets.RSPINYIN_GPG_KEY_ID }}
        run: |
          cd dist
          sha256sum ./*.tar.gz ./*.deb ./*.rpm ./*.pkg.tar.zst > SHA256SUMS
          gpg --batch --yes --local-user "$GPG_KEY_ID" \
              --detach-sign --armor --output SHA256SUMS.asc SHA256SUMS
          gpg --verify SHA256SUMS.asc SHA256SUMS
      - name: Remove the private key
        if: always()
        run: gpg --batch --yes --delete-secret-keys --pinentry-mode loopback "${{ secrets.RSPINYIN_GPG_KEY_ID }}" || true
```

  > `--no-network` 说明：GPG 的密钥服务器查询（`--recv-keys`）**不在此流水线中**。用户的公钥导入走 `packaging/keys/rspinyin-signing-key.asc` 这个随发布包分发的文件，不依赖任何 keyserver。这与 `ASM-04` 一致，也避免了 keyserver 的可用性成为校验链的单点。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 生成签名密钥（**用户操作**），把公钥提交到 `packaging/keys/`，私钥存入 CI secret。这一步必须由用户执行并确认，不得由 agent 代生成。
  2. 实现 `xtask verify`，逐条覆盖 3.3.2 的九个错误码，并为每个码写一条失败路径测试。
  3. 在 `xtask/src/main.rs` 加 `Verify` 子命令（**主 agent 负责**）。
  4. 把签名段加入发布流水线，用一次预发布（`-rc` 版本）验证整条链。
  5. 编写 `packaging/keys/README.md`，把上面的命令链与"私钥绝不入仓"的规则写清。

- **验收标准 (DoD)**：
  - [ ] `xtask verify` 对合法发布返回 0，对九种破坏场景分别返回对应错误码（每种一个测试用例）；
  - [ ] `xtask verify` **不写任何文件**（在只读挂载的目录上运行仍能完成校验，作为该性质的断言）；
  - [ ] `SHA256SUMS.asc` 能被 `gpg --verify` 通过，且公钥通过 `packaging/keys/rspinyin-signing-key.asc` 导入（不访问 keyserver）；
  - [ ] CI 中私钥只经环境变量传递，日志中不出现私钥内容（用 `grep` 断言日志）；
  - [ ] 流水线结束后私钥已从 runner 的钥匙环删除（`if: always()` 步骤）。

---

### 任务 ID：BUILD-P1.06.01 许可与归属文件落地（LICENSE / README / OB-1）

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-12`、`BUILD-DEF-13`
  - 优先级与复杂度：`P1` | 中 | 预估工时: 1.5 人天
  - 前置依赖：无
  - 关键路径：**CP: 是**
  - 并行通道：Track C（CI/CD·发布）
  - 代码落地锚点 (Code Anchor)：`LICENSE-APACHE`、`LICENSE-MIT`、`README.md`、`README.zh.md`、`docs/dev/features.md`（`OB-1` 行）
  - 当前状态：`[ ] 待开始`
- **目标与核心交付物**：
  - 核心改进指标：`Cargo.toml:12` 声明的 `MIT OR Apache-2.0` 有对应文件；`OB-1` 的 Slint 归属展示在 `README.md` / `README.zh.md` 显著位置落地（**发布阻塞项**，截止 W4）。
  - 目标产物格式：四个文件：`LICENSE-APACHE`、`LICENSE-MIT`、`README.md`、`README.zh.md`。
- **工程实现方案与配置文件/脚本全文**：

  这是纯文档交付，但**不是可选项**：`OB-1` 是 Slint Royalty-free 2.0 的义务，`docs/dev/features.md:4321` 把它列为 Phase 1 出口准则之一；`docs/dev/NOTICE:9` 又声明两个 LICENSE 文件由 `TASK-2.06.03` 落地——**两者在本卡一并完成，不留到 Phase 2**，因为发布包必须随附许可证，而 `BUILD-P1.03.01` 的 `debian/copyright` 与 `BUILD-P1.03.02` 的 `%license` 行都引用它们。

  `README.md` 的归属徽章段（**必须在首屏可见，不得折叠**）：

```markdown
# rspinyin

Offline-first Chinese pinyin input method for Linux, delivered as an in-process
Fcitx5 addon with a self-drawn candidate window.

<!--
  OB-1: the Slint Royalty-free 2.0 licence requires attribution. rspinyin has no
  about dialog and no splash screen -- it is a background system component with no
  window of its own -- so the "public web page" route is the only viable one.
  This badge is a licence obligation, not decoration. Do not move it below the
  fold and do not remove it.
-->
[![Built with Slint](https://img.shields.io/badge/built%20with-Slint-2F80ED?logo=slint)](https://slint.dev)

This software uses [Slint](https://slint.dev) under the
[Slint Royalty-free Desktop, Mobile, and Web Applications License, Version 2.0](https://slint.dev/royalty-free).
```

  `README.zh.md` 的对应段：

```markdown
[![使用 Slint 构建](https://img.shields.io/badge/built%20with-Slint-2F80ED?logo=slint)](https://slint.dev)

本程序使用 [Slint](https://slint.dev) 构建，依据
[Slint Royalty-free Desktop, Mobile, and Web Applications License, Version 2.0](https://slint.dev/royalty-free) 使用。
```

  `LICENSE-APACHE` 与 `LICENSE-MIT` 使用 SPDX 官方原文，**逐字不改**（Apache-2.0 的 `APPENDIX` 段需要按项目实际情况填写版权行）。

  同步修正 `docs/dev/features.md` 两处：
- `:4321` 的 `OB-1` 行"承担任务"列由 `TASK-1.06.03、TASK-2.03.03` 改为 `TASK-1.06.03、BUILD-P1.06.01、TASK-2.03.03`；
- `docs/dev/NOTICE:9` 的"由 `TASK-2.06.03` 落地"改为"由 `BUILD-P1.06.01` 落地"（该文件是手工维护段，不在生成块内）。

  CI 断言（加进 `BUILD-P0.05.01` 的 `quality` 作业）：

```yaml
      - name: Assert the licence and attribution files exist
        run: |
          set -eu
          for f in LICENSE-APACHE LICENSE-MIT README.md README.zh.md; do
            test -f "$f" || { echo "dist/verify/artifact-missing: $f" >&2; exit 1; }
          done
          # OB-1: the attribution must be present in both READMEs. The check is on
          # the link, not the badge image: a badge URL can rot while the licence
          # obligation is about the attribution being visible and reachable.
          for f in README.md README.zh.md; do
            grep -q 'slint.dev' "$f" \
              || { echo "dist/verify/missing-attribution: $f has no Slint attribution" >&2; exit 1; }
          done
```

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 落地两个 LICENSE 文件（SPDX 原文，填好 Apache-2.0 的版权行）。
  2. 写 `README.md` / `README.zh.md`，归属徽章置于首屏；正文覆盖安装（三种包格式 + 源码）、快捷键、诊断与隐私说明。
  3. 同步修正 `features.md` 与 `NOTICE` 的任务归属。
  4. 加 CI 断言；把 `OB-1` 的达成证据（README 的 URL 与 commit）记入 `docs/dev/licenses.md` 的 `OB-1` 行。

- **验收标准 (DoD)**：
  - [ ] `LICENSE-APACHE` 与 `LICENSE-MIT` 存在且为 SPDX 原文；
  - [ ] 两个 README 的 Slint 归属徽章在首屏可见、链接可达 `https://slint.dev`；
  - [ ] CI 断言两个文件存在且含 `slint.dev`，破坏后能失败；
  - [ ] `docs/dev/features.md` 的 `OB-1` 行与 `docs/dev/NOTICE` 的任务归属已同步；
  - [ ] `docs/dev/licenses.md` 的 `OB-1` 复核结论已更新为"已达成"并附证据。

---

### 任务 ID：BUILD-P1.06.02 `NOTICE` 的 `LICENSES/` 路径与守卫断言修复

- **基本属性**：
  - 绑定来源编号：`BUILD-DEF-14`
  - 优先级与复杂度：`P1` | 中 | 预估工时: 1.0 人天
  - 前置依赖：`BUILD-P1.06.01`
  - 关键路径：CP: 否
  - 并行通道：Track C（CI/CD·发布）
  - 代码落地锚点 (Code Anchor)：`LICENSES/`、`docs/dev/NOTICE`、`docs/dev/licenses.md`、`scripts/gen-licenses.sh`
  - 当前状态：`[ ] 待开始`
- **目标与核心交付物**：
  - 核心改进指标：`NOTICE` 引用的许可原文路径在本仓库**真实存在**；`gen-licenses.sh` 的"未改动"断言不再空转。
  - 目标产物格式：`LICENSES/LicenseRef-Slint-Royalty-free-2.0.md` 等许可原文文件进入版本控制。
- **工程实现方案与配置文件/脚本全文**：

  问题链条（实测）：`docs/dev/NOTICE` 声明"本文件随发布产物提供"，其生成块引用 `LICENSES/LicenseRef-Slint-Royalty-free-2.0.md` 并记录其 SHA256；但仓库内**没有 `LICENSES/` 目录**。`scripts/gen-licenses.sh:395` 把该路径解析为 `Path(package["manifest_path"]).parent / "LICENSES"`，即 **cargo registry 里 Slint crate 自己的目录**；`:453` 的守卫执行 `git status --porcelain -- *LICENSES*` 并断言为空——在没有 `LICENSES/` 被跟踪的情况下，该断言**恒真**，因此这个悬空引用不可能被发现。

  修复分两步：

  **第一步：把许可原文纳入版本控制。** 新增 `LICENSES/` 目录，把发布产物必须随附的许可原文复制进来：

```bash
# 从 Slint 发行包取出许可原文，逐字不改地放入 LICENSES/。
# 路径由 cargo metadata 定位，避免依赖 registry 的目录布局。
slint_dir="$(cargo metadata --format-version 1 --no-deps \
  | python3 -c 'import json,sys,os; print(os.path.dirname(json.load(sys.stdin)["packages"][0]["manifest_path"]))')"
# 实际取的是依赖包而非工作区包，因此改用 --filter-platform 的完整 metadata：
cargo metadata --format-version 1 \
  | python3 -c '
import json, sys, os
packages = json.load(sys.stdin)["packages"]
slint = next(p for p in packages if p["name"] == "slint")
print(os.path.join(os.path.dirname(slint["manifest_path"]), "LICENSES"))
'
```

  取到目录后：

```bash
mkdir -p LICENSES
cp -- "<slint-package>/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md" LICENSES/
# 若 Slint 的 LICENSES/ 还含 GPL-3.0 与商业许可的原文（Royalty-free 授权引用它们），
# 一并复制：OB-3 的"嵌入式需另取 GPL-3.0 或商业许可"声明引用了它们。
cp -- "<slint-package>/LICENSES/GPL-3.0-only.txt" LICENSES/ 2>/dev/null || true
sha256sum LICENSES/* | tee LICENSES/SHA256SUMS
```

  **第二步：让守卫断言真的能失败。** `scripts/gen-licenses.sh` 的 `check_licenses_untouched()` 改为"目录必须存在且非空，且其中的文件必须与 `docs/dev/licenses.md` 登记的 SHA256 一致"：

```python
def check_licenses_untouched(root, recorded):
    """Assert the licence texts under LICENSES/ are present and unmodified.

    The previous form of this check ran `git status --porcelain -- *LICENSES*`
    and asserted the output was empty. That is vacuously true when no LICENSES/
    directory is tracked at all -- which is exactly the state the repository was
    in, so the check could not report the dangling reference docs/dev/NOTICE
    carried. Asserting presence first is what turns the check from a formality
    into a gate.
    """
    directory = root / "LICENSES"
    if not directory.is_dir():
        report("LICENSES/ 不存在；docs/dev/NOTICE 引用了其中的许可原文，"
               "发布产物会携带一个悬空引用")
        return "未达成：LICENSES/ 不存在"

    present = {p.name for p in directory.iterdir() if p.is_file() and p.name != "SHA256SUMS"}
    if not present:
        report("LICENSES/ 为空；许可原文必须随发布产物提供")
        return "未达成：LICENSES/ 为空"

    missing = sorted(set(recorded) - present)
    if missing:
        report("LICENSES/ 缺少 docs/dev/licenses.md 登记的许可原文：" + "、".join(missing))
        return "未达成：许可原文缺失"

    for name in sorted(present):
        actual = sha256(directory / name)
        expected = recorded.get(name)
        if expected and expected != actual:
            report(f"LICENSES/{name} 的 SHA256 为 {actual}，"
                   f"与 docs/dev/licenses.md 登记的 {expected} 不一致")
            return f"未达成：{name} 被改动"

    dirty = subprocess.run(
        ["git", "status", "--porcelain", "--", "LICENSES/"],
        cwd=root, capture_output=True, text=True,
    ).stdout.strip()
    if dirty:
        report("LICENSES/ 下有未提交的改动：" + dirty.replace("\n", "；"))
        return "未达成：LICENSES/ 下有本地改动"

    return f"已达成（LICENSES/ 下 {len(present)} 个文件均与登记值一致）"
```

  **第三步：`NOTICE` 的措辞对齐。** 把"许可原文（`LICENSES/LicenseRef-Slint-Royalty-free-2.0.md`）"的表述明确为"随本发布产物一同提供的许可原文"，并在发布 tarball 的文件清单（`BUILD-P0.02.01` 的 `xtask package`）中加入 `LICENSES/`。

- **逐步落地实施步骤 (Implementation Steps)**：
  1. 从 Slint 发行包取出许可原文放入 `LICENSES/`，生成 `SHA256SUMS`。
  2. 把各文件 SHA256 登记到 `docs/dev/licenses.md`（手工维护段，非生成块）。
  3. 改写 `check_licenses_untouched()`；**自证**：临时把 `LICENSES/` 改名，断言 `gen-licenses.sh --check` 失败并打印"LICENSES/ 不存在"；恢复后断言通过。
  4. 把 `LICENSES/` 加入 `xtask package` 的载荷与三个发行版包的 `%license` / `debian/copyright` 引用。
  5. 把该场景加入 `gen-licenses.sh --self-test`。

- **验收标准 (DoD)**：
  - [ ] `LICENSES/` 存在、非空，且 `docs/dev/NOTICE` 引用的每个路径都真实存在；
  - [ ] `gen-licenses.sh --check` 在 `LICENSES/` 缺失或内容被改动时**失败**（两种场景各有一次实测记录）；
  - [ ] 该场景已加入 `--self-test`，`just check-self-tests` 覆盖它；
  - [ ] 发布 tarball 内含 `LICENSES/`，且 `OB-2`（不单独分发 Slint）与 `OB-3`（嵌入式排除）的声明在 `NOTICE` 中仍完整。

---

## 2. 分片出口准则

- [ ] 9 张 P1 任务卡全部落地，且每张的 DoD 逐条有证据（命令输出、CI 日志或截图）；
- [ ] 主文档第 2 节中由 P1 承接的问题编号（`BUILD-DEF-10`~`BUILD-DEF-15`）全部标记为已修复；
- [ ] 主文档第 3.1 节交付矩阵的 `L-02`~`L-07` 切片状态由"受阻/待评估"更新为实测结论；
- [ ] 三个发行版的包在同一份源码上产出，且三者的 addon 安装路径差异被实测确认；
- [ ] `docs/dev/features.md` 的 `OB-1` 行、`docs/dev/NOTICE`、`docs/dev/licenses.md` 三处状态一致；
- [ ] 新增的依赖（若有）通过 `AGENTS.md` §3.5 的基线评审，且未引入任何网络能力（`just check-net` 仍绿）。

## 3. 续写指令

- **续写输入** = 主文档 [`../opt-deploy.md`](../opt-deploy.md) + 本分片
- **目标分片路径** = `./docs/dev/opt-deploy/phase-2.md`（本文件）或 `./docs/dev/opt-deploy/phase-3.md`
- **模板** = 主文档第 5 节的任务卡字段（基本属性 / 目标与核心交付物 / 工程实现方案与配置文件脚本全文 / 逐步落地实施步骤 / 验收标准）
- **约束** = 新增问题编号必须先在主文档第 2 节登记，并在第 4 节追溯表挂载双向映射；编号必须满足 `dep < self` 的字典序；配置片段与脚本必须完整可直接落盘，**严禁伪代码与占位符**（唯一例外是 `BUILD-P1.01.01` 与 `BUILD-P1.03.05` 中必须由 `docker inspect` 实测取得的容器 digest）
