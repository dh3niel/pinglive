"""Draws the PingLive pixel-art icon and writes assets/pinglive.ico (+ a PNG preview).

A green "ping" - a dot sending out signal arcs - on a dark rounded tile, with
"Ping Live" in a pixel font underneath. The big sizes are a 64x64 pixel grid
scaled up by whole numbers; 48, 32 and 16 get their own grids without the text,
which would be unreadable that small.

    python scripts/make-icon.py
"""
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets"

TILE = (11, 31, 20, 255)
BORDER = (22, 101, 52, 255)
OUTLINE = (5, 46, 22, 255)
DOT = (187, 247, 208, 255)
ARCS = [(134, 239, 172, 255), (74, 222, 128, 255), (34, 197, 94, 255)]
TEXT = (220, 252, 231, 255)

# 9-row glyphs (row 6 is the baseline, 7-8 the descender of "g").
GLYPHS = {
    "P": ["###.", "#..#", "#..#", "###.", "#...", "#...", "#..."],
    "i": ["#", ".", "#", "#", "#", "#", "#"],
    "n": ["....", "....", "###.", "#..#", "#..#", "#..#", "#..#"],
    "g": ["....", "....", ".###", "#..#", "#..#", "#..#", ".###", "...#", "###."],
    "L": ["#...", "#...", "#...", "#...", "#...", "#...", "####"],
    "v": [".....", ".....", "#...#", "#...#", ".#.#.", ".#.#.", "..#.."],
    "e": ["....", "....", ".##.", "#..#", "####", "#...", ".###"],
}


def tile(n, radius, border):
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    px = img.load()
    for y in range(n):
        for x in range(n):
            # distance outside the inner (un-rounded) square, per axis
            dx = max(radius - x - 0.5, x + 0.5 - (n - radius), 0)
            dy = max(radius - y - 0.5, y + 0.5 - (n - radius), 0)
            d = (dx * dx + dy * dy) ** 0.5
            if d <= radius:
                px[x, y] = BORDER if border and d > radius - 1.2 else TILE
    # plain square edges get the border too
    if border:
        for i in range(radius, n - radius):
            for (x, y) in ((i, 0), (i, n - 1), (0, i), (n - 1, i)):
                px[x, y] = BORDER
    return img


def signal(img, cx, cy, dot_r, rings, outline):
    px = img.load()
    n = img.width
    drawn = set()
    for y in range(n):
        for x in range(n):
            dx, dy = x + 0.5 - cx, y + 0.5 - cy
            d = (dx * dx + dy * dy) ** 0.5
            if d <= dot_r:
                px[x, y] = DOT
                drawn.add((x, y))
                continue
            if dy < 0 and abs(dx) <= -dy * 1.05:  # 90-degree wedge, pointing up
                for (r0, r1), color in zip(rings, ARCS):
                    if r0 <= d <= r1:
                        px[x, y] = color
                        drawn.add((x, y))
    if outline:
        for (x, y) in list(drawn):
            for nx, ny in ((x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)):
                if (nx, ny) not in drawn and 0 <= nx < n and 0 <= ny < n and px[nx, ny] == TILE:
                    px[nx, ny] = OUTLINE


def text(img, s, y0):
    px = img.load()
    widths = [3 if ch == " " else len(GLYPHS[ch][0]) for ch in s]
    total = sum(widths) + len(s) - 1
    x = (img.width - total) // 2
    for ch, w in zip(s, widths):
        if ch != " ":
            for row, line in enumerate(GLYPHS[ch]):
                for col, c in enumerate(line):
                    if c == "#":
                        px[x + col, y0 + row] = TEXT
        x += w + 1


def big():
    img = tile(64, 9, True)
    signal(img, 32, 40, 3.2, [(9, 12), (16, 19), (23, 26)], True)
    text(img, "Ping Live", 48)
    return img


def medium():
    img = tile(32, 5, True)
    signal(img, 16, 24, 2.2, [(6, 8.5), (11, 13.5), (16, 18.5)], True)
    return img


def mid48():
    img = tile(48, 7, True)
    signal(img, 24, 34, 2.8, [(8, 11), (14, 17), (20, 23)], True)
    return img


def small():
    img = tile(16, 3, False)
    signal(img, 8, 13, 1.5, [(3.6, 5.4), (7, 9)], False)
    return img


def up(img, k):
    return img.resize((img.width * k, img.height * k), Image.NEAREST)


def main():
    ASSETS.mkdir(exist_ok=True)
    b, m, s = big(), medium(), small()
    frames = [up(b, 4), up(b, 2), b, mid48(), m, s]  # 256 128 64 48 32 16
    frames[0].save(ASSETS / "pinglive.ico", format="ICO",
                   sizes=[(f.width, f.height) for f in frames], append_images=frames[1:])
    frames[0].save(ASSETS / "pinglive-256.png")
    # side-by-side preview of every size, for eyeballing
    sheet = Image.new("RGBA", (sum(f.width for f in frames) + 10 * len(frames), 256), (40, 40, 40, 255))
    x = 0
    for f in frames:
        sheet.paste(f, (x, 256 - f.height), f)
        x += f.width + 10
    sheet.save(ASSETS / "icon-sizes-preview.png")
    print("wrote", ASSETS / "pinglive.ico")


if __name__ == "__main__":
    main()
