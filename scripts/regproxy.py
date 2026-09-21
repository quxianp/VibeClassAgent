# -*- coding: utf-8 -*-
"""本地 sparse registry 代理：把 cargo 的 index 请求转发到 index.crates.io。

为什么要它：本机沙箱下 cargo 自己的 TLS（走系统 schannel）会被拒
（`SEC_E_NO_CREDENTIALS`），而 .toolchain/cargo/config.toml 把 crates-io
替换成了 `sparse+http://127.0.0.1:13579/`。这个进程就是那个 13579。

它没在跑时的症状很误导：cargo 只会一遍遍刷
`spurious network error ... Could not connect to server`，
看起来像卡死，其实是代理不在。

用法：
    python scripts/regproxy.py         # 前台（开发时挂着即可）
    REGPROXY_PORT=13579 python ...     # 换端口

只转发 index（元数据）；`.crate` 包体由 config.json 里的 dl 地址决定，
走的是 static.crates.io，不在本代理的职责范围内。
"""

import http.server
import json
import os
import re
import urllib.error
import urllib.request

UPSTREAM = "https://index.crates.io"
# `.crate` 包体不在 index 上：cargo 会请求 `<源>/dl/{name}/{version}/download`，
# 真实文件在 static.crates.io 的 `crates/{name}/{name}-{version}.crate`。
# 不转这一段，cargo 会拿到 404 NoSuchKey（实测），而报错里只提代理地址，
# 很容易误判成「代理没起来」。
DL = "https://static.crates.io/crates"
PORT = int(os.environ.get("REGPROXY_PORT", "13579"))

_DL_RE = re.compile(r"^/dl/([^/]+)/([^/]+)/download$")


def upstream_for(path: str) -> str:
    """把 cargo 请求的路径映射到上游真实地址。"""
    m = _DL_RE.match(path)
    if m:
        name, ver = m.group(1), m.group(2)
        return f"{DL}/{name}/{name}-{ver}.crate"
    return UPSTREAM + path


def rewrite_config(data: bytes) -> bytes:
    """把上游 config.json 里的 dl 地址改成指向本代理。

    不这么做的话会撞上一个很绕的坑：上游给的是**绝对**地址
    `https://static.crates.io/crates`，cargo 会拿它**直连** ——
    而本机 schannel 是被拒的（`SEC_E_NO_CREDENTIALS`），于是又变成「下载失败」，
    报错里却只说 TLS 握手失败，看不出是代理没兜住。
    指针改到本地之后，包体也走 Python 的 TLS，与 index 同一条路，不再有例外。
    """
    try:
        obj = json.loads(data)
    except Exception:  # noqa: BLE001 - 上游格式变了就原样透传
        return data
    if isinstance(obj, dict) and "dl" in obj:
        obj["dl"] = f"http://127.0.0.1:{PORT}/dl"
        return json.dumps(obj).encode()
    return data


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):  # noqa: N802 - BaseHTTPRequestHandler 的命名约定
        url = upstream_for(self.path)
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "vca-regproxy"})
            with urllib.request.urlopen(req, timeout=60) as r:
                data = r.read()
                if self.path == "/config.json":
                    data = rewrite_config(data)
                self.send_response(r.status)
                self.send_header("Content-Type", r.headers.get("Content-Type", "text/plain"))
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
        except urllib.error.HTTPError as e:
            body = e.read()
            self.send_response(e.code)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        except Exception as e:  # noqa: BLE001 - 代理就是要兜住一切
            msg = f"regproxy upstream error: {e}".encode()
            self.send_response(502)
            self.send_header("Content-Length", str(len(msg)))
            self.end_headers()
            self.wfile.write(msg)

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    print(f"regproxy listening on 127.0.0.1:{PORT} -> {UPSTREAM}", flush=True)
    srv.serve_forever()
