use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{Align, Align2, Area, CornerRadius, Frame, Id, Layout, Margin, Order, Popup, RichText, Ui, vec2};
use egui_commonmark::CommonMarkCache;
use egui_phosphor::regular as icon;
use notify_debouncer_full::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::config::AppConfig;
use crate::project::{Project, Sort};
use crate::terminal::{self, Terminals};
use crate::theme;
use crate::ticket::Ticket;
use crate::ui::project::{ProjectForm, ProjectOutcome};
use crate::ui::settings::{SettingsForm, SettingsOutcome};
use crate::ui::ticket::{self as ticket_ui, FormMode, FormOutcome, TicketForm, ViewOutcome};
use crate::ui::{self, accent_button, icon_button};

pub struct OpenProject {
    pub path: PathBuf,
    pub project: Result<Project, String>,
    /// Column folder -> search text. Present = search box open.
    pub search: HashMap<String, String>,
}

impl OpenProject {
    fn load(path: PathBuf) -> Self {
        let project = Project::load(&path).map_err(|e| e.to_string());
        OpenProject {
            path,
            project,
            search: HashMap::new(),
        }
    }

    pub fn name(&self) -> String {
        match &self.project {
            Ok(p) => p.meta.name.clone(),
            Err(_) => self.path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        }
    }
}

/// Things the UI asks for; applied after drawing so widgets never hold borrows across mutations.
pub enum Action {
    SwitchProject(usize),
    OpenProjectDialog,
    NewProject,
    EditProject,
    OpenSettings,
    MoveTicket { id: u32, to_col: usize },
    SetSort { col: usize, sort: Sort },
    ViewTicket(u32),
    EditTicket(u32),
    NewTicket(usize),
    DuplicateTicket(u32),
    AskDelete(u32),
    RunAi(u32),
}

pub enum Modal {
    Project(ProjectForm),
    ViewTicket(u32),
    EditTicket(Box<TicketForm>),
    ConfirmDelete(u32),
    Settings(SettingsForm),
}

type Watcher = Debouncer<RecommendedWatcher, RecommendedCache>;

pub struct App {
    pub config: AppConfig,
    pub projects: Vec<OpenProject>,
    pub active: Option<usize>,
    pub modal: Option<Modal>,
    md_cache: CommonMarkCache,
    terminals: Terminals,
    terminal_focused: bool,
    toasts: Vec<(String, Instant)>,
    watcher: Option<Watcher>,
    watch_tx: Sender<DebounceEventResult>,
    watch_rx: Receiver<DebounceEventResult>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = &cc.egui_ctx;
        egui_extras::install_image_loaders(ctx);
        theme::apply(ctx);

        let mut config = AppConfig::load();
        if config.settings.ai_clis.is_none() {
            // First launch: offer the AI CLIs installed on this machine.
            config.settings.ai_clis = Some(terminal::detect_ai_clis());
            let _ = config.save();
        }
        let projects: Vec<OpenProject> = config.open_projects.iter().cloned().map(OpenProject::load).collect();
        let active = config
            .active_project
            .as_ref()
            .and_then(|a| projects.iter().position(|p| &p.path == a))
            .filter(|&i| projects[i].project.is_ok())
            .or_else(|| projects.iter().position(|p| p.project.is_ok()));

        let (watch_tx, watch_rx) = mpsc::channel();
        let mut app = App {
            config,
            projects,
            active,
            modal: None,
            md_cache: CommonMarkCache::default(),
            terminals: Terminals::new(),
            terminal_focused: false,
            toasts: Vec::new(),
            watcher: None,
            watch_tx,
            watch_rx,
        };
        for p in &app.projects {
            if let Err(e) = &p.project {
                app.toasts
                    .push((format!("Couldn't load {}: {e}", p.path.display()), Instant::now()));
            }
        }
        app.watch_active(ctx);
        app
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        eprintln!("{msg}");
        self.toasts.push((msg, Instant::now()));
    }

    pub fn active_project(&self) -> Option<&Project> {
        self.projects.get(self.active?)?.project.as_ref().ok()
    }

    pub fn active_project_mut(&mut self) -> Option<&mut Project> {
        self.projects.get_mut(self.active?)?.project.as_mut().ok()
    }

    /// (column, ticket) of a ticket in the active project.
    pub fn find_ticket(&self, id: u32) -> Option<(usize, &Ticket)> {
        let p = self.active_project()?;
        p.tickets
            .iter()
            .enumerate()
            .find_map(|(c, ts)| ts.iter().find(|t| t.id == id).map(|t| (c, t)))
    }

    fn save_config(&mut self) {
        self.config.open_projects = self.projects.iter().map(|p| p.path.clone()).collect();
        self.config.active_project = self.active.map(|i| self.projects[i].path.clone());
        if let Err(e) = self.config.save() {
            self.toast(format!("Couldn't save settings: {e}"));
        }
    }

    fn watch_active(&mut self, ctx: &egui::Context) {
        self.watcher = None;
        let Some(p) = self.active_project() else { return };
        let (tickets, root) = (p.tickets_dir(), p.root.clone());
        let tx = self.watch_tx.clone();
        let ctx = ctx.clone();
        let handler = move |res: DebounceEventResult| {
            let _ = tx.send(res);
            ctx.request_repaint();
        };
        let result = new_debouncer(Duration::from_millis(200), None, handler).and_then(|mut w| {
            w.watch(&tickets, RecursiveMode::Recursive)?;
            w.watch(&root, RecursiveMode::NonRecursive)?;
            Ok(w)
        });
        match result {
            Ok(w) => self.watcher = Some(w),
            Err(e) => self.toast(format!("File watching unavailable: {e}")),
        }
    }

    fn poll_watcher(&mut self) {
        let mut changed = false;
        while let Ok(res) = self.watch_rx.try_recv() {
            let relevant = |p: &Path| {
                let s = p.to_string_lossy();
                !s.ends_with(".tmp") && !s.contains(".ghostlane_tmp")
            };
            if let Ok(events) = res {
                changed |= events.iter().any(|e| e.paths.iter().any(|p| relevant(p)));
            }
        }
        if changed {
            self.reload_active();
        }
    }

    /// Re-reads the active project from disk; drops it if its .ghost file is gone.
    pub fn reload_active(&mut self) {
        let Some(i) = self.active else { return };
        let path = self.projects[i].path.clone();
        if !path.is_file() {
            self.toast(format!("{} was removed; closing project", path.display()));
            self.projects.remove(i);
            self.active = self.projects.iter().position(|p| p.project.is_ok());
            self.watcher = None;
            self.save_config();
            return;
        }
        match Project::load(&path) {
            Ok(p) => self.projects[i].project = Ok(p),
            Err(e) => self.toast(format!("Couldn't reload project: {e}")),
        }
    }

    fn switch_to(&mut self, i: usize, ctx: &egui::Context) {
        self.active = Some(i);
        self.reload_active();
        self.save_config();
        self.watch_active(ctx);
    }

    pub fn add_project(&mut self, project: Project, ctx: &egui::Context) {
        let path = project.ghost_path.clone();
        let i = match self.projects.iter().position(|p| p.path == path) {
            Some(i) => i,
            None => {
                self.projects.push(OpenProject {
                    path,
                    project: Err(String::new()),
                    search: HashMap::new(),
                });
                self.projects.len() - 1
            }
        };
        self.projects[i].project = Ok(project);
        self.switch_to(i, ctx);
    }

    fn open_project_dialog(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("GhostLane project", &["ghost"])
            .pick_file()
        else {
            return;
        };
        self.open_path(path, ctx);
    }

    fn open_path(&mut self, path: PathBuf, ctx: &egui::Context) {
        let path = std::fs::canonicalize(&path).map(strip_unc).unwrap_or(path);
        match Project::load(&path) {
            Ok(p) => self.add_project(p, ctx),
            Err(e) => self.toast(format!("Couldn't open {}: {e}", path.display())),
        }
    }

    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::SwitchProject(i) => self.switch_to(i, ctx),
            Action::OpenProjectDialog => self.open_project_dialog(ctx),
            Action::MoveTicket { id, to_col } => {
                let Some((from, t)) = self.find_ticket(id) else { return };
                if from == to_col {
                    return;
                }
                let path = t.path.clone();
                let p = self.active_project_mut().expect("ticket found in active project");
                if let Err(e) = p.move_ticket(&path, to_col) {
                    self.toast(format!("Couldn't move {}: {e}", crate::ticket::format_id(id)));
                }
            }
            Action::SetSort { col, sort } => {
                if let Some(Err(e)) = self.active_project_mut().map(|p| p.set_sort(col, sort)) {
                    self.toast(format!("Couldn't save sort order: {e}"));
                }
            }
            Action::NewProject => {
                let clis = self.config.settings.enabled_clis();
                self.modal = Some(Modal::Project(ProjectForm::new_project(clis)));
            }
            Action::EditProject => {
                let clis = self.config.settings.enabled_clis();
                if let Some(p) = self.active_project() {
                    self.modal = Some(Modal::Project(ProjectForm::edit(p, clis)));
                }
            }
            Action::ViewTicket(id) => self.modal = Some(Modal::ViewTicket(id)),
            Action::EditTicket(id) => {
                if let Some((_, t)) = self.find_ticket(id) {
                    self.modal = Some(Modal::EditTicket(Box::new(TicketForm::edit(t))));
                }
            }
            Action::NewTicket(col) => {
                let Some(p) = self.active_project() else { return };
                let t = Ticket {
                    id: p.meta.next_ticket_id,
                    ..Default::default()
                };
                self.modal = Some(Modal::EditTicket(Box::new(TicketForm::create(col, t))));
            }
            Action::DuplicateTicket(id) => {
                let Some(next) = self.active_project().map(|p| p.meta.next_ticket_id) else {
                    return;
                };
                let Some((col, t)) = self.find_ticket(id) else { return };
                let copy = Ticket {
                    id: next,
                    path: PathBuf::new(),
                    title: format!("Duplicate: {}", t.title),
                    malformed: false,
                    ..t.clone()
                };
                self.modal = Some(Modal::EditTicket(Box::new(TicketForm::create(col, copy))));
            }
            Action::AskDelete(id) => self.modal = Some(Modal::ConfirmDelete(id)),
            Action::RunAi(id) => self.run_ai(id, ctx),
            Action::OpenSettings => self.modal = Some(Modal::Settings(SettingsForm::new(&self.config.settings))),
        }
    }

    /// The AI CLI a project's play button and terminal use.
    fn project_cli(&self, p: &Project) -> Result<&'static terminal::AiCli, String> {
        let id = p
            .meta
            .ai_cli
            .clone()
            .unwrap_or_else(|| self.config.settings.enabled_clis()[0].clone());
        terminal::ai_cli(&id).ok_or_else(|| format!("Unknown AI CLI \"{id}\"; pick one in the project settings"))
    }

    fn run_ai(&mut self, id: u32, ctx: &egui::Context) {
        let Some(p) = self.active_project() else { return };
        let Some((_, t)) = self.find_ticket(id) else { return };
        let (key, root, mut t) = (p.ghost_path.clone(), p.root.clone(), t.clone());
        let cli = match self.project_cli(p) {
            Ok(cli) => cli,
            Err(e) => return self.toast(e),
        };
        let settings = &self.config.settings;
        let prompt = match terminal::fill_prompt(&settings.ai_prompt, &t.path, &t.id_str(), &root) {
            Ok(prompt) => prompt,
            Err(e) => return self.toast(e),
        };
        let shell = settings.shell.clone();
        let command = terminal::cli_command(cli, &prompt, &terminal::detect_shell(shell.as_deref()).0);
        if self.config.terminal_collapsed {
            self.config.terminal_collapsed = false;
            self.save_config();
        }
        if !self.terminals.is_live(&key) {
            // No shell yet (or it exited): start one that launches the CLI with this prompt.
            self.terminals.restart(&key);
            if let Err(e) = self
                .terminals
                .get_or_start(&key, &root, shell.as_deref(), Some(&command), ctx)
            {
                return self.toast(e);
            }
        } else {
            let session = self
                .terminals
                .get_or_start(&key, &root, shell.as_deref(), None, ctx)
                .expect("session is live");
            if terminal::runs_program(session.shell_pid(), cli.program) {
                // The CLI is already open in the terminal: hand it the prompt directly.
                session.type_prompt(&prompt, ctx);
            } else {
                session.run(&command);
            }
        }
        t.run_count += 1;
        if let Err(e) = t.save() {
            self.toast(format!("Couldn't update run count for {}: {e}", t.id_str()));
        }
        self.reload_active();
    }

    /// Key and working directory for the terminal: the active project, or the home folder.
    fn terminal_target(&self) -> (PathBuf, PathBuf) {
        match self.active_project() {
            Some(p) => (p.ghost_path.clone(), p.root.clone()),
            None => {
                let home = directories::UserDirs::new()
                    .map(|d| d.home_dir().to_path_buf())
                    .unwrap_or_default();
                (PathBuf::new(), home)
            }
        }
    }

    fn terminal_panel(&mut self, ui: &mut Ui) {
        self.terminals.poll();
        let collapsed = self.config.terminal_collapsed;
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(10, 6));
        let panel = if collapsed {
            egui::Panel::bottom("terminal_collapsed")
                .resizable(false)
                .exact_size(34.0)
        } else {
            let max = (ui.available_height() - 160.0).max(120.0);
            egui::Panel::bottom("terminal")
                .resizable(true)
                .default_size(260.0)
                .size_range(100.0..=max)
        };
        let (key, cwd) = self.terminal_target();
        panel.frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (glyph, tip) = if collapsed {
                    (icon::CARET_UP, "Show terminal")
                } else {
                    (icon::CARET_DOWN, "Hide terminal")
                };
                if icon_button(ui, glyph, tip).clicked() {
                    self.config.terminal_collapsed = !collapsed;
                    if let Err(e) = self.config.save() {
                        eprintln!("couldn't save settings: {e}");
                    }
                }
                ui.label(RichText::new(format!("{}  Terminal", icon::TERMINAL_WINDOW)).strong());
            });
            if collapsed {
                return;
            }
            let shell = self.config.settings.shell.clone();
            // A project's terminal opens with its AI CLI running, ready to use.
            let startup = self
                .active_project()
                .and_then(|p| self.project_cli(p).ok())
                .map(|c| c.program);
            match self
                .terminals
                .get_or_start(&key, &cwd, shell.as_deref(), startup, ui.ctx())
            {
                Ok(session) if session.exited => {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Shell exited.").color(theme::TEXT_MUTED));
                        if ui.button("Restart shell").clicked() {
                            self.terminals.restart(&key);
                        }
                    });
                }
                Ok(session) => {
                    let font = egui_term::TerminalFont::new(egui_term::FontSettings {
                        font_type: egui::FontId::monospace(self.config.settings.terminal_font_size),
                    });
                    let focus = self.terminal_focused && self.modal.is_none();
                    let response = Frame::new()
                        .fill(theme::BG)
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::symmetric(12, 6))
                        .show(ui, |ui| {
                            let view = egui_term::TerminalView::new(ui, &mut session.backend)
                                .set_focus(focus)
                                .set_font(font)
                                .set_theme(terminal::theme())
                                .set_size(ui.available_size());
                            ui.add(view)
                        })
                        .response;
                    if ui.input(|i| i.pointer.any_pressed()) {
                        self.terminal_focused = response.contains_pointer();
                    }
                }
                Err(e) => {
                    ui.label(RichText::new(e).color(theme::DANGER));
                }
            }
        });
    }

    fn show_modal(&mut self, ctx: &egui::Context) {
        match &mut self.modal {
            None => {}
            Some(Modal::Project(form)) => {
                if let Some(outcome) = form.show(ctx) {
                    self.project_outcome(outcome, ctx);
                }
            }
            Some(Modal::ViewTicket(id)) => {
                let id = *id;
                let Some(t) = self.find_ticket(id).map(|(_, t)| t.clone()) else {
                    self.modal = None;
                    return;
                };
                match ticket_ui::view(ctx, &t, &mut self.md_cache) {
                    Some(ViewOutcome::Close) => self.modal = None,
                    Some(ViewOutcome::Edit) => self.modal = Some(Modal::EditTicket(Box::new(TicketForm::edit(&t)))),
                    None => {}
                }
            }
            Some(Modal::EditTicket(form)) => match form.show(ctx) {
                Some(FormOutcome::Close) => self.modal = None,
                Some(FormOutcome::Save) => self.save_ticket_form(),
                None => {}
            },
            Some(Modal::Settings(form)) => match form.show(ctx) {
                Some(SettingsOutcome::Save(settings)) => {
                    self.modal = None;
                    self.config.settings = settings;
                    self.save_config();
                }
                Some(SettingsOutcome::Cancel) => self.modal = None,
                None => {}
            },
            Some(Modal::ConfirmDelete(id)) => {
                let id = *id;
                let Some(t) = self.find_ticket(id).map(|(_, t)| t.clone()) else {
                    self.modal = None;
                    return;
                };
                let msg = format!("Delete ticket {} – {}?", t.id_str(), t.title);
                match ticket_ui::confirm(ctx, "delete", &msg, "Cancel", "Delete") {
                    Some(true) => {
                        self.modal = None;
                        match trash::delete(&t.path) {
                            Ok(()) => self.reload_active(),
                            Err(e) => self.toast(format!("Couldn't delete {}: {e}", t.id_str())),
                        }
                    }
                    Some(false) => self.modal = None,
                    None => {}
                }
            }
        }
    }

    fn save_ticket_form(&mut self) {
        let Some(Modal::EditTicket(form)) = &self.modal else {
            return;
        };
        let (mode, mut ticket) = (form.mode, form.ticket.clone());
        ticket.title = ticket.title.trim().to_string();
        let current_path = self.find_ticket(ticket.id).map(|(_, t)| t.path.clone());
        let Some(p) = self.active_project_mut() else { return };
        let result = match mode {
            FormMode::Edit => match current_path {
                Some(path) => {
                    ticket.path = path;
                    ticket.save().and_then(|()| p.rescan()).map(|()| ticket.id)
                }
                None => Err(std::io::Error::other("this ticket no longer exists on disk")),
            },
            FormMode::Create { col } => {
                let col = col.min(p.meta.columns.len().saturating_sub(1));
                p.add_ticket(col, ticket).map(|t| t.id)
            }
        };
        match (result, mode) {
            (Ok(id), FormMode::Edit) => self.modal = Some(Modal::ViewTicket(id)),
            (Ok(_), FormMode::Create { .. }) => self.modal = None,
            (Err(e), _) => {
                if let Some(Modal::EditTicket(form)) = &mut self.modal {
                    form.error = Some(format!("Couldn't save: {e}"));
                }
            }
        }
    }

    fn project_outcome(&mut self, outcome: ProjectOutcome, ctx: &egui::Context) {
        match outcome {
            ProjectOutcome::Cancel => self.modal = None,
            ProjectOutcome::OpenExisting(path) => {
                self.modal = None;
                self.open_path(path, ctx);
            }
            ProjectOutcome::Create {
                dir,
                name,
                columns,
                ai_cli,
            } => match Project::create(&dir, &name, &columns) {
                Ok(mut p) => {
                    self.modal = None;
                    p.meta.ai_cli = Some(ai_cli);
                    if let Err(e) = p.save_meta() {
                        self.toast(format!("Couldn't save the project's AI CLI: {e}"));
                    }
                    self.add_project(p, ctx);
                }
                Err(e) => {
                    if let Some(Modal::Project(form)) = &mut self.modal {
                        form.error = Some(e.to_string());
                    }
                }
            },
            ProjectOutcome::Save {
                name,
                edits,
                targets,
                ai_cli,
            } => {
                self.modal = None;
                let Some(p) = self.active_project_mut() else { return };
                p.meta.ai_cli = Some(ai_cli);
                if let Err(e) = p.apply_columns(&name, &edits, &targets) {
                    self.toast(format!("Couldn't update project: {e}"));
                    self.reload_active();
                }
            }
        }
    }

    fn top_bar(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 8));
        egui::Panel::top("top_bar").frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::Image::new(egui::include_image!("../assets/ghost.svg")).fit_to_exact_size(vec2(28.0, 28.0)),
                );
                ui.label(RichText::new("GhostLane").size(20.0).strong());
                ui.add_space(16.0);
                if !self.projects.is_empty() {
                    self.project_dropdown(ui, actions);
                }
                if self.active_project().is_some() && icon_button(ui, icon::PENCIL_SIMPLE, "Edit project").clicked() {
                    actions.push(Action::EditProject);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, icon::GEAR, "Settings").clicked() {
                        actions.push(Action::OpenSettings);
                    }
                    ui.add_space(8.0);
                    if accent_button(ui, &format!("{}  New Project", icon::PLUS)).clicked() {
                        actions.push(Action::NewProject);
                    }
                    if ui.button(format!("{}  Open Project", icon::FOLDER_OPEN)).clicked() {
                        actions.push(Action::OpenProjectDialog);
                    }
                });
            });
        });
    }

    fn project_dropdown(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let label = self.active.map_or("Select project".into(), |i| self.projects[i].name());
        let btn = ui.button(format!("{label}  {}", icon::CARET_DOWN));
        Popup::menu(&btn).show(|ui| {
            ui.set_min_width(240.0);
            for (i, p) in self.projects.iter().enumerate() {
                let selected = self.active == Some(i);
                let text = match &p.project {
                    Ok(_) => p.name(),
                    Err(_) => format!("{}  {}", icon::WARNING, p.name()),
                };
                let tooltip = match &p.project {
                    Ok(_) => p.path.display().to_string(),
                    Err(e) => e.clone(),
                };
                let row = ui.add_enabled(p.project.is_ok(), egui::Button::selectable(selected, text));
                if row.on_hover_text(tooltip).clicked() && !selected {
                    actions.push(Action::SwitchProject(i));
                }
            }
        });
    }

    fn empty_state(ui: &mut Ui, actions: &mut Vec<Action>) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() / 2.0 - 90.0).max(0.0));
            ui.add(egui::Image::new(egui::include_image!("../assets/ghost.svg")).fit_to_exact_size(vec2(72.0, 72.0)));
            ui.add_space(8.0);
            ui.label(RichText::new("No project open").size(18.0).strong());
            ui.label(RichText::new("Open an existing .ghost file or start a new project.").color(theme::TEXT_MUTED));
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                let width = 2.0 * 170.0 + ui.spacing().item_spacing.x;
                ui.add_space((ui.available_width() - width) / 2.0);
                let size = vec2(170.0, 40.0);
                if ui
                    .add_sized(size, egui::Button::new(format!("{}  Open Project", icon::FOLDER_OPEN)))
                    .clicked()
                {
                    actions.push(Action::OpenProjectDialog);
                }
                let new = egui::Button::new(
                    RichText::new(format!("{}  New Project", icon::PLUS))
                        .color(theme::ACCENT_TEXT)
                        .strong(),
                )
                .fill(theme::ACCENT);
                if ui.add_sized(size, new).clicked() {
                    actions.push(Action::NewProject);
                }
            });
        });
    }

    fn show_toasts(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|(_, t)| t.elapsed() < Duration::from_secs(6));
        if self.toasts.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(500));
        let mut dismissed = None;
        Area::new(Id::new("toasts"))
            .order(Order::Tooltip)
            .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -16.0))
            .show(ctx, |ui| {
                for (i, (msg, _)) in self.toasts.iter().enumerate() {
                    let r = Frame::new()
                        .fill(theme::CARD)
                        .stroke(egui::Stroke::new(1.0, theme::DANGER))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.set_max_width(420.0);
                            ui.label(msg);
                        })
                        .response
                        .interact(egui::Sense::click());
                    if r.clicked() {
                        dismissed = Some(i);
                    }
                }
            });
        if let Some(i) = dismissed {
            self.toasts.remove(i);
        }
    }
}

/// Windows `canonicalize` yields `\\?\C:\...`; keep paths human-readable.
fn strip_unc(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => p,
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_watcher();
        let mut actions = Vec::new();

        self.top_bar(ui, &mut actions);
        self.terminal_panel(ui);

        let central = Frame::new().fill(theme::BG).inner_margin(Margin::same(12));
        egui::CentralPanel::default()
            .frame(central)
            .show(ui, |ui| match self.active {
                Some(i) if self.projects[i].project.is_ok() => ui::board::show(ui, &mut self.projects[i], &mut actions),
                _ => Self::empty_state(ui, &mut actions),
            });

        for action in actions {
            self.apply(action, &ctx);
        }
        self.show_modal(&ctx);
        self.show_toasts(&ctx);
    }
}
