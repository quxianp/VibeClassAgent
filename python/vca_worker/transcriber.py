"""语音转写：faster-whisper（本地，零调用费，不上云）。

设计要点：
- 完全离线：音频不离开本机；
- 分段处理：长音频按 10 分钟切片，避免单次占用过多内存；
- 优雅降级：未安装 faster-whisper 时返回明确错误（由 Rust 侧决定是否跳过）。
"""

from __future__ import annotations

import os
from typing import Any, Dict, List, Optional

# 模型缓存：同一进程内复用，避免重复加载（加载一次约数百 MB）
_MODEL_CACHE: Dict[str, Any] = {}


def _load_model(model_size: str, threads: int):
    """加载（并缓存）Whisper 模型。"""
    key = f"{model_size}@{threads}"
    if key in _MODEL_CACHE:
        return _MODEL_CACHE[key]
    try:
        from faster_whisper import WhisperModel  # type: ignore
    except ImportError as exc:  # pragma: no cover - 依赖缺失路径
        raise RuntimeError(
            "未安装 faster-whisper。请执行：python -m pip install -r requirements.txt"
        ) from exc

    cpu = os.cpu_count() or 2
    nt = threads if threads and threads > 0 else max(1, min(2, cpu - 1))
    # int8 量化在 CPU 上速度最好、内存占用最低，适合低配一体机
    model = WhisperModel(model_size, device="cpu", compute_type="int8", cpu_threads=nt)
    _MODEL_CACHE[key] = model
    return model


def split_audio(path: str, chunk_seconds: int = 600) -> List[str]:
    """把长音频切成片段。

    优先用 ffmpeg 切片；ffmpeg 不可用时直接返回原文件（不切片）。
    """
    if not os.path.exists(path):
        raise FileNotFoundError(f"音频不存在: {path}")
    try:
        import subprocess
        import tempfile

        probe = subprocess.run(
            ["ffprobe", "-v", "error", "-show_entries", "format=duration",
             "-of", "default=noprint_wrappers=1:nokey=1", path],
            capture_output=True, text=True, timeout=60,
        )
        if probe.returncode != 0:
            return [path]
        duration = float(probe.stdout.strip() or "0")
    except Exception:
        return [path]

    if duration <= chunk_seconds:
        return [path]

    tmpdir = tempfile.mkdtemp(prefix="vca_audio_")
    parts: List[str] = []
    n = int(duration // chunk_seconds) + 1
    for i in range(n):
        out = os.path.join(tmpdir, f"part{i:03d}.wav")
        try:
            r = subprocess.run(
                ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
                 "-ss", str(i * chunk_seconds), "-t", str(chunk_seconds),
                 "-i", path, "-ar", "16000", "-ac", "1", out],
                capture_output=True, timeout=600,
            )
            if r.returncode == 0 and os.path.exists(out) and os.path.getsize(out) > 0:
                parts.append(out)
        except Exception:
            continue
    return parts or [path]


def transcribe(
    audio_path: str,
    model_size: str = "tiny",
    language: str = "zh",
    threads: int = 0,
    chunk_seconds: int = 600,
) -> Dict[str, Any]:
    """转写音频，返回 {text, segments, chunks, model}。"""
    model = _load_model(model_size, threads)
    parts = split_audio(audio_path, chunk_seconds)

    all_text: List[str] = []
    all_segments: List[Dict[str, Any]] = []
    offset = 0.0

    for idx, p in enumerate(parts):
        try:
            segments, info = model.transcribe(
                p,
                language=language or None,
                beam_size=5,
                vad_filter=True,  # 过滤静音，显著提速
                vad_parameters={"min_silence_duration_ms": 500},
            )
        except Exception as exc:
            raise RuntimeError(f"第 {idx + 1} 段转写失败: {exc}") from exc

        for s in segments:
            text = (s.text or "").strip()
            if not text:
                continue
            all_segments.append(
                {
                    "start": float(s.start) + offset,
                    "end": float(s.end) + offset,
                    "text": text,
                }
            )
            all_text.append(text)
        # 段落之间的时间偏移（用 info.duration 更准确）
        if getattr(info, "duration", None):
            offset += float(info.duration)
        else:
            offset += chunk_seconds

    return {
        "text": "\n".join(all_text),
        "segments": all_segments,
        "chunks": len(parts),
        "model": model_size,
    }
