pub mod board;
pub mod project;
pub mod settings;
pub mod ticket;

use egui::{Button, Color32, Response, RichText, Ui};
use egui_phosphor::regular as icon;

use crate::theme;
use crate::ticket::Priority;

/// Frameless icon button that shows a frame on hover.
pub fn icon_button(ui: &mut Ui, glyph: &str, tooltip: &str) -> Response {
    ui.add(Button::new(RichText::new(glyph).size(16.0)).frame_when_inactive(false))
        .on_hover_text(tooltip)
}

pub fn accent_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        Button::new(RichText::new(text).color(theme::ACCENT_TEXT).strong())
            .fill(theme::ACCENT)
            .min_size(egui::vec2(0.0, 28.0)),
    )
}

pub fn priority_color(p: Priority) -> Color32 {
    match p {
        Priority::Low => Color32::from_rgb(0x6C, 0xA0, 0xDC),
        Priority::Medium => Color32::from_rgb(0xE0, 0xC0, 0x60),
        Priority::High => Color32::from_rgb(0xE8, 0x96, 0x4A),
        Priority::Highest => theme::DANGER,
    }
}

pub fn priority_icon(p: Priority) -> RichText {
    let glyph = match p {
        Priority::Low => icon::CARET_DOWN,
        Priority::Medium => icon::EQUALS,
        Priority::High => icon::CARET_UP,
        Priority::Highest => icon::CARET_DOUBLE_UP,
    };
    RichText::new(glyph).color(priority_color(p)).strong()
}
