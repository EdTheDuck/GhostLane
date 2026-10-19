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
use crate::project::{AutoMove, Project, Sort};
use crate::terminal::{self, Terminals};
use crate::theme;
use crate::ticket::Ticket;
use crate::ui::import::{ImportForm, ImportOutcome};
use crate::ui::project::{ProjectForm, ProjectOutcome};
use crate::ui::settings::{SettingsForm, SettingsOutcome};
use crate::ui::ticket::{self as ticket_ui, FormMode, FormOutcome, TicketForm, ViewOutcome};
use crate::ui::{self, accent_button, icon_button};

pub struct OpenProject {
    pub path: PathBuf,
    pub project: Result<Project, String>,
    /// Column folder -> search text. Present = search box open.
    pub search: HashMap<String, String>,
    /// Tickets to hand to the AI once it finishes the current one, in order.
    pub queue: Vec<u32>,
    /// Folders of the columns set to Auto. Kept in memory only, so a cloned repo never starts the AI by itself.
    pub auto: std::collections::HashSet<String>,
}

impl OpenProject {
    fn load(path: PathBuf) -> Self {
        let project = Project::load(&path).map_err(|e| e.to_string());
        OpenProject {
            path,
            project,
            search: HashMap::new(),
            queue: Vec::new(),
            auto: std::collections::HashSet::new(),
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
    ImportDocument,
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
    Import(ImportForm),
    /// A short notice with an OK button.
    Message {
        title: String,
        body: String,
    },
}

/// A ticket the AI CLI is writing; GhostLane waits for its file to appear.
pub struct Generation {
    ghost: PathBuf,
    path: PathBuf,
    program: &'static str,
    started: Instant,
    next_alive_check: Instant,
    /// When the file first appeared (or changed); it counts as written once it has been there a moment.
    seen: Option<Instant>,
    /// For updates: the file's modified time before the AI started. `None` = waiting for a new file.
    baseline: Option<std::time::SystemTime>,
    /// Ticket to show once an update lands.
    reopen: Option<u32>,
}

impl Generation {
    fn title(&self) -> &'static str {
        if self.baseline.is_some() {
            "Updating ticket…"
        } else {
            "Generating ticket…"
        }
    }

    /// Has the AI written its file yet?
    fn written(&self) -> bool {
        let modified = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        match self.baseline {
            None => self.path.exists(),
            Some(before) => modified.is_some_and(|m| m != before),
        }
    }
}

/// A ticket the play button handed to the AI CLI.
struct AiRun {
    ghost: PathBuf,
    id: u32,
    started: Instant,
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
    generating: Option<Generation>,
    /// Is the active project's AI CLI running in its terminal? Refreshed every couple of seconds.
    cli_running: bool,
    next_cli_check: Instant,
    /// Last time the user typed into the terminal (their own echo isn't the AI working).
    last_terminal_typing: Option<Instant>,
    /// When a request was last sent to the AI CLI (play button, ticket generation, or Enter in
    /// the terminal). The busy animation only runs for output that follows a request.
    ai_request: Option<Instant>,
    /// Tickets handed to the AI by the play button, waiting to move on "AI Complete".
    ai_runs: Vec<AiRun>,
    toasts: Vec<(String, Instant)>,
    /// Newer release found by the startup check, shown as a top bar button.
    update: Option<String>,
    update_rx: Option<Receiver<String>>,
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
            generating: None,
            cli_running: false,
            next_cli_check: Instant::now(),
            last_terminal_typing: None,
            ai_request: None,
            ai_runs: Vec::new(),
            toasts: Vec::new(),
            update: None,
            update_rx: None,
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
        if app.config.settings.check_for_updates {
            let (tx, rx) = mpsc::channel();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                if let Some(v) = crate::update::newer_release() {
                    let _ = tx.send(v);
                    ctx.request_repaint();
                }
            });
            app.update_rx = Some(rx);
        }
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
        self.active_project()?.find_ticket(id)
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
                    queue: Vec::new(),
                    auto: std::collections::HashSet::new(),
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
                let form = ProjectForm::new_project(clis, &self.config.settings.ai_prompt);
                self.modal = Some(Modal::Project(form));
            }
            Action::EditProject => {
                let clis = self.config.settings.enabled_clis();
                if let Some(p) = self.active_project() {
                    let form = ProjectForm::edit(p, clis, &self.config.settings.ai_prompt);
                    self.modal = Some(Modal::Project(form));
                }
            }
            Action::ImportDocument => self.modal = Some(Modal::Import(ImportForm::default())),
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
            Action::RunAi(id) => self.play(id, ctx),
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

    /// Hands `prompt` to the project's AI CLI in its terminal: typed into the CLI when it is already
    /// running there, otherwise run as a command that starts the CLI with the prompt.
    fn send_to_cli(
        &mut self,
        key: &Path,
        root: &Path,
        cli: &'static terminal::AiCli,
        prompt: &str,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let shell = self.config.settings.shell.clone();
        let command = terminal::cli_command(cli, prompt, &terminal::detect_shell(shell.as_deref()).0);
        if self.config.terminal_collapsed {
            self.config.terminal_collapsed = false;
            self.save_config();
        }
        if !self.terminals.is_live(key) {
            // No shell yet (or it exited): start one that launches the CLI with this prompt.
            self.terminals.restart(key);
            self.terminals
                .get_or_start(key, root, shell.as_deref(), Some(&command), ctx)?;
        } else {
            let session = self
                .terminals
                .get_or_start(key, root, shell.as_deref(), None, ctx)
                .expect("session is live");
            if terminal::runs_program(session.shell_pid(), cli.program) {
                session.type_prompt(prompt, ctx);
            } else {
                session.run(&command);
            }
        }
        self.ai_request = Some(Instant::now());
        Ok(())
    }

    /// Play button: hands ticket `id` to the AI now, or queues it while the AI is on another ticket.
    fn play(&mut self, id: u32, ctx: &egui::Context) {
        let Some(i) = self.active else { return };
        let Ok(p) = &self.projects[i].project else { return };
        if !self.ai_runs.iter().any(|r| r.ghost == p.ghost_path) {
            self.run_ai(i, id, ctx);
        } else if self.projects[i].queue.contains(&id) {
            self.toast(format!("{} is already queued", crate::ticket::format_id(id)));
        } else {
            self.projects[i].queue.push(id);
        }
    }

    /// Hands ticket `id` of project `pi` to its AI CLI. False if it couldn't.
    fn run_ai(&mut self, pi: usize, id: u32, ctx: &egui::Context) -> bool {
        let Some(p) = self.projects.get(pi).and_then(|op| op.project.as_ref().ok()) else {
            return false;
        };
        let Some((col, t)) = p.find_ticket(id) else {
            return false;
        };
        let (key, root, mut t) = (p.ghost_path.clone(), p.root.clone(), t.clone());
        let cli = match self.project_cli(p) {
            Ok(cli) => cli,
            Err(e) => {
                self.toast(e);
                return false;
            }
        };
        let template = p.meta.columns[col]
            .ai_prompt
            .clone()
            .unwrap_or_else(|| self.config.settings.ai_prompt.clone());
        let working_col = p.auto_target(col, AutoMove::AiWorking);
        // The AI reads the ticket after it has moved, so point it at the new location.
        let ai_path = match (working_col, t.path.file_name()) {
            (Some(to), Some(file)) => p.column_dir(to).join(file),
            _ => t.path.clone(),
        };
        let prompt = match terminal::fill_prompt(&template, &ai_path, &t.id_str(), &root) {
            Ok(prompt) => prompt,
            Err(e) => {
                self.toast(e);
                return false;
            }
        };
        if let Err(e) = self.send_to_cli(&key, &root, cli, &prompt, ctx) {
            self.toast(e);
            return false;
        }
        let mut errors = Vec::new();
        t.run_count += 1;
        if let Err(e) = t.save() {
            errors.push(format!("Couldn't update run count for {}: {e}", t.id_str()));
        }
        if let Ok(p) = &mut self.projects[pi].project {
            if let Err(e) = p.rescan() {
                errors.push(format!("Couldn't reload project: {e}"));
            }
            if let Some(to) = working_col
                && let Err(e) = p.move_ticket(&t.path, to)
            {
                errors.push(format!("Couldn't move {}: {e}", t.id_str()));
            }
        }
        for e in errors {
            self.toast(e);
        }
        self.ai_runs.push(AiRun {
            ghost: key,
            id,
            started: Instant::now(),
        });
        true
    }

    /// Once a project's AI CLI goes quiet, moves the tickets it was working on into the next
    /// column if that column takes tickets on "AI Complete".
    // ponytail: "quiet" is the same output heuristic as the busy title, so a CLI waiting on a
    // permission prompt counts as done; a CLI-side completion hook would be exact.
    fn poll_ai_runs(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let terminals = &self.terminals;
        let (done, running): (Vec<AiRun>, Vec<AiRun>) = std::mem::take(&mut self.ai_runs)
            .into_iter()
            .partition(|r| !ai_activity(Some(r.started), terminals.last_output(&r.ghost), None, now).0);
        self.ai_runs = running;
        for run in done {
            let Some(p) = self
                .projects
                .iter_mut()
                .find_map(|op| op.project.as_mut().ok().filter(|p| p.ghost_path == run.ghost))
            else {
                continue;
            };
            let _ = p.rescan();
            let Some((col, t)) = p.find_ticket(run.id) else {
                continue;
            };
            let path = t.path.clone();
            if let Some(to) = p.auto_target(col, AutoMove::AiComplete)
                && let Err(e) = p.move_ticket(&path, to)
            {
                self.toast(format!("Couldn't move {}: {e}", crate::ticket::format_id(run.id)));
            }
        }
        // A project whose AI is free starts its next queued ticket, or with an empty queue, the next one
        // from its Auto columns. Not while a ticket is being written: that prompt is still in flight.
        if self.generating.is_some() {
            return;
        }
        let order = self.config.settings.auto_order;
        for i in 0..self.projects.len() {
            let Ok(p) = &self.projects[i].project else { continue };
            if self.ai_runs.iter().any(|r| r.ghost == p.ghost_path) {
                continue;
            }
            while !self.projects[i].queue.is_empty() {
                let id = self.projects[i].queue.remove(0);
                if self.run_ai(i, id, ctx) {
                    break;
                }
            }
            let op = &self.projects[i];
            let Ok(p) = &op.project else { continue };
            if !op.queue.is_empty() || self.ai_runs.iter().any(|r| r.ghost == p.ghost_path) {
                continue;
            }
            let Some((col, id)) = p.next_auto(&op.auto, order) else {
                continue;
            };
            let started = self.run_ai(i, id, ctx);
            let op = &mut self.projects[i];
            let moved = op
                .project
                .as_ref()
                .is_ok_and(|p| p.find_ticket(id).is_none_or(|(c, _)| c != col));
            if !started || !moved {
                // Otherwise Auto would pick the same ticket again and again.
                op.auto.clear();
                self.toast(format!(
                    "Auto turned off: {} didn't start",
                    crate::ticket::format_id(id)
                ));
            }
        }
    }

    /// Asks the AI CLI to write a new ticket into column `col` and waits for it.
    fn start_generation(&mut self, col: usize, request: &str, ctx: &egui::Context) -> Result<(), String> {
        let p = self.active_project().ok_or("No project is open")?;
        let cli = self.project_cli(p)?;
        let col = col.min(p.meta.columns.len().saturating_sub(1));
        let id = crate::ticket::format_id(p.meta.next_ticket_id);
        let path = p.column_dir(col).join(format!("{id}.md"));
        let (key, root) = (p.ghost_path.clone(), p.root.clone());
        let prompt = terminal::ticket_request_prompt(request, &path, &id);
        self.send_to_cli(&key, &root, cli, &prompt, ctx)?;
        let now = Instant::now();
        self.generating = Some(Generation {
            ghost: key,
            path,
            program: cli.program,
            started: now,
            // Give a freshly started CLI time to appear before checking it is alive.
            next_alive_check: now + Duration::from_secs(8),
            seen: None,
            baseline: None,
            reopen: None,
        });
        Ok(())
    }

    /// Asks the AI CLI to change ticket `id` in place and waits for its file to change.
    fn start_update(&mut self, id: u32, request: &str, ctx: &egui::Context) -> Result<(), String> {
        let p = self.active_project().ok_or("No project is open")?;
        let cli = self.project_cli(p)?;
        let (_, ticket) = self.find_ticket(id).ok_or("This ticket no longer exists on disk")?;
        let path = ticket.path.clone();
        let baseline = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map_err(|e| e.to_string())?;
        let (key, root) = (p.ghost_path.clone(), p.root.clone());
        let prompt = terminal::ticket_update_prompt(request, &path, &crate::ticket::format_id(id));
        self.send_to_cli(&key, &root, cli, &prompt, ctx)?;
        let now = Instant::now();
        self.generating = Some(Generation {
            ghost: key,
            path,
            program: cli.program,
            started: now,
            next_alive_check: now + Duration::from_secs(8),
            seen: None,
            baseline: Some(baseline),
            reopen: Some(id),
        });
        Ok(())
    }

    /// Asks the AI CLI to break the document at `doc` into new tickets in the first column. They show up on
    /// the board as the AI writes them.
    fn start_import(&mut self, doc: &Path, ctx: &egui::Context) -> Result<(), String> {
        if !doc.is_file() {
            return Err(format!("{} isn't a file", doc.display()));
        }
        let p = self.active_project().ok_or("No project is open")?;
        if p.meta.columns.is_empty() {
            return Err("Add a column to the project first".into());
        }
        let cli = self.project_cli(p)?;
        let prompt =
            terminal::tickets_from_document_prompt(doc, &p.column_dir(0), &p.tickets_dir(), p.meta.next_ticket_id);
        let (key, root) = (p.ghost_path.clone(), p.root.clone());
        self.send_to_cli(&key, &root, cli, &prompt, ctx)
    }

    /// A file dropped on the window opens the document import with it (the import modal reads the drop itself).
    fn open_dropped_document(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.raw.dropped_files.iter().any(|f| f.path.is_some())) {
            return;
        }
        if self.active_project().is_none() {
            self.toast("Open a project before dropping in a document");
        } else if self.modal.is_none() && self.generating.is_none() {
            self.modal = Some(Modal::Import(ImportForm::default()));
        }
    }

    /// Finishes the generation once its file is written, or reports why it failed.
    fn poll_generation(&mut self) {
        const TIMEOUT: Duration = Duration::from_secs(15 * 60);
        let Some(g) = &mut self.generating else { return };
        let now = Instant::now();
        let failure = if g.written() {
            let seen = *g.seen.get_or_insert(now);
            if now.duration_since(seen) < Duration::from_millis(800) {
                return;
            }
            match Ticket::load(&g.path) {
                Ok(Some(t)) if !t.malformed => None,
                Ok(_) => Some(format!(
                    "The AI wrote {} but GhostLane couldn't read its title. Open the ticket to fix it.",
                    g.path.display()
                )),
                Err(e) => Some(format!("Couldn't read the generated ticket: {e}")),
            }
        } else if g.started.elapsed() > TIMEOUT {
            Some("The AI didn't finish the ticket within 15 minutes.".to_string())
        } else if now >= g.next_alive_check {
            g.next_alive_check = now + Duration::from_secs(2);
            let alive = self
                .terminals
                .shell_pid(&g.ghost)
                .is_some_and(|pid| terminal::runs_program(pid, g.program));
            if alive {
                return;
            }
            Some("The AI CLI stopped before it finished the ticket. Check the terminal for what happened.".to_string())
        } else {
            return;
        };
        let done = self.generating.take().expect("checked above");
        self.reload_active();
        match failure {
            Some(body) => {
                let title = if done.reopen.is_some() {
                    "Ticket update failed"
                } else {
                    "Ticket generation failed"
                };
                self.modal = Some(Modal::Message {
                    title: title.into(),
                    body,
                });
            }
            None => {
                if let Some(id) = done.reopen {
                    self.modal = Some(Modal::ViewTicket(id));
                }
            }
        }
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

    /// Bottom panel: the terminal (75% of the width) and the AI queue (25%); either one alone gets it all.
    fn terminal_panel(&mut self, ui: &mut Ui) {
        self.terminals.poll();
        let collapsed = self.config.terminal_collapsed;
        let queue_open = self.config.queue_open;
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(10, 6));
        let panel = if collapsed && !queue_open {
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
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let queued = self.active.is_some_and(|i| !self.projects[i].queue.is_empty());
                    let idle = if queued { theme::ACCENT } else { theme::TEXT };
                    let tip = if queue_open { "Hide queue" } else { "Show queue" };
                    if ui::glyph_button(ui, icon::QUEUE, idle, theme::ACCENT, false)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        self.config.queue_open = !queue_open;
                        if let Err(e) = self.config.save() {
                            eprintln!("couldn't save settings: {e}");
                        }
                    }
                });
            });
            if collapsed && !queue_open {
                return;
            }
            const GAP: f32 = 8.0;
            let size = ui.available_size();
            let terminal_width = match (collapsed, queue_open) {
                (true, _) => 0.0,
                (false, true) => (size.x - GAP) * 0.75,
                (false, false) => size.x,
            };
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                if !collapsed {
                    ui.allocate_ui_with_layout(vec2(terminal_width, size.y), Layout::top_down(Align::Min), |ui| {
                        self.terminal_view(ui, &key, &cwd);
                    });
                }
                if queue_open {
                    ui.allocate_ui_with_layout(ui.available_size(), Layout::top_down(Align::Min), |ui| {
                        Frame::new()
                            .fill(theme::BG)
                            .corner_radius(CornerRadius::same(6))
                            .inner_margin(Margin::same(8))
                            .show(ui, |ui| {
                                ui.set_min_size(ui.available_size());
                                match self.active.and_then(|i| self.projects.get_mut(i)) {
                                    Some(op) => ui::queue::show(ui, &mut op.queue, op.project.as_ref().ok()),
                                    None => ui::queue::show(ui, &mut Vec::new(), None),
                                }
                            });
                    });
                }
            });
        });
    }

    fn terminal_view(&mut self, ui: &mut Ui, key: &Path, cwd: &Path) {
        let shell = self.config.settings.shell.clone();
        // A project's terminal opens with its AI CLI running, ready to use.
        let startup = self
            .active_project()
            .and_then(|p| self.project_cli(p).ok())
            .map(|c| c.program);
        match self
            .terminals
            .get_or_start(key, cwd, shell.as_deref(), startup, ui.ctx())
        {
            Ok(session) if session.exited => {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Shell exited.").color(theme::TEXT_MUTED));
                    if ui.button("Restart shell").clicked() {
                        self.terminals.restart(key);
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
                let typed = ui.input(|i| {
                    i.events
                        .iter()
                        .any(|e| matches!(e, egui::Event::Text(_) | egui::Event::Key { pressed: true, .. }))
                });
                if self.terminal_focused && typed {
                    self.last_terminal_typing = Some(Instant::now());
                }
                let submitted = ui.input(|i| {
                    i.events.iter().any(|e| {
                        matches!(
                            e,
                            egui::Event::Key {
                                key: egui::Key::Enter,
                                pressed: true,
                                ..
                            }
                        )
                    })
                });
                if self.terminal_focused && submitted {
                    self.ai_request = Some(Instant::now());
                }
            }
            Err(e) => {
                ui.label(RichText::new(e).color(theme::DANGER));
            }
        }
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
                Some(FormOutcome::Generate(request)) => {
                    let started = match form.mode {
                        FormMode::Create { col } => self.start_generation(col, &request, ctx),
                        FormMode::Edit => {
                            let id = form.ticket.id;
                            self.start_update(id, &request, ctx)
                        }
                    };
                    match started {
                        Ok(()) => self.modal = None,
                        Err(e) => {
                            if let Some(Modal::EditTicket(form)) = &mut self.modal {
                                form.error = Some(e);
                            }
                        }
                    }
                }
                None => {}
            },
            Some(Modal::Import(form)) => match form.show(ctx) {
                Some(ImportOutcome::Close) => self.modal = None,
                Some(ImportOutcome::Generate(doc)) => match self.start_import(&doc, ctx) {
                    Ok(()) => self.modal = None,
                    Err(e) => {
                        if let Some(Modal::Import(form)) = &mut self.modal {
                            form.error = Some(e);
                        }
                    }
                },
                None => {}
            },
            Some(Modal::Message { title, body }) => {
                if ticket_ui::message(ctx, title, body) {
                    self.modal = None;
                }
            }
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
            } => match Project::create(&dir, &name, &columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>()) {
                Ok(mut p) => {
                    self.modal = None;
                    p.meta.ai_cli = Some(ai_cli);
                    for (c, edit) in p.meta.columns.iter_mut().zip(columns) {
                        c.auto_move = edit.auto_move;
                        c.ai_prompt = edit.ai_prompt;
                    }
                    if let Err(e) = p.save_meta() {
                        self.toast(format!("Couldn't save the project's settings: {e}"));
                    }
                    if let Err(e) = write_ai_guide(&p) {
                        self.toast(e);
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
                let result = p
                    .apply_columns(&name, &edits, &targets)
                    .map_err(|e| format!("Couldn't update project: {e}"));
                match result.and_then(|_| write_ai_guide(p)) {
                    Ok(()) => {}
                    Err(e) => {
                        self.toast(e);
                        self.reload_active();
                    }
                }
            }
        }
    }

    /// Is the AI working? True while a ticket is generating, or while the project's AI CLI is
    /// printing output in the terminal that isn't just the echo of the user typing.
    fn ai_busy(&mut self) -> bool {
        if self.generating.is_some() {
            return true;
        }
        let Some(key) = self.active_project().map(|p| p.ghost_path.clone()) else {
            return false;
        };
        let now = Instant::now();
        let output = self.terminals.last_output(&key);
        let (still_active, working) = ai_activity(self.ai_request, output, self.last_terminal_typing, now);
        if !still_active {
            self.ai_request = None;
        }
        if !working {
            return false;
        }
        if now >= self.next_cli_check {
            self.next_cli_check = now + Duration::from_secs(2);
            let program = self
                .active_project()
                .and_then(|p| self.project_cli(p).ok())
                .map(|c| c.program);
            self.cli_running = match (self.terminals.shell_pid(&key), program) {
                (Some(pid), Some(program)) => terminal::runs_program(pid, program),
                _ => false,
            };
        }
        self.cli_running
    }

    /// The top bar is also the window's title bar: drag to move, double-click to maximise.
    fn top_bar(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, busy: bool) {
        let mac = ui::chrome::native_buttons();
        let left = if mac { ui::chrome::MAC_BUTTONS_WIDTH as i8 } else { 12 };
        let right = if mac { 12 } else { 0 };
        let frame = Frame::new().fill(theme::PANEL).inner_margin(Margin {
            left,
            right,
            top: 0,
            bottom: 0,
        });
        egui::Panel::top("top_bar")
            .frame(frame)
            .exact_size(ui::chrome::BAR_HEIGHT)
            .show(ui, |ui| {
                let bar = ui.max_rect();
                ui::chrome::drag_area(ui, bar);
                let (mut left_end, mut right_start) = (bar.left(), bar.right());
                ui.horizontal_centered(|ui| {
                    if !self.projects.is_empty() {
                        self.project_dropdown(ui, actions);
                    }
                    if self.active_project().is_some() && icon_button(ui, icon::PENCIL_SIMPLE, "Edit project").clicked()
                    {
                        actions.push(Action::EditProject);
                    }
                    if self.active_project().is_some()
                        && icon_button(ui, icon::UPLOAD_SIMPLE, "Generate tickets from a document").clicked()
                    {
                        actions.push(Action::ImportDocument);
                    }
                    left_end = ui.min_rect().right();
                    let right_group = ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !mac {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            ui::chrome::window_buttons(ui);
                            ui.spacing_mut().item_spacing.x = 8.0;
                            ui.add_space(8.0);
                        }
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
                        if let Some(v) = &self.update {
                            let text = RichText::new(format!("{}  Update", icon::ARROW_CIRCLE_UP)).color(theme::ACCENT);
                            let tip = format!("GhostLane {v} is available. Download the installer and run it; no need to uninstall first.");
                            if ui.button(text).on_hover_text(tip).clicked() {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(crate::update::RELEASES_URL));
                            }
                        }
                    });
                    right_start = right_group.response.rect.left();
                });
                ui::chrome::title(ui, bar, left_end, right_start, busy);
            });
    }

    fn project_dropdown(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let label = self.active.map_or("Select project".into(), |i| self.projects[i].name());
        let btn = ui.button(format!("{label}  {}", icon::CARET_DOWN));
        // No popup padding: each row's own padding makes its hitbox and highlight run edge to edge.
        let style = ui.style().clone();
        let radius = style.visuals.menu_corner_radius;
        let frame = egui::Frame::popup(&style).inner_margin(Margin::ZERO);
        Popup::menu(&btn).frame(frame).show(|ui| {
            ui.set_min_width(240.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            // egui's menu style shrinks button padding; rows need the full amount.
            ui.spacing_mut().button_padding = vec2(14.0, 8.0);
            let last = self.projects.len().saturating_sub(1);
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
                // Round only the corners that touch the popup's rounded edge.
                let corners = egui::CornerRadius {
                    nw: if i == 0 { radius.nw } else { 0 },
                    ne: if i == 0 { radius.ne } else { 0 },
                    sw: if i == last { radius.sw } else { 0 },
                    se: if i == last { radius.se } else { 0 },
                };
                let button = egui::Button::selectable(selected, text)
                    .corner_radius(corners)
                    .min_size(vec2(0.0, 36.0));
                let row = ui.add_enabled(p.project.is_ok(), button);
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

/// Is terminal output the AI working on a request? Returns (request still active, AI working).
/// A request stays active while the CLI keeps printing and ends after a few quiet seconds. Output
/// with no request behind it (redraws from resizing, the CLI starting up) never counts, and nor
/// does the echo of the user typing.
fn ai_activity(
    request: Option<Instant>,
    output: Option<Instant>,
    typing: Option<Instant>,
    now: Instant,
) -> (bool, bool) {
    let within = |t: Option<Instant>, ms: u64| t.is_some_and(|t| now.duration_since(t) < Duration::from_millis(ms));
    let active = request.is_some() && (within(request, 4000) || within(output, 4000));
    let working = active && within(output, 1200) && !within(typing, 800);
    (active, working)
}

/// Writes the ticket guide into the instructions file the project's AI CLI reads (CLAUDE.md for
/// Claude Code, AGENTS.md for Codex, ...), plus Developer and Tester sub agents where the CLI has them.
fn write_ai_guide(p: &Project) -> Result<(), String> {
    let id = p.meta.ai_cli.as_deref().unwrap_or(terminal::DEFAULT_CLI);
    let Some(cli) = terminal::ai_cli(id) else { return Ok(()) };
    p.write_ai_guide(cli.instructions_file)
        .map_err(|e| format!("Couldn't update {}: {e}", cli.instructions_file))?;
    let skipped = crate::agents::write_agents(p, cli.id).map_err(|e| format!("Couldn't write the AI agents: {e}"))?;
    match skipped.as_slice() {
        [] => Ok(()),
        files => Err(format!(
            "Left {} as it is: it wasn't made by GhostLane",
            files
                .iter()
                .map(|f| f.display().to_string())
                .collect::<Vec<_>>()
                .join(" and ")
        )),
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
        self.poll_generation();
        self.poll_ai_runs(&ctx);
        if let Some(v) = self.update_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.update = Some(v);
        }
        let mut actions = Vec::new();

        let inset = ui::chrome::maximized_overflow(&ctx);
        if inset > 0.0 {
            let pad = Frame::new().fill(theme::PANEL);
            egui::Panel::top("max_inset_top")
                .exact_size(inset)
                .frame(pad)
                .show_separator_line(false)
                .show(ui, |_| {});
            egui::Panel::bottom("max_inset_bottom")
                .exact_size(inset)
                .frame(pad)
                .show_separator_line(false)
                .show(ui, |_| {});
            egui::Panel::left("max_inset_left")
                .exact_size(inset)
                .resizable(false)
                .frame(pad)
                .show_separator_line(false)
                .show(ui, |_| {});
            egui::Panel::right("max_inset_right")
                .exact_size(inset)
                .resizable(false)
                .frame(pad)
                .show_separator_line(false)
                .show(ui, |_| {});
        }
        let busy = self.ai_busy();
        self.top_bar(ui, &mut actions, busy);
        self.terminal_panel(ui);

        let central = Frame::new().fill(theme::BG).inner_margin(Margin::same(12));
        let board = egui::CentralPanel::default()
            .frame(central)
            .show(ui, |ui| match self.active {
                Some(i) if self.projects[i].project.is_ok() => ui::board::show(ui, &mut self.projects[i], &mut actions),
                _ => Self::empty_state(ui, &mut actions),
            })
            .response
            .rect;
        if let Some(title) = self.generating.as_ref().map(Generation::title)
            && ui::generating::overlay(&ctx, board, title)
        {
            self.generating = None;
        }

        let hovering_file = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if hovering_file && self.modal.is_none() && self.generating.is_none() && self.active_project().is_some() {
            ui::import::drop_hint(&ctx, board);
        }

        for action in actions {
            self.apply(action, &ctx);
        }
        self.open_dropped_document(&ctx);
        self.show_modal(&ctx);
        self.show_toasts(&ctx);
        ui::chrome::resize_edges(ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generation(path: PathBuf, baseline: Option<std::time::SystemTime>) -> Generation {
        let now = Instant::now();
        Generation {
            ghost: PathBuf::new(),
            path,
            program: "claude",
            started: now,
            next_alive_check: now,
            seen: None,
            baseline,
            reopen: None,
        }
    }

    #[test]
    fn new_tickets_are_written_once_the_file_exists() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("00001.md");
        assert!(!generation(path.clone(), None).written());
        std::fs::write(&path, "x").unwrap();
        assert!(generation(path, None).written());
    }

    #[test]
    fn updates_are_written_once_the_file_changes() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("00001.md");
        std::fs::write(&path, "x").unwrap();
        let now = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert!(!generation(path.clone(), Some(now)).written());
        assert!(generation(path, Some(std::time::UNIX_EPOCH)).written());
    }

    #[test]
    fn title_only_animates_for_output_after_a_request() {
        let now = Instant::now();
        let ago = |ms: u64| Some(now - Duration::from_millis(ms));
        // Resizing makes the CLI redraw, but with no request that isn't the AI working.
        assert_eq!(ai_activity(None, ago(100), None, now), (false, false));
        // Output soon after a request is the AI working.
        assert_eq!(ai_activity(ago(2000), ago(100), None, now), (true, true));
        // Long jobs stay busy while output keeps coming.
        assert_eq!(ai_activity(ago(60_000), ago(200), None, now), (true, true));
        // A quiet CLI ends the request.
        assert_eq!(ai_activity(ago(10_000), ago(5000), None, now), (false, false));
        // The echo of the user typing isn't the AI either.
        assert_eq!(ai_activity(ago(2000), ago(100), ago(200), now), (true, false));
    }
}
