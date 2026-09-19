"""直接用 fpdf2 生成中文 PDF（**不需要 LibreOffice**）。

为什么放弃「先做 Word 再调 LibreOffice 转 PDF」：
  1. LibreOffice 安装包约 350 MB，一体机上装它不现实；
  2. 转换过程要拉起一个重量级进程，弱机上要几十秒；
  3. 我们本来就是在自己排版，直接生成 PDF 反而版式更可控。

字体：优先用 Windows 自带的中文字体（黑体 simhei.ttf）。
fpdf2 需要 TTF（不支持 TTC 字体集合），因此按 TTF 优先的顺序探测。
"""

from __future__ import annotations

import os
from typing import Any, Dict, List, Optional

# TTF 优先（fpdf2 不支持 .ttc 集合）
_FONT_CANDIDATES = [
    r"C:\Windows\Fonts\simhei.ttf",      # 黑体
    r"C:\Windows\Fonts\simkai.ttf",      # 楷体
    r"C:\Windows\Fonts\Deng.ttf",        # 等线
    r"C:\Windows\Fonts\msyh.ttc",        # 雅黑（TTC，最后尝试）
]

_FONT_CACHE: Optional[str] = None


def find_cjk_font() -> Optional[str]:
    """找一个可用于 fpdf2 的中文字体。"""
    global _FONT_CACHE
    if _FONT_CACHE and os.path.exists(_FONT_CACHE):
        return _FONT_CACHE
    for c in _FONT_CANDIDATES:
        if os.path.exists(c):
            _FONT_CACHE = c
            return c
    return None


def _safe_name(s: str) -> str:
    bad = '<>:"/\\|?*'
    return "".join("_" if ch in bad else ch for ch in s).strip() or "未命名"


def build_pdf(
    course: str,
    date: str,
    start: str,
    end: str,
    summary: Dict[str, Any],
    transcript: str,
    screenshots: List[str],
    out_dir: str,
    max_screenshots: int = 6,
    include_transcript: bool = True,
) -> Optional[str]:
    """生成中文 PDF，返回路径（失败抛异常）。"""
    try:
        from fpdf import FPDF  # type: ignore
    except ImportError as exc:
        raise RuntimeError(
            "未安装 fpdf2。请执行：python -m pip install -r requirements.txt"
        ) from exc

    font = find_cjk_font()
    if not font:
        raise RuntimeError("找不到可用的中文字体（尝试了 simhei/simkai/Deng/msyh）")

    os.makedirs(out_dir, exist_ok=True)
    pdf = FPDF()
    pdf.set_auto_page_break(auto=True, margin=15)
    pdf.add_page()
    pdf.add_font("cjk", "", font)
    pdf.set_font("cjk", size=18)

    title = summary.get("title") or course
    pdf.multi_cell(0, 10, f"{course} 课堂纪要", align="C")
    pdf.ln(2)
    pdf.set_font("cjk", size=10)
    pdf.multi_cell(0, 6, f"日期：{date}    时间：{start} - {end}")
    if title and title != course:
        pdf.multi_cell(0, 6, f"主题：{title}")
    pdf.ln(3)

    def heading(text: str) -> None:
        pdf.ln(2)
        pdf.set_font("cjk", size=14)
        pdf.multi_cell(0, 8, text)
        pdf.set_font("cjk", size=11)

    points = summary.get("points") or []
    if points:
        heading("课堂重点")
        for i, p in enumerate(points, 1):
            pdf.multi_cell(0, 6, f"{i}. {p}", new_x="LMARGIN", new_y="NEXT")

    notices = summary.get("notices") or []
    if notices:
        heading("重点通知")
        for n in notices:
            pdf.multi_cell(0, 6, f"- {n}", new_x="LMARGIN", new_y="NEXT")

    keywords = summary.get("keywords") or []
    if keywords:
        heading("关键词")
        pdf.multi_cell(0, 6, "、".join(str(k) for k in keywords))

    shots = [s for s in (screenshots or []) if s and os.path.exists(s)][:max_screenshots]
    if shots:
        heading("关键截图")
        avail = pdf.w - pdf.l_margin - pdf.r_margin
        for s in shots:
            try:
                y = pdf.get_y()
                if y > pdf.h - 100:
                    pdf.add_page()
                pdf.image(s, w=avail)
                pdf.set_font("cjk", size=8)
                pdf.multi_cell(0, 5, os.path.basename(s))
                pdf.set_font("cjk", size=11)
            except Exception:
                continue

    if include_transcript and transcript.strip():
        heading("附：转写全文")
        pdf.set_font("cjk", size=10)
        for line in transcript.splitlines():
            if line.strip():
                pdf.multi_cell(0, 5.5, line.strip(), new_x="LMARGIN", new_y="NEXT")

    path = os.path.join(out_dir, f"{_safe_name(course)}_{date.replace('-', '')}_课堂纪要.pdf")
    pdf.output(path)
    return path
