pub mod board;
pub mod chrome;
pub mod generating;
pub mod import;
pub mod project;
pub mod queue;
pub mod quick_open;
pub mod rich_editor;
pub mod settings;
pub mod stats;
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

/// Shows a modal with generous padding to its edge and extra room between items.
pub fn show_modal<R>(
    ctx: &egui::Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::ModalResponse<R> {
    let frame = egui::Frame::popup(&ctx.style_of(egui::Theme::Dark)).inner_margin(egui::Margin::same(20));
    egui::Modal::new(egui::Id::new(id)).frame(frame).show(ctx, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
        add_contents(ui)
    })
}

/// Modal title with an X on the right. True when the X is clicked.
pub fn modal_header(ui: &mut Ui, title: &str, close_tip: &str) -> bool {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(20.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            icon_button(ui, icon::X, close_tip).clicked()
        })
        .inner
    })
    .inner
}

/// A switch with a label that toggles it too, like a checkbox label.
/// Returns the switch and label together, for a shared tooltip.
pub fn switch_row(ui: &mut Ui, on: &mut bool, text: impl Into<egui::WidgetText>) -> Response {
    ui.horizontal(|ui| {
        let switch = toggle(ui, on);
        let label = pointer(ui.add(egui::Label::new(text).sense(egui::Sense::click())));
        if label.clicked() {
            *on = !*on;
        }
        switch.union(label)
    })
    .inner
}

/// Drag handle for reorderable rows.
pub fn drag_handle(ui: &mut Ui) {
    ui.label(
        RichText::new(icon::DOTS_SIX_VERTICAL)
            .size(18.0)
            .color(theme::TEXT_MUTED),
    );
}

/// On/off switch. Track turns the accent colour when on; the knob slides across.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(36.0, 20.0), egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let (enabled, value) = (ui.is_enabled(), *on);
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, value, ""));
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let radius = rect.height() / 2.0;
        let track = if *on { theme::ACCENT } else { theme::CARD_HOVER };
        let knob = if *on { theme::ACCENT_TEXT } else { theme::TEXT_MUTED };
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        let painter = ui.painter();
        painter.rect_filled(rect, radius, track);
        painter.circle_filled(egui::pos2(x, rect.center().y), radius - 3.0, knob);
    }
    pointer(response)
}

/// Inner padding for every text input, in line with button padding.
pub const INPUT_MARGIN: egui::Margin = egui::Margin::symmetric(10, 12);

/// Frameless icon button that shows a frame on hover.
pub fn icon_button(ui: &mut Ui, glyph: &str, tooltip: &str) -> Response {
    glyph_button(ui, glyph, theme::TEXT, theme::ACCENT, false).on_hover_text(tooltip)
}

/// Icon button for destructive actions: turns red on hover.
pub fn danger_icon_button(ui: &mut Ui, glyph: &str, tooltip: &str) -> Response {
    glyph_button(ui, glyph, theme::TEXT, theme::DANGER, false).on_hover_text(tooltip)
}

/// Icon-only button: no background or border; the icon turns `hover` colour on hover.
/// With `fill_on_hover` it also switches to the filled Phosphor glyph.
pub fn glyph_button(ui: &mut Ui, glyph: &str, idle: Color32, hover: Color32, fill_on_hover: bool) -> Response {
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
        let color = if hot { hover } else { idle };
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

/// A ticket label as a pill. Laid out on one line and sized to its text, so it never wraps or gets clipped.
/// `clickable` pills light up in the accent colour on hover.
pub fn label_chip(ui: &mut Ui, label: &str, clickable: bool) -> Response {
    const PAD: egui::Vec2 = egui::vec2(10.0, 4.0);
    let galley = egui::WidgetText::from(RichText::new(label).small().color(theme::TEXT)).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Small,
    );
    let sense = if clickable {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(galley.size() + PAD * 2.0, sense);
    let kind = if clickable {
        egui::WidgetType::Button
    } else {
        egui::WidgetType::Label
    };
    response.widget_info(|| egui::WidgetInfo::labeled(kind, true, galley.text()));
    if ui.is_rect_visible(rect) {
        let hot = clickable && response.hovered();
        let (fill, text) = if hot {
            (theme::ACCENT, theme::ACCENT_TEXT)
        } else {
            (theme::BORDER, theme::TEXT)
        };
        ui.painter().rect_filled(rect, rect.height() / 2.0, fill);
        ui.painter().galley(rect.min + PAD, galley, text);
    }
    if clickable { pointer(response) } else { response }
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
