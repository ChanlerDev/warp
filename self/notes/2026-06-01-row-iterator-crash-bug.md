# RowIterator 越界崩溃 — 综合档

日期：2026-06-01
状态：panic 机制已锁定，产品入口未锁定，main 未修复
工作 worktree（保留）：`.claude/worktrees/rowiter-repro`

---

## 1. 现场

Apple crash report 栈：

```
RowIterator::next
  -> FlatStorage::pop_rows
    -> GridHandler::resize_storage
      -> GridHandler::resize
        -> TerminalModel::resize
          -> BlockList::resize
            -> TerminalView::resize_internal
              -> TerminalView::after_terminal_view_layout
```

panic 文本：`row_iterator.rs:132:20 index out of bounds: the len is 117 but the index is 117`。

发生序列推断：CLI Agent TUI（Claude / Codex / Starship）输出大量 CJK 或 emoji，scrollback 累积，clear / finish_background_block 后下一次完整 reflow resize 触发 `pop_rows`。

---

## 2. 已确认机制

`RowIterator::next`（`crates/warp_terminal/src/model/grid/flat_storage/row_iterator.rs:132`）在 `cell_width == 2` 分支无边界 guard，直接写 `row[idx + 1]`。

只要 flat_storage 中存在任意 Row，其末位 cell 带 `Flags::WIDE_CHAR` 而无后续 spacer 槽位，材料化必崩。

`Index::rebuild`（`FlatStorage::set_columns`）不清洗坏 Row。`process_grapheme_info_unchecked` 让坏 Row 通过 `push_rows_internal` 进入索引后保留。

`#10305` 修复（commit `c28fdddb`）只覆盖活动 CLI Agent TUI 的 Clear 早退分支中 `grid` 与 `flat_storage.columns` 不同步问题。Apple 栈走完整 reflow 路径，未被覆盖。

---

## 3. 已加单测结果

worktree 内 metal shim：`crates/warpui/build.rs` 探测 `xcrun metal --find` 失败时写空 metallib，仅本机非渲染单测，不入产品。

### 3.1 `crates/warp_terminal/src/model/grid/flat_storage/mod_tests.rs`

| 测试 | 行为 | main 结果 |
|---|---|---|
| `repro_wide_char_at_last_column_roundtrip` | 5 列 push `aaa中`，`rows_from(0)` round-trip | 通过 |
| `repro_wide_char_after_set_columns_shrink` | 7 列 push `abcde中`，`set_columns(6)` | 通过 |
| `repro_wide_char_after_pop_rows_117_columns` | 117 列 50 行 CJK，`set_columns(116)` | 通过 |
| `repro_corrupt_row_wide_char_at_last_cell_panics` | 手工构造末位 `WIDE_CHAR` 无 spacer，`push_rows` → `pop_rows(1)` | **PANIC `row_iterator.rs:132 index out of bounds: the len is 117 but the index is 117`** |
| `repro_corrupt_row_then_set_columns_then_pop_rows` | 同上加 `set_columns(cols)` 一次 | **PANIC，同位置同文本** |

后两测试 panic 文本与 Apple crash report 一字不差。

### 3.2 `app/src/terminal/model/grid/grid_handler_tests.rs`

| 测试 | 行为 | main 结果 |
|---|---|---|
| `repro_apple_emoji_selector_at_last_column` v1/v2/v3 | 4 列，input `a b c ☁ FE0F` 等 emoji variation | 通过；末位为 `WIDE_CHAR_SPACER`，未生成末位 `WIDE_CHAR` |
| `repro_resize_shrink_with_wide_char_at_new_last_column` | 6 列写 `aaaa中`，`resize(3,5)`→`resize(4,6)` | 通过 |
| `repro_stress_cjk_resize_loop` | 17 列写满 CJK + ASCII，30 轮列宽抖动 | 通过 |

### 3.3 结论

- panic 机制 100% 在 main 仍存在
- `Index::rebuild` 不清洗坏 Row
- 所有已知 ANSI 公共入口（emoji selector / resize / scroll / wide char wrap）单测都未在 main 上自然产出末位 `WIDE_CHAR` 的 Row
- 不能宣告 main 修好；任何上游 bug 让坏 Row 进 flat_storage 即炸

---

## 4. 待覆盖入口候选

下面路径都能直接写 `Flags::WIDE_CHAR` 到任意 cell，未被现有 `repro_*` 覆盖到末列：

- `app/src/terminal/model/grid/ansi_handler.rs:230` — emoji variation selector 在 wrap 边界与多行 selection 替换
- `app/src/terminal/model/grid/ansi_handler.rs:294` — wrap-disabled 模式下的 wide char
- `ansi_handler.rs` 中 `reset_wide_char_*` 边界函数：列宽变化或反向 ANSI 删除时是否留下 `WIDE_CHAR` 末列残留
- `flat_storage::push_rows_internal`（`mod.rs:251`）的 `process_grapheme_info_unchecked` 路径在源 Row 含 `LEADING_WIDE_CHAR_SPACER` 时的 grapheme_runs 状态
- `app/src/terminal/model/grid/grid_storage.rs::scroll_up` / `set_stored_rows` — 完整 reflow 时往 flat_storage push 的源 Row 是否可能短行 + 末位 wide
- 老 Preview build 写入 sentry / autosave 后被反序列化恢复的 Row 状态（`GridStorage::deserialize`）

---

## 5. 测试计划（按 CONTRIBUTING 偏好）

CONTRIBUTING.md 要求：bug fix 必带 regression test；user-facing flow 优先 `crates/integration/`；高质量覆盖 + presubmit + 手动测试。

5 层栈对应位置：

```
TerminalView::after_terminal_view_layout    ← integration / view test
TerminalView::resize_internal
TerminalModel::resize                        ← TerminalModel test
BlockList::resize                            ← BlockList test
GridHandler::resize                          ← grid_handler_tests.rs (已加, 未自然复现)
GridHandler::resize_storage
FlatStorage::pop_rows                        ← mod_tests.rs (已加, panic ✓)
RowIterator::next                            ← 单元层
```

### 5.1 优先级 P0：crates/integration 端到端

最贴近 Apple 现场，CONTRIBUTING 偏好。

文件：新建 `crates/integration/src/test/row_iterator_crash.rs`，注册到 `src/bin/integration.rs::register_tests` 与 `tests/integration.rs::integration_tests!`。

测试形态：

```rust
fn test_resize_with_cjk_scrollback_after_clear() -> TestDriver {
    new_builder()
        .with_step(wait_for_bootstrapping(0))
        .with_step(
            TestStep::new("Fill scrollback with wide chars at edge column")
                .with_input_string(
                    // 写 200 行，每行右边界落在双宽字符
                    &(0..200)
                        .map(|_| format!("{}\u{2601}\u{FE0F}\n", " ".repeat(115)))
                        .collect::<String>(),
                )
                .set_timeout(Duration::from_secs(10)),
        )
        .with_step(
            TestStep::new("Clear screen (finish_background_block)")
                .with_keystrokes(&[Keystroke::parse("ctrl-l").unwrap()]),
        )
        .with_step(
            TestStep::new("Resize narrower then wider, multiple times")
                .with_window_resize(/* 117 → 80 → 117 → 50 → 130 */),
        )
        .with_step(
            TestStep::new("App still alive")
                .set_assertion(/* terminal view present, no panic */),
        )
        .build()
}
```

依赖：需要 `with_window_resize` step（若不存在，新增到 `ui/src/integration/test_driver.rs`）。

### 5.2 优先级 P1：BlockList / TerminalModel 层

走 `clear_visible_screen → finish_background_block → 下次 resize 完整 reflow` Apple 现场链路。

文件：
- `app/src/terminal/model/blocks.rs` 同目录加 `blocks_tests.rs`（或在已有 tests mod）测 `clear_visible_screen` 后接 `resize`
- `app/src/terminal/model/terminal_model.rs` 配套 `terminal_model_tests.rs`

```rust
#[test]
fn clear_then_resize_with_cjk_scrollback() {
    let mut model = TerminalModel::new_for_test(/* 117 cols, 30 rows */);
    for _ in 0..200 {
        model.input(&format!("{}\u{2601}\u{FE0F}\n", " ".repeat(115)));
    }
    model.handle_clear_hook();
    model.resize(SizeUpdate::new_without_font_metrics(20, 117));
    model.resize(SizeUpdate::new_without_font_metrics(20, 80));
    // 不 panic = 通过；理想再 assert flat_storage 任意 Row 末位非 WIDE_CHAR
}
```

### 5.3 优先级 P2：物化层防御回归

已存在于 worktree；建议搬入产品。

文件：`crates/warp_terminal/src/model/grid/flat_storage/mod_tests.rs`
- `repro_corrupt_row_wide_char_at_last_cell_panics`
- `repro_corrupt_row_then_set_columns_then_pop_rows`

修复落地后这两个测试要变成 **不 panic** 而是 guard 触发 sentry log + skip。

### 5.4 presubmit

每层落地后跑 `./script/presubmit`。fmt + clippy + nextest 必过。

---

## 6. 手动复现

前提：Apple 栈复现概率低，需凑齐 wide char 末列 + reflow。

### 6.1 方案 A：ANSI 脚本 + 拖窗口

```bash
#!/bin/zsh
# repro-rowiter-crash.sh
# 调窗口宽度到 117 列。stty cols 117 仅 ssh / vanilla shell 生效。

# 1. 累积 scrollback：每行末位为 emoji presentation
for i in {1..500}; do
    printf '%116s☁️\n' ' '
done

# 2. 末列直接写 wide char
printf '\033[1;117H'   # cursor 行 1 列 117
printf '☁'        # ☁ 占 1 列
printf '\u{FE0F}'      # zero-width promote 到 wide

# 3. scroll up 把行进 scrollback
for i in {1..30}; do echo; done

# 4. clear 触发 finish_background_block
clear

# 5. 立即拖窗口宽度 117 → 80 → 130 → 50 来回
echo '现在拖窗口宽度，应崩'
```

跑：`zsh repro-rowiter-crash.sh`，然后手动拖窗口。窗口越窄越快、越奇数列越好。

### 6.2 方案 B：Starship + Preview 真实使用

最贴近原现场。

1. 装 Starship，启 GCloud / AWS chip（用 ☁️ ✈️ 等 emoji presentation）
2. 装 `WarpPreview.app`（不是 stable）
3. 新建 tab，窗口调窄到奇数列
4. 启动 Claude Code 或 Codex CLI，让 TUI 输出大量中文 / emoji
5. 反复显著拉宽拉窄窗口，重点奇数列
6. 退 TUI 回 shell：`printf '中文中文中文中文中文中文中文中文\n%.0s' {1..200}; clear`
7. 立即轻微 resize，或开关侧栏、分栏
8. 不复现就增加 tab、切换 pane、连续关闭两个其他 tab 后重复

### 6.3 录制要求

- 全程录屏
- 记录 Preview 版本、tab 数量、split 状态、最后 30 秒操作顺序
- 崩后保存 Apple crash report `~/Library/Logs/DiagnosticReports/Warp*.ips`

---

## 7. 给 codex 的下一步建议

1. 把 `repro_corrupt_row_*` 两测试搬入产品（去 metal shim），作为物化层防御回归
2. `push_rows_internal` 与 `Index::rebuild` 加 `debug_assert!`：source Row 末位若是 `WIDE_CHAR` 或 `WIDE_CHAR_SPACER` 落错位置，立刻 panic 并 dump grapheme runs
3. `RowIterator::next` 的 `cell_width == 2` 分支加边界 guard：超界时记 log + sentry，把 hard panic 降级为可观测脏数据
4. Apple 栈实际产品入口本地不可穷举，靠 sentry 抓真实序列
5. P0 integration 测试落地后跑 nightly，捕获回归
6. 跟踪未覆盖入口候选（§4），逐项加测试或 debug_assert 触发器

---

## 8. 当前结论

- 不能笼统表述为“main 没修这个 bug”。`#10305` 已修复 Clear 早退分支列宽同步，但本次 panic 走完整 reflow；当前仍缺少对坏 Row 的物化层防御，也尚未定位本次坏 Row 的实际生产入口
- `RowIterator::next` 仍无末列 wide char 防御；任何上游 bug 让坏 Row 进 flat_storage 都触发同 panic
- 性价比最高的产品级修复：`RowIterator` 加 guard + `debug_assert` 抓污染入口；不必等找到产品序列
- regression test 三层都要：integration（P0）、TerminalModel/BlockList（P1）、FlatStorage 物化层防御（P2）

## 9. 2026-06-02 Integration 实跑

在仓库根目录运行：

```bash
RUST_BACKTRACE=1 cargo run -p integration --bin integration -- \
  test_row_iterator_panic_on_resize_with_cjk_scrollback
```

结果：通过，`exit 0`。测试实际启动了隔离 HOME 的 integration app，执行 400 行 CJK/emoji 输出、`clear` 和多轮窗口宽度 resize，没有触发 `RowIterator` panic。

随后尝试：

```bash
cargo nextest run --no-fail-fast --workspace \
  test_row_iterator_panic_on_resize_with_cjk_scrollback
```

本机未安装 `cargo-nextest`，无法执行。改用标准 Rust harness：

```bash
RUST_BACKTRACE=1 cargo test -p integration \
  test_row_iterator_panic_on_resize_with_cjk_scrollback -- --nocapture
```

结果：通过，`1 passed; 0 failed; 292 filtered out`。说明测试在 `ui_tests.rs` 的 CI 注册路径有效。

注意：该 integration 测试仍是压力回归脚手架。它通过只能证明当前构造未触发崩溃，不能证明 latent bug 已修复。

## 10. 2026-06-02 本机日志时间线与复现分层

### 10.1 Preview 构建已包含 `#10305`

- Apple report：`2026-06-01 14:18:17.5332 +0800`
- Preview：`0.2026.05.27.15.44.01`
- 本机更新日志对应：`v0.2026.05.27.15.44.preview_01`
- 仓库较早的 `v0.2026.05.27.09.22.preview_00` tag 已包含 `c28fdddb`，因此同日更晚的 `15.44.preview_01` 也应包含该修复

结论：不能把 Apple crash 直接归因于“Preview 没带 `#10305`”。

### 10.2 `clear` 是触发读取，不一定是污染入口

轮转日志 `~/Library/Logs/warp_preview.log.old.0`：

```text
2026-06-01T06:18:16Z [INFO] Received Preexec hook
2026-06-01T06:18:16Z [INFO] Received Clear hook
2026-06-01T06:18:17Z [ERROR] thread 'main' panicked at
  'index out of bounds: the len is 117 but the index is 117':
  crates/warp_terminal/src/model/grid/flat_storage/row_iterator.rs:132
```

`Clear hook` 调用 `TerminalModel::clear_visible_screen`，改变 gap/layout；随后
`after_terminal_view_layout -> resize_internal -> TerminalModel::resize ->
BlockList::resize -> GridHandler::resize_storage -> FlatStorage::pop_rows`
物化已有历史行并 panic。

因此复现要拆成两段：

1. 先制造一个已写入 `FlatStorage` 的坏 Row；
2. 再执行 shell `clear`，利用 layout resize 读取坏 Row。

### 10.3 现有 integration 测试遗漏了关键状态

当前 integration 只运行普通 shell 输出、`clear` 和窗口 resize。它没有触发
`CLIAgentSessionsModelEvent::Started`，因此不会执行：

```rust
active_block.enable_full_grid_clear_behavior();
```

该测试通过不能覆盖 CLI-agent TUI 专用状态机，也不能证明 Apple crash 已修复。

### 10.4 新的优先诊断点

`FlatStorage::push_rows_internal` 使用
`EntryBuilder::process_grapheme_info_unchecked`。其前提是输入 `Row` 一定满足列宽
invariant，但目前没有运行时校验。下一步应在临时诊断构建中于写入端检查：

- `row.occ`、`row.len()`、`flat_storage.columns`
- 尾部 cell flags
- 尾部 grapheme 的 cell width
- 写入来自 `scroll_region_up` 还是 full reflow

这样能在坏 Row 第一次进入 `FlatStorage` 时抓到生产入口，而不是等 `clear` 二次触发时只看到消费者崩溃。

### 10.5 真实用户动作顺序

从轮转日志还原出的最后几分钟动作：

```text
14:14:05  window resized，连续两次
14:15:36  PaneGroupAction::ResizeMove([1405.7578, 421.52344])，连续两次
14:18:09  Warp 窗口重新激活
14:18:10  FocusPane，切换到一个 terminal pane
14:18:11  FocusPane，切换到另一个 terminal pane
14:18:13  WorkspaceAction::CloseTab(4)
14:18:15  WorkspaceAction::CloseTab(4)
14:18:16  EditorAction::Enter
14:18:16  Received Preexec hook
14:18:16  Received Clear hook
14:18:17  RowIterator::next panic
```

下一版 UI integration 应按此顺序构造：

1. 多 tab、多 pane；
2. 启动 Warp 可识别的 CLI agent session；
3. 产生包含宽字符的长输出和 scrollback；
4. 调整 pane 分隔条宽度；
5. 切换 pane，连续关闭相邻 tab；
6. 最后执行 `clear`，由自动 layout resize 触发 `FlatStorage::pop_rows`。

### 10.6 Integration 模拟 CLI agent 模式

推荐通过隔离 shell 输出真实 OSC 777 `session_start` 通知，不直接修改
`CLIAgentSessionsModel`。这样测试覆盖正式产品链路：

```text
PTY bytes
-> ANSI OSC 777 parser
-> PluggableNotification
-> TerminalView::handle_cli_agent_notification
-> CLIAgentSessionsModel::register_listener
-> CLIAgentSessionsModelEvent::Started
-> active_block.enable_full_grid_clear_behavior()
```

通知形状：

```text
ESC ] 777 ; notify ; warp://cli-agent ;
{"v":1,"agent":"claude","event":"session_start","session_id":"rowiter-repro","cwd":"/tmp","project":"rowiter-repro","plugin_version":"1.1.0"}
BEL
```

实现时增加一个 integration step：

1. 通过 `execute_command_for_single_terminal_in_tab` 执行 `printf` 注入 OSC 777；
2. 轮询断言目标 `TerminalView` 的 `CLIAgentSessionsModel` session 已存在；
3. 轮询断言 active block 的 `FullGridClearBehavior` 已切换为 `Clear`；
4. 再执行宽字符 scrollback、pane resize、切 pane、关闭 tab、`clear`。

不推荐只直接更新 singleton model：虽然更短，但会跳过 ANSI parser、PTY
事件转发和 `TerminalView` 订阅链路，容易得到一个形式上通过但产品路径未覆盖的测试。

### 10.7 确定性坏 Row 生产入口

继续沿 ANSI 输入链路检查后，发现 emoji variation selector 的生产路径可直接构造
非法 Row。`ansi_handler.rs` 在接收 `U+FE0F` 时，会把前一个窄字符提升为
`WIDE_CHAR`，并在当前 cursor 位置写入 `WIDE_CHAR_SPACER`。如果基础字符
`U+2601` 已经写在最后一列，提升后的宽字符没有合法 spacer 位置。

Integration 不再依赖随机 CJK 换行碰撞，而是使用 ANSI `CSI 999 C` 将 cursor
夹到最右列，再输出 `☁️` 和换行。命令先输出真实 OSC 777 CLI-agent
`session_start`，随后在同一个 foreground block 内循环输出边界 payload，确保
坏 Row 的生产覆盖 CLI-agent full-grid clear 模式。最后仍通过 shell `clear` 和
窗口 resize 触发 `FlatStorage::pop_rows`。
