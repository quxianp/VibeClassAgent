"""Worker 入口。

三种用法：

    python -m vca_worker --stdio
        以 JSON-RPC 服务模式运行，供宿主按需拉起（转写 / 文档 / 去重）。

    python -m vca_worker --record --out <视频路径> [--fps 8] [--height 720]
                        [--shot-dir <目录>] [--shot-interval 180] [--no-audio]
        以**独立录制进程**运行（基于 PyAV，不需要 ffmpeg.exe）。
        从 stdin 读到一行 "stop" 即优雅停止并封装文件。

    python -m vca_worker --selftest
        自检依赖与外部能力。

`run` 载荷（JSON-RPC，供宿主调用）：

    {"kind":"transcribe","audio_path":"...","model":"tiny","language":"zh","threads":0}
    {"kind":"docgen","course":"...","summary_path":"...","transcript_path":"...",
     "screenshots":[...],"out_dir":"...","formats":["docx","pdf"]}
    {"kind":"dedupe","paths":[...],"threshold":5,"limit":6}
    {"kind":"probe"}      -> 录制能力自检（PyAV / 屏幕 / 音频设备）
"""

from __future__ import annotations

import argparse
import sys
import threading
import traceback
from typing import Any

from . import API_VERSION, __version__
from .rpc import RpcError, ensure_utf8_stdio, serve


def _describe(_params: Any = None) -> dict:
    return {
        "id": "worker.python",
        "kind": "transcriber",
        "version": __version__,
        "api_version": API_VERSION,
        "config_schema": {},
        "capabilities": ["transcribe", "docgen", "dedupe", "record", "probe"],
    }


def _init(params: Any = None) -> dict:
    return {"ok": True, "echo": params}


def _run(params: Any = None) -> dict:
    p = params or {}
    kind = str(p.get("kind") or "").strip().lower()

    if kind == "transcribe":
        from .transcriber import transcribe

        return transcribe(
            audio_path=str(p.get("audio_path") or ""),
            model_size=str(p.get("model") or "tiny"),
            language=str(p.get("language") or "zh"),
            threads=int(p.get("threads") or 0),
        )

    if kind == "docgen":
        from .docgen import generate

        return generate(p)

    if kind == "dedupe":
        from .linker import dedupe

        return dedupe(
            list(p.get("paths") or []),
            threshold=int(p.get("threshold") or 5),
            limit=int(p.get("limit") or 6),
        )

    if kind == "probe":
        from .recorder import probe_capabilities

        return probe_capabilities()

    raise RpcError(-32602, f"未知的 kind: {kind!r}（支持 transcribe / docgen / dedupe / probe）")


def _cleanup(_params: Any = None) -> dict:
    try:
        from .transcriber import _MODEL_CACHE

        _MODEL_CACHE.clear()
    except Exception:
        pass
    return {"ok": True}


def _health_check(_params: Any = None) -> dict:
    missing = []
    for mod, name in [
        ("faster_whisper", "faster-whisper"),
        ("docx", "python-docx"),
        ("fpdf", "fpdf2"),
        ("PIL", "Pillow"),
        ("numpy", "numpy"),
        ("av", "PyAV"),
        ("yaml", "PyYAML"),
    ]:
        try:
            __import__(mod)
        except ImportError:
            missing.append(name)
    return {
        "healthy": not missing,
        "message": "全部依赖就绪" if not missing else f"缺失依赖: {', '.join(missing)}",
    }


HANDLERS = {
    "describe": _describe,
    "init": _init,
    "run": _run,
    "cleanup": _cleanup,
    "health_check": _health_check,
}


# ---------------------------------------------------------------------------
# 独立录制模式
# ---------------------------------------------------------------------------

def _record_mode(args) -> int:
    """录制到文件，直到 stdin 收到 "stop" 或进程被要求退出。"""
    from .recorder import record

    stop = threading.Event()

    def watch_stdin() -> None:
        """监听宿主指令：stop = 优雅停止，quit = 立即停止。"""
        try:
            for line in sys.stdin:
                cmd = line.strip().lower()
                if cmd in ("stop", "quit", "q", ""):
                    stop.set()
                    return
        except Exception:
            stop.set()

    threading.Thread(target=watch_stdin, daemon=True).start()

    try:
        res = record(
            out_path=args.out,
            fps=args.fps,
            height=args.height,
            crf=args.crf,
            audio=not args.no_audio,
            stop_event=stop,
            max_seconds=args.max_seconds,
            screenshot_dir=args.shot_dir,
            shot_interval=args.shot_interval,
        )
    except Exception as exc:  # noqa: BLE001
        print(f"ERROR {type(exc).__name__}: {exc}", file=sys.stderr, flush=True)
        return 2

    # 结果同时写文件（宿主读文件更稳妥）与 stdout
    import json

    try:
        with open(args.out + ".record.json", "w", encoding="utf-8") as f:
            json.dump(res, f, ensure_ascii=False, indent=2)
    except Exception:
        pass
    print(json.dumps(res, ensure_ascii=False), flush=True)
    return 0


def _selftest() -> int:
    """检查依赖与外部能力。"""
    print(f"vca_worker {__version__}")
    print("-" * 58)
    checks = [
        ("faster_whisper", "语音转写 (faster-whisper)"),
        ("ctranslate2", "推理加速 (CTranslate2)"),
        ("docx", "Word 生成 (python-docx)"),
        ("fpdf", "PDF 生成 (fpdf2)"),
        ("PIL", "图像处理 (Pillow)"),
        ("numpy", "数值计算 (numpy)"),
        ("av", "屏幕录制 (PyAV，自带 FFmpeg)"),
        ("yaml", "YAML 解析 (PyYAML)"),
    ]
    missing = 0
    for mod, desc in checks:
        try:
            m = __import__(mod)
            v = getattr(m, "__version__", "")
            print(f"[OK  ] {mod:<16} {desc} {v}")
        except ImportError:
            missing += 1
            print(f"[MISS] {mod:<16} {desc}")
    print("-" * 58)

    # 录制能力探测
    try:
        from .recorder import probe_capabilities

        cap = probe_capabilities()
        print(f"[{'OK  ' if cap.get('screen') else 'MISS'}] 屏幕采集" +
              (f"  {cap.get('screen_size')}" if cap.get("screen") else f"  {cap.get('error')}"))
        all_devs = cap.get("audio_all") or []
        sel = cap.get("audio_selected")
        # 说明：这里只看「选中」结果。逐个探测所有设备很慢，
        # 而且打不开的设备可能卡住十几秒，没必要为了展示而全试一遍。
        if sel:
            print(f"[OK  ] 音频设备  已选中：{sel}")
            others = [d for d in all_devs if d != sel]
            if others:
                print(f"        其它可用：{'; '.join(others)}")
        else:
            print("[WARN] 音频设备  未找到可用的麦克风（将纯视频录制，转写无内容）")
            if all_devs:
                print(f"        系统里有 {len(all_devs)} 个输入设备，但都打不开：")
                for d in all_devs:
                    print(f"          - {d}")
                print("        请在「声音设置  录制」里确认麦克风已启用且未被独占")
    except Exception as exc:  # noqa: BLE001
        print(f"[MISS] 录制能力探测失败: {type(exc).__name__}")

    print("-" * 58)
    if missing:
        print(f"{missing} 个依赖缺失（vendor 目录可能不完整）")
    else:
        print("依赖全部就绪。")
    return 0


def main() -> int:
    ensure_utf8_stdio()
    parser = argparse.ArgumentParser(prog="vca_worker", description="VibeClassAgent 算法侧 Worker")
    g = parser.add_mutually_exclusive_group()
    g.add_argument("--stdio", action="store_true", help="以 JSON-RPC stdio 模式运行")
    g.add_argument("--selftest", action="store_true", help="自检依赖与能力")
    g.add_argument("--record", action="store_true", help="以独立录制进程运行")

    parser.add_argument("--out", help="录制输出文件（--record 时必填）")
    parser.add_argument("--fps", type=int, default=8)
    parser.add_argument("--height", type=int, default=720)
    parser.add_argument("--crf", type=int, default=30)
    parser.add_argument("--no-audio", action="store_true", help="不采集音频")
    parser.add_argument("--max-seconds", type=int, default=0, help="最长录制秒数（0=不限）")
    parser.add_argument("--shot-dir", help="截图输出目录")
    parser.add_argument("--shot-interval", type=int, default=180, help="截图间隔（秒）")

    args = parser.parse_args()

    if args.selftest:
        return _selftest()
    if args.record:
        if not args.out:
            print("--record 需要 --out", file=sys.stderr)
            return 2
        return _record_mode(args)
    if args.stdio:
        return serve(HANDLERS)

    parser.print_help()
    return 0


if __name__ == "__main__":
    sys.exit(main())
