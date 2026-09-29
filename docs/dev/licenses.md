# 依赖与许可证清单（licenses.md）

> 本文件是 `TASK-1.06.03` 的交付物，也是 [ADR-0000](adr/0000-upstream-decisions.md) 中
> `OB-1`~`OB-6` 六项 Slint Royalty-free 2.0 义务的登记处。手工维护的段落与三个生成块
> （`BEGIN GENERATED` / `END GENERATED` 标记之间）共同构成合规记录；生成块由
> `bash scripts/gen-licenses.sh --write` 从 `cargo metadata` 重新推导，`--check` 逐项核对。

## 1. 范围与结论

- 审计对象：`cargo metadata --format-version 1 --all-features --locked --filter-platform <host>`
  报告的**全部**包，含工作区自身 8 个（7 个 `ime-*` crate 与 `xtask`），与 `Cargo.lock` 一致。
- 结论：依赖闭包中不存在 `GPL-*`、`AGPL-*`、未知或缺失许可证的包；全部依赖的许可证与本项目
  的 `MIT OR Apache-2.0` 双许可兼容。
- 本项目自身以 **MIT OR Apache-2.0** 双许可发布（`Cargo.toml` 的 `license` 字段）；
  `LICENSE-APACHE` 与 `LICENSE-MIT` 由 Phase 2 的许可文件任务落地。
- 词源：`data/sources.toml` 登记的来源全部为宽松许可，`permissive = false` 的来源数为 0。
- Slint：依据 ADR-0000 决策 1 主张 `LicenseRef-Slint-Royalty-free-2.0` 分支，`OB-1`~`OB-6`
  逐条复核见第 5 节。

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

- 包总数：**601**（与 `cargo metadata` 报告一致，含工作区自身 8 个）
- 第三方依赖：**593**；随 `librspinyin.so` 发布的 Linux 构建闭包：不含 dev / build 依赖与
  `xtask`（闭包内包数由 `--write` 登记）
- 禁止的许可证：**0**（`GPL-*`、`AGPL-*`、未知/缺失一律禁止）；书面豁免：**3**

### 第一方（工作区成员，`MIT OR Apache-2.0`）

| 包 | 版本 | 声明许可证 | 主张分支 | 结论 |
|---|---|---|---|---|
| `ime-config` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |
| `ime-core` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |
| `ime-diag` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |
| `ime-dict` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |
| `ime-fcitx5` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |
| `ime-types` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |
| `ime-ui` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |
| `xtask` | 0.1.0 | `MIT OR Apache-2.0` | `MIT` | 本项目自有 |

### 第三方（按声明表达式分组，共 593 个包）

> 同名多版本（`hashbrown`、`windows-sys`、`linux-raw-sys` 等）在包列表中按出现次数重复列出；
> 数量列是包（版本）数。

| 声明表达式 | 数量 | 主张分支 | 结论 | 包 |
|---|---|---|---|---|
| `(MIT OR Apache-2.0) AND NCSA` | 1 | `MIT AND NCSA` | 豁免（书面豁免：NCSA（伊利诺伊大学）是 MIT/BSD 系宽松许可；libfuzzer-sys 只被 workspace 之外的 fuzz 目标使用，不在 Linux 构建闭包内） | `libfuzzer-sys` |
| `(MIT OR Apache-2.0) AND Unicode-3.0` | 1 | `MIT AND Unicode-3.0` | 允许 | `unicode-ident` |
| `0BSD OR MIT OR Apache-2.0` | 1 | `0BSD` | 允许 | `adler2` |
| `Apache-2.0` | 13 | `Apache-2.0` | 允许 | `ciborium`、`ciborium-io`、`ciborium-ll`、`clang-sys`、`gethostname`、`gl_generator`、`glutin`、`glutin_egl_sys`、`glutin_wgl_sys`、`khronos_api`、`linked_hash_set`、`unicode-linebreak`、`winit` |
| `Apache-2.0 / MIT` | 1 | `Apache-2.0` | 允许 | `fnv` |
| `Apache-2.0 AND MIT` | 1 | `Apache-2.0 AND MIT` | 允许 | `dpi` |
| `Apache-2.0 OR MIT` | 44 | `Apache-2.0` | 允许 | `async-channel`、`async-executor`、`async-io`、`async-lock`、`async-process`、`async-signal`、`async-task`、`atomic-waker`、`auto_enums`、`autocfg`、`bit-set`、`bit-vec`、`blocking`、`concurrent-queue`、`criterion`、`criterion-plot`、`derive_utils`、`equivalent`、`event-listener`、`event-listener-strategy`、`fastrand`、`futures-lite`、`idna_adapter`、`indexmap`、`kurbo`、`muda`、`no_std_io2`、`parking`、`pin-project`、`pin-project-internal`、`pin-project-lite`、`polling`、`portable-atomic`、`resvg`、`rustc-hash`、`simd_cesu8`、`simplecss`、`spin_on`、`svgtypes`、`tinytemplate`、`usvg`、`utf8_iter`、`utf8parse`、`uuid` |
| `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | 8 | `Apache-2.0` | 允许 | `io-lifetimes`、`linux-raw-sys`、`linux-raw-sys`、`linux-raw-sys`、`rustix`、`rustix`、`wasip2`、`wit-bindgen` |
| `Apache-2.0/MIT` | 5 | `Apache-2.0` | 允许 | `bit_field`、`cexpr`、`integer-sqrt`、`rustc-hash`、`signal-hook` |
| `BSD-2-Clause` | 4 | `BSD-2-Clause` | 允许 | `arrayref`、`av1-grain`、`rav1e`、`v_frame` |
| `BSD-2-Clause OR Apache-2.0 OR MIT` | 2 | `BSD-2-Clause` | 允许 | `zerocopy`、`zerocopy-derive` |
| `BSD-3-Clause` | 7 | `BSD-3-Clause` | 允许 | `avif-serialize`、`bindgen`、`exr`、`lebe`、`ravif`、`tiny-skia`、`tiny-skia-path` |
| `BSD-3-Clause OR Apache-2.0` | 2 | `BSD-3-Clause` | 允许 | `moxcms`、`pxfm` |
| `BSD-3-Clause OR MIT OR Apache-2.0` | 2 | `BSD-3-Clause` | 允许 | `num_enum`、`num_enum_derive` |
| `BSL-1.0` | 2 | `BSL-1.0` | 豁免（书面豁免：BSL-1.0 已获 OSI 认可且属宽松许可；两个 crate 均为 Windows 专用（Win32 剪贴板后端及其错误辅助库），不在 Linux 构建闭包内） | `clipboard-win`、`error-code` |
| `CC0-1.0 OR Apache-2.0` | 1 | `CC0-1.0` | 允许 | `imgref` |
| `GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0` | 11 | `LicenseRef-Slint-Royalty-free-2.0` | 允许 | `i-slint-backend-linuxkms`、`i-slint-backend-selector`、`i-slint-backend-winit`、`i-slint-common`、`i-slint-compiler`、`i-slint-core`、`i-slint-core-macros`、`i-slint-renderer-skia`、`slint`、`slint-build`、`slint-macros` |
| `ISC` | 1 | `ISC` | 允许 | `libloading` |
| `MIT` | 124 | `MIT` | 允许 | `aligned-vec`、`alloca`、`android-properties`、`arg_enum_proc_macro`、`av-scenechange`、`bincode`、`block2`、`block2`、`built`、`bytes`、`calloop`、`calloop`、`cfg_aliases`、`clru`、`color_quant`、`combine`、`convert_case`、`core_maths`、`crunchy`、`derive_more`、`derive_more-impl`、`dispatch`、`dlib`、`drm`、`drm-ffi`、`drm-fourcc`、`drm-sys`、`endi`、`equator`、`equator-macro`、`fax`、`float-cmp`、`fontconfig-parser`、`fontdb`、`generic-array`、`imagesize`、`input`、`input-sys`、`interpolate_name`、`libm`、`libredox`、`libudev-sys`、`linereader`、`loop9`、`maybe-rayon`、`memoffset`、`new_debug_unreachable`、`nix`、`nom`、`nom`、`noop_proc_macro`、`nu-ansi-term`、`objc-sys`、`objc2`、`objc2`、`objc2-app-kit`、`objc2-cloud-kit`、`objc2-contacts`、`objc2-core-data`、`objc2-core-image`、`objc2-core-location`、`objc2-encode`、`objc2-foundation`、`objc2-foundation`、`objc2-link-presentation`、`objc2-metal`、`objc2-quartz-core`、`objc2-symbols`、`objc2-ui-kit`、`objc2-uniform-type-identifiers`、`objc2-user-notifications`、`oorandom`、`orbclient`、`pico-args`、`pin-weak`、`plotters`、`plotters-backend`、`plotters-svg`、`polib`、`pulp`、`pulp-wasm-simd-flag`、`raw-cpuid`、`reborrow`、`redox_syscall`、`redox_syscall`、`redox_syscall`、`rgb`、`rustybuzz`、`sharded-slab`、`simd-adler32`、`simd_helpers`、`skia-bindings`、`skia-safe`、`slab`、`strict-num`、`strsim`、`strum`、`strum_macros`、`synstructure`、`tiff`、`tokio`、`tracing`、`tracing-appender`、`tracing-attributes`、`tracing-core`、`tracing-log`、`tracing-subscriber`、`udev`、`uds_windows`、`valuable`、`winnow`、`winnow`、`xkbcommon`、`xkbcommon-dl`、`xml-rs`、`xmlwriter`、`y4m`、`zbus`、`zbus_macros`、`zbus_names`、`zmij`、`zvariant`、`zvariant_derive`、`zvariant_utils` |
| `MIT / Apache-2.0` | 2 | `MIT` | 允许 | `cgl`、`copypasta` |
| `MIT OR Apache-2.0` | 274 | `MIT` | 允许 | `aligned`、`allocator-api2`、`android-activity`、`android_system_properties`、`anes`、`anstream`、`anstyle`、`anstyle-parse`、`anstyle-query`、`anstyle-wincon`、`anyhow`、`arbitrary`、`arrayvec`、`as-slice`、`async-broadcast`、`async-recursion`、`async-trait`、`base64`、`bitflags`、`block-buffer`、`borsh`、`bumpalo`、`by_address`、`cast`、`cc`、`cfg-if`、`chrono`、`clap`、`clap_builder`、`clap_derive`、`clap_lex`、`colorchoice`、`const-field-offset`、`const-field-offset-macro`、`core-foundation`、`core-foundation-sys`、`core-graphics`、`core-graphics-types`、`countme`、`cpufeatures`、`crc32fast`、`critical-section`、`crossbeam-channel`、`crossbeam-deque`、`crossbeam-epoch`、`crossbeam-utils`、`crypto-common`、`data-url`、`deranged`、`digest`、`displaydoc`、`either`、`enumflags2`、`enumflags2_derive`、`errno`、`euclid`、`fdeflate`、`field-offset`、`find-msvc-tools`、`flate2`、`form_urlencoded`、`futures`、`futures-channel`、`futures-core`、`futures-executor`、`futures-io`、`futures-macro`、`futures-sink`、`futures-task`、`futures-util`、`getrandom`、`getrandom`、`gif`、`gif`、`glob`、`half`、`hashbrown`、`hashbrown`、`hashbrown`、`hashbrown`、`heck`、`hermit-abi`、`hermit-abi`、`hex`、`iana-time-zone`、`iana-time-zone-haiku`、`idna`、`image`、`image-webp`、`is_terminal_polyfill`、`itertools`、`itertools`、`itoa`、`jni`、`jni-macros`、`jni-sys`、`jni-sys`、`jni-sys-macros`、`jobserver`、`js-sys`、`keyboard-types`、`lazy_static`、`libc`、`log`、`lyon_algorithms`、`lyon_extra`、`lyon_geom`、`lyon_path`、`memmap2`、`ndk`、`ndk-context`、`ndk-sys`、`num-bigint`、`num-complex`、`num-conv`、`num-derive`、`num-integer`、`num-rational`、`num-traits`、`once_cell`、`once_cell_polyfill`、`ordered-stream`、`paste`、`pastey`、`percent-encoding`、`pin-utils`、`piper`、`pkg-config`、`png`、`png`、`powerfmt`、`ppv-lite86`、`prettyplease`、`proc-macro-crate`、`proc-macro2`、`profiling`、`profiling-procmacros`、`proptest`、`quote`、`rand`、`rand_chacha`、`rand_core`、`rand_xorshift`、`raw-window-metal`、`rayon`、`rayon-core`、`redb`、`regex`、`regex-automata`、`regex-syntax`、`rowan`、`roxmltree`、`rustc_version`、`rustversion`、`scopeguard`、`semver`、`serde`、`serde_core`、`serde_derive`、`serde_json`、`serde_repr`、`serde_spanned`、`serde_spanned`、`sha2`、`shlex`、`shlex`、`signal-hook-registry`、`simdutf8`、`siphasher`、`smallvec`、`smol_str`、`smol_str`、`softbuffer`、`stable_deref_trait`、`syn`、`syn`、`sys-locale`、`tar`、`tempfile`、`text-size`、`thiserror`、`thiserror`、`thiserror-impl`、`thiserror-impl`、`thread_local`、`time`、`time-core`、`time-macros`、`toml`、`toml`、`toml_datetime`、`toml_datetime`、`toml_datetime`、`toml_edit`、`toml_edit`、`toml_edit`、`toml_parser`、`toml_write`、`toml_writer`、`ttf-parser`、`typed-index-collections`、`typenum`、`unarray`、`unicode-bidi`、`unicode-script`、`unicode-segmentation`、`unicode-xid`、`unty`、`url`、`vtable`、`vtable-macro`、`wasm-bindgen`、`wasm-bindgen-futures`、`wasm-bindgen-macro`、`wasm-bindgen-macro-support`、`wasm-bindgen-shared`、`web-sys`、`web-time`、`weezl`、`windows`、`windows`、`windows-collections`、`windows-core`、`windows-core`、`windows-core`、`windows-future`、`windows-implement`、`windows-implement`、`windows-interface`、`windows-interface`、`windows-link`、`windows-link`、`windows-numerics`、`windows-result`、`windows-result`、`windows-result`、`windows-strings`、`windows-strings`、`windows-strings`、`windows-sys`、`windows-sys`、`windows-sys`、`windows-sys`、`windows-sys`、`windows-targets`、`windows-targets`、`windows-targets`、`windows-threading`、`windows_aarch64_gnullvm`、`windows_aarch64_gnullvm`、`windows_aarch64_gnullvm`、`windows_aarch64_msvc`、`windows_aarch64_msvc`、`windows_aarch64_msvc`、`windows_i686_gnu`、`windows_i686_gnu`、`windows_i686_gnu`、`windows_i686_gnullvm`、`windows_i686_gnullvm`、`windows_i686_msvc`、`windows_i686_msvc`、`windows_i686_msvc`、`windows_x86_64_gnu`、`windows_x86_64_gnu`、`windows_x86_64_gnu`、`windows_x86_64_gnullvm`、`windows_x86_64_gnullvm`、`windows_x86_64_gnullvm`、`windows_x86_64_msvc`、`windows_x86_64_msvc`、`windows_x86_64_msvc`、`x11rb`、`x11rb-protocol`、`xattr` |
| `MIT OR Apache-2.0 OR LGPL-2.1-or-later` | 2 | `MIT` | 允许 | `r-efi`、`r-efi` |
| `MIT OR Apache-2.0 OR Zlib` | 10 | `MIT` | 允许 | `cursor-icon`、`fontdue`、`glow`、`raw-window-handle`、`xkeysym`、`zune-core`、`zune-core`、`zune-inflate`、`zune-jpeg`、`zune-jpeg` |
| `MIT OR Zlib OR Apache-2.0` | 2 | `MIT` | 允许 | `miniz_oxide`、`miniz_oxide` |
| `MIT/Apache-2.0` | 27 | `MIT` | 允许 | `bitflags`、`bitstream-io`、`codemap`、`codemap-diagnostic`、`filetime`、`foreign-types`、`foreign-types-macros`、`foreign-types-shared`、`linked-hash-map`、`minimal-lexical`、`page_size`、`plain`、`qoi`、`quick-error`、`quick-error`、`rusty-fork`、`scoped-tls-hkt`、`symlink`、`unicode-bidi-mirroring`、`unicode-ccc`、`unicode-properties`、`unicode-vo`、`version_check`、`wait-timeout`、`winapi`、`winapi-i686-pc-windows-gnu`、`winapi-x86_64-pc-windows-gnu` |
| `Unicode-3.0` | 18 | `Unicode-3.0` | 允许 | `icu_collections`、`icu_locale_core`、`icu_normalizer`、`icu_normalizer_data`、`icu_properties`、`icu_properties_data`、`icu_provider`、`litemap`、`potential_utf`、`tinystr`、`writeable`、`yoke`、`yoke-derive`、`zerofrom`、`zerofrom-derive`、`zerotrie`、`zerovec`、`zerovec-derive` |
| `Unlicense OR MIT` | 5 | `MIT` | 允许 | `aho-corasick`、`byteorder-lite`、`memchr`、`termcolor`、`winapi-util` |
| `Unlicense/MIT` | 3 | `MIT` | 允许 | `fst`、`same-file`、`walkdir` |
| `Zlib` | 4 | `Zlib` | 允许 | `foldhash`、`foldhash`、`slotmap`、`zlib-rs` |
| `Zlib OR Apache-2.0 OR MIT` | 15 | `Zlib` | 允许 | `bytemuck`、`bytemuck_derive`、`dispatch2`、`objc2-app-kit`、`objc2-cloud-kit`、`objc2-core-data`、`objc2-core-foundation`、`objc2-core-graphics`、`objc2-core-image`、`objc2-core-text`、`objc2-core-video`、`objc2-io-surface`、`objc2-metal`、`objc2-quartz-core`、`tinyvec` |

### 需人工审查或特别登记

| 包 | 声明表达式 | 主张分支 | 结论 | 依据 |
|---|---|---|---|---|
| `libfuzzer-sys` 0.4.13 | `(MIT OR Apache-2.0) AND NCSA` | `MIT AND NCSA` | 豁免 | NCSA（伊利诺伊大学）是 MIT/BSD 系宽松许可；libfuzzer-sys 只被 workspace 之外的 fuzz 目标使用，不在 Linux 构建闭包内 |
| `clipboard-win` 5.4.1 | `BSL-1.0` | `BSL-1.0` | 豁免 | BSL-1.0 已获 OSI 认可且属宽松许可；两个 crate 均为 Windows 专用（Win32 剪贴板后端及其错误辅助库），不在 Linux 构建闭包内 |
| `error-code` 3.4.0 | `BSL-1.0` | `BSL-1.0` | 豁免 | 同上 |

<!-- END GENERATED: dependency inventory -->

## 4. 词源清单（`data/sources.toml`）

<!-- BEGIN GENERATED: dictionary sources -->
<!-- 由 `scripts/gen-licenses.sh --write` 生成；请勿手工编辑本块 -->

- 来源数：**5**；`permissive = false` 的来源数：**0**（ADR-0000 决策 2 要求为 0）

| id | kind | layer | 许可证 | SPDX | 获取日期 | SHA256 | permissive |
|---|---|---|---|---|---|---|---|
| `pinyin-data` | upstream | L1 | MIT | `MIT` | 2026-09-29 | ec888380a88f317e8ae6f90e8fc2d26207e258c1aa3f9f3b4cb4b34732063f6c | true |
| `unihan` | upstream | L1 | Unicode License | `Unicode-3.0` | 2026-09-29 | a03ec1b8edcf0786d3892eed75ad9ae1a59cf8606fd734cd56337a9e26b48832 | true |
| `jieba-dict` | upstream | L2 | MIT | `MIT` | 2026-09-29 | 5784e097f4363940321ababfbd9851ae6955e98245029d28c89b833a3654c596 | true |
| `base` | derived | L2 | Project-owned (MIT OR Apache-2.0) | `MIT OR Apache-2.0` | — | （衍生数据，不固定哈希） | true |
| `polyphone` | derived | L3c | Project-owned (MIT OR Apache-2.0) | `MIT OR Apache-2.0` | — | （衍生数据，不固定哈希） | true |

<!-- END GENERATED: dictionary sources -->

## 5. Slint Royalty-free 2.0 义务核对（`OB-1`~`OB-6`）

<!-- BEGIN GENERATED: slint obligations -->
<!-- 由 `scripts/gen-licenses.sh --write` 生成；请勿手工编辑本块 -->

- 许可原文：Slint 1.13.1 发行包内的 `LICENSES/LicenseRef-Slint-Royalty-free-2.0.md`
- 原文 SHA256：`5167f5056e850419106ab6265efbdca7cba4d99c849d1445ca0bbf6a1e2315fe`（登记值变化即说明上游条款变动，必须重新复核）

| 义务 | 条款依据（许可原文摘录） | 核对方式 | 复核结论 |
|---|---|---|---|
| `OB-1` 归属展示 | (b) Display the [Slint attribution badge](https://github.com/slint-ui/slint/tree/master/logo/MadeWithSlint-logo-whitebg.png) on a public webpage, preferably where the binaries of your Application can be downloaded from, in such a way that it can be easily found by any visitor to that page. | 断言 `README.md` 与 `README.zh.md` 含 Slint 归属徽章与 `https://slint.dev` 链接（`--check-links` 时另做可达性探测） | 未达成：README.md 与 README.zh.md 不存在（补齐后重新运行 `--write`） |
| `OB-2` 不得单独分发 Slint | The License does not permit to distribute or make the Software publicly available alone and without integration into an Application. For this purpose you may use the Software under the GNU General Public License, version 3. | 扫描 `packaging/` 与构建产物，断言不存在独立的 Slint 库文件（只允许 `librspinyin.so`） | 已达成（`packaging/` 中无独立 Slint 库；构建产物尚未生成，未核对 target/） |
| `OB-3` 不得用于嵌入式系统 | The License does not permit the use of the Software within Embedded Systems. An **Embedded System** is a computer system designed to perform a specific task within a larger mechanical or electrical system. | 断言本文件第 6 节含显式的嵌入式/自助终端/车机排除声明 | 已达成（第 6 节声明） |
| `OB-4` 不得暴露 Slint API | The License does not permit the distribution of Application that exposes the APIs, in part or in total, of the Software. | 由 `scripts/check-slint-leak.sh` 解析 `cargo public-api -p ime-ui` 强制（0.4 规则 11） | 已达成（`scripts/check-slint-leak.sh` 强制） |
| `OB-5` 不得移除许可声明 | You may not remove or alter any license notices (including copyright notices, disclaimers of warranty, or limitations of liability) contained within the source code form of the Software. | 断言 `git status` 无 `LICENSES/` 下的改动，且许可原文的 SHA256 与本文件登记值一致 | 已达成（LICENSES/ 下无改动） |
| `OB-6` 按现状提供、无担保 | SixtyFPS is only liable for conflicting rights of third parties if SixtyFPS was aware of these rights without informing you. Unless required by applicable law or agreed to in writing, SixtyFPS provides the Software on an "as is" basis, without warranties of any kind, either express or implied, including, without limitation, any warranties or conditions of merchantability, or fitness for a particular purpose. | 断言本文件第 7 节与 `README` 许可段含“按现状提供、无担保”的转述 | 已达成（第 7 节转述） |

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
| `git status -- '*LICENSES*'` | `OB-5`（不得篡改许可声明） | 本脚本的 `OB-5` 核对 |

## 9. 复核与再生成

```bash
bash scripts/gen-licenses.sh --write          # 重新生成三个生成块与 docs/dev/NOTICE
bash scripts/gen-licenses.sh --check          # CI：核对本文件是否仍覆盖全部依赖与义务
bash scripts/gen-licenses.sh --check --check-links   # 附加 https://slint.dev 可达性探测
```

上游条款、依赖或词源变动后必须重新运行 `--write` 并提交结果；`--check` 会在依赖清单、词源清单
或义务登记缺失时失败，并在许可原文的 SHA256 与登记值不一致时失败（`OB-5`）。
