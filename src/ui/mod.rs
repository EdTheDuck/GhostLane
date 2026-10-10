pub mod board;
pub mod project;
pub mod settings;
pub mod ticket;

use egui::{Button, Color32, Response, RichText, Ui};
use egui_phosphor::regular as icon;

use crate::theme;
use crate::ticket::Priority;

/// Pointer cursor on hover. egui only does this for its own `Button`, so other clickables opt in here.
pub fn pointer(response: Response) -> Response {
    if response.enabled() {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// Modal with generous padding between its content and its edge.
pub fn modal(ctx: &egui::Context, id: impl std::hash::Hash + std::fmt::Debug) -> egui::Modal {
    let frame = egui::Frame::popup(&ctx.style_of(egui::Theme::Dark)).inner_margin(egui::Margin::same(20));
    egui::Modal::new(egui::Id::new(id)).frame(frame)
}

/// Inner padding for every text input, in line with button padding.
pub const INPUT_MARGIN: egui::Margin = egui::Margin::symmetric(10, 6);

/// Frameless icon button that shows a frame on hover.
pub fn icon_button(ui: &mut Ui, glyph: &str, tooltip: &str) -> Response {
    glyph_button(ui, glyph, theme::TEXT, false).on_hover_text(tooltip)
}

/// Icon-only button: no background or border; the icon turns the accent colour on hover.
/// With `fill_on_hover` it also switches to the filled Phosphor glyph.
pub fn glyph_button(ui: &mut Ui, glyph: &str, idle: Color32, fill_on_hover: bool) -> Response {
    let enabled = ui.is_enabled();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, glyph));
    let response = pointer(response);
    if ui.is_rect_visible(rect) {
        let hot = response.hovered() || response.has_focus();
        let family = if hot && fill_on_hover {
            egui::FontFamily::Name(theme::ICON_FILL.into())
        } else {
            egui::FontFamily::Proportional
        };
        let color = if hot { theme::ACCENT } else { idle };
        let font = egui::FontId::new(16.0, family);
        ui.painter()
            .text(rect.center(), egui::Align2::CENTER_CENTER, glyph, font, color);
    }
    response
}

pub fn accent_button(ui: &mut Ui, text: &str) -> Response {
    ui.add(Button::new(RichText::new(text).color(theme::ACCENT_TEXT).strong()).fill(theme::ACCENT))
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
