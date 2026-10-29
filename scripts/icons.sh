#!/usr/bin/env bash
# Renders egit's icon in every size and format its packages carry, from its two drawings:
#
#     scripts/icons.sh
#
# packaging/icons/egit.svg draws it from 48 pixels up, egit-small.svg (three rows, every shape
# thicker) at 16 to 32. Made from them, beside them:
#
# - egit-<size>.png at the hicolor sizes (16, 22, 24, 32, 48, 64, 96, 128, 192, 256, 512): the
#   Linux menu entry's icons;
# - egit.ico (16, 24, 32, 48, 64, 256): the Windows shortcuts' and Programs and Features' icon;
# - egit.icns: the macOS app's, each image on Apple's grid (the rounded square 824 pixels of
#   1024, centred on a transparent canvas, as macOS's own icons are).
#
# The output is the same bytes every run (no timestamps or other metadata), so the files only
# change in a commit when the drawings do. Needs rsvg-convert (librsvg), ImageMagick 7's
# `magick`, and python3 (the .ico and .icns are written by hand: standard library only).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
icons="$root/packaging/icons"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The drawing for a size: the small one up to 32 pixels.
drawing() {
    if (($1 <= 32)); then echo "$icons/egit-small.svg"; else echo "$icons/egit.svg"; fi
}

# A PNG without anything that would differ between runs.
clean() {
    magick "$1" -strip -define png:exclude-chunk=date,time,tIME,tEXt,zTXt,iTXt "PNG32:$2"
}

for size in 16 22 24 32 48 64 96 128 192 256 512; do
    rsvg-convert --width "$size" --height "$size" "$(drawing "$size")" -o "$work/raw.png"
    clean "$work/raw.png" "$icons/egit-$size.png"
done

# Windows: 32-bit bitmaps up to 64 pixels and a PNG at 256, as Microsoft's guidelines have an
# icon (ImageMagick would store the 256 pixels uncompressed too). Their pixels, raw.
for size in 16 24 32 48 64; do
    magick "$icons/egit-$size.png" -depth 8 "RGBA:$work/rgba-$size"
done
python3 -I - "$work" "$icons" <<'PYTHON'
"""Writes egit.ico: its directory, then each image (a bitmap, or the 256 pixels' PNG)."""
import struct
import sys

work, icons = sys.argv[1], sys.argv[2]


def bitmap(size):
    """A 32-bit DIB: its header (twice the height: the colours, then the mask), the BGRA
    rows bottom up, and the AND mask (set where the image is clear), rows to 32 bits."""
    with open(f"{work}/rgba-{size}", "rb") as raw:
        rgba = raw.read()
    rows = [rgba[y * size * 4:(y + 1) * size * 4] for y in range(size)]
    pixels = b"".join(
        bytes(b for x in range(size) for b in (row[x * 4 + 2], row[x * 4 + 1], row[x * 4], row[x * 4 + 3]))
        for row in reversed(rows)
    )
    stride = (size + 31) // 32 * 4
    mask = b""
    for row in reversed(rows):
        bits = bytearray(stride)
        for x in range(size):
            if row[x * 4 + 3] == 0:
                bits[x // 8] |= 0x80 >> (x % 8)
        mask += bytes(bits)
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, len(pixels) + len(mask), 0, 0, 0, 0)
    return header + pixels + mask


images = [(size, bitmap(size)) for size in (16, 24, 32, 48, 64)]
with open(f"{icons}/egit-256.png", "rb") as png:
    images.append((256, png.read()))
directory = struct.pack("<HHH", 0, 1, len(images))
offset = len(directory) + 16 * len(images)
entries, body = b"", b""
for size, data in images:
    side = size % 256  # 256 is written 0
    entries += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(data), offset + len(body))
    body += data
with open(f"{icons}/egit.ico", "wb") as ico:
    ico.write(directory + entries + body)
PYTHON

# macOS: the drawing's rounded square (928 of 1024 in egit.svg, 960 in egit-small.svg) scaled
# to 824 of the canvas, centred.
mac() {
    local size=$1 square inner
    if ((size <= 32)); then square=960; else square=928; fi
    inner=$(((size * 824 + square / 2) / square))
    rsvg-convert --width "$inner" --height "$inner" "$(drawing "$size")" -o "$work/raw.png"
    magick "$work/raw.png" -background none -gravity center -extent "${size}x${size}" \
        "$work/padded.png"
    clean "$work/padded.png" "$work/mac-$size.png"
}
for size in 32 64 128 256 512 1024; do
    mac "$size"
done

python3 -I - "$work" "$icons/egit.icns" <<'PYTHON'
"""Writes an icns file of PNG images: its header, then one entry per image."""
import struct
import sys

work, out = sys.argv[1], sys.argv[2]
# Each entry's type and the pixels it holds (the @2x types hold twice their points).
ENTRIES = [
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic07", 128),
    (b"ic13", 256),
    (b"ic08", 256),
    (b"ic14", 512),
    (b"ic09", 512),
    (b"ic10", 1024),
]
body = b""
for kind, size in ENTRIES:
    with open(f"{work}/mac-{size}.png", "rb") as png:
        data = png.read()
    body += kind + struct.pack(">I", len(data) + 8) + data
with open(out, "wb") as icns:
    icns.write(b"icns" + struct.pack(">I", len(body) + 8) + body)
PYTHON

echo "rendered into $icons"
