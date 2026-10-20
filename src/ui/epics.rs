//! Epics: the sidebar beside the board, an epic's own view, and the form that makes or edits one.

use egui::{Align, Layout, RichText, ScrollArea, TextEdit, Ui, vec2};
use egui_commonmark::CommonMarkCache;
use egui_phosphor::regular as icon;

use super::rich_editor::RichEditor;
use super::ticket::{Dependency, RADIUS, card, markdown, mini_card, sheet, tabs};
use super::{accent_button, icon_button};
use crate::app::Action;
use crate::project::Project;
use crate::theme;
use crate::ticket::{Epic, Filter, compact};

/// Width of the sidebar when it's collapsed to its button.
pub const COLLAPSED_WIDTH: f32 = 44.0;

/// The epics down the left of the board, each with its progress. Clicking one opens it; its funnel filters the
/// board by it. Finished epics fold away at the bottom.
pub fn sidebar(ui: &mut Ui, p: &Project, filter: &Filter, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{}  Epics", icon::FLAG))
                .strong()
                .color(theme::EPIC),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_button(ui, icon::CARET_LEFT, "Hide epics").clicked() {
                actions.push(Action::ToggleEpics);
            }
            if icon_button(ui, icon::PLUS, "New epic").clicked() {
                actions.push(Action::NewEpic);
            }
        });
    });
    if p.epics.is_empty() {
        ui.label(
            RichText::new(
                "Group tickets working towards one goal. Press + to make an epic, or import a document and its \
                 tickets arrive as one.",
            )
            .small()
            .color(theme::TEXT_MUTED),
        );
        return;
    }
    let finished = |id: u32| {
        let (done, total) = p.epic_progress(id);
        total > 0 && done == total
    };
    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        for e in p.epics.iter().filter(|e| !finished(e.id)) {
            epic_card(ui, p, e, filter, actions);
        }
        let done: Vec<&Epic> = p.epics.iter().filter(|e| finished(e.id)).collect();
        if !done.is_empty() {
            egui::CollapsingHeader::new(RichText::new(format!("Finished ({})", done.len())).color(theme::TEXT_MUTED))
                .id_salt("finished_epics")
                .show(ui, |ui| {
                    for e in done {
                        epic_card(ui, p, e, filter, actions);
                    }
                });
        }
    });
}

/// The collapsed sidebar: just the button that brings it back.
pub fn collapsed(ui: &mut Ui, actions: &mut Vec<Action>) {
    ui.vertical_centered(|ui| {
        if super::glyph_button(ui, icon::FLAG, theme::EPIC, theme::EPIC, true)
            .on_hover_text("Show epics")
            .clicked()
        {
            actions.push(Action::ToggleEpics);
        }
    });
}

/// One epic in the sidebar: title, progress, a button that queues its unfinished tickets, and its filter funnel.
fn epic_card(ui: &mut Ui, p: &Project, e: &Epic, filter: &Filter, actions: &mut Vec<Action>) {
    const BUTTON: f32 = 30.0;
    let filtering = filter.epics.contains(&e.id);
    let (done, total) = p.epic_progress(e.id);
    // Only offered while it has tickets left to do.
    let queue = done < total;
    let buttons = if queue { 2.0 } else { 1.0 };
    // The whole card is clickable underneath its funnel, which takes its own clicks.
    let builder = egui::UiBuilder::new()
        .id_salt(("epic_card", e.id))
        .sense(egui::Sense::click());
    let response = ui
        .scope_builder(builder, |ui| {
            let hovered = ui.response().hovered();
            let stroke = if filtering { theme::EPIC } else { theme::BORDER };
            let frame = egui::Frame::new()
                .fill(if hovered { theme::CARD_HOVER } else { theme::CARD })
                .stroke(egui::Stroke::new(1.0, stroke))
                .corner_radius(RADIUS)
                .inner_margin(egui::Margin {
                    left: 14,
                    right: 8,
                    top: 8,
                    bottom: 8,
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 4.0;
                    // Exactly what's left beside the buttons: any more and the sidebar grows to fit, every frame.
                    let width = ui.available_width() - buttons * (BUTTON + ui.spacing().item_spacing.x);
                    ui.horizontal_top(|ui| {
                        ui.scope(|ui| {
                            ui.set_width(width);
                            ui.add(
                                egui::Label::new(RichText::new(&e.title).strong())
                                    .wrap()
                                    .selectable(false),
                            );
                        });
                        if queue
                            && super::glyph_button(ui, icon::QUEUE, theme::TEXT_MUTED, theme::EPIC, true)
                                .on_hover_text("Add its unfinished tickets to the queue")
                                .clicked()
                        {
                            actions.push(Action::QueueEpic(e.id));
                        }
                        let tip = if filtering {
                            "Stop filtering by this epic"
                        } else {
                            "Show only this epic's tickets"
                        };
                        let idle = if filtering { theme::EPIC } else { theme::TEXT_MUTED };
                        if super::glyph_button(ui, icon::FUNNEL, idle, theme::EPIC, true)
                            .on_hover_text(tip)
                            .clicked()
                        {
                            actions.push(Action::FilterByEpic(e.id));
                        }
                    });
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{} · {done} / {total} done", e.id_str()))
                                .small()
                                .color(theme::TEXT_MUTED),
                        )
                        .selectable(false),
                    );
                    progress_bar(ui, done, total, 4.0);
                });
            super::stripe(ui, frame.response.rect, theme::EPIC);
        })
        .response;
    let label = format!("{} {}", e.id_str(), e.title);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
    if super::pointer(response).on_hover_text("Open this epic").clicked() {
        actions.push(Action::ViewEpic(e.id));
    }
}

/// A thin bar, filled in the epic colour for the share of tickets done.
fn progress_bar(ui: &mut Ui, done: usize, total: usize, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, height / 2.0, theme::BORDER);
    if total > 0 && done > 0 {
        let mut fill = rect;
        fill.set_width(rect.width() * done as f32 / total as f32);
        painter.rect_filled(fill, height / 2.0, theme::EPIC);
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum EpicTab {
    Tickets,
    Description,
    Activity,
}

/// An open epic view.
pub struct EpicView {
    pub id: u32,
    tab: EpicTab,
    /// Asking whether to delete it; `bool` = also take it off its tickets.
    confirm_delete: Option<bool>,
}

impl EpicView {
    pub fn new(id: u32) -> Self {
        EpicView {
            id,
            tab: EpicTab::Tickets,
            confirm_delete: None,
        }
    }
}

pub enum EpicOutcome {
    Close,
    Edit,
    /// Delete it; `clear` also takes it off its tickets.
    Delete {
        clear: bool,
    },
    /// Queue its unfinished tickets.
    QueueAll,
    /// Make a new ticket in it.
    NewTicket,
    /// Ask the AI to write tickets for it.
    Breakdown,
    /// Show this ticket.
    Open(u32),
}

/// Epic `e` of project `p`: its tickets, description and what the AI has spent on it.
pub fn view(
    ctx: &egui::Context,
    state: &mut EpicView,
    p: &Project,
    e: &Epic,
    cache: &mut CommonMarkCache,
) -> Option<EpicOutcome> {
    let mut outcome = None;
    let tickets = p.epic_tickets(e.id);
    let (done, total) = p.epic_progress(e.id);
    let (closed, modal) = sheet(ctx, "view_epic", &e.id_str(), (icon::FLAG, "Epic"), |ui| {
        let card_frame = egui::Frame::new()
            .fill(theme::CARD)
            .stroke(egui::Stroke::new(1.0, theme::BORDER))
            .corner_radius(RADIUS)
            .inner_margin(egui::Margin {
                left: 20,
                right: 16,
                top: 14,
                bottom: 14,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::FLAG).size(22.0).color(theme::EPIC));
                    ui.add(egui::Label::new(RichText::new(&e.title).size(24.0).strong()).wrap());
                });
                ui.label(RichText::new(format!("{done} of {total} tickets done")).color(theme::TEXT_MUTED));
                progress_bar(ui, done, total, 6.0);
            });
        super::stripe(ui, card_frame.response.rect, theme::EPIC);
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.horizontal(|ui| {
                if super::danger_button(ui, "Delete").clicked() {
                    state.confirm_delete = Some(true);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if accent_button(ui, "Edit").clicked() {
                        outcome = Some(EpicOutcome::Edit);
                    }
                    if ui
                        .button(format!("{}  Break into tickets", icon::SPARKLE))
                        .on_hover_text("Ask the project's AI CLI to write tickets for this epic from its description")
                        .clicked()
                    {
                        outcome = Some(EpicOutcome::Breakdown);
                    }
                    let open = tickets.iter().filter(|(c, _)| *c != p.last_col()).count();
                    let queue = ui
                        .add_enabled(open > 0, egui::Button::new(format!("{}  Queue all", icon::QUEUE)))
                        .on_hover_text("Add its unfinished tickets to the queue; blocked ones wait their turn");
                    if queue.clicked() {
                        outcome = Some(EpicOutcome::QueueAll);
                    }
                    if ui
                        .button(format!("{}  Add ticket", icon::PLUS))
                        .on_hover_text("A new ticket in this epic, in the first column")
                        .clicked()
                    {
                        outcome = Some(EpicOutcome::NewTicket);
                    }
                });
            });
            ui.separator();
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                tabs(
                    ui,
                    &mut state.tab,
                    &[
                        (EpicTab::Tickets, "Tickets"),
                        (EpicTab::Description, "Description"),
                        (EpicTab::Activity, "Activity"),
                    ],
                );
                ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| match state.tab {
                        EpicTab::Tickets => {
                            if tickets.is_empty() {
                                ui.label(
                                    RichText::new(
                                        "No tickets yet. Pick this epic in a ticket's Epic field, or press Break into \
                                     tickets.",
                                    )
                                    .color(theme::TEXT_MUTED),
                                );
                            }
                            ui.spacing_mut().item_spacing.y = 6.0;
                            for (col, t) in &tickets {
                                let d = Dependency {
                                    id: t.id,
                                    title: t.title.clone(),
                                    column: p.meta.columns[*col].name.clone(),
                                    priority: Some(t.priority),
                                    waiting: Some(*col != p.last_col()),
                                };
                                if mini_card(ui, &d).clicked() {
                                    outcome = Some(EpicOutcome::Open(t.id));
                                }
                            }
                        }
                        EpicTab::Description => card(ui, "Description", |ui| markdown(ui, cache, &e.description)),
                        EpicTab::Activity => activity(ui, p, e.id),
                    });
            });
        });
    });
    if closed {
        outcome = Some(EpicOutcome::Close);
    }
    if let Some(clear) = &mut state.confirm_delete {
        let n = tickets.len();
        let msg = format!("Delete epic {} – {}?", e.id_str(), e.title);
        let text = format!("Also take it off its {n} tickets");
        let check = (n > 0).then_some((text.as_str(), &mut *clear));
        match super::ticket::confirm_with(ctx, "delete_epic", &msg, check, "Cancel", "Delete") {
            Some(true) => outcome = Some(EpicOutcome::Delete { clear: n > 0 && *clear }),
            Some(false) => state.confirm_delete = None,
            None => {}
        }
    } else if modal.should_close() && outcome.is_none() {
        outcome = Some(EpicOutcome::Close);
    }
    outcome
}

/// What the AI has done on epic `id`'s tickets, from their run histories.
fn activity(ui: &mut Ui, p: &Project, id: u32) {
    let tickets = p.epic_tickets(id);
    let runs: Vec<_> = tickets.iter().flat_map(|(_, t)| &t.runs).collect();
    let seconds: u64 = runs.iter().map(|r| r.seconds).sum();
    let tokens: u64 = runs.iter().filter_map(|r| r.tokens).map(|t| t.total()).sum();
    card(ui, "Activity", |ui| {
        egui::Grid::new("epic_activity").spacing([24.0, 6.0]).show(ui, |ui| {
            for (label, value) in [
                ("AI runs", runs.len().to_string()),
                ("Time the AI spent", super::ticket::duration(seconds)),
                ("Tokens", if tokens > 0 { compact(tokens) } else { "–".into() }),
                (
                    "Runs per ticket",
                    match tickets.len() {
                        0 => "–".into(),
                        n => format!("{:.1}", runs.len() as f32 / n as f32),
                    },
                ),
            ] {
                ui.label(RichText::new(label).color(theme::TEXT_MUTED));
                ui.label(value);
                ui.end_row();
            }
        });
    });
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum FormTab {
    Ai,
    Manual,
}

/// Making a new epic, by hand or by asking the AI, or editing one.
pub struct EpicForm {
    pub epic: Epic,
    original: Epic,
    tab: FormTab,
    request: String,
    description: RichEditor,
    pub error: Option<String>,
}

pub enum EpicFormOutcome {
    Close,
    Save,
    /// Ask the AI CLI to write the epic from this request.
    Generate(String),
}

impl EpicForm {
    /// `epic.id` should already be the number the new epic will get.
    pub fn create(epic: Epic) -> Self {
        Self::new(epic, FormTab::Ai)
    }

    pub fn edit(epic: &Epic) -> Self {
        Self::new(epic.clone(), FormTab::Manual)
    }

    fn new(epic: Epic, tab: FormTab) -> Self {
        EpicForm {
            description: RichEditor::new(("epic_description", epic.id), &epic.description),
            original: epic.clone(),
            epic,
            tab,
            request: String::new(),
            error: None,
        }
    }

    pub fn editing(&self) -> bool {
        !self.original.title.is_empty()
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<EpicFormOutcome> {
        let mut outcome = None;
        let size = super::ticket::modal_size(ctx);
        let title = if self.editing() {
            format!("Edit epic {}", self.epic.id_str())
        } else {
            format!("New epic {}", self.epic.id_str())
        };
        let modal = super::show_modal(ctx, "epic_form", |ui| {
            ui.set_width(size.x.min(900.0));
            ui.set_height(size.y.min(700.0));
            if super::modal_header(ui, &title, "Close") {
                outcome = Some(EpicFormOutcome::Close);
            }
            if !self.editing() {
                tabs(
                    ui,
                    &mut self.tab,
                    &[(FormTab::Ai, "AI Assisted"), (FormTab::Manual, "Manual")],
                );
            }
            if self.tab == FormTab::Ai {
                ui.label("What is this epic for?");
                ui.label(
                    RichText::new(
                        "The project's AI CLI writes its title and a description of the goal, using the project for \
                         context. Break it into tickets afterwards from the epic.",
                    )
                    .small()
                    .color(theme::TEXT_MUTED),
                );
                let height = (ui.available_height() - 56.0).max(160.0);
                ui.add_sized(
                    vec2(ui.available_width(), height),
                    TextEdit::multiline(&mut self.request)
                        .margin(super::INPUT_MARGIN)
                        .hint_text("e.g. Let people pay adoption fees online, safely and without double charges"),
                );
                ui.separator();
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let ready = !self.request.trim().is_empty();
                    if ui
                        .add_enabled_ui(ready, |ui| accent_button(ui, "Generate Epic"))
                        .inner
                        .clicked()
                    {
                        outcome = Some(EpicFormOutcome::Generate(self.request.trim().to_string()));
                    }
                });
                return;
            }
            ui.label("Title");
            ui.add(
                TextEdit::singleline(&mut self.epic.title)
                    .margin(super::INPUT_MARGIN)
                    .hint_text("Required")
                    .font(egui::TextStyle::Heading)
                    .desired_width(f32::INFINITY),
            );
            ui.label("Description");
            let height = (ui.available_height() - 120.0).max(160.0);
            self.description.show(ui, height);
            ui.separator();
            ui.horizontal(|ui| {
                if let Some(err) = &self.error {
                    ui.label(RichText::new(err).color(theme::DANGER));
                } else if self.epic.title.trim().is_empty() {
                    ui.label(RichText::new("A title is required").color(theme::TEXT_MUTED));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let ok = !self.epic.title.trim().is_empty();
                    if ui.add_enabled_ui(ok, |ui| accent_button(ui, "Save")).inner.clicked() {
                        outcome = Some(EpicFormOutcome::Save);
                    }
                });
            });
        });
        if matches!(outcome, Some(EpicFormOutcome::Save)) {
            self.epic.title = self.epic.title.trim().to_string();
            if self.description.changed() {
                self.epic.description = self.description.markdown();
            }
        }
        if modal.should_close() && outcome.is_none() {
            outcome = Some(EpicFormOutcome::Close);
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    fn project(d: &std::path::Path) -> Project {
        let mut p = Project::create(d, "P", &["To Do".into(), "Done".into()]).unwrap();
        let e = p
            .add_epic(Epic {
                title: "Payments".into(),
                ..Default::default()
            })
            .unwrap();
        let t = crate::ticket::Ticket {
            title: "Charge once".into(),
            epic: Some(e.id),
            ..Default::default()
        };
        p.add_ticket(0, t).unwrap();
        p
    }

    #[test]
    fn sidebar_opens_an_epic_and_its_funnel_filters_by_it() {
        let d = tempfile::tempdir().unwrap();
        let p = project(d.path());
        let mut h = Harness::new_ui_state(
            move |ui, actions: &mut Vec<Action>| sidebar(ui, &p, &Filter::default(), actions),
            Vec::new(),
        );
        theme::apply(&h.ctx);
        h.set_size(vec2(260.0, 400.0));
        h.run();
        assert!(h.query_by_label_contains("0 / 1 done").is_some());
        h.get_by_label(icon::FUNNEL).click();
        h.run();
        assert!(matches!(h.state()[..], [Action::FilterByEpic(1)]));
        h.state_mut().clear();
        h.get_by_label(icon::QUEUE).click();
        h.run();
        assert!(matches!(h.state()[..], [Action::QueueEpic(1)]));
        h.state_mut().clear();
        h.get_by_label("00001 Payments").click();
        h.run();
        assert!(matches!(h.state()[..], [Action::ViewEpic(1)]));
    }

    #[test]
    fn the_sidebar_keeps_its_width() {
        let d = tempfile::tempdir().unwrap();
        let p = project(d.path());
        let widths = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = widths.clone();
        let mut h = Harness::new_ui(move |ui| {
            let shown = egui::Panel::left("epics")
                .resizable(true)
                .default_size(250.0)
                .frame(egui::Frame::new().inner_margin(egui::Margin::same(10)))
                .show(ui, |ui| sidebar(ui, &p, &Filter::default(), &mut Vec::new()));
            seen.lock().unwrap().push(shown.response.rect.width());
        });
        theme::apply(&h.ctx);
        h.set_size(vec2(1000.0, 500.0));
        h.run_steps(6);
        let widths = widths.lock().unwrap();
        assert!(widths.windows(2).all(|w| w[0] == w[1]), "{widths:?}");
    }

    #[test]
    fn the_epic_view_lists_its_tickets_and_queues_them() {
        let d = tempfile::tempdir().unwrap();
        let p = project(d.path());
        let mut h = Harness::new_ui_state(
            move |ui, (view, out): &mut (EpicView, Vec<String>)| {
                let e = p.find_epic(1).unwrap().clone();
                let mut cache = CommonMarkCache::default();
                match super::view(ui.ctx(), view, &p, &e, &mut cache) {
                    Some(EpicOutcome::QueueAll) => out.push("queue".into()),
                    Some(EpicOutcome::NewTicket) => out.push("new".into()),
                    Some(EpicOutcome::Open(id)) => out.push(format!("open {id}")),
                    Some(EpicOutcome::Delete { clear }) => out.push(format!("delete {clear}")),
                    _ => {}
                }
            },
            (EpicView::new(1), Vec::new()),
        );
        theme::apply(&h.ctx);
        h.set_size(vec2(1200.0, 800.0));
        h.run_steps(3);
        h.get_by_label("00002 Charge once").click();
        h.run();
        h.get_by_label_contains("Queue all").click();
        h.run();
        h.get_by_label_contains("Add ticket").click();
        h.run();
        h.get_by_label("Delete").click();
        h.run();
        h.get_all_by_label("Delete").last().unwrap().click();
        h.run();
        assert_eq!(h.state().1, ["open 2", "queue", "new", "delete true"]);
    }
}
