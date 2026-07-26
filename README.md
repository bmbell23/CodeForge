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

**One shared clone serves everyone** — you don't clone it yourself. It lives on
the shared filesystem at `/home/bbell/projects/CodeForge`. To enroll (once per
user), just run its installer:

```bash
/home/bbell/projects/CodeForge/scripts/install.sh
```

That one idempotent command:

- **bootstraps your runtime deps** (`scripts/bootstrap-deps.sh`) — installs the
  fragile ones that aren't OS packages and live under your `$HOME`: **nvim**
  (release tarball → `~/.local/nvim`) and the **tree-sitter CLI** (npm global,
  needed by nvim-treesitter to compile parsers). Warns if a system dep is
  missing (`rg git cc make curl node npm tar setsid`);
- installs the **`forge` launcher** at `~/.local/bin/forge` (pointed at the
  shared clone);
- installs the Neovim config under `~/.config/codeforge` (isolated via
  `NVIM_APPNAME` — never touches your personal `~/.config/nvim`; it's a symlink
  into the shared clone, so config updates reach you automatically).

Make sure `~/.local/bin` is on your `PATH`, then run `forge`. First launch also
auto-installs your Neovim plugins (needs network).

**Always up to date, no per-user builds.** `~/.local/bin/forge` is a thin
wrapper, not a bare symlink. Two roles, decided automatically by whether you can
write the shared clone:

- **Owner** (writable clone): a *fresh* `forge` `git pull --ff-only`s and
  rebuilds the shared binary — so the whole team then rides the latest.
- **Consumer** (read-only clone, i.e. everyone else): `forge` just runs the
  shared binary the owner keeps built. It never tries to pull or compile.

Both roles heal their *own* per-user deps on a fresh launch; attaching to a
running session skips all of it. Every step is time-bounded and best-effort — no
network, no problem, it launches what's there. The owner can force a refresh
without relaunching: `git -C /home/bbell/projects/CodeForge pull` then `Ctrl-a r`.

> Because there's one clone and one binary, a coworker on another VM
> (`cforry@co-sf-pe-050`) does nothing but run the installer once and `forge` —
> no clone, no toolchain, no build. Their config, session, and socket stay
> per-user under their own `$HOME`.

Recommended tools for full IDE features: **ripgrep (`rg`)** for grep, **`fd`**
for faster file finding, and LSP servers via `:Mason` inside nvim.

**Font.** CodeForge is a terminal program — it draws characters, but the
*font* is chosen by your terminal emulator (iTerm2, Windows Terminal, Kitty,
your phone's SSH app, …), not by CodeForge or nvim. To use e.g. **Cascadia Code
Mono**, set it in that app's settings; it applies to every pane. CodeForge
needs only a normal monospace font — icons are off by default, so no Nerd Font
is required (that's what those `[]` boxes were).

### Run

```bash
forge                 # startup picker: choose a project from your projects root
forge codeforge       # open a project by name under your projects root
forge ~/src/foo       # open a project by path
```

Bare `forge` shows a **project picker** (type to filter, ↑↓ to select, `Enter`
to open, a **number `1`–`9` to open that row directly**, `Esc`/`Ctrl-c` to
cancel). The same picker opens for `Ctrl-a n` (new window) and `Ctrl-a p`
(switch project). All three panes (nvim, shell, claude) start in the chosen
directory.

**Windows.** Each window is an independent editor/shell/AI workspace for one
project or worktree. `Ctrl-a c` opens a new window (pick its project), `Ctrl-a n`
cycles, `Ctrl-a 1`–`9` jump. The bottom status bar lists them. `Ctrl-a p`
re-homes the *current* window in a different project.

The **projects root** is `$DDN_PROJECTS` if set (from the DDN bashrc), otherwise
the first of `~/projects` or `~/work/projects` that exists.

## Keys

### CodeForge (`Ctrl-a` prefix)

| Keys              | Action                                        |
|-------------------|-----------------------------------------------|
| `Ctrl-a e`        | Show/hide the **editor** pane                 |
| `Ctrl-a t`        | Show/hide the **terminal** pane               |
| `Ctrl-a c`        | Show/hide the **Claude** pane                 |
| `Ctrl-a s`        | New tab in the focused slot (terminal/Claude) |
| `Ctrl-a ]` / `[`  | Next / prev tab in the focused slot           |
| `Ctrl-a w`        | Close the active tab in the focused slot      |
| `Ctrl-a v`        | Copy/scroll mode on the focused pane          |
| `Ctrl-a h/j/k/l`  | Move focus left/down/up/right                 |
| `Ctrl-a o`        | Cycle focus                                   |
| `Ctrl-a p`        | Switch project (re-home window)               |
| `Ctrl-a n`        | New window (pick its project)                 |
| `Ctrl-a X`        | Close current window                          |
| `Ctrl-a 1`–`9`    | Jump to window                                |
| `Ctrl-a d`        | Detach (server keeps running)                 |
| `Ctrl-a r`        | Reload server on latest build                 |
| `Ctrl-a F`        | Forget saved session (fresh next run)         |
| `Ctrl-a ?`        | Keybinding help + live editor                 |
| `Ctrl-a q`        | Quit CodeForge (ends session)                 |
| `Ctrl-a Ctrl-a`   | Send a literal `Ctrl-a` to the child          |
| **Click a pane**  | Focus it (clicks reach nvim too)              |

Keys are rebindable live in the `Ctrl-a ?` overlay (press `e`), no file editing
needed. Each slot (terminal, Claude) can hold several **tabs**; the border shows
`shell 2/3` when a slot is stacked. The editor stays a single nvim — use its
native buffers/tabs (see below) for multiple files.

**Copy / scroll a pane.** `Ctrl-a v` enters copy mode on the focused pane
(scroll back through its output and copy text — works on the terminal and
Claude, where the terminal's own selection can't be copied over SSH):

| Key | Action |
|-----|--------|
| `k` / `j` or ↑ / ↓ | Move up / down (scrolls at the edges) |
| `Ctrl-u` / `Ctrl-d`, PgUp / PgDn | Half-page up / down |
| `g` / `G` | Top of scrollback / live bottom |
| `v` or Space | Start / clear selection |
| `y` or Enter | Copy selection (or the cursor line) — sets the clipboard via OSC 52 |
| `Esc` / `q` | Leave copy mode |

Every other key goes to the focused pane's process. **Exiting a pane's program**
(`exit`, Ctrl-D, `:q`) **respawns it** — the terminal comes back, the editor
reopens, the AI pane returns on `claude --resume`. Panes are never destroyed,
only **shown/hidden**: `Ctrl-a e` / `t` / `c`. The editor is a full-height left
column with the terminal + Claude stacked on the right; hide the editor and the
other two go side by side.

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
