#!/usr/bin/env bash
# Install CodeForge: build the `forge` binary, link it onto PATH, and install
# the Neovim config under ~/.config/codeforge (isolated via NVIM_APPNAME).
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
NVIM_CFG="$HOME/.config/codeforge"

echo "==> Building forge (release)"
( cd "$REPO" && cargo build --release )

echo "==> Linking $BIN_DIR/forge"
mkdir -p "$BIN_DIR"
ln -sfn "$REPO/target/release/forge" "$BIN_DIR/forge"

echo "==> Installing Neovim config -> $NVIM_CFG"
if [ -e "$NVIM_CFG" ] && [ ! -L "$NVIM_CFG" ]; then
  echo "    !! $NVIM_CFG exists and is not a symlink; leaving it untouched." >&2
  echo "    !! Remove it yourself if you want CodeForge's config there." >&2
else
  ln -sfn "$REPO/config/nvim" "$NVIM_CFG"
fi

echo
echo "Done. Make sure $BIN_DIR is on your PATH, then run: forge"
echo "First launch installs Neovim plugins automatically (needs network)."
echo "Recommended tools for full IDE features: ripgrep (rg), fd; LSP via :Mason."
