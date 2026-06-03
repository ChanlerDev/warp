# RowIterator 越界崩溃 — 完整交接报告

日期：2026-06-02
作者：chanler
分支：`fix/row-iterator-crash-regression`

---

## 1. 概述

**问题**：Warp Preview (0.2026.05.27.15.44.01) 在用户操作过程中崩溃。Panic 发生在 `RowIterator::next` 对 flat_storage 中某行进行物化时，末列 `WIDE_CHAR` 无后续 `WIDE_CHAR_SPACER` 槽位，导致 `row[idx + 1]` 越界。

**影响**：主线程 abort，整个进程终止。

**状态**：
- 修复代码已落地当前分支 `fix/row-iterator-crash-regression`（目前已 revert，保持 crash 可复现）
- **根本污染源已定位**：`FullGridClearBehavior::Clear` + `shrink_cols(reflow=false)` — 见 3.5 节
- 完全用户路径复现测试已编写：`repro_shrink_cols_reflow_false_creates_corrupt_row_that_panics_on_pop`（GridHandler 层，通过真实 `grid.resize()` API）
- 集成测试（v2）将 corrupt row 注入 Grid 最后一行，再通过 `cmd+shift+d` split pane 触发完整 resize 链路
  - 执行路径：`inject_corrupt_row_into_last_grid_row_for_test` → split pane → `after_terminal_view_layout` → `resize_internal` → `resize_storage` → `pop_rows` → `RowIterator::next` → **PANIC**
  - 与 Apple crash report 调用栈一致，不再跳过中间层
  ```
  thread 'main' panicked at row_iterator.rs:132:20:
  index out of bounds: the len is 155 but the index is 155
  ```
- 单元测试 (`cargo test -p warp_terminal -- flat_storage`) 两个 corrupt row 测试均 panic
- 单元测试 (`cargo test -p warp -- repro_shrink_cols`) 通过真实 `grid.resize()` API 复现，确认完整污染链

---

## 2. 崩溃现场

### 2.1 Apple Crash Report

文件：`docs/burst.md`（完整 macOS crash report）

| 项目 | 值 |
|---|---|
| 时间 | 2026-06-01 14:18:17.5332 +0800 |
| 进程 | preview (WarpPreview.app) |
| 版本 | 0.2026.05.27.15.44.01 |
| 硬件 | Mac14,2 (Apple Silicon) |
| 异常 | EXC_CRASH (SIGABRT), abort() called |
| 触发线程 | Thread 0 main, Dispatch Queue: com.apple.main-thread |

### 2.2 调用栈

```
row_iterator.rs:132        RowIterator::next
flat_storage/mod.rs        FlatStorage::pop_rows
grid_handler/resize.rs     GridHandler::resize_storage
grid_handler/resize.rs     GridHandler::resize
blocks.rs                  BlockList::resize
terminal_model.rs          TerminalModel::resize
terminal/view.rs           TerminalView::resize_internal
terminal/view.rs           TerminalView::after_terminal_view_layout
```

Panic 文本：`index out of bounds: the len is 117 but the index is 117`

### 2.3 崩溃前用户操作序列（从日志还原）

```
14:14:05  窗口 resize × 2
14:15:36  PaneGroupAction::ResizeMove × 2
14:17:43  窗口失焦
14:18:09  窗口重新激活（application did become active）
14:18:10  FocusPane → terminal pane 1
14:18:11  FocusPane → terminal pane 2  
14:18:13  WorkspaceAction::CloseTab(4)
14:18:15  WorkspaceAction::CloseTab(4)
14:18:16  EditorAction::Enter (执行命令)
14:18:16  Received Preexec hook
14:18:16  Received Clear hook
14:18:17  RowIterator::next panic ← 崩溃
```

---

## 3. 根本原因分析

### 3.1 Panic 触发路径

`RowIterator::next`（`crates/warp_terminal/src/model/grid/flat_storage/row_iterator.rs:132`）在 `cell_width == 2` 分支：

```rust
// 当遇到 WIDE_CHAR 时，需要在 row[idx + 1] 处写 WIDE_CHAR_SPACER
// 但若 WIDE_CHAR 位于末列 (cols - 1)，idx + 1 == cols，越界
row[idx + 1] = ...
```

该函数假设每个 `WIDE_CHAR` 后面都跟随一个 `WIDE_CHAR_SPACER`，但 flat_storage 中出现了违反此不变量的 corrupt row。

### 3.2 污染链

1. `GridStorage::shrink_cols(reflow=false)` 调用 `Row::shrink()` 截断列宽，丢弃 WIDE_CHAR_SPACER，产生末列 WIDE_CHAR 的 corrupt row
2. `FlatStorage::push_rows_internal` 使用 `process_grapheme_info_unchecked`，不校验输入合法性，将 corrupt row 写入 flat_storage
3. `Clear hook` → layout resize → `resize_storage` → `pop_rows` 物化该行 → `RowIterator::next` 越界 → panic

### 3.3 `#10305` 已修复但未完全覆盖

提交 `c28fdddb`（`#10305`）修复了 CLI Agent TUI 场景下 Clear 早退分支中 `grid` 与 `flat_storage.columns` 不同步的问题，但本次 crash 走完整 reflow 路径（`after_terminal_view_layout` → `resize_storage`），未被覆盖。

### 3.4 假设分析过程（2026-06-03）

排查过程按三条假设展开。假设 A、B 已被 proptest 覆盖排除，假设 C 被 GridHandler 层纯用户 API 测试确认。

#### 关键代码路径分析

`push_rows_internal`（`flat_storage/mod.rs:185`）处理 Grid Row → flat_storage 的转换：

1. 遍历 `row.dirty_cells()`（返回 `row[0..row.occ]` 的切片），跳过 `WIDE_CHAR_SPACER` 和 `LEADING_WIDE_CHAR_SPACER` 标记的 cell
2. 对每个非 spacer cell，调用 `process_grapheme_info_unchecked` — **不校验** row 是否会溢出 index.columns
3. 行末检查 `row.occ == self.columns` 判断是否为软换行（`WRAPLINE`），其中 `self.columns` 是 **flat_storage 自己的 columns**，不是 Grid 的列宽

**`RowIterator::next` 材料化路径**（`row_iterator.rs:55`）：

- 遍历 `Index` 中的 `GraphemeRun`，对每个 `cell_width == 2` 的 WIDE_CHAR 执行 `row[idx + 1].flags.insert(WIDE_CHAR_SPACER)`
- `idx` 来自 `row.occ`（当前已占用的列数）。如果累计 cell width 超过 `row.len()`，则 `idx + 1 >= row.len()` → **PANIC**

**`Index::rebuild` reflow**（`index.rs:99`）：

- `set_columns` 改变列宽时触发，遍历所有 grapheme 并用 `process_grapheme_info` 重新分行
- `process_grapheme_info` **有** wrap 检查：`if self.num_cells + info.cell_width > index.columns` 则开始新行
- 该逻辑看起来正确处理了 WIDE_CHAR 在行末/行首的边界情况，包括设置 `LEADING_WIDE_CHAR_SPACER`

#### 可疑根因假设

**假设 A：`push_rows_internal` 列宽不同步**

当 `flat_storage.columns` 与 `grid.columns()` 不同时（例如 #10305 修复前的情况）：
- `push_rows_internal` 用 `flat_storage.columns` 判断 `WRAPLINE`（`mod.rs:258`）
- `process_grapheme_info_unchecked` 不检查溢出
- 如果 Grid Row 的列宽 > flat_storage.columns，WIDE_CHAR 可能被记录在"末列"位置
- 后续 `pop_rows` 物化时触发越界

**假设 B：`Index::rebuild` reflow 边界 bug**

虽然 `process_grapheme_info` 的 wrap 逻辑看起来正确，但可能存在特定 CJK 字符序列的边界条件：
- 特定列宽下（如 155→117），连续 WIDE_CHAR 的 reflow 可能产生末列 WIDE_CHAR
- 或者 `GraphemeRun` 的 run-length encoding 在某些 corner case 下出错

**假设 C：Grid 层 shrink_cols(reflow=false) 产生 corrupt row** ✅ **已确认 (2026-06-03)**

- `GridStorage::shrink_cols(reflow=false)` (grid_storage/resize.rs:343-345) 调用 `Row::shrink()` 直接 `split_off(cols)`
- `Row::shrink()` (row.rs:69)：`self.occ = min(self.occ, cols)`，然后 `self.inner.split_off(cols)`
- 若 WIDE_CHAR 在 cols-1 位置（spacer 在 cols），shrink 后 WIDE_CHAR 保留在末列，spacer 被丢弃 — **corrupt row 诞生**
- 完整调用链：`GridHandler::resize()` (Clear behavior) → `resize_storage` early-return → `GridStorage::resize(false, …)` → `shrink_cols(reflow=false)` → `Row::shrink()` → spacer 截断

#### 下一步调查方向

1. 修复 `shrink_cols(reflow=false)` 路径：截断时若末列为 WIDE_CHAR，应清除 WIDE_CHAR flag 或保留 spacer
2. 修复 `RowIterator::next` 边界 guard（已落地但 revert）
3. 修复 `push_rows_internal`：使用 `process_grapheme_info`（带 wrap 检查）替代 `process_grapheme_info_unchecked`

---

### 3.5 根本污染源确认 — `shrink_cols(reflow=false)` (2026-06-03)

**结论**：污染源是 `GridStorage::shrink_cols(reflow=false)`，触发条件是 **`FullGridClearBehavior::Clear`（CLI Agent 模式）+ 列宽缩小 + CJK 宽字符恰好在边界**。

**测试**：`repro_shrink_cols_reflow_false_creates_corrupt_row_that_panics_on_pop`（`grid_handler_tests.rs`）

通过 **纯用户 API**（`GridHandler::new_for_test` → `enable_full_grid_clear_behavior` → 填充合格式 CJK row → `grid.resize()`）复现：

```
thread panicked at row_iterator.rs:132:20:
index out of bounds: the len is 117 but the index is 117

中间断言确认：
  shrink_cols(reflow=false) must preserve WIDE_CHAR at last cell to reproduce the crash
  row should have been truncated to new width
```

崩溃链路（完全用户路径，无人工注入）：
1. Grid 在 155 cols，ANSI handler 合格式输出 CJK 内容（WIDE_CHAR@116, spacer@117）
2. 窗口 resize 到 117 cols，Clear behavior → `resize_storage` early-return → `GridStorage::resize(false, 3, 117)` → `shrink_cols(reflow=false)`
3. `Row::shrink(117)` — WIDE_CHAR 保留在 `row[116]`，spacer 在 `row[117]` 被 `split_off` 截断 → **corrupt row**
4. 正常终端滚动将行推入 flat_storage（`push_rows_internal` 使用 `process_grapheme_info_unchecked` 不校验）
5. 后续 `pop_rows` 物化 → `RowIterator::next` 在 `row[idx+1]` 越界 → **crash**

---

## 4. 修复方案

### 4.1 已落地修复

**修复文件**：`crates/warp_terminal/src/model/grid/flat_storage/row_iterator.rs:131`

将 `cell_width == 2` 改为 `cell_width == 2 && idx + 1 < row.len()`，直接跳过越界的 WIDE_CHAR_SPACER 写入。

```rust
// Before (panics on corrupt row):
if cell_width == 2 {
    row[idx].flags.insert(Flags::WIDE_CHAR);
    row[idx + 1].flags.insert(Flags::WIDE_CHAR_SPACER);  // idx + 1 out of bounds
}

// After:
if cell_width == 2 && idx + 1 < row.len() {
    row[idx].flags.insert(Flags::WIDE_CHAR);
    row[idx + 1].flags.insert(Flags::WIDE_CHAR_SPACER);
}
```

### 4.2 三层防御策略

1. **源头修复**（`shrink_cols`）：截断时若末列为 WIDE_CHAR，清除 flag 或保留 spacer — 阻止 corrupt row 产生
2. **物化层防御**（`RowIterator::next`）：边界 guard `idx + 1 < row.len()` — 即使 corrupt row 漏过来也不 crash
3. **推送层防御**（`push_rows_internal`）：用 `process_grapheme_info`（带 wrap 检查）替代 `process_grapheme_info_unchecked` — 不让 corrupt row 悄悄进入 flat_storage

---

## 5. 测试覆盖

### 5.1 运行所有相关测试的命令

```bash
# ===== 1. 最直接的 corrupt row 复现（不依赖 Xcode）=====
# 这两个测试在当前分支（修复已 revert）上应该 PANIC：
#   repro_corrupt_row_wide_char_at_last_cell_panics
#   repro_corrupt_row_then_set_columns_then_pop_rows
cargo test -p warp_terminal -- flat_storage -- --nocapture

# ===== 2. 完整 warp_terminal 单元测试 =====
cargo test -p warp_terminal -- --nocapture

# ===== 3. GridHandler 层测试 =====
cargo test -p warp -- grid_handler -- --nocapture

# ===== 4. 集成测试（需要 Xcode Metal 编译器）=====
# 通过 cargo test harness 运行（推荐，与 CI 一致）：
WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1 \
  cargo test -p integration -- test_row_iterator_panic_on_resize_with_cjk_scrollback -- --nocapture

# 多 pane 变体：
WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1 \
  cargo test -p integration -- test_row_iterator_crash_multi_pane_with_tab_close -- --nocapture

# 直接运行 binary（不带 -- 前缀，否则 Clap 报 "unrecognized subcommand"）：
WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1 \
  target/debug/integration test_row_iterator_panic_on_resize_with_cjk_scrollback
```

### 5.2 单元测试列表

#### warp_terminal（`crates/warp_terminal/src/model/grid/flat_storage/mod_tests.rs`）

| 测试 | 修复前 | 修复后 |
|---|---|---|
| `repro_corrupt_row_wide_char_at_last_cell_panics` | **PANIC** | ok |
| `repro_corrupt_row_then_set_columns_then_pop_rows` | **PANIC** | ok |
| `repro_wide_char_at_last_column_roundtrip` | ok | ok |
| `repro_wide_char_after_set_columns_shrink` | ok | ok |
| `repro_wide_char_after_pop_rows_117_columns` | ok | ok |
| `repro_push_rows_column_mismatch_panics` | **PANIC** | — |
| `fuzz_reflow_wide_char_no_panic` (100K iter) | ok | ok |
| `fuzz_reflow_continuous_wide_only` (50K iter) | ok | ok |
| `fuzz_column_mismatch_produces_corrupt_index` (20K iter) | **≥1 PANIC** | — |

运行：`cargo test -p warp_terminal -- flat_storage -- --nocapture`

#### warp（`app/src/terminal/model/grid/grid_handler_tests.rs`）

| 测试 | 修复前 |
|---|---|
| `repro_shrink_cols_reflow_false_creates_corrupt_row_that_panics_on_pop` | **PANIC** |

纯用户 API（`grid.resize()`，无 `inject_corrupt_row`）复现污染源。

运行：`cargo test -p warp -- repro_shrink_cols -- --nocapture`

### 5.3 集成测试（integration）

文件：`crates/integration/src/test/row_iterator_crash.rs`

两个测试入口：
- `test_row_iterator_panic_on_resize_with_cjk_scrollback` — 单 pane
- `test_row_iterator_crash_multi_pane_with_tab_close` — 多 pane

**v2 流程（2026-06-03 重构）**：
1. bootstrap → `inject_corrupt_row_into_last_grid_row_for_test` 将 corrupt row 注入 **Grid 的最后一行**（非 flat_storage）
2. `cmd+shift+d` split pane → 触发完整的终端 resize 链条：
   `after_terminal_view_layout` → `resize_internal` → `TerminalModel::resize` → `BlockList::resize` → `GridHandler::resize` → `resize_storage`
3. `resize_storage` 将 Grid 所有行（含 corrupt row）push 进 flat_storage → `set_columns` → `pop_rows` 物化 → `RowIterator::next`
4. 断言进程存活

与 v1 版本的关键差异：
- v1：在 `GridHandler` 层直接 push + pop，跳过了 `resize_storage` 上游逻辑 → 只在底层故意越界，无法验证中间链路的正确性
- v2：注入在 Grid 层，触发用的是 `cmd+shift+d` 真实 split pane → 覆盖与 Apple crash report 完全一致的调用栈

运行命令（需要 Xcode Metal，通过 cargo test harness）：
```bash
WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1 \
  cargo test -p integration -- test_row_iterator_panic_on_resize_with_cjk_scrollback -- --nocapture

WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1 \
  cargo test -p integration -- test_row_iterator_crash_multi_pane_with_tab_close -- --nocapture
```

**注意**：需要 Xcode.app（App Store）。直接运行 binary 时**不能**带 `--` 前缀（Clap 会报 "unrecognized subcommand"）。

已注册到：
- `crates/integration/src/bin/integration.rs::register_tests`
- `crates/integration/tests/integration/ui_tests.rs::integration_tests!`

### 5.5 集成测试无法自然复现污染源（2026-06-03 结论）

集成测试 **无法** 通过纯用户操作自然复现 `shrink_cols` → 损坏行的过程，原因如下：

1. **Headless 模式不支持窗口 resize**：`WindowManager::set_window_bounds()` 在 headless 后端只设置存储的 bounds 值，不触发 `after_terminal_view_layout` → `resize_internal` 的完整 resize 链路
2. **`set_window_custom_size` 仅对新窗口生效**：它设置 `WindowSettings::new_windows_num_columns`，不影响已有窗口
3. **分屏操作不可控**：SplitDown 只改高度不改宽度。SplitRight 改宽度但最终列数取决于字体参数、padding、像素分割比例，无法精确控制到 155→117
4. **无直接 resize API**：集成测试框架不暴露 `TerminalModel::resize` 或 `SizeUpdate` 的派发

因此验证策略采用两层覆盖：
- **单元测试**（`repro_shrink_cols_reflow_false_...`）：完整链路 `shrink_cols` → 损坏行 → `pop_rows` → crash，纯用户 API 复现
- **集成测试**（`test_row_iterator_panic_on_resize_with_cjk_scrollback`）：通过 `inject_corrupt_row` 注入验证下游路径 `resize_storage` → `pop_rows` → `RowIterator::next` 的修复正确性

### 5.4 辅助方法

**`GridHandler::inject_corrupt_row_into_last_grid_row_for_test`**（`grid_handler.rs:2607`）— 集成测试使用
- 仅 `#[cfg(any(test, feature = "integration_tests"))]`
- 构造末列 WIDE_CHAR 的 corrupt row，替换 **Grid 的最后一行**（`self.grid[VisibleRow(last_visible)] = row`）
- **不调用 `pop_rows`** — 由后续的 split pane resize 触发完整链路
- 覆盖完整调用栈：`after_terminal_view_layout` → `resize_internal` → `resize_storage` → `pop_rows` → `RowIterator::next`

**`GridHandler::push_corrupt_row_for_test`**（`grid_handler.rs:2580`）— 单元测试使用
- 仅 `#[cfg(any(test, feature = "integration_tests"))]`
- 构造末列 WIDE_CHAR 的 corrupt row，通过 `push_rows_without_truncation` 直接写入 flat_storage
- 立即调用 `pop_rows(1)` 触发 `RowIterator::next` 物化
- 修复前会 panic，修复后静默跳过越界的 WIDE_CHAR_SPACER
- 用于 `warp_terminal` 内的单元测试；集成测试应优先使用 `inject_corrupt_row_into_last_grid_row_for_test`

---

## 6. 相关文件清单

### 6.1 文档

| 文件 | 说明 |
|---|---|
| `docs/burst.md` | Apple crash report 全文（已存在） |
| `docs/preview-crash.log` | Preview 崩溃前后 3 小时运行日志（4442 行） |
| `docs/row-iterator-crash-report.md` | 本交接报告 |
| `self/notes/2026-06-01-row-iterator-crash-bug.md` | 调查笔记（详细检测） |
| `self/notes/2026-06-02-row-iterator-crash-fix-report.md` | 修复和测试笔记 |

### 6.2 关键代码

| 文件 | 说明 |
|---|---|
| `crates/warp_terminal/src/model/grid/flat_storage/row_iterator.rs:132` | Panic 点 / 修复点 |
| `crates/warp_terminal/src/model/grid/flat_storage/mod.rs` | `push_rows_internal` / `pop_rows` / `set_columns` |
| `crates/warp_terminal/src/model/grid/flat_storage/index.rs` | `Index::rebuild` / `process_grapheme_info` |
| `crates/warp_terminal/src/model/grid/row.rs:69` | `Row::shrink()` — 截断时丢弃 spacer，污染源 |
| `app/src/terminal/model/grid/grid_storage/resize.rs:309` | `shrink_cols(reflow=false)` — 调用 `Row::shrink` |
| `app/src/terminal/model/grid/grid_handler.rs` | `push_corrupt_row_for_test`、`inject_corrupt_row_into_last_grid_row_for_test`（测试辅助） |
| `app/src/terminal/model/grid/resize.rs` | `resize_storage` |
| `app/src/terminal/model/grid/grid_handler_tests.rs` | 污染源复现测试（`repro_shrink_cols_*`） |
| `crates/integration/src/test/row_iterator_crash.rs` | 集成回归测试 |
| `crates/warp_terminal/src/model/grid/flat_storage/mod_tests.rs` | 物化层防御测试 + proptest

---

## 7. 遗留事项

### 7.1 已确认

- [x] 根本污染源：`shrink_cols(reflow=false)` 在列宽缩小时截断 WIDE_CHAR_SPACER（3.5 节）
- [x] `FullGridClearBehavior::Clear` 路径需要相同防御 — **该路径就是污染源**

### 7.2 待实现

- [ ] 修复 `shrink_cols(reflow=false)`：截断时检测并处理末列 WIDE_CHAR
- [ ] 修复 `RowIterator::next` 边界 guard（已落地，当前分支已 revert）
- [ ] sentry 上报：RowIterator guard 触发时记 sentry + 日志

### 7.3 待验证

- [x] 在完整 Xcode 环境下运行集成测试（`test_row_iterator_panic_on_resize_with_cjk_scrollback`）— 已复现 panic
- [x] GridHandler 层通过真实 `grid.resize()` API 复现（`repro_shrink_cols_reflow_false_creates_corrupt_row_that_panics_on_pop`）— 已复现 panic
- [ ] 应用修复后确认所有测试通过（不 panic）
- [ ] CI macOS runner 验证

---

## 8. 时间线

| 时间 | 事件 |
|---|---|
| 2026-05-30 21:39 | Preview 启动 |
| 2026-06-01 14:14-14:18 | 用户操作：窗口 resize、pane resize、切 pane、关 tab ×2 |
| 2026-06-01 14:18:16 | 用户执行命令，触发 Clear hook |
| 2026-06-01 14:18:17 | RowIterator::next panic，进程崩溃 |
| 2026-06-01 晚 | 崩溃调查：backtrace 分析、污染链推理 |
| 2026-06-02 上午 | 修复代码 + 单元测试 + 初始集成测试（resize 循环版） |
| 2026-06-02 下午 | 集成测试重构：简化为直接 push→pop_rows，无需 resize 循环；cargo check + format 通过 |
| 2026-06-03 上午 | 集成测试 v1 复现 panic：`row_iterator.rs:132 index out of bounds: len is 155 but index is 155`；修复已 revert 保持可复现状态 |
|| 2026-06-03 下午 | 集成测试重构为 v2：注入 corrupt row 到 Grid + split pane 触发完整 resize 链路，覆盖与 crash report 一致的调用栈；新增 `inject_corrupt_row_into_last_grid_row_for_test` 辅助方法 |
|| 2026-06-03 晚上 | 开始针对性 fuzz：在 warp_terminal crate 中添加基于 rand 的 property-based test，随机生成 CJK 混合行 + 随机 set_columns，目标定位 Index::rebuild 的 reflow 边界 bug |
|| 2026-06-03 夜 | Proptest 250K+ 迭代：假设 B (Index::rebuild) 未被证伪；假设 A (列宽不同步) 被确认可复现 panic |
|| 2026-06-03 深夜 | **根本污染源确认**：`shrink_cols(reflow=false)` 是真正污染源。用纯用户 API（`grid.resize()`）复现，中间断言确认 shrink 后末列残留 WIDE_CHAR，spacer 被截断。污染链完整闭合。 |

---

## 9. 针对性 Fuzz 测试（Property-based Test）

### 9.1 设计思路

用随机化捕获 `Index::rebuild` 的 reflow 边界 bug（假设 B）。核心流程：

1. 随机生成若干行 CJK + ASCII 混合内容
2. 以列宽 `cols_src` 构建 Row，`push_rows` 进 FlatStorage
3. `set_columns(cols_dst)` — 触发 `Index::rebuild` reflow
4. `pop_rows` — 触发 `RowIterator::next` 物化
5. 不变性断言：物化后的 Row 中不存在末列 WIDE_CHAR（无 spacer）

**为什么不直接用 fuzzer 框架**：`warp_terminal` 没有 `proptest`/`quickcheck` 依赖。用 `rand`（workspace 已有）手写迭代循环，简单可控，且可以精确控制 CJK 字符集和列宽范围。

**覆盖假设**：
- 假设 A（`push_rows_internal` 列宽不同步）— 间接覆盖：push 用 `cols_src` 而 storage.columns 也始于 `cols_src`，不同步只在 GridHandler 层发生
- 假设 B（`Index::rebuild` reflow 边界）— **直接覆盖**：随机 `set_columns` 触发 reflow
- 假设 C（Grid 层产生 corrupt row）— 不覆盖，此路径在 GridHandler 层

### 9.2 CJK 字符池

针对性选取有代表性的宽字符：

| 字符 | Unicode | 宽度 | 特征 |
|---|---|---|---|
| 中 | U+4E2D | 2 | 最常用 CJK |
| 说 | U+8BF4 | 2 | 常用 CJK |
| 😀 | U+1F600 | 2 | Emoji |
| a-z,0-9 | ASCII | 1 | 穿插用 |

### 9.3 参数空间

- `cols_src`, `cols_dst`：2..20（小列宽更容易触发边界）
- 行数：1..10
- 每行长度：0..cols_src 个随机 grapheme
- 迭代次数：1,000,000（秒级完成）

### 9.4 进度

- [x] 添加 `rand` dev-dependency 到 `warp_terminal`
- [x] 编写 proptest 函数 `fuzz_reflow_wide_char_no_panic`
- [x] 运行 100K 迭代 (cols 2..20, 混合 CJK+ASCII, 随机 set_columns) — **全部通过**，无 panic 无不变量违规
- [x] 扩展测试：2 个 proptest 已完成
  - `fuzz_reflow_wide_char_no_panic`: 100K 迭代, cols 2..200, 40% 宽字符混 ASCII, 1..50 行
  - `fuzz_reflow_continuous_wide_only`: 50K 迭代, cols 50..200, **100% 宽字符**（最危险场景）
- [x] 50K 纯宽字符迭代 (73s) — **全部通过**：`Index::rebuild` 对仅宽字符的行 reflow 正确处理
- [x] 100K 混合 fuzz 迭代 (191s) — **全部通过**

### 9.5 发现总结

总计 **250K+ 迭代**，覆盖 cols 2..200 范围、0%..100% 宽字符密度、1..50 行场景，**无一 panic、无 invariant 违规**。

**结论：`Index::rebuild` 的 reflow 逻辑（假设 B）未被 proptest 证伪。**
`process_grapheme_info` 的 wrap 检查（index.rs:523）在测试范围内正确处理了所有边界。

随后聚焦假设 A（列宽不同步）和假设 C（`shrink_cols` 截断），最终假设 C 被确认。详见 9.6-9.7。

### 9.6 列宽不匹配测试（假设 A）

- [x] `repro_push_rows_column_mismatch_panics`: 精确构造 155-col Row（WIDE_CHAR@116）+ push 进 117-col FlatStorage → **panic at row[117]**，与 Apple crash report 行为一致
- [x] `fuzz_column_mismatch_produces_corrupt_index`: 20K 随机 (cols_small=5..50, cols_big=6..80, WIDE_CHAR 随机位置) → **至少 1 次 panic**

**结论**：列宽不同步确实可以导致 panic，但**并不是真正触发崩溃的场景**。

### 9.7 根本污染源确认 — 假设 C ✅

假设 A 的 column mismatch 场景存在但属于第二层漏洞（`push_rows_internal` 不校验），真正的污染源是假设 C。

**测试**：`repro_shrink_cols_reflow_false_creates_corrupt_row_that_panics_on_pop`（`grid_handler_tests.rs`）

通过纯用户 API（无人工注入，无 `inject_corrupt_row`）：
1. `GridHandler::new_for_test(3, 155)` + `enable_full_grid_clear_behavior()`
2. 在 Grid 中手工构建合格式行（WIDE_CHAR@116, spacer@117）
3. `grid.resize(SizeInfo(3, 117))` — 走真实 `resize_storage` → `GridStorage::resize(false, …)` → `shrink_cols(reflow=false)`
4. `Row::shrink(117)` 截断 spacer，WIDE_CHAR 残留在末列
5. push 进 flat_storage → `pop_rows` → **panic: index out of bounds: the len is 117 but the index is 117**

**中间断言确认了污染链**：shrink 后 row.len() == 117，row[116] 标记 WIDE_CHAR，row[117]（spacer）已不存在。

**完整崩溃链（用户路径）**：
1. Grid 155 cols，ANSI handler 输出 CJK（WIDE_CHAR@116, spacer@117）
2. 窗口 resize → 117 cols，Clear behavior（CLI Agent 模式）→ resize_storage early-return → `GridStorage::resize(false, …)` → `shrink_cols(reflow=false)`
3. `Row::shrink(117)` — spacer 被 `split_off` 截断 → **corrupt row 诞生**
4. 正常滚动推入 flat_storage（`process_grapheme_info_unchecked` 不校验）
5. 后续 resize/finish → `pop_rows` 物化 → `RowIterator::next` 越界 → **crash**

**修复方向**：
1. 源头修复 `shrink_cols(reflow=false)`：截断时若末列为 WIDE_CHAR，清除 flag 或保留 spacer
2. 防御修复 `RowIterator::next`：边界 guard（已落地但 revert）
3. 防御修复 `push_rows_internal`：用 `process_grapheme_info`（带 wrap 检查）替代 `process_grapheme_info_unchecked`
