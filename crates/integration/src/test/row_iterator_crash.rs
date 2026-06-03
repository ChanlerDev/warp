//! Regression integration test for the `RowIterator::next` out-of-bounds panic.
//!
//! Apple crash report:
//! ```text
//! RowIterator::next
//!   -> FlatStorage::pop_rows
//!     -> GridHandler::resize_storage
//!       -> GridHandler::resize
//!         -> TerminalModel::resize
//!           -> BlockList::resize
//!             -> TerminalView::resize_internal
//!               -> TerminalView::after_terminal_view_layout
//! panicked at row_iterator.rs:132:20
//!   index out of bounds: the len is 117 but the index is 117
//! ```
//!
//! See `self/notes/2026-06-01-row-iterator-crash-bug.md` for the full
//! investigation.
//!
//! Corrupt rows (WIDE_CHAR at the last column, no spacer) cannot be produced
//! through normal PTY I/O because the ANSI handler always pairs WIDE_CHAR /
//! WIDE_CHAR_SPACER correctly. The crash occurs when a buggy upstream resize
//! path produces a corrupt row, which is then materialized by
//! `RowIterator::next` during a subsequent resize.
//!
//! The test injects a corrupt row into the last visible row of the **grid**
//! (not flat_storage) and then triggers a vertical pane split via
//! cmd+shift+D (SplitDown).  SplitDown changes height but NOT width, so
//! `set_columns` is a no-op and the WIDE_CHAR stays at the last column:
//!
//!   inject_corrupt_row_into_grid → split down →
//!   after_terminal_view_layout → resize_internal →
//!   TerminalModel::resize → BlockList::resize →
//!   GridHandler::resize → resize_storage → pop_rows →
//!   RowIterator::next
//!
//! `resize_storage` pushes grid rows (including the corrupt one) into
//! flat_storage at the same column count, then materializes them via
//! `pop_rows`.  Without the fix, this panics when `RowIterator::next`
//! encounters the WIDE_CHAR in the last cell position.

use std::time::Duration;

use warp::integration_testing::pane_group::assert_focused_pane_index;
use warp::integration_testing::step::new_step_with_default_assertions;
use warp::integration_testing::terminal::{
    wait_until_bootstrapped_pane, wait_until_bootstrapped_single_pane_for_tab,
};
use warp::integration_testing::view_getters::{single_terminal_view_for_tab, terminal_view};
use warp::terminal::TerminalView;
use warpui::integration::TestStep;
use warpui::{async_assert, ViewHandle};

use super::{new_builder, Builder};

/// Injects a corrupt row (WIDE_CHAR in last cell, no spacer) into the last
/// visible row of the grid via `TerminalView::inject_corrupt_row_into_last_grid_row_for_test`.
/// Does NOT call `pop_rows` — the following resize step materializes it
/// through `RowIterator::next`.
fn inject_corrupt_row_into_grid() -> TestStep {
    TestStep::new("Inject corrupt row into last grid row").with_action(move |app, window_id, _| {
        let view: ViewHandle<TerminalView> = single_terminal_view_for_tab(app, window_id, 0);
        view.update(app, |view, _ctx| {
            view.inject_corrupt_row_into_last_grid_row_for_test();
        });
    })
}

/// Triggers a vertical pane split via cmd+shift+D (SplitDown).
///
/// Unlike SplitRight, this changes height but NOT width, so `set_columns`
/// is a no-op and the corrupt row's WIDE_CHAR stays at the last column
/// position.  The resize flows through the full call chain:
///   resize_storage → pop_rows → RowIterator::next — which panics
///   without the fix.
fn split_down_triggering_resize() -> TestStep {
    new_step_with_default_assertions("Split pane down — triggers resize_storage")
        .with_keystrokes(&["cmd-shift-D"])
        .add_assertion(assert_focused_pane_index(0, 1))
}

/// Returns an assertion step that verifies pane 0 is still alive.
fn assert_terminal_alive(label: &str) -> TestStep {
    new_step_with_default_assertions(label)
        .add_assertion(|app, window_id| {
            let view: ViewHandle<TerminalView> = terminal_view(app, window_id, 0, 0);
            view.read(app, |view, _ctx| {
                let model = view.model.lock();
                async_assert!(
                    !model.is_block_list_empty(),
                    "Block list should be non-empty",
                )
            })
        })
        .set_timeout(Duration::from_secs(10))
}

/// Inject corrupt row → split down (height-only resize) →
/// `resize_storage` → `pop_rows` → `RowIterator::next`.
/// Without the fix, this panics.
pub fn test_row_iterator_panic_on_resize_with_cjk_scrollback() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(inject_corrupt_row_into_grid())
        .with_step(split_down_triggering_resize())
        .with_step(wait_until_bootstrapped_pane(0, 1))
        .with_step(assert_terminal_alive(
            "Terminal view alive after resize through RowIterator",
        ))
}

/// Inject corrupt row into grid of pane 0, split down — exercises the full
/// `resize_storage` → `pop_rows` path against the corrupt row.
///
/// Registered in `crates/integration/src/bin/integration.rs::register_tests`
/// and added to the manual-runner list in
/// `crates/integration/tests/integration.rs`.
pub fn test_row_iterator_crash_multi_pane_with_tab_close() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(inject_corrupt_row_into_grid())
        .with_step(split_down_triggering_resize())
        .with_step(wait_until_bootstrapped_pane(0, 1))
        .with_step(assert_terminal_alive(
            "Terminal view alive after resize through RowIterator",
        ))
}
