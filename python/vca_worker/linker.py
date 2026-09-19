"""截图去重：**dHash（差异感知哈希）**。

为什么不用 imagehash 的 pHash：
  完整 pHash 需要 scipy（DCT 变换），而 scipy 的 wheel 有 35 MB；
  dHash 只需要 numpy，效果在「判断 PPT 是否翻页」这个场景下同样够用，
  且计算更快。部署体积因此少了 35 MB。

原理：
  把图缩到 (n+1) x n 的灰度图，比较每个像素与右邻居的亮度，
  得到 n*n 位指纹；两张图指纹的汉明距离越小，画面越相似。
  对于「PPT 没翻页」的连续截图，距离通常为 0~4；
  翻页后会明显变大，因此用阈值就能筛掉重复帧。
"""

from __future__ import annotations

import os
from typing import Any, Dict, List, Optional, Tuple


def _dhash(path: str, size: int = 8) -> Optional[Tuple[int, ...]]:
    """计算 dHash；缺依赖或读图失败返回 None。"""
    try:
        import numpy as np  # type: ignore
        from PIL import Image  # type: ignore
    except ImportError:
        return None
    try:
        with Image.open(path) as im:
            # 缩到 (size+1) x size，转灰度；LANCZOS 保证缩小后仍保留结构
            small = im.convert("L").resize((size + 1, size), Image.LANCZOS)
            arr = np.asarray(small, dtype=np.int16)
        diff = arr[:, 1:] > arr[:, :-1]
        return tuple(int(x) for x in diff.flatten())
    except Exception:
        return None


def hamming(a: Tuple[int, ...], b: Tuple[int, ...]) -> int:
    """两个指纹的汉明距离。"""
    return sum(1 for x, y in zip(a, b) if x != y)


def dedupe(paths: List[str], threshold: int = 5, limit: int = 6) -> Dict[str, Any]:
    """按 dHash 汉明距离去重，保留前 `limit` 张。

    - `threshold`：距离 <= 该值视为「同一画面」，丢弃后来的（默认 5）；
    - 缺 numpy/Pillow 或读图失败时，退化为**均匀抽样**，保证永不失败。
    """
    exist = [p for p in (paths or []) if p and os.path.exists(p)]
    if not exist:
        return {"kept": [], "dropped": [], "mode": "empty"}

    fingerprints = []
    for p in exist:
        h = _dhash(p)
        if h is None:
            step = max(1, len(exist) // max(1, limit))
            kept = exist[::step][:limit]
            return {
                "kept": kept,
                "dropped": [p for p in exist if p not in kept],
                "mode": "uniform-fallback",
            }
        fingerprints.append((p, h))

    kept: List[str] = []
    dropped: List[str] = []
    for p, h in fingerprints:
        if len(kept) >= limit:
            dropped.append(p)
            continue
        duplicate = False
        for kp in kept:
            kh = dict(fingerprints)[kp]
            if hamming(h, kh) <= threshold:
                duplicate = True
                break
        if duplicate:
            dropped.append(p)
        else:
            kept.append(p)

    return {"kept": kept, "dropped": dropped, "mode": "dhash"}
