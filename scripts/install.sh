#!/usr/bin/env bash
# Install CodeForge: build the `forge` binary, link it onto PATH, and install
# the Neovim config under ~/.config/codeforge (isolated via NVIM_APPNAME).
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
NVIM_CFG="$HOME/.config/codeforge"

echo "==> Ensuring runtime deps (nvim, tree-sitter)"
BIN_DIR="$BIN_DIR" bash "$REPO/scripts/bootstrap-deps.sh"

echo "==> Building forge (release)"
( cd "$REPO" && cargo build --release )

echo "==> Linking $BIN_DIR/forge"
mkdir -p "$BIN_DIR"
ln -sfn "$REPO/target/release/forge" "$BIN_DIR/forge"

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
echo "  - forge config:  $NVIM_CFG/config.toml   (written on first run)"
echo "  - first launch installs Neovim plugins automatically (needs network)."
echo "  - recommended: ripgrep (rg), fd; LSP servers via :Mason."
