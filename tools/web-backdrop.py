#!/usr/bin/env python3
"""Draws the quiet ambient console backdrop aligned with the slate & indigo-teal tokens.

Writes web/src/assets/backdrop-light.svg and backdrop-dark.svg. The drawing uses only
soft radial/linear gradients and a minimal set of geodesic route arcs in the periphery
(no filters, no vertical streaks, no star speckles), so browsers rasterise it once and
tables or charts never compete with decorative noise.

    python3 tools/web-backdrop.py
"""
from pathlib import Path

WIDTH, HEIGHT = 1600, 1000
OUT = Path(__file__).resolve().parent.parent / "web" / "src" / "assets"

THEMES = {
    "light": {
        "base": ("#f7f9fc", "#f1f5f9"),
        # (cx, cy, rx, ry, colour, peak opacity): quiet ambient colour washes.
        "fields": [
            (1340, 20, 780, 420, "#3b66d4", 0.095),
            (160, 80, 620, 380, "#1ca0c9", 0.075),
            (260, 980, 700, 340, "#189b82", 0.045),
        ],
        "ribbons": [
            ("#3b66d4", 0.045, "M-60,170 C420,60 980,250 1660,105"),
            ("#1ca0c9", 0.035, "M-60,260 C520,165 1060,310 1660,195"),
        ],
        "arc": "#3558b8",
        "arc_opacity": 0.055,
        "node": "#3558b8",
        "node_opacity": 0.11,
        "halo_opacity": 0.035,
    },
    "dark": {
        "base": ("#0d1220", "#080c15"),
        "fields": [
            (1340, 20, 780, 420, "#3862cf", 0.17),
            (160, 80, 620, 380, "#178db5", 0.115),
            (260, 980, 700, 340, "#12836d", 0.07),
        ],
        "ribbons": [
            ("#4f7cf0", 0.06, "M-60,170 C420,60 980,250 1660,105"),
            ("#22a8d8", 0.045, "M-60,260 C520,165 1060,310 1660,195"),
        ],
        "arc": "#93b4ff",
        "arc_opacity": 0.055,
        "node": "#b8ceff",
        "node_opacity": 0.14,
        "halo_opacity": 0.045,
    },
}

# Minimal entry -> relay -> exit geodesic curves in the upper-right periphery.
ARCS = [
    "M980,38 Q1230,118 1485,84",
    "M1115,158 Q1310,96 1485,84",
    "M980,38 Q1055,108 1115,158",
    "M1115,158 Q1290,210 1530,224",
]
WAYPOINTS = [
    (980, 38, 2.2, False),
    (1235, 92, 2.6, True),
    (1485, 84, 2.4, False),
    (1115, 158, 2.8, True),
    (1320, 192, 2.0, False),
    (1530, 224, 2.2, False),
]


def draw(theme: str) -> str:
    spec = THEMES[theme]
    defs = [
        f'<linearGradient id="base" x1="0" y1="0" x2="0" y2="1">'
        f'<stop offset="0" stop-color="{spec["base"][0]}"/>'
        f'<stop offset="1" stop-color="{spec["base"][1]}"/></linearGradient>'
    ]
    body = ['<rect width="100%" height="100%" fill="url(#base)"/>']

    for index, (cx, cy, rx, ry, colour, peak) in enumerate(spec["fields"]):
        defs.append(
            f'<radialGradient id="f{index}">'
            f'<stop offset="0" stop-color="{colour}" stop-opacity="{peak:.3f}"/>'
            f'<stop offset="0.55" stop-color="{colour}" stop-opacity="{peak * 0.42:.3f}"/>'
            f'<stop offset="1" stop-color="{colour}" stop-opacity="0"/></radialGradient>'
        )
        body.append(f'<ellipse cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}" fill="url(#f{index})"/>')

    for index, (colour, opacity, path) in enumerate(spec["ribbons"]):
        defs.append(
            f'<linearGradient id="r{index}" x1="0" y1="0" x2="1" y2="0">'
            f'<stop offset="0" stop-color="{colour}" stop-opacity="0"/>'
            f'<stop offset="0.48" stop-color="{colour}" stop-opacity="{opacity * 1.8:.3f}"/>'
            f'<stop offset="1" stop-color="{colour}" stop-opacity="0"/></linearGradient>'
        )
        layers, width = 5, 180 - index * 40
        strokes = "".join(
            f'<path d="{path}" stroke-width="{width * (1 - layer / layers):.0f}"/>'
            for layer in range(layers)
        )
        body.append(
            f'<g fill="none" stroke="url(#r{index})" stroke-linecap="round" opacity="{1.5 / layers:.3f}">'
            f"{strokes}</g>"
        )

    arc_paths = "".join(f'<path d="{arc}"/>' for arc in ARCS)
    body.append(
        f'<g fill="none" stroke="{spec["arc"]}" stroke-opacity="{spec["arc_opacity"]}" '
        f'stroke-width="1" stroke-dasharray="3 5">{arc_paths}</g>'
    )
    halos = "".join(
        f'<circle cx="{x}" cy="{y}" r="8" fill="{spec["node"]}" fill-opacity="{spec["halo_opacity"]}"/>'
        for x, y, _, hub in WAYPOINTS
        if hub
    )
    dots = "".join(
        f'<circle cx="{x}" cy="{y}" r="{r}" fill="{spec["node"]}" fill-opacity="{spec["node_opacity"]}"/>'
        for x, y, r, _ in WAYPOINTS
    )
    body.append(halos + dots)

    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {WIDTH} {HEIGHT}" '
        f'preserveAspectRatio="xMidYMid slice"><defs>{"".join(defs)}</defs>{"".join(body)}</svg>\n'
    )


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    for theme in THEMES:
        target = OUT / f"backdrop-{theme}.svg"
        target.write_text(draw(theme))
        print(target, target.stat().st_size, "bytes")
