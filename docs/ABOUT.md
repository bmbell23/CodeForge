# About CodeForge

CodeForge is a **terminal-native IDE** — a single Rust binary (`forge`) that
multiplexes **Neovim**, a **shell**, and the **Claude CLI** into managed panes,
tabs, and windows. It is the workbench *around* your editor: client/server
multiplexing, layouts, a project launcher, session persistence, a native git
diff viewer, repo-wide find, and a configurable keymap — all driven from the
terminal, over SSH, with nothing to render but text.

> Press your **`about`** key (default **Ctrl-a i**) to open this page any time.
> It is bundled into the binary, so it always matches the build you're running
> (**v0.3.0**).

---

## Table of contents

1. [Design principles](#design-principles)
2. [Architecture](#architecture)
3. [The rendering pipeline](#the-rendering-pipeline)
4. [Panes, slots, windows, tabs](#panes-slots-windows-tabs)
5. [Neovim integration](#neovim-integration)
6. [Source layout](#source-layout)
7. [Tech stack & dependencies](#tech-stack--dependencies)
8. [Feature tour](#feature-tour)
9. [Keybindings](#keybindings)
10. [Configuration](#configuration)
11. [Build, run, install](#build-run-install)
12. [Files & runtime locations](#files--runtime-locations)
13. [Process & project](#process--project)

---

## Design principles

Five non-negotiables shape every part of CodeForge (from its constitution):

1. **Never leave the terminal broken.** A single `TerminalGuard` restores raw
   mode, the alt screen, and mouse mode on *every* exit path — including panics.
   A crash must never hand you back a garbled, unusable terminal.
2. **The render path stays race-free.** One single-threaded consumer owns all
   rendering, fed by one `mpsc` channel. There is never a second writer to the
   screen. One pane blowing up (e.g. a `vt100` panic) is isolated, not fatal.
3. **The IDE never spends API tokens.** The AI pane launches a CLI (`claude`,
   `auggie`, …) as a *subprocess in a PTY*. CodeForge is not an API client and
   makes no model calls of its own.
4. **Config and session data are forgiving.** Every field has a default; a
   missing or partial `config.toml` still works, and older files upgrade in
   place instead of breaking. A bad prefix key can never lock you out.
5. **The shared clone is a shared resource.** One clone serves a team over NFS;
   a `cargo build --release` affects every user, so builds/installs are explicit
   and gated.

---

## Architecture

### Client / server

Like tmux, CodeForge splits into a persistent server and a thin client:

- **Server** (`forge --server`) owns *all* state — windows, panes, scrollback,
  and the `vt100` screen emulation for every child. It runs detached and
  outlives any client, so an SSH drop or `Ctrl-a d` detach loses nothing.
- **Client** (`forge`) attaches over a per-user Unix socket
  (`$XDG_RUNTIME_DIR/codeforge-<user>.sock`), forwards your keystrokes and
  terminal size, and paints whatever bytes the server sends. It holds no state.
- **Framed protocol** over the socket — `[tag u8][len u32-le][payload]`, one
  `write_all` per frame:

  | Tag | Name | Direction | Payload |
  |-----|------|-----------|---------|
  | 1 | `ATTACH` | client → server | rows, cols (u16 le) |
  | 2 | `INPUT` | client → server | raw input bytes |
  | 3 | `RESIZE` | client → server | rows, cols |
  | 4 | `DETACH` | both ways | — |
  | 5 | `OUTPUT` | server → client | terminal bytes to write |
  | 6 | `RECONNECT` | server → client | — (drop & reconnect after a reload) |

### The event loop (single consumer)

- Each pane's child program runs in its own PTY (`portable-pty`); its output is
  parsed by a `vt100` emulator into a screen grid.
- Producer threads — child output, user input, resize, quit, and nvim
  flag-file watchers — funnel `Msg` values into **one** `mpsc` channel. A single
  consumer drains it and owns rendering. This single-consumer model is *the*
  reason the render path is race-free.
- Messages are **batch-coalesced**: the consumer drains everything queued, sets
  a `dirty` flag, and renders once per batch instead of once per byte.

### nvim ↔ forge signaling

Two directions, both lightweight:

- **forge → nvim:** commands over nvim's `--listen` RPC socket
  (`codeforge-nvim-<id>.sock`) via `nvim --server … --remote-expr …` — used to
  open files, cycle buffers, close buffers, and drive the diff view.
- **nvim → forge:** for events nvim must *originate* (diff view opened,
  grug-far search opened), nvim writes a tiny **flag file** next to its socket
  (`…​.diff`, `…​.grug`); a watcher thread polls it and posts a `Msg`, so forge
  can fullscreen the editor and restore the layout on close.

### Reload & the stale-inode gotcha

`Ctrl-a r` restarts the server on the latest build while keeping your windows.
The subtlety: a `cargo build --release` replaces `target/release/forge` with a
*new inode*; the running process still holds the old one open (silly-renamed to
`.nfsXXXX` on NFS). So the server re-execs `<dir>/forge` **by path**, not via
`current_exe()`, so a reload actually picks up the new binary. Once a stale
server is running, one full `Ctrl-a q` + `forge` breaks out of it.

### Session persistence

Project dir, shell cwd, and open editor files are snapshotted to disk so a bare
`forge` restores your windows after a quit or reboot. `Ctrl-a F` forgets the
saved session (next launch starts from the picker). The just-launched nvim is
*not* queried immediately (it's still loading and its RPC would block); richer
capture happens on change and at teardown.

---

## The rendering pipeline

The typing-latency story, in order:

1. The server composites all visible panes into a full-screen `framebuf`
   (bytes + escape sequences) exactly as if it would repaint everything.
2. That frame is fed to a server-side `vt100` **mirror** of what the client
   currently shows (the `FrameDiffer`, #46). Instead of shipping the whole
   screen, it ships the **minimal cell delta** against the previously presented
   screen — so a keystroke echo repaints a few cells, not ~2000. This was the
   single biggest win against general typing lag.
3. A fresh attach, resize, or clear ships the full `contents_formatted()`
   instead of a diff.
4. Every frame is wrapped in **DEC private mode 2026** (synchronized output) so
   the client presents it atomically — no tearing, no scroll/redraw flicker.

---

## Panes, slots, windows, tabs

- A **window** is one project's workspace: a fixed **editor + terminal + AI**
  trio. There are no arbitrary splits — the editor is a full-height left column;
  the terminal and AI stack in the right column. Hide the editor and the other
  two go side by side. Widths are set by `editor_ratio` and `right_ratio`.
- The terminal and AI **slots** each hold several **tabs** (stacked children) —
  e.g. multiple shells or Claude sessions. The editor stays a single nvim; use
  its native buffers/tabs for multiple files (shown via bufferline).
- **Panes are shown/hidden, never destroyed.** Exiting a pane's program respawns
  it — nvim, the shell, or the AI CLI — so a stray `exit` or crash can't leave a
  dead hole in the layout.
- Multiple windows, one per project; `Ctrl-a 1..9` jumps between them.

---

## Neovim integration

The editing power (fuzzy find, grep, LSP, git diff) is **Neovim running inside
CodeForge**, not a reimplementation.

- **Isolated config.** Launched with `NVIM_APPNAME=codeforge`, so the config
  lives at `~/.config/codeforge/init.lua` and never touches your personal
  `~/.config/nvim`. For consumers it's a symlink to the shared clone's
  `config/nvim/init.lua`, so editor changes reach everyone live.
- **Plugin manager:** [lazy.nvim](https://github.com/folke/lazy.nvim),
  bootstrapped on first run (with a full-clone fallback and a loud error if the
  clone fails, so a plugin-less editor is never silent).
- **Editor-key injection.** CodeForge passes your `[editor_keys]` chords into
  nvim via the `CODEFORGE_EDITOR_KEYS` env var, so the finder/explorer/tab maps
  *and* the splash cheatsheet all read from one source — a rebind can't leave
  the splash showing a stale key.

### Plugins

| Plugin | Role |
|--------|------|
| `telescope.nvim` + `plenary.nvim` | fuzzy file finding, pickers |
| `grug-far.nvim` | live, grouped-by-file repo-wide find & replace ("find all") |
| `nvim-treesitter` | syntax; shells out to the `tree-sitter` CLI to build parsers |
| `nvim-lspconfig` | LSP client config (go-to-def, references, rename) |
| `mason.nvim` + `mason-lspconfig.nvim` | install & wire LSP servers (`:Mason`) |
| `trouble.nvim` | grouped diagnostics & references (find-callers) |
| `diffview.nvim` | git diff / file history views |
| `glance.nvim` | peek definitions/references in a floating window |
| `oil.nvim` | file explorer (edit the filesystem like a buffer) |
| `bufferline.nvim` | the editor's visible buffer "tabs" |
| `alpha-nvim` | the splash / start screen + live cheatsheet |
| `tokyonight.nvim` | colorscheme |

External tools used when present: **ripgrep** (`rg`) for live grep, **fd** for
faster file finding, **git**, and LSP servers via `:Mason`. `.pt` files are
mapped to Perl so perlnavigator attaches.

---

## Source layout

Rust, ~7,250 lines across nine modules:

| File | Lines | Responsibility |
|------|------:|----------------|
| `src/main.rs` | 4,253 | event loop, client/server, protocol wiring, rendering, panes, overlays, all `Msg` handling |
| `src/config.rs` | 837 | config load/persist, `Keys` + `EditorKeys`, prefix parsing, live-rebind, forgiving upgrades |
| `src/gitdiff.rs` | 526 | native git diff model — changed-file list, hunks, side-by-side view (#18) |
| `src/wtform.rs` | 486 | the "new git worktree" form flow (#50) |
| `src/picker.rs` | 330 | fuzzy project picker (startup + live switch) |
| `src/pane.rs` | 256 | a single pane: its PTY child, vt100 screen, feed/respawn |
| `src/layout.rs` | 249 | window layout math (editor column, right-stack split, rects) |
| `src/worktree.rs` | 249 | git worktree creation/listing behind the form |
| `src/protocol.rs` | 64 | the framed client/server wire format |

Other trees: `config/nvim/` (the isolated Neovim config), `scripts/`
(`install.sh`, `bootstrap-deps.sh`, `forge-launcher.sh`), `.augment/`
(development principles, behavior guidelines, and the knowledge base), `docs/`
(this page).

---

## Tech stack & dependencies

A single Cargo binary. Direct crate dependencies:

| Crate | Version | Role |
|-------|---------|------|
| `crossterm` | 0.28 | terminal control, input events, raw/alt-screen, mouse |
| `portable-pty` | 0.8 | spawn child programs (nvim, shell, AI CLI) in PTYs |
| `vt100` | 0.15 | parse child terminal output into a screen grid; frame diffing |
| `signal-hook` | 0.3 | handle resize (SIGWINCH) and teardown signals |
| `serde` (+derive) | 1 | (de)serialize `config.toml` and session snapshots |
| `toml` | 0.8 | parse/emit the config file |
| `chrono` | 0.4 | status-bar clock/date |
| `anyhow` | 1 | error context (`.context(...)`) across call sites |

No async runtime, no LLM SDK — the concurrency is plain threads funneling into
one channel, and the "AI" is whatever CLI you configure.

---

## Feature tour

Grouped by area; `#n` are the tracking issues.

- **Editor / finding** — CodeForge splash with live recents, changed files and a
  gridded cheatsheet (#23); visible buffer tabs + nav (#11); fuzzy open, grep,
  and file explorer; grouped repo-wide find-all via grug-far with an
  auto-fullscreen editor (#53); dotfiles in the finder, last-tab-closes-to-splash
  (#27); leftover `[No Name]` buffer cleanup (#33).
- **LSP** — go-to-definition peek for Rust/Python/C/Bash/Perl (#11); grouped
  find-callers via trouble.nvim (#36); right-click LSP menu + more servers +
  Perl callers (#31, #35, #39); Mason self-heal for the "already linked" loop
  (#48); `.pt`-as-Perl detection (#37).
- **Git diff** — native changed-file panel + side-by-side *editable* diff (#18);
  hunk caching so scrolling doesn't spawn git per notch (#43); `]`/`[` file
  stepping, per-project panel, focus-aware border (#51, #56, #60, #61); git file
  history (#… via diffview).
- **AI pane** — Claude launched as a PTY subprocess; resume per project and a
  resume/fresh chooser on a new Claude tab (#54); start a fresh session in a new
  tab (#49).
- **Panes / tabs / windows** — persistent client/server surviving detach and SSH
  drop (#9); tabbed stacks for the terminal & AI slots (#12); respawn panes on
  child exit (#8); mouse copy + copy/scroll mode for terminal & AI panes
  (#21, #22); a vt100 panic can't crash the server.
- **Picker / worktrees** — project picker at startup and live switch (Ctrl-a p);
  numbered rows, digit-to-open, Esc to cancel; create a git worktree from the
  new-project flow (#50).
- **Status bar** — CPU/RAM/disk metrics with right-side toggles (#16);
  clock/date/temperature; window-tab strip kept to one row (#55).
- **Config / keys** — config file (#5); AI choice, startup panes, autosave
  (#13, #17, #19); configurable editor keys + a full, editable `Ctrl-a ?`
  overlay (#28); a bad prefix can never lock you out.
- **Rendering** — diff-based frames (#46); synchronized output to kill flicker.

---

## Keybindings

Everything is driven by a **command prefix** (default `Ctrl-a`, configurable),
then a single key. Two layers:

- **`[keys]`** — single chars handled by CodeForge itself (pane/tab/window
  management). Defaults:

  | Action | Default | Action | Default |
  |--------|:------:|--------|:------:|
  | focus left / down / up / right | `h` `j` `k` `l` | cycle focus | `o` |
  | show/hide editor · terminal · AI | `e` · `t` · `c` | switch project (picker) | `p` |
  | new tab (focused slot) | `s` | next / prev tab | `]` / `[` |
  | close tab | `w` | copy / scroll mode | `v` |
  | git diff list | `g` | **about (this page)** | **`i`** |
  | new window | `n` | close window | `X` |
  | detach (server lives) | `d` | reload (new build) | `r` |
  | forget session | `F` | quit | `q` |
  | toggle this help | `?` | jump to window 1–9 | `1`…`9` |

- **`[editor_keys]`** — full chords passed through to Neovim. Defaults:

  | Action | Default chord |
  |--------|---------------|
  | open file | `Ctrl-p` |
  | search in file | `Ctrl-f` |
  | search repo (grep) | `Space f g` |
  | file explorer | `Space e` |
  | close editor tab | `Space b d` |
  | git file history | `Space g h` |

Rebind live in the **`Ctrl-a ?`** overlay, or edit `config.toml`; changes
persist and survive reload. Digits `1`–`9` are reserved for window switching, so
they're rejected as bindings.

> These are the *code defaults*. Your own `config.toml` may differ — the overlay
> and the splash always show your live bindings.

---

## Configuration

`~/.config/codeforge/config.toml` (honors `$XDG_CONFIG_HOME`). Every field has a
default and missing fields are appended on launch. Top-level options:

- `prefix` — command prefix (e.g. `"C-a"` or a single char).
- `projects_root` — override the root the picker lists.
- `editor` — editor command line (default `nvim`).
- `shell` — shell command (defaults to `$SHELL`).
- `ai` — the AI CLI command line (default `claude`; e.g. `auggie`). Commands
  starting with `claude` also get `--continue`/`--resume` session handling.
- `editor_ratio` / `right_ratio` — pane split fractions.
- `wrap` — editor line wrapping (off starts nvim with `nowrap`).
- `autosave` — nvim writes on change (default on).
- `start_editor` / `start_terminal` / `start_ai` — which panes open in a new
  window (at least one; editor is forced on if all are false).
- `status_metrics` / `status_weather` / `status_date` / `status_clock` —
  right-side status-bar toggles.
- `weather` — location for the status temperature (empty disables).
- `[keys]` / `[editor_keys]` — the keybinding tables above.

---

## Build, run, install

```bash
cargo build            # debug
cargo run              # launch the IDE (needs a real TTY)
cargo build --release  # optimized `forge` — affects ALL shared-clone users
cargo clippy --all-targets
cargo test
```

**Shared-clone model.** One git clone serves a whole team over NFS:

- The clone's **owner** (whoever can write it) builds
  `target/release/forge`; a fresh `forge` fast-forwards and rebuilds so everyone
  rides the latest.
- **Consumers** enroll with `scripts/install.sh` and just run the prebuilt
  binary. Their launcher is a small self-updating wrapper on `PATH`; `init.lua`
  is symlinked from the clone.

`scripts/bootstrap-deps.sh` ensures per-user deps: **nvim** (release tarball to
`~/.local/nvim`), the **tree-sitter** CLI (via npm), **ripgrep**, and checks for
`git cc make curl node npm tar setsid`. (`node`/`npm` come via nvm and are
needed by some AI CLIs and the tree-sitter install.)

The TUI needs a real terminal — it can't be driven from a non-TTY harness.
Verify interactive changes with `cargo run`; nvim config can be smoke-tested
with `nvim --headless`.

---

## Files & runtime locations

| Path | What |
|------|------|
| `$XDG_RUNTIME_DIR/codeforge-<user>.sock` | the client↔server socket |
| `$XDG_RUNTIME_DIR/codeforge-nvim-<id>.sock` | per-editor nvim RPC socket |
| `…​.diff` / `…​.grug` (next to the nvim socket) | nvim→forge flag files |
| `~/.config/codeforge/config.toml` | your config (prefix, keys, panes, `ai`) |
| `~/.config/codeforge/init.lua` | the Neovim config (symlinked from the clone) |
| `~/.local/share/codeforge/` | nvim data — lazy.nvim & plugins |
| `~/.cache/codeforge/ABOUT.md` | this page, written on open |
| `~/.local/bin/forge` | the launcher wrapper |
| `<clone>/target/release/forge` | the actual server binary |

The AI CLI keeps its own session data (e.g. Claude under
`~/.claude/projects/<key>`, where `<key>` is the project's absolute path with
each non-alphanumeric char turned into `-`).

---

## Process & project

- **Source & issues:** https://github.com/bmbell23/CodeForge — GitHub Issues are
  the source of truth for scope and status (Backlog → Ready → In Progress → In
  Review → Done).
- Development principles, agent behavior guidelines, and the curated knowledge
  base live in the repo's `.augment/` tree, shared by the human and AI
  contributors who work on CodeForge.

_This page is compiled into the binary; it reflects the exact build you're
running._
