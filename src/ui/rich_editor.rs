//! Word-style editor for ticket text. Each block (paragraph, heading, list item, quote, code)
//! is its own text field showing formatted text; Markdown is only produced on save.

use std::borrow::Cow;

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{
    Color32, CornerRadius, FontFamily, FontId, Frame, Id, Key, KeyboardShortcut, Margin, Modifiers, RichText,
    ScrollArea, Sense, Stroke, TextBuffer, TextEdit, Ui, vec2,
};
use egui_phosphor::regular as icon;

use super::{glyph_button, icon_button};
use crate::richtext::{self, Block, Flag, Kind, Style, adjust_styles};
use crate::theme;

const GUTTER: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Caret {
    Start,
    End,
    At(usize),
}

/// Structural edits found while drawing, applied once the frame's blocks are drawn.
#[derive(Debug, PartialEq)]
enum Op {
    /// Enter pressed: split block at char.
    Split(usize, usize),
    /// Backspace at the start of a block.
    BackspaceAtStart(usize),
    /// Delete at the end of a block.
    DeleteAtEnd(usize),
    Focus(usize, Caret),
}

pub struct RichEditor {
    id: Id,
    blocks: Vec<Block>,
    uids: Vec<u64>,
    next_uid: u64,
    /// Markdown the visual editor can't represent; edited as plain text instead so nothing is lost.
    raw: Option<String>,
    initial: String,
    /// Block to focus next frame, with the selection to restore.
    focus: Option<(usize, CCursorRange)>,
    /// Block that last had keyboard focus (toolbar actions apply to it).
    active: Option<usize>,
    /// Style toggled with nothing selected; applies to the next typed text.
    pending: Option<Style>,
}

impl RichEditor {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, markdown: &str) -> Self {
        let (blocks, raw) = match richtext::parse(markdown) {
            Some(blocks) => (blocks, None),
            None => (Vec::new(), Some(markdown.to_string())),
        };
        let mut editor = RichEditor {
            id: Id::new(id),
            blocks: Vec::new(),
            uids: Vec::new(),
            next_uid: 0,
            raw,
            initial: String::new(),
            focus: None,
            active: None,
            pending: None,
        };
        editor.set_blocks(blocks);
        editor.initial = editor.markdown();
        editor
    }

    fn set_blocks(&mut self, mut blocks: Vec<Block>) {
        if blocks.is_empty() {
            blocks.push(Block::new(Kind::Paragraph));
        }
        self.uids = (0..blocks.len() as u64).collect();
        self.next_uid = blocks.len() as u64;
        self.blocks = blocks;
    }

    pub fn markdown(&self) -> String {
        match &self.raw {
            Some(raw) => raw.trim().to_string(),
            None => richtext::to_markdown(&self.blocks),
        }
    }

    pub fn changed(&self) -> bool {
        self.markdown() != self.initial
    }

    fn insert_block(&mut self, at: usize, block: Block) {
        self.blocks.insert(at, block);
        self.uids.insert(at, self.next_uid);
        self.next_uid += 1;
    }

    fn remove_block(&mut self, at: usize) -> Block {
        self.uids.remove(at);
        self.blocks.remove(at)
    }

    fn focus_at(&mut self, block: usize, caret: Caret) {
        let len = self.blocks[block].char_len();
        let at = match caret {
            Caret::Start => 0,
            Caret::End => len,
            Caret::At(i) => i.min(len),
        };
        self.focus = Some((block, CCursorRange::one(CCursor::new(at))));
    }

    fn apply(&mut self, op: Op) {
        match op {
            Op::Split(i, at) => {
                let b = &mut self.blocks[i];
                if b.text.trim().is_empty() && b.kind.is_list() {
                    // Enter on an empty list item leaves the list, like Word.
                    b.kind = Kind::Paragraph;
                    b.text.clear();
                    b.styles.clear();
                    return self.focus_at(i, Caret::Start);
                }
                let mut rest = b.split_off(at);
                rest.kind = match b.kind {
                    Kind::Check(_) => Kind::Check(false),
                    k @ (Kind::Bullet | Kind::Numbered | Kind::Quote) => k,
                    _ => Kind::Paragraph,
                };
                self.insert_block(i + 1, rest);
                self.focus_at(i + 1, Caret::Start);
            }
            Op::BackspaceAtStart(i) => {
                if self.blocks[i].kind != Kind::Paragraph {
                    self.blocks[i].kind = Kind::Paragraph;
                    self.focus_at(i, Caret::Start);
                } else if i > 0 {
                    let block = self.remove_block(i);
                    let at = self.blocks[i - 1].char_len();
                    self.blocks[i - 1].append(block);
                    self.focus_at(i - 1, Caret::At(at));
                }
            }
            Op::DeleteAtEnd(i) => {
                if i + 1 < self.blocks.len() {
                    let next = self.remove_block(i + 1);
                    let at = self.blocks[i].char_len();
                    self.blocks[i].append(next);
                    self.focus_at(i, Caret::At(at));
                }
            }
            Op::Focus(i, caret) => self.focus_at(i, caret),
        }
    }

    /// Typing "- ", "1. ", "# ", "> ", "[ ] " or "```" at the start of a paragraph turns it into that block.
    fn autoformat(block: &mut Block) {
        if block.kind != Kind::Paragraph {
            return;
        }
        let rules: [(&str, Kind); 11] = [
            ("- [ ] ", Kind::Check(false)),
            ("[ ] ", Kind::Check(false)),
            ("- [x] ", Kind::Check(true)),
            ("[x] ", Kind::Check(true)),
            ("- ", Kind::Bullet),
            ("* ", Kind::Bullet),
            ("1. ", Kind::Numbered),
            ("### ", Kind::Heading(3)),
            ("## ", Kind::Heading(2)),
            ("# ", Kind::Heading(1)),
            ("> ", Kind::Quote),
        ];
        if let Some((prefix, kind)) = rules.iter().find(|(p, _)| block.text.starts_with(p)) {
            block.strip_prefix_chars(prefix.chars().count());
            block.kind = *kind;
        } else if block.text.starts_with("```") {
            block.strip_prefix_chars(3);
            block.kind = Kind::Code;
            block.styles.iter_mut().for_each(|s| *s = Style::default());
        }
    }

    pub fn show(&mut self, ui: &mut Ui, height: f32) {
        if let Some(raw) = &mut self.raw {
            ui.label(
                RichText::new(
                    "This text uses formatting the visual editor can't show (such as tables or images), so it is edited as Markdown.",
                )
                .small()
                .color(theme::TEXT_MUTED),
            );
            ui.add_sized(
                vec2(ui.available_width(), height),
                TextEdit::multiline(raw).code_editor().margin(super::INPUT_MARGIN),
            );
            return;
        }
        self.toolbar(ui);
        let mut op = None;
        Frame::new()
            .fill(theme::BG)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_min_height(height);
                ScrollArea::vertical()
                    .id_salt(self.id.with("scroll"))
                    .max_height(height)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        let mut number = 0;
                        for i in 0..self.blocks.len() {
                            let kind = self.blocks[i].kind;
                            number = if kind == Kind::Numbered { number + 1 } else { 0 };
                            // Paragraph-style gap between blocks; list items and quote lines stay together.
                            if let Some(prev) = i.checked_sub(1).map(|p| self.blocks[p].kind) {
                                let list = prev.is_list() && kind.is_list();
                                let quote = prev == Kind::Quote && kind == Kind::Quote;
                                if !list && !quote {
                                    ui.add_space(6.0);
                                }
                            }
                            if let Some(found) = self.block_ui(ui, i, number) {
                                op = Some(found);
                            }
                        }
                        // Clicking the empty space below the text puts the caret at the end.
                        let rest = ui.allocate_response(
                            vec2(ui.available_width(), ui.available_height().max(24.0)),
                            Sense::click(),
                        );
                        if rest.clicked() {
                            op = Some(Op::Focus(self.blocks.len() - 1, Caret::End));
                        }
                    });
            });
        if let Some(op) = op {
            self.apply(op);
            ui.ctx().request_repaint();
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let active_kind = self.active.and_then(|i| self.blocks.get(i)).map(|b| b.kind);
        let mut inline = None;
        let mut kind = None;
        let mut link = false;
        ui.horizontal(|ui| {
            let btn = |ui: &mut Ui, glyph: &str, tip: &str, on: bool| {
                let idle = if on { theme::ACCENT } else { theme::TEXT };
                glyph_button(ui, glyph, idle, theme::ACCENT, false)
                    .on_hover_text(tip)
                    .clicked()
            };
            if btn(ui, icon::TEXT_B, "Bold (Ctrl+B)", false) {
                inline = Some(Flag::Bold);
            }
            if btn(ui, icon::TEXT_ITALIC, "Italic (Ctrl+I)", false) {
                inline = Some(Flag::Italic);
            }
            if btn(ui, icon::CODE, "Inline code", false) {
                inline = Some(Flag::Code);
            }
            if btn(ui, icon::LINK, "Link", false) {
                link = true;
            }
            ui.separator();
            for (glyph, tip, k) in [
                (icon::TEXT_H, "Heading", Kind::Heading(2)),
                (icon::LIST_BULLETS, "Bulleted list", Kind::Bullet),
                (icon::LIST_NUMBERS, "Numbered list", Kind::Numbered),
                (icon::CHECK_SQUARE, "Checklist", Kind::Check(false)),
                (icon::QUOTES, "Quote", Kind::Quote),
                (icon::CODE_BLOCK, "Code block", Kind::Code),
            ] {
                let on = active_kind.is_some_and(|a| std::mem::discriminant(&a) == std::mem::discriminant(&k));
                if btn(ui, glyph, tip, on) {
                    kind = Some(k);
                }
            }
        });
        if let Some(flag) = inline {
            self.toggle_inline(ui.ctx(), flag);
        }
        if link {
            self.toggle_link(ui.ctx());
        }
        if let Some(k) = kind {
            self.toggle_kind(k);
        }
    }

    fn edit_id(&self, i: usize) -> Id {
        self.id.with(self.uids[i])
    }

    fn selection(&self, ctx: &egui::Context, i: usize) -> CCursorRange {
        TextEdit::load_state(ctx, self.edit_id(i))
            .and_then(|s| s.cursor.char_range())
            .unwrap_or(CCursorRange::one(CCursor::new(self.blocks[i].char_len())))
    }

    fn toggle_inline(&mut self, ctx: &egui::Context, flag: Flag) {
        let Some(i) = self.active.filter(|&i| i < self.blocks.len()) else {
            return;
        };
        let sel = self.selection(ctx, i);
        let range = sel.as_sorted_char_range();
        let (start, end) = (range.start.0, range.end.0);
        let block = &mut self.blocks[i];
        if start < end {
            block.toggle(start..end, flag);
        } else {
            let mut s = self
                .pending
                .clone()
                .or_else(|| start.checked_sub(1).and_then(|p| block.styles.get(p).cloned()))
                .unwrap_or_default();
            match flag {
                Flag::Bold => s.bold = !s.bold,
                Flag::Italic => s.italic = !s.italic,
                Flag::Code => s.code = !s.code,
            }
            self.pending = Some(s);
        }
        self.focus = Some((i, sel));
    }

    fn toggle_link(&mut self, ctx: &egui::Context) {
        let Some(i) = self.active.filter(|&i| i < self.blocks.len()) else {
            return;
        };
        let sel = self.selection(ctx, i);
        let range = sel.as_sorted_char_range();
        let (start, end) = (range.start.0, range.end.0);
        let block = &mut self.blocks[i];
        if start < end {
            if block.styles[start..end].iter().all(|s| s.link.is_some()) {
                block.set_link(start..end, None);
            } else {
                let text: String = block.text.chars().skip(start).take(end - start).collect();
                let url = if text.starts_with("http://") || text.starts_with("https://") {
                    text
                } else {
                    "https://".into()
                };
                block.set_link(start..end, Some(url));
            }
        } else if let Some((run, _)) = block.link_run(start) {
            block.set_link(run, None);
        }
        self.focus = Some((i, sel));
    }

    fn toggle_kind(&mut self, kind: Kind) {
        let Some(i) = self.active.filter(|&i| i < self.blocks.len()) else {
            return;
        };
        let block = &mut self.blocks[i];
        let same = std::mem::discriminant(&block.kind) == std::mem::discriminant(&kind);
        block.kind = if same { Kind::Paragraph } else { kind };
        if block.kind == Kind::Code {
            block.styles.iter_mut().for_each(|s| *s = Style::default());
        }
        let at = self.blocks[i].char_len();
        self.focus_at(i, Caret::At(at));
    }

    fn block_ui(&mut self, ui: &mut Ui, i: usize, number: usize) -> Option<Op> {
        let ctx = ui.ctx().clone();
        let edit_id = self.edit_id(i);
        if let Some((target, range)) = self.focus
            && target == i
        {
            let mut state = TextEdit::load_state(&ctx, edit_id).unwrap_or_default();
            state.cursor.set_char_range(Some(range));
            state.store(&ctx, edit_id);
            ctx.memory_mut(|m| m.request_focus(edit_id));
            self.focus = None;
        }
        let had_focus = ctx.memory(|m| m.has_focus(edit_id));
        if had_focus {
            let shortcut = ui.input_mut(|inp| {
                if inp.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::B)) {
                    Some(Flag::Bold)
                } else if inp.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::I)) {
                    Some(Flag::Italic)
                } else {
                    None
                }
            });
            if let Some(flag) = shortcut {
                self.active = Some(i);
                self.toggle_inline(&ctx, flag);
            }
        }
        let pre = TextEdit::load_state(&ctx, edit_id).and_then(|s| s.cursor.char_range());
        let (backspace, delete, up, down) = ui.input(|inp| {
            (
                inp.key_pressed(Key::Backspace),
                inp.key_pressed(Key::Delete),
                inp.key_pressed(Key::ArrowUp),
                inp.key_pressed(Key::ArrowDown),
            )
        });

        let block_count = self.blocks.len();
        let pending = if had_focus { self.pending.clone() } else { None };
        let block = &mut self.blocks[i];
        let kind = block.kind;
        let old_text = block.text.clone();
        let old_styles = block.styles.clone();
        let size = match kind {
            Kind::Heading(1) => 24.0,
            Kind::Heading(2) => 20.0,
            Kind::Heading(_) => 17.0,
            _ => 15.0,
        };
        let mut layouter = |ui: &Ui, buf: &dyn TextBuffer, wrap: f32| {
            let text = buf.as_str();
            let styles: Cow<'_, [Style]> = if text == old_text {
                Cow::Borrowed(&old_styles)
            } else {
                Cow::Owned(adjust_styles(&old_text, &old_styles, text, pending.as_ref()))
            };
            // Fonts set this frame only apply next frame; fall back to the regular font until then.
            let bold = ui.fonts(|f| {
                f.definitions()
                    .families
                    .contains_key(&FontFamily::Name(theme::BOLD.into()))
            });
            let mut job = layout(text, &styles, kind, size, bold);
            job.wrap.max_width = wrap;
            ui.fonts_mut(|f| f.layout_job(job))
        };

        let mut toggle_check = false;
        let show_hint = block_count == 1 && block.text.is_empty();
        let row = ui.horizontal_top(|ui| {
            gutter(ui, kind, number, size, &mut toggle_check);
            let mut edit = TextEdit::multiline(&mut block.text)
                .id(edit_id)
                .frame(Frame::NONE)
                .margin(Margin::symmetric(2, 3))
                .desired_rows(1)
                .desired_width(ui.available_width())
                .layouter(&mut layouter);
            if kind == Kind::Code {
                edit = edit.code_editor().lock_focus(true);
            }
            if show_hint {
                edit = edit.hint_text("Start typing…");
            }
            let content = |ui: &mut Ui| edit.show(ui);
            if kind == Kind::Code {
                Frame::new()
                    .fill(theme::CARD)
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(Margin::same(6))
                    .show(ui, content)
                    .inner
            } else {
                content(ui)
            }
        });
        let out = row.inner;
        if kind == Kind::Quote {
            let r = row.response.rect;
            let x = r.left() + GUTTER / 2.0;
            ui.painter().line_segment(
                [egui::pos2(x, r.top() + 2.0), egui::pos2(x, r.bottom() - 2.0)],
                Stroke::new(3.0, theme::ACCENT),
            );
        }
        if toggle_check && let Kind::Check(done) = block.kind {
            block.kind = Kind::Check(!done);
        }

        let focused = out.response.has_focus();
        if focused {
            self.active = Some(i);
        }
        let changed = block.text != old_text;
        if changed {
            block.styles = adjust_styles(&old_text, &old_styles, &block.text, pending.as_ref());
            self.pending = None;
            Self::autoformat(block);
        } else if focused && out.cursor_range != pre {
            self.pending = None;
        }

        if kind != Kind::Code
            && let Some(nl) = block.text.chars().position(|c| c == '\n')
        {
            let byte = richtext::byte_at(&block.text, nl);
            block.text.remove(byte);
            block.styles.remove(nl);
            return Some(Op::Split(i, nl));
        }
        let len = block.char_len();
        if !had_focus || changed {
            self.link_row(ui, i, focused);
            return None;
        }
        let pre = pre.map(|r| r.as_sorted_char_range());
        let collapsed_at = |at: usize| pre.as_ref().is_some_and(|r| r.start.0 == at && r.end.0 == at);
        let row_of = |idx: usize| out.galley.layout_from_cursor(CCursor::new(idx)).row;
        let op = if backspace && collapsed_at(0) {
            Some(Op::BackspaceAtStart(i))
        } else if delete && collapsed_at(len) {
            Some(Op::DeleteAtEnd(i))
        } else if up && i > 0 && pre.as_ref().is_some_and(|r| row_of(r.start.0) == 0) {
            Some(Op::Focus(i - 1, Caret::End))
        } else if down
            && i + 1 < block_count
            && pre
                .as_ref()
                .is_some_and(|r| row_of(r.end.0) + 1 >= out.galley.rows.len())
        {
            Some(Op::Focus(i + 1, Caret::Start))
        } else {
            None
        };
        self.link_row(ui, i, focused);
        op
    }

    /// Under the active block, edit the URL of the link the caret is in.
    fn link_row(&mut self, ui: &mut Ui, i: usize, focused: bool) {
        if self.active != Some(i) {
            return;
        }
        let url_id = self.edit_id(i).with("url");
        let editing_url = ui.memory(|m| m.has_focus(url_id));
        if !focused && !editing_url {
            return;
        }
        let at = self.selection(ui.ctx(), i).primary.index.0;
        let Some((run, mut url)) = self.blocks[i].link_run(at) else {
            return;
        };
        let mut remove = false;
        ui.horizontal(|ui| {
            ui.add_space(GUTTER);
            ui.label(RichText::new(icon::LINK).color(theme::ACCENT));
            ui.add(
                TextEdit::singleline(&mut url)
                    .id(url_id)
                    .margin(super::INPUT_MARGIN)
                    .desired_width(320.0),
            );
            remove = icon_button(ui, icon::LINK_BREAK, "Remove link").clicked();
        });
        let block = &mut self.blocks[i];
        if remove {
            block.set_link(run, None);
        } else {
            block.set_link(run, Some(url));
        }
    }
}

/// Bullet, number, checkbox or quote bar to the left of a block.
fn gutter(ui: &mut Ui, kind: Kind, number: usize, size: f32, toggle_check: &mut bool) {
    let height = size + 6.0;
    match kind {
        Kind::Paragraph | Kind::Heading(_) | Kind::Code => {}
        Kind::Quote => {
            ui.add_space(GUTTER);
        }
        Kind::Bullet => {
            let (rect, _) = ui.allocate_exact_size(vec2(GUTTER, height), Sense::hover());
            ui.painter().circle_filled(rect.center(), 2.5, theme::TEXT);
        }
        Kind::Numbered => {
            let (rect, _) = ui.allocate_exact_size(vec2(GUTTER, height), Sense::hover());
            ui.painter().text(
                rect.right_center() - vec2(6.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                format!("{number}."),
                FontId::proportional(size),
                theme::TEXT_MUTED,
            );
        }
        Kind::Check(done) => {
            let (rect, response) = ui.allocate_exact_size(vec2(GUTTER, height), Sense::click());
            let response = super::pointer(response);
            let glyph = if done { icon::CHECK_SQUARE } else { icon::SQUARE };
            let color = if response.hovered() || done {
                theme::ACCENT
            } else {
                theme::TEXT_MUTED
            };
            let font = FontId::proportional(size + 1.0);
            ui.painter()
                .text(rect.center(), egui::Align2::CENTER_CENTER, glyph, font, color);
            *toggle_check = response.clicked();
        }
    }
}

/// Formatted layout for one block: bold/italic/code/link runs, no Markdown markers.
fn layout(text: &str, styles: &[Style], kind: Kind, size: f32, has_bold: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    let base_color = match kind {
        Kind::Quote => theme::TEXT_MUTED,
        Kind::Check(true) => theme::TEXT_MUTED,
        _ => theme::TEXT,
    };
    let heading = matches!(kind, Kind::Heading(_));
    let mut start = 0;
    let chars: Vec<char> = text.chars().collect();
    let default = Style::default();
    for i in 1..=chars.len() {
        let style_at = |j: usize| styles.get(j).unwrap_or(&default);
        if i < chars.len() && style_at(i) == style_at(start) {
            continue;
        }
        let s = style_at(start);
        let run: String = chars[start..i].iter().collect();
        let font_id = if kind == Kind::Code || s.code {
            FontId::monospace(size - 1.0)
        } else if (s.bold || heading) && has_bold {
            FontId::new(size, FontFamily::Name(theme::BOLD.into()))
        } else {
            FontId::proportional(size)
        };
        let format = TextFormat {
            font_id,
            color: if s.link.is_some() { theme::ACCENT } else { base_color },
            italics: s.italic || kind == Kind::Quote,
            background: if s.code && kind != Kind::Code {
                theme::CARD
            } else {
                Color32::TRANSPARENT
            },
            underline: if s.link.is_some() {
                Stroke::new(1.0, theme::ACCENT)
            } else {
                Stroke::NONE
            },
            strikethrough: if kind == Kind::Check(true) {
                Stroke::new(1.0, theme::TEXT_MUTED)
            } else {
                Stroke::NONE
            },
            ..Default::default()
        };
        job.append(&run, 0.0, format);
        start = i;
    }
    if chars.is_empty() {
        job.append("", 0.0, TextFormat::simple(FontId::proportional(size), base_color));
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(md: &str) -> RichEditor {
        RichEditor::new("test", md)
    }

    fn texts(e: &RichEditor) -> Vec<(&str, Kind)> {
        e.blocks.iter().map(|b| (b.text.as_str(), b.kind)).collect()
    }

    #[test]
    fn enter_splits_and_continues_lists() {
        let mut e = editor("- one two");
        e.apply(Op::Split(0, 4));
        assert_eq!(texts(&e), [("one ", Kind::Bullet), ("two", Kind::Bullet)]);
        // Enter on an empty item leaves the list.
        e.insert_block(2, Block::new(Kind::Bullet));
        e.apply(Op::Split(2, 0));
        assert_eq!(e.blocks[2].kind, Kind::Paragraph);
        assert_eq!(e.blocks.len(), 3);
        assert_eq!(e.uids.len(), 3);
    }

    #[test]
    fn backspace_and_delete_join_blocks() {
        let mut e = editor("first\n\n- second");
        e.apply(Op::BackspaceAtStart(1));
        assert_eq!(texts(&e), [("first", Kind::Paragraph), ("second", Kind::Paragraph)]);
        e.apply(Op::BackspaceAtStart(1));
        assert_eq!(texts(&e), [("firstsecond", Kind::Paragraph)]);
        assert_eq!(e.focus.map(|(i, r)| (i, r.primary.index.0)), Some((0, 5)));
        let mut e = editor("a\n\nb");
        e.apply(Op::DeleteAtEnd(0));
        assert_eq!(texts(&e), [("ab", Kind::Paragraph)]);
    }

    #[test]
    fn typing_markdown_prefixes_autoformats() {
        for (typed, kind, rest) in [
            ("- item", Kind::Bullet, "item"),
            ("1. item", Kind::Numbered, "item"),
            ("## Title", Kind::Heading(2), "Title"),
            ("[ ] task", Kind::Check(false), "task"),
            ("> said", Kind::Quote, "said"),
            ("```", Kind::Code, ""),
        ] {
            let mut b = Block::new(Kind::Paragraph);
            b.text = typed.into();
            b.styles = vec![Style::default(); typed.chars().count()];
            RichEditor::autoformat(&mut b);
            assert_eq!((b.kind, b.text.as_str()), (kind, rest), "{typed}");
            assert_eq!(b.styles.len(), b.char_len());
        }
    }

    #[test]
    fn unsupported_markdown_stays_raw_and_unchanged() {
        let md = "| a | b |\n|---|---|\n| 1 | 2 |";
        let e = editor(md);
        assert!(e.raw.is_some());
        assert_eq!(e.markdown(), md);
        assert!(!e.changed());
    }

    #[test]
    fn empty_text_starts_with_one_paragraph() {
        let e = editor("");
        assert_eq!(texts(&e), [("", Kind::Paragraph)]);
        assert_eq!(e.markdown(), "");
    }

    fn harness(md: &str) -> egui_kittest::Harness<'static, RichEditor> {
        let mut e = editor(md);
        e.focus_at(0, Caret::End);
        let mut h = egui_kittest::Harness::new_ui_state(|ui, e: &mut RichEditor| e.show(ui, 300.0), e);
        crate::theme::apply(&h.ctx);
        h.set_size(vec2(700.0, 400.0));
        h.run();
        h
    }

    fn type_text(h: &mut egui_kittest::Harness<'static, RichEditor>, text: &str) {
        h.event(egui::Event::Text(text.into()));
        h.run();
    }

    #[test]
    fn typing_builds_a_list_without_showing_markdown() {
        let mut h = harness("");
        type_text(&mut h, "- ");
        type_text(&mut h, "first");
        h.key_press(Key::Enter);
        h.run();
        type_text(&mut h, "second");
        assert_eq!(texts(h.state()), [("first", Kind::Bullet), ("second", Kind::Bullet)]);
        assert_eq!(
            h.state().markdown(),
            "- first
- second"
        );
    }

    #[test]
    fn ctrl_b_then_typing_is_bold() {
        let mut h = harness("Plain");
        type_text(&mut h, " ");
        h.key_press_modifiers(Modifiers::COMMAND, Key::B);
        h.run();
        type_text(&mut h, "loud");
        assert_eq!(h.state().markdown(), "Plain **loud**");
    }

    #[test]
    fn backspace_at_start_turns_list_item_into_paragraph() {
        let mut h = harness("- item");
        h.state_mut().focus_at(0, Caret::Start);
        h.run();
        h.key_press(Key::Backspace);
        h.run();
        assert_eq!(texts(h.state()), [("item", Kind::Paragraph)]);
    }
}
