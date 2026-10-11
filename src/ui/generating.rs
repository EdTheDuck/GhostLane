//! "Generating" / "Updating" card shown over the board while the AI CLI writes a ticket. It covers only the
//! board so the terminal stays usable for any permission prompts the CLI asks.

use egui::{Align2, Area, Button, Color32, FontId, Id, Order, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::theme;

/// Draws the overlay over `board`. Returns true when the user stops waiting.
pub fn overlay(ctx: &egui::Context, board: Rect, title: &str) -> bool {
    let mut stop = false;
    Area::new(Id::new("generating_overlay"))
        .order(Order::Foreground)
        .fixed_pos(board.min)
        .show(ctx, |ui| {
            // Swallow clicks so the board underneath can't be used meanwhile.
            ui.allocate_exact_size(board.size(), Sense::click_and_drag());
            let painter = ui.painter();
            painter.rect_filled(board, 0.0, Color32::from_black_alpha(150));

            let card = Rect::from_center_size(board.center(), vec2(440.0, 230.0));
            painter.rect(
                card,
                12.0,
                theme::PANEL,
                Stroke::new(1.0, theme::BORDER),
                StrokeKind::Inside,
            );

            // The ghost glides along a trail of dots, eating each one it reaches.
            let time = ui.input(|i| i.time) as f32;
            let y = card.top() + 70.0;
            let (start, end) = (card.left() + 50.0, card.right() - 50.0);
            let progress = (time % 2.6) / 2.6;
            let ghost_x = egui::lerp((start - 20.0)..=(end + 20.0), progress);
            let mut x = start;
            while x <= end {
                if x > ghost_x + 8.0 {
                    painter.circle_filled(pos2(x, y), 3.0, theme::ACCENT.gamma_multiply(0.85));
                }
                x += 22.0;
            }
            let bob = (time * 8.0).sin() * 2.0;
            egui::Image::new(egui::include_image!("../../assets/ghost.svg"))
                .paint_at(ui, Rect::from_center_size(pos2(ghost_x, y + bob), vec2(38.0, 38.0)));

            let painter = ui.painter();
            painter.text(
                pos2(card.center().x, y + 48.0),
                Align2::CENTER_CENTER,
                title,
                FontId::proportional(19.0),
                theme::TEXT,
            );
            painter.text(
                pos2(card.center().x, y + 76.0),
                Align2::CENTER_CENTER,
                "Your AI is writing it in the terminal below.",
                FontId::proportional(13.0),
                theme::TEXT_MUTED,
            );
            painter.text(
                pos2(card.center().x, y + 94.0),
                Align2::CENTER_CENTER,
                "Approve any prompts it shows there.",
                FontId::proportional(13.0),
                theme::TEXT_MUTED,
            );
            let button = Rect::from_center_size(pos2(card.center().x, card.bottom() - 34.0), vec2(140.0, 32.0));
            stop = super::pointer(ui.put(button, Button::new("Stop waiting"))).clicked();
        });
    ctx.request_repaint();
    stop
}
