use egui::{Align, ComboBox, Layout, RichText, TextEdit};
use egui_phosphor::regular as icon;

use super::{accent_button, icon_button};
use crate::config::Settings;
use crate::project::AutoOrder;
use crate::terminal::{AI_CLIS, detect_ai_clis, detect_shell};
use crate::theme;

const FONT_SIZES: std::ops::RangeInclusive<u8> = 10..=20;

pub struct SettingsForm {
    ai_prompt: String,
    /// Ticked state per entry of `AI_CLIS`.
    clis: Vec<bool>,
    shell: String,
    terminal_font_size: f32,
    check_for_updates: bool,
    auto_order: AutoOrder,
    detected_shell: String,
    installed: Vec<String>,
}

pub enum SettingsOutcome {
    Cancel,
    Save(Settings),
}

impl SettingsForm {
    pub fn new(s: &Settings) -> Self {
        let enabled = s.enabled_clis();
        SettingsForm {
            ai_prompt: s.ai_prompt.clone(),
            clis: AI_CLIS.iter().map(|c| enabled.iter().any(|id| id == c.id)).collect(),
            shell: s.shell.clone().unwrap_or_default(),
            terminal_font_size: s.terminal_font_size,
            check_for_updates: s.check_for_updates,
            auto_order: s.auto_order,
            detected_shell: detect_shell(None).0,
            installed: detect_ai_clis(),
        }
    }

    /// Back to the defaults, with the AI CLIs found on this machine ticked.
    fn reset(&mut self) {
        let defaults = Settings {
            ai_clis: Some(detect_ai_clis()),
            ..Settings::default()
        };
        *self = SettingsForm::new(&defaults);
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<SettingsOutcome> {
        let mut outcome = None;
        let modal = super::show_modal(ctx, "settings_modal", |ui| {
            ui.set_width(560.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Settings").size(20.0).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, icon::X, "Close without saving").clicked() {
                        outcome = Some(SettingsOutcome::Cancel);
                    }
                });
            });
            ui.add_space(6.0);

            ui.label(RichText::new("AI CLIs").strong());
            ui.label(
                RichText::new("Projects choose one of the enabled tools for the ▶ button.")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            egui::Grid::new("ai_clis")
                .num_columns(2)
                .spacing([24.0, 6.0])
                .show(ui, |ui| {
                    for (n, (cli, on)) in AI_CLIS.iter().zip(&mut self.clis).enumerate() {
                        ui.horizontal(|ui| {
                            super::toggle(ui, on);
                            // The name toggles too, like a checkbox label.
                            let name = ui.add(egui::Label::new(cli.name).sense(egui::Sense::click()));
                            if super::pointer(name).clicked() {
                                *on = !*on;
                            }
                            if self.installed.iter().any(|id| id == cli.id) {
                                ui.label(RichText::new("installed").small().color(theme::ACCENT));
                            }
                        });
                        if n % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
            ui.add_space(8.0);

            ui.label(RichText::new("Default AI command").strong())
                .on_hover_text("Placeholders: {ticket_path}  {ticket_id}  {project_dir}");
            ui.add(
                TextEdit::singleline(&mut self.ai_prompt)
                    .margin(super::INPUT_MARGIN)
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                ui.label(RichText::new("Auto order").strong())
                    .on_hover_text("Which ticket columns set to Auto start next");
                let combo = ComboBox::from_id_salt("auto_order")
                    .selected_text(self.auto_order.label())
                    .show_ui(ui, |ui| {
                        for order in AutoOrder::ALL {
                            ui.selectable_value(&mut self.auto_order, order, order.label());
                        }
                    });
                super::pointer(combo.response);
            });
            ui.add_space(8.0);

            ui.label(RichText::new("Shell").strong());
            ui.horizontal(|ui| {
                ui.add(
                    TextEdit::singleline(&mut self.shell)
                        .margin(super::INPUT_MARGIN)
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
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                ui.label(RichText::new("Terminal font size").strong());
                let current = self.terminal_font_size.round() as u8;
                let combo = ComboBox::from_id_salt("terminal_font_size")
                    .selected_text(format!("{current} pt"))
                    .show_ui(ui, |ui| {
                        for size in FONT_SIZES {
                            let size = f32::from(size);
                            ui.selectable_value(&mut self.terminal_font_size, size, format!("{size} pt"));
                        }
                    });
                super::pointer(combo.response);
            });
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                super::toggle(ui, &mut self.check_for_updates);
                let label = ui.add(
                    egui::Label::new(RichText::new("Check for updates on startup").strong())
                        .sense(egui::Sense::click()),
                );
                if super::pointer(label).clicked() {
                    self.check_for_updates = !self.check_for_updates;
                }
            });

            ui.add_space(8.0);
            ui.separator();
            let valid = self.clis.iter().any(|on| *on);
            ui.horizontal(|ui| {
                if ui.button("Reset to defaults").clicked() {
                    self.reset();
                }
                if !valid {
                    ui.label(RichText::new("Turn on at least one AI CLI").color(theme::TEXT_MUTED));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add_enabled_ui(valid, |ui| accent_button(ui, "Save")).inner.clicked() {
                        outcome = Some(SettingsOutcome::Save(self.to_settings()));
                    }
                });
            });
        });
        if modal.should_close() && outcome.is_none() {
            outcome = Some(SettingsOutcome::Cancel);
        }
        outcome
    }

    fn to_settings(&self) -> Settings {
        let shell = self.shell.trim();
        let clis = AI_CLIS
            .iter()
            .zip(&self.clis)
            .filter(|(_, on)| **on)
            .map(|(c, _)| c.id.to_string())
            .collect();
        Settings {
            ai_prompt: self.ai_prompt.trim().to_string(),
            ai_clis: Some(clis),
            shell: (!shell.is_empty()).then(|| shell.to_string()),
            terminal_font_size: self.terminal_font_size,
            check_for_updates: self.check_for_updates,
            auto_order: self.auto_order,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_round_trips_settings() {
        let mut form = SettingsForm::new(&Settings {
            shell: Some("fish".into()),
            ai_clis: Some(vec!["codex".into(), "aider".into()]),
            ..Default::default()
        });
        assert_eq!(
            form.to_settings().ai_clis,
            Some(vec!["codex".to_string(), "aider".to_string()])
        );
        form.shell = "   ".into();
        form.ai_prompt = "  Fix {ticket_path} ".into();
        let s = form.to_settings();
        assert_eq!(s.shell, None);
        assert_eq!(s.ai_prompt, "Fix {ticket_path}");
    }

    #[test]
    fn reset_restores_defaults_with_detected_clis() {
        let mut form = SettingsForm::new(&Settings {
            ai_prompt: "custom".into(),
            terminal_font_size: 18.0,
            ai_clis: Some(vec!["aider".into()]),
            shell: Some("fish".into()),
            check_for_updates: false,
            auto_order: AutoOrder::IdAsc,
        });
        form.reset();
        let s = form.to_settings();
        assert_eq!(s.ai_prompt, crate::config::DEFAULT_AI_PROMPT);
        assert_eq!(s.terminal_font_size, 13.0);
        assert_eq!(s.shell, None);
        assert!(s.check_for_updates);
        assert_eq!(s.auto_order, AutoOrder::PriorityDesc);
        assert_eq!(s.ai_clis, Some(detect_ai_clis()));
    }

    #[test]
    fn clicking_a_cli_name_toggles_it() {
        use egui_kittest::kittest::Queryable;
        let settings = Settings {
            ai_clis: Some(vec!["claude".into()]),
            ..Default::default()
        };
        let mut h = egui_kittest::Harness::new_ui_state(
            |ui, f: &mut SettingsForm| _ = f.show(ui.ctx()),
            SettingsForm::new(&settings),
        );
        crate::theme::apply(&h.ctx);
        h.set_size(egui::vec2(900.0, 900.0));
        h.run_steps(3);
        h.get_by_label("Gemini CLI").click();
        h.run();
        let ids = h.state().to_settings().ai_clis.unwrap();
        assert_eq!(ids, ["claude", "gemini"]);
    }
}
