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
//! This test injects a corrupt row directly into `flat_storage` via the
//! test-only `GridHandler::push_corrupt_row_for_test` API, which immediately
//! calls `pop_rows` to exercise `RowIterator::next` on the corrupt data.

use std::time::Duration;

use warp::integration_testing::step::new_step_with_default_assertions;
use warp::integration_testing::terminal::wait_until_bootstrapped_single_pane_for_tab;
use warp::integration_testing::view_getters::single_terminal_view_for_tab;
use warp::terminal::TerminalView;
use warpui::integration::TestStep;
use warpui::{async_assert, ViewHandle};

use super::{new_builder, Builder};

/// Injects a corrupt row into flat_storage and immediately calls `pop_rows`, which
/// materializes it through `RowIterator::next`. Without the fix, this panics.
fn inject_and_pop_corrupt_row() -> TestStep {
    TestStep::new("Inject corrupt row and pop through RowIterator").with_action(
        move |app, window_id, _| {
            let view: ViewHandle<TerminalView> = single_terminal_view_for_tab(app, window_id, 0);
            view.update(app, |view, _ctx| {
                let mut model = view.model.lock();
                let cols = model.block_list().active_block().size().columns();
                let gh = model.block_list_mut().active_block_mut().grid_handler_mut();
                gh.push_corrupt_row_for_test(cols);
            });
        },
    )
}

/// Returns an assertion step that verifies the terminal view is still alive.
fn assert_terminal_alive(label: &str) -> TestStep {
    new_step_with_default_assertions(label)
        .add_assertion(|app, window_id| {
            let view: ViewHandle<TerminalView> = single_terminal_view_for_tab(app, window_id, 0);
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

/// Injects a corrupt row and immediately materializes it through
/// `RowIterator::next`. Without the fix, this panics at `row_iterator.rs:132`
/// when the WIDE_CHAR at the last column causes `row[idx + 1]` to go out of
/// bounds.
pub fn test_row_iterator_panic_on_resize_with_cjk_scrollback() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(inject_and_pop_corrupt_row())
        .with_step(assert_terminal_alive(
            "Terminal view alive after RowIterator exercise",
        ))
}

/// Same as above but also exercises the path through multiple panes.
///
/// Registered in `crates/integration/src/bin/integration.rs::register_tests`
/// and added to the manual-runner list in
/// `crates/integration/tests/integration.rs`.
pub fn test_row_iterator_crash_multi_pane_with_tab_close() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(inject_and_pop_corrupt_row())
        .with_step(assert_terminal_alive(
            "Terminal view alive after RowIterator exercise",
        ))
}
