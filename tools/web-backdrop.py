#!/usr/bin/env python3
"""Draws the console backdrop: an aurora over a faint proxy-network constellation.

Writes web/src/assets/backdrop-light.svg and backdrop-dark.svg. The drawing uses only
gradients and thin strokes (no filters), so a browser rasterises it once and
cheaply. The constellation is seeded, so reruns give the same image.

    python3 tools/web-backdrop.py
"""
import math
import random
from pathlib import Path

WIDTH, HEIGHT = 1600, 1000
OUT = Path(__file__).resolve().parent.parent / "web" / "src" / "assets"

THEMES = {
    "light": {
        "base": ("#f8f7ff", "#f3f5fb"),
        # (cx, cy, rx, ry, colour, peak opacity): soft colour fields of the aurora.
        "fields": [
            (1330, 40, 760, 430, "#8b7bff", 0.26),
            (130, 90, 560, 380, "#5cc6f5", 0.2),
            (1500, 760, 560, 420, "#ff9ccf", 0.14),
            (240, 960, 640, 360, "#6ee7c6", 0.12),
            (820, -40, 520, 220, "#b9a8ff", 0.16),
        ],
        "ribbons": [("#8b7bff", 0.1, 230), ("#5cc6f5", 0.08, 170), ("#ff9ccf", 0.06, 130)],
        "line": "#4f46e5", "line_opacity": 0.085, "node": "#4f46e5", "node_opacity": 0.22, "halo_opacity": 0.07,
        # Curtain rays inside the aurora, a glow along the bottom, and (dark only) stars.
        "rays": ["#8b7bff", "#5cc6f5", "#b9a8ff"], "ray_opacity": 0.22, "horizon": ("#a78bfa", 0.1), "stars": 0,
    },
    "dark": {
        "base": ("#141530", "#0b0d1c"),
        "fields": [
            (1330, 40, 760, 430, "#6d5dfc", 0.34),
            (130, 90, 560, 380, "#1fa3d9", 0.22),
            (1500, 760, 560, 420, "#d04b9a", 0.16),
            (240, 960, 640, 360, "#12a98a", 0.13),
            (820, -40, 520, 220, "#8b7bff", 0.2),
        ],
        "ribbons": [("#8b7bff", 0.12, 230), ("#38bdf8", 0.09, 170), ("#f472b6", 0.06, 130)],
        "line": "#c7c3ff", "line_opacity": 0.075, "node": "#d9d6ff", "node_opacity": 0.26, "halo_opacity": 0.09,
        "rays": ["#8b7bff", "#38bdf8", "#a78bfa"], "ray_opacity": 0.3, "horizon": ("#6d5dfc", 0.16), "stars": 150,
    },
}


def constellation(seed=7):
    """Points strewn over the canvas, denser towards the top right, joined to near neighbours."""
    rng = random.Random(seed)
    points = []
    columns, rows = 13, 8
    for row in range(rows):
        for column in range(columns):
            x = (column + 0.5) * WIDTH / columns + rng.uniform(-48, 48)
            y = (row + 0.5) * HEIGHT / rows + rng.uniform(-40, 40)
            # Thin out the lower left so the drawing leans towards the light.
            keep = 0.35 + 0.65 * (x / WIDTH) * (1 - 0.55 * y / HEIGHT)
            if rng.random() < keep:
                points.append((round(x, 1), round(y, 1), rng.random()))
    edges = set()
    for index, (x, y, _) in enumerate(points):
        nearest = sorted(range(len(points)), key=lambda other: (points[other][0] - x) ** 2 + (points[other][1] - y) ** 2)[1:3]
        for other in nearest:
            if math.dist((x, y), points[other][:2]) < 230:
                edges.add(tuple(sorted((index, other))))
    return points, sorted(edges)


def ribbon_path(phase, amplitude, baseline):
    """A long, slow wave across the top of the canvas."""
    steps = 8
    coordinates = [(WIDTH * step / steps - 80, baseline + amplitude * math.sin(step * 0.9 + phase)) for step in range(steps + 2)]
    path = f"M{coordinates[0][0]:.0f},{coordinates[0][1]:.0f}"
    for (x0, y0), (x1, y1) in zip(coordinates, coordinates[1:]):
        path += f" Q{x0 + (x1 - x0) / 2:.0f},{y0:.0f} {x1:.0f},{y1:.0f}"
    return path


def rays(spec, seed=11):
    """Soft vertical streaks along the aurora bands, the texture of a real aurora's curtain.

    Tall, thin ellipses filled with a radial gradient fade out in every direction,
    so the streaks blend into the band instead of reading as stripes."""
    rng = random.Random(seed)
    shapes = []
    for _ in range(64):
        x = rng.uniform(40, WIDTH - 40)
        band = 150 + 70 * math.sin(x / WIDTH * 4.2 + 0.6)
        ry = rng.uniform(70, 170)
        rx = rng.uniform(9, 30)
        colour = rng.randrange(len(spec["rays"]))
        # Strongest towards the middle of the canvas, fading at both sides.
        opacity = spec["ray_opacity"] * rng.uniform(0.3, 1) * (0.3 + 0.7 * math.sin(math.pi * x / WIDTH))
        shapes.append(f'<ellipse cx="{x:.0f}" cy="{band - ry * 0.25:.0f}" rx="{rx:.0f}" ry="{ry:.0f}" fill="url(#ray{colour})" opacity="{opacity:.3f}"/>')
    return shapes


def stars(count, seed=23):
    """A sparse starfield above the horizon, brighter near the top."""
    rng = random.Random(seed)
    dots = []
    for _ in range(count):
        x, y = rng.uniform(0, WIDTH), rng.uniform(0, HEIGHT * 0.72) ** 1.08 / (HEIGHT * 0.72) ** 0.08
        radius = rng.choice([0.5, 0.6, 0.7, 0.8, 1.0, 1.3])
        dots.append(f'<circle cx="{x:.0f}" cy="{y:.0f}" r="{radius}" fill-opacity="{rng.uniform(0.18, 0.7) * (1 - 0.6 * y / HEIGHT):.2f}"/>')
    return dots


def draw(theme):
    spec = THEMES[theme]
    points, edges = constellation()
    defs = [f'<linearGradient id="base" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{spec["base"][0]}"/><stop offset="1" stop-color="{spec["base"][1]}"/></linearGradient>']
    body = ['<rect width="100%" height="100%" fill="url(#base)"/>']
    for index, (cx, cy, rx, ry, colour, peak) in enumerate(spec["fields"]):
        defs.append(f'<radialGradient id="f{index}"><stop offset="0" stop-color="{colour}" stop-opacity="{peak}"/>'
                    f'<stop offset="0.55" stop-color="{colour}" stop-opacity="{peak * 0.42:.3f}"/><stop offset="1" stop-color="{colour}" stop-opacity="0"/></radialGradient>')
        body.append(f'<ellipse cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}" fill="url(#f{index})"/>')
    for index, (colour, opacity, width) in enumerate(spec["ribbons"]):
        defs.append(f'<linearGradient id="r{index}" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="{colour}" stop-opacity="0"/>'
                    f'<stop offset="0.45" stop-color="{colour}" stop-opacity="{opacity * 2.2:.3f}"/><stop offset="1" stop-color="{colour}" stop-opacity="0"/></linearGradient>')
        # Stacked strokes of shrinking width approximate a soft falloff without a blur filter.
        path, layers = ribbon_path(index * 1.7, 70 - index * 14, 150 + index * 70), 8
        body.append(f'<g fill="none" stroke="url(#r{index})" stroke-linecap="round" stroke-linejoin="round" opacity="{1.8 / layers:.3f}">'
                    + "".join(f'<path d="{path}" stroke-width="{width * (1 - layer / layers):.0f}"/>' for layer in range(layers)) + "</g>")
    for index, colour in enumerate(spec["rays"]):
        defs.append(f'<radialGradient id="ray{index}"><stop offset="0" stop-color="{colour}" stop-opacity="1"/>'
                    f'<stop offset="0.5" stop-color="{colour}" stop-opacity="0.45"/><stop offset="1" stop-color="{colour}" stop-opacity="0"/></radialGradient>')
    body.append("<g>" + "".join(rays(spec)) + "</g>")
    colour, opacity = spec["horizon"]
    defs.append(f'<radialGradient id="horizon"><stop offset="0" stop-color="{colour}" stop-opacity="{opacity}"/><stop offset="1" stop-color="{colour}" stop-opacity="0"/></radialGradient>')
    body.append(f'<ellipse cx="{WIDTH / 2:.0f}" cy="{HEIGHT + 60}" rx="{WIDTH * 0.75:.0f}" ry="260" fill="url(#horizon)"/>')
    if spec["stars"]:
        body.append('<g fill="#ffffff">' + "".join(stars(spec["stars"])) + "</g>")
    lines = " ".join(f"M{points[a][0]},{points[a][1]}L{points[b][0]},{points[b][1]}" for a, b in edges)
    body.append(f'<path d="{lines}" fill="none" stroke="{spec["line"]}" stroke-opacity="{spec["line_opacity"]}" stroke-width="0.9"/>')
    hubs = [point for point in points if point[2] > 0.86]
    body.append("".join(f'<circle cx="{x}" cy="{y}" r="9" fill="{spec["node"]}" fill-opacity="{spec["halo_opacity"]}"/>' for x, y, _ in hubs))
    body.append(f'<g fill="{spec["node"]}" fill-opacity="{spec["node_opacity"]}">' + "".join(f'<circle cx="{x}" cy="{y}" r="{1.3 + weight * 1.4:.1f}"/>' for x, y, weight in points) + "</g>")
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {WIDTH} {HEIGHT}" preserveAspectRatio="xMidYMid slice">'
            f'<defs>{"".join(defs)}</defs>{"".join(body)}</svg>\n')


if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    for theme in THEMES:
        target = OUT / f"backdrop-{theme}.svg"
        target.write_text(draw(theme))
        print(target, target.stat().st_size, "bytes")
