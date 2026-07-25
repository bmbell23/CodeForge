# CodeForge

A terminal-native IDE, built as a single-binary multiplexer in Rust.

CodeForge hosts your editor (Neovim), a shell, and the [Claude CLI](https://github.com/anthropics/claude-code) in managed panes inside one program. The editing power comes from Neovim running *inside* CodeForge — fuzzy file finding, project-wide grep, find-callers/LSP references, and rename all ride on the Neovim ecosystem. CodeForge's job is the surrounding workbench: pane/tab management, a consistent launcher for editor + terminal + AI, and (later) session persistence across machines.

> **Why not just tmux?** You can absolutely run `nvim` + `claude` in tmux panes today — that's the current workflow. CodeForge is the opinionated, IDE-shaped version of that: one binary that drops you straight into editor + terminal + AI, with a curated Neovim config. The Claude CLI is *invoked*, not embedded — no headless API tokens are burned by the IDE itself.

## Persistence

CodeForge is a **client/server** app, like tmux. A detached background **server**
owns your windows, panes, and scrollback; the `forge` you run is a thin **client**.
Detach with `Ctrl-a d` (or just close the terminal / drop the SSH connection) and
the session keeps running. Reattach by running `forge` again — everything is where
you left it. `Ctrl-a q` ends the session for good.

```bash
forge            # start or attach to your session
# ... Ctrl-a d to detach, or disconnect ...
forge            # reattach — same windows, same state
forge attach     # attach only (error if nothing is running)
```

**Across quit / reboot.** CodeForge snapshots each window to disk: its project
dir, the **shell's working directory**, and the **editor's open files**. After
`Ctrl-a q` or a reboot, a bare `forge` **restores** those windows — nvim reopens
the same files, the shell starts in the same dir, and the AI pane resumes its
conversation via `claude --continue`. (Terminal *scrollback* and editor undo
history don't survive a full teardown — that needs live process/fd handoff.)

- `forge` — restore the last session (or the picker on first run)
- `forge <project>` — start a *fresh* single-window session
- `Ctrl-a r` — reload the server on a newly-built binary, restoring the windows
- `Ctrl-a F` — forget the saved session (next `forge` starts fresh)

The server socket lives at `$XDG_RUNTIME_DIR/codeforge-<user>.sock`; the saved
session at `$XDG_STATE_HOME/codeforge/session`.

## Status

**v0.3 — persistent.** Launching `forge` opens the default layout:

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
forge                 # startup picker: choose a project from your projects root
forge codeforge       # open a project by name under your projects root
forge ~/src/foo       # open a project by path
```

Bare `forge` shows a **project picker** (type to filter, ↑↓ to select, Enter to
open, Ctrl-c to cancel). All three panes (nvim, shell, claude) start in the
chosen directory.

**Windows.** Each window is an independent editor/shell/AI workspace for one
project or worktree. `Ctrl-a c` opens a new window (pick its project), `Ctrl-a n`
cycles, `Ctrl-a 1`–`9` jump. The bottom status bar lists them. `Ctrl-a p`
re-homes the *current* window in a different project.

The **projects root** is `$DDN_PROJECTS` if set (from the DDN bashrc), otherwise
the first of `~/projects` or `~/work/projects` that exists.

## Keys

### CodeForge (`Ctrl-a` prefix)

| Keys        | Action                               |
|-------------|--------------------------------------|
| `Ctrl-a \|` | Split focused pane side by side      |
| `Ctrl-a -`  | Split focused pane top/bottom        |
| `Ctrl-a h/j/k/l` | Move focus left/down/up/right   |
| `Ctrl-a o`  | Cycle focus                          |
| `Ctrl-a x`  | Close focused pane                   |
| `Ctrl-a p`  | Switch project (re-home window)      |
| `Ctrl-a c`  | New window (pick its project)        |
| `Ctrl-a X`  | Close current window                 |
| `Ctrl-a n`  | Next window                          |
| `Ctrl-a 1`–`9` | Jump to window                    |
| `Ctrl-a d`  | Detach (server keeps running)        |
| `Ctrl-a r`  | Reload server on latest build        |
| `Ctrl-a F`  | Forget saved session (fresh next run)|
| `Ctrl-a ?`  | Toggle keybinding help               |
| `Ctrl-a q`  | Quit CodeForge (ends session)        |
| `Ctrl-a a`  | Send a literal `Ctrl-a` to the child |
| **Click a pane** | Focus it (clicks reach nvim too) |

Every other key goes to the focused pane's process. **Exiting a pane's program**
(`exit`, Ctrl-D, `:q`) **respawns it** — the terminal comes back, the editor
reopens, the AI pane returns on `claude --resume`. Only `Ctrl-a x` removes a
pane. Split (`Ctrl-a |` / `-`) for **multiple terminals in one window**.

> **Running inside tmux?** Don't — CodeForge is itself a multiplexer, and tmux
> will intercept `Ctrl-a`. Run `forge` in a plain terminal. (If you must nest it,
> press `Ctrl-a` twice to send one through tmux.)
>
> **First launch** opens the Lazy plugin-manager window (Home/Install/Update…)
> while it installs. Press `q` to close it once it's done, then use the keys below.

### Open a file

Click the nvim pane (or `Ctrl-a h`), then (VS Code style):

- **`Ctrl-P`** — open a file by name (fuzzy)
- **`Ctrl-F`** — search within the current file
- **`Ctrl-Shift-F`** — search the whole repo (falls back to `Space fg` if your
  terminal doesn't send the Shift chord)
- **`Space e`** / **`-`** — file explorer (Enter opens)

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
