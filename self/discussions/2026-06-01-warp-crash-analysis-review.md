# Warp 崩溃分析初审

日期：2026-06-01

## 用户目标

评审 Claude Code 针对 Warp 崩溃的分析，判断哪些结论可靠、哪些仍需补证。当前仅做分析，不修改产品代码。

## 输入材料

- 用户提供的 Claude Code 分析文本：`/Users/chanler/.codex/attachments/f72a5e77-bae9-4be5-8845-1ddcd73e39af/pasted-text.txt`

## 当前可确认

- `RowIterator::next` 在处理双宽字符时直接写入 `row[idx + 1]`。如果索引记录与实际行宽失配，确实可能在末列越界。
- `BlockList::clear_visible_screen()` 会先调用 `finish_background_block()`。
- `GridHandler::resize_storage()` 对活动中的 `FullGridClearBehavior::Clear` 使用早退 resize 路径。
- 当前 checkout 已在该早退路径中同步调用 `flat_storage.set_columns(num_cols)`，注释明确说明旧行为会让 grid 与 flat storage 列数失配，随后由 unchecked push 污染索引并导致 `RowIterator::next` panic。
- 当前 checkout 已包含对应回归测试，覆盖变宽、变窄和字符串物化路径。

## 需要补证或降级为推测

- “clear 后 `finished = true`，下一次 resize 进入完整 reflow 并直接触发 panic”与当前代码中已有修复注释指向的根因不完全一致。更稳妥的说法是：`FullGridClearBehavior::Clear` 早退 resize 曾未同步 flat storage 列宽，之后滚动或物化旧行时可能触发 panic。
- 多 tab 切换、连续关 tab、`clear` hook 是现场事件，但是否为必要触发条件尚未证明。
- “117 列末尾中文宽字符”与越界形态一致，但需要原始 panic 日志中的 grapheme runs、调用栈和复现用例进一步确认。
- “41 小时 scrollback”可能提高命中概率，不应视为根因。

## 下一步建议

- 获取原始 crash 日志和对应版本 commit，核对 panic 文本、完整栈和 `resize.rs` 当时实现。
- 运行当前回归测试，确认本地 checkout 的修复有效。
- 如果需要复盘用户现场，再补一个包含 CJK 双宽字符、Clear 行为、resize 和 scroll 的最小回归测试。

## Apple Crash Report 补充核验

用户指出原始 Apple crash report 已保存为 `docs/burst.md`。核验后需要纠正初审表述：

- 崩溃发生于 `2026-06-01 14:18:17 +0800`，运行版本为 Warp Preview `0.2026.05.27.15.44.01`。
- 对应 Preview release 分支已经包含 `c28fdddb`（`Fix RowIterator crashes for third-party agents. (#10305)`）。
- Apple 报告中的崩溃栈精确命中：
  `RowIterator::next -> FlatStorage::pop_rows -> GridHandler::resize_storage -> GridHandler::resize -> BlockList::resize -> TerminalModel::resize -> TerminalView::resize_internal -> after_terminal_view_layout`。

因此不能得出“当前问题已经修复”的结论。更准确的判断是：

- `#10305` 修复了 CLI Agent TUI 活动状态早退 resize 分支未同步 `flat_storage.columns` 的已知问题。
- 用户这次崩溃发生在包含该修复的构建中，说明仍有未覆盖变体：可能是历史上已经污染的 flat storage 在后续完整 reflow 中暴露，也可能存在另一个产生非法 index entry 的路径。
- Claude Code 给出的 `clear -> finished -> 完整 reflow -> pop_rows -> RowIterator` 现场链条与 Apple 栈相容，值得作为优先复现方向；但 Apple crash report 本身无法证明 `clear` 是必要条件。

另外发现两个 `2026-05-28` 的远端实验分支提交：

- `7b6a34e5 perf: skip flat_storage round-trip during rows-only terminal resize`
- `90a37cfc perf: skip resize_storage for finished blocks on height-only changes`

它们会减少进入相关物化路径的机会，但提交说明针对 Sentry 内存峰值，不应直接视为本次越界的完整修复。

## Preview 构建归属核验

- 本机 `/Applications/WarpPreview.app/Contents/Info.plist` 中的 `WarpVersion` 为
  `v0.2026.05.27.15.44.preview_01`。
- `WarpPreview.app` 表示 Preview 渠道构建，不等价于直接运行当前 `main`。
- 远端当前未保留该临时 Preview 标签，因此无法仅通过现有 git refs 将该二进制精确映射到 commit。
- 仓库保留的更早 `v0.2026.05.27.09.22.preview_00` release tag 已包含 `c28fdddb`。因此同日更晚的
  `15.44.preview_01` 极大概率也包含该修复，除非它来自特殊回退或非标准构建流程。
- 即使包含 `c28fdddb`，本次 crash 仍可能发生：该修复只处理活动 CLI Agent TUI 的早退 resize
  分支列宽同步；Apple 栈显示本次 panic 发生于完整 reflow 的 `FlatStorage::pop_rows` 物化阶段。

## CONTRIBUTING.md 约束

- Bug fix 在报告可复现或至少 actionable 后可以直接进入 code PR，不要求 feature spec。
- Bug fix 必须包含能够捕获问题的 regression test。
- 提交前需要运行 `./script/presubmit`，PR 还需要提供 manual testing 证据。
- 当前状态已经足以确定崩点和优先调查方向，但尚未有最小复现，因此不能宣称根因已经锁定。

## 自动化复现进展

- 已在 `FlatStorage` 级稳定复现 Apple report 同形态越界：窄 storage 物化末尾双宽字符时，`RowIterator::next()` 的 `row[idx + 1]` 越界。
- 已在 `c28fdddb^` 上稳定复现 `#10305` 的旧 ASCII 列宽失配基线，说明旧修复针对的问题真实存在。
- 已尝试将 `Clear + resize + scrollback + CJK + finish + full reflow` 接成 `GridHandler` 自然序列，但旧版和当前版均未触发 Apple 栈。
- 当前结论：
  - CC 对最终 panic 机制定位正确。
  - CC 推断的完整产品入口尚未证明。
  - 不建议继续依赖手动随机使用；下一步应在临时 worktree 增加诊断断言，捕获首个非法索引生产点。
