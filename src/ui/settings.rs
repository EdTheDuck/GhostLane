use egui::{Align, DragValue, Grid, Id, Layout, Modal, RichText, Slider, TextEdit};
use egui_phosphor::regular as icon;

use super::accent_button;
use crate::config::{DEFAULT_AI_COMMAND, Settings};
use crate::terminal::detect_shell;
use crate::theme;

pub struct SettingsForm {
    ai_command: String,
    shell: String,
    terminal_font_size: f32,
    ui_scale: f32,
    detected_shell: String,
}

pub enum SettingsOutcome {
    Cancel,
    Save(Settings),
}

impl SettingsForm {
    pub fn new(s: &Settings) -> Self {
        SettingsForm {
            ai_command: s.ai_command.clone(),
            shell: s.shell.clone().unwrap_or_default(),
            terminal_font_size: s.terminal_font_size,
            ui_scale: s.ui_scale,
            detected_shell: detect_shell(None).0,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<SettingsOutcome> {
        let mut outcome = None;
        let modal = Modal::new(Id::new("settings_modal")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(RichText::new("Settings").size(20.0).strong());
            ui.add_space(12.0);

            ui.label(RichText::new("AI command").strong());
            ui.add(
                TextEdit::singleline(&mut self.ai_command)
                    .code_editor()
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("Run by the ▶ button. Placeholders: {ticket_path}  {ticket_id}  {project_dir}")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                if ui.button("Reset").clicked() {
                    self.ai_command = DEFAULT_AI_COMMAND.into();
                }
            });
            ui.add_space(12.0);

            ui.label(RichText::new("Shell").strong());
            ui.horizontal(|ui| {
                ui.add(
                    TextEdit::singleline(&mut self.shell)
                        .hint_text(format!("Auto: {}", self.detected_shell))
                        .desired_width(ui.available_width() - 100.0),
                );
                if ui.button(format!("{}  Browse", icon::FOLDER_OPEN)).clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_file()
                {
                    self.shell = path.display().to_string();
                }
            });
            ui.label(
                RichText::new("Leave empty to detect automatically. Applies to newly started terminals.")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.add_space(12.0);

            Grid::new("settings_grid")
                .num_columns(2)
                .spacing([16.0, 10.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Terminal font size").strong());
                    ui.add(
                        DragValue::new(&mut self.terminal_font_size)
                            .range(8.0..=32.0)
                            .speed(0.2)
                            .suffix(" pt"),
                    );
                    ui.end_row();
                    ui.label(RichText::new("UI scale").strong());
                    ui.add(Slider::new(&mut self.ui_scale, 0.75..=2.0).step_by(0.05).suffix("×"));
                    ui.end_row();
                });

            ui.add_space(12.0);
            ui.separator();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if accent_button(ui, "Save").clicked() {
                    outcome = Some(SettingsOutcome::Save(self.to_settings()));
                }
                if ui.button("Cancel").clicked() {
                    outcome = Some(SettingsOutcome::Cancel);
                }
            });
        });
        if modal.should_close() && outcome.is_none() {
            outcome = Some(SettingsOutcome::Cancel);
        }
        outcome
    }

    fn to_settings(&self) -> Settings {
        let shell = self.shell.trim();
        Settings {
            ai_command: self.ai_command.trim().to_string(),
            shell: (!shell.is_empty()).then(|| shell.to_string()),
            terminal_font_size: self.terminal_font_size,
            ui_scale: self.ui_scale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_shell_means_auto_detect() {
        let mut form = SettingsForm::new(&Settings {
            shell: Some("fish".into()),
            ..Default::default()
        });
        form.shell = "   ".into();
        form.ai_command = "  my-ai {ticket_path} ".into();
        let s = form.to_settings();
        assert_eq!(s.shell, None);
        assert_eq!(s.ai_command, "my-ai {ticket_path}");
    }
}
