# CodeForge — Claude Code / Augment Project Instructions

Agent guidance for this repo lives in `.augment/` and is shared between Claude
Code and Augment. **`.augment/` is the single source of truth — do not duplicate
its content here.**

## Rules (always apply)

@.augment/rules/constitution.md
@.augment/rules/behavior.md

## Knowledge base (load on demand)

Curated docs live in `.augment/knowledge/`. It's too large to load wholesale:

1. Before implementing, debugging, or answering system-behavior questions, read
   `.augment/knowledge/toc.md` and use its index to find relevant docs.
2. Read the specific `.augment/knowledge/**/*.md` files it points to before
   acting.
3. When you create a knowledge doc, put it under `.augment/knowledge/` and update
   `toc.md`.

Start with [`.augment/knowledge/architecture/overview.md`](.augment/knowledge/architecture/overview.md).

## Slash commands

`.claude/commands` is a directory symlink to `.augment/commands/` — one set of
command files shared by both tools. Add/edit commands in `.augment/commands/`.
Don't name a command `code-review` (clashes with the built-in `/code-review`).
None are defined yet.

## Permissions (source-controlled)

The Claude Code permission rules for this repo are checked in at
`.claude/settings.json` (read-only/build allowlist plus `ask` gates). Per-machine
overrides go in `.claude/settings.local.json` (gitignored) — don't relax the
shared gates there.

## Build / run / install

```bash
cargo build            # debug
cargo run              # launch the IDE (needs a real TTY)
cargo build --release  # optimized `forge` binary
cargo clippy --all-targets
cargo test
```

The TUI needs a real terminal — it can't be driven from a non-TTY harness. Verify
interactive changes by running `cargo run` in a real terminal.

**Shared-clone install model.** One clone at `/home/bbell/projects/CodeForge`
(NFS) serves the team; `~/.local/bin/forge` runs the installed release binary.
Two roles, decided by whether you can write the clone:
- **Owner** (writable): a fresh `forge` fast-forwards + rebuilds the shared
  binary, so everyone rides the latest.
- **Consumer** (read-only): just runs the shared binary the owner keeps built.

A `cargo build --release` or `scripts/install.sh` therefore affects **every**
user — treat it as a gated action (see the constitution) and say who runs it.

## Tool-name translation (for `.augment/` docs written for Augment)

- `codebase-retrieval` → use Grep/Glob, or the Explore agent for broad searches.
- `view` → use the Read tool.
- "task list" → use TodoWrite.
