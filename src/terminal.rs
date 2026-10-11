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
}

pub const DEFAULT_CLI: &str = "claude";

pub const AI_CLIS: [AiCli; 7] = [
    AiCli {
        id: "claude",
        name: "Claude Code",
        program: "claude",
        prompt_flag: None,
    },
    AiCli {
        id: "codex",
        name: "OpenAI Codex",
        program: "codex",
        prompt_flag: None,
    },
    AiCli {
        id: "gemini",
        name: "Gemini CLI",
        program: "gemini",
        prompt_flag: Some("-i"),
    },
    AiCli {
        id: "copilot",
        name: "GitHub Copilot CLI",
        program: "copilot",
        prompt_flag: Some("-i"),
    },
    AiCli {
        id: "cursor",
        name: "Cursor",
        program: "cursor-agent",
        prompt_flag: None,
    },
    AiCli {
        id: "opencode",
        name: "opencode",
        program: "opencode",
        prompt_flag: Some("--prompt"),
    },
    AiCli {
        id: "aider",
        name: "Aider",
        program: "aider",
        prompt_flag: Some("--message"),
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
        "pwsh" | "powershell" => format!("'{}'", arg.replace('"', "").replace('\'', "''")),
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

/// Shell command that starts `cli` with `prompt`, quoted as a single argument for `shell`.
pub fn cli_command(cli: &AiCli, prompt: &str, shell: &str) -> String {
    let flag = cli.prompt_flag.map(|f| format!("{f} ")).unwrap_or_default();
    format!("{} {flag}{}", cli.program, quote(shell, prompt))
}

/// Is `program` running somewhere below the shell with process id `shell_pid`?
/// Matches the executable name or, for Node/Python CLIs, any argument mentioning it.
pub fn runs_program(shell_pid: u32, program: &str) -> bool {
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
                return true;
            }
            parents.push(*pid);
        }
    }
    false
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
            if matches!(event, PtyEvent::Exit)
                && let Some((_, s)) = self.sessions.values_mut().find(|(sid, _)| *sid == id)
            {
                s.exited = true;
            }
        }
    }

    /// Process id of the live shell for `key`.
    pub fn shell_pid(&self, key: &Path) -> Option<u32> {
        self.sessions
            .get(key)
            .filter(|(_, s)| !s.exited)
            .map(|(_, s)| s.shell_pid())
    }

    /// Is there a session for `key` whose shell is still alive?
    pub fn is_live(&self, key: &Path) -> bool {
        self.sessions.get(key).is_some_and(|(_, s)| !s.exited)
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
        Ok(cli_command(cli, &fill_prompt(prompt, path, id, dir)?, shell))
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

    #[test]
    fn hostile_paths_stay_inside_the_quotes() {
        let bad = Path::new("/tmp/x'; rm -rf ~; echo '/00001.md");
        let posix = build_ai_command(claude(), "{ticket_path}", bad, "1", Path::new("/tmp"), "/bin/bash").unwrap();
        assert_eq!(posix, r"claude '/tmp/x'\''; rm -rf ~; echo '\''/00001.md'");
        let ps = build_ai_command(claude(), "{ticket_path}", bad, "1", Path::new("/tmp"), "powershell.exe").unwrap();
        assert_eq!(ps, "claude '/tmp/x''; rm -rf ~; echo ''/00001.md'");
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
    fn detection_never_comes_back_empty() {
        let found = detect_ai_clis();
        assert!(!found.is_empty());
        assert!(found.iter().all(|id| ai_cli(id).is_some()));
    }

    #[test]
    fn custom_shell_wins() {
        assert_eq!(detect_shell(Some(" fish ")), ("fish".to_string(), vec![]));
        assert!(!detect_shell(None).0.is_empty());
    }
}
