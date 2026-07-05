#!/usr/bin/env python3
"""Flatten <text> in brand SVGs to self-contained <path> outlines.

The Sema icons draw their glyphs with <text font-family="JetBrains Mono">
(and a Georgia 'S' variant). That renders correctly only where the font is
installed *and* applied — fine on the website (webfont loaded), but broken as
a distributable icon: editor file-icon slots, favicons, and any SVG rasterizer
that doesn't resolve the webfont fall back to a wrong font or nothing.

This tool converts each <text> run into vector <path> outlines using the real
font files, so the icon carries its own geometry and depends on nothing.

Usage:
  flatten-svg-text.py FILE...        # rewrite in place
  flatten-svg-text.py --check FILE...  # exit non-zero if any <text> remains
"""
import os
import re
import sys

# fontTools is imported lazily inside load_font so the `--check` guard (a plain
# grep for leftover <text>) runs in CI/bare-python without the dependency.

# Standard font install locations searched by basename (macOS + Linux).
FONT_DIRS = [
    os.path.expanduser("~/Library/Fonts"),
    "/System/Library/Fonts/Supplemental",
    "/Library/Fonts",
    os.path.expanduser("~/.local/share/fonts"),
    "/usr/share/fonts",
    "/usr/local/share/fonts",
]

# font-family/weight -> face filename. Weights match how browsers resolve the
# CSS request (Georgia has no 600, so 600 rounds up to its Bold/700 face).
FONT_FILES = {
    ("jetbrains", 700): "JetBrainsMono-Bold.ttf",
    ("jetbrains", 800): "JetBrainsMono-ExtraBold.ttf",
    ("georgia", 700): "Georgia Bold.ttf",
}


def find_font(name):
    for d in FONT_DIRS:
        for root, _, files in os.walk(d) if os.path.isdir(d) else []:
            if name in files:
                return os.path.join(root, name)
    raise SystemExit(
        f"font {name!r} not found in {FONT_DIRS}. Install JetBrains Mono "
        f"(Bold + ExtraBold) and Georgia, or edit FONT_DIRS."
    )

_FONT_CACHE: dict[str, tuple] = {}


def load_font(path):
    from fontTools.ttLib import TTFont

    if path not in _FONT_CACHE:
        f = TTFont(path)
        _FONT_CACHE[path] = (f, f.getGlyphSet(), f.getBestCmap(), f["head"].unitsPerEm)
    return _FONT_CACHE[path]


def resolve_face(family, weight):
    fam = family.lower()
    if "jetbrains" in fam:
        key = ("jetbrains", 800 if weight >= 800 else 700)
    elif "georgia" in fam or "times" in fam or "serif" in fam:
        key = ("georgia", 700)  # only Bold face registered; 500-700 map here
    else:
        raise SystemExit(f"no font registered for family {family!r}")
    return find_font(FONT_FILES[key])


def glyph_path(font_path, char, scale, x, baseline):
    """SVG path 'd' for one glyph, plus its advance in px."""
    from fontTools.pens.svgPathPen import SVGPathPen
    from fontTools.pens.transformPen import TransformPen

    _, glyphset, cmap, upm = load_font(font_path)
    name = cmap.get(ord(char))
    if name is None:
        raise SystemExit(f"glyph {char!r} missing from {font_path}")
    s = scale / upm  # font units -> px, but caller passes px-per-em as scale
    pen = SVGPathPen(glyphset)
    # matrix maps (gx,gy) -> (s*gx + x, -s*gy + baseline): scale + flip Y to SVG
    tpen = TransformPen(pen, (s, 0, 0, -s, x, baseline))
    glyphset[name].draw(tpen)
    advance = glyphset[name].width * s
    return pen.getCommands(), advance


# tokenize a <text> body into runs: (text, fill_override|None, opacity|None)
_TSPAN = re.compile(r'<tspan\b([^>]*)>(.*?)</tspan>|([^<]+)', re.DOTALL)
_ATTR = re.compile(r'(\S+?)="([^"]*)"')


def parse_attrs(s):
    return dict(_ATTR.findall(s))


def flatten_text(el_str, indent):
    open_tag = re.match(r'<text\b([^>]*)>(.*)</text>\s*$', el_str, re.DOTALL)
    attrs = parse_attrs(open_tag.group(1))
    body = open_tag.group(2)

    x = float(attrs["x"])
    baseline = float(attrs["y"])
    anchor = attrs.get("text-anchor", "start")
    size = float(attrs["font-size"])
    weight = int(attrs.get("font-weight", "400"))
    family = attrs["font-family"]
    base_fill = attrs.get("fill", "#000000")
    font_path = resolve_face(family, weight)

    # collect runs of (char, fill, opacity)
    runs = []
    for m in _TSPAN.finditer(body):
        if m.group(3) is not None:  # bare text between tspans
            text, fill, opacity = m.group(3), base_fill, None
        else:
            a = parse_attrs(m.group(1))
            text, fill, opacity = m.group(2), a.get("fill", base_fill), a.get("fill-opacity")
        for ch in text:
            runs.append((ch, fill, opacity))

    # measure total advance for anchor offset
    total = sum(glyph_path(font_path, ch, size, 0, 0)[1] for ch, _, _ in runs)
    off = {"start": 0.0, "middle": -total / 2, "end": -total}[anchor]

    # lay out; group path data by (fill, opacity)
    cursor = x + off
    groups: dict[tuple, list] = {}
    for ch, fill, opacity in runs:
        d, adv = glyph_path(font_path, ch, size, cursor, baseline)
        if d:
            groups.setdefault((fill, opacity), []).append(d)
        cursor += adv

    lines = []
    for (fill, opacity), ds in groups.items():
        d = " ".join(ds)
        # round coords to 3 decimals to keep files small
        d = re.sub(r'-?\d+\.\d+', lambda mm: f"{float(mm.group()):.3f}".rstrip("0").rstrip("."), d)
        op = f' fill-opacity="{opacity}"' if opacity is not None else ""
        lines.append(f'{indent}<path fill="{fill}"{op} d="{d}"/>')
    return "\n".join(lines)


_TEXT_EL = re.compile(r'([ \t]*)<text\b[^>]*>.*?</text>', re.DOTALL)


def process(path, check):
    src = open(path).read()
    if check:
        return "<text" in src
    def repl(m):
        return flatten_text(m.group(0).lstrip(), m.group(1))
    out = _TEXT_EL.sub(repl, src)
    if out != src:
        open(path, "w").write(out)
        print(f"flattened {path}")
    return False


def main(argv):
    check = "--check" in argv
    files = [a for a in argv if not a.startswith("--")]
    remaining = [f for f in files if process(f, check)]
    if check and remaining:
        print("SVGs still contain <text>: " + ", ".join(remaining), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
