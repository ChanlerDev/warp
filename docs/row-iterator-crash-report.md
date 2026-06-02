# RowIterator 越界崩溃 — 完整交接报告

日期：2026-06-02
作者：chanler
分支：`fix/row-iterator-crash-regression`

---

## 1. 概述

**问题**：Warp Preview (0.2026.05.27.15.44.01) 在用户操作过程中崩溃。Panic 发生在 `RowIterator::next` 对 flat_storage 中某行进行物化时，末列 `WIDE_CHAR` 无后续 `WIDE_CHAR_SPACER` 槽位，导致 `row[idx + 1]` 越界。

**影响**：主线程 abort，整个进程终止。

**状态**：
- 修复代码已落地当前分支 `fix/row-iterator-crash-regression`
- 集成回归测试已简化：`push_corrupt_row_for_test` 直接 push → pop_rows，无需 resize 循环
- 修复和测试均通过 `cargo check`，format 已跑
- 产品入口的根本污染源尚未定位

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

1. 某上游路径将末列为 `WIDE_CHAR` 的 Row 写入 `flat_storage`
2. `FlatStorage::push_rows_internal` 使用 `process_grapheme_info_unchecked`，不校验输入合法性
3. `Index::rebuild`（`set_columns` 时触发）不清洗非法 Row
4. `Clear hook` → layout resize → `resize_storage` → `pop_rows` 物化该行 → panic

### 3.3 `#10305` 已修复但未完全覆盖

提交 `c28fdddb`（`#10305`）修复了 CLI Agent TUI 场景下 Clear 早退分支中 `grid` 与 `flat_storage.columns` 不同步的问题，但本次 crash 走完整 reflow 路径（`after_terminal_view_layout` → `resize_storage`），未被覆盖。

### 3.4 未定位的污染源

单元测试尝试了所有已知 ANSI 入口（emoji variation selector、resize、宽字符 wrap、scroll 等），均未在 main 上自然产出末列 `WIDE_CHAR` 的行。所以根本污染源尚未确定，但需防御。

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

### 4.2 防御层

遵循"防御性编程"原则：
1. RowIterator 物化层：边界 guard（已修）
2. `push_rows_internal`：`debug_assert` 捕捉污染源（待加）
3. `Index::rebuild`：不清洗 corrupt row 的问题（待跟进）

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
# 先在终端运行 xcrun -sdk macosx --find metal 确认可用
# 然后重新构建：
cargo build --bin integration

# 运行单个集成测试（-- 前缀必须）：
WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1 \
  target/debug/integration -- test_row_iterator_panic_on_resize_with_cjk_scrollback

# 运行多 pane 变体：
WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1 \
  target/debug/integration -- test_row_iterator_crash_multi_pane_with_tab_close

# 通过 cargo test harness 运行（CI 用）：
WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1 \
  cargo test -p integration -- test_row_iterator_panic_on_resize_with_cjk_scrollback -- --nocapture
```

### 5.2 单元测试列表（warp_terminal）

文件：`crates/warp_terminal/src/model/grid/flat_storage/mod_tests.rs`

| 测试 | 修复前（当前分支） | 修复后 |
|---|---|---|
| `repro_corrupt_row_wide_char_at_last_cell_panics` | **PANIC** | ok |
| `repro_corrupt_row_then_set_columns_then_pop_rows` | **PANIC** | ok |
| `repro_wide_char_at_last_column_roundtrip` | ok | ok |
| `repro_wide_char_after_set_columns_shrink` | ok | ok |
| `repro_wide_char_after_pop_rows_117_columns` | ok | ok |

运行命令：`cargo test -p warp_terminal -- flat_storage -- --nocapture`

### 5.3 集成测试（integration）

文件：`crates/integration/src/test/row_iterator_crash.rs`

两个测试入口：
- `test_row_iterator_panic_on_resize_with_cjk_scrollback` — 单 pane
- `test_row_iterator_crash_multi_pane_with_tab_close` — 多 pane

流程：bootstrap → `push_corrupt_row_for_test` 注入 corrupt row 到 flat_storage 并立即 `pop_rows` 触发 `RowIterator::next` → 断言进程存活。

与之前版本的差异：不再依赖 resize 循环来间接触发 pop_rows。`push_corrupt_row_for_test` 直接走 `push_rows_without_truncation` + `pop_rows(1)`，确定性触发崩溃路径。

已注册到：
- `crates/integration/src/bin/integration.rs::register_tests`
- `crates/integration/tests/integration/ui_tests.rs::integration_tests!`

运行命令（需要 Xcode Metal）：
```bash
WARPUI_USE_REAL_DISPLAY_IN_INTEGRATION_TESTS=1 \
  target/debug/integration -- test_row_iterator_panic_on_resize_with_cjk_scrollback
```

**注意**：需要 Xcode.app（App Store），运行前先 `xcrun -sdk macosx --find metal` 确认 Metal 编译器可用。没有 Xcode 时 `build.rs` 生成空 metallib，渲染初始化即崩溃。

### 5.4 辅助方法

`GridHandler::push_corrupt_row_for_test`（`grid_handler.rs:2575`）
- 仅 `#[cfg(any(test, feature = "integration_tests"))]`
- 构造末列 WIDE_CHAR 的 corrupt row，通过 `push_rows_without_truncation` 直接写入 flat_storage
- 立即调用 `pop_rows(1)` 触发 `RowIterator::next` 物化
- 修复前会 panic，修复后静默跳过越界的 WIDE_CHAR_SPACER

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
| `crates/warp_terminal/src/model/grid/flat_storage/mod.rs` | `push_rows_internal` / `pop_rows` / `Index::rebuild` |
| `app/src/terminal/model/grid/grid_handler.rs` | `push_corrupt_row_for_test`（测试辅助） |
| `app/src/terminal/model/grid/resize.rs` | `resize_storage` |
| `crates/integration/src/test/row_iterator_crash.rs` | 集成回归测试 |
| `crates/warp_terminal/src/model/grid/flat_storage/mod_tests.rs` | 物化层防御测试 |

---

## 7. 遗留事项

### 7.1 待确认

- [ ] 根本污染源（哪个上游路径产生了末列 WIDE_CHAR 的 Row）
- [ ] `FullGridClearBehavior::Clear` 路径是否也需要相同防御（CLI Agent TUI 场景）
- [ ] `Index::rebuild` 是否能主动清洗 corrupt row

### 7.2 待实现

- [ ] `push_rows_internal` 加 `debug_assert` 探测污染源
- [ ] `Index::rebuild` 清洗逻辑
- [ ] sentry 上报：RowIterator guard 触发时记 sentry + 日志

### 7.3 待验证

- [ ] 在完整 Xcode 环境下运行集成测试（`test_row_iterator_panic_on_resize_with_cjk_scrollback`）
- [ ] 确认修复后该测试通过（不 panic）
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
