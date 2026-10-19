use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use egui_term::{BackendCommand, BackendSettings, ColorPalette, PtyEvent, TerminalBackend, TerminalTheme};

/// An AI coding CLI the play button can hand a ticket to.
pub struct AiCli {
    pub id: &'static str,
    pub name: &'static str,
    pub program: &'static str,
    /// Flag that starts an interactive session with an initial prompt; `None` = positional prompt.
    pub prompt_flag: Option<&'static str>,
    /// Project instructions file the CLI reads, relative to the project root.
    pub instructions_file: &'static str,
    /// Keys that stop the CLI's current turn without quitting it.
    pub interrupt: &'static [u8],
}

pub const DEFAULT_CLI: &str = "claude";

pub const AI_CLIS: [AiCli; 7] = [
    AiCli {
        id: "claude",
        name: "Claude Code",
        program: "claude",
        prompt_flag: None,
        interrupt: b"\x1b",
        instructions_file: "CLAUDE.md",
    },
    AiCli {
        id: "codex",
        name: "OpenAI Codex",
        program: "codex",
        prompt_flag: None,
        interrupt: b"\x1b",
        instructions_file: "AGENTS.md",
    },
    AiCli {
        id: "gemini",
        name: "Gemini CLI",
        program: "gemini",
        prompt_flag: Some("-i"),
        interrupt: b"\x1b",
        instructions_file: "GEMINI.md",
    },
    AiCli {
        id: "copilot",
        name: "GitHub Copilot CLI",
        program: "copilot",
        prompt_flag: Some("-i"),
        interrupt: b"\x1b",
        instructions_file: ".github/copilot-instructions.md",
    },
    AiCli {
        id: "cursor",
        name: "Cursor",
        program: "cursor-agent",
        prompt_flag: None,
        interrupt: b"\x1b",
        instructions_file: "AGENTS.md",
    },
    AiCli {
        id: "opencode",
        name: "opencode",
        program: "opencode",
        prompt_flag: Some("--prompt"),
        interrupt: b"\x1b",
        instructions_file: "AGENTS.md",
    },
    AiCli {
        id: "aider",
        name: "Aider",
        program: "aider",
        prompt_flag: Some("--message"),
        interrupt: b"\x03",
        instructions_file: "CONVENTIONS.md",
    },
];

pub fn ai_cli(id: &str) -> Option<&'static AiCli> {
    AI_CLIS.iter().find(|c| c.id == id)
}

/// IDs of the AI CLIs found on PATH; Claude Code when none are.
pub fn detect_ai_clis() -> Vec<String> {
    let found: Vec<String> = AI_CLIS
        .iter()
        .filter(|c| which::which(c.program).is_ok())
        .map(|c| c.id.to_string())
        .collect();
    if found.is_empty() {
        vec![DEFAULT_CLI.to_string()]
    } else {
        found
    }
}

/// Quotes one argument for the shell that will run it.
fn quote(shell: &str, arg: &str) -> String {
    // Split on both separators: a Windows shell path must be recognised whatever OS parses it.
    let file = shell.rsplit(['/', '\\']).next().unwrap_or(shell).to_lowercase();
    let name = file.strip_suffix(".exe").unwrap_or(&file);
    match name {
        // ponytail: double quotes are dropped for PowerShell/cmd; Windows PowerShell 5.1 mangles them in native args.
        // PowerShell also ends single-quoted strings on the curly quotes ‘ ’ ‚ ‛; doubling any of them escapes it.
        "pwsh" | "powershell" => {
            let mut out = String::from("'");
            for c in arg.chars().filter(|&c| c != '"') {
                if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
                    out.push(c);
                }
                out.push(c);
            }
            out.push('\'');
            out
        }
        "cmd" => format!("\"{}\"", arg.replace(['"', '%'], "")),
        _ => format!("'{}'", arg.replace('\'', r"'\''")),
    }
}

/// The prompt template with `{ticket_path}`, `{ticket_id}` and `{project_dir}` filled in, on one line.
pub fn fill_prompt(prompt: &str, ticket_path: &Path, ticket_id: &str, project_dir: &Path) -> Result<String, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("Set an AI command in Settings first".into());
    }
    Ok(prompt
        .replace("{ticket_path}", &ticket_path.display().to_string())
        .replace("{ticket_id}", ticket_id)
        .replace("{project_dir}", &project_dir.display().to_string())
        .replace(['\n', '\r'], " "))
}

/// Prompt asking the AI CLI to write a new ticket file at `path` for `request`, on one line.
pub fn ticket_request_prompt(request: &str, path: &Path, id: &str) -> String {
    let request = request.replace(['\n', '\r'], " ");
    format!(
        "Create a new ticket for this project. The request is: {request} -- Look through the project files for context first, \
         then write the ticket to {path} and change nothing else. The file must be Markdown that starts with a YAML front \
         matter block between two lines of three dashes, containing: id: '{id}', title: a short title, priority: one of low, \
         medium, high or highest, and run_count: 0. After the front matter add a '## Description' section explaining what \
         needs doing and why, then a '## Test Scope' section listing how to check it is done.",
        path = path.display()
    )
}

/// Prompt asking the AI CLI to change the existing ticket file at `path`, on one line.
pub fn ticket_update_prompt(request: &str, path: &Path, id: &str) -> String {
    let request = request.replace(['\n', '\r'], " ");
    format!(
        "Update the existing ticket at {path} (ticket {id}). The requested change is: {request} -- Look through the \
         project files for context if needed, then edit only that file and change nothing else. Keep its format: a YAML \
         front matter block between two lines of three dashes with id, title, priority (one of low, medium, high or \
         highest) and run_count, followed by a '## Description' section, a '## Test Scope' section and, if it has one, a \
         '## Notes' section. Leave the id, run_count, blocked_by, done_at and runs as they are unless the change \
         asks for them.",
        path = path.display()
    )
}

/// Prompt asking the AI CLI to break the document at `doc` into new ticket files in `dir`, numbered from
/// `first_id`, on one line.
pub fn tickets_from_document_prompt(doc: &Path, dir: &Path, tickets: &Path, first_id: u32) -> String {
    let line = |p: &Path| p.display().to_string().replace(['\n', '\r'], " ");
    format!(
        "Read the document at {doc} (a technical spec, design document or project plan) and break the work it \
         describes into tickets for this project. Look through the project files and the existing tickets under \
         {tickets} first, and skip work that already has a ticket. Write each new ticket as its own file in {dir}, \
         named with a five-digit zero-padded id: start at {first}.md and count up by one, skipping any id that already \
         has a file anywhere under {tickets}. Change nothing else. Each file must be Markdown that starts with a YAML \
         front matter block between two lines of three dashes, containing: id: the file's id in single quotes, \
         title: a short title, priority: one of low, medium, high or highest, and run_count: 0. When a ticket can only \
         start once other new tickets are finished, also add blocked_by: a list of their ids in single quotes. After \
         the front matter add a '## Description' section explaining what needs doing and why, then a '## Test Scope' \
         section listing how to check it is done. Keep each ticket a piece of work small enough to finish in one \
         sitting, and number them in the order they should be done.",
        doc = line(doc),
        dir = line(dir),
        tickets = line(tickets),
        first = crate::ticket::format_id(first_id),
    )
}

/// Shell command that starts `cli` with `extra` arguments and `prompt`, quoted as a single argument for `shell`.
pub fn cli_command(cli: &AiCli, extra: Option<&str>, prompt: Option<&str>, shell: &str) -> String {
    let mut command = cli.program.to_string();
    if let Some(extra) = extra {
        command = format!("{command} {extra}");
    }
    if let Some(prompt) = prompt {
        let flag = cli.prompt_flag.map(|f| format!("{f} ")).unwrap_or_default();
        command = format!("{command} {flag}{}", quote(shell, prompt));
    }
    command
}

/// Command-line flag that runs GhostLane as a completion hook: it touches the file named next and exits.
pub const DONE_FLAG: &str = "--ai-done";
/// Follows the marker when the hook gets the CLI's JSON on stdin (Claude Code): its transcript path goes into the
/// marker so the run's token use can be read back.
pub const HOOK_INPUT_FLAG: &str = "--hook-input";

/// What done mode writes to the marker: the time, then the transcript path from the hook's JSON `input`, if any.
pub fn marker_contents(nanos: u128, input: Option<&str>) -> String {
    let transcript = input
        .and_then(|i| serde_json::from_str::<serde_json::Value>(i).ok())
        .and_then(|v| v.get("transcript_path")?.as_str().map(str::to_string))
        .filter(|p| !p.contains(['\n', '\r']));
    match transcript {
        Some(path) => format!("{nanos}\n{path}"),
        None => nanos.to_string(),
    }
}

/// Tokens a run that started at `since` used, from the transcript named in its hook's marker file.
pub fn run_tokens(marker: &Path, since: std::time::SystemTime) -> Option<crate::ticket::Tokens> {
    let text = std::fs::read_to_string(marker).ok()?;
    let transcript = text.lines().nth(1).filter(|p| p.ends_with(".jsonl"))?;
    let since = since.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    transcript_tokens(&std::fs::read_to_string(transcript).ok()?, since)
}

/// Tokens of the assistant messages in a Claude Code transcript (JSON lines) from `since` (seconds since 1970) on.
/// A message is logged once per content block with the same usage, so each message id counts once.
// ponytail: sub agents keep their own transcripts, so their tokens aren't counted.
pub fn transcript_tokens(jsonl: &str, since: u64) -> Option<crate::ticket::Tokens> {
    let mut seen = std::collections::HashSet::new();
    let mut total = None::<crate::ticket::Tokens>;
    for line in jsonl.lines().filter(|l| l.contains("\"usage\"")) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let at = v["timestamp"]
            .as_str()
            .and_then(|t| crate::ticket::parse_utc(&format!("{}Z", t.get(..19)?)));
        let message = &v["message"];
        if v["type"] != "assistant" || at.is_none_or(|at| at < since) {
            continue;
        }
        if let Some(id) = message["id"].as_str()
            && !seen.insert(id.to_string())
        {
            continue;
        }
        let usage = &message["usage"];
        let n = |k: &str| usage[k].as_u64().unwrap_or(0);
        total.get_or_insert_default().add(crate::ticket::Tokens {
            input: n("input_tokens") + n("cache_creation_input_tokens"),
            cached: n("cache_read_input_tokens"),
            output: n("output_tokens"),
        });
    }
    total
}

/// Where a project's completion hook reports to.
pub struct DoneHook {
    /// The GhostLane executable.
    pub exe: PathBuf,
    /// Touched each time the CLI finishes a turn.
    pub marker: PathBuf,
    /// Claude Code settings file holding the Stop hook.
    pub settings: PathBuf,
}

/// `path` with forward slashes, or `None` if it holds characters some shell would need escaped.
fn hook_path(path: &Path) -> Option<String> {
    let s = path.display().to_string().replace('\\', "/");
    (!s.contains(['"', '\'', '`', '$', '!', '%', '\n', '\r'])).then_some(s)
}

/// Claude Code settings with a Stop hook that runs GhostLane's done mode.
pub fn claude_hook_settings(hook: &DoneHook) -> Option<String> {
    let (exe, marker) = (hook_path(&hook.exe)?, hook_path(&hook.marker)?);
    // The paths hold no quotes or backslashes, so escaping the quotes around them is all JSON needs.
    let command = format!(r#"\"{exe}\" {DONE_FLAG} \"{marker}\" {HOOK_INPUT_FLAG}"#);
    Some(format!(
        r#"{{"hooks":{{"Stop":[{{"hooks":[{{"type":"command","command":"{command}"}}]}}]}}}}"#
    ))
}

/// Arguments that make `cli` run GhostLane's done mode each time it finishes a turn; `None` for CLIs without
/// a hook for that.
pub fn done_hook_args(cli: &AiCli, hook: &DoneHook, shell: &str) -> Option<String> {
    let (exe, marker) = (hook_path(&hook.exe)?, hook_path(&hook.marker)?);
    match cli.id {
        "claude" => Some(format!("--settings {}", quote(shell, &hook_path(&hook.settings)?))),
        // Codex runs `notify` with the turn's JSON as one more argument, which done mode ignores.
        "codex" => Some(format!(
            "-c {}",
            quote(shell, &format!("notify=['{exe}','{DONE_FLAG}','{marker}']"))
        )),
        _ => None,
    }
}

/// Is `program` running somewhere below the shell with process id `shell_pid`?
pub fn runs_program(shell_pid: u32, program: &str) -> bool {
    program_command_line(shell_pid, program).is_some()
}

/// Lowercased command line of `program` running somewhere below the shell with process id `shell_pid`.
/// Matches the executable name or, for Node/Python CLIs, any argument mentioning it.
pub fn program_command_line(shell_pid: u32, program: &str) -> Option<String> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    let program = program.to_lowercase();
    let mentions = |s: &std::ffi::OsStr| s.to_string_lossy().to_lowercase().contains(&program);
    let mut parents = vec![Pid::from_u32(shell_pid)];
    let mut seen = std::collections::HashSet::new();
    while let Some(parent) = parents.pop() {
        for (pid, proc) in sys.processes() {
            if proc.parent() != Some(parent) || !seen.insert(*pid) {
                continue;
            }
            if mentions(proc.name()) || proc.cmd().iter().skip(1).any(|a| mentions(a)) {
                let args: Vec<_> = proc.cmd().iter().map(|a| a.to_string_lossy().to_lowercase()).collect();
                return Some(args.join(" "));
            }
            parents.push(*pid);
        }
    }
    None
}

/// Shell program and arguments. `custom` wins when set.
pub fn detect_shell(custom: Option<&str>) -> (String, Vec<String>) {
    if let Some(s) = custom.map(str::trim).filter(|s| !s.is_empty()) {
        return (s.to_string(), Vec::new());
    }
    if cfg!(windows) {
        for candidate in ["pwsh.exe", "powershell.exe"] {
            if let Ok(path) = which::which(candidate) {
                return (path.display().to_string(), vec!["-NoLogo".into()]);
            }
        }
        (
            std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()),
            Vec::new(),
        )
    } else {
        let fallback = if cfg!(target_os = "macos") {
            "/bin/zsh"
        } else {
            "/bin/bash"
        };
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| fallback.into());
        // Login shell so PATH from the user's profile (where CLIs like `claude` live) is loaded.
        (shell, vec!["-l".into()])
    }
}

/// Program and arguments that make `shell` run `script` and exit.
pub fn script_command(shell: &str, script: &str) -> (String, Vec<String>) {
    let file = shell.rsplit(['/', '\\']).next().unwrap_or(shell).to_lowercase();
    let flags: &[&str] = match file.strip_suffix(".exe").unwrap_or(&file) {
        "pwsh" | "powershell" => &["-NoLogo", "-NoProfile", "-Command"],
        "cmd" => &["/C"],
        // Login shell, like the terminal, so tools on the user's PATH are found.
        _ => &["-lc"],
    };
    let mut args: Vec<String> = flags.iter().map(|f| f.to_string()).collect();
    args.push(script.to_string());
    (shell.to_string(), args)
}

/// Keeps a background process from flashing a console window on Windows.
pub fn no_window(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Shows a desktop notification in the background. The text goes in as arguments or environment variables, never
/// as script, since ticket titles can come from untrusted repos.
pub fn notify(title: &str, body: &str) {
    let mut cmd = notify_command(title, body);
    std::thread::spawn(move || {
        let null = std::process::Stdio::null;
        let _ = no_window(&mut cmd).stdin(null()).stdout(null()).stderr(null()).status();
    });
}

/// Windows toast through Windows PowerShell, which can load the WinRT notification types.
// ponytail: shows under PowerShell's name; GhostLane's own would need a Start menu shortcut with an app id.
const TOAST_SCRIPT: &str = "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, \
     ContentType = WindowsRuntime] > $null; \
     $t = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent(\
     [Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
     $x = $t.GetElementsByTagName('text'); \
     $x.Item(0).AppendChild($t.CreateTextNode($env:GHOSTLANE_TITLE)) > $null; \
     $x.Item(1).AppendChild($t.CreateTextNode($env:GHOSTLANE_BODY)) > $null; \
     [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier(\
     '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe').Show(\
     [Windows.UI.Notifications.ToastNotification]::new($t))";

fn notify_command(title: &str, body: &str) -> std::process::Command {
    if cfg!(windows) {
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", TOAST_SCRIPT])
            .env("GHOSTLANE_TITLE", title)
            .env("GHOSTLANE_BODY", body);
        cmd
    } else if cfg!(target_os = "macos") {
        let mut cmd = std::process::Command::new("osascript");
        cmd.args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv)",
            "-e",
            "end run",
            title,
            body,
        ]);
        cmd
    } else {
        // Some notification daemons read the body as markup.
        let body = body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let mut cmd = std::process::Command::new("notify-send");
        cmd.args(["--app-name=GhostLane", title, &body]);
        cmd
    }
}

/// Opens `dir` in the system's file manager (Explorer, Finder or the desktop's default) without waiting.
pub fn open_folder(dir: &Path) -> std::io::Result<()> {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = std::process::Command::new(program).arg(dir).spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Runs `script` with `shell` in `cwd` and waits. Returns whether it exited with 0, and the last lines of its
/// output.
pub fn run_script(shell: Option<&str>, script: &str, cwd: &Path) -> (bool, String) {
    const TAIL: usize = 40;
    let (program, args) = script_command(&detect_shell(shell).0, script);
    let output = no_window(std::process::Command::new(&program).args(&args).current_dir(cwd))
        .stdin(std::process::Stdio::null())
        .output();
    match output {
        Ok(out) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let lines: Vec<&str> = text.trim_end().lines().collect();
            let mut tail = lines[lines.len().saturating_sub(TAIL)..].join("\n");
            if let Some(code) = out.status.code().filter(|_| !out.status.success()) {
                tail = format!("{tail}\n(exit code {code})").trim_start().to_string();
            }
            (out.status.success(), tail)
        }
        Err(e) => (false, format!("Couldn't run {program}: {e}")),
    }
}

pub fn theme() -> TerminalTheme {
    TerminalTheme::new(Box::new(ColorPalette {
        background: "#1E1F22".into(),
        foreground: "#DCDDDE".into(),
        green: "#A8E6A1".into(),
        bright_green: "#C4F0BF".into(),
        ..Default::default()
    }))
}

pub struct Session {
    pub backend: TerminalBackend,
    pub exited: bool,
    /// Enter still to send after `type_prompt`.
    pending_enter: Option<Instant>,
    /// When the shell (or the program running in it) last printed something.
    last_output: Option<Instant>,
}

/// One shell per open project, keyed by .ghost path. Started lazily.
pub struct Terminals {
    sessions: HashMap<PathBuf, (u64, Session)>,
    next_id: u64,
    tx: Sender<(u64, PtyEvent)>,
    // egui_term's reader threads panic if this receiver is dropped, so it lives as long as the app.
    rx: Receiver<(u64, PtyEvent)>,
}

impl Terminals {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Terminals {
            sessions: HashMap::new(),
            next_id: 0,
            tx,
            rx,
        }
    }

    /// Marks sessions whose shell has exited (they stay until `restart`) and sends delayed Enters.
    pub fn poll(&mut self) {
        let now = Instant::now();
        for (_, s) in self.sessions.values_mut() {
            if s.pending_enter.is_some_and(|at| at <= now) {
                s.pending_enter = None;
                s.backend.process_command(BackendCommand::Write(b"\r".to_vec()));
            }
        }
        while let Ok((id, event)) = self.rx.try_recv() {
            let Some((_, s)) = self.sessions.values_mut().find(|(sid, _)| *sid == id) else {
                continue;
            };
            match event {
                PtyEvent::Exit => s.exited = true,
                PtyEvent::Wakeup => s.last_output = Some(now),
                _ => {}
            }
        }
    }

    /// The session for `key`, if its shell is still alive.
    fn live(&self, key: &Path) -> Option<&Session> {
        self.sessions.get(key).map(|(_, s)| s).filter(|s| !s.exited)
    }

    /// The session for `key`, if its shell is still alive.
    pub fn live_mut(&mut self, key: &Path) -> Option<&mut Session> {
        self.sessions.get_mut(key).map(|(_, s)| s).filter(|s| !s.exited)
    }

    /// When the live session for `key` last printed output.
    pub fn last_output(&self, key: &Path) -> Option<Instant> {
        self.live(key)?.last_output
    }

    /// Process id of the live shell for `key`.
    pub fn shell_pid(&self, key: &Path) -> Option<u32> {
        self.live(key).map(Session::shell_pid)
    }

    /// Is `program` running in the live shell for `key`?
    pub fn runs(&self, key: &Path, program: &str) -> bool {
        self.shell_pid(key).is_some_and(|pid| runs_program(pid, program))
    }

    /// Is there a session for `key`, alive or not?
    pub fn contains(&self, key: &Path) -> bool {
        self.sessions.contains_key(key)
    }

    /// Is there a session for `key` whose shell is still alive?
    pub fn is_live(&self, key: &Path) -> bool {
        self.live(key).is_some()
    }

    /// The session for `key`, starting one in `cwd` if needed. A new session runs `startup` first.
    pub fn get_or_start(
        &mut self,
        key: &Path,
        cwd: &Path,
        shell: Option<&str>,
        startup: Option<&str>,
        ctx: &egui::Context,
    ) -> Result<&mut Session, String> {
        if !self.sessions.contains_key(key) {
            let (shell, args) = detect_shell(shell);
            let settings = BackendSettings {
                shell: shell.clone(),
                args,
                working_directory: Some(cwd.to_path_buf()),
            };
            let id = self.next_id;
            self.next_id += 1;
            let backend = TerminalBackend::new(id, ctx.clone(), self.tx.clone(), settings)
                .map_err(|e| format!("Couldn't start {shell}: {e}"))?;
            let mut session = Session {
                backend,
                exited: false,
                pending_enter: None,
                last_output: None,
            };
            if let Some(cmd) = startup {
                session.run(cmd);
            }
            self.sessions.insert(key.to_path_buf(), (id, session));
        }
        Ok(&mut self.sessions.get_mut(key).expect("just inserted").1)
    }

    pub fn restart(&mut self, key: &Path) {
        self.sessions.remove(key);
    }
}

impl Session {
    /// Sends the CLI's interrupt keys.
    pub fn interrupt(&mut self, keys: &[u8]) {
        self.backend.process_command(BackendCommand::Write(keys.to_vec()));
    }

    pub fn run(&mut self, command: &str) {
        self.backend
            .process_command(BackendCommand::Write(format!("{command}\r").into_bytes()));
    }

    /// Types `text` into a running TUI and presses Enter a moment later; sent in one burst,
    /// many TUIs treat the Enter as part of a paste instead of a submit.
    pub fn type_prompt(&mut self, text: &str, ctx: &egui::Context) {
        const DELAY: Duration = Duration::from_millis(150);
        self.backend
            .process_command(BackendCommand::Write(text.as_bytes().to_vec()));
        self.pending_enter = Some(Instant::now() + DELAY);
        ctx.request_repaint_after(DELAY);
    }

    pub fn shell_pid(&self) -> u32 {
        self.backend.pty_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude() -> &'static AiCli {
        ai_cli("claude").unwrap()
    }

    fn build_ai_command(
        cli: &AiCli,
        prompt: &str,
        path: &Path,
        id: &str,
        dir: &Path,
        shell: &str,
    ) -> Result<String, String> {
        Ok(cli_command(
            cli,
            None,
            Some(&fill_prompt(prompt, path, id, dir)?),
            shell,
        ))
    }

    #[test]
    fn finds_programs_running_below_a_process() {
        let (program, mut child) = if cfg!(windows) {
            let c = std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", "Start-Sleep 10"])
                .spawn();
            ("powershell", c.unwrap())
        } else {
            ("sleep", std::process::Command::new("sleep").arg("10").spawn().unwrap())
        };
        let me = std::process::id();
        let found = runs_program(me, program);
        let missing = runs_program(me, "definitely-not-a-real-cli");
        let _ = child.kill();
        assert!(found);
        assert!(!missing);
    }

    #[test]
    fn builds_command_per_cli_and_shell() {
        let path = Path::new("/p/tickets/to_do/00007.md");
        let prompt = "Do {ticket_id} at {ticket_path} in {project_dir}";
        let posix = build_ai_command(claude(), prompt, path, "00007", Path::new("/p"), "/bin/zsh").unwrap();
        assert_eq!(posix, "claude 'Do 00007 at /p/tickets/to_do/00007.md in /p'");
        let gemini = build_ai_command(ai_cli("gemini").unwrap(), "Go", path, "1", Path::new("/p"), "/bin/bash");
        assert_eq!(gemini.unwrap(), "gemini -i 'Go'");
        let ps = build_ai_command(
            claude(),
            "it's {ticket_id}",
            path,
            "7",
            Path::new("/p"),
            r"C:\pwsh\pwsh.exe",
        );
        assert_eq!(ps.unwrap(), "claude 'it''s 7'");
        let cmd = build_ai_command(claude(), "say \"%PATH%\"", path, "7", Path::new("/p"), "cmd.exe");
        assert_eq!(cmd.unwrap(), "claude \"say PATH\"");
        let pwsh_unix = build_ai_command(claude(), "it's", path, "7", Path::new("/p"), "/usr/bin/pwsh");
        assert_eq!(pwsh_unix.unwrap(), "claude 'it''s'");
        assert!(build_ai_command(claude(), "  ", path, "1", Path::new("/p"), "/bin/sh").is_err());
    }

    fn hook(dir: &str) -> DoneHook {
        DoneHook {
            exe: PathBuf::from(r"C:\Apps\GhostLane\ghostlane.exe"),
            marker: PathBuf::from(format!(r"{dir}\abc.done")),
            settings: PathBuf::from(format!(r"{dir}\abc.json")),
        }
    }

    #[test]
    fn done_hooks_for_claude_and_codex() {
        let h = hook(r"C:\Cfg\hooks");
        let claude = done_hook_args(claude(), &h, "pwsh.exe").unwrap();
        assert_eq!(claude, "--settings 'C:/Cfg/hooks/abc.json'");
        assert_eq!(
            cli_command(self::claude(), Some(&claude), Some("go"), "pwsh.exe"),
            "claude --settings 'C:/Cfg/hooks/abc.json' 'go'"
        );
        let codex = done_hook_args(ai_cli("codex").unwrap(), &h, "/bin/bash").unwrap();
        assert_eq!(
            codex,
            r"-c 'notify=['\''C:/Apps/GhostLane/ghostlane.exe'\'','\''--ai-done'\'','\''C:/Cfg/hooks/abc.done'\'']'"
        );
        assert_eq!(done_hook_args(ai_cli("aider").unwrap(), &h, "/bin/bash"), None);
        let json = claude_hook_settings(&h).unwrap();
        assert_eq!(
            json,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"\"C:/Apps/GhostLane/ghostlane.exe\" --ai-done \"C:/Cfg/hooks/abc.done\" --hook-input"}]}]}}"#
        );
    }

    #[test]
    fn markers_carry_the_transcript_and_runs_count_its_tokens() {
        assert_eq!(marker_contents(7, None), "7");
        assert_eq!(marker_contents(7, Some("not json")), "7");
        assert_eq!(
            marker_contents(7, Some(r#"{"session_id":"s","transcript_path":"C:/t/abc.jsonl"}"#)),
            "7\nC:/t/abc.jsonl"
        );
        let line = |ts: &str, id: &str, out: u64| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"{id}","usage":{{"input_tokens":2,"cache_creation_input_tokens":100,"cache_read_input_tokens":5000,"output_tokens":{out}}}}}}}"#
            )
        };
        let jsonl = [
            line("2024-02-29T12:00:00.000Z", "old", 9), // before the run
            r#"{"type":"user","timestamp":"2024-02-29T12:35:00.000Z","message":{"usage":{"output_tokens":1}}}"#.into(),
            line("2024-02-29T12:35:00.120Z", "a", 30),
            line("2024-02-29T12:35:01.000Z", "a", 30), // same message, next content block
            line("2024-02-29T12:36:00.000Z", "b", 70),
            "garbage with \"usage\"".into(),
        ]
        .join("\n");
        let t = transcript_tokens(&jsonl, 1_709_210_096).unwrap(); // 2024-02-29T12:34:56Z
        assert_eq!((t.input, t.cached, t.output), (204, 10_000, 100));
        assert_eq!(transcript_tokens(&jsonl, 1_909_210_096), None);

        let d = tempfile::tempdir().unwrap();
        let transcript = d.path().join("s.jsonl");
        std::fs::write(&transcript, &jsonl).unwrap();
        let marker = d.path().join("m.done");
        std::fs::write(&marker, format!("1\n{}", transcript.display())).unwrap();
        let since = std::time::UNIX_EPOCH + Duration::from_secs(1_709_210_096);
        assert_eq!(run_tokens(&marker, since), Some(t));
        std::fs::write(&marker, "1").unwrap();
        assert_eq!(run_tokens(&marker, since), None);
    }

    #[test]
    fn notifications_keep_the_text_out_of_the_script() {
        let body = "00012 '; Remove-Item -Recurse C:\\ ; $(rm -rf ~) <b>";
        let cmd = notify_command("GhostLane", body);
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        if cfg!(windows) {
            assert!(args.iter().all(|a| !a.contains("Remove-Item")));
            let env: Vec<_> = cmd.get_envs().collect();
            assert!(env.contains(&(std::ffi::OsStr::new("GHOSTLANE_BODY"), Some(std::ffi::OsStr::new(body)))));
        } else if cfg!(target_os = "macos") {
            assert_eq!(args.last().map(String::as_str), Some(body));
        } else {
            assert!(args.last().is_some_and(|a| a.ends_with("&lt;b&gt;")));
        }
    }

    #[test]
    fn done_hooks_refuse_paths_a_shell_would_expand() {
        for dir in [r"C:\$HOME", r"C:\it's", r#"C:\a"b"#, r"C:\%TEMP%", "C:\\a`b"] {
            let h = hook(dir);
            assert_eq!(done_hook_args(claude(), &h, "/bin/bash"), None, "{dir}");
            assert_eq!(claude_hook_settings(&h), None, "{dir}");
        }
    }

    #[test]
    fn hostile_paths_stay_inside_the_quotes() {
        let bad = Path::new("/tmp/x'; rm -rf ~; echo '/00001.md");
        let posix = build_ai_command(claude(), "{ticket_path}", bad, "1", Path::new("/tmp"), "/bin/bash").unwrap();
        assert_eq!(posix, r"claude '/tmp/x'\''; rm -rf ~; echo '\''/00001.md'");
        let ps = build_ai_command(claude(), "{ticket_path}", bad, "1", Path::new("/tmp"), "powershell.exe").unwrap();
        assert_eq!(ps, "claude '/tmp/x''; rm -rf ~; echo ''/00001.md'");
        let curly = Path::new("C:/p/x’; calc; ‘‚‛/00001.md");
        let ps = build_ai_command(claude(), "{ticket_path}", curly, "1", Path::new("/p"), "pwsh.exe").unwrap();
        assert_eq!(ps, "claude 'C:/p/x’’; calc; ‘‘‚‚‛‛/00001.md'");
    }

    #[test]
    fn ticket_request_prompt_is_one_line_with_path_and_id() {
        let p = ticket_request_prompt("Add login\nwith email", Path::new("/p/tickets/to_do/00009.md"), "00009");
        assert!(!p.contains('\n'));
        assert!(p.contains("Add login with email"));
        assert!(p.contains("/p/tickets/to_do/00009.md"));
        assert!(p.contains("id: '00009'"));
        assert!(p.contains("## Test Scope"));
    }

    #[test]
    fn ticket_update_prompt_names_the_file_and_keeps_ids() {
        let p = ticket_update_prompt("Raise\npriority", Path::new("/p/tickets/done/00003.md"), "00003");
        assert!(!p.contains('\n'));
        assert!(p.contains("Raise priority"));
        assert!(p.contains("/p/tickets/done/00003.md"));
        assert!(p.contains("Leave the id, run_count"));
    }

    #[test]
    fn document_prompt_is_one_line_with_paths_and_first_id() {
        let p = tickets_from_document_prompt(
            Path::new("/docs/spec\nv2.md"),
            Path::new("/p/tickets/to_do"),
            Path::new("/p/tickets"),
            12,
        );
        assert!(!p.contains('\n'));
        assert!(p.contains("/docs/spec v2.md"));
        assert!(p.contains("in /p/tickets/to_do,"));
        assert!(p.contains("start at 00012.md"));
        assert!(p.contains("## Test Scope"));
    }

    #[test]
    fn detection_never_comes_back_empty() {
        let found = detect_ai_clis();
        assert!(!found.is_empty());
        assert!(found.iter().all(|id| ai_cli(id).is_some()));
    }

    #[test]
    fn scripts_run_through_the_shell() {
        assert_eq!(
            script_command(r"C:\Program Files\PowerShell\7\pwsh.exe", "cargo test").1,
            ["-NoLogo", "-NoProfile", "-Command", "cargo test"]
        );
        assert_eq!(script_command("cmd.exe", "x").1, ["/C", "x"]);
        assert_eq!(script_command("/bin/zsh", "make check").1, ["-lc", "make check"]);

        let d = tempfile::tempdir().unwrap();
        let (ok, out) = run_script(None, "echo hello", d.path());
        assert!(ok, "{out}");
        assert!(out.contains("hello"), "{out}");
        let (ok, out) = run_script(None, "exit 3", d.path());
        assert!(!ok);
        assert!(out.ends_with("(exit code 3)"), "{out}");
    }

    #[test]
    fn custom_shell_wins() {
        assert_eq!(detect_shell(Some(" fish ")), ("fish".to_string(), vec![]));
        assert!(!detect_shell(None).0.is_empty());
    }
}
