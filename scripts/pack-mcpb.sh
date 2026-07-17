#!/usr/bin/env bash
#
# Build the single cross-platform Sema MCP Bundle (.mcpb).
#
# A .mcpb is a zip of `manifest.json` + the server binaries. We ship ONE bundle
# covering every OS. MCPB picks a binary per OS (darwin/win32/linux) via
# `mcp_config.platform_overrides`, but has NO CPU-arch dimension — so each OS
# must resolve to a single command:
#
#   macOS    two arch binaries are `lipo`'d into ONE universal binary.
#   Linux    both arch binaries ship; the committed shim `mcpb/sema-linux`
#            picks the right one at runtime via `uname -m`.
#   Windows  x86_64 only.
#
# The per-target binaries come from a cargo-dist GitHub release (not a local
# build), so the bundle always matches a published release.
#
# Usage:
#   scripts/pack-mcpb.sh [--tag vX.Y.Z] [--out DIR] [--upload]
#   scripts/pack-mcpb.sh --from-dir DIR --version X.Y.Z [--out DIR]
#
#   --tag       release to download assets from   (default: latest local tag)
#   --from-dir  use a dir already holding the 5 target binaries instead of
#               downloading; named `sema-<triple>` (+ `.exe` for Windows).
#               Requires --version.
#   --out       where to write sema.mcpb          (default: <repo>/dist)
#   --upload    attach the built sema.mcpb to the --tag release (needs --tag)
#
# Requires: npx, plus gh/tar/unzip (download mode) and lipo (macOS-only host).
set -euo pipefail

log() { printf '== %s\n' "$*"; }
die() {
  printf 'pack-mcpb: %s\n' "$1" >&2
  exit "${2:-1}"
}

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MCPB_DIR="$ROOT/mcpb"

# ── Arguments ────────────────────────────────────────────────────────────────
TAG=""
FROM_DIR=""
VERSION=""
OUT="$ROOT/dist"
UPLOAD=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --tag)
      TAG="$2"
      shift 2
      ;;
    --from-dir)
      FROM_DIR="$2"
      shift 2
      ;;
    --version)
      VERSION="$2"
      shift 2
      ;;
    --out)
      OUT="$2"
      shift 2
      ;;
    --upload)
      UPLOAD=1
      shift
      ;;
    *) die "unknown arg: $1" 2 ;;
  esac
done

# lipo fuses the macOS slices; it exists only on macOS. Fail early and loudly
# rather than producing a bundle with a broken darwin binary.
command -v lipo >/dev/null || die "'lipo' not found — run on a macOS host (lipo is macOS-only)"

# The five cargo-dist targets (keep in sync with dist-workspace.toml).
MAC_ARM="aarch64-apple-darwin"
MAC_X64="x86_64-apple-darwin"
LNX_ARM="aarch64-unknown-linux-gnu"
LNX_X64="x86_64-unknown-linux-gnu"
WIN_X64="x86_64-pc-windows-msvc"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
BINS="$WORK/bins" # the 5 raw per-target binaries land here, named sema-<triple>
mkdir -p "$BINS"

# ── Collect the per-target binaries ──────────────────────────────────────────
if [[ -n "$FROM_DIR" ]]; then
  # Local mode: caller already extracted the binaries. Used for testing.
  [[ -n "$VERSION" ]] || die "--from-dir requires --version" 2
  for t in "$MAC_ARM" "$MAC_X64" "$LNX_ARM" "$LNX_X64"; do
    cp "$FROM_DIR/sema-$t" "$BINS/sema-$t"
  done
  cp "$FROM_DIR/sema-$WIN_X64.exe" "$BINS/sema-$WIN_X64.exe"
else
  # Download mode: pull the archives cargo-dist attached to the release.
  command -v gh >/dev/null || die "'gh' required to download release assets"
  [[ -n "$TAG" ]] || TAG="$(git -C "$ROOT" describe --tags --abbrev=0)"
  VERSION="${TAG#v}" # bundle version tracks the release tag, not local Cargo.toml
  DL="$WORK/dl"
  mkdir -p "$DL"

  log "downloading $TAG release assets"
  # cargo-dist names assets by the crate (sema-lang), not the binary (sema).
  gh release download "$TAG" --repo sema-lisp/sema --dir "$DL" \
    --pattern 'sema-lang-*.tar.xz' --pattern 'sema-lang-*.zip'

  # Each archive unpacks to sema-lang-<triple>/sema — extract, then lift the
  # `sema` binary out by matching its triple in the path.
  for t in "$MAC_ARM" "$MAC_X64" "$LNX_ARM" "$LNX_X64"; do
    tar -xf "$DL/sema-lang-$t.tar.xz" -C "$WORK"
    cp "$(find "$WORK" -type f -name sema -path "*$t*" | head -1)" "$BINS/sema-$t"
  done
  unzip -oq "$DL/sema-lang-$WIN_X64.zip" -d "$WORK/win"
  cp "$(find "$WORK/win" -type f -name 'sema.exe' | head -1)" "$BINS/sema-$WIN_X64.exe"
fi

# ── Assemble the bundle tree ─────────────────────────────────────────────────
log "assembling bundle (version $VERSION)"
BUNDLE="$WORK/bundle"
BIN="$BUNDLE/server/bin"
mkdir -p "$BIN"

# macOS: fuse both arch slices into one universal binary (see header).
lipo -create "$BINS/sema-$MAC_ARM" "$BINS/sema-$MAC_X64" -output "$BIN/sema-macos-universal"
# Linux: ship both arches behind the committed uname-dispatch shim.
cp "$BINS/sema-$LNX_X64" "$BIN/sema-linux-x64"
cp "$BINS/sema-$LNX_ARM" "$BIN/sema-linux-arm64"
cp "$MCPB_DIR/sema-linux" "$BIN/sema-linux"
# Windows: x86_64.
cp "$BINS/sema-$WIN_X64.exe" "$BIN/sema-windows-x64.exe"
chmod +x "$BIN"/sema-*

# Manifest is a committed template with a __VERSION__ placeholder.
sed "s/__VERSION__/$VERSION/" "$MCPB_DIR/manifest.json" >"$BUNDLE/manifest.json"

# ── Validate, pack, (optionally) upload ──────────────────────────────────────
log "validate + pack"
mkdir -p "$OUT"
npx --yes @anthropic-ai/mcpb@latest validate "$BUNDLE/manifest.json"
npx --yes @anthropic-ai/mcpb@latest pack "$BUNDLE" "$OUT/sema.mcpb"
log "built: $OUT/sema.mcpb"

if [[ "$UPLOAD" == "1" ]]; then
  [[ -n "$TAG" ]] || die "--upload needs --tag" 2
  log "uploading sema.mcpb to $TAG"
  gh release upload "$TAG" "$OUT/sema.mcpb" --repo sema-lisp/sema --clobber
fi
