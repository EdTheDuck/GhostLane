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

/// How a run of the AI on a ticket ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunResult {
    /// The ticket ended up further along the board.
    MovedOn,
    /// The ticket came back to an earlier column.
    SentBack,
    /// The ticket stayed in the column it started in.
    Unmoved,
    /// The ticket reached the last column but the project's test command failed.
    TestsFailed,
    /// Stopped from the terminal bar.
    Stopped,
    /// The AI CLI quit before it finished.
    CliClosed,
    #[serde(other)]
    Other,
}

impl RunResult {
    pub fn label(self) -> &'static str {
        match self {
            RunResult::MovedOn => "Moved on",
            RunResult::SentBack => "Sent back",
            RunResult::Unmoved => "Didn't move",
            RunResult::TestsFailed => "Test command failed",
            RunResult::Stopped => "Stopped",
            RunResult::CliClosed => "AI CLI closed",
            RunResult::Other => "Unknown",
        }
    }

    pub fn failed(self) -> bool {
        !matches!(self, RunResult::MovedOn | RunResult::Unmoved)
    }
}

/// One time the AI worked on a ticket, kept in its frontmatter under `runs`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    /// UTC, as written by `utc_timestamp`.
    pub started: String,
    pub seconds: u64,
    /// Names of the columns the ticket started in and ended up in.
    pub from: String,
    pub to: String,
    pub result: RunResult,
    /// The git commit checked out when the run started, so the ticket's changes can be shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

/// Runs kept per ticket; older ones are dropped.
pub const MAX_RUNS: usize = 20;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ticket {
    pub id: u32,
    pub path: PathBuf,
    pub title: String,
    pub priority: Priority,
    pub run_count: u32,
    /// Tickets that must reach the last column before Auto or the queue start this one.
    pub blocked_by: Vec<u32>,
    /// When the ticket last arrived in the last column, in seconds since 1970.
    pub done_at: Option<u64>,
    /// Oldest first.
    pub runs: Vec<RunRecord>,
    pub description: String,
    pub test_scope: String,
    pub notes: String,
    /// Frontmatter was missing or unreadable; shown with a warning marker.
    pub malformed: bool,
    /// The file's modified time in seconds since 1970, when loaded from disk.
    pub modified: Option<u64>,
}

#[derive(Serialize)]
struct FrontOut<'a> {
    id: String,
    title: &'a str,
    priority: Priority,
    run_count: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    blocked_by: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    done_at: Option<String>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    runs: &'a [RunRecord],
}

pub fn format_id(id: u32) -> String {
    format!("{id:05}")
}

/// Seconds since 1970, now.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Seconds since 1970 as `2026-10-19T14:03:00Z`.
pub fn utc_timestamp(secs: u64) -> String {
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// `2026-10-19T14:03:00Z` as `2026-10-19 14:03 UTC`; anything else as it is.
pub fn readable_utc(stamp: &str) -> String {
    match parse_utc(stamp).map(utc_timestamp) {
        Some(full) => format!("{} {} UTC", &full[..10], &full[11..16]),
        None => stamp.to_string(),
    }
}

/// Reads back what `utc_timestamp` writes.
pub fn parse_utc(s: &str) -> Option<u64> {
    let (date, time) = s.trim().strip_suffix('Z')?.split_once('T')?;
    let num = |part: Option<&str>| part?.parse::<i64>().ok();
    let mut d = date.split('-');
    let (y, m, day) = (num(d.next())?, num(d.next())?, num(d.next())?);
    let mut t = time.split(':');
    let (h, min, sec) = (num(t.next())?, num(t.next())?, num(t.next())?);
    let in_range = (1..=12).contains(&m) && (1..=31).contains(&day) && (0..24).contains(&h);
    if !in_range || !(0..60).contains(&min) || !(0..60).contains(&sec) {
        return None;
    }
    u64::try_from(days_from_civil(y, m, day) * 86_400 + h * 3600 + min * 60 + sec).ok()
}

// Howard Hinnant's civil date algorithms.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
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
                t.blocked_by = get("blocked_by").map(parse_ids).unwrap_or_default();
                t.blocked_by.retain(|&b| b != id);
                t.done_at = get("done_at").and_then(|v| v.as_str()).and_then(parse_utc);
                t.runs = get("runs")
                    .and_then(|v| serde_yaml_ng::from_value(v.clone()).ok())
                    .unwrap_or_default();
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
            blocked_by: self.blocked_by.iter().map(|&b| format_id(b)).collect(),
            done_at: self.done_at.map(utc_timestamp),
            runs: &self.runs,
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
        let mut t = Ticket::parse(id, path.to_path_buf(), &text);
        t.modified = fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        Ok(Some(t))
    }

    /// Adds a run to the history, dropping the oldest past `MAX_RUNS`.
    pub fn record_run(&mut self, run: RunRecord) {
        self.runs.push(run);
        let extra = self.runs.len().saturating_sub(MAX_RUNS);
        self.runs.drain(..extra);
    }

    /// The commit the latest run started from. Work isn't committed until a ticket is done, so everything the
    /// ticket changed so far is in the diff from there.
    // ponytail: that diff also holds whatever else changed in the repo meanwhile; worktrees per run would fix it.
    pub fn base(&self) -> Option<&str> {
        self.runs.iter().rev().find_map(|r| r.base.as_deref())
    }

    /// Has the ticket been in the last column for more than `days` days? 0 = never archived.
    pub fn archived(&self, now: u64, days: u32) -> bool {
        let since = self.done_at.or(self.modified);
        days > 0 && since.is_some_and(|s| now.saturating_sub(s) > u64::from(days) * 86_400)
    }

    /// Atomic write: temp file then rename over the target.
    pub fn save(&self) -> io::Result<()> {
        write_atomic(&self.path, &self.serialize())
    }
}

/// Ticket IDs from a YAML list (or a single value) of numbers or zero-padded strings; anything else is skipped.
fn parse_ids(v: &serde_yaml_ng::Value) -> Vec<u32> {
    let one = |v: &serde_yaml_ng::Value| match v {
        serde_yaml_ng::Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
        serde_yaml_ng::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    };
    let mut ids: Vec<u32> = match v.as_sequence() {
        Some(seq) => seq.iter().filter_map(one).collect(),
        None => one(v).into_iter().collect(),
    };
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Writes `contents` to a temp file beside `path`, then renames it over `path`. Creates missing folders.
pub fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
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
            blocked_by: vec![2, 5],
            done_at: Some(1_709_210_096),
            runs: vec![RunRecord {
                started: "2024-02-29T12:00:00Z".into(),
                seconds: 252,
                from: "To Do".into(),
                to: "Ready For Test".into(),
                result: RunResult::MovedOn,
                base: Some("3f2a9c1e".into()),
            }],
            malformed: false,
            modified: None,
        };
        let text = t.serialize();
        assert!(text.contains("blocked_by:\n- '00002'\n- '00005'\n"), "{text}");
        let back = Ticket::parse(7, p(), &text);
        assert_eq!(back, t);
    }

    #[test]
    fn utc_timestamps() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(951_868_800), "2000-03-01T00:00:00Z");
        assert_eq!(utc_timestamp(1_709_210_096), "2024-02-29T12:34:56Z");
        for secs in [0, 59, 86_399, 951_782_400, 1_709_210_096, 4_102_444_800] {
            assert_eq!(parse_utc(&utc_timestamp(secs)), Some(secs));
        }
        assert_eq!(parse_utc("2024-13-01T00:00:00Z"), None);
        assert_eq!(parse_utc("2024-02-29 12:34:56"), None);
        assert_eq!(parse_utc("soon"), None);
    }

    #[test]
    fn blockers_parse_leniently() {
        let t = Ticket::parse(3, p(), "---\ntitle: X\nblocked_by: [12, '00007', nope, '3', 7]\n---\n");
        assert_eq!(t.blocked_by, [7, 12]); // self and junk dropped, sorted, deduplicated
        let t = Ticket::parse(3, p(), "---\ntitle: X\nblocked_by: '00009'\nruns: garbage\n---\n");
        assert_eq!(t.blocked_by, [9]);
        assert!(t.runs.is_empty());
    }

    #[test]
    fn run_history_keeps_the_latest() {
        let mut t = Ticket::default();
        for i in 0..MAX_RUNS + 3 {
            t.record_run(RunRecord {
                started: i.to_string(),
                seconds: 1,
                from: "A".into(),
                to: "B".into(),
                result: RunResult::Unmoved,
                base: None,
            });
        }
        assert_eq!(t.runs.len(), MAX_RUNS);
        assert_eq!(t.runs[0].started, "3");
        let odd = Ticket::parse(
            1,
            p(),
            "---\ntitle: X\nruns:\n- {started: s, seconds: 1, from: A, to: B, result: new_kind}\n---\n",
        );
        assert_eq!(odd.runs[0].result, RunResult::Other);
    }

    #[test]
    fn archived_after_days_in_done() {
        let day = 86_400;
        let t = Ticket {
            done_at: Some(10 * day),
            modified: Some(0),
            ..Default::default()
        };
        assert!(!t.archived(12 * day, 7));
        assert!(t.archived(18 * day, 7));
        assert!(!t.archived(18 * day, 0));
        let untouched = Ticket {
            modified: Some(0),
            ..Default::default()
        };
        assert!(untouched.archived(8 * day, 7)); // falls back to the file's age
        assert!(!Ticket::default().archived(8 * day, 7));
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
