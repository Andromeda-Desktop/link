# Writes the one-colour Andromeda mark (the A and its galaxy's outer ring, with gaps where they cross) as SVG path data.
# Run: uv run --with shapely python dist/branding/silhouette.py
import math
from pathlib import Path

from shapely import affinity
from shapely.geometry import LineString, Point, Polygon
from shapely.ops import unary_union

HERE = Path(__file__).parent
A = Polygon([(116, 414), (232, 94), (280, 94), (396, 414), (334, 414), (256, 186), (178, 414)])
CENTER, RX, RY, TILT = (256, 296), 200, 62, -34
RING, GAP = 30, 14  # widths in the 512 viewBox, heavier than the colour mark so 16 px stays legible


def arc(start, end):
    points = [(RX * math.cos(t), RY * math.sin(t)) for t in (start + (end - start) * i / 200 for i in range(201))]
    line = affinity.rotate(LineString(points), TILT, origin=(0, 0))
    return affinity.translate(line, *CENTER)


back = arc(math.pi - 0.05, 2 * math.pi + 0.05).buffer(RING / 2, cap_style="flat")
front = arc(0, math.pi).buffer(RING / 2, cap_style="flat")
shape = unary_union([A.difference(front.buffer(GAP)), back.difference(A.buffer(GAP)), front])


def path(geometry):
    rings = []
    for polygon in getattr(geometry, "geoms", [geometry]):
        for ring in [polygon.exterior, *polygon.interiors]:
            coords = list(ring.coords)
            rings.append("M" + " L".join(f"{x:.1f},{y:.1f}" for x, y in coords[:-1]) + "Z")
    return "".join(rings)


data = path(shape.simplify(0.4))
(HERE / "andromeda-mark-path.txt").write_text(data + "\n")
(HERE / "andromeda-symbolic.svg").write_text(
    f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512"><path d="{data}" fill="#2E3436"/></svg>\n'
)
