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

## Working rules (same process as GreatReads)

Repo: https://github.com/bmbell23/CodeForge · Board: **Project #8 "CodeForge"**
(https://github.com/users/bmbell23/projects/8)

- **GitHub Issues are the source of truth.** Plans, scoping, next-steps, and status
  live in issues — never in local planning `.md` files. Read `gh issue list` before
  starting work; comment/close/edit issues as work moves. The issues — not memory,
  not docs — are what the next session trusts.
- **Every issue is tagged `STORY:` or `BUG:`** in three synced places: title prefix,
  first line of the description, and the `story`/`bug` label.
- **No work without a ticket.** Create the ticket before touching code.
- **Board flow, in order: Backlog → Ready → In progress → In review → Done**
  (this board's names for GreatReads' Scoping → Ready to Implement → …).
  Never skip columns. New tickets land in Backlog (open questions — ask the user,
  never build from a guess) or Ready (confidently scoped).
- **ONE active ticket at a time** in In progress + In Review (committed "watch"
  tickets sitting in In Review don't count). If the user pivots while an active
  uncommitted ticket is In Review, STOP and resolve it first.
- **Any code change → move the ticket to In Review and say so.** In-Review work
  stays **uncommitted**; uncommitted changes must match the one active ticket.
- **Done = the user blesses it.** Only then commit.
- **Builds/installs are explicit:** the launcher runs the *installed* `forge`
  from the shared clone's release build — if a change only becomes visible after
  `cargo build --release` or `./scripts/install.sh`, say so plainly and ask who
  runs it. Never let the user discover a rebuild was needed on their own.
- **Gated actions (always ask first):** (1) `git commit` / `gvc` / `git push`,
  (2) reinstalling the shared launcher/binary (`./scripts/install.sh`) — other
  users ride the shared clone, (3) destructive git ops (`reset --hard`, `clean`,
  `rm -rf`).
- Remind the user of In-Review tickets at every task transition.

## Permissions (source-controlled)

The Claude Code permission rules for this repo are **checked in** at
`.claude/settings.json` — the read-only/build allowlist plus `ask` gates,
adapted from `GreatReads/.claude/settings.json` and merged with the home-level
`~/.claude/settings.json` ask rules (git tag, `rm -rf`, `sudo`, docker) so the
home rules are versioned here too. Per-machine overrides belong in
`.claude/settings.local.json`, which is gitignored — don't relax the shared
gates there.

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
