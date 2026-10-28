#!/usr/bin/env python3
"""Draws the app icon, two screens joined by a line, as a 1024x1024 PNG.

    scripts/make-icon.py OUTPUT.png

Drawn from code so that no image file has to be kept in the repository.
"""
import struct
import sys
import zlib

SIZE = 1024


def inside_round_rect(x, y, left, top, right, bottom, radius):
    if x < left or x >= right or y < top or y >= bottom:
        return False
    cx = min(max(x, left + radius), right - radius - 1)
    cy = min(max(y, top + radius), bottom - radius - 1)
    return (x - cx) ** 2 + (y - cy) ** 2 <= radius ** 2


def pixel(x, y):
    # Background tile with a vertical blue gradient.
    if not inside_round_rect(x, y, 40, 40, SIZE - 40, SIZE - 40, 200):
        return (0, 0, 0, 0)
    t = y / SIZE
    r, g, b = int(70 - 30 * t), int(140 - 50 * t), int(255 - 40 * t)
    white = (255, 255, 255, 255)
    # Two screens.
    for left, top, right, bottom in ((160, 330, 480, 620), (544, 404, 864, 694)):
        if inside_round_rect(x, y, left, top, right, bottom, 36):
            if inside_round_rect(x, y, left + 28, top + 28, right - 28, bottom - 28, 18):
                return (r, g, b, 255)
            return white
    # A line between them.
    if 480 <= x < 544 and 466 <= y < 506:
        return white
    return (r, g, b, 255)


def chunk(kind, data):
    body = kind + data
    return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)


def main(path):
    rows = []
    for y in range(SIZE):
        row = bytearray([0])
        for x in range(SIZE):
            row += bytes(pixel(x, y))
        rows.append(bytes(row))
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(b"".join(rows), 9))
        + chunk(b"IEND", b"")
    )
    with open(path, "wb") as out:
        out.write(png)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
