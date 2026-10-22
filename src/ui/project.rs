use std::collections::HashMap;
use std::path::PathBuf;

use egui::{Align, ComboBox, Id, Layout, RichText, TextEdit, Ui};
use egui_dnd::{DragDropItem, dnd};
use egui_phosphor::regular as icon;

use super::{accent_button, icon_button};
use crate::agents::{AgentEdits, AgentInfo, NewAgent, Role, has_agents, valid_model};
use crate::app::ExportFormat;
use crate::config::ProjectSettings;
use crate::project::{
    AutoMove, ColumnEdit, DEFAULT_AGENTS, DEFAULT_AUTO_MOVES, DEFAULT_COLUMNS, DEFAULT_PROMPTS, Project,
    find_ghost_file, folder_name, validate_columns,
};
use crate::theme;

pub struct ColumnRow {
    key: u64,
    name: String,
    orig_folder: Option<String>,
    tickets: usize,
    /// How tickets move in from the row above.
    auto_move: AutoMove,
    /// Play button prompt; starts as the one in Settings.
    ai_prompt: String,
    show_prompt: bool,
    /// 0 = no limit.
    wip_limit: u32,
    /// Sub agent for tickets moved in on AI Working; `None` = the session itself.
    agent: Option<String>,
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
    /// AI CLI id for the play button, chosen from `cli_options`.
    ai_cli: String,
    cli_options: Vec<String>,
    /// Play button prompt from Settings; columns that keep it follow later changes there.
    default_prompt: String,
    /// Commit, push and test command; kept on this computer, not in the .ghost file.
    settings: ProjectSettings,
    /// The project's .ghost file, when editing.
    ghost: Option<PathBuf>,
    /// The .ghost file and tickets folder are in the project's .gitignore.
    git_excluded: bool,
    /// Asking how to delete the project.
    confirm_delete: bool,
    /// Sub agents of `ai_cli` already in the project folder, and the CLI and folder they were read for.
    found: Vec<AgentInfo>,
    found_for: Option<(String, PathBuf)>,
    /// Models typed into the sub agent list, by name; empty = the session's model.
    models: HashMap<String, String>,
    /// Sub agents to write when the dialog is saved.
    new_agents: Vec<NewAgent>,
    /// The sub agent being written, while its form is open.
    draft: Option<NewAgent>,
    /// The agent the open form edits, by name; `None` = it adds one.
    editing_agent: Option<String>,
    /// Sub agents edited, by name, written when the dialog is saved. GhostLane's own only take instructions.
    edited: HashMap<String, NewAgent>,
    pub error: Option<String>,
}

/// A line of the sub agent list: name, model (empty = the session's), and where it comes from (empty = not
/// written yet).
struct AgentRow {
    name: String,
    model: String,
    note: &'static str,
    /// What an empty model means: GhostLane's own agents go back to their default.
    fallback: Option<&'static str>,
    /// GhostLane writes it from the columns.
    managed: bool,
    /// Its file, if any, can be edited here.
    editable: bool,
}

pub enum ProjectOutcome {
    Cancel,
    Create {
        dir: PathBuf,
        name: String,
        columns: Vec<ColumnEdit>,
        ai_cli: String,
        settings: ProjectSettings,
        /// Put the .ghost file and tickets folder in .gitignore.
        git_excluded: bool,
        agents: AgentEdits,
    },
    Save {
        name: String,
        edits: Vec<ColumnEdit>,
        targets: HashMap<String, usize>,
        ai_cli: String,
        settings: ProjectSettings,
        git_excluded: bool,
        agents: AgentEdits,
    },
    OpenExisting(PathBuf),
    /// Save the board to a file; the dialog stays open.
    Export(ExportFormat),
    /// Open this folder in the system's file manager; the dialog stays open.
    ShowFolder(PathBuf),
    /// Close the project and forget it. `files`: also send its .ghost file and tickets folder to the trash.
    Delete {
        files: bool,
    },
}

impl ProjectForm {
    /// `clis`: AI CLI ids ticked in Settings (never empty). `prompt`: the play button prompt in Settings.
    pub fn new_project(clis: Vec<String>, prompt: &str) -> Self {
        let rows = DEFAULT_COLUMNS
            .iter()
            .zip(DEFAULT_AUTO_MOVES)
            .zip(DEFAULT_PROMPTS)
            .enumerate()
            .map(|(i, ((n, auto_move), column_prompt))| ColumnRow {
                key: i as u64,
                name: n.to_string(),
                orig_folder: None,
                tickets: 0,
                auto_move,
                ai_prompt: column_prompt.unwrap_or(prompt).to_string(),
                show_prompt: false,
                wip_limit: 0,
                agent: DEFAULT_AGENTS[i].map(str::to_string),
            })
            .collect();
        ProjectForm {
            editing: false,
            name: String::new(),
            location: None,
            rows,
            removed: Vec::new(),
            next_key: DEFAULT_COLUMNS.len() as u64,
            ai_cli: clis[0].clone(),
            cli_options: clis,
            default_prompt: prompt.to_string(),
            settings: ProjectSettings::default(),
            ghost: None,
            git_excluded: false,
            confirm_delete: false,
            found: Vec::new(),
            found_for: None,
            models: HashMap::new(),
            new_agents: Vec::new(),
            draft: None,
            editing_agent: None,
            edited: HashMap::new(),
            error: None,
        }
    }

    pub fn edit(p: &Project, mut clis: Vec<String>, prompt: &str, settings: ProjectSettings) -> Self {
        let ai_cli = p.meta.ai_cli.clone().unwrap_or_else(|| clis[0].clone());
        // Keep the project's current choice selectable even if it was unticked in Settings.
        if !clis.contains(&ai_cli) {
            clis.push(ai_cli.clone());
        }
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
                auto_move: c.auto_move,
                ai_prompt: c.ai_prompt.clone().unwrap_or_else(|| prompt.to_string()),
                show_prompt: false,
                wip_limit: c.wip_limit.unwrap_or(0),
                agent: c.agent.clone(),
            })
            .collect();
        ProjectForm {
            editing: true,
            name: p.meta.name.clone(),
            location: Some(p.root.clone()),
            next_key: rows.len() as u64,
            rows,
            removed: Vec::new(),
            ai_cli,
            cli_options: clis,
            default_prompt: prompt.to_string(),
            settings,
            ghost: Some(p.ghost_path.clone()),
            git_excluded: p.git_excluded(),
            confirm_delete: false,
            found: Vec::new(),
            found_for: None,
            models: HashMap::new(),
            new_agents: Vec::new(),
            draft: None,
            editing_agent: None,
            edited: HashMap::new(),
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
        if self
            .models
            .values()
            .any(|m| !m.trim().is_empty() && !valid_model(m.trim()))
        {
            return Err("Models take letters, digits, spaces and . - _ / :".into());
        }
        Ok(())
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<ProjectOutcome> {
        self.find_agents();
        let mut outcome = None;
        let modal = super::show_modal(ctx, "project_modal", |ui| {
            // Room for each column row's name, folder, ticket count, WIP limit and buttons side by side.
            ui.set_width(640.0);
            let title = if self.editing { "Edit Project" } else { "New Project" };
            if super::modal_close(ui, "Close") {
                outcome = Some(ProjectOutcome::Cancel);
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(20.0).strong());
                if self.editing
                    && let Some(dir) = &self.location
                    && icon_button(ui, icon::FOLDER_OPEN, "Open the project folder").clicked()
                {
                    outcome = Some(ProjectOutcome::ShowFolder(dir.clone()));
                }
            });
            ui.add_space(6.0);

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
                ui.label("AI CLI");
                let name = |id: &str| crate::terminal::ai_cli(id).map_or(id.to_string(), |c| c.name.to_string());
                let combo = ComboBox::from_id_salt("project_ai_cli")
                    .selected_text(name(&self.ai_cli))
                    .show_ui(ui, |ui| {
                        for id in &self.cli_options {
                            ui.selectable_value(&mut self.ai_cli, id.clone(), name(id));
                        }
                    });
                super::pointer(combo.response).on_hover_text("Used by the ▶ button. Turn on more tools in Settings.");
            });
            ui.add_space(8.0);
            self.agents_ui(ui);

            let add = ui
                .horizontal(|ui| {
                    ui.label("Columns");
                    ui.label(RichText::new("drag to reorder").color(theme::TEXT_MUTED).small());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.button(format!("{}  Add column", icon::PLUS)).clicked()
                    })
                    .inner
                })
                .inner;
            if add {
                self.rows.push(ColumnRow {
                    key: self.next_key,
                    name: "New Column".into(),
                    orig_folder: None,
                    tickets: 0,
                    auto_move: AutoMove::Manual,
                    ai_prompt: self.default_prompt.clone(),
                    show_prompt: false,
                    wip_limit: 0,
                    agent: None,
                });
                self.next_key += 1;
            }
            self.columns_ui(ui);
            self.removed_ui(ui);
            ui.add_space(8.0);
            settings_ui(ui, &mut self.settings, &mut self.git_excluded);
            if self.editing {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label("Export");
                    if ui.button(format!("{}  Tickets as CSV…", icon::TABLE)).clicked() {
                        outcome = Some(ProjectOutcome::Export(ExportFormat::Csv));
                    }
                    if ui
                        .button(format!("{}  Board as Markdown report…", icon::FILE_TEXT))
                        .clicked()
                    {
                        outcome = Some(ProjectOutcome::Export(ExportFormat::Markdown));
                    }
                });
            }

            ui.add_space(12.0);
            let valid = self.validation();
            if let Some(err) = &self.error {
                ui.label(RichText::new(err).color(theme::DANGER));
            } else if let Err(hint) = &valid {
                ui.label(RichText::new(hint).color(theme::TEXT_MUTED));
            }
            ui.separator();
            ui.horizontal(|ui| {
                if self.editing && super::danger_button(ui, "Delete").clicked() {
                    self.confirm_delete = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let label = if self.editing { "Save" } else { "Create" };
                    if ui
                        .add_enabled_ui(valid.is_ok(), |ui| accent_button(ui, label))
                        .inner
                        .clicked()
                    {
                        outcome = Some(self.submit());
                    }
                });
            });
        });
        if self.confirm_delete {
            match delete_choice(ctx, &self.name, self.ghost.as_deref()) {
                Some(Some(files)) => outcome = Some(ProjectOutcome::Delete { files }),
                Some(None) => self.confirm_delete = false,
                None => {}
            }
        } else if modal.should_close() && outcome.is_none() {
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
        let agents: Vec<String> = self.agent_rows().into_iter().map(|a| a.name).collect();
        let default_prompt = &self.default_prompt;
        dnd(ui, "project_columns").show_vec(&mut self.rows, |ui, row, handle, state| {
            if state.index > 0 {
                auto_move_ui(ui, row, &agents);
            }
            // Same rule as the board: tickets that arrive on "AI Working" get no play button.
            let has_play = state.index == 0 || row.auto_move != AutoMove::AiWorking;
            ui.horizontal(|ui| {
                handle.ui(ui, super::drag_handle);
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
                        .add_enabled_ui(!only_one, |ui| super::danger_icon_button(ui, icon::TRASH, tip))
                        .inner
                        .clicked()
                    {
                        remove = Some(row.key);
                    }
                    let custom = row.ai_prompt.trim() != default_prompt.trim();
                    let idle = if custom { theme::ACCENT } else { theme::TEXT };
                    let tip = if custom {
                        "Play button command (customised)"
                    } else {
                        "Play button command"
                    };
                    if has_play
                        && super::glyph_button(ui, icon::TERMINAL_WINDOW, idle, theme::ACCENT, false)
                            .on_hover_text(tip)
                            .clicked()
                    {
                        row.show_prompt = !row.show_prompt;
                    }
                    let wip = egui::DragValue::new(&mut row.wip_limit)
                        .range(0..=99)
                        .speed(0.1)
                        .custom_formatter(|n, _| if n == 0.0 { "No limit".into() } else { format!("Max {n}") })
                        .custom_parser(|s| s.trim().trim_start_matches("Max").trim().parse().ok().or(Some(0.0)));
                    ui.add(wip).on_hover_text(
                        "Work in progress limit: the column's count turns red past it, and Auto won't move more                          tickets into it.",
                    );
                    if row.tickets > 0 {
                        ui.label(
                            RichText::new(plural(row.tickets, "ticket"))
                                .color(theme::TEXT_MUTED)
                                .small(),
                        );
                    }
                });
            });
            if row.show_prompt && has_play {
                prompt_ui(ui, row, default_prompt);
            }
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
        let edits = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let prompt = r.ai_prompt.trim();
                ColumnEdit {
                    name: r.name.trim().to_string(),
                    orig_folder: r.orig_folder.clone(),
                    // Nothing comes before the first column; it has no dropdown.
                    auto_move: if i == 0 { AutoMove::Manual } else { r.auto_move },
                    // Unchanged or cleared: keep following Settings.
                    ai_prompt: (!prompt.is_empty() && prompt != self.default_prompt.trim()).then(|| prompt.to_string()),
                    wip_limit: (r.wip_limit > 0).then_some(r.wip_limit),
                    agent: r.agent.clone().filter(|_| i > 0 && r.auto_move == AutoMove::AiWorking),
                }
            })
            .collect();
        if !self.editing {
            return ProjectOutcome::Create {
                dir: self.location.clone().expect("validated"),
                name,
                columns: edits,
                ai_cli: self.ai_cli.clone(),
                settings: self.submitted_settings(),
                git_excluded: self.git_excluded,
                agents: self.agent_edits(),
            };
        }
        let targets = self
            .removed
            .iter()
            .filter_map(|r| {
                let idx = self.rows.iter().position(|row| Some(row.key) == r.target)?;
                Some((r.row.orig_folder.clone()?, idx))
            })
            .collect();
        ProjectOutcome::Save {
            name,
            edits,
            targets,
            ai_cli: self.ai_cli.clone(),
            settings: self.submitted_settings(),
            git_excluded: self.git_excluded,
            agents: self.agent_edits(),
        }
    }

    /// Re-reads the sub agents in the project folder when the AI CLI or folder changed; models chosen and edits
    /// made for the old ones go.
    fn find_agents(&mut self) {
        let key = self.location.clone().map(|l| (self.ai_cli.clone(), l));
        if key != self.found_for {
            self.found = key
                .as_ref()
                .map(|(cli, dir)| crate::agents::list(dir, cli))
                .unwrap_or_default();
            self.models.clear();
            self.edited.clear();
            self.found_for = key;
        }
    }

    /// The sub agents the project will have: those in its folder, the Developer, Tester and (when committing) Git
    /// agents GhostLane writes, and new ones. By name.
    fn agent_rows(&self) -> Vec<AgentRow> {
        if !has_agents(&self.ai_cli) {
            return Vec::new();
        }
        let fallback = |a: &AgentInfo| role_default(&a.name, &self.ai_cli).filter(|_| a.managed);
        let mut rows: Vec<AgentRow> = self
            .found
            .iter()
            .map(|a| AgentRow {
                name: a.name.clone(),
                // GhostLane's own agents without a model get their default when saved.
                model: a
                    .model
                    .clone()
                    .or_else(|| fallback(a).map(str::to_string))
                    .unwrap_or_default(),
                note: if a.managed { "GhostLane" } else { "Yours" },
                fallback: fallback(a),
                managed: a.managed,
                editable: a.editable,
            })
            .collect();
        let roles = [Role::Developer, Role::Tester, Role::Git];
        for r in roles.into_iter().filter(|&r| r != Role::Git || self.settings.commit) {
            if !rows.iter().any(|a| a.name == r.id()) {
                rows.push(AgentRow {
                    name: r.id().to_string(),
                    model: r.default_model(&self.ai_cli).unwrap_or_default().to_string(),
                    note: "",
                    fallback: r.default_model(&self.ai_cli),
                    managed: true,
                    editable: true,
                });
            }
        }
        rows.extend(self.new_agents.iter().map(|a| AgentRow {
            name: a.name.clone(),
            model: a.model.clone(),
            note: "",
            fallback: None,
            managed: false,
            editable: true,
        }));
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        rows
    }

    /// New sub agents, and models changed in the list.
    fn agent_edits(&self) -> AgentEdits {
        if !has_agents(&self.ai_cli) {
            return AgentEdits::default();
        }
        let rows = self.agent_rows();
        let mut models: Vec<(String, String)> = self
            .models
            .iter()
            .map(|(name, m)| (name.clone(), m.trim().to_string()))
            .filter(|(name, m)| {
                rows.iter()
                    .any(|r| &r.name == name && &r.model != m && !(m.is_empty() && r.fallback.is_some()))
            })
            .collect();
        models.sort();
        let mut edited: Vec<NewAgent> = self
            .edited
            .values()
            .filter(|a| rows.iter().any(|r| r.name == a.name))
            .cloned()
            .collect();
        edited.sort_by(|a, b| a.name.cmp(&b.name));
        AgentEdits {
            new: self.new_agents.clone(),
            models,
            edited,
        }
    }

    /// The sub agent list with each one's model, and the form for a new one.
    fn agents_ui(&mut self, ui: &mut Ui) {
        if !has_agents(&self.ai_cli) {
            return;
        }
        let rows = self.agent_rows();
        ui.horizontal(|ui| {
            ui.label("Sub agents");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let new = egui::Button::new(format!("{}  New sub agent", icon::PLUS));
                if ui.add_enabled(self.draft.is_none(), new).clicked() {
                    self.draft = Some(NewAgent::default());
                    self.editing_agent = None;
                }
            });
        });
        let mut open = None;
        for row in &rows {
            ui.horizontal(|ui| {
                ui.add_space(26.0);
                ui.label(RichText::new(&row.name).monospace());
                let edit = ui
                    .add_enabled_ui(self.draft.is_none() && row.editable, |ui| {
                        icon_button(ui, icon::PENCIL_SIMPLE, "Edit")
                    })
                    .inner;
                if edit.clicked() {
                    open = Some(row);
                }
                if !row.editable {
                    edit.on_disabled_hover_text("Its file is laid out in a way GhostLane can't edit");
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let tip = match row.note {
                        "GhostLane" => {
                            "Kept up to date by GhostLane from the board's columns when the project is saved"
                        }
                        "Yours" => "Not made by GhostLane, which never rewrites it",
                        _ => "Written when the project is saved",
                    };
                    if !row.note.is_empty() {
                        ui.label(RichText::new(row.note).small().color(theme::TEXT_MUTED))
                            .on_hover_text(tip);
                    }
                    let mut model = self.models.get(&row.name).unwrap_or(&row.model).clone();
                    let id = ("agent_model", &row.name);
                    if model_combo(ui, id, &self.ai_cli, &mut model, row.fallback.is_none()) {
                        self.models.insert(row.name.clone(), model);
                    }
                });
            });
        }
        ui.add_space(8.0);
        if let Some(row) = open {
            self.draft = Some(self.agent_to_edit(row));
            self.editing_agent = Some(row.name.clone());
        }
        let cli = self.ai_cli.clone();
        let Some(d) = &mut self.draft else { return };
        let editing = self.editing_agent.clone();
        let managed = editing.is_some() && rows.iter().any(|r| r.name == d.name && r.managed);
        let mut close = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let field = |ui: &mut Ui, label: &str, text: &mut String, hint: &str| {
                ui.label(label);
                ui.add(
                    TextEdit::singleline(text)
                        .margin(super::INPUT_MARGIN)
                        .hint_text(hint)
                        .desired_width(f32::INFINITY),
                );
            };
            if let Some(name) = &editing {
                ui.label(RichText::new(name).monospace().strong());
            } else {
                field(ui, "Agent name", &mut d.name, "reviewer");
                ui.label("Model");
                model_combo(ui, "new_agent_model", &cli, &mut d.model, true);
            }
            if managed {
                ui.label(
                    RichText::new(
                        "GhostLane writes this agent's steps from the board's columns. What you add here is kept \
                         when it rewrites them.",
                    )
                    .color(theme::TEXT_MUTED),
                );
            } else {
                field(
                    ui,
                    "Description",
                    &mut d.description,
                    "When to use it, e.g. Reviews code changes",
                );
            }
            ui.label(if managed { "Your instructions" } else { "Instructions" });
            ui.add(
                TextEdit::multiline(&mut d.instructions)
                    .margin(super::INPUT_MARGIN)
                    .hint_text("What it does and how")
                    .desired_rows(4)
                    .desired_width(f32::INFINITY),
            );
            let problem = if managed {
                None
            } else if editing.is_none() && rows.iter().any(|r| r.name == d.name) {
                Some("There's already a sub agent with that name")
            } else {
                d.problem()
            };
            ui.horizontal(|ui| {
                if let Some(p) = problem {
                    ui.label(RichText::new(p).color(theme::TEXT_MUTED));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let label = if editing.is_some() { "Save" } else { "Add" };
                    if ui
                        .add_enabled_ui(problem.is_none(), |ui| accent_button(ui, label))
                        .inner
                        .clicked()
                    {
                        close = true;
                        if editing.is_none() {
                            self.new_agents.push(d.clone());
                        } else if let Some(a) = self.new_agents.iter_mut().find(|a| a.name == d.name) {
                            *a = d.clone();
                        } else {
                            self.edited.insert(d.name.clone(), d.clone());
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        });
        if close {
            self.draft = None;
        }
    }

    /// The form's contents for editing sub agent `row`: earlier edits, else what its file says.
    fn agent_to_edit(&self, row: &AgentRow) -> NewAgent {
        let earlier = self.edited.get(&row.name);
        earlier
            .or_else(|| self.new_agents.iter().find(|a| a.name == row.name))
            .cloned()
            .or_else(|| crate::agents::read(self.location.as_deref()?, &self.ai_cli, &row.name))
            .unwrap_or_else(|| NewAgent {
                name: row.name.clone(),
                ..Default::default()
            })
    }

    fn submitted_settings(&self) -> ProjectSettings {
        let s = &self.settings;
        ProjectSettings {
            commit: s.commit,
            // Only offered while committing (and for pull requests, pushing) is on.
            push: s.commit && s.push,
            pull_request: s.commit && s.push && s.pull_request,
            test_command: s.test_command.trim().to_string(),
        }
    }
}

/// Dropdown of AI CLI `cli`'s models for `model`, plus the session's model (empty) when `session`. A model set
/// by hand in the agent file stays listed. True when changed.
fn model_combo(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    cli: &str,
    model: &mut String,
    session: bool,
) -> bool {
    const SESSION: &str = "Session's model";
    let current = model.clone();
    let mut options: Vec<&str> = session.then_some("").into_iter().collect();
    options.extend(crate::agents::models(cli));
    if !options.contains(&current.as_str()) {
        options.push(&current);
    }
    let label = |m: &str| if m.is_empty() { SESSION } else { m }.to_string();
    let mut changed = false;
    let combo = ComboBox::from_id_salt(id)
        .width(220.0)
        .selected_text(label(&current))
        .show_ui(ui, |ui| {
            for m in &options {
                if ui.selectable_label(current == *m, label(m)).clicked() && current != *m {
                    *model = m.to_string();
                    changed = true;
                }
            }
        });
    super::pointer(combo.response).on_hover_text("The model this sub agent runs on");
    changed
}

/// Default model of GhostLane's own agent `name` on AI CLI `cli`.
fn role_default(name: &str, cli: &str) -> Option<&'static str> {
    [Role::Developer, Role::Tester, Role::Git]
        .into_iter()
        .find(|r| r.id() == name)
        .and_then(|r| r.default_model(cli))
}

/// Asks whether deleting the project should leave its files or trash them too. `Some(Some(files))` once chosen,
/// `Some(None)` when cancelled.
fn delete_choice(ctx: &egui::Context, name: &str, ghost: Option<&std::path::Path>) -> Option<Option<bool>> {
    let mut choice = None;
    let file = ghost
        .and_then(|g| g.file_name())
        .map_or("the .ghost file".to_string(), |f| f.to_string_lossy().into_owned());
    let trash = if cfg!(windows) { "Recycle Bin" } else { "Trash" };
    let modal = super::show_modal(ctx, "delete_project", |ui| {
        ui.set_width(460.0);
        ui.label(RichText::new(format!("Delete {}?", name.trim())).size(18.0).strong());
        ui.label("Remove from GhostLane closes the project and leaves all its files where they are.");
        ui.label(format!(
            "Delete Files also moves {file} and the tickets folder to the {trash}. Everything else in the project \
             folder stays."
        ));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                choice = Some(None);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if super::danger_button(ui, "Delete Files").clicked() {
                    choice = Some(Some(true));
                }
                if ui.button("Remove from GhostLane").clicked() {
                    choice = Some(Some(false));
                }
            });
        });
    });
    if modal.should_close() && choice.is_none() {
        choice = Some(None);
    }
    choice
}

/// Git options and the test command for the project.
fn settings_ui(ui: &mut Ui, s: &mut ProjectSettings, git_excluded: &mut bool) {
    ui.label("Git");
    super::pointer(ui.checkbox(git_excluded, "Exclude from Git")).on_hover_text(
        "Adds the .ghost file and the tickets folder to the project's .gitignore, so the board stays out of the \
         repository. Unticking takes them out again. Files git already tracks stay tracked until you remove them \
         with git rm --cached.",
    );
    super::pointer(ui.checkbox(&mut s.commit, "Commit when ticket is done")).on_hover_text(
        "When a ticket reaches the last column, commit everything in the project's git repository with the \
         ticket's number and title as the message.",
    );
    if s.commit {
        ui.horizontal(|ui| {
            ui.add_space(26.0);
            super::pointer(ui.checkbox(&mut s.push, "Push when commit is made"));
        });
    }
    if s.commit && s.push {
        ui.horizontal(|ui| {
            ui.add_space(52.0);
            super::pointer(ui.checkbox(&mut s.pull_request, "Open a pull request instead")).on_hover_text(
                "Push each done ticket to a branch of its own (ghostlane/<number>) and open a pull request for it with \
                 the GitHub CLI (gh), instead of pushing to the current branch. With parallel runs each pull request \
                 holds just its ticket; otherwise it also holds earlier commits that aren't pushed yet.",
            );
        });
    }
    ui.horizontal(|ui| {
        ui.label("Test command");
        ui.add(
            TextEdit::singleline(&mut s.test_command)
                .margin(super::INPUT_MARGIN)
                .hint_text("Optional, e.g. cargo test")
                .desired_width(f32::INFINITY),
        )
        .on_hover_text(
            "Runs in the project folder when the AI finishes a ticket in the last column. The ticket only stays \
             there if the command succeeds; otherwise it goes back to the first column with the output in its notes.",
        );
    });
    ui.label(
        RichText::new("Committing, pushing and the test command stay on this computer.")
            .small()
            .color(theme::TEXT_MUTED),
    );
}

/// Dropdown between two rows: what moves tickets from the row above into `row`, and on AI Working, which of
/// `agents` works them.
fn auto_move_ui(ui: &mut Ui, row: &mut ColumnRow, agents: &[String]) {
    ui.horizontal(|ui| {
        ui.add_space(26.0);
        ui.label(RichText::new(icon::ARROW_DOWN).color(theme::TEXT_MUTED));
        ui.label(RichText::new("Moves in on").color(theme::TEXT_MUTED).small());
        let combo = ComboBox::from_id_salt(("auto_move", row.key))
            .selected_text(row.auto_move.label())
            .show_ui(ui, |ui| {
                for m in AutoMove::ALL {
                    ui.selectable_value(&mut row.auto_move, m, m.label());
                }
            });
        super::pointer(combo.response)
            .on_hover_text("AI Working: when ▶ is pressed above. AI Complete: when the AI finishes.");
        if row.auto_move == AutoMove::AiWorking && !agents.is_empty() {
            ui.label(RichText::new("Sub agent").color(theme::TEXT_MUTED).small());
            const MAIN: &str = "Main session";
            let combo = ComboBox::from_id_salt(("agent", row.key))
                .selected_text(row.agent.as_deref().unwrap_or(MAIN))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut row.agent, None, MAIN);
                    for a in agents {
                        ui.selectable_value(&mut row.agent, Some(a.clone()), a);
                    }
                });
            super::pointer(combo.response).on_hover_text(
                "The sub agent ▶ hands these tickets to. Main session: the AI CLI works on them itself.",
            );
        }
    });
}

/// Play button prompt for one row, with a reset to the one in Settings.
fn prompt_ui(ui: &mut Ui, row: &mut ColumnRow, default_prompt: &str) {
    ui.horizontal(|ui| {
        ui.add_space(26.0);
        ui.add(
            TextEdit::singleline(&mut row.ai_prompt)
                .margin(super::INPUT_MARGIN)
                .hint_text(default_prompt)
                .desired_width(ui.available_width() - 40.0),
        )
        .on_hover_text(
            "Prompt the ▶ button sends for tickets in this column. \
             {ticket_path}, {ticket_id} and {project_dir} are filled in.",
        );
        if icon_button(ui, icon::ARROW_COUNTER_CLOCKWISE, "Reset to the command in Settings").clicked() {
            row.ai_prompt = default_prompt.to_string();
        }
    });
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    fn harness() -> Harness<'static, (ProjectForm, Option<bool>)> {
        let form = ProjectForm::new_project(vec!["claude".into()], "Do {ticket_path}");
        let mut h = Harness::new_ui_state(
            |ui, (form, cancelled): &mut (ProjectForm, Option<bool>)| {
                if let Some(outcome) = form.show(ui.ctx()) {
                    *cancelled = Some(matches!(outcome, ProjectOutcome::Cancel));
                }
            },
            (form, None),
        );
        crate::theme::apply(&h.ctx);
        h.set_size(egui::vec2(900.0, 1400.0));
        h.run_steps(3);
        h
    }

    #[test]
    fn x_closes_and_there_is_no_cancel_button() {
        let mut h = harness();
        assert!(h.query_by_label("Cancel").is_none());
        h.get_by_label(icon::X).click();
        h.run();
        assert_eq!(h.state().1, Some(true));
    }

    #[test]
    fn export_is_offered_when_editing_only() {
        let h = harness();
        assert!(h.query_by_label_contains("Tickets as CSV").is_none());
        let d = tempfile::tempdir().unwrap();
        let p = Project::create(d.path(), "P", &["To Do".into()]).unwrap();
        let form = ProjectForm::edit(&p, vec!["claude".into()], "Do", ProjectSettings::default());
        let mut h = Harness::new_ui_state(
            |ui, (form, out): &mut (ProjectForm, Option<&'static str>)| {
                if let Some(ProjectOutcome::Export(ExportFormat::Markdown)) = form.show(ui.ctx()) {
                    *out = Some("markdown");
                }
            },
            (form, None),
        );
        crate::theme::apply(&h.ctx);
        h.set_size(egui::vec2(900.0, 1400.0));
        h.run_steps(3);
        h.get_by_label_contains("Board as Markdown report").click();
        h.run();
        assert_eq!(h.state().1, Some("markdown"));
    }

    #[test]
    fn delete_offers_keeping_or_trashing_the_files() {
        let h = harness();
        assert!(
            h.query_by_label("Delete").is_none(),
            "nothing to delete for a new project"
        );
        let d = tempfile::tempdir().unwrap();
        let p = Project::create(d.path(), "P", &["To Do".into()]).unwrap();
        let form = ProjectForm::edit(&p, vec!["claude".into()], "Do", ProjectSettings::default());
        let mut h = Harness::new_ui_state(
            |ui, (form, out): &mut (ProjectForm, Vec<String>)| match form.show(ui.ctx()) {
                Some(ProjectOutcome::Delete { files }) => out.push(format!("delete files={files}")),
                Some(ProjectOutcome::Cancel) => out.push("cancel".into()),
                _ => {}
            },
            (form, Vec::new()),
        );
        crate::theme::apply(&h.ctx);
        h.set_size(egui::vec2(900.0, 1400.0));
        h.run_steps(3);
        // Bottom left, on the row with Save.
        let delete = h.get_by_label("Delete").rect();
        let save = h.get_by_label("Save").rect();
        assert!(delete.right() < save.left() && (delete.center().y - save.center().y).abs() < 2.0);
        let name = h.get_by_label("Name").rect();
        assert!((delete.left() - name.left()).abs() < 2.0, "lines up with the left edge");
        // Red outline and text; filled red with dark text on hover.
        let text = |h: &Harness<'static, (ProjectForm, Vec<String>)>| {
            h.output().shapes.iter().find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text() == "Delete" => t.override_text_color,
                _ => None,
            })
        };
        assert_eq!(text(&h), Some(theme::DANGER));
        h.get_by_label("Delete").hover();
        h.run();
        assert_eq!(text(&h), Some(theme::ACCENT_TEXT));

        h.get_by_label("Delete").click();
        h.run();
        assert!(h.query_by_label_contains("p.ghost and the tickets folder").is_some());
        h.get_by_label("Cancel").click();
        h.run();
        assert!(h.state().1.is_empty(), "cancelling keeps the dialog open");
        h.get_by_label("Delete").click();
        h.run();
        h.get_by_label("Remove from GhostLane").click();
        h.run();
        h.get_by_label("Delete Files").click();
        h.run();
        assert_eq!(h.state().1, ["delete files=false", "delete files=true"]);
    }

    #[test]
    fn folder_icon_opens_the_project_folder_when_editing() {
        let h = harness();
        assert!(
            h.query_by_label(icon::FOLDER_OPEN).is_none(),
            "a new project has no folder yet"
        );
        let d = tempfile::tempdir().unwrap();
        let p = Project::create(d.path(), "P", &["To Do".into()]).unwrap();
        let form = ProjectForm::edit(&p, vec!["claude".into()], "Do", ProjectSettings::default());
        let mut h = Harness::new_ui_state(
            |ui, (form, out): &mut (ProjectForm, Option<PathBuf>)| {
                if let Some(ProjectOutcome::ShowFolder(dir)) = form.show(ui.ctx()) {
                    *out = Some(dir);
                }
            },
            (form, None),
        );
        crate::theme::apply(&h.ctx);
        h.set_size(egui::vec2(900.0, 1400.0));
        h.run_steps(3);
        let title = h.get_by_label("Edit Project").rect();
        let folder = h.get_by_label(icon::FOLDER_OPEN).rect();
        assert!(folder.left() > title.right() && (folder.center().y - title.center().y).abs() < 4.0);
        h.get_by_label(icon::FOLDER_OPEN).click();
        h.run();
        assert_eq!(h.state().1.as_deref(), Some(d.path()));
    }

    #[test]
    fn add_column_appends_a_row() {
        let mut h = harness();
        h.get_by_label_contains("Add column").click();
        h.run();
        assert_eq!(h.state().0.rows.len(), DEFAULT_COLUMNS.len() + 1);
    }

    #[test]
    fn default_columns_move_on_ai_and_keep_the_settings_prompt() {
        let mut form = ProjectForm::new_project(vec!["claude".into()], "Do {ticket_path}");
        form.location = Some(PathBuf::from("/p"));
        form.rows[2].ai_prompt = " Test {ticket_path} ".into();
        let ProjectOutcome::Create { columns, .. } = form.submit() else {
            panic!("expected create");
        };
        let moves: Vec<_> = columns.iter().map(|c| c.auto_move).collect();
        assert_eq!(moves, DEFAULT_AUTO_MOVES);
        let prompts: Vec<_> = columns.iter().map(|c| c.ai_prompt.as_deref()).collect();
        assert_eq!(
            prompts,
            [DEFAULT_PROMPTS[0], None, Some("Test {ticket_path}"), None, None]
        );
        assert!(DEFAULT_PROMPTS[0].unwrap().contains("{ticket_path}"));
        assert!(DEFAULT_PROMPTS[2].unwrap().contains("{ticket_path}"));
    }

    #[test]
    fn dropdowns_sit_between_columns() {
        let h = harness();
        assert_eq!(h.get_all_by_label("Moves in on").count(), DEFAULT_COLUMNS.len() - 1);
    }

    #[test]
    fn command_icon_only_on_columns_with_a_play_button() {
        // Defaults: In Dev and In Test take tickets on AI Working, so 3 of 5 columns keep the icon.
        let h = harness();
        assert_eq!(h.get_all_by_label(icon::TERMINAL_WINDOW).count(), 3);
    }

    #[test]
    fn push_only_shows_and_saves_with_commit() {
        let mut h = harness();
        assert!(h.query_by_label("Push when commit is made").is_none());
        h.get_by_label("Commit when ticket is done").click();
        h.run();
        assert!(h.query_by_label("Open a pull request instead").is_none());
        h.get_by_label("Push when commit is made").click();
        h.run();
        h.get_by_label("Open a pull request instead").click();
        h.run();
        let s = &h.state().0.settings;
        assert!(s.commit && s.push && s.pull_request);
        h.get_by_label("Commit when ticket is done").click();
        h.run();
        assert!(h.query_by_label("Push when commit is made").is_none());
        assert!(h.query_by_label("Open a pull request instead").is_none());
        let form = &mut h.state_mut().0;
        form.location = Some(PathBuf::from("/p"));
        form.settings.test_command = "  cargo test ".into();
        let ProjectOutcome::Create { settings, .. } = form.submit() else {
            panic!("expected create");
        };
        assert_eq!(
            settings,
            ProjectSettings {
                commit: false,
                push: false,
                pull_request: false,
                test_command: "cargo test".into()
            }
        );
    }

    #[test]
    fn exclude_from_git_sits_under_the_git_heading_and_reflects_gitignore() {
        let mut h = harness();
        let heading = h.get_by_label("Git").rect();
        let exclude = h.get_by_label("Exclude from Git").rect();
        let commit = h.get_by_label("Commit when ticket is done").rect();
        assert!(heading.bottom() <= exclude.top() && exclude.bottom() <= commit.top());
        h.get_by_label("Exclude from Git").click();
        h.run();
        let form = &mut h.state_mut().0;
        form.location = Some(PathBuf::from("/p"));
        let ProjectOutcome::Create { git_excluded, .. } = form.submit() else {
            panic!("expected create");
        };
        assert!(git_excluded);

        let d = tempfile::tempdir().unwrap();
        let p = Project::create(d.path(), "P", &["To Do".into()]).unwrap();
        p.set_git_excluded(true).unwrap();
        let form = ProjectForm::edit(&p, vec!["claude".into()], "Do", ProjectSettings::default());
        assert!(form.git_excluded, "read from .gitignore");
    }

    #[test]
    fn a_tall_modal_fits_a_small_window_and_scrolls() {
        let mut h = harness();
        h.set_size(egui::vec2(700.0, 420.0));
        h.run_steps(3);
        let modal = h.ctx.memory(|m| m.area_rect(egui::Id::new("project_modal"))).unwrap();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(700.0, 420.0));
        assert!(screen.contains_rect(modal), "{modal:?} within {screen:?}");
        // The X stays in the corner and the button at the bottom is reachable by scrolling.
        let x = h.get_by_label(icon::X).rect();
        assert!(
            modal.contains_rect(x) && x.top() - modal.top() < 20.0,
            "{x:?} in {modal:?}"
        );
        assert!(
            h.get_by_label("Create").rect().top() > modal.bottom(),
            "below the fold until scrolled"
        );
        h.hover_at(modal.center());
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -2000.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        });
        // Scrolling animates, so it keeps repainting for a while.
        h.run_steps(60);
        let create = h.get_by_label("Create").rect();
        assert!(modal.contains_rect(create), "{create:?} scrolled into {modal:?}");
        assert!(
            (h.get_by_label(icon::X).rect().top() - x.top()).abs() < 0.5,
            "the X doesn't scroll away"
        );
        // A real click on it lands, scroll bar or not.
        let pos = x.center();
        h.hover_at(pos);
        h.run();
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
            h.step();
        }
        assert_eq!(h.state().1, Some(true));
    }

    #[test]
    fn ai_working_columns_pick_a_sub_agent() {
        let mut h = harness();
        assert_eq!(h.get_all_by_label("Sub agent").count(), 2, "In Dev and In Test");
        let form = &mut h.state_mut().0;
        form.location = Some(PathBuf::from("/p"));
        // A column that stops moving on AI Working drops its agent.
        form.rows[2].agent = Some("developer".into());
        let ProjectOutcome::Create { columns, .. } = form.submit() else {
            panic!("expected create");
        };
        let agents: Vec<_> = columns.iter().map(|c| c.agent.as_deref()).collect();
        assert_eq!(agents, DEFAULT_AGENTS);
    }

    #[test]
    fn sub_agents_list_their_models_and_git_comes_with_commit() {
        let mut h = harness();
        // Room for the new sub agent form without scrolling.
        h.set_size(egui::vec2(900.0, 2000.0));
        h.run_steps(3);
        let names = |h: &Harness<'static, (ProjectForm, Option<bool>)>| {
            h.state()
                .0
                .agent_rows()
                .into_iter()
                .map(|a| (a.name, a.model))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&h),
            [
                ("developer".into(), "claude-opus-5-5".into()),
                ("tester".into(), "claude-sonnet-5-5".into())
            ]
        );
        assert!(h.query_by_label("New").is_none(), "agents not written yet carry no tag");
        // Above the columns, each model a dropdown of the CLI's models.
        assert!(h.get_by_label("Sub agents").rect().bottom() < h.get_by_label("Columns").rect().top());
        h.get_by_value("claude-sonnet-5-5").click();
        h.run();
        h.get_by_label("claude-haiku-5-5").click();
        h.run();
        assert_eq!(
            h.state().0.models.get("tester").map(String::as_str),
            Some("claude-haiku-5-5")
        );
        h.get_by_value("claude-opus-5-5").click();
        h.run();
        assert!(h.query_by_label("claude-fable-5-1").is_some());
        assert!(
            h.query_by_label("Session's model").is_none(),
            "GhostLane's agents fall back to their default"
        );
        h.get_by_label("claude-opus-5-5").click();
        h.run();
        h.get_by_label("Commit when ticket is done").click();
        h.run();
        assert_eq!(names(&h)[0], ("developer".into(), "claude-opus-5-5".into()));
        assert_eq!(names(&h)[1], ("git".into(), "claude-sonnet-5-5".into()));
        assert!(h.query_by_label("git").is_some());

        h.get_by_label_contains("New sub agent").click();
        h.run();
        let form = &mut h.state_mut().0;
        *form.draft.as_mut().unwrap() = NewAgent {
            name: "tester".into(),
            description: "Reviews".into(),
            instructions: "Review it.".into(),
            ..Default::default()
        };
        h.run();
        assert!(h.query_by_label("There's already a sub agent with that name").is_some());
        h.state_mut().0.draft.as_mut().unwrap().name = "reviewer".into();
        h.run();
        h.get_by_label("Add").click();
        h.run();
        let form = &mut h.state_mut().0;
        assert!(form.draft.is_none());
        form.location = Some(PathBuf::from("/p"));
        form.models.insert("tester".into(), " claude-haiku-5-5 ".into());
        form.models.insert("developer".into(), "claude-opus-5-5".into());
        let ProjectOutcome::Create { agents, .. } = form.submit() else {
            panic!("expected create");
        };
        assert_eq!(agents.new[0].name, "reviewer");
        assert_eq!(
            agents.models,
            [("tester".into(), "claude-haiku-5-5".into())],
            "only changed ones"
        );
        form.models.insert("tester".into(), "bad: model".into());
        assert!(form.validation().is_err());
    }

    #[test]
    fn each_sub_agent_has_an_edit_button() {
        let mut h = harness();
        h.set_size(egui::vec2(900.0, 2000.0));
        h.run_steps(3);
        let edit = |h: &mut Harness<'static, (ProjectForm, Option<bool>)>, i: usize| {
            h.get_all_by_label(icon::PENCIL_SIMPLE).nth(i).unwrap().click();
            h.run();
        };
        assert_eq!(h.get_all_by_label(icon::PENCIL_SIMPLE).count(), 2);
        // GhostLane's tester only takes added instructions.
        edit(&mut h, 1);
        assert!(h.query_by_label("Your instructions").is_some() && h.query_by_label("Description").is_none());
        h.state_mut().0.draft.as_mut().unwrap().instructions = "Run the slow tests too.".into();
        h.run();
        h.get_by_label("Save").click();
        h.run();
        assert!(h.state().0.draft.is_none());

        h.state_mut().0.new_agents.push(NewAgent {
            name: "reviewer".into(),
            description: "Reviews".into(),
            instructions: "Review it.".into(),
            ..Default::default()
        });
        h.run();
        edit(&mut h, 1);
        assert!(h.query_by_label("Description").is_some() && h.query_by_label("Agent name").is_none());
        h.state_mut().0.draft.as_mut().unwrap().description = "Reviews code".into();
        h.run();
        h.get_by_label("Save").click();
        h.run();
        edit(&mut h, 2);
        assert_eq!(
            h.state().0.draft.as_ref().unwrap().instructions,
            "Run the slow tests too.",
            "earlier edits"
        );
        h.get_by_label("Cancel").click();
        h.run();

        let form = &mut h.state_mut().0;
        assert_eq!(form.new_agents[0].description, "Reviews code");
        form.location = Some(PathBuf::from("/p"));
        let ProjectOutcome::Create { agents, .. } = form.submit() else {
            panic!("expected create");
        };
        let edited: Vec<_> = agents
            .edited
            .iter()
            .map(|a| (a.name.as_str(), a.instructions.as_str()))
            .collect();
        assert_eq!(edited, [("tester", "Run the slow tests too.")]);
    }

    #[test]
    fn first_column_never_auto_moves() {
        let mut form = ProjectForm::new_project(vec!["claude".into()], "Do");
        form.location = Some(PathBuf::from("/p"));
        form.rows[0].auto_move = AutoMove::AiWorking;
        form.rows[1].wip_limit = 2;
        let ProjectOutcome::Create { columns, .. } = form.submit() else {
            panic!("expected create");
        };
        assert_eq!(columns[0].auto_move, AutoMove::Manual);
        assert_eq!((columns[0].wip_limit, columns[1].wip_limit), (None, Some(2)));
    }
}
