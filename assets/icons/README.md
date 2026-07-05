# Sema icon assets — canonical source

The single source of truth for every Sema brand and editor icon. Everything else
in the repo (favicons, the VS Code marketplace icon, IntelliJ plugin + file
icons, the website showcase) is **generated or copied from here** — don't edit
those copies by hand.

## Layout

- `svg/` — hand-editable, flattened (vectorized, no `<text>`) source SVGs.
- `png/` — rendered rasters at every needed size (generated, do not edit).

## Icons

| source (`svg/`) | what it is | where it's used |
| --- | --- | --- |
| `sema-mark-rounded.svg` | `(s)` mark on a rounded dark tile | favicons, VS Code / Open VSX marketplace icon, IntelliJ plugin icon |
| `sema-mark-square.svg` | `(s)` mark, full-bleed square (no rounding) | avatars / org logos where the slot applies its own mask |
| `sema-logotype.svg` | `(sema)` wordmark | website `themeConfig.logo` (`website/public/logo.svg`), `playground/logo.svg`, headers, marketing |
| `sema-file-light.svg` / `sema-file-dark.svg` | `.sema` file icon (light-/dark-theme glyph) | VS Code + IntelliJ file-type icons |
| `semac-file.svg` | `.semac` compiled-file icon | IntelliJ file-type icon |
| `sema-notebook-file-light.svg` / `-dark.svg` | `.sema-nb` notebook file icon | VS Code + IntelliJ file-type icons |

## Regenerate

```bash
make icons-assets     # render png/ + sync every consumer copy
```

This runs `scripts/gen-icon-assets.py`: it verifies each SVG is flattened,
renders `png/` at all sizes, and overwrites the consumer copies (website /
playground / pkg favicons, logos, and avatars). Editor plugins live in their own
repos and carry their own icon copies, pulling from the canonical SVGs here.

The website `/icons` showcase inlines these via
`scripts/gen-brand-assets.py` → `website/.vitepress/theme/brandAssets.js`.

To edit an icon: change the file in `svg/` (keep it flattened — use
`scripts/flatten-svg-text.py` if you introduce `<text>`), then run
`make icons-assets`.
