"""GitHub mark used by the "Source code" link on the settings screen.
Requires: pip install pillow

Draws the official mark (Primer Octicons, mark-github-24) white on a
transparent background and writes rust/assets/github_28.png. The app tints it
with the text color of the current theme. 28 px = twice the size it is shown
at, so it stays sharp on 100% and 200% screens.

Run from the repository root: python tools/make_github_mark.py
"""
import math
import re

from PIL import Image, ImageDraw

# https://github.com/primer/octicons/blob/main/icons/mark-github-24.svg
PATH = (
    "M10.226 17.284c-2.965-.36-5.054-2.493-5.054-5.256 0-1.123.404-2.336 1.078-3.144"
    "-.292-.741-.247-2.314.09-2.965.898-.112 2.111.36 2.83 1.01.853-.269 1.752-.404"
    " 2.853-.404 1.1 0 1.999.135 2.807.382.696-.629 1.932-1.1 2.83-.988.315.606.36"
    " 2.179.067 2.942.72.854 1.101 2 1.101 3.167 0 2.763-2.089 4.852-5.098 5.234.763"
    ".494 1.28 1.572 1.28 2.807v2.336c0 .674.561 1.056 1.235.786 4.066-1.55 7.255"
    "-5.615 7.255-10.646C23.5 6.188 18.334 1 11.978 1 5.62 1 .5 6.188.5 12.545c0"
    " 4.986 3.167 9.12 7.435 10.669.606.225 1.19-.18 1.19-.786V20.63a2.9 2.9 0 0 1"
    "-1.078.224c-1.483 0-2.359-.808-2.987-2.313-.247-.607-.517-.966-1.034-1.033-.27"
    "-.023-.359-.135-.359-.27 0-.27.45-.471.898-.471.652 0 1.213.404 1.797 1.235.45"
    ".651.921.943 1.483.943.561 0 .92-.202 1.437-.719.382-.381.674-.718.944-.943"
)
VIEWBOX = 24.0
SIZE = 28
SCALE = 16  # draw large and scale down (anti-aliasing)
OUT = "rust/assets/github_28.png"


def tokens(d):
    for cmd, args in re.findall(r"([A-Za-z])([^A-Za-z]*)", d):
        nums = [float(n) for n in re.findall(r"-?(?:\d+\.?\d*|\.\d+)(?:e-?\d+)?", args)]
        yield cmd, nums


def cubic(p0, p1, p2, p3, steps=24):
    for i in range(1, steps + 1):
        t = i / steps
        u = 1 - t
        yield (
            u**3 * p0[0] + 3 * u * u * t * p1[0] + 3 * u * t * t * p2[0] + t**3 * p3[0],
            u**3 * p0[1] + 3 * u * u * t * p1[1] + 3 * u * t * t * p2[1] + t**3 * p3[1],
        )


def arc(p0, rx, ry, rot, large, sweep, p1, steps=24):
    """SVG elliptical arc (endpoint form) as points, per the SVG spec F.6.5."""
    phi = math.radians(rot)
    cos, sin = math.cos(phi), math.sin(phi)
    dx, dy = (p0[0] - p1[0]) / 2, (p0[1] - p1[1]) / 2
    x1 = cos * dx + sin * dy
    y1 = -sin * dx + cos * dy
    lam = x1 * x1 / (rx * rx) + y1 * y1 / (ry * ry)
    if lam > 1:
        rx, ry = rx * math.sqrt(lam), ry * math.sqrt(lam)
    num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1
    den = rx * rx * y1 * y1 + ry * ry * x1 * x1
    k = math.sqrt(max(0.0, num / den)) * (-1 if large == sweep else 1)
    cx1, cy1 = k * rx * y1 / ry, -k * ry * x1 / rx
    cx = cos * cx1 - sin * cy1 + (p0[0] + p1[0]) / 2
    cy = sin * cx1 + cos * cy1 + (p0[1] + p1[1]) / 2
    a0 = math.atan2((y1 - cy1) / ry, (x1 - cx1) / rx)
    a1 = math.atan2((-y1 - cy1) / ry, (-x1 - cx1) / rx)
    delta = a1 - a0
    if sweep and delta < 0:
        delta += 2 * math.pi
    elif not sweep and delta > 0:
        delta -= 2 * math.pi
    for i in range(1, steps + 1):
        a = a0 + delta * i / steps
        x, y = rx * math.cos(a), ry * math.sin(a)
        yield (cos * x - sin * y + cx, sin * x + cos * y + cy)


def polygon(d):
    """Flattens a single-contour path made of M, C/c, V/v, H/h, L/l, A/a, Z."""
    pts = []
    cur = (0.0, 0.0)
    for cmd, n in tokens(d):
        rel = cmd.islower()
        c = cmd.upper()
        ox, oy = cur if rel else (0.0, 0.0)
        if c == "M":
            cur = (ox + n[0], oy + n[1])
            pts.append(cur)
            n = n[2:]
            c = "L"  # extra pairs after M are lines
        if c == "L":
            for i in range(0, len(n), 2):
                ox, oy = cur if rel else (0.0, 0.0)
                cur = (ox + n[i], oy + n[i + 1])
                pts.append(cur)
        elif c == "H":
            for x in n:
                cur = ((cur[0] if rel else 0.0) + x, cur[1])
                pts.append(cur)
        elif c == "V":
            for y in n:
                cur = (cur[0], (cur[1] if rel else 0.0) + y)
                pts.append(cur)
        elif c == "C":
            for i in range(0, len(n), 6):
                ox, oy = cur if rel else (0.0, 0.0)
                p1 = (ox + n[i], oy + n[i + 1])
                p2 = (ox + n[i + 2], oy + n[i + 3])
                p3 = (ox + n[i + 4], oy + n[i + 5])
                pts.extend(cubic(cur, p1, p2, p3))
                cur = p3
        elif c == "A":
            for i in range(0, len(n), 7):
                ox, oy = cur if rel else (0.0, 0.0)
                end = (ox + n[i + 5], oy + n[i + 6])
                pts.extend(arc(cur, n[i], n[i + 1], n[i + 2], bool(n[i + 3]), bool(n[i + 4]), end))
                cur = end
        elif c == "Z":
            pass
        else:
            raise ValueError(f"unsupported path command {cmd}")
    return pts


def main():
    s = SIZE * SCALE
    k = s / VIEWBOX
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).polygon([(x * k, y * k) for x, y in polygon(PATH)], fill=255)
    alpha = mask.resize((SIZE, SIZE), Image.Resampling.BOX)
    img = Image.new("RGBA", (SIZE, SIZE), (255, 255, 255, 0))
    img.putalpha(alpha)
    img.save(OUT, optimize=True)
    print("wrote", OUT)


if __name__ == "__main__":
    main()
