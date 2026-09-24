"""Draws the GitHub social preview card (1280x640) to assets/social-preview.png.

Reuses the pixel-art icon from make-icon.py, next to the name, a one-line pitch
and a small latency graph in the overlay's green / yellow / red.
Upload it under Settings > General > Social preview.

    python scripts/make-social-preview.py
"""
import importlib.util
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets"

spec = importlib.util.spec_from_file_location("make_icon", Path(__file__).with_name("make-icon.py"))
icon = importlib.util.module_from_spec(spec)
spec.loader.exec_module(icon)

W, H = 1280, 640
BG_TOP, BG_BOTTOM = (12, 32, 21), (4, 14, 9)
TITLE = (220, 252, 231)
SUB = (134, 239, 172)
MUTED = (74, 140, 100)
GOOD, WARN, BAD = (34, 197, 94), (234, 179, 8), (239, 68, 68)

FONT_DIR = Path("C:/Windows/Fonts") if Path("C:/Windows/Fonts").exists() else Path("/usr/share/fonts/truetype/dejavu")


def font(names, size):
    for name in names:
        p = FONT_DIR / name
        if p.exists():
            return ImageFont.truetype(str(p), size)
    return ImageFont.load_default(size)


BOLD = ["segoeuib.ttf", "DejaVuSans-Bold.ttf"]
REGULAR = ["segoeui.ttf", "DejaVuSans.ttf"]


def main():
    img = Image.new("RGBA", (W, H))
    d = ImageDraw.Draw(img)
    for y in range(H):
        t = y / (H - 1)
        d.line([(0, y), (W, y)], fill=tuple(round(a + (b - a) * t) for a, b in zip(BG_TOP, BG_BOTTOM)))

    # icon: the 64x64 grid at 5x, left side, vertically centred
    ic = icon.up(icon.big(), 5)
    ix, iy = 110, (H - ic.height) // 2
    img.paste(ic, (ix, iy), ic)

    x = ix + ic.width + 80
    d.text((x, 150), "PingLive", font=font(BOLD, 104), fill=TITLE)
    sub = font(REGULAR, 34)
    d.text((x, 285), "Always-on-top ping monitor", font=sub, fill=SUB)
    d.text((x, 330), "for Windows", font=sub, fill=SUB)
    d.text((x, 395), "Rust  \u00b7  single exe  \u00b7  MIT", font=font(REGULAR, 26), fill=MUTED)

    # mini latency graph under the text, like the overlay's bars
    pings = [22, 25, 21, 30, 28, 45, 90, 140, 60, 26, 24, None, None, 35, 27, 23, 26, 24, 110, 32, 25, 22, 24, 28]
    gx, gy, gh, bw, gap = x, 455, 70, 14, 5
    for i, p in enumerate(pings):
        if p is None:
            h, c = gh, BAD
        else:
            h, c = max(6, round(gh * min(p, 160) / 160)), GOOD if p <= 60 else WARN if p <= 120 else BAD
        bx = gx + i * (bw + gap)
        d.rectangle([bx, gy + gh - h, bx + bw - 1, gy + gh], fill=c)

    ASSETS.mkdir(exist_ok=True)
    img.convert("RGB").save(ASSETS / "social-preview.png", optimize=True)
    print("wrote", ASSETS / "social-preview.png")


if __name__ == "__main__":
    main()
