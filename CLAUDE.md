# CodeForge — AI Assistant Guidelines

## What this project is

CodeForge is a **terminal-native IDE**: a single-binary multiplexer written in
Rust that hosts Neovim, a shell, and the Claude CLI in managed panes. Editing
features come from Neovim running inside CodeForge; CodeForge provides the
workbench (pane/tab management, layouts, launcher, later session persistence).

The Claude CLI is *invoked as a subprocess*, never embedded as a headless API
client — the IDE must not burn API tokens on its own.

Work is tracked on the GitHub Project board:
https://github.com/users/bmbell23/projects/8/views/1 — use `gh` to read/write it.

## Architecture (v0.1)

- `src/main.rs` — the event loop. Spawns a child in a PTY (`portable-pty`),
  parses its output with a vt100 emulator (`vt100`), renders the screen with
  `crossterm`, and forwards raw stdin. `Ctrl-a` is the command prefix.
- Producer threads funnel `Msg`s (child output, user input, resize, quit) into a
  single-threaded event loop over an `mpsc` channel. Keep that single-consumer
  model — it's what makes the render path race-free.

## Conventions

- `cargo fmt` + `cargo clippy` before committing; keep the build warning-clean.
- Prefer `anyhow::Result` with `.context(...)` at call sites for error trails.
- Terminal state (raw mode, alt screen) must always be restored on every exit
  path — the `TerminalGuard` Drop is the single source of truth; don't bypass it.
- Match surrounding comment density and naming. Explain *why*, not *what*.

## Build / run / test

```bash
cargo build            # debug
cargo run              # launch the IDE (needs a real TTY)
cargo build --release  # optimized `forge` binary
cargo clippy --all-targets
```

Note: the TUI needs a real terminal — it can't be driven from a non-TTY harness.
Verify interactive changes by running `cargo run` in a real terminal.

## Safety (host machine)

This runs on bbell's homelab server. General rules still apply:
- Never `sudo reboot` / `shutdown` / `systemctl reboot` to "fix" something —
  diagnose the real cause first (disk space, a single service).
- No destructive DB/container ops without explicit permission and a verified backup.
- When in doubt, ask.
