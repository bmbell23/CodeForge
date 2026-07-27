---
type: "always_apply"
description: "AI agent behavior, workflow, and communication guidelines for CodeForge"
---

# AI Agent Behavior Guidelines for CodeForge

How AI assistants should work on the CodeForge codebase: communication style, the
ticket workflow, knowledge-base usage, and code-change discipline.

---

## Communication Style

### Be direct — no pleasantries
Lead with the action or answer, not acknowledgment. Banned openers: "You're
absolutely right", "Great question", "Perfect", "Good catch", apologies, and any
validating filler. State facts; when correcting, state the fix, not the mistake.
No emojis in committed text.

> WRONG: "You're absolutely right — let me fix that…"
> RIGHT: "Fixed. `parse_prefix` now rejects Ctrl-S/Ctrl-Q."

### Be explicit about uncertainty
State when you're unsure, flag assumptions, and say when you need more
information. Don't present a guess as fact.

### Explain the *why*
Show reasoning for non-obvious decisions; reference specific files and lines
(`src/main.rs:1861`). When you weigh a choice, give a recommendation, not an
exhaustive survey.

---

## The Ticket Workflow (mandatory)

GitHub Issues on `bmbell23/CodeForge` (Project #8) are the source of truth — not
memory, not local docs. The next session trusts the issues.

1. **Read the board before starting** (`gh issue list`, the project board).
2. **No work without a ticket.** Create it first. Tag `STORY:`/`BUG:` in three
   synced places: title prefix, first line of the description, and the
   `story`/`bug` label.
3. **Board flow, in order — never skip:**
   **Backlog → Ready → In Progress → In Review → Done.**
   - **Backlog** — has open questions; ask the user, never build from a guess.
   - **Ready** — confidently scoped and ready to implement.
   - **In Progress** — being actively worked. **One at a time.**
   - **In Review** — believed done; **uncommitted**.
   - **Done** — the user has blessed it; only then commit.
4. **Scoping can be parallel; implementation is serial.** You may move several
   tickets to Ready in one pass, but only **one** ticket is In Progress / In
   Review at a time.
5. **Any code change → move the ticket to In Review and say so.** In-Review work
   stays **uncommitted**, and the uncommitted diff must match that one ticket.
6. **Done = the user blesses it.** Only then `git commit` (after asking) and move
   to Done. Remind the user of In-Review tickets at each task transition.
7. **Every ticket is documented** before it closes: **Scope** (how it was
   scoped), **Implementation** (what changed + why, with commit SHAs), and
   **Testing** (how it was verified).

Builds/installs are explicit: the launcher runs the *installed* release binary
from the shared clone. If a change only becomes visible after `cargo build
--release` or `scripts/install.sh`, say so plainly and ask who runs it — never
let the user discover a needed rebuild on their own.

---

## Working with the Code

### Gather context first
1. Check the knowledge base — read `.augment/knowledge/toc.md` and load the docs
   its index points to before implementing, debugging, or answering
   system-behavior questions.
2. Use Grep/Glob (or the Explore agent for broad searches) to understand the area
   you're changing; identify dependencies and downstream impact.
3. Follow existing patterns, naming, and error-handling conventions in the file.

### Minimize blast radius
- Make the smallest change that solves the problem.
- Don't "improve" code that isn't broken; don't combine unrelated changes.
- Diagnose root causes, not symptoms (e.g. the reload-runs-a-stale-inode bug was
  fixed at the exec path, not by papering over it).

### Testing philosophy
- `cargo fmt` + `cargo clippy --all-targets` clean; `cargo test` green.
- The TUI needs a real terminal — it can't be driven from a non-TTY harness.
  nvim config changes can be checked with `nvim --headless`, but interactive
  behavior is verified by running `cargo run` in a real terminal. State clearly
  what was verified and how.

---

## Knowledge Base

Curated docs live in `.augment/knowledge/`, indexed by `toc.md`. It's too large
to load wholesale:
1. Search `toc.md` for keywords related to the task.
2. Read the specific `.augment/knowledge/**/*.md` files it points to before
   acting.
3. When you learn something worth keeping, add a doc under
   `.augment/knowledge/` and update `toc.md`.

Don't guess at a subsystem's behavior if it might be documented — check `toc.md`
first.

---

## Interaction Patterns

- **Do what's asked, nothing more** — no unsolicited features or refactors; ask
  before expanding scope.
- **Ask strategically** — clarify genuinely ambiguous requirements or risky
  changes, but when you have enough to act (sensible default exists), act and say
  what you chose. Don't over-ask.
- **When stuck** (looping, can't find info, unsure of the right approach): stop
  and ask.

---

## Status
**Status**: Accepted
**Last Updated**: 2026-07-26
**Review Cycle**: as the project evolves
