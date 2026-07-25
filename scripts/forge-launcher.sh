#!/usr/bin/env bash
# CodeForge launcher. install.sh copies this to ~/.local/bin/forge with __REPO__
# replaced by the shared clone path. Local edits here are overwritten on the
# next install.
#
# One shared clone serves the whole team. Two roles, decided by whether you can
# write the clone:
#   - OWNER (writable clone): a fresh launch fast-forwards the repo and rebuilds
#     the shared binary, so everyone rides the latest.
#   - CONSUMER (read-only clone): just runs the shared binary the owner keeps
#     built. Never tries to pull/build (it couldn't anyway).
# Both roles heal their OWN per-user deps (nvim, tree-sitter live under $HOME).
# Attaching to a running server skips all of this. Every step is best-effort and
# time-bounded so a network/build hiccup never blocks launch.
set -uo pipefail

REPO="__REPO__"
BIN="$REPO/target/release/forge"

# A server already running for this user => we're attaching; leave it alone.
# (pgrep -f matches only the '--server' child, not this wrapper.)
if ! pgrep -u "$(id -u)" -f 'forge --server' >/dev/null 2>&1; then
  # Every user needs their own nvim / tree-sitter (they live under $HOME).
  [ -x "$REPO/scripts/bootstrap-deps.sh" ] \
    && bash "$REPO/scripts/bootstrap-deps.sh" >/dev/null 2>&1 || true

  # Only a writable clone (the owner) updates the shared binary. A lock
  # serializes the owner's concurrent launches; the branch is unreachable for
  # read-only consumers, so no build race between users.
  if [ -w "$REPO/.git" ]; then
    (
      flock -n 9 || exit 0
      command -v git >/dev/null 2>&1 \
        && timeout 15 git -C "$REPO" pull --ff-only -q 2>/dev/null || true
      command -v cargo >/dev/null 2>&1 \
        && ( cd "$REPO" && cargo build --release -q ) 2>/dev/null || true
    ) 9>"${XDG_RUNTIME_DIR:-/tmp}/codeforge-update.lock"
  fi
fi

if [ ! -x "$BIN" ]; then
  echo "codeforge: binary missing at $BIN — ask the owner to run scripts/install.sh" >&2
  exit 1
fi
exec "$BIN" "$@"
