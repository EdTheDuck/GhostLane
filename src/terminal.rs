use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

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
    let name = Path::new(shell)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match name.as_str() {
        // ponytail: double quotes are dropped for PowerShell/cmd; Windows PowerShell 5.1 mangles them in native args.
        "pwsh" | "powershell" => format!("'{}'", arg.replace('"', "").replace('\'', "''")),
        "cmd" => format!("\"{}\"", arg.replace(['"', '%'], "")),
        _ => format!("'{}'", arg.replace('\'', r"'\''")),
    }
}

/// `cli`'s command with the prompt template filled in (`{ticket_path}`, `{ticket_id}`, `{project_dir}`)
/// and quoted as a single argument for `shell`.
pub fn build_ai_command(
    cli: &AiCli,
    prompt: &str,
    ticket_path: &Path,
    ticket_id: &str,
    project_dir: &Path,
    shell: &str,
) -> Result<String, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("Set an AI command in Settings first".into());
    }
    let filled = prompt
        .replace("{ticket_path}", &ticket_path.display().to_string())
        .replace("{ticket_id}", ticket_id)
        .replace("{project_dir}", &project_dir.display().to_string())
        .replace(['\n', '\r'], " ");
    let flag = cli.prompt_flag.map(|f| format!("{f} ")).unwrap_or_default();
    Ok(format!("{} {flag}{}", cli.program, quote(shell, &filled)))
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

    /// Marks sessions whose shell has exited; they stay until `restart`.
    pub fn poll(&mut self) {
        while let Ok((id, event)) = self.rx.try_recv() {
            if matches!(event, PtyEvent::Exit)
                && let Some((_, s)) = self.sessions.values_mut().find(|(sid, _)| *sid == id)
            {
                s.exited = true;
            }
        }
    }

    pub fn get_or_start(
        &mut self,
        key: &Path,
        cwd: &Path,
        shell: Option<&str>,
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
            self.sessions
                .insert(key.to_path_buf(), (id, Session { backend, exited: false }));
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude() -> &'static AiCli {
        ai_cli("claude").unwrap()
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
