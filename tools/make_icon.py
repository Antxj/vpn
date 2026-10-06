"""VPN icon design (shield with a padlock). Requires: pip install pillow

Writes the .ico files to the current folder. The app only uses the blue icon,
copied to rust/assets/icon.ico (the tray icons are the PNGs in rust/assets).

- icon.ico       blue    (app/window/exe icon)
- icon_gray.ico  gray    (tray: disconnected)
- icon_warn.ico  amber   (tray: connecting/reconnecting)
- icon_ok.ico    green   (tray: connected)
"""
from PIL import Image, ImageDraw

WHITE = (255, 255, 255, 255)
VARIANTS = {
    "icon.ico":      ((37, 99, 235, 255), (29, 78, 216, 255)),
    "icon_gray.ico": ((107, 114, 128, 255), (75, 85, 99, 255)),
    "icon_warn.ico": ((245, 158, 11, 255), (180, 83, 9, 255)),
    "icon_ok.ico":   ((5, 150, 105, 255), (4, 120, 87, 255)),
}


def draw_icon(size: int, accent, accent_dark) -> Image.Image:
    scale = 4  # draw large and scale down (anti-aliasing)
    s = size * scale
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    def p(x: float, y: float) -> tuple[float, float]:
        return x * s, y * s

    # rounded background with a slight vertical gradient
    for i in range(s):
        t = i / s
        r = int(accent[0] + (accent_dark[0] - accent[0]) * t)
        g = int(accent[1] + (accent_dark[1] - accent[1]) * t)
        b = int(accent[2] + (accent_dark[2] - accent[2]) * t)
        d.line([(0, i), (s, i)], fill=(r, g, b, 255))
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, s - 1, s - 1],
                                           radius=int(s * 0.22), fill=255)
    img.putalpha(mask)
    d = ImageDraw.Draw(img)

    # white shield
    shield = [p(0.50, 0.13), p(0.79, 0.245), p(0.79, 0.52),
              p(0.71, 0.70), p(0.50, 0.87), p(0.29, 0.70), p(0.21, 0.52),
              p(0.21, 0.245)]
    d.polygon(shield, fill=WHITE)

    # padlock (in the background color) inside the shield
    d.arc([p(0.40, 0.28)[0], p(0.40, 0.28)[1], p(0.60, 0.48)[0], p(0.60, 0.48)[1]],
          start=180, end=360, fill=accent_dark, width=int(s * 0.045))
    d.rounded_rectangle([p(0.355, 0.40)[0], p(0.355, 0.40)[1],
                         p(0.645, 0.66)[0], p(0.645, 0.66)[1]],
                        radius=int(s * 0.035), fill=accent_dark)
    d.ellipse([p(0.465, 0.465)[0], p(0.465, 0.465)[1],
               p(0.535, 0.535)[0], p(0.535, 0.535)[1]], fill=WHITE)
    d.rectangle([p(0.485, 0.50)[0], p(0.485, 0.50)[1],
                 p(0.515, 0.60)[0], p(0.515, 0.60)[1]], fill=WHITE)

    return img.resize((size, size), Image.LANCZOS)


if __name__ == "__main__":
    sizes = [16, 24, 32, 48, 64, 128, 256]
    for name, (accent, accent_dark) in VARIANTS.items():
        images = [draw_icon(s, accent, accent_dark) for s in sizes]
        images[-1].save(name, format="ICO",
                        sizes=[(s, s) for s in sizes],
                        append_images=images[:-1])
        print(f"{name} generated.")
