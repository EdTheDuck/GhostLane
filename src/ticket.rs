use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::project::{SortBy, SortDir};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    #[default]
    Medium,
    High,
    Highest,
}

impl Priority {
    pub const ALL: [Priority; 4] = [Priority::Low, Priority::Medium, Priority::High, Priority::Highest];

    pub fn label(self) -> &'static str {
        match self {
            Priority::Low => "Low",
            Priority::Medium => "Medium",
            Priority::High => "High",
            Priority::Highest => "Highest",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.label().eq_ignore_ascii_case(s.trim()))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ticket {
    pub id: u32,
    pub path: PathBuf,
    pub title: String,
    pub priority: Priority,
    pub run_count: u32,
    pub description: String,
    pub test_scope: String,
    pub notes: String,
    /// Frontmatter was missing or unreadable; shown with a warning marker.
    pub malformed: bool,
}

#[derive(Serialize)]
struct FrontOut<'a> {
    id: String,
    title: &'a str,
    priority: Priority,
    run_count: u32,
}

pub fn format_id(id: u32) -> String {
    format!("{id:05}")
}

/// `00042.md` -> 42. Anything else is not a ticket file.
pub fn id_from_path(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".md")?;
    if stem.is_empty() || !stem.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    stem.parse().ok()
}

impl Ticket {
    pub fn id_str(&self) -> String {
        format_id(self.id)
    }

    /// Has the ticket been sent to the AI `limit` times already? Auto skips it then. 0 = no limit.
    pub fn at_run_limit(&self, limit: u32) -> bool {
        limit > 0 && self.run_count >= limit
    }

    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        q.is_empty()
            || self.id_str().contains(&q)
            || self.title.to_lowercase().contains(&q)
            || self.description.to_lowercase().contains(&q)
            || self.test_scope.to_lowercase().contains(&q)
            || self.notes.to_lowercase().contains(&q)
    }

    pub fn parse(id: u32, path: PathBuf, text: &str) -> Ticket {
        let text = text.replace("\r\n", "\n");
        let mut t = Ticket {
            id,
            path,
            ..Default::default()
        };

        let front = split_frontmatter(&text).and_then(|(yaml, body)| {
            let map: serde_yaml_ng::Mapping = serde_yaml_ng::from_str(yaml).ok()?;
            Some((map, body))
        });

        let body = match front {
            Some((map, body)) => {
                let get = |k: &str| map.get(k);
                match get("title").and_then(|v| v.as_str()) {
                    Some(title) if !title.trim().is_empty() => t.title = title.to_string(),
                    _ => t.malformed = true,
                }
                t.priority = get("priority")
                    .and_then(|v| v.as_str())
                    .and_then(Priority::parse)
                    .unwrap_or_default();
                t.run_count = get("run_count")
                    .and_then(|v| v.as_u64())
                    .map(|n| n.min(u32::MAX as u64) as u32)
                    .unwrap_or(0);
                body
            }
            None => {
                t.malformed = true;
                text.as_str()
            }
        };
        if t.title.is_empty() {
            t.title = format_id(id);
        }

        [t.description, t.test_scope, t.notes] = split_body(body);
        t
    }

    pub fn serialize(&self) -> String {
        let front = FrontOut {
            id: self.id_str(),
            title: &self.title,
            priority: self.priority,
            run_count: self.run_count,
        };
        let yaml = serde_yaml_ng::to_string(&front).expect("ticket frontmatter serializes");
        let mut out = format!("---\n{yaml}---\n\n## Description\n\n");
        let desc = self.description.trim();
        if !desc.is_empty() {
            out.push_str(desc);
            out.push_str("\n\n");
        }
        for (heading, text) in [("## Test Scope", &self.test_scope), ("## Notes", &self.notes)] {
            let text = text.trim();
            if !text.is_empty() {
                out.push_str(&format!("{heading}\n\n{text}\n\n"));
            }
        }
        out.truncate(out.trim_end().len());
        out.push('\n');
        out
    }

    pub fn load(path: &Path) -> io::Result<Option<Ticket>> {
        let Some(id) = id_from_path(path) else { return Ok(None) };
        let text = fs::read_to_string(path)?;
        Ok(Some(Ticket::parse(id, path.to_path_buf(), &text)))
    }

    /// Atomic write: temp file then rename over the target.
    pub fn save(&self) -> io::Result<()> {
        write_atomic(&self.path, &self.serialize())
    }
}

pub fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path)
}

/// Returns (yaml, body) when text starts with a `---` fenced block.
fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("---\n")?;
    if let Some(body) = rest.strip_prefix("---\n") {
        return Some(("", body));
    }
    let end = rest
        .find("\n---\n")
        .map(|i| (i, i + 5))
        .or_else(|| rest.strip_suffix("\n---").map(|r| (r.len(), rest.len())))?;
    Some((&rest[..end.0], &rest[end.1..]))
}

/// Byte offsets (line start, next line start) of the first line equal to `heading`.
fn find_heading(text: &str, heading: &str) -> Option<(usize, usize)> {
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let end = start + line.len();
        if line.trim_end().eq_ignore_ascii_case(heading) {
            return Some((start, end));
        }
        start = end;
    }
    None
}

/// [description, test scope, notes], in whatever order the headings appear. Text before the first
/// heading counts as description.
fn split_body(body: &str) -> [String; 3] {
    let mut found: Vec<(usize, usize, usize)> = ["## Description", "## Test Scope", "## Notes"]
        .iter()
        .enumerate()
        .filter_map(|(i, h)| find_heading(body, h).map(|(s, e)| (s, e, i)))
        .collect();
    found.sort();
    let mut out: [String; 3] = Default::default();
    let first = found.first().map_or(body.len(), |f| f.0);
    out[0] = body[..first].trim().to_string();
    for (k, &(_, end, i)) in found.iter().enumerate() {
        let next = found.get(k + 1).map_or(body.len(), |f| f.0);
        let text = body[end..next].trim();
        if !text.is_empty() {
            out[i] = text.to_string();
        }
    }
    out
}

pub fn sort(tickets: &mut [Ticket], by: SortBy, dir: SortDir) {
    tickets.sort_by(|a, b| {
        let ord = match by {
            SortBy::Id => a.id.cmp(&b.id),
            SortBy::Priority => a.priority.cmp(&b.priority),
            SortBy::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
        };
        let ord = if dir == SortDir::Desc { ord.reverse() } else { ord };
        if ord == Ordering::Equal { a.id.cmp(&b.id) } else { ord }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> PathBuf {
        PathBuf::from("00001.md")
    }

    #[test]
    fn id_formatting() {
        assert_eq!(format_id(1), "00001");
        assert_eq!(format_id(99999), "99999");
        assert_eq!(format_id(100000), "100000");
        assert_eq!(id_from_path(Path::new("x/00042.md")), Some(42));
        assert_eq!(id_from_path(Path::new("x/readme.md")), None);
        assert_eq!(id_from_path(Path::new("x/00042.md.tmp")), None);
        assert_eq!(id_from_path(Path::new("x/.md")), None);
    }

    #[test]
    fn round_trip() {
        let t = Ticket {
            id: 7,
            path: p(),
            title: "Login: \"quoted\" # page".into(),
            priority: Priority::Highest,
            run_count: 3,
            description: "Build it.\n\n- a\n- b".into(),
            test_scope: "- works".into(),
            notes: "Spoke to *design*.".into(),
            malformed: false,
        };
        let back = Ticket::parse(7, p(), &t.serialize());
        assert_eq!(back, t);
    }

    #[test]
    fn round_trip_without_scope_omits_heading() {
        let t = Ticket {
            id: 1,
            path: p(),
            title: "T".into(),
            ..Default::default()
        };
        let s = t.serialize();
        assert!(!s.contains("Test Scope") && !s.contains("Notes"));
        assert!(s.ends_with("## Description\n"));
        assert_eq!(Ticket::parse(1, p(), &s), t);
    }

    #[test]
    fn notes_without_scope_and_in_any_order() {
        let t = Ticket {
            id: 1,
            path: p(),
            title: "T".into(),
            description: "d".into(),
            notes: "n".into(),
            ..Default::default()
        };
        let s = t.serialize();
        assert!(s.ends_with("## Description\n\nd\n\n## Notes\n\nn\n") && !s.contains("Test Scope"));
        assert_eq!(Ticket::parse(1, p(), &s), t);
        let t = Ticket::parse(
            1,
            p(),
            "---\ntitle: X\n---\n## Notes\nn\n## Description\nd\n## Test Scope\ns\n",
        );
        assert_eq!(
            (t.description.as_str(), t.test_scope.as_str(), t.notes.as_str()),
            ("d", "s", "n")
        );
    }

    #[test]
    fn crlf_and_missing_description_heading() {
        let text = "---\r\ntitle: Hi\r\n---\r\nJust text\r\n## Test Scope\r\nscope\r\n";
        let t = Ticket::parse(1, p(), text);
        assert_eq!(t.title, "Hi");
        assert_eq!(t.description, "Just text");
        assert_eq!(t.test_scope, "scope");
        assert!(!t.malformed);
    }

    #[test]
    fn bad_priority_and_count_default() {
        let t = Ticket::parse(1, p(), "---\ntitle: X\npriority: urgent\nrun_count: lots\n---\n");
        assert_eq!(t.priority, Priority::Medium);
        assert_eq!(t.run_count, 0);
    }

    #[test]
    fn id_mismatch_uses_filename() {
        let t = Ticket::parse(5, p(), "---\nid: 00009\ntitle: X\n---\n");
        assert_eq!(t.id, 5);
        assert!(t.serialize().contains("00005"));
    }

    #[test]
    fn missing_frontmatter_is_malformed() {
        let t = Ticket::parse(3, p(), "# Some notes\nhello");
        assert!(t.malformed);
        assert_eq!(t.title, "00003");
        assert_eq!(t.description, "# Some notes\nhello");
        let t = Ticket::parse(3, p(), "---\ntitle: [unclosed\n---\nbody");
        assert!(t.malformed);
    }

    #[test]
    fn search() {
        let t = Ticket {
            id: 12,
            title: "Login Page".into(),
            test_scope: "OAuth".into(),
            notes: "Ask Sam".into(),
            ..Default::default()
        };
        assert!(t.matches("login"));
        assert!(t.matches("sam"));
        assert!(t.matches("oauth"));
        assert!(t.matches("00012"));
        assert!(t.matches("  "));
        assert!(!t.matches("signup"));
    }

    #[test]
    fn sorting() {
        let mk = |id, title: &str, priority| Ticket {
            id,
            title: title.into(),
            priority,
            ..Default::default()
        };
        let mut v = vec![
            mk(3, "b", Priority::Low),
            mk(1, "C", Priority::High),
            mk(2, "a", Priority::High),
        ];
        let ids = |v: &[Ticket]| v.iter().map(|t| t.id).collect::<Vec<_>>();
        sort(&mut v, SortBy::Id, SortDir::Asc);
        assert_eq!(ids(&v), [1, 2, 3]);
        sort(&mut v, SortBy::Title, SortDir::Asc);
        assert_eq!(ids(&v), [2, 3, 1]);
        sort(&mut v, SortBy::Priority, SortDir::Desc);
        assert_eq!(ids(&v), [1, 2, 3]); // tie broken by id asc
    }
}
