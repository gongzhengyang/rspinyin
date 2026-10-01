# 依赖与许可证清单（licenses.md）

> 本文件是 `TASK-1.06.03` 的交付物，也是 [ADR-0000](adr/0000-upstream-decisions.md) 中
> `OB-1`~`OB-6` 六项 Slint Royalty-free 2.0 义务的登记处。手工维护的段落与三个生成块
> （`BEGIN GENERATED` / `END GENERATED` 标记之间）共同构成合规记录；生成块由
> `bash scripts/gen-licenses.sh --write` 从 `cargo metadata` 重新推导，`--check` 逐项核对。

## 1. 范围与结论

- 审计对象：`cargo metadata --format-version 1 --all-features --locked --filter-platform <host>`
  报告的**全部**包，含工作区自身 9 个（8 个 `ime-*` crate 与 `xtask`），与 `Cargo.lock` 一致。
- 结论：依赖闭包中不存在 `GPL-*`、`AGPL-*`、未知或缺失许可证的包；全部依赖的许可证与本项目
  的 `MIT OR Apache-2.0` 双许可兼容。
- 本项目自身以 **MIT OR Apache-2.0** 双许可发布（`Cargo.toml` 的 `license` 字段）；
  `LICENSE-APACHE` 与 `LICENSE-MIT` 已随仓库根提供，`README.md` / `README.zh.md` 的许可段
  与之对应。
- 词源：`data/sources.toml` 登记的来源全部为宽松许可，`permissive = false` 的来源数为 0。
- Slint：依据 ADR-0000 决策 1 主张 `LicenseRef-Slint-Royalty-free-2.0` 分支，`OB-1`~`OB-6`
  逐条复核见第 5 节。

### 1.1 随仓库与发布产物分发的许可原文

| 文件 | 内容 | 断言 |
|---|---|---|
| `LICENSE-APACHE`、`LICENSE-MIT` | 本项目 `MIT OR Apache-2.0` 双许可的两份 SPDX 原文 | 两份文件都存在，且各自含一段只属于它自己的原文（`Apache License` 与 `Version 2.0, January 2004`；`MIT License` 与 `Permission is hereby granted, free of charge`），由 `scripts/gen-licenses.sh --check` 断言。只断言"存在"会放过两份文件被互换的情况，而那正是打包时最难靠肉眼发现的一类错误 |
| `LICENSES/LicenseRef-Slint-Royalty-free-2.0.md` | Slint 1.13.1 发行包内同名文件的逐字副本（路径见第 5 节） | 与发行包内的原文逐字一致（只忽略行尾换行），且 `docs/dev/NOTICE` 引用的每个 `LICENSES/` 路径都真实存在；缺失或被改动都会让 `--check` 失败 |

`OB-1` 的落地位置是本仓库公开页面（`Cargo.toml` 的 `repository` 字段所指的
<https://github.com/gongzhengyang/rspinyin>）上的 `README.md` 与 `README.zh.md`：两份 README 的
第一个二级标题之前都有 Slint 归属徽章并链接 <https://slint.dev>，`--check` 断言其位置与
链接，`--check-links` 另做可达性探测。`LICENSES/` 必须随发布产物一同分发：`docs/dev/NOTICE`
随包提供并指向它，两者分开发布就是一条悬空引用，而这正是上面那条断言要拦住的情形。

## 2. 许可证判定口径

- SPDX 表达式按语义求值：`A OR B` 表示可选择，只要有一个分支在允许清单内即通过，并**主张**
  该分支（本项目的主张结果见第 3 节的"主张分支"列）；`A AND B` 要求每个操作数都在清单内。
  旧式 `A/B` 写法按 OR 处理；`WITH <exception>` 只放宽其修饰的许可证（例如
  `Apache-2.0 WITH LLVM-exception`），不构成另一个许可证。
- 允许清单：`MIT`、`Apache-2.0`、`BSD-2-Clause`、`BSD-3-Clause`、`ISC`、`Zlib`、
  `Unicode-3.0`、`MPL-2.0`、`CC0-1.0`、`0BSD`。`MPL-2.0` 是文件级 copyleft：若修改了某个
  MPL 文件，该文件必须继续以 MPL-2.0 提供；本项目未修改任何 MPL 依赖。
- 未在清单内、但出现在某个 OR 分支上的许可证（如 `Unlicense`、`LGPL-2.1-or-later`）不构成
  失败，因为该项目主张的是另一个分支；被主张的分支必须始终在清单内。
- `LicenseRef-Slint-*` 只对 Slint 自身的 crate（`slint`、`slint-macros`、`slint-build`、
  `i-slint-*`）有效，依据是 ADR-0000 决策 1；出现在其他 crate 上即判失败。
- 书面豁免（第 3.3 节）只覆盖**不随包发布**的 crate 且必须具名；一旦豁免的 crate 落入 Linux
  构建闭包即判失败，因此豁免不可能把代码夹带进 `librspinyin.so`。

## 3. 依赖清单

<!-- BEGIN GENERATED: dependency inventory -->
<!-- 由 `scripts/gen-licenses.sh --write` 生成；请勿手工编辑本块 -->

- 包总数：**432**（与 `cargo metadata` 报告一致，含工作区自身 10 个）
- 第三方依赖：**422**；随 `librspinyin.so` 发布的 Linux 构建闭包：**361** 个包（不含 dev / build 依赖与 `xtask`）
- 禁止的许可证：**0**（`GPL-*`、`AGPL-*`、未知/缺失一律禁止）；书面豁免：**0**

### 第一方（工作区成员，`MIT OR Apache-2.0`）

| 包 | 版本 | 声明许可证 | 主张分支 | 结论 |
|---|---|---|---|---|
| `alloc-count` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |
| `ime-config` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |
| `ime-core` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |
| `ime-diag` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |
| `ime-dict` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |
| `ime-fcitx5` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |
| `ime-types` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |
| `ime-ui` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |
| `ime-ui-addon` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |
| `xtask` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |

### 第三方（按声明表达式分组，共 422 个包）

| 声明表达式 | 数量 | 主张分支 | 结论 | 包 |
|---|---|---|---|---|
| `(MIT OR Apache-2.0) AND Unicode-3.0` | 1 | `MIT AND Unicode-3.0` | 允许 | `unicode-ident` |
| `0BSD OR MIT OR Apache-2.0` | 1 | `0BSD` | 允许 | `adler2` |
| `Apache-2.0` | 7 | `Apache-2.0` | 允许 | `ciborium`、`ciborium-io`、`ciborium-ll`、`gethostname`、`linked_hash_set`、`unicode-linebreak`、`winit` |
| `Apache-2.0 / MIT` | 1 | `Apache-2.0` | 允许 | `fnv` |
| `Apache-2.0 AND MIT` | 1 | `Apache-2.0 AND MIT` | 允许 | `dpi` |
| `Apache-2.0 OR MIT` | 41 | `Apache-2.0` | 允许 | `async-channel`、`async-executor`、`async-io`、`async-lock`、`async-process`、`async-signal`、`async-task`、`atomic-waker`、`auto_enums`、`autocfg`、`bit-set`、`bit-vec`、`blocking`、`concurrent-queue`、`criterion`、`criterion-plot`、`derive_utils`、`equivalent`、`event-listener`、`event-listener-strategy`、`fastrand`、`futures-lite`、`idna_adapter`、`indexmap`、`kurbo`、`no_std_io2`、`parking`、`pin-project`、`pin-project-internal`、`pin-project-lite`、`polling`、`portable-atomic`、`resvg`、`simplecss`、`spin_on`、`svgtypes`、`tinytemplate`、`usvg`、`utf8_iter`、`utf8parse`、`uuid` |
| `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | 6 | `Apache-2.0` | 允许 | `io-lifetimes`、`linux-raw-sys`、`linux-raw-sys`、`linux-raw-sys`、`rustix`、`rustix` |
| `Apache-2.0/MIT` | 4 | `Apache-2.0` | 允许 | `bit_field`、`integer-sqrt`、`rustc-hash`、`signal-hook` |
| `BSD-2-Clause` | 4 | `BSD-2-Clause` | 允许 | `arrayref`、`av1-grain`、`rav1e`、`v_frame` |
| `BSD-2-Clause OR Apache-2.0 OR MIT` | 2 | `BSD-2-Clause` | 允许 | `zerocopy`、`zerocopy-derive` |
| `BSD-3-Clause` | 6 | `BSD-3-Clause` | 允许 | `avif-serialize`、`exr`、`lebe`、`ravif`、`tiny-skia`、`tiny-skia-path` |
| `BSD-3-Clause OR Apache-2.0` | 2 | `BSD-3-Clause` | 允许 | `moxcms`、`pxfm` |
| `BSD-3-Clause OR MIT OR Apache-2.0` | 2 | `BSD-3-Clause` | 允许 | `num_enum`、`num_enum_derive` |
| `CC0-1.0 OR Apache-2.0` | 1 | `CC0-1.0` | 允许 | `imgref` |
| `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` | 10 | `LicenseRef-Slint-Royalty-free-2.0` | 允许 | `i-slint-backend-linuxkms`、`i-slint-backend-selector`、`i-slint-backend-winit`、`i-slint-common`、`i-slint-compiler`、`i-slint-core`、`i-slint-core-macros`、`slint`、`slint-build`、`slint-macros` |
| `ISC` | 1 | `ISC` | 允许 | `libloading` |
| `MIT` | 86 | `MIT` | 允许 | `aligned-vec`、`alloca`、`arg_enum_proc_macro`、`av-scenechange`、`bincode`、`built`、`bytes`、`calloop`、`calloop`、`cfg_aliases`、`clru`、`color_quant`、`convert_case`、`core_maths`、`derive_more`、`derive_more-impl`、`dlib`、`drm`、`drm-ffi`、`drm-fourcc`、`drm-sys`、`endi`、`equator`、`equator-macro`、`fax`、`float-cmp`、`fontconfig-parser`、`fontdb`、`generic-array`、`imagesize`、`input`、`input-sys`、`libm`、`libudev-sys`、`linereader`、`loop9`、`maybe-rayon`、`memoffset`、`new_debug_unreachable`、`nix`、`nom`、`noop_proc_macro`、`nu-ansi-term`、`oorandom`、`pico-args`、`pin-weak`、`plotters`、`plotters-backend`、`plotters-svg`、`polib`、`pulp`、`pulp-wasm-simd-flag`、`raw-cpuid`、`reborrow`、`rgb`、`rustybuzz`、`sharded-slab`、`simd-adler32`、`simd_helpers`、`slab`、`strict-num`、`strsim`、`strum`、`strum_macros`、`synstructure`、`tiff`、`tracing`、`tracing-appender`、`tracing-attributes`、`tracing-core`、`tracing-log`、`tracing-subscriber`、`udev`、`winnow`、`winnow`、`xkbcommon`、`xkbcommon-dl`、`xmlwriter`、`y4m`、`zbus`、`zbus_macros`、`zbus_names`、`zmij`、`zvariant`、`zvariant_derive`、`zvariant_utils` |
| `MIT / Apache-2.0` | 1 | `MIT` | 允许 | `copypasta` |
| `MIT OR Apache-2.0` | 184 | `MIT` | 允许 | `aligned`、`allocator-api2`、`anes`、`anstream`、`anstyle`、`anstyle-parse`、`anstyle-query`、`anyhow`、`arrayvec`、`as-slice`、`async-broadcast`、`async-trait`、`base64`、`bitflags`、`block-buffer`、`borsh`、`bumpalo`、`by_address`、`cast`、`cc`、`cfg-if`、`chrono`、`clap`、`clap_builder`、`clap_derive`、`clap_lex`、`colorchoice`、`const-field-offset`、`const-field-offset-macro`、`countme`、`cpufeatures`、`crc32fast`、`critical-section`、`crossbeam-channel`、`crossbeam-deque`、`crossbeam-epoch`、`crossbeam-utils`、`crypto-common`、`data-url`、`deranged`、`digest`、`displaydoc`、`either`、`enumflags2`、`enumflags2_derive`、`errno`、`euclid`、`fdeflate`、`field-offset`、`find-msvc-tools`、`flate2`、`form_urlencoded`、`futures`、`futures-channel`、`futures-core`、`futures-executor`、`futures-io`、`futures-macro`、`futures-sink`、`futures-task`、`futures-util`、`getrandom`、`getrandom`、`gif`、`gif`、`half`、`hashbrown`、`hashbrown`、`hashbrown`、`hashbrown`、`heck`、`hex`、`iana-time-zone`、`idna`、`image`、`image-webp`、`is_terminal_polyfill`、`itertools`、`itertools`、`itoa`、`jobserver`、`lazy_static`、`libc`、`log`、`lyon_algorithms`、`lyon_extra`、`lyon_geom`、`lyon_path`、`memmap2`、`num-bigint`、`num-complex`、`num-conv`、`num-derive`、`num-integer`、`num-rational`、`num-traits`、`once_cell`、`ordered-stream`、`paste`、`pastey`、`percent-encoding`、`pin-utils`、`piper`、`pkg-config`、`png`、`png`、`powerfmt`、`ppv-lite86`、`proc-macro-crate`、`proc-macro2`、`profiling`、`profiling-procmacros`、`proptest`、`quote`、`rand`、`rand_chacha`、`rand_core`、`rand_xorshift`、`rayon`、`rayon-core`、`redb`、`regex`、`regex-automata`、`regex-syntax`、`rowan`、`roxmltree`、`rustc_version`、`rustversion`、`scopeguard`、`semver`、`serde`、`serde_core`、`serde_derive`、`serde_json`、`serde_repr`、`serde_spanned`、`sha2`、`shlex`、`signal-hook-registry`、`siphasher`、`smallvec`、`smol_str`、`smol_str`、`softbuffer`、`stable_deref_trait`、`syn`、`syn`、`sys-locale`、`tempfile`、`text-size`、`thiserror`、`thiserror`、`thiserror-impl`、`thiserror-impl`、`thread_local`、`time`、`time-core`、`time-macros`、`toml`、`toml_datetime`、`toml_datetime`、`toml_edit`、`toml_edit`、`toml_parser`、`toml_writer`、`ttf-parser`、`typed-index-collections`、`typenum`、`unarray`、`unicode-bidi`、`unicode-script`、`unicode-segmentation`、`unicode-xid`、`unty`、`url`、`vtable`、`vtable-macro`、`wasm-bindgen`、`wasm-bindgen-macro`、`wasm-bindgen-macro-support`、`wasm-bindgen-shared`、`weezl`、`x11rb`、`x11rb-protocol` |
| `MIT OR Apache-2.0 OR Zlib` | 9 | `MIT` | 允许 | `cursor-icon`、`fontdue`、`raw-window-handle`、`xkeysym`、`zune-core`、`zune-core`、`zune-inflate`、`zune-jpeg`、`zune-jpeg` |
| `MIT OR Zlib OR Apache-2.0` | 2 | `MIT` | 允许 | `miniz_oxide`、`miniz_oxide` |
| `MIT/Apache-2.0` | 18 | `MIT` | 允许 | `bitflags`、`bitstream-io`、`codemap`、`codemap-diagnostic`、`linked-hash-map`、`page_size`、`qoi`、`quick-error`、`quick-error`、`rusty-fork`、`scoped-tls-hkt`、`symlink`、`unicode-bidi-mirroring`、`unicode-ccc`、`unicode-properties`、`unicode-vo`、`version_check`、`wait-timeout` |
| `Unicode-3.0` | 18 | `Unicode-3.0` | 允许 | `icu_collections`、`icu_locale_core`、`icu_normalizer`、`icu_normalizer_data`、`icu_properties`、`icu_properties_data`、`icu_provider`、`litemap`、`potential_utf`、`tinystr`、`writeable`、`yoke`、`yoke-derive`、`zerofrom`、`zerofrom-derive`、`zerotrie`、`zerovec`、`zerovec-derive` |
| `Unlicense OR MIT` | 4 | `MIT` | 允许 | `aho-corasick`、`byteorder-lite`、`memchr`、`termcolor` |
| `Unlicense/MIT` | 3 | `MIT` | 允许 | `fst`、`same-file`、`walkdir` |
| `Zlib` | 4 | `Zlib` | 允许 | `foldhash`、`foldhash`、`slotmap`、`zlib-rs` |
| `Zlib OR Apache-2.0 OR MIT` | 3 | `Zlib` | 允许 | `bytemuck`、`bytemuck_derive`、`tinyvec` |

<!-- END GENERATED: dependency inventory -->

## 4. 词源清单（`data/sources.toml`）

<!-- BEGIN GENERATED: dictionary sources -->
<!-- 由 `scripts/gen-licenses.sh --write` 生成；请勿手工编辑本块 -->

- 来源数：**6**；`permissive = false` 的来源数：**0**（ADR-0000 决策 2 要求为 0）

| id | kind | layer | 许可证 | SPDX | 获取日期 | SHA256 | permissive |
|---|---|---|---|---|---|---|---|
| `pinyin-data` | upstream | L1 | MIT | `MIT` | 2026-09-29 | ec888380a88f317e8ae6f90e8fc2d26207e258c1aa3f9f3b4cb4b34732063f6c | true |
| `unihan` | upstream | L1 | Unicode License | `Unicode-3.0` | 2026-09-29 | a03ec1b8edcf0786d3892eed75ad9ae1a59cf8606fd734cd56337a9e26b48832 | true |
| `jieba-dict` | upstream | L2 | MIT | `MIT` | 2026-09-29 | 5784e097f4363940321ababfbd9851ae6955e98245029d28c89b833a3654c596 | true |
| `base` | derived | L2 | Project-owned (MIT OR Apache-2.0) | `MIT OR Apache-2.0` | — | （衍生数据，不固定哈希） | true |
| `polyphone` | derived | L3c | Project-owned (MIT OR Apache-2.0) | `MIT OR Apache-2.0` | — | （衍生数据，不固定哈希） | true |
| `phrase` | derived | L5 | Project-owned (MIT OR Apache-2.0) | `MIT OR Apache-2.0` | — | （衍生数据，不固定哈希） | true |

<!-- END GENERATED: dictionary sources -->

## 5. Slint Royalty-free 2.0 义务核对（`OB-1`~`OB-6`）

<!-- BEGIN GENERATED: slint obligations -->
<!-- 由 `scripts/gen-licenses.sh --write` 生成；请勿手工编辑本块 -->

- 许可原文：`/home/gong/.cargo/registry/src/mirrors.ustc.edu.cn-38d0e5eb5da2abae/slint-1.13.1/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md`（Slint 发行包内，随 `cargo metadata` 展开）
- 原文 SHA256：`5167f5056e850419106ab6265efbdca7cba4d99c849d1445ca0bbf6a1e2315fe`（登记值变化即说明上游条款变动，必须重新复核）

| 义务 | 条款依据（许可原文摘录） | 核对方式 | 复核结论 |
|---|---|---|---|
| `OB-1` 归属展示 | (b) Display the [Slint attribution badge](https://github.com/slint-ui/slint/tree/master/logo/MadeWithSlint-logo-whitebg.png) on a public webpage, preferably where the binaries of your Application can be downloaded from, in such a way that it can be easily found by any visitor to that page. | 断言 `README.md` 与 `README.zh.md` 含 Slint 归属徽章与 `https://slint.dev` 链接（`--check-links` 时另做可达性探测） | 已达成（README.md 与 README.zh.md 的第一个二级标题之前均含 Slint 归属徽章，链接 https://slint.dev；公开页面 https://github.com/gongzhengyang/rspinyin） |
| `OB-2` 不得单独分发 Slint | The License does not permit to distribute or make the Software publicly available alone and without integration into an Application. For this purpose you may use the Software under the GNU General Public License, version 3. | 扫描 `packaging/` 与构建产物，断言不存在独立的 Slint 库文件（只允许 `librspinyin.so`） | 已达成（`packaging/` 中无独立 Slint 库；构建产物尚未生成，未核对 target/） |
| `OB-3` 不得用于嵌入式系统 | The License does not permit the use of the Software within Embedded Systems. An **Embedded System** is a computer system designed to perform a specific task within a larger mechanical or electrical system. | 断言本文件第 6 节含显式的嵌入式/自助终端/车机排除声明 | 已达成（第 6 节声明） |
| `OB-4` 不得暴露 Slint API | The License does not permit the distribution of Application that exposes the APIs, in part or in total, of the Software. | 由 `scripts/check-slint-leak.sh` 解析 `cargo public-api -p ime-ui` 强制（0.4 规则 11） | 已达成（`scripts/check-slint-leak.sh` 强制） |
| `OB-5` 不得移除许可声明 | You may not remove or alter any license notices (including copyright notices, disclaimers of warranty, or limitations of liability) contained within the source code form of the Software. | 断言 `LICENSES/` 存在且非空、其中的许可原文与 Slint 发行包内的同名原文逐字一致、`docs/dev/NOTICE` 引用的每个 `LICENSES/` 路径都真实存在，并断言 `git status` 无 `LICENSES/` 下的改动 | 未达成：LICENSES/ 下有本地改动 |
| `OB-6` 按现状提供、无担保 | SixtyFPS is only liable for conflicting rights of third parties if SixtyFPS was aware of these rights without informing you. Unless required by applicable law or agreed to in writing, SixtyFPS provides the Software on an "as is" basis, without warranties or conditions of any kind, either express or implied, including, without limitation, any warranties or conditions of merchantability, or fitness for a particular purpose. | 断言本文件第 7 节与 `README` 许可段含“按现状提供、无担保”的转述 | 已达成（第 7 节转述） |

<!-- END GENERATED: slint obligations -->

## 6. 嵌入式与自助终端排除声明（`OB-3`）

本项目的 Slint 授权（`LicenseRef-Slint-Royalty-free-2.0`）**不覆盖嵌入式系统**。按许可原文，
嵌入式系统指"为在更大的机械或电气系统中执行特定任务而设计的计算机系统"，例如家电显示屏、
POS 机与自助终端、车载仪表。rspinyin 是 Linux 桌面输入法，属于授权范围内的桌面应用；若要在
上述场景部署，必须自行取得 **GPL-3.0** 或 SixtyFPS GmbH 的**商业许可**，本项目的 Royalty-free
授权对此不提供任何权利。该限制同步登记为 `docs/dev/features.md` 0.5.2 能力矩阵中的一行
"不支持"。

## 7. 免责声明转述（`OB-6`）

Slint 按**"现状"提供、无担保**：除法律要求或书面约定外，SixtyFPS GmbH 不提供任何明示或默示
的担保，包括但不限于可销售性与特定用途适用性的担保。除法律要求外，SixtyFPS GmbH 不对使用该
软件造成的任何直接、间接、偶发或后果性损害负责。对第三方权利冲突，其责任仅限于"SixtyFPS
已知悉该权利而未告知"的情形。本项目如实转述上述条款，不作出超出许可范围的担保承诺。

## 8. 强制这些结论的门禁

| 门禁 | 覆盖 | 位置 |
|---|---|---|
| `scripts/gen-licenses.sh` | 依赖许可证分类、词源审计、`OB-1`~`OB-6` 复核 | 本文件与 `NOTICE` 的生成与核对 |
| `scripts/check-slint-leak.sh` | `OB-4`（0.4 规则 11：`ime-ui` 不得导出 Slint 类型） | `just check-slint` |
| `scripts/check-dict-sources.sh` | 词源许可与 SHA256（ADR-0000 决策 2） | `just check-dict` |
| `scripts/check-no-network.sh` | 依赖闭包中不存在网络能力（0.4 规则 6） | `just check-net` |
| `deny.toml` + `cargo deny check` | 同一批决策的机器可读形式：RustSec 通告、许可证允许清单、网络 crate 禁用、依赖源锁定 | `just check-advisories` |
| `LICENSES/` 的存在与内容比对（对照 Slint 发行包内的原文）＋ `git status -- LICENSES/` | `OB-5`（不得篡改许可声明） | 本脚本的 `OB-5` 核对 |
| `LICENSE-APACHE` / `LICENSE-MIT` 的存在与内容抽查 | `Cargo.toml` 的 `MIT OR Apache-2.0` 声明；发布产物必须随附两份原文 | 本脚本的项目许可原文核对 |

`deny.toml` 不是本文件之外的第二套口径，而是同一批决策在 `cargo deny check` 中的表达：
许可证允许清单与第 2 节的判定口径一致，`LicenseRef-Slint-Royalty-free-2.0` 是唯一被列入的
Slint 分支——`GPL-3.0-only` 与商业分支 `LicenseRef-Slint-Software-3.0` 都**不**在清单内，
它们只是 Slint 三选一表达式里本项目明确不主张的分支；`[bans]` 把 0.4 规则 6 的禁用集重申
一遍；`[sources]` 只承认 crates.io 这一个依赖源，git 源一律拒绝（AGENTS.md 3.1 禁止
submodule 的理由），未登记的其他 registry 会以警告形式暴露出来供人工复核。
两处刻意的差异：允许清单额外列出 `Unlicense` 与 `Apache-2.0 WITH LLVM-exception`——前者在
图中只作为 `Unlicense OR MIT` / `Unlicense/MIT` 的另一个分支出现（本文件主张的是 MIT 分支），
列出它只是让结论不依赖工具是否展开旧式 `A/B` 写法；后者是 Apache-2.0 的附加许可而非另一个
许可证。`deny.toml` 的许可证清单是**策略**而非当前图的快照，因此允许清单里当前图未使用的
`MPL-2.0` 不构成问题；反过来 `CC0-1.0` 与 `0BSD` 虽然只以 OR 分支的形式出现
（`CC0-1.0 OR Apache-2.0`、`0BSD OR MIT OR Apache-2.0`），也必须在清单内，否则本项目的
主张分支就不成立。

通告方面的书面豁免与 `deny.toml` 一一对应：`RUSTSEC-2026-0009`（`time`）的完整理由写在
`.cargo/audit.toml`，Slint 依赖树中四个 unmaintained crate（`paste`、`bincode`、
`rustybuzz`、`ttf-parser`）的豁免写在 `deny.toml` 的 `[advisories]`，逐条给出依赖路径与
"通告为 unmaintained 而非漏洞"的依据，并按**通告编号**而非 crate 名豁免，使将来针对同一
crate 的漏洞通告仍然会让门禁失败。BSL-1.0（`clipboard-win`、`error-code`）的许可证豁免以
`[[licenses.exceptions]]` 具名登记，只覆盖不在 Linux 构建闭包内的 Windows 专用依赖。

## 9. 复核与再生成

```bash
bash scripts/gen-licenses.sh --write          # 重新生成三个生成块与 docs/dev/NOTICE
bash scripts/gen-licenses.sh --check          # CI：核对本文件是否仍覆盖全部依赖与义务
bash scripts/gen-licenses.sh --check --check-links   # 附加 https://slint.dev 可达性探测
```

上游条款、依赖或词源变动后必须重新运行 `--write` 并提交结果；`--check` 会在依赖清单、词源清单
或义务登记缺失时失败，并在许可原文的 SHA256 与登记值不一致时失败（`OB-5`）。

`--check` 还守着两处仓库自身的许可文件，它们的失败不需要重新生成任何文档，需要的是改文件：

- `LICENSE-APACHE` / `LICENSE-MIT` 缺失，或某一份不含它自己那段原文时失败（`Cargo.toml`
  声明了双许可，发布产物就要随附两份原文）。
- `LICENSES/` 不存在、为空、缺少 `docs/dev/NOTICE` 引用的文件、其副本与 Slint 发行包内的
  原文不一致，或 `git status` 报告该目录下有未提交的改动时失败。最后一条意味着**新增或改动
  `LICENSES/` 后必须先提交再跑 `--check`**：`--write` 只刷新文档里的生成块，不会替你提交
  许可原文。
