# -*- coding: utf-8 -*-
"""生成占位图标 assets/icon.ico。

为什么要有这个脚本
------------------
正式 LOGO 还没定，但托盘、窗口、快捷方式都要有图。与其在代码里写死一个
只用一次的图形，不如把它做成**可替换的资源文件**：`assets/icon.ico`。
拿到正式 LOGO 后，直接覆盖这个文件即可，一行代码都不用改。

这里手写 ICO 字节（不依赖 Pillow）：ICO 就是个容器，
里面装一张 32 位 BGRA 的 DIB 加一张 1 位掩码，
现代 Windows 只看 alpha 通道，掩码全 0 即可。

用法
----
    python scripts/make-icon.py                 # 32x32
    python scripts/make-icon.py --sizes 16,32,48,256
"""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets" / "icon.ico"

# 配色：陶土橙（与 CLI 主色一致）+ 白色字母
ORANGE = (31, 111, 235)  # 注意：下面按 BGRA 存，这里是 (R,G,B) 的占位
BRAND = (0xD9, 0x77, 0x57)  # Claude 风陶土橙
INK = (0xFF, 0xFF, 0xFF)


def rounded_coverage(x: float, y: float, size: int, radius: float) -> float:
    """圆角矩形的覆盖率（SDF + 0.5 阈值，够做抗锯齿）。"""
    cx = abs(x - size / 2.0) - (size / 2.0 - radius)
    cy = abs(y - size / 2.0) - (size / 2.0 - radius)
    qx, qy = max(cx, 0.0), max(cy, 0.0)
    sdf = (qx * qx + qy * qy) ** 0.5 + min(max(cx, cy), 0.0) - radius
    return max(0.0, min(1.0, 0.5 - sdf))


def dist_to_segment(px: float, py: float, ax: float, ay: float, bx: float, by: float) -> float:
    vx, vy = bx - ax, by - ay
    wx, wy = px - ax, py - ay
    seg = vx * vx + vy * vy
    t = 0.0 if seg == 0 else max(0.0, min(1.0, (wx * vx + wy * vy) / seg))
    dx, dy = wx - t * vx, wy - t * vy
    return (dx * dx + dy * dy) ** 0.5


def letter_v(x: float, y: float, size: int, stroke: float) -> float:
    """字母 V 的覆盖率：两条斜线的距离场。"""
    left = (0.30 * size, 0.28 * size)
    right = (0.70 * size, 0.28 * size)
    bottom = (0.50 * size, 0.76 * size)
    d = min(
        dist_to_segment(x, y, left[0], left[1], bottom[0], bottom[1]),
        dist_to_segment(x, y, right[0], right[1], bottom[0], bottom[1]),
    )
    return max(0.0, min(1.0, stroke - d + 0.5))


def bgra_bitmap(size: int) -> bytes:
    """生成 size×size 的 32 位 BGRA 像素（自下而上）。"""
    radius = max(1.0, size * 0.22)
    stroke = max(1.0, size * 0.085)
    rows = []
    for y in range(size - 1, -1, -1):  # DIB 是 bottom-up
        row = bytearray()
        for x in range(size):
            fx, fy = x + 0.5, y + 0.5
            cov = rounded_coverage(fx, fy, size, radius)
            if cov <= 0.0:
                row += b"\x00\x00\x00\x00"
                continue
            lc = letter_v(fx, fy, size, stroke)
            r = BRAND[0] * (1 - lc) + INK[0] * lc
            g = BRAND[1] * (1 - lc) + INK[1] * lc
            b = BRAND[2] * (1 - lc) + INK[2] * lc
            a = cov * 255.0
            # 预乘 alpha（Windows 图标是预乘的）
            row += bytes(
                (
                    int(b * cov),
                    int(g * cov),
                    int(r * cov),
                    int(a),
                )
            )
        rows.append(bytes(row))
    return b"".join(rows)


def and_mask(size: int) -> bytes:
    """1 位掩码：全 0（不透明），现代 Windows 只看 alpha。"""
    stride = ((size + 31) // 32) * 4  # 每行按 4 字节对齐
    return b"\x00" * (stride * size)


def build_image(size: int) -> bytes:
    """BITMAPINFOHEADER + 像素 + 掩码。"""
    header = struct.pack(
        "<IiiHHIIiiII",
        40,          # biSize
        size,        # biWidth
        size * 2,    # biHeight = 2×（颜色 + 掩码，DIB 惯例）
        1,           # biPlanes
        32,          # biBitCount
        0,           # biCompression = BI_RGB
        0,           # biSizeImage
        0, 0, 0, 0,  # 分辨率与调色板
    )
    return header + bgra_bitmap(size) + and_mask(size)


def build_ico(sizes: list[int]) -> bytes:
    images = [build_image(s) for s in sizes]
    count = len(images)
    out = struct.pack("<HHH", 0, 1, count)  # ICONDIR
    offset = 6 + 16 * count
    entries = b""
    for s, img in zip(sizes, images):
        entries += struct.pack(
            "<BBBBHHII",
            0 if s >= 256 else s,  # 宽（256 记作 0）
            0 if s >= 256 else s,  # 高
            0,                     # 调色板数
            0,                     # 保留
            1,                     # 色彩平面
            32,                    # 位深
            len(img),
            offset,
        )
        offset += len(img)
    return out + entries + b"".join(images)


def main() -> int:
    ap = argparse.ArgumentParser(description="生成占位图标")
    ap.add_argument("--sizes", default="16,32,48,256", help="逗号分隔的尺寸")
    ap.add_argument("--out", default=str(OUT))
    args = ap.parse_args()

    sizes = [int(s) for s in args.sizes.split(",") if s.strip()]
    data = build_ico(sizes)
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(data)
    print(f"已生成 {out}（{len(data)} 字节，尺寸 {sizes}）")
    print("换成正式 LOGO 时直接覆盖这个文件即可，代码不用改。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
