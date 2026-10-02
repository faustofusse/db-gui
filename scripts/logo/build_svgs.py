"""Snaps traced.svg to the logo palette and writes dbear-logo.svg, dbear-icon.svg, bear-layer.svg."""
import re, json
src = open("traced.svg").read()
pal = {"dark": (0x35,0x26,0x21), "taupe": (0x82,0x6f,0x61), "muzzle": (0xa3,0x97,0x8d)}
hexs = {k: "#%02x%02x%02x" % v for k, v in pal.items()}
paths = re.findall(r'<path\s+d="([^"]+)"\s+fill="#([0-9A-Fa-f]{6})"\s+transform="translate\(([-\d.]+),([-\d.]+)\)"\s*/>', src)
if not paths:  # attribute order varies
    paths = [(re.search(r'd="([^"]+)"', p).group(1), re.search(r'fill="#([0-9A-Fa-f]{6})"', p).group(1),
              *re.search(r'translate\(([-\d.]+),([-\d.]+)\)', p).groups()) for p in re.findall(r'<path[^>]*/>', src)]
def nearest(h):
    c = tuple(int(h[i:i+2], 16) for i in (0, 2, 4))
    return min(pal, key=lambda k: sum((a-b)**2 for a, b in zip(c, pal[k])))
out = []
for d, fill, tx, ty in paths:
    out.append((d, nearest(fill), float(tx), float(ty)))
print("paths", [(k, round(len(d))) for d, k, *_ in out])
# bear bbox (from flat.png alpha, 1024-tall image)
from PIL import Image
import numpy as np
al = np.asarray(Image.open("flat.png"))[:, :, 3]
ys, xs = np.nonzero(al)
x0, y0, x1, y1 = xs.min(), ys.min(), xs.max() + 1, ys.max() + 1
bw, bh = x1 - x0, y1 - y0
print("bbox", x0, y0, bw, bh)
body = "\n".join(f'  <path fill="{hexs[k]}" transform="translate({tx - x0:.2f} {ty - y0:.2f})" d="{d}"/>' for d, k, tx, ty in out)

# 1. Logo: bear only, tight viewBox.
open("dbear-logo.svg", "w").write(
    f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {bw} {bh}" width="{bw}" height="{bh}">\n{body}\n</svg>\n')

# 2. macOS icon (classic): 1024 canvas, 824 squircle-ish rounded rect at 100,100 (Apple grid), bear centered.
size, inset, radius = 1024, 100, 185
box = size - 2 * inset
target_h = box * 0.70
s = target_h / bh
ox = (size - bw * s) / 2
oy = inset + (box - bh * s) / 2 + box * 0.02
icon = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}" width="{size}" height="{size}">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#fbf8f4"/>
      <stop offset="1" stop-color="#e9e1d8"/>
    </linearGradient>
    <filter id="shadow" x="-10%" y="-10%" width="120%" height="125%">
      <feDropShadow dx="0" dy="10" stdDeviation="12" flood-color="#000" flood-opacity="0.28"/>
    </filter>
  </defs>
  <rect x="{inset}" y="{inset}" width="{box}" height="{box}" rx="{radius}" fill="url(#bg)" filter="url(#shadow)"/>
  <g transform="translate({ox:.2f} {oy:.2f}) scale({s:.4f})">
{body}
  </g>
</svg>
'''
open("dbear-icon.svg", "w").write(icon)

# 3. Bear layer for Icon Composer: square artboard, bear centered with breathing room.
side = max(bw, bh) * 1.0
open("bear-layer.svg", "w").write(
    f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{-(side-bw)/2:.1f} {-(side-bh)/2:.1f} {side} {side}" width="680" height="680">\n{body}\n</svg>\n')
