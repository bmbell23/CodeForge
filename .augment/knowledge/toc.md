# CodeForge Knowledge Base — Table of Contents

Index to curated documentation for agents working on CodeForge. Consult this
first, then load the specific docs it points to. Don't load the whole tree.

---

## How to use this knowledge base

**Before implementing, debugging, or answering system-behavior questions:**
1. Search this `toc.md` for keywords related to your task.
2. Load the specific `.augment/knowledge/**/*.md` files it points to.
3. Use Grep/Glob (or the Explore agent) for implementation detail in the code.
4. When you create a new knowledge doc, add it here.

---

## Structure

```
.augment/knowledge/
├── toc.md (this file)
├── architecture/   - system-level design (client/server, render path, panes)
├── components/     - individual subsystem docs (config, picker, diff, …)
├── development/    - build/run/install, conventions, workflows
└── features/       - cross-cutting feature docs
```

---

## Architecture

- [`architecture/overview.md`](architecture/overview.md) — **start here.** What
  CodeForge is, the client/server model, the single-consumer event loop, the
  render path, and how panes (editor/shell/AI) are spawned and managed.
  Keywords: client, server, socket, event loop, mpsc, vt100, PTY, render,
  framebuf, TerminalGuard, reload, stale inode.

## Components
_(Add per-subsystem docs here as they're written — e.g. config loading &
keybindings, the project picker, the git-diff panel, the splash screen.)_

## Development

- Build / run / test and the shared-clone install model: see the root
  `CLAUDE.md` "Build / run / install" section (owner vs consumer roles).
  Keywords: cargo build --release, scripts/install.sh, forge launcher, NFS.
  _(Promote to a dedicated `development/build-and-run.md` when it grows.)_

## Features
_(Cross-cutting feature docs go here — e.g. session persistence, copy/scroll
mode, configurable keybindings.)_

---

## Maintenance

When you add a `.md` under `.augment/knowledge/`, add a bullet in the matching
section above with a one-line description and search keywords. Keep entries
short; the file is an index, not the content.
