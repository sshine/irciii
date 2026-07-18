//! The startup splash: the irciii logo, embedded in the binary and rendered
//! as ANSI block art by [`graphix`], shown for a few seconds (any key skips).

use std::cell::RefCell;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

/// The logo PNG, embedded at compile time.
static LOGO_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../irciii-logo.png"
));

thread_local! {
    /// Cached cell grid for the last render size: (cols, rows, grid).
    #[expect(clippy::type_complexity, reason = "a simple cache triple")]
    static GRID: RefCell<Option<(u32, u32, Vec<Vec<graphix::Cell>>)>> =
        const { RefCell::new(None) };
}

/// Render the splash centered in the frame.
pub fn render(frame: &mut Frame<'_>) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    GRID.with(|cell| {
        let mut cache = cell.borrow_mut();
        let (cols, rows) = match &*cache {
            Some((c, r, _)) if grid_fits(*c, *r, area) => (*c, *r),
            _ => {
                let Ok(img) = image::load_from_memory(LOGO_PNG) else {
                    return;
                };
                let img = img.to_rgb8();
                let (w, h) = img.dimensions();
                let (cols, rows) =
                    graphix::fit_grid(w, h, u32::from(area.width), u32::from(area.height));
                *cache = Some((cols, rows, graphix::render_cells(&img, cols, rows)));
                (cols, rows)
            }
        };
        let Some((_, _, grid)) = &*cache else { return };

        let x0 = area.x + (area.width.saturating_sub(cols as u16)) / 2;
        let y0 = area.y + (area.height.saturating_sub(rows as u16)) / 2;
        let buf = frame.buffer_mut();
        for (dy, row) in grid.iter().enumerate() {
            for (dx, cell) in row.iter().enumerate() {
                let (x, y) = (x0 + dx as u16, y0 + dy as u16);
                if x < area.right()
                    && y < area.bottom()
                    && let Some(target) = buf.cell_mut((x, y))
                {
                    target.set_char(cell.ch);
                    target.set_style(
                        Style::default()
                            .fg(Color::Rgb(cell.fg.r, cell.fg.g, cell.fg.b))
                            .bg(Color::Rgb(cell.bg.r, cell.bg.g, cell.bg.b)),
                    );
                }
            }
        }
    });
}

/// Whether a cached grid still fits (recompute only when the size changes).
fn grid_fits(cols: u32, rows: u32, area: Rect) -> bool {
    let (want_cols, want_rows) = cached_fit(area);
    cols == want_cols && rows == want_rows
}

fn cached_fit(area: Rect) -> (u32, u32) {
    // Logo dimensions are fixed; recompute the target fit cheaply from them.
    // (Decoding only happens when the grid is actually rebuilt.)
    thread_local! {
        static DIMS: RefCell<Option<(u32, u32)>> = const { RefCell::new(None) };
    }
    let (w, h) = DIMS.with(|d| {
        *d.borrow_mut().get_or_insert_with(|| {
            image::load_from_memory(LOGO_PNG)
                .map(|img| img.to_rgb8().dimensions())
                .unwrap_or((1, 1))
        })
    });
    graphix::fit_grid(w, h, u32::from(area.width), u32::from(area.height))
}
