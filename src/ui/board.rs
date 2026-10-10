use std::collections::HashMap;

use egui::text::LayoutJob;
use egui::{
    Align, Area, CornerRadius, FontId, Frame, Id, Layout, Margin, Order, Popup, RichText, ScrollArea, Sense, Stroke,
    Ui, UiBuilder, vec2,
};
use egui_phosphor::regular as icon;

use super::{icon_button, priority_icon};
use crate::app::{Action, OpenProject};
use crate::project::{Column, Sort, SortBy, SortDir};
use crate::theme;
use crate::ticket::Ticket;

const MIN_COLUMN_WIDTH: f32 = 300.0;
const COLUMN_GAP: f32 = 12.0;

/// Drag payload for a ticket card.
pub struct DragTicket(pub u32);

pub fn show(ui: &mut Ui, op: &mut OpenProject, actions: &mut Vec<Action>) {
    let Ok(project) = &op.project else { return };
    let width = column_width(ui.available_width(), project.meta.columns.len());
    ScrollArea::horizontal().auto_shrink(false).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = COLUMN_GAP;
            let height = ui.available_height();
            for (col, column) in project.meta.columns.iter().enumerate() {
                ui.allocate_ui_with_layout(vec2(width, height), Layout::top_down(Align::Min), |ui| {
                    column_ui(ui, col, column, width, &project.tickets[col], &mut op.search, actions);
                });
            }
        });
    });
}

/// Columns share the board width equally, but never shrink below `MIN_COLUMN_WIDTH`
/// (the board scrolls sideways instead).
fn column_width(available: f32, columns: usize) -> f32 {
    let n = columns.max(1) as f32;
    ((available - COLUMN_GAP * (n - 1.0)) / n).floor().max(MIN_COLUMN_WIDTH)
}

fn column_ui(
    ui: &mut Ui,
    col: usize,
    column: &Column,
    width: f32,
    tickets: &[Ticket],
    search: &mut HashMap<String, String>,
    actions: &mut Vec<Action>,
) {
    let mut prepared = Frame::new()
        .fill(theme::PANEL)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(8))
        .begin(ui);
    {
        let ui = &mut prepared.content_ui;
        ui.set_width(width - 16.0);
        ui.set_min_height(ui.available_height());

        let mut focus_search = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(&column.name).strong().size(15.0));
            ui.label(RichText::new(format!("({})", tickets.len())).color(theme::TEXT_MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, icon::PLUS, "Add ticket").clicked() {
                    actions.push(Action::NewTicket(col));
                }
                let sort_btn = icon_button(ui, icon::ARROWS_DOWN_UP, "Sort");
                Popup::menu(&sort_btn).show(|ui| sort_menu(ui, col, column.sort, actions));
                if icon_button(ui, icon::MAGNIFYING_GLASS, "Search").clicked()
                    && search.remove(&column.folder).is_none()
                {
                    search.insert(column.folder.clone(), String::new());
                    focus_search = true;
                }
            });
        });

        let mut close_search = false;
        if let Some(q) = search.get_mut(&column.folder) {
            let resp = ui.add(
                egui::TextEdit::singleline(q)
                    .margin(super::INPUT_MARGIN)
                    .hint_text(format!("{} Search {}", icon::MAGNIFYING_GLASS, column.name))
                    .desired_width(f32::INFINITY),
            );
            if focus_search {
                resp.request_focus();
            }
            close_search = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape));
        }
        if close_search {
            search.remove(&column.folder);
        }
        let query = search.get(&column.folder).map_or("", String::as_str);
        ui.add_space(4.0);

        ScrollArea::vertical()
            .id_salt(("column", col))
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                for t in tickets.iter().filter(|t| t.matches(query)) {
                    card(ui, t, actions);
                }
            });
    }
    let response = prepared.allocate_space(ui);
    if let Some(payload) = response.dnd_hover_payload::<DragTicket>() {
        let from_here = tickets.iter().any(|t| t.id == payload.0);
        if !from_here {
            prepared.frame.stroke = Stroke::new(2.0, theme::ACCENT);
        }
    }
    if let Some(payload) = response.dnd_release_payload::<DragTicket>() {
        actions.push(Action::MoveTicket {
            id: payload.0,
            to_col: col,
        });
    }
    prepared.paint(ui);
}

fn sort_menu(ui: &mut Ui, col: usize, sort: Sort, actions: &mut Vec<Action>) {
    ui.label(RichText::new("Sort by").color(theme::TEXT_MUTED));
    for (by, label) in [
        (SortBy::Id, "ID"),
        (SortBy::Priority, "Priority"),
        (SortBy::Title, "Title"),
    ] {
        if super::pointer(ui.radio(sort.by == by, label)).clicked() {
            actions.push(Action::SetSort {
                col,
                sort: Sort { by, ..sort },
            });
        }
    }
    ui.separator();
    for (dir, label) in [(SortDir::Asc, "Ascending"), (SortDir::Desc, "Descending")] {
        if super::pointer(ui.radio(sort.dir == dir, label)).clicked() {
            actions.push(Action::SetSort {
                col,
                sort: Sort { dir, ..sort },
            });
        }
    }
}

fn card(ui: &mut Ui, t: &Ticket, actions: &mut Vec<Action>) {
    let builder = UiBuilder::new().id_salt(("card", t.id)).sense(Sense::click_and_drag());
    let response = ui
        .scope_builder(builder, |ui| {
            ui.style_mut().interaction.selectable_labels = false;
            let r = ui.response();
            let fill = if r.hovered() || r.dragged() {
                theme::CARD_HOVER
            } else {
                theme::CARD
            };
            let alpha = if r.dragged() { 0.4 } else { 1.0 };
            ui.set_opacity(alpha);
            Frame::new()
                .fill(fill)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(10, 8))
                .show(ui, |ui| card_contents(ui, t, actions));
        })
        .response;

    if response.drag_started() {
        response.dnd_set_drag_payload(DragTicket(t.id));
    }
    if response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        drag_preview(ui, t);
    }
    let response = super::pointer(response);
    if response.clicked() {
        actions.push(Action::ViewTicket(t.id));
    }
    response.context_menu(|ui| {
        if ui.button(format!("{}  Edit Ticket", icon::PENCIL_SIMPLE)).clicked() {
            actions.push(Action::EditTicket(t.id));
        }
        if ui.button(format!("{}  Duplicate Ticket", icon::COPY)).clicked() {
            actions.push(Action::DuplicateTicket(t.id));
        }
        if ui
            .button(RichText::new(format!("{}  Delete Ticket", icon::TRASH)).color(theme::DANGER))
            .clicked()
        {
            actions.push(Action::AskDelete(t.id));
        }
    });
}

fn card_contents(ui: &mut Ui, t: &Ticket, actions: &mut Vec<Action>) {
    const PLAY: f32 = 30.0;
    let full = ui.available_width();
    let left = ui.cursor().left();
    ui.set_min_width(full);
    let width = (full - PLAY - 6.0).max(40.0);
    let text_rect = ui
        .allocate_ui(vec2(width, 0.0), |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.label(priority_icon(t.priority)).on_hover_text(t.priority.label());
                ui.label(RichText::new(t.id_str()).monospace().color(theme::TEXT_MUTED));
                if t.malformed {
                    ui.label(RichText::new(icon::WARNING).color(theme::DANGER))
                        .on_hover_text("This ticket file's header couldn't be read");
                }
            });
            ui.label(title_job(ui, &t.title, width));
        })
        .response
        .rect;
    // Fixed slot at the right edge, vertically centred.
    let slot = egui::Rect::from_min_size(
        egui::pos2(left + full - PLAY, text_rect.center().y - PLAY / 2.0),
        vec2(PLAY, PLAY),
    );
    let clicked = ui
        .put(slot, |ui: &mut Ui| {
            super::glyph_button(ui, icon::PLAY, theme::ACCENT, true)
        })
        .on_hover_text("Complete task with AI")
        .clicked();
    if clicked {
        actions.push(Action::RunAi(t.id));
    }
}

/// Title wrapped to at most two lines, then an ellipsis.
fn title_job(ui: &Ui, title: &str, width: f32) -> LayoutJob {
    let mut job = LayoutJob::simple(
        title.to_owned(),
        FontId::proportional(14.0),
        ui.visuals().text_color(),
        width,
    );
    job.wrap.max_rows = 2;
    job.wrap.break_anywhere = false;
    job
}

fn drag_preview(ui: &Ui, t: &Ticket) {
    let Some(pos) = ui.ctx().pointer_interact_pos() else {
        return;
    };
    Area::new(Id::new("ticket_drag_preview"))
        .order(Order::Tooltip)
        .fixed_pos(pos + vec2(12.0, 8.0))
        .interactable(false)
        .show(ui.ctx(), |ui| {
            Frame::new()
                .fill(theme::CARD_HOVER)
                .stroke(Stroke::new(1.0, theme::ACCENT))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(priority_icon(t.priority));
                        ui.label(RichText::new(t.id_str()).monospace().color(theme::TEXT_MUTED));
                        ui.label(&t.title);
                    });
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    type State = (OpenProject, Vec<Action>);

    fn harness(dir: &std::path::Path) -> Harness<'static, State> {
        harness_with(dir, &["Hello"])
    }

    fn harness_with(dir: &std::path::Path, titles: &[&str]) -> Harness<'static, State> {
        let mut p = Project::create(dir, "P", &["To Do".into(), "Done".into()]).unwrap();
        for title in titles {
            let t = Ticket {
                title: title.to_string(),
                ..Default::default()
            };
            p.add_ticket(0, t).unwrap();
        }
        let op = OpenProject {
            path: p.ghost_path.clone(),
            project: Ok(p),
            search: HashMap::new(),
        };
        let mut h = Harness::new_ui_state(|ui, (op, actions): &mut State| show(ui, op, actions), (op, Vec::new()));
        crate::theme::apply(&h.ctx);
        h.set_size(vec2(1000.0, 600.0));
        h.run();
        h
    }

    #[test]
    fn clicking_card_opens_ticket() {
        let d = tempfile::tempdir().unwrap();
        let mut h = harness(d.path());
        h.get_by_label("Hello").click();
        h.run();
        assert!(h.state().1.iter().any(|a| matches!(a, Action::ViewTicket(1))));
    }

    #[test]
    fn play_button_runs_ai() {
        let d = tempfile::tempdir().unwrap();
        let mut h = harness(d.path());
        h.get_by_label(icon::PLAY).click();
        h.run();
        let actions = &h.state().1;
        assert!(actions.iter().any(|a| matches!(a, Action::RunAi(1))));
        assert!(!actions.iter().any(|a| matches!(a, Action::ViewTicket(_))));
    }

    #[test]
    fn add_button_creates_in_column() {
        let d = tempfile::tempdir().unwrap();
        let mut h = harness(d.path());
        h.get_all_by_label(icon::PLUS).nth(1).unwrap().click();
        h.run();
        assert!(h.state().1.iter().any(|a| matches!(a, Action::NewTicket(1))));
    }

    #[test]
    fn dragging_card_to_other_column_moves_it() {
        let d = tempfile::tempdir().unwrap();
        let mut h = harness(d.path());
        let from = h.get_by_label("Hello").rect().center();
        let done = h.get_by_label("Done").rect().center();
        let to = egui::pos2(done.x, from.y + 120.0);
        h.hover_at(from);
        h.drag_at(from);
        h.run();
        for i in 1..=10 {
            h.hover_at(from + (to - from) * (i as f32 / 10.0));
            h.run();
        }
        h.drop_at(to);
        h.run();
        assert!(
            h.state()
                .1
                .iter()
                .any(|a| matches!(a, Action::MoveTicket { id: 1, to_col: 1 }))
        );
    }

    #[test]
    fn cards_fill_the_column() {
        let d = tempfile::tempdir().unwrap();
        let titles = [
            "Hi",
            "A much longer title that wraps onto a second line for sure",
            "Short one",
        ];
        let mut h = harness_with(d.path(), &titles);
        h.run();
        let rights: Vec<f32> = h.get_all_by_label(icon::PLAY).map(|n| n.rect().right()).collect();
        assert_eq!(rights.len(), 3);
        assert!(
            rights.iter().all(|r| (r - rights[0]).abs() < 0.5),
            "play buttons misaligned: {rights:?}"
        );
        let next_column = h.get_by_label("Done").rect().left();
        assert!(rights[0] < next_column, "card overflows the column: {rights:?}");
    }

    #[test]
    fn columns_stretch_but_keep_minimum() {
        assert_eq!(column_width(1012.0, 2), 500.0);
        assert_eq!(column_width(500.0, 5), MIN_COLUMN_WIDTH);
        assert_eq!(column_width(1000.0, 0), 1000.0);

        // In the 1000px harness two columns split the space, so "Done" starts near the middle.
        let d = tempfile::tempdir().unwrap();
        let h = harness(d.path());
        assert!(h.get_by_label("Done").rect().left() > 490.0);
    }

    #[test]
    fn clickable_things_show_pointer_cursor() {
        let d = tempfile::tempdir().unwrap();
        let mut h = harness(d.path());
        for label in [icon::PLAY, icon::PLUS, "Hello"] {
            h.get_all_by_label(label).next().unwrap().hover();
            h.run();
            assert_eq!(
                h.output().platform_output.cursor_icon,
                egui::CursorIcon::PointingHand,
                "{label}"
            );
        }
    }
}
