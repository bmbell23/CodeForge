---
type: "always_apply"
description: "CodeForge development governing principles and guardrails"
---

# CodeForge Development Constitution

**Governing principles for the CodeForge terminal-native IDE.**

These principles and guardrails govern all development work on CodeForge. They
override other considerations and apply to every contributor — human and AI. The
`.augment/` tree is the single source of truth; do not duplicate its content
elsewhere.

---

## What CodeForge is

CodeForge is a **terminal-native IDE**: a single-binary multiplexer written in
Rust that hosts Neovim, a shell, and the Claude CLI in managed panes. Editing
features come from Neovim running inside CodeForge; CodeForge provides the
workbench (client/server, pane/tab/window management, layouts, launcher, session
persistence). The Claude CLI is *invoked as a subprocess*, never embedded as a
headless API client — **the IDE must not burn API tokens on its own.**

---

## Core Principles

### 1. Never leave the terminal broken
- **ALWAYS** restore terminal state (raw mode, alt screen, mouse mode) on every
  exit path — the `TerminalGuard` Drop is the single source of truth.
- **NEVER** bypass `TerminalGuard` or add an exit path that skips it.
- A crash must not leave the user with a garbled, unusable terminal.

### 2. The render path stays race-free
- **ALWAYS** keep the single-threaded event loop over one `mpsc` channel:
  producer threads (child output, input, resize, quit) funnel `Msg`s to one
  consumer that owns rendering.
- **NEVER** introduce a second writer to the screen or shared render state.
- **NEVER** let one pane's failure crash the server — isolate and recover
  (e.g. a vt100 panic must not take down the event loop).

### 3. The IDE never spends API tokens
- **ALWAYS** launch Claude as a subprocess in a PTY.
- **NEVER** call an LLM API from CodeForge itself.

### 4. Config and session data are forgiving
- **ALWAYS** give every config field a default; a missing or partial file must
  still work, and older/partial files upgrade in place rather than break.
- **NEVER** lock the user out through config (e.g. a prefix key that freezes the
  session) — validate and fall back with a warning.

### 5. The shared clone is a shared resource
- One clone at `/home/bbell/projects/CodeForge` on NFS serves the whole team; the
  launcher runs the **installed release binary** from it.
- **ALWAYS** treat a `cargo build --release` / `scripts/install.sh` as affecting
  every user — say so and get agreement before doing it.
- **NEVER** relax the shared permission gates in a way other users inherit.

---

## Development Guardrails

### ALWAYS ASK FIRST
Request explicit approval before:
- `git commit` / `git push` (and `gvc`).
- Rebuilding or reinstalling the shared launcher/binary (`cargo build --release`,
  `scripts/install.sh`) — other users ride the shared clone.
- Destructive git operations (`reset --hard`, `clean`, `rm -rf`).
- Deleting or overwriting files you did not create.
- `git tag`, `sudo`, and destructive container/DB operations.

### NEVER DO WITHOUT EXPLICIT PERMISSION
- `sudo reboot` / `shutdown` / `systemctl reboot` to "fix" something — diagnose
  the real cause first (this runs on bbell's homelab server).
- Destructive DB/container operations without a verified backup.
- Bypassing the source-controlled permission rules in `.claude/settings.json`.

---

## Code Quality Standards

- **ALWAYS** run `cargo fmt` and `cargo clippy --all-targets` before committing;
  keep the build warning-clean.
- **ALWAYS** prefer `anyhow::Result` with `.context(...)` at call sites for
  readable error trails.
- **ALWAYS** write/update tests for logic changes; the TUI itself can't be driven
  headlessly, so verify interactive changes by running `cargo run` in a real
  terminal and say so.
- **ALWAYS** match surrounding comment density and naming; explain *why*, not
  *what*.
- **NEVER** submit untested code or disable tests to make a build pass.

---

## Process (source of truth: GitHub Issues)

- **GitHub Issues on `bmbell23/CodeForge` (Project #8) are the source of truth.**
  Plans, scope, and status live in issues — never in local planning `.md` files.
- **No work without a ticket.** Every issue is tagged `STORY:`/`BUG:` in three
  synced places: title prefix, first description line, and the `story`/`bug`
  label.
- Board flow, in order: **Backlog → Ready → In Progress → In Review → Done**;
  never skip columns. See `behavior.md` for the full workflow.

---

## Status
**Status**: Accepted
**Last Updated**: 2026-07-26
**Review Cycle**: as the project evolves
