#!/usr/bin/env python3
"""生成 RunHelp 应用图标（纯标准库，无需 Pillow）。
生成 32x32、128x128、256x256 PNG 与 Windows ICO。
"""

import struct
import zlib
from pathlib import Path

ICON_DIR = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"
ICON_DIR.mkdir(parents=True, exist_ok=True)

BLUE = (91, 140, 255)
WHITE = (255, 255, 255)


def make_pixels(size: int) -> list:
    px = [[BLUE for _ in range(size)] for _ in range(size)]
    cx, cy = size / 2, size / 2
    r = size * 0.30
    thick = max(2, int(size * 0.09))
    for y in range(size):
        for x in range(size):
            dx, dy = x - cx, y - cy
            d = (dx * dx + dy * dy) ** 0.5
            if r - thick <= d <= r:
                if dx > 0 and abs(dy) < size * 0.06:
                    continue
                px[y][x] = WHITE
            if d <= size * 0.07:
                px[y][x] = WHITE
    return px


def png_bytes(size: int) -> bytes:
    raw = bytearray()
    px = make_pixels(size)
    for row in px:
        raw.append(0)
        for r, g, b in row:
            raw.extend((r, g, b))

    def chunk(tag: bytes, data: bytes) -> bytes:
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def ico_bytes(sizes: list) -> bytes:
    images = [(s, png_bytes(s)) for s in sizes]
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries = bytearray()
    data = bytearray()
    for s, png in images:
        w = 0 if s >= 256 else s
        h = 0 if s >= 256 else s
        entries.extend(struct.pack("<BBBBHHII", w, h, 0, 0, 1, 32, len(png), offset))
        data.extend(png)
        offset += len(png)
    return bytes(header) + bytes(entries) + bytes(data)


ICON_DIR.joinpath("32x32.png").write_bytes(png_bytes(32))
ICON_DIR.joinpath("128x128.png").write_bytes(png_bytes(128))
ICON_DIR.joinpath("128x128@2x.png").write_bytes(png_bytes(256))
ICON_DIR.joinpath("icon.ico").write_bytes(ico_bytes([16, 32, 48, 64, 128, 256]))
print("图标已生成:", ", ".join(p.name for p in sorted(ICON_DIR.iterdir())))
