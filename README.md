# 👻 GhostLane

**Plan the work. Hand it to your AI. Watch it move across the board.**

AI coding tools are great at a well-scoped task and shaky at a vague, sprawling one. Ask for "add favourites to the app" in one prompt and you get a giant diff, half-finished edges and no clear way to check it. Split the same change into ten small tickets, each with a clear description and a test scope, and the same AI gets far more reliable.

GhostLane is a project board built for that way of working. Your AI writes the tickets, you shape them, and one click sends a ticket to Claude Code, Codex, Gemini or whichever CLI you use, running in a terminal right under the board. Every ticket is a Markdown file in your repo, so the plan sits next to the code, diffs cleanly in git, and is something your AI can read for context.

Native app for Windows, macOS and Linux, written in Rust. No account, no server, no subscription.

**[⬇ Download the latest release](../../releases/latest)**

![The GhostLane board with Claude Code running in the built-in terminal](docs/screenshots/board.png)

---

## Why GhostLane for AI work?

- **Big changes become a queue of small prompts.** Break a feature into tickets, give each one a test scope, then hand them to your AI one at a time. You review ticket 3 while ticket 4 runs, and nothing gets lost in a scrollback.
- **Your AI writes the tickets.** Describe what you want in a sentence. GhostLane asks the project's AI CLI to read the codebase and write a full ticket with a priority, a description and a test scope.
- **One click to run.** The ▶ button on every card sends that ticket to your AI CLI. If the CLI is already open in the terminal, GhostLane types the prompt straight into it.
- **The AI can see the plan.** Tickets live in `tickets/` inside your project, so your AI can read past and upcoming work, and can create or edit tickets itself. The board picks up every change the moment the file changes.
- **Bring your own AI.** Claude Code, OpenAI Codex, Gemini CLI, GitHub Copilot CLI, Cursor, opencode and Aider are built in. Each project picks its own.
- **Plain files, no lock-in.** A ticket is a `.md` file with a short YAML header. Grep it, commit it, edit it in any editor. If you stop using GhostLane tomorrow, your plan is still readable.

---

## Features

### AI-assisted tickets

Press **➕** on any column and the New Ticket dialog opens on the **AI Assisted** tab. Type what the ticket should do and press **Generate Ticket**.

![Describing a new ticket for the AI to write](docs/screenshots/ai-new-ticket.png)

GhostLane sends the request to the project's AI CLI in the terminal. The AI looks through the project for context, then writes the ticket file. While it works, the board shows a Generating card and the ghost in the title bar eats a trail of dots. The terminal stays free, so you can approve any permission prompts the AI asks for.

![A ticket being generated, with the AI working in the terminal below](docs/screenshots/generating.png)

When the file appears, the new ticket lands on the board. If the CLI exits early or something goes wrong, GhostLane tells you what happened.

Existing tickets work the same way. Open a ticket, click the pencil, and the **AI Assisted** tab lets you say what should change ("raise the priority and add a test for expired links"). The AI updates the file in place and keeps its ID and run count.

Prefer to write it yourself? The **Manual** tab is always one click away.

### Send a ticket to AI with one click

Every card has a ▶ button. Press it and GhostLane builds a prompt from your template (by default *"Complete the ticket at {ticket_path}"*) and hands it to the project's AI CLI in the terminal. The ticket's run count goes up, so you can see at a glance what has already been attempted.

When you open a project, GhostLane starts its AI CLI in the terminal for you, so it's ready before you need it.

### A terminal that's always there

A real shell sits at the bottom of the window, opened in your project folder: PowerShell on Windows, your usual shell on macOS and Linux. `git`, `cargo`, `npm` and your AI CLI are right there. Drag the top edge to resize it, or collapse it when you want the whole board.

### A visual ticket editor

Tickets are edited like a document, not like source code. Headings, bullet and numbered lists, checklists, quotes, code blocks, bold, italic, inline code and links all show formatted as you type. Markdown shortcuts such as `- ` or `## ` format the line for you, and Ctrl+B and Ctrl+I work as you'd expect.

![The visual ticket editor](docs/screenshots/editor.png)

Behind the scenes it's still plain Markdown. If a ticket uses Markdown the editor can't show (a table, say), that field opens as plain text so nothing gets lost, and fields you don't touch are saved exactly as they were.

### Tickets with everything you need

Every ticket has:

- an **ID** that counts up automatically (`00001`, `00002`, …) and is never reused
- a **title**
- a **priority**: Low, Medium, High or Highest, each with its own colour-coded icon
- a **description**
- an optional **test scope**, so you and your AI agree on what "done" means
- a **run count** showing how many times the ticket has been sent to AI

Click a card to read it with the Markdown fully rendered. Right-click it to **Edit**, **Duplicate** or **Delete**. Deleted tickets go to your Recycle Bin or Trash, so you can get them back.

### Boards shaped to your workflow

New projects start with **To Do → In Dev → Ready For Test → In Test → Done**. Click the pencil next to the project name to change that: drag columns into a new order, rename them, add more or remove some. If a column you remove still has tickets, GhostLane asks where to move them first. This is also where you pick the project's AI CLI.

![Editing a project's columns and AI CLI](docs/screenshots/project.png)

Each column is a folder with a safe, predictable name, so `Ready For Test` becomes `tickets/ready_for_test/`. Drag a card to another column and the file moves on disk. Move a file in Explorer, Finder or the terminal and the card moves on the board.

### Search and sort every column

Each column header shows a live ticket count and three quick tools:

- 🔍 **Search** filters the column as you type, matching on ID, title, description and test scope.
- ⇅ **Sort** orders tickets by ID, priority or title, ascending or descending. GhostLane remembers your choice for each column.
- ➕ **Add** creates a new ticket directly in that column.

### Projects that switch in a click

Open as many projects as you like and switch between them from the dropdown in the title bar. Each project keeps its own board, its own columns and its own terminal session. GhostLane remembers which projects you had open, and if a project's files have been moved or deleted, it drops it from the list on launch.

### Your settings

Click the ⚙ cog to choose:

- which **AI CLIs** are available to your projects (installed ones are switched on for you the first time)
- the **AI command** the ▶ button sends, with the placeholders `{ticket_path}`, `{ticket_id}` and `{project_dir}`
- which **shell** to use (detected automatically by default)
- the **terminal font size**

![The settings dialog](docs/screenshots/settings.png)

For safety, GhostLane never puts a ticket's title or text into the command, and refuses to run if a path or ID contains shell characters such as quotes, `$`, `;` or `|`. That matters when you open a repo full of tickets someone else wrote.

### Easy on the eyes

Dark greys with a soft pale green accent, made for long sessions. The title bar is drawn in the same style, with the window controls built in.

---

## How it looks on disk

```
my-project/
├── my_project.ghost        ← project settings (YAML)
└── tickets/
    ├── to_do/
    │   ├── 00001.md
    │   └── 00004.md
    ├── in_dev/
    │   └── 00002.md
    ├── ready_for_test/
    ├── in_test/
    └── done/
        └── 00003.md
```

A ticket file looks like this:

```markdown
---
id: 00001
title: Login page
priority: high
run_count: 2
---

## Description

Build the login form with email and password fields.

## Test Scope

- valid credentials log the user in
- invalid credentials show an error
```

You, your teammates or your AI can write tickets like this by hand, and GhostLane picks them up straight away. Try asking your AI in the terminal to split a feature into tickets and watch them land in To Do.

---

## Getting started

1. **Download** the build for your platform from the [releases page](../../releases/latest) and run it.
2. Click **New Project**, give it a name and choose a folder. An existing code repository works well, since that's what your AI will read.
3. Pick the project's AI CLI, adjust the columns if you like, then click **Create**.
4. Press **➕** on a column and describe your first ticket on the **AI Assisted** tab, or write it yourself on **Manual**.
5. Press **▶** to hand a ticket to your AI, and drag cards across the board as the work moves along.

To open an existing project, click **Open Project** and select its `.ghost` file.

---

## Build from source

You need a recent stable [Rust toolchain](https://rustup.rs).

```sh
git clone https://github.com/EdTheDuck/GhostLane.git
cd GhostLane
cargo build --release
```

On Linux, install the GUI and dialog dependencies first, for example on Debian or Ubuntu:

```sh
sudo apt install libxkbcommon-dev libgtk-3-dev libwayland-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev
```

---

## Built with

[egui](https://github.com/emilk/egui) · [egui_commonmark](https://github.com/lampsitter/egui_commonmark) · [egui_term](https://github.com/Harzu/egui_term) · [notify](https://github.com/notify-rs/notify) · [rfd](https://github.com/PolyMeilex/rfd) · [trash](https://github.com/Byron/trash-rs)

Bold text uses [Ubuntu Bold](https://design.ubuntu.com/font) under the Ubuntu Font Licence.

---

*GhostLane: work that moves quietly from lane to lane.* 👻
