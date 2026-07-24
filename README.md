# CodeForge

A terminal-native IDE, built as a single-binary multiplexer in Rust.

CodeForge hosts your editor (Neovim), a shell, and the [Claude CLI](https://github.com/anthropics/claude-code) in managed panes inside one program. The editing power comes from Neovim running *inside* CodeForge — fuzzy file finding, project-wide grep, find-callers/LSP references, and rename all ride on the Neovim ecosystem. CodeForge's job is the surrounding workbench: pane/tab management, a consistent launcher for editor + terminal + AI, and (later) session persistence across machines.

> **Why not just tmux?** You can absolutely run `nvim` + `claude` in tmux panes today — that's the current workflow. CodeForge is the opinionated, IDE-shaped version of that: one binary that drops you straight into editor + terminal + AI, with a curated Neovim config. The Claude CLI is *invoked*, not embedded — no headless API tokens are burned by the IDE itself.

## Status

**v0.2 — it's an IDE.** Launching `forge` opens the default layout:

```
┌────────────────────┬────────────┐
│                    │   shell    │
│        nvim        ├────────────┤
│                    │   claude   │
└────────────────────┴────────────┘
```

Each pane is a real process in its own PTY, emulated and tiled by CodeForge.
See the [project board](https://github.com/users/bmbell23/projects/8/views/1)
for the roadmap.

## Install

```bash
./scripts/install.sh
```

This builds `forge`, links it into `~/.local/bin`, and installs the Neovim
config under `~/.config/codeforge` (isolated via `NVIM_APPNAME` — it never
touches your personal `~/.config/nvim`). First `forge` launch auto-installs the
Neovim plugins (needs network).

Recommended tools for full IDE features: **ripgrep (`rg`)** for grep, **`fd`**
for faster file finding, and LSP servers via `:Mason` inside nvim.

### Run

```bash
forge                 # open the current directory as the project
forge codeforge       # open a project by name under your projects root
forge ~/src/foo       # open a project by path
```

All three panes (nvim, shell, claude) start in the chosen project directory.
The **projects root** is `$DDN_PROJECTS` if set (from the DDN bashrc), otherwise
the first of `~/projects` or `~/work/projects` that exists — a bare `forge <name>`
resolves against it.

## Keys

### CodeForge (`Ctrl-a` prefix)

| Keys        | Action                               |
|-------------|--------------------------------------|
| `Ctrl-a \|` | Split focused pane side by side      |
| `Ctrl-a -`  | Split focused pane top/bottom        |
| `Ctrl-a h/j/k/l` | Move focus left/down/up/right   |
| `Ctrl-a o`  | Cycle focus                          |
| `Ctrl-a x`  | Close focused pane                   |
| `Ctrl-a q`  | Quit CodeForge                       |
| `Ctrl-a a`  | Send a literal `Ctrl-a` to the child |

Every other key goes to the focused pane's process.

### Neovim (leader = `Space`)

| Keys         | Action                                  |
|--------------|-----------------------------------------|
| `<leader>ff` | Find files                              |
| `<leader>fg` | Find all — live grep across the repo    |
| `<leader>fb` | Switch buffer                           |
| `<leader>e` / `-` | File explorer (oil)                |
| `gd`         | Go to definition                        |
| `gr`         | Find references (callers)               |
| `<leader>rn` | Rename symbol (project-wide)            |
| `<leader>ca` | Code action                             |
| `<leader>rr` | Search & replace in file                |

## Roadmap (near-term)

- Config file (TOML) for keybinds and startup layout
- Diff-based rendering (kill full-screen redraw flicker)
- Status bar; tabs / multiple projects
- Longer horizon: browser-based and Windows front-ends over a shared session

## License

MIT
