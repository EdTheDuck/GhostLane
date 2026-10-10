//! Document model behind the visual ticket editor: blocks of styled characters that
//! convert to and from Markdown. The user never sees Markdown; it only exists on disk.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Paragraph,
    Heading(u8),
    Bullet,
    Numbered,
    Check(bool),
    Quote,
    Code,
}

impl Kind {
    pub fn is_list(self) -> bool {
        matches!(self, Kind::Bullet | Kind::Numbered | Kind::Check(_))
    }
}

/// One editable line of the document. `styles` has exactly one entry per char of `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: Kind,
    pub text: String,
    pub styles: Vec<Style>,
    /// Code block language (fence info string).
    pub lang: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    Bold,
    Italic,
    Code,
}

impl Block {
    pub fn new(kind: Kind) -> Self {
        Block {
            kind,
            text: String::new(),
            styles: Vec::new(),
            lang: String::new(),
        }
    }

    fn push(&mut self, s: &str, style: &Style) {
        self.text.push_str(s);
        self.styles.extend(s.chars().map(|_| style.clone()));
    }

    pub fn char_len(&self) -> usize {
        self.styles.len()
    }

    /// Splits at char `at`; `self` keeps the start, the rest is returned with the same kind.
    pub fn split_off(&mut self, at: usize) -> Block {
        let byte = byte_at(&self.text, at);
        Block {
            kind: self.kind,
            text: self.text.split_off(byte),
            styles: self.styles.split_off(at.min(self.styles.len())),
            lang: self.lang.clone(),
        }
    }

    pub fn append(&mut self, other: Block) {
        self.text.push_str(&other.text);
        self.styles.extend(other.styles);
    }

    /// Removes the first `n` chars (used when an autoformat prefix like "- " is consumed).
    pub fn strip_prefix_chars(&mut self, n: usize) {
        let rest = self.split_off(n);
        *self = Block {
            kind: self.kind,
            ..rest
        };
    }

    /// Turns `flag` on for chars `range`, or off if every char there already has it.
    pub fn toggle(&mut self, range: std::ops::Range<usize>, flag: Flag) {
        let get = |s: &Style| match flag {
            Flag::Bold => s.bold,
            Flag::Italic => s.italic,
            Flag::Code => s.code,
        };
        let range = range.start.min(self.styles.len())..range.end.min(self.styles.len());
        let on = !self.styles[range.clone()].iter().all(get);
        for s in &mut self.styles[range] {
            match flag {
                Flag::Bold => s.bold = on,
                Flag::Italic => s.italic = on,
                Flag::Code => s.code = on,
            }
        }
    }

    /// Range of chars sharing the link at char `at` (or the char before it).
    pub fn link_run(&self, at: usize) -> Option<(std::ops::Range<usize>, String)> {
        let probe = [at, at.saturating_sub(1)]
            .into_iter()
            .find(|&i| self.styles.get(i).is_some_and(|s| s.link.is_some()))?;
        let url = self.styles[probe].link.clone()?;
        let same = |i: usize| self.styles.get(i).is_some_and(|s| s.link.as_deref() == Some(&url));
        let mut start = probe;
        while start > 0 && same(start - 1) {
            start -= 1;
        }
        let mut end = probe + 1;
        while same(end) {
            end += 1;
        }
        Some((start..end, url))
    }

    pub fn set_link(&mut self, range: std::ops::Range<usize>, url: Option<String>) {
        for s in &mut self.styles[range] {
            s.link = url.clone();
        }
    }
}

pub fn byte_at(text: &str, char_idx: usize) -> usize {
    text.char_indices().nth(char_idx).map_or(text.len(), |(b, _)| b)
}

/// Styles for `new` after an edit turned `old` into `new`: untouched chars keep theirs, inserted
/// chars take `pending` (a style toggled with nothing selected) or continue the char before them.
pub fn adjust_styles(old: &str, old_styles: &[Style], new: &str, pending: Option<&Style>) -> Vec<Style> {
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();
    let prefix = old_chars.iter().zip(&new_chars).take_while(|(a, b)| a == b).count();
    let max_suffix = old_chars.len().min(new_chars.len()) - prefix;
    let suffix = old_chars
        .iter()
        .rev()
        .zip(new_chars.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let inserted = new_chars.len() - prefix - suffix;
    let fill = pending.cloned().unwrap_or_else(|| {
        let before = prefix.checked_sub(1).and_then(|i| old_styles.get(i));
        let mut s = before.or(old_styles.get(prefix)).cloned().unwrap_or_default();
        // Typing right after a link doesn't extend it; typing inside one does.
        let inside_link =
            before.is_some_and(|b| b.link.is_some() && old_styles.get(prefix).is_some_and(|a| a.link == b.link));
        if !inside_link {
            s.link = None;
        }
        s
    });
    let mut out = Vec::with_capacity(new_chars.len());
    out.extend_from_slice(&old_styles[..prefix.min(old_styles.len())]);
    out.extend(std::iter::repeat_n(fill, inserted));
    let tail_start = old_chars.len() - suffix;
    out.extend_from_slice(&old_styles[tail_start.min(old_styles.len())..]);
    out.resize(new_chars.len(), Style::default());
    out
}

/// Parses Markdown into blocks. `None` when it uses something the editor can't represent
/// (tables, images, HTML, nested lists, hard line breaks, ...) so the caller can fall back to raw Markdown.
pub fn parse(md: &str) -> Option<Vec<Block>> {
    let mut doc = Vec::new();
    let mut cur: Option<Block> = None;
    let (mut bold, mut italic) = (0u32, 0u32);
    let mut link: Option<String> = None;
    let mut list: Option<Kind> = None;
    let mut in_quote = false;
    let mut item_paragraphs = 0;

    // Same extensions the markdown viewer enables, so anything it would render specially is detected here.
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_DEFINITION_LIST;
    for event in Parser::new_ext(md, options) {
        let style = Style {
            bold: bold > 0,
            italic: italic > 0,
            code: false,
            link: link.clone(),
        };
        match event {
            Event::Start(Tag::Paragraph) if list.is_some() => {
                item_paragraphs += 1;
                if item_paragraphs > 1 {
                    return None;
                }
            }
            Event::End(TagEnd::Paragraph) if list.is_some() => {}
            Event::Start(Tag::Paragraph) => {
                cur = Some(Block::new(if in_quote { Kind::Quote } else { Kind::Paragraph }))
            }
            Event::Start(Tag::Heading {
                level,
                id,
                classes,
                attrs,
            }) => {
                if in_quote || list.is_some() || id.is_some() || !classes.is_empty() || !attrs.is_empty() {
                    return None;
                }
                cur = Some(Block::new(Kind::Heading(level as u8)));
            }
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_)) => doc.push(cur.take()?),
            Event::Start(Tag::BlockQuote(kind)) => {
                if in_quote || kind.is_some() || list.is_some() {
                    return None;
                }
                in_quote = true;
            }
            Event::End(TagEnd::BlockQuote(_)) => in_quote = false,
            Event::Start(Tag::CodeBlock(kind)) => {
                if in_quote || list.is_some() {
                    return None;
                }
                let mut b = Block::new(Kind::Code);
                if let CodeBlockKind::Fenced(lang) = kind {
                    b.lang = lang.to_string();
                }
                cur = Some(b);
            }
            Event::End(TagEnd::CodeBlock) => {
                let mut b = cur.take()?;
                if b.text.ends_with('\n') {
                    b.text.pop();
                    b.styles.pop();
                }
                doc.push(b);
            }
            Event::Start(Tag::List(start)) => {
                if list.is_some() || in_quote {
                    return None;
                }
                list = Some(match start {
                    None => Kind::Bullet,
                    Some(1) => Kind::Numbered,
                    Some(_) => return None,
                });
            }
            Event::End(TagEnd::List(_)) => list = None,
            Event::Start(Tag::Item) => {
                item_paragraphs = 0;
                cur = Some(Block::new(list?));
            }
            Event::End(TagEnd::Item) => doc.push(cur.take()?),
            Event::TaskListMarker(done) => {
                let b = cur.as_mut()?;
                if b.kind != Kind::Bullet {
                    return None;
                }
                b.kind = Kind::Check(done);
            }
            Event::Start(Tag::Emphasis) => italic += 1,
            Event::End(TagEnd::Emphasis) => italic = italic.saturating_sub(1),
            Event::Start(Tag::Strong) => bold += 1,
            Event::End(TagEnd::Strong) => bold = bold.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, title, .. }) => {
                if !title.is_empty() || link.is_some() {
                    return None;
                }
                link = Some(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => link = None,
            Event::Text(t) => {
                let b = cur.as_mut()?;
                let style = if b.kind == Kind::Code { Style::default() } else { style };
                b.push(&t, &style);
            }
            Event::Code(t) => cur.as_mut()?.push(&t, &Style { code: true, ..style }),
            Event::SoftBreak => cur.as_mut()?.push(" ", &style),
            _ => return None,
        }
    }
    Some(doc)
}

/// Serializes blocks back to Markdown. Empty paragraphs are dropped.
pub fn to_markdown(doc: &[Block]) -> String {
    let list_group = |k: Kind| match k {
        Kind::Bullet | Kind::Check(_) => Some(0),
        Kind::Numbered => Some(1),
        _ => None,
    };
    let mut out = String::new();
    let mut prev: Option<Kind> = None;
    for b in doc
        .iter()
        .filter(|b| !(b.kind == Kind::Paragraph && b.text.trim().is_empty()))
    {
        if let Some(p) = prev {
            out.push_str(if p == Kind::Quote && b.kind == Kind::Quote {
                "\n>\n"
            } else if list_group(p).is_some() && list_group(p) == list_group(b.kind) {
                "\n"
            } else {
                "\n\n"
            });
        }
        let text = inline(&b.text, &b.styles);
        match b.kind {
            Kind::Paragraph => out.push_str(&text),
            Kind::Heading(n) => out.push_str(&format!("{} {text}", "#".repeat(n.clamp(1, 6) as usize))),
            Kind::Bullet => out.push_str(&format!("- {text}")),
            Kind::Check(done) => out.push_str(&format!("- [{}] {text}", if done { 'x' } else { ' ' })),
            Kind::Numbered => out.push_str(&format!("1. {text}")),
            Kind::Quote => out.push_str(&format!("> {text}")),
            Kind::Code => {
                let fence = "`".repeat(longest_run(&b.text, '`').max(2) + 1);
                out.push_str(&format!("{fence}{}\n{}\n{fence}", b.lang, b.text));
            }
        }
        prev = Some(b.kind);
    }
    out
}

fn longest_run(text: &str, c: char) -> usize {
    text.split(|x| x != c).map(str::len).max().unwrap_or(0)
}

/// Styled chars to inline Markdown. Whitespace at the edge of a bold/italic run is moved
/// outside the markers so CommonMark still recognises them.
// ponytail: a bold run touching an italic run mid-word (`**a***b*`) can parse ambiguously; whitespace-separated styling is exact.
fn inline(text: &str, styles: &[Style]) -> String {
    let chars: Vec<(char, &Style)> = text
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .zip(styles)
        .collect();
    let mut out = String::new();
    for link_run in runs(&chars, |s| s.link.clone()) {
        let body = emphasis(link_run);
        match &link_run[0].1.link {
            Some(url) if url.contains([' ', '(', ')', '<', '>']) => out.push_str(&format!("[{body}](<{url}>)")),
            Some(url) => out.push_str(&format!("[{body}]({url})")),
            None => out.push_str(&body),
        }
    }
    // Block-start characters that would otherwise turn a paragraph into a list or heading.
    let trimmed = out.trim_start();
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if trimmed.starts_with("- ") || trimmed.starts_with("+ ") || trimmed == "-" || trimmed == "+" {
        format!("\\{trimmed}")
    } else if digits > 0 && trimmed[digits..].starts_with(['.', ')']) {
        format!("{}\\{}", &trimmed[..digits], &trimmed[digits..])
    } else {
        trimmed.to_string()
    }
}

fn runs<'a, K: PartialEq>(chars: &'a [(char, &Style)], key: impl Fn(&Style) -> K) -> Vec<&'a [(char, &'a Style)]> {
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..=chars.len() {
        if i == chars.len() || key(chars[i].1) != key(chars[start].1) {
            out.push(&chars[start..i]);
            start = i;
        }
    }
    out
}

fn emphasis(chars: &[(char, &Style)]) -> String {
    let mut out = String::new();
    for run in runs(chars, |s| (s.bold, s.italic)) {
        let marker = match (run[0].1.bold, run[0].1.italic) {
            (true, true) => "***",
            (true, false) => "**",
            (false, true) => "*",
            (false, false) => "",
        };
        let is_ws = |(c, s): &(char, &Style)| c.is_whitespace() && !s.code;
        let lead = run.iter().take_while(|c| is_ws(c)).count();
        let trail = run[lead..].iter().rev().take_while(|c| is_ws(c)).count();
        let core = &run[lead..run.len() - trail];
        let ws = |part: &[(char, &Style)]| part.iter().map(|(c, _)| *c).collect::<String>();
        out.push_str(&ws(&run[..lead]));
        if !core.is_empty() {
            out.push_str(marker);
            for seg in runs(core, |s| s.code) {
                let s: String = seg.iter().map(|(c, _)| *c).collect();
                if seg[0].1.code {
                    out.push_str(&code_span(&s));
                } else {
                    out.push_str(&escape(&s));
                }
            }
            out.push_str(marker);
        }
        out.push_str(&ws(&run[run.len() - trail..]));
    }
    out
}

fn code_span(s: &str) -> String {
    let ticks = "`".repeat(longest_run(s, '`') + 1);
    let pad =
        if s.starts_with('`') || s.ends_with('`') || (s.starts_with(' ') && s.ends_with(' ') && !s.trim().is_empty()) {
            " "
        } else {
            ""
        };
    format!("{ticks}{pad}{s}{pad}{ticks}")
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '>' | '&' | '#' | '~' | '|' | '!'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(md: &str) -> Vec<Block> {
        let doc = parse(md).unwrap_or_else(|| panic!("unsupported: {md}"));
        let back = to_markdown(&doc);
        assert_eq!(
            parse(&back).as_ref(),
            Some(&doc),
            "round trip changed the document:\n{md}\n---\n{back}"
        );
        doc
    }

    #[test]
    fn blocks_round_trip() {
        let doc = round_trip(
            "# Title\n\nSome **bold** and *italic* and ***both*** with `code` and a [link](https://x.dev).\n\n\
             - one\n- [ ] todo\n- [x] done\n\n1. first\n1. second\n\n> quoted\n>\n> more\n\n```rust\nfn main() {}\n```\n\n## Sub",
        );
        let kinds: Vec<Kind> = doc.iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            [
                Kind::Heading(1),
                Kind::Paragraph,
                Kind::Bullet,
                Kind::Check(false),
                Kind::Check(true),
                Kind::Numbered,
                Kind::Numbered,
                Kind::Quote,
                Kind::Quote,
                Kind::Code,
                Kind::Heading(2),
            ]
        );
        assert_eq!(doc[9].lang, "rust");
        assert_eq!(doc[9].text, "fn main() {}");
        let p = &doc[1];
        let bold_at = |word: &str| p.styles[p.text[..p.text.find(word).unwrap()].chars().count()].clone();
        assert!(bold_at("bold").bold);
        assert!(bold_at("italic").italic);
        assert!(bold_at("both").bold && bold_at("both").italic);
        assert!(bold_at("code").code);
        assert_eq!(bold_at("link").link.as_deref(), Some("https://x.dev"));
    }

    #[test]
    fn tricky_text_round_trips() {
        round_trip("Use *stars* and \\*literal stars\\* and 2 \\* 3, `a*b` and [x](<a b>)");
        round_trip("\\- not a list\n\n1\\. not numbered\n\n\\# not heading");
        round_trip("A `` code with ` tick `` here, **bold `code` inside** too.");
        round_trip("Soft\nwrapped line");
    }

    #[test]
    fn generated_documents_round_trip() {
        let styles = [
            Style::default(),
            Style {
                bold: true,
                ..Default::default()
            },
            Style {
                italic: true,
                ..Default::default()
            },
            Style {
                code: true,
                ..Default::default()
            },
            Style {
                link: Some("https://a.b".into()),
                ..Default::default()
            },
        ];
        // Words separated by spaces, each word given one style, across every block kind.
        let words = ["alpha", "beta*", "_gamma_", "1.", "#tag", "x<y>", "a&b", "[c]"];
        let kinds = [
            Kind::Paragraph,
            Kind::Heading(2),
            Kind::Bullet,
            Kind::Numbered,
            Kind::Check(true),
            Kind::Quote,
        ];
        let mut doc = Vec::new();
        for (k, kind) in kinds.iter().enumerate() {
            let mut b = Block::new(*kind);
            for (w, word) in words.iter().enumerate() {
                if w > 0 {
                    b.push(" ", &Style::default());
                }
                b.push(word, &styles[(w + k) % styles.len()]);
            }
            doc.push(b);
        }
        let md = to_markdown(&doc);
        assert_eq!(parse(&md), Some(doc), "{md}");
    }

    #[test]
    fn unsupported_markdown_is_refused() {
        for md in [
            "| a | b |\n|---|---|\n| 1 | 2 |",
            "![img](x.png)",
            "<div>html</div>",
            "- outer\n  - nested",
            "line one  \nline two",
            "3. starts at three",
            "---",
            "[t](http://x \"title\")",
            "~~struck~~",
        ] {
            assert!(parse(md).is_none(), "{md}");
        }
    }

    #[test]
    fn style_adjustment_follows_edits() {
        let s = |b: bool| Style {
            bold: b,
            ..Default::default()
        };
        let old = [s(false), s(true), s(true), s(false)];
        // Typing inside the bold run extends it.
        let new = adjust_styles("abcd", &old, "abXcd", None);
        assert_eq!(new, [s(false), s(true), s(true), s(true), s(false)]);
        // Deleting a char drops its style.
        assert_eq!(adjust_styles("abcd", &old, "acd", None), [s(false), s(true), s(false)]);
        // A pending style wins.
        assert_eq!(adjust_styles("ab", &old[..2], "abc", Some(&s(false)))[2], s(false));
        // Typing just after a link doesn't extend it.
        let l = Style {
            link: Some("u".into()),
            ..Default::default()
        };
        assert_eq!(adjust_styles("a", std::slice::from_ref(&l), "ab", None)[1].link, None);
    }

    #[test]
    fn toggling_and_splitting() {
        let mut b = Block::new(Kind::Bullet);
        b.push("hello world", &Style::default());
        b.toggle(0..5, Flag::Bold);
        assert!(b.styles[..5].iter().all(|s| s.bold) && !b.styles[5].bold);
        b.toggle(0..5, Flag::Bold);
        assert!(b.styles.iter().all(|s| !s.bold));
        let tail = b.split_off(6);
        assert_eq!(
            (b.text.as_str(), tail.text.as_str(), tail.kind),
            ("hello ", "world", Kind::Bullet)
        );
        b.append(tail);
        assert_eq!(b.char_len(), 11);
        b.strip_prefix_chars(6);
        assert_eq!(b.text, "world");
    }
}
