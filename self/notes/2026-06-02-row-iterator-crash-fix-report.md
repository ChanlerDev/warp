# RowIterator 越界崩溃修复报告

## 结论

Apple 崩溃报告和本地 `warp_preview.log.old.0` 指向同一消费者崩溃：

```text
RowIterator::next
-> FlatStorage::pop_rows
-> GridHandler::resize_storage
-> BlockList::resize
-> TerminalView::after_terminal_view_layout
```

Apple 报错为 `len is 117 but the index is 117`。本地未修复 main 对照测试稳定得到
`len is 5 but the index is 5`，失败位置同为 `row_iterator.rs:132`。

## 日志时序

本地日志显示用户执行 `clear` 前已经存在潜在坏 scrollback：

```text
Received Preexec hook
Received Clear hook
thread 'main' panicked at row_iterator.rs:132
```

`clear` 和随后 layout resize 是暴露器，不一定是最初 producer。

## 与 #10305 的关系

`c28fdddb` 已修复 CLI agent 运行中 resize 时未同步 `flat_storage.columns` 的明确
路径。Apple 使用的 WarpPreview 已包含该修复，因此不能只重复修一次相同分支。

仍然存在通用脆弱点：`FlatStorage::push_rows_internal` 无条件使用 unchecked
builder，默认上游 Row 永远满足列宽 invariant。一旦任何遗漏路径交付较宽 Row
或尾部非法宽字符，坏 index 会延迟到 resize/read 时触发进程级 panic。

## 修复

1. 正常 Row 继续使用 unchecked fast path。
2. 检测到列数不一致或尾部非法宽字符时，改用 checked builder reflow。
3. `RowIterator` 对遗留坏 index 增加越界保护，记录 warning 并降级渲染。

## Integration 覆盖

Integration 测试使用真实 OSC 777 `warp://cli-agent` session_start，保持同一个
foreground block 运行，并在 block 运行中执行 resize。随后输出 TUI 风格全屏
redraw、CJK、variation selector、insert/delete/erase 序列，结束 block，执行
shell `clear`，再循环 resize。

这覆盖正式产品链路和 Apple 日志中的暴露时序。当前未拿到 Apple 会话最初写入
坏 Row 的最短 public ANSI 序列，因此报告不把 `☁️` 或任一单独 ANSI 序列误报
为唯一根因。

## 证据边界

未修复 main 的 `FlatStorage` 对照测试可稳定触发与 Apple 报告相同的越界形态：

```text
index out of bounds: the len is 5 but the index is 5
crates/warp_terminal/src/model/grid/flat_storage/row_iterator.rs:132
```

尝试把 CLI-agent integration 用例移植到 pre-#10305 commit `1f72e823` 做 GUI
对照，但该历史版本在当前机器进入测试前即链接失败：

```text
Undefined symbols for architecture x86_64:
  "_configureAndRunModal"
```

因此不能声称产品级 integration 已在历史版本上观察到 panic。当前能严格确认的
范围是：单元测试稳定复现消费者崩溃，integration 覆盖真实 CLI-agent 暴露链路，
修复后两者均通过。

## 验证

```text
cargo test -p warp_terminal flat_storage -- --nocapture
# 33 passed, 2 ignored

cargo run -p integration --bin integration -- \
  test_row_iterator_panic_on_resize_with_cjk_scrollback
# passed

cargo test -p warp \
  test_full_grid_clear_resize_then_scroll_does_not_panic_on_row_iteration -- \
  --nocapture
# passed
```
