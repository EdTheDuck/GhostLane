use egui::text::{LayoutJob, TextFormat};
use egui::{Align, ComboBox, DragValue, Layout, RichText, ScrollArea, TextEdit, Ui, vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use egui_phosphor::regular as icon;

use super::rich_editor::RichEditor;
use super::{accent_button, icon_button, priority_icon};
use crate::theme;
use crate::ticket::{Priority, Ticket};

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
}

pub fn view(ctx: &egui::Context, t: &Ticket, cache: &mut CommonMarkCache) -> Option<ViewOutcome> {
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
        ui.separator();
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
        });
    });
    if modal.should_close() && outcome.is_none() {
        outcome = Some(ViewOutcome::Close);
    }
    outcome
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
            tabs(ui, &mut self.tab);
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
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add(DragValue::new(&mut t.run_count).range(0..=u32::MAX).speed(0.1));
                    ui.label("Runs");
                });
            });

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
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let can_save = !self.ticket.title.trim().is_empty();
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
fn tabs(ui: &mut Ui, current: &mut Tab) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 24.0;
        for (tab, label) in [(Tab::Ai, "AI Assisted"), (Tab::Manual, "Manual")] {
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
fn priority_text(p: Priority) -> LayoutJob {
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
    let danger = ok == "Delete" || ok == "Discard";
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
