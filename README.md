# CodeForge

A terminal-native IDE, built as a single-binary multiplexer in Rust.

CodeForge hosts your editor (Neovim), a shell, and the [Claude CLI](https://github.com/anthropics/claude-code) in managed panes inside one program. The editing power comes from Neovim running *inside* CodeForge — fuzzy file finding, project-wide grep, find-callers/LSP references, and rename all ride on the Neovim ecosystem. CodeForge's job is the surrounding workbench: pane/tab management, a consistent launcher for editor + terminal + AI, and (later) session persistence across machines.

> **Why not just tmux?** You can absolutely run `nvim` + `claude` in tmux panes today — that's the current workflow. CodeForge is the opinionated, IDE-shaped version of that: purpose-built layouts, project awareness, and one binary to configure instead of a pile of dotfiles. The Claude CLI is *invoked*, not embedded — no headless API tokens are burned by the IDE itself.

## Status

**v0.1 — vertical slice.** Single-pane PTY host: spawns a child process, runs its
output through a vt100 emulator, renders it, and forwards your keystrokes. This is
the hard core (terminal-in-terminal) that splits, tabs, and the editor/AI layout
build on. See the [project board](https://github.com/users/bmbell23/projects/8/views/1)
for the roadmap.

## Build & run

```bash
cargo run          # launches your $SHELL inside CodeForge
cargo build --release && ./target/release/forge
```

### Controls (v0.1)

| Keys        | Action                              |
|-------------|-------------------------------------|
| `Ctrl-a q`  | Quit CodeForge                      |
| `Ctrl-a a`  | Send a literal `Ctrl-a` to the child |

`Ctrl-a` is the command prefix (tmux-style). Everything else is forwarded to the
focused child process unchanged.

## Roadmap (near-term)

- Split panes (vertical/horizontal) and focus switching
- A default IDE layout: editor pane + terminal pane + Claude pane
- Config file for keybinds and startup layout
- Diff-based rendering (kill full-screen redraw flicker)
- Tabs / multiple projects
- Longer horizon: browser-based and Windows front-ends over a shared session

## License

MIT
