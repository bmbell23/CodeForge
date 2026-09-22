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
  if [ -w "$REPO/.git" ] && command -v git >/dev/null 2>&1; then
    (
      flock -n 9 || exit 0
      before=$(git -C "$REPO" rev-parse HEAD 2>/dev/null)
      timeout 15 git -C "$REPO" pull --ff-only -q 2>/dev/null || true
      after=$(git -C "$REPO" rev-parse HEAD 2>/dev/null)
      # Only rebuild when the pull actually advanced HEAD (or the binary is
      # missing). A release build has LTO and takes tens of seconds; doing it on
      # every launch made a cold start look hung. Local dev builds with cargo
      # directly, so this never masks your own edits.
      if command -v cargo >/dev/null 2>&1 && { [ "$before" != "$after" ] || [ ! -x "$BIN" ]; }; then
        # Say what's happening (#117). The build is still synchronous — the
        # whole point of it is that nobody rides a stale binary — but a silent
        # twenty seconds is indistinguishable from a hang, which is what made
        # this read as "CodeForge is slow to start" rather than "it's building".
        log="$(mktemp)"
        ( cd "$REPO" && cargo build --release -q ) >"$log" 2>&1 &
        build=$!
        start=$SECONDS
        if [ -t 2 ]; then
          # A real terminal gets a live counter on one rewritten line.
          spin='-\|/'
          i=0
          while kill -0 "$build" 2>/dev/null; do
            i=$(( (i + 1) % 4 ))
            printf '\r\033[2Kcodeforge: building update %s %ds' \
              "${spin:$i:1}" "$(( SECONDS - start ))" >&2
            sleep 0.2
          done
          printf '\r\033[2K' >&2
        else
          # Piped or scripted: one line, no animation.
          echo "codeforge: building update..." >&2
        fi
        if wait "$build"; then
          echo "codeforge: updated in $(( SECONDS - start ))s" >&2
        else
          # Never fail silently into the old binary: that is exactly how users
          # end up running something main doesn't say (constitution principle 6).
          echo "codeforge: update build FAILED - launching the previous binary" >&2
          sed -n '1,20p' "$log" >&2
          echo "codeforge: full log at $log" >&2
          keep_log=1
        fi
        [ -n "${keep_log:-}" ] || rm -f "$log"
      fi
    ) 9>"${XDG_RUNTIME_DIR:-/tmp}/codeforge-update.lock"
  fi
fi

# A moved or deleted clone is a different problem from an unbuilt one, and the
# fix is different too — say which (#130). The launcher lives on $PATH outside
# the clone, so it cannot find where the clone went; install.sh is the answer.
if [ ! -d "$REPO" ]; then
  echo "codeforge: the clone is no longer at $REPO" >&2
  echo "  it was moved or removed. From its new location, run:" >&2
  echo "      <new-path>/scripts/install.sh" >&2
  echo "  that rewrites this launcher and re-links the Neovim config." >&2
  exit 1
fi

if [ ! -x "$BIN" ]; then
  echo "codeforge: binary missing at $BIN" >&2
  if [ -w "$REPO/.git" ]; then
    echo "  build it with: ( cd $REPO && cargo build --release )" >&2
  else
    echo "  ask the clone owner to run scripts/install.sh once to build it." >&2
  fi
  exit 1
fi
exec "$BIN" "$@"
