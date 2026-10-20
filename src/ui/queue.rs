use egui::{Align, ComboBox, Label, Layout, RichText, ScrollArea, Ui};
use egui_dnd::dnd;
use egui_phosphor::regular as icon;

use super::{danger_icon_button, glyph_button, switch_row};
use crate::config::{Schedule, clock_time};
use crate::project::Project;
use crate::theme;
use crate::ticket::{compact, format_id};

/// The queue's own switches, beside the tickets in it.
pub struct Controls<'a> {
    /// Work through the queue by itself.
    pub auto: &'a mut bool,
    /// When it may do that.
    pub schedule: &'a mut Schedule,
    /// Where the schedule is at, in a line, while it is on.
    pub status: Option<String>,
}

/// Token budgets offered for a scheduled window; 0 = no limit.
const BUDGETS: [u64; 6] = [0, 500_000, 1_000_000, 2_000_000, 5_000_000, 10_000_000];
const DAYS: [(&str, &str); 7] = [
    ("M", "Monday"),
    ("T", "Tuesday"),
    ("W", "Wednesday"),
    ("T", "Thursday"),
    ("F", "Friday"),
    ("S", "Saturday"),
    ("S", "Sunday"),
];

/// Tickets waiting for the AI, in run order: drag to reorder, ✕ to drop one. Above them, the Auto switch and the
/// clock that opens the schedule in place of the list. True when the schedule changed.
pub fn show(ui: &mut Ui, queue: &mut Vec<u32>, project: Option<&Project>, c: Controls) -> bool {
    let editing_id = ui.id().with("editing_schedule");
    let mut editing = ui.data(|d| d.get_temp(editing_id)).unwrap_or(false);
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{}  Queue", icon::QUEUE)).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let idle = if c.schedule.enabled { theme::ACCENT } else { theme::TEXT };
            if glyph_button(ui, icon::CLOCK, idle, theme::ACCENT, false)
                .on_hover_text("Schedule: work through the queue only at set times")
                .clicked()
            {
                editing = !editing;
            }
            switch_row(ui, c.auto, "Auto").on_hover_text(
                "Work through the queue by itself. Switched off, tickets wait here until you switch it back on.",
            );
        });
    });
    ui.data_mut(|d| d.insert_temp(editing_id, editing));
    if editing {
        let changed = schedule_ui(ui, c.schedule);
        if ui.button("Done").clicked() {
            ui.data_mut(|d| d.insert_temp(editing_id, false));
        }
        return changed;
    }
    if let Some(status) = c.status {
        ui.label(RichText::new(status).small().color(theme::TEXT_MUTED));
    }
    if queue.is_empty() {
        ui.label(
            RichText::new("Press ▶ on a ticket while the AI is busy, or right-click one and choose Add to Queue.")
                .small()
                .color(theme::TEXT_MUTED),
        );
        return false;
    }
    let mut remove = None;
    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        dnd(ui, "ai_queue").show_vec(queue, |ui, id, handle, state| {
            ui.horizontal(|ui| {
                handle.ui(ui, super::drag_handle);
                ui.label(RichText::new(format_id(*id)).monospace().color(theme::TEXT_MUTED));
                if project.is_some_and(|p| p.is_blocked(*id)) {
                    ui.label(RichText::new(icon::LOCK).color(theme::TEXT_MUTED))
                        .on_hover_text("Waits until the tickets it's blocked by are done");
                }
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
    false
}

/// What a scheduled queue window got done.
pub struct Summary {
    /// Project, when, and the tokens used, in a line.
    pub title: String,
    /// Ticket number, title, how its run ended, and whether that counts as failing.
    pub runs: Vec<(u32, String, String, bool)>,
}

impl Summary {
    pub fn markdown(&self) -> String {
        let mut out = format!("## Queue run: {}\n\n", self.title);
        for (id, title, outcome, _) in &self.runs {
            out.push_str(&format!("- {} {title}: {outcome}\n", format_id(*id)));
        }
        out
    }
}

/// The summary as a dialog. True once it's closed.
pub fn summary(ctx: &egui::Context, s: &Summary) -> bool {
    let mut close = false;
    let modal = super::show_modal(ctx, "queue_summary", |ui| {
        ui.set_width(600.0);
        if super::modal_header(ui, "Queue run finished", "Close") {
            close = true;
        }
        ui.label(RichText::new(&s.title).color(theme::TEXT_MUTED));
        egui::Grid::new("queue_summary_runs")
            .num_columns(3)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                for (id, title, outcome, failed) in &s.runs {
                    ui.label(RichText::new(format_id(*id)).monospace().color(theme::TEXT_MUTED));
                    ui.label(title);
                    let color = if *failed { theme::DANGER } else { theme::ACCENT };
                    ui.label(RichText::new(outcome).color(color));
                    ui.end_row();
                }
            });
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Copy as Markdown").clicked() {
                ui.ctx().copy_text(s.markdown());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if super::accent_button(ui, "OK").clicked() {
                    close = true;
                }
            });
        });
    });
    close || modal.should_close()
}

/// The schedule's settings. True when one changed.
fn schedule_ui(ui: &mut Ui, s: &mut Schedule) -> bool {
    let before = s.clone();
    switch_row(ui, &mut s.enabled, "Only work through the queue at set times");
    ui.add_enabled_ui(s.enabled, |ui| {
        ui.horizontal(|ui| {
            ui.label("From");
            time_combo(ui, "schedule_start", &mut s.start);
            ui.label("to");
            time_combo(ui, "schedule_end", &mut s.end);
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (on, (letter, day)) in s.days.iter_mut().zip(DAYS) {
                if super::pointer(ui.selectable_label(*on, letter))
                    .on_hover_text(day)
                    .clicked()
                {
                    *on = !*on;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Stop after");
            let label = |n: u64| match n {
                0 => "No token limit".to_string(),
                n => format!("{} tokens", compact(n)),
            };
            let combo = ComboBox::from_id_salt("schedule_budget")
                .selected_text(label(s.token_budget))
                .show_ui(ui, |ui| {
                    for n in BUDGETS {
                        ui.selectable_value(&mut s.token_budget, n, label(n));
                    }
                });
            super::pointer(combo.response);
        });
        ui.label(
            RichText::new(
                "GhostLane has to be open and the computer awake. A ticket still going when time is up finishes.",
            )
            .small()
            .color(theme::TEXT_MUTED),
        );
    });
    *s != before
}

/// A time of day in half hours.
fn time_combo(ui: &mut Ui, id: &str, minute: &mut u32) {
    let combo = ComboBox::from_id_salt(id)
        .selected_text(clock_time(*minute))
        .height(240.0)
        .show_ui(ui, |ui| {
            for m in (0..48).map(|n| n * 30) {
                ui.selectable_value(minute, m, clock_time(m));
            }
        });
    super::pointer(combo.response);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    type State = (Vec<u32>, bool, Schedule);

    fn harness(queue: Vec<u32>) -> Harness<'static, State> {
        let mut h = Harness::new_ui_state(
            |ui, (q, auto, schedule): &mut State| {
                let c = Controls {
                    auto,
                    schedule,
                    status: None,
                };
                show(ui, q, None, c);
            },
            (queue, true, Schedule::default()),
        );
        theme::apply(&h.ctx);
        h.set_size(egui::vec2(400.0, 300.0));
        h.run();
        h
    }

    #[test]
    fn summaries_copy_as_markdown() {
        let s = Summary {
            title: "P · 22:00–06:41 · 1.8M tokens".into(),
            runs: vec![(12, "Fix it".into(), "moved on to Done".into(), false)],
        };
        assert_eq!(
            s.markdown(),
            "## Queue run: P · 22:00–06:41 · 1.8M tokens\n\n- 00012 Fix it: moved on to Done\n"
        );
    }

    #[test]
    fn cross_removes_a_ticket_from_the_queue() {
        let mut h = harness(vec![1, 2, 3]);
        h.get_all_by_label(icon::X).nth(1).unwrap().click();
        h.run();
        assert_eq!(h.state().0, [1, 3]);
    }

    #[test]
    fn auto_switch_and_schedule_sit_above_the_queue() {
        let mut h = harness(vec![1]);
        h.get_by_label("Auto").click();
        h.run();
        assert!(!h.state().1);
        h.get_by_label(icon::CLOCK).click();
        h.run();
        h.get_by_label("Only work through the queue at set times").click();
        h.run();
        assert!(h.state().2.enabled);
        h.get_by_label("Done").click();
        h.run();
        assert!(h.query_by_label("Only work through the queue at set times").is_none());
    }
}
