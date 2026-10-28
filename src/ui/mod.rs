pub mod board;
pub mod chrome;
pub mod epics;
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

/// Space between a modal's edge and its contents.
const MODAL_PADDING: f32 = 20.0;

/// Shows a modal with generous padding to its edge and extra room between items.
pub fn show_modal<R>(
    ctx: &egui::Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::ModalResponse<R> {
    let style = ctx.style_of(egui::Theme::Dark);
    let frame = egui::Frame::popup(&style).inner_margin(egui::Margin::same(MODAL_PADDING as i8));
    // Never taller than the window: contents that were taller last frame scroll. Only then, so contents sized from
    // the space they're given don't shrink a scroll area that sizes itself from them.
    // ponytail: height only; every modal is narrower than the window's 900px minimum width.
    let edges = 2.0 * (MODAL_PADDING + style.visuals.window_stroke.width);
    let fit = ctx.content_rect().height() - edges;
    let id = egui::Id::new(id);
    let tall = id.with("content_height");
    let overflowed = ctx.data(|d| d.get_temp::<f32>(tall)).is_some_and(|h| h > fit + 0.5);
    egui::Modal::new(id).frame(frame).show(ctx, |ui| {
        let contents = |ui: &mut Ui| {
            ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
            add_contents(ui)
        };
        let (inner, height) = if overflowed {
            let out = egui::ScrollArea::vertical()
                .id_salt("modal_scroll")
                .max_height(fit)
                .auto_shrink([true, false])
                .show(ui, contents);
            (out.inner, out.content_size.y)
        } else {
            let inner = contents(ui);
            (inner, ui.min_rect().height())
        };
        ui.ctx().data_mut(|d| d.insert_temp(tall, height));
        if overflowed != (height > fit + 0.5) {
            ui.ctx().request_repaint();
        }
        inner
    })
}

/// Modal title with an X in the top right corner. True when the X is clicked.
pub fn modal_header(ui: &mut Ui, title: &str, close_tip: &str) -> bool {
    let close = modal_close(ui, close_tip);
    ui.label(RichText::new(title).size(20.0).strong());
    close
}

/// The X in a modal's top right corner, tucked into the padding: the gap between the glyph and the modal's top
/// and right edges is half what it would be at the corner of the contents. It stays put while the contents scroll.
/// True when clicked.
pub fn modal_close(ui: &mut Ui, tooltip: &str) -> bool {
    const SIZE: f32 = 30.0;
    const GLYPH: f32 = 16.0;
    // The contents' top right corner; when they scroll, the top of the part showing.
    let max = ui.max_rect();
    let content = egui::Rect::from_min_max(egui::pos2(max.left(), max.top().max(ui.clip_rect().top())), max.max);
    // Laid out at the contents' corner, the glyph's gap to the edge is the border, the padding and the button's own
    // margin.
    let edge = MODAL_PADDING + ui.visuals().window_stroke.width;
    let gap = (edge + (SIZE - GLYPH) / 2.0) / 2.0;
    let inset = gap + GLYPH / 2.0 - edge;
    let rect = egui::Rect::from_center_size(
        egui::pos2(content.right() - inset, content.top() + inset),
        egui::vec2(SIZE, SIZE),
    );
    // A layer of its own just above the modal: half of it sits in the padding, outside a scrolling modal's clip, and
    // it must win over that modal's scroll bar.
    let parent = ui.layer_id();
    let corner = egui::Area::new(ui.id().with("modal_close"))
        .order(parent.order)
        .fixed_pos(rect.min)
        .show(ui.ctx(), |ui| {
            let (rect, response) = ui.allocate_exact_size(rect.size(), egui::Sense::click());
            paint_glyph(ui, rect, &response, icon::X, theme::TEXT, theme::ACCENT, false);
            pointer(response).on_hover_text(tooltip).clicked()
        });
    ui.ctx().set_sublayer(parent, corner.response.layer_id);
    corner.inner
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
    let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::click());
    let filled = fill_on_hover && (response.hovered() || response.has_focus());
    paint_glyph(ui, rect, &response, glyph, idle, hover, filled);
    pointer(response)
}

/// Icon-only button that always shows the filled Phosphor glyph, for a switch that is on.
pub fn solid_glyph_button(ui: &mut Ui, glyph: &str, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::click());
    paint_glyph(ui, rect, &response, glyph, color, color, true);
    pointer(response)
}

/// Draws an icon button's glyph in `rect`, `hover` coloured while `response` is hovered or focused, `filled` in the
/// filled Phosphor variant.
fn paint_glyph(
    ui: &Ui,
    rect: egui::Rect,
    response: &Response,
    glyph: &str,
    idle: Color32,
    hover: Color32,
    filled: bool,
) {
    let enabled = ui.is_enabled();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, glyph));
    if ui.is_rect_visible(rect) {
        let hot = response.hovered() || response.has_focus();
        let family = if filled {
            egui::FontFamily::Name(theme::ICON_FILL.into())
        } else {
            egui::FontFamily::Proportional
        };
        let color = if hot { hover } else { idle };
        let font = egui::FontId::new(16.0, family);
        ui.painter()
            .text(rect.center(), egui::Align2::CENTER_CENTER, glyph, font, color);
    }
}

/// Red-outlined button with red text for destructive actions; on hover it fills red and the text turns dark.
pub fn danger_button(ui: &mut Ui, text: &str) -> Response {
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    let size = (galley.size() + ui.spacing().button_padding * 2.0).max(egui::vec2(0.0, ui.spacing().interact_size.y));
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let enabled = ui.is_enabled();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, galley.text()));
    if ui.is_rect_visible(rect) {
        let (fill, text) = if response.hovered() {
            (theme::DANGER, theme::ACCENT_TEXT)
        } else {
            (Color32::TRANSPARENT, theme::DANGER)
        };
        let radius = ui.visuals().widgets.inactive.corner_radius;
        let painter = ui.painter();
        painter.rect(
            rect,
            radius,
            fill,
            egui::Stroke::new(1.0, theme::DANGER),
            egui::StrokeKind::Inside,
        );
        painter.galley_with_override_text_color(rect.center() - galley.size() / 2.0, galley, text);
    }
    pointer(response)
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
/// `picked`: `None` for a plain pill. Otherwise it is clickable and lights up in the accent colour on hover; a
/// picked pill (one the board is filtered by) stays lit and inverts on hover, ready to be unpicked.
pub fn label_chip(ui: &mut Ui, label: &str, picked: Option<bool>) -> Response {
    const PAD: egui::Vec2 = egui::vec2(10.0, 4.0);
    let galley = egui::WidgetText::from(RichText::new(label).small()).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Small,
    );
    let (sense, kind) = match picked {
        Some(_) => (egui::Sense::click(), egui::WidgetType::Button),
        None => (egui::Sense::hover(), egui::WidgetType::Label),
    };
    let (rect, response) = ui.allocate_exact_size(galley.size() + PAD * 2.0, sense);
    response.widget_info(|| egui::WidgetInfo::labeled(kind, true, galley.text()));
    if ui.is_rect_visible(rect) {
        let (fill, text, stroke) = match (picked, response.hovered()) {
            (Some(true), true) => (theme::ACCENT_TEXT, theme::ACCENT, egui::Stroke::new(1.0, theme::ACCENT)),
            (Some(true), false) | (Some(false), true) => (theme::ACCENT, theme::ACCENT_TEXT, egui::Stroke::NONE),
            _ => (theme::BORDER, theme::TEXT, egui::Stroke::NONE),
        };
        let painter = ui.painter();
        painter.rect(rect, rect.height() / 2.0, fill, stroke, egui::StrokeKind::Inside);
        painter.galley_with_override_text_color(rect.min + PAD, galley, text);
    }
    if picked.is_some() { pointer(response) } else { response }
}

/// A bar in `color` down the left edge of the card in `rect`, following its 6px rounded corners.
pub fn stripe(ui: &Ui, rect: egui::Rect, color: Color32) {
    let bar = egui::Rect::from_min_size(rect.min, egui::vec2(4.0, rect.height()));
    let corners = egui::CornerRadius {
        nw: 6,
        sw: 6,
        ..Default::default()
    };
    ui.painter().rect_filled(bar, corners, color);
}

/// An epic as a pill on a card: outlined in the epic colour with a flag, so it never reads as a label. It fills in
/// on hover, and stays filled while the board is filtered by it (`picked`). Long titles are cut short.
pub fn epic_chip(ui: &mut Ui, title: &str, picked: bool) -> Response {
    const PAD: egui::Vec2 = egui::vec2(8.0, 3.0);
    let max = (ui.available_width() - PAD.x * 2.0).max(40.0);
    let mut job = egui::text::LayoutJob::single_section(
        format!("{}  {title}", icon::FLAG),
        egui::TextFormat::simple(egui::TextStyle::Small.resolve(ui.style()), theme::EPIC),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(max);
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let (rect, response) = ui.allocate_exact_size(galley.size() + PAD * 2.0, egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title));
    if ui.is_rect_visible(rect) {
        let (fill, text) = if picked || response.hovered() {
            (theme::EPIC, theme::ACCENT_TEXT)
        } else {
            (Color32::TRANSPARENT, theme::EPIC)
        };
        let painter = ui.painter();
        painter.rect(
            rect,
            rect.height() / 2.0,
            fill,
            egui::Stroke::new(1.0, theme::EPIC),
            egui::StrokeKind::Inside,
        );
        painter.galley_with_override_text_color(rect.min + PAD, galley, text);
    }
    pointer(response)
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
