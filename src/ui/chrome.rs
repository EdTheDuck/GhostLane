//! Custom window chrome: the top bar doubles as the title bar. Windows and Linux get drawn
//! minimise / maximise / close buttons and invisible resize edges; macOS keeps its native buttons.

use egui::{
    Color32, CursorIcon, FontId, Id, Pos2, Rect, ResizeDirection, Sense, Stroke, Ui, ViewportCommand, pos2, vec2,
};

use crate::theme;

pub const BAR_HEIGHT: f32 = 44.0;
/// Room left of the bar's content for macOS's native traffic lights.
pub const MAC_BUTTONS_WIDTH: f32 = 80.0;
const BUTTON_WIDTH: f32 = 46.0;
const CLOSE_RED: Color32 = Color32::from_rgb(0xC4, 0x2B, 0x1C);
const TITLE: &str = "GhostLane";

pub fn native_buttons() -> bool {
    cfg!(target_os = "macos")
}

fn maximized(ui: &Ui) -> bool {
    ui.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false))
}

/// How far a maximised borderless window hangs off each screen edge (Windows extends it by the
/// hidden resize frame). The app pads itself inward by this much so nothing is cut off.
pub fn maximized_overflow(ctx: &egui::Context) -> f32 {
    if !cfg!(windows) {
        return 0.0;
    }
    ctx.input(|i| {
        let v = i.viewport();
        let (Some(true), Some(outer), Some(monitor)) = (v.maximized, v.outer_rect, v.monitor_size) else {
            return 0.0;
        };
        // The taskbar shrinks one axis; the other shows the overflow on both sides.
        let spill = (outer.width() - monitor.x).max(outer.height() - monitor.y);
        (spill / 2.0).clamp(0.0, 20.0)
    })
}

/// Dragging `rect` moves the window; double-clicking toggles maximise. Call before the bar's
/// widgets so they keep their clicks.
pub fn drag_area(ui: &mut Ui, rect: Rect) {
    let response = ui.interact(rect, Id::new("title_bar_drag"), Sense::click_and_drag());
    if response.drag_started() {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    if response.double_clicked() {
        let max = maximized(ui);
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!max));
    }
}

/// Minimise, maximise/restore and close, right to left, full bar height. Use in a right-to-left layout.
pub fn window_buttons(ui: &mut Ui) {
    let max = maximized(ui);
    if button(ui, Glyph::Close, "Close").clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Close);
    }
    let tip = if max { "Restore" } else { "Maximise" };
    if button(ui, if max { Glyph::Restore } else { Glyph::Maximise }, tip).clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!max));
    }
    if button(ui, Glyph::Minimise, "Minimise").clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Glyph {
    Minimise,
    Maximise,
    Restore,
    Close,
}

fn button(ui: &mut Ui, glyph: Glyph, tip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(BUTTON_WIDTH, ui.available_height()), Sense::click());
    let response = super::pointer(response).on_hover_text(tip);
    let hot = response.hovered();
    let (bg, fg) = match (glyph, hot) {
        (Glyph::Close, true) => (CLOSE_RED, Color32::WHITE),
        (_, true) => (theme::CARD_HOVER, theme::TEXT),
        _ => (Color32::TRANSPARENT, theme::TEXT),
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, bg);
    let c = rect.center();
    let s = Stroke::new(1.0, fg);
    let h = 5.0;
    match glyph {
        Glyph::Minimise => {
            painter.hline((c.x - h)..=(c.x + h), c.y, s);
        }
        Glyph::Maximise => {
            let square = Rect::from_center_size(c, vec2(2.0 * h, 2.0 * h));
            painter.rect_stroke(square, 0.0, s, egui::StrokeKind::Inside);
        }
        Glyph::Restore => {
            let front = Rect::from_center_size(c + vec2(-1.5, 1.5), vec2(8.0, 8.0));
            painter.rect_stroke(front, 0.0, s, egui::StrokeKind::Inside);
            // Back square peeking out top-right.
            let (l, t, r, b) = (
                front.left() + 3.0,
                front.top() - 3.0,
                front.right() + 3.0,
                front.bottom() - 3.0,
            );
            painter.line(
                vec![
                    pos2(l, front.top()),
                    pos2(l, t),
                    pos2(r, t),
                    pos2(r, b),
                    pos2(front.right(), b),
                ],
                s,
            );
        }
        Glyph::Close => {
            painter.line_segment([c + vec2(-h, -h), c + vec2(h, h)], s);
            painter.line_segment([c + vec2(-h, h), c + vec2(h, -h)], s);
        }
    }
    response
}

/// Invisible strips along the window edges that resize the borderless window, plus a thin outline.
/// Call last, with the root `Ui`, so the strips sit above everything else.
pub fn resize_edges(ui: &mut Ui) {
    if native_buttons() || maximized(ui) {
        return;
    }
    let r = ui.ctx().content_rect();
    ui.painter()
        .rect_stroke(r, 0.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
    let (e, c) = (5.0, 12.0); // edge thickness, corner size
    let zones = [
        (
            Rect::from_min_max(pos2(r.left() + c, r.top()), pos2(r.right() - c, r.top() + e)),
            ResizeDirection::North,
            CursorIcon::ResizeVertical,
        ),
        (
            Rect::from_min_max(pos2(r.left() + c, r.bottom() - e), pos2(r.right() - c, r.bottom())),
            ResizeDirection::South,
            CursorIcon::ResizeVertical,
        ),
        (
            Rect::from_min_max(pos2(r.left(), r.top() + c), pos2(r.left() + e, r.bottom() - c)),
            ResizeDirection::West,
            CursorIcon::ResizeHorizontal,
        ),
        (
            Rect::from_min_max(pos2(r.right() - e, r.top() + c), pos2(r.right(), r.bottom() - c)),
            ResizeDirection::East,
            CursorIcon::ResizeHorizontal,
        ),
        (
            Rect::from_min_size(r.left_top(), vec2(c, c)),
            ResizeDirection::NorthWest,
            CursorIcon::ResizeNwSe,
        ),
        (
            Rect::from_min_size(pos2(r.right() - c, r.top()), vec2(c, c)),
            ResizeDirection::NorthEast,
            CursorIcon::ResizeNeSw,
        ),
        (
            Rect::from_min_size(pos2(r.left(), r.bottom() - c), vec2(c, c)),
            ResizeDirection::SouthWest,
            CursorIcon::ResizeNeSw,
        ),
        (
            Rect::from_min_size(r.right_bottom() - vec2(c, c), vec2(c, c)),
            ResizeDirection::SouthEast,
            CursorIcon::ResizeNwSe,
        ),
    ];
    for (i, (zone, direction, cursor)) in zones.into_iter().enumerate() {
        let response = ui.interact(zone, Id::new(("window_resize", i)), Sense::drag());
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(cursor);
        }
        if response.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(direction));
        }
    }
}

/// Ghost and "GhostLane" centred in `bar`, kept clear of the content between `left_end` and
/// `right_start`, with the version in small text after it. While `busy`, the letters become dots that the ghost
/// eats, on a loop, and the version hides.
pub fn title(ui: &Ui, bar: Rect, left_end: f32, right_start: f32, busy: bool) {
    const GHOST: f32 = 26.0;
    const GAP: f32 = 8.0;
    let galley = ui
        .painter()
        .layout_no_wrap(TITLE.into(), FontId::proportional(19.0), theme::TEXT);
    let width = GHOST + GAP + galley.size().x;
    // Centre on the window; slide into the free space if the bar's content would overlap.
    let free = (left_end + 12.0)..=(right_start - 12.0);
    if free.end() - free.start() < GHOST {
        return;
    }
    let mut x = bar.center().x - width / 2.0;
    x = x.min(free.end() - width).max(*free.start());
    let y = bar.center().y;
    let text_pos = pos2(x + GHOST + GAP, y - galley.size().y / 2.0);
    let ghost_center = if busy {
        // The ghost travels from its usual spot to just past the last letter, then starts over.
        let time = ui.input(|i| i.time) as f32;
        let progress = (time % 2.4) / 2.4;
        let travel = GHOST + GAP + galley.size().x;
        ui.ctx().request_repaint();
        pos2(x + GHOST / 2.0 + progress * travel, y + (time * 9.0).sin() * 1.5)
    } else {
        pos2(x + GHOST / 2.0, y)
    };
    let painter = ui.painter();
    if busy {
        // One dot where each letter was; dots behind the ghost have been eaten.
        for row in &galley.rows {
            for glyph in &row.glyphs {
                let dot: Pos2 = text_pos + vec2(glyph.pos.x + glyph.advance_width / 2.0, galley.size().y / 2.0);
                if dot.x > ghost_center.x + 6.0 {
                    painter.circle_filled(dot, 2.5, theme::ACCENT);
                }
            }
        }
    } else if x + width <= *free.end() {
        let version = painter.layout_no_wrap(
            format!("v{}", env!("CARGO_PKG_VERSION")),
            FontId::proportional(11.0),
            theme::TEXT_MUTED,
        );
        // Bottoms lined up, so it sits on the title's baseline. Left out when it doesn't fit.
        let version_pos = text_pos + vec2(galley.size().x + 5.0, galley.size().y - version.size().y - 2.0);
        if version_pos.x + version.size().x <= *free.end() {
            painter.galley(version_pos, version, theme::TEXT_MUTED);
        }
        painter.galley(text_pos, galley, theme::TEXT);
    }
    egui::Image::new(egui::include_image!("../../assets/ghost.svg"))
        .paint_at(ui, Rect::from_center_size(ghost_center, vec2(GHOST, GHOST)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    fn texts(busy: bool) -> Vec<String> {
        let mut h = Harness::new_ui(move |ui| {
            let bar = Rect::from_min_size(Pos2::ZERO, vec2(900.0, BAR_HEIGHT));
            title(ui, bar, 0.0, 900.0, busy);
        });
        h.step();
        h.output()
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn version_follows_the_title_and_hides_while_busy() {
        let version = format!("v{}", env!("CARGO_PKG_VERSION"));
        // No image loader here, so the ghost paints as a ⚠; only the text matters.
        assert!(texts(false).starts_with(&[version.clone(), TITLE.to_string()]));
        let busy = texts(true);
        assert!(
            !busy.contains(&version) && !busy.contains(&TITLE.to_string()),
            "{busy:?}"
        );
    }
}
