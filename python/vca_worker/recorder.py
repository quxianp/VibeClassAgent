"""屏幕录制：基于 PyAV（自带 FFmpeg 库，**不需要**独立安装 ffmpeg.exe）。

为什么走这条路：
  独立分发的 ffmpeg.exe 约 100 MB，在网络受限的机器上很难拿到；
  而 PyAV 的 wheel 里已经打包了 FFmpeg 的全部编解码与设备支持，
  直接复用即可，部署体积反而更小。

音频设备是怎么找的：
  不去猜设备名（各机器完全不同），而是
  1. 用 Windows 的 `waveInGetDevCapsW` 枚举**真实存在的输入设备**；
  2. 逐个尝试用 dshow 打开，第一个成功的就用它；
  3. 结果缓存到 `.audio_device` 文件，下次直接复用，不再重复探测。
  打不开某个设备时 dshow 可能**不返回错误而是阻塞**，所以探测放在
  守护线程里做超时控制。

线程模型：
  录制在**调用线程**里跑，通过 `threading.Event` 接收停止信号，
  因此宿主只需 `set()` 即可优雅停止，无需强杀进程。
"""

from __future__ import annotations

import ctypes
import os
import threading
import time
from ctypes import wintypes
from fractions import Fraction
from typing import Any, Callable, Dict, List, Optional

# 缓存文件：记住上次探测成功的设备名
_CACHE_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".audio_device")


def enumerate_input_devices() -> List[str]:
    """用 Windows waveIn API 枚举音频输入设备名。

    比让 FFmpeg 自己枚举更可靠：dshow 的设备名与系统「录制设备」名一致，
    而 waveIn 给出的正是系统设备列表。
    """
    if os.name != "nt":
        return []
    try:
        winmm = ctypes.WinDLL("winmm")

        class WAVEINCAPSW(ctypes.Structure):
            _fields_ = [
                ("wMid", wintypes.WORD),
                ("wPid", wintypes.WORD),
                ("vDriverVersion", wintypes.UINT),
                ("szPname", wintypes.WCHAR * 32),
                ("dwFormats", wintypes.DWORD),
                ("wChannels", wintypes.WORD),
                ("wReserved1", wintypes.WORD),
            ]

        out: List[str] = []
        for i in range(winmm.waveInGetNumDevs()):
            caps = WAVEINCAPSW()
            if winmm.waveInGetDevCapsW(i, ctypes.byref(caps), ctypes.sizeof(caps)) == 0:
                name = (caps.szPname or "").strip()
                if name:
                    out.append(name)
        return out
    except Exception:
        return []


def _try_open_audio(name: str, timeout: float = 8.0) -> bool:
    """在守护线程里试开设备；超时视为失败。

    dshow 遇到打不开的设备时可能**既不报错也不返回**，因此必须有超时保护。
    """
    result = {"ok": False}

    def work() -> None:
        try:
            import av  # type: ignore

            c = av.open(f"audio={name}", format="dshow")
            streams = list(c.streams.audio)
            c.close()
            result["ok"] = len(streams) > 0
        except Exception:
            result["ok"] = False

    t = threading.Thread(target=work, daemon=True)
    t.start()
    t.join(timeout)
    return result["ok"]


def _load_cached_device() -> Optional[str]:
    try:
        with open(_CACHE_FILE, "r", encoding="utf-8") as f:
            name = f.read().strip()
        return name or None
    except Exception:
        return None


def _save_cached_device(name: str) -> None:
    try:
        with open(_CACHE_FILE, "w", encoding="utf-8") as f:
            f.write(name)
    except Exception:
        pass


def find_working_audio_device(force: bool = False) -> Optional[str]:
    """返回一个能被 dshow 打开的音频设备名；找不到返回 None。

    优先用缓存；缓存失效时重新枚举探测。探测顺序把
    「阵列麦克风 / Microphone / 麦克风」这类**内置麦克风**排在前面，
    虚拟声卡排后面，避免默认录到远端音频。
    """
    if not force:
        cached = _load_cached_device()
        if cached and cached in enumerate_input_devices():
            return cached

    devices = enumerate_input_devices()
    if not devices:
        return None

    def priority(n: str) -> int:
        low = n.lower()
        if "virtual" in low or "cable" in low or "vb-audio" in low or "uu" in low:
            return 3
        if "array" in low or "阵列" in low or "microphone" in low or "麦克风" in low:
            return 1
        return 2

    for name in sorted(devices, key=priority):
        if _try_open_audio(name):
            _save_cached_device(name)
            return name
    return None


def available_audio_devices() -> List[str]:
    """列出**实际可用**（能被 dshow 打开）的音频设备。"""
    return [n for n in enumerate_input_devices() if _try_open_audio(n, timeout=6.0)]

def _open_screen(fps: int, offset_x: int = 0, offset_y: int = 0):
    """打开屏幕输入。"""
    import av  # type: ignore

    options = {
        "framerate": str(fps),
        "draw_mouse": "1",
        "offset_x": str(offset_x),
        "offset_y": str(offset_y),
    }
    return av.open("desktop", format="gdigrab", options=options, timeout=10)


def available_audio_devices() -> List[str]:
    """列出 Windows 上能被 dshow 打开的音频输入设备名。

    通过逐个尝试候选名实现PyAV 未暴露设备枚举接口，
    但「能打开」本身就是最可靠的判据。
    """
    import av  # type: ignore

    found = []
    for name in _AUDIO_CANDIDATES:
        try:
            c = av.open(f"audio={name}", format="dshow", timeout=5)
            c.close()
            found.append(name)
        except Exception:
            continue
    return found


def record(
    out_path: str,
    fps: int = 8,
    height: int = 720,
    crf: int = 30,
    audio: bool = True,
    stop_event: Optional[threading.Event] = None,
    max_seconds: Optional[int] = None,
    screenshot_dir: Optional[str] = None,
    shot_interval: int = 180,
    on_progress: Optional[Callable[[Dict[str, Any]], None]] = None,
) -> Dict[str, Any]:
    """录制到 `out_path`，直到 `stop_event` 置位或超过 `max_seconds`。

    **架构**：屏幕与音频各自跑一个读取线程，把帧丢进队列；
    主线程按顺序取出、打上正确的时间戳并 mux 进输出文件。

    为什么不能在一个线程里轮流读两个设备：dshow 音频按**实时速率**交付数据，
    在视频循环里同步等音频会把整个循环拖慢（实测掉到 2 fps），
    而且两个设备各自的时间戳基准不同，混用会让时长算成几十小时。
    线程分读 + 主线程统一打戳可以彻底避开这两个坑。
    """
    try:
        import av  # type: ignore
    except ImportError as exc:
        raise RuntimeError(
            "未安装 PyAV。请执行：python -m pip install -r requirements.txt"
        ) from exc

    import queue

    stop_event = stop_event or threading.Event()
    notes: List[str] = []
    shots: List[str] = []

    os.makedirs(os.path.dirname(out_path) or ".", exist_ok=True)
    if screenshot_dir:
        os.makedirs(screenshot_dir, exist_ok=True)

    # ---- 输入 ----
    screen = _open_screen(fps)
    screen_stream = screen.streams.video[0]

    audio_in = None
    audio_stream_in = None
    if audio:
        dev = find_working_audio_device()
        if dev:
            try:
                audio_in = av.open(f"audio={dev}", format="dshow")
                audio_stream_in = audio_in.streams.audio[0]
                notes.append(f"音频设备：{dev}")
            except Exception as exc:  # noqa: BLE001
                notes.append(f"打开音频设备失败（{dev}）：{str(exc)[:80]}")
                audio_in = None
        if audio_in is None:
            notes.append(
                "未找到可用音频输入设备，本次为纯视频录制（转写将无内容）。"
                "请检查 Windows「声音设置  录制」是否有已启用的麦克风。"
            )

    # ---- 输出 ----
    out = av.open(out_path, mode="w")
    vstream = out.add_stream("libx264", rate=fps)
    vstream.width = screen_stream.codec_context.width
    vstream.height = screen_stream.codec_context.height
    vstream.pix_fmt = "yuv420p"
    vstream.options = {"crf": str(crf), "preset": "veryfast", "tune": "stillimage"}
    vstream.time_base = Fraction(1, fps)

    # 缩放目标
    src_h = screen_stream.codec_context.height
    target_h = 0
    target_w = 0
    if height and src_h > height:
        target_h = (int(height) // 2) * 2
        target_w = int(screen_stream.codec_context.width * (target_h / float(src_h)) / 2) * 2
        vstream.width = target_w
        vstream.height = target_h

    astream = None
    if audio_in is not None:
        try:
            astream = out.add_stream("aac", rate=audio_stream_in.codec_context.sample_rate)
            astream.bit_rate = 64000
            astream.time_base = Fraction(1, int(audio_stream_in.codec_context.sample_rate))
        except Exception as exc:  # noqa: BLE001
            notes.append(f"音频编码器不可用，改为纯视频：{str(exc)[:80]}")
            astream = None

    # ---- 读取线程 ----
    video_q: "queue.Queue" = queue.Queue(maxsize=16)
    audio_q: "queue.Queue" = queue.Queue(maxsize=256)

    def read_video() -> None:
        try:
            for fr in screen.decode(video=0):
                if stop_event.is_set():
                    break
                video_q.put(fr)  # 满了就阻塞，形成背压
        except Exception as exc:  # noqa: BLE001
            notes.append(f"屏幕读取中断：{str(exc)[:80]}")
        finally:
            video_q.put(None)

    def read_audio() -> None:
        if audio_in is None:
            return
        try:
            for fr in audio_in.decode(audio=0):
                if stop_event.is_set():
                    break
                try:
                    audio_q.put(fr, timeout=1.0)
                except queue.Full:
                    continue  # 主线程跟不上时丢帧，避免内存膨胀
        except Exception as exc:  # noqa: BLE001
            notes.append(f"音频读取中断：{str(exc)[:80]}")

    th_v = threading.Thread(target=read_video, daemon=True)
    th_a = threading.Thread(target=read_audio, daemon=True)
    th_v.start()
    th_a.start()

    # ---- 主循环：统一打戳并 mux ----
    started = time.time()
    frames = 0
    samples_written = 0
    last_shot = 0.0
    first_video_pts = None

    try:
        while True:
            try:
                vframe = video_q.get(timeout=2.0)
            except queue.Empty:
                if stop_event.is_set():
                    break
                continue
            if vframe is None:
                break
            if max_seconds and (time.time() - started) >= max_seconds:
                stop_event.set()
                break

            if target_w:
                vframe = vframe.reformat(width=target_w, height=target_h, format="yuv420p")
            else:
                vframe = vframe.reformat(format="yuv420p")

            # 视频时间戳：从 0 开始的帧序号（配合 time_base = 1/fps）
            if first_video_pts is None:
                first_video_pts = frames
            vframe.pts = frames - first_video_pts
            vframe.time_base = Fraction(1, fps)
            for pkt in vstream.encode(vframe):
                out.mux(pkt)
            frames += 1

            # 音频：把队列里现有的都取出，按累计样本数打戳
            if astream is not None:
                while True:
                    try:
                        aframe = audio_q.get_nowait()
                    except queue.Empty:
                        break
                    try:
                        aframe.pts = samples_written
                        aframe.time_base = Fraction(1, int(astream.rate))
                        samples_written += int(aframe.samples)
                        for pkt in astream.encode(aframe):
                            out.mux(pkt)
                    except Exception as exc:  # noqa: BLE001
                        notes.append(f"音频编码中断：{str(exc)[:60]}")
                        astream = None
                        break

            # 截图
            if screenshot_dir and (time.time() - last_shot) >= shot_interval:
                last_shot = time.time()
                try:
                    p = _save_shot(vframe, screenshot_dir, len(shots) + 1)
                    if p:
                        shots.append(p)
                except Exception as exc:  # noqa: BLE001
                    notes.append(f"截图失败：{str(exc)[:60]}")

            if on_progress and frames % max(1, fps * 30) == 0:
                on_progress({"frames": frames, "seconds": int(time.time() - started)})

        # ---- 收尾 ----
        stop_event.set()  # 通知读取线程退出
        for pkt in vstream.encode(None):
            out.mux(pkt)
        if astream is not None:
            for pkt in astream.encode(None):
                out.mux(pkt)
    finally:
        stop_event.set()
        try:
            out.close()
        except Exception:
            pass
        try:
            screen.close()
        except Exception:
            pass
        if audio_in is not None:
            try:
                audio_in.close()
            except Exception:
                pass

    elapsed = max(1, int(time.time() - started))
    return {
        "video": out_path if os.path.exists(out_path) else "",
        "screenshots": shots,
        "seconds": elapsed,
        "frames": frames,
        "audio_used": audio_in is not None and astream is not None,
        "notes": notes,
    }


def _save_shot(frame, out_dir: str, seq: int) -> Optional[str]:
    """把一帧存成 JPEG。"""
    try:
        from PIL import Image  # type: ignore
    except ImportError:
        return None
    img = frame.to_image()
    ts = time.strftime("%Y%m%d_%H%M%S")
    path = os.path.join(out_dir, f"{ts}_shot{seq:03d}.jpg")
    img.convert("RGB").save(path, "JPEG", quality=80, optimize=True)
    return path


def probe_capabilities() -> Dict[str, Any]:
    """自检：PyAV 是否可用、能否打开屏幕、有哪些音频设备。"""
    out: Dict[str, Any] = {"pyav": False, "screen": False, "audio_devices": [], "error": None}
    try:
        import av  # type: ignore

        out["pyav"] = True
        out["ffmpeg_version"] = getattr(av, "__version__", "?")
    except ImportError as exc:
        out["error"] = f"未安装 PyAV: {exc}"
        return out
    try:
        c = _open_screen(1)
        out["screen"] = True
        out["screen_size"] = (
            c.streams.video[0].codec_context.width,
            c.streams.video[0].codec_context.height,
        )
        c.close()
    except Exception as exc:  # noqa: BLE001
        out["error"] = f"无法打开屏幕输入：{str(exc)[:120]}"
    try:
        out["audio_all"] = enumerate_input_devices()
        # 只挑一个能用的，不全量试打不开的设备可能阻塞十几秒
        out["audio_selected"] = find_working_audio_device()
    except Exception as exc:  # noqa: BLE001
        out["audio_error"] = str(exc)[:120]
    return out
