#!/usr/bin/env bash
# Enroll the current user against a CodeForge clone: ensure their per-user deps,
# link the `forge` launcher onto their PATH, and install the Neovim config under
# ~/.config/codeforge (isolated via NVIM_APPNAME).
#
# One shared clone serves the whole team (e.g. /home/bbell/projects/CodeForge).
# The clone's OWNER (whoever can write it) builds the shared binary; everyone
# else enrolls as a CONSUMER and just runs it. This script figures out which you
# are from the clone's writability — a coworker runs the exact same command.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
NVIM_CFG="$HOME/.config/codeforge"
BIN="$REPO/target/release/forge"

echo "==> Ensuring your runtime deps (nvim, tree-sitter)"
BIN_DIR="$BIN_DIR" bash "$REPO/scripts/bootstrap-deps.sh"

# A writable clone is yours to build (a standalone VM with its own clone); a
# read-only clone is someone else's shared copy, so ride its prebuilt binary.
if [ -w "$REPO/.git" ]; then
  if ! command -v cargo >/dev/null 2>&1; then
    echo "==> Installing Rust toolchain (rustup) to build forge"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    # Use it now; rustup also adds it to your shell profile for future sessions.
    . "$HOME/.cargo/env" 2>/dev/null || export PATH="$HOME/.cargo/bin:$PATH"
  fi
  echo "==> Building forge (release)"
  ( cd "$REPO" && cargo build --release )
else
  echo "==> Consumer mode: read-only clone; using its prebuilt binary"
  if [ ! -x "$BIN" ]; then
    echo "    ERROR: no built binary at $BIN." >&2
    echo "    Ask the clone owner to run scripts/install.sh once to build it." >&2
    exit 1
  fi
fi

echo "==> Installing launcher -> $BIN_DIR/forge"
mkdir -p "$BIN_DIR"
# A self-updating wrapper (not a bare symlink): on a fresh start it pulls +
# rebuilds so coworkers always run the latest. The real binary stays at
# target/release/forge, which is what the server spawns and `reload` restarts.
# rm first: an older install symlinked forge -> the binary, and redirecting into
# that symlink would clobber the binary itself.
rm -f "$BIN_DIR/forge"
sed "s#__REPO__#$REPO#g" "$REPO/scripts/forge-launcher.sh" > "$BIN_DIR/forge"
chmod +x "$BIN_DIR/forge"

echo "==> Installing config -> $NVIM_CFG"
# The config dir is a real directory: forge writes its own config.toml there,
# and we symlink just init.lua so editing the repo updates Neovim live.
if [ -L "$NVIM_CFG" ]; then
  # Migrate an older whole-dir symlink.
  rm -f "$NVIM_CFG"
fi
mkdir -p "$NVIM_CFG"
ln -sfn "$REPO/config/nvim/init.lua" "$NVIM_CFG/init.lua"

echo
echo "Done. Make sure $BIN_DIR is on your PATH, then run: forge"
echo "  - shared clone: $REPO"
echo "  - if you own the clone, a fresh 'forge' pulls + rebuilds it (whole team"
echo "    then rides the latest); consumers just run the shared binary."
echo "  - forge config:  $NVIM_CFG/config.toml   (written on first run)"
echo "  - first launch installs your Neovim plugins automatically (needs network)."
echo "  - recommended: ripgrep (rg), fd; LSP servers via :Mason."
