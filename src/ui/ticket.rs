use egui::text::{CCursor, CCursorRange};
use egui::{
    Align, ComboBox, DragValue, Id, Key, KeyboardShortcut, Layout, Modifiers, RichText, ScrollArea, TextEdit, Ui, vec2,
};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use egui_phosphor::regular as icon;

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
        CommonMarkViewer::new().show(ui, cache, text);
    }
}

pub enum ViewOutcome {
    Close,
    Edit,
}

pub fn view(ctx: &egui::Context, t: &Ticket, cache: &mut CommonMarkCache) -> Option<ViewOutcome> {
    let mut outcome = None;
    let size = modal_size(ctx);
    let modal = super::modal(ctx, "view_ticket").show(ctx, |ui| {
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
            ui.label(RichText::new("Description").size(17.0).strong().color(theme::ACCENT));
            ui.add_space(4.0);
            markdown(ui, cache, &t.description);
            if !t.test_scope.trim().is_empty() {
                ui.add_space(16.0);
                ui.label(RichText::new("Test Scope").size(17.0).strong().color(theme::ACCENT));
                ui.add_space(4.0);
                markdown(ui, cache, &t.test_scope);
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

pub struct TicketForm {
    pub mode: FormMode,
    pub ticket: Ticket,
    original: Ticket,
    confirm_discard: bool,
    pub error: Option<String>,
}

pub enum FormOutcome {
    Close,
    Save,
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
        TicketForm {
            mode,
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
            || a.description != b.description
            || a.test_scope != b.test_scope
    }

    pub fn show(&mut self, ctx: &egui::Context, cache: &mut CommonMarkCache) -> Option<FormOutcome> {
        let mut outcome = None;
        let mut close = false;
        let size = modal_size(ctx);
        let modal = super::modal(ctx, "edit_ticket").show(ctx, |ui| {
            ui.set_width(size.x);
            ui.set_height(size.y);
            let heading = match self.mode {
                FormMode::Edit => "Edit Ticket",
                FormMode::Create { .. } => "New Ticket",
            };
            ui.label(RichText::new(heading).size(20.0).strong());
            ui.add_space(8.0);

            let t = &mut self.ticket;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(t.id_str())
                        .monospace()
                        .size(16.0)
                        .color(theme::TEXT_MUTED),
                );
                let title = ui.add(
                    TextEdit::singleline(&mut t.title)
                        .margin(super::INPUT_MARGIN)
                        .hint_text("Title (required)")
                        .font(egui::TextStyle::Heading)
                        .desired_width(f32::INFINITY),
                );
                if self.original.title.is_empty() && ui.memory(|m| m.focused().is_none()) {
                    title.request_focus();
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Priority");
                let combo = ComboBox::from_id_salt("priority")
                    .selected_text(RichText::new(t.priority.label()))
                    .show_ui(ui, |ui| {
                        for p in Priority::ALL {
                            let text = RichText::new(format!("{}  {}", priority_icon(p).text(), p.label()))
                                .color(super::priority_color(p));
                            ui.selectable_value(&mut t.priority, p, text);
                        }
                    });
                super::pointer(combo.response);
                ui.add_space(16.0);
                ui.label("Runs");
                ui.add(DragValue::new(&mut t.run_count).range(0..=u32::MAX).speed(0.1));
            });
            ui.add_space(8.0);

            let footer = 48.0;
            let body_h = (ui.available_height() - footer).max(200.0);
            ScrollArea::vertical()
                .max_height(body_h)
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.label(RichText::new("Description").strong());
                    markdown_editor(ui, "description", &mut t.description, (body_h * 0.6).max(160.0), cache);
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Test Scope").strong());
                        ui.label(RichText::new("optional").small().color(theme::TEXT_MUTED));
                    });
                    markdown_editor(ui, "test_scope", &mut t.test_scope, (body_h * 0.35).max(120.0), cache);
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
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        });
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

/// Small yes/no modal. Returns Some(true) on confirm, Some(false) on cancel.
pub fn confirm(ctx: &egui::Context, id: &str, message: &str, cancel: &str, ok: &str) -> Option<bool> {
    let mut result = None;
    let danger = ok == "Delete" || ok == "Discard";
    let modal = super::modal(ctx, ("confirm", id)).show(ctx, |ui| {
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

enum Format {
    Wrap(&'static str, &'static str),
    LinePrefix(&'static str),
}

const TOOLBAR: [(&str, &str, Format); 10] = [
    (icon::TEXT_B, "Bold (Ctrl+B)", Format::Wrap("**", "**")),
    (icon::TEXT_ITALIC, "Italic (Ctrl+I)", Format::Wrap("*", "*")),
    (icon::TEXT_H, "Heading", Format::LinePrefix("## ")),
    (icon::LIST_BULLETS, "Bulleted list", Format::LinePrefix("- ")),
    (icon::LIST_NUMBERS, "Numbered list", Format::LinePrefix("1. ")),
    (icon::CHECK_SQUARE, "Checklist", Format::LinePrefix("- [ ] ")),
    (icon::CODE, "Inline code", Format::Wrap("`", "`")),
    (icon::CODE_BLOCK, "Code block", Format::Wrap("\n```\n", "\n```\n")),
    (icon::LINK, "Link", Format::Wrap("[", "](https://)")),
    (icon::QUOTES, "Quote", Format::LinePrefix("> ")),
];

/// Toolbar + markdown source + live preview. Side by side when wide, stacked when narrow.
fn markdown_editor(ui: &mut Ui, id: &str, text: &mut String, height: f32, cache: &mut CommonMarkCache) {
    let edit_id = ui.make_persistent_id(("md_editor", id));
    let mut format = None;
    ui.horizontal(|ui| {
        for (i, (glyph, tip, _)) in TOOLBAR.iter().enumerate() {
            if icon_button(ui, glyph, tip).clicked() {
                format = Some(i);
            }
        }
    });
    if ui.memory(|m| m.has_focus(edit_id)) {
        ui.input_mut(|i| {
            if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::B)) {
                format = Some(0);
            } else if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::I)) {
                format = Some(1);
            }
        });
    }
    if let Some(i) = format {
        apply_format(ui.ctx(), edit_id, text, &TOOLBAR[i].2);
    }

    let wide = ui.available_width() > 720.0;
    let editor = |ui: &mut Ui, text: &mut String, w: f32| {
        ScrollArea::vertical()
            .id_salt((id, "src"))
            .max_height(height)
            .show(ui, |ui| {
                ui.add_sized(
                    vec2(w, height),
                    TextEdit::multiline(text)
                        .margin(super::INPUT_MARGIN)
                        .id(edit_id)
                        .code_editor()
                        .desired_width(w)
                        .hint_text("Write markdown…"),
                );
            });
    };
    let preview = |ui: &mut Ui, text: &str, w: f32, cache: &mut CommonMarkCache| {
        egui::Frame::new()
            .fill(theme::BG)
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(w - 16.0);
                ui.set_min_height(height - 16.0);
                ScrollArea::vertical()
                    .id_salt((id, "preview"))
                    .max_height(height - 16.0)
                    .min_scrolled_height(height - 16.0)
                    .show(ui, |ui| {
                        ui.set_width(w - 16.0);
                        markdown(ui, cache, text);
                    });
            });
    };
    if wide {
        let w = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
        ui.horizontal_top(|ui| {
            editor(ui, text, w);
            preview(ui, text, w, cache);
        });
    } else {
        let w = ui.available_width();
        editor(ui, text, w);
        preview(ui, text, w, cache);
    }
}

fn apply_format(ctx: &egui::Context, edit_id: Id, text: &mut String, format: &Format) {
    let mut state = TextEdit::load_state(ctx, edit_id).unwrap_or_default();
    let len = text.chars().count();
    let (start, end) = state
        .cursor
        .char_range()
        .map(|r| {
            let r = r.as_sorted_char_range();
            (r.start.0.min(len), r.end.0.min(len))
        })
        .unwrap_or((len, len));
    let (new_text, sel) = match format {
        Format::Wrap(pre, suf) => wrap(text, start, end, pre, suf),
        Format::LinePrefix(prefix) => prefix_lines(text, start, end, prefix),
    };
    *text = new_text;
    state
        .cursor
        .set_char_range(Some(CCursorRange::two(CCursor::new(sel.0), CCursor::new(sel.1))));
    state.store(ctx, edit_id);
    ctx.memory_mut(|m| m.request_focus(edit_id));
}

fn byte_at(text: &str, char_idx: usize) -> usize {
    text.char_indices().nth(char_idx).map_or(text.len(), |(b, _)| b)
}

/// Wraps chars `start..end` in `pre`/`suf`; returns new text and the selection inside the markers.
fn wrap(text: &str, start: usize, end: usize, pre: &str, suf: &str) -> (String, (usize, usize)) {
    let (bs, be) = (byte_at(text, start), byte_at(text, end));
    let out = format!("{}{pre}{}{suf}{}", &text[..bs], &text[bs..be], &text[be..]);
    let inner = start + pre.chars().count();
    (out, (inner, inner + (end - start)))
}

/// Adds `prefix` to every line touched by chars `start..end`; selects the changed lines.
fn prefix_lines(text: &str, start: usize, end: usize, prefix: &str) -> (String, (usize, usize)) {
    let (bs, be) = (byte_at(text, start), byte_at(text, end));
    let line_start = text[..bs].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[be..].find('\n').map_or(text.len(), |i| be + i);
    let block: Vec<String> = text[line_start..line_end]
        .split('\n')
        .map(|l| format!("{prefix}{l}"))
        .collect();
    let block = block.join("\n");
    let out = format!("{}{}{}", &text[..line_start], block, &text[line_end..]);
    let sel_start = text[..line_start].chars().count();
    (out, (sel_start, sel_start + block.chars().count()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_selection_and_empty() {
        assert_eq!(wrap("make bold", 5, 9, "**", "**"), ("make **bold**".into(), (7, 11)));
        assert_eq!(wrap("ab", 1, 1, "`", "`"), ("a``b".into(), (2, 2)));
        assert_eq!(wrap("héllo", 1, 2, "*", "*"), ("h*é*llo".into(), (2, 3)));
    }

    #[test]
    fn toolbar_uses_stored_selection() {
        let ctx = egui::Context::default();
        let id = Id::new("editor");
        let mut state = egui::text_edit::TextEditState::default();
        state
            .cursor
            .set_char_range(Some(CCursorRange::two(CCursor::new(9), CCursor::new(5))));
        state.store(&ctx, id);
        let mut text = "make bold".to_string();
        apply_format(&ctx, id, &mut text, &Format::Wrap("**", "**"));
        assert_eq!(text, "make **bold**");
        let sel = TextEdit::load_state(&ctx, id)
            .unwrap()
            .cursor
            .char_range()
            .unwrap()
            .as_sorted_char_range();
        assert_eq!((sel.start.0, sel.end.0), (7, 11));

        // No stored selection: append at the end.
        let mut text = "x".to_string();
        apply_format(&ctx, Id::new("fresh"), &mut text, &Format::LinePrefix("- "));
        assert_eq!(text, "- x");
    }

    #[test]
    fn prefix_touched_lines() {
        let (out, sel) = prefix_lines("one\ntwo\nthree", 5, 9, "- ");
        assert_eq!(out, "one\n- two\n- three");
        assert_eq!(sel, (4, 17));
        assert_eq!(prefix_lines("", 0, 0, "## ").0, "## ");
        assert_eq!(prefix_lines("a\nb", 3, 3, "> ").0, "a\n> b");
    }
}
