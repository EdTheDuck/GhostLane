# 👻 GhostLane

**A project board that lives in your folders, not in someone else's cloud.**

GhostLane is a fast, native project management board for Windows, macOS and Linux. It looks and feels like the Jira-style boards you already know, but every project is just a folder, every column is just a subfolder, and every ticket is just a Markdown file. Your tickets sit right next to your code, they diff cleanly in git, and they're ready for your AI tools to read.

Built in Rust. No account, no server, no subscription.

**[⬇ Download the latest release](../../releases/latest)** for Windows, macOS or Linux.

---

## Why GhostLane?

- **Your files, your board.** Tickets are plain `.md` files with a small YAML header. Open them in any editor, commit them, grep them, back them up. If you stop using GhostLane tomorrow, your data is still in a format you can read.
- **Live sync with your file system.** Move a ticket file in Explorer, Finder or from the terminal and the board updates on its own. Drag a card on the board and the file moves on disk.
- **A terminal built in.** A real shell sits at the bottom of the window, already in your project folder. PowerShell on Windows, your usual shell on macOS and Linux.
- **Send tickets to AI with one click.** Every card has a ▶ button. Press it and GhostLane runs your AI command line tool on that ticket in the built-in terminal. It works with Claude Code out of the box and with any other CLI you configure.
- **Native and light.** One small binary with no browser engine bundled. It starts instantly and stays responsive with large boards.
- **Easy on the eyes.** A dark grey theme with a soft pale green accent, made for long sessions.

---

## Features

### Projects that switch in a click

Open as many projects as you like and switch between them from the dropdown in the top bar. Each project keeps its own board, its own columns and its own terminal session. GhostLane remembers which projects you had open, and if a project's files have been moved or deleted, it tidies the list for you on launch.

### Boards shaped to your workflow

New projects start with **To Do → In Dev → Ready For Test → In Test → Done**. You can change that. When you create or edit a project you can:

- drag columns into any order
- rename them
- add as many as you need
- remove the ones you don't (GhostLane asks where to move any tickets that are still in a removed column)

Each column maps to a folder with a safe, predictable name, so `Ready For Test` becomes `tickets/ready_for_test/`.

### Tickets with everything you need

Every ticket has:

- an **ID** that counts up automatically (`00001`, `00002`, …) and is never reused
- a **title**
- a **priority**: Low, Medium, High or Highest, each with its own colour-coded icon
- a **description** written in Markdown
- an optional **test scope**, so you and your AI know what "done" means
- a **run count** showing how many times the ticket has been sent to AI

### Search and sort every column

Each column header shows a live ticket count and three quick tools:

- 🔍 **Search** filters the column as you type, matching on ID, title, description and test scope.
- ⇅ **Sort** orders tickets by ID, priority or title, ascending or descending. GhostLane remembers your choice for each column.
- ➕ **Add** creates a new ticket directly in that column.

### Read and write tickets comfortably

Click a ticket to open it in a large, clean view with the Markdown fully rendered. Click the pencil to edit. The editor has a formatting toolbar for bold, italic, headings, lists, checklists, code, links and quotes, plus a live preview next to your text. Close without saving and GhostLane asks first, so you won't lose work.

### Right-click shortcuts

Right-click any card to **Edit**, **Duplicate** or **Delete** it. Duplicating opens a new ticket that's already filled in. Deleted tickets go to your Recycle Bin or Trash, so you can get them back.

### A terminal that's always there

The terminal panel sits at the bottom of the window. Drag its top edge to make it bigger or smaller, and the board resizes to fit. Collapse it when you want the full board. It opens in your project folder, so `git`, `cargo`, `npm` and anything else you use are right there.

### Your settings

Click the ⚙ cog to set:

- the **AI command** run by the ▶ button, using the placeholders `{ticket_path}`, `{ticket_id}` and `{project_dir}`
- which **shell** to use (detected automatically by default)
- the **terminal font size**
- the **UI scale** for the whole app

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

You can write tickets like this by hand, and GhostLane will pick them up straight away.

---

## Getting started

1. **Download** the build for your platform from the [releases page](../../releases/latest) and run it.
2. Click **New Project**, name it, and choose a folder. This can be an existing code repository.
3. Adjust the columns if you want to, then click **Create**.
4. Press **➕** on a column to add your first ticket.
5. Drag cards across the board as work moves along, and press **▶** when you want AI to take a ticket on.

To open an existing project, click **Open Project** and select its `.ghost` file.

---

## Build from source

You need a recent stable [Rust toolchain](https://rustup.rs).

```sh
git clone <this repo>
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

---

*GhostLane: work that moves quietly from lane to lane.* 👻
