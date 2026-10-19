use egui::{CornerRadius, Frame, Margin, RichText, Sense, Ui, vec2};

use crate::project::{STATS_DAYS, Stats};
use crate::theme;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// How the project is going: headline numbers, tickets done per day and the tickets that needed the most runs.
/// True once closed.
pub fn show(ctx: &egui::Context, name: &str, s: &Stats, today: u64) -> bool {
    let modal = super::show_modal(ctx, "stats", |ui| {
        ui.set_width(640.0);
        let close = super::modal_header(ui, &format!("Stats - {name}"), "Close");
        let done: u32 = s.done_per_day.iter().sum();
        let (passed, worked) = s.first_try;
        ui.horizontal(|ui| {
            tile(ui, &done.to_string(), &format!("done in {STATS_DAYS} days"));
            tile(ui, &s.runs.to_string(), "AI runs");
            let avg = s.avg_run_secs.map_or("–".into(), super::ticket::duration);
            tile(ui, &avg, "average run");
            let rate = (passed * 100)
                .checked_div(worked)
                .map_or("–".into(), |pct| format!("{pct}%"));
            tile(ui, &rate, &format!("passed first try ({passed} of {worked})"));
        });
        ui.add_space(6.0);
        ui.label(RichText::new("Done per day").strong());
        bars(ui, &s.done_per_day, today);
        ui.add_space(6.0);
        ui.label(RichText::new("Most runs").strong());
        if s.most_retried.is_empty() {
            ui.label(RichText::new("No ticket has needed more than one run.").color(theme::TEXT_MUTED));
        }
        egui::Grid::new("most_retried").spacing([16.0, 4.0]).show(ui, |ui| {
            for (id, title, runs) in &s.most_retried {
                ui.label(
                    RichText::new(crate::ticket::format_id(*id))
                        .monospace()
                        .color(theme::TEXT_MUTED),
                );
                ui.add(egui::Label::new(title).truncate());
                ui.label(RichText::new(format!("{runs} runs")).color(theme::TEXT_MUTED));
                ui.end_row();
            }
        });
        close
    });
    modal.inner || modal.should_close()
}

/// A headline number with what it counts underneath.
fn tile(ui: &mut Ui, value: &str, label: &str) {
    Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(124.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(value).size(22.0).strong());
            ui.add(egui::Label::new(RichText::new(label).small().color(theme::TEXT_MUTED)).wrap());
        });
}

/// One bar per day, today on the right; hover a day for its count.
fn bars(ui: &mut Ui, counts: &[u32; STATS_DAYS], today: u64) {
    const GAP: f32 = 2.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 120.0), Sense::hover());
    let label_h = 16.0;
    let plot = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.bottom() - label_h));
    let max = counts.iter().copied().max().unwrap_or(0).max(1) as f32;
    let width = (plot.width() - GAP * (STATS_DAYS as f32 - 1.0)) / STATS_DAYS as f32;
    let painter = ui.painter();
    painter.hline(plot.x_range(), plot.bottom(), egui::Stroke::new(1.0, theme::BORDER));
    for (i, &n) in counts.iter().enumerate() {
        let x = plot.left() + i as f32 * (width + GAP);
        let column = egui::Rect::from_min_size(egui::pos2(x, plot.top()), vec2(width, plot.height()));
        if n > 0 {
            let top = plot.bottom() - plot.height() * n as f32 / max;
            let bar = egui::Rect::from_min_max(egui::pos2(x, top), egui::pos2(x + width, plot.bottom()));
            let round = CornerRadius {
                nw: 4,
                ne: 4,
                sw: 0,
                se: 0,
            };
            painter.rect_filled(bar, round, theme::ACCENT);
        }
        let day = today - (STATS_DAYS - 1 - i) as u64;
        let (month, date) = month_day(day);
        painter.text(
            egui::pos2(column.center().x, rect.bottom()),
            egui::Align2::CENTER_BOTTOM,
            date.to_string(),
            egui::FontId::proportional(11.0),
            theme::TEXT_MUTED,
        );
        let tip = format!("{month} {date}: {n} done");
        ui.interact(column, ui.id().with(("stats_day", i)), Sense::hover())
            .on_hover_text(tip);
    }
}

/// Month name and day of the month of day `day` since 1970 (UTC).
fn month_day(day: u64) -> (&'static str, u32) {
    let stamp = crate::ticket::utc_timestamp(day * 86_400);
    let month: usize = stamp[5..7].parse().unwrap_or(1);
    (MONTHS[month.clamp(1, 12) - 1], stamp[8..10].parse().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_labels() {
        assert_eq!(month_day(0), ("Jan", 1));
        assert_eq!(month_day(19_740), ("Jan", 18)); // 2024-01-18
    }
}
