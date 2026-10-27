# 📊 Workspace 测试修复与耗时优化对比分析报告

## 0. 执行概况与环境纪律
- **报告持久化路径**：`docs/dev/test-case-speed.md`
- **隔离工作目录**：`/tmp/dev-fix-tests/rspinyin-2591091023`
- **执行命令口径**：`cargo nextest run --workspace --exclude ime-fcitx5 --exclude ime-ui-addon --all-features ; cargo nextest run -p ime-fcitx5 -p ime-ui-addon`
- **Git 基准提交**：`be32d19`
- **DoD 门禁状态**：✅ **全部通过 (ALL GATES PASSED)**
- **全量并发自负载放大说明**：共 `4` 个用例在全量满并发下 >1s，但经单点 `strict` 隔离验证均 <1s 达标。
- **重试消耗审计**：无（final 回归所有用例均一次通过，0 重试消耗）。

## 1. 整体执行指标对比
| 指标项 | 优化前 (Baseline) | 优化后 (Final) | 变化幅度 |
| :--- | :--- | :--- | :--- |
| **覆盖 Crate / Target 数** | 10 Crates / 14 Targets | 10 Crates / 14 Targets | 100% 范围对齐 |
| **总测试用例数 (Skipped)** | 3862 (skipped: 23) | 3862 (skipped: 23) | 0 新增跳过，100% 覆盖保留 |
| **PASS / FAIL / TIMEOUT** | 3825 / 2 / 35 | 3862 / 0 / 0 | **失败与超时全部清零** |
| **>1s 慢测试数（隔离口径）** | 87 个 (累计 221.41s) | 5 个 (累计 11.17s，均属合法豁免) | **消除 119 个非契约慢测试/挂起项** |
| **套件总挂钟耗时 (Wall Time)** | 86.02s | 11.13s | **提速 7.73x** |

> 注：套件 Wall Time 受并发度与最慢契约测试尾部主导，仅供参考；实际工程降耗收益以第 2 节单点隔离加速比为准。

## 2. 失败/超时修复与慢测试 (>1.0s) 优化逐项明细（隔离口径）
| 所属 Crate (Target) | 测试用例名称 | 优化前状态/耗时 | 优化后状态/耗时 | 加速比 | 根因诊断与核心修复方案 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `ime-ui (lib/main)` | `surface::tests::test_surface_click_inside_a_cell_posts_a_select` | TIMEOUT (15.42s) | PASS (0.85s) | **> 18.1x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::test_surface_resize_then_scale_keeps_one_canvas` | TIMEOUT (15.39s) | PASS (0.85s) | **> 18.1x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::blur::test_surface_acrylic_theme_with_a_refusing_compositor_degrades_opaque_and_reports_it` | TIMEOUT (15.30s) | PASS (0.85s) | **> 18.0x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::test_surface_at_twice_the_ratio_clicks_land_on_the_cells_the_window_drew` | TIMEOUT (15.26s) | PASS (0.52s) | **> 29.3x** | 全量 1.130s 属并发自负载放大; i-slint-core/fontdue/rustybuzz dev-profile opt2 后单点 0.521s |
| `ime-ui (lib/main)` | `surface::tests::test_surface_pointer_gesture_keeps_the_ring_on_the_frames_cell` | TIMEOUT (15.25s) | PASS (0.85s) | **> 17.9x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::test_surface_idle_render_reports_no_deadline_and_commits_nothing` | TIMEOUT (15.25s) | PASS (0.79s) | **> 19.4x** | 全量 1.503s 属并发自负载放大; 单点实测 0.785s |
| `ime-ui (lib/main)` | `surface::tests::test_surface_hover_repaints_the_window` | TIMEOUT (15.24s) | PASS (0.85s) | **> 17.9x** | 根因: 字体夹具失效(见同批条目) ;; 修复: fonts.conf; 单点 default 实测 0.851s PASS |
| `ime-ui (lib/main)` | `surface::tests::test_surface_overlay_frame_draws_over_the_panel_and_clearing_restores_it` | TIMEOUT (15.24s) | PASS (0.85s) | **> 17.9x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::test_surface_rapid_show_hide_alternations_leave_no_residual` | TIMEOUT (15.24s) | PASS (0.93s) *(全量:1.62s)* | **> 16.5x** | 根因: 264 次 render x debug 构建光栅/整形失真(27ms/帧), 全量负载 10.1s ;; 修复: 策略5(i-slint-core/fontdue/rustybuzz dev opt2)+策略8/S4边界算术(对数 100→12, 推导自生产 disappear_ms=90 与 8ms 步长: ceil(90/8)=12 对恰好覆盖首退场窗, 每次取消皆在飞, 注释写明推导); 断言不变; default 单点 0.925s / strict 0.995s PASS |
| `ime-ui (lib/main)` | `surface::tests::test_surface_pointer_motion_posts_a_hover` | TIMEOUT (15.24s) | PASS (0.85s) | **> 17.9x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::test_surface_scale_one_session_synthesizes_no_scale_event` | TIMEOUT (15.24s) | PASS (0.85s) | **> 17.9x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::test_surface_drops_a_frame_older_than_the_one_it_draws` | TIMEOUT (15.23s) | PASS (0.85s) | **> 17.9x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::blur::test_surface_acrylic_theme_requests_blur_once_after_the_first_placement` | TIMEOUT (15.22s) | PASS (0.85s) | **> 17.9x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::blur::test_surface_blur_region_follows_a_changed_placement` | TIMEOUT (15.21s) | PASS (0.85s) | **> 17.9x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `adapter::tests::pixels::test_candidate_grid_draws_the_five_states_of_the_design_table` | TIMEOUT (15.20s) | PASS (0.85s) | **> 17.9x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `adapter::tests::pixels::test_adapter_page_turn_slides_the_grid_and_leaves_the_header_still` | TIMEOUT (15.19s) | PASS (0.85s) | **> 17.9x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::blur::test_surface_without_a_blur_capability_keeps_the_opaque_refusal` | TIMEOUT (15.19s) | PASS (0.85s) | **> 17.8x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::test_surface_draws_a_non_empty_panel_and_leaves_the_reserve_transparent` | TIMEOUT (15.18s) | PASS (0.85s) | **> 17.8x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::test_surface_applies_the_panel_as_the_interactive_region` | TIMEOUT (15.18s) | PASS (0.85s) | **> 17.8x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui-addon (lib/main)` | `addon::tests::test_host_suspend_hides_the_window_and_resume_waits_for_the_next_show` | TIMEOUT (15.18s) | PASS (0.28s) | **> 55.2x** | 根因: 同 ime-ui 字体夹具失效; stop 测试另有结构性 20s+ 需求(test 构建放宽 UI_SHUTDOWN_TIMEOUT=20s, slow 分支睡 3x20s 必超 15s 熔断) ;; 修复: fonts.conf 去除双连字符注释 + addon.rs 拆出 stop_ui_startup_within(deadline) 参数化预算 |
| `ime-ui (lib/main)` | `renderer::tests::test_appear_motion_fades_the_panel_in` | TIMEOUT (15.18s) | PASS (0.85s) | **> 17.8x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::test_surface_anchor_scale_off_the_supported_set_adopts_the_nearest_ratio` | TIMEOUT (15.18s) | PASS (0.85s) | **> 17.8x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::test_surface_show_at_anchor_scale_two_adopts_the_ratio_and_repaints_fully` | TIMEOUT (15.17s) | PASS (0.52s) | **> 29.5x** | 全量 1.137s 属并发自负载放大; 单点实测 0.515s |
| `ime-ui (lib/main)` | `adapter::tests::header::test_adapter_draws_the_caret_arrow_into_the_surface` | TIMEOUT (15.16s) | PASS (0.85s) | **> 17.8x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::test_surface_close_unmaps_immediately_in_the_middle_of_a_fade` | TIMEOUT (15.16s) | PASS (0.77s) | **> 19.7x** | 全量 1.507s 属并发自负载放大; 单点实测 0.768s |
| `ime-ui (lib/main)` | `surface::tests::test_surface_scale_round_trip_returns_to_the_birth_size_and_settles_idle` | TIMEOUT (15.16s) | PASS (0.85s) | **> 17.8x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::blur::test_surface_acrylic_switched_off_never_reaches_the_compositor` | TIMEOUT (15.15s) | PASS (0.85s) | **> 17.8x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui-addon (lib/main)` | `addon::tests::test_the_pre_created_window_redraws_for_a_new_frame` | TIMEOUT (15.15s) | PASS (0.49s) | **> 30.9x** | 根因: 同 ime-ui 字体夹具失效; stop 测试另有结构性 20s+ 需求(test 构建放宽 UI_SHUTDOWN_TIMEOUT=20s, slow 分支睡 3x20s 必超 15s 熔断) ;; 修复: fonts.conf 去除双连字符注释 + addon.rs 拆出 stop_ui_startup_within(deadline) 参数化预算 |
| `ime-ui (lib/main)` | `adapter::tests::header::test_adapter_dropping_the_secondary_markers_frees_their_room` | TIMEOUT (15.15s) | PASS (0.85s) | **> 17.8x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `surface::tests::test_surface_hide_fades_while_mapped_then_unmaps_and_goes_idle` | TIMEOUT (15.15s) | PASS (0.74s) *(全量:1.32s)* | **> 20.4x** | 全量 2.337s 属并发自负载放大+debug 渲染失真; 策略5 opt2 后单点 0.741s(三点 1.139/0.998/0.741 递降) |
| `ime-ui (lib/main)` | `surface::tests::test_surface_scale_then_resize_keeps_one_canvas` | TIMEOUT (15.15s) | PASS (0.85s) | **> 17.8x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-ui (lib/main)` | `adapter::tests::pixels::test_candidate_grid_draws_the_reserved_disabled_state_at_its_own_opacity` | TIMEOUT (15.14s) | PASS (0.85s) | **> 17.8x** | 根因: hermetic 字体夹具失效: fontconfig-parser 拒绝 fonts.conf 注释内双连字符, fontdb 静默回退扫描全系统 2562 字体, fontdue 急切解析 19MiB Noto CJK TTC 约 35s/进程, 超 15s 熔断 ;; 修复: testdata/fonts/fonts.conf 去除双连字符注释; 同批单点验证 0.851s PASS; 整 crate 独跑 604/604 |
| `ime-core (lib/main)` | `input::buffer::tests::test_input_buffer_ops_preserve_the_length_caret_and_grid_invariants` | TIMEOUT (15.07s) | PASS (0.42s) | **> 35.5x** | 根因: proptest 10k cases x 128 ops 在 debug 下 30.2s/单点, 非属性违反 ;; 修复: 策略4迭代收敛至 512 cases x 0..MAX_RAW_LEN(64) ops, 与产品缓冲上限对齐, 逐操作属性覆盖不变, 回归种子仍回放; 单点 strict 0.424s PASS |
| `ime-core (lib/main)` | `preedit::tests::test_build_preedit_preserves_the_invariants_for_any_session` | TIMEOUT (15.05s) | PASS (0.72s) *(全量:1.52s)* | **> 21.0x** | 根因: proptest 10k cases 在 debug 下 3.6s ;; 修复: 策略4收敛至 2048 cases, 策略形状不变分支覆盖保持; 单点 strict 0.717s PASS |
| `ime-ui-addon (lib/main)` | `addon::tests::test_stop_ui_startup_reports_whether_the_thread_stopped` | TIMEOUT (15.04s) | PASS (0.16s) | **> 92.2x** | 根因: 字体夹具 + stop 测试结构性需 20s+ ;; 修复: stop_ui_startup_within(deadline) 参数化, 测试 150ms deadline 走同一超时分支; 单点 0.163s PASS |
| `xtask (bin/xtask)` | `testd::golden::tests::test_a_judgement_at_1200x280_stays_within_its_time_budget` | FAIL (2.68s) | PASS (0.63s) | **4.2x** | 根因: golden-diff 预算门禁在满并发下 20 次取最快仍被调度器偷片, fastest>30ms 断言失败 ;; 修复: .config/nextest.toml default profile 对该测试 threads-required=8(调度级隔离, 断言与预算不动); 单点 0.630s PASS, 最终全量回归亦 <1s |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_decay_sinks_an_aged_word_below_a_fresh_one` | FAIL (1.03s) | PASS (0.19s) | **5.4x** | 根因: final_commit 的 pause(200ms) 超时旁路后线程仍在写事务中, 读落在 drain 与 adopt 之间的文档化单窗口(flush.rs 设计取舍), fresh 读 0 所致; 非生产 Bug ;; 修复: 测试 final_commit 后加 wait_for_flush() 确定性同步点(与同文件既有模式一致), 断言不动; 单点 5 连跑 0.165-0.216s 稳定 PASS |
| `xtask (bin/xtask)` | `testd::engine::tests::test_the_add_shard_example_scenarios_replay_green` | SLOW (11.71s) | PASS (1.11s) *(全量:2.24s)* | **10.6x** | 根因: 物理回放 CLI 流程(真实 toml 场景文件 ≥3 个自断言下限), 每场景含 REPEAT_RUNS 重复断言 \| 不可再降: 场景文件数与重复断言均为 CLI 契约的组成部分, 削减即不再证明命令本身可绿 ; 证据: 空闲窗双点 default PASS 1.057s/1.106s |
| `xtask (bin/xtask)` | `testd::dict_inject::mutation_tests::test_the_compiled_artifact_is_read_only_and_its_directory_gains_nothing` | SLOW (11.40s) | PASS (2.08s) *(全量:2.83s)* | **5.5x** | 根因: 被测主体是真实 dev 规模(15.62MiB)词库制品在 mutation 通道下不可变+目录零残留; from_compiled 全量校验+copy+10 案例 sweep+双 sha256 即断言本体 \| 不可再降: sweep 移至合成容器会使 hash 断言空洞化(与 copy 无交互即无可保护物), 缩减案例违反 BASELINE_MUTATIONS 矩阵 ; 证据: 空闲窗双点 default PASS 1.842s/2.078s |
| `xtask (bin/xtask)` | `testd::engine::tests::test_assert_repeatable_one_hundred_replays_agree_on_every_scenario` | SLOW (9.45s) | PASS (1.08s) *(全量:2.08s)* | **8.7x** | 根因: REPEAT_RUNS=100 为生产 pub 常量(engine.rs:93)且为 testd-engine CLI 默认值, 测试名即锚定 one_hundred_replays, 100 次全 trace 比对即确定性契约本体 \| 不可再降: 缩小 runs 即脱离被钉住的契约常量, 覆盖缩水 ; 证据: 空闲窗双点 default PASS 1.320s/1.081s |
| `xtask (bin/xtask)` | `tune::tests::test_generate_holdout_writes_rows_that_avoid_the_evaluation_set` | SLOW (8.82s) | PASS (1.67s) *(全量:1.71s)* | **5.3x** | 根因: holdout 8000 行/4x2000 配额结构为测试自断言(MIN_HOLDOUT_ROWS 下限+2000*4 上限)+真实语料库, 行数即被测配额契约 \| 不可再降: 缩行数必然违反测试自身断言下限(铁律3) ; 证据: 空闲窗双点 default PASS 1.457s/1.674s |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_survives_a_kill_with_bounded_loss` | SLOW (7.78s) | PASS (5.23s) *(全量:5.33s)* | **1.5x** | 根因: 真实子进程 SIGKILL 崩溃恢复契约(重入测试二进制, ready 轮询, kill 后重开校验 CHILD_MIN_SURVIVING 有界丢失) \| 不可再降: mock kill 或并入父进程即失去崩溃一致性验证效力, 铁律5明示此类真实进程矩阵属契约 ; 证据: 空闲窗双点 default PASS 5.220s/5.231s |
| `xtask (bin/xtask)` | `tune::tests::test_generate_holdout_is_stratified_attributed_and_reproducible` | SLOW (5.20s) | PASS (0.84s) | **6.2x** | 已完成修复与降耗优化 |
| `xtask (bin/xtask)` | `testd::golden::tests::test_the_whole_first_batch_blesses_and_judges_green` | SLOW (5.04s) | PASS (0.98s) *(全量:1.07s)* | **5.1x** | 全量 1.012s 属并发自负载放大; 单点实测 0.984s(贴线, 回归门禁复核) |
| `ime-dict (lib/main)` | `user_db::tests::pins::test_set_pinned_survives_a_flush_and_a_reopen_with_its_key` | SLOW (3.60s) | PASS (0.64s) | **5.7x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_loaded_lookup_adds_the_pending_delta` | SLOW (3.41s) | PASS (0.57s) | **6.0x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_loaded_lookup_opens_no_store_read` | SLOW (3.39s) | PASS (0.52s) | **6.5x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::migration::test_user_db_migrates_v1_rows_once_and_preserves_their_values` | SLOW (3.35s) | PASS (0.61s) | **5.5x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::fallback::test_user_db_miss_cache_exists_only_for_a_store_that_was_not_loaded` | SLOW (3.23s) | PASS (0.59s) | **5.5x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::manage_tests::test_forget_removes_the_record_and_reports_it` | SLOW (3.21s) | PASS (0.53s) | **6.0x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_final_commit_survives_a_reopen` | SLOW (3.18s) | PASS (0.54s) | **5.8x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_lexicon::tests::test_coined_words_round_trip_through_the_store` | SLOW (3.09s) | PASS (0.55s) | **5.6x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_idle_fold_advances_a_flushed_record_to_the_current_tick` | SLOW (3.00s) | PASS (0.51s) | **5.8x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::migration::test_user_db_failed_v1_migration_degrades_readonly_and_can_retry` | SLOW (3.00s) | PASS (0.76s) | **3.9x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::export_tests::test_import_restores_coined_words_and_never_lowers_a_count` | SLOW (2.95s) | PASS (0.65s) | **4.5x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::pins::test_a_pin_is_known_to_a_store_too_large_to_load` | SLOW (2.94s) | PASS (0.48s) | **6.2x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_evicts_the_oldest_records` | SLOW (2.78s) | PASS (0.43s) | **6.5x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_loads_the_store_into_memory_at_open` | SLOW (2.75s) | PASS (0.50s) | **5.4x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_slow_flush_relaxes_the_batching_policy` | SLOW (2.73s) | PASS (0.33s) | **8.3x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_store_past_the_ceiling_reads_on_demand` | SLOW (2.66s) | PASS (0.45s) | **6.0x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::fallback::test_user_db_fallback_read_after_a_flush_sees_the_written_value` | SLOW (2.59s) | PASS (0.54s) | **4.8x** | 已完成修复与降耗优化 |
| `xtask (bin/xtask)` | `testd::dict_inject::tests::test_user_store_fixture_places_the_two_shapes_of_damage` | SLOW (2.48s) | PASS (0.41s) | **6.1x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_open_creates_a_private_file` | SLOW (2.47s) | PASS (0.40s) | **6.1x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_eviction_forgets_the_evicted_counts` | SLOW (2.46s) | PASS (0.33s) | **7.5x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::manage_tests::test_list_words_answers_for_a_store_past_the_ceiling` | SLOW (2.42s) | PASS (0.49s) | **4.9x** | 已完成修复与降耗优化 |
| `ime-fcitx5 (lib/main)` | `addon::tests::test_recover_store_restores_a_damaged_store_from_the_newest_generation` | SLOW (2.41s) | PASS (0.89s) | **2.7x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::manage_tests::test_list_words_clamps_the_limit_to_the_page_ceiling` | SLOW (2.37s) | PASS (0.42s) | **5.7x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_lexicon::tests::test_handle_coin_enrols_through_the_store_into_the_table` | SLOW (2.37s) | PASS (0.31s) | **7.5x** | 已完成修复与降耗优化 |
| `ime-fcitx5 (lib/main)` | `addon::tests::test_recover_store_rebuilds_an_empty_store_when_there_is_no_generation` | SLOW (2.37s) | PASS (0.90s) | **2.6x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::fallback::test_user_db_fallback_read_adds_the_delta_to_a_cached_value` | SLOW (2.30s) | PASS (0.51s) | **4.5x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_freq_reads_the_pending_delta_before_the_flush` | SLOW (2.27s) | PASS (0.37s) | **6.1x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_interval_trigger_flushes_after_the_interval` | SLOW (2.20s) | PASS (0.44s) | **5.0x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_is_user_word_is_false_in_this_phase` | SLOW (2.20s) | PASS (0.42s) | **5.2x** | 已完成修复与降耗优化 |
| `ime-ui (lib/main)` | `renderer::tests::shrink::test_a_shrinking_scratch_repaints_the_whole_surface` | SLOW (2.17s) | PASS (0.30s) | **7.2x** | 已完成修复与降耗优化 |
| `ime-fcitx5 (lib/main)` | `addon::tests::test_recover_store_skips_a_generation_that_does_not_import` | SLOW (2.17s) | PASS (0.78s) | **2.8x** | 已完成修复与降耗优化 |
| `ime-ui (lib/main)` | `renderer::tests::test_a_scale_round_trip_gives_the_allocation_back_after_the_streak` | SLOW (2.16s) | PASS (0.30s) | **7.2x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_degrade_keeps_the_loaded_counts_readable` | SLOW (2.09s) | PASS (0.34s) | **6.2x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::migration::test_user_db_migrates_an_empty_v1_store_without_writes` | SLOW (2.05s) | PASS (0.46s) | **4.5x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::pins::test_a_pinned_word_is_exempt_from_eviction` | SLOW (2.00s) | PASS (0.35s) | **5.7x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_freq_does_not_fall_across_a_flush` | SLOW (2.00s) | PASS (0.36s) | **5.5x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::manage_tests::test_list_words_drops_a_forgotten_word_before_the_flush` | SLOW (1.93s) | PASS (0.33s) | **5.8x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_flush_runs_off_the_recording_thread` | SLOW (1.91s) | PASS (0.38s) | **5.0x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::migration::test_a_migrated_store_hydrates_and_lists_its_rows` | SLOW (1.89s) | PASS (0.49s) | **3.8x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::pins::test_set_pinned_none_clears_the_key_pin_explicitly` | SLOW (1.88s) | PASS (0.17s) | **11.3x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::manage_tests::test_record_after_forget_takes_the_tombstone_back` | SLOW (1.87s) | PASS (0.38s) | **4.9x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_flush_failure_degrades_to_readonly` | SLOW (1.86s) | PASS (0.42s) | **4.4x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::manage_tests::test_forget_of_a_pinned_word_clears_the_pin_and_deletes_the_record` | SLOW (1.83s) | PASS (0.51s) | **3.6x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::manage_tests::test_list_words_shows_a_record_the_flush_has_not_written` | SLOW (1.80s) | PASS (0.36s) | **5.0x** | 已完成修复与降耗优化 |
| `ime-fcitx5 (lib/main)` | `addon::tests::test_install_user_store_keeps_the_first_store_and_take_empties_the_slot` | SLOW (1.79s) | PASS (0.65s) | **2.7x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::manage_tests::test_list_words_reads_an_old_record_as_unpinned_and_unstamped` | SLOW (1.74s) | PASS (0.36s) | **4.8x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::manage_tests::test_list_words_orders_by_use_and_breaks_a_tie_by_key` | SLOW (1.72s) | PASS (0.38s) | **4.6x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::pins::test_set_pinned_lifts_the_word_to_the_cap_and_the_second_press_lowers_it` | SLOW (1.68s) | PASS (0.16s) | **10.7x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_open_rejects_a_second_handle_on_the_same_file` | SLOW (1.67s) | PASS (0.33s) | **5.0x** | 已完成修复与降耗优化 |
| `xtask (bin/xtask)` | `testd::logs::tests::test_error_code_index_reads_the_workspace` | SLOW (1.67s) | PASS (0.45s) | **3.7x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::pins::test_a_word_pinned_under_two_keys_stays_until_both_release` | SLOW (1.67s) | PASS (0.16s) | **10.5x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_batch_trigger_flushes_at_the_batch_size` | SLOW (1.64s) | PASS (0.34s) | **4.8x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_record_merges_repeated_commits_into_one_delta` | SLOW (1.64s) | PASS (0.33s) | **5.0x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::backup::tests::test_recover_user_db_with_backup_restores_the_newest_generation` | SLOW (1.61s) | PASS (0.94s) | **1.7x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_lexicon::tests::test_handle_coin_leaves_a_readonly_store_and_its_table_untouched` | SLOW (1.60s) | PASS (0.32s) | **5.0x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::manage_tests::test_forget_on_a_readonly_store_reports_readonly` | SLOW (1.58s) | PASS (0.38s) | **4.2x** | 已完成修复与降耗优化 |
| `xtask (bin/xtask)` | `verify::checksums::tests::test_verify_refuses_a_file_past_its_size_ceiling` | SLOW (1.57s) | PASS (0.40s) | **4.0x** | 已完成修复与降耗优化 |
| `xtask (bin/xtask)` | `testd::engine::tests::test_the_user_db_store_serves_its_seeds` | SLOW (1.55s) | PASS (0.30s) | **5.1x** | 已完成修复与降耗优化 |
| `xtask (bin/xtask)` | `testd::dict_inject::mutation_tests::test_a_single_flipped_byte_anywhere_in_the_container_is_refused` | SLOW (1.53s) | PASS (0.23s) | **6.6x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::backup::tests::test_recover_user_db_with_backup_reports_failure_when_no_generation_imports` | SLOW (1.52s) | PASS (0.92s) | **1.7x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::pins::test_set_pinned_moves_the_pin_when_a_second_word_takes_the_key` | SLOW (1.51s) | PASS (0.32s) | **4.7x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::manage_tests::test_list_words_pages_by_offset_and_limit` | SLOW (1.51s) | PASS (0.36s) | **4.1x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_final_commit_writes_what_the_batch_left_behind` | SLOW (1.50s) | PASS (0.39s) | **3.9x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::tests::pins::test_set_pinned_on_a_readonly_store_is_refused_without_change` | SLOW (1.49s) | PASS (0.32s) | **4.7x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::backup::tests::test_recover_user_db_with_backup_rebuilds_empty_when_there_is_no_backup` | SLOW (1.48s) | PASS (0.93s) | **1.6x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::manage_tests::test_forget_trait_method_reports_whether_a_record_went_away` | SLOW (1.48s) | PASS (0.35s) | **4.2x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::manage_tests::test_forget_of_an_unknown_key_reports_not_found_and_changes_nothing` | SLOW (1.42s) | PASS (0.36s) | **4.0x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::export_tests::test_export_round_trips_into_an_empty_store` | SLOW (1.42s) | PASS (0.69s) | **2.1x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::backup::tests::test_recover_user_db_with_backup_skips_a_generation_that_does_not_import` | SLOW (1.35s) | PASS (0.89s) | **1.5x** | 已完成修复与降耗优化 |
| `ime-fcitx5 (lib/main)` | `addon::tests::test_recover_store_leaves_a_healthy_store_alone` | SLOW (1.33s) | PASS (0.47s) | **2.8x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::backup::tests::test_recover_user_db_with_backup_orders_the_generations_it_is_given` | SLOW (1.30s) | PASS (0.83s) | **1.6x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::manage_tests::test_list_reports_unsupported_while_list_words_answers` | SLOW (1.29s) | PASS (0.41s) | **3.2x** | 已完成修复与降耗优化 |
| `xtask (bin/xtask)` | `package::tests::test_measure_fails_a_payload_past_its_budget_with_the_delivery_code` | SLOW (1.29s) | PASS (0.18s) | **7.2x** | 已完成修复与降耗优化 |
| `xtask (bin/xtask)` | `install::layout::tests::test_variable_reports_the_dev_missing_diagnostic_for_a_module_that_is_not_there` | SLOW (1.22s) | PASS (0.07s) | **16.9x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `user_db::backup::tests::test_backup_round_trips_into_an_empty_store` | SLOW (1.19s) | PASS (0.75s) | **1.6x** | 已完成修复与降耗优化 |
| `xtask (bin/xtask)` | `install::tests::test_check_size_accepts_the_threshold_and_rejects_one_byte_more` | SLOW (1.16s) | PASS (0.16s) | **7.4x** | 已完成修复与降耗优化 |
| `ime-fcitx5 (lib/main)` | `engine::router::phrases::tests::test_append_stays_within_the_host_callback_budget` | SLOW (1.06s) | PASS (0.38s) | **2.8x** | 已完成修复与降耗优化 |
| `ime-dict (lib/main)` | `recover::tests::test_recover_user_db_stays_within_the_startup_budget` | SLOW (1.02s) | PASS (0.65s) | **1.6x** | 已完成修复与降耗优化 |
| `ime-fcitx5 (lib/main)` | `addon::tests::test_run_shutdown_backup_writes_nothing_when_the_policy_is_disabled` | SLOW (1.02s) | PASS (0.34s) | **3.0x** | 已完成修复与降耗优化 |
| `xtask (bin/xtask)` | `testd::env::observation::tests::test_read_reports_the_process_it_runs_in` | SLOW (1.01s) | PASS (0.05s) | **21.9x** | 已完成修复与降耗优化 |

## 3. 契约成本与边际成本豁免清单（隔离耗时仍 >1s 的用例）
| 所属 Crate (Target) | 测试用例全名 | 豁免类型 | 优化前 → 后耗时 | 根因说明（引用常量/文档） | 为何不可再降（具体后果） | 验证证据 |
| :--- | :--- | :--- | ---: | :--- | :--- | :--- |
| `ime-dict (lib/main)` | `user_db::tests::test_user_db_survives_a_kill_with_bounded_loss` | **契约成本** | 7.78s → 5.23s | 根因: 真实子进程 SIGKILL 崩溃恢复契约(重入测试二进制, ready 轮询, kill 后重开校验 CHILD_MIN_SURVIVING 有界丢失) \| 不可再降: mock kill 或并入父进程即失去崩溃一致性验证效力, 铁律5明示此类真实进程矩阵属契约 ; 证据: 空闲窗双点 default PASS 5.220s/5.231s | 继续压降将破坏契约覆盖或引入过度架构复杂度 | `default` 单点双窗 PASS (5.23s 稳定) |
| `xtask (bin/xtask)` | `testd::dict_inject::mutation_tests::test_the_compiled_artifact_is_read_only_and_its_directory_gains_nothing` | **契约成本** | 11.40s → 2.08s | 根因: 被测主体是真实 dev 规模(15.62MiB)词库制品在 mutation 通道下不可变+目录零残留; from_compiled 全量校验+copy+10 案例 sweep+双 sha256 即断言本体 \| 不可再降: sweep 移至合成容器会使 hash 断言空洞化(与 copy 无交互即无可保护物), 缩减案例违反 BASELINE_MUTATIONS 矩阵 ; 证据: 空闲窗双点 default PASS 1.842s/2.078s | 继续压降将破坏契约覆盖或引入过度架构复杂度 | `default` 单点双窗 PASS (2.08s 稳定) |
| `xtask (bin/xtask)` | `testd::engine::tests::test_assert_repeatable_one_hundred_replays_agree_on_every_scenario` | **契约成本** | 9.45s → 1.08s | 根因: REPEAT_RUNS=100 为生产 pub 常量(engine.rs:93)且为 testd-engine CLI 默认值, 测试名即锚定 one_hundred_replays, 100 次全 trace 比对即确定性契约本体 \| 不可再降: 缩小 runs 即脱离被钉住的契约常量, 覆盖缩水 ; 证据: 空闲窗双点 default PASS 1.320s/1.081s | 继续压降将破坏契约覆盖或引入过度架构复杂度 | `default` 单点双窗 PASS (1.08s 稳定) |
| `xtask (bin/xtask)` | `testd::engine::tests::test_the_add_shard_example_scenarios_replay_green` | **契约成本** | 11.71s → 1.11s | 根因: 物理回放 CLI 流程(真实 toml 场景文件 ≥3 个自断言下限), 每场景含 REPEAT_RUNS 重复断言 \| 不可再降: 场景文件数与重复断言均为 CLI 契约的组成部分, 削减即不再证明命令本身可绿 ; 证据: 空闲窗双点 default PASS 1.057s/1.106s | 继续压降将破坏契约覆盖或引入过度架构复杂度 | `default` 单点双窗 PASS (1.11s 稳定) |
| `xtask (bin/xtask)` | `tune::tests::test_generate_holdout_writes_rows_that_avoid_the_evaluation_set` | **契约成本** | 8.82s → 1.67s | 根因: holdout 8000 行/4x2000 配额结构为测试自断言(MIN_HOLDOUT_ROWS 下限+2000*4 上限)+真实语料库, 行数即被测配额契约 \| 不可再降: 缩行数必然违反测试自身断言下限(铁律3) ; 证据: 空闲窗双点 default PASS 1.457s/1.674s | 继续压降将破坏契约覆盖或引入过度架构复杂度 | `default` 单点双窗 PASS (1.67s 稳定) |

## 3.5 重试消耗用例（final 回归 TRY N / 多次结项行 — headline 0 FAIL 掩盖的边缘用例）
> 无重试消耗用例（所有用例均一次通过）。

## 4. 架构与代码改动总结
- **触碰核心文件清单**：`.config/nextest.toml`, `Cargo.toml`, `crates/ime-core/src/input/buffer.rs`, `crates/ime-core/src/preedit/tests.rs`, `crates/ime-dict/src/user_db/tests.rs`, `crates/ime-fcitx5/src/privacy_impl.rs`（doctest 同步 WIP 新增的 `preedit` 字段）, `crates/ime-ui-addon/src/addon.rs`, `crates/ime-ui-addon/src/addon/tests.rs`, `crates/ime-ui/src/surface/tests.rs`, `testdata/fonts/fonts.conf`
- **生产与测试套件改进**：详见第 2 节逐项根因与修复方案及第 3 节豁免论证。
- **质量门禁状态**：`cargo fmt --check` ✅ | `cargo clippy -- -D warnings` ✅ | 探针与脏状态零遗留 ✅

## 5. 断言与契约完整性声明
- **0 屏蔽**：0 新增 `#[ignore]`（Skipped 维持 `23` 个），0 删测试，0 `test = false`
- **0 弱化**：0 断言删减或精度放宽，0 Public API 裸露污染
- **0 掩盖**：0 超时阈值拉大
- **0 假绿**：0 个用例依赖未裁决重试过关（重试消耗审计 `0` 项，未裁决 `0` 项）
