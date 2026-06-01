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
//! investigation. This test exercises the public product entry point that
//! the Apple stack walks through:
//!
//! 1. Bootstrap a single-pane terminal at a custom narrow window size so the
//!    columns approach the same odd-ish boundary class as the crash (~117).
//! 2. Pump a long burst of CJK + emoji-with-variation-selector glyphs into
//!    the active block, so wide-char graphemes land repeatedly on the right
//!    edge of `flat_storage` and accumulate in scrollback.
//! 3. Issue `clear` so the model walks the `finish_background_block` /
//!    `clear_visible_screen` path that precedes the crashing reflow on Apple
//!    crash reports.
//! 4. Cycle the window bounds across several widths (narrower → wider →
//!    narrower) so each transition triggers a full `BlockList::resize` ->
//!    `GridHandler::resize` -> `FlatStorage::pop_rows` reflow.
//! 5. Assert the terminal view is still alive after the cycle. If
//!    `RowIterator::next` ever panics, the entire test process terminates,
//!    so the assertion never runs and the test fails.
//!
//! NOTE: this test is a *regression scaffold*. On the current `master` the
//! known ANSI public entry points are not yet known to deterministically
//! produce a `Flags::WIDE_CHAR` cell at the last column, so the test may
//! pass even when the underlying defect is still latent. It is wired up so
//! that future input fuzzing or a `debug_assert!` in
//! `flat_storage::push_rows_internal` can be added without rewriting the
//! harness.

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

/// Number of scrollback lines we attempt to fill with wide chars. Larger
/// values increase the chance that some transient parser state leaves a
/// `WIDE_CHAR` cell on the right edge during reflow.
const SCROLLBACK_LINES: usize = 400;

/// Window widths we cycle through. Each transition forces a full reflow
/// through `GridHandler::resize_storage` -> `FlatStorage::pop_rows`.
///
/// Pixel widths are intentionally chosen near the column counts that show
/// up in the field crashes (~80 / ~117 / ~130 columns) without depending
/// on the exact font metrics, which vary by platform.
const RESIZE_WIDTHS: &[f32] = &[700.0, 480.0, 880.0, 360.0, 960.0, 600.0];

/// Build the long CJK + emoji-presentation string we feed through the PTY.
///
/// One unit mixes naked CJK wide chars (`中文`), a narrow base + VS-16 that
/// promotes to wide (`☁\u{FE0F}`), and a trailing ASCII tail so wrap
/// boundaries shift between lines.
fn payload_unit() -> &'static str {
    "中文中文中\u{2601}\u{FE0F}x"
}

/// `printf` invocation that pushes the wide-char payload through the shell.
///
/// We deliberately avoid embedding raw newlines in the command line — the
/// shell would split a multi-line command into separate prompt entries and
/// the integration test's "command executed" assertion would never see the
/// full command. Instead we drive the loop inside the shell with `seq`.
fn payload_command() -> String {
    format!(
        "for i in $(seq 1 {SCROLLBACK_LINES}); do printf '{unit}\\n'; done",
        unit = payload_unit(),
    )
}

/// Returns a step that resizes the active window to the given pixel width
/// while preserving the current height. The test framework dispatches the
/// bounds change through `set_and_cache_window_bounds`, which mirrors what
/// the windowing platform does on a user-driven drag.
fn resize_window_width(new_width: f32) -> TestStep {
    let label = format!("Resize window width to {new_width}");
    TestStep::new(&label)
        .with_action(move |app, window_id, _| {
            let bounds = app
                .window_bounds(&window_id)
                .expect("window bounds should exist");
            let new_bounds = RectF::new(
                bounds.origin(),
                vec2f(new_width, bounds.size().y()),
            );
            app.update(|ctx| {
                ctx.set_and_cache_window_bounds(window_id, new_bounds);
            });
        })
        .set_timeout(Duration::from_secs(5))
}

/// The integration test entry point. Registered in
/// `crates/integration/src/bin/integration.rs::register_tests` and added to
/// the manual-runner list in `crates/integration/tests/integration.rs`.
pub fn test_row_iterator_panic_on_resize_with_cjk_scrollback() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(execute_command_for_single_terminal_in_tab(
            0,
            payload_command(),
            ExpectedExitStatus::Success,
            (),
        ))
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
        .with_step(
            new_step_with_default_assertions("Terminal view alive after resize cycle")
                .add_assertion(|app, window_id| {
                    let view: ViewHandle<TerminalView> =
                        single_terminal_view_for_tab(app, window_id, 0);
                    view.read(app, |view, _ctx| {
                        let model = view.model.lock();
                        // The block list having any block at all is enough
                        // to prove we did not unwind through a panic; if
                        // `RowIterator::next` had blown up the entire
                        // process would be gone before this runs.
                        async_assert!(
                            !model.is_block_list_empty(),
                            "Block list should be non-empty after wide-char + resize cycle",
                        )
                    })
                })
                .set_timeout(Duration::from_secs(10)),
        )
}
