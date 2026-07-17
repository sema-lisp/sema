#!/usr/bin/env bash
# Build the single cross-platform Sema MCP Bundle (.mcpb) from the per-target
# binaries cargo-dist attaches to a GitHub release.
#
# One .mcpb ships every OS. MCPB selects a binary per OS (darwin/win32/linux)
# via `mcp_config.platform_overrides`, but has NO arch dimension, so:
#   - macOS  : the two arch binaries are lipo'd into ONE universal binary.
#   - Linux  : both arch binaries ship; `mcpb/sema-linux` (a committed shim)
#              picks the right one at runtime via `uname -m`.
#   - Windows: x86_64 only.
#
# Usage:
#   scripts/pack-mcpb.sh [--tag vX.Y.Z] [--out DIR] [--upload]
#   scripts/pack-mcpb.sh --from-dir DIR --version X.Y.Z [--out DIR]
#
#   --tag       release tag to download assets from (default: latest tag).
#   --from-dir  skip download; DIR already holds the 5 target binaries named
#               by triple (sema-<triple>[.exe]). Implies --version is required.
#   --upload    attach the built .mcpb to the release named by --tag (gh).
#
# Requires: gh, tar, unzip, lipo (macOS host — lipo is macOS-only), npx.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MCPB_DIR="$ROOT/mcpb"

TAG=""; FROM_DIR=""; VERSION=""; OUT="$ROOT/dist"; UPLOAD=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --tag) TAG="$2"; shift 2 ;;
    --from-dir) FROM_DIR="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --upload) UPLOAD=1; shift ;;
    *) echo "pack-mcpb: unknown arg: $1" >&2; exit 2 ;;
  esac
done

command -v lipo >/dev/null || { echo "pack-mcpb: 'lipo' not found — run on a macOS host (lipo is macOS-only)" >&2; exit 1; }

# The five cargo-dist targets (see dist-workspace.toml). Keep in sync.
MAC_ARM="aarch64-apple-darwin"
MAC_X64="x86_64-apple-darwin"
LNX_ARM="aarch64-unknown-linux-gnu"
LNX_X64="x86_64-unknown-linux-gnu"
WIN_X64="x86_64-pc-windows-msvc"

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
BINS="$WORK/bins"; mkdir -p "$BINS"

if [[ -n "$FROM_DIR" ]]; then
  [[ -n "$VERSION" ]] || { echo "pack-mcpb: --from-dir requires --version" >&2; exit 2; }
  for t in "$MAC_ARM" "$MAC_X64" "$LNX_ARM" "$LNX_X64"; do cp "$FROM_DIR/sema-$t" "$BINS/sema-$t"; done
  cp "$FROM_DIR/sema-$WIN_X64.exe" "$BINS/sema-$WIN_X64.exe"
else
  command -v gh >/dev/null || { echo "pack-mcpb: 'gh' required to download release assets" >&2; exit 1; }
  [[ -n "$TAG" ]] || TAG="$(git -C "$ROOT" describe --tags --abbrev=0)"
  VERSION="${TAG#v}"
  DL="$WORK/dl"; mkdir -p "$DL"
  echo "== downloading $TAG release assets =="
  # cargo-dist names assets by the crate (sema-lang), not the binary (sema).
  gh release download "$TAG" --repo sema-lisp/sema --dir "$DL" \
    --pattern 'sema-lang-*.tar.xz' --pattern 'sema-lang-*.zip'
  # Extract each archive and lift out its `sema` binary, named by triple.
  for t in "$MAC_ARM" "$MAC_X64" "$LNX_ARM" "$LNX_X64"; do
    tar -xf "$DL/sema-lang-$t.tar.xz" -C "$WORK"
    cp "$(find "$WORK" -type f -name sema -path "*$t*" | head -1)" "$BINS/sema-$t"
  done
  unzip -oq "$DL/sema-lang-$WIN_X64.zip" -d "$WORK/win"
  cp "$(find "$WORK/win" -type f -name 'sema.exe' | head -1)" "$BINS/sema-$WIN_X64.exe"
fi

echo "== assembling bundle (version $VERSION) =="
BUNDLE="$WORK/bundle"; BIN="$BUNDLE/server/bin"; mkdir -p "$BIN"

# macOS: fuse the two arch binaries into one universal2 binary.
lipo -create "$BINS/sema-$MAC_ARM" "$BINS/sema-$MAC_X64" -output "$BIN/sema-macos-universal"
# Linux: both arches + the committed arch-dispatch shim.
cp "$BINS/sema-$LNX_X64" "$BIN/sema-linux-x64"
cp "$BINS/sema-$LNX_ARM" "$BIN/sema-linux-arm64"
cp "$MCPB_DIR/sema-linux" "$BIN/sema-linux"
# Windows: x86_64.
cp "$BINS/sema-$WIN_X64.exe" "$BIN/sema-windows-x64.exe"
chmod +x "$BIN"/sema-*

# Manifest with the version substituted in.
sed "s/__VERSION__/$VERSION/" "$MCPB_DIR/manifest.json" > "$BUNDLE/manifest.json"

echo "== validate + pack =="
mkdir -p "$OUT"
npx --yes @anthropic-ai/mcpb@latest validate "$BUNDLE/manifest.json"
npx --yes @anthropic-ai/mcpb@latest pack "$BUNDLE" "$OUT/sema.mcpb"
echo "built: $OUT/sema.mcpb"

if [[ "$UPLOAD" == "1" ]]; then
  [[ -n "$TAG" ]] || { echo "pack-mcpb: --upload needs --tag" >&2; exit 2; }
  echo "== uploading sema.mcpb to $TAG =="
  gh release upload "$TAG" "$OUT/sema.mcpb" --repo sema-lisp/sema --clobber
fi
