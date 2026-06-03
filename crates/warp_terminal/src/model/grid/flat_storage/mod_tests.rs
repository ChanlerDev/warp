use itertools::Itertools;
use testing::{assert_rows_equal, ToRows as _};

use super::*;
use crate::model::char_or_str::CharOrStr;
use crate::model::grid::cell::{Cell, Flags};

#[test]
fn test_row_iteration() {
    let storage = FlatStorage::from_content_using_rows("hello world\n", 7, Some(2));

    let mut rows = storage.rows_from(0);

    let row1 = rows
        .next()
        .expect("should be able to get first row from storage");
    assert_eq!(row1.occ, 7);
    assert_eq!(row1[0].c, 'h');
    assert_eq!(row1[6].c, 'w');

    let row2 = rows
        .next()
        .expect("should be able to get first row from storage");
    assert_eq!(row2.occ, 4);
    assert_eq!(row2[0].c, 'o');
    assert_eq!(row2[3].c, 'd');

    assert!(rows.next().is_none());
}

#[test]
fn test_row_with_double_width_char() {
    let storage = FlatStorage::from_content_using_rows("hi 😀 hello\n", 6, Some(2));

    let mut rows = storage.rows_from(0);

    let row1 = rows
        .next()
        .expect("should be able to get first row from storage");
    assert_eq!(row1.occ, 6);
    assert_eq!(row1[0].c, 'h');
    assert_eq!(row1[3].c, '😀');
    assert!(row1[4].flags().contains(Flags::WIDE_CHAR_SPACER));
    assert_eq!(row1[5].c, ' ');

    let row2 = rows
        .next()
        .expect("should be able to get first row from storage");
    assert_eq!(row2.occ, 5);
    assert_eq!(row2[0].c, 'h');

    assert!(rows.next().is_none());
}

/// This test validates our handling of complex emoji sequences.
///
/// The three graphemes here are comprised of a number of Unicode characters.
/// Below are the individual characters that comprise the test string, with
/// "---" denoting how the string gets segmented into graphemes.
///
///  1. 🧑  1F9D1   ADULT
///  2.     1F3FF   EMOJI MODIFIER FITZPATRICK TYPE-6
///  3. ‍    200D    ZERO WIDTH JOINER
///  4. 🦰  1F9B0   EMOJI COMPONENT RED HAIR
///  ---
///  1. 👩  1F469   WOMAN
///  2. ‍    200D    ZERO WIDTH JOINER
///  3. 🦲  1F9B2   EMOJI COMPONENT BALD
///  ---
///  1. 🧔  1F9D4   BEARDED PERSON
///  2. 🏿   1F3FF   EMOJI MODIFIER FITZPATRICK TYPE-6
///  3. ‍    200D    ZERO WIDTH JOINER
///  4. ♂   2642    MALE SIGN
///  5. ️    FE0F    VARIATION SELECTOR-16
#[test]
#[ignore = "will not pass until using a version of unicode-width that includes commit afab363"]
fn test_row_with_complex_emoji() {
    let storage = FlatStorage::from_content_using_rows("🧑🏿‍🦰👩‍🦲🧔🏿‍♂️", 6, Some(1));

    let mut rows = storage.rows_from(0);
    let row1 = rows
        .next()
        .expect("should be able to get first row from storage");
    assert_eq!(row1.occ, 6);

    assert_eq!(row1[0].c, '🧑');
    assert!(matches!(
        row1[0].content_for_display(),
        CharOrStr::Str("🧑🏿‍🦰")
    ));

    assert!(row1[1].flags().contains(Flags::WIDE_CHAR_SPACER));
}

#[test]
fn test_push_rows_with_color() {
    let mut storage = FlatStorage::new(5, None, Some(2));

    let mut fg_cell = Cell::default();
    fg_cell.c = 'f';

    let mut red_cell = Cell::default();
    red_cell.c = 'r';
    red_cell.fg = ansi::Color::Named(ansi::NamedColor::Red);

    let row = Row::from_vec(
        vec![
            Cell::default(),
            Cell::default(),
            red_cell.clone(),
            red_cell,
            Cell::default(),
        ],
        5,
    );
    storage.push_rows([&row]);

    assert_eq!(storage.rows_from(0).next().unwrap().as_ref(), &row);
}

#[test]
fn test_push_rows_with_color_and_multibyte_chars() {
    let mut storage = FlatStorage::new(5, None, Some(2));

    let mut fg_cell = Cell::default();
    fg_cell.c = '❤';

    let mut red_cell = Cell::default();
    red_cell.c = 'r';
    red_cell.fg = ansi::Color::Named(ansi::NamedColor::Red);

    let row = Row::from_vec(
        vec![
            fg_cell.clone(),
            fg_cell.clone(),
            red_cell.clone(),
            red_cell,
            fg_cell,
        ],
        5,
    );
    storage.push_rows([&row]);

    assert_eq!(storage.rows_from(0).next().unwrap().as_ref(), &row);
}

#[test]
fn test_row_roundtrip_and_resize() {
    let num_cols = 5;
    let rows = "😀😃😄ag\na😁😆~!!\n😅sdf😂\n".to_rows(num_cols);

    // Build FlatStorage from the set of rows.
    let mut storage = FlatStorage::new(num_cols, None, None);
    storage.push_rows(&rows);

    // Make sure the generated rows match the original input.
    let flat_rows = storage
        .rows_from(0)
        .map(|row| row.as_ref().clone())
        .collect_vec();

    assert_rows_equal(&flat_rows, &rows);

    // "Resize" the storage, keeping the number of columns the same.
    storage.set_columns(num_cols);

    // Make sure the generated rows match the original input.
    let flat_rows = storage
        .rows_from(0)
        .map(|row| row.as_ref().clone())
        .collect_vec();

    assert_rows_equal(&flat_rows, &rows);
}

#[test]
fn test_styling_change_within_trailing_empty_cells() {
    let num_cols = 5;
    let mut rows = "a\nb\n".to_rows(num_cols);

    // Make the final cell in the first row bold.
    rows[0][num_cols - 1].flags.insert(Flags::BOLD);

    // Push the rows into storage.  This should produce a first row that is 5
    // cells long (the "a" followed by 3 empty cells followed by a bold empty
    // cell) and then clear the bold styling on the first cell of the second
    // line.
    let mut storage = FlatStorage::new(num_cols, None, None);
    storage.push_rows(&rows);

    let flat_rows = storage
        .rows_from(0)
        .map(|row| row.as_ref().clone())
        .collect_vec();

    // The first row's content should be 5 characters + a trailing newline.
    assert_eq!(flat_rows[0][0].c, 'a');
    assert_eq!(flat_rows[0][1].c, '\0');
    assert_eq!(flat_rows[0][2].c, '\0');
    assert_eq!(flat_rows[0][3].c, '\0');
    assert_eq!(flat_rows[0][4].c, '\0');
    assert!(!flat_rows[0][4].flags.contains(Flags::WRAPLINE));

    // The final cell in the first row should be bold, but the first cell in
    // the second row should not.
    assert!(flat_rows[0][num_cols - 1].flags.intersects(Flags::BOLD));
    assert!(!flat_rows[1][0].flags.intersects(Flags::BOLD));
}

// === Repro for Apple crash report 2026-06-01 14:18:17 ===
//
// Apple stack hits `RowIterator::next` -> `FlatStorage::pop_rows` ->
// `GridHandler::resize_storage` after a CJK-heavy session in WarpPreview
// `0.2026.05.27.15.44.preview_01`.  Panic text: `index out of bounds: the
// len is 117 but the index is 117`.
//
// These tests probe the materialization path itself (`RowIterator::next` at
// `row[idx + 1]`) by feeding the index a row that legitimately ends with a
// wide character + spacer pair, then asking flat storage to round-trip the
// content.  Each scenario is documented inline so a future maintainer can
// see exactly which boundary it exercises.
#[test]
fn repro_wide_char_at_last_column_roundtrip() {
    // Width-5 storage carrying "aaa中" — three ASCII cells then a CJK
    // grapheme that must occupy cells 3 and 4.  This is the simplest shape
    // that mirrors the Apple panic: the wide character lands in the final
    // column.  RowIterator::next must not over-write past the end of the
    // 5-cell row when re-materializing this row.
    let num_cols = 5;
    let rows = "aaa中\n".to_rows(num_cols);

    let mut storage = FlatStorage::new(num_cols, None, None);
    storage.push_rows(&rows);

    let flat_rows = storage
        .rows_from(0)
        .map(|row| row.as_ref().clone())
        .collect_vec();

    assert_rows_equal(&flat_rows, &rows);
}

#[test]
fn repro_wide_char_after_set_columns_shrink() {
    // Push a row with a CJK grapheme at columns 5/6 in a 7-column storage,
    // then shrink to 6 columns.  Index::rebuild reflows the row; if the
    // reflow lets the wide character land on the final cell instead of
    // wrapping, RowIterator::next will write to row[idx + 1] past the end.
    let num_cols = 7;
    let rows = "abcde中\n".to_rows(num_cols);

    let mut storage = FlatStorage::new(num_cols, None, None);
    storage.push_rows(&rows);

    storage.set_columns(6);

    let flat_rows = storage
        .rows_from(0)
        .map(|row| row.as_ref().clone())
        .collect_vec();

    // We don't care about the exact layout — the failure mode is panic
    // during materialization.  Just make sure we got at least one row out.
    assert!(!flat_rows.is_empty());
}

#[test]
fn repro_wide_char_after_pop_rows_117_columns() {
    // Mimic the Apple report's terminal width (117 cols) with CJK-heavy
    // scrollback and many rows, so that pop_rows has to materialize each
    // row through RowIterator.  If any row ends up with a wide character
    // in the final column after Index::rebuild, this should panic.
    let num_cols = 117;
    let mut content = String::new();
    for _ in 0..50 {
        // Fill with CJK characters — each is 2 cells wide.  117 is odd so
        // a pure CJK fill leaves one ASCII column at the end.  Insert a
        // single ASCII at the row start to push the trailing CJK into the
        // last two columns: 1 + 58 * 2 = 117 → final wide char on cols
        // 115/116.  When set_columns shrinks by 1, the wide char would
        // land on cols 114/115, but the trailing ASCII at offset 1 means
        // the last grapheme ends exactly at the new boundary.
        content.push('a');
        for _ in 0..58 {
            content.push('中');
        }
        content.push('\n');
    }

    let rows = content.as_str().to_rows(num_cols);
    let mut storage = FlatStorage::new(num_cols, None, None);
    storage.push_rows(&rows);

    // Resize down by one column — exactly the boundary that the
    // production reflow path hits when the user nudges the window or
    // closes a pane.
    storage.set_columns(num_cols - 1);

    // Force materialization of every row, the same way pop_rows does.
    let _ = storage
        .rows_from(0)
        .map(|row| row.as_ref().clone())
        .collect_vec();
}

/// Construct a Row that violates the wrap invariant: the wide character
/// is placed at the absolute final cell, with no spacer cell after it.
/// This mirrors the corrupt state that a buggy upstream resize path
/// could leave behind, and that #10305 was supposed to prevent.
fn build_corrupt_row_wide_at_end(cols: usize) -> Row {
    let mut row = Row::new(cols);
    // Fill cols 0..cols-1 with ASCII to keep the row contiguous.
    for i in 0..cols - 1 {
        let cell = &mut row[i];
        cell.c = ('a' as u32 + (i as u32 % 26)) as u8 as char;
    }
    // Final cell holds the leading half of a wide char without a
    // spacer to its right (which is impossible in a 1D row).
    let last = &mut row[cols - 1];
    last.c = '中';
    last.flags.insert(Flags::WIDE_CHAR);
    row.occ = cols;
    row
}

#[test]
fn repro_corrupt_row_wide_char_at_last_cell_panics() {
    // Push a hand-built Row whose final cell is marked WIDE_CHAR with no
    // spacer after it.  push_rows -> push_rows_internal goes through
    // process_grapheme_info_unchecked which does NOT enforce the wrap
    // invariant, so the corruption propagates into the index.  Once
    // RowIterator tries to materialize the row it should panic exactly
    // like the Apple stack:
    //
    //   index out of bounds: the len is N but the index is N
    //
    // If this test passes silently, the materializer somehow tolerated
    // the corruption.  If it panics, we have reproduced the production
    // crash without going through the UI.
    let cols = 117;
    let row = build_corrupt_row_wide_at_end(cols);

    let mut storage = FlatStorage::new(cols, None, None);
    storage.push_rows([&row]);

    // pop_rows is the exact entry point in the Apple stack.
    let _ = storage.pop_rows(1);
}

#[test]
fn repro_corrupt_row_then_set_columns_then_pop_rows() {
    // Variant: push corrupt row first, then run set_columns (full reflow
    // path) and finally pop_rows.  Apple crash hit a 117-column
    // GridHandler::resize_storage, which calls set_columns followed by
    // pop_rows under the hood.
    let cols = 117;
    let row = build_corrupt_row_wide_at_end(cols);

    let mut storage = FlatStorage::new(cols, None, None);
    storage.push_rows([&row]);

    // Trigger Index::rebuild — the same path as resize_storage's full
    // reflow branch.  If rebuild can't sanitize the corruption, the
    // following materialization should explode.
    storage.set_columns(cols);

    let _ = storage.pop_rows(1);
}

#[test]
fn test_clear_after_truncate_front() {
    let num_cols = 20;
    let rows = "abcd\n789\n1 overflow\n2 overflow\n".to_rows(num_cols);

    let mut storage = FlatStorage::new(num_cols, Some(2), None);
    storage.push_rows(&rows);

    // We pushed 4 rows, and the limit is 2, so we should have truncated 2 rows.
    assert_eq!(storage.total_rows(), 2);
    assert_eq!(storage.num_truncated_rows(), 2);

    // Make sure the truncated rows are what we expect.
    assert_eq!(
        storage.rows_from(0).next().expect("should have a row")[0].c,
        '1'
    );
    assert_eq!(
        storage.rows_from(1).next().expect("should have a row")[0].c,
        '2'
    );

    // Clear flat storage, and ensure the state is as we expect.
    storage.clear();
    assert_eq!(storage.total_rows(), 0);
    // Should still have 2 truncated rows, as clearing storage doesn't affect
    // the number of rows we've truncated in total so far.
    assert_eq!(storage.num_truncated_rows(), 2);

    // Make sure we can push new rows.
    storage.push_rows(&rows);
    assert_eq!(storage.total_rows(), 2);
    assert_eq!(storage.num_truncated_rows(), 4);

    // Make sure remaining truncated rows are what we expect.
    assert_eq!(
        storage.rows_from(0).next().expect("should have a row")[0].c,
        '1'
    );
    assert_eq!(
        storage.rows_from(1).next().expect("should have a row")[0].c,
        '2'
    );
}

// === Proptest: reflow fuzz for RowIterator crash (Apple crash 2026-06-01) ===
//
// Randomly generates CJK + ASCII content, pushes it into FlatStorage at one
// column width, calls set_columns to a different width (triggering
// Index::rebuild), then materializes all rows through RowIterator::next.
//
// This directly targets Hypothesis B from the crash report: a reflow
// boundary bug in Index::rebuild that could leave a WIDE_CHAR in the
// last cell with no spacer after it, causing RowIterator::next to panic
// at row[idx + 1].

#[test]
fn fuzz_reflow_wide_char_no_panic() {
    // CJK/emoji characters known to have cell_width == 2
    const WIDE_CHARS: &[&str] = &["中", "说", "😀", "汉", "字", "语", "文", "明"];
    const ASCII_CHARS: &[char] = &['a', 'b', 'c', 'x', 'y', 'z', '0', '1', '2'];

    use rand::Rng;

    let mut rng = rand::thread_rng();
    let mut buf = String::new();
    const ITERATIONS: usize = 100_000;

    // Column widths matching the Apple crash report scenario:
    // crash had 117 columns; panic at index 155 suggests
    // 155-column width.  Test both small (to maximize edge
    // density) and large (to match real crash) ranges.
    const COL_RANGES: &[(usize, usize)] = &[(2, 21), (50, 201)];
    const COL_RANGES_LARGE: &[(usize, usize)] = &[(2, 21), (80, 201)];

    for _ in 0..ITERATIONS {
        buf.clear();

        // Use the wider column range 80% of the time to match
        // real crash scenarios.
        let range = if rng.gen_bool(0.8) {
            COL_RANGES_LARGE[rng.gen_range(0..COL_RANGES_LARGE.len())]
        } else {
            COL_RANGES[rng.gen_range(0..COL_RANGES.len())]
        };
        let cols_src: usize = rng.gen_range(range.0..range.1);
        let cols_dst: usize = rng.gen_range(range.0..range.1);
        let num_rows: usize = rng.gen_range(1..51);

        // Generate random content: for each row, fill with random
        // mix of ASCII and wide chars, capped at cols_src width
        for _ in 0..num_rows {
            let mut row_cells = 0usize;
            loop {
                let is_wide = rng.gen_bool(0.4);
                let cell_w = if is_wide { 2 } else { 1 };
                if row_cells + cell_w > cols_src {
                    break;
                }
                row_cells += cell_w;
                if is_wide {
                    let idx: usize = rng.gen_range(0..WIDE_CHARS.len());
                    buf.push_str(WIDE_CHARS[idx]);
                } else {
                    let idx: usize = rng.gen_range(0..ASCII_CHARS.len());
                    buf.push(ASCII_CHARS[idx]);
                }
            }
            buf.push('\n');
        }

        let rows = buf.as_str().to_rows(cols_src);
        let mut storage = FlatStorage::new(cols_src, None, None);
        storage.push_rows(&rows);

        if cols_src != cols_dst {
            storage.set_columns(cols_dst);
        }

        // Materialize all rows — if RowIterator panics, the test fails.
        for row in storage.rows_from(0) {
            let row = row.as_ref();
            // Invariant: if the last cell is WIDE_CHAR, it must have
            // a WIDE_CHAR_SPACER in cell after it (impossible if
            // it's truly the last cell).
            let last = row.occ - 1;
            if last < row.len() && row[last].flags().contains(Flags::WIDE_CHAR) {
                assert!(
                    last + 1 < row.len() && row[last + 1].flags().contains(Flags::WIDE_CHAR_SPACER),
                    "row has WIDE_CHAR at last cell ({last}/{}) without WIDE_CHAR_SPACER\n\
                     cols_src={cols_src} cols_dst={cols_dst} content={buf:?}",
                    row.len(),
                );
            }
        }
    }
}

/// Hypothesis A: column-mismatch repro.
///
/// Push a valid 155-col Row (WIDE_CHAR at pos 116, spacer at 117) into
/// a 117-col FlatStorage.  `push_rows_internal` uses `self.columns` (117)
/// for the WRAPLINE check but `process_grapheme_info_unchecked` records
/// all 118 source cells.  The Index entry totals 118 cells, so
/// RowIterator materializes a 117-cell Row and panics at `row[idx + 1]`.
///
/// This mirrors the Apple crash: 117-col terminal, panics at index 117.
#[test]
#[should_panic(expected = "index out of bounds")]
fn repro_push_rows_column_mismatch_panics() {
    let cols_big: usize = 155;
    let cols_small: usize = 117;

    // Build a 155-col Row where a WIDE_CHAR is at source position 116
    // (spacer at 117).  This mirrors the crash: 117-col terminal,
    // panics at `row[117]`.
    let mut row = Row::new(cols_big);
    // Fill 0..116 with ASCII.
    for i in 0..116 {
        row[i].c = ('a' as u32 + (i as u32 % 26)) as u8 as char;
    }
    // WIDE_CHAR at position 116, spacer at 117.
    row[116].c = '中';
    row[116].flags.insert(Flags::WIDE_CHAR);
    row[117].flags.insert(Flags::WIDE_CHAR_SPACER);
    // Fill remaining positions to make row full (occ = cols_big).
    for i in 118..cols_big {
        row[i].c = 'x';
    }
    // Mark row as full so push_rows_internal sees it.
    row.occ = cols_big;
    row[cols_big - 1].flags.insert(Flags::WRAPLINE);

    assert!(row[116].flags().contains(Flags::WIDE_CHAR));
    assert!(row[117].flags().contains(Flags::WIDE_CHAR_SPACER));

    // Push into FlatStorage that has FEWER columns.
    let mut storage = FlatStorage::new(cols_small, None, None);
    storage.push_rows([&row]);

    // Materialization panics at row_iterator.rs:132.
    let _ = storage.pop_rows(1);
}

/// Fuzz for Hypothesis A: random wide rows pushed into narrow FlatStorage.
///
/// Builds valid rows at a wider column count, then pushes them into a
/// narrower FlatStorage — the exact column-mismatch scenario that can
/// happen in CLI Agent mode (resize changes flat_storage.columns but
/// subsequent scroll_region_up pushes grid rows at the old width).
#[test]
fn fuzz_column_mismatch_produces_corrupt_index() {
    use rand::Rng;
    use std::panic::catch_unwind;

    let mut rng = rand::thread_rng();
    const ITERATIONS: usize = 20_000;
    let mut panics = 0usize;

    for _ in 0..ITERATIONS {
        let cols_small: usize = rng.gen_range(5..51);
        let cols_big: usize = cols_small + rng.gen_range(1..31);

        // Build a cols_big-wide Row with a random layout.
        let mut row = Row::new(cols_big);
        let wide_pos: usize = rng.gen_range(0..cols_big - 1);
        for i in 0..cols_big {
            if i == wide_pos {
                // WIDE_CHAR at this position
                row[i].c = '中';
                row[i].flags.insert(Flags::WIDE_CHAR);
            } else if i == wide_pos + 1 {
                row[i].flags.insert(Flags::WIDE_CHAR_SPACER);
            } else {
                row[i].c = rng.gen_range('a'..='z');
            }
        }
        row.occ = cols_big;
        row[cols_big - 1].flags.insert(Flags::WRAPLINE);

        let mut storage = FlatStorage::new(cols_small, None, None);
        storage.push_rows([&row]);

        let result = catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = storage.pop_rows(1);
        }));

        if result.is_err() {
            panics += 1;
        }
    }

    // At least some column-mismatch combos should trigger the panic.
    // The exact count depends on random seed, but we need at least 1.
    assert!(panics > 0, "no column-mismatch scenarios triggered panic");
    eprintln!("column-mismatch fuzz: {panics}/{ITERATIONS} iterations panicked");
}

/// Focused fuzz: continuous wide characters only (no ASCII).
///
/// Pure CJK rows maximize the chance that a WIDE_CHAR lands on the
/// last column without a spacer.  With 100_000 random column pairs
/// in a wider range, this explicitly targets the Index::rebuild
/// reflow boundary.
///
/// Independent of `fuzz_reflow_wide_char_no_panic` — this one does
/// not mix in ASCII, so every grapheme is 2 cells wide.
#[test]
fn fuzz_reflow_continuous_wide_only() {
    const WIDE_CHARS: &[&str] = &["中", "说", "😀", "汉", "字", "语", "文", "明", "好", "和"];

    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut buf = String::new();
    const ITERATIONS: usize = 50_000;

    for _ in 0..ITERATIONS {
        buf.clear();

        // Wide columns matching real crash: crash text shows 155 cols,
        // production resize happens at 117.  Focus on these ranges.
        let cols_src: usize = rng.gen_range(50..201);
        let cols_dst: usize = rng.gen_range(50..201);
        let num_rows: usize = rng.gen_range(5..51);

        for _ in 0..num_rows {
            // Fill the row with wide chars only.  For an odd-width
            // column, the last cell position is always available for a
            // WIDE_CHAR to land on without a following spacer.
            let num_wide = cols_src / 2;
            for _ in 0..num_wide {
                let idx = rng.gen_range(0..WIDE_CHARS.len());
                buf.push_str(WIDE_CHARS[idx]);
            }
            buf.push('\n');
        }

        let rows = buf.as_str().to_rows(cols_src);
        let mut storage = FlatStorage::new(cols_src, None, None);
        storage.push_rows(&rows);

        if cols_src != cols_dst {
            storage.set_columns(cols_dst);
        }

        for row in storage.rows_from(0) {
            let row = row.as_ref();
            for col in 0..row.occ {
                if row[col].flags().contains(Flags::WIDE_CHAR) {
                    assert!(
                        col + 1 < row.len(),
                        "WIDE_CHAR at absolute last cell ({col}/{}) — \
                         no room for spacer! cols_src={cols_src} cols_dst={cols_dst}",
                        row.len(),
                    );
                    assert!(
                        row[col + 1].flags().contains(Flags::WIDE_CHAR_SPACER),
                        "WIDE_CHAR at {col} missing WIDE_CHAR_SPACER at {next} — \
                         cols_src={cols_src} cols_dst={cols_dst}",
                        next = col + 1,
                    );
                }
            }
        }
    }
}
