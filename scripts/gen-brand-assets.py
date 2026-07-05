#!/usr/bin/env python3
"""Generate website/.vitepress/theme/brandAssets.js from the canonical icon SVGs.

The /icons showcase page must inline every icon as a string (the Vercel build
only uploads website/, so it can't import from assets/ at the repo root). This copies
the canonical flattened SVGs from assets/icons/svg/ into a JS module so the
showcase always matches what actually ships. Run after changing any source icon.
"""
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
SVG = ROOT / "assets/icons/svg"

# brandAssets.js export name -> canonical svg file
ASSETS = {
    "sMarkTile": "sema-mark-rounded.svg",
    "sMarkSquare": "sema-mark-square.svg",
    "fileSemaLight": "sema-file-light.svg",
    "fileSemaDark": "sema-file-dark.svg",
    "fileSemac": "semac-file.svg",
    "fileNotebookLight": "sema-notebook-file-light.svg",
    "fileNotebookDark": "sema-notebook-file-dark.svg",
    "logotype": "sema-logotype.svg",
}


def main() -> None:
    lines = [
        "// AUTO-GENERATED brand/editor icon assets for the /icons showcase.",
        "// Regenerate with scripts/gen-brand-assets.py after changing assets/icons/svg/.",
        "",
    ]
    for export, fname in ASSETS.items():
        svg = (SVG / fname).read_text().strip()
        if "`" in svg:
            raise SystemExit(f"backtick in {fname} would break the template literal")
        lines.append(f"export const {export} = `{svg}`;\n")
    (ROOT / "website/.vitepress/theme/brandAssets.js").write_text("\n".join(lines))
    print("wrote website/.vitepress/theme/brandAssets.js:", ", ".join(ASSETS))


if __name__ == "__main__":
    main()
