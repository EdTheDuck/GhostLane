use std::collections::HashMap;
use std::path::PathBuf;

use egui::{Align, ComboBox, Id, Layout, RichText, TextEdit, Ui};
use egui_dnd::{DragDropItem, dnd};
use egui_phosphor::regular as icon;

use super::{accent_button, icon_button};
use crate::project::{ColumnEdit, DEFAULT_COLUMNS, Project, find_ghost_file, folder_name, validate_columns};
use crate::theme;

pub struct ColumnRow {
    key: u64,
    name: String,
    orig_folder: Option<String>,
    tickets: usize,
}

impl DragDropItem for &mut ColumnRow {
    fn id(&self) -> Id {
        Id::new(("column_row", self.key))
    }
}

/// A column removed while editing that still holds tickets.
struct Removed {
    row: ColumnRow,
    target: Option<u64>,
}

pub struct ProjectForm {
    editing: bool,
    name: String,
    location: Option<PathBuf>,
    rows: Vec<ColumnRow>,
    removed: Vec<Removed>,
    next_key: u64,
    pub error: Option<String>,
}

pub enum ProjectOutcome {
    Cancel,
    Create {
        dir: PathBuf,
        name: String,
        columns: Vec<String>,
    },
    Save {
        name: String,
        edits: Vec<ColumnEdit>,
        targets: HashMap<String, usize>,
    },
    OpenExisting(PathBuf),
}

impl ProjectForm {
    pub fn new_project() -> Self {
        let rows = DEFAULT_COLUMNS
            .iter()
            .enumerate()
            .map(|(i, n)| ColumnRow {
                key: i as u64,
                name: n.to_string(),
                orig_folder: None,
                tickets: 0,
            })
            .collect();
        ProjectForm {
            editing: false,
            name: String::new(),
            location: None,
            rows,
            removed: Vec::new(),
            next_key: DEFAULT_COLUMNS.len() as u64,
            error: None,
        }
    }

    pub fn edit(p: &Project) -> Self {
        let rows: Vec<ColumnRow> = p
            .meta
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| ColumnRow {
                key: i as u64,
                name: c.name.clone(),
                orig_folder: Some(c.folder.clone()),
                tickets: p.tickets[i].len(),
            })
            .collect();
        ProjectForm {
            editing: true,
            name: p.meta.name.clone(),
            location: Some(p.root.clone()),
            next_key: rows.len() as u64,
            rows,
            removed: Vec::new(),
            error: None,
        }
    }

    fn validation(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("Enter a project name".into());
        }
        if self.location.is_none() {
            return Err("Choose a location for the project".into());
        }
        let names: Vec<String> = self.rows.iter().map(|r| r.name.clone()).collect();
        validate_columns(&names)?;
        if self.removed.iter().any(|r| r.target.is_none()) {
            return Err("Choose where to move tickets from removed columns".into());
        }
        Ok(())
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<ProjectOutcome> {
        let mut outcome = None;
        let modal = super::show_modal(ctx, "project_modal", |ui| {
            ui.set_width(540.0);
            let title = if self.editing { "Edit Project" } else { "New Project" };
            ui.label(RichText::new(title).size(20.0).strong());
            ui.add_space(12.0);

            ui.label("Name");
            ui.add(
                TextEdit::singleline(&mut self.name)
                    .margin(super::INPUT_MARGIN)
                    .hint_text("My Project")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(8.0);

            if !self.editing {
                self.location_ui(ui, &mut outcome);
                ui.add_space(8.0);
            }

            ui.horizontal(|ui| {
                ui.label("Columns");
                ui.label(RichText::new("drag to reorder").color(theme::TEXT_MUTED).small());
            });
            self.columns_ui(ui);
            if ui.button(format!("{}  Add column", icon::PLUS)).clicked() {
                self.rows.push(ColumnRow {
                    key: self.next_key,
                    name: "New Column".into(),
                    orig_folder: None,
                    tickets: 0,
                });
                self.next_key += 1;
            }
            self.removed_ui(ui);

            ui.add_space(12.0);
            let valid = self.validation();
            if let Some(err) = &self.error {
                ui.label(RichText::new(err).color(theme::DANGER));
            } else if let Err(hint) = &valid {
                ui.label(RichText::new(hint).color(theme::TEXT_MUTED));
            }
            ui.separator();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if self.editing { "Save" } else { "Create" };
                if ui
                    .add_enabled_ui(valid.is_ok(), |ui| accent_button(ui, label))
                    .inner
                    .clicked()
                {
                    outcome = Some(self.submit());
                }
                if ui.button("Cancel").clicked() {
                    outcome = Some(ProjectOutcome::Cancel);
                }
            });
        });
        if modal.should_close() && outcome.is_none() {
            outcome = Some(ProjectOutcome::Cancel);
        }
        outcome
    }

    fn location_ui(&mut self, ui: &mut Ui, outcome: &mut Option<ProjectOutcome>) {
        ui.label("Location");
        ui.horizontal(|ui| {
            let mut shown = self
                .location
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            ui.add(
                TextEdit::singleline(&mut shown)
                    .margin(super::INPUT_MARGIN)
                    .hint_text("Folder for the .ghost file and tickets")
                    .interactive(false)
                    .desired_width(ui.available_width() - 90.0),
            );
            if ui.button(format!("{}  Browse", icon::FOLDER_OPEN)).clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                self.location = Some(dir);
                self.error = None;
            }
        });
        if let Some(existing) = self.location.as_deref().and_then(find_ghost_file) {
            ui.horizontal(|ui| {
                ui.label(RichText::new("This folder already has a project.").color(theme::DANGER));
                if ui.link("Open it instead").clicked() {
                    *outcome = Some(ProjectOutcome::OpenExisting(existing));
                }
            });
        }
    }

    fn columns_ui(&mut self, ui: &mut Ui) {
        let only_one = self.rows.len() == 1;
        let mut remove = None;
        dnd(ui, "project_columns").show_vec(&mut self.rows, |ui, row, handle, _state| {
            ui.horizontal(|ui| {
                handle.ui(ui, |ui| {
                    ui.label(
                        RichText::new(icon::DOTS_SIX_VERTICAL)
                            .size(18.0)
                            .color(theme::TEXT_MUTED),
                    );
                });
                ui.add(
                    TextEdit::singleline(&mut row.name)
                        .margin(super::INPUT_MARGIN)
                        .desired_width(220.0),
                );
                let folder = match folder_name(&row.name) {
                    Some(f) => RichText::new(format!("tickets/{f}"))
                        .color(theme::TEXT_MUTED)
                        .monospace()
                        .small(),
                    None => RichText::new("invalid name").color(theme::DANGER),
                };
                ui.label(folder);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let tip = if only_one {
                        "A project needs at least one column"
                    } else {
                        "Remove column"
                    };
                    if ui
                        .add_enabled_ui(!only_one, |ui| icon_button(ui, icon::TRASH, tip))
                        .inner
                        .clicked()
                    {
                        remove = Some(row.key);
                    }
                    if row.tickets > 0 {
                        ui.label(
                            RichText::new(plural(row.tickets, "ticket"))
                                .color(theme::TEXT_MUTED)
                                .small(),
                        );
                    }
                });
            });
        });
        if let Some(key) = remove {
            let i = self.rows.iter().position(|r| r.key == key).expect("removed row exists");
            let row = self.rows.remove(i);
            if row.orig_folder.is_some() && row.tickets > 0 {
                self.removed.push(Removed { row, target: None });
            }
        }
    }

    fn removed_ui(&mut self, ui: &mut Ui) {
        if self.removed.is_empty() {
            return;
        }
        ui.add_space(8.0);
        let mut restore = None;
        for (i, r) in self.removed.iter_mut().enumerate() {
            if r.target.is_some_and(|k| !self.rows.iter().any(|row| row.key == k)) {
                r.target = None;
            }
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Move {} from \"{}\" to",
                    plural(r.row.tickets, "ticket"),
                    r.row.name
                ));
                let selected = r
                    .target
                    .and_then(|k| self.rows.iter().find(|row| row.key == k))
                    .map_or("Choose column…".to_string(), |row| row.name.clone());
                let combo = ComboBox::from_id_salt(("removed_target", r.row.key))
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for row in &self.rows {
                            ui.selectable_value(&mut r.target, Some(row.key), &row.name);
                        }
                    });
                super::pointer(combo.response);
                if ui.button("Restore").clicked() {
                    restore = Some(i);
                }
            });
        }
        if let Some(i) = restore {
            let r = self.removed.remove(i);
            self.rows.push(r.row);
        }
    }

    fn submit(&self) -> ProjectOutcome {
        let name = self.name.trim().to_string();
        if !self.editing {
            return ProjectOutcome::Create {
                dir: self.location.clone().expect("validated"),
                name,
                columns: self.rows.iter().map(|r| r.name.trim().to_string()).collect(),
            };
        }
        let edits = self
            .rows
            .iter()
            .map(|r| ColumnEdit {
                name: r.name.trim().to_string(),
                orig_folder: r.orig_folder.clone(),
            })
            .collect();
        let targets = self
            .removed
            .iter()
            .filter_map(|r| {
                let idx = self.rows.iter().position(|row| Some(row.key) == r.target)?;
                Some((r.row.orig_folder.clone()?, idx))
            })
            .collect();
        ProjectOutcome::Save { name, edits, targets }
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}
