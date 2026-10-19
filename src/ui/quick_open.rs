use egui::{Key, Label, Modifiers, RichText, ScrollArea, TextEdit};

use super::priority_icon;
use crate::project::Project;
use crate::theme;
use crate::ticket::Ticket;

/// Rows shown at most; typing narrows them down.
const MAX_RESULTS: usize = 50;

/// Ctrl+K: find any ticket on the board by number, title or text and open it.
#[derive(Default)]
pub struct QuickOpen {
    pub query: String,
    selected: usize,
}

pub enum QuickOutcome {
    Close,
    Open(u32),
}

/// (column, ticket) of the tickets matching `query`, archived ones too: an exact number first, then newest first.
pub fn results<'a>(project: &'a Project, query: &str) -> Vec<(usize, &'a Ticket)> {
    let exact: Option<u32> = query.trim().parse().ok();
    let mut found: Vec<(usize, &Ticket)> = project
        .tickets
        .iter()
        .enumerate()
        .flat_map(|(c, ts)| ts.iter().map(move |t| (c, t)))
        .filter(|(_, t)| t.matches(query))
        .collect();
    found.sort_by_key(|(_, t)| (Some(t.id) != exact, std::cmp::Reverse(t.id)));
    found.truncate(MAX_RESULTS);
    found
}

impl QuickOpen {
    pub fn show(&mut self, ctx: &egui::Context, project: &Project) -> Option<QuickOutcome> {
        let found = results(project, &self.query);
        // Read before the text box sees them: it would move its cursor instead.
        let (down, up, enter) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
            )
        });
        let last = found.len().saturating_sub(1);
        if down {
            self.selected = (self.selected + 1).min(last);
        }
        if up {
            self.selected = self.selected.saturating_sub(1);
        }
        self.selected = self.selected.min(last);
        let mut outcome = enter
            .then(|| found.get(self.selected).map(|(_, t)| QuickOutcome::Open(t.id)))
            .flatten();
        let modal = super::show_modal(ctx, "quick_open", |ui| {
            ui.set_width(560.0);
            // Rows take the clicks, not the text in them.
            ui.style_mut().interaction.selectable_labels = false;
            let typed = ui.add(
                TextEdit::singleline(&mut self.query)
                    .margin(super::INPUT_MARGIN)
                    .hint_text(format!(
                        "{}  Find a ticket by number, title or text",
                        egui_phosphor::regular::MAGNIFYING_GLASS
                    ))
                    .desired_width(f32::INFINITY),
            );
            typed.request_focus();
            if typed.changed() {
                self.selected = 0;
            }
            if found.is_empty() {
                ui.label(RichText::new("No tickets match.").color(theme::TEXT_MUTED));
                return;
            }
            ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (i, (col, t)) in found.iter().enumerate() {
                    let fill = if i == self.selected {
                        theme::CARD_HOVER
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    let row = egui::Frame::new()
                        .fill(fill)
                        .corner_radius(4)
                        .inner_margin(egui::Margin::symmetric(8, 2))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.set_width(ui.available_width());
                                ui.label(priority_icon(t.priority));
                                ui.label(RichText::new(t.id_str()).monospace().color(theme::TEXT_MUTED));
                                let column = &project.meta.columns[*col].name;
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(RichText::new(column).color(theme::TEXT_MUTED));
                                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                        ui.add(Label::new(&t.title).truncate());
                                    });
                                });
                            });
                        })
                        .response
                        .interact(egui::Sense::click());
                    if super::pointer(row).clicked() {
                        outcome = Some(QuickOutcome::Open(t.id));
                    }
                }
            });
        });
        if modal.should_close() && outcome.is_none() {
            outcome = Some(QuickOutcome::Close);
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    fn project(dir: &std::path::Path) -> Project {
        let mut p = Project::create(dir, "P", &["To Do".into(), "Done".into()]).unwrap();
        for (col, title) in [(0, "Login page"), (1, "Logout button"), (0, "Signup")] {
            let t = Ticket {
                title: title.into(),
                ..Default::default()
            };
            p.add_ticket(col, t).unwrap();
        }
        p
    }

    #[test]
    fn finds_across_columns_with_an_exact_number_first() {
        let d = tempfile::tempdir().unwrap();
        let p = project(d.path());
        let ids = |q: &str| results(&p, q).iter().map(|(_, t)| t.id).collect::<Vec<_>>();
        assert_eq!(ids("log"), [2, 1]);
        assert_eq!(ids(""), [3, 2, 1]);
        assert_eq!(ids("1"), [1]);
        assert_eq!(ids("nothing"), Vec::<u32>::new());
    }

    #[test]
    fn typing_and_enter_open_the_ticket() {
        let d = tempfile::tempdir().unwrap();
        let state = (QuickOpen::default(), project(d.path()), None::<u32>);
        let mut h = Harness::new_ui_state(
            |ui, (q, p, opened): &mut (QuickOpen, Project, Option<u32>)| {
                if let Some(QuickOutcome::Open(id)) = q.show(ui.ctx(), p) {
                    *opened = Some(id);
                }
            },
            state,
        );
        theme::apply(&h.ctx);
        h.set_size(egui::vec2(900.0, 700.0));
        h.run_steps(3);
        h.event(egui::Event::Text("log".into()));
        h.run();
        assert!(h.query_by_label("Signup").is_none());
        h.key_press(Key::ArrowDown);
        h.run();
        h.key_press(Key::Enter);
        h.run();
        assert_eq!(h.state().2, Some(1)); // second row: "Login page"
    }
}
