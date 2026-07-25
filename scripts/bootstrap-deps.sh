#!/usr/bin/env bash
# Ensure CodeForge's runtime dependencies exist. Idempotent: safe to re-run
# after a VM reinstall wipes the ones that live under $HOME.
#
# Two deps are fragile because they are NOT Debian packages here and live under
# $HOME, so a reprovision won't restore them:
#   - nvim          the editor pane (installed as a release tarball -> ~/.local/nvim)
#   - tree-sitter   nvim-treesitter's `main` branch shells out to it to compile
#                   parsers (installed via npm -g)
#
# Everything else CodeForge needs (rg git cc make curl node npm unzip tar setsid)
# comes from the system image; this script only warns if any are missing.
set -euo pipefail

BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
NVIM_DIR="$HOME/.local/nvim"
mkdir -p "$BIN_DIR"

have() { command -v "$1" >/dev/null 2>&1; }

echo "==> Checking system deps"
missing=()
for b in rg git cc make curl node npm tar setsid; do
  have "$b" || missing+=("$b")
done
if [ "${#missing[@]}" -gt 0 ]; then
  echo "   WARNING: missing system deps: ${missing[*]}"
  echo "   install (Debian): sudo apt-get install -y ripgrep git build-essential curl tar util-linux"
  echo "   node/npm: via nvm"
fi

# --- nvim -------------------------------------------------------------------
if have nvim; then
  echo "==> nvim present: $(nvim --version | head -1)"
else
  echo "==> Installing neovim -> $NVIM_DIR"
  arch="$(uname -m)"
  case "$arch" in
    x86_64)  asset="nvim-linux-x86_64.tar.gz" ;;
    aarch64) asset="nvim-linux-arm64.tar.gz" ;;
    *) echo "   ERROR: unsupported arch $arch; install nvim manually" >&2; exit 1 ;;
  esac
  tmp="$(mktemp -d)"
  curl -fsSL -o "$tmp/nvim.tar.gz" \
    "https://github.com/neovim/neovim/releases/latest/download/$asset"
  rm -rf "$NVIM_DIR"
  mkdir -p "$NVIM_DIR"
  tar xzf "$tmp/nvim.tar.gz" -C "$NVIM_DIR" --strip-components=1
  ln -sfn "$NVIM_DIR/bin/nvim" "$BIN_DIR/nvim"
  rm -rf "$tmp"
  echo "   installed: $("$BIN_DIR/nvim" --version | head -1)"
fi

# --- tree-sitter CLI --------------------------------------------------------
if have tree-sitter; then
  echo "==> tree-sitter present: $(tree-sitter --version)"
elif have npm; then
  echo "==> Installing tree-sitter CLI (npm -g)"
  npm install -g tree-sitter-cli
  echo "   installed: $(tree-sitter --version 2>/dev/null || echo '(check PATH)')"
else
  echo "   WARNING: npm missing; cannot install tree-sitter CLI (treesitter parsers won't build)"
fi

echo
echo "Done. Deps ready. Ensure $BIN_DIR is on PATH, then: forge"
