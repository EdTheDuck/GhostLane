//! "Tickets from a document" modal: pick or drop a spec, design doc or project plan for the AI to break into tickets.

use std::path::PathBuf;

use egui::{Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke};
use egui_phosphor::regular as icon;

use super::accent_button;
use crate::theme;

pub enum ImportOutcome {
    Close,
    Generate(PathBuf),
}

/// Shown over the board while a file is dragged over the window.
pub fn drop_hint(ctx: &egui::Context, board: egui::Rect) {
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop_hint")));
    painter.rect(
        board,
        8.0,
        egui::Color32::from_black_alpha(150),
        Stroke::new(2.0, theme::ACCENT),
        egui::StrokeKind::Inside,
    );
    painter.text(
        board.center(),
        egui::Align2::CENTER_CENTER,
        format!(
            "{}  Drop a spec, design doc or plan to generate tickets",
            icon::UPLOAD_SIMPLE
        ),
        egui::FontId::proportional(18.0),
        theme::TEXT,
    );
}

#[derive(Default)]
pub struct ImportForm {
    pub path: Option<PathBuf>,
    pub error: Option<String>,
}

impl ImportForm {
    pub fn show(&mut self, ctx: &egui::Context) -> Option<ImportOutcome> {
        let mut outcome = None;
        let (hovering, dropped) = ctx.input(|i| {
            let dropped = i.raw.dropped_files.iter().find_map(|f| f.path.clone());
            (!i.raw.hovered_files.is_empty(), dropped)
        });
        if let Some(path) = dropped {
            self.path = Some(path);
            self.error = None;
        }
        let modal = super::show_modal(ctx, "import_document", |ui| {
            ui.set_width(520.0);
            if super::modal_header(ui, "Tickets from a Document", "Close") {
                outcome = Some(ImportOutcome::Close);
            }
            ui.label(
                RichText::new(
                    "Choose a technical spec, design document or project plan. Your AI reads it and writes a \
                     ticket for each piece of work into the first column.",
                )
                .color(theme::TEXT_MUTED),
            );
            let accent = if hovering { theme::ACCENT } else { theme::BORDER };
            Frame::new()
                .fill(theme::BG)
                .stroke(Stroke::new(1.5, accent))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::same(24))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.vertical_centered(|ui| {
                        let glyph = if hovering { theme::ACCENT } else { theme::TEXT_MUTED };
                        ui.label(RichText::new(icon::UPLOAD_SIMPLE).size(32.0).color(glyph));
                        match &self.path {
                            Some(p) => {
                                let name = p.file_name().unwrap_or_default().to_string_lossy();
                                ui.label(RichText::new(name).strong())
                                    .on_hover_text(p.display().to_string());
                            }
                            None => {
                                ui.label("Drop a file here");
                            }
                        }
                        if ui.button(format!("{}  Choose File", icon::FOLDER_OPEN)).clicked()
                            && let Some(p) = rfd::FileDialog::new().pick_file()
                        {
                            self.path = Some(p);
                            self.error = None;
                        }
                    });
                });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if let Some(err) = &self.error {
                    ui.label(RichText::new(err).color(theme::DANGER));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let generate = ui.add_enabled_ui(self.path.is_some(), |ui| accent_button(ui, "Generate Tickets"));
                    if let Some(p) = &self.path
                        && generate.inner.clicked()
                    {
                        outcome = Some(ImportOutcome::Generate(p.clone()));
                    }
                    if ui.button("Cancel").clicked() {
                        outcome = Some(ImportOutcome::Close);
                    }
                });
            });
        });
        if modal.should_close() && outcome.is_none() {
            outcome = Some(ImportOutcome::Close);
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn dropped_file_is_picked_and_generated() {
        let mut h = Harness::new_ui_state(
            |ui, (form, out): &mut (ImportForm, Option<PathBuf>)| {
                if let Some(ImportOutcome::Generate(p)) = form.show(ui.ctx()) {
                    *out = Some(p);
                }
            },
            (ImportForm::default(), None),
        );
        crate::theme::apply(&h.ctx);
        h.run();
        h.input_mut().dropped_files.push(egui::DroppedFile {
            path: Some(PathBuf::from("/docs/spec.md")),
            ..Default::default()
        });
        h.run();
        h.get_by_label("spec.md");
        h.get_by_label("Generate Tickets").click();
        h.run();
        assert_eq!(h.state().1.as_deref(), Some(std::path::Path::new("/docs/spec.md")));
    }
}
