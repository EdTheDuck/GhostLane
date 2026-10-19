use egui::{Align, Label, Layout, RichText, ScrollArea, Ui};
use egui_dnd::dnd;
use egui_phosphor::regular as icon;

use super::danger_icon_button;
use crate::project::Project;
use crate::theme;
use crate::ticket::format_id;

/// Tickets waiting for the AI, in run order: drag to reorder, ✕ to drop one.
pub fn show(ui: &mut Ui, queue: &mut Vec<u32>, project: Option<&Project>) {
    ui.label(RichText::new(format!("{}  Queue", icon::QUEUE)).strong());
    if queue.is_empty() {
        ui.label(
            RichText::new("Press ▶ on a ticket while the AI is busy to queue it.")
                .small()
                .color(theme::TEXT_MUTED),
        );
        return;
    }
    let mut remove = None;
    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        dnd(ui, "ai_queue").show_vec(queue, |ui, id, handle, state| {
            ui.horizontal(|ui| {
                handle.ui(ui, super::drag_handle);
                ui.label(RichText::new(format_id(*id)).monospace().color(theme::TEXT_MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if danger_icon_button(ui, icon::X, "Remove from queue").clicked() {
                        remove = Some(state.index);
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        match project.and_then(|p| p.find_ticket(*id)) {
                            Some((_, t)) => ui.add(Label::new(&t.title).truncate()),
                            None => ui.label(RichText::new("Missing ticket").color(theme::TEXT_MUTED)),
                        };
                    });
                });
            });
        });
    });
    if let Some(i) = remove {
        queue.remove(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn cross_removes_a_ticket_from_the_queue() {
        let mut h = Harness::new_ui_state(|ui, q: &mut Vec<u32>| show(ui, q, None), vec![1, 2, 3]);
        theme::apply(&h.ctx);
        h.run();
        h.get_all_by_label(icon::X).nth(1).unwrap().click();
        h.run();
        assert_eq!(h.state(), &[1, 3]);
    }
}
