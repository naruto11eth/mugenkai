//! Cursor position reality-check tests.
//!
//! These tests pin down what `Terminal::cursor_position()` actually
//! returns after sequences the UI cares about — entering the alt
//! screen, hiding the cursor, claude-code-like full-screen redraws,
//! exiting alt screen, and `clear(1)`. The pane widget's cursor
//! splicer trusts these coordinates; if they drift out of the
//! visible grid we end up drawing the cursor below the viewport,
//! which is exactly what the post-`claude -c` "no cursor until I
//! run `clear`" bug looks like.
//!
//! Each test names the exact invariant the UI depends on so a
//! future regression surfaces with a specific failure instead of
//! a vague "cursor disappeared" bug report.
//!
//! Grid: 80x24 for everything so row/col math is easy to read.

use host_ghostty::{Formatter, OutputFormat, Terminal};

const COLS: u16 = 80;
const ROWS: u16 = 24;

fn new_term() -> Terminal {
    Terminal::new(COLS, ROWS, 1000).expect("construct terminal")
}

#[test]
fn cursor_starts_at_origin() {
    let term = new_term();
    let cur = term.cursor_position();
    assert_eq!((cur.col, cur.row), (0, 0));
    assert!(cur.visible, "DEC mode 25 defaults to on");
}

#[test]
fn cursor_row_stays_within_grid_after_many_newlines() {
    // Pushing past the bottom row shouldn't advance the cursor
    // row beyond rows-1 — the grid scrolls instead. If this ever
    // regresses, splice_cursor will try to draw the cursor on a
    // row that doesn't exist in the formatter output and the
    // "past all content" padding path will push the cursor below
    // the viewport (the claude-code bug symptom).
    let mut term = new_term();
    for _ in 0..200 {
        term.vt_write(b"line\r\n");
    }
    let cur = term.cursor_position();
    assert!(
        cur.row < ROWS,
        "cursor row {} escaped grid (rows={})",
        cur.row,
        ROWS
    );
}

#[test]
fn cursor_col_stays_within_grid_after_long_line() {
    let mut term = new_term();
    // 200 chars on a 80-col grid — should wrap, not leave col
    // pointing past the right edge.
    term.vt_write(&[b'x'; 200]);
    let cur = term.cursor_position();
    assert!(
        cur.col < COLS,
        "cursor col {} escaped grid (cols={})",
        cur.col,
        COLS
    );
}

#[test]
fn hide_cursor_then_show_round_trips_visibility() {
    let mut term = new_term();
    term.vt_write(b"\x1b[?25l");
    assert!(!term.cursor_position().visible, "ESC[?25l should hide");
    term.vt_write(b"\x1b[?25h");
    assert!(term.cursor_position().visible, "ESC[?25h should show");
}

#[test]
fn clear_homes_the_cursor_to_origin() {
    let mut term = new_term();
    term.vt_write(b"foo\r\nbar\r\nbaz");
    let before = term.cursor_position();
    assert!(
        before.row > 0 || before.col > 0,
        "precondition: cursor should have moved off origin"
    );

    // `clear(1)` emits ED 2 (clear screen) + CUP (home) — that's
    // literally all the user's fix was. Re-assert it lands the
    // cursor at (0, 0) so any change to ghostty's ED/CUP handling
    // breaks this test loudly.
    term.vt_write(b"\x1b[H\x1b[2J");
    let after = term.cursor_position();
    assert_eq!(
        (after.col, after.row),
        (0, 0),
        "clear should home the cursor"
    );
}

#[test]
fn cursor_after_alt_screen_exit_stays_within_primary_grid() {
    // Reproduces the claude-code exit flow:
    //   1. save cursor + enter alt screen (ESC[?1049h)
    //   2. hide cursor (ESC[?25l)
    //   3. redraw full screen in TUI mode
    //   4. leave alt screen + restore cursor (ESC[?1049l)
    //
    // After step 4 we're back on the primary screen. The cursor
    // must be inside the visible grid — otherwise splice_cursor
    // ends up drawing it below the viewport and the user sees
    // "no cursor until I run clear".
    let mut term = new_term();

    // Fill primary screen with a realistic shell scrollback.
    for i in 0..ROWS - 2 {
        term.vt_write(format!("$ cmd-{i}\r\n").as_bytes());
    }
    let primary_before = term.cursor_position();

    // Enter alt screen, hide cursor.
    term.vt_write(b"\x1b[?1049h\x1b[?25l");
    assert!(term.on_alt_screen(), "should be on alt screen after 1049h");

    // Full-screen TUI paint: position to (0,0) and emit a grid of
    // box-drawing chars like claude-code's UI.
    term.vt_write(b"\x1b[H");
    for r in 1..=ROWS {
        term.vt_write(format!("\x1b[{r};1H").as_bytes());
        term.vt_write(&[b'#'; COLS as usize]);
    }

    // Leave alt screen. Claude does NOT always send ESC[?25h.
    term.vt_write(b"\x1b[?1049l");
    assert!(
        !term.on_alt_screen(),
        "should be back on primary after 1049l"
    );

    let primary_after = term.cursor_position();
    assert!(
        primary_after.row < ROWS,
        "cursor row {} escaped primary grid (rows={}) after alt-screen exit",
        primary_after.row,
        ROWS
    );
    assert!(
        primary_after.col < COLS,
        "cursor col {} escaped primary grid (cols={}) after alt-screen exit",
        primary_after.col,
        COLS
    );
    // Cursor position should also restore close to where it was.
    // libghostty may snap to column 0 on alt-screen transitions,
    // so only assert row continuity.
    assert_eq!(
        primary_after.row, primary_before.row,
        "alt-screen round-trip should preserve cursor row"
    );
}

#[test]
fn formatter_trim_false_still_strips_trailing_blank_rows() {
    // Reality check: `trim=false` is a hint, not a guarantee of
    // emit-full-grid. libghostty-vt still stops after the last
    // non-blank row. pane_widget has to pad the markup to ROWS
    // lines itself before splicing the cursor — otherwise a
    // cursor on a logically-blank row past the last non-blank
    // line falls into splice_cursor's "past all content" branch
    // and ends up drawn below the viewport (the `claude -c`
    // cursor-disappears bug).
    //
    // If a future ghostty upgrade changes this behaviour, this
    // test flips red (expected=1 but emitted=ROWS) and the
    // pane_widget padding becomes redundant — at which point the
    // padding can be removed. We do NOT want silent drift.
    let mut term = new_term();
    term.vt_write(b"hi");

    let html = Formatter::new(&term, OutputFormat::Html, false)
        .and_then(|f| f.format())
        .expect("format html");
    let emitted_rows = html.matches('\n').count() + 1;
    assert_eq!(
        emitted_rows, 1,
        "expected trim=false to still trim trailing blanks; got {emitted_rows}-row output:\n{html}"
    );
}

#[test]
fn cursor_can_land_past_emitted_rows_after_alt_exit() {
    // Direct evidence of the failure shape behind the claude-code
    // cursor bug: after an alt-screen burst + exit, ghostty's
    // cursor row points to a row that the formatter does NOT
    // emit. pane_widget compensates by padding the markup to
    // `last_rows` lines before splicing.
    let mut term = new_term();

    term.vt_write(b"a\r\nb\r\nc\r\n");
    term.vt_write(b"\x1b[?1049h\x1b[?25l");
    term.vt_write(b"\x1b[H\x1b[2J");
    term.vt_write(b"alt-screen body");
    term.vt_write(b"\x1b[?1049l");

    let cur = term.cursor_position();
    let html = Formatter::new(&term, OutputFormat::Html, false)
        .and_then(|f| f.format())
        .expect("format html");
    let emitted_rows = html.matches('\n').count() + 1;

    assert!(
        (cur.row as usize) >= emitted_rows,
        "expected cursor row {} to be past emitted {} rows (HTML: {:?})",
        cur.row,
        emitted_rows,
        html,
    );
    assert!(
        cur.row < ROWS,
        "cursor row {} must still be inside the 0..{} grid",
        cur.row,
        ROWS
    );
}
