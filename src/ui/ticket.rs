use egui::text::{LayoutJob, TextFormat};
use egui::{Align, ComboBox, DragValue, Layout, RichText, ScrollArea, TextEdit, Ui, vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use egui_phosphor::regular as icon;

use super::rich_editor::RichEditor;
use super::{accent_button, icon_button, priority_icon};
use crate::theme;
use crate::ticket::{Priority, RunRecord, Ticket, format_id, labels_from_text};

fn modal_size(ctx: &egui::Context) -> egui::Vec2 {
    let screen = ctx.content_rect().size();
    vec2((screen.x * 0.8).min(1400.0), screen.y * 0.8)
}

fn markdown(ui: &mut Ui, cache: &mut CommonMarkCache, text: &str) {
    if text.trim().is_empty() {
        ui.label(RichText::new("Nothing here yet.").italics().color(theme::TEXT_MUTED));
    } else {
        // Rendered markdown keeps egui's default spacing; the app's taller rows misalign list bullets.
        ui.scope(|ui| {
            *ui.spacing_mut() = egui::Spacing::default();
            CommonMarkViewer::new().show(ui, cache, text);
        });
    }
}

pub enum ViewOutcome {
    Close,
    Edit,
    /// Move the ticket to the last column.
    Approve,
    /// Send the ticket back to the first column with this note.
    Reject(String),
    /// Revert this commit of the ticket.
    Revert(String),
    /// Show this other ticket instead.
    Open(u32),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ViewTab {
    Details,
    Changes,
}

/// An open ticket view.
pub struct TicketView {
    pub id: u32,
    tab: ViewTab,
    /// The commit made when the ticket was done, if there is one.
    pub commit: Option<String>,
    /// What the ticket changed, loaded by the app when the Changes tab first shows. Err: why there's nothing.
    pub changes: Option<Result<crate::git::Changes, String>>,
    reject_note: String,
    confirm_revert: bool,
}

impl TicketView {
    pub fn new(id: u32, commit: Option<String>) -> Self {
        TicketView {
            id,
            tab: ViewTab::Details,
            commit,
            changes: None,
            reject_note: String::new(),
            confirm_revert: false,
        }
    }

    /// The Changes tab is showing but nothing is loaded yet.
    pub fn wants_changes(&self) -> bool {
        self.tab == ViewTab::Changes && self.changes.is_none()
    }
}

/// A ticket linked to the one shown, through `blocked_by`.
pub struct Dependency {
    pub id: u32,
    /// Number, title and column.
    pub label: String,
    /// Not in the last column yet. `None`: the ticket is missing.
    pub waiting: Option<bool>,
}

/// The tickets the shown one waits for, and those waiting for it.
#[derive(Default)]
pub struct Links {
    pub blocked_by: Vec<Dependency>,
    pub blocks: Vec<Dependency>,
}

/// `done`: the ticket is in the last column (`last` is that column's name).
pub fn view(
    ctx: &egui::Context,
    state: &mut TicketView,
    t: &Ticket,
    links: &Links,
    (done, last): (bool, &str),
    cache: &mut CommonMarkCache,
) -> Option<ViewOutcome> {
    let mut outcome = None;
    let size = modal_size(ctx);
    let modal = super::show_modal(ctx, "view_ticket", |ui| {
        ui.set_width(size.x);
        ui.set_height(size.y);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(t.id_str())
                    .monospace()
                    .size(16.0)
                    .color(theme::TEXT_MUTED),
            );
            ui.label(priority_icon(t.priority).size(16.0));
            ui.label(RichText::new(t.priority.label()).color(theme::TEXT_MUTED));
            ui.label(RichText::new(format!("·  Runs: {}", t.run_count)).color(theme::TEXT_MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, icon::X, "Close").clicked() {
                    outcome = Some(ViewOutcome::Close);
                }
                if icon_button(ui, icon::PENCIL_SIMPLE, "Edit ticket").clicked() {
                    outcome = Some(ViewOutcome::Edit);
                }
            });
        });
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(&t.title).size(24.0).strong()).wrap());
        if !t.labels.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                for l in &t.labels {
                    super::label_chip(ui, l, None);
                }
            });
        }
        let waiting = links.blocked_by.iter().any(|d| d.waiting == Some(true));
        let lock = if waiting {
            (icon::LOCK, theme::DANGER)
        } else {
            (icon::LOCK_OPEN, theme::TEXT_MUTED)
        };
        for (deps, (glyph, color), text) in [
            (&links.blocked_by, lock, "Blocked by"),
            (
                &links.blocks,
                (icon::ARROW_BEND_DOWN_RIGHT, theme::TEXT_MUTED),
                "Blocks",
            ),
        ] {
            if deps.is_empty() {
                continue;
            }
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{glyph}  {text}")).color(color));
                for d in deps {
                    let color = if d.waiting == Some(true) {
                        theme::TEXT
                    } else {
                        theme::TEXT_MUTED
                    };
                    if d.waiting.is_none() {
                        ui.label(RichText::new(&d.label).color(color));
                        continue;
                    }
                    let link = egui::Label::new(RichText::new(&d.label).color(color).underline());
                    if super::pointer(ui.add(link.sense(egui::Sense::click()))).clicked() {
                        outcome = Some(ViewOutcome::Open(d.id));
                    }
                }
            });
        }
        let before = state.tab;
        tabs(
            ui,
            &mut state.tab,
            &[(ViewTab::Details, "Details"), (ViewTab::Changes, "Changes")],
        );
        if state.tab != before {
            // The app loads the changes on the next frame.
            ui.ctx().request_repaint();
        }
        if state.tab == ViewTab::Changes {
            changes_tab(ui, state, done, last, &mut outcome);
            return;
        }
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            // Description always shows; the optional sections only when they have text.
            let sections = [("Test Scope", &t.test_scope), ("Notes", &t.notes)];
            let shown = sections.into_iter().filter(|(_, text)| !text.trim().is_empty());
            for (i, (heading, text)) in std::iter::once(("Description", &t.description))
                .chain(shown)
                .enumerate()
            {
                if i > 0 {
                    ui.add_space(16.0);
                }
                ui.label(RichText::new(heading).size(17.0).strong().color(theme::ACCENT));
                ui.add_space(4.0);
                markdown(ui, cache, text);
            }
            if !t.runs.is_empty() || state.commit.is_some() {
                ui.add_space(16.0);
                ui.label(RichText::new("Run History").size(17.0).strong().color(theme::ACCENT));
                ui.add_space(4.0);
                if let Some(commit) = &state.commit {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::GIT_COMMIT).color(theme::ACCENT));
                        ui.label("Committed as");
                        ui.label(RichText::new(commit).monospace());
                        ui.label(RichText::new("· review or revert it on the Changes tab").color(theme::TEXT_MUTED));
                    });
                }
                run_history(ui, &t.runs);
            }
        });
    });
    if state.confirm_revert {
        let commit = state.commit.clone().unwrap_or_default();
        let msg = format!(
            "Revert commit {commit}? A new commit undoes its changes; the ticket goes back to the first column."
        );
        match confirm(ctx, "revert", &msg, "Cancel", "Revert") {
            Some(true) => outcome = Some(ViewOutcome::Revert(commit)),
            Some(false) => state.confirm_revert = false,
            None => {}
        }
    } else if modal.should_close() && outcome.is_none() {
        outcome = Some(ViewOutcome::Close);
    }
    outcome
}

/// What the ticket changed, with buttons to pass it, send it back or undo its commit.
fn changes_tab(ui: &mut Ui, state: &mut TicketView, done: bool, last: &str, outcome: &mut Option<ViewOutcome>) {
    let heading = match &state.changes {
        Some(Ok(c)) => match &c.commit {
            Some(commit) => format!("{}  Commit {commit}", icon::GIT_COMMIT),
            None => format!(
                "{}  Changed since the last run started (not committed yet)",
                icon::GIT_DIFF
            ),
        },
        _ => format!("{}  Changes", icon::GIT_DIFF),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(heading).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if !done {
                let approve = accent_button(ui, &format!("{}  Approve", icon::CHECK))
                    .on_hover_text(format!("Move the ticket to {last}"));
                if approve.clicked() {
                    *outcome = Some(ViewOutcome::Approve);
                }
            } else if state.commit.is_some()
                && ui
                    .button(RichText::new(format!("{}  Revert Commit", icon::ARROW_U_UP_LEFT)).color(theme::DANGER))
                    .on_hover_text("Undo this ticket's changes with a new commit")
                    .clicked()
            {
                state.confirm_revert = true;
            }
        });
    });
    if !done {
        ui.horizontal(|ui| {
            let note = state.reject_note.trim().to_string();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let reject = egui::Button::new(RichText::new(format!("{}  Reject", icon::ARROW_BEND_UP_LEFT)));
                if ui
                    .add_enabled(!note.is_empty(), reject)
                    .on_hover_text("Add the note to the ticket and send it back to the first column")
                    .clicked()
                {
                    *outcome = Some(ViewOutcome::Reject(note));
                }
                ui.add(
                    TextEdit::singleline(&mut state.reject_note)
                        .margin(super::INPUT_MARGIN)
                        .hint_text("What needs fixing? Sent back with the ticket when you reject it")
                        .desired_width(f32::INFINITY),
                );
            });
        });
    }
    ui.separator();
    match &state.changes {
        None => {
            ui.label(RichText::new("Loading…").color(theme::TEXT_MUTED));
        }
        Some(Err(e)) => {
            ui.label(RichText::new(e).color(theme::TEXT_MUTED));
        }
        Some(Ok(c)) if c.diff.trim().is_empty() => {
            ui.label(RichText::new("Nothing changed outside the board.").color(theme::TEXT_MUTED));
        }
        Some(Ok(c)) => diff_view(ui, &c.diff),
    }
}

/// A unified diff, coloured by line. Only the rows on screen are laid out, so big diffs stay quick.
fn diff_view(ui: &mut Ui, diff: &str) {
    let lines: Vec<&str> = diff.lines().collect();
    let font = egui::FontId::monospace(13.0);
    let row = ui.fonts_mut(|f| f.row_height(&font));
    ui.spacing_mut().item_spacing.y = 0.0;
    ScrollArea::both()
        .auto_shrink(false)
        .show_rows(ui, row, lines.len(), |ui, range| {
            for line in &lines[range] {
                let color = match line.as_bytes().first() {
                    _ if line.starts_with("+++") || line.starts_with("---") => theme::TEXT,
                    Some(b'+') => theme::ACCENT,
                    Some(b'-') => theme::DANGER,
                    Some(b'@') => theme::TEXT_MUTED,
                    Some(b' ') => theme::TEXT,
                    _ if line.starts_with("diff ") => theme::TEXT,
                    _ => theme::TEXT_MUTED,
                };
                let text = RichText::new(*line).font(font.clone()).color(color);
                ui.add(egui::Label::new(text).extend());
            }
        });
}

/// Newest run first: when, how long, where the ticket went and how it ended.
fn run_history(ui: &mut Ui, runs: &[RunRecord]) {
    egui::Grid::new("run_history")
        .num_columns(5)
        .spacing([24.0, 4.0])
        .show(ui, |ui| {
            for r in runs.iter().rev() {
                let when = crate::ticket::readable_utc(&r.started);
                ui.label(RichText::new(when).monospace().color(theme::TEXT_MUTED));
                ui.label(duration(r.seconds));
                ui.label(format!("{} {} {}", r.from, icon::ARROW_RIGHT, r.to));
                let color = if r.result.failed() {
                    theme::DANGER
                } else {
                    theme::ACCENT
                };
                ui.label(RichText::new(r.result.label()).color(color));
                match r.tokens {
                    Some(t) => {
                        let text = format!("{} tokens", crate::ticket::compact(t.total()));
                        ui.label(RichText::new(text).color(theme::TEXT_MUTED))
                            .on_hover_text(format!(
                                "{} input, {} from cache, {} output",
                                crate::ticket::compact(t.input),
                                crate::ticket::compact(t.cached),
                                crate::ticket::compact(t.output)
                            ));
                    }
                    None => {
                        ui.label("");
                    }
                }
                ui.end_row();
            }
        });
}

/// `45s`, `4m 12s`, `1h 03m`.
pub fn duration(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, secs % 3600 / 60),
    }
}

/// "12, 00015" -> [12, 15]. Separated by commas or spaces; a ticket can't wait for itself.
fn parse_blocked(text: &str, own: u32) -> Result<Vec<u32>, String> {
    let mut ids = Vec::new();
    for part in text.split([',', ' ']).map(str::trim).filter(|p| !p.is_empty()) {
        let id: u32 = part.parse().map_err(|_| format!("\"{part}\" isn't a ticket number"))?;
        if id == own {
            return Err("A ticket can't be blocked by itself".into());
        }
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

/// (id, title) of the `tickets` matching `query` by number or title words, leaving out `own` and those in `taken`.
fn blocker_options<'a>(tickets: &'a [(u32, String)], own: u32, query: &str, taken: &[u32]) -> Vec<&'a (u32, String)> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    tickets
        .iter()
        .filter(|(id, _)| *id != own && !taken.contains(id))
        .filter(|(id, title)| {
            let text = format!("{} {id} {}", format_id(*id), title.to_lowercase());
            words.iter().all(|w| text.contains(w))
        })
        .collect()
}

/// The tickets matching what is being typed after the last comma of `blocked`; clicking one puts its number there.
/// True when one was picked.
fn blocker_list(ui: &mut Ui, own: u32, tickets: &[(u32, String)], blocked: &mut String) -> bool {
    ui.set_width(ui.available_width());
    let (head, typing) = match blocked.rfind(',') {
        Some(i) => blocked.split_at(i + 1),
        None => ("", blocked.as_str()),
    };
    // A number written out in full, as the list writes them, is a pick rather than a search.
    let written = typing.trim().parse().is_ok_and(|n: u32| format_id(n) == typing.trim());
    let (head, typing) = if written {
        (blocked.as_str(), "")
    } else {
        (head, typing)
    };
    let taken = parse_blocked(head, own).unwrap_or_default();
    let options = blocker_options(tickets, own, typing.trim(), &taken);
    let note = match (typing.trim(), options.is_empty()) {
        ("", true) => "No other tickets to wait for.".to_string(),
        (typed, true) => format!("No tickets match \"{typed}\"."),
        ("", false) => "Pick a ticket this one waits for, or type to search by number or title.".to_string(),
        (typed, false) => format!("Tickets matching \"{typed}\""),
    };
    ui.label(RichText::new(note).small().color(theme::TEXT_MUTED));
    let mut picked = None;
    ScrollArea::vertical()
        .id_salt("blocker_list")
        .max_height(180.0)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (id, title) in options {
                let row = egui::Button::new(format!("{}  {}", format_id(*id), title.replace(['\n', '\r'], " ")))
                    .frame(false)
                    .truncate();
                if ui.add_sized(vec2(ui.available_width(), 28.0), row).clicked() {
                    picked = Some(*id);
                }
            }
        });
    let Some(id) = picked else { return false };
    *blocked = match head.trim_end().trim_end_matches(',') {
        "" => format!("{}, ", format_id(id)),
        head => format!("{head}, {}, ", format_id(id)),
    };
    true
}

fn blocked_text(ids: &[u32]) -> String {
    ids.iter().map(|&id| format_id(id)).collect::<Vec<_>>().join(", ")
}

#[derive(Clone, Copy, PartialEq)]
pub enum FormMode {
    Edit,
    Create { col: usize },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tab {
    /// Describe the ticket and let the project's AI CLI write it.
    Ai,
    Manual,
}

pub struct TicketForm {
    pub mode: FormMode,
    tab: Tab,
    /// What the user asks the AI for on the AI Assisted tab.
    request: String,
    pub ticket: Ticket,
    original: Ticket,
    description: RichEditor,
    test_scope: RichEditor,
    scope_open: bool,
    notes: RichEditor,
    notes_open: bool,
    /// Ticket numbers this one waits for, as typed.
    blocked: String,
    /// The list of tickets to pick for Blocked by shows below the fields.
    blocked_open: bool,
    /// Where that list was drawn last frame; it stays open while the pointer is on it.
    blocked_list: Option<egui::Rect>,
    /// Give the Blocked by field the keyboard back after a ticket was picked from the list.
    blocked_refocus: bool,
    /// (id, title) of the project's tickets, offered in the Blocked by dropdown.
    pub tickets: Vec<(u32, String)>,
    /// Comma-separated labels, as typed.
    labels: String,
    confirm_discard: bool,
    pub error: Option<String>,
}

pub enum FormOutcome {
    Close,
    Save,
    /// Ask the AI CLI to write the ticket from this request.
    Generate(String),
}

impl TicketForm {
    pub fn edit(t: &Ticket) -> Self {
        Self::new(FormMode::Edit, t.clone())
    }

    /// `ticket.id` should already be the ID the new ticket will get.
    pub fn create(col: usize, ticket: Ticket) -> Self {
        Self::new(FormMode::Create { col }, ticket)
    }

    fn new(mode: FormMode, ticket: Ticket) -> Self {
        // Open on AI Assisted, except duplicates, which arrive pre-filled for manual tweaks.
        let duplicate = matches!(mode, FormMode::Create { .. }) && !ticket.title.is_empty();
        let tab = if duplicate { Tab::Manual } else { Tab::Ai };
        TicketForm {
            mode,
            tab,
            request: String::new(),
            description: RichEditor::new(("description", ticket.id), &ticket.description),
            test_scope: RichEditor::new(("test_scope", ticket.id), &ticket.test_scope),
            scope_open: false,
            notes: RichEditor::new(("notes", ticket.id), &ticket.notes),
            notes_open: false,
            blocked: blocked_text(&ticket.blocked_by),
            blocked_open: false,
            blocked_list: None,
            blocked_refocus: false,
            tickets: Vec::new(),
            labels: ticket.labels.join(", "),
            original: ticket.clone(),
            ticket,
            confirm_discard: false,
            error: None,
        }
    }

    fn dirty(&self) -> bool {
        let (a, b) = (&self.ticket, &self.original);
        // Duplicates start pre-filled but are unsaved, so they always count as changed.
        matches!(self.mode, FormMode::Create { .. }) && !a.title.trim().is_empty()
            || a.title != b.title
            || a.priority != b.priority
            || a.run_count != b.run_count
            || parse_blocked(&self.blocked, a.id).ok().as_ref() != Some(&b.blocked_by)
            || labels_from_text(&self.labels) != b.labels
            || self.description.changed()
            || self.test_scope.changed()
            || self.notes.changed()
            || (self.tab == Tab::Ai && !self.request.trim().is_empty())
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<FormOutcome> {
        let mut outcome = None;
        let mut close = false;
        let size = modal_size(ctx);
        let modal = super::show_modal(ctx, "edit_ticket", |ui| {
            ui.set_width(size.x);
            ui.set_height(size.y);
            let heading = match self.mode {
                FormMode::Edit => "Edit Ticket",
                FormMode::Create { .. } => "New Ticket",
            };
            close = super::modal_header(ui, &format!("{heading} - {}", self.ticket.id_str()), "Close");
            tabs(ui, &mut self.tab, &[(Tab::Ai, "AI Assisted"), (Tab::Manual, "Manual")]);
            ui.add_space(4.0);
            if self.tab == Tab::Ai {
                self.ai_body(ui, &mut outcome);
                return;
            }
            ui.add_space(4.0);

            let t = &mut self.ticket;
            ui.label("Title");
            let title = ui.add(
                TextEdit::singleline(&mut t.title)
                    .margin(super::INPUT_MARGIN)
                    .hint_text("Required")
                    .font(egui::TextStyle::Heading)
                    .desired_width(f32::INFINITY),
            );
            if self.original.title.is_empty() && ui.memory(|m| m.focused().is_none()) {
                title.request_focus();
            }
            ui.horizontal(|ui| {
                ui.label("Priority");
                let combo = ComboBox::from_id_salt("priority")
                    .selected_text(priority_text(t.priority))
                    .show_ui(ui, |ui| {
                        for p in Priority::ALL {
                            ui.selectable_value(&mut t.priority, p, priority_text(p));
                        }
                    });
                super::pointer(combo.response);
                ui.add_space(16.0);
                ui.label("Blocked by");
                let field = ui
                    .add(
                        TextEdit::singleline(&mut self.blocked)
                            .margin(super::INPUT_MARGIN)
                            .hint_text("Type a number or title")
                            .desired_width(240.0),
                    )
                    .on_hover_text(
                        "Tickets that must reach the last column first. Type to search by number or title and pick \
                         from the list. Auto and the queue wait for them; ▶ still runs the ticket straight away.",
                    );
                if std::mem::take(&mut self.blocked_refocus) {
                    // Back to typing after the number just added.
                    if let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), field.id) {
                        let end = egui::text::CCursor::new(self.blocked.chars().count());
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
                        state.store(ui.ctx(), field.id);
                    }
                    field.request_focus();
                    self.blocked_open = true;
                } else if field.has_focus() {
                    self.blocked_open = true;
                } else {
                    // Clicking the list takes the keyboard from the field; keep the list until the click lands.
                    let pointer = ui.input(|i| i.pointer.interact_pos());
                    let on_list = self.blocked_list.zip(pointer).is_some_and(|(r, p)| r.contains(p));
                    self.blocked_open &= on_list;
                }
                ui.add_space(16.0);
                ui.label("Labels");
                ui.add(
                    TextEdit::singleline(&mut self.labels)
                        .margin(super::INPUT_MARGIN)
                        .hint_text("e.g. ui, backend")
                        .desired_width(200.0),
                )
                .on_hover_text("Separate labels with commas. Search a column for label:name to find them.");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(DragValue::new(&mut t.run_count).range(0..=u32::MAX).speed(0.1));
                    ui.label("Runs");
                });
            });
            self.blocked_list = None;
            if self.blocked_open {
                let (own, tickets) = (self.ticket.id, &self.tickets);
                let shown = egui::Frame::new()
                    .fill(theme::CARD)
                    .corner_radius(6)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| blocker_list(ui, own, tickets, &mut self.blocked));
                self.blocked_list = Some(shown.response.rect);
                self.blocked_refocus = shown.inner;
            }
            let t = &mut self.ticket;
            let blocked = parse_blocked(&self.blocked, t.id);

            let footer = 56.0;
            let body_h = (ui.available_height() - footer).max(240.0);
            // Toolbar, labels and the optional section headers take roughly this much of the body.
            let chrome = 170.0;
            let open = usize::from(self.scope_open) + usize::from(self.notes_open);
            let (desc_h, section_h) = if open > 0 {
                let room = (body_h - chrome - 50.0 * open as f32).max(240.0);
                let desc = if open == 1 { 0.6 } else { 0.5 };
                (room * desc, room * (1.0 - desc) / open as f32)
            } else {
                ((body_h - chrome).max(160.0), 0.0)
            };
            ScrollArea::vertical()
                .max_height(body_h)
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.label("Description");
                    self.description.show(ui, desc_h);
                    ui.add_space(6.0);
                    optional_section(ui, "Test Scope", &mut self.scope_open, &mut self.test_scope, section_h);
                    optional_section(ui, "Notes", &mut self.notes_open, &mut self.notes, section_h);
                });

            ui.separator();
            ui.horizontal(|ui| {
                if let Some(err) = &self.error {
                    ui.label(RichText::new(err).color(theme::DANGER));
                } else if self.ticket.title.trim().is_empty() {
                    ui.label(RichText::new("A title is required").color(theme::TEXT_MUTED));
                } else if let Err(e) = &blocked {
                    ui.label(RichText::new(e).color(theme::DANGER));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let can_save = !self.ticket.title.trim().is_empty() && blocked.is_ok();
                    if ui
                        .add_enabled_ui(can_save, |ui| accent_button(ui, "Save"))
                        .inner
                        .clicked()
                    {
                        outcome = Some(FormOutcome::Save);
                    }
                });
            });
        });
        // Untouched text keeps its original Markdown byte for byte instead of the editor's normalised form.
        if matches!(outcome, Some(FormOutcome::Save)) {
            self.ticket.blocked_by = parse_blocked(&self.blocked, self.ticket.id).unwrap_or_default();
            self.ticket.labels = labels_from_text(&self.labels);
            if self.description.changed() {
                self.ticket.description = self.description.markdown();
            }
            if self.test_scope.changed() {
                self.ticket.test_scope = self.test_scope.markdown();
            }
            if self.notes.changed() {
                self.ticket.notes = self.notes.markdown();
            }
        }
        if (close || modal.should_close()) && outcome.is_none() {
            if self.dirty() {
                self.confirm_discard = true;
            } else {
                outcome = Some(FormOutcome::Close);
            }
        }
        if self.confirm_discard {
            match confirm(ctx, "discard", "Discard unsaved changes?", "Keep editing", "Discard") {
                Some(true) => outcome = Some(FormOutcome::Close),
                Some(false) => self.confirm_discard = false,
                None => {}
            }
        }
        outcome
    }
}

impl TicketForm {
    fn ai_body(&mut self, ui: &mut Ui, outcome: &mut Option<FormOutcome>) {
        let editing = self.mode == FormMode::Edit;
        let (question, explain, hint, button) = if editing {
            (
                "What should change?",
                "The project's AI CLI updates this ticket's title, priority, description and test scope, using the project for context.",
                "e.g. Raise the priority and add a test for expired reset codes",
                "Update Ticket",
            )
        } else {
            (
                "What should this ticket do?",
                "The project's AI CLI writes the title, priority, description and test scope, using the project for context.",
                "e.g. Add a \"Forgot password\" link to the login page that emails a reset code",
                "Generate Ticket",
            )
        };
        ui.label(question);
        ui.label(RichText::new(explain).small().color(theme::TEXT_MUTED));
        let footer = 56.0;
        let height = (ui.available_height() - footer).max(160.0);
        let request = ui.add_sized(
            vec2(ui.available_width(), height),
            TextEdit::multiline(&mut self.request)
                .margin(super::INPUT_MARGIN)
                .hint_text(hint),
        );
        if ui.memory(|m| m.focused().is_none()) {
            request.request_focus();
        }
        ui.separator();
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let ready = !self.request.trim().is_empty();
            if ui.add_enabled_ui(ready, |ui| accent_button(ui, button)).inner.clicked() {
                *outcome = Some(FormOutcome::Generate(self.request.trim().to_string()));
            }
        });
    }
}

/// Collapsible optional field: a caret and label that show or hide `editor`.
fn optional_section(ui: &mut Ui, label: &str, open: &mut bool, editor: &mut RichEditor, height: f32) {
    let caret = if *open { icon::CARET_DOWN } else { icon::CARET_RIGHT };
    let tip = format!("{} {}", if *open { "Hide" } else { "Show" }, label.to_lowercase());
    let toggled = ui
        .horizontal(|ui| {
            let caret = icon_button(ui, caret, &tip).clicked();
            let label = ui.add(egui::Label::new(label).sense(egui::Sense::click()));
            ui.label(RichText::new("optional").small().color(theme::TEXT_MUTED));
            caret || super::pointer(label).clicked()
        })
        .inner;
    if toggled {
        *open = !*open;
    }
    if *open {
        editor.show(ui, height.max(120.0));
    }
}

/// Underlined tab strip.
fn tabs<T: Copy + PartialEq>(ui: &mut Ui, current: &mut T, items: &[(T, &str)]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 24.0;
        for &(tab, label) in items {
            let selected = *current == tab;
            let color = if selected { theme::ACCENT } else { theme::TEXT_MUTED };
            let text = RichText::new(label).size(15.0).color(color);
            let response = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
            if selected {
                let y = response.rect.bottom() + 5.0;
                ui.painter()
                    .hline(response.rect.x_range(), y, egui::Stroke::new(2.0, theme::ACCENT));
            }
            if super::pointer(response).clicked() {
                *current = tab;
            }
        }
    });
    ui.add_space(2.0);
    ui.separator();
}

/// Priority icon in its colour followed by the label, for the dropdown and its options.
pub fn priority_text(p: Priority) -> LayoutJob {
    let mut job = LayoutJob::default();
    let font = egui::FontId::proportional(14.0);
    job.append(
        priority_icon(p).text(),
        0.0,
        TextFormat::simple(font.clone(), super::priority_color(p)),
    );
    job.append(p.label(), 8.0, TextFormat::simple(font, theme::TEXT));
    job
}

/// Notice with an OK button. Returns true once dismissed.
pub fn message(ctx: &egui::Context, title: &str, body: &str) -> bool {
    let mut done = false;
    let modal = super::show_modal(ctx, ("message", title), |ui| {
        ui.set_width(420.0);
        ui.label(RichText::new(title).size(18.0).strong());
        ui.label(body);
        ui.add_space(6.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            done = accent_button(ui, "OK").clicked();
        });
    });
    done || modal.should_close()
}

/// Small yes/no modal. Returns Some(true) on confirm, Some(false) on cancel.
pub fn confirm(ctx: &egui::Context, id: &str, message: &str, cancel: &str, ok: &str) -> Option<bool> {
    let mut result = None;
    let danger = matches!(ok, "Delete" | "Discard" | "Revert");
    let modal = super::show_modal(ctx, ("confirm", id), |ui| {
        ui.set_width(360.0);
        ui.label(RichText::new(message).size(16.0));
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let ok_btn = if danger {
                egui::Button::new(RichText::new(ok).color(theme::TEXT).strong()).fill(theme::DANGER)
            } else {
                egui::Button::new(RichText::new(ok).color(theme::ACCENT_TEXT).strong()).fill(theme::ACCENT)
            };
            if ui.add(ok_btn).clicked() {
                result = Some(true);
            }
            if ui.button(cancel).clicked() {
                result = Some(false);
            }
        });
    });
    if modal.should_close() && result.is_none() {
        result = Some(false);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn x_in_header_closes_the_form() {
        let ticket = Ticket {
            id: 1,
            title: "T".into(),
            ..Default::default()
        };
        let state = (TicketForm::edit(&ticket), false);
        let mut h = Harness::new_ui_state(
            |ui, (form, closed): &mut (TicketForm, bool)| {
                *closed |= matches!(form.show(ui.ctx()), Some(FormOutcome::Close));
            },
            state,
        );
        crate::theme::apply(&h.ctx);
        h.set_size(vec2(1200.0, 800.0));
        h.run_steps(3); // modals centre themselves once their size is known
        assert!(h.query_by_label("Close").is_none(), "no Close button at the bottom");
        h.get_by_label(icon::X).click();
        h.run();
        assert!(h.state().1);
    }

    #[test]
    fn new_ticket_defaults_to_ai_and_generates_from_the_request() {
        let form = TicketForm::create(
            0,
            Ticket {
                id: 9,
                ..Default::default()
            },
        );
        assert_eq!(form.tab, Tab::Ai);
        let mut h = Harness::new_ui_state(
            |ui, (form, out): &mut (TicketForm, Option<String>)| {
                if let Some(FormOutcome::Generate(request)) = form.show(ui.ctx()) {
                    *out = Some(request);
                }
            },
            (form, None),
        );
        crate::theme::apply(&h.ctx);
        h.set_size(vec2(1200.0, 800.0));
        h.run_steps(3);
        assert!(h.query_by_label("Save").is_none());
        h.state_mut().0.request = "  Add a logout button  ".into();
        h.run();
        h.get_by_label("Generate Ticket").click();
        h.run();
        assert_eq!(h.state().1.as_deref(), Some("Add a logout button"));
    }

    #[test]
    fn notes_open_and_save() {
        let ticket = Ticket {
            id: 4,
            title: "T".into(),
            ..Default::default()
        };
        let mut form = TicketForm::edit(&ticket);
        form.tab = Tab::Manual;
        let mut h = Harness::new_ui_state(
            |ui, (form, saved): &mut (TicketForm, bool)| {
                *saved |= matches!(form.show(ui.ctx()), Some(FormOutcome::Save));
            },
            (form, false),
        );
        crate::theme::apply(&h.ctx);
        h.set_size(vec2(1200.0, 1400.0));
        h.run_steps(3);
        h.get_by_label("Notes").click();
        h.run();
        assert!(h.state().0.notes_open);
        // The notes editor is the last text box in the form.
        h.get_all_by_role(egui::accesskit::Role::MultilineTextInput)
            .last()
            .unwrap()
            .click();
        h.run();
        h.event(egui::Event::Text("Ask design".into()));
        h.run();
        h.get_by_label("Save").click();
        h.run();
        assert!(h.state().1);
        assert_eq!(h.state().0.ticket.notes, "Ask design");
    }

    #[test]
    fn blocked_by_parses_numbers_and_saves() {
        assert_eq!(parse_blocked(" 15, 00012 15 ", 3), Ok(vec![12, 15]));
        assert_eq!(parse_blocked("", 3), Ok(vec![]));
        assert!(parse_blocked("12, abc", 3).is_err());
        assert!(parse_blocked("3", 3).is_err());
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(252), "4m 12s");
        assert_eq!(duration(3780), "1h 03m");

        let ticket = Ticket {
            id: 3,
            title: "T".into(),
            blocked_by: vec![1],
            ..Default::default()
        };
        let mut form = TicketForm::edit(&ticket);
        form.tab = Tab::Manual;
        assert_eq!(form.blocked, "00001");
        assert!(!form.dirty());
        form.blocked = "1, 2".into();
        assert!(form.dirty());
        form.labels = " ui,api ".into();
        let mut h = Harness::new_ui_state(
            |ui, (form, saved): &mut (TicketForm, bool)| {
                *saved |= matches!(form.show(ui.ctx()), Some(FormOutcome::Save));
            },
            (form, false),
        );
        crate::theme::apply(&h.ctx);
        h.set_size(vec2(1200.0, 900.0));
        h.run_steps(3);
        h.get_by_label("Save").click();
        h.run();
        assert!(h.state().1);
        assert_eq!(h.state().0.ticket.blocked_by, [1, 2]);
        assert_eq!(h.state().0.ticket.labels, ["ui", "api"]);
    }

    #[test]
    fn blocked_by_picker_searches_and_appends() {
        let tickets = vec![
            (1, "Login page".to_string()),
            (2, "Logout button".to_string()),
            (3, "Signup".to_string()),
            (12, "Account table".to_string()),
        ];
        let ids = |q: &str, taken: &[u32]| {
            blocker_options(&tickets, 3, q, taken)
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("", &[]), [1, 2, 12]); // never itself
        assert_eq!(ids("LOG", &[1]), [2]);
        assert_eq!(ids("0001", &[]), [1, 12]);
        assert_eq!(ids("12", &[]), [12]);
        assert_eq!(ids("account table", &[]), [12]);

        let mut form = TicketForm::edit(&Ticket {
            id: 3,
            title: "Signup".into(),
            blocked_by: vec![1],
            ..Default::default()
        });
        form.tab = Tab::Manual;
        form.tickets = tickets.clone();
        let mut h = Harness::new_ui_state(|ui, form: &mut TicketForm| _ = form.show(ui.ctx()), form);
        crate::theme::apply(&h.ctx);
        h.set_size(vec2(1200.0, 900.0));
        h.run_steps(3);
        // Real pointer clicks: the field loses the keyboard on press, and the list must survive that.
        let field = h.get_by_label("Blocked by").rect().right_center() + vec2(60.0, 0.0);
        pointer_click(&mut h, field);
        assert!(h.query_by_label("00002  Logout button").is_some(), "the list shows");
        assert!(h.query_by_label("00001  Login page").is_none(), "already listed");
        h.event(egui::Event::Text(", acc".into()));
        h.run();
        assert!(h.query_by_label_contains("matching \"acc\"").is_some());
        assert!(h.query_by_label("00002  Logout button").is_none());
        let row = h.get_by_label("00012  Account table").rect().center();
        pointer_click(&mut h, row);
        assert_eq!(h.state().blocked, "00001, 00012, ");
        assert_eq!(parse_blocked(&h.state().blocked, 3), Ok(vec![1, 12]));
        assert!(
            h.query_by_label("00002  Logout button").is_some(),
            "the list stays for the next one"
        );
        h.event(egui::Event::Text("log".into()));
        h.run();
        let row = h.get_by_label("00002  Logout button").rect().center();
        pointer_click(&mut h, row);
        assert_eq!(h.state().blocked, "00001, 00012, 00002, ");
        // Leaving the field closes the list.
        let title = h.get_by_label("Title").rect().center() + vec2(0.0, 40.0);
        pointer_click(&mut h, title);
        assert!(h.query_by_label("00003  Signup").is_none() && h.query_by_label_contains("Pick a ticket").is_none());
    }

    /// Moves the pointer to `pos` and clicks there.
    /// Single frames between press and release: a focused text box keeps repainting, and `run` would let the
    /// clock pass egui's click time limit.
    fn pointer_click<S>(h: &mut Harness<'_, S>, pos: egui::Pos2) {
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
        h.run();
    }

    #[test]
    fn duplicates_open_on_the_manual_tab() {
        let copy = Ticket {
            id: 10,
            title: "Duplicate: Login".into(),
            ..Default::default()
        };
        assert_eq!(TicketForm::create(0, copy).tab, Tab::Manual);
        let edit = TicketForm::edit(&Ticket {
            id: 1,
            title: "T".into(),
            ..Default::default()
        });
        assert_eq!(edit.tab, Tab::Ai);
    }

    type ViewState = (TicketView, Ticket, bool, Vec<String>);

    /// A ticket view; `done` = in the last column. Outcomes are collected as text.
    fn view_harness(view: TicketView, done: bool) -> Harness<'static, ViewState> {
        let ticket = Ticket {
            id: 5,
            title: "Login".into(),
            ..Default::default()
        };
        let mut h = Harness::new_ui_state(
            |ui, (view_state, t, done, out): &mut ViewState| {
                let mut cache = CommonMarkCache::default();
                match super::view(ui.ctx(), view_state, t, &Links::default(), (*done, "Done"), &mut cache) {
                    Some(ViewOutcome::Approve) => out.push("approve".into()),
                    Some(ViewOutcome::Reject(note)) => out.push(format!("reject {note}")),
                    Some(ViewOutcome::Revert(c)) => out.push(format!("revert {c}")),
                    _ => {}
                }
            },
            (view, ticket, done, Vec::new()),
        );
        crate::theme::apply(&h.ctx);
        h.set_size(vec2(1200.0, 800.0));
        h.run_steps(3);
        h
    }

    #[test]
    fn changes_tab_shows_the_diff_to_approve_or_reject() {
        let mut h = view_harness(TicketView::new(5, None), false);
        h.get_by_label("Changes").click();
        h.run();
        assert!(h.state().0.wants_changes());
        h.state_mut().0.changes = Some(Ok(crate::git::Changes {
            commit: None,
            diff: "--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-old line\n+new line".into(),
        }));
        h.run();
        assert!(h.query_by_label("+new line").is_some());
        assert!(h.query_by_label_contains("Revert").is_none());
        h.get_by_label_contains("Approve").click();
        h.run();
        assert_eq!(h.state().3, ["approve"]);
        // Reject needs a note to send back.
        h.get_by_label_contains("Reject").click();
        h.run();
        assert_eq!(h.state().3.len(), 1);
        h.state_mut().0.reject_note = "  Handle empty passwords ".into();
        h.run();
        h.get_by_label_contains("Reject").click();
        h.run();
        assert_eq!(h.state().3[1], "reject Handle empty passwords");
    }

    #[test]
    fn done_tickets_with_a_commit_can_be_reverted() {
        let mut h = view_harness(TicketView::new(5, Some("abc1234".into())), true);
        assert!(h.query_by_label("abc1234").is_some(), "commit shows in the run history");
        h.get_by_label("Changes").click();
        h.run();
        h.state_mut().0.changes = Some(Ok(crate::git::Changes {
            commit: Some("abc1234".into()),
            diff: String::new(),
        }));
        h.run();
        assert!(h.query_by_label_contains("Approve").is_none());
        assert!(h.query_by_label("Nothing changed outside the board.").is_some());
        h.get_by_label_contains("Revert Commit").click();
        h.run();
        h.get_by_label("Revert").click();
        h.run();
        assert_eq!(h.state().3, ["revert abc1234"]);
    }

    #[test]
    fn linked_tickets_open_from_the_view() {
        let ticket = Ticket {
            id: 5,
            title: "Login".into(),
            ..Default::default()
        };
        let dep = |id, label: &str, waiting| Dependency {
            id,
            label: label.into(),
            waiting,
        };
        let links = Links {
            blocked_by: vec![
                dep(2, "00002 Schema (To Do)", Some(true)),
                dep(9, "00009 (missing)", None),
            ],
            blocks: vec![dep(7, "00007 Logout (Done)", Some(false))],
        };
        let mut h = Harness::new_ui_state(
            |ui, (view_state, opened): &mut (TicketView, Option<u32>)| {
                let mut cache = CommonMarkCache::default();
                if let Some(ViewOutcome::Open(id)) =
                    super::view(ui.ctx(), view_state, &ticket, &links, (false, "Done"), &mut cache)
                {
                    *opened = Some(id);
                }
            },
            (TicketView::new(5, None), None),
        );
        crate::theme::apply(&h.ctx);
        h.set_size(vec2(1200.0, 800.0));
        h.run_steps(3);
        assert!(h.query_by_label_contains("Blocks").is_some());
        h.get_by_label("00007 Logout (Done)").click();
        h.run();
        assert_eq!(h.state().1, Some(7));
        h.get_by_label("00009 (missing)").click();
        h.run();
        assert_eq!(h.state().1, Some(7), "missing tickets aren't links");
    }

    #[test]
    fn edit_ai_tab_offers_update_ticket() {
        let ticket = Ticket {
            id: 3,
            title: "Login".into(),
            ..Default::default()
        };
        let mut h = Harness::new_ui_state(
            |ui, (form, out): &mut (TicketForm, Option<String>)| {
                if let Some(FormOutcome::Generate(request)) = form.show(ui.ctx()) {
                    *out = Some(request);
                }
            },
            (TicketForm::edit(&ticket), None),
        );
        crate::theme::apply(&h.ctx);
        h.set_size(vec2(1200.0, 800.0));
        h.run_steps(3);
        h.state_mut().0.request = "Make it high priority".into();
        h.run();
        h.get_by_label("Update Ticket").click();
        h.run();
        assert_eq!(h.state().1.as_deref(), Some("Make it high priority"));
    }
}
