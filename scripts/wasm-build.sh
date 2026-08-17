#!/usr/bin/env bash

# Single entry point for every wasm-pack build in this repo.
#
# Exists to make the compiled WASM byte-identical across machines. rustc bakes
# absolute source paths into the binary (panic/debug strings from dependencies,
# e.g. `/Users/you/.cargo/registry/src/.../regex-1.12.3/src/lib.rs`), so a build
# on a laptop and the same build on a CI runner differ by thousands of bytes.
# That made the committed browser runtime under `crates/sema/src/web/assets/`
# impossible to verify: verify.yml rebuilds it and requires `git diff --quiet`,
# which no contributor could ever satisfy from their own machine. It also meant
# every shipped artifact leaked the maintainer's home directory.
#
# `--remap-path-prefix` rewrites those three roots (CARGO_HOME, the rustc
# sysroot, this checkout) to fixed names, so the output depends on the source,
# not on where it was built.
#
# Usage: scripts/wasm-build.sh <wasm-pack args...>

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO_ROOT="${CARGO_HOME:-$HOME/.cargo}"
SYSROOT="$(rustc --print sysroot)"

# wasm-pack lazily `cargo install`s an uncached wasm-bindgen-cli as a HOST
# build. It inherits CARGO_ENCODED_RUSTFLAGS (exported below), whose wasm link
# flags (`-C link-args=--stack-first -z stack-size=…`) break that host build
# ("failed to run rustc to learn about target-specific information"). Pre-install
# the CLI with a scrubbed env into the exact dir wasm-pack probes for, so
# wasm-pack skips its own install and never sees the flags. Mirrors wasm-pack's
# layout: the binaries live at the install root, not under bin/.
install_wasm_bindgen_cli() {
  local version
  version="$(grep -A2 '^name = "wasm-bindgen"' "$ROOT/Cargo.lock" |
    grep '^version' | head -1 | cut -d'"' -f2)"
  if [ -z "$version" ]; then
    echo "wasm-build: could not resolve wasm-bindgen version from Cargo.lock" >&2
    exit 1
  fi
  # Mirrors wasm-pack's cache location (dirs::cache_dir + "/.wasm-pack").
  local cache
  if [ -n "${XDG_CACHE_HOME:-}" ]; then
    cache="$XDG_CACHE_HOME/.wasm-pack"
  elif [ "$(uname -s)" = "Darwin" ]; then
    cache="$HOME/Library/Caches/.wasm-pack"
  else
    cache="$HOME/.cache/.wasm-pack"
  fi
  local dir="$cache/wasm-bindgen-cargo-install-$version"
  if [ -x "$dir/wasm-bindgen" ]; then
    return 0 # cached from a previous build; wasm-pack reuses it
  fi
  (
    unset CARGO_ENCODED_RUSTFLAGS RUSTFLAGS CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS
    cargo install --force wasm-bindgen-cli --root "$dir" --version "$version"
    mv "$dir/bin/wasm-bindgen" "$dir/bin/wasm-bindgen-test-runner" "$dir/"
  )
}

install_wasm_bindgen_cli

# CARGO_ENCODED_RUSTFLAGS, not RUSTFLAGS: the flags are \x1f-separated, so a
# single flag may contain spaces. The wasm link flags below are exactly that
# (`link-args=--stack-first -z stack-size=16777216`) — passing them through the
# space-separated RUSTFLAGS splits `-z` into its own argument and rustc dies
# with "Unrecognized option: 'z'".
#
# Either variable REPLACES `target.<triple>.rustflags` from .cargo/config.toml —
# they do not merge — so setting one naively would silently drop the wasm stack
# configuration and produce a binary that overflows on deep recursion. Read that
# array out of the config and prepend it rather than restating it here, so the
# config stays the single source of truth for those flags.
CARGO_ENCODED_RUSTFLAGS="$(
  python3 - "$ROOT/.cargo/config.toml" "$CARGO_ROOT" "$SYSROOT" "$ROOT" <<'PY'
import pathlib
import sys
import tomllib

config_path, cargo_root, sysroot, checkout = sys.argv[1:5]
config = tomllib.loads(pathlib.Path(config_path).read_text())
base = config.get("target", {}).get("wasm32-unknown-unknown", {}).get("rustflags")
if not base:
    raise SystemExit(
        f"wasm-build: no [target.wasm32-unknown-unknown] rustflags in {config_path}. "
        "If they moved, update this script — dropping them silently would ship a "
        "WASM binary with the wrong stack size."
    )
# Remap targets are arbitrary but must be identical on every machine.
remaps = [
    f"--remap-path-prefix={cargo_root}=/cargo",
    f"--remap-path-prefix={sysroot}=/rustc",
    f"--remap-path-prefix={checkout}=/sema",
]
sys.stdout.write("\x1f".join([*base, *remaps]))
PY
)"
export CARGO_ENCODED_RUSTFLAGS

exec wasm-pack build "$@"
