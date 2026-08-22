//! The scrollable history viewport.
//!
//! Renders only the visible slice of the history: item heights are cached
//! (invalidated wholesale when width or render options change), and lines are
//! produced on demand by [`render_item`]. `scroll_offset` counts display lines
//! up from the bottom; 0 keeps the view pinned to the newest line.

use irciii_proto::ui_state::UiState;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use crate::render::{RenderOpts, render_item};

/// Ensure the height cache matches the current width and options generation.
/// Heights are filled lazily by `height_of` while walking visible items.
fn refresh_heights(state: &mut UiState, width: u16) {
    if state.heights_width != width || state.heights_gen != state.opts_gen {
        state.heights.clear();
        state.heights_width = width;
        state.heights_gen = state.opts_gen;
    }
    state.heights.resize(state.history.len(), None);
}

fn height_of(state: &mut UiState, idx: usize, width: u16, opts: &RenderOpts) -> u16 {
    if let Some(Some(h)) = state.heights.get(idx) {
        return *h;
    }
    let h = render_item(&state.history[idx], width, opts).len() as u16;
    state.heights[idx] = Some(h);
    h
}

/// Total display lines from the bottom of history to the top, capped at
/// `limit` (so we never measure more items than needed).
fn lines_from_bottom(state: &mut UiState, width: u16, opts: &RenderOpts, limit: usize) -> usize {
    let mut total = 0usize;
    for idx in (0..state.history.len()).rev() {
        total += height_of(state, idx, width, opts) as usize;
        if total >= limit {
            break;
        }
    }
    total
}

/// Clamp the scroll offset so we cannot scroll past the oldest line, and
/// anchor the view while scrolled: new arrivals grow the offset so the
/// visible lines stay put (and the status bar's "below" count grows).
pub fn clamp_scroll(state: &mut UiState, area_width: u16, area_height: u16) {
    let opts = RenderOpts::from_state(state);
    refresh_heights(state, area_width);
    if state.scroll_offset > 0 {
        for idx in state.anchored_len..state.history.len() {
            state.scroll_offset += height_of(state, idx, area_width, &opts) as usize;
        }
    }
    state.anchored_len = state.history.len();
    let viewport = area_height as usize;
    let want = state.scroll_offset + viewport;
    let available = lines_from_bottom(state, area_width, &opts, want + 1);
    state.scroll_offset = state.scroll_offset.min(available.saturating_sub(viewport));
}

/// Render the history into `area`, bottom-aligned, honoring `scroll_offset`.
pub fn render(state: &mut UiState, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let opts = RenderOpts::from_state(state);
    refresh_heights(state, area.width);

    let viewport = area.height as usize;
    // The window of display lines we want, counted from the bottom of history:
    // [scroll_offset, scroll_offset + viewport).
    let skip_bottom = state.scroll_offset;
    let want_top = skip_bottom + viewport;

    // Walk items newest→oldest until we have covered `want_top` display
    // lines, keeping each item's visible lines (possibly clipped at the
    // window edges). Items are collected newest-first, so reverse at item
    // granularity before flattening into top-down rows.
    let mut covered = 0usize; // display lines covered so far, from the bottom
    let mut visible: Vec<Vec<ratatui::text::Line<'static>>> = Vec::new();
    for idx in (0..state.history.len()).rev() {
        if covered >= want_top {
            break;
        }
        let h = height_of(state, idx, area.width, &opts) as usize;
        // An ignored item occupies no lines, so it neither advances `covered`
        // nor has anything to draw. Skipping here keeps the walk from
        // re-rendering the whole log once the break condition stops firing.
        if h == 0 {
            continue;
        }
        let item_top = covered + h;
        if item_top <= skip_bottom {
            covered = item_top;
            continue; // entirely below the window (scrolled past)
        }
        // Line j (0-based from the item's top) sits `item_top - j - 1` lines
        // from history bottom; keep those in [skip_bottom, want_top).
        let lines = render_item(&state.history[idx], area.width, &opts)
            .into_iter()
            .enumerate()
            .filter(|(j, _)| {
                let from_bottom = item_top - j - 1;
                from_bottom >= skip_bottom && from_bottom < want_top
            })
            .map(|(_, line)| line)
            .collect();
        visible.push(lines);
        covered = item_top;
    }
    let rows: Vec<ratatui::text::Line<'static>> = visible.into_iter().rev().flatten().collect();

    // Bottom-align: draw the last `viewport` rows into the area.
    let start_y = (viewport).saturating_sub(rows.len()) as u16;
    for (i, line) in rows.iter().enumerate() {
        let y = area.y + start_y + i as u16;
        if y < area.y + area.height {
            line.render(Rect::new(area.x, y, area.width, 1), buf);
        }
    }
}
