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
//! 2. Emit a real CLI-agent `session_start` OSC notification, then repeatedly
//!    force an emoji-with-variation-selector glyph onto the right edge of the
//!    active block and accumulate the resulting rows in scrollback.
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
use std::time::Duration;
use std::{env, fs};

use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::vec2f;
use warp::integration_testing::step::new_step_with_default_assertions;
use warp::integration_testing::terminal::util::ExpectedExitStatus;
use warp::integration_testing::terminal::{
    assert_active_block_received_precmd, assert_no_block_executing,
    execute_command_for_single_terminal_in_tab, execute_long_running_command,
    wait_until_bootstrapped_single_pane_for_tab,
};
use warp::integration_testing::view_getters::single_terminal_view_for_tab;
use warp::terminal::TerminalView;
use warpui::integration::TestStep;
use warpui::{async_assert, ViewHandle};

use super::{new_builder, Builder};

/// Number of corrupt right-edge rows we push into scrollback before exposing
/// the invalid row through a clear + resize reflow.
const SCROLLBACK_LINES: usize = 400;
const CONTINUE_SENTINEL: &str = ".rowiter-continue";
const EXIT_SENTINEL: &str = ".rowiter-exit";
const READY_MARKER: &str = "ROWITER_READY";
const DONE_MARKER: &str = "ROWITER_DONE";

/// Window widths we cycle through. Each transition forces a full reflow
/// through `GridHandler::resize_storage` -> `FlatStorage::pop_rows`.
///
/// Pixel widths are intentionally chosen near the column counts that show
/// up in the field crashes (~80 / ~117 / ~130 columns) without depending
/// on the exact font metrics, which vary by platform.
const RESIZE_WIDTHS: &[f32] = &[700.0, 480.0, 880.0, 360.0, 960.0, 600.0];

/// `printf` invocation that enters CLI-agent mode and waits for the test to
/// resize the live block before pushing rows through the shell.
///
/// We deliberately avoid embedding raw newlines in the command line — the
/// shell would split a multi-line command into separate prompt entries and
/// the integration test's "command executed" assertion would never see the
/// full command. The session-start notification, resize, and payload
/// intentionally happen while the same foreground block is active: that is
/// the sequence which previously desynchronized `flat_storage.columns`.
fn staged_cli_agent_payload_command() -> String {
    format!(
        "rm -f \"$HOME/{CONTINUE_SENTINEL}\" \"$HOME/{EXIT_SENTINEL}\"; \
         printf '\\033]777;notify;warp://cli-agent;{{\"v\":1,\"agent\":\"claude\",\"event\":\"session_start\",\"session_id\":\"rowiter-repro\",\"cwd\":\"/tmp\",\"project\":\"rowiter-repro\",\"plugin_version\":\"1.1.0\"}}\\007'; \
         printf '{READY_MARKER}\\n'; \
         while [ ! -f \"$HOME/{CONTINUE_SENTINEL}\" ]; do sleep 0.05; done; \
         for i in $(seq 1 {SCROLLBACK_LINES}); do \
             printf '\\033[2J\\033[H\\033[999C中\\033[1;1Habc中def\\033[4G\\033[P\\033[@\\033[X\\033[999C☁️\\r\\n'; \
         done; \
         printf '{DONE_MARKER}\\n'; \
         while [ ! -f \"$HOME/{EXIT_SENTINEL}\" ]; do sleep 0.05; done",
    )
}

fn active_block_output_contains(marker: &'static str) -> TestStep {
    TestStep::new(&format!("Active block output contains {marker}"))
        .add_assertion(move |app, window_id| {
            let view: ViewHandle<TerminalView> = single_terminal_view_for_tab(app, window_id, 0);
            view.read(app, |view, _ctx| {
                let model = view.model.lock();
                async_assert!(
                    model
                        .block_list()
                        .active_block()
                        .output_to_string()
                        .contains(marker),
                    "Active block output should contain {marker}",
                )
            })
        })
        .set_timeout(Duration::from_secs(10))
}

fn release_sentinel(filename: &'static str) -> TestStep {
    TestStep::new(&format!("Create {filename} sentinel")).with_action(move |_, _, _| {
        fs::write(
            env::var("HOME")
                .expect("HOME should be set for integration tests")
                .to_owned()
                + "/"
                + filename,
            "",
        )
        .expect("sentinel should be writable");
    })
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
            let new_bounds = RectF::new(bounds.origin(), vec2f(new_width, bounds.size().y()));
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
        .with_step(execute_long_running_command(
            0,
            staged_cli_agent_payload_command(),
        ))
        .with_step(active_block_output_contains(READY_MARKER))
        .with_step(resize_window_width(960.0))
        .with_step(release_sentinel(CONTINUE_SENTINEL))
        .with_step(active_block_output_contains(DONE_MARKER))
        .with_step(release_sentinel(EXIT_SENTINEL))
        .with_step(
            new_step_with_default_assertions("CLI agent payload block exits")
                .add_assertion(assert_no_block_executing(0, 0))
                .add_assertion(assert_active_block_received_precmd(0, 0))
                .set_timeout(Duration::from_secs(10)),
        )
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
