use std::collections::HashSet;

use egui::text::LayoutJob;
use egui::{
    Align, Area, CornerRadius, FontId, Frame, Id, Layout, Margin, Order, Popup, RichText, ScrollArea, Sense, Stroke,
    Ui, UiBuilder, vec2,
};
use egui_phosphor::regular as icon;

use super::{icon_button, priority_icon};
use crate::app::{Action, OpenProject};
use crate::project::{AutoMove, Project, Sort, SortBy, SortDir};
use crate::theme;
use crate::ticket::Ticket;

const MIN_COLUMN_WIDTH: f32 = 300.0;
const COLUMN_GAP: f32 = 12.0;

/// Drag payload for a ticket card.
pub struct DragTicket(pub u32);

/// Board-wide settings the columns need.
#[derive(Clone, Copy)]
pub struct BoardSettings {
    /// The AI is busy with or has queued tickets, so play buttons add to the queue instead.
    pub engaged: bool,
    /// Auto skips tickets sent to the AI this many times; their cards are flagged.
    pub run_limit: u32,
    /// The last column hides tickets that have been there longer than this many days. 0 = never.
    pub archive_days: u32,
}

pub fn show(ui: &mut Ui, op: &mut OpenProject, settings: BoardSettings, actions: &mut Vec<Action>) {
    let Ok(project) = &op.project else { return };
    op.selected.retain(|&id| project.find_ticket(id).is_some());
    if !op.selected.is_empty() && ui.input(|i| i.key_pressed(egui::Key::Escape)) && ui.memory(|m| m.focused().is_none())
    {
        op.selected.clear();
    }
    if !op.selected.is_empty() {
        selection_bar(ui, op, actions);
        ui.add_space(8.0);
    }
    let Ok(project) = &op.project else { return };
    let columns = project.meta.columns.len();
    let width = column_width(ui.available_width(), columns);
    ScrollArea::horizontal().auto_shrink(false).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = COLUMN_GAP;
            let height = ui.available_height();
            for col in 0..columns {
                ui.allocate_ui_with_layout(vec2(width, height), Layout::top_down(Align::Min), |ui| {
                    column_ui(ui, op, col, width, settings, actions);
                });
            }
        });
    });
}

/// The selected tickets in board order: columns left to right, each top to bottom.
fn selection(p: &Project, selected: &HashSet<u32>) -> Vec<u32> {
    p.tickets
        .iter()
        .flatten()
        .filter(|t| selected.contains(&t.id))
        .map(|t| t.id)
        .collect()
}

/// What to do with all the selected tickets at once.
fn selection_bar(ui: &mut Ui, op: &mut OpenProject, actions: &mut Vec<Action>) {
    let Ok(project) = &op.project else { return };
    let ids = selection(project, &op.selected);
    let mut clear = false;
    ui.horizontal(|ui| {
        let n = ids.len();
        ui.label(RichText::new(format!("{n} selected")).strong());
        ui.add_space(8.0);
        if ui
            .button(format!("{}  Send to AI", icon::PLAY))
            .on_hover_text("Run the first now and queue the rest, in board order")
            .clicked()
        {
            actions.push(Action::QueueTickets(ids.clone()));
        }
        ui.menu_button(format!("{}  Move to", icon::ARROW_RIGHT), |ui| {
            for (col, c) in project.meta.columns.iter().enumerate() {
                if ui.button(&c.name).clicked() {
                    actions.push(Action::MoveTickets {
                        ids: ids.clone(),
                        to_col: col,
                    });
                }
            }
        });
        ui.menu_button(format!("{}  Priority", icon::CARET_UP), |ui| {
            for p in crate::ticket::Priority::ALL {
                if ui.button(super::ticket::priority_text(p)).clicked() {
                    actions.push(Action::SetPriority {
                        ids: ids.clone(),
                        priority: p,
                    });
                }
            }
        });
        if ui
            .button(RichText::new(format!("{}  Delete", icon::TRASH)).color(theme::DANGER))
            .clicked()
        {
            actions.push(Action::AskDelete(ids.clone()));
        }
        clear = icon_button(ui, icon::X, "Clear the selection (Esc)").clicked();
    });
    if clear {
        op.selected.clear();
    }
}

/// Columns share the board width equally, but never shrink below `MIN_COLUMN_WIDTH`
/// (the board scrolls sideways instead).
fn column_width(available: f32, columns: usize) -> f32 {
    let n = columns.max(1) as f32;
    ((available - COLUMN_GAP * (n - 1.0)) / n).floor().max(MIN_COLUMN_WIDTH)
}

fn column_ui(ui: &mut Ui, op: &mut OpenProject, col: usize, width: f32, s: BoardSettings, actions: &mut Vec<Action>) {
    let OpenProject {
        project: Ok(project),
        search,
        auto,
        show_archived,
        selected,
        anchor,
        ..
    } = op
    else {
        return;
    };
    let (column, tickets) = (&project.meta.columns[col], &project.tickets[col]);
    let auto_column = project.auto_target(col, AutoMove::AiWorking).is_some();
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
            let gap = ui.spacing().item_spacing.x;
            // Buttons first, so a long column name shortens instead of running under them.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Visible gap between glyphs was 14 (30px button, 16px glyph) + 12 spacing; 75% of that.
                ui.spacing_mut().item_spacing.x = 5.5;
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
                if auto_column {
                    auto_toggle(ui, auto, &column.folder);
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    let count =
                        egui::WidgetText::from(RichText::new(format!("({})", tickets.len())).color(theme::TEXT_MUTED))
                            .into_galley(
                                ui,
                                Some(egui::TextWrapMode::Extend),
                                f32::INFINITY,
                                egui::TextStyle::Body,
                            );
                    let name_width = (ui.available_width() - count.size().x - gap - 8.0).max(0.0);
                    ui.scope(|ui| {
                        ui.set_max_width(name_width);
                        ui.add(egui::Label::new(RichText::new(&column.name).strong().size(15.0)).truncate());
                    });
                    ui.label(count);
                });
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
                // Tickets arrive here because the AI is already on them.
                let play = (column.auto_move != AutoMove::AiWorking).then_some(if s.engaged {
                    (icon::LIST_PLUS, "Add to the AI queue")
                } else {
                    (icon::PLAY, "Complete task with AI")
                });
                // The last column tucks away tickets finished a while ago; a search still finds them.
                let now = crate::ticket::now_secs();
                let archived = |t: &Ticket| col == project.last_col() && t.archived(now, s.archive_days);
                let hidden = tickets.iter().filter(|t| archived(t)).count();
                let visible: Vec<&Ticket> = tickets
                    .iter()
                    .filter(|t| t.matches(query))
                    .filter(|t| !archived(t) || *show_archived || !query.trim().is_empty())
                    .collect();
                for t in &visible {
                    let flagged = auto_column && t.at_run_limit(s.run_limit);
                    let picked = selected.contains(&t.id);
                    let dragging = if picked { selected.len() } else { 1 };
                    if card(ui, t, play, flagged, &project.blockers(t), (picked, dragging), actions) {
                        click(ui, t.id, &visible, selected, anchor, actions);
                    }
                }
                if hidden > 0 && query.trim().is_empty() {
                    let label = if *show_archived {
                        "Hide archived".to_string()
                    } else {
                        format!("Show {hidden} archived")
                    };
                    ui.vertical_centered(|ui| {
                        let tip = format!("Tickets in {} for more than {} days", column.name, s.archive_days);
                        if super::pointer(ui.link(RichText::new(label).color(theme::TEXT_MUTED)))
                            .on_hover_text(tip)
                            .clicked()
                        {
                            *show_archived = !*show_archived;
                        }
                    });
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
        // Dragging one of the selected cards takes the whole selection along.
        let ids = if selected.contains(&payload.0) {
            selection(project, selected)
        } else {
            vec![payload.0]
        };
        actions.push(Action::MoveTickets { ids, to_col: col });
    }
    prepared.paint(ui);
}

/// Ctrl (Cmd on macOS) click adds a card to the selection or takes it out, Shift-click selects the cards between it
/// and the last one clicked in the column, and a plain click opens the ticket.
fn click(
    ui: &Ui,
    id: u32,
    visible: &[&Ticket],
    selected: &mut HashSet<u32>,
    anchor: &mut Option<u32>,
    actions: &mut Vec<Action>,
) {
    let m = ui.input(|i| i.modifiers);
    let pos = |id: u32| visible.iter().position(|t| t.id == id);
    if m.shift
        && let (Some(a), Some(b)) = (anchor.and_then(pos), pos(id))
    {
        selected.extend(visible[a.min(b)..=a.max(b)].iter().map(|t| t.id));
    } else if m.command || m.shift {
        if !selected.remove(&id) {
            selected.insert(id);
        }
        *anchor = Some(id);
    } else {
        selected.clear();
        *anchor = Some(id);
        actions.push(Action::ViewTicket(id));
    }
}

/// "Auto" switch: while on, the AI works through this column's tickets by itself whenever its queue is empty.
fn auto_toggle(ui: &mut Ui, auto: &mut HashSet<String>, folder: &str) {
    const TIP: &str = "Work on this column's tickets automatically when the queue is empty";
    let mut on = auto.contains(folder);
    ui.add_space(6.0);
    super::switch_row(ui, &mut on, "Auto").on_hover_text(TIP);
    if on {
        auto.insert(folder.to_string());
    } else {
        auto.remove(folder);
    }
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

/// `flagged`: Auto skips the ticket, it has been sent to the AI too many times. `blockers`: tickets it waits for.
/// `selected`: the card is part of the selection, and dragging it takes `dragging` cards along. True when clicked.
fn card(
    ui: &mut Ui,
    t: &Ticket,
    play: Option<(&str, &str)>,
    flagged: bool,
    blockers: &[u32],
    (selected, dragging): (bool, usize),
    actions: &mut Vec<Action>,
) -> bool {
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
            let rect = Frame::new()
                .fill(fill)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| card_contents(ui, t, play, flagged, blockers, actions))
                .response
                .rect;
            if selected {
                let stroke = Stroke::new(2.0, theme::ACCENT);
                ui.painter().rect_stroke(rect, 6, stroke, egui::StrokeKind::Inside);
            }
        })
        .response;

    if response.drag_started() {
        response.dnd_set_drag_payload(DragTicket(t.id));
    }
    if response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        drag_preview(ui, t, dragging);
    }
    let response = super::pointer(response);
    response.context_menu(|ui| {
        if ui.button(format!("{}  Edit Ticket", icon::PENCIL_SIMPLE)).clicked() {
            actions.push(Action::EditTicket(t.id));
        }
        if ui.button(format!("{}  Duplicate Ticket", icon::COPY)).clicked() {
            actions.push(Action::DuplicateTicket(t.id));
        }
        if t.run_count > 0
            && ui
                .button(format!("{}  Reset Run Count", icon::ARROW_COUNTER_CLOCKWISE))
                .clicked()
        {
            actions.push(Action::ResetRunCount(t.id));
        }
        if ui
            .button(RichText::new(format!("{}  Delete Ticket", icon::TRASH)).color(theme::DANGER))
            .clicked()
        {
            actions.push(Action::AskDelete(vec![t.id]));
        }
    });
    response.clicked()
}

/// play: glyph and tooltip of the AI button, if the card has one.
fn card_contents(
    ui: &mut Ui,
    t: &Ticket,
    play: Option<(&str, &str)>,
    flagged: bool,
    blockers: &[u32],
    actions: &mut Vec<Action>,
) {
    const PLAY: f32 = 30.0;
    let full = ui.available_width();
    let left = ui.cursor().left();
    ui.set_min_width(full);
    let width = if play.is_some() {
        (full - PLAY - 6.0).max(40.0)
    } else {
        full
    };
    let text_rect = ui
        .allocate_ui(vec2(width, 0.0), |ui| {
            ui.set_width(width);
            // Rows hug their text here; the app-wide 30px row height would pad the ID line.
            ui.spacing_mut().interact_size.y = 0.0;
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.horizontal(|ui| {
                ui.label(priority_icon(t.priority)).on_hover_text(t.priority.label());
                ui.label(RichText::new(t.id_str()).monospace().color(theme::TEXT_MUTED));
                if t.malformed {
                    ui.label(RichText::new(icon::WARNING).color(theme::DANGER))
                        .on_hover_text("This ticket file's header couldn't be read");
                }
                if flagged {
                    let tip = format!(
                        "Sent to the AI {} times, so Auto skips it. Press ▶ to run it anyway, or right-click to reset \
                         its run count.",
                        t.run_count
                    );
                    ui.label(RichText::new(icon::FLAG).color(theme::DANGER))
                        .on_hover_text(tip);
                }
                if !blockers.is_empty() {
                    let ids: Vec<String> = blockers.iter().map(|&b| crate::ticket::format_id(b)).collect();
                    let tip = format!(
                        "Blocked by {}. Auto and the queue wait until they're done; ▶ runs it anyway.",
                        ids.join(", ")
                    );
                    ui.label(RichText::new(icon::LOCK).color(theme::TEXT_MUTED))
                        .on_hover_text(tip);
                }
            });
            ui.label(title_job(ui, &t.title, width));
        })
        .response
        .rect;
    let Some((glyph, tip)) = play else { return };
    // Fixed slot at the right edge, vertically centred.
    let slot = egui::Rect::from_min_size(
        egui::pos2(left + full - PLAY, text_rect.center().y - PLAY / 2.0),
        vec2(PLAY, PLAY),
    );
    let clicked = ui
        .put(slot, |ui: &mut Ui| {
            super::glyph_button(ui, glyph, theme::ACCENT, theme::ACCENT, true)
        })
        .on_hover_text(tip)
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

/// `count`: cards being dragged, this one included.
fn drag_preview(ui: &Ui, t: &Ticket, count: usize) {
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
                        if count > 1 {
                            ui.label(RichText::new(format!("+{} more", count - 1)).color(theme::ACCENT));
                        }
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

    /// The project, actions pushed, and whether the AI is engaged.
    type State = (OpenProject, Vec<Action>, bool);

    fn harness(dir: &std::path::Path) -> Harness<'static, State> {
        harness_with(dir, &["Hello"])
    }

    fn harness_with(dir: &std::path::Path, titles: &[&str]) -> Harness<'static, State> {
        harness_cols(dir, titles, AutoMove::Manual)
    }

    /// Two columns, "To Do" then "Done" (which tickets move into on `done_move`); `titles` go in "To Do",
    /// plus one ticket in "Done".
    fn harness_cols(dir: &std::path::Path, titles: &[&str], done_move: AutoMove) -> Harness<'static, State> {
        let mut p = Project::create(dir, "P", &["To Do".into(), "Done".into()]).unwrap();
        p.meta.columns[1].auto_move = done_move;
        for title in titles {
            let t = Ticket {
                title: title.to_string(),
                ..Default::default()
            };
            p.add_ticket(0, t).unwrap();
        }
        if done_move != AutoMove::Manual {
            let t = Ticket {
                title: "Finished".into(),
                ..Default::default()
            };
            p.add_ticket(1, t).unwrap();
        }
        let op = OpenProject::new(p.ghost_path.clone(), Ok(p));
        let mut h = Harness::new_ui_state(
            |ui, (op, actions, engaged): &mut State| {
                let settings = BoardSettings {
                    engaged: *engaged,
                    run_limit: 3,
                    archive_days: 7,
                };
                show(ui, op, settings, actions)
            },
            (op, Vec::new(), false),
        );
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
    fn play_becomes_add_to_queue_while_the_ai_is_engaged() {
        let d = tempfile::tempdir().unwrap();
        let mut h = harness(d.path());
        h.state_mut().2 = true;
        h.run();
        assert_eq!(h.query_all_by_label(icon::PLAY).count(), 0);
        h.get_by_label(icon::LIST_PLUS).click();
        h.run();
        assert!(h.state().1.iter().any(|a| matches!(a, Action::RunAi(1))));
        h.state_mut().2 = false;
        h.run();
        assert_eq!(h.get_all_by_label(icon::PLAY).count(), 1);
    }

    #[test]
    fn ai_working_columns_have_no_play_button() {
        let d = tempfile::tempdir().unwrap();
        let h = harness_cols(d.path(), &["Hello"], AutoMove::AiWorking);
        assert_eq!(h.get_all_by_label(icon::PLAY).count(), 1);
        let d = tempfile::tempdir().unwrap();
        let h = harness_cols(d.path(), &["Hello"], AutoMove::AiComplete);
        assert_eq!(h.get_all_by_label(icon::PLAY).count(), 2);
    }

    #[test]
    fn auto_switch_only_on_columns_that_hand_tickets_to_the_ai() {
        let d = tempfile::tempdir().unwrap();
        let mut h = harness_cols(d.path(), &["Hello"], AutoMove::AiWorking);
        assert_eq!(h.get_all_by_label("Auto").count(), 1);
        h.get_by_label("Auto").click();
        h.run();
        assert!(h.state().0.auto.contains("to_do"));
        // The switch itself toggles as well as its label.
        h.get_by_role(egui::accesskit::Role::CheckBox).click();
        h.run();
        assert!(h.state().0.auto.is_empty());
        let d = tempfile::tempdir().unwrap();
        let h = harness_cols(d.path(), &["Hello"], AutoMove::AiComplete);
        assert_eq!(h.query_all_by_label("Auto").count(), 0);
    }

    #[test]
    fn tickets_at_the_run_limit_are_flagged_in_auto_columns() {
        let d = tempfile::tempdir().unwrap();
        let mut h = harness_cols(d.path(), &["Fresh", "Tired"], AutoMove::AiWorking);
        assert_eq!(h.query_all_by_label(icon::FLAG).count(), 0);
        let p = h.state_mut().0.project.as_mut().unwrap();
        let mut t = p.tickets[0].iter().find(|t| t.title == "Tired").unwrap().clone();
        t.run_count = 3;
        t.save().unwrap();
        p.rescan().unwrap();
        h.run();
        assert_eq!(h.get_all_by_label(icon::FLAG).count(), 1);
        h.get_by_label("Tired").click_secondary();
        h.run();
        h.get_by_label_contains("Reset Run Count").click();
        h.run();
        assert!(h.state().1.iter().any(|a| matches!(a, Action::ResetRunCount(2))));
    }

    #[test]
    fn blocked_cards_show_a_lock_until_the_blocker_is_done() {
        let d = tempfile::tempdir().unwrap();
        let mut h = harness_with(d.path(), &["First", "Second"]);
        assert_eq!(h.query_all_by_label(icon::LOCK).count(), 0);
        let p = h.state_mut().0.project.as_mut().unwrap();
        let mut t = p.find_ticket(2).unwrap().1.clone();
        t.blocked_by = vec![1];
        t.save().unwrap();
        p.rescan().unwrap();
        h.run();
        assert_eq!(h.get_all_by_label(icon::LOCK).count(), 1);
        let p = h.state_mut().0.project.as_mut().unwrap();
        let first = p.find_ticket(1).unwrap().1.path.clone();
        p.move_ticket(&first, 1).unwrap();
        h.run();
        assert_eq!(h.query_all_by_label(icon::LOCK).count(), 0);
    }

    #[test]
    fn old_done_tickets_hide_until_asked_for() {
        let d = tempfile::tempdir().unwrap();
        let mut h = harness_with(d.path(), &["Old", "New"]);
        let p = h.state_mut().0.project.as_mut().unwrap();
        for (id, days_ago) in [(1, 30), (2, 1)] {
            let mut t = p.find_ticket(id).unwrap().1.clone();
            t.done_at = Some(crate::ticket::now_secs() - days_ago * 86_400);
            t.save().unwrap();
            p.move_ticket(&t.path, 1).unwrap();
        }
        h.run();
        assert!(h.query_by_label("Old").is_none());
        assert!(h.query_by_label("New").is_some());
        h.get_by_label("Show 1 archived").click();
        h.run();
        assert!(h.query_by_label("Old").is_some());
        h.get_by_label("Hide archived").click();
        h.run();
        assert!(h.query_by_label("Old").is_none());
    }

    #[test]
    fn ctrl_and_shift_click_select_cards_for_bulk_actions() {
        let d = tempfile::tempdir().unwrap();
        let mut h = harness_with(d.path(), &["One", "Two", "Three", "Four"]);
        h.get_by_label("One").click_modifiers(egui::Modifiers::COMMAND);
        h.run();
        h.get_by_label("Three").click_modifiers(egui::Modifiers::SHIFT);
        h.run();
        assert_eq!(h.state().0.selected, HashSet::from([1, 2, 3]));
        assert!(h.state().1.is_empty(), "selecting opens nothing");
        h.get_by_label("3 selected");
        h.get_by_label_contains("Send to AI").click();
        h.run();
        assert!(
            h.state()
                .1
                .iter()
                .any(|a| matches!(a, Action::QueueTickets(ids) if ids == &[1, 2, 3]))
        );
        h.get_by_label_contains("Delete").click();
        h.run();
        assert!(
            h.state()
                .1
                .iter()
                .any(|a| matches!(a, Action::AskDelete(ids) if ids == &[1, 2, 3]))
        );
        // Ctrl-click again takes a card out; a plain click opens one and drops the selection.
        h.get_by_label("Two").click_modifiers(egui::Modifiers::COMMAND);
        h.run();
        assert_eq!(h.state().0.selected, HashSet::from([1, 3]));
        h.get_by_label("Four").click();
        h.run();
        assert!(h.state().0.selected.is_empty());
        assert!(h.state().1.iter().any(|a| matches!(a, Action::ViewTicket(4))));
        assert!(h.query_by_label_contains("selected").is_none());
    }

    #[test]
    fn dragging_a_selected_card_moves_the_whole_selection() {
        let d = tempfile::tempdir().unwrap();
        let mut h = harness_with(d.path(), &["One", "Two", "Three"]);
        h.state_mut().0.selected = HashSet::from([1, 3]);
        h.run();
        let from = h.get_by_label("Three").rect().center();
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
                .any(|a| matches!(a, Action::MoveTickets { ids, to_col: 1 } if ids == &[1, 3]))
        );
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
                .any(|a| matches!(a, Action::MoveTickets { ids, to_col: 1 } if ids == &[1]))
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

    #[test]
    fn ticket_number_sits_close_to_the_title() {
        let d = tempfile::tempdir().unwrap();
        let h = harness(d.path());
        let gap = h.get_by_label("Hello").rect().top() - h.get_by_label("00001").rect().bottom();
        assert!((0.0..=6.0).contains(&gap), "gap between number and title: {gap}");
    }
}
