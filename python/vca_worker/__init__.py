"""VibeClassAgent 算法侧 Worker。

以子进程形式被 Rust 宿主按需拉起，通过 JSON-RPC over stdio 通信。

**依赖自动挂载**：本目录同级有一个 `vendor/` 目录，内含全部第三方库
（python-docx / PyAV / faster-whisper / fpdf2 / Pillow / numpy）。
之所以不用 pip 安装，是因为本项目要能部署在**没有网络、没有管理员权限**的
一体机上把 wheel 解压后随程序分发是最稳的方式。
"""

import os as _os
import sys as _sys

# 把 vendor/ 放到 sys.path 最前面，优先于系统 site-packages
_VENDOR = _os.path.join(_os.path.dirname(_os.path.dirname(_os.path.abspath(__file__))), "vendor")
if _os.path.isdir(_VENDOR) and _VENDOR not in _sys.path:
    _sys.path.insert(0, _VENDOR)

__version__ = "0.2.0"
API_VERSION = "1"
