use egui::{Key, Label, Modifiers, RichText, ScrollArea, TextEdit};

use super::priority_icon;
use crate::app::Action;
use crate::project::Project;
use crate::theme;
use crate::ticket::Ticket;

/// Rows shown at most; typing narrows them down.
const MAX_RESULTS: usize = 50;

/// Commands listed above the tickets while the query doesn't start with `>`.
const MAX_COMMANDS: usize = 5;

/// Ctrl+K: find any ticket on the board by number, title or text and open it, or run a command.
#[derive(Default)]
pub struct QuickOpen {
    pub query: String,
    selected: usize,
}

pub enum QuickOutcome {
    Close,
    Open(u32),
    Run(Action),
}

/// Positions in `commands` whose label has every word of `query`. A query starting with `>` lists commands only,
/// all of them when nothing follows; otherwise an empty query lists none.
pub fn matching_commands(commands: &[(String, Action)], query: &str) -> Vec<usize> {
    let only = query.trim_start().strip_prefix('>');
    let words: Vec<String> = only
        .unwrap_or(query)
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    if words.is_empty() && only.is_none() {
        return Vec::new();
    }
    let found = commands
        .iter()
        .enumerate()
        .filter(|(_, (label, _))| words.iter().all(|w| label.to_lowercase().contains(w)))
        .map(|(i, _)| i);
    if only.is_some() {
        found.collect()
    } else {
        found.take(MAX_COMMANDS).collect()
    }
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
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        project: &Project,
        commands: &[(String, Action)],
    ) -> Option<QuickOutcome> {
        let commanding = self.query.trim_start().starts_with('>');
        let matched = matching_commands(commands, &self.query);
        let found = if commanding {
            Vec::new()
        } else {
            results(project, &self.query)
        };
        // Commands first, then tickets.
        let rows = matched.len() + found.len();
        let pick = |row: usize| match row.checked_sub(matched.len()) {
            None => QuickOutcome::Run(commands[matched[row]].1.clone()),
            Some(t) => QuickOutcome::Open(found[t].1.id),
        };
        // Read before the text box sees them: it would move its cursor instead.
        let (down, up, enter) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
            )
        });
        let last = rows.saturating_sub(1);
        if down {
            self.selected = (self.selected + 1).min(last);
        }
        if up {
            self.selected = self.selected.saturating_sub(1);
        }
        self.selected = self.selected.min(last);
        let mut outcome = (enter && self.selected < rows).then(|| pick(self.selected));
        let modal = super::show_modal(ctx, "quick_open", |ui| {
            ui.set_width(560.0);
            // Rows take the clicks, not the text in them.
            ui.style_mut().interaction.selectable_labels = false;
            let typed = ui.add(
                TextEdit::singleline(&mut self.query)
                    .margin(super::INPUT_MARGIN)
                    .hint_text(format!(
                        "{}  Find a ticket by number, title or text, or type > for commands",
                        egui_phosphor::regular::MAGNIFYING_GLASS
                    ))
                    .desired_width(f32::INFINITY),
            );
            typed.request_focus();
            if typed.changed() {
                self.selected = 0;
            }
            if rows == 0 {
                let text = if commanding {
                    "No commands match."
                } else {
                    "No tickets match."
                };
                ui.label(RichText::new(text).color(theme::TEXT_MUTED));
                return;
            }
            ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (i, &c) in matched.iter().enumerate() {
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
                                ui.label(RichText::new(egui_phosphor::regular::CARET_RIGHT).color(theme::ACCENT));
                                ui.add(Label::new(&commands[c].0).truncate());
                            });
                        })
                        .response
                        .interact(egui::Sense::click());
                    if super::pointer(row).clicked() {
                        outcome = Some(pick(i));
                    }
                }
                for (i, (col, t)) in found.iter().enumerate() {
                    let i = i + matched.len();
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
                if let Some(QuickOutcome::Open(id)) = q.show(ui.ctx(), p, &[]) {
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

    fn commands() -> Vec<(String, Action)> {
        vec![
            ("Project stats".into(), Action::ShowStats),
            ("Login settings".into(), Action::OpenSettings),
            ("Filter the board".into(), Action::ToggleFilter),
        ]
    }

    #[test]
    fn commands_match_every_word_and_show_with_a_prefix() {
        let c = commands();
        assert!(matching_commands(&c, "").is_empty());
        assert_eq!(matching_commands(&c, ">"), [0, 1, 2]);
        assert_eq!(matching_commands(&c, "STATS"), [0]);
        assert_eq!(matching_commands(&c, "the filter"), [2]);
        assert_eq!(matching_commands(&c, "> set"), [1]);
    }

    type PaletteState = (QuickOpen, Project, Option<&'static str>);

    #[test]
    fn commands_come_before_tickets_and_run() {
        let d = tempfile::tempdir().unwrap();
        let state: PaletteState = (QuickOpen::default(), project(d.path()), None);
        let mut h = Harness::new_ui_state(
            |ui, (q, p, out): &mut PaletteState| match q.show(ui.ctx(), p, &commands()) {
                Some(QuickOutcome::Run(Action::OpenSettings)) => *out = Some("settings"),
                Some(QuickOutcome::Open(_)) => *out = Some("ticket"),
                _ => {}
            },
            state,
        );
        theme::apply(&h.ctx);
        h.set_size(egui::vec2(900.0, 700.0));
        h.run_steps(3);
        h.event(egui::Event::Text("login".into()));
        h.run();
        assert!(h.query_by_label("Login settings").is_some());
        assert!(h.query_by_label("Login page").is_some());
        h.key_press(Key::Enter);
        h.run();
        assert_eq!(h.state().2, Some("settings"));
        h.state_mut().2 = None;
        h.state_mut().0.query = ">".into();
        h.run();
        assert!(h.query_by_label("Login page").is_none(), "> lists commands only");
        h.get_by_label("Login settings").click();
        h.run();
        assert_eq!(h.state().2, Some("settings"));
    }
}
