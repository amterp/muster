//! Reading a terminal cell by cell.
//!
//! Several FFI calls per cell, so this is for tests and oracles, never for a path that runs
//! per output chunk: the daemon reads text through the formatter.

use crate::ffi;
use crate::grid::{Cell, Color, Grid, Row, Style, Width};
use crate::state::Rgb;
use crate::terminal::Terminal;

impl Terminal {
    /// Reads the visible screen.
    ///
    /// The viewport rather than the active area, because the viewport is what a user is
    /// looking at, and that is the thing tests are supposed to assert on.
    pub fn viewport(&self, columns: u16, rows: u16) -> Grid {
        Grid {
            rows: (0..u32::from(rows))
                .map(|y| self.row(ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_VIEWPORT, y, columns))
                .collect(),
            cursor: self.cursor(),
        }
    }

    /// Every row of the active screen, history first.
    pub fn screen(&self) -> Vec<Row> {
        let columns = self.columns();
        let total = u32::try_from(self.total_rows()).unwrap_or(u32::MAX);
        (0..total)
            .map(|y| self.row(ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_SCREEN, y, columns))
            .collect()
    }

    /// The width of one cell of the active area, without reading its text or style.
    pub(crate) fn active_width(&self, x: u16, y: u16) -> Option<Width> {
        let grid_ref =
            self.grid_ref(ffi::GhosttyPointTag_GHOSTTY_POINT_TAG_ACTIVE, x, u32::from(y))?;
        let mut raw: ffi::GhosttyCell = 0;
        // SAFETY: the ref was just filled in by libghostty and the out parameter is ours.
        if unsafe { ffi::ghostty_grid_ref_cell(&raw const grid_ref, &raw mut raw) }
            != ffi::GhosttyResult_GHOSTTY_SUCCESS
        {
            return None;
        }
        Some(Width::from_raw(cell_data(raw, ffi::GhosttyCellData_GHOSTTY_CELL_DATA_WIDE, 0)))
    }

    fn row(&self, tag: ffi::GhosttyPointTag, y: u32, columns: u16) -> Row {
        let cells: Vec<Cell> = (0..columns).filter_map(|x| self.cell(tag, x, y)).collect();
        let wraps = self.grid_ref(tag, 0, y).is_some_and(|grid_ref| row_wraps(&grid_ref));
        Row { cells, wraps }
    }

    pub(crate) fn grid_ref(
        &self,
        tag: ffi::GhosttyPointTag,
        x: u16,
        y: u32,
    ) -> Option<ffi::GhosttyGridRef> {
        let point = ffi::GhosttyPoint {
            tag,
            value: ffi::GhosttyPointValue { coordinate: ffi::GhosttyPointCoordinate { x, y } },
        };
        let mut grid_ref = ffi::GhosttyGridRef { node: std::ptr::null_mut(), x: 0, y: 0 };
        // SAFETY: the point is fully initialized and the ref is a local we own.
        let found =
            unsafe { ffi::ghostty_terminal_grid_ref(self.handle(), point, &raw mut grid_ref) };
        (found == ffi::GhosttyResult_GHOSTTY_SUCCESS).then_some(grid_ref)
    }

    fn cell(&self, tag: ffi::GhosttyPointTag, x: u16, y: u32) -> Option<Cell> {
        let mut grid_ref = self.grid_ref(tag, x, y)?;

        let mut raw: ffi::GhosttyCell = 0;
        // SAFETY: the ref was just filled in by libghostty and the out parameter is ours.
        if unsafe { ffi::ghostty_grid_ref_cell(&raw const grid_ref, &raw mut raw) }
            != ffi::GhosttyResult_GHOSTTY_SUCCESS
        {
            return None;
        }

        let wide = cell_data(raw, ffi::GhosttyCellData_GHOSTTY_CELL_DATA_WIDE, 0);
        let protected = cell_data(raw, ffi::GhosttyCellData_GHOSTTY_CELL_DATA_PROTECTED, false);

        Some(Cell {
            text: graphemes(&mut grid_ref),
            width: Width::from_raw(wide),
            style: style(&grid_ref, raw),
            protected,
            hyperlink: hyperlink(&grid_ref),
        })
    }
}

/// One cell field. `fallback` fixes the out parameter's type, which must be the one
/// screen.h documents for `data`.
fn cell_data<T: Copy>(cell: ffi::GhosttyCell, data: ffi::GhosttyCellData, fallback: T) -> T {
    let mut value = fallback;
    // SAFETY: every caller pairs `data` with its documented output type.
    let result = unsafe { ffi::ghostty_cell_get(cell, data, (&raw mut value).cast()) };
    if result == ffi::GhosttyResult_GHOSTTY_SUCCESS { value } else { fallback }
}

fn row_wraps(grid_ref: &ffi::GhosttyGridRef) -> bool {
    let mut row: ffi::GhosttyRow = 0;
    // SAFETY: the ref was filled in by libghostty and the out parameter is ours.
    if unsafe { ffi::ghostty_grid_ref_row(grid_ref, &raw mut row) }
        != ffi::GhosttyResult_GHOSTTY_SUCCESS
    {
        return false;
    }
    let mut wraps = false;
    // SAFETY: ROW_DATA_WRAP writes a bool.
    unsafe {
        ffi::ghostty_row_get(
            row,
            ffi::GhosttyRowData_GHOSTTY_ROW_DATA_WRAP,
            (&raw mut wraps).cast(),
        );
    }
    wraps
}

// GhosttyStyleFlags' bits, written out because style.h spells them as casts, which bindgen
// leaves out. The header's layout: one attribute per bit from 0 to 7, the underline style in
// the three bits from 8.
const STYLE_FLAG_BOLD: u16 = 1 << 0;
const STYLE_FLAG_ITALIC: u16 = 1 << 1;
const STYLE_FLAG_FAINT: u16 = 1 << 2;
const STYLE_FLAG_BLINK: u16 = 1 << 3;
const STYLE_FLAG_INVERSE: u16 = 1 << 4;
const STYLE_FLAG_INVISIBLE: u16 = 1 << 5;
const STYLE_FLAG_STRIKETHROUGH: u16 = 1 << 6;
const STYLE_FLAG_OVERLINE: u16 = 1 << 7;
const STYLE_UNDERLINE_SHIFT: u16 = 8;
const STYLE_UNDERLINE_MASK: u16 = 0x7 << STYLE_UNDERLINE_SHIFT;

/// The cell's style, with an erase's background folded in.
///
/// An erase under a background color leaves cells that hold only that color, as a content
/// tag rather than a style, and a reader that looked only at the style would call them blank.
fn style(grid_ref: &ffi::GhosttyGridRef, cell: ffi::GhosttyCell) -> Style {
    // SAFETY: zeroed is the default style, all zero bytes as style.h documents.
    let mut raw: ffi::GhosttyStyle = unsafe { std::mem::zeroed() };
    // SAFETY: the ref was filled in by libghostty and the out parameter is ours.
    unsafe { ffi::ghostty_grid_ref_style(grid_ref, &raw mut raw) };

    let flag = |bit: u16| raw.flags & bit != 0;
    let mut style = Style {
        foreground: color(raw.fg_color),
        background: color(raw.bg_color),
        underline_color: color(raw.underline_color),
        bold: flag(STYLE_FLAG_BOLD),
        italic: flag(STYLE_FLAG_ITALIC),
        faint: flag(STYLE_FLAG_FAINT),
        blink: flag(STYLE_FLAG_BLINK),
        inverse: flag(STYLE_FLAG_INVERSE),
        invisible: flag(STYLE_FLAG_INVISIBLE),
        strikethrough: flag(STYLE_FLAG_STRIKETHROUGH),
        overline: flag(STYLE_FLAG_OVERLINE),
        underline: i32::from((raw.flags & STYLE_UNDERLINE_MASK) >> STYLE_UNDERLINE_SHIFT),
    };

    let tag = cell_data(
        cell,
        ffi::GhosttyCellData_GHOSTTY_CELL_DATA_CONTENT_TAG,
        ffi::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_CODEPOINT,
    );
    if tag == ffi::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE {
        let index = cell_data(cell, ffi::GhosttyCellData_GHOSTTY_CELL_DATA_COLOR_PALETTE, 0u8);
        style.background = Some(Color::Palette(index));
    } else if tag == ffi::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_RGB {
        let rgb = cell_data(
            cell,
            ffi::GhosttyCellData_GHOSTTY_CELL_DATA_COLOR_RGB,
            ffi::GhosttyColorRgb { r: 0, g: 0, b: 0 },
        );
        style.background = Some(Color::Rgb(Rgb { r: rgb.r, g: rgb.g, b: rgb.b }));
    }
    style
}

fn color(raw: ffi::GhosttyStyleColor) -> Option<Color> {
    match ffi::GhosttyStyleColorTag::from(raw.tag) {
        ffi::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_PALETTE => {
            // SAFETY: the tag says the palette member of the union is the live one.
            let index = unsafe { raw.value.palette };
            Some(Color::Palette(index))
        }
        ffi::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_RGB => {
            // SAFETY: the tag says the rgb member of the union is the live one.
            let rgb = unsafe { raw.value.rgb };
            Some(Color::Rgb(Rgb { r: rgb.r, g: rgb.g, b: rgb.b }))
        }
        _ => None,
    }
}

fn hyperlink(grid_ref: &ffi::GhosttyGridRef) -> Option<String> {
    let mut buffer = vec![0u8; 256];
    let mut length = 0usize;
    // SAFETY: the buffer is ours and its length is reported honestly; on OUT_OF_SPACE
    // libghostty writes the length it needs instead.
    let mut result = unsafe {
        ffi::ghostty_grid_ref_hyperlink_uri(
            grid_ref,
            buffer.as_mut_ptr(),
            buffer.len(),
            &raw mut length,
        )
    };
    if result == ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        buffer = vec![0u8; length];
        // SAFETY: as above, with the capacity libghostty asked for.
        result = unsafe {
            ffi::ghostty_grid_ref_hyperlink_uri(
                grid_ref,
                buffer.as_mut_ptr(),
                buffer.len(),
                &raw mut length,
            )
        };
    }
    if result != ffi::GhosttyResult_GHOSTTY_SUCCESS || length == 0 {
        return None;
    }
    buffer.truncate(length);
    Some(String::from_utf8_lossy(&buffer).into_owned())
}

/// The cell's whole grapheme cluster, not just its first codepoint.
///
/// A snapshot that dropped combining marks would render an agent's output as something the
/// user never saw, and would do it silently.
fn graphemes(grid_ref: &mut ffi::GhosttyGridRef) -> String {
    let mut codepoints = vec![0u32; 8];
    let mut count = 0usize;

    // SAFETY: the buffer is ours and its length is reported honestly; on
    // GHOSTTY_OUT_OF_SPACE libghostty writes the count it needs into `count` instead.
    let mut result = unsafe { read_graphemes(grid_ref, &mut codepoints, &raw mut count) };
    if result == ffi::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        codepoints = vec![0u32; count];
        // SAFETY: as above, now with the capacity libghostty asked for.
        result = unsafe { read_graphemes(grid_ref, &mut codepoints, &raw mut count) };
    }
    if result != ffi::GhosttyResult_GHOSTTY_SUCCESS {
        return String::new();
    }

    codepoints.iter().take(count).filter_map(|point| char::from_u32(*point)).collect()
}

unsafe fn read_graphemes(
    grid_ref: &mut ffi::GhosttyGridRef,
    codepoints: &mut [u32],
    count: *mut usize,
) -> ffi::GhosttyResult {
    // SAFETY: the caller guarantees `count` points at a usize it owns; the buffer is a live
    // slice for the duration of the call.
    unsafe {
        ffi::ghostty_grid_ref_graphemes(
            &raw const *grid_ref,
            codepoints.as_mut_ptr(),
            codepoints.len(),
            count,
        )
    }
}
