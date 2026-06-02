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
//! This test injects corrupt rows directly into `flat_storage` via a
//! test-only API (`GridHandler::push_corrupt_row_for_test`), then exercises
//! the full production reflow path: clear + window resize.

use std::time::Duration;

use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::vec2f;
use warp::integration_testing::step::new_step_with_default_assertions;
use warp::integration_testing::terminal::util::ExpectedExitStatus;
use warp::integration_testing::terminal::{
    execute_command_for_single_terminal_in_tab, wait_until_bootstrapped_single_pane_for_tab,
};
use warp::integration_testing::view_getters::single_terminal_view_for_tab;
use warp::terminal::TerminalView;
use warpui::integration::TestStep;
use warpui::{async_assert, ViewHandle};

use super::{new_builder, Builder};

/// Number of corrupt rows to inject into scrollback before triggering reflow.
const NUM_CORRUPT_ROWS: usize = 400;

/// Window widths we cycle through. Each transition forces a full reflow
/// through `GridHandler::resize_storage` -> `FlatStorage::pop_rows`.
const RESIZE_WIDTHS: &[f32] = &[700.0, 480.0, 880.0, 360.0, 960.0, 600.0];

/// Injects `num_rows` corrupt rows into the terminal's active block grid
/// storage. Each row has a WIDE_CHAR in the last column with no trailing
/// WIDE_CHAR_SPACER — a state that cannot be produced through normal PTY
/// I/O but was historically created by a buggy resize path in
/// `push_rows_internal`.
fn inject_corrupt_rows() -> TestStep {
    TestStep::new("Inject corrupt rows into grid storage").with_action(move |app, window_id, _| {
        let view: ViewHandle<TerminalView> = single_terminal_view_for_tab(app, window_id, 0);
        view.update(app, |view, _ctx| {
            let mut model = view.model.lock();
            let cols = model.block_list().active_block().size().columns();
            let gh = model.block_list_mut().active_block_mut().grid_handler_mut();
            for _ in 0..NUM_CORRUPT_ROWS {
                gh.push_corrupt_row_for_test(cols);
            }
        });
    })
}

/// Returns a step that resizes the active window to the given pixel width
/// while preserving the current height.
fn resize_window_width(new_width: f32) -> TestStep {
    let label = format!("Resize window width to {new_width}");
    TestStep::new(&label)
        .with_action(move |app, window_id, _| {
            let bounds = app
                .window_bounds(&window_id)
                .expect("window bounds should exist");
            let new_bounds = RectF::new(bounds.origin(), vec2f(new_width, bounds.size().y()));
            app.update(|ctx| {
                ctx.set_and_cache_window_bounds(window_id, new_bounds);
            });
        })
        .set_timeout(Duration::from_secs(5))
}

/// Returns an assertion step that verifies the terminal view at (tab_idx,
/// pane_idx) is still alive after the resize cycle.
fn assert_terminal_alive(label: &str) -> TestStep {
    new_step_with_default_assertions(label)
        .add_assertion(|app, window_id| {
            let view: ViewHandle<TerminalView> = single_terminal_view_for_tab(app, window_id, 0);
            view.read(app, |view, _ctx| {
                let model = view.model.lock();
                async_assert!(
                    !model.is_block_list_empty(),
                    "Block list should be non-empty after resize cycle",
                )
            })
        })
        .set_timeout(Duration::from_secs(10))
}

/// Injects corrupt rows into scrollback, clears the screen, then cycles
/// through multiple window widths to trigger full reflow through
/// `BlockList::resize` → `FlatStorage::pop_rows` → `RowIterator::next`.
///
/// Without the fix this panics at `row_iterator.rs:132` when the corrupt
/// row's WIDE_CHAR at the last column causes an out-of-bounds access on
/// `row[idx + 1]`.
pub fn test_row_iterator_panic_on_resize_with_cjk_scrollback() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(inject_corrupt_rows())
        .with_step(execute_command_for_single_terminal_in_tab(
            0,
            "clear".to_owned(),
            ExpectedExitStatus::Success,
            (),
        ))
        .with_steps(
            RESIZE_WIDTHS
                .iter()
                .copied()
                .map(resize_window_width)
                .collect::<Vec<_>>(),
        )
        .with_step(assert_terminal_alive(
            "Terminal view alive after resize cycle",
        ))
}

/// Same as above but also exercises the path through multiple panes.
/// Splits the terminal, injects corrupt rows, and triggers reflow.
///
/// Registered in `crates/integration/src/bin/integration.rs::register_tests`
/// and added to the manual-runner list in
/// `crates/integration/tests/integration.rs`.
pub fn test_row_iterator_crash_multi_pane_with_tab_close() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(inject_corrupt_rows())
        .with_step(execute_command_for_single_terminal_in_tab(
            0,
            "clear".to_owned(),
            ExpectedExitStatus::Success,
            (),
        ))
        .with_steps(
            RESIZE_WIDTHS
                .iter()
                .copied()
                .map(resize_window_width)
                .collect::<Vec<_>>(),
        )
        .with_step(assert_terminal_alive(
            "Terminal view alive after resize cycle",
        ))
}
