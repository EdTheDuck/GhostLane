#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // no console window in release

mod app;
mod config;
mod project;
mod terminal;
mod ticket;
mod ui;

pub const GHOST_SVG: &[u8] = include_bytes!("../assets/ghost.svg");

pub mod theme {
    use egui::{Color32, CornerRadius, Stroke, Visuals};

    pub const BG: Color32 = Color32::from_rgb(0x1E, 0x1F, 0x22);
    pub const PANEL: Color32 = Color32::from_rgb(0x2B, 0x2D, 0x30);
    pub const CARD: Color32 = Color32::from_rgb(0x35, 0x37, 0x3B);
    pub const CARD_HOVER: Color32 = Color32::from_rgb(0x3E, 0x40, 0x45);
    pub const BORDER: Color32 = Color32::from_rgb(0x45, 0x47, 0x4C);
    pub const TEXT: Color32 = Color32::from_rgb(0xDC, 0xDD, 0xDE);
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x9A, 0x9D, 0xA3);
    pub const ACCENT: Color32 = Color32::from_rgb(0xA8, 0xE6, 0xA1);
    pub const ACCENT_TEXT: Color32 = BG;
    pub const DANGER: Color32 = Color32::from_rgb(0xE0, 0x5A, 0x5A);

    pub fn visuals() -> Visuals {
        let mut v = Visuals::dark();
        v.override_text_color = Some(TEXT);
        v.panel_fill = PANEL;
        v.window_fill = PANEL;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.extreme_bg_color = BG;
        v.faint_bg_color = CARD;
        v.code_bg_color = BG;
        v.hyperlink_color = ACCENT;
        v.error_fg_color = DANGER;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        v.text_cursor.stroke = Stroke::new(2.0, ACCENT);
        v.window_corner_radius = CornerRadius::same(10);

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = PANEL;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        for (state, fill) in [
            (&mut w.inactive, CARD),
            (&mut w.hovered, CARD_HOVER),
            (&mut w.active, CARD_HOVER),
            (&mut w.open, CARD_HOVER),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.fg_stroke = Stroke::new(1.0, TEXT);
            state.corner_radius = CornerRadius::same(6);
        }
        w.inactive.bg_stroke = Stroke::NONE;
        w.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.6));
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        w.open.bg_stroke = Stroke::new(1.0, BORDER);
        v
    }
}

fn window_icon() -> Option<egui::IconData> {
    let size = egui::SizeHint::Size {
        width: 256,
        height: 256,
        maintain_aspect_ratio: true,
    };
    let img = egui_extras::image::load_svg_bytes_with_size(GHOST_SVG, size, &Default::default()).ok()?;
    Some(egui::IconData {
        rgba: img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect(),
        width: img.size[0] as u32,
        height: img.size[1] as u32,
    })
}

fn main() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("GhostLane")
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([640.0, 420.0]);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    eframe::run_native(
        "GhostLane",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
