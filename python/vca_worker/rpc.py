"""JSON-RPC 2.0 over stdio 的最小实现（框架骨架）。

与 Rust 侧 `vca-ipc` crate 的契约保持一致：
方法名为 init / run / cleanup / health_check / describe。

TODO(骨架): 仅支持单行 JSON 消息；后续可加批量与流式。
"""

from __future__ import annotations

import json
import sys
from typing import Any, Callable, Dict

JSONRPC_VERSION = "2.0"


def ensure_utf8_stdio() -> None:
    """强制 stdin/stdout 使用 UTF-8。

    Windows 下子进程的管道默认使用区域编码（简体中文为 GBK），
    而 JSON-RPC 协议约定 UTF-8。若不显式切换，宿主会解码失败。
    """
    for stream in (sys.stdin, sys.stdout):
        try:
            stream.reconfigure(encoding="utf-8", errors="strict")  # type: ignore[attr-defined]
        except (AttributeError, ValueError):
            # 极旧版本 Python 或不支持重配置的流：忽略（此时依赖 ensure_ascii 兜底）
            pass




class RpcError(Exception):
    """带错误码的 RPC 异常。"""

    def __init__(self, code: int, message: str, data: Any = None) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.data = data


def make_response(req_id: Any, result: Any) -> Dict[str, Any]:
    """构造成功响应。"""
    return {"jsonrpc": JSONRPC_VERSION, "id": req_id, "result": result}


def make_error(req_id: Any, code: int, message: str, data: Any = None) -> Dict[str, Any]:
    """构造错误响应。"""
    err: Dict[str, Any] = {"code": code, "message": message}
    if data is not None:
        err["data"] = data
    return {"jsonrpc": JSONRPC_VERSION, "id": req_id, "error": err}


def serve(handlers: Dict[str, Callable[[Any], Any]]) -> int:
    """从 stdin 逐行读取 JSON-RPC 请求并分发到 handlers。

    返回进程退出码（0 表示正常结束）。
    """
    ensure_utf8_stdio()
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError as exc:
            _write(make_error(None, -32700, f"解析错误: {exc}"))
            continue

        req_id = req.get("id")
        method = req.get("method", "")
        params = req.get("params")

        handler = handlers.get(method)
        if handler is None:
            _write(make_error(req_id, -32601, f"未知方法: {method}"))
            continue

        try:
            _write(make_response(req_id, handler(params)))
        except RpcError as exc:
            _write(make_error(req_id, exc.code, exc.message, exc.data))
        except Exception as exc:  # noqa: BLE001 - 骨架阶段统一兜底
            _write(make_error(req_id, -32603, f"内部错误: {exc}"))

    return 0


def _write(payload: Dict[str, Any]) -> None:
    """向 stdout 写出一行 JSON，并立即刷新。"""
    sys.stdout.write(json.dumps(payload, ensure_ascii=False) + "\n")
    sys.stdout.flush()
