use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{
    Align, Align2, Area, CornerRadius, Frame, Id, Key, KeyboardShortcut, Layout, Margin, Modifiers, Order, Popup,
    RichText, Ui, vec2,
};
use egui_commonmark::CommonMarkCache;
use egui_phosphor::regular as icon;
use notify_debouncer_full::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::config::AppConfig;
use crate::project::{AutoMove, Project, Sort};
use crate::terminal::{self, Terminals};
use crate::theme;
use crate::ticket::{Filter, Priority, RunRecord, RunResult, Ticket, format_id};
use crate::ui::import::{ImportForm, ImportOutcome};
use crate::ui::project::{ProjectForm, ProjectOutcome};
use crate::ui::quick_open::{QuickOpen, QuickOutcome};
use crate::ui::settings::{SettingsForm, SettingsOutcome};
use crate::ui::ticket::{self as ticket_ui, FormMode, FormOutcome, TicketForm, TicketView, ViewOutcome};
use crate::ui::{self, accent_button, icon_button};

pub struct OpenProject {
    pub path: PathBuf,
    pub project: Result<Project, String>,
    /// Column folder -> search text. Present = search box open.
    pub search: HashMap<String, String>,
    /// Tickets to hand to the AI once it finishes the current one, in order.
    pub queue: Vec<u32>,
    /// Folders of the columns set to Auto. Kept in memory only, so a cloned repo never starts the AI by itself.
    pub auto: HashSet<String>,
    /// Start nothing new from the queue or Auto columns; the ticket the AI is on still finishes.
    pub paused: bool,
    /// The last column shows its archived tickets too.
    pub show_archived: bool,
    /// Cards picked with Ctrl or Shift click, for acting on several at once.
    pub selected: HashSet<u32>,
    /// The card last clicked; Shift-click selects from here.
    pub anchor: Option<u32>,
    /// Board filter, kept while the app runs.
    pub filter: Filter,
    /// The filter bar shows above the board.
    pub filter_open: bool,
    /// The card the arrow keys are on.
    pub cursor: Option<u32>,
    /// Scroll the cursor's card into view on the next frame.
    pub scroll_to_cursor: bool,
}

impl OpenProject {
    pub fn new(path: PathBuf, project: Result<Project, String>) -> Self {
        OpenProject {
            path,
            project,
            search: HashMap::new(),
            queue: Vec::new(),
            auto: HashSet::new(),
            paused: false,
            show_archived: false,
            selected: HashSet::new(),
            anchor: None,
            filter: Filter::default(),
            filter_open: false,
            cursor: None,
            scroll_to_cursor: false,
        }
    }

    fn load(path: PathBuf) -> Self {
        let project = Project::load(&path).map_err(|e| e.to_string());
        Self::new(path, project)
    }

    pub fn name(&self) -> String {
        match &self.project {
            Ok(p) => p.meta.name.clone(),
            Err(_) => self.path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        }
    }
}

/// Things the UI asks for; applied after drawing so widgets never hold borrows across mutations.
#[derive(Clone)]
pub enum Action {
    SwitchProject(usize),
    OpenProjectDialog,
    NewProject,
    EditProject,
    ImportDocument,
    OpenSettings,
    MoveTickets {
        ids: Vec<u32>,
        to_col: usize,
    },
    SetSort {
        col: usize,
        sort: Sort,
    },
    ViewTicket(u32),
    EditTicket(u32),
    NewTicket(usize),
    DuplicateTicket(u32),
    AskDelete(Vec<u32>),
    RunAi(u32),
    /// Run the first now and queue the rest, like pressing ▶ on each in turn.
    QueueTickets(Vec<u32>),
    SetPriority {
        ids: Vec<u32>,
        priority: Priority,
    },
    ResetRunCount(u32),
    QuickOpen,
    ShowStats,
    ToggleFilter,
    /// Open the filter bar with this label ticked, or untick it if it already is.
    FilterByLabel(String),
    Export(ExportFormat),
    /// Switch Auto on or off for the column with this folder.
    ToggleAuto(String),
    /// Pause or resume the AI queue and Auto.
    TogglePause,
}

#[derive(Clone, Copy)]
pub enum ExportFormat {
    Csv,
    Markdown,
}

pub enum Modal {
    Project(ProjectForm),
    ViewTicket(TicketView),
    EditTicket(Box<TicketForm>),
    ConfirmDelete(Vec<u32>),
    QuickOpen(QuickOpen),
    Stats,
    Settings(SettingsForm),
    Import(ImportForm),
    /// A short notice with an OK button.
    Message {
        title: String,
        body: String,
    },
}

/// The AI CLI that work was handed to, in the terminal of the project with .ghost `ghost`.
struct CliWatch {
    ghost: PathBuf,
    program: &'static str,
    started: Instant,
    next_alive_check: Instant,
}

impl CliWatch {
    fn new(ghost: PathBuf, program: &'static str) -> Self {
        let now = Instant::now();
        CliWatch {
            ghost,
            program,
            started: now,
            // Give a freshly started CLI time to appear before checking it is alive.
            next_alive_check: now + Duration::from_secs(8),
        }
    }

    /// Has the CLI quit? Looked up every couple of seconds; false in between.
    fn gone(&mut self, terminals: &Terminals, now: Instant) -> bool {
        if now < self.next_alive_check {
            return false;
        }
        self.next_alive_check = now + Duration::from_secs(2);
        !terminals.runs(&self.ghost, self.program)
    }
}

/// A ticket the AI CLI is writing; GhostLane waits for its file to appear.
pub struct Generation {
    cli: CliWatch,
    path: PathBuf,
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
    cli: CliWatch,
    id: u32,
    /// Folder of the column the ticket was in when it started; ending up left of it means it failed.
    from: String,
    started_at: std::time::SystemTime,
    /// The git commit checked out when it started.
    base: Option<String>,
    /// The CLI's completion hook touches this when it finishes. `None` = the CLI has no hook, so a quiet
    /// terminal counts as finished.
    marker: Option<PathBuf>,
}

/// How a run ended.
enum RunEnd {
    Finished,
    /// The CLI quit before its completion hook fired.
    CliGone,
}

/// A ticket the AI finished in the last column, waiting on the project's test command.
struct Check {
    run: AiRun,
    /// How long the AI took.
    seconds: u64,
    command: String,
}

/// Work done on a background thread, reported back to the UI.
enum Job {
    /// The test command of project `ghost` finished for ticket `id`.
    Checked {
        ghost: PathBuf,
        id: u32,
        passed: bool,
        output: String,
    },
    GitFailed(String),
    /// Reverting ticket `id`'s commit in project `ghost` finished.
    Reverted {
        ghost: PathBuf,
        id: u32,
        result: Result<(), String>,
    },
}

/// One thing Ctrl+Z puts back.
enum UndoStep {
    /// Ticket `id` was moved out of the column with this folder.
    Move { ghost: PathBuf, id: u32, folder: String },
    /// A ticket file was sent to the trash; it is written back from what it held.
    Delete {
        ghost: PathBuf,
        path: PathBuf,
        contents: String,
    },
}

/// Undo steps kept; older ones are dropped.
const MAX_UNDO: usize = 50;

type Watcher = Debouncer<RecommendedWatcher, RecommendedCache>;

const FIND: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::K);
const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);

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
    /// Finished runs whose test command is still going. Their project starts nothing new meanwhile.
    checks: Vec<Check>,
    /// Card moves and deletes, newest last; each entry is undone in one go.
    undo: Vec<Vec<UndoStep>>,
    jobs_tx: Sender<Job>,
    jobs_rx: Receiver<Job>,
    toasts: Vec<(String, Instant)>,
    /// Newer release found by the startup check, shown as a top bar button.
    update: Option<String>,
    update_rx: Option<Receiver<String>>,
    /// .ghost files handed over by later launches (see `instance`).
    handed_rx: Option<Receiver<PathBuf>>,
    watcher: Option<Watcher>,
    watch_tx: Sender<DebounceEventResult>,
    watch_rx: Receiver<DebounceEventResult>,
}

impl App {
    /// `open`: a .ghost file GhostLane was started with, opened and added to the projects.
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
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
        let (jobs_tx, jobs_rx) = mpsc::channel();
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
            checks: Vec::new(),
            undo: Vec::new(),
            jobs_tx,
            jobs_rx,
            toasts: Vec::new(),
            update: None,
            update_rx: None,
            handed_rx: None,
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
        if let Some(path) = open {
            app.open_path(path, ctx);
        }
        let (tx, rx) = mpsc::channel();
        let wake = ctx.clone();
        match crate::instance::listen(tx, move || wake.request_repaint()) {
            Ok(()) => app.handed_rx = Some(rx),
            Err(e) => eprintln!("can't take .ghost files from other launches: {e}"),
        }
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

    /// The open project with .ghost `ghost`.
    fn project_mut(&mut self, ghost: &Path) -> Option<&mut Project> {
        self.projects
            .iter_mut()
            .find_map(|op| op.project.as_mut().ok().filter(|p| p.ghost_path == ghost))
    }

    /// Opens ticket `id`'s view, with the commit it was done in when it's in the last column.
    fn open_ticket(&mut self, id: u32) {
        let commit = self.active_project().and_then(|p| {
            let (col, t) = p.find_ticket(id)?;
            let done_at = t.done_at.filter(|_| col == p.last_col())?;
            crate::git::ticket_commit(&p.root, &t.id_str(), done_at)
        });
        self.modal = Some(Modal::ViewTicket(TicketView::new(id, commit)));
    }

    fn push_undo(&mut self, steps: Vec<UndoStep>) {
        if steps.is_empty() {
            return;
        }
        self.undo.push(steps);
        let extra = self.undo.len().saturating_sub(MAX_UNDO);
        self.undo.drain(..extra);
    }

    /// Ctrl+Z: puts back the last card move or delete.
    fn undo(&mut self) {
        let Some(steps) = self.undo.pop() else { return };
        let errors: Vec<String> = steps
            .into_iter()
            .rev()
            .filter_map(|step| undo_step(&mut self.projects, step).err())
            .collect();
        for e in errors {
            self.toast(e);
        }
    }

    /// Ctrl+K finds a ticket and Ctrl+Z undoes the last card move or delete (Cmd on macOS), unless something else
    /// has the keyboard: a dialog, a text box or the terminal.
    fn shortcuts(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if !self.keyboard_free(ctx) || self.active_project().is_none() {
            return;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&FIND)) {
            actions.push(Action::QuickOpen);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&UNDO)) {
            self.undo();
        }
    }

    /// The ticket form as a modal, offering the active project's tickets for Blocked by.
    fn ticket_form(&self, mut form: TicketForm) -> Modal {
        if let Some(p) = self.active_project() {
            form.tickets = p
                .tickets
                .iter()
                .flatten()
                .map(|t| (t.id, t.priority, t.title.clone()))
                .collect();
            form.tickets.sort_by_key(|(id, _, _)| *id);
        }
        Modal::EditTicket(Box::new(form))
    }

    /// Nothing else has the keyboard: no dialog, text box or terminal.
    fn keyboard_free(&self, ctx: &egui::Context) -> bool {
        self.modal.is_none() && !self.terminal_focused && ctx.memory(|m| m.focused().is_none())
    }

    /// What Ctrl+K can run besides opening a ticket, for the active project.
    fn commands(&self) -> Vec<(String, Action)> {
        let mut c: Vec<(String, Action)> = Vec::new();
        let Some(i) = self.active else { return c };
        let op = &self.projects[i];
        if let Ok(p) = &op.project {
            for (col, column) in p.meta.columns.iter().enumerate() {
                c.push((format!("New ticket in {}", column.name), Action::NewTicket(col)));
            }
            for (col, column) in p.meta.columns.iter().enumerate() {
                if p.auto_target(col, AutoMove::AiWorking).is_some() {
                    let verb = if op.auto.contains(&column.folder) { "off" } else { "on" };
                    let action = Action::ToggleAuto(column.folder.clone());
                    c.push((format!("Turn Auto {verb} for {}", column.name), action));
                }
            }
        }
        let pause = if op.paused { "Resume the AI" } else { "Pause the AI" };
        let filter = if op.filter_open {
            "Close the filter"
        } else {
            "Filter the board"
        };
        c.extend([
            (pause.to_string(), Action::TogglePause),
            (filter.to_string(), Action::ToggleFilter),
            ("Project stats".to_string(), Action::ShowStats),
            ("Edit project".to_string(), Action::EditProject),
            ("Generate tickets from a document".to_string(), Action::ImportDocument),
            ("Export tickets as CSV".to_string(), Action::Export(ExportFormat::Csv)),
            (
                "Export board as Markdown report".to_string(),
                Action::Export(ExportFormat::Markdown),
            ),
            ("Settings".to_string(), Action::OpenSettings),
            ("New project".to_string(), Action::NewProject),
            ("Open project".to_string(), Action::OpenProjectDialog),
        ]);
        for (j, other) in self.projects.iter().enumerate() {
            if j != i && other.project.is_ok() {
                c.push((format!("Switch to {}", other.name()), Action::SwitchProject(j)));
            }
        }
        c
    }

    /// Sends `tickets` to the trash, remembering what they held so Ctrl+Z can bring them back.
    fn delete_tickets(&mut self, tickets: &[Ticket]) {
        let Some(ghost) = self.active_project().map(|p| p.ghost_path.clone()) else {
            return;
        };
        let mut undo = Vec::new();
        for t in tickets {
            let contents = std::fs::read_to_string(&t.path);
            match trash::delete(&t.path) {
                Ok(()) => undo.extend(contents.ok().map(|contents| UndoStep::Delete {
                    ghost: ghost.clone(),
                    path: t.path.clone(),
                    contents,
                })),
                Err(e) => self.toast(format!("Couldn't delete {}: {e}", t.id_str())),
            }
        }
        self.push_undo(undo);
        self.reload_active();
    }

    /// Review found a problem: the note goes into the ticket's notes and it goes back to the first column.
    fn reject(&mut self, id: u32, note: &str) {
        let Some(mut t) = self.find_ticket(id).map(|(_, t)| t.clone()) else {
            return;
        };
        let when = crate::ticket::readable_utc(&crate::ticket::utc_timestamp(crate::ticket::now_secs()));
        t.notes = append_note(&t.notes, &format!("Rejected in review on {when}:\n\n{}", note.trim()));
        let Some(p) = self.active_project_mut() else { return };
        if let Err(e) = t.save().and_then(|()| p.move_ticket(&t.path, 0)) {
            self.toast(format!("Couldn't send {} back: {e}", t.id_str()));
        }
    }

    /// Undoes `commit`, the one ticket `id` was done in, on a background thread: commit hooks can take a while.
    fn revert(&mut self, id: u32, commit: String, ctx: &egui::Context) {
        let Some((p, (_, t))) = self.active_project().and_then(|p| Some((p, p.find_ticket(id)?))) else {
            return;
        };
        let message = format!("Revert \"{}: {}\"", t.id_str(), t.title.replace(['\n', '\r'], " "));
        let (root, ghost) = (p.root.clone(), p.ghost_path.clone());
        let (tx, ctx) = (self.jobs_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = crate::git::revert(&root, &commit, &message);
            let _ = tx.send(Job::Reverted { ghost, id, result });
            ctx.request_repaint();
        });
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
            Some(i) => {
                self.projects[i].project = Ok(project);
                i
            }
            None => {
                self.projects.push(OpenProject::new(path, Ok(project)));
                self.projects.len() - 1
            }
        };
        self.switch_to(i, ctx);
    }

    /// Closes the active project and forgets it, stopping its AI first. `files`: also sends its .ghost file and
    /// tickets folder to the trash; nothing else in its folder is touched.
    fn delete_project(&mut self, files: bool, ctx: &egui::Context) {
        let Some(i) = self.active else { return };
        if self.ai_running(i) {
            self.stop_ai(i);
        }
        let op = self.projects.remove(i);
        self.watcher = None;
        let ghost = op.project.as_ref().map_or(op.path.clone(), |p| p.ghost_path.clone());
        self.terminals.restart(&ghost);
        self.config.projects.remove(&ghost);
        if files && let Ok(p) = &op.project {
            for path in [p.ghost_path.clone(), p.tickets_dir()] {
                if path.exists()
                    && let Err(e) = trash::delete(&path)
                {
                    self.toast(format!("Couldn't delete {}: {e}", path.display()));
                }
            }
        }
        self.active = self.projects.iter().position(|p| p.project.is_ok());
        self.save_config();
        self.reload_active();
        self.watch_active(ctx);
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
            Action::MoveTickets { ids, to_col } => {
                let mut undo = Vec::new();
                for id in ids {
                    let Some((from, t)) = self.find_ticket(id) else {
                        continue;
                    };
                    if from == to_col {
                        continue;
                    }
                    let path = t.path.clone();
                    let p = self.active_project_mut().expect("ticket found in active project");
                    let (ghost, done) = (p.ghost_path.clone(), to_col == p.last_col());
                    let folder = p.meta.columns[from].folder.clone();
                    match p.move_ticket(&path, to_col) {
                        Ok(()) => {
                            undo.push(UndoStep::Move {
                                ghost: ghost.clone(),
                                id,
                                folder,
                            });
                            if done {
                                self.ticket_done(&ghost, id, ctx);
                            }
                        }
                        Err(e) => self.toast(format!("Couldn't move {}: {e}", format_id(id))),
                    }
                }
                self.push_undo(undo);
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
                    let settings = self.config.project(&p.ghost_path);
                    let form = ProjectForm::edit(p, clis, &self.config.settings.ai_prompt, settings);
                    self.modal = Some(Modal::Project(form));
                }
            }
            Action::ImportDocument => self.modal = Some(Modal::Import(ImportForm::default())),
            Action::ViewTicket(id) => self.open_ticket(id),
            Action::EditTicket(id) => {
                if let Some((_, t)) = self.find_ticket(id) {
                    self.modal = Some(self.ticket_form(TicketForm::edit(t)));
                }
            }
            Action::NewTicket(col) => {
                let Some(p) = self.active_project() else { return };
                let t = Ticket {
                    id: p.meta.next_ticket_id,
                    ..Default::default()
                };
                self.modal = Some(self.ticket_form(TicketForm::create(col, t)));
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
                    runs: Vec::new(),
                    done_at: None,
                    ..t.clone()
                };
                self.modal = Some(self.ticket_form(TicketForm::create(col, copy)));
            }
            Action::AskDelete(ids) if ids.is_empty() => {}
            Action::AskDelete(ids) => self.modal = Some(Modal::ConfirmDelete(ids)),
            Action::RunAi(id) => self.play(id, ctx),
            Action::QueueTickets(ids) => {
                for id in ids {
                    // Columns the AI fills by itself have no ▶.
                    let filled_by_ai = self.active_project().is_some_and(|p| {
                        p.find_ticket(id)
                            .is_some_and(|(c, _)| p.meta.columns[c].auto_move == AutoMove::AiWorking)
                    });
                    if !filled_by_ai {
                        self.play(id, ctx);
                    }
                }
            }
            Action::SetPriority { ids, priority } => {
                for id in ids {
                    let Some(mut t) = self.find_ticket(id).map(|(_, t)| t.clone()) else {
                        continue;
                    };
                    t.priority = priority;
                    if let Err(e) = t.save() {
                        self.toast(format!("Couldn't update {}: {e}", t.id_str()));
                    }
                }
                self.reload_active();
            }
            Action::QuickOpen => self.modal = Some(Modal::QuickOpen(QuickOpen::default())),
            Action::ShowStats => self.modal = Some(Modal::Stats),
            Action::Export(format) => self.export(format),
            Action::ToggleAuto(folder) => {
                if let Some(op) = self.active.and_then(|i| self.projects.get_mut(i))
                    && !op.auto.remove(&folder)
                {
                    op.auto.insert(folder);
                }
            }
            Action::TogglePause => {
                if let Some(op) = self.active.and_then(|i| self.projects.get_mut(i)) {
                    op.paused = !op.paused;
                }
            }
            Action::FilterByLabel(label) => {
                if let Some(op) = self.active.and_then(|i| self.projects.get_mut(i)) {
                    op.filter_open = true;
                    let labels = &mut op.filter.labels;
                    match labels.iter().position(|l| l.to_lowercase() == label.to_lowercase()) {
                        Some(i) => _ = labels.remove(i),
                        None => labels.push(label),
                    }
                }
            }
            Action::ToggleFilter => {
                if let Some(op) = self.active.and_then(|i| self.projects.get_mut(i)) {
                    // Closing the bar drops the filter too, so nothing stays hidden out of sight.
                    if op.filter_open {
                        op.filter = Filter::default();
                    }
                    op.filter_open = !op.filter_open;
                }
            }
            Action::ResetRunCount(id) => {
                let Some(mut t) = self.find_ticket(id).map(|(_, t)| t.clone()) else {
                    return;
                };
                t.run_count = 0;
                if let Err(e) = t.save() {
                    self.toast(format!("Couldn't reset {}: {e}", t.id_str()));
                }
                self.reload_active();
            }
            Action::OpenSettings => self.modal = Some(Modal::Settings(SettingsForm::new(&self.config.settings))),
        }
    }

    /// Saves the active board as a CSV file or a Markdown report wherever the user picks.
    fn export(&mut self, format: ExportFormat) {
        let Some(p) = self.active_project() else { return };
        let stem = p
            .ghost_path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let (name, kind, ext, text) = match format {
            ExportFormat::Csv => (format!("{stem}_tickets.csv"), "CSV", "csv", p.export_csv()),
            ExportFormat::Markdown => (
                format!("{stem}_board.md"),
                "Markdown",
                "md",
                p.export_markdown(crate::ticket::now_secs()),
            ),
        };
        let picked = rfd::FileDialog::new()
            .set_directory(&p.root)
            .set_file_name(name)
            .add_filter(kind, &[ext])
            .save_file();
        if let Some(path) = picked
            && let Err(e) = crate::ticket::write_atomic(&path, &text)
        {
            self.toast(format!("Couldn't export to {}: {e}", path.display()));
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

    /// Completion hook files for the project with .ghost `key`, with the Claude Code settings written.
    /// `None` when they can't be set up; runs then finish once the terminal goes quiet.
    fn done_hook(key: &Path) -> Option<terminal::DoneHook> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        key.hash(&mut hasher);
        let name = format!("{:016x}", hasher.finish());
        let dir = crate::config::config_dir()?.join("hooks");
        let hook = terminal::DoneHook {
            exe: std::env::current_exe().ok().map(strip_unc)?,
            marker: dir.join(format!("{name}.done")),
            settings: dir.join(format!("{name}.json")),
        };
        let json = terminal::claude_hook_settings(&hook)?;
        if let Err(e) = crate::ticket::write_atomic(&hook.settings, &json) {
            eprintln!("couldn't write {}: {e}", hook.settings.display());
            return None;
        }
        Some(hook)
    }

    /// Shell command that starts `cli`, with its completion hook when it has one, and `prompt`.
    /// Also returns the hook's marker file.
    fn cli_launch(&self, key: &Path, cli: &'static terminal::AiCli, prompt: Option<&str>) -> (String, Option<PathBuf>) {
        let shell = terminal::detect_shell(self.config.settings.shell.as_deref()).0;
        let hook = Self::done_hook(key);
        let args = hook.as_ref().and_then(|h| terminal::done_hook_args(cli, h, &shell));
        let marker = hook.filter(|_| args.is_some()).map(|h| h.marker);
        (terminal::cli_command(cli, args.as_deref(), prompt, &shell), marker)
    }

    /// Hands `prompt` to the project's AI CLI in its terminal: typed into the CLI when it is already
    /// running there, otherwise run as a command that starts the CLI with the prompt. Returns the marker
    /// file the CLI's completion hook touches, if it was started with one.
    fn send_to_cli(
        &mut self,
        key: &Path,
        root: &Path,
        cli: &'static terminal::AiCli,
        prompt: &str,
        ctx: &egui::Context,
    ) -> Result<Option<PathBuf>, String> {
        let shell = self.config.settings.shell.clone();
        let (command, mut marker) = self.cli_launch(key, cli, Some(prompt));
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
            match terminal::program_command_line(session.shell_pid(), cli.program) {
                Some(running) => {
                    session.type_prompt(prompt, ctx);
                    // Started by hand, the CLI has no hook; GhostLane's launches name the hook's files.
                    let hooked = |m: &PathBuf| {
                        m.file_stem()
                            .is_some_and(|stem| running.contains(&stem.to_string_lossy().to_lowercase()))
                    };
                    marker = marker.filter(hooked);
                }
                None => session.run(&command),
            }
        }
        self.ai_request = Some(Instant::now());
        Ok(marker)
    }

    /// Play button: hands ticket `id` to the AI now, or queues it while the AI is on another ticket.
    fn play(&mut self, id: u32, ctx: &egui::Context) {
        let Some(i) = self.active else { return };
        if !self.ai_engaged(i) {
            self.run_ai(i, id, ctx);
        } else if self.projects[i].queue.contains(&id) {
            self.toast(format!("{} is already queued", crate::ticket::format_id(id)));
        } else {
            self.projects[i].queue.push(id);
        }
    }

    /// Is project `i`'s AI on a ticket from the play button, or are tickets waiting for it? Play buttons queue then.
    fn ai_engaged(&self, i: usize) -> bool {
        (self.next_queued(i).is_some() && !self.projects[i].paused) || self.ai_running(i)
    }

    /// Position in project `i`'s queue of the first ticket not waiting for others.
    fn next_queued(&self, i: usize) -> Option<usize> {
        let op = &self.projects[i];
        let blocked = |id: u32| op.project.as_ref().is_ok_and(|p| p.is_blocked(id));
        op.queue.iter().position(|&id| !blocked(id))
    }

    /// Is project `i`'s AI on a ticket from the play button, or is its test command checking one?
    fn ai_running(&self, i: usize) -> bool {
        let ghost = self.projects[i].project.as_ref().map(|p| &p.ghost_path);
        let ours = |g: &PathBuf| ghost.is_ok_and(|ghost| ghost == g);
        self.ai_runs.iter().any(|r| ours(&r.cli.ghost)) || self.checks.iter().any(|c| ours(&c.run.cli.ghost))
    }

    /// Stop button: interrupts the AI's current ticket, leaving it where it is, and pauses the queue.
    fn stop_ai(&mut self, i: usize) {
        self.projects[i].paused = true;
        let Ok(p) = &mut self.projects[i].project else { return };
        let key = p.ghost_path.clone();
        let (stopped, running): (Vec<AiRun>, Vec<AiRun>) = std::mem::take(&mut self.ai_runs)
            .into_iter()
            .partition(|r| r.cli.ghost == key);
        self.ai_runs = running;
        let (checks, others): (Vec<Check>, Vec<Check>) = std::mem::take(&mut self.checks)
            .into_iter()
            .partition(|c| c.run.cli.ghost == key);
        self.checks = others;
        let mut errors = Vec::new();
        for run in stopped.iter().chain(checks.iter().map(|c| &c.run)) {
            errors.extend(record_run(p, run, elapsed(run), RunResult::Stopped).err());
        }
        for e in errors {
            self.toast(e);
        }
        let Some(cli) = self.projects[i]
            .project
            .as_ref()
            .ok()
            .and_then(|p| self.project_cli(p).ok())
        else {
            return;
        };
        if let Some(session) = self.terminals.live_mut(&key) {
            session.interrupt(cli.interrupt);
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
        let from = p.meta.columns[col].folder.clone();
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
        let started_at = std::time::SystemTime::now();
        // Takes milliseconds; outside a repository there is just nothing to show later.
        let base = crate::git::head(&root);
        let marker = match self.send_to_cli(&key, &root, cli, &prompt, ctx) {
            Ok(marker) => marker,
            Err(e) => {
                self.toast(e);
                return false;
            }
        };
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
            cli: CliWatch::new(key, cli.program),
            id,
            from,
            started_at,
            marker,
            base,
        });
        true
    }

    /// Has `run` ended? Hooked runs end when the hook fires (or the CLI quits), others once the terminal
    /// goes quiet.
    fn run_end(&self, run: &mut AiRun, now: Instant) -> Option<RunEnd> {
        let Some(marker) = &run.marker else {
            let output = self.terminals.last_output(&run.cli.ghost);
            return (!ai_activity(Some(run.cli.started), output, None, now).0).then_some(RunEnd::Finished);
        };
        let modified = std::fs::metadata(marker).and_then(|m| m.modified());
        if modified.is_ok_and(|m| m > run.started_at) {
            return Some(RunEnd::Finished);
        }
        run.cli.gone(&self.terminals, now).then_some(RunEnd::CliGone)
    }

    /// Once a project's AI CLI has finished a ticket, moves it into the next column if that column takes
    /// tickets on "AI Complete", then starts the next queued or Auto ticket.
    // ponytail: CLIs without a completion hook finish once the terminal goes quiet (the busy title's
    // heuristic), so one waiting on a permission prompt counts as done.
    fn poll_ai_runs(&mut self, ctx: &egui::Context) {
        if !self.ai_runs.is_empty() {
            // A quiet CLI makes no repaints, so look again to notice it has finished.
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        let now = Instant::now();
        let mut ended = Vec::new();
        for mut run in std::mem::take(&mut self.ai_runs) {
            match self.run_end(&mut run, now) {
                Some(end) => ended.push((run, end)),
                None => self.ai_runs.push(run),
            }
        }
        let settings = self.config.settings.clone();
        let mut attention = false;
        // Desktop notifications, shown while GhostLane is in the background.
        let mut notices = Vec::new();
        let ended_ghosts: Vec<PathBuf> = ended.iter().map(|(run, _)| run.cli.ghost.clone()).collect();
        for (run, end) in ended {
            let ghost = run.cli.ghost.clone();
            let Some(p) = self
                .projects
                .iter_mut()
                .find_map(|op| op.project.as_mut().ok().filter(|p| p.ghost_path == ghost))
            else {
                continue;
            };
            let (id, seconds) = (format_id(run.id), elapsed(&run));
            let mut messages = Vec::new();
            if let RunEnd::CliGone = end {
                messages.push(format!("The AI CLI closed before it finished {id}"));
                notices.push(messages[0].clone());
                messages.extend(record_run(p, &run, seconds, RunResult::CliClosed).err());
                self.toasts.extend(messages.into_iter().map(|m| (m, Instant::now())));
                continue;
            }
            let _ = p.rescan();
            let Some((col, t)) = p.find_ticket(run.id) else {
                continue;
            };
            let path = t.path.clone();
            if let Some(to) = p.auto_target(col, AutoMove::AiComplete)
                && let Err(e) = p.move_ticket(&path, to)
            {
                messages.push(format!("Couldn't move {id}: {e}"));
            }
            let from = p.meta.columns.iter().position(|c| c.folder == run.from);
            let now_col = p.find_ticket(run.id).map(|(c, _)| c);
            let arrived = now_col == Some(p.last_col()) && from.is_some_and(|f| f < p.last_col());
            let command = self.config.project(&ghost).test_command;
            if arrived && !command.is_empty() {
                // The ticket only counts as done once the project's own test command passes.
                let (tx, ctx2, root, shell) = (
                    self.jobs_tx.clone(),
                    ctx.clone(),
                    p.root.clone(),
                    settings.shell.clone(),
                );
                let (job_ghost, job_id, script) = (ghost.clone(), run.id, command.clone());
                std::thread::spawn(move || {
                    let (passed, output) = terminal::run_script(shell.as_deref(), &script, &root);
                    let _ = tx.send(Job::Checked {
                        ghost: job_ghost,
                        id: job_id,
                        passed,
                        output,
                    });
                    ctx2.request_repaint();
                });
                self.toasts.extend(messages.into_iter().map(|m| (m, Instant::now())));
                self.checks.push(Check { run, seconds, command });
                continue;
            }
            let result = moved(p, &run);
            messages.extend(record_run(p, &run, seconds, result).err());
            if let Some((col, t)) = p.find_ticket(run.id) {
                let (title, column) = (t.title.replace(['\n', '\r'], " "), &p.meta.columns[col].name);
                notices.push(match result {
                    RunResult::SentBack => format!("{id} {title} came back to {column}"),
                    _ => format!("{id} {title} is in {column}"),
                });
            }
            // A ticket that ends up left of where it started came back from testing.
            if settings.notify_failed
                && result == RunResult::SentBack
                && let Some((col, _)) = p.find_ticket(run.id)
            {
                attention = true;
                messages.push(format!("{id} came back to {}", p.meta.columns[col].name));
            }
            self.toasts.extend(messages.into_iter().map(|m| (m, Instant::now())));
            if arrived {
                self.ticket_done(&ghost, run.id, ctx);
            }
        }
        // A project whose AI is free starts its next queued ticket, or with an empty queue, the next one
        // from its Auto columns. Not while a ticket is being written: that prompt is still in flight.
        if self.generating.is_none() {
            for i in 0..self.projects.len() {
                self.start_next(i, settings.auto_order, settings.auto_run_limit, ctx);
            }
        }
        // Out of work: a run just ended and nothing took its place.
        let idle = self.generating.is_none()
            && ended_ghosts.iter().any(|g| {
                !self.ai_runs.iter().any(|r| r.cli.ghost == *g) && !self.checks.iter().any(|c| c.run.cli.ghost == *g)
            });
        if attention || (idle && settings.notify_idle) {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                egui::UserAttentionType::Informational,
            ));
        }
        if idle && settings.notify_idle {
            notices.push("The AI has finished all its work".to_string());
        }
        for n in notices {
            self.desktop_notify(ctx, &n);
        }
    }

    /// Pops up a desktop notification, unless they're switched off or GhostLane is in front.
    fn desktop_notify(&self, ctx: &egui::Context, body: &str) {
        if self.config.settings.notify_desktop && !ctx.input(|i| i.focused) {
            terminal::notify("GhostLane", body);
        }
    }

    /// Starts project `i`'s next queued ticket, or with an empty queue, the next one from its Auto columns,
    /// unless its AI is busy or paused.
    fn start_next(&mut self, i: usize, order: crate::project::AutoOrder, run_limit: u32, ctx: &egui::Context) {
        if self.projects[i].paused || self.projects[i].project.is_err() || self.ai_running(i) {
            return;
        }
        // Queued tickets waiting for others stay queued until those are done.
        while let Some(pos) = self.next_queued(i) {
            let id = self.projects[i].queue.remove(pos);
            if self.run_ai(i, id, ctx) {
                return;
            }
        }
        let op = &self.projects[i];
        let Some((col, id)) = op
            .project
            .as_ref()
            .ok()
            .and_then(|p| p.next_auto(&op.auto, order, run_limit))
        else {
            return;
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

    /// Picks up work finished on background threads: test commands and commits.
    fn poll_jobs(&mut self, ctx: &egui::Context) {
        while let Ok(job) = self.jobs_rx.try_recv() {
            match job {
                Job::GitFailed(e) => self.toast(e),
                Job::Reverted { id, result: Err(e), .. } => {
                    self.toast(format!("Couldn't revert {}: {e}", format_id(id)))
                }
                Job::Reverted {
                    ghost,
                    id,
                    result: Ok(()),
                } => {
                    // Its work is undone, so it starts over.
                    let Some(p) = self.project_mut(&ghost) else { continue };
                    let _ = p.rescan();
                    let Some(mut t) = p.find_ticket(id).map(|(_, t)| t.clone()) else {
                        continue;
                    };
                    t.done_at = None;
                    if let Err(e) = t.save().and_then(|()| p.move_ticket(&t.path, 0)) {
                        self.toast(format!("Reverted {}, but couldn't move it back: {e}", t.id_str()));
                    }
                }
                Job::Checked {
                    ghost,
                    id,
                    passed,
                    output,
                } => {
                    // Gone when Stop was pressed meanwhile.
                    let Some(i) = self
                        .checks
                        .iter()
                        .position(|c| c.run.cli.ghost == ghost && c.run.id == id)
                    else {
                        continue;
                    };
                    let check = self.checks.remove(i);
                    self.check_done(check, passed, &output, ctx);
                    // The project is free again; the next frame starts its next ticket.
                    ctx.request_repaint();
                }
            }
        }
    }

    /// The test command finished for a ticket the AI moved to the last column. Passed: the ticket is done.
    /// Failed: it goes back to the first column with the output in its notes, like a failed test run.
    fn check_done(&mut self, check: Check, passed: bool, output: &str, ctx: &egui::Context) {
        let ghost = check.run.cli.ghost.clone();
        let Some(p) = self
            .projects
            .iter_mut()
            .find_map(|op| op.project.as_mut().ok().filter(|p| p.ghost_path == ghost))
        else {
            return;
        };
        let mut messages = Vec::new();
        if passed {
            messages.extend(record_run(p, &check.run, check.seconds, RunResult::MovedOn).err());
        } else {
            let _ = p.rescan();
            if let Some((_, t)) = p.find_ticket(check.run.id) {
                let mut t = t.clone();
                t.notes = with_failure_note(&t.notes, &check.command, output, crate::ticket::now_secs());
                let back = t.save().and_then(|()| p.move_ticket(&t.path, 0));
                messages.extend(back.err().map(|e| format!("Couldn't send {} back: {e}", t.id_str())));
            }
            messages.extend(record_run(p, &check.run, check.seconds, RunResult::TestsFailed).err());
            let first = p.meta.columns.first().map_or_else(String::new, |c| c.name.clone());
            messages.push(format!(
                "{} failed \"{}\" and went back to {first}",
                format_id(check.run.id),
                check.command
            ));
        }
        let notice = match passed {
            true => format!("{} passed \"{}\"", format_id(check.run.id), check.command),
            false => messages.last().cloned().unwrap_or_default(),
        };
        self.desktop_notify(ctx, &notice);
        for m in messages {
            self.toast(m);
        }
        if passed {
            self.ticket_done(&ghost, check.run.id, ctx);
        } else if self.config.settings.notify_failed {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                egui::UserAttentionType::Informational,
            ));
        }
    }

    /// Ticket `id` of project `ghost` reached the last column: notes when (archiving counts from then) and
    /// commits, then pushes, if the project asks for it.
    fn ticket_done(&mut self, ghost: &Path, id: u32, ctx: &egui::Context) {
        let settings = self.config.project(ghost);
        let Some(p) = self
            .projects
            .iter_mut()
            .find_map(|op| op.project.as_mut().ok().filter(|p| p.ghost_path == ghost))
        else {
            return;
        };
        let Some(mut t) = p.find_ticket(id).map(|(_, t)| t.clone()) else {
            return;
        };
        let root = p.root.clone();
        t.done_at = Some(crate::ticket::now_secs());
        if let Err(e) = t.save().and_then(|()| p.rescan()) {
            self.toast(format!("Couldn't update {}: {e}", t.id_str()));
        }
        if !settings.commit {
            return;
        }
        let message = format!("{}: {}", t.id_str(), t.title.replace(['\n', '\r'], " "));
        let (tx, ctx) = (self.jobs_tx.clone(), ctx.clone());
        // ponytail: commits in the background, so a ticket Auto starts next could slip edits in; it starts
        // seconds later, and the git commands take milliseconds.
        std::thread::spawn(move || {
            if let Err(e) = crate::git::commit(&root, &message, settings.push) {
                let _ = tx.send(Job::GitFailed(format!("Couldn't commit {}: {e}", format_id(id))));
                ctx.request_repaint();
            }
        });
    }

    /// Asks the AI CLI to write a new ticket into column `col` and waits for it.
    fn start_generation(&mut self, col: usize, request: &str, ctx: &egui::Context) -> Result<(), String> {
        let p = self.active_project().ok_or("No project is open")?;
        let col = col.min(p.meta.columns.len().saturating_sub(1));
        let id = crate::ticket::format_id(p.meta.next_ticket_id);
        let path = p.column_dir(col).join(format!("{id}.md"));
        let prompt = terminal::ticket_request_prompt(request, &path, &id);
        self.start_writing(path, &prompt, None, ctx)
    }

    /// Asks the AI CLI to change ticket `id` in place and waits for its file to change.
    fn start_update(&mut self, id: u32, request: &str, ctx: &egui::Context) -> Result<(), String> {
        let (_, ticket) = self.find_ticket(id).ok_or("This ticket no longer exists on disk")?;
        let path = ticket.path.clone();
        let prompt = terminal::ticket_update_prompt(request, &path, &crate::ticket::format_id(id));
        self.start_writing(path, &prompt, Some(id), ctx)
    }

    /// Sends `prompt` to the active project's AI CLI and waits for it to write the ticket at `path`.
    /// `update`: the ticket being changed in place, shown again once it lands.
    fn start_writing(
        &mut self,
        path: PathBuf,
        prompt: &str,
        update: Option<u32>,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let p = self.active_project().ok_or("No project is open")?;
        let cli = self.project_cli(p)?;
        let baseline = match update {
            Some(_) => Some(
                std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .map_err(|e| e.to_string())?,
            ),
            None => None,
        };
        let (key, root) = (p.ghost_path.clone(), p.root.clone());
        self.send_to_cli(&key, &root, cli, prompt, ctx)?;
        self.generating = Some(Generation {
            cli: CliWatch::new(key, cli.program),
            path,
            seen: None,
            baseline,
            reopen: update,
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
        self.send_to_cli(&key, &root, cli, &prompt, ctx).map(|_| ())
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
        } else if g.cli.started.elapsed() > TIMEOUT {
            Some("The AI didn't finish the ticket within 15 minutes.".to_string())
        } else if g.cli.gone(&self.terminals, now) {
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
                    self.open_ticket(id);
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
        let (mut toggle_pause, mut stop) = (false, false);
        panel.frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (glyph, tip) = if collapsed {
                    (icon::CARET_UP, "Show terminal")
                } else {
                    (icon::CARET_DOWN, "Hide terminal")
                };
                if icon_button(ui, glyph, tip).clicked() {
                    self.config.terminal_collapsed = !collapsed;
                    self.save_config();
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
                        self.save_config();
                    }
                    let Some(i) = self.active else { return };
                    let running = self.ai_running(i);
                    let op = &self.projects[i];
                    let ghost = op.project.as_ref().ok().map(|p| p.ghost_path.clone());
                    let checking = self.checks.iter().find(|c| Some(&c.run.cli.ghost) == ghost.as_ref());
                    if op.paused {
                        let resume = ui::glyph_button(ui, icon::PLAY, theme::ACCENT, theme::ACCENT, true)
                            .on_hover_text("Resume: start queued and Auto tickets again");
                        if resume.clicked() {
                            toggle_pause = true;
                        }
                        ui.label(RichText::new("Paused").color(theme::ACCENT));
                    } else if (running || !op.queue.is_empty() || !op.auto.is_empty())
                        && icon_button(
                            ui,
                            icon::PAUSE,
                            "Pause: let the AI finish this ticket, then start nothing new",
                        )
                        .clicked()
                    {
                        toggle_pause = true;
                    }
                    if running
                        && ui::danger_icon_button(ui, icon::STOP, "Stop the AI now and pause the queue").clicked()
                    {
                        stop = true;
                    }
                    if let Some(c) = checking {
                        let text = format!("Testing {}…", format_id(c.run.id));
                        ui.label(RichText::new(text).color(theme::TEXT_MUTED))
                            .on_hover_text(format!("Running \"{}\"", c.command));
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
        if let Some(i) = self.active {
            if stop {
                self.stop_ai(i);
            } else if toggle_pause {
                self.projects[i].paused = !self.projects[i].paused;
            }
        }
    }

    fn terminal_view(&mut self, ui: &mut Ui, key: &Path, cwd: &Path) {
        let shell = self.config.settings.shell.clone();
        // A project's terminal opens with its AI CLI running, ready to use (worked out only when it opens:
        // that writes the completion hook's settings).
        let startup = match self.active_project() {
            Some(p) if !self.terminals.contains(key) => {
                self.project_cli(p).ok().map(|cli| self.cli_launch(key, cli, None).0)
            }
            _ => None,
        };
        match self
            .terminals
            .get_or_start(key, cwd, shell.as_deref(), startup.as_deref(), ui.ctx())
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
        let commands = match self.modal {
            Some(Modal::QuickOpen(_)) => self.commands(),
            _ => Vec::new(),
        };
        match &mut self.modal {
            None => {}
            Some(Modal::Project(form)) => {
                if let Some(outcome) = form.show(ctx) {
                    self.project_outcome(outcome, ctx);
                }
            }
            Some(Modal::ViewTicket(view)) => {
                let id = view.id;
                let (Some(p), Some((col, t))) = (self.active_project(), self.find_ticket(id)) else {
                    self.modal = None;
                    return;
                };
                let (t, last, root) = (t.clone(), p.last_col(), p.root.clone());
                let (col_name, last_name) = (p.meta.columns[col].name.clone(), p.meta.columns[last].name.clone());
                let links = self.links(&t);
                let Some(Modal::ViewTicket(view)) = &mut self.modal else {
                    return;
                };
                if view.wants_changes() {
                    view.changes = Some(load_changes(&root, view.commit.as_deref(), &t));
                }
                match ticket_ui::view(
                    ctx,
                    view,
                    &t,
                    &links,
                    (&col_name, col == last, &last_name),
                    &mut self.md_cache,
                ) {
                    Some(ViewOutcome::Close) => self.modal = None,
                    Some(ViewOutcome::Edit) => self.modal = Some(self.ticket_form(TicketForm::edit(&t))),
                    Some(ViewOutcome::Delete) => {
                        self.modal = None;
                        self.delete_tickets(&[t]);
                    }
                    Some(ViewOutcome::Approve) => {
                        self.modal = None;
                        self.apply(
                            Action::MoveTickets {
                                ids: vec![id],
                                to_col: last,
                            },
                            ctx,
                        );
                    }
                    Some(ViewOutcome::Reject(note)) => {
                        self.modal = None;
                        self.reject(id, &note);
                    }
                    Some(ViewOutcome::Open(other)) => self.open_ticket(other),
                    Some(ViewOutcome::FilterByLabel(label)) => {
                        self.modal = None;
                        self.apply(Action::FilterByLabel(label), ctx);
                    }
                    Some(ViewOutcome::Revert(commit)) => {
                        self.modal = None;
                        self.revert(id, commit, ctx);
                    }
                    None => {}
                }
            }
            Some(Modal::QuickOpen(q)) => {
                let Some(p) = self.active.and_then(|i| self.projects[i].project.as_ref().ok()) else {
                    self.modal = None;
                    return;
                };
                match q.show(ctx, p, &commands) {
                    Some(QuickOutcome::Open(id)) => self.open_ticket(id),
                    Some(QuickOutcome::Run(action)) => {
                        self.modal = None;
                        self.apply(action, ctx);
                    }
                    Some(QuickOutcome::Close) => self.modal = None,
                    None => {}
                }
            }
            Some(Modal::Stats) => {
                let Some(p) = self.active_project() else {
                    self.modal = None;
                    return;
                };
                let now = crate::ticket::now_secs();
                if ui::stats::show(ctx, &p.meta.name, &p.stats(now), now / 86_400) {
                    self.modal = None;
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
            Some(Modal::ConfirmDelete(ids)) => {
                let ids = ids.clone();
                let tickets: Vec<Ticket> = ids
                    .iter()
                    .filter_map(|&id| self.find_ticket(id).map(|(_, t)| t.clone()))
                    .collect();
                let msg = match tickets.as_slice() {
                    [] => {
                        self.modal = None;
                        return;
                    }
                    [t] => format!("Delete ticket {} – {}?", t.id_str(), t.title),
                    many => format!("Delete {} tickets?", many.len()),
                };
                match ticket_ui::confirm(ctx, "delete", &msg, "Cancel", "Delete") {
                    Some(true) => {
                        self.modal = None;
                        self.delete_tickets(&tickets);
                    }
                    Some(false) => self.modal = None,
                    None => {}
                }
            }
        }
    }

    /// The tickets `t` is blocked by and those it blocks, with their titles and columns.
    fn links(&self, t: &Ticket) -> ticket_ui::Links {
        let Some(p) = self.active_project() else {
            return ticket_ui::Links::default();
        };
        let dep = |id: u32| match p.find_ticket(id) {
            Some((c, dep)) => ticket_ui::Dependency {
                id,
                title: dep.title.clone(),
                column: p.meta.columns[c].name.clone(),
                priority: Some(dep.priority),
                waiting: Some(c != p.last_col()),
            },
            None => ticket_ui::Dependency {
                id,
                title: "(missing)".into(),
                column: String::new(),
                priority: None,
                waiting: None,
            },
        };
        ticket_ui::Links {
            blocked_by: t.blocked_by.iter().map(|&b| dep(b)).collect(),
            blocks: p.dependents(t.id).into_iter().map(dep).collect(),
            filtered: self
                .active
                .map(|i| self.projects[i].filter.labels.clone())
                .unwrap_or_default(),
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
            (Ok(id), FormMode::Edit) => self.open_ticket(id),
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
            // The dialog stays open.
            ProjectOutcome::Export(format) => self.export(format),
            ProjectOutcome::Delete { files } => {
                self.modal = None;
                self.delete_project(files, ctx);
            }
            ProjectOutcome::ShowFolder(dir) => {
                if let Err(e) = terminal::open_folder(&dir) {
                    self.toast(format!("Couldn't open {}: {e}", dir.display()));
                }
            }
            ProjectOutcome::OpenExisting(path) => {
                self.modal = None;
                self.open_path(path, ctx);
            }
            ProjectOutcome::Create {
                dir,
                name,
                columns,
                ai_cli,
                settings,
                git_excluded,
            } => match Project::create(&dir, &name, &columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>()) {
                Ok(mut p) => {
                    self.modal = None;
                    p.meta.ai_cli = Some(ai_cli);
                    for (c, edit) in p.meta.columns.iter_mut().zip(columns) {
                        c.auto_move = edit.auto_move;
                        c.ai_prompt = edit.ai_prompt;
                        c.wip_limit = edit.wip_limit.filter(|&n| n > 0);
                    }
                    if let Err(e) = p.save_meta() {
                        self.toast(format!("Couldn't save the project's settings: {e}"));
                    }
                    if let Err(e) = write_ai_guide(&p) {
                        self.toast(e);
                    }
                    if let Err(e) = p.set_git_excluded(git_excluded) {
                        self.toast(format!("Couldn't update .gitignore: {e}"));
                    }
                    self.config.projects.insert(p.ghost_path.clone(), settings);
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
                settings,
                git_excluded,
            } => {
                self.modal = None;
                if let Some(Err(e)) = self.active_project().map(|p| p.set_git_excluded(git_excluded)) {
                    self.toast(format!("Couldn't update .gitignore: {e}"));
                }
                if let Some(ghost) = self.active_project().map(|p| p.ghost_path.clone()) {
                    self.config.projects.insert(ghost, settings);
                    self.save_config();
                }
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
            self.cli_running = program.is_some_and(|program| self.terminals.runs(&key, program));
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
                    if self.active_project().is_some() {
                        if icon_button(ui, icon::UPLOAD_SIMPLE, "Generate tickets from a document").clicked() {
                            actions.push(Action::ImportDocument);
                        }
                        let find = format!("Find a ticket or run a command ({})", ui.ctx().format_shortcut(&FIND));
                        if icon_button(ui, icon::MAGNIFYING_GLASS, &find).clicked() {
                            actions.push(Action::QuickOpen);
                        }
                        let filtering = self.active.is_some_and(|i| self.projects[i].filter_open);
                        let idle = if filtering { theme::ACCENT } else { theme::TEXT };
                        if ui::glyph_button(ui, icon::FUNNEL, idle, theme::ACCENT, false)
                            .on_hover_text("Filter the board")
                            .clicked()
                        {
                            actions.push(Action::ToggleFilter);
                        }
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
                        if self.active_project().is_some()
                            && icon_button(ui, icon::CHART_BAR, "Project stats").clicked()
                        {
                            actions.push(Action::ShowStats);
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

/// How long `run` has been going, in seconds.
fn elapsed(run: &AiRun) -> u64 {
    run.started_at.elapsed().map_or(0, |d| d.as_secs())
}

/// Adds `run` to its ticket's run history, with the column the ticket is in now.
fn record_run(p: &mut Project, run: &AiRun, seconds: u64, result: RunResult) -> Result<(), String> {
    p.rescan().map_err(|e| format!("Couldn't reload project: {e}"))?;
    let Some((col, t)) = p.find_ticket(run.id) else {
        return Ok(());
    };
    let mut t = t.clone();
    let from = p
        .meta
        .columns
        .iter()
        .find(|c| c.folder == run.from)
        .map_or_else(|| run.from.clone(), |c| c.name.clone());
    let started = run
        .started_at
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    t.record_run(RunRecord {
        started: crate::ticket::utc_timestamp(started),
        seconds,
        from,
        to: p.meta.columns[col].name.clone(),
        result,
        base: run.base.clone(),
        tokens: run
            .marker
            .as_deref()
            .and_then(|m| terminal::run_tokens(m, run.started_at)),
    });
    t.save()
        .map_err(|e| format!("Couldn't save the run history of {}: {e}", t.id_str()))?;
    p.rescan().map_err(|e| format!("Couldn't reload project: {e}"))
}

/// Where a finished run left its ticket. Staying in the column the play button moved it to counts as not moving.
fn moved(p: &Project, run: &AiRun) -> RunResult {
    let Some(from) = p.meta.columns.iter().position(|c| c.folder == run.from) else {
        return RunResult::Unmoved;
    };
    let working = p.auto_target(from, AutoMove::AiWorking);
    match p.find_ticket(run.id).map(|(c, _)| c) {
        Some(c) if c == from || Some(c) == working => RunResult::Unmoved,
        Some(c) if c > from => RunResult::MovedOn,
        Some(_) => RunResult::SentBack,
        None => RunResult::Unmoved,
    }
}

/// `notes` with a note saying `command` failed at `when`, followed by its output.
fn with_failure_note(notes: &str, command: &str, output: &str, when: u64) -> String {
    // A fence longer than any run of backticks in the output, so the output can't close it.
    let longest = output.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    let when = crate::ticket::readable_utc(&crate::ticket::utc_timestamp(when));
    let note = format!(
        "Test command `{command}` failed on {when}:\n\n{fence}\n{}\n{fence}",
        output.trim_end()
    );
    append_note(notes, &note)
}

/// Puts back one card move or delete. Steps for projects that have been closed since are skipped.
fn undo_step(projects: &mut [OpenProject], step: UndoStep) -> Result<(), String> {
    let ghost = match &step {
        UndoStep::Move { ghost, .. } | UndoStep::Delete { ghost, .. } => ghost.clone(),
    };
    let Some(p) = projects
        .iter_mut()
        .find_map(|op| op.project.as_mut().ok().filter(|p| p.ghost_path == ghost))
    else {
        return Ok(());
    };
    match step {
        UndoStep::Move { id, folder, .. } => {
            let col = p.meta.columns.iter().position(|c| c.folder == folder);
            let result = match (col, p.find_ticket(id).map(|(_, t)| t.path.clone())) {
                (Some(col), Some(path)) => p.move_ticket(&path, col).map_err(|e| e.to_string()),
                (None, _) => Err("its column is gone".to_string()),
                (_, None) => Err("it's gone".to_string()),
            };
            result.map_err(|e| format!("Couldn't move {} back: {e}", format_id(id)))
        }
        UndoStep::Delete { path, contents, .. } => {
            let result = if path.exists() {
                Err(std::io::Error::other("a file with its name is back already"))
            } else {
                crate::ticket::write_atomic(&path, &contents).and_then(|()| p.rescan())
            };
            result.map_err(|e| format!("Couldn't restore {}: {e}", path.display()))
        }
    }
}

/// `notes` with `note` added as a paragraph of its own.
fn append_note(notes: &str, note: &str) -> String {
    match notes.trim_end() {
        "" => note.to_string(),
        notes => format!("{notes}\n\n{note}"),
    }
}

/// What ticket `t` changed: its commit when it has one, otherwise everything since its latest run started.
fn load_changes(root: &Path, commit: Option<&str>, t: &Ticket) -> Result<crate::git::Changes, String> {
    match (commit, t.base()) {
        (Some(c), _) => crate::git::commit_changes(root, c),
        (None, Some(base)) => crate::git::changes_since(root, base),
        (None, None) => Err(
            "No changes recorded yet. They show here once the AI has worked on this ticket in a git repository.".into(),
        ),
    }
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
        self.poll_jobs(&ctx);
        if let Some(v) = self.update_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.update = Some(v);
        }
        let handed: Vec<PathBuf> = self
            .handed_rx
            .as_ref()
            .map_or_else(Vec::new, |rx| rx.try_iter().collect());
        for path in handed {
            self.open_path(path, &ctx);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        let mut actions = Vec::new();
        self.shortcuts(&ctx, &mut actions);

        let inset = ui::chrome::maximized_overflow(&ctx);
        if inset > 0.0 {
            for panel in [
                egui::Panel::top("max_inset_top"),
                egui::Panel::bottom("max_inset_bottom"),
                egui::Panel::left("max_inset_left"),
                egui::Panel::right("max_inset_right"),
            ] {
                panel
                    .exact_size(inset)
                    .resizable(false)
                    .frame(Frame::new().fill(theme::PANEL))
                    .show_separator_line(false)
                    .show(ui, |_| {});
            }
        }
        let busy = self.ai_busy();
        self.top_bar(ui, &mut actions, busy);
        self.terminal_panel(ui);

        let central = Frame::new().fill(theme::BG).inner_margin(Margin::same(12));
        let board = egui::CentralPanel::default()
            .frame(central)
            .show(ui, |ui| match self.active {
                Some(i) if self.projects[i].project.is_ok() => {
                    let settings = ui::board::BoardSettings {
                        engaged: self.ai_engaged(i),
                        run_limit: self.config.settings.auto_run_limit,
                        archive_days: self.config.settings.archive_after_days,
                        keys: self.keyboard_free(&ctx),
                    };
                    ui::board::show(ui, &mut self.projects[i], settings, &mut actions)
                }
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
        Generation {
            cli: CliWatch::new(PathBuf::new(), "claude"),
            path,
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
    fn failure_notes_append_the_output_in_a_fence_it_cant_close() {
        let note = with_failure_note("", "cargo test", "boom\n", 1_709_210_096);
        assert_eq!(
            note,
            "Test command `cargo test` failed on 2024-02-29 12:34 UTC:\n\n```\nboom\n```"
        );
        let note = with_failure_note("Earlier.\n", "t", "a ```` b", 0);
        assert!(note.starts_with("Earlier.\n\nTest command"));
        assert!(note.contains("\n`````\na ```` b\n`````"), "{note}");
    }

    #[test]
    fn runs_are_judged_by_where_the_ticket_ended_up() {
        let d = tempfile::tempdir().unwrap();
        let names: Vec<String> = crate::project::DEFAULT_COLUMNS.iter().map(|c| c.to_string()).collect();
        let mut p = Project::create(d.path(), "P", &names).unwrap();
        for (c, m) in p.meta.columns.iter_mut().zip(crate::project::DEFAULT_AUTO_MOVES) {
            c.auto_move = m;
        }
        let t = p.add_ticket(0, Ticket::default()).unwrap();
        let run = AiRun {
            cli: CliWatch::new(p.ghost_path.clone(), "claude"),
            id: t.id,
            from: "to_do".into(),
            started_at: std::time::SystemTime::now(),
            marker: None,
            base: None,
        };
        p.move_ticket(&t.path, 1).unwrap(); // the play button moved it to In Dev
        assert_eq!(moved(&p, &run), RunResult::Unmoved);
        let at = |p: &Project| p.find_ticket(t.id).unwrap().1.path.clone();
        p.move_ticket(&at(&p), 2).unwrap();
        assert_eq!(moved(&p, &run), RunResult::MovedOn);
        record_run(&mut p, &run, 252, RunResult::MovedOn).unwrap();
        let back = p.find_ticket(t.id).unwrap().1;
        assert_eq!(back.runs.len(), 1);
        assert_eq!(
            (back.runs[0].from.as_str(), back.runs[0].to.as_str()),
            ("To Do", "Ready For Test")
        );
        assert_eq!(back.runs[0].seconds, 252);

        let test_run = AiRun {
            from: "ready_for_test".into(),
            ..run
        };
        p.move_ticket(&at(&p), 0).unwrap();
        assert_eq!(moved(&p, &test_run), RunResult::SentBack);
    }

    #[test]
    fn undo_puts_moved_and_deleted_tickets_back() {
        let d = tempfile::tempdir().unwrap();
        let names: Vec<String> = crate::project::DEFAULT_COLUMNS.iter().map(|c| c.to_string()).collect();
        let mut p = Project::create(d.path(), "P", &names).unwrap();
        let t = p.add_ticket(0, Ticket::default()).unwrap();
        let ghost = p.ghost_path.clone();
        p.move_ticket(&t.path, 2).unwrap();
        let mut projects = vec![OpenProject::new(ghost.clone(), Ok(p))];
        let col = |projects: &[OpenProject]| projects[0].project.as_ref().unwrap().find_ticket(t.id).map(|(c, _)| c);
        let back = UndoStep::Move {
            ghost: ghost.clone(),
            id: t.id,
            folder: "to_do".into(),
        };
        undo_step(&mut projects, back).unwrap();
        assert_eq!(col(&projects), Some(0));

        let contents = std::fs::read_to_string(&t.path).unwrap();
        std::fs::remove_file(&t.path).unwrap();
        projects[0].project.as_mut().unwrap().rescan().unwrap();
        assert_eq!(col(&projects), None);
        let restore = || UndoStep::Delete {
            ghost: ghost.clone(),
            path: t.path.clone(),
            contents: contents.clone(),
        };
        undo_step(&mut projects, restore()).unwrap();
        assert_eq!(col(&projects), Some(0));
        // Never overwrites a file that is there again.
        assert!(
            undo_step(&mut projects, restore())
                .unwrap_err()
                .contains("back already")
        );
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
