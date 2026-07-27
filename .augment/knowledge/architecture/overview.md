# Architecture Overview (v0.3)

What CodeForge is and how its pieces fit together. Load this before working on
the event loop, rendering, panes, or the client/server split.

## The shape

CodeForge is a single Rust binary (`forge`) that acts as a terminal multiplexer
hosting three kinds of pane — the **editor** (Neovim), a **shell**, and the
**Claude CLI** — for one project per *window*. Editing power (fuzzy find, grep,
LSP) comes from Neovim running *inside* CodeForge via an isolated config
(`NVIM_APPNAME=codeforge`, `config/nvim/`). CodeForge is the workbench around it.

The Claude CLI is launched as a subprocess in a PTY — never called as a headless
API client. The IDE must not spend API tokens on its own.

## Client / server

Like tmux, CodeForge is a **client/server** app:

- A detached **server** (`forge --server`) owns all state: windows, panes,
  scrollback, and the vt100 screen emulation. It survives client exit / SSH drop.
- The `forge` the user runs is a thin **client** that attaches over a unix socket
  (`$XDG_RUNTIME_DIR/codeforge-<user>.sock`) and renders frames the server sends.
- Framed protocol over the socket (ATTACH, INPUT, RESIZE, DETACH, OUTPUT,
  RECONNECT).
- `Ctrl-a d` detaches; `Ctrl-a r` reloads the server on the latest build,
  restoring windows; `Ctrl-a q` ends the session. Session state (project dir,
  shell cwd, open editor files) is snapshotted to disk so a bare `forge` restores
  windows after quit/reboot.

## The event loop (keep it single-consumer)

- Each pane's child runs in its own PTY (`portable-pty`); its output is parsed by
  a vt100 emulator (`vt100`).
- Producer threads funnel `Msg`s (child output, user input, resize, quit) into a
  single `mpsc` channel. **One** single-threaded consumer drains it and owns all
  rendering. This single-consumer model is what makes the render path race-free —
  do not add a second writer.
- Renders are batch-coalesced (`if dirty`): the server draws the whole screen into
  a `framebuf` and ships it to the client, which writes + flushes. Frames are
  wrapped in DEC private mode 2026 (synchronized output) to present atomically and
  reduce flicker.
- A single pane's failure (e.g. a vt100 panic) must be isolated — it must not
  crash the event loop or the server.

## Panes, slots, windows

- A **window** is a fixed editor + terminal + AI trio for one project. No arbitrary
  splits: the editor is a full-height left column; the terminal and AI stack on
  the right. Hide the editor and the other two go side by side.
- The terminal and AI **slots** can each hold several **tabs** (stacked children).
  The editor stays a single nvim; use its native buffers/tabs for multiple files.
- Panes are shown/hidden, never destroyed: exiting a pane's program respawns it
  (`build_editor` for nvim, the shell, `claude --resume` for AI).
- `Ctrl-a` is the command prefix (configurable). Keybindings live in
  `src/config.rs` (`Keys` for prefix actions, `EditorKeys` for nvim keys injected
  via env); the `Ctrl-a ?` overlay edits them.

## Gotchas

- **Reload must re-exec the stable binary path.** A `cargo build --release`
  replaces `target/release/forge` with a new inode; the running process keeps the
  old one open (silly-renamed to `.nfsXXXX` on NFS, marked " (deleted)" on local
  fs), so `current_exe()` points at the *stale* binary. `spawn_server` re-execs
  `<dir>/forge` by path so `Ctrl-a r` actually picks up new builds. Once a stale
  server is running, one full `Ctrl-a q` + `forge` is needed to break out of it.
- The TUI needs a real TTY — it can't be driven from a non-TTY harness. Verify
  interactive changes with `cargo run` in a real terminal; nvim config can be
  smoke-tested with `nvim --headless`.

## Where things live

- `src/main.rs` — event loop, protocol, server/client, rendering, panes, overlays.
- `src/config.rs` — config load/persist, `Keys` + `EditorKeys`, prefix parsing.
- `src/picker.rs` — the fuzzy project picker.
- `config/nvim/init.lua` — the isolated Neovim config (telescope, oil, bufferline,
  LSP, alpha splash, and the CodeForge key injection).
- `scripts/install.sh`, `scripts/bootstrap-deps.sh` — shared-clone install + deps.
