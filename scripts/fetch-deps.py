# -*- coding: utf-8 -*-
"""获取运行时需要的第三方组件（ffmpeg / whisper.cpp / 语音模型）。

为什么要有这个脚本
------------------
这些组件加起来 200 MB 上下，不可能塞进 Git 仓库，也不该由用户手动找。
部署到一台新的一体机时，跑一次本脚本即可把所有外部依赖摆到位：

    tools/
      ffmpeg/ffmpeg.exe         屏幕采集与音视频合并
      whisper/whisper-cli.exe   本地语音转写（可离线、零调用费）
      whisper/models/ggml-*.bin 转写模型（tiny / base / small）

脚本特性
--------
- 支持 HTTP 代理（VCA_PROXY / HTTPS_PROXY / https_proxy），受限网络下也能跑；
- 已存在的文件默认跳过，可以反复执行（断点式补齐）；
- 每个组件独立失败，不互相拖累；
- 模型源自动在主站与国内镜像之间回退。

用法
----
    python scripts/fetch-deps.py                # 全部（含 tiny 模型）
    python scripts/fetch-deps.py --models tiny  # 只取 tiny 模型
    python scripts/fetch-deps.py --only ffmpeg  # 只取 ffmpeg
    python scripts/fetch-deps.py --force        # 忽略已存在，重新下载
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import urllib.error
import urllib.request
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOOLS = ROOT / "tools"

# Windows 控制台默认是 GBK，中文提示会变乱码；
# 这里把标准流强制切到 UTF-8（失败也不影响功能）。
for _stream in (sys.stdout, sys.stderr):
    try:
        _stream.reconfigure(encoding="utf-8")  # type: ignore[union-attr]
    except Exception:  # noqa: BLE001
        pass

FFMPEG_DIR = TOOLS / "ffmpeg"
WHISPER_DIR = TOOLS / "whisper"
MODEL_DIR = WHISPER_DIR / "models"

FFMPEG_URLS = [
    "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip",
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip",
]

# whisper.cpp 的预编译包在 GitHub Release 上，资产名随版本略有变化，
# 所以先问 API 要最新的 x64 资产地址，拿不到再用兜底直链。
WHISPER_RELEASE_API = "https://api.github.com/repos/ggml-org/whisper.cpp/releases/latest"
WHISPER_ASSET_HINT = "bin-x64"
WHISPER_URL_FALLBACKS = [
    "https://github.com/ggml-org/whisper.cpp/releases/download/v1.7.6/whisper-bin-x64.zip",
]

MODEL_SOURCES = {
    "tiny": [
        "https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
    ],
    "base": [
        "https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
    ],
    "small": [
        "https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
    ],
}

# 低于这个大小基本可以断定是错误页而不是真文件
MIN_REASONABLE = {
    "ffmpeg": 20 * 1024 * 1024,
    "whisper": 200 * 1024,
    "model": 20 * 1024 * 1024,
}


def proxy_url() -> str | None:
    for k in ("VCA_PROXY", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"):
        v = os.environ.get(k)
        if v and v.strip():
            return v.strip()
    return None


def make_opener():
    p = proxy_url()
    if p:
        return urllib.request.build_opener(
            urllib.request.ProxyHandler({"http": p, "https": p})
        )
    return urllib.request.build_opener()


def download(opener, url: str, dest: Path, min_size: int = 1) -> bool:
    """下载到 dest，成功返回 True。"""
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".part")
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
        with opener.open(req, timeout=600) as r, open(tmp, "wb") as f:
            shutil.copyfileobj(r, f, 1024 * 256)
        size = tmp.stat().st_size
        if size < min_size:
            print(f"      文件只有 {size} 字节，判定为无效响应")
            tmp.unlink(missing_ok=True)
            return False
        tmp.replace(dest)
        print(f"      OK {dest.relative_to(ROOT)} ({size / 1048576:.1f} MB)")
        return True
    except Exception as e:  # noqa: BLE001
        print(f"      失败 {type(e).__name__}: {e}")
        tmp.unlink(missing_ok=True)
        return False


def extract_from_zip(zip_path: Path, match_suffix: str, dest_dir: Path) -> list[Path]:
    """把 zip 里匹配后缀的文件解到 dest_dir（保持扁平）。"""
    written: list[Path] = []
    dest_dir.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(zip_path) as zf:
        for name in zf.namelist():
            if not name.lower().endswith(match_suffix.lower()):
                continue
            target = dest_dir / Path(name).name
            with zf.open(name) as src, open(target, "wb") as dst:
                shutil.copyfileobj(src, dst, 1024 * 256)
            written.append(target)
    return written


def ensure_ffmpeg(opener, force: bool) -> bool:
    exe = FFMPEG_DIR / "ffmpeg.exe"
    if exe.is_file() and not force:
        print(f"[ffmpeg] 已存在，跳过：{exe.relative_to(ROOT)}")
        return True
    print("[ffmpeg] 开始下载")
    tmp = TOOLS / "_dl_ffmpeg.zip"
    for url in FFMPEG_URLS:
        print(f"  -> {url}")
        if download(opener, url, tmp, MIN_REASONABLE["ffmpeg"]):
            got = extract_from_zip(tmp, "/bin/ffmpeg.exe", FFMPEG_DIR)
            if not got:
                got = extract_from_zip(tmp, "ffmpeg.exe", FFMPEG_DIR)
            tmp.unlink(missing_ok=True)
            if got:
                print(f"[ffmpeg] 就绪：{got[0].relative_to(ROOT)}")
                return True
            print("[ffmpeg] 压缩包里没找到 ffmpeg.exe")
    print("[ffmpeg] 全部源都失败")
    return False


def resolve_whisper_url(opener) -> list[str]:
    urls: list[str] = []
    try:
        req = urllib.request.Request(
            WHISPER_RELEASE_API, headers={"User-Agent": "VibeClassAgent"}
        )
        with opener.open(req, timeout=60) as r:
            data = json.load(r)
        for asset in data.get("assets", []):
            name = asset.get("name", "")
            if WHISPER_ASSET_HINT in name and name.lower().endswith(".zip"):
                urls.append(asset["browser_download_url"])
    except Exception as e:  # noqa: BLE001
        print(f"      查询 Release 失败（用兜底直链）：{type(e).__name__}")
    urls.extend(WHISPER_URL_FALLBACKS)
    return urls


def ensure_whisper(opener, force: bool) -> bool:
    exe = WHISPER_DIR / "whisper-cli.exe"
    if exe.is_file() and not force:
        print(f"[whisper] 已存在，跳过：{exe.relative_to(ROOT)}")
        return True
    print("[whisper] 开始下载")
    tmp = TOOLS / "_dl_whisper.zip"
    for url in resolve_whisper_url(opener):
        print(f"  -> {url}")
        if download(opener, url, tmp, MIN_REASONABLE["whisper"]):
            # 预编译包里同时有 whisper-cli.exe 与它依赖的若干 dll，
            # 必须一起取出，只拿 exe 会因缺 dll 起不来。
            got: list[Path] = []
            # 必须先把目录建出来：zip 里除了 whisper-cli.exe 还有 bench.exe 等，
            # 目标目录不存在就会在写第一个文件时抛 FileNotFoundError。
            WHISPER_DIR.mkdir(parents=True, exist_ok=True)
            with zipfile.ZipFile(tmp) as zf:
                for name in zf.namelist():
                    low = name.lower()
                    if name.endswith("/") or "__macosx" in low:
                        continue
                    if low.endswith((".exe", ".dll")):
                        target = WHISPER_DIR / Path(name).name
                        with zf.open(name) as src, open(target, "wb") as dst:
                            shutil.copyfileobj(src, dst, 1024 * 256)
                        got.append(target)
            tmp.unlink(missing_ok=True)
            if (WHISPER_DIR / "whisper-cli.exe").is_file():
                print(f"[whisper] 就绪：{WHISPER_DIR.relative_to(ROOT)}（{len(got)} 个文件）")
                return True
            print("[whisper] 包里没有 whisper-cli.exe")
    print("[whisper] 全部源都失败")
    return False


def ensure_models(opener, names: list[str], force: bool) -> bool:
    ok = True
    for name in names:
        target = MODEL_DIR / f"ggml-{name}.bin"
        if target.is_file() and not force:
            print(f"[model] {name} 已存在，跳过")
            continue
        print(f"[model] 开始下载 {name}")
        got = False
        for url in MODEL_SOURCES.get(name, []):
            print(f"  -> {url}")
            if download(opener, url, target, MIN_REASONABLE["model"]):
                got = True
                break
        if not got:
            print(f"[model] {name} 全部源都失败")
            ok = False
    return ok


def main() -> int:
    ap = argparse.ArgumentParser(description="获取 VibeClassAgent 的运行时依赖")
    ap.add_argument("--only", choices=["ffmpeg", "whisper", "models"], default=None)
    ap.add_argument("--models", nargs="*", default=["tiny"], choices=list(MODEL_SOURCES))
    ap.add_argument("--force", action="store_true", help="已存在也重新下载")
    args = ap.parse_args()

    opener = make_opener()
    p = proxy_url()
    print(f"代理：{p or '(直连)'}")
    print(f"目标目录：{TOOLS}")
    print()

    results = {}
    if args.only in (None, "ffmpeg"):
        results["ffmpeg"] = ensure_ffmpeg(opener, args.force)
    if args.only in (None, "whisper"):
        results["whisper"] = ensure_whisper(opener, args.force)
    if args.only in (None, "models"):
        results["models"] = ensure_models(opener, args.models, args.force)

    print()
    for k, v in results.items():
        print(f"  {k:<8} {'成功' if v else '失败'}")
    return 0 if all(results.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
