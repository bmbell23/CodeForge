#!/usr/bin/env bash
# Debounced two-way sync for the Notes repo (#70).
#
# CodeForge's Notes window autosaves .md files into ~/projects/Notes (a real git
# repo). Another device/app may push notes to the same remote. This script keeps
# them in sync: pull the remote in, then commit + push local changes.
#
# Debounce: local changes are only committed once the working tree has been
# *unchanged since the previous run* — so mid-typing bursts collapse into one
# commit. Run every minute:
#
#   * * * * * /home/bbell/projects/CodeForge/scripts/notes-autosync.sh >/dev/null 2>&1
#
# Flags:
#   --now / FORCE=1   sync immediately, skip the idle wait ("Save now")
#
# Conflicts: timestamped-unique filenames make same-file conflicts rare. If a
# rebase does conflict, we abort (leaving a clean, unpushed local state) and drop
# a flag file so CodeForge can offer an interactive resolve (diffview) — we never
# leave a half-merged repo behind a cron job.
set -euo pipefail

FORCE="${FORCE:-0}"
if [ "${1:-}" = "--now" ]; then
  FORCE=1
  shift
fi
DIR="${1:-${NOTES_DIR:-$HOME/projects/Notes}}"
STATE="${XDG_RUNTIME_DIR:-/tmp}/codeforge-notes-sync.state"
CONFLICT_FLAG="${XDG_RUNTIME_DIR:-/tmp}/codeforge-notes-conflict"
# Unix epoch of the last successful sync (push), read by CodeForge's status bar.
SYNCED_AT="${XDG_RUNTIME_DIR:-/tmp}/codeforge-notes-synced-at"

cd "$DIR" 2>/dev/null || exit 0
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || exit 0

# Pull the remote in via rebase. On conflict, abort to keep the tree clean and
# raise the flag; otherwise clear it. Returns non-zero on conflict.
pull_remote() {
  git remote get-url origin >/dev/null 2>&1 || return 0 # no remote: local-only
  if GIT_TERMINAL_PROMPT=0 git pull --rebase -q 2>/dev/null; then
    rm -f "$CONFLICT_FLAG" 2>/dev/null || true
    return 0
  fi
  git rebase --abort 2>/dev/null || true
  : >"$CONFLICT_FLAG"
  return 1
}

status="$(git status --porcelain)"

# Clean tree: just pull so another device's notes show up promptly. Already
# synced, so stamp the time (unless a conflict was raised).
if [ -z "$status" ]; then
  : >"$STATE" 2>/dev/null || true
  if pull_remote; then
    date +%s >"$SYNCED_AT" 2>/dev/null || true
  fi
  exit 0
fi

sync_now() {
  git add -A
  git commit -q -m "notes: auto-sync $(date '+%Y-%m-%d %H:%M:%S')"
  pull_remote || return 0 # conflict flagged; leave the commit unpushed for resolve
  GIT_TERMINAL_PROMPT=0 git push -q || true
  date +%s >"$SYNCED_AT" 2>/dev/null || true
  : >"$STATE" 2>/dev/null || true
}

if [ "$FORCE" = "1" ]; then
  sync_now
  exit 0
fi

# Content-sensitive fingerprint: tracked diff + untracked file contents. While
# you're typing this keeps changing; once you stop it's stable across runs.
fp="$(
  {
    git status --porcelain
    git diff
    git ls-files --others --exclude-standard -z | xargs -0 -r sha1sum
  } | sha1sum | cut -d' ' -f1
)"
prev="$(cat "$STATE" 2>/dev/null || true)"

if [ "$fp" = "$prev" ]; then
  sync_now # unchanged since last run => idle => sync
else
  printf '%s' "$fp" >"$STATE" # still changing => wait for the next run
fi
