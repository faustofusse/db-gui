"""Flattens the logo JPEG to its three flat colors (dark fur, taupe, muzzle) on transparency.
Reads in.png (1024px tall), writes flat.png. Coordinates below are in that image."""
import numpy as np
from PIL import Image, ImageFilter
img = Image.open("in.png").convert("RGB").filter(ImageFilter.MedianFilter(5))
a = np.asarray(img).astype(float)
def sample(x, y, r=6): return a[y-r:y+r, x-r:x+r].reshape(-1, 3).mean(0)
# sampled in the 1024px image
dark, taupe = sample(320, 704), sample(602, 538)
lum = a @ [0.299, 0.587, 0.114]
head = a[150:400, 600:900]; hl = lum[150:400, 600:900]
cand = head[(hl > 140) & (hl < 190)]
muzzle = np.median(cand, 0)
print("muzzle candidates", len(cand))
bg_in, bg_out = sample(200, 250), sample(20, 20)
print("dark #%02x%02x%02x taupe #%02x%02x%02x muzzle #%02x%02x%02x" % tuple(int(v) for v in np.concatenate([dark, taupe, muzzle])))
pal = np.array([dark, taupe, muzzle, bg_in, bg_out])
lab = ((a[:, :, None, :] - pal[None, None]) ** 2).sum(-1).argmin(-1)
h, w, _ = a.shape
# The muzzle color only exists on the snout; elsewhere it's antialiasing between fur and
# background, which would trace as a light rim. Reclassify those pixels without it.
yy, xx = np.mgrid[0:h, 0:w]
snout = (xx > 640) & (yy < 360)
alt = pal.copy(); alt[2] = 1e6
lab2 = ((a[:, :, None, :] - alt[None, None]) ** 2).sum(-1).argmin(-1)
lab = np.where((lab == 2) & ~snout, lab2, lab)
# Thin taupe slivers are antialiasing too (dark fur against the white background).
# An opening (erode, then dilate) removes anything a few px wide and keeps real areas.
taupe_mask = Image.fromarray(((lab == 1) * 255).astype(np.uint8))
opened = np.asarray(taupe_mask.filter(ImageFilter.MinFilter(7)).filter(ImageFilter.MaxFilter(7))) > 0
alt = pal.copy(); alt[1] = 1e6; alt[2] = 1e6
lab3 = ((a[:, :, None, :] - alt[None, None]) ** 2).sum(-1).argmin(-1)
lab = np.where((lab == 1) & ~opened, lab3, lab)
out = np.zeros((h, w, 4), np.uint8)
for k in range(3):
    m = lab == k
    out[m, :3] = pal[k].astype(np.uint8); out[m, 3] = 255
Image.fromarray(out, "RGBA").save("flat.png")
