#!/usr/bin/env python3
"""Canonical icon-asset pipeline.

`assets/icons/svg/` holds the hand-editable, flattened (vectorized, no <text>)
source SVGs — the single source of truth for every Sema brand/editor icon.
This script:

  1. verifies each source is flattened,
  2. renders PNGs at every needed size into `assets/icons/png/`,
  3. syncs SVG + PNG copies out to the places that actually consume them
     (favicons, brand logos, avatars for the website/playground/pkg).

Editor plugins live in their own repos (`sema-lisp/<editor>-sema`) and carry
their own icon copies; they pull from the canonical SVGs in `assets/icons/svg/`
out-of-band, so this script no longer writes into any editor tree.

Edit the SVGs in assets/icons/svg/, then re-run: `make icons-assets`. Consumers
are always overwritten from canonical, so this folder is authoritative.

Requires rsvg-convert (librsvg): `brew install librsvg`.
"""
import pathlib
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SVG = ROOT / "assets/icons/svg"
PNG = ROOT / "assets/icons/png"

# canonical source -> PNG sizes to render. Square icons render N x N; the
# logotype (wide) renders by width, preserving aspect.
RASTER = {
    "sema-mark-rounded": [16, 32, 48, 64, 128, 256, 512],   # app / marketplace tile
    "sema-mark-square": [128, 256, 512, 1024],              # full-bleed avatar
    "sema-file-light": [16, 32, 64],
    "sema-file-dark": [16, 32, 64],
    "semac-file": [16, 32, 64],
    "sema-notebook-file-light": [16, 32, 64],
    "sema-notebook-file-dark": [16, 32, 64],
}
LOGOTYPE_WIDTHS = [366, 732, 1098]  # sema-logotype.svg, by width

# canonical name -> consumer paths that get an exact copy of the SVG.
SVG_CONSUMERS = {
    "sema-mark-rounded": [
        "website/public/favicon.svg",
        "pkg/static/favicon.svg",
        "playground/favicon.svg",
    ],
    "sema-mark-square": ["website/public/avatar.svg"],
    "sema-logotype": [
        "website/public/logo.svg",   # VitePress themeConfig.logo
        "playground/logo.svg",
    ],
}

# rendered PNG (name-SIZE) -> consumer path.
PNG_CONSUMERS = {
    "sema-mark-square-512": "website/public/avatar.png",
}


def rsvg(src, dst, *, w=None, h=None):
    args = ["rsvg-convert"]
    if w:
        args += ["-w", str(w)]
    if h:
        args += ["-h", str(h)]
    args += [str(src), "-o", str(dst)]
    subprocess.run(args, check=True)


def main() -> None:
    if not shutil.which("rsvg-convert"):
        raise SystemExit("rsvg-convert not found — install librsvg (brew install librsvg)")

    sources = sorted(SVG.glob("*.svg"))
    if not sources:
        raise SystemExit(f"no source SVGs in {SVG}")

    # 1. flattened check
    bad = [p.name for p in sources if "<text" in p.read_text()]
    if bad:
        raise SystemExit(f"not flattened (contains <text>): {bad} — run flatten-svg-text.py")

    PNG.mkdir(parents=True, exist_ok=True)

    # 2. render PNGs
    made = 0
    for name, sizes in RASTER.items():
        src = SVG / f"{name}.svg"
        for s in sizes:
            rsvg(src, PNG / f"{name}-{s}.png", w=s, h=s)
            made += 1
    for w in LOGOTYPE_WIDTHS:
        rsvg(SVG / "sema-logotype.svg", PNG / f"sema-logotype-{w}.png", w=w)
        made += 1

    # 3. sync SVG copies
    copied = 0
    for name, dests in SVG_CONSUMERS.items():
        src = SVG / f"{name}.svg"
        for d in dests:
            dp = ROOT / d
            dp.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(src, dp)
            copied += 1
    # 4. sync PNG copies
    for png_name, dest in PNG_CONSUMERS.items():
        shutil.copyfile(PNG / f"{png_name}.png", ROOT / dest)
        copied += 1

    print(f"rendered {made} PNGs into assets/icons/png/; synced {copied} consumer copies")


if __name__ == "__main__":
    sys.exit(main())
